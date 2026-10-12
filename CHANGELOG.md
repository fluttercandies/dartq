# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.0] - 2026-10-12

Initial release: a Rust-native re-implementation of the Dart analyzer's
diagnostic pipeline — scanner, parser, flow analysis, resolution, the
semantic diagnoser, and the lint engine — validated by differential
testing against `dart analyze` (Dart 3.13.1) as the oracle.

### Added

- **CLI** (`dartq`) with oracle exit-code tiers (0 clean/info · 1 fatal infos ·
  2 fatal warnings · 3 errors · 4 internal · 64 usage) across the faces:
  - `check` — scan + parse over a directory tree, machine output
    byte-compatible with `dart analyze --format=machine` (JSON alternative),
    `@dart` version-comment override rescan, `--metrics-json` run facts.
  - `analyze` — the full engine over a real package directory: pub faces,
    discovery, scan, parse, resolution, and the semantic/flow/lint
    diagnostic tiers; machine and human formats, `--overlay PATH=FILE`
    in-memory edits (stdin via `-`), `--memory-limit`, `--trace-events`
    JSONL engine events, and `--cache-size-limit` for the persistent
    content store.
  - `query` — symbol-index queries over the analyzed workspace: `symbol`,
    `references`, `callers`, `implementations`, `dependencies`, and
    `batch` (many symbols x kinds in one workspace assembly); machine rows
    or the frozen v1 JSON envelope.
  - `explain` — the one-stop JSON report for one symbol over the analyzed
    workspace (declaration, uses, imports, tests, dependencies).
  - `impact` — what a change to one file would affect: changed API symbols,
    semantic/transitive affected files, affected tests, environments.
  - `compat` — one file analyzed under several SDK roots (`--sdk-root`,
    repeatable, or `dartq.yaml` contexts) into a per-axis compatibility
    matrix with first-failure attribution.
  - `fix` / `assists` — anchored, drift-corrected fix application over
    built-in lint fixes plus plugin proposals (`--dry-run` proposes without
    writing), and report-only plugin assist queries for one location.
  - `index` — build the symbol index over the analyzed workspace and report
    what it built.
  - `daemon` — an LSP 3.17 server over stdin/stdout (see below).
  - `version`.
- **Daemon** — LSP 3.17 over stdio: standard lifecycle, whole-file overlay
  edits through `textDocument/didOpen`/`didChange`/`didClose`, full
  diagnostic pushes, turn batching, cancellation, and thirteen `dartq/*`
  RPC methods mirroring the CLI (`analyze`, `check`, `query`, `queryBatch`,
  `index`, `impact`, `explain`, `fix`, `assists`, `compat`) plus multi-agent
  overlay staging (`overlay.set`/`overlay.commit`/`overlay.clear`), delta
  `since` cursors, and an optional poll-based `--watch` mode.
- **Scanner** — full Dart 3.15 tokenizer: single pass, no backtracking, the
  `@dart` version-comment window, and error recovery to a linear token
  table plus syntax diagnostics.
- **Parser** — fault-tolerant recursive-descent parsing of the full Dart
  3.15 grammar, with recovery aligned to the official parser's decision
  order, emitting a parser-event stream over the token table.
- **Diagnostics** — stable diagnostic code registry and `machine`/`json`
  writers byte-compatible with the oracle's formats.
- **Semantic and flow analysis** — element/reference/symbol arenas, the
  type-system surface (canonical interning, subtype/assignability, least
  upper bound), and the semantic/flow diagnostic families: undefined
  identifiers and members, wrong-number-of-arguments/type-arguments, the
  part/part-of family, duplicate definitions, class-modifier decisions, and
  dead-code/reachability with oracle-anchored spans.
- **Lint rules** — all 89 `package:lints` rules (34 core + 55 recommended)
  implemented and wired into the analyze face, with enable sets resolved
  from `analysis_options.yaml` include chains, plus one opt-in
  diagnostic-mirror rule outside `package:lints`.
