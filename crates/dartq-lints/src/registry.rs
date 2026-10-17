//! The rule registry: one static [`RuleSpec`] table, machine-name lookup, and
//! enable-set resolution.
//!
//! Every rule pins its behavior to the official implementation in upstream
//! Dart SDK source, `pkg/linter/lib/src/rules/<rule>.dart` (cited per
//! module) and its message templates to the generated codes in
//! `pkg/linter/lib/src/diagnostic.g.dart` (cited in each spec). Group
//! membership follows the pinned `package:lints` snapshot (`core.yaml` /
//! `recommended.yaml`, upstream dart-lang/lints; recommended includes
//! core); the removed rules
//! follow the SDK's `RemovedAnalysisRule` registrations (the rules.json
//! `removed` state).

use std::sync::LazyLock;

use dartq_ast::NodeKind;
use dartq_language::LanguageVersion;

use crate::engine::Check;

/// The `package:lints` set a rule belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    /// The pinned `package:lints` snapshot's `core.yaml` (upstream
    /// dart-lang/lints) — critical issues.
    Core,
    /// The pinned `package:lints` snapshot's `recommended.yaml` (upstream
    /// dart-lang/lints) — core plus idiom/style rules.
    Recommended,
    /// A lint-tier mirror of an analyzer diagnostic that is not a
    /// `package:lints` member. No include set carries it — the enable set
    /// is entirely options-driven (no options file, no lints) — so the
    /// rule joins a run only by explicit name (`analysis_options`
    /// `linter.rules` or a caller-built [`EnableSet`]).
    Diagnostic,
    /// A rule the SDK removed but still registers (`RemovedAnalysisRule`,
    /// `diagnosticCodes => const []`): the name resolves in the options
    /// and ignore faces — no `undefined_lint` — and the uppercase name is
    /// in `currentLintCodeNames`
    /// (`analysis_options_parse_model.dart:82`) — while the rule never
    /// fires. Not a `package:lints` member: no include set carries it and
    /// the lint-gate freeze never judges it.
    Removed,
    /// A registered SDK rule outside the `package:lints` sets (the rules.json
    /// stable/deprecated/experimental states minus the 89-member union): no
    /// include set carries it, so enablement is options-driven only
    /// (`analysis_options` `linter.rules` or a caller-built
    /// [`EnableSet`]). Unlike [`Group::Removed`] the
    /// rule generally emits, so it joins the lint-gate freeze by name — the
    /// `RuleState.internal` SDK-dogfooding wave (#1157 L1) being the
    /// exception: registered with `kinds: &[]` and structurally silent, it
    /// is pinned out of the freeze via `STRUCTURALLY_SILENT` and the
    /// divergence ledger.
    Standalone,
}

/// Index into [`rules()`]. Stable within one build of this crate; the machine
/// name is the cross-process identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RuleId(pub u16);

/// One lint rule: identity, message templates, group, dispatch kinds, and the
/// check function. All fields are compile-time constants; the table is
/// iterated, never mutated.
pub struct RuleSpec {
    /// The official machine name (`lints.yaml` / `LintNames` spelling).
    pub name: &'static str,
    pub group: Group,
    /// `problemMessage` template (`diagnostic.g.dart`); `{0}`-style slots.
    pub problem: &'static str,
    /// `correctionMessage` template; empty when the official code has none.
    pub correction: &'static str,
    /// Node kinds the check runs on — the engine dispatch table source.
    pub kinds: &'static [NodeKind],
    /// The check. Called once per node of a [`RuleSpec::kinds`] kind, in a
    /// traversal where [`Check::path`] ends at the visited node.
    pub check: fn(&mut Check<'_>),
}

impl RuleSpec {
    /// Renders the `problemMessage` with `args` substituted into the `{n}`
    /// slots (the oracle's `LocatableDiagnosticImpl` argument fill).
    #[must_use]
    pub fn render_problem(&self, args: &[&str]) -> String {
        render(self.problem, args)
    }

    /// Renders the `correctionMessage`, `None` when the code has none.
    #[must_use]
    pub fn render_correction(&self, args: &[&str]) -> Option<String> {
        (!self.correction.is_empty()).then(|| render(self.correction, args))
    }
}

impl core::fmt::Debug for RuleSpec {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RuleSpec")
            .field("name", &self.name)
            .field("group", &self.group)
            .field("kinds", &self.kinds)
            .finish_non_exhaustive()
    }
}

