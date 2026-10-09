//! Per-library member enumeration over the resolution layer (#117): the
//! library scope face, the in-library scoped lookup, and the name-level
//! member walk — the query surface the `undefined_*`/`wrong_number_*`
//! diagnostic tickets consume.
//!
//! # The library face (adjudicated essential #1)
//!
//! [`LibraryScopes`] materializes, per library, the public name face with a
//! micro-kind tag: local declarations and re-exports from the graph's export
//! scope (a declaration is identified by `Decl(file)` + looked-up name —
//! #117's contract; the kind comes from the declaring file's
//! [`FileElements`]), plus one `prefix` marker per import prefix. A name
//! declared outside the indexed files (`dart:` SDK, an unanalyzed
//! dependency) surfaces as [`ScopeKind::External`] — present, but
//! kind-sensitive diagnostics must be suppressed.
//!
//! # In-library scope (the private-name face)
//!
//! The export scope only carries public names, but library-internal code
//! sees private ones too. [`lookup_scoped`] is the in-library chain:
//! the current file's own declarations first (private included, locals beat
//! imports), then the [`Namespaces`] lookup (import face, build-time
//! combinator filtering, ambiguity with all candidates).
//!
//! # Member walk (adjudicated essentials #2–#3)
//!
//! [`member_lookup`] resolves a container by name (file-local first, then
//! the namespace) and walks superclass name edges, returning the first hit.
//! A miss is **not** proof of undefined: the caller must still check the
//! extension pool ([`FileElements::extension_member`]) of the library's
//! files before reporting an instance-scope `undefined_*`.
//!
//! # The part/part-of seam
//!
//! The parser consumes directives diet-style and emits no `part` events
//! yet, so this layer cannot assign files to libraries; callers pass the
//! owning library and current file explicitly, and **sibling-part private
//! names are not merged** (a part file's in-library lookup sees only its
//! own declarations plus the public face). The `part`/`part-of` family
//! codes ([`dartq_resolution::ResolutionCode`] `PartOf*`) are frozen in
//! the resolution layer for the diagnostic tickets; when the directive
//! milestone lands, the file→library fold moves behind this same API.

use dartq_resolution::{LibraryGraph, LibraryId, Lookup, NamespaceEntry, Namespaces};
use dartq_source::{FileId, StringId};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::model::{Container, FileElements, MemberEntry, MicroKind, TopDecl};

/// Per-file element models, keyed by file. Sized by the analyzed file set.
#[derive(Debug, Default)]
pub struct ElementIndex {
    files: FxHashMap<FileId, FileElements>,
}

impl ElementIndex {
    /// Stores (or replaces, on re-analysis) the model of `file`.
    pub fn insert(&mut self, file: FileId, elements: FileElements) {
        self.files.insert(file, elements);
    }

    /// The model of `file`, if analyzed.
    #[must_use]
    pub fn get(&self, file: FileId) -> Option<&FileElements> {
        self.files.get(&file)
    }
}

/// The kind tag of a name in a library face: a known [`MicroKind`], or
/// [`ScopeKind::External`] when the declaring file is outside the indexed
/// set — present, but kind-sensitive diagnostics must be suppressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeKind {
    /// Declared in an indexed file; the kind is known.
    Kind(MicroKind),
    /// Declared outside the indexed files (`dart:` SDK, unanalyzed
    /// dependency) — presence is certain, kind is unknown.
    External,
}

/// The public name face of one library: name → kind tag.
#[derive(Debug, Default)]
pub struct ScopeFace {
    names: FxHashMap<StringId, ScopeKind>,
}

impl ScopeFace {
    /// The kind tag of `name`, if the face carries it.
    #[must_use]
    pub fn kind(&self, name: StringId) -> Option<ScopeKind> {
        self.names.get(&name).copied()
    }

    /// The number of names in the face.
    #[must_use]
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether the face is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Every name with its tag, in arbitrary order.
    pub fn iter(&self) -> impl Iterator<Item = (StringId, ScopeKind)> + '_ {
        self.names.iter().map(|(name, kind)| (*name, *kind))
    }
}

/// The public faces of the analyzed libraries, indexed by
/// [`LibraryId::index`]; libraries not included in [`LibraryScopes::build`]
/// have no face. Sized by the graph; nothing grows after build.
#[derive(Debug, Default)]
pub struct LibraryScopes {
    scopes: Vec<Option<ScopeFace>>,
}

impl LibraryScopes {
    /// Builds the faces of `libs` over `graph` and `index`. The graph
    /// already folded each library's part tree into its local declarations
    /// (the caller declared every file), so a face needs no part information
    /// here.
    #[must_use]
    pub fn build(graph: &LibraryGraph, index: &ElementIndex, libs: &[LibraryId]) -> Self {
        let mut scopes: Vec<Option<ScopeFace>> = (0..graph.len()).map(|_| None).collect();
        for &lib in libs {
            let mut face = ScopeFace::default();
            for (name, entry) in graph.export_scope(lib) {
                let kind = index
                    .get(entry.file)
                    .and_then(|file| file.decl(name))
                    .map_or(ScopeKind::External, |decl| ScopeKind::Kind(decl.kind));
                face.names.insert(name, kind);
            }
            for edge in graph.import_edges(lib) {
                if let Some(prefix) = edge.prefix {
                    face.names
                        .entry(prefix)
                        .or_insert(ScopeKind::Kind(MicroKind::Prefix));
                }
            }
            scopes[lib.index() as usize] = Some(face);
        }
        Self { scopes }
    }

