//! The query surface of a loaded [`SdkSnapshot`]: target tables, `dart:` URI
//! resolution, and conditional-import support values.

use std::path::{Component, Path, PathBuf};

use crate::metadata::{SdkSnapshot, TargetLibraries};

impl SdkSnapshot {
    /// The dart SDK root this snapshot was loaded from (the directory
    /// holding `version` and `lib/`).
    #[must_use]
    pub fn dart_sdk_root(&self) -> &Path {
        &self.dart_sdk_root
    }

    /// The expanded library table of target segment `segment` (`"vm"`,
    /// `"dart2js"`, `"dartdevc"`, …), or `None` when this SDK's
    /// `libraries.json` has no such segment.
    ///
    /// Segment sets drift across SDK versions, so segments are addressed by
    /// their `libraries.json` name — never a hardcoded enumeration
    ///.
    #[must_use]
    pub fn target(&self, segment: &str) -> Option<&TargetLibraries> {
        self.sdk_libraries.get(segment)
    }

    /// Resolves a `dart:` URI against target segment `segment` to a source
    /// file path: `"dart:core"` to the library root
    /// (`<root>/lib/core/core.dart`) and `"dart:core/bool.dart"` to the part
    /// file joined relative to the library's directory — the analyzer's
    /// `mapDartUri` consumption surface
    /// (`pkg/analyzer/lib/src/dart/sdk/sdk.dart`). Part joins mirror the
    /// analyzer's string concat, so an absolute-shaped part
    /// (`dart:core//etc/x`) lands *under* the library directory instead of
    /// re-anchoring the path. Resolution is purely
    /// metadata-level: the returned path need not exist yet (the content
    /// layer reads and hashes it on demand).
    ///
    /// Patch files are deliberately unreachable from here: they are backend
    /// implementation details, kept as [`SdkLibrary::patches`](crate::SdkLibrary::patches)
    /// path lists only.
    ///
    /// The structural twin [`SdkSnapshot::library_to_path`] consumes the
    /// already-split `(library, part)` pair with byte-identical semantics;
    /// this face parses and keeps the explicit per-cause errors.
    ///
    /// # Errors
    ///
    /// [`SdkUriError::NotDartUri`] when `dart_uri` lacks the `dart:` prefix,
    /// [`SdkUriError::UnknownTarget`] when the SDK has no `segment`, and
    /// [`SdkUriError::UnknownLibrary`] when the segment's effective table
    /// has no such library (e.g. `dart:html` on `vm`). No failure mode is a
    /// silent `None`.
    pub fn uri_to_path(&self, segment: &str, dart_uri: &str) -> Result<PathBuf, SdkUriError> {
        let rest = dart_uri
            .strip_prefix("dart:")
            .ok_or_else(|| SdkUriError::NotDartUri {
                uri: dart_uri.to_owned(),
            })?;
        let (name, part) = match rest.split_once('/') {
            Some((name, part)) => (name, part),
            None => (rest, ""),
        };
        let library = &self
            .target(segment)
            .ok_or_else(|| SdkUriError::UnknownTarget {
                target: segment.to_owned(),
            })?
            .libraries;
        let info = library
            .get(name)
            .ok_or_else(|| SdkUriError::UnknownLibrary {
                target: segment.to_owned(),
                library: name.to_owned(),
            })?;
        Ok(self.join_part(&info.uri, part))
    }

    /// Resolves a `dart:` library from its already-split parts against
    /// target segment `segment`: the face [`SdkSnapshot::uri_to_path`]
    /// parses back out of the joined URI string, minus the round-trip.
    /// Byte-identical to the string form — for every input,
    /// `library_to_path(segment, library, part)` equals
    /// `uri_to_path(segment, &join)` where `join` is `"dart:{library}"` for
    /// an empty `part` and `"dart:{library}/{part}"` otherwise, with `None`
    /// standing in for every error.
    ///
    /// `library` is a table name (`core`, `_internal`) — a name carrying a
    /// `/` can never hit the table and always yields `None`, mirroring the
    /// string form's first-segment split. `part` is the raw remainder after
    /// that split, percent-escapes intact: the same analyzer string-concat
    /// join applies, so an absolute-shaped part (`/etc/passwd`) stays under
    /// the library directory and `..` climbs are kept.
    #[must_use]
    pub fn library_to_path(&self, segment: &str, library: &str, part: &str) -> Option<PathBuf> {
        let info = self.target(segment)?.libraries.get(library)?;
        Some(self.join_part(&info.uri, part))
    }

