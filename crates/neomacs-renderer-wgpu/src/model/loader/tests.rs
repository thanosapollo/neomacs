use super::*;
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
