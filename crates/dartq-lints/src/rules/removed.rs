//! The removed family: the 14 rules the SDK still registers as
//! `RemovedAnalysisRule` (`pkg/analyzer/lib/src/analysis_rule/
//! analysis_rule.dart:385` — state `RuleState.removed`,
//! `diagnosticCodes => const []`), the `removed` slice of the published
//! rules.json universe (264 = 233 stable + 11 experimental + 6 deprecated
//! + 14 removed).
//!
//! Observable oracle semantics, per the official sources:
//!
//! - **Registered, not unknown**: the rule sits in `Registry.ruleRegistry`,
//!   so naming it in `linter.rules:`, `analyzer-error-for:`,
//!   `cannot-ignore:`, or an ignore comment resolves — no `undefined_lint`
//!   — and `currentLintCodeNames()`
//!   (`analysis_options_parse_model.dart:82`) carries the uppercase name.
//! - **Never fires**: zero diagnostic codes; a run with the rule enabled
//!   emits nothing, even on the rule's historical violation shape.
//! - **The removal warnings belong to other faces**: enabling a removed
//!   rule directly in the *initial* options file reports `removed_lint`
//!   (`analysis_options_parse_model.dart:1308-1330`); ignoring one in code
//!   reports `removed_lint_use` (`ignore_validator.dart:219`). None of the
//!   14 carries a `replacedBy`, so the `replaced_lint` variants never
//!   apply. Those diagnostics are keyed to the rule name but emitted by
//!   the options/ignore faces — outside this crate's lint face.
//!
//! Each spec therefore carries `kinds: &[]` and the shared
//! [`not_dispatched`][crate::rules::not_dispatched] check — the engine
//! never dispatches it — with the `removed_lint` template as the spec's
//! message of record.

use crate::registry::{Group, RuleSpec};

/// The `removed_lint` problemMessage (`pkg/analyzer/lib/src/diagnostic/
/// diagnostic.g.dart`): the only official message keyed to a removed rule
/// name. The lint face itself renders nothing — zero dispatch kinds.
const REMOVED_PROBLEM: &str = "'{0}' was removed in Dart '{1}'";

/// The `removed_lint` correctionMessage.
const REMOVED_CORRECTION: &str = "Try removing the reference to '{0}'.";

/// Registers one removed rule: the registered-no-op [`RuleSpec`] plus the
/// registry-face test — the name resolves, a run with only the rule
/// enabled stays silent on its historical violation shape.
macro_rules! removed_rule {
    ($module:ident, $name:literal, $since:literal, $sample:literal) => {
        #[doc = concat!(
                    "Removed rule (`RemovedAnalysisRule`, ",
                    $since,
                    "): a registered no-op — the name resolves in the options and ",
                    "ignore faces, the rule never fires. Official: `pkg/linter/lib/src/rules/",
                    $name,
                    ".dart`."
                )]
        pub(crate) mod $module {
            use super::{Group, RuleSpec};

            pub(crate) static SPEC: RuleSpec = RuleSpec {
                name: $name,
                group: Group::Removed,
                problem: super::REMOVED_PROBLEM,
                correction: super::REMOVED_CORRECTION,
                kinds: &[],
                check: crate::rules::not_dispatched,
            };

            #[cfg(test)]
            mod tests {
                use crate::registry::EnableSet;
                use crate::testsupport::Fixture;

                /// The rule's historical violation shape: the code the rule
                /// once flagged must stay silent — zero-fire is the parity
                /// contract, not an implementation gap.
                const SAMPLE: &str = $sample;

                #[test]
                fn resolves_in_options_and_never_fires() {
                    let (set, unknown) = EnableSet::resolve([super::SPEC.name]);
                    assert!(
                        unknown.is_empty(),
                        "{} must resolve as a registered name",
                        super::SPEC.name
                    );
                    let fixture = Fixture::parse(SAMPLE);
                    assert!(
                        fixture.run(&set).is_empty(),
                        "{} must never fire",
                        super::SPEC.name
                    );
                }
            }
        }
    };
}

removed_rule!(
    always_require_non_null_named_parameters,
    "always_require_non_null_named_parameters",
    "removed in Dart 3.3.0",
    "void f({int? x}) {}"
);

removed_rule!(
    avoid_as,
    "avoid_as",
    "removed in Dart 2.12.0",
    "void f(Object y) { y as int; }"
);

removed_rule!(
    avoid_returning_null,
    "avoid_returning_null",
    "removed in Dart 3.3.0",
    "String? f() { return null; }"
);

removed_rule!(
    avoid_returning_null_for_future,
    "avoid_returning_null_for_future",
    "removed in Dart 3.3.0",
    "Future<int>? f() { return null; }"
);

removed_rule!(
    avoid_unstable_final_fields,
    "avoid_unstable_final_fields",
    "removed without a versioned since",
    "class C { final int x = 1; }"
);

removed_rule!(
    enable_null_safety,
    "enable_null_safety",
    "removed in Dart 3.0.0",
    "// @dart = 2.9\nvoid f() {}"
);

removed_rule!(
    invariant_booleans,
    "invariant_booleans",
    "removed in Dart 3.0.0",
    "void f(bool a) { if (a && a) print(a); }"
);

removed_rule!(
    iterable_contains_unrelated_type,
    "iterable_contains_unrelated_type",
    "removed in Dart 3.3.0",
    "void f() { <int>[1].contains('x'); }"
);

removed_rule!(
    list_remove_unrelated_type,
    "list_remove_unrelated_type",
    "removed in Dart 3.3.0",
    "void f() { <int>[1].remove('x'); }"
);

removed_rule!(
    package_api_docs,
    "package_api_docs",
    "removed in Dart 3.7.0",
    "int publicApi = 1;"
);

removed_rule!(
    prefer_bool_in_asserts,
    "prefer_bool_in_asserts",
    "removed in Dart 3.0.0",
    "void f(bool b) { assert(b == true); }"
);

removed_rule!(
    prefer_equal_for_default_values,
    "prefer_equal_for_default_values",
    "removed in Dart 3.0.0",
    "void f({int x: 1}) {}"
);

removed_rule!(
    super_goes_last,
    "super_goes_last",
    "removed in Dart 3.0.0",
    "class C extends B { C() : super(), x = 1; final int x; }"
);

removed_rule!(
    unsafe_html,
    "unsafe_html",
    "removed in Dart 3.7.0",
    "void f(e) { e.innerHtml = '<br>'; }"
);
