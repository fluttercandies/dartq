# recovery corpus

Systematic parser-recovery differential corpus. Every fixture is the input
side of one recovery case transcribed from the upstream Dart analyzer's own
recovery test suite (upstream Dart SDK source,
`pkg/analyzer/test/src/fasta/recovery`): the inputs the analyzer team uses to
hammer their parser recovery paths, one syntax context at a time (missing
tokens, extra tokens, paired-token damage, out-of-order clauses, misplaced
code).

Expectations are never transcribed. The upstream suite's inline annotations
(caret position markers and `// [diag.*]` expectation lines) are stripped at
generation time; what parity should look like is decided by the oracle
differential gate (`dart analyze` vs dartq, Jaccard over `(code, file)`), the
same contract as every other syntax corpus. A fixture is therefore pure input
data: it makes no claim about which diagnostics are correct.

## Layout

```
partial_code/<context>/<method>.dart   31 contexts, one per upstream
                                       partial_code test file (class
                                       declarations, statements, directives,
                                       members, parameters, ...)
code_order/<class>/<method>.dart       clause-order damage (extends/with/
                                       implements, directives before library,
                                       misplaced metadata)
extra_code/<class>/<method>.dart       extraneous tokens and modifiers
invalid_code/<class>/<method>.dart     mistyped / misplaced code shapes
missing_code/<class>/<method>.dart     missing commas, names, operands, bodies
paired_tokens/<class>/<method>.dart    unbalanced (), {}, [], <> damage
```

The five cross-cutting files define several upstream test classes each, so
fixtures sit one level deeper, one directory per test class. Fixture file
names are the upstream test method names converted to snake_case; methods
whose body carries more than one recovery input emit `<name>_v2.dart`,
`_v3.dart`, ... Each fixture's first line records its provenance:

```dart
// recovery: partial_code/class_declaration_test.dart/test_class_declaration_equals_class
```

(upstream file + test method, plus ` (variant N)` when applicable), so every
fixture is traceable without referencing any local checkout path.

## Regeneration

`generate.py` rebuilds the whole tree deterministically (same source ->
byte-identical fixtures). The upstream test-suite directory is a required
environment variable; nothing machine-specific is embedded:

```sh
DARTQ_RECOVERY_SOURCE=/path/to/analyzer/test/src/fasta/recovery \
    python3 tests/corpus/recovery/generate.py
```

Current census: 3757 fixtures from 3753 upstream test methods (four methods
carry two or three recovery inputs each: one in class_declaration, three in
try_statement). Regeneration is add/overwrite; fixtures removed upstream must
be pruned by hand.

## Language version

No `pubspec.yaml` here on purpose: fixtures are analyzed as loose files at the
consumer's latest language version, matching the posture of the upstream
suite (its tests run at the SDK's current language). A minority of fixtures
use recent language syntax; that is intended — they are differential inputs,
not build artifacts.

## Gate wiring (deliberately deferred)

This corpus is not yet wired into `scripts/serial-gates.sh`. Wiring is a
one-line addition of `--corpus-dir tests/corpus/recovery` to the trend syntax
differential leg, plus the usual harness corpus rebuild — deferred to the
serial phase because recovery parity is not yet proven, and wiring an
unproven corpus would hold the gate red. Note for that phase: 3757 tiny
files, so expect the oracle leg of the gate to grow accordingly.
