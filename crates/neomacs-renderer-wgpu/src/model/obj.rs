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

/// Longest prefix of an offending word an error quotes: one word can be
/// megabytes long.
const QUOTED_BYTES: usize = 32;

/// The first bytes of a word, kept for an error message.
#[derive(Default)]
struct Quote {
    bytes: Vec<u8>,
    truncated: bool,
}

impl Quote {
    fn clear(&mut self) {
        self.bytes.clear();
        self.truncated = false;
    }

    fn push(&mut self, byte: u8) {
        // A few bytes past the limit, so the text can end on a whole
        // character.
        if self.bytes.len() < QUOTED_BYTES + 3 {
            self.bytes.push(byte);
        } else {
            self.truncated = true;
        }
    }

    /// The word, or its first [`QUOTED_BYTES`] at most and `…`.
    fn text(&self) -> String {
        let cut = self.truncated || self.bytes.len() > QUOTED_BYTES;
        let mut end = self.bytes.len();
        if cut {
            end = QUOTED_BYTES;
            while end > 0 && std::str::from_utf8(&self.bytes[..end]).is_err() {
                end -= 1;
            }
        }
        let mut text = String::from_utf8_lossy(&self.bytes[..end]).into_owned();
        if cut {
            text.push('…');
        }
        text
    }
}

/// Significant digits a number keeps. A decimal halfway point between two
/// `f64` has at most 768 significant digits (`(2^54 - 3) * 2^-1075` has
/// exactly that many), so the first 800 digits and whether any later one
/// is nonzero round exactly as the whole text does.
const MAX_SIGNIFICANT: usize = 800;

/// An exponent this large in magnitude over- or underflows whatever the
/// (at most [`MAX_OBJ_FILE_BYTES`]) digits of its mantissa.
const MAX_EXPONENT: i64 = 1_000_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum NumberPart {
    #[default]
    Start,
    Int,
    Frac,
    ExpStart,
    ExpSign,
    Exp,
    Bad,
}

/// One number word, checked and reduced as its bytes arrive, in bounded
/// state whatever its length. Gnosis's syntax
/// `[-+]?(D+(.D*)?|.D+)([eE][-+]?D+)?` is stricter than `f64::from_str`
/// (no `inf`, `nan` or bare exponent), so a file the Gnosis validator
/// rejects is rejected here too; the value is the correctly rounded one
/// of the whole text.
#[derive(Default)]
struct NumberWord {
    quote: Quote,
    part: NumberPart,
    negative: bool,
    mantissa_digits: u64,
    frac_digits: u64,
    /// ASCII digits from the first nonzero one, at most [`MAX_SIGNIFICANT`].
    significant: Vec<u8>,
    significant_count: u64,
    /// A nonzero digit past [`MAX_SIGNIFICANT`] was dropped.
    sticky: bool,
    exp_negative: bool,
    exp: i64,
}

impl NumberWord {
    fn clear(&mut self) {
        self.quote.clear();
        self.part = NumberPart::Start;
        self.negative = false;
        self.mantissa_digits = 0;
        self.frac_digits = 0;
        self.significant.clear();
        self.significant_count = 0;
        self.sticky = false;
        self.exp_negative = false;
        self.exp = 0;
    }

    fn mantissa_digit(&mut self, digit: u8) {
        self.mantissa_digits += 1;
        if self.significant_count == 0 && digit == b'0' {
            return;
        }
        self.significant_count += 1;
        if self.significant.len() < MAX_SIGNIFICANT {
            self.significant.push(digit);
        } else if digit != b'0' {
            self.sticky = true;
        }
    }

    fn push(&mut self, byte: u8) {
        use NumberPart::*;
        self.quote.push(byte);
        self.part = match (self.part, byte) {
            (Start, b'+' | b'-') => {
                self.negative = byte == b'-';
                Int
            }
            (Start | Int, b'0'..=b'9') => {
                self.mantissa_digit(byte);
                Int
            }
            (Start | Int, b'.') => Frac,
            (Frac, b'0'..=b'9') => {
                self.mantissa_digit(byte);
                self.frac_digits += 1;
                Frac
            }
            (Int | Frac, b'e' | b'E') => ExpStart,
            (ExpStart, b'+' | b'-') => {
                self.exp_negative = byte == b'-';
                ExpSign
            }
            (ExpStart | ExpSign | Exp, b'0'..=b'9') => {
                self.exp = (self.exp * 10 + i64::from(byte - b'0')).min(MAX_EXPONENT);
                Exp
            }
            _ => Bad,
        };
    }

