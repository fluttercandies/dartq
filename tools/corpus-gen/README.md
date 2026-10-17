# corpus-gen

Deterministic synthetic Dart corpus generator for dartq scale and
differential testing. It renders a self-contained multi-package Dart
"mega project" whose size is pinned by a scale preset (1k / 10k / 100k
files, plus the ~160-file `slice` preset whose seed-0 output is the tracked
golden at tests/corpus/mega-x/slice/), exercising the language feature
matrix — class modifiers,
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
valid Dart that stresses parsing and semantic analysis at scale — plus a
small adversarial overlay (below) that deliberately is not.

## Adversarial overlay (`zadv/`)

On top of the base corpus, every run appends an adversarial subtree under
`zadv/` (paths sort after `tests/`, so overlay files are emitted strictly
after all base files — the base corpus's bytes are unchanged for existing
seeds and the overlay is pure addition to the plan and the fingerprint):

- `zadv/patho/patho_NNNNN.dart` — pathological single files (~0.1% of the
  corpus, floored at 3): 1000-deep nested collections, ~20k-term
  expressions, 600-arm switch expressions, 800-segment cascades, 400-part
  interpolation chains, 300-deep nested closures. All syntactically valid —
  the stress axis is size/depth, not error count.
- `zadv/unicode/uni_NNNNN.dart` — unicode edge family (~1%): RTL literals,
  combining marks, astral escapes and raw astral characters, emoji/ZWJ
  sequences, zero-width characters, unicode doc comments, unicode URI
  literals, and non-ASCII identifier edges (the scanner's recovery-merge
  shape). Dart identifiers stay ASCII-only; unicode lives in string
  literals, comments and URI literals.
- `zadv/errstorm_NNNN/` — recovery-storm packages (~0.1% as packages):
  every `lib/storm_*.dart` stacks >= 5 malformed constructs drawn from the
  `tests/corpus/recovery` damage taxonomy (unclosed paired tokens, missing
  clauses, declaration-order damage, extra modifiers), selected
  deterministically per file from the seed.

## Usage

```bash
cargo run -p corpus-gen -- --out tests/corpus/mega/100k --scale 100k --seed 0
```

