//! Product admission of native OSC 7 metadata; no terminal byte parser.

/// Admit only local, absolute UTF-8 paths. Keep raw URI checks ahead of the
/// standard URL parser: that parser normalizes authority and malformed input.
/// No DNS, filesystem, process-global cwd or shell probing is performed.
pub(super) fn local_directory(uri: &[u8], local_host: Option<&str>) -> Option<String> {
    if uri.len() > 16 * 1024 {
        return None;
    }
    let raw = std::str::from_utf8(uri).ok()?;
    if raw.bytes().any(|byte| byte <= 0x20 || byte == 0x7f) || raw.contains(['\\', '?', '#']) {
        return None;
    }
    let rest = raw.strip_prefix("file://")?;
    let slash = rest.find('/')?;
    let (authority, path) = rest.split_at(slash);
    if !authority.is_ascii()
        || !(authority.is_empty()
            || authority.eq_ignore_ascii_case("localhost")
            || local_host.is_some_and(|host| authority.eq_ignore_ascii_case(host)))
        || authority.contains(['@', ':', '%', '[', ']'])
    {
        return None;
    }
    let url = url::Url::parse(raw).ok()?;
    if url.scheme() != "file"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    // percent_decode_str deliberately tolerates malformed escapes, so refuse
    // them before decoding. Decode exactly once; %2520 denotes literal %20.
    let bytes = path.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if !bytes.get(index + 1).is_some_and(u8::is_ascii_hexdigit)
                || !bytes.get(index + 2).is_some_and(u8::is_ascii_hexdigit)
            {
                return None;
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    let directory = percent_encoding::percent_decode_str(path)
        .decode_utf8()
        .ok()?;
    if !directory.starts_with('/')
        || directory.starts_with("//")
        || directory.starts_with("/:")
        || directory.contains('\\')
        // Native relative file expansion removes these components and can
        // expose a leading Emacs magic/remote name. Refuse, never normalize.
        || directory.split('/').any(|part| matches!(part, "." | ".."))
        || directory.chars().any(char::is_control)
    {
        return None;
    }
    Some(directory.into_owned())
}

/// One bounded semantic slot per terminal, not one event per OSC or PTY read.
/// Invalid later metadata cancels an undelivered path rather than replaying it.
#[derive(Default)]
pub(super) struct DirectoryUpdates {
    last: Option<String>,
    pending: Option<String>,
}

impl DirectoryUpdates {
    pub(super) fn observe(&mut self, directory: Option<String>) {
        if self.last != directory {
            self.pending.clone_from(&directory);
            self.last = directory;
        }
    }

    pub(super) fn take(&mut self) -> Option<String> {
        self.pending.take()
    }
}

#[cfg(test)]
#[path = "tests/directory_test.rs"]
mod tests;
