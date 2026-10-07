;;; neomacs-mcp-tests.el --- Native MCP regressions -*- lexical-binding: t; -*-
(require 'ert)
(require 'cl-lib)
(require 'neomacs-mcp)
(require 'neomacs-mcp-companion)

(ert-deftest neomacs-mcp-bundled-entrypoints ()
  (should (fboundp 'neomacs-mcp-start))
  (should (fboundp 'neomacs-mcp-stop))
  (should (fboundp 'neomacs-mcp-register-tool)))

(ert-deftest neomacs-mcp-owner-fence-before-effect ()
  (let ((neomacs-mcp-test-effect nil))
    (should-error
     (neomacs-mcp--eval
      (neomacs-mcp--object "instance" "wrong" "code"
                           "(setq neomacs-mcp-test-effect t)")))
    (should-not neomacs-mcp-test-effect)))

(ert-deftest neomacs-mcp-eval-all-forms-and-unicode ()
  (should (equal "\"Ελληνικά\\n42\""
                 (neomacs-mcp--eval
                  (neomacs-mcp--object
                   "instance" (gethash "instance" (neomacs-mcp-identity))
                   "code" "(setq neomacs-mcp-test-value 42) (format \"Ελληνικά\\n%s\" neomacs-mcp-test-value)")))))

(ert-deftest neomacs-mcp-registry-validation ()
  (should-error (neomacs-mcp-register-tool "" "Bad" nil #'ignore))
  (should-error (neomacs-mcp-register-tool "bad" "Bad" (neomacs-mcp--object "type" "object") nil)))

(ert-deftest neomacs-mcp-invalid-schema-arguments ()
  (let ((schema (neomacs-mcp--schema '(("code" . "string")) '("code"))))
    (should-error (neomacs-mcp--arguments schema (neomacs-mcp--object "code" 7)))
    (should-error (neomacs-mcp--arguments schema (neomacs-mcp--object)))
    (should-error (neomacs-mcp--arguments schema (neomacs-mcp--object "code" "ok" "extra" 1)))))

(ert-deftest neomacs-mcp-companion-preserves-study ()
  (neomacs-mcp-enable-companion-tools)
  (let* ((study (current-buffer))
         (point (point))
         (identity (gethash "instance" (neomacs-mcp-identity)))
         (claim (neomacs-mcp--companion-claim (neomacs-mcp--object "instance" identity)))
         (args (neomacs-mcp--object "instance" identity
                                    "handle" (gethash "handle" claim)
                                    "operationId" "ert-edit" "tick" (gethash "tick" claim)
                                    "start" 1 "end" 1 "text" "study\nΕλλάδα")))
    (unwind-protect
        (progn
          (should (equal "succeeded" (gethash "status" (neomacs-mcp--companion-edit args))))
          (should (equal "succeeded" (gethash "status" (neomacs-mcp--companion-edit args))))
          (should (eq study (current-buffer)))
          (should (= point (point))))
      (kill-buffer (gethash "buffer" claim)))))

(ert-deftest neomacs-mcp-nested-json-unicode ()
  (let ((neomacs-mcp-tools nil))
    (neomacs-mcp-register-tool
     "unicode" "Fixture" (neomacs-mcp--schema nil nil)
     (lambda (_) (neomacs-mcp--object "text" "Ελλάδα\n")))
    (let* ((result (neomacs-mcp--call (neomacs-mcp--object "name" "unicode") t))
           (wire (json-serialize result :false-object :false)))
      (should (equal "Ελλάδα\n"
                     (gethash "text" (json-parse-string
                                      (gethash "text" (aref (gethash "content" result) 0))))))
      (should (stringp wire)))))

(ert-deftest neomacs-mcp-stable-boot-and-owned-socket ()
  (let* ((root (make-temp-file "neomacs-mcp-ert-" t))
         (socket (expand-file-name "mcp" root))
         (boot (gethash "instance" (neomacs-mcp-identity))))
    (unwind-protect
        (progn
          (neomacs-mcp-start socket)
          (should (file-exists-p socket))
          (should-error (neomacs-mcp-start socket))
          (neomacs-mcp-stop)
          (should-not (file-exists-p socket))
          (neomacs-mcp-start socket)
          (should (equal boot (gethash "instance" (neomacs-mcp-identity))))
          (delete-file socket)
          (with-temp-file socket (insert "successor"))
          (neomacs-mcp-stop)
          (should (file-exists-p socket)))
      (neomacs-mcp-stop)
      (delete-directory root t))))

(ert-deftest neomacs-mcp-refuse-existing-node-and-symlink ()
  (let* ((root (make-temp-file "neomacs-mcp-node-" t))
         (socket (expand-file-name "mcp" root)))
    (unwind-protect
        (progn
          (with-temp-file socket (insert "not a socket"))
          (should-error (neomacs-mcp-start socket))
          (delete-file socket)
          (make-symbolic-link (expand-file-name "absent" root) socket)
          (should-error (neomacs-mcp-start socket)))
      (delete-directory root t))))

(ert-deftest neomacs-mcp-reclaim-stale-socket-only ()
  "A socket left by a dead listener is reclaimed; a live one is refused.
Like GNU `server-start', which deletes a leftover socket unless
`server-running-p' can connect to it."
  (let* ((root (make-temp-file "neomacs-mcp-stale-" t))
         (socket (expand-file-name "mcp" root))
         live)
    (unwind-protect
        (progn
          ;; A crashed editor leaves its socket node behind with no listener.
          (delete-process (make-network-process
                           :name "neomacs-mcp-ert-dead" :family 'local
                           :service socket :server t :noquery t))
          (should (file-exists-p socket))
          (neomacs-mcp-start socket)
          (should (process-live-p neomacs-mcp--listener))
          (should (equal (neomacs-mcp--socket-id socket) neomacs-mcp--socket-identity))
          (neomacs-mcp-stop)
          (should-not (file-exists-p socket))
          ;; Another live listener keeps its socket.
          (setq live (make-network-process
                      :name "neomacs-mcp-ert-live" :family 'local
                      :service socket :server t :noquery t))
          (let ((identity (neomacs-mcp--socket-id socket)))
            (should-error (neomacs-mcp-start socket))
            (should-not neomacs-mcp--listener)
            (should (process-live-p live))
            (should (equal identity (neomacs-mcp--socket-id socket)))))
      (when live (delete-process live))
      (neomacs-mcp-stop)
      (delete-directory root t))))

(ert-deftest neomacs-mcp-reclaim-refuses-unconfirmed-stale-socket ()
  "Only a refused connection proves a socket stale.
A live listener whose socket cannot be connected to (permission denied)
keeps its node."
  (let* ((root (make-temp-file "neomacs-mcp-stale-" t))
         (socket (expand-file-name "mcp" root))
         (live (make-network-process
                :name "neomacs-mcp-ert-live" :family 'local
                :service socket :server t :noquery t))
         (identity (neomacs-mcp--socket-id socket)))
    (unwind-protect
        (progn
          (set-file-modes socket 0)
          (should-error (neomacs-mcp-start socket))
          (should-not neomacs-mcp--listener)
          (should (process-live-p live))
          (should (equal identity (neomacs-mcp--socket-id socket))))
      (delete-process live)
      (neomacs-mcp-stop)
      (delete-directory root t))))

(ert-deftest neomacs-mcp-reclaim-reads-only-the-failure-reason ()
  "A socket path containing \"refused\" does not make a denial look stale."
  (let* ((root (make-temp-file "neomacs-mcp-stale-" t))
         (socket (expand-file-name "refused" root))
         (live (make-network-process
                :name "neomacs-mcp-ert-live" :family 'local
                :service socket :server t :noquery t))
         (identity (neomacs-mcp--socket-id socket)))
    (unwind-protect
        (progn
          (set-file-modes socket 0)
          (should-not (neomacs-mcp--stale-socket-id socket))
          (should-error (neomacs-mcp-start socket))
          (should (process-live-p live))
          (should (equal identity (neomacs-mcp--socket-id socket))))
      (delete-process live)
      (neomacs-mcp-stop)
      (delete-directory root t))))

(ert-deftest neomacs-mcp-reclaim-deletes-only-the-node-it-checked ()
  "A node replaced right after its type check is not deleted.
The replacement, a regular file, also refuses connections, so only
checking type, owner and deletion identity on one stat keeps it."
  (let* ((root (make-temp-file "neomacs-mcp-stale-" t))
         (socket (expand-file-name "mcp" root))
         (file-attributes-fn (symbol-function 'file-attributes))
         replaced)
    (unwind-protect
        (progn
          (delete-process (make-network-process
                           :name "neomacs-mcp-ert-dead" :family 'local
                           :service socket :server t :noquery t))
          (cl-letf (((symbol-function 'file-attributes)
                     (lambda (name &optional id-format)
                       (prog1 (funcall file-attributes-fn name id-format)
                         (when (and (equal name socket) (not replaced))
                           (setq replaced t)
                           (delete-file socket)
                           (with-temp-file socket (insert "successor")))))))
            (should-error (neomacs-mcp-start socket)))
          (should replaced)
          (should-not neomacs-mcp--listener)
          (should (equal "successor"
                         (with-temp-buffer
                           (insert-file-contents socket)
                           (buffer-string)))))
      (neomacs-mcp-stop)
      (delete-directory root t))))

(ert-deftest neomacs-mcp-reclaim-never-deletes-a-successor ()
  "A socket that another editor replaced after the probe is not deleted."
  (let* ((root (make-temp-file "neomacs-mcp-stale-" t))
         (socket (expand-file-name "mcp" root))
         successor identity)
    (cl-flet ((replace-after-probe (stale)
                ;; Another editor reclaims the same stale node first.
                (when stale
                  (delete-file socket)
                  (setq successor (make-network-process
                                   :name "neomacs-mcp-ert-successor"
                                   :family 'local :service socket
                                   :server t :noquery t)
                        identity (neomacs-mcp--socket-id socket)))
                stale))
      (unwind-protect
          (progn
            (delete-process (make-network-process
                             :name "neomacs-mcp-ert-dead" :family 'local
                             :service socket :server t :noquery t))
            (advice-add 'neomacs-mcp--stale-socket-id
                        :filter-return #'replace-after-probe)
            (should-error (neomacs-mcp-start socket))
            (should-not neomacs-mcp--listener)
            (should (process-live-p successor))
            (should (equal identity (neomacs-mcp--socket-id socket))))
        (advice-remove 'neomacs-mcp--stale-socket-id #'replace-after-probe)
        (when successor (delete-process successor))
        (neomacs-mcp-stop)
        (delete-directory root t)))))

(defun neomacs-mcp-test--pump (predicate)
  "Accept process output until PREDICATE returns non-nil."
  (with-timeout (10 (ert-fail "Condition not reached within 10 seconds"))
    (while (not (funcall predicate))
      (accept-process-output nil 0.05))))

(ert-deftest neomacs-mcp-connections-leave-no-buffers ()
  "Accepted, rejected and stopped connections leave no buffer behind.
Like GNU `server-start', the listener has a filter, so
`server_accept_connection' gives accepted connections none."
  (let* ((root (make-temp-file "neomacs-mcp-buffers-" t))
         (socket (expand-file-name "mcp" root))
         (line (concat (json-serialize
                        (neomacs-mcp--object
                         "jsonrpc" "2.0" "id" 1 "method" "initialize" "params"
                         (neomacs-mcp--object
                          "protocolVersion" "2025-06-18"
                          "capabilities" (neomacs-mcp--object)
                          "clientInfo" (neomacs-mcp--object))))
                       "\n"))
         (replies 0)
         (before (buffer-list))
         clients)
    (cl-flet ((connect ()
                (let ((client (make-network-process
                               :name "neomacs-mcp-ert-client" :family 'local
                               :service socket :coding 'utf-8 :noquery t
                               :filter (lambda (_ chunk)
                                         (cl-incf replies (cl-count ?\n chunk))))))
                  (push client clients)
                  (process-send-string client line)
                  client))
              (new-buffers ()
                ;; GNU creates internal buffers such as
                ;; " *code-conversion-work*" lazily; they are not per-connection.
                (cl-remove-if (lambda (buffer)
                                (or (memq buffer before)
                                    (string-prefix-p " " (buffer-name buffer))))
                              (buffer-list))))
      (unwind-protect
          (progn
            (neomacs-mcp-start socket)
            (dotimes (i 3)
              (let ((client (connect)))
                (neomacs-mcp-test--pump (lambda () (= replies (1+ i))))
                (should (= 1 (length neomacs-mcp--peers)))
                (delete-process client)
                (neomacs-mcp-test--pump (lambda () (null neomacs-mcp--peers)))
                (should-not (new-buffers))))
            ;; Fill the quota, then one more connection is rejected.  Wait
            ;; for each accept: a connect beyond the listen backlog blocks.
            (setq replies 0 clients nil)
            (dotimes (i (1+ neomacs-mcp--peer-limit))
              (connect)
              (neomacs-mcp-test--pump
               (lambda () (or (= (length neomacs-mcp--peers) (1+ i))
                              (cl-some (lambda (c) (not (process-live-p c))) clients)))))
            (neomacs-mcp-test--pump
             (lambda () (and (= replies neomacs-mcp--peer-limit)
                             (= 1 (cl-count-if-not #'process-live-p clients)))))
            (should (= neomacs-mcp--peer-limit (length neomacs-mcp--peers)))
            (should-not (cl-some #'process-buffer neomacs-mcp--peers))
            (should-not (new-buffers))
            (mapc #'delete-process clients)
            (neomacs-mcp-test--pump (lambda () (null neomacs-mcp--peers)))
            (should-not (new-buffers))
            ;; Stopping with connections still open leaves none either.
            (setq replies 0 clients nil)
            (dotimes (_ 2) (connect))
            (neomacs-mcp-test--pump (lambda () (= replies 2)))
            (neomacs-mcp-stop)
            (should-not neomacs-mcp--peers)
            (should-not (new-buffers)))
        (mapc #'delete-process clients)
        (neomacs-mcp-stop)
        (delete-directory root t)))))

(ert-deftest neomacs-mcp-filter-never-evaluates ()
  (let ((calls nil))
    (cl-letf (((symbol-function 'neomacs-mcp--enqueue)
               (lambda (_peer message &optional failure) (push (or failure message) calls)))
              ((symbol-function 'neomacs-mcp--dispatch)
               (lambda (&rest _) (ert-fail "Filter executed tool"))))
      (neomacs-mcp--frame nil (encode-coding-string
                              "{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"tools/list\"}" 'utf-8))
      (neomacs-mcp--frame nil "{bad}")
      (should (= 2 (length calls)))
      (should (= -32700 (car (car calls)))))))

(ert-deftest neomacs-mcp-tool-notification-does-not-mutate ()
  (let ((calls nil))
    (cl-letf (((symbol-function 'neomacs-mcp--enqueue)
               (lambda (&rest _) (setq calls t))))
      (neomacs-mcp--frame nil "{\"jsonrpc\":\"2.0\",\"method\":\"tools/call\",\"params\":{}}")
      (should-not calls))))

(ert-deftest neomacs-mcp-drain-fairness-and-nested-guard ()
  (let* ((peer 'fixture-peer)
         (neomacs-mcp--active nil)
         (neomacs-mcp--timer nil)
         (neomacs-mcp--queue
          (mapcar (lambda (id)
                    (list :peer peer :generation neomacs-mcp--generation
                          :cancelled nil :message (neomacs-mcp--object
                                                   "id" id "method" "fixture")))
                  '(1 2 3 4 5 6)))
         (calls 0) (scheduled 0))
    (cl-letf (((symbol-function 'neomacs-mcp--live-p) (lambda (&rest _) t))
              ((symbol-function 'input-pending-p) (lambda (&optional _) nil))
              ((symbol-function 'neomacs-mcp--send) #'ignore)
              ((symbol-function 'neomacs-mcp--schedule) (lambda () (cl-incf scheduled)))
              ((symbol-function 'neomacs-mcp--dispatch)
               (lambda (&rest _)
                 (cl-incf calls)
                 (neomacs-mcp--drain)
                 (neomacs-mcp--object))))
      (neomacs-mcp--drain)
      (should (= 1 calls))
      (should (= 5 (length neomacs-mcp--queue)))
      (should (= 1 scheduled))
      (should-not neomacs-mcp--active))))

(ert-deftest neomacs-mcp-peer-queue-quota ()
  (let ((neomacs-mcp--queue (make-list 16 (list :peer 'peer)))
        (closed nil))
    (cl-letf (((symbol-function 'neomacs-mcp--close) (lambda (peer) (setq closed peer))))
      (neomacs-mcp--enqueue 'peer (neomacs-mcp--object))
      (should (eq closed 'peer))
      (should (= 16 (length neomacs-mcp--queue))))))

(ert-deftest neomacs-mcp-malformed-code-not-silently-truncated ()
  (let ((neomacs-mcp-test-effect nil))
    (should-error
     (neomacs-mcp--eval
      (neomacs-mcp--object
       "instance" (gethash "instance" (neomacs-mcp-identity))
       "code" ") (setq neomacs-mcp-test-effect t)")))
    (should-not neomacs-mcp-test-effect)))

(ert-deftest neomacs-mcp-legacy-metadata-preserves-era ()
  (cl-letf (((symbol-function 'process-get) (lambda (_peer _key) 'ready)))
    (dolist (meta (list (neomacs-mcp--object)
                        (neomacs-mcp--object "progressToken" "ert-progress")
                        (neomacs-mcp--object "example.com/context" "fixture")))
      (should (= 0 (hash-table-count
                    (neomacs-mcp--dispatch
                     'peer (neomacs-mcp--object "method" "ping" "params"
                                               (neomacs-mcp--object "_meta" meta)))))))))

(ert-deftest neomacs-mcp-dual-era-explicit-modern-validation ()
  (cl-letf (((symbol-function 'process-get) (lambda (_peer _key) 'ready)))
    (dolist (key '("io.modelcontextprotocol/protocolVersion"
                   "io.modelcontextprotocol/clientCapabilities"
                   "io.modelcontextprotocol/clientInfo"))
      (should-error
       (neomacs-mcp--dispatch
        'peer (neomacs-mcp--object "method" "ping" "params"
                                  (neomacs-mcp--object "_meta"
                                                       (neomacs-mcp--object key :null))))
       :type 'neomacs-mcp-protocol-error))))

(ert-deftest neomacs-mcp-unsupported-version-exact-data ()
  (let* ((failure
          (should-error
           (neomacs-mcp--modern-p
            (neomacs-mcp--object
             "_meta" (neomacs-mcp--object
                      "io.modelcontextprotocol/protocolVersion" "1900-01-01"
                      "io.modelcontextprotocol/clientCapabilities" (neomacs-mcp--object))))
           :type 'neomacs-mcp-protocol-error))
         (data (nth 3 failure)))
    (should (= -32022 (nth 1 failure)))
    (should (equal "1900-01-01" (gethash "requested" data)))
    (should (equal ["2026-07-28" "2025-11-25" "2025-06-18"] (gethash "supported" data)))
    (should (= 2 (hash-table-count data)))))

(ert-deftest neomacs-mcp-modern-ping-complete ()
  (cl-letf (((symbol-function 'process-get) (lambda (_peer _key) 'ready)))
    (let ((result
           (neomacs-mcp--dispatch
            'peer (neomacs-mcp--object
                   "method" "ping" "params"
                   (neomacs-mcp--object
                    "_meta" (neomacs-mcp--object
                             "io.modelcontextprotocol/protocolVersion" neomacs-mcp--modern
                             "io.modelcontextprotocol/clientCapabilities" (neomacs-mcp--object)))))))
      (should (equal "complete" (gethash "resultType" result))))))

;; Keep the established public ERT entry point inclusive of optional reads.
(require 'neomacs-mcp-responsive-tests)
(require 'neomacs-mcp-negotiation-tests)
(require 'neomacs-mcp-reply-tests)

(provide 'neomacs-mcp-tests)
;;; neomacs-mcp-tests.el ends here
