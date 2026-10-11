//! Point-driven scrolls reuse rows only after canonical viewport placement.

use super::*;

const LINE: &str = "(defun f (a b) (+ a b))\n";

fn fixture(
    height: u32,
    margin: i64,
) -> (
    Context,
    neovm_core::window::FrameId,
    BufferId,
    neovm_core::window::WindowId,
    LayoutEngine,
) {
    fixture_with_text(&LINE.repeat(200), height, margin)
}

fn fixture_with_text(
    text: &str,
    height: u32,
    margin: i64,
) -> (
    Context,
    neovm_core::window::FrameId,
    BufferId,
    neovm_core::window::WindowId,
    LayoutEngine,
) {
    let (mut eval, frame, buffer, window) = incr_editing_frame(text, 800, height);
    realize_test_gui_frame(&mut eval, frame);
    {
        let buffer = eval.buffer_manager_mut().get_mut(buffer).unwrap();
        buffer.set_buffer_local("scroll-conservatively", Value::fixnum(101));
        buffer.set_buffer_local("scroll-margin", Value::fixnum(margin));
        buffer.set_buffer_local("display-line-numbers", Value::NIL);
        buffer.set_buffer_local("mode-line-format", Value::string("ML"));
        buffer.set_buffer_local("bidi-paragraph-direction", Value::symbol("left-to-right"));
    }
    let mut engine = LayoutEngine::new();
    engine.layout_frame_rust(&mut eval, frame);
    activate_last_engine_presentation(&mut eval, &engine, frame);
    (eval, frame, buffer, window, engine)
}

fn next_unshown_line(
    eval: &Context,
    engine: &LayoutEngine,
    frame: neovm_core::window::FrameId,
) -> usize {
    selected_window_layout_trace(eval, engine, frame)
        .matrix_rows
        .iter()
        .filter(|row| row.role == GlyphRowRole::Text && row.displays_text)
        .last()
        .expect("visible text")
        .start_charpos
        + LINE.len()
}

fn first_line_beyond_previous_viewport(
    eval: &Context,
    engine: &LayoutEngine,
    frame: neovm_core::window::FrameId,
) -> usize {
    let (_, windows) = crate::neovm_bridge::collect_layout_params_with_font_sizing(
        eval,
        frame,
        None,
        engine.font_sizing,
    )
    .unwrap();
    let previous_end = windows
        .iter()
        .find(|window| window.selected)
        .unwrap()
        .previous_visible_end
        .unwrap() as usize;
    (previous_end / LINE.len() + 1) * LINE.len()
}

fn move_point(eval: &mut Context, buffer: BufferId, target: usize) {
    eval.buffer_manager_mut()
        .get_mut(buffer)
        .unwrap()
        .goto_emacs_byte_pos(EmacsBytePos::new(target));
}

#[test]
fn point_scroll_reuses_overlap_and_publishes_its_new_start() {
    // This frame leaves a whole number of body rows; the partial-row cases
    // below intentionally exercise rejection instead of successful reuse.
    let (mut eval, frame, buffer, window, mut engine) = fixture(256, 0);
    let (mut full_eval, full_frame, full_buffer, _, mut full) = fixture(256, 0);
    engine.layout_frame_rust(&mut eval, frame);
    activate_last_engine_presentation(&mut eval, &engine, frame);
    for _ in 0..3 {
        let before = selected_window_layout_trace(&eval, &engine, frame).window_start;
        let target = first_line_beyond_previous_viewport(&eval, &engine, frame);
        move_point(&mut eval, buffer, target);
        move_point(&mut full_eval, full_buffer, target);
        full.retained_window_matrices.clear();
        full.prepared_viewports = PreparedViewports::default();
        engine.layout_frame_rust(&mut eval, frame);
        full.layout_frame_rust(&mut full_eval, full_frame);
        let actual = selected_window_layout_trace(&eval, &engine, frame);
        assert_eq!(
            actual,
            selected_window_layout_trace(&full_eval, &full, full_frame)
        );
        assert!(
            actual.window_start > before,
            "point must scroll the viewport"
        );
        let stats = engine.last_layout_stats();
        assert_eq!(stats.scroll_windows, 1, "{stats:?}");
        assert!(stats.reused_shifted_rows > 0, "{stats:?}");
        assert!(
            stats.prepared_windows == 0,
            "a cursor replay cannot publish the new start: {stats:?}"
        );
        assert_eq!(
            engine.retained_window_matrices[&DisplayWindowId::new(window.0 as i64)]
                .key
                .window_start,
            actual.window_start.as_i64() - 1,
        );
        activate_last_engine_presentation(&mut eval, &engine, frame);
        activate_last_engine_presentation(&mut full_eval, &full, full_frame);
    }
    engine.layout_frame_rust(&mut eval, frame);
    assert_eq!(engine.last_layout_stats().cursor_only_windows, 1);
}

