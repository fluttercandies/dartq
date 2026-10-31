# Test policy — behavior is proven by conformance; unit tests keep invariants

Status: audit + ruling (#2711). Read-only: this document codifies the
keep/migrate rule and inventories the existing test surface. It does NOT
delete or rewrite any test — migration is a follow-up wave.

## The rule

**Behavioral correctness is proven ONLY by conformance.** A test that
asserts a user-observable verdict — a diagnostic `(severity, code, range,
message)`, a lint row, a parse error, a feature verdict — is a claim the
oracle already answers through the `conformance/` corpus. Keeping that
claim in a crate unit test creates a second source of truth that drifts
silently the moment the oracle's answer changes (SDK bump, recovery-order
fix, message-text tweak).

**Crate unit tests keep ONLY invariants** — properties of *our*
implementation the oracle cannot see:

- encoding / serialization round-trips (snapshot, codec frames,
  persisted models),
- termination & bound invariants (caps latch, no panic, no unbounded
  growth),
- structural contracts (table sorted for binary search, registry
  completeness, interning stability, id allocation),
- internal-state facts conformance can't observe (flow facts,
  intermediate inference state, private-API shapes),
- resource/memory budgets and determinism of our own artifacts.

When the same property is both an invariant and oracle-visible, the
oracle face wins and the unit test is redundant — migrate it.

## Decision table

| Assertion shape | Example | Verdict |
|---|---|---|
| `report_ops(src)` + `code.name`/offset/len asserts | `ticket_*_tests.rs`, `*_tests.rs` in `diagnose/` | **MIGRATE** — behavior, oracle-visible |
| `analyze_files`/`engine.analyze` + diagnostic row asserts | `dartq-engine` `src/*_tests.rs`, `tests/semantic_gaps.rs` | **MIGRATE** — behavior, oracle-visible |
| `Fixture::parse`/`f.run(&one_rule)` + lint count/message/correction | `dartq-lints/src/rules/**` | **MIGRATE** — lint behavior, oracle-visible |
| `assert_codes`/`assert_kinds` on scan output | `dartq-scanner/src/tests/errors.rs` | **MIGRATE** (diag codes) / **UNCLEAR** (token-kind goldens — see exceptions) |
| diagnostic-multiset match on a corpus file (`*.expect`) | `dartq-parser/src/tests/gate_m0*.rs` | **UNCLEAR→KEEP-lite** — pins a *no-panic + multiset* contract on a fixed upstream file; migrate the multiset half to conformance parser cases, keep the no-panic invariant |
| `FlowFact`/`promoted_read_rows`/`facts` asserts | `dartq-flow` `ticket_*_tests.rs`, `record.rs` | **KEEP (exception)** — internal facts conformance can't observe |
| caps/termination/`is_bailed`/`MAX_DIAGNOSTICS` | `dartq-parser/src/tests/termination.rs` | **KEEP** — termination invariant |
| `is_subtype`/`lub`/`instantiate_to_bound` on a hand-built fixture | `dartq-types/tests/*`, `src/*` inline | **KEEP** — internal type-relation contract; oracle can't see `TypeId`s |
| codec frame round-trip, `read_frame` | `dartq-daemon/src/codec.rs` | **KEEP** — encoding round-trip |
| registry/table sortedness & completeness | `dartq-commands/src/fix_table.rs`, `lints/registry.rs` | **KEEP** — structural invariant |
| URI parse/directive shapes, file-role, cycles | `dartq-resolution/src/*` | **KEEP** — internal model contract |
| plugin discovery/manifest/session shapes | `dartq-plugin/src/*` | **KEEP** — internal contract |
| snapshot/persisted round-trip | `dartq-snapshot`, `engine/state/persist.rs` | **KEEP** — round-trip |
| `Fixture`/manifest/`workspace` scaffolding helpers | `dartq-fixture`, `dartq-test` | **KEEP** — harness, not a verdict |
| AI/explain/query/index token-tier output rows | `dartq-ai`, `engine` `query_index.rs` | **UNCLEAR** — machine-face output; no oracle. Keep as *contract* tests until a conformance face exists for them |

## Per-crate inventory

Counts are `#[test]` markers per face (measured, not estimated). "Migrate"
means the assertions re-prove oracle behavior; "keep" means the face is an
invariant or harness; "unclear" needs a per-test read (mixed file).

