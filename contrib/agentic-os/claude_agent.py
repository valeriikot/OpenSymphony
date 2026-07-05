"""Claude Code headless agent adapter for Agentic OS.

Drop-in port of OpenSymphony's `opensymphony-claude` module: drives
Anthropic's Claude Code CLI in headless mode (`claude --print
--output-format stream-json`) as a fourth Agentic OS agent alongside
opencode, Hermes, and Gemini. Stdlib only.

One call = one non-interactive session: the prompt is written to the child's
stdin, newline-delimited JSON events stream back on stdout, and the session
ends with a terminal ``result`` event that carries the final text, token
usage, and ``total_cost_usd`` (feed that straight into /api/cost/record).

Environment:
    CLAUDE_BIN   Path to the Claude Code CLI (default: "claude" on PATH).
                 Requires a working `claude` login or ANTHROPIC_API_KEY.
"""
from __future__ import annotations

import json
import os
import queue
import signal
import subprocess
import threading
from dataclasses import dataclass, field

CLAUDE_BIN_ENV = "CLAUDE_BIN"
DEFAULT_TIMEOUT_SECONDS = 600
SUMMARY_PREVIEW_CHARS = 160


def build_command(model: str | None = None, claude_bin: str | None = None) -> list[str]:
    """Launch flags for one headless stream-json session.

    Headless runs cannot answer interactive permission prompts, hence
    bypassPermissions — run inside a workspace you trust the agent with.
    """
    args = [
        claude_bin or os.environ.get(CLAUDE_BIN_ENV, "claude"),
        "--print",
        "--verbose",
        "--output-format",
        "stream-json",
        "--permission-mode",
        "bypassPermissions",
    ]
    if model:
        args += ["--model", model]
    return args


def normalize_event(value: object) -> dict | None:
    """Normalizes one stream-json line; None for non-event payloads."""
    if not isinstance(value, dict):
        return None
    event_type = value.get("type")
    if not isinstance(event_type, str):
        return None
    subtype = value.get("subtype")
    qualified = (
        f"{event_type}.{subtype}"
        if event_type == "system" and isinstance(subtype, str)
        else event_type
    )
    session_id = value.get("session_id")
    if not (isinstance(session_id, str) and session_id.strip()):
        session_id = None
    return {
        "type": qualified,
        "session_id": session_id,
        "usage": token_usage(value),
        "raw": value,
    }


def token_usage(value: dict) -> dict | None:
    """Token usage from a result event's `usage` or an assistant message's
    `message.usage`. Returns input/output/cache_read/total or None."""
    usage = value.get("usage")
    if not isinstance(usage, dict):
        message = value.get("message")
        usage = message.get("usage") if isinstance(message, dict) else None
    if not isinstance(usage, dict):
        return None

    def read(key: str) -> int:
        raw = usage.get(key)
        return raw if isinstance(raw, int) and raw >= 0 else 0

    input_tokens = read("input_tokens")
    output_tokens = read("output_tokens")
    cache_read = read("cache_read_input_tokens")
    cache_creation = read("cache_creation_input_tokens")
    if input_tokens == 0 and output_tokens == 0 and cache_read == 0:
        return None
    return {
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "cache_read_tokens": cache_read,
        "total_tokens": input_tokens + output_tokens + cache_read + cache_creation,
    }


def event_summary(event: dict) -> str:
    """Bounded human-readable line for dashboards/streaming views."""
    raw = event["raw"]
    event_type = event["type"]
    if event_type == "system.init":
        return f"Claude Code session started (model {raw.get('model', '<unknown>')})"
    if event_type == "assistant":
        message = raw.get("message") or {}
        for block in message.get("content") or []:
            if not isinstance(block, dict):
                continue
            if block.get("type") == "text" and (text := (block.get("text") or "").strip()):
                preview = " ".join(text[: SUMMARY_PREVIEW_CHARS + 40].split())
                if len(preview) > SUMMARY_PREVIEW_CHARS:
                    preview = preview[:SUMMARY_PREVIEW_CHARS] + "…"
                return f"Claude: {preview}"
            if block.get("type") == "tool_use":
                return f"Claude invoked tool {block.get('name', '<unknown>')}"
        return "Claude Code assistant message"
    if event_type == "user":
        return "Tool results returned to Claude Code"
    if event_type == "result":
        state = "failed" if raw.get("is_error") else "completed"
        turns = raw.get("num_turns")
        suffix = f" after {turns} turn(s)" if isinstance(turns, int) else ""
        return f"Claude Code session {state} ({raw.get('subtype', 'unknown')}){suffix}"
    return f"Claude Code event {event_type}"


