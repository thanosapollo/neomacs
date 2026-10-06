//! OBJ geometry for the native model view.
//!
//! Semantics match the Gnosis scene validator (`gnosis-model--geometry`) and
//! the canvas-3d renderer it replaces, because stored Gnosis targets name
//! faces by index:
//!
//! - `#` starts a comment; only `v` and `f` lines matter (`vt`, `vn`, `o`,
//!   `g`, `usemtl`, `s`, `mtllib` are ignored).
//! - `v` takes the first three numbers; each must match Gnosis's strict
//!   number syntax and satisfy |n| <= 1e6.
//! - `f` takes the position index before the first `/` of each reference: a
//!   decimal integer, 1-based when positive, relative to the vertices defined
//!   so far when negative. A polygon `p0..pk` becomes the file-order fan
//!   `(p0, pi, pi+1)`; faces are numbered from 0 in that order.

use std::io::Read;
use std::path::Path;

/// Largest OBJ file the model view reads (Gnosis's limit).
pub const MAX_OBJ_FILE_BYTES: u64 = 100_000_000;

/// Largest absolute coordinate accepted (Gnosis's `gnosis-model--number`).
const MAX_COORDINATE: f64 = 1_000_000.0;

/// Read granularity; also how often a load checks for cancellation.
const READ_CHUNK: usize = 1 << 20;

/// One parsed mesh: original vertices and file-order fan triangles.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjMesh {
    pub vertices: Vec<[f64; 3]>,
    pub triangles: Vec<[u32; 3]>,
}

/// Remaining vertex/triangle allowance for one file. The caller passes what
/// is left of the model's budget, so a model's caps hold across its meshes
/// and a file fails as soon as it would exceed them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjLimits {
    pub max_vertices: usize,
    pub max_triangles: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ObjError {
    #[error("line {line}: OBJ vertex requires three coordinates")]
    ShortVertex { line: usize },
    #[error("line {line}: invalid OBJ number {text:?}")]
    BadNumber { line: usize, text: String },
    #[error("line {line}: OBJ number out of bounds")]
    NumberOutOfBounds { line: usize },
    #[error("line {line}: OBJ face requires three vertices")]
    ShortFace { line: usize },
    #[error("line {line}: invalid OBJ index {text:?}")]
    BadIndex { line: usize, text: String },
    #[error("line {line}: OBJ index outside vertices")]
    IndexOutOfRange { line: usize },
    #[error("line {line}: OBJ is not valid UTF-8")]
    NotUtf8 { line: usize },
    #[error("OBJ exceeds {max} vertices")]
    TooManyVertices { max: usize },
    #[error("OBJ exceeds {max} triangles")]
    TooManyTriangles { max: usize },
    #[error("OBJ contains no triangles")]
    NoTriangles,
    #[error("OBJ exceeds its byte allowance (at most {MAX_OBJ_FILE_BYTES} per file)")]
    TooLarge,
    #[error("OBJ is not a regular file")]
    NotRegular,
    #[error("OBJ digest mismatch")]
    DigestMismatch,
    #[error("model load cancelled")]
    Cancelled,
    #[error("{0}")]
    Io(String),
}

/// Gnosis's number syntax: `[-+]?(D+(.D*)?|.D+)([eE][-+]?D+)?`. Stricter than
/// Rust's `f64::from_str` (no `inf`, `nan` or bare exponent), so a file the
/// Gnosis validator rejects is rejected here too.
fn parse_number(text: &str, line: usize) -> Result<f64, ObjError> {
    let bytes = text.as_bytes();
    let mut i = 0;
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        i += 1;
    }
    let int_start = i;
    while bytes.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
    }
    let int_digits = i - int_start;
    let mut frac_digits = 0;
    if bytes.get(i) == Some(&b'.') {
        i += 1;
        let frac_start = i;
        while bytes.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        frac_digits = i - frac_start;
    }
    let mantissa_ok = int_digits > 0 || frac_digits > 0;
    if matches!(bytes.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(bytes.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let exp_start = i;
        while bytes.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == exp_start {
            return Err(bad_number(text, line));
        }
    }
    if !mantissa_ok || i != bytes.len() {
        return Err(bad_number(text, line));
    }
    let value: f64 = text.parse().map_err(|_| bad_number(text, line))?;
    if !value.is_finite() || value.abs() > MAX_COORDINATE {
        return Err(ObjError::NumberOutOfBounds { line });
    }
    Ok(value)
}

