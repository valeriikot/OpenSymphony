# Vikunja Tracker

OpenSymphony can poll a self-hosted [Vikunja](https://vikunja.io) instance
instead of Linear or Jira for the orchestrator run loop. Select it with
`tracker.kind: vikunja` in the target repository's `WORKFLOW.md` front matter.

## 1. Configuration

```yaml
tracker:
  kind: vikunja
  endpoint: https://vikunja.example.com
  project_slug: "7"
  active_states:
    - Todo
  terminal_states:
    - Done
```

- `tracker.endpoint` is required and stores the Vikunja instance base URL
  (the client appends `/api/v1`). There is no default endpoint.
- `tracker.project_slug` stores the **numeric Vikunja project id** (the `7`
  in `https://vikunja.example.com/projects/7`), not the project title.
- Vikunja has no workflow statuses — a task is either open or done — so the
  tracker exposes exactly two state names: `Todo` and `Done`. Configure
  `active_states: [Todo]` and `terminal_states: [Done]`.
- `active_states` and `terminal_states` accept **only** `Todo` and `Done`
  (case-insensitive). Any other name — for example a Linear/Jira value such as
  `In Progress` or `Backlog` carried over from another workflow — is rejected
  when the client is constructed. This is deliberate: such a name matches no
  task, so the scheduler would otherwise poll forever and never dispatch, with
  nothing in the logs to explain why.

## 2. Credentials

| Source | Field / Env Var | Notes |
|--------|-----------------|-------|
| API token | `tracker.api_key` or `VIKUNJA_API_TOKEN` | Required; sent as a bearer token |

Create the token under *Settings → API Tokens* in Vikunja and grant it read
access to tasks, projects, and task comments. `tracker.api_key` supports
`${VAR}` environment expansion, mirroring the other tracker fields.

## 3. API surface and normalization

The internal `opensymphony_vikunja` module talks to the Vikunja REST API v1:

- `GET /api/v1/projects/{id}/tasks` for candidate and terminal reads (paged
  with `page`/`per_page`)
- `GET /api/v1/tasks/{id}` for state refresh
- `GET /api/v1/tasks/{id}/comments` for Agent Harness Workpad comments (also
  paged with `page`/`per_page`)

Both list reads page until a request returns no task or comment id that has
not already been seen. A short page does **not** terminate paging: Vikunja
clamps `per_page` to the server's `service.maxitemsperpage` setting, so a
truncated page is normal. Deduplicating by id also makes the loop terminate
against a server that replays the same page.

Payloads normalize into the same tracker-neutral domain models Linear and
Jira use:

| Vikunja | Domain |
|---------|--------|
| task `id` | `TrackerIssue.id` |
| task `identifier` (`PREFIX-12`, or `#12` rewritten to `TASK-12`) | `TrackerIssue.identifier` |
| `done` flag | `TrackerIssue.state` (`Todo`/`Done`), `TrackerIssueStateKind` (`Unstarted`/`Completed`) |
| `priority` (`1` low .. `5` DO NOW) | `TrackerIssue.priority` (`1` urgent .. `4` low; `0`/unset maps to none) |
| `related_tasks.parenttask` / `related_tasks.subtask` | `parent` / `sub_issues` |
| `related_tasks.blocked` | `blocked_by` |
| labels | `TrackerIssue.labels` |

Descriptions and comments arrive as HTML fragments from Vikunja's editor and
are flattened to plain text; agent-authored comments (such as the
`## Agent Harness Workpad` marker comment) round-trip unchanged.

Rate limiting honors `Retry-After`, with the same retry policy semantics as
the Linear and Jira clients.

## 4. Current scope

Supported with `tracker.kind: vikunja`:

- the scheduler tracker backend (candidate polling, state refresh, terminal
  cleanup, identifier lookups)
- workpad comment rehydration for issue sessions
- gateway task graph reads

Still Linear-only:

- planning publish artifacts (`convert-tasks-to-linear`) and gateway task
  graph mutations
- `opensymphony memory` tracker sources (the memory commands fail with a
  clear error when the workflow tracker is Vikunja)
- Vikunja does not attach branch names or pull requests to tasks, so
  `branch_name` and `pr_url` are always empty; PR linkage relies on the
  worker-side flow instead.

Kanban buckets are not mapped to states; only the open/done flag drives the
`Todo`/`Done` state split.
