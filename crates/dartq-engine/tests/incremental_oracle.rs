//! Dual-arm incremental oracle — first-run subset C1/C2/C4/C6 (goal #88; the
//! executable form of `incremental-oracle-cases.md` §1–§3, PRD §19.4's gate:
//! every step of an edit sequence, incremental result == full-recompute
//! result, byte-identical except internal IDs).
//!
//! Per step, over the case's manifest: the long-lived incremental arm
//! (explicit `apply_pending` + `analyze`) is compared against (A1) a fresh
//! full-recompute arm through `write_machine` bytes, (A2) the projected
//! `AffectedReport`, (A3) an absolute (code, file) set, and — on the
//! designated steps — (A4) the ID-free event golden. C6 additionally pins
//! risk 1 (event order ≠ output order) and runs the A5 mode-B arm (single
//! `analyze()` per step; bytes identical, event stream identical to the
//! combined apply+analyze stream).
//!
//! First-run form: the manifests and expectations live as in-test tables.
//! Hoisting them into `tests/cases/<case>/{manifest.json, expectations.json}`
//! (§4 placement) is mechanical and happens at execution bring-up together
//! with C3/C5/C7/C8 (which need the M1 semantic codes).

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use common::Fixture;
use dartq_diagnostics::{AnalysisOutput, Diagnostic, FileLayoutProvider, write_machine};
use dartq_engine::{CancellationToken, Engine, EngineEvent, LibraryId, RevalidateGate};
use dartq_source::FileId;

// ---------------------------------------------------------------------------
// Case tables (incremental-oracle-cases §2, verbatim expectations)
// ---------------------------------------------------------------------------

const C1_A0: &str = "int f() => 1;\n";
const C1_A1: &str = "int f() => 2;\n";
const C1_B0: &str = "import 'a.dart';\nint g() => f();\n";
const C1_B1: &str = "int g() => f() + 0;\n";

const C2_A0: &str = "// v1\nint f() => 1;\n";
const C2_A1: &str = "// v2\nint f() => 1;\n";

const C4_A: &str = "part 'p.dart';\nint h() => p();\n";
const C4_P0: &str = "part of 'a.dart';\nint p() => 1;\n";
const C4_P1: &str = "part of 'a.dart';\nint p() => 2;\n";
const C4_B: &str = "import 'a.dart';\nint g() => h() + p();\n";

const C6_B: &str = "import 'z.dart';\nint x = ;\n";
const C6_M0: &str = "import 'b.dart';\nint x = ;\n";
const C6_M1: &str = "import 'b.dart';\nint x = ; // t\n";
const C6_W: &str = "int w = ;\n";
const C6_Z0: &str = "int f() => 1;\nint y = ;\n";
const C6_Z1: &str = "int f() => 2;\nint y = ;\n";

/// One manifest batch entry: `Some(bytes)` = `set_content`, `None` =
/// `remove_file` (incremental-oracle-cases §1.1).
type Batch = &'static [(&'static str, Option<&'static str>)];

struct Step {
    batch: Batch,
    changed: &'static [&'static str],
    removed: &'static [&'static str],
    invalidated: &'static [&'static str],
}

/// A3 policy: empty (code, file) set at every step, or the step-0 set held
/// constant (C6's syntax errors are constant by construction).
enum Absolute {
    Empty,
    Constant,
}

struct Case {
    id: &'static str,
    initial: &'static [(&'static str, &'static str)],
    steps: &'static [Step],
    absolute: Absolute,
    /// 1-based steps whose output bytes must equal the previous step's.
    stable_bytes: &'static [usize],
    /// Run the A5 mode-B arm (single analyze per step).
    mode_b: bool,
    /// 1-based step carrying the risk-1 dual-order pin.
    order_pin: Option<usize>,
}

/// C1 — body change, gate no-op, closure direction (l1-spec ①).
static CASE_C1: Case = Case {
    id: "C1",
    initial: &[("lib/a.dart", C1_A0), ("lib/b.dart", C1_B0)],
    steps: &[
        Step {
            batch: &[("lib/a.dart", Some(C1_A1))],
            changed: &["lib/a.dart"],
            removed: &[],
            invalidated: &["lib/a.dart", "lib/b.dart"],
        },
        Step {
            batch: &[("lib/a.dart", Some(C1_A1))],
            changed: &[],
            removed: &[],
            invalidated: &[],
        },
        Step {
            batch: &[("lib/b.dart", Some(C1_B1))],
            changed: &["lib/b.dart"],
            removed: &[],
            invalidated: &["lib/b.dart"],
        },
    ],
    absolute: Absolute::Empty,
    stable_bytes: &[],
    mode_b: false,
    order_pin: None,
};