    /// The face of `lib`, if built.
    #[must_use]
    pub fn face(&self, lib: LibraryId) -> Option<&ScopeFace> {
        self.scopes
            .get(lib.index() as usize)
            .and_then(|s| s.as_ref())
    }
}

/// The outcome of an in-library scoped lookup ([`lookup_scoped`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopedHit<'a> {
    /// Not defined in the current file nor the import face.
    NotFound,
    /// Declared in the current file itself (private names included; the
    /// local declaration shadows imports).
    Local(&'a TopDecl),
    /// Resolved through the library namespace to one target.
    Found(NamespaceEntry),
    /// Ambiguous between distinct targets — reportable as `ambiguous_import`
    /// at the use point, with every candidate kept.
    Ambiguous(&'a [NamespaceEntry]),
}

/// The in-library scope chain for `name`: the current file's declarations
/// (private included), then the library's namespace. Use this for
/// diagnostics **inside** the library; the public face
/// ([`LibraryScopes`]) answers import-side checks.
///
/// The part seam applies: sibling-part private names are not visible until
/// the directive milestone lands (module docs).
#[must_use]
pub fn lookup_scoped<'a>(
    graph: &LibraryGraph,
    namespaces: &'a Namespaces,
    index: &'a ElementIndex,
    lib: LibraryId,
    file: FileId,
    name: StringId,
) -> ScopedHit<'a> {
    if let Some(decl) = index.get(file).and_then(|f| f.decl(name)) {
        return ScopedHit::Local(decl);
    }
    match namespaces.lookup(graph, lib, name) {
        Lookup::Found(entry) => ScopedHit::Found(entry),
        Lookup::Ambiguous(candidates) => ScopedHit::Ambiguous(candidates),
        Lookup::NotFound => ScopedHit::NotFound,
    }
}

/// Resolves a container by name: the file's own declarations first, then the
/// library namespace. Returns the container and the file that owns it (for
/// the superclass walk's next hop).
///
/// Crate-visible so the diagnostic engine's member walk can tell a cleanly
/// walked chain (member genuinely absent) from a broken one (a superclass
/// name that resolves to nothing — suppression territory, never a report).
pub(crate) fn resolve_container<'a>(
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &'a ElementIndex,
    lib: LibraryId,
    file: FileId,
    name: StringId,
) -> Option<(&'a Container, FileId)> {
    if let Some(container) = index.get(file).and_then(|f| f.container(name)) {
        return Some((container, file));
    }
    match namespaces.lookup(graph, lib, name) {
        Lookup::Found(NamespaceEntry::Decl(owner)) => {
            let container = index.get(owner).and_then(|f| f.container(name))?;
            Some((container, owner))
        }
        _ => None,
    }
}