    /// The word's value, within |n| <= 1e6.
    fn value(&self, line: usize) -> Result<f64, ObjError> {
        use std::fmt::Write;
        if !matches!(
            self.part,
            NumberPart::Int | NumberPart::Frac | NumberPart::Exp
        ) || self.mantissa_digits == 0
        {
            return Err(ObjError::BadNumber {
                line,
                text: self.quote.text(),
            });
        }
        let value = if self.significant_count == 0 {
            if self.negative { -0.0 } else { 0.0 }
        } else {
            // significant × 10^exp, the dropped digits standing in as one
            // nonzero digit when any was nonzero.
            let dropped = (self.significant_count - self.significant.len() as u64) as i64;
            let exp = if self.exp_negative {
                -self.exp
            } else {
                self.exp
            } - self.frac_digits as i64
                + dropped
                - i64::from(self.sticky);
            let mut text = String::with_capacity(self.significant.len() + 24);
            if self.negative {
                text.push('-');
            }
            text.push_str(std::str::from_utf8(&self.significant).expect("ASCII digits"));
            if self.sticky {
                text.push('1');
            }
            let _ = write!(text, "e{exp}");
            text.parse::<f64>().map_err(|_| ObjError::BadNumber {
                line,
                text: self.quote.text(),
            })?
        };
        if !value.is_finite() || value.abs() > MAX_COORDINATE {
            return Err(ObjError::NumberOutOfBounds { line });
        }
        Ok(value)
    }
}

/// [`NumberWord`] over a whole `text`.
#[cfg(test)]
fn parse_number(text: &str, line: usize) -> Result<f64, ObjError> {
    let mut word = NumberWord::default();
    for &byte in text.as_bytes() {
        word.push(byte);
    }
    word.value(line)
}

/// One face reference (`7`, `7/1`, `-2//3`), checked as its bytes arrive:
/// its position index is the text before the first `/`, a decimal
/// integer. Gnosis and canvas-3d read the value, so leading zeros of any
/// length are valid.
#[derive(Default)]
struct IndexWord {
    quote: Quote,
    /// Past the first `/`: the rest of the reference is ignored.
    past_index: bool,
    len: usize,
    negative: bool,
    digits: usize,
    /// Digits from the first nonzero one (saturating).
    significant: u32,
    magnitude: i64,
    bad: bool,
}

impl IndexWord {
    fn clear(&mut self) {
        self.quote.clear();
        self.past_index = false;
        self.len = 0;
        self.negative = false;
        self.digits = 0;
        self.significant = 0;
        self.magnitude = 0;
        self.bad = false;
    }

    fn push(&mut self, byte: u8) {
        if self.past_index {
            return;
        }
        if byte == b'/' {
            self.past_index = true;
            return;
        }
        self.quote.push(byte);
        self.len += 1;
        match byte {
            b'-' if self.len == 1 => self.negative = true,
            b'0'..=b'9' => {
                self.digits += 1;
                if self.significant > 0 || byte != b'0' {
                    self.significant = self.significant.saturating_add(1);
                    if self.significant <= 18 {
                        self.magnitude = self.magnitude * 10 + i64::from(byte - b'0');
                    }
                }
            }
            _ => self.bad = true,
        }
    }

    /// The 0-based vertex the reference names among `count` defined so
    /// far: 1-based when positive, relative to `count` when negative.
    fn resolve(&self, count: usize, line: usize) -> Result<u32, ObjError> {
        if self.bad || self.digits == 0 {
            return Err(ObjError::BadIndex {
                line,
                text: self.quote.text(),
            });
        }
        // Past 18 significant digits it is out of range for any file.
        if self.significant > 18 {
            return Err(ObjError::IndexOutOfRange { line });
        }
        let n = if self.negative {
            -self.magnitude
        } else {
            self.magnitude
        };
        let count = count as i64;
        let resolved = if n > 0 { n - 1 } else { count + n };
        if n == 0 || resolved < 0 || resolved >= count {
            return Err(ObjError::IndexOutOfRange { line });
        }
        Ok(resolved as u32)
    }
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

/// Bytes parsed between two cancellation polls, however they split into
/// lines and words.
const SCAN_WINDOW: usize = READ_CHUNK;

/// Lines, words or triangles handled between two cancellation polls.
const CHECK_EVERY: usize = 1 << 16;

/// What the current line is, once its first word is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Record {
    /// Before or inside the line's first word.
    Keyword,
    /// A `v` line with this many coordinates read.
    Vertex(usize),
    /// An `f` line; its references so far are in `polygon`.
    Face,
    /// Any other line, or a `v` line past its third coordinate.
    Ignored,
}

