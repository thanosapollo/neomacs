use super::{Context, Value, runtime_startup_context};
use crate::window::{FrameId, Rect, WindowId};

fn configure_frame(ev: &mut Context, fid: FrameId, border: i64, chrome: bool) -> WindowId {
    let frame = ev.frames.get_mut(fid).expect("frame");
    frame.set_window_system(Some(Value::symbol("neo")));
    frame.width = 842;
    frame.height = 676;
    frame.char_width = 10.0;
    frame.char_height = 23.0;
    frame.set_parameter(
        Value::symbol("internal-border-width"),
        Value::fixnum(border),
    );
    frame.displays_chrome = chrome;
    frame.menu_bar_height = 23;
    frame.tool_bar_height = 16;
    frame.tab_bar_height = 23;
    frame.compact_bar_height = 0;
    frame
        .minibuffer_leaf
        .as_mut()
        .expect("minibuffer")
        .set_bounds(Rect::new(0.0, 0.0, 842.0, 23.0));
    frame.sync_window_area_bounds();
    let wid = frame.selected_window;
    let display = frame
        .find_window_mut(wid)
        .expect("window")
        .display_mut()
        .expect("display");
    display.left_fringe_width = 8;
    display.right_fringe_width = 8;
    display.scroll_bar_width = 10;
    display.vertical_scroll_bar_type = Value::symbol("right");
    display.horizontal_scroll_bar_type = Value::NIL;
    wid
}

fn context(border: i64, chrome: bool) -> (Context, FrameId, WindowId) {
    crate::test_utils::init_test_tracing();
    let mut ev = Context::new();
    let buffer = ev.buffers.create_buffer("*raw-origin*");
    ev.buffers.set_current(buffer);
    let fid = ev.frames.create_frame("raw-origin", 842, 676, buffer);
    let wid = configure_frame(&mut ev, fid, border, chrome);
    (ev, fid, wid)
}

fn raw_origin(ev: &mut Context, args: Vec<Value>) -> (i64, i64) {
    (
        super::super::builtin_window_pixel_left(ev, args.clone())
            .expect("raw left")
            .as_int()
            .expect("integer"),
        super::super::builtin_window_pixel_top(ev, args)
            .expect("raw top")
            .as_int()
            .expect("integer"),
    )
}

#[test]
fn border_eight_keeps_physical_bounds_and_reports_raw_origin_before_presentation() {
    let (mut ev, fid, wid) = context(8, true);
    let frame = ev.frames.get(fid).expect("frame");
    assert!(frame.active_presentation_geometry().is_none());
    assert_eq!(
        *frame.find_window(wid).expect("window").bounds(),
        Rect::new(8.0, 70.0, 826.0, 575.0)
    );
    let mini = frame.minibuffer_window.expect("minibuffer");
    assert_eq!(
        *frame.find_window(mini).expect("minibuffer").bounds(),
        Rect::new(8.0, 645.0, 826.0, 23.0)
    );
    assert_eq!(raw_origin(&mut ev, vec![]), (0, 62));
    assert_eq!(raw_origin(&mut ev, vec![Value::NIL]), (0, 62));
    assert_eq!(
        raw_origin(&mut ev, vec![Value::make_window(wid.0)]),
        (0, 62)
    );
    assert_eq!(
        raw_origin(&mut ev, vec![Value::make_window(mini.0)]),
        (0, 637)
    );
    assert_eq!(
        super::super::builtin_window_pixel_width(&mut ev, vec![]).unwrap(),
        Value::fixnum(826)
    );
    assert_eq!(
        super::super::builtin_window_pixel_height(&mut ev, vec![]).unwrap(),
        Value::fixnum(575)
    );
}

