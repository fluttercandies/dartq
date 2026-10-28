//! dartq's diagnostic data model and oracle-exact output writers.
//!
//! This crate is the single source of truth for the shape and serialization of
//! dartq diagnostics. Its output must match `dart analyze`'s
//! machine formats field-for-field, so every layout decision here is pinned to
//! the measured oracle facts (the oracle machine-format contract).
//!
//! - [`Severity`] and [`DiagnosticType`]: the severity and type layering
//!   measured from `dart analyze --format=machine` (`TYPE` value set:
//!   `SYNTACTIC_ERROR` / `COMPILE_TIME_ERROR` / `STATIC_WARNING` / `LINT`,
//!   plus dartq's own `TOOL` for the `dartq/` namespace).
//! - [`CodeInfo`] and [`CodeRegistry`]: stable official code names
//!   and
//!   dartq-own `dartq/`-prefixed codes. The official table is generated at
//!   build time from `data/syntax-codes.tsv` (dart 3.13.1 oracle lock).
//! - [`Diagnostic`]: one diagnostic located by a [`FileId`] plus a
//!   UTF-16-unit [`SourceRange`].
//! - [`write_machine`] / [`write_json`]: byte-exact oracle output writers.
//!   Both consume an [`AnalysisOutput`], pairing the diagnostics with a
//!   [`FileLayoutProvider`] the caller implements on top of the engine's line
//!   index; this crate never depends on the scanner.
//!
//! [`FileId`] and [`SourceRange`] are re-exported from `dartq-source`, the
//! single source of truth for file identity and the UTF-16 position
//! convention.

mod analyzer_docs;
mod code;
mod lint_docs;
mod model;
mod writer;

// Everything in `code` is public API: the per-code constants span the
// build-generated official table (data/syntax-codes.tsv) plus the
// M0-measured and dartq-own namespaces.
pub use code::*;
pub use model::{
    AnalysisOutput, ContextMessage, Diagnostic, DiagnosticType, FileId, FileLayoutProvider,
    Severity, SharedStr, SourceRange,
};
pub use writer::{write_json, write_machine};

/// The URL prefix of an official code's documentation page, decided by
/// name from the captured published tables (`analyzer_docs` /
/// `lint_docs`): the diagnostics page for a published analyzer or lint
/// code, the `lints` page for an unpublished first-party lint, `None`
/// for an unpublished analyzer/parser code and a code the first-party
/// tables do not carry (a plugin lint — the oracle's URL is null).
pub(crate) fn doc_prefix(name: &str, is_lint: bool) -> Option<&'static str> {
    match is_lint {
        true => lint_docs::lint_doc_prefix(name),
        false => analyzer_docs::published(name).then_some("https://dart.dev/diagnostics/"),
    }
}

/// The documentation URL an official code publishes — the one decision
/// every `documentation`/`doc` field serves (the json writer's rows, the
/// explain faces), so the faces cannot disagree. Decided by name from
/// the captured published tables, never from a persisted per-row flag
/// (which would serve stale URLs from pre-capture stores).
#[must_use]
pub fn documentation_url(code_name: &str, is_lint: bool) -> Option<String> {
    Some(format!("{}{code_name}", doc_prefix(code_name, is_lint)?))
}

#[cfg(test)]
mod tests {
    use super::documentation_url;

    /// The published-table decisions the faces share: a published
    /// analyzer code and a published first-party lint take the
    /// diagnostics page, an unpublished first-party lint takes the lint
    /// page, and an unpublished analyzer code, a plugin code, and an
    /// unknown name take none — the json face's own pins re-checked
    /// through the one accessor both share.
    #[test]
    fn documentation_url_decides_by_the_published_tables() {
        assert_eq!(
            documentation_url("unused_import", false).as_deref(),
            Some("https://dart.dev/diagnostics/unused_import")
        );
        assert_eq!(
            documentation_url("avoid_print", true).as_deref(),
            Some("https://dart.dev/diagnostics/avoid_print")
        );
        assert_eq!(
            documentation_url("always_require_non_null_named_parameters", true).as_deref(),
            Some("https://dart.dev/lints/always_require_non_null_named_parameters")
        );
        assert_eq!(documentation_url("expected_token", false), None);
        assert_eq!(documentation_url("plug_lint", true), None);
        assert_eq!(documentation_url("never_heard_of_it", false), None);
    }
}