| Crate | tests | keep | migrate | unclear | Notes |
|---|---:|---:|---:|---:|---|
| dartq-semantic | 4177 | ~258 | ~3589 | ~330 | `diagnose/ticket_*` 2751 + `diagnose` other `*_tests.rs`/`tests.rs` 757 = `report_ops` diag asserts → migrate. `tests/` dir 81 → migrate. `src` non-diagnose inline 258 (collector/const_eval/persisted/ty_fold/model) → mostly keep; audit the few that assert `codes`. |
| dartq-lints | 1914 | ~15 | ~1899 | 0 | `src/rules/**` = `f.run(&one_rule)` + message/correction asserts → migrate. `tests/upstream_cases.rs` 15 = already a behavioral port of upstream corpus → keep as the migration *destination* shape (move to conformance when lint cases cover the rule). |
| dartq-parser | 1200 | ~120 | ~0 | ~1080 | `tests/termination.rs` + inline (63) → keep. `tests/gate_*.rs`/`ticket_*` (~1080) → mixed: no-panic invariant = keep; diagnostic-multiset parity on upstream corpus files = migrate to conformance parser cases (they already pin the same `.expect`). |
| dartq-flow | 722 | ~722 | 0 | 0 | All `FlowFact`/promotion/facts asserts — internal facts conformance can't observe → **exception class: keep**. |
| dartq-engine | 526 | ~284 | ~242 | 0 | `tests/` dir 242 (`semantic_gaps`, `ticket_*`, `reproducibility`…) = `analyze_files` diag rows → migrate. `src` inline 284 (budget/scheduler/state/persist/store/event) → keep. |
| dartq-commands | 519 | ~519 | 0 | 0 | fix_table sortedness/registry completeness, layout/config/resolve internals → keep. |
| dartq-daemon | 260 | ~260 | 0 | 0 | codec frames, rpc/session shapes → keep. |
| dartq-cli | 167 | ~167 | 0 | 0 | arg/output plumbing → keep. |
| dartq-resolution | 159 | ~159 | 0 | 0 | uri/namespace/cycles/file_role internal contracts → keep. |
| dartq-types | 157 | ~157 | 0 | 0 | subtype/lub/i2b on hand-built fixtures → keep (internal relation). |
| dartq-debug | 145 | ~145 | 0 | 0 | debug-session internals → keep. |
| dartq-scanner | 144 | ~40 | ~104 | 0 | `errors.rs`/`inserted.rs` `assert_codes` → migrate; `kinds.rs` keyword/token goldens + `feature_fold`/`sii_probes` invariants → keep/unclear (see exceptions). |
| dartq-pub | 144 | ~144 | 0 | 0 | pubspec/lock/workspace model → keep. |
| dartq-schema | 129 | ~129 | 0 | 0 | schema decode/validate → keep. |
| dartq-plugin | 107 | ~107 | 0 | 0 | discovery/manifest/session → keep. |
| dartq-source | 91 | ~91 | 0 | 0 | FileTable/offset math → keep. |
| dartq-test | 76 | ~76 | 0 | 0 | harness → keep. |
| dartq-diagnostics | 56 | ~56 | 0 | 0 | code registry/render → keep. |
| dartq-ast | 55 | ~55 | 0 | 0 | arena/layout → keep. |
| dartq-ai | 54 | 0 | 0 | ~54 | machine-face output, no oracle → unclear, treat as contract tests. |
| dartq-index | 50 | ~50 | 0 | 0 | index build/query → keep. |
| dartq-sdk | 42 | ~42 | 0 | 0 | sdk discovery/summary → keep. |
| dartq-language | 38 | ~38 | 0 | 0 | feature-set/version → keep. |
| dartq-snapshot | 37 | ~37 | 0 | 0 | snapshot round-trip → keep. |
| dartq-package | 36 | ~36 | 0 | 0 | package graph → keep. |
| dartq-session | 27 | ~27 | 0 | 0 | session state → keep. |
| dartq-fixture | 25 | ~25 | 0 | 0 | harness generator → keep. |
| dartq-plugin-host | 0 | 0 | 0 | 0 | — |
| **total** | **~10,700** | **~5,400** | **~5,830** | **~1,470** | |

## Migration order

Migrate the highest-drift surfaces first — the ones whose oracle answer is
most likely to change under an SDK bump or a recovery-order fix.