/// The frozen registry skeleton: all rules through the #586 standard-set
/// completion (the full `package:lints` union: 34 core + 55
/// recommended-only), the #570 diagnostic mirror, the #986 removed-registry
/// no-ops (the 14 `RemovedAnalysisRule` registrations), the #987 W2
/// standalone wave (the first 11 options-driven SDK rules), the #992 W3
/// declaration-shape wave (11 more), the batch close that promoted the
/// parity-plan W4/W7/W8 expression, name/empty/slice, and collection waves,
/// the `require_trailing_commas` face, and the strings/directives mini wave
/// (30 more), and the #1094 batch close that promoted the waves A/B/C
/// declaration-shape, doc-comment, expression, flow-pattern,
/// type-reference, and collection leftovers (17 more), and the final W5
/// closeout promotion (#1153) that emptied the last seven domain incoming
/// slices (19 more, two of them structurally silent), and the W6 closeout
/// promotion (#1213) that landed the #1157 wave's thirteen names — the
/// nine `RuleState.internal` SDK-dogfooding rules, the two experimental
/// variance-domain rules, and the two process-adjacent stable rules
/// (`use_build_context_synchronously`, `unreachable_from_main`) — leaving
/// the tail empty again — and the pubspec path-validator tranche (R780
/// #1521) that landed the analyzer dependency validator's three `path:`
/// mirrors in the frozen diagnostic-mirror section. Order is registry
/// order ([`RuleId`]
/// assignment), grouped core-first then alphabetically within each group,
/// the mirror section, then the wave sections in wave order (the removed
/// block first; each new wave appends its section) — keep the pin test in
/// `tests/registry_pin.rs` in sync.
///
/// This array is write-frozen: a new registration never edits it. Rules
/// land in the per-domain `INCOMING_RULES` slices under [`crate::rules`],
/// aggregated in [`RULES`] below; a batch close promotes the closed block
/// into this skeleton in the same edit that pins its wave list in the pin
/// test.
static BASE_RULES: &[&RuleSpec] = &[
    // —— core (the pinned `package:lints` snapshot's core.yaml) ——
    &crate::rules::empty::avoid_empty_else::SPEC,
    &crate::rules::directives::avoid_relative_lib_imports::SPEC,
    &crate::rules::types::avoid_shadowing_type_parameters::SPEC,
    &crate::rules::names::avoid_types_as_parameter_names::SPEC,
    &crate::rules::expressions::await_only_futures::SPEC,
    &crate::rules::names::camel_case_extensions::SPEC,
    &crate::rules::names::camel_case_types::SPEC,
    &crate::rules::collections::collection_methods_unrelated_type::SPEC,
    &crate::rules::flow::SPEC,
    &crate::rules::docs::dangling_library_doc_comments::SPEC,
    &crate::rules::pubspec::depend_on_referenced_packages::SPEC,
    &crate::rules::empty::empty_catches::SPEC,
    &crate::rules::names::file_names::SPEC,
    &crate::rules::declarations::hash_and_equals::SPEC,
    &crate::rules::expressions::implicit_call_tearoffs::SPEC,
    &crate::rules::directives::library_annotations::SPEC,
    &crate::rules::flow::no_duplicate_case_values::SPEC,
    &crate::rules::names::no_wildcard_variable_uses::SPEC,
    &crate::rules::names::non_constant_identifier_names::SPEC,
    &crate::rules::expressions::null_check_on_nullable_type_parameter::SPEC,
    &crate::rules::declarations::prefer_generic_function_type_aliases::SPEC,
    &crate::rules::expressions::prefer_is_empty::SPEC,
    &crate::rules::expressions::prefer_is_not_empty::SPEC,
    &crate::rules::collections::prefer_iterable_where_type::SPEC,
    &crate::rules::declarations::prefer_typing_uninitialized_variables::SPEC,
    &crate::rules::directives::provide_deprecation_message::SPEC,
    &crate::rules::pubspec::secure_pubspec_urls::SPEC,
    &crate::rules::flow::type_literal_in_constant_pattern::SPEC,
    &crate::rules::docs::unintended_html_in_doc_comment::SPEC,
    &crate::rules::declarations::unnecessary_overrides::SPEC,
    &crate::rules::expressions::unrelated_type_equality_checks::SPEC,
    &crate::rules::directives::use_string_in_part_of_directives::SPEC,
    &crate::rules::regexp::valid_regexps::SPEC,
    &crate::rules::expressions::void_checks::SPEC,
    // —— recommended (the pinned `package:lints` snapshot's recommended.yaml) ——
    &crate::rules::expressions::avoid_function_literals_in_foreach_calls::SPEC,
    &crate::rules::declarations::annotate_overrides::SPEC,
    &crate::rules::declarations::avoid_init_to_null::SPEC,
    &crate::rules::declarations::avoid_renaming_method_parameters::SPEC,
    &crate::rules::declarations::avoid_return_types_on_setters::SPEC,
    &crate::rules::flow::avoid_returning_null_for_void::SPEC,
    &crate::rules::expressions::avoid_single_cascade_in_expression_statements::SPEC,
    &crate::rules::names::constant_identifier_names::SPEC,
    &crate::rules::flow::control_flow_in_finally::SPEC,
    &crate::rules::empty::empty_constructor_bodies::SPEC,
    &crate::rules::empty::empty_statements::SPEC,
    &crate::rules::flow::exhaustive_cases::SPEC,
    &crate::rules::directives::implementation_imports::SPEC,
    &crate::rules::expressions::invalid_runtime_check_with_js_interop_types::SPEC,
    &crate::rules::directives::library_prefixes::SPEC,
    &crate::rules::types::library_private_types_in_public_api::SPEC,
    &crate::rules::directives::no_leading_underscores_for_library_prefixes::SPEC,
    &crate::rules::names::no_leading_underscores_for_local_identifiers::SPEC,
    &crate::rules::expressions::null_closures::SPEC,
    &crate::rules::declarations::overridden_fields::SPEC,
    &crate::rules::pubspec::package_names::SPEC,
    &crate::rules::expressions::prefer_adjacent_string_concatenation::SPEC,
    &crate::rules::collections::prefer_collection_literals::SPEC,
    &crate::rules::expressions::prefer_conditional_assignment::SPEC,
    &crate::rules::expressions::prefer_contains::SPEC,
    &crate::rules::expressions::prefer_final_fields::SPEC,
    &crate::rules::collections::prefer_for_elements_to_map_from_iterable::SPEC,
    &crate::rules::declarations::prefer_function_declarations_over_variables::SPEC,
    &crate::rules::expressions::prefer_if_null_operators::SPEC,
    &crate::rules::declarations::prefer_initializing_formals::SPEC,
    &crate::rules::collections::prefer_inlined_adds::SPEC,
    &crate::rules::expressions::prefer_interpolation_to_compose_strings::SPEC,
    &crate::rules::expressions::prefer_is_not_operator::SPEC,
    &crate::rules::expressions::prefer_null_aware_operators::SPEC,
    &crate::rules::collections::prefer_spread_collections::SPEC,
    &crate::rules::declarations::recursive_getters::SPEC,
    &crate::rules::docs::slash_for_doc_comments::SPEC,
    &crate::rules::strings::unnecessary_brace_in_string_interps::SPEC,
    &crate::rules::expressions::unnecessary_const::SPEC,
    &crate::rules::declarations::unnecessary_constructor_name::SPEC,
    &crate::rules::declarations::unnecessary_getters_setters::SPEC,
    &crate::rules::declarations::unnecessary_late::SPEC,
    &crate::rules::directives::unnecessary_library_name::SPEC,
    &crate::rules::expressions::unnecessary_new::SPEC,
    &crate::rules::expressions::unnecessary_null_aware_assignments::SPEC,
    &crate::rules::expressions::unnecessary_null_in_if_null_operators::SPEC,
    &crate::rules::declarations::unnecessary_nullable_for_final_variable_declarations::SPEC,
    &crate::rules::expressions::unnecessary_this::SPEC,
    &crate::rules::strings::unnecessary_string_escapes::SPEC,
    &crate::rules::strings::unnecessary_string_interpolations::SPEC,
    &crate::rules::declarations::type_init_formals::SPEC,
    &crate::rules::collections::unnecessary_to_list_in_spreads::SPEC,
    &crate::rules::declarations::use_function_type_syntax_for_parameters::SPEC,
    &crate::rules::names::use_super_parameters::SPEC,
    &crate::rules::flow::use_rethrow_when_possible::SPEC,
    // —— diagnostic mirrors (analyzer diagnostics outside `package:lints`;
    // no include set carries them, opt-in by name) ——
    &crate::rules::pubspec::path_dependencies::PATH_DOES_NOT_EXIST,
    &crate::rules::pubspec::path_dependencies::PATH_NOT_POSIX,
    &crate::rules::pubspec::path_dependencies::SPEC,
    &crate::rules::expressions::unnecessary_type_check::SPEC,
    // —— removed (the `RemovedAnalysisRule` no-ops, plan W1 #986; the
    // rules.json `removed` state, alphabetical): the names resolve in the
    // options and ignore faces, the rules never fire. Re-enumerated at the
    // #1025 reconciliation against the published rules.json: the `removed`
    // state holds exactly these 14 — `prefer_equal_for_default_values` is a
    // member of the fourteen, not a fifteenth ——
    &crate::rules::removed::always_require_non_null_named_parameters::SPEC,
    &crate::rules::removed::avoid_as::SPEC,
    &crate::rules::removed::avoid_returning_null::SPEC,
    &crate::rules::removed::avoid_returning_null_for_future::SPEC,
    &crate::rules::removed::avoid_unstable_final_fields::SPEC,
    &crate::rules::removed::enable_null_safety::SPEC,
    &crate::rules::removed::invariant_booleans::SPEC,
    &crate::rules::removed::iterable_contains_unrelated_type::SPEC,
    &crate::rules::removed::list_remove_unrelated_type::SPEC,
    &crate::rules::removed::package_api_docs::SPEC,
    &crate::rules::removed::prefer_bool_in_asserts::SPEC,
    &crate::rules::removed::prefer_equal_for_default_values::SPEC,
    &crate::rules::removed::super_goes_last::SPEC,
    &crate::rules::removed::unsafe_html::SPEC,
    // —— standalone W2 (#987, options-driven SDK rules outside
    // `package:lints`; parity-plan wave 2, alphabetical): they emit, so
    // each joins the lint-gate freeze by name ——
    &crate::rules::declarations::always_declare_return_types::SPEC,
    &crate::rules::flow::always_put_control_body_on_new_line::SPEC,
    &crate::rules::declarations::always_put_required_named_parameters_first::SPEC,
    &crate::rules::types::avoid_annotating_with_dynamic::SPEC,
    &crate::rules::flow::avoid_catches_without_on_clauses::SPEC,
    &crate::rules::declarations::avoid_classes_with_only_static_members::SPEC,
    &crate::rules::expressions::avoid_double_and_int_checks::SPEC,
    &crate::rules::strings::avoid_escaping_inner_quotes::SPEC,
    &crate::rules::declarations::avoid_final_parameters::SPEC,
    &crate::rules::expressions::avoid_js_rounded_ints::SPEC,
    &crate::rules::declarations::avoid_multiple_declarations_per_line::SPEC,
    // —— standalone W3 (#992, parity-plan wave 3, alphabetical) ——
    &crate::rules::declarations::avoid_private_typedef_functions::SPEC,
    &crate::rules::declarations::avoid_returning_this::SPEC,
    &crate::rules::declarations::avoid_types_on_closure_parameters::SPEC,
    &crate::rules::declarations::avoid_unused_constructor_parameters::SPEC,
    &crate::rules::directives::combinators_ordering::SPEC,
    &crate::rules::declarations::initialize_in_field_declaration::SPEC,
    &crate::rules::strings::leading_newlines_in_multiline_strings::SPEC,
    &crate::rules::directives::migrate_design_widgets::SPEC,
    &crate::rules::strings::missing_whitespace_between_adjacent_strings::SPEC,
    &crate::rules::strings::no_adjacent_strings_in_list::SPEC,
    &crate::rules::expressions::no_literal_bool_comparisons::SPEC,
    // —— standalone W4 (parity-plan wave 4, the expression-shape wave,
    // alphabetical) ——
    &crate::rules::expressions::avoid_bool_literals_in_conditional_expressions::SPEC,
    &crate::rules::expressions::avoid_null_checks_in_equality_operators::SPEC,
    &crate::rules::expressions::literal_only_boolean_expressions::SPEC,
    &crate::rules::expressions::noop_primitive_operations::SPEC,
    &crate::rules::expressions::prefer_expression_function_bodies::SPEC,
    &crate::rules::expressions::prefer_int_literals::SPEC,
    &crate::rules::expressions::unnecessary_lambdas::SPEC,
    &crate::rules::expressions::unnecessary_parenthesis::SPEC,
    &crate::rules::expressions::unnecessary_statements::SPEC,
    // —— standalone W7 (parity-plan wave 7, the name/empty/slice wave,
    // alphabetical across the three domain files) ——
    &crate::rules::slice::no_self_assignments::SPEC,
    &crate::rules::empty::one_member_abstracts::SPEC,
    &crate::rules::empty::prefer_asserts_with_message::SPEC,
    &crate::rules::names::sort_constructors_first::SPEC,
    &crate::rules::names::sort_unnamed_constructors_first::SPEC,
    &crate::rules::empty::throw_in_finally::SPEC,
    &crate::rules::empty::unnecessary_breaks::SPEC,
    &crate::rules::empty::unnecessary_final::SPEC,
    // —— standalone W8 (parity-plan wave 8, the collection wave,
    // alphabetical) ——
    &crate::rules::collections::prefer_foreach::SPEC,
    &crate::rules::collections::prefer_if_elements_to_conditional_expressions::SPEC,
    // —— standalone: the `require_trailing_commas` face (#1053) — the
    // version-gated tall-style rule, silent from 3.7 ——
    &crate::rules::flow::require_trailing_commas::SPEC,
    // —— standalone: the strings/directives mini wave (alphabetical) ——
    &crate::rules::directives::directives_ordering::SPEC,
    &crate::rules::strings::eol_at_end_of_file::SPEC,
    &crate::rules::directives::library_names::SPEC,
    &crate::rules::strings::lines_longer_than_80_chars::SPEC,
    &crate::rules::directives::package_prefixed_library_names::SPEC,
    &crate::rules::strings::prefer_double_quotes::SPEC,
    &crate::rules::strings::prefer_single_quotes::SPEC,
    &crate::rules::directives::unnecessary_library_directive::SPEC,
    &crate::rules::strings::unnecessary_raw_strings::SPEC,
    &crate::rules::strings::use_raw_strings::SPEC,
    // —— standalone: the #1094 batch close (waves A/B/C — the
    // declaration-shape, doc-comment, expression, flow-pattern,
    // type-reference, and collection leftovers, alphabetical) ——
    &crate::rules::docs::document_ignores::SPEC,
    &crate::rules::declarations::empty_container_bodies::SPEC,
    &crate::rules::docs::flutter_style_todos::SPEC,
    &crate::rules::flow::invalid_case_patterns::SPEC,
    &crate::rules::docs::missing_code_block_language_in_doc_comment::SPEC,
    &crate::rules::types::prefer_constructors_over_static_methods::SPEC,
    &crate::rules::declarations::prefer_final_parameters::SPEC,
    &crate::rules::flow::simplify_variable_pattern::SPEC,
    &crate::rules::types::strict_top_level_inference::SPEC,
    &crate::rules::flow::switch_on_type::SPEC,
    &crate::rules::types::type_annotate_public_apis::SPEC,
    &crate::rules::declarations::unnecessary_const_in_enum_constructor::SPEC,
    &crate::rules::declarations::unnecessary_type_name_in_constructor::SPEC,
    &crate::rules::expressions::unnecessary_underscores::SPEC,
    &crate::rules::collections::use_null_aware_elements::SPEC,
    &crate::rules::declarations::use_primary_constructors::SPEC,
    &crate::rules::declarations::var_with_no_type_annotation::SPEC,
    // —— standalone: the W1/W2 closeout promotion (#1108 — the directives,
    // flow, pubspec, type-reference, flutter, doc-comment, and declaration
    // incoming slices, alphabetical) ——
    &crate::rules::types::always_specify_types::SPEC,
    &crate::rules::directives::always_use_package_imports::SPEC,
    &crate::rules::declarations::annotate_redeclares::SPEC,
    &crate::rules::declarations::async_return_with_no_await::SPEC,
    &crate::rules::flow::avoid_catching_errors::SPEC,
    &crate::rules::declarations::avoid_equals_and_hash_code_on_mutable_classes::SPEC,
    &crate::rules::declarations::avoid_field_initializers_in_const_classes::SPEC,
    &crate::rules::types::avoid_futureor_void::SPEC,
    &crate::rules::declarations::avoid_implementing_value_types::SPEC,
    &crate::rules::declarations::avoid_positional_boolean_parameters::SPEC,
    &crate::rules::declarations::avoid_setters_without_getters::SPEC,
    &crate::rules::flutter::avoid_unnecessary_containers::SPEC,
    &crate::rules::declarations::avoid_void_async::SPEC,
    &crate::rules::directives::avoid_web_libraries_in_flutter::SPEC,
    &crate::rules::directives::conditional_uri_does_not_exist::SPEC,
    &crate::rules::declarations::deprecated_consistency::SPEC,
    &crate::rules::flutter::diagnostic_describe_all_properties::SPEC,
    &crate::rules::types::erase_dart_type_extension_types::SPEC,
    &crate::rules::flow::join_return_with_assignment::SPEC,
    &crate::rules::flow::no_default_cases::SPEC,
    &crate::rules::flutter::no_logic_in_create_state::SPEC,
    &crate::rules::types::no_raw_types::SPEC,
    &crate::rules::declarations::omit_local_variable_types::SPEC,
    &crate::rules::declarations::omit_obvious_local_variable_types::SPEC,
    &crate::rules::declarations::omit_obvious_property_types::SPEC,
    &crate::rules::declarations::prefer_asserts_in_initializer_lists::SPEC,
    &crate::rules::declarations::prefer_const_constructors_in_immutables::SPEC,
    &crate::rules::declarations::prefer_final_in_for_each::SPEC,
    &crate::rules::declarations::prefer_final_locals::SPEC,
    &crate::rules::directives::prefer_relative_imports::SPEC,
    &crate::rules::types::prefer_void_to_null::SPEC,
    &crate::rules::docs::public_member_api_docs::SPEC,
    &crate::rules::directives::simple_directive_paths::SPEC,
    &crate::rules::flutter::sized_box_for_whitespace::SPEC,
    &crate::rules::flutter::sized_box_shrink_expand::SPEC,
    &crate::rules::flutter::sort_child_properties_last::SPEC,
    &crate::rules::pubspec::sort_pub_dependencies::SPEC,
    &crate::rules::declarations::specify_nonobvious_local_variable_types::SPEC,
    &crate::rules::declarations::specify_nonobvious_property_types::SPEC,
    &crate::rules::declarations::test_types_in_equals::SPEC,
    &crate::rules::declarations::unnecessary_async::SPEC,
    &crate::rules::declarations::unnecessary_primary_constructor_body::SPEC,
    &crate::rules::flutter::use_colored_box::SPEC,
    &crate::rules::declarations::use_declaring_parameters::SPEC,
    &crate::rules::flutter::use_decorated_box::SPEC,
    &crate::rules::flutter::use_full_hex_values_for_flutter_colors::SPEC,
    &crate::rules::flutter::use_key_in_widget_constructors::SPEC,
    &crate::rules::declarations::use_late_for_private_fields_and_variables::SPEC,
    &crate::rules::declarations::use_setters_to_change_properties::SPEC,
    &crate::rules::declarations::use_to_and_as_if_applicable::SPEC,
    // —— standalone: the W1/W2 final tail promotion (#1141 — the
    // expressions, slice, and strings incoming slices, alphabetical) ——
    &crate::rules::expressions::avoid_dynamic_calls::SPEC,
    &crate::rules::expressions::avoid_print::SPEC,
    &crate::rules::expressions::avoid_type_to_string::SPEC,
    &crate::rules::expressions::cascade_invocations::SPEC,
    &crate::rules::expressions::cast_nullable_to_non_nullable::SPEC,
    &crate::rules::expressions::discarded_futures::SPEC,
    &crate::rules::expressions::do_not_use_environment::SPEC,
    &crate::rules::expressions::no_dynamic_casts::SPEC,
    &crate::rules::expressions::no_runtimetype_tostring::SPEC,
    &crate::rules::expressions::only_throw_errors::SPEC,
    &crate::rules::slice::parameter_assignments::SPEC,
    &crate::rules::expressions::prefer_null_aware_method_calls::SPEC,
    &crate::rules::expressions::unawaited_futures::SPEC,
    &crate::rules::expressions::unnecessary_await_in_return::SPEC,
    &crate::rules::expressions::unnecessary_null_aware_operator_on_extension_on_nullable::SPEC,
    &crate::rules::expressions::unnecessary_null_checks::SPEC,
    &crate::rules::expressions::unnecessary_unawaited::SPEC,
    &crate::rules::expressions::use_if_null_to_convert_nulls_to_bools::SPEC,
    &crate::rules::expressions::use_is_even_rather_than_modulo::SPEC,
    &crate::rules::strings::use_string_buffers::SPEC,
    &crate::rules::expressions::use_truncating_division::SPEC,
    // —— standalone: the W5 closeout promotion (#1153 — the collections,
    // declarations, docs, expressions, flow, names, and types incoming
    // slices, alphabetical) ——
    &crate::rules::expressions::avoid_redundant_argument_values::SPEC,
    &crate::rules::expressions::avoid_slow_async_io::SPEC,
    &crate::rules::types::cancel_subscriptions::SPEC,
    &crate::rules::types::close_sinks::SPEC,
    &crate::rules::docs::comment_references::SPEC,
    &crate::rules::declarations::deprecated_member_use_from_same_package::SPEC,
    &crate::rules::expressions::future_sync_value::SPEC,
    &crate::rules::names::matching_super_parameters::SPEC,
    &crate::rules::expressions::prefer_const_constructors::SPEC,
    &crate::rules::declarations::prefer_const_declarations::SPEC,
    &crate::rules::collections::prefer_const_literals_to_create_immutables::SPEC,
    &crate::rules::declarations::prefer_mixin::SPEC,
    &crate::rules::declarations::remove_deprecations_in_breaking_versions::SPEC,
    &crate::rules::types::tighten_type_of_initializing_formals::SPEC,
    &crate::rules::docs::unnecessary_ignore::SPEC,
    &crate::rules::expressions::unnecessary_this_alias::SPEC,
    &crate::rules::declarations::use_enums::SPEC,
    &crate::rules::expressions::use_named_constants::SPEC,
    &crate::rules::flow::use_test_throws_matchers::SPEC,
    // —— standalone: the W6 closeout promotion (#1213 — the #1157 wave's
    // thirteen names: the nine `RuleState.internal` SDK-dogfooding rules,
    // the two experimental variance-domain rules, and the two
    // process-adjacent stable rules, alphabetical; the aggregated tail is
    // empty again) ——
    &crate::rules::declarations::analyzer_element_model_tracking_bad::SPEC,
    &crate::rules::declarations::analyzer_element_model_tracking_more_than_one::SPEC,
    &crate::rules::declarations::analyzer_element_model_tracking_zero::SPEC,
    &crate::rules::declarations::analyzer_public_api_bad_part_directive::SPEC,
    &crate::rules::declarations::analyzer_public_api_bad_type::SPEC,
    &crate::rules::declarations::analyzer_public_api_experimental_inconsistency::SPEC,
    &crate::rules::declarations::analyzer_public_api_exports_non_public_name::SPEC,
    &crate::rules::declarations::analyzer_public_api_impl_in_public_api::SPEC,
    &crate::rules::declarations::analyzer_to_be_deprecated_use::SPEC,
    &crate::rules::declarations::implicit_reopen::SPEC,
    &crate::rules::declarations::unreachable_from_main::SPEC,
    &crate::rules::types::unsafe_variance::SPEC,
    &crate::rules::flow::use_build_context_synchronously::SPEC,
];

