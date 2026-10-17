//! Dual-arm warm-start oracle — the facts-snapshot seam is not observable
//! (the incremental-equals-full-recompute gate extended to the persistence
//! face): every step of an edit sequence, an engine that lived through the
//! edits and an engine warm-started from a mid-sequence facts snapshot
//! produce byte-identical output.
//!
//! Arm A is the long-lived engine: a six-step sequence over one import
//! chain (a ← b ← c ← m), each step an established oracle case shape —
//! C2's comment-only edit, C9's signature-changing rename (the
//! full-cascade seed), C3's import heal with a `show` combinator and its
//! broken-import sibling, C1's body edit (the gate no-op), and a constant
//! C6-shaped syntax-error file (w) riding every step. After step 4 — the
//! sequence midpoint, with a coded directive and its baked
//! `uri_does_not_exist` row in residency, so the payload exercises the
//! snapshot's diagnostic and directive-target columns — the engine flushes
//! its facts through `save_facts_snapshot`, the daemon's flush face.
//!
//! Arm B is a fresh engine warm-started exactly the way the daemon
//! assembles a session: load the generation under the context's own
//! header, `ingest_facts_snapshot` before the first apply, then feed the
//! mid-sequence overlay and apply. It replays the remaining steps (5, 6).
//!
//! Assertions — at the fork, after every remaining step, and at the end:
//! - A1: raw `write_machine` bytes are equal across the arms, and the end
//!   state also matches the cold full-recompute arm (the oracle's standard
//!   face). The machine row is
//!   `severity|type|code|path|line|column|length|message`, so it carries
//!   no internal IDs: the "byte-identical except internal IDs" exemption
//!   has nothing to exempt here and no normalization is applied (the
//!   suite's A1 precedent — bytes compare raw).
//! - A2: the `AffectedReport` faces (changed/removed/invalidated),
//!   projected to fixture-relative defining paths (the suite's ID-free
//!   projection), match the per-step table on both arms.
//! - A3: the absolute (code, file) set equals the constant w baseline
//!   plus the step's rows (the C6 `Constant` pattern, so the syntax row is
//!   pinned without spelling its code name).
//!
//! Warm-start engagement is pinned at the daemon's own contract depth: the
//! generation loads under arm B's header (the SDK identity, package-graph
//! fingerprint, and context salt all derive from the shared fixture), the
//! ingest stages exactly the saved entry count (a load is never a subset
//! of a save), and arm B's first apply reports every live file changed —
//! the batch whose identities the staged payloads answer by content hash.
//! The one thing output equality cannot distinguish is a payload that
//! served from one that drained as stale and recomputed: both agree with
//! the truth by construction. The staging pins close that gap as far as
//! the public faces allow — the classification lookup is keyed by the same
//! content hash the save wrote, so a staged-but-never-served payload would
//! require the save and the live batch to disagree on the key, which the
//! shared fixture forecloses.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use common::Fixture;
use dartq_diagnostics::{AnalysisOutput, Diagnostic, FileLayoutProvider, write_machine};
use dartq_engine::{AffectedReport, CancellationToken, Engine, LibraryId, PersistedFacts};
use dartq_snapshot::{SnapshotConfig, SnapshotStore};
use dartq_source::FileId;

// ---------------------------------------------------------------------------
// The sequence (oracle case shapes; step numbers are 1-based)
// ---------------------------------------------------------------------------

