//! Case conversion and character builtins.
//!
//! Implements `capitalize`, `upcase-initials`, and `char-resolve-modifiers`.

use super::casetab::{CaseMap, CaseTableOverride};
use super::error::{EvalResult, Flow, signal};
use super::value::*;
use crate::buffer::{EmacsBytePos, EmacsByteRange};
use crate::emacs_core::error::LispCondition;
use crate::emacs_core::error::expect_args;
use crate::emacs_core::value::ValueKind;
use crate::heap_types::LispString;

// ---------------------------------------------------------------------------
// Argument helpers
// ---------------------------------------------------------------------------

fn expect_min_max_args(name: &str, args: &[Value], min: usize, max: usize) -> Result<(), Flow> {
    if args.len() < min || args.len() > max {
        Err(signal(
            LispCondition::WrongNumberOfArguments,
            vec![Value::symbol(name), Value::fixnum(args.len() as i64)],
        ))
    } else {
        Ok(())
    }
}

fn expect_int(value: &Value) -> Result<i64, Flow> {
    match value.kind() {
        ValueKind::Fixnum(n) => Ok(n),
        _other => Err(signal(
            LispCondition::WrongTypeArgument,
            vec![Value::symbol("integerp"), *value],
        )),
    }
}

// ---------------------------------------------------------------------------
// Character helpers
// ---------------------------------------------------------------------------

#[allow(dead_code)] // grandfathered when dead_code lint was enabled; delete or wire up
const CHAR_META: i64 = 0x8000000;
#[allow(dead_code)] // grandfathered when dead_code lint was enabled; delete or wire up
const CHAR_CTL: i64 = 0x4000000;
#[allow(dead_code)] // grandfathered when dead_code lint was enabled; delete or wire up
const CHAR_SHIFT: i64 = 0x2000000;
#[allow(dead_code)] // grandfathered when dead_code lint was enabled; delete or wire up
const CHAR_HYPER: i64 = 0x1000000;
#[allow(dead_code)] // grandfathered when dead_code lint was enabled; delete or wire up
const CHAR_SUPER: i64 = 0x0800000;
#[allow(dead_code)] // grandfathered when dead_code lint was enabled; delete or wire up
const CHAR_ALT: i64 = 0x0400000;
#[allow(dead_code)] // grandfathered when dead_code lint was enabled; delete or wire up
const CHAR_MODIFIER_MASK: i64 =
    CHAR_META | CHAR_CTL | CHAR_SHIFT | CHAR_HYPER | CHAR_SUPER | CHAR_ALT;

/// Convert a character code to a Rust char (if it's a valid Unicode scalar value).
fn code_to_char(code: i64) -> Option<char> {
    if (0..=0x10FFFF).contains(&code) {
        char::from_u32(code as u32)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Case conversion helpers
// ---------------------------------------------------------------------------

/// Uppercase a single character code, consulting the per-buffer case table
/// first (GNU `upcase`), then falling through to the hardwired mapping.
fn upcase_char_override(code: i64, casetab: &CaseTableOverride) -> i64 {
    casetab
        .map(CaseMap::Up, code)
        .unwrap_or_else(|| upcase_char(code))
}

/// Character title casing prepares all GNU casing properties before selecting
/// the authoritative current-buffer table. Only the simple title table survives
/// subsequent lazy-load callbacks; special strings never enter this result.
fn titlecase_character_in_state(eval: &mut super::eval::Context, code: i64) -> EvalResult {
    if !(0..=super::emacs_char::MAX_CHAR as i64).contains(&code) {
        let casetab = CaseTableOverride::for_current_buffer(eval)?;
        return Ok(Value::fixnum(upcase_char_override(code, &casetab)));
    }
    let title_table =
        super::chartable::uniprop_table_in_state(eval, Value::symbol("titlecase"))?;
    let roots = eval.save_specpdl_roots();
    if !title_table.is_nil() {
        eval.push_specpdl_root(title_table);
    }
    let result = (|| {
        // `prepare_casing_context` loads these even for character operations.
        // A callback on the final property can replace the buffer table or Up.
        for property in ["special-uppercase", "special-lowercase", "special-titlecase"] {
            let _ = super::chartable::uniprop_table_in_state(eval, Value::symbol(property))?;
        }
        // No copied case-table Values cross a Lisp-loading boundary. Rooting
        // an old Up snapshot would not make that snapshot authoritative.
        let casetab = CaseTableOverride::for_current_buffer(eval)?;
        if !title_table.is_nil() {
            // GNU character casing uses CHAR_TABLE_REF, not the public decoded
            // property API. Accept explicit identity but reject non-characters.
            let mapped = super::chartable::ct_lookup(&title_table, code)?;
            if let ValueKind::Fixnum(title) = mapped.kind() {
                if (0..=super::emacs_char::MAX_CHAR as i64).contains(&title) {
                    return Ok(mapped);
                }
            }
        }
        Ok(Value::fixnum(upcase_char_override(code, &casetab)))
    })();
    eval.restore_specpdl_roots(roots);
    result
}

/// Uppercase a single character code, returning the new code.
fn upcase_char(code: i64) -> i64 {
    if preserve_casefiddle_upcase_payload(code) {
        return code;
    }
    match code {
        223 => return 7838,
        452 | 497 => return code + 1,
        454 | 457 | 460 | 499 => return code - 1,
        455 | 458 => return code + 1,
        8064..=8071 | 8080..=8087 | 8096..=8103 => return code + 8,
        8115 | 8131 | 8179 => return code + 9,
        _ => {}
    }
    match code_to_char(code) {
        Some(c) => {
            let mut upper = c.to_uppercase();
            // to_uppercase() may yield multiple chars (e.g. German eszett);
            // take only the first to stay consistent with Emacs behavior.
            upper.next().map(|u| u as i64).unwrap_or(code)
        }
        None => code,
    }
}

fn preserve_casefiddle_upcase_payload(code: i64) -> bool {
    matches!(
        code,
        329
            | 411
            | 453
            | 456
            | 459
            | 496
            | 498
            | 612
            | 912
            | 944
            | 1415
            | 4304..=4346
            | 4349..=4351
            | 7306
            | 7830..=7834
            | 8016
            | 8018
            | 8020
            | 8022
            | 8072..=8079
            | 8088..=8095
            | 8104..=8111
            | 8114
            | 8116
            | 8118..=8119
            | 8124
            | 8130
            | 8132
            | 8134..=8135
            | 8140
            | 8146..=8147
            | 8150..=8151
            | 8162..=8164
            | 8166..=8167
            | 8178
            | 8180
            | 8182..=8183
            | 8188
            | 42957
            | 42959
            | 42963
            | 42965
            | 42971
            | 64256..=64262
            | 64275..=64279
            | 68976..=68997
            | 93883..=93907
    )
}

fn titlecase_from_uppercase_expansion(expansion: &[char]) -> String {
    let mut result = String::new();
    let mut seen_cased = false;

    for uc in expansion {
        let is_cased = uc.is_uppercase() || uc.is_lowercase();
        if !seen_cased {
            result.push(*uc);
            if is_cased {
                seen_cased = true;
            }
            continue;
        }

        if is_cased {
            for lc in uc.to_lowercase() {
                result.push(lc);
            }
        } else {
            result.push(*uc);
        }
    }

    result
}

fn titlecase_combining_iota_override(code: i64) -> Option<&'static str> {
    match code {
        8114 => Some("\u{1FBA}\u{0345}"),
        8116 => Some("\u{0386}\u{0345}"),
        8119 => Some("\u{0391}\u{0342}\u{0345}"),
        8130 => Some("\u{1FCA}\u{0345}"),
        8132 => Some("\u{0389}\u{0345}"),
        8135 => Some("\u{0397}\u{0342}\u{0345}"),
        8178 => Some("\u{1FFA}\u{0345}"),
        8180 => Some("\u{038F}\u{0345}"),
        8183 => Some("\u{03A9}\u{0342}\u{0345}"),
        _ => None,
    }
}

fn titlecase_uses_precomposed_upcase(code: i64) -> bool {
    matches!(
        code,
        8064..=8071
            | 8072..=8111
            | 8115
            | 8124
            | 8131
            | 8140
            | 8179
            | 8188
    )
}

fn titlecase_word_initial(c: char) -> String {
    let code = c as i64;
    if let Some(explicit) = titlecase_combining_iota_override(code) {
        return explicit.to_string();
    }

    let expansion: Vec<char> = c.to_uppercase().collect();
    if expansion.len() > 1 && !titlecase_uses_precomposed_upcase(code) {
        return titlecase_from_uppercase_expansion(&expansion);
    }

    if let Some(mapped) = code_to_char(upcase_char(code)) {
        mapped.to_string()
    } else {
        c.to_uppercase().collect()
    }
}

fn push_multibyte_char_code(out: &mut Vec<u8>, code: u32) {
    let mut buf = [0u8; crate::emacs_core::emacs_char::MAX_MULTIBYTE_LENGTH];
    let len = crate::emacs_core::emacs_char::char_string(code, &mut buf);
    out.extend_from_slice(&buf[..len]);
}

fn push_multibyte_chars(out: &mut Vec<u8>, chars: impl IntoIterator<Item = char>) {
    for ch in chars {
        push_multibyte_char_code(out, ch as u32);
    }
}

/// GNU `do_casify_multibyte_region` first/last changed span, in byte offsets
/// of the cased text. A character is changed when an admissible special
/// string was used (even a byte-identical one) or its mapping differs.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CaseChangeSpan {
    old_pos: usize,
    first: Option<usize>,
    old_end: usize,
    new_end: usize,
}

