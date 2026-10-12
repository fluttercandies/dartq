//! Dart SDK metadata: loading, identity, and querying of the `dart:` library
//! surface of a Dart SDK (PRD §7).
//!
//! The single source of truth is the SDK's `lib/libraries.json`. Unlike the
//! analyzer's `FolderBasedDartSdk` — which parses a Dart-source-shaped
//! `libraries.dart` with an embedded Dart parser — dartq consumes the
//! machine-readable `libraries.json`: same public library names, plus the
//! target segments, patch paths, and conditional-import gates the
//! Dart-shaped table lacks (rationale in the SDK metadata capture §2).
//!
//! Loading is eager over the small metadata files and lazy over `dart:`
//! source bodies:
//!
//! - [`SdkSnapshot::load`] reads `version`, `revision`, `libraries.json`
//!   (with per-target `include` expansion), and
//!   `lib/_internal/allowed_experiments.json`. A flutter root is accepted
//!   for signature stability but rejected until embedder support lands
//!   (M2); `None` is fully self-sufficient.
//! - [`SdkSnapshot::id`] is the blake3 content hash of the *parsed*
//!   metadata in canonical form, so two SDKs whose metadata parses the same
//!   share one id regardless of JSON layout; raw-file hashes live only in
//!   [`SdkIdentity`] for change detection.
//! - Queries mirror the compiler consumption surface, always scoped to a
//!   target segment: [`SdkSnapshot::uri_to_path`] (including
//!   `dart:lib/part.dart` resolution, with explicit errors instead of
//!   silent `None`s), its structural twin
//!   [`SdkSnapshot::library_to_path`] over the split `(library, part)`
//!   pair, [`SdkSnapshot::target`], and
//!   [`SdkSnapshot::library_support_value`] for the
//!   `dart.library.<name>` environment.
//! - [`AllowedExperiments`] answers the experiment whitelist per SDK
//!   library / package and maps it onto
//!   [`FeatureSet`](dartq_language::FeatureSet) at a given language
//!   version.
//!
//! Error discipline: where the analyzer silently degrades — a missing
//! library map yields empty analysis and a malformed
//! `allowed_experiments.json` disables every experiment — dartq hard-errors
//! with [`SdkError`], so a crippled SDK can never masquerade as a valid
//! analysis baseline (the SDK loader contract §6).

mod identity;
mod loader;
mod metadata;
mod query;

pub use loader::SdkError;
pub use metadata::{
    AllowedExperiments, IMPLICITLY_IMPORTED_LIBRARIES, Importability, SdkIdentity, SdkLibraries,
    SdkLibrary, SdkSnapshot, TargetLibraries, UnknownExperiments,
};
pub use query::SdkUriError;
