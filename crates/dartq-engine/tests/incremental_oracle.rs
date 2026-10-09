//! Dual-arm incremental oracle — the full eight-case set C1–C8 (the
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
//! C8 is doc-faithful and currently RED on substance, for two engine-side
//! reasons: the engine builds part-of belonging edges from URI-resolved
//! targets only, so a name-form `part of p8;` file never joins library n,
//! and the URI layer mis-resolves the name-form directive itself (it emits a
//! bogus `uri_with_interpolation` at offset 0). C3/C5/C7 are red only on
//! their `undefined_function` entries: the fixture's engine has
//! `semdiag: None`, and no `SemanticDiagnoser` adapter can be written
//! test-side yet because `LibraryView` exposes neither the source text nor
//! the token table `dartq_semantic::diagnose` needs. Their URI-layer halves
//! are verified green (C7 step 1 fires exactly the contracted
//! `uri_does_not_exist` pair over a green A2 + A1). Faithful red over fake
//! green; see the case comments.
//!
//! The manifests and expectations live as in-test tables. Hoisting them into
//! `tests/cases/<case>/{manifest.json, expectations.json}` (§4 placement) is
//! mechanical and deferred until a consumer needs the files.

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

// C3 reuses C1_A0 for a and C1_B0 for the import-bearing step-1/step-4
// content (the case doc spells step 1 as exactly that text).
const C3_B0: &str = "int g() => f();\n";
const C3_B3: &str = "import 'missing.dart';\nimport 'a.dart';\nint g() => f();\n";

const C5_A0: &str = "part 'p1.dart';\nint u = r();\nint f() => q();\n";
const C5_A1: &str = "part 'p1.dart';\npart 'p2.dart';\nint u = r();\nint f() => q();\n";
const C5_P1: &str = "part of 'a.dart';\nint q() => 1;\n";
const C5_P2: &str = "part of 'a.dart';\nint r() => 2;\n";

const C7_A: &str =
    "import 'util.dart';\npart 'pa.dart';\npart 'pb.dart';\nint h() => u() + pa() + pb();\n";
const C7_B: &str = "import 'a.dart';\nimport 'util.dart';\nint g() => h() + u();\n";
const C7_PA: &str = "part of 'a.dart';\nint pa() => 1;\n";
const C7_PB: &str = "part of 'a.dart';\nint pb() => 1;\n";
const C7_UTIL: &str = "int u() => 1;\n";

const C8_M: &str = "import 'n.dart';\nint v = g();\n";
const C8_N0: &str = "library p8;\npart 'pn.dart';\nint f() => 1;\n";
const C8_N1: &str = "library p9;\npart 'pn.dart';\nint f() => 1;\n";
const C8_PN0: &str = "part of p8;\nint g() => 1;\nint g() => 2;\n";
const C8_PN1: &str = "part of p8;\nint g() => 3;\n";

/// One manifest batch entry: `Some(bytes)` = `set_content`, `None` =
/// `remove_file` (incremental-oracle-cases §1.1).
type Batch = &'static [(&'static str, Option<&'static str>)];

struct Step {
    batch: Batch,
    changed: &'static [&'static str],
    removed: &'static [&'static str],
    invalidated: &'static [&'static str],
}

/// A3 policy: empty (code, file) set at every step, the step-0 set held
/// constant (C6's syntax errors are constant by construction), or an
/// explicit per-step set — index 0 is step 0, k is step k (the case docs
/// for C3/C5/C7/C8 list step-0 diagnostics, so Explicit asserts the
/// baseline too).
enum Absolute {
    Empty,
    Constant,
    Explicit(&'static [&'static [(&'static str, &'static str)]]),
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

/// C3 — directive add/remove: import (l1-spec ③): the add fixes
/// `undefined_function`, the remove re-breaks it, a second import to an
/// unresolvable target adds `uri_does_not_exist`, and dropping it heals
/// again. b's directive changes rebuild b's outgoing edges only, so a is
/// never invalidated. KNOWN RED (shared with C5/C7): the
/// `undefined_function` entries need a wired `SemanticDiagnoser`, which no
/// `LibraryView` consumer can provide yet.
static CASE_C3: Case = Case {
    id: "C3",
    initial: &[("lib/a.dart", C1_A0), ("lib/b.dart", C3_B0)],
    steps: &[
        Step {
            batch: &[("lib/b.dart", Some(C1_B0))],
            changed: &["lib/b.dart"],
            removed: &[],
            invalidated: &["lib/b.dart"],
        },
        Step {
            batch: &[("lib/b.dart", Some(C3_B0))],
            changed: &["lib/b.dart"],
            removed: &[],
            invalidated: &["lib/b.dart"],
        },
        Step {
            batch: &[("lib/b.dart", Some(C3_B3))],
            changed: &["lib/b.dart"],
            removed: &[],
            invalidated: &["lib/b.dart"],
        },
        Step {
            batch: &[("lib/b.dart", Some(C1_B0))],
            changed: &["lib/b.dart"],
            removed: &[],
            invalidated: &["lib/b.dart"],
        },
    ],
    absolute: Absolute::Explicit(&[
        &[("undefined_function", "lib/b.dart")],
        &[],
        &[("undefined_function", "lib/b.dart")],
        &[("uri_does_not_exist", "lib/b.dart")],
        &[],
    ]),
    stable_bytes: &[],
    mode_b: false,
    order_pin: None,
};

