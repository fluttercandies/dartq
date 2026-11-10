//! #3643 `unused_import` oracle-parity pins — the four faces this ticket
//! moved onto the upstream decision (`generateUnusedImportWarnings`,
//! imports_verifier.dart:230-268, which walks `fileAnalysis.unit.
//! directives` — THIS file's written imports — and credits by element
//! attribution):
//!
//! - **per-file edge alignment** (`LibraryGraph::import_runs` slicing): a
//!   part file's pass aligns `self.imports` against its OWN run of the
//!   regrouped edge vector, never the library-wide head or a sibling
//!   part's edges — `scope_A08_t01`'s part-file `import ... as lN` sites.
//! - **per-file combined scope** (`lookup_in_file`): a bare type-use /
//!   annotation head resolves through the diagnosing file's combined
//!   import scope — a part's own `import 'dart:math'` answers `Random` in
//!   that part even though the library-level namespace never sees it
//!   (`test_part_usedPartImport`).
//! - **unresolved-URI `''` staging**: `edge.target = None` with an empty
//!   recovered URI still reports `unused_import` (`import ''`, the
//!   unquoted-imports family, directive recovery) while a named-but-
//!   missing target keeps the `isOriginNotExistingFile` skip.
//! - **literal-receiver extension credit**: `0[1]`, `7(9)`, `'abc'-'c'`,
//!   `-'abc'` fold into inert operator/invoke rows upstream, but
//!   `notifyExtensionUsed` still marks the applied extension element —
//!   the token-scan derives `(receiver type head, member)` pairs.
//!
//! Oracle: dart 3.13.7, upstream `pkg/analyzer`.

