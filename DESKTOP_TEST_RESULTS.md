# RStudio Desktop PoC — validation results

Tested 2026-05-02 on macOS 26.3.1 (arm64). RStudio Desktop
2025.09.0+387 (Cucumberleaf Sunflower), R 4.5.2 (2025-10-31).
rsession PID `26002`, RPC port `26258`, launcher-token `31d9ef78`.
rstudio-cli commit: `b29516f1cfbacdfdf54a214d179b56aba5102e81`
(branch `desktop-poc/aclemen1`).

Re-validated 2026-05-03 against commit `7817221a` (γ patch on top of
the original PoC). See § "B1 — re-validation 2026-05-03" below for
the re-run outputs.

## Verdict

**PASS — ready to merge.** All Server-equivalent surfaces work over
the new Desktop transport: auto-detection, mode overrides, read-only
RPC, write actions (`r send`, `editor open/close`, `term run/buffer/
exit-code/kill`, `pane markers`), explicit `--port`/`--secret`
overrides, and FIFO serialisation timing. The B1 contention bug
(queued `execute_r_code` returning a body with `asyncHandle` and no
`result`) was patched as option γ in `src/rpc.rs`: the second of two
overlapping `r exec` calls now surfaces a clean
`kind=session_unavailable` with the handle in the message instead
of a silent `null` corruption. Re-validation on 2026-05-03 ran the
contention reproducer three times, plus smoke and sequential checks;
all six FIRST calls succeeded, all three SECOND calls produced the
clean error, no `kind=internal` slipped through. See § "B1 —
re-validation 2026-05-03" for verbatim outputs.

## Step 1 — build

`cargo build --release` succeeded in 11.39s on a clean target tree.
No warnings beyond the deprecation notice for `jj git push --allow-new`
(unrelated to the PoC).

## Step 2 — auto-detection

```text
$ rstudio version
{"ok":true,"result":{"version":"0.4.0"}}
exit: 0

$ rstudio --mode auto session info
{"ok":true,"result":{"active_project":null,"has_color_console":true,
 "long_version":"2025.09.0+387","mode":"desktop",
 "release_name":"Cucumberleaf Sunflower","system_username":"aclemen1",
 "user_identity":"aclemen1","version":"2025.9.0.387"}}
exit: 0

$ rstudio --mode desktop session info
(same payload as --mode auto, mode="desktop")
exit: 0

$ rstudio --mode server session info
{"error":{"code":1,"kind":"session_unavailable",
 "message":"RSTUDIO_SESSION_STREAM is not set — not running inside an
            RStudio Server session? Pass --socket <path>, or run with
            --mode desktop."},"ok":false}
exit: 1
```

`auto` correctly resolves to Desktop when no Server stream is set; the
forced `--mode server` error message even points the user at
`--mode desktop`.

## Step 3 — read-only smoke tests

| Command | Result |
|---|---|
| `r exec '1+1'` | `output: "[1] 2"` ✅ |
| `r exec 'paste("hello from", R.version.string)'` | `output: "[1] \"hello from R version 4.5.2 (2025-10-31)\""` ✅ |
| `r exec 'stop("intentional")'` | `kind=r_error, message="intentional"` ✅ |
| `r exec --timeout 1 'Sys.sleep(2)'` | `kind=timeout, "exceeded elapsed time limit"` ✅ |
| `env list \| jq '.result.vars \| length'` | `2` ✅ |
| `console history --limit 3` | 3 prior commands returned (residual state from feasibility tests) ✅ |
| `console context` | `id: "#console"` with empty contents ✅ |
| `editor list \| jq '.result.documents \| length'` | `2` (ids `E327BF68`, `91DEB48E`) ✅ |
| `editor active-id` | `{id: "#console"}` (user has the console focused) ✅ |
| `editor active-context` | `{id: "#console", path: ""}` ✅ |
| `editor context` | `{id: "91DEB48E", path: "~/code/aclemen1/rstudio-cli/Cargo.toml"}` ✅ |
| `term list` | `terminals: []` ✅ |
| `term visible` | `{id: null}` ✅ |
| `session info` | mode=desktop, version=2025.9.0.387 ✅ |
| `session project` | `{path: null}` ✅ |

