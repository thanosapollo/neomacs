//! Lisp bridge for compositor-owned neo-term terminal instances.
//!
//! The evaluator validates Lisp values and sends typed requests through
//! [`DisplayHost`]. PTY ownership, VT parsing, and rendering remain entirely
//! behind the display-runtime boundary.

mod subrs;

#[cfg(test)]
pub(crate) use subrs::SUBRS;
pub(crate) use subrs::register_subrs;

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;

use crate::emacs_core::display_host::{
    DisplayHost, TerminalCreateRequest, TerminalDisplayTarget, TerminalFloatPlacement,
    TerminalGridSize, TerminalId,
};
use crate::emacs_core::error::{EvalResult, Flow, signal};
use crate::emacs_core::eval::Context;
use crate::emacs_core::value::Value;
use std::fmt::{Display, Formatter};
use std::num::NonZeroU16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminalOperation {
    Create,
    SetPalette,
    Spawn,
    Write,
    Resize,
    Destroy,
    SetFloat,
    GetText,
}

impl Display for TerminalOperation {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Create => "neomacs-terminal-create",
            Self::SetPalette => "neomacs-terminal-set-palette",
            Self::Spawn => "neomacs-terminal-spawn",
            Self::Write => "neomacs-terminal-write",
            Self::Resize => "neomacs-terminal-resize",
            Self::Destroy => "neomacs-terminal-destroy",
            Self::SetFloat => "neomacs-terminal-set-float",
            Self::GetText => "neomacs-terminal-get-text",
        })
    }
}

fn terminal_error(message: impl Into<String>) -> Flow {
    signal("error", vec![Value::string(message.into())])
}

fn wrong_type(predicate: &str, value: Value) -> Flow {
    signal("wrong-type-argument", vec![Value::symbol(predicate), value])
}

fn positive_u16(
    value: Value,
    operation: TerminalOperation,
    argument: &str,
) -> Result<NonZeroU16, Flow> {
    let integer = value.as_int().ok_or_else(|| wrong_type("fixnump", value))?;
    u16::try_from(integer)
        .ok()
        .and_then(NonZeroU16::new)
        .ok_or_else(|| terminal_error(format!("{operation}: {argument} must be in 1..=65535")))
}

fn terminal_id(value: Value, operation: TerminalOperation) -> Result<TerminalId, Flow> {
    let integer = value.as_int().ok_or_else(|| wrong_type("fixnump", value))?;
    u32::try_from(integer)
        .ok()
        .and_then(TerminalId::new)
        .ok_or_else(|| terminal_error(format!("{operation}: terminal id must be positive")))
}

fn number(value: Value, operation: TerminalOperation, argument: &str) -> Result<f32, Flow> {
    let number = value
        .as_int()
        .map(|value| value as f32)
        .or_else(|| value.as_float().map(|value| value as f32))
        .ok_or_else(|| wrong_type("numberp", value))?;
    if number.is_finite() {
        Ok(number)
    } else {
        Err(terminal_error(format!(
            "{operation}: {argument} must be finite"
        )))
    }
}

fn display_host(eval: &Context, operation: TerminalOperation) -> Result<&dyn DisplayHost, Flow> {
    eval.display_host
        .as_deref()
        .ok_or_else(|| terminal_error(format!("{operation}: no GUI display host in this session")))
}

/// `(neomacs-terminal-set-palette FRAME PALETTE)`.
/// Publish a complete palette for FRAME; nil retires it during frame deletion.
fn set_palette(eval: &mut Context, args: Vec<Value>) -> EvalResult {
    let frame = crate::emacs_core::window_cmds::resolve_frame_id(
        eval,
        args.first(),
        crate::emacs_core::window_cmds::FrameDomain::Live,
    )?;
    let palette = if args[1].is_nil() {
        None
    } else {
        let values = args[1]
            .as_vector_data()
            .ok_or_else(|| wrong_type("vectorp", args[1]))?;
        if values.len() != 106 {
            return Err(terminal_error(
                "terminal palette requires 105 RGB bytes and one boolean",
            ));
        }
        let mut rgb = [0; 105];
        for (slot, value) in rgb.iter_mut().zip(values.iter()) {
            *slot = value
                .as_int()
                .and_then(|n| u8::try_from(n).ok())
                .ok_or_else(|| terminal_error("terminal palette RGB components must be bytes"))?;
        }
        if values[105] != Value::T && !values[105].is_nil() {
            return Err(wrong_type("booleanp", values[105]));
        }
        Some(
            neomacs_display_protocol::neo_term_palette::NeoTermPalette::from_rgb_bytes(
                rgb,
                !values[105].is_nil(),
            ),
        )
    };
    display_host(eval, TerminalOperation::SetPalette)?
        .set_terminal_palette(frame, palette)
        .map_err(terminal_error)?;
    Ok(Value::NIL)
}

