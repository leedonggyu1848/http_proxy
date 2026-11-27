"""Run the local PR verification steps in order and print a compact summary.

Each step's combined stdout and stderr is written to its own log file in a
fresh temporary directory. A passing step prints a single
`PASS: <step> (<elapsed>s)` line. The first failing step prints its key error
lines, the last lines of its log and the log path, then the script exits with
that step's exit code (or 1) without running later steps.

Usage:
    python3 scripts/verify-pr.py            # full PR verification
    python3 scripts/verify-pr.py --quick    # diff-check, boundaries, fmt, clippy, test
    python3 scripts/verify-pr.py --base origin/main
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
TAIL_LINES = 40
MAX_KEY_LINES = 20
KEY_LINE = re.compile(
    r"^(error|warning)(\[[A-Za-z0-9_]+\])?:"
    r"|^\s+--> "
    r"|panicked at"
    r"|^test .+ FAILED$"
    r"|^test result: FAILED"
    r"|^Diff in "
    r"|^[^:\s]+:\d+: "
    r"|^Traceback "
    r"|^\S*Error: "
)


@dataclass(frozen=True)
class Step:
    name: str
    command: list[str]
    quick: bool
    env: dict[str, str] = field(default_factory=dict)


def diff_base(requested: str | None) -> str:
    """Return the commit `git diff --check` compares the working tree against.

    Comparing against the merge base with the main branch covers both
    committed and uncommitted changes of the branch; without a reachable base
    the check falls back to uncommitted changes only.
    """
    candidates = [requested] if requested else ["origin/main", "main"]
    for candidate in candidates:
        result = subprocess.run(
            ["git", "merge-base", candidate, "HEAD"],
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
            check=False,
        )
        if result.returncode == 0:
            return result.stdout.strip()
    if requested:
        raise SystemExit(f"verify-pr: no merge base between {requested} and HEAD")
    return "HEAD"


def steps(base: str) -> list[Step]:
    return [
        Step("diff-check", ["git", "diff", "--check", base], quick=True),
        Step("check-boundaries", [sys.executable, "scripts/check-boundaries.py"], quick=True),
        Step("fmt", ["cargo", "fmt", "--all", "--check"], quick=True),
        Step(
            "clippy",
            [
                "cargo", "clippy", "--workspace", "--all-targets", "--all-features",
                "--", "-D", "warnings",
            ],
            quick=True,
        ),
        Step("test", ["cargo", "test", "--workspace", "--all-features"], quick=True),
        Step(
            "doc",
            ["cargo", "doc", "--workspace", "--all-features", "--no-deps"],
            quick=False,
            env={"RUSTDOCFLAGS": "-D warnings"},
        ),
        Step("deny", ["cargo", "deny", "check"], quick=False),
        Step("build-release", ["cargo", "build", "--release"], quick=False),
    ]


def run_step(step: Step, log_path: Path) -> int:
    env = {**os.environ, "CARGO_TERM_COLOR": "never", **step.env}
    with log_path.open("w", encoding="utf-8") as log:
        log.write(f"$ {' '.join(step.command)}\n")
        log.flush()
        try:
            completed = subprocess.run(
                step.command,
                cwd=REPO_ROOT,
                env=env,
                stdin=subprocess.DEVNULL,
                stdout=log,
                stderr=subprocess.STDOUT,
                check=False,
            )
        except FileNotFoundError as error:
            log.write(f"verify-pr: command not found: {error.filename}\n")
            return 127
    return completed.returncode


def report_failure(step: Step, log_path: Path, code: int, elapsed: float) -> None:
    lines = log_path.read_text(encoding="utf-8", errors="replace").splitlines()
    key_lines = [line for line in lines if KEY_LINE.search(line)]
    print(f"FAIL: {step.name} ({elapsed:.1f}s, exit {code})")
    if key_lines:
        print(f"--- key error lines (first {MAX_KEY_LINES}) ---")
        for line in key_lines[:MAX_KEY_LINES]:
            print(line)
    print(f"--- last {TAIL_LINES} log lines ---")
    for line in lines[-TAIL_LINES:]:
        print(line)
    print(f"Full log: {log_path}")


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run local PR verification steps in order.")
    parser.add_argument(
        "--quick",
        action="store_true",
        help="run only diff-check, check-boundaries, fmt, clippy and test",
    )
    parser.add_argument(
        "--base",
        help="revision whose merge base with HEAD git diff --check compares against "
        "(default: origin/main, then main)",
    )
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    selected = [step for step in steps(diff_base(args.base)) if step.quick or not args.quick]
    log_dir = Path(tempfile.mkdtemp(prefix="verify-pr-"))
    started = time.monotonic()
    for index, step in enumerate(selected, start=1):
        log_path = log_dir / f"{index:02d}-{step.name}.log"
        step_started = time.monotonic()
        code = run_step(step, log_path)
        elapsed = time.monotonic() - step_started
        if code != 0:
            report_failure(step, log_path, code, elapsed)
            return code if 0 < code < 256 else 1
        print(f"PASS: {step.name} ({elapsed:.1f}s)", flush=True)
    mode = "quick" if args.quick else "full"
    total = time.monotonic() - started
    print(f"OK: {len(selected)} {mode} steps passed ({total:.1f}s). Logs: {log_dir}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
