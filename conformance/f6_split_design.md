# F6 split-design — invalid_assignment / argument_type_not_assignable subtickets

**Input:** the F6 row extract (3,585 rows; re-derived from the
full-corpus report snapshot taken 2026-10-30 20:14). Recon only —
no `crates/**` edits, no conformance/oracle runs. Per `wave_b_report.md` §F6,
this family is multi-seam; below is the dispatchable split.

**Live-lane ownership (per brief):**
| file(s) | owner |
|---|---|
| `assignability.rs` + `types.rs` | #3202 |
| `invocation.rs` + `expressions.rs` | #2272 + #2990 |
| `engine_core.rs` + `resolution.rs` | #2999 |
| `row_resolution.rs` | #2990 |
| `extension_types.rs` | #3198 |
| `scopes_bodies.rs` + `unused_finalization.rs` + `collector.rs` | #3044 |
| `ffi.rs` | #3205 |
| `parser/**` | #2978 + #3054 + #3199 |
| `dartq-lints/**` | #3107 |
| `fixture+engine+commands` | #3037 |

`call_result.rs`, `send_result.rs`, `member_face.rs`, `operand_face.rs`,
`collection_literal.rs`, `creation_args.rs`, `flow_scope_tests.rs`,
`receiver_face.rs`, `index_result.rs`, `type_capture.rs` are unlisted →
presumed free (verify against `goal list wip` at dispatch).

---

## Subticket table (ranked by yield)

