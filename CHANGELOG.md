# Changelog

All notable changes to **rstudio-cli** are documented here. The format is based
on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.19.0] — 2026-06-09

### Added

- **First-class support for R's debugger** (`browser()`, `debug()`,
  `recover()`). The CLI is now aware when R is at a `Browse[n]>`
  prompt and adapts every R-execution surface accordingly.

  - **`r send` and `r exec` are now browser-aware**. When the active
    R session is at a `Browse[n]>` prompt, both commands automatically
    evaluate user code in the frame being debugged — not in
    `.GlobalEnv`. This means `r send 'y'` reads the debugged
    function's local `y`, `r exec 'ls()'` lists the locals of the
    current frame, and assignments persist after the user types `c`.
    Detection is best-effort and free: `r send` uses the
    `context_depth` field already present in `get_environment_state`,
    `r exec` walks `sys.calls()` server-side to find the user/rsession
    frame boundary (zero extra RPCs).

  - **Every `r exec` / `r send` response now carries an `eval_env`
    field** that confirms where the code actually ran. Values:
    `{kind: "global"}`, `{kind: "attached", name}`,
    `{kind: "browser_frame", function, depth}`,
    `{kind: "top_level"}` (for `r exec` outside a debugger), or
    `{kind: "background_job"}` (for `r exec --async` and `r poll`).
    Agents reading the response know unambiguously which scope was
    targeted, without needing to re-poll session state.

  - **New `debug` subcommand category** with six actions:
    - `debug status` — full debugger state (depth, current frame,
      source location, typed locals, call stack), projected from
      `get_environment_state`.
    - `debug step <n|s|f|c|Q|where|help|r>` — push a browser
      meta-command via `console_input`, without ever wrapping it in
      `ℝ(~{…})`. Refuses with `kind=user_error` when no debugger is
      active.
    - `debug where` — call stack only (subset of `debug status`).
    - `debug locals` — typed locals of the current frame only.
    - `debug src` — `{file, line}` of the current frame (when a real
      `srcref` is available).
    - `debug exit` — alias of `debug step Q`.

  - **`rstudio status` gains a `rsession.debugger` field**: `null` at
    the regular prompt, `{in_browser: true, depth, function}` while a
    debugger is active. Surfaced in the `--format text` rendering too.
    Costs one extra RPC at session start; agents land oriented.

  - **Three new `observe` events at Tier 2**: `debug.entered`,
    `debug.exited`, `debug.frame_changed`. Emitted on transitions of
    `get_environment_state.context_depth` and `environment_name`.
    The initial snapshot also emits `debug.entered` when observe
    attaches while a debugger is already active.

  - **Embedded skill updates** — both `src/skills/rstudio.md` (CLI)
    and `src/skills/rstudio-mcp.md` (MCP) gain a *Debugging workflow*
    section documenting the new surface, the `eval_env` envelope
    contract, and the "don't send `r send 'n'`" footgun.

## [0.18.2] — 2026-06-09

### Fixed