/// C5 — part add/remove: library shape change (l1-spec ⑤). One batch edits a
/// and interns p2; the membership churn rebuilds a's content identity (part
/// directive order, §7.4) and step 2 must round-trip it — over-cascade to b
/// is L1 definition behavior. KNOWN RED: see CASE_C3 (the `r()` miss at
/// steps 0/2 needs the unwired semantic diagnoser).
static CASE_C5: Case = Case {
    id: "C5",
    initial: &[
        ("lib/a.dart", C5_A0),
        ("lib/b.dart", C1_B0),
        ("lib/p1.dart", C5_P1),
    ],
    steps: &[
        Step {
            batch: &[("lib/a.dart", Some(C5_A1)), ("lib/p2.dart", Some(C5_P2))],
            changed: &["lib/a.dart", "lib/p2.dart"],
            removed: &[],
            invalidated: &["lib/a.dart", "lib/b.dart"],
        },
        Step {
            batch: &[("lib/a.dart", Some(C5_A0)), ("lib/p2.dart", None)],
            changed: &["lib/a.dart"],
            removed: &["lib/p2.dart"],
            invalidated: &["lib/a.dart", "lib/b.dart"],
        },
    ],
    absolute: Absolute::Explicit(&[
        &[("undefined_function", "lib/a.dart")],
        &[],
        &[("undefined_function", "lib/a.dart")],
    ]),
    stable_bytes: &[],
    mode_b: false,
    order_pin: None,
};

/// C7 — delete file, edge cleanup, orphan part (l1-spec ⑦). Removals are
/// overlay-only; the residency-gap rule at analysis time (a directive whose
/// target left residency carries `uri_does_not_exist`) is what surfaces the
/// deletions in both arms. Step 3: library a ceases, pa's `part of 'a.dart'`
/// dangles, and pa reappears in `invalidated` as a newly-formed singleton —
/// the engine-api-spec §1.2 orphan rule, observed. KNOWN RED: the
/// `undefined_function` entries need the unwired semantic diagnoser (see
/// CASE_C3); the URI half is verified green.
static CASE_C7: Case = Case {
    id: "C7",
    initial: &[
        ("lib/a.dart", C7_A),
        ("lib/b.dart", C7_B),
        ("lib/pa.dart", C7_PA),
        ("lib/pb.dart", C7_PB),
        ("lib/util.dart", C7_UTIL),
    ],
    steps: &[
        Step {
            batch: &[("lib/util.dart", None)],
            changed: &[],
            removed: &["lib/util.dart"],
            invalidated: &["lib/a.dart", "lib/b.dart"],
        },
        Step {
            batch: &[("lib/pb.dart", None)],
            changed: &[],
            removed: &["lib/pb.dart"],
            invalidated: &["lib/a.dart", "lib/b.dart"],
        },
        Step {
            batch: &[("lib/a.dart", None)],
            changed: &[],
            removed: &["lib/a.dart"],
            invalidated: &["lib/b.dart", "lib/pa.dart"],
        },
    ],
    absolute: Absolute::Explicit(&[
        &[],
        // util gone: both importers lose u() and gain the URI error.
        &[
            ("undefined_function", "lib/a.dart"),
            ("uri_does_not_exist", "lib/a.dart"),
            ("undefined_function", "lib/b.dart"),
            ("uri_does_not_exist", "lib/b.dart"),
        ],
        // pb gone: a's new part-gap URI error and pb() miss collapse into
        // the same (code, file) set entries as step 1.
        &[
            ("undefined_function", "lib/a.dart"),
            ("uri_does_not_exist", "lib/a.dart"),
            ("undefined_function", "lib/b.dart"),
            ("uri_does_not_exist", "lib/b.dart"),
        ],
        // a gone: b loses h() (its util errors persist); pa is an orphan
        // singleton whose `part of 'a.dart'` now dangles.
        &[
            ("undefined_function", "lib/b.dart"),
            ("uri_does_not_exist", "lib/b.dart"),
            ("uri_does_not_exist", "lib/pa.dart"),
        ],
    ]),
    stable_bytes: &[],
    mode_b: false,
    order_pin: None,
};