| ID | Title | rows | side split | dominant shape | root seam | oracle pin | live-lane block |
|---|---|---:|---|---|---|---|---|
| F6-A | I2B / super-bounds bound conformance | ~1,391 | missing-only | `co19/Instantiate-to-bound/*` + `co19/Simple-bounds/*` — `invalid_assignment` on `F<A<...>>` vs `F<A<...<Null>>>` / `F<A<FutureOr<...>>>` | bound-conformance between recursive `Fcov`/`FutureOr`/function args — *same root as wave-B F3* (the `type_argument_not_matching_bounds` family); the two code paths share a single predicate in `types.rs`/`assignability.rs` | upstream `pkg/analyzer` `type_system.dart: isSubtypeOf` on `InterfaceType` arguments + `pkg/_fe_analyzer_shared` `typeAnalyzer` bound check; oracle emits both `invalid_assignment` AND `type_argument_not_matching_bounds` from the same recursive bound predicate | **YES — `types.rs`+`assignability.rs` are lane #3202** |
| F6-B | Downward inference leaves `<dynamic>` | ~150 | missing+extra | `diagnostics/_misc/src/task/strong/inferred_type/InferredTypeTest/*` (84 missing + 9 extra), `StrongModeLocalInferenceTest` (16 missing + 20 missing `argument_type_not_assignable`), `CouldNotInferTest` (8 missing) | `call_result.rs` `instantiate_call_return` — the in-code comment pins it: *"until the call-site inference solves it, the under-report direction is the only sound stand-in"* → call-site inference is unimplemented; `invocation.rs` `close_arguments` decides `argument_type_not_assignable` on declared types only | `pkg/analyzer` `generic_inferrer.dart` — `inferFunctionExpressionInvocation`/`inferMethodInvocation` produces `C<int>` not `C<dynamic>` | **partial — `invocation.rs` lane #2272+#2990; `call_result.rs` free** |
| F6-C | `.call` desugar + constructor tear-offs | ~100 | mostly missing | `diagnostics/invalid_assignment/InvalidAssignment_ImplicitCallReferenceTest/*` (12 missing — `num Function(num)` vs `String Function(String)`, `void Function(int, dynamic)` vs `void Function(bool, String)`), `co19/Constructor-tear-offs/*` (~53 missing `argument_type_not_assignable` + ~30 missing `invalid_assignment`) | callable-object `.call` resolution — when `obj` has a `.call` method the assignability question should run against `obj.call`'s signature, not `obj`'s interface type; `extension_types.rs` and the tear-off face in `invocation.rs` are adjacent | `pkg/analyzer` `assignment_expression_resolver.dart` — implicit `.call` rewrite on assignment targets; `constructor_tearoffs.dart` resolver | **YES — `extension_types.rs` lane #3198; `invocation.rs` lane #2272+#2990** |
| F6-D | `await` / `Future` unwrapping | ~75 | extra | `features/_core/async/language/async/await_test_02` (~25 extra, all `"num can't be assigned to int"` — `await` returns `num` where oracle returns `int`), `features/non-nullable/*` (~15), `language/await_type_test` (~6) | `await` on `Future<int>` should produce `int`, dartq produces `num`/wider — the `flatten(Future<T>) → T` rule isn't applied at expression-eval; `member_face.rs`/`send_result.rs`/`row_resolution.rs` adjacent | `pkg/analyzer` `await_expression_resolver.dart` — `flatten` on `Future<T>` | **YES — `row_resolution.rs` lane #2990** |
| F6-E | Function-type return covariance | ~80 | extra | `features/sound-flow-analysis/language/sound_flow_analysis/equality_comparisons_*` + `if_null_*` + `null_aware_access_*` (~82 extras — `"void Function() can't be assigned to Null/int Function()"`) | `void` as return-type is *not* a bottom for assignability in the direction dartq takes — `void Function()` IS assignable to `int Function()` in covariant-return position (return-type covariance); `types.rs` subtyping on `FunctionType.returnType` | `pkg/analyzer` `type_system.dart` — `FunctionType` subtype rule ignores return-type direction per Dart's `void`/`dynamic`/`Object?` corner | **YES — `types.rs` lane #3202** |
| F6-F | Record-shape / `InvalidType` leak in record types | ~76 | mixed | `co19/LanguageFeatures/Records/record_type_annotations_A0{2,4}_*` (dartq emits `InvalidType` in rendered record types), `co19/LanguageFeatures/Static-access-shorthand/grammar_A01_t08` | record-type field-name resolution — dartq's `InvalidType` sentinel leaks into the message where oracle renders the actual field name/type; type-pretty-printer / record-shape classifier | `pkg/analyzer` `record_type.dart` — `RecordTypeImpl.getDisplayString` uses resolved field types, never `InvalidType` | **partial — `types.rs` lane #3202** |
| F6-G | Flow promotion → `Null` boundary | ~120 | extra | `features/inference-update-3/language/inference_update_3/if_null_assignment_*` (~64 extras — `"Null can't be assigned to int"`), `features/sound-flow-analysis/*` (~30 extras), `features/this-promotion/*`, `features/non-nullable/*` | `??=` / `?.` / `if (x==null) return` promotion isn't reaching the assigned-expression type — dartq infers `Null` where oracle promotes to the non-null branch type; `scopes_bodies.rs` + `flow_scope_tests.rs` region | `pkg/analyzer` `flow_analysis.dart` — `FlowAnalysis.assign`/`nullAwareAccess_end` promotion | **YES — `scopes_bodies.rs` lane #3044** |
| F6-H | Variance annotations on generic params | ~52 | missing | `features/variance/language/variance/variance_in_inference_error_test` (~21 inv + 17 arg missing) — `Contravariant<Lower>` vs `Contravariant<Upper>`, `Invariant<Middle>` vs `Invariant<Object>`, `Exactly<...>` | `in`/`out`/`inout`/`exactly` variance declaration on `typedef`/generic-parameter shapes — subtype check doesn't honour declared variance, treats all params as covariant | `pkg/_fe_analyzer_shared` `variance.dart` + `pkg/analyzer` `type_system.dart` — `isSubtypeOf` uses declared-variance map | **YES — `types.rs` lane #3202** |
| F6-I | `FutureOr` shape (shares F6-A seam) | ~62 | mostly missing | scattered `language/least_upper_bound/*`, `language/nonfunction_type_aliases/usage_futureor_error_test`, `lib/test.dart` — `FutureOr<List<dynamic>>` vs `FutureOr<List<int>>` | `FutureOr<T>` unwrapping during assignability — `FutureOr<int>` should decompose to `Future<int> \| int` for the subtype question; same recursive-bound predicate as F6-A | `pkg/analyzer` `type_system.dart` `isSubtypeOf` — `FutureOr` case | **YES — `types.rs` lane #3202** |
| F6-J | Residual basic type-vs-type | ~1,000 | mixed | long tail: `regress/language`, `operator/language`, `compile_time_constant`, `least_upper_bound`, `mixin_legacy`, `typedef`, `prefix`, `type_variable`, `covariant`, `unsorted`, `argument`, `parameter`, `call`, `map`, `generic`, `await_type_test` — each ≤30 rows | **not one seam** — these are mostly downstream artifacts: a wrong inferred type at a leaf propagates into dozens of `invalid_assignment`/`argument_type_not_assignable` verdicts. Will partially collapse once F6-A..F6-I land; residual after that is a follow-up recon. | — | **mixed** |

