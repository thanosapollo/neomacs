//! The renderer's model cache (design §4.1): GPU buffers, targets and the
//! budget reservation of every uploaded model, behind [`ModelGpuHost`].
//!
//! A model's resolved texture is installed in the shader-surface cache as an
//! ordinary pipeline-less surface, so both composite paths (inline glyphs
//! and child-frame content) sample it through the one surface resolver,
//! with the same LRU touch and `mark_drawn`.

use std::collections::HashMap;

use super::WgpuRenderer;
use crate::model::budget::ModelBudget;
use crate::model::gpu::{
    MAX_MODEL_EDGE_PX, ModelDraw, ModelGpu, PickPoll, PickRequest, geometry_bytes, model_target_px,
    target_bytes,
};
use crate::model::scene::ModelScene;
use crate::model::store::ModelGpuHost;
use crate::shader_surface_cache::{PreparedSurface, ShaderSurfaceCache};

pub(crate) struct ModelCache {
    models: HashMap<u32, ModelGpu>,
    budget: ModelBudget,
    /// Test fault injection: the next composite installation fails.
    #[cfg(test)]
    fail_next_composite: bool,
}

impl ModelCache {
    pub(crate) fn new() -> Self {
        Self {
            models: HashMap::new(),
            budget: ModelBudget::from_env(),
            #[cfg(test)]
            fail_next_composite: false,
        }
    }
}

impl WgpuRenderer {
    /// Bytes the model budget holds now (for tests and diagnostics).
    pub fn model_budget_used(&self) -> u64 {
        self.models.budget.used()
    }

    fn install_model_texture(&mut self, id: u32, logical: (u32, u32)) -> Result<(), String> {
        let gpu = self.models.models.get(&id).ok_or("model not uploaded")?;
        let ((texture, view), size_px) = (gpu.texture(), gpu.size_px());
        let prepared = self.prepare_model_composite(texture, view, logical, size_px)?;
        self.caches.surface.install_prepared(id, prepared);
        Ok(())
    }

    /// Build the composite surface entry for a model texture without
    /// installing it (the fallible step of every installation).
    fn prepare_model_composite(
        &mut self,
        texture: wgpu::Texture,
        view: wgpu::TextureView,
        logical: (u32, u32),
        size_px: (u32, u32),
    ) -> Result<PreparedSurface, String> {
        #[cfg(test)]
        if std::mem::take(&mut self.models.fail_next_composite) {
            return Err("injected composite failure".to_owned());
        }
        ShaderSurfaceCache::prepare_rendered(
            &self.device,
            self.caches.image.bind_group_layout(),
            self.caches.image.sampler(),
            texture,
            view,
            logical,
            size_px,
        )
    }
}

impl ModelGpuHost for WgpuRenderer {
    fn model_upload(
        &mut self,
        id: u32,
        scene: &ModelScene,
        logical: (u32, u32),
        scale: f32,
    ) -> Result<(u32, u32), String> {
        self.model_free(id);
        let size_px = model_target_px(logical.0, logical.1, scale, MAX_MODEL_EDGE_PX);
        let vertices: usize = scene.meshes().iter().map(|m| m.vertices.len()).sum();
        let bytes = geometry_bytes(vertices as u64, scene.triangle_count() as u64)
            + target_bytes(size_px.0, size_px.1);
        self.models.budget.reserve(id, bytes)?;
        let gpu = match ModelGpu::new(&self.device, &self.queue, scene, size_px) {
            Ok(gpu) => gpu,
            Err(error) => {
                self.models.budget.release_all(id);
                return Err(error);
            }
        };
        self.models.models.insert(id, gpu);
        if let Err(error) = self.install_model_texture(id, logical) {
            self.model_free(id);
            return Err(error);
        }
        tracing::info!(
            "model {id}: {} meshes, {} triangles, {}x{} px, {bytes} GPU bytes",
            scene.meshes().len(),
            scene.triangle_count(),
            size_px.0,
            size_px.1
        );
        Ok(size_px)
    }

    fn model_resize(
        &mut self,
        id: u32,
        logical: (u32, u32),
        scale: f32,
    ) -> Result<(u32, u32), String> {
        let size_px = model_target_px(logical.0, logical.1, scale, MAX_MODEL_EDGE_PX);
        let old = self
            .models
            .models
            .get(&id)
            .ok_or("model not uploaded")?
            .size_px();
        if old == size_px {
            // Same targets: only the logical size the composite uses.
            self.install_model_texture(id, logical)?;
            return Ok(size_px);
        }
        // Transactional: reserve, then stage the new targets AND their
        // composite entry while the old ones stay installed and accounted
        // (that overlap is the peak, and what is admitted). Any failure
        // rolls back the reservation and drops what was staged; only then
        // are both owners committed, infallibly, and the old reservation
        // released.
        let new_bytes = target_bytes(size_px.0, size_px.1);
        self.models.budget.reserve(id, new_bytes)?;
        let staged = match ModelGpu::stage_targets(&self.device, size_px) {
            Ok(staged) => staged,
            Err(error) => {
                self.models.budget.release(id, new_bytes);
                return Err(error);
            }
        };
        let (texture, view) = staged.texture();
        let prepared = match self.prepare_model_composite(texture, view, logical, size_px) {
            Ok(prepared) => prepared,
            Err(error) => {
                drop(staged);
                self.models.budget.release(id, new_bytes);
                return Err(error);
            }
        };
        self.models
            .models
            .get_mut(&id)
            .expect("uploaded")
            .commit_targets(staged);
        self.caches.surface.install_prepared(id, prepared);
        self.models.budget.release(id, target_bytes(old.0, old.1));
        Ok(size_px)
    }

    fn model_render(&mut self, id: u32, draw: &ModelDraw) -> Result<(), String> {
        let gpu = self
            .models
            .models
            .get_mut(&id)
            .ok_or("model not uploaded")?;
        gpu.render(&self.device, &self.queue, draw)
    }

    fn model_free(&mut self, id: u32) {
        if self.models.models.remove(&id).is_some() {
            self.caches.surface.free(id);
        }
        self.models.budget.release_all(id);
    }

    fn model_pick_start(&mut self, id: u32, request: &PickRequest) -> Result<(), String> {
        let gpu = self
            .models
            .models
            .get_mut(&id)
            .ok_or("model not uploaded")?;
        gpu.pick_start(&self.device, &self.queue, request)
    }

    fn model_pick_poll(&mut self, id: u32) -> PickPoll {
        match self.models.models.get_mut(&id) {
            Some(gpu) => gpu.pick_poll(&self.device),
            None => PickPoll::Idle,
        }
    }
}

#[cfg(test)]
#[path = "models/tests.rs"]
mod tests;
