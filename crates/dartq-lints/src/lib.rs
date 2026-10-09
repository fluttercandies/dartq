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
//! # Wave-1 selection
//!
//! Chosen from `.third/lints/lib/{core,recommended}.yaml` under one
//! criterion: the rule's behavior must be fully determined by the
//! syntax-level substrate — no flow analysis (#278), no type inference, no
//! element model. Every rule cites its official implementation in
//! `pkg/linter/lib/src/rules/` and its message template in
//! `pkg/linter/lib/src/diagnostic.g.dart`.
//!
//! Implemented (18): `avoid_empty_else`, `camel_case_extensions`,
//! `camel_case_types`, `curly_braces_in_flow_control_structures`,
//! `empty_catches`, `prefer_typing_uninitialized_variables` (core);
//! `avoid_return_types_on_setters`,
//! `avoid_single_cascade_in_expression_statements`,
//! `constant_identifier_names`, `empty_constructor_bodies`,
//! `empty_statements`, `no_leading_underscores_for_local_identifiers`,
//! `prefer_adjacent_string_concatenation`, `prefer_is_not_operator`,
//! `unnecessary_late`, `unnecessary_new`,
//! `unnecessary_null_in_if_null_operators`,
//! `use_function_type_syntax_for_parameters` (recommended).
//!
//! Excluded for now, with the blocker:
//! - **Needs the element model (resolution)**: `avoid_print` (dart:core
//!   identity), `prefer_collection_literals`, `prefer_contains`,
//!   `prefer_initializing_formals`, `unnecessary_getters_setters`,
//!   `unnecessary_this`, `use_rethrow_when_possible` (canonical element
//!   identity of the rethrown name), `type_init_formals` (field type
//!   comparison), `avoid_init_to_null` (declared-type nullability,
//!   `avoid_init_to_null.dart:59-67`), `avoid_relative_lib_imports` /
//!   `library_prefixes` (directive structure, diet-parsed today).
//! - **Needs flow analysis (#278)**: `prefer_final_locals`,
//!   `prefer_final_fields`, `prefer_function_declarations_over_variables`
//!   (`isPotentiallyMutatedInScope`,
//!   prefer_function_declarations_over_variables.dart:44-56).
//! - **Needs constant evaluation**: `unnecessary_const` (const-context
//!   computation), `no_duplicate_case_values`, `exhaustive_cases`.
//! - **Pinned for the next syntax wave** (behavior read, implementable over
//!   today's substrate, deferred for scope):
//!   `prefer_conditional_assignment` / `prefer_if_null_operators` /
//!   `prefer_null_aware_operators` (official behavior compares sub-expression
//!   text; port after the text-compare anchor is agreed),
//!   `unnecessary_brace_in_string_interps` / `unnecessary_string_escapes`
//!   (string-segment walks), `slash_for_doc_comments` (doc-comment
//!   attachment model).
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
pub use engine::{run, Check, Lint, Unit};
pub use ignore::IgnoreInfo;
pub use registry::{rule_by_name, EnableSet, Group, RuleId, RuleSpec, RULES};
