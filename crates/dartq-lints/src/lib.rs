//! dartq-lints — the built-in lint rule engine and the wave-1 slice of the
//! `package:lints` recommended/core sets (PRD §13).
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
//! # Wave-1/2 selection
//!
//! Chosen from `.third/lints/lib/{core,recommended}.yaml` under one
//! criterion: the rule's behavior must be fully determined by the
//! syntax-level substrate — no flow analysis (#278), no type inference, no
//! element model. Every rule cites its official implementation in
//! `pkg/linter/lib/src/rules/` and its message template in
//! `pkg/linter/lib/src/diagnostic.g.dart`.
//!
//! Implemented (52):
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
//!
//! Excluded for now, with the blocker:
//! - **Needs the element model (resolution)**: `avoid_print` (dart:core
//!   identity), `prefer_initializing_formals`, `unnecessary_getters_setters`,
//!   `unnecessary_this`. Within `library_annotations`, the
//!   `targetKinds` branch (an annotation declaration's `@Target` metadata,
//!   library_annotations.dart:66-73) is likewise resolution-blocked and not
//!   ported; the pragma branch is.
//! - **Needs flow analysis (#278)**: `prefer_final_locals`,
//!   `prefer_final_fields` (mutation analysis; wave 3's
//!   `prefer_function_declarations_over_variables` ports the unmutated
//!   subset with the mutated-local divergence documented).
//! - **Needs constant evaluation**: `unnecessary_const` (const-context
//!   computation), `exhaustive_cases` (wave 3's `no_duplicate_case_values`
//!   ports the token-lexeme subset with the constant-value divergence
//!   documented).
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
