"""Build distribution archives from an explicit public-file allowlist."""
import hashlib
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib

manifest = tomllib.loads(Path("Cargo.toml").read_text())
raw_version = manifest["package"]["version"]
version = (
    manifest["workspace"]["package"]["version"]
    if isinstance(raw_version, dict) and raw_version.get("workspace")
    else raw_version
)
tag = os.environ.get("REF_NAME", "")
if os.environ.get("REF_TYPE") == "tag":
    if not re.fullmatch(r"v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", tag) or tag != f"v{version}":
        raise SystemExit("Release tag must equal v<Cargo.toml package.version>")
if sys.argv[1] == "--validate":
    sys.exit(0)
target = sys.argv[1]
if target not in {"x86_64-unknown-linux-gnu", "aarch64-apple-darwin", "x86_64-apple-darwin"}:
    raise SystemExit("Unsupported distribution target")
commit = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
identity = tag if os.environ.get("REF_TYPE") == "tag" else f"v{version}-{commit}"
name = f"http-proxy-{identity}-{target}"
dist = Path("dist")
dist.mkdir(exist_ok=True)
with tempfile.TemporaryDirectory() as temporary:
    root = Path(temporary) / name
    root.mkdir()
    for source, destination in [
        (f"target/{target}/release/http-proxy", "http-proxy"),
        ("README.md", "README.md"),
        ("LICENSE", "LICENSE"),
        ("assets/fonts/OFL.txt", "FONT-LICENSE.txt"),
    ]:
        shutil.copyfile(source, root / destination)
    (root / "http-proxy").chmod(0o755)
    (root / "BUILD.txt").write_text(f"version={version}\ncommit={commit}\ntarget={target}\n")
    archive = dist / f"{name}.tar.gz"
    with tarfile.open(archive, "w:gz") as output:
        output.add(root, arcname=name)
digest = hashlib.sha256(archive.read_bytes()).hexdigest()
archive.with_suffix(archive.suffix + ".sha256").write_text(f"{digest}  {archive.name}\n")
print(archive)
