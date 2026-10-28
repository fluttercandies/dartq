use crate::registry::RuleSpec;
use crate::rules::not_dispatched;

/// Upstream `RuleState.internal` (`analyzer_element_model_tracking.dart:26`),
/// SDK-dogfooding only, registered to honor explicit naming — structurally
/// silent by design (same SDK-only annotation face as
/// [`analyzer_element_model_tracking_bad`]). Message:
/// `diagnostic.g.dart:161-167`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "analyzer_element_model_tracking_more_than_one",
    &[],
    not_dispatched,
);
