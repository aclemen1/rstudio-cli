# RStudio Desktop support — feasibility report

Authored 2026-05-02. Recommendation: **GO**. The minimum surface needed
for `rstudio version` and `rstudio r exec '1+1'` against RStudio Desktop
is reachable with a small, additive change to the existing transport.

This report is the artefact required by step 3 of the desktop-support
mission. It covers reconnaissance, manual curl round-trip, upstream
cross-check, and a robustness assessment. No code is changed yet.

## Test environment

- Host: macOS (Darwin 25.3.0, arm64).
- RStudio Desktop: `/Applications/RStudio.app`,
  binary `Contents/Resources/app/bin/rsession-arm64`.
- R: 4.5.2 (Homebrew).
- A single Desktop window opened; one rsession child process spawned.

## 1. Reconnaissance — verbatim findings

### rsession process

```text
PID  : 26002
argv : /Applications/RStudio.app/Contents/Resources/app/bin/rsession-arm64
       --config-file none
       --program-mode desktop
       --www-port 26258
       --launcher-token 31d9ef78
       --show-help-home 1
```

### rsession environment (filtered to what matters here)

```text
RS_SHARED_SECRET=2def695d-aac3-46e0-80b9-aa2a15e7459a
RS_LOG_LEVEL=WARN
RSTUDIO_DESKTOP_EXE=/Applications/RStudio.app/Contents/MacOS/RStudio
RSTUDIO_FALLBACK_LIBRARY_PATH=/var/folders/.../rstudio-fallback-library-path-...
USER=aclemen1
HOME=/Users/aclemen1
```

Notably absent: `RS_LOCAL_PEER`, `RS_SESSION_TMP_DIR`, `RSTUDIO_PROGRAM_MODE`,
`RSTUDIO_SESSION_ID`, `RSTUDIO_SESSION_STREAM`. None of the variables
that drive Server-mode auto-detection are set on macOS Desktop.

### TCP listeners owned by rsession (PID 26002)

```text
TCP 127.0.0.1:26258 (LISTEN)   <- matches --www-port; this is the RPC port
TCP 127.0.0.1:31699 (LISTEN)   <- secondary; R's internal httpd, returns
                                  "R: httpd error" HTML; not an RPC port
```

There is **one** RPC listener on loopback, on the port advertised in argv.

### Filesystem layout

| path | state |
|---|---|
| `~/Library/Application Support/RStudio/sessions/active/` | does not exist |
| `~/.local/share/rstudio/sessions/active/` | exists, empty |
| `~/.local/share/rstudio/sources/session-31d9ef78/` | exists, holds `lock_file` and per-document files |
| `~/.local/share/rstudio/rstudio-desktop.json` | `{"context_id": "028909F1"}` |

Two implications:

- The session id used in on-disk paths is the value of `--launcher-token`
  (here, `31d9ef78`). After opening a document, the per-doc files
  (`<docId>` metadata + `<docId>-contents` buffer) appear under
  `~/.local/share/rstudio/sources/session-31d9ef78/` with the same
  schema as Server — see Variant R below for the empirical check.
- **There is no `session-persistent-state` file on Desktop** — Desktop
  doesn't persist an `active-client-id` to disk because the desktop
  client id is hardcoded (see §3 below).

### Correction to the brief's hypothesis

The brief stated:

> Auth uses a shared secret (`--launcher-token`) passed by the Electron
> parent at spawn time.

This is **wrong**. Two distinct values are passed by the Electron
parent at spawn:

- `--launcher-token` (argv) — an opaque identifier reused as the session
  id in `sources/session-<token>/`. Not the auth secret.
- `RS_SHARED_SECRET` (env) — the actual auth secret. Compared against
  the `X-Shared-Secret` request header by the listener's `authenticate`.

Empirical confirmation in §2; upstream confirmation in §3.

## 2. Manual curl round-trip

All requests below were issued from the same machine, against the
running rsession of §1. CSRF tokens are random UUIDs.

### Baseline (no auth) — expected to fail

```text
$ curl -i http://127.0.0.1:26258/rpc/get_environment_state \
    -H "Content-Type: application/json" \
    --data '{"method":"get_environment_state","params":[],"id":1,
             "clientId":"33e600bb-c1b1-46bf-b562-ab5cba070b0e"}'
HTTP/1.1 403 Forbidden
Connection: close
```

### Variant A — `X-Shared-Secret: $RS_SHARED_SECRET` ✅

