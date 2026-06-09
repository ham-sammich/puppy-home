"""puppy-home <-> Code Puppy bridge (sidecar).

This process runs *inside an environment that has `code_puppy` importable*. It
imports Code Puppy's real internals, drives the current agent programmatically,
and bridges its two messaging systems (the new structured MessageBus and the
legacy MessageQueue) to the Rust GUI over a line-delimited JSON protocol.

Protocol (newline-delimited JSON, UTF-8):

  Rust -> sidecar (stdin), one object per line:
    {"op": "prompt",  "id": <int>, "text": "..."}   # a model turn
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

stdout is reserved exclusively for the protocol. Any stray library `print()` is
redirected to stderr so it can never corrupt a JSON line.
"""

import asyncio
import json
import os
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
        send({
            "event": "ready",
            "agent": getattr(self.agent, "name", "code-puppy"),
            "model": model or "(unset)",
            "cp_version": self.cp_version,
            "cwd": os.getcwd(),
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
    async def handle_prompt(self, msg_id: int, text: str) -> None:
        self.current_run = asyncio.current_task()
        try:
            result = await self.agent.run_with_mcp(text)
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
            if isinstance(result, str):
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

    def handle_command(self, cmd: dict) -> None:
        op = cmd.get("op")
        if op == "prompt":
            asyncio.run_coroutine_threadsafe(
                self.handle_prompt(int(cmd.get("id", 0)), cmd.get("text", "")),
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
