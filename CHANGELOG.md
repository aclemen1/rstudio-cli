# Changelog

All notable changes to **rstudio-cli** are documented here. The format is based
on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
