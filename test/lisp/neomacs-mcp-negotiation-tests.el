;;; neomacs-mcp-negotiation-tests.el --- Legacy MCP negotiation -*- lexical-binding: t; -*-
(require 'ert)
(require 'neomacs-mcp)

(defvar neomacs-mcp-test-negotiation-effect)

(defmacro neomacs-mcp-test-with-peer (&rest body)
  "Run BODY with a fresh process-property fixture named peer."
  (declare (indent 0) (debug t))
  `(let ((peer (make-process :name "mcp-negotiation-fixture"
                             :command '("cat") :connection-type 'pipe
                             :noquery t)))
     (unwind-protect (progn ,@body)
       (when (process-live-p peer) (delete-process peer)))))

(defun neomacs-mcp-test-initialize (peer version)
  "Initialize PEER with the offered protocol VERSION."
  (neomacs-mcp--dispatch
   peer (neomacs-mcp--object
         "method" "initialize" "params"
         (neomacs-mcp--object "protocolVersion" version
                              "capabilities" (neomacs-mcp--object)
                              "clientInfo" (neomacs-mcp--object
                                            "name" "codex-mcp-client"
                                            "version" "0.155.1")))))

(ert-deftest neomacs-mcp-negotiation-supported-legacy-offers ()
  (dolist (version '("2025-06-18" "2025-11-25"))
    (neomacs-mcp-test-with-peer
      (let ((result (neomacs-mcp-test-initialize peer version)))
        (should (equal version (gethash "protocolVersion" result)))
        (should (hash-table-p (gethash "tools" (gethash "capabilities" result))))
        (should-not (gethash "resultType" result))
        (should (eq 'initializing (process-get peer 'legacy)))))))

(ert-deftest neomacs-mcp-negotiation-unsupported-offer-counterproposal ()
  ;; The handshake negotiates legacy semantics, never stateless-modern semantics.
  (dolist (version '("2024-11-05" "1900-01-01" "2026-07-28"))
    (neomacs-mcp-test-with-peer
      (should (equal "2025-11-25"
                     (gethash "protocolVersion"
                              (neomacs-mcp-test-initialize peer version))))
      (should (eq 'initializing (process-get peer 'legacy))))))

(ert-deftest neomacs-mcp-negotiation-malformed-does-not-initialize ()
  (dolist (params
           (append
            (mapcar (lambda (version)
                      (neomacs-mcp--object "protocolVersion" version
                                           "capabilities" (neomacs-mcp--object)
                                           "clientInfo" (neomacs-mcp--object)))
                    '(:null 20250618 ""))
            (list (neomacs-mcp--object)
                  (neomacs-mcp--object "capabilities" (neomacs-mcp--object)
                                       "clientInfo" (neomacs-mcp--object))
                  (neomacs-mcp--object "protocolVersion" "2025-06-18"
                                       "capabilities" :null
                                       "clientInfo" (neomacs-mcp--object))
                  (neomacs-mcp--object "protocolVersion" "2025-06-18"
                                       "capabilities" (neomacs-mcp--object)
                                       "clientInfo" :null))))
    (neomacs-mcp-test-with-peer
      (let ((failure
             (should-error
              (neomacs-mcp--dispatch
               peer (neomacs-mcp--object "method" "initialize" "params" params))
              :type 'neomacs-mcp-protocol-error)))
        (should (= -32602 (nth 1 failure)))
        (should-not (process-get peer 'legacy))
        ;; A malformed offer must not poison a later valid handshake.
        (should (equal "2025-06-18"
                       (gethash "protocolVersion"
                                (neomacs-mcp-test-initialize peer "2025-06-18"))))))))

(ert-deftest neomacs-mcp-negotiation-readiness-and-repeated-initialize ()
  (dolist (version '("2025-06-18" "2025-11-25"))
    (neomacs-mcp-test-with-peer
      (should-error
       (neomacs-mcp--dispatch peer (neomacs-mcp--object "method" "notifications/initialized"))
       :type 'neomacs-mcp-protocol-error)
      (should-not (process-get peer 'legacy))
      (neomacs-mcp-test-initialize peer version)
      (let ((failure
             (should-error
              (neomacs-mcp--dispatch peer (neomacs-mcp--object "method" "tools/list"))
              :type 'neomacs-mcp-protocol-error)))
        (should (= -32600 (nth 1 failure))))
      (should-error (neomacs-mcp-test-initialize peer version)
                    :type 'neomacs-mcp-protocol-error)
      (should (eq 'initializing (process-get peer 'legacy)))
      (neomacs-mcp--dispatch peer (neomacs-mcp--object "method" "notifications/initialized"))
      (should (eq 'ready (process-get peer 'legacy)))
      (should-error (neomacs-mcp-test-initialize peer "2025-11-25")
                    :type 'neomacs-mcp-protocol-error)
      (should-error
       (neomacs-mcp--dispatch peer (neomacs-mcp--object "method" "notifications/initialized"))
       :type 'neomacs-mcp-protocol-error)
      (should (eq 'ready (process-get peer 'legacy)))
      (should (= 0 (hash-table-count
                    (neomacs-mcp--dispatch peer (neomacs-mcp--object "method" "ping"))))))))

(ert-deftest neomacs-mcp-negotiation-legacy-tool-envelopes-and-fence ()
  (dolist (version '("2025-06-18" "2025-11-25"))
    (neomacs-mcp-test-with-peer
      (neomacs-mcp-test-initialize peer version)
      (neomacs-mcp--dispatch peer (neomacs-mcp--object "method" "notifications/initialized"))
      (let* ((listed (neomacs-mcp--dispatch peer (neomacs-mcp--object "method" "tools/list")))
             (instance (gethash "instance" (neomacs-mcp-identity)))
             (neomacs-mcp-test-negotiation-effect nil)
             (params (neomacs-mcp--object
                      "name" "neomacs_eval" "arguments"
                      (neomacs-mcp--object "instance" "wrong" "code"
                                           "(setq neomacs-mcp-test-negotiation-effect t)")))
             (wrong (neomacs-mcp--dispatch
                     peer (neomacs-mcp--object "method" "tools/call" "params" params))))
        (should (vectorp (gethash "tools" listed)))
        (should-not (gethash "resultType" listed))
        (should (eq t (gethash "isError" wrong)))
        (should-not (gethash "resultType" wrong))
        (should-not neomacs-mcp-test-negotiation-effect)
        (puthash "arguments" (neomacs-mcp--object "instance" instance "code" "(+ 20 22)") params)
        (let ((result (neomacs-mcp--dispatch
                       peer (neomacs-mcp--object "method" "tools/call" "params" params))))
          (should (eq :false (gethash "isError" result)))
          (should (equal "42" (gethash "text" (aref (gethash "content" result) 0))))
          (should-not (gethash "resultType" result)))))))

(ert-deftest neomacs-mcp-negotiation-modern-discovery-all-versions ()
  (neomacs-mcp-test-with-peer
    (let* ((params (neomacs-mcp--object
                    "_meta" (neomacs-mcp--object
                             "io.modelcontextprotocol/protocolVersion" "2026-07-28"
                             "io.modelcontextprotocol/clientCapabilities" (neomacs-mcp--object))))
           (result (neomacs-mcp--dispatch
                    peer (neomacs-mcp--object "method" "server/discover" "params" params))))
      (should (equal ["2026-07-28" "2025-11-25" "2025-06-18"]
                     (gethash "supportedVersions" result)))
      (should (equal "complete" (gethash "resultType" result)))
      (should-not (process-get peer 'legacy)))))

(provide 'neomacs-mcp-negotiation-tests)
;;; neomacs-mcp-negotiation-tests.el ends here