/// The registry storage: `BASE_RULES` followed by the domain-owned
/// incoming slices, name-merged into the open batch's order. The aggregation
/// is what keeps a registration a one-file edit — the slices live in the
/// rule files, this wiring is batch-invariant, and a closed batch is
/// promoted into the skeleton (see `BASE_RULES`).
pub static RULES: LazyLock<Vec<&'static RuleSpec>> = LazyLock::new(|| {
    // The domain seams (`rules/*.rs`); a new domain file joins here once.
    const INCOMING: &[&[&RuleSpec]] = &[
        crate::rules::collections::INCOMING_RULES,
        crate::rules::declarations::INCOMING_RULES,
        crate::rules::directives::INCOMING_RULES,
        crate::rules::docs::INCOMING_RULES,
        crate::rules::empty::INCOMING_RULES,
        crate::rules::expressions::INCOMING_RULES,
        crate::rules::flutter::INCOMING_RULES,
        crate::rules::flow::INCOMING_RULES,
        crate::rules::names::INCOMING_RULES,
        crate::rules::pubspec::INCOMING_RULES,
        crate::rules::regexp::INCOMING_RULES,
        crate::rules::slice::INCOMING_RULES,
        crate::rules::strings::INCOMING_RULES,
        crate::rules::types::INCOMING_RULES,
    ];
    let tail_len: usize = INCOMING.iter().map(|slice| slice.len()).sum();
    let mut rules = Vec::with_capacity(BASE_RULES.len() + tail_len);
    rules.extend_from_slice(BASE_RULES);
    let base = rules.len();
    for slice in INCOMING {
        rules.extend_from_slice(slice);
    }
    // The open batch: name order across the domain slices (the wave-order
    // contract the pin test judges).
    rules[base..].sort_unstable_by_key(|spec| spec.name);
    // The sort above (and `rule_by_name`'s first-match position lookup) is
    // well-defined only under unique names — a duplicate would leave the
    // tail order to pdqsort internals. Debug-only, one-time init: a
    // duplicate surfaces at build time instead of as a silent reorder (the
    // wave-order pin `rule_names_are_unique` holds the invariant in tests).
    debug_assert!(
        {
            let mut names: Vec<&str> = rules.iter().map(|spec| spec.name).collect();
            names.sort_unstable();
            names.windows(2).all(|pair| pair[0] != pair[1])
        },
        "duplicate registered rule name"
    );
    rules
});

