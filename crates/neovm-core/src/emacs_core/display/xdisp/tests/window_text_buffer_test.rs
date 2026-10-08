//! Window-buffer display-property regressions for neomacs#81.
//! Exercise the real measurement builtin and its image-catalog seam, not a
//! standalone property helper. Buffer A stays in the selected window while B
//! is current; both explicit WINDOW and nil must measure A.
use super::*;
use std::sync::{Arc, Mutex};

type ImageRequests = Arc<Mutex<Vec<ImageResolveRequest>>>;

struct RecordingImageHost(ImageRequests);

impl DisplayHost for RecordingImageHost {
    fn realize_gui_frame(
        &mut self,
        _request: crate::emacs_core::eval::GuiFrameHostRequest,
    ) -> Result<(), String> {
        Ok(())
    }

    fn resize_gui_frame(
        &mut self,
        _request: crate::emacs_core::eval::GuiFrameHostRequest,
    ) -> Result<(), String> {
        Ok(())
    }

    fn image_catalog(&self) -> Option<&dyn ImageCatalog> {
        Some(self)
    }
}

impl ImageCatalog for RecordingImageHost {
    fn lookup(&self, request: ImageResolveRequest, limit: ImageSizeLimit) -> ImageLookup {
        self.0
            .lock()
            .expect("record image request")
            .push(request.clone());
        DecodedImageHost.lookup(request, limit)
    }
}

fn window_buffer_context(text: &str) -> (Context, Value, BufferId, BufferId, ImageRequests) {
    let (mut eval, window) = pixel_size_image_context();
    let target = eval.buffers.current_buffer_id().expect("target buffer A");
    eval.buffers.get_mut(target).expect("A").insert(text);
    let frame = eval
        .frames
        .find_window_frame_id(WindowId(window as u64))
        .expect("A's frame");
    assert!(eval.frames.select_frame(frame));
    let ambient = eval.buffers.create_buffer("*pixel-size-current-B*");
    eval.buffers.get_mut(ambient).expect("B").insert(text);
    eval.buffers.set_current(ambient);
    let requests = Arc::new(Mutex::new(Vec::new()));
    eval.set_display_host(Box::new(RecordingImageHost(Arc::clone(&requests))));
    (
        eval,
        Value::make_window(window as u64),
        target,
        ambient,
        requests,
    )
}

fn put_display(eval: &mut Context, buffer: BufferId, start: i64, end: i64, spec: Value) {
    crate::emacs_core::textprop::builtin_put_text_property(
        eval,
        vec![
            Value::fixnum(start),
            Value::fixnum(end),
            Value::symbol("display"),
            spec,
            Value::make_buffer(buffer),
        ],
    )
    .expect("put display property on explicit buffer");
}

fn measure_window_buffer(eval: &mut Context, window: Value, ambient: BufferId) -> (i64, i64) {
    assert_eq!(eval.buffers.current_buffer_id(), Some(ambient));
    let selected = eval
        .frames
        .selected_frame()
        .map(|frame| (frame.id, frame.selected_window));
    let size = builtin_window_text_pixel_size_ctx(
        eval,
        vec![window, Value::NIL, Value::NIL, Value::T, Value::NIL],
    )
    .expect("measure window A while buffer B is current");
    assert_eq!(
        eval.buffers.current_buffer_id(),
        Some(ambient),
        "leave B current"
    );
    assert_eq!(
        eval.frames
            .selected_frame()
            .map(|frame| (frame.id, frame.selected_window)),
        selected,
        "leave frame and window selection unchanged"
    );
    (
        size.cons_car().as_int().expect("integer width"),
        size.cons_cdr().as_int().expect("integer height"),
    )
}

#[test]
fn window_text_pixel_size_ignores_current_buffer_image_properties() {
    crate::test_utils::init_test_tracing();
    for nil_window in [false, true] {
        let (mut eval, window, target, ambient, requests) = window_buffer_context("abcd");
        assert_ne!(target, ambient);
        put_display(&mut eval, ambient, 1, 5, image_display_spec(2000, 800, &[]));
        assert_eq!(
            measure_window_buffer(
                &mut eval,
                if nil_window { Value::NIL } else { window },
                ambient,
            ),
            (40, 20),
            "plain A must not acquire B's image extent"
        );
        assert!(
            requests.lock().expect("image requests").is_empty(),
            "B must not reach the catalog"
        );
    }
}

#[test]
fn window_text_pixel_size_measures_target_buffer_image_with_other_buffer_current() {
    crate::test_utils::init_test_tracing();
    for nil_window in [false, true] {
        let (mut eval, window, target, ambient, requests) = window_buffer_context("ab cd");
        let spec = image_display_spec(200, 80, &[]);
        put_display(&mut eval, target, 3, 4, spec);
        let frame = eval.frames.selected_frame().expect("selected frame");
        let expected_request = crate::emacs_core::image::image_resolve_request_from_spec(
            &spec,
            crate::emacs_core::image_catalog::image_scale_environment(frame, &eval.obarray),
            eval.face_table().default_face_colors(),
        )
        .expect("target image request");
        assert_eq!(
            measure_window_buffer(
                &mut eval,
                if nil_window { Value::NIL } else { window },
                ambient,
            ),
            (240, 80),
            "A's image replaces one character, not the rest of A"
        );
        assert_eq!(
            requests.lock().expect("image requests").as_slice(),
            &[expected_request],
            "exactly A's image must reach the production catalog seam"
        );
    }
}

#[test]
fn window_text_pixel_size_uses_target_buffer_display_run_boundaries() {
    crate::test_utils::init_test_tracing();
    for nil_window in [false, true] {
        // No display in B is a positive space control. The other cases give
        // the SAME value at position 1 but a shorter/longer run in B, so a
        // value-only fix that still reads B's next change cannot satisfy this.
        for ambient_end in [None, Some(2), Some(5)] {
            let (mut eval, window, target, ambient, requests) = window_buffer_context("abcd");
            let space = Value::list(vec![
                Value::symbol("space"),
                Value::keyword(":width"),
                Value::fixnum(7),
            ]);
            put_display(&mut eval, target, 1, 3, space);
            if let Some(end) = ambient_end {
                put_display(&mut eval, ambient, 1, end, space);
            }
            assert_eq!(
                measure_window_buffer(
                    &mut eval,
                    if nil_window { Value::NIL } else { window },
                    ambient,
                ),
                (90, 20),
                "one 7-cell space for A's ab, then two cells for cd; B end = {ambient_end:?}"
            );
            assert!(requests.lock().expect("image requests").is_empty());
        }
    }
}
