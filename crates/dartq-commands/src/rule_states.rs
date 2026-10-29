//! The rule-state data the option faces render (`removed_lint` /
//! `removed_lint_use` / `deprecated_lint`): the since versions of the
//! removed and deprecated rules, read through the generated upstream
//! metadata table (#2676 — [`dartq_lints::rule_meta`], derived from
//! `pkg/linter` of the pinned SDK tree: the `RemovedAnalysisRule` /
//! `RuleState.deprecated` registrations, `--check`-pinned by CI).
//! Membership stays the lint registry's ([`dartq_lints::Group::Removed`]
//! for the removed set; the table only carries the version each state
//! renders) — version-pinned data like the engine's
//! `KNOWN_EXPERIMENTS`.

/// `state.since.toString()` — the messages' `{1}` argument: the version
/// text, or `null` for the unversioned state.
pub(crate) fn since_arg(since: Option<&str>) -> &str {
    since.unwrap_or("null")
}

/// The removed state of `name`: its `since` version when the rule is
/// removed — the inner `None` covers an unversioned removal, and a
/// call-side flatten renders the same `null`-since face for both.
/// Case-insensitive — the official registry lookup lowercases every name
/// (`Registry.ruleRegistry`, registry.dart:42,53); the generated table is
/// the machine-name spelling, so the lookup lowercases first.
#[expect(clippy::option_option)] // the inner None is the table's unversioned-removal marker; flattening would skip the row on the options face
pub(crate) fn removed_since(name: &str) -> Option<Option<&'static str>> {
    let meta = dartq_lints::rule_meta(&name.to_ascii_lowercase())?;
    match meta.state {
        dartq_lints::MetaState::Removed(since) => Some(since),
        _ => None,
    }
}

/// The deprecated state of `name`: its `since` version when the rule is
/// deprecated. Case-insensitive like [`removed_since`]. The pinned tree
/// carries no `replacedBy` (the generator fails the regen on one), so
/// only the plain `deprecated_lint` row applies, never
/// `deprecated_lint_with_replacement`.
pub(crate) fn deprecated_since(name: &str) -> Option<&'static str> {
    let meta = dartq_lints::rule_meta(&name.to_ascii_lowercase())?;
    match meta.state {
        dartq_lints::MetaState::Deprecated(since) => since,
        _ => None,
    }
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
    /// carries a removed state in the generated table and vice versa — a
    /// registry wave that adds or drops a removed rule without the
    /// regenerated table is caught here.
    #[test]
    fn removed_table_matches_the_registry_group() {
        let registry: Vec<_> = RULES
            .iter()
            .filter(|spec| spec.group == Group::Removed)
            .map(|spec| spec.name)
            .collect();
        let table: Vec<_> = RULES
            .iter()
            .map(|spec| spec.name)
            .filter(|name| removed_since(name).is_some())
            .collect();
        assert_eq!(registry, table);
        // The pinned since values (the registration texts): one per
        // removed rule, `avoid_unstable_final_fields` the unversioned one.
        assert_eq!(
            removed_since("avoid_as").flatten(),
            Some("2.12.0"),
            "the registration-side since, not the yaml state version"
        );
        assert_eq!(removed_since("avoid_unstable_final_fields"), Some(None));
        assert_eq!(removed_since("unsafe_html").flatten(), Some("3.7.0"));
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
        for spec in RULES.iter() {
            if let Some(since) = deprecated_since(spec.name) {
                assert!(is_known(spec.name), "{} registers in the SDK", spec.name);
                assert_ne!(
                    spec.group,
                    Group::Removed,
                    "{} joined the removed set while deprecated",
                    spec.name
                );
                assert!(
                    since.chars().next().is_some_and(|c| c.is_ascii_digit()),
                    "{} carries a versioned since",
                    spec.name
                );
            }
        }
        // The 3.13 wave the hand-copied table missed (#2676's find): the
        // drift fix that lands with the derivation.
        assert_eq!(
            deprecated_since("unnecessary_await_in_return"),
            Some("3.13.0")
        );
        let _ = rule_by_name; // keep the import honest for future pins
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