---

## Dispatch order

1. **F6-A (I2B bound conformance)** — 1,391 rows, single predicate, but
   **BLOCKED on #3202**. Dispatch AFTER #3202 lands; combined with F3
   (type_argument_not_matching_bounds) and F6-I (FutureOr) into one
   `bounds_conformance` ticket — all three ride the same `isSubtypeOf`
   recursive-bound path.
2. **F6-B (downward inference)** — ~150 rows, `call_result.rs` is free.
   The `instantiate_call_return` `Unknown`-standin comment pinpoints the
   seam. Can dispatch now (call_result.rs only); the `invocation.rs`
   `close_arguments` face stays under #2272+#2990.
3. **F6-G (flow promotion → Null)** — ~120 rows but **BLOCKED on #3044**.
   Defer until scopes_bodies lane lands.
4. **F6-E (function-type return covariance)** — ~80 rows, **BLOCKED on
   #3202**. Bundle with F6-A when dispatching — both are `types.rs`
   subtype rules.
5. **F6-C (`.call` desugar)** — ~100 rows, **BLOCKED on #2272+#2990 and
   #3198**. Defer.
6. **F6-D (await/Future flatten)** — ~75 rows, **BLOCKED on #2990** (the
   `member_face`/`row_resolution` chain). Defer.
7. **F6-F (record-shape `InvalidType`)** — ~76 rows, partially blocked on
   `types.rs` #3202; the *record-pretty-printer* half may sit in
   `engine_core.rs` #2999. Defer pending owner clarity.
8. **F6-H (variance annotations)** — ~52 rows, **BLOCKED on #3202**.
   Bundle into F6-A mega-ticket (types.rs subtype lattice).
9. **F6-I (FutureOr)** — ~62 rows, merge into F6-A.
10. **F6-J (residual)** — re-audit *after* F6-A..F6-I land; expected to
    shrink significantly. Do not dispatch standalone.

## Immediate actionable (unblocked lanes)

Only **F6-B (`call_result.rs`)** is fully unblocked today. Everything
else either needs `types.rs`/`assignability.rs` (#3202), `invocation.rs`
(#2272+#2990), `row_resolution.rs` (#2990), `extension_types.rs` (#3198),
or `scopes_bodies.rs` (#3044).

**Recommendation**: dispatch a single `bounds_conformance` mega-ticket
(F6-A + F6-I + F3) after #3202 lands — it's the single highest-yield
seam in wave-B (~2,000+ rows including the F3 missing side). Until then,
F6-B's call-site inference work is the only zero-blocking dispatch.

## Cross-references

- Wave-B report: `conformance/wave_b_report.md` §F6.
- Bound-conformance overlap: `wave_b_report.md` §F3
  (`type_argument_not_matching_bounds`) — the same predicate in
  `types.rs` produces both codes; one ticket should close both families.
- InvalidType leaks: also observed in `Records/record_type_annotations_A04`
  — shares the pretty-printer seam with F6-F.
