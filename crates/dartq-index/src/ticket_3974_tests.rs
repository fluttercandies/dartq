//! #3974 pins: the declaration row order is canonical — name bytes, then
//! offset — so one source captures to the same row table regardless of
//! the order its names were interned in. Interned ids follow a run's
//! file-processing order and the engine discovers files in parallel, so
//! the id-keyed order these pins replaced flipped the export stream run
//! to run (`s1=top, s2=Foo` in one run, `s1=Foo, s2=top` in the next).

use crate::capture::{DeclKind, FileCapture, capture};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_scanner::{ScanConfig, scan};
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// Captures `source` over a fresh file id: a clean scan and a
/// full-feature parse, interning into `strings` as the parse walks.
fn capture_fresh(source: &str, strings: &StringTable) -> FileCapture {
    let mut files = FileTable::new();
    let file = files.intern(Path::new("ticket_3974.dart"));
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
    capture(file, source, &result.tokens, features, strings)
}

/// The resolved spellings of the stored rows, in stored order — the
/// sequence every byte-exact consumer (the export's short-id table and
/// declaration rows) emits.
fn stored_names(captured: &FileCapture, strings: &StringTable) -> Vec<String> {
    captured
        .index
        .decls()
        .iter()
        .map(|row| strings.resolve(row.name).to_owned())
        .collect()
}

/// The observable row surface: every field a byte-exact consumer reads,
/// with names resolved through the run's own table — interned ids are
/// per-table encodings and legitimately differ run to run; the resolved
/// surface must not.
type RowSurface = Vec<(
    String,
    DeclKind,
    dartq_source::SourceRange,
    String,
    u32,
    u32,
)>;

fn row_surface(captured: &FileCapture, strings: &StringTable) -> RowSurface {
    captured
        .index
        .decls()
        .iter()
        .map(|row| {
            let container = (row.container != crate::DeclRow::NONE)
                .then(|| {
                    strings
                        .resolve(captured.index.row(row.container).name)
                        .to_owned()
                })
                .unwrap_or_default();
            (
                strings.resolve(row.name).to_owned(),
                row.kind,
                row.range,
                container,
                row.decl_end,
                row.body_open,
            )
        })
        .collect()
}

/// Twelve captures of the same source, each over a fresh table whose
/// names were interned in a rotated order — the stand-in for the
/// engine's per-run parallel file-processing order. Every run must hold
/// the identical resolved row table (anchors included) and the identical
/// canonical spelling sequence: the order keys on the spellings' bytes,
/// never on the interned ids that name them (#3974's `s1`/`s2` flip).
#[test]
fn row_table_is_identical_across_intern_orders() {
    const SOURCE: &str = "class Foo {}\nvoid top() {}\n";
    const NAMES: [&str; 7] = ["Foo", "top", "main", "Sub", "Base", "helper", "run"];
    const RUNS: usize = 12;

    let mut baseline_rows: Option<RowSurface> = None;
    let mut baseline_names: Option<Vec<String>> = None;
    for run in 0..RUNS {
        // `run` rotates the intern order: run 0 interns `Foo` first (the
        // a.dart-first order), run 1 interns `top` first (the b.dart-first
        // order — `b.dart` calls `top()`), later runs spread further.
        let strings = StringTable::new();
        for i in 0..NAMES.len() {
            // The pre-intern mints ids in the rotated order; the ids
            // themselves are the point, the return value is not.
            let _ = strings.intern(NAMES[(i + run) % NAMES.len()]);
        }
        let captured = capture_fresh(SOURCE, &strings);
        let rows = row_surface(&captured, &strings);
        let names = stored_names(&captured, &strings);
        if run == 0 {
            baseline_rows = Some(rows);
            baseline_names = Some(names);
        } else {
            assert_eq!(
                rows,
                baseline_rows.clone().expect("run 0 set the baseline"),
                "run {run}: row table must not follow the intern order"
            );
            assert_eq!(
                names,
                baseline_names.clone().expect("run 0 set the baseline"),
                "run {run}: canonical spelling sequence must be identical"
            );
        }
    }
    let baseline = baseline_names.expect("run 0 set the baseline");
    assert_eq!(
        baseline,
        ["Foo", "top"],
        "byte-canonical: 'Foo' sorts before 'top' whatever interned first"
    );
}

/// The stored order is the spellings' byte order, not the interned-id
/// order: the fixture declares `Zoo` before `Alpha`, so parse-order
/// interning mints `id(Zoo) < id(Alpha)` while the bytes say
/// `'A' < 'Z'` — an id-keyed sort places `Zoo` first, the canonical sort
/// places `Alpha` first. Members prove the anchors survived the new
/// permutation: `alpha`/`beta`/member `top` point at `Zoo`'s row wherever
/// it landed, the `extends Base` edge rides the same remap, and the
/// equal-name run (`top`, once member and once top-level) stays
/// offset-ascending for `by_name`.
#[test]
fn rows_sort_by_name_bytes_not_intern_id() {
    const SOURCE: &str = "\
class Zoo extends Base {
  void beta() {}
  void alpha() {}
  void top() {}
}
class Alpha {}
void top() {}
";
    let strings = StringTable::new();
    let captured = capture_fresh(SOURCE, &strings);

    // The discrimination premise: the fixture's parse order interns `Zoo`
    // before `Alpha`, so an id-keyed order would disagree with the bytes.
    let id = |name: &str| strings.intern(name);
    assert!(
        id("Zoo") < id("Alpha"),
        "premise: parse order mints id(Zoo) < id(Alpha)"
    );

    assert_eq!(
        stored_names(&captured, &strings),
        ["Alpha", "Zoo", "alpha", "beta", "top", "top"],
        "canonical byte order: Alpha < Zoo < alpha < beta < top"
    );

    let index = &captured.index;
    // Every distinct name still answers `by_name`, edge-only spellings
    // included in the negative.
    for name in ["Alpha", "Zoo", "alpha", "beta", "top"] {
        assert!(
            index.by_name(id(name)).is_some_and(|rows| !rows.is_empty()),
            "{name}"
        );
    }
    assert!(
        index.by_name(id("Base")).is_none(),
        "`Base` is an edge name, never a row"
    );

    // The equal-name run serves both `top` rows offset-ascending: the
    // member (inside `Zoo`) first, the top-level second.
    let tops = index.by_name(id("top")).expect("two top rows");
    assert_eq!(tops.len(), 2);
    assert!(tops[0].range.offset < tops[1].range.offset);
    assert_ne!(tops[0].container, crate::DeclRow::NONE, "member first");
    assert_eq!(tops[1].container, crate::DeclRow::NONE, "top-level second");
    assert_eq!(
        index.by_name(id("alpha")).expect("alpha")[0].kind,
        DeclKind::Method
    );

    // Member anchors ride the permutation: each member's container is
    // `Zoo`'s row wherever the canonical sort placed it.
    let zoo_position = index
        .decls()
        .iter()
        .position(|row| row.name == id("Zoo"))
        .expect("Zoo row");
    for name in ["alpha", "beta", "top"] {
        let row = index.by_name(id(name)).expect("member row")[0];
        assert_eq!(
            row.container as usize, zoo_position,
            "{name}'s anchor must remap to Zoo's row"
        );
    }
    assert_eq!(index.row(crate::narrow(zoo_position)).name, id("Zoo"));

    // The supertype edge anchors at `Zoo`'s row and carries `Base`.
    assert_eq!(index.supers().len(), 1);
    let edge = &index.supers()[0];
    assert_eq!(edge.container as usize, zoo_position);
    assert_eq!(edge.name, id("Base"));
}