impl CaseChangeSpan {
    fn note(&mut self, code: u32, out: &[u8], out_start: usize, special: bool) {
        let mut buf = [0u8; crate::emacs_core::emacs_char::MAX_MULTIBYTE_LENGTH];
        let len = crate::emacs_core::emacs_char::char_string(code, &mut buf);
        let old_start = self.old_pos;
        self.old_pos += len;
        if special || out[out_start..] != buf[..len] {
            // Text before the first change is identical in both coordinates.
            self.first.get_or_insert(old_start);
            self.old_end = self.old_pos;
            self.new_end = out.len();
        }
    }

    /// Unibyte casing is per byte: changed exactly where bytes differ.
    fn from_byte_diff(old: &[u8], new: &[u8]) -> Self {
        if old.len() != new.len() {
            return Self { old_pos: old.len(), first: Some(0), old_end: old.len(), new_end: new.len() };
        }
        let first = old.iter().zip(new).position(|(a, b)| a != b);
        let last = old.iter().zip(new).rposition(|(a, b)| a != b).map_or(0, |i| i + 1);
        Self { old_pos: old.len(), first, old_end: last, new_end: last }
    }
}

/// Word-boundary predicate over the *standard* syntax table, for the pure/test
/// casing forms that run without a current buffer. Mirrors GNU's `Sword` test
/// against the standard syntax table.
#[allow(dead_code)] // grandfathered when dead_code lint was enabled; delete or wire up
fn standard_word_predicate(code: u32) -> bool {
    crate::emacs_core::syntax::standard_syntax_class_for_code(code)
        == crate::emacs_core::syntax::SyntaxClass::Word
}

/// GNU unibyte object casing falls back to the ASCII Unicode property when
/// an ASCII input maps outside 0..=255. Buffer casing instead keeps the low
/// byte (`do_casify_unibyte_region`), including replace-match after insertion.
/// High input bytes are raw-byte characters, not Latin-1 case-table indices.
pub(crate) fn casify_unibyte_string(
    text: &LispString,
    which: CaseMap,
    casetab: &CaseTableOverride,
    target: CaseTarget,
) -> LispString {
    let bytes = text
        .as_bytes()
        .iter()
        .map(|&byte| {
            let code = crate::emacs_core::emacs_char::unibyte_to_char(byte) as i64;
            let ascii = if which == CaseMap::Down {
                byte.to_ascii_lowercase()
            } else {
                byte.to_ascii_uppercase()
            };
            let mapped = casetab.map(which, code).unwrap_or(ascii as i64);
            if target == CaseTarget::String && byte.is_ascii() && mapped >= 0x100 {
                ascii
            } else {
                make_char_unibyte(mapped)
            }
        })
        .collect();
    LispString::from_unibyte(bytes)
}

/// Prepare the existing GNU uniprop tables before copying current-buffer
/// casing authority. Retain each result across later lazy-load callbacks;
/// the input roots supplied by object callers and all tables unwind on Flow.
pub(crate) fn with_text_downcase_table<T>(
    eval: &mut super::eval::Context,
    capitalize: bool,
    body: impl FnOnce(&mut super::eval::Context, Value) -> Result<T, Flow>,
) -> Result<T, Flow> {
    let roots = eval.save_specpdl_roots();
    let result = (|| {
        if capitalize {
            for property in ["titlecase", "special-uppercase"] {
                let table = super::chartable::uniprop_table_in_state(eval, Value::symbol(property))?;
                eval.push_specpdl_root(table);
            }
        }
        let lower = super::chartable::uniprop_table_in_state(eval, Value::symbol("special-lowercase"))?;
        eval.push_specpdl_root(lower);
        if capitalize {
            let table = super::chartable::uniprop_table_in_state(eval, Value::symbol("special-titlecase"))?;
            eval.push_specpdl_root(table);
        }
        body(eval, lower)
    })();
    eval.restore_specpdl_roots(roots);
    result
}

/// GNU text DOWN: special lowercase precedes the simple Down table; final
/// sigma is postprocessed using original syntax, not the mapped character.
/// Numeric character casing and unibyte casing do not use this expansion step.
fn push_text_downcase(
    out: &mut Vec<u8>,
    code: u32,
    casetab: &CaseTableOverride,
    was_inword: bool,
    next_word: bool,
    special_lowercase: Option<Value>,
) -> bool {
    // None is only the no-Context pure helper. Runtime Some(nil) explicitly
    // means unavailable support, never an unconditional Rust expansion.
    if let Some(table) = special_lowercase {
        if !table.is_nil() {
            // The prepared loader already validates purpose, slots and decoder.
            // GNU casing reads raw CHAR_TABLE_REF, not decoded public properties.
            if let Ok(property) = super::chartable::ct_lookup(&table, code as i64) {
                if let Some(string) = property.as_lisp_string() {
                    if string.sbytes() <= 6 {
                        if code == 0x03A3 && was_inword && !next_word {
                            push_multibyte_char_code(out, 0x03C2);
                        } else {
                            out.extend_from_slice(string.as_bytes());
                        }
                        return true;
                    }
                }
            }
        }
    } else if let Some(ch) = code_to_char(code as i64) {
        let expansion = ch.to_lowercase();
        if expansion.clone().nth(1).is_some() {
            push_multibyte_chars(out, expansion);
            return false;
        }
    }
    // The standard simple mapping already preserves Kelvin and other GNU
    // table exceptions. Explicit custom mappings (including nil identity)
    // remain authoritative; no blanket Kelvin exception can precede them.
    let mapped = casetab.map(CaseMap::Down, code as i64).unwrap_or_else(|| {
        super::builtins::downcase_char_code_emacs_compat(code as i64)
    }) as u32;
    let mapped = if code == 0x03A3 && mapped != code && was_inword && !next_word {
        0x03C2
    } else {
        mapped
    };
    push_multibyte_char_code(out, mapped);
    false
}

pub(crate) fn downcase_lisp_string_emacs_compat(
    text: &LispString,
    is_word: impl Fn(u32) -> bool,
    is_prefix: impl Fn(u32) -> bool,
    casetab: &CaseTableOverride,
    target: CaseTarget,
    special_lowercase: Option<Value>,
) -> LispString {
    downcase_lisp_string_tracked(text, is_word, is_prefix, casetab, target, special_lowercase, None)
}

