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

/// Uppercase a single character code, returning the new code.
fn upcase_char(code: i64) -> i64 {
    upcase_char_with_unicode(code, || match code_to_char(code) {
        Some(c) => c.to_uppercase().next().map(|u| u as i64).unwrap_or(code),
        None => code,
    })
}

/// Resolve GNU's overrides before evaluating the Unicode fallback. A caller
/// already resolving a Unicode expansion can supply its existing scalar.
#[inline(always)]
fn upcase_char_with_unicode(code: i64, unicode: impl FnOnce() -> i64) -> i64 {
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
    unicode()
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

#[inline]
fn push_multibyte_char_code(out: &mut Vec<u8>, code: u32) {
    if code < 0x80 {
        out.push(code as u8);
        return;
    }
    let mut buf = [0u8; crate::emacs_core::emacs_char::MAX_MULTIBYTE_LENGTH];
    let len = crate::emacs_core::emacs_char::char_string(code, &mut buf);
    out.extend_from_slice(&buf[..len]);
}

/// Word-boundary predicate over the *standard* syntax table, for the pure/test
/// casing forms that run without a current buffer. Mirrors GNU's `Sword` test
/// against the standard syntax table.
#[allow(dead_code)] // grandfathered when dead_code lint was enabled; delete or wire up
fn standard_word_predicate(code: u32) -> bool {
    crate::emacs_core::syntax::standard_syntax_class_for_code(code)
        == crate::emacs_core::syntax::SyntaxClass::Word
}

/// GNU's four casing operations. The enum keeps titlecasing and uppercasing
/// distinct, and prevents callers from combining incompatible mode flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaseAction {
    Up,
    Down,
    Capitalize,
    Initials,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CharacterCaseAction {
    Up,
    Down,
    Title,
    Unchanged,
}

/// Call-local word context decides whether capitalization uses an initial.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CasingWordState {
    Outside,
    Inside,
}

/// Buffer encoding determines whether integer characters 128..255 denote
/// Latin-1 or raw bytes. This call-local projection contains no shared state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaseEncoding {
    Unibyte,
    Multibyte,
}

impl CaseEncoding {
    pub(crate) fn for_current_buffer(ctx: &super::eval::Context) -> Self {
        if ctx
            .buffers
            .current_buffer()
            .is_some_and(|buf| !buf.get_multibyte())
        {
            Self::Unibyte
        } else {
            Self::Multibyte
        }
    }
}

/// Standard upcasing cannot change a capital sigma, so no syntax lookup is
/// needed. A custom table may change it and requires GNU's word context.
pub(crate) fn upcase_word_predicate(
    ctx: &super::eval::Context,
    casetab: &CaseTableOverride,
) -> impl Fn(u32) -> bool + Copy + 'static {
    let word = casetab
        .is_custom()
        .then(|| crate::emacs_core::syntax::casing_word_predicate(ctx));
    move |code| word.is_some_and(|word| word(code))
}

impl CaseAction {
    fn character_action(self, word_state: CasingWordState) -> CharacterCaseAction {
        match self {
            Self::Up => CharacterCaseAction::Up,
            Self::Down => CharacterCaseAction::Down,
            Self::Capitalize => match word_state {
                CasingWordState::Inside => CharacterCaseAction::Down,
                CasingWordState::Outside => CharacterCaseAction::Title,
            },
            Self::Initials => match word_state {
                CasingWordState::Inside => CharacterCaseAction::Unchanged,
                CasingWordState::Outside => CharacterCaseAction::Title,
            },
        }
    }
}

fn simple_case_character(
    code: i64,
    action: CharacterCaseAction,
    casetab: &CaseTableOverride,
) -> i64 {
    match action {
        CharacterCaseAction::Up => casetab
            .map(CaseMap::Up, code)
            .unwrap_or_else(|| super::builtins::upcase_char_code_emacs_compat(code)),
        CharacterCaseAction::Down => casetab
            .map(CaseMap::Down, code)
            .unwrap_or_else(|| super::builtins::downcase_char_code_emacs_compat(code)),
        CharacterCaseAction::Title => {
            // Unicode's simple titlecase property precedes the buffer table
            // (GNU casefiddle.c:166-180), including titlecase digraphs.
            let title = upcase_char(code);
            let simple_title = matches!(code, 452..=460 | 497..=499 | 4304..=4346 | 4349..=4351 | 8064..=8071 | 8080..=8087 | 8096..=8103 | 8115 | 8131 | 8179)
                || (title != code
                    && code_to_char(code).is_some_and(|ch| ch.to_uppercase().count() == 1));
            if simple_title {
                title
            } else {
                upcase_char_override(code, casetab)
            }
        }
        CharacterCaseAction::Unchanged => code,
    }
}

/// A validated nonnegative fixnum event. GNU first projects it to the C int
/// event domain; modifiers are restored after casing. Out-of-domain projections
/// and characters whose case is unchanged retain the original fixnum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CaseNatnum(i64);

#[derive(Debug, thiserror::Error)]
pub(crate) enum CaseNatnumError {
    #[error("Casing input must be a nonnegative fixnum")]
    OutsideFixnumDomain,
}

