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
fn point_scroll_measured_clipped_bottom_reuses_repeated_next_lines() {
    measured_next_line_trace(false, false);
}

#[test]
fn point_scroll_measured_blank_org_lines_reuse_and_reject_uncertified_rows() {
    measured_next_line_trace(true, false);
}

#[test]
fn point_scroll_measured_blank_org_fringe_reuses_with_exact_traces() {
    measured_next_line_trace(true, true);
}

fn measured_next_line_trace(rich_org: bool, fringe: bool) {
    let mut text = String::new();
    let mut starts = Vec::new();
    for line in 0..200 {
        starts.push(text.len());
        text.push_str(if rich_org && line % 4 == 0 {
            "\n"
        } else {
            LINE
        });
    }
    let (mut eval, frame, buffer, window, mut engine) = fixture_with_text(&text, 245, 0);
    let (mut control, control_frame, control_buffer, _, mut full) =
        fixture_with_text(&text, 245, 0);
    for (context, owner) in [(&mut eval, buffer), (&mut control, control_buffer)] {
        let buffer = context.buffer_manager_mut().get_mut(owner).unwrap();
        buffer.set_buffer_local("scroll-conservatively", Value::fixnum(0));
        // Display-space height preserves subpixel geometry, unlike face
        // sizes which the native font backend quantizes to device pixels.
        for line in (2..200).step_by(4) {
            assert!(buffer.put_text_property(
                starts[line],
                starts[line] + 1,
                Value::symbol("display"),
                Value::list(vec![
                    Value::symbol("space"),
                    Value::keyword("relative-height"),
                    Value::make_float(1.025)
                ])
            ));
        }
        if rich_org {
            // Leave the newline-only rows undecorated: real Org's empty Text
            // rows have no glyphs, displays_text=false, and start=end anchors.
            for line in (1..200).step_by(4) {
                assert!(buffer.put_text_property(
                    starts[line],
                    starts[line] + LINE.len() - 1,
                    Value::symbol("face"),
                    Value::list(vec![Value::keyword("foreground"), Value::string("#426789")]),
                ));
                if fringe {
                    // Source-produced decorations must survive shifted reuse
                    // with their exact bitmap, face and source precedence.
                    assert!(buffer.put_text_property(
                        starts[line],
                        starts[line] + 1,
                        Value::symbol("display"),
                        Value::list(vec![
                            Value::symbol("left-fringe"),
                            Value::symbol("right-triangle"),
                            Value::symbol("default")
                        ]),
                    ));
                }
            }
            for line in (3..200).step_by(4) {
                assert!(buffer.put_text_property(
                    starts[line],
                    starts[line] + 1,
                    Value::symbol("display"),
                    Value::string("* Org heading "),
                ));
            }
        }
    }
    engine.layout_frame_rust(&mut eval, frame);
    full.layout_frame_rust(&mut control, control_frame);
    activate_last_engine_presentation(&mut eval, &engine, frame);
    activate_last_engine_presentation(&mut control, &full, control_frame);
    let heights: Vec<_> = engine.retained_window_matrices[&DisplayWindowId::new(window.0 as i64)]
        .matrix
        .rows
        .iter()
        .filter(|row| row.enabled && row.role == GlyphRowRole::Text)
        .map(|row| row.height_px)
        .collect();
    if rich_org {
        let retained = &engine.retained_window_matrices[&DisplayWindowId::new(window.0 as i64)];
        assert_eq!(
            retained.matrix.rows.iter().any(|row| row.enabled
                && row.role == GlyphRowRole::Text
                && row.displays_text
                && row.left_fringe_bitmap.is_some()),
            fringe,
            "fixture must realize the requested text fringe, not only set a property"
        );
        assert!(
            trace_text_rows(&selected_window_layout_trace(&eval, &engine, frame))
                .iter()
                .any(|row| row.contains("Org heading")),
            "fixture must realize the display replacement"
        );
    }
    println!("measured body heights: {heights:?}");
    assert!(
        heights.iter().any(|height| height.fract() != 0.0),
        "fixture must measure fractional body heights"
    );
    let mut scrolls = 0;
    let mut measured_proposals = 0;
    let mut blank_proposals = 0;
    for line in 1..45 {
        let target = starts[line];
        let retained = &engine.retained_window_matrices[&DisplayWindowId::new(window.0 as i64)];
        let body = retained.key.partition.text_body();
        let clipped = retained.matrix.rows.iter().any(|row| {
            row.enabled
                && row.role == GlyphRowRole::Text
                && row.displays_text
                && row.pixel_y + retained.display_snapshot.regions.outer.y + row.height_px
                    > body.y + body.height
        });
        move_point(&mut eval, buffer, target);
        move_point(&mut control, control_buffer, target);
        let (params, frame_params, layout_box) = measured_inputs(&mut eval, frame, &engine);
        if engine
            .retained_measured_forward_start(&params, layout_box, &eval)
            .is_some()
        {
            assert_eq!(
                resolve_leaf_window_start(
                    &eval,
                    &params,
                    &frame_params,
                    &layout_box,
                    WindowPositionPublication::Redisplay,
                    false
                )
                .get(),
                params.window_start
            );
            measured_proposals += 1;
            if rich_org {
                let id = DisplayWindowId::new(window.0 as i64);
                let original = engine.retained_window_matrices[&id].clone();
                let blank = original
                    .display_snapshot
                    .rows
                    .iter()
                    .find(|row| {
                        let matrix = &original.matrix.rows[row.row as usize];
                        matrix.enabled
                            && matrix.role == GlyphRowRole::Text
                            && !matrix.displays_text
                            && !matrix.ends_at_zv
                            && row.start_buffer_pos.is_some()
                            && row.start_buffer_pos == row.end_buffer_pos
                    })
                    .expect("rich viewport must retain an anchored empty logical row");
                blank_proposals += 1;
                // Change only the admitted blank row's certification. None of
                // these invalid rows may be silently omitted or extrapolated.
                for invalid in ["eob", "missing-start", "missing-end", "non-text", "filler"] {
                    let mut mutant = original.clone();
                    let row_index = blank.row as usize;
                    match invalid {
                        "eob" => {
                            neomacs_display_protocol::MatrixRow::make_mut(
                                &mut mutant.matrix.rows[row_index],
                            )
                            .ends_at_zv = true
                        }
                        "non-text" => {
                            neomacs_display_protocol::MatrixRow::make_mut(
                                &mut mutant.matrix.rows[row_index],
                            )
                            .role = GlyphRowRole::Minibuffer
                        }
                        _ => {
                            let snapshot = std::sync::Arc::make_mut(&mut mutant.display_snapshot);
                            let row = snapshot
                                .rows
                                .iter_mut()
                                .find(|row| row.row == blank.row)
                                .unwrap();
                            if invalid != "missing-end" {
                                row.start_buffer_pos = None;
                            }
                            if invalid != "missing-start" {
                                row.end_buffer_pos = None;
                            }
                        }
                    }
                    engine.retained_window_matrices.insert(id, mutant);
                    assert!(
                        engine
                            .retained_measured_forward_start(&params, layout_box, &eval)
                            .is_none(),
                        "uncertified {invalid} blank row admitted on next line {line}"
                    );
                }
                engine.retained_window_matrices.insert(id, original);
                assert!(
                    engine
                        .retained_measured_forward_start(&params, layout_box, &eval)
                        .is_some()
                );
            }
        }
        full.retained_window_matrices.clear();
        full.prepared_viewports = PreparedViewports::default();
        engine.layout_frame_rust(&mut eval, frame);
        full.layout_frame_rust(&mut control, control_frame);
        let actual = selected_window_layout_trace(&eval, &engine, frame);
        let expected = selected_window_layout_trace(&control, &full, control_frame);
        assert_eq!(
            actual,
            expected,
            "next line {line}, stats={:?}",
            engine.last_layout_stats()
        );
        if fringe {
            let actual_rows = &engine.retained_window_matrices
                [&DisplayWindowId::new(window.0 as i64)]
                .matrix
                .rows;
            let expected_rows = &full
                .retained_window_matrices
                .values()
                .find(|retained| retained.key.buffer_id == control_buffer.0)
                .unwrap()
                .matrix
                .rows;
            let actual_rows: Vec<_> = actual_rows.iter().filter(|row| row.enabled).collect();
            let expected_rows: Vec<_> = expected_rows.iter().filter(|row| row.enabled).collect();
            assert_eq!(actual_rows.len(), expected_rows.len());
            for (actual, expected) in actual_rows.iter().zip(expected_rows) {
                assert_eq!(actual.left_fringe_bitmap, expected.left_fringe_bitmap);
                assert_eq!(actual.right_fringe_bitmap, expected.right_fringe_bitmap);
            }
        }
        let stats = engine.last_layout_stats();
        if stats.scroll_windows > 0 {
            assert!(
                clipped,
                "reuse must originate from a clipped measured viewport"
            );
            assert!(stats.reused_shifted_rows > 0, "{stats:?}");
            scrolls += 1;
        }
        activate_last_engine_presentation(&mut eval, &engine, frame);
        activate_last_engine_presentation(&mut control, &full, control_frame);
    }
    if rich_org {
        assert!(
            blank_proposals >= 3,
            "got {blank_proposals} anchored-blank measured proposals"
        );
        println!(
            "rich Org blanks: {blank_proposals} proposals, five uncertified-row controls each"
        );
    }
    assert!(
        measured_proposals >= 3,
        "got {measured_proposals} measured proposals"
    );
    println!(
        "measured next-line: {scrolls} scroll reuses / {measured_proposals} measured proposals / 44 exact full traces"
    );
    assert!(
        scrolls >= 3,
        "repeated next-line must reuse measured scrolls, got {scrolls}"
    );
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

fn policy_interleaving_fixture(
    height: u32,
) -> (
    Context,
    neovm_core::window::FrameId,
    BufferId,
    BufferId,
    LayoutEngine,
) {
    let (mut eval, frame, buffer, target, mut engine) = fixture(height, 0);
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

fn measured_inputs(
    eval: &mut Context,
    frame: neovm_core::window::FrameId,
    engine: &LayoutEngine,
) -> (WindowParams, FrameParams, WindowLayoutBox) {
    let target = eval.frame_manager().get(frame).unwrap().selected_window;
    let path = eval
        .frame_manager()
        .leaf_window_paths(frame)
        .unwrap()
        .into_iter()
        .find(|(id, _)| *id == target)
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
    (inputs.window, inputs.frame, layout_box)
}

#[test]
fn point_scroll_measured_plan_revalidates_sibling_policy_change() {
    let (mut eval, frame, buffer, earlier, mut engine) = policy_interleaving_fixture(245);
    let (mut control, control_frame, control_buffer, control_earlier, mut full) =
        policy_interleaving_fixture(245);
    engine.layout_frame_rust(&mut eval, frame);
    activate_last_engine_presentation(&mut eval, &engine, frame);
    let trace = selected_window_layout_trace(&eval, &engine, frame);
    let target = trace
        .matrix_rows
        .iter()
        .rev()
        .find(|row| row.role == GlyphRowRole::Text && row.displays_text)
        .unwrap()
        .start_charpos;
    move_point(&mut eval, buffer, target);
    move_point(&mut control, control_buffer, target);
    let (params, frame_params, layout_box) = measured_inputs(&mut eval, frame, &engine);
    let proposal = engine
        .retained_measured_forward_start(&params, layout_box, &eval)
        .expect("clipped bottom must admit measured plan");
    assert_eq!(
        resolve_leaf_window_start(
            &eval,
            &params,
            &frame_params,
            &layout_box,
            WindowPositionPublication::Redisplay,
            false
        )
        .get(),
        params.window_start
    );
    let mut proposed = params.clone();
    proposed.window_start = proposal;
    assert!(
        engine
            .build_scroll_replay(&proposed, layout_box, &eval)
            .is_some()
    );
    for (context, sibling) in [(&mut eval, earlier), (&mut control, control_earlier)] {
        let mode_line = context
            .eval_str(
                "'((:eval (progn
            (if point-scroll-policy-armed (progn
              (setq point-scroll-policy-armed nil)
              (setq point-scroll-policy-mutations (1+ point-scroll-policy-mutations))
              (save-current-buffer (set-buffer point-scroll-policy-target)
                (set (make-local-variable 'scroll-margin) 3)))) \"EARLIER\")))",
            )
            .unwrap();
        context
            .buffer_manager_mut()
            .get_mut(sibling)
            .unwrap()
            .set_buffer_local("mode-line-format", mode_line);
        context.set_variable("point-scroll-policy-armed", Value::T);
        context
            .buffer_manager_mut()
            .get_mut(sibling)
            .unwrap()
            .insert("x");
    }
    full.retained_window_matrices.clear();
    full.prepared_viewports = PreparedViewports::default();
    engine.layout_frame_rust(&mut eval, frame);
    full.layout_frame_rust(&mut control, control_frame);
    for context in [&mut eval, &mut control] {
        assert_eq!(
            context
                .eval_str("point-scroll-policy-mutations")
                .unwrap()
                .as_fixnum(),
            Some(1)
        );
    }
    assert_eq!(engine.last_layout_stats().scroll_windows, 0);
    assert_eq!(
        selected_window_layout_trace(&eval, &engine, frame),
        selected_window_layout_trace(&control, &full, control_frame)
    );
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
    let (mut eval, frame, buffer, earlier_buffer, mut engine) = policy_interleaving_fixture(480);
    let (mut control, control_frame, control_buffer, control_earlier, mut full) =
        policy_interleaving_fixture(480);
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

fn margin_cursor_fixture(
    height: u32,
) -> (
    Context,
    neovm_core::window::FrameId,
    BufferId,
    neovm_core::window::WindowId,
    LayoutEngine,
) {
    let (mut eval, frame, buffer, window, mut engine) = fixture(height, 2);
    scroll_window_to(
        &mut eval,
        frame,
        window,
        buffer,
        (20 * LINE.len()) as i64 + 1,
        24 * LINE.len(),
    );
    engine.layout_frame_rust(&mut eval, frame);
    activate_last_engine_presentation(&mut eval, &engine, frame);
    (eval, frame, buffer, window, engine)
}

#[test]
fn point_cursor_margin_interior_reuses_retained_rows() {
    // Include an oversized margin: the canonical cap, not its raw value,
    // determines the usable rows.
    for margin in [2, 100] {
        let (mut eval, frame, buffer, window, mut engine) = margin_cursor_fixture(256);
        eval.buffer_manager_mut()
            .get_mut(buffer)
            .unwrap()
            .set_buffer_local("scroll-margin", Value::fixnum(margin));
        engine.layout_frame_rust(&mut eval, frame);
        activate_last_engine_presentation(&mut eval, &engine, frame);
        let before = selected_window_layout_trace(&eval, &engine, frame).window_start;
        for line in [25, 24, 25] {
            move_point(&mut eval, buffer, line * LINE.len());
            engine.layout_frame_rust(&mut eval, frame);
            let actual = selected_window_layout_trace(&eval, &engine, frame);
            assert_eq!(actual.window_start, before);
            assert_eq!(engine.last_layout_stats().cursor_only_windows, 1);
            assert_eq!(engine.last_layout_stats().prepared_windows, 0);
            let mut full = LayoutEngine::new();
            full.layout_frame_rust(&mut eval, frame);
            assert_eq!(actual, selected_window_layout_trace(&eval, &full, frame));
            assert_eq!(
                engine.retained_window_matrices[&DisplayWindowId::new(window.0 as i64)]
                    .key
                    .window_start,
                before.as_i64() - 1
            );
            activate_last_engine_presentation(&mut eval, &engine, frame);
        }
    }
}

#[test]
fn point_cursor_margin_revalidates_policy_changed_by_an_earlier_sibling() {
    fn interleaving_fixture() -> (
        Context,
        neovm_core::window::FrameId,
        BufferId,
        BufferId,
        LayoutEngine,
    ) {
        // A 496px frame leaves whole 20px body rows and enough rows that
        // margins two and three remain distinct after the canonical cap.
        // The 480px scroll fixture clips its last row, preventing admission.
        let (mut eval, frame, buffer, earlier_buffer, mut engine) =
            policy_interleaving_fixture(496);
        {
            let buffer = eval.buffer_manager_mut().get_mut(buffer).unwrap();
            buffer.set_buffer_local("scroll-margin", Value::fixnum(2));
            buffer.set_buffer_local("scroll-conservatively", Value::fixnum(101));
        }
        let mode_line = eval
            .eval_str(
                "'((:eval (progn
                 (if point-scroll-policy-armed
                   (progn
                     (setq point-scroll-policy-armed nil)
                     (setq point-scroll-policy-mutations (1+ point-scroll-policy-mutations))
                     (save-current-buffer
                       (set-buffer point-scroll-policy-target)
                       (set (make-local-variable 'scroll-margin) 3))))
                 \"EARLIER\")))",
            )
            .unwrap();
        eval.buffer_manager_mut()
            .get_mut(earlier_buffer)
            .unwrap()
            .set_buffer_local("mode-line-format", mode_line);
        let window = eval.frame_manager().get(frame).unwrap().selected_window;
        scroll_window_to(
            &mut eval,
            frame,
            window,
            buffer,
            (20 * LINE.len()) as i64 + 1,
            24 * LINE.len(),
        );
        engine.layout_frame_rust(&mut eval, frame);
        activate_last_engine_presentation(&mut eval, &engine, frame);
        (eval, frame, buffer, earlier_buffer, engine)
    }

    let (mut eval, frame, buffer, earlier_buffer, mut engine) = interleaving_fixture();
    let (mut control, control_frame, control_buffer, control_earlier, mut full) =
        interleaving_fixture();
    // Constructing the control Context advances the process-wide character
    // table revision. Refresh both matrices after both contexts exist so
    // phase A is testing a point-only change, not unrelated key invalidation.
    for (context, frame_id, buffer_id) in [
        (&mut eval, frame, buffer),
        (&mut control, control_frame, control_buffer),
    ] {
        let window = context
            .frame_manager()
            .get(frame_id)
            .unwrap()
            .selected_window;
        scroll_window_to(
            context,
            frame_id,
            window,
            buffer_id,
            (20 * LINE.len()) as i64 + 1,
            24 * LINE.len(),
        );
    }
    engine.layout_frame_rust(&mut eval, frame);
    activate_last_engine_presentation(&mut eval, &engine, frame);
    full.layout_frame_rust(&mut control, control_frame);
    activate_last_engine_presentation(&mut control, &full, control_frame);
    let initial = selected_window_layout_trace(&eval, &engine, frame);
    let rows: Vec<_> = initial
        .matrix_rows
        .iter()
        .filter(|row| row.role == GlyphRowRole::Text && row.displays_text)
        .collect();
    // Row two is usable with margin two, but inside the raised top margin.
    // It is not an edge row, so the older first/last-row guard cannot mask
    // this phase-A/phase-B policy race.
    let target = rows[2].start_charpos;
    move_point(&mut eval, buffer, target);
    move_point(&mut control, control_buffer, target);
    let window = eval.frame_manager().get(frame).unwrap().selected_window;
    let path = eval
        .frame_manager()
        .leaf_window_paths(frame)
        .unwrap()
        .into_iter()
        .find(|(id, _)| *id == window)
        .unwrap()
        .1;
    let inputs = collect_live_window_layout_inputs(
        &mut eval,
        frame,
        window,
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
    assert_eq!(inputs.window.scroll_margin, 2);
    assert!(
        engine
            .build_cursor_only_replay(&inputs.window, layout_box, &inputs.frame, &eval)
            .is_some(),
        "the target must admit a cursor plan before the sibling raises its margin"
    );
    for (context, earlier) in [(&mut eval, earlier_buffer), (&mut control, control_earlier)] {
        context.set_variable("point-scroll-policy-armed", Value::T);
        // Changing the earlier buffer's modified star forces its :eval to run
        // after phase A and before the selected target consumes its plan.
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
        "margin revalidation must not depend on generic display invalidation"
    );
    for context in [&mut eval, &mut control] {
        assert_eq!(
            context
                .eval_str("point-scroll-policy-mutations")
                .unwrap()
                .as_fixnum(),
            Some(1),
            "the earlier sibling must raise the margin exactly once"
        );
    }
    let stats = engine.last_layout_stats();
    assert_eq!(stats.cursor_only_windows, 0, "{stats:?}");
    assert_eq!(stats.prepared_windows, 0, "{stats:?}");
    assert_eq!(stats.scroll_windows, 0, "{stats:?}");
    let actual = selected_window_layout_trace(&eval, &engine, frame);
    let expected = selected_window_layout_trace(&control, &full, control_frame);
    assert_ne!(actual.window_start, initial.window_start);
    assert_eq!(actual.window_start, expected.window_start);
    assert_eq!(actual, expected);
    assert_eq!(
        engine.retained_window_matrices[&DisplayWindowId::new(window.0 as i64)]
            .key
            .scroll_margin,
        3,
        "the accepted matrix must retain the policy used at consumption"
    );
}

#[test]
fn point_cursor_top_and_bottom_margin_fall_back() {
    for top in [true, false] {
        let (mut eval, frame, buffer, _, mut engine) = margin_cursor_fixture(256);
        let (mut control, control_frame, control_buffer, _, mut full) = margin_cursor_fixture(256);
        let initial = selected_window_layout_trace(&eval, &engine, frame);
        let rows: Vec<_> = initial
            .matrix_rows
            .iter()
            .filter(|row| row.role == GlyphRowRole::Text && row.displays_text)
            .collect();
        // Neither target is an edge row: these must reject because of the
        // effective margin, not the older first/last-row recenter guard.
        let target = rows[if top { 1 } else { rows.len() - 2 }].start_charpos;
        move_point(&mut eval, buffer, target);
        move_point(&mut control, control_buffer, target);
        full.retained_window_matrices.clear();
        full.prepared_viewports = PreparedViewports::default();
        engine.layout_frame_rust(&mut eval, frame);
        full.layout_frame_rust(&mut control, control_frame);
        let actual = selected_window_layout_trace(&eval, &engine, frame);
        assert_eq!(engine.last_layout_stats().cursor_only_windows, 0);
        assert_eq!(engine.last_layout_stats().prepared_windows, 0);
        assert_ne!(actual.window_start, initial.window_start);
        assert_eq!(
            actual,
            selected_window_layout_trace(&control, &full, control_frame)
        );
    }
}

#[test]
fn point_cursor_margin_forced_start_preserves_start_and_point_policy() {
    for top in [true, false] {
        let (mut eval, frame, buffer, window, mut engine) = margin_cursor_fixture(256);
        let (mut control, control_frame, control_buffer, control_window, mut full) =
            margin_cursor_fixture(256);
        let initial = selected_window_layout_trace(&eval, &engine, frame);
        let rows: Vec<_> = initial
            .matrix_rows
            .iter()
            .filter(|row| row.role == GlyphRowRole::Text && row.displays_text)
            .collect();
        let target = rows[if top { 0 } else { rows.len() - 1 }].start_charpos;
        for (context, frame_id, buffer_id, window_id) in [
            (&mut eval, frame, buffer, window),
            (&mut control, control_frame, control_buffer, control_window),
        ] {
            move_point(context, buffer_id, target);
            let leaf = context
                .frame_manager_mut()
                .get_mut(frame_id)
                .unwrap()
                .find_window_mut(window_id)
                .unwrap();
            if let neovm_core::window::Window::Leaf { force_start, .. } = leaf {
                *force_start = true;
            } else {
                panic!("fixture must be a leaf");
            }
        }
        full.retained_window_matrices.clear();
        full.prepared_viewports = PreparedViewports::default();
        engine.layout_frame_rust(&mut eval, frame);
        full.layout_frame_rust(&mut control, control_frame);
        let actual = selected_window_layout_trace(&eval, &engine, frame);
        assert_eq!(actual.window_start, initial.window_start);
        // Forced starts remain conservative: the canonical walk owns any
        // point relocation needed at the measured bottom margin.
        assert_eq!(engine.last_layout_stats().cursor_only_windows, 0);
        assert_eq!(
            actual,
            selected_window_layout_trace(&control, &full, control_frame)
        );
        assert_eq!(
            eval.buffer_manager()
                .get(buffer)
                .unwrap()
                .point_emacs_byte_pos(),
            control
                .buffer_manager()
                .get(control_buffer)
                .unwrap()
                .point_emacs_byte_pos()
        );
        if top {
            assert_eq!(
                eval.buffer_manager()
                    .get(buffer)
                    .unwrap()
                    .point_emacs_byte_pos(),
                EmacsBytePos::new(target)
            );
        }
    }
}

#[test]
fn point_cursor_margin_clipped_viewport_falls_back() {
    let (mut eval, frame, buffer, window, mut engine) = margin_cursor_fixture(245);
    let retained = &engine.retained_window_matrices[&DisplayWindowId::new(window.0 as i64)];
    let body = retained.key.partition.text_body();
    assert!(
        retained.matrix.rows.iter().any(|row| {
            row.enabled
                && row.role == GlyphRowRole::Text
                && row.pixel_y + retained.display_snapshot.regions.outer.y + row.height_px
                    > body.y + body.height
        }),
        "fixture must contain a clipped body row"
    );
    move_point(&mut eval, buffer, 25 * LINE.len());
    engine.layout_frame_rust(&mut eval, frame);
    assert_eq!(engine.last_layout_stats().cursor_only_windows, 0);
    let actual = selected_window_layout_trace(&eval, &engine, frame);
    let mut full = LayoutEngine::new();
    full.layout_frame_rust(&mut eval, frame);
    assert_eq!(actual, selected_window_layout_trace(&eval, &full, frame));
}
