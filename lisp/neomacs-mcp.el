;;; neomacs-mcp.el --- Native owner MCP endpoint -*- lexical-binding: t; -*-

;; Copyright (C) 2026 Free Software Foundation, Inc.
;; This file is part of GNU Emacs.
;; GNU Emacs is free software: you can redistribute it and/or modify
;; it under the terms of the GNU General Public License as published by
;; the Free Software Foundation, either version 3 of the License, or
;; (at your option) any later version.

;;; Commentary:
;; Explicitly start a dedicated private Unix endpoint, separate from server.el.
;; NDJSON is a custom local transport; the neomacs-mcp stdio relay copies bytes.
;; Owner eval is unrestricted, synchronous and never promises rollback or CPU
;; preemption.  Filters frame and enqueue; one guarded timer admits tool calls.
;; See doc/misc/neomacs-mcp.md for limits and the dual-era protocol contract.

;;; Code:
(require 'cl-lib)
(require 'json)
(require 'server)

(defgroup neomacs-mcp nil
  "Native owner-controlled MCP endpoint."
  :group 'external)

(defcustom neomacs-mcp-send-timeout 0.25
  "Seconds before closing an owned peer whose response send has not returned."
  :type 'number :group 'neomacs-mcp)

(defconst neomacs-mcp--frame-limit 131072)
(defconst neomacs-mcp--output-limit 131072)
(defconst neomacs-mcp--queue-limit 64)
(defconst neomacs-mcp--peer-queue-limit 16)
(defconst neomacs-mcp--peer-limit 8)
(defconst neomacs-mcp--scan-limit 8)
(defconst neomacs-mcp--modern "2026-07-28")
(defconst neomacs-mcp--legacy "2025-11-25")
(defvar neomacs-mcp--boot nil)
(defvar neomacs-mcp--generation 0)
(defvar neomacs-mcp--listener nil)
(defvar neomacs-mcp--socket nil)
(defvar neomacs-mcp--socket-identity nil)
(defvar neomacs-mcp--peers nil)
(defvar neomacs-mcp--queue nil)
(defvar neomacs-mcp--active nil)
(defvar neomacs-mcp--timer nil)
(defvar neomacs-mcp-tools nil
  "Plain alist of NAME and tool plist.
Each plist has :description, :schema, :handler and optional :annotations.
Use `neomacs-mcp-register-tool' to replace an entry.  Handlers receive a JSON
object hash table and return a JSON value.  Changes are visible on next list;
no listChanged capability or subscriptions are advertised.")

(define-error 'neomacs-mcp-protocol-error "MCP protocol error")

(defun neomacs-mcp--object (&rest pairs)
  "Return a JSON object populated by alternating key/value PAIRS."
  (let ((object (make-hash-table :test #'equal)))
    (while pairs (puthash (pop pairs) (pop pairs) object))
    object))

(defun neomacs-mcp--fail (code message &optional data)
  "Signal protocol CODE with MESSAGE and optional DATA."
  (signal 'neomacs-mcp-protocol-error (list code message data)))

(defun neomacs-mcp-identity ()
  "Return the stable process identity, independent of endpoint restarts."
  (unless neomacs-mcp--boot
    (setq neomacs-mcp--boot
          (format "%s:%s:%s" (emacs-pid) (float-time) (random))))
  (neomacs-mcp--object "instance" neomacs-mcp--boot "pid" (emacs-pid)
                       "runtime" emacs-version "serverName" server-name
                       "endpointGeneration" neomacs-mcp--generation))

(defun neomacs-mcp--instance (arguments)
  "Refuse unless ARGUMENTS explicitly name this process incarnation."
  (unless (equal (gethash "instance" arguments)
                 (gethash "instance" (neomacs-mcp-identity)))
    (user-error "Neomacs instance mismatch")))

(defun neomacs-mcp-register-tool (name description schema handler &optional annotations)
  "Register a named tool with a JSON schema and handler.
NAME identifies the tool; DESCRIPTION documents it; SCHEMA describes inputs.
HANDLER receives a JSON object and returns a JSON value.  Optional ANNOTATIONS
are descriptive hints, not permissions.  Replace the same name
 deterministically."
  (unless (and (stringp name) (string-match-p "\\`[A-Za-z0-9_.-]+\\'" name)
               (<= (length name) 128) (stringp description)
               (hash-table-p schema) (equal (gethash "type" schema) "object")
               (functionp handler))
    (error "Invalid MCP tool registration"))
  (setf (alist-get name neomacs-mcp-tools nil nil #'equal)
        (list :description description :schema schema :handler handler
              :annotations annotations)))

(defun neomacs-mcp--schema (properties required)
  "Return a closed object schema from PROPERTIES and REQUIRED field names."
  (neomacs-mcp--object
   "type" "object" "properties"
   (apply #'neomacs-mcp--object
          (cl-loop for (name . type) in properties
                   append (list name (neomacs-mcp--object "type" type))))
   "required" (vconcat required) "additionalProperties" :false))

(defun neomacs-mcp--arguments (schema arguments)
  "Validate ARGUMENTS against the supported object subset of SCHEMA.
Require fields, primitive types and closed properties; return ARGUMENTS.
Custom handlers own additional constraints, including nested JSON Schema."
  (unless (hash-table-p arguments) (neomacs-mcp--fail -32602 "Arguments must be an object"))
  (mapc (lambda (key)
          (unless (not (eq (gethash key arguments :absent) :absent))
            (neomacs-mcp--fail -32602 (concat "Missing argument: " key))))
        (gethash "required" schema))
  (maphash
   (lambda (key value)
     (let* ((property (gethash key (gethash "properties" schema)))
            (type (and property (gethash "type" property))))
       (when (and (not property) (eq (gethash "additionalProperties" schema) :false))
         (neomacs-mcp--fail -32602 (concat "Unknown argument: " key)))
       (unless (pcase type
                 ("string" (stringp value)) ("integer" (integerp value))
                 ("number" (numberp value)) ("object" (hash-table-p value))
                 ("array" (vectorp value))
                 ("boolean" (memq value '(t :false))) (_ t))
         (neomacs-mcp--fail -32602 (concat "Wrong argument type: " key)))))
   arguments)
  arguments)

(defun neomacs-mcp--eval (arguments)
  "Evaluate all Lisp forms in ARGUMENTS after the instance fence.
Return a bounded printed value; effects are not rolled back on failure."
  (neomacs-mcp--instance arguments)
  (let* ((code (gethash "code" arguments))
         (wrapped (concat "(progn\n" code "\n)"))
         (parsed (read-from-string wrapped))
         (form (car parsed))
         (_complete
          (unless (= (cdr parsed) (length wrapped))
            (error "Malformed Lisp input; trailing forms outside wrapper")))
         (value (eval form t))
         (print-length 64) (print-level 16) (print-circle t)
         (print-escape-newlines t))
    (let ((text (prin1-to-string value)))
      (if (> (string-bytes text) 65536)
          (error "Eval result exceeds output limit; effects may have occurred")
        text))))

(defun neomacs-mcp--tool-list ()
  "Return deterministic descriptors for the registered tools."
  (vconcat
   (mapcar (lambda (entry)
             (let* ((tool (cdr entry))
                    (object (neomacs-mcp--object
                             "name" (car entry) "description" (plist-get tool :description)
                             "inputSchema" (plist-get tool :schema))))
               (when (plist-get tool :annotations)
                 (puthash "annotations" (plist-get tool :annotations) object))
               object))
           (sort (copy-sequence neomacs-mcp-tools)
                 (lambda (a b) (string-lessp (car a) (car b)))))))

(defun neomacs-mcp--call (params modern)
  "Call the tool named by PARAMS and return the MODERN or legacy envelope."
  (let* ((tool (alist-get (gethash "name" params) neomacs-mcp-tools nil nil #'equal))
         (arguments (gethash "arguments" params (neomacs-mcp--object))))
    (unless tool (neomacs-mcp--fail -32602 "Unknown tool"))
    (neomacs-mcp--arguments (plist-get tool :schema) arguments)
    (let* ((failed nil)
           (value (condition-case failure
                      (with-local-quit (funcall (plist-get tool :handler) arguments))
                    ((error quit)
                     (setq failed t)
                     (error-message-string failure))))
           (text (if (stringp value) value
                   (decode-coding-string
                    (json-serialize value :false-object :false :null-object :null)
                    'utf-8)))
           (result (neomacs-mcp--object
                    "content" (vector (neomacs-mcp--object "type" "text" "text" text))
                    "isError" (if failed t :false))))
      (when modern (puthash "resultType" "complete" result))
      result)))

(defun neomacs-mcp--modern-p (params)
  "Validate per-request modern metadata in PARAMS and return non-nil."
  (let* ((meta (gethash "_meta" params))
         (version (and (hash-table-p meta) (gethash "io.modelcontextprotocol/protocolVersion" meta))))
    (unless (and (stringp version)
                 (hash-table-p (gethash "io.modelcontextprotocol/clientCapabilities" meta)))
      (neomacs-mcp--fail -32602 "Required protocol metadata is missing"))
    (unless (equal version neomacs-mcp--modern)
      (neomacs-mcp--fail -32022 "Unsupported protocol version"
                         (neomacs-mcp--object
                          "supported" (vector neomacs-mcp--modern neomacs-mcp--legacy)
                          "requested" version)))
    t))

(defun neomacs-mcp--dispatch (peer message)
  "Dispatch validated MESSAGE from PEER outside process filters."
  (let* ((method (gethash "method" message))
         (params (gethash "params" message (neomacs-mcp--object)))
         (meta (gethash "_meta" params))
         ;; Legacy progress and extension metadata do not select a new era.
         (modern (and (not (equal method "initialize"))
                      (or (and (hash-table-p meta)
                               (cl-some
                                (lambda (key) (not (eq (gethash key meta :absent) :absent)))
                                '("io.modelcontextprotocol/protocolVersion"
                                  "io.modelcontextprotocol/clientCapabilities"
                                  "io.modelcontextprotocol/clientInfo")))
                          (not (process-get peer 'legacy)))
                      (neomacs-mcp--modern-p params))))
    (pcase method
      ("initialize"
       (unless (and (equal (gethash "protocolVersion" params) neomacs-mcp--legacy)
                    (hash-table-p (gethash "capabilities" params))
                    (hash-table-p (gethash "clientInfo" params))
                    (not (process-get peer 'legacy)))
         (neomacs-mcp--fail -32602 "Expected fresh 2025-11-25 initialization"))
       (process-put peer 'legacy 'initializing)
       (neomacs-mcp--object "protocolVersion" neomacs-mcp--legacy
                            "capabilities" (neomacs-mcp--object "tools" (neomacs-mcp--object))
                            "serverInfo" (neomacs-mcp--object "name" "Neomacs" "version" "1")))
      ("notifications/initialized"
       (unless (eq (process-get peer 'legacy) 'initializing)
         (neomacs-mcp--fail -32600 "Unexpected initialized notification"))
       (process-put peer 'legacy 'ready) nil)
      (_
       (unless (or modern (eq (process-get peer 'legacy) 'ready))
         (neomacs-mcp--fail -32600 "Legacy initialization is incomplete"))
       (pcase method
         ("ping" (if modern (neomacs-mcp--object "resultType" "complete")
                   (neomacs-mcp--object)))
         ("server/discover"
          (unless modern (neomacs-mcp--fail -32601 "Method not found"))
          (neomacs-mcp--object
           "resultType" "complete" "supportedVersions" (vector neomacs-mcp--modern neomacs-mcp--legacy)
           "capabilities" (neomacs-mcp--object "tools" (neomacs-mcp--object))
           "_meta" (neomacs-mcp--object "io.modelcontextprotocol/serverInfo"
                                       (neomacs-mcp--object "name" "Neomacs" "version" "1"))
           "ttlMs" 0 "cacheScope" "private"))
         ("tools/list"
          (when (gethash "cursor" params) (neomacs-mcp--fail -32602 "No pagination cursor is supported"))
          (let ((result (neomacs-mcp--object "tools" (neomacs-mcp--tool-list))))
            (when modern
              (puthash "resultType" "complete" result)
              (puthash "ttlMs" 0 result) (puthash "cacheScope" "private" result))
            result))
         ("tools/call" (neomacs-mcp--call params modern))
         (_ (neomacs-mcp--fail -32601 "Method not found")))))))

(defun neomacs-mcp--live-p (peer generation)
  "Return non-nil if PEER still belongs to endpoint GENERATION."
  (and (= generation neomacs-mcp--generation)
       (memq peer neomacs-mcp--peers) (process-live-p peer)))

(defun neomacs-mcp--close (peer)
  "Retire only PEER, its queue entries and owned send timer."
  (setq neomacs-mcp--peers (delq peer neomacs-mcp--peers)
        neomacs-mcp--queue
        (cl-remove peer neomacs-mcp--queue :key (lambda (request) (plist-get request :peer))))
  (when-let* ((timer (process-get peer 'send-timer)))
    (cancel-timer timer) (process-put peer 'send-timer nil))
  (when (and neomacs-mcp--active (eq peer (plist-get neomacs-mcp--active :peer)))
    (setf (plist-get neomacs-mcp--active :cancelled) t))
  (when (process-live-p peer) (delete-process peer)))

(defun neomacs-mcp--sentinel (peer _event)
  "Retire PEER when its native transport closes; ignore EVENT."
  (unless (process-live-p peer) (neomacs-mcp--close peer)))

(defun neomacs-mcp--send (request response)
  "Bound serialization and sending RESPONSE for the exact REQUEST lifetime.
Normal send return is not delivery acknowledgement.  Close an unread peer."
  (let* ((peer (plist-get request :peer))
         (generation (plist-get request :generation))
         (wire (concat (json-serialize response :false-object :false :null-object :null) "\n")))
    (when (and (not (plist-get request :cancelled)) (neomacs-mcp--live-p peer generation))
      (if (> (string-bytes wire) neomacs-mcp--output-limit)
          (neomacs-mcp--close peer)
        (let ((timer (run-at-time
                      neomacs-mcp-send-timeout nil
                      (lambda ()
                        (when (neomacs-mcp--live-p peer generation)
                          (neomacs-mcp--close peer))))))
          (process-put peer 'send-timer timer)
          (unwind-protect
              (condition-case nil
                  (progn (process-send-string peer wire)
                         (neomacs-mcp--live-p peer generation))
                (error (neomacs-mcp--close peer)))
            (cancel-timer timer)
            (when (eq timer (process-get peer 'send-timer))
              (process-put peer 'send-timer nil))))))))

(defun neomacs-mcp--schedule ()
  "Schedule a bounded drain unless another drain owns admission."
  (when (and neomacs-mcp--queue (not neomacs-mcp--active) (not neomacs-mcp--timer))
    (setq neomacs-mcp--timer (run-at-time 0.01 nil #'neomacs-mcp--drain))))

(defun neomacs-mcp--drain ()
  "Admit at most one request when human input is not pending.
Retire at most `neomacs-mcp--scan-limit' cancelled or stale queue entries.
This is cooperative admission, not a handler, serialization or send deadline."
  (setq neomacs-mcp--timer nil)
  (unless neomacs-mcp--active
    ;; Guard admission across native input polling too.  The no-timers query
    ;; services native special input, but does not request timer execution.
    (setq neomacs-mcp--active (list :admission t))
    (unwind-protect
        (unless (input-pending-p nil)
          (let ((scanned 0))
            (while (and neomacs-mcp--queue (< scanned neomacs-mcp--scan-limit)
                        (let ((request (car neomacs-mcp--queue)))
                          (or (plist-get request :cancelled)
                              (not (neomacs-mcp--live-p
                                    (plist-get request :peer)
                                    (plist-get request :generation))))))
              (pop neomacs-mcp--queue)
              (cl-incf scanned))
            (when (and neomacs-mcp--queue (< scanned neomacs-mcp--scan-limit))
              (let* ((request (pop neomacs-mcp--queue))
                     (peer (plist-get request :peer))
                     (message (plist-get request :message))
                     (id (and message (gethash "id" message :notification)))
                     (response nil))
                (setq neomacs-mcp--active request)
                (condition-case failure
                    (if (plist-get request :error)
                        (signal 'neomacs-mcp-protocol-error (plist-get request :error))
                      (setq response (neomacs-mcp--object
                                      "jsonrpc" "2.0" "id" id "result"
                                      (neomacs-mcp--dispatch peer message))))
                  (neomacs-mcp-protocol-error
                   (setq response
                         (neomacs-mcp--object
                          "jsonrpc" "2.0" "id" (if (eq id :notification) :null (or id :null))
                          "error" (neomacs-mcp--object "code" (nth 1 failure) "message" (nth 2 failure))))
                   (when (nth 3 failure) (puthash "data" (nth 3 failure) (gethash "error" response))))
                  ((error quit)
                   (setq response (neomacs-mcp--object
                                   "jsonrpc" "2.0" "id" (or id :null) "error"
                                   (neomacs-mcp--object "code" -32603 "message" "Internal error")))))
                (unless (eq id :notification) (neomacs-mcp--send request response))))))
      (setq neomacs-mcp--active nil)
      (neomacs-mcp--schedule))))

(defun neomacs-mcp--enqueue (peer message &optional failure)
  "Enqueue validated MESSAGE or framing FAILURE for PEER with bounded quotas."
  (if (or (>= (length neomacs-mcp--queue) neomacs-mcp--queue-limit)
          (>= (cl-count peer neomacs-mcp--queue :key (lambda (r) (plist-get r :peer)))
              neomacs-mcp--peer-queue-limit))
      (neomacs-mcp--close peer)
    (setq neomacs-mcp--queue
          (nconc neomacs-mcp--queue
                 (list (list :peer peer :generation neomacs-mcp--generation
                             :message message :error failure :cancelled nil))))
    (neomacs-mcp--schedule)))

(defun neomacs-mcp--cancel (peer id)
  "Mark pending or yielding active requests matching PEER and ID abandoned."
  (dolist (request (cons neomacs-mcp--active neomacs-mcp--queue))
    (when (and request (eq peer (plist-get request :peer))
               (hash-table-p (plist-get request :message))
               (equal id (gethash "id" (plist-get request :message) :absent)))
      (setf (plist-get request :cancelled) t))))

(defun neomacs-mcp--frame (peer line)
  "Validate LINE from PEER, then enqueue or mark cancellation without eval."
  (condition-case nil
      (let* ((decoded (decode-coding-string line 'utf-8))
             (_valid-utf8
              (unless (equal line (encode-coding-string decoded 'utf-8))
                (error "Invalid UTF-8")))
             (message (json-parse-string decoded :object-type 'hash-table :array-type 'array
                                        :null-object :null :false-object :false))
             (id (and (hash-table-p message) (gethash "id" message :absent)))
             (method (and (hash-table-p message) (gethash "method" message)))
             (params (and (hash-table-p message) (gethash "params" message (neomacs-mcp--object)))))
        (cond
         ((not (and (hash-table-p message) (equal (gethash "jsonrpc" message) "2.0")
                    (stringp method) (hash-table-p params)
                    (or (eq id :absent) (stringp id) (integerp id))))
          (neomacs-mcp--enqueue peer nil '(-32600 "Invalid request")))
         ((and (eq id :absent) (equal method "notifications/cancelled"))
          (neomacs-mcp--cancel peer (gethash "requestId" params :absent)))
         ((and (eq id :absent) (not (equal method "notifications/initialized")))
          nil)
         ((and (not (eq id :absent)) (string-prefix-p "notifications/" method))
          (neomacs-mcp--enqueue peer message '(-32600 "Notification must not have an ID")))
         ((and (not (eq id :absent))
               (cl-some (lambda (r)
                          (and r (eq peer (plist-get r :peer))
                               (hash-table-p (plist-get r :message))
                               (equal id (gethash "id" (plist-get r :message) :absent))))
                        (cons neomacs-mcp--active neomacs-mcp--queue)))
          (neomacs-mcp--close peer))
         (t (neomacs-mcp--enqueue peer message))))
    (error (neomacs-mcp--enqueue peer nil '(-32700 "Parse error")))))

(defun neomacs-mcp--filter (peer chunk)
  "Frame bounded UTF-8 CHUNK from PEER; never invoke a tool in this callback."
  (when (neomacs-mcp--live-p peer (process-get peer 'generation))
    (let ((text (concat (process-get peer 'input) chunk)) (frames 0))
      (if (> (string-bytes text) neomacs-mcp--frame-limit)
          (neomacs-mcp--close peer)
        (while (and (process-live-p peer) (string-match "\n" text)
                    (< frames neomacs-mcp--peer-queue-limit))
          (let ((end (match-beginning 0)))
            (neomacs-mcp--frame peer (substring text 0 end))
            (setq text (substring text (1+ end)) frames (1+ frames))))
        (if (string-match-p "\n" text) (neomacs-mcp--close peer)
          (process-put peer 'input text))))))

(defun neomacs-mcp--accept (listener peer _message)
  "Admit PEER from LISTENER, subject to the finite connection quota."
  (if (or (not (eq listener neomacs-mcp--listener))
          (>= (length neomacs-mcp--peers) neomacs-mcp--peer-limit))
      (delete-process peer)
    (push peer neomacs-mcp--peers)
    (process-put peer 'generation neomacs-mcp--generation)
    (process-put peer 'input (encode-coding-string "" 'no-conversion))
    (set-process-query-on-exit-flag peer nil)
    (set-process-coding-system peer 'no-conversion 'no-conversion)
    (set-process-filter peer #'neomacs-mcp--filter)
    (set-process-sentinel peer #'neomacs-mcp--sentinel)))

(defun neomacs-mcp--socket-id (socket)
  "Return the filesystem identity of SOCKET, excluding mutable timestamps."
  (when-let* ((attributes (file-attributes socket 'integer)))
    (list (file-attribute-inode-number attributes)
          (file-attribute-device-number attributes))))

;;;###autoload
(defun neomacs-mcp-start (socket)
  "Start an owner-controlled MCP listener at explicit absolute SOCKET.
Require a private owner directory, refuse any existing node and do not change
GNU server parsing.  Loading the library alone never starts a listener."
  (interactive "FPrivate MCP socket: ")
  (unless (and (stringp socket) (file-name-absolute-p socket)
               (not (file-remote-p socket)))
    (user-error "MCP requires an absolute local socket"))
  (when neomacs-mcp--listener (user-error "MCP is already started"))
  (server-ensure-safe-dir (file-name-directory socket))
  (when (or (file-exists-p socket) (file-symlink-p socket))
    (user-error "MCP socket already exists"))
  (cl-incf neomacs-mcp--generation)
  (let ((listener (make-network-process
                   :name "neomacs-mcp" :family 'local :service socket
                   :server t :noquery t :coding 'no-conversion :log #'neomacs-mcp--accept)))
    (setq neomacs-mcp--listener listener neomacs-mcp--socket socket
          neomacs-mcp--socket-identity (neomacs-mcp--socket-id socket))
    (add-hook 'kill-emacs-hook #'neomacs-mcp-stop)
    (neomacs-mcp-identity)))

;;;###autoload
(defun neomacs-mcp-stop ()
  "Retire admission and close only this endpoint's connections and socket.
Keep boot identity and companion receipts until the editor process exits."
  (interactive)
  (cl-incf neomacs-mcp--generation)
  (when neomacs-mcp--timer (cancel-timer neomacs-mcp--timer))
  (setq neomacs-mcp--timer nil neomacs-mcp--queue nil)
  (mapc #'neomacs-mcp--close (copy-sequence neomacs-mcp--peers))
  (when (process-live-p neomacs-mcp--listener) (delete-process neomacs-mcp--listener))
  (when (and neomacs-mcp--socket
             (equal (neomacs-mcp--socket-id neomacs-mcp--socket) neomacs-mcp--socket-identity)
             (file-exists-p neomacs-mcp--socket))
    (delete-file neomacs-mcp--socket))
  (setq neomacs-mcp--listener nil neomacs-mcp--socket nil neomacs-mcp--socket-identity nil)
  (remove-hook 'kill-emacs-hook #'neomacs-mcp-stop))

(neomacs-mcp-register-tool
 "neomacs_identity" "Read the stable editor incarnation before fenced calls."
 (neomacs-mcp--schema nil nil) (lambda (_) (neomacs-mcp-identity))
 (neomacs-mcp--object "readOnlyHint" t))
(neomacs-mcp-register-tool
 "neomacs_eval" "Evaluate unrestricted owner Lisp. No rollback or CPU preemption."
 (neomacs-mcp--schema '(("instance" . "string") ("code" . "string")) '("instance" "code"))
 #'neomacs-mcp--eval)

(provide 'neomacs-mcp)
;;; neomacs-mcp.el ends here
