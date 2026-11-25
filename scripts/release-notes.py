"""Print the CHANGELOG.md section for one release version.

Usage: release-notes.py <version>

Fails when CHANGELOG.md has no `## [<version>] - YYYY-MM-DD` heading or the
section is empty, so a release tag cannot be published without notes.
"""
from pathlib import Path
import re
import sys

version = sys.argv[1]
lines = Path("CHANGELOG.md").read_text(encoding="utf-8").splitlines()
heading = re.compile(rf"^## \[{re.escape(version)}\] - \d{{4}}-\d{{2}}-\d{{2}}$")
starts = [index for index, line in enumerate(lines) if heading.match(line)]
if len(starts) != 1:
    raise SystemExit(f"CHANGELOG.md needs exactly one '## [{version}] - YYYY-MM-DD' heading")
section = []
for line in lines[starts[0] + 1:]:
    if line.startswith("## ") or re.match(r"^\[[^\]]+\]: ", line):
        break
    section.append(line)
body = "\n".join(section).strip()
if not body:
    raise SystemExit(f"CHANGELOG.md section for {version} is empty")
print(body)