    /// The flat existence face: resolves `library` (+ optional `part`)
    /// against the generated [`FLAT_LIBRARIES`] table — the oracle's own
    /// `dart:` library map, flat across all target segments (the vendored
    /// analyzer never consults platform flags for URI existence), which is
    /// the posture `dart analyze` answers `uri_does_not_exist` from.
    /// `library` carries no `dart:` prefix; a name outside the table (e.g.
    /// `web_sql`, on disk in the SDK tree but tabulated by neither side)
    /// yields `None`.
    ///
    /// [`FLAT_LIBRARIES`] is generated from the pinned oracle SDK — see
    /// `tools/sdk-faces-gen`; the per-segment tables above stay authoritative
    /// for `dart.library.*` support values.
    #[must_use]
    pub fn flat_library_to_path(&self, library: &str, part: &str) -> Option<PathBuf> {
        let index = crate::FLAT_LIBRARIES
            .binary_search_by_key(&library, |entry| entry.0)
            .ok()?;
        Some(self.join_part(crate::FLAT_LIBRARIES[index].1, part))
    }

    /// The single path builder behind both faces: `<root>/lib/<library uri>`
    /// with `part` joined the analyzer's way.
    fn join_part(&self, library_uri: &str, part: &str) -> PathBuf {
        let lib_root = self.dart_sdk_root.join("lib");
        let library_file = lib_root.join(library_uri);
        match part {
            "" => library_file,
            part => {
                // The analyzer builds part paths by string concat
                // (`'$prefix$relativePath'`, `pkg/analyzer/lib/src/dart/sdk/sdk.dart`),
                // so a leading `/` in the part merely collapses into a double
                // slash and the result always stays under the library
                // directory. `Path::join` has no such tolerance — an absolute
                // `part` would REPLACE the base — so absolute components
                // (root/prefix) are dropped before joining, while `..`
                // climbs stay exactly as the analyzer resolves them.
                let relative = Path::new(part)
                    .components()
                    .skip_while(|component| {
                        matches!(component, Component::RootDir | Component::Prefix(_))
                    })
                    .collect::<PathBuf>();
                library_file.parent().unwrap_or(&lib_root).join(relative)
            }
        }
    }

    /// The `dart.library.<library>` environment value for target segment
    /// `segment`: `true` exactly when the library is in the segment's
    /// effective table and allowed to participate in conditional imports
    /// (`support_conditional_import`; `_`-prefixed names are forced off at
    /// parse time).
    ///
    /// The key domain is library names, not platform segment names: the CFE
    /// looks up `dart.library.<libraryName>` against the target's effective
    /// library set (`pkg/kernel/lib/target/targets.dart`,
    /// `dartLibraryPrefix`).
    #[must_use]
    pub fn library_support_value(&self, segment: &str, library: &str) -> bool {
        self.target(segment)
            .and_then(|target| target.libraries.get(library))
            .is_some_and(|info| info.support_conditional_import)
    }
}

/// The deprecation annotation message of the `dart:` library `library`
/// (name without the `dart:` prefix), or `None` when its `library`
/// directive carries none — the SDK-side half of upstream's
/// `deprecated_member_use` on import directives
/// (`pkg/analyzer/lib/src/error/element_usage_detector.dart:769`,
/// `importDirective` → `checkUsage(node.libraryImport?.importedLibrary)`).
///
/// The raw annotation text is returned verbatim: `""` for a bare
/// `@deprecated` (upstream treats it as no message). Message normalization
/// — trim, `.`-terminated punctuation check, empty-means-none — is the
/// reporter's own policy upstream
/// (`pkg/analyzer/lib/src/error/deprecated_member_use_verifier.dart`,
/// `normalizeDeprecationMessage`) and stays the consumer's call.
///
/// The table is snapshot-independent SDK-source metadata — every SDK ships
/// the same library files — so this face takes no `self`; it is a free
/// function sibling of the [`FLAT_LIBRARIES`] table.
#[must_use]
pub fn deprecated_library_message(library: &str) -> Option<&'static str> {
    crate::metadata::DEPRECATED_SDK_LIBRARIES
        .binary_search_by_key(&library, |entry| entry.0)
        .ok()
        .map(|index| crate::metadata::DEPRECATED_SDK_LIBRARIES[index].1)
}

/// Failure of [`SdkSnapshot::uri_to_path`]: every unresolvable `dart:` URI
/// is an explicit error, never a silent no-op.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SdkUriError {
    /// The URI does not start with the `dart:` scheme.
    #[error("not a `dart:` URI: `{uri}`")]
    NotDartUri { uri: String },
    /// The SDK's `libraries.json` has no segment with this name.
    #[error("unknown target segment `{target}`")]
    UnknownTarget { target: String },
    /// The target's effective library table has no library with this name.
    #[error("no dart: library `{library}` in target `{target}`")]
    UnknownLibrary { target: String, library: String },
}
