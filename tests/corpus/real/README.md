# Real-world corpus — pub.dev ecosystem sweep

Vendored sources of real pub.dev packages (and two Flutter plugin packages)
driving the real-world differential + performance sweep. Oracle = `dart
analyze`; the engine side = `dartq analyze --format=machine`. Selection,
vendoring policy, and per-package provenance live in each package's
`PROVENANCE.md`; the sweep driver is `scripts/real-corpus-sweep.sh`.

## Layout (per package)

```
real/<pkg>/
  lib/              # upstream lib/, byte-faithful (data assets included)
  pubspec.yaml      # byte-identical to upstream
  LICENSE(*)        # upstream license text(s), kept verbatim
  PROVENANCE.md     # exact version, cache origin, license, why-selected
```

No `.dart_tool/` is committed for the expansion packages (the sweep
materializes resolution at run time; `tests/corpus/*/.dart_tool/*` is
gitignored). The three legacy packages (`args`, `collection`, `meta`) keep
their hand-written, tracked `package_config.json` — the pre-expansion
convention.

## Selection (15 packages, 3 legacy + 12 expansion)

| package | version | dart files | LOC | face | source |
| --- | --- | --- | --- | --- | --- |
| args | 2.7.0 | 12 | 2,295 | zero-dep CLI baseline (legacy) | vendored 2026-10-11 |
| collection | 1.19.1 | 29 | 6,050 | zero-dep algorithms (legacy) | vendored 2026-10-11 |
| meta | 1.19.0 | 3 | 1,216 | annotations (legacy) | vendored 2026-10-11 |
| analyzer | 14.5.0 | 447 | 317,851 | deep transitive dependency tree (12 runtime deps) + large project | local pub cache |
| intl | 0.20.3 | 47 | 37,768 | top i18n; large single package, locale-data assets | local pub cache |
| xml | 7.1.0 | 172 | 29,681 | top data format; parser-heavy | local pub cache |
| html | 0.15.7 | 15 | 26,639 | top HTML parser; long files | local pub cache |
| archive | 4.3.0 | 97 | 22,750 | top compression; binary-format/io face | local pub cache |
| webview_flutter_android | 4.13.0 | 14 | 10,232 | flutter plugin: pigeon-generated platform-channel surface | local pub cache |
| petitparser | 7.1.0 | 132 | 7,971 | zero-dep parser combinators; generics-heavy | local pub cache |
| dio | 5.11.1 | 37 | 5,986 | top HTTP client; async/transformer face | local pub cache |
| http | 1.6.0 | 27 | 2,758 | top HTTP package; conditional-import (io/web) split | local pub cache |
| path | 1.9.1 | 13 | 2,627 | top-5 most-depended; zero-dep clean baseline | local pub cache |
| bloc | 9.2.1 | 8 | 1,002 | top state management; stream/event face | local pub cache |
| url_launcher_android | 6.3.33 | 2 | 518 | flutter plugin: canonical MethodChannel surface | local pub cache |

Faces required by the sweep charter: ≥2 flutter-plugin packages
(`webview_flutter_android`, `url_launcher_android`), ≥1 deep dependency
tree (`analyzer`, whose 12 runtime deps are all present in the local pub
cache), ecosystem-top general packages, and three zero-dep clean baselines
(`path`, `petitparser`, `bloc` — deps resolvable from the vendored corpus
itself). Selection came from what the local pub cache actually holds; the
cache carries no `url_launcher`/`webview`-class package WITH a resolvable
flutter SDK, so the plugin faces run unresolved-`package:flutter` (see
Resolution). Nothing was downloaded or fabricated; every version is the
exact cache copy.

## Resolution (materialized by the sweep, never committed)

Per package, in order, when `<pkg>/.dart_tool/package_config.json` is
absent:

1. **pub-offline** — `dart pub get --offline` in the package dir. Succeeds
   when the local cache resolves the full dependency set (expected:
   `analyzer` at best; recorded per package in
   `target/evidence/real-sweep/<label>/resolution.tsv`).
