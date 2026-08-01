#!/usr/bin/env python3
"""One timed language-server startup, over the real stdio protocol.

Driven by `scripts/lsp-startup-bench.sh`, which builds the binaries and
generates the corpus; this script owns exactly one thing — the clock.

The measurement is `initialize` sent -> the FIRST `textDocument/
publishDiagnostics` for the opened document. That is what the editor's user
experiences as "the server came up": the handshake, the workspace bootstrap,
the rock-tree harvest, and the first diagnostic pass for the file they are
looking at. Timing anything narrower (say, the `initialize` response) would
miss the harvest entirely, which is the thing under measurement.

Prints one line of JSON: {"ms": <float>, "diagnostics": <int>}.
"""

import argparse
import json
import os
import pathlib
import subprocess
import sys
import time


def send(proc, payload):
    body = json.dumps(payload).encode("utf-8")
    proc.stdin.write(b"Content-Length: %d\r\n\r\n" % len(body))
    proc.stdin.write(body)
    proc.stdin.flush()


def read_message(proc):
    """One LSP message off stdout, or None at EOF."""
    length = None
    while True:
        line = proc.stdout.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        name, _, value = line.decode("ascii", "replace").partition(":")
        if name.strip().lower() == "content-length":
            length = int(value.strip())
    if length is None:
        return None
    body = b""
    while len(body) < length:
        chunk = proc.stdout.read(length - len(body))
        if not chunk:
            return None
        body += chunk
    return json.loads(body.decode("utf-8"))


def uri_of(path):
    return pathlib.Path(path).absolute().as_uri()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--luabox", required=True, help="path to the luabox binary")
    ap.add_argument("--project", required=True, help="project root to open")
    ap.add_argument(
        "--document",
        default="src/main.lua",
        help="project-relative file to open and wait for diagnostics on",
    )
    ap.add_argument(
        "--timeout", type=float, default=120.0, help="seconds before giving up"
    )
    args = ap.parse_args()

    root = pathlib.Path(args.project).absolute()
    document = root / args.document
    text = document.read_text(encoding="utf-8")
    doc_uri = uri_of(document)

    proc = subprocess.Popen(
        [args.luabox, "lsp"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        cwd=str(root),
        env={**os.environ, "RUST_BACKTRACE": "0"},
    )

    started = time.perf_counter()
    send(
        proc,
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "processId": os.getpid(),
                "rootUri": uri_of(root),
                "capabilities": {
                    # Work-done progress is advertised so the bootstrap takes
                    # the same code path a real editor drives.
                    "window": {"workDoneProgress": True},
                    "textDocument": {"publishDiagnostics": {}},
                },
            },
        },
    )

    deadline = started + args.timeout
    elapsed = None
    count = 0
    opened = False
    while time.perf_counter() < deadline:
        message = read_message(proc)
        if message is None:
            break
        if not opened and message.get("id") == 1 and "result" in message:
            send(proc, {"jsonrpc": "2.0", "method": "initialized", "params": {}})
            send(
                proc,
                {
                    "jsonrpc": "2.0",
                    "method": "textDocument/didOpen",
                    "params": {
                        "textDocument": {
                            "uri": doc_uri,
                            "languageId": "lua",
                            "version": 1,
                            "text": text,
                        }
                    },
                },
            )
            opened = True
            continue
        if (
            message.get("method") == "textDocument/publishDiagnostics"
            and message.get("params", {}).get("uri") == doc_uri
        ):
            elapsed = (time.perf_counter() - started) * 1000.0
            count = len(message["params"].get("diagnostics", []))
            break
        # A server->client request (progress token creation, watcher
        # registration) blocks the server until it is answered.
        if "id" in message and "method" in message:
            send(proc, {"jsonrpc": "2.0", "id": message["id"], "result": None})

    try:
        send(proc, {"jsonrpc": "2.0", "id": 99, "method": "shutdown", "params": None})
        send(proc, {"jsonrpc": "2.0", "method": "exit", "params": None})
        proc.stdin.close()
        proc.wait(timeout=10)
    except Exception:
        proc.kill()

    if elapsed is None:
        print("lsp-startup-bench: no publishDiagnostics before the timeout", file=sys.stderr)
        return 1
    print(json.dumps({"ms": elapsed, "diagnostics": count}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
