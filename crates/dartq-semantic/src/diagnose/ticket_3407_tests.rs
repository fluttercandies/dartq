//! #3407 pins — the three `undefined_*` cluster faces, pinned at the MODEL
//! layer (`collector.rs` namespace construction + `library.rs` scope
//! composition) rather than at their emission sites.
//!
//! # C6 — an `augment` declaration declares no name
//!
//! Upstream, an `augment` fragment belongs to the element its introduction
//! declares (`element.firstFragment` is the introduction, upstream Dart SDK
//! source, pkg/_fe_analyzer_shared augmentation handling), so the unit that
//! writes an `augment` head introduces no name of its own. The collector used
//! to claim a vacant slot for it, which invented a unit-local declaration for
//! a target declared in a sibling unit — the name then resolved to the
//! augmenting unit's own partial face — and made an `augment` with no
//! introduction at all read as defined.
//!
//! # C9 — the unit namespace carries declarations, never members
//!
//! The bare-identifier domain is the library scope (locals, then the combined
//! import scope). A class or extension body's member spelling (`length`, a
//! static `foo`) lives on the receiver face; letting one into the unit
//! namespace would silence the `undefined_identifier` an unqualified read
//! deserves (upstream `simple_identifier_resolver.dart:216-221` reports
//! whenever the scope chain returns nothing, and the chain never carries
//! another element's members). Type parameters ride the name set as a tagged
//! placeholder precisely so callers can tell them apart from library-visible
//! declarations.
//!
//! # C12 — combinators filter the import face, and a prefix keeps its table
//!
//! `show` keeps only the listed names, `hide` drops them, in written order,
//! with the setter spelling joining its bare name (upstream
//! `summary2/combinator.dart` `Combinator.matches` + `allows`). A prefixed
//! import registers the prefix alone — its target's exports never join the
//! library face — and the prefix's own merged table filters by the same
//! clauses (upstream `named_type_resolver.dart:105-121`, `scope.dart:352-357`).

use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    ClassModifiers, CombinatorClause, Combinators, ImportEdge, LibraryGraph, LibraryGraphBuilder,
    LibraryId, LocalLookup, Lookup, NamespaceEntry, Namespaces,
};
use dartq_scanner::{ScanConfig, TokenTable};
use dartq_source::{FileId, FileTable, StringId, StringTable};
use std::path::Path;

use crate::collector::collect;
use crate::library::{ElementIndex, LibraryScopes, ScopeKind, ScopedHit, lookup_scoped};
use crate::model::{FileElements, MicroKind};

/// Scans + collects one unit, interning its path into `files`.
fn unit(source: &str, strings: &StringTable, files: &mut FileTable, path: &str) -> FileElements {
    let file = files.intern(Path::new(path));
    let scanned = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(scanned.diagnostics.is_empty(), "{source:?}");
    let tokens: &'static TokenTable = Box::leak(Box::new(scanned.tokens));
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    collect(source, tokens, features, strings)
}

/// One assembled fixture: the units, their namespace combinator, and the
/// element index.
struct Fixture {
    strings: StringTable,
    graph: LibraryGraph,
    namespaces: Namespaces,
    index: ElementIndex,
    main_lib: LibraryId,
    main_file: FileId,
    /// The dependency units, each its own library: `(path, library, file)`.
    deps: Vec<(&'static str, LibraryId, FileId)>,
    /// Every unit's model, main unit first.
    units: Vec<(FileId, FileElements)>,
}

impl Fixture {
    /// The id of `text` in the fixture's string table.
    fn id(&self, text: &str) -> StringId {
        self.strings.intern(text)
    }

    /// The `index`-th unit's model (main unit is index 0).
    fn elements(&self, index: usize) -> &FileElements {
        &self.units[index].1
    }

    /// The in-library scoped answer for `text` at the main unit.
    fn scoped(&self, text: &str) -> ScopedHit<'_> {
        lookup_scoped(
            &self.graph,
            &self.namespaces,
            &self.index,
            self.main_lib,
            self.main_file,
            self.id(text),
        )
    }

    /// The file registered for the dependency at `path`.
    fn dep_file(&self, path: &str) -> FileId {
        self.deps
            .iter()
            .find(|(candidate, _, _)| *candidate == path)
            .map(|(_, _, file)| *file)
            .expect("the dependency is part of the fixture")
    }
}

