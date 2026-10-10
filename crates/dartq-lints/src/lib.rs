//! dartq-lints — the built-in lint rule engine and the syntax-visible slice
//! of the `package:lints` recommended/core sets (PRD §13), complete to the
//! member level (#353): every set member is implemented under the documented
//! lexical-approximation discipline or listed blocked with its fact.
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
//! Implemented (64):
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
//!
//! With this tranche every `package:lints` member has a terminal state:
//! implemented (64), or documented-blocked with the exact resolution fact it
//! waits for (below).
//!
//! # Documented-blocked members, with the blocker
//!
//! - **Type inference (static types of expressions)**: `await_only_futures`
//!   (Future-subtype test on the awaited expression),
//!   `collection_methods_unrelated_type` (receiver type arguments vs
//!   argument type), `null_check_on_nullable_type_parameter` (type-parameter
//!   nullability), `unrelated_type_equality_checks` (operand type
//!   relation), `unnecessary_string_interpolations` (operand already
//!   String), `void_checks` (void-typed expressions),
//!   `null_closures` (function-typed parameters).
//! - **Element model (member/type resolution)**: `implicit_call_tearoffs`
//!   (callable `call` member), `type_literal_in_constant_pattern`
//!   (identifier classification: type vs constant),
//!   `valid_regexps` (constant evaluation for interpolated patterns plus an
//!   ECMAScript-grammar regex validator — the Rust `regex` crate is a
//!   different grammar and would misjudge valid patterns),
//!   `avoid_types_as_parameter_names` (in-scope name resolution incl.
//!   imports and type parameters — a same-file-only face under-reports the
//!   dominant imported-type case).
//! - **Inheritance / override graph**: `annotate_overrides`,
//!   `unnecessary_overrides` (plus body-equivalence over resolved
//!   elements), `overridden_fields`, `avoid_renaming_method_parameters`,
//!   `use_super_parameters` (super-constructor parameter correspondence),
//!   `prefer_initializing_formals` (field-on-this-class identity, subtype
//!   check, and the single-reference count; the exact-form lexical port
//!   false-positives on setter-only names and inherited fields — legal
//!   code).
//! - **Scope resolution beyond the file**: `unnecessary_this` needs the
//!   library scope — an instance member `x` alongside an imported top-level
//!   `x` makes `this.x` necessary, so the local-shadow index cannot decide;
//!   `implementation_imports` needs the analyzed file's own package
//!   identity (`package_config`).
//! - **Non-Dart artifacts**: `depend_on_referenced_packages` and
//!   `secure_pubspec_urls` / `package_names` read the pubspec and the
//!   package graph.
//! - **Flow analysis (#278)**: `prefer_final_fields` (mutation analysis
//!   across member bodies).
//! - **Enum + constant evaluation**: `exhaustive_cases` (enum value
//!   enumeration for switch exhaustiveness).
//!
//! (The pre-#353 notes naming `unnecessary_getters_setters`,
//! `unnecessary_const`, `prefer_initializing_formals`, `unnecessary_this`,
//! `avoid_print`, and `prefer_final_locals` were re-audited: the first two
//! ported and moved above; `prefer_initializing_formals` /
//! `unnecessary_this` kept blocked with sharper facts; `avoid_print` and
//! `prefer_final_locals` are not `package:lints` members and never were.)
//!
//! Era note: behavior aligns to the Dart 3.15 main linter in
//! `.third/dart-sdk` (the fact source per AGENTS.md), with rule lists from
//! the pinned `package:lints` in `.third/lints`.

mod comments;
mod engine;
mod ignore;
mod registry;
mod rules;

#[cfg(test)]
mod testsupport;

pub use comments::{Comment, CommentKind, Comments};
pub use engine::{Check, Lint, Unit, run};
pub use ignore::IgnoreInfo;
pub use registry::{EnableSet, Group, RULES, RuleId, RuleSpec, rule_by_name};
