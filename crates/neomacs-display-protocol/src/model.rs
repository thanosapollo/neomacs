//! Native model view protocol: the values the evaluator, the render thread
//! and the renderer exchange about one model (campaign design §3).
//!
//! Every type here is plain data. The renderer owns geometry and GPU state;
//! the evaluator owns mesh names and callbacks; these types are what crosses
//! between them, in both directions.

/// Camera in canvas-3d units: degrees, degrees, zoom 0.25..=4.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelView {
    pub yaw: f64,
    pub pitch: f64,
    pub zoom: f64,
}

/// Smallest and largest zoom (`gnosis-model--view`).
pub const MODEL_ZOOM_RANGE: std::ops::RangeInclusive<f64> = 0.25..=4.0;
/// Largest absolute camera angle in degrees.
pub const MODEL_MAX_ANGLE: f64 = 1_000_000.0;
/// Logical edge range of a model glyph (design §2).
pub const MODEL_EDGE_RANGE: std::ops::RangeInclusive<u32> = 32..=2048;
/// Most meshes in one model (Gnosis accepts 255 scene objects).
pub const MODEL_MAX_MESHES: usize = 255;

impl Default for ModelView {
    fn default() -> Self {
        Self {
            yaw: 0.0,
            pitch: 0.0,
            zoom: 1.0,
        }
    }
}

impl ModelView {
    /// Validate as `gnosis-model--view` and canvas-3d do.
    pub fn validated(self) -> Result<Self, String> {
        let finite = [self.yaw, self.pitch, self.zoom]
            .iter()
            .all(|v| v.is_finite() && v.abs() <= MODEL_MAX_ANGLE);
        if !finite || !MODEL_ZOOM_RANGE.contains(&self.zoom) {
            return Err("model view needs finite angles and zoom 0.25..4".to_owned());
        }
        Ok(self)
    }

    /// Rows of R = Ry(yaw) * Rx(pitch).
    pub fn rotation(&self) -> [[f64; 3]; 3] {
        let (sy, cy) = self.yaw.to_radians().sin_cos();
        let (sp, cp) = self.pitch.to_radians().sin_cos();
        // Ry = [[cy,0,sy],[0,1,0],[-sy,0,cy]], Rx = [[1,0,0],[0,cp,-sp],[0,sp,cp]].
        [
            [cy, sy * sp, sy * cp],
            [0.0, cp, -sp],
            [-sy, cy * sp, cy * cp],
        ]
    }

    /// This view orbited by a pointer drag of `dx`, `dy` physical pixels:
    /// 0.5° per pixel, both angles wrapped into [0, 360) like canvas-3d's
    /// `canvas-3d-rotate` (no clamp, so a stored (0 -90 z) never jumps on
    /// its first step beyond the wrap).
    pub fn orbited(self, dx: f64, dy: f64) -> Self {
        Self {
            yaw: (self.yaw + 0.5 * dx).rem_euclid(360.0),
            pitch: (self.pitch + 0.5 * dy).rem_euclid(360.0),
            zoom: self.zoom,
        }
    }

    /// This view zoomed by `notches` wheel notches (×1.1 each, positive
    /// zooms in), clamped to the zoom range.
    pub fn zoomed(self, notches: i32) -> Self {
        let zoom = (self.zoom * 1.1f64.powi(notches))
            .clamp(*MODEL_ZOOM_RANGE.start(), *MODEL_ZOOM_RANGE.end());
        Self { zoom, ..self }
    }
}

/// One OBJ file of a model, in Lisp's mesh order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelMeshSpec {
    pub path: std::path::PathBuf,
    /// Expected SHA-256 of the exact bytes parsed, when the caller has one.
    pub sha256: Option<[u8; 32]>,
}

/// A question target painted on the model (design §2). Mesh numbers are
/// indices into the model's mesh list; labels never reach the renderer.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelTarget {
    /// The whole mesh.
    Object { mesh: u16 },
    /// Every fragment of `mesh` within `tolerance` (original units) of the
    /// barycentric point on face `face`.
    Point {
        mesh: u16,
        face: u32,
        barycentric: [f64; 3],
        tolerance: f64,
    },
    /// The listed faces of `mesh`.
    Region { mesh: u16, faces: Vec<u32> },
}

impl ModelTarget {
    pub fn mesh(&self) -> u16 {
        match self {
            Self::Object { mesh } | Self::Point { mesh, .. } | Self::Region { mesh, .. } => *mesh,
        }
    }

