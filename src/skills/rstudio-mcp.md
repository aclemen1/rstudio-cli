# rstudio MCP server — agent guidance

You're connected to the **rstudio-cli** MCP server (version __VERSION__).
This server bridges to a running RStudio Server (Linux) or RStudio
Desktop (macOS) IDE. Roughly 90 tools are available, but only a small
**core set** is surfaced in `tools/list` (progressive discovery — see
the next section). This document covers what you can't infer from a
single tool's metadata.

## Discovering tools

`tools/list` returns only a core set:

- `meta_version`, `meta_status` — bridge health and session info.
- `tools_search` — discover the rest of the catalog on demand.
- `tx_begin`, `tx_end`, `tx_run` — multi-agent serialisation (see below).
- `r_script` — run an R script that calls multiple actions in one
  round-trip (programmatic tool calling — see below).

**Every other tool is still callable via `tools/call`** — they're just
not listed up front, to keep your context lean. Use `tools_search` to
find them. It's a 3-level drill-down that mirrors the `rstudio schema`
CLI command (single source of truth for both surfaces):

| Call | Returns | Approx. tokens |
|---|---|---:|
| `tools_search({})` | catalog: 15 categories with `action_count` | ~450 |
| `tools_search({category: "editor"})` | actions of that category as `[{name, summary, param_count}]` | ~400 |
| `tools_search({category: "editor", action: "read-buffer"})` | full ActionSpec + `input_schema` + `mcp_tool_name` ready for `tools/call` | ~250 |
| `tools_search({search: "buffer"})` | regex-matching actions across all categories (flat list) | varies |

**Workflow**: catalog → pick a category → list its actions → drill into
the one you want → `tools/call` using the `mcp_tool_name` from the
level-2 response.

The level-2 response gives you `mcp_tool_name` (e.g. `editor_read_buffer`)
so you don't have to translate hyphens to underscores yourself.

**Shortcut**: if you already know a tool's name from prior context or
from the convention `<category>_<action>` (hyphens → underscores), call
it directly via `tools/call` — `tools_search` is only needed when you
don't yet know what's available.

Categories: `editor`, `r`, `console`, `term`, `env`, `pane`, `skill`,
`project`, `session`, `pref`, `job`, `ui`, `observe`, `policy`, `meta`.

## Tool naming

Every action is a flat tool of the form `<category>_<action>`, with
any hyphens in either part replaced by underscores. So:

- `editor.read-buffer` → `editor_read_buffer`
- `r.exec` → `r_exec`
- `pane.save-plot` → `pane_save_plot`
- `meta.status` → `meta_status`

## Multi-agent safety: when to wrap calls in a transaction

You may share this RStudio session with another agent (another MCP
client, a shell user typing `rstudio`, an editor addin, …). The
server exposes three tools that mirror an OS-level per-session
writer lock:

- **`tx_begin`** — acquire the lock; hold it across subsequent calls.
- **`tx_end`** — release the lock. Always pair with `tx_begin`.
- **`tx_run`** — `{operations: [{tool, arguments}]}` — execute a
  pre-known sequence under one tx, with auto-cleanup on error.

### The rule

| Operation | What you do |
|---|---|
| Single read (`editor_list`, `env_list`, `meta_status`, …) | Nothing. Reads don't lock. |
| Single write (one tool-call) | Nothing. The server's per-call mutex protects it automatically. |
| Multi-call sequence where any later call depends on state read by an earlier one | **Always wrap in `tx_begin` / `tx_end` (or `tx_run`).** |

### Why

Single calls are protected. But across calls, the lock is released
between tool invocations. If you read a buffer, transform it, and
write it back as three separate tool-calls, **another agent can
write to the same buffer between your read and your write — silently
overwriting their work or yours**.

### How do I know if I'm alone?

You can't reliably know. A check `meta_status` → `session.lock` races
against any new agent connecting between the check and your action.
**Default to defensive**: always wrap multi-call sequences in tx,
regardless of perceived solitude. Cost when alone: a single extra
tool-call (~10 ms). Cost when not alone without tx: silent data loss.

