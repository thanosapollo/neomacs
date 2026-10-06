use super::*;
use crate::model::scene::{ModelScene, remaining_limits};
use std::io::Write;
use std::time::{Duration, Instant};

/// A fresh scratch directory per test (this crate has no temp-dir dependency).
fn scratch_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "neomacs-model-loader-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn obj_file(dir: &std::path::Path, name: &str, body: &[u8]) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::File::create(&path)
        .unwrap()
        .write_all(body)
        .unwrap();
    path
}

fn spec(path: std::path::PathBuf) -> ModelMeshSpec {
    ModelMeshSpec { path, sha256: None }
}

fn wait_for(loader: &mut ModelLoader, count: usize) -> Vec<LoadResult> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut out = Vec::new();
    while out.len() < count && Instant::now() < deadline {
        out.extend(loader.drain());
        std::thread::sleep(Duration::from_millis(5));
    }
    out
}

const TRIANGLE: &[u8] = b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";

#[test]
fn a_load_completes_off_thread_and_reports_its_identity() {
    let dir = scratch_dir("complete");
    let a = obj_file(dir.as_path(), "a.obj", TRIANGLE);
    let mut loader = ModelLoader::spawn().unwrap();
    loader
        .submit(LoadRequest {
            id: 7,
            token: 3,
            meshes: vec![spec(a.clone()), spec(a)],
            cancel: Arc::new(AtomicBool::new(false)),
        })
        .unwrap();
    assert!(loader.has_pending());
    let results = wait_for(&mut loader, 1);
    assert_eq!(results.len(), 1);
    assert_eq!((results[0].id, results[0].token), (7, 3));
    let scene = results[0].outcome.as_ref().unwrap();
    assert_eq!(scene.triangle_counts(), vec![1, 1]);
    assert!(!loader.has_pending());
}

#[test]
fn digest_mismatch_and_missing_files_fail_with_the_path() {
    let dir = scratch_dir("digest");
    let a = obj_file(dir.as_path(), "a.obj", TRIANGLE);
    let cancel = AtomicBool::new(false);
    let wrong = ModelMeshSpec {
        path: a.clone(),
        sha256: Some([0u8; 32]),
    };
    let err = load_scene(&[wrong], &cancel).unwrap_err();
    assert!(err.contains("a.obj") && err.contains("digest"), "{err}");
    use sha2::Digest;
    let right = ModelMeshSpec {
        path: a,
        sha256: Some(sha2::Sha256::digest(TRIANGLE).into()),
    };
    assert!(load_scene(&[right], &cancel).is_ok());
    let missing = spec(dir.as_path().join("missing.obj"));
    assert!(
        load_scene(&[missing], &cancel)
            .unwrap_err()
            .contains("missing.obj")
    );
}

/// Deferred Sol slice SHOULD: 255 files of mostly comments must not each
/// get the full per-file allowance; the scene shares one input budget.
#[test]
fn input_bytes_are_bounded_across_the_whole_scene() {
    let dir = scratch_dir("aggregate");
    // Valid files just over a quarter of the aggregate allowance each: one
    // triangle, then a sparse run of NUL padding (an ignored record), so the
    // test writes almost nothing to disk. Four of them cannot all be read.
    let quarter = MAX_MODEL_INPUT_BYTES / 4 + 1;
    let mut paths = Vec::new();
    for i in 0..4 {
        let path = obj_file(dir.as_path(), &format!("big{i}.obj"), TRIANGLE);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(quarter)
            .unwrap();
        paths.push(spec(path));
    }
    let cancel = AtomicBool::new(false);
    let err = load_scene(&paths, &cancel).unwrap_err();
    assert!(
        err.contains("big3.obj") && err.contains("allowance"),
        "{err}"
    );
}

#[test]
fn cancel_stops_a_queued_load_and_its_result_is_an_error() {
    let dir = scratch_dir("cancel");
    let a = obj_file(dir.as_path(), "a.obj", TRIANGLE);
    let mut loader = ModelLoader::spawn().unwrap();
    let cancel = Arc::new(AtomicBool::new(true));
    loader
        .submit(LoadRequest {
            id: 1,
            token: 1,
            meshes: vec![spec(a)],
            cancel,
        })
        .unwrap();
    loader.cancel(1);
    let results = wait_for(&mut loader, 1);
    assert!(results[0].outcome.is_err());
}

#[test]
fn more_than_the_queue_limit_is_busy_not_blocking() {
    // Submit far more than the queue holds, faster than the thread can
    // answer: the overflow is refused at once ("busy"), never waited for,
    // and every accepted request is still answered.
    let mut loader = ModelLoader::spawn().unwrap();
    let start = Instant::now();
    let mut busy = 0;
    for i in 0..64 {
        let request = LoadRequest {
            id: i,
            token: 1,
            meshes: vec![spec("/nonexistent/x.obj".into())],
            cancel: Arc::new(AtomicBool::new(false)),
        };
        if loader.submit(request).is_err() {
            busy += 1;
        }
    }
    assert!(start.elapsed() < Duration::from_secs(2), "submit blocked");
    let answered = wait_for(&mut loader, 64 - busy);
    assert_eq!(answered.len(), 64 - busy);
}

