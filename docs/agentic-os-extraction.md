# Extracting OpenSymphony Components Into agentic-os

Analysis of [modimihir07/agentic-os](https://github.com/modimihir07/agentic-os)
(v0.3.0, Python/FastAPI) against this repository: which OpenSymphony
components, contracts, and hard-won design lessons are worth extracting into
agentic-os, where each lands in its codebase, and what the port costs.

## The two projects in one paragraph each

**agentic-os** is a locally-hosted control plane that unifies three CLI agents
(opencode, Hermes, Gemini CLI) behind one FastAPI dashboard (`server.py`,
~1,600 lines). Agents are invoked as *blocking subprocesses* returning plain
text (`execute_agent`, 30–180 s timeouts). Around that core: SKILL.md skill
packs with per-run learnings, an APScheduler cron engine with file-watcher
reload, markdown "brain" memory with SQLite FTS5 search, manual cost tracking
(`POST /api/cost/record`), JSONL audit log, local-JSON kanban, inbound
webhooks, and a circuit breaker.

**OpenSymphony** (this repo, Rust) is an issue-tracker-driven orchestrator:
it polls Linear/Jira, creates isolated per-issue workspaces, dispatches AI
harnesses (OpenHands, Codex app-server, Claude Code headless), streams
normalized runtime events through a gateway API, and manages retries,
interrupts, recovery, and notifications.

Since the languages differ, "extract" means porting small self-contained
modules to Python and reusing protocol/contract knowledge — not linking code.
Everything below is ranked by value-to-effort.

---

## 1. Claude Code headless adapter → a 4th agentic-os agent (highest value)

**Extract from:** `crates/opensymphony-claude/src/lib.rs` (~400 lines,
self-contained, zero internal dependencies) and the launch/outcome handling in
`crates/opensymphony-cli/src/orchestrator_run/backends.rs`
(`try_run_claude_code_issue`).

**Lands in:** `server.py::execute_agent` as a new `claude` branch, plus
`agents/claude/` config folder.

agentic-os already parses opencode's newline-JSON output line by line — the
exact same pattern OpenSymphony uses for Claude Code stream-json. The
extractable knowledge:

- Launch contract: `claude --print --verbose --output-format stream-json
  --permission-mode bypassPermissions [--model <m>]`, prompt via stdin.
- Event normalization: `system.init` (session id, model, tools) /
  `assistant` (text + tool_use blocks) / `user` (tool results) / terminal
  `result` (subtype, `is_error`, `num_turns`, `duration_ms`,
  `total_cost_usd`, cumulative `usage`).
- Outcome mapping: `success` → succeeded; `is_error` or `error_*` subtypes →
  failed.
- Token usage extraction (`claude_token_usage`): input/output/
  cache_read/cache_creation from `usage` objects.

Two free wins for agentic-os features that already exist:

- **Cost analytics becomes automatic** — the `result` event carries
  `total_cost_usd` and full token usage, so instead of the manual
  `POST /api/cost/record`, the adapter records real per-run costs.
- **Live streaming to the dashboard** — reading stream-json incrementally
  (instead of the current blocking `run_cli`) lets the SPA show assistant
  progress and tool invocations in real time; the `claude_event_summary`
  logic (bounded text previews, "Claude invoked tool X") ports directly.

Port cost: ~150 lines of Python. The Rust module's unit tests double as a
fixture list for the Python port.

## 2. Slack/LINE completion notifications (lowest friction)

**Extract from:** `crates/opensymphony-notify/src/lib.rs` (~300 lines,
self-contained; see `docs/notifications.md`).

**Lands in:** a new `notify.py` called from `run_skill`, scheduler job
completion, and goal completion; env config in Settings.

agentic-os has inbound webhooks but nothing outbound — a heartbeat cron and
daily standup that only write to the dashboard. The port is two HTTP POSTs:

- Slack incoming webhook: `{"text": message}` to `SLACK_WEBHOOK_URL`.
- LINE Messaging API push: `{to, messages:[{type:"text",text}]}` with a
  channel-token bearer header (`LINE_CHANNEL_ACCESS_TOKEN` +
  `LINE_RECIPIENT_ID`), 5,000-char truncation. (LINE Notify is discontinued;
  the Messaging API contract here is the current one.)

Keep the semantics, not just the payloads: best-effort delivery (failures are
logged, never fail the skill run), 10 s timeouts, one channel failing must not
block the other, disabled entirely when unconfigured. ~60 lines of Python.

## 3. Jira/Linear tracker sync for the kanban board

**Extract from:** `crates/opensymphony-jira/src/` (REST client, JQL
construction, normalization) and `crates/opensymphony-domain/src/tracker.rs`
(the tracker-neutral issue model).

**Lands in:** a `tracker.py` + scheduler job syncing the local-JSON kanban
(`data/kanban/`) with a real tracker; optionally webhook-triggered skill runs
per issue.

agentic-os's kanban is local-only. The extractable pieces:

- Jira Cloud v3 **enhanced JQL search** contract (`/rest/api/3/search/jql`,
  `nextPageToken` pagination — including the lesson that empty pages can
  still carry a token and must be followed).
- Auth matrix: basic auth (email + API token) for Cloud, bearer PAT for Data
  Center.
- Normalization decisions: status-category → three-state kind
  (new/indeterminate/done ↔ todo/in-progress/done maps directly onto kanban
  columns), default priority-name mapping with graceful degradation on custom
  schemes, `is blocked by` links → blocked flags (the kanban already has
  block/unblock).
- JQL string escaping and identifier validation before interpolation.
- ADF-to-text rendering (`adf.rs`) for Jira descriptions/comments.

This is the largest port (~300–400 lines of Python) but turns agentic-os's
kanban into a live view of a real tracker, and its webhook receiver can then
drive "issue moved to Todo → run skill" automation — a lightweight version of
what OpenSymphony's scheduler does.

## 4. HTTP retry discipline for agent and API calls

**Extract from:** `should_retry`/`sleep_before_retry`/`exponential_backoff`
in `crates/opensymphony-jira/src/client.rs` and the failure-streak backoff in
`crates/opensymphony-domain/src/runtime.rs` (`RetryPolicy`).

**Lands in:** `server.py::run_cli` / `execute_agent` and any tracker/notify
HTTP calls.

agentic-os currently has timeouts and a circuit breaker but zero retries — a
single transient failure surfaces straight to the user. The portable rules,
each of which cost a real bug to learn here:

- Exponential backoff with `checked` doubling and a hard cap; never trust a
  server-supplied `Retry-After` on non-429 errors beyond your own cap (5xx
  responses can carry hour-long reset headers).
- Retry only idempotent operations; classify errors (auth/not-found are
  permanent, 429/5xx/transport are transient).
- Base backoff growth on the *consecutive-failure streak*, not the total
  attempt count, when successful runs also increment attempts.

~40 lines of Python; composes with their existing circuit breaker (backoff
handles transient noise, the breaker handles sustained outage).

## 5. Declarative agent capability descriptors for the router

**Extract from:** `crates/opensymphony-gateway-schema/src/capability.rs`
(`HarnessKind`, `HarnessCapability`) and
`packages/gateway-schema/src/model_config.ts`.

**Lands in:** `agents/<name>/capability.json` + the router in `server.py`
(currently keyword lists hardcoded in `run_skill`).

agentic-os routes by scanning skill names for keywords. OpenSymphony's
capability model — a per-harness descriptor of available actions, transports,
cost profile, and feature gaps, consumed by a single validation gate — is a
straightforwardly better substrate: skills declare what they need, the router
matches against declared capabilities, and "agent unavailable" becomes a
capability check instead of a subprocess failure. The TypeScript types in
`packages/gateway-schema` can be reused nearly verbatim as JSON Schema for
their dashboard. ~1 day including router rewrite.

## 6. Event-journal cursor contract for audit/history/replay

**Extract from:** `crates/opensymphony-domain/src/journal.rs` and the run
events endpoint in `crates/opensymphony-gateway/src/lib.rs`.

**Lands in:** `scheduler/scheduler.py::get_history`, `GET /api/audit`, and
the session-replay/error dashboards.

agentic-os's histories are truncated JSON arrays fetched with `limit` — no
stable position, so a dashboard polling for "what's new" re-reads and
re-renders everything, and misses events that scroll out between polls. The
portable contract: producer-assigned monotonic sequence numbers, cursor =
"last sequence received", replay starts at `cursor + 1`, bounded buffer with
eviction, and gapless-resume validation (`cursor + 1 < oldest` → invalid, not
`cursor < oldest` — an off-by-one this repo shipped and fixed). ~50 lines and
it makes their SSE-style incremental updates possible.

## 7. Per-run manifests and workspace receipts

**Extract from:** `crates/opensymphony-workspace/src/` (run manifests,
conversation manifests, receipts) — as a *pattern*, not code.

**Lands in:** skill runs (`skills/<name>/runs/<run_id>.json`) instead of
appending unstructured text to `learnings.md`.

agentic-os already stores per-run eval scores; a structured manifest per run
(agent, input, outcome status, duration, token usage, output path) gives its
session-replay and learning-analytics features a real substrate and makes
`learnings.md` a rendered view instead of the source of truth.

---

## Not worth extracting

- **OpenHands / Codex app-server harnesses** — JSON-RPC session management
  solves a problem agentic-os doesn't have; its agents are one-shot CLIs.
- **The Rust TUI, gateway WS streaming stack, DuckDB memory index** —
  agentic-os has its own dashboard, and SQLite FTS5 already covers its memory
  search needs.
- **Workspace git isolation / lifecycle hooks** — agentic-os deliberately
  operates on one live directory; per-issue clones would fight its design.

## Suggested order

| # | Extraction | Effort | Payoff |
|---|-----------|--------|--------|
| 1 | Slack/LINE notifications | ~½ day | Outbound alerts for skills/cron/goals |
| 2 | Claude Code adapter | ~1 day | 4th agent, live streaming, automatic cost capture |
| 3 | Retry discipline | ~½ day | Fewer user-visible transient failures |
| 4 | Tracker sync (Jira first) | ~2–3 days | Kanban becomes a live tracker view; issue-driven skills |
| 5 | Capability router | ~1 day | Principled routing, reusable schema |
| 6 | Journal cursors | ~½ day | Incremental dashboard updates, stable replay |
| 7 | Run manifests | ~1 day | Structured replay/analytics substrate |

Items 1–3 are independent and could each be a first PR against agentic-os;
4–7 build on each other loosely (manifests benefit from cursors, the router
benefits from capability files created alongside the Claude agent).
