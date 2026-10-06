//! Whether a composed frame's model provenance is one snapshot per model
//! (SOL-REVIEW-2 MUST 2, design §6.2).
//!
//! The fresh glyph passes of a frame record which models they composited
//! and at which snapshot. Some compositions also show pixels of an *older*
//! picture: a pane morph placing composition-ring pictures, a frame or
//! theme transition blending the previous composition, a child frame's
//! resize crossfade layering its previous presentation. Those pixels may
//! hold a model at an older camera (or a model the fresh passes no longer
//! draw), which one snapshot per model cannot describe.

use crate::render_thread::frame_windows::GuiFrameRenderState;
use neomacs_display_protocol::frame_time::FrameSample;

/// Whether the frame just composed for `render` (with `pane_placed`: the
/// composition was placed a pane at a time from the composition ring) also
/// shows pixels of a picture older than its fresh glyph passes. Asked after
/// the composition, at the frame's `sample`.
pub(in crate::render_thread) fn blends_older_pictures(
    render: &GuiFrameRenderState,
    pane_placed: bool,
    sample: FrameSample,
) -> bool {
    pane_placed
        || render.compositor.transitions.has_active()
        || render.compositor.child_frames.has_active_crossfade(sample)
}

#[cfg(test)]
#[path = "model_provenance/tests.rs"]
mod tests;
