# Native Neomacs MCP

Neomacs exposes owner-controlled tools through a bundled Lisp library. Start a
**separate, explicitly named Unix MCP socket** in the editor you intend to use;
the normal GNU-compatible editor server is unchanged. Loading the library does
not start a listener, select a buffer, or connect to an agent service.

```elisp
(require 'neomacs-mcp)
(neomacs-mcp-start "/absolute/private-directory/mcp")
;; Optional cooperative scratch-buffer tools:
(require 'neomacs-mcp-companion)
(neomacs-mcp-enable-companion-tools)
;; Optional bounded native observation (no listener or buffer creation):
(require 'neomacs-mcp-editor)
(neomacs-mcp-enable-editor-tools)
;; Later:
(neomacs-mcp-stop)
```

The containing directory must belong to the editor's owner and be private.
Existing nodes, including dangling symlinks, are refused rather than replaced.
Use a short path because Unix sockets have an operating-system path limit.

A standard stdio MCP client launches the bundled byte relay with:

```sh
neomacs-mcp --socket /absolute/private-directory/mcp
```

The relay requires an explicit socket. It does not start an editor, choose a
default editor, interpret MCP, retry calls, or evaluate Lisp. Direct local Unix
clients use a custom transport: UTF-8 JSON-RPC 2.0, one message per newline.
The MCP library itself has no Python or agent-specific dependency.

## Tools and identity

`neomacs_identity` returns `instance`, `pid`, `runtime`, `serverName`, and
`endpointGeneration`. Obtain this before invoking editor tools. `instance` is
stable for this loaded library's editor process lifetime, including endpoint
stop/start and relay reconnects. A fresh process has a different incarnation.
This is an accidental-target fence, not an authentication secret.

`neomacs_eval` requires `instance` and `code`. It evaluates all forms as a Lisp
`progn` with lexical evaluation and returns a printed Lisp value. It is full
trusted-owner evaluation, without a sandbox or a new approval prompt. Printing
limits list depth/length; a printed value exceeding 65536 bytes fails **after**
evaluation. Arbitrary code can change the editor, load other libraries, perform
I/O, enter a minibuffer, or exit the process. Such effects are not rolled back.
Use this tool deliberately; it does not promise to preserve the study window.

The optional tools operate only on uniquely claimed, undisplayed non-file
companion buffers. They do not adopt a namesake, visit learner files, select a
window, or edit arbitrary buffers:

| Tool | Required arguments beyond `instance` | Result |
| --- | --- | --- |
| `neomacs_companion_claim` | none | `handle`, `buffer`, `tick` |
| `neomacs_companion_read` | `handle` | Live metadata and property-free `text` |
| `neomacs_companion_edit` | `operationId`, `handle`, `tick`, `start`, `end`, `text` | Historical operation receipt |
| `neomacs_companion_receipt` | `operationId` | Historical receipt, even after undo/retirement |
| `neomacs_companion_undo` | `handle`, `tick` | Live metadata after one ordinary undo group |
| `neomacs_companion_retire` | `handle` | Retire ownership without erasing/killing the buffer |

Positions are widened, 1-based **character** positions. Literal edits require
the exact `buffer-modified-tick`, writable state and enabled undo. A stale tick
or retired claim refuses without trying again. A mode change, file association,
or buffer kill irreversibly retires the existing helper's claim. Edits preserve
point/narrowing and suppress modification callbacks only for this dedicated
scratch-buffer edit. Native read-only checks and ordinary undo remain enabled.

Receipt results contain `operation-id` and `status`; success adds `result`
metadata. Failed operations retain a condition symbol, not payload/error text.
The ledger admits **256 operation IDs per editor process**, never evicts them,
and refuses new IDs at capacity. Payload text is at most 65536 characters;
readback text has the same character limit. Receipts and payloads stay in process
memory. There is **no exactly-once guarantee across editor restart**, no automatic
replay, and no durable receipt for arbitrary eval, claim, undo or retirement.
Repeated identical edit payloads return the historical receipt without editing;
conflicting ID reuse refuses. Ordinary undo does not erase or rewrite a receipt.

A timeout, disconnect or successful send return is not proof of delivery or of
no effect. After an uncertain edit, reconnect to the same `instance` and query
`neomacs_companion_receipt` before deciding whether a retry is appropriate.
`absent` is distinguishable from `running`, `succeeded`, `failed` and
`indeterminate`. Indeterminate operations must not be blindly replayed.

## Optional native observation

`neomacs-mcp-enable-editor-tools` registers two read-only tools. Registration
creates no buffers and starts no listener. Full owner eval and companion claims
remain separate; these reads confer **no mutation authority**.

