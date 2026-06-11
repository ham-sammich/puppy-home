"""puppy-home <-> Code Puppy bridge (sidecar).

This process runs *inside an environment that has `code_puppy` importable*. It
imports Code Puppy's real internals, drives the current agent programmatically,
and bridges its two messaging systems (the new structured MessageBus and the
legacy MessageQueue) to the Rust GUI over a line-delimited JSON protocol.

Protocol (newline-delimited JSON, UTF-8):

  Rust -> sidecar (stdin), one object per line:
    {"op": "prompt",  "id": <int>, "text": "...", "images": ["<b64 png>"]}  # a model turn (images optional)
    {"op": "cancel"}                                 # cancel the running turn
    {"op": "command", "id": <int>, "text": "/..."}  # a slash command
    {"op": "complete", "id": <int>, "text": "...", "cursor": <int>}  # caret at char index
    {"op": "list_commands"}                          # re-send the catalog
    {"op": "list_agents"}                            # re-send the agent catalog
    {"op": "list_models"}                            # re-send the model catalog
    {"op": "set_model", "name": "..."}               # switch model + reload agent
    {"op": "ask_response", "id": "...", "cancelled": false,
        "answers": [{"question_header": "...", "selected_options": ["..."], "other_text": null}]}
    {"op": "respond_input",       "prompt_id": "...", "value": "..."}
    {"op": "respond_confirmation","prompt_id": "...", "confirmed": true, "feedback": null}
    {"op": "respond_selection",   "prompt_id": "...", "selected_index": 0, "selected_value": "..."}
    {"op": "set_agent", "name": "..."}
    {"op": "list_mcp_servers"}                       # -> mcp_servers event
    {"op": "set_mcp_enabled", "name": "...", "enabled": true}
    {"op": "add_mcp_server", "name": "...", "type": "stdio"|"sse"|"http", "config": {...}}
    {"op": "list_skills"}                            # -> skills event
    {"op": "get_skill", "name": "..."}               # -> skill_detail event
    {"op": "set_skill_enabled", "name": "...", "enabled": true}
    {"op": "save_skill", "name": "...", "description": "...", "content": "...",
        "scope": "user"|"project"}                   # create/overwrite SKILL.md
    {"op": "fs_list_dir", "id": <int>, "path": "..."}  # -> fs_result (remote file tree)
{"op": "fs_read_file", "id": <int>, "path": "..."} # -> fs_result (remote editor)
{"op": "git_run", "id": <int>, "root": "...", "args": [...]}  # -> git_result (remote git)
{"op": "shutdown"}

  sidecar -> Rust (stdout), one object per line:
    {"event": "ready",    "agent": "...", "model": "...", "cp_version": "...", "cwd": "..."}
    {"event": "agents",   "items": [{"name","display_name","description","current"}]}
    {"event": "models",   "items": [{"name","description","current"}]}
    {"event": "ask",      "id": "...", "questions": [{"header","question","multi_select","options":[{"label","description"}]}]}
    {"event": "commands", "items": [{"name","usage","description","category","aliases"}]}
    {"event": "message",  "source": "bus"|"legacy", "kind": "...",
                          "category": "...", "text": "...", "payload": {...}}
    {"event": "completions",  "id": <int>, "items": [{"text","start_position","display","meta"}]}
    {"event": "result",       "id": <int>, "output": "..."}
    {"event": "command_done", "id": <int>, "handled": true}
    {"event": "error",        "id": <int|null>, "message": "..."}
    {"event": "log",          "text": "..."}
    {"event": "mcp_servers",  "items": [{"id","name","type","enabled","state","summary","error"}]}
    {"event": "skills",       "items": [{"name","description","path","enabled","source"}]}
    {"event": "skill_detail", "name": "...", "description": "...", "path": "...", "content": "..."}

stdout is reserved exclusively for the protocol. Any stray library `print()` is
redirected to stderr so it can never corrupt a JSON line.
"""

import asyncio
import base64
import json
import os
import shutil
import subprocess
import sys
import threading
import traceback
import uuid
from typing import Any, Optional

# ---------------------------------------------------------------------------
# Protect the protocol channel.
# Keep the real stdout for JSON; route everything else (stray prints, Rich,
# warnings) to stderr where the GUI shows it as logs.
# ---------------------------------------------------------------------------
_REAL_STDOUT = sys.stdout
try:
    _REAL_STDOUT.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")
except Exception:
    pass
sys.stdout = sys.stderr

_write_lock = threading.Lock()


def send(obj: dict) -> None:
    """Write a single protocol object to the real stdout, atomically."""
    line = json.dumps(obj, ensure_ascii=False, default=_json_fallback)
    with _write_lock:
        _REAL_STDOUT.write(line + "\n")
        _REAL_STDOUT.flush()


def _json_fallback(value: Any) -> str:
    return str(value)


def log(text: str) -> None:
    send({"event": "log", "text": text})


# ---------------------------------------------------------------------------
# Message serialization
# ---------------------------------------------------------------------------
def _summarize(kind: str, payload: dict) -> str:
    """Best-effort human-readable text for a structured message."""
    for key in ("text", "content", "response", "reasoning", "summary",
                "line", "title", "prompt_text", "description"):
        val = payload.get(key)
        if isinstance(val, str) and val:
            return val
    if kind == "DiffMessage":
        return f"{payload.get('operation', 'modify')} {payload.get('path', '')}".strip()
    if kind == "FileListingMessage":
        return f"listed {payload.get('directory', '')} ({payload.get('file_count', 0)} files)"
    if kind == "ShellOutputMessage":
        return f"$ {payload.get('command', '')} (exit {payload.get('exit_code', '?')})"
    if kind == "FileContentMessage":
        return f"read {payload.get('path', '')}"
    if kind == "GrepResultMessage":
        return f"grep '{payload.get('search_term', '')}' -> {payload.get('total_matches', 0)} matches"
    return kind


def forward_bus_message(message: Any) -> None:
    """Serialize a structured MessageBus message and forward it."""
    try:
        payload = message.model_dump(mode="json")
    except Exception:
        payload = {"repr": repr(message)}
    kind = type(message).__name__
    send({
        "event": "message",
        "source": "bus",
        "kind": kind,
        "category": str(payload.get("category", "")),
        "text": _summarize(kind, payload),
        "payload": payload,
    })


def forward_legacy_message(message: Any) -> None:
    """Serialize a legacy UIMessage and forward it."""
    try:
        mtype = getattr(message.type, "value", str(message.type))
    except Exception:
        mtype = "unknown"
    content = getattr(message, "content", "")
    if not isinstance(content, str):
        content = str(content)
    metadata = getattr(message, "metadata", {}) or {}
    send({
        "event": "message",
        "source": "legacy",
        "kind": f"UIMessage:{mtype}",
        "category": mtype,
        "text": content,
        "payload": {"type": mtype, "content": content, "metadata": metadata},
    })


# ---------------------------------------------------------------------------
# Streaming "thinking" capture
# ---------------------------------------------------------------------------
def _make_stream_console(on_thinking):
    """A Rich-Console replacement set as Code Puppy's streaming console.

    Code Puppy renders the live token stream (THINKING banner + dim reasoning,
    then the AGENT RESPONSE) to this console. We suppress the terminal output and
    forward just the reasoning text to the GUI so a watching user can read the
    agent's thoughts and pause/steer. The final answer still arrives via `result`.
    """
    import io as _io

    from rich.console import Console as _Console
    from rich.text import Text as _Text

    class _StreamConsole(_Console):
        def __init__(self):
            super().__init__(file=_io.StringIO(), force_terminal=False,
                             color_system=None, width=120, soft_wrap=True)
            self._mode = None  # None | "thinking" | "response"

        def _plain(self, values):
            out = []
            for v in values:
                try:
                    if isinstance(v, str):
                        out.append(_Text.from_markup(v).plain)
                    elif isinstance(v, _Text):
                        out.append(v.plain)
                    else:
                        out.append(getattr(v, "plain", None) or str(v))
                except Exception:
                    out.append(str(v))
            return "".join(out)

        def print(self, *values, **kwargs):  # noqa: A003
            try:
                text = self._plain(values)
            except Exception:
                return
            s = text.strip()
            if not s:
                return
            up = s.upper()
            if len(s) < 48 and "THINKING" in up:
                self._mode = "thinking"
                return
            if len(s) < 48 and "AGENT RESPONSE" in up:
                self._mode = "response"
                return
            if self._mode == "thinking":
                try:
                    on_thinking(text)
                except Exception:
                    pass
            # response text + everything else is suppressed (answer comes via result)

    return _StreamConsole()