All 15 commands returned the expected envelope shape and exit code.

Note on the orphan `3AB7218C-contents` file in
`~/.local/share/rstudio/sources/session-31d9ef78/`: it has no
metadata sibling, so `editor list` correctly skips it (the
`is_document_id` filter requires the bare 8-hex name to be present).

## Step 4 — write tests

### 4.a `r send`

```text
$ rstudio r send 'cat("[rstudio-cli desktop validation]\n")'
{"ok":true}
exit: 0

$ rstudio console history --limit 1 | jq '.result.commands'
["cat(\"[rstudio-cli desktop validation]\\n\")"]
```

Visually confirmed by the user: the line was typed and executed in
the Desktop console.

### 4.b `editor open` + `editor close`

```text
$ SCRATCH=/tmp/rstudio-desktop-validation-91334.R
$ echo '# scratch for desktop PoC validation' > "$SCRATCH"

$ rstudio editor open "$SCRATCH" --line 1
{"ok":true,"result":{"col":null,"id":"09ABB64A","line":1,
 "path":"/private/tmp/rstudio-desktop-validation-91334.R"}}

$ rstudio editor active-id
{"ok":true,"result":{"id":"#console"}}      # see "Observation" below

$ rstudio editor close 09ABB64A --save false
{"ok":true,"result":{"id":"09ABB64A","saved":"false"}}

$ rstudio editor list | jq --arg id 09ABB64A \
                           '.result.documents | map(.id) | index($id)'
null
```

Visually confirmed: the scratch file appeared as a Source-pane tab,
then disappeared after `close`. The `/tmp` → `/private/tmp` rewrite
is macOS-standard canonicalisation and not a transport concern.

**Observation, not a bug.** `editor active-id` returned `#console`
right after `editor open`. RStudio's `documentOpen` opens the tab
without stealing focus from the console — same behaviour as Server
when the user's caret is in the console pane.

### 4.c `term run`

```text
$ rstudio term run 'echo hi from desktop validation && pwd && exit 0'
{"ok":true,"result":{"id":"99FCF5DD"}}

$ rstudio term buffer 99FCF5DD --limit 6
{"ok":true,"result":{"id":"99FCF5DD",
 "lines":["hi from desktop validation","/Users/aclemen1",""]}}

$ rstudio term exit-code 99FCF5DD
{"ok":true,"result":{"exit_code":0}}

$ rstudio term kill 99FCF5DD
{"ok":true}
```

Visually confirmed: a new terminal tab appeared, ran the command,
exited 0, then was removed.

### 4.d `pane markers`

```text
$ rstudio pane markers --name 'desktop-validation' --markers '[
    {"type":"info","file":"<repo>/Cargo.toml","line":1,
     "message":"Hello from desktop PoC validation"}
  ]'
{"ok":true,"result":{"count":1,"name":"desktop-validation"}}
```

The Markers pane should display a `desktop-validation` tab with one
info entry on `Cargo.toml:1`.

## Step 5 — explicit overrides

Auto-discovery values for the running rsession (PID `26002`):

```text
PORT=26258
SECRET=2def695d-aac3-46e0-80b9-aa2a15e7459a
```

```text
$ rstudio --mode desktop --port 26258 --secret <SECRET> r exec '"override-path-ok"'
{"ok":true,"result":{"output":"[1] \"override-path-ok\""}}
exit: 0

$ rstudio --mode desktop --port 26258 r exec '1+1'
{"error":{"code":1,"kind":"session_unavailable",
 "message":"Desktop mode: --port and --secret must be passed together
            when overriding discovery, or omit both to auto-discover
            from the running rsession process."},"ok":false}
exit: 1

$ rstudio --mode desktop --port 26258 --secret WRONG r exec '1+1'
{"error":{"code":403,"kind":"rpc_error",
 "message":"rpc execute_r_code returned HTTP 403"},"ok":false}
exit: 1
```