fn bad_number(text: &str, line: usize) -> ObjError {
    ObjError::BadNumber {
        line,
        text: text.to_owned(),
    }
}

/// Resolve one face reference (`7`, `7/1`, `-2//3`) against `count`
/// vertices defined so far.
fn parse_index(word: &str, count: usize, line: usize) -> Result<u32, ObjError> {
    let text = word.split('/').next().unwrap_or("");
    let digits = text.strip_prefix('-').unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ObjError::BadIndex {
            line,
            text: text.to_owned(),
        });
    }
    // Gnosis and canvas-3d read the value, so leading zeros of any length
    // are valid; past 18 significant digits it is out of range for any file.
    let significant = digits.trim_start_matches('0');
    if significant.len() > 18 {
        return Err(ObjError::IndexOutOfRange { line });
    }
    let magnitude: i64 = if significant.is_empty() {
        0
    } else {
        significant.parse().map_err(|_| ObjError::BadIndex {
            line,
            text: text.to_owned(),
        })?
    };
    let n = if text.starts_with('-') {
        -magnitude
    } else {
        magnitude
    };
    let count = count as i64;
    let resolved = if n > 0 { n - 1 } else { count + n };
    if n == 0 || resolved < 0 || resolved >= count {
        return Err(ObjError::IndexOutOfRange { line });
    }
    Ok(resolved as u32)
}

/// Parse OBJ `bytes` within `limits`.
pub fn parse_obj(bytes: &[u8], limits: ObjLimits) -> Result<ObjMesh, ObjError> {
    parse_obj_cancellable(bytes, limits, &|| false)
}

/// [`parse_obj`], polling `cancelled` at bounded intervals.
pub fn parse_obj_cancellable(
    bytes: &[u8],
    limits: ObjLimits,
    cancelled: &dyn Fn() -> bool,
) -> Result<ObjMesh, ObjError> {
    let mut parser = ObjParser::new(limits);
    parser.feed(bytes, cancelled)?;
    parser.finish(cancelled)
}

/// A word already known to be UTF-8 (its line was validated) as text.
fn utf8(word: &[u8], line: usize) -> Result<&str, ObjError> {
    std::str::from_utf8(word).map_err(|_| ObjError::NotUtf8 { line })
}

/// Bytes of one line validated (and searched for a comment) between two
/// cancellation polls.
const SCAN_WINDOW: usize = READ_CHUNK;

/// Lines, references or triangles handled between two cancellation polls.
const CHECK_EVERY: usize = 1 << 16;

/// Incremental OBJ parser: fed the file in chunks as it is read, it keeps
/// only the unfinished last line, enforces the caps as it goes and polls
/// the cancel flag at bounded intervals, also inside one long line.
pub struct ObjParser {
    limits: ObjLimits,
    vertices: Vec<[f64; 3]>,
    triangles: Vec<[u32; 3]>,
    polygon: Vec<u32>,
    /// The current, not yet terminated line.
    pending: Vec<u8>,
    /// Number of the next line to complete (1-based).
    line: usize,
    /// Work units since the last cancellation poll.
    work: usize,
}

impl ObjParser {
    pub fn new(limits: ObjLimits) -> Self {
        Self {
            limits,
            vertices: Vec::new(),
            triangles: Vec::new(),
            polygon: Vec::new(),
            pending: Vec::new(),
            line: 1,
            work: 0,
        }
    }

    /// Count `units` of work; poll `cancelled` once enough accumulated.
    fn tick(&mut self, units: usize, cancelled: &dyn Fn() -> bool) -> Result<(), ObjError> {
        self.work += units;
        if self.work >= CHECK_EVERY {
            self.work = 0;
            if cancelled() {
                return Err(ObjError::Cancelled);
            }
        }
        Ok(())
    }

