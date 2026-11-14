#!/usr/bin/env python3
"""Call Deskhand's MCP tools directly, for manual and scripted tests.

  mcp_call.py init                      start an MCP session (id kept in /tmp)
  mcp_call.py call TOOL '{"json":1}'    call a tool in that session
  mcp_call.py end                       end the session (releases the lock)
  mcp_call.py list                      list tool names

Reads the server URL from the claude-mcp.json Deskhand writes and the
bearer token from its data dir; never prints the token. Images in results
are saved to $MCP_IMG_DIR (default /tmp/deskhand-mcp-img) and their paths
printed.
"""
import base64, json, os, sys, time, urllib.request, urllib.error

DATA = os.path.expanduser("~/Library/Application Support/Deskhand")
SESSION_FILE = "/tmp/deskhand-mcp-session"
IMG_DIR = os.environ.get("MCP_IMG_DIR", "/tmp/deskhand-mcp-img")


def url_and_token():
    cfg = json.load(open(os.path.join(DATA, "claude-mcp.json")))
    srv = next(iter(cfg["mcpServers"].values()))
    token = open(os.path.join(DATA, "mcp-token")).read().strip()
    return srv["url"], token


def post(body, session=None, method="POST"):
    url, token = url_and_token()
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Authorization", "Bearer " + token)
    req.add_header("Content-Type", "application/json")
    req.add_header("Accept", "application/json, text/event-stream")
    if session:
        req.add_header("Mcp-Session-Id", session)
    try:
        resp = urllib.request.urlopen(req, timeout=300)
    except urllib.error.HTTPError as e:
        sys.exit(f"HTTP {e.code}: {e.read().decode(errors='replace')[:300]}")
    sid = resp.headers.get("Mcp-Session-Id")
    raw = resp.read().decode()
    msgs = []
    if resp.headers.get("Content-Type", "").startswith("text/event-stream"):
        for line in raw.splitlines():
            if line.startswith("data:"):
                d = line[5:].strip()
                if d:
                    msgs.append(json.loads(d))
    elif raw.strip():
        msgs.append(json.loads(raw))
    return sid, msgs


def session():
    try:
        return open(SESSION_FILE).read().strip()
    except FileNotFoundError:
        sys.exit("no session: run `mcp_call.py init` first")


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else "list"
    if cmd == "init":
        sid, msgs = post({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "mcp_call", "version": "0"}}})
        post({"jsonrpc": "2.0", "method": "notifications/initialized"}, sid)
        open(SESSION_FILE, "w").write(sid)
        info = msgs[0]["result"]["serverInfo"] if msgs else {}
        print(f"session started ({info.get('name')} {info.get('version')})")
    elif cmd == "list":
        sid = session()
        _, msgs = post({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}, sid)
        for t in msgs[-1]["result"]["tools"]:
            print(t["name"], "-", t["description"].split(".")[0])
    elif cmd == "call":
        sid = session()
        tool = sys.argv[2]
        args = json.loads(sys.argv[3]) if len(sys.argv) > 3 else {}
        t0 = time.time()
        _, msgs = post({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                        "params": {"name": tool, "arguments": args}}, sid)
        res = msgs[-1]
        if "error" in res:
            print("JSON-RPC error:", res["error"])
            return
        r = res["result"]
        print(f"[{tool} {'ERROR' if r.get('isError') else 'ok'} in {time.time() - t0:.2f}s]")
        os.makedirs(IMG_DIR, exist_ok=True)
        for c in r.get("content", []):
            if c["type"] == "text":
                print(c["text"])
            elif c["type"] == "image":
                p = os.path.join(IMG_DIR, f"{tool}-{int(time.time() * 1000)}.jpg")
                open(p, "wb").write(base64.b64decode(c["data"]))
                print(f"(image saved: {p})")
    elif cmd == "end":
        sid = session()
        post(None, sid, method="DELETE")
        os.remove(SESSION_FILE)
        print("session ended")
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
