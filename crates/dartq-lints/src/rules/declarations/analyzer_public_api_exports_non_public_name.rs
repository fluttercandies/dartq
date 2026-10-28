use crate::registry::RuleSpec;
use crate::rules::not_dispatched;

/// Upstream `RuleState.internal` (`analyzer_public_api.dart:28`, codes
/// `:32-38`), SDK-dogfooding only, registered to honor explicit naming —
/// structurally silent by design (same analyzer-URI gate). Message:
/// `diagnostic.g.dart:287-295`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "analyzer_public_api_exports_non_public_name",
    &[],
    not_dispatched,
);