use super::super::{FileReport, StrictModes, diagnose_with};
use crate::collector::collect;
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder, Namespaces,
};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The `unused_import` rows of `r`, as (offset, length) pairs in source
/// order — the pins module's own `import_rows`, kept private there.
fn import_rows(r: &FileReport) -> Vec<(u32, u32)> {
    r.diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_import")
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// One aux file of the import rig: the body and the written prefix (if
/// any) — the pins rig's `Aux` shape, kept crate-visible there. `body`
/// empty means the written import's URI resolved to no library
/// (`edge.target = None`) — the `import ''` / missing-file face.
struct Aux {
    prefix: Option<&'static str>,
    body: &'static str,
}

/// [`report_with_imports`] variant for the non-part pins: `source` is the
/// sole analyzed file, each [`Aux`] collected+declared into its own graph
/// library and wired through a plain import edge owned by `main.dart`.
fn report_with_imports(source: &str, aux: &[Aux]) -> FileReport {
    let main_imports: Vec<(usize, Option<&'static str>)> =
        aux.iter().enumerate().map(|(i, a)| (i, a.prefix)).collect();
    report_with_part_file_imports(source, None, &main_imports, &[], aux)
}

/// [`report_with_imports`] with a PART unit whose own written imports are
/// edge-attributed to the part file: the graph's `import_in` owner rows
/// give `import_runs` a real per-file slice, and `builder.part` wires the
/// include so the combined-scope chain climbs part → defining unit.
/// `main_imports`/`part_imports` are `(aux_index, prefix)` pairs — the
/// edge owner each written import belongs to — so a main source that
/// writes no `import` never gets an unattributed edge starving its count
/// gate, and the part unit's written imports land on ITS run.
/// Returns the merged report of every diagnosed unit (the defining unit
/// plus, when `part_source` is present, the part-file pass — each pass's
/// `unused_import` rows anchor at their own file through
/// `Diagnostic.file`, exactly the row set the census's per-file oracle
/// emits).
fn report_with_part_file_imports(
    source: &str,
    part_source: Option<&'static str>,
    main_imports: &[(usize, Option<&'static str>)],
    part_imports: &[(usize, Option<&'static str>)],
    aux: &[Aux],
) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    // EnhancedParts is a still-experimental flag at 3.15 — the part
    // unit's own `import`/`export` directives only parse under it, the
    // same opt-in the oracle's shared-options harness turns on for the
    // Parts-with-imports corpus.
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15))
        .with_experiment(dartq_language::Feature::EnhancedParts);
    let features = ParserFeatures::from_feature_set(set);
    let scanned = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {source:?}"
    );
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    builder.unit(lib, file);
    builder.seed_implicit_core(
        lib,
        file,
        &[
            "Object", "String", "int", "bool", "double", "num", "List", "Map", "void",
        ],
    );
    // The part unit (when present): declared into the same library under
    // its own file id, include-linked to the defining unit, and carrying
    // its OWN import edges — `import_in` attributes each edge to
    // `part_file`, so `import_runs` regroups them out of the library-wide
    // head.
    let part_data = part_source.map(|body| {
        let part_file = files.intern(Path::new("p.dart"));
        let scanned_part = dartq_scanner::scan(
            body,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            part_file,
        );
        assert!(
            scanned_part.diagnostics.is_empty(),
            "scanner diagnostics: {body}"
        );
        let part_elements = collect(body, &scanned_part.tokens, features, &strings);
        for (name, decl) in part_elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, part_file);
        }
        builder.unit(lib, part_file);
        builder.part(lib, file, part_file);
        (part_file, scanned_part, part_elements)
    });
    let mut aux_libs = Vec::new();
    let mut aux_units = Vec::new();
    for (position, item) in aux.iter().enumerate() {
        if item.body.is_empty() {
            // An unresolved import: no library node, no file, no decls —
            // `edge.target = None` below.
            aux_libs.push(None);
            continue;
        }
        let path = format!("a{position}.dart");
        let aux_file = files.intern(Path::new(&path));
        let scanned_aux = dartq_scanner::scan(
            item.body,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            aux_file,
        );
        assert!(
            scanned_aux.diagnostics.is_empty(),
            "scanner diagnostics: {}",
            item.body
        );
        let aux_elements = collect(item.body, &scanned_aux.tokens, features, &strings);
        let aux_lib = builder.intern(&format!("package:t/{path}"));
        for (name, decl) in aux_elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(aux_lib, name, aux_file);
        }
        aux_libs.push(Some(aux_lib));
        aux_units.push((aux_file, aux_elements));
    }
    // The defining unit's own written imports — attributed to `file` via
    // `import_in` so `import_runs` groups them into `main.dart`'s run
    // alongside the part file's own run (library-wide `builder.import`
    // rows would leave `main.dart` run-less and starve the count gate).
    // Only `main_imports` land here: a main source that writes no import
    // keeps a zero-edge run and a zero-length `self.imports`.
    for &(aux_index, prefix) in main_imports.iter() {
        builder.import_in(
            lib,
            file,
            ImportEdge {
                target: aux_libs[aux_index],
                prefix: prefix.map(|p| strings.intern(p)),
                deferred: false,
                non_library: false,
                combinators: Combinators::new(std::iter::empty(), &strings),
            },
        );
    }
    // The part-file import edges: written against aux libraries in
    // declaration order, attributed to `part_file` (`builder.import_in`,
    // owner=Some(part_file)).
    if let Some((part_file, _, _)) = &part_data {
        for &(aux_index, prefix) in part_imports.iter() {
            builder.import_in(
                lib,
                *part_file,
                ImportEdge {
                    target: aux_libs[aux_index],
                    prefix: prefix.map(|p| strings.intern(p)),
                    deferred: false,
                    non_library: false,
                    combinators: Combinators::new(std::iter::empty(), &strings),
                },
            );
        }
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    if let Some((part_file, _, part_elements)) = &part_data {
        index.insert(*part_file, part_elements.clone());
    }
    for (aux_file, aux_elements) in aux_units {
        index.insert(aux_file, aux_elements);
    }
    let mut report = diagnose_with(
        source,
        &scanned.tokens,
        features,
        &strings,
        file,
        lib,
        &graph,
        &namespaces,
        &index,
        StrictModes::default(),
        &LibraryClassModifiers::default(),
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
    );
    // The part unit is a separate compilation unit with its own
    // `self.imports` — `diagnose_with` is per-file, so the part-file
    // pass runs as a second fold over the same graph (the augment_rig
    // precedent) and its `unused_import` rows merge into the report.
    if let Some((part_file, scanned_part, _)) = &part_data {
        let part_report = diagnose_with(
            part_source.expect("part_data implies part_source"),
            &scanned_part.tokens,
            features,
            &strings,
            *part_file,
            lib,
            &graph,
            &namespaces,
            &index,
            StrictModes::default(),
            &LibraryClassModifiers::default(),
            set.is_enabled(dartq_language::Feature::InferenceUpdate2),
        );
        report.diagnostics.extend(part_report.diagnostics);
    }
    report
}

// ---------------------------------------------------------------------
// Face 1: per-file edge alignment (`import_runs` slicing).
// ---------------------------------------------------------------------

