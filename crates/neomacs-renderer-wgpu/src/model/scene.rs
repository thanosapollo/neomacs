//! Scene normalisation for the native model view.
//!
//! A scene is every mesh of one model. Its centre is the bounding-box
//! midpoint and its radius the largest distance from that centre, over *all*
//! vertices (unused ones included), exactly as canvas-3d computed them, so a
//! stored Gnosis camera frames the scene the same way.

use super::obj::{ObjError, ObjLimits, ObjMesh};

/// Most meshes in one model (Gnosis accepts 255 scene objects).
pub const MAX_MODEL_MESHES: usize = 255;
/// Most triangles in one model, over all its meshes.
pub const MAX_MODEL_TRIANGLES: usize = 2_000_000;
/// Most vertices in one model, over all its meshes.
pub const MAX_MODEL_VERTICES: usize = 2_000_000;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SceneError {
    #[error("model needs 1..={MAX_MODEL_MESHES} meshes")]
    MeshCount,
    #[error("mesh {mesh}: {source}")]
    Obj {
        mesh: usize,
        #[source]
        source: ObjError,
    },
    #[error("mesh {mesh}: degenerate triangle {face}")]
    DegenerateTriangle { mesh: usize, face: usize },
    #[error("degenerate geometry")]
    DegenerateScene,
    #[error("model exceeds {MAX_MODEL_VERTICES} vertices or {MAX_MODEL_TRIANGLES} triangles")]
    TooLarge,
}

/// A validated, normalised scene: original geometry plus its frame.
///
/// Fields are private: the only way to obtain one is [`ModelScene::new`],
/// so every index in it is known to be in range and every count fits the
/// `u32` the GPU uses.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelScene {
    meshes: Vec<ObjMesh>,
    /// Global index of each mesh's first triangle (GPU face numbering is
    /// global; Lisp face numbering is per mesh).
    face_starts: Vec<u32>,
    centre: [f64; 3],
    radius: f64,
}

/// What is left of a model's caps after `meshes`.
pub fn remaining_limits(meshes: &[ObjMesh]) -> ObjLimits {
    let vertices: usize = meshes.iter().map(|m| m.vertices.len()).sum();
    let triangles: usize = meshes.iter().map(|m| m.triangles.len()).sum();
    ObjLimits {
        max_vertices: MAX_MODEL_VERTICES.saturating_sub(vertices),
        max_triangles: MAX_MODEL_TRIANGLES.saturating_sub(triangles),
    }
}

