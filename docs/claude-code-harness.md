# Claude Code Local Harness

OpenSymphony can dispatch issue runs to Anthropic's Claude Code CLI in
headless mode, alongside the managed OpenHands agent-server (default) and the
local Codex app-server harness.

## Selecting the harness

In the target repo's `WORKFLOW.md` front matter:

```yaml
routing:
  harness: claude_code
  model: claude-sonnet-5   # optional; omit to use the CLI's configured model
```

Or per shell session:

```bash
export OPENSYMPHONY_HARNESS="claude_code"
export OPENSYMPHONY_MODEL="claude-sonnet-5"      # optional
export OPENSYMPHONY_CLAUDE_BIN="$(command -v claude)"  # optional when `claude` is on PATH
```

## Local Harness Scope

- Runtime kind: `claude_code`.
- Transport: one headless CLI session per issue run:
  `claude --print --verbose --output-format stream-json --permission-mode bypassPermissions [--model <model>]`.
- The rendered workflow prompt (the same Liquid template used by the other
  harnesses) is written to the child's stdin.
- Contract: newline-delimited stream-json events
  (`claude-code-stream-json-v1`).

The `opensymphony_claude` module provides:

- launch argument construction for headless stream-json sessions,
- normalization of `system.init`, `assistant`, `user`, and terminal `result`
  events while preserving the raw payload,
- bounded human-readable summaries (assistant text previews, tool
  invocations, session lifecycle),
- token usage mapping from assistant and result events into scheduler
  `TokenUsageUpdate`s.

## Session lifecycle

1. The worker backend spawns the CLI inside the issue workspace with the
   worker environment (this is how `ANTHROPIC_API_KEY` or the operator's
   `claude` login reach the harness).
2. The `system.init` event's `session_id` becomes the conversation id; the
   conversation manifest and launch report are recorded at that point.
3. Every stream event is forwarded to the gateway as a
   `claude.<type>` runtime event with a summary and the raw payload.
4. The terminal `result` event maps to the run outcome: `success` →
   succeeded, `is_error`/`error_*` subtypes → failed.
5. Scheduler interrupts (for example when the tracker reports the issue is
   merging) terminate the session process and record the run as cancelled —
   the headless contract has no graceful in-turn cancellation.

## Credentials

The harness relies on the operator-owned Claude Code CLI setup: either an
active `claude` login (Claude subscription) or `ANTHROPIC_API_KEY` exported
into the environment that `opensymphony run` inherits. OpenSymphony does not
manage or store Anthropic credentials itself.

## Current limitations

- Single non-interactive session per run: no mid-session user messages,
  approvals, pause/resume, history fetch, or replay cursors.
- Interrupts kill the session process rather than cancelling a turn in-band.
- Hosted Claude worker pools and remote transports are out of scope for the
  local adapter.
