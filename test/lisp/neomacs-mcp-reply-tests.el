;;; neomacs-mcp-reply-tests.el --- Error replies and full access -*- lexical-binding: t; -*-
(require 'ert)
(require 'cl-lib)
(require 'neomacs-mcp)
(require 'neomacs-mcp-companion)

(defvar neomacs-mcp-test-reply-effect nil)

(defun neomacs-mcp-test--call-line (id name arguments)
  "Return a modern tools/call request line for tool NAME with ARGUMENTS and ID."
  (concat
   (json-serialize
    (neomacs-mcp--object
     "jsonrpc" "2.0" "id" id "method" "tools/call" "params"
     (neomacs-mcp--object
      "name" name "arguments" arguments
      "_meta" (neomacs-mcp--object
               "io.modelcontextprotocol/protocolVersion" neomacs-mcp--modern
               "io.modelcontextprotocol/clientCapabilities" (neomacs-mcp--object)))))
   "\n"))

(defun neomacs-mcp-test--reply (client output line)
  "Send LINE from CLIENT and return the parsed reply collected in OUTPUT.
Return nil if the server closes the connection without replying."
  (setcar output "")
  (process-send-string client line)
  (with-timeout (10 (ert-fail "No reply within 10 seconds"))
    (while (and (process-live-p client) (not (string-search "\n" (car output))))
      (accept-process-output client 0.05)))
  (when (string-search "\n" (car output))
    (json-parse-string (car (split-string (car output) "\n")))))

(ert-deftest neomacs-mcp-unsendable-response-is-error-reply ()
  ;; ESC prints raw but JSON-escapes to 6 bytes: 30000 of them print to
  ;; about 30 KB but encode to about 180 KB, over the response limit.  A raw
  ;; byte, as in undecodable process output, cannot be encoded at all.
  ;; Either must produce an error reply for the request, not a silent
  ;; disconnect, and the connection must stay usable.
  (let* ((root (make-temp-file "neomacs-mcp-reply-" t))
         (socket (expand-file-name "mcp" root))
         (instance (gethash "instance" (neomacs-mcp-identity)))
         (neomacs-mcp-tools (copy-sequence neomacs-mcp-tools))
         (output (list ""))
         client)
    (neomacs-mcp-register-tool
     "raw" "Fixture" (neomacs-mcp--schema nil nil)
     (lambda (_) (string ?a (unibyte-char-to-multibyte 200) ?b)))
    (unwind-protect
        (progn
          (neomacs-mcp-start socket)
          (setq client (make-network-process
                        :name "neomacs-mcp-reply-client" :family 'local :service socket
                        :coding 'utf-8 :noquery t
                        :filter (lambda (_ chunk) (setcar output (concat (car output) chunk)))))
          (dolist (case (list (list 1 "neomacs_eval"
                                    (neomacs-mcp--object "instance" instance
                                                         "code" "(make-string 30000 27)"))
                              (list 2 "raw" (neomacs-mcp--object))))
            (let ((reply (neomacs-mcp-test--reply
                          client output (apply #'neomacs-mcp-test--call-line case))))
              (should reply)
              (should (equal (car case) (gethash "id" reply)))
              (should (= -32603 (gethash "code" (gethash "error" reply))))))
          (let ((reply (neomacs-mcp-test--reply
                        client output
                        (neomacs-mcp-test--call-line
                         3 "neomacs_eval"
                         (neomacs-mcp--object "instance" instance "code" "(* 6 7)")))))
            (should reply)
            (should (equal "42" (gethash "text" (aref (gethash "content" (gethash "result" reply))
                                                      0))))))
      (when (process-live-p client) (delete-process client))
      (neomacs-mcp-stop)
      (delete-directory root t))))

(defun neomacs-mcp-test--legacy-initialize (client output)
  "Complete a legacy handshake for CLIENT whose replies collect in OUTPUT."
  (should (neomacs-mcp-test--reply
           client output
           (concat (json-serialize
                    (neomacs-mcp--object
                     "jsonrpc" "2.0" "id" 1 "method" "initialize" "params"
                     (neomacs-mcp--object
                      "protocolVersion" "2025-06-18"
                      "capabilities" (neomacs-mcp--object)
                      "clientInfo" (neomacs-mcp--object))))
                   "\n")))
  (process-send-string
   client "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n"))

(ert-deftest neomacs-mcp-near-limit-id-reply-stays-bounded ()
  ;; A request just under the input limit whose ID alone is near that
  ;; limit must not produce an error reply over the output limit.  The ID
  ;; is not echoed; the connection stays usable.
  (let* ((root (make-temp-file "neomacs-mcp-id-" t))
         (socket (expand-file-name "mcp" root))
         (output (list ""))
         client)
    (unwind-protect
        (progn
          (neomacs-mcp-start socket)
          (setq client (make-network-process
                        :name "neomacs-mcp-id-client" :family 'local :service socket
                        :coding 'utf-8 :noquery t
                        :filter (lambda (_ chunk) (setcar output (concat (car output) chunk)))))
          (neomacs-mcp-test--legacy-initialize client output)
          (dolist (id (list (make-string 131000 ?x)
                            (json-parse-string (make-string 131000 ?9))))
            (let* ((line (concat (json-serialize
                                  (neomacs-mcp--object
                                   "jsonrpc" "2.0" "id" id "method" "tools/list"))
                                 "\n"))
                   (reply (progn
                            (should (<= (string-bytes line) neomacs-mcp--frame-limit))
                            (neomacs-mcp-test--reply client output line))))
              (should reply)
              (should (<= (string-bytes (car output)) neomacs-mcp--output-limit))
              (should (eq :null (gethash "id" reply)))
              (should (= -32600 (gethash "code" (gethash "error" reply))))))
          (let ((reply (neomacs-mcp-test--reply
                        client output
                        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n")))
            (should (equal 2 (gethash "id" reply)))
            (should (gethash "tools" (gethash "result" reply)))))
      (when (process-live-p client) (delete-process client))
      (neomacs-mcp-stop)
      (delete-directory root t))))

(ert-deftest neomacs-mcp-wire-never-exceeds-the-output-limit ()
  ;; Admission bounds IDs, but the encoder must hold the limit on its own:
  ;; an oversized or unencodable response with an oversized ID still
  ;; yields one bounded error line.
  (let ((id (make-string neomacs-mcp--output-limit ?x)))
    (dolist (result (list (make-string neomacs-mcp--output-limit ?y)
                          (string ?a (unibyte-char-to-multibyte 200))))
      (let* ((wire (neomacs-mcp--wire
                    (neomacs-mcp--object "jsonrpc" "2.0" "id" id "result" result)))
             (reply (json-parse-string wire)))
        (should (<= (string-bytes wire) neomacs-mcp--output-limit))
        (should (string-suffix-p "\n" wire))
        (should (eq :null (gethash "id" reply)))
        (should (= -32603 (gethash "code" (gethash "error" reply))))))
    (let ((wire (neomacs-mcp--wire
                 (neomacs-mcp--object "jsonrpc" "2.0" "id" 7
                                      "result" (make-string neomacs-mcp--output-limit ?y)))))
      (should (equal 7 (gethash "id" (json-parse-string wire)))))))

(defun neomacs-mcp-test--listed-names ()
  "Return the names of the tools a client can currently list."
  (mapcar (lambda (tool) (gethash "name" tool)) (neomacs-mcp--tool-list)))

(ert-deftest neomacs-mcp-full-access-default-and-off ()
  (should (eq t (default-value 'neomacs-mcp-full-access)))
  (should (custom-variable-p 'neomacs-mcp-full-access))
  (let ((neomacs-mcp-full-access nil)
        (instance (gethash "instance" (neomacs-mcp-identity))))
    (setq neomacs-mcp-test-reply-effect nil)
    (should-not (member "neomacs_eval" (neomacs-mcp-test--listed-names)))
    (should (member "neomacs_identity" (neomacs-mcp-test--listed-names)))
    (let ((failure
           (should-error
            (neomacs-mcp--call
             (neomacs-mcp--object
              "name" "neomacs_eval" "arguments"
              (neomacs-mcp--object "instance" instance
                                   "code" "(setq neomacs-mcp-test-reply-effect t)"))
             nil)
            :type 'neomacs-mcp-protocol-error)))
      (should (= -32602 (nth 1 failure))))
    (should-not neomacs-mcp-test-reply-effect))
  (let ((neomacs-mcp-full-access t))
    (should (member "neomacs_eval" (neomacs-mcp-test--listed-names)))))

(ert-deftest neomacs-mcp-full-access-off-keeps-companion-tools ()
  (let ((neomacs-mcp-tools (copy-sequence neomacs-mcp-tools))
        (neomacs-mcp-full-access nil))
    (neomacs-mcp-enable-companion-tools)
    (dolist (name '("neomacs_companion_claim" "neomacs_companion_read"
                    "neomacs_companion_edit" "neomacs_companion_receipt"
                    "neomacs_companion_undo" "neomacs_companion_retire"))
      (should (member name (neomacs-mcp-test--listed-names))))
    (should-not (member "neomacs_eval" (neomacs-mcp-test--listed-names)))))

(provide 'neomacs-mcp-reply-tests)
;;; neomacs-mcp-reply-tests.el ends here
