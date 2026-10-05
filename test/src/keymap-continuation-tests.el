;;; keymap-continuation-tests.el --- Internal keymap iteration -*- lexical-binding: t; -*-

(require 'ert)

(ert-deftest keymap-continuation-two-components-exact-cdr ()
  (let* ((first (make-sparse-keymap))
         (second (make-sparse-keymap))
         (map (make-composed-keymap (list first second)))
         (calls nil)
         (tail (map-keymap-internal (lambda (&rest args) (push args calls)) map)))
    (should (eq tail (cdr map)))
    (should (eq (car tail) first))
    (should (eq (cadr tail) second))
    (should-not calls)))

(ert-deftest keymap-continuation-nested-composition-exact-cdr ()
  (let* ((a (make-sparse-keymap))
         (b (make-sparse-keymap))
         (nested (make-composed-keymap (list a b)))
         (c (make-sparse-keymap))
         (map (make-composed-keymap (list nested c))))
    (should (eq (map-keymap-internal #'ignore map) (cdr map)))
    (should (eq (car (map-keymap-internal #'ignore map)) nested))
    (should (eq (map-keymap-internal #'ignore nested) (cdr nested)))))

(ert-deftest keymap-continuation-local-bindings-components-and-parent ()
  (let* ((a (make-sparse-keymap))
         (b (make-sparse-keymap))
         (parent (make-sparse-keymap))
         (map (make-composed-keymap (list a b) parent))
         (tail (cdr map))
         (seen nil))
    ;; `define-key' on a composed map edits its first component; splice
    ;; genuine local entries before the component spine instead.
    (setcdr map (cons (cons 98 'local-b) (cons (cons 97 'local-a) tail)))
    (should (eq (map-keymap-internal
                 (lambda (event binding) (push (cons event binding) seen)) map)
                tail))
    (should (equal (nreverse seen) '((98 . local-b) (97 . local-a))))
    (should (eq (cddr tail) parent))))

(ert-deftest keymap-continuation-ordinary-parent-and-empty ()
  (let ((child (make-sparse-keymap))
        (parent (make-sparse-keymap))
        (seen nil))
    (define-key child [97] 'local)
    (define-key parent [98] 'inherited)
    (set-keymap-parent child parent)
    (should (eq (map-keymap-internal
                 (lambda (event binding) (push (cons event binding) seen)) child)
                parent))
    (should (equal seen '((97 . local))))
    (should-not (map-keymap-internal #'ignore (make-sparse-keymap)))))

(ert-deftest keymap-continuation-direct-lookup-and-map-keymap-order ()
  (let ((a (make-sparse-keymap)) (b (make-sparse-keymap)) (seen nil))
    (define-key a [97] 'first-a)
    (define-key b [97] 'second-a)
    (define-key b [98] 'second-b)
    (let ((map (make-composed-keymap (list a b))))
      (should (eq (lookup-key map [97]) 'first-a))
      (should (eq (lookup-key map [98]) 'second-b))
      (map-keymap (lambda (event binding) (push (cons event binding) seen)) map)
      (should (equal (nreverse seen)
                     '((97 . first-a) (98 . second-b) (97 . second-a)))))))

(ert-deftest keymap-continuation-callback-detach-and-gc ()
  ;; After detaching MAP's cdr, no Lisp reference keeps either the remaining
  ;; local binding or composed maps alive: the iterator must root its plan.
  (let* ((map (list 'keymap
                    (cons 97 (copy-sequence "local-a"))
                    (cons 98 (copy-sequence "local-b"))
                    (list 'keymap (cons 99 (copy-sequence "component-c")))
                    (list 'keymap (cons 100 (copy-sequence "component-d")))))
         (seen nil)
         (tail (map-keymap-internal
                (lambda (event binding)
                  (setcdr map nil)
                  (garbage-collect)
                  (push (cons event binding) seen))
                map)))
    (should (equal (nreverse seen) '((97 . "local-a") (98 . "local-b"))))
    (should (equal (lookup-key (car tail) [99]) "component-c"))
    (should (equal (lookup-key (cadr tail) [100]) "component-d"))
    (should (equal map '(keymap)))))

(ert-deftest keymap-continuation-callback-nonlocal-exit ()
  (let ((map (list 'keymap (cons 97 'binding) (make-sparse-keymap))))
    (should (eq (catch 'done
                  (map-keymap-internal
                   (lambda (&rest _) (garbage-collect) (throw 'done 'escaped)) map))
                'escaped))
    (should-error (map-keymap-internal (lambda (&rest _) (error "callback")) map))
    (garbage-collect)
    (should (eq (map-keymap-internal #'ignore map) (cddr map)))))

(provide 'keymap-continuation-tests)
;;; keymap-continuation-tests.el ends here
