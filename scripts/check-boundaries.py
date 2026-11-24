"""Check domain module dependency boundaries and write-handle ownership.

Enforces the module architecture defined in issues #152 and #161:
1. src/lib.rs declares domain modules and app constants only (no flat `pub use`).
2. No library module (anything in src/ outside src/ui/ and src/main.rs) may
   reference `eframe`, `egui`, or `egui_extras`.
3. Every `crate::...` reference in a domain module must follow the allowed
   dependency graph (`exchange` and `http_semantics` depend on no other domain
   module; `content`, `analysis`, and `interop` never depend on `interception`,
   `recording`, `https`, or `replay`).
4. `Exchange` write handles (`ExchangeWriter`, `BodyPreviewWriter`,
   `TunnelWriter`) may be used in production code only inside `exchange`,
   `interception`, and `replay`.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import sys

ROOT_CONSTANTS: frozenset[str] = frozenset({"APP_NAME", "DEFAULT_PORT"})

FORBIDDEN_UI_CRATES: frozenset[str] = frozenset({"eframe", "egui", "egui_extras"})

WRITER_HANDLES: frozenset[str] = frozenset(
    {"ExchangeWriter", "BodyPreviewWriter", "TunnelWriter"}
)
WRITER_ALLOWED_MODULES: frozenset[str] = frozenset(
    {"exchange", "interception", "replay"}
)

# Allowed cross-domain dependencies for each top-level library module.
# Keeping this table exhaustive ensures new top-level modules are classified
# before they can introduce unchecked edges.
ALLOWED_DEPS: dict[str, frozenset[str]] = {
    "exchange": frozenset(),
    "http_semantics": frozenset(),
    "settings": frozenset(),
    "content": frozenset({"exchange", "http_semantics"}),
    "analysis": frozenset({"content", "exchange"}),
    "interop": frozenset({"analysis", "content", "http_semantics", "exchange"}),
    "https": frozenset({"settings"}),
    "launcher": frozenset({"https", "settings"}),
    "recording": frozenset({"http_semantics", "exchange"}),
    "interception": frozenset({"https", "recording", "http_semantics", "exchange"}),
    "replay": frozenset({"interception", "http_semantics", "exchange"}),
}

# `ExchangeBuilder` currently lives in `interception` because it reuses
# `interception::capture` rules. Unit tests in these modules may import only
# these `test-support` fixture items under `#[cfg(test)]` without opening any
# dependency on the proxy runtime or other domains.
TEST_FIXTURE_CONSUMERS: frozenset[str] = frozenset({"analysis", "interop", "recording"})
TEST_FIXTURE_ITEMS: frozenset[str] = frozenset(
    {
        "BODY_PREVIEW_BYTES",
        "ConnectExchangeBuilder",
        "ExchangeBuilder",
        "FixtureError",
        "ResponseFixture",
    }
)


@dataclass(frozen=True)
class Token:
    kind: str  # "IDENT" or "PUNCT"
    text: str
    line: int
    in_test: bool = False


@dataclass(frozen=True)
class Violation:
    path: Path
    line: int
    message: str


def _is_ident_start(ch: str) -> bool:
    return ch == "_" or ch.isalpha()


def _is_ident_continue(ch: str) -> bool:
    return ch == "_" or ch.isalnum()


def _raw_string_hashes(source: str, pos: int) -> int | None:
    """If `source[pos:]` starts a raw string (`r"..."` / `r#"..."#`), return hash count."""
    n = len(source)
    i = pos
    if source[i] in ("b", "c") and i + 1 < n and source[i + 1] == "r":
        i += 2
    elif source[i] == "r":
        i += 1
    else:
        return None
    hashes = 0
    while i < n and source[i] == "#":
        hashes += 1
        i += 1
    if i < n and source[i] == '"':
        return hashes
    return None


def tokenize_rust(source: str) -> list[Token]:
    """Tokenize Rust source into identifiers and punctuation, skipping comments and literals."""
    tokens: list[Token] = []
    n = len(source)
    i = 0
    line = 1

    while i < n:
        ch = source[i]

        if ch == "\n":
            line += 1
            i += 1
            continue
        if ch.isspace():
            i += 1
            continue

        # Line comment.
        if ch == "/" and i + 1 < n and source[i + 1] == "/":
            nl = source.find("\n", i + 2)
            if nl == -1:
                break
            line += 1
            i = nl + 1
            continue

        # Nested block comment.
        if ch == "/" and i + 1 < n and source[i + 1] == "*":
            depth = 1
            i += 2
            while i < n and depth > 0:
                if source[i] == "\n":
                    line += 1
                    i += 1
                elif source[i] == "/" and i + 1 < n and source[i + 1] == "*":
                    depth += 1
                    i += 2
                elif source[i] == "*" and i + 1 < n and source[i + 1] == "/":
                    depth -= 1
                    i += 2
                else:
                    i += 1
            continue

        # Raw string literal (`r"..."`, `r#"..."#`, `br#"..."#`, `cr#"..."#`).
        hashes = _raw_string_hashes(source, i)
        if hashes is not None:
            start_quote = source.index('"', i)
            i = start_quote + 1
            closing = '"' + "#" * hashes
            end = source.find(closing, i)
            if end == -1:
                line += source.count("\n", i)
                break
            line += source.count("\n", i, end)
            i = end + len(closing)
            continue

        # Byte or C normal string (`b"..."`, `c"..."`).
        if ch in ("b", "c") and i + 1 < n and source[i + 1] == '"':
            i += 2
            while i < n:
                if source[i] == "\\":
                    i += 2
                    continue
                if source[i] == "\n":
                    line += 1
                elif source[i] == '"':
                    i += 1
                    break
                i += 1
            continue

        # Normal string literal (`"..."`).
        if ch == '"':
            i += 1
            while i < n:
                if source[i] == "\\":
                    if i + 1 < n and source[i + 1] == "\n":
                        line += 1
                    i += 2
                    continue
                if source[i] == "\n":
                    line += 1
                elif source[i] == '"':
                    i += 1
                    break
                i += 1
            continue

        # Byte char literal (`b'...'`).
        if ch == "b" and i + 1 < n and source[i + 1] == "'":
            i += 2
            while i < n:
                if source[i] == "\\":
                    i += 2
                    continue
                if source[i] == "'":
                    i += 1
                    break
                i += 1
            continue

        # Char literal vs lifetime.
        if ch == "'":
            if i + 1 < n and source[i + 1] == "\\":
                i += 2
                while i < n:
                    if source[i] == "'":
                        i += 1
                        break
                    if source[i] == "\n":
                        line += 1
                    i += 1
                continue
            if i + 2 < n and source[i + 2] == "'" and source[i + 1] != "\n":
                i += 3
                continue
            # Lifetime (`'a`, `'static`, `'_`): consume apostrophe + ident.
            i += 1
            if i < n and _is_ident_start(source[i]):
                while i < n and _is_ident_continue(source[i]):
                    i += 1
            continue

        # Raw identifier (`r#ident`).
        if (
            ch == "r"
            and i + 2 < n
            and source[i + 1] == "#"
            and _is_ident_start(source[i + 2])
        ):
            start = i + 2
            j = start + 1
            while j < n and _is_ident_continue(source[j]):
                j += 1
            tokens.append(Token("IDENT", "r#" + source[start:j], line))
            i = j
            continue

        # Identifier.
        if _is_ident_start(ch):
            j = i + 1
            while j < n and _is_ident_continue(source[j]):
                j += 1
            tokens.append(Token("IDENT", source[i:j], line))
            i = j
            continue

        # Double colon punctuation.
        if ch == ":" and i + 1 < n and source[i + 1] == ":":
            tokens.append(Token("PUNCT", "::", line))
            i += 2
            continue

        # Other single punctuation character.
        tokens.append(Token("PUNCT", ch, line))
        i += 1

    return tokens


def _is_cfg_test_attr(tokens: list[Token], start: int) -> tuple[bool, int]:
    """Check if `tokens[start:]` is an outer `#[cfg(...test...)]` attribute.

    Returns `(is_cfg_test, next_index_after_attr)`.
    """
    n = len(tokens)
    if start + 1 >= n or tokens[start].text != "#" or tokens[start + 1].text != "[":
        return False, start
    # Find matching `]`.
    depth = 0
    end = start + 1
    while end < n:
        if tokens[end].text == "[":
            depth += 1
        elif tokens[end].text == "]":
            depth -= 1
            if depth == 0:
                break
        end += 1
    if end >= n:
        return False, start + 1

    inner = tokens[start + 2 : end]
    if len(inner) >= 3 and inner[0].kind == "IDENT" and inner[0].text == "cfg":
        # Check for `test` token inside `cfg(...)`, excluding `not(test)`.
        saw_test = False
        for idx, tok in enumerate(inner):
            if tok.kind == "IDENT" and tok.text == "test":
                if (
                    idx >= 2
                    and inner[idx - 2].text == "not"
                    and inner[idx - 1].text == "("
                ):
                    continue
                saw_test = True
        return saw_test, end + 1
    return False, end + 1


def annotate_test_scope(tokens: list[Token], whole_file_is_test: bool) -> list[Token]:
    """Mark every token with whether it lies inside a `#[cfg(test)]` item."""
    if whole_file_is_test:
        return [Token(t.kind, t.text, t.line, in_test=True) for t in tokens]

    annotated: list[Token] = []
    n = len(tokens)
    i = 0
    brace_depth = 0
    pending_test = False
    test_enter_depth: int | None = None

    while i < n:
        tok = tokens[i]
        in_test_now = test_enter_depth is not None or pending_test

        # Detect outer attributes (`#[...]`) at the start of an item.
        if tok.text == "#" and i + 1 < n and tokens[i + 1].text == "[":
            is_cfg_test, next_i = _is_cfg_test_attr(tokens, i)
            if is_cfg_test and test_enter_depth is None:
                pending_test = True
                in_test_now = True
            for attr_tok in tokens[i:next_i]:
                annotated.append(
                    Token(attr_tok.kind, attr_tok.text, attr_tok.line, in_test_now)
                )
            i = next_i
            continue

        if tok.text == "{":
            if pending_test and test_enter_depth is None:
                test_enter_depth = brace_depth
                pending_test = False
                in_test_now = True
            brace_depth += 1
            annotated.append(Token(tok.kind, tok.text, tok.line, in_test_now))
            i += 1
            continue

        if tok.text == "}":
            brace_depth = max(0, brace_depth - 1)
            annotated.append(Token(tok.kind, tok.text, tok.line, in_test_now))
            if test_enter_depth is not None and brace_depth <= test_enter_depth:
                test_enter_depth = None
            i += 1
            continue

        if tok.text == ";" and pending_test:
            annotated.append(Token(tok.kind, tok.text, tok.line, True))
            pending_test = False
            i += 1
            continue

        annotated.append(Token(tok.kind, tok.text, tok.line, in_test_now))
        i += 1

    return annotated


def discover_cfg_test_files(src_dir: Path) -> set[Path]:
    """Find files loaded via `#[cfg(test)] mod <name>;`."""
    test_files: set[Path] = set()
    for rs_file in sorted(src_dir.rglob("*.rs")):
        tokens = annotate_test_scope(tokenize_rust(rs_file.read_text()), False)
        for idx in range(len(tokens) - 2):
            tok = tokens[idx]
            if (
                tok.in_test
                and tok.kind == "IDENT"
                and tok.text == "mod"
                and tokens[idx + 1].kind == "IDENT"
                and tokens[idx + 2].text == ";"
            ):
                child = tokens[idx + 1].text
                if rs_file.name in ("mod.rs", "lib.rs", "main.rs"):
                    parent_dir = rs_file.parent
                else:
                    parent_dir = rs_file.parent / rs_file.stem
                for candidate in (
                    parent_dir / f"{child}.rs",
                    parent_dir / child / "mod.rs",
                ):
                    if candidate.exists():
                        test_files.add(candidate.resolve())
    return test_files


def _parse_use_tree(
    tokens: list[Token], pos: int, prefix: list[str]
) -> tuple[list[tuple[list[str], int, bool]], int]:
    """Parse a Rust `use` tree starting at `pos`, returning `(paths, next_pos)`."""
    results: list[tuple[list[str], int, bool]] = []
    n = len(tokens)
    current = list(prefix)
    line = tokens[pos].line if pos < n else 0
    in_test = tokens[pos].in_test if pos < n else False

    while pos < n:
        tok = tokens[pos]
        if tok.text == "{":
            pos += 1
            while pos < n and tokens[pos].text != "}":
                sub, pos = _parse_use_tree(tokens, pos, current)
                results.extend(sub)
                if pos < n and tokens[pos].text == ",":
                    pos += 1
            if pos < n and tokens[pos].text == "}":
                pos += 1
            return results, pos
        if tok.text in (",", "}", ";"):
            if current:
                results.append((current, line, in_test))
            return results, pos
        if tok.kind == "IDENT" and tok.text == "as":
            if current:
                results.append((current, line, in_test))
            pos += 2  # skip `as <alias>`
            return results, pos
        if tok.text == "::":
            pos += 1
            continue
        if tok.kind == "IDENT" or tok.text == "*":
            if not current:
                line = tok.line
                in_test = tok.in_test
            current.append(tok.text)
            pos += 1
            continue
        pos += 1

    if current:
        results.append((current, line, in_test))
    return results, pos


def extract_crate_refs(tokens: list[Token]) -> list[tuple[list[str], int, bool]]:
    """Extract all `crate::...` and `http_proxy::...` paths from token stream."""
    refs: list[tuple[list[str], int, bool]] = []
    n = len(tokens)
    i = 0
    while i < n:
        tok = tokens[i]
        # Handle `use ...;` statements cleanly (including nested `{...}` groups).
        if tok.kind == "IDENT" and tok.text == "use":
            paths, next_i = _parse_use_tree(tokens, i + 1, [])
            for path, line, in_test in paths:
                if len(path) >= 2 and path[0] in ("crate", "http_proxy"):
                    refs.append((path[1:], line, in_test))
            i = next_i + 1
            continue

        # Handle inline paths (`crate::foo::bar`, `pub(in crate::exchange)`).
        if (
            tok.kind == "IDENT"
            and tok.text in ("crate", "http_proxy")
            and i + 2 < n
            and tokens[i + 1].text == "::"
            and tokens[i + 2].kind == "IDENT"
        ):
            path = [tokens[i + 2].text]
            j = i + 3
            while j + 1 < n and tokens[j].text == "::" and tokens[j + 1].kind == "IDENT":
                path.append(tokens[j + 1].text)
                j += 2
            refs.append((path, tok.line, tok.in_test))
            i = j
            continue

        i += 1
    return refs


def check_lib_rs(lib_rs: Path) -> list[Violation]:
    """Ensure src/lib.rs contains no flat `pub use` re-exports."""
    violations: list[Violation] = []
    tokens = tokenize_rust(lib_rs.read_text())
    for i in range(len(tokens) - 1):
        if (
            tokens[i].kind == "IDENT"
            and tokens[i].text == "pub"
            and tokens[i + 1].kind == "IDENT"
            and tokens[i + 1].text == "use"
        ):
            violations.append(
                Violation(
                    lib_rs,
                    tokens[i].line,
                    "src/lib.rs must declare domain modules and app constants only (no flat `pub use`)",
                )
            )
    return violations


def domain_module_for(src_dir: Path, path: Path) -> str | None:
    rel = path.relative_to(src_dir)
    first = rel.parts[0]
    if first in ("lib.rs", "main.rs", "ui"):
        return None
    return first.removesuffix(".rs")


def check_repository(root: Path) -> list[Violation]:
    src_dir = root / "src"
    violations: list[Violation] = []

    lib_rs = src_dir / "lib.rs"
    violations.extend(check_lib_rs(lib_rs))

    # Verify every top-level entry in src/ is classified.
    for entry in sorted(src_dir.iterdir()):
        if entry.name in ("lib.rs", "main.rs", "ui"):
            continue
        mod_name = entry.name.removesuffix(".rs")
        if mod_name not in ALLOWED_DEPS:
            violations.append(
                Violation(
                    entry,
                    1,
                    f"unknown domain module `{mod_name}`; classify it in scripts/check-boundaries.py",
                )
            )

    test_files = discover_cfg_test_files(src_dir)

    for rs_file in sorted(src_dir.rglob("*.rs")):
        rel = rs_file.relative_to(src_dir)
        if rel.parts[0] in ("ui", "main.rs"):
            continue

        whole_file_is_test = rs_file.resolve() in test_files
        tokens = annotate_test_scope(
            tokenize_rust(rs_file.read_text()), whole_file_is_test
        )

        # Rule 2: No eframe/egui references anywhere outside src/ui/ and src/main.rs.
        for tok in tokens:
            if tok.kind == "IDENT" and tok.text in FORBIDDEN_UI_CRATES:
                violations.append(
                    Violation(
                        rs_file,
                        tok.line,
                        f"library code must not reference UI crate `{tok.text}`",
                    )
                )

        src_mod = domain_module_for(src_dir, rs_file)
        if src_mod is None or src_mod not in ALLOWED_DEPS:
            continue

        # Rule 5: Exchange write handles are owned exclusively by exchange/interception/replay.
        if src_mod not in WRITER_ALLOWED_MODULES:
            for tok in tokens:
                if (
                    not tok.in_test
                    and tok.kind == "IDENT"
                    and tok.text in WRITER_HANDLES
                ):
                    violations.append(
                        Violation(
                            rs_file,
                            tok.line,
                            f"module `{src_mod}` must not use write handle `{tok.text}` "
                            f"(allowed only in {sorted(WRITER_ALLOWED_MODULES)})",
                        )
                    )

        # Rule 3 & 4: Domain dependency direction.
        allowed = ALLOWED_DEPS[src_mod]
        for path_parts, line, in_test in extract_crate_refs(tokens):
            target = path_parts[0]
            if target == src_mod or target in ROOT_CONSTANTS or target in allowed:
                continue
            item = path_parts[1] if len(path_parts) > 1 else None
            if (
                in_test
                and src_mod in TEST_FIXTURE_CONSUMERS
                and target == "interception"
                and item in TEST_FIXTURE_ITEMS
            ):
                continue
            rendered = "crate::" + "::".join(path_parts)
            allowed_list = ", ".join(sorted(allowed)) if allowed else "none"
            violations.append(
                Violation(
                    rs_file,
                    line,
                    f"forbidden dependency `{src_mod}` -> `{target}` via `{rendered}` "
                    f"(allowed domain deps for `{src_mod}`: {allowed_list})",
                )
            )

    return violations


def main(argv: list[str]) -> int:
    root = Path(argv[1]).resolve() if len(argv) > 1 else Path(__file__).resolve().parent.parent
    violations = check_repository(root)
    if violations:
        for v in violations:
            rel = v.path.relative_to(root) if v.path.is_relative_to(root) else v.path
            print(f"{rel}:{v.line}: {v.message}", file=sys.stderr)
        print(
            f"\nBoundary check failed with {len(violations)} violation(s).",
            file=sys.stderr,
        )
        return 1

    print(
        f"Boundary check passed ({len(ALLOWED_DEPS)} domain modules verified, 0 violations)."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