/// C2 — comment-only change; output invariance under the edit (L2 boundary
/// golden: at L2 `invalidated` narrows to [a] — flip then, not before).
static CASE_C2: Case = Case {
    id: "C2",
    initial: &[("lib/a.dart", C2_A0), ("lib/b.dart", C1_B0)],
    steps: &[Step {
        batch: &[("lib/a.dart", Some(C2_A1))],
        changed: &["lib/a.dart"],
        removed: &[],
        invalidated: &["lib/a.dart", "lib/b.dart"],
    }],
    absolute: Absolute::Empty,
    stable_bytes: &[1],
    mode_b: false,
    order_pin: None,
};

/// C4 — part content change, URI form: the belonging edge drags the owning
/// library and its importers into the closure.
static CASE_C4: Case = Case {
    id: "C4",
    initial: &[
        ("lib/a.dart", C4_A),
        ("lib/b.dart", C4_B),
        ("lib/p.dart", C4_P0),
    ],
    steps: &[Step {
        batch: &[("lib/p.dart", Some(C4_P1))],
        changed: &["lib/p.dart"],
        removed: &[],
        invalidated: &["lib/a.dart", "lib/b.dart"],
    }],
    absolute: Absolute::Empty,
    stable_bytes: &[],
    mode_b: false,
    order_pin: None,
};

/// C6 — transitive importer closure + the dual-order golden (syntax-only).
static CASE_C6: Case = Case {
    id: "C6",
    initial: &[
        ("lib/b.dart", C6_B),
        ("lib/m.dart", C6_M0),
        ("lib/w.dart", C6_W),
        ("lib/z.dart", C6_Z0),
    ],
    steps: &[
        Step {
            batch: &[("lib/z.dart", Some(C6_Z1))],
            changed: &["lib/z.dart"],
            removed: &[],
            invalidated: &["lib/b.dart", "lib/m.dart", "lib/z.dart"],
        },
        Step {
            batch: &[("lib/m.dart", Some(C6_M1))],
            changed: &["lib/m.dart"],
            removed: &[],
            invalidated: &["lib/m.dart"],
        },
    ],
    absolute: Absolute::Constant,
    stable_bytes: &[],
    mode_b: true,
    order_pin: Some(1),
};

// ---------------------------------------------------------------------------
// A1 caliber: write_machine bytes over the canonical set (§1.2)
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
// A4 event capture: raw events now, ID-free projection afterwards (§1.3)
// ---------------------------------------------------------------------------

#[derive(Default)]
struct RawLog {
    events: Vec<RawEvent>,
}

enum RawEvent {
    Overlay {
        changed: Vec<FileId>,
        removed: Vec<FileId>,
    },
    Revalidated {
        file: FileId,
        gate: RevalidateGate,
    },
    Analyzed {
        library: LibraryId,
        from_cache: bool,
    },
    Cascade {
        libraries: Vec<LibraryId>,
    },
}

