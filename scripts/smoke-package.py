"""Run the downloaded GUI binary and forward a local test request through it."""
import hashlib
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import os
from pathlib import Path
import platform
import socket
import subprocess
import sys
import tarfile
import tempfile
import threading
import time


class Origin(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"artifact-smoke-ok")


archives = list(Path(sys.argv[1]).glob("*.tar.gz"))
assert len(archives) == 1, archives
archive = archives[0]
expected = archive.with_suffix(archive.suffix + ".sha256").read_text().split()[0]
assert hashlib.sha256(archive.read_bytes()).hexdigest() == expected
with tempfile.TemporaryDirectory() as temporary:
    root = Path(temporary)
    with tarfile.open(archive) as package:
        package.extractall(root, filter="data")
    binary = next(root.glob("*/http-proxy"))
    assert os.access(binary, os.X_OK), "Archive lost executable permission"
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    origin = ThreadingHTTPServer(("127.0.0.1", 0), Origin)
    threading.Thread(target=origin.serve_forever, daemon=True).start()
    environment = dict(os.environ, HOME=str(root), XDG_DATA_HOME=str(root / "data"))
    with (root / "app.log").open("w+") as log:
        process = subprocess.Popen([str(binary.resolve()), "--port", str(port)],
                                   cwd=root, env=environment, stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 60
            while True:
                assert process.poll() is None, "GUI process exited before proxy was ready"
                try:
                    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=2)
                    connection.request("GET", f"http://127.0.0.1:{origin.server_port}/artifact-smoke")
                    response = connection.getresponse()
                    assert response.status == 200
                    assert response.read() == b"artifact-smoke-ok"
                    break
                except OSError:
                    if time.monotonic() >= deadline:
                        raise
                    time.sleep(0.5)
                finally:
                    connection.close()
            print(f"PASS downloaded {archive.name}: {platform.platform()} {platform.machine()}, GUI alive and HTTP forwarded")
        except BaseException:
            log.seek(0)
            print(log.read())
            raise
        finally:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            origin.shutdown()