/// Unit normal of triangle `t`, or `None` when degenerate.
///
/// Port of canvas-3d's `triangle_normals`: edges are scaled by a power of two
/// before the cross product so tiny geometry does not underflow to zero.
pub fn face_normal(t: [[f64; 3]; 3]) -> Option<[f64; 3]> {
    let e1 = sub(t[1], t[0]);
    let e2 = sub(t[2], t[0]);
    let scale = e1
        .iter()
        .chain(e2.iter())
        .fold(0.0f64, |m, v| m.max(v.abs()));
    if !scale.is_finite() || scale == 0.0 {
        return None;
    }
    // frexp: scale = m * 2^exp with m in [0.5, 1). Apply 2^-exp in two
    // halves so a subnormal scale cannot overflow the factor to infinity.
    let exp = scale.log2().floor() as i32 + 1;
    let half = -exp / 2;
    let (f1, f2) = (2f64.powi(half), 2f64.powi(-exp - half));
    let a = e1.map(|v| v * f1 * f2);
    let b = e2.map(|v| v * f1 * f2);
    let n = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    let length = n[0].hypot(n[1]).hypot(n[2]);
    if !length.is_finite() || length == 0.0 {
        return None;
    }
    Some(n.map(|v| v / length))
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

impl ModelScene {
    /// Validate `meshes` (already parsed within the model caps) and compute
    /// the scene frame.
    pub fn new(meshes: Vec<ObjMesh>) -> Result<Self, SceneError> {
        if meshes.is_empty() || meshes.len() > MAX_MODEL_MESHES {
            return Err(SceneError::MeshCount);
        }
        let vertices: usize = meshes.iter().map(|m| m.vertices.len()).sum();
        let triangles: usize = meshes.iter().map(|m| m.triangles.len()).sum();
        if vertices > MAX_MODEL_VERTICES || triangles > MAX_MODEL_TRIANGLES {
            return Err(SceneError::TooLarge);
        }
        let mut face_starts = Vec::with_capacity(meshes.len());
        let mut next = 0u32;
        for (mesh_index, mesh) in meshes.iter().enumerate() {
            face_starts.push(next);
            // Within the caps above, so the narrowing cannot fail; checked
            // anyway so an unchecked caller can never wrap an index.
            next = u32::try_from(mesh.triangles.len())
                .ok()
                .and_then(|n| next.checked_add(n))
                .ok_or(SceneError::TooLarge)?;
            if mesh
                .triangles
                .iter()
                .flatten()
                .any(|&i| i as usize >= mesh.vertices.len())
            {
                return Err(SceneError::Obj {
                    mesh: mesh_index,
                    source: ObjError::IndexOutOfRange { line: 0 },
                });
            }
            for (face, tri) in mesh.triangles.iter().enumerate() {
                let corners = tri.map(|i| mesh.vertices[i as usize]);
                if face_normal(corners).is_none() {
                    return Err(SceneError::DegenerateTriangle {
                        mesh: mesh_index,
                        face,
                    });
                }
            }
        }
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for v in meshes.iter().flat_map(|m| m.vertices.iter()) {
            for axis in 0..3 {
                lo[axis] = lo[axis].min(v[axis]);
                hi[axis] = hi[axis].max(v[axis]);
            }
        }
        let centre = [0, 1, 2].map(|axis| (hi[axis] + lo[axis]) / 2.0);
        let radius = meshes
            .iter()
            .flat_map(|m| m.vertices.iter())
            .map(|v| {
                let d = sub(*v, centre);
                d[0].hypot(d[1]).hypot(d[2])
            })
            .fold(0.0f64, f64::max);
        if !radius.is_finite() || radius <= 0.0 {
            return Err(SceneError::DegenerateScene);
        }
        Ok(Self {
            meshes,
            face_starts,
            centre,
            radius,
        })
    }

    pub fn meshes(&self) -> &[ObjMesh] {
        &self.meshes
    }

    pub fn centre(&self) -> [f64; 3] {
        self.centre
    }

    pub fn radius(&self) -> f64 {
        self.radius
    }

    /// Triangle counts per mesh, in mesh order.
    pub fn triangle_counts(&self) -> Vec<u32> {
        self.meshes
            .iter()
            .map(|m| m.triangles.len() as u32)
            .collect()
    }

    /// Global GPU face number of `mesh`'s face `face`, when both exist.
    pub fn global_face(&self, mesh: u16, face: u32) -> Option<u32> {
        let m = self.meshes.get(mesh as usize)?;
        ((face as usize) < m.triangles.len()).then(|| self.face_starts[mesh as usize] + face)
    }

    /// The (mesh, per-mesh face) a global GPU face number names.
    pub fn locate(&self, global: u32) -> Option<(u16, u32)> {
        let mesh = self.face_starts.partition_point(|&start| start <= global);
        let mesh = mesh.checked_sub(1)?;
        let local = global - self.face_starts[mesh];
        ((local as usize) < self.meshes[mesh].triangles.len()).then_some((mesh as u16, local))
    }

    /// The original-coordinate point `w0·v0 + w1·v1 + w2·v2` on `mesh`'s
    /// face `face`, in f64 (design §2 point targets, §5.4 picks).
    pub fn point_on(&self, mesh: u16, face: u32, weights: [f64; 3]) -> Option<[f64; 3]> {
        let m = self.meshes.get(mesh as usize)?;
        let tri = m.triangles.get(face as usize)?;
        let v = tri.map(|i| m.vertices[i as usize]);
        Some([0, 1, 2].map(|axis| {
            weights[0] * v[0][axis] + weights[1] * v[1][axis] + weights[2] * v[2][axis]
        }))
    }

    /// `v` in the normalised frame the GPU draws: `(v - centre) / radius`.
    pub fn normalise(&self, v: [f64; 3]) -> [f32; 3] {
        let d = sub(v, self.centre);
        d.map(|c| (c / self.radius) as f32)
    }

    pub fn triangle_count(&self) -> usize {
        self.meshes.iter().map(|m| m.triangles.len()).sum()
    }
}

#[cfg(test)]
#[path = "scene/tests.rs"]
mod tests;