/// `(neomacs-terminal-create COLS ROWS MODE &optional SHELL)`.
fn create(eval: &mut Context, args: Vec<Value>) -> EvalResult {
    const OPERATION: TerminalOperation = TerminalOperation::Create;
    let cols = positive_u16(args[0], OPERATION, "COLS")?;
    let rows = positive_u16(args[1], OPERATION, "ROWS")?;
    let target = match args[2]
        .as_int()
        .ok_or_else(|| wrong_type("fixnump", args[2]))?
    {
        0 => TerminalDisplayTarget::Window {
            buffer: eval.buffers.current_buffer_id().ok_or_else(|| {
                terminal_error(format!(
                    "{OPERATION}: no current buffer for window terminal"
                ))
            })?,
        },
        1 => TerminalDisplayTarget::Inline,
        2 => TerminalDisplayTarget::Floating,
        _ => {
            return Err(terminal_error(format!(
                "{OPERATION}: MODE must be 0, 1, or 2"
            )));
        }
    };
    let shell = match args.get(3).copied().unwrap_or(Value::NIL) {
        value if value.is_nil() => None,
        value => Some(
            value
                .as_lisp_string()
                .ok_or_else(|| wrong_type("stringp", value))?
                .as_utf8_str()
                .ok_or_else(|| terminal_error(format!("{OPERATION}: SHELL must be UTF-8")))?
                .to_owned(),
        ),
    };
    let id = display_host(eval, OPERATION)?
        .create_terminal(TerminalCreateRequest {
            size: TerminalGridSize { cols, rows },
            target,
            shell,
            invocation: None,
        })
        .map_err(terminal_error)?;
    Ok(Value::fixnum(i64::from(id.get())))
}

/// `(neomacs-terminal-cwd-pinning-p DIRECTORY)`.
/// Non-spawning capability observation before Eshell interpreter admission.
/// The renderer still independently pins/refuses at spawn time; no retry.
fn cwd_pinning_p(_eval: &mut Context, args: Vec<Value>) -> EvalResult {
    let supported = args[0]
        .as_lisp_string()
        .and_then(|text| text.as_utf8_str().map(str::to_owned))
        .filter(|path| {
            path.starts_with('/')
                && !path.split('/').nth(1).unwrap_or("").contains(':')
                && !path.split('/').any(|part| matches!(part, "." | ".."))
                && !path.chars().any(char::is_control)
        })
        .is_some_and(|path| cwd_pinning_available(&path));
    Ok(if supported { Value::T } else { Value::NIL })
}

#[cfg(target_os = "linux")]
fn cwd_pinning_available(directory: &str) -> bool {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(directory)
    else {
        return false;
    };
    // Authenticate the actual opened inode through the same editor-pid/fd
    // spelling the renderer uses, not merely a /proc directory's existence.
    let path = format!("/proc/{}/fd/{}", std::process::id(), file.as_raw_fd());
    match (file.metadata(), std::fs::metadata(path)) {
        (Ok(opened), Ok(pinned)) => {
            opened.is_dir()
                && pinned.is_dir()
                && opened.dev() == pinned.dev()
                && opened.ino() == pinned.ino()
        }
        _ => false,
    }
}

#[cfg(not(target_os = "linux"))]
fn cwd_pinning_available(_directory: &str) -> bool {
    false
}

