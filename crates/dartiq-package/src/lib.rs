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
/// Two shapes: a name outside its layer's grammar (pubspec-produced names
/// follow pub's grammar, `package_config.json` entries only the reader's
/// looser charset), and a config/graph pair that disagrees — including
/// package roots that overlap or a `packageUri` that escapes its root.
/// Underlying artifact errors surface as [`PackageError::Pub`] unchanged.
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
    /// A `package_config.json` entry name is outside even the reader's
    /// loose grammar (`package_config-3.0.0` `util.dart:12-36`).
    #[error(
        "invalid package_config name `{name}`: expected characters \
         `!$&'()*+,-.0-9;=@A-Z_a-z~` with at least one non-dot character"
    )]
    InvalidConfigName { name: String },
    /// A config entry has no instance in the resolved graph.
    #[error("package `{name}` is missing from the resolved graph")]
    MissingFromGraph { name: String },
    /// A graph root or workspace member has no entry in the package config.
    #[error("package `{name}` is missing from the package config")]
    MissingFromConfig { name: String },
    /// The config lists the same package name twice.
    #[error("duplicate package_config entry for `{name}`")]
    DuplicateEntry { name: String },
    /// A package's `packageUri` directory resolves outside the package's
    /// own `rootUri` directory (oracle: "The package URI root is not below
    /// the package root", `package_config-3.0.0` `package_config.dart:838`).
    #[error(
        "package `{name}` has packageUri directory `{lib_dir}` outside its \
         root `{root}`"
    )]
    LibDirOutsideRoot {
        name: String,
        lib_dir: String,
        root: String,
    },
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
    /// The engine passed a relative `config_dir`; the assembly contract is
    /// the absolute `.dart_tool/` directory, and a relative one would
    /// silently produce wrong roots.
    #[error("config_dir must be absolute, got `{path}`")]
    RelativeConfigDir { path: String },
}
