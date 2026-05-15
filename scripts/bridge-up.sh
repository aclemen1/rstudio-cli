#!/usr/bin/env bash
# Spawn a Dockerised, self-contained RStudio Server for integration testing.
# Everything runs inside the container: rserver, rsession, Chrome (headless),
# the CLI test binary. No host↔container tunnel, no host browser, no
# bind-mounts of the source tree.
#
# Why in-container: rsession's `accept()` loop on its Unix listening socket
# is serialised against its post-REPL event drain. From the host, when two
# CLI invocations cross that window (e.g. two consecutive `r send`), the
# second connection waits ~1 s in the kernel backlog. From inside the
# container the listener is reached without that hop and the bug disappears.
#
# Why Chrome inside too: the host browser required a host Chrome install
# (chrome.app on macOS, fragile in CI) and Python+uv for the DevTools
# protocol dance. Chromium in the container is self-contained; the container
# is the unit of reproducibility.
#
# Requires:
#   - Any Docker runtime. No bind-mount semantics involved.
#
# Writes bridge state to /tmp/rstudio-bridge-state.env:
#
#   export RSTUDIO_CLI_CLIENT_ID=<chrome's clientId>
#   export RSTUDIO_CLI_PORT_TOKEN=<chrome's port-token cookie>
#
# Usage:
#   scripts/bridge-up.sh up                       # spawn container, install toolchain
#   scripts/bridge-up.sh test-live                # sync + build + run live tests
#   scripts/bridge-up.sh test-live r_send         # filter to a substring
#   scripts/bridge-up.sh test-destructive         # down/up between each (slow but reliable)
#   scripts/bridge-up.sh test-destructive jsonlite # only the jsonlite scenario
#   scripts/bridge-up.sh test-all                 # fmt + clippy + unit + non-live + live + destructive
#   scripts/bridge-up.sh sync                     # re-copy sources after local edits
#   scripts/bridge-up.sh refresh                  # re-sync clientId/port-token
#   scripts/bridge-up.sh down                     # stop container (cargo cache kept)
#
# `test` is kept as an alias for `test-live` for backwards compatibility.

set -euo pipefail

ACTION="${1:-up}"
shift || true

CONTAINER=rstudio-bridge
BRIDGE_ENV=/tmp/rstudio-bridge-state.env
CHROME_DEBUG_PORT=9222
RSTUDIO_HTTP_PORT=18787
IMAGE=rocker/rstudio:4.5.2
# Named volume for cargo's registry/cache so consecutive `test` runs are fast.
CARGO_CACHE_VOL=rstudio-bridge-cargo

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

log() { echo "[bridge] $*" >&2; }

# Copy the repo sources into the container. Skips target/ and other
# build artefacts so the cargo cache in the named volume is the source
# of truth for compiled state. Uses `docker cp` for runtime independence
# (no reliance on colima/orbstack/Docker Desktop bind-mount semantics).
sync_sources() {
  log "syncing sources into container"
  docker exec "$CONTAINER" bash -c \
    'rm -rf /home/rstudio/rstudio-cli && mkdir -p /home/rstudio/rstudio-cli && chown rstudio:rstudio /home/rstudio/rstudio-cli'
  # Pipe a tar archive of the sources into a tar-extract running inside the
  # container. We pick the right "strip macOS metadata" flag based on the
  # host's tar dialect:
  #   - macOS bsdtar:  --no-mac-metadata
  #   - GNU tar (Linux runners): --no-xattrs (and macOS xattrs are absent
  #     anyway, so the flag is just defensive)
  # Without the strip, macOS bsdtar bakes xattrs (com.apple.provenance, …)
  # into the archive and the Linux extractor inside the container would
  # otherwise refuse them on lsetxattr.
  local tar_strip_xattrs=()
  if tar --help 2>&1 | grep -q -- '--no-mac-metadata'; then
    tar_strip_xattrs=(--no-mac-metadata)
  elif tar --help 2>&1 | grep -q -- '--no-xattrs'; then
    tar_strip_xattrs=(--no-xattrs)
  fi
  COPYFILE_DISABLE=1 tar "${tar_strip_xattrs[@]}" \
    --exclude='./target' --exclude='./.git' \
    --exclude='./.jj' --exclude='./node_modules' \
    -cf - -C "$REPO_ROOT" . \
    | docker exec -i "$CONTAINER" tar -xf - -C /home/rstudio/rstudio-cli 2>&1 \
    | grep -v 'LIBARCHIVE.xattr' || true
  docker exec "$CONTAINER" chown -R rstudio:rstudio /home/rstudio/rstudio-cli
}