fn downcase_lisp_string_tracked(
    text: &LispString,
    is_word: impl Fn(u32) -> bool,
    is_prefix: impl Fn(u32) -> bool,
    casetab: &CaseTableOverride,
    target: CaseTarget,
    special_lowercase: Option<Value>,
    mut span: Option<&mut CaseChangeSpan>,
) -> LispString {
    if !text.is_multibyte() {
        return casify_unibyte_string(text, CaseMap::Down, casetab, target);
    }

    let codes = super::builtins::lisp_string_char_codes(text);
    let mut out = Vec::with_capacity(text.sbytes());
    let mut prev_word = false;
    for (i, &code) in codes.iter().enumerate() {
        let next_word = codes.get(i + 1).is_some_and(|&next| is_word(next));
        let start = out.len();
        let special = push_text_downcase(&mut out, code, casetab, prev_word, next_word, special_lowercase);
        if let Some(span) = span.as_deref_mut() {
            span.note(code, &out, start, special);
        }
        prev_word = is_word(code)
            && (prev_word || target == CaseTarget::String || !is_prefix(code));
    }
    LispString::from_emacs_bytes(out)
}

fn upcase_lisp_string_emacs_compat(
    text: &LispString,
    casetab: &CaseTableOverride,
    target: CaseTarget,
) -> LispString {
    if !text.is_multibyte() {
        return casify_unibyte_string(text, CaseMap::Up, casetab, target);
    }

    let mut out = Vec::with_capacity(text.sbytes());
    for code in super::builtins::lisp_string_char_codes(text) {
        let code_i64 = code as i64;
        // GNU `case_character_impl` checks special-uppercase before the
        // one-to-one Up table, but only for multibyte strings/buffer text.
        if let Some(expansion) = special_upcase_expansion(code) {
            push_multibyte_chars(&mut out, expansion);
            continue;
        }
        if let Some(mapped) = casetab.map(CaseMap::Up, code_i64) {
            push_multibyte_char_code(&mut out, mapped as u32);
            continue;
        }
        if code == 0x0131 || preserve_upcase_case_string_payload(code_i64) {
            push_multibyte_char_code(&mut out, code);
            continue;
        }
        if let Some(ch) = code_to_char(code_i64) {
            push_multibyte_chars(&mut out, ch.to_uppercase());
        } else {
            push_multibyte_char_code(&mut out, code);
        }
    }
    LispString::from_emacs_bytes(out)
}

/// The one-to-many uppercase mappings already used by string casing. GNU
/// applies these before a custom Up entry; single-character/unibyte casing
/// still consults only the one-to-one table (`case_single_character`).
pub(crate) fn special_upcase_expansion(code: u32) -> Option<std::char::ToUppercase> {
    let expansion = char::from_u32(code)?.to_uppercase();
    expansion.clone().nth(1).is_some().then_some(expansion)
}

/// Whether [`capitalize_like_gnu`] cases a string or buffer text.  GNU
/// differs between the two in the syntax prefix rule and in how a unibyte
/// byte takes a case-table mapping that does not fit a byte.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaseTarget {
    String,
    Buffer,
}

/// What [`capitalize_like_gnu`] does to a character inside a word.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WordRest {
    /// `capitalize`: down-case it.
    Downcase,
    /// `upcase-initials`: leave it alone.
    Keep,
}

/// The cased form of a word-initial character, GNU `case_character_impl`
/// with flag `CASE_CAPITALIZE`: the Unicode title-case mapping (special
/// casing, then the `titlecase` property) wins; only a character without one
/// goes through the case table (`upcase`), which leaves it unchanged when
/// the table has no entry.  So a buffer case table that pairs `Q` with `a`
/// still capitalizes `a` to `A`, as in GNU.
fn push_word_initial(out: &mut Vec<u8>, code: u32, casetab: &CaseTableOverride) {
    if code < 0x80 {
        // ASCII: only lowercase letters have a title case (their upper case).
        let byte = code as u8;
        let mapped = if byte.is_ascii_lowercase() {
            byte.to_ascii_uppercase() as i64
        } else {
            casetab
                .map(CaseMap::Up, code as i64)
                .unwrap_or_else(|| byte.to_ascii_uppercase() as i64)
        };
        push_multibyte_char_code(out, mapped as u32);
        return;
    }
    if let Some(c) = code_to_char(code as i64) {
        let title = titlecase_word_initial(c);
        let mut chars = title.chars();
        if chars.next() != Some(c) || chars.next().is_some() {
            push_multibyte_chars(out, title.chars());
            return;
        }
    }
    let mapped = casetab
        .map(CaseMap::Up, code as i64)
        .unwrap_or_else(|| upcase_char(code as i64));
    push_multibyte_char_code(out, mapped as u32);
}

/// GNU `casify_object` / `casify_region` with `CASE_CAPITALIZE` or
/// `CASE_CAPITALIZE_UP` (`case_character_impl`).  Every character that does
/// not continue a word is title-cased, word constituent or not; characters
/// inside a word are down-cased or kept per `rest`.  A character continues
/// a word when the previous one was in a word; it starts one when it is a
/// word constituent (`is_word`, which carries `case-symbols-as-words`), and
/// in a buffer only if it lacks the syntax prefix flag (`is_prefix`).
fn capitalize_like_gnu(
    text: &LispString,
    is_word: impl Fn(u32) -> bool,
    is_prefix: impl Fn(u32) -> bool,
    casetab: &CaseTableOverride,
    rest: WordRest,
    target: CaseTarget,
    special_lowercase: Option<Value>,
) -> LispString {
    capitalize_like_gnu_tracked(text, is_word, is_prefix, casetab, rest, target, special_lowercase, None)
}

#[allow(clippy::too_many_arguments)]
fn capitalize_like_gnu_tracked(
    text: &LispString,
    is_word: impl Fn(u32) -> bool,
    is_prefix: impl Fn(u32) -> bool,
    casetab: &CaseTableOverride,
    rest: WordRest,
    target: CaseTarget,
    special_lowercase: Option<Value>,
    mut span: Option<&mut CaseChangeSpan>,
) -> LispString {
    let mut inword = false;
    let mut step = |code: u32| {
        let was_inword = inword;
        inword = is_word(code) && (was_inword || target == CaseTarget::String || !is_prefix(code));
        was_inword
    };

    if !text.is_multibyte() {
        // GNU `do_casify_unibyte_string` / `do_casify_unibyte_region`: each
        // byte is cased as the character `make_char_multibyte` gives, so a
        // byte above ASCII is a raw-byte character (for syntax too), which
        // has no case.  A mapping that does not fit a byte leaves an ASCII
        // byte of a string to plain ASCII casing; in a buffer it is cut to a
        // byte by `make_char_unibyte`.
        let mut out = Vec::with_capacity(text.sbytes());
        for &byte in text.as_bytes() {
            let was_inword = step(crate::emacs_core::emacs_char::unibyte_to_char(byte));
            out.push(
                if !byte.is_ascii() || (was_inword && rest == WordRest::Keep) {
                    byte
                } else if !was_inword && byte.is_ascii_lowercase() {
                    // An ASCII lowercase letter has a title-case mapping, which
                    // wins over the case table.
                    byte.to_ascii_uppercase()
                } else {
                    let (which, ascii) = if was_inword {
                        (CaseMap::Down, byte.to_ascii_lowercase())
                    } else {
                        (CaseMap::Up, byte.to_ascii_uppercase())
                    };
                    match casetab.map(which, byte as i64) {
                        None => ascii,
                        Some(m) if m < 0x100 => m as u8,
                        Some(_) if target == CaseTarget::String => ascii,
                        Some(m) => make_char_unibyte(m),
                    }
                },
            );
        }
        return LispString::from_unibyte(out);
    }

    let codes = super::builtins::lisp_string_char_codes(text);
    let mut out = Vec::with_capacity(text.sbytes());
    for (i, &code) in codes.iter().enumerate() {
        let was_inword = step(code);
        let start = out.len();
        let mut special = false;
        if !was_inword {
            push_word_initial(&mut out, code, casetab);
        } else if rest == WordRest::Downcase {
            let next_word = codes.get(i + 1).is_some_and(|&next| is_word(next));
            special = push_text_downcase(&mut out, code, casetab, was_inword, next_word, special_lowercase);
        } else {
            push_multibyte_char_code(&mut out, code);
        }
        if let Some(span) = span.as_deref_mut() {
            span.note(code, &out, start, special);
        }
    }
    LispString::from_emacs_bytes(out)
}

