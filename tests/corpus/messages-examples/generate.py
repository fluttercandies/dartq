#!/usr/bin/env python3
"""Regenerate the messages.yaml per-code example corpus.

Every fixture is one example snippet transcribed from the upstream Dart
analyzer's diagnostic documentation (upstream Dart SDK source,
pkg/analyzer/messages.yaml and pkg/linter/messages.yaml). The upstream
verify_diagnostics_test enforces that every documented example produces
exactly its own diagnostic code, so these snippets are per-code reproducer
inputs, never hand-written expectations.

The two documentation files come from required environment variables so this
generator never embeds a machine-specific path. Output is written next to
this script; regeneration is add/overwrite and deterministic (same sources ->
same fixtures byte for byte). Fixtures removed upstream must be pruned by
hand.

Usage:
  DARTQ_ANALYZER_MESSAGES=/path/to/analyzer/messages.yaml \
  DARTQ_LINTER_MESSAGES=/path/to/linter/messages.yaml \
      python3 tests/corpus/messages-examples/generate.py
"""

import os
import sys
from pathlib import Path

try:
    import yaml
except ImportError:  # pragma: no cover - reported as a tutorial error below
    yaml = None

# Directives understood by the upstream documentation parser
# (upstream Dart SDK source, pkg/analyzer/tool/messages/
# error_code_documentation_info.dart). They are doc-tooling directives, not
# lexable Dart, so they are stripped from fixtures.
DIRECTIVE_PREFIXES = ("%language=", '%uri="', "%ignore=", "%experiments=")

# Upstream marker pair delimiting the expected error range inside a snippet.
# The upstream verifier removes both sequences before analyzing; we do the
# same so a fixture is pure input data with no expectation baked in.
MARKER_START = "[!"
MARKER_END = "!]"

# Section headers recognized by the upstream parser. Only these carry
# reproducer inputs; "Common fixes" blocks produce no diagnostic by design.
EXAMPLE_SECTIONS = ("#### Example", "#### Examples")

SOURCES = [
    ("analyzer", "DARTQ_ANALYZER_MESSAGES"),
    ("linter", "DARTQ_LINTER_MESSAGES"),
]


def camel_to_snake(name: str) -> str:
    out = []
    for i, ch in enumerate(name):
        if ch.isupper() and i > 0 and (not name[i - 1].isupper() or (i + 1 < len(name) and name[i + 1].islower())):
            out.append("_")
        out.append(ch.lower())
    return "".join(out)


def parse_blocks(doc: str) -> list[tuple[str | None, str, list[str], str | None]]:
    """Mirror the upstream documentation block parser.

    Returns (section, filetype, code lines, uri) per fenced block, in document
    order. Mirrors: sections switch on non-##### headers, parsing stops at a
    line starting with TODO, and a line starting with % inside a block must be
    a known directive.
    """
    blocks: list[tuple[str | None, str, list[str], str | None]] = []
    lines = doc.split("\n")
    section: str | None = None
    i = 0
    while i < len(lines):
        line = lines[i]
        if line.startswith("TODO"):
            break
        if line.startswith("```"):
            filetype = line[3:]
            i += 1
            code: list[str] = []
            uri: str | None = None
            while i < len(lines) and not lines[i].startswith("```"):
                l = lines[i]
                if l.startswith("%"):
                    # Directive lines are dropped from the block text.
                    if not l.startswith(DIRECTIVE_PREFIXES):
                        print(f"warning: unrecognized directive {l!r}", file=sys.stderr)
                    elif l.startswith('%uri="'):
                        if not l.endswith('"'):
                            print(f"warning: malformed uri directive {l!r}", file=sys.stderr)
                        uri = l[len('%uri="'):-1]
                else:
                    code.append(l)
                i += 1
            if i < len(lines):
                i += 1  # closing fence
            blocks.append((section, filetype, code, uri))
        else:
            if line.startswith("#") and not line.startswith("#####"):
                section = line
            i += 1
    return blocks


def extract_snippets(doc: str, qualified: str) -> tuple[list[str], list[str]]:
    """Return (standalone dart example snippet texts, anomaly notes)."""
    snippets: list[str] = []
    anomalies: list[str] = []
    for section, filetype, code, uri in parse_blocks(doc):
        if section not in EXAMPLE_SECTIONS or filetype != "dart" or uri is not None:
            continue
        text = "\n".join(code)
        starts = text.count(MARKER_START)
        ends = text.count(MARKER_END)
        if starts != 1 or ends != 1:
            # Upstream tolerates these only in codes exempted from semantic
            # verification; both shapes are still usable pure inputs.
            anomalies.append(f"{qualified}: {starts} range starts / {ends} "
                             "range ends in one example block")
        text = text.replace(MARKER_START, "").replace(MARKER_END, "")
        if not text.strip():
            anomalies.append(f"{qualified}: example block is empty after stripping")
            continue
        snippets.append(text)
    return snippets, anomalies


