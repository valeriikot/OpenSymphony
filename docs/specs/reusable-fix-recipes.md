# Reusable Fix-Recipe Library Specification

Status: draft

Source basis: the current OpenSymphony project-memory model
([docs/memory.md](../memory.md), [okf-memory-spec.md](okf-memory-spec.md)) and
the continuous-learning model described by Pipecrew (<https://pipecrew.ai>),
whose `/patch` command turns a recurring fix into "both a template and a
detector — so a class of change gets cheaper every time."

Reader: an OpenSymphony engineer extending the memory subsystem.

Post-read action: add a reusable fix-recipe library that captures recurring
fixes as detector-plus-template artifacts, matches them against a run, and
injects the relevant recipes into the `memory context` kickoff bundle — without
weakening private-memory boundaries, source citation, or scheduler correctness.

## 1. Summary

OpenSymphony already has a durable learning loop:

- **Capture** turns a completed issue into a source-referenced capsule and
  evolves `memory.yaml` (`crates/opensymphony-memory/src/capture.rs`).
- **`memory context`** compiles a kickoff bundle from captured memory and writes
  it to `.opensymphony/generated/memory-context.md` inside each new issue
  workspace (`crates/opensymphony-memory/src/query.rs`,
  `render_memory_context`).
- **Docs sync** promotes stable knowledge into topic docs
  (`crates/opensymphony-memory/src/docs_sync.rs`).

This is a close match for Pipecrew's "durable workspace layer that gets sharper
every run." The gap is the *shape* of what the loop produces. Capsules and topic
docs are **narrative** knowledge — what happened, decisions, gotchas. They are
not **actionable, reusable procedures** with a detector that says "this run
looks like a case where this fix applies."

A fix recipe closes that gap. It is a small Markdown document with two halves:

- a **detector** — changed-path globs plus issue-text keywords that decide when
  the recipe is relevant to a run; and
- a **template** — the reusable Markdown guidance the agent should follow when
  the detector fires.

Recipes are stored alongside capsules, matched during context assembly, and
rendered into the kickoff bundle so the next run starts with the relevant
recurring-fix playbooks already in hand.

## 2. Goals

1. Represent a recurring fix as a portable detector-plus-template Markdown file.
2. Classify each recipe by tier — `repo`, `workspace`, or `plugin` — mirroring
   Pipecrew's tier-classified updates, so a recipe's blast radius is explicit.
3. Match recipes deterministically against a run's changed paths and issue text.
4. Inject matched recipes into the existing `memory context` kickoff bundle.
5. Propose new/updated recipes from merged-PR review signal, gated by operator
   approval, so the library compounds without polluting itself automatically.
6. Reuse existing memory idioms: Markdown + YAML frontmatter, repo containment,
   private-by-default visibility, and the DuckDB catalog as a derived layer.

## 3. Non-Goals

- Do not replace capsules or topic docs; recipes are a third, complementary
  output of the same loop.
- Do not execute recipe bodies. A recipe is guidance an agent reads, never code
  OpenSymphony runs on a target repo.
- Do not make recipe matching a scheduler dependency. Missing, malformed, or
  empty recipes must degrade to today's behavior, never block or fail a run.
- Do not auto-write recipes into the durable layer without operator approval in
  the first slice.
- Do not build a cross-repo assessor or multi-repo fan-out (a separate Pipecrew
  pillar that does not map onto OpenSymphony's one-issue-one-workspace model).

## 4. Data Model

A recipe is a Markdown file under `<memory_root>/recipes/<id>.md` with YAML
frontmatter and a Markdown body:

```markdown
---
id: reconnect-backoff
title: Add bounded backoff to WebSocket reconnect loops
tier: repo
path_globs:
  - crates/*/src/session.rs
  - crates/**/reconnect.rs
keywords:
  - reconnect
  - websocket drop
source_issue: COE-265
---

1. Wrap the reconnect attempt in an exponential backoff with a capped ceiling.
2. Emit a structured `reconnect.attempt` event with the attempt number.
3. Add a regression test that asserts the ceiling is respected after N failures.
```

Fields:

| Field          | Required | Meaning                                                                 |
| -------------- | -------- | ----------------------------------------------------------------------- |
| `id`           | yes      | Stable slug; also the file name.                                        |
| `title`        | yes      | One-line human summary.                                                 |
| `tier`         | yes      | `repo` \| `workspace` \| `plugin` — the recipe's scope/blast radius.    |
| `path_globs`   | no       | `/`-delimited globs (`*` within a segment, `**` across segments).       |
| `keywords`     | no       | Case-insensitive substrings matched against issue title + description.  |
| `source_issue` | no       | Provenance — the issue/PR the recipe was learned from.                  |
| body           | yes      | Reusable Markdown guidance shown when the detector fires.               |

A recipe with no `path_globs` and no `keywords` never fires; the detector is
"any glob hit OR any keyword hit."

## 5. Detector Semantics

- **Path globs** match against the run's changed-file list. `*` matches any run
  of characters within a single path segment; `**` matches any number of whole
  segments. `crates/*/src/session.rs` matches `crates/foo/src/session.rs` but
  not `crates/foo/bar/src/session.rs`; `crates/**/session.rs` matches both.
- **Keywords** match case-insensitively as substrings of the concatenated issue
  title and description.
- A recipe **fires** when at least one glob or one keyword matches. Each match
  contributes a human-readable reason string rendered into the bundle so the
  agent (and operator) can see *why* a recipe surfaced.
- Matching is pure and deterministic — no network, no DuckDB dependency — so it
  is safe to run on the scheduler's hot path during context assembly.

## 6. Integration Points

1. **Context assembly (read path).** `render_memory_context` in `query.rs`
   appends an "Applicable Fix Recipes" section when recipes match. The changed
   paths come from the same `--paths` discovery `memory context` already accepts
   for code-intelligence; issue text comes from the current `IssueEvidence`. The
   prototype exposes exactly this call:
   `matched_recipes_section(config, changed_paths, issue_text)`.
2. **Capture (write path, later slice).** After a terminal transition, capture
   reads the merged PR's review comments and diff (it already fetches these for
   capsules) and proposes tier-classified recipe additions/edits. Proposals are
   surfaced for per-finding operator approval — matching the confidence-gated
   posture docs sync already uses — before anything is written to the durable
   layer.
3. **CLI.** `opensymphony memory recipes list|show|add|match` for inspection and
   manual authoring, consistent with the existing `memory` command surface.
4. **Catalog (optional).** Recipes may be indexed into DuckDB as a derived
   layer for search/related queries, exactly as capsules are; the Markdown files
   remain the durable store.

## 7. Visibility, Sharing, and Safety

- Recipes are **private by default**, under `.opensymphony/memory/recipes/`,
  following the memory visibility posture. Public promotion is explicit.
- Recipe bodies must carry the same redaction guarantees as capsules: no secret
  values, only `file:line` and issue/PR references.
- Team sharing rides the existing OKF export/import and multi-repo memory-server
  paths (`memory export-okf` / `import-okf`, COE-448). A future "pull team
  latest before each run" auto-sync is Pipecrew's shared-durable-layer behavior
  and is tracked separately.
- All recipe writes are repo-containment-checked (`ensure_repo_contained`), like
  every other memory write.

## 8. Prototype Status

`crates/opensymphony-memory/src/recipes.rs` implements the read/render slice:

- `Recipe` / `RecipeTier` types and Markdown+frontmatter (de)serialization;
- `load_recipes`, `write_recipe` (containment-checked);
- `match_recipes` with the glob + keyword detector above;
- `render_recipes_section` and the one-shot `matched_recipes_section` that a
  `render_memory_context` integration or a post-run hook calls directly;
- unit tests covering glob semantics, detector firing, disk round-trip, and the
  empty/no-match cases.

Deferred to later slices: capture-side recipe proposal from merged-PR review,
operator-approval UX, CLI surface, DuckDB indexing, and wiring the section into
`render_memory_context` behind changed-path discovery.

## 9. Open Questions

1. Should recipe matching prefer changed paths from code-intelligence discovery,
   the PR diff on retries, or both?
2. How aggressively should capture propose recipes — every recurring review
   comment, or only patterns seen across N issues?
3. Do `plugin`-tier recipes graduate into shipped skills/docs, and who reviews
   that promotion?
