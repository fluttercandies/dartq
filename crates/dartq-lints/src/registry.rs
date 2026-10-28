//! The rule registry: one static [`RuleSpec`] table, machine-name lookup, and
//! enable-set resolution.
//!
//! Every rule pins its behavior to the official implementation in upstream
//! Dart SDK source, `pkg/linter/lib/src/rules/<rule>.dart` (cited per
//! module); its message templates, group, state, and docs flag resolve
//! through the generated metadata table
//! ([`crate::generated::rule_meta`], #2676 — `pkg/linter/messages.yaml`
//! of the pinned SDK tree is the source). Group membership follows the
//! pinned `package:lints` snapshot (`core.yaml` / `recommended.yaml`,
//! upstream dart-lang/lints; recommended includes core); the removed rules
//! follow the SDK's `RemovedAnalysisRule` registrations (the rules.json
//! `removed` state).

use std::sync::LazyLock;

use dartq_ast::NodeKind;
use dartq_language::{Feature, LanguageVersion};

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

/// A `static SPEC` literal whose metadata (group, message templates) the
/// generated table resolves at compile time (#2676): every rule file
/// writes only its name, dispatch kinds, and check here — the
/// hand-copied metadata fields are gone. A name the table does not carry
/// fails the build at the `static` initializer (the const-fn panic in
/// [`crate::generated::rule_meta`]); the four diagnostic mirrors and the
/// removed no-ops keep hand-written literals (see [`META_EXEMPT`] and
/// `rules/removed.rs`).
macro_rules! spec {
    ($name:literal, $kinds:expr, $check:expr $(,)?) => {
        RuleSpec {
            name: $name,
            group: $crate::generated::rule_meta::meta_group($name),
            problem: $crate::generated::rule_meta::meta_problem($name),
            correction: $crate::generated::rule_meta::meta_correction($name),
            kinds: $kinds,
            check: $check,
        }
    };
}
pub(crate) use spec;

/// The registered names the generated metadata table deliberately does
/// not carry: the analyzer-diagnostic mirrors — pubspec `path:` validator
/// codes and the `unnecessary_type_check` pair — which are analyzer
/// codes, not `LinterLintCode` entries of `pkg/linter/messages.yaml`
/// (their specs hand-write the analyzer messages; the pin test's
/// `OFFICIAL_DIAGNOSTIC_MIRRORS` holds the same four).
const META_EXEMPT: &[&str] = &[
    "path_does_not_exist",
    "path_not_posix",
    "path_pubspec_does_not_exist",
    "unnecessary_type_check",
];

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
    // The startup consistency check: the sort above (and `rule_by_name`'s
    // first-match position lookup) is well-defined only under unique names,
    // and every name-keyed wiring table below must resolve against the
    // merged registry. Runs in release too — a drifted gate or mount name
    // is a wrong-but-silent run, which fails loudly here (the audit's
    // registry-wiring class) instead of at a distant first query.
    assert_startup_consistency(&rules);
    rules
});

