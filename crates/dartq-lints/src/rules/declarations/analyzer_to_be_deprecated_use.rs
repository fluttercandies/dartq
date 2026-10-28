use crate::registry::RuleSpec;
use crate::rules::not_dispatched;

/// Upstream `RuleState.internal` (`analyzer_to_be_deprecated_use.dart:44`),
/// SDK-dogfooding only, registered to honor explicit naming — structurally
/// silent by design. Its recognition is the one name-based face in the
/// internal family (a user-declared `ToBeDeprecated` class could fire
/// upstream); registering silent is the ruled under-approximation
/// (`lint_divergences.tsv`). Message: `diagnostic.g.dart:329-340`.
/// The spec is a hand-written literal: the 3.13.7 conformance pin's
/// linter does not carry this rule (it exists only in newer upstream
/// mains), so the generated metadata table has no row to resolve — the
/// same posture as the [`crate::registry`] `META_EXEMPT` faces. The
/// texts are the rule's established messages, kept byte-identical.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "analyzer_to_be_deprecated_use",
    group: crate::registry::Group::Standalone,
    problem: "'{0}' is to be deprecated, and should be used only by declarations that are also to be deprecated.{1}",
    correction: "Try using the replacement, or annotating the enclosing declaration with '@ToBeDeprecated'.",
    kinds: &[],
    check: not_dispatched,
};