const A0: &str = "int f() => 1;\n";
// Step 1: C2's comment-only shape — bytes move, face stable.
const A_COMMENT: &str = "// v2\nint f() => 1;\n";
// Step 2: C9's signature-changing rename — the full-cascade seed.
const A_RENAMED: &str = "int f2() => 1;\n";
// Step 5: C1's body edit under a stable face — the gate no-op.
const A_BODY: &str = "int f2() => 3;\n";
const B0: &str = "import 'a.dart';\nint g() => f();\n";
// Steps 3 and 6: C3's heal shape, carrying a `show` combinator (the
// combinator columns round-trip through the snapshot payload).
const B_COMBINATOR: &str = "import 'a.dart' show f2;\nint g() => f2();\n";
// Step 4: C3's broken-import shape — the coded directive whose URI row is
// baked into the mid-sequence snapshot.
const B_MISSING: &str = "import 'missing.dart';\nimport 'a.dart' show f2;\nint g() => f2();\n";
const C0: &str = "import 'b.dart';\nint h() => g();\n";
const M0: &str = "import 'c.dart';\nint m() => h();\n";
// C6's constant syntax error: present at every step, never edited.
const W: &str = "int w = ;\n";

/// The step after which arm A flushes and arm B warm-starts.
const FORK_AFTER: usize = 4;

struct Step {
    label: &'static str,
    batch: &'static [(&'static str, &'static str)],
    changed: &'static [&'static str],
    invalidated: &'static [&'static str],
    /// Absolute (code, file) rows beyond the constant w baseline.
    expected: &'static [(&'static str, &'static str)],
    /// The output bytes must equal the previous step's (the C1/C2
    /// output-invariance pins).
    stable: bool,
}

const STEPS: &[Step] = &[
    Step {
        label: "step 1",
        batch: &[("lib/a.dart", A_COMMENT)],
        changed: &["lib/a.dart"],
        invalidated: &["lib/a.dart"],
        expected: &[],
        stable: true,
    },
    Step {
        label: "step 2",
        batch: &[("lib/a.dart", A_RENAMED)],
        changed: &["lib/a.dart"],
        invalidated: &["lib/a.dart", "lib/b.dart", "lib/c.dart", "lib/m.dart"],
        // The rename orphans b's only use of a: the `f()` reference resolves
        // to nothing, so a.dart's import has no used elements — upstream
        // reports `unused_import` on the URI alongside `undefined_function`
        // (upstream Dart SDK source, imports_verifier.dart
        // `generateUnusedImportWarnings`; the not-existing-URI skip does not
        // apply, a.dart resolves).
        expected: &[
            ("undefined_function", "lib/b.dart"),
            ("unused_import", "lib/b.dart"),
        ],
        stable: false,
    },
    Step {
        label: "step 3",
        batch: &[("lib/b.dart", B_COMBINATOR)],
        changed: &["lib/b.dart"],
        // g's face stands, but the combinator moves b's directive face —
        // the signature folds the directive-region lexemes precisely so an
        // export-combinator edit cascades — so b seeds the importer closure:
        // c and m recompute (soundly; their rows are unchanged).
        invalidated: &["lib/b.dart", "lib/c.dart", "lib/m.dart"],
        expected: &[],
        stable: false,
    },
    Step {
        label: "step 4",
        batch: &[("lib/b.dart", B_MISSING)],
        changed: &["lib/b.dart"],
        // The added import moves b's directive face again: the same
        // seeded closure as step 3.
        invalidated: &["lib/b.dart", "lib/c.dart", "lib/m.dart"],
        expected: &[("uri_does_not_exist", "lib/b.dart")],
        stable: false,
    },
    Step {
        label: "step 5",
        batch: &[("lib/a.dart", A_BODY)],
        changed: &["lib/a.dart"],
        invalidated: &["lib/a.dart"],
        expected: &[("uri_does_not_exist", "lib/b.dart")],
        stable: true,
    },
    Step {
        label: "step 6",
        batch: &[("lib/b.dart", B_COMBINATOR)],
        changed: &["lib/b.dart"],
        // The healing import moves b's directive face: step 3's closure
        // shape again.
        invalidated: &["lib/b.dart", "lib/c.dart", "lib/m.dart"],
        expected: &[],
        stable: false,
    },
];

// ---------------------------------------------------------------------------
// A1 caliber: raw write_machine bytes — no normalization, no exempt fields
// ---------------------------------------------------------------------------

