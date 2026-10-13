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

    fn resolve_font_for_spec(
        &mut self,
        _request: crate::emacs_core::eval::FontSpecResolveRequest,
    ) -> Result<Option<crate::emacs_core::eval::ResolvedFontSpecMatch>, String> {
        Ok(Some(crate::emacs_core::eval::ResolvedFontSpecMatch {
            family: crate::heap_types::LispString::from_utf8("Test Mono"),
            foundry: None,
            registry: None,
            file: None,
            weight: Some(FontWeight::NORMAL),
            slant: Some(FontSlant::Normal),
            width: Some(FontWidth::Normal),
            spacing: None,
            postscript_name: None,
            coverage_handle: None,
        }))
    }

    fn probe_font_entity_metrics(
        &mut self,
        request: crate::emacs_core::eval::FontEntityMetricsRequest,
    ) -> Result<Option<crate::emacs_core::eval::ResolvedFontEntityMetrics>, String> {
        let pixel_size = match request.size {
            crate::emacs_core::display_host::FontOpeningSize::Pixels(size) => size.get(),
            other => panic!("expected the realized XLFD pixel size, got {other:?}"),
        };
        let size = i32::try_from(pixel_size).unwrap();
        Ok(Some(crate::emacs_core::eval::ResolvedFontEntityMetrics {
            metrics: FontPxProbeResult {
                pixel_size,
                height: size + 4,
                ascent: size,
                descent: 4,
                max_width: size / 2,
                space_width: size / 2,
                average_width: size / 2,
            },
            file: None,
            capability: None,
        }))
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
    configure_context(Context::new())
}

