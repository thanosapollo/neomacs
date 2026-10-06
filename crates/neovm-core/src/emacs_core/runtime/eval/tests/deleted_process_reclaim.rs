//! Deleted processes are reclaimed like any other unreachable object.
//!
//! GNU `delete-process` (process.c `Fdelete_process`, `remove_process`)
//! only takes the process off `Vprocess_alist`; the process stays a normal
//! vectorlike that keeps its status, name and plist while something
//! references it, and the vector sweep (alloc.c `sweep_vectors`) frees it
//! once nothing does.

use crate::emacs_core::eval::Context;
use crate::emacs_core::format_eval_result;
use crate::emacs_core::value::Value;

/// Make and delete COUNT pipe processes, keeping no reference to any.
fn churn_pipe_processes(ev: &mut Context, count: usize) {
    let form = format!(
        "(let ((i 0))
           (while (< i {count})
             (delete-process
              (make-pipe-process :name \"churn\" :buffer nil :noquery t))
             (setq i (1+ i))))"
    );
    ev.eval_str(&form).expect("pipe-process churn");
}

/// Boxed process objects currently in the heap.
fn process_objects(ev: &mut Context) -> usize {
    ev.tagged_heap.close_alloc_regions();
    ev.tagged_heap
        .layout_stats()
        .boxed
        .iter()
        .find(|kind| kind.class == "process")
        .map_or(0, |kind| kind.objects)
}

/// Unrelated garbage, so a freed process object's memory gets reused and a
/// dangling reference shows up as a wrong answer instead of passing by luck.
fn churn_strings(ev: &mut Context) {
    ev.eval_str(
        "(let ((i 0) (l nil))
           (while (< i 20000)
             (setq l (cons (make-string 24 ?z) l))
             (setq i (1+ i))))",
    )
    .expect("string churn");
}

fn collect_twice(ev: &mut Context) {
    // The first cycle frees the unreachable process objects; their deleted
    // records drop after it, and whatever those records held goes in the
    // next cycle.
    ev.gc_collect_exact();
    ev.gc_collect_exact();
}

fn eval_ok(ev: &mut Context, form: &str) -> String {
    let results = ev.eval_str_each(form);
    format_eval_result(results.last().expect("one form"))
}

/// A context whose collector runs generational minor and major cycles.
/// The heap reads the knob once, when it is made, so the variable is set
/// only around that and put back before any other test can see it.
fn generational_context() -> Context {
    let saved = std::env::var_os("NEOVM_GC_GENERATIONAL");
    unsafe { std::env::set_var("NEOVM_GC_GENERATIONAL", "1") };
    let ev = Context::new();
    match saved {
        Some(value) => unsafe { std::env::set_var("NEOVM_GC_GENERATIONAL", value) },
        None => unsafe { std::env::remove_var("NEOVM_GC_GENERATIONAL") },
    }
    assert!(ev.tagged_heap.generational_enabled());
    ev
}

#[test]
fn deleted_pipe_processes_leave_no_objects_or_records_behind() {
    deleted_processes_stay_flat(Context::new());
}

#[test]
fn deleted_pipe_processes_stay_flat_under_generational_collection() {
    deleted_processes_stay_flat(generational_context());
}

fn deleted_processes_stay_flat(mut ev: Context) {
    churn_pipe_processes(&mut ev, 100);
    collect_twice(&mut ev);
    let objects_before = process_objects(&mut ev);
    let deleted_before = ev.processes.deleted_process_count();

    churn_pipe_processes(&mut ev, 2_000);
    collect_twice(&mut ev);
    let objects_after = process_objects(&mut ev);
    let deleted_after = ev.processes.deleted_process_count();

    assert!(
        objects_after <= objects_before + 16,
        "process objects grew: {objects_before} -> {objects_after}"
    );
    assert!(
        deleted_after <= deleted_before + 16,
        "deleted-process records grew: {deleted_before} -> {deleted_after}"
    );
}

