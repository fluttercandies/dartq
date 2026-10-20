//! The library-scope use-face substrate (M3 U1, #767): what each analyzed
//! unit observably uses, and the per-library fold the unused-element /
//! unused-import consumers close over.
//!
//! # Reference semantics (behavior, not shape)
//!
//! The oracle gathers a per-file `UsedLocalElements` record
//! (`GatherUsedLocalElementsVisitor`) for every file of a library, merges
//! the records, and runs the warning passes against the merged set
//! (library_analyzer.dart:305-315, `UsedLocalElements.merge` at
//! unused_local_elements_verifier.dart:1358-1371) — a use in any file of
//! the library spares a declaration in every other file of the same
//! library. [`UnitUseFace`] is the per-unit gather at name granularity;
//! [`LibraryUsages::close_library_usage`] is the merge, producing the
//! per-library consumed set ([`LibraryUseSet`]) its consumers fold into
//! their per-declaration checks.
//!
//! # Layout
//!
//! Element keys are interned [`StringId`]s (u32), never strings — a close
//! unions ids against the shared string table and interns nothing (zero
//! live strings). The per-reference recording rides the existing pass sets
//! (the engine moves them into the face at extraction), so the hot path
//! pays nothing: no per-use allocation beyond the sets the diagnosis
//! already kept. Faces accumulate per library and are released at close —
//! per-library maps, no unbounded growth.

use dartq_resolution::LibraryId;
use dartq_source::{FileId, SourceRange, StringId};
use rustc_hash::{FxHashMap, FxHashSet};

/// One unit's observable uses (the per-file gather face).
#[derive(Debug, Default)]
pub struct UnitUseFace {
    /// Names some resolved reference used: top-level names, containers,
    /// class members, import prefixes, import-shown names, type
    /// annotations, creation heads (the official `elements` ∪ `members`
    /// sets at name granularity). Locals included — a shadowed use only
    /// ever suppresses, the under-report direction.
    pub used: FxHashSet<StringId>,
    /// Names a READ-shaped reference resolved to (the `unused_field`
    /// read-only rule: writes never spare a field; the official
    /// `readMembers`). A subset of [`Self::used`].
    pub reads: FxHashSet<StringId>,
    /// Annotation names a resolved `@name` reference used (the official
    /// `addElement` on metadata resolution; an unresolved annotation is an
    /// error of its own and never counts).
    pub annotations: FxHashSet<StringId>,
    /// Extension members an implicit application resolved to (the official
    /// `usedElements.members` — an ELEMENT set): `(declaring file, index of
    /// the face in that file's `extension_faces()`, member spelling)`. The
    /// element key keeps the oracle's `identical(element.library)` gate at
    /// the library close: a resolved application credits the exact face it
    /// bound to — same-spelled members of sibling extensions, or of an
    /// extension in another library, never share the credit.
    pub extension_members: FxHashSet<(FileId, u32, StringId)>,
    /// The unit's import directives: the written prefix and shown names,
    /// in written order — the per-import attribution face the official
    /// combinator semantics consumes (which import contributed a used
    /// name).
    pub imports: Vec<ImportUseFace>,
    /// The unit's export directives' shown names, in written order — the
    /// re-export face (a name this library re-exports observably flows
    /// through it).
    pub exports: Vec<ExportUseFace>,
}

/// One import directive's written combinator face (prefix + shown names;
/// hide names are never uses and don't join).
#[derive(Debug, Default)]
pub struct ImportUseFace {
    /// The `as p` prefix (`None` = unnamed import).
    pub prefix: Option<StringId>,
    /// The `show` clause's names, in written order (empty = unfiltered or
    /// hide-only).
    pub shown: Box<[StringId]>,
}

/// One export directive's written show face.
#[derive(Debug)]
pub struct ExportUseFace {
    /// The `show` clause's names, in written order.
    pub shown: Vec<StringId>,
    /// The URI as written (quotes stripped); `""` when the directive's URI
    /// was not a clean literal (the recovery posture — the deprecated-SDK
    /// face stays silent on it).
    pub uri_text: String,
    /// The whole directive's span (`export` keyword start → `;` end) —
    /// the `deprecated_member_use` face's anchor (upstream
    /// `element_usage_detector.dart:256`, `usageRange: node.sourceRange` =
    /// the ExportDirective, not the URI literal). Zero-length when the
    /// capture never ran (a `default()` face built outside the listener).
    pub directive_range: SourceRange,
}

