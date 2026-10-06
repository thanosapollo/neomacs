//! Native models on the render thread (design §4–§6): the glue between the
//! renderer-independent [`ModelStore`], the renderer that implements its GPU
//! host, the frame loop and the ordered input channel.
//!
//! The store survives renderer loss. Every event it produces goes out
//! through `comms.send_input`, the channel keys and mouse events use, after
//! the state change it reports.

use std::collections::HashSet;
use std::time::Instant;

use neomacs_display_protocol::FrameGlyph;
use neomacs_renderer_wgpu::model::store::ModelGpuHost;

use super::RenderApp;

/// The models a picture holds, each with the snapshot its pixels show.
pub(super) type ModelPicture =
    std::collections::BTreeMap<u32, neomacs_renderer_wgpu::model::store::Snapshot>;

impl RenderApp {
    fn model_host(&mut self) -> Option<&mut dyn ModelGpuHost> {
        self.renderer
            .as_mut()
            .map(|renderer| renderer as &mut dyn ModelGpuHost)
    }

    /// Send every pending model event to the evaluator, in order.
    pub(super) fn flush_model_events(&mut self) {
        for (id, event) in self.models.take_events() {
            self.comms
                .send_input(crate::thread_comm::InputEvent::Model { id, event });
        }
    }

    /// Finished loads, uploads a renderer lacks, wheel deadlines and picks.
    /// Runs on every loop wake, so an idle editor still completes a load
    /// and delivers a pick with no further input.
    pub(super) fn service_models(&mut self) {
        if self.models.is_empty() {
            return;
        }
        let mut models = std::mem::take(&mut self.models);
        models.poll_loads(self.model_host());
        if let Some(host) = self.model_host() {
            models.upload_pending(host);
        }
        models.tick(Instant::now(), self.model_host());
        self.models = models;
        self.flush_model_events();
    }

    /// Before a frame composites: follow the device scale, then draw every
    /// dirty model into its texture.
    pub(super) fn render_dirty_models(&mut self) {
        if self.models.is_empty() {
            return;
        }
        let scale = self
            .frame_windows
            .windows
            .values()
            .filter(|window| window.window().is_some())
            .map(|window| window.scale_factor() as f32)
            .fold(0.0f32, f32::max);
        let mut models = std::mem::take(&mut self.models);
        if let Some(host) = self.model_host() {
            if scale > 0.0 {
                models.set_scale(scale, host);
            }
            models.render_dirty(host);
        }
        self.models = models;
        self.flush_model_events();
    }

    /// Top-level window `emacs_frame_id` presented `frame`: update the
    /// showing sets from what the composite drew.
    pub(super) fn note_models_presented(
        &mut self,
        emacs_frame_id: u64,
        frame: &crate::core::frame_glyphs::FrameGlyphBuffer,
    ) {
        let Some(drawn) = self
            .renderer
            .as_mut()
            .and_then(|renderer| renderer.take_drawn_surfaces())
        else {
            return;
        };
        self.note_models_composited(emacs_frame_id, &drawn, frame);
    }

    /// The composite of `frame` in window `emacs_frame_id` drew `drawn`.
    pub(super) fn note_models_composited(
        &mut self,
        emacs_frame_id: u64,
        drawn: &HashSet<u32>,
        frame: &crate::core::frame_glyphs::FrameGlyphBuffer,
    ) {
        if self.models.is_empty() {
            return;
        }
        let in_frame: HashSet<u32> = frame
            .glyphs
            .iter()
            .filter_map(|glyph| match glyph {
                FrameGlyph::Surface { surface_id, .. } => Some(surface_id.get()),
                _ => None,
            })
            .collect();
        self.models.presented(emacs_frame_id, drawn, &in_frame);
        self.flush_model_events();
    }

    /// Window `emacs_frame_id` is hidden or gone: it shows no model.
    pub(super) fn note_models_window_gone(&mut self, emacs_frame_id: u64) {
        if self.models.is_empty() {
            return;
        }
        self.models.window_gone(emacs_frame_id);
        self.flush_model_events();
    }

    /// Window `emacs_frame_id` is hidden, iconified or occluded: it leaves
    /// now (its capture ends) and stays out until shown and presented.
    pub(super) fn note_models_window_hidden(&mut self, emacs_frame_id: u64) {
        self.models.window_hidden(emacs_frame_id);
        self.flush_model_events();
    }

    /// Window `emacs_frame_id` is shown again: its next presentation
    /// rejoins the showing sets.
    pub(super) fn note_models_window_shown(&mut self, emacs_frame_id: u64) {
        self.models.window_shown(emacs_frame_id);
    }

    /// The renderer went away (device loss or teardown).
    pub(super) fn note_models_renderer_lost(&mut self) {
        self.comms.capabilities.set_model_renderer_live(false);
        self.models.renderer_lost();
    }

    /// A renderer is live: publish it and upload what it lacks.
    pub(super) fn note_models_renderer_ready(&mut self) {
        self.comms.capabilities.set_model_renderer_live(true);
        self.service_models();
    }

    /// The next instant the loop must wake for model work.
    pub(super) fn model_deadline(&self, now: Instant) -> Option<Instant> {
        self.models.next_deadline(now)
    }

    pub(super) fn handle_model_create(
        &mut self,
        id: u32,
        meshes: Vec<neomacs_display_protocol::model::ModelMeshSpec>,
        view: neomacs_display_protocol::model::ModelView,
        background: [f32; 4],
        logical: (u32, u32),
    ) {
        self.models.create(id, meshes, view, background, logical);
        self.flush_model_events();
    }

    pub(super) fn handle_model_update(
        &mut self,
        id: u32,
        req: u64,
        change: neomacs_display_protocol::model::ModelChange,
    ) {
        let mut models = std::mem::take(&mut self.models);
        models.update(id, req, change, self.model_host());
        self.models = models;
        self.flush_model_events();
    }

    /// Free surface `id` if it is a model; answers whether it was.
    pub(super) fn handle_model_free(&mut self, id: u32) -> bool {
        if !self.models.contains(id) {
            return false;
        }
        let mut models = std::mem::take(&mut self.models);
        models.free(id, self.model_host());
        self.models = models;
        true
    }
}

#[cfg(test)]
#[path = "models/tests.rs"]
mod tests;