#[test]
fn point_scroll_respects_measured_margins_and_partial_rows() {
    for (height, margin) in [(240, 2), (245, 0), (245, 2)] {
        let (mut eval, frame, buffer, _, mut engine) = fixture(height, margin);
        let (mut full_eval, full_frame, full_buffer, _, mut full) = fixture(height, margin);
        engine.layout_frame_rust(&mut eval, frame);
        activate_last_engine_presentation(&mut eval, &engine, frame);
        for _ in 0..3 {
            let target = next_unshown_line(&eval, &engine, frame);
            move_point(&mut eval, buffer, target);
            move_point(&mut full_eval, full_buffer, target);
            full.retained_window_matrices.clear();
            full.prepared_viewports = PreparedViewports::default();
            engine.layout_frame_rust(&mut eval, frame);
            full.layout_frame_rust(&mut full_eval, full_frame);
            assert_eq!(
                selected_window_layout_trace(&eval, &engine, frame),
                selected_window_layout_trace(&full_eval, &full, full_frame),
                "height={height}, margin={margin}",
            );
            activate_last_engine_presentation(&mut eval, &engine, frame);
            activate_last_engine_presentation(&mut full_eval, &full, full_frame);
        }
    }
}

#[test]
fn point_scroll_hook_can_replace_the_proposed_start() {
    let (mut eval, frame, buffer, _, mut engine) = fixture(256, 0);
    let target = first_line_beyond_previous_viewport(&eval, &engine, frame);
    eval.eval_str(&format!(
        "(progn
           (setq neomacs-point-scroll-hook-start nil)
           (setq neomacs-point-scroll-hook-count 0)
           (setq window-scroll-functions
             (list (lambda (window start)
               (setq neomacs-point-scroll-hook-count (1+ neomacs-point-scroll-hook-count))
               (if neomacs-point-scroll-hook-start nil
                 (progn
                   (setq neomacs-point-scroll-hook-start (+ start {}))
                   (set-window-start window neomacs-point-scroll-hook-start)))))))",
        LINE.len()
    ))
    .unwrap();
    move_point(&mut eval, buffer, target);
    let FrameLayoutAttempt::Prepared(state) = engine.redisplay_frame_attempt(&mut eval, frame)
    else {
        panic!("hook-adjusted point scroll must prepare");
    };
    engine.last_frame_display_state = Some(state);
    let actual = selected_window_layout_trace(&eval, &engine, frame);
    assert_eq!(
        eval.eval_str("neomacs-point-scroll-hook-count")
            .unwrap()
            .as_fixnum(),
        Some(1),
        "point-driven start publication must run its hook once"
    );
    let expected_start = eval
        .eval_str("neomacs-point-scroll-hook-start")
        .unwrap()
        .as_fixnum()
        .unwrap();
    assert_eq!(actual.window_start.as_i64(), expected_start);
    let mut fresh = LayoutEngine::new();
    fresh.layout_frame_rust(&mut eval, frame);
    assert_eq!(actual, selected_window_layout_trace(&eval, &fresh, frame));
}