1. **dartq-semantic `diagnose/ticket_*_tests.rs` (2,751)** — the largest
   drift surface. These are `report_ops` pins written per ticket; every one
   restates an oracle verdict. Convert each pin's `(source → expected code
   rows)` into a `conformance/manual/diagnostic/<code>/` `.case` (the
   format already exists; `ticket_f6b` pins are the model: source +
   expected rows). Delete the `*_tests.rs` pin once the case is in the
   corpus and `status.tsv` shows the face covered.
2. **dartq-semantic `diagnose` other `*_tests.rs` + `tests/` dir (838)** —
   same harness, same destination.
3. **dartq-lints `src/rules/**` (1,899)** — the `prefer_double_quotes`-style
   per-rule message/correction asserts. The corpus already carries 6.5k
   lint cases + `tests/upstream_cases.rs` as the ported-fixture shape;
   migrate rule tests to `conformance/manual/` lint cases or the upstream
   fixture port, keyed by rule name.
4. **dartq-engine `tests/` dir (242)** — `analyze_files` diag-row asserts;
   most are end-to-end pins of a single code → conformance diagnostic
   cases. `reproducibility.rs`/`budget.rs`-style determinism+perf files
   inside `tests/` are the exception (keep — they're invariants).
5. **dartq-parser `tests/gate_*`/`ticket_*` (~1,080 unclear)** — split on
   read: keep only the no-panic/termination halves as invariants; move the
   diagnostic-multiset parity halves to conformance parser cases.
6. **dartq-scanner diag-code asserts (~104)** — migrate `assert_codes`
   rows to parser-face cases; keep token-kind goldens (see exceptions).
7. **dartq-ai / machine-face (~54)** — last; needs a conformance face
   (explain/impact/query) defined before migration is meaningful.

## Invariant exceptions — behavior tests conformance can't reach

These assert an internal or machine-level contract with no oracle
counterpart. They are **load-bearing** — do NOT migrate; losing them drops
real coverage.

- **dartq-flow `FlowFact` pins (all 722)** — `PromotedRead`,
  `BodyMayCompleteNormally`, label-id stability, type-base
  classification. Conformance sees the *consequence* (a diagnostic
  downstream), never the fact; a regression that produces the same
  diagnostic via a wrong fact is invisible to the corpus.
- **`test_rig`/`testsupport`/`tests/common` harnesses** — `report_ops`,
  `Fixture`, `assert_doc`, `CORE_PRELUDE`. Infrastructure, not verdicts.
- **Termination/cap invariants** — `parser/tests/termination.rs`,
  `MAX_DIAGNOSTICS` latch, `is_bailed`, `TYPE_DEPTH_CRASH_GUARD`. The
  oracle answers "which diagnostics"; it cannot answer "does it stop."
- **Perf/resource budgets** — `engine/tests/budget.rs`,
  `memory_raw_sites.rs`, RSS/budget pins; `ASSIGNMENT_TARGET_VISITS`-style
  instruction-count faces. Conformance measures verdicts, not cost.
- **Registry/table structural invariants** — `fix_table` ascending-order
  (binary-search precondition), registry-code completeness, `code_info`
  name pins. A wrong sort order is a correctness bug with zero oracle
  signal.
- **Encoding/persistence round-trips** — snapshot, codec frames,
  persisted model, `FileTable` interning. Byte-for-byte contracts of our
  own artifacts.
- **Determinism** — `engine/tests/reproducibility.rs` (same input →
  byte-identical output) is an invariant, not an oracle verdict.
- **dartq-scanner token-kind goldens** (`kinds.rs`, `feature_fold.rs`) —
  the keyword↔kind table and lexeme mapping is our scanner's structural
  contract; conformance's parser cases only see downstream diagnostics.
  The `assert_codes` *diagnostic* rows in `errors.rs`/`inserted.rs`
  DO migrate — those are oracle-visible.
- **dartq-ai / machine-face contract tests** — until a conformance face
  (explain/impact/query/index/token-tier) is defined, these are the only
  coverage. Treat as unclear-pending-a-face, not as behavior to delete.

## Notes

- `conformance/` is the single behavioral oracle: `cases.manifest`
  (43.6k cases — diagnostics 16.4k, features 14.2k, lints 6.5k, parser
  5.5k, parser-fe 937), `expected.tsv`, `status.tsv`, plus
  `conformance/manual/**` hand-written `.case`s for faces the extracted
  corpus misses.
- Migration = move the assertion into the corpus, confirm `status.tsv`
  covers the face, then delete the unit pin. Never delete first.
- A behavior test that has NO conformance face and NO internal-invariant
  justification is a *gap in the corpus*, not a reason to keep the test
  — file the `.case` first, then delete.