/// All registered rules in registry order — the registry's read face
/// ([`RuleId`] indexes this slice; the machine name is the cross-process
/// identity). New code reads through this; [`RULES`] is the storage.
#[must_use]
pub fn rules() -> &'static [&'static RuleSpec] {
    &RULES
}

/// The rule with machine name `name`, case-insensitively (linear scan; a
/// one-per-run lookup, not a hot path). The official registry lowercases
/// every name (`pkg/analyzer/lib/src/lint/registry.dart:42,53,133`), so any
/// casing of a registered rule resolves — the SDK machine names are lowercase
/// while `package:lints` yaml uses mixed-case spellings; the spec's canonical
/// name (what reports render) is unchanged.
#[expect(
    clippy::cast_possible_truncation,
    reason = "rule positions come from the static registry, bounded far below \
              u16::MAX; u16 is the RuleId payload width"
)]
#[must_use]
pub fn rule_by_name(name: &str) -> Option<RuleId> {
    rules()
        .iter()
        .position(|spec| spec.name.eq_ignore_ascii_case(name))
        .map(|i| RuleId(i as u16))
}

/// The rules that declare incompatible peers (`rule.incompatibleRules`,
/// upstream Dart SDK source, `pkg/linter/lib/src/rules/<name>.dart` — each
/// file's `List<String> get incompatibleRules` override; the getter is
/// opt-in, so every name outside this table declares none). The lists hold
/// the machine-name lowercase spellings and mirror the upstream order; the
/// one empty override (`specify_nonobvious_property_types`) is omitted —
/// the lookup's default arm renders the same empty face.
static INCOMPATIBLE_RULES: &[(&str, &[&str])] = &[
    (
        "always_specify_types",
        &[
            "avoid_types_on_closure_parameters",
            "omit_local_variable_types",
            "omit_obvious_local_variable_types",
            "omit_obvious_property_types",
        ],
    ),
    ("always_use_package_imports", &["prefer_relative_imports"]),
    ("avoid_final_parameters", &["prefer_final_parameters"]),
    (
        "avoid_types_on_closure_parameters",
        &["always_specify_types"],
    ),
    (
        "omit_local_variable_types",
        &[
            "always_specify_types",
            "specify_nonobvious_local_variable_types",
        ],
    ),
    (
        "omit_obvious_local_variable_types",
        &["always_specify_types"],
    ),
    (
        "omit_obvious_property_types",
        &["always_specify_types", "type_annotate_public_apis"],
    ),
    ("prefer_double_quotes", &["prefer_single_quotes"]),
    ("prefer_final_in_for_each", &["unnecessary_final"]),
    (
        "prefer_final_parameters",
        &["unnecessary_final", "avoid_final_parameters"],
    ),
    ("prefer_final_locals", &["unnecessary_final"]),
    ("prefer_relative_imports", &["always_use_package_imports"]),
    ("prefer_single_quotes", &["prefer_double_quotes"]),
    (
        "specify_nonobvious_local_variable_types",
        &["omit_local_variable_types"],
    ),
    (
        "type_annotate_public_apis",
        &["omit_obvious_property_types"],
    ),
    (
        "unnecessary_final",
        &[
            "prefer_final_locals",
            "prefer_final_parameters",
            "prefer_final_in_for_each",
        ],
    ),
];

