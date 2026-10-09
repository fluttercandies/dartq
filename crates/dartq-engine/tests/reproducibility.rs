//! The reproducibility-tuple pin (PRD §19.5): same input + same engine
//! version ⇒ byte-identical outputs. The assertion face is the rendered
//! machine-format lines (`dartq-diagnostics`' `write_machine`) over the
//! canonical merged diagnostic set — the observable bytes the CLI prints,
//! not `Debug` output — so the pin holds against work-stealing order, intern
//! order, and cross-run process state, and fails if any of those leaks.
//!
//! Determinism sources exercised deliberately:
//!
//! * a five-library workspace so the batch fan-out has >1 item (the engine
//!   collects positionally; proven end-to-end here);
//! * same-named symbols (`Item`, `shared`) in different libraries — the
//!   shared string intern table must order identically regardless of which
//!   library interned first;
//! * an incremental sequence (edit + `apply_pending`) whose result must be
//!   byte-identical to a fresh full recompute of the same final content —
//!   PRD §19.4's caliber at smoke depth;
//! * the double analysis runs twice (four cold chains in one process), so
//!   any process-wide cache (statics, lazy cells) that leaked between runs
//!   would diverge the later bytes.
//!
//! Version keying: [`EngineConfig`] carries only content-derived faces
//! (store, SDK snapshot keyed by a blake3 `ContentId`, package table over
//! the parsed config, paths, the semantic plug point) — no timestamps,
//! randomness, or process ids. The per-library `Duration` exists only on
//! work-order `EngineEvent` telemetry (PRD §19.3 benchmark hook), never in
//! the output face. `engine_version_key_is_stable_across_constructions`
//! pins the version-tuple precondition the output identity is keyed by.

mod common;

use std::path::Path;

use common::Fixture;
use dartq_diagnostics::{AnalysisOutput, Diagnostic, FileLayoutProvider, write_machine};
use dartq_engine::Engine;
use dartq_sdk::SdkSnapshot;
use dartq_source::FileId;

// ---------------------------------------------------------------------------
// Corpus: five libraries, a diamond import chain, one URI failure, three
// syntax errors, and same-named symbols in three libraries
// ---------------------------------------------------------------------------

/// Root library importing both same-named providers plus a missing file
/// (one `uri_does_not_exist` on the byte face).
const MAIN: &str = "import 'alpha.dart';\nimport 'beta.dart';\nimport 'missing.dart';\n\nvoid main() {\n  print(shared());\n}\n";
/// Provider A — `Item`/`shared` deliberately collide with beta (and delta's
/// edit) across library boundaries; carries one syntax error.
const ALPHA: &str =
    "import 'gamma.dart';\n\nclass Item {\n  int value = ~;\n}\n\nint shared() => gammaValue;\n";
/// Provider B — same names as alpha, different library (intern-order face).
const BETA: &str = "import 'gamma.dart';\n\nclass Item {\n  String name = '';\n}\n\nint shared() => gammaValue + 1;\n";
/// The diamond's base — both providers import it.
const GAMMA: &str = "final int gammaValue = 7;\n";
/// Standalone library with two syntax errors.
const DELTA: &str = "int broken() => ;\nvar x = ;\n";
/// The edit applied on the incremental arm: new bytes, a third `Item`
/// definition, and a third syntax error — ContentId must differ.
const DELTA_EDITED: &str = "int broken() => ;\nvar x = ;\nvar y = '';\nclass Item {}\n";

const LIB_FILES: [(&str, &str); 5] = [
    ("lib/main.dart", MAIN),
    ("lib/alpha.dart", ALPHA),
    ("lib/beta.dart", BETA),
    ("lib/gamma.dart", GAMMA),
    ("lib/delta.dart", DELTA),
];

// ---------------------------------------------------------------------------
// The observable face: machine-format bytes (§19.5 asserts these, not Debug)
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

/// One full cold chain through a fresh engine over the same workspace: CLI
/// discovery (real `read_dir` order in), canonical merged set out. Returns
/// the rendered machine bytes; the discovery order must not move them.
fn cold_chain_bytes(fixture: &Fixture) -> Vec<u8> {
    let mut engine = fixture.engine();
    engine.discover_workspace().expect("discovery");
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &Default::default())
        .expect("analyze");
    let bytes = machine_bytes(&engine, &diagnostics);
    assert!(
        !bytes.is_empty(),
        "corpus must yield diagnostics for the byte pin to be meaningful"
    );
    bytes
}

// ---------------------------------------------------------------------------
// The tuple's faces
// ---------------------------------------------------------------------------

/// Face 1 + face 4: the double analysis, run twice — four cold chains in one
/// process, all byte-identical. Catches fan-out ordering, intern order, and
/// cross-run state leakage (statics, lazy cell caches) in one pin.
#[test]
fn full_chain_bytes_are_identical_across_repeated_runs() {
    let fixture = Fixture::new(&LIB_FILES);
    let first = cold_chain_bytes(&fixture);
    let second = cold_chain_bytes(&fixture);
    let third = cold_chain_bytes(&fixture);
    let fourth = cold_chain_bytes(&fixture);
    assert_eq!(first, second, "double analysis, round 1");
    assert_eq!(second, third, "cross-round stability (first repeat)");
    assert_eq!(third, fourth, "cross-round stability (second repeat)");
}

/// The version-tuple precondition: the SDK identity the outputs are keyed by
/// is content-derived, so repeated loads over the same snapshot carry the
/// same key ([`EngineConfig`] itself holds nothing time- or process-keyed —
/// see the module header).
#[test]
fn engine_version_key_is_stable_across_constructions() {
    let fixture = Fixture::new(&LIB_FILES);
    let sdk_dir = fixture.root.join("sdk");
    let first = SdkSnapshot::load(&sdk_dir, None).expect("first load");
    let second = SdkSnapshot::load(&sdk_dir, None).expect("second load");
    assert_eq!(first.id, second.id, "SDK identity is content-derived");
    assert_eq!(
        first.language_metadata, second.language_metadata,
        "derived language version is load-stable"
    );
}

/// Face 3: cold → edit + `apply_pending` → incremental analyze must equal a
/// fresh full recompute of the same final content, byte for byte (PRD §19.4's
/// incremental==full gate at smoke depth).
#[test]
fn incremental_result_matches_full_recompute_of_final_content() {
    let fixture = Fixture::new(&LIB_FILES);

    // Runs 1+2 in one engine: cold, then the edit through the explicit
    // apply barrier.
    let mut incremental = fixture.engine();
    incremental.discover_workspace().expect("discovery");
    incremental
        .analyze(&[], &mut |_| {}, &Default::default())
        .expect("cold");
    incremental.set_content(&fixture.path("lib/delta.dart"), DELTA_EDITED.as_bytes());
    incremental.apply_pending(&mut |_| {});
    let edited = incremental
        .analyze(&[], &mut |_| {}, &Default::default())
        .expect("incremental");
    let incremental_bytes = machine_bytes(&incremental, &edited);
    assert!(
        !incremental_bytes.is_empty(),
        "edited corpus must keep diagnostics on the face"
    );

    // Run 3: a fresh engine over the same final content.
    let mut full = fixture.engine();
    full.discover_workspace().expect("discovery");
    full.set_content(&fixture.path("lib/delta.dart"), DELTA_EDITED.as_bytes());
    let recomputed = full
        .analyze(&[], &mut |_| {}, &Default::default())
        .expect("full recompute");
    assert_eq!(
        incremental_bytes,
        machine_bytes(&full, &recomputed),
        "incremental result of the final content equals the full recompute"
    );
}
