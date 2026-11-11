//! #3987 pins: the resolved supertype rows order canonically — the
//! subtype container's `(name bytes, offset, clause)` rank, exactly the
//! order the capture's byte-canonical declaration table gives its
//! containers — so one source builds the same `supers` table whatever
//! order its names were interned in. Interned ids follow a run's
//! file-processing order and the engine discovers files in parallel, so
//! the id-keyed order these pins replaced flipped the export's
//! per-document supertype rows run to run (`Zed`'s rows before `Alpha`'s
//! in one run, after them in the next). The row order is the export
//! face's document order (the stream filters the stored slice by file
//! and keeps the stored order); the id-keyed probe face
//! [`LibraryIndex::supertypes_of`] reads the run directory instead, so
//! it keeps its one-binary-search shape.

use crate::capture::{SuperClause, capture};
use crate::{LibraryBuilder, LibraryIndex, SubtypeEdge, TargetRef};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{LibraryGraphBuilder, Namespaces};
use dartq_scanner::{ScanConfig, scan};
use dartq_source::{FileId, FileTable, StringTable};
use std::path::Path;

/// The flip fixture: TWO subtype containers whose source order disagrees
/// with their names' byte order (`Zed` is written first, `'A' < 'Z'`), each
/// carrying two clauses so the within-container clause order is pinned too.
/// No declarations for `Base`/`Iface` — the supertypes stay bare-name
/// targets, keeping the fixture to one file and one library.
const SOURCE: &str = "\
class Zed extends Base implements Iface {}
class Alpha extends Base implements Iface {}
";

/// The names the fixture's parse interns (plus noise the rotations spread
/// over), in parse order — the stand-in for one run's file-processing
/// order.
const NAMES: [&str; 7] = ["Zed", "Base", "Iface", "Alpha", "main", "helper", "top"];

/// The resolved spelling of a supertype target — both target shapes carry
/// the name the clause wrote.
fn target_spelling(target: TargetRef, strings: &StringTable) -> String {
    let name = match target {
        TargetRef::Decl(_, name) | TargetRef::Name(name) => name,
    };
    strings.resolve(name).to_owned()
}

/// Builds the single-file library's surface over a fresh table whose names
/// were pre-interned in `intern_order`: capture, graph, namespaces, and
/// the library fold — the same chain the engine drives.
fn build_surface(source: &str, intern_order: &[&str]) -> (LibraryIndex, StringTable, FileId) {
    let strings = StringTable::new();
    for name in intern_order {
        // The pre-intern mints ids in the given order; the ids themselves
        // are the point, the return value is not.
        let _ = strings.intern(name);
    }
    let mut files = FileTable::new();
    let file = files.intern(Path::new("ticket_3987.dart"));
    let result = scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(
        result.diagnostics.is_empty(),
        "scanner diagnostics: {result:?}"
    );
    let features = dartq_parser::ParserFeatures::from_feature_set(
        FeatureSet::for_language_version(LanguageVersion::new(3, 15)),
    );
    let captured = capture(file, source, &result.tokens, features, &strings);

    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/ticket_3987.dart");
    for row in captured.index.decls() {
        if row.is_library_face() {
            builder.declare(lib, row.name, file);
        }
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);

    let mut builder = LibraryBuilder::new(lib, &graph, &namespaces);
    builder.add_file(&captured);
    (builder.finish(), strings, file)
}

/// The export-visible row surface: every field the export stream's
/// supertype rows read (the subtype's spelling, the clause, the
/// supertype's spelling), in stored order — the sequence the per-document
/// assembly emits, with names resolved through the run's own table
/// (interned ids are per-table encodings and legitimately differ run to
/// run; the resolved surface must not).
fn super_surface(
    index: &LibraryIndex,
    strings: &StringTable,
) -> Vec<(String, SuperClause, String)> {
    index
        .supers()
        .iter()
        .map(|edge| {
            (
                strings.resolve(edge.name).to_owned(),
                edge.clause,
                target_spelling(edge.supertype, strings),
            )
        })
        .collect()
}