/// The names `name` declares incompatible (upstream
/// `rule.incompatibleRules`): the empty slice for every rule without the
/// override, including unregistered spellings. Case-insensitive like
/// [`rule_by_name`] (a linear scan; a once-per-run lookup, not a hot path).
#[must_use]
pub fn incompatible_rules(name: &str) -> &'static [&'static str] {
    INCOMPATIBLE_RULES
        .iter()
        .find(|(rule, _)| rule.eq_ignore_ascii_case(name))
        .map_or(&[], |(_, peers)| peers)
}

/// The `kinds: &[]` set rules that still emit — through the dartq-commands
/// pubspec pass at the #434 mounting face, not through AST dispatch (the
/// crate docs' pubspec seam). The only legitimate empty-`kinds` members
/// among the gate-governed set rules (the removed no-ops are empty-kinds
/// too, but they are registry-face registrations, not the gate's universe).
/// `sort_pub_dependencies` joined at the #1108 close once #1131 wired the
/// pubspec pass to feed it (`pubspec_rows` -> `check_section`).
const PUBSPEC_PASS_SEAM: &[&str] = &[
    "package_names",
    "secure_pubspec_urls",
    "sort_pub_dependencies",
];

/// The Dart-attached rules whose rows join a file's diagnostic list after
/// a run returns — the mounting shell's breaking-version walk
/// ([`crate::engine::run_breaking_version_annotations`]) runs once the
/// run's `unnecessary_ignore` epilogue has already fired, so the run's
/// suppression record can never hold their citations. The epilogue skips
/// their names: upstream's validator sees the unfiltered FINAL list where
/// the row is already in (`library_analyzer.dart`:331-341). Enumerated
/// like [`PUBSPEC_PASS_SEAM`] — the structural complement (empty `kinds`,
/// no seam membership) would wrongly sweep in the internal analyzer set,
/// whose citations in a Dart file are genuinely unused upstream. The rows
/// genuinely emit (at the shell), so this is also a mount face of
/// [`emittable_rule_names`] — one enumeration, two consumers.
pub(crate) const LATE_JOIN_RULES: &[&str] = &["remove_deprecations_in_breaking_versions"];