# Run a command inside the container as the `rstudio` user with the
# cargo/rustup PATH pre-baked. CARGO_TARGET_DIR points outside the
# source tree so incremental builds land in the cache volume.
in_container() {
  docker exec \
    -u rstudio \
    -e USER=rstudio \
    -e HOME=/home/rstudio \
    -e CARGO_HOME=/home/rstudio/.cargo \
    -e RUSTUP_HOME=/home/rstudio/.cargo/rustup \
    -e CARGO_TARGET_DIR=/home/rstudio/.cargo/target \
    -e PATH=/home/rstudio/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    "$@"
}

# Read Chrome's clientId from the session-persistent-state file (written
# by rsession when it accepts the client_init RPC) and the port-token
# cookie via Chromium's DevTools Protocol over the in-container debug port.
# Both queries happen entirely inside the container, no host Python.
refresh_creds() {
  local sess
  sess=$(docker exec "$CONTAINER" bash -c \
    'ls /home/rstudio/.local/share/rstudio/sessions/active/ 2>/dev/null | head -1' \
    | sed 's/^session-//')
  [[ -z "$sess" ]] && { log "no session found"; return 1; }

  local cid
  cid=$(docker exec "$CONTAINER" bash -c \
    "awk -F'\"' '/active-client-id/ {print \$2}' /home/rstudio/.local/share/rstudio/sessions/active/session-$sess/session-persistent-state 2>/dev/null")

  # Use Python from inside the container (rocker/rstudio:4.5.2 ships
  # python3 by default). We talk to Chromium's DevTools Protocol on the
  # in-container debug port: list pages, pick the rstudio one (matched on
  # the in-container rserver port 8787, not the host-side mapped port),
  # open its webSocketDebuggerUrl, ask Network.getAllCookies, extract
  # port-token.
  local pt
  pt=$(docker exec -i "$CONTAINER" python3 - "$CHROME_DEBUG_PORT" <<'PY'
import json, sys, urllib.request, socket

debug_port = sys.argv[1]
pages = json.loads(urllib.request.urlopen(f"http://127.0.0.1:{debug_port}/json").read())
ws_url = next(
    (p["webSocketDebuggerUrl"] for p in pages
     if p["type"] == "page" and ":8787" in p.get("url", "")),
    None,
)
if not ws_url:
    print("", end=""); sys.exit(0)

# Minimal websocket client: opening handshake + one text frame request,
# parse one text frame response. Avoids a `websockets` dependency.
from urllib.parse import urlparse
import base64, hashlib, os, struct

u = urlparse(ws_url)
sock = socket.create_connection((u.hostname, u.port))
key = base64.b64encode(os.urandom(16)).decode()
req = (
    f"GET {u.path} HTTP/1.1\r\n"
    f"Host: {u.hostname}:{u.port}\r\n"
    f"Upgrade: websocket\r\n"
    f"Connection: Upgrade\r\n"
    f"Sec-WebSocket-Key: {key}\r\n"
    f"Sec-WebSocket-Version: 13\r\n\r\n"
).encode()
sock.sendall(req)
# Drain headers
buf = b""
while b"\r\n\r\n" not in buf:
    buf += sock.recv(4096)

def send_text(s, payload):
    data = payload.encode()
    mask = os.urandom(4)
    masked = bytes(b ^ mask[i % 4] for i, b in enumerate(data))
    header = bytes([0x81])
    n = len(data)
    if n < 126:
        header += bytes([0x80 | n])
    elif n < 65536:
        header += bytes([0x80 | 126]) + struct.pack(">H", n)
    else:
        header += bytes([0x80 | 127]) + struct.pack(">Q", n)
    s.sendall(header + mask + masked)

def recv_text(s):
    h = s.recv(2)
    n = h[1] & 0x7F
    if n == 126:
        n = struct.unpack(">H", s.recv(2))[0]
    elif n == 127:
        n = struct.unpack(">Q", s.recv(8))[0]
    data = b""
    while len(data) < n:
        chunk = s.recv(n - len(data))
        if not chunk: break
        data += chunk
    return data.decode()

send_text(sock, json.dumps({"id": 1, "method": "Network.getAllCookies"}))
resp = json.loads(recv_text(sock))
for c in resp.get("result", {}).get("cookies", []):
    if c["name"] == "port-token":
        print(c["value"], end=""); break
sock.close()
PY
)

  cat > "$BRIDGE_ENV" <<EOF
export RSTUDIO_CLI_CLIENT_ID=$cid
export RSTUDIO_CLI_PORT_TOKEN=$pt
EOF
  log "creds refreshed: cid=$cid pt=$pt session=$sess"
}

