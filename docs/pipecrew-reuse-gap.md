# Pipecrew "Reuse" vs. OpenSymphony: Gap Analysis

Analysis of [Pipecrew](https://pipecrew.ai) against this repository: what
Pipecrew means by knowledge *reuse*, how much of it OpenSymphony already has,
and the narrow gaps worth putting on the roadmap.

## The two products in one paragraph each

**Pipecrew** is a Claude Code plugin — "a self-learning multi-repo agent crew."
You describe one feature in plain language; it detects which repositories are
affected, dispatches a stack-specialized implementer + reviewer per repo in
parallel git worktrees against a shared contract, verifies cross-repo before
opening PRs, and then *learns*: `/learn` reads the merged PR's review comments
and proposes tier-classified updates to a durable, on-disk workspace layer, so
"every run starts smarter than the last."

**OpenSymphony** (this repo, Rust) is an issue-tracker-driven orchestrator: it
polls Linear/Jira, creates an isolated per-issue workspace, dispatches an AI
harness (OpenHands, Codex app-server, or Claude Code headless), streams
normalized runtime events through a gateway, and manages retries, interrupts,
recovery, and notifications.

## What Pipecrew means by "reuse"

"Reuse" is not one feature on the Pipecrew site — it is the spine of the
product, and it means **knowledge reuse across runs**:

- **Durable workspace layer** (long-term memory on disk): domain, topology and
  conventions — "the platform map, written once and reused" — plus run history
  and "reusable recipes and skills."
- **`/learn` loop:** reads a merged PR's review comments, proposes
  *tier-classified* updates (repo / workspace / plugin) to that durable layer.
- **`/patch` recipes:** each recurring fix becomes "both a template and a
  detector — so a class of change gets cheaper every time."
- **Prefix caching:** a stable prefix is "read once and reused."
- **Team sharing:** the durable layer lives in a private repo; every run pulls
  the team's latest first.

## Two different axes called "reuse"

OpenSymphony already uses the word "reuse" heavily, but for a **narrower,
runtime-level** meaning than Pipecrew's cross-run learning:

| Axis | What it reuses | Where in this repo |
| --- | --- | --- |
| **Conversation reuse** | An OpenHands conversation/session per issue across retries and daemon restarts (`per_issue` default, `fresh_each_run` override) | `crates/opensymphony-workflow/src/{model,resolve}.rs`, `docs/architecture.md` |
| **Context condensation** | In-conversation history via `LLMSummarizingCondenser` | `docs/configuration.md` |
| **Learned-knowledge reuse** | Completed-issue knowledge across *future* issues | `crates/opensymphony-memory/*` |

The first two are session-level and have nothing to do with Pipecrew. The third
is the axis Pipecrew is selling — and OpenSymphony already has most of it.

## What OpenSymphony already has (this is the important finding)

The `opensymphony-memory` crate is a substantial cross-run learning loop that
maps closely onto Pipecrew's continuous-learning pillar:

- **Capture ≈ `/learn`:** terminal issue transitions are auto-captured during
  `opensymphony run` into source-referenced capsules; capture reads Linear
  narrative, PR body, **review discussion, checks**, and changed files
  (`capture.rs`, `github.rs`).
- **`memory context` ≈ reading the durable layer every run:** a pre-implementation
  context compiler assembles a kickoff bundle and writes it to
  `.opensymphony/generated/memory-context.md` inside each new issue workspace
  (`query.rs`, `render_memory_context`).
- **Docs sync ≈ tier-classified doc updates:** stable, confidence-gated
  knowledge is promoted into topic docs (`docs_sync.rs`).
- **Durable, portable, shareable layer:** Markdown + OKF bundles, DuckDB
  catalog, `export-okf` / `import-okf`, and a multi-repo memory server
  (COE-448).
- **Code intelligence:** Tree-sitter AST symbols/diagnostics give agents
  source-cited structural context (`opensymphony-code-intel`).

So the earlier instinct — "we're missing learned-knowledge reuse" — is wrong.
We have the loop. What differs is the *shape* of what the loop produces and a
couple of sharing ergonomics.

## The actual gaps

1. **Actionable fix recipes (`/patch`).** Our capsules and topic docs are
   *narrative* ("what happened, decisions, gotchas"). We have no
   *detector-plus-template* artifact — a recurring fix expressed as a reusable
   procedure plus a rule that says "this run looks like a case that needs it."
   This is the clearest, highest-value gap. **Prototyped** in
   `crates/opensymphony-memory/src/recipes.rs`; designed in
   [docs/specs/reusable-fix-recipes.md](specs/reusable-fix-recipes.md).
2. **Interactive tier-classified learning approval.** We auto-capture and sync
   docs by confidence threshold. Pipecrew surfaces *per-finding* proposals the
   operator approves and classifies as repo/workspace/plugin. Adopting explicit
   tiers + approval would sharpen what enters the durable layer.
3. **Auto team-sync before each run.** We can export/import OKF bundles and run
   a multi-repo memory server, but there is no "pull the team's latest durable
   layer before the crew starts" step. This is an ergonomics gap on top of
   existing plumbing, not new infrastructure.

Explicitly **out of scope** for OpenSymphony: Pipecrew's multi-repo fan-out and
one-feature-many-PRs model. Our unit of work is one tracker issue in one
workspace; copying the crew topology would be a different product, not a memory
feature.

## Roadmap recommendation

- **Now (prototyped):** land the fix-recipe library as a third memory output —
  read/render slice first (inject matched recipes into the `memory context`
  bundle), capture-side proposal second.
- **Next:** add tier + per-finding approval to the capture → docs-sync path.
- **Later / ergonomics:** an opt-in "pull team durable layer before run" sync on
  top of OKF import and the multi-repo memory server.
- **Won't do:** multi-repo crew fan-out; it does not fit the tracker/workspace
  model.

## Pointers

- Prototype: `crates/opensymphony-memory/src/recipes.rs`
- Design spec: [docs/specs/reusable-fix-recipes.md](specs/reusable-fix-recipes.md)
- Existing memory model: [docs/memory.md](memory.md),
  [docs/specs/okf-memory-spec.md](specs/okf-memory-spec.md)
