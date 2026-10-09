;;; Source-only r019 real lazy-load fixture; no process or editor operation.
(when (boundp 'neo-r019-observed-coding)
  (setq neo-r019-observed-coding coding-system-for-read))
(string-match "x" "x")
(let ((table (make-char-table 'char-code-property-table nil)))
  (set-char-table-range table ?é ?é)
  (setcdr (assq 'titlecase char-code-property-alist) table))
(when (boundp 'neo-r019-load-log)
  (setq neo-r019-load-log (append neo-r019-load-log '(titlecase))))