    /// Check the parts that do not depend on topology: barycentric weights
    /// finite, non-negative and summing to 1 ± 1e-6, tolerance finite and
    /// positive, a region nonempty and distinct. Face ranges are checked
    /// against the loaded scene.
    pub fn validated(self) -> Result<Self, String> {
        match &self {
            Self::Object { .. } => {}
            Self::Point {
                barycentric,
                tolerance,
                ..
            } => {
                let sum: f64 = barycentric.iter().sum();
                if !barycentric.iter().all(|w| w.is_finite() && *w >= 0.0)
                    || (sum - 1.0).abs() > 1e-6
                {
                    return Err("point target needs finite weights >= 0 summing to 1".to_owned());
                }
                if !tolerance.is_finite() || *tolerance <= 0.0 {
                    return Err("point target needs a finite tolerance > 0".to_owned());
                }
            }
            Self::Region { faces, .. } => {
                if faces.is_empty() {
                    return Err("region target needs at least one face".to_owned());
                }
                let mut sorted = faces.clone();
                sorted.sort_unstable();
                if sorted.windows(2).any(|w| w[0] == w[1]) {
                    return Err("region target faces must be distinct".to_owned());
                }
            }
        }
        Ok(self)
    }
}

/// What Lisp asks of a live model. Each request carries its `req` number.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelChange {
    View(ModelView),
    /// Both fields are replaced. A target suppresses `selected` painting.
    Highlight {
        target: Option<ModelTarget>,
        selected: Option<u16>,
    },
    /// New logical size, in the model edge range.
    Resize {
        width: u32,
        height: u32,
    },
    /// Keep the last picture; stop interaction and further requests.
    Freeze,
}

/// A pick that landed on geometry. `point` is in the original, unrotated
/// OBJ coordinates of mesh `mesh`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelHit {
    pub mesh: u16,
    pub face: u32,
    pub barycentric: [f64; 3],
    pub point: [f64; 3],
}

/// What the renderer reports about one model (design §3, §6.2).
#[derive(Debug, Clone, PartialEq)]
pub enum ModelEvent {
    /// Loaded and uploaded; triangle counts per mesh, in mesh order.
    Ready { triangles: Vec<u32> },
    /// Load, digest, parse, budget or GPU failure. Terminal.
    Failed { error: String },
    /// Non-fatal. `req` names the request this settles (a rejection or an
    /// invalid highlight), `None` when it settles nothing.
    Error { req: Option<u64>, error: String },
    /// A renderer-originated camera change settled (orbit or wheel).
    View { view: ModelView, rev: u64 },
    /// The readiness fence, reported level-triggered (design §6.2).
    Readiness { current: bool, rev: u64, req: u64 },
    /// A click resolved against the picture presented where it landed.
    Pick {
        hit: Option<ModelHit>,
        view: ModelView,
        rev: u64,
        req: u64,
    },
}

impl ModelEvent {
    /// The Lisp event type symbol name.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Ready { .. } => "ready",
            Self::Failed { .. } => "failed",
            Self::Error { .. } => "error",
            Self::View { .. } => "view",
            Self::Readiness { .. } => "readiness",
            Self::Pick { .. } => "pick",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orbit_wraps_without_clamping() {
        let v = ModelView {
            yaw: 0.0,
            pitch: -90.0,
            zoom: 1.1,
        };
        let o = v.orbited(-2.0, 4.0);
        assert_eq!(o.yaw, 359.0);
        assert_eq!(o.pitch, 272.0);
        assert_eq!(o.zoom, 1.1);
    }

    #[test]
    fn zoom_steps_and_clamps() {
        let v = ModelView::default();
        assert!((v.zoomed(1).zoom - 1.1).abs() < 1e-12);
        assert!((v.zoomed(-2).zoom - 1.0 / 1.21).abs() < 1e-12);
        assert_eq!(v.zoomed(100).zoom, 4.0);
        assert_eq!(v.zoomed(-100).zoom, 0.25);
    }

    #[test]
    fn target_validation() {
        let point = |b: [f64; 3], r: f64| ModelTarget::Point {
            mesh: 0,
            face: 0,
            barycentric: b,
            tolerance: r,
        };
        assert!(point([0.2, 0.3, 0.5], 1.0).validated().is_ok());
        assert!(point([0.2, 0.3, 0.6], 1.0).validated().is_err());
        assert!(point([-0.1, 0.6, 0.5], 1.0).validated().is_err());
        assert!(point([0.2, 0.3, 0.5], 0.0).validated().is_err());
        let region = |faces: Vec<u32>| ModelTarget::Region { mesh: 1, faces };
        assert!(region(vec![3, 1]).validated().is_ok());
        assert!(region(vec![]).validated().is_err());
        assert!(region(vec![1, 1]).validated().is_err());
    }
}