- `neomacs_buffer_list(instance, offset, limit)` accepts nonnegative native-slot
  `offset` and `limit` from 1 through 32. It examines at most 128 native
  `buffer-list` slots, returning at most `limit` entries. Names are exact labels
  of at most 256 characters, never handles. Allowlisted metadata is `name`,
  `tick`, `sizeChars`, `point`, `mode` (at most 128 characters, with
  `modeTruncated`), `modified` and `readOnly`; no file paths or text are returned.
  The result includes `instance`, `offset`, `scanned`, `buffers`, `truncated`,
  `nextOffset` (null at exhaustion) and `encodedOutputLimit`. Excluded entries
  still consume slots. An empty page can therefore have a next offset.
- `neomacs_buffer_read(instance, name, start, maxChars, expectedTick?)` resolves
  the **current** native namesake on admission. It reads widened 1-based,
  end-exclusive character coordinates, at most 4096 literal property-free
  characters. `start` must be in `[1, point-max]`; EOB returns empty text without
  truncation. A supplied nonnegative `expectedTick` must equal the current native
  modification tick or the read refuses. The result includes `instance`, `name`,
  `tick`, actual `start`/`end`, `text`, `truncated`, `nextStart` (null at EOB) and
  `encodedOutputLimit`. Point, restriction, current buffer and selected window
  are preserved. No mode hooks, total-line scans, fontification, project
  discovery, file opening or subprocesses are needed.

Both tools cap the **encoded tool-result envelope at 32768 bytes**, accounting
for UTF-8 and nested JSON escaping, not just the inner text. Core JSON-RPC ID
and wire limits remain separate. Discovery stops before a non-fitting entry;
reads halve a bounded character range until it fits (not necessarily a maximal
prefix). Metadata that cannot fit, or a non-EOB read that cannot fit one
character, refuses. Each attempted read extraction is already bounded to 4096
characters before serialization; the encoded cap is not a peak-allocation cap.

Minibuffers (including indirect aliases), credential-like authinfo/netrc and
password-store names/visited paths/already-known truenames, and eval-approval
modes are excluded before metadata/text access. No filesystem resolution or
content inspection is performed. This lexical disclosure policy cannot detect
renamed secret copies or unknown credential locations; explicitly named reads
are observation, not a security sandbox. Trusted owner eval is unaffected.
Space-prefixed internal buffers and overlong names are omitted from discovery.

Native enumeration may allocate all buffer references, and offset traversal is
O(offset). Pages are **not atomic snapshots or O(page-size) total work**: native
creation/kill/rename/reordering can invalidate offsets between calls. Refresh
rather than treating concatenated pages as a stable catalogue. A name plus tick
is not an enduring identity: rename or same-name recreation resolves the current
buffer, not an old object. Use the accepted companion claim and receipt API for
structured mutation.

## Protocol versions

The endpoint implements these two distinct eras:

- **2026-07-28:** no initialization handshake. Every request requires
  `params._meta["io.modelcontextprotocol/protocolVersion"]` and an object at
  `params._meta["io.modelcontextprotocol/clientCapabilities"]`. Optional client
  identity is informational. `server/discover` exposes versions, tools capability,
  server identity, `resultType: "complete"`, `ttlMs: 0`, and private cache scope.
  Modern tool lists carry those cache fields; calls and ping carry `resultType`.
  Unsupported per-request versions receive `-32022` with `data.supported`
  listing all three implemented versions and `data.requested` echoing the request.
