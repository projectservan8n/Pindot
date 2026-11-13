#!/usr/bin/env python3
"""Call Deskhand's MCP server by hand, without a model (spike helper).

    python3 spikes/mcp-call.py screenshot '{}' --save /tmp/shot.jpg
    python3 spikes/mcp-call.py screenshot '{}' click '{"x": 233, "y": 220}'

Pairs of TOOL JSON_ARGS run in order in one MCP session, because `click`
uses the geometry of the latest `screenshot`. Reads the bearer token from the
Deskhand data dir and never prints it. Standard library only.
"""
import base64
import json
import os
import sys
import urllib.error
import urllib.request

PORT = int(os.environ.get("DESKHAND_PORT", "47823"))
URL = f"http://127.0.0.1:{PORT}/mcp"
TOKEN_FILE = os.path.expanduser("~/Library/Application Support/Deskhand/mcp-token")


def post(body, token, session=None):
    headers = {
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
        "Authorization": f"Bearer {token}",
        "User-Agent": "deskhand-spike-build",
    }
    if session:
        headers["Mcp-Session-Id"] = session
    req = urllib.request.Request(URL, data=json.dumps(body).encode(), headers=headers, method="POST")
    with urllib.request.urlopen(req, timeout=60) as r:
        sid = r.headers.get("Mcp-Session-Id")
        raw = r.read().decode()
    # Streamable HTTP answers with JSON or an SSE stream; take the JSON-RPC response.
    msgs = []
    for line in raw.splitlines():
        if line.startswith("data:") and line[5:].strip():
            msgs.append(json.loads(line[5:]))
    if not msgs and raw.strip():
        msgs.append(json.loads(raw))
    resp = next((m for m in msgs if "id" in m and m.get("id") == body.get("id")), None)
    return sid, resp


def main():
    args = sys.argv[1:]
    save = None
    if "--save" in args:
        i = args.index("--save")
        save = args[i + 1]
        del args[i : i + 2]
    if len(args) % 2:
        sys.exit(__doc__)
    token = open(TOKEN_FILE).read().strip()
    sid, init = post(
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "deskhand-spike-build", "version": "0"},
            },
        },
        token,
    )
    print("initialize:", init["result"]["serverInfo"])
    post({"jsonrpc": "2.0", "method": "notifications/initialized"}, token, sid)
    for n, (tool, js) in enumerate(zip(args[::2], args[1::2]), start=2):
        _, resp = post(
            {"jsonrpc": "2.0", "id": n, "method": "tools/call", "params": {"name": tool, "arguments": json.loads(js)}},
            token,
            sid,
        )
        if "error" in resp:
            print(tool, "ERROR", resp["error"])
            continue
        for block in resp["result"].get("content", []):
            if block["type"] == "image":
                data = base64.b64decode(block["data"])
                print(f"{tool}: image {block['mimeType']} {len(data)} bytes")
                if save:
                    open(save, "wb").write(data)
                    print(f"{tool}: saved to {save}")
            else:
                print(f"{tool}: {block.get('text')}")
        if resp["result"].get("isError"):
            print(tool, "isError: true")


if __name__ == "__main__":
    try:
        main()
    except urllib.error.HTTPError as e:
        sys.exit(f"HTTP {e.code}: {e.read()[:300]!r}")