/// `scope_A08_t01`: a part file writes `import ... as lN` directives —
/// the part-file pass must align `self.imports` against the part's OWN
/// run of edges (the regrouped vector's `Some(part_file)` slice), never
/// the library-wide head or a sibling unit's edges. Before the slice
/// the pass read the WHOLE `import_edges` vector (prelude + the
/// defining unit's edges + the part's), died on
/// `edges.len() != self.imports.len()`, and silently skipped every
/// `unused_import` row the part file owed.
///
/// The pin: the part's own import IS used inside the part (`p1.T`
/// below — a prefix spelling no library declaration collides with;
/// upstream's `_lookupLibrary` answers library declarations before
/// prefix elements, so a `p1`-spelled top-level name would steal the
/// `lN.T` family `scope_A08_t01` itself never exercises) but the
/// DEFINING unit's import is not — the defining unit's report must
/// carry exactly its own unused edge's row, proving the part-file run
/// sliced out of its slice rather than being credited or blamed
/// across units.
#[test]
fn part_file_run_aligns_the_part_own_edges() {
    // `main.dart` writes one unused import; the part writes one it USES.
    let source = "import 'a0.dart';\n\
                  part 'p.dart';\n\
                  main() {}\n";
    let part_source = "part of 'main.dart';\n\
                       import 'a1.dart' as p1;\n\
                       void f(p1.T _) {}\n";
    let aux = [
        Aux {
            prefix: None,
            body: "class A0 {}\n",
        },
        Aux {
            prefix: None,
            body: "class T {}\n",
        },
    ];
    // The defining unit's written import is a0 — unused there; the
    // part's written import is a1 (used via `p1.T` inside the part).
    let report = report_with_part_file_imports(
        source,
        Some(part_source),
        &[(0, None)],
        &[(1, Some("p1"))],
        &aux,
    );
    let uri = source.find("'a0.dart'").unwrap() as u32;
    assert_eq!(
        import_rows(&report),
        vec![(uri, 9)],
        "source: {source}\npart: {part_source}"
    );
}

/// The negative arm: when the part-file's own import IS the unused one
/// and the defining unit writes none, the part file reports its own row —
/// the slice does not starve the part-file pass behind the count gate.
#[test]
fn part_file_own_unused_import_reports_on_the_part() {
    let source = "part 'p.dart';\n\
                  int l1 = 1;\n\
                  main() {}\n";
    let part_source = "part of 'main.dart';\n\
                       import 'a0.dart' as l1;\n\
                       void f() {}\n";
    let aux = [Aux {
        prefix: None,
        body: "class T {}\n",
    }];
    let report =
        report_with_part_file_imports(source, Some(part_source), &[], &[(0, Some("l1"))], &aux);
    // The part-file row anchors at the part file's URI literal (the
    // `Diagnostic.file` carries `p.dart`; `range` is byte-in-part).
    let uri = part_source.find("'a0.dart'").unwrap() as u32;
    assert_eq!(
        import_rows(&report),
        vec![(uri, 9)],
        "source: {source}\npart: {part_source}"
    );
}

// ---------------------------------------------------------------------
// Face 2: per-file combined scope (`lookup_in_file`).
// ---------------------------------------------------------------------

/// `test_part_usedPartImport`: the part's own `import 'a0.dart'` answers
/// `Random` inside the part (`Random _`), but the LIBRARY-level
/// `namespaces.lookup` the old code ran asks the defining unit's scope —
/// which never sees the part's imports — so the bare type-use resolved
/// `NotFound`, no `bare_decls` entry formed, the import read unused, and
/// the row over-reported. `lookup_in_file(self.file)` walks the part's
/// combined scope chain (its own rows, then part-parent up to the
/// defining unit) and credits the part's edge.
#[test]
fn part_own_import_credits_the_part_file_use() {
    let source = "part 'p.dart';\n\
                  main() {}\n";
    let part_source = "part of 'main.dart';\n\
                       import 'a0.dart';\n\
                       void f(Random _) {}\n";
    let aux = [Aux {
        prefix: None,
        body: "class Random {}\n",
    }];
    let report = report_with_part_file_imports(source, Some(part_source), &[], &[(0, None)], &aux);
    assert_eq!(
        import_rows(&report),
        Vec::<(u32, u32)>::new(),
        "source: {source}\npart: {part_source}"
    );
}

/// The defining unit's own bare use stays library-scoped — a part-file
/// import must NOT credit a name the defining unit reads (enhanced-
/// parts strict shadowing: the defining unit's combined scope never
/// reaches down into a part's imports).
#[test]
fn part_own_import_does_not_credit_the_library_scope() {
    // The defining unit references `Random` — visible only via ITS OWN
    // import scope (dart:core's seeded `Random` is absent — the seed
    // carries no `Random`, so the name resolves through the part's edge
    // ONLY inside the part; in the defining unit it stays unresolved).
    let source = "part 'p.dart';\n\
                  void g(Random _) {}\n";
    let part_source = "part of 'main.dart';\n\
                       import 'a0.dart';\n\
                       void f() {}\n";
    let aux = [Aux {
        prefix: None,
        body: "class Random {}\n",
    }];
    let report = report_with_part_file_imports(source, Some(part_source), &[], &[(0, None)], &aux);
    let uri = part_source.find("'a0.dart'").unwrap() as u32;
    assert_eq!(
        import_rows(&report),
        vec![(uri, 9)],
        "source: {source}\npart: {part_source}"
    );
}

// ---------------------------------------------------------------------
// Face 3: unresolved-URI `''` staging vs missing-file skip.
// ---------------------------------------------------------------------

