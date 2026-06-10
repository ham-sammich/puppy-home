#!/usr/bin/env python3
"""puppy-home CDP helper — inspect the in-app browser over the Chrome DevTools
Protocol with **zero third-party dependencies** (raw websocket over stdlib).

Usage:
    python cdp.py <http_endpoint> console [seconds]   # dump console logs/errors
    python cdp.py <http_endpoint> eval "<js expr>"    # run JS, print result
    python cdp.py <http_endpoint> screenshot <out.png>

<http_endpoint> looks like http://127.0.0.1:53239 (from .puppy/browser.json).

Agents: RUN THIS HELPER. Do not write your own CDP script into the project.
"""
import base64
import json
import os
import socket
import struct
import sys
import time
import urllib.request
from urllib.parse import urlparse


def _find_ws(endpoint: str) -> str:
    url = endpoint.rstrip("/") + "/json/list"
    with urllib.request.urlopen(url, timeout=5) as r:
        targets = json.loads(r.read())
    pages = [t for t in targets if t.get("type") == "page"] or targets
    for t in pages:
        if t.get("webSocketDebuggerUrl"):
            return t["webSocketDebuggerUrl"]
    raise SystemExit("cdp: no debuggable page target at " + endpoint)


class WS:
    """A tiny websocket client (client frames masked; we read server frames)."""

    def __init__(self, url: str):
        u = urlparse(url)
        path = (u.path or "/") + (("?" + u.query) if u.query else "")
        self.sock = socket.create_connection((u.hostname, u.port or 80), timeout=10)
        key = base64.b64encode(os.urandom(16)).decode()
        self.sock.sendall(
            (
                f"GET {path} HTTP/1.1\r\n"
                f"Host: {u.hostname}:{u.port or 80}\r\n"
                "Upgrade: websocket\r\nConnection: Upgrade\r\n"
                f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
            ).encode()
        )
        self._buf = b""
        while b"\r\n\r\n" not in self._buf:
            self._buf += self.sock.recv(4096)
        head, self._buf = self._buf.split(b"\r\n\r\n", 1)
        if b"101" not in head.split(b"\r\n", 1)[0]:
            raise SystemExit("cdp: websocket handshake failed")

    def send(self, obj) -> None:
        data = json.dumps(obj).encode()
        header = bytearray([0x81])  # FIN + text
        n = len(data)
        if n < 126:
            header.append(0x80 | n)
        elif n < 65536:
            header.append(0x80 | 126)
            header += struct.pack(">H", n)
        else:
            header.append(0x80 | 127)
            header += struct.pack(">Q", n)
        mask = os.urandom(4)
        header += mask
        self.sock.sendall(bytes(header) + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))

    def _exact(self, n: int) -> bytes:
        while len(self._buf) < n:
            chunk = self.sock.recv(4096)
            if not chunk:
                raise SystemExit("cdp: connection closed")
            self._buf += chunk
        out, self._buf = self._buf[:n], self._buf[n:]
        return out

    def recv(self):
        b = self._exact(2)
        opcode = b[0] & 0x0F
        length = b[1] & 0x7F
        if length == 126:
            length = struct.unpack(">H", self._exact(2))[0]
        elif length == 127:
            length = struct.unpack(">Q", self._exact(8))[0]
        payload = self._exact(length) if length else b""
        if opcode == 0x8:
            raise SystemExit("cdp: closed by peer")
        if opcode == 0x9:  # ping
            return None
        return payload.decode("utf-8", "replace")

    def close(self) -> None:
        try:
            self.sock.close()
        except Exception:
            pass


def main() -> None:
    if len(sys.argv) < 3:
        print(__doc__)
        raise SystemExit(2)
    endpoint, cmd = sys.argv[1], sys.argv[2]
    try:
        ws = WS(_find_ws(endpoint))
    except OSError as e:
        raise SystemExit(f"cdp: cannot reach {endpoint} ({e}). Is the browser open?")
    counter = [0]

    def call(method, params=None):
        counter[0] += 1
        ws.send({"id": counter[0], "method": method, "params": params or {}})
        return counter[0]

    def wait_for(req_id, timeout=8.0):
        ws.sock.settimeout(timeout)
        end = time.time() + timeout
        while time.time() < end:
            msg = ws.recv()
            if msg:
                d = json.loads(msg)
                if d.get("id") == req_id:
                    return d
        return None

    if cmd == "eval":
        expr = sys.argv[3] if len(sys.argv) > 3 else "1+1"
        d = wait_for(call("Runtime.evaluate", {"expression": expr, "returnByValue": True}))
        print(json.dumps((d or {}).get("result", d), indent=2))
    elif cmd == "screenshot":
        out = sys.argv[3] if len(sys.argv) > 3 else "screenshot.png"
        d = wait_for(call("Page.captureScreenshot", {}), timeout=15)
        data = (d or {}).get("result", {}).get("data")
        if data:
            with open(out, "wb") as f:
                f.write(base64.b64decode(data))
            print("saved " + out)
        else:
            print("cdp: no screenshot data")
    else:  # console
        seconds = float(sys.argv[3]) if len(sys.argv) > 3 else 3.0
        call("Runtime.enable")
        call("Log.enable")
        call("Page.enable")
        call("Page.reload", {})  # capture fresh page-load console output
        ws.sock.settimeout(0.4)
        events, end = [], time.time() + seconds
        while time.time() < end:
            try:
                msg = ws.recv()
            except (socket.timeout, TimeoutError):
                continue
            except SystemExit:
                break
            if not msg:
                continue
            try:
                d = json.loads(msg)
            except Exception:
                continue
            m = d.get("method")
            if m == "Runtime.consoleAPICalled":
                p = d["params"]
                args = " ".join(
                    str(a.get("value", a.get("description", ""))) for a in p.get("args", [])
                )
                events.append(f"[console.{p.get('type')}] {args}")
            elif m == "Log.entryAdded":
                e = d["params"]["entry"]
                events.append(f"[{e.get('level')}] {e.get('text')}  ({e.get('url', '')})")
            elif m == "Runtime.exceptionThrown":
                ex = d["params"]["exceptionDetails"]
                desc = ex.get("exception", {}).get("description", ex.get("text", ""))
                events.append(f"[exception] {desc}")
        print("\n".join(events) if events else f"(no console output in {seconds:.1f}s)")
    ws.close()


if __name__ == "__main__":
    main()
