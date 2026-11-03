"""Serves a local Motocross Madness 2 install to the browser build, for development.

    python tools/serve_game.py "D:/GAMES/.../Motorcross Madness 2" [--port 8081] [--with TERAFORM/...] [--page dist]

GET /manifest.json lists the files to load up front, GET /remote.json the
(path, size) of the track files the page fetches when an event is opened; any
other path serves that file from the install, read-only. Only 127.0.0.1 is bound. By default the menus and
shared resources load (UI with its movies, RES, BIKES, RIDERS); `--with` adds
more folders. `--page` also serves a built page (a `trunk build` folder) from the same
port, so one server is all a test needs: open http://127.0.0.1:8081/?game=http://127.0.0.1:8081.
The install is never written to.
"""
import argparse, json, os
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer

SKIP = {".pdf", ".hlp", ".exe", ".dll", ".rtf", ".txt", ".icd", ".cur", ".016", ".256", ".sem"}
BASE = ["UI", "RES", "BIKES", "RIDERS"]
# The menus' text lives in the string tables of these two libraries.
ROOT_FILES = ["UILANG.DLL", "LANG.DLL"]
# Fetched on demand, by event.
REMOTE = ["TERAFORM"]

ap = argparse.ArgumentParser()
ap.add_argument("game")
ap.add_argument("--port", type=int, default=8081)
ap.add_argument("--with", dest="extra", action="append", default=[])
ap.add_argument("--page")
args = ap.parse_args()
root = os.path.abspath(args.game)


def manifest():
    out = [f for f in ROOT_FILES if os.path.isfile(os.path.join(root, f))]
    for top in BASE + args.extra:
        for d, _, files in os.walk(os.path.join(root, top)):
            for f in files:
                if os.path.splitext(f)[1].lower() not in SKIP or f.upper() == "PCNAMES.TXT":
                    out.append(os.path.relpath(os.path.join(d, f), root).replace("\\", "/"))
    return sorted(out)


def remote():
    eager = set(manifest())
    out = []
    for top in REMOTE:
        for d, _, files in os.walk(os.path.join(root, top)):
            for f in files:
                p = os.path.relpath(os.path.join(d, f), root).replace("\\", "/")
                if p not in eager and os.path.splitext(f)[1].lower() not in SKIP:
                    out.append([p, os.path.getsize(os.path.join(root, p))])
    return sorted(out)


class H(SimpleHTTPRequestHandler):
    def __init__(self, *a, **k):
        super().__init__(*a, directory=root, **k)

    def translate_path(self, path):
        # The built page wins over the install, which has no index of its own.
        if args.page:
            mine = super().translate_path(path)
            page = os.path.join(os.path.abspath(args.page), os.path.relpath(mine, root))
            if os.path.isfile(page) or os.path.isdir(page) and os.path.isfile(os.path.join(page, "index.html")):
                return page
        return super().translate_path(path)

    def end_headers(self):
        self.send_header("Access-Control-Allow-Origin", "*")
        super().end_headers()

    def do_GET(self):
        if self.path.startswith("/__fonts/"):
            # The four Arial faces the game's text uses, from this machine's
            # Windows fonts; for local development only.
            name = self.path[len("/__fonts/"):]
            path = os.path.join(os.environ.get("WINDIR", "C:\\Windows"), "Fonts", name)
            if name.lower() not in {"arial.ttf", "arialbd.ttf", "arialbi.ttf", "ariali.ttf"} or not os.path.isfile(path):
                self.send_error(404)
                return
            with open(path, "rb") as f:
                body = f.read()
            self.send_response(200)
            self.send_header("Content-Type", "font/ttf")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        elif self.path in ("/manifest.json", "/remote.json"):
            body = json.dumps(manifest() if self.path == "/manifest.json" else remote()).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        else:
            super().do_GET()

    def log_message(self, *a):
        pass


print(f"serving {root} on http://127.0.0.1:{args.port} ({len(manifest())} files)")
ThreadingHTTPServer(("127.0.0.1", args.port), H).serve_forever()