/// The `kinds: &[]` rules that emit through the engine run's epilogue —
/// a run-level tail that fires after the dispatch walk completes but
/// inside the run ([`crate::engine::unnecessary_ignore_rows`], the #1345
/// mount). No per-node check can carry their verdicts: the verdict needs
/// the run's complete diagnostic set, so `kinds` stays empty. Distinct
/// from [`LATE_JOIN_RULES`] on both membership and semantics — an
/// epilogue emitter's own citations MUST stay in the epilogue's walk (a
/// `// ignore: <epilogue rule>` citation can itself be unused and must
/// self-report), while a late join's citations must be skipped.
const RUN_EPILOGUE_SEAM: &[&str] = &["unnecessary_ignore"];

/// The registered rules the implementation can emit: non-mirror,
/// non-removed specs that dispatch on the AST (`kinds` non-empty) or run
/// through one of the out-of-dispatch mount faces — the dartq-commands
/// pubspec pass, the engine run's epilogue, or the mounting shell's
/// late-join walk. The single derivation behind the consumers — the
/// frozen lint-gate universe's governance pin (`tests/registry_pin.rs`,
/// the `lint_codes.txt` freeze) and the harness fixture-coverage pin's
/// live lint tier — so the seams are restated nowhere else.
#[must_use]
pub fn emittable_rule_names() -> impl Iterator<Item = &'static str> {
    rules()
        .iter()
        .filter(|spec| !matches!(spec.group, Group::Diagnostic | Group::Removed))
        .filter(|spec| {
            !spec.kinds.is_empty()
                || PUBSPEC_PASS_SEAM.contains(&spec.name)
                || RUN_EPILOGUE_SEAM.contains(&spec.name)
                || LATE_JOIN_RULES.contains(&spec.name)
        })
        .map(|spec| spec.name)
}

