use crate::registry::{Group, RuleSpec};
use crate::rules::not_dispatched;

/// Upstream `RuleState.internal` (`analyzer_element_model_tracking.dart:26`),
/// SDK-dogfooding only, registered to honor explicit naming — structurally
/// silent by design (same SDK-only annotation face as
/// [`analyzer_element_model_tracking_bad`]). Message:
/// `diagnostic.g.dart:170-176`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "analyzer_element_model_tracking_zero",
    group: Group::Standalone,
    problem: "No required tracking annotation.",
    correction: "",
    kinds: &[],
    check: not_dispatched,
};