/// Incremental OBJ parser, fed the file in chunks as it is read.
///
/// It is a byte-level state machine: it keeps no line or word, only
/// bounded per-word state, so one huge word (a zero-padded index, a long
/// coordinate) costs no more than many short ones. Every
/// [`SCAN_WINDOW`] bytes, and every [`CHECK_EVERY`] lines, words or
/// triangles, it polls the cancel flag; the caps, including a face's
/// reference allowance, are enforced as the bytes arrive. Errors are
/// reported in file order (invalid UTF-8 too), whatever the chunking.
pub struct ObjParser {
    limits: ObjLimits,
    vertices: Vec<[f64; 3]>,
    triangles: Vec<[u32; 3]>,
    polygon: Vec<u32>,
    /// Number of the current line (1-based).
    line: usize,
    /// Work units since the last cancellation poll.
    work: usize,
    /// Bytes parsed since the last cancellation poll.
    scanned: usize,
    /// The start of a UTF-8 character split by the end of the last chunk.
    split_char: Vec<u8>,
    record: Record,
    /// Skipping to the end of the line: a comment or an ignored record.
    skipping: bool,
    in_word: bool,
    /// The first byte of the line's first word.
    keyword: u8,
    point: [f64; 3],
    /// References the current face may have: k of them are k - 2
    /// triangles, within what is left of the allowance.
    max_refs: usize,
    number: NumberWord,
    index: IndexWord,
}