/// The one-time registry startup check (runs at [`RULES`] init, the first
/// registry touch of any process): duplicate machine names break the name
/// order and `rule_by_name`'s first-match lookup, and every name-keyed
/// wiring table — [`SEMANTIC_RULES`], [`REGISTRATION_GATES`],
/// [`INCOMPATIBLE_RULES`] (keys and peers), [`PUBSPEC_PASS_SEAM`],
/// [`RUN_EPILOGUE_SEAM`], [`LATE_JOIN_RULES`] — must resolve against the
/// registry, case-insensitively like [`rule_by_name`]. A miss fails fast
/// here with the offenders named, instead of surfacing as a silent no-op
/// (a skipped version gate, a shrunk semantic mount, an empty incompatible
/// set). Takes the merged slice rather than calling [`rules()`]: this runs
/// inside the [`RULES`] initializer, and re-entry would deadlock.
fn assert_startup_consistency(rules: &[&'static RuleSpec]) {
    let mut sorted: Vec<&str> = rules.iter().map(|spec| spec.name).collect();
    sorted.sort_unstable();
    let duplicates: Vec<&str> = sorted
        .windows(2)
        .filter_map(|pair| (pair[0] == pair[1]).then_some(pair[0]))
        .collect();

    fn unresolved<'a>(
        rules: &'a [&'static RuleSpec],
        names: impl Iterator<Item = &'static str>,
    ) -> Vec<&'static str> {
        names
            .filter(|name| !rules.iter().any(|spec| spec.name.eq_ignore_ascii_case(name)))
            .collect()
    }

    let mut drift: Vec<String> = Vec::new();
    if !duplicates.is_empty() {
        drift.push(format!("duplicate rule names {duplicates:?}"));
    }
    let semantic = unresolved(rules, SEMANTIC_RULES.iter().copied());
    if !semantic.is_empty() {
        drift.push(format!("SEMANTIC_RULES unregistered names {semantic:?}"));
    }
    let gated = unresolved(rules, REGISTRATION_GATES.iter().map(|(name, _)| *name));
    if !gated.is_empty() {
        drift.push(format!("REGISTRATION_GATES unregistered names {gated:?}"));
    }
    let incompatible_keys = unresolved(rules, INCOMPATIBLE_RULES.iter().map(|(name, _)| *name));
    if !incompatible_keys.is_empty() {
        drift.push(format!("INCOMPATIBLE_RULES unregistered keys {incompatible_keys:?}"));
    }
    let incompatible_peers =
        unresolved(rules, INCOMPATIBLE_RULES.iter().flat_map(|(_, peers)| peers.iter().copied()));
    if !incompatible_peers.is_empty() {
        drift.push(format!(
            "INCOMPATIBLE_RULES unregistered peer names {incompatible_peers:?}"
        ));
    }
    let pubspec_seam = unresolved(rules, PUBSPEC_PASS_SEAM.iter().copied());
    if !pubspec_seam.is_empty() {
        drift.push(format!("PUBSPEC_PASS_SEAM unregistered names {pubspec_seam:?}"));
    }
    let epilogue_seam = unresolved(rules, RUN_EPILOGUE_SEAM.iter().copied());
    if !epilogue_seam.is_empty() {
        drift.push(format!("RUN_EPILOGUE_SEAM unregistered names {epilogue_seam:?}"));
    }
    let late_join = unresolved(rules, LATE_JOIN_RULES.iter().copied());
    if !late_join.is_empty() {
        drift.push(format!("LATE_JOIN_RULES unregistered names {late_join:?}"));
    }
    // The generated-metadata arm (#2676): every registered name resolves
    // in the table (the mirrors exempt) and — for the rules whose spec
    // resolves through it — the spec's group agrees with the row's. A
    // hand-written spec that drifts from the table (a stale removed
    // classification, a regenerated group) fails here in release too,
    // the same silent-wrong-run protection the wiring tables get.
    let mut missing_meta: Vec<&str> = rules
        .iter()
        .map(|spec| spec.name)
        .filter(|name| !META_EXEMPT.contains(name))
        .filter(|name| crate::generated::rule_meta::rule_meta(name).is_none())
        .collect();
    missing_meta.sort_unstable();
    if !missing_meta.is_empty() {
        drift.push(format!(
            "names without generated metadata rows {missing_meta:?} — regenerate \
             the table or extend META_EXEMPT with a justification"
        ));
    }
    let group_drift: Vec<&str> = rules
        .iter()
        .filter(|spec| !META_EXEMPT.contains(&spec.name))
        .filter_map(|spec| {
            let meta = crate::generated::rule_meta::rule_meta(spec.name)?;
            (meta.group != spec.group).then_some(spec.name)
        })
        .collect();
    if !group_drift.is_empty() {
        drift.push(format!(
            "specs whose group disagrees with the generated table {group_drift:?}"
        ));
    }

    assert!(
        drift.is_empty(),
        "lint registry wiring drift — fix the named tables in this file \
         (or register the named rules) before shipping:\n  {}",
        drift.join("\n  ")
    );
}

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

    /// Whether the two sets share any enabled rule — the word-AND the
    /// property gates test ([`mounts_semantics`]).
    #[must_use]
    pub fn intersects(&self, other: &Self) -> bool {
        self.words
            .iter()
            .zip(other.words.iter())
            .any(|(a, b)| a & b != 0)
    }
}

