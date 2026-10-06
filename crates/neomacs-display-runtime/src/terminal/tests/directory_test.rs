use super::*;

#[test]
fn local_cwd_uri_admits_only_local_authority() {
    for uri in [
        "file:///home/work",
        "file://localhost/home/work",
        "file://machine/home/work",
        "file://MACHINE/home/work",
    ] {
        assert_eq!(
            local_directory(uri.as_bytes(), Some("machine")).as_deref(),
            Some("/home/work"),
            "{uri}"
        );
    }
    assert_eq!(local_directory(b"file://machine/home/work", None), None);
    assert_eq!(
        local_directory(b"file://localhost/home/work", None).as_deref(),
        Some("/home/work")
    );
}

#[test]
fn local_cwd_uri_decodes_unicode_and_escapes_once() {
    for (uri, expected) in [
        ("file:///home/%CE%B1%20b", "/home/α b"),
        ("file:///home/α", "/home/α"),
        ("file:///home/%2520", "/home/%20"),
        ("file:///home/%23%3F", "/home/#?"),
        ("file:///", "/"),
    ] {
        assert_eq!(
            local_directory(uri.as_bytes(), Some("machine")).as_deref(),
            Some(expected),
            "{uri}"
        );
    }
}

#[test]
fn local_cwd_uri_refuses_lossy_or_remote_admission() {
    for uri in [
        "file://foreign/home/work",
        "ssh://machine/home/work",
        "file:/home/work",
        "file:relative",
        "file://machine",
        "file://machine:22/home/work",
        "file://user@machine/home/work",
        "file://machine/home/work?q",
        "file://machine/home/work#f",
        "file:///home/%",
        "file:///home/%2",
        "file:///home/%GG",
        "file:///home/%00",
        "file:///home/%FF",
        "file:///home/%C0%AF",
        "file:///home/%0A",
        "file:////foreign/home",
        "file:///%2Fforeign/home",
        "file:///%3Assh:host:/tmp",
        "file:///home\\work",
        "file:///home/with space",
        "file://machine./home/work",
        "file://%6dachine/home/work",
    ] {
        assert_eq!(
            local_directory(uri.as_bytes(), Some("machine")),
            None,
            "{uri}"
        );
    }
    assert_eq!(local_directory(b"file:///home/\0work", None), None);
    assert_eq!(local_directory(b"file:///home/\xff", None), None);
    assert_eq!(
        local_directory(
            format!("file:///{}", "a".repeat(16 * 1024)).as_bytes(),
            None
        ),
        None
    );
}

#[test]
fn local_cwd_uri_refuses_exact_decoded_dot_components() {
    for path in [
        "/tmp/../ssh:host:/work",
        "/./sudo::/work",
        "/tmp/%2e%2E/:/work",
        "/%2E/ssh:host:/work",
        "/tmp/.%2e/sudo::/work",
        "/tmp/%2e./ssh:host:/work",
        "/tmp%2F..%2Fssh:host:/work",
        "/tmp/.",
        "/tmp/..",
        "/tmp/%2e",
        "/tmp/%2E%2e/",
    ] {
        assert_eq!(
            local_directory(format!("file://{path}").as_bytes(), None),
            None,
            "{path}"
        );
    }
    for (path, expected) in [
        ("/home/%CE%B1/.hidden", "/home/α/.hidden"),
        ("/home/.../a..b", "/home/.../a..b"),
        ("/home/%252e%252e", "/home/%2e%2e"),
    ] {
        assert_eq!(
            local_directory(format!("file://{path}").as_bytes(), None).as_deref(),
            Some(expected)
        );
    }
}

#[test]
fn cwd_updates_coalesce_changes_and_suppress_repetition() {
    let mut updates = DirectoryUpdates::default();
    updates.observe(Some("/a".into()));
    updates.observe(Some("/b".into()));
    updates.observe(Some("/b".into()));
    assert_eq!(updates.take().as_deref(), Some("/b"));
    assert_eq!(updates.take(), None);
    updates.observe(Some("/b".into()));
    assert_eq!(updates.take(), None);
    updates.observe(Some("/a".into()));
    assert_eq!(updates.take().as_deref(), Some("/a"));
}

#[test]
fn invalid_cwd_cancels_undelivered_metadata_and_allows_recovery() {
    let mut updates = DirectoryUpdates::default();
    updates.observe(Some("/a".into()));
    updates.observe(None);
    assert_eq!(updates.take(), None);
    updates.observe(Some("/a".into()));
    assert_eq!(updates.take().as_deref(), Some("/a"));
}
