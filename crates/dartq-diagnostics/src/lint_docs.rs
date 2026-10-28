//! The first-party lint documentation table: which lint codes carry a
//! published docs page, the one fact the JSON face's `documentation`
//! field keys on for lint rows (#2676: derived — the rows come from the
//! generated table `tools/upstream-gen`'s `linter-meta-gen` emits from
//! `pkg/linter/messages.yaml` of the pinned SDK tree, one row per
//! first-party linter rule; `--check` byte-compares it in CI).
//!
//! Upstream truth (`LinterLintCode.url`, upstream Dart SDK source,
//! `pkg/linter/lib/src/lint_codes.dart`): every first-party lint code has
//! a URL — `https://dart.dev/diagnostics/<name>` when the code is
//! `hasPublishedDocs: true`, else `https://dart.dev/lints/<name>` — unlike
//! analyzer codes, whose `url` is null unless published
//! (`_fe_analyzer_shared` `errors.dart`). A family's flag folds
//! any-variant (upstream mixes flags within one shared name —
//! `always_specify_types`); the universe is every first-party linter rule
//! of the pinned tree, the same universe the options faces validate
//! against: a lint-typed row whose code is absent here is not a
//! first-party linter code — plugin codes take no `documentation` (the
//! analyzer's own `LintCode.url` is null) — and dartq-own codes never
//! spell `DiagnosticType::Lint`.
//!
//! Version-pinned data like `crate::code`'s generated table: sorted by
//! name, membership and flag read by one binary search.

/// One lint code's documentation fact: the rule name and whether the
/// upstream code is `hasPublishedDocs: true`. The included file is the
/// generated row slice (single expression, the `include!` shape).
static LINT_DOCS: &[(&str, bool)] = include!("generated/lint_docs_table.rs");

/// The documentation URL prefix a first-party lint code carries, `None`
/// when `name` is not a registered first-party linter code (plugin codes:
/// the oracle's analyzer-side `LintCode.url` is null, so the JSON face
/// omits `documentation` for them).
#[must_use]
pub(crate) fn lint_doc_prefix(name: &str) -> Option<&'static str> {
    LINT_DOCS
        .binary_search_by(|(entry, _)| (*entry).cmp(name))
        .ok()
        .map(|index| {
            if LINT_DOCS[index].1 {
                "https://dart.dev/diagnostics/"
            } else {
                "https://dart.dev/lints/"
            }
        })
}

#[cfg(test)]
mod tests {
    use super::{LINT_DOCS, lint_doc_prefix};

    /// The capture invariants: sorted (binary search's contract), the
    /// universe size (the 3.13.7 pin's yaml carries 315 consts folding to
    /// these names), and the pinned prefix decisions — a published core
    /// lint and `unnecessary_ignore` take the diagnostics page, an
    /// unpublished code takes the lint page, an unknown/plugin code takes
    /// none. The three rules dartq keeps mounted beyond the pin's registry
    /// (`future_sync_value`, `unnecessary_this_alias`,
    /// `analyzer_to_be_deprecated_use` — newer-upstream-only, see the
    /// lints registry's `META_EXEMPT`) have no rows here: the oracle of
    /// the pin never emits them, so their rows take no documentation URL.
    #[test]
    fn lint_doc_prefix_decides_by_table_membership() {
        assert!(
            LINT_DOCS.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "sorted: binary search's contract"
        );
        assert_eq!(LINT_DOCS.len(), 275);
        assert_eq!(
            lint_doc_prefix("avoid_empty_else"),
            Some("https://dart.dev/diagnostics/")
        );
        assert_eq!(
            lint_doc_prefix("unnecessary_ignore"),
            Some("https://dart.dev/diagnostics/")
        );
        assert_eq!(
            lint_doc_prefix("future_sync_value"),
            None,
            "mounted beyond the 3.13.7 pin's registry — no upstream URL face"
        );
        // `always_require_non_null_named_parameters` is a removed rule with
        // no published docs page upstream.
        assert_eq!(
            lint_doc_prefix("always_require_non_null_named_parameters"),
            Some("https://dart.dev/lints/")
        );
        assert_eq!(lint_doc_prefix("plug_lint"), None, "a plugin code");
        assert_eq!(lint_doc_prefix("never_heard_of_it"), None);
    }
}