fn lisp_body_case(header_and_tab: bool) {
    crate::test_utils::init_test_tracing();
    let mut ev = runtime_startup_context();
    let fid = ev.frames.selected_frame().expect("selected frame").id;
    let wid = configure_frame(&mut ev, fid, 8, true);
    // The real GNU window.el functions must be loaded, not replaced by a test
    // reconstruction of their border/chrome composition.
    assert_eq!(
        ev.eval_str("(subrp (symbol-function 'window-edges))")
            .unwrap(),
        Value::NIL
    );
    let (top, height) = if header_and_tab {
        (92.0, 530.0)
    } else {
        (70.0, 552.0)
    };
    let body = neomacs_display_protocol::types::Rect::new(16.0, top, 800.0, height);
    let outer = neomacs_display_protocol::types::Rect::new(8.0, 70.0, 826.0, 575.0);
    let mode = neomacs_display_protocol::types::Rect::new(8.0, 622.0, 826.0, 23.0);
    let frame = ev.frames.get_mut(fid).expect("frame");
    frame
        .prepare_and_activate_display_presentation_for_test(
            crate::window::geometry::PresentationId::new(1),
            vec![crate::window::WindowDisplaySnapshot {
                window_id: wid,
                regions_materialized: true,
                regions: crate::window::PresentedWindowRegions {
                    outer,
                    text_body: body,
                    left_fringe: Some(neomacs_display_protocol::types::Rect::new(
                        8.0, top, 8.0, height,
                    )),
                    right_fringe: Some(neomacs_display_protocol::types::Rect::new(
                        816.0, top, 8.0, height,
                    )),
                    right_scroll_bar: Some(neomacs_display_protocol::types::Rect::new(
                        824.0, top, 10.0, height,
                    )),
                    header_line: header_and_tab.then_some(
                        neomacs_display_protocol::types::Rect::new(8.0, 87.0, 826.0, 5.0),
                    ),
                    tab_line: header_and_tab.then_some(neomacs_display_protocol::types::Rect::new(
                        8.0, 70.0, 826.0, 17.0,
                    )),
                    mode_line: Some(mode),
                    ..Default::default()
                },
                // Deliberately stale scalars: the accepted partition is authoritative.
                header_line_height: 999,
                tab_line_height: 999,
                mode_line_height: 999,
                ..Default::default()
            }],
        )
        .expect("accept body partition");
    let result = ev.eval_str("(list (window-pixel-edges) (window-inside-pixel-edges) (window-body-width nil t) (window-body-height nil t) (window-body-width) (window-body-height))").expect("GNU Lisp body composition");
    let printed = crate::emacs_core::print::print_value(&result);
    assert_eq!(
        printed,
        format!(
            "((8 70 834 645) (16 {} 816 622) 800 {} 80 {})",
            top as i64,
            height as i64,
            (height / 23.0) as i64
        )
    );
    let regions = ev
        .frames
        .get(fid)
        .unwrap()
        .redisplay_snapshot(wid)
        .unwrap()
        .regions
        .clone();
    assert_eq!(regions.text_body, body);
    assert_eq!(regions.mode_line, Some(mode));
    assert_eq!(regions.text_body.y + regions.text_body.height, mode.y);
    assert_eq!(
        *ev.frames
            .get(fid)
            .unwrap()
            .find_window(wid)
            .unwrap()
            .bounds(),
        Rect::new(8.0, 70.0, 826.0, 575.0)
    );
}

#[test]
fn border_eight_gnu_lisp_body_edges_match_accepted_eighty_by_twenty_four_partition() {
    lisp_body_case(false);
}

#[test]
fn border_eight_gnu_lisp_body_edges_keep_nonzero_header_and_tab_lines() {
    lisp_body_case(true);
}

#[test]
fn zero_border_and_undisplayed_chrome_keep_raw_coordinates() {
    let (mut ev, _, _) = context(0, true);
    assert_eq!(raw_origin(&mut ev, vec![]), (0, 62));
    let (mut ev, fid, wid) = context(0, false);
    assert_eq!(
        *ev.frames
            .get(fid)
            .unwrap()
            .find_window(wid)
            .unwrap()
            .bounds(),
        Rect::new(0.0, 0.0, 842.0, 653.0)
    );
    assert_eq!(raw_origin(&mut ev, vec![]), (0, 0));
    let (mut ev, _, _) = context(8, false);
    assert_eq!(raw_origin(&mut ev, vec![]), (0, 0));
}

#[test]
fn tty_ignores_stored_borders_and_preserves_character_projection() {
    let (mut ev, fid, wid) = context(8, true);
    let frame = ev.frames.get_mut(fid).unwrap();
    frame.set_window_system(None);
    frame.set_parameter(Value::symbol("child-frame-border-width"), Value::fixnum(3));
    frame
        .find_window_mut(wid)
        .unwrap()
        .set_bounds(Rect::new(40.0, 69.0, 400.0, 230.0));
    assert_eq!(frame.internal_border_width(), 0);
    assert_eq!(raw_origin(&mut ev, vec![]), (4, 3));
    let frame = ev.frames.get_mut(fid).unwrap();
    frame.char_width = 0.0;
    frame.char_height = 0.0;
    assert_eq!(raw_origin(&mut ev, vec![]), (0, 0));
}