impl ObjParser {
    pub fn new(limits: ObjLimits) -> Self {
        Self {
            limits,
            vertices: Vec::new(),
            triangles: Vec::new(),
            polygon: Vec::new(),
            line: 1,
            work: 0,
            scanned: 0,
            split_char: Vec::new(),
            record: Record::Keyword,
            skipping: false,
            in_word: false,
            keyword: 0,
            point: [0.0; 3],
            max_refs: 0,
            number: NumberWord::default(),
            index: IndexWord::default(),
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

    /// Parse `chunk`, the file's next bytes. It is cut where the current
    /// [`SCAN_WINDOW`] ends, so the poll falls every `SCAN_WINDOW` bytes of
    /// the file however the caller sizes its chunks.
    pub fn feed(&mut self, mut chunk: &[u8], cancelled: &dyn Fn() -> bool) -> Result<(), ObjError> {
        while !chunk.is_empty() {
            let allowance = SCAN_WINDOW - self.scanned;
            let (window, rest) = chunk.split_at(allowance.min(chunk.len()));
            self.feed_window(window, cancelled)?;
            chunk = rest;
        }
        Ok(())
    }

    /// The end of the file: its last (possibly unterminated) line, then
    /// the mesh.
    pub fn finish(mut self, cancelled: &dyn Fn() -> bool) -> Result<ObjMesh, ObjError> {
        if !self.split_char.is_empty() {
            return Err(ObjError::NotUtf8 { line: self.line });
        }
        self.skipping = false;
        self.end_line(cancelled)?;
        if self.triangles.is_empty() {
            return Err(ObjError::NoTriangles);
        }
        Ok(ObjMesh {
            vertices: self.vertices,
            triangles: self.triangles,
        })
    }

    /// Validate `window` (at most what is left of the current
    /// [`SCAN_WINDOW`]) as UTF-8 and parse it, then poll once the whole
    /// window has gone by.
    fn feed_window(&mut self, window: &[u8], cancelled: &dyn Fn() -> bool) -> Result<(), ObjError> {
        let joined;
        let bytes = if self.split_char.is_empty() {
            window
        } else {
            let mut start = std::mem::take(&mut self.split_char);
            start.extend_from_slice(window);
            joined = start;
            &joined[..]
        };
        let (valid, invalid) = match std::str::from_utf8(bytes) {
            Ok(_) => (bytes.len(), false),
            // A character split by the end of the chunk: finish it next time.
            Err(e) if e.error_len().is_none() => (e.valid_up_to(), false),
            Err(e) => (e.valid_up_to(), true),
        };
        // What precedes an invalid byte is parsed first, so the error
        // reported is the first in the file.
        self.parse_bytes(&bytes[..valid], cancelled)?;
        if invalid {
            return Err(ObjError::NotUtf8 { line: self.line });
        }
        self.split_char.extend_from_slice(&bytes[valid..]);
        self.scanned += window.len();
        if self.scanned >= SCAN_WINDOW {
            self.scanned -= SCAN_WINDOW;
            if cancelled() {
                return Err(ObjError::Cancelled);
            }
        }
        Ok(())
    }

    /// Parse valid UTF-8 `bytes` (whole characters).
    fn parse_bytes(
        &mut self,
        mut bytes: &[u8],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), ObjError> {
        while let Some((&byte, rest)) = bytes.split_first() {
            if self.skipping {
                match bytes.iter().position(|b| *b == b'\n') {
                    Some(end) => {
                        self.skipping = false;
                        bytes = &bytes[end..];
                        continue;
                    }
                    None => return Ok(()),
                }
            }
            bytes = rest;
            match byte {
                b'\n' => self.end_line(cancelled)?,
                b'#' => {
                    self.end_word(cancelled)?;
                    self.skipping = true;
                }
                b' ' | b'\t' | b'\r' => self.end_word(cancelled)?,
                _ => self.word_byte(byte, cancelled)?,
            }
        }
        Ok(())
    }

    fn word_byte(&mut self, byte: u8, cancelled: &dyn Fn() -> bool) -> Result<(), ObjError> {
        if !self.in_word {
            self.in_word = true;
            self.tick(1, cancelled)?;
            match self.record {
                Record::Keyword => {
                    self.keyword = byte;
                    return Ok(());
                }
                Record::Vertex(_) => self.number.clear(),
                Record::Face => {
                    // Stop as soon as the polygon alone exceeds the
                    // remaining allowance.
                    if self.polygon.len() == self.max_refs {
                        return Err(ObjError::TooManyTriangles {
                            max: self.limits.max_triangles,
                        });
                    }
                    self.index.clear();
                }
                Record::Ignored => {}
            }
        }
        match self.record {
            // A second byte: neither `v` nor `f`, an ignored record.
            Record::Keyword => {
                self.in_word = false;
                self.record = Record::Ignored;
                self.skipping = true;
            }
            Record::Vertex(_) => self.number.push(byte),
            Record::Face => self.index.push(byte),
            Record::Ignored => {}
        }
        Ok(())
    }

    fn end_word(&mut self, _cancelled: &dyn Fn() -> bool) -> Result<(), ObjError> {
        if !self.in_word {
            return Ok(());
        }
        self.in_word = false;
        let line = self.line;
        match self.record {
            Record::Keyword => {
                self.record = match self.keyword {
                    b'v' => Record::Vertex(0),
                    b'f' => {
                        self.polygon.clear();
                        self.max_refs = self
                            .limits
                            .max_triangles
                            .saturating_sub(self.triangles.len())
                            .saturating_add(2);
                        Record::Face
                    }
                    _ => {
                        self.skipping = true;
                        Record::Ignored
                    }
                };
            }
            Record::Vertex(filled) => {
                self.point[filled] = self.number.value(line)?;
                if filled < 2 {
                    self.record = Record::Vertex(filled + 1);
                } else {
                    if self.vertices.len() >= self.limits.max_vertices {
                        return Err(ObjError::TooManyVertices {
                            max: self.limits.max_vertices,
                        });
                    }
                    self.vertices.push(self.point);
                    // Coordinates past the third are ignored.
                    self.record = Record::Ignored;
                    self.skipping = true;
                }
            }
            Record::Face => {
                let index = self.index.resolve(self.vertices.len(), line)?;
                self.polygon.push(index);
            }
            Record::Ignored => {}
        }
        Ok(())
    }

    fn end_line(&mut self, cancelled: &dyn Fn() -> bool) -> Result<(), ObjError> {
        self.end_word(cancelled)?;
        let line = self.line;
        match self.record {
            Record::Vertex(_) => return Err(ObjError::ShortVertex { line }),
            Record::Face => {
                if self.polygon.len() < 3 {
                    return Err(ObjError::ShortFace { line });
                }
                for i in 1..self.polygon.len() - 1 {
                    self.triangles
                        .push([self.polygon[0], self.polygon[i], self.polygon[i + 1]]);
                    self.tick(1, cancelled)?;
                }
            }
            Record::Keyword | Record::Ignored => {}
        }
        self.line += 1;
        self.record = Record::Keyword;
        self.skipping = false;
        self.tick(1, cancelled)
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
    let mut options = std::fs::File::options();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A FIFO (including a symlink to one) must not block the open.
        options.custom_flags(libc::O_NONBLOCK);
    }
    // Validate the descriptor on every platform: the file checked is the
    // file read, even if a pathname is replaced between lookup and open.
    let file = options
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
    // The parser's own polls (not the read loop's) report the parse phase
    // to a test observer: they run only once parsing is under way.
    let parse_poll = || {
        super::loader::probe("parse", path);
        cancelled()
    };
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
        parser.feed(&chunk[..n], &parse_poll)?;
    }
    if let (Some(expected), Some(hasher)) = (sha256, hasher)
        && hasher.finalize().as_slice() != expected.as_slice()
    {
        return Err(ObjError::DigestMismatch);
    }
    if cancelled() {
        return Err(ObjError::Cancelled);
    }
    parser.finish(&parse_poll).map(|mesh| (mesh, read))
}

#[cfg(test)]
#[path = "obj/tests.rs"]
mod tests;