cmd_up() {
  log "starting bridge"

  # Cleanup any previous state
  docker stop "$CONTAINER" 2>/dev/null || true
  sleep 1

  log "spawning $IMAGE"
  docker run -d --rm --name "$CONTAINER" \
    -e DISABLE_AUTH=true \
    -p "127.0.0.1:$RSTUDIO_HTTP_PORT:8787" \
    -v "$CARGO_CACHE_VOL:/home/rstudio/.cargo" \
    "$IMAGE" >/dev/null
  sleep 5

  log "installing build deps + Chromium + R packages in container"
  # build-essential / pkg-config / libssl-dev for cargo.
  # Chromium for the in-container headless GWT client.
  # Ubuntu's chromium package is a snap wrapper that doesn't work in Docker;
  # pull the Debian-bookworm chromium .deb instead — it's a real binary.
  # `--allow-unauthenticated` is safe here because we trust the docker-cached
  # base image and the package is only used for headless test traffic.
  # python3 ships in the base image; we use it for the DevTools dance.
  docker exec "$CONTAINER" bash -c '
    apt-get update -qq
    apt-get install -y -qq curl build-essential pkg-config libssl-dev
    echo "deb [trusted=yes] http://deb.debian.org/debian bookworm main" \
      > /etc/apt/sources.list.d/debian-bookworm.list
    apt-get update -qq -o Acquire::AllowInsecureRepositories=true 2>&1 \
      | grep -v "^W:" || true
    apt-get install -y -qq --allow-unauthenticated chromium 2>&1 \
      | grep -v "policy-rc.d\|invoke-rc.d\|polkitd\|Processing triggers" || true
  ' >/dev/null
  # Pre-install rstudiocli dependencies (rstudioapi, jsonlite, callr) so
  # the CLI's auto-install of rstudiocli itself can succeed without
  # needing network from rsession. rstudiocli itself is auto-installed
  # by the CLI on first RPC (no pre-install here — that lets us catch any
  # auto-install regressions in CI).
  docker exec "$CONTAINER" R -e \
    'install.packages(c("rstudioapi","jsonlite","callr"), repos="https://cloud.r-project.org", quiet=TRUE)' \
    >/dev/null 2>&1

  # Destructive-test hook: if $BRIDGE_UNINSTALL_PKG is set, remove that
  # package now — BEFORE Chromium starts and triggers rsession spawn.
  # This is what gives the pre-check probe a real missing-dep to see;
  # uninstalling after rsession is running has no effect because rsession
  # keeps namespaces loaded in memory regardless of disk state.
  if [[ -n "${BRIDGE_UNINSTALL_PKG:-}" ]]; then
    log "uninstalling '${BRIDGE_UNINSTALL_PKG}' (destructive-test hook)"
    docker exec "$CONTAINER" R --vanilla --quiet -e \
      "remove.packages('${BRIDGE_UNINSTALL_PKG}')" \
      >/dev/null 2>&1
  fi
  # Make the cache volume writable by the rstudio user (named volumes are
  # owned by root by default).
  docker exec "$CONTAINER" chown -R rstudio:rstudio /home/rstudio/.cargo

  log "installing Rust toolchain (cached in $CARGO_CACHE_VOL volume)"
  # Profile `default` (vs `minimal`) ships rustfmt + clippy so `test-all`
  # can run the same gauntlet as the local CI workflow.
  docker exec \
    -u rstudio \
    -e HOME=/home/rstudio \
    -e CARGO_HOME=/home/rstudio/.cargo \
    -e RUSTUP_HOME=/home/rstudio/.cargo/rustup \
    "$CONTAINER" bash -c '
      set -e
      if [ ! -x "$HOME/.cargo/bin/cargo" ]; then
        curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs \
          | sh -s -- -y --default-toolchain stable --profile default --no-modify-path >/dev/null
      else
        # Existing install (cache hit): make sure rustfmt + clippy are
        # present in case the volume was built with an older `minimal`
        # profile install.
        "$HOME/.cargo/bin/rustup" component add rustfmt clippy >/dev/null 2>&1 || true
      fi
    '

  log "launching headless Chromium in container (debug port $CHROME_DEBUG_PORT)"
  # --no-sandbox: container has no user namespace setup
  # --headless=new: modern headless mode (Chrome 109+)
  # --user-data-dir under /tmp so it does not persist across container restarts
  # We background the process; its lifetime is tied to the container's.
  docker exec -u rstudio -d "$CONTAINER" bash -c "
    rm -rf /tmp/chrome-rs && mkdir -p /tmp/chrome-rs
    /usr/bin/chromium --headless=new --disable-gpu --no-sandbox \\
             --user-data-dir=/tmp/chrome-rs \\
             --remote-debugging-port=$CHROME_DEBUG_PORT \\
             --remote-debugging-address=127.0.0.1 \\
             http://127.0.0.1:8787/ >/tmp/chrome.log 2>&1
  "

  log "waiting for rsession socket"
  local i
  for i in $(seq 1 30); do
    if docker exec "$CONTAINER" test -S /var/run/rstudio-server/rstudio-rsession/rstudio-d 2>/dev/null; then
      log "rsession ready after ${i}s"
      break
    fi
    sleep 1
  done
  # Give Chromium time to perform the initial /client_init handshake and
  # for rsession to settle. A short fixed wait here; cmd_test_* below does
  # a follow-up active probe (which can only run after the binary is
  # built) for the rare case the runner is exceptionally slow.
  sleep 5

  refresh_creds
  log "bridge up; run scripts/bridge-up.sh test"
}

