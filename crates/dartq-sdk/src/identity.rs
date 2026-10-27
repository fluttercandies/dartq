//! Canonical snapshot identity: the `SdkId` cache key.
//!
//! The id is the blake3 hash (via
//! [`dartq_source::content_hash`]) of the *parsed* metadata serialized in
//! canonical form — never of the raw files. Equal parses share an id across
//! differing JSON layouts (3.9.0 and 3.10.0 ship a byte-identical
//! `libraries.json`, and any regenerated-but-equal file must not fork the
//! cache), while every identity-relevant value — dart version and revision,
//! language version, the expanded library tables, and the experiment
//! whitelist — participates.

use serde::Serialize;

use dartq_language::LanguageVersion;
use dartq_source::{ContentId, content_hash};

use crate::metadata::{AllowedExperiments, SdkIdentity, SdkLibraries, TargetLibraries};

/// Computes the snapshot id over the parsed metadata parts.
pub(crate) fn snapshot_id(
    dart: &SdkIdentity,
    language: LanguageVersion,
    sdk_libraries: &SdkLibraries,
    platform_metadata: &AllowedExperiments,
    embedder: Option<&TargetLibraries>,
) -> ContentId {
    let fingerprint = SnapshotFingerprint {
        dart_version: &dart.version,
        dart_revision: dart.revision.as_deref(),
        language_major: language.major,
        language_minor: language.minor,
        targets: &sdk_libraries.targets,
        allowed_experiments: platform_metadata,
        embedder,
    };
    let bytes = serde_json::to_vec(&fingerprint)
        .expect("canonical fingerprint holds only maps and strings; serialization cannot fail");
    content_hash(&bytes)
}

/// The canonical metadata view the id hashes. Serde emits struct fields in
/// declaration order and `BTreeMap` keys sorted, so the byte form is
/// deterministic; raw-file hashes are deliberately excluded (they live in
/// [`SdkIdentity`] for change detection only). The embedder table is
/// serialized only while present, so plain snapshots keep the exact
/// pre-embedder fingerprint bytes (no id churn for non-Flutter workspaces)
/// and an embedder-backed load forks the id through its parsed table.
#[derive(Serialize)]
struct SnapshotFingerprint<'a> {
    dart_version: &'a str,
    dart_revision: Option<&'a str>,
    language_major: u32,
    language_minor: u32,
    targets: &'a std::collections::BTreeMap<String, TargetLibraries>,
    allowed_experiments: &'a AllowedExperiments,
    #[serde(skip_serializing_if = "Option::is_none")]
    embedder: Option<&'a TargetLibraries>,
}