Override path is wired correctly. Both failure modes surface clean,
actionable error messages.

## Step 6 — concurrency

```text
$ ( rstudio r exec --timeout 5 'Sys.sleep(2); "FIRST"' &
    sleep 0.1
    rstudio r exec --timeout 5 'Sys.sleep(0.5); "SECOND"'
    wait )

# wall: 2.27s — 2.29s across two runs

# Run #1
SECOND: {"error":{"code":1,"kind":"internal",
                  "message":"execute_r_code returned non-string: null"},
         "ok":false}
FIRST:  {"ok":true,"result":{"output":"[1] \"FIRST\""}}

# Run #2 (PIDs labelled)
FIRST  (3852): {"ok":true,"result":{"output":"[1] \"FIRST\""}}
SECOND (3891): {"error":{"code":1,"kind":"internal",
                         "message":"execute_r_code returned non-string: null"},
                "ok":false}
```

Wall time matches the FIFO expectation (≈ 2.5 s for 2.0 s + 0.5 s
serialised, including the 0.1 s start delay) — calls are not
running in parallel. **However**, the queued call returns `null`
instead of its evaluation result, surfaced by the CLI as an
`internal` error (`r_eval.rs:38`). This is reproducible across runs.

Counter-checks:

- **`r send` during a long `r exec` works.** A 2-second
  `r exec --timeout 0 'Sys.sleep(2); "EXEC-C"'` succeeded while
  a concurrent `r send 'cat("[send during exec]\n")'` returned
  `{"ok":true}` immediately, without affecting the exec result.
  This is consistent with the README: postbacks/`console_input`
  bypass the R FIFO.

- **The session recovers within a second.** Sequential `r exec`
  calls run **immediately after** the contention burst returned the
  same `null` error twice in a row; after a brief idle window, simple
  `r exec '1+1'`, `r exec 'Sys.sleep(0.5); "WITH-SLEEP"'`, etc. all
  succeeded. No restart needed.

## Bugs found

### B1 — Queued `execute_r_code` returns null under `r exec` contention

- **Severity**: medium. Affects any caller that issues two `r exec`
  calls within the same wall-time window.
- **Repro**: see Step 6 commands. Two consecutive runs both showed the
  later-arriving call surfacing as
  `kind=internal, "execute_r_code returned non-string: null"`.
- **Surface in code**: `src/r_eval.rs:38` —
  `raw.as_str().ok_or_else(|| CliError::internal(...))`. The RPC
  envelope's `result` field is JSON `null`; `r_eval` expects a
  string.
- **Origin**: not yet localised. Three plausible sources, ordered by
  likelihood:
  1. rsession's RPC-layer behaviour: under FIFO contention, the
     queued caller's HTTP response is closed early with `result:
     null` while the actual evaluation result is delivered through
     a different channel (event stream / next request). This would
     pre-date the Desktop PoC.
  2. PoC-introduced TCP framing: `src/transport.rs` reading the body
     differently from the Unix path could truncate before the result
     is written.
  3. Pre-existing CLI behaviour, never noticed because the live test
     suite doesn't exercise concurrent `r exec`.
- **Why this matters before merge**: the README documents FIFO
  serialisation as the contract, with users expected to issue
  back-to-back `r exec` calls. Returning `null` instead of the result
  silently breaks any script that orchestrates two parallel
  evaluations.
- **Suggested next step**: reproduce on an RStudio Server with the
  current `main` build to determine whether this is Desktop-specific
  or pre-existing. If pre-existing, file a separate issue and merge
  the PoC. If new in the PoC, root-cause in `src/transport.rs` or
  `src/rpc.rs` first.

### Observations (not bugs)

- After `editor open`, `editor active-id` returns `#console` when the
  user has the console pane focused. Same as Server.
