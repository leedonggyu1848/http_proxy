"""Check workspace crate dependency direction, intra-crate module boundaries, and write-handle ownership.

This script enforces the architectural invariants that Cargo's acyclic graph
alone does not guarantee:
1. Workspace crate dependency direction (`ALLOWED_WORKSPACE_DEPS`): each domain
   crate's `[dependencies]` (including `target.*.dependencies` and
   `build-dependencies`) may only depend on explicitly allowed workspace crates,
   blocking reverse dependencies and transitive pulls of `http-inspector-core`.
2. Runtime and UI isolation: runtime-independent crates (`http-inspector-encoding`, `http-inspector-exchange`,
   `http-inspector-content`, `http-inspector-analysis`, `http-inspector-interop`,
   `http-inspector-file`)
   must not depend on `tokio`, `hyper`, `hyper-util`, `eframe`, `egui`, or
   `egui_extras` in normal dependencies or `dev-dependencies` (nor on
   `http-inspector-core` in `dev-dependencies`, allowing only self
   `dev-dependencies` such as `http-inspector-exchange` enabling `test-support`);
   `http-inspector-core` must not depend on UI crates. Dependency aliases via
   `package = "..."` (direct or workspace-inherited) are resolved to the real
   package name.
3. Multi-module crates (`http-inspector-exchange`, `http-inspector-core`) keep
   their internal module dependency direction (`crate::`, `<crate>::`, `self::`,
   `super::` normalized against the file's module path, including inline `mod`
   blocks, and crate-root aliases including `extern crate self` rejected).
4. `Exchange` write handles (`ExchangeWriter`, `BodyPreviewWriter`,
   `TunnelWriter`) may be used in production code only inside
   `http-inspector-exchange` and `http-inspector-core`'s `interception` and
   `replay` modules.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import sys
import tomllib

UI_CRATES: frozenset[str] = frozenset({"eframe", "egui", "egui_extras"})
RUNTIME_CRATES: frozenset[str] = frozenset({"tokio", "hyper", "hyper-util"})

ALLOWED_WORKSPACE_DEPS: dict[str, frozenset[str]] = {
    "http-inspector-encoding": frozenset(),
    "http-inspector-exchange": frozenset(),
    "http-inspector-file": frozenset(),
    "http-inspector-content": frozenset({"http-inspector-encoding", "http-inspector-exchange"}),
    "http-inspector-analysis": frozenset(
        {"http-inspector-exchange", "http-inspector-content"}
    ),
    "http-inspector-interop": frozenset(
        {
            "http-inspector-encoding",
            "http-inspector-exchange",
            "http-inspector-content",
            "http-inspector-analysis",
            "http-inspector-file",
        }
    ),
    "http-inspector-core": frozenset({"http-inspector-encoding", "http-inspector-exchange", "http-inspector-file"}),
}

FORBIDDEN_NORMAL_DEPS: dict[str, frozenset[str]] = {
    "http-inspector-encoding": UI_CRATES | RUNTIME_CRATES,
    "http-inspector-exchange": UI_CRATES | RUNTIME_CRATES,
    "http-inspector-file": UI_CRATES | RUNTIME_CRATES,
    "http-inspector-content": UI_CRATES | RUNTIME_CRATES,
    "http-inspector-analysis": UI_CRATES | RUNTIME_CRATES,
    "http-inspector-interop": UI_CRATES | RUNTIME_CRATES,
    "http-inspector-core": UI_CRATES,
}

FORBIDDEN_DEV_DEPS: dict[str, frozenset[str]] = {
    "http-inspector-encoding": UI_CRATES | RUNTIME_CRATES | frozenset({"http-inspector-core"}),
    "http-inspector-exchange": (
        UI_CRATES | RUNTIME_CRATES | frozenset({"http-inspector-core"})
    ),
    "http-inspector-file": (
        UI_CRATES | RUNTIME_CRATES | frozenset({"http-inspector-core"})
    ),
    "http-inspector-content": (
        UI_CRATES | RUNTIME_CRATES | frozenset({"http-inspector-core"})
    ),
    "http-inspector-analysis": (
        UI_CRATES | RUNTIME_CRATES | frozenset({"http-inspector-core"})
    ),
    "http-inspector-interop": (
        UI_CRATES | RUNTIME_CRATES | frozenset({"http-inspector-core"})
    ),
    "http-inspector-core": UI_CRATES,
}

WRITER_HANDLES: frozenset[str] = frozenset(
    {"ExchangeWriter", "BodyPreviewWriter", "TunnelWriter"}
)


@dataclass(frozen=True)
class CrateBoundarySpec:
    crate_dir: str
    crate_ident: str
    allowed_deps: dict[str, frozenset[str]]
    root_constants: frozenset[str]
    writer_allowed_modules: frozenset[str]


# Internal module dependency rules for multi-module workspace crates.
# Every top-level module in each listed crate must be classified here.
INTRA_CRATE_SPECS: tuple[CrateBoundarySpec, ...] = (
    CrateBoundarySpec(
        crate_dir="crates/http-inspector-exchange",
        crate_ident="http_inspector_exchange",
        allowed_deps={
            "exchange": frozenset(),
            "http_semantics": frozenset(),
            "capture": frozenset({"exchange", "http_semantics"}),
            "exchange_builder": frozenset({"exchange", "capture"}),
        },
        root_constants=frozenset({"APP_NAME"}),
        writer_allowed_modules=frozenset(
            {"exchange", "capture", "exchange_builder"}
        ),
    ),
    CrateBoundarySpec(
        crate_dir="crates/http-inspector-core",
        crate_ident="http_inspector_core",
        allowed_deps={
            "settings": frozenset(),
            "https": frozenset({"settings"}),
            "launcher": frozenset({"https", "settings"}),
            "recording": frozenset(),
            "hls": frozenset({"https", "recording"}),
            "interception": frozenset({"https", "recording"}),
            "replay": frozenset({"interception"}),
        },
        root_constants=frozenset({"DEFAULT_PORT"}),
        writer_allowed_modules=frozenset({"interception", "replay"}),
    ),
)

# Crates whose production code must never reference `Exchange` write handles.
NO_WRITER_CRATE_SRCS: tuple[str, ...] = (
    "crates/http-inspector-encoding/src",
    "crates/http-inspector-file/src",
    "crates/http-inspector-content/src",
    "crates/http-inspector-analysis/src",
    "crates/http-inspector-interop/src",
    "src",
)


@dataclass(frozen=True)
class Token:
    kind: str  # "IDENT" or "PUNCT"
    text: str
    line: int
    in_test: bool = False
    mod_path: tuple[str, ...] = ()


@dataclass(frozen=True)
class CrateRef:
    resolved: list[str]
    raw: str
    line: int
    in_test: bool


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
    """Check if `tokens[start:]` is an outer `#[cfg(...test...)]` attribute."""
    n = len(tokens)
    if start + 1 >= n or tokens[start].text != "#" or tokens[start + 1].text != "[":
        return False, start
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


def module_path_for(src_dir: Path, rs_file: Path) -> tuple[str, ...]:
    """Return the Rust module path segments for `rs_file` relative to `src_dir`."""
    rel = rs_file.relative_to(src_dir)
    parts = list(rel.parts)
    if parts == ["lib.rs"] or parts == ["main.rs"]:
        return ()
    if parts[-1] == "mod.rs":
        return tuple(parts[:-1])
    parts[-1] = parts[-1].removesuffix(".rs")
    return tuple(parts)


def annotate_scopes(
    tokens: list[Token],
    whole_file_is_test: bool,
    file_mod_path: tuple[str, ...] = (),
) -> list[Token]:
    """Mark every token with its `#[cfg(test)]` scope and effective module path."""
    annotated: list[Token] = []
    n = len(tokens)
    i = 0
    brace_depth = 0
    pending_test = False
    test_enter_depth: int | None = None
    pending_mod_name: str | None = None
    mod_stack: list[tuple[str, int]] = []

    while i < n:
        tok = tokens[i]
        in_test_now = whole_file_is_test or test_enter_depth is not None or pending_test
        cur_mod = file_mod_path + tuple(name for name, _ in mod_stack)

        if tok.text == "#" and i + 1 < n and tokens[i + 1].text == "[":
            is_cfg_test, next_i = _is_cfg_test_attr(tokens, i)
            if not whole_file_is_test and is_cfg_test and test_enter_depth is None:
                pending_test = True
                in_test_now = True
            for attr_tok in tokens[i:next_i]:
                annotated.append(
                    Token(
                        attr_tok.kind,
                        attr_tok.text,
                        attr_tok.line,
                        in_test_now,
                        cur_mod,
                    )
                )
            i = next_i
            continue

        if (
            tok.kind == "IDENT"
            and tok.text == "mod"
            and i + 2 < n
            and tokens[i + 1].kind == "IDENT"
            and tokens[i + 2].text == "{"
        ):
            pending_mod_name = tokens[i + 1].text

        if tok.text == "{":
            if not whole_file_is_test and pending_test and test_enter_depth is None:
                test_enter_depth = brace_depth
                pending_test = False
                in_test_now = True
            if pending_mod_name is not None:
                mod_stack.append((pending_mod_name, brace_depth))
                pending_mod_name = None
                cur_mod = file_mod_path + tuple(name for name, _ in mod_stack)
            brace_depth += 1
            annotated.append(
                Token(tok.kind, tok.text, tok.line, in_test_now, cur_mod)
            )
            i += 1
            continue

        if tok.text == "}":
            brace_depth = max(0, brace_depth - 1)
            annotated.append(
                Token(tok.kind, tok.text, tok.line, in_test_now, cur_mod)
            )
            if (
                not whole_file_is_test
                and test_enter_depth is not None
                and brace_depth <= test_enter_depth
            ):
                test_enter_depth = None
            while mod_stack and brace_depth <= mod_stack[-1][1]:
                mod_stack.pop()
            i += 1
            continue

        if tok.text == ";" and pending_test:
            annotated.append(Token(tok.kind, tok.text, tok.line, True, cur_mod))
            pending_test = False
            i += 1
            continue

        annotated.append(Token(tok.kind, tok.text, tok.line, in_test_now, cur_mod))
        i += 1

    return annotated


def discover_cfg_test_files(src_dir: Path) -> set[Path]:
    """Find files loaded via `#[cfg(test)] mod <name>;`."""
    test_files: set[Path] = set()
    for rs_file in sorted(src_dir.rglob("*.rs")):
        tokens = annotate_scopes(
            tokenize_rust(rs_file.read_text()),
            False,
            module_path_for(src_dir, rs_file),
        )
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
) -> tuple[list[tuple[list[str], int, bool, tuple[str, ...]]], int]:
    """Parse a Rust `use` tree starting at `pos`, returning `(paths, next_pos)`."""
    results: list[tuple[list[str], int, bool, tuple[str, ...]]] = []
    n = len(tokens)
    current = list(prefix)
    line = tokens[pos].line if pos < n else 0
    in_test = tokens[pos].in_test if pos < n else False
    mod_path = tokens[pos].mod_path if pos < n else ()

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
                results.append((current, line, in_test, mod_path))
            return results, pos
        if tok.kind == "IDENT" and tok.text == "as":
            if current:
                results.append((current, line, in_test, mod_path))
            pos += 2  # skip `as <alias>`
            return results, pos
        if tok.text == "::":
            pos += 1
            continue
        if tok.kind == "IDENT" or tok.text == "*":
            if not current:
                line = tok.line
                in_test = tok.in_test
                mod_path = tok.mod_path
            current.append(tok.text)
            pos += 1
            continue
        pos += 1

    if current:
        results.append((current, line, in_test, mod_path))
    return results, pos


def extract_extern_crate_self_aliases(tokens: list[Token]) -> list[CrateRef]:
    """Find `extern crate self as <name>;` declarations."""
    refs: list[CrateRef] = []
    n = len(tokens)
    i = 0
    while i + 4 < n:
        tok = tokens[i]
        if (
            tok.kind == "IDENT"
            and tok.text == "extern"
            and tokens[i + 1].kind == "IDENT"
            and tokens[i + 1].text == "crate"
            and tokens[i + 2].kind == "IDENT"
            and tokens[i + 2].text == "self"
            and tokens[i + 3].kind == "IDENT"
            and tokens[i + 3].text == "as"
            and tokens[i + 4].kind == "IDENT"
        ):
            raw = f"extern crate self as {tokens[i + 4].text}"
            refs.append(CrateRef([], raw, tok.line, tok.in_test))
            i += 5
            continue
        i += 1
    return refs


def normalize_crate_path(
    raw_path: list[str], cur_mod_path: tuple[str, ...], crate_ident: str
) -> list[str] | None:
    """Resolve `crate::`, `<crate_ident>::`, `self::`, or `super::` paths to crate-root segments."""
    if not raw_path:
        return None
    head = raw_path[0]
    if head in ("crate", crate_ident):
        resolved: list[str] = []
        segments = raw_path[1:]
    elif head in ("self", "super"):
        resolved = list(cur_mod_path)
        segments = raw_path
    else:
        return None

    for seg in segments:
        if seg == "self":
            continue
        if seg == "super":
            if resolved:
                resolved.pop()
            continue
        resolved.append(seg)
    return resolved


def extract_crate_refs(tokens: list[Token], crate_ident: str) -> list[CrateRef]:
    """Extract and normalize all intra-crate paths from `use` items and inline paths."""
    refs: list[CrateRef] = []
    n = len(tokens)
    i = 0
    roots = ("crate", crate_ident, "self", "super")
    while i < n:
        tok = tokens[i]
        if tok.kind == "IDENT" and tok.text == "use":
            paths, next_i = _parse_use_tree(tokens, i + 1, [])
            for path, line, in_test, mod_path in paths:
                resolved = normalize_crate_path(path, mod_path, crate_ident)
                if resolved is not None:
                    refs.append(CrateRef(resolved, "::".join(path), line, in_test))
            i = next_i + 1
            continue

        if (
            tok.kind == "IDENT"
            and tok.text in roots
            and (i == 0 or tokens[i - 1].text != "::")
            and i + 2 < n
            and tokens[i + 1].text == "::"
            and tokens[i + 2].kind == "IDENT"
        ):
            raw = [tok.text, tokens[i + 2].text]
            j = i + 3
            while j + 1 < n and tokens[j].text == "::" and tokens[j + 1].kind == "IDENT":
                raw.append(tokens[j + 1].text)
                j += 2
            resolved = normalize_crate_path(raw, tok.mod_path, crate_ident)
            if resolved is not None:
                refs.append(CrateRef(resolved, "::".join(raw), tok.line, tok.in_test))
            i = j
            continue

        i += 1
    return refs


def top_level_module(src_dir: Path, path: Path) -> str | None:
    rel = path.relative_to(src_dir)
    first = rel.parts[0]
    if first in ("lib.rs", "main.rs"):
        return None
    return first.removesuffix(".rs")


def _resolve_package_name(
    dep_key: str, spec: object, workspace_deps: dict[str, object]
) -> str:
    """Resolve the real Cargo package name behind a dependency key and table spec."""
    if isinstance(spec, dict):
        pkg = spec.get("package")
        if spec.get("workspace") is True:
            ws_key = pkg if isinstance(pkg, str) else dep_key
            ws_spec = workspace_deps.get(ws_key)
            if isinstance(ws_spec, dict) and isinstance(ws_spec.get("package"), str):
                return ws_spec["package"]
            return ws_key
        if isinstance(pkg, str):
            return pkg
    return dep_key


def _collect_section_deps(
    manifest: dict[str, object],
    section: str,
    workspace_deps: dict[str, object],
) -> list[tuple[str, str]]:
    """Collect sorted unique `(dep_key, resolved_package)` pairs from `[section]` and `target.*.section`."""
    found: set[tuple[str, str]] = set()
    direct = manifest.get(section)
    if isinstance(direct, dict):
        for dep_key, spec in direct.items():
            found.add((dep_key, _resolve_package_name(dep_key, spec, workspace_deps)))
    targets = manifest.get("target")
    if isinstance(targets, dict):
        for target_cfg in targets.values():
            if not isinstance(target_cfg, dict):
                continue
            target_deps = target_cfg.get(section)
            if isinstance(target_deps, dict):
                for dep_key, spec in target_deps.items():
                    found.add(
                        (dep_key, _resolve_package_name(dep_key, spec, workspace_deps))
                    )
    return sorted(found, key=lambda item: (item[1], item[0]))


def _format_dep(dep_key: str, resolved: str) -> str:
    if dep_key == resolved:
        return f"`{resolved}`"
    return f"`{resolved}` (via alias `{dep_key}`)"


def check_manifest_deps(root: Path) -> list[Violation]:
    """Verify workspace dependency direction and runtime/UI isolation in crate manifests."""
    violations: list[Violation] = []
    workspace_deps: dict[str, object] = {}
    workspace_crates: set[str] = set(ALLOWED_WORKSPACE_DEPS.keys())

    root_manifest_path = root / "Cargo.toml"
    if root_manifest_path.exists():
        root_manifest = tomllib.loads(root_manifest_path.read_text())
        ws_table = root_manifest.get("workspace", {})
        if isinstance(ws_table, dict):
            ws_deps = ws_table.get("dependencies", {})
            if isinstance(ws_deps, dict):
                workspace_deps = ws_deps
        root_pkg = root_manifest.get("package", {})
        if isinstance(root_pkg, dict) and isinstance(root_pkg.get("name"), str):
            workspace_crates.add(root_pkg["name"])

    crates_dir = root / "crates"
    manifests: list[tuple[Path, dict[str, object], str]] = []
    for manifest_path in sorted(crates_dir.glob("*/Cargo.toml")):
        manifest = tomllib.loads(manifest_path.read_text())
        pkg_table = manifest.get("package", {})
        pkg_name = (
            pkg_table.get("name", manifest_path.parent.name)
            if isinstance(pkg_table, dict)
            else manifest_path.parent.name
        )
        workspace_crates.add(pkg_name)
        manifests.append((manifest_path, manifest, pkg_name))

    for manifest_path, manifest, pkg_name in manifests:
        if (
            pkg_name not in ALLOWED_WORKSPACE_DEPS
            or pkg_name not in FORBIDDEN_NORMAL_DEPS
            or pkg_name not in FORBIDDEN_DEV_DEPS
        ):
            violations.append(
                Violation(
                    manifest_path,
                    1,
                    f"unknown workspace crate `{pkg_name}`; classify it in scripts/check-boundaries.py",
                )
            )
            continue

        allowed_ws = ALLOWED_WORKSPACE_DEPS[pkg_name]
        forbidden_normal = FORBIDDEN_NORMAL_DEPS[pkg_name]
        allowed_ws_list = ", ".join(sorted(allowed_ws)) if allowed_ws else "none"

        for section in ("dependencies", "build-dependencies"):
            for dep_key, resolved in _collect_section_deps(
                manifest, section, workspace_deps
            ):
                if resolved in workspace_crates:
                    if resolved not in allowed_ws:
                        violations.append(
                            Violation(
                                manifest_path,
                                1,
                                f"crate `{pkg_name}` must not depend on workspace crate "
                                f"{_format_dep(dep_key, resolved)} in `{section}` "
                                f"(allowed workspace deps: {allowed_ws_list})",
                            )
                        )
                elif resolved in forbidden_normal:
                    violations.append(
                        Violation(
                            manifest_path,
                            1,
                            f"crate `{pkg_name}` must not have a `{section}` dependency on "
                            f"{_format_dep(dep_key, resolved)}",
                        )
                    )

        forbidden_dev = FORBIDDEN_DEV_DEPS[pkg_name]
        allowed_dev_ws = (allowed_ws | {pkg_name}) - forbidden_dev
        allowed_dev_ws_list = (
            ", ".join(sorted(allowed_dev_ws)) if allowed_dev_ws else "none"
        )
        for dep_key, resolved in _collect_section_deps(
            manifest, "dev-dependencies", workspace_deps
        ):
            if resolved in workspace_crates:
                if resolved not in allowed_dev_ws:
                    violations.append(
                        Violation(
                            manifest_path,
                            1,
                            f"crate `{pkg_name}` must not depend on workspace crate "
                            f"{_format_dep(dep_key, resolved)} in `dev-dependencies` "
                            f"(allowed workspace dev-deps: {allowed_dev_ws_list})",
                        )
                    )
            elif resolved in forbidden_dev:
                violations.append(
                    Violation(
                        manifest_path,
                        1,
                        f"crate `{pkg_name}` must not have a `dev-dependencies` dependency on "
                        f"{_format_dep(dep_key, resolved)}",
                    )
                )

    return violations


def check_writer_handles_forbidden(
    rs_file: Path, tokens: list[Token], scope_label: str
) -> list[Violation]:
    violations: list[Violation] = []
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
                    f"{scope_label} must not use write handle `{tok.text}` "
                    f"(allowed in production only in http-inspector-exchange and "
                    f"http-inspector-core::{{interception, replay}})",
                )
            )
    return violations


def check_intra_crate(root: Path, spec: CrateBoundarySpec) -> list[Violation]:
    src_dir = root / spec.crate_dir / "src"
    violations: list[Violation] = []

    for entry in sorted(src_dir.iterdir()):
        if entry.name == "lib.rs":
            continue
        mod_name = entry.name.removesuffix(".rs")
        if mod_name not in spec.allowed_deps:
            violations.append(
                Violation(
                    entry,
                    1,
                    f"unknown module `{mod_name}` in `{spec.crate_dir}`; "
                    f"classify it in scripts/check-boundaries.py",
                )
            )

    test_files = discover_cfg_test_files(src_dir)

    for rs_file in sorted(src_dir.rglob("*.rs")):
        whole_file_is_test = rs_file.resolve() in test_files
        tokens = annotate_scopes(
            tokenize_rust(rs_file.read_text()),
            whole_file_is_test,
            module_path_for(src_dir, rs_file),
        )
        src_mod = top_level_module(src_dir, rs_file)
        scope = (
            spec.crate_ident
            if src_mod is None
            else f"{spec.crate_ident}::{src_mod}"
        )
        for ref in extract_extern_crate_self_aliases(tokens):
            violations.append(
                Violation(
                    rs_file,
                    ref.line,
                    f"forbidden crate-root import or alias via `{ref.raw}` "
                    f"in `{scope}`",
                )
            )
        if src_mod is None or src_mod not in spec.allowed_deps:
            continue

        if src_mod not in spec.writer_allowed_modules:
            violations.extend(
                check_writer_handles_forbidden(
                    rs_file, tokens, f"module `{spec.crate_ident}::{src_mod}`"
                )
            )

        allowed = spec.allowed_deps[src_mod]
        for ref in extract_crate_refs(tokens, spec.crate_ident):
            if not ref.resolved:
                violations.append(
                    Violation(
                        rs_file,
                        ref.line,
                        f"forbidden crate-root import or alias via `{ref.raw}` "
                        f"in `{spec.crate_ident}::{src_mod}`",
                    )
                )
                continue
            target = ref.resolved[0]
            if target == src_mod or target in spec.root_constants or target in allowed:
                continue
            normalized = "crate::" + "::".join(ref.resolved)
            via = (
                f"`{normalized}`"
                if ref.raw == normalized
                else f"`{ref.raw}` (resolved to `{normalized}`)"
            )
            allowed_list = ", ".join(sorted(allowed)) if allowed else "none"
            violations.append(
                Violation(
                    rs_file,
                    ref.line,
                    f"forbidden intra-crate dependency `{spec.crate_ident}::{src_mod}` -> `{target}` via {via} "
                    f"(allowed intra-crate deps: {allowed_list})",
                )
            )

    return violations


def check_repository(root: Path) -> list[Violation]:
    violations: list[Violation] = []
    violations.extend(check_manifest_deps(root))

    for spec in INTRA_CRATE_SPECS:
        violations.extend(check_intra_crate(root, spec))

    for rel_src in NO_WRITER_CRATE_SRCS:
        src_dir = root / rel_src
        if not src_dir.exists():
            continue
        test_files = discover_cfg_test_files(src_dir)
        for rs_file in sorted(src_dir.rglob("*.rs")):
            whole_file_is_test = rs_file.resolve() in test_files
            tokens = annotate_scopes(
                tokenize_rust(rs_file.read_text()),
                whole_file_is_test,
                module_path_for(src_dir, rs_file),
            )
            violations.extend(
                check_writer_handles_forbidden(rs_file, tokens, f"`{rel_src}`")
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
        f"Boundary check passed ({len(FORBIDDEN_NORMAL_DEPS)} workspace crates and "
        f"{len(INTRA_CRATE_SPECS)} intra-crate module graphs verified, 0 violations)."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