const KEPT_STATE: &str = "(list (processp kept) (process-status kept) (process-name kept)
       (plist-get (process-plist kept) 'tag) (eq kept (car kept-alias))
       (prin1-to-string kept) (memq kept (process-list))
       (process-exit-status kept) (buffer-name (process-buffer kept))
       (get-process \"kept\") (delete-process kept)
       (process-status kept))";

/// What GNU 32.0.50 prints for `KEPT_STATE`, before and after collections.
/// (`process-get` is `plist-get` on `process-plist`, already covered; a bare
/// `Context` has no subr.el to define it.)
const KEPT_STATE_GNU: &str =
    "OK (t closed \"kept\" \"qqq\" t \"#<process kept>\" nil 0 \"kb\" nil nil closed)";

#[test]
fn a_referenced_deleted_process_stays_a_deleted_process_across_gc() {
    referenced_deleted_process_survives(Context::new());
}

#[test]
fn a_referenced_deleted_process_survives_generational_collection() {
    referenced_deleted_process_survives(generational_context());
}

fn referenced_deleted_process_survives(mut ev: Context) {
    ev.eval_str(
        "(progn
           (setq kept (make-pipe-process :name \"kept\" :noquery t
                                         :buffer (get-buffer-create \"kb\")))
           (setq kept-alias (list kept))
           (set-process-plist kept (list 'tag (make-string 3 ?q)))
           (delete-process kept))",
    )
    .expect("delete a referenced process");
    let id = ev
        .eval_str("kept")
        .unwrap()
        .as_process_id()
        .expect("process object");
    let before = eval_ok(&mut ev, KEPT_STATE);
    assert_eq!(before, KEPT_STATE_GNU);

    for _ in 0..3 {
        collect_twice(&mut ev);
        churn_pipe_processes(&mut ev, 200);
        churn_strings(&mut ev);
    }
    collect_twice(&mut ev);

    assert_eq!(
        eval_ok(&mut ev, KEPT_STATE),
        before,
        "a referenced deleted process changed across GC"
    );
    assert!(
        ev.processes.get_any(id).is_some(),
        "the record of a referenced deleted process was dropped"
    );
}

#[test]
fn a_deleted_process_is_reclaimed_once_its_last_reference_goes() {
    let mut ev = Context::new();
    ev.eval_str(
        "(progn (setq held (make-pipe-process :name \"held\" :buffer nil :noquery t))
                (delete-process held))",
    )
    .expect("delete a referenced process");
    let id = ev.eval_str("held").unwrap().as_process_id().unwrap();
    collect_twice(&mut ev);
    assert!(
        ev.processes.get_any(id).is_some(),
        "reclaimed while referenced"
    );
    let objects_held = process_objects(&mut ev);
    let deleted_held = ev.processes.deleted_process_count();

    ev.eval_str("(setq held nil)").unwrap();
    collect_twice(&mut ev);
    assert!(
        ev.processes.get_any(id).is_none(),
        "unreferenced deleted process kept its record"
    );
    assert_eq!(ev.processes.deleted_process_count(), deleted_held - 1);
    assert_eq!(
        process_objects(&mut ev),
        objects_held - 1,
        "unreferenced deleted process object not freed"
    );
}

/// A live process is a root even when Lisp holds no reference to it
/// (GNU marks it through `Vprocess_alist`).
#[test]
fn a_live_unreferenced_process_survives_gc() {
    let mut ev = Context::new();
    ev.eval_str("(make-pipe-process :name \"alive\" :buffer nil :noquery t)")
        .unwrap();
    for _ in 0..2 {
        churn_strings(&mut ev);
        collect_twice(&mut ev);
    }
    assert_eq!(
        eval_ok(
            &mut ev,
            "(let ((p (get-process \"alive\")))
               (list (processp p) (process-status p)
                     (prin1-to-string p)))"
        ),
        "OK (t open \"#<process alive>\")"
    );
}

/// A Rust-held id outliving its process object (no Lisp reference left)
/// must not crash: the object made for it again is a process, and is
/// freed again once dropped.
#[test]
fn a_reclaimed_process_id_turned_back_into_a_value_is_harmless() {
    let mut ev = Context::new();
    let id = ev
        .eval_str("(setq gone (make-pipe-process :name \"gone\" :buffer nil :noquery t))")
        .unwrap()
        .as_process_id()
        .unwrap();
    ev.eval_str("(progn (delete-process gone) (setq gone nil))")
        .unwrap();
    collect_twice(&mut ev);
    assert!(ev.processes.get_any(id).is_none(), "record kept");
    let objects = process_objects(&mut ev);

    ev.set_variable("stale", Value::make_process(id));
    assert_eq!(
        eval_ok(
            &mut ev,
            "(list (processp stale) (memq stale (process-list)))"
        ),
        "OK (t nil)"
    );
    ev.eval_str("(setq stale nil)").unwrap();
    collect_twice(&mut ev);
    assert!(
        process_objects(&mut ev) <= objects,
        "an object made for a reclaimed id was never freed"
    );
}

/// A minor collection does not trace the old generation, so it cannot tell
/// whether an old deleted process is still referenced: its record must wait
/// for a major collection, which frees it if nothing refers to it.
#[test]
fn an_old_deleted_process_waits_for_a_major_collection() {
    let mut ev = generational_context();
    ev.tagged_heap.set_gc_threshold(usize::MAX);
    ev.eval_str(
        "(setq old-held (list (make-pipe-process :name \"old-held\" :buffer nil :noquery t))
               old-free (make-pipe-process :name \"old-free\" :buffer nil :noquery t))",
    )
    .unwrap();
    let held = ev
        .eval_str("(car old-held)")
        .unwrap()
        .as_process_id()
        .unwrap();
    let free = ev.eval_str("old-free").unwrap().as_process_id().unwrap();
    ev.gc_collect_exact();
    ev.eval_str(
        "(progn (delete-process (car old-held)) (delete-process old-free) (setq old-free nil))",
    )
    .unwrap();
    assert!(ev.tagged_heap.should_run_minor(false, false));
    let completed = ev.gc_count;
    ev.gc_collect_from_current_roots_impl(false);
    for _ in 0..10_000 {
        if !ev.tagged_heap.sweep_in_progress() {
            break;
        }
        ev.gc_collect_from_current_roots_impl(false);
    }
    assert_eq!(ev.gc_count, completed + 1, "no minor collection completed");
    assert!(ev.processes.get_any(held).is_some());
    assert!(
        ev.processes.get_any(free).is_some(),
        "a minor collection dropped an old object's record"
    );
    collect_twice(&mut ev);
    assert!(
        ev.processes.get_any(free).is_none(),
        "a major collection kept it"
    );
    assert_eq!(
        eval_ok(
            &mut ev,
            "(list (process-status (car old-held)) (process-name (car old-held)))"
        ),
        "OK (closed \"old-held\")"
    );
}

/// GNU's read loop holds the process in a C local while it decodes output
/// and runs the filter (process.c `read_and_dispose_of_process_output`).
/// A `:post-read-conversion` that deletes the process by name and collects
/// must still see the output reach the filter, with the process's name.
/// GNU 32.0.50 gives (("dpr" signal "hello")); the status is not checked.
#[cfg(unix)]
#[test]
fn a_process_deleted_while_its_output_decodes_still_gets_its_output() {
    let mut ev = crate::test_utils::runtime_startup_context();
    assert_eq!(
        eval_ok(
            &mut ev,
            "(progn
               (defun dpr-post-read (len)
                 (when (get-process \"dpr\")
                   (delete-process \"dpr\")
                   (garbage-collect))
                 len)
               (define-coding-system 'dpr-test \"test\" :coding-type 'raw-text
                 :mnemonic ?T :post-read-conversion 'dpr-post-read)
               (setq dpr-seen nil)
               (make-process :name \"dpr\" :command '(\"printf\" \"hello\")
                             :coding 'dpr-test :connection-type 'pipe
                             :noquery t :sentinel #'ignore
                             :filter (lambda (proc out)
                                       (push (list (process-name proc) out) dpr-seen)))
               (let ((n 0))
                 (while (and (null dpr-seen) (< n 50))
                   (accept-process-output nil 0.1)
                   (setq n (1+ n))))
               dpr-seen)"
        ),
        "OK ((\"dpr\" \"hello\"))"
    );
}

/// GNU `send_process` holds the process while it waits for a `:nowait`
/// connection; a timer that deletes it by name and collects meanwhile
/// leaves a deleted process, and the send signals that it is not running.
/// GNU 32.0.50 signals exactly this for both primitives.  Without a route
/// that keeps the connection pending there is nothing to test.
#[test]
fn sending_to_a_process_deleted_while_it_connects_says_not_running() {
    for send in [
        "(process-send-string \"dps\" \"x\")",
        "(process-send-region \"dps\" 1 2)",
    ] {
        let mut ev = crate::test_utils::runtime_startup_context();
        let status = eval_ok(
            &mut ev,
            "(progn
               (set-buffer (get-buffer-create \"dps-src\"))
               (insert \"x\")
               (process-status
                (make-network-process :name \"dps\" :host \"10.255.255.1\"
                                      :service 9 :nowait t :noquery t
                                      :sentinel #'ignore)))",
        );
        if status != "OK connect" {
            eprintln!("skipped: the connection is not pending ({status})");
            return;
        }
        ev.eval_str("(run-at-time 0 nil (lambda () (delete-process \"dps\") (garbage-collect)))")
            .unwrap();
        assert_eq!(
            eval_ok(
                &mut ev,
                &format!("(condition-case err (progn {send} 'sent) (error err))")
            ),
            "OK (error \"Process dps not running: deleted\n\")",
            "{send}"
        );
    }
}
