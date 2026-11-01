#!/usr/bin/env python3
"""Keep a Rust test-module registry in sync with its directory.

A registry file (for example `crates/dartq-semantic/src/diagnose/tests.rs`)
tail-declares sibling test modules as

    #[cfg(test)]
    #[path = "ticket_3519_tests.rs"]
    mod ticket_3519_tests;

Hand-maintained tails drift: one lane appends its block, a stale
read-modify-write pass silently drops another lane's block, and the module
file stays on disk mounted by nobody -- its tests never compile and never
run, with no signal at all. Eight parallel lanes produced exactly that.

This tool makes the tail a pure function of the directory: it appends one
block per `*_tests.rs` file in the registry's own directory that nothing in
the crate mounts, in ascending ticket-number order (non-ticket stems sort
after ticket stems, lexicographically). Existing blocks are never touched,
reordered, or removed.

A module counts as mounted when the crate's source tree declares it, by
name (`mod NAME;`) or by a resolved `#[path]` target -- so a module already
mounted from a sibling registry (`diagnose.rs` next to `diagnose/tests.rs`)
is not declared a second time.

Usage:
    scripts/sync-test-registry.py <registry.rs> [--check]

    --check   report drift without writing: exit 1 when a module file is
              unmounted or a registration points at a missing file. Never
              writes.

Exit codes: 0 in sync - 1 drift (--check) - 2 usage or environment refusal.
Stdlib only by design: the gate that calls this must not need a venv.
"""

import argparse
import re
import sys
from pathlib import Path

MOD_ITEM_RE = re.compile(r"^[ \t]*(?:pub(?:[ \t]*\([^)]*\))?[ \t]+)?mod[ \t]+([A-Za-z_][A-Za-z_0-9]*)[ \t]*;")
MOD_DECL_RE = re.compile(
    r"^[ \t]*(?:pub(?:[ \t]*\([^)]*\))?[ \t]+)?mod[ \t]+([A-Za-z_][A-Za-z_0-9]*)[ \t]*[;{]",
    re.M,
)
PATH_ATTR_RE = re.compile(r'#\[path[ \t]*=[ \t]*"([^"]+)"[ \t]*\]')
TICKET_RE = re.compile(r"ticket_(\d+)_tests")
BLOCK_COMMENT_RE = re.compile(r"/\*.*?\*/", re.S)


class Refusal(Exception):
    """The registry's shape changed under us: writing would corrupt it."""


def strip_comments(text):
    """Drop line and block comments so comment text never looks like a mod."""
    text = BLOCK_COMMENT_RE.sub("", text)
    return "\n".join(line.split("//", 1)[0] for line in text.splitlines())


def find_crate_root(registry):
    """Nearest ancestor holding Cargo.toml; its src/ is the module tree."""
    for parent in registry.resolve().parents:
        if (parent / "Cargo.toml").is_file():
            src = parent / "src"
            if not src.is_dir():
                raise Refusal(f"{parent} has a Cargo.toml but no src/ directory")
            return src
    raise Refusal(f"no Cargo.toml above {registry}: cannot locate the crate source root")


def crate_mounts(src_root):
    """Module names and resolved #[path] targets declared anywhere in the crate."""
    names, targets = set(), set()
    for path in src_root.rglob("*.rs"):
        code = strip_comments(path.read_text(encoding="utf-8", errors="replace"))
        names.update(MOD_DECL_RE.findall(code))
        for attr in PATH_ATTR_RE.findall(code):
            target = Path(attr) if Path(attr).is_absolute() else path.parent / attr
            targets.add(str(target.resolve()))
    return names, targets


def registrations(lines):
    """Yield (line number, module name, #[path] target or None) per `mod NAME;`."""
    pending = None
    for number, line in enumerate(lines, 1):
        attr = PATH_ATTR_RE.search(line)
        if attr:
            pending = attr.group(1)
            continue
        item = MOD_ITEM_RE.match(line)
        if item:
            yield number, item.group(1), pending
            pending = None
            continue
        stripped = line.strip()
        if stripped and not stripped.startswith("//") and not stripped.startswith("#["):
            pending = None


def sort_key(path):
    """Ascending ticket number first, then bare stems lexicographically."""
    match = TICKET_RE.fullmatch(path.stem)
    if match:
        return (0, int(match.group(1)), path.name)
    return (1, 0, path.name)


def block(path):
    return f'#[cfg(test)]\n#[path = "{path.name}"]\nmod {path.stem};\n'


def main(argv):
    parser = argparse.ArgumentParser(
        description="Append missing test-module registrations to a Rust test registry.",
    )
    parser.add_argument("registry", help="registry .rs file (for example .../diagnose/tests.rs)")
    parser.add_argument(
        "--check",
        action="store_true",
        help="report drift and exit 1 instead of appending registrations",
    )
    args = parser.parse_args(argv)

    registry = Path(args.registry)
    try:
        if not registry.is_file():
            raise Refusal(f"{registry}: no such file")
        text = registry.read_text(encoding="utf-8")
        lines = text.splitlines()
        last = next((line for line in reversed(lines) if line.strip()), "")
        if not MOD_ITEM_RE.match(last):
            raise Refusal(
                f"{registry}: last non-blank line is not a `mod NAME;` declaration\n"
                f"  got: {last.strip()!r}\n"
                "  the registry's shape changed; fix it by hand rather than letting\n"
                "  this tool append past an arbitrary tail"
            )
        src_root = find_crate_root(registry)
        if not str(registry.resolve()).startswith(str(src_root.resolve()) + "/"):
            raise Refusal(
                f"{registry}: not inside the crate's src/ tree ({src_root})\n"
                "  only source-tree registries use `#[path]` test modules; a file under\n"
                "  tests/ is an integration target Cargo discovers by itself"
            )
        names, targets = crate_mounts(src_root)
    except Refusal as refusal:
        print(f"sync-test-registry: {refusal}", file=sys.stderr)
        return 2

    found = list(registrations(lines))
    dangling = []
    for number, name, target in found:
        if target is None:
            continue
        resolved = Path(target) if Path(target).is_absolute() else registry.parent / target
        if not resolved.is_file():
            dangling.append((number, name, target, resolved))

    missing = [
        path
        for path in sorted(registry.parent.glob("*_tests.rs"), key=sort_key)
        if path != registry
        and path.stem not in names
        and str(path.resolve()) not in targets
    ]

    if args.check:
        for path in missing:
            print(f"missing registration: {path} (module `{path.stem}` is mounted nowhere in the crate)")
        for number, name, target, resolved in dangling:
            print(f"dangling registration: {registry}:{number} `mod {name};` -> {target} ({resolved}: no such file)")
        if missing or dangling:
            print(
                f"sync-test-registry: FAIL -- {len(missing)} missing, {len(dangling)} dangling"
                f" ({len(found)} registrations)"
            )
            return 1
        print(f"sync-test-registry: in sync ({len(found)} registrations)")
        return 0

    if not missing:
        print(f"sync-test-registry: unchanged ({len(found)} registrations)")
        return 0

    if not text.endswith("\n"):
        text += "\n"
    text += "".join("\n" + block(path) for path in missing)
    registry.write_text(text, encoding="utf-8")
    print(f"sync-test-registry: appended {len(missing)} registration(s) to {registry}:")
    for path in missing:
        print(f"  {path.name}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
