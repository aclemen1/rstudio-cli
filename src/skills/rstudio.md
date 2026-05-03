---
name: rstudio
description: Interact with the embedded RStudio IDE (Server on Linux, Desktop on macOS) from a terminal — open files, run R code, drive the editor / viewer / markers / files panes, list and read terminal buffers, inspect the live R environment.
version: __VERSION__
---

# rstudio — RStudio IDE bridge

Use the `rstudio` CLI to talk to the live RStudio session running on the
same machine. The bridge auto-detects which mode applies:

- **RStudio Server** (Linux): speaks to the rsession Unix socket and
  shares the active browser client, so visible actions land directly in
  the user's open RStudio tab.
- **RStudio Desktop** (macOS): speaks to the rsession's TCP loopback
  with the shared secret; same effect on the live IDE window.

The CLI works whether you're invoked from inside RStudio's embedded
terminal (env vars set) or from any other process on the same host
(auto-discovery kicks in).

This skill is intentionally minimal: the CLI is self-documenting via
`rstudio schema`. **Don't pre-load the surface — discover on demand.**

## Self-update check

Before relying on this skill, verify it's current:

    rstudio version

Compare the JSON's `version` field with the `version` in the frontmatter
above. If the CLI's value is higher, update:

    rstudio skill install --force

Then re-read this file. The skill is shipped inside the CLI binary, so
its version always equals the CLI's `Cargo.toml` version.

## At the start of a session

Run once, before anything else:

    rstudio status

This returns a single-call snapshot: CLI version + auto-detected mode
(Server / Desktop), transport (Unix socket or TCP loopback), user
identity, session id, active client id, sources directory, R version,
RStudio version, active project, and the open-document count + the
active doc id and path. It saves a chain of `session info` + `editor
list` + `editor active-id` calls and gives you the full context the
user is working in.

## How to use

Always start with the catalog and drill down only where you need detail:

    rstudio schema                # level 0: every action with category + summary
    rstudio schema <category>     # level 1: actions in that category
    rstudio schema <cat> <action> # level 2: full ActionSpec (params, examples, errors, returns)

Each level-2 entry tells you exactly: parameter names and types, defaults,
allowed values for enums, runnable examples, the JSON return shape, and
the error kinds the action can produce. **Read the level-2 schema before
invoking any unfamiliar action.**

## Output contract

JSON by default. Add `--format text` for a human-readable rendering on
the few commands that support it; otherwise `--format text` falls back
to pretty JSON.

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

## Concurrency model

R is single-threaded; the `rsession` serialises every `r exec` /
`exec eval` style call into a FIFO. Two concurrent calls do **not**
run in parallel — total wall time ≈ sum of per-call time. Implications:

- `--timeout 0` on a long-running `r exec` blocks every subsequent
  `exec`-style call until it returns. Be explicit about long timeouts.
- For real parallelism (running shell commands or external processes),
  use `term exec` — the Terminal pane spawns a separate pty/process,
  not bound to the R FIFO.
- Postbacks (`editor edit`) and console_input (`r send`) don't go
  through the R queue and therefore aren't subject to that limit.

## Patterns worth knowing

- **Run R silently and read its output**: `rstudio r exec '<R code>'`.
  Returns `{output: string}`. Default elapsed limit is 2 s — pass
  `--timeout T` to extend (or `--timeout 0` to disable, see above).
- **Type into the user's R console (visible)**: `rstudio r send '<R code>'`.
  Fire-and-forget; the user sees the command appear and run.
- **Open a file at a specific line in the editor**:
  `rstudio editor open <path> --line N`. This is the non-modal path —
  the file appears in the Source pane. `editor edit <path>` is the
  *modal* path (R `edit()` dialog with Save/Cancel).
- **Read what the user typed lately**: `rstudio console history --limit N`.
- **Read shell terminal output (RStudio Terminal pane)**: `rstudio term list`
  to find the id, then `rstudio term buffer <id> [--limit N]`. To run a
  shell command: `rstudio term exec <id> '<bash command>'`, then re-read
  `term buffer` after a moment to see the output.
- **Surface lint-style feedback**: `rstudio pane markers --markers '<JSON>'`
  with `[{type, file, line, column?, message}, ...]`. Useful for
  batch-reporting issues you found.
- **Inspect a variable without loading it**: `rstudio env info <name>`
  returns class/typeof/length/dim/size_bytes only. For the formatted
  contents, `rstudio env contents <name>`.
- **Sync the editor buffer after an external file write**: if you
  modify a file's content via tools other than `rstudio editor *`
  (e.g. `Edit`, `Write`, `MultiEdit`, shell redirection, `git`), the
  user's RStudio buffer keeps the stale content until they click the
  tab and accept the "file changed" dialog. To skip that friction:

      rstudio editor reload --path <path> --if-clean

  Safe to call unconditionally: it's a no-op (`action: skipped-not-open`)
  if the file isn't in the Source pane, and a no-op (`action:
  skipped-dirty`) if the user has unsaved edits in that buffer. Only
  call once per file write — don't loop.

## Hard constraints

- Never invoke `rstudio rpc client_init`. It's blacklisted at the CLI
  level because it invalidates the user's browser client and resets
  their RStudio session.
- `rstudio r send` and `rstudio term send/exec` are **visible** in
  the user's UI — only use them when the action is meant to be seen.
- `rstudio editor insert/select/edit` operate on the **active** document
  (or open a modal). If you don't know which one is active, run
  `rstudio editor context` first.
- `rstudio term kill <id>` and similar destructive ops are not undoable
  through the CLI. Confirm the id with `rstudio term list` first.