# Active rsession warmup: pings rsession via the just-built CLI binary
# until it answers cleanly (no asyncHandle). The first few pings after
# `up` typically return asyncHandle because rsession is still processing
# Chromium's /client_init handshake; once it settles, every subsequent
# RPC succeeds.
#
# The probe needs the CLI binary at target/debug/rstudio. If it's not
# there yet, we build it explicitly so the probe has something to talk
# with (cargo test --no-run only builds test binaries, not the main bin).
warmup_rsession() {
  in_container "$CONTAINER" bash -c \
    'cd /home/rstudio/rstudio-cli && \
     [ -x /home/rstudio/.cargo/target/debug/rstudio ] || cargo build 2>&1 | tail -3' \
    >/dev/null 2>&1 || true

  log "warming up rsession (active probe)"
  local i
  for i in $(seq 1 30); do
    if in_container \
         -e RSTUDIO_CLI_CLIENT_ID="$RSTUDIO_CLI_CLIENT_ID" \
         -e RSTUDIO_CLI_PORT_TOKEN="$RSTUDIO_CLI_PORT_TOKEN" \
         "$CONTAINER" bash -c \
         'cd /home/rstudio/rstudio-cli && /home/rstudio/.cargo/target/debug/rstudio r exec "Sys.getpid()" 2>&1 | grep -q "\"ok\":true"' \
         >/dev/null 2>&1; then
      log "rsession warm after ${i}s"
      return 0
    fi
    sleep 1
  done
  log "warning: rsession did not warm up in 30 s; tests may flake"
}

cmd_refresh() {
  refresh_creds
}

cmd_sync() {
  sync_sources
}

cmd_test_live() {
  local filter="${1:-}"
  # shellcheck disable=SC1090
  source "$BRIDGE_ENV"

  sync_sources

  log "building live tests in container"
  in_container "$CONTAINER" bash -c \
    'cd /home/rstudio/rstudio-cli && cargo test --test live --no-run 2>&1' \
    | tail -3

  # The CLI binary is also built by --no-run as a dev-dep of the live
  # test binary, so we can use it for the active warmup probe now.
  warmup_rsession

  log "running live tests${filter:+ (filter: $filter)}"
  in_container \
    -e RSTUDIO_CLI_CLIENT_ID="$RSTUDIO_CLI_CLIENT_ID" \
    -e RSTUDIO_CLI_PORT_TOKEN="$RSTUDIO_CLI_PORT_TOKEN" \
    "$CONTAINER" bash -c \
    "cd /home/rstudio/rstudio-cli && cargo test --test live $filter -- --ignored --test-threads=1"
}