/// A registration's language-version predicate: the upstream registration
/// consults the library's settled language version and joins the run only
/// when the predicate admits it (`registerNodeProcessors`' version guard).
/// The default is no constraint — a rule absent from
/// [`REGISTRATION_GATES`] registers unconditionally, so existing rules
/// never carry the face.
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

/// A registration's feature or version predicate: the upstream
/// `isFeatureEnabled`/version bails that decide whether a rule registers
/// for a library (the rule-constructor guards, upstream Dart SDK source,
/// `pkg/linter/lib/src/rules/<name>.dart`). The default is no constraint —
/// a rule absent from [`REGISTRATION_GATES`] registers unconditionally, so
/// existing rules never carry the face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    /// Registers only with the feature enabled — the plain
    /// `isFeatureEnabled` registration bail.
    Feature(Feature),
    /// Registers only with the feature disabled — the inverted bail: a
    /// superseding form took the feature's release over (e.g. the
    /// declaring-parameter rules supersede the `final`/`var` parameter
    /// ones once primary constructors ship).
    NotFeature(Feature),
    /// Registers under a language-version predicate ([`VersionGate`]); a
    /// run without a settled version face (the latest-default posture)
    /// admits.
    Version(VersionGate),
}

impl Gate {
    /// Whether a unit carrying `features` and `version` admits this gate.
    fn admits(
        self,
        features: &crate::engine::Features,
        version: Option<LanguageVersion>,
    ) -> bool {
        match self {
            Self::Feature(feature) => features.is_enabled(feature),
            Self::NotFeature(feature) => !features.is_enabled(feature),
            Self::Version(gate) => match version {
                Some(version) => gate.admits(version),
                None => true,
            },
        }
    }
}

