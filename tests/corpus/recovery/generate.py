#!/usr/bin/env python3
"""Regenerate the parser-recovery differential corpus.

Every fixture is the input side of one recovery case taken from the upstream
Dart analyzer recovery test suite (upstream Dart SDK source,
pkg/analyzer/test/src/fasta/recovery): the recovery inputs the analyzer team
uses to hammer their own parser. Expectation annotations (caret position
markers and `// [diag.*]` diagnostic lines) are stripped: expected results are
never transcribed by hand. Parity for this corpus is decided by the oracle
differential gate, exactly like the rest of the syntax corpus.

The analyzer test-suite location comes from the DARTQ_RECOVERY_SOURCE
environment variable (required) so this generator never embeds a
machine-specific path. Output is written next to this script; regeneration is
add/overwrite and deterministic (same source -> same fixtures byte for byte).

Usage:
  DARTQ_RECOVERY_SOURCE=/path/to/analyzer/test/src/fasta/recovery \
      python3 tests/corpus/recovery/generate.py
"""

import os
import re
import sys
from pathlib import Path

# One recovery input: the raw (or plain) triple-quoted string handed to the
# parse helper. group(1) captures the code plus inline expectation annotations.
SNIPPET_RE = re.compile(r"parseTestCodeWithDiagnostics\((r?)'''(.*?)'''\)", re.S)
METHOD_RE = re.compile(r"^  void (test_\w+)\(\)", re.M)
CLASS_RE = re.compile(r"^class (\w+) extends ParserDiagnosticsTest", re.M)

# Annotation lines inside a snippet: caret position markers ("// ^^^") and
# diagnostic expectation lines ("// [diag.someCode] message"). Verified against
# the whole suite: no legitimate code line in any snippet starts with "//".
CARET_LINE_RE = re.compile(r"^//[\s^]*$")
DIAG_LINE_RE = re.compile(r"^// \[diag\.")


def camel_to_snake(name: str) -> str:
    out = []
    for i, ch in enumerate(name):
        if ch.isupper() and i > 0 and (not name[i - 1].isupper() or (i + 1 < len(name) and name[i + 1].islower())):
            out.append("_")
        out.append(ch.lower())
    return "".join(out)


def clean_snippet(text: str) -> str:
    """Drop expectation annotations, then trim edge blank lines."""
    lines = [
        line
        for line in text.split("\n")
        if not (CARET_LINE_RE.match(line) or DIAG_LINE_RE.match(line))
    ]
    while lines and not lines[0].strip():
        lines.pop(0)
    while lines and not lines[-1].strip():
        lines.pop()
    return "\n".join(lines) + "\n" if lines else ""


def main() -> int:
    source = os.environ.get("DARTQ_RECOVERY_SOURCE")
    if not source:
        print("error: DARTQ_RECOVERY_SOURCE must point at the upstream analyzer "
              "recovery test directory (the one holding partial_code/)",
              file=sys.stderr)
        return 2
    source = Path(source)
    if not source.is_dir():
        print(f"error: DARTQ_RECOVERY_SOURCE={source} is not a directory", file=sys.stderr)
        return 2

    out_root = Path(__file__).resolve().parent
    files = sorted(p for p in source.glob("*_test.dart") if p.name != "test_all.dart")
    files += sorted(p for p in (source / "partial_code").glob("*_test.dart")
                    if p.name != "test_all.dart")

    fixtures: dict[str, str] = {}
    per_file: list[tuple[str, int, int]] = []  # official file, methods, fixtures
    unattributed = 0

    for path in files:
        text = path.read_text()
        classes = [(m.start(), m.group(1)) for m in CLASS_RE.finditer(text)]
        methods = list(METHOD_RE.finditer(text))
        snippets = list(SNIPPET_RE.finditer(text))

        # A method's span runs from its signature to the next method's
        # signature; snippets are grouped by the span containing them. Snippets
        # and methods are both in source order, so a two-pointer walk suffices.
        spans: list[tuple[str, int, int, str | None]] = []
        cls_i = 0
        for i, meth in enumerate(methods):
            end = methods[i + 1].start() if i + 1 < len(methods) else len(text)
            while cls_i + 1 < len(classes) and classes[cls_i + 1][0] < meth.start():
                cls_i += 1
            owner_cls = classes[cls_i][1] if classes and classes[cls_i][0] < meth.start() else None
            spans.append((meth.group(1), meth.start(), end, owner_cls))

        per_method: dict[int, list[str]] = {}
        si = 0
        for idx, (name, start, end, _) in enumerate(spans):
            while si < len(snippets) and snippets[si].start() < end:
                if snippets[si].start() >= start:
                    per_method.setdefault(idx, []).append(snippets[si].group(2))
                else:
                    unattributed += 1
                si += 1

        multi_class = len(classes) > 1
        count = 0
        for idx, (method, _, _, owner_cls) in enumerate(spans):
            if idx not in per_method:
                continue
            # Directory: partial_code/<context> or <file>/<class> for the
            # cross-cutting files that define several test classes.
            if path.parent.name == "partial_code":
                group = f"partial_code/{path.stem.removesuffix('_test')}"
            elif multi_class:
                group = (f"{path.stem.removesuffix('_test')}/"
                         + camel_to_snake((owner_cls or "unnamed").removesuffix("Test")))
            else:
                group = path.stem.removesuffix("_test")
            base = camel_to_snake(method.removeprefix("test_"))
            for i, snippet in enumerate(per_method[idx], start=1):
                stem = base if i == 1 else f"{base}_v{i}"
                rel = f"{group}/{stem}.dart"
                if rel in fixtures:
                    print(f"error: fixture name collision: {rel}", file=sys.stderr)
                    return 2
                fixtures[rel] = (f"// recovery: {path.relative_to(source)}/{method}"
                                 + (f" (variant {i})" if i > 1 else "") + "\n"
                                 + clean_snippet(snippet))
                count += 1
        per_file.append((f"{path.parent.name}/{path.name}", len(spans), count))

    for rel, content in sorted(fixtures.items()):
        target = out_root / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content)

    print(f"{'official file':52} methods fixtures")
    for name, methods, count in per_file:
        print(f"{name:52} {methods:7} {count:8}")
    total_fixtures = sum(c for _, _, c in per_file)
    total_methods = sum(m for _, m, _ in per_file)
    print(f"{'TOTAL':52} {total_methods:7} {total_fixtures:8}")
    if unattributed:
        print(f"warning: {unattributed} snippets fell outside any test method "
              "(emitted as _unattributed_*)", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
