;;; process_pre_write_lifetime.el --- Name-only send lifetime regressions -*- lexical-binding: t; -*-

;; Run only under the approved process sandbox. These forms are shared by the
;; deleted-process ownership tests and the live GNU oracle comparison. No
;; process handle is kept in a variable, sentinel closure, or process plist.

(defun dps-pre-write-lifetime-hook (_from _to)
  (unwind-protect
      (progn
        (delete-process "dps-pre-write")
        (garbage-collect)
        (garbage-collect)
        ;; Check before reading the weak key: reading it must not mask a missing
        ;; root during the collections above.
        (setq dps-pre-write-inside-count
              (hash-table-count dps-pre-write-weak))
        (maphash
         (lambda (process _value)
           ;; Store only scalar/string state, never the process itself. These
           ;; accessors also require the retained deleted record to exist.
           (setq dps-pre-write-inside-state
                 (list (processp process) (process-status process)
                       (process-name process)
                       (consp (process-coding-system process)))))
         dps-pre-write-weak)
        (when dps-pre-write-throw
          (throw 'dps-pre-write-exit 'escaped)))
    (garbage-collect)
    (garbage-collect)
    (setq dps-pre-write-cleanup-count
          (hash-table-count dps-pre-write-weak))))

(defun dps-pre-write-lifetime-setup (throwp)
  (setq dps-pre-write-weak (make-hash-table :weakness 'key)
        dps-pre-write-inside-count nil
        dps-pre-write-inside-state nil
        dps-pre-write-cleanup-count nil
        dps-pre-write-result nil
        dps-pre-write-throw throwp)
  (define-coding-system 'dps-pre-write-coding "Process send lifetime regression"
    :coding-type 'utf-8 :mnemonic ?L
    :pre-write-conversion 'dps-pre-write-lifetime-hook)
  (puthash (make-pipe-process :name "dps-pre-write" :buffer nil :noquery t
                             :coding '(binary . dps-pre-write-coding)
                             :sentinel #'ignore)
           t dps-pre-write-weak)
  nil)

(defun dps-pre-write-lifetime-send (primitive)
  (unwind-protect
      (setq dps-pre-write-result
            (catch 'dps-pre-write-exit
              (condition-case err
                  (with-temp-buffer
                    (insert "x")
                    (if (eq primitive 'process-send-string)
                        (process-send-string "dps-pre-write" "x")
                      (process-send-region "dps-pre-write" (point-min) (point-max)))
                    'sent)
                (error err))))
    (when (get-process "dps-pre-write")
      (delete-process "dps-pre-write")))
  ;; Neither this result nor any recorded state retains the weak key.
  (list dps-pre-write-inside-count dps-pre-write-inside-state
        dps-pre-write-cleanup-count dps-pre-write-result))
