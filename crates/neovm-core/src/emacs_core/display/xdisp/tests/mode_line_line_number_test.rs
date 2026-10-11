//! Signed, revision-qualified mode-line line-number anchor regressions.
use super::*;
use crate::emacs_core::Context;

fn query(buf: &Buffer, end: usize) -> LineColumn {
    prefix_line_and_column(
        buf,
        buf.accessible_emacs_byte_region(),
        EmacsBytePos::new(end),
    )
}

fn scan_bytes() -> usize {
    MODE_LINE_NEWLINE_SCAN_BYTES.with(|bytes| bytes.replace(0))
}

// Deliberately does not use count_newlines or the anchor: full-prefix oracle.
fn assert_full_prefix(buf: &Buffer, end: usize) -> LineColumn {
    let accessible = buf.accessible_emacs_byte_region();
    let end = accessible.clamp(EmacsBytePos::new(end));
    let mut prefix = Vec::new();
    buf.copy_emacs_byte_range_to(EmacsByteRange::new(accessible.start(), end), &mut prefix);
    let tail = prefix.rsplit(|byte| *byte == b'\n').next().unwrap();
    let expected = LineColumn {
        line: 1 + prefix.iter().filter(|byte| **byte == b'\n').count(),
        column: if buf.get_multibyte() {
            crate::emacs_core::emacs_char::chars_in_multibyte(tail)
        } else {
            tail.len()
        },
    };
    let actual = query(buf, end.get());
    assert_eq!(actual, expected, "end byte {}", end.get());
    actual
}

#[test]
fn line_number_adjacent_motion_counts_only_the_distance_both_directions() {
    let mut eval = Context::new();
    let buf = eval.buffers.current_buffer_mut().unwrap();
    buf.insert(&"abcd\n".repeat(20_000));
    buf.reset_unchanged_region();
    assert_eq!(
        query(buf, 50_002),
        LineColumn {
            line: 10_001,
            column: 2
        }
    );
    scan_bytes();
    for line in 10_001..11_001 {
        assert_eq!(
            query(buf, line * 5 + 2),
            LineColumn {
                line: line + 1,
                column: 2
            }
        );
        assert_eq!(
            scan_bytes(),
            5,
            "forward step must not scan from an old anchor"
        );
    }
    for line in (9_000..11_000).rev() {
        assert_eq!(
            query(buf, line * 5 + 2),
            LineColumn {
                line: line + 1,
                column: 2
            }
        );
        assert_eq!(scan_bytes(), 5, "backward step must not scan from BEGV");
    }
    assert_eq!(query(buf, 45_004).column, 4);
    assert_eq!(
        scan_bytes(),
        0,
        "same-line column change has no newline-count work"
    );
}

#[test]
fn line_number_boundaries_jumps_and_empty_lines_match_full_prefix() {
    let mut eval = Context::new();
    let buf = eval.buffers.current_buffer_mut().unwrap();
    buf.insert("\na\n\nxyz\n");
    for end in [8, 0, 7, 6, 4, 3, 2, 1, 8, 5, 0, 100] {
        assert_full_prefix(buf, end);
    }
    buf.delete_emacs_byte_range(EmacsByteRange::from_usize(0, 8));
    assert_eq!(
        assert_full_prefix(buf, 0),
        LineColumn { line: 1, column: 0 }
    );
}

#[test]
fn line_number_edits_before_at_and_after_anchor_rebuild_once_without_ack() {
    for edit in [0, 4, 7] {
        let mut eval = Context::new();
        let buf = eval.buffers.current_buffer_mut().unwrap();
        buf.insert("a\nb\nc\nd\n");
        buf.reset_unchanged_region();
        query(buf, 5); // Anchor at byte 4.
        buf.goto_emacs_byte_pos(EmacsBytePos::new(edit));
        buf.insert("λ\n");
        let end = buf.accessible_emacs_byte_region().end().get();
        assert_full_prefix(buf, end);
        scan_bytes();
        assert_full_prefix(buf, end);
        assert_eq!(
            scan_bytes(),
            0,
            "pending dirty range must not repeatedly invalidate a fresh anchor"
        );
        buf.delete_emacs_byte_range(EmacsByteRange::from_usize(edit, edit + 3));
        assert_full_prefix(buf, 5);
        assert_full_prefix(buf, 3);
    }
}