/// GNU `make_char_unibyte` (`CHAR_TO_BYTE8`) for a non-ASCII character: a
/// raw-byte character gives its byte, any other its low eight bits.
fn make_char_unibyte(code: i64) -> u8 {
    let code = code as u32;
    if crate::emacs_core::emacs_char::char_byte8_p(code) {
        crate::emacs_core::emacs_char::char_to_byte8(code)
    } else {
        (code & 0xFF) as u8
    }
}

/// `capitalize` on a string (no syntax prefix rule).
fn capitalize_lisp_string(
    text: &LispString,
    is_word: impl Fn(u32) -> bool,
    casetab: &CaseTableOverride,
    special_lowercase: Option<Value>,
) -> LispString {
    capitalize_like_gnu(
        text,
        is_word,
        |_| false,
        casetab,
        WordRest::Downcase,
        CaseTarget::String,
        special_lowercase,
    )
}

/// `upcase-initials` on a string (no syntax prefix rule).
pub(crate) fn upcase_initials_lisp_string(
    text: &LispString,
    is_word: impl Fn(u32) -> bool,
    casetab: &CaseTableOverride,
) -> LispString {
    capitalize_like_gnu(
        text,
        is_word,
        |_| false,
        casetab,
        WordRest::Keep,
        CaseTarget::String,
        None,
    )
}

fn preserve_upcase_case_string_payload(code: i64) -> bool {
    matches!(
        code,
        411
            | 612
            | 7306
            | 42957
            | 42959
            | 42963
            | 42965
            | 42971
            | 68976..=68997
            | 93883..=93907
    )
}

fn noncontiguous_case_regions(
    eval: &mut super::eval::Context,
) -> Result<Vec<super::position::LispRegionArgs>, Flow> {
    let extractor = eval
        .eval_symbol("region-extract-function")
        .unwrap_or(Value::symbol("buffer-substring"));
    let bounds = eval.funcall_general(extractor, vec![Value::symbol("bounds")])?;
    let bounds_list = crate::emacs_core::value::list_to_vec(&bounds).ok_or_else(|| {
        signal(
            LispCondition::WrongTypeArgument,
            vec![Value::symbol("listp"), bounds],
        )
    })?;
    bounds_list
        .into_iter()
        .map(|value| {
            if !value.is_cons() {
                return Err(signal(
                    LispCondition::WrongTypeArgument,
                    vec![Value::symbol("consp"), value],
                ));
            }
            super::position::LispRegionArgs::from_values(
                &eval.buffers,
                value.cons_car(),
                value.cons_cdr(),
            )
        })
        .collect()
}

fn replace_current_buffer_region_in_buffers(
    eval: &mut super::eval::Context,
    byte_range: EmacsByteRange,
    replacement: &LispString,
    restore_point: bool,
) -> EvalResult {
    let (buffer_id, saved_pt) = {
        let buf = eval
            .buffers
            .current_buffer()
            .ok_or_else(|| signal("error", vec![Value::string("No current buffer")]))?;
        (
            eval.buffers
                .current_buffer_id()
                .ok_or_else(|| signal("error", vec![Value::string("No current buffer")]))?,
            buf.point_emacs_byte_pos(),
        )
    };
    super::fns::replace_buffer_emacs_byte_range_lisp_string(
        eval,
        buffer_id,
        byte_range,
        replacement,
    )?;
    if restore_point {
        let restore_pos = eval
            .buffers
            .get(buffer_id)
            .map(|buf| saved_pt.min(buf.accessible_emacs_byte_region().end()));
        if let Some(restore_pos) = restore_pos {
            let _ = eval
                .buffers
                .goto_buffer_emacs_byte_pos(buffer_id, restore_pos);
        }
    }
    Ok(Value::NIL)
}

fn casify_region_without_preparation(
    eval: &mut super::eval::Context,
    args: Vec<Value>,
    name: &str,
    transform: impl Fn(&mut super::eval::Context, &LispString) -> Result<LispString, Flow> + Copy,
) -> EvalResult {
    expect_min_max_args(name, &args, 2, 3)?;

    let regions = if args.get(2).is_some_and(|value| !value.is_nil()) {
        noncontiguous_case_regions(eval)?
    } else {
        vec![super::position::LispRegionArgs::from_values(
            &eval.buffers,
            args[0],
            args[1],
        )?]
    };

    for region in regions {
        let (buffer_id, byte_range, text) = {
            let buf = eval
                .buffers
                .current_buffer()
                .ok_or_else(|| signal("error", vec![Value::string("No current buffer")]))?;
            let byte_range = region.accessible_byte_range(buf)?;
            if byte_range.is_empty() {
                continue;
            }
            if super::editfns::buffer_read_only_active_in_state(&eval.obarray, &[], buf) {
                return Err(signal(
                    LispCondition::BufferReadOnly,
                    vec![buf.name_value()],
                ));
            }
            let buffer_id = eval
                .buffers
                .current_buffer_id()
                .ok_or_else(|| signal("error", vec![Value::string("No current buffer")]))?;
            let text = buf.buffer_substring_lisp_string_range(byte_range);
            (buffer_id, byte_range, text)
        };

        // GNU `casify_region` calls `modify_text (start, end)`
        // (casefiddle.c:540) -> `prepare_to_modify_buffer_1` ->
        // `verify_interval_modification`, which signals `text-read-only` for any
        // read-only interval in [start,end) BEFORE any character is cased --
        // even when the conversion would be a no-op. The buffer-wide
        // `buffer-read-only` check above is GNU's `Fbarf_if_buffer_read_only`;
        // this is the text-property half it was missing.
        crate::emacs_core::textprop::verify_text_read_only_emacs_byte_range_in_state(
            &eval.obarray,
            &eval.buffers,
            buffer_id,
            byte_range,
        )?;

        // GNU `casify_region` (casefiddle.c) always `modify_text`s the range
        // and records `record_delete (start, ORIGINAL) + record_insert (start,
        // NEW_LEN)` even when no character changes, so the undo list keeps the
        // GNU shape `((START . END) (ORIGINAL . START) POINT ...)`.  Route the
        // edit through the casify-specific replace so the undo recording and
        // marker handling match GNU instead of the generic replace path.
        let replacement = transform(eval, &text)?;
        // GNU fires after-change-functions only when a character actually
        // changed (e.g. `downcase-region` over already-lowercase text signals
        // before- but not after-change); before-change and the undo record
        // still happen for the no-op. Match that.
        let changed = replacement.as_bytes() != text.as_bytes();
        casify_replace_current_buffer_region(eval, byte_range, &replacement, changed)?;
    }

    Ok(Value::NIL)
}

/// Apply a case-region replacement to the current buffer, recording undo with
/// GNU `casify_region`'s shape.  Point and markers are preserved for a
/// same-length change, matching GNU's in-place `replace_range_2`.
fn casify_replace_current_buffer_region(
    eval: &mut super::eval::Context,
    byte_range: EmacsByteRange,
    replacement: &LispString,
    changed: bool,
) -> EvalResult {
    let buffer_id = eval
        .buffers
        .current_buffer_id()
        .ok_or_else(|| signal("error", vec![Value::string("No current buffer")]))?;
    // GNU's `casify_region` (casefiddle.c) runs `modify_text` before and
    // `signal_after_change` after the edit; the earlier port kept only the
    // read-only checks and dropped the change hooks, so `upcase-region` &c.
    // mutated the buffer without firing before/after-change-functions -- which
    // `track-changes.el` flags as "Missing/incorrect calls to
    // before/after-change-functions" (issue #145). Bracket the casify-specific
    // replace exactly like the generic replace + the *word* case variants.
    let change = super::editfns::text_change_for_lisp_string_replacement_in_manager(
        &eval.buffers,
        buffer_id,
        byte_range,
        replacement,
    )?;
    super::editfns::signal_before_text_change(eval, change)?;
    eval.buffers
        .casify_replace_buffer_emacs_byte_range_lisp_string(buffer_id, byte_range, replacement);
    if changed {
        super::editfns::signal_after_text_change(eval, change)?;
    }
    Ok(Value::NIL)
}