struct Layout<'a>(&'a Engine);

impl FileLayoutProvider for Layout<'_> {
    fn file_path(&self, file: FileId) -> &Path {
        self.0
            .file_table()
            .path(file)
            .expect("diagnosed file is resident")
    }

    fn line_col(&self, file: FileId, utf16_offset: u32) -> (u32, u32) {
        let source = self.0.content(file).expect("diagnosed file is resident");
        let index = self.0.line_index(file).expect("diagnosed file is resident");
        index.line_col_utf16(
            std::str::from_utf8(source).unwrap_or_default(),
            utf16_offset,
        )
    }
}

fn machine_bytes(engine: &Engine, diagnostics: &[Diagnostic]) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_machine(
        &mut bytes,
        &AnalysisOutput {
            diagnostics,
            files: &Layout(engine),
        },
    )
    .expect("in-memory write");
    bytes
}

// ---------------------------------------------------------------------------
// A2/A3 projections (the suite's ID-free discipline)
// ---------------------------------------------------------------------------

fn relative(fixture: &Fixture, file: FileId, engine: &Engine) -> String {
    let path = engine
        .file_table()
        .path(file)
        .expect("projected file is resident");
    let canonical = fixture.root.canonicalize().expect("fixture root resolves");
    path.strip_prefix(&canonical)
        .expect("fixture-relative path")
        .to_string_lossy()
        .into_owned()
}

fn sorted_paths(fixture: &Fixture, engine: &Engine, files: &[FileId]) -> Vec<String> {
    let mut paths: Vec<String> = files
        .iter()
        .map(|file| relative(fixture, *file, engine))
        .collect();
    paths.sort();
    paths
}

fn sorted_defining(fixture: &Fixture, engine: &Engine, libraries: &[LibraryId]) -> Vec<String> {
    let mut paths: Vec<String> = libraries
        .iter()
        .map(|library| {
            let file = engine
                .library_defining_file(*library)
                .expect("reported library is live");
            relative(fixture, file, engine)
        })
        .collect();
    paths.sort();
    paths
}

fn absolute_set(
    fixture: &Fixture,
    engine: &Engine,
    diagnostics: &[Diagnostic],
) -> BTreeSet<(String, String)> {
    diagnostics
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.code.name.to_owned(),
                relative(fixture, diagnostic.file, engine),
            )
        })
        .collect()
}

fn code_file_set(entries: &[(&'static str, &'static str)]) -> BTreeSet<(String, String)> {
    entries
        .iter()
        .map(|(code, path)| ((*code).to_owned(), (*path).to_owned()))
        .collect()
}

/// The step's expected absolute set: the constant baseline plus the step's
/// rows.
fn expected_absolute(
    baseline: &BTreeSet<(String, String)>,
    step: &Step,
) -> BTreeSet<(String, String)> {
    let mut expected = baseline.clone();
    expected.extend(code_file_set(step.expected));
    expected
}

// ---------------------------------------------------------------------------
// The arms
// ---------------------------------------------------------------------------

/// One engine pass's observable faces.
struct Outcome {
    report: AffectedReport,
    /// A1: the raw output bytes.
    bytes: Vec<u8>,
    /// A3: the absolute (code, file) set.
    absolute: BTreeSet<(String, String)>,
}

/// Sets every live file's overlay content (arm B's feed; sorted keys make
/// the intern order deterministic).
fn feed(fixture: &Fixture, engine: &mut Engine, state: &BTreeMap<String, String>) {
    for (path, source) in state {
        engine.set_content(&fixture.path(path), source.as_bytes());
    }
}

/// One apply + analyze pass over `state`'s live set.
fn settle(fixture: &Fixture, engine: &mut Engine, state: &BTreeMap<String, String>) -> Outcome {
    let report = engine.apply_pending(&mut |_| {});
    let diagnostics = analyze_all(fixture, engine, state);
    Outcome {
        bytes: machine_bytes(engine, &diagnostics),
        absolute: absolute_set(fixture, engine, &diagnostics),
        report,
    }
}

