# Test bridge — Dockerised RStudio Server for live integration tests

The `tests/live.rs` suite exercises the CLI against a real rsession.
Locally that can be a Desktop session you have open; on a clean machine
or in CI, `scripts/bridge-up.sh` brings up a containerised RStudio
Server, registers a real GWT client via headless Chrome, and runs the
test binary **inside the container** against rsession's Unix socket.

```
  ┌─────────────────────────────────────────────────────────┐
  │  Docker container (rocker/rstudio:4.5.2)                │
  │                                                         │
  │   rserver  ──────────────  Chrome (headless, on host)   │
  │     │       :18787          opens :18787, supplies the  │
  │     ▼                       clientId + port-token       │
  │   rsession (Unix socket: /var/run/.../rstudio-d)        │
  │     ▲                                                   │
  │     │  direct AF_UNIX connect — no socat, no tunnel     │
  │     │                                                   │
  │   cargo test --test live (binary compiled in container) │
  └─────────────────────────────────────────────────────────┘
```

## Why in-container, not a host↔container tunnel

The previous design tunneled the CLI on the host through
`socat UNIX-LISTEN → TCP → socat UNIX-CONNECT` into the container.
That tripped a deterministic 1-on-1-off bug: every second `r send`
silently dropped. Root cause: rsession's `accept()` loop on its Unix
listening socket is serialised against its post-REPL event drain. When
two CLI invocations cross that ~1 s window (typical for consecutive
`r send` calls), the second `connect()` waits in the kernel backlog and
the request times out.

From inside the container the listener is reached without that hop and
the bug disappears completely — 23/23 tests pass without any bridge-
specific code paths in the CLI.

## Usage

```sh
scripts/bridge-up.sh up        # spawn container, install toolchain + R deps,
                               # launch headless Chrome, mint clientId/token
scripts/bridge-up.sh test      # sync sources, build, run all live tests
scripts/bridge-up.sh test r_send_captures_stdout   # filter to one test
scripts/bridge-up.sh sync      # re-copy sources after a local edit
scripts/bridge-up.sh refresh   # re-read clientId/port-token if Chrome reloaded
scripts/bridge-up.sh down      # stop container + Chrome (cargo cache preserved)
```

State (clientId, port-token) is written to `/tmp/rstudio-bridge-state.env`
and consumed by `cmd_test`. Two env vars only — no `*_BRIDGE_*`,
`*_SKIP_*`, `*_PATH_REMAP` overrides anymore:

```
RSTUDIO_CLI_CLIENT_ID    Chrome's clientId (mandatory: rsession rejects
                         unknown clientIds with rpc code 4)
RSTUDIO_CLI_PORT_TOKEN   port-token cookie minted by rsession on first
                         /client_init; without it every clientId is
                         "unauthorized"
```

## How the pieces fit

- **`docker run rocker/rstudio:4.5.2`** — Ubuntu-based image with R 4.5,
  RStudio Server, and the rserver/rsession pair pre-configured. We mount
  a single named volume `rstudio-bridge-cargo` at `/home/rstudio/.cargo`
  so the Rust toolchain and the compiled `target/` survive container
  restarts.
- **Headless Chrome on the host** (not in the container — it would need
  X or wayland). It hits `localhost:18787`, registers a GWT client with
  rsession, gets a clientId and a `port-token` cookie. Without Chrome,
  any RPC that reads from the active-client event stream
  (`getSourceEditorContext`, `documentId`, etc.) crashes the HTTP
  response mid-stream — rsession assumes a browser is polling and
  panics when it isn't.
- **`docker cp` based source sync.** `cmd_sync` pipes a tar of the
  working tree into the container, excluding `target/`, `.git`, `.jj`,
  `node_modules`. Runs in ~1 s for this repo. Doesn't require any
  bind-mount support from the underlying Docker runtime, so the same
  script works on Docker Desktop, OrbStack, colima, and any cloud
  Docker host.
- **Rust toolchain in the container.** Installed once on first `up`,
  then cached in the named volume. Incremental rebuilds typically
  finish in <10 s.
- **Test execution.** `cargo test --test live -- --ignored` from inside
  the container, with `USER=rstudio` and the captured clientId/token
  exported into the environment.

## Known runtime requirements

- Any Docker runtime works — OrbStack, colima (any mount-type),
  Docker Desktop. No `--mount` flag, no bind-mounts.
- macOS host needs `bsdtar` (default), `curl`, Google Chrome.app,
  and Python (for the websockets-based Chrome DevTools snippet).
  `uv` auto-installs the `websockets` package on demand.
- Container needs internet access during first `up` (rustup, apt,
  CRAN packages). Subsequent `up`s reuse the cargo volume cache.

## Cleanup

```sh
scripts/bridge-up.sh down              # stop container + Chrome
docker volume rm rstudio-bridge-cargo  # nuke the cargo cache (forces a
                                       # full toolchain reinstall + rebuild
                                       # next time)
```

## Adding tests

The migrated R wrappers (lots 1–6 of the 0.14.0 work) aren't covered by
any live test yet. ~30 new ones to add, by category:

| Category | Suggested tests |
|---|---|
| status | `status_returns_full_payload` (everything: r_version, rstudio_version, active_doc_*, transport, lock) |
| session | `session_info_fields_present`, `session_restart_with_confirm` |
| project | `project_current_null_when_no_project`, `project_open_disruptive` |
| pref | `pref_read_write_user`, `pref_read_write_rstudio`, `pref_get_set_persistent` |
| pane | `pane_viewer_local_file`, `pane_files_navigate`, `pane_markers_dataframe`, `pane_markers_list_form` |
| term | full lifecycle: `create → send → buffer → kill`, then `term_running`, `term_busy`, `term_exit_code`, `term_visible` |
| job | `job_add_lifecycle` (add, set_progress, set_state, remove), `job_is_active_false` |
| ui | skip in live (modal); cover by R unit tests with `local_mocked_bindings` instead |
| editor | `open → set_contents → read_buffer → close` cycle, plus `modify_range`, `set_cursor`, `set_marks` |
| r | `r_script` programmatic-calling: simple value, `stop()` propagation, tx-guard rejection, large intermediate-data scenario |

The `ui_*` family stays out of live tests on purpose (modal,
interactive). For those, a separate R-unit-test sprint with
`testthat::local_mocked_bindings(.package = "rstudioapi", ...)` should
mock the underlying `rstudioapi::showDialog / showPrompt / ...` and
assert that the wrappers forward the right arguments. Sketch is in
`r-package/tests/testthat/test-editor.R` (the existing input-validation
tests) — extend with the mocked-behaviour pattern.