/// `import ''`: the URI literal parses to an empty string, no library
/// resolves (`edge.target = None`), and the oracle still reports
/// `unused_import` with `relativeUriString = ''` — the directive is a
/// real written import whose URI happens to be empty.
#[test]
fn empty_uri_reports_unused_import() {
    let source = "import '';\nmain() {}\n";
    // The `import ''` directive resolves to no library — `target: None`
    // (an `Aux` with `body: ""` produces exactly that edge).
    let report = report_with_imports(
        source,
        &[Aux {
            prefix: None,
            body: "",
        }],
    );
    let uri = source.find("''").unwrap() as u32;
    assert_eq!(import_rows(&report), vec![(uri, 2)], "source: {source}");
}

/// `import 'missing.dart'`: a named-but-missing target keeps the
/// `isOriginNotExistingFile` skip — the `uri_does_not_exist` face owns
/// the directive already; `unused_import` must not double-report. The
/// rig still wires the `target: None` edge (`Aux` with `body: ""`) so
/// the skip arm is the non-empty-URI one, not "no edge at all".
#[test]
fn missing_file_uri_stays_silent_on_unused_import() {
    let source = "import 'missing.dart';\nmain() {}\n";
    let report = report_with_imports(
        source,
        &[Aux {
            prefix: None,
            body: "",
        }],
    );
    assert_eq!(
        import_rows(&report),
        Vec::<(u32, u32)>::new(),
        "source: {source}"
    );
}

// ---------------------------------------------------------------------
// Face 4: literal-receiver extension credit.
// ---------------------------------------------------------------------

/// `0[1]`: the index-read applies `[]` on a literal `int` receiver —
/// the aux `E`'s `int operator [](int i)` resolves through implicit
/// application upstream (`notifyExtensionUsed`), so the import stays
/// used even though no identifier spelling reached `used_names`.
#[test]
fn literal_index_read_credits_the_extension_import() {
    let source = "import 'a0.dart';\n\
                  void f() { var x = 0[1]; }\n";
    let aux = [Aux {
        prefix: None,
        body: "extension E on int { int operator [](int i) => i; }\n",
    }];
    let report = report_with_imports(source, &aux);
    assert_eq!(
        import_rows(&report),
        Vec::<(u32, u32)>::new(),
        "source: {source}"
    );
}

/// `7(9)`: the invoke shape applies `call` on a literal `int` receiver.
#[test]
fn literal_invoke_credits_the_call_extension_import() {
    let source = "import 'a0.dart';\n\
                  void f() { var x = 7(9); }\n";
    let aux = [Aux {
        prefix: None,
        body: "extension E on int { int call(int i) => i; }\n",
    }];
    let report = report_with_imports(source, &aux);
    assert_eq!(
        import_rows(&report),
        Vec::<(u32, u32)>::new(),
        "source: {source}"
    );
}

/// `'abc'-'c'`: the binary `-` applies on a literal `String` receiver.
#[test]
fn literal_binary_op_credits_the_extension_import() {
    let source = "import 'a0.dart';\n\
                  void f() { var x = 'abc'-'c'; }\n";
    let aux = [Aux {
        prefix: None,
        body: "extension E on String { String operator -(String o) => this; }\n",
    }];
    let report = report_with_imports(source, &aux);
    assert_eq!(
        import_rows(&report),
        Vec::<(u32, u32)>::new(),
        "source: {source}"
    );
}

/// `-'abc'`: the prefix `-` applies the unary member on a literal
/// `String` receiver — the aux's `String operator -()` (zero-arity
/// minus) is the unary face.
#[test]
fn literal_unary_minus_credits_the_extension_import() {
    let source = "import 'a0.dart';\n\
                  void f() { var x = -'abc'; }\n";
    let aux = [Aux {
        prefix: None,
        body: "extension E on String { String operator -() => this; }\n",
    }];
    let report = report_with_imports(source, &aux);
    assert_eq!(
        import_rows(&report),
        Vec::<(u32, u32)>::new(),
        "source: {source}"
    );
}

/// The negative: a literal application whose member the extension does
/// NOT declare leaves the import unused — `1 % 2` applies `%`, and the
/// aux face only declares `[]`. Proves the credit is member-granular,
/// not "any literal use credits any extension on the same type".
#[test]
fn unmatched_literal_application_keeps_the_import_unused() {
    let source = "import 'a0.dart';\n\
                  void f() { var x = 1 % 2; }\n";
    let aux = [Aux {
        prefix: None,
        body: "extension E on int { int operator [](int i) => i; }\n",
    }];
    let report = report_with_imports(source, &aux);
    let uri = source.find("'a0.dart'").unwrap() as u32;
    assert_eq!(import_rows(&report), vec![(uri, 9)], "source: {source}");
}
