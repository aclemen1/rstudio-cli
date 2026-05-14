#!/usr/bin/env bash
# Spawn a Dockerised RStudio Server, bridge it to a local Unix socket so the
# rstudio CLI can drive it as if it were a Desktop session. Used by the live
# integration tests in `tests/live.rs`.
#
# Requires:
#   - A Docker runtime that supports bidirectional bind-mounts from the
#     macOS host. Tested working:
#       - OrbStack: works out of the box (virtiofs by default, no setup).
#       - colima:   start with `--mount-type virtiofs --vm-type vz` and
#                   an explicit `--mount /tmp/rstudio-bridge:w` so the
#                   host-side shared dir (kept under `/tmp` to avoid
#                   polluting `$HOME`) is reachable from inside the VM:
#                       colima start --mount-type virtiofs --vm-type vz \\
#                                    --mount /tmp/rstudio-bridge:w
#                   (colima only mounts the paths you ask for; `/tmp`
#                   is *not* mounted by default. Also: colima/lima
#                   reserve the *guest* `/tmp`, which is why the
#                   in-container mount point is `/shared-tmp`.)
#     Known not to work with default config:
#       - colima with default sshfs mount (one-way only: container
#         writes do not propagate to the host).
#       - Docker Desktop on older versions (gRPC-FUSE may bottleneck).
#   - `socat`, `curl`, Google Chrome.app (for IDE event loop), Python with
#     the `websockets` package (auto-installed via `uv`).
#
# Writes bridge state to /tmp/rstudio-bridge-state.env, sourced by the test
# harness:
#
#   export RSTUDIO_SESSION_STREAM=/tmp/rstudio-bridge.sock
#   export RSTUDIO_SESSION_ID=<container-side session id>
#   export RSTUDIO_CLI_CLIENT_ID=<chrome's clientId>
#   export RSTUDIO_CLI_PORT_TOKEN=<chrome's port-token cookie>
#   export RSTUDIO_CLI_BRIDGE_TARBALL_DIR=<host shared dir>
#   export RSTUDIO_CLI_BRIDGE_TARBALL_RPATH_DIR=<container path>
#   export USER=rstudio
#
# Usage:
#   scripts/bridge-up.sh        # spawn fresh
#   scripts/bridge-up.sh refresh # re-sync client_id/port-token with current Chrome state
#   scripts/bridge-up.sh down   # tear down everything

set -euo pipefail

ACTION="${1:-up}"
# Host-side shared dir for the bridge. Lives under /tmp on purpose:
# avoids polluting $HOME, and host /tmp is mounted rw by both OrbStack
# (everything by default) and colima (with --mount-type virtiofs --vm-type vz).
# The *container-side* mount point is /shared-tmp because colima/lima
# reserve the guest /tmp.
SHARED=/tmp/rstudio-bridge/shared
BRIDGE_SOCK=/tmp/rstudio-bridge.sock
BRIDGE_ENV=/tmp/rstudio-bridge-state.env
CHROME_DEBUG_PORT=9222
RSTUDIO_HTTP_PORT=18787
RSTUDIO_TCP_PORT=19999
IMAGE=rocker/rstudio:4.5.2
CHROME_APP='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

log() { echo "[bridge] $*" >&2; }

refresh_creds() {
  # Read Chrome's current client_id (via /session-persistent-state file)
  # and port-token cookie (via Chrome DevTools Protocol).
  local sess
  sess=$(ls "$SHARED/home/.local/share/rstudio/sessions/active/" 2>/dev/null | head -1 | sed 's/^session-//')
  [[ -z "$sess" ]] && { log "no session found"; return 1; }

  local cid
  cid=$(awk -F'"' '/active-client-id/ {print $2}' \
    "$SHARED/home/.local/share/rstudio/sessions/active/session-$sess/session-persistent-state" 2>/dev/null)

  local pt
  pt=$(curl -s "http://127.0.0.1:$CHROME_DEBUG_PORT/json" \
    | python3 -c "
import json, sys
pages = json.load(sys.stdin)
for p in pages:
    if p['type'] == 'page' and '$RSTUDIO_HTTP_PORT' in p.get('url', ''):
        print(p['webSocketDebuggerUrl']); break
" \
    | { read -r ws_url; cd /tmp && uv run --quiet --with websockets python -c "
import json, asyncio, sys, websockets

async def get_port_token(url):
    async with websockets.connect(url, max_size=None) as ws:
        await ws.send(json.dumps({'id': 1, 'method': 'Network.getAllCookies'}))
        r = json.loads(await ws.recv())
        for c in r.get('result', {}).get('cookies', []):
            if c['name'] == 'port-token':
                return c['value']
        return ''

print(asyncio.run(get_port_token('$ws_url'))) " 2>/dev/null
    }
  )

  cat > "$BRIDGE_ENV" <<EOF
