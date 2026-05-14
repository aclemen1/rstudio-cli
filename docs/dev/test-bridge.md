# Resuming the test-coverage work

State as of 2026-05-15 (mid-session pause). 14 commits sit on local
`main` past `main@origin` (`cb7a666`); none pushed.

## What's done

The 0.14.0 work splits into two sprints:

**Sprint 1 — package refonte (commits `luzlrwzy` … `tytrytkn`).**
Progressive discovery in the MCP server, `rstudiocli.mcp` R package
embedded in the binary, programmatic tool calling via `r_script`,
auto-install on first RPC, then a 6-lot migration of every
runtime `format!("rstudioapi::...")` call site to
`rstudiocli.mcp::*`. See `CHANGELOG.md` `[0.14.0]` for the
user-facing summary.

**Sprint 2 — Docker test bridge (commits `prmoxylq` … `stkovnmv`).**
Infrastructure to drive a containerised RStudio Server from the CLI
as if it were a local Desktop session, so live tests can run on a
clean throwaway instance.

```
       (CLI on macOS host)
         │
         ▼
  /tmp/rstudio-bridge.sock  ← socat UNIX-LISTEN
         │
         ▼ TCP localhost:19999
         │
  ┌──────┴──────────────────────┐
  │  Docker container           │
  │   ├─ rserver                │
  │   ├─ rsession ← socat       │
  │   │    UNIX-CONNECT to      │
  │   │    /var/run/.../d       │
  │   └─ Chrome (headless,      │
  │       on host) opens        │
  │       :18787 to register    │
  │       a real GWT client     │
  └─────────────────────────────┘
```

Why all the moving parts:
- Headless Chrome is required because rsession refuses to honour
  `rstudioapi::documentId / getSourceEditorContext / ...` without a
  live GWT client polling its event channel. With Chrome attached,
  these calls work; without it they crash the HTTP response mid-stream.
- The double-`socat` bridge avoids the macOS↔Linux Unix-socket-mount
  limitation: rsession's socket stays inside the container, the host
  side gets a fresh Unix socket the CLI can `connect()` to.
- The CLI needs Chrome's actual `clientId` + `port-token` cookie to
  authenticate as a recognised client — without them every RPC after
  `client_init` returns `Invalid client id` or `Client unauthorized`.

Working: **13 / 23** existing live tests pass through the bridge on a
fresh container.

## How to spin up the bridge

```sh
# colima with virtiofs (works) — recommended (open source)
colima start --mount-type virtiofs --vm-type vz \
             --mount "$HOME/.cache/rstudio-bridge:w"

# or OrbStack: just start the app, no flags needed
```

Then:

```sh
scripts/bridge-up.sh up        # spawn container, install pkg, etc.
source /tmp/rstudio-bridge-state.env
cargo test --test live -- --ignored --test-threads=1

scripts/bridge-up.sh refresh   # if Chrome did a Page.reload, re-sync creds
scripts/bridge-up.sh down      # tear it all down
```

The script writes its state to `/tmp/rstudio-bridge-state.env`. The
test harness in `tests/live.rs` honours these env vars without
modification (the CLI does — see the override env vars below).

### Bridge-only env vars on the CLI side

Six switches let the CLI play along with the bridge. All default to
"behave normally" so Desktop and Server-on-same-host are unaffected.

| Var | Purpose |
|---|---|
| `RSTUDIO_CLI_CLIENT_ID` | Override the clientId the CLI reads from `session-persistent-state`. Bridge sets it to Chrome's actual clientId. |
| `RSTUDIO_CLI_PORT_TOKEN` | Cookie value rsession requires to recognise the clientId. Captured from Chrome via CDP. |
| `RSTUDIO_CLI_BRIDGE_TARBALL_DIR` + `_RPATH_DIR` | `r_package` install: host write path + container read path of the same bind-mount. |
| `RSTUDIO_CLI_BRIDGE_CAPTURE_DIR` + `_RPATH_DIR` | `r send` capture: same idea, for the tempfile R writes and the CLI polls. |
| `RSTUDIO_CLI_SKIP_ENSURE_INSTALL` | The bridge pre-installs `rstudiocli.mcp` directly in the container's R lib; skip the CLI's own (which sometimes fails through the bridged HTTP). |
| `RSTUDIO_CLI_SKIP_PID_CHECK` | `r send` polls `kill(pid, 0)`; with the rsession PID in the container's namespace, the host check always reports "process died". |