fn casify_word_without_preparation(
    eval: &mut super::eval::Context,
    args: Vec<Value>,
    name: &str,
    transform: impl FnOnce(&mut super::eval::Context, &LispString) -> Result<LispString, Flow>,
) -> EvalResult {
    expect_args(name, &args, 1)?;
    let n = expect_int(&args[0])?;

    // Honor `find-word-boundary-function-table` (subword/superword) for the
    // word boundary, mirroring GNU's forward-word; computed without moving point.
    let honor = crate::emacs_core::syntax::parse_sexp_lookup_properties_enabled(eval);
    let target = crate::emacs_core::syntax::forward_word_destination(eval, n, honor);
    let (byte_range, text, buffer_name, read_only) = {
        let buf = eval
            .buffers
            .current_buffer()
            .ok_or_else(|| signal("error", vec![Value::string("No current buffer")]))?;
        let pt = buf.point_emacs_byte_pos();
        let byte_range = EmacsByteRange::ordered(pt, target);
        let text = buf.buffer_substring_lisp_string_range(byte_range);
        (
            byte_range,
            text,
            buf.name_value(),
            super::editfns::buffer_read_only_active_in_state(&eval.obarray, &[], buf),
        )
    };

    let replacement = transform(eval, &text)?;
    if replacement.schars() != text.schars() {
        // A mapping that changes the character count (`ß` -> `SS`): the
        // casify path's same-byte-length overwrite does not yet move point
        // and ZV for it, so keep the generic replacement here.
        if replacement != text {
            if read_only {
                return Err(signal(LispCondition::BufferReadOnly, vec![buffer_name]));
            }
            replace_current_buffer_region_in_buffers(eval, byte_range, &replacement, false)?;
        }
    } else if !byte_range.is_empty() {
        // GNU `casify_word` is `casify_region (PT, farend)`: the same
        // read-only checks, `modify_text` and undo record even when no
        // character changes, and a same-size overwrite that leaves text
        // properties alone -- so take the region command's path rather than a
        // generic replace.
        if read_only {
            return Err(signal(LispCondition::BufferReadOnly, vec![buffer_name]));
        }
        let buffer_id = eval
            .buffers
            .current_buffer_id()
            .ok_or_else(|| signal("error", vec![Value::string("No current buffer")]))?;
        crate::emacs_core::textprop::verify_text_read_only_emacs_byte_range_in_state(
            &eval.obarray,
            &eval.buffers,
            buffer_id,
            byte_range,
        )?;
        let changed = replacement.as_bytes() != text.as_bytes();
        casify_replace_current_buffer_region(eval, byte_range, &replacement, changed)?;
    }
    // GNU `casify_word` sets point to `casify_region(PT, farend)`, i.e. the
    // greater of point and the forward-word destination (the end of the cased
    // region), shifted by any length change. For a negative ARG this leaves
    // point at the original PT ("convert previous words but do not move").
    let _ = n;
    if let Some(id) = eval.buffers.current_buffer_id() {
        let delta = replacement.sbytes() as i64 - text.sbytes() as i64;
        let new_pt = (byte_range.end().get() as i64 + delta).max(0) as usize;
        let _ = eval
            .buffers
            .goto_buffer_emacs_byte_pos(id, EmacsBytePos::new(new_pt));
    }
    Ok(Value::NIL)
}

fn casify_buffer_range(
    eval: &mut super::eval::Context,
    byte_range: EmacsByteRange,
    prepare_downcase: Option<bool>,
    transform: impl FnOnce(&mut super::eval::Context, &LispString, Option<Value>) -> Result<(LispString, Option<CaseChangeSpan>), Flow>,
) -> Result<EmacsBytePos, Flow> {
    let (buffer_id, start, end) = {
        let buf = eval.buffers.current_buffer()
            .ok_or_else(|| signal("error", vec![Value::string("No current buffer")]))?;
        if byte_range.is_empty() {
            return Ok(byte_range.end());
        }
        if super::editfns::buffer_read_only_active_in_state(&eval.obarray, &[], buf) {
            return Err(signal(LispCondition::BufferReadOnly, vec![buf.name_value()]));
        }
        (buf.id, buf.emacs_byte_pos_to_lisp_char_pos(byte_range.start()),
            buf.emacs_byte_pos_to_lisp_char_pos(byte_range.end()))
    };
    // GNU freezes character endpoints, then modify_text, then casing preparation.
    // Neither byte coordinates nor input text may survive either callback boundary.
    eval.with_specpdl_roots(&[Value::make_buffer(buffer_id)], |eval| {
        crate::emacs_core::textprop::verify_text_read_only_emacs_byte_range_in_state(
            &eval.obarray, &eval.buffers, buffer_id, byte_range,
        )?;
        let change = super::editfns::text_change_for_unchanged_extent_in_manager(
            &eval.buffers, buffer_id, byte_range,
        )?;
        super::editfns::signal_before_text_change(eval, change)?;
        eval.set_current_buffer_unrecorded(buffer_id)?;
        let apply = |eval: &mut super::eval::Context, lower: Option<Value>| {
            // Modification hooks belong to the initiating buffer, but GNU casing
            // follows the current buffer left by callback-capable preparation.
            let buffer_id = eval.buffers.current_buffer_id()
                .ok_or_else(|| signal("error", vec![Value::string("No current buffer")]))?;
            eval.with_specpdl_roots(&[Value::make_buffer(buffer_id)], |eval| {
                let (range, text) = {
                    let buf = eval.buffers.get(buffer_id)
                        .ok_or_else(|| signal("error", vec![Value::string("Buffer was killed")]))?;
                    let region = super::position::LispRegionArgs::from_values(
                        &eval.buffers, Value::fixnum(start.as_i64()), Value::fixnum(end.as_i64()),
                    )?;
                    let range = region.accessible_byte_range(buf)?;
                    (range, buf.buffer_substring_lisp_string_range(range))
                };
                let (replacement, span) = transform(eval, &text, lower)?;
                // GNU signals after-change only over the first..last changed
                // characters; a used special string counts even when identical.
                let span = span.unwrap_or_else(|| {
                    CaseChangeSpan::from_byte_diff(text.as_bytes(), replacement.as_bytes())
                });
                let change = span.first.map(|first| {
                    let old = EmacsByteRange::new(
                        EmacsBytePos::new(range.start().get() + first),
                        EmacsBytePos::new(range.start().get() + span.old_end),
                    );
                    let new = crate::buffer::TextExtent::from_emacs_bytes(
                        &replacement.as_bytes()[first..span.new_end], true,
                    );
                    super::editfns::text_change_for_replacement_in_manager(
                        &eval.buffers, buffer_id, old, new,
                    )
                }).transpose()?;
                eval.buffers.casify_replace_buffer_emacs_byte_range_lisp_string(
                    buffer_id, range, &replacement,
                ).ok_or_else(|| signal("error", vec![Value::string("Buffer was killed")]))?;
                let new_end = EmacsBytePos::new((range.end().get() as i64
                    + replacement.sbytes() as i64 - text.sbytes() as i64).max(0) as usize);
                if let Some(change) = change {
                    super::editfns::signal_after_text_change(eval, change)?;
                }
                Ok(new_end)
            })
        };
        match prepare_downcase {
            Some(capitalize) => with_text_downcase_table(eval, capitalize, |eval, lower| {
                apply(eval, Some(lower))
            }),
            None => apply(eval, None),
        }
    })
}

