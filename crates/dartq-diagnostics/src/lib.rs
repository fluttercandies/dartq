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