impl Default for ExportUseFace {
    fn default() -> Self {
        Self {
            shown: Vec::new(),
            uri_text: String::new(),
            directive_range: SourceRange {
                offset: 0,
                length: 0,
            },
        }
    }
}

/// The per-library consumed set (the official merged `UsedLocalElements`
/// at name granularity): every name some unit of the library observably
/// used. The unused-element family checks declarations against `used`
/// (annotations folded in — the official metadata uses are element uses);
/// the `unused_field` read-only rule checks against `reads`.
#[derive(Debug, Default)]
pub struct LibraryUseSet {
    /// Every used name, annotations included.
    pub used: FxHashSet<StringId>,
    /// The read-shaped subset.
    pub reads: FxHashSet<StringId>,
    /// Every extension member some unit of the library resolved an
    /// implicit application to (the element-keyed `usedElements.members`
    /// merge — the library-close counterpart of the name pool).
    pub extension_members: FxHashSet<(FileId, u32, StringId)>,
}

impl LibraryUseSet {
    /// Whether `name` was consumed by any use shape (read or write; the
    /// `_isUsedElement` indexed-use test).
    #[must_use]
    pub fn used(&self, name: StringId) -> bool {
        self.used.contains(&name)
    }

    /// Whether `name` was consumed by a READ-shaped use (the
    /// `_isReadMember` test).
    #[must_use]
    pub fn read(&self, name: StringId) -> bool {
        self.reads.contains(&name)
    }

    /// Whether the extension member `key` (`declaring file`, `face index`,
    /// `spelling`) was consumed by a resolved implicit application (the
    /// `_isUsedMember` element-identity test).
    #[must_use]
    pub fn extension_member_used(&self, key: (FileId, u32, StringId)) -> bool {
        self.extension_members.contains(&key)
    }
}

/// The analyze fan-out's accumulated unit faces, keyed by owning library.
/// One per analysis; [`Self::close_library_usage`] consumes a library's
/// faces, so closed libraries hold nothing.
#[derive(Debug, Default)]
pub struct LibraryUsages {
    faces: FxHashMap<LibraryId, Vec<UnitUseFace>>,
}

impl LibraryUsages {
    /// Records one diagnosed unit's face under its owning library.
    pub fn add_unit(&mut self, lib: LibraryId, face: UnitUseFace) {
        self.faces.entry(lib).or_default().push(face);
    }

