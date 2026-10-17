//! The explain report's clause-typed supertype split and its constructor
//! section: `inheritance` carries the `extends` edges while `interfaces`
//! carries `with`/`on`/`implements` (each row naming its clause), and
//! `constructors` places the caller-rendered facts — with the `minimal`
//! tier's counts mirroring the row builders exactly, and the empty-face
//! degradation answering present, empty arrays (never placeholders).

use std::path::Path;

use dartq_ai::explain::{self as query, ConstructorFact, SuperTypeRow, SymbolDeclHit, Verbosity};
use dartq_ai::rev_import_graph::RevImportGraph;
use dartq_index::{DeclKind, DeclRow, SuperClause, TargetRef, TestGraph};
use dartq_source::{FileId, FileTable, SourceRange, StringId, StringTable};

/// One file's workspace: `a.dart` declares `Bar` and `Foo` (the queried
/// class — source-wise `class Foo extends Bar with M implements I, J`),
/// `b.dart` declares the mixin `M`. Ids: a=0, b=1, t=2.
struct Fixture {
    strings: StringTable,
    bar: StringId,
    mixin_m: StringId,
    file_a: FileId,
    file_b: FileId,
    surface_a: Vec<DeclRow>,
    surface_b: Vec<DeclRow>,
}

fn row(name: StringId, kind: DeclKind, offset: u32, container: u32) -> DeclRow {
    DeclRow {
        name,
        kind,
        unnamed_extension_member: false,
        range: SourceRange { offset, length: 3 },
        container,
        decl_end: DeclRow::UNKNOWN,
        body_open: DeclRow::UNKNOWN,
    }
}

fn fixture() -> Fixture {
    let strings = StringTable::new();
    let id = |text: &str| strings.intern(text);
    let (bar, mixin_m) = (id("Bar"), id("M"));
    let foo = id("Foo");
    let mut files = FileTable::new();
    let file_a = files.intern(Path::new("a.dart"));
    let file_b = files.intern(Path::new("b.dart"));
    // Capture order: Bar, Foo, then Foo's members — the member anchors
    // point at Foo's row index 1. The constructors ride their own section,
    // fed by the caller's facts face — the capture mints no rows for them.
    let surface_a = vec![
        row(bar, DeclKind::Class, 0, DeclRow::NONE),
        row(foo, DeclKind::Class, 60, DeclRow::NONE),
        row(id("label"), DeclKind::Field, 120, 1),
        row(id("area"), DeclKind::Method, 150, 1),
    ];
    let surface_b = vec![row(mixin_m, DeclKind::Mixin, 0, DeclRow::NONE)];
    Fixture {
        strings,
        bar,
        mixin_m,
        file_a,
        file_b,
        surface_a,
        surface_b,
    }
}

fn labels(lib: u32) -> &'static str {
    [
        "package:t/a.dart",
        "package:t/b.dart",
        "package:t/a_test.dart",
    ][lib as usize]
}

fn file_labels(file: FileId) -> &'static str {
    ["a.dart", "b.dart"][file.index() as usize]
}

/// The graph faces over the three-library id space: `b` and the test
/// import `a`.
fn graphs() -> (RevImportGraph, TestGraph) {
    let imports = |lib: u32| -> &'static [u32] {
        match lib {
            1..=2 => &[0],
            _ => &[],
        }
    };
    let importers = RevImportGraph::build(3, imports);
    let tests = TestGraph::build(3, imports, |lib| {
        Some(labels(lib).rsplit('/').next().unwrap_or("x"))
    });
    (importers, tests)
}

fn hits(fx: &Fixture) -> Vec<SymbolDeclHit<'_>> {
    vec![SymbolDeclHit {
        library: 0,
        file: fx.file_a,
        decl: &fx.surface_a[1],
    }]
}

/// The clause-typed supertype face for `class Foo extends Bar with M
/// implements I, J`: the extends target resolves in `a.dart`, the
/// with-clause mixin in `b.dart`, and the two implements names stay bare
/// (outside the analyzed set).
fn supertypes(fx: &Fixture) -> Vec<(TargetRef, SuperClause)> {
    vec![
        (TargetRef::Decl(fx.file_a, fx.bar), SuperClause::Extends),
        (TargetRef::Decl(fx.file_b, fx.mixin_m), SuperClause::With),
        (
            TargetRef::Name(fx.strings.intern("I")),
            SuperClause::Implements,
        ),
        (
            TargetRef::Name(fx.strings.intern("J")),
            SuperClause::Implements,
        ),
    ]
}

/// The constructor facts the wiring renders for the queried container:
/// the unnamed face under `Foo` plus one named selector — deliberately
/// unsorted input, the builder's name sort is the pin.
fn ctors() -> Vec<ConstructorFact> {
    vec![
        ConstructorFact {
            name: "Foo.named".to_owned(),
            signature: "Foo.named(bool flag)".to_owned(),
        },
        ConstructorFact {
            name: "Foo".to_owned(),
            signature: "Foo()".to_owned(),
        },
    ]
}