# Run the destructive test binary. Each test needs a freshly-spawned
# container with exactly one CRAN package uninstalled before the test
# starts. Respawning rsession from inside the test process is brittle
# (races against rserver's client_init handshake), so we use the
# crowbar approach: down + up between tests. Slower (~30 s / test) but
# rock-solid.
#
# Usage:
#   scripts/bridge-up.sh test-destructive          # both tests
#   scripts/bridge-up.sh test-destructive jsonlite # only jsonlite
cmd_test_destructive() {
  local filter="${1:-}"
  local packages=("jsonlite" "rstudioapi")
  if [[ -n "$filter" ]]; then
    packages=("$filter")
  fi

  for pkg in "${packages[@]}"; do
    log "=== destructive test: '${pkg}' missing ==="

    # Fresh container so no state leaks between iterations. We pass
    # BRIDGE_UNINSTALL_PKG to `cmd_up`, which removes the package
    # before Chromium starts — otherwise rsession would load the
    # namespace in memory before the test could observe its absence.
    cmd_down >/dev/null 2>&1 || true
    BRIDGE_UNINSTALL_PKG="$pkg" cmd_up

    # shellcheck disable=SC1090
    source "$BRIDGE_ENV"

    sync_sources

    log "building destructive test binary in container"
    in_container "$CONTAINER" bash -c \
      'cd /home/rstudio/rstudio-cli && cargo test --test destructive --no-run 2>&1' \
      | tail -3

    # We deliberately skip warmup_rsession here: it probes via the CLI,
    # which would itself trip the pre-check (the whole point of the
    # test) and report failure. The test uses RpcClient directly,
    # bypassing ensure_installed, so a not-yet-warm rsession is fine —
    # check_dependencies retries internally if the first call gets an
    # async handle.

    log "running destructive test for '${pkg}'"
    local test_name="destructive_precheck_reports_missing_${pkg}"
    in_container \
      -e RSTUDIO_CLI_CLIENT_ID="$RSTUDIO_CLI_CLIENT_ID" \
      -e RSTUDIO_CLI_PORT_TOKEN="$RSTUDIO_CLI_PORT_TOKEN" \
      -e RSTUDIO_CLI_DESTRUCTIVE_TESTS=1 \
      "$CONTAINER" bash -c \
      "cd /home/rstudio/rstudio-cli && cargo test --test destructive ${test_name} -- --ignored --exact --test-threads=1"
  done

  # Leave the container in a clean (i.e. down) state — the last
  # iteration's container has at least one CRAN package missing, which
  # would break a subsequent test-live invocation. The user can `up`
  # again explicitly if they want to keep working.
  log "=== destructive suite complete (tearing down) ==="
  cmd_down >/dev/null 2>&1 || true
}

# Run the *full* test suite inside the container: cargo fmt --check,
# cargo clippy, cargo test (unit + non-live integration), then the live
# tests on top. Matches what the local preflight gauntlet in CLAUDE.md
# enforces before a release tag.
cmd_test_all() {
  # shellcheck disable=SC1090
  source "$BRIDGE_ENV"

  sync_sources

  log "cargo fmt --check"
  in_container "$CONTAINER" bash -c \
    'cd /home/rstudio/rstudio-cli && cargo fmt --check'

  log "cargo clippy --all-targets -- -D warnings"
  in_container "$CONTAINER" bash -c \
    'cd /home/rstudio/rstudio-cli && cargo clippy --all-targets -- -D warnings 2>&1' \
    | tail -5

  log "cargo test (unit + non-live integration)"
  # --tests runs every integration binary including `live` — but its tests
  # are #[ignore]d by default so they don't trigger without --ignored.
  # We filter the output to surface the per-binary summary lines, then run
  # `live` separately below with --ignored.
  in_container "$CONTAINER" bash -c \
    'cd /home/rstudio/rstudio-cli && cargo test --lib --tests 2>&1' \
    | grep -E "^test result:|^     Running"

  # The CLI binary is now built (as a dev-dep of the test binaries),
  # so the active probe can run.
  warmup_rsession

  log "cargo test --test live -- --ignored"
  in_container \
    -e RSTUDIO_CLI_CLIENT_ID="$RSTUDIO_CLI_CLIENT_ID" \
    -e RSTUDIO_CLI_PORT_TOKEN="$RSTUDIO_CLI_PORT_TOKEN" \
    "$CONTAINER" bash -c \
    'cd /home/rstudio/rstudio-cli && cargo test --test live -- --ignored --test-threads=1'

  # Destructive tests rebuild the container between iterations, so
  # they run last and re-orchestrate everything themselves.
  cmd_test_destructive
}

cmd_down() {
  log "tearing down bridge"
  docker stop "$CONTAINER" 2>/dev/null || true
  rm -f "$BRIDGE_ENV"
  log "down (cargo cache volume $CARGO_CACHE_VOL preserved; rm with: docker volume rm $CARGO_CACHE_VOL)"
}

case "$ACTION" in
  up) cmd_up ;;
  refresh) cmd_refresh ;;
  sync) cmd_sync ;;
  test|test-live) cmd_test_live "$@" ;;
  test-destructive) cmd_test_destructive "$@" ;;
  test-all) cmd_test_all ;;
  down) cmd_down ;;
  *)
    echo "usage: $0 [up|refresh|sync|test-live [filter]|test-destructive [pkg]|test-all|down]" >&2
    exit 1
    ;;
esac