#[test]
fn drop_cancels_and_joins_promptly() {
    let dir = scratch_dir("drop");
    let a = obj_file(dir.as_path(), "a.obj", TRIANGLE);
    let mut loader = ModelLoader::spawn().unwrap();
    for i in 0..3 {
        let _ = loader.submit(LoadRequest {
            id: i,
            token: 1,
            meshes: vec![spec(a.clone())],
            cancel: Arc::new(AtomicBool::new(false)),
        });
    }
    let start = Instant::now();
    drop(loader);
    assert!(start.elapsed() < Duration::from_secs(2));
}

// ----------------------------------------------------------------------
// Review MUST 2: dropping a loader that is genuinely mid-parse or
// mid-scene returns within one cancellation step.

/// 2,000,000 one-triangle face lines (about 16 MB).
fn heavy_body() -> Vec<u8> {
    let mut body = b"v 0 0 0\nv 1 0 0\nv 0 1 0\n".to_vec();
    for _ in 0..crate::model::scene::MAX_MODEL_TRIANGLES {
        body.extend_from_slice(b"f 1 2 3\n");
    }
    body
}

fn drop_while_in(phase: &'static str) {
    let dir = scratch_dir(&format!("mid-{phase}"));
    let body = heavy_body();
    let path = obj_file(dir.as_path(), "heavy.obj", &body);
    // How long the phase alone takes in this build, uncancelled.
    let start = Instant::now();
    let mesh = crate::model::obj::parse_obj(&body, remaining_limits(&[])).unwrap();
    let parse_time = start.elapsed();
    let start = Instant::now();
    ModelScene::new(vec![mesh]).unwrap();
    let scene_time = start.elapsed();
    let phase_time = if phase == "parse" {
        parse_time
    } else {
        scene_time
    };
    drop(body);

    let (entered, inbox) = mpsc::channel();
    let wanted = path.clone();
    *PROBE.lock().unwrap() = Some(Box::new(move |at, file: &std::path::Path| {
        if at == phase && file == wanted {
            let _ = entered.send(());
        }
    }));
    let mut loader = ModelLoader::spawn().unwrap();
    loader
        .submit(LoadRequest {
            id: 1,
            token: 1,
            meshes: vec![spec(path)],
            cancel: Arc::new(AtomicBool::new(false)),
        })
        .unwrap();
    inbox
        .recv_timeout(Duration::from_secs(60))
        .unwrap_or_else(|_| panic!("the load never entered its {phase} phase"));
    let start = Instant::now();
    drop(loader);
    let stopped = start.elapsed();
    *PROBE.lock().unwrap() = None;
    assert!(
        stopped * 4 < phase_time,
        "dropping the loader mid-{phase} waited {stopped:?}; the {phase} alone takes {phase_time:?}"
    );
}

#[test]
fn dropping_a_loader_mid_parse_returns_within_one_step() {
    drop_while_in("parse");
}

#[test]
fn dropping_a_loader_mid_scene_returns_within_one_step() {
    drop_while_in("scene");
}

// ----------------------------------------------------------------------
// Review MUST 1: completed results awaiting admission are bounded.

/// Results the render thread has not drained count against the loader's
/// capacity, so a stalled render thread cannot accumulate scenes.
#[test]
fn completed_results_awaiting_admission_are_bounded() {
    let dir = scratch_dir("bounded-results");
    let a = obj_file(dir.as_path(), "a.obj", TRIANGLE);
    let mut loader = ModelLoader::spawn().unwrap();
    let request = |id| LoadRequest {
        id,
        token: 1,
        meshes: vec![spec(a.clone())],
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let capacity = MAX_QUEUED_LOADS + 1;
    for id in 0..capacity as u32 {
        let deadline = Instant::now() + Duration::from_secs(5);
        while loader.submit(request(id)).is_err() {
            assert!(Instant::now() < deadline, "request {id} never accepted");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    // Every accepted load completes; none is drained.
    std::thread::sleep(Duration::from_millis(300));
    for _ in 0..3 {
        assert_eq!(
            loader.submit(request(99)).err().as_deref(),
            Some("model loader busy"),
            "{capacity} undrained results already wait for admission"
        );
    }
    assert_eq!(wait_for(&mut loader, capacity).len(), capacity);
    assert!(
        loader.submit(request(100)).is_ok(),
        "draining frees capacity"
    );
}
