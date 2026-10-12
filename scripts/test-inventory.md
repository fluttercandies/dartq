# Unit-Test Inventory — dartq workspace

Human-auditable companion to `coverage-gate.sh`. This inventory exists BEFORE
llvm-cov numbers land: it records which critical behaviors are already pinned
by tests, per crate, so the coverage gate has a reviewable baseline to compare
against once measured line coverage exists. Where llvm-cov says "80% covered",
this file says "covered of WHAT".

Built by recon (2026-10-09, R770 lane #285) and refreshed 2026-10-12 after the
#943 M4 recon falsified six GAP rows: every former stub crate (flow, snapshot,
lints, ai, plugin, schema) now carries a real implementation and a test
suite, two members were missing rows entirely (commands, daemon), and every
count below has been re-derived. Rows are pointers into the test files, not
proof of depth — the gate measures, this file explains.

## Derivation

Every number in this file is a static count; no test bodies are executed.

- `#[test]` counts: `grep -rh --include='*.rs' -c '#\[test\]' <crate-dir>/`
  summed over the crate's `src/` and `tests/` (a read of the test-module
  declarations). Rows are sorted by that count, descending.
- Test-file/suite citations: directory listing of the crate's `tests/` plus
  the `mod tests` / test-function declarations in each cited file.
- Workspace member list: the `members` array in the root `Cargo.toml`.
- Cross-check for a single crate (compiles the lib, does not run test
  bodies): `cargo test -p <crate> --lib -- --list --format=terse`.
- Never run for this file: workspace-wide tests, `cargo run`, bench,
  `dart analyze` on repo code, full test bodies.

## Summary

- 25 workspace members: 23 Rust crates plus `tests/harness` and
  `tools/corpus-gen`. **All 23 Rust crates carry tests** (3,004 `#[test]`
  fns in crates) plus the differential harness (128 `#[test]` fns) and the
  corpus generator (6) = **3,138 pinned test fns**.
- No empty-scaffold crates remain. The six stubs of the 2026-10-09 recon
  (flow, snapshot, lints, ai, plugin, schema) all shipped and are pinned
  below; their GAP rows are gone.
- `dartq-plugin-host` is a Dart package (no `Cargo.toml`) — not applicable
  to the Rust gate. It carries its own Dart suite: 10 test files under
  `test/` with 97 `test(` occurrences (`grep -h -c 'test(' test/*.dart`).

## Per-crate critical-path checklist

### Pinned (tests exist)

