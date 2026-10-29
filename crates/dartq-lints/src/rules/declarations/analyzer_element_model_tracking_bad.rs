use crate::registry::RuleSpec;
use crate::rules::not_dispatched;

/// Upstream `RuleState.internal` (`analyzer_element_model_tracking.dart:26`),
/// SDK-dogfooding only, registered to honor explicit naming — structurally
/// silent by design. Firing requires the SDK's own
/// `package:analyzer/src/fine/annotations.dart` tracking annotations on a
/// member of an `@elementClass` class. Message: `diagnostic.g.dart:152-158`.
pub(crate) static SPEC: RuleSpec =
    crate::spec!("analyzer_element_model_tracking_bad", &[], not_dispatched,);
