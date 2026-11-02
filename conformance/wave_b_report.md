# Wave-B diff-family report — types / assignability / resolver sweep

**Input:** the census report table (43,959 cases, snapshot 2026-10-30 20:14 — the newest
full snapshot, see `baseline_now.md`). Re-classified existing diff rows; no fresh sweep
launched. Wave-B covers the type-system, assignability and resolver-cascade diff
families (66 codes, enumerated in the wave-B code list produced by the census sweep).

**Filter:** rows where `code ∈ wave_b_codes` from fail cases, extracted into the wave-B row table.
(33,448 diff rows total: 10,261 missing / 23,187 extra).

**Global context (unchanged since #3017 baseline):**
- cases pass 26,041 / 43,959 (ROW Jaccard 0.598)
- Top-10 row-volume `(severity, code)` pairs account for ~57% of all wave-B diffs.

---

## Headline table — top wave-B families by diff-row volume

| rows | side | severity | code | dominant file shape | hypothesised root seam |
|---:|---|---|---|---|---|
| 5,790 | extra | ERROR | `undefined_operator` | `co19/Utils/expect_common.dart` | aux-file receiver-typing (`[]` on `''` / unresolved) — single cascade root |
| 2,433 | extra | ERROR | `unchecked_use_of_nullable_value` | `language/**` | dartq calls `expectStaticType` on nullable `Never`/extension receiver — flow promotion not modelled |
| 1,974 | missing | ERROR | `invalid_assignment` | `lib/test.dart` + `language/**` + `co19/**` | upward inference / record / `dynamic` vs generic bound — multi-seam |
| 1,450 | missing | ERROR | `type_argument_not_matching_bounds` | `language/generic/super_bounded_types_error{,2}_test.dart` | super-bounded / `FutureOr` recursive bound check |
| 1,001 | missing | WARNING | `dead_code` | `language/**` + `lib/test.dart` | flow-analysis reachability + span-shape mismatch |
| 875 | extra | ERROR | `expected_token` | `lib/test.dart` (`parser/src/fasta/recovery/partial_code`) | parser recovery position shift + extra recovery markers |
| 870 | extra | WARNING | `dead_code` | `language/void/void_type_usage_test.dart` + object-pattern | dartq emits multiple short spans vs oracle's one tail-span |
| 798 | missing | WARNING | `invalid_null_aware_operator` | `co19/LanguageFeatures/Control-flow-collections/Null-aware-elements/*` | null-aware element promotion — receiver-non-null after `?` elision |
| 789 | missing | WARNING | `unnecessary_type_check` | `co19/LanguageFeatures/Spread-collections/*` + `language/identifier/known_prefix*` | type-promotion `is`-check reachability |
| 703 | extra | ERROR | `duplicate_definition` | `co19/LanguageFeatures/Unquoted-imports/keywords_A01_t0{1,2}.dart` + `lib/test.dart` + `parser-fe/primary_constructors/*` | top-level decl dedup across unquoted imports |
| 678 | extra | ERROR | `type_argument_not_matching_bounds` | `language/generic/super_bounded_types_error_test.dart` + `co19/Instantiate-to-bound/*` | same root as missing row above — dartq adds phantom bound violations |
| 585 | missing | ERROR | `expected_token` | `lib/test.dart` (`parser/src/fasta/recovery/partial_code`) | parser recovery column drift |
| 565 | missing | ERROR | `argument_type_not_assignable` | `language/**` + `co19/**` | inference of record/named-parameter shape |
| 544 | extra | ERROR | `argument_type_not_assignable` | `language/**` + `lib/test.dart` | inference of record/named-parameter shape (paired with missing) |
| 538 | missing | ERROR | `invalid_use_of_type_outside_library` | `co19/LanguageFeatures/Class-modifiers/basic_restrictions_*` + `syntax_sealed_class_*` | sealed/base/final transitivity not modelled |
| 528 | missing | ERROR | `use_of_void_result` | `language/void/void_type_usage_test.dart` | `void` value-usage reachability — flow gap |
| 502 | extra | ERROR | `invalid_assignment` | `lib/test.dart` (InferredTypeTest) | dartq adds phantom generic mismatches on `A<dynamic>` vs `A<int>` |
| 384 | extra | ERROR | `return_of_invalid_type` | `language/dot_shorthands/dot_shorthand_helper.dart` | dot-shorthand static-type resolution — return-position inference |
| 363 | extra | ERROR | `unexpected_token` | `lib/test.dart` (partial_code recovery) | parser recovery — emits `'}'`/`':'` extras |
| 336 | missing | ERROR | `missing_identifier` | `lib/test.dart` (partial_code recovery) + `parser-fe/error_recovery/keyword_named_*` | parser recovery — drops identifier on keyword-as-identifier |

---

## Named families (root-seam buckets)

### F1 — `aux_file_receiver_typing` (single root, 5,790 extra rows)
- `(code,severity)`: `ERROR|undefined_operator`
- `file`: `co19/Utils/expect_common.dart`, lines **316** and **320** only.
- `message`: `The operator '[]' isn't defined for the type ''.`
- `cases affected`: 2,895 (entire `co19/LanguageFeatures/**` corpus shares this aux).
- `example`: `features/_core/co19/LanguageFeatures/co19/LanguageFeatures/Patterns/*`
  — every co19 case that pulls in `expect_common.dart` adds exactly two extra
  `undefined_operator` rows at `expect_common.dart:316` and `:320`.
- **Hypothesis**: dartq's resolver sees the receiver of `[]` at lines 316/320 as
  `''` (unresolved / `InvalidType`), so `operator[]` lookup fails. Same root as
  the one flagged in earlier recon (`recon_invalid_assignment.md` bucket 3):
  `row_resolution.rs` receiver typing on aux fixture. One fix flips 2,895 cases.
- **Suggested ticket scope**: `crates/dartq-diagnose/src/row_resolution.rs` receiver
  typing — `expect_common.dart` uses `Expect.throws()` helpers whose closure args
  land on `InvalidType` when the closure is unresolvable.

### F2 — `expect_static_type_promotion` (2,186 extra + 247 missing rows)
- `(code,severity)`: `ERROR|unchecked_use_of_nullable_value` (and a smaller
  `missing` tail for the paired `receiver_of_type_never`).
- `file`: `language/**/*.dart` (the language-test corpus).
- `message`: `The method 'expectStaticType' can't be unconditionally invoked
  because the receiver can be 'null'.` — also `'foo'`, `'clamp'`, `'unary-'`,
  `'+'`, `'%'`, `'&'`, `'remainder'`, `'abs'`, `'first'`, `'isEven'` variants.
- `cases affected`: 182.
- `example`: `features/_core/async/language/async/derive_future_type_test`
  (lines 132–483 — dartq emits 40+ `expectStaticType` errors on a single file
  where oracle emits none).
- **Hypothesis**: `expectStaticType` is a `package:expect` extension method used
  throughout `language/**` tests as `x.expectStaticType<T>()`. dartq resolves the
  receiver as nullable even though the test pattern is `x?.expectStaticType` or
  after a `!` — i.e. null-promotion on the receiver is not applied inside
  extension-method invocation position. The same seam likely feeds `null_aware`
  and `is`/`as` promotion in F8/F9.
- **Suggested ticket scope**: `crates/dartq-diagnose/src/type_resolver.rs` /
  extension-method receiver-promotion path; also `flow_analysis` for `?.` chains.

### F3 — `super_bounded_recursive_bound` (1,450 missing + 678 extra rows)
- `(code,severity)`: `ERROR|type_argument_not_matching_bounds`.
- `file`: `language/generic/super_bounded_types_error{,2}_test.dart` (896 rows
  across two files), `co19/LanguageFeatures/Instantiate-to-bound/*` (300+ rows),
  `co19/LanguageFeatures/Augmentations/type_annotation_inheritance_*`.
- `message`: `'A<A<...<T>>>' doesn't conform to the bound 'A<A<...<X>>>' of the
  type parameter 'X'.`
- `cases affected`: 2 (super_bounded_tests) + ~30 (Instantiate-to-bound typedef
  family).
- `example`:
  `language/generic/super_bounded_types_error_test.dart:100:21` —
  `'Object' doesn't conform to the bound 'dynamic Function(Object)'`
- **Hypothesis**: recursive / nested generic bounds are not unfolded the way
  oracle does. Both *missing* (we under-report) and *extra* (we over-report)
  point at the same predicate — bound conformance between `Fcov<T>` /
  `FutureOr<Object>` / `Function(Object)` shapes.
- **Suggested ticket scope**: `crates/dartq-diagnose/src/bounds.rs` /
  `type_check.rs` — recursive bound conformance + `FutureOr` unwrap.

### F4 — `recovery_position_shift` (932 extra + ~500 missing rows, parser-fe)
- `(code,severity)`: `ERROR|expected_token`, `ERROR|unexpected_token`,
  `ERROR|missing_identifier`, `ERROR|expected_body`.
- `file`: `lib/test.dart` cases under `parser/src/fasta/recovery/partial_code/**`
  plus `parser-fe/error_recovery/keyword_named_*`.
- `example`: `parser/src/fasta/recovery/partial_code/annotation/AnnotationTest/
  test_annotation_local_ampersand_labeled` — oracle emits `expected_token` at
  `2:10`, dartq emits it at `2:9` plus an extra `unexpected_token ':'` at `2:10`
  and drops the resolved-identifier row.
- **Hypothesis**: dartq's parser recovery emits the `expected_token` marker on
  the previous token's column and adds phantom `unexpected_token` / loses the
  identifier-recovery row that the reference parser produces. The `partial_code`
  corpus is literally probing "parser at EOF mid-statement" — sensitive to the
  exact recovery rule order.
- **Suggested ticket scope**: `crates/dartq-parser/src/recovery.rs` — column
  convention and recovery token-choice order to match `pkg/_fe_analyzer_shared`
  `partial_code` expectations.

### F5 — `dead_code_span_shape` (1,001 missing + 870 extra rows)
- `(code,severity)`: `WARNING|dead_code`.
- `file`: `language/void/void_type_usage_test.dart` (83 missing + 40 extra),
  `language/patterns/exhaustiveness/object_pattern_switch_test.dart`,
  `language/constants_2018/*`, `lib/test.dart` (`DeadCodeTest`,
  `ForStatementTest`, `PatternNeverMatchesValueTypeTest`).
- **Pattern**: oracle reports *one* long-tail `dead_code` span (e.g. len 5,790
  starting at `:14` after the `:`) plus a small marker on the colon/paren;
  dartq reports many small spans at slightly different columns and misses the
  "tail" span.
- `example`: `void_type_usage_test.dart` line 46: oracle `dead_code len=5790`;
  dartq emits `dead_code len=2` at `21:3` and `len=6675` at `21:3` — a *shifted*
  tail span.
- **Hypothesis**: dartq computes `dead_code` spans from the start of the
  *next* statement instead of the token that triggers unreachable state
  (`:`/`)`/`}`), and it splits multiple `dead_code` markers into one big span
  with a different start column. Likely a single "what counts as the dead-code
  anchor token" rule in flow_analysis / dead_code emitter.
- **Suggested ticket scope**: `crates/dartq-diagnose/src/dead_code.rs` or
  `flow_analysis` — span anchor + merging rule.

### F6 — `type_infer_record_named` (565 missing + 544 extra rows + 1974 missing + 502 extra)
- `(code,severity)`: `ERROR|invalid_assignment` + `ERROR|argument_type_not_assignable`.
- `file`: `lib/test.dart` (`InferredTypeTest` 103 missing rows,
  `StrongModeLocalInferenceTest`, `InvalidAssignmentTest`,
  `ArgumentTypeNotAssignableTest`, `CouldNotInferTest`), `language/**`,
  `co19/LanguageFeatures/Patterns/*`.
- `message`: `A value of type '(int, int, int)' can't be assigned to the
  parameter type 'A'. Expected 2 positional arguments, but got 3 instead.` —
  records vs named parameters, `A<dynamic>` vs `A<int>`, `Iterable<void>` vs
  `Iterable<String>`, `List<dynamic>` vs `List<int>`.
- `example`: `InferredTypeTest` — `A value of type 'A<dynamic, dynamic>' can't
  be assigned to a variable of type 'A<int, String>'` is emitted as *extra* by
  dartq on a case oracle expects to be clean; the inverse (oracle expects a
  specific mismatch, dartq doesn't fire) also common.
- **Hypothesis**: multiple seams share the same code —
  (a) record-literal → named-parameter assignability (record destructuring not
      modelled),
  (b) `<dynamic>` upward-inference on generic instantiation (we fire when
      oracle doesn't, suggesting we keep `dynamic` where oracle promotes),
  (c) `ImplicitCallReference` / `.call` desugaring.
  Probably 3–4 sub-seams under one code.
- **Suggested ticket scope**: `crates/dartq-diagnose/src/assignability.rs` —
  split into record-shape, dynamic-boundary and `.call`-desugar subtickets
  after a focused recon on `InferredTypeTest` (103 rows, highest yield).

### F7 — `null_aware_receiver_after_shortcircuit` (798 missing + ~250 extra rows)
- `(code,severity)`: `WARNING|invalid_null_aware_operator`.
- `file`: `co19/LanguageFeatures/Control-flow-collections/Null-aware-elements/*`
  (~150 missing), `lib/test.dart` (`InvalidNullAwareOperatorTest`,
  `InvalidNullAwareOperatorAfterShortCircuitTest`,
  `CollectionLiteralParserTest`), `language/**`.
- `message`: `The receiver can't be 'null' because of short-circuiting, so the
  null-aware operator '?.' can't be used.` / `The element can't be null, so
  the null-aware operator '?' is unnecessary.`
- **Hypothesis**: oracle emits `invalid_null_aware_operator` when the receiver
  is *provably non-null* (post `&&` / `?:` short-circuit). dartq misses these
  warnings, so our promotion lattice treats `?.` as reachable-or-not differently.
- **Suggested ticket scope**: `crates/dartq-diagnose/src/flow_analysis.rs` —
  null-aware short-circuit promotion → emits `invalid_null_aware_operator`.

### F8 — `type_check_is_reachability` (789 missing + ~200 extra rows)
- `(code,severity)`: `WARNING|unnecessary_type_check`.
- `file`: `co19/LanguageFeatures/Spread-collections/TypeInference_*`,
  `language/identifier/known_prefix*_test.dart`, `lib/test.dart`
  (`ConstantVisitorTest`).
- `message`: `Unnecessary type check; the result is always 'true'/'false'.`
- **Hypothesis**: the `is`/`as` reachability lattice used by
  `unnecessary_type_check` doesn't model "the static type already proves the
  check is a tautology" (e.g. `int x; x is int` → always true). Missing rows
  dominate → we fail to fire.
- **Suggested ticket scope**: `crates/dartq-diagnose/src/type_check.rs` —
  subtyping predicate inside the `unnecessary_type_check` lint rule.

### F9 — `class_modifier_transitivity` (538 missing + ~200 extra rows)
- `(code,severity)`: `ERROR|invalid_use_of_type_outside_library` +
  `ERROR|subtype_of_base_or_final_is_not_base_final_or_sealed`.
- `file`: `co19/LanguageFeatures/Class-modifiers/basic_restrictions_A0{1,3,4}_*`,
  `syntax_sealed_class_A04_t01`, `versioning_A02_t03`,
  `language/class_modifiers/base_transitivity/*`.
- **Hypothesis**: `sealed`/`base`/`final`/`interface` class-modifier
  transitivity rules — e.g. "a subtype of a `base` class outside its library
  must itself be `base`/`final`/`sealed`" — are not implemented or incomplete.
- **Suggested ticket scope**: `crates/dartq-diagnose/src/class_modifiers.rs` —
  implement transitive modifier propagation per SDK `class_modifiers` spec.

### F10 — `use_of_void_result_flow` (528 missing rows)
- `(code,severity)`: `ERROR|use_of_void_result`.
- `file`: `language/void/void_type_usage_test.dart` (285 missing — the entire
  file), `lib/test.dart` (`UseOfVoidResultTest` 58 missing).
- **Hypothesis**: expressions of type `void` used in value position are not
  detected — the `void`-value reachability check is missing. Companion to F5
  (`dead_code`) — both are `void`-flow / reachability seams.
- **Suggested ticket scope**: `crates/dartq-diagnose/src/type_resolver.rs` —
  `void`-usage detection on expression evaluation.

### F11 — `package_aux_materialization` (221 missing + ~150 extra rows)
- `(code,severity)`: `ERROR|uri_does_not_exist`.
- `file`: `lib/test.dart` (`DeprecatedMemberUse_PackageConfigWorkspaceTest`
  family — `package:aaa/a.dart`, `package:foo/src/a.dart`,
  `package:test/a.dart`, `package:bbb/bbb.dart`).
- **Hypothesis**: dartq fabricates `package:aaa/` mock URIs on `lib/test.dart`
  analyzer-test cases but they aren't materialised into the package_config —
  oracle resolves them fine. This is the `materializer`/`mocks` seam noted in
  earlier recon (aux-package not materialised).
- **Suggested ticket scope**: conformance harness `mocks/` materialization, not
  `crates/**` — re-check `DeprecatedMemberUse_PackageConfigWorkspaceTest`
  fixture wiring.

### F12 — `duplicate_definition_unquoted_import` (703 extra + 206 missing rows)
- `(code,severity)`: `ERROR|duplicate_definition`.
- `file`: `co19/LanguageFeatures/Unquoted-imports/keywords_A01_t0{1,2}.dart`
  (125+ extra), `parser-fe/primary_constructors/declaring_parameter_pre_feature.dart`,
  `lib/test.dart`.
- `message`: `The name 'X=' is already defined.` — fires on `name=` setter
  forms inside `import`-ed unquoted source.
- **Hypothesis**: dartq double-registers top-level declarations when the same
  source is reached via both quoted and unquoted import paths — declaration
  dedup doesn't recognise them as the same symbol.
- **Suggested ticket scope**: `crates/dartq-diagnose/src/symbol_table.rs` —
  top-level name dedup keyed on `(library_uri, decl_name)`.

### F13 — `subtype_disallowed_type` (180 missing rows)
- `(code,severity)`: `ERROR|subtype_of_disallowed_type`.
- `file`: `lib/test.dart` (115 missing — `SubtypeOfDisallowedTypeTest`,
  `SubtypeOfDisallowedType_ExtensionTypeTest`), `language/null/inherit_static_*`,
  `language/nonfunction_type_aliases/usage_{futureor,null}_error_test.dart`.
- **Hypothesis**: `Function`/`Record`/`bool`/`enum`/`dynamic`/`FutureOr` etc.
  cannot be subtyped — dartq misses these errors entirely.
- **Suggested ticket scope**: `crates/dartq-diagnose/src/type_check.rs` —
  disallowed-superclass list.

### F14 — `dot_shorthand_return` (384 extra rows)
- `(code,severity)`: `ERROR|return_of_invalid_type`.
- `file`: `language/dot_shorthands/dot_shorthand_helper.dart` (126 extra),
  `lib/test.dart`, `co19/LanguageFeatures/Augmentations/augmenting_functions_*`.
- **Hypothesis**: dot-shorthand (`.name` / `.name(...)`) static-type resolution
  in return position — dartq infers `dynamic`/`Object?` where oracle infers the
  context type.
- **Suggested ticket scope**: `crates/dartq-diagnose/src/expression_type.rs` —
  dot-shorthand context-type propagation.

### F15 — `conflicting_generic_interfaces` (212 missing rows)
- `(code,severity)`: `ERROR|conflicting_generic_interfaces`.
- `file`: `co19/LanguageFeatures/nnbd/overriding/override_checking_A06_*` (181
  rows across many cases), `language/**`, `lib/test.dart`.
- **Hypothesis**: dartq doesn't detect when a class inherits the same
  generic interface via two paths with different type arguments (e.g.
  `implements I<int>` via two supertypes with different args).
- **Suggested ticket scope**: `crates/dartq-diagnose/src/interface.rs` —
  superinterface unification check.

### F16 — `const_*` cluster (~700 rows total)
- `(code,severity)`: `ERROR|const_initialized_with_non_constant_value`,
  `ERROR|const_eval_throws_exception`, `ERROR|const_not_initialized`,
  `ERROR|final_not_initialized_constructor`, `ERROR|assignment_to_final_local`,
  `ERROR|late_final_local_already_assigned`,
  `ERROR|not_initialized_non_nullable_variable`, `ERROR|assignment_to_final`.
- `file`: `lib/test.dart` (`ConstConstructorParamTypeMismatchTest`,
  `ConstantVisitorTest`, `NotInitializedPotentiallyNonNullable*`,
  `FieldPromotionTest`), `co19/LanguageFeatures/Patterns/variable_A03_*`,
  `co19/LanguageFeatures/Patterns/pattern_assignment_A05_*`,
  `language/nnbd/definite_assignment/write_error_test.dart`.
- **Hypothesis**: three sub-seams —
  (a) const-eval throws (dartq doesn't model const evaluation failures),
  (b) pattern-variable definite-assignment (patterns' binding isn't tracked
      as initialisation),
  (c) `late final` assignment state machine.
- **Suggested ticket scope**: three separate tickets (const-eval, pattern
  assignment, late-final state) — they're orthogonal seams under a shared
  family name.

### F17 — `integer_literal_imprecise_as_double` (178 missing rows)
- `(code,severity)`: `ERROR|integer_literal_imprecise_as_double`.
- `file`: `language/double_literals/double_literal_coercion_error_test_*.dart`
  (177 missing — one row per file variant), `lib/test.dart`.
- **Hypothesis**: a single rule — "integer literal > 2^53 coerced to `double`
  loses precision" — is not implemented. Highest row-density per fix of any
  family (one rule → ~178 cases flip).
- **Suggested ticket scope**: `crates/dartq-diagnose/src/literal_type.rs` —
  integer→double precision check.

### F18 — `pattern_never_matches` (193 missing + 30 extra rows)
- `(code,severity)`: `WARNING|pattern_never_matches_value_type`.
- `file`: `lib/test.dart`, `co19/LanguageFeatures/Patterns/list_A05_t10.dart`,
  `pattern_variable_sets_A02_t01.dart`, `language/sound_flow_analysis/pattern_not_matching_null*`.
- **Hypothesis**: pattern-exhaustiveness / value-type vs pattern-type
  conformance — when the matched value's static type can't possibly match the
  pattern, dartq doesn't warn.
- **Suggested ticket scope**: `crates/dartq-diagnose/src/patterns.rs` —
  pattern vs static-type conformance.

### F19 — `experiment_not_enabled` (389 missing rows)
- `(code,severity)`: `ERROR|experiment_not_enabled`.
- `file`: `parser-fe/error_recovery/keyword_named_*` (records),
  `parser-fe/primary_constructors/declaring_parameter_pre_feature.dart`,
  others.
- **Hypothesis**: dartq doesn't gate `records`, `primary_constructors` and
  other experimental features on the experiment flag — fires
  `experiment_not_enabled` where oracle silently accepts.
- **Suggested ticket scope**: `crates/dartq-parser/src/features.rs` —
  experiment-flag gating matrix.

---

## Other wave-B-adjacent rows (not in types/resolver filter)

These codes were extracted because they appeared in `wave_b_codes.txt` but are
not primarily type/assignability seams. Listed for completeness so next wave
tickets don't double-book them.

- `ERROR|invalid_override` (126 missing + 181 extra) — override-conformance.
- `ERROR|wrong_type_parameter_variance_in_superinterface` (127 missing).
- `WARNING|unnecessary_null_comparison` (93 missing).
- `ERROR|abstract_super_member_reference` / `concrete_class_with_abstract_member`
  / `instantiate_abstract_class` / `implicit_this_reference_in_initializer` —
  small missing sets, abstract-class semantics.
- `ERROR|undefined_{identifier,getter,setter,method,class}` —
  `co19/LanguageFeatures/Wildcards/binding_A01_*` family, `Augmentations/*`.
- `WARNING|unused_local_variable`, `unused_element`, `unused_field`, `unused_import` —
  separate wave-A unused-family already tracked elsewhere.

---

## Recommended next-wave ticket list (ordered by row-yield per fix)

1. **F1 aux_file_receiver_typing** — 2,895 cases flip, single seam in
   `row_resolution.rs` receiver typing. Highest priority.
2. **F17 integer_literal_imprecise_as_double** — ~178 missing rows for one
   small rule. Cheapest fix per row.
3. **F3 super_bounded_recursive_bound** — 2,128 combined rows in two files +
   Instantiate-to-bound family. Bound-conformance unification.
4. **F2 expect_static_type_promotion** — 2,433 rows, one extension-receiver
   promotion seam. Likely also fixes F7+F8 partially (same flow lattice).
5. **F4 recovery_position_shift** — ~1,400 rows, parser recovery column
   convention + recovery-token choice. Single rule, big yield.
6. **F5 dead_code_span_shape** — ~1,900 rows, dead-code span anchor + merging
   rule.
7. **F9 class_modifier_transitivity** — ~740 rows, sealed/base/final
   transitive rule implementation.
8. **F10 use_of_void_result_flow** — ~528 rows, `void` reachability rule.
9. **F12 duplicate_definition_unquoted_import** — ~900 rows, symbol-table
   dedup on unquoted-import path.
10. **F6 type_infer_record_named** — ~2,500 rows but multi-seam — split into
    three subtickets (record-shape, dynamic-boundary, `.call` desugar) after
    an `InferredTypeTest`-focused recon lane.
11. **F11 package_aux_materialization** — ~370 rows, conformance-harness
    fixture wiring (not `crates/**`).
12. **F15 conflicting_generic_interfaces** — ~212 rows, superinterface
    unification.
13. **F16 const_* cluster** — ~700 rows total across three orthogonal seams —
    land after F1/F2 (const-eval may already benefit from flow fixes).
14. **F18 pattern_never_matches** — ~223 rows, pattern type-conformance.
15. **F19 experiment_not_enabled** — ~389 rows, parser feature-gating matrix.

---

## Method notes

- All figures computed from the census report table with `awk`
  aggregation scripts (reproducible from the census output).
- `case_id` paths reveal the corpus: `diagnostics/**` (analyzer_test,
  `lib/test.dart`), `features/_core/co19/**` (co19 corpus with `co19/` and
  `co19/Utils/` aux files), `features/_core/<feature>/language/**` (language
  tests), `parser/src/fasta/**` and `parser-fe/**` (parser corpora).
- `missing` = row oracle expected but dartq didn't emit.
- `extra` = row dartq emitted but oracle didn't expect.
- For paired missing+extra on the same case (e.g. `dead_code` on
  `void_type_usage_test`), the family is a *span-shape* mismatch, not a
  detection mismatch — same root-seam bucket.
