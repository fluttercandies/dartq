use super::{Group, RuleSpec};
use crate::rules::not_dispatched;

/// Official: the `unnecessary_type_check` diagnostic — an `x is T` whose
/// answer the static types fix at a constant reports at the check
/// (`_checkAllTypeChecks`,
/// `pkg/analyzer/lib/src/error/best_practices_verifier.dart:987-1041`).
/// The rule file left the linter in the Dart 3.15 main re-pin (no
/// `pkg/linter/lib/src/rules/unnecessary_type_check.dart`); the templates
/// live in `pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart`
/// (`unnecessaryTypeCheckTrue` / `unnecessaryTypeCheckFalse`, one
/// registry name across the polarity pair, both `STATIC_WARNING` with
/// published docs, :17642-17666).
///
/// Mirror posture ([`Group::Diagnostic`], `kinds: &[]` — the pubspec
/// `path:` family's shape): the oracle's verifier is an always-on
/// analysis pass (`library_analyzer.dart:481`), not a lint the enable
/// set gates, so the emission lives with the semantic tier's fold (the
/// `check_type_check` walk) and this registration exists only for the
/// name-resolution faces — the options/ignore faces resolve the name
/// and the template renders oracle-exact — never as an AST run arm
/// (a run row would double the semantic tier's and carry the wrong
/// lint tier).
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "unnecessary_type_check",
    group: Group::Diagnostic,
    problem: "Unnecessary type check; the result is always 'true'.",
    correction: "Try correcting the type check, or removing the type check.",
    kinds: &[],
    check: not_dispatched,
};
