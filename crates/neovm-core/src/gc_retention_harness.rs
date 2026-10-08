//! Reduced native-evidence harness; not the historical unit-test binary.
//! Only the original killed-window fixture and its observational companion
//! are copied here. Bodies and helpers remain byte-identical to those fixtures.

use crate::emacs_core::eval::Context;
use crate::emacs_core::format_eval_result;

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

#[doc(hidden)]
pub fn a_window_configuration_keeps_a_killed_window_buffer() {
    let mut ev = crate::test_utils::runtime_startup_context();
    ev.eval_str(
        "(progn
           (setq wk (make-hash-table :weakness 'key))
           (puthash (get-buffer-create \"lf\") t wk)
           (set-window-buffer (selected-window) \"lf\")
           (set-buffer (get-buffer-create \"cur\"))
           (setq conf (current-window-configuration))
           (set-window-buffer (selected-window) (get-buffer-create \"other\"))
           (kill-buffer \"lf\")
           nil)",
    )
    .unwrap();
    collect_twice(&mut ev);
    assert_eq!(eval_ok(&mut ev, "(hash-table-count wk)"), "OK 1");
    ev.eval_str("(setq conf nil)").unwrap();
    collect_twice(&mut ev);
    assert_eq!(eval_ok(&mut ev, "(hash-table-count wk)"), "OK 0");
}

fn killed_window_retention_checkpoint(ev: &Context, checkpoint: &str) {
    use crate::emacs_core::symbol::SymbolRedirect;
    use crate::tagged::header::{BufferObj, HashTableObj, VecLikeHeader, VecLikeType};
    use crate::tagged::value::{TAG_MASK, TAG_VECLIKE};

    // UNKNOWN is a string, distinct from absent/null and observed false.
    let mut weak_entry = "\"UNKNOWN\"";
    let mut buffer_id = None;
    let mut address = None;
    let mut permanent = "\"UNKNOWN\"";
    let mut tenured = "\"UNKNOWN\"";
    // Startup intentionally does not read a possible bootstrap symbol named
    // wk: this test has not yet created its lf/key/table.
    if checkpoint != "startup" {
        if let Some(symbol) = ev.obarray.get("wk") {
            if symbol.redirect() == SymbolRedirect::Plainval {
                let table_value = symbol.plain();
                if table_value.tag() == TAG_VECLIKE {
                    let ptr = (table_value.bits() & !TAG_MASK) as *const VecLikeHeader;
                    // SAFETY: the current plain obarray value is live; no Lisp,
                    // safepoint, collection or mutation occurs in this helper.
                    if unsafe { (*ptr).type_tag == VecLikeType::HashTable } {
                        let table = unsafe { &(*(ptr as *const HashTableObj)).table };
                        if !table.needs_hydration() {
                            let mut entries = table.entries_in_slot_order();
                            let first = entries.next();
                            if entries.next().is_none() {
                                match first {
                                    None => weak_entry = "false",
                                    Some(entry) if entry.key.tag() == TAG_VECLIKE => {
                                        let key_ptr = (entry.key.bits() & !TAG_MASK)
                                            as *const VecLikeHeader;
                                        // SAFETY: re-lookup in wk's current live
                                        // storage, never a saved pre-GC pointer.
                                        if unsafe { (*key_ptr).type_tag == VecLikeType::Buffer } {
                                            let buffer = unsafe { &*(key_ptr as *const BufferObj) };
                                            weak_entry = "true";
                                            buffer_id = Some(buffer.id.0);
                                            address = Some(key_ptr as usize);
                                            permanent = if buffer.header.gc.generation.permanent() {
                                                "true"
                                            } else {
                                                "false"
                                            };
                                            tenured = if buffer.header.gc.tenured {
                                                "true"
                                            } else {
                                                "false"
                                            };
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let (partition_dump, dump_blackened, mapped) =
        ev.tagged_heap.retention_discriminator_phase_for_test(address);
    let id_json = buffer_id.map_or_else(|| "null".to_owned(), |id| id.to_string());
    let address_json = address.map_or_else(|| "null".to_owned(), |addr| addr.to_string());
    let mapped_json = mapped.map_or("\"UNKNOWN\"", |mapped| {
        if mapped { "true" } else { "false" }
    });
    // No root enumeration or claim about an image's path/hash, a precise
    // promotion/shrink boundary, or a per-heap epoch. gc_collections is NOT
    // renamed to an epoch. Output contains bounded scalars, never Lisp data.
    eprintln!(
        "RETENTION_R10 {{\"checkpoint\":\"{checkpoint}\",\"collections\":{},\"partition_dump\":{partition_dump},\"dump_blackened\":{dump_blackened},\"first_cycle\":{},\"weak_entry\":{weak_entry},\"buffer_id\":{id_json},\"address\":{address_json},\"mapped\":{mapped_json},\"permanent\":{permanent},\"tenured\":{tenured},\"heap_epoch\":\"UNKNOWN\",\"root\":\"UNKNOWN\",\"promotion_checkpoint\":\"UNKNOWN\",\"shrink_counter\":\"UNKNOWN\"}}",
        ev.tagged_heap.gc_collections(),
        ev.tagged_heap.is_partition_first_cycle(),
    );
}

#[doc(hidden)]
pub fn killed_window_retention_observational_discriminator() {
    let mut ev = crate::test_utils::runtime_startup_context();
    killed_window_retention_checkpoint(&ev, "startup");
    ev.eval_str(
        "(progn
           (setq wk (make-hash-table :weakness 'key))
           (puthash (get-buffer-create \"lf\") t wk)
           (set-window-buffer (selected-window) \"lf\")
           (set-buffer (get-buffer-create \"cur\"))
           (setq conf (current-window-configuration))
           (set-window-buffer (selected-window) (get-buffer-create \"other\"))
           (kill-buffer \"lf\")
           nil)",
    )
    .unwrap();
    killed_window_retention_checkpoint(&ev, "before_first_collection");
    collect_twice(&mut ev);
    killed_window_retention_checkpoint(&ev, "after_first_pair");
    assert_eq!(eval_ok(&mut ev, "(hash-table-count wk)"), "OK 1");
    ev.eval_str("(setq conf nil)").unwrap();
    killed_window_retention_checkpoint(&ev, "after_conf_nil");
    collect_twice(&mut ev);
    killed_window_retention_checkpoint(&ev, "after_final_pair");
    assert_eq!(eval_ok(&mut ev, "(hash-table-count wk)"), "OK 0");
}
