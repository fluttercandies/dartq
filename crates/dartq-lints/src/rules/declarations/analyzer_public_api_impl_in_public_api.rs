use crate::registry::RuleSpec;
use crate::rules::not_dispatched;

/// Upstream `RuleState.internal` (`analyzer_public_api.dart:28`, codes
/// `:32-38`), SDK-dogfooding only, registered to honor explicit naming —
/// structurally silent by design (same analyzer-URI gate). Message:
/// `diagnostic.g.dart:305-312`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "analyzer_public_api_impl_in_public_api",
    &[],
    not_dispatched,
);
