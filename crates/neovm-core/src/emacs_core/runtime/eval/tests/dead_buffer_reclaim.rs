//! Killed buffers are reclaimed like any other unreachable object.
//!
//! GNU `kill-buffer` (buffer.c) only makes the buffer dead: it resets the
//! local variables, moves `name` to `last_name` and frees the text, but the
//! `struct buffer` stays a normal Lisp object.  `mark_buffer` (alloc.c) marks
//! its remaining slots while something references it, and the ordinary vector
//! sweep frees it once nothing does.  So a killed buffer keeps answering
//! `bufferp`, `buffer-live-p` and `buffer-last-name` while it is referenced,
//! and costs nothing once it is not.

use crate::emacs_core::eval::Context;
use crate::emacs_core::format_eval_result;
use crate::emacs_core::value::Value;

/// Make and kill COUNT temporary buffers the way `with-temp-buffer` does,
/// keeping no reference to any of them.
fn churn_temp_buffers(ev: &mut Context, count: usize) {
    let form = format!(
        "(let ((i 0))
           (while (< i {count})
             (let ((b (get-buffer-create (generate-new-buffer-name \" *temp*\"))))
               (save-current-buffer (set-buffer b) (insert \"x\"))
               (kill-buffer b))
             (setq i (1+ i))))"
    );
    ev.eval_str(&form).expect("temp-buffer churn");
}

/// Boxed buffer objects currently in the heap.
fn buffer_objects(ev: &mut Context) -> usize {
    ev.tagged_heap.close_alloc_regions();
    ev.tagged_heap
        .layout_stats()
        .boxed
        .iter()
        .find(|kind| kind.class == "buffer")
        .map_or(0, |kind| kind.objects)
}

/// Unrelated garbage, so a freed buffer object's memory gets reused and a
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
    // The first cycle frees the unreachable buffer objects; their killed
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
fn temp_buffers_leave_no_buffer_objects_or_killed_records_behind() {
    temp_buffers_stay_flat(Context::new(), 100_000);
}

#[test]
fn temp_buffers_stay_flat_under_generational_collection() {
    temp_buffers_stay_flat(generational_context(), 20_000);
}

fn temp_buffers_stay_flat(mut ev: Context, count: usize) {
    churn_temp_buffers(&mut ev, 1_000);
    collect_twice(&mut ev);
    let objects_before = buffer_objects(&mut ev);
    let killed_before = ev.buffers.dead_buffer_count();

    churn_temp_buffers(&mut ev, count);
    collect_twice(&mut ev);
    let objects_after = buffer_objects(&mut ev);
    let killed_after = ev.buffers.dead_buffer_count();

    assert!(
        objects_after <= objects_before + 16,
        "buffer objects grew: {objects_before} -> {objects_after}"
    );
    assert!(
        killed_after <= killed_before + 16,
        "killed-buffer records grew: {killed_before} -> {killed_after}"
    );
}

#[test]
fn a_referenced_killed_buffer_stays_a_dead_buffer_across_gc() {
    referenced_killed_buffer_survives(Context::new());
}

#[test]
fn a_referenced_killed_buffer_survives_generational_collection() {
    referenced_killed_buffer_survives(generational_context());
}

fn referenced_killed_buffer_survives(mut ev: Context) {
    ev.eval_str(
        "(progn
           (setq kept-dead (get-buffer-create \"kept-dead\"))
           (setq kept-dead-alias (list kept-dead))
           (save-current-buffer
             (set-buffer kept-dead)
             (insert \"some text\"))
           (kill-buffer kept-dead))",
    )
    .expect("kill a referenced buffer");
    let id = ev
        .eval_str("kept-dead")
        .unwrap()
        .as_buffer_id()
        .expect("buffer object");
    let before = eval_ok(
        &mut ev,
        "(list (bufferp kept-dead) (buffer-live-p kept-dead) (buffer-name kept-dead)
               (buffer-last-name kept-dead) (eq kept-dead (car kept-dead-alias)))",
    );
    assert_eq!(before, "OK (t nil nil \"kept-dead\" t)");

    for _ in 0..3 {
        collect_twice(&mut ev);
        churn_temp_buffers(&mut ev, 200);
        churn_strings(&mut ev);
    }
    collect_twice(&mut ev);

    assert_eq!(
        eval_ok(
            &mut ev,
            "(list (bufferp kept-dead) (buffer-live-p kept-dead) (buffer-name kept-dead)
                   (buffer-last-name kept-dead) (eq kept-dead (car kept-dead-alias)))",
        ),
        before,
        "a referenced killed buffer changed across GC"
    );
    assert!(
        ev.buffers.get_dead(id).is_some(),
        "the killed record of a referenced buffer was dropped"
    );
}

#[test]
fn a_killed_buffer_is_reclaimed_once_its_last_reference_goes() {
    let mut ev = Context::new();
    ev.eval_str("(progn (setq held (get-buffer-create \"held\")) (kill-buffer held))")
        .expect("kill a referenced buffer");
    let id = ev.eval_str("held").unwrap().as_buffer_id().unwrap();
    collect_twice(&mut ev);
    assert!(
        ev.buffers.get_dead(id).is_some(),
        "reclaimed while referenced"
    );
    let objects_held = buffer_objects(&mut ev);

    ev.eval_str("(setq held nil)").unwrap();
    collect_twice(&mut ev);
    assert!(
        ev.buffers.get_dead(id).is_none(),
        "unreferenced killed buffer kept its record"
    );
    assert_eq!(
        buffer_objects(&mut ev),
        objects_held - 1,
        "unreferenced killed buffer object not freed"
    );
}

/// A Rust-side id outliving the buffer object (no Lisp reference left)
/// must still name a dead buffer, never a live one, a crash, or a leak.
#[test]
fn a_reclaimed_buffer_id_still_names_a_dead_buffer() {
    let mut ev = Context::new();
    ev.eval_str("(kill-buffer (get-buffer-create \"gone\"))")
        .unwrap();
    let id = ev
        .buffers
        .find_dead_buffer_by_name("gone")
        .expect("killed record");
    collect_twice(&mut ev);
    let objects = buffer_objects(&mut ev);

    ev.set_variable("stale", Value::make_buffer(id));
    assert_eq!(
        eval_ok(
            &mut ev,
            "(list (bufferp stale) (buffer-live-p stale) (buffer-name stale))"
        ),
        "OK (t nil nil)"
    );
    ev.eval_str("(setq stale nil)").unwrap();
    collect_twice(&mut ev);
    assert!(
        buffer_objects(&mut ev) <= objects,
        "an object made for a reclaimed id was never freed"
    );
}

fn kill_unreferenced(ev: &mut Context, name: &str) -> crate::buffer::BufferId {
    ev.eval_str(&format!("(kill-buffer (get-buffer-create {name:?}))"))
        .unwrap();
    ev.buffers
        .find_dead_buffer_by_name(name)
        .expect("killed record")
}

fn assert_held_dead_buffer(ev: &mut Context, name: &str) {
    churn_strings(ev);
    collect_twice(ev);
    churn_strings(ev);
    assert_eq!(
        eval_ok(
            ev,
            "(list (bufferp held-dead) (buffer-live-p held-dead) (buffer-last-name held-dead))"
        ),
        format!("OK (t nil {name:?})")
    );
}

/// Fetching a killed buffer's object by id while a concurrent mark runs
/// (the object was not a root when the mark started) must keep it alive,
/// even when the only reference goes into an object the marker treats as
/// already black: a cons born during the mark, after the marker drained.
#[test]
fn a_killed_buffer_fetched_during_a_mark_survives_it() {
    let mut ev = Context::new();
    // The first cycle is the stop-the-world bootstrap; later ones are
    // concurrent.
    ev.gc_collect_exact();
    let id = kill_unreferenced(&mut ev, "mid-mark");
    for _ in 0..1_000 {
        ev.gc_collect_from_current_roots();
        if ev.tagged_heap.mark_in_progress() {
            break;
        }
    }
    assert!(ev.tagged_heap.mark_in_progress(), "no mark started");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while ev.tagged_heap.concurrent_mark_running()
        && !ev.tagged_heap.concurrent_mark_done()
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(
        ev.tagged_heap.concurrent_mark_running() && ev.tagged_heap.concurrent_mark_done(),
        "the concurrent marker did not drain before the read"
    );
    let cell = Value::cons(Value::make_buffer(id), Value::NIL);
    ev.set_variable("held-dead-cell", cell);
    for _ in 0..10_000 {
        if !ev.tagged_heap.mark_in_progress() {
            break;
        }
        ev.gc_collect_from_current_roots();
        std::thread::sleep(std::time::Duration::from_micros(200));
    }
    assert!(
        !ev.tagged_heap.mark_in_progress(),
        "the mark never terminated"
    );
    ev.set_variable("again", Value::make_buffer(id));
    assert_eq!(
        eval_ok(&mut ev, "(eq (car held-dead-cell) again)"),
        "OK t",
        "the fetched object was condemned while referenced"
    );
    ev.eval_str("(setq held-dead (car held-dead-cell) again nil)")
        .unwrap();
    assert_held_dead_buffer(&mut ev, "mid-mark");
    assert!(ev.buffers.get_dead(id).is_some());
}

/// The same between mark termination and the end of a sliced sweep: the
/// unreferenced object is condemned, so the read must not return it but
/// make a fresh dead-buffer object, and the drain must keep the record
/// that object now names.
#[test]
fn a_killed_buffer_fetched_during_a_sweep_survives_it() {
    let mut ev = Context::new();
    ev.set_gc_threshold(256 * 1024);
    ev.gc_collect_exact();
    let id = kill_unreferenced(&mut ev, "mid-sweep");
    let garbage = |ev: &mut Context| {
        for i in 0..200_000u64 {
            let _ = ev
                .tagged_heap
                .alloc_bignum(malachite::integer::Integer::from(
                    (1u128 << 100) + i as u128,
                ));
        }
    };
    garbage(&mut ev);
    let mut sweeping = false;
    for _ in 0..20_000 {
        ev.gc_collect_from_current_roots();
        if ev.tagged_heap.sweep_in_progress() {
            sweeping = true;
            break;
        }
        if ev.tagged_heap.mark_in_progress() {
            std::thread::sleep(std::time::Duration::from_micros(200));
        }
    }
    assert!(sweeping, "no incremental sweep started");
    assert!(
        ev.tagged_heap.buffer_object_reclaimed(id),
        "the unreferenced object was not condemned"
    );
    ev.set_variable("held-dead", Value::make_buffer(id));
    assert!(!ev.tagged_heap.buffer_object_reclaimed(id));
    ev.tagged_heap.finish_incremental_sweep_now();
    assert_held_dead_buffer(&mut ev, "mid-sweep");
    assert!(
        ev.buffers.get_dead(id).is_some(),
        "the drain dropped the record of the object made during the sweep"
    );
}

/// GNU `mark_buffer` marks `base_buffer`: a referenced killed indirect
/// buffer keeps its killed base, record included.
#[test]
fn a_referenced_killed_indirect_buffer_keeps_its_killed_base() {
    let mut ev = Context::new();
    ev.eval_str(
        "(progn
           (setq ind (make-indirect-buffer (get-buffer-create \"b\") \"i\"))
           (kill-buffer \"b\"))",
    )
    .unwrap();
    for _ in 0..2 {
        churn_strings(&mut ev);
        collect_twice(&mut ev);
    }
    // GNU 32.0.50 prints exactly this for the same forms.
    assert_eq!(
        eval_ok(
            &mut ev,
            "(list (buffer-live-p ind) (buffer-live-p (buffer-base-buffer ind))
                   (buffer-last-name (buffer-base-buffer ind))
                   (prin1-to-string (buffer-base-buffer ind))
                   (buffer-last-name ind) (prin1-to-string ind))"
        ),
        "OK (nil nil \"b\" \"#<killed buffer>\" \"i\" \"#<killed buffer>\")"
    );
}

/// A buffer made and killed from Rust never had an object, so nothing can
/// name it from Lisp: its record goes after the next cycle, and its id
/// still prints as a killed buffer.
#[test]
fn a_buffer_killed_without_an_object_drops_its_record() {
    let mut ev = Context::new();
    let id = ev.buffers.create_buffer(" rust-only");
    assert!(ev.buffers.kill_buffer(id));
    assert!(ev.buffers.get_dead(id).is_some());
    collect_twice(&mut ev);
    assert!(ev.buffers.get_dead(id).is_none(), "the record was kept");
    assert!(ev.buffers.is_killed(id));
    ev.set_variable("stale", Value::make_buffer(id));
    assert_eq!(
        eval_ok(
            &mut ev,
            "(list (bufferp stale) (buffer-live-p stale) (prin1-to-string stale))"
        ),
        "OK (t nil \"#<killed buffer>\")"
    );
}

/// GNU's match data holds the searched buffer object (`last_thing_searched`
/// in search.c), so `(match-data t)` still returns the killed buffer, name
/// and all, after any number of collections.
#[test]
fn match_data_keeps_the_killed_buffer_it_searched() {
    let mut ev = Context::new();
    search_in_a_killed_buffer(&mut ev);
    for _ in 0..2 {
        churn_strings(&mut ev);
        collect_twice(&mut ev);
    }
    // GNU 32.0.50 prints exactly this for the same forms.
    assert_eq!(
        eval_ok(
            &mut ev,
            "(let ((buf (nth 2 (match-data t))))
               (list (bufferp buf) (buffer-live-p buf) (buffer-last-name buf)))"
        ),
        "OK (t nil \"srch\")"
    );
}

/// Once the match data moves on, nothing names the killed buffer and it
/// goes.  (No `(match-data t)` read here: that makes an object the test
/// itself could keep reachable.)
#[test]
fn a_killed_buffer_goes_once_the_match_data_moves_on() {
    let mut ev = Context::new();
    let id = search_in_a_killed_buffer(&mut ev);
    collect_twice(&mut ev);
    assert!(
        ev.buffers.get_dead(id).is_some(),
        "the match data still names the killed buffer"
    );
    ev.eval_str("(string-match \"x\" \"x\")").unwrap();
    collect_twice(&mut ev);
    collect_twice(&mut ev);
    assert!(
        ev.buffers.get_dead(id).is_none(),
        "the record outlived the match data naming it"
    );
}

fn search_in_a_killed_buffer(ev: &mut Context) -> crate::buffer::BufferId {
    ev.eval_str(
        "(save-current-buffer
           (set-buffer (get-buffer-create \"srch\"))
           (insert \"abc\")
           (goto-char 1)
           (re-search-forward \"b\"))",
    )
    .unwrap();
    ev.eval_str("(kill-buffer \"srch\")").unwrap();
    ev.buffers.find_dead_buffer_by_name("srch").unwrap()
}

/// A weak-key table entry whose key is an unreferenced killed buffer goes,
/// one keyed by a live buffer stays. GNU 32.0.50 counts 1 here too.
#[test]
fn a_weak_table_drops_an_unreferenced_killed_buffer_key() {
    let mut ev = Context::new();
    ev.eval_str(
        "(progn
           (setq wt (make-hash-table :weakness 'key))
           (let ((b (get-buffer-create \"wk\"))) (puthash b t wt) (kill-buffer b))
           (puthash (get-buffer-create \"wl\") t wt))",
    )
    .unwrap();
    churn_strings(&mut ev);
    collect_twice(&mut ev);
    assert_eq!(eval_ok(&mut ev, "(hash-table-count wt)"), "OK 1");
}

/// A minor collection does not trace the old generation, so it cannot tell
/// whether an old killed buffer is still referenced: its record must wait
/// for a major collection, which frees it if nothing refers to it.
#[test]
fn an_old_killed_buffer_waits_for_a_major_collection() {
    unsafe { std::env::set_var("NEOVM_GC_GENERATIONAL", "1") };
    let mut ev = Context::new();
    ev.gc_stress = false;
    assert!(ev.tagged_heap.generational_enabled());
    ev.tagged_heap.set_gc_threshold(usize::MAX);
    ev.eval_str(
        "(setq old-held (list (get-buffer-create \"old-held\"))
               old-free (get-buffer-create \"old-free\"))",
    )
    .unwrap();
    ev.gc_collect_exact();
    ev.eval_str("(progn (kill-buffer (car old-held)) (kill-buffer old-free) (setq old-free nil))")
        .unwrap();
    let held = ev.buffers.find_dead_buffer_by_name("old-held").unwrap();
    let free = ev.buffers.find_dead_buffer_by_name("old-free").unwrap();
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
    assert!(ev.buffers.get_dead(held).is_some());
    assert!(
        ev.buffers.get_dead(free).is_some(),
        "a minor collection dropped an old object's record"
    );
    collect_twice(&mut ev);
    assert!(
        ev.buffers.get_dead(free).is_none(),
        "a major collection kept it"
    );
    assert_eq!(
        eval_ok(
            &mut ev,
            "(list (buffer-live-p (car old-held)) (buffer-last-name (car old-held)))"
        ),
        "OK (nil \"old-held\")"
    );
}

/// The dump keeps live buffers only, while a dumped value can still name a
/// buffer killed before the dump. After loading, that buffer's object must
/// not be a root: it goes once the value naming it does.
#[test]
fn a_killed_buffer_named_by_a_dump_is_not_a_root_after_loading() {
    let dir = tempfile::tempdir().expect("dump fixture directory");
    let path = dir.path().join("killed-buffer.pdump");
    {
        let mut ev = Context::new();
        ev.eval_str(
            "(progn (setq held-dead (get-buffer-create \"dumped\")) (kill-buffer held-dead))",
        )
        .unwrap();
        crate::emacs_core::pdump::dump_to_file(&ev, &path).expect("dump");
    }
    let mut ev = crate::emacs_core::pdump::load_from_dump(&path).expect("load");
    let id = ev
        .eval_str("held-dead")
        .unwrap()
        .as_buffer_id()
        .expect("a buffer object");
    assert_eq!(
        eval_ok(
            &mut ev,
            "(list (bufferp held-dead) (buffer-live-p held-dead))"
        ),
        "OK (t nil)"
    );
    ev.eval_str("(setq held-dead nil)").unwrap();
    collect_twice(&mut ev);
    assert!(
        ev.tagged_heap.buffer_object_reclaimed(id),
        "the loaded object of a killed buffer stayed rooted"
    );
}