/// Collects `sources` as the units of ONE library, in order, and registers
/// every top-level declaration the graph carries (type-parameter
/// placeholders excluded — they are declaration-scoped, the documented
/// `LibraryGraphBuilder::declare` contract).
fn one_library(sources: &[(&'static str, &str)]) -> Fixture {
    let strings = StringTable::new();
    let mut table = FileTable::new();
    let mut units: Vec<(FileId, FileElements)> = Vec::new();
    for (path, source) in sources {
        let elements = unit(source, &strings, &mut table, path);
        let file = table.try_file_id(Path::new(path)).expect("unit interned");
        units.push((file, elements));
    }
    let mut builder = LibraryGraphBuilder::new(&strings);
    let main_lib = builder.intern("package:t/main.dart");
    let mut index = ElementIndex::default();
    for (file, elements) in &units {
        index.insert(*file, elements.clone());
        builder.unit(main_lib, *file);
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            if matches!(
                decl.kind,
                MicroKind::Class | MicroKind::Enum | MicroKind::Mixin
            ) {
                builder.declare_class(main_lib, name, *file, ClassModifiers::default());
            } else {
                builder.declare(main_lib, name, *file);
            }
        }
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    Fixture {
        strings,
        graph,
        namespaces,
        index,
        main_lib,
        main_file: units[0].0,
        deps: Vec::new(),
        units,
    }
}

/// One written `show`/`hide` clause of a fixture import edge, in bare
/// spellings (interned against the fixture's own table when the edge is
/// built — combinator ids must index the same table the graph does).
enum Clause {
    Show(&'static [&'static str]),
    Hide(&'static [&'static str]),
}

