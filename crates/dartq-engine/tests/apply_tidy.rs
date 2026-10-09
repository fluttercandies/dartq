//! Apply-path tidy pins (goal #272): a batch computes identical-content
//! members once (with per-file attribution and resolution preserved), and
//! create/retire churn reuses retired `LibraryId`s instead of growing the
//! arena. Goal #303 pins the cross-batch face: a facts-cache hit re-runs
//! the same per-file derivation (attribution + URI resolution).

mod common;

use std::collections::BTreeMap;

use common::{EventLog, Fixture, STANDALONE_0};
use dartq_engine::{CancellationToken, EngineEvent, RevalidateGate};

/// Byte-identical batch members: one `import` whose target exists nowhere,
/// so each member's facts must carry its own resolution diagnostic. Every
/// member is written to disk (not overlay-only): the file table's path memo
/// canonicalizes on-disk spellings and keeps lexical ones for the rest, and
/// the attribution face only matches the lexical package roots — an
/// overlay-only member would derive a different language version and land
/// on a different `FactsKey` (pre-existing mismatch, not this ticket's).
const SHARED: &str = "import 'missing.dart';\n";

#[test]
fn batch_of_identical_files_computes_once_and_attributes_per_file() {
    let fixture = Fixture::new(&[
        ("lib/a.dart", SHARED),
        ("lib/src/b.dart", SHARED),
        ("lib/c.dart", SHARED),
    ]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/a.dart"), SHARED.as_bytes());
    engine.set_content(&fixture.path("lib/src/b.dart"), SHARED.as_bytes());

    // One batch: the first member computes, the identical sibling derives
    // without a re-parse.
    let mut log = EventLog::default();
    engine.apply_pending(&mut log.sink());
    let parsed: Vec<bool> = log
        .revalidated
        .iter()
        .map(|(_, gate)| match gate {
            RevalidateGate::Parsed { ast_cache_hit } => *ast_cache_hit,
            RevalidateGate::ContentIdHit => panic!("fresh content cannot hit the ContentId gate"),
        })
        .collect();
    assert_eq!(parsed.len(), 2, "both batch members revalidated");
    assert_eq!(
        parsed.iter().filter(|hit| !**hit).count(),
        1,
        "exactly one member computes: {parsed:?}"
    );
    assert_eq!(
        parsed.iter().filter(|hit| **hit).count(),
        1,
        "the sibling derives without a re-parse: {parsed:?}"
    );

    // Derivation preserves per-file attribution: each path carries exactly
    // its own `uri_does_not_exist` — the sibling's copy must not leak.
    let diagnostics = engine
        .analyze(&[], &mut log.sink(), &CancellationToken::default())
        .expect("analyze succeeds");
    let mut per_path: BTreeMap<String, usize> = BTreeMap::new();
    for diagnostic in &diagnostics {
        assert_eq!(diagnostic.code.name, "uri_does_not_exist");
        let path = engine
            .file_table()
            .path(diagnostic.file)
            .expect("attributed file is interned")
            .to_string_lossy()
            .into_owned();
        *per_path.entry(path).or_default() += 1;
    }
    assert_eq!(
        per_path.len(),
        2,
        "each identical member attributes its own diagnostic: {per_path:?}"
    );
    for (path, count) in &per_path {
        assert_eq!(*count, 1, "{path} must carry exactly one diagnostic");
    }

    // The pre-existing cross-batch path is unchanged: a later file with the
    // same content hits the facts cache.
    let c = engine.set_content(&fixture.path("lib/c.dart"), SHARED.as_bytes());
    let mut again = EventLog::default();
    engine.apply_pending(&mut again.sink());
    assert_eq!(
        again.revalidated,
        vec![(
            c,
            RevalidateGate::Parsed {
                ast_cache_hit: true
            }
        )],
        "same content in a later batch must hit the facts cache"
    );
}

#[test]
fn cross_batch_facts_cache_hit_reattributes_per_file() {
    // Two identical-content twins per face, in different directories:
    // the first twin of each pair computes and caches in batch 1, the
    // second hits the cache in batch 2 — the served face must re-derive
    // for the hit's own file (attribution + relative-URI resolution).
    const IMPORTER: &str = "import 'anchor.dart';\n";
    const LEAKER: &str = "import 'missing.dart';\n";
    const ANCHOR: &str = "// the resolution anchor: exists next to a, not b\n";
    let fixture = Fixture::new(&[
        ("lib/anchor.dart", ANCHOR),
        ("lib/a.dart", IMPORTER),
        ("lib/src/b.dart", IMPORTER),
        ("lib/c.dart", LEAKER),
        ("lib/src/d.dart", LEAKER),
    ]);
    let mut engine = fixture.engine();
    let mut sink = |_: EngineEvent| {};

    // Batch 1: the anchor plus both cache templates. `a` resolves its
    // relative import against lib/ (target resident, no diagnostic); `c`
    // caches a failing resolution with its diagnostic attributed to itself.
    engine.set_content(&fixture.path("lib/a.dart"), IMPORTER.as_bytes());
    engine.set_content(&fixture.path("lib/c.dart"), LEAKER.as_bytes());
    engine.set_content(&fixture.path("lib/anchor.dart"), ANCHOR.as_bytes());
    let mut first = EventLog::default();
    engine.apply_pending(&mut first.sink());
    assert_eq!(
        first
            .revalidated
            .iter()
            .filter(|(_, gate)| matches!(
                gate,
                RevalidateGate::Parsed {
                    ast_cache_hit: false
                }
            ))
            .count(),
        3,
        "batch 1 computes every member fresh: {:?}",
        first.revalidated
    );
    engine
        .analyze(&[], &mut sink, &CancellationToken::default())
        .expect("batch 1 analyze succeeds");

    // Batch 2: the twins. Both must ride the facts cache.
    let b = engine.set_content(&fixture.path("lib/src/b.dart"), IMPORTER.as_bytes());
    let d = engine.set_content(&fixture.path("lib/src/d.dart"), LEAKER.as_bytes());
    let mut second = EventLog::default();
    engine.apply_pending(&mut second.sink());
    assert_eq!(
        second.revalidated,
        vec![
            (
                b,
                RevalidateGate::Parsed {
                    ast_cache_hit: true
                }
            ),
            (
                d,
                RevalidateGate::Parsed {
                    ast_cache_hit: true
                }
            ),
        ],
        "both twins must hit the facts cache (FileIds ascend): {:?}",
        second.revalidated
    );

    // The hits land fully re-attributed: `b` re-resolves against lib/src/
    // (no anchor there — its own uri_does_not_exist), and `d`'s served
    // diagnostics are its own copy, not `c`'s FileId.
    let diagnostics = engine
        .analyze(&[], &mut second.sink(), &CancellationToken::default())
        .expect("batch 2 analyze succeeds");
    let mut per_path: BTreeMap<String, usize> = BTreeMap::new();
    for diagnostic in &diagnostics {
        assert_eq!(diagnostic.code.name, "uri_does_not_exist");
        let path = engine
            .file_table()
            .path(diagnostic.file)
            .expect("attributed file is interned")
            .to_string_lossy()
            .into_owned();
        *per_path.entry(path).or_default() += 1;
    }
    let expected: BTreeMap<String, usize> = ["lib/src/b.dart", "lib/c.dart", "lib/src/d.dart"]
        .into_iter()
        .map(|path| (fixture.path(path).to_string_lossy().into_owned(), 1))
        .collect();
    assert_eq!(
        per_path, expected,
        "each hit must carry exactly its own diagnostic: {per_path:?}"
    );
}

#[test]
fn retired_library_ids_are_reused_across_churn() {
    let fixture = Fixture::new(&[("lib/a.dart", STANDALONE_0)]);
    let mut engine = fixture.engine();
    let mut sink = |_: EngineEvent| {};

    let a = engine.set_content(&fixture.path("lib/a.dart"), STANDALONE_0.as_bytes());
    engine.apply_pending(&mut sink);
    let a_lib = engine.library_of(a).expect("a forms a library");
    assert_eq!(engine.library_import_graph().nodes(), 1, "one arena slot");

    // Create/retire churn: each cycle retires the live library and forms a
    // replacement, which must reuse the retired slot — the arena never
    // grows past the historical live peak.
    let mut current = "lib/a.dart".to_owned();
    for step in 0..5 {
        engine
            .remove_file(&fixture.path(&current))
            .expect("the live library's file is resident");
        engine.apply_pending(&mut sink);
        current = format!("lib/gen{step}.dart");
        let replacement = engine.set_content(&fixture.path(&current), STANDALONE_0.as_bytes());
        engine.apply_pending(&mut sink);
        assert_eq!(
            engine.library_import_graph().nodes(),
            1,
            "cycle {step}: arena must not grow past the live peak"
        );
        assert_eq!(
            engine.library_of(replacement),
            Some(a_lib),
            "cycle {step}: replacement must reuse the retired id"
        );
    }
}
