# messages-examples corpus

Per-code differential corpus transcribed from the official diagnostic
documentation itself (upstream Dart SDK source, `pkg/analyzer/messages.yaml`
and `pkg/linter/messages.yaml`): every documented code carries one or more
`#### Example` code blocks, and the upstream `verify_diagnostics_test`
enforces that each example, analyzed on its own, produces exactly that one
diagnostic code. Each fixture is therefore a documented reproducer input for
one diagnostic code — an independent behavioral evidence face for the
diagnostic registry 1:1 reconciliation (the registry face compares code
lists; this corpus exercises that each code's own documented example really
triggers it).

The name reflects the sources: the example blocks of the two upstream
messages.yaml files, one fixture per code (plus `_v2`, `_v3`, ... when a
code documents several examples, in document order).

## Layout

```
analyzer/<code>.dart     analyzer-side codes, fixture stem = yaml key in
                         snake_case (no collisions across the 13 classes)
linter/<code>.dart       linter codes (class LinterLintCode), same rule
census.txt               regenerated per-code census: counts, codes without
                         documentation, codes whose examples are yaml or
                         uri-auxiliary blocks only, removed codes, sharedName
                         mappings, marker anomalies
```

Every fixture's first line records its provenance (source file + yaml class
and key, plus the variant index):

```dart
// messages-example: analyzer/messages.yaml CompileTimeErrorCode.abstractMember
```

Fixture stems are the yaml keys, but the emitted diagnostic name is the
snake_case of `(sharedName ?? key)`; codes where the two differ are listed in
census.txt (e.g. `LinterLintCode.alwaysDeclareReturnTypesOfFunctions` emits
`always_declare_return_types`).

## Extraction contract

Blocks are located exactly the way the upstream documentation parser
(`pkg/analyzer/tool/messages/error_code_documentation_info.dart`) does it:
sections switch on non-`#####` headers, parsing stops at a `TODO` line, and
only fenced blocks under `#### Example` / `#### Examples` count. A fixture is
a pure input; nothing about expected results is baked in:

- Only dart-type blocks without a `%uri="..."` directive become fixtures.
  `%uri` blocks are auxiliary files for neighboring snippets, not standalone
  inputs; codes whose examples are exclusively yaml or uri-auxiliary blocks
  are listed in census.txt instead.
- Error-range markers `[!` and `!]` are removed. The upstream verifier
  removes the same sequences before analyzing: they delimit the expected
  error range, i.e. an expectation, not code.
- `%`-directive lines (`%language=`, `%ignore=`, `%experiments=`, `%uri=`)
  are removed: they are doc-tooling directives, not lexable Dart. (Same
  rationale as the recovery corpus's annotation stripping; here the stripped
  lines are not even comments.)
- `// ignore:`-style comment lines are kept verbatim. Unlike the `%`
  directives they are lexically valid Dart and part of the documented
  reproducer itself — the upstream verifier analyzes them too (one code's
  example is an ignore comment), and stripping them would alter analyzer
  behavior rather than just remove expectations. No `// expect_lint:`
  annotations exist in either source file.
- `#### Common fixes` blocks are intentionally not extracted: they are the
  corrected counterparts and produce no diagnostic by construction.
- Linter codes whose state is `removed` are skipped (14), mirroring the
  upstream verifier's `isRemoved` filter; they are listed in census.txt.

## Regeneration

`generate.py` rebuilds the whole tree deterministically (same sources ->
byte-identical fixtures). Both upstream files are required environment
variables; nothing machine-specific is embedded. Requires PyYAML.

```sh
DARTQ_ANALYZER_MESSAGES=/path/to/analyzer/messages.yaml \
DARTQ_LINTER_MESSAGES=/path/to/linter/messages.yaml \
    python3 tests/corpus/messages-examples/generate.py
```

Current census: 876 fixtures (650 analyzer, 226 linter) from 1315 live codes;
600 + 199 codes carry documentation, 43 + 4 of those have no standalone dart
example (see census.txt). Regeneration is add/overwrite; fixtures removed
upstream must be pruned by hand.

## Language version

Like the recovery corpus: no `pubspec.yaml` here on purpose; fixtures are
analyzed as loose files at the consumer's latest language version. A minority
of upstream examples carry `%language=` / `%experiments=` directives
(strip-listed above, counted in census.txt) pinning the version or feature
set they were written for; that context is intentionally not baked into the
fixtures — they are differential inputs, not build artifacts.

## Gate wiring (deliberately deferred)

This corpus is not yet wired into `scripts/serial-gates.sh`. Wiring is a
one-line addition of `--corpus-dir tests/corpus/messages-examples` to the
trend syntax differential leg, plus the usual harness corpus rebuild —
deferred to the serial phase. Note for that phase: per-code parity here is
stronger than raw Jaccard — each fixture has a documented expected code, so
the reconciliation leg should check that the code appears in the differential
result for that fixture, not merely aggregate rates.
