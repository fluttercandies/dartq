# dartiq

dartiq is a semantic analysis engine for Dart and Flutter, written in Rust.
It targets CLI and CI usage as a replacement for `dart analyze`, with AI
coding agents as its first-class users: machine-readable output, incremental
re-analysis, a bounded memory footprint, and multi-SDK support. The official
Dart analyzer is the semantic oracle — dartiq replicates its diagnostics and
exceeds its performance envelope, and every semantic claim is validated by
differential testing against `dart analyze`. See `PRD.md` for the full
design document.

## Design Pillars

1. **Performance** — end-to-end budgets: cold/warm start, incremental
   latency, cache hit rate, parallelism; controlled memory is the premise,
   not the goal.
2. **Design** — one-way layering, a strict identity model, a single source
   of truth per fact, data-driven evolution.
3. **AI-first** — every API treats AI agents as first-class users:
   structured output, impact prediction, cancellation, concurrency.
4. **Token economy** — output is a cost: tiered verbosity, stable
   references, deltas, batch queries; fewest tokens for equal information.
5. **Extensibility** — new Dart versions, experiments, platforms, and
   plugins are "add data / add rules", never "change the architecture".
6. **Analyzer replacement** — `dartiq analyze` / `dartiq check` take the
   place of `dart analyze` (machine-readable superset), measured against
   the official analyzer as the oracle.

## Performance

Design targets (budgets from the charter, not yet end-to-end claims):

- **≥ 10x** faster than `dart analyze` end-to-end — the floor for shipping
  any milestone.
- **100x – 1000x** on cold analysis of large repositories.
- **1000x+** on incremental re-analysis after small edits.

Measured so far (component-level microbenchmarks; Apple M1-class arm64,
release builds; these are current measurements, not projections):

| Component | Measurement |
| --- | --- |
| Scanner | 260.5 MB/s single-thread over the `dart-sdk` package corpus |
| Type system | subtype chain 4.7M ops/s; interner hit path 29M ops/s; 64 B per interned type |
| Source layer | ~1.16 MB of persistent storage per MB of source (content-addressed) |