#[test]
fn child_effective_border_precedence_fallback_and_clamping() {
    let (mut ev, parent, _) = context(8, true);
    let buffer = ev.buffers.current_buffer_id().expect("buffer");
    let child = ev.frames.create_frame("child", 842, 676, buffer);
    let wid = configure_frame(&mut ev, child, 8, true);
    for (child_border, effective) in [(Some(3), 3), (None, 8), (Some(-1), 0)] {
        let frame = ev.frames.get_mut(child).unwrap();
        frame.parent_frame = Value::make_frame(parent.0);
        match child_border {
            Some(border) => frame.set_parameter(
                Value::symbol("child-frame-border-width"),
                Value::fixnum(border),
            ),
            None => {
                frame.remove_parameter(Value::symbol("child-frame-border-width"))
            }
        };
        frame.sync_window_area_bounds();
        assert_eq!(frame.internal_border_width(), effective);
        assert_eq!(frame.find_window(wid).unwrap().bounds().x, effective as f32);
        assert_eq!(
            frame.find_window(wid).unwrap().bounds().y,
            62.0 + effective as f32
        );
        assert_eq!(
            raw_origin(&mut ev, vec![Value::make_window(wid.0)]),
            (0, 62)
        );
    }
    let frame = ev.frames.get_mut(parent).unwrap();
    frame.set_parameter(Value::symbol("child-frame-border-width"), Value::fixnum(3));
    frame.sync_window_area_bounds();
    assert_eq!(
        frame.internal_border_width(),
        8,
        "top-level ignores child-only parameter"
    );
    assert_eq!(raw_origin(&mut ev, vec![]), (0, 62));
}

#[test]
fn split_leaf_internal_and_selected_origins_retain_relative_offsets() {
    let (mut ev, fid, _) = context(8, true);
    let new = ev
        .eval_str("(split-window-internal (selected-window) 300 t nil)")
        .expect("horizontal split");
    let root = ev.eval_str("(frame-root-window)").unwrap();
    assert_eq!(
        ev.eval_str("(window-live-p (frame-root-window))").unwrap(),
        Value::NIL
    );
    let frame = ev.frames.get(fid).unwrap();
    let new_id = WindowId(new.as_window_id().expect("new window"));
    assert_eq!(frame.find_window(new_id).unwrap().bounds().x, 534.0);
    assert_eq!(raw_origin(&mut ev, vec![new]), (526, 62));
    assert_eq!(raw_origin(&mut ev, vec![root]), (0, 62));
    // Explicit selection of the split leaf must also drive nil/omitted WINDOW.
    ev.frames.get_mut(fid).unwrap().select_window(new_id);
    assert_eq!(raw_origin(&mut ev, vec![]), (526, 62));
    assert_eq!(raw_origin(&mut ev, vec![Value::NIL]), (526, 62));
}

#[test]
fn raw_origins_use_new_logical_bounds_not_stale_presentation() {
    let (mut ev, fid, wid) = context(8, true);
    ev.frames
        .get_mut(fid)
        .unwrap()
        .prepare_and_activate_display_presentation_for_test(
            crate::window::geometry::PresentationId::new(1),
            vec![crate::window::WindowDisplaySnapshot {
                window_id: wid,
                regions_materialized: true,
                regions: crate::window::PresentedWindowRegions {
                    outer: neomacs_display_protocol::types::Rect::new(8.0, 70.0, 826.0, 575.0),
                    text_body: neomacs_display_protocol::types::Rect::new(16.0, 70.0, 800.0, 552.0),
                    ..Default::default()
                },
                ..Default::default()
            }],
        )
        .expect("presentation");
    ev.frames
        .get_mut(fid)
        .unwrap()
        .find_window_mut(wid)
        .unwrap()
        .set_bounds(Rect::new(101.0, 202.0, 400.0, 300.0));
    assert_eq!(raw_origin(&mut ev, vec![]), (93, 194));
}

#[test]
fn raw_origin_errors_keep_valid_window_predicate_and_arity() {
    let (mut ev, _, _) = context(8, true);
    for function in ["window-pixel-left", "window-pixel-top"] {
        for argument in ["t", "17", "\"bad\"", "(selected-frame)"] {
            let form = format!(
                "(condition-case err ({function} {argument}) (error (list (car err) (car (cdr err)))))"
            );
            let result = ev.eval_str(&form).unwrap();
            assert_eq!(
                crate::emacs_core::print::print_value(&result),
                "(wrong-type-argument window-valid-p)"
            );
        }
        let form = format!("(condition-case err ({function} nil nil) (error (car err)))");
        assert_eq!(
            ev.eval_str(&form).unwrap(),
            Value::symbol("wrong-number-of-arguments")
        );
    }
    let deleted = ev
        .eval_str("(split-window-internal (selected-window) 300 nil nil)")
        .unwrap();
    ev.set_variable("raw-origin-deleted-window", deleted);
    ev.eval_str("(delete-window-internal raw-origin-deleted-window)")
        .unwrap();
    for function in ["window-pixel-left", "window-pixel-top"] {
        let form = format!(
            "(condition-case err ({function} raw-origin-deleted-window) (error (list (car err) (car (cdr err)))))"
        );
        assert_eq!(
            crate::emacs_core::print::print_value(&ev.eval_str(&form).unwrap()),
            "(wrong-type-argument window-valid-p)"
        );
    }
}
