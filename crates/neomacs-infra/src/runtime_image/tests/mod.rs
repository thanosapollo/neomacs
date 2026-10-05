//! Unit tests for the pure half of provisioning.
//!
//! A real bootstrap needs the editor binary and minutes of Lisp loading, and
//! belongs to the daemon suite's integration run; what is tested here is the
//! decision logic that run depends on — when an image counts as fresh, and
//! how the role copy tracks the editor — because a wrong answer there is
//! silent (a stale image loads fine and tests the wrong code).

use super::*;
use std::time::{Duration, SystemTime};

fn write_file(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().expect("test paths have parents")).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn set_mtime(path: &Path, when: SystemTime) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(when)
        .unwrap();
}

fn write_at(path: &Path, contents: &str, when: SystemTime) {
    write_file(path, contents);
    set_mtime(path, when);
}

#[test]
fn a_missing_or_empty_image_is_never_fresh() {
    let root = tempfile::tempdir().unwrap();
    let editor = root.path().join("neomacs");
    write_file(&editor, "editor");
    let image = root.path().join("bootstrap-neomacs.pdump");

    assert_eq!(freshness(&image, &editor), Freshness::Missing);

    write_file(&image, "");
    assert_eq!(freshness(&image, &editor), Freshness::Missing);
}

#[test]
fn freshness_is_decided_by_the_editor_clock() {
    let root = tempfile::tempdir().unwrap();
    let editor = root.path().join("neomacs");
    let image = root.path().join("bootstrap-neomacs.pdump");
    let now = SystemTime::now();
    write_at(&editor, "editor", now);

    write_at(&image, "image", now + Duration::from_secs(5));
    assert_eq!(freshness(&image, &editor), Freshness::Fresh);

    // A rebuild moves the editor past the image: the dump embeds the code of
    // the binary that produced it, so this pair must not be reused.
    write_at(&image, "image", now - Duration::from_secs(5));
    assert_eq!(freshness(&image, &editor), Freshness::Stale);
}

#[test]
fn the_role_copy_tracks_the_editor_bytes_and_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let editor = root.path().join("neomacs");
    let role = root.path().join("neomacs-temacs");
    write_file(&editor, "first build");
    std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755)).unwrap();

    ensure_role_binary(&editor, &role).unwrap();
    assert_eq!(std::fs::read_to_string(&role).unwrap(), "first build");
    assert_eq!(
        std::fs::metadata(&role).unwrap().permissions().mode() & 0o777,
        0o755
    );

    // The editor is rebuilt: the role copy must follow, or the bootstrap
    // would dump an image of the previous binary.
    write_at(
        &editor,
        "second build",
        SystemTime::now() + Duration::from_secs(5),
    );
    ensure_role_binary(&editor, &role).unwrap();
    assert_eq!(std::fs::read_to_string(&role).unwrap(), "second build");
}

fn plan_with_fresh_pair(root: &Path) -> BootstrapImagePlan {
    let editor = root.join("neomacs");
    let loader = root.join("bootstrap-neomacs-fingerprint.pdump");
    let now = SystemTime::now();
    write_at(&editor, "editor", now);
    write_at(
        &root.join("bootstrap-neomacs.pdump"),
        "image",
        now + Duration::from_secs(5),
    );
    write_at(&loader, "image", now + Duration::from_secs(5));
    BootstrapImagePlan {
        editor,
        runtime_root: root.to_path_buf(),
        role_binary_name: "neomacs-temacs".to_string(),
        canonical_image_name: "bootstrap-neomacs.pdump".to_string(),
        loader_image: loader,
        terminal_layer: None,
    }
}

#[test]
fn a_fresh_pair_is_reused_without_running_anything() {
    let root = tempfile::tempdir().unwrap();
    let plan = plan_with_fresh_pair(root.path());
    // No lisp/loadup.el exists here; on the reuse path that must not matter.
    let image = provision_bootstrap_image(&plan).unwrap();
    assert_eq!(image.outcome, BootstrapImageOutcome::Reused);
    assert_eq!(image.canonical, root.path().join("bootstrap-neomacs.pdump"));
}

#[test]
fn a_missing_loader_name_forces_provisioning() {
    let root = tempfile::tempdir().unwrap();
    let plan = plan_with_fresh_pair(root.path());
    std::fs::remove_file(&plan.loader_image).unwrap();
    // With the fingerprinted twin gone the daemon would not find the image at
    // all, so this must not take the reuse path.
    assert!(reuse_if_fresh(&plan, &root.path().join("bootstrap-neomacs.pdump")).is_none());
}

#[test]
fn gui_terminal_bytecode_requires_matching_editor_source_and_image() {
    let root = tempfile::tempdir().unwrap();
    let mut plan = plan_with_fresh_pair(root.path());
    let source = root.path().join("neo-win.el");
    let bytecode = source.with_extension("elc");
    let image = root.path().join("bootstrap-neomacs.pdump");
    let now = SystemTime::now();
    write_at(&source, "source", now);
    plan.terminal_layer = Some(BootstrapTerminalLayer {
        source: source.clone(),
        bootstrap_role_binary_name: "bootstrap-neomacs".into(),
    });
    assert!(
        !terminal_layer_is_fresh(&plan, &image),
        "missing leaf must not reuse image-only preparation"
    );
    write_at(&bytecode, "bytecode", now + Duration::from_secs(10));
    assert!(terminal_layer_is_fresh(&plan, &image));
    for input in [&plan.editor, &source, &image] {
        let previous = fs::metadata(input).unwrap().modified().unwrap();
        set_mtime(input, now + Duration::from_secs(20));
        assert!(
            !terminal_layer_is_fresh(&plan, &image),
            "changed input: {input:?}"
        );
        set_mtime(input, previous);
    }
    write_at(&bytecode, "", now + Duration::from_secs(10));
    assert!(
        !terminal_layer_is_fresh(&plan, &image),
        "empty output is not prepared"
    );
}

#[cfg(unix)]
#[test]
fn failed_terminal_compiler_cannot_leave_a_reusable_partial_leaf() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let mut plan = plan_with_fresh_pair(root.path());
    let source = root.path().join("neo-win.el");
    let bytecode = source.with_extension("elc");
    write_file(&source, "source");
    // An owned fake compiler exposes the partial-output failure boundary;
    // it does not stand in for the separate matching-native preparation gate.
    write_file(
        &plan.editor,
        &format!(
            "#!/bin/sh\nprintf partial > '{}'\nexit 42\n",
            bytecode.display()
        ),
    );
    fs::set_permissions(&plan.editor, fs::Permissions::from_mode(0o755)).unwrap();
    plan.terminal_layer = Some(BootstrapTerminalLayer {
        source,
        bootstrap_role_binary_name: "bootstrap-neomacs".into(),
    });
    let result = prepare_terminal_layer(
        &plan,
        &root.path().join("bootstrap-neomacs.pdump"),
        root.path(),
    );
    assert!(result.is_err());
    assert!(
        !bytecode.exists(),
        "failed compilation left a fresh-looking partial leaf"
    );
}

#[test]
fn a_runtime_root_without_loadup_is_rejected_by_name() {
    let root = tempfile::tempdir().unwrap();
    let plan = plan_with_fresh_pair(root.path());
    // Stale images force the bootstrap path, which needs a bootable tree.
    std::fs::remove_file(root.path().join("bootstrap-neomacs.pdump")).unwrap();
    std::fs::remove_file(&plan.loader_image).unwrap();
    let error = provision_bootstrap_image(&plan).unwrap_err();
    assert!(
        error.contains("loadup.el"),
        "error should name the missing prerequisite: {error}"
    );
}