impl TryFrom<i64> for CaseNatnum {
    type Error = CaseNatnumError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        (0..=Value::MOST_POSITIVE_FIXNUM)
            .contains(&value)
            .then_some(Self(value))
            .ok_or(CaseNatnumError::OutsideFixnumDomain)
    }
}

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct CaseModifiers: i32 {
        const ALT = CHAR_ALT as i32;
        const SUPER = CHAR_SUPER as i32;
        const HYPER = CHAR_HYPER as i32;
        const SHIFT = CHAR_SHIFT as i32;
        const CONTROL = CHAR_CTL as i32;
        const META = CHAR_META as i32;
    }
}

const _: () = assert!(CaseModifiers::all().bits() as i64 == CHAR_MODIFIER_MASK);

impl CaseNatnum {
    pub(crate) fn casify(
        self,
        action: CaseAction,
        encoding: CaseEncoding,
        casetab: &CaseTableOverride,
    ) -> i64 {
        // GNU do_casify_natnum (casefiddle.c:248-277).
        let event = self.0 as i32 as i64;
        if !(0..=CHAR_MODIFIER_MASK).contains(&event) {
            return self.0;
        }
        let flags = CaseModifiers::from_bits_truncate(event as i32);
        let code = event & !(CaseModifiers::all().bits() as i64);
        let raw_byte = matches!(encoding, CaseEncoding::Unibyte) && code < 256;
        let source = if raw_byte {
            super::emacs_char::unibyte_to_char(code as u8) as i64
        } else {
            code
        };
        let mapped = simple_case_character(
            source,
            action.character_action(CasingWordState::Outside),
            casetab,
        );
        if mapped == source {
            self.0
        } else {
            (if raw_byte { mapped & 0xff } else { mapped }) | flags.bits() as i64
        }
    }
}

/// Measured output for exactly one source character. Character and byte units
/// cannot be confused. Values live only during one mutator's casing call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CaseExtent {
    source_pos: crate::buffer::CharPos0,
    source_extent: crate::buffer::TextExtent,
    output_extent: crate::buffer::TextExtent,
}

impl CaseExtent {
    pub(crate) fn source_pos(self) -> crate::buffer::CharPos0 {
        self.source_pos
    }
    pub(crate) fn source_extent(self) -> crate::buffer::TextExtent {
        self.source_extent
    }
    pub(crate) fn output_extent(self) -> crate::buffer::TextExtent {
        self.output_extent
    }
}

/// GNU applies ASCII fallback only to unibyte strings, while buffer casing
/// writes the cased byte directly. The target makes callers choose that policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaseTarget {
    String,
    Buffer,
}

#[inline]
pub(crate) fn casify_lisp_string_with_extents(
    text: &LispString,
    action: CaseAction,
    is_word: impl Fn(u32) -> bool,
    casetab: &CaseTableOverride,
    extent: impl FnMut(CaseExtent),
) -> LispString {
    casify_text_with_extents(text, action, CaseTarget::String, is_word, casetab, extent)
}

/// GNU's unibyte loops case one byte at a time (casefiddle.c:330-345,451-460).
/// Standard tables change only ASCII, so string and buffer targets agree and
/// every source/output extent is one character and one byte. Raw bytes still
/// enter the syntax predicate in their Emacs character domain for word casing.
#[inline]
fn casify_standard_unibyte_with_extents(
    bytes: &[u8],
    action: CaseAction,
    is_word: impl Fn(u32) -> bool,
    mut extent: impl FnMut(CaseExtent),
) -> LispString {
    let mut out = bytes.to_vec();
    let one_byte = crate::buffer::TextExtent::new(
        crate::buffer::CharLen::new(1),
        crate::buffer::EmacsByteLen::new(1),
    );
    let mut report_extent = |source_index| {
        extent(CaseExtent {
            source_pos: crate::buffer::CharPos0::new(source_index),
            source_extent: one_byte,
            output_extent: one_byte,
        });
    };
    match action {
        CaseAction::Up => {
            out.make_ascii_uppercase();
            for source_index in 0..out.len() {
                report_extent(source_index);
            }
        }
        CaseAction::Down => {
            out.make_ascii_lowercase();
            for source_index in 0..out.len() {
                report_extent(source_index);
            }
        }
        CaseAction::Capitalize | CaseAction::Initials => {
            let mut word_state = CasingWordState::Outside;
            for (source_index, byte) in out.iter_mut().enumerate() {
                let code = super::emacs_char::unibyte_to_char(*byte);
                *byte = match action.character_action(word_state) {
                    CharacterCaseAction::Up | CharacterCaseAction::Title => {
                        byte.to_ascii_uppercase()
                    }
                    CharacterCaseAction::Down => byte.to_ascii_lowercase(),
                    CharacterCaseAction::Unchanged => *byte,
                };
                report_extent(source_index);
                word_state = if is_word(code) {
                    CasingWordState::Inside
                } else {
                    CasingWordState::Outside
                };
            }
        }
    }
    LispString::from_unibyte(out)
}

