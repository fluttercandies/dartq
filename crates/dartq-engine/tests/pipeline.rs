//! The full-pipeline integration pin (goal #88): a two-file fixture through
//! scan → parse → semantic collect → resolution graph → diagnostics
//! assembly, plus the CSR export shape and the removal path. Contracts come
//! from engine-api-spec §1.2/§1.3/§2.2, incremental-l1-spec §3/§6, and the
//! GPU posture rider (CSR-friendly `&[u32]` views).

mod common;

use common::{EventLog, Fixture, GREETER_0, GREETER_1, MAIN_0};
use dartq_engine::{EngineError, RevalidateGate};

#[test]
fn two_file_fixture_runs_the_full_pipeline() {
    let fixture = Fixture::new(&[("lib/main.dart", MAIN_0), ("lib/greeter.dart", GREETER_0)]);
    let mut engine = fixture.engine();

    let stats = engine.discover_workspace().expect("discovery");
    assert_eq!(stats.file_count, 2, "both lib files discovered");
    assert!(
        stats.loc_total >= 6,
        "one line per source line, got {}",
        stats.loc_total
    );

    let mut log = EventLog::default();
    let diagnostics = engine
        .analyze(&[], &mut log.sink(), &Default::default())
        .expect("analyze");

    // Valid Dart: no diagnostics.
    assert!(
        diagnostics.is_empty(),
        "unexpected diagnostics: {diagnostics:?}"
    );

    // Event contract for the cold run (§1.4): one OverlayApplied with both
    // files changed, one Parsed FileRevalidated per file, one non-empty
    // CascadeInvalidated, one LibraryAnalyzed per library.
    assert_eq!(log.overlay_applied, vec![(2, 0)], "overlay batch boundary");
    assert_eq!(log.revalidated.len(), 2, "both files revalidated");
    assert!(log.revalidated.iter().all(|(_, gate)| matches!(
        gate,
        RevalidateGate::Parsed {
            ast_cache_hit: false
        }
    )));
    assert_eq!(log.cascades.len(), 1);
    assert_eq!(log.cascades[0].len(), 2, "both libraries dirty on cold run");
    assert_eq!(log.analyzed.len(), 2);
    assert!(log.analyzed.iter().all(|(_, from_cache)| !*from_cache));

    // Read faces (§1.3): content round-trip, line index presence, library
    // membership — two defining libraries, no parts.
    let main = engine
        .file_table()
        .try_file_id(&fixture.path("lib/main.dart"))
        .expect("main interned");
    let greeter = engine
        .file_table()
        .try_file_id(&fixture.path("lib/greeter.dart"))
        .expect("greeter interned");
    assert_eq!(engine.content(main), Some(MAIN_0.as_bytes()));
    assert!(engine.line_index(main).is_some());
    let main_library = engine.library_of(main).expect("main has a library");
    let greeter_library = engine.library_of(greeter).expect("greeter has a library");
    assert_ne!(main_library, greeter_library);
    for (_, members) in engine.libraries() {
        assert_eq!(members.len(), 1, "no parts in this fixture");
    }

    // Repeat analyze: everything cached, no work events.
    let mut warm = EventLog::default();
    let warm_diagnostics = engine
        .analyze(&[], &mut warm.sink(), &Default::default())
        .expect("warm analyze");
    assert_eq!(warm_diagnostics, diagnostics, "clean re-analyze is stable");
    assert!(warm.overlay_applied.is_empty(), "empty queue emits nothing");
    assert!(warm.revalidated.is_empty());
    assert!(
        warm.analyzed.iter().all(|(_, from_cache)| *from_cache),
        "targets report cache hits"
    );
}

#[test]
fn unresolved_import_emits_uri_diagnostic() {
    let fixture = Fixture::new(&[(
        "lib/main.dart",
        "import 'missing.dart';\n\nvoid main() {}\n",
    )]);
    let mut engine = fixture.engine();
    engine.discover_workspace().expect("discovery");

    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &Default::default())
        .expect("analyze");
    assert_eq!(
        diagnostics.len(),
        1,
        "one uri_does_not_exist, got {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].code.name, "uri_does_not_exist");
}