- **Embedded `rstudiocli` companion package now installs into a
  CLI-managed library**, out of reach of any renv project shim.
  Previously the install used the default user library, which under
  renv-active sessions either landed the tarball in the renv cache
  without symlinking the project library (so the immediate
  `requireNamespace()` check failed with "install.packages reported
  success but 'rstudiocli' still not findable") or polluted the
  user's `renv.lock` with an internal-only package. The new behaviour
  installs into `tools::R_user_dir("rstudio-cli", "data")` and
  prepends that directory to `.libPaths()` for the duration of the
  R session. The lib is XDG-respecting, dedicated, and invisible to
  the user's project state. Idempotent: the prepend is wrapped in
  `unique()` and is now run at the top of every probe so freshly-
  restarted rsessions self-heal on the next CLI call.

## [0.18.1] — 2026-06-09

### Fixed

- **Clearer error message when no browser tab is bound to the rsession.**
  Previously, when `session-persistent-state` was missing or had no
  `active-client-id`, or when an RPC was rejected twice in a row with
  code 4 (`Invalid client id`) or code 6 (`Invalid json-rpc request`)
  after re-reading the file, the surfaced wording ("No active browser
  client found … Open RStudio in your browser first") read to LLM
  agents as "no rsession found" and led to wrong diagnoses (users
  searching for a missing session that was, in fact, present). The
  rewording now says explicitly "RStudio session is detected, but no
  browser tab is currently bound to it. ACTION: open or refresh the
  RStudio tab in your browser, wait for it to finish loading, then
  retry." The double-failure RPC retry path in `RpcClient::rpc` now
  also rewraps the second-attempt error with the same actionable
  message instead of letting the raw `RpcError` bubble up.

## [0.18.0] — 2026-06-03

### Changed

- **`callr` is now an officially required dependency.** It was already
  declared in `Imports:` of the `rstudiocli` companion R package (so
  `R CMD INSTALL` already refused to proceed without it), but the
  Rust-side `R_HARD_DEPS` precheck explicitly excluded it, the error
  message claimed it was optional, and the `r exec --async` call site
  guarded itself with a `requireNamespace("callr", ...)` check.
  This inconsistency caused opaque "had non-zero exit status" install
  failures for users missing `callr` — now they get the same upfront,
  actionable message that already shielded missing `rstudioapi` /
  `jsonlite`: a clear list of missing packages and the exact
  `install.packages(...)` command. The dead defensive check at the
  `r exec --async` site is removed; the precheck is the single source
  of truth.

### Added

- **Three new "stop running R" commands**, one per surface (no umbrella
  command — each surface has different semantics):
  - `r interrupt` — equivalent of the console pane's Stop button.
    Fires the rsession `interrupt` JSON-RPC; the foreground R execution
    aborts and any blocked `r send` resolves with `kind=r_error`,
    `message="R execution was interrupted"`. Returns `{interrupted: true}`.
  - `r kill <id> [--tree]` — terminate an async job started with
    `r exec --async`. Calls callr's `process$kill()` (SIGTERM), or
    `process$kill_tree()` with `--tree` to also reap descendants.
    Idempotent: `{status: "killed" | "already-done"}`.
  - `job kill <id>` — stop a job in the Jobs pane (created via
    `job add` or `job run-script`). Best-effort: fires the rsession
    internal `execute_job_action` RPC with `action="stop"` (swallowed
    on unknown-method to stay compatible across RStudio versions),
    then always flips the UI state via `jobSetState(id, "cancelled")`.
    Returns `{cancelled: true, hard_killed: bool}` so callers can tell
    whether the underlying sub-process was actually reaped or only
    the pane label changed.

## [0.17.0] — 2026-05-15

### Added

- **Hard-dependency pre-check.** Before installing the embedded
  `rstudiocli` R package, the CLI now probes for `rstudioapi` and
  `jsonlite` via `requireNamespace()`. If either is missing, every
  CLI/MCP call fails early with an actionable error message:
  "Nothing was opened, run, or modified in RStudio." followed by the
  exact `install.packages(…)` command. `callr` remains optional (gated
  at the `rstudio r --async` call site).

## [0.16.1] — 2026-05-15

### Fixed (R side)

- `editor_new()`: create the document empty first, wait one throttle
  interval, then write the content via `setDocumentContents()`. The
  previous single-call `documentNew(text = ...)` performed both steps
  internally without a pause, leaving ghost lines in the editor buffer.
- `console_activate()`, `job_add()`, `job_remove()`, `job_run_script()`:
  added the missing `.throttle()` call after each UI-mutating
  `rstudioapi` invocation. Discrete UI actions (opening a job, giving
  focus to the console, …) need the same browser-refresh margin as
  editor operations.

## [0.16.0] — 2026-05-15

### Changed (breaking, R side)

Five `rstudiocli::*` wrappers were renamed to mirror the MCP / CLI
naming convention (`category.action` with hyphens → underscores). This
is a breaking change for any user code that imported the old names —
pre-1.0 we don't ship aliases.

| Before                                   | After                                |
|------------------------------------------|--------------------------------------|
| `rstudiocli::editor_get_contents()`      | `rstudiocli::editor_read_buffer()`   |
| `rstudiocli::editor_document_path()`     | `rstudiocli::editor_path()`          |
| `rstudiocli::editor_select_range()`      | `rstudiocli::editor_select()`        |
| `rstudiocli::pane_files_navigate()`      | `rstudiocli::pane_files()`           |
| `rstudiocli::ui_dialog_update()`         | `rstudiocli::ui_update_dialog()`     |

### Added

- **16 new `rstudiocli::*` exports** for `r_script` parity. Every
  rsession-touching MCP / CLI action that has a sensible R equivalent
  now lives in the R package, so an LLM orchestrating a multi-step
  workflow via `r_script` can call them by name (instead of routing
  through `tx_run` as a workaround):
  - `editor_read`, `editor_list`, `editor_reload`, `editor_set_marks`
  - `env_list`, `env_contents`, `env_info`
  - `console_history`, `console_context`
  - `pane_preview`, `pane_preview_md`, `pane_preview_rmd`, `pane_preview_qmd`
  - `project_new`, `project_init`, `project_clone`
  - `session_list`

  `console_actions` is left in CLI-only territory (couples to RStudio's
  private on-disk format); `meta_*`, `observe_*`, `policy_*`,
  `schema_*`, `skill_*` are CLI infrastructure, not rsession-touching;
  `r_exec` / `r_send` / `r_poll` would deadlock if called from inside
  `r_script` itself.

### Fixed

- `r_package::install_from_embedded` no longer muffles
  `install.packages()` warnings. Previous versions wrapped the call
  in `withCallingHandlers(warning = invokeRestart('muffleWarning'))`
  to keep the CLI's auto-install silent — but `install.packages()`
  signals NAMESPACE-mismatch failures **through warnings**, and the
  muffler dropped them on the floor. Symptom: install reported
  success, every subsequent call returned 'there is no package called
  rstudiocli' with no actionable diagnostic. The new code lets
  warnings bubble up as `CliError`, plus a `requireNamespace()` check
  confirms the install actually landed before returning.

  This bug bit us during the 0.16.0 development cycle (a few hours
  of WTF before noticing) so it earns a CHANGELOG line.

### Added — R package dependencies

- `utils` is now an explicit `Imports`. New wrappers use
  `utils::savehistory` (console_history) and
  `utils::capture.output` (env_contents).
- `markdown`, `rmarkdown`, `quarto` added to `Suggests` (runtime
  `requireNamespace()` gates in `pane_preview_*`). Users who never
  preview a document pay no cost.

## [0.15.1] — 2026-05-15

### Fixed

- **MCP `tools/list` now exposes every action.** Earlier versions
  surfaced only a 7-tool bootstrap core and expected agents to discover
  the rest via `tools_search`. Claude Code 2.x's *ToolSearchTool*
  refused to dispatch tools absent from `tools/list` (it considers the
  catalog authoritative) and agents fell back to routing everything
  through `tx_run` — see the diagnostic walkthrough on PR-merge day.

  This release switches to the *deferred tools* pattern Claude Code is
  actually designed for: every MCP tool ships in `tools/list`, and the
  bootstrap core carries the `_meta["anthropic/alwaysLoad"] = true`
  annotation Claude Code recognises (the rest defer cleanly, surfaced
  via `system-reminder` and reachable through ToolSearch on demand).
  Non-Claude-Code clients ignore `_meta` and load every schema upfront,
  which is also fine — same behaviour as Claude Code's "standard" mode.

  Inspired by David Soria Parra's
  [*The Future of MCP*](https://www.youtube.com/watch?v=v3Fr2JR47KA)
  talk; references in the README and code comments call it out.

### Changed

- `src/skills/rstudio-mcp.md` rewritten to describe the new catalog
  layout (bootstrap core + deferred registry) instead of the old
  progressive-discovery story.

## [0.15.0] — 2026-05-15

The MCP design choices the project already had since 0.13.x (progressive
discovery via `tools_search`, tool composition via `r_script`, atomicity
via `tx_begin` / `tx_end` / `tx_run`) line up with the patterns David
Soria Parra (Anthropic) summarised in his [*The Future of MCP*](https://www.youtube.com/watch?v=v3Fr2JR47KA)
talk. Worth a watch if you're wondering why the catalog isn't all flat
in `tools/list`. The README's *MCP server* section now references the
talk and walks through the three patterns explicitly.

### Changed

- **R companion package renamed: `rstudiocli.mcp` → `rstudiocli`.** The
  package is used by every CLI command (not just MCP), so the `.mcp`
  suffix was misleading. **Breaking** for users who had installed
  `rstudiocli.mcp` 0.14.0 manually: uninstall it before this release
  (`remove.packages("rstudiocli.mcp")`), then let the auto-install
  reinstall it under the new name. The internal `.rstudio_mcp_build_id()`
  helper is also renamed to `.rstudiocli_build_id()`.
- **`editor read-buffer` no longer returns `dirty`.** The wrapper now
  goes through `rstudioapi::getSourceEditorContext()` (live buffer)
  instead of the raw `get_source_document` RPC, which was returning
  the pre-modification buffer for ~1 s after a `set-contents` /
  `modify-range`. `getSourceEditorContext()` doesn't expose `dirty`;
  callers that need it can pull it from `editor list`, which still
  surfaces `dirty` for every open document.

### Added

- **`callr` and `jsonlite` declared as `Imports`** of the `rstudiocli`
  R package, so the auto-install pulls them automatically. Previously
  required manual `install.packages()` for the `r exec --async` path.
- **Internal `rstudiocli:::.throttle()` between UI-mutating ops.** Every
  wrapper that touches the editor / pane / terminal state (open, close,
  set-contents, modify-range, set-cursor, viewer, markers, term create /
  send / kill / …) now sleeps briefly after the rstudioapi call so the
  GWT client (Chrome / Electron) has time to acknowledge the event
  before the next call lands. Default 500 ms, override via
  `options(rstudiocli.throttle_ms = N)` or `RSTUDIOCLI_THROTTLE_MS=N`;
  `0` disables it entirely. Solves a back-to-back-RPC saturation issue
  observed against rocker/rstudio in CI.

### Internal — test harness

- **Self-contained Docker test harness (`scripts/bridge-up.sh`).** A
  rewrite of the integration test scaffold: everything (rserver, rsession,
  headless Chromium, the cargo toolchain, the compiled test binary)
  now runs inside a single `rocker/rstudio:4.5.2` container. No host
  Chrome, no host Python, no host socat, no bind-mounts — `docker cp`
  pushes sources in, a named volume keeps the cargo cache warm.
  Sidesteps a 1-on-1-off `accept()` bug observed when tunneling from
  the host. New commands: `test-live` (live tests only) and `test-all`
  (full local-preflight gauntlet against the Linux target).
- **42 live integration tests** covering status, session, pref, pane,
  job, term, editor, env, console, project, r exec, r send. Was 13.
- **CI: new `live` job** that runs the full live test suite on every PR
  and every push to main via the in-container harness. ~2 min on a warm
  Docker pull. Runs after the existing `fmt` / `clippy` / `test` jobs,
  so a plain compilation regression fails fast.

## [0.14.0] — 2026-05-14

### Added

- **Embedded R companion package `rstudiocli.mcp`.** A real CRAN-style R
  package, source tree in `r-package/`, gets packaged into the binary
  at compile time (`build.rs`) and auto-installed at first use into
  the user's R library (silent, policy 1). Single source of truth for
  the R-side surface used by both the CLI and human R users:
  `library(rstudiocli.mcp); editor_set_contents(...)` works directly
  from any R session. The package now wraps ~50 endpoints across all
  categories (editor, term, pane, job, ui, session, pref, project,
  console, status). Roxygen-documented, testthat-tested, R CMD check
  clean.
- **MCP `r_script` tool — programmatic tool calling.** Send an R
  script that orchestrates multiple actions; only the final value is
  returned to the agent. Intermediate data (buffer contents, env
  dumps) never traverses the LLM's context window. Forbidden inside
  an active `tx_begin` (would deadlock); the server rejects the
  combination with a clear message.
- **Compile-time version sync + build-id** (`build.rs`):
  `Cargo.toml::version` must match `r-package/DESCRIPTION::Version`
  (mismatch fails the build). A content-hash of the package source
  tree is baked in as a build-id and exposed via
  `rstudiocli.mcp:::.rstudio_mcp_build_id()` — the runtime install
  check compares this hash so any change to the R package (even
  within the same Cargo version) triggers a reinstall. Stale
  loaded namespaces are `unloadNamespace()`-ed after install so
  newly-shipped exports take effect immediately.

### Internal

- New `src/r_package.rs` module: tarball embed + auto-install
  (memoised per-process via `OnceLock`, recursion-safe re-entry
  through the RPC layer).
- `RpcClient::rpc()` now calls `r_package::ensure_installed()` once
  per process before any RPC — guarantees that any code path that
  reaches rsession finds the companion package available.
- `tempfile` moved from dev-dependencies to dependencies.
- The legacy `format!("rstudioapi::...")` call sites in
  `src/commands/*.rs` (95+ of them) have all migrated to
  `format!("rstudiocli.mcp::...")`, with the exception of two
  intentionally-kept sites (`editor insert` end-of-document
  resolution and `console context` selection projection) plus the
  `document_position` / `document_range` constructors that aren't
  endpoints.

## [0.13.0] — 2026-05-14

### Changed (breaking)

- **`rstudio schema` (level 0) now returns only the 15 categories** with
  their description and an `action_count`, instead of the full flat list
  of every action with category + summary. Pick a category and call
  `rstudio schema <cat>` for the actions, or use the new shortcut
  `rstudio schema --search '.*'` to recover the legacy flat output.
  Levels 1 and 2 are unchanged. Motivation: align with the MCP server's
  new progressive-discovery surface and reduce default discovery cost
  for agents.

### Added

- **MCP server: progressive discovery via `tools_search`.** `tools/list`
  now returns only a small core set (`meta_version`, `meta_status`,
  `tools_search`, `tx_begin`, `tx_end`, `tx_run`) instead of all ~90
  registry-derived tools. Other tools remain callable via `tools/call`
  and discoverable through the new `tools_search` tool, which mirrors
  the 3-level drill-down of `rstudio schema`:
  - `tools_search({})` — list of categories with `action_count`.
  - `tools_search({category: "editor"})` — actions in that category.
  - `tools_search({category, action})` — full ActionSpec, augmented with
    the MCP `input_schema` and `mcp_tool_name` ready for `tools/call`.
  - `tools_search({search: "<regex>"})` — matching actions across all
    categories (regex on category|name|summary).
  Net effect: the fixed per-turn cost of `tools/list` drops from ~2 300
  to ~1 000 tokens (-57%) for every connected agent.
- **`schema::browse()`** shared helper used by both `rstudio schema`
  (CLI) and `tools_search` (MCP) so the two surfaces stay in lockstep.
- **`scripts/bench_discovery.py`** — tokenizer-based bench
  (cl100k_base / tiktoken) that measures the cost of each drill-down
  level vs. the pre-progressive baseline. Run with
  `uv run --with tiktoken scripts/bench_discovery.py`.

## [0.12.4] — 2026-05-12

### Fixed

- **`r send` — evaluate in the active RStudio Environment pane scope.**
  Previously `ℝ()` always called `eval(..., envir = globalenv())`, so code
  sent to the console was evaluated in the global environment even when the
  user had selected an attached data frame in the Environment pane. The
  helper now queries `get_environment_state` before installing `ℝ` and
  resolves the target environment at runtime: `.GlobalEnv` maps to
  `globalenv()`; any other name (e.g. after `attach(df)`) is resolved via
  `as.environment(match(name, search()))` with a `globalenv()` fallback.

### Added

- **23 live smoke tests** (`tests/live.rs`) against a real RStudio session.
  Covers `r exec` (basic, R error, timeout, async + poll), `r send`
  (stdout capture, message capture, R error, multiline, `--no-capture`,
  attached-env evaluation, invisible value, mixed stdout + message),
  `env` (list, pattern filter, info, contents), `console` (history,
  context), `editor` (read, list), `term` (list), `project` (current),
  and schema registry shape. All 23 tests are serialised through a
  process-wide mutex to avoid Desktop rsession async-handle collisions.
  Tests that create R variables clean up with `r exec` (synchronous);
  skip silently when no live session is reachable.

## [0.12.3] — 2026-05-10

### Changed

- **`rstudio skill show` — accepts `--for` and `--target`, byte-identical
  output to `install`.** Previously `show` printed the embedded template
  with a generic update command, while `install` baked the exact path and
  reinstall command for the destination. Piping `show` to a custom location
  (`rstudio skill show > /path/to/skill.md`) thus produced a self-update
  section that lied about being baked. Now both commands share the same
  pure path-resolution and rendering helpers (`compute_target_file`,
  `render_for`); `show --for X --target Y` produces exactly what
  `install --for X --target Y` would write at that location. Update
  section reformatted with the path and command on their own indented
  lines for readability. New `__UPDATE_SECTION__` block placeholder
  replaces the previous single-line `__UPDATE_COMMAND__`.

## [0.12.2] — 2026-05-10

### Changed

- **`rstudio skill install` — bake the exact reinstall command into the
  installed SKILL.md.** When the user passes `--target <dir>` (or a
  non-default `--for <tool>`), the skill's self-update section now shows
  the precise command needed to overwrite *that specific file* — not the
  generic `rstudio skill install --force`, which would refresh only the
  default `~/.claude/skills/` copy and leave a custom-located skill stale.
  Implemented via a new `__UPDATE_COMMAND__` placeholder substituted at
  install time alongside `__VERSION__`. Paths with shell metacharacters are
  POSIX single-quoted. `rstudio skill show` keeps the generic command since
  it has no install context.

## [0.12.1] — 2026-05-08

### Fixed

- **`r send` — suppress warning on `rm(list = ls())`.**
  When user code called `rm(list = ls())` inside `ℝ(~{ ... })`, the helper
  removed `ℝ` from `.GlobalEnv` before `on.exit` could do it. The subsequent
  `rm("ℝ", ...)` in `on.exit` emitted a visible warning (`object 'ℝ' not
  found`). Wrapped the call in `suppressWarnings()` — `try(..., silent =
  TRUE)` only suppresses errors, not warnings.

### Changed

- **New logo** — hexagonal sticker in Posit proportions (W/H = √3/2). Dark
  navy fill (`#1A3654`), blue accent border, ℝ (double-struck R, U+211D)
  centred in blue, green terminal prompt `>` and cursor block, `rstudio-cli`
  wordmark inside the hex. Replaces the previous light-background hexagon.

## [0.12.0] — 2026-05-08

### Added — `r send` output capture + automatic update check

**`r send` now captures output while keeping code visible.**
Previously, `r send` was fire-and-forget: code appeared in the user's
R console but the agent received nothing back, pushing agents toward
`r exec` (silent). `r send` now installs a helper `ℝ` in `.GlobalEnv`,
sends `ℝ(~{ code })` via `console_input`, polls a per-session sentinel
file (`/tmp/rstudio_cap_<pid>.json`), and returns
`{stdout, messages, error}` — the same payload as `r exec`, but fully
visible to the user. `ℝ` uses a tilde-formula parameter (`f[[2]]`)
instead of NSE, self-removes from `.GlobalEnv` via `rm()` at the end of
its `on.exit`, and handles R-side interruptions gracefully. Pass
`--no-capture` for the old fire-and-forget behaviour.

**Automatic update check (background, TTL 24 h).**
At each invocation the CLI reads a platform-appropriate cache file
(`dirs::cache_dir()/rstudio-cli/update-check.json`). When the cache is
older than 24 hours a background thread refreshes it via
`curl api.github.com/repos/…/releases/latest` without blocking the
call. In CLI mode a bare notice is printed on `stderr` when a newer
version is cached. In MCP mode a `_update_available` field is injected
into every tool response, so the agent sees it regardless of which tool
it called. Set `RSTUDIO_CLI_NO_UPDATE_CHECK=1` to opt out.

## [0.11.2] — 2026-05-07

### Fixed — MCP error propagation and RPC race condition after Markers pane click

**`invoke_action` now signals failures correctly (`isError: true`).** When the
CLI subprocess returned `{"ok": false, …}`, the MCP server was forwarding it as
`isError: false` — the LLM had no way to know the tool had failed. The envelope
is now checked and any `ok: false` result is turned into an `Err`, which
`handle_tools_call` maps to `isError: true`.

**Retry on transient RPC error 6.** Clicking a marker in RStudio's Markers pane
triggers a browser→rsession state transition. If a subsequent MCP tool call
arrives during that transition, rsession rejects it with code 6 ("Invalid
json-rpc request"). `RpcClient::rpc` now retries once after 200 ms and a
client-id refresh when it receives code 6, matching the existing retry for
code 4.

**`ParamKind::Json` MCP schema now accepts arrays.** The `inputSchema` for JSON
parameters was `{"type": "object"}`, causing schema-validators to reject `--markers`
(which is an array). Changed to `{"anyOf": [{"type": "object"}, {"type": "array"}]}`.

**Integer coercion in `pane markers`.** `rstudioapi::sourceMarkers` requires
integer `line`/`column` values. JSON numbers are doubles by default; the R code
now explicitly coerces both columns with `as.integer()`.

12 new unit tests cover all four fixes.

## [0.11.1] — 2026-05-04

### Fixed — `pane preview-md` compatibility with `markdown` < 1.0

`markdown::mark_html()` was introduced in the `markdown` package >= 1.0
(API rewrite). Environments running an older version only expose
`markdownToHTML()`. The R code now dispatches at runtime:

```r
if (utils::packageVersion("markdown") >= "1.0") {
  markdown::mark_html(f, output = out)
} else {
  markdown::markdownToHTML(f, output = out)
}
```

`pane preview-md` and `pane preview` (when auto-dispatching to Markdown)
are both affected.

## [0.11.0] — 2026-05-03

### Added — document preview actions (`pane preview`, `preview-md`, `preview-rmd`, `preview-qmd`)

Four new actions in the `pane` category let an agent render and preview
Markdown, R Markdown, and Quarto documents directly in the RStudio
Viewer pane:

| Action | Format | Render engine |
|---|---|---|
| `pane preview <path>` | auto-detect from extension | dispatches to one of the three below |
| `pane preview-md <path>` | `.md` | `markdown::mark_html()` |
| `pane preview-rmd <path>` | `.Rmd` / `.rmd` | `rmarkdown::render(output_format="html_document")` |
| `pane preview-qmd <path>` | `.qmd` | `system2("quarto", c("render", …, "--to", "html"))` |

Common flags:

- `--no-view` — render to HTML but skip the `rstudioapi::viewer()` call
  (useful for CI or when you only need the output file).
- `--output-dir <dir>` — redirect the rendered HTML to a specific
  directory (`preview-md` and `preview-rmd` only; quarto always
  outputs next to the source).

All four commands lift the socket timeout (via `EvalTimeout::NoLimit`)
because rendering can take tens of seconds for complex documents.

The unified `pane preview` auto-detects the format from the file
extension. For full control (e.g. `--output-dir`), use the explicit
per-format variants.

Return value: `{input: string, output: string, format: "html", viewer_loaded: bool}`.

Auto-exposed via MCP as `pane_preview`, `pane_preview_md`,
`pane_preview_rmd`, `pane_preview_qmd`.

Action count: 93 → 97.

Tests: 8 unit tests in `src/commands/pane.rs` — `detect_format` for
all four extensions + the no-extension / unknown-extension error paths;
plus three output-path derivation tests (one per format).

### Internal — skill files updated

Both `src/skills/rstudio.md` and `src/skills/rstudio-mcp.md` updated
with the new preview patterns. Per CLAUDE.md convention, any skill
change requires a version bump (so `rstudio skill install --force`
delivers the update).

## [0.10.0] — 2026-05-04

### Changed (BREAKING) — `project` is now a top-level category

Project lifecycle commands move out of `session` into a dedicated
`project` category. Migration is a one-token edit:

| Before (v0.9.x) | After (v0.10.0) |
|---|---|
| `rstudio session project` | `rstudio project current` |
| `rstudio session open-project <path>` | `rstudio project open <path>` |
| MCP tool `session_project` | `project_current` |
| MCP tool `session_open_project` | `project_open` |

The `session` category retains only what genuinely concerns the R
session: `info`, `restart`, `list`. This is a clean break — the old
spellings don't work anymore. Pre-1.0 SemVer permits breaking changes
in a minor bump; we believe adoption is low enough that the
ergonomics win is worth the migration cost.

### Added — three new project creation actions

Three new actions under the `project` category cover the full
lifecycle from directory to open RStudio project:

- **`project new <path>`** — creates a NEW directory + a default
  `.Rproj`, optionally scaffolds (`R/` + `README.md` + `.gitignore`),
  optionally `git init`, then (by default) opens. Refuses if the
  path already exists.
- **`project init <path>`** — makes an EXISTING directory a project:
  writes a `.Rproj` (refuses if one already exists), optional
  scaffold/git, then opens. Useful to upgrade plain R workspaces.
- **`project clone <url> [<path>]`** — `git clone` the URL, add a
  `.Rproj` if the repo doesn't have one, then open. The destination
  path defaults to the URL's basename minus `.git`.

The three creation commands share helpers (`write_rproj_file`,
`scaffold_dir`, `git_init`, `git_clone`) and emit a uniform JSON
result `{path, rproj, scaffolded, git_initialized, opened}`.

**Pure-Rust filesystem path**: with `--no-open`, `project new` /
`init` / `clone` do not require RStudio to be running. They only
write files (and optionally invoke `git`). This makes them safe to
use in CI scripts, headless setup, or before launching RStudio for
the first time.

Auto-exposed via MCP as `project_new`, `project_init`, `project_clone`
(plus `project_open` / `project_current`).

Action count: 85 → 88. Categories: 16 → 17.

Tests: 5 unit tests in `src/commands/project.rs` cover the helpers
(URL parsing for clone, .Rproj template write + overwrite refusal,
scaffold idempotence, .Rproj detection).

### Added — `Skill change ⇒ version bump` convention in CLAUDE.md

Both skill files (`src/skills/rstudio.md` and
`src/skills/rstudio-mcp.md`) are embedded at compile time, so any
non-trivial change requires a new binary version for users to see
the update. Codified explicitly in CLAUDE.md so future contributors
remember to bump.

### Internal — `.gitignore` hardened

Added `.Rhistory`, `.RData`, `.Ruserdata` to the project's `.gitignore`
so working in the repo doesn't leak R session state.

## [0.9.2] — 2026-05-04

### Documentation — MCP server installation procedure

Adds full MCP install coverage in two surfaces:

- **README** (`## Install / ### MCP server`): per-client procedures
  for Claude Code, Claude Desktop (with full `claude_desktop_config.json`
  snippet), Cline / Continue / Cursor, plus a stdin verification
  one-liner and a note on cross-surface coexistence.
- **Embedded CLI skill** (`src/skills/rstudio.md`): the `## MCP server
  mode` section now lists the same per-client variants instead of only
  Claude Code. Since the skill is embedded at compile time, the binary
  needs a new version to ship the updated content; that's the
  motivation for the bump despite this being a doc-only release.

No code changes. Bumps Cargo.toml 0.9.1 → 0.9.2.

## [0.9.1] — 2026-05-04

### Added — MCP server returns agent guidance via `initialize.instructions`

The MCP server's `initialize` response now carries an `instructions`
field with cross-cutting agent guidance — the things an agent connected
via MCP can't infer from per-tool descriptions alone:

- the defensive `tx_begin`/`tx_end`/`tx_run` rule for multi-call
  sequences and the "you can't reliably know if you're alone" framing
- which tools NOT to put inside a tx (`observe_stream`, `ui_dialog` /
  `ui_*` modals)
- R FIFO concurrency model and what `r_send` / `r_exec` differ on
- hard constraints (never call `rpc` with `client_init`, etc.)
- patterns worth knowing (markers, async R, env_info vs env_contents,
  editor_reload after external file write)

The content lives in `src/skills/rstudio-mcp.md`, embedded at compile
time via `include_str!` and substituted with the binary version at
runtime — same pattern as the existing CLI skill (`src/skills/rstudio.md`).
Two distinct skills now: the CLI skill is for shell-driven agents
(installed into `~/.claude/skills/` via `rstudio skill install`); the
MCP skill is for MCP-driven agents (returned by the server during
handshake). They overlap on semantics but the vocabulary differs.

CLAUDE.md updated to record the dual-skill convention so future
features land in both surfaces in sync.

## [0.9.0] — 2026-05-04

### Added — `rstudio mcp`: native MCP server over stdio

Exposes the entire CLI surface as **Model Context Protocol** tools.
Configure once with your MCP client and Claude Code / Cline / Cursor /
Continue / Claude Desktop see ~90 native tools (`editor_open`,
`editor_read_buffer`, `r_exec`, `observe_events`, `meta_status`, …)
in their tool catalog. No more shell-quoting issues, no more parsing
JSON envelopes manually — the LLM calls them like any other tool.

```sh
# One-time setup with Claude Code:
claude mcp add rstudio --scope user -- rstudio mcp
```

The protocol is JSON-RPC 2.0 over stdio. We implement `initialize`,
`tools/list`, `tools/call`, `ping`, and gracefully handle
notifications, parse errors (-32700), unknown methods (-32601).

**Tool dispatch** = subprocess spawn (`rstudio <category> <action>
[args]`), which reuses 100% of the existing dispatch + per-call lock
infrastructure. Subprocess overhead (~10ms) is negligible at LLM
pace. The category-action arg translation is auto-derived from the
`ActionSpec` registry: positionals stay positional, `--flag` params
become object properties in the tool's `inputSchema`.

**Multi-agent transactionality** transposes from CLI to MCP via three
new MCP-native tools:

- `tx_begin` — server acquires the per-session writer lock, holds it
  in struct state.
- `tx_end` — drops the lock (kernel close on FD).
- `tx_run` — script-style: takes `{operations: [{tool, arguments}]}`,
  runs all under one tx with auto-cleanup on error.

While in tx, the server sets `RSTUDIO_TX_HELD=1` on every subprocess;
child rstudio invocations detect the env var and skip their own
per-call lock — same fork-inherit pattern as `rstudio tx -- <child>`.
The MCP server plays the role of "tx parent" instead of a shell.

**Cross-surface coherence**: a Claude Code MCP server, a Cline MCP
server, a shell `rstudio editor write`, and a human in `rstudio tx --
bash` all contend on the same `~/.config/rstudio-cli/locks/session-<id>.lock`.
The kernel arbitrates. No competing tool offers comparable
cross-surface multi-agent safety.

Tests:
- 6 unit tests in `src/commands/mcp.rs` cover name mapping (mcp_name
  / build_argv / build_input_schema), initialize handler, tools/list
  shape.
- 12 integration tests in `tests/mcp.rs` drive the binary as a
  subprocess: initialize, tools/list, ping, notification produces no
  response, parse error → -32700, unknown method → -32601, unknown
  tool → isError=true, observe_events without RStudio. The
  RStudio-dependent ones (tx_begin, tx_run, status visible in tx)
  skip cleanly when no session is reachable.

Skipped via MCP: `meta_tx` (replaced by tx_begin/end/run), `observe
stream` (long-running — agent should invoke via Bash if needed),
`tx` itself (CLI-only). Everything else flows through.

Bumps Cargo.toml 0.8.2 → 0.9.0 (minor — substantial new feature, no
breaking changes to existing surface).

## [0.8.2] — 2026-05-04

### Added — `rstudio observe replay`

Third subcommand under `observe`: replay a previously captured JSONL
stream at the original cadence (or scaled). Reads the input file (or
`-` for stdin), forwards every line to stdout, sleeping between lines
to respect the original `ts` timestamps. Lines with malformed JSON or
missing `ts` are forwarded as-is without affecting the timing baseline.

```sh
# Capture
rstudio observe stream > /tmp/session.jsonl

# Replay at original speed
rstudio observe replay /tmp/session.jsonl

# Replay 10x faster
rstudio observe replay /tmp/session.jsonl --speed 10

# Replay instantly (for tests / downstream consumer stress)
rstudio observe replay /tmp/session.jsonl --speed 0

# Stdin pipeline
cat /tmp/session.jsonl | rstudio observe replay -
```

Does NOT require an RStudio session — reads disk, writes stdout, no
RPC. Useful for: reproducible CI tests, post-mortem debugging,
offline demos, stress-testing JSONL consumers.

SIGPIPE is reset to default so `rstudio observe replay session.jsonl
| head -n 5` exits cleanly.

Bumps action count 84 → 85 (new `observe replay` action).

Tests: 6 unit tests in `src/commands/observe.rs` cover the ISO 8601
parser (`iso_to_epoch_ms`) and its round-trip with `iso_now()`. 7
integration tests in `tests/replay.rs` exercise the binary with
real JSONL fixtures (forwarding order, --speed 0/1/10 timing,
stdin input, malformed lines, missing file).

## [0.8.1] — 2026-05-04

### Added — `session.lock` field in `rstudio status`, plus `meta` schema category

Three small additions that make the multi-agent locking design
discoverable by an agent that's reading the schema and querying status,
without having to read the README or skill first:

- **`status.session.lock`** — a moment-in-time read of the per-session
  lock state: `{state: "free" | "held", holder: {pid, command,
  started_ms} | null, inside_tx: bool}`. Information-only; the holder
  can release between the read and the next call. Use to debug
  timeouts or surface "another agent is active" awareness, never as a
  control-flow gate.

- **`schema meta`** category, with three actions: `version`, `status`,
  `tx`. These were previously top-level commands without schema
  entries, invisible to agents discovering surface via `rstudio
  schema`. The `meta tx` action description carries the full
  multi-agent transactional contract: when to use tx, what
  `RSTUDIO_TX_HELD` means, what NOT to put inside it, the
  serialisation-vs-ACID note, the defensive-default rule.

- **Embedded skill update** (`src/skills/rstudio.md`) — explicit
  defensive rule for agents: there's no reliable "am I alone" check,
  so always wrap multi-call write sequences in `rstudio tx --`,
  regardless of perceived solitude. Cost when alone: ~10ms (fork);
  cost when not alone without tx: silent data loss.

Bumps action count 81 → 84 (one per meta-CLI command), category count
15 → 16 (the new `meta` category).

### Fixed — Integration tests serialise on shared session lock

`tests/locking.rs` now uses a global `Mutex` to serialise tests that
all target the same per-session `flock` on the dev machine's live
RStudio session. Previously cargo's parallel test runner caused them
to contend with each other (a test expecting an immediate acquire
would instead wait for another test's `sleep`). All 11 integration
tests now pass under `cargo test --test locking` without
`--test-threads=1`.

## [0.8.0] — 2026-05-04

### Added — Multi-agent safety: per-session lock + `rstudio tx`

When two agents run `rstudio` against the same RStudio session, write
commands now compete for an OS-level `flock` at
`~/.config/rstudio-cli/locks/session-<id>.lock`. Reads, `observe stream`,
and meta-CLI commands take no lock — `rsession` already serialises its
own RPCs internally, so reader/writer races aren't a protocol hazard.
Two new global flags:

- `--no-lock` — bypass for power users / debugging.
- `--lock-timeout <s>` — wait time before erroring (default 30 s).

On timeout, the error message carries the holder's PID, command, and
start timestamp (read from a sidecar JSON next to the lock file). When
the holding process exits — cleanly, on `kill -9`, or on crash — the
kernel releases the `flock`. No stale locks, no daemon, no PID files.

For atomic multi-call sequences (the common read-modify-write pattern),
new top-level command:

```sh
rstudio tx -- bash -c '
  buf=$(rstudio editor read-buffer X | jq -r .result.contents)
  new=$(printf "%s" "$buf" | sed "s/foo/bar/g")
  rstudio editor set-contents X "$new"
'
```

`rstudio tx -- <cmd>` acquires the session lock, sets
`RSTUDIO_TX_HELD=1` in the child environment, and execs `<cmd>`. Every
nested `rstudio` invocation inside detects the env var and skips its
own per-call lock (the parent already holds it). Patterned after
`flock(1)` from util-linux — kernel cleanup on parent exit handles
every failure mode.

With no args, `rstudio tx` defaults to `$SHELL` (interactive REPL
inside a transaction). With `bash -c '...'`, it's a one-shot atomic
script. Both compose naturally with shell variables, `jq`, and pipes —
no new DSL or REPL grammar to learn. The shell *is* the monad.

Tests: 6 unit tests in `lock.rs` cover the primitive (acquire / release
/ timeout / serialization / sidecar / env detection); 11 integration
tests in `tests/locking.rs` exercise the binary end-to-end (read-only
without RStudio; live tests skip cleanly when no session is running).

No competing tool in the R/MCP ecosystem provides comparable enforced
multi-agent safety: clauder offers a collaborative protocol that
relies on LLM compliance (no enforcement); the others ignore the
problem entirely. See the comparative table in README.

## [0.7.2] — 2026-05-04

### Fixed — `editor list` / `status` / `observe stream` after opening a project

Reported as #4. RStudio Server (and Desktop) RELOCATES the per-session
sources directory when a project is opened: from
`~/.local/share/rstudio/sources/session-<id>/` to
`<project>/.Rproj.user/<hash>/sources/session-<id>/`. The session id
is unchanged — only the parent path moves. Until 0.7.1 the CLI cached
the global path at session-detection time, so once a project was open
every command that walks the sources directory failed:

- `editor list` returned `session_unavailable: cannot read RStudio
  sources directory ... No such file or directory (os error 2)`.
- `editor read-buffer --path` could not resolve open documents.
- `status.documents.open_count` was always 0 even when documents were
  open (computed from the missing dir), while `active_id` came from a
  different RPC path and stayed correct — the two contradicted.
- `observe stream` Tier 1 silently emitted no editor events.

The fix adds `Session::resolve_sources_dir()` that tries the global
path first (cheap stat) and falls back to scanning
`<active-project>/.Rproj.user/*/sources/session-<id>` when missing.
The active project is read from disk only — never invokes R — so
`observe stream --tier 1` remains R-free:

- Server: `active-project-file` key in `session-persistent-state`.
- Both modes: `~/.local/share/rstudio/projects_settings/last-project-path`.

`observe stream`'s `DiskPaths` was also brittle: it derived the
RStudio data root from the sources directory's grand-parent, which is
correct only outside a project. It now hard-codes
`~/.local/share/rstudio/` (the data root is global; only the per-
session sources directory relocates).

## [0.7.1] — 2026-05-04

### Changed — `observe` becomes a multi-subcommand category

The single `rstudio observe` action shipped in 0.7.0 is now split into
two explicit subcommands:

- **`rstudio observe stream`** — the live JSONL streamer. Same flags
  as before (`--interval`, `--once`, `--tier`).
- **`rstudio observe events`** — static catalog of every event type
  this version emits. Per-type: tier (1 / 2 / 3), source (which file
  or RPC populates it), payload schema, whether it appears in the
  initial snapshot, one-line description. Useful for agents
  discovering the surface, downstream parsers / validators, and
  documentation.

A subcommand is now mandatory. `rstudio observe` (no subcommand)
prints help and exits non-zero. Migration is a one-token edit:
`rstudio observe X` → `rstudio observe stream X`.

This fixes the awkward `rstudio schema observe observe` drill-down
that resulted from a single-action category. Drill-down is now
symmetric with every other multi-action category:
`rstudio schema observe stream` / `rstudio schema observe events`.

The schema action count is 81 (was 80 in 0.7.0).

## [0.7.0] — 2026-05-04

### Added — `rstudio observe`: live JSONL stream of session-state changes

New top-level command that polls the rsession at a configurable interval
and emits one JSON Line per detected change on stdout. Three coverage
tiers, selected with `--tier` (default 2):

- **Tier 1** — file-watching only, never invokes R. Detects document
  open / close / save / dirty / typing / renamed; tails
  `history_database` for `console.input` events with the authoritative
  `rstudio_ts_ms` timestamp; tails the rsession log for `rsession.error`
  lines; tracks project, markers, files-pane dir, find-in-files state,
  source-pane active column.
- **Tier 2** (default) — Tier 1 + one cheap `execute_r_code` per tick.
  Adds `r.busy_changed` (latency heuristic), `r.error`, `env.added` /
  `env.removed`, `wd.changed`, `search.added` / `search.removed`,
  `namespaces.added` / `namespaces.removed`.
- **Tier 3** — Tier 2 + heavier introspection. Adds `env.typed_changed`
  (per-name class + length), `last_value.changed`, `plot.count_changed`.

Tier-2/3 events are buffered for up to 3 ticks waiting for the matching
`console.input` to land in `history_database` (RStudio writes the
history file *after* R has finished executing, so a naive emit order
would produce effects before causes). When flushed because of an
arriving `console.input`, each event is stamped with `caused_by_ts_ms`
pointing to the input's `rstudio_ts_ms` — a strong correlation key for
downstream agents. On timeout flush, the field is omitted (cause was
likely non-console: addin / `r exec` / external RPC).

Output is JSONL on stdout (NOT the AI-native envelope contract).
SIGPIPE is reset to default so `rstudio observe | head -n 5` exits
cleanly. With `--once`, takes a single snapshot and exits.

No competing R/MCP tool exposes a comparable live event stream — this
is a genuine differentiator. See the README's "Observability (live
JSONL stream)" section in the comparative table.

### Added — `rstudio policy`: per-user block list

New top-level command to manage `~/.config/rstudio-cli/policy.json`.
Three subcommands (no live session required): `policy show`,
`policy block <key>`, `policy unblock <key>`. Two granularities: bare
category (`session` blocks every session subcommand) or
`category.action` (`session.restart` blocks one specific action).

Policy is checked at dispatch time, before opening the RPC socket, so
it applies on Server and Desktop alike. Carve-outs that bypass policy:
`version`, `status`, `schema`, `skill`, and `policy` itself. The
destructive session subcommands (`restart`, `open-project`) and `r exec`
/ `r send` are mapped to their full key, so fine-grained rules work
without having to block the entire category.

### Added — Internal convention: README + help + schema sync on every command change

`CLAUDE.md` now records the project convention: any user-visible CLI
change must update three surfaces in the same commit — README's
command-summary table and comparative table; clap doc-comments
(rendered as `--help`); and the schema registry (`CATEGORIES` +
`registry()` in `src/schema.rs`).

## [0.6.3] — 2026-05-03

### Added — `rstudio status --format text` rendering

`rstudio status` now ships a polished human-readable rendering for
`--format text`:

```
rstudio-cli 0.6.3 — Server (unix:///var/run/.../endreas-d)
user            endreas
session         761ca43e
client_id       f6938b37-d079-4d59-8f87-a7da41336ff8
project         (none)
R / RStudio     3.6.3 / 2026.4.0.526
documents open  22 (active: PLAN_TST_ASSEMBLY.md)
```

The default output (no `--format`) stays JSON — agents are still
the primary consumer.

### Internal — `Reply::Adaptive` gains `default_text`

The output `Reply` enum now carries an explicit `default_text: bool`
on the `Adaptive` variant. Meta-CLI commands (`version`, `skill show`,
`skill install`) keep `default_text = true` (text-by-default). Action
commands with a custom text mode (`status`, future others) use
`default_text = false` (JSON-by-default, text on demand). Removes the
ambiguity of overloading `Adaptive` for both audiences.

## [0.6.2] — 2026-05-03

### Added — `rstudio status`

Top-level command returning a single-call snapshot of the CLI ↔
session wiring:

```sh
rstudio status
```

Aggregates in one call what previously needed `session info` +
`editor list` + `editor active-id` + manual mode checks:

- **cli**: version, auto-detected mode (`server` / `desktop`)
- **transport**: Unix socket path (Server) or TCP loopback address (Desktop)
- **user**: identity sent in `X-RStudioUserIdentity`
- **session**: id, active client id, sources directory, state path, active project
- **rsession**: R version, RStudio version
- **documents**: open-doc count, active id, active path

The skill markdown is updated to recommend running this first at the
start of an agent session — it gives the agent immediate context
without chaining multiple discovery calls.

## [0.6.1] — 2026-05-03

### Added — `console activate`

Move keyboard focus to the R console pane. Symmetric counterpart to
`term activate <id>` for the console (which has no id — there's only
one). Wraps the named RStudio command `activateConsole` via
`.rs.api.executeCommand`.

```sh
rstudio console activate
```

Closes a small but real gap: bringing the R console to focus
previously required `rstudio rpc` or `rstudio r exec` workarounds.

## [0.6.0] — 2026-05-03

Editor-surface consolidation pass: one breaking rename, one consistent
addressing pattern across every mutator, and multi-agent skill install.

### Changed (BREAKING) — `context` and `active-context` merged

The standalone `editor active-context` action is **removed**. Its
behavior is preserved as `editor context --include-console`. The
unified action also gains the ability to query a non-active document
via `editor context --id <ID>`.

Migration:

| before                                  | after                                             |
| --------------------------------------- | ------------------------------------------------- |
| `rstudio editor context`                | `rstudio editor context` (unchanged)              |
| `rstudio editor active-context`         | `rstudio editor context --include-console`        |
| (no equivalent)                         | `rstudio editor context --id <ID>` (new)          |

`--include-contents` is preserved on every variant. `--id` and
`--include-console` are mutually exclusive (the console doesn't accept
an id).

Rationale: the previous split confused agents and humans alike. Both
actions named "context" — the difference (one excludes the console,
one doesn't) was not obvious from the name. The merged form makes the
dispatch explicit via flags.

### Added — `--path` on every editor mutator

`close`, `save`, `set-contents`, `modify-range`, `set-cursor` now
accept `--path <PATH>` in addition to id-based addressing. The path
is resolved against the open-doc listing. Same id-or-path pattern as
`reload` and `read-buffer` from 0.5.4.

| action          | id                       | path                       | active default |
| --------------- | ------------------------ | -------------------------- | -------------- |
| `close`         | `close <id>`             | `close --path <p>`         | n/a (required) |
| `save`          | `save --id <id>`         | `save --path <p>`          | yes            |
| `set-contents`  | `set-contents <t> --id`  | `set-contents <t> --path`  | yes            |
| `modify-range`  | `modify-range <r> <t> --id` | `modify-range <r> <t> --path` | yes      |
| `set-cursor`    | `set-cursor <p> --id`    | `set-cursor <p> --path`    | yes            |

Internal: factored the resolution into a single `resolve_target_id`
helper used by all five sites.

### Added — multi-agent skill install via `--for <tool>`

`rstudio skill install --for {claude-code|cursor|cline}` writes the
SKILL.md to the conventional location for that tool. The content is
the same Anthropic open-format markdown across every tool — only the
directory and filename extension vary. Default remains `claude-code`
for backward compatibility.

| `--for`       | location                                      |
| ------------- | --------------------------------------------- |
| `claude-code` | `<root>/.claude/skills/rstudio/SKILL.md`      |
| `cursor`      | `<root>/.cursor/rules/rstudio.mdc`            |
| `cline`       | `<root>/.clinerules/rstudio.md`               |

For other agents, `rstudio skill show > <path>` is the universal
fallback. `--target <path>` still overrides the auto-resolved
directory entirely.

### Fixed — `editor save` returned `{"id": true}`

`.rs.api.documentSave` returns a logical (TRUE on success), not the
doc id. The CLI now captures the resolved id explicitly and returns
that. The active-default case looks up the id via
`documentId(allowConsole = FALSE)` first, saves it, and returns it.

### Fixed — silent no-ops on unknown ids

`editor close <bogus-id>`, `editor save --id <bogus-id>`, and
`editor set-contents <text> --id <bogus-id>` previously returned
`ok=true` with no work done — the rstudioapi wrappers no-op silently
on unknown ids, masking agent typos. `resolve_target_id` now validates
explicit ids by calling `get_source_document` and returns a clear
`user_error` if the doc isn't open. Cost: one extra RPC per
id-validating action.

### Fixed — `editor reload <bogus-id>` surfaced an OS error

The rsession's "No such file or directory" leaked to the user. Now
matched by the shared `rpc_error_is_unknown_doc` helper, so reload
returns `action: skipped-not-open` consistent with the path-based
no-op contract.

## [0.5.4] — 2026-05-03

### Added — `editor reload`

Re-read a document's buffer from disk, preserving the document id so
cached references stay valid. Wraps the rsession `revert_document`
RPC. Variants:

- `rstudio editor reload <id>` — explicit doc id.
- `rstudio editor reload --path <path>` — resolve the id from the
  path by listing open documents and matching.
- `--if-clean` — no-op when the buffer has unsaved changes
  (`action: skipped-dirty`); otherwise the buffer is overwritten
  with the on-disk contents.
- A path/id that doesn't match any open document is a silent no-op
  (`action: skipped-not-open`). Safe to call after external file
  writes regardless of which file was touched.

The embedded skill gains a soft directive recommending
`rstudio editor reload --path <path> --if-clean` after Edit/Write/
MultiEdit, so the user's RStudio buffer stays in sync without
relying on the manual "file changed on disk" dialog.

### Added — `editor read-buffer`

Read the live buffer of any open document, by id or by path. Closes
the gap between `editor read` (on-disk file) and `editor context
--include-contents` (active document only):

```sh
rstudio editor read-buffer D4F4972F            # by id
rstudio editor read-buffer --path /tmp/foo.R   # by path
```

Returns `{id, path, contents, dirty}`. The `dirty` flag is read
from the source_database snapshot, which can lag the frontend by a
fraction of a second after rapid edits.

### Changed — `editor select` accepts `--id`

`rstudio editor select <range> --id <id>` now works on any open
document, not just the active one. Brings `select` in line with
`set-cursor`, `set-contents`, `modify-range`. Without `--id`, the
behavior is unchanged (targets the active document).

### Changed — schema summaries translated

The `editor open` and `editor edit` schema summaries were in
French; they're now in English, matching the project rule.

### Changed — help/about/skill mention RStudio Desktop

Cargo.toml's `description`, the clap `about` line, and the embedded
skill text said "RStudio Server" only — Desktop support has shipped
since 0.5.0. Updated to "RStudio Server (Linux) or Desktop (macOS)".

## [0.5.3] — 2026-05-03

### Fixed — `r send` no longer inserts a blank line before the output

`rstudio r send '<code>'` previously appended `\n` to the input before
sending it through the `console_input` RPC. rsession itself already
terminates the input with a newline before pushing it to the R input
queue, so the extra one rendered as a blank line between the typed
command and its output:

```
> print("hello")

[1] "hello"
```

The CLI now sends the code as-is. The output renders cleanly and the
command executes immediately, exactly as if the user had typed it:

```
> print("hello")
[1] "hello"
```

The schema entry for `r send`'s `code` parameter is updated to reflect
the new contract: pass the code *without* a trailing newline.

## [0.5.2] — 2026-05-02

### Added — Server socket auto-discovery

When `$RSTUDIO_SESSION_STREAM` is not set, the CLI now scans
`$RS_SESSION_TMP_DIR` (default `/var/run/rstudio-server/rstudio-rsession`)
for rsession Unix sockets owned by the current uid. Behaviour by
case:

- **Exactly one match** → use it, transparently.
- **Zero matches** → clear error: `… no rsession socket owned by the
  current user was found in <dir>. Either rsession isn't running, or
  you're on the wrong machine. Pass --socket <path>, or run with
  --mode desktop.`
- **Multiple matches** (a single user *can* run several RStudio Server
  sessions) → error listing every candidate as a copy-pastable
  `--socket <path>` line, with a hint to set
  `$RSTUDIO_SESSION_STREAM` to disambiguate.

This unblocks the case where Claude Code (or any process) is launched
on the same machine as the rsession but not from inside its embedded
terminal — no env var is set, but the socket *is* on disk and
connectable. Previously the CLI errored with "RSTUDIO_SESSION_STREAM
is not set"; now it just works.

The previous behaviour (read `$RSTUDIO_SESSION_STREAM`, fast-path) is
preserved: when the env var is set, no scan happens.

### Dependencies

Adds `libc = "0.2"` for `getuid()`, used by the uid filter on
discovered sockets. Saves writing custom uid lookup via `/proc` or
shelling out to `id -u`.

## [0.5.1] — 2026-05-02

Polishes the output contract for the three meta-CLI commands so they're
human-friendly out of the box without compromising the AI-native JSON
envelope used by every action command.

### Changed — meta-CLI commands default to plain text

- `rstudio version` defaults to plain `0.5.1\n` instead of the JSON
  envelope. Pass `--format json` to opt back into `{"ok":true,"result":
  {"version":"0.5.1"}}`.
- `rstudio skill show` defaults to raw markdown stdout (pipeable into
  `less`, `glow`, etc.). The trailing `{"ok":true}` envelope is gone.
  Pass `--format json` to wrap the markdown in the envelope.
- `rstudio skill install` defaults to a one-line human status:
  `✓ created  ./path/to/SKILL.md (v0.5.1)` (or `updated` / `unchanged`).
  The `✓`/`✗` marks and ANSI colors light up only when stdout/stderr is
  a TTY; piped output dégrades gracefully to ASCII `OK`/`FAIL`. Pass
  `--format json` for the structured `{path, action, version}` payload
  scripts may want.

### Changed — error output in text mode

- Errors in `--format text` now print `✗ <message>` on stderr (TTY-aware,
  same color rule). Previously: `error (UserError): <message>`. The JSON
  envelope contract on stderr/stdout for `--format json` is unchanged
  (still includes `kind` and `code`).

### Implementation note

The global `--format` flag changed from a `default_value = "json"` to an
unset `Option<Format>`. Action commands resolve `None` to JSON (the
AI-native default), the three meta-CLI commands resolve it to text. No
breaking change for explicit `--format json`/`--format text` callers,
and agents that don't pass `--format` still get JSON for every action.

## [0.5.0] — 2026-05-02

Adds RStudio Desktop support on macOS, ships the Homebrew install path, and
keeps Server fully backward compatible.

### Added — RStudio Desktop on macOS

- New `--mode auto|server|desktop` global flag (default `auto`). Auto-detects
  Server when an rsession Unix socket is reachable, Desktop when a local
  rsession process is running.
- `--port` / `--secret` global flags to override Desktop discovery (rare cases:
  multiple rsessions, restricted process inspection).
- TCP-loopback transport with `X-Shared-Secret` authentication. The Desktop
  client id (`33e600bb-c1b1-46bf-b562-ab5cba070b0e`) is hardcoded — no
  `client_init` ever, the blacklist stays.
- `desktop_discovery` module reads the rsession process's argv (`--www-port`,
  `--launcher-token`) and environment (`RS_SHARED_SECRET`) via `ps` on macOS
  and `/proc/<pid>/{cmdline,environ}` on Linux.
- `transport` module replaces `socket`. `Backend::{Unix, Tcp}` enum picks the
  connection type at runtime.

### Fixed — Desktop async-handle handling

- `parse_rpc_envelope` now refuses queued `execute_r_code` responses with a
  clean `kind=session_unavailable` naming the `asyncHandle`. Desktop's TCP
  listener takes the async path under FIFO contention; without this guard,
  the second concurrent `r exec` surfaced as `kind=internal` ("returned
  non-string: null"). Server's Unix-socket listener never takes the async
  path, so this branch is dead code on Server (verified by 21 unit + 7 live
  Server tests + Step 6 ×3 still green).

### Added — install paths

- Homebrew formula on `aclemen1/homebrew-tap`:

  ```sh
  brew install aclemen1/tap/rstudio-cli
  ```

- GitHub Release artifacts for four targets:
  - `x86_64-unknown-linux-gnu`
  - `aarch64-unknown-linux-gnu`
  - `x86_64-apple-darwin`
  - `aarch64-apple-darwin`

### Known limitation

Concurrent `r exec` on Desktop returns `session_unavailable` for the second
call (γ behaviour). The natural follow-up is the β path documented in
`DESKTOP_TEST_RESULTS.md` § "B1 — spike β" — minting a per-invocation client
id and polling the `kAsyncCompletion` event channel. Out of scope for 0.5.0.

### Added documentation

- `DESKTOP_FEASIBILITY.md` — architectural delta Server vs Desktop, recon, curl
  round-trips proving each surface.
- `DESKTOP_TEST_RESULTS.md` — end-to-end validation on macOS, B1 wire capture
  and re-validation post-γ.

## [0.4.0] — 2026-05-02

This release rounds out the rstudioapi surface, ships the AI-native
discoverability story end-to-end, and crosses the line of `cargo clippy
-D warnings` clean.

### Added — sessions, preferences, jobs, modal UI

- **`session`** category (4 actions): `info` (versionInfo + getVersion +
  user/system identity + has_color_console + active_project), `project`
  (getActiveProject), `open-project` (openProject; destructive without
  `--new-session`), `restart` (restartSession; refuses without `--confirm`).
- **`pref`** category (6 actions): `read` / `write` / `read-rstudio` /
  `write-rstudio` / `get-persistent` / `set-persistent`. Values move
  through `--value-json` / `--default-json` so any JSON-representable
  type survives the round-trip.
- **`job`** category (10 actions): `list` / `add` / `remove` /
  `set-progress` / `add-progress` / `set-state` / `set-status` /
  `add-output` / `run-script` / `is-active`.
- **`ui`** category (8 actions, all BLOCKING): `dialog` / `update-dialog`
  / `prompt` / `question` / `select-file` / `select-dir` / `ask-password`
  / `ask-secret`. Schema entries explicitly mark them as blocking.

### Added — editor / term / pane completion

- **editor** gains `new` (documentNew), `active-id` (documentId),
  `path` (documentPath), `set-contents` (setDocumentContents),
  `modify-range` (modifyRange), `set-cursor` (setCursorPosition),
  `close` (`.rs.api.documentClose`), `save` (documentSave), `save-all`
  (documentSaveAll), `list` (hybrid: filenames as ids + per-doc RPC),
  `active-context` (getActiveDocumentContext).
- **term** gains `busy` (terminalBusy), `running` (terminalRunning),
  `exit-code` (terminalExitCode), `visible` (terminalVisible), `run`
  (terminalExecute).
- **pane** gains `preview-rd` (previewRd), `preview-sql` (previewSql),
  `save-plot` (savePlotAsImage; `--image-format` to dodge the global
  `--format`), `highlight-ui` (highlightUi).
- **console** gains `context` (getConsoleEditorContext).

### Added — schema traceability

- Each `ActionSpec` now exposes `rstudioapi_fn` and `rpc_method` so
  `rstudio schema <cat> <action>` traces every action back to its
  rstudioapi function and the JSON-RPC method (or postback) used.
  Postbacks are noted as `postback:<cmd>`.

### Changed — breaking renames (already shipped in 0.3.0, recapped)

- `view` → `pane`. `view html` → `pane viewer`, `view files` → `pane
  files`, `view mark` → `pane markers`.
- `exec` → `r`. `exec run` → `r exec`, `exec send` → `r send`.
- Skill layout migrated from a single file `<dir>/<name>.md` to
  `<dir>/<name>/SKILL.md` per the current Claude Code convention.
- `SKILL_VERSION` constant removed; the skill ships in lockstep with
  the CLI version (one `version` field).

### Fixed

- `editor close` now actually closes the tab. Previously the
  `close_document` JSON-RPC method only enqueued a UI event for the
  browser, which silently failed when the event was not consumed.
  The new path goes through `.rs.api.documentClose(id, save)`, which
  invokes the C call `rs_requestDocumentClose` and unmounts the tab
  regardless of browser state.
- `r exec` (silent eval) wraps the user's code in a tryCatch so
  R errors surface as `kind=r_error` and timeouts as `kind=timeout`,
  instead of being swallowed into an empty string.
- `editor open` no longer routes through the `editfile` postback,
  which opens the modal R `edit()` dialog and intermittently times
  out the socket. It now uses `rstudioapi::documentOpen` and returns
  the document id. The previous behaviour is preserved as a separate
  `editor edit` action.
- `connection_test` is no longer used internally for any action that
  can throw — it leaks R errors into the user's visible console.
  Every wrapper goes through `execute_r_code` (silent) or
  `console_input` (visible) instead.
- `rstudio rpc client_init` is blacklisted. Calling `client_init`
  invalidates the active browser client and forces a session reload.

### Tooling

- `cargo clippy --all-targets -- -D warnings` is clean.
- `cargo fmt` applied across the tree.
- 13 unit + 7 live integration tests green
  (`cargo test --test live -- --ignored`).

## [0.3.0] — 2026-05-02

### Changed — breaking

- Renamed `view` → `pane` and `exec` → `r` for vocabulary alignment
  with the rstudioapi package.
- Skill layout migrated to `<dir>/<name>/SKILL.md` per the current
  Claude Code convention.
- `SKILL_VERSION` removed; CLI and skill ship in lockstep.

### Added

- `editor active-context` (getActiveDocumentContext) and `console
  context` (getConsoleEditorContext): full coverage of the three
  rstudioapi `*Context` getters.
- `ActionSpec.rstudioapi_fn` and `ActionSpec.rpc_method` for
  schema-level traceability.
- `editor close`, `editor save`, `editor save-all` backed by
  `.rs.api.documentClose` / `documentSave` / `documentSaveAll`.

## [0.2.0] — 2026-05-02

### Changed — breaking

- Skill layout: file → directory (`<name>/SKILL.md`) per Claude Code
  convention.
- Version unified between CLI and embedded skill.

## [0.1.0] — 2026-05-02

### Added

- Initial scaffold: `editor`, `r`, `console`, `term`, `env`, `pane`,
  `skill`, plus `schema` (3-level drill-down catalog) and `rpc` /
  `postback` raw escape hatches.
- AI-native pattern: small embedded skill + `rstudio schema` for
  on-demand discovery.
- Live integration test suite under `tests/live.rs`.
- Concurrency model documented: R is single-threaded; `r exec` calls
  serialise FIFO; `term exec` uses a separate pty.
