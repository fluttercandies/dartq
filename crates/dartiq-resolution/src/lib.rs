//! URI, import, export, part, and conditional import resolution with
//! namespace computation.
//!
//! This crate currently carries the URI layer (M1 ticket #65):
//!
//! - [`uri`] — directive URI parsing/validation under Dart `Uri.parse`
//!   rules, relative composition against the container's canonical URI,
//!   scheme dispatch (`dart:`/`package:`/`file:`), interning to
//!   [`dartiq_source::FileId`], and the [`ResolvedTarget`] projection.
//! - [`conditional`] — the analyzer-oracle conditional-URI selection
//!   (declared-variables exact string match, first match wins).
//! - [`codes`] — the frozen §4.2 resolution diagnostic code table.
//!
//! Namespace building, the library/part graph, and the export fixpoint are
//! later tickets (M1 #117/#119).

pub mod codes;
pub mod conditional;
pub mod uri;

pub use codes::{ResolutionCode, ResolutionCodeInfo};
pub use conditional::{ConditionalEnv, Configuration, SelectedUri, select_configured_uri};
pub use uri::{
    PackageTable, RawUri, ResolvedTarget, ResolvedUri, UnresolvedKind, UriResolution, UriResolver,
    parse_directive_uri,
};