fn analyze_all(
    fixture: &Fixture,
    engine: &mut Engine,
    state: &BTreeMap<String, String>,
) -> Vec<Diagnostic> {
    let targets: Vec<FileId> = state
        .keys()
        .map(|path| {
            engine
                .file_table()
                .try_file_id(&fixture.path(path))
                .expect("live file interned")
        })
        .collect();
    engine
        .analyze(&targets, &mut |_| {}, &CancellationToken::new())
        .expect("analyze")
}

/// One step on one engine: the batch in (sorted by path, the oracle
/// enqueue's determinism), the observable faces out.
fn run_step(
    fixture: &Fixture,
    engine: &mut Engine,
    state: &mut BTreeMap<String, String>,
    step: &Step,
) -> Outcome {
    let mut batch: Vec<&(&str, &str)> = step.batch.iter().collect();
    batch.sort_by_key(|entry| entry.0);
    for (path, source) in batch {
        engine.set_content(&fixture.path(path), source.as_bytes());
        state.insert((*path).to_owned(), (*source).to_owned());
    }
    settle(fixture, engine, state)
}

/// The cold arm (the oracle's full-recompute face): a fresh engine over the
/// state, one cold feed + apply + analyze. Only its output bytes matter.
fn full_arm_bytes(fixture: &Fixture, state: &BTreeMap<String, String>) -> Vec<u8> {
    let mut engine = fixture.engine();
    feed(fixture, &mut engine, state);
    settle(fixture, &mut engine, state).bytes
}

/// Arm B: a fresh engine warm-started the way the daemon assembles a
/// session — load the generation under the context's own header, ingest
/// before the first apply, feed the mid-sequence overlay, apply, analyze.
/// Returns the engine (for the remaining steps), the staged count, and the
/// fork-point faces.
fn fork_warm_arm(
    fixture: &Fixture,
    store: &SnapshotStore,
    state: &BTreeMap<String, String>,
) -> (Engine, usize, Outcome) {
    let mut engine = fixture.engine();
    let snapshot = store
        .load::<PersistedFacts>(&engine.snapshot_header())
        .expect("arm A's generation loads under arm B's own header");
    let staged = engine.ingest_facts_snapshot(snapshot);
    feed(fixture, &mut engine, state);
    let outcome = settle(fixture, &mut engine, state);
    assert_eq!(
        outcome.report.changed.len(),
        state.len(),
        "fresh residency: every live file is a first-apply change"
    );
    (engine, staged, outcome)
}

/// A2 on both faces of one arm's step report.
fn check_report(fixture: &Fixture, engine: &Engine, report: &AffectedReport, step: &Step) {
    assert_eq!(
        sorted_paths(fixture, engine, &report.changed),
        step.changed,
        "A2 changed {}",
        step.label
    );
    assert_eq!(
        sorted_paths(fixture, engine, &report.removed),
        &[] as &[&str],
        "A2 removed {}",
        step.label
    );
    assert_eq!(
        sorted_defining(fixture, engine, &report.invalidated),
        step.invalidated,
        "A2 invalidated {}",
        step.label
    );
}

/// A3: the step's absolute set against the constant baseline.
fn check_absolute(outcome: &Outcome, baseline: &BTreeSet<(String, String)>, step: &Step) {
    assert_eq!(
        outcome.absolute,
        expected_absolute(baseline, step),
        "A3 {}",
        step.label
    );
}

