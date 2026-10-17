//! Dual-arm incremental oracle — the full eight-case set C1–C8 (the gate:
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
//! C8's name-form halves are engine-green and its pins hold end to end; C1,
//! C3 and C5's semantic halves light through the wired `SemanticDiagnoser`
//! (#236: the default `semdiag::Diagnoser`, the graph-level dart:core face).
//! Since #282 the cascade is
//! signature-gated: body/comment edits with a stable element face stop at
//! the edited file (C1.1/C2.1/C4.1/C6.1/C8.1 narrow), while a
//! signature-changing edit seeds the unconditional transitive closure (C9).
//! C5 and C7 were long red on pre-existing gaps (C5: the overlay-only part
//! file interning under a different `FileTable` spelling than the resolver
//! composes; C7: dartq-semantic's `UndefinedGuard` over-suppressing bare
//! misses under any unfiltered broken import) — both now pass end to end
//! (C7's A3 table asserts the previously-suppressed `undefined_function`
//! rows), so no case is red; the case comments carry the history.
//!
//! The manifests and expectations live as in-test tables. Hoisting them into
//! `tests/cases/<case>/{manifest.json, expectations.json}` is
//! mechanical and deferred until a consumer needs the files.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use common::Fixture;
use dartq_diagnostics::{AnalysisOutput, Diagnostic, FileLayoutProvider, write_machine};
use dartq_engine::{CancellationToken, Engine, EngineEvent, LibraryId, RevalidateGate};
use dartq_source::FileId;

// ---------------------------------------------------------------------------
// Case tables
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

// C9 reuses C1_A0/C1_B0 for a and b; c closes the chain.
const C9_C: &str = "import 'b.dart';\nint h() => g();\n";
const C9_A_RENAMED: &str = "int f2() => 1;\n";

// C10/C11 — membership-change seeds (audit P0: part detach / include
// detach). Both edit one file yet move a part between membership states (C10
// re-homes it across libraries, C11 dangles one library's include while the
// equally-named sibling keeps it), so the affected sides carry no signature
// of their own.
const C10_M: &str = "import 'n.dart';\nint v = f() + g();\n";
const C10_N: &str = "library p8;\npart 'pn.dart';\nint f() => 1;\n";
const C10_P9: &str = "library p9;\npart 'pn.dart';\nint h() => 2;\n";
const C10_PN0: &str = "part of p8;\nint g() => 1;\n";
const C10_PN1: &str = "part of p9;\nint g() => 1;\n";

const C11_M: &str = "import 'n2.dart';\nint v = h() + g();\n";
const C11_N1_0: &str = "library p8;\npart 'pn.dart';\nint f() => 1;\n";
const C11_N1_1: &str = "library p10;\npart 'pn.dart';\nint f() => 1;\n";
const C11_N2: &str = "library p8;\npart 'pn.dart';\nint h() => 2;\n";
const C11_PN: &str = "part of p8;\nint g() => 1;\n";

/// One manifest batch entry: `Some(bytes)` = `set_content`, `None` =
/// `remove_file`.
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

/// C1 — body change, gate no-op, closure direction (l1-spec ①). The edit
/// keeps f's element face (`int f()`), so the L2 signature gate stops the
/// cascade at a (step 1); step 3's b content `int g() => f() + 0;` drops the
/// `import 'a.dart';` line, so `f` is genuinely undefined there — the
/// identical no-import shape C3 steps 0/2 pin. Adjudicated #288 against a
/// recorded oracle probe (`dart analyze` 3.13.1 run on this exact case): the
/// case doc's ∅ delta was the case defect; the A3 table asserts
/// `undefined_function` on b at step 3 (∅ through steps 0-2, where b still
/// carries the import).
static CASE_C1: Case = Case {
    id: "C1",
    initial: &[("lib/a.dart", C1_A0), ("lib/b.dart", C1_B0)],
    steps: &[
        Step {
            batch: &[("lib/a.dart", Some(C1_A1))],
            changed: &["lib/a.dart"],
            removed: &[],
            // L2: a's signature is unchanged, so b keeps its result.
            invalidated: &["lib/a.dart"],
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
    absolute: Absolute::Explicit(&[
        // Step 0 baseline and steps 1-2: b still imports a, so f resolves.
        &[],
        &[],
        &[],
        // Step 3: C1_B1 drops the import — same shape as C3 step 0 (#288).
        &[("undefined_function", "lib/b.dart")],
    ]),
    stable_bytes: &[],
    mode_b: false,
    order_pin: None,
};

/// C2 — comment-only change; output invariance under the edit, and the L2
/// signature gate keeps the cascade at a: b's semantic products are not
/// invalidated (the A4 golden pins b's cache-hit analysis).
static CASE_C2: Case = Case {
    id: "C2",
    initial: &[("lib/a.dart", C2_A0), ("lib/b.dart", C1_B0)],
    steps: &[Step {
        batch: &[("lib/a.dart", Some(C2_A1))],
        changed: &["lib/a.dart"],
        removed: &[],
        invalidated: &["lib/a.dart"],
    }],
    absolute: Absolute::Empty,
    stable_bytes: &[1],
    mode_b: false,
    order_pin: None,
};

/// C4 — part content change, URI form: p's edit keeps its element face, so
/// the L2 gate stops the cascade at the owning library a — a recomputes
/// (its content identity includes p's bytes), b keeps its result.
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
        invalidated: &["lib/a.dart"],
    }],
    absolute: Absolute::Empty,
    stable_bytes: &[],
    mode_b: false,
    order_pin: None,
};

