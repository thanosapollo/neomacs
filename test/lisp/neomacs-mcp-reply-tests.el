;;; neomacs-mcp-reply-tests.el --- Error replies and full access -*- lexical-binding: t; -*-
(require 'ert)
(require 'cl-lib)
(require 'neomacs-mcp)

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
  ;; Control characters are escaped twice: 30000 of them print to about
  ;; 30 KB but encode to more than the 128 KiB response limit.  A raw
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

(provide 'neomacs-mcp-reply-tests)
;;; neomacs-mcp-reply-tests.el ends here