```text
$ curl -i http://127.0.0.1:26258/rpc/get_environment_state \
    -H "Content-Type: application/json" \
    -H "X-Shared-Secret: 2def695d-aac3-46e0-80b9-aa2a15e7459a" \
    -H "X-RS-CSRF-Token: <uuid>" \
    -H "Cookie: rs-csrf-token=<uuid>; csrf-token=<uuid>" \
    --data '{"method":"get_environment_state","params":[],"id":1,
             "clientId":"33e600bb-c1b1-46bf-b562-ab5cba070b0e"}'
HTTP/1.1 200 OK
Content-Type: application/json
Content-Length: 239

{"result":{"environment_monitoring":true,"environment_list":[],
"context_depth":0,"call_frames":[],"function_name":"",
"environment_name":".GlobalEnv","environment_is_local":false,
"use_provided_source":false,"function_code":""},"ep":"false"}
```

### Variant B — `X-Shared-Secret: $LAUNCHER_TOKEN` (control)

```text
HTTP/1.1 403 Forbidden
```

Confirms the launcher token is **not** the auth secret.

### Variant C — `X-RS-Token: $RS_SHARED_SECRET` (control)

```text
HTTP/1.1 403 Forbidden
```

Confirms the header name must be exactly `X-Shared-Secret`.

### Variant D — Server-style headers + `X-Shared-Secret` (compatibility)

Adding the existing `X-Session-Postback`, `X-RStudioUserIdentity`,
`X-RS-CSRF-Token`, and Cookie headers on top of `X-Shared-Secret`
returns 200 — Desktop simply ignores the extra Server-mode headers.

> **PoC consequence**: in `src/rpc.rs::auth_headers`, we can keep the
> existing four headers as-is and append `X-Shared-Secret` when the
> session is in Desktop mode. No need to fork the auth path.

### Variant F — silent execute_r_code('1+1')

```text
$ curl -i http://127.0.0.1:26258/rpc/execute_r_code \
    -H "Content-Type: application/json" \
    -H "X-Shared-Secret: 2def695d-aac3-46e0-80b9-aa2a15e7459a" \
    --data '{"method":"execute_r_code","params":["1+1"],"id":1,
             "clientId":"33e600bb-c1b1-46bf-b562-ab5cba070b0e"}'
HTTP/1.1 200 OK
Content-Type: application/json
Content-Length: 27

{"result":"2","ep":"false"}
```

This is the proof that `rstudio r exec '1+1'` is reachable end-to-end
on Desktop with the same `execute_r_code` primitive used on Server.
Variants J and K below extend the proof to `editor open` and
`r send` — the three CLI surfaces the user explicitly asked to
validate before the go/no-go.

### Variant G — wrong client id (control)

```text
{"error":{"code":4,"message":"jsonrpc error 4 (Invalid client id)","error":null}}
```

Same RPC-level error code (`4`) as Server. The retry-on-invalid-id
logic in `RpcClient::rpc` keeps working; the only thing that changes
on Desktop is that the "refreshed" id is the same hardcoded UUID.

### Variant J — `editor open` (execute_r_code → rstudioapi::documentOpen)

```text
$ curl -i http://127.0.0.1:26258/rpc/execute_r_code \
    -H "X-Shared-Secret: 2def695d-aac3-46e0-80b9-aa2a15e7459a" \
    -H "Content-Type: application/json" \
    --data '{"method":"execute_r_code","params":[
       "r <- try(rstudioapi::documentOpen(\"/.../Cargo.toml\",
              line=14L, col=1L, moveCursor=TRUE), silent=TRUE);
        cat(if (inherits(r,\"try-error\"))
              paste(\"ERR:\", conditionMessage(attr(r,\"condition\")))
            else paste(\"RET:\", deparse(r)))"],
        "id":1,"clientId":"33e600bb-c1b1-46bf-b562-ab5cba070b0e"}'
HTTP/1.1 200 OK

{"result":"RET: \"91DEB48E\"","ep":"true"}
```

`Cargo.toml` opened in the Desktop editor pane, cursor at line 14
(visually confirmed). The hex string is the document id returned by
`rstudioapi::documentOpen`.

**Caveat unrelated to transport.** A first run returned
`{"result":"","ep":"false"}` because the host R didn't have the
`rstudioapi` package installed. Wrapping the call in `try()` exposed
the actual R-level error (`there is no package called 'rstudioapi'`).
After `install.packages("rstudioapi")` (executed through the same
RPC), the call returned the document id and the file opened.

This is the same package requirement that already applies on Server —
the CLI's `editor` surface needs `rstudioapi` on the host. It is not a
Desktop-specific concern. Worth flagging in the PoC's user-facing
docs, but no code change needed to make it surface a clean error.

### Variant K — `r send` (direct `console_input` RPC)