You CAN check `meta_status` for **awareness / debugging**: see the
holder's PID, command, and whether you're already inside a tx. Do
not gate behaviour on it.

### What NOT to put inside a tx

- **`observe_stream`** — never returns; would hold the lock forever.
  (It's a read anyway, so it doesn't need a tx.)
- **`ui_dialog`, `ui_prompt`, `ui_question`, `ui_select_*`,
  `ui_ask_*`** — modals. They block the rsession until the user
  dismisses them, freezing every other agent on the same session.
- **`r_exec` with `--timeout 0`** — open-ended R execution. Use
  bounded timeouts inside tx, or do the long work outside.

### Serialisation, not full ACID

`tx` only ensures no other agent interleaves. If your 3rd call
fails, the first two are already applied to the IDE / R state.
Rollback is your responsibility — snapshot before, restore on error.

## Concurrency model — R is single-threaded

The rsession serialises every `r_exec` and any other R-touching call
into a FIFO. Two concurrent calls do **not** run in parallel: total
wall time ≈ sum of per-call time. Implications:

- A long `r_exec` blocks all subsequent `r`-style calls until it
  returns. Be explicit about long timeouts.
- For real parallelism, use `term_exec` — the Terminal pane spawns a
  separate pty/process, not bound to the R FIFO.
- Postbacks (`editor_edit`) and `r_send` (visible console input) bypass
  the R queue.

## Programmatic tool calling: `r_script`

For workflows that chain several reads, transformations, and writes,
prefer **`r_script`** to a chain of `tools/call`s. You send a small R
program; the server runs it in the active rsession, and only the final
value comes back. Intermediate data — buffer contents, file
trees, regex matches — **never traverses your context window**.

```r
# r_script body example:
buf <- editor_get_contents()
lines <- strsplit(buf$contents, "\n", fixed = TRUE)[[1]]
list(
  path = buf$path,
  total_lines = length(lines),
  too_long = which(nchar(lines) > 80)
)
```

The script runs with the `rstudiocli.mcp` R package on the search
path; its exported functions follow the MCP tool naming (`editor_*`,
`pane_*`, …). Inspect the surface from inside the script via
`ls("package:rstudiocli.mcp")`.

Rules:

