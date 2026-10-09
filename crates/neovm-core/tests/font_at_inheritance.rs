//! Regression for `font-at` querying an Org-style named face which inherits
//! its height and weight. Captures the real evaluator's host request without
//! fabricating an opened font or starting a graphical renderer.
use neovm_core::emacs_core::display_host::FontResolveRequest;
use neovm_core::emacs_core::eval::{DisplayHost, GuiFrameHostRequest, ResolvedFontMatch};
use neovm_core::emacs_core::{Context, Value};
use neovm_core::face::{FaceHeight, FontWeight};
use std::cell::RefCell;
use std::rc::Rc;

struct CapturingHost(Rc<RefCell<Option<FontResolveRequest>>>);
impl DisplayHost for CapturingHost {
    fn realize_gui_frame(&mut self, _: GuiFrameHostRequest) -> Result<(), String> {
        Ok(())
    }
    fn resize_gui_frame(&mut self, _: GuiFrameHostRequest) -> Result<(), String> {
        Ok(())
    }
    fn resolve_font_for_char(
        &mut self,
        request: FontResolveRequest,
    ) -> Result<Option<ResolvedFontMatch>, String> {
        *self.0.borrow_mut() = Some(request);
        Ok(None)
    }
}

fn check_named_inheritance(buffer: bool) {
    std::thread::Builder::new()
        .stack_size(128 * 1024 * 1024)
        .spawn(move || {
            let mut eval = Context::new();
            eval.eval_str("(selected-frame)").unwrap();
            let frame_id = eval.frame_manager().selected_frame().unwrap().id;
            eval.frame_manager_mut()
                .get_mut(frame_id)
                .unwrap()
                .set_window_system(Some(Value::symbol("neo")));
            let captured = Rc::new(RefCell::new(None));
            eval.set_display_host(Box::new(CapturingHost(captured.clone())));
            eval.eval_str(
                "(internal-make-lisp-face 'font-at-parent)
                 (internal-set-lisp-face-attribute 'font-at-parent :height 1.4 nil)
                 (internal-set-lisp-face-attribute 'font-at-parent :weight 'extra-bold nil)
                 (internal-make-lisp-face 'font-at-child)
                 (internal-set-lisp-face-attribute 'font-at-child :inherit 'font-at-parent nil)",
            )
            .unwrap();
            // Control: the backend boundary receives the correct attributes
            // when the same face is spelled as a direct anonymous plist.
            assert!(
                eval.eval_str(
                    "(font-at 0 nil (propertize \"Κ\" 'face '(:height 1.4 :weight extra-bold)))",
                )
                .unwrap()
                .is_nil()
            );
            let direct = captured.borrow().as_ref().unwrap().face.clone();
            assert_eq!(direct.weight, Some(FontWeight::EXTRA_BOLD));
            assert_eq!(direct.height, Some(FaceHeight::Relative(1.4)));
            let expression = if buffer {
                "(insert (propertize \"* Καρδιακός\" 'face 'font-at-child)) (font-at 3)"
            } else {
                "(font-at 0 nil (propertize \"Κ\" 'face 'font-at-child))"
            };
            captured.borrow_mut().take();
            assert!(eval.eval_str(expression).unwrap().is_nil());
            let named_request = captured
                .borrow_mut()
                .take()
                .expect("named font-at query must call the display host");
            assert_eq!(named_request.character.code(), u32::from('Κ'));
            let named = named_request.face;
            eprintln!(
                "named request: weight={:?}, height={:?}",
                named.weight, named.height
            );
            assert_eq!(
                named.weight, direct.weight,
                "named face must retain inherited weight"
            );
            assert_eq!(
                named.height, direct.height,
                "named face must retain inherited height"
            );
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn font_at_buffer_named_face_retains_inherited_font_attributes() {
    check_named_inheritance(true);
}

#[test]
fn font_at_string_named_face_retains_inherited_font_attributes() {
    check_named_inheritance(false);
}

fn check_query(setup: &str, query: &str, height: FaceHeight, weight: FontWeight) {
    let setup = setup.to_owned();
    let query = query.to_owned();
    std::thread::Builder::new()
        .stack_size(128 * 1024 * 1024)
        .spawn(move || {
            let mut eval = Context::new();
            eval.eval_str("(selected-frame)").unwrap();
            let id = eval.frame_manager().selected_frame().unwrap().id;
            eval.frame_manager_mut()
                .get_mut(id)
                .unwrap()
                .set_window_system(Some(Value::symbol("neo")));
            let captured = Rc::new(RefCell::new(None));
            eval.set_display_host(Box::new(CapturingHost(captured.clone())));
            eval.eval_str(&setup).unwrap();
            // font-at deliberately preserves the runtime default baseline,
            // rather than replacing it with the frame's concrete Lisp height.
            // Establish that real production baseline for these composition guards.
            eval.face_table_mut().set_attribute(
                "default",
                neovm_core::face::LFaceAttr::Height,
                neovm_core::face::FaceAttrValue::Height(FaceHeight::Absolute(150)),
            );
            captured.borrow_mut().take();
            assert!(eval.eval_str(&query).unwrap().is_nil());
            let request = captured.borrow_mut().take().expect("fresh host request");
            assert_eq!(request.character.code(), u32::from('Κ'));
            assert_eq!(request.face.height, Some(height));
            assert_eq!(request.face.weight, Some(weight));
        })
        .unwrap()
        .join()
        .unwrap();
}

const NAMED_QUERY: &str = "(font-at 0 nil (propertize \"Κ\" 'face 'font-at-child))";
const PARENT_CHILD: &str = "
    (internal-make-lisp-face 'font-at-parent)
    (internal-set-lisp-face-attribute 'font-at-parent :height 1.4 nil)
    (internal-set-lisp-face-attribute 'font-at-parent :weight 'extra-bold nil)
    (internal-make-lisp-face 'font-at-child)
    (internal-set-lisp-face-attribute 'font-at-child :inherit 'font-at-parent nil)";

#[test]
fn font_at_inherited_relative_height_applies_once_to_default() {
    check_query(
        &format!(
            "{PARENT_CHILD}
        (internal-set-lisp-face-attribute 'default :height 150 nil)"
        ),
        NAMED_QUERY,
        FaceHeight::Absolute(210),
        FontWeight::EXTRA_BOLD,
    );
}

#[test]
fn font_at_child_attributes_override_inherited_attributes() {
    check_query(
        &format!(
            "{PARENT_CHILD}
        (internal-set-lisp-face-attribute 'font-at-child :height 180 nil)
        (internal-set-lisp-face-attribute 'font-at-child :weight 'normal nil)"
        ),
        NAMED_QUERY,
        FaceHeight::Absolute(180),
        FontWeight::NORMAL,
    );
}

#[test]
fn font_at_named_face_list_first_entry_wins() {
    check_query(
        &format!(
            "{PARENT_CHILD}
        (internal-make-lisp-face 'font-at-second)
        (internal-set-lisp-face-attribute 'font-at-second :height 300 nil)
        (internal-set-lisp-face-attribute 'font-at-second :weight 'normal nil)
        (internal-set-lisp-face-attribute 'font-at-child :height 180 nil)"
        ),
        "(font-at 0 nil (propertize \"Κ\" 'face '(font-at-child font-at-second)))",
        FaceHeight::Absolute(180),
        FontWeight::EXTRA_BOLD,
    );
}

#[test]
fn font_at_inheritance_list_first_parent_wins() {
    check_query(&format!("{PARENT_CHILD}
        (internal-make-lisp-face 'font-at-second)
        (internal-set-lisp-face-attribute 'font-at-second :height 300 nil)
        (internal-set-lisp-face-attribute 'font-at-second :weight 'normal nil)
        (internal-set-lisp-face-attribute 'font-at-parent :height 180 nil)
        (internal-set-lisp-face-attribute 'font-at-child :inherit '(font-at-parent font-at-second) nil)"),
        NAMED_QUERY, FaceHeight::Absolute(180), FontWeight::EXTRA_BOLD);
}

#[test]
fn font_at_named_overlay_does_not_inject_default_attributes() {
    check_query(
        "(internal-make-lisp-face 'font-at-family)
        (internal-set-lisp-face-attribute 'font-at-family :family \"Test Mono\" nil)
        (insert (propertize \"Κ\" 'face '(:height 200 :weight extra-bold)))
        (setq font-at-overlay (make-overlay 1 2))
        (overlay-put font-at-overlay 'face 'font-at-family)",
        "(font-at 1)",
        FaceHeight::Absolute(200),
        FontWeight::EXTRA_BOLD,
    );
}

#[test]
fn font_at_remapped_named_child_keeps_inheritance_and_relative_height() {
    check_query(
        &format!(
            "{PARENT_CHILD}
        (internal-set-lisp-face-attribute 'default :height 150 nil)
        (internal-set-lisp-face-attribute 'font-at-parent :height 150 nil)
        (setq face-remapping-alist '((font-at-child (:height 1.2) font-at-child)))"
        ),
        NAMED_QUERY,
        FaceHeight::Absolute(180),
        FontWeight::EXTRA_BOLD,
    );
}

#[test]
fn font_at_remapped_default_is_not_applied_twice_by_named_face() {
    check_query(
        "(internal-set-lisp-face-attribute 'default :height 150 nil)
        (internal-make-lisp-face 'font-at-child)
        (internal-set-lisp-face-attribute 'font-at-child :family \"Test Mono\" nil)
        (setq face-remapping-alist '((default (:height 1.2) default)))",
        NAMED_QUERY,
        FaceHeight::Absolute(180),
        FontWeight::NORMAL,
    );
}
