//! Package identity model (PRD §5.1, §8.1) — the typed identity surface the
//! resolution layer consumes, composed on top of `dartq-pub`'s artifacts.
//!
//! `dartq-pub` owns the pub side of the model: parsing
//! `package_config.json`/`pubspec.lock` and assembling the
//! [`dartq_pub::ResolvedPackageGraph`], whose [`dartq_pub::PackageInstance`]
//! is the PRD §8.1 instance tuple `(source_kind, source_ref, version,
//! content_hash)`. This crate adds only what the identity face still lacks
//! and never redefines a pub type:
//!
//! - [`PackageName`] — the grammar-validated canonical name
//!   (`identifier(.identifier)*`, `pubspec_parse.dart:17-19`), usable as a
//!   map key.
//! - [`PackageId`] — PRD §5.1 `PackageId = (name, PackageInstanceId)`, the
//!   cross-context semantic isolation unit; its instance components are
//!   `dartq-pub` types verbatim.
//! - [`PackageIdentity`] — one package's resolution-facing identity: root
//!   directory, `lib/` directory, language version, and the coarse
//!   [`SourceCategory`] (path / pub-cache / SDK) that tells consumers which
//!   packages are workspace source and which are immutable cache content.
//! - [`PackageTable`] — the name-keyed view over one resolution, built by
//!   composing [`dartq_pub::PackageConfig`] with
//!   [`dartq_pub::ResolvedPackageGraph`]; the build rejects artifact pairs
//!   that disagree instead of silently dropping packages.
//!
//! Format facts and pub behavior anchors live in the pub formats contract
//! and `upstream pub source, lib/src/package.dart`; do not lint-prose them here.
#![allow(clippy::doc_markdown)]

mod identity;
mod name;
mod table;

pub use identity::{PackageId, PackageIdentity, SourceCategory};
pub use name::PackageName;
pub use table::PackageTable;

// The pub identity vocabulary, so consumers of this crate can name instance
// types without a second dependency edge. These are re-exports, not
// redefinitions — the graph module in `dartq-pub` remains their home.
pub use dartq_pub::{PackageInstance, PackageInstanceId, SourceKind, SourceRef};

/// Errors from the package identity layer.
///
/// Two shapes: a name outside pub's pubspec grammar, and a config/graph
/// pair that disagrees — including package roots that overlap. The config
/// entry policies (reader name grammar, duplicate names, `packageUri`
/// subtree validation, absolute `config_dir`) are dartq-pub's shared
/// [`dartq_pub::PackageConfig::resolve`] face and surface as
/// [`PackageError::Pub`]; this layer adds only graph composition and
/// attribution errors.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PackageError {
    /// Underlying pub artifact handling failed (rootUri resolution, YAML,
    /// JSON, or a `PackageConfig::resolve` policy violation — duplicate
    /// names, reader-grammar names, a `packageUri` outside its root, a
    /// relative config dir). Forwarded verbatim from `dartq-pub`.
    #[error(transparent)]
    Pub(#[from] dartq_pub::PubError),
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
    /// Two config entries resolve to the same normalized package root —
    /// file→package attribution could not pick an owner (analyzer analog:
    /// `package_map_resolver.dart:33-52` scans every root per file, where
    /// duplicates make the deepest match ambiguous).
    #[error(
        "packages `{first}` and `{name}` resolve to the same package root \
         `{root}`"
    )]
    DuplicateRoot {
        first: String,
        name: String,
        root: String,
    },
}