impl RawLog {
    fn sink(&mut self) -> impl FnMut(EngineEvent) + '_ {
        move |event| {
            self.events.push(match event {
                EngineEvent::OverlayApplied { changed, removed } => {
                    RawEvent::Overlay { changed, removed }
                }
                EngineEvent::FileRevalidated { file, gate } => RawEvent::Revalidated { file, gate },
                EngineEvent::LibraryAnalyzed {
                    library,
                    from_cache,
                    ..
                } => RawEvent::Analyzed {
                    library,
                    from_cache,
                },
                EngineEvent::CascadeInvalidated { libraries } => RawEvent::Cascade { libraries },
            });
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GoldenEvent {
    Overlay {
        changed: Vec<String>,
        removed: Vec<String>,
    },
    Revalidated {
        path: String,
        gate: RevalidateGate,
    },
    Analyzed {
        defining: String,
        from_cache: bool,
    },
    Cascade {
        libraries: Vec<String>,
    },
}

fn relative(fixture: &Fixture, file: FileId, engine: &Engine) -> String {
    let path = engine
        .file_table()
        .path(file)
        .expect("event file is resident");
    path.strip_prefix(&fixture.root)
        .expect("fixture-relative path")
        .to_string_lossy()
        .into_owned()
}

fn project(fixture: &Fixture, engine: &Engine, log: &RawLog) -> Vec<GoldenEvent> {
    log.events
        .iter()
        .map(|event| match event {
            RawEvent::Overlay { changed, removed } => GoldenEvent::Overlay {
                changed: changed
                    .iter()
                    .map(|f| relative(fixture, *f, engine))
                    .collect(),
                removed: removed
                    .iter()
                    .map(|f| relative(fixture, *f, engine))
                    .collect(),
            },
            RawEvent::Revalidated { file, gate } => GoldenEvent::Revalidated {
                path: relative(fixture, *file, engine),
                gate: *gate,
            },
            RawEvent::Analyzed {
                library,
                from_cache,
            } => GoldenEvent::Analyzed {
                defining: defining_relative(fixture, engine, *library),
                from_cache: *from_cache,
            },
            RawEvent::Cascade { libraries } => GoldenEvent::Cascade {
                libraries: libraries
                    .iter()
                    .map(|library| defining_relative(fixture, engine, *library))
                    .collect(),
            },
        })
        .collect()
}

fn defining_relative(fixture: &Fixture, engine: &Engine, library: LibraryId) -> String {
    let file = engine
        .library_defining_file(library)
        .expect("golden library is live");
    relative(fixture, file, engine)
}

/// The §3 A4 goldens (step numbers are 1-based manifest steps).
fn expected_golden(case: &str, step: usize) -> Option<Vec<GoldenEvent>> {
    match (case, step) {
        // C1.2: gate-emptied apply — OverlayApplied with empty payloads, the
        // ContentIdHit revalidation, the empty cascade, cache-hit analyses.
        ("C1", 2) => Some(vec![
            GoldenEvent::Overlay {
                changed: vec![],
                removed: vec![],
            },
            GoldenEvent::Revalidated {
                path: "lib/a.dart".into(),
                gate: RevalidateGate::ContentIdHit,
            },
            GoldenEvent::Cascade { libraries: vec![] },
            GoldenEvent::Analyzed {
                defining: "lib/a.dart".into(),
                from_cache: true,
            },
            GoldenEvent::Analyzed {
                defining: "lib/b.dart".into(),
                from_cache: true,
            },
        ]),
        // C6.1: work-order analyses (z, b, m) then the clean target (w) —
        // while the byte output is path-sorted (b, m, w, z).
        ("C6", 1) => Some(vec![
            GoldenEvent::Overlay {
                changed: vec!["lib/z.dart".into()],
                removed: vec![],
            },
            GoldenEvent::Revalidated {
                path: "lib/z.dart".into(),
                gate: RevalidateGate::Parsed {
                    ast_cache_hit: false,
                },
            },
            GoldenEvent::Cascade {
                libraries: vec![
                    "lib/b.dart".into(),
                    "lib/m.dart".into(),
                    "lib/z.dart".into(),
                ],
            },
            GoldenEvent::Analyzed {
                defining: "lib/z.dart".into(),
                from_cache: false,
            },
            GoldenEvent::Analyzed {
                defining: "lib/b.dart".into(),
                from_cache: false,
            },
            GoldenEvent::Analyzed {
                defining: "lib/m.dart".into(),
                from_cache: false,
            },
            GoldenEvent::Analyzed {
                defining: "lib/w.dart".into(),
                from_cache: true,
            },
        ]),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// A2/A3 projections
// ---------------------------------------------------------------------------

fn sorted_paths(fixture: &Fixture, engine: &Engine, files: &[FileId]) -> Vec<String> {
    let mut paths: Vec<String> = files
        .iter()
        .map(|f| relative(fixture, *f, engine))
        .collect();
    paths.sort();
    paths
}

fn sorted_defining(fixture: &Fixture, engine: &Engine, libraries: &[LibraryId]) -> Vec<String> {
    let mut paths: Vec<String> = libraries
        .iter()
        .map(|library| defining_relative(fixture, engine, *library))
        .collect();
    paths.sort();
    paths
}

/// A3's absolute (code, file) set.
fn absolute_set(
    fixture: &Fixture,
    engine: &Engine,
    diagnostics: &[Diagnostic],
) -> BTreeSet<(String, String)> {
    diagnostics
        .iter()
        .map(|diagnostic| {
            let path = engine
                .file_table()
                .path(diagnostic.file)
                .map(|path| {
                    path.strip_prefix(&fixture.root)
                        .unwrap_or(path)
                        .to_string_lossy()
                        .into_owned()
                })
                .unwrap_or_default();
            (diagnostic.code.name.to_owned(), path)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The dual-arm runner (§1.1)
// ---------------------------------------------------------------------------

fn enqueue(
    fixture: &Fixture,
    engine: &mut Engine,
    state: &mut BTreeMap<String, String>,
    batch: Batch,
) {
    let mut entries: Vec<&(&str, Option<&str>)> = batch.iter().collect();
    entries.sort_by_key(|entry| entry.0);
    for (path, content) in entries {
        match content {
            Some(source) => {
                engine.set_content(&fixture.path(*path), source.as_bytes());
                state.insert((*path).to_owned(), (*source).to_owned());
            }
            None => {
                engine
                    .remove_file(&fixture.path(*path))
                    .expect("removed path was interned");
                state.remove(*path);
            }
        }
    }
}

fn analyze_all(
    fixture: &Fixture,
    engine: &mut Engine,
    state: &BTreeMap<String, String>,
    log: &mut RawLog,
) -> Vec<u8> {
    let targets = live_targets(fixture, engine, state);
    let diagnostics = engine
        .analyze(&targets, &mut log.sink(), &CancellationToken::new())
        .expect("analyze");
    machine_bytes(engine, &diagnostics)
}

fn live_targets(
    fixture: &Fixture,
    engine: &Engine,
    state: &BTreeMap<String, String>,
) -> Vec<FileId> {
    state
        .keys()
        .map(|path| {
            engine
                .file_table()
                .try_file_id(&fixture.path(path))
                .expect("live file interned")
        })
        .collect()
}

/// The full arm (§1.1): a fresh engine over the step's end state, one cold
/// `set_content`-all + `analyze`. Only its output bytes matter.
fn full_arm_bytes(fixture: &Fixture, state: &BTreeMap<String, String>) -> Vec<u8> {
    let mut engine = fixture.engine();
    for (path, source) in state {
        engine.set_content(&fixture.path(path), source.as_bytes());
    }
    let mut log = RawLog::default();
    analyze_all(fixture, &mut engine, state, &mut log)
}

fn run_case(case: &'static Case) {
    let fixture = Fixture::new(case.initial);
    let mut state: BTreeMap<String, String> = case
        .initial
        .iter()
        .map(|(p, s)| ((*p).to_owned(), (*s).to_owned()))
        .collect();

    // Incremental arm, mode A (explicit apply per step).
    let mut engine_a = fixture.engine();
    // Incremental arm, mode B (analyze alone — A5 host; only built when the
    // case mandates it).
    let mut engine_b = case.mode_b.then(|| fixture.engine());

    // Step 0: initial state; A1 + A3 baseline only (§2 conventions).
    for (path, source) in case.initial {
        engine_a.set_content(&fixture.path(*path), source.as_bytes());
        if let Some(engine_b) = engine_b.as_mut() {
            engine_b.set_content(&fixture.path(*path), source.as_bytes());
        }
    }
    let mut log_a = RawLog::default();
    let report = engine_a.apply_pending(&mut log_a.sink());
    assert_eq!(
        report.changed.len(),
        case.initial.len(),
        "{} step 0",
        case.id
    );
    let bytes_a = analyze_all(&fixture, &mut engine_a, &state, &mut log_a);
    let full = full_arm_bytes(&fixture, &state);
    assert_eq!(bytes_a, full, "A1 {} step 0", case.id);
    let mut baseline = BTreeSet::new();
    let mut previous_bytes = Some(bytes_a);
    if let (Some(engine_b), true) = (engine_b.as_mut(), case.mode_b) {
        let mut log_b = RawLog::default();
        let bytes_b = analyze_all(&fixture, engine_b, &state, &mut log_b);
        assert_eq!(
            bytes_b,
            previous_bytes.as_deref().expect("step 0 bytes"),
            "A5 {} step 0",
            case.id,
        );
        assert_eq!(
            project(&fixture, engine_b, &log_b),
            project(&fixture, &engine_a, &log_a),
            "A5 event stream {} step 0",
            case.id,
        );
    }

    // Steps 1..n.
    for (index, step) in case.steps.iter().enumerate() {
        let k = index + 1;
        // Mode B reruns the same batch from the same pre-batch state on its
        // own long-lived engine (§1.1 arms are long-lived by definition).
        let mut state_b = state.clone();
        enqueue(&fixture, &mut engine_a, &mut state, step.batch);

        // Mode A: explicit apply (A2) + analyze; one combined event log.
        let mut log_a = RawLog::default();
        let report = engine_a.apply_pending(&mut log_a.sink());
        assert_eq!(
            sorted_paths(&fixture, &engine_a, &report.changed),
            step.changed,
            "A2 changed {} step {k}",
            case.id,
        );
        assert_eq!(
            sorted_paths(&fixture, &engine_a, &report.removed),
            step.removed,
            "A2 removed {} step {k}",
            case.id,
        );
        assert_eq!(
            sorted_defining(&fixture, &engine_a, &report.invalidated),
            step.invalidated,
            "A2 invalidated {} step {k}",
            case.id,
        );
        let diagnostics_a = {
            let targets = live_targets(&fixture, &engine_a, &state);
            engine_a
                .analyze(&targets, &mut log_a.sink(), &CancellationToken::new())
                .expect("analyze")
        };
        let bytes_a = machine_bytes(&engine_a, &diagnostics_a);

        // A1: dual-arm byte equality.
        let full = full_arm_bytes(&fixture, &state);
        assert_eq!(bytes_a, full, "A1 {} step {k}", case.id);

        // A3: absolute (code, file) set.
        let set = absolute_set(&fixture, &engine_a, &diagnostics_a);
        match case.absolute {
            Absolute::Empty => {
                assert!(set.is_empty(), "A3 {} step {k}: {set:?}", case.id);
            }
            Absolute::Constant => {
                if k == 1 {
                    baseline = set.clone();
                }
                assert_eq!(set, baseline, "A3 {} step {k}", case.id);
            }
        }

        // Stable-bytes pins.
        if case.stable_bytes.contains(&k) {
            assert_eq!(
                bytes_a,
                previous_bytes.as_deref().expect("previous step bytes"),
                "output invariance {} step {k}",
                case.id,
            );
        }
        previous_bytes = Some(bytes_a);

        // A5 mode B: identical bytes, identical combined event stream.
        if let (Some(engine_b), true) = (engine_b.as_mut(), case.mode_b) {
            enqueue(&fixture, engine_b, &mut state_b, step.batch);
            let mut log_b = RawLog::default();
            let bytes_b = analyze_all(&fixture, engine_b, &state_b, &mut log_b);
            assert_eq!(bytes_b, full, "A5 {} step {k}", case.id);
            assert_eq!(
                project(&fixture, engine_b, &log_b),
                project(&fixture, &engine_a, &log_a),
                "A5 event stream {} step {k}",
                case.id,
            );
        }

        // A4 golden on the designated step.
        if let Some(expected) = expected_golden(case.id, k) {
            assert_eq!(
                project(&fixture, &engine_a, &log_a),
                expected,
                "A4 {} step {k}",
                case.id,
            );
        }

        // Risk-1 pin: work order (events) ≠ output order (path-sorted bytes).
        if case.order_pin == Some(k) {
            let mut output_order: Vec<String> = Vec::new();
            for diagnostic in &diagnostics_a {
                let path = relative(&fixture, diagnostic.file, &engine_a);
                if output_order.last() != Some(&path) {
                    output_order.push(path);
                }
            }
            let mut sorted_output = output_order.clone();
            sorted_output.sort();
            assert_eq!(
                output_order, sorted_output,
                "output is path-sorted {} step {k}",
                case.id
            );
            let event_order: Vec<String> = project(&fixture, &engine_a, &log_a)
                .iter()
                .filter_map(|event| match event {
                    GoldenEvent::Analyzed { defining, .. } => Some(defining.clone()),
                    _ => None,
                })
                .collect();
            assert_ne!(
                event_order, output_order,
                "risk 1: the two orders differ by construction {} step {k}",
                case.id,
            );
        }
    }
}

#[test]
fn c1_body_change_gate_no_op_closure_direction() {
    run_case(&CASE_C1);
}

#[test]
fn c2_comment_only_change_output_invariant() {
    run_case(&CASE_C2);
}

#[test]
fn c4_part_content_change_belonging_cascade() {
    run_case(&CASE_C4);
}

#[test]
fn c6_transitive_closure_dual_order_pin() {
    run_case(&CASE_C6);
}