- **2025-11-25 and 2025-06-18 compatibility:** a fresh connection sends
  `initialize` with a nonempty string `protocolVersion`, object client information
  and object capabilities, then `notifications/initialized`. Supported offers
  are echoed exactly, including the 2025-06-18 offer used by Codex 0.155.1.
  Other string offers receive a 2025-11-25 counterproposal, the newest supported
  handshake version; clients that cannot use it should disconnect, as specified
  by [MCP version negotiation](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle#version-negotiation).
  A malformed or repeated initialization is rejected without changing readiness.
  Tool requests require the initialized notification and use the common legacy
  envelopes, without `resultType` or modern required metadata. An `initialize`
  offer of 2026-07-28 receives the same legacy counterproposal; modern semantics
  remain exclusively per-request and do not use a handshake.

Legacy `_meta.progressToken` and unrelated extension metadata do not switch eras;
progress notifications are optional and are not emitted. Reserved modern
`protocolVersion`, `clientCapabilities`, or `clientInfo` keys under the
`io.modelcontextprotocol/` prefix select modern validation even on an initialized
legacy connection. Legacy ping retains an empty result object.

Only `ping`, discovery and tools are implemented. Resources, prompts, tasks,
subscriptions and list-change notifications are not advertised. Tool lists are
sorted and cannot vary because of the connection. Schema/unknown-tool failures
are protocol errors; tool execution errors have `isError: true`. Notifications
never invoke tools. Request IDs must be strings or integers and cannot be reused
while outstanding on the same connection; duplicates close that connection.

## Scheduling and limits

Filters only frame, validate, enqueue and mark cancellation. A guarded regular
timer queries `(input-pending-p nil)` before admission, without requesting timer
execution. Pending human input defers all admission without consuming a live
request. Otherwise at most eight cancelled/stale entries are retired and **at
most one** FIFO request is dispatched per callback. Remaining work gets another
regular 10ms timer after unwind; admission does not require complete idleness.
Continuous pending input can intentionally defer agent work indefinitely: no
hard fairness or starvation bound is promised.

The active guard covers both native input polling and dispatch. Native waits may
service more filters/timers, but cannot admit a successor tool inside an active
tool. This is cooperative editor event-loop scheduling, **not** thread
concurrency, timer fairness proof or CPU preemption. Arbitrary eval/handlers,
serialization and native sends remain synchronous; this slice cannot guarantee
whole-editor freeze immunity. A non-yielding Lisp loop prevents incoming
cancellation from being observed until it yields or returns.

There are at most eight peers, 64 queued requests globally, and 16 queued
requests/frames per peer/filter turn. Input accumulation and individual output
are limited to 131072 bytes. Saturation closes the offending connection;
responses are rejected after serialization, not by a memory-isolated evaluator.
A peer with an incomplete frame remains subject to the finite peer/input caps;
there is no inactivity timer. Close unused clients rather than treating a long
idle connection as an editor session lease.

During a send, an owned timer closes that exact peer after
`neomacs-mcp-send-timeout` seconds (default 0.25). The native send may return
normally after the peer was closed; return is not an acknowledgement. Stop/EOF
retire the connection's pending work and suppress abandoned replies. Endpoint
stop first changes the generation, cancels owned timers/queued work, then closes
owned peers/listener and removes only its still-owned socket node. Stopping an
endpoint does not terminate the editor or erase its receipts. Active effects
are never rolled back merely because cancellation, stop or EOF was observed.

## Extending the registry

`neomacs-mcp-tools` is a plain alist. Register or replace an entry with
`neomacs-mcp-register-tool NAME DESCRIPTION SCHEMA HANDLER &optional ANNOTATIONS`.
The handler receives a JSON object hash table and returns a JSON value (a string
is used directly as text content). Hints are descriptive, not permissions.
Handlers run synchronously at the guarded admission boundary, with no promise
about a selected buffer. Explicitly choose buffers and fence editor operations
with the expected identity. Do not create a second companion ownership ledger.

The built-in validator covers required fields, primitive types, and closed
object properties used by the supplied tools. Custom handlers own deeper JSON
Schema constraints. Tool descriptors must remain valid JSON data. Changes are
visible on the next list request; zero TTL avoids advertising stale cached lists.

## Focused verification

No full Rust/product build or personal editor activation is needed to exercise
the Lisp slice. Use a clean source load (no stale `.elc` files), a disposable
HOME/XDG/TMPDIR, and an explicitly selected native executable:

```sh
"$NEOMACS" -Q --batch -L lisp -L test/lisp \
  -l neomacs-mcp-tests -l neomacs-companion-tests \
  -l neomacs-companion-receipts-tests -f ert-run-tests-batch-and-exit
NEOMACS="$NEOMACS" python3 test/lisp/neomacs-mcp-protocol.py --output /owned/evidence
```

The protocol harness starts/reaps its own headless foreground daemon and removes
its fixture root. It exercises **raw current-protocol wire behavior**, not
exhaustive specification conformance. For actual legacy SDK discovery/calls:

```sh
# Run with a separately provisioned Python environment containing mcp==1.30.0.
NEOMACS="$NEOMACS" NEOMACS_MCP_RELAY="$RELAY" \
  python test/lisp/neomacs-mcp-sdk.py --output /owned/sdk-evidence
```

The SDK test is explicitly 2025-11-25 interoperability. It exercises the real
stdio relay/native listener, full eval, companion read/edit/receipt/undo and
study preservation, then proves relay EOF leaves the editor alive and normal
shutdown removes the owned endpoint. It does not reconfigure any client.