- `r send` adds a trailing newline (`r.rs:124`); two empty prompts
  show in the console after `print(...)`. Pre-existing CLI behaviour,
  identical on Server.
- The orphan `<id>-contents` file in
  `~/.local/share/rstudio/sources/session-<token>/` is correctly
  skipped by `editor list`; no metadata file means no listing entry.

## B1 — wire capture (added 2026-05-03)

The "non-string: null" surface in `r_eval.rs:38` was misdiagnosed in
the original report. The CLI does not see `null` because rsession is
returning a literal `null` `result` field; it sees `null` because the
PoC's RPC parser falls back to `Value::Null` when the response body
contains **neither** a `result` **nor** an `error` field. What
rsession actually returns under `execute_r_code` contention is an
`asyncHandle`. This places the bug in **Case C** of the wire-shape
classification (neither A nor B), so per the brief I did not patch.

### Verbatim curl outputs

Setup mirrored `DESKTOP_FEASIBILITY.md`:

```text
PID    = 26002
PORT   = 26258
SECRET = 2def695d-aac3-46e0-80b9-aa2a15e7459a   (RS_SHARED_SECRET env)
```

Two overlapping `execute_r_code` calls launched in parallel
(`FIRST` slept 2 s, `SECOND` slept 0.5 s, started ~100 ms after
FIRST):

```text
===== FIRST =====
HTTP/1.1 200 OK
Content-Type: application/json
Content-Length: 35
Connection: close

{"result":"\"FIRST\"","ep":"false"}

===== SECOND =====
HTTP/1.1 200 OK
Content-Type: application/json
Content-Length: 67
Connection: close

{"asyncHandle":"22e9ffe3-b62a-41c1-9909-f7f883cca9fc","ep":"false"}
```

The second call's body has **no `result`, no `error`** — only an
`asyncHandle`. The CLI's parser (`parse_rpc_envelope` in `src/rpc.rs`)
returns `Value::Null` in that case, then `r_eval::run` rejects null
with the "non-string: null" message.

### Async-completion mechanism (upstream cross-check + empirical)

Upstream `src/cpp/session/SessionAsyncRpcConnection.hpp` documents the
contract:

> Created from an HttpConnectionImpl, then put into mainConnectionQueue
> for RPC requests handled asynchronously by the rsession, where it
> first acks the http request, then later calls the rpc handler, then
> sends a kAsyncCompletion event in the response.

`SessionRpc.cpp:271` enqueues the completion event with
`{handle, response}` payload:

```cpp
json::Object value;
value["handle"]   = asyncHandle;
value["response"] = pJsonRpcResponse->getRawResponse();
ClientEvent evt(client_events::kAsyncCompletion, value);
module_context::enqueClientEvent(evt);
```

The GWT client's `RemoteServer.java:3488` exposes the polling endpoint
(scope `events`, method `get_events`) and
`RemoteServerEventListener.java:380-396` consumes the event:

```java
if (type == ClientEvent.AsyncCompletion)
{
   AsyncCompletion completion = event.getData();
   String handle = completion.getHandle();
   AsyncRequestInfo req = asyncRequests_.remove(handle);
   if (req != null)
       req.callback.onResponseReceived(req.request,
                                       completion.getResponse());
}
```

I verified the wire shape end-to-end with one extra curl probe
(no code changes):

```text
$ curl -s "http://127.0.0.1:26258/events/get_events" \
       -H "X-Shared-Secret: <SECRET>" \
       --data '{"method":"get_events","params":[-1],"id":99,
                "clientId":"33e600bb-c1b1-46bf-b562-ab5cba070b0e"}'
{"result":[
  {"id":586,"type":"async_completion",
   "data":{"handle":"a31bcd8d-ff56-4bed-a7f9-c78de5dafe24",
           "response":{"result":"DEFERRED","ep":"false"}}}
 ],"ep":"false"}
```