/// The feature- and version-gated registrations, keyed by machine name —
/// every upstream `isFeatureEnabled`/version registration bail the engine
/// honors, one row per gated rule with its rule-file citation (the same
/// citations the gated rules' own early returns carry; those in-rule
/// mirrors predate the table and stay as defense-in-depth). Consulted once
/// per run ([`registration_gated`]); a gate the unit's faces reject clears
/// the rule from the run's enable set (the official silent-at-that-version
/// / feature-off posture).
///
/// Linkage is forced twice: every `Feature` here is a variant of the
/// generated registry enum (an unknown feature cannot compile) and
/// resolves through [`crate::engine::Features::is_enabled`]'s exhaustive
/// stance match (a feature without a lints stance fails compilation), and
/// every rule name is held against the merged registry at [`RULES`] init
/// by [`assert_startup_consistency`] — in release builds too, so a
/// drifted name is a fast named failure, never a silent no-op.
pub(crate) static REGISTRATION_GATES: &[(&str, Gate)] = &[
    // `patterns` (shipped 3.0, `experimental_features.yaml:387-389`): the
    // pattern-era rules register only with it; the legacy-case rules only
    // without it.
    ("switch_on_type", Gate::Feature(Feature::Patterns)), // registration bail (:34)
    ("simplify_variable_pattern", Gate::Feature(Feature::Patterns)), // registration bail (:32)
    ("unnecessary_breaks", Gate::Feature(Feature::Patterns)), // registers only with patterns on
    ("no_duplicate_case_values", Gate::NotFeature(Feature::Patterns)), // `member is SwitchCase` gate (:41)
    ("invalid_case_patterns", Gate::NotFeature(Feature::Patterns)), // registration bail (:41)
    // `wildcard_variables` (shipped 3.7): the multi-underscore face
    // registers only with it; the lone-use rule only without it.
    ("no_wildcard_variable_uses", Gate::NotFeature(Feature::WildcardVariables)), // (:36-38)
    ("unnecessary_underscores", Gate::Feature(Feature::WildcardVariables)), // registration bail (:33)
    // `primary_constructors` (shipped 3.13, `enabledIn: 3.13.0`): the
    // primary-form family registers only with it; the superseded
    // parameter rules only without it. (`unnecessary_final` is NOT here:
    // only its parameter-visitor faces skip under the feature
    // (unnecessary_final.dart:66-68) — an in-rule body check, not a
    // registration bail; the rule registers unconditionally.)
    ("use_primary_constructors", Gate::Feature(Feature::PrimaryConstructors)), // registration bail (:35)
    ("use_declaring_parameters", Gate::Feature(Feature::PrimaryConstructors)), // registration bail (:35)
    ("initialize_in_field_declaration", Gate::Feature(Feature::PrimaryConstructors)), // registration bail (:35)
    ("empty_container_bodies", Gate::Feature(Feature::PrimaryConstructors)), // registration bail (:35)
    ("unnecessary_const_in_enum_constructor", Gate::Feature(Feature::PrimaryConstructors)), // registration bail (:36-40)
    ("unnecessary_type_name_in_constructor", Gate::Feature(Feature::PrimaryConstructors)), // registration bail (:36-38)
    ("avoid_final_parameters", Gate::NotFeature(Feature::PrimaryConstructors)), // inverted bail (:56-59)
    ("prefer_final_parameters", Gate::NotFeature(Feature::PrimaryConstructors)), // inverted bail (:48-52)
    ("var_with_no_type_annotation", Gate::NotFeature(Feature::PrimaryConstructors)), // inverted bail (:34-36)
    // `unnamed_libraries` (shipped 2.19, `experimental_features.yaml:397-400`).
    ("unnecessary_library_name", Gate::Feature(Feature::UnnamedLibraries)), // registration bail (:32-34)
    // `enhanced_parts` (experimental — no `enabledIn`,
    // `tools/experimental_features.yaml:150-152`).
    ("use_string_in_part_of_directives", Gate::NotFeature(Feature::EnhancedParts)), // registration bail (:32-34)
    // `super_parameters` (shipped 2.17, `enabledIn: 2.17.0`).
    ("use_super_parameters", Gate::Feature(Feature::SuperParameters)), // registration bail (:43)
    // `null_aware_elements` (shipped 3.8, `enabledIn: 3.8.0`).
    ("use_null_aware_elements", Gate::Feature(Feature::NullAwareElements)), // registration bail (:35)
    // The settled-version face: `require_trailing_commas` registers below
    // 3.7 and stays silent from 3.7, where `dart format` enforces the
    // commas itself (require_trailing_commas.dart:52-56).
    ("require_trailing_commas", Gate::Version(VersionGate::Below(LanguageVersion::new(3, 7)))),
];

/// Doc-link anchor for the frozen `rules/flow/require_trailing_commas.rs`
/// module docs, which still cite the table by its pre-unification name
/// (`crate::registry::VERSION_GATES`) — the table itself is the unified
/// [`REGISTRATION_GATES`] above.
#[expect(
    unused_imports,
    reason = "doc-link anchor for the frozen `require_trailing_commas.rs` module \
              docs; rustdoc resolution is its only consumer"
)]
pub(crate) use REGISTRATION_GATES as VERSION_GATES;

