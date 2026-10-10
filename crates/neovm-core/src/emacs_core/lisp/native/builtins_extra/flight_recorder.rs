//! Explicit Lisp inspection/export of numeric-only flight recorder data.
use super::{EvalResult, Integer, LispCondition, Value, expect_args, signal};
use neomacs_display_protocol::flight_recorder::{self, Phase};

fn integer(n: u64) -> Value {
    if n <= (1u64 << 60) - 1 {
        Value::fixnum(n as i64)
    } else {
        Value::bignum(Integer::from(n))
    }
}
fn pair(key: &str, n: u64) -> Value {
    Value::cons(Value::symbol(key), integer(n))
}
fn phase_label(phase: Phase) -> &'static str {
    match phase {
        Phase::CommandStart => "command-start",
        Phase::CommandEnd => "command-end",
        Phase::RedisplayStart => "redisplay-start",
        Phase::RedisplayEnd => "redisplay-end",
        Phase::LayoutStart => "layout-start",
        Phase::LayoutEnd => "layout-end",
        Phase::LayoutSealed => "layout-sealed",
        Phase::RenderHandoff => "render-handoff",
        Phase::Prepared => "prepared",
        Phase::RenderStart => "render-start",
        Phase::Submit => "submit",
        Phase::CpuPresentCall => "cpu-present-call",
        Phase::Superseded => "superseded",
        Phase::Discarded => "discarded",
    }
}

/// Read-only alist; entries contain static phase symbols and exact integers.
pub(crate) fn recent(args: Vec<Value>) -> EvalResult {
    expect_args("neomacs-flight-recorder-recent", &args, 0)?;
    let snapshot = flight_recorder::recent();
    let entries = Value::list(
        snapshot
            .entries
            .iter()
            .map(|e| {
                Value::list(vec![
                    Value::cons(Value::symbol("phase"), Value::symbol(phase_label(e.phase))),
                    pair("ns", e.ns),
                    pair("command", e.command),
                    pair("input-stream", e.input_stream),
                    pair("event-seq", e.event_seq),
                    pair("frame", e.frame),
                    pair("revision", e.revision),
                ])
            })
            .collect(),
    );
    Ok(Value::list(vec![
        Value::cons(Value::symbol("entries"), entries),
        pair("dropped-contention", snapshot.dropped_contention),
        pair("dropped-clock", snapshot.dropped_clock),
        pair("overwritten", snapshot.overwritten),
        pair("retention-ns", snapshot.retention_ns),
        pair("capacity", snapshot.capacity as u64),
        pair("now-ns", snapshot.now_ns),
        Value::cons(
            Value::symbol("busy"),
            if snapshot.busy { Value::T } else { Value::NIL },
        ),
    ]))
}

/// Export only on an explicit call, never on command/redisplay hot paths.
pub(crate) fn dump(args: Vec<Value>) -> EvalResult {
    expect_args("neomacs-flight-recorder-dump", &args, 1)?;
    let Some(path) = args[0].as_utf8_str() else {
        return Err(signal(
            LispCondition::WrongTypeArgument,
            vec![Value::symbol("stringp"), args[0]],
        ));
    };
    if path.contains('\0') {
        return Err(signal(
            LispCondition::FileError,
            vec![Value::string("Invalid flight recorder dump path"), args[0]],
        ));
    }
    flight_recorder::dump(std::path::Path::new(path)).map_err(|error| {
        signal(
            LispCondition::FileError,
            vec![Value::string(error.to_string()), args[0]],
        )
    })?;
    Ok(Value::T)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emacs_core::Context;

    #[test]
    fn flight_recorder_lisp_recent_shape_and_read_only() {
        let mut ctx = Context::new();
        flight_recorder::record(Phase::LayoutEnd, 778899, 0, 445566, 123);
        let value = ctx.eval_str("(let ((s (neomacs-flight-recorder-recent))) (and (integerp (cdr (assq 'capacity s))) (integerp (cdr (assq 'retention-ns s))) (integerp (cdr (assq 'overwritten s))) (integerp (cdr (assq 'dropped-contention s))) (listp (cdr (assq 'entries s)))))").unwrap();
        assert_eq!(value, Value::T);
        let count = || {
            flight_recorder::recent()
                .entries
                .iter()
                .filter(|e| e.command == 778899 && e.frame == 445566)
                .count()
        };
        let before = count();
        recent(vec![]).unwrap();
        recent(vec![]).unwrap();
        assert_eq!(
            count(),
            before,
            "inspection must not consume or append records"
        );
        assert!(ctx.eval_str("(neomacs-flight-recorder-recent 1)").is_err());
        assert!(ctx.eval_str("(neomacs-flight-recorder-dump 123)").is_err());
        assert!(ctx.eval_str("(neomacs-flight-recorder-dump)").is_err());
        assert!(integer(u64::MAX).as_bignum().is_some());
    }

    #[test]
    fn flight_recorder_lisp_dump_writes_requested_file_and_reports_io_error() {
        let mut ctx = Context::new();
        let dir =
            tempfile::tempdir_in(std::env::var_os("TMPDIR").expect("scratch TMPDIR")).unwrap();
        let path = dir.path().join("flight.json");
        let result = ctx
            .apply(
                Value::symbol("neomacs-flight-recorder-dump"),
                vec![Value::string(path.to_str().unwrap())],
            )
            .unwrap();
        assert_eq!(result, Value::T);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("entries"));
        assert!(text.contains("capacity"));
        assert!(
            ctx.apply(
                Value::symbol("neomacs-flight-recorder-dump"),
                vec![Value::string(dir.path().to_str().unwrap())]
            )
            .is_err()
        );
    }
}
