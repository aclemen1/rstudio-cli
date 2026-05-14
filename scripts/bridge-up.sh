#!/usr/bin/env bash
# Spawn a Dockerised RStudio Server, bridge it to a local Unix socket so the
# rstudio CLI can drive it as if it were a Desktop session. Used by the live
# integration tests in `tests/live.rs`.
#
# Requires: docker (with OrbStack on macOS for socket forwarding), `socat`,
# `curl`, Google Chrome (for IDE event loop), Python with the `websockets`
# package (via `uv`).
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

set -euo pipefail

ACTION="${1:-up}"
SHARED=/tmp/rstudio-orbstack-shared
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
export USER=rstudio
EOF
  log "creds refreshed: cid=$cid pt=$pt session=$sess"
}

cmd_up() {
  log "starting bridge"

  # Cleanup any previous state
  docker stop rs-orb 2>/dev/null || true
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
  docker run -d --rm --name rs-orb \
    -e DISABLE_AUTH=true \
    -p "127.0.0.1:$RSTUDIO_HTTP_PORT:8787" \
    -p "127.0.0.1:$RSTUDIO_TCP_PORT:$RSTUDIO_TCP_PORT" \
    -v "$SHARED/home:/home/rstudio" \
    -v "$SHARED/tmp:/shared-tmp" \
    "$IMAGE" >/dev/null
  sleep 5

  log "installing socat + R packages in container"
  docker exec rs-orb bash -c 'apt-get update -qq && apt-get install -y -qq socat curl' >/dev/null 2>&1
  docker exec rs-orb R -e 'install.packages(c("rstudioapi","jsonlite"), repos="https://cloud.r-project.org", quiet=TRUE)' >/dev/null 2>&1

  log "building + installing rstudiocli.mcp"
  R CMD build r-package >/dev/null 2>&1
  mv rstudiocli.mcp_*.tar.gz "$SHARED/tmp/pkg.tar.gz"
  docker exec rs-orb R -e 'install.packages("/shared-tmp/pkg.tar.gz", repos=NULL, type="source", quiet=TRUE)' >/dev/null 2>&1

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
    if docker exec rs-orb test -S /var/run/rstudio-server/rstudio-rsession/rstudio-d 2>/dev/null; then
      log "rsession ready after ${i}s"
      break
    fi
    sleep 1
  done
  sleep 5  # let GWT finish init

  log "starting socat (container side)"
  docker exec -d rs-orb sh -c \
    "sudo -u rstudio socat TCP-LISTEN:$RSTUDIO_TCP_PORT,bind=0.0.0.0,reuseaddr,fork UNIX-CONNECT:/var/run/rstudio-server/rstudio-rsession/rstudio-d"
  sleep 2

  log "starting socat (host side)"
  rm -f "$BRIDGE_SOCK"
  socat "UNIX-LISTEN:$BRIDGE_SOCK,fork,reuseaddr,unlink-early" \
    "TCP:127.0.0.1:$RSTUDIO_TCP_PORT" >/tmp/host-socat.log 2>&1 &
  sleep 2

  # Symlink container's session state into the host's ~/.local/share/rstudio
  local sess
  sess=$(ls "$SHARED/home/.local/share/rstudio/sessions/active/" | head -1 | sed 's/^session-//')
  mkdir -p ~/.local/share/rstudio/sessions/active
  rm -rf "$HOME/.local/share/rstudio/sessions/active/session-$sess" 2>/dev/null
  ln -sfn "$SHARED/home/.local/share/rstudio/sessions/active/session-$sess" \
    "$HOME/.local/share/rstudio/sessions/active/session-$sess"

  refresh_creds
  log "bridge up; source $BRIDGE_ENV to use it"
}

cmd_refresh() {
  refresh_creds
}

cmd_down() {
  log "tearing down bridge"
  docker stop rs-orb 2>/dev/null || true
  pkill -f "$BRIDGE_SOCK" 2>/dev/null || true
  pkill -f "remote-debugging-port=$CHROME_DEBUG_PORT" 2>/dev/null || true
  rm -f "$BRIDGE_SOCK" "$BRIDGE_ENV"
  rm -rf "$SHARED"
  log "down"
}

case "$ACTION" in
  up) cmd_up ;;
  refresh) cmd_refresh ;;
  down) cmd_down ;;
  *) echo "usage: $0 [up|refresh|down]" >&2; exit 1 ;;
esac