fn casify_region_in_state(
    eval: &mut super::eval::Context,
    args: Vec<Value>,
    name: &str,
    prepare_downcase: Option<bool>,
    transform: impl Fn(&mut super::eval::Context, &LispString, Option<Value>) -> Result<(LispString, Option<CaseChangeSpan>), Flow> + Copy,
) -> EvalResult {
    if prepare_downcase.is_none() {
        return casify_region_without_preparation(eval, args, name, |eval, text| {
            transform(eval, text, None).map(|(text, _)| text)
        });
    }
    expect_min_max_args(name, &args, 2, 3)?;
    let regions = if args.get(2).is_some_and(|value| !value.is_nil()) {
        noncontiguous_case_regions(eval)?
    } else {
        vec![super::position::LispRegionArgs::from_values(&eval.buffers, args[0], args[1])?]
    };
    for region in regions {
        let buf = eval.buffers.current_buffer()
            .ok_or_else(|| signal("error", vec![Value::string("No current buffer")]))?;
        let range = region.accessible_byte_range(buf)?;
        casify_buffer_range(eval, range, prepare_downcase, transform)?;
    }
    Ok(Value::NIL)
}

fn casify_word_in_state(
    eval: &mut super::eval::Context,
    args: Vec<Value>,
    name: &str,
    prepare_downcase: Option<bool>,
    transform: impl FnOnce(&mut super::eval::Context, &LispString, Option<Value>) -> Result<(LispString, Option<CaseChangeSpan>), Flow>,
) -> EvalResult {
    if prepare_downcase.is_none() {
        return casify_word_without_preparation(eval, args, name, |eval, text| {
            transform(eval, text, None).map(|(text, _)| text)
        });
    }
    expect_args(name, &args, 1)?;
    let n = expect_int(&args[0])?;
    let honor = crate::emacs_core::syntax::parse_sexp_lookup_properties_enabled(eval);
    let target = crate::emacs_core::syntax::forward_word_destination(eval, n, honor);
    let buf = eval.buffers.current_buffer()
        .ok_or_else(|| signal("error", vec![Value::string("No current buffer")]))?;
    let range = EmacsByteRange::ordered(buf.point_emacs_byte_pos(), target);
    let new_end = casify_buffer_range(eval, range, prepare_downcase, transform)?;
    // Word scanning precedes preparation; do not rescan callback-mutated text.
    // Like GNU SET_PT, settle point in the current buffer left by casing.
    if let Some(buffer_id) = eval.buffers.current_buffer_id() {
        let _ = eval.buffers.goto_buffer_emacs_byte_pos(buffer_id, new_end);
    }
    Ok(Value::NIL)
}

// ---------------------------------------------------------------------------
// Pure builtins
// ---------------------------------------------------------------------------

/// `(capitalize OBJ)` -- if OBJ is a string, capitalize the first letter
/// (uppercase first, lowercase rest).  If OBJ is a character, uppercase it.
fn capitalize_with_word_pred(
    args: Vec<Value>,
    is_word: impl Fn(u32) -> bool,
    casetab: &CaseTableOverride,
    special_lowercase: Option<Value>,
) -> EvalResult {
    expect_args("capitalize", &args, 1)?;
    match args[0].kind() {
        ValueKind::String => {
            let source = args[0];
            let string = args[0].as_lisp_string().expect("string");
            let source_props = (!string.is_multibyte())
                .then(|| get_string_text_properties_table_for_value(source))
                .flatten();
            let result = Value::heap_string(capitalize_lisp_string(string, is_word, casetab, special_lowercase));
            if let Some(table) = source_props {
                set_string_text_properties_table_for_value(result, table);
            }
            Ok(result)
        }
        ValueKind::Fixnum(c) => {
            let code = c;
            Ok(Value::fixnum(upcase_char_override(code, casetab)))
        }
        _ => Err(signal(
            LispCondition::WrongTypeArgument,
            vec![Value::symbol("char-or-string-p"), args[0]],
        )),
    }
}

/// Pure form (used by tests); word boundaries follow the standard syntax table.
#[cfg(any(test, feature = "case68-test-support"))]
pub(crate) fn builtin_capitalize(args: Vec<Value>) -> EvalResult {
    capitalize_with_word_pred(args, standard_word_predicate, &CaseTableOverride::none(), None)
}

/// Dispatched form: word boundaries follow the current buffer's syntax table
/// (honoring `case-symbols-as-words` and any `set-case-syntax-pair` word
/// syntax), and case mapping follows the current buffer's case table.
pub(crate) fn builtin_capitalize_in_state(
    eval: &mut super::eval::Context,
    args: Vec<Value>,
) -> EvalResult {
    expect_args("capitalize", &args, 1)?;
    if args.len() == 1 {
        if let ValueKind::Fixnum(code) = args[0].kind() {
            return titlecase_character_in_state(eval, code);
        }
    }
    eval.with_specpdl_roots(&args.clone(), |eval| {
        with_text_downcase_table(eval, true, |eval, lower| {
            let is_word = crate::emacs_core::syntax::casing_word_predicate(eval);
            let casetab = CaseTableOverride::for_current_buffer(eval)?;
            capitalize_with_word_pred(args, is_word, &casetab, Some(lower))
        })
    })
}

/// `(upcase-initials OBJ)` -- uppercase the first letter of each word in
/// a string, leaving the rest unchanged.  For a char, uppercase it.
fn upcase_initials_with_word_pred(
    args: Vec<Value>,
    is_word: impl Fn(u32) -> bool,
    casetab: &CaseTableOverride,
) -> EvalResult {
    expect_args("upcase-initials", &args, 1)?;
    match args[0].kind() {
        ValueKind::String => {
            let source = args[0];
            let string = args[0].as_lisp_string().expect("string");
            let source_props = (!string.is_multibyte())
                .then(|| get_string_text_properties_table_for_value(source))
                .flatten();
            let result = Value::heap_string(upcase_initials_lisp_string(string, is_word, casetab));
            if let Some(table) = source_props {
                set_string_text_properties_table_for_value(result, table);
            }
            Ok(result)
        }
        ValueKind::Fixnum(c) => {
            let code = c;
            Ok(Value::fixnum(upcase_char_override(code, casetab)))
        }
        _ => Err(signal(
            LispCondition::WrongTypeArgument,
            vec![Value::symbol("char-or-string-p"), args[0]],
        )),
    }
}

/// Pure form (used by tests); word boundaries follow the standard syntax table.
#[cfg(any(test, feature = "case68-test-support"))]
pub(crate) fn builtin_upcase_initials(args: Vec<Value>) -> EvalResult {
    upcase_initials_with_word_pred(args, standard_word_predicate, &CaseTableOverride::none())
}

/// Dispatched form: word boundaries follow the current buffer's syntax table.
pub(crate) fn builtin_upcase_initials_in_state(
    eval: &mut super::eval::Context,
    args: Vec<Value>,
) -> EvalResult {
    if args.len() == 1 {
        if let ValueKind::Fixnum(code) = args[0].kind() {
            return titlecase_character_in_state(eval, code);
        }
    }
    let is_word = crate::emacs_core::syntax::casing_word_predicate(eval);
    let casetab = CaseTableOverride::for_current_buffer(eval)?;
    upcase_initials_with_word_pred(args, is_word, &casetab)
}

/// Uppercase the first letter of each word, leaving the rest unchanged.
#[allow(dead_code)] // grandfathered when dead_code lint was enabled; delete or wire up
pub(crate) fn upcase_initials_string(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut new_word = true;
    for c in s.chars() {
        if c.is_alphanumeric() {
            if new_word {
                for u in titlecase_word_initial(c).chars() {
                    result.push(u);
                }
                new_word = false;
            } else {
                result.push(c);
            }
        } else {
            result.push(c);
            new_word = true;
        }
    }
    result
}

/// Classify the original matched text to decide whether to leave
/// REPLACEMENT as-is, upcase it entirely, or capitalize each word.
///
/// This is the backing logic for `replace-match`'s FIXEDCASE=nil
/// behavior. It mirrors GNU `src/search.c:2460-2525`, including the
/// `case-symbols-as-words` branch at search.c:2486/2495/2505.
///
/// `is_word_char` decides whether a character counts as a word
/// constituent for the "start of word" check. GNU consults the
/// buffer's syntax table (`SYNTAX(prevc) == Sword`) and, when
/// `case-symbols-as-words` is non-nil, also accepts `Ssymbol`. The
/// default closure below uses the standard syntax table defaults and
/// honors `case-symbols-as-words` via the supplied flag so that
/// callers who don't have a buffer handy still behave like GNU on
/// the standard table. Callers who do have a buffer handy should
/// pass a closure that consults `BVAR(current_buffer, syntax_table)`.
///
/// See audit findings #14 and #20 in `drafts/regex-search-audit.md`:
/// the old code used Rust's Unicode `is_alphanumeric()` and ignored
/// `case-symbols-as-words` entirely.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReplaceMatchCaseAction {
    NoChange,
    AllCaps,
    CapInitial,
}