```text
$ curl -i http://127.0.0.1:26258/rpc/console_input \
    -H "X-Shared-Secret: 2def695d-aac3-46e0-80b9-aa2a15e7459a" \
    -H "Content-Type: application/json" \
    --data "{\"method\":\"console_input\",
             \"params\":[\"print('hello from rstudio-cli desktop PoC')\n\", \"\", 0],
             \"id\":1,
             \"clientId\":\"33e600bb-c1b1-46bf-b562-ab5cba070b0e\"}"
HTTP/1.1 200 OK
Content-Length: 15

{"result":null}
```

The line appeared and executed in the Desktop console (visually
confirmed). Cross-checked twice:

- via `get_recent_history`:

  ```text
  {"result":{"index":[0],"timestamp":[0.0],
             "command":["print('hello from rstudio-cli desktop PoC')"]},
   "ep":"false"}
  ```

- via a side-effect probe — `console_input("poc_var <- 42\n", ...)`
  followed by `execute_r_code("if (exists('poc_var')) cat('OK:', poc_var) else cat('MISSING')")`
  returned `OK: 42`. The console input was actually evaluated, not
  just queued.

So both `console_input` (visible) and `execute_r_code` (silent) take
the same `X-Shared-Secret` + hardcoded clientId path. The two
primitives the CLI relies on for `r send` and `r exec` are confirmed.

### Variant R — `editor list` (sources directory + get_source_document)

After the `documentOpen` round-trip, the on-disk source database
matches the Server layout exactly:

```text
$ ls ~/.local/share/rstudio/sources/session-31d9ef78/
3AB7218C-contents        <- orphan buffer (no metadata sibling)
91DEB48E                 <- metadata for the just-opened Cargo.toml
91DEB48E-contents        <- live buffer for the same doc
E327BF68                 <- metadata for another open doc
E327BF68-contents        <- buffer for that other doc
lock_file
```

- Filename pattern: 8-uppercase-hex doc id, with a sibling
  `<id>-contents` holding the live buffer text. Identical to Server.
- Metadata JSON shape is identical too (`id`, `path`, `type`,
  `relative_order`, `properties.cursorPosition`, `dirty`, `hash`, ...).
- `get_source_document(<id>)` over RPC returns the same payload as on
  Server:

  ```text
  id: 91DEB48E | path: ~/code/aclemen1/rstudio-cli/Cargo.toml
  type: toml | relative_order: 2 | dirty: False
  ```

> **PoC consequence**: `client_id::sources_dir_for(session_id)` and
> the directory-scanning logic in `editor::list_open` work as-is on
> Desktop. The only change required is to seed `session_id` with the
> value of `--launcher-token` parsed from rsession's argv (in our
> recon, `31d9ef78`). No new code path for source-DB enumeration.

### Variant H/I — missing or empty clientId

```text
{"error":{"code":4,"message":"jsonrpc error 4 (Invalid client id)","error":null}}
```

Even on Desktop, the RPC envelope's `clientId` field is **required**
and must equal the desktop client id constant. The hardcoded UUID is
the routing key, not a no-op.

The `client_init` blacklist in `src/commands/raw.rs` must remain in
place for Desktop too — calling it would still rotate the (hardcoded)
client and force a reload.

## 3. Upstream cross-check

All references to https://github.com/rstudio/rstudio (main).

### Header name is `X-Shared-Secret`

`src/cpp/session/http/SessionHttpConnectionUtils.cpp`, in
`connection::authenticate`:

```cpp
return secret == ptrConnection->request().headerValue("X-Shared-Secret");
```

### TCP listener auth = compare against `secret_` member

`src/cpp/session/http/SessionTcpIpHttpConnectionListener.hpp`,
`TcpIpHttpConnectionListener::authenticate`:

```cpp
bool authenticate(boost::shared_ptr<HttpConnection> ptrConnection)
{
   bool res = connection::authenticate(ptrConnection, secret_);
   ...
}
```

### `secret_` comes from `RS_SHARED_SECRET` env var

`src/cpp/session/SessionOptions.cpp:362`:

```cpp
secret_ = core::system::getenv("RS_SHARED_SECRET");
/* SECURITY: Need RS_SHARED_SECRET to be available to ... */
//core::system::unsetenv("RS_SHARED_SECRET");
```

The line is commented (the env var is *deliberately* not unset),
which is precisely what makes it readable from `ps eww` / `/proc`.

### Listener selection — Desktop = TCP unless RS_LOCAL_PEER is set

`src/cpp/session/http/SessionPosixHttpConnectionListener.cpp:96–113`:

```cpp
if (options.programMode() == kSessionProgramModeDesktop)
{
   std::string localPeer = core::system::getenv("RS_LOCAL_PEER");
   if (!localPeer.empty()) {
      ...
      s_pHttpConnectionListener = new LocalStreamHttpConnectionListener(
          ..., options.sharedSecret(), ...);
   }
   else {
      initTcpHttpConnectionListener(options.wwwAddress(), options.wwwPort(),
                                    options, options.sharedSecret(), "desktop");
   }
}
```

