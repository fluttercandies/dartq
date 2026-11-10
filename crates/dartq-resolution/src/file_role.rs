//! The file-kind classification and the part-of role matrix — the pure
//! single-source mirror of the analyzer's `FileState._updateKind`
//! (file_state.dart:928-949) and the include-side arms of
//! `library_analyzer.dart::_resolvePartDirective` (:1015-1071), pinned
//! against `dart analyze` 3.13.7 over the external (`dart:`) and workspace
//! faces alike.
//!
//! The classification is a pure function of one file's directive facts:
//!
//! 1. a `library` directive classifies the file as a library outright (any
//!    `part of` is dead weight — the oracle's first `_updateKind` branch);
//! 2. else `part of a.b.c;` binds by library-name equality
//!    (`PartOfNameFileKind`);
//! 3. else `part of '<uri>';` binds by URI-file identity — resolved target
//!    [`FileRole::PartOfUriKnownFile`], an SDK (`dart:`) target
//!    [`FileRole::PartOfUriSdk`] (Known upstream — the uriFile is the SDK
//!    file; dartq has no `FileId` for it), anything unresolvable or
//!    shapeless [`FileRole::PartOfUriUnknown`] (never attaches);
//! 4. else the file is an unnamed library.
//!
//! The matrix ([`part_of_row`]) answers, for one `part '…'` directive whose
//! URI already resolved and exists (the interpolation/invalid/missing rows
//! and `duplicate_part`'s per-library membership check are the caller's
//! earlier gates), which part-family row the include carries — or that the
//! part joined the library silently.

use dartq_source::FileId;

use crate::uri::{ResolvedTarget, SdkFileKind};

/// The role one file's directives classify it under — the four-way
/// `_updateKind` outcome plus dartq's split of the URI form's known target
/// by analysis-set membership (SDK files have no [`FileId`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileRole<'a> {
    /// A `library` directive (named or bare), or no `part of` at all.
    Library { name: Option<&'a str> },
    /// `part of a.b.c;` — binds by library-name equality.
    PartOfName { name: &'a str },
    /// `part of '<uri>';` resolving to a file in the analysis set — binds
    /// by identity with the file carrying the `part` directive.
    PartOfUriKnownFile { target: FileId },
    /// `part of '<uri>';` resolving to an SDK (`dart:`) file — Known
    /// upstream, but no workspace container can match its identity.
    PartOfUriSdk,
    /// `part of '<uri>';` with an unresolvable, invalid, or shapeless
    /// target — `PartOfUriUnknownFileKind`, never attaches.
    PartOfUriUnknown,
}

impl FileRole<'_> {
    /// The classification over one unit's directive facts:
    /// `library_directive` (a `library` keyword is present) with its
    /// declared `name` (`None` for a bare `library;`), the name-form
    /// part-of's dotted name, and the URI-form part-of's resolution target
    /// (`None` when the unit carries no URI-form `part of`; the engine's
    /// unresolved default maps to [`FileRole::PartOfUriUnknown`] — a
    /// shapeless or missing target is the same classification).
    #[must_use]
    pub fn of<'a>(
        library_directive: bool,
        library_name: Option<&'a str>,
        part_of_name: Option<&'a str>,
        part_of_target: Option<ResolvedTarget>,
    ) -> FileRole<'a> {
        if library_directive {
            return FileRole::Library { name: library_name };
        }
        if let Some(name) = part_of_name {
            return FileRole::PartOfName { name };
        }
        match part_of_target {
            Some(ResolvedTarget::Resolved(target)) => FileRole::PartOfUriKnownFile { target },
            Some(ResolvedTarget::External) => FileRole::PartOfUriSdk,
            // A URI-form part-of whose target is missing (or shapeless —
            // the engine's unresolved default): the unattached URI form.
            Some(ResolvedTarget::Missing) => FileRole::PartOfUriUnknown,
            // No part-of at all: the oracle's unnamed library.
            None => FileRole::Library { name: None },
        }
    }

    /// The classification of an external SDK file probed from content
    /// ([`SdkFileKinds::probe`](crate::SdkFileKinds::probe)) — the probe
    /// decides the part-of form the same way `_updateKind` does over the
    /// parsed unit. Borrows the probe's answer; the caller holds the
    /// [`SdkFileKind`] it asked for.
    #[must_use]
    pub fn of_sdk(kind: &SdkFileKind) -> FileRole<'_> {
        match kind {
            SdkFileKind::Library => FileRole::Library { name: None },
            SdkFileKind::NamePartOf(name) => FileRole::PartOfName { name },
            SdkFileKind::UriPartOf => FileRole::PartOfUriSdk,
        }
    }

    /// Whether the file is a part (carries a live `part of`): every role
    /// but [`FileRole::Library`].
    #[must_use]
    pub const fn is_part(&self) -> bool {
        !matches!(self, FileRole::Library { .. })
    }
}

