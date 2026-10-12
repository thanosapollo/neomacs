use crate::emacs_core::display_host::FontResolveRequest;
use crate::emacs_core::eval::{
    Context, DisplayHost, FontPxProbeResult, GuiFrameHostRequest, ResolvedFontMatch,
};
use crate::emacs_core::value::Value;
use crate::face::{FaceHeight, FontSlant, FontWeight, FontWidth};

/// Deterministic font-system adapter: point sizes at 96 DPI, no font cache.
/// Face definitions, remapping, and Lisp queries all use the real evaluator.
struct PointSizeFontHost;

impl DisplayHost for PointSizeFontHost {
    fn realize_gui_frame(&mut self, _request: GuiFrameHostRequest) -> Result<(), String> {
        Ok(())
    }

    fn resize_gui_frame(&mut self, _request: GuiFrameHostRequest) -> Result<(), String> {
        Ok(())
    }

    fn resolve_font_for_char(
        &mut self,
        request: FontResolveRequest,
    ) -> Result<Option<ResolvedFontMatch>, String> {
        let height = match request.face.height {
            Some(FaceHeight::Absolute(height)) => f64::from(height),
            Some(FaceHeight::Relative(scale)) => 150.0 * scale,
            None => 150.0,
        };
        let pixel_size = (height * 96.0 / 720.0).round() as i32;
        Ok(Some(ResolvedFontMatch {
            font: crate::emacs_core::eval::test_resolved_opened_font(
                "Test Mono",
                None,
                None,
                FontWeight::NORMAL,
                FontSlant::Normal,
                FontWidth::Normal,
                Some("TestMono"),
                FontPxProbeResult {
                    pixel_size: pixel_size.try_into().unwrap(),
                    height: pixel_size + 4,
                    ascent: pixel_size,
                    descent: 4,
                    max_width: pixel_size / 2,
                    space_width: pixel_size / 2,
                    average_width: pixel_size / 2,
                },
                None,
            ),
            glyph_code: Some(request.character.code()),
        }))
    }
}

fn context() -> Context {
    let mut eval = Context::new();
    let frame = crate::emacs_core::window_cmds::ensure_selected_frame_id(&mut eval);
    eval.frames
        .get_mut(frame)
        .unwrap()
        .set_window_system(Some(Value::symbol("neo")));
    eval.eval_str("(internal-set-lisp-face-attribute 'default :height 150 nil)")
        .unwrap();
    eval.set_display_host(Box::new(PointSizeFontHost));
    eval
}

fn default_font_size(eval: &mut Context) -> i64 {
    eval.eval_str("(font-get (font-spec :name (face-font 'default)) :size)")
        .unwrap()
        .as_int()
        .unwrap()
}

#[test]
fn face_font_relative_remapping_takes_precedence_over_its_base_face() {
    let mut eval = context();
    assert_eq!(default_font_size(&mut eval), 20);
    // GNU face-remap-add-relative emits this shape. The leftmost entry wins;
    // the trailing self-reference supplies the unremapped base attributes.
    eval.eval_str("(setq face-remapping-alist '((default (:height 1.2) default)))")
        .unwrap();
    assert_eq!(default_font_size(&mut eval), 24);
    eval.eval_str("(setq face-remapping-alist nil)").unwrap();
    assert_eq!(default_font_size(&mut eval), 20);
}

#[test]
fn face_font_remapping_preserves_gnu_relative_and_absolute_precedence() {
    let mut eval = context();
    // GNU xfaces.c merges the tail before the head. Relative heights compose,
    // whereas an absolute height replaces the lower-priority contribution.
    for (specification, expected) in [
        ("((default (:height 1.2) (:height 1.5) default))", 36),
        ("((default (:height 150) (:height 2.0) default))", 20),
        ("((default (:height 2.0) (:height 150) default))", 40),
    ] {
        eval.eval_str(&format!("(setq face-remapping-alist '{specification})"))
            .unwrap();
        assert_eq!(default_font_size(&mut eval), expected, "{specification}");
    }
}