## What's left to fix (10 / 23 tests still red)

Pick up in the order roughly cheap → involved:

1. **`editor_read_returns_content`**.
   `editor read` (the on-disk file reader, not `read-buffer`) does
   `path.canonicalize()` host-side and then asks R to read that same
   path. In the bridge the path needs to be relative to the container's
   filesystem. Two options:
   - Have the harness mount a known host directory at the same path
     inside the container (e.g. `/Users/aclemen1/test-files:
     /Users/aclemen1/test-files`) and have the test write its fixture
     there. Or:
   - Add `RSTUDIO_CLI_PATH_REMAP=host:container` so the CLI rewrites
     paths before sending them to R. Cleaner but invasive.

2. **`env_list_pattern_filter`, `env_info_returns_metadata`,
   `env_contents_returns_lines`**.
   Need to inspect what these call exactly. The simple `env_list_returns_array`
   passes, so it's probably a specific filter/projection edge case.

3. **`r_exec_async_and_poll`**.
   Async/poll uses `callr` which the rocker image doesn't ship by
   default. Easy: have the bridge pre-install `callr` in the container.

4. **`r_exec_timeout_surfaces_as_timeout`**.
   Specifically wants `Sys.sleep(3)` with a 1 s timeout to come back
   as `Timeout`. With bridge overhead this might race differently.
   Maybe bump the test's timeout, or its assertion tolerance.

5. **The remaining `r_send_*` failures**
   (`captures_message`, `in_attached_env`, `mixed_stdout_and_message`,
   `surfaces_r_error_as_cli_error`).
   The "captures stdout" variant works, so the wiring is fine — these
   four hit timeouts on patterns that should be quick. Probably the
   bridge poll interval (50 ms) is too aggressive given the extra
   socat hop latency, or the `current_environment_name` call
   occasionally lags. Try raising `SEND_POLL_INTERVAL` to 200 ms in
   bridge mode (gate via the existing skip-pid env var or add a new
   one), or instrument the polling loop to log which branch wins.

6. **Auto-install of `rstudiocli.mcp` through the bridge.**
   Currently the harness pre-installs the package and the CLI is
   told to skip its own install via `RSTUDIO_CLI_SKIP_ENSURE_INSTALL=1`.
   The reason: when `r_package::install_from_embedded` runs via the
   bridge, the resulting `execute_r_code` response is malformed (HTTP
   missing headers terminator). Same shape as the `documentId` crash
   we hit before Chrome was attached, but the package install isn't
   client-dependent — so something else is going on. Investigate by
   capturing the raw HTTP bytes coming back from the bridge during
   the install call.

## New tests to write once the above is sorted

The migrated R wrappers (lots 1–6) aren't covered by any live test.
~30 new ones to add. By category:

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
| editor | once `editor_read` is fixed: `open → set_contents → read_buffer → close` cycle, plus `modify_range`, `set_cursor`, `set_marks` |
| r | `r_script` programmatic-calling: simple value, `stop()` propagation, tx-guard rejection, large intermediate-data scenario |

The `ui_*` family stays out of live tests on purpose (modal,
interactive). For those, a separate R-unit-test sprint with
`testthat::local_mocked_bindings(.package = "rstudioapi", ...)` should
mock the underlying `rstudioapi::showDialog/showPrompt/...` and assert
that the wrappers forward the right arguments. Sketch is in
`r-package/tests/testthat/test-editor.R` (the existing input-validation
tests) — extend with the mocked-behaviour pattern.

## Cleanup commands when you come back

```sh
# Tear down everything from a previous session
scripts/bridge-up.sh down
colima stop                  # if using colima
rm -rf ~/.cache/rstudio-bridge  # nuke the shared volume
docker context use orbstack  # or your preferred default
```

## Known-good runtime combos

Tested working:
- macOS 25 (Tahoe) + OrbStack 1.7.5 + Docker 27.3.1
- macOS 25 + colima 0.x with `--mount-type virtiofs --vm-type vz`

Tested NOT working:
- colima with default `sshfs` mount: container writes never propagate
  to host. Easy fix — see flags above.
- Bind-mount of `/run/rstudio-server/rstudio-rsession` into the host
  (any runtime): the socket file appears but kernel-level connections
  don't pass through. That's why we use the double-`socat` bridge
  instead of mounting the socket directly. Don't bother retrying this
  path.