/// C6 — transitive importer closure + the dual-order golden (syntax-only).
/// Step 1 is a body edit with a stable face, so the L2 gate stops the chain
/// at z; step 2's suffix comment keeps m alone.
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
            invalidated: &["lib/z.dart"],
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
/// never invalidated. Green end to end over the wired semantic diagnoser
/// (#236).
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
/// directive order) and step 2 must round-trip it — over-cascade to b
/// is L1 definition behavior. The semantic half is wired (#236). RED on a
/// pre-existing engine divergence the harness's path projection used to mask:
/// overlay-only p2 (never on disk) interns under the literal `temp_dir`
/// spelling while a's re-resolved `part 'p2.dart'` composes from a's
/// canonicalized path — `FileTable` normalization is existence-dependent, so
/// the two spellings carry different ids, the part claim misses residency,
/// and p2 forms an orphan singleton (`invalidated` gains it; the gap rule
/// would add a bogus `uri_does_not_exist`). Fix belongs to the
/// resolution/formation layer: reconcile freshly resolved targets against
/// residency by canonical URI.
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
/// the orphan rule (a removed library's parts leave in `invalidated`),
/// observed. RED on the semantic half:
/// dartq-semantic's `UndefinedGuard` suppresses a bare miss under ANY
/// unfiltered broken import, so `u()` after util's removal is silenced — the
/// oracle ignores only show-listed names (element.dart:7470-7502) and
/// expects the pair. The guard's own doc defers the tightening to
/// show-clause enumeration; the fix belongs in dartq-semantic. The URI half
/// is verified green.
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

/// C8 — name-form `part of` + rename orphan. Green end to end: the URI layer skips name-form part-of
/// resolution, the bounded same-package name scan claims pn, the
/// capture-time duplicate log bakes step 0's `duplicate_definition`, and the
/// wired semantic diagnoser reports m's `g()` loss after the rename orphan
/// leaves n's scope (#236). Step 1's pn edit removes the duplicate and
/// changes pn's body under a stable face — the L2 gate keeps m clean (its
/// view of `g` is untouched) while n recomputes (its content identity
/// includes pn's bytes) and the duplicate diagnostic clears.
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
            invalidated: &["lib/n.dart"],
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
        // The rename orphans pn (no p8 remains to claim it) and n's dangling
        // include reports its name mismatch (the oracle's partOfDifferentLibrary
        // fires on the failed join); m's loss of g strands its import, but the
        // oracle still spares it — the bare miss consulted the namespace, so
        // unused_import must NOT co-fire (oracle-probed 2026-10-18, #2086:
        // dart analyze 3.13.1 emits undefined_function + part_of_different_
        // library only; 4 further family probes never co-fire).
        &[
            ("part_of_different_library", "lib/n.dart"),
            ("undefined_function", "lib/m.dart"),
        ],
    ]),
    stable_bytes: &[],
    mode_b: false,
    order_pin: None,
};

