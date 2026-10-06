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
/// Nextest runs each test in its own process, so the knob is set before
/// the first `Context` reads it.
fn generational_context() -> Context {
    unsafe { std::env::set_var("NEOVM_GC_GENERATIONAL", "1") };
    let ev = Context::new();
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
       (prin1-to-string kept) (memq kept (process-list)))";

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
           (setq kept (make-pipe-process :name \"kept\" :buffer nil :noquery t))
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
    assert_eq!(
        before,
        "OK (t closed \"kept\" \"qqq\" t \"#<process kept>\" nil)"
    );

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