fn projected_fixture(
    prefix_lines: usize,
    projection: &str,
) -> (
    Context,
    neovm_core::window::FrameId,
    BufferId,
    LayoutEngine,
    usize,
    usize,
) {
    let prefix = LINE.repeat(prefix_lines);
    let hidden = "hidden Org drawer content\n".repeat(80);
    let hidden_start = prefix.len();
    let hidden_end = hidden_start + hidden.len();
    let text = format!("{prefix}{hidden}{}", LINE.repeat(120));
    let (mut eval, frame, buffer, _) = incr_editing_frame(&text, 800, 245);
    realize_test_gui_frame(&mut eval, frame);
    {
        let buffer = eval.buffer_manager_mut().get_mut(buffer).unwrap();
        buffer.set_buffer_local("scroll-conservatively", Value::fixnum(101));
        buffer.set_buffer_local("scroll-margin", Value::fixnum(2));
        buffer.set_buffer_local("display-line-numbers", Value::NIL);
        buffer.set_buffer_local("mode-line-format", Value::string("ML"));
        buffer.set_buffer_local("bidi-paragraph-direction", Value::symbol("left-to-right"));
        buffer.set_buffer_local(
            "buffer-invisibility-spec",
            Value::list(vec![Value::cons(
                Value::symbol("org-fold-outline"),
                Value::T,
            )]),
        );
        if projection == "text-property" {
            assert!(buffer.put_text_property(
                hidden_start,
                hidden_end,
                Value::symbol("invisible"),
                Value::symbol("org-fold-outline"),
            ));
        } else if projection == "replacement" {
            assert!(buffer.put_text_property(
                hidden_start,
                hidden_end,
                Value::symbol("display"),
                Value::string("[folded replacement]\n"),
            ));
        }
    }
    if projection == "overlay" {
        eval.eval_str(&format!(
            "(overlay-put (make-overlay {} {}) 'invisible 'org-fold-outline)",
            hidden_start + 1,
            hidden_end + 1,
        ))
        .unwrap();
    }
    let mut engine = LayoutEngine::new();
    engine.layout_frame_rust(&mut eval, frame);
    activate_last_engine_presentation(&mut eval, &engine, frame);
    (eval, frame, buffer, engine, hidden_start, hidden_end)
}

#[test]
fn point_scroll_across_org_folds_and_replacements_matches_full_layout() {
    // Four lines put the projection inside the retained rows; thirty leave it
    // beyond the measured viewport, where source-line extrapolation is unsafe.
    for prefix_lines in [4, 30] {
        for projection in ["text-property", "overlay", "replacement"] {
            let (mut eval, frame, buffer, mut engine, hidden_start, hidden_end) =
                projected_fixture(prefix_lines, projection);
            let (mut control, control_frame, control_buffer, mut full, _, _) =
                projected_fixture(prefix_lines, projection);
            engine.layout_frame_rust(&mut eval, frame);
            activate_last_engine_presentation(&mut eval, &engine, frame);
            for tail_line in [16, 17, 18] {
                let target = hidden_end + tail_line * LINE.len();
                move_point(&mut eval, buffer, target);
                move_point(&mut control, control_buffer, target);
                full.retained_window_matrices.clear();
                full.prepared_viewports = PreparedViewports::default();
                engine.layout_frame_rust(&mut eval, frame);
                full.layout_frame_rust(&mut control, control_frame);
                let actual = selected_window_layout_trace(&eval, &engine, frame);
                assert_eq!(
                    actual,
                    selected_window_layout_trace(&control, &full, control_frame),
                    "{projection}, prefix={prefix_lines}, tail={tail_line}, stats={:?}",
                    engine.last_layout_stats(),
                );
                let start = actual.window_start.as_i64() as usize - 1;
                assert!(start <= hidden_start || start >= hidden_end);
                assert!(
                    actual.phys_cursor.is_some(),
                    "point after a fold must stay visible"
                );
                assert!(
                    trace_text_rows(&actual)
                        .iter()
                        .all(|row| !row.contains("hidden Org"))
                );
                activate_last_engine_presentation(&mut eval, &engine, frame);
                activate_last_engine_presentation(&mut control, &full, control_frame);
            }
        }
    }
}