    /// Folds `lib`'s accumulated unit faces into the per-library consumed
    /// set (the `UsedLocalElements.merge` face) and releases them. The
    /// fold is a plain set union per field: uses are library-visible by
    /// construction (a use spares the declaration it resolves to, in
    /// every file of the same library), so the merge is order-free and
    /// allocation-free beyond the union sets themselves.
    ///
    /// Units of *other* libraries never enter `lib`'s set: imports are
    /// per-file faces (an import is used from its own file only), and the
    /// unused-element candidates are library-private names, reachable
    /// only from the declaring library.
    pub fn close_library_usage(&mut self, lib: LibraryId) -> LibraryUseSet {
        let mut set = LibraryUseSet::default();
        if let Some(faces) = self.faces.remove(&lib) {
            for face in &faces {
                set.used.extend(&face.used);
                set.reads.extend(&face.reads);
                set.used.extend(&face.annotations);
                set.extension_members.extend(&face.extension_members);
            }
        }
        set
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::collect;
    use crate::diagnose::{StrictModes, diagnose_unit};
    use crate::library::ElementIndex;
    use crate::model::MicroKind;
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_parser::ParserFeatures;
    use dartq_resolution::{
        CombinatorClause, Combinators, ImportEdge, LibraryClassModifiers, LibraryGraph,
        LibraryGraphBuilder, Namespaces,
    };
    use dartq_scanner::{ScanConfig, ScanResult};
    use dartq_source::{FileId, FileTable, StringTable};
    use std::path::Path;

    fn features() -> ParserFeatures {
        ParserFeatures::from_feature_set(FeatureSet::for_language_version(LanguageVersion::new(
            3, 15,
        )))
    }

    fn scanned(source: &str, file: FileId) -> ScanResult {
        let result = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(result.diagnostics.is_empty(), "scanner: {source:?}");
        result
    }

    /// Diagnoses `source` as the sole file of a library with no imports;
    /// returns the use-face plus the string table (the face's ids resolve
    /// against it).
    fn sole_face(source: &str) -> (UnitUseFace, StringTable) {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file = files.intern(Path::new("main.dart"));
        let scanned = scanned(source, file);
        let features = features();
        let elements = collect(source, &scanned.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib = builder.intern("package:t/main.dart");
        let mut index = ElementIndex::default();
        index.insert(file, elements.clone());
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let (_, face) = diagnose_unit(
            source,
            &scanned.tokens,
            features,
            &strings,
            file,
            lib,
            &graph,
            &namespaces,
            &index,
            StrictModes::default(),
            &LibraryClassModifiers::default(),
            true,
        );
        (face, strings)
    }

    /// The two-library fixture: `b_source` is library B (declarations),
    /// `a_source` is library A importing it through `edge` (built with the
    /// live string table; A's own declarations are declared into the
    /// graph). [`Self::faces`] diagnoses both units; the id lookups go
    /// through [`Self::name`].
    struct TwoLib {
        strings: StringTable,
        graph: LibraryGraph,
        namespaces: Namespaces,
        index: ElementIndex,
        scanned_a: ScanResult,
        scanned_b: ScanResult,
        source_a: String,
        source_b: String,
        file_a: FileId,
        file_b: FileId,
        lib_a: LibraryId,
        lib_b: LibraryId,
    }

    impl TwoLib {
        fn build(
            a_source: &str,
            b_source: &str,
            edge: impl FnOnce(&StringTable, LibraryId) -> ImportEdge,
        ) -> Self {
            let strings = StringTable::new();
            let mut files = FileTable::new();
            let file_a = files.intern(Path::new("a.dart"));
            let file_b = files.intern(Path::new("b.dart"));
            let scanned_a = scanned(a_source, file_a);
            let scanned_b = scanned(b_source, file_b);
            let features = features();
            let elements_a = collect(a_source, &scanned_a.tokens, features, &strings);
            let elements_b = collect(b_source, &scanned_b.tokens, features, &strings);
            let mut builder = LibraryGraphBuilder::new(&strings);
            let lib_a = builder.intern("package:t/a.dart");
            let lib_b = builder.intern("package:t/b.dart");
            let mut index = ElementIndex::default();
            index.insert(file_a, elements_a.clone());
            index.insert(file_b, elements_b.clone());
            for (name, decl) in elements_a.declarations() {
                if decl.kind == MicroKind::TypeParam {
                    continue;
                }
                builder.declare(lib_a, name, file_a);
            }
            for (name, _) in elements_b.declarations() {
                builder.declare(lib_b, name, file_b);
            }
            builder.import(lib_a, edge(&strings, lib_b));
            let graph = builder.finish();
            let namespaces = Namespaces::build(&graph);
            Self {
                strings,
                graph,
                namespaces,
                index,
                scanned_a,
                scanned_b,
                source_a: a_source.to_owned(),
                source_b: b_source.to_owned(),
                file_a,
                file_b,
                lib_a,
                lib_b,
            }
        }

        fn name(&self, text: &str) -> StringId {
            self.strings.intern(text)
        }

        /// Diagnoses both units through the live pipeline.
        fn faces(&self) -> (UnitUseFace, UnitUseFace) {
            let features = features();
            let (_, face_a) = diagnose_unit(
                &self.source_a,
                &self.scanned_a.tokens,
                features,
                &self.strings,
                self.file_a,
                self.lib_a,
                &self.graph,
                &self.namespaces,
                &self.index,
                StrictModes::default(),
                &LibraryClassModifiers::default(),
                true,
            );
            let (_, face_b) = diagnose_unit(
                &self.source_b,
                &self.scanned_b.tokens,
                features,
                &self.strings,
                self.file_b,
                self.lib_b,
                &self.graph,
                &self.namespaces,
                &self.index,
                StrictModes::default(),
                &LibraryClassModifiers::default(),
                true,
            );
            (face_a, face_b)
        }
    }

    /// A plain reference lands in the face's used set (`g` invoked from
    /// `main`), and the read-shaped subset stays distinct (an invocation
    /// is a use, not a field read).
    #[test]
    fn face_records_a_plain_reference() {
        let (face, strings) = sole_face("void g() {} void main() { g(); }");
        let g = strings.intern("g");
        assert!(face.used.contains(&g), "{face:?}");
        assert!(!face.reads.contains(&g));
    }

    /// An import-shown name resolves through the namespace and lands in
    /// the used set; the directive face carries the shown list the
    /// combinator semantics will attribute against.
    #[test]
    fn face_records_an_import_shown_use() {
        let fixture = TwoLib::build(
            "import 'b.dart' show x; void f() { x(); }",
            "void x() {}",
            |strings, target| ImportEdge {
                target: Some(target),
                prefix: None,
                deferred: false,
                combinators: Combinators::new(
                    [CombinatorClause::Show(
                        vec![strings.intern("x")].into_boxed_slice(),
                    )],
                    strings,
                ),
            },
        );
        let (face_a, _) = fixture.faces();
        let x = fixture.name("x");
        assert!(face_a.used.contains(&x), "{face_a:?}");
        assert_eq!(face_a.imports.len(), 1);
        assert_eq!(face_a.imports[0].prefix, None);
        assert_eq!(face_a.imports[0].shown.as_ref(), &[x]);
    }

    /// A resolved annotation is a use of the annotation name (the
    /// official `addElement` on metadata resolution).
    #[test]
    fn face_records_an_annotation_use() {
        let (face, strings) = sole_face("class Foo { const Foo(); } @Foo() class C {}");
        let foo = strings.intern("Foo");
        assert!(face.annotations.contains(&foo), "{face:?}");
    }

    /// An export's show names join the re-export record; the directive's
    /// names never enter the unit's used set (they name the exported
    /// library's declarations).
    #[test]
    fn face_records_export_shown_names() {
        let (face, strings) = sole_face("export 'other.dart' show Ex; void f() {}");
        let ex = strings.intern("Ex");
        assert_eq!(face.exports.len(), 1, "{face:?}");
        assert_eq!(face.exports[0].shown, vec![ex]);
        assert!(!face.used.contains(&ex));
    }

    /// The two-library close: A imports B and uses B's `shared`; A's
    /// consumed set contains it and every A-only use, B's contains only
    /// what B's own units used — the folds never cross libraries.
    #[test]
    fn close_keeps_the_consumed_sets_per_library() {
        let fixture = TwoLib::build(
            "import 'b.dart'; void f() { shared(); aOnly(); } void aOnly() {}",
            "void shared() {} void g() { shared(); }",
            |_strings, target| ImportEdge {
                target: Some(target),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let (face_a, face_b) = fixture.faces();
        let mut usages = LibraryUsages::default();
        usages.add_unit(fixture.lib_a, face_a);
        usages.add_unit(fixture.lib_b, face_b);
        let set_a = usages.close_library_usage(fixture.lib_a);
        let set_b = usages.close_library_usage(fixture.lib_b);
        let (shared, a_only, f, g) = (
            fixture.name("shared"),
            fixture.name("aOnly"),
            fixture.name("f"),
            fixture.name("g"),
        );
        // A used B's `shared` through the import and its own `aOnly`.
        assert!(set_a.used(shared) && set_a.used(a_only), "{set_a:?}");
        // B's own unit used `shared` (in `g`) but none of A's names.
        assert!(set_b.used(shared), "{set_b:?}");
        assert!(!set_b.used(a_only) && !set_b.used(f) && !set_b.used(g));
    }

    /// The library closure: two units of ONE library fold into one
    /// consumed set — a use in the first spares a declaration in the
    /// second (the official merge's whole point), the read-shaped
    /// subset stays distinct, annotations join the used set, and the
    /// close releases the faces (a second close is empty).
    #[test]
    fn close_unions_one_librarys_unit_faces_and_releases_them() {
        let strings = StringTable::new();
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib = builder.intern("package:t/main.dart");
        let _graph = builder.finish();
        let (member, annotate, write_only) = (
            strings.intern("member"),
            strings.intern("Anno"),
            strings.intern("writtenOnly"),
        );
        let mut face_one = UnitUseFace::default();
        face_one.used.insert(member);
        let mut face_two = UnitUseFace::default();
        face_two.reads.insert(member);
        face_two.used.insert(write_only);
        face_two.annotations.insert(annotate);
        let mut usages = LibraryUsages::default();
        usages.add_unit(lib, face_one);
        usages.add_unit(lib, face_two);
        let set = usages.close_library_usage(lib);
        assert!(
            set.used(member) && set.used(write_only) && set.used(annotate),
            "{set:?}"
        );
        // The read-only rule's subset: `member` was read, `writtenOnly`
        // was not (a write never spares a field).
        assert!(set.read(member) && !set.read(write_only));
        // The close consumed the faces.
        let drained = usages.close_library_usage(lib);
        assert!(
            drained.used.is_empty() && drained.reads.is_empty(),
            "{drained:?}"
        );
    }
}
