//! dartq-lints — the built-in lint rule engine and the syntax-visible slice
//! of the `package:lints` recommended/core sets (PRD §13), complete to the
//! member level through #586: every set member is implemented under the
//! documented lexical-approximation discipline or listed blocked with its
//! fact.
//!
//! # Shape
//!
//! - [`Unit`] — the substrate facts a run reads: source, unified token
//!   table, arena AST, line index. One [`run`] traversal dispatches enabled
//!   rules per node kind (a static kind→rules table built from [`RULES`]),
//!   and applies suppression at the sink.
//! - [`registry::RuleSpec`] — one static entry per rule: machine name,
//!   `package:lints` group, official message templates, dispatch kinds, and
//!   the check. [`EnableSet`] resolves machine names once; the hot path
//!   tests bits only.
//! - [`Comments`] — comments re-extracted from the inter-token gaps (the
//!   scanner keeps them out of the token table).
//! - [`IgnoreInfo`] — the `// ignore:` / `// ignore_for_file:` family,
//!   ported from the analyzer's ignore-comment machinery.
//!
//! # Selection and coverage
//!
//! Drawn from `.third/lints/lib/{core,recommended}.yaml`. The base criterion:
//! the rule's behavior must be fully determined by the syntax-level
//! substrate — no flow analysis (#278), no type inference, no element model —
//! with resolution-bound faces replaced by documented lexical approximations
//! whose divergences stay inside the erroneous / user-shadowed families.
//! Every rule cites its official implementation in
//! `pkg/linter/lib/src/rules/` and its message template in
//! `pkg/linter/lib/src/diagnostic.g.dart`.
//!
//! Implemented (72):
//! - wave 1 (18): `avoid_empty_else`, `camel_case_extensions`,
//!   `camel_case_types`, `curly_braces_in_flow_control_structures`,
//!   `empty_catches`, `prefer_typing_uninitialized_variables` (core);
//!   `avoid_return_types_on_setters`,
//!   `avoid_single_cascade_in_expression_statements`,
//!   `constant_identifier_names`, `empty_constructor_bodies`,
//!   `empty_statements`, `no_leading_underscores_for_local_identifiers`,
//!   `prefer_adjacent_string_concatenation`, `prefer_is_not_operator`,
//!   `unnecessary_late`, `unnecessary_new`,
//!   `unnecessary_null_in_if_null_operators`,
//!   `use_function_type_syntax_for_parameters` (recommended).
//! - wave 2 (18): `avoid_shadowing_type_parameters`,
//!   `dangling_library_doc_comments`, `file_names`, `hash_and_equals`,
//!   `non_constant_identifier_names`,
//!   `prefer_generic_function_type_aliases`,
//!   `unintended_html_in_doc_comment` (core); `control_flow_in_finally`,
//!   `library_private_types_in_public_api`, `prefer_conditional_assignment`,
//!   `prefer_if_null_operators`, `prefer_inlined_adds`,
//!   `prefer_null_aware_operators`, `prefer_spread_collections`,
//!   `slash_for_doc_comments`, `unnecessary_brace_in_string_interps`,
//!   `unnecessary_constructor_name`, `unnecessary_string_escapes`
//!   (recommended). The substrate grew two faces for wave 2: [`Unit`] now
//!   carries the file path (`file_names` checks its basename) and the
//!   feature query (`avoid_shadowing_type_parameters` gates `_` parameters
//!   on the `wildcard_variables` feature, defaulting enabled for Dart 3.7+).
//! - directive milestone (6): `avoid_relative_lib_imports`,
//!   `library_annotations`, `use_string_in_part_of_directives` (core);
//!   `library_prefixes`,
//!   `no_leading_underscores_for_library_prefixes`,
//!   `unnecessary_library_name` (recommended). The #300 parser milestone
//!   landed the directive nodes (collapse map §3.1), so these read the
//!   [`Unit`] AST directly: `Import` prefix/URI children,
//!   `LibraryName.has_name`, `PartOf.has_name`, and the `MetadataStar` /
//!   annotated-sibling pair. The substrate grew two [`Features`] bits for
//!   the gated rules (`enhanced_parts`, `unnamed_libraries`).
//! - wave 3 (10): `no_duplicate_case_values` (core); `avoid_init_to_null`,
//!   `avoid_returning_null_for_void`, `prefer_collection_literals`,
//!   `prefer_contains`, `prefer_function_declarations_over_variables`,
//!   `type_init_formals`, `unnecessary_null_aware_assignments`,
//!   `unnecessary_to_list_in_spreads`, `use_rethrow_when_possible`
//!   (recommended). These rules' resolution-bound facets (type and element
//!   checks) are replaced by documented lexical approximations — each rule
//!   module cites the official anchor and spells out the divergences
//!   (over/under-report only on erroneous or user-shadowed code).
//! - completion tranche (12, #353): `no_wildcard_variable_uses`,
//!   `prefer_is_empty`, `prefer_is_not_empty`, `prefer_iterable_whereType`,
//!   `provide_deprecation_message` (core); `avoid_function_literals_in_foreach_calls`,
//!   `prefer_for_elements_to_map_fromIterable`,
//!   `prefer_interpolation_to_compose_strings`, `recursive_getters`,
//!   `unnecessary_const`, `unnecessary_getters_setters`,
//!   `unnecessary_nullable_for_final_variable_declarations` (recommended).
//!   Two re-classifications out of the old exclusion list
//!   (`unnecessary_const` — the official declares `canUseParsedResult`, so
//!   the parse-time `constantContext` walk is the whole semantics, ported
//!   node-for-node as `rules::in_const_context`; `unnecessary_getters_setters`
//!   — the trivial-wrap shape is same-lexeme-visible). The receiver-type
//!   checks dropped here follow the wave-3 `prefer_contains` precedent
//!   (over-report on user classes with the same method name);
//!   `no_wildcard_variable_uses` gained a binding table the engine collects
//!   in the same DFS ([`engine::WildcardScope`], feature-gated).
//! - alignment wave 3 (4, #315): `type_literal_in_constant_pattern` (core);
//!   `null_closures`, `prefer_initializing_formals`, `unnecessary_this`
//!   (recommended). Same documented-approximation discipline: built-in type
//!   names stand in for the type-literal test, written names for the
//!   resolved core APIs of `null_closures`, same-file field + written-type
//!   equality for the initializing-formal conversion, and a syntax shadow
//!   index for the `this.` scope question. Each rule module spells out its
//!   over/under-report faces.
//! - release-gate seam tranche (4, #435): `secure_pubspec_urls` (core);
//!   `package_names`, `implementation_imports`, `exhaustive_cases`
//!   (recommended). Each official verdict leans on facts the [`Unit`]
//!   substrate does not carry — pubspec data, the analyzing library's
//!   package identity, element-model constant values — so the body is
//!   ported as a pure function over an explicit input face (the
//!   documented seam), and the engine wiring belonged to the mounting
//!   ticket #434, since landed: the pubspec pair run through the
//!   `dartq-commands` pubspec pass over `dartq_pub::pubspec_facts`, and
//!   `depend_on_referenced_packages` / `implementation_imports` dispatch
//!   off `Unit.workspace`. `exhaustive_cases` keeps
//!   (enum-like candidate, switch members) as its seam; its row stays
//!   `kinds: &[]`.
//! - standard-set completion (12, #586): `annotate_overrides`,
//!   `overridden_fields`, `unnecessary_overrides`,
//!   `avoid_renaming_method_parameters` (same-file inheritance chain —
//!   extends/with/on as the concrete chain, plus implements as the
//!   any-member chain, standing in for `getInheritedConcreteMember` /
//!   `getInheritedMember`); `await_only_futures`,
//!   `unrelated_type_equality_checks`, `void_checks`,
//!   `null_check_on_nullable_type_parameter`,
//!   `collection_methods_unrelated_type` (scalar-literal / written-type
//!   faces standing in for static types); `implicit_call_tearoffs`
//!   (written `FunctionType` target + same-file `call` class);
//!   `invalid_runtime_check_with_js_interop_types` (same-file `@JS` class
//!   under a `dart:js_interop` import); `depend_on_referenced_packages`
//!   (the `rules::pubspec` package seam, `kinds: &[]` until #434 mounts
//!   it). Each rule module spells out its over/under-report faces; the
//!   registry now covers the full `package:lints` core+recommended union
//!   (89 specs).
//! - diagnostic-mirror tranche (1, #570): `unnecessary_type_check` — the
//!   analyzer `STATIC_WARNING` family (`_checkAllTypeChecks`,
//!   best_practices_verifier.dart:987-1041; the rule file left the linter
//!   in the Dart 3.15 main re-pin) ported as the lint-tier face over the
//!   written-type operand face (scalar literals, `this`, the same-class
//!   field; the promotion face stays with the semantic tier's always-on
//!   producer). Registered under a new [`Group::Diagnostic`] — not a
//!   `package:lints` member, no include set carries it (the enable set is
//!   entirely options-driven: no options file, no lints), opt-in by
//!   name (90 specs).
//! - approximation-conversion tranche (2026-10-12, exemption-reaudit
//!   F2–F8): eight documented approximation faces replaced with the
//!   official decisions over the same-file channel — `exhaustive_cases`
//!   mounted on its enum-like seam (wave 4), the
//!   `collection_methods_unrelated_type` index/`Queue` rows plus
//!   shadow-correct receiver resolution, the typed-receiver gate on
//!   `prefer_contains`/`prefer_iterable_whereType` (the official bails on
//!   an unresolvable receiver), the forward member-label scope on
//!   `control_flow_in_finally`, and the resolution-backed type-literal /
//!   alias fold on `type_literal_in_constant_pattern`. The remaining
//!   divergences are the import-resolution faces each module documents.
//!
//! - removed-registry tranche (2026-10-12, #986, parity-plan W1): the 14
//!   rules the SDK still registers as `RemovedAnalysisRule`
//!   (`diagnosticCodes => const []`) joined under [`Group::Removed`] as
//!   registered no-ops — the names resolve in the options and ignore
//!   faces (the oracle's `currentLintCodeNames`), the rules never fire.
//!   The rules.json `removed` slice of the 264 universe is now covered
//!   (104 specs; [`EnableSet`] grew to five 64-bit words).
//!
//! With this tranche every `package:lints` member has a terminal state:
//! implemented (84), or documented-blocked with the exact resolution fact it
//! waits for (below).
//!
//! # Documented-blocked members, with the blocker
//!
//! - **Type inference (static types of expressions)**:
//!   `unnecessary_string_interpolations` (operand already String — the
//!   dominant hit face is `'${stringVar}'`, neither erroneous nor
//!   user-shadowed).
//! - **Element model (member/type resolution)**: `valid_regexps` (constant
//!   evaluation for interpolated patterns plus an ECMAScript-grammar regex
//!   validator — the Rust `regex` crate is a different grammar and would
//!   misjudge valid patterns), `avoid_types_as_parameter_names` (in-scope
//!   name resolution incl. imports and type parameters — a same-file-only
//!   face under-reports the dominant imported-type case).
//! - **Inheritance / override graph**: `use_super_parameters`
//!   (super-constructor parameter correspondence).
//! - **Scope resolution beyond the file**: `package_config` package
//!   identity remains the one scope face the [`Unit`] lacks;
//!   `implementation_imports` (the only rule waiting on it) ships its
//!   predicate over the seam above.
//! - **Flow analysis (#278)**: `prefer_final_fields` (mutation analysis
//!   across member bodies; a per-unit write scan over-reports on fields
//!   mutated in a sibling part file).
//!
//! (The pre-#353 notes naming `unnecessary_getters_setters` and
//! `unnecessary_const` were re-audited and ported in that tranche;
//! `prefer_initializing_formals` and `unnecessary_this`, kept blocked there
//! with sharper facts, landed in #315 once their divergence families were
//! argued inside the discipline; `avoid_print` and `prefer_final_locals`
//! are not `package:lints` members and never were.)
//!
//! Era note: behavior aligns to the Dart 3.15 main linter in
//! `.third/dart-sdk` (the fact source per AGENTS.md), with rule lists from
//! the pinned `package:lints` in `.third/lints`.

mod bridge;
mod comments;
mod engine;
mod ignore;
mod registry;
mod rules;

#[cfg(test)]
mod testsupport;

pub use bridge::{ParsedUnit, parse_unit};
pub use comments::{Comment, CommentKind, Comments};
pub use engine::{Check, Features, Lint, Unit, WorkspaceFacts, run};
pub use ignore::{IgnoreInfo, UnignorableIgnore, unignorable_ignores};
pub use registry::{EnableSet, Group, RULES, RuleId, RuleSpec, rule_by_name};
// The pubspec-pass verdicts (#434): the mounting face (dartq-commands'
// `analyze_lints`) walks the scanned pubspec facts through these — the
// rules themselves stay crate-internal (`kinds: &[]`, no AST dispatch).
pub use rules::pubspec::package_names::check_name;
pub use rules::pubspec::secure_pubspec_urls::check_url;
