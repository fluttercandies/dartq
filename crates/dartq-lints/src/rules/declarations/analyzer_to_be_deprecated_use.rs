use crate::registry::RuleSpec;
use crate::rules::not_dispatched;

/// Upstream `RuleState.internal` (`analyzer_to_be_deprecated_use.dart:44`),
/// SDK-dogfooding only, registered to honor explicit naming — structurally
/// silent by design. Its recognition is the one name-based face in the
/// internal family (a user-declared `ToBeDeprecated` class could fire
/// upstream); registering silent is the ruled under-approximation
/// (`lint_divergences.tsv`). Message: `diagnostic.g.dart:329-340`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "analyzer_to_be_deprecated_use",
    &[],
    not_dispatched,
);