/// The dual-arm warm-start gate: a mid-sequence snapshot save followed by a
/// warm start must be indistinguishable from living through the edits.
#[test]
fn warm_start_is_not_an_observable_state_seam() {
    let initial = [
        ("lib/a.dart", A0),
        ("lib/b.dart", B0),
        ("lib/c.dart", C0),
        ("lib/m.dart", M0),
        ("lib/w.dart", W),
    ];
    let fixture = Fixture::new(&initial);
    let mut state: BTreeMap<String, String> = initial
        .iter()
        .map(|(path, source)| ((*path).to_owned(), (*source).to_owned()))
        .collect();

    // Arm A, step 0: the baseline. w's syntax row is captured, never
    // spelled (the C6 Constant pattern).
    let mut arm_a = fixture.engine();
    feed(&fixture, &mut arm_a, &state);
    let step0 = settle(&fixture, &mut arm_a, &state);
    assert_eq!(step0.report.changed.len(), initial.len(), "step 0 apply");
    assert!(
        !step0.absolute.is_empty(),
        "w's syntax row anchors the baseline"
    );
    let baseline = step0.absolute;
    let mut previous_a = step0.bytes;

    // Arm A, steps 1..=4: the pre-fork segment, oracle faces per step.
    for step in &STEPS[..FORK_AFTER] {
        let outcome = run_step(&fixture, &mut arm_a, &mut state, step);
        check_report(&fixture, &arm_a, &outcome.report, step);
        check_absolute(&outcome, &baseline, step);
        if step.stable {
            assert_eq!(
                outcome.bytes, previous_a,
                "A1 output invariance {}",
                step.label
            );
        }
        previous_a = outcome.bytes;
    }

    // The flush: the daemon's save face over the mid-sequence residency.
    let store = SnapshotStore::open(SnapshotConfig {
        root: Some(fixture.root.join("snapshot-store")),
        capacity_bytes: 64 * 1024 * 1024,
    })
    .expect("store opens");
    let saved = arm_a.save_facts_snapshot(&store).expect("flush");
    assert_eq!(
        saved,
        state.len(),
        "one entry per distinct content identity"
    );

    // A1 at the fork: arm A's bytes are the true full-recompute face.
    assert_eq!(
        previous_a,
        full_arm_bytes(&fixture, &state),
        "A1 at the fork"
    );

    // Arm B: the warm start. Its first apply materializes every file from
    // the staged payloads, so the fork-point faces come straight out of
    // the persisted columns.
    let (mut arm_b, staged, fork_b) = fork_warm_arm(&fixture, &store, &state);
    assert_eq!(staged, saved, "the load is never a subset of the save");
    assert_eq!(
        fork_b.bytes, previous_a,
        "A1 the warm arm answers the fork point"
    );
    assert_eq!(
        fork_b.absolute,
        expected_absolute(&baseline, &STEPS[FORK_AFTER - 1]),
        "A3 the warm fork point"
    );
    let mut previous_b = fork_b.bytes;

    // Steps 5..=6 in lockstep: the lived arm and the warm arm.
    for step in &STEPS[FORK_AFTER..] {
        let a = run_step(&fixture, &mut arm_a, &mut state, step);
        let b = run_step(&fixture, &mut arm_b, &mut state, step);
        check_report(&fixture, &arm_a, &a.report, step);
        check_report(&fixture, &arm_b, &b.report, step);
        check_absolute(&a, &baseline, step);
        check_absolute(&b, &baseline, step);
        assert_eq!(a.bytes, b.bytes, "A1 warm-vs-lived {}", step.label);
        if step.stable {
            assert_eq!(
                a.bytes, previous_a,
                "A1 output invariance {} (lived arm)",
                step.label
            );
            assert_eq!(
                b.bytes, previous_b,
                "A1 output invariance {} (warm arm)",
                step.label
            );
        }
        previous_a = a.bytes;
        previous_b = b.bytes;
    }

    // The cold face at the end: both arms equal the full recompute.
    let full = full_arm_bytes(&fixture, &state);
    assert_eq!(
        previous_a, full,
        "A1 the lived arm matches the cold recompute"
    );
    assert_eq!(
        previous_b, full,
        "A1 the warm arm matches the cold recompute"
    );
}
