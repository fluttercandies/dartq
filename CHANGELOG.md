# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Dart semantic analysis engine in Rust, targeting CLI and CI usage as a
  `dart analyze` replacement with AI coding agents as first-class users.
- Full Dart 3.15 frontend: single-pass scanner, fault-tolerant
  recursive-descent parser, and a compact arena AST.
- Semantic layers: type system with canonical interning and subtyping,
  import/export/part resolution, element/symbol arenas, event-driven flow
  analysis, and 64 built-in core/recommended lint rules.
- `dartq` CLI with eleven subcommands (`analyze`, `check`, `compat`,
  `daemon`, `explain`, `fix`, `assists`, `index`, `impact`, `query`,
  `version`) and machine-readable output byte-compatible with the oracle.
- LSP 3.17 daemon over stdin/stdout: whole-file overlay edits, pushed
  diagnostics, batched turns, and `dartq/*` extension methods.
- Plugin protocol v1 wire types (frames, codec, handshake, manifest) and
  the host-side Dart package.
- Differential-testing harness: `dart analyze` as the oracle over shared
  corpora, `(code, file)` Jaccard comparison, M0 syntax and M1 semantic
  acceptance gates.
- Benchmark harness and oracle baseline measurement (wall-clock, peak RSS,
  corpus scale), plus `dartq.yaml` workspace configuration.