/// The set of rules a run executes.
///
/// Resolved once from machine names; the hot path tests bits only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnableSet {
    words: [u64; WORDS],
}

/// `rules().len()` outgrew one 64-bit word at 68 rules (wave 3, #315) and
/// two at 104 (the removed-registry wave, #986); five words carry the full
/// SDK registry (269) plus the mirror. The word count is asserted in
/// `tests/registry_pin.rs` so further growth is caught together with the
/// pin.
const WORDS: usize = 5;

impl EnableSet {
    /// Every registered rule.
    #[must_use]
    pub fn all() -> Self {
        let mut set = Self { words: [0; WORDS] };
        #[expect(
            clippy::cast_possible_truncation,
            reason = "rule positions come from the static registry, bounded \
                      far below u16::MAX; u16 is the RuleId payload width"
        )]
        for i in 0..rules().len() {
            set.set(RuleId(i as u16));
        }
        set
    }

    /// Resolves machine names into the set. Unknown names are reported back
    /// verbatim (the caller decides whether that is an error — the oracle
    /// reports an undefined lint as a warning and keeps analyzing).
    #[must_use]
    pub fn resolve<'a, I>(names: I) -> (Self, Vec<&'a str>)
    where
        I: IntoIterator<Item = &'a str>,
    {
        let mut set = Self { words: [0; WORDS] };
        let mut unknown = Vec::new();
        for name in names {
            match rule_by_name(name) {
                Some(id) => set.set(id),
                None => unknown.push(name),
            }
        }
        (set, unknown)
    }

    /// No rules enabled — the run is a no-op.
    #[must_use]
    pub fn none() -> Self {
        Self { words: [0; WORDS] }
    }

    /// Enables one rule (test/ caller convenience).
    pub fn add(&mut self, id: RuleId) {
        self.set(id);
    }

    #[must_use]
    pub fn is_enabled(&self, id: RuleId) -> bool {
        self.words[id.0 as usize / 64] & (1 << (id.0 as usize % 64)) != 0
    }

    fn set(&mut self, id: RuleId) {
        self.words[id.0 as usize / 64] |= 1 << (id.0 as usize % 64);
    }

    fn clear(&mut self, id: RuleId) {
        self.words[id.0 as usize / 64] &= !(1 << (id.0 as usize % 64));
    }
}