#[test]
fn csr_exports_match_the_import_shape() {
    let fixture = Fixture::new(&[("lib/main.dart", MAIN_0), ("lib/greeter.dart", GREETER_0)]);
    let mut engine = fixture.engine();
    engine.discover_workspace().expect("discovery");
    engine
        .analyze(&[], &mut |_| {}, &Default::default())
        .expect("analyze");

    let main = engine
        .file_table()
        .try_file_id(&fixture.path("lib/main.dart"))
        .expect("main");
    let greeter = engine
        .file_table()
        .try_file_id(&fixture.path("lib/greeter.dart"))
        .expect("greeter");
    let main_library = engine.library_of(main).expect("owned");
    let greeter_library = engine.library_of(greeter).expect("owned");

    // Library import graph (GPU posture): flat u32 views, one edge
    // main → greeter.
    let csr = engine.library_import_graph();
    assert_eq!(csr.nodes(), 2, "two libraries");
    assert_eq!(csr.offsets().len(), 3, "offsets are nodes + 1");
    assert_eq!(csr.len_edges(), 1, "one import edge");
    let row = main_library.index() as usize;
    let edges = &csr.edges()[csr.offsets()[row] as usize..csr.offsets()[row + 1] as usize];
    assert_eq!(edges, &[greeter_library.index()], "main imports greeter");
    let greeter_row = greeter_library.index() as usize;
    assert_eq!(
        csr.offsets()[greeter_row],
        csr.offsets()[greeter_row + 1],
        "greeter imports nothing"
    );

    // File reference graph: main's directive resolves to the greeter FileId.
    let refs = engine.file_reference_graph();
    let main_row = main.index() as usize;
    let ref_edges =
        &refs.edges()[refs.offsets()[main_row] as usize..refs.offsets()[main_row + 1] as usize];
    assert_eq!(ref_edges, &[greeter.index()]);
}

#[test]
fn remove_file_recomputes_the_importer_with_uri_diagnostic() {
    let fixture = Fixture::new(&[("lib/main.dart", MAIN_0), ("lib/greeter.dart", GREETER_0)]);
    let mut engine = fixture.engine();
    engine.discover_workspace().expect("discovery");
    let clean = engine
        .analyze(&[], &mut |_| {}, &Default::default())
        .expect("clean");
    assert!(clean.is_empty());

    engine
        .remove_file(&fixture.path("lib/greeter.dart"))
        .expect("removal enqueued");

    let mut log = EventLog::default();
    let diagnostics = engine
        .analyze(&[], &mut log.sink(), &Default::default())
        .expect("analyze");

    assert!(log.overlay_applied.contains(&(0, 1)), "one file removed");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code.name == "uri_does_not_exist"),
        "the importer now fails to resolve its import: {diagnostics:?}"
    );
    // The removed file's library is gone; only the importer remains.
    assert_eq!(engine.libraries().len(), 1);
    let csr = engine.library_import_graph();
    assert_eq!(
        csr.len_edges(),
        0,
        "the import edge disappeared with the target"
    );
}

#[test]
fn edited_content_is_still_clean() {
    // Sanity that the edited bytes (the incremental test's input) are valid.
    let fixture = Fixture::new(&[("lib/main.dart", MAIN_0), ("lib/greeter.dart", GREETER_1)]);
    let mut engine = fixture.engine();
    engine.discover_workspace().expect("discovery");
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &Default::default())
        .expect("analyze");
    assert!(diagnostics.is_empty());
}

#[test]
fn unknown_remove_is_an_explicit_error() {
    let fixture = Fixture::new(&[]);
    let mut engine = fixture.engine();
    let error = engine
        .remove_file(&fixture.path("lib/never.dart"))
        .expect_err("unknown path");
    assert!(matches!(error, EngineError::UnknownPath(_)));
}
