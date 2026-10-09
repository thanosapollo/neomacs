;;; Remove all Lisp reachability to the previously prepared title table.
(setq char-code-property-alist nil)
(garbage-collect)