fn explain_foo(
    fx: &Fixture,
    verbosity: Verbosity,
    supers: &[(TargetRef, SuperClause)],
    ctors: &[ConstructorFact],
    importers: &RevImportGraph,
    tests: &TestGraph,
) -> query::ExplainReport {
    query::explain(
        "Foo",
        verbosity,
        &hits(fx),
        &[],
        |file| match file.index() {
            0 => &fx.surface_a,
            _ => &fx.surface_b,
        },
        |_, _| supers,
        |_, _| ctors,
        importers,
        labels,
        file_labels,
        tests,
        &fx.strings,
    )
}

/// A class whose header carries every clause kind: only the `extends`
/// edges ride `inheritance`, the rest land in `interfaces` under their
/// clause spelling (sorted by name, then file; bare names keep no file),
/// and the constructor facts ride their own section under the section's
/// kind — `methods` carries the members the capture mints, untouched.
#[test]
fn mixed_clause_edges_split_by_clause_and_constructors_fill() {
    let fx = fixture();
    let supers = supertypes(&fx);
    let ctors = ctors();
    let (importers, tests) = graphs();
    let report = explain_foo(&fx, Verbosity::Compact, &supers, &ctors, &importers, &tests);
    assert_eq!(
        report.inheritance,
        Some(vec![SuperTypeRow {
            name: "Bar".to_owned(),
            file: Some("a.dart".to_owned()),
            clause: Some("extends".to_owned()),
        }]),
        "only the extends edges ride inheritance"
    );
    assert_eq!(
        report.interfaces,
        Some(vec![
            SuperTypeRow {
                name: "I".to_owned(),
                file: None,
                clause: Some("implements".to_owned()),
            },
            SuperTypeRow {
                name: "J".to_owned(),
                file: None,
                clause: Some("implements".to_owned()),
            },
            SuperTypeRow {
                name: "M".to_owned(),
                file: Some("b.dart".to_owned()),
                clause: Some("with".to_owned()),
            },
        ]),
        "the with/implements edges, clause-named, sorted by (name, file)"
    );
    assert_eq!(
        report.constructors,
        Some(vec![
            query::ConstructorRow {
                name: "Foo".to_owned(),
                kind: "constructor".to_owned(),
                signature: "Foo()".to_owned(),
            },
            query::ConstructorRow {
                name: "Foo.named".to_owned(),
                kind: "constructor".to_owned(),
                signature: "Foo.named(bool flag)".to_owned(),
            },
        ]),
        "the unnamed face under the container's own name, the selector under A.named"
    );
    assert_eq!(
        report
            .methods
            .as_ref()
            .expect("methods")
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        vec!["area", "label"],
        "the member sections the capture can fill stay untouched"
    );

    let value = serde_json::to_value(&report).expect("serialize");
    assert_eq!(
        value["inheritance"][0]["clause"],
        serde_json::json!("extends"),
        "the clause rides the serialized row"
    );
    assert_eq!(
        value["constructors"],
        serde_json::json!([
            {"name": "Foo", "kind": "constructor", "signature": "Foo()"},
            {"name": "Foo.named", "kind": "constructor", "signature": "Foo.named(bool flag)"},
        ])
    );
}

/// The `minimal` tier's counts mirror the row builders exactly: the
/// clause split counts each section's edges, the constructor count is the
/// facts face's length.
#[test]
fn minimal_counts_mirror_the_filled_sections() {
    let fx = fixture();
    let supers = supertypes(&fx);
    let ctors = ctors();
    let (importers, tests) = graphs();
    let report = explain_foo(&fx, Verbosity::Minimal, &supers, &ctors, &importers, &tests);
    assert_eq!(
        report.counts,
        Some(query::SectionCounts {
            inheritance: 1,
            interfaces: 3,
            constructors: 2,
            methods: 2,
            references: 0,
            dependencies: 2,
            affected_tests: 1,
        })
    );
}

/// The no-clause, no-facts edge: a plain `class Foo {}` (no
/// extends/with/implements, constructors aside) answers all three sections
/// empty — nothing leaks from the member surface into the supertype
/// sections, and nothing appears where the facts face has no rows.
#[test]
fn plain_class_answers_the_sections_empty() {
    let fx = fixture();
    let (importers, tests) = graphs();
    let report = explain_foo(&fx, Verbosity::Compact, &[], &[], &importers, &tests);
    assert_eq!(report.inheritance, Some(Vec::new()), "no edges, no rows");
    assert_eq!(report.interfaces, Some(Vec::new()));
    assert_eq!(report.constructors, Some(Vec::new()));
    assert_eq!(report.methods.as_ref().map(Vec::len), Some(2));
}
