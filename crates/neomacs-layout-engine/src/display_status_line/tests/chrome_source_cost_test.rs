use super::*;

fn span(start: usize, end: usize, source_start: usize, id: u64) -> WindowChromeSourceSpan {
    WindowChromeSourceSpan {
        output_start: start,
        output_end: end,
        source_start,
        string_id: GlyphStringId::new(id),
    }
}

#[test]
fn chrome_span_index_matches_linear_lookup_at_gaps_boundaries_and_out_of_order_queries() {
    let spans = vec![span(1, 4, 7, 2), span(4, 6, 20, 3), span(9, 12, 0, 4)];
    let index = WindowChromeSourceSpans::new(spans.clone());
    assert!(index.ordered);
    for position in [12, 1, 5, 0, 4, 9, 8, 3, 6, 11, usize::MAX] {
        assert_eq!(
            index.source_position(position),
            spans.iter().find_map(|span| span.source_position(position)),
            "position {position}"
        );
    }
    assert_eq!(index.source_position(1), Some((GlyphStringId::new(2), 7)));
    assert_eq!(index.source_position(4), Some((GlyphStringId::new(3), 20)));
}

#[test]
fn chrome_span_index_keeps_first_match_for_overlaps_reordering_and_empty_spans() {
    for spans in [
        vec![span(2, 8, 7, 2), span(4, 10, 20, 3)],
        vec![span(8, 10, 7, 2), span(2, 8, 20, 3)],
        vec![span(2, 2, 7, 2), span(2, 8, 20, 3)],
        vec![span(8, 2, 7, 2), span(2, 8, 20, 3)],
    ] {
        let index = WindowChromeSourceSpans::new(spans.clone());
        assert!(!index.ordered);
        for position in 0..12 {
            assert_eq!(
                index.source_position(position),
                spans.iter().find_map(|span| span.source_position(position))
            );
        }
    }
    assert_eq!(
        WindowChromeSourceSpans::new(vec![]).source_position(0),
        None
    );
}

#[test]
fn chrome_span_index_admits_many_leaf_strings_without_a_per_glyph_prefix_scan() {
    let spans = (0..4096)
        .map(|i| span(i * 3, i * 3 + 2, i % 5, i as u64 + 2))
        .collect::<Vec<_>>();
    let index = WindowChromeSourceSpans::new(spans.clone());
    assert!(
        index.ordered,
        "the production binary-search branch must be admitted"
    );
    for (i, span) in spans.iter().enumerate().rev() {
        assert_eq!(index.source_position(i * 3), span.source_position(i * 3));
        assert_eq!(index.source_position(i * 3 + 2), None);
    }
}

fn chrome_context() -> (Context, BufferId, WindowId) {
    let mut eval = Context::new();
    eval.eval_str("(setq noninteractive nil)")
        .expect("interactive display");
    let buffer = eval.buffer_manager().current_buffer().expect("buffer").id();
    let frame = eval
        .frame_manager_mut()
        .create_frame("chrome-source-cost", 640, 480, buffer);
    let window = eval
        .frame_manager()
        .get(frame)
        .expect("frame")
        .selected_window;
    (eval, buffer, window)
}

fn format(
    eval: &mut Context,
    buffer: BufferId,
    window: WindowId,
    value: Value,
) -> ModeLineDisplayOutput {
    eval.buffer_manager_mut()
        .get_mut(buffer)
        .expect("buffer")
        .set_buffer_local("mode-line-format", value);
    eval_status_line_format_output(eval, "mode-line-format", window.0 as i64, buffer.0, 80)
        .expect("nonempty mode line")
}

