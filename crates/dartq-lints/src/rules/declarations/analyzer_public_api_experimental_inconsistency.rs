use crate::registry::{Group, RuleSpec};
use crate::rules::not_dispatched;

/// Upstream `RuleState.internal` (`analyzer_public_api.dart:28`, codes
/// `:32-38`), SDK-dogfooding only, registered to honor explicit naming —
/// structurally silent by design (same analyzer-URI gate). Message:
/// `diagnostic.g.dart:264-272`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "analyzer_public_api_experimental_inconsistency",
    group: Group::Standalone,
    problem: "Element makes use of experimental type(s), but is not itself marked with `@experimental`: {0}.",
    correction: "",
    kinds: &[],
    check: not_dispatched,
};
