"""Prove repair_tool_call_adjacency() fixes the tool_use/tool_result 400.

Reproduces the corruption seen in real autosaved sessions: a tool_result that is
duplicated and re-appended across several ModelRequests (which code_puppy's
set-based prune is blind to), causing Anthropic to 400 with
"unexpected tool_use_id ... must have a corresponding tool_use block in the
previous message".

Runs two checks:
  1. A synthetic broken history (machine-independent) -> must become valid while
     keeping every real user prompt.
  2. An opportunistic scan of the local autosave dir, if present.

Run:  python scripts/verify_history_repair.py   (exits 0 on success)
"""
from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
SIDECAR_PATH = HERE.parent / "sidecar" / "sidecar.py"

# Load the repair straight from the shipped sidecar (single source of truth).
_spec = importlib.util.spec_from_file_location("pp_sidecar", str(SIDECAR_PATH))
_sidecar = importlib.util.module_from_spec(_spec)
assert _spec and _spec.loader
_spec.loader.exec_module(_sidecar)
repair = _sidecar.repair_tool_call_adjacency

from code_puppy.agents._history import (  # noqa: E402
    _classify_tool_part,
    prune_interrupted_tool_calls,
    sanitize_tool_call_ids,
)
from pydantic_ai.messages import (  # noqa: E402
    ModelRequest,
    ModelResponse,
    TextPart,
    ToolCallPart,
    ToolReturnPart,
    UserPromptPart,
)


def _parts(m):
    return getattr(m, "parts", []) or []


def adjacency_orphans(hist):
    """(msg_index, tool_call_id) for every tool_result lacking a prev-msg call."""
    bad = []
    for i, m in enumerate(hist):
        prev_calls = set()
        if i > 0:
            prev_calls = {p.tool_call_id for p in _parts(hist[i - 1])
                          if _classify_tool_part(p) == "call"}
        seen = set()
        for p in _parts(m):
            if _classify_tool_part(p) == "return":
                cid = p.tool_call_id
                if cid not in prev_calls or cid in seen:
                    bad.append((i, cid))
                seen.add(cid)
    return bad


def full_pipeline(hist):
    return sanitize_tool_call_ids(prune_interrupted_tool_calls(repair(hist)))


def user_prompts(hist):
    out = []
    for m in hist:
        for p in _parts(m):
            if type(p).__name__ == "UserPromptPart":
                out.append(p.content)
    return out


def synthetic_case() -> bool:
    print("[1] synthetic corrupted history")
    ret = lambda: ToolReturnPart(tool_name="create_file", content="ok", tool_call_id="t1")
    hist = [
        ModelResponse(parts=[ToolCallPart(tool_name="create_file", args={}, tool_call_id="t1")]),
        ModelRequest(parts=[ret()]),                                   # valid pair
        ModelResponse(parts=[TextPart(content="done")]),
        ModelRequest(parts=[ret(), UserPromptPart(content="add facts")]),       # orphan + prompt
        ModelRequest(parts=[UserPromptPart(content="run server")]),
        ModelRequest(parts=[ret(), ret(), UserPromptPart(content="is it up")]),  # dup x2 + prompt
    ]
    before = adjacency_orphans(hist)
    fixed = full_pipeline(hist)
    after = adjacency_orphans(fixed)
    prompts_before = user_prompts(hist)
    prompts_after = user_prompts(fixed)

    print(f"    orphans before={len(before)} after={len(after)}")
    print(f"    user prompts kept: {prompts_after}")
    ok = (len(before) > 0 and len(after) == 0
          and prompts_after == prompts_before)
    # the one genuine pair must survive
    survived = any(_classify_tool_part(p) == "return"
                   for m in fixed for p in _parts(m))
    print(f"    valid pair preserved: {survived}")
    return ok and survived


def real_sessions_case() -> bool:
    print("\n[2] real autosave scan (opportunistic)")
    try:
        import pathlib

        from code_puppy.config import AUTOSAVE_DIR
        from code_puppy.session_storage import list_sessions, load_session
    except Exception as exc:
        print(f"    skipped ({exc})")
        return True
    base = pathlib.Path(AUTOSAVE_DIR)
    if not base.exists():
        print("    skipped (no autosave dir)")
        return True
    bad_before = bad_after = 0
    for name in list_sessions(base):
        try:
            hist = load_session(name, base)
        except Exception:
            continue
        if not hist:
            continue
        if adjacency_orphans(hist):
            bad_before += 1
            if adjacency_orphans(full_pipeline(hist)):
                bad_after += 1
    print(f"    sessions adjacency-bad: before={bad_before} after_repair={bad_after}")
    return bad_after == 0


def main() -> int:
    ok1 = synthetic_case()
    ok2 = real_sessions_case()
    print("\n== Verdict ==")
    print(f"  synthetic repair works : {'YES' if ok1 else 'NO'}")
    print(f"  real sessions all clean : {'YES' if ok2 else 'NO'}")
    if ok1 and ok2:
        print("\nPASS: adjacency repair kills the tool_result 400. woof!")
        return 0
    print("\nFAIL: see numbers above.")
    return 1


if __name__ == "__main__":
    sys.exit(main())
