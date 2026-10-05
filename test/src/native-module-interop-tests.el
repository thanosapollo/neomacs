;;; native-module-interop-tests.el --- Native ABI regressions -*- lexical-binding: t; -*-

;; Build emacs-module-resources/native-interop.c as a shared module, then set
;; NEOMACS_MODULE_INTEROP_SO to its absolute path and load this file with -Q.
;; No personal initialization or account configuration is needed.
(require 'ert)
(require 'cl-lib)
(module-load (or (getenv "NEOMACS_MODULE_INTEROP_SO")
                 (error "NEOMACS_MODULE_INTEROP_SO must name the test module")))
(defvar native-interop-special 'outer)

(ert-deftest native-interop-binary-copy ()
  ;; Every copy also asserts short-buffer sizing/error, no partial writes,
  ;; exact output size, embedded NUL, terminator and canaries in C.
  (dolist (bytes (list (unibyte-string) (unibyte-string 65 0 66)
                      (unibyte-string 128) (unibyte-string 255)
                      (unibyte-string 195 169)
                      (apply #'unibyte-string (number-sequence 0 255))))
    (should (equal bytes (native-interop-copy bytes)))))

(ert-deftest native-interop-unicode-copy ()
  (dolist (text '("" "ASCII\0tail" "Ελληνικά é 😀"))
    (should (equal (encode-coding-string text 'utf-8 t)
                   (native-interop-copy text)))))

(ert-deftest native-interop-reject-multibyte-raw-octets ()
  (dolist (bytes (list (unibyte-string 255) (unibyte-string 128)))
    (let ((raw (string-as-multibyte bytes)))
      (should (multibyte-string-p raw))
      (should (equal (should-error (native-interop-copy raw) :type 'wrong-type-argument)
                     (list 'wrong-type-argument 'unicode-string-p raw)))))
  (should (equal (should-error (native-interop-copy 17) :type 'wrong-type-argument)
                 '(wrong-type-argument stringp 17))))

(ert-deftest native-interop-capture-unmatched-throw ()
  (let ((calls 0) cleaned)
    (should (equal '(2 done 17 42)
                   (native-interop-capture
                    (lambda () (let ((native-interop-special 'inner))
                                 (unwind-protect (throw 'done 17) (setq cleaned t))))
                    (lambda () (cl-incf calls) 42))))
    (should cleaned)
    (should (= calls 1))
    (should (eq native-interop-special 'outer)))
  ;; nil is not a valid catch tag, even at GNU's native wildcard boundary.
  (should (equal '(1 no-catch (nil 17) 42)
                 (native-interop-capture (lambda () (throw nil 17)) (lambda () 42))))
  (should-error (throw 'not-caught 17) :type 'no-catch))

(ert-deftest native-interop-captured-error-quit-hide-caller-handlers ()
  (dolist (kind '(error quit))
    (let ((outer 0) (inner 0) (calls 0) cleaned)
      (should
       (equal (list 1 kind (if (eq kind 'error) '("fixture") nil) 42)
              (handler-bind
                  (((error quit) (lambda (_) (cl-incf outer))))
                (native-interop-capture
                 (lambda ()
                   (handler-bind
                       (((error quit) (lambda (_) (cl-incf inner))))
                     (let ((native-interop-special 'inner))
                       (unwind-protect
                           (signal kind (if (eq kind 'error) '("fixture") nil))
                         (setq cleaned t)))))
                 (lambda () (cl-incf calls) 42)))))
      (should (= outer 0)) (should (= inner 1)) (should (= calls 1))
      (should cleaned) (should (eq native-interop-special 'outer)))))

(ert-deftest native-interop-propagated-signals-reach-caller-once ()
  (dolist (kind '(error quit))
    (let ((outer 0) caught cleaned)
      (condition-case err
          (handler-bind
              (((error quit) (lambda (_) (cl-incf outer))))
            (unwind-protect
                (native-interop-propagate
                 (lambda () (signal kind (if (eq kind 'error) '("fixture") nil))))
              (setq cleaned t)))
        ((error quit) (setq caught err)))
      (should (equal caught (cons kind (if (eq kind 'error) '("fixture") nil))))
      (should (= outer 1)) (should cleaned)))
  (should (equal 'escaped
                 (catch 'done (native-interop-propagate (lambda () (throw 'done 'escaped)))))))

(ert-deftest native-interop-inner-handler-escape-is-native-throw ()
  (let ((outer 0))
    (should (equal '(2 handler-escape 19 42)
                   (handler-bind ((error (lambda (_) (cl-incf outer))))
                     (native-interop-capture
                      (lambda () (handler-bind ((error (lambda (_) (throw 'handler-escape 19))))
                                   (error "fixture")))
                      (lambda () 42)))))
    (should (= outer 0))))

(ert-deftest native-interop-nested-contexts-and-inner-catches ()
  (should (= 17 (native-interop-propagate (lambda () (catch 'done (throw 'done 17))))))
  (should (= 18 (native-interop-propagate
                 (lambda () (condition-case nil (error "inner") (error 18))))))
  (let ((native-interop-special 'nested) (outer 0))
    (should (equal '(nested (1 error ("inner") 42) 43)
                   (handler-bind ((error (lambda (_) (cl-incf outer))))
                     (native-interop-propagate
                      (lambda () (list native-interop-special
                                       (native-interop-capture (lambda () (error "inner")) (lambda () 42))
                                       (native-interop-propagate (lambda () 43))))))))
    (should (= outer 0)))
  (should (= 44 (native-interop-propagate (lambda () 44))))
  (should (eq native-interop-special 'outer)))

(ert-deftest native-interop-caller-handler-escape-only-after-propagation ()
  (should (equal '(1 error ("fixture") 42)
                 (catch 'outer-escape
                   (handler-bind ((error (lambda (_) (throw 'outer-escape 'too-early))))
                     (native-interop-capture (lambda () (error "fixture")) (lambda () 42))))))
  (should (eq 'propagated
              (catch 'outer-escape
                (handler-bind ((error (lambda (_) (throw 'outer-escape 'propagated))))
                  (native-interop-propagate (lambda () (error "fixture"))))))))

(ert-deftest native-interop-bytecompiled-callback-exits ()
  (require 'bytecomp)
  (should (equal '(2 compiled-tag 17 42)
                 (native-interop-capture
                  (byte-compile (lambda () (throw 'compiled-tag 17)))
                  (lambda () 42))))
  (should (= 18 (native-interop-propagate
                 (byte-compile (lambda () (catch 'done (throw 'done 18)))))))
  (let ((count 0))
    (should (equal '(1 error ("compiled") 42)
                   (handler-bind ((error (lambda (_) (cl-incf count))))
                     (native-interop-capture
                      (byte-compile (lambda () (error "compiled")))
                      (lambda () 42)))))
    (should (= count 0))))

(provide 'native-module-interop-tests)
;;; native-module-interop-tests.el ends here
