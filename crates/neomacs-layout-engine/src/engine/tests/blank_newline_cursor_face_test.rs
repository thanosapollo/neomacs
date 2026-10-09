//! r019 minimal r017 regression port; production remains canonical.
use super::*;
use neomacs_display_protocol::face::Face;
use neomacs_display_protocol::glyph_matrix::FrameDisplayState;

fn normalize_face(mut face: Face) -> Face {
    face.id = FaceId::new(0);
    face.default_resolved_font_id = face
        .default_resolved_font_id
        .map(|_| neomacs_display_protocol::font::ResolvedFontId(0));
    face
}
fn frame_paint(state: &FrameDisplayState) -> Vec<(FrameGlyph, Option<Face>)> {
    let mut paint = Vec::new();
    state.for_each_glyph(|mut glyph| {
        let face = match &mut glyph {
            FrameGlyph::Char { face_id, .. }
            | FrameGlyph::Stretch { face_id, .. }
            | FrameGlyph::Image { face_id, .. }
            | FrameGlyph::Video { face_id, .. }
            | FrameGlyph::Xwidget { face_id, .. }
            | FrameGlyph::Surface { face_id, .. }
            | FrameGlyph::FringeBitmap { face_id, .. } => {
                let face = state.faces.get(face_id).cloned().map(normalize_face);
                *face_id = FaceId::new(0);
                face
            }
            _ => None,
        };
        paint.push((glyph, face));
    });
    paint
}

fn complete_frame_rows(state: &FrameDisplayState) -> Vec<(i64, Vec<(usize, RowTrace)>)> {
    state
        .window_matrices
        .iter()
        .map(|entry| {
            (
                entry.window_id.get(),
                entry
                    .matrix
                    .rows
                    .iter()
                    .enumerate()
                    .filter(|(_, row)| row.enabled)
                    .map(|(index, row)| (index, RowTrace::from_row(row, &state.faces)))
                    .collect(),
            )
        })
        .collect()
}

fn normalized_matrix(
    state: &neomacs_display_protocol::glyph_matrix::FrameDisplayState,
) -> Vec<(i64, Vec<GlyphRow>)> {
    state
        .window_matrices
        .iter()
        .map(|entry| {
            (
                entry.window_id.get(),
                entry
                    .matrix
                    .rows
                    .iter()
                    .filter(|row| row.enabled)
                    .map(|row| {
                        let mut row = GlyphRow::clone(row);
                        row.hash = 0;
                        for area in &mut row.glyphs {
                            for glyph in area {
                                glyph.face_id = FaceId::new(0);
                            }
                        }
                        for fringe in [&mut row.left_fringe_bitmap, &mut row.right_fringe_bitmap]
                            .into_iter()
                            .flatten()
                        {
                            fringe.face_id = FaceId::new(0);
                        }
                        row
                    })
                    .collect(),
            )
        })
        .collect()
}

fn journey_frame(
    text: &str,
    margin: usize,
    conservative: usize,
    realistic: bool,
) -> (
    Context,
    neovm_core::window::FrameId,
    BufferId,
    neovm_core::window::WindowId,
) {
    let (mut eval, frame, buffer, window) = incr_editing_frame(
        text,
        if realistic { 1040 } else { 800 },
        if realistic { 811 } else { 480 },
    );
    eval.eval_str(&format!("(setq scroll-conservatively {conservative} scroll-margin {margin} bidi-paragraph-direction 'left-to-right mode-line-format {})",
        if realistic { "'(\" Org  %b  %p \")" } else { "nil" })).unwrap();
    if realistic {
        let f = eval.frame_manager_mut().get_mut(frame).unwrap();
        f.set_window_system(Some(Value::symbol("neo")));
        f.char_height = 29.0;
        f.char_width = 13.0;
        if let neovm_core::window::Window::Leaf { bounds, .. } = f.find_window_mut(window).unwrap()
        {
            // GUI mode lines include a one-pixel border: 29 + 1 chrome
            // pixels leave exactly 753 pixels for the body after convergence.
            bounds.height = 783.0;
        }
    }
    (eval, frame, buffer, window)
}

// Pin GUI cell metrics instead of depending on the host's installed fonts.
// Unlike a terminal window, GUI layout emits the clipped final body row.
fn journey_engine(realistic: bool) -> LayoutEngine {
    if realistic {
        LayoutEngine::new_without_font_metrics()
    } else {
        LayoutEngine::new()
    }
}

fn journey_point(text: &str, line: usize) -> usize {
    1 + text
        .split_inclusive('\n')
        .take(line)
        .map(|row| row.chars().count())
        .sum::<usize>()
}