| crate | `#[test]` | critical paths pinned |
| --- | --- | --- |
| dartq-parser | 535 | grammar coverage (literals, collections, records, patterns, types, members, statements, formal params, metadata), error recovery paths, feature gating (`features.rs`), cursor/listener event shapes |
| dartq-lints | 473 | registry pins (integration `tests/registry_pin.rs`, 12: unique rule names, official-set membership, core/recommended disjointness, pinned registry count, code-string freeze, message-template well-formedness); per-rule behavior suites in the 12 rule modules under `src/rules/` (names, declarations, expressions, collections, types, strings, docs, directives, pubspec, regexp, empty, flow) plus engine/bridge/comments/ignore machinery |
| dartq-semantic | 388 | element resolution: bare names, containers, member rows, receiver rows, symbol interning; M3 diagnostic-code pin (integration `tests/m3_codes_pin.rs`) |
| dartq-engine | 189 | discovery, pipeline, incremental edit chains (`tests/incremental.rs`, `incremental_oracle.rs`), §19.4/§19.5 byte-identity pins (`tests/reproducibility.rs`: repeated-run, cross-round, and incremental==full-recompute byte equality; engine version-key stability), memory accounting (`tests/memory_accounting.rs`) |
| dartq-flow | 152 | flow analysis over parser events: reachability/dead-code span composition (returns, `if`/`else`, switch members, loop bodies, short-circuit operand rows), definite assignment incl. `late` (zero-iteration loops, `break`/`continue` edges), type promotion/demotion chains; record layer label-id stability and type-token base classification (`src/tests.rs` 150, `src/record.rs` 2) |
| dartq-daemon | 151 | LSP shell: lifecycle smoke, overlay sessions (`tests/overlay_session.rs`), RPC command/query faces (`tests/rpc_commands.rs`, `rpc_query_batch.rs`, `rpc_query_kinds.rs`), schema conformance (`tests/rpc_schema.rs`), multi-agent read pool (`tests/seven_agents.rs`); plugin RPC (`src/plugins.rs`) |
| dartq-types | 136 | subtype relation pairs, nullability merges (`T?` edges), LUB, hierarchy checks, function subtyping, structural type interning canonicality (integration suites under `tests/`) |
| dartq-scanner | 119 | tokenization state machine: numbers (int/hex/dot runs), keywords, lexeme slicing; golden corpus pinned via `tests/corpus/scanner-goldens/manifest.json`; scanner diagnostic surface |
| dartq-pub | 113 | package_config / pubspec.lock consumption: version scanning (`scan_bare_version`, comparison ops), number parsing, config parsing |
| dartq-schema | 112 | v1 frozen output faces: real reports validate and every drift is named (integration `tests/alignment.rs`, 28: version constants align with `dartq-ai`, tier invariance, envelope/row drift naming; `tests/output_faces.rs`, 42: diagnostics/fix/query/compat/assists faces, v1 freeze assertion); per-face validator unit tests in `src/` (42) |
| dartq-resolution | 92 | URI resolution (canonical base/segment joins), import/export namespace assembly, index construction |
| dartq-commands | 86 | the per-command cores shared by CLI and daemon: discover, check, config, analyze, query, explain, impact, lints, index, layout (unit `mod tests` per face module in `src/`) |
| dartq-plugin | 81 | plugin protocol: frame codec bounds (16 MiB cap, chunked push, oversize rejection before allocation), manifest parsing + budget validation, handshake/capability intersection, discovery conventions (unit tests in `src/`, 45); 20 integration suites: live-host round trips against the Dart host package (`live_host.rs`, `compat_live.rs`, `e2e_round_trip.rs`, `plugin_discovery.rs`), budget-enforcement kills (`handshake_timeout.rs`, `analyze_timeout.rs`, `callback_timeout.rs`, `analyze_clock_reopen.rs`, `shutdown_timeout.rs`, `write_timeout.rs`, `memory_budget.rs`), crash/restart and hang attribution (`crash_restart.rs`, `relaunch_hang.rs`, `shutdown_hang.rs`, `wait_dead_generation.rs`, `reader_eof_death_attribution.rs`), protocol violations (`protocol_violation.rs`, `outbound_frame_guard.rs`, `startup_stderr.rs`) |
| dartq-source | 66 | ContentId/intern table: idempotent + dense ids, existence-transition stability, canonicalization, lexical fallback; string-table memory bounds (integration `tests/string_table_memory.rs`) |
| dartq-cli | 46 | argument parsing, per-file scan/parse entry points |
| dartq-diagnostics | 37 | data-table byte-pin via blake3 (`tests/syntax_codes_tsv.rs`), code display format (`UPPERCASE_UNDERSCORE`), golden output (`tests/golden.rs`) |
| dartq-package | 36 | package-name grammar, package table construction (integration `tests/package_name_grammar.rs`, `package_table.rs`) |
| dartq-ai | 35 | agent-facing queries: compatibility axis matrix, explain symbol report, impact reverse-dependency closure + test-graph join + environment matrix, shared CSR reverse-import-graph build (unit `mod tests` in `src/{compatibility,explain,impact,rev_import_graph}.rs`) |
| dartq-snapshot | 34 | the accelerator invariant — every load-time failure demotes to a clean miss, never a partial generation: header/tables damage, truncated/corrupt/missing/extra/relabeled entries, format/semantic-version/SDK/package-graph/salt mismatches; atomic saves, temp-file isolation, capacity eviction incl. symlink-cycle termination (`src/tests.rs` 33, `src/entry.rs` 1) |
| dartq-ast | 33 | compact-AST access: `token_at`, indexed open/scan, token ranges; memory-footprint metrics (integration `tests/metrics.rs`) |
| dartq-language | 32 | language-version parsing, version digit handling, feature registry (integration `tests/language.rs`) |
| dartq-sdk | 30 | libraries.json parsing, null-patch semantics, real/vendored SDK loading (integration `tests/load.rs`, `real_sdk.rs`, `vendored_sdk.rs` + fixtures) |
| dartq-index | 28 | bare-name resolution index |
| tests/harness | 128 | differential gate plumbing: authoritative TSV vs 255-code gate-file pin (`gate_codes_file_matches_authoritative_tsv`), Jaccard compare semantics incl. empty-set/filtered-set rules, machine-format parsing, corpus assembly preflight (language-version guard), oracle driver lifecycle, report verdicts, bisect/repro/bench drivers |
| tools/corpus-gen | 6 | corpus generator determinism: same seed is byte-identical, seed change alters the fingerprint, plan hits scale targets/distribution and yields sorted unique paths, feature-matrix coverage at 1k, cell tables stay in sync (`src/tests.rs`) |

### GAPS

None. No Rust crate is an empty scaffold. `dartq-plugin-host` remains N/A to
the Rust gate as the workspace's only non-Cargo member (see Summary).

## Cross-crate gates (strongest pins)

- **259/255 count pin**: `crates/dartq-diagnostics/data/syntax-codes.tsv`
  carries 259 data rows — the 255 scanner/parser gate codes plus 4
  supplemental codes that live only in the authoritative table (the
  language-version-override trio and
  `invalid_covariant_modifier_in_primary_constructor`). Counts derived with
  `awk '!/^#/ && NF>0' | wc -l`; the gate file
  `tests/corpus/syntax-codes.txt` carries exactly the 255 codes. The table
  is byte-audited by blake3 (`dartq-diagnostics/tests/syntax_codes_tsv.rs`)
  and the gate file is re-derived and count-pinned in the harness
  (`tests/harness/src/main.rs`, `gate_codes_file_matches_authoritative_tsv`).
- **§19.5 output determinism**: `dartq-engine/tests/reproducibility.rs` pins
  byte-identical machine-format output across repeated runs, cross-round
  process state, and an incremental-vs-full-recompute chain.
- **Environment tuple**: `scripts/repro-tuple.sh` seals git HEAD + worktree
  patch hash + toolchain versions (+ optional run-output digest) per §19.5;
  `scripts/coverage-gate.sh` enforces measured coverage against the floor.

## Maintenance rule

Any crate added to the workspace gets a row here on the commit that adds it:
either a pinned-behavior line (with test locations) or an explicit GAP entry.
When a stub crate gains its first real implementation, its GAP row converts
to a pinned row in the same change.
