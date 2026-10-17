//! The rule-state data the pinned SDK's linter registry attaches to its
//! registered rules (dart 3.15 main): the `since` versions the
//! `removed_lint` / `removed_lint_use` / `deprecated_lint` faces render.
//! Membership stays the lint registry's ([`dartq_lints::Group::Removed`] for
//! the removed set; the tables here only carry the version each state
//! renders) — version-pinned data like the engine's `KNOWN_EXPERIMENTS`.

/// The removed rules' `RemovedAnalysisRule` `since` versions (each rule
/// file's registration in `pkg/linter/lib/src/rules/<name>.dart`), keyed by
/// the canonical rule name. `None` — the state carries no `since` — renders
/// Dart's `null` (`state.since.toString()`, `parse_model.dart:1316`) and
/// applies to every SDK (`_beforeCurrentConstraint`, `parse_model.dart`
/// :1197-1205).
pub(crate) const REMOVED_SINCE: &[(&str, Option<&str>)] = &[
    ("always_require_non_null_named_parameters", Some("3.3.0")),
    ("avoid_as", Some("2.12.0")),
    ("avoid_returning_null", Some("3.3.0")),
    ("avoid_returning_null_for_future", Some("3.3.0")),
    ("avoid_unstable_final_fields", None),
    ("enable_null_safety", Some("3.0.0")),
    ("invariant_booleans", Some("3.0.0")),
    ("iterable_contains_unrelated_type", Some("3.3.0")),
    ("list_remove_unrelated_type", Some("3.3.0")),
    ("package_api_docs", Some("3.7.0")),
    ("prefer_bool_in_asserts", Some("3.0.0")),
    ("prefer_equal_for_default_values", Some("3.0.0")),
    ("super_goes_last", Some("3.0.0")),
    ("unsafe_html", Some("3.7.0")),
];

/// The deprecated rules' `RuleState.deprecated` `since` versions (the five
/// registrations in `pkg/linter/lib/src/rules/<name>.dart` — three at
/// `Version(3, 11, 0)`, two at `Version(3, 13, 0)` — none with a
/// `replacedBy`, so only the plain `deprecated_lint` row applies, never
/// `deprecated_lint_with_replacement`). The names are implemented in
/// dartq-lints too, and the row still fires: the official face keys on the
/// resolved rule's `state`, not on the implementation (the rules keep
/// running despite deprecation upstream), so the options face renders the
/// registry texts by name (analyze.rs `options_registry_rows`).
pub(crate) const DEPRECATED_SINCE: &[(&str, &str)] = &[
    ("avoid_null_checks_in_equality_operators", "3.11.0"),
    ("avoid_private_typedef_functions", "3.13.0"),
    ("one_member_abstracts", "3.13.0"),
    ("prefer_final_parameters", "3.11.0"),
    ("use_if_null_to_convert_nulls_to_bools", "3.11.0"),
];

/// `state.since.toString()` — the messages' `{1}` argument: the version
/// text, or `null` for the unversioned state.
pub(crate) fn since_arg(since: Option<&str>) -> &str {
    since.unwrap_or("null")
}

/// The removed state of `name`: its `since` version when the rule is
/// removed — the inner `None` covers an unversioned removal, and a call-side
/// flatten renders the same `null`-since face for both). Case-insensitive —
/// the official registry lookup lowercases every name
/// (`Registry.ruleRegistry`, registry.dart:42,53).
#[expect(clippy::option_option)] // the inner None is the table's unversioned-removal marker; flattening would skip the row on the options face
pub(crate) fn removed_since(name: &str) -> Option<Option<&'static str>> {
    REMOVED_SINCE
        .iter()
        .find(|(rule, _)| rule.eq_ignore_ascii_case(name))
        .map(|(_, since)| *since)
}

/// The deprecated state of `name`: its `since` version when the rule is
/// deprecated. Case-insensitive like [`removed_since`].
pub(crate) fn deprecated_since(name: &str) -> Option<&'static str> {
    DEPRECATED_SINCE
        .iter()
        .find(|(rule, _)| rule.eq_ignore_ascii_case(name))
        .map(|(_, since)| *since)
}

/// Whether the removal/deprecation `since` predates or meets the analyzing
/// package's SDK-constraint lower bound (`_beforeCurrentConstraint`,
/// parse_model.dart:1197-1205: a null since applies to all SDKs; otherwise
/// `since <= min` over the constraint's `VersionRange` — the official switch
/// matches `VersionRange` only, so an exact-version or `any` constraint
/// carries no min and every versioned since fails).
pub(crate) fn before_current(since: Option<&str>, min: Option<(u64, u64, u64)>) -> bool {
    let Some(since) = since else {
        return true;
    };
    let Some((major, minor, patch)) = min else {
        return false;
    };
    parse_since(since).is_some_and(|(s, i, n)| (s, i, n) <= (major, minor, patch))
}

/// Parses a pinned `major.minor.patch` `since` text for the bound compare.
fn parse_since(text: &str) -> Option<(u64, u64, u64)> {
    let (major, rest) = text.split_once('.')?;
    let (minor, patch) = rest.split_once('.')?;
    Some((
        major.parse().ok()?,
        minor.parse().ok()?,
        patch.parse().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lint_names::is_known;
    use dartq_lints::{Group, RULES, rule_by_name};

    /// The two removed-set authorities agree: every `Group::Removed` spec
    /// has a since row and vice versa — a registry wave that adds or drops
    /// a removed rule without updating the table is caught here.
    #[test]
    fn removed_table_matches_the_registry_group() {
        let registry: Vec<_> = RULES
            .iter()
            .filter(|spec| spec.group == Group::Removed)
            .map(|spec| spec.name)
            .collect();
        let table: Vec<_> = REMOVED_SINCE.iter().map(|(name, _)| *name).collect();
        assert_eq!(registry, table);
    }

    /// Deprecated rules register in the SDK's registry (the options faces
    /// accept them — no `undefined_lint`) and are implemented in
    /// dartq-lints: the official `deprecated_lint` row keys on the
    /// resolved rule's `state`, not on the implementation
    /// (parse_model.dart:1288-1296 — the rules keep running despite
    /// deprecation), so an implemented deprecated rule must keep its row.
    /// The row's firing face pins end to end in analyze.rs
    /// (`run_overlaid_reports_deprecated_lint_rows`).
    #[test]
    fn deprecated_names_register_and_stay_implemented() {
        for (name, _) in DEPRECATED_SINCE {
            assert!(is_known(name), "{name} registers in the SDK");
            let id = rule_by_name(name).unwrap_or_else(|| {
                panic!(
                    "{name} lost its implementation — the deprecated_lint face must keep firing for it"
                )
            });
            assert_ne!(
                RULES[id.0 as usize].group,
                Group::Removed,
                "{name} joined the removed set — DEPRECATED_SINCE is stale"
            );
        }
    }

    /// The `_beforeCurrentConstraint` arithmetic: a null since applies to
    /// every SDK, a versioned since needs a `VersionRange`-shaped min at or
    /// after it, and a missing min fails every versioned since.
    #[test]
    fn before_current_gates_on_the_constraint_min() {
        assert!(before_current(None, None));
        assert!(before_current(None, Some((2, 0, 0))));
        assert!(before_current(Some("3.0.0"), Some((3, 0, 0))));
        assert!(before_current(Some("3.0.0"), Some((3, 5, 0))));
        assert!(!before_current(Some("3.7.0"), Some((3, 5, 0))));
        assert!(!before_current(Some("3.0.0"), None));
    }
}
