use crate::registry::{Group, RuleSpec};
use crate::rules::not_dispatched;

/// Upstream `RuleState.internal` (`analyzer_element_model_tracking.dart:26`),
/// SDK-dogfooding only, registered to honor explicit naming — structurally
/// silent by design. Firing requires the SDK's own
/// `package:analyzer/src/fine/annotations.dart` tracking annotations on a
/// member of an `@elementClass` class. Message: `diagnostic.g.dart:152-158`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "analyzer_element_model_tracking_bad",
    group: Group::Standalone,
    problem: "Bad tracking annotation for this member.",
    correction: "",
    kinds: &[],
    check: not_dispatched,
};
