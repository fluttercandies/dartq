# corpus-gen

Deterministic synthetic Dart corpus generator for dartq scale and
differential testing. It renders a self-contained multi-package Dart
"mega project" whose size is pinned by a scale preset (1k / 10k / 100k
files), exercising the language feature matrix — class modifiers,
generics with bounds, mixins (including `on` constraints), extension
types, plain and enhanced enums, records, the full pattern set (object,
record, list, map, relational, logical) across switch expressions,
switch statements, and if-case, collection control flow, null safety,
labeled jumps, async/await and streams, typedefs, operators, factory and
constant constructors, cascades, string interpolation, numeric literal
shapes, external declarations, part/part-of pairs, conditional and
deferred imports, export combinators, and cross-file inheritance chains
(6 classes deep) plus cross-file factory redirects.

The output does not need to pass `dart analyze`; it must be syntactically
valid Dart that stresses parsing and semantic analysis at scale.

## Usage

```bash
cargo run -p corpus-gen -- --out tests/corpus/mega/100k --scale 100k --seed 0
```

| Flag | Meaning |
| --- | --- |
| `--out <dir>` | Output directory (created on demand; use a fresh directory for a clean manifest) |
| `--scale` | `1k`, `10k`, or `100k` target file count (exact) |
| `--seed <u64>` | Content seed, default `0` |

Bad arguments exit with a non-zero status and a clear message. On success
the tool prints one summary line with the file/line counts and the
fingerprint.

## Layout

```
<out>/
  manifest.json            # counters + fingerprint
  pubspec.yaml             # app package (name: app, path deps into packages/)
  lib/
    main.dart              # entry point, deferred-loads core
    src/controllers/*.dart
    src/screens/*.dart
  packages/
    core/                  # shared package: barrel with show/hide exports,
      lib/core.dart        # generics-heavy containers, 6-link inheritance
      lib/lazy.dart        # chain, conditional-import platform group,
      lib/src/**.dart      # typedefs, shared value types
    feature_NNNN/          # N feature packages: models / repositories /
      lib/**.dart          # services / widgets rotations, anchor model,
      pubspec.yaml         # gateway factory redirect, widget part pairs,
                           # conditional-import facet trio
  tests/
    unit_NNNNN_test.dart   # test-shaped files using package:test syntax
```

File distribution: ~60% feature packages, ~15% core, ~10% app (root
package), ~15% tests/. Feature package count grows with scale (10 at 1k,
100 at 10k, 400 at 100k). Per-file size is mostly 20-80 lines with a
deterministic 200-line file every 31st file.

## Determinism contract

Same seed and scale produce byte-identical output and an identical
manifest across runs and platforms. The generator is std-only: a
hand-rolled SplitMix64 PRNG, no time or entropy sources, and
`BTreeMap`-ordered manifest keys. Files are written in lexicographic
path order, which is also the fingerprint order, so generation streams
to disk file by file with memory bounded by the file being rendered.

`manifest.json` records seed, scale, file count, total line count,
per-top-level-dir file counts, and a fingerprint: FNV-1a 64 computed
over, per file in generation order, the relative path bytes, a `0x00`
separator, the decimal byte size, and another `0x00`. Strong enough to
assert reproducibility; deliberately not a cryptographic hash.

## Intended measurement axes

- incremental P95 (edit to diagnostics) over this corpus;
- RSS per 100k LOC (memory ceiling for the 100k scale);
- cold-start full-analysis time against the `dart analyze` oracle;
- differential diagnostics at scale (Jaccard against the oracle).

## Verification

`cargo test -p corpus-gen` covers scale math and distribution ratios,
sorted streaming order, same-seed byte identity, seed sensitivity, and a
feature-matrix coverage gate (every matrix cell at least 3 hits at the
1k scale). Serial-phase generation commands:

```bash
cargo run -p corpus-gen -- --out tests/corpus/mega/1k --scale 1k --seed 0
cargo run -p corpus-gen -- --out tests/corpus/mega/10k --scale 10k --seed 0
cargo run -p corpus-gen -- --out tests/corpus/mega/100k --scale 100k --seed 0
```

`tests/corpus/mega/` is gitignored; regenerate locally as needed.