End-to-end comparisons against `dart analyze` are established by
`tests/bench/oracle_baseline.sh` (see [Differential Testing](#differential-testing)
and [Oracle Baseline](#oracle-baseline)).

## Architecture

One-way layering, one crate per concern. 23 workspace crates plus the
differential-testing harness:

| Layer | Crate | Role | Status |
| --- | --- | --- | --- |
| Foundation | [`dartiq-source`](crates/dartiq-source) | Content-addressed source storage: `ContentId` (blake3), source store, `FileId`, locators | landed |
| Foundation | [`dartiq-language`](crates/dartiq-language) | Feature registry generated at build time from the SDK's `experimental_features.yaml`; `LanguageVersion` / `FeatureSet` resolution | landed |
| Foundation | [`dartiq-diagnostics`](crates/dartiq-diagnostics) | Stable diagnostic code registry (265 official + 12 dartiq-own codes) and `machine` / `json` writers byte-compatible with `dart analyze --format=machine` | landed |
| Foundation | [`dartiq-schema`](crates/dartiq-schema) | Versioned machine-readable JSON output schemas | in development |
| Frontend | [`dartiq-scanner`](crates/dartiq-scanner) | Full Dart 3.15 tokenizer: single pass, no backtracking, `@dart` version-comment window, error recovery → linear token table + syntax diagnostics | landed |
| Frontend | [`dartiq-parser`](crates/dartiq-parser) | Fault-tolerant recursive-descent parser for the full Dart 3.15 grammar: token table in, parser-event stream out | landed |
| Frontend | [`dartiq-ast`](crates/dartiq-ast) | Compact arena AST: nodes reference token ranges, spans derive from the token table (byte + UTF-16) | landed |
| Environment | [`dartiq-pub`](crates/dartiq-pub) | Consumers for pub's resolution artifacts (`package_config.json`, `pubspec.yaml`, `pubspec.lock`), workspace discovery, freshness, resolved package graphs; no version solving | landed |
| Environment | [`dartiq-package`](crates/dartiq-package) | Typed package-identity model consumed by resolution, composed on `dartiq-pub`'s artifacts | landed |
| Environment | [`dartiq-sdk`](crates/dartiq-sdk) | SDK metadata: `libraries.json` loading, identity, `dart:` library surface, target/platform scoping | landed |
| Semantics | [`dartiq-types`](crates/dartiq-types) | Type system M1 surface: ten-shape type skeleton, nullability dimension, canonical interning, subtype / assignability, least upper bound | landed |
| Semantics | [`dartiq-resolution`](crates/dartiq-resolution) | URI, import/export/part, and conditional-import resolution with namespace computation | in development |
| Semantics | [`dartiq-semantic`](crates/dartiq-semantic) | Element, reference, and symbol arenas forming the semantic graph | in development |
| Semantics | [`dartiq-flow`](crates/dartiq-flow) | Event-driven flow analysis, decoupled from the AST | in development |
| Services | [`dartiq-index`](crates/dartiq-index) | Reference, inheritance, and test graphs | in development |
| Services | [`dartiq-incremental`](crates/dartiq-incremental) | Invalidation engine on signature hashes and dependency fingerprints | in development |
| Services | [`dartiq-cache`](crates/dartiq-cache) | Layered cache (L1/L2/L3) and eviction policies | in development |
| Services | [`dartiq-snapshot`](crates/dartiq-snapshot) | Persistence for analysis contexts and snapshots | in development |
| Services | [`dartiq-engine`](crates/dartiq-engine) | Runtime, scheduler, cancellation, overlay management | in development |
| Services | [`dartiq-lints`](crates/dartiq-lints) | Built-in lint rule set | in development |
| Services | [`dartiq-plugin`](crates/dartiq-plugin) | Plugin protocol IPC types | in development |
| Interface | [`dartiq-ai`](crates/dartiq-ai) | Agent-facing queries: impact, explain, compatibility, fix | in development |
| Interface | [`dartiq-cli`](crates/dartiq-cli) | Command-line entry point | in development |
| Testing | [`harness`](tests/harness) | Differential-testing driver: oracle vs dartiq over shared corpora, `(code, file)` Jaccard comparison | landed |

`crates/dartiq-plugin-host/` is the host-side Dart package of the plugin
protocol (M4) and is not part of the Cargo workspace.

## Differential Testing

`tests/harness` runs the official analyzer (`dart analyze --format=machine`)
and dartiq over the same corpora and compares results as `(code, file)`
entry sets (Jaccard). Corpora live in `tests/corpus/`: `m0` (assembled
analyzer-suite cases), `m0-syntax` (syntax-broken files), and the syntax
gate's code list `syntax-codes.txt` — the syntax codes of
`crates/dartiq-diagnostics/data/syntax-codes.tsv` (265 rows: 262
scanner/parser codes + the 3-code language-version family; the txt file
drops the family, leaving 262).

Current measured baseline (oracle: `dart analyze` 3.13.1, `--e2e-oracle`
self-check, both sides the analyzer — Jaccard 1.000 across all runs):

| Run | Corpus | Caliber | Files with diags | Instances | Jaccard |
| --- | --- | --- | ---: | ---: | ---: |
| full | `m0` | all | 166 | 1844 | 1.000 |
| syntax | `m0-syntax` | codes ∩ ERROR | 272 | 2144 | 1.000 |
| full | `m0` + `m0-syntax` | all | 439 | 6873 | 1.000 |

The M0 gate is the syntax-caliber run against the real dartiq side; the
gate passes at ERROR-tier Jaccard ≥ 0.99 (baseline: 539 entries /
2144 instances):

```bash
cargo test -p harness          # harness unit tests
cargo run -p harness -- corpus # assemble corpora and run pub get

# Comparator self-check: both sides dart analyze, Jaccard must be 1.000.
cargo run -p harness -- run --e2e-oracle \
  --corpus-dir tests/corpus/m0 --corpus-dir tests/corpus/m0-syntax \
  --severity-filter ERROR --codes-file tests/corpus/syntax-codes.txt

# Real differential run: swap --e2e-oracle for the dartiq driver.
cargo run -p harness -- run \
  --corpus-dir tests/corpus/m0 --corpus-dir tests/corpus/m0-syntax \
  --severity-filter ERROR --codes-file tests/corpus/syntax-codes.txt \
  --dartiq-cmd "cargo run -q -p dartiq-cli -- check --format=machine"
```

Exit codes: `0` = gate met, `1` = below threshold, `2` = harness error.
Preflight requires every corpus package to carry a
`.dart_tool/package_config.json` with `languageVersion >= 3.13`.
`--oracle-dart`, `--type-filter`, and `--threshold` are also available; see
`cargo run -p harness -- --help`.

## Oracle Baseline

`tests/bench/oracle_baseline.sh <target-dir> [rounds]` measures the oracle
itself: wall-clock time and peak RSS of `dart analyze --format=machine`
over N sequential rounds (default 5; round 1 is "cold", rounds 2..N are
"hot"), plus corpus scale (files / LOC / bytes). One JSON object per line,
one per round plus a summary record. Run it before and after performance
work to keep comparisons honest — dartiq budgets are stated relative to
these numbers.

## Build and Test

```bash
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check                # rustfmt.toml: style_edition 2024, max_width 100
cargo clippy --workspace --all-targets    # rust_2018_idioms + clippy all/pedantic (warn)
```

The toolchain is pinned by `rust-toolchain.toml` (Rust 1.96.0, edition
2024, resolver "3"). `Cargo.lock` is committed: the workspace ships
binaries (`dartiq-cli`, `harness`).

## Status

M0 (scanner, parser, arena AST, source layer, diagnostics registry, and the
differential harness) has landed; the language/version, pub/package, SDK,
and type-system foundations are in place, and the syntax-caliber gate runs
at Jaccard 1.000. M1 (resolution, semantic graph, incremental analysis,
JSON CLI) is in progress — the remaining crates in the table above are
under development.

Milestones per `PRD.md` §20.3:

- **M0 — skeleton**: fault-tolerant parser + arena AST for the full Dart
  3.15 grammar; syntax differential testing against the official analyzer.
- **M1 — MVP**: URI resolution, symbol index, static nullability, basic
  semantic diagnostics, L1 incremental analysis, JSON CLI.
- **M2 — full semantics**: complete type system, flow analysis, built-in
  lints, L2 incremental analysis.
- **M3 — multi-environment**: impact/test graphs, multi-workspace,
  persistent snapshots, L3 incremental analysis.
- **M4 — ecosystem**: plugin host, fixes/assists, daemon + RPC.

## Third-Party Notices

Reference sources vendored under `.third/` (the Dart SDK, pub, and
rust-analyzer repositories) are local development references used for
semantic grounding and calibration only. They are not part of the source
distribution, are never modified, and are not required to build or use
dartiq.

## License

Licensed under either of Apache-2.0 or MIT, at your option:
[LICENSE-APACHE](LICENSE-APACHE) · [LICENSE-MIT](LICENSE-MIT).