/// Twelve builds of the same source, each over a fresh table whose names
/// were interned in a rotated order — the stand-in for the engine's
/// per-run parallel file-processing order. Every run must hold the
/// identical resolved supertype surface: the row order keys on the
/// subtype names' bytes (via the capture's canonical container rank),
/// never on the interned ids that name them (#3987's `Zed`/`Alpha`
/// flip).
#[test]
fn super_rows_are_identical_across_intern_orders() {
    const RUNS: usize = 12;

    let mut baseline: Option<Vec<(String, SuperClause, String)>> = None;
    for run in 0..RUNS {
        // `run` rotates the intern order: run 0 keeps the fixture's parse
        // order (`Zed` first), run 1 leads with `Base`, run 3 leads with
        // `Alpha` (the byte-canonical leader interned first), later runs
        // spread over the noise names.
        let rotated: Vec<&str> = (0..NAMES.len())
            .map(|i| NAMES[(i + run) % NAMES.len()])
            .collect();
        let (index, strings, _file) = build_surface(SOURCE, &rotated);
        let surface = super_surface(&index, &strings);
        match &baseline {
            None => baseline = Some(surface),
            Some(expected) => assert_eq!(
                &surface, expected,
                "run {run} (intern order {rotated:?}): supertype rows must \
                 not follow the intern order"
            ),
        }
    }
    let canonical: Vec<(String, SuperClause, String)> = [
        ("Alpha", SuperClause::Extends, "Base"),
        ("Alpha", SuperClause::Implements, "Iface"),
        ("Zed", SuperClause::Extends, "Base"),
        ("Zed", SuperClause::Implements, "Iface"),
    ]
    .iter()
    .map(|&(name, clause, target)| (name.to_owned(), clause, target.to_owned()))
    .collect();
    assert_eq!(
        baseline.expect("run 0 set the baseline"),
        canonical,
        "byte-canonical: Alpha's rows before Zed's, clause-typed within"
    );
}

/// The stored order is the subtype names' byte order, not the interned-id
/// order: the fixture writes `Zed` before `Alpha`, so parse-order
/// interning mints `id(Zed) < id(Alpha)` while the bytes say `'A' < 'Z'`
/// — an id-keyed sort places `Zed`'s rows first, the canonical sort
/// places `Alpha`'s first. The probe face keeps answering by id through
/// the run directory: both containers' probes return their clause-typed
/// edges whatever order the rows landed in.
#[test]
fn super_rows_follow_name_bytes_not_intern_order() {
    // Parse order, no pre-intern: the discrimination premise.
    let (index, strings, file) = build_surface(SOURCE, &[]);
    assert!(
        strings.intern("Zed") < strings.intern("Alpha"),
        "premise: parse order mints id(Zed) < id(Alpha)"
    );

    let canonical: Vec<(String, SuperClause, String)> = [
        ("Alpha", SuperClause::Extends, "Base"),
        ("Alpha", SuperClause::Implements, "Iface"),
        ("Zed", SuperClause::Extends, "Base"),
        ("Zed", SuperClause::Implements, "Iface"),
    ]
    .iter()
    .map(|&(name, clause, target)| (name.to_owned(), clause, target.to_owned()))
    .collect();
    assert_eq!(
        super_surface(&index, &strings),
        canonical,
        "byte-canonical: Alpha's rows before Zed's, clause-typed within"
    );

    // The probe face: one search per container returns exactly its edges,
    // clause-ascending, whatever row slot the canonical order gave them.
    for name in ["Alpha", "Zed"] {
        let edges: &[SubtypeEdge] = index
            .supertypes_of(file, strings.intern(name))
            .unwrap_or(&[]);
        assert_eq!(edges.len(), 2, "{name}: both clauses");
        let resolved: Vec<(String, SuperClause, String)> = edges
            .iter()
            .map(|edge| {
                (
                    strings.resolve(edge.name).to_owned(),
                    edge.clause,
                    target_spelling(edge.supertype, &strings),
                )
            })
            .collect();
        let expected: Vec<(String, SuperClause, String)> = [
            (SuperClause::Extends, "Base"),
            (SuperClause::Implements, "Iface"),
        ]
        .iter()
        .map(|&(clause, target)| (name.to_owned(), clause, target.to_owned()))
        .collect();
        assert_eq!(
            resolved, expected,
            "{name}: the probe serves its canonical block, clause-typed"
        );
    }
    assert!(
        index.supertypes_of(file, strings.intern("Base")).is_none(),
        "a clause name is no subtype container"
    );
}
