# Agentic OS drop-in ports

Python ports of OpenSymphony components for
[modimihir07/agentic-os](https://github.com/modimihir07/agentic-os),
per the analysis in [`docs/agentic-os-extraction.md`](../../docs/agentic-os-extraction.md).
Stdlib only (Python 3.10+) — nothing to add to `requirements.txt`.

| File | Ported from | Purpose |
|------|-------------|---------|
| `claude_agent.py` | `crates/opensymphony-claude` | Claude Code CLI as a 4th agent (headless stream-json) |
| `notify.py` | `crates/opensymphony-notify` | Slack + LINE completion notifications |
| `retry.py` | tracker-client retry logic | Backoff discipline for agent/HTTP calls |
| `tests/` | — | Self-contained unittest suites for all three |

## Install

Copy the three modules into the agentic-os repo root (next to `server.py`),
and optionally the tests:

```bash
cp claude_agent.py notify.py retry.py /path/to/agentic-os/
cp -r tests /path/to/agentic-os/tests-contrib
cd /path/to/agentic-os && python3 -m unittest discover tests-contrib -v
```

## Wire up Claude Code as a 4th agent

In `server.py`, add a branch to `execute_agent`:

```python
import claude_agent

def execute_agent(agent: str, message: str) -> str:
    ...
    elif agent == "claude":
        result = claude_agent.run_claude(message, timeout=600)
        if result.outcome == "not_installed":
            return f"⚠ {result.error}"
        if result.outcome == "timeout":
            return f"⏱ Claude Code timed out.\n\n**Message:** {message[:100]}"
        if result.total_cost_usd is not None:
            record_cost({          # automatic cost analytics — no manual POST
                "provider": "anthropic",
                "model": "claude",
                "agent": "claude",
                "cost": result.total_cost_usd,
                "tokens": result.usage["total_tokens"],
            })
        if result.ok:
            return result.text or "**Claude**\n\nProcessed your message."
        return f"⚠ {result.error}"
```

Then add `"claude"` to the router candidates in `run_skill` and a
`check_agent("claude")` health probe (`claude --version`). For live dashboard
streaming, pass `on_event=lambda event, summary: ...` and forward summaries
over your SSE/WebSocket channel instead of waiting for the final text.

Requirements: an installed [Claude Code CLI](https://claude.com/claude-code)
with a `claude` login or `ANTHROPIC_API_KEY` in the server's environment.
Note the adapter launches with `--permission-mode bypassPermissions` (headless
runs can't answer prompts) — treat the working directory as agent-writable.

## Wire up notifications

```python
import notify

# at the end of run_skill(...)
if notify.is_enabled():
    errors = notify.notify_skill_completed(name, agent_choice, run_id,
                                           output_preview=response_text[:300])
    for error in errors:
        append_audit({"action": "notify_error", "detail": error})
```

Configure via environment: `SLACK_WEBHOOK_URL` for Slack;
`LINE_CHANNEL_ACCESS_TOKEN` + `LINE_RECIPIENT_ID` for LINE (Messaging API
push — LINE Notify is discontinued). Delivery is best-effort by design:
`notify_success` returns error strings for logging and never raises.

## Wire up retries

Wrap transient-prone calls (agent CLIs, outbound HTTP) with:

```python
from retry import TransientError, classify_http_status, retry_call

def call():
    code, out, err = run_cli([...], timeout=30)
    if code != 0 and looks_transient(err):
        raise TransientError(err)
    return out

out = retry_call(call, attempts=3)
```

Key semantics (see `retry.py` docstring): exponential backoff with a cap,
`Retry-After` honored only for 429 and only up to a ceiling, permanent errors
never retried. Composes with the existing circuit breaker.
