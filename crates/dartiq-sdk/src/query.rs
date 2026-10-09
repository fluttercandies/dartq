//! The query surface of a loaded [`SdkSnapshot`]: target tables, `dart:` URI
//! resolution, and conditional-import support values.

use std::path::{Path, PathBuf};

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
    /// (`docs/facts/sdk-metadata.md` §7).
    #[must_use]
    pub fn target(&self, segment: &str) -> Option<&TargetLibraries> {
        self.sdk_libraries.get(segment)
    }

    /// Resolves a `dart:` URI against target segment `segment` to a source
    /// file path: `"dart:core"` to the library root
    /// (`<root>/lib/core/core.dart`) and `"dart:core/bool.dart"` to the part
    /// file joined relative to the library's directory — the analyzer's
    /// `mapDartUri` consumption surface
    /// (`pkg/analyzer/lib/src/dart/sdk/sdk.dart`). Resolution is purely
    /// metadata-level: the returned path need not exist yet (the content
    /// layer reads and hashes it on demand).
    ///
    /// Patch files are deliberately unreachable from here: they are backend
    /// implementation details, kept as [`SdkLibrary::patches`](crate::SdkLibrary::patches)
    /// path lists only (`docs/facts/sdk-metadata.md` §6).
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
        let libraries = &self
            .target(segment)
            .ok_or_else(|| SdkUriError::UnknownTarget {
                target: segment.to_owned(),
            })?
            .libraries;
        let library = libraries
            .get(name)
            .ok_or_else(|| SdkUriError::UnknownLibrary {
                target: segment.to_owned(),
                library: name.to_owned(),
            })?;
        let lib_root = self.dart_sdk_root.join("lib");
        let library_file = lib_root.join(&library.uri);
        Ok(match part {
            "" => library_file,
            part => library_file.parent().unwrap_or(&lib_root).join(part),
        })
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
    /// `dartLibraryPrefix`; correction of PRD §7 in
    /// `docs/facts/sdk-metadata.md` §1).
    #[must_use]
    pub fn library_support_value(&self, segment: &str, library: &str) -> bool {
        self.target(segment)
            .and_then(|target| target.libraries.get(library))
            .is_some_and(|info| info.support_conditional_import)
    }
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
