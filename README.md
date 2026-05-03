# rstudio-cli

<p align="center"><img src="assets/logo.svg" alt="rstudio-cli" width="200"></p>

[![CI](https://github.com/aclemen1/rstudio-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/aclemen1/rstudio-cli/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

[AI-native CLI](https://github.com/aclemen1/ai-native-cli) bridge that lets a terminal-bound process — Claude Code,
a shell user, anything else — drive the embedded RStudio Server IDE
it runs inside: open files, run R code, list and read terminals,
inspect the live R environment, surface lint-style markers, manage
background jobs, install a Claude Code skill, and more.

The binary is named `rstudio`. It speaks to the rsession Unix socket
of the **same** RStudio Server session it runs inside, sharing the
active browser client so visible actions land in the user's open tab
without disrupting it.

## In Claude Code

Claude Code's `/ide` slash command connects the agent to VS Code or
JetBrains (as of this writing — the list may evolve). The agent and
the IDE share state: open files, diagnostics, selection. `/ide` does
not cover RStudio.

Installing the skill (`rstudio skill install`) closes that gap by
adding `/rstudio` as a slash command in Claude Code. Typing
`/rstudio` invokes the skill, which tells the agent how to drive
your live RStudio session — open files, run R, surface markers,
inspect the live environment, manage Jobs — through the CLI, without
disrupting your browser tab.

## Status

**v0.5.0** — covers ~50 of the 117 functions exported by `rstudioapi`,
across 13 categories and 76 actions. Live-tested end-to-end on both
**RStudio Server** (Linux) and **RStudio Desktop** (macOS).

| category | actions | summary |
|---|---|---|
| `editor` | `open` `edit` `close` `reload` `save` `save-all` `read` `read-buffer` `context` `insert` `select` `list` `new` `active-id` `path` `set-contents` `modify-range` `set-cursor` | Source pane and document operations |
| `r`      | `exec` `send` | Run R code (silent or visible) |
| `console`| `history` `actions` `context` | Console history + buffer + live editor context |
| `term`   | `list` `buffer` `context` `create` `send` `exec` `kill` `clear` `activate` `busy` `running` `exit-code` `visible` `run` | Terminal pane (live shells) |
| `env`    | `list` `contents` `info` | Live R environment inspection |
| `pane`   | `viewer` `files` `markers` `preview-rd` `preview-sql` `save-plot` `highlight-ui` | Non-editor panes |
| `session`| `info` `project` `open-project` `restart` | Whole-session lifecycle |
| `pref`   | `read` `write` `read-rstudio` `write-rstudio` `get-persistent` `set-persistent` | Preferences + persistent values |
| `job`    | `list` `add` `remove` `set-progress` `add-progress` `set-state` `set-status` `add-output` `run-script` `is-active` | Background Jobs pane |
| `ui`     | `dialog` `update-dialog` `prompt` `question` `select-file` `select-dir` `ask-password` `ask-secret` | Modal prompts (BLOCKING) |
| `skill`  | `show` `install` | Embedded Claude Code skill |
| `schema` | (drill-down catalog) | Self-describing surface |
| escape   | `rpc` `postback` | Raw JSON-RPC / postback |

Run `rstudio schema` for the auto-generated catalog of every action.

## [AI-native](https://github.com/aclemen1/ai-native-cli) pattern

This CLI follows the [AI-native CLI](https://github.com/aclemen1/ai-native-cli)
pattern — embedded skill, schema drill-down, JSON envelope. The
design rationale and the wider landscape (Google Workspace CLI,
Linearis, prior writings on the term) live in the spec.

The CLI ships an embedded skill markdown. An LLM agent loads only the
skill, then discovers the surface on demand:

```sh
rstudio schema                  # level 0: every action with category + summary
rstudio schema editor           # level 1: actions in 'editor'
rstudio schema editor open      # level 2: full ActionSpec (params, examples, errors,
                                #          rstudioapi_fn, rpc_method)
```

Each level-2 entry traces back to its `rstudioapi` function and the
JSON-RPC method (or postback) used, so the contract is fully
discoverable without reading the source code.

```sh
rstudio skill install           # writes ./.claude/skills/rstudio/SKILL.md
rstudio skill show              # prints the embedded skill markdown
rstudio version                 # 0.6.2
```

This keeps the agent's context window lean — no tool descriptions are
loaded for actions the agent never uses in a session.

## Requirements

Either:

- **RStudio Server** (Linux). Run the CLI inside a session's embedded
  terminal — same Unix user as the `rsession` process. A browser tab
  must be attached to the session (the CLI reads the active client id
  from the on-disk session state and **never** calls `client_init`,
  which would invalidate that client).
- **RStudio Desktop** (macOS). Run the CLI in any terminal as the user
  who launched RStudio. The CLI auto-discovers the running `rsession`
  process (TCP port + shared secret from argv/environ); pass
  `--port`/`--secret` to override.

Linux Desktop and Windows are out of scope for v0.5.x.

## Install

### Homebrew (macOS, Linuxbrew on Linux)

```sh
brew install aclemen1/tap/rstudio-cli
rstudio version
```

### From a release binary

Builds are attached to each [GitHub Release](https://github.com/aclemen1/rstudio-cli/releases)
for four targets: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`,
`x86_64-apple-darwin`, `aarch64-apple-darwin`.

```sh
# Replace VERSION and TARGET as needed
curl -sL "https://github.com/aclemen1/rstudio-cli/releases/download/vVERSION/rstudio-cli-vVERSION-TARGET.tar.gz" \
  | tar -xzC ~/.local/bin
chmod +x ~/.local/bin/rstudio
rstudio version
```

### From source

```sh
cargo install --git https://github.com/aclemen1/rstudio-cli rstudio-cli
# or
git clone https://github.com/aclemen1/rstudio-cli && cd rstudio-cli
cargo build --release && cp target/release/rstudio ~/.local/bin/
```

### Skill

```sh
rstudio skill install   # ./.claude/skills/rstudio/SKILL.md
```

## Auto-detection

The CLI reads the following at each invocation:

| Var                       | Purpose                                              | Fallback                                                                  |
|---------------------------|------------------------------------------------------|---------------------------------------------------------------------------|
| `$RSTUDIO_SESSION_STREAM` | Unix socket name under `$RS_SESSION_TMP_DIR`         | scan `$RS_SESSION_TMP_DIR` for a socket owned by the current uid; single match wins, error otherwise |
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

`error.kind` is one of: `user_error`, `r_error`, `rpc_error`,
`timeout`, `session_unavailable`, `internal`.

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

# Spawn a shell command in a fresh terminal and watch its output.
ID=$(rstudio --format text term run 'cargo build' --working-dir ~/projects/foo | jq -r .id)
sleep 5; rstudio term buffer "$ID" --limit 30

# Surface lint-style feedback in the Markers pane.
rstudio pane markers --name 'lint' --markers '[
  {"type":"warning","file":"/path/to/foo.R","line":12,"message":"Unused variable"}
]'

# Inspect the active R environment.
rstudio env list --pattern '^df_'
rstudio env info mtcars

# Background job.
JOB=$(rstudio --format text job add --name 'indexing' --progress-units 100 --running | jq -r .id)
rstudio job set-progress "$JOB" 50
rstudio job set-state    "$JOB" succeeded

# Whole-session info on startup.
rstudio session info
```

## Tests

```sh
cargo test                                # unit tests, fast, no live session needed
cargo test --test live -- --ignored       # integration tests against a live session
```

The integration tests skip silently (`SKIP: no live RStudio session
available`) when no `rsession` socket is reachable, so the suite is
safe to run anywhere. They never mutate the user's UI: read-only
paths only (`r exec` round-trips, `editor read`, `env list`,
`term list`, schema registry shape).

## Concurrency model

R is single-threaded; the rsession serialises every `r exec` (and any
other `execute_r_code`-based call) into a FIFO. Two concurrent calls
do **not** run in parallel — total wall time ≈ sum of per-call time.

- `--timeout 0` on a long `r exec` blocks every subsequent `r`-style
  call until it returns. Use deliberately.
- For real parallelism (running shell commands or external processes),
  use `term exec` / `term run` — the Terminal pane spawns a separate
  pty/process, not bound to the R FIFO.
- Postbacks (`editor edit`) and `console_input` (`r send`) don't go
  through the R queue, so they aren't subject to the FIFO.

## Hard "do not"

- `rstudio rpc client_init` is **blacklisted** at the CLI level —
  calling it invalidates the active browser client and resets the
  user's RStudio session.
- Avoid `connection_test` (raw RPC) for anything that can throw: R
  errors raised through it leak into the user's visible console. The
  `r` and `editor` wrappers route through `execute_r_code` instead,
  which is silent.

## License

MIT — see [LICENSE](LICENSE).