2. **hand-written fallback** — a `package_config.json` listing the package
   itself (`rootUri: "../"`, `languageVersion: "3.13"` — the harness
   preflight floor, overridden from the upstream bound exactly like the
   legacy packages) plus the four vendored zero-dep corpus packages as
   siblings (`args` 3.3, `collection` 3.4, `meta` 3.5, `path` 3.4 at
   `rootUri: "../../<pkg>"`), so imports of those resolve and the
   cross-library semantic face is exercised without network access. A
   minimal `pubspec.lock` (`packages: {}`) accompanies it.
3. The two plugin packages declare `sdk: flutter`; pub cannot resolve that
   offline by design — they land on the fallback face with `package:flutter`
   unresolved, producing identical unresolved-import cascades on BOTH
   sides. That is the documented platform-channel face under a
   zero-network cache; the divergence capture still applies to it.

The same config feeds both faces, so every override above is
comparison-fair. To refresh resolution for one package, delete only that
package's untracked `.dart_tool/` and rerun the sweep.

## Sweep protocol

`scripts/real-corpus-sweep.sh` (serial verification phase only — it runs
real `dart`, `dartq`, and `harness` processes):

```bash
# plan only (no dart/cargo/dartq execution)
scripts/real-corpus-sweep.sh --dry-run

# full sweep: diff face + perf face + aggregate, evidence under
# target/evidence/real-sweep/<label>/
DARTQ_DART_SDK=/path/to/dart/sdk scripts/real-corpus-sweep.sh --label r1
```

Per package the sweep runs the repo's own instruments — no bespoke diff
logic:

- **Diff face** — `harness run --corpus-dir tests/corpus/real/<pkg>
  --oracle-dart "$DARTQ_DART_SDK/bin/dart" --dartq-cmd "./target/release/
  dartq analyze --format=machine --sdk $DARTQ_DART_SDK"
  --severity-filter all --allow-vacuous --persist-regressions <out>/cases/<pkg>
  --out <out>/diff/<pkg>`. Entry caliber is the harness's own: lowercase
  `(code, file)` keys, severity-insensitive matching per severity stats.
  `--severity-filter all` widens the judged set to every severity so
  WARNING/INFO divergences are captured, not just the ERROR gate;
  `--allow-vacuous` accepts clean-vs-clean outcomes (the zero-dep upstream
  packages are lint-clean). Per-package `report.json`/`report.md` +
  `raw-outputs.json` land under `<out>/diff/<pkg>/`.
- **Perf face** — `harness bench --metric cold --metric rss-100k
  --corpus-dir tests/corpus/real/<pkg> --dartq-cmd ... --oracle-dart ...
  --sdk "$DARTQ_DART_SDK" --out <out>/bench/<pkg>`: one cold
  full-analysis wall time per side plus the oracle/dartq ratio
  (`cold/report.json`), and peak tree-RSS via the bench's own ps-polling
  sampler with watchdog (`rss-100k/<pkg>/report.json`: `peak_rss_kib`,
  `rss_per_100k_loc_kib`, `elapsed_ms`). Both are single-run instruments;
  the cold report's methodology caveat applies — cite small packages as
  relative numbers and `analyzer` (~318k LOC) as the meaningful absolute
  one.
- **Aggregate** — a final pass folds every per-package report into
  `<out>/summary.json` + `<out>/summary.md` (per-package jaccards, diff
  rows, resolution mode, cold times, peak RSS; totals row). `resolution.tsv`
  records how each package's `.dart_tool/` was produced.

Exit codes: `0` sweep completed (divergences may exist — they are
discoveries, persisted as cases, not sweep failures); `2` preflight or
harness/infra error on any face.

## Divergence capture

Every diffing `(code, file)` entry lands in the repository's standard
regression format: `harness run --persist-regressions <out>/cases/<pkg>/`
writes `regression-case-v1` case files (`<hash8>.json` + `.edits.jsonl`,
self-contained source inline, replay command ready) — same failure always
overwrites the same case file, never grows. Triage workflow (replay,
exemption via `tests/corpus/regressions/known-diffs.json`, promotion into
a targeted corpus fixture) follows `tests/corpus/regressions/README.md`.
Note its derivation pin: promoting or deleting cases under
`tests/corpus/regressions/cases/` requires re-running the sem2 pin.

## LOC

Counts above are total lines of `.dart` files under `lib/` (the same
`wc -l` basis as each PROVENANCE record; the bench's `loc` field uses its
own non-blank definition and is recorded per report).
