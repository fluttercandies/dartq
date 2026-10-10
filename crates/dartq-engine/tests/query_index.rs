//! The maintained symbol-index face (index-design-spec §6/§8 — the Engine
//! owns dirty scheduling, the index never invalidates itself): `query
//! symbol` answers from the captures and per-library reference surfaces
//! analyze maintains at the `LibraryAnalyzed` point.
//!
//! Pins the three behaviors the per-query rebuild used to get for free and
//! the maintained index must now uphold: empty-before-analyze, declaration
//! spans tracking content the L2 gate keeps library-clean (a body edit
//! moves offsets without dirtying any library), and removals dropping both
//! the declarations and the resolved references.

mod common;

use common::Fixture;

const MAIN: &str = "import 'greeter.dart';\n\nclass Use {\n  String hi() => greet();\n}\n";
const GREETER: &str = "// header\nString greet() => 'hi';\n";
/// Same declarations, shifted down one comment line — every offset moves
/// by exactly the inserted line while the API signature stands.
const GREETER_SHIFTED: &str = "// header\n// shifted\nString greet() => 'hi';\n";

fn analyze(engine: &mut dartq_engine::Engine) {
    engine
        .analyze(&[], &mut |_| {}, &Default::default())
        .expect("analyze");
}

/// Registers the fixture's files through the content overlay — the engine's
/// only content entry (nothing is resident until a `set_content` batch
/// applies; discovery is itself just a `set_content` batch) — then runs the
/// first analyze. This is the population path every maintained capture and
/// library surface enters through.
fn populate_and_analyze(fixture: &Fixture, engine: &mut dartq_engine::Engine) {
    engine.set_content(&fixture.path("lib/main.dart"), MAIN.as_bytes());
    engine.set_content(&fixture.path("lib/greeter.dart"), GREETER.as_bytes());
    analyze(engine);
}

#[test]
fn query_is_empty_before_the_first_analyze() {
    let fixture = Fixture::new(&[("lib/main.dart", MAIN), ("lib/greeter.dart", GREETER)]);
    let engine = fixture.engine();
    let answer = engine.query_symbols("greet");
    assert!(answer.decls.is_empty());
    assert!(answer.refs.is_empty());
}

#[test]
fn query_tracks_spans_across_signature_stable_edits() {
    let fixture = Fixture::new(&[("lib/main.dart", MAIN), ("lib/greeter.dart", GREETER)]);
    let mut engine = fixture.engine();
    populate_and_analyze(&fixture, &mut engine);

    // The declaration: top-level `greet` in the greeter, `greet` at offset
    // 17 (`// header\n` + `String `), and the one reference from main's
    // `Use.hi` body targeting it.
    let answer = engine.query_symbols("greet");
    assert_eq!(answer.decls.len(), 1, "one declaration: {answer:?}");
    let decl = answer.decls[0];
    assert_eq!(decl.name, "greet");
    assert!(decl.container.is_none(), "top-level row");
    assert_eq!(
        engine
            .file_table()
            .path(decl.file)
            .and_then(|path| path.to_str())
            .map(str::to_owned)
            .as_deref(),
        fixture.path("lib/greeter.dart").to_str(),
    );
    assert_eq!(decl.range.offset, 17);
    assert_eq!(decl.range.length, 5);
    assert_eq!(answer.refs.len(), 1, "one use site: {answer:?}");
    assert_eq!(answer.refs[0].target_file, decl.file);
    assert!(!answer.refs[0].qualified);

    // A comment line above the declaration: the L2 gate keeps the greeter
    // library clean (no recomputation, no cascade), but the name token's
    // offset moved — the maintained capture must track it.
    engine.set_content(
        fixture.path("lib/greeter.dart").as_path(),
        GREETER_SHIFTED.as_bytes(),
    );
    analyze(&mut engine);
    let answer = engine.query_symbols("greet");
    assert_eq!(answer.decls.len(), 1);
    let shift = u32::try_from("// shifted\n".len()).expect("literal length");
    assert_eq!(
        answer.decls[0].range.offset,
        17 + shift,
        "the maintained capture tracks the shifted span",
    );
    assert_eq!(answer.refs.len(), 1);
}

#[test]
fn query_drops_removed_files() {
    let fixture = Fixture::new(&[("lib/main.dart", MAIN), ("lib/greeter.dart", GREETER)]);
    let mut engine = fixture.engine();
    populate_and_analyze(&fixture, &mut engine);
    assert_eq!(engine.query_symbols("greet").decls.len(), 1);

    engine
        .remove_file(fixture.path("lib/greeter.dart").as_path())
        .expect("remove");
    analyze(&mut engine);

    // The capture left residency with the file, and the reference surface
    // rebuilt without the target (the use site degrades to an unresolved
    // name row — `undefined_function` is the diagnostics face's business).
    let answer = engine.query_symbols("greet");
    assert!(answer.decls.is_empty(), "{answer:?}");
    assert!(answer.refs.is_empty(), "{answer:?}");
}

/// The use-file column (#361): one library spans its defining unit plus a
/// part, and the use sits in the part — the hit's `file` must name the
/// library member the use site sits in, never the target's file and never
/// another member of the same library.
#[test]
fn query_use_file_is_the_library_member() {
    const PARTED_MAIN: &str = "import 'greeter.dart';\npart 'extra.dart';\n";
    const PART_EXTRA: &str = "part of 'main.dart';\nvoid extra() {\n  greet();\n}\n";

    let fixture = Fixture::new(&[
        ("lib/main.dart", PARTED_MAIN),
        ("lib/greeter.dart", GREETER),
        ("lib/extra.dart", PART_EXTRA),
    ]);
    let mut engine = fixture.engine();
    for (relative, source) in [
        ("lib/main.dart", PARTED_MAIN),
        ("lib/greeter.dart", GREETER),
        ("lib/extra.dart", PART_EXTRA),
    ] {
        engine.set_content(fixture.path(relative).as_path(), source.as_bytes());
    }
    analyze(&mut engine);

    let answer = engine.query_symbols("greet");
    assert_eq!(answer.decls.len(), 1, "{answer:?}");
    let decl = answer.decls[0];
    assert_eq!(answer.refs.len(), 1, "{answer:?}");
    let hit = answer.refs[0];
    assert_eq!(
        hit.target_file, decl.file,
        "the target pair still names the declaring file"
    );
    assert_eq!(
        engine
            .file_table()
            .path(hit.file)
            .and_then(|path| path.to_str())
            .map(str::to_owned)
            .as_deref(),
        fixture.path("lib/extra.dart").to_str(),
        "the use file is the part member, not the target and not the defining unit",
    );
}
