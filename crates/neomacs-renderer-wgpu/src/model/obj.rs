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
    #[error("OBJ exceeds {MAX_OBJ_FILE_BYTES} bytes")]
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
    let mut vertices: Vec<[f64; 3]> = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    let mut polygon: Vec<u32> = Vec::new();
    for (index, raw) in bytes.split(|b| *b == b'\n').enumerate() {
        let line = index + 1;
        let text = std::str::from_utf8(raw).map_err(|_| ObjError::NotUtf8 { line })?;
        let code = text.split('#').next().unwrap_or("");
        let mut words = code.split([' ', '\t', '\r']).filter(|w| !w.is_empty());
        match words.next() {
            Some("v") => {
                let mut point = [0.0; 3];
                for slot in &mut point {
                    let word = words.next().ok_or(ObjError::ShortVertex { line })?;
                    *slot = parse_number(word, line)?;
                }
                if vertices.len() >= limits.max_vertices {
                    return Err(ObjError::TooManyVertices {
                        max: limits.max_vertices,
                    });
                }
                vertices.push(point);
            }
            Some("f") => {
                polygon.clear();
                // A polygon of k references is k - 2 triangles: stop as soon
                // as it alone exceeds the remaining allowance.
                let max_refs = limits.max_triangles.saturating_sub(triangles.len()) + 2;
                for word in words {
                    if polygon.len() == max_refs {
                        return Err(ObjError::TooManyTriangles {
                            max: limits.max_triangles,
                        });
                    }
                    polygon.push(parse_index(word, vertices.len(), line)?);
                }
                if polygon.len() < 3 {
                    return Err(ObjError::ShortFace { line });
                }
                for i in 1..polygon.len() - 1 {
                    triangles.push([polygon[0], polygon[i], polygon[i + 1]]);
                }
            }
            _ => {}
        }
    }
    if triangles.is_empty() {
        return Err(ObjError::NoTriangles);
    }
    Ok(ObjMesh {
        vertices,
        triangles,
    })
}

/// Read and parse the OBJ file at `path`.
///
/// Only a regular file is read (others are [`ObjError::NotRegular`], found
/// without blocking). It is read once, in 1 MB steps, never beyond
/// [`MAX_OBJ_FILE_BYTES`]; `cancelled` is polled between steps. When
/// `sha256` is given, the exact bytes read are hashed and must match, so the
/// caller's verification and this parse see the same content.
pub fn read_obj_file(
    path: &Path,
    sha256: Option<&[u8; 32]>,
    limits: ObjLimits,
    cancelled: &dyn Fn() -> bool,
) -> Result<ObjMesh, ObjError> {
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
    if meta.len() > MAX_OBJ_FILE_BYTES {
        return Err(ObjError::TooLarge);
    }
    let mut reader = file.take(MAX_OBJ_FILE_BYTES + 1);
    let mut bytes = Vec::new();
    let mut chunk = vec![0u8; READ_CHUNK];
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
        bytes.extend_from_slice(&chunk[..n]);
        if bytes.len() as u64 > MAX_OBJ_FILE_BYTES {
            return Err(ObjError::TooLarge);
        }
    }
    if let Some(expected) = sha256
        && sha2::Sha256::digest(&bytes).as_slice() != expected.as_slice()
    {
        return Err(ObjError::DigestMismatch);
    }
    if cancelled() {
        return Err(ObjError::Cancelled);
    }
    parse_obj(&bytes, limits)
}

#[cfg(test)]
#[path = "obj/tests.rs"]
mod tests;
