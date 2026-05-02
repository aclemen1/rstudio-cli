# rstudio-cli

AI-native CLI bridge that lets a terminal-bound process — Claude Code, a
shell user, anything else — drive the embedded RStudio Server IDE it
runs inside : open files, run R code, list and read terminals, inspect
the live R environment, surface lint-style markers, install a Claude
Code skill, and more.

The binary is named `rstudio`. It speaks to the rsession Unix socket
of the **same** RStudio Server session it runs inside, sharing the
active browser client so visible actions land in the user's open tab
without disrupting it.

## Status

WIP, but covers a substantial slice of the IDE :

| category | actions |
|---|---|
| `editor` | `open` (non-modal) `edit` (modal R `edit()`) `read` `context` `active-context` `insert` `select` |
| `r`      | `exec` (silent) `send` (visible) |
| `console`| `history` (live) `actions` (snapshot) `context` (live) |
| `term`   | `list` `buffer` `context` `create` `send` `exec` `kill` `clear` `activate` |
| `env`    | `list` `contents` `info` |
| `pane`   | `viewer` `files` `markers` |
| `skill`  | `show` `install` |
| `schema` | the AI-native catalog (drill-down 3 levels) |
| escape   | `rpc <method>` `postback <cmd>` |

Run `rstudio schema` for the auto-generated catalog of every action.

## AI-native pattern

The CLI is paired with a small embedded skill (markdown). An LLM agent
loads only the skill, then discovers the surface on demand:

```
rstudio schema                  # level 0: every action with category + summary
rstudio schema editor           # level 1: actions in 'editor'
rstudio schema editor open      # level 2: full ActionSpec (params, examples, errors)
```

This keeps the agent's context window lean — no tool descriptions are
loaded for actions the agent never uses.

```
rstudio skill install           # writes ./.claude/skills/rstudio.md
rstudio skill show              # prints the embedded skill markdown
rstudio version                 # {"cli": "0.1.0", "skill": 1}
```

## Requirements

- Linux (RStudio Server only — RStudio Desktop is not supported)
- A live RStudio Server session belonging to the same Unix user as the CLI
- A browser tab attached to that session (the CLI reads the active
  client id from the on-disk session state and **never** calls
  `client_init`, which would invalidate that client)

## Build

```sh
cargo build --release
./target/release/rstudio version
```

## Tests

```sh
cargo test               # unit tests, fast, no live session needed
cargo test --test live -- --ignored   # integration tests against a live session
```

The integration tests skip silently (`SKIP: no live RStudio session
available`) when no `rsession` socket is reachable, so the suite is safe
to run anywhere. They never mutate the user's UI: read-only paths only
(`r exec` round-trips, `editor read`, `env list`, `term list`, schema
registry shape).

## Auto-detection

The CLI reads the following at each invocation:

| Var                       | Purpose                                              | Fallback                                                                  |
|---------------------------|------------------------------------------------------|---------------------------------------------------------------------------|
| `$RSTUDIO_SESSION_STREAM` | Unix socket name under `$RS_SESSION_TMP_DIR`         | none — must be set or pass `--socket`                                     |
| `$RS_SESSION_TMP_DIR`     | Directory holding the rsession socket                | `/var/run/rstudio-server/rstudio-rsession`                                |
| `$RSTUDIO_SESSION_ID`     | Used to find `session-persistent-state` for clientId | most-recently-modified `session-*` under `~/.local/share/rstudio/sessions/active` |
| `$USER`                   | Identity sent in `X-RStudioUserIdentity`             | `$LOGNAME`                                                                |

Override any of them via `--socket`, `--user`, `--session-id`, `--state-path`.

## Output contract

JSON by default. Pass `--format text` for human-readable output where
sensible (`output` strings, `lines` / `commands` arrays).

```json
{ "ok": true,  "result": { "..." : "..." } }
{ "ok": true }
{ "ok": false, "error": { "code": 0, "kind": "session_unavailable", "message": "..." } }
```

`error.kind` is one of: `user_error`, `r_error`, `rpc_error`, `timeout`,
`session_unavailable`, `internal`.

Exit codes: `0` success, `1` runtime error, `2` bad CLI args.

## A few examples

```sh
# Open a file at line 42 in the user's editor.
rstudio editor open src/main.rs --line 42

# Evaluate R silently and read the captured output.
rstudio r exec 'summary(mtcars)'

# Bypass the 2 s server limit for a longer job.
rstudio r exec --timeout 60 'Sys.sleep(10); summary(mtcars)'
rstudio r exec --timeout 0  'long_running()'   # no limit

# Make something appear and execute in the user's R console.
rstudio r send 'print(Sys.time())'

# Read the buffer of an open shell terminal.
rstudio term list
rstudio term buffer 93555F0A --limit 50

# Run a shell command in a fresh terminal.
ID=$(rstudio --format text term create --name 'rstudio-cli-task' | jq -r .id)
rstudio term exec "$ID" 'find . -name "*.R" | head'
sleep 1
rstudio term buffer "$ID" --limit 30
rstudio term kill "$ID"

# Surface lint-style feedback in the Markers pane.
rstudio pane markers --name 'lint' --markers '[
  {"type":"warning","file":"/path/to/foo.R","line":12,"message":"Unused variable"},
  {"type":"error",  "file":"/path/to/bar.R","line":3, "message":"Syntax error"}
]'

# Inspect the active R environment.
rstudio env list --pattern '^df_'
rstudio env info mtcars
rstudio env contents mtcars
```

## Concurrency model

R is single-threaded; the rsession serialises every `r exec` /
`exec eval` style call into a FIFO. Two concurrent calls do **not** run
in parallel — total wall time ≈ sum of per-call time. So:

- `--timeout 0` on a long `r exec` blocks every subsequent `exec`-style
  call until it returns. Use it deliberately.
- For real parallelism (running shell commands or external processes),
  use `term exec` — the Terminal pane spawns a separate pty/process,
  not bound to the R FIFO.
- Postbacks (`editor edit`) and `console_input` (`r send`) don't go
  through the R queue, so they aren't subject to the FIFO.

## Hard "do not"

- `rstudio rpc client_init` is **blacklisted** at the CLI level — calling
  it invalidates the active browser client and resets the user's RStudio
  session. Confirmed twice during development.
- Avoid `connection_test` (raw RPC) for anything that can throw : R
  errors raised through it leak into the user's visible console. The
  `exec` and `editor` wrappers route through `execute_r_code` instead,
  which is silent.

## License

MIT — see [LICENSE](LICENSE).