/// A registration's language-version predicate: the upstream registration
/// consults the library's settled language version and joins the run only
/// when the predicate admits it (`registerNodeProcessors`' version guard).
/// The default is no constraint — a rule absent from `VERSION_GATES`
/// registers unconditionally, so existing rules never carry the face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VersionGate {
    /// Registers at/above the version — the feature-`enabledIn`-milestone
    /// shape the rest of the version-gated family uses.
    #[expect(
        dead_code,
        reason = "constructed with the family's next gate; the face exists \
                  so both upstream predicate shapes are expressible (#1053)"
    )]
    AtLeast(LanguageVersion),
    /// Registers below the version — the tall-style formatter shape:
    /// `require_trailing_commas` registers below 3.7 and stays silent from
    /// 3.7, where `dart format` enforces the commas itself
    /// (require_trailing_commas.dart:52-56).
    Below(LanguageVersion),
}

impl VersionGate {
    fn admits(self, version: LanguageVersion) -> bool {
        match self {
            Self::AtLeast(floor) => version >= floor,
            Self::Below(ceiling) => version < ceiling,
        }
    }
}

/// The version-gated registrations, keyed by machine name. Consulted once
/// per run when [`crate::engine::Unit::version`] carries a settled version;
/// a gate whose predicate rejects the version clears the rule from the
/// run's enable set (the official silent-at-that-version posture).
static VERSION_GATES: &[(&str, VersionGate)] = &[(
    "require_trailing_commas",
    VersionGate::Below(LanguageVersion::new(3, 7)),
)];

/// `enabled` without the gated rules `version` rejects — the registration
/// consult of one run. One pass over `VERSION_GATES` (a two-entry-scale
/// slice); the per-node dispatch keeps testing the filtered bits, so the
/// gate costs nothing on the hot path.
pub(crate) fn version_gated(enabled: &EnableSet, version: LanguageVersion) -> EnableSet {
    let mut gated = *enabled;
    for &(name, gate) in VERSION_GATES {
        if !gate.admits(version)
            && let Some(id) = rule_by_name(name)
        {
            gated.clear(id);
        }
    }
    gated
}

/// `{n}` slot fill, sequentially (the templates use at most `{0}` today; the
/// loop stays general at zero cost for the common no-arg case).
pub(crate) fn render(template: &str, args: &[&str]) -> String {
    if args.is_empty() {
        return template.to_owned();
    }
    let mut out = template.to_owned();
    for (i, arg) in args.iter().enumerate() {
        out = out.replace(&format!("{{{i}}}"), arg);
    }
    out
}