/// The registered rules whose run consumes the per-file semantic fold: every
/// rule whose check call-graph reads [`crate::engine::Check::semantics`] —
/// directly, or through a shared semantics-reading helper
/// (`same_file::nullable_extension_member`,
/// `declarations::bridge_inherited_member` /
/// `super_formal_provably_unmatched`,
/// `collections::inferred_binding_type_face`, the flutter domain's
/// `spells_framework`/`resolves_user_container`/`chain_spells_root`
/// refutation faces) — plus `use_build_context_synchronously`, which reads
/// the fold's `AsyncContextUse` facts through
/// [`crate::engine::Unit::async_context_uses`] instead of the query face.
/// Enumerated like [`INCOMPATIBLE_RULES`]; membership is what mounts the
/// driver-side fold ([`mounts_semantics`]), and the pin test
/// (`tests/registry_pin.rs`) holds the list against the accessor's readers.
pub static SEMANTIC_RULES: &[&str] = &[
    "always_put_required_named_parameters_first",
    "annotate_overrides",
    "avoid_bool_literals_in_conditional_expressions",
    "avoid_classes_with_only_static_members",
    "avoid_dynamic_calls",
    "avoid_function_literals_in_foreach_calls",
    "avoid_print",
    "avoid_renaming_method_parameters",
    "avoid_slow_async_io",
    "avoid_type_to_string",
    "avoid_types_on_closure_parameters",
    "avoid_unnecessary_containers",
    "await_only_futures",
    "cast_nullable_to_non_nullable",
    "collection_methods_unrelated_type",
    "comment_references",
    "diagnostic_describe_all_properties",
    "future_sync_value",
    "implicit_call_tearoffs",
    "invalid_runtime_check_with_js_interop_types",
    "literal_only_boolean_expressions",
    "matching_super_parameters",
    "no_dynamic_casts",
    "no_literal_bool_comparisons",
    "no_logic_in_create_state",
    "no_runtimetype_tostring",
    "noop_primitive_operations",
    "null_check_on_nullable_type_parameter",
    "null_closures",
    "overridden_fields",
    "prefer_collection_literals",
    "prefer_contains",
    "prefer_for_elements_to_map_fromiterable",
    "prefer_initializing_formals",
    "prefer_int_literals",
    "prefer_is_empty",
    "prefer_is_not_empty",
    "prefer_iterable_wheretype",
    "prefer_null_aware_method_calls",
    "sized_box_for_whitespace",
    "sized_box_shrink_expand",
    "sort_child_properties_last",
    "type_annotate_public_apis",
    "type_init_formals",
    "unnecessary_getters_setters",
    "unnecessary_lambdas",
    "unnecessary_null_aware_operator_on_extension_on_nullable",
    "unnecessary_null_checks",
    "unnecessary_nullable_for_final_variable_declarations",
    "unnecessary_parenthesis",
    "unnecessary_statements",
    "unnecessary_string_interpolations",
    "unnecessary_this",
    "unnecessary_this_alias",
    "unnecessary_to_list_in_spreads",
    "unnecessary_unawaited",
    "unrelated_type_equality_checks",
    "use_build_context_synchronously",
    "use_colored_box",
    "use_decorated_box",
    "use_full_hex_values_for_flutter_colors",
    "use_key_in_widget_constructors",
    "use_super_parameters",
    "use_truncating_division",
    "void_checks",
];

/// The fold-consumer set resolved from [`SEMANTIC_RULES`]: the word-AND
/// target behind [`mounts_semantics`]. Built once — the names are
/// compile-time constants, the positions registry-derived. Name drift is
/// impossible past [`assert_startup_consistency`] (which holds every
/// [`SEMANTIC_RULES`] name against the registry, in release too, at the
/// earlier [`RULES`] init this lock depends on).
static SEMANTIC_SET: LazyLock<EnableSet> = LazyLock::new(|| {
    EnableSet::resolve(SEMANTIC_RULES.iter().copied()).0
});

/// Whether `set` enables any fold-consuming rule — the predicate behind the
/// per-file semantic-fold mount (the driver's `Analysis` build) and the memo
/// key's library-context generation half. One word-AND per call; a file
/// whose enabled set touches no member mounts nothing.
#[must_use]
pub fn mounts_semantics(set: &EnableSet) -> bool {
    set.intersects(&SEMANTIC_SET)
}