// r015: point moves one row into the bottom margin, but stays in the reused
// prefix. The clipped old bottom and the newly exposed row are the only walk.
// Keep controls before the styled blank case so a paint failure is not confused
// with a fixture that never admitted the partial-bottom production path.
#[test]
fn point_scroll_partial_bottom_blank_newline_cursor_colors_match_full() {
    let mut complete_pairs = Vec::new();
    for (blank, styled) in [(true, false), (false, true), (true, true)] {
        let text: String = (0..80)
            .map(|line| if blank && line == 23 { "\n" } else { "plain row\n" })
            .collect();
        let point = journey_point(&text, 23);
        let setup = || {
            let (mut eval, frame, buffer, window) = journey_frame(&text, 2, 101, true);
            assert!(eval.frame_manager_mut().select_frame(frame));
            eval.eval_str(
                "(progn
                   (internal-set-lisp-face-attribute 'default :foreground \"#000000\" (selected-frame))
                   (internal-set-lisp-face-attribute 'default :background \"#ffffff\" (selected-frame)))",
            )
            .expect("setup: pin default face colors");
            if styled {
                eval.eval_str(&format!(
                    "(put-text-property {point} {} 'face '(:foreground \"#123456\" :background \"#abcdef\" :extend nil))",
                    point + 1,
                ))
                .expect("setup: style just point's source character, including the blank newline");
            }
            eval.eval_str(&format!("(goto-char {})", journey_point(&text, 22)))
                .expect("setup: point at the last usable row before scrolling");
            // Face-color publication goes through modify-frame-parameters,
            // which can refresh the window area. Pin the synthetic GUI bounds
            // after Lisp setup, not before it; keep the 29-pixel cell contract.
            let f = eval.frame_manager_mut().get_mut(frame).unwrap();
            assert_eq!(f.char_height, 29.0, "setup: face colors preserve cell height");
            if let neovm_core::window::Window::Leaf { bounds, .. } = f.find_window_mut(window).unwrap() {
                bounds.height = 783.0;
            }
            (eval, frame, buffer, window)
        };
        // Separate contexts preserve the pre-redisplay viewport for the oracle;
        // a fresh engine has no retained rows and must make the full decision.
        let (mut eval, frame, _, window) = setup();
        let (mut oracle_eval, oracle_frame, _, oracle_window) = setup();
        let mut engine = journey_engine(true);
        let mut initial_oracle = journey_engine(true);
        engine.layout_frame_rust(&mut eval, frame);
        initial_oracle.layout_frame_rust(&mut oracle_eval, oracle_frame);
        activate_last_engine_presentation(&mut eval, &engine, frame);
        activate_last_engine_presentation(&mut oracle_eval, &initial_oracle, oracle_frame);
        let window_id = DisplayWindowId::new(window.0 as i64);
        let previous = &engine.retained_window_matrices[&window_id];
        let body = previous.key.partition.text_body();
        assert_eq!(body.height, 753.0, "setup: pinned GUI body height");
        assert_eq!(previous.key.window_start, 0, "setup: no initial scroll");
        let rows: Vec<_> = previous.matrix.rows.iter()
            .filter(|row| row.enabled && !RetainedWindowMatrix::is_chrome_role(row.role))
            .collect();
        assert_eq!(rows.len(), 26, "setup: 25 whole rows plus clipped bottom");
        assert!(rows.iter().all(|row| row.height_px == 29.0), "setup: fixed cell heights");
        let bottom = body.y + body.height - previous.display_snapshot.regions.outer.y;
        assert_eq!(rows.iter().filter(|row| row.pixel_y + row.height_px <= bottom).count(), 25);
        let last = rows.last().unwrap();
        assert!(last.pixel_y < bottom && last.pixel_y + last.height_px > bottom,
            "setup: genuine partial bottom row");
        let old_target = rows[23];
        let old_target_y = old_target.pixel_y;
        assert_eq!(old_target.start_charpos, point - 1, "setup: target in retained prefix");
        if blank {
            assert_eq!(old_target.end_charpos, point - 1);
            assert!(old_target.glyphs[GlyphArea::Text.index()].is_empty(),
                "setup: no glyph face from which to reconstruct blank newline paint");
            let snapshot = previous.display_snapshot.rows.iter()
                .find(|snapshot| snapshot.start_buffer_pos.map(|pos| pos.as_i64()) == Some(point as i64))
                .expect("setup: blank source snapshot");
            assert_eq!(snapshot.end_source, neovm_core::window::DisplayRowEndSource::Buffer);
            assert_eq!(snapshot.start_buffer_pos, snapshot.end_buffer_pos);
        } else {
            assert!(old_target.glyphs[GlyphArea::Text.index()].iter()
                .any(|glyph| old_target.glyph_covers_buffer_charpos(glyph, point - 1)),
                "setup: nonblank control has a point-covering glyph");
        }
        for context in [&mut eval, &mut oracle_eval] {
            context.eval_str(&format!("(goto-char {point})")).unwrap();
        }
        engine.layout_frame_rust(&mut eval, frame);
        let stats = engine.last_layout_stats();
        assert_eq!(stats.scroll_windows, 1, "path: actual production scroll reuse: {stats:?}");
        assert_eq!(stats.cursor_only_windows, 0, "path: not merely cursor-only");
        assert_eq!(stats.reused_shifted_rows, 24, "path: old whole rows 1..=24 shifted");
        assert_eq!(stats.relaid_body_rows, 2, "path: clipped old bottom plus exposed row");
        let retained = &engine.retained_window_matrices[&window_id];
        assert_eq!(retained.key.window_start, journey_point(&text, 1) as i64 - 1,
            "path: one natural row of viewport displacement");
        let target = retained.matrix.rows.iter()
            .find(|row| row.enabled && row.start_charpos == point - 1)
            .expect("path: target survived in assembled matrix");
        assert_eq!(target.pixel_y, old_target_y - 29.0,
            "path: cursor row came from the shifted prefix, not the suffix walk");
        let actual = engine.last_frame_display_state.as_ref().unwrap();
        let mut full = journey_engine(true);
        full.layout_frame_rust(&mut oracle_eval, oracle_frame);
        assert_eq!(full.last_layout_stats().scroll_windows, 0, "oracle: no retained reuse");
        assert_eq!(full.last_layout_stats().cursor_only_windows, 0);
        assert!(full.last_layout_stats().relaid_body_rows >= 26, "oracle: full body walked");
        assert_eq!(
            eval.frame_manager().get(frame).unwrap().find_window(window).unwrap().window_start(),
            oracle_eval.frame_manager().get(oracle_frame).unwrap().find_window(oracle_window).unwrap().window_start(),
            "geometry: full and incremental choose the same semantic start",
        );
        let expected = full.last_frame_display_state.as_ref().unwrap();
        let cursor = actual.phys_cursor.as_ref().expect("path: reconstructed physical cursor");
        let full_cursor = expected.phys_cursor.as_ref().expect("oracle: captured physical cursor");
        // Geometry and path failures above are distinct from the paint assertion.
        assert_eq!((cursor.charpos, cursor.row, cursor.col, cursor.slot_id),
            (full_cursor.charpos, full_cursor.row, full_cursor.col, full_cursor.slot_id),
            "geometry: source position and slot");
        assert_eq!((cursor.x, cursor.y, cursor.width, cursor.height, cursor.ascent, cursor.style),
            (full_cursor.x, full_cursor.y, full_cursor.width, full_cursor.height, full_cursor.ascent, full_cursor.style),
            "geometry: physical cursor placement");
        assert_eq!(cursor.charpos, point - 1);
        assert_eq!(cursor.col, 0);
        assert_eq!(full_cursor.cursor_fg,
            Color::from_pixel(if styled { 0x00abcdef } else { 0x00ffffff }),
            "oracle setup: source face is actually reflected in cursor paint");
        assert_eq!((cursor.color, cursor.cursor_fg), (full_cursor.color, full_cursor.cursor_fg),
            "paint: lost source face on reused row (blank={blank}, styled={styled})");
        assert_eq!(actual.phys_cursor, expected.phys_cursor);
        eprintln!("cursor control passed: blank={blank}, styled={styled}");
        // Compare complete rows in the same Context: chrome strings carry
        // source-object identities which an independent Context cannot share.
        // The separate oracle above already checked the viewport decision.
        let mut same_context_full = journey_engine(true);
        same_context_full.layout_frame_rust(&mut eval, frame);
        let same_context_expected = same_context_full.last_frame_display_state.as_ref().unwrap();
        complete_pairs.push((blank, styled, actual.clone(), same_context_expected.clone()));
    }
    // Run every cursor-color discriminator before the complete-output checks:
    // a separate whole-window fill defect must not mask the styled-newline RED.
    // Retain the complete matrix, row, paint and cursor oracle for every control.
    for (blank, styled, actual, expected) in complete_pairs {
        assert_eq!(normalized_matrix(&actual), normalized_matrix(&expected), "complete assembled matrix (blank={blank}, styled={styled})");
        assert_eq!(complete_frame_rows(&actual), complete_frame_rows(&expected));
        assert_eq!(frame_paint(&actual), frame_paint(&expected), "complete paint (blank={blank}, styled={styled})");
        assert_eq!(actual.phys_cursor, expected.phys_cursor);
        eprintln!("complete output passed: blank={blank}, styled={styled}");
    }
}
