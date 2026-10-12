//! Fringe policy mutations must invalidate real retained engine rows.

use super::*;

struct Fixture {
    eval: Context,
    frame: neovm_core::window::FrameId,
    buffer: BufferId,
    window: neovm_core::window::WindowId,
    engine: LayoutEngine,
    line_len: usize,
}

impl Fixture {
    fn new(source_bitmap: bool) -> Self {
        let line = if source_bitmap {
            " MARK source fringe\n".to_owned()
        } else {
            format!("{}\n", "x".repeat(150))
        };
        let (mut eval, frame, buffer, window) = incr_editing_frame(&line.repeat(100), 800, 256);
        realize_test_gui_frame(&mut eval, frame);
        {
            let owner = eval.buffer_manager_mut().get_mut(buffer).unwrap();
            owner.set_buffer_local("scroll-conservatively", Value::fixnum(101));
            owner.set_buffer_local("scroll-margin", Value::fixnum(0));
            owner.set_buffer_local("display-line-numbers", Value::NIL);
            owner.set_buffer_local("mode-line-format", Value::string("ML"));
            owner.set_buffer_local("bidi-paragraph-direction", Value::symbol("left-to-right"));
            owner.set_buffer_local("truncate-lines", Value::T);
        }
        if source_bitmap {
            // Create BOTH distinct faces before retaining rows. The mutation
            // below only changes a bitmap's existing override, not a face.
            eval.eval_str(
                r##"(progn
                  (internal-make-lisp-face 'fringe-replay-red)
                  (internal-make-lisp-face 'fringe-replay-blue)
                  (internal-set-lisp-face-attribute 'fringe-replay-red :foreground "#ff0000" (selected-frame))
                  (internal-set-lisp-face-attribute 'fringe-replay-blue :foreground "#0000ff" (selected-frame))
                  (define-fringe-bitmap 'fringe-replay-source [255 129 255] nil nil 'center)
                  (set-fringe-bitmap-face 'fringe-replay-source 'fringe-replay-red))"##,
            )
            .unwrap();
            let owner = eval.buffer_manager_mut().get_mut(buffer).unwrap();
            for line_number in 0..100 {
                assert!(owner.put_text_property(
                    line_number * line.len(),
                    line_number * line.len() + 1,
                    Value::symbol("display"),
                    Value::list(vec![
                        Value::symbol("left-fringe"),
                        Value::symbol("fringe-replay-source"),
                        Value::symbol("default"),
                    ]),
                ));
            }
        } else {
            let alist = eval
                .eval_str("'((truncation left-arrow right-arrow))")
                .unwrap();
            eval.buffer_manager_mut()
                .get_mut(buffer)
                .unwrap()
                .set_buffer_local("fringe-indicator-alist", alist);
        }
        Self {
            eval,
            frame,
            buffer,
            window,
            engine: LayoutEngine::new(),
            line_len: line.len(),
        }
    }

    fn scroll_to(&mut self, line: usize) {
        scroll_window_to(
            &mut self.eval,
            self.frame,
            self.window,
            self.buffer,
            (line * self.line_len) as i64 + 1,
            (line + 2) * self.line_len + 1,
        );
    }

    fn layout(&mut self, force_full: bool) {
        if force_full {
            self.engine.retained_window_matrices.clear();
            self.engine.prepared_viewports = PreparedViewports::default();
        }
        self.engine.layout_frame_rust(&mut self.eval, self.frame);
    }

    fn activate(&mut self) {
        activate_last_engine_presentation(&mut self.eval, &self.engine, self.frame);
    }

    fn bitmap_index(&mut self, symbol: &str) -> u16 {
        self.eval
            .eval_str(&format!("(get '{symbol} 'fringe)"))
            .unwrap()
            .as_fixnum()
            .unwrap() as u16
    }

    // Include both slots (including absent slots), exact ids AND complete
    // published face definitions. The ordinary layout trace omits fringes.
    fn fringes(&self) -> serde_json::Value {
        let state = self.engine.last_frame_display_state.as_ref().unwrap();
        let entry = state
            .window_matrices
            .iter()
            .find(|entry| entry.window_id.get() == self.window.0 as i64)
            .unwrap();
        serde_json::json!(
            entry
                .matrix
                .rows
                .iter()
                .filter(|row| row.enabled)
                .map(|row| {
                    [row.left_fringe_bitmap, row.right_fringe_bitmap].map(|bitmap| {
                        bitmap.map(|bitmap| {
                            serde_json::json!({
                                "bitmap": bitmap,
                                "face": state.faces[&bitmap.face_id],
                            })
                        })
                    })
                })
                .collect::<Vec<_>>()
        )
    }

    fn body_bitmaps(
        &self,
        left: bool,
    ) -> Vec<neomacs_display_protocol::glyph_matrix::FringeBitmapInfo> {
        self.engine.retained_window_matrices[&DisplayWindowId::new(self.window.0 as i64)]
            .matrix
            .rows
            .iter()
            .filter(|row| row.enabled && row.role == GlyphRowRole::Text && row.displays_text)
            .filter_map(|row| {
                if left {
                    row.left_fringe_bitmap
                } else {
                    row.right_fringe_bitmap
                }
            })
            .collect()
    }
}

fn assert_exact(incremental: &Fixture, full: &Fixture) {
    assert_eq!(
        selected_window_layout_trace(&incremental.eval, &incremental.engine, incremental.frame),
        selected_window_layout_trace(&full.eval, &full.engine, full.frame),
    );
    assert_eq!(incremental.fringes(), full.fringes());
}

fn establish_shifted_reuse(incremental: &mut Fixture, full: &mut Fixture) {
    // Both Contexts already exist: a control constructor must not invalidate
    // the candidate through the process-wide character-table revision.
    for fixture in [&mut *incremental, &mut *full] {
        fixture.scroll_to(20);
        fixture.layout(false);
        fixture.activate();
    }
    incremental.scroll_to(21);
    full.scroll_to(21);
    incremental.layout(false);
    full.layout(true);
    assert_exact(incremental, full);
    let stats = incremental.engine.last_layout_stats();
    assert_eq!(stats.scroll_windows, 1, "{stats:?}");
    assert!(stats.reused_shifted_rows > 0, "{stats:?}");
    incremental.activate();
    full.activate();
}

fn assert_full_fallback(fixture: &Fixture) {
    let stats = fixture.engine.last_layout_stats();
    assert_eq!(stats.scroll_windows, 0, "{stats:?}");
    assert_eq!(stats.cursor_only_windows, 0, "{stats:?}");
    assert_eq!(stats.prepared_windows, 0, "{stats:?}");
    assert_eq!(stats.reused_shifted_rows, 0, "{stats:?}");
    assert!(stats.full_windows > 0, "{stats:?}");
    assert_eq!(stats.total_windows(), stats.full_windows, "{stats:?}");
}

#[test]
fn nested_fringe_indicator_alist_mutation_rejects_retained_scroll() {
    let mut incremental = Fixture::new(false);
    let mut full = Fixture::new(false);
    establish_shifted_reuse(&mut incremental, &mut full);
    let old_index = incremental.bitmap_index("right-arrow");
    let new_index = incremental.bitmap_index("right-triangle");
    assert_ne!(old_index, new_index);
    let before = incremental.body_bitmaps(false);
    assert!(!before.is_empty(), "fixture must produce truncation arrows");
    assert!(before.iter().all(|bitmap| bitmap.bitmap_index == old_index));

    for fixture in [&mut incremental, &mut full] {
        let display_revision = fixture.eval.display_var_change_count;
        let face_revision = fixture.eval.face_change_count;
        let root = fixture.eval.eval_str("fringe-indicator-alist").unwrap();
        let registry_revision = fixture.eval.fringe_bitmap_registry().revision();
        // Mutate a nested cons without replacing the buffer-local root.
        // Raw-value identity and display-variable counters cannot catch this.
        fixture
            .eval
            .eval_str(
                "(setcar (cdr (cdr (assq 'truncation fringe-indicator-alist))) 'right-triangle)",
            )
            .unwrap();
        assert_eq!(
            fixture.eval.eval_str("fringe-indicator-alist").unwrap(),
            root
        );
        assert_eq!(fixture.eval.display_var_change_count, display_revision);
        assert_eq!(fixture.eval.face_change_count, face_revision);
        assert_eq!(
            fixture.eval.fringe_bitmap_registry().revision(),
            registry_revision
        );
        fixture.scroll_to(22);
    }
    incremental.layout(false);
    full.layout(true);
    assert_full_fallback(&incremental);
    assert_exact(&incremental, &full);
    let after = incremental.body_bitmaps(false);
    assert!(!after.is_empty());
    assert!(after.iter().all(|bitmap| bitmap.bitmap_index == new_index));
}

#[test]
fn source_fringe_replacing_point_backward_keeps_full_walk_cursor_width() {
    let mut incremental = Fixture::new(true);
    let mut full = Fixture::new(true);
    establish_shifted_reuse(&mut incremental, &mut full);
    for fixture in [&mut incremental, &mut full] {
        fixture.scroll_to(20);
        fixture
            .eval
            .buffer_manager_mut()
            .get_mut(fixture.buffer)
            .unwrap()
            .goto_emacs_byte_pos(EmacsBytePos::new(22 * fixture.line_len));
    }
    incremental.layout(false);
    full.layout(true);
    assert_exact(&incremental, &full);
    assert_eq!(incremental.engine.last_layout_stats().scroll_windows, 0);
    assert_eq!(
        selected_window_layout_trace(&incremental.eval, &incremental.engine, incremental.frame)
            .phys_cursor
            .unwrap()
            .width,
        1
    );
}

#[test]
fn source_fringe_replacing_point_keeps_full_walk_cursor_width() {
    let mut incremental = Fixture::new(true);
    let mut full = Fixture::new(true);
    establish_shifted_reuse(&mut incremental, &mut full);
    for fixture in [&mut incremental, &mut full] {
        fixture.scroll_to(22);
        fixture
            .eval
            .buffer_manager_mut()
            .get_mut(fixture.buffer)
            .unwrap()
            .goto_emacs_byte_pos(EmacsBytePos::new(24 * fixture.line_len));
    }
    incremental.layout(false);
    full.layout(true);
    assert_exact(&incremental, &full);
    assert_eq!(incremental.engine.last_layout_stats().scroll_windows, 0);
    assert_eq!(
        selected_window_layout_trace(&incremental.eval, &incremental.engine, incremental.frame)
            .phys_cursor
            .unwrap()
            .width,
        1
    );
}

#[test]
fn existing_source_bitmap_face_override_rejects_retained_scroll() {
    let mut incremental = Fixture::new(true);
    let mut full = Fixture::new(true);
    establish_shifted_reuse(&mut incremental, &mut full);
    let index = incremental.bitmap_index("fringe-replay-source");
    let before = incremental.body_bitmaps(true);
    assert!(
        !before.is_empty(),
        "fixture must realize source-produced bitmaps"
    );
    assert!(before.iter().all(|bitmap| bitmap.bitmap_index == index));
    let old_face = before[0].face_id;
    assert!(before.iter().all(|bitmap| bitmap.face_id == old_face));
    let old_definition = serde_json::to_value(
        &incremental
            .engine
            .last_frame_display_state
            .as_ref()
            .unwrap()
            .faces[&old_face],
    )
    .unwrap();

    for fixture in [&mut incremental, &mut full] {
        let display_revision = fixture.eval.display_var_change_count;
        let face_revision = fixture.eval.face_change_count;
        let registry_revision = fixture.eval.fringe_bitmap_registry().revision();
        fixture
            .eval
            .eval_str("(set-fringe-bitmap-face 'fringe-replay-source 'fringe-replay-blue)")
            .unwrap();
        assert_eq!(fixture.eval.display_var_change_count, display_revision);
        assert_eq!(fixture.eval.face_change_count, face_revision);
        assert!(fixture.eval.fringe_bitmap_registry().revision() > registry_revision);
        fixture.scroll_to(22);
    }
    incremental.layout(false);
    full.layout(true);
    assert_full_fallback(&incremental);
    assert_exact(&incremental, &full);
    let after = incremental.body_bitmaps(true);
    assert!(!after.is_empty());
    assert!(after.iter().all(|bitmap| bitmap.bitmap_index == index));
    let state = incremental
        .engine
        .last_frame_display_state
        .as_ref()
        .unwrap();
    for bitmap in after {
        assert_ne!(
            bitmap.face_id, old_face,
            "existing override must change the row face"
        );
        assert_ne!(
            serde_json::to_value(&state.faces[&bitmap.face_id]).unwrap(),
            old_definition
        );
    }
}
