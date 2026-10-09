//! Package identity model (PRD §5.1, §8.1) — the typed identity surface the
//! resolution layer consumes, composed on top of `dartiq-pub`'s artifacts.
//!
//! `dartiq-pub` owns the pub side of the model: parsing
//! `package_config.json`/`pubspec.lock` and assembling the
//! [`dartiq_pub::ResolvedPackageGraph`], whose [`dartiq_pub::PackageInstance`]
//! is the PRD §8.1 instance tuple `(source_kind, source_ref, version,
//! content_hash)`. This crate adds only what the identity face still lacks
//! and never redefines a pub type:
//!
//! - [`PackageName`] — the grammar-validated canonical name
//!   (`identifier(.identifier)*`, `pubspec_parse.dart:17-19`), usable as a
//!   map key.
//! - [`PackageId`] — PRD §5.1 `PackageId = (name, PackageInstanceId)`, the
//!   cross-context semantic isolation unit; its instance components are
//!   `dartiq-pub` types verbatim.
//! - [`PackageIdentity`] — one package's resolution-facing identity: root
//!   directory, `lib/` directory, language version, and the coarse
//!   [`SourceCategory`] (path / pub-cache / SDK) that tells consumers which
//!   packages are workspace source and which are immutable cache content.
//! - [`PackageTable`] — the name-keyed view over one resolution, built by
//!   composing [`dartiq_pub::PackageConfig`] with
//!   [`dartiq_pub::ResolvedPackageGraph`]; the build rejects artifact pairs
//!   that disagree instead of silently dropping packages.
//!
//! Format facts and pub behavior anchors live in `docs/facts/pub-formats.md`
//! and `.third/pub/lib/src/package.dart`; do not lint-prose them here.
#![allow(clippy::doc_markdown)]

mod identity;
mod name;
mod table;

pub use identity::{PackageId, PackageIdentity, SourceCategory};
pub use name::PackageName;
pub use table::PackageTable;

// The pub identity vocabulary, so consumers of this crate can name instance
// types without a second dependency edge. These are re-exports, not
// redefinitions — the graph module in `dartiq-pub` remains their home.
pub use dartiq_pub::{PackageInstance, PackageInstanceId, SourceKind, SourceRef};

/// Errors from the package identity layer.
///
/// Two shapes: a name outside pub's package-name grammar, and a
/// config/graph pair that disagrees. Underlying artifact errors surface as
/// [`PackageError::Pub`] unchanged.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PackageError {
    /// Underlying pub artifact handling failed (rootUri resolution, YAML,
    /// JSON). Forwarded verbatim from `dartiq-pub`.
    #[error(transparent)]
    Pub(#[from] dartiq_pub::PubError),
    /// A package name does not match pub's grammar.
    #[error(
        "invalid package name `{name}`: expected identifier(.identifier)* \
         with identifier [a-zA-Z_][a-zA-Z0-9_]*"
    )]
    InvalidName { name: String },
    /// A config entry has no instance in the resolved graph.
    #[error("package `{name}` is missing from the resolved graph")]
    MissingFromGraph { name: String },
    /// A graph root or workspace member has no entry in the package config.
    #[error("package `{name}` is missing from the package config")]
    MissingFromConfig { name: String },
    /// The config lists the same package name twice.
    #[error("duplicate package_config entry for `{name}`")]
    DuplicateEntry { name: String },
}
