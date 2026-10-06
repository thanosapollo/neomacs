//! The model loader thread (design §4.2).
//!
//! One thread, FIFO, at most [`MAX_QUEUED_LOADS`] requests waiting. It reads
//! and parses OBJ files and builds the [`ModelScene`]; it never touches wgpu.
//! Results come back on a channel the render thread drains; while any load
//! is pending the render loop polls (the `has_pending_images` pattern), so
//! an idle editor still completes a load.
//!
//! Cancellation is a flag per request, checked between 1 MB reads. Dropping
//! the loader cancels everything queued or running and closes the channel,
//! so the thread exits within one read step.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use neomacs_display_protocol::model::ModelMeshSpec;

use super::obj::read_obj_file_within;
use super::scene::{ModelScene, remaining_limits};

/// Most requests waiting behind the one being loaded.
pub const MAX_QUEUED_LOADS: usize = 4;
/// Most bytes one model may read over all its files: bounds the input work
/// of 255 files of mostly comments, not only the accepted geometry.
pub const MAX_MODEL_INPUT_BYTES: u64 = 256 * 1024 * 1024;

/// One model to load. `token` is the store's identity for this load; a
/// result whose (id, token) is no longer live is dropped by the caller.
pub struct LoadRequest {
    pub id: u32,
    pub token: u64,
    pub meshes: Vec<ModelMeshSpec>,
    pub cancel: Arc<AtomicBool>,
}

/// The outcome of one [`LoadRequest`].
pub struct LoadResult {
    pub id: u32,
    pub token: u64,
    pub outcome: Result<Arc<ModelScene>, String>,
}

/// Load `meshes` into a scene, within the model caps and the aggregate
/// input-byte allowance, checking `cancel` between reads.
pub fn load_scene(meshes: &[ModelMeshSpec], cancel: &AtomicBool) -> Result<ModelScene, String> {
    if meshes.is_empty() || meshes.len() > super::scene::MAX_MODEL_MESHES {
        return Err(format!(
            "model needs 1..={} meshes",
            super::scene::MAX_MODEL_MESHES
        ));
    }
    let cancelled = || cancel.load(Ordering::Relaxed);
    let mut parsed = Vec::with_capacity(meshes.len());
    let mut allowance = MAX_MODEL_INPUT_BYTES;
    for spec in meshes {
        let (mesh, read) = read_obj_file_within(
            &spec.path,
            spec.sha256.as_ref(),
            remaining_limits(&parsed),
            allowance,
            &cancelled,
        )
        .map_err(|e| format!("{}: {e}", spec.path.display()))?;
        allowance = allowance.saturating_sub(read);
        parsed.push(mesh);
    }
    ModelScene::new(parsed).map_err(|e| e.to_string())
}

/// Handle to the loader thread, owned by the render thread.
pub struct ModelLoader {
    requests: Option<mpsc::SyncSender<LoadRequest>>,
    results: mpsc::Receiver<LoadResult>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// Cancel flags of every request submitted and not yet answered.
    live: Vec<(u32, u64, Arc<AtomicBool>)>,
}

impl ModelLoader {
    pub fn spawn() -> std::io::Result<Self> {
        let (requests, inbox) = mpsc::sync_channel::<LoadRequest>(MAX_QUEUED_LOADS);
        let (outbox, results) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("neomacs-model-loader".to_owned())
            .spawn(move || {
                while let Ok(request) = inbox.recv() {
                    let outcome = if request.cancel.load(Ordering::Relaxed) {
                        Err("model load cancelled".to_owned())
                    } else {
                        load_scene(&request.meshes, &request.cancel).map(Arc::new)
                    };
                    let result = LoadResult {
                        id: request.id,
                        token: request.token,
                        outcome,
                    };
                    if outbox.send(result).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            requests: Some(requests),
            results,
            thread: Some(thread),
            live: Vec::new(),
        })
    }

    /// Queue a load. Errors when [`MAX_QUEUED_LOADS`] are already waiting
    /// ("loader busy") or the thread is gone.
    pub fn submit(&mut self, request: LoadRequest) -> Result<(), String> {
        let sender = self.requests.as_ref().ok_or("model loader stopped")?;
        let entry = (request.id, request.token, request.cancel.clone());
        match sender.try_send(request) {
            Ok(()) => {
                self.live.push(entry);
                Ok(())
            }
            Err(mpsc::TrySendError::Full(_)) => Err("model loader busy".to_owned()),
            Err(mpsc::TrySendError::Disconnected(_)) => Err("model loader stopped".to_owned()),
        }
    }

    /// Cancel the load of `id`, if one is queued or running. Its result
    /// still arrives (as an error) and is dropped by the caller.
    pub fn cancel(&mut self, id: u32) {
        for (live_id, _, flag) in &self.live {
            if *live_id == id {
                flag.store(true, Ordering::Relaxed);
            }
        }
    }

    /// Whether any submitted load has not been answered yet.
    pub fn has_pending(&self) -> bool {
        !self.live.is_empty()
    }

    /// Every result available now, without blocking.
    pub fn drain(&mut self) -> Vec<LoadResult> {
        let mut out = Vec::new();
        while let Ok(result) = self.results.try_recv() {
            self.live
                .retain(|(id, token, _)| !(*id == result.id && *token == result.token));
            out.push(result);
        }
        out
    }
}

impl Drop for ModelLoader {
    fn drop(&mut self) {
        for (_, _, flag) in &self.live {
            flag.store(true, Ordering::Relaxed);
        }
        self.requests = None;
        if let Some(thread) = self.thread.take() {
            // Exits within one 1 MB read: every request is cancelled and the
            // request channel is closed.
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
#[path = "loader/tests.rs"]
mod tests;