| Flag | Meaning |
| --- | --- |
| `--out <dir>` | Output directory (created on demand; use a fresh directory for a clean manifest) |
| `--scale` | `slice`, `1k`, `10k`, or `100k` target file count (exact; `slice` pins one of every adversarial shape — 6 patho, 8 unicode, 1 storm package) |
| `--seed <u64>` | Content seed, default `0` |
| `--layout` | `flat` (default, the byte-stable shape) or `workspace` — a pub-workspace tree: root `workspace:` pubspec, `resolution: workspace` members at two depths, per-member `.dart_tool/pub/workspace_ref.json`, vendored non-member `third_party/` path deps, and a hand-written root `.dart_tool/package_config.json` (the `tests/corpus/nested-versions/` shape at scale) |
| `--version-ladder` | Version×feature matrix: feature-package `environment.sdk` constraints cycle `^2.12.0` through `^3.15.0`, a deterministic subset of rotation files carry `// @dart =` banners (below AND above the package floor), every package gets an `analysis_options.yaml` with an `enable-experiment` list, and cross-version package import edges are wired |
| `--flutter-app` | Flutter-widget face (#2063): adds an in-corpus stub `packages/flutter` package (pubspec + `lib/widgets.dart` + part files declaring StatelessWidget, StatefulWidget/State, BuildContext, Key, and the MaterialApp/Scaffold/AppBar/Text/TextButton/Center/Column/Row/Container/Padding concrete surface), makes every feature package path-depend on it, turns the widget host/part slots into real StatefulWidget/State pairs with nested build trees, and renders every 4th rotation file as a widget scene. Both sides of a differential analyze the same stub, so parity stays `dart analyze`-over-identical-sources — no Flutter SDK, no network. The stub and widget-file bytes are index-derived (RNG-free), hence scale-invariant and pinnable (goldens/app-x/slice) |
| `--emit-replay-seq <n>` | Additionally write `<out>/edits.jsonl`: an n-step deterministic replay sequence that round-trips the tree to its initial state |

Both faces default OFF — the flat, single-version tree is byte-identical
to the pre-flag output for every seed and scale (the `slice` golden pins
that contract). Under `workspace`, member constraints stay at >= 3.5 (the
pub `resolution: workspace` gate, documented in the nested-versions
corpus); the low end of the ladder (2.12–3.3) is carried by the vendored
non-member packages instead.

Bad arguments exit with a non-zero status and a clear message. On success
the tool prints one summary line with the file/line counts and the
fingerprint.

## Replay sequences

`--emit-replay-seq <n>` writes `edits.jsonl` next to `manifest.json`, in the
schema consumed by `tests/harness` `replay_edit` (one JSON object per line:
`{"file","line","column","length","new_text"}` UTF-16 splices, `{"remove"}`,
`{"create","text"}`). The sequence is built from self-inverting units —
comment-noise splice pairs, `splice`/`remove`/`create` triples on an
imported facet file, class-rename waves and arity waves across the 6-link
`ChainNode` inheritance chain — so the final tree state is byte-identical
to the initial state. The file is an opt-in sidecar: it is not part of the
plan and is not fingerprinted.

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
    unit_NNNNN_test.dart   # test-shaped files: pattern/expression blocks in
                           # plain Dart (no package:test dependency)
  zadv/                  # adversarial overlay (appended last)
    errstorm_NNNN/       #   recovery-storm packages (>=5 damages per file)
    patho/*.dart         #   pathological singles (deep nesting, mega exprs)
    unicode/*.dart       #   unicode edge files (RTL/astral/emoji/BOM/URI)
```

File distribution: ~60% feature packages, ~15% core, ~10% app (root
package), ~15% tests/, plus the adversarial overlay appended on top
(~2% more files: ~1% unicode, ~0.1% patho, ~0.1% storm packages).
Feature package count grows with scale (10 at 1k, 100 at 10k, 400 at
100k). Per-file size is mostly 20-80 lines with a deterministic 200-line
file every 31st file.

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
sorted streaming order (including under `--layout workspace` +
`--version-ladder` — the extras interleave at their true lexicographic
positions), same-seed byte identity, seed sensitivity, the feature-matrix
coverage gate (every unconditional cell at least 3 hits at the 1k scale;
the adversarial cells additionally pin >= 30 hits at 100k via plan-count
assertions), plus overlay checks: storm damage stacking, patho depth
budgets, unicode byte-exactness, replay-sequence schema, and the 1d/1e
faces (ladder rungs, bidirectional banners, member refs, vendored
packages, shared package_config). A serial-phase `DARTQ_MEGA_X=1`
`#[ignore]` leg runs the adversarial `rss-100k` ceiling check against a
regenerated `tests/corpus/mega-x/100k` tree — the in-test mirror of
`scripts/mega-x-run.sh` leg X1.
Serial-phase generation commands:

```bash
cargo run -p corpus-gen -- --out tests/corpus/mega/1k --scale 1k --seed 0
cargo run -p corpus-gen -- --out tests/corpus/mega/10k --scale 10k --seed 0
cargo run -p corpus-gen -- --out tests/corpus/mega/100k --scale 100k --seed 0
```

`tests/corpus/mega/` is gitignored; regenerate locally as needed. The
`slice` scale is the opposite contract: `tests/corpus/mega-x/slice/` IS
tracked, and `golden_slice_is_byte_stable` regenerates it in-process to
assert byte-equality — regenerate it only alongside a deliberate generator
change:

```bash
cargo run -p corpus-gen -- --out tests/corpus/mega-x/slice --scale slice --seed 0 --emit-replay-seq 30
```

## Consumption

`scripts/mega-run.sh` is the wired consumer (the S4 budget shape): per
scale it regenerates the tree if absent (same pinned
`--seed 0`), materializes the package config the generated tree lacks, and
runs the harness bench faces `rss-100k` and `incremental-p95` with
`--corpus-dir` pointed at the scale dir. No oracle differential by default
(S4 is a dartq-side budget: RSS ceiling + latency, not a Jaccard row).

`scripts/mega-x-run.sh` is the adversarial-axis consumer: it regenerates
`tests/corpus/mega-x/{1k,100k}` (the 1k tree with `--emit-replay-seq`) and
runs the four X-legs — adversarial `rss-100k`, the valid-subset oracle
differential (errstorm + diagnostic-bearing unicode variants excluded),
the per-storm-package non-vacuity + parity leg, and `harness replay` over
the generated `edits.jsonl`. Serial phase only, driven by
`serial-gates.sh` as the `mega-x-r1` step.

```bash
scripts/mega-run.sh --dry-run --scale 10k   # plan only, executes nothing
scripts/mega-run.sh --scale 1k              # one scale (default)
scripts/mega-run.sh --all                   # the 1k -> 10k -> 100k ladder
scripts/mega-run.sh --all --label mega-r2   # report dir suffix
```

Facts the runner relies on:

- **Seed contract:** same seed + scale = byte-identical tree and manifest
  (`manifest.json` fingerprint). The runner always pins `--seed 0`; only
  regenerate with a different seed if you intend to fork the corpus shape.
- **Disk footprint** (measured on the seed-0 trees): 1k ~4 MB, 10k ~40 MB,
  100k ~398 MB. `incremental-p95` additionally copies the scale dir per run
  (edit fixture), so the 100k scale needs ~800 MB of headroom.
- **Package config:** the generator emits `pubspec.yaml` files but no
  `.dart_tool/`. `scripts/mega-run.sh` materializes a hand-written
  `.dart_tool/package_config.json` per scale (app + every `packages/*`
  entry, config-file-relative `rootUri`, `languageVersion 3.13` — the
  harness corpus preflight's floor) before invoking the harness. It is
  idempotent and skipped when present; delete it alongside the tree when
  regenerating from scratch.
- **Edit manifest:** `incremental-p95` needs `--manifest`; the runner
  writes a five-step budget-smoke manifest (whole-file edits to
  `lib/main.dart`, stable across scales) into the report dir. Replace it
  there for a heavier acceptance sequence — the face's manifest schema is
  documented in `tests/harness/src/bench.rs`.
- **Timeout:** the runner exports
  `DARTQ_HARNESS_ORACLE_TIMEOUT_MS=600000` unless already set — a
  100k-file batch analysis exceeds the harness's 120 s default.
- **Reports:** `target/evidence/mega/<label>/<scale>/{rss-100k,
  incremental-p95}/report.json`; the runner's summary prints per-scale
  wall time, files/LOC, peak RSS (+ per-100k normalization), and the
  incremental p50/p95/p99. Budget thresholds are intentionally not asserted
  yet (no baseline exists; exit 1 is reserved for the acceptance run).