#[test]
fn line_number_ack_cannot_resurrect_stale_anchor_after_insert_or_delete() {
    let mut eval = Context::new();
    let buf = eval.buffers.current_buffer_mut().unwrap();
    buf.insert("a\nb\nc\nd\n");
    buf.reset_unchanged_region();
    query(buf, 7);
    buf.goto_emacs_byte_pos(EmacsBytePos::ZERO);
    buf.insert("new\n");
    buf.reset_unchanged_region(); // Before this buffer's next mode-line query.
    assert_eq!(assert_full_prefix(buf, 11).line, 4 + 1);
    buf.delete_emacs_byte_range(EmacsByteRange::from_usize(0, 6));
    buf.reset_unchanged_region();
    assert_eq!(assert_full_prefix(buf, 5).line, 3);
}

#[test]
fn line_number_narrowing_origins_ends_and_widening_match_full_prefix() {
    let mut eval = Context::new();
    let buf = eval.buffers.current_buffer_mut().unwrap();
    buf.insert("αβ\na\ncd\nef\n");
    query(buf, 12);
    // BEGV in the middle of a multibyte-containing line.
    buf.narrow_to_emacs_byte_range(EmacsByteRange::from_usize(2, 12));
    for end in [12, 0, 4, 5, 8, 11, 2] {
        assert_full_prefix(buf, end);
    }
    query(buf, 11);
    // Same origin, but old anchor now lies beyond ZV.
    buf.narrow_to_emacs_byte_range(EmacsByteRange::from_usize(2, 6));
    for end in [100, 0, 5, 6] {
        assert_full_prefix(buf, end);
    }
    buf.widen();
    for end in [12, 2, 0, 8] {
        assert_full_prefix(buf, end);
    }
}

#[test]
fn line_number_multibyte_columns_backward_across_gap_and_long_lines() {
    let mut eval = Context::new();
    let buf = eval.buffers.current_buffer_mut().unwrap();
    let text = format!("α🙂\n{}\n界z\n", "λ".repeat(20_000));
    buf.insert(&text);
    let last = text.len() - 1;
    assert_eq!(query(buf, last), LineColumn { line: 3, column: 2 });
    assert_eq!(query(buf, 6), LineColumn { line: 1, column: 2 });
    // Move the physical gap into the long line, without changing final text.
    buf.goto_emacs_byte_pos(EmacsBytePos::new(207));
    buf.insert("x");
    buf.delete_emacs_byte_range(EmacsByteRange::from_usize(207, 208));
    buf.reset_unchanged_region();
    for end in [last, 207, 7, 6, 0, last, 20_007] {
        assert_full_prefix(buf, end);
    }
    assert_eq!(
        query(buf, 207),
        LineColumn {
            line: 2,
            column: 100
        }
    );
}

#[test]
fn line_number_unibyte_and_property_only_changes_keep_correct_coordinates() {
    let mut eval = Context::new();
    let buf = eval.buffers.current_buffer_mut().unwrap();
    buf.set_multibyte_value(false);
    buf.insert("ab\ncd\nef\n");
    query(buf, 7);
    let tick = buf.chars_modified_tick();
    buf.record_text_property_modification();
    buf.note_changed_property_region(EmacsByteRange::from_usize(0, 2));
    assert_eq!(buf.chars_modified_tick(), tick);
    scan_bytes();
    assert_eq!(
        assert_full_prefix(buf, 4),
        LineColumn { line: 2, column: 1 }
    );
    assert_eq!(
        scan_bytes(),
        3,
        "property-only changes cannot invalidate newline counts"
    );
}


