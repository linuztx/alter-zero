#!/usr/bin/env python3
"""A stand-in for registry.npmjs.org, so the npm packages can be published
and installed offline (docs/npm.md).

    python3 scripts/release/npm_registry.py DIR

Serves every `*.tgz` in DIR the way the registry does: a package's document
(its "packument") at /NAME — a scoped name arriving as /@scope%2fname — built
from the package.json inside each of its tarballs, with `dist.tarball`
pointing back here and the `integrity` and `shasum` npm verifies, and
`dist-tags.latest` naming its newest version; the tarballs themselves under
/-/; a 404 for any other package, which is how npm learns that an optional
dependency cannot be had. `npm publish` lands here too: the PUT's tarball is
written into DIR under `npm pack`'s own name and the version appended to
DIR/published.log, in the order the versions arrived, and a version already
present is refused with the registry's 403 — versions are immutable there
too. Any token is accepted. Binds an ephemeral port on 127.0.0.1 and prints
it on the first line of stdout; the selftest points npm's registry at it,
which makes `scripts/release.sh npm` and `npm install -g` the real thing
minus the network.
"""
import base64
import hashlib
import http.server
import json
import os
import socketserver
import sys
import tarfile
import urllib.parse

DIR = sys.argv[1]


def version_key(version):
    """Order versions well enough for a fixture: numeric release parts, a
    pre-release below its release."""
    release, _, pre = version.partition("-")
    parts = tuple(int(p) if p.isdigit() else 0 for p in release.split("."))
    return parts, pre == "", pre


def load():
    """{name: {version: (manifest, tarball file name, bytes)}} for DIR."""
    packages = {}
    for entry in sorted(os.listdir(DIR)):
        if not entry.endswith(".tgz"):
            continue
        path = os.path.join(DIR, entry)
        with tarfile.open(path, "r:gz") as tar:
            manifest = json.load(tar.extractfile("package/package.json"))
        with open(path, "rb") as fh:
            data = fh.read()
        packages.setdefault(manifest["name"], {})[manifest["version"]] = (manifest, entry, data)
    return packages


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_args):  # quiet
        pass

    def _send(self, status, body, ctype):
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _not_found(self):
        self._send(404, b'{"error":"Not found"}', "application/json")

    def do_GET(self):
        packages = load()
        path = urllib.parse.unquote(self.path.split("?", 1)[0])
        if path.startswith("/-/"):
            name = os.path.basename(path)
            for versions in packages.values():
                for _manifest, entry, data in versions.values():
                    if entry == name:
                        return self._send(200, data, "application/octet-stream")
            return self._not_found()
        name = path.lstrip("/")
        versions = packages.get(name)
        if not versions:
            return self._not_found()
        base = f"http://127.0.0.1:{self.server.server_address[1]}/-/"
        doc = {"name": name, "dist-tags": {}, "versions": {}}
        for version, (manifest, entry, data) in versions.items():
            sha512 = base64.b64encode(hashlib.sha512(data).digest()).decode()
            doc["versions"][version] = dict(
                manifest,
                _id=f"{name}@{version}",
                dist={
                    "tarball": base + urllib.parse.quote(entry),
                    "integrity": f"sha512-{sha512}",
                    "shasum": hashlib.sha1(data).hexdigest(),
                },
            )
        doc["dist-tags"]["latest"] = max(versions, key=version_key)
        self._send(200, json.dumps(doc).encode(), "application/json")

    def _body(self):
        if self.headers.get("Transfer-Encoding", "").lower() == "chunked":
            data = b""
            while True:
                size = int(self.rfile.readline().split(b";")[0], 16)
                if size == 0:
                    self.rfile.readline()
                    return data
                data += self.rfile.read(size)
                self.rfile.readline()
        return self.rfile.read(int(self.headers.get("Content-Length", 0)))

    def do_PUT(self):
        doc = json.loads(self._body())
        name = doc["name"]
        known = load().get(name, {})
        for version in doc.get("versions", {}):
            if version in known:
                message = f"You cannot publish over the previously published versions: {version}."
                return self._send(403, json.dumps({"error": message}).encode(), "application/json")
        for attachment in doc.get("_attachments", {}).values():
            # npm sends `@scope/name-1.2.3.tgz`; on disk it is `npm pack`'s
            # own `scope-name-1.2.3.tgz`, which is what load() reads.
            manifest = next(iter(doc["versions"].values()))
            filename = f"{manifest['name'].lstrip('@').replace('/', '-')}-{manifest['version']}.tgz"
            with open(os.path.join(DIR, filename), "wb") as fh:
                fh.write(base64.b64decode(attachment["data"]))
        with open(os.path.join(DIR, "published.log"), "a") as log:
            for version in doc.get("versions", {}):
                log.write(f"{name}@{version}\n")
        self._send(201, b'{"ok":true}', "application/json")


class Server(socketserver.ThreadingMixIn, socketserver.TCPServer):
    allow_reuse_address = True
    daemon_threads = True


with Server(("127.0.0.1", 0), Handler) as server:
    print(server.server_address[1], flush=True)
    server.serve_forever()
