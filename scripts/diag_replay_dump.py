"""Reconstruct the dumped history skeleton and test the repair pipeline on it."""
from __future__ import annotations

import importlib.util
import json
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

dump = json.load(open(sys.argv[1], encoding="utf-8"))


def make_part(p):
    t = p["type"]
    cid = p["tool_call_id"]
    name = p["tool_name"]
    if t == "ToolReturnPart":
        return ToolReturnPart(tool_name=name or "x", content="ok", tool_call_id=cid)
    if t == "ToolCallPart":
        return ToolCallPart(tool_name=name or "x", args={}, tool_call_id=cid)
    if t == "UserPromptPart":
        return UserPromptPart(content="u")
    if t == "TextPart":
        return TextPart(content="t")
    return TextPart(content="?" + t)


hist = []
for r in dump["messages"]:
    parts = [make_part(p) for p in r["parts"]]
    if r["msg"] == "ModelResponse":
        hist.append(ModelResponse(parts=parts))
    else:
        hist.append(ModelRequest(parts=parts))

print("reconstructed", len(hist), "messages")
print("violations before     :", len(violations(hist)))
print("after repair only     :", len(violations(repair(hist))))
print("after prune only      :", len(violations(prune_interrupted_tool_calls(hist))))
full = sanitize_tool_call_ids(prune_interrupted_tool_calls(repair(hist)))
print("after full pipeline    :", len(violations(full)))
for v in violations(full)[:10]:
    print("   STILL BAD:", v)