#[test]
fn line_number_text_swap_clears_both_anchors_even_when_ticks_collide() {
    for acknowledge in [false, true] {
        let mut eval = Context::new();
        eval.eval_str(
            "(progn (set-buffer (get-buffer-create \"line-swap-a\")) (insert \"a\\nb\\n\"))",
        )
        .unwrap();
        let a = eval.buffers.current_buffer_id().unwrap();
        let buf = eval.buffers.get(a).unwrap();
        assert_eq!(
            assert_full_prefix(buf, 3),
            LineColumn { line: 2, column: 1 }
        );
        let cached_tick = buf.chars_modified_tick();
        assert_eq!(cached_tick, 4);
        eval.eval_str(
            "(progn (set-buffer (get-buffer-create \"line-swap-b\")) (insert \"xyz\"))",
        )
        .unwrap();
        let b = eval.buffers.current_buffer_id().unwrap();
        let buf = eval.buffers.get(b).unwrap();
        assert_eq!(buf.chars_modified_tick(), 3);
        assert_eq!(
            assert_full_prefix(buf, 3),
            LineColumn { line: 1, column: 3 }
        );
        eval.eval_str("(progn (set-buffer \"line-swap-a\") (buffer-swap-text (get-buffer \"line-swap-b\")))")
            .unwrap();
        assert!(eval.buffers.get(a).unwrap().line_number_anchor.get().is_none());
        assert!(eval.buffers.get(b).unwrap().line_number_anchor.get().is_none());
        assert_eq!(eval.buffers.get(a).unwrap().chars_modified_tick(), cached_tick);
        if acknowledge {
            eval.buffers.get(a).unwrap().reset_unchanged_region();
            eval.buffers.get(b).unwrap().reset_unchanged_region();
        }
        assert_eq!(
            assert_full_prefix(eval.buffers.get(a).unwrap(), 3),
            LineColumn { line: 1, column: 3 }
        );
        assert_eq!(
            assert_full_prefix(eval.buffers.get(b).unwrap(), 3),
            LineColumn { line: 2, column: 1 }
        );
        // Both buffers have fresh anchors now; reverse the ordinary swap.
        eval.eval_str("(progn (set-buffer \"line-swap-b\") (buffer-swap-text (get-buffer \"line-swap-a\")))")
            .unwrap();
        assert!(eval.buffers.get(a).unwrap().line_number_anchor.get().is_none());
        assert!(eval.buffers.get(b).unwrap().line_number_anchor.get().is_none());
        if acknowledge {
            eval.buffers.get(a).unwrap().reset_unchanged_region();
            eval.buffers.get(b).unwrap().reset_unchanged_region();
        }
        assert_eq!(
            assert_full_prefix(eval.buffers.get(a).unwrap(), 3),
            LineColumn { line: 2, column: 1 }
        );
        assert_eq!(
            assert_full_prefix(eval.buffers.get(b).unwrap(), 3),
            LineColumn { line: 1, column: 3 }
        );
    }
}

#[test]
fn line_number_tick_rewind_clears_base_and_indirect_anchors_before_edit() {
    for setter_on_indirect in [false, true] {
        for acknowledge in [false, true] {
            let mut eval = Context::new();
            eval.eval_str(
                "(progn (set-buffer (get-buffer-create \"line-rewind-base\")) (insert \"a\\nb\\n\"))",
            )
            .unwrap();
            let base = eval.buffers.current_buffer_id().unwrap();
            let indirect = eval
                .buffers
                .create_indirect_buffer(base, "line-rewind-indirect", false)
                .unwrap();
            for id in [base, indirect] {
                let buf = eval.buffers.get(id).unwrap();
                assert_eq!(
                    assert_full_prefix(buf, 3),
                    LineColumn { line: 2, column: 1 }
                );
                assert_eq!(buf.chars_modified_tick(), 4);
                assert!(buf.line_number_anchor.get().is_some());
            }
            let target = if setter_on_indirect {
                "line-rewind-indirect"
            } else {
                "line-rewind-base"
            };
            eval.eval_str(&format!(
                "(internal--set-buffer-modified-tick 3 (get-buffer {:?}))",
                target,
            ))
            .unwrap();
            for id in [base, indirect] {
                let buf = eval.buffers.get(id).unwrap();
                assert!(buf.line_number_anchor.get().is_none());
                assert_eq!(buf.modified_tick(), 3);
                assert_eq!(buf.chars_modified_tick(), 4);
            }
            // Remove the first newline. CHARS_MODIFF remains monotonic even
            // though MODIFF was rewound; both views must recount correctly.
            eval.eval_str("(delete-region 2 3)").unwrap();
            for id in [base, indirect] {
                let buf = eval.buffers.get(id).unwrap();
                assert_eq!(buf.chars_modified_tick(), 5);
                if acknowledge {
                    buf.reset_unchanged_region();
                }
                assert_eq!(
                    assert_full_prefix(buf, 1),
                    LineColumn { line: 1, column: 1 }
                );
                assert_eq!(
                    assert_full_prefix(buf, 3),
                    LineColumn { line: 2, column: 0 }
                );
            }
        }
    }
}
