---
name: rstudio
description: Interact with the embedded RStudio Server IDE from a terminal — open files, run R code, drive the editor / viewer / markers / files panes, list and read terminal buffers, inspect the live R environment.
skill_version: 1
---

# rstudio — RStudio Server IDE bridge

Use the `rstudio` CLI to talk to the RStudio Server session that hosts the
terminal you're running in. The bridge speaks to that session's `rsession`
Unix socket and shares the active browser client, so visible actions land
directly in the user's open RStudio tab without disrupting it.

This skill is intentionally minimal: the CLI is self-documenting via
`rstudio schema`. **Don't pre-load the surface — discover on demand.**

## Self-update check

Before relying on this skill, verify it's current:

    rstudio version

Compare the JSON's `skill` field with the `skill_version` in the
frontmatter above. If the CLI's value is higher, update:

    rstudio skill install --force

Then re-read this file.

## How to use

Always start with the catalog and drill down only where you need detail:

    rstudio schema                # level 0: every action with category + summary
    rstudio schema <category>     # level 1: actions in that category
    rstudio schema <cat> <action> # level 2: full ActionSpec (params, examples, errors, returns)

Each level-2 entry tells you exactly: parameter names and types, defaults,
allowed values for enums, runnable examples, the JSON return shape, and the
error kinds the action can produce. **Read the level-2 schema before
invoking any unfamiliar action.**

## Output contract

JSON by default. Add `--format text` for a human-readable rendering on the
few commands that support it; otherwise `--format text` falls back to the
JSON value.

    {"ok": true,  "result": <T>}      # success with data
    {"ok": true}                      # success, no data
    {"ok": false, "error": {
       "code": <int>,
       "kind": "<kind>",
       "message": "..."
    }}

`kind` is one of: `user_error` (bad CLI args), `r_error` (R raised a
condition — see `message` for the conditionMessage), `rpc_error` (the
RStudio JSON-RPC layer rejected the call), `timeout` (R evaluation
exceeded the elapsed-time limit), `session_unavailable` (no live RStudio
session reachable), `internal` (CLI bug, please report).

Exit codes: `0` ok, `1` runtime error, `2` bad CLI args.

## Patterns worth knowing

- **Run R silently and read its output**: `rstudio exec run '<R code>'`.
  Returns `{output: string}` (auto-printed values + cat/message/print).
  Default elapsed limit is 2 s — pass `--timeout T` to extend (or
  `--timeout 0` to disable).
- **Type into the user's R console (visible)**: `rstudio exec send '<R code>'`.
  Fire-and-forget; the user sees the command appear and run.
- **Read what the user typed lately**: `rstudio console history --limit N`.
  Live, sourced from `get_recent_history`.
- **Read shell terminal output (RStudio Terminal pane)**: `rstudio term list`
  to find the id, then `rstudio term buffer <id> [--limit N]`. To run a
  shell command: `rstudio term exec <id> '<bash command>'`, then re-read
  `term buffer` after a moment to see the output. Or
  `rstudio term create --name '...'` to spawn a fresh terminal first.
- **Open a file at a specific line in the editor**:
  `rstudio editor open <path> --line N`.
- **Surface lint-style feedback**: `rstudio view mark --markers '<JSON>'`
  with `[{type, file, line, column?, message}, ...]`. The user sees them
  in the Markers pane. Useful for batch-reporting issues you found.
- **Inspect a variable without loading it**: `rstudio env info <name>`
  returns class/typeof/length/dim/size_bytes only. For the formatted
  contents, `rstudio env contents <name>`.

## Hard constraints

- Never invoke `rstudio rpc client_init`. It's blacklisted at the CLI level
  because it invalidates the user's browser client and resets their
  RStudio session.
- `rstudio exec send` and `rstudio term send/exec` are **visible** in the
  user's UI — only use them when the action is meant to be seen.
- `rstudio editor insert/select` operate on the **active** document. If
  you don't know which one is active, run `rstudio editor context` first.
- `rstudio term kill <id>` and similar destructive ops are not undoable
  through the CLI. Confirm the id with `rstudio term list` first.
