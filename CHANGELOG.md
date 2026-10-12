# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-10-12

Initial release: a semantic analysis engine for Dart and Flutter, written in
Rust, validated by differential testing against `dart analyze` as the oracle.

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
- **Lint rules** — the `package:lints` core and recommended rule sets
  implemented and wired into the analyze face, with enable sets resolved
  from `analysis_options.yaml` include chains.
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
- **Differential-testing harness** — oracle vs dartq over shared corpora
  with `(code, file)` Jaccard comparison, auto-bisect, replay/edit-replay
  drivers, benchmark arms, corpus tooling, and per-gate scripts.
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

[0.1.0]: https://github.com/fluttercandies/dartq/releases/tag/v0.1.0