- **pub/package layer** — consumers for pub's resolution artifacts
  (`package_config.json`, `pubspec.yaml`, `pubspec.lock`), workspace
  discovery, freshness advisories, and validation faces; no version solving.
- **Symbol index** — reference, inheritance (clause-typed supertype edges),
  and test graphs backing the query and impact faces.
- **Plugin system** — protocol v1 (frames, codec, handshake, manifest), the
  enforced runtime with discovery, spawn, and isolation budgets, an
  `analysis_server_plugin` compatibility shim, and the host-side Dart
  package.
- **Persistence** — content-addressed per-file facts snapshots with atomic
  generation save/load (daemon debounced flush), and the persistent L3
  content store backing incremental re-analysis across sessions.
- **Workspace configuration** — optional `dartq.yaml` (named SDK contexts,
  generated-file policy, memory and cache limits), all-optional with frozen
  key validation.
- **Frozen v1 output schemas** — versioned machine-readable JSON contracts
  (check/analyze/assists/query/index/impact/compat/explain domains) with CI
  validators and verbosity tiers.
- **Differential-testing harness and gate chain** — oracle vs dartq over
  shared corpora with `(code, file)` Jaccard comparison, auto-bisect,
  replay/edit-replay drivers, benchmark arms, corpus tooling, and per-gate
  scripts. The oracle of record is `dart analyze` from a pinned Dart
  3.13.1 install, and every differential gate holds the full-parity bar
  (Jaccard 1.0): the M0 syntax run (check face over the 255-code syntax
  universe), the M0 union run (analyze face over the 290-code union), the
  M1 frozen-code semantic run, the INFO-tier lint gate, the M2
  warning-tier and M3 unused-family gates, the valid-syntax acceptance
  gate (zero diagnostics over the feature-family corpus), and the
  reproducibility gate sealing a byte-identical output digest with a
  repro tuple.
- **Packaging** — dual-license (Apache-2.0 OR MIT) publishable crates,
  crates.io metadata, CI workflow, and a committed `Cargo.lock`.

### Changed

- Performance passes across the frontend: scanner memchr skip fast paths
  and cheap recovery-option snapshots, parser/resolution hot-path hashing
  on `FxHashMap`, and batched workspace reads in the engine.
- Daemon robustness: off-loop fix dispatch, per-group panic containment,
  bounded plugin teardown, and clamped untrusted plugin diagnostics.

### Fixed

- Dead-code region anchoring: labeled-statement rows anchor at the first
  label token and for-header regions at the first unreachable node.
- Parser recovery alignment: extraneous-modifier faces, synthetic-formals
  reset, parameter-group recovery staying in-list, bracket-mismatch spill
  draining, and constructor-invocation recovery.
- Lint gating: `no_duplicate_case_values` keyed on the patterns version
  face, rule names resolved case-insensitively.
- pub validation: git URL/path shape checks, `tag_pattern` marker rules,
  fail-closed escapes, and the stale-config advisory face.

### Performance

Measured numbers only — measured on the differential corpus, check face;
analyze-face numbers pending the 1.0.0 gate run:

- Small corpus, the m0 differential corpus (396 files): 17.4x wall-clock
  and 33x lower peak memory than `dart analyze` over the same face.
- 100k-tier generated corpus (99,598 files, ~5.4M LOC): 30.1x wall-clock
  and 38.7x lower peak memory (check face).
- Scanner micro-benchmark: 245 MB/s single-thread tokenization.

### Known limitations

- Semantics are pinned to the Dart 3.13.1 oracle: syntax features that
  newer SDKs ship behind experiment flags are diagnosed, never silently
  accepted.
- Incremental L3 fine-grained invalidation lands post-1.0.0.
- The supported daemon/LSP shell lands post-1.0.0; the 1.0.0 differential
  gates cover the `check` and `analyze` faces.

[1.0.0]: https://github.com/fluttercandies/dartq/releases/tag/v1.0.0