def process_source(name: str, env_var: str, out_root: Path) -> tuple[int, list[str]]:
    path_value = os.environ.get(env_var)
    if not path_value:
        print(f"error: {env_var} must point at the upstream {name} "
              "messages.yaml (see README.md)", file=sys.stderr)
        raise SystemExit(2)
    path = Path(path_value)
    if not path.is_file():
        print(f"error: {env_var}={path} is not a file", file=sys.stderr)
        raise SystemExit(2)
    data = yaml.safe_load(path.read_text())

    fixtures: dict[str, str] = {}
    census: list[str] = []
    no_doc: list[str] = []
    no_snippet: list[str] = []
    removed: list[str] = []
    renamed: list[str] = []
    per_class: dict[str, tuple[int, int]] = {}
    anomalies: list[str] = []

    for cls, entries in sorted(data.items()):
        codes = fixtures_from_class = 0
        for code, entry in sorted(entries.items()):
            codes += 1
            qualified = f"{cls}.{code}"
            state = entry.get("state") or {}
            if "removed" in state:
                removed.append(qualified)
                continue
            doc = entry.get("documentation")
            if doc is None:
                no_doc.append(qualified)
                continue
            snippets, bad = extract_snippets(doc, qualified)
            anomalies += bad
            if not snippets:
                no_snippet.append(qualified)
                continue
            stem = camel_to_snake(code)
            emitted = camel_to_snake(entry.get("sharedName") or code)
            if emitted != stem:
                renamed.append(f"{qualified} -> {emitted}")
            for i, snippet in enumerate(snippets, start=1):
                rel_stem = stem if i == 1 else f"{stem}_v{i}"
                rel = f"{name}/{rel_stem}.dart"
                if rel in fixtures:
                    print(f"error: fixture name collision: {rel}", file=sys.stderr)
                    raise SystemExit(2)
                provenance = (f"// messages-example: {name}/messages.yaml {qualified}"
                              + (f" (variant {i})" if i > 1 else ""))
                fixtures[rel] = provenance + "\n" + snippet + "\n"
            fixtures_from_class += len(snippets)
        per_class[cls] = (codes, fixtures_from_class)

    for rel, content in sorted(fixtures.items()):
        target = out_root / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content)

    census.append(f"source: {name}/messages.yaml")
    for cls, (codes, count) in per_class.items():
        census.append(f"  {cls}: {codes} codes, {count} fixtures")
    total_codes = sum(c for c, _ in per_class.values())
    total_fixtures = sum(f for _, f in per_class.values())
    census.append(f"  total: {total_codes} codes, {total_fixtures} fixtures")
    census.append(f"  removed codes skipped ({len(removed)}):")
    census += [f"    {q}" for q in removed]
    census.append(f"  codes without documentation ({len(no_doc)}):")
    census += [f"    {q}" for q in no_doc]
    census.append("  documented codes without a standalone dart example "
                  f"({len(no_snippet)}; examples are yaml or uri-auxiliary blocks):")
    census += [f"    {q}" for q in no_snippet]
    if renamed:
        census.append("  codes whose emitted diagnostic name differs from the "
                      "fixture stem (sharedName):")
        census += [f"    {q}" for q in renamed]
    if anomalies:
        census.append(f"  blocks with marker anomalies ({len(anomalies)}):")
        census += [f"    {a}" for a in anomalies]
    census.append("")

    print(f"{name}: {total_codes} codes, {total_fixtures} fixtures, "
          f"{len(no_doc)} without documentation, {len(no_snippet)} without "
          f"standalone dart example, {len(removed)} removed, "
          f"{len(anomalies)} marker anomalies")
    return total_fixtures, census


def main() -> int:
    if yaml is None:
        print("error: PyYAML is required (python3 -m pip install pyyaml)",
              file=sys.stderr)
        return 2
    out_root = Path(__file__).resolve().parent
    all_census: list[str] = [
        "# Generated by generate.py; do not edit.",
        "# Per-code census of the messages.yaml example extraction.",
        "# Fixture stems are the yaml keys converted to snake_case; the",
        "# emitted diagnostic name is snake_case of (sharedName or key).",
        "",
    ]
    total = 0
    for name, env_var in SOURCES:
        count, lines = process_source(name, env_var, out_root)
        total += count
        all_census += lines
    all_census.append(f"total fixtures: {total}")
    (out_root / "census.txt").write_text("\n".join(all_census))
    return 0


if __name__ == "__main__":
    sys.exit(main())