export RSTUDIO_SESSION_STREAM=$BRIDGE_SOCK
export RSTUDIO_SESSION_ID=$sess
export RSTUDIO_CLI_CLIENT_ID=$cid
export RSTUDIO_CLI_PORT_TOKEN=$pt
export RSTUDIO_CLI_BRIDGE_TARBALL_DIR=$SHARED/tmp
export RSTUDIO_CLI_BRIDGE_TARBALL_RPATH_DIR=/shared-tmp
# r send writes its capture result via R to one filesystem and reads it
# from the CLI on another. Bridge: same bind-mount as the tarball.
export RSTUDIO_CLI_BRIDGE_CAPTURE_DIR=$SHARED/tmp
export RSTUDIO_CLI_BRIDGE_CAPTURE_RPATH_DIR=/shared-tmp
# Bridge installs rstudiocli.mcp into the container's R library directly
# (the CLI's auto-install via execute_r_code RPC misbehaves through the
# container's HTTP proxy). Skip the runtime check.
export RSTUDIO_CLI_SKIP_ENSURE_INSTALL=1
# r send polls kill(pid, 0) to detect rsession crashes; in the bridge
# the PID lives in the container's PID namespace, invisible from macOS,
# which would false-positive.
export RSTUDIO_CLI_SKIP_PID_CHECK=1
# Rewrite host-canonicalised file paths into the path R sees inside the
# container. Same bind-mount as the capture dir (the only host path the
# CLI canonicalises that resolves on both sides of the bridge). The host
# prefix is the *canonicalised* path: on macOS, /tmp is a symlink to
# /private/tmp, so the CLI's std::fs::canonicalize emits /private/tmp/...
export RSTUDIO_CLI_PATH_REMAP=$(cd "$SHARED/tmp" && pwd -P):/shared-tmp
export USER=rstudio
EOF
  log "creds refreshed: cid=$cid pt=$pt session=$sess"
}

