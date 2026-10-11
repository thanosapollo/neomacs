use super::*;

fn strings(entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| {
            let string = entry.as_lisp_string().expect("load-path string");
            crate::emacs_core::emacs_char::to_utf8_lossy(string.as_bytes())
        })
        .collect()
}

fn name(path: &Path) -> String {
    crate::emacs_core::fileio::host_path_to_lisp_file_name_string(path)
}

#[test]
fn cli_load_path_leaves_subdirectory_expansion_to_startup() {
    let temp = tempfile::tempdir().unwrap();
    let lisp = temp.path().join("lisp");
    for sub in BOOTSTRAP_LOAD_PATH_SUBDIRS {
        std::fs::create_dir_all(lisp.join(sub)).unwrap();
    }
    std::fs::write(lisp.join("subdirs.el"), ";; startup expansion metadata\n").unwrap();
    assert_eq!(
        strings(&cli_load_path_entries_from_os(&lisp, None, &[])),
        vec![name(&lisp)],
        "normal-top-level must receive roots, not pre-expanded subdirs"
    );
    assert_eq!(
        strings(&runtime_load_path_entries_from_os(&lisp, None, &[])),
        strings(&bootstrap_load_path_entries(&lisp)),
        "core evaluators must keep finding libraries without normal-top-level"
    );
}

#[test]
fn cli_load_path_keeps_expanded_search_without_generated_subdirs_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let lisp = temp.path().join("lisp");
    std::fs::create_dir_all(lisp.join("emacs-lisp")).unwrap();
    assert!(!lisp.join("subdirs.el").exists());
    assert_eq!(
        strings(&cli_load_path_entries_from_os(&lisp, None, &[])),
        vec![name(&lisp), name(&lisp.join("emacs-lisp"))],
        "a clean source-only CLI must still find unpreloaded libraries"
    );
}

#[test]
fn cli_load_path_preserves_site_order_empty_splices_and_explicit_duplicates() {
    let temp = tempfile::tempdir().unwrap();
    let lisp = temp.path().join("lisp");
    std::fs::create_dir_all(lisp.join("emacs-lisp")).unwrap();
    std::fs::write(lisp.join("subdirs.el"), ";; startup expansion metadata\n").unwrap();
    let site = temp.path().join("site-lisp");
    let custom = temp.path().join("custom");
    let env = std::env::join_paths([
        custom.as_path(),
        Path::new(""),
        custom.as_path(),
        Path::new(""),
    ])
    .unwrap();
    assert_eq!(
        strings(&cli_load_path_entries_from_os(
            &lisp,
            Some(env),
            std::slice::from_ref(&site)
        )),
        vec![
            name(&custom),
            name(&site),
            name(&lisp),
            name(&custom),
            name(&site),
            name(&lisp)
        ],
        "do not globally deduplicate GNU's EMACSLOADPATH splice semantics"
    );
    let env = std::env::join_paths([custom.as_path()]).unwrap();
    assert_eq!(
        strings(&cli_load_path_entries_from_os(
            &lisp,
            Some(env),
            std::slice::from_ref(&site)
        )),
        vec![name(&custom), name(&site), name(&lisp)]
    );
}