/// Which part-family row one include carries (the `_resolvePartDirective`
/// arms after the URI-layer gates and the `duplicate_part` membership
/// check), or that the include joined silently. The row carries no message
/// strings: the emitter owns the spellings (the included/includer URIs and
/// library names it already holds).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartOfRow {
    /// The part joined this library (`isPartOf` held) — no row.
    Attached,
    /// `part_of_non_part` — the included file exists but is not a part.
    NonPart,
    /// `part_of_unnamed_library` — name-form part, the library is unnamed.
    UnnamedLibrary,
    /// `part_of_different_library` — name-form mismatch (library names).
    NameMismatch,
    /// `part_of_different_library` — URI-form mismatch (URI strings).
    UriMismatch,
    /// An unattached name-form part under the enhanced-parts feature: the
    /// name rows are gated off (`library_analyzer.dart:1046-1062` runs them
    /// only `!isEnabled(Feature.enhanced_parts)`) and the parser's
    /// `part_of_name` is the covering row.
    Silent,
}

/// The include-side role matrix. `included` is the target file's role;
/// `includer` is the role of the file carrying the `part` directive (the
/// container of `isPartOf` — a part includer never claims a name-form
/// part, `file_state.dart:2222-2225`); `includer_file` is that file's id
/// (the identity a known URI-form part-of binds to,
/// `file_state.dart:2248-2252`); `library_name` is the DEFINING library's
/// declared name (`_libraryElement.name`, the name the mismatch rows
/// print); `enhanced_parts` gates the name rows.
#[must_use]
pub fn part_of_row(
    included: FileRole<'_>,
    includer: FileRole<'_>,
    includer_file: FileId,
    library_name: Option<&str>,
    enhanced_parts: bool,
) -> PartOfRow {
    match included {
        FileRole::Library { .. } => PartOfRow::NonPart,
        FileRole::PartOfUriKnownFile { target } if target == includer_file => PartOfRow::Attached,
        FileRole::PartOfUriKnownFile { .. }
        | FileRole::PartOfUriSdk
        | FileRole::PartOfUriUnknown => PartOfRow::UriMismatch,
        FileRole::PartOfName { name } => {
            let attached = match includer {
                FileRole::Library {
                    name: Some(container_name),
                } => container_name == name,
                _ => false,
            };
            if attached {
                PartOfRow::Attached
            } else if enhanced_parts {
                PartOfRow::Silent
            } else if library_name.is_none() {
                PartOfRow::UnnamedLibrary
            } else {
                PartOfRow::NameMismatch
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dartq_source::FileTable;

    /// Two distinct interned ids (equality is identity; any two paths do).
    fn two_files() -> (FileId, FileId) {
        let mut files = FileTable::new();
        let a = files.intern(std::path::Path::new("/w/lib/a.dart"));
        let b = files.intern(std::path::Path::new("/w/lib/b.dart"));
        (a, b)
    }

    /// The classification precedence: `library` over `part of`, name form
    /// over URI form, and the URI form's target split
    /// (`_updateKind`, file_state.dart:928-949).
    #[test]
    fn classification_precedence_mirrors_update_kind() {
        // A `library` directive wins over any part-of.
        assert_eq!(
            FileRole::of(
                true,
                Some("my.lib"),
                Some("other.lib"),
                Some(ResolvedTarget::Missing)
            ),
            FileRole::Library {
                name: Some("my.lib")
            }
        );
        // A bare `library;` is a named-less library all the same.
        assert_eq!(
            FileRole::of(true, None, None, None),
            FileRole::Library { name: None }
        );
        // Name form over URI form.
        assert_eq!(
            FileRole::of(false, None, Some("a.b"), Some(ResolvedTarget::External)),
            FileRole::PartOfName { name: "a.b" }
        );
        // The URI form's binding split.
        let (target, _) = two_files();
        assert_eq!(
            FileRole::of(false, None, None, Some(ResolvedTarget::Resolved(target))),
            FileRole::PartOfUriKnownFile { target }
        );
        assert_eq!(
            FileRole::of(false, None, None, Some(ResolvedTarget::External)),
            FileRole::PartOfUriSdk
        );
        assert_eq!(
            FileRole::of(false, None, None, Some(ResolvedTarget::Missing)),
            FileRole::PartOfUriUnknown
        );
        // No part-of at all: the unnamed library.
        assert_eq!(
            FileRole::of(false, None, None, None),
            FileRole::Library { name: None }
        );
        // `is_part` is exactly "not a library".
        assert!(!FileRole::of(false, None, None, None).is_part());
        assert!(FileRole::of(false, None, Some("a"), None).is_part());
        assert!(FileRole::of(false, None, None, Some(ResolvedTarget::External)).is_part());
    }

    /// The external (`dart:`) faces pinned against `dart analyze` 3.13.7:
    /// an SDK entry (`dart:math`) is a library → `part_of_non_part`; an SDK
    /// URI-form part (`dart:async/future.dart`) never attaches → the URI
    /// mismatch; an SDK name-form part (`dart:developer/
    /// creation_tracking.dart`, `part of dart.developer;`) attaches by name
    /// equality, else the name rows.
    #[test]
    fn sdk_faces_match_the_oracle_rows() {
        let (includer_file, _) = two_files();
        let uri_part = SdkFileKind::UriPartOf;
        let name_part = SdkFileKind::NamePartOf("dart.developer".into());
        // `library dart.async; part 'dart:async/future.dart';`
        assert_eq!(
            part_of_row(
                FileRole::of_sdk(&uri_part),
                FileRole::Library {
                    name: Some("dart.async")
                },
                includer_file,
                Some("dart.async"),
                false,
            ),
            PartOfRow::UriMismatch
        );
        // `part 'dart:math';` — the entry is a library.
        assert_eq!(
            part_of_row(
                FileRole::of_sdk(&SdkFileKind::Library),
                FileRole::Library { name: None },
                includer_file,
                None,
                false,
            ),
            PartOfRow::NonPart
        );
        // `library L3; part 'dart:developer/creation_tracking.dart';`
        assert_eq!(
            part_of_row(
                FileRole::of_sdk(&name_part),
                FileRole::Library { name: Some("L3") },
                includer_file,
                Some("L3"),
                false,
            ),
            PartOfRow::NameMismatch
        );
        // `library dart.developer;` over the same part: silent attach.
        assert_eq!(
            part_of_row(
                FileRole::of_sdk(&name_part),
                FileRole::Library {
                    name: Some("dart.developer")
                },
                includer_file,
                Some("dart.developer"),
                false,
            ),
            PartOfRow::Attached
        );
        // The unnamed includer: `part_of_unnamed_library`.
        assert_eq!(
            part_of_row(
                FileRole::of_sdk(&name_part),
                FileRole::Library { name: None },
                includer_file,
                None,
                false,
            ),
            PartOfRow::UnnamedLibrary
        );
    }

    /// The workspace faces: a known URI-form part attaches only to the file
    /// its `part of` URI resolves to (the depth-2 chain joins through the
    /// direct includer, not the defining library); anything else takes the
    /// URI mismatch; a part includer never claims a name-form part; the
    /// name rows are gated off under enhanced-parts.
    #[test]
    fn workspace_faces_match_the_matrix() {
        let (defining, mid) = two_files();
        // leaf's `part of 'mid.dart'` binds the DIRECT includer.
        assert_eq!(
            part_of_row(
                FileRole::PartOfUriKnownFile { target: mid },
                FileRole::Library { name: None },
                mid,
                None,
                false,
            ),
            PartOfRow::Attached
        );
        // The defining library's own include of the same leaf: mismatch.
        assert_eq!(
            part_of_row(
                FileRole::PartOfUriKnownFile { target: mid },
                FileRole::Library { name: None },
                defining,
                None,
                false,
            ),
            PartOfRow::UriMismatch
        );
        // An unresolvable part-of URI still mismatches (UnknownFileKind
        // never attaches, `_resolvePartDirective`'s PartOfUriFileKind arm).
        assert_eq!(
            part_of_row(
                FileRole::PartOfUriUnknown,
                FileRole::Library { name: Some("a") },
                defining,
                Some("a"),
                false,
            ),
            PartOfRow::UriMismatch
        );
        // A name-form part of a part-includer: the container is not a
        // library, so no attach — the defining library's name decides the
        // row.
        assert_eq!(
            part_of_row(
                FileRole::PartOfName { name: "my.lib" },
                FileRole::PartOfUriKnownFile { target: defining },
                mid,
                Some("my.lib"),
                false,
            ),
            PartOfRow::NameMismatch
        );
        // Enhanced parts: the name rows vanish (the parser's
        // `part_of_name` covers the form).
        assert_eq!(
            part_of_row(
                FileRole::PartOfName { name: "my.lib" },
                FileRole::Library {
                    name: Some("other.lib")
                },
                defining,
                Some("other.lib"),
                true,
            ),
            PartOfRow::Silent
        );
    }
}
