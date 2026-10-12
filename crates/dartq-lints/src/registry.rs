//! The rule registry: one static [`RuleSpec`] table, machine-name lookup, and
//! enable-set resolution.
//!
//! Every rule pins its behavior to the official implementation in
//! `.third/dart-sdk/pkg/linter/lib/src/rules/<rule>.dart` (cited per module)
//! and its message templates to the generated codes in
//! `pkg/linter/lib/src/diagnostic.g.dart` (cited in each spec). Group
//! membership follows `.third/lints/lib/{core,recommended}.yaml` (the
//! `package:lints` sets; recommended includes core); the removed rules
//! follow the SDK's `RemovedAnalysisRule` registrations (the rules.json
//! `removed` state).

use dartq_ast::NodeKind;

use crate::engine::Check;

/// The `package:lints` set a rule belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    /// `.third/lints/lib/core.yaml` — critical issues.
    Core,
    /// `.third/lints/lib/recommended.yaml` — core plus idiom/style rules.
    Recommended,
    /// A lint-tier mirror of an analyzer diagnostic that is not a
    /// `package:lints` member. No include set carries it — the enable set
    /// is entirely options-driven (no options file, no lints) — so the
    /// rule joins a run only by explicit name (`analysis_options`
    /// `linter.rules` or a caller-built [`EnableSet`]).
    Diagnostic,
    /// A rule the SDK removed but still registers (`RemovedAnalysisRule`,
    /// `diagnosticCodes => const []`): the name resolves in the options
    /// and ignore faces — no `undefined_lint`, and the uppercase name is
    /// in `currentLintCodeNames`
    /// (`analysis_options_parse_model.dart:82`) — while the rule never
    /// fires. Not a `package:lints` member: no include set carries it and
    /// the lint-gate freeze never judges it.
    Removed,
}

/// Index into [`RULES`]. Stable within one build of this crate; the machine
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

/// All rules through the #586 standard-set completion (the full
/// `package:lints` union: 34 core + 55 recommended-only), the #570
/// diagnostic mirror, and the #986 removed-registry no-ops (the 14
/// `RemovedAnalysisRule` registrations). Order is registry order
/// ([`RuleId`] assignment), grouped core-first then alphabetically within
/// each group, the mirror section, then the wave sections in wave order
/// (the removed block first; each new wave appends its section) — keep the
/// pin test in `tests/registry_pin.rs` in sync.
pub static RULES: &[&RuleSpec] = &[
    // —— core (`.third/lints/lib/core.yaml`) ——
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
    // —— recommended (`.third/lints/lib/recommended.yaml`) ——
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
    &crate::rules::expressions::unnecessary_type_check::SPEC,
    // —— removed (the `RemovedAnalysisRule` no-ops, plan W1 #986; the
    // rules.json `removed` state, alphabetical): the names resolve in the
    // options and ignore faces, the rules never fire ——
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
];

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
    RULES
        .iter()
        .position(|spec| spec.name.eq_ignore_ascii_case(name))
        .map(|i| RuleId(i as u16))
}

/// The set of rules a run executes.
///
/// Resolved once from machine names; the hot path tests bits only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnableSet {
    words: [u64; WORDS],
}

/// `RULES.len()` outgrew one 64-bit word at 68 rules (wave 3, #315) and
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
        for i in 0..RULES.len() {
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