def _decode_image_attachments(images):
    """Decode base64 PNGs from the GUI into pydantic-ai BinaryContent parts.

    Returns None when there are no (valid) images so callers fall back to a
    plain text turn. Never raises - a bad attachment is logged and skipped.
    """
    if not images:
        return None
    try:
        from pydantic_ai import BinaryContent
    except Exception:
        log("pydantic_ai.BinaryContent unavailable; dropping image attachments")
        return None
    out = []
    for b64 in images:
        try:
            out.append(BinaryContent(data=base64.b64decode(b64), media_type="image/png"))
        except Exception:
            log("bad image attachment:\n" + traceback.format_exc())
    return out or None


# ---------------------------------------------------------------------------
# Bridge
# ---------------------------------------------------------------------------
class Bridge:
    def __init__(self) -> None:
        self.loop: Optional[asyncio.AbstractEventLoop] = None
        self.bus = None
        self.agent = None
        self.completer = None
        self.cp_version = "?"
        # Outstanding ask_user_question requests: id -> {"event", "data"}.
        self.pending_asks: dict = {}
        # The currently-running agent turn (for cancellation).
        self.current_run = None
        self._stop = threading.Event()

    # --- initialization ----------------------------------------------------
    def init_code_puppy(self) -> None:
        from code_puppy import __version__ as cp_version
        self.cp_version = cp_version
        from code_puppy.config import (
            ensure_config_exists,
            load_api_keys_to_environment,
        )
        from code_puppy.agents.agent_manager import get_current_agent
        from code_puppy.messaging import get_global_queue, get_message_bus

        # Importing the command handler triggers @register_command registration
        # for all built-in command modules (core/config/session/uc).
        import code_puppy.command_line.command_handler  # noqa: F401

        ensure_config_exists()
        load_api_keys_to_environment()

        # New structured bus: mark a renderer active so emit() flows to the
        # outgoing queue instead of being buffered, then poll it ourselves.
        self.bus = get_message_bus()
        self.bus.mark_renderer_active()
        # Allow cross-thread response futures to resolve on our loop.
        self.bus._event_loop = self.loop

        # Drain anything buffered before we attached.
        for msg in self.bus.get_buffered_messages():
            forward_bus_message(msg)
        self.bus.clear_buffer()

        # Legacy queue: attach a listener (also marks it renderer-active).
        legacy = get_global_queue()
        legacy.add_listener(forward_legacy_message)

        self.agent = get_current_agent()
        self.build_completer()
        self.install_ask_connector()
        self.install_stream_capture()

        self.emit_ready()
        self.emit_commands()
        self.emit_agents()
        self.emit_models()

    def emit_ready(self) -> None:
        """Announce (or re-announce) the active agent, model, version, and cwd."""
        model = None
        try:
            model = self.agent.get_model_name()
        except Exception:
            pass
        autosave = ""
        try:
            from code_puppy.config import get_current_autosave_session_name
            autosave = get_current_autosave_session_name()
        except Exception:
            pass
        puppy_name, owner_name = "Puppy", "Master"
        try:
            from code_puppy.config import get_owner_name, get_puppy_name
            puppy_name = get_puppy_name()
            owner_name = get_owner_name()
        except Exception:
            pass
        send({
            "event": "ready",
            "agent": getattr(self.agent, "name", "code-puppy"),
            "model": model or "(unset)",
            "cp_version": self.cp_version,
            "cwd": os.getcwd(),
            "autosave": autosave,
            "puppy_name": puppy_name,
            "owner_name": owner_name,
        })

    def emit_agents(self) -> None:
        """Send the catalog of available agents (with the current one flagged)."""
        try:
            from code_puppy.agents.agent_manager import (
                get_agent_descriptions,
                get_available_agents,
            )
            available = get_available_agents()       # {name: display_name}
            descriptions = get_agent_descriptions()   # {name: description}
        except Exception:
            log("agent enumeration failed:\n" + traceback.format_exc())
            return
        current = getattr(self.agent, "name", None)
        items = [
            {
                "name": name,
                "display_name": display,
                "description": descriptions.get(name, ""),
                "current": name == current,
            }
            for name, display in sorted(available.items(), key=lambda kv: kv[1].lower())
        ]
        send({"event": "agents", "items": items})

    def emit_models(self) -> None:
        """Send the catalog of available models (with the current one flagged)."""
        try:
            from code_puppy.config import get_global_model_name
            from code_puppy.model_factory import ModelFactory
            config = ModelFactory.load_config()       # {name: {..config..}}
            current = get_global_model_name()
        except Exception:
            log("model enumeration failed:\n" + traceback.format_exc())
            return
        items = []
        for name in sorted(config.keys(), key=str.lower):
            entry = config.get(name) or {}
            desc = ""
            if isinstance(entry, dict):
                desc = str(entry.get("description") or entry.get("type") or "")
            items.append({"name": name, "description": desc, "current": name == current})
        send({"event": "models", "items": items})

    def set_model(self, name: str) -> None:
        """Switch the active model and reload the agent, then re-announce."""
        if not name:
            return
        try:
            from code_puppy.model_switching import set_model_and_reload_agent
            set_model_and_reload_agent(name)
        except Exception as exc:
            send({"event": "error", "id": None,
                  "message": f"set_model failed: {type(exc).__name__}: {exc}"})
            log(traceback.format_exc())
            return
        from code_puppy.agents.agent_manager import get_current_agent
        self.agent = get_current_agent()
        self.emit_ready()
        self.emit_models()

    def emit_status(self) -> None:
        """Snapshot live run metrics: conversation stats + concurrent sub-agents.

        Both sources are best-effort — Code Puppy only tracks sub-agents that
        were spawned via ``invoke_agent``; an idle session reports neither.
        """
        stats = ""
        try:
            from code_puppy.agents.run_stats import AgentRunStats
            avg_ttft, avg_gen = AgentRunStats.get_conversation_stats()
            stats = AgentRunStats.format_conversation_stats(avg_ttft, avg_gen) or ""
        except Exception:
            pass

        token_rate = 0.0
        try:
            from code_puppy import status_display
            token_rate = float(getattr(status_display, "CURRENT_TOKEN_RATE", 0.0) or 0.0)
        except Exception:
            pass

        sub_agents = []
        try:
            from code_puppy.messaging.subagent_console import SubAgentConsoleManager
            for a in SubAgentConsoleManager.get_instance().get_all_agents():
                sub_agents.append({
                    "agent_name": a.agent_name,
                    "model_name": a.model_name,
                    "status": a.status,
                    "tool_call_count": int(a.tool_call_count),
                    "token_count": int(a.token_count),
                    "current_tool": a.current_tool,
                    "elapsed": float(a.elapsed_seconds()),
                })
        except Exception:
            pass

        send({
            "event": "status",
            "stats": stats,
            "token_rate": token_rate,
            "sub_agents": sub_agents,
        })

    # --- MCP servers (Code Puppy's MCPManager) ------------------------------

    def _mcp_manager(self):
        """Code Puppy's singleton MCP manager (the same one /mcp drives)."""
        from code_puppy.mcp_.manager import get_mcp_manager
        return get_mcp_manager()

    @staticmethod
    def _mcp_summary(server_type: str, raw: dict) -> str:
        """One-line config summary: the command line (stdio) or the URL."""
        if server_type == "stdio":
            command = str(raw.get("command", "") or "")
            args = raw.get("args") or []
            if isinstance(args, list):
                args = " ".join(str(a) for a in args)
            return f"{command} {args}".strip()
        return str(raw.get("url", "") or "")

    def emit_mcp_servers(self) -> None:
        """List registered MCP servers with live status + a config summary."""
        items = []
        try:
            manager = self._mcp_manager()
            for info in manager.list_servers():
                summary = ""
                try:
                    conf = manager.get_server_by_name(info.name)
                    summary = self._mcp_summary(
                        info.type, conf.config if conf else {})
                except Exception:
                    pass
                items.append({
                    "id": info.id,
                    "name": info.name,
                    "type": info.type,
                    "enabled": bool(info.enabled),
                    "state": getattr(info.state, "value", str(info.state)),
                    "summary": summary,
                    "error": info.error_message or "",
                })
        except Exception:
            log("mcp enumeration failed:\n" + traceback.format_exc())
        send({"event": "mcp_servers", "items": items})

    def set_mcp_enabled(self, name: str, enabled: bool) -> None:
        """Start/stop one MCP server by name (the /mcp start/stop path)."""
        try:
            manager = self._mcp_manager()
            conf = manager.get_server_by_name(name)
            if conf is None:
                send({"event": "error", "id": None,
                      "message": f"unknown MCP server: {name}"})
                return

            # start/stop_server_sync schedule the real work as a background
            # task on the running loop, so hop onto the loop thread first.
            def toggle() -> None:
                try:
                    if enabled:
                        manager.start_server_sync(conf.id)
                    else:
                        manager.stop_server_sync(conf.id)
                except Exception as exc:
                    send({"event": "error", "id": None,
                          "message": f"set_mcp_enabled failed: "
                                     f"{type(exc).__name__}: {exc}"})
                self.emit_mcp_servers()
                # The process starts/stops asynchronously; re-announce once
                # the dust has had a moment to settle.
                self.loop.call_later(1.5, self.emit_mcp_servers)

            self.loop.call_soon_threadsafe(toggle)
        except Exception as exc:
            send({"event": "error", "id": None,
                  "message": f"set_mcp_enabled failed: {type(exc).__name__}: {exc}"})
            log(traceback.format_exc())

    def add_mcp_server(self, cmd: dict) -> None:
        """Register a new MCP server (registry + mcp_servers.json), then re-list."""
        name = str(cmd.get("name") or "").strip()
        server_type = str(cmd.get("type") or "").strip().lower()
        config = cmd.get("config")
        if not name:
            send({"event": "error", "id": None,
                  "message": "add_mcp_server: a server name is required"})
            return
        if server_type not in ("stdio", "sse", "http"):
            send({"event": "error", "id": None,
                  "message": f"add_mcp_server: invalid type {server_type!r} "
                             "(expected stdio, sse, or http)"})
            return
        if not isinstance(config, dict):
            send({"event": "error", "id": None,
                  "message": "add_mcp_server: 'config' must be an object"})
            return
        try:
            from code_puppy.mcp_.managed_server import ServerConfig
            manager = self._mcp_manager()
            # register_server validates (name shape, required url/command, ...)
            # and raises ValueError with a readable message on bad input.
            manager.register_server(ServerConfig(
                id="", name=name, type=server_type, enabled=True,
                config=dict(config)))
            self._persist_mcp_config(name, server_type, dict(config))
        except Exception as exc:
            send({"event": "error", "id": None,
                  "message": f"add_mcp_server failed: {exc}"})
            log(traceback.format_exc())
            return
        self.emit_mcp_servers()

    @staticmethod
    def _persist_mcp_config(name: str, server_type: str, config: dict) -> None:
        """Mirror a new server into mcp_servers.json (the CLI does the same)."""
        from code_puppy.config import MCP_SERVERS_FILE
        data = {"mcp_servers": {}}
        if os.path.exists(MCP_SERVERS_FILE):
            try:
                with open(MCP_SERVERS_FILE, "r", encoding="utf-8") as f:
                    data = json.load(f)
            except Exception:
                pass
        servers = data.setdefault("mcp_servers", {})
        entry = dict(config)
        entry["type"] = server_type
        servers[name] = entry
        os.makedirs(os.path.dirname(MCP_SERVERS_FILE), exist_ok=True)
        with open(MCP_SERVERS_FILE, "w", encoding="utf-8") as f:
            json.dump(data, f, indent=2)

    # --- Skills (Code Puppy's agent_skills plugin) ---------------------------

    @staticmethod
    def _classify_skill_source(path) -> str:
        """user / plugin / project / other, by where the skill dir lives."""
        from pathlib import Path
        from code_puppy.config import CACHE_DIR
        p = Path(path).resolve()
        roots = (
            ("user", Path.home() / ".code_puppy" / "skills"),
            ("plugin", Path(CACHE_DIR) / "plugin-skills"),
            ("project", Path.cwd()),
        )
        for label, root in roots:
            try:
                p.relative_to(root.resolve())
                return label
            except (ValueError, OSError):
                continue
        return "other"

    def emit_skills(self) -> None:
        """List discovered skills with frontmatter metadata + enabled flag.

        Reuses Code Puppy's own machinery (the same path /skills walks):
        discovery.discover_skills + metadata.parse_skill_metadata + the
        disabled_skills config set. Skill dirs without a SKILL.md are
        skipped - they can't be activated anyway.
        """
        items = []
        try:
            from code_puppy.plugins.agent_skills import config as sk_config
            from code_puppy.plugins.agent_skills import discovery, metadata
            disabled = sk_config.get_disabled_skills()
            for info in discovery.discover_skills():
                if not info.has_skill_md:
                    continue
                meta = metadata.parse_skill_metadata(info.path)
                items.append({
                    "name": info.name,
                    "description": meta.description if meta else "",
                    "path": str(info.path),
                    "enabled": info.name not in disabled,
                    "source": self._classify_skill_source(info.path),
                })
        except Exception:
            log("skills enumeration failed:\n" + traceback.format_exc())
        send({"event": "skills", "items": items})

    def get_skill(self, name: str) -> None:
        """Send one skill's full SKILL.md text (skill_detail event)."""
        try:
            from code_puppy.plugins.agent_skills import discovery, metadata
            info = next(
                (i for i in discovery.discover_skills()
                 if i.name == name and i.has_skill_md),
                None,
            )
            if info is None:
                send({"event": "error", "id": None,
                      "message": f"unknown skill: {name}"})
                return
            meta = metadata.parse_skill_metadata(info.path)
            send({
                "event": "skill_detail",
                "name": info.name,
                "description": meta.description if meta else "",
                "path": str(info.path),
                "content": metadata.load_full_skill_content(info.path) or "",
            })
        except Exception as exc:
            send({"event": "error", "id": None,
                  "message": f"get_skill failed: {type(exc).__name__}: {exc}"})
            log(traceback.format_exc())

    def set_skill_enabled(self, name: str, enabled: bool) -> None:
        """Enable/disable one skill (Code Puppy's disabled_skills config)."""
        try:
            from code_puppy.plugins.agent_skills import config as sk_config
            sk_config.set_skill_disabled(name, not enabled)
        except Exception as exc:
            send({"event": "error", "id": None,
                  "message": f"set_skill_enabled failed: "
                             f"{type(exc).__name__}: {exc}"})
            log(traceback.format_exc())
        self.emit_skills()

    @staticmethod
    def _valid_skill_name(name: str) -> bool:
        """Alphanumeric plus - and _ only: blocks path traversal by shape."""
        return bool(name) and all(c.isalnum() or c in "-_" for c in name)

    @staticmethod
    def _compose_skill_md(name: str, description: str, body: str) -> str:
        """Frontmatter + body - unless the body is already a full document."""
        if body.lstrip().startswith("---"):
            return body
        head = "\n".join(
            ["---", f"name: {name}", f"description: {description}", "---"])
        return head + "\n\n" + body.rstrip() + "\n"

    def save_skill(self, cmd: dict) -> None:
        """Create or overwrite <skills dir>/<name>/SKILL.md, then re-list.

        scope "user" -> ~/.code_puppy/skills, "project" -> ./.code_puppy/skills
        (both are default discovery directories, so the new skill is live
        immediately).
        """
        from pathlib import Path
        name = str(cmd.get("name") or "").strip()
        description = str(cmd.get("description") or "").strip()
        content = str(cmd.get("content") or "")
        scope = str(cmd.get("scope") or "user").strip().lower()
        if not self._valid_skill_name(name):
            send({"event": "error", "id": None,
                  "message": "save_skill: name must be alphanumeric "
                             "(hyphens and underscores allowed)"})
            return
        if scope not in ("user", "project"):
            send({"event": "error", "id": None,
                  "message": f"save_skill: invalid scope {scope!r} "
                             "(expected user or project)"})
            return
        if scope == "user":
            base = Path.home() / ".code_puppy" / "skills"
        else:
            base = Path.cwd() / ".code_puppy" / "skills"
        try:
            skill_dir = base / name
            skill_dir.mkdir(parents=True, exist_ok=True)
            (skill_dir / "SKILL.md").write_text(
                self._compose_skill_md(name, description, content),
                encoding="utf-8",
            )
            from code_puppy.plugins.agent_skills import discovery
            discovery.refresh_skill_cache()
        except Exception as exc:
            send({"event": "error", "id": None,
                  "message": f"save_skill failed: {type(exc).__name__}: {exc}"})
            log(traceback.format_exc())
            return
        self.emit_skills()

    # --- Agent configs (Code Puppy JSON agents) -----------------------------

    @staticmethod
    def _valid_agent_name(name: str) -> bool:
        """Alphanumeric plus - and _ only: blocks path traversal by shape."""
        return bool(name) and all(c.isalnum() or c in "-_" for c in name)

    @staticmethod
    def _discover_json_paths() -> dict:
        """Map agent name -> JSON file path for editable JSON agents."""
        try:
            from code_puppy.agents.json_agent import discover_json_agents
            return discover_json_agents()
        except Exception:
            log("json agent discovery failed:\n" + traceback.format_exc())
            return {}

    @staticmethod
    def _classify_agent_source(path) -> str:
        """user / project, by which agents directory the JSON file lives in."""
        from pathlib import Path
        from code_puppy.config import (
            get_project_agents_directory,
            get_user_agents_directory,
        )
        p = Path(path).resolve()
        try:
            if p.parent == Path(get_user_agents_directory()).resolve():
                return "user"
        except Exception:
            pass
        proj = get_project_agents_directory()
        if proj:
            try:
                if p.parent == Path(proj).resolve():
                    return "project"
            except Exception:
                pass
        return "user"

    @staticmethod
    def _available_tool_names() -> list:
        """Sorted list of built-in tool names an agent may enable."""
        try:
            from code_puppy.tools import get_available_tool_names
            return sorted(get_available_tool_names())
        except Exception:
            return []

    def _available_mcp_names(self) -> list:
        """Sorted list of registered MCP server names (for bindings)."""
        try:
            return sorted(i.name for i in self._mcp_manager().list_servers())
        except Exception:
            return []

    def emit_agent_configs(self) -> None:
        """Send the agent catalog (JSON-editable + read-only built-ins)."""
        items = []
        try:
            from code_puppy.agents.agent_manager import (
                get_agent_descriptions,
                get_available_agents,
                refresh_agents,
            )
            refresh_agents()
            available = get_available_agents()      # {name: display_name}
            descriptions = get_agent_descriptions()
            json_paths = self._discover_json_paths()
            current = getattr(self.agent, "name", None)
            for name, display in sorted(
                    available.items(), key=lambda kv: kv[1].lower()):
                path = json_paths.get(name)
                model = ""
                tool_count = 0
                if path:
                    try:
                        with open(path, "r", encoding="utf-8") as f:
                            cfg = json.load(f)
                        model = str(cfg.get("model") or "")
                        tools = cfg.get("tools")
                        tool_count = len(tools) if isinstance(tools, list) else 0
                    except Exception:
                        pass
                items.append({
                    "name": name,
                    "display_name": display,
                    "description": descriptions.get(name, ""),
                    "model": model,
                    "tool_count": tool_count,
                    "source": self._classify_agent_source(path) if path else "builtin",
                    "editable": path is not None,
                    "path": path or "",
                    "current": name == current,
                })
        except Exception:
            log("agent config enumeration failed:\n" + traceback.format_exc())
        send({
            "event": "agent_configs",
            "items": items,
            "available_tools": self._available_tool_names(),
            "available_mcp": self._available_mcp_names(),
        })

    def get_agent_config(self, name: str) -> None:
        """Send one agent's full config (agent_config event).

        Editable JSON agents are read straight off disk; built-in (Python)
        agents are instantiated and their authored fields surfaced read-only.
        """
        name = str(name or "").strip()
        try:
            from code_puppy.agents.agent_manager import (
                _AGENT_REGISTRY,
                refresh_agents,
            )
            refresh_agents()
            ref = _AGENT_REGISTRY.get(name)
            if ref is None:
                send({"event": "error", "id": None,
                      "message": f"unknown agent: {name}"})
                return
            path = self._discover_json_paths().get(name)
            if path:
                with open(path, "r", encoding="utf-8") as f:
                    cfg = json.load(f)
                source = self._classify_agent_source(path)
                editable = True
            else:
                inst = ref() if not isinstance(ref, str) else None
                cfg = {
                    "name": name,
                    "display_name": getattr(inst, "display_name", name),
                    "description": getattr(inst, "description", ""),
                    "system_prompt": inst.get_system_prompt() if inst else "",
                    "tools": inst.get_available_tools() if inst else [],
                }
                source = "builtin"
                editable = False
            system_prompt = cfg.get("system_prompt", "")
            if isinstance(system_prompt, list):
                system_prompt = "\n".join(str(s) for s in system_prompt)
            raw_tools = cfg.get("tools")
            tools = [str(t) for t in raw_tools] if isinstance(raw_tools, list) else []
            raw_mcp = cfg.get("mcp_servers")
            if isinstance(raw_mcp, list):
                mcp_servers = [str(s) for s in raw_mcp]
            elif isinstance(raw_mcp, dict):
                mcp_servers = [str(s) for s in raw_mcp.keys()]
            else:
                mcp_servers = []
            user_prompt = cfg.get("user_prompt")
            send({
                "event": "agent_config",
                "name": name,
                "display_name": str(cfg.get("display_name") or ""),
                "description": str(cfg.get("description") or ""),
                "system_prompt": str(system_prompt or ""),
                "user_prompt": user_prompt if user_prompt is None else str(user_prompt),
                "model": str(cfg.get("model") or ""),
                "tools": tools,
                "mcp_servers": mcp_servers,
                "editable": editable,
                "source": source,
                "path": path or "",
                "content": json.dumps(cfg, indent=2, ensure_ascii=False),
            })
        except Exception as exc:
            send({"event": "error", "id": None,
                  "message": f"get_agent_config failed: "
                             f"{type(exc).__name__}: {exc}"})
            log(traceback.format_exc())

    def save_agent_config(self, cmd: dict) -> None:
        """Create or overwrite <agents dir>/<name>.json, then re-list.

        scope "user" -> ~/.code_puppy/agents, "project" -> ./.code_puppy/agents
        (both are discovery directories, so the agent is live immediately).
        """
        from pathlib import Path
        name = str(cmd.get("name") or "").strip()
        if not self._valid_agent_name(name):
            send({"event": "error", "id": None,
                  "message": "save_agent_config: name must be alphanumeric "
                             "(hyphens and underscores allowed)"})
            return
        scope = str(cmd.get("scope") or "user").strip().lower()
        if scope not in ("user", "project"):
            send({"event": "error", "id": None,
                  "message": f"save_agent_config: invalid scope {scope!r} "
                             "(expected user or project)"})
            return
        description = str(cmd.get("description") or "").strip()
        if not description:
            send({"event": "error", "id": None,
                  "message": "save_agent_config: a description is required"})
            return
        tools = cmd.get("tools")
        if not isinstance(tools, list):
            tools = []
        config = {
            "name": name,
            "description": description,
            "system_prompt": str(cmd.get("system_prompt") or ""),
            "tools": [str(t) for t in tools],
        }
        display_name = str(cmd.get("display_name") or "").strip()
        if display_name:
            config["display_name"] = display_name
        model = str(cmd.get("model") or "").strip()
        if model:
            config["model"] = model
        user_prompt = cmd.get("user_prompt")
        if user_prompt is not None and str(user_prompt).strip():
            config["user_prompt"] = str(user_prompt)
        mcp = cmd.get("mcp_servers")
        if isinstance(mcp, list) and mcp:
            config["mcp_servers"] = [str(s) for s in mcp]
        try:
            from code_puppy.config import get_user_agents_directory
            if scope == "user":
                base = Path(get_user_agents_directory())
            else:
                base = Path.cwd() / ".code_puppy" / "agents"
            base.mkdir(parents=True, exist_ok=True)
            (base / f"{name}.json").write_text(
                json.dumps(config, indent=2, ensure_ascii=False),
                encoding="utf-8",
            )
            from code_puppy.agents.agent_manager import refresh_agents
            refresh_agents()
        except Exception as exc:
            send({"event": "error", "id": None,
                  "message": f"save_agent_config failed: "
                             f"{type(exc).__name__}: {exc}"})
            log(traceback.format_exc())
            return
        self.emit_agent_configs()

    def delete_agent_config(self, name: str) -> None:
        """Delete a JSON agent file (user/project only), then re-list."""
        from pathlib import Path
        name = str(name or "").strip()
        path = self._discover_json_paths().get(name)
        if not path:
            send({"event": "error", "id": None,
                  "message": f"delete_agent_config: {name!r} is not an "
                             "editable JSON agent"})
            return
        if getattr(self.agent, "name", None) == name:
            send({"event": "error", "id": None,
                  "message": "delete_agent_config: cannot delete the active "
                             "agent; switch agents first"})
            return
        try:
            Path(path).unlink()
            from code_puppy.agents.agent_manager import (
                _AGENT_REGISTRY,
                refresh_agents,
            )
            _AGENT_REGISTRY.pop(name, None)
            refresh_agents()
        except Exception as exc:
            send({"event": "error", "id": None,
                  "message": f"delete_agent_config failed: "
                             f"{type(exc).__name__}: {exc}"})
            log(traceback.format_exc())
            return
        self.emit_agent_configs()

    def clone_agent_config(self, name: str) -> None:
        """Clone an agent (built-in or JSON) into a user JSON copy, re-list."""
        name = str(name or "").strip()
        try:
            from code_puppy.agents.agent_manager import (
                clone_agent,
                refresh_agents,
            )
            new_name = clone_agent(name)
            refresh_agents()
            if not new_name:
                send({"event": "error", "id": None,
                      "message": f"clone_agent_config: clone failed for {name!r}"})
        except Exception as exc:
            send({"event": "error", "id": None,
                  "message": f"clone_agent_config failed: "
                             f"{type(exc).__name__}: {exc}"})
            log(traceback.format_exc())
        self.emit_agent_configs()

    # --- Code Puppy sessions (autosave + named contexts) --------------------

    def set_puppy_name(self, name: str) -> None:
        """Rename the puppy (global Code Puppy config), then re-announce."""
        name = (name or "").strip()
        if not name:
            return
        try:
            from code_puppy.config import set_config_value
            set_config_value("puppy_name", name)
        except Exception:
            log("set_puppy_name failed:\n" + traceback.format_exc())
            return
        self.emit_ready()

    def emit_sessions(self, open_picker: bool = False) -> None:
        """List saved Code Puppy sessions (autosave + named contexts) + metadata."""
        import json as _json
        import pathlib
        items = []
        try:
            from code_puppy.session_storage import list_sessions
            from code_puppy.config import AUTOSAVE_DIR, CONTEXTS_DIR
            sources = [("autosave", AUTOSAVE_DIR), ("context", CONTEXTS_DIR)]
            for source, base in sources:
                base_p = pathlib.Path(base)
                for name in list_sessions(base_p):
                    meta = {}
                    try:
                        meta = _json.loads(
                            (base_p / f"{name}_meta.json").read_text(encoding="utf-8")
                        )
                    except Exception:
                        pass
                    items.append({
                        "name": name,
                        "source": source,
                        "timestamp": meta.get("timestamp", ""),
                        "messages": int(meta.get("message_count", 0) or 0),
                        "tokens": int(meta.get("total_tokens", 0) or 0),
                    })
        except Exception:
            log("session enumeration failed:\n" + traceback.format_exc())
        items.sort(key=lambda x: x.get("timestamp", ""), reverse=True)
        current = ""
        try:
            from code_puppy.config import get_current_autosave_session_name
            current = get_current_autosave_session_name()
        except Exception:
            pass
        send({"event": "sessions", "items": items, "current": current,
              "open": bool(open_picker)})

    def _history_to_entries(self, history) -> list:
        """Flatten pydantic-ai message history into {role,text} transcript rows."""
        out = []
        for msg in history or []:
            for part in getattr(msg, "parts", None) or []:
                kind = type(part).__name__
                content = getattr(part, "content", None)
                if not isinstance(content, str) or not content.strip():
                    continue
                if kind == "UserPromptPart":
                    out.append({"role": "user", "text": content})
                elif kind == "TextPart":
                    out.append({"role": "agent", "text": content})
        return out

    def preview_session(self, name: str, source: str) -> None:
        """Read a saved session's history from disk and emit it for previewing,
        WITHOUT loading it into the agent or touching the autosave id."""
        if not name:
            return
        import pathlib
        try:
            from code_puppy.session_storage import load_session as _load
            from code_puppy.config import AUTOSAVE_DIR, CONTEXTS_DIR
            base = pathlib.Path(AUTOSAVE_DIR if source == "autosave" else CONTEXTS_DIR)
            history = _load(name, base)
        except Exception as exc:
            send({"event": "error", "id": None,
                  "message": f"preview failed: {type(exc).__name__}: {exc}"})
            return
        send({
            "event": "session_preview",
            "name": name,
            "source": source,
            "messages": len(history) if history else 0,
            "entries": self._history_to_entries(history),
        })

    def load_session(self, name: str, source: str) -> None:
        """Load a saved session into the agent + (autosave) reattach its id."""
        if not name:
            return
        import pathlib
        try:
            from code_puppy.session_storage import load_session as _load
            from code_puppy.config import (
                AUTOSAVE_DIR,
                CONTEXTS_DIR,
                rotate_autosave_id,
                set_current_autosave_from_session_name,
            )
            base = pathlib.Path(AUTOSAVE_DIR if source == "autosave" else CONTEXTS_DIR)
            history = _load(name, base)
        except FileNotFoundError:
            send({"event": "error", "id": None, "message": f"session not found: {name}"})
            return
        except Exception as exc:
            send({"event": "error", "id": None,
                  "message": f"load_session failed: {type(exc).__name__}: {exc}"})
            log(traceback.format_exc())
            return
        try:
            self.agent.set_message_history(history)
            self._sanitize_history()  # repair any orphaned tool pairs in the saved file
        except Exception:
            log("set_message_history failed:\n" + traceback.format_exc())
        try:
            if source == "autosave":
                set_current_autosave_from_session_name(name)
            else:
                rotate_autosave_id()
        except Exception:
            pass
        send({
            "event": "session_loaded",
            "name": name,
            "messages": len(history) if history else 0,
            "entries": self._history_to_entries(history),
        })
        self.emit_ready()

    # --- pause / steer (Code Puppy's PauseController is thread-safe) ---------

    def pause_agent(self) -> None:
        """Pause the running turn at the next safe boundary."""
        try:
            from code_puppy.messaging.pause_controller import get_pause_controller
            get_pause_controller().pause()
            send({"event": "paused", "paused": True})
        except Exception:
            log("pause failed:\n" + traceback.format_exc())

    def resume_agent(self) -> None:
        """Resume a paused turn."""
        try:
            from code_puppy.messaging.pause_controller import get_pause_controller
            get_pause_controller().resume()
            send({"event": "paused", "paused": False})
        except Exception:
            log("resume failed:\n" + traceback.format_exc())

    def steer_agent(self, text: str, mode: str) -> None:
        """Inject a steering message: ``now`` (mid-turn) or ``queue`` (next turn)."""
        text = (text or "").strip()
        if not text:
            return
        if mode not in ("now", "queue"):
            mode = "now"
        try:
            from code_puppy.messaging.pause_controller import get_pause_controller
            get_pause_controller().request_steer(text, mode=mode)
        except Exception:
            log("steer failed:\n" + traceback.format_exc())

    def emit_commands(self) -> None:
        """Send the catalog of available slash commands for the GUI menu."""
        from code_puppy.command_line.command_registry import get_unique_commands

        items = []
        for c in get_unique_commands():
            items.append({
                "name": c.name,
                "usage": c.usage,
                "description": c.description,
                "category": c.category,
                "aliases": list(c.aliases),
            })

        # Plugin/custom commands (mirror what the CLI's /help surfaces), so the
        # menu and autocomplete show everything — not just @register_command.
        try:
            from code_puppy import callbacks, plugins
            plugins.load_plugin_callbacks()
            for res in callbacks.on_custom_command_help():
                if not res:
                    continue
                entries = res if isinstance(res, list) else [res]
                for item in entries:
                    if isinstance(item, tuple) and len(item) == 2:
                        name = str(item[0]).lstrip("/")
                        items.append({
                            "name": name,
                            "usage": f"/{name}",
                            "description": str(item[1]),
                            "category": "custom",
                            "aliases": [],
                        })
        except Exception:
            log("custom command enumeration failed:\n" + traceback.format_exc())

        items.sort(key=lambda i: (i["category"], i["name"]))
        send({"event": "commands", "items": items})

    def build_completer(self) -> None:
        """Assemble Code Puppy's real prompt_toolkit completers so the GUI gets
        identical, always-in-sync completion (commands, agents, models, config
        keys, @file paths) — without a terminal."""
        from prompt_toolkit.completion import merge_completers

        completers = []

        def add(make) -> None:
            try:
                completers.append(make())
            except Exception:
                log("completer init skipped:\n" + traceback.format_exc())

        from code_puppy.command_line.file_path_completion import FilePathCompleter
        from code_puppy.command_line.model_picker_completion import ModelNameCompleter
        from code_puppy.command_line.load_context_completion import LoadContextCompleter
        from code_puppy.command_line.pin_command_completion import (
            PinCompleter,
            UnpinCompleter,
        )
        from code_puppy.command_line.mcp_completion import MCPCompleter
        from code_puppy.command_line.skills_completion import SkillsCompleter
        from code_puppy.command_line.prompt_toolkit_completion import (
            AgentCompleter,
            CDCompleter,
            SetCompleter,
            SlashCompleter,
        )

        add(lambda: FilePathCompleter(symbol="@"))
        add(lambda: ModelNameCompleter(trigger="/model"))
        add(lambda: ModelNameCompleter(trigger="/m"))
        add(lambda: CDCompleter(trigger="/cd"))
        add(lambda: SetCompleter(trigger="/set"))
        add(lambda: LoadContextCompleter(trigger="/load_context"))
        add(lambda: PinCompleter(trigger="/pin_model"))
        add(lambda: UnpinCompleter(trigger="/unpin"))
        for trig in ("/agent", "/a", "/switch-agent", "/sa"):
            add(lambda t=trig: AgentCompleter(trigger=t))
        add(lambda: MCPCompleter(trigger="/mcp"))
        add(lambda: SkillsCompleter(trigger="/skills"))
        try:
            from code_puppy.plugins.ollama_setup.completer import OllamaSetupCompleter
            completers.append(OllamaSetupCompleter())
        except Exception:
            pass
        add(lambda: SlashCompleter())

        self.completer = merge_completers(completers)

    def complete(self, msg_id: int, text: str, cursor: int) -> None:
        """Return completions for `text` with the caret at char index `cursor`."""
        items = []
        if self.completer is not None:
            try:
                from prompt_toolkit.completion import CompleteEvent
                from prompt_toolkit.document import Document

                doc = Document(text, cursor_position=cursor)
                event = CompleteEvent(completion_requested=True)
                for c in self.completer.get_completions(doc, event):
                    items.append({
                        "text": c.text,
                        "start_position": c.start_position,
                        "display": c.display_text,
                        "meta": c.display_meta_text,
                    })
                    if len(items) >= 80:
                        break
            except Exception:
                log("completion failed:\n" + traceback.format_exc())
        send({"event": "completions", "id": msg_id, "items": items})

    # --- interactive questions connector ----------------------------------
    def _emit_thinking(self, text: str) -> None:
        """Forward a chunk of the agent's reasoning stream to the GUI."""
        if not text:
            return
        send({
            "event": "message",
            "source": "stream",
            "kind": "agent_reasoning",
            "category": "thinking",
            "text": text,
            "payload": {},
        })

    def install_stream_capture(self) -> None:
        """Capture Code Puppy's live token stream so the GUI can show the agent's
        thinking. Without this the stream renders to a (hidden) console."""
        try:
            from code_puppy.agents.event_stream_handler import set_streaming_console
            set_streaming_console(_make_stream_console(self._emit_thinking))
            log("stream capture installed")
        except Exception:
            log("stream capture install failed:\n" + traceback.format_exc())

    def install_ask_connector(self) -> None:
        """Route Code Puppy's `ask_user_question` tool to the GUI instead of its
        terminal TUI (which needs a real TTY we don't have). The registered tool
        resolves `_ask_user_question_impl` from the module at call time, so
        replacing it here takes effect for every invocation."""
        try:
            import code_puppy.tools.ask_user_question.registration as reg
            reg._ask_user_question_impl = self._gui_ask
            log("ask_user_question connector installed")
        except Exception:
            log("ask connector install failed:\n" + traceback.format_exc())

    def _gui_ask(self, questions: Any, timeout: int = 600):
        """Replacement for ask_user_question's impl: emit an `ask` request to the
        GUI and block (on this tool-call's thread) until it answers."""
        from code_puppy.tools.ask_user_question.models import (
            AskUserQuestionInput,
            AskUserQuestionOutput,
            QuestionAnswer,
        )
        try:
            validated = AskUserQuestionInput.model_validate({"questions": questions})
        except Exception as exc:
            return AskUserQuestionOutput.error_response(f"invalid questions: {exc}")

        payload = [
            {
                "header": q.header,
                "question": q.question,
                "multi_select": q.multi_select,
                "options": [
                    {"label": o.label, "description": o.description} for o in q.options
                ],
            }
            for q in validated.questions
        ]

        req_id = str(uuid.uuid4())
        event = threading.Event()
        self.pending_asks[req_id] = {"event": event, "data": None}
        send({"event": "ask", "id": req_id, "questions": payload})

        signaled = event.wait(timeout=timeout)
        entry = self.pending_asks.pop(req_id, None)
        if not signaled or entry is None or entry["data"] is None:
            return AskUserQuestionOutput.cancelled_response()

        data = entry["data"]
        if data.get("cancelled"):
            return AskUserQuestionOutput.cancelled_response()

        answers = [
            QuestionAnswer(
                question_header=a.get("question_header", ""),
                selected_options=list(a.get("selected_options") or []),
                other_text=a.get("other_text"),
            )
            for a in data.get("answers", [])
        ]
        return AskUserQuestionOutput(answers=answers)

    def resolve_ask(self, cmd: dict) -> None:
        entry = self.pending_asks.get(cmd.get("id"))
        if entry is not None:
            entry["data"] = cmd
            entry["event"].set()

    # --- background pollers -------------------------------------------------
    def start_bus_poller(self) -> None:
        def poll() -> None:
            while not self._stop.is_set():
                msg = self.bus.get_message_nowait()
                if msg is None:
                    self._stop.wait(0.01)
                    continue
                forward_bus_message(msg)
        threading.Thread(target=poll, name="bus-poller", daemon=True).start()

    # --- command handling --------------------------------------------------
    def _sanitize_history(self) -> None:
        """Drop orphaned tool_call/tool_result pairs from the agent's history.

        A history with a tool_result whose tool_use is missing (e.g. from a
        cancelled tool call or a resumed/auto-saved partial turn) makes the model
        reject the request with a 400. Code Puppy ships the repair; we apply it
        before each run, on session load, and before autosave."""
        try:
            from code_puppy.agents._history import (
                prune_interrupted_tool_calls,
                sanitize_tool_call_ids,
            )
            hist = self.agent.get_message_history()
            if not hist:
                return
            cleaned = sanitize_tool_call_ids(prune_interrupted_tool_calls(hist))
            if cleaned is not hist or len(cleaned) != len(hist):
                self.agent.set_message_history(cleaned)
                if len(cleaned) != len(hist):
                    log(f"pruned interrupted tool calls: {len(hist)} -> {len(cleaned)} msgs")
        except Exception:
            log("history sanitize failed:\n" + traceback.format_exc())

    def _autosave(self) -> None:
        """Persist the current conversation to its autosave session file.

        The CLI does this after every turn from its own loop; the sidecar runs
        the agent directly (bypassing that loop), so we must trigger it here or
        conversations would never be saved. Silent (no chat 🐾 line) + best-effort.
        """
        try:
            from code_puppy.config import (
                AUTOSAVE_DIR,
                get_auto_save_session,
                get_current_autosave_session_name,
            )
            if not get_auto_save_session():
                return
            self._sanitize_history()  # don't persist a broken history
            history = self.agent.get_message_history()
            if not history:
                return
            import datetime
            import pathlib
            from code_puppy.session_storage import save_session
            save_session(
                history=history,
                session_name=get_current_autosave_session_name(),
                base_dir=pathlib.Path(AUTOSAVE_DIR),
                timestamp=datetime.datetime.now().isoformat(),
                token_estimator=self.agent.estimate_tokens_for_message,
                auto_saved=True,
            )
        except Exception:
            log("autosave failed:\n" + traceback.format_exc())

    # Words that signal the user's turn is about the in-app browser/page.
    _BROWSER_HINTS = (
        "browser", "console", "devtools", "dev tools", "cdp", "dom", "inspect",
        "the page", "web page", "webpage", "the site", "the app", "localhost",
        "screenshot", "javascript", "front-end", "frontend", "network tab",
        "rendered", "rendering",
    )

    def _browser_context(self, user_text: str) -> str:
        """If the in-app browser plugin is open, the host drops a breadcrumb at
        ``.puppy/browser.json`` (in our cwd) with a live Chrome DevTools Protocol
        endpoint. Surface it to the agent so prompts like "check my app's console"
        Just Work — no need for the user to paste the endpoint. Only injected when
        the turn looks browser-related, so it never pollutes unrelated turns."""
        try:
            if not any(k in user_text.lower() for k in self._BROWSER_HINTS):
                return ""
            path = os.path.join(os.getcwd(), ".puppy", "browser.json")
            if not os.path.exists(path):
                return ""
            with open(path, "r", encoding="utf-8") as f:
                data = json.load(f)
            cdp = data.get("cdp", "")
            if not cdp:
                return ""
            url = data.get("url", "")
            helper = data.get("helper", "")
            where = f" showing {url}" if url else ""
            note = f"[context] The in-app browser is open{where}. CDP endpoint: {cdp}. "
            if helper:
                note += (
                    "A ready-made, dependency-free CDP helper is already on disk at "
                    f'"{helper}". RUN IT \u2014 do NOT write your own script into the '
                    "project. "
                    f'`python "{helper}" {cdp} console` dumps recent console '
                    "logs/errors; "
                    f'`python "{helper}" {cdp} eval "<js>"` runs JavaScript in the '
                    "page; "
                    f'`python "{helper}" {cdp} screenshot <out.png>` grabs a shot. '
                )
            else:
                note += (
                    "To inspect it, attach over CDP: GET "
                    f"{cdp}/json/list for a target's webSocketDebuggerUrl, connect that "
                    "websocket, and issue CDP methods (Runtime.evaluate, Log.enable, "
                    "Page.captureScreenshot). "
                )
            note += (
                "Do NOT create files in my project for this (use the helper, or a temp "
                "dir you clean up). Ignore this note if my request is unrelated to the "
                "browser."
            )
            return note
        except Exception:
            return ""

    async def handle_prompt(self, msg_id: int, text: str, images=None) -> None:
        self.current_run = asyncio.current_task()
        try:
            self._sanitize_history()  # never send an orphaned tool_use/result pair
            ctx = self._browser_context(text)
            prompt_text = f"{ctx}\n\n{text}" if ctx else text
            attachments = _decode_image_attachments(images)
            if attachments:
                result = await self.agent.run_with_mcp(prompt_text, attachments=attachments)
            else:
                result = await self.agent.run_with_mcp(prompt_text)
            # Canonicalize the agent's history from the result, exactly like the
            # CLI does. The history_processors callback may not capture the final
            # message, so without this the NEXT turn (and the autosave) can send a
            # malformed history → 400 "tool_result without tool_use".
            if hasattr(result, "all_messages"):
                try:
                    self.agent.set_message_history(list(result.all_messages()))
                except Exception:
                    log("set history from result failed:\n" + traceback.format_exc())
            output = getattr(result, "output", None)
            if output is None:
                output = str(result)
            send({"event": "result", "id": msg_id, "output": output})
        except asyncio.CancelledError:
            send({"event": "error", "id": msg_id, "message": "cancelled by user"})
        except Exception as exc:  # surface, don't crash the bridge
            send({
                "event": "error",
                "id": msg_id,
                "message": f"{type(exc).__name__}: {exc}",
            })
            log(traceback.format_exc())
        finally:
            self.current_run = None
            # Save the conversation after every turn (success or cancel), like the CLI.
            self._autosave()

    def run_slash_command(self, msg_id: int, text: str) -> None:
        """Run a Code Puppy slash command via its dispatcher, off the loop.

        Handlers emit their output through the message bus (already streamed).
        A handler may return a string, which means "treat this as user input" —
        in that case we run it as a normal model turn.
        """
        def work() -> None:
            try:
                from code_puppy.command_line.command_handler import handle_command
                result = handle_command(text)
            except Exception as exc:
                send({"event": "error", "id": msg_id,
                      "message": f"command failed: {type(exc).__name__}: {exc}"})
                log(traceback.format_exc())
                return
            if result == "__AUTOSAVE_LOAD__":
                # /autosave_load (/resume): the CLI opens a TTY picker — instead
                # surface our GUI session browser.
                self.emit_sessions(open_picker=True)
                send({"event": "command_done", "id": msg_id, "handled": True})
            elif isinstance(result, str):
                asyncio.run_coroutine_threadsafe(
                    self.handle_prompt(msg_id, result), self.loop)
            else:
                send({"event": "command_done", "id": msg_id,
                      "handled": bool(result)})
        threading.Thread(target=work, name=f"cmd-{msg_id}", daemon=True).start()

    def _cancel_run(self) -> None:
        """Cancel the in-flight agent turn (called on the event loop thread)."""
        if self.current_run is not None and not self.current_run.done():
            self.current_run.cancel()

    # --- remote filesystem (for SSH-hosted workspaces) ---------------------
    # These let the GUI browse + read files where the sidecar runs. They are
    # synchronous and run on the stdin-reader thread; `send` is lock-guarded.
    def fs_list_dir(self, cmd: dict) -> None:
        rid = cmd.get("id")
        path = cmd.get("path", "")
        try:
            entries = []
            with os.scandir(path) as it:
                for e in it:
                    try:
                        is_dir = e.is_dir()
                    except OSError:
                        is_dir = False
                    entries.append({"name": e.name, "is_dir": is_dir})
            send({"event": "fs_result", "id": rid, "op": "list_dir",
                  "ok": True, "entries": entries})
        except Exception as exc:
            send({"event": "fs_result", "id": rid, "op": "list_dir",
                  "ok": False, "error": str(exc)})

    def fs_read_file(self, cmd: dict) -> None:
        rid = cmd.get("id")
        path = cmd.get("path", "")
        try:
            with open(path, "r", encoding="utf-8", errors="replace") as f:
                content = f.read()
            send({"event": "fs_result", "id": rid, "op": "read_file",
                  "ok": True, "content": content})
        except Exception as exc:
            send({"event": "fs_result", "id": rid, "op": "read_file",
                  "ok": False, "error": str(exc)})

    def fs_stat(self, cmd: dict) -> None:
        rid = cmd.get("id")
        path = cmd.get("path", "")
        send({"event": "fs_result", "id": rid, "op": "stat", "ok": True,
              "exists": os.path.exists(path), "is_dir": os.path.isdir(path)})

    def _fs_mutate(self, rid, op, fn) -> None:
        """Run a mutating fs op, reporting ok/error uniformly."""
        try:
            fn()
            send({"event": "fs_result", "id": rid, "op": op, "ok": True})
        except Exception as exc:
            send({"event": "fs_result", "id": rid, "op": op,
                  "ok": False, "error": str(exc)})

    def fs_write_file(self, cmd: dict) -> None:
        path = cmd.get("path", "")
        content = cmd.get("content", "")

        def do() -> None:
            with open(path, "w", encoding="utf-8") as f:
                f.write(content)
        self._fs_mutate(cmd.get("id"), "write_file", do)

    def fs_mkdir(self, cmd: dict) -> None:
        path = cmd.get("path", "")
        self._fs_mutate(cmd.get("id"), "mkdir", lambda: os.mkdir(path))

    def fs_create_file(self, cmd: dict) -> None:
        path = cmd.get("path", "")
        # 'x' = exclusive create: error if it already exists.
        self._fs_mutate(cmd.get("id"), "create_file",
                        lambda: open(path, "x").close())

    def fs_remove(self, cmd: dict) -> None:
        path = cmd.get("path", "")

        def do() -> None:
            if os.path.isdir(path) and not os.path.islink(path):
                shutil.rmtree(path)
            else:
                os.remove(path)
        self._fs_mutate(cmd.get("id"), "remove", do)

    def fs_rename(self, cmd: dict) -> None:
        src = cmd.get("from", "")
        dst = cmd.get("to", "")

        def do() -> None:
            if os.path.exists(dst):  # refuse to clobber
                raise FileExistsError(dst)
            os.rename(src, dst)
        self._fs_mutate(cmd.get("id"), "rename", do)

    def git_run(self, cmd: dict) -> None:
        """Run `git -C <root> <args>` on this (remote) host for RemoteGit."""
        rid = cmd.get("id")
        root = cmd.get("root", "")
        args = [str(a) for a in (cmd.get("args") or [])]
        # Never block on a tty prompt; merge any caller-supplied env (creds).
        env = os.environ.copy()
        env["GIT_TERMINAL_PROMPT"] = "0"
        for k, v in (cmd.get("env") or {}).items():
            env[str(k)] = str(v)
        try:
            proc = subprocess.run(
                ["git", "-C", root, *args],
                capture_output=True, text=True, errors="replace", env=env)
            send({"event": "git_result", "id": rid, "ok": proc.returncode == 0,
                  "code": proc.returncode, "stdout": proc.stdout,
                  "stderr": proc.stderr})
        except Exception as exc:
            send({"event": "git_result", "id": rid, "ok": False, "code": -1,
                  "stdout": "", "stderr": str(exc)})

    def handle_command(self, cmd: dict) -> None:
        op = cmd.get("op")
        if op == "prompt":
            asyncio.run_coroutine_threadsafe(
                self.handle_prompt(
                    int(cmd.get("id", 0)), cmd.get("text", ""), cmd.get("images")
                ),
                self.loop,
            )
        elif op == "cancel":
            if self.loop is not None:
                self.loop.call_soon_threadsafe(self._cancel_run)
        elif op == "command":
            self.run_slash_command(int(cmd.get("id", 0)), cmd.get("text", ""))
        elif op == "complete":
            self.complete(int(cmd.get("id", 0)), cmd.get("text", ""),
                          int(cmd.get("cursor", 0)))
        elif op == "list_commands":
            self.emit_commands()
        elif op == "list_agents":
            self.emit_agents()
        elif op == "list_models":
            self.emit_models()
        elif op == "set_model":
            self.set_model(cmd.get("name", ""))
        elif op == "status":
            self.emit_status()
        elif op == "list_sessions":
            self.emit_sessions()
        elif op == "load_session":
            self.load_session(cmd.get("name", ""), cmd.get("source", "autosave"))
        elif op == "preview_session":
            self.preview_session(cmd.get("name", ""), cmd.get("source", "autosave"))
        elif op == "set_puppy_name":
            self.set_puppy_name(cmd.get("name", ""))
        elif op == "pause":
            self.pause_agent()
        elif op == "resume":
            self.resume_agent()
        elif op == "steer":
            self.steer_agent(cmd.get("text", ""), cmd.get("mode", "now"))
        elif op == "ask_response":
            self.resolve_ask(cmd)
        elif op == "respond_input":
            from code_puppy.messaging import UserInputResponse
            self.bus.provide_response(UserInputResponse(
                prompt_id=cmd["prompt_id"], value=cmd.get("value", "")))
        elif op == "respond_confirmation":
            from code_puppy.messaging import ConfirmationResponse
            self.bus.provide_response(ConfirmationResponse(
                prompt_id=cmd["prompt_id"],
                confirmed=bool(cmd.get("confirmed", False)),
                feedback=cmd.get("feedback")))
        elif op == "respond_selection":
            from code_puppy.messaging import SelectionResponse
            self.bus.provide_response(SelectionResponse(
                prompt_id=cmd["prompt_id"],
                selected_index=int(cmd.get("selected_index", -1)),
                selected_value=cmd.get("selected_value", "")))
        elif op == "set_agent":
            from code_puppy.agents.agent_manager import (
                get_current_agent,
                set_current_agent,
            )
            if set_current_agent(cmd.get("name", "")):
                self.agent = get_current_agent()
                try:
                    self.agent.reload_code_generation_agent()
                except Exception:
                    pass
                self.emit_ready()
                self.emit_agents()
                log(f"switched agent to {cmd.get('name')}")
            else:
                send({"event": "error", "id": None,
                      "message": f"unknown agent: {cmd.get('name')}"})
        elif op == "list_mcp_servers":
            self.emit_mcp_servers()
        elif op == "set_mcp_enabled":
            self.set_mcp_enabled(cmd.get("name", ""),
                                 bool(cmd.get("enabled", False)))
        elif op == "add_mcp_server":
            self.add_mcp_server(cmd)
        elif op == "list_skills":
            self.emit_skills()
        elif op == "get_skill":
            self.get_skill(str(cmd.get("name", "")))
        elif op == "set_skill_enabled":
            self.set_skill_enabled(str(cmd.get("name", "")),
                                   bool(cmd.get("enabled", False)))
        elif op == "save_skill":
            self.save_skill(cmd)
        elif op == "list_agent_configs":
            self.emit_agent_configs()
        elif op == "get_agent_config":
            self.get_agent_config(cmd.get("name", ""))
        elif op == "save_agent_config":
            self.save_agent_config(cmd)
        elif op == "delete_agent_config":
            self.delete_agent_config(cmd.get("name", ""))
        elif op == "clone_agent_config":
            self.clone_agent_config(cmd.get("name", ""))
        elif op == "fs_list_dir":
            self.fs_list_dir(cmd)
        elif op == "fs_read_file":
            self.fs_read_file(cmd)
        elif op == "fs_stat":
            self.fs_stat(cmd)
        elif op == "fs_write_file":
            self.fs_write_file(cmd)
        elif op == "fs_mkdir":
            self.fs_mkdir(cmd)
        elif op == "fs_create_file":
            self.fs_create_file(cmd)
        elif op == "fs_remove":
            self.fs_remove(cmd)
        elif op == "fs_rename":
            self.fs_rename(cmd)
        elif op == "git_run":
            self.git_run(cmd)
        elif op == "shutdown":
            self._stop.set()
            self.loop.call_soon_threadsafe(self.loop.stop)
        else:
            log(f"unknown op: {op!r}")

    # --- stdin reader ------------------------------------------------------
    def start_stdin_reader(self) -> None:
        def read() -> None:
            for raw in sys.stdin:
                line = raw.strip()
                if not line:
                    continue
                try:
                    cmd = json.loads(line)
                except json.JSONDecodeError as exc:
                    log(f"bad command JSON: {exc}")
                    continue
                try:
                    self.handle_command(cmd)
                except Exception:
                    log(traceback.format_exc())
            # stdin closed -> parent gone -> shut down.
            self._stop.set()
            if self.loop is not None:
                self.loop.call_soon_threadsafe(self.loop.stop)
        threading.Thread(target=read, name="stdin-reader", daemon=True).start()

    # --- run ---------------------------------------------------------------
    def run(self) -> None:
        self.loop = asyncio.new_event_loop()
        asyncio.set_event_loop(self.loop)
        try:
            self.init_code_puppy()
        except Exception as exc:
            send({"event": "error", "id": None,
                  "message": f"init failed: {type(exc).__name__}: {exc}"})
            log(traceback.format_exc())
            return
        self.start_bus_poller()
        self.start_stdin_reader()
        try:
            self.loop.run_forever()
        finally:
            self._stop.set()


if __name__ == "__main__":
    Bridge().run()