fn configure_context(mut eval: Context) -> Context {
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
    let expected = table.resolve_for_font(name, &remapping);
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
           (internal-make-lisp-face 'empty-font)
           (internal-set-lisp-face-attribute 'empty-font :height 'unspecified nil)
           (internal-set-lisp-face-attribute 'parent-font :height 180 nil)
           (internal-set-lisp-face-attribute 'alternate-font :height 210 nil))",
    )
    .unwrap();
    let frame = eval.frames.selected_frame().unwrap().id;
    // Every written :inherit is reachable in GNU's ordered plist merge, even
    // when a later one overrides it. Keep this sparse graph complete; missing
    // vector fallback is covered by the missing-vectors test above.
    for inheritance in [
        "parent-font",
        "(parent-font alternate-font)",
        "(alternate-font parent-font)",
        "(:height 1.5 :inherit parent-font)",
        "((:height 1.5 :inherit (parent-font alternate-font)) alternate-font)",
        "(:inherit empty-font :inherit parent-font)",
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
           (setq inherited-font-plist (list :inherit 'parent-font :height 1.5))
           (internal-set-lisp-face-attribute 'child-font :inherit inherited-font-plist nil)
           (setq parent-font-entry (gethash 'parent-font (frame--face-hash-table)))
           (setq parent-font-vector (if (vectorp parent-font-entry)
                                        parent-font-entry (cdr parent-font-entry))))",
    )
    .unwrap();
    let before = eval.face_change_count;
    for (mutation, height) in [
        ("nil", 270),
        ("(setcar (cdr (cdr (cdr inherited-font-plist))) 2.0)", 360),
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
fn face_font_sparse_named_inheritance_keeps_named_font_depth_limit() {
    let mut eval = context();
    let frame = eval.frames.selected_frame().unwrap().id;
    for index in 0..43 {
        let vector = super::ensure_frame_lisp_face_vector(
            &mut eval,
            frame,
            &format!("depth-font-{index}"),
            super::FrameFaceInitial::Empty,
        )
        .unwrap();
        if index < 42 {
            super::set_lisp_face_vector_attr(
                vector,
                crate::face::LFaceAttr::Inherit,
                Value::symbol(&format!("depth-font-{}", index + 1)),
            );
        }
        if index == 41 {
            super::set_lisp_face_vector_attr(
                vector,
                crate::face::LFaceAttr::Height,
                Value::fixnum(210),
            );
        }
    }
    let remapping = super::face_remapping_for_current_buffer(&eval);
    for (name, expected) in [
        ("depth-font-0", Some(FaceHeight::Absolute(150))),
        ("depth-font-1", Some(FaceHeight::Absolute(210))),
    ] {
        let actual = super::resolve_face_for_font(&eval, frame, name, &remapping);
        assert_eq!(actual.height, expected);
        assert_font_face_matches_full_table(&mut eval, name);
    }
}

#[test]
fn face_font_sparse_inline_inheritance_keeps_named_font_depth_limit() {
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
    for (wrappers, expected) in [
        (26, Some(FaceHeight::Absolute(180))),
        (27, Some(FaceHeight::Absolute(150))),
    ] {
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

#[test]
fn named_font_merge_preserves_gnu_remapping_boundaries_plist_order_and_cycles() {
    let mut eval = context();
    for (name, height, inherit) in [
        ("height-base", "101", "nil"),
        ("height-parent", "200", "nil"),
        ("height-a", "0.5", "'height-b"),
        ("height-b", "0.5", "nil"),
    ] {
        eval.eval_str(&format!("(internal-make-lisp-face '{name})"))
            .unwrap();
        eval.eval_str(&format!(
            "(internal-set-lisp-face-attribute '{name} :height {height} nil)"
        ))
        .unwrap();
        eval.eval_str(&format!(
            "(internal-set-lisp-face-attribute '{name} :inherit {inherit} nil)"
        ))
        .unwrap();
    }
    let frame = crate::emacs_core::window_cmds::ensure_selected_frame_id(&mut eval);
    eval.eval_str("(internal-set-lisp-face-attribute 'default :height 101 nil)")
        .unwrap();
    let table = super::runtime_face_table_from_frame_lisp_faces(&eval, frame, false);
    let empty = crate::face::FaceRemapping::new();
    assert_eq!(
        table.resolve_for_font("height-a", &empty).height,
        Some(FaceHeight::Absolute(25))
    );
    for (specification, expected) in [
        ("((height-base (:height 1.333) (:height 0.5)))", 67),
        ("((height-base (:height 0.5 :inherit height-parent)))", 200),
        ("((height-base (:inherit height-parent :height 0.5)))", 100),
        ("((height-base (:height 0.5) height-base))", 50),
    ] {
        eval.eval_str(&format!("(setq face-remapping-alist '{specification})"))
            .unwrap();
        let remapping = crate::emacs_core::font::face_remapping_for_current_buffer(&eval);
        assert_eq!(
            table.resolve_for_font("height-base", &remapping).height,
            Some(FaceHeight::Absolute(expected)),
            "{specification}"
        );
    }
    eval.eval_str("(setq face-remapping-alist '((height-b height-a)))")
        .unwrap();
    let remapping = crate::emacs_core::font::face_remapping_for_current_buffer(&eval);
    assert_eq!(
        table.resolve_for_font("height-a", &remapping).height,
        Some(FaceHeight::Absolute(12))
    );
}

/// GNU xfaces.c lookup_named_face copies DEFAULT_FACE_ID's attributes before
/// merge_face_ref; merge_face_vectors applies inheritance before direct height.
/// Sizes here are the source-oracle point heights realized at the host's 96 DPI.
/// The layout resolver independently checks parity with redisplay's base.
#[test]
fn face_font_merges_unspecified_inherited_and_relative_heights_with_default() {
    let mut eval = context();
    eval.eval_str("(internal-set-lisp-face-attribute 'default :height 180 nil)")
        .unwrap();
    eval.eval_str("(internal-make-lisp-face 'height-parent)")
        .unwrap();
    eval.eval_str("(internal-set-lisp-face-attribute 'height-parent :height 240 nil)")
        .unwrap();
    eval.eval_str("(internal-make-lisp-face 'height-child)")
        .unwrap();
    eval.eval_str("(internal-set-lisp-face-attribute 'height-child :inherit 'height-parent nil)")
        .unwrap();
    eval.eval_str("(internal-set-lisp-face-attribute 'height-child :height 0.5 nil)")
        .unwrap();
    eval.eval_str("(internal-make-lisp-face 'height-relative)")
        .unwrap();
    eval.eval_str("(internal-set-lisp-face-attribute 'height-relative :height 1.5 nil)")
        .unwrap();
    for (face, size) in [
        ("fixed-pitch", 24),
        ("variable-pitch", 24),
        ("height-child", 16),
        ("height-relative", 36),
    ] {
        let actual = eval
            .eval_str(&format!(
                "(font-get (font-spec :name (face-font '{face})) :size)"
            ))
            .unwrap();
        assert_eq!(actual.as_int(), Some(size), "{face}");
        let width = eval
            .eval_str(&format!("(aref (font-info (face-font '{face})) 11)"))
            .unwrap();
        assert_eq!(
            width.as_int(),
            Some(size / 2),
            "window-font-width input {face}"
        );
    }
}

#[test]
fn face_font_named_queries_use_canonical_default_but_honor_explicit_remapping() {
    let mut eval = context();
    eval.eval_str("(internal-make-lisp-face 'height-absolute)")
        .unwrap();
    eval.eval_str("(internal-set-lisp-face-attribute 'height-absolute :height 120 nil)")
        .unwrap();
    eval.eval_str("(internal-make-lisp-face 'height-relative)")
        .unwrap();
    eval.eval_str("(internal-set-lisp-face-attribute 'height-relative :height 0.5 nil)")
        .unwrap();
    eval.eval_str("(setq face-remapping-alist '((default (:height 2.0) default)))")
        .unwrap();
    for (face, size) in [
        ("default", 40),
        ("fixed-pitch", 20),
        ("variable-pitch", 20),
        ("height-relative", 10),
        ("height-absolute", 16),
    ] {
        let actual = eval
            .eval_str(&format!(
                "(font-get (font-spec :name (face-font '{face})) :size)"
            ))
            .unwrap();
        assert_eq!(actual.as_int(), Some(size), "{face}");
        // GNU named queries start from canonical default, whereas text starts
        // from the buffer-remapped default. They intentionally differ here.
    }
}

fn load_window_font_width_source(eval: &mut Context) {
    // Import the real GNU-derived definitions and their small dependency
    // closure, without requiring a generated whole-editor bootstrap dump.
    eval.eval_str(
        r#"
(defalias 'defun (cons 'macro #'(lambda (name args &rest body)
  (list 'defalias (list 'quote name) (cons 'function (list (cons 'lambda (cons args body))))))))
(defalias 'defmacro (cons 'macro #'(lambda (name args &rest body)
  (list 'defalias (list 'quote name)
        (list 'cons ''macro (cons 'function (list (cons 'lambda (cons args body)))))))))
(defalias 'when (cons 'macro #'(lambda (condition &rest body)
  (list 'if condition (cons 'progn body)))))
(defalias 'unless (cons 'macro #'(lambda (condition &rest body)
  (cons 'if (cons condition (cons nil body))))))
"#,
    )
    .unwrap();
    eval.set_lexical_binding(true);
    let root = crate::test_utils::workspace_root();
    eval.eval_str(&format!(
        "(load {:?} nil t t)",
        root.join("lisp/emacs-lisp/backquote.el").to_str().unwrap()
    ))
    .unwrap();
    for (file, markers) in [
        (
            "lisp/frame.el",
            vec![
                "(defun framep-on-display ",
                "(defun display-graphic-p ",
                "(defalias 'display-multi-font-p ",
            ],
        ),
        (
            "lisp/emacs-lisp/macroexp.el",
            vec!["(defun macroexp-progn "],
        ),
        (
            "lisp/subr.el",
            vec![
                "(defalias 'not ",
                "(defun cadr ",
                "(defun caar ",
                "(defun cdar ",
                "(defun cddr ",
                "(defun last ",
                "(defmacro push ",
                "(defmacro pop ",
                "(defun internal--build-binding ",
                "(defun internal--build-bindings ",
                "(defmacro if-let* ",
                "(defmacro when-let* ",
                "(defun internal--before-with-selected-window ",
                "(defun internal--after-with-selected-window ",
                "(defmacro with-selected-window ",
            ],
        ),
        (
            "lisp/window.el",
            vec![
                "(defun window-normalize-window ",
                "(defun window-font-width ",
            ],
        ),
    ] {
        let source = std::fs::read_to_string(root.join(file)).unwrap();
        for marker in markers {
            let start = source.find(marker).unwrap();
            let obarray = crate::emacs_core::symbol::Obarray::new();
            let (form, _) =
                crate::emacs_core::value_reader::read_one(&source[start..], 0, &obarray)
                    .unwrap()
                    .unwrap();
            eval.eval_form(form).unwrap();
        }
    }
}

#[test]
fn window_font_width_uses_merged_named_font_metrics() {
    let mut eval = context();
    load_window_font_width_source(&mut eval);
    // Frame kind and native terminal output method are distinct owners. The
    // bare evaluator starts with an initial (TTY) terminal; advertise a synthetic
    // graphical connection so the real display-multi-font-p takes the font path.
    crate::emacs_core::terminal::pure::configure_terminal_runtime(
        crate::emacs_core::terminal::pure::TerminalRuntimeConfig::window_system(
            neomacs_display_protocol::GraphicalDisplayIdentity::anonymous_connection(
                neomacs_display_protocol::GraphicalBackend::Wayland,
            ),
        ),
    );
    assert!(!eval.eval_str("(display-multi-font-p)").unwrap().is_nil());
    eval.eval_str("(internal-set-lisp-face-attribute 'default :height 180 nil)")
        .unwrap();
    for face in ["default", "fixed-pitch", "variable-pitch"] {
        let width = eval
            .eval_str(&format!("(window-font-width nil '{face})"))
            .unwrap();
        assert_eq!(width.as_int(), Some(12), "{face}");
    }
    eval.eval_str("(setq face-remapping-alist '((default (:height 2.0) default)))")
        .unwrap();
    assert_eq!(
        eval.eval_str("(window-font-width)").unwrap().as_int(),
        Some(24)
    );
    assert_eq!(
        eval.eval_str("(window-font-width nil 'fixed-pitch)")
            .unwrap()
            .as_int(),
        Some(12)
    );
}

/// GUI-less GNU 32 oracle: an identity function-valued :height on an observer
/// inheriting each reference records the absolute argument to merge_face_heights.
/// Explicit parents avoid the TTY default's forced height 1. Recorded values:
/// parent 101 -> child .5 -> outer 1.333 = 66; remapped parent 200 -> child .5 = 100.
#[test]
fn face_font_inheritance_merges_remapped_parents_and_truncates_each_height() {
    let mut eval = context();
    for (face, height, parent) in [
        ("height-parent", "101", "nil"),
        ("height-child", "0.5", "'height-parent"),
        ("height-outer", "1.333", "'height-child"),
    ] {
        eval.eval_str(&format!("(internal-make-lisp-face '{face})"))
            .unwrap();
        eval.eval_str(&format!(
            "(internal-set-lisp-face-attribute '{face} :height {height} nil)"
        ))
        .unwrap();
        eval.eval_str(&format!(
            "(internal-set-lisp-face-attribute '{face} :inherit {parent} nil)"
        ))
        .unwrap();
    }
    let frame = crate::emacs_core::window_cmds::ensure_selected_frame_id(&mut eval);
    let table = super::runtime_face_table_from_frame_lisp_faces(&eval, frame, false);
    assert_eq!(
        table
            .resolve_for_font("height-outer", &crate::face::FaceRemapping::new())
            .height,
        Some(FaceHeight::Absolute(66))
    );
    // At 96 DPI, GNU's sequential 66-tenths height opens as 9 pixels.
    assert_eq!(
        eval.eval_str("(font-get (font-spec :name (face-font 'height-outer)) :size)")
            .unwrap()
            .as_int(),
        Some(9)
    );
    eval.eval_str("(setq face-remapping-alist '((height-parent (:height 200))))")
        .unwrap();
    assert_eq!(
        eval.eval_str("(font-get (font-spec :name (face-font 'height-child)) :size)")
            .unwrap()
            .as_int(),
        Some(13)
    );
}