#[test]
fn font_at_named_face_remapping_uses_the_same_precedence_as_face_font() {
    let mut eval = context();
    eval.eval_str("(internal-make-lisp-face 'issue360-font)")
        .unwrap();
    eval.eval_str("(internal-set-lisp-face-attribute 'issue360-font :height 150 nil)")
        .unwrap();
    eval.eval_str("(setq face-remapping-alist '((issue360-font (:height 1.2) issue360-font)))")
        .unwrap();
    let size = eval
        .eval_str("(font-get (font-at 0 nil (propertize \"M\" 'face 'issue360-font)) :size)")
        .unwrap();
    assert_eq!(size.as_int(), Some(24));
}

fn assert_font_face_matches_full_table(eval: &mut Context, name: &str) {
    let frame = eval.frames.selected_frame().unwrap().id;
    let remapping = super::face_remapping_for_current_buffer(eval);
    let table = super::runtime_face_table_from_frame_lisp_faces(eval, frame, false);
    let expected = if remapping.is_empty() {
        table.resolve(name)
    } else {
        table.resolve_with_remapping(name, &remapping)
    };
    let actual = super::resolve_face_for_font(eval, frame, name, &remapping);
    assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
}

#[test]
fn face_font_leaf_resolution_matches_full_table_and_remapping_order() {
    let mut eval = context();
    eval.eval_str(
        "(progn
           (internal-make-lisp-face 'leaf-font)
           (internal-make-lisp-face 'leaf-font-target)
           (internal-set-lisp-face-attribute 'leaf-font :family \"Leaf\" nil)
           (internal-set-lisp-face-attribute 'leaf-font :height 120 nil)
           (internal-set-lisp-face-attribute 'leaf-font-target :height 180 nil))",
    )
    .unwrap();
    for spec in [
        "nil",
        "((leaf-font (:height 1.2) leaf-font))",
        "((leaf-font leaf-font-target) (leaf-font-target (:height 1.5) leaf-font-target))",
        "((leaf-font leaf-font-target) (leaf-font-target leaf-font))",
        "((leaf-font (:height 2.0) (:height 150) leaf-font))",
        "((leaf-font (:height 150) (:height 2.0) leaf-font))",
        "((unrelated (:inherit missing)) (leaf-font (:height 1.2) leaf-font))",
    ] {
        eval.eval_str(&format!("(setq face-remapping-alist '{spec})"))
            .unwrap();
        assert_font_face_matches_full_table(&mut eval, "default");
        assert_font_face_matches_full_table(&mut eval, "leaf-font");
    }
}

#[test]
fn face_font_leaf_resolution_observes_raw_vector_entry_and_remap_mutation() {
    let mut eval = context();
    eval.eval_str(
        "(let ((entry (gethash 'default (frame--face-hash-table))))
           (setq leaf-font-vector (if (vectorp entry) entry (cdr entry))))",
    )
    .unwrap();
    let before = eval.face_change_count;
    eval.eval_str("(aset leaf-font-vector 4 180)").unwrap();
    assert_eq!(default_font_size(&mut eval), 24);
    assert_font_face_matches_full_table(&mut eval, "default");
    eval.eval_str(
        "(progn
           (setq leaf-font-entry (cons 42 leaf-font-vector))
           (puthash 'default leaf-font-entry (frame--face-hash-table))
           (setq leaf-font-vector (copy-sequence leaf-font-vector))
           (aset leaf-font-vector 4 210)
           (setcdr leaf-font-entry leaf-font-vector))",
    )
    .unwrap();
    assert_eq!(default_font_size(&mut eval), 28);
    assert_font_face_matches_full_table(&mut eval, "default");
    eval.eval_str("(setq face-remapping-alist '((default (:height 1.5) default)))")
        .unwrap();
    assert_eq!(default_font_size(&mut eval), 42);
    eval.eval_str("(setcar (cdr (car (cdr (car face-remapping-alist)))) 2.0)")
        .unwrap();
    assert_eq!(default_font_size(&mut eval), 56);
    assert_font_face_matches_full_table(&mut eval, "default");
    assert_eq!(eval.face_change_count, before);
}

#[test]
fn face_font_sparse_resolution_handles_inheritance_and_missing_vectors() {
    let mut eval = context();
    eval.eval_str(
        "(progn
           (internal-make-lisp-face 'leaf-font)
           (internal-make-lisp-face 'leaf-parent)
           (internal-set-lisp-face-attribute 'leaf-parent :height 180 nil))",
    )
    .unwrap();
    let frame = eval.frames.selected_frame().unwrap().id;
    for inheritance in ["leaf-parent", "(leaf-parent default)", "(:height 210)"] {
        eval.eval_str(&format!(
            "(internal-set-lisp-face-attribute 'leaf-font :inherit '{inheritance} nil)"
        ))
        .unwrap();
        assert!(super::leaf_runtime_face_for_font(&eval, frame, "leaf-font").is_none());
        let remapping = super::face_remapping_for_current_buffer(&eval);
        assert!(super::face_table_for_font(&eval, frame, "leaf-font", &remapping).is_some());
        assert_font_face_matches_full_table(&mut eval, "leaf-font");
    }
    eval.eval_str("(internal-set-lisp-face-attribute 'leaf-font :inherit nil nil)")
        .unwrap();
    for spec in [
        "((leaf-font (:inherit leaf-parent)))",
        "((leaf-font (:inherit (:height 210))))",
    ] {
        eval.eval_str(&format!("(setq face-remapping-alist '{spec})"))
            .unwrap();
        let remapping = super::face_remapping_for_current_buffer(&eval);
        assert!(super::face_table_for_font(&eval, frame, "leaf-font", &remapping).is_some());
        assert_font_face_matches_full_table(&mut eval, "leaf-font");
    }
    eval.eval_str("(setq face-remapping-alist '((leaf-font missing-leaf-font)))")
        .unwrap();
    let remapping = super::face_remapping_for_current_buffer(&eval);
    assert!(super::face_table_for_font(&eval, frame, "leaf-font", &remapping).is_none());
    assert_font_face_matches_full_table(&mut eval, "leaf-font");
    eval.eval_str("(remhash 'default (frame--face-hash-table))")
        .unwrap();
    assert!(super::leaf_runtime_face_for_font(&eval, frame, "default").is_none());
    assert_font_face_matches_full_table(&mut eval, "default");
}

#[test]
fn face_font_leaf_table_cost_depends_on_reachable_remaps() {
    let mut eval = context();
    let frame = eval.frames.selected_frame().unwrap().id;
    eval.eval_str("(setq face-remapping-alist '((default (:height 1.2) default)))")
        .unwrap();
    let remapping = super::face_remapping_for_current_buffer(&eval);
    let before = super::face_table_for_font(&eval, frame, "default", &remapping).unwrap();
    for index in 0..1000 {
        super::ensure_frame_lisp_face_vector(
            &mut eval,
            frame,
            &format!("unrelated-font-{index}"),
            super::FrameFaceInitial::Empty,
        )
        .unwrap();
    }
    let after = super::face_table_for_font(&eval, frame, "default", &remapping).unwrap();
    assert_eq!(before.len(), after.len());
    assert!(after.get("unrelated-font-999").is_none());
    assert!(
        super::runtime_face_table_from_frame_lisp_faces(&eval, frame, false).len()
            >= after.len() + 1000
    );
    assert_font_face_matches_full_table(&mut eval, "default");
}

#[test]
fn face_font_sparse_inheritance_preserves_nested_order_cycles_and_remapping() {
    let mut eval = context();
    eval.eval_str(
        "(progn
           (internal-make-lisp-face 'child-font)
           (internal-make-lisp-face 'parent-font)
           (internal-make-lisp-face 'alternate-font)
           (internal-set-lisp-face-attribute 'parent-font :height 180 nil)
           (internal-set-lisp-face-attribute 'alternate-font :height 210 nil))",
    )
    .unwrap();
    let frame = eval.frames.selected_frame().unwrap().id;
    for inheritance in [
        "parent-font",
        "(parent-font alternate-font)",
        "(alternate-font parent-font)",
        "(:height 1.5 :inherit parent-font)",
        "((:height 1.5 :inherit (parent-font alternate-font)) alternate-font)",
        "(:inherit missing-font :inherit parent-font)",
    ] {
        eval.eval_str(&format!(
            "(internal-set-lisp-face-attribute 'child-font :inherit '{inheritance} nil)"
        ))
        .unwrap();
        for remapping in [
            "nil",
            "((child-font (:height 1.2) child-font))",
            "((child-font alternate-font child-font) (parent-font alternate-font))",
        ] {
            eval.eval_str(&format!("(setq face-remapping-alist '{remapping})"))
                .unwrap();
            let remapping = super::face_remapping_for_current_buffer(&eval);
            assert!(super::face_table_for_font(&eval, frame, "child-font", &remapping).is_some());
            assert_font_face_matches_full_table(&mut eval, "child-font");
        }
    }
    eval.eval_str(
        "(progn
           (setq face-remapping-alist nil)
           (internal-set-lisp-face-attribute 'child-font :inherit 'parent-font nil)
           (internal-set-lisp-face-attribute 'parent-font :inherit 'child-font nil))",
    )
    .unwrap();
    assert_font_face_matches_full_table(&mut eval, "child-font");
    eval.eval_str(
        "(progn
           (setq child-font-entry (gethash 'child-font (frame--face-hash-table)))
           (setq child-font-vector (if (vectorp child-font-entry)
                                       child-font-entry (cdr child-font-entry)))
           (setq recursive-font-reference (list nil 'alternate-font))
           (setcar recursive-font-reference recursive-font-reference)
           (aset child-font-vector 16 recursive-font-reference))",
    )
    .unwrap();
    let remapping = super::face_remapping_for_current_buffer(&eval);
    assert!(super::face_table_for_font(&eval, frame, "child-font", &remapping).is_some());
    assert_font_face_matches_full_table(&mut eval, "child-font");
    eval.eval_str("(setcdr recursive-font-reference recursive-font-reference)")
        .unwrap();
    assert_font_face_matches_full_table(&mut eval, "child-font");
}

#[test]
fn face_font_sparse_inheritance_observes_live_nested_reference_mutation() {
    let mut eval = context();
    eval.eval_str(
        "(progn
           (internal-make-lisp-face 'child-font)
           (internal-make-lisp-face 'parent-font)
           (internal-set-lisp-face-attribute 'parent-font :height 180 nil)
           (setq inherited-font-plist (list :height 1.5 :inherit 'parent-font))
           (internal-set-lisp-face-attribute 'child-font :inherit inherited-font-plist nil)
           (setq parent-font-entry (gethash 'parent-font (frame--face-hash-table)))
           (setq parent-font-vector (if (vectorp parent-font-entry)
                                        parent-font-entry (cdr parent-font-entry))))",
    )
    .unwrap();
    let before = eval.face_change_count;
    for (mutation, height) in [
        ("nil", 270),
        ("(setcar (cdr inherited-font-plist) 2.0)", 360),
        ("(aset parent-font-vector 4 210)", 420),
    ] {
        eval.eval_str(mutation).unwrap();
        let remapping = super::face_remapping_for_current_buffer(&eval);
        let frame = eval.frames.selected_frame().unwrap().id;
        let face = super::resolve_face_for_font(&eval, frame, "child-font", &remapping);
        assert_eq!(face.height, Some(FaceHeight::Absolute(height)));
        assert_font_face_matches_full_table(&mut eval, "child-font");
    }
    assert_eq!(eval.face_change_count, before);
}

#[test]
fn face_font_sparse_inheritance_cost_excludes_unrelated_live_faces() {
    let mut eval = context();
    eval.eval_str(
        "(progn
           (internal-make-lisp-face 'child-font)
           (internal-make-lisp-face 'parent-font)
           (internal-set-lisp-face-attribute 'parent-font :height 180 nil)
           (internal-set-lisp-face-attribute 'child-font :inherit
                                            '(:height 1.5 :inherit parent-font) nil))",
    )
    .unwrap();
    let frame = eval.frames.selected_frame().unwrap().id;
    let specifications = ["nil", "((child-font (:height 1.2) child-font))"];
    let baseline_sizes: Vec<_> = specifications
        .iter()
        .map(|specification| {
            eval.eval_str(&format!("(setq face-remapping-alist '{specification})"))
                .unwrap();
            let remapping = super::face_remapping_for_current_buffer(&eval);
            super::face_table_for_font(&eval, frame, "child-font", &remapping)
                .unwrap()
                .len()
        })
        .collect();
    for index in 0..1001 {
        super::ensure_frame_lisp_face_vector(
            &mut eval,
            frame,
            &format!("unrelated-inherited-font-{index}"),
            super::FrameFaceInitial::Empty,
        )
        .unwrap();
    }
    for (specification, baseline_size) in specifications.iter().zip(baseline_sizes) {
        eval.eval_str(&format!("(setq face-remapping-alist '{specification})"))
            .unwrap();
        let remapping = super::face_remapping_for_current_buffer(&eval);
        let table = super::face_table_for_font(&eval, frame, "child-font", &remapping).unwrap();
        assert_eq!(table.len(), baseline_size);
        assert!(table.get("parent-font").is_some());
        assert!(table.get("unrelated-inherited-font-1000").is_none());
        assert!(
            super::runtime_face_table_from_frame_lisp_faces(&eval, frame, false).len()
                >= table.len() + 1001
        );
        assert_font_face_matches_full_table(&mut eval, "child-font");
    }
}

#[test]
fn face_font_sparse_named_inheritance_keeps_full_resolver_depth_limit() {
    let mut eval = context();
    let frame = eval.frames.selected_frame().unwrap().id;
    for index in 0..13 {
        let vector = super::ensure_frame_lisp_face_vector(
            &mut eval,
            frame,
            &format!("depth-font-{index}"),
            super::FrameFaceInitial::Empty,
        )
        .unwrap();
        if index < 12 {
            super::set_lisp_face_vector_attr(
                vector,
                crate::face::LFaceAttr::Inherit,
                Value::symbol(&format!("depth-font-{}", index + 1)),
            );
        }
        if index == 11 {
            super::set_lisp_face_vector_attr(
                vector,
                crate::face::LFaceAttr::Height,
                Value::fixnum(210),
            );
        }
    }
    let remapping = super::face_remapping_for_current_buffer(&eval);
    for (name, expected) in [
        ("depth-font-0", None),
        ("depth-font-1", Some(FaceHeight::Absolute(210))),
    ] {
        let actual = super::resolve_face_for_font(&eval, frame, name, &remapping);
        assert_eq!(actual.height, expected);
        assert_font_face_matches_full_table(&mut eval, name);
    }
}

#[test]
fn face_font_sparse_inline_inheritance_keeps_full_resolver_depth_limit() {
    let mut eval = context();
    let frame = eval.frames.selected_frame().unwrap().id;
    let vector = super::ensure_frame_lisp_face_vector(
        &mut eval,
        frame,
        "deep-inline-font",
        super::FrameFaceInitial::Empty,
    )
    .unwrap();
    let remapping = super::face_remapping_for_current_buffer(&eval);
    for (wrappers, expected) in [(39, Some(FaceHeight::Absolute(180))), (40, None)] {
        let mut reference = Value::list(vec![Value::keyword("height"), Value::fixnum(180)]);
        for index in 0..wrappers {
            reference = if index % 2 == 0 {
                Value::list(vec![reference])
            } else {
                Value::list(vec![Value::keyword("inherit"), reference])
            };
        }
        super::set_lisp_face_vector_attr(vector, crate::face::LFaceAttr::Inherit, reference);
        let actual = super::resolve_face_for_font(&eval, frame, "deep-inline-font", &remapping);
        assert_eq!(actual.height, expected);
        assert_font_face_matches_full_table(&mut eval, "deep-inline-font");
    }
}

#[test]
fn face_font_sparse_uninterned_parent_falls_back_to_full_name_resolution() {
    let mut eval = context();
    let frame = eval.frames.selected_frame().unwrap().id;
    let parent = super::ensure_frame_lisp_face_vector(
        &mut eval,
        frame,
        "uninterned-parent-font",
        super::FrameFaceInitial::Empty,
    )
    .unwrap();
    super::set_lisp_face_vector_attr(parent, crate::face::LFaceAttr::Height, Value::fixnum(210));
    let child = super::ensure_frame_lisp_face_vector(
        &mut eval,
        frame,
        "uninterned-child-font",
        super::FrameFaceInitial::Empty,
    )
    .unwrap();
    let uninterned_parent = eval
        .eval_str("(make-symbol \"uninterned-parent-font\")")
        .unwrap();
    super::set_lisp_face_vector_attr(child, crate::face::LFaceAttr::Inherit, uninterned_parent);
    let remapping = super::face_remapping_for_current_buffer(&eval);
    assert!(
        super::face_table_for_font(&eval, frame, "uninterned-child-font", &remapping).is_none()
    );
    let actual = super::resolve_face_for_font(&eval, frame, "uninterned-child-font", &remapping);
    assert_eq!(actual.height, Some(FaceHeight::Absolute(210)));
    assert_font_face_matches_full_table(&mut eval, "uninterned-child-font");
}