/// Collects `main` plus one library per `deps` entry, wiring `edges` as
/// `main`'s imports.
fn import_library(
    main: &str,
    deps: &[(&'static str, &str)],
    edges: &[(Option<&'static str>, &'static str, &[Clause])],
) -> Fixture {
    let strings = StringTable::new();
    let mut table = FileTable::new();
    let main_elements = unit(main, &strings, &mut table, "main.dart");
    let main_file = table.try_file_id(Path::new("main.dart")).unwrap();
    let mut dep_units: Vec<(&'static str, FileId, FileElements)> = Vec::new();
    for (path, source) in deps {
        let elements = unit(source, &strings, &mut table, path);
        let file = table.try_file_id(Path::new(path)).unwrap();
        dep_units.push((path, file, elements));
    }
    let mut builder = LibraryGraphBuilder::new(&strings);
    let main_lib = builder.intern("package:t/main.dart");
    let mut index = ElementIndex::default();
    let mut units: Vec<(FileId, FileElements)> = Vec::new();
    index.insert(main_file, main_elements.clone());
    builder.unit(main_lib, main_file);
    for (name, decl) in main_elements.declarations() {
        if decl.kind != MicroKind::TypeParam {
            builder.declare(main_lib, name, main_file);
        }
    }
    units.push((main_file, main_elements));
    let mut dep_refs: Vec<(&'static str, LibraryId, FileId)> = Vec::new();
    for (path, file, elements) in dep_units {
        let lib = builder.intern(&format!("package:t/{path}"));
        index.insert(file, elements.clone());
        builder.unit(lib, file);
        for (name, decl) in elements.declarations() {
            if decl.kind != MicroKind::TypeParam {
                builder.declare(lib, name, file);
            }
        }
        dep_refs.push((path, lib, file));
        units.push((file, elements));
    }
    for (prefix, target, clauses) in edges {
        let target_lib = dep_refs
            .iter()
            .find(|(path, _, _)| *path == *target)
            .map(|(_, lib, _)| *lib)
            .expect("the imported unit is part of the fixture");
        let combinators = if clauses.is_empty() {
            Combinators::unfiltered()
        } else {
            Combinators::new(
                clauses.iter().map(|clause| {
                    let intern = |names: &&'static [&'static str]| -> Box<[StringId]> {
                        names
                            .iter()
                            .map(|name| strings.intern(name))
                            .collect::<Vec<_>>()
                            .into_boxed_slice()
                    };
                    match clause {
                        Clause::Show(names) => CombinatorClause::Show(intern(names)),
                        Clause::Hide(names) => CombinatorClause::Hide(intern(names)),
                    }
                }),
                &strings,
            )
        };
        builder.import(
            main_lib,
            ImportEdge {
                target: Some(target_lib),
                prefix: (*prefix).map(|prefix| strings.intern(prefix)),
                deferred: false,
                combinators,
            },
        );
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    Fixture {
        strings,
        graph,
        namespaces,
        index,
        main_lib,
        main_file,
        deps: dep_refs,
        units,
    }
}

/// C6: the augmenting unit introduces no name — the target resolves to the
/// INTRODUCTION's unit through the library scope, not to a phantom local.
#[test]
fn augment_unit_declares_no_name() {
    let fixture = one_library(&[
        ("main.dart", "class C { void m() {} }"),
        (
            "aug.dart",
            "part of 'main.dart';\naugment class C { void n() {} }",
        ),
    ]);
    let intro_file = fixture.main_file;
    let aug = fixture.elements(1);

    // The augment's own model carries no declaration of the target name: the
    // name belongs to the introduction, which lives in another unit here.
    assert!(
        aug.decl(fixture.id("C")).is_none(),
        "an `augment` head must not claim a name in its own unit"
    );
    // Its own member face survives — the augmentation verifier and the
    // merged-element walk read the fragments from here.
    let aug_container = aug
        .container(fixture.id("C"))
        .expect("the augment unit keeps its own container face");
    assert!(
        aug_container.members.get(&fixture.id("n")).is_some(),
        "the augment's own member stays on its face"
    );
    assert!(
        aug_container.members.get(&fixture.id("m")).is_none(),
        "the introduction's member is not duplicated onto the augmenting unit"
    );
    assert_eq!(aug.augmentations().len(), 1, "the augment row is captured");

    // The library scope resolves the name to the introduction's unit, from
    // the augmenting unit's own use site.
    let from_aug = lookup_scoped(
        &fixture.graph,
        &fixture.namespaces,
        &fixture.index,
        fixture.main_lib,
        fixture.units[1].0,
        fixture.id("C"),
    );
    assert_eq!(
        from_aug,
        ScopedHit::Found(NamespaceEntry::Decl(intro_file)),
        "the augmented name resolves to the introducing unit"
    );
    assert!(
        matches!(
            fixture.graph.local(fixture.main_lib, fixture.id("C")),
            LocalLookup::Found(file) if file == intro_file
        ),
        "the graph's locals layer keeps the introduction as the owner"
    );
    assert_eq!(
        fixture.scoped("C"),
        ScopedHit::Local(
            fixture
                .elements(0)
                .decl(fixture.id("C"))
                .expect("the introduction declares the name")
        ),
        "the introducing unit still owns its own declaration"
    );
}

/// C6 negative: an `augment` with no introduction anywhere declares nothing —
/// the name stays undefined instead of being invented by the augment head.
#[test]
fn augment_without_introduction_declares_nothing() {
    let fixture = one_library(&[("main.dart", "augment class Missing { void n() {} }")]);
    let elements = fixture.elements(0);
    assert!(
        elements.decl(fixture.id("Missing")).is_none(),
        "`augment class Missing` introduces no `Missing`"
    );
    assert!(
        elements.container(fixture.id("Missing")).is_some(),
        "the augment's own member face still exists"
    );
    assert!(
        matches!(fixture.scoped("Missing"), ScopedHit::NotFound),
        "a target with no introduction stays undefined at the model layer"
    );
}

/// C9: a member spelling never enters the unit namespace — only declarations
/// do. A bare read of a member name is the receiver face's question.
#[test]
fn member_names_stay_out_of_the_unit_scope() {
    let fixture = one_library(&[(
        "main.dart",
        "class C<T> { int length = 0; static const foo = 'x'; } \
         extension Ext on C<int> { int extLength() => 0; }",
    )]);
    let elements = fixture.elements(0);

    assert!(
        elements.decl(fixture.id("C")).is_some(),
        "the class declares its name"
    );
    for member in ["length", "foo", "extLength"] {
        assert!(
            elements.decl(fixture.id(member)).is_none(),
            "`{member}` is a member spelling, never a unit declaration"
        );
        assert!(
            elements.container(fixture.id(member)).is_none(),
            "`{member}` never walks as a container"
        );
        assert!(
            matches!(fixture.scoped(member), ScopedHit::NotFound),
            "`{member}` resolves through the receiver face, not the unit scope"
        );
    }
    // The type parameter rides the name set TAGGED, so a caller can tell it
    // apart from a library-visible declaration (`lib.rs`'s model contract).
    let type_param = elements
        .decl(fixture.id("T"))
        .expect("the placeholder is carried");
    assert_eq!(type_param.kind, MicroKind::TypeParam);
}

/// C12: `show` keeps only the listed names on the library face, and a local
/// declaration of a filtered-out spelling is unaffected.
#[test]
fn show_combinator_filters_the_import_face() {
    let fixture = import_library(
        "import 'a.dart' show A, B;\nclass Local {}",
        &[("a.dart", "class A {} class B {} class Hidden {}")],
        &[(None, "a.dart", &[Clause::Show(&["A", "B"])])],
    );
    let dep_file = fixture.dep_file("a.dart");
    let scopes = LibraryScopes::build(&fixture.graph, &fixture.index, &[fixture.main_lib]);
    let kind = |text: &str| {
        scopes
            .face(fixture.main_lib)
            .and_then(|face| face.kind(fixture.id(text)))
    };
    assert_eq!(kind("A"), Some(ScopeKind::Kind(MicroKind::Class)));
    assert_eq!(kind("B"), Some(ScopeKind::Kind(MicroKind::Class)));
    assert_eq!(kind("Hidden"), None, "`show` keeps only its listed names");
    assert_eq!(
        kind("Local"),
        Some(ScopeKind::Kind(MicroKind::Class)),
        "a local declaration is never filtered by an import's clauses"
    );
    assert_eq!(
        fixture.scoped("A"),
        ScopedHit::Found(NamespaceEntry::Decl(dep_file))
    );
    assert!(
        matches!(fixture.scoped("Hidden"), ScopedHit::NotFound),
        "a name the `show` clause excludes does not resolve"
    );
}

/// C12 negative direction: `hide` drops exactly its listed names and keeps
/// the rest of the target's face.
#[test]
fn hide_combinator_keeps_the_remaining_face() {
    let fixture = import_library(
        "import 'a.dart' hide A;",
        &[("a.dart", "class A {} class B {} class Hidden {}")],
        &[(None, "a.dart", &[Clause::Hide(&["A"])])],
    );
    let scopes = LibraryScopes::build(&fixture.graph, &fixture.index, &[fixture.main_lib]);
    let kind = |text: &str| {
        scopes
            .face(fixture.main_lib)
            .and_then(|face| face.kind(fixture.id(text)))
    };
    assert_eq!(kind("A"), None, "`hide` drops its listed name");
    assert_eq!(kind("B"), Some(ScopeKind::Kind(MicroKind::Class)));
    assert_eq!(kind("Hidden"), Some(ScopeKind::Kind(MicroKind::Class)));
    assert!(
        matches!(fixture.scoped("A"), ScopedHit::NotFound),
        "the hidden name does not resolve through the bare scope"
    );
}

/// C12 prefix face: a prefixed import registers the prefix alone (its
/// target's exports stay out of the bare scope) and the prefix's own table
/// honours the directive's clauses.
#[test]
fn prefixed_import_registers_the_prefix_and_filters_its_table() {
    let fixture = import_library(
        "import 'a.dart' as p;\nimport 'b.dart' as q show Q;",
        &[
            ("a.dart", "class A {}"),
            ("b.dart", "class Q {} class Other {}"),
        ],
        &[
            (Some("p"), "a.dart", &[]),
            (Some("q"), "b.dart", &[Clause::Show(&["Q"])]),
        ],
    );
    let a_file = fixture.dep_file("a.dart");
    let b_file = fixture.dep_file("b.dart");
    let scopes = LibraryScopes::build(&fixture.graph, &fixture.index, &[fixture.main_lib]);
    let face = scopes
        .face(fixture.main_lib)
        .expect("the face is built");
    let kind = |text: &str| face.kind(fixture.id(text));
    assert_eq!(kind("p"), Some(ScopeKind::Kind(MicroKind::Prefix)));
    assert_eq!(kind("q"), Some(ScopeKind::Kind(MicroKind::Prefix)));
    assert_eq!(
        kind("A"),
        None,
        "a prefixed import never contributes its target's bare names"
    );
    let prefixed = |prefix: &str, name: &str| {
        fixture.namespaces.lookup_prefixed(
            &fixture.graph,
            fixture.main_lib,
            fixture.id(prefix),
            fixture.id(name),
        )
    };
    assert_eq!(
        prefixed("p", "A"),
        Lookup::Found(NamespaceEntry::Decl(a_file))
    );
    assert_eq!(
        prefixed("q", "Q"),
        Lookup::Found(NamespaceEntry::Decl(b_file))
    );
    assert_eq!(
        prefixed("q", "Other"),
        Lookup::NotFound,
        "the prefix's table honours the directive's `show` clause"
    );
    assert!(
        matches!(fixture.scoped("A"), ScopedHit::NotFound),
        "the prefixed target's names are reachable through `p` only"
    );
}