cmd_up() {
  log "starting bridge"

  # Cleanup any previous state
  docker stop rstudio-bridge 2>/dev/null || true
  pkill -f "$BRIDGE_SOCK" 2>/dev/null || true
  pkill -f "remote-debugging-port=$CHROME_DEBUG_PORT" 2>/dev/null || true
  sleep 1
  rm -rf "$SHARED"
  mkdir -p "$SHARED/home" "$SHARED/tmp"

  # Pre-create home with rstudio UID so rserver can write
  docker run --rm -v "$SHARED/home:/home/rstudio" alpine \
    sh -c "mkdir -p /home/rstudio/.local/share/rstudio/sources /home/rstudio/.local/share/rstudio/sessions/active && chown -R 1000:1000 /home/rstudio" \
    >/dev/null

  log "spawning $IMAGE"
  docker run -d --rm --name rstudio-bridge \
    -e DISABLE_AUTH=true \
    -p "127.0.0.1:$RSTUDIO_HTTP_PORT:8787" \
    -p "127.0.0.1:$RSTUDIO_TCP_PORT:$RSTUDIO_TCP_PORT" \
    -v "$SHARED/home:/home/rstudio" \
    -v "$SHARED/tmp:/shared-tmp" \
    "$IMAGE" >/dev/null
  sleep 5

  log "installing socat + R packages in container"
  docker exec rstudio-bridge bash -c 'apt-get update -qq && apt-get install -y -qq socat curl' >/dev/null 2>&1
  docker exec rstudio-bridge R -e 'install.packages(c("rstudioapi","jsonlite"), repos="https://cloud.r-project.org", quiet=TRUE)' >/dev/null 2>&1

  log "building + installing rstudiocli.mcp"
  R CMD build r-package >/dev/null 2>&1
  mv rstudiocli.mcp_*.tar.gz "$SHARED/tmp/pkg.tar.gz"
  docker exec rstudio-bridge R -e 'install.packages("/shared-tmp/pkg.tar.gz", repos=NULL, type="source", quiet=TRUE)' >/dev/null 2>&1

  log "launching headless Chrome to spawn rsession"
  local prof
  prof=$(mktemp -d /tmp/chrome-rs-XXXX)
  "$CHROME_APP" --headless=new --disable-gpu --no-sandbox \
    --user-data-dir="$prof" \
    --remote-debugging-port="$CHROME_DEBUG_PORT" \
    "http://127.0.0.1:$RSTUDIO_HTTP_PORT/" >/tmp/chrome.log 2>&1 &

  log "waiting for rsession socket"
  local i
  for i in $(seq 1 30); do
    if docker exec rstudio-bridge test -S /var/run/rstudio-server/rstudio-rsession/rstudio-d 2>/dev/null; then
      log "rsession ready after ${i}s"
      break
    fi
    sleep 1
  done
  sleep 5  # let GWT finish init

  log "starting socat (container side)"
  docker exec -d rstudio-bridge sh -c \
    "sudo -u rstudio socat TCP-LISTEN:$RSTUDIO_TCP_PORT,bind=0.0.0.0,reuseaddr,fork UNIX-CONNECT:/var/run/rstudio-server/rstudio-rsession/rstudio-d"
  sleep 2

  log "starting socat (host side)"
  rm -f "$BRIDGE_SOCK"
  socat "UNIX-LISTEN:$BRIDGE_SOCK,fork,reuseaddr,unlink-early" \
    "TCP:127.0.0.1:$RSTUDIO_TCP_PORT" >/tmp/host-socat.log 2>&1 &
  sleep 2

  # Symlink container's session state into the host's ~/.local/share/rstudio
  # so the CLI's state/sources lookups find the live container paths.
  local sess
  sess=$(ls "$SHARED/home/.local/share/rstudio/sessions/active/" | head -1 | sed 's/^session-//')
  mkdir -p ~/.local/share/rstudio/sessions/active ~/.local/share/rstudio/sources
  rm -rf "$HOME/.local/share/rstudio/sessions/active/session-$sess" 2>/dev/null
  ln -sfn "$SHARED/home/.local/share/rstudio/sessions/active/session-$sess" \
    "$HOME/.local/share/rstudio/sessions/active/session-$sess"
  # `editor list` and friends scan the per-session sources dir
  # to enumerate open documents.
  if [ -d "$SHARED/home/.local/share/rstudio/sources/session-$sess" ]; then
    rm -rf "$HOME/.local/share/rstudio/sources/session-$sess" 2>/dev/null
    ln -sfn "$SHARED/home/.local/share/rstudio/sources/session-$sess" \
      "$HOME/.local/share/rstudio/sources/session-$sess"
  fi

  refresh_creds
  log "bridge up; source $BRIDGE_ENV to use it"
}

cmd_refresh() {
  refresh_creds
}

cmd_down() {
  log "tearing down bridge"
  docker stop rstudio-bridge 2>/dev/null || true
  pkill -f "$BRIDGE_SOCK" 2>/dev/null || true
  pkill -f "remote-debugging-port=$CHROME_DEBUG_PORT" 2>/dev/null || true
  rm -f "$BRIDGE_SOCK" "$BRIDGE_ENV"
  # Wipe contents only — never the mount point itself. virtiofs in colima
  # caches the inode of /tmp/rstudio-bridge, and rm-then-recreate makes
  # the guest see an empty directory even when the host has populated it.
  rm -rf "$SHARED"
  log "down"
}

case "$ACTION" in
  up) cmd_up ;;
  refresh) cmd_refresh ;;
  down) cmd_down ;;
  *) echo "usage: $0 [up|refresh|down]" >&2; exit 1 ;;
esac