/// `enabled` without the gated rules the unit's faces reject — the
/// registration consult of one run, over [`REGISTRATION_GATES`]
/// (a 21-entry-scale slice); the per-node dispatch keeps testing the
/// filtered bits, so the gates cost nothing on the hot path.
pub(crate) fn registration_gated(
    enabled: &EnableSet,
    features: &crate::engine::Features,
    version: Option<LanguageVersion>,
) -> EnableSet {
    let mut gated = *enabled;
    for &(name, gate) in REGISTRATION_GATES {
        if !gate.admits(features, version)
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

#[cfg(test)]
mod gate_tests {
    use super::*;
    use crate::engine::Features;

    /// Every gate row names a registered rule — the startup consistency
    /// arm's mirror as a direct test (the assert itself fires at `RULES`
    /// init in every process; this names the contract in test form).
    #[test]
    fn every_gate_names_a_registered_rule() {
        for &(name, _) in REGISTRATION_GATES {
            assert!(
                rule_by_name(name).is_some(),
                "REGISTRATION_GATES names unregistered rule `{name}`"
            );
        }
    }

    /// Every gated feature resolves through the lints stance fold: a set
    /// carrying exactly that feature folds its bit on, so a `Gate::Feature`
    /// entry can never ride a feature the [`Features`] projection silently
    /// drops (the fold and the gate lookup agree by construction — pinned
    /// here because they are two matches).
    #[test]
    fn every_gated_feature_has_a_stance_bit() {
        for &(_, gate) in REGISTRATION_GATES {
            let feature = match gate {
                Gate::Feature(feature) | Gate::NotFeature(feature) => feature,
                Gate::Version(_) => continue,
            };
            let set = dartq_language::FeatureSet::latest().with_experiment(feature);
            assert!(
                Features::from_feature_set(set).is_enabled(feature),
                "gate rides `{}`, which the Features fold drops",
                feature.meta().name
            );
        }
    }

    /// The registration consult: the default (latest) features clear the
    /// inverted-bail rules and keep the plain ones; the all-off fold
    /// clears the plain ones and keeps the inverted; a version gate
    /// consults only a settled version face.
    #[test]
    fn registration_gated_filters_by_polarity() {
        let all = EnableSet::resolve(REGISTRATION_GATES.iter().map(|(name, _)| *name)).0;
        let enabled = |set: &EnableSet, name: &str| {
            rule_by_name(name).is_some_and(|id| set.is_enabled(id))
        };

        let latest = registration_gated(&all, &Features::default(), None);
        assert!(enabled(&latest, "use_primary_constructors"));
        assert!(enabled(&latest, "use_super_parameters"));
        assert!(!enabled(&latest, "var_with_no_type_annotation"));
        assert!(!enabled(&latest, "no_wildcard_variable_uses"));
        // No settled version face: the version gate admits (the official
        // latest-default posture); a settled 3.7 face clears it.
        assert!(enabled(&latest, "require_trailing_commas"));
        let gated_37 = registration_gated(
            &all,
            &Features::default(),
            Some(LanguageVersion::new(3, 7)),
        );
        assert!(!enabled(&gated_37, "require_trailing_commas"));

        // The 2.12 floor predates every shipped milestone among the seven
        // stances (the earliest is `super-parameters` at 2.17), so its
        // fold is all-off: the plain-bail rules clear, the inverted ones
        // register.
        let pre_patterns = registration_gated(
            &all,
            &Features::from_feature_set(dartq_language::FeatureSet::for_language_version(
                LanguageVersion::new(2, 12),
            )),
            None,
        );
        assert!(!enabled(&pre_patterns, "use_primary_constructors"));
        assert!(!enabled(&pre_patterns, "unnecessary_underscores"));
        assert!(!enabled(&pre_patterns, "use_null_aware_elements"));
        assert!(!enabled(&pre_patterns, "use_super_parameters"));
        assert!(!enabled(&pre_patterns, "unnecessary_library_name"));
        assert!(enabled(&pre_patterns, "var_with_no_type_annotation"));
        assert!(enabled(&pre_patterns, "no_wildcard_variable_uses"));
        assert!(enabled(&pre_patterns, "no_duplicate_case_values"));
    }
}
