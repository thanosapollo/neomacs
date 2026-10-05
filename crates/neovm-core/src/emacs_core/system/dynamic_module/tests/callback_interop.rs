use super::*;

unsafe extern "C" fn capture_thunk(
    env: *mut emacs_env,
    _nargs: isize,
    _args: *mut emacs_value,
    _data: *mut c_void,
) -> emacs_value {
    unsafe {
        let e = &*env;
        let thunk = (e.intern.unwrap())(env, c"native-interop-thunk".as_ptr());
        let result = (e.funcall.unwrap())(env, thunk, 0, std::ptr::null_mut());
        let mut symbol = std::ptr::null_mut();
        let mut data = std::ptr::null_mut();
        let code = (e.non_local_exit_get.unwrap())(env, &mut symbol, &mut data);
        if code == emacs_funcall_exit::Return {
            return result;
        }
        // While pending, even a valid Lisp call must be blocked and leave the
        // original tag/signal/data intact. Handles remain local env GC roots.
        let blocked = (e.funcall.unwrap())(env, thunk, 0, std::ptr::null_mut());
        if !blocked.is_null() || (e.non_local_exit_check.unwrap())(env) != code {
            return std::ptr::null_mut();
        }
        (e.non_local_exit_clear.unwrap())(env);
        let mut args = [(e.make_integer.unwrap())(env, code as i64), symbol, data];
        let list = (e.intern.unwrap())(env, c"list".as_ptr());
        (e.funcall.unwrap())(env, list, 3, args.as_mut_ptr())
    }
}

unsafe extern "C" fn propagate_thunk(
    env: *mut emacs_env,
    _nargs: isize,
    _args: *mut emacs_value,
    _data: *mut c_void,
) -> emacs_value {
    unsafe {
        let e = &*env;
        let thunk = (e.intern.unwrap())(env, c"native-interop-thunk".as_ptr());
        (e.funcall.unwrap())(env, thunk, 0, std::ptr::null_mut())
    }
}

fn context_with_native_functions() -> Context {
    let mut ctx = Context::new();
    install_module_function(&mut ctx, "native-interop-capture", capture_thunk);
    install_module_function(&mut ctx, "native-interop-propagate", propagate_thunk);
    ctx.eval_str("(defvar native-interop-special 'outer)")
        .unwrap();
    ctx
}

fn assert_balanced(ctx: &mut Context, roots: usize) {
    assert_eq!(ctx.condition_stack.len(), 0);
    assert_eq!(ctx.specpdl.len(), 0);
    assert_eq!(ctx.gc_inhibit_depth, 0);
    assert_eq!(crate::emacs_core::eval::save_scratch_gc_roots(), roots);
    assert!(MODULE_CTX.with(|cell| cell.get()).is_null());
    assert_eq!(
        ctx.eval_str("native-interop-special").unwrap(),
        Value::symbol("outer")
    );
    assert_eq!(ctx.eval_str("(+ 20 22)").unwrap(), Value::fixnum(42));
    ctx.eval_str("(garbage-collect)").unwrap();
}

