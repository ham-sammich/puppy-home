"""Force (and fix) the Claude Code OAuth refresh-token race across processes.

Background
----------
Every puppy-home workspace runs its own sidecar process, but they all share a
single OAuth token file. The code_puppy plugin only coordinates refreshes within
a process, so concurrent instances refresh simultaneously. Anthropic rotates the
refresh token on every refresh (single-use), so the second concurrent refresh
gets HTTP 400 -- which is why fresh workspaces "fail to load" and the first
prompt 400s.

This harness reproduces that with N real subprocesses hammering a local mock
token endpoint that mimics single-use refresh-token rotation, then proves the
cross-process guard in sidecar.py eliminates it.

Run:  python scripts/verify_oauth_refresh_race.py
Exits 0 if the bug reproduces WITHOUT the guard and is gone WITH it.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

WORKERS = 5
SERVER_DELAY = 0.3  # widen the overlap window so the workers truly collide
HERE = Path(__file__).resolve().parent
SIDECAR_PATH = HERE.parent / "sidecar" / "sidecar.py"


# ---------------------------------------------------------------------------
# Worker: run inside each subprocess. Points the plugin at the mock endpoint +
# a shared token file, optionally installs the guard, then refreshes.
# ---------------------------------------------------------------------------
def _run_worker() -> int:
    token_url = os.environ["MOCK_TOKEN_URL"]
    token_file = os.environ["SHARED_TOKEN_FILE"]
    result_file = os.environ["RESULT_FILE"]
    start_at = float(os.environ["START_AT"])
    use_guard = os.environ.get("USE_GUARD") == "1"

    from code_puppy.plugins.claude_code_oauth import utils as ccu

    # Redirect the plugin to our mock endpoint + shared token file, and stub the
    # model-config writeback (irrelevant to the race, touches other files).
    ccu.CLAUDE_CODE_OAUTH_CONFIG["token_url"] = token_url
    ccu.get_token_storage_path = lambda: Path(token_file)  # type: ignore[assignment]
    ccu.update_claude_code_model_tokens = lambda *a, **k: True  # type: ignore[assignment]

    if use_guard:
        import importlib.util

        spec = importlib.util.spec_from_file_location("pp_sidecar", str(SIDECAR_PATH))
        mod = importlib.util.module_from_spec(spec)
        assert spec and spec.loader
        spec.loader.exec_module(mod)
        mod.install_oauth_refresh_guard()

    # Barrier: all workers fire at the same wall-clock instant.
    time.sleep(max(0.0, start_at - time.time()))

    result = {"pid": os.getpid(), "ok": False, "token": None, "error": None}
    try:
        token = ccu.get_valid_access_token()
        result["token"] = token
        result["ok"] = bool(token)
    except Exception as exc:  # pragma: no cover - defensive
        result["error"] = f"{type(exc).__name__}: {exc}"

    with open(result_file, "w", encoding="utf-8") as fh:
        json.dump(result, fh)
    return 0


# ---------------------------------------------------------------------------
# Mock token endpoint: single-use refresh tokens (rotation), like Anthropic.
# ---------------------------------------------------------------------------
class _MockOAuth:
    def __init__(self) -> None:
        self.valid_refresh = {"refresh-initial"}
        self.lock = threading.Lock()
        self.requests = 0
        self.successes = 0
        self.failures = 0
        self._seq = 0

    def handle(self, presented: str | None) -> tuple[int, dict]:
        time.sleep(SERVER_DELAY)  # read body, then collide on the lock below
        with self.lock:
            self.requests += 1
            if presented and presented in self.valid_refresh:
                self.valid_refresh.discard(presented)
                self._seq += 1
                new_refresh = f"refresh-{self._seq}"
                self.valid_refresh.add(new_refresh)
                self.successes += 1
                return 200, {
                    "access_token": f"access-{self._seq}",
                    "refresh_token": new_refresh,
                    "expires_in": 28800,
                }
            self.failures += 1
            return 400, {"error": "invalid_grant",
                         "error_description": "refresh token reused/rotated"}


def _make_handler(state: _MockOAuth):
    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):  # noqa: N802
            length = int(self.headers.get("Content-Length", 0))
            raw = self.rfile.read(length) if length else b"{}"
            try:
                presented = json.loads(raw).get("refresh_token")
            except Exception:
                presented = None
            status, body = state.handle(presented)
            payload = json.dumps(body).encode("utf-8")
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def log_message(self, *a):  # silence
            return

    return Handler


# ---------------------------------------------------------------------------
# Driver: one race run (guard on or off).
# ---------------------------------------------------------------------------
def _run_race(use_guard: bool) -> dict:
    state = _MockOAuth()
    server = HTTPServer(("127.0.0.1", 0), _make_handler(state))
    port = server.server_address[1]
    threading.Thread(target=server.serve_forever, daemon=True).start()

    tmp = Path(tempfile.mkdtemp(prefix="pp_oauth_race_"))
    token_file = tmp / "claude_code_oauth.json"
    # Seed an EXPIRED token whose refresh_token is the server's single live one.
    token_file.write_text(json.dumps({
        "access_token": "access-initial",
        "refresh_token": "refresh-initial",
        "expires_in": 28800,
        "expires_at": time.time() - 100,  # already expired -> forces a refresh
    }), encoding="utf-8")

    start_at = time.time() + 1.0
    procs = []
    result_files = []
    for i in range(WORKERS):
        rf = tmp / f"result_{i}.json"
        result_files.append(rf)
        env = dict(os.environ)
        env.update(
            PP_WORKER="1",
            MOCK_TOKEN_URL=f"http://127.0.0.1:{port}/token",
            SHARED_TOKEN_FILE=str(token_file),
            RESULT_FILE=str(rf),
            START_AT=str(start_at),
            USE_GUARD="1" if use_guard else "0",
        )
        # Worker reports via RESULT_FILE; mute its stdout (guard log lines).
        procs.append(subprocess.Popen(
            [sys.executable, __file__, "--worker"],
            env=env, stdout=subprocess.DEVNULL,
        ))

    for p in procs:
        p.wait(timeout=120)
    server.shutdown()

    results = []
    for rf in result_files:
        try:
            results.append(json.loads(rf.read_text(encoding="utf-8")))
        except Exception:
            results.append({"ok": False, "token": None, "error": "no result file"})

    return {
        "ok_count": sum(1 for r in results if r.get("ok")),
        "fail_count": sum(1 for r in results if not r.get("ok")),
        "server_requests": state.requests,
        "server_successes": state.successes,
        "server_failures": state.failures,
    }


def main() -> int:
    print(f"== Forcing the OAuth refresh race with {WORKERS} concurrent processes ==\n")

    print("[1/2] WITHOUT guard (expect failures + multiple network refreshes)")
    no_guard = _run_race(use_guard=False)
    print(f"      workers ok={no_guard['ok_count']} fail={no_guard['fail_count']} | "
          f"server reqs={no_guard['server_requests']} "
          f"200s={no_guard['server_successes']} 400s={no_guard['server_failures']}")

    print("\n[2/2] WITH guard (expect all-ok + exactly one network refresh)")
    guard = _run_race(use_guard=True)
    print(f"      workers ok={guard['ok_count']} fail={guard['fail_count']} | "
          f"server reqs={guard['server_requests']} "
          f"200s={guard['server_successes']} 400s={guard['server_failures']}")

    bug_reproduced = no_guard["fail_count"] > 0 and no_guard["server_requests"] >= 2
    fix_works = guard["fail_count"] == 0 and guard["server_requests"] == 1

    print("\n== Verdict ==")
    print(f"  bug reproduced without guard : {'YES' if bug_reproduced else 'no'}")
    print(f"  fix works with guard         : {'YES' if fix_works else 'NO'}")

    if bug_reproduced and fix_works:
        print("\nPASS: the cross-process guard eliminates the 400 race. woof!")
        return 0
    print("\nFAIL: see numbers above.")
    return 1


if __name__ == "__main__":
    if "--worker" in sys.argv:
        sys.exit(_run_worker())
    sys.exit(main())