/// Looks up `member` on `container`'s instance/static face: the container's
/// member set, then direct-superclass name edges (each hop resolved
/// file-locally first, then through the namespace; cycle-guarded).
///
/// A `None` here is **not** proof of undefined — before reporting an
/// instance-scope `undefined_*`, check the extension pool
/// ([`FileElements::extension_member`]) of the library's files (the caller
/// owns the file set; see the part seam in the module docs).
#[must_use]
pub fn member_lookup<'a>(
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &'a ElementIndex,
    lib: LibraryId,
    file: FileId,
    container: StringId,
    member: StringId,
) -> Option<&'a MemberEntry> {
    let mut name = container;
    let mut seen: FxHashSet<StringId> = FxHashSet::default();
    let mut owner = file;
    loop {
        if !seen.insert(name) {
            return None; // malformed cyclic chain: treat as unresolved
        }
        let (found, found_owner) = resolve_container(graph, namespaces, index, lib, owner, name)?;
        if let Some(entry) = found.members.get(&member) {
            return Some(entry);
        }
        name = found.superclass?;
        owner = found_owner;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::collect;
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_parser::ParserFeatures;
    use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder};
    use dartq_scanner::{ScanConfig, TokenTable};
    use dartq_source::{FileTable, StringTable};
    use std::path::Path;

    /// Scans + collects one unit, interning its names into `strings`.
    fn unit(
        source: &str,
        strings: &StringTable,
        files: &mut FileTable,
        path: &str,
    ) -> FileElements {
        let file = files.intern(Path::new(path));
        let result = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(result.diagnostics.is_empty(), "{source:?}");
        let tokens: &'static TokenTable = Box::leak(Box::new(result.tokens));
        let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
            LanguageVersion::new(3, 15),
        ));
        collect(source, tokens, features, strings)
    }

    /// Two-file library merge through #117: a's public declarations are
    /// visible in b's face with their origin kinds; the prefix is tagged;
    /// b's locals shadow the import; a's private name stays invisible.
    #[test]
    fn library_merge_through_namespaces() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let elements_a = unit(
            "class A {} \
             void fa({int? n}) {} \
             int _private = 0;",
            &strings,
            &mut files,
            "a.dart",
        );
        let elements_b = unit(
            "import 'a.dart'; \
             import 'a.dart' as p; \
             class A {} \
             class B {}",
            &strings,
            &mut files,
            "b.dart",
        );
        let file_a = files.try_file_id(Path::new("a.dart")).unwrap();
        let file_b = files.try_file_id(Path::new("b.dart")).unwrap();

        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib_a = builder.intern("package:t/a.dart");
        let lib_b = builder.intern("package:t/b.dart");
        let mut index = ElementIndex::default();
        for (lib, file, elements) in [(lib_a, file_a, &elements_a), (lib_b, file_b, &elements_b)] {
            index.insert(file, elements.clone());
            for (name, decl) in elements.declarations() {
                if decl.kind == MicroKind::TypeParam {
                    continue;
                }
                builder.declare(lib, name, file);
            }
        }
        builder.import(
            lib_b,
            ImportEdge {
                target: Some(lib_a),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        builder.import(
            lib_b,
            ImportEdge {
                target: Some(lib_a),
                prefix: Some(strings.intern("p")),
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let scopes = LibraryScopes::build(&graph, &index, &[lib_a, lib_b]);

        let kind = |lib: LibraryId, name: &str| {
            scopes
                .face(lib)
                .and_then(|face| face.kind(strings.intern(name)))
        };
        assert_eq!(
            kind(lib_b, "A"),
            Some(ScopeKind::Kind(MicroKind::Class)),
            "b's local class A wins the face seeding"
        );
        assert_eq!(
            kind(lib_b, "fa"),
            Some(ScopeKind::Kind(MicroKind::TopLevelFn))
        );
        assert_eq!(kind(lib_b, "p"), Some(ScopeKind::Kind(MicroKind::Prefix)));
        assert_eq!(kind(lib_b, "nope"), None);
        // Private names never reach the public face.
        assert_eq!(kind(lib_a, "_private"), None);

        // In-library scoped lookup: locals (incl. private) then imports.
        let hit = |file: FileId, lib: LibraryId, name: &str| {
            lookup_scoped(&graph, &namespaces, &index, lib, file, strings.intern(name))
        };
        assert_eq!(
            hit(file_b, lib_b, "A"),
            ScopedHit::Local(
                index
                    .get(file_b)
                    .unwrap()
                    .decl(strings.intern("A"))
                    .unwrap()
            ),
            "b's local A shadows the import"
        );
        assert_eq!(
            hit(file_b, lib_b, "fa"),
            ScopedHit::Found(NamespaceEntry::Decl(file_a))
        );
        assert!(matches!(
            hit(file_b, lib_b, "_private"),
            ScopedHit::NotFound
        ));
        assert!(matches!(
            hit(file_a, lib_a, "_private"),
            ScopedHit::Local(_)
        ));
    }

    /// Member walk: superclass edges resolve across files through the
    /// namespace; a miss falls to the extension-pool recipe.
    #[test]
    fn member_walk_crosses_superclass_edges() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let base = unit(
            "class B { void m() {} int get g => 1; } \
             extension on B { int get extG => 1; }",
            &strings,
            &mut files,
            "base.dart",
        );
        let derived = unit("class C extends B {}", &strings, &mut files, "use.dart");
        let base_file = files.try_file_id(Path::new("base.dart")).unwrap();
        let use_file = files.try_file_id(Path::new("use.dart")).unwrap();

        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib = builder.intern("package:t/lib.dart");
        for (file, elements) in [(base_file, &base), (use_file, &derived)] {
            for (name, decl) in elements.declarations() {
                if decl.kind != MicroKind::TypeParam {
                    builder.declare(lib, name, file);
                }
            }
        }
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let mut index = ElementIndex::default();
        index.insert(base_file, base.clone());
        index.insert(use_file, derived);

        let m = member_lookup(
            &graph,
            &namespaces,
            &index,
            lib,
            use_file,
            strings.intern("C"),
            strings.intern("m"),
        )
        .expect("inherited through the superclass edge");
        assert_eq!(m.kind, crate::model::MemberKind::Method);
        assert!(
            member_lookup(
                &graph,
                &namespaces,
                &index,
                lib,
                use_file,
                strings.intern("C"),
                strings.intern("g")
            )
            .is_some()
        );
        // The miss that must consult the extension pool before reporting.
        assert!(
            member_lookup(
                &graph,
                &namespaces,
                &index,
                lib,
                use_file,
                strings.intern("C"),
                strings.intern("extG")
            )
            .is_none()
        );
        assert!(
            index
                .get(base_file)
                .unwrap()
                .extension_member(strings.intern("extG"))
                .is_some()
        );
    }
}
