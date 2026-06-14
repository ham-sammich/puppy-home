"""Find which autosave holds the offending tool id and whether repair fixes it."""
from __future__ import annotations

import importlib.util
import pathlib
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
SIDECAR_PATH = HERE.parent / "sidecar" / "sidecar.py"
_spec = importlib.util.spec_from_file_location("pp_sidecar", str(SIDECAR_PATH))
_sidecar = importlib.util.module_from_spec(_spec)
assert _spec and _spec.loader
_spec.loader.exec_module(_sidecar)
repair = _sidecar.repair_tool_call_adjacency
violations = _sidecar.history_tool_violations

from code_puppy.agents._history import (  # noqa: E402
    _classify_tool_part,
    prune_interrupted_tool_calls,
    sanitize_tool_call_ids,
)
from code_puppy.config import AUTOSAVE_DIR  # noqa: E402
from code_puppy.session_storage import list_sessions, load_session  # noqa: E402

TARGET = sys.argv[1] if len(sys.argv) > 1 else "toolu_01TFpfV9es"


def parts(m):
    return getattr(m, "parts", []) or []


def full_pipeline(hist):
    return sanitize_tool_call_ids(prune_interrupted_tool_calls(repair(hist)))


def has_target(hist):
    for m in hist:
        for p in parts(m):
            if TARGET in str(getattr(p, "tool_call_id", "")):
                return True
    return False


base = pathlib.Path(AUTOSAVE_DIR)
for name in list_sessions(base):
    try:
        hist = load_session(name, base)
    except Exception as exc:
        print(f"{name}: LOAD FAIL {exc}")
        continue
    if not hist:
        continue
    v = violations(hist)
    tgt = has_target(hist)
    if v or tgt:
        fixed = full_pipeline(hist)
        vf = violations(fixed)
        print(f"\n=== {name} (msgs={len(hist)}) target={tgt} ===")
        print(f"  violations before={len(v)}  after_repair={len(vf)}")
        for row in v[:8]:
            print(f"    {row}")
        if vf:
            print("  !!! STILL BAD AFTER REPAIR:")
            for row in vf[:8]:
                print(f"    {row}")
print("\ndone")
