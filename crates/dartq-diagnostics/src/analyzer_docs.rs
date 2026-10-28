//! The published analyzer/parser documentation table: which official
//! non-lint codes carry a `https://dart.dev/diagnostics/<code>` page,
//! the one fact the JSON face's `documentation` field keys on for
//! analyzer rows.
//!
//! Upstream truth (`DiagnosticCode.url`, upstream Dart SDK source,
//! `pkg/_fe_analyzer_shared/lib/src/base/errors.dart`): the URL is
//! `https://dart.dev/diagnostics/<lowercase name>` when the code's
//! registry entry is `hasPublishedDocs: true`, `null` otherwise —
//! unlike lint codes, every one of which carries a page
//! (`crate::lint_docs`).
//!
//! The published set is the `hasPublishedDocs` column of the generated
//! upstream registry table (`crate::code`'s `ANALYZER_PUBLISHED_DOCS`,
//! build-time output of `data/analyzer-codes.tsv`; the TSV is generated
//! from the analyzer's consolidated registry by `tools/upstream-gen`'s
//! `analyzer-codes-gen`). The registry has no LINT rows (lints never
//! enter it) and no name with mixed published/unpublished variants —
//! the generator verifies the any-published fold on every run.
//!
//! The JSON face decides by code name here, never by the row's
//! `has_published_docs` flag: the flag is persisted per row (the
//! content store's `code_published`) and would serve stale URLs from
//! pre-capture stores, and emitters build `CodeInfo` rows
//! independently of this crate — the name-keyed table is stable
//! across warm and cold rows alike (same posture as `lint_docs`).
//! `dartq/`-namespace codes never spell a table name (`snake_case`
//! without `/`), so they take no URL by construction.

use crate::code::ANALYZER_PUBLISHED_DOCS;

/// Whether `name` is an official analyzer/parser code with a
/// published docs page (the JSON `documentation` URL's gate for
/// non-lint rows).
#[must_use]
pub(crate) fn published(name: &str) -> bool {
    ANALYZER_PUBLISHED_DOCS.binary_search(&name).is_ok()
}

#[cfg(test)]
mod tests {
    use super::{ANALYZER_PUBLISHED_DOCS, published};

    /// The generated table's contract: sorted and unique (binary
    /// search's premise) and every name lowercase snake_case (the URL
    /// is spelled from the name verbatim — a casing drift would mint a
    /// dead URL and break the `dartq/` namespace separation argument in
    /// the module docs).
    #[test]
    fn table_is_sorted_unique_snake_case() {
        assert!(
            ANALYZER_PUBLISHED_DOCS
                .windows(2)
                .all(|pair| pair[0] < pair[1]),
            "sorted and unique: binary search's contract"
        );
        for name in ANALYZER_PUBLISHED_DOCS {
            assert!(
                name.chars().enumerate().all(|(i, c)| c.is_ascii_lowercase()
                    || c.is_ascii_digit() && i > 0
                    || c == '_' && i > 0),
                "{name}: not lowercase snake_case"
            );
        }
    }

    /// Every published name is a row of the generated registry (the
    /// column the list was folded from) — the two generated artifacts
    /// cannot drift apart.
    #[test]
    fn published_names_exist_in_the_official_table() {
        for name in ANALYZER_PUBLISHED_DOCS {
            assert!(
                crate::code::OFFICIAL_CODES
                    .iter()
                    .any(|code| code.name == *name),
                "{name}: published but absent from OFFICIAL_CODES"
            );
        }
    }

    /// The pinned membership decisions — the ticket's regression row
    /// and the four edges the registry tests in `code.rs` pin per-row:
    /// published codes answer `true`, the unpublished parser pair and
    /// a dartq-own namespace name answer `false`.
    #[test]
    fn published_decides_by_table_membership() {
        for name in [
            "built_in_identifier_in_declaration",
            "uri_does_not_exist",
            "deprecated_member_use",
            "expected_token",
            "illegal_language_version_override",
        ] {
            // `expected_token` and `illegal_language_version_override`
            // are the two negative pins; the rest published.
            let expect = !matches!(name, "expected_token" | "illegal_language_version_override");
            assert_eq!(published(name), expect, "{name}");
        }
        assert!(!published("dartq/internal_error"));
        assert!(!published("never_heard_of_it"));
    }
}
