;;; Load succeeds but the resulting property table is not C-usable.
(setcdr (assq 'titlecase char-code-property-alist)
        (make-char-table 'case-table ?R))