- The last expression's value is serialised with `jsonlite::toJSON
  (auto_unbox = TRUE)` and returned in the `result` field.
- Throw `stop("msg")` to signal a logical error — the server returns
  `isError: true` with the message.
- **Forbidden inside `tx_begin`**: an active transaction holds the
  session lock that `r_script`'s nested CLI calls would also need →
  deadlock. The server rejects the combination with a clear message.
  Choose one or the other per call.

When to prefer `r_script` over chained `tools/call`s:

- The intermediate data is large (file contents, env dumps) and the
  final answer is small (counts, summaries, ids).
- The orchestration uses control flow (loops, conditions) that
  `tx_run`'s declarative `operations` array can't express.
- You want a single round-trip atomic from the agent's perspective.

When to prefer chained `tools/call`s:

- You need to reason between calls (e.g. "did the markers reveal a
  bug? then open that file"). The LLM has to see the intermediate
  values.
- The sequence is short (2-3 calls).

## Patterns worth knowing

- **Run R visibly and capture its output**: `r_send({code: "..."})`.
  Returns `{stdout: string, messages: string[], error: string|null}`.
  The user sees `ℝ(~{ code })` appear and run in their console.
  **Prefer this over `r_exec` whenever the user should see what is
  running** — you get the same output with full visibility. Pass
  `no_capture: true` for true fire-and-forget (nothing returned). Pass
  `timeout: T` to bound the poll wait.
- **Run R silently and read its output**: `r_exec({code: "<R code>"})`.
  Returns `{output: string}`. Use when silent/background execution is
  specifically required. Default elapsed limit is 2s — pass
  `timeout: T` to extend (or `0` to disable).
- **Open a file at a specific line**: `editor_open({path, line})`.
  Non-modal — the file appears in the Source pane. Use `editor_edit`
  for the modal `edit()` dialog.
- **Read the user's recent console history**: `console_history({limit: N})`.
- **Read RStudio Terminal output**: `term_list` → find the id, then
  `term_buffer({id, limit: N})`. To run a shell command:
  `term_exec({id, code: "<bash>"})` then re-read `term_buffer` after.
- **Surface lint-style markers in the IDE**: `pane_markers({markers: [...]})`
  with `[{type, file, line, column?, message}, ...]`.
- **Render and preview a document in the Viewer pane**:
  - `pane_preview({path})` — auto-detects `.md`, `.Rmd`/`.rmd`, `.qmd`
    from the extension and renders to HTML in the Viewer pane.
    Pass `no_view: true` to render without displaying.
  - Explicit variants: `pane_preview_md`, `pane_preview_rmd`,
    `pane_preview_qmd`. The md/rmd variants accept `output_dir`.
  - Returns `{input, output, format: "html", viewer_loaded}`.
  - These tools lift the socket timeout — long renders are safe.
- **Inspect a variable without loading it**: `env_info({name})` returns
  class/typeof/length/dim/size_bytes only. Use `env_contents({name})`
  for formatted contents.
- **Sync the editor buffer after an external file write**: if you
  modify a file's content via tools other than `editor_*` (e.g.
  shell, `git`, your own write-to-disk), the user's RStudio buffer
  keeps the stale content. Call `editor_reload({path, if_clean: true})`
  to force a reload — safe to call unconditionally (no-op when the
  file isn't open or has unsaved edits).

## Project lifecycle

Five tools under the `project_` prefix:

- **`project_current`** — return the active project path (null if
  none). Read.
- **`project_open(path, new_session?)`** — open an existing project.
  Replaces the session (R restarts) unless `new_session` is true.
- **`project_new(path, scaffold?, git?, no_open?, new_session?)`** —
  create a NEW directory + `.Rproj` template, optionally scaffold
  `R/` + `README.md` + `.gitignore`, optionally `git init`, then
  (default) open. Refuses if the path already exists.
- **`project_init(path, scaffold?, git?, no_open?, new_session?)`** —
  make an EXISTING directory a project: writes a `.Rproj` (refuses
  if one already exists), optional scaffold/git, then open.
- **`project_clone(url, path?, no_open?, new_session?)`** — `git
  clone` the URL, add a `.Rproj` if missing, then open.

`project_open` / `project_new` / `project_init` / `project_clone` are
DISRUPTIVE — they restart the R session unless `new_session: true`.
Make sure you've snapshotted any in-flight R state you care about
before calling these. Same defensive rule as for any sequence: wrap
in a tx if you're chaining (`project_clone` → `editor_open` → … must
use tx_begin/end to be atomic with respect to other agents).

## Hard constraints

- **Never invoke `rpc` with method `client_init`**. It's blacklisted at
  the server level because it invalidates the user's browser client
  and resets their RStudio session.
- **`r_send` and `term_send` / `term_exec` are visible**. The user
  sees them in their UI. **Default to `r_send` when the user should
  see what runs**; reserve `r_exec` for truly silent/background
  operations. `r_send` (without `no_capture`) holds the session lock
  while polling for the captured result.
- **Modal UI tools (`ui_*`) BLOCK** until the user dismisses them.
  Don't chain them in long sequences — the user has to interact
  between every call.

## Discovery

- `meta_status` — current session info, R version, open documents, lock
  state. Good first call to confirm the bridge is healthy.
- `tools_search` — find tools by keyword or category (see the
  "Discovering tools" section above). This is the canonical way to
  reach the ~85 tools that aren't in the core `tools/list`.
- `observe_events` — static catalog of every event type emitted by
  `observe_stream` (which you can invoke from a shell context if
  needed; not recommended through MCP since it never returns). Reach
  it via `tools_search({query: "observe events"})` or call directly.
