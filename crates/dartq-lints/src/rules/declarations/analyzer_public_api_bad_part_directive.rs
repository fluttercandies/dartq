use crate::registry::{Group, RuleSpec};
use crate::rules::not_dispatched;

/// Upstream `RuleState.internal` (`analyzer_public_api.dart:28`, codes
/// `:32-38`), SDK-dogfooding only, registered to honor explicit naming —
/// structurally silent by design. Firing requires the declaring library's
/// URI to be `package:analyzer/*` outside `src/` (`isInAnalyzerPublicLib`).
/// Message: `diagnostic.g.dart:190-198`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "analyzer_public_api_bad_part_directive",
    group: Group::Standalone,
    problem: "Part directives in the analyzer public API should point to files in the analyzer public API.",
    correction: "",
    kinds: &[],
    check: not_dispatched,
};
