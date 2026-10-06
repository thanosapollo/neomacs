;;; native-module-omemo-roundtrip-tests.el --- Real module consumption -*- lexical-binding: t; -*-

;; Explicit opt-in acceptance against the same installed OMEMO module in GNU
;; and Neomacs. Set NEOMACS_OMEMO_MODULE to an absolute .so path and run with
;; -Q in a disposable HOME/XDG environment. Do not load Jabber/account init.
(require 'ert)
(require 'cl-lib)
(require 'sqlite)
(module-load (or (getenv "NEOMACS_OMEMO_MODULE")
                 (error "NEOMACS_OMEMO_MODULE must name the actual OMEMO module")))

(ert-deftest native-interop-omemo-store-roundtrip ()
  (let ((blob (jabber-omemo--setup-store)))
    (should-not (multibyte-string-p blob))
    (should (cl-some (lambda (byte) (> byte 127)) (string-to-list blob)))
    (should (user-ptrp (jabber-omemo--deserialize-store blob)))))

(ert-deftest native-interop-omemo-crypto-roundtrip ()
  (let* ((text "fixture Greek Ελληνικά")
         (encrypted (jabber-omemo--encrypt-message text)))
    (dolist (field '(:key :iv :ciphertext))
      (should (stringp (plist-get encrypted field)))
      (should-not (multibyte-string-p (plist-get encrypted field))))
    (should (equal (encode-coding-string text 'utf-8 t)
                   (jabber-omemo--decrypt-message
                    (plist-get encrypted :key) (plist-get encrypted :iv)
                    (plist-get encrypted :ciphertext))))))

(ert-deftest native-interop-omemo-sqlite-close-reopen ()
  (let* ((path (make-temp-file "native-omemo-" nil ".sqlite"))
         (db (sqlite-open path))
         (blob (jabber-omemo--setup-store)))
    (unwind-protect
        (progn
          (sqlite-execute db "CREATE TABLE stores (value BLOB)")
          (sqlite-execute db "INSERT INTO stores VALUES (?)"
                          (list (propertize blob 'coding-system 'binary)))
          (sqlite-close db)
          (setq db (sqlite-open path))
          (let ((stored (caar (sqlite-select db "SELECT value FROM stores"))))
            (should (equal blob stored))
            (should-not (multibyte-string-p stored))
            (should (equal '(("blob")) (sqlite-select db "SELECT typeof(value) FROM stores")))
            (should (equal '(("ok")) (sqlite-select db "PRAGMA integrity_check")))
            (should (user-ptrp (jabber-omemo--deserialize-store stored)))))
      (sqlite-close db)
      (delete-file path))))

(provide 'native-module-omemo-roundtrip-tests)
;;; native-module-omemo-roundtrip-tests.el ends here
