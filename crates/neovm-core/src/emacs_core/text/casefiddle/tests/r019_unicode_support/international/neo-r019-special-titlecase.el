;;; Final lazy preparation boundary; an after-load callback changes Up.
(setcdr (assq 'special-titlecase char-code-property-alist)
        (make-char-table 'char-code-property-table nil))
(setq neo-r019-load-log (append neo-r019-load-log '(special-titlecase)))