/// `(neomacs-terminal-spawn COLS ROWS EXECUTABLE ARGV DIRECTORY ENVIRONMENT)`.
/// Strict UTF-8/local-only seam; no shell syntax or implicit inherited env.
fn spawn(eval: &mut Context, args: Vec<Value>) -> EvalResult {
    const OPERATION: TerminalOperation = TerminalOperation::Spawn;
    fn string(value: Value) -> Result<String, Flow> {
        let text = value
            .as_lisp_string()
            .ok_or_else(|| wrong_type("stringp", value))?;
        let text = text
            .as_utf8_str()
            .ok_or_else(|| terminal_error("terminal invocation requires UTF-8"))?;
        if text.contains('\0') {
            return Err(terminal_error("terminal invocation refuses NUL"));
        }
        Ok(text.to_owned())
    }
    fn strings(value: Value) -> Result<Vec<String>, Flow> {
        crate::emacs_core::value::list_to_vec(&value)
            .ok_or_else(|| wrong_type("proper-list-p", value))?
            .into_iter()
            .map(string)
            .collect()
    }
    fn local_absolute(path: &str) -> bool {
        path.starts_with('/')
            && !path.split('/').nth(1).unwrap_or("").contains(':')
            && !path.split('/').any(|part| matches!(part, "." | ".."))
            && !path.chars().any(char::is_control)
    }
    let size = TerminalGridSize {
        cols: positive_u16(args[0], OPERATION, "COLS")?,
        rows: positive_u16(args[1], OPERATION, "ROWS")?,
    };
    let executable = string(args[2])?;
    let argv = strings(args[3])?;
    let directory = string(args[4])?;
    let environment = strings(args[5])?;
    if !local_absolute(&executable) || !local_absolute(&directory) {
        return Err(terminal_error(
            "terminal invocation requires absolute local executable and cwd without dot components",
        ));
    }
    if environment
        .iter()
        .any(|entry| entry.split('=').next().unwrap_or("").is_empty())
    {
        return Err(terminal_error(
            "terminal invocation requires nonempty environment names",
        ));
    }
    if !environment
        .iter()
        .find(|entry| entry.split('=').next() == Some("SHELL"))
        .is_some_and(|entry| entry.starts_with("SHELL="))
    {
        return Err(terminal_error(
            "exact environment requires explicit SHELL=value (portable-pty injects SHELL otherwise)",
        ));
    }
    let buffer = eval
        .buffers
        .current_buffer_id()
        .ok_or_else(|| terminal_error("no current buffer for terminal invocation"))?;
    let id = display_host(eval, OPERATION)?
        .create_terminal(TerminalCreateRequest {
            size,
            target: TerminalDisplayTarget::Window { buffer },
            shell: None,
            invocation: Some(crate::emacs_core::display_host::TerminalInvocation {
                executable,
                argv,
                directory,
                environment,
            }),
        })
        .map_err(terminal_error)?;
    Ok(Value::fixnum(i64::from(id.get())))
}

/// `(neomacs-terminal-write TERMINAL-ID STRING)`.
fn write(eval: &mut Context, args: Vec<Value>) -> EvalResult {
    const OPERATION: TerminalOperation = TerminalOperation::Write;
    let id = terminal_id(args[0], OPERATION)?;
    let data = args[1]
        .as_lisp_string()
        .ok_or_else(|| wrong_type("stringp", args[1]))?
        .as_bytes()
        .to_vec();
    display_host(eval, OPERATION)?
        .write_terminal(id, data)
        .map_err(terminal_error)?;
    Ok(Value::T)
}

/// `(neomacs-terminal-resize TERMINAL-ID COLS ROWS)`.
fn resize(eval: &mut Context, args: Vec<Value>) -> EvalResult {
    const OPERATION: TerminalOperation = TerminalOperation::Resize;
    let id = terminal_id(args[0], OPERATION)?;
    let size = TerminalGridSize {
        cols: positive_u16(args[1], OPERATION, "COLS")?,
        rows: positive_u16(args[2], OPERATION, "ROWS")?,
    };
    display_host(eval, OPERATION)?
        .resize_terminal(id, size)
        .map_err(terminal_error)?;
    Ok(Value::T)
}

/// `(neomacs-terminal-destroy TERMINAL-ID)`.
fn destroy(eval: &mut Context, args: Vec<Value>) -> EvalResult {
    const OPERATION: TerminalOperation = TerminalOperation::Destroy;
    let id = terminal_id(args[0], OPERATION)?;
    display_host(eval, OPERATION)?
        .destroy_terminal(id)
        .map_err(terminal_error)?;
    Ok(Value::T)
}

/// `(neomacs-terminal-set-float TERMINAL-ID X Y OPACITY)`.
fn set_float(eval: &mut Context, args: Vec<Value>) -> EvalResult {
    const OPERATION: TerminalOperation = TerminalOperation::SetFloat;
    let id = terminal_id(args[0], OPERATION)?;
    let x = number(args[1], OPERATION, "X")?;
    let y = number(args[2], OPERATION, "Y")?;
    let opacity = number(args[3], OPERATION, "OPACITY")?;
    let placement = TerminalFloatPlacement::new(x, y, opacity)
        .ok_or_else(|| terminal_error(format!("{OPERATION}: OPACITY must be in 0.0..=1.0")))?;
    display_host(eval, OPERATION)?
        .set_floating_terminal(id, placement)
        .map_err(terminal_error)?;
    Ok(Value::T)
}

/// `(neomacs-terminal-get-text TERMINAL-ID)`.
fn get_text(eval: &mut Context, args: Vec<Value>) -> EvalResult {
    const OPERATION: TerminalOperation = TerminalOperation::GetText;
    let id = terminal_id(args[0], OPERATION)?;
    Ok(display_host(eval, OPERATION)?
        .terminal_text(id)
        .map_err(terminal_error)?
        .map(Value::string)
        .unwrap_or(Value::NIL))
}