#[test]
fn chrome_source_ids_use_identity_not_equal_text_and_keep_first_appearance_order() {
    let (mut eval, buffer, window) = chrome_context();
    let first = Value::string("same");
    let second = Value::string("same");
    assert_ne!(first.bits(), second.bits());
    let formatted = format(
        &mut eval,
        buffer,
        window,
        Value::list(vec![first, second, first]),
    );
    let sources =
        WindowChromeStringSources::new(PresentedWindowChromeArea::ModeLine, &formatted, &eval)
            .expect("source roots");
    assert_eq!(
        sources.presented.len(),
        3,
        "root and two distinct original strings"
    );
    assert_eq!(sources.source_position(0), Some((GlyphStringId::new(2), 0)));
    assert_eq!(sources.source_position(4), Some((GlyphStringId::new(3), 0)));
    assert_eq!(sources.source_position(8), Some((GlyphStringId::new(2), 0)));
    assert_eq!(sources.source_position(12), None);
}

#[test]
fn chrome_literal_equal_to_flattened_root_keeps_original_source_identity() {
    let (mut eval, buffer, window) = chrome_context();
    let original = Value::string("leaf");
    let formatted = format(&mut eval, buffer, window, original);
    let sources =
        WindowChromeStringSources::new(PresentedWindowChromeArea::ModeLine, &formatted, &eval)
            .expect("source roots");
    // Even if the flattened row's text equals the leaf's text, GNU mouse
    // provenance belongs to that leaf, not to the newly allocated flat row.
    assert_ne!(formatted.value().bits(), original.bits());
    assert_eq!(sources.presented.len(), 2);
    assert_eq!(sources.source_position(0), Some((GlyphStringId::new(2), 0)));
}

#[test]
fn chrome_cost_reduction_still_evaluates_all_three_formats_and_fresh_mouse_properties() {
    let (mut eval, buffer, window) = chrome_context();
    let value = eval
        .eval_str(
            r#"(progn
      (setq chrome-cost-evals 0)
      '((:eval (progn (setq chrome-cost-evals (1+ chrome-cost-evals))
                     (propertize "same" 'help-echo (number-to-string chrome-cost-evals))))))"#,
        )
        .expect("format");
    reset_mode_line_eval_count();
    for symbol in ["mode-line-format", "header-line-format", "tab-line-format"] {
        eval.buffer_manager_mut()
            .get_mut(buffer)
            .expect("buffer")
            .set_buffer_local(symbol, value);
        for _ in 0..2 {
            let formatted =
                eval_status_line_format_output(&mut eval, symbol, window.0 as i64, buffer.0, 80)
                    .expect("format output");
            let sources = WindowChromeStringSources::new(
                PresentedWindowChromeArea::ModeLine,
                &formatted,
                &eval,
            )
            .expect("fresh sources");
            assert_eq!(sources.source_position(0), Some((GlyphStringId::new(2), 0)));
            let count = eval
                .eval_str("chrome-cost-evals")
                .expect("count")
                .as_fixnum()
                .expect("integer");
            let source = formatted.source_spans()[0].source();
            let help = source
                .as_lisp_string()
                .expect("leaf")
                .intervals()
                .get_property_at_char_pos(
                    neovm_core::buffer::CharPos0::new(0),
                    Value::symbol("help-echo"),
                );
            assert_eq!(help, Some(Value::string(count.to_string())));
        }
    }
    assert_eq!(
        eval.eval_str("chrome-cost-evals").expect("count"),
        Value::fixnum(6)
    );
    assert_eq!(
        (
            mode_line_eval_count(),
            header_line_eval_count(),
            tab_line_eval_count()
        ),
        (2, 2, 2)
    );
}

#[test]
fn chrome_empty_check_preserves_nonempty_multibyte_and_unibyte_strings() {
    let (mut eval, buffer, window) = chrome_context();
    for expression in [r#""λ""#, "(unibyte-string 255)", r#""plain""#] {
        let value = eval.eval_str(expression).expect("string");
        let output = format(&mut eval, buffer, window, value);
        assert!(
            !output
                .value()
                .as_lisp_string()
                .expect("output")
                .as_bytes()
                .is_empty()
        );
    }
    eval.buffer_manager_mut()
        .get_mut(buffer)
        .expect("buffer")
        .set_buffer_local("mode-line-format", Value::string(""));
    assert!(
        eval_status_line_format_output(
            &mut eval,
            "mode-line-format",
            window.0 as i64,
            buffer.0,
            80
        )
        .is_none()
    );
}
