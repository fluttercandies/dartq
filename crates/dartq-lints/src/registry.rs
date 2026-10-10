//! The rule registry: one static [`RuleSpec`] table, machine-name lookup, and
//! enable-set resolution.
//!
//! Every rule pins its behavior to the official implementation in
//! `.third/dart-sdk/pkg/linter/lib/src/rules/<rule>.dart` (cited per module)
//! and its message templates to the generated codes in
//! `pkg/linter/lib/src/diagnostic.g.dart` (cited in each spec). Group
//! membership follows `.third/lints/lib/{core,recommended}.yaml` (the
//! `package:lints` sets; recommended includes core).

use dartq_ast::NodeKind;

use crate::engine::Check;

/// The `package:lints` set a rule belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    /// `.third/lints/lib/core.yaml` — critical issues.
    Core,
    /// `.third/lints/lib/recommended.yaml` — core plus idiom/style rules.
    Recommended,
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

/// All rules through the wave-3 tranche. Order is registry order
/// ([`RuleId`] assignment), grouped core-first then alphabetically within
/// each group — keep the pin test in `tests/registry_pin.rs` in sync.
pub static RULES: &[&RuleSpec] = &[
    // —— core (`.third/lints/lib/core.yaml`) ——
    &crate::rules::empty::avoid_empty_else::SPEC,
    &crate::rules::directives::avoid_relative_lib_imports::SPEC,
    &crate::rules::types::avoid_shadowing_type_parameters::SPEC,
    &crate::rules::names::camel_case_extensions::SPEC,
    &crate::rules::names::camel_case_types::SPEC,
    &crate::rules::flow::SPEC,
    &crate::rules::docs::dangling_library_doc_comments::SPEC,
    &crate::rules::empty::empty_catches::SPEC,
    &crate::rules::names::file_names::SPEC,
    &crate::rules::declarations::hash_and_equals::SPEC,
    &crate::rules::directives::library_annotations::SPEC,
    &crate::rules::flow::no_duplicate_case_values::SPEC,
    &crate::rules::names::non_constant_identifier_names::SPEC,
    &crate::rules::declarations::prefer_generic_function_type_aliases::SPEC,
    &crate::rules::declarations::prefer_typing_uninitialized_variables::SPEC,
    &crate::rules::docs::unintended_html_in_doc_comment::SPEC,
    &crate::rules::directives::use_string_in_part_of_directives::SPEC,
    // —— recommended (`.third/lints/lib/recommended.yaml`) ——
    &crate::rules::declarations::avoid_init_to_null::SPEC,
    &crate::rules::declarations::avoid_return_types_on_setters::SPEC,
    &crate::rules::flow::avoid_returning_null_for_void::SPEC,
    &crate::rules::expressions::avoid_single_cascade_in_expression_statements::SPEC,
    &crate::rules::names::constant_identifier_names::SPEC,
    &crate::rules::flow::control_flow_in_finally::SPEC,
    &crate::rules::empty::empty_constructor_bodies::SPEC,
    &crate::rules::empty::empty_statements::SPEC,
    &crate::rules::directives::library_prefixes::SPEC,
    &crate::rules::types::library_private_types_in_public_api::SPEC,
    &crate::rules::directives::no_leading_underscores_for_library_prefixes::SPEC,
    &crate::rules::names::no_leading_underscores_for_local_identifiers::SPEC,
    &crate::rules::expressions::prefer_adjacent_string_concatenation::SPEC,
    &crate::rules::collections::prefer_collection_literals::SPEC,
    &crate::rules::expressions::prefer_conditional_assignment::SPEC,
    &crate::rules::expressions::prefer_contains::SPEC,
    &crate::rules::declarations::prefer_function_declarations_over_variables::SPEC,
    &crate::rules::expressions::prefer_if_null_operators::SPEC,
    &crate::rules::collections::prefer_inlined_adds::SPEC,
    &crate::rules::expressions::prefer_is_not_operator::SPEC,
    &crate::rules::expressions::prefer_null_aware_operators::SPEC,
    &crate::rules::collections::prefer_spread_collections::SPEC,
    &crate::rules::docs::slash_for_doc_comments::SPEC,
    &crate::rules::strings::unnecessary_brace_in_string_interps::SPEC,
    &crate::rules::declarations::unnecessary_constructor_name::SPEC,
    &crate::rules::declarations::unnecessary_late::SPEC,
    &crate::rules::directives::unnecessary_library_name::SPEC,
    &crate::rules::expressions::unnecessary_new::SPEC,
    &crate::rules::expressions::unnecessary_null_aware_assignments::SPEC,
    &crate::rules::expressions::unnecessary_null_in_if_null_operators::SPEC,
    &crate::rules::strings::unnecessary_string_escapes::SPEC,
    &crate::rules::declarations::type_init_formals::SPEC,
    &crate::rules::collections::unnecessary_to_list_in_spreads::SPEC,
    &crate::rules::declarations::use_function_type_syntax_for_parameters::SPEC,
    &crate::rules::flow::use_rethrow_when_possible::SPEC,
];

/// The rule with machine name `name` (linear scan; a one-per-run lookup, not
/// a hot path).
#[must_use]
pub fn rule_by_name(name: &str) -> Option<RuleId> {
    RULES
        .iter()
        .position(|spec| spec.name == name)
        .map(|i| RuleId(i as u16))
}

/// The set of rules a run executes.
///
/// Resolved once from machine names; the hot path tests bits only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnableSet {
    words: [u64; WORDS],
}

/// `RULES.len()` will stay far below 64 for a long time; the word count is
/// asserted in `tests/registry_pin.rs` so growth past one word is caught.
const WORDS: usize = 1;

impl EnableSet {
    /// Every registered rule.
    #[must_use]
    pub fn all() -> Self {
        let mut set = Self { words: [0] };
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
        let mut set = Self { words: [0] };
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
        Self { words: [0] }
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