    /// Parse the complete lines of `chunk`; keep its unterminated tail.
    pub fn feed(&mut self, chunk: &[u8], cancelled: &dyn Fn() -> bool) -> Result<(), ObjError> {
        let mut rest = chunk;
        while let Some(end) = rest.iter().position(|b| *b == b'\n') {
            if self.pending.is_empty() {
                self.parse_line(&rest[..end], cancelled)?;
            } else {
                self.pending.extend_from_slice(&rest[..end]);
                let line = std::mem::take(&mut self.pending);
                self.parse_line(&line, cancelled)?;
            }
            rest = &rest[end + 1..];
        }
        self.pending.extend_from_slice(rest);
        Ok(())
    }

    /// The last (possibly empty) line, then the mesh.
    pub fn finish(mut self, cancelled: &dyn Fn() -> bool) -> Result<ObjMesh, ObjError> {
        let line = std::mem::take(&mut self.pending);
        self.parse_line(&line, cancelled)?;
        if self.triangles.is_empty() {
            return Err(ObjError::NoTriangles);
        }
        Ok(ObjMesh {
            vertices: self.vertices,
            triangles: self.triangles,
        })
    }

    fn parse_line(&mut self, raw: &[u8], cancelled: &dyn Fn() -> bool) -> Result<(), ObjError> {
        let line = self.line;
        self.line += 1;
        self.tick(1, cancelled)?;
        let code_end = self.scan_line(raw, line, cancelled)?;
        let mut words = raw[..code_end].split(|b| matches!(b, b' ' | b'\t' | b'\r'));
        match self.next_word(&mut words, cancelled)? {
            Some(b"v") => {
                let mut point = [0.0; 3];
                for slot in &mut point {
                    let word = self
                        .next_word(&mut words, cancelled)?
                        .ok_or(ObjError::ShortVertex { line })?;
                    *slot = parse_number(utf8(word, line)?, line)?;
                }
                if self.vertices.len() >= self.limits.max_vertices {
                    return Err(ObjError::TooManyVertices {
                        max: self.limits.max_vertices,
                    });
                }
                self.vertices.push(point);
            }
            Some(b"f") => {
                self.polygon.clear();
                // A polygon of k references is k - 2 triangles: stop as soon
                // as it alone exceeds the remaining allowance.
                let max_refs = self
                    .limits
                    .max_triangles
                    .saturating_sub(self.triangles.len())
                    + 2;
                while let Some(word) = self.next_word(&mut words, cancelled)? {
                    if self.polygon.len() == max_refs {
                        return Err(ObjError::TooManyTriangles {
                            max: self.limits.max_triangles,
                        });
                    }
                    let index = parse_index(utf8(word, line)?, self.vertices.len(), line)?;
                    self.polygon.push(index);
                }
                if self.polygon.len() < 3 {
                    return Err(ObjError::ShortFace { line });
                }
                for i in 1..self.polygon.len() - 1 {
                    self.triangles
                        .push([self.polygon[0], self.polygon[i], self.polygon[i + 1]]);
                    self.tick(1, cancelled)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// The next non-empty word, polling `cancelled` across long runs of
    /// separators or words.
    fn next_word<'a>(
        &mut self,
        words: &mut impl Iterator<Item = &'a [u8]>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<&'a [u8]>, ObjError> {
        for word in words {
            self.tick(1, cancelled)?;
            if !word.is_empty() {
                return Ok(Some(word));
            }
        }
        Ok(None)
    }

    /// Validate `raw` as UTF-8 and find where its comment starts, one
    /// window at a time with a cancellation poll between windows.
    fn scan_line(
        &mut self,
        raw: &[u8],
        line: usize,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<usize, ObjError> {
        let mut code_end = None;
        let mut at = 0;
        while at < raw.len() {
            let end = (at + SCAN_WINDOW).min(raw.len());
            let valid = match std::str::from_utf8(&raw[at..end]) {
                Ok(text) => text,
                // A character split by the window edge: validate up to it
                // and start the next window there.
                Err(e) if e.error_len().is_none() && end < raw.len() && e.valid_up_to() > 0 => {
                    std::str::from_utf8(&raw[at..at + e.valid_up_to()]).expect("validated prefix")
                }
                Err(_) => return Err(ObjError::NotUtf8 { line }),
            };
            if code_end.is_none()
                && let Some(hash) = valid.find('#')
            {
                code_end = Some(at + hash);
            }
            at += valid.len();
            // A long line polls between windows.
            if at < raw.len() && cancelled() {
                return Err(ObjError::Cancelled);
            }
        }
        Ok(code_end.unwrap_or(raw.len()))
    }
}

/// Read and parse the OBJ file at `path`.
///
/// Only a regular file is read (others are [`ObjError::NotRegular`], found
/// without blocking). It is read once, in 1 MB steps, never beyond
/// [`MAX_OBJ_FILE_BYTES`], each step hashed and parsed incrementally
/// ([`ObjParser`]); `cancelled` is polled between steps and inside the
/// parse. When `sha256` is given, the exact bytes read are hashed and must
/// match before the mesh is returned, so the caller's verification and this
/// parse see the same content. (A file that is both malformed and of the
/// wrong digest reports the parse error found first.)
pub fn read_obj_file(
    path: &Path,
    sha256: Option<&[u8; 32]>,
    limits: ObjLimits,
    cancelled: &dyn Fn() -> bool,
) -> Result<ObjMesh, ObjError> {
    read_obj_file_within(path, sha256, limits, MAX_OBJ_FILE_BYTES, cancelled).map(|(mesh, _)| mesh)
}

/// [`read_obj_file`] reading at most `max_bytes` (itself capped at
/// [`MAX_OBJ_FILE_BYTES`]), so a caller can bound the input work of a whole
/// scene, not only of one file. Returns the mesh and the bytes read.
pub fn read_obj_file_within(
    path: &Path,
    sha256: Option<&[u8; 32]>,
    limits: ObjLimits,
    max_bytes: u64,
    cancelled: &dyn Fn() -> bool,
) -> Result<(ObjMesh, u64), ObjError> {
    let max_bytes = max_bytes.min(MAX_OBJ_FILE_BYTES);
    use sha2::Digest;
    use std::os::unix::fs::OpenOptionsExt;
    // O_NONBLOCK so a FIFO (or a symlink to one) cannot block the open; the
    // type and size then come from the opened descriptor, so the file
    // checked is the file read.
    let file = std::fs::File::options()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| ObjError::Io(e.to_string()))?;
    let meta = file.metadata().map_err(|e| ObjError::Io(e.to_string()))?;
    if !meta.is_file() {
        return Err(ObjError::NotRegular);
    }
    if meta.len() > max_bytes {
        return Err(ObjError::TooLarge);
    }
    let mut reader = file.take(max_bytes + 1);
    // One pass: each 1 MB step is read, hashed and parsed before the next,
    // so cancellation, the caps and the size bound all act as it goes.
    let mut hasher = sha256.map(|_| sha2::Sha256::new());
    let mut parser = ObjParser::new(limits);
    let mut read = 0u64;
    let mut chunk = vec![0u8; READ_CHUNK];
    super::loader::probe("parse", path);
    loop {
        if cancelled() {
            return Err(ObjError::Cancelled);
        }
        let n = reader
            .read(&mut chunk)
            .map_err(|e| ObjError::Io(e.to_string()))?;
        if n == 0 {
            break;
        }
        read += n as u64;
        if read > max_bytes {
            return Err(ObjError::TooLarge);
        }
        if let Some(hasher) = hasher.as_mut() {
            hasher.update(&chunk[..n]);
        }
        parser.feed(&chunk[..n], cancelled)?;
    }
    if let (Some(expected), Some(hasher)) = (sha256, hasher)
        && hasher.finalize().as_slice() != expected.as_slice()
    {
        return Err(ObjError::DigestMismatch);
    }
    if cancelled() {
        return Err(ObjError::Cancelled);
    }
    parser.finish(cancelled).map(|mesh| (mesh, read))
}

#[cfg(test)]
#[path = "obj/tests.rs"]
mod tests;
