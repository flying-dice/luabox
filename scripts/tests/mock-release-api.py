#!/usr/bin/env python3
"""A stand-in for GitHub's release API, so the draft-install path is exercised.

`scripts/install.sh`'s draft path (LUABOX_DRAFT_INSTALL=1) could not be run
anywhere before a real `v*` tag existed: it needs a draft release, a token that
can see it, and asset-id endpoints that redirect to storage. So the first thing
that ever ran it was a production release — the one moment where a bug in it is
most expensive. This serves the same shapes locally, and CI's
`draft-install-mock` job runs the real installer against it on every push.

Deliberately python3 stdlib only: the repo already depends on python3 in CI
(scripts/per-crate-coverage.py) and adding a package for a test server would be
a new dependency in the release path's own test.

What it models, and why each piece is here:

* **`GET /repos/<owner>/<repo>/releases?per_page&page`** — the LIST endpoint,
  because `GET /releases/tags/<tag>` does not return a draft even with a token.
  It is **paginated with the wanted tag on page 2**, so the installer's page
  walk is actually walked rather than passing by luck on page 1; page 3 is
  empty, which is the installer's stop condition.
* **`GET /repos/<owner>/<repo>/releases/assets/<id>`** — answers **302** to a
  second port, exactly as the real API redirects to pre-signed storage. The
  storage side asserts the `Authorization` header is **absent**: that
  cross-host auth drop is the riskiest logic in the whole path (curl gets it
  free from `-L`; wget forwards headers across redirects and has to drop them
  by hand), and it is silent when it breaks — the real storage host answers 400
  and the installer reports a download failure that names nothing.
* **Authorization on the API endpoints** is *required*: a request without a
  bearer token gets 401, so an installer that quietly stopped sending one would
  fail here rather than in production.

Usage:
    mock-release-api.py --tag v9.9.9 --asset-dir DIR [--port 0] [--storage-port 0]

Prints one line, `API_BASE=http://127.0.0.1:<port>`, once both ports are up,
then serves until killed. Every file in --asset-dir is offered as an asset of
the release tagged --tag.
"""

import argparse
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

#: Releases per listing page. Small on purpose: the point is to force a walk.
PER_PAGE = 1


class State:
    """Everything both servers need, filled in before either starts."""

    tag = ""
    #: asset id -> (name, bytes)
    assets: dict = {}
    storage_base = ""
    #: Set if the storage side ever saw an Authorization header — the failure
    #: this whole mock exists to catch.
    auth_leaked = False


def other_releases() -> list:
    """Filler releases the walk has to step past to reach the wanted tag."""
    return [{"tag_name": "v0.0.1-decoy", "draft": False, "assets": []}]


def wanted_release() -> dict:
    return {
        "tag_name": State.tag,
        "draft": True,
        "assets": [
            {"name": name, "id": asset_id, "size": len(body)}
            for asset_id, (name, body) in sorted(State.assets.items())
        ],
    }


def page(number: int) -> list:
    """The release listing, with the wanted tag on page 2 and page 3 empty."""
    pages = [other_releases(), [wanted_release()], []]
    if 1 <= number <= len(pages):
        return pages[number - 1]
    return []


class ApiHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):  # noqa: D102 - quiet by default
        pass

    def _json(self, status: int, payload) -> None:
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's name
        parsed = urlparse(self.path)
        # Every API endpoint is authenticated: a draft is invisible without it.
        if not (self.headers.get("Authorization") or "").startswith("Bearer "):
            self._json(401, {"message": "Requires authentication"})
            return

        if parsed.path.endswith("/releases"):
            number = int(parse_qs(parsed.query).get("page", ["1"])[0])
            self._json(200, page(number))
            return

        if "/releases/assets/" in parsed.path:
            asset_id = parsed.path.rsplit("/", 1)[-1]
            if asset_id not in State.assets:
                self._json(404, {"message": "Not Found"})
                return
            # The real API redirects to a pre-signed URL on a different host.
            self.send_response(302)
            self.send_header("Location", f"{State.storage_base}/{asset_id}")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return

        self._json(404, {"message": "Not Found"})


class StorageHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):  # noqa: D102 - quiet by default
        pass

    def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's name
        # THE assertion this mock exists for. Real pre-signed storage answers
        # 400 to a request that also carries a bearer token; answering 400 here
        # makes the installer fail loudly instead of silently relying on a
        # client's redirect behaviour.
        if self.headers.get("Authorization"):
            State.auth_leaked = True
            body = b"credentials must not be forwarded across the redirect"
            self.send_response(400)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return

        asset_id = urlparse(self.path).path.lstrip("/")
        if asset_id not in State.assets:
            self.send_response(404)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        body = State.assets[asset_id][1]
        self.send_response(200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True, help="the draft release's tag")
    parser.add_argument(
        "--asset-dir",
        required=True,
        help="directory whose files become the release's assets",
    )
    parser.add_argument("--port", type=int, default=0, help="API port (0 = pick one)")
    parser.add_argument(
        "--storage-port", type=int, default=0, help="storage port (0 = pick one)"
    )
    args = parser.parse_args()

    State.tag = args.tag
    for index, path in enumerate(sorted(Path(args.asset_dir).iterdir())):
        if path.is_file():
            State.assets[str(1000 + index)] = (path.name, path.read_bytes())
    if not State.assets:
        print(f"mock: no assets in {args.asset_dir}", file=sys.stderr)
        return 2

    storage = ThreadingHTTPServer(("127.0.0.1", args.storage_port), StorageHandler)
    State.storage_base = f"http://127.0.0.1:{storage.server_address[1]}"
    threading.Thread(target=storage.serve_forever, daemon=True).start()

    api = ThreadingHTTPServer(("127.0.0.1", args.port), ApiHandler)
    # The caller blocks on this line, so it must only be printed once both
    # listeners are bound.
    print(f"API_BASE=http://127.0.0.1:{api.server_address[1]}", flush=True)
    api.serve_forever()
    return 0


if __name__ == "__main__":
    sys.exit(main())