/// Faithful (Emacs-byte) classification of the matched text's casing, read
/// directly from its `LispString` bytes with no storage-String round-trip.
pub(crate) fn replace_match_case_action_lisp_default(
    matched: &LispString,
) -> ReplaceMatchCaseAction {
    replace_match_case_action_lisp(matched, default_is_word_char)
}

#[allow(dead_code)] // grandfathered when dead_code lint was enabled; delete or wire up
pub(crate) fn replace_match_case_action_with<F>(
    matched: &str,
    mut is_word_char: F,
) -> ReplaceMatchCaseAction
where
    F: FnMut(char) -> bool,
{
    let mut some_multiletter_word = false;
    let mut some_lowercase = false;
    let mut some_uppercase = false;
    let mut some_nonuppercase_initial = false;
    // GNU `Freplace_match` starts with `prevc = '\n'`, so a newline with word
    // syntax makes the first matched character continue a word.
    let mut prev_is_word = is_word_char('\n');

    for ch in matched.chars() {
        if ch.is_lowercase() {
            some_lowercase = true;
            if prev_is_word {
                some_multiletter_word = true;
            } else {
                some_nonuppercase_initial = true;
            }
        } else if ch.is_uppercase() {
            some_uppercase = true;
            if prev_is_word {
                some_multiletter_word = true;
            }
        } else if !prev_is_word {
            some_nonuppercase_initial = true;
        }

        prev_is_word = is_word_char(ch);
    }

    if !some_lowercase && some_multiletter_word {
        ReplaceMatchCaseAction::AllCaps
    } else if !some_nonuppercase_initial && some_multiletter_word {
        ReplaceMatchCaseAction::CapInitial
    } else if !some_nonuppercase_initial && some_uppercase {
        ReplaceMatchCaseAction::AllCaps
    } else {
        ReplaceMatchCaseAction::NoChange
    }
}

#[allow(dead_code)] // grandfathered when dead_code lint was enabled; delete or wire up
pub(crate) fn apply_replace_match_case(replacement: &str, matched: &str) -> String {
    apply_replace_match_case_with(replacement, matched, default_is_word_char)
}

/// Like `apply_replace_match_case`, but lets the caller supply the
/// predicate used for the "previous character is a word constituent"
/// check. Use this from paths that have a buffer syntax table in
/// scope so per-mode definitions of word constituents apply.
#[allow(dead_code)] // grandfathered when dead_code lint was enabled; delete or wire up
pub(crate) fn apply_replace_match_case_with<F>(
    replacement: &str,
    matched: &str,
    is_word_char: F,
) -> String
where
    F: FnMut(char) -> bool,
{
    let case_action = replace_match_case_action_with(matched, is_word_char);

    match case_action {
        ReplaceMatchCaseAction::NoChange => replacement.to_string(),
        ReplaceMatchCaseAction::AllCaps => replacement.to_uppercase(),
        ReplaceMatchCaseAction::CapInitial => upcase_initials_string(replacement),
    }
}

/// Emacs-byte-aware variant of [`apply_replace_match_case`]. Operates over real
/// Emacs char codes (via `emacs_char::string_char`) and the LispString case
/// primitives, so eight-bit raw bytes and Private-Use-Area glyphs are analyzed
/// and cased faithfully instead of through the legacy PUA-sentinel storage form
/// (issue #131).
pub(crate) fn apply_replace_match_case_lisp(
    replacement: &LispString,
    matched: &LispString,
) -> LispString {
    apply_replace_match_case_lisp_with(replacement, matched, default_is_word_char)
}

/// Like [`apply_replace_match_case_lisp`], but lets the caller supply the
/// predicate used for the "previous character is a word constituent" check.
/// Use this from paths that have a buffer syntax table in scope so per-mode
/// definitions of word constituents apply. Mirrors
/// [`apply_replace_match_case_with`] but stays byte-faithful (issue #131).
pub(crate) fn apply_replace_match_case_lisp_with<F>(
    replacement: &LispString,
    matched: &LispString,
    is_word_char: F,
) -> LispString
where
    F: FnMut(char) -> bool,
{
    // replace-match's case-adjustment uses the standard case mapping (it has no
    // buffer case table in scope here); this matches prior behavior.
    let casetab = CaseTableOverride::none();
    match replace_match_case_action_lisp(matched, is_word_char) {
        ReplaceMatchCaseAction::NoChange => replacement.clone(),
        ReplaceMatchCaseAction::AllCaps => {
            upcase_lisp_string_emacs_compat(replacement, &casetab, CaseTarget::String)
        }
        ReplaceMatchCaseAction::CapInitial => {
            // replace-match's case adjustment has no buffer syntax table in
            // scope, so word boundaries here follow the Unicode-alphanumeric
            // rule (matching prior behavior), independent of the buffer-aware
            // casing predicate used by capitalize/upcase-initials.
            upcase_initials_lisp_string(
                replacement,
                |code| char::from_u32(code).is_some_and(char::is_alphanumeric),
                &casetab,
            )
        }
    }
}

/// Like [`apply_replace_match_case_lisp_with`] but with caller-supplied
/// uppercase/lowercase predicates and case table, so a buffer's case table +
/// syntax table drive GNU's `Freplace_match` decisions (`UPPERCASEP` /
/// `LOWERCASEP` + `SYNTAX`). Used by the buffer replace-match path, which has
/// both tables in scope.
pub(crate) fn apply_replace_match_case_lisp_cased(
    replacement: &LispString,
    matched: &LispString,
    is_word: impl Fn(u32) -> bool,
    is_prefix: impl Fn(u32) -> bool,
    casetab: &CaseTableOverride,
    target: CaseTarget,
) -> LispString {
    let action = replace_match_case_action_lisp_cased(
        matched,
        |ch| is_word(ch as u32),
        |ch| casetab.is_upper(ch),
        |ch| casetab.is_lower(ch),
    );
    match action {
        ReplaceMatchCaseAction::NoChange => replacement.clone(),
        ReplaceMatchCaseAction::AllCaps => {
            upcase_lisp_string_emacs_compat(replacement, casetab, target)
        }
        // GNU applies `Fupcase_initials` to a string replacement and
        // `Fupcase_initials_region` to a buffer one.
        ReplaceMatchCaseAction::CapInitial => capitalize_like_gnu(
            replacement,
            is_word,
            is_prefix,
            casetab,
            WordRest::Keep,
            target,
            None,
        ),
    }
}

/// Emacs-byte-aware counterpart of [`replace_match_case_action_with`]. Mirrors
/// the same GNU `src/search.c` decision logic, but iterates the matched text's
/// Emacs char codes; codes outside the Unicode scalar range (eight-bit raw
/// bytes, extended codes) are caseless, non-word constituents — matching GNU,
/// where raw bytes have no case and are not `Sword`.
pub(crate) fn replace_match_case_action_lisp<F>(
    matched: &LispString,
    is_word_char: F,
) -> ReplaceMatchCaseAction
where
    F: FnMut(char) -> bool,
{
    // Default (no buffer case table) path: use Unicode case, matching GNU's
    // standard case table over the ASCII/Unicode range.
    replace_match_case_action_lisp_cased(
        matched,
        is_word_char,
        char::is_uppercase,
        char::is_lowercase,
    )
}

