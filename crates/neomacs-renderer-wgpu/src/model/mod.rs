//! Native 3D model view (campaign design).
//!
//! `obj` and `scene` are the pure CPU side: OBJ parsing with the exact
//! semantics stored Gnosis data depends on, and scene normalisation.
//! `loader` runs them on a thread. `gpu` uploads a scene, draws it into a
//! sampled surface texture and answers picks. `store` is the render-thread
//! model state (requests, readiness, interaction) over [`ModelGpuHost`],
//! with `readiness` its level-triggered fence and `budget` its GPU pool.

pub mod budget;
pub mod gpu;
pub mod loader;
pub mod obj;
pub mod readiness;
pub mod scene;
pub mod store;

pub use gpu::{
    GpuHighlight, MAX_MODEL_EDGE_PX, MODEL_TEXTURE_FORMAT, ModelDraw, ModelGpu, ModelView,
    PickPoll, PickRequest, model_target_px,
};
pub use obj::{MAX_OBJ_FILE_BYTES, ObjError, ObjLimits, ObjMesh, parse_obj, read_obj_file};
pub use scene::{
    MAX_MODEL_MESHES, MAX_MODEL_TRIANGLES, MAX_MODEL_VERTICES, ModelScene, SceneError, face_normal,
    remaining_limits,
};
pub use store::{GlyphHit, ModelGpuHost, ModelStatus, ModelStore};
