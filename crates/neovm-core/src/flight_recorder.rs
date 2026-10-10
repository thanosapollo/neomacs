//! Numeric-only evaluator correlation for the bounded display flight recorder.
//! Existing last tracked delivery identity is observed, never acknowledged.
use neomacs_display_protocol::flight_recorder::{Phase, record_input};
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_COMMAND: AtomicU64 = AtomicU64::new(1);
thread_local! {
    static ACTIVE: Cell<u64> = const { Cell::new(0) };
    static LATEST: Cell<u64> = const { Cell::new(0) };
    static ACTIVE_INPUT: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
    static LATEST_INPUT: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
}

/// Active command (including recursive commands), otherwise the latest command
/// on this evaluator thread. This is correlation, not input acknowledgement.
pub fn correlation() -> (u64, u64) {
    let active = ACTIVE.get();
    (
        if active == 0 { LATEST.get() } else { active },
        input_identity().1,
    )
}
pub fn input_identity() -> (u64, u64) {
    if ACTIVE.get() == 0 {
        LATEST_INPUT.get()
    } else {
        ACTIVE_INPUT.get()
    }
}

pub(crate) struct CommandSpan {
    id: u64,
    previous: u64,
    input: (u64, u64),
    previous_input: (u64, u64),
    frame: u64,
    ended: bool,
}
impl CommandSpan {
    pub(crate) fn begin(frame: u64, input: (u64, u64)) -> Self {
        // Stay in the positive fixnum range; wrapping is unreachable within the
        // five-minute retention window, and does not grow an identity registry.
        let id = NEXT_COMMAND.fetch_add(1, Ordering::Relaxed) % ((1u64 << 60) - 1) + 1;
        let previous = ACTIVE.replace(id);
        let previous_input = ACTIVE_INPUT.replace(input);
        record_input(Phase::CommandStart, id, input.0, input.1, frame, 0);
        Self {
            id,
            previous,
            input,
            previous_input,
            frame,
            ended: false,
        }
    }
    pub(crate) fn finish(&mut self) {
        if !self.ended {
            record_input(
                Phase::CommandEnd,
                self.id,
                self.input.0,
                self.input.1,
                self.frame,
                0,
            );
            ACTIVE.set(self.previous);
            ACTIVE_INPUT.set(self.previous_input);
            LATEST.set(self.id);
            LATEST_INPUT.set(self.input);
            self.ended = true;
        }
    }
}
impl Drop for CommandSpan {
    fn drop(&mut self) {
        self.finish();
    }
}

/// Balanced end records on normal return, Lisp non-local exit and unwinding.
pub(crate) struct RedisplaySpan {
    command: u64,
    event: u64,
    stream: u64,
    frame: u64,
}
impl RedisplaySpan {
    pub(crate) fn begin(frame: u64) -> Self {
        let (command, event) = correlation();
        let stream = input_identity().0;
        record_input(Phase::RedisplayStart, command, stream, event, frame, 0);
        Self {
            command,
            event,
            stream,
            frame,
        }
    }
}
impl Drop for RedisplaySpan {
    fn drop(&mut self) {
        record_input(
            Phase::RedisplayEnd,
            self.command,
            self.stream,
            self.event,
            self.frame,
            0,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flight_recorder_real_command_loop_and_redisplay_hooks() {
        use crate::emacs_core::{Context, Value};
        let mut ctx = Context::new();
        ctx.eval_str(
            r#"(progn
            (setq inhibit-redisplay nil)
            (fset 'command-execute
                  (lambda (command &optional _record _keys _special) (funcall command)))
            (fset 'flight-step (lambda () (interactive) nil))
            (let ((global (make-sparse-keymap)))
              (use-global-map global)
              (define-key global "a" 'flight-step)
              (execute-kbd-macro "aa")))"#,
        )
        .unwrap();
        let command = correlation().0;
        assert_ne!(command, 0);
        let snapshot = neomacs_display_protocol::flight_recorder::recent();
        let phases: Vec<_> = snapshot
            .entries
            .iter()
            .filter(|e| e.command == command)
            .map(|e| e.phase)
            .collect();
        assert!(phases.contains(&Phase::CommandStart));
        assert!(phases.contains(&Phase::CommandEnd));
        ctx.redisplay_fn = Some(Box::new(|_| {}));
        assert_eq!(ctx.eval_str("(redisplay t)").unwrap(), Value::T);
        let snapshot = neomacs_display_protocol::flight_recorder::recent();
        let phases: Vec<_> = snapshot
            .entries
            .iter()
            .filter(|e| e.command == command)
            .map(|e| e.phase)
            .collect();
        assert!(phases.contains(&Phase::RedisplayStart));
        assert!(phases.contains(&Phase::RedisplayEnd));
    }

    #[test]
    fn flight_recorder_nested_commands_restore_correlation_and_balance() {
        let before = ACTIVE.get();
        let mut outer = CommandSpan::begin(7654321, (123, 7));
        let outer_id = correlation().0;
        {
            let _inner = CommandSpan::begin(7654321, (124, 8));
            assert_ne!(correlation().0, outer_id);
            assert_eq!(input_identity(), (124, 8));
        }
        assert_eq!(correlation(), (outer_id, 7));
        assert_eq!(input_identity(), (123, 7));
        outer.finish();
        assert_eq!(ACTIVE.get(), before);
        let snapshot = neomacs_display_protocol::flight_recorder::recent();
        let own: Vec<_> = snapshot
            .entries
            .iter()
            .filter(|e| e.command == outer_id)
            .collect();
        assert_eq!(own.len(), 2);
        assert_eq!(own[0].phase, Phase::CommandStart);
        assert_eq!(own[1].phase, Phase::CommandEnd);
        assert!(
            own.iter()
                .all(|e| e.input_stream == 123 && e.event_seq == 7)
        );
    }
}