// ---------------------------------------------------------------------------
// A1 caliber: write_machine bytes over the canonical set
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
// A4 event capture: raw events now, ID-free projection afterwards
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
                // A budget tier firing is not part of the incremental
                // contract (this suite never arms a budget) — skipped.
                // A version-skew degrade needs a skewed SDK whitelist —
                // this suite's synthetic SDK names none — so it is
                // likewise outside the contract (ticket #1304). A
                // span-capacity rejection needs an over-4GiB source —
                // likewise outside the contract (#1543).
                EngineEvent::MemoryDegrade { .. } => return,
                EngineEvent::ExperimentDegrade { .. } => return,
                EngineEvent::SourceRejected { .. } => return,
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
    // The path-universe invariant (#287): every interned path — disk or
    // overlay-only — carries the canonical root spelling (dartq-pub
    // canonicalizes package roots, and FileTable derives the same universe
    // for missing paths), so the canonical root is the one strip base.
    let canonical = fixture.root.canonicalize().expect("fixture root resolves");
    path.strip_prefix(&canonical)
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

/// The C1–C8 A4 goldens (step numbers are 1-based manifest steps).
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
        // C2.1: the L2 zero-cascade win — a recomputes (its bytes moved), b's
        // semantic products stay cached (signature unchanged).
        ("C2", 1) => Some(vec![
            GoldenEvent::Overlay {
                changed: vec!["lib/a.dart".into()],
                removed: vec![],
            },
            GoldenEvent::Revalidated {
                path: "lib/a.dart".into(),
                gate: RevalidateGate::Parsed {
                    ast_cache_hit: false,
                },
            },
            GoldenEvent::Cascade {
                libraries: vec!["lib/a.dart".into()],
            },
            GoldenEvent::Analyzed {
                defining: "lib/a.dart".into(),
                from_cache: false,
            },
            GoldenEvent::Analyzed {
                defining: "lib/b.dart".into(),
                from_cache: true,
            },
        ]),
        // C6.1: the signature gate stops the cascade at z — z recomputes,
        // the b/m/w chain rides from cache — while the byte output stays
        // path-sorted (b, m, w, z).
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
                libraries: vec!["lib/z.dart".into()],
            },
            GoldenEvent::Analyzed {
                defining: "lib/z.dart".into(),
                from_cache: false,
            },
            GoldenEvent::Analyzed {
                defining: "lib/b.dart".into(),
                from_cache: true,
            },
            GoldenEvent::Analyzed {
                defining: "lib/m.dart".into(),
                from_cache: true,
            },
            GoldenEvent::Analyzed {
                defining: "lib/w.dart".into(),
                from_cache: true,
            },
        ]),
        // C8.1: pn's duplicate collapse keeps its face stable, so the L2
        // gate dirties only n (pn's bytes sit in n's content identity); m
        // keeps its result — its view of g() is untouched.
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
                libraries: vec!["lib/n.dart".into()],
            },
            GoldenEvent::Analyzed {
                defining: "lib/n.dart".into(),
                from_cache: false,
            },
            GoldenEvent::Analyzed {
                defining: "lib/m.dart".into(),
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
// The dual-arm runner
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
        if let Some(source) = content {
            engine.set_content(&fixture.path(path), source.as_bytes());
            state.insert((*path).to_owned(), (*source).to_owned());
        } else {
            engine
                .remove_file(&fixture.path(path))
                .expect("removed path was interned");
            state.remove(*path);
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

/// The full arm: a fresh engine over the step's end state, one cold
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

#[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
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

    // Step 0: initial state; A1 + A3 baseline only.
    for (path, source) in case.initial {
        engine_a.set_content(&fixture.path(path), source.as_bytes());
        if let Some(engine_b) = engine_b.as_mut() {
            engine_b.set_content(&fixture.path(path), source.as_bytes());
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
        // own long-lived engine.
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

/// C9 — signature-changing edit, transitive cascade through clean middles
///: renaming a.dart's `f` flips its API
/// signature, seeding the closure; the chain a←b←c propagates **through** b
/// even though b and c are identity-clean (their own faces never moved) —
/// the unconditional-closure adjudication. Step 2 reverts the rename: the
/// diagnostics heal, and the invalidated set stays the full chain, proving
/// the closure is signature-driven rather than diagnostic-driven.
static CASE_C9: Case = Case {
    id: "C9",
    initial: &[
        ("lib/a.dart", C1_A0),
        ("lib/b.dart", C1_B0),
        ("lib/c.dart", C9_C),
    ],
    steps: &[
        Step {
            batch: &[("lib/a.dart", Some(C9_A_RENAMED))],
            changed: &["lib/a.dart"],
            removed: &[],
            invalidated: &["lib/a.dart", "lib/b.dart", "lib/c.dart"],
        },
        Step {
            batch: &[("lib/a.dart", Some(C1_A0))],
            changed: &["lib/a.dart"],
            removed: &[],
            invalidated: &["lib/a.dart", "lib/b.dart", "lib/c.dart"],
        },
    ],
    absolute: Absolute::Explicit(&[
        // Step 0: the chain resolves cleanly.
        &[],
        // Step 1: b's `f()` went undefined; c still sees b's `g()`.
        &[("undefined_function", "lib/b.dart")],
        // Step 2: the revert heals b.
        &[],
    ]),
    stable_bytes: &[],
    mode_b: false,
    order_pin: None,
};

#[test]
fn c9_signature_rename_cascades_through_clean_middles() {
    run_case(&CASE_C9);
}

/// C10 — part detach (audit P0 hole 1): pn edits its own `part of` from p8
/// to p9, moving itself from n's library to p9's. pn is the only changed
/// file, but the LOSING library n is identity-stale with identity-clean
/// importers: m keeps its cached "`g` defined" view of n's namespace while a
/// full recompute reports `undefined_function` — the pre-fix silent
/// divergence. The membership seeds (pre-reform owner capture in `apply` +
/// the owner diff in `close`) must seed n so m recomputes.
static CASE_C10: Case = Case {
    id: "C10",
    initial: &[
        ("lib/m.dart", C10_M),
        ("lib/n.dart", C10_N),
        ("lib/p9.dart", C10_P9),
        ("lib/pn.dart", C10_PN0),
    ],
    steps: &[Step {
        batch: &[("lib/pn.dart", Some(C10_PN1))],
        changed: &["lib/pn.dart"],
        removed: &[],
        invalidated: &["lib/m.dart", "lib/n.dart", "lib/p9.dart"],
    }],
    absolute: Absolute::Explicit(&[
        // Baseline: m sees f() (n) and g() (pn through n); p9's competing
        // include of the n-claimed pn reports its name mismatch.
        &[("part_of_different_library", "lib/p9.dart")],
        // n's namespace drops g with pn (its dangling include reports the
        // mismatch); m's stale view must recompute.
        &[
            ("part_of_different_library", "lib/n.dart"),
            ("undefined_function", "lib/m.dart"),
        ],
    ]),
    stable_bytes: &[],
    mode_b: false,
    order_pin: None,
};

#[test]
fn c10_part_detach_seeds_losing_library_importers() {
    run_case(&CASE_C10);
}

/// C11 — include detach under per-library membership (audit P0 hole 2): n1
/// and n2 both declare `library p8;` with a `part 'pn.dart';` directive, so
/// pn is a MEMBER of both libraries — the baseline is clean (m resolves h()
/// and g() through n2's namespace; the oracle's element layer links a
/// name-matched part into every matching library), while pn's attribution
/// owner is still the path-smallest claimant n1. Renaming n1's directive
/// away dangles only n1's include — its name-mismatch row fires on
/// recompute — while n2's membership never moved (pn's bytes and n2's files
/// are unchanged), so n2 keeps its cached result and m its view through it:
/// the invalidated set is n1 alone. The pre-membership engine flipped pn's
/// ownership between the libraries here and had to recompute both sides;
/// membership makes the transfer invisible outside n1.
static CASE_C11: Case = Case {
    id: "C11",
    initial: &[
        ("lib/m.dart", C11_M),
        ("lib/n1.dart", C11_N1_0),
        ("lib/n2.dart", C11_N2),
        ("lib/pn.dart", C11_PN),
    ],
    steps: &[Step {
        batch: &[("lib/n1.dart", Some(C11_N1_1))],
        changed: &["lib/n1.dart"],
        removed: &[],
        invalidated: &["lib/n1.dart"],
    }],
    absolute: Absolute::Explicit(&[
        // Baseline: both equally-named libraries include pn, so m's g()
        // through n2 resolves — the oracle is clean.
        &[],
        // The transfer leaves n1's include dangling (its name mismatch
        // fires); n2's namespace still carries pn's g(), so m stays clean.
        &[("part_of_different_library", "lib/n1.dart")],
    ]),
    stable_bytes: &[],
    mode_b: false,
    order_pin: None,
};

#[test]
fn c11_include_detach_keeps_equal_named_sibling_namespace() {
    run_case(&CASE_C11);
}

/// `part 'b.dart';` whose target is a plain library: the target belongs to
/// neither membership nor the dep set, so its residency flip is visible only
/// through the part-target staleness identity. The cold baseline pins the
/// plain-library include's `part_of_non_part` (the oracle's partOfNonPart,
/// unconditionally); removing the target dirties the owning library and
/// surfaces the owed `uri_does_not_exist`, byte-identical to a full
/// recompute; the re-add round-trip (fresh library id for the target,
/// part-target flip for the owner) must settle back to the cold full
/// recompute.
#[test]
fn unclaimed_part_target_removal_matches_full_recompute() {
    const A: &str = "part 'b.dart';\nint f() => 1;\n";
    const B: &str = "int g() => 2;\n";
    let fixture = Fixture::new(&[("lib/a.dart", A), ("lib/b.dart", B)]);
    let b_disk = fixture.path("lib/b.dart");
    let mut engine = fixture.engine();

    // Cold baseline: the part target resolves (on disk) but is a plain
    // library, so the include reports part_of_non_part.
    engine.set_content(&fixture.path("lib/a.dart"), A.as_bytes());
    engine.set_content(&b_disk, B.as_bytes());
    engine.apply_pending(&mut |_| {});
    let mut state: BTreeMap<String, String> = BTreeMap::from([
        ("lib/a.dart".to_owned(), A.to_owned()),
        ("lib/b.dart".to_owned(), B.to_owned()),
    ]);
    let mut log = RawLog::default();
    let cold = analyze_all(&fixture, &mut engine, &state, &mut log);
    let cold_set = code_file_set(&[("part_of_non_part", "lib/a.dart")]);
    assert_eq!(
        absolute_set(&fixture, &engine, &cold),
        cold_set,
        "cold baseline pins the plain-library include: {:?}",
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
    assert_eq!(
        absolute_set(&fixture, &engine, &diagnostics),
        cold_set,
        "re-add settles back to the cold face: {:?}",
        absolute_set(&fixture, &engine, &diagnostics)
    );
    assert_eq!(
        machine_bytes(&engine, &diagnostics),
        full_arm_bytes(&fixture, &state),
        "A1 re-add step"
    );
}

/// Adjacent-string URI concatenation (audit P1, coverage P3.7c): the
/// directive URI is the concatenation of adjacent string literals —
/// `import 'a' '.dart';` resolves `a.dart` — so the resolver must see the
/// joined value, not the prefix. Unlike the dual-arm cases this defect is
/// arm-symmetric (a full recompute mis-resolves identically), so the sharp
/// assertion is direct: every concatenated form resolves against real files
/// and the whole workspace is diagnostic-free. Covers all three `read_uri`
/// consumers: `import` (main), `part` (host), `part of` URI-form (p).
/// Pre-fix this fails with `uri_does_not_exist` on the prefixes (`'a'`,
/// `'p'`, `'host'`) and m's `undefined_function` for the unclaimed part's
/// `w()`.
#[test]
fn adjacent_string_uri_concatenates_before_resolution() {
    const MAIN: &str = "import 'a' '.dart';\nint v = t();\n";
    const HOST: &str = "part 'p' '.dart';\nint u() => 3;\n";
    const P: &str = "part of 'host' '.dart';\nint w() => 4;\n";
    const A: &str = "int t() => 1;\n";
    const M: &str = "import 'host.dart';\nint x = u() + w();\n";
    let initial = [
        ("lib/a.dart", A),
        ("lib/host.dart", HOST),
        ("lib/m.dart", M),
        ("lib/main.dart", MAIN),
        ("lib/p.dart", P),
    ];
    let fixture = Fixture::new(&initial);
    let state: BTreeMap<String, String> = initial
        .iter()
        .map(|(path, source)| ((*path).to_owned(), (*source).to_owned()))
        .collect();
    let mut engine = fixture.engine();
    for (path, source) in initial {
        engine.set_content(&fixture.path(path), source.as_bytes());
    }
    engine.apply_pending(&mut |_| {});
    let mut log = RawLog::default();
    let diagnostics = analyze_all(&fixture, &mut engine, &state, &mut log);
    assert!(
        diagnostics.is_empty(),
        "concatenated URIs resolve cleanly: {:?}",
        absolute_set(&fixture, &engine, &diagnostics)
    );
    assert_eq!(
        machine_bytes(&engine, &diagnostics),
        full_arm_bytes(&fixture, &state),
        "A1 adjacent-string baseline"
    );
}

/// Cancelled-analyze leftovers survive the next unrelated edit (audit #428
/// P0): a library invalidated because a DEPENDENCY's face moved carries that
/// staleness nowhere in its own identity, so the identity re-derivation in
/// `close` cannot rediscover it — the apply after a cancelled analyze must
/// seed the leftover set into its dirty set. Sequence: cold analyze (l1
/// imports l2) → signature-changing edit on l2 → pre-cancelled analyze (the
/// l1/l2 leftovers stay invalidated, nothing recomputes) → body edit on
/// unrelated u (stable face, unreachable from any seed) → apply + analyze.
/// Pre-fix, l1's cached result — computed against l2's pre-edit face — is
/// served from then on: the rename's `undefined_function` never appears and
/// the bytes diverge from a fresh engine's full recompute, permanently
/// (incremental ≠ full recompute with no repair path).
#[test]
fn cancelled_analyze_leftover_survives_an_unrelated_edit() {
    const L1: &str = "import 'l2.dart';\nint f() => g();\n";
    const L2_0: &str = "int g() => 1;\n";
    const L2_1: &str = "int g2() => 1;\n"; // the rename flips l2's face
    const U_0: &str = "int u() => 3;\n";
    const U_1: &str = "int u() => 4;\n"; // body edit, stable face

    let fixture = Fixture::new(&[
        ("lib/l1.dart", L1),
        ("lib/l2.dart", L2_0),
        ("lib/u.dart", U_0),
    ]);
    let mut state: BTreeMap<String, String> = BTreeMap::from([
        ("lib/l1.dart".to_owned(), L1.to_owned()),
        ("lib/l2.dart".to_owned(), L2_0.to_owned()),
        ("lib/u.dart".to_owned(), U_0.to_owned()),
    ]);
    let mut engine = fixture.engine();
    for (path, source) in &state {
        engine.set_content(&fixture.path(path), source.as_bytes());
    }
    engine.apply_pending(&mut |_| {});
    let mut log = RawLog::default();
    let cold = analyze_all(&fixture, &mut engine, &state, &mut log);
    assert!(cold.is_empty(), "cold baseline is clean: {cold:?}");

    // Signature-changing edit on l2, analyzed under a pre-cancelled token:
    // the run stops before any recompute, and the l1/l2 leftovers stay
    // invalidated.
    engine.set_content(&fixture.path("lib/l2.dart"), L2_1.as_bytes());
    state.insert("lib/l2.dart".to_owned(), L2_1.to_owned());
    let token = CancellationToken::new();
    token.cancel();
    assert!(matches!(
        engine.analyze(&[], &mut |_| {}, &token),
        Err(dartq_engine::Cancelled)
    ));

    // Unrelated body edit: u's library is identity-stale and re-derived;
    // identity-fresh l1 must carry its invalidation through this apply
    // instead of being dropped by the assignment.
    engine.set_content(&fixture.path("lib/u.dart"), U_1.as_bytes());
    state.insert("lib/u.dart".to_owned(), U_1.to_owned());
    let report = engine.apply_pending(&mut |_| {});
    let l1 = engine
        .file_table()
        .try_file_id(&fixture.path("lib/l1.dart"))
        .expect("interned");
    let l1_library = engine.library_of(l1).expect("live");
    assert!(
        report.invalidated.contains(&l1_library),
        "the closure-stale leftover carries forward: {:?}",
        sorted_defining(&fixture, &engine, &report.invalidated)
    );

    // The full-recompute face: l1 sees g vanish and says so.
    let mut log = RawLog::default();
    let diagnostics = engine
        .analyze(&[], &mut log.sink(), &CancellationToken::new())
        .expect("analyze");
    assert_eq!(
        absolute_set(&fixture, &engine, &diagnostics),
        code_file_set(&[("undefined_function", "lib/l1.dart")]),
        "l1 recomputes against l2's new face: {diagnostics:?}"
    );
    assert_eq!(
        machine_bytes(&engine, &diagnostics),
        full_arm_bytes(&fixture, &state),
        "A1 cancel-then-edit"
    );
}