This means **on Linux Desktop, `RS_LOCAL_PEER` may be set** — in which
case rsession listens on a Unix domain socket at the path given by
that env var, not on TCP. The PoC scope is macOS-only (where it is
reliably empty), but the production-grade implementation will need
to handle three transports rather than two:

- Server, local stream (current).
- Desktop, TCP loopback (this PoC).
- Desktop, local stream (Linux only, when `RS_LOCAL_PEER` is set).

The third one is mostly the existing Unix code path, only the auth
needs to send `X-Shared-Secret` instead of relying on `SO_PEERCRED`.

### Desktop client id is hardcoded

`src/cpp/session/SessionPersistentState.cpp`:

```cpp
// always the same so that we can supporrt a restart of
// the session without reloading the client page
desktopClientId_ = "33e600bb-c1b1-46bf-b562-ab5cba070b0e";
```

Confirms: on Desktop, `client_id::read_active_client_id` should never
be called — the code must short-circuit to this constant.

## 4. Robustness assessment

| Concern | Verdict |
|---|---|
| Port discoverable | Yes. `--www-port` is in `rsession`'s argv. Reliably present (it's the only way the Electron parent reaches the child). |
| Auth secret discoverable | Yes. `RS_SHARED_SECRET` is in `rsession`'s env. Upstream deliberately leaves the env var set. |
| Reading another process's argv on macOS | OK via `ps -p <pid> -o args=`. Always works for processes the user owns. |
| Reading another process's env on macOS | OK via `ps eww -p <pid>`. Works for the user's own processes. **Not future-proof** — Apple has tightened process introspection in past releases. Current (Darwin 25.3) confirmed working. |
| Reading argv/env on Linux | OK via `/proc/<pid>/cmdline` and `/proc/<pid>/environ` for the user's own processes. Standard. |
| Secret stability across restarts | The secret is a fresh UUIDv4 per restart. The CLI must rediscover both port and secret on every invocation. This is fine — discovery is already per-invocation in Server mode. |
| Identifying the right rsession process | `pgrep -fU $(id -u) -- 'rsession.*--program-mode desktop'` then parse argv. If multiple matches (multiple Desktop windows), surface them and require a `--port` override. |
| Avoiding `client_init` on Desktop | The blacklist in `src/commands/raw.rs` must stay. Hardcoded client id ≠ dispensable client id. |
| `state_path` / `session-persistent-state` | Desktop never writes one. The PoC must skip the file read in `Session::detect` when in Desktop mode and use the hardcoded UUID directly. |

### Risks not relevant to the PoC scope

- Postbacks (`/rsession-local/postback/...`, used by `editor edit`) — not yet
  verified on Desktop. Out of scope for `r exec` round-trip; flag for the
  later mass-test pass.
- Multiple R sessions per Desktop process — Desktop allows multiple
  windows; whether they share one rsession or spawn multiple is not
  exercised here.
- Windows (named pipe) — explicitly out of scope.

## 5. Recommendation: GO

The Desktop transport is reachable with a small, additive change to
the existing code. Concretely:

1. **`src/transport.rs`** (renamed from `socket.rs`): expose the same
   `request()` surface but accept a `Backend::Unix(&Path)` or
   `Backend::Tcp(SocketAddr)` instead of a hardcoded `&Path`.
2. **`src/session.rs`**: introduce a `Mode { Server, Desktop }`
   enum; detect `Desktop` when no Server-mode envs/socket are found
   and a single `rsession --program-mode desktop` process owned by
   the user is running. Carry `Mode`, `socket_addr_or_path`, and
   an optional `shared_secret` on `Session`.
3. **`src/rpc.rs`**: when `mode == Desktop`, append
   `("X-Shared-Secret", session.shared_secret.unwrap())` to the
   `auth_headers` array. Everything else is unchanged.
4. **`src/client_id.rs`**: when `mode == Desktop`, return the hardcoded
   `33e600bb-c1b1-46bf-b562-ab5cba070b0e` and skip the file lookup.
5. **CLI flag**: `--mode {server,desktop,auto}` (default `auto`).
   `--port`, `--secret` for manual override.

`cargo test --lib` should keep passing. `cargo test --test live -- --ignored`
unchanged on Server. A Desktop equivalent of `tests/live.rs` can be
added incrementally — it is not required for the PoC.

## What changes for an existing Server-mode user

Nothing. If `$RSTUDIO_SESSION_STREAM` is set or
`/var/run/rstudio-server/rstudio-rsession/<stream>` exists, mode = Server.
Desktop discovery only runs when those checks fail.