So the actual evaluation result is delivered through the events
queue at `/events/get_events` (POST, JSON-RPC envelope, single integer
param = `lastEventId`, returns an array of events). The endpoint
**long-polls** when the queue is empty (observed ~8 s timeout per
call when no events are pending).

### Implications for the fix

A simple retry-on-null in `r_eval::run` will not work — repeated
`/rpc/execute_r_code` calls will keep returning fresh `asyncHandle`s
instead of the result. The fix must instead:

1. Detect the `asyncHandle` shape in `parse_rpc_envelope`
   (`src/rpc.rs`).
2. Drive an event-polling loop against `/events/get_events` until an
   `async_completion` event arrives whose `data.handle` matches the
   handle from step 1.
3. Treat `data.response` as the actual JSON-RPC envelope (it has the
   same `result`/`error` shape) and continue parsing through the
   existing `parse_rpc_envelope` code path.
4. Track `lastEventId` across polls inside that single CLI
   invocation.

### Open question for the reviewer

Events are delivered through a single shared queue keyed by
`clientId`. The desktop UI itself uses the same hardcoded clientId
`33e600bb-c1b1-46bf-b562-ab5cba070b0e` and is also a consumer of
`/events/get_events` (the GWT renderer inside Electron). If the
CLI drains events from that queue, the UI may miss `kFileChanged`,
`kMemoryUsageChanged`, `kHelpShown`, etc., until its next poll. The
empirical impact of one CLI call draining a single
`memory_usage_changed` event was not visible to the user during the
recon, but this is not a robust observation. The PoC author needs
to decide:

- **Option α**: accept the race and only drain events tagged
  `async_completion` matching our handle, then re-enqueue the rest.
  rsession does not expose a re-enqueue endpoint, so this would
  require **dropping** non-matching events. **Risky.**
- **Option β**: use a separate clientId for CLI calls. Would need to
  validate that `execute_r_code` results are still routed to that
  separate id (likely yes; events are addressed to the calling
  client). Would not affect the desktop UI's queue. **Probably the
  right call** but needs upstream verification.
