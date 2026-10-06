//! Native 3D model view (docs/display-engine/MODELS.md, campaign design).
//!
//! `obj` and `scene` are the pure CPU side: OBJ parsing with the exact
//! semantics stored Gnosis data depends on, and scene normalisation. `gpu`
//! uploads a scene and draws it into a sampled surface texture.

pub mod gpu;
pub mod obj;
pub mod scene;

pub use gpu::{MAX_MODEL_EDGE_PX, MODEL_TEXTURE_FORMAT, ModelGpu, ModelView, model_target_px};
pub use obj::{MAX_OBJ_FILE_BYTES, ObjError, ObjLimits, ObjMesh, parse_obj, read_obj_file};
pub use scene::{
    MAX_MODEL_MESHES, MAX_MODEL_TRIANGLES, MAX_MODEL_VERTICES, ModelScene, SceneError, face_normal,
    remaining_limits,
};
