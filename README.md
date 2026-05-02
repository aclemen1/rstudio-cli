# rstudio-cli

AI-native CLI bridge to interact with the embedded RStudio Server IDE from a
terminal — open files, run R code, drive the viewer, query the environment,
all from outside the running R session.

The binary is named `rstudio`. It speaks to the running `rsession` Unix socket
of the **same** RStudio Server session it runs inside, sharing the active
browser client so that visible actions (console input, navigation) land in the
user's open tab without disrupting it.

## Status

Early WIP (v0.1). Implemented so far:

- `rstudio version` — CLI + skill version (JSON)
- `rstudio exec run <code>` — run R code silently (capture, max 2s)
- `rstudio exec send <code>` — type code into the user's console (visible, executes)
- `rstudio editor open <path> [--line N] [--col N]` — open a file, optionally jump to line
- `rstudio rpc <method> [--params JSON]` — raw JSON-RPC escape hatch
- `rstudio postback <cmd> <body>` — raw postback escape hatch

## Requirements

- Linux (RStudio Server only — RStudio Desktop is not supported)
- A live RStudio Server session belonging to the same Unix user as the CLI
- A browser tab attached to that session (the CLI reads the active client id
  from the on-disk session state and never calls `client_init`)

## Build

```sh
cargo build --release
./target/release/rstudio version
```

## Auto-detection

The CLI reads the following at each invocation:

| Var                       | Purpose                                              | Fallback                                                                  |
|---------------------------|------------------------------------------------------|---------------------------------------------------------------------------|
| `$RSTUDIO_SESSION_STREAM` | Unix socket name under `$RS_SESSION_TMP_DIR`         | none — must be set or pass `--socket`                                     |
| `$RS_SESSION_TMP_DIR`     | Directory holding the rsession socket                | `/var/run/rstudio-server/rstudio-rsession`                                |
| `$RSTUDIO_SESSION_ID`     | Used to find `session-persistent-state` for clientId | most-recently-modified `session-*` under `~/.local/share/rstudio/sessions/active` |
| `$USER`                   | Identity sent in `X-RStudioUserIdentity`             | `$LOGNAME`                                                                |

Override any of them via `--socket`, `--user`, `--session-id`, `--state-path`.

## Output

JSON by default. Pass `--format text` for human-readable output.

```json
{ "ok": true, "result": { ... } }
{ "ok": false, "error": { "code": 0, "kind": "session_unavailable", "message": "..." } }
```

Exit codes: `0` success, `1` runtime error, `2` bad CLI args.

## License

MIT — see [LICENSE](LICENSE).