@dataclass
class ClaudeRunResult:
    outcome: str  # succeeded | failed | timeout | not_installed | error
    text: str = ""
    session_id: str | None = None
    subtype: str | None = None
    num_turns: int | None = None
    duration_ms: int | None = None
    total_cost_usd: float | None = None
    usage: dict = field(
        default_factory=lambda: {
            "input_tokens": 0,
            "output_tokens": 0,
            "cache_read_tokens": 0,
            "total_tokens": 0,
        }
    )
    error: str | None = None

    @property
    def ok(self) -> bool:
        return self.outcome == "succeeded"


def _accumulate_usage(totals: dict, usage: dict, terminal: bool) -> None:
    # Assistant events carry per-message usage (accumulate); the terminal
    # result event carries session-cumulative usage (field-wise max so
    # totals never shrink regardless of which source counted more).
    for key in totals:
        totals[key] = (
            max(totals[key], usage[key]) if terminal else totals[key] + usage[key]
        )


def run_claude(
    message: str,
    model: str | None = None,
    timeout: float = DEFAULT_TIMEOUT_SECONDS,
    on_event=None,
    claude_bin: str | None = None,
    cwd: str | None = None,
) -> ClaudeRunResult:
    """Runs one headless Claude Code session.

    ``timeout`` is an idle timeout between stream events, not a total budget —
    long tool executions reset it. ``on_event`` (if given) receives
    ``(normalized_event, summary)`` per event for live dashboard streaming.
    Never raises: every failure mode is a ClaudeRunResult outcome, matching
    Agentic OS's execute_agent error style.
    """
    command = build_command(model, claude_bin)
    try:
        child = subprocess.Popen(
            command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            cwd=cwd,
            # New session => the CLI and every tool subprocess it spawns share
            # a process group we can kill as one; otherwise an inherited stdout
            # pipe keeps the reader (and stream close) blocked after kill.
            start_new_session=True,
        )
    except FileNotFoundError:
        return ClaudeRunResult(
            outcome="not_installed",
            error=f"Claude Code CLI not found (`{command[0]}`). Install it and try again.",
        )

    result = ClaudeRunResult(outcome="error")
    assistant_text: list[str] = []
    lines: queue.Queue = queue.Queue()

    def pump() -> None:
        for line in child.stdout:
            lines.put(line)
        lines.put(None)

    reader = threading.Thread(target=pump, daemon=True)
    reader.start()

    try:
        child.stdin.write(message)
        child.stdin.close()
    except (BrokenPipeError, OSError):
        pass  # the CLI may exit early with an error event; keep reading

    try:
        return _read_session(child, lines, result, assistant_text, timeout, on_event)
    finally:
        _kill_session(child)
        child.wait()
        reader.join(timeout=2)
        for stream in (child.stdin, child.stdout, child.stderr):
            if stream is not None:
                try:
                    stream.close()
                except OSError:
                    pass


def _kill_session(child) -> None:
    try:
        os.killpg(os.getpgid(child.pid), signal.SIGKILL)
    except (ProcessLookupError, PermissionError, OSError):
        try:
            child.kill()
        except OSError:
            pass


def _read_session(
    child,
    lines: queue.Queue,
    result: ClaudeRunResult,
    assistant_text: list[str],
    timeout: float,
    on_event,
) -> ClaudeRunResult:
    while True:
        try:
            line = lines.get(timeout=timeout)
        except queue.Empty:
            result.outcome = "timeout"
            result.error = f"no stream events for {timeout:.0f}s; session terminated"
            return result
        if line is None:
            result.error = "Claude Code stdout closed before a result event"
            stderr_tail = (child.stderr.read() or "")[-300:].strip()
            if stderr_tail:
                result.error += f"; stderr: {stderr_tail}"
            return result
        line = line.strip()
        if not line:
            continue
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        event = normalize_event(value)
        if event is None:
            continue
        if on_event is not None:
            try:
                on_event(event, event_summary(event))
            except Exception:
                pass  # a broken dashboard listener must not kill the run
        if event["session_id"] and result.session_id is None:
            result.session_id = event["session_id"]

        raw = event["raw"]
        if event["type"] == "assistant":
            for block in (raw.get("message") or {}).get("content") or []:
                if isinstance(block, dict) and block.get("type") == "text":
                    text = (block.get("text") or "").strip()
                    if text:
                        assistant_text.append(text)
        if event["usage"]:
            _accumulate_usage(result.usage, event["usage"], event["type"] == "result")

        if event["type"] == "result":
            result.subtype = raw.get("subtype")
            result.num_turns = raw.get("num_turns")
            result.duration_ms = raw.get("duration_ms")
            cost = raw.get("total_cost_usd")
            result.total_cost_usd = cost if isinstance(cost, (int, float)) else None
            final_text = raw.get("result")
            result.text = (
                final_text.strip()
                if isinstance(final_text, str) and final_text.strip()
                else "\n\n".join(assistant_text)
            )
            is_error = bool(raw.get("is_error")) or str(result.subtype or "").startswith(
                "error"
            )
            result.outcome = "failed" if is_error else "succeeded"
            if is_error:
                result.error = event_summary(event)
            return result