/// C8 — name-form `part of` + rename orphan (l1-spec §7.1, engine-api-spec
/// §5.3). KNOWN RED on substance, two engine-side causes. First, the URI
/// layer mis-resolves the name-form directive: `walk_part` records
/// `uri: None`, `resolve_unit` still pushes it through `resolve`, and the
/// null-URI branch codes it `uri_with_interpolation` at offset 0 — pn's only
/// step-0 diagnostic, where the contract expects `duplicate_definition`.
/// Second, belonging edges claim URI-resolved `part` targets only
/// (`reform_libraries` matches `ResolvedTarget::Resolved`), so pn stays an
/// orphan singleton: g never enters n's scope and step 1's cascade cannot
/// reach [m, n]. Expectations stay doc-faithful (faithful red over fake
/// green); the pins flip when URI resolution skips name-form part-of and
/// the §7.1 bounded same-package name scan lands.
static CASE_C8: Case = Case {
    id: "C8",
    initial: &[
        ("lib/m.dart", C8_M),
        ("lib/n.dart", C8_N0),
        ("lib/pn.dart", C8_PN0),
    ],
    steps: &[
        Step {
            batch: &[("lib/pn.dart", Some(C8_PN1))],
            changed: &["lib/pn.dart"],
            removed: &[],
            invalidated: &["lib/m.dart", "lib/n.dart"],
        },
        Step {
            batch: &[("lib/n.dart", Some(C8_N1))],
            changed: &["lib/n.dart"],
            removed: &[],
            invalidated: &["lib/m.dart", "lib/n.dart", "lib/pn.dart"],
        },
    ],
    absolute: Absolute::Explicit(&[
        &[("duplicate_definition", "lib/pn.dart")],
        &[],
        // The §5.4-TBD orphan-part entry is deferred (no code landed for a
        // name-form orphan); m's loss of g is the pinned remainder.
        &[("undefined_function", "lib/m.dart")],
    ]),
    stable_bytes: &[],
    mode_b: false,
    order_pin: None,
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
    // dartq-pub canonicalizes package roots, so on macOS interned paths live
    // under /private/var while temp_dir() spells /var: strip against the
    // canonical root, not the literal one.
    let root = fixture.root.canonicalize().expect("fixture root resolves");
    path.strip_prefix(&root)
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
        // C8.1: intended-world golden — with the name-form edge, pn is a
        // member of library n, so the cascade dirties n and its importer m
        // (id order: m interned first) and both reanalyze in topological
        // order (n before m). Unreachable while CASE_C8's A3/A2 gaps stand;
        // written to the §3 contract so the pin activates with the fix.
        ("C8", 1) => Some(vec![
            GoldenEvent::Overlay {
                changed: vec!["lib/pn.dart".into()],
                removed: vec![],
            },
            GoldenEvent::Revalidated {
                path: "lib/pn.dart".into(),
                gate: RevalidateGate::Parsed {
                    ast_cache_hit: false,
                },
            },
            GoldenEvent::Cascade {
                libraries: vec!["lib/m.dart".into(), "lib/n.dart".into()],
            },
            GoldenEvent::Analyzed {
                defining: "lib/n.dart".into(),
                from_cache: false,
            },
            GoldenEvent::Analyzed {
                defining: "lib/m.dart".into(),
                from_cache: false,
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

/// A3's absolute (code, file) set. Paths go through `relative` so the set
/// carries fixture-relative names under the canonical root (the same #229
/// discipline as every other projection here).
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

/// The `Explicit` A3 table entry as a comparable set.
fn code_file_set(entries: &[(&'static str, &'static str)]) -> BTreeSet<(String, String)> {
    entries
        .iter()
        .map(|(code, path)| ((*code).to_owned(), (*path).to_owned()))
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
) -> Vec<Diagnostic> {
    let targets = live_targets(fixture, engine, state);
    engine
        .analyze(&targets, &mut log.sink(), &CancellationToken::new())
        .expect("analyze")
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
    let diagnostics = analyze_all(fixture, &mut engine, state, &mut log);
    machine_bytes(&engine, &diagnostics)
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
    let diagnostics_a = analyze_all(&fixture, &mut engine_a, &state, &mut log_a);
    let bytes_a = machine_bytes(&engine_a, &diagnostics_a);
    let full = full_arm_bytes(&fixture, &state);
    assert_eq!(bytes_a, full, "A1 {} step 0", case.id);
    let mut baseline = BTreeSet::new();
    match case.absolute {
        // Explicit contracts list step-0 diagnostics; assert the baseline.
        Absolute::Explicit(expected) => {
            assert_eq!(
                absolute_set(&fixture, &engine_a, &diagnostics_a),
                code_file_set(expected[0]),
                "A3 {} step 0",
                case.id
            );
        }
        Absolute::Constant => {
            baseline = absolute_set(&fixture, &engine_a, &diagnostics_a);
        }
        Absolute::Empty => {}
    }
    let mut previous_bytes = Some(bytes_a);
    if let (Some(engine_b), true) = (engine_b.as_mut(), case.mode_b) {
        let mut log_b = RawLog::default();
        let diagnostics_b = analyze_all(&fixture, engine_b, &state, &mut log_b);
        let bytes_b = machine_bytes(engine_b, &diagnostics_b);
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
        let diagnostics_a = analyze_all(&fixture, &mut engine_a, &state, &mut log_a);
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
                assert_eq!(set, baseline, "A3 {} step {k}", case.id);
            }
            Absolute::Explicit(expected) => {
                assert_eq!(set, code_file_set(expected[k]), "A3 {} step {k}", case.id);
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
            let diagnostics_b = analyze_all(&fixture, engine_b, &state_b, &mut log_b);
            let bytes_b = machine_bytes(engine_b, &diagnostics_b);
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

#[test]
fn c3_directive_add_remove_import() {
    run_case(&CASE_C3);
}

#[test]
fn c5_part_add_remove_library_shape() {
    run_case(&CASE_C5);
}

#[test]
fn c7_delete_file_edge_cleanup_orphan_part() {
    run_case(&CASE_C7);
}

#[test]
fn c8_name_form_part_rename_orphan() {
    run_case(&CASE_C8);
}

/// `part 'b.dart';` whose target is a plain library: the target belongs to
/// neither membership nor the dep set, so its residency flip is visible only
/// through the part-target staleness identity. Removing it must dirty the
/// owning library and surface the owed `uri_does_not_exist`, byte-identical
/// to a full recompute; the re-add round-trip (fresh library id for the
/// target, part-target flip for the owner) must settle back to the clean
/// full recompute.
#[test]
fn unclaimed_part_target_removal_matches_full_recompute() {
    const A: &str = "part 'b.dart';\nint f() => 1;\n";
    const B: &str = "int g() => 2;\n";
    let fixture = Fixture::new(&[("lib/a.dart", A), ("lib/b.dart", B)]);
    let b_disk = fixture.path("lib/b.dart");
    let mut engine = fixture.engine();

    // Cold baseline: the part target resolves (on disk), nothing owed.
    engine.set_content(&fixture.path("lib/a.dart"), A.as_bytes());
    engine.set_content(&b_disk, B.as_bytes());
    engine.apply_pending(&mut |_| {});
    let mut state: BTreeMap<String, String> = BTreeMap::from([
        ("lib/a.dart".to_owned(), A.to_owned()),
        ("lib/b.dart".to_owned(), B.to_owned()),
    ]);
    let mut log = RawLog::default();
    let cold = analyze_all(&fixture, &mut engine, &state, &mut log);
    assert!(
        cold.is_empty(),
        "cold baseline is clean: {:?}",
        absolute_set(&fixture, &engine, &cold)
    );

    // Remove the target — from the overlay and the workspace, so the full
    // arm re-parses against the same absent disk the ticket's scenario
    // describes.
    engine.remove_file(&b_disk).expect("removal enqueued");
    std::fs::remove_file(&b_disk).expect("disk removal");
    assert!(state.remove("lib/b.dart").is_some());
    let report = engine.apply_pending(&mut |_| {});
    assert_eq!(
        sorted_defining(&fixture, &engine, &report.invalidated),
        vec!["lib/a.dart".to_owned()],
        "the part-target flip dirties the owning library"
    );
    let mut log = RawLog::default();
    let diagnostics = engine
        .analyze(&[], &mut log.sink(), &CancellationToken::new())
        .expect("analyze");
    assert_eq!(
        absolute_set(&fixture, &engine, &diagnostics),
        code_file_set(&[("uri_does_not_exist", "lib/a.dart")]),
        "the owed uri diagnostic appears"
    );
    assert_eq!(
        machine_bytes(&engine, &diagnostics),
        full_arm_bytes(&fixture, &state),
        "A1 removal step"
    );

    // Re-add: a fresh library id for the target and an already-stale owner —
    // both recompute and the pair settles back to the clean full recompute.
    std::fs::write(&b_disk, B).expect("disk re-add");
    engine.set_content(&b_disk, B.as_bytes());
    state.insert("lib/b.dart".to_owned(), B.to_owned());
    engine.apply_pending(&mut |_| {});
    let mut log = RawLog::default();
    let diagnostics = engine
        .analyze(&[], &mut log.sink(), &CancellationToken::new())
        .expect("analyze");
    assert!(
        diagnostics.is_empty(),
        "re-add settles clean: {diagnostics:?}"
    );
    assert_eq!(
        machine_bytes(&engine, &diagnostics),
        full_arm_bytes(&fixture, &state),
        "A1 re-add step"
    );
}
