//! Per-package identity types (PRD §5.1, §8.1).

use std::path::PathBuf;

use dartiq_pub::{PackageInstance, SourceKind, SourceRef, Version};
use dartiq_source::ContentId;

use crate::PackageName;

/// Coarse layout category of a resolved package — what the resolution and
/// filesystem layers need to know about where the bytes live.
///
/// The precise source identity stays in [`SourceRef`]; this is the derived
/// classification: hosted archives and git checkouts materialize under the
/// pub cache, while path packages are workspace source. Note that
/// freshness's mtime-skip keys on cache ownership — the file living within
/// the config's `pubCache` root (entrypoint.dart:1263-1265) — not on this
/// category: the two coincide for pub-faithful resolutions and can diverge
/// when the config lacks a decodable `pubCache` property or uses a custom
/// cache layout (facts §6.5). `pubspec.lock`-level [`SourceKind`]
/// is not reused here because the two taxonomies answer different questions
/// (provenance vs. layout) and the shapes differ (three vs. four variants).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SourceCategory {
    /// Local directory the user edits — workspace members, the root, and
    /// path dependencies.
    Path,
    /// Content materialized by pub into its cache — hosted archives and git
    /// checkouts.
    PubCache,
    /// SDK-provided packages.
    Sdk,
}

impl SourceCategory {
    /// Classifies a resolved source reference.
    #[must_use]
    pub fn of(source_ref: &SourceRef) -> SourceCategory {
        match source_ref {
            SourceRef::Hosted { .. } | SourceRef::Git { .. } => SourceCategory::PubCache,
            SourceRef::Path { .. } => SourceCategory::Path,
            SourceRef::Sdk { .. } => SourceCategory::Sdk,
        }
    }
}

/// PRD §5.1: `PackageId = (name, PackageInstanceId)`, where the instance
/// tuple is `(source_kind, source_ref, version, content_hash)`.
///
/// This struct is that tuple flattened into direct fields — not
/// [`crate::PackageInstanceId`], the re-exported graph-side instance
/// handle (a newtype over `String`) that happens to share its name.
///
/// The semantic isolation unit: the same name from different sources or
/// checkouts is a different package, while identical instances share the
/// source layer across contexts. Every component is a `dartiq-pub` /
/// `dartiq-source` type; nothing is redefined. Hashable so it works as a
/// map key for cross-context sharing tables.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PackageId {
    pub name: PackageName,
    pub source: SourceKind,
    pub source_ref: SourceRef,
    /// `None` for packages without a pubspec version (e.g. the implicit SDK
    /// instance).
    pub version: Option<Version>,
    /// Per-package content Merkle — populated by the M2 incremental layer.
    pub content_hash: Option<ContentId>,
}

impl PackageId {
    /// Projects a resolved instance onto the identity pair, validating its
    /// name against pub's grammar.
    ///
    /// # Errors
    ///
    /// Returns [`PackageError::InvalidName`] if the instance name is not a
    /// valid package name (the implicit `"sdk:dart"` instance, for example).
    pub fn from_instance(instance: &PackageInstance) -> Result<PackageId, crate::PackageError> {
        Ok(PackageId {
            name: PackageName::new(&instance.name)?,
            source: instance.source,
            source_ref: instance.source_ref.clone(),
            version: instance.version.clone(),
            content_hash: instance.content_hash,
        })
    }
}

/// One package's resolution-facing identity: where it lives and what
/// language version applies (PRD §8.3, package_config's `languageVersion`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PackageIdentity {
    pub name: PackageName,
    /// Package root directory, lexically normalized (no symlink resolution
    /// — identity needs spelling stability, not filesystem truth).
    pub root: PathBuf,
    /// The Dart-source directory (`packageUri`, normally `lib/`).
    pub lib_dir: PathBuf,
    pub language_version: Option<dartiq_pub::LanguageVersion>,
    pub category: SourceCategory,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PackageError;
    use dartiq_source::content_hash;

    fn instance(name: &str, source_ref: SourceRef) -> PackageInstance {
        PackageInstance {
            name: name.to_owned(),
            source: SourceKind::Path,
            source_ref,
            version: None,
            content_hash: None,
        }
    }

    #[test]
    fn classifies_source_refs_by_layout() {
        assert_eq!(
            SourceCategory::of(&SourceRef::Hosted {
                url: "https://pub.dev".into(),
                sha256: None,
            }),
            SourceCategory::PubCache
        );
        assert_eq!(
            SourceCategory::of(&SourceRef::Git {
                url: "https://github.com/x/y".into(),
                resolved_ref: "abc".into(),
                path: String::new(),
            }),
            SourceCategory::PubCache
        );
        assert_eq!(
            SourceCategory::of(&SourceRef::Path {
                path: "/proj".into()
            }),
            SourceCategory::Path
        );
        assert_eq!(
            SourceCategory::of(&SourceRef::Sdk { sdk: "dart".into() }),
            SourceCategory::Sdk
        );
    }

    #[test]
    fn package_id_carries_the_instance_tuple() {
        let mut instance = instance("hosted_dep", SourceRef::Path { path: "/p".into() });
        instance.source = SourceKind::Hosted;
        instance.version = Some(Version::new(1, 4, 0));
        let id = PackageId::from_instance(&instance).expect("valid name");

        assert_eq!(id.name.as_str(), "hosted_dep");
        assert_eq!(id.source, SourceKind::Hosted);
        assert_eq!(
            id.version.as_ref().map(ToString::to_string).as_deref(),
            Some("1.4.0")
        );
        assert_eq!(id.content_hash, None);
    }

    #[test]
    fn package_id_rejects_non_package_names() {
        let instance = instance("sdk:dart", SourceRef::Sdk { sdk: "dart".into() });
        assert!(matches!(
            PackageId::from_instance(&instance),
            Err(PackageError::InvalidName { .. })
        ));
    }

    #[test]
    fn package_id_equality_tracks_every_identity_component() {
        let base = instance("dep", SourceRef::Path { path: "/a".into() });
        let id = PackageId::from_instance(&base).expect("valid");

        // Same source bytes, different content hash: different instance.
        let mut rehash = base.clone();
        rehash.content_hash = Some(content_hash(b"other bytes"));
        let rehashed = PackageId::from_instance(&rehash).expect("valid");
        assert_ne!(id, rehashed);

        // Same version+source at a different checkout path: different instance.
        let moved = instance("dep", SourceRef::Path { path: "/b".into() });
        assert_ne!(id, PackageId::from_instance(&moved).expect("valid"));
    }
}