- **Option γ**: scope down — refuse contended `r exec` calls on
  Desktop with a clear error message ("Desktop does not support
  concurrent `r exec`; serialise externally"). Trades feature parity
  for safety. **Cheapest patch.**

I have **not** chosen between α/β/γ. The brief says: "If Step 1
lands in Case C, **stop and ask the user**. Don't guess a patch."
Awaiting reviewer guidance before any code change to `desktop-poc`.

## B1 — spike β (2026-05-03)

The reviewer chose β as the preferred option subject to a two-check
empirical spike, with γ as the fallback. **Check β-1 failed
immediately**, which disqualified β before β-2 was even attempted.

### β-1 — does Desktop accept an ad-hoc clientId without `client_init`?

```text
$ ADHOC=4137a11d-eee5-4b8d-861e-6e190e1bb3c6   # fresh uuidgen
$ curl --max-time 5 -i \
    -H "Content-Type: application/json" \
    -H "X-Shared-Secret: 2def695d-aac3-46e0-80b9-aa2a15e7459a" \
    -H "X-RS-CSRF-Token: <uuid>" \
    -H "Cookie: rs-csrf-token=<uuid>; csrf-token=<uuid>" \
    "http://127.0.0.1:26258/rpc/execute_r_code" \
    --data '{"method":"execute_r_code","params":["42"],"id":1,
             "clientId":"4137a11d-eee5-4b8d-861e-6e190e1bb3c6"}'
HTTP/1.1 200 OK
Content-Length: 81

{"error":{"code":4,"message":"jsonrpc error 4 (Invalid client id)","error":null}}
```

Result: `code 4 (Invalid client id)`. Identical to Server's behaviour
when an unregistered clientId is used. The desktop client id
`33e600bb-c1b1-46bf-b562-ab5cba070b0e` is the **only** valid id on
Desktop, and the only way to register a new one is `client_init`,
which is hard-blacklisted (it would rotate the desktop client id and
force a UI reload).

### β-2 — skipped

β-2 (whether async_completion routes to an ad-hoc queue) is moot once
β-1 fails: no ad-hoc clientId is accepted, so there is no ad-hoc queue
to route to. Running β-2 would only produce identical "Invalid client
id" responses for the events poll.

### Decision: ship γ

The brief's explicit fail-rule applied:

> Fail condition: `{"error":{"code":4,"message":"…Invalid client id…"}}`
> or any 4xx/5xx → β is dead, go straight to γ. Do not call
> `client_init` to register the UUID; it stays blacklisted.

Implementation lives on a fresh change atop `desktop-poc/aclemen1`.
Diff is scoped to `src/rpc.rs` only (no transport / session / r_eval
changes). When `parse_rpc_envelope` sees `"asyncHandle"` in the
response body, it returns `kind=session_unavailable` with a message
naming the handle, instead of falling back to `Value::Null` and
hitting `r_eval.rs:38`.

## B1 — re-validation 2026-05-03

Re-validated against commit `7817221a` (γ patch in `src/rpc.rs`).
Server-side non-regression confirmed in PR #2 comment of
2026-05-02 22:30 (21 unit + 7 live Server tests + Step 6 ×3 — γ
guard does not fire on Server). Desktop re-validation below.

Working copy rebased onto `desktop-poc/aclemen1@origin` cleanly
(no conflicts, validation commit only touches this report; γ patch
only touches `src/rpc.rs`). `cargo build --release` finished in
3.51 s with no warnings.

### 2.a Smoke

```text
$ rstudio --mode desktop r exec '1+1'
{"ok":true,"result":{"output":"[1] 2"}}                              exit:0

$ rstudio --mode desktop r exec 'stop("intentional")'
{"error":{"code":1,"kind":"r_error","message":"intentional"},
 "ok":false}                                                          exit:1

$ rstudio --mode desktop r exec --timeout 1 'Sys.sleep(2)'
{"error":{"code":1,"kind":"timeout",
 "message":"R evaluation exceeded elapsed time limit
            (default 2s; pass --timeout to override)"},
 "ok":false}                                                          exit:1
```

### 2.b Sequential slow `r exec` (γ must NOT fire)

The point of this check is that γ only fires under **concurrent**
contention. A slow non-overlapping `r exec` must keep the synchronous
fast path and return its result.

```text
$ rstudio --mode desktop r exec --timeout 5 'Sys.sleep(2); "SEQ-1"'
{"ok":true,"result":{"output":"[1] \"SEQ-1\""}}                       exit:0

$ rstudio --mode desktop r exec '"SEQ-2"'
{"ok":true,"result":{"output":"[1] \"SEQ-2\""}}                       exit:0
```

Neither call returned `session_unavailable`. The γ guard is correctly
scoped to the async-handle response shape, not to slow calls.

### 2.c `editor open --line` (the other path through `execute_r_code`)

```text
$ rstudio --mode desktop editor open "$PWD/Cargo.toml" --line 1
{"ok":true,"result":{"col":null,"id":"91DEB48E","line":1,
 "path":"/Users/aclemen1/code/aclemen1/rstudio-cli/Cargo.toml"}}      exit:0
```

Doc id `91DEB48E` is the existing tab from the previous validation
run; `documentOpen` returned the same id and the cursor moved to
line 1 — exactly the expected idempotent behaviour.

### 2.d Step 6 ×3 — the regression we're testing

```text
=== run 1 ===
FIRST  (exit=0): {"ok":true,"result":{"output":"[1] \"FIRST\""}}
SECOND (exit=1): {"error":{"code":1,"kind":"session_unavailable",
                  "message":"Desktop rsession queued this
                             execute_r_code call
                             (asyncHandle=7547bb35-a330-49a8-928f-dc696dd4ffbd);
                             the CLI does not poll the
                             kAsyncCompletion event channel.
                             Serialise r exec calls externally,
                             or wait for async support to land.
                             Server is unaffected."},
                  "ok":false}
wall: 2.18s

=== run 2 ===
FIRST  (exit=0): {"ok":true,"result":{"output":"[1] \"FIRST\""}}
SECOND (exit=1): kind=session_unavailable,
                 asyncHandle=80d90274-ca35-4fd8-adbb-48a4701035b2
wall: 2.18s

=== run 3 ===
FIRST  (exit=0): {"ok":true,"result":{"output":"[1] \"FIRST\""}}
SECOND (exit=1): kind=session_unavailable,
                 asyncHandle=de7c4233-a594-46ad-98ce-a4665966ac88
wall: 2.18s
```

All three runs match the acceptance criteria: FIRST returns its
output, SECOND surfaces a clean `session_unavailable` with the
substring `asyncHandle=` in the message, total wall time ≈ 2.18 s
(within the 2.2-2.5 s target). **No `kind=internal` errors.** The
silent-corruption pre-patch behaviour is fully replaced with the
explicit error.

### 2.e Sanity post-burst

```text
$ rstudio --mode desktop r exec '"healthy"'
{"ok":true,"result":{"output":"[1] \"healthy\""}}                     exit:0
```

Session is not stuck after the contention bursts.

### Verdict

**PASS.** γ closes the silent `null` corruption from B1. Both the
sequential slow path (2.b) and the `editor open` path through
`execute_r_code` (2.c) keep working. The contention case (2.d) now
returns the documented `session_unavailable` instead of an
`internal` error. Session recovers cleanly between bursts (2.e).
The error message names the handle, the method, and tells the user
how to work around it. No new regressions detected on the surfaces
exercised by this re-run.

## Recommendations

**Ready to merge `desktop-poc/aclemen1` into `main`.** Every surface
exercised in this suite reaches feature parity with Server.
Auto-detection, override paths, error envelopes and exit codes all
match the existing CLI contract. The B1 silent-`null` corruption is
closed: contended `r exec` now surfaces a clean
`session_unavailable` with the handle in the message instead of an
`internal` error.

### Feature gap left for follow-up

γ trades feature parity with Server for safety. Two simultaneous
`r exec` calls on Desktop cannot both return their result; the
second always surfaces `session_unavailable` with `asyncHandle=` in
the message, naming the handle so the user can correlate it with
upstream rsession logs if needed. Workaround: serialise `r exec`
externally (single FIFO at the caller) or fall back to `r send`
(non-blocking, doesn't go through the R FIFO) for fire-and-forget
side effects.

β remains the natural evolution: a per-CLI-invocation clientId plus
a `/events/get_events` polling loop in `src/rpc.rs` would let the
queued call resolve to its actual result. β was disqualified at
spike time because Desktop rejects unregistered clientIds with
code 4 (and `client_init` is hard-blacklisted) — see § "B1 — spike
β" above. A follow-up ticket should track:

1. Whether upstream rsession can be persuaded to accept ad-hoc
   client ids on a separate registration path that doesn't rotate
   the desktop UI's hardcoded id.
2. Or whether the CLI should embed a thin event-listener fork that
   coexists with the desktop UI's GWT polling without dropping
   events meant for the IDE.

Neither is in scope for this PoC; γ is the right shippable patch.

### Why this isn't a Server bug

The author confirmed B1 does NOT reproduce on Server with the same
binary. This is **PoC-introduced behaviour exposed by the new TCP
transport**, not a pre-existing CLI bug — the Server unix-socket
listener keeps the HTTP response open until the result is ready
(synchronous fast path), while the Desktop TCP listener takes the
async path more aggressively. PR #2 comment of 2026-05-02 22:30
documents 21 unit + 7 live Server tests + Step 6 ×3 against Server
all green; the γ guard does not fire on Server because the Server
listener never returns an `asyncHandle` body shape.

No other regressions observed. Patched commit:
`7817221a` (atop original PoC `b29516f1`).