#[test]
fn module_callback_intercepts_unmatched_throw_and_restores_bindings() {
    let mut ctx = context_with_native_functions();
    let roots = crate::emacs_core::eval::save_scratch_gc_roots();
    assert_eq!(
        ctx.eval_str(
            r#"(progn
        (defvar native-interop-cleaned nil)
        (fset 'native-interop-thunk (lambda ()
          (let ((native-interop-special 'inner))
            (unwind-protect (throw 'native-done 17) (setq native-interop-cleaned t)))))
        (and (equal (native-interop-capture) '(2 native-done 17)) native-interop-cleaned))"#
        )
        .unwrap(),
        Value::T
    );
    assert_balanced(&mut ctx, roots);
    assert_eq!(
        ctx.eval_str("(condition-case err (throw 'outside 18) (no-catch (car err)))")
            .unwrap(),
        Value::symbol("no-catch")
    );
    assert_eq!(ctx.eval_str("(progn (fset 'native-interop-thunk (lambda () (throw nil 19))) (equal (native-interop-capture) '(1 no-catch (nil 19))))").unwrap(), Value::T);
}

#[test]
fn module_callback_hides_caller_handlers_but_keeps_inner_handlers() {
    let mut ctx = context_with_native_functions();
    let roots = crate::emacs_core::eval::save_scratch_gc_roots();
    for condition in ["error", "quit"] {
        let source = format!(
            r#"(progn
          (defvar native-interop-outer 0) (setq native-interop-outer 0)
          (defvar native-interop-inner 0) (setq native-interop-inner 0)
          (fset 'native-interop-thunk (lambda ()
            (handler-bind-1 (lambda () (signal '{condition} '(17)))
                            '(error quit)
                            (lambda (_) (setq native-interop-inner (+ native-interop-inner 1))))))
          (let ((captured (handler-bind-1 (lambda () (native-interop-capture))
                                         '(error quit)
                                         (lambda (_) (setq native-interop-outer (+ native-interop-outer 1))))))
            (and (equal captured '(1 {condition} (17)))
                 (= native-interop-outer 0) (= native-interop-inner 1))))"#
        );
        assert_eq!(ctx.eval_str(&source).unwrap(), Value::T, "{condition}");
        assert_balanced(&mut ctx, roots);
    }
}

#[test]
fn module_callback_propagated_signal_runs_caller_handler_once() {
    let mut ctx = context_with_native_functions();
    let roots = crate::emacs_core::eval::save_scratch_gc_roots();
    for condition in ["error", "quit"] {
        let source = format!(
            r#"(progn
          (defvar native-interop-count 0) (setq native-interop-count 0)
          (fset 'native-interop-thunk (lambda () (signal '{condition} '(17))))
          (let ((caught (condition-case err
              (handler-bind-1 (lambda () (native-interop-propagate)) '(error quit)
                              (lambda (_) (setq native-interop-count (+ native-interop-count 1))))
              ((error quit) err))))
            (and (equal caught '({condition} 17)) (= native-interop-count 1))))"#
        );
        assert_eq!(ctx.eval_str(&source).unwrap(), Value::T, "{condition}");
        assert_balanced(&mut ctx, roots);
    }
    assert_eq!(ctx.eval_str("(progn (fset 'native-interop-thunk (lambda () (throw 'outer-catch 19))) (catch 'outer-catch (native-interop-propagate)))").unwrap(), Value::fixnum(19));
}

#[test]
fn module_callback_inner_handler_escape_is_throw_not_no_catch() {
    let mut ctx = context_with_native_functions();
    let roots = crate::emacs_core::eval::save_scratch_gc_roots();
    assert_eq!(
        ctx.eval_str(
            r#"(progn
      (fset 'native-interop-thunk (lambda ()
        (handler-bind-1 (lambda () (signal 'error '(17))) '(error)
                        (lambda (_) (throw 'handler-escape 19)))))
      (equal (native-interop-capture) '(2 handler-escape 19)))"#
        )
        .unwrap(),
        Value::T
    );
    assert_balanced(&mut ctx, roots);
}

#[test]
fn module_callback_inner_condition_case_and_catch_take_priority() {
    let mut ctx = context_with_native_functions();
    let roots = crate::emacs_core::eval::save_scratch_gc_roots();
    for body in [
        "(catch 'done (throw 'done 17))",
        "(condition-case nil (signal 'error '(18)) (error 17))",
    ] {
        assert_eq!(
            ctx.eval_str(&format!(
                "(progn (fset 'native-interop-thunk (lambda () {body})) (native-interop-capture))"
            ))
            .unwrap(),
            Value::fixnum(17)
        );
        assert_balanced(&mut ctx, roots);
    }
}
