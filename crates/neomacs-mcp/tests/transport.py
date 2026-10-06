"""Real OS pipes/sockets; fixture peer is transport-only, not MCP proof.
Run: python3 transport.py /absolute/path/to/neomacs-mcp
"""
import os
import pathlib
import select
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest

BINARY = pathlib.Path(sys.argv.pop(1)).resolve()
BUDGET = 300


class Transport(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="mcp-relay-", dir=os.environ["TMPDIR"])
        self.root = pathlib.Path(self.tmp.name)
        self.path = str(self.root / "peer")
        self.listener = socket.socket(socket.AF_UNIX)
        self.listener.bind(self.path)
        self.listener.listen(4)
        self.listener.settimeout(3)
        self.children = []
        self.sockets = []
        self.threads = []
        self.errors = []

    def tearDown(self):
        for p in self.children:
            if p.poll() is None:
                p.kill()
            p.wait(timeout=3)
            for f in [p.stdin, p.stdout, p.stderr]:
                if f and not f.closed:
                    f.close()
        for s in self.sockets:
            s.close()
        self.listener.close()
        for t in self.threads:
            t.join(3)
            self.assertFalse(t.is_alive(), "fixture worker leaked")
        self.tmp.cleanup()
        if self.errors:
            raise self.errors[0]

    def launch(self, path=None, **kwargs):
        p = subprocess.Popen([str(BINARY), "--socket", path or self.path,
                              "--timeout-ms", str(BUDGET)], stdin=subprocess.PIPE,
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, **kwargs)
        self.children.append(p)
        return p

    def accept(self):
        peer, _ = self.listener.accept()
        peer.settimeout(3)
        self.sockets.append(peer)
        return peer

    def thread(self, fn):
        def wrapped():
            try:
                fn()
            except Exception as e:
                self.errors.append(e)
        t = threading.Thread(target=wrapped)
        t.start()
        self.threads.append(t)
        return t

    def finish(self, p, success=True, needle=None):
        start = time.monotonic()
        code = p.wait(timeout=2)
        self.assertLess(time.monotonic() - start, 1.2)
        self.assertEqual(code, 0 if success else 1)
        err = p.stderr.read()
        if needle:
            self.assertIn(needle, err)
        elif success:
            self.assertEqual(err, b"")
        return p.stdout.read()

    def test_utf8_ndjson_fragmented_and_coalesced_blind(self):
        payload = ('{"x":"γειά 👋","escaped":"a\\nb"}\n'
                   '{"not":"a protocol request"}\n\n').encode() + b'\xff\x00not-json\r\n'
        p = self.launch()
        peer = self.accept()
        received = bytearray()
        def server():
            while True:
                data = peer.recv(7)
                if not data:
                    break
                received.extend(data)
                # Deliberately split UTF8 sequences and framing both ways.
                for byte in data:
                    peer.sendall(bytes([byte]))
            peer.shutdown(socket.SHUT_WR)
        self.thread(server)
        for chunk in [payload[:1], payload[1:8], payload[8:]]:
            p.stdin.write(chunk)
            p.stdin.flush()
        p.stdin.close()
        self.assertEqual(self.finish(p), payload)
        self.assertEqual(bytes(received), payload)

    def test_large_duplex_exact_bytes(self):
        payload = bytes(range(256)) * 8192
        p = self.launch()
        peer = self.accept()
        def server():
            while data := peer.recv(16384):
                peer.sendall(data)
            peer.shutdown(socket.SHUT_WR)
        self.thread(server)
        out, err = p.communicate(payload, timeout=6)
        self.assertEqual(p.returncode, 0)
        self.assertEqual(out, payload)
        self.assertEqual(err, b"")

    def test_eof_drains_delayed_response_and_keeps_listener(self):
        p = self.launch()
        peer = self.accept()
        def server():
            request = b""
            while data := peer.recv(1024):
                request += data
            self.assertEqual(request, b"request\n")
            peer.sendall(b"final outstanding\n")
            peer.shutdown(socket.SHUT_WR)
        self.thread(server)
        p.stdin.write(b"request\n")
        p.stdin.close()
        self.assertEqual(self.finish(p), b"final outstanding\n")
        # Relay closes only its connection; it neither unlinks nor kills server.
        self.assertTrue(pathlib.Path(self.path).exists())
        next_client = socket.socket(socket.AF_UNIX)
        self.sockets.append(next_client)
        next_client.connect(self.path)
        self.accept()

    def test_eof_no_peer_eof_deadline_and_peer_recovers(self):
        p = self.launch()
        peer = self.accept()
        p.stdin.close()
        self.assertEqual(peer.recv(1), b"")
        self.assertEqual(self.finish(p, False, b"stdin EOF drain deadline"), b"")
        self.assertEqual(peer.recv(1), b"")
        q = self.launch()
        successor = self.accept()
        successor.sendall(b"recovered\n")
        successor.close()
        self.assertEqual(self.finish(q), b"recovered\n")

    def test_peer_close_exits_with_stdin_still_open_blocked(self):
        p = self.launch()
        self.accept().close()
        self.assertEqual(self.finish(p), b"")
        self.assertFalse(p.stdin.closed)

    def test_stdout_broken_cleanup(self):
        p = self.launch()
        peer = self.accept()
        p.stdout.close()
        peer.sendall(b"response\n")
        p.wait(timeout=2)
        self.assertEqual(p.returncode, 1)
        self.assertIn(b"socket to stdout", p.stderr.read())
        self.assertEqual(peer.recv(1), b"")

    def test_unread_stdout_backpressure_bounded_and_exit(self):
        p = self.launch()
        peer = self.accept()
        peer.setblocking(False)
        sent = 0
        deadline = time.monotonic() + 2
        block = b"x" * 16384
        while time.monotonic() < deadline and p.poll() is None:
            try:
                sent += peer.send(block)
            except BlockingIOError:
                select.select([], [peer], [], 0.03)
            except (BrokenPipeError, ConnectionResetError):
                break
        self.assertGreater(sent, 16384)
        # No stdout reader until after deadline: this genuinely blocks its worker.
        out = self.finish(p, False, b"stdout write deadline")
        self.assertLess(len(out), sent)
        self.assertLess(sent, 2 * 1024 * 1024, "transport did not backpressure")

    def test_unread_upstream_deadline_closes_own_connection(self):
        p = self.launch()
        peer = self.accept()
        os.set_blocking(p.stdin.fileno(), False)
        sent = 0
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline and p.poll() is None:
            try:
                sent += os.write(p.stdin.fileno(), b"x" * 16384)
            except BlockingIOError:
                select.select([], [p.stdin], [], 0.03)
            except BrokenPipeError:
                break
        self.assertGreater(sent, 16384)
        self.assertEqual(self.finish(p, False, b"upstream write deadline"), b"")
        self.assertLess(sent, 2 * 1024 * 1024)
        total = 0
        while data := peer.recv(65536):
            total += len(data)
        self.assertLess(total, sent)

    def test_saturated_stderr_cannot_hang_diagnostic_exit(self):
        read, write = os.pipe()
        try:
            os.set_blocking(write, False)
            while True:
                try:
                    os.write(write, b"x" * 4096)
                except BlockingIOError:
                    break
            os.set_blocking(write, True)
            p = subprocess.Popen([str(BINARY), "--socket", str(self.root / "absent")],
                                 stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=write)
            self.children.append(p)
            p.wait(timeout=1)
            self.assertEqual(p.returncode, 1)
            self.assertEqual(p.stdout.read(), b"")
        finally:
            os.close(write)
            os.close(read)

    def test_explicit_selection_no_fallback_or_automatic_reconnect(self):
        p = self.launch()
        original = self.accept()
        # Replace the pathname while connection is live: original inode stays bound.
        os.unlink(self.path)
        successor = socket.socket(socket.AF_UNIX)
        self.sockets.append(successor)
        successor.bind(self.path)
        successor.listen(4)
        successor.settimeout(0.15)
        original.sendall(b"original\n")
        self.assertEqual(p.stdout.readline(), b"original\n")
        original.close()
        self.assertEqual(self.finish(p), b"")
        with self.assertRaises(TimeoutError):
            successor.accept()
        q = self.launch()
        new_peer, _ = successor.accept()
        self.sockets.append(new_peer)
        new_peer.sendall(b"successor\n")
        new_peer.close()
        self.assertEqual(self.finish(q), b"successor\n")
        missing = self.launch(str(self.root / "missing"))
        self.assertEqual(self.finish(missing, False, b"connect:"), b"")
        self.assertTrue(pathlib.Path(self.path).exists())

    def test_full_connect_backlog_deadline(self):
        self.listener.close()
        os.unlink(self.path)
        self.listener = socket.socket(socket.AF_UNIX)
        self.listener.bind(self.path)
        self.listener.listen(0)
        filler = socket.socket(socket.AF_UNIX)
        self.sockets.append(filler)
        filler.connect(self.path)
        p = self.launch()
        self.assertEqual(self.finish(p, False, b"connect deadline/worker"), b"")
        self.assertTrue(pathlib.Path(self.path).exists())

    def test_cli_requires_one_absolute_socket(self):
        for args in [[], ["--socket", "relative"], ["--socket"],
                     ["--socket", self.path, "--socket", self.path],
                     ["--socket", self.path, "--timeout-ms", "0"],
                     ["--socket", self.path, "--timeout-ms", "60001"],
                     ["--unknown"], ["--socket", self.path, "--start-editor"]]:
            p = subprocess.run([str(BINARY), *args], capture_output=True, timeout=2)
            self.assertEqual(p.returncode, 1)
            self.assertEqual(p.stdout, b"")
            self.assertIn(b"usage:", p.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
