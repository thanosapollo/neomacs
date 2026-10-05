#!/usr/bin/env python3
"""Exercise source-loaded native MCP in an owned headless foreground daemon.

NEOMACS names the executable. No personal sockets/configuration are used.
Raw current-protocol wire proof is not SDK or exhaustive conformance proof.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import time

META = {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "native-regression", "version": "1"}}


class Wire:
    def __init__(self, path):
        self.sock = socket.socket(socket.AF_UNIX)
        self.sock.settimeout(4)
        self.sock.connect(str(path))
        self.buffer = b""
        self.serial = 0

    def send(self, method, params=None, modern=True, request_id=None):
        self.serial += 1
        params = dict(params or {})
        if modern:
            params["_meta"] = META
        message = {"jsonrpc": "2.0", "id": self.serial if request_id is None else request_id,
                   "method": method, "params": params}
        self.sock.sendall(json.dumps(message, ensure_ascii=False).encode() + b"\n")
        return message["id"]

    def receive(self):
        while b"\n" not in self.buffer:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise EOFError("MCP peer closed")
            self.buffer += chunk
        line, self.buffer = self.buffer.split(b"\n", 1)
        return json.loads(line)

    def request(self, method, params=None, modern=True):
        request_id = self.send(method, params, modern)
        result = self.receive()
        assert result["id"] == request_id, result
        return result

    def tool(self, name, arguments):
        result = self.request("tools/call", {"name": name, "arguments": arguments})
        assert "error" not in result, result
        return result["result"]

    def data(self, name, arguments):
        result = self.tool(name, arguments)
        assert not result.get("isError"), result
        return json.loads(result["content"][0]["text"])

    def close(self):
        self.sock.close()


def run(output):
    executable = Path(os.environ["NEOMACS"]).resolve()
    repo = Path(__file__).resolve().parents[2]
    root = Path(tempfile.mkdtemp(prefix="nmcp-", dir=os.environ.get("TMPDIR")))
    for name in ("home", "run", "state", "cache", "config", "data"):
        (root / name).mkdir(mode=0o700)
    env = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8", "RUST_LOG": "error", "TMPDIR": str(root)}
    env.update(HOME=str(root / "home"), **{"XDG_" + name.upper() + "_HOME": str(root / name)
                                         for name in ("state", "cache", "config", "data")})
    env["XDG_RUNTIME_DIR"] = str(root / "run")
    mcp_socket = root / "run/mcp"
    command = [str(executable), "-Q", "--fg-daemon=native-mcp-regression", "-L", str(repo / "lisp"),
               "--eval", f'(setq server-socket-dir {json.dumps(str(root / "run"))})',
               "--eval", '(progn (require \'neomacs-mcp-companion) (neomacs-mcp-enable-companion-tools))',
               "--eval", f'(neomacs-mcp-start {json.dumps(str(mcp_socket))})']
    output.mkdir(parents=True, exist_ok=True)
    rows, peers = [], []
    log = (output / "daemon.log").open("w")
    process = subprocess.Popen(command, env=env, cwd=root, stdout=log, stderr=subprocess.STDOUT)

    def record(name, ok, observed=None):
        rows.append({"name": name, "pass": bool(ok), "observed": observed})
        assert ok, (name, observed)

    def connect():
        wire = Wire(mcp_socket)
        peers.append(wire)
        return wire

    try:
        deadline = time.monotonic() + 20
        while not mcp_socket.exists():
            if process.poll() is not None:
                raise RuntimeError("Daemon exited before MCP readiness")
            if time.monotonic() >= deadline:
                raise TimeoutError("MCP readiness")
            time.sleep(.01)
        w = connect()
        discover = w.request("server/discover")["result"]
        record("current-discover", discover["resultType"] == "complete" and discover["ttlMs"] == 0, discover)
        tools = w.request("tools/list")["result"]
        record("current-native-tools", len(tools["tools"]) == 8 and tools["cacheScope"] == "private", tools)
        ping = w.request("ping")["result"]
        record("current-ping-complete", ping.get("resultType") == "complete", ping)
        identity = w.data("neomacs_identity", {})
        record("owned-process-identity", identity["pid"] == process.pid, identity)
        instance = identity["instance"]

        def evaluate(code, wire=w):
            return wire.tool("neomacs_eval", {"instance": instance, "code": code})

        value = evaluate('(setq native-mcp-value "Ελλάδα\\n42") native-mcp-value')
        record("trusted-eval-persistent-unicode", "Ελλάδα" in value["content"][0]["text"] and not value["isError"], value)
        wrong = w.tool("neomacs_eval", {"instance": "wrong", "code": "(setq native-mcp-wrong t)"})
        absent = evaluate('(boundp \'native-mcp-wrong)')
        record("instance-fence-before-effect", wrong["isError"] and absent["content"][0]["text"] == "nil")
        bad = w.request("tools/call", {"name": "neomacs_eval", "arguments": {"instance": instance, "code": 7}})
        record("argument-validation", bad["error"]["code"] == -32602, bad)
        badmeta = w.request("tools/list", modern=False)
        record("current-missing-metadata", badmeta["error"]["code"] == -32602, badmeta)
        w.sock.sendall(b'{bad}\n')
        malformed = w.receive()
        record("parse-error-recovery", malformed["error"]["code"] == -32700 and "result" in w.request("ping"), malformed)
        w.sock.sendall(b'{"jsonrpc":"2.0","id":999,"method":"tools/list","params":{"bad":"\xff"}}\n')
        invalid_utf8 = w.receive()
        record("invalid-utf8-refusal", invalid_utf8["error"]["code"] == -32700, invalid_utf8)
        unsupported = w.request("tools/list", {"_meta": dict(META, **{"io.modelcontextprotocol/protocolVersion": "1900-01-01"})}, modern=False)
        record("unsupported-version-data", unsupported["error"]["code"] == -32022 and unsupported["error"]["data"] == {
            "supported": ["2026-07-28", "2025-11-25"], "requested": "1900-01-01"}, unsupported)
        legacy = connect()
        initialized = legacy.request("initialize", {"protocolVersion": "2025-11-25", "capabilities": {},
                                                   "clientInfo": {"name": "native-regression", "version": "1"}}, modern=False)
        assert initialized["result"]["protocolVersion"] == "2025-11-25", initialized
        legacy.sock.sendall(b'{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
        for name, meta in [("progress", {"progressToken": "wire-progress"}),
                           ("unrelated", {"example.com/context": "fixture"})]:
            response = legacy.request("tools/call", {"name": "neomacs_eval", "arguments": {
                "instance": instance, "code": "(+ 20 22)"}, "_meta": meta}, modern=False)
            result = response.get("result", {})
            record("legacy-" + name + "-metadata-eval", not result.get("isError", True)
                   and result["content"][0]["text"] == "42" and "resultType" not in result, response)
        record("legacy-ping-empty", legacy.request("ping", modern=False)["result"] == {})
        record("dual-era-explicit-modern-ping", legacy.request("ping")["result"].get("resultType") == "complete")
        legacy.close()
        oversized = connect()
        oversized.sock.sendall(b"x" * 131073)
        try:
            eof = oversized.sock.recv(1) == b""
        except ConnectionResetError:
            eof = True
        record("oversized-frame-closes-exact-peer", eof and "result" in w.request("ping"))
        # Fragment a UTF-8 message across a multibyte code point.
        w.serial += 1
        message = {"jsonrpc": "2.0", "id": w.serial, "method": "tools/call", "params": {
            "_meta": META, "name": "neomacs_eval", "arguments": {"instance": instance, "code": '"λ"'}}}
        encoded = json.dumps(message, ensure_ascii=False).encode() + b"\n"
        split = encoded.index("λ".encode()) + 1
        w.sock.sendall(encoded[:split]); w.sock.sendall(encoded[split:])
        fragmented = w.receive()
        record("fragmented-utf8", "λ" in fragmented["result"]["content"][0]["text"], fragmented)
        # No successor admission while the active eval yields to timer/filter work.
        a = connect(); b = connect()
        a.send("tools/call", {"name": "neomacs_eval", "arguments": {"instance": instance, "code":
            "(setq native-mcp-order '(start)) (sleep-for 0.15) (setq native-mcp-order (append native-mcp-order '(end)))"}})
        time.sleep(.03)
        b.send("tools/call", {"name": "neomacs_eval", "arguments": {"instance": instance, "code":
            "(setq native-mcp-order (append native-mcp-order '(successor)))"}})
        a.receive(); b.receive()
        order = evaluate("native-mcp-order")["content"][0]["text"]
        record("guarded-no-nested-mutation", order == "(start end successor)", order)
        # Active yielding cancellation suppresses result, never rolls back effect.
        cancel_id = a.send("tools/call", {"name": "neomacs_eval", "arguments": {"instance": instance, "code":
            "(sleep-for 0.1) (setq native-mcp-cancelled-effect 'done)"}})
        time.sleep(.02)
        a.sock.sendall(json.dumps({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": cancel_id}}).encode()+b"\n")
        time.sleep(.13)
        a.sock.settimeout(.1)
        try:
            a.receive(); suppressed = False
        except socket.timeout:
            suppressed = True
        a.sock.settimeout(4)
        record("yielding-cancel-suppresses-not-rollback", suppressed and evaluate("native-mcp-cancelled-effect")["content"][0]["text"] == "done")
        # Cancel a queued request behind active work.
        a.send("tools/call", {"name": "neomacs_eval", "arguments": {"instance": instance, "code": "(sleep-for 0.15)"}})
        time.sleep(.02)
        request_id = b.send("tools/call", {"name": "neomacs_eval", "arguments": {"instance": instance, "code": "(setq native-mcp-forbidden t)"}})
        b.sock.sendall(json.dumps({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": request_id}}).encode()+b"\n")
        a.receive()
        record("cancel-before-admission", evaluate("(boundp 'native-mcp-forbidden)")["content"][0]["text"] == "nil")
        # Claim/edit/read/receipt/undo while preserving the native study surface.
        evaluate('(switch-to-buffer (get-buffer-create "*MCP study fixture*")) (erase-buffer) (insert "learner text") (goto-char 3) (setq native-mcp-study-state (list (selected-window) (window-buffer) (point) (window-start) (buffer-string) buffer-undo-list))')
        claim = w.data("neomacs_companion_claim", {"instance": instance})
        args = {"instance": instance, "handle": claim["handle"], "operationId": "wire-edit-1", "tick": claim["tick"], "start": 1, "end": 1, "text": "Companion Ελλάδα\n"}
        receipt = w.data("neomacs_companion_edit", args)
        state = w.data("neomacs_companion_read", {"instance": instance, "handle": claim["handle"]})
        repeat = w.data("neomacs_companion_edit", args)
        record("companion-once-and-readback", receipt == repeat and state["text"] == args["text"], receipt)
        stale = w.tool("neomacs_companion_edit", dict(args, operationId="stale-tick"))
        record("companion-conflict", stale["isError"], stale)
        undo = w.data("neomacs_companion_undo", {"instance": instance, "handle": claim["handle"], "tick": state["tick"]})
        empty = w.data("neomacs_companion_read", {"instance": instance, "handle": claim["handle"]})
        historical = w.data("neomacs_companion_receipt", {"instance": instance, "operationId": "wire-edit-1"})
        record("ordinary-undo-historical-receipt", empty["text"] == "" and historical == receipt, undo)
        preserved = evaluate('(with-current-buffer (window-buffer (selected-window)) (equal native-mcp-study-state (list (selected-window) (window-buffer) (point) (window-start) (buffer-string) buffer-undo-list)))')
        record("study-selection-point-text-undo-preserved", preserved["content"][0]["text"] == "t", preserved)
        # Optional native reads use this same owned daemon and real timers.
        # Preserve the predecessor eight-tool control before enabling the module.
        evaluate("(require 'neomacs-mcp-editor) (neomacs-mcp-enable-editor-tools)")
        read_tools = w.request("tools/list")["result"]["tools"]
        record("optional-native-read-discovery", len(read_tools) == 10 and
               {"neomacs_eval", "neomacs_buffer_list", "neomacs_buffer_read"} <=
               {tool["name"] for tool in read_tools})
        evaluate('(with-current-buffer (get-buffer-create "*MCP read fixture*") (erase-buffer) (insert (make-string 100000 ?x) "Ελλάδα\\n\\\"\\\\🙂") (goto-char 100005) (narrow-to-region 100003 100008) (setq native-mcp-read-state (list (point) (point-min) (point-max) (buffer-modified-tick) buffer-undo-list)))')
        read_args = {"instance": instance, "name": "*MCP read fixture*", "start": 100001, "maxChars": 4096}
        literal = w.data("neomacs_buffer_read", read_args)
        record("native-character-read-literal-distant-offset", literal["text"] == 'Ελλάδα\n"\\🙂'
               and literal["start"] == 100001 and literal["end"] == 100011
               and not literal["truncated"] and literal["nextStart"] is None)
        stale = w.tool("neomacs_buffer_read", dict(read_args, expectedTick=literal["tick"] - 1))
        record("native-character-read-stale-tick", stale["isError"])
        listed = w.data("neomacs_buffer_list", {"instance": instance, "offset": 0, "limit": 2})
        record("native-discovery-bounded-metadata", len(listed["buffers"]) <= 2 and listed["scanned"] <= 128)
        # Finite two-peer service observation, not physical keyboard fairness.
        # No manual drain/timer pumping: each peer submits eight bounded reads.
        burst_start = time.monotonic_ns()
        bursts = []
        for peer in (a, b):
            ids = [peer.send("tools/call", {"name": "neomacs_buffer_read", "arguments": read_args})
                   for _ in range(8)]
            bursts.append((peer, ids))
        for peer_index, (peer, ids) in enumerate(bursts):
            for request_id in ids:
                response = peer.receive()
                assert response["id"] == request_id and not response["result"]["isError"]
            rows.append({"name": "native-read-finite-peer-burst", "pass": True,
                         "observed": {"peer": peer_index, "completed": len(ids),
                                      "receiveElapsedNs": time.monotonic_ns() - burst_start}})
        reader_preserved = evaluate('(with-current-buffer "*MCP read fixture*" (equal native-mcp-read-state (list (point) (point-min) (point-max) (buffer-modified-tick) buffer-undo-list)))')
        study_preserved = evaluate('(with-current-buffer (window-buffer (selected-window)) (equal native-mcp-study-state (list (selected-window) (window-buffer) (point) (window-start) (buffer-string) buffer-undo-list)))')
        record("native-read-target-and-study-preservation", reader_preserved["content"][0]["text"] == "t"
               and study_preserved["content"][0]["text"] == "t")
        # Lose a reply after effect, reconnect and query without replay.
        lost = connect()
        edit2 = dict(args, operationId="lost-reply", tick=empty["tick"], text="second")
        lost.send("tools/call", {"name": "neomacs_companion_edit", "arguments": edit2})
        time.sleep(.03); lost.close()
        recovered = w.data("neomacs_companion_receipt", {"instance": instance, "operationId": "lost-reply"})
        record("disconnect-after-effect-receipt", recovered["status"] == "succeeded", recovered)
        # A custom large-output handler exercises real native send backpressure.
        evaluate('(neomacs-mcp-register-tool "test_large" "Fixture" (neomacs-mcp--schema nil nil) (lambda (_) (make-string 120000 ?x)))')
        slow = connect(); slow.sock.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 1024)
        started = time.monotonic()
        for _ in range(6):
            slow.send("tools/call", {"name": "test_large", "arguments": {}})
        time.sleep(.4)
        responsiveness = w.request("ping")
        slow_bytes = 0
        closed = False
        try:
            while True:
                chunk = slow.sock.recv(65536)
                if not chunk:
                    closed = True
                    break
                slow_bytes += len(chunk)
        except socket.timeout:
            pass
        record("slow-peer-recovery", closed and slow_bytes < 720000 and "result" in responsiveness and time.monotonic()-started < 2,
               {"closed": closed, "bytes": slow_bytes})
        # Legacy is explicitly separate: initialize and initialized, no modern metadata.
        legacy = connect()
        init = legacy.request("initialize", {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "legacy-wire", "version": "1"}}, modern=False)
        record("legacy-version", init["result"]["protocolVersion"] == "2025-11-25", init)
        legacy.sock.sendall(b'{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
        listed = legacy.request("tools/list", modern=False)
        record("legacy-list-envelope", "resultType" not in listed["result"] and len(listed["result"]["tools"]) == 11, listed)
        client = executable.with_name("emacsclient" if executable.name.startswith("emacs") else "neomacsclient")
        # Retire the endpoint while an active eval yields; old queued work must
        # not migrate to its successor. Active effects are not rolled back.
        a.send("tools/call", {"name": "neomacs_eval", "arguments": {"instance": instance, "code": "(sleep-for 0.2)"}})
        time.sleep(.03)
        b.send("tools/call", {"name": "neomacs_eval", "arguments": {"instance": instance, "code": "(setq native-mcp-old-generation-effect t)"}})
        time.sleep(.02)
        restart = subprocess.run([str(client), "-s", str(root/"run/native-mcp-regression"), "--eval", f'(progn (neomacs-mcp-stop) (neomacs-mcp-start {json.dumps(str(mcp_socket))}))'], env=env, capture_output=True, timeout=5)
        replacement = connect()
        next_identity = replacement.data("neomacs_identity", {})
        stale_effect = evaluate("(boundp 'native-mcp-old-generation-effect)", replacement)
        historical = replacement.data("neomacs_companion_receipt", {"instance": instance, "operationId": "lost-reply"})
        record("endpoint-restart-generation-fence-stable-boot-receipt", restart.returncode == 0 and next_identity["instance"] == instance and next_identity["endpointGeneration"] > identity["endpointGeneration"] and stale_effect["content"][0]["text"] == "nil" and historical == recovered, next_identity)
        # EOF must remove exact peer; shutdown must remove owned socket.
        for peer in peers:
            peer.close()
        shutdown = subprocess.run([str(client), "-s", str(root/"run/native-mcp-regression"), "--eval", "(kill-emacs 0)"], env=env, capture_output=True, timeout=5)
        status = process.wait(timeout=5)
        record("normal-shutdown-owned-cleanup", status == 0 and not mcp_socket.exists(), {"exit": status, "clientExit": shutdown.returncode})
    finally:
        for peer in peers:
            peer.close()
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill(); process.wait(timeout=5)
        log.close()
        shutil.rmtree(root)
        receipt = {"executable": str(executable), "command": command, "checks": rows,
                   "count": len(rows), "passed": sum(row["pass"] for row in rows), "fixtureRemoved": not root.exists()}
        (output / "protocol.json").write_text(json.dumps(receipt, indent=2, ensure_ascii=False)+"\n")
        print(json.dumps({key: receipt[key] for key in ("count", "passed", "fixtureRemoved")}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    run(parser.parse_args().output)