/// Like [`replace_match_case_action_lisp`] but with caller-supplied uppercase /
/// lowercase predicates, so a buffer's case table drives GNU's `UPPERCASEP` /
/// `LOWERCASEP` decisions (`src/search.c` `Freplace_match`).
pub(crate) fn replace_match_case_action_lisp_cased<W, U, L>(
    matched: &LispString,
    mut is_word_char: W,
    mut is_upper: U,
    mut is_lower: L,
) -> ReplaceMatchCaseAction
where
    W: FnMut(char) -> bool,
    U: FnMut(char) -> bool,
    L: FnMut(char) -> bool,
{
    let mut some_multiletter_word = false;
    let mut some_lowercase = false;
    let mut some_uppercase = false;
    let mut some_nonuppercase_initial = false;
    // GNU `Freplace_match` starts with `prevc = '\n'`, so a newline with word
    // syntax makes the first matched character continue a word.
    let mut prev_is_word = is_word_char('\n');

    let bytes = matched.as_bytes();
    let mut pos = 0;
    while pos < bytes.len() {
        let (code, len) = crate::emacs_core::emacs_char::string_char(&bytes[pos..]);
        pos += len.max(1);
        let ch = char::from_u32(code);
        if ch.is_some_and(&mut is_lower) {
            some_lowercase = true;
            if prev_is_word {
                some_multiletter_word = true;
            } else {
                some_nonuppercase_initial = true;
            }
        } else if ch.is_some_and(&mut is_upper) {
            some_uppercase = true;
            if prev_is_word {
                some_multiletter_word = true;
            }
        } else if !prev_is_word {
            some_nonuppercase_initial = true;
        }

        prev_is_word = ch.is_some_and(&mut is_word_char);
    }

    if !some_lowercase && some_multiletter_word {
        ReplaceMatchCaseAction::AllCaps
    } else if !some_nonuppercase_initial && some_multiletter_word {
        ReplaceMatchCaseAction::CapInitial
    } else if !some_nonuppercase_initial && some_uppercase {
        ReplaceMatchCaseAction::AllCaps
    } else {
        ReplaceMatchCaseAction::NoChange
    }
}

/// Default "is this a word constituent?" predicate for
/// `apply_replace_match_case`.
///
/// Mirrors GNU's standard syntax table: ASCII letters and digits are
/// `Sword`, `_` is `Ssymbol`, so `_` is not a word constituent in
/// the default baseline. Callers who want to honor
/// `case-symbols-as-words` or per-mode syntax tables should use
/// `apply_replace_match_case_with` with a closure that consults
/// `BVAR(current_buffer, syntax_table)`. See audit findings #14 and
/// #20 in `drafts/regex-search-audit.md`.
fn default_is_word_char(ch: char) -> bool {
    if ch.is_ascii_alphanumeric() {
        return true;
    }
    // GNU standard-syntax-table puts `$` and `%` in Sword. Neomacs's
    // `SyntaxTable::new_standard` agrees. Leave them inline here to
    // keep this hot path allocation-free.
    matches!(ch, '$' | '%')
}

fn downcase_buffer_text(
    ctx: &mut super::eval::Context,
    text: &LispString,
    capitalize: bool,
    lower: Value,
) -> Result<(LispString, Option<CaseChangeSpan>), Flow> {
    if text.schars() == 0 {
        return Ok((text.clone(), None));
    }
        let is_word = crate::emacs_core::syntax::casing_word_predicate(ctx);
        let is_prefix = crate::emacs_core::syntax::casing_prefix_predicate(ctx);
        let casetab = CaseTableOverride::for_current_buffer(ctx)?;
        let mut span = CaseChangeSpan::default();
        let cased = if capitalize {
            capitalize_like_gnu_tracked(text, is_word, is_prefix, &casetab,
                WordRest::Downcase, CaseTarget::Buffer, Some(lower), Some(&mut span))
        } else {
            downcase_lisp_string_tracked(text, is_word, is_prefix,
                &casetab, CaseTarget::Buffer, Some(lower), Some(&mut span))
        };
        // Unibyte casing records no per-character span; diff its bytes.
        Ok((cased, text.is_multibyte().then_some(span)))
}

pub(crate) fn builtin_downcase_region(
    ctx: &mut super::eval::Context,
    args: Vec<Value>,
) -> EvalResult {
    ctx.with_specpdl_roots(&args.clone(), |ctx| {
        casify_region_in_state(ctx, args, "downcase-region", Some(false), |ctx, s, lower| {
            downcase_buffer_text(ctx, s, false, lower.expect("prepared lower"))
        })
    })
}

pub(crate) fn builtin_upcase_region(
    ctx: &mut super::eval::Context,
    args: Vec<Value>,
) -> EvalResult {
    let casetab = CaseTableOverride::for_current_buffer(ctx)?;
    casify_region_in_state(ctx, args, "upcase-region", None, move |_, s, _| {
        Ok((upcase_lisp_string_emacs_compat(s, &casetab, CaseTarget::Buffer), None))
    })
}

pub(crate) fn builtin_capitalize_region(
    ctx: &mut super::eval::Context,
    args: Vec<Value>,
) -> EvalResult {
    ctx.with_specpdl_roots(&args.clone(), |ctx| {
        casify_region_in_state(ctx, args, "capitalize-region", Some(true), |ctx, s, lower| {
            downcase_buffer_text(ctx, s, true, lower.expect("prepared lower"))
        })
    })
}

pub(crate) fn builtin_upcase_initials_region(
    ctx: &mut super::eval::Context,
    args: Vec<Value>,
) -> EvalResult {
    let is_word = crate::emacs_core::syntax::casing_word_predicate(ctx);
    let is_prefix = crate::emacs_core::syntax::casing_prefix_predicate(ctx);
    let casetab = CaseTableOverride::for_current_buffer(ctx)?;
    casify_region_in_state(ctx, args, "upcase-initials-region", None, move |_, s, _| {
        Ok((capitalize_like_gnu(
            s,
            is_word,
            is_prefix,
            &casetab,
            WordRest::Keep,
            CaseTarget::Buffer,
            None,
        ), None))
    })
}

pub(crate) fn builtin_downcase_word(
    ctx: &mut super::eval::Context,
    args: Vec<Value>,
) -> EvalResult {
    ctx.with_specpdl_roots(&args.clone(), |ctx| {
        casify_word_in_state(ctx, args, "downcase-word", Some(false), |ctx, s, lower| {
            downcase_buffer_text(ctx, s, false, lower.expect("prepared lower"))
        })
    })
}

pub(crate) fn builtin_upcase_word(ctx: &mut super::eval::Context, args: Vec<Value>) -> EvalResult {
    let casetab = CaseTableOverride::for_current_buffer(ctx)?;
    casify_word_in_state(ctx, args, "upcase-word", None, move |_, s, _| {
        Ok((upcase_lisp_string_emacs_compat(s, &casetab, CaseTarget::Buffer), None))
    })
}

pub(crate) fn builtin_capitalize_word(
    ctx: &mut super::eval::Context,
    args: Vec<Value>,
) -> EvalResult {
    ctx.with_specpdl_roots(&args.clone(), |ctx| {
        casify_word_in_state(ctx, args, "capitalize-word", Some(true), |ctx, s, lower| {
            downcase_buffer_text(ctx, s, true, lower.expect("prepared lower"))
        })
    })
}

/// `(char-resolve-modifiers CHAR)` -- resolve modifier bits in character.
/// Resolve shift/control modifiers into the base character where possible.
pub(crate) fn builtin_char_resolve_modifiers(args: Vec<Value>) -> EvalResult {
    expect_args("char-resolve-modifiers", &args, 1)?;

    let code = match args[0].kind() {
        ValueKind::Fixnum(n) => n,
        _other => {
            return Err(signal(
                LispCondition::WrongTypeArgument,
                vec![Value::symbol("fixnump"), args[0]],
            ));
        }
    };

    Ok(Value::fixnum(
        crate::emacs_core::emacs_char::char_resolve_modifier_mask(code),
    ))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
#[path = "tests/casefiddle_test.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/r014_unibyte.rs"]
mod r014_unibyte_tests;

#[cfg(test)]
#[path = "tests/r017_special_up.rs"]
mod r017_special_up_tests;

#[cfg(test)]
#[path = "tests/r018_nil_up.rs"]
mod r018_nil_up_tests;
