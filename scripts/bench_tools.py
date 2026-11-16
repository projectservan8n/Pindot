#!/usr/bin/env python3
"""Time Pindot's tools, the same way on macOS and Windows, to compare the two.

Needs a running Pindot server (the app, or `spike_mcp`) and a window the
test may use, with a button in it. On a test build the app is approved by
PINDOT_TEST_AUTO_APPROVE; otherwise answer the approval window yourself.

  macOS:
    swiftc -O -o spikes/test-target/test-target spikes/test-target/main.swift
    spikes/test-target/test-target &
    cargo build --release -p pindot-core --bin spike_mcp --features test-switches
    PINDOT_DATA_DIR=/tmp/pindot-bench PINDOT_TEST_AUTO_APPROVE=test-target \\
        target/release/spike_mcp --port 47999 &
    PINDOT_DATA_DIR=/tmp/pindot-bench python3 scripts/bench_tools.py --app test-target

  Windows (any small window of your own with a button; see
  docs/windows-test-checklist.md for the one that was used):
    set PINDOT_DATA_DIR and PINDOT_TEST_AUTO_APPROVE the same way, then
    python scripts\\bench_tools.py --app <its name> --title <its title>

Each call is timed around the HTTP request, as scripts/mcp_call.py does.
Measure release builds only. The figures of both systems are kept in
docs/windows-test-checklist.md.
"""
import argparse
import os
import statistics
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mcp_call  # noqa: E402  (the test client next to this file)


def start():
    sid, _ = mcp_call.post({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "bench_tools", "version": "0"}}})
    mcp_call.post({"jsonrpc": "2.0", "method": "notifications/initialized"}, sid)
    return sid


def call(sid, tool, args):
    """One tool call: (milliseconds, the text of its result)."""
    t0 = time.perf_counter()
    _, msgs = mcp_call.post({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                             "params": {"name": tool, "arguments": args}}, sid)
    ms = (time.perf_counter() - t0) * 1000
    result = msgs[-1]["result"]
    text = "\n".join(c["text"] for c in result.get("content", []) if c["type"] == "text")
    if result.get("isError"):
        sys.exit(f"{tool} failed: {text[:300]}")
    return ms, text


def show(name, times):
    times = sorted(times)
    p95 = times[max(0, int(round(0.95 * len(times))) - 1)]
    print(f"{name}: n={len(times)}, median {statistics.median(times):.1f} ms, p95 {p95:.1f} ms, max {times[-1]:.1f} ms")


def button_ref(tree, needles):
    """The ref of the first button whose line holds one of the words."""
    for line in tree.splitlines():
        if "] button " in line and any(n in line for n in needles):
            return line.split("]")[0].lstrip("[")
    return None


def main():
    ap = argparse.ArgumentParser(description="Time Pindot's tools on one test window.")
    ap.add_argument("--app", required=True, help="the test window's app, as request_access names it")
    ap.add_argument("--title", default="Pindot Test Target", help="part of the window's title")
    ap.add_argument("--button", default="Pindot Test Button,Clicked,Press", help="words of the button's name, comma-separated")
    args = ap.parse_args()
    needles = [n for n in args.button.split(",") if n]

    sid = start()
    try:
        call(sid, "request_access", {"apps": [args.app], "reason": "timing the tools on a test window"})
        _, context = call(sid, "desktop_context", {})
        line = next((l for l in context.splitlines() if "window_id" in l and args.title in l), None)
        if line is None:
            sys.exit(f"no approved window with \"{args.title}\" in its title; is the test window open?")
        window = int(line.split("window_id")[1].split()[0])

        show("screenshot, 40 back to back", [call(sid, "screenshot", {})[0] for _ in range(40)])
        spaced = []
        for _ in range(12):
            time.sleep(2)
            spaced.append(call(sid, "screenshot", {})[0])
        show("screenshot, 12 two seconds apart", spaced)
        show("read_window", [call(sid, "read_window", {"window_id": window})[0] for _ in range(30)])
        show("read_window with menus",
             [call(sid, "read_window", {"window_id": window, "include_menus": True})[0] for _ in range(20)])
        show("find", [call(sid, "find", {"query": needles[0].split()[0]})[0] for _ in range(20)])
        show("desktop_context", [call(sid, "desktop_context", {})[0] for _ in range(20)])
        presses = []
        for _ in range(10):
            _, tree = call(sid, "read_window", {"window_id": window})
            ref = button_ref(tree, needles)
            if ref is None:
                sys.exit("no button with those words in the test window; pass --button")
            presses.append(call(sid, "click", {"ref": ref})[0])
        show("click by ref (with the AI cursor's glide, if the app draws one)", presses)
    finally:
        mcp_call.post(None, sid, method="DELETE")


if __name__ == "__main__":
    main()
