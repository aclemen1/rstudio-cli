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
above. If the CLI's value is higher, update this skill.

__UPDATE_SECTION__

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

    rstudio schema                  # level 0: the 15 categories + action_count
    rstudio schema <category>       # level 1: actions in that category (name + summary)
    rstudio schema <cat> <action>   # level 2: full ActionSpec (params, examples, errors, returns)
    rstudio schema --search <regex> # search: matching actions across all categories

Level 0 returns just the categories — pick one, then drill in. To get
the full flat catalog of every action (the old level-0 output), use
`rstudio schema --search '.*'`.

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

## MCP server mode

`rstudio mcp` exposes the entire CLI surface as MCP tools over stdio.
A user configures their MCP client once. Variants per client:

**Claude Code** (CLI):

```sh
claude mcp add rstudio --scope user -- rstudio mcp
```

**Claude Desktop** — edit `claude_desktop_config.json`
(`~/Library/Application Support/Claude/` on macOS):

```json
{
  "mcpServers": {
    "rstudio": { "command": "rstudio", "args": ["mcp"] }
  }
}
```

**Cline / Continue / Cursor** — same `{command, args}` shape in the
extension's MCP settings panel.

**Verify** the server runs:

```sh
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
  | rstudio mcp | jq '.result.serverInfo'
```

After configuration, `~90` tools appear in the agent's tool catalog.
Naming:
`{category}_{action}` with hyphens replaced by underscores —
`editor.read-buffer` becomes `editor_read_buffer`, `meta.status`
becomes `meta_status`, etc.

For multi-call atomicity (the same use cases that warrant `rstudio tx
--` from the CLI), three MCP-native tools mirror the lock semantics:

- **`tx_begin`** — acquire the per-session writer lock; hold across
  subsequent tool-calls.
- **`tx_end`** — release. Pair with tx_begin.
- **`tx_run`** — `{operations: [{tool, arguments}]}` — execute a list
  under one tx with auto-cleanup on error. Use when you can pre-
  compute the entire sequence.

The same defensive rule from the CLI applies: any sequence of MCP
tool-calls that depends on intermediate state read from earlier calls
must run inside a tx (begin/end pair, or tx_run). Cost when alone:
~10ms (one extra tool-call). Cost of skipping: silent data loss when
another agent shows up between your read and your write.

The MCP server's lock contends with shell `rstudio` invocations and
other MCP servers via the same `flock` — there's no special MCP-only
coordination.

## Multi-agent safety: the per-session lock and `rstudio tx`

