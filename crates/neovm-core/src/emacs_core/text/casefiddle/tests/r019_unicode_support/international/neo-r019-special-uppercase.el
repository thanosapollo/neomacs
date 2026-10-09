;;; Character casing prepares this property but does not emit special text.
(setcdr (assq 'special-uppercase char-code-property-alist)
        (make-char-table 'char-code-property-table nil))
(setq neo-r019-load-log (append neo-r019-load-log '(special-uppercase)))
