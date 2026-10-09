;;; Character casing prepares this property before selecting current Up.
(setcdr (assq 'special-lowercase char-code-property-alist)
        (make-char-table 'char-code-property-table nil))
(setq neo-r019-load-log (append neo-r019-load-log '(special-lowercase)))