When two agents run `rstudio` against the same RStudio session, write
commands compete for an OS-level `flock` at
`~/.config/rstudio-cli/locks/session-<id>.lock`. Reads, `observe stream`,
and meta-CLI never lock. Default timeout when waiting: 30 s. If you
hit a timeout, the error includes the holder's PID and command — that
is another agent (or a previous run of yours that hasn't finished).

### How do I know if I'm alone?

**You can't know reliably.** Any check races against new agents
appearing between your check and your action. Don't try to gate
behaviour on it. The defensive rule below works whether you're alone
or not.

You CAN, however, get a moment-in-time read for awareness or
debugging:

```sh
rstudio status | jq '.result.session.lock'
# {state: "free" | "held", holder: {pid, command, started_ms} | null,
#  inside_tx: bool}
```

Use this to debug a timeout or to surface "another agent is active"
to the user — not as a control-flow gate.

### The defensive rule

| Operation | What you must do |
|---|---|
| Single read (`editor list`, `env list`, …) | Nothing. Reads don't lock. |
| `observe stream` | Nothing. It's a read; runs alongside everything. |
| Single write (one `rstudio` invocation) | Nothing. Per-call mutex protects you. |
| Multi-call sequence where any call reads state used by a later call | **Always wrap in `rstudio tx --`.** |

The third row covers everything where you'd be tempted to "just check
first and then write". Don't. The check and the write are two CLI
invocations; another agent can interleave. Wrap.

### `rstudio tx --` in practice

```sh
# Atomic read-modify-write — no other agent can interleave.
rstudio tx -- bash -c '
  buf=$(rstudio editor read-buffer X | jq -r .result.contents)
  new=$(printf "%s" "$buf" | sed "s/foo/bar/g")
  rstudio editor set-contents X "$new"
'
```

`rstudio tx -- <cmd>` acquires the lock, sets `RSTUDIO_TX_HELD=1`,
and execs `<cmd>`. Every nested `rstudio` invocation inside detects
the env var and skips its own per-call lock acquisition (the parent
already holds it). Kernel cleanup on parent exit — there are no
stale locks.

Cost when alone: ~10ms (one fork). Cost of NOT using tx when not
alone: silent data loss. Always wrap multi-call write sequences.

### What NOT to put inside a `tx`

- `rstudio observe stream` — it never returns and would hold the lock
  indefinitely. (Read-only; doesn't need tx anyway.)
- `rstudio ui dialog` and other `ui` commands — they block the
  rsession until the user dismisses the modal, which freezes every
  other agent's RPC during that time.
- Anything else that you can't ensure terminates promptly.

### Interactive mode

`rstudio tx -- bash` (or `rstudio tx` for `$SHELL`) gives you a shell
with the lock held — the whole shell session runs inside the
transaction. Useful when you want a series of commands with results
fed back to you between calls (the LLM-agent equivalent of a REPL
inside a lock). Exit the shell to release the lock.

### Serialisation, not full ACID

`tx` provides serialisation (no other agent interleaves), not
transactionality. If your 3rd command fails, the first two are
already applied. Rollback is your responsibility (snapshot state
before, restore on error).

The global `--no-lock` flag bypasses all locking. Don't use it from
within an LLM-driven agent unless you understand the consequences —
it's intended for debugging and solo scripts.

## Patterns worth knowing

- **Run R visibly and capture its output**: `rstudio r send '<R code>'`.
  Returns `{stdout, messages: string[], warnings: string[], error:
  string|null, eval_env}`. The user sees `ℝ(~{ code })` appear and run in
  their console. **Prefer this over `r exec` whenever the user should see
  what is running** — you get the same output with full visibility. On an
  R error the envelope is `ok:false`/`kind=r_error` but still carries the
  partial `stdout`/`messages`/`warnings` captured before the failure — so
  you never lose what ran up to a `stop()`. Pass `--no-capture` for true
  fire-and-forget (nothing returned). Pass `--timeout T` to bound the poll
  wait.
- **Run R silently and read its output**: `rstudio r exec '<R code>'`.
  Returns `{output: string}`. Use when silent/background execution is
  specifically required. Default elapsed limit is 2 s — pass
  `--timeout T` to extend (or `--timeout 0` to disable, see above).
- **Open a file at a specific line in the editor**:
  `rstudio editor open <path> --line N`. This is the non-modal path —
  the file appears in the Source pane. `editor edit <path>` is the
  *modal* path (R `edit()` dialog with Save/Cancel).
- **Stop a long-running computation**: three distinct surfaces, three
  distinct commands — pick by what you launched.
  - `rstudio r interrupt` — equivalent of the Stop button in the
    console pane. Targets the foreground R execution (a blocked
    `r send`, a user-typed loop, …). Returns immediately; the blocked
    `r send` in the other shell ends with `kind=r_error`,
    `message="R execution was interrupted"`. Fire-and-forget; safe
    when nothing is running (no-op). **Bypasses the per-session lock
    by design** — its purpose is to unblock whoever currently holds
    the lock, so it must not wait for them to release it.
  - `rstudio r kill <id> [--tree]` — terminate an async job created
    by `r exec --async` (callr sub-process). Sends SIGTERM; with
    `--tree`, also kills descendants spawned via `system()` /
    `processx`. Returns `{status: "killed" | "already-done"}`.
  - `rstudio job kill <id>` — stop a job in the Jobs pane (created
    by `job add` or `job run-script`). Best-effort: fires the
    rsession internal "stop" RPC if available, then forces the UI
    state to `cancelled`. Returns `{cancelled: true, hard_killed: bool}`
    — `hard_killed=false` means the underlying R sub-process may
    still be running (rare; depends on RStudio version), only the
    pane label changed.
- **Read what the user typed lately**: `rstudio console history --limit N`.
- **Read shell terminal output (RStudio Terminal pane)**: `rstudio term list`
  to find the id, then `rstudio term buffer <id> [--limit N]`. To run a
  shell command: `rstudio term exec <id> '<bash command>'`, then re-read
  `term buffer` after a moment to see the output.
- **Surface lint-style feedback**: `rstudio pane markers --markers '<JSON>'`
  with `[{type, file, line, column?, message}, ...]`. Useful for
  batch-reporting issues you found.
- **Render and preview a document in the Viewer pane**:
  - `rstudio pane preview <path>` — auto-detects `.md`, `.Rmd`/`.rmd`,
    or `.qmd` and renders to HTML in the Viewer pane. Add `--no-view`
    to render without displaying.
  - Explicit variants with full control: `pane preview-md` (requires
    `markdown` R package), `pane preview-rmd` (requires `rmarkdown`),
    `pane preview-qmd` (requires Quarto on PATH). Both `preview-md` and
    `preview-rmd` accept `--output-dir` to redirect the HTML output.
  - Return value: `{input, output, format: "html", viewer_loaded: bool}`.
  - These commands lift the socket timeout for long renders.
- **Inspect a variable without loading it**: `rstudio env info <name>`
  returns class/typeof/length/dim/size_bytes only. For the formatted
  contents, `rstudio env contents <name>`.
- **Project lifecycle** — under `rstudio project`:
  - `project current` — return the active project path (null if none).
  - `project open <path>` — open an existing project (replaces session
    unless `--new-session`).
  - `project new <path> [--scaffold] [--git] [--no-open]` — create a
    NEW directory + a `.Rproj`, optionally scaffold `R/` + `README.md`
    + `.gitignore`, optionally `git init`. With `--no-open` works
    even without RStudio running (pure filesystem).
  - `project init <path> [--scaffold] [--git] [--no-open]` — make
    an EXISTING directory a project (writes `.Rproj`, optionally
    scaffolds, optionally initialises git). Refuses if a `.Rproj`
    already exists.
  - `project clone <git-url> [<path>] [--no-open]` — `git clone`
    the URL, then add a `.Rproj` if the repo doesn't have one, then
    open. Useful to bring an external R codebase into the IDE in
    one shot.
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

## Debugging workflow (R's `browser()`, `debug()`, `recover()`)

When the user's R session enters a debugger, the console sits at a
`Browse[n]>` prompt. The CLI is fully aware of this state — both for
introspection and for evaluation — but the verbs are different from
"normal" R execution.

**Detect**. `rstudio status` reports a top-level `rsession.debugger`
field (`null` at the regular prompt; populated when a browser is
active — including a bare `browser()` or one sent via `r send`). For
the full picture — current `function`, source location, typed locals,
full call stack — call `rstudio debug status`. It also reports
`browse_level` (the N of `Browse[N]>`) when `browse_level_source` is
`"native"`; on hosts without a C toolchain the level can't be computed
(R doesn't expose it) and `browse_level` is `null` with
`browse_level_source: "unavailable"`. You never need the level to
navigate — `debug exit` / `debug step Q` leaves all nested browsers at
once.

**Evaluate**. `r send` and `r exec` are *browser-aware*: when called
while a debugger is active, they automatically evaluate the user's code
in the frame being debugged (not in `.GlobalEnv`). The response always
carries an `eval_env` field that confirms where the code actually ran:

      rstudio r send 'y + 1'   # at a Browse[n]> prompt
      → {stdout: "[1] 43", messages: [], error: null,
         eval_env: {kind: "browser_frame", function: "debug_me", depth: 1}}

      rstudio r exec 'ls()'    # while at the same prompt
      → {output: "[1] \"x\" \"y\" \"z\"",
         eval_env: {kind: "browser_frame", function: "debug_me", depth: 1}}

This works for both reads (`ls()`, `str(x)`) and writes (`y <- 9999L`
will persist when the user types `c`). `eval_env.kind` is one of:
`global`, `attached` (search-path env), `browser_frame`, `top_level`
(`r exec` outside a debugger), or `background_job` (`r exec --async` /
`r poll`).

**Navigate**. Browser meta-commands have a dedicated verb so they
never get wrapped in `ℝ(~{…})` (which would evaluate them as bare R
symbols):

      rstudio debug step n     # next statement
      rstudio debug step s     # step in
      rstudio debug step f     # finish current function
      rstudio debug step c     # continue (exits one level)
      rstudio debug step Q     # quit all browser levels
      rstudio debug exit       # alias of `debug step Q`

`debug step` refuses with `kind=user_error` if no debugger is active.

**Inspect**. `debug locals`, `debug where`, `debug src` project
rsession's `get_environment_state` into compact agent-friendly JSON.

**Observe**. The `observe` stream (`--tier 2`) emits three debugger
events: `debug.entered`, `debug.exited`, `debug.frame_changed`. Agents
that tail observe will see browser entries/exits without polling.

**Don't** send raw browser commands via `r send 'n'` — the wrapper
evaluates `n` as a symbol and errors out with "object 'n' not found".
Use `debug step n` instead. (`r send --no-capture 'n'` works as an
escape hatch but is not idiomatic.)

## Hard constraints

- Never invoke `rstudio rpc client_init`. It's blacklisted at the CLI
  level because it invalidates the user's browser client and resets
  their RStudio session.
- `rstudio r send` and `rstudio term send/exec` are **visible** in
  the user's UI — only use them when the action is meant to be seen.
  `r send` (without `--no-capture`) holds the session lock while
  polling for the captured result; sequential agents are unaffected,
  but concurrent `r exec` calls from other agents queue behind it.
- `rstudio editor insert/select/edit` operate on the **active** document
  (or open a modal). If you don't know which one is active, run
  `rstudio editor context` first.
- `rstudio term kill <id>` and similar destructive ops are not undoable
  through the CLI. Confirm the id with `rstudio term list` first.
