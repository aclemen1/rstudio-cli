"""Bench the progressive-discovery token gain for `rstudio mcp`.

Measures the cost (in cl100k_base tokens) of:

1. `tools/list` core surface — what every connected agent pays on every turn.
2. The drill-down rounds an agent walks through to find a specific tool:
   - level 0: `tools_search({})` — categories only.
   - level 1: `tools_search({category: "editor"})` — actions in one category.
   - level 2: `tools_search({category, action})` — full ActionSpec + MCP
     input_schema for one action.
3. The hypothetical baseline: what `tools/list` would have cost if every
   registry-derived action were listed up front (the pre-progressive-discovery
   behaviour).

The full discovery flow (1 + level 0 + level 1 + level 2) is what an agent
actually pays end-to-end to find and call one tool. Compare that to the
baseline to see the real savings.

Run:

    uv run --with tiktoken scripts/bench_discovery.py
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

import tiktoken

REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_BIN = REPO_ROOT / "target" / "debug" / "rstudio"


def jsonrpc(method: str, params: dict | None = None, msg_id: int = 1) -> str:
    msg: dict = {"jsonrpc": "2.0", "id": msg_id, "method": method}
    if params is not None:
        msg["params"] = params
    return json.dumps(msg) + "\n"


def query_server(bin_path: Path) -> dict[int, dict]:
    """Spawn the MCP server, send a fixed bench script, and return responses
    keyed by JSON-RPC id."""
    env = os.environ.copy()
    env.setdefault("RSTUDIO_SESSION_ID", "bench")

    proc = subprocess.Popen(
        [str(bin_path), "mcp"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=env,
        cwd=REPO_ROOT,
    )
    assert proc.stdin is not None and proc.stdout is not None

    script = (
        jsonrpc(
            "initialize",
            {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "bench", "version": "0"},
            },
            msg_id=1,
        )
        + jsonrpc("tools/list", {}, msg_id=2)
        # Level 0: catalog (categories only).
        + jsonrpc(
            "tools/call",
            {"name": "tools_search", "arguments": {}},
            msg_id=3,
        )
        # Level 1: actions of one category.
        + jsonrpc(
            "tools/call",
            {"name": "tools_search", "arguments": {"category": "editor"}},
            msg_id=4,
        )
        # Level 2: full ActionSpec + MCP input_schema for one action.
        + jsonrpc(
            "tools/call",
            {
                "name": "tools_search",
                "arguments": {"category": "editor", "action": "read-buffer"},
            },
            msg_id=5,
        )
        # Baseline: simulate the pre-progressive-discovery tools/list by
        # asking for every action across every category. We do this with a
        # regex that matches everything.
        + jsonrpc(
            "tools/call",
            {"name": "tools_search", "arguments": {"search": ".*"}},
            msg_id=6,
        )
    )

    try:
        out, err = proc.communicate(input=script, timeout=15)
    except subprocess.TimeoutExpired:
        proc.kill()
        out, err = proc.communicate()
        print(f"server timed out; stderr:\n{err}", file=sys.stderr)
        raise

    if proc.returncode != 0:
        print(f"server exited {proc.returncode}; stderr:\n{err}", file=sys.stderr)

    responses: dict[int, dict] = {}
    for line in out.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
        except json.JSONDecodeError:
            continue
        if "id" in obj:
            responses[obj["id"]] = obj
    return responses


def count_tokens(payload: dict | list, enc: tiktoken.Encoding) -> int:
    return len(enc.encode(json.dumps(payload, separators=(",", ":"))))


def inner(resp: dict) -> dict:
    """tools/call wraps the tool's JSON result inside content[0].text."""
    text = resp["result"]["content"][0]["text"]
    return json.loads(text)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--bin", type=Path, default=DEFAULT_BIN)
    args = ap.parse_args()

    if not args.bin.exists():
        print(f"error: binary not found at {args.bin}", file=sys.stderr)
        return 2

    enc = tiktoken.get_encoding("cl100k_base")
    resp = query_server(args.bin)
    missing = [i for i in (2, 3, 4, 5, 6) if i not in resp]
    if missing:
        print(f"error: missing responses for ids {missing}", file=sys.stderr)
        return 1

    # tools/list result
    list_result = resp[2]["result"]
    list_tokens = count_tokens(list_result, enc)
    list_count = len(list_result["tools"])

    # Drill-down levels
    level0 = inner(resp[3])
    level0_tokens = count_tokens(level0, enc)
    level0_cats = len(level0["categories"])

    level1 = inner(resp[4])
    level1_tokens = count_tokens(level1, enc)
    level1_actions = len(level1["actions"])

    level2 = inner(resp[5])
    level2_tokens = count_tokens(level2, enc)

    # Baseline: every action listed (what tools/list looked like before).
    baseline = inner(resp[6])
    baseline_actions = baseline["actions"]
    baseline_tokens = count_tokens({"tools": baseline_actions}, enc)

    discovery_flow = level0_tokens + level1_tokens + level2_tokens
    full_first_turn_old = list_tokens + baseline_tokens  # hypothetical "all in tools/list"
    full_first_turn_new = list_tokens  # only the core, every other turn pays just this

    print("rstudio-cli MCP — progressive discovery token bench")
    print("=" * 64)
    print(f"binary:    {args.bin}")
    print(f"tokenizer: cl100k_base (tiktoken)")
    print()
    print(f"tools/list (core surface): {list_count:3d} tools, {list_tokens:6d} tokens")
    print()
    print("Drill-down rounds (only paid when an agent searches):")
    print(f"  level 0  ({level0_cats:2d} categories):                       {level0_tokens:5d} tokens")
    print(f"  level 1  ({level1_actions:2d} actions in 'editor'):             {level1_tokens:5d} tokens")
    print(f"  level 2  (full ActionSpec + input_schema):       {level2_tokens:5d} tokens")
    print(f"  full 3-round discovery flow:                     {discovery_flow:5d} tokens")
    print()
    print("Baseline comparison:")
    print(f"  tools/list (pre-progressive, every tool listed):  {baseline_tokens:5d} tokens "
          f"(over {len(baseline_actions)} actions)")
    print()
    print("Net savings on every-turn fixed cost:")
    saved_fixed = baseline_tokens - list_tokens
    pct = saved_fixed / baseline_tokens * 100
    print(f"  per-turn:  {list_tokens:5d} new vs {baseline_tokens:5d} old "
          f"= -{saved_fixed} tokens ({pct:.1f}% reduction)")
    print()
    print("First-turn cost for an agent that DOES need to discover one tool:")
    paid_to_discover = list_tokens + discovery_flow
    pct2 = (1 - paid_to_discover / full_first_turn_old) * 100
    print(f"  new (list + 3-round drill-down): {paid_to_discover:5d} tokens")
    print(f"  old (full list, no drill-down):  {full_first_turn_old:5d} tokens")
    print(f"  delta: {full_first_turn_old - paid_to_discover:+d} tokens ({pct2:+.1f}%)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