/// Casing with an optional monomorphized extent sink. The no-op sink used by
/// string builtins compiles away; buffer edits retain both coordinate units.
/// GNU casefiddle.c:137-151 resolves Unicode special casing before case tables;
/// 221-237 then applies contextual final sigma to a changed capital sigma.
#[inline]
pub(crate) fn casify_text_with_extents(
    text: &LispString,
    action: CaseAction,
    target: CaseTarget,
    is_word: impl Fn(u32) -> bool,
    casetab: &CaseTableOverride,
    extent: impl FnMut(CaseExtent),
) -> LispString {
    casify_text_with_context(
        text,
        action,
        target,
        is_word,
        casetab,
        extent,
        None,
        |_| false,
        None,
    )
}

fn casify_text_with_context(
    text: &LispString,
    action: CaseAction,
    target: CaseTarget,
    is_word: impl Fn(u32) -> bool,
    casetab: &CaseTableOverride,
    mut extent: impl FnMut(CaseExtent),
    special_lowercase: Option<Value>,
    is_prefix: impl Fn(u32) -> bool,
    mut span: Option<&mut CaseChangeSpan>,
) -> LispString {
    let multibyte = text.is_multibyte();
    let bytes = text.as_bytes();
    if !multibyte && !casetab.is_custom() && target == CaseTarget::String {
        return casify_standard_unibyte_with_extents(bytes, action, is_word, extent);
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut pos = 0;
    let mut source_index = 0;
    let mut word_state = CasingWordState::Outside;
    while pos < bytes.len() {
        let (code, len) = if multibyte {
            super::emacs_char::string_char(&bytes[pos..])
        } else {
            (super::emacs_char::unibyte_to_char(bytes[pos]), 1)
        };
        pos += len;
        let char_action = action.character_action(word_state);
        if multibyte && matches!(char_action, CharacterCaseAction::Unchanged) {
            // GNU casefiddle.c:130-137 leaves in-word initials unchanged.
            // Copy the canonical source unit without Unicode remapping or
            // re-encoding, while retaining original syntax and typed extents.
            let output_start = out.len();
            out.extend_from_slice(&bytes[pos - len..pos]);
            if let Some(span) = span.as_deref_mut() {
                span.note(code, &out, output_start, false);
            }
            let unit = crate::buffer::TextExtent::new(
                crate::buffer::CharLen::new(1),
                crate::buffer::EmacsByteLen::new(len),
            );
            extent(CaseExtent {
                source_pos: crate::buffer::CharPos0::new(source_index),
                source_extent: unit,
                output_extent: unit,
            });
            source_index += 1;
            word_state = if is_word(code)
                && (word_state == CasingWordState::Inside
                    || target == CaseTarget::String
                    || !is_prefix(code))
            {
                CasingWordState::Inside
            } else {
                CasingWordState::Outside
            };
            continue;
        }
        let was_inword = matches!(word_state, CasingWordState::Inside);
        let next_word = code == 0x03a3
            && was_inword
            && pos < bytes.len()
            && is_word(if multibyte {
                super::emacs_char::string_char(&bytes[pos..]).0
            } else {
                super::emacs_char::unibyte_to_char(bytes[pos])
            });
        let output_start = out.len();
        let mut output_chars = 0;
        let mut emit = |mapped: u32| {
            if multibyte {
                push_multibyte_char_code(&mut out, mapped);
            } else {
                // GNU casefiddle.c:343-344: an ASCII-to-non-byte mapping
                // uses Unicode ASCII casing instead of truncating its code.
                let mapped = if matches!(target, CaseTarget::String)
                    && code < 128
                    && mapped >= 256
                    && !super::emacs_char::char_byte8_p(mapped)
                {
                    match char_action {
                        CharacterCaseAction::Down => (code as u8).to_ascii_lowercase() as u32,
                        CharacterCaseAction::Up | CharacterCaseAction::Title => {
                            (code as u8).to_ascii_uppercase() as u32
                        }
                        CharacterCaseAction::Unchanged => code,
                    }
                } else {
                    mapped
                };
                out.push((mapped & 0xff) as u8);
            }
            output_chars += 1;
        };
        let mut expanded = false;
        let mut special = false;
        if multibyte
            && matches!(char_action, CharacterCaseAction::Down)
            && special_lowercase.is_some()
        {
            let mut mapped = Vec::new();
            special = push_text_downcase(
                &mut mapped,
                code,
                casetab,
                was_inword,
                next_word,
                special_lowercase,
            );
            let mut offset = 0;
            while offset < mapped.len() {
                let (mapped_code, len) = super::emacs_char::string_char(&mapped[offset..]);
                emit(mapped_code);
                offset += len;
            }
            expanded = true;
        } else if multibyte && code < 0x80 && !casetab.is_custom() {
            // GNU's standard Unicode tables map ASCII one-to-one. Keep the
            // word-state decision and extent sink, but avoid Unicode iterator
            // construction for each ASCII source character. Installed tables
            // still take the full path, including string/buffer target policy.
            emit(match char_action {
                CharacterCaseAction::Up | CharacterCaseAction::Title => {
                    (code as u8).to_ascii_uppercase() as u32
                }
                CharacterCaseAction::Down => (code as u8).to_ascii_lowercase() as u32,
                CharacterCaseAction::Unchanged => code,
            });
            expanded = true;
        } else if multibyte && let Some(ch) = char::from_u32(code) {
            match char_action {
                CharacterCaseAction::Up => {
                    let upper = ch.to_uppercase();
                    if !casetab.is_custom() {
                        if code == 0x0131 || preserve_upcase_case_string_payload(code as i64) {
                            emit(code);
                        } else {
                            upper.for_each(|ch| emit(ch as u32));
                        }
                        expanded = true;
                    } else if upper.clone().count() > 1
                        && !preserve_upcase_case_string_payload(code as i64)
                    {
                        upper.for_each(|ch| emit(ch as u32));
                        expanded = true;
                    }
                }
                CharacterCaseAction::Down => {
                    let lower = ch.to_lowercase();
                    if !casetab.is_custom() {
                        if code == 0x03a3 && was_inword && !next_word {
                            emit(0x03c2);
                        } else if code == 0x212a
                            || preserve_downcase_case_string_payload(code as i64)
                        {
                            emit(code);
                        } else {
                            lower.for_each(|ch| emit(ch as u32));
                        }
                        expanded = true;
                    } else if lower.clone().count() > 1
                        && !preserve_downcase_case_string_payload(code as i64)
                    {
                        lower.for_each(|ch| emit(ch as u32));
                        expanded = true;
                    }
                }
                CharacterCaseAction::Title => {
                    let upper = ch.to_uppercase();
                    let upper_count = upper.clone().count();
                    if titlecase_combining_iota_override(code as i64).is_some()
                        || (upper_count > 1 && !titlecase_uses_precomposed_upcase(code as i64))
                    {
                        titlecase_word_initial(ch)
                            .chars()
                            .for_each(|ch| emit(ch as u32));
                        expanded = true;
                    } else {
                        // Reuse this character's Unicode lookup for GNU's
                        // simple titlecase before the installed-table fallback.
                        let code = i64::from(code);
                        let title = upcase_char_with_unicode(code, || {
                            upper.clone().next().map(|ch| ch as i64).unwrap_or(code)
                        });
                        let simple_title = matches!(code,
                            452..=460 | 497..=499 | 4304..=4346 | 4349..=4351 |
                            8064..=8071 | 8080..=8087 | 8096..=8103 | 8115 | 8131 | 8179)
                            || (title != code && upper_count == 1);
                        emit(if simple_title {
                            title
                        } else {
                            upcase_char_override(code, casetab)
                        } as u32);
                        expanded = true;
                    }
                }
                CharacterCaseAction::Unchanged => {}
            }
        }
        if !expanded {
            let mapped = simple_case_character(code as i64, char_action, casetab) as u32;
            emit(
                if multibyte && was_inword && code == 0x03a3 && mapped != code && !next_word {
                    0x03c2
                } else {
                    mapped
                },
            );
        }
        if let Some(span) = span.as_deref_mut() {
            if multibyte {
                span.note(code, &out, output_start, special);
            }
        }
        extent(CaseExtent {
            source_pos: crate::buffer::CharPos0::new(source_index),
            source_extent: crate::buffer::TextExtent::new(
                crate::buffer::CharLen::new(1),
                crate::buffer::EmacsByteLen::new(len),
            ),
            output_extent: crate::buffer::TextExtent::new(
                crate::buffer::CharLen::new(output_chars),
                crate::buffer::EmacsByteLen::new(out.len() - output_start),
            ),
        });
        source_index += 1;
        word_state = match action {
            CaseAction::Up if !casetab.is_custom() => CasingWordState::Outside,
            CaseAction::Down if !multibyte => CasingWordState::Outside,
            CaseAction::Up | CaseAction::Down | CaseAction::Capitalize | CaseAction::Initials => {
                if is_word(code) && (was_inword || target == CaseTarget::String || !is_prefix(code))
                {
                    CasingWordState::Inside
                } else {
                    CasingWordState::Outside
                }
            }
        };
    }
    if multibyte {
        LispString::from_emacs_bytes(out)
    } else {
        LispString::from_unibyte(out)
    }
}

#[inline]
pub(crate) fn casify_lisp_string(
    text: &LispString,
    action: CaseAction,
    is_word: impl Fn(u32) -> bool,
    casetab: &CaseTableOverride,
) -> LispString {
    casify_lisp_string_with_extents(text, action, is_word, casetab, |_| {})
}

fn upcase_lisp_string_emacs_compat(
    text: &LispString,
    casetab: &CaseTableOverride,
    target: CaseTarget,
) -> LispString {
    casify_text_with_extents(text, CaseAction::Up, target, |_| false, casetab, |_| {})
}

fn capitalize_lisp_string(
    text: &LispString,
    is_word: impl Fn(u32) -> bool,
    casetab: &CaseTableOverride,
) -> LispString {
    casify_lisp_string(text, CaseAction::Capitalize, is_word, casetab)
}

pub(crate) fn upcase_initials_lisp_string(
    text: &LispString,
    is_word: impl Fn(u32) -> bool,
    casetab: &CaseTableOverride,
) -> LispString {
    casify_lisp_string(text, CaseAction::Initials, is_word, casetab)
}

fn preserve_downcase_case_string_payload(code: i64) -> bool {
    matches!(
        code,
        7305
            | 42955
            | 42956
            | 42958
            | 42962
            | 42964
            | 42970
            | 42972
            | 68944..=68965
            | 93856..=93880
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

/// A recased region retains the character expansions produced by the same
/// transducer as its text. Call-local typed offsets cannot be mixed with bytes.
/// Its heap-backed text belongs to this mutator and cannot cross threads.
#[derive(Debug)]
struct CasedRegion {
    span: CaseChangeSpan,
    text: LispString,
    expansions: Vec<crate::buffer::CasifyExpansion>,
    storage_shape: crate::buffer::CasifyStorageShape,
    mutator: std::marker::PhantomData<*const ()>,
}

fn casify_region_text(
    text: &LispString,
    action: CaseAction,
    is_word: impl Fn(u32) -> bool,
    casetab: &CaseTableOverride,
    lower: Option<Value>,
    is_prefix: impl Fn(u32) -> bool,
) -> CasedRegion {
    let mut expansions = Vec::new();
    let mut storage_shape = crate::buffer::CasifyStorageShape::PerCharacterExtentPreserved;
    let mut span = CaseChangeSpan::default();
    let original = text;
    let text = casify_text_with_context(
        text,
        action,
        CaseTarget::Buffer,
        is_word,
        casetab,
        |extent| {
            if extent.source_extent() != extent.output_extent() {
                storage_shape = crate::buffer::CasifyStorageShape::Changed;
            }
            if let Some(expansion) = crate::buffer::CasifyExpansion::new(
                extent.source_pos(),
                extent.output_extent().chars(),
            ) {
                expansions.push(expansion);
            }
        },
        lower,
        is_prefix,
        Some(&mut span),
    );
    if !original.is_multibyte() {
        span = CaseChangeSpan::from_byte_diff(original.as_bytes(), text.as_bytes());
    }
    CasedRegion {
        text,
        expansions,
        storage_shape,
        span,
        mutator: std::marker::PhantomData,
    }
}

fn casify_region_in_state(
    eval: &mut super::eval::Context,
    args: Vec<Value>,
    name: &str,
    action: CaseAction,
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
        casify_replace_current_buffer_region(eval, region, action)?;
    }
    Ok(Value::NIL)
}

/// Run GNU's modification preparation before reading the casing context or
/// source. Character arguments survive hooks; byte positions and per-character
/// replacement extents are measured together afterwards, without Lisp callbacks
/// between measurement and storage execution (casefiddle.c:528-583).
fn casify_replace_current_buffer_region(
    eval: &mut super::eval::Context,
    region: super::position::LispRegionArgs,
    action: CaseAction,
) -> Result<crate::buffer::CharPos0, Flow> {
    let (buffer_id, byte_range) = {
        let buf = eval.buffers.current_buffer().ok_or_else(|| {
            signal(
                LispCondition::Error,
                vec![Value::string("No current buffer")],
            )
        })?;
        let byte_range = region.accessible_byte_range(buf)?;
        if byte_range.is_empty() {
            return Ok(buf
                .emacs_byte_pos_to_lisp_char_pos(byte_range.end())
                .to_char_pos());
        }
        if super::editfns::buffer_read_only_active_in_state(&eval.obarray, &[], buf) {
            return Err(signal(
                LispCondition::BufferReadOnly,
                vec![buf.name_value()],
            ));
        }
        (buf.id, byte_range)
    };
    eval.with_specpdl_roots(&[Value::make_buffer(buffer_id)], |eval| {
        crate::emacs_core::textprop::verify_text_read_only_emacs_byte_range_in_state(
            &eval.obarray,
            &eval.buffers,
            buffer_id,
            byte_range,
        )?;
        let before = super::editfns::text_change_for_unchanged_extent_in_manager(
            &eval.buffers,
            buffer_id,
            byte_range,
        )?;
        // GNU modify_text precedes prepare_casing_context (casefiddle.c:540-541).
        // A before-change hook can change text, encoding, syntax and case tables.
        super::editfns::signal_before_text_change(eval, before)?;
        eval.set_current_buffer_unrecorded(buffer_id)?;
        let apply = |eval: &mut super::eval::Context, lower: Option<Value>| {
            let (buffer_id, byte_range, text) = {
                let buf = eval.buffers.current_buffer().ok_or_else(|| {
                    signal(
                        LispCondition::Error,
                        vec![Value::string("No current buffer")],
                    )
                })?;
                let chars = before.old_range().char_range();
                if chars.end() > buf.total_char_end_pos() {
                    return Err(signal(
                        LispCondition::ArgsOutOfRange,
                        vec![
                            Value::make_buffer(buf.id),
                            Value::fixnum(chars.start().to_lisp().as_i64()),
                            Value::fixnum(chars.end().to_lisp().as_i64()),
                        ],
                    ));
                }
                // GNU validates the original accessible region only before hooks.
                // CHAR_TO_BYTE and make_buffer_string then use physical bounds even
                // if the hook narrows the end (casefiddle.c:534-557; editfns.c:1561-1566).
                // Its property copy still validates the original starting position
                // (editfns.c:1620-1624), so narrowing past start signals before editing.
                if chars.start() < buf.point_min_char_pos()
                    || chars.start() > buf.point_max_char_pos()
                {
                    crate::emacs_core::textprop::builtin_text_properties_at_in_buffers(
                        &eval.buffers,
                        &[Value::fixnum(chars.start().to_lisp().as_i64())],
                    )?;
                }
                let byte_range = buf.edit_range_for_char_range(chars).byte_range();
                (
                    buf.id,
                    byte_range,
                    buf.buffer_substring_lisp_string_range(byte_range),
                )
            };
            let casetab = CaseTableOverride::for_current_buffer(eval)?;
            let is_prefix = crate::emacs_core::syntax::casing_prefix_predicate(eval);
            let replacement = match action {
                CaseAction::Up => {
                    let is_word = upcase_word_predicate(eval, &casetab);
                    casify_region_text(&text, action, is_word, &casetab, lower, is_prefix)
                }
                CaseAction::Down | CaseAction::Capitalize | CaseAction::Initials => {
                    let is_word = crate::emacs_core::syntax::casing_word_predicate(eval);
                    casify_region_text(&text, action, is_word, &casetab, lower, is_prefix)
                }
            };
            let change = super::editfns::text_change_for_lisp_string_replacement_in_manager(
                &eval.buffers,
                buffer_id,
                byte_range,
                &replacement.text,
            )?;
            let end = change
                .old_range()
                .char_start()
                .add_len(change.new_extent().chars());
            // GNU records casing undo even for an unchanged nonempty region. The
            // before hook runs for that case, but the after hook requires changed text.
            // GNU sticky inheritance controls are read only after modification hooks.
            // Plain/no-expansion casing needs no policy lookup or interval allocation.
            let after = replacement
                .span
                .first
                .map(|first| {
                    let old = EmacsByteRange::new(
                        EmacsBytePos::new(byte_range.start().get() + first),
                        EmacsBytePos::new(byte_range.start().get() + replacement.span.old_end),
                    );
                    let new = crate::buffer::TextExtent::from_emacs_bytes(
                        &replacement.text.as_bytes()[first..replacement.span.new_end],
                        text.is_multibyte(),
                    );
                    super::editfns::text_change_for_replacement_in_manager(
                        &eval.buffers,
                        buffer_id,
                        old,
                        new,
                    )
                })
                .transpose()?;
            let controls = (!replacement.expansions.is_empty()
                && eval
                    .buffers
                    .get(buffer_id)
                    .is_some_and(|buf| !buf.text_props_is_empty()))
            .then(|| crate::buffer::text_props::CasingPropertyControls::for_context(eval));
            let properties = crate::buffer::text_props::CasingPropertyMode::from_controls(
                &eval.obarray,
                controls,
            );
            eval.buffers.casify_replace_buffer_region_with_expansions(
                buffer_id,
                byte_range,
                &replacement.text,
                &replacement.expansions,
                replacement.storage_shape,
                &properties,
            )?;
            if let Some(after) = after {
                super::editfns::signal_after_text_change(eval, after)?;
            }
            Ok(end)
        };
        match action {
            CaseAction::Down | CaseAction::Capitalize => {
                with_text_downcase_table(eval, action == CaseAction::Capitalize, |eval, lower| {
                    apply(eval, Some(lower))
                })
            }
            CaseAction::Up | CaseAction::Initials => apply(eval, None),
        }
    })
}

fn casify_word_in_state(
    eval: &mut super::eval::Context,
    args: Vec<Value>,
    name: &str,
    action: CaseAction,
) -> EvalResult {
    expect_args(name, &args, 1)?;
    let n = expect_int(&args[0])?;
    // GNU scans words before modification preparation (casefiddle.c:661-668).
    let honor = crate::emacs_core::syntax::parse_sexp_lookup_properties_enabled(eval);
    let target = crate::emacs_core::syntax::forward_word_destination(eval, n, honor);
    let (start, end) = {
        let buf = eval.buffers.current_buffer().ok_or_else(|| {
            signal(
                LispCondition::Error,
                vec![Value::string("No current buffer")],
            )
        })?;
        (
            buf.point_lisp_char_pos(),
            buf.emacs_byte_pos_to_lisp_char_pos(target),
        )
    };
    let region = super::position::LispRegionArgs::from_values(
        &eval.buffers,
        Value::fixnum(start.as_i64()),
        Value::fixnum(end.as_i64()),
    )?;
    let end = casify_replace_current_buffer_region(eval, region, action)?;
    // GNU SET_PT converts casify_region's character endpoint after the after
    // hook, which can change byte widths again (casefiddle.c:668-669). Negative
    // word arguments retain the greater endpoint, as region validation orders it.
    if let Some(buf) = eval.buffers.current_buffer() {
        let id = buf.id;
        let point = buf.char_pos_to_emacs_byte_pos_clamped(end);
        let _ = eval.buffers.goto_buffer_emacs_byte_pos(id, point);
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
    encoding: CaseEncoding,
    lower: Option<Value>,
) -> EvalResult {
    expect_args("capitalize", &args, 1)?;
    match args[0].kind() {
        ValueKind::String => {
            let source = args[0];
            let string = args[0].as_lisp_string().expect("string");
            let source_props = (!string.is_multibyte())
                .then(|| get_string_text_properties_table_for_value(source))
                .flatten();
            let result = Value::heap_string(capitalize_like_gnu(
                string,
                is_word,
                |_| false,
                casetab,
                WordRest::Downcase,
                CaseTarget::String,
                lower,
            ));
            if let Some(table) = source_props {
                set_string_text_properties_table_for_value(result, table);
            }
            Ok(result)
        }
        ValueKind::Fixnum(c) if c >= 0 => {
            let code = CaseNatnum::try_from(c).map_err(|_| {
                signal(
                    LispCondition::WrongTypeArgument,
                    vec![Value::symbol("char-or-string-p"), args[0]],
                )
            })?;
            Ok(Value::fixnum(code.casify(
                CaseAction::Capitalize,
                encoding,
                casetab,
            )))
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
    capitalize_with_word_pred(
        args,
        standard_word_predicate,
        &CaseTableOverride::none(),
        CaseEncoding::Multibyte,
        None,
    )
}

/// Dispatched form: word boundaries follow the current buffer's syntax table
/// (honoring `case-symbols-as-words` and any `set-case-syntax-pair` word
/// syntax), and case mapping follows the current buffer's case table.
pub(crate) fn builtin_capitalize_in_state(
    eval: &mut super::eval::Context,
    args: Vec<Value>,
) -> EvalResult {
    expect_args("capitalize", &args, 1)?;
    if let ValueKind::Fixnum(code) = args[0].kind() {
        return titlecase_character_in_state(eval, code);
    }
    eval.with_specpdl_roots(&args.clone(), |eval| {
        with_text_downcase_table(eval, true, |eval, lower| {
            let is_word = crate::emacs_core::syntax::casing_word_predicate(eval);
            let casetab = CaseTableOverride::for_current_buffer(eval)?;
            capitalize_with_word_pred(
                args,
                is_word,
                &casetab,
                CaseEncoding::for_current_buffer(eval),
                Some(lower),
            )
        })
    })
}

/// `(upcase-initials OBJ)` -- uppercase the first letter of each word in
/// a string, leaving the rest unchanged.  For a char, uppercase it.
fn upcase_initials_with_word_pred(
    args: Vec<Value>,
    is_word: impl Fn(u32) -> bool,
    casetab: &CaseTableOverride,
    encoding: CaseEncoding,
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
        ValueKind::Fixnum(c) if c >= 0 => {
            let code = CaseNatnum::try_from(c).map_err(|_| {
                signal(
                    LispCondition::WrongTypeArgument,
                    vec![Value::symbol("char-or-string-p"), args[0]],
                )
            })?;
            Ok(Value::fixnum(code.casify(
                CaseAction::Capitalize,
                encoding,
                casetab,
            )))
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
    upcase_initials_with_word_pred(
        args,
        standard_word_predicate,
        &CaseTableOverride::none(),
        CaseEncoding::Multibyte,
    )
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
    upcase_initials_with_word_pred(
        args,
        is_word,
        &casetab,
        CaseEncoding::for_current_buffer(eval),
    )
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
    // GNU Freplace_match initializes prevc to newline, whose syntax can be
    // a word constituent even when the match contains only one character.
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
    // GNU Freplace_match initializes prevc to newline, whose syntax can be
    // a word constituent even when the match contains only one character.
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

pub(crate) fn builtin_downcase_region(
    ctx: &mut super::eval::Context,
    args: Vec<Value>,
) -> EvalResult {
    casify_region_in_state(ctx, args, "downcase-region", CaseAction::Down)
}

pub(crate) fn builtin_upcase_region(
    ctx: &mut super::eval::Context,
    args: Vec<Value>,
) -> EvalResult {
    casify_region_in_state(ctx, args, "upcase-region", CaseAction::Up)
}

pub(crate) fn builtin_capitalize_region(
    ctx: &mut super::eval::Context,
    args: Vec<Value>,
) -> EvalResult {
    casify_region_in_state(ctx, args, "capitalize-region", CaseAction::Capitalize)
}

pub(crate) fn builtin_upcase_initials_region(
    ctx: &mut super::eval::Context,
    args: Vec<Value>,
) -> EvalResult {
    casify_region_in_state(ctx, args, "upcase-initials-region", CaseAction::Initials)
}

pub(crate) fn builtin_downcase_word(
    ctx: &mut super::eval::Context,
    args: Vec<Value>,
) -> EvalResult {
    casify_word_in_state(ctx, args, "downcase-word", CaseAction::Down)
}

pub(crate) fn builtin_upcase_word(ctx: &mut super::eval::Context, args: Vec<Value>) -> EvalResult {
    casify_word_in_state(ctx, args, "upcase-word", CaseAction::Up)
}

pub(crate) fn builtin_capitalize_word(
    ctx: &mut super::eval::Context,
    args: Vec<Value>,
) -> EvalResult {
    casify_word_in_state(ctx, args, "capitalize-word", CaseAction::Capitalize)
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
#[path = "tests/casing_types.rs"]
mod casing_types;

fn push_multibyte_chars(out: &mut Vec<u8>, chars: impl IntoIterator<Item = char>) {
    for ch in chars {
        push_multibyte_char_code(out, ch as u32);
    }
}

fn titlecase_character_in_state(eval: &mut super::eval::Context, code: i64) -> EvalResult {
    let natnum = CaseNatnum::try_from(code).map_err(|_| {
        signal(
            LispCondition::WrongTypeArgument,
            vec![Value::symbol("char-or-string-p"), Value::fixnum(code)],
        )
    })?;
    let encoding = CaseEncoding::for_current_buffer(eval);
    if !(0..=super::emacs_char::MAX_CHAR as i64).contains(&code)
        || (encoding == CaseEncoding::Unibyte && code < 256)
    {
        let casetab = CaseTableOverride::for_current_buffer(eval)?;
        return Ok(Value::fixnum(natnum.casify(
            CaseAction::Capitalize,
            encoding,
            &casetab,
        )));
    }
    let title_table = super::chartable::uniprop_table_in_state(eval, Value::symbol("titlecase"))?;
    let roots = eval.save_specpdl_roots();
    if !title_table.is_nil() {
        eval.push_specpdl_root(title_table);
    }
    let result = (|| {
        // `prepare_casing_context` loads these even for character operations.
        // A callback on the final property can replace the buffer table or Up.
        for property in [
            "special-uppercase",
            "special-lowercase",
            "special-titlecase",
        ] {
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

pub(crate) fn with_text_downcase_table<T>(
    eval: &mut super::eval::Context,
    capitalize: bool,
    body: impl FnOnce(&mut super::eval::Context, Value) -> Result<T, Flow>,
) -> Result<T, Flow> {
    let roots = eval.save_specpdl_roots();
    let result = (|| {
        if capitalize {
            for property in ["titlecase", "special-uppercase"] {
                let table =
                    super::chartable::uniprop_table_in_state(eval, Value::symbol(property))?;
                eval.push_specpdl_root(table);
            }
        }
        let lower =
            super::chartable::uniprop_table_in_state(eval, Value::symbol("special-lowercase"))?;
        eval.push_specpdl_root(lower);
        if capitalize {
            let table =
                super::chartable::uniprop_table_in_state(eval, Value::symbol("special-titlecase"))?;
            eval.push_specpdl_root(table);
        }
        body(eval, lower)
    })();
    eval.restore_specpdl_roots(roots);
    result
}

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
    let mapped = casetab
        .map(CaseMap::Down, code as i64)
        .unwrap_or_else(|| super::builtins::downcase_char_code_emacs_compat(code as i64))
        as u32;
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
    downcase_lisp_string_tracked(
        text,
        is_word,
        is_prefix,
        casetab,
        target,
        special_lowercase,
        None,
    )
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
        let special = push_text_downcase(
            &mut out,
            code,
            casetab,
            prev_word,
            next_word,
            special_lowercase,
        );
        if let Some(span) = span.as_deref_mut() {
            span.note(code, &out, start, special);
        }
        prev_word =
            is_word(code) && (prev_word || target == CaseTarget::String || !is_prefix(code));
    }
    LispString::from_emacs_bytes(out)
}

pub(crate) fn special_upcase_expansion(code: u32) -> Option<std::char::ToUppercase> {
    let expansion = char::from_u32(code)?.to_uppercase();
    expansion.clone().nth(1).is_some().then_some(expansion)
}

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

fn capitalize_like_gnu(
    text: &LispString,
    is_word: impl Fn(u32) -> bool,
    is_prefix: impl Fn(u32) -> bool,
    casetab: &CaseTableOverride,
    rest: WordRest,
    target: CaseTarget,
    special_lowercase: Option<Value>,
) -> LispString {
    capitalize_like_gnu_tracked(
        text,
        is_word,
        is_prefix,
        casetab,
        rest,
        target,
        special_lowercase,
        None,
    )
}

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
            special = push_text_downcase(
                &mut out,
                code,
                casetab,
                was_inword,
                next_word,
                special_lowercase,
            );
        } else {
            push_multibyte_char_code(&mut out, code);
        }
        if let Some(span) = span.as_deref_mut() {
            span.note(code, &out, start, special);
        }
    }
    LispString::from_emacs_bytes(out)
}

fn make_char_unibyte(code: i64) -> u8 {
    let code = code as u32;
    if crate::emacs_core::emacs_char::char_byte8_p(code) {
        crate::emacs_core::emacs_char::char_to_byte8(code)
    } else {
        (code & 0xFF) as u8
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
            return Self {
                old_pos: old.len(),
                first: Some(0),
                old_end: old.len(),
                new_end: new.len(),
            };
        }
        let first = old.iter().zip(new).position(|(a, b)| a != b);
        let last = old
            .iter()
            .zip(new)
            .rposition(|(a, b)| a != b)
            .map_or(0, |i| i + 1);
        Self {
            old_pos: old.len(),
            first,
            old_end: last,
            new_end: last,
        }
    }
}

/// What [`capitalize_like_gnu`] does to a character inside a word.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WordRest {
    /// `capitalize`: down-case it.
    Downcase,
    /// `upcase-initials`: leave it alone.
    Keep,
}

#[cfg(test)]
#[path = "tests/r014_unibyte.rs"]
mod r014_unibyte_tests;

#[cfg(test)]
#[path = "tests/r017_special_up.rs"]
mod r017_special_up_tests;

#[cfg(test)]
#[path = "tests/r018_nil_up.rs"]
mod r018_nil_up_tests;
