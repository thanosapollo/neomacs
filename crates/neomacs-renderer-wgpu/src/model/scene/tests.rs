use super::*;
use crate::model::obj::parse_obj;

fn mesh(text: &str) -> ObjMesh {
    parse_obj(text.as_bytes(), remaining_limits(&[])).unwrap()
}

#[test]
fn frame_is_bbox_midpoint_and_max_distance_over_all_vertices() {
    // The unused vertex (10,0,0) still shapes the frame, as in canvas-3d.
    let scene =
        ModelScene::new(vec![mesh("v 0 0 0\nv 2 0 0\nv 0 2 0\nv 10 0 0\nf 1 2 3\n")]).unwrap();
    assert_eq!(scene.centre, [5.0, 1.0, 0.0]);
    let expected = (25.0f64 + 1.0).sqrt();
    assert!((scene.radius - expected).abs() < 1e-12);
    assert_eq!(scene.normalise([5.0, 1.0, 0.0]), [0.0, 0.0, 0.0]);
}

#[test]
fn frame_spans_every_mesh() {
    let scene = ModelScene::new(vec![
        mesh("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n"),
        mesh("v 3 0 0\nv 4 0 0\nv 3 1 0\nf 1 2 3\n"),
    ])
    .unwrap();
    assert_eq!(scene.centre, [2.0, 0.5, 0.0]);
    assert_eq!(scene.triangle_count(), 2);
}

#[test]
fn degenerate_triangles_and_scenes_are_rejected() {
    assert_eq!(
        ModelScene::new(vec![mesh(
            "v 0 0 0\nv 1 0 0\nv 2 0 0\nv 0 1 0\nf 1 2 4\nf 1 2 3\n"
        )])
        .unwrap_err(),
        SceneError::DegenerateTriangle { mesh: 0, face: 1 }
    );
    assert_eq!(ModelScene::new(vec![]).unwrap_err(), SceneError::MeshCount);
}

#[test]
fn tiny_triangles_keep_their_normal() {
    // Products of 1e-200 edges underflow without the power-of-two scaling.
    let n = face_normal([[0.0; 3], [1e-200, 0.0, 0.0], [0.0, 1e-200, 0.0]]).unwrap();
    assert_eq!(n, [0.0, 0.0, 1.0]);
    let n = face_normal([[0.0; 3], [5e-324, 0.0, 0.0], [0.0, 5e-324, 0.0]]).unwrap();
    assert_eq!(n, [0.0, 0.0, 1.0], "subnormal edges");
    assert!(face_normal([[1.0; 3], [1.0; 3], [1.0; 3]]).is_none());
}

#[test]
fn remaining_limits_shrink_across_meshes() {
    let first = mesh("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n");
    let left = remaining_limits(&[first]);
    assert_eq!(left.max_vertices, MAX_MODEL_VERTICES - 3);
    assert_eq!(left.max_triangles, MAX_MODEL_TRIANGLES - 1);
}

/// Parity with canvas-3d on a real Gnosis asset, when present on this host.
/// `NEOMACS_MODEL_PARITY_OBJ` names an OBJ and `NEOMACS_MODEL_PARITY_SUMMARY`
/// a file of "vertices triangles f0a f0b f0c flast_a flast_b flast_c sha256"
/// written by canvas-3d's `obj_geometry` for it, where sha256 covers every
/// vertex (little-endian f64) followed by every triangle (little-endian u32),
/// so all coordinates and the whole topology must match bit for bit.
/// `NEOMACS_MODEL_REQUIRE_EVIDENCE=1` turns a missing input into a failure.
#[test]
fn parity_with_canvas_3d_on_real_asset() {
    let (Ok(obj), Ok(summary)) = (
        std::env::var("NEOMACS_MODEL_PARITY_OBJ"),
        std::env::var("NEOMACS_MODEL_PARITY_SUMMARY"),
    ) else {
        assert!(
            std::env::var_os("NEOMACS_MODEL_REQUIRE_EVIDENCE").is_none(),
            "parity evidence required but NEOMACS_MODEL_PARITY_* unset"
        );
        eprintln!("skipping: no parity asset");
        return;
    };
    let never = || false;
    let parsed = crate::model::read_obj_file(
        std::path::Path::new(&obj),
        None,
        remaining_limits(&[]),
        &never,
    )
    .unwrap();
    let summary = std::fs::read_to_string(summary).unwrap();
    let words: Vec<&str> = summary.split_whitespace().collect();
    assert_eq!(words.len(), 9, "summary fields");
    let numbers: Vec<usize> = words[..8].iter().map(|w| w.parse().unwrap()).collect();
    assert_eq!(parsed.vertices.len(), numbers[0]);
    assert_eq!(parsed.triangles.len(), numbers[1]);
    assert_eq!(
        parsed.triangles[0].map(|i| i as usize),
        [numbers[2], numbers[3], numbers[4]]
    );
    let last = parsed.triangles[parsed.triangles.len() - 1];
    assert_eq!(
        last.map(|i| i as usize),
        [numbers[5], numbers[6], numbers[7]]
    );
    let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
    for v in &parsed.vertices {
        for c in v {
            sha2::Digest::update(&mut hasher, c.to_le_bytes());
        }
    }
    for t in &parsed.triangles {
        for i in t {
            sha2::Digest::update(&mut hasher, i.to_le_bytes());
        }
    }
    let digest: String = sha2::Digest::finalize(hasher)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(
        digest, words[8],
        "every coordinate and triangle matches canvas-3d"
    );
    ModelScene::new(vec![parsed]).unwrap();
}
