//! Bundled declaration source for the `dart:` extension declarations whose
//! members apply implicitly to receiver types.
//!
//! The generated [`FLAT_LIBRARIES`] table answers where a `dart:` library
//! lives; the generated export-face table the engine builds its synthetic
//! `<dart:…>` nodes from answers which top-level names a `dart:` import
//! exposes. Neither carries element models, so extension *members* — which
//! surface on receiver types, not in import namespaces — resolve only when
//! an element model for the extension declaration reaches the semantic
//! extension pool. This table carries those declarations as checked-in Dart
//! source, in the same `(dart: library name, declaration excerpt)` shape the
//! engine's bundled SDK extension faces iterate (the consumer scans +
//! collects each excerpt once per analyze and indexes it under the
//! library's synthetic face file — see
//! `dartq-engine::sdk_extension_faces`).
//!
//! Source fidelity: verbatim member surfaces from the upstream Dart SDK
//! source, `sdk/lib/core/enum.dart` — `EnumName` (:137) and `EnumByName`
//! (:170) at the oracle of record (3.13.1). Members carry the exact
//! on-types, type-parameter bounds, names, kinds, return types, and formals
//! of the upstream declarations; concrete bodies are spelled `external`
//! (the name-level model is identical and no body is captured either way,
//! the engine excerpts' established ruling). `EnumByName.byName` missing
//! from the extension pool over-fired `undefined_method` on
//! `values.byName(...)` and on `List<T>` receivers whose type parameter is
//! bound by `extends Enum`.
//!
//! Only libraries whose corpus-visible surface includes implicit-application
//! members join — the table is empty of libraries whose face is name-only
//! complete, and it is hand-curated (a small, fidelity-reviewed surface),
//! unlike the generated bulk tables.

/// Per-library bundled extension-face source: `(dart: library name, the
/// declaration excerpt)`. Each entry's library must be addressable — a name
/// of the generated [`FLAT_LIBRARIES`] surface, so the consumer's synthetic
/// `<dart:{library}>` face file pairs with a library the graph build knows.
pub static EXTENSION_FACE_SOURCES: &[(&str, &str)] = &[(
    "core",
    r#"extension EnumName on Enum {
  external String get name;
}
extension EnumByName<T extends Enum> on Iterable<T> {
  external T byName(String name);
  external Map<String, T> asNameMap();
}
"#,
)];

#[cfg(test)]
mod tests {
    use super::EXTENSION_FACE_SOURCES;

    /// Every entry's library is addressable: the consumer interns the
    /// excerpt under the synthetic `<dart:{library}>` face file, and that
    /// id only pairs with the graph's SDK nodes when the library name is
    /// one the generated surfaces know (`FLAT_LIBRARIES` is the in-crate
    /// face of the same generated SDK table the engine's export-face list
    /// is built from).
    #[test]
    fn entries_reference_addressable_libraries() {
        for (library, source) in EXTENSION_FACE_SOURCES {
            assert!(
                crate::FLAT_LIBRARIES.iter().any(|(name, _)| name == library),
                "extension face library `{library}` is not an addressable dart: library"
            );
            assert!(!source.is_empty(), "extension face `{library}` is empty");
        }
    }

    /// A library joins the table at most once: the consumer indexes each
    /// excerpt under the library's face file, and a duplicate entry would
    /// silently overwrite the earlier face's element model.
    #[test]
    fn library_names_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for (library, _) in EXTENSION_FACE_SOURCES {
            assert!(seen.insert(*library), "duplicate extension face `{library}`");
        }
    }

    /// The `core` excerpt carries the dart:core enum extension pair with
    /// the upstream heads (exact on-types and bounds) and the exact
    /// member surfaces — `EnumName.name`,
    /// `EnumByName.byName`/`EnumByName.asNameMap` (upstream Dart SDK
    /// source, sdk/lib/core/enum.dart:137/:170). These spellings are the
    /// contract the semantic extension pool resolves against; any drift
    /// here is a parity regression on the `values.byName(...)` face.
    #[test]
    fn core_excerpt_carries_the_enum_extension_faces() {
        let core = EXTENSION_FACE_SOURCES
            .iter()
            .find(|(library, _)| *library == "core")
            .expect("core extension face present")
            .1;
        // Exact declaration heads: on-types and the `T extends Enum` bound.
        assert!(
            core.contains("extension EnumName on Enum {"),
            "EnumName head drifted"
        );
        assert!(
            core.contains("extension EnumByName<T extends Enum> on Iterable<T> {"),
            "EnumByName head or bound drifted"
        );
        // Member surfaces: kinds, names, return types, formals.
        assert!(
            core.contains("external String get name;"),
            "EnumName.name surface drifted"
        );
        assert!(
            core.contains("external T byName(String name);"),
            "EnumByName.byName surface drifted"
        );
        assert!(
            core.contains("external Map<String, T> asNameMap();"),
            "EnumByName.asNameMap surface drifted"
        );
    }
}
