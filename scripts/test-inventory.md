# Unit-Test Inventory — dartq workspace

Human-auditable companion to `coverage-gate.sh`. This inventory exists BEFORE
llvm-cov numbers land: it records which critical behaviors are already pinned
by tests, per crate, so the coverage gate has a reviewable baseline to compare
against once measured line coverage exists. Where llvm-cov says "80% covered",
this file says "covered of WHAT".

Built by recon (2026-10-09, R770 lane #285): per-crate `#[test]` counts over
`src/` and `tests/`, integration-test directory listing, differential-harness
pin reading (gate TSV/code counts), and test-function-name sampling. One line
per crate; treat the lines as pointers into the test files, not as proof of
depth — the gate measures, this file explains.

## Summary

- 23 Rust workspace members; **15 carry tests** (853 `#[test]` fns in crates)
  plus the differential harness (56 `#[test]` fns) = **909 pinned test fns**.
- 6 members are empty scaffolds (1-line `lib.rs`, no implementation): no
  behavior to cover, listed as GAPS with their unblocking condition.
- `dartq-plugin-host` is a Dart package (no Cargo.toml) — not applicable.

## Per-crate critical-path checklist

### Pinned (tests exist)

| crate | `#[test]` | critical paths pinned |
| --- | --- | --- |
| dartq-scanner | 89 | tokenization state machine: numbers (int/hex/dot runs), keywords, lexeme slicing; golden corpus pinned via `tests/corpus/scanner-goldens/manifest.json`; scanner diagnostic surface |
| dartq-parser | 178 | grammar coverage (literals, collections, records, patterns, types, members, statements, formal params, metadata), error recovery paths, feature gating (`features.rs`), cursor/listener event shapes |
| dartq-source | 62 | ContentId/intern table: idempotent + dense ids, existence-transition stability, canonicalization, lexical fallback; string-table memory bounds (integration `tests/string_table_memory.rs`) |
| dartq-pub | 98 | package_config / pubspec.lock consumption: version scanning (`scan_bare_version`, comparison ops), number parsing, config parsing |
| dartq-semantic | 60 | element resolution: bare names, containers, member rows, receiver rows, symbol interning |
| dartq-types | 80 | subtype relation pairs, nullability merges (`T?` edges), LUB, hierarchy checks, function subtyping, structural type interning canonicality (integration suites under `tests/`) |
| dartq-resolution | 56 | URI resolution (canonical base/segment joins), import/export namespace assembly, index construction |
| dartq-diagnostics | 35 | 258-row `syntax-codes.tsv` byte-pinned via blake3 (`tests/syntax_codes_tsv.rs`), code display format (`UPPERCASE_UNDERSCORE`), golden output (`tests/golden.rs`) |
| dartq-engine | 31 | discovery, pipeline, incremental edit chains (`tests/incremental.rs`, `incremental_oracle.rs`), §19.4/§19.5 byte-identity pins (`tests/reproducibility.rs`: repeated-run, cross-round, and incremental==full-recompute byte equality; engine version-key stability) |
| dartq-language | 32 | language-version parsing, version digit handling, feature registry (integration `tests/language.rs`) |
| dartq-ast | 30 | compact-AST access: `token_at`, indexed open/scan, token ranges; memory-footprint metrics (integration `tests/metrics.rs`) |
| dartq-sdk | 29 | libraries.json parsing, null-patch semantics, real/vendored SDK loading (integration `tests/load.rs`, `real_sdk.rs`, `vendored_sdk.rs` + fixtures) |
| dartq-package | 35 | package-name grammar, package table construction (integration `tests/package_name_grammar.rs`, `package_table.rs`) |
| dartq-cli | 19 | argument parsing, per-file scan/parse entry points |
| dartq-index | 19 | bare-name resolution index |
| tests/harness | 56 | differential gate plumbing: 258-row authoritative TSV vs 255-code gate-file pin (`gate_codes_file_matches_authoritative_tsv`), Jaccard compare semantics incl. empty-set/filtered-set rules, machine-format parsing, corpus assembly preflight (language-version guard), oracle driver lifecycle, report verdicts |

### GAPS

| crate | status | note |
| --- | --- | --- |
| dartq-flow | GAP (stub) | 1-line `lib.rs`; flow analysis unimplemented — pins land with the first real implementation (PRD §19.4 also demands signature-hash gate unit tests there) |
| dartq-snapshot | GAP (stub) | 1-line `lib.rs`; no implementation |
| dartq-lints | GAP (stub) | 1-line `lib.rs`; the 264-rule lint surface has zero pins |
| dartq-ai | GAP (stub) | 1-line `lib.rs`; no implementation |
| dartq-plugin | GAP (stub) | 1-line `lib.rs`; no implementation |
| dartq-schema | GAP (stub) | 1-line `lib.rs`; no implementation |
| dartq-plugin-host | N/A | Dart package (no Cargo.toml); outside Rust unit-test scope |

## Cross-crate gates (strongest pins)

- **258/255 count pin**: `crates/dartq-diagnostics/data/syntax-codes.tsv`
  (258 rows = 255 syntax codes + 3 version-family codes) is byte-audited by
  blake3 (`dartq-diagnostics/tests/syntax_codes_tsv.rs`) and the derived
  255-code gate file `tests/corpus/syntax-codes.txt` is re-derived and
  count-pinned in the harness (`tests/harness/src/main.rs`).
- **§19.5 output determinism**: `dartq-engine/tests/reproducibility.rs` pins
  byte-identical machine-format output across repeated runs, cross-round
  process state, and an incremental-vs-full-recompute chain.
- **Environment tuple**: `scripts/repro-tuple.sh` seals git HEAD + worktree
  patch hash + toolchain versions (+ optional run-output digest) per §19.5;
  `scripts/coverage-gate.sh` enforces measured coverage against the floor.

## Maintenance rule

Any crate added to the workspace gets a row here on the commit that adds it:
either a pinned-behavior line (with test locations) or an explicit GAP entry.
When a stub crate gains its first real implementation, its GAP row converts to
a pinned row in the same change.
