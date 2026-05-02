# Changelog

All notable changes to **rstudio-cli** are documented here. The format is based
on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