fn policy_interleaving_fixture() -> (
    Context,
    neovm_core::window::FrameId,
    BufferId,
    BufferId,
    LayoutEngine,
) {
    let (mut eval, frame, buffer, target, mut engine) = fixture(480, 0);
    eval.buffer_manager_mut()
        .get_mut(buffer)
        .unwrap()
        .set_buffer_local("scroll-conservatively", Value::fixnum(0));
    let earlier_buffer = eval
        .buffer_manager_mut()
        .create_buffer(" *point-scroll-earlier*");
    {
        let buffer = eval.buffer_manager_mut().get_mut(earlier_buffer).unwrap();
        buffer.insert("earlier sibling\n");
        buffer.set_modified(false);
    }
    let earlier = eval
        .frame_manager_mut()
        .split_window(
            frame,
            target,
            neovm_core::window::SplitDirection::Horizontal,
            earlier_buffer,
            None,
            neovm_core::window::SplitPlacement::BeforeTarget,
        )
        .expect("earlier sibling");
    let leaves = eval.frame_manager().get(frame).unwrap().root_leaf_ids();
    assert_eq!(&leaves[..2], &[earlier, target]);
    eval.set_variable("point-scroll-policy-target", Value::make_buffer(buffer));
    eval.set_variable("point-scroll-policy-armed", Value::NIL);
    eval.set_variable("point-scroll-policy-mutations", Value::fixnum(0));
    let mode_line = eval
        .eval_str(
            "'((:eval (progn
             (if point-scroll-policy-armed
               (progn
                 (setq point-scroll-policy-armed nil)
                 (setq point-scroll-policy-mutations (1+ point-scroll-policy-mutations))
                 (save-current-buffer
                   (set-buffer point-scroll-policy-target)
                   (set (make-local-variable 'scroll-conservatively) 101))))
             \"EARLIER\")))",
        )
        .unwrap();
    eval.buffer_manager_mut()
        .get_mut(earlier_buffer)
        .unwrap()
        .set_buffer_local("mode-line-format", mode_line);
    engine.layout_frame_rust(&mut eval, frame);
    activate_last_engine_presentation(&mut eval, &engine, frame);
    (eval, frame, buffer, earlier_buffer, engine)
}

fn proposed_forward_scroll(
    eval: &mut Context,
    frame: neovm_core::window::FrameId,
    engine: &LayoutEngine,
) -> i64 {
    let target = eval.frame_manager().get(frame).unwrap().selected_window;
    let path = eval
        .frame_manager()
        .leaf_window_paths(frame)
        .unwrap()
        .into_iter()
        .find(|(window, _)| *window == target)
        .unwrap()
        .1;
    let inputs = collect_live_window_layout_inputs(
        eval,
        frame,
        target,
        &path,
        None,
        engine.font_sizing,
        &engine.retained_window_chrome_metrics,
        WindowLayoutWalkPurpose::Redisplay,
    )
    .unwrap();
    let geometry = WindowFrameGeometryRequest::new(
        &inputs.window,
        &inputs.frame,
        inputs.root_right_edge,
        inputs.main_area_bottom,
    )
    .resolve();
    let layout_box = WindowLayoutBox::resolve(
        &inputs.window,
        WindowChromeMetrics::from_params(&inputs.window),
        WindowDividerLayout::resolve(&inputs.window, &inputs.frame, geometry),
    );
    let start = resolve_leaf_window_start(
        eval,
        &inputs.window,
        &inputs.frame,
        &layout_box,
        WindowPositionPublication::Redisplay,
        false,
    );
    assert!(start.get() > inputs.window.window_start);
    let mut resolved = inputs.window;
    resolved.window_start = start.get();
    engine
        .build_scroll_replay(&resolved, layout_box, eval)
        .expect("canonical forward placement must admit a retained scroll plan");
    start.get()
}

#[test]
fn point_scroll_revalidates_policy_changed_by_an_earlier_sibling() {
    let (mut eval, frame, buffer, earlier_buffer, mut engine) = policy_interleaving_fixture();
    let (mut control, control_frame, control_buffer, control_earlier, mut full) =
        policy_interleaving_fixture();
    engine.layout_frame_rust(&mut eval, frame);
    activate_last_engine_presentation(&mut eval, &engine, frame);
    let target = first_line_beyond_previous_viewport(&eval, &engine, frame);
    move_point(&mut eval, buffer, target);
    move_point(&mut control, control_buffer, target);
    let original_start = proposed_forward_scroll(&mut eval, frame, &engine);
    eval.set_variable("point-scroll-policy-armed", Value::T);
    control
        .buffer_manager_mut()
        .get_mut(control_buffer)
        .unwrap()
        .set_buffer_local("scroll-conservatively", Value::fixnum(101));
    // Flip the earlier buffer's modified star so GNU's chrome gate must enter
    // its :eval after phase A and before the selected leaf consumes its plan.
    for (context, earlier) in [(&mut eval, earlier_buffer), (&mut control, control_earlier)] {
        context
            .buffer_manager_mut()
            .get_mut(earlier)
            .unwrap()
            .insert("x");
    }
    full.retained_window_matrices.clear();
    full.prepared_viewports = PreparedViewports::default();
    let display_revision = eval.display_var_change_count;
    engine.layout_frame_rust(&mut eval, frame);
    full.layout_frame_rust(&mut control, control_frame);
    assert_eq!(
        eval.display_var_change_count, display_revision,
        "a policy-only change must exercise phase-B revalidation, not generic key invalidation"
    );
    assert_eq!(
        eval.eval_str("point-scroll-policy-mutations")
            .unwrap()
            .as_fixnum(),
        Some(1)
    );
    let actual = selected_window_layout_trace(&eval, &engine, frame);
    assert_ne!(
        actual.window_start.as_i64() - 1,
        original_start,
        "conservative scrolling must replace the recentered phase-A placement"
    );
    assert_eq!(
        actual,
        selected_window_layout_trace(&control, &full, control_frame)
    );
    assert_eq!(
        engine.last_layout_stats().scroll_windows,
        0,
        "the stale recentering plan must not be accepted"
    );
}

struct PointScrollProbeGuard;

impl PointScrollProbeGuard {
    fn new() -> Self {
        super::super::viewport_retry_depth_probe::reset();
        Self
    }
}

impl Drop for PointScrollProbeGuard {
    fn drop(&mut self) {
        super::super::viewport_retry_depth_probe::reset();
    }
}

fn install_scroll_log(eval: &mut Context) {
    eval.eval_str(
        "(progn
           (setq point-scroll-fallback-starts nil)
           (setq window-scroll-functions
             (list (lambda (_window start)
               (setq point-scroll-fallback-starts
                 (cons start point-scroll-fallback-starts))))))",
    )
    .unwrap();
}

#[test]
fn point_scroll_tall_exposed_row_rejects_replay_and_matches_full_fallback() {
    let _probe = PointScrollProbeGuard::new();
    for with_hook in [false, true] {
        let (mut eval, frame, buffer, _, mut engine) = fixture(240, 0);
        let (mut control, control_frame, control_buffer, _, mut full) = fixture(240, 0);
        let target = next_unshown_line(&eval, &engine, frame) + 3 * LINE.len();
        for (context, owner) in [(&mut eval, buffer), (&mut control, control_buffer)] {
            assert!(
                context
                    .buffer_manager_mut()
                    .get_mut(owner)
                    .unwrap()
                    .put_text_property(
                        target,
                        target + LINE.len() - 1,
                        Value::symbol("face"),
                        Value::list(vec![Value::keyword("height"), Value::make_float(4.0)]),
                    )
            );
        }
        // The tall row is outside both accepted viewports when retention starts.
        engine.layout_frame_rust(&mut eval, frame);
        full.layout_frame_rust(&mut control, control_frame);
        activate_last_engine_presentation(&mut eval, &engine, frame);
        activate_last_engine_presentation(&mut control, &full, control_frame);
        if with_hook {
            install_scroll_log(&mut eval);
            install_scroll_log(&mut control);
        }
        move_point(&mut eval, buffer, target);
        move_point(&mut control, control_buffer, target);
        proposed_forward_scroll(&mut eval, frame, &engine);
        full.retained_window_matrices.clear();
        full.prepared_viewports = PreparedViewports::default();
        super::super::viewport_retry_depth_probe::reset();
        engine.layout_frame_rust(&mut eval, frame);
        let rejections = super::super::viewport_retry_depth_probe::point_scroll_rejections();
        if !with_hook {
            assert!(
                rejections > 0,
                "the naturally clipped tall row must reject a point replay"
            );
        }
        // A scroll hook consumes the phase-A plan while re-entering the leaf.
        // Its counterpart still must publish exactly the full walk's starts.
        full.layout_frame_rust(&mut control, control_frame);
        let actual = selected_window_layout_trace(&eval, &engine, frame);
        assert_eq!(
            actual,
            selected_window_layout_trace(&control, &full, control_frame)
        );
        assert!(actual.phys_cursor.is_some());
        assert_eq!(engine.last_layout_stats().scroll_windows, 0);
        if with_hook {
            let starts = eval
                .eval_str("(format \"%S\" point-scroll-fallback-starts)")
                .unwrap();
            let expected = control
                .eval_str("(format \"%S\" point-scroll-fallback-starts)")
                .unwrap();
            assert_eq!(
                starts.as_str_owned(),
                expected.as_str_owned(),
                "replay fallback must not add or omit scroll-hook publications"
            );
            assert!(
                eval.eval_str("(length point-scroll-fallback-starts)")
                    .unwrap()
                    .as_fixnum()
                    .unwrap()
                    > 0
            );
        }
    }
}

#[test]
fn point_scroll_at_eob_matches_full_layout_with_and_without_final_newline() {
    for final_newline in [false, true] {
        let mut text = LINE.repeat(25);
        if !final_newline {
            text.pop();
        }
        let (mut eval, frame, buffer, _, mut engine) = fixture_with_text(&text, 245, 2);
        let (mut control, control_frame, control_buffer, _, mut full) =
            fixture_with_text(&text, 245, 2);
        engine.layout_frame_rust(&mut eval, frame);
        activate_last_engine_presentation(&mut eval, &engine, frame);
        for target in [24 * LINE.len(), text.len()] {
            move_point(&mut eval, buffer, target);
            move_point(&mut control, control_buffer, target);
            full.retained_window_matrices.clear();
            full.prepared_viewports = PreparedViewports::default();
            engine.layout_frame_rust(&mut eval, frame);
            full.layout_frame_rust(&mut control, control_frame);
            let actual = selected_window_layout_trace(&eval, &engine, frame);
            assert_eq!(
                actual,
                selected_window_layout_trace(&control, &full, control_frame),
                "final newline={final_newline}, target={target}, stats={:?}",
                engine.last_layout_stats()
            );
            assert!(
                actual.phys_cursor.is_some(),
                "EOB must retain a visible cursor"
            );
            activate_last_engine_presentation(&mut eval, &engine, frame);
            activate_last_engine_presentation(&mut control, &full, control_frame);
        }
    }
}

fn install_mixed_height_chrome(eval: &mut Context, buffer: BufferId) {
    let buffer = eval.buffer_manager_mut().get_mut(buffer).unwrap();
    for (name, text, height) in [
        ("header-line-format", "HEADER", 1.5),
        ("tab-line-format", "TAB", 2.0),
    ] {
        let string = Value::string_with_text_properties(
            text,
            vec![StringTextPropertyRun {
                start: 0,
                end: text.len(),
                plist: Value::list(vec![
                    Value::symbol("face"),
                    Value::list(vec![Value::keyword("height"), Value::make_float(height)]),
                ]),
            }],
        );
        buffer.set_buffer_local(name, string);
    }
    for line in (2..60).step_by(4) {
        assert!(buffer.put_text_property(
            line * LINE.len(),
            (line + 1) * LINE.len() - 1,
            Value::symbol("face"),
            Value::list(vec![Value::keyword("height"), Value::make_float(2.0)]),
        ));
    }
}

#[test]
fn point_scroll_with_header_tab_and_mixed_body_heights_matches_full_layout() {
    let (mut eval, frame, buffer, _, mut engine) = fixture(480, 2);
    let (mut control, control_frame, control_buffer, _, mut full) = fixture(480, 2);
    engine.layout_frame_rust(&mut eval, frame);
    activate_last_engine_presentation(&mut eval, &engine, frame);
    install_mixed_height_chrome(&mut eval, buffer);
    install_mixed_height_chrome(&mut control, control_buffer);
    engine.layout_frame_rust(&mut eval, frame);
    full.layout_frame_rust(&mut control, control_frame);
    activate_last_engine_presentation(&mut eval, &engine, frame);
    activate_last_engine_presentation(&mut control, &full, control_frame);
    let initial = selected_window_layout_trace(&eval, &engine, frame);
    for role in [GlyphRowRole::HeaderLine, GlyphRowRole::TabLine] {
        assert!(initial.matrix_rows.iter().any(|row| row.role == role));
    }
    let heights: std::collections::BTreeSet<_> = initial
        .matrix_rows
        .iter()
        .filter(|row| row.role == GlyphRowRole::Text && row.displays_text)
        .map(|row| row.height_px_bits)
        .collect();
    assert!(
        heights.len() > 1,
        "the fixture must actually measure mixed body heights"
    );
    for _ in 0..3 {
        let target = next_unshown_line(&eval, &engine, frame);
        move_point(&mut eval, buffer, target);
        move_point(&mut control, control_buffer, target);
        full.retained_window_matrices.clear();
        full.prepared_viewports = PreparedViewports::default();
        engine.layout_frame_rust(&mut eval, frame);
        full.layout_frame_rust(&mut control, control_frame);
        assert_eq!(
            selected_window_layout_trace(&eval, &engine, frame),
            selected_window_layout_trace(&control, &full, control_frame)
        );
        activate_last_engine_presentation(&mut eval, &engine, frame);
        activate_last_engine_presentation(&mut control, &full, control_frame);
    }
}

#[test]
fn point_scroll_margin_cannot_reuse_an_old_page_cursor() {
    let (mut eval, frame, buffer, window, mut engine) = fixture(256, 2);
    for start in [0, 60, 0] {
        scroll_window_to(
            &mut eval,
            frame,
            window,
            buffer,
            (start * LINE.len()) as i64 + 1,
            (start + 4) * LINE.len(),
        );
        engine.layout_frame_rust(&mut eval, frame);
        activate_last_engine_presentation(&mut eval, &engine, frame);
    }
    for line in [9, 10, 9] {
        move_point(&mut eval, buffer, line * LINE.len());
        engine.layout_frame_rust(&mut eval, frame);
        let actual = selected_window_layout_trace(&eval, &engine, frame);
        assert_eq!(engine.last_layout_stats().prepared_windows, 0);
        let mut full = LayoutEngine::new();
        full.layout_frame_rust(&mut eval, frame);
        assert_eq!(actual, selected_window_layout_trace(&eval, &full, frame));
        activate_last_engine_presentation(&mut eval, &engine, frame);
    }
}
