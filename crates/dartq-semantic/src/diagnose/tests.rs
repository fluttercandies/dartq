
use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{CombinatorClause, Combinators, LibraryGraphBuilder};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports.
/// Core-library names are deliberately absent: an M1 harness must model
/// them through concrete declarations or shown broken edges — see the
/// undefined-guard module docs.
fn report(source: &str) -> FileReport {
    report_with_edge(source, |_| None)
}

/// [`report`], plus one extra import edge built with the live string
/// table (combinator names need interning).
fn report_with_edge(
    source: &str,
    make_edge: impl FnOnce(&StringTable) -> Option<ImportEdge>,
) -> FileReport {
    report_with(source, StrictModes::default(), make_edge)
}

/// [`report`] with the #480 language-strictness modes set.
fn report_strict(source: &str, strict: StrictModes) -> FileReport {
    report_with(source, strict, |_| None)
}

/// [`report`] with the `variance` experiment granted (#612): the
/// explicit-keyword faces additionally arm.
fn report_variance(source: &str) -> FileReport {
    report_granted(source, true)
}

/// [`report`] with the augmentations gate OFF — the recorded engine
/// configuration for plain 3.13 (the engine's `scan_config` folds the
/// feature set, and `augmentations` carries no `enabledIn`, so `augment`
/// lexes as an identifier): the #1445 fixture faces.
fn report_augmentations_off(source: &str) -> FileReport {
    report_granted_features(source, false, StrictModes::default(), |_| None, false)
}

/// [`report`] with the `variance` experiment explicitly withheld — the
/// legacy superinterface face still answers; the explicit-keyword faces
/// stay gated (#742).
fn report_ungranted(source: &str) -> FileReport {
    report_granted(source, false)
}

/// The shared harness: one file, one library, the live pipeline.
fn report_with(
    source: &str,
    strict: StrictModes,
    make_edge: impl FnOnce(&StringTable) -> Option<ImportEdge>,
) -> FileReport {
    report_granted_features(source, false, strict, make_edge, true)
}

/// [`report_with`] with the experiment grant switch.
fn report_granted(source: &str, variance: bool) -> FileReport {
    report_granted_features(source, variance, StrictModes::default(), |_| None, true)
}

fn report_granted_features(
    source: &str,
    variance: bool,
    strict: StrictModes,
    make_edge: impl FnOnce(&StringTable) -> Option<ImportEdge>,
    augmentations: bool,
) -> FileReport {
    report_full_pipeline(source, variance, strict, make_edge, augmentations, false)
}

/// [`report`] over a source whose scanner tier carries recovery rows
/// (#1823 corpus faces: illegal-character identifier merges,
/// unterminated literals). The scanner rows themselves drop — the faces
/// under pin are the parse and semantic tiers; production merges the
/// scanner rows beside these (`l1.rs:823`) and the corpus pins own
/// their text.
fn report_with_scanner_residue(source: &str) -> FileReport {
    report_full_pipeline(source, false, StrictModes::default(), |_| None, true, true)
}

fn report_full_pipeline(
    source: &str,
    variance: bool,
    strict: StrictModes,
    make_edge: impl FnOnce(&StringTable) -> Option<ImportEdge>,
    augmentations: bool,
    tolerate_scanner_diagnostics: bool,
) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let scanned = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: augmentations,
        },
        file,
    );
    assert!(
        tolerate_scanner_diagnostics || scanned.diagnostics.is_empty(),
        "scanner diagnostics: {source:?}"
    );
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let set = if variance {
        set.with_experiment(dartq_language::Feature::Variance)
    } else {
        set
    };
    let features = ParserFeatures::from_feature_set(set);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    if let Some(edge) = make_edge(&strings) {
        builder.import(lib, edge);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let fold = fold_unit(
        source,
        &scanned.tokens,
        features,
        &strings,
        file,
        lib,
        &graph,
        &namespaces,
        &index,
        strict,
        &LibraryClassModifiers::default(),
        // The same derivation the engine's fold call site makes from
        // the settled set (the `inference-update-2` release gate).
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
    );
    // The full oracle face: the parse-recovery rows merge ahead of the
    // semantic rows (parse precedes resolution; ties keep the parse
    // row) — the production pipeline's canonical merge serves the same
    // two halves side by side.
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect::<Vec<_>>()
}

/// #2165: the callee-adjacency and source-peeking helpers read UTF-16
/// row offsets against the byte source. A non-ASCII char earlier in the
/// file must not shift them: `catch (e)` after a `try/finally` keeps the
/// invocation face (`undefined_function` on `catch`, `undefined_identifier`
/// on `e` only), exactly as in an all-ASCII file (oracle 3.13.1, probed
/// with an em dash in a leading comment).
#[test]
fn non_ascii_prefix_keeps_callee_adjacency() {
    let source = "// \u{2014}\nvoid t() {\n  try {\n  } finally {\n  } catch (e) {\n  }\n}\n";
    let names = code_names(&report(source));
    assert!(names.contains(&"undefined_function"), "{names:?}");
    assert_eq!(
        names
            .iter()
            .filter(|n| **n == "undefined_identifier")
            .count(),
        1,
        "{names:?}"
    );
}

/// #2165: an unnamed factory keys as `new` in the duplicate-constructor
/// pass, and a named duplicate's row spans the type name through the
/// selector (`errorRange`; oracle 3.13.1 `class K { K(); K(); K.a();
/// K.a(); factory K.b() => K(); K.b(); }` → cols 16/28/57, the named
/// rows 3 wide; `const L(); factory L(..)` → the factory's `L`).
#[test]
fn duplicate_constructor_counts_factories_and_spans_selector() {
    let source = "class K { K(); K(); K.a(); K.a(); factory K.b() => K(); K.b(); }\n\
                      class L { const L(); factory L(String v) => L(); }\n";
    let r = report(source);
    let rows: Vec<(u32, u32)> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "duplicate_constructor")
        .map(|d| (d.range.offset, d.range.length))
        .collect();
    assert_eq!(rows, vec![(15, 1), (27, 3), (56, 3), (94, 1)], "{rows:?}");
}

/// [`report`] over a library wired to a synthetic `dart:core` face node
/// (the `report_with_sdk_faces` shape): the names the #2172 pins
/// consume resolve through the namespace while their unit stays
/// unindexed — exactly the CLI's SDK posture the static-return table
/// and the frozen core faces answer under.
fn report_with_core_faces(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let core_file = files.intern(Path::new("<dart:core>"));
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
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let core = builder.intern("dart:core");
    for name in ["RangeError", "Iterator", "Comparable"] {
        builder.declare(core, strings.intern(name), core_file);
    }
    builder.import(
        lib,
        ImportEdge {
            target: Some(core),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    diagnose_with(
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
    )
}

/// [`report`] over a library wired to synthetic `dart:async` and
/// `dart:io` face nodes — the engine's `build_library_graph` SDK shape:
/// names declared into unindexed `<dart:...>` files, joined by unfiltered
/// unprefixed import edges. The faces carry exactly the names this
/// engine's evidence corpus consumes (each verified at its upstream
/// declaration surface — the `FutureOr` union typedef the front end
/// synthesizes into dart:async, `io/process.dart`'s `Process` family).
fn report_with_sdk_faces(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
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
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let async_lib = builder.intern("dart:async");
    let io_lib = builder.intern("dart:io");
    let async_file = files.intern(Path::new("<dart:async>"));
    let io_file = files.intern(Path::new("<dart:io>"));
    for (node, sdk_file, names) in [
        (async_lib, async_file, &["FutureOr", "Future", "Stream"][..]),
        (io_lib, io_file, &["Process", "ProcessResult"][..]),
    ] {
        for name in names {
            builder.declare(node, strings.intern(name), sdk_file);
        }
        builder.import(
            lib,
            ImportEdge {
                target: Some(node),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
    }
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    diagnose_with(
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
    )
}

/// The SDK name-table face (evidence `future_or.dart` /
/// `issue_49477.dart`): a properly imported dart:async/dart:io name
/// resolves — the bare `FutureOr<int>` type head carries no
/// `undefined_class`, the `await Process.start('')` value use no
/// `undefined_identifier` (the oracle reports zero rows in both
/// fixtures' shapes).
#[test]
fn imported_sdk_async_and_io_names_resolve() {
    let report = report_with_sdk_faces(
        "import 'dart:async';\nimport 'dart:io';\n\
             class int {}\n\
             FutureOr<int> f(FutureOr<int> x) => x;\n\
             void main() async {\n  await Process.start('', []);\n}\n",
    );
    assert!(report.diagnostics.is_empty(), "{:?}", code_names(&report));
}

/// [`report`] over a library wired to a synthetic unindexed
/// `<dart:core>` carrying the collection heads — the SDK projection's
/// names-only stub shape: a collection head in a value position
/// resolves through an unindexed file (the generic class type
/// literal's resolution grade).
fn report_with_core_stubs(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
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
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let core_lib = builder.intern("dart:core");
    let core_file = files.intern(Path::new("<dart:core>"));
    for name in ["List", "Set", "Map", "Iterable"] {
        builder.declare(core_lib, strings.intern(name), core_file);
    }
    builder.import(
        lib,
        ImportEdge {
            target: Some(core_lib),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    diagnose_with(
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
    )
}

/// The throw expression's static type is `Never` (the resolver records
/// `NeverTypeImpl`; `subtype.dart` Right Bottom) — an initializer
/// `b = throw 'x'` assigns into `bool` with no `invalid_assignment` row
/// (evidence `promotion_in_dead_code.dart:106`, the lane's single F-I
/// row: dartq typed the throw as its operand's `String`). `bool` is
/// modeled concretely (the harness has no core names).
#[test]
fn throw_expression_types_as_never() {
    let report = report("class bool {}\nclass C {\n  bool b;\n  C() : b = throw 'x';\n}\n");
    assert!(report.diagnostics.is_empty(), "{:?}", code_names(&report));
}

/// A bare undefined read reports `undefined_identifier` with the oracle's
/// verbatim text; the miss also lands in the names table.
///
/// The static-nullability trio with primitive literal types (#350): a
/// String literal into an int parameter / variable / return host reports
/// each verbatim text; the same-name and int-literal-in-double-context
/// shapes stay silent (the oracle's literal conversion).
///
/// Hand-trace: `f('s')` — the argument flush attributes the single
/// String-typed row; the formal's declared `int` answers
/// [`Ty::assignable_from`] `None` (distinct named types), the final-
/// primitive table decides `false`. The declaration initializer and the
/// return check read the same table.
#[test]
fn primitive_literals_drive_the_assignability_trio() {
    let report = report(
        "class int {} class double {} \
             void f(int x) {} int g() { return 's'; } \
             void main() { f('s'); int y = 's'; double d = 1; int ok = 1; }",
    );
    assert_eq!(
        code_names(&report),
        [
            "return_of_invalid_type",
            "argument_type_not_assignable",
            // `y`/`d`/`ok` are never read: the official
            // `_isUsefulRead`/`_isUsedElement` split counts reads only,
            // so each declaration also reports `unused_local_variable`
            // at its name (source order interleaves with the assignability rows).
            "unused_local_variable",
            "invalid_assignment",
            "unused_local_variable",
            "unused_local_variable",
        ],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "A value of type 'String' can't be returned from the function 'g' because it has a return type of 'int'."
    );
    assert_eq!(
        report.diagnostics[1].message,
        "The argument type 'String' can't be assigned to the parameter type 'int'. "
    );
    assert_eq!(
        report.diagnostics[3].message,
        "A value of type 'String' can't be assigned to a variable of type 'int'."
    );
}

/// The promotion consumption (#477 S1): a read promoted by an `is`
/// test types its assignment sites with the promoted type, so the
/// promoted branch's `int y = x;` is clean while the identical
/// unpromoted shape still reports `invalid_assignment` against the
/// declared `int?` (the official read's static type IS the promoted
/// one, `property_element_resolver.dart:2158-2166`).
///
/// Hand-trace: flow emits one `PromotedRead` on the branch's `x` read
/// (`int` narrows `int?`, no type arguments — the #611 gate open); the
/// teardown join keys on the read's exact span and substitutes, the
/// re-run verdict answers `Some(true)`, the staged row never emits.
#[test]
fn promotion_cleans_only_the_promoted_branch_assignment() {
    let report = report(
        "class int {} \
             void promoted() { int? x = 1; if (x is int) { int y = x; } } \
             void unpromoted() { int? x = 1; int y = x; }",
    );
    let assignments: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .collect();
    assert_eq!(assignments.len(), 1, "{:?}", code_names(&report));
    assert_eq!(
        assignments[0].message,
        "A value of type 'int?' can't be assigned to a variable of type 'int'."
    );
}

/// #2167: the `is` type-test on a `&&` LHS promotes the variable into
/// the RHS, so the RHS member read resolves on the promoted type — the
/// app corpus's generated `==` override shape
/// (`other is Money11 && other.cents == cents`, one shape × ~100
/// dartq-only `undefined_getter` rows; the oracle is silent). The
/// pass-2 promoted-receiver re-decision (#1465) reads the flow type at
/// the receiver read's span; the static face being non-nullable
/// (`Object other`) is not an exit — only the `!` pre-assert widening
/// is (#2161's same-head nullable flow face).
#[test]
fn is_promotion_types_the_and_rhs_member_read() {
    let report = report(
        "class int {} class bool {} class Object {} \
             class Money11 { int cents = 0; } \
             class C { int cents = 0; bool operator ==(Object other) => other is Money11 && other.cents == cents; }",
    );
    assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
}

/// #2167's negative: without the `is` test the RHS read stays on the
/// declared `Object` and the getter miss fires with the oracle text.
#[test]
fn unpromoted_object_member_read_still_reports() {
    let report = report(
        "class int {} class bool {} class Object {} \
             class C { int cents = 0; bool operator ==(Object other) => other.cents == cents; }",
    );
    assert_eq!(
        code_names(&report),
        ["undefined_getter"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The getter 'cents' isn't defined for the type 'Object'."
    );
}

/// #1511 F-R: a record-type annotation captures its field shape, a record
/// literal projects its field rows onto the same shape, and the
/// assignability face decides the pair field-wise — the fixture's fire
/// line (an arity mismatch whose second side also carries a recovered
/// hole) reports while the equal-shape control stays silent (upstream
/// `subtype.dart`'s record rule: equal arity, pairwise field
/// assignability; the comma hole splices an `InvalidType` field that is
/// assignable in both directions).
#[test]
fn record_type_assignability_reads_the_field_shapes() {
    let report = report(
        "class int {}\n\
             void main() {\n\
               (int, ,) a = (42, 42, ,);\n\
               (int, int, ,) b = (42, 42, ,);\n\
             }\n",
    );
    let assignments: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .collect();
    assert_eq!(assignments.len(), 1, "{:?}", code_names(&report));
    assert_eq!(
        assignments[0].message,
        "A value of type '(int, int, InvalidType)' can't be assigned to a variable of type '(int, InvalidType)'."
    );
}

/// #1011 pin: the `!= null` equality check promotes the same staged row
/// the `is` test does, so a local declared with a `null` initializer
/// reads non-null inside the then branch and `int y = x;` stays clean;
/// the identical shape without the null check still reports against the
/// declared `int?` (the negative control keeps the pin two-sided).
#[test]
fn null_check_promotion_cleans_the_branch_assignment() {
    let assignments = |source: &str| {
        let report = report(source);
        report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "invalid_assignment")
            .cloned()
            .collect::<Vec<_>>()
    };
    let promoted = "class int {} void f() { int? x = null; if (x != null) { int y = x; } }";
    assert!(
        assignments(promoted).is_empty(),
        "{:?}",
        code_names(&report(promoted))
    );
    let unpromoted = "class int {} void f() { int? x = null; int y = x; }";
    let rows = assignments(unpromoted);
    assert_eq!(rows.len(), 1, "{:?}", code_names(&report(unpromoted)));
    assert_eq!(
        rows[0].message,
        "A value of type 'int?' can't be assigned to a variable of type 'int'."
    );
}

/// The why-not-promoted context (#1877): a write-demotion shape rides
/// the invalid_assignment row — "Variable 'x' could not be promoted due
/// to an assignment" anchored at the demoting write expression (the
/// official `DemoteViaExplicitWrite.node`, upstream Dart SDK source,
/// pkg/analyzer/lib/src/dart/resolver/type_analyzer.dart:6180-6193).
/// The `is int` test stands the promotion the write drops (the
/// declaration initializer does not promote), and the read stays inside
/// the branch — the join does not carry the variable's history.
#[test]
fn write_demotion_context_rides_the_invalid_assignment() {
    let source = "class int {} void f() { int? x = 1; if (x is int) { x = null; int y = x; } }";
    let report = report(source);
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "invalid_assignment")
        .expect("the invalid_assignment row");
    let context = row.context.as_ref().expect("the why-not-promoted rows");
    assert_eq!(context.len(), 1, "{context:?}");
    assert_eq!(
        &*context[0].message,
        "Variable 'x' could not be promoted due to an assignment"
    );
    assert_eq!(context[0].file, row.file);
    assert_eq!(context[0].range.offset, offset_of(source, "x = null"));
    assert_eq!(context[0].range.length, "x = null".len() as u32);
}

/// The property family (#1877): a public member's would-be promotion
/// rides the argument row as the inherent verdict — "'p' refers to a
/// public property so it couldn't be promoted." anchored at the
/// property's declaration name token, cross-file capable and same-file
/// here (the official inherent-reason visitor, upstream Dart SDK
/// source, pkg/analyzer/lib/src/dart/resolver/type_analyzer.dart:6023-6040).
/// The evidence arms through the null test and the receiver write
/// sweeps it into the slot's history (the official
/// `allPreviouslyPromotedTypes` gate — the same shape the flow layer's
/// own `non_promoted_property_read_reports_the_swept_evidence` pins);
/// the read is the promotable-shaped argument the flow layer tracks.
#[test]
fn public_property_context_rides_the_argument_row() {
    let source = "class int {} class C { int? p = 1; } void g(int x) {} \
             void f(C c) { if (c.p != null) { c = C(); g(c.p); } }";
    let report = report(source);
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "argument_type_not_assignable")
        .expect("the argument_type_not_assignable row");
    let context = row.context.as_ref().expect("the why-not-promoted rows");
    assert_eq!(context.len(), 1, "{context:?}");
    assert_eq!(
        &*context[0].message,
        "'p' refers to a public property so it couldn't be promoted."
    );
    assert_eq!(context[0].file, row.file);
    assert_eq!(context[0].range.offset, offset_of(source, "p = 1"));
}

/// The per-file emission fuse (#1856): an over-cap file truncates to
/// exactly [`MAX_DIAGNOSTICS_PER_FILE`] rows and gains one terminal
/// marker at the tail — the structural bound, a `truncate` at the fold's
/// single assembly point, never a loop.
#[test]
fn per_file_cap_truncates_with_one_marker_row() {
    let mut source = String::from("void f() {\n");
    for i in 0..MAX_DIAGNOSTICS_PER_FILE + 50 {
        source.push_str("  undefined_name_");
        source.push_str(&i.to_string());
        source.push_str(";\n");
    }
    source.push_str("}\n");
    let report = report(&source);
    assert_eq!(
        report.diagnostics.len(),
        MAX_DIAGNOSTICS_PER_FILE + 1,
        "the survivors truncate to the cap and the marker rides the tail"
    );
    let marker = report.diagnostics.last().expect("non-empty");
    assert_eq!(marker.code.name, DARTQ_INTERNAL_ERROR.name);
    assert_eq!(
        &*marker.message,
        format!("Too many diagnostics ({MAX_DIAGNOSTICS_PER_FILE}); the rest were suppressed.")
    );
}

/// The drop rule: a promotion to `Never` never stages
/// (`type_system.dart:1729` `tryPromoteToType` only subtype-narrows,
/// and the M1 lattice has no bottom source type), so the join misses
/// and the unpromoted verdict stands — under-report, never a
/// fabricated clean.
#[test]
fn never_promotion_does_not_clean_the_row() {
    let report = report(
        "class int {} class Never {} \
             void f() { int? x = 1; if (x is Never) { int y = x; } }",
    );
    let assignments: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .collect();
    assert_eq!(assignments.len(), 1, "{:?}", code_names(&report));
}

/// The promotion face over a written-argument local (#611 step 4
/// consumer readiness): a written element-covariant test promotes, so
/// the promoted branch's `List<int> y = x;` is clean while the
/// identical unpromoted shape still reports against the declared
/// `List<int>?` — the arg-bearing promotion shape the retired
/// written-argument channel used to serve.
#[test]
fn promotion_over_written_args_local_cleans_the_promoted_branch() {
    let report = report(
        "class int {} class List<T> {} \
             void promoted() { List<int>? x = null; if (x is List<int>) { List<int> y = x; } } \
             void unpromoted() { List<int>? x = null; List<int> y = x; }",
    );
    let assignments: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .collect();
    assert_eq!(assignments.len(), 1, "{:?}", code_names(&report));
    assert_eq!(
        assignments[0].message,
        "A value of type 'List<int>?' can't be assigned to a variable of type 'List<int>'."
    );
}

/// The conversion's drop rule at the seam: `void` and `Never` promoted
/// types return `None` (never stage); the special bases map one-to-one
/// onto the M1 lattice.
#[test]
fn staged_promotion_drops_void_and_never() {
    let strings = StringTable::new();
    let mut pool = ArgPool::default();
    let scanned = dartq_scanner::scan(
        "",
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        FileTable::new().intern(Path::new("test.dart")),
    );
    let flow = Flow::new(&scanned.tokens, "");
    let mut never_stages =
        |ty| staged_promotion(ty, dartq_flow::ty::EMPTY_ARGS, &flow, &strings, &mut pool).is_none();
    assert!(never_stages(FlowTy {
        base: FlowBase::Void,
        nullable: false
    }));
    assert!(never_stages(FlowTy {
        base: FlowBase::Never,
        nullable: false
    }));
    assert_eq!(
        staged_promotion(
            FlowTy {
                base: FlowBase::Dynamic,
                nullable: false
            },
            dartq_flow::ty::EMPTY_ARGS,
            &flow,
            &strings,
            &mut pool
        ),
        Some(Ty::Dynamic)
    );
    assert_eq!(
        staged_promotion(
            FlowTy {
                base: FlowBase::Null,
                nullable: true
            },
            dartq_flow::ty::EMPTY_ARGS,
            &flow,
            &strings,
            &mut pool
        ),
        Some(Ty::Null)
    );
}

/// The join is span-exact: the same start with a wider span (a compound
/// expression headed by the promoted read, `x.foo`) never joins; an
/// empty staging misses everything; a diverging repeat poisons the
/// span; an identical repeat collapses.
#[test]
fn promotion_staging_joins_exact_spans_only() {
    let strings = StringTable::new();
    let int = strings.intern("int");
    let mut staging = PromotionStaging::default();
    let read = SourceRange {
        offset: 4,
        length: 1,
    };
    // No facts, no join.
    assert_eq!(staging.promoted(read), None);
    staging.stage(read, Ty::named(int));
    assert_eq!(staging.promoted(read), Some(Ty::named(int)));
    // `x.foo` starts at the promoted read but is not the bare read.
    let wide = SourceRange {
        offset: 4,
        length: 5,
    };
    assert_eq!(staging.promoted(wide), None);
    let other = SourceRange {
        offset: 5,
        length: 1,
    };
    assert_eq!(staging.promoted(other), None);
    // Two walks disagreeing on the promoted type leave nothing that
    // stands for the span.
    staging.stage(read, Ty::Dynamic);
    assert_eq!(staging.promoted(read), None);
    // An identical repeat re-stages.
    staging.stage(read, Ty::named(int));
    assert_eq!(staging.promoted(read), Some(Ty::named(int)));
}

/// The int→double literal conversion (`analyzeIntLiteral`'s
/// `convertToDouble`): an int *literal* in a double context is clean on
/// every context site the M1 facts check (declaration initializer,
/// argument, return expression, assignment right-hand side), while an
/// int *value* (a variable read) reports on the same sites — the
/// conversion keys on the literal, never on the value.
///
/// Hand-trace: `1` pushes a [`Engine::push_primitive_row`] row (a
/// `Value` row typed `int`, [`Engine::primitive_literal`] true); `i`'s
/// bare row carries the declared `int` in a non-literal shape. The
/// final-primitive table answers the literal flag for the int→double
/// pair, so the literal sites return `true` (silent) and the variable
/// sites `false` (the trio's codes, in source order).
#[test]
fn int_literal_converts_to_double_but_the_value_does_not() {
    let report = report(
        "class int {} class double {} \
             void f(double x) {} double g() { return 1; } \
             void main() { int i = 1; double a = 1; double b = i; f(1); f(i); double c; c = 1; c = i; }",
    );
    assert_eq!(
        code_names(&report),
        [
            // `a`/`b`/`c` are never read (a pure write is not a use —
            // the official read-only use rule), so each reports
            // `unused_local_variable` interleaved in source order.
            "unused_local_variable",
            "unused_local_variable",
            "invalid_assignment",
            "argument_type_not_assignable",
            "unused_local_variable",
            "invalid_assignment",
        ],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[2].message,
        "A value of type 'int' can't be assigned to a variable of type 'double'."
    );
}

/// A dart:core top-level annotation name (`@override` — meta 1.19.0
/// `meta_meta.dart:202`, the M1 r2 gate's only non-#351 differential
/// row) is defined by the implicit dart:core import: no
/// `undefined_annotation` and no names-table miss. A genuinely missing name
/// still reports.
///
/// Hand-trace: the `MetadataReference` identifier resolves `Missing` for
/// all three names; the [`CORE_TOP_LEVEL_ANNOTATIONS`] membership
/// clears `override`/`pragma`, leaving `gone` to report over the `@`.
#[test]
fn core_top_level_annotation_names_stay_silent() {
    let report = report("@override @pragma('x') @gone void f() {}");
    assert_eq!(code_names(&report), ["undefined_annotation"]);
    assert_eq!(
        report.diagnostics[0].message,
        "Undefined name 'gone' used as an annotation."
    );
}

/// A fully missing annotation name reports `undefined_annotation` over
/// the `@` (#350; the 3.13.1 caliber — the diagnostic was removed
/// upstream in 3.14). A defined class annotation stays silent.
///
/// Hand-trace: `@gone` — `begin_metadata` latches the `@`; the
/// `MetadataReference` identifier resolves `Missing` (the annotation name
/// is no declaration) -> the emit spans `@gone`.
#[test]
fn missing_annotation_reports_over_the_at_sign() {
    let report = report("class A {} @gone void f() {} @A void g() {}");
    assert_eq!(code_names(&report), ["undefined_annotation"]);
    let d = &report.diagnostics[0];
    assert_eq!(d.message, "Undefined name 'gone' used as an annotation.");
    assert_eq!(u32::try_from("class A {}@".len()).unwrap(), d.range.offset);
    assert_eq!(u32::try_from("@gone".len()).unwrap(), d.range.length);
}

/// The metadata path skips the broken-import shown-name guard: an
/// annotation name riding a dead import's `show` list still reports
/// `undefined_annotation` (the oracle's scope lookup answers the
/// metadata resolution before the element.dart:8217-8250 suppression —
/// probe-verified both ways; the identifier face keeps the guard).
///
/// Hand-trace: `import 'missing.dart' show useResult;` resolves to no
/// target — the namespace contributes nothing, `resolve_bare` answers
/// `Missing`, and the guard the identifier faces share is not consulted.
#[test]
fn shown_name_on_broken_import_still_reports_annotation() {
    let report = report("import 'missing.dart' show useResult;\n@useResult void f() {}");
    assert_eq!(code_names(&report), ["undefined_annotation"]);
    assert_eq!(
        report.diagnostics[0].message,
        "Undefined name 'useResult' used as an annotation."
    );
}

/// A dead supertype edge (undefined, ambiguous, or non-class spelling —
/// the oracle's InvalidType) contributes nothing to the member
/// interface: the class's own declarations are the whole face, so
/// instance misses report (`undefined_method`, `undefined_getter`,
/// `undefined_identifier`). An own member stays silent.
///
/// Hand-trace: `walk_from` resolves `B`, misses `foo`/`bar`, climbs to
/// the `Missing` super — `supertype_hop_dead` answers true (namespace
/// miss) — the climb ends `Walk::Missing` and the callers report. The
/// bare `y` rides the same dead edge through the implicit-this face.
#[test]
fn dead_super_edge_member_misses_report() {
    let report = report(
        "class B extends Missing {\n  int x = 0;\n  void g() {\n    x;\n    y;\n  }\n}\nvoid f(B b) {\n  b.foo();\n  b.bar;\n}",
    );
    // The bare-unit harness additionally runs the extends head through
    // the resolver's plain path (`undefined_class` beside the clause
    // face) — the CLI pipeline, probe-verified against the oracle on
    // this exact source, emits precisely the four faces below; the pin
    // filters the harness artifact and pins the family faces.
    let names: Vec<&str> = code_names(&report)
        .into_iter()
        .filter(|name| *name != "undefined_class")
        .collect();
    assert_eq!(
        names,
        [
            "extends_non_class",
            "undefined_identifier",
            "undefined_method",
            "undefined_getter",
        ]
    );
}

/// An unresolvable superclass face takes no constructor formals: every
/// super formal reports unassociated — named and positional alike (the
/// live 3.13.1 oracle on `AndroidSslAuthError extends
/// PlatformSslAuthError`, all rows fire; the pre-dead-edge early return
/// under-reported).
#[test]
fn dead_super_formals_report_unassociated() {
    let report = report(
        "class C extends Missing {\n  C({required super.code, super.a});\n}\nclass D extends Missing {\n  D(super.b);\n}",
    );
    let mut names = code_names(&report);
    names.sort();
    assert_eq!(
        names,
        [
            "extends_non_class",
            "extends_non_class",
            "super_formal_parameter_without_associated_named",
            "super_formal_parameter_without_associated_named",
            "super_formal_parameter_without_associated_positional",
        ]
    );
}

/// An undefined `is` operand reports `type_test_with_undefined_name` at
/// the operand name; an undefined `as` operand reports
/// `cast_to_non_type` (error_verifier.dart `visitIsExpression` /
/// `visitAsExpression`). The dedicated codes replace the resolver's
/// `undefined_class` at the operand position — the CLI pipeline (with
/// the SDK import face) keeps the operand silent there, probed
/// identical to the oracle; this bare-unit harness lacks the SDK face,
/// so the two operand rows surface and are filtered below.
#[test]
fn undefined_is_as_operands_report_their_codes() {
    let report = report("void f(Object x, Object y) {\n  if (x is Missing) {}\n  y as Missing;\n}");
    let names: Vec<&str> = code_names(&report)
        .into_iter()
        .filter(|name| *name != "undefined_class")
        .collect();
    assert_eq!(names, ["type_test_with_undefined_name", "cast_to_non_type"]);
    assert_eq!(
        report.diagnostics[0].message,
        "The name 'Missing' isn't defined, so it can't be used in an 'is' expression."
    );
    assert_eq!(
        report.diagnostics[1].message,
        "The name 'Missing' isn't a type, so it can't be used in an 'as' expression."
    );
}

/// A `with` entry resolving to a plain class reports
/// `class_used_as_mixin` at the entry name; a `mixin class` entry stays
/// silent (messages.yaml `classUsedAsMixin`).
#[test]
fn plain_class_with_entry_reports_class_used_as_mixin() {
    let report = report("class M {}\nclass C with M {}\nmixin class M2 {}\nclass D with M2 {}");
    assert_eq!(code_names(&report), ["class_used_as_mixin"]);
    assert_eq!(
        report.diagnostics[0].message,
        "The class 'M' can't be used as a mixin because it's neither a mixin class nor a mixin."
    );
}

/// A dead `implements` edge contributes no subtype relation: a class
/// implementing an undefined name is not assignable to an unrelated
/// class — the definite miss the return/argument faces report
/// (`return_of_invalid_type`).
#[test]
fn dead_implements_edge_breaks_assignability() {
    let report = report("class Locale {}\nclass Impl implements Dead {}\nLocale f(Impl x) => x;");
    assert_eq!(
        code_names(&report),
        ["implements_non_class", "return_of_invalid_type"]
    );
}

/// `super.named()` in a constructor-initializer list is a super
/// *constructor* reference: the named shape reports
/// `undefined_constructor_in_initializer` against the superclass, and
/// the member pass never mis-reports `undefined_super_member` (#350).
/// A declared selector stays silent.
///
/// Hand-trace: the initializer bracket raises the depth; `super.named()`
/// merges at the dot with a `Super` receiver — the merge emits from the
/// superclass constructor face and folds the row, so pass 2 sees only a
/// value.
#[test]
fn super_initializer_selector_reports_the_class_constructor() {
    let report = report(
        "class A { A.named(); } \
             class B extends A { B.bad() : super.missing(); B.good() : super.named(); }",
    );
    assert_eq!(
        code_names(&report),
        ["undefined_constructor_in_initializer"]
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The class 'A' doesn't have a constructor named 'missing'."
    );
}

/// A superclass with no declared constructors at all: the implicit
/// default is the only constructor, so any named selector misses
/// (regression pin `cd3b3a85`, m1 `undefined_constructor_in_initializer`
/// — the empty-superclass shape went silent when the decision deferred
/// to the list's close). The field-initializer and recovery residue
/// shapes stay silent beside it.
#[test]
fn super_initializer_selector_reports_over_an_implicit_only_superclass() {
    let missing = report("class A {}\n\nclass B extends A {\n  B() : super.named();\n}\n");
    assert_eq!(
        code_names(&missing),
        ["undefined_constructor_in_initializer"],
        "{:?}",
        code_names(&missing)
    );
    assert_eq!(
        missing.diagnostics[0].message,
        "The class 'A' doesn't have a constructor named 'named'."
    );
    // The assignment shape (`super.x = v`) carries no
    // constructor-reference verdict — the only row is the CFE parser's
    // declaring-class rider, which fires on the spelling alone
    // (parseSuperInitializerExpression's EQ branch, upstream Dart SDK
    // source, pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart,
    // anchored by analyzer's error_parser_test.dart
    // `C() : super.a = 42` — the parser cannot see whether `x` is a
    // declared field). The `super().foo` recovery residue drops — no
    // constructor-reference verdict either.
    let assigned = report(
        "class int {}\nclass A { int x = 0; }\n\
             class B extends A {\n  B() : super.x = 1;\n}\n",
    );
    assert_eq!(
        code_names(&assigned),
        ["field_initialized_outside_declaring_class"],
        "{:?}",
        code_names(&assigned)
    );
}

/// The constructor-initializer face (F-C): a field initializer on an
/// undeclared name codes through the dedicated initializer world
/// (`initializer_for_non_existent_field` over the whole node,
/// `_checkForInvalidField`), never `undefined_setter`. A declared field
/// stays clean (`int` modeled concretely — the harness has no core
/// names).
///
/// Hand-trace: `this.x` merges a this-receiver row inside the
/// initializer bracket — the push stamps it into `init_rows`, and the
/// pass-2 instance-member dispatch folds it before the member trio.
#[test]
fn initializer_field_setter_stays_out_of_the_member_face() {
    let bare = report("class C { C() : this.x = 0; }");
    assert_eq!(
        code_names(&bare),
        ["initializer_for_non_existent_field"],
        "{:?}",
        code_names(&bare)
    );
    let declared = report("class int {}\nclass C { int x = 0; C() : this.x = 1; }");
    assert!(
        declared.diagnostics.is_empty(),
        "{:?}",
        code_names(&declared)
    );
}

/// The constructor-initializer face (F-C): a bare LHS name in an
/// initializer list is the field-initializer shape — the oracle codes
/// the miss `initializer_for_non_existent_field` (`_checkForInvalidField`
/// reads the enclosing class's own field map), never
/// `undefined_identifier`; the list's right-hand sides keep the
/// ordinary resolution.
///
/// Hand-trace: the bare `x` row pushes while `initializer_depth` is up
/// (`init_rows` stamp keyed to the container) and the `=` flips it to
/// the write intent — the missing-name arm folds it and the pass-2
/// field check reports `x` against the (empty) field set. The RHS
/// `gone` keeps its read intent and reports beside it.
#[test]
fn initializer_lhs_never_reports_undefined_identifier() {
    let lhs = report("class C { C() : x = 0; }");
    assert_eq!(code_names(&lhs), ["initializer_for_non_existent_field"]);
    let rhs = report("class C { C() : x = gone; }");
    assert_eq!(
        code_names(&rhs),
        ["initializer_for_non_existent_field", "undefined_identifier"]
    );
}

/// The recovery shapes keep the face: an initializer LHS recovered out
/// of a getter declaration (`get Foo() : initializer = true`, the
/// issue-26810 recovered-parameter shape) never reports
/// `undefined_identifier` — the parser's own rows are the only report.
#[test]
fn recovered_initializer_lhs_stays_silent() {
    let report = report("class Foo { get Foo() : initializer = true { } }");
    assert!(
        !code_names(&report).contains(&"undefined_identifier"),
        "{:?}",
        code_names(&report)
    );
}

/// `super()` followed by a selector (`super().foo`, the
/// invalid_super_in_initializer fixture) is recovery residue: the
/// oracle reports only the parser's `invalid_super_in_initializer` —
/// no `undefined_constructor_in_initializer` mis-fire on the trailing
/// selector.
///
/// Hand-trace: the `.foo` merge folds a `pending_init_super` candidate;
/// no argument group follows the selector, so the list's close drops it
/// unreported.
#[test]
fn super_invocation_selector_residue_drops_the_constructor_face() {
    let report = report("class A {} class B extends A { B() : super().foo {} }");
    assert_eq!(code_names(&report), ["invalid_super_in_initializer"]);
}

/// `super.toString = 1` is the field-initialized-outside shape (the
/// parser's declaring-class rider reports it) — never a super
/// *constructor* reference.
///
/// Hand-trace: the `super.toString` merge folds a candidate; the `=`
/// never opens an argument group, so the close drops it and the
/// constructor face stays silent.
#[test]
fn super_field_initializer_is_not_a_constructor_reference() {
    let report = report("class A {} class C extends A { C() : super.toString = 1; }");
    assert_eq!(
        code_names(&report),
        [
            // The whole-node row anchors at `super` (name through RHS
            // end), the parser's declaring-class rider at the name.
            "initializer_for_non_existent_field",
            "field_initialized_outside_declaring_class",
        ],
        "{:?}",
        code_names(&report)
    );
}

/// `this()` followed by a selector (`this().foo`, the
/// invalid_this_in_initializer fixture) is recovery residue: the
/// parser's `invalid_this_in_initializer` at `this`, plus the
/// verifier's `recursive_constructor_redirect` at `this()` (the
/// self-redirect participates in a cycle of one) — the trailing
/// selector never mis-fires `undefined_getter`.
///
/// Hand-trace: the `.foo` merge stamps the this-receiver row into
/// `init_rows`; the pass-2 dispatch folds it before the member faces.
#[test]
fn this_invocation_selector_residue_drops_the_member_face() {
    let report = report("class A {} class B extends A { B() : this().foo; }");
    assert_eq!(
        code_names(&report),
        [
            "invalid_this_in_initializer",
            "recursive_constructor_redirect",
        ],
        "{:?}",
        code_names(&report)
    );
}

/// `this.other()` in an initializer list resolves `other` against the
/// enclosing class's *constructor* table
/// (`RedirectingConstructorInvocation`) — a declared `C.other` is no
/// `undefined_method`; a missing target reports nothing upstream either
/// (element_resolver.dart defers the report).
#[test]
fn redirecting_initializer_resolves_against_the_constructor_table() {
    let report = report("class C { C() : this.other() {} C.other(); }");
    assert_eq!(code_names(&report), ["redirecting_constructor_with_body"]);
}

/// A constructor-initializer list owns its assignability question: a
/// non-assignable field initializer codes
/// `field_initializer_not_assignable` at the initializer expression —
/// even under a misnamed constructor, whose own face stays
/// `invalid_constructor_name` (the parser's, capture: issue_41121
/// L23/L27) — never the plain `invalid_assignment`. `int` is modeled
/// concretely (the harness has no core names).
///
/// Hand-trace: `Foo() : _c = null` — the parser's misnamed-constructor
/// row anchors at `Foo`; the field `_c` is declared `int`, and `null`
/// (Null) is not assignable to it, so the assignability face anchors at
/// `null`. The body read only spares `_c` the `unused_field` face.
#[test]
fn initializer_assignment_suppresses_the_plain_assignment_check() {
    let report = report("class int {}\nclass C { int _c = 0; Foo() : _c = null { _c; } }");
    assert_eq!(
        code_names(&report),
        [
            "invalid_constructor_name",
            "field_initializer_not_assignable",
        ],
        "{:?}",
        code_names(&report)
    );
}

// —— F-B: `new` as an identifier — the invocation/tear-off faces ————

/// A qualified creation head (`prefix.C` of `const prefix.C.new()`)
/// resolves as a prefixed NamedType: the dead prefix reports
/// `undefined_identifier` over the written `prefix.C` span — never the
/// creation-head type-miss a bare head carries (named_type_resolver's
/// importPrefix face; the prefix bail returns before any argument
/// resolution, so the `<int>` group adds no row). The
/// parameterized-oracle probe q3 parses `prefix.C<int>.new()` as the
/// plain selector chain (the group splits the head from the
/// continuation — the ast_rewrite's UnqualifiedFunctionInvocation
/// shape): the head row plus the `.new` method face on the Invalid
/// receiver.
///
/// q3 dependency (verified against the current parser, 2026-10-16): the
/// parser's implicit-creation lookahead (`identifier ('.' identifier)?
/// '<' typeArguments '.' (identifier|new) '('` — upstream Dart SDK
/// source, pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart
/// :7264-7283) takes the one-dot parameterized form
/// `prefix.C<int>.new()` into `ConstructorReferenceContext::Implicit`,
/// so the engine sees one requalified head row where the oracle's
/// ast_rewrite (the missing `const`/`new` keyword) keeps the selector
/// chain. That keyword-less PARAMETERIZED shape stays red until the
/// parser lands the ast_rewrite form — no semantic-side approximation.
/// The unparameterized `prefix.C.new()` never enters the probe (no `<`
/// after the second segment) and already parses as the plain chain —
/// see `prefixed_invocation_chain_reports_head_then_method`. The
/// two-segment reference
/// `const Missing.new()` — the
/// `.new` riding the qualified rest — makes the type itself the
/// two-segment NamedType `Missing.new`, so the same importPrefix bail
/// reports `undefined_identifier 'Missing'` over the whole written
/// head (oracle q4: 1:18 length 11), never the plain-path
/// `undefined_class`.
///
/// Hand-trace: the head row pushes as a `Missing` type use with the
/// creation-context entry armed; `end_constructor_reference` rewrites
/// the row to the prefix face — the written period's span for the
/// three-segment form, the selector's end for the two-segment one —
/// consuming the context entry.
#[test]
fn qualified_creation_head_reports_the_prefix_face() {
    let qualified = report("void f() { const prefix.C.new(); }");
    assert_eq!(code_names(&qualified), ["undefined_identifier"]);
    let new_form = report("void f() { new prefix.C.new(); }");
    assert_eq!(code_names(&new_form), ["undefined_identifier"]);
    let generic = report("class int {} void f() { prefix.C<int>.new(); }");
    assert_eq!(
        code_names(&generic),
        ["undefined_identifier", "undefined_method"]
    );
    let bare = report("void f() { const Missing.new(); }");
    assert_eq!(code_names(&bare), ["undefined_identifier"]);
    let row = &bare.diagnostics[0];
    assert_eq!(row.message, "Undefined name 'Missing'.");
    assert_eq!(row.range.length, 11);
}

/// The d1b3378f theft class (#1869-1): the generic implicit creation's
/// selector row latches the F-H displacement on its send, and the next
/// statement's member merge used to read the latch as its own —
/// draining the fresh rows and folding the latched selector against
/// the stranded head, so BOTH faces vanished behind the corpus pin's
/// missing `undefined_method`. The displaced probe now requires every
/// row above the latch to end before the merge's operator (an honest
/// argument window lies inside the completed send), so the follower's
/// merge folds its own rows and the latched selector resolves for
/// itself: head then method, exactly the standalone shape.
#[test]
fn generic_creation_selector_survives_a_following_plain_chain() {
    let source = "class int {} class C { C.new(); }\nvar a = prefix.C<int>.new();\nvar b = C.new;";
    assert_eq!(
        code_names(&report(source)),
        ["undefined_identifier", "undefined_method"]
    );
    // The standalone shape (the corpus pin's non-generic (39,57)
    // posture at top level) stays green beside it.
    let single = report("class C { C.new(); }\nvar a = prefix.C.new();");
    assert_eq!(
        code_names(&single),
        ["undefined_identifier", "undefined_method"]
    );
}

/// A prefixed implicit creation without type arguments parses as a
/// plain selector chain: the dead head survives the member folds (its
/// `undefined_identifier` defers) and the `.new()` invocation still
/// reports — `undefined_method`, the oracle's name=='new' property
/// lookup on the InvalidType receiver
/// (type_property_resolver.dart:78-83 answers `needsGetterError`
/// before the invalid-receiver silence).
///
/// Parser shape (verified 2026-10-16): `prefix.C.new()` never enters
/// the implicit-creation lookahead — the probe requires the
/// `<typeArguments>` group after the second segment (upstream Dart SDK
/// source, pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart
/// :7264-7283) — so the events are the plain dot chain (the `.new`
/// rides the dot-continuation identifier rewrite) and this pin is the
/// green half of the q3 probe. The parameterized sibling
/// `prefix.C<int>.new()` still builds the implicit constructor
/// reference; its dependency note lives on
/// `qualified_creation_head_reports_the_prefix_face`.
///
/// Hand-trace: `prefix` pushes `Missing`; the `.C` merge defers it and
/// folds an unknown receiver; the `.new` merge defers the `C` row; the
/// pass-2 dispatch reports head then method.
#[test]
fn prefixed_invocation_chain_reports_head_then_method() {
    let report = report("void f() { prefix.C.new(); }");
    assert_eq!(
        code_names(&report),
        ["undefined_identifier", "undefined_method"]
    );
}

/// The prefixed tear-off (`prefix.C.new`) reports the head and the
/// `new` getter — the read face of the same name=='new' lookup — and
/// the chain stops there: the failed `.new` read is InvalidType, whose
/// property lookups stay silent, so `.toString()` adds no row.
#[test]
fn prefixed_tearoff_reports_head_then_new_getter() {
    let tearoff = report("void f() { prefix.C.new; }");
    assert_eq!(
        code_names(&tearoff),
        ["undefined_identifier", "undefined_getter"]
    );
    let chained = report("void f() { prefix.C.new.toString(); }");
    assert_eq!(
        code_names(&chained),
        ["undefined_identifier", "undefined_getter"]
    );
}

/// A redirecting-factory target (`= prefix.C.new;`) resolves through
/// the redirect face: the head miss is the oracle's
/// `redirect_to_non_class` (named_type_resolver.dart:587-594, the
/// `_reportRedirectToNonClass` shape — inside the frozen gate since the
/// #2129 redirect family landed), never the scored `undefined_class` a
/// plain type use carries. The initializer recovery reuses the same
/// body parse (`this.new = null;`), so it drops the misfire too.
///
/// Hand-trace: `begin_redirecting_factory_body` raises the depth; the
/// head row pushes with the `RedirectTarget` verdict, whose pass-2 arm
/// records without a report; `end_redirecting_factory_body` then runs
/// the redirect-target checks (`resolve_container` for the head
/// container) and emits `redirect_to_non_class` on an unresolved head.
#[test]
fn redirect_target_head_never_reports_undefined_class() {
    let prefixed = report("class D { factory D.r() = prefix.C.new; }");
    assert_eq!(
        code_names(&prefixed),
        ["redirect_to_non_class"],
        "{:?}",
        code_names(&prefixed)
    );
    let missing = report("class D { factory D.r() = Missing.new; }");
    assert_eq!(
        code_names(&missing),
        ["redirect_to_non_class"],
        "{:?}",
        code_names(&missing)
    );
    let initializer = report("class C { C.constructor_field_initializer() : this.new = null; }");
    let names = code_names(&initializer);
    assert!(!names.contains(&"undefined_class"), "{:?}", names);
    assert!(
        names.contains(&"expected_identifier_but_got_keyword"),
        "{:?}",
        names
    );
}

/// `redirect_to_invalid_return_type` at the `= S` head (`S` is not a
/// subtype of `T`), verbatim from `redirect_to_invalid_return_type.dart`
/// and `clean_constructor_forms.dart`'s `const factory B() = A;` —
/// `_checkForRedirectToInvalidReturnType`'s assignability arm
/// (error_verifier.dart:2986-2999). The `B extends A ⇒ B() = A` arm
/// fires too: the official `isAssignableTo(redirectedReturnType=A,
/// constructorReturnType=B)` checks the TARGET's type against the
/// REDIRECTING's — a supertype target is no subtype of the factory's
/// class, so the diagnostic still reports.
#[test]
fn redirect_to_invalid_return_type_reports_on_unrelated_target() {
    let r = report("class S {} class T { factory T() = S; }");
    assert_eq!(
        code_names(&r),
        ["redirect_to_invalid_return_type"],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "The return type 'S' of the redirected constructor isn't a subtype of 'T'."
    );
    // Supertype target: still fires — `A` is not a subtype of `B`.
    let r = report("class A {} class B extends A { factory B() = A; }");
    assert!(
        code_names(&r).contains(&"redirect_to_invalid_return_type"),
        "{:?}",
        code_names(&r)
    );
    // Self-target stays silent: `B` is trivially a subtype of `B`.
    let r = report("class B { factory B.dup() = B; }");
    assert!(
        !code_names(&r).contains(&"redirect_to_invalid_return_type"),
        "{:?}",
        code_names(&r)
    );
}

/// `redirect_to_non_class` on an unresolvable head —
/// `redirect_to_non_class.dart`'s `factory Q() = NoSuch;` shape.
#[test]
fn redirect_to_non_class_reports_on_missing_head() {
    let r = report("class Q { factory Q() = NoSuch; }");
    assert_eq!(
        code_names(&r),
        ["redirect_to_non_class"],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "The name 'NoSuch' isn't a type and can't be used in a redirected constructor."
    );
}

/// `redirect_to_non_const_constructor` beside
/// `redirect_to_invalid_return_type` — `redirect_to_non_const_
/// constructor.dart`'s `const factory B() = A;` fires BOTH faces at the
/// same `= A` extent (`_checkForRedirectToNonConstConstructor`,
/// error_verifier.dart:7335-7347, runs independently of the return-type
/// arm), while a const target (`const A();`) silences only the const
/// face.
#[test]
fn redirect_to_non_const_constructor_pairs_with_return_type_face() {
    let r = report("class A { A(); } class B { const factory B() = A; }");
    assert_eq!(
        code_names(&r),
        [
            "redirect_to_invalid_return_type",
            "redirect_to_non_const_constructor"
        ],
        "{:?}",
        code_names(&r)
    );
    // Both rows anchor the `= A` target extent (oracle line 10, col 23,
    // length 1).
    assert_eq!(r.diagnostics[0].range.offset, r.diagnostics[1].range.offset);
    assert_eq!(r.diagnostics[1].range.length, 1);
    // A const target silences the const face only.
    let r = report("class A { const A(); } class B { const factory B() = A; }");
    assert_eq!(
        code_names(&r),
        ["redirect_to_invalid_return_type"],
        "{:?}",
        code_names(&r)
    );
}

/// `redirect_generative_to_non_generative_constructor` —
/// redirect_generative_to_non_generative_constructor.dart's generative
/// `W() : this.n();` whose target resolves to the factory `W.n`
/// (error_verifier.dart:4300-4306, `.at(invocation)` = the whole
/// `this.n()` node extent); a generative target stays silent.
#[test]
fn redirect_generative_to_factory_constructor_reports_at_invocation() {
    let r = report("class W { factory W.n() => W();\n  W() : this.n(); }");
    assert_eq!(
        code_names(&r),
        ["redirect_generative_to_non_generative_constructor"],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "Generative constructors can't redirect to a factory constructor."
    );
    // The invocation node extent: `this.n()` — 8 characters.
    assert_eq!(r.diagnostics[0].range.length, 8);
    // A generative target (`W.n` without the `factory` head) stays
    // silent.
    let r = report("class W { W.n();\n  W() : this.n(); }");
    assert!(
        !code_names(&r).contains(&"redirect_generative_to_non_generative_constructor"),
        "{:?}",
        code_names(&r)
    );
    // A forward-declared factory target judges the same — the face
    // rides the post-parse drain, where every declaration site is
    // complete.
    let r = report("class W { W() : this.n();\n  factory W.n() => W(); }");
    assert_eq!(
        code_names(&r),
        ["redirect_generative_to_non_generative_constructor"],
        "{:?}",
        code_names(&r)
    );
}

/// The const generative redirect's non-const face — the official
/// messages-example shape (`const C.a() : this.b();` over `C.b();`):
/// `_checkForRedirectToNonConstConstructor` anchors the target selector
/// token (`constructorSelector?.name2 ?? thisKeyword`).
#[test]
fn const_generative_redirect_reports_at_target_selector() {
    let r = report("class C { const C.a() : this.b(); C.b(); }");
    assert_eq!(
        code_names(&r)
            .iter()
            .filter(|name| **name == "redirect_to_non_const_constructor")
            .count(),
        1,
        "{:?}",
        code_names(&r)
    );
}
/// head's recovery shape: `factory new() => 0;` reports
/// `return_of_invalid_type` (`int` is no `C` — the declared-container
/// negative closes the nominal blind spot), and a satisfied body stays
/// clean.
///
/// Hand-trace: the factory name event latches `Ty::named(C)` into the
/// executable frame (`ctor` armed); the arrow body's single row folds
/// `0` (primitive literal) and the staged verdict re-decides false.
#[test]
fn factory_body_checks_against_the_container() {
    let recovery = report("class C { factory new() => 0; }");
    let mut names = code_names(&recovery);
    names.sort_unstable();
    assert_eq!(
        names,
        ["factory_constructor_new_name", "return_of_invalid_type"]
    );
    let const_form = report("class C { const factory new() => 0; }");
    let mut names = code_names(&const_form);
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "const_factory",
            "factory_constructor_new_name",
            "return_of_invalid_type"
        ]
    );
    let satisfied = report("class C { factory C.new() => C(); }");
    assert!(
        satisfied.diagnostics.is_empty(),
        "{:?}",
        code_names(&satisfied)
    );
}

/// The constructor return check reads the declared container faces:
/// `factory D.new() => C();` reports — `C` is absent from `D`'s
/// declared supertype closure — while a subclass body stays silent.
///
/// Hand-trace: the body's single row folds `C()` into a `Created`
/// receiver whose container lifts to `Ty::named(C)`; the negative
/// walks `D`'s declared edges.
#[test]
fn factory_return_walks_the_declared_supertypes() {
    let unrelated = report("class C {} class D { factory D.new() => C(); }");
    assert_eq!(code_names(&unrelated), ["return_of_invalid_type"]);
    // A factory may return a subtype of its declared container
    // (`C.new() => D()` with `D extends C` — the body creation folds to
    // `Ty::named(D)`, whose closure contains `C`, so the negative stays
    // silent); the reverse direction keeps the report (returning the
    // superclass `C` from `D.new()` fails assignability regardless of
    // the extends edge — the closure walk answers the assignability
    // question, not the inheritance direction).
    let subtype = report("class C {} class D extends C { factory C.new() => D(); }");
    assert!(subtype.diagnostics.is_empty(), "{:?}", code_names(&subtype));
}

/// The cascade-section `..new()` invokes the name `new` on the target
/// instance — `undefined_method` (the oracle's property lookup, never a
/// constructor invocation). A folded section (a following `..x = 1`)
/// keeps its own report, and the null-aware twin carries its
/// `invalid_null_aware_operator` beside it.
///
/// Hand-trace: the section's member row folds as the next section's
/// receiver; the deferred member row re-enters pass 2 and resolves
/// against `C2`'s instance face, where `new` misses. The `?..` twin's
/// operator check lifts the `C2()` creation receiver to `Ty::named(C2)`
/// (strictly non-nullable) and reports at the operator.
#[test]
fn cascade_new_section_reports_the_method_face() {
    // The `class int {}` stub is the harness's core stand-in (#537):
    // the field head's bare `int` would otherwise land as a
    // `TypeUse(Missing)` row — an `undefined_class` at pass 2 that
    // crowds the face pin.
    let chained =
        report("class int {} class C2 { C2(); int x = 0; } void f() { C2()..new()..x = 1; }");
    assert_eq!(code_names(&chained), ["undefined_method"]);
    let null_aware = report("class C2 { C2(); } void f() { C2()?..new(); }");
    let mut names = code_names(&null_aware);
    names.sort_unstable();
    assert_eq!(names, ["invalid_null_aware_operator", "undefined_method"]);
}

/// The unnamed-constructor tear-off (`A.new`) resolves through the
/// creation face when the unnamed constructor exists — implicit or the
/// `.new` spelling — and reports `undefined_getter` only when the
/// class genuinely lacks one (named constructors only).
#[test]
fn new_tearoff_resolves_against_the_unnamed_face() {
    let implicit = report("class A { A(); } void f() { A.new; }");
    assert!(
        implicit.diagnostics.is_empty(),
        "{:?}",
        code_names(&implicit)
    );
    let spelled = report("class A { A.new(); } void f() { A.new; }");
    assert!(spelled.diagnostics.is_empty(), "{:?}", code_names(&spelled));
    let named_only = report("class A { A.named(); } void f() { A.new; }");
    assert_eq!(code_names(&named_only), ["undefined_getter"]);
}

/// A `.new` constructor reference inside a constant pattern resolves as
/// a constructor reference, never a getter (#1455 F-M, retired under
/// the unnamed-face escape): `case GenericClass<int>.new:` and its
/// parenthesized-const twin carry no `undefined_getter 'new'` — the
/// selector form keeps only the oracle-shared parse verdict
/// (`invalid_constant_pattern_generic` at the group), and the const
/// creation `case const GenericClass<int>.new(a: 1)` checks `<int>`
/// against the class face (one declared type parameter) — no
/// `wrong_number_of_type_arguments_constructor`. Negative control: the
/// same selector on a class without an unnamed constructor reports the
/// getter miss (the escape is the unnamed face, not the `.new`
/// spelling).
#[test]
fn const_pattern_new_reference_is_not_a_getter() {
    let selector = report(
        "class int {} class GenericClass<T> { const GenericClass({a}); } \
             void f(dynamic o) { switch (o) { case GenericClass<int>.new: break; } }",
    );
    assert_eq!(
        code_names(&selector),
        ["invalid_constant_pattern_generic"],
        "{:?}",
        code_names(&selector)
    );
    let parenthesized = report(
        "class int {} class GenericClass<T> { const GenericClass({a}); } \
             void f(dynamic o) { switch (o) { case const (GenericClass<int>.new): break; } }",
    );
    assert!(
        parenthesized.diagnostics.is_empty(),
        "{:?}",
        code_names(&parenthesized)
    );
    let creation = report(
        "class int {} class GenericClass<T> { const GenericClass({a}); } \
             void f(dynamic o) { switch (o) { case const GenericClass<int>.new(a: 1): break; } }",
    );
    assert!(
        creation.diagnostics.is_empty(),
        "{:?}",
        code_names(&creation)
    );
    let named_only = report(
        "class int {} class NamedOnly<T> { NamedOnly.other(); } \
             void f(dynamic o) { switch (o) { case NamedOnly<int>.new: break; } }",
    );
    assert_eq!(
        code_names(&named_only),
        ["invalid_constant_pattern_generic", "undefined_getter"],
        "{:?}",
        code_names(&named_only)
    );
}

/// A constant pattern whose expression fails const-evaluation reports
/// `constant_pattern_with_non_constant_expression` at the failing
/// expression (upstream Dart SDK source, pkg/analyzer/lib/src/dart/
/// constant/constant_verifier.dart `visitConstantPattern` →
/// `_evaluateAndReportError` :128-142; registry row diagnostic.g.dart
/// :2528-2538). The fixture's offenders — postfix/prefix inc-dec,
/// `this`, `new Class()`, `await`, an `assert` or a nested `switch`
/// expression — capture as non-constant nodes; a valid constant
/// (`1 + 2`, a const reference) stays silent on this face.
#[test]
fn constant_pattern_reports_non_constant_expression() {
    let multi = report(
        "void f(o) { switch (o) { \
             case o++: break; \
             case this: break; \
             case 1 + 2: break; \
             case new Object(): break; \
             } }",
    );
    let mut non_const = multi
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "constant_pattern_with_non_constant_expression")
        .map(|d| (d.range.offset, d.range.length))
        .collect::<Vec<_>>();
    non_const.sort_unstable();
    // `o++` at the postfix's span, `this` at its keyword, `new
    // Object()` at the creation — `1 + 2` evaluates to a constant and
    // stays silent (the oracle's `ConstantPattern` of a valid
    // binary expression is not this code's target).
    assert_eq!(
        non_const.len(),
        3,
        "{:?}",
        multi
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.range.offset, d.range.length))
            .collect::<Vec<_>>()
    );
    let sample = report("void f(o) { switch (o) { case o++: break; } }");
    let diagnostic = sample
        .diagnostics
        .iter()
        .find(|d| d.code.name == "constant_pattern_with_non_constant_expression")
        .expect("the non-constant pattern row");
    assert_eq!(
        diagnostic.message,
        "The expression of a constant pattern must be a valid constant."
    );
    assert_eq!(
        diagnostic.correction.as_deref(),
        Some("Try making the expression a valid constant.")
    );
    // The anchor is the whole postfix expression (`o++` — utf16
    // offsets 30..33; upstream pins the operand-through-operator
    // span, while a prefix `++o` anchors at the operand alone).
    assert_eq!(diagnostic.range.offset, 30);
    assert_eq!(diagnostic.range.length, 3);
}

/// `case this():` / `case super():` inside a constant pattern (the
/// 70d226a5 face): the send machinery still runs inside the pattern
/// window — `this()` is callable when the enclosing class declares
/// `call`, so the const-eval face answers (non-constant), while
/// `super()` resolves against the superclass (`Object`, no `call`)
/// and reports `invocation_of_non_function_expression`
/// (function_expression_invocation_resolver.dart:137-144).
#[test]
fn constant_pattern_this_super_invocation_faces() {
    let sample = report(
        "class Class { const Class([a]); call() {} \
             test(o) { switch (o) { case this(): break; case super(): break; } } }",
    );
    // Baseline probe: `super()` in plain context carries the same
    // invocation face (the send machinery is context-free).
    let plain = report(
        "class Class { const Class([a]); call() {} \
             test(o) { var b = super(); } }",
    );
    assert_eq!(
        plain
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.range.offset, d.range.length))
            .collect::<Vec<_>>(),
        vec![
            ("unused_local_variable", 56, 1),
            ("invocation_of_non_function_expression", 60, 5),
        ],
    );

    let mut rows = sample
        .diagnostics
        .iter()
        .map(|d| (d.code.name, d.range.offset, d.range.length))
        .collect::<Vec<_>>();
    rows.sort_by_key(|r| r.1);
    assert_eq!(
        rows,
        vec![
            ("constant_pattern_with_non_constant_expression", 70, 6),
            ("invocation_of_non_function_expression", 90, 5),
        ],
    );
}

#[test]
fn why_not_promoted_assignability_pair_anchors_at_the_expression() {
    // #1599 (the 630acb14 `why_not_promoted` fixture's C23/E33 rows): the
    // assignability faces anchor at the whole right-hand / receiver
    // expression, not the folded member-name span — `y = c\n.\n…bad`
    // reports over `c..bad` and `E33(c\n.\n…bad)` over the argument's
    // property-access span (`.at(expression)` /
    // `.at(receiverExpression)`).
    let initializer = report(
        "class C23 {\n  int? bad;\n  final int x;\n  final int y;\n  C23.constructorInitializer(C23 c)\n    : x = c.bad!,\n      y = c\n          .\n          /*note*/ bad;\n}",
    );
    let rows: Vec<_> = initializer
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "field_initializer_not_assignable")
        .map(|d| (d.range.offset, d.range.length))
        .collect();
    // `y`'s RHS `c.bad` spans `c` (the receiver) through `bad`'s end.
    let rhs = "class C23 {\n  int? bad;\n  final int x;\n  final int y;\n  C23.constructorInitializer(C23 c)\n    : x = c.bad!,\n      y = "
            .len() as u32;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, rhs);
    assert_eq!(
        rows[0].1,
        ("c\n          .\n          /*note*/ bad".len()) as u32
    );

    // The extension override's receiver check: `E33(c.bad)` on `int`
    // reports `int?` against `int` at the whole `c.bad` span.
    let override_src = "extension E on int { void f() {} }\nclass C { int? bad; }\nvoid g(C c) {\n  if (c.bad == null) return;\n  E(\n    c\n        .\n        /*note*/ bad,\n  ).f();\n}";
    let report = report(override_src);
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "extension_override_argument_not_assignable")
        .map(|d| {
            (
                d.code.name,
                d.range.offset,
                d.range.length,
                d.message.to_string(),
            )
        })
        .collect();
    assert_eq!(
            rows,
            vec![(
                "extension_override_argument_not_assignable",
                override_src.find("c\n        .\n        /*note*/ bad").unwrap() as u32,
                "c\n        .\n        /*note*/ bad".len() as u32,
                "The type of the argument to the extension override 'int?' isn't assignable to the extended type 'int'.".to_string(),
            )],
        );
}

/// An enum constant without a constructor selector invokes the unnamed
/// constructor: when the enum declares none — a recovered `new.named();`
/// member names a constructor upstream, killing the implicit unnamed
/// face — the constant reports `undefined_enum_constructor`
/// (type_analyzer.dart's `visitEnumConstantDeclaration` unnamed arm).
/// A declared selector cancels the constant's own question (the
/// selector's face answers instead), and a named-constructor enum
/// reports its selector-less constants.
///
/// Hand-trace: the constant name arms a pending candidate; the body's
/// `new.named()` head kills the unnamed face; the enum close flushes
/// the candidate. Message note: the unnamed variant's oracle text is
/// argument-less (`The enum doesn't have an unnamed constructor.`) —
/// the shared code still renders the named template.
#[test]
fn enum_constant_reports_the_missing_unnamed_constructor() {
    let recovered = report("enum E { a; new.named(); }");
    let mut names = code_names(&recovered);
    names.sort_unstable();
    assert_eq!(
        names,
        ["new_constructor_dot_name", "undefined_enum_constructor"]
    );
    let named_only = report("enum E { a; const E.other(); }");
    assert_eq!(code_names(&named_only), ["undefined_enum_constructor"]);
    let selector_cancels = report("enum E { a.named(); const E.other(); }");
    assert_eq!(
        code_names(&selector_cancels),
        ["undefined_enum_constructor"]
    );
    let unnamed_keeps = report("enum E { a; }");
    assert!(
        unnamed_keeps.diagnostics.is_empty(),
        "{:?}",
        code_names(&unnamed_keeps)
    );
}

/// A non-augmentation enum whose element list is empty reports
/// `enum_without_constants` at the name (upstream Dart SDK source,
/// pkg/analyzer/lib/src/error/error_verifier.dart `checkEnumDeclaration`
/// — `!isAugmentation && constants.isEmpty` at `node.namePart.typeName`,
/// registry row diagnostic.g.dart:4966-4974). An enum with constants and
/// the missing-body recovery shape keep silent on this code's own row.
#[test]
fn enum_without_constants_reports_at_the_name() {
    let empty = report("enum E {}");
    assert_eq!(code_names(&empty), ["enum_without_constants"]);
    let diagnostic = &empty.diagnostics[0];
    assert_eq!(
        diagnostic.message,
        "The enum must have at least one enum constant."
    );
    assert_eq!(
        diagnostic.correction.as_deref(),
        Some("Try declaring an enum constant.")
    );
    // The emit anchor is the enum's name token (`enum E` — `E` spans
    // utf16 offsets 5..6).
    assert_eq!(diagnostic.range.offset, 5);
    assert_eq!(diagnostic.range.length, 1);
    let declared = report("enum F { a }");
    assert!(
        declared
            .diagnostics
            .iter()
            .all(|d| d.code.name != "enum_without_constants"),
        "{:?}",
        code_names(&declared)
    );
}

/// An enum constant's constructor selector names a constructor the enum
/// does not declare: `undefined_enum_constructor` (#350). A declared
/// selector stays silent.
///
/// Hand-trace: `c.bad()` — the selector identifier fires inside the
/// enum-element window; the enum's constructor face is `{other}` (the
/// named selector) with no unnamed constructor; `bad` misses -> the
/// emit at the selector.
#[test]
fn enum_constant_selector_reports_the_enum_constructor() {
    let report = report("enum E { c.bad(); const E.other(); }");
    assert_eq!(code_names(&report), ["undefined_enum_constructor"]);
    assert_eq!(
        report.diagnostics[0].message,
        "The enum doesn't have a constructor named 'bad'."
    );
}

/// The explicit extension application resolves its members against the
/// extension's face — `E('x').h` reports the extension trio (#350),
/// including the operator shape at the operator token — and `Object`
/// members (the `on` type's face) stay silent.
///
/// Hand-trace: `E(1)` folds at the send into a `Created` receiver; the
/// merged member resolves through the extension kind's
/// `check_container_member`; `toString` suppresses (the `on` type
/// carries it).
#[test]
fn extension_application_reports_the_extension_trio() {
    let report = report(
        "class int {} \
             extension E on int { int get g => 1; void m() {} set s(int v) {} int operator +(int o) => 1; } \
             void f() { E(1).h; E(1).n(); E(1).t = 1; E(1) * 2; E(1).toString; }",
    );
    assert_eq!(
        code_names(&report),
        [
            "undefined_extension_getter",
            "undefined_extension_method",
            "undefined_extension_setter",
            "undefined_extension_operator",
        ],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The getter 'h' isn't defined for the extension 'E'."
    );
    assert_eq!(
        report.diagnostics[3].message,
        "The operator '*' isn't defined for the extension 'E'."
    );
}

/// The external-primitive member face (#350): a member access on a
/// dart:core primitive receiver reports the interface trio when the
/// primitive genuinely lacks the name, and stays silent on its real
/// surface (`isEven`, `toString`).
///
/// Hand-trace: `int x` types the receiver `int` — unresolvable as a
/// container, the walk chain-breaks at the root and
/// [`CORE_TYPE_MEMBERS`] decides. Locally declared faces keep the
/// regular walk.
#[test]
fn primitive_receivers_report_the_member_trio() {
    let report = report(
        "void f() { int x = 1; x.noSuchGetter; x.noSuchMethodHere(); x.noSuchSetter = 2; x.isEven; x.toString; }",
    );
    // The receiver type `int` is undeclared in the harness: its type use
    // carries the known `undefined_class` row ahead of the member trio.
    assert_eq!(
        code_names(&report),
        [
            "undefined_class",
            "undefined_getter",
            "undefined_method",
            "undefined_setter",
        ],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[1].message,
        "The getter 'noSuchGetter' isn't defined for the type 'int'."
    );
    assert_eq!(
        report.diagnostics[2].message,
        "The method 'noSuchMethodHere' isn't defined for the type 'int'."
    );
    assert_eq!(
        report.diagnostics[3].message,
        "The setter 'noSuchSetter' isn't defined for the type 'int'."
    );
}

/// A creation result used as an operator receiver reports
/// `undefined_operator` at the operator token (#350); the creation's own
/// instance face still resolves (`Object` members, superclass chains).
///
/// Hand-trace: `B() + 1` — the send folds the class callee into a
/// `Created` receiver; the operator intent walks `B`'s member face and
/// misses `+`. `B().m()` resolves through the superclass edge;
/// `B().toString` suppresses (the Object member every class carries).
#[test]
fn creation_operator_reports_undefined_operator() {
    let report = report(
        "class A { void m() {} } class B extends A {} \
             void f() { B() + 1; B().m(); B().toString; C() + 1; } class C { void nosuch() {} }",
    );
    assert_eq!(
        code_names(&report),
        ["undefined_operator", "undefined_operator"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The operator '+' isn't defined for the type 'B'."
    );
    assert_eq!(
        report.diagnostics[1].message,
        "The operator '+' isn't defined for the type 'C'."
    );
}

/// A member's own type parameters scope over its whole signature
/// (#350): the return-type row pushed before `<T>` parsed re-scopes, so
/// no `undefined_class` fires — and the invocation arity checks still
/// answer.
///
/// Hand-trace: `T m<T>(T x)` — the Executable frame opens before the
/// return type; `begin_type_variable` joins the frame's `type_params`
/// and rewrites the still-missing `T` rows above the frame's row mark.
#[test]
fn member_type_parameters_scope_over_the_signature() {
    let report = report(
        "class D {} class C { T m<T>(T x) => x; static T sm<T>(T x) => x; } \
             void g<T>(D x) {} \
             void f() { var d = D(); g<D, D>(d); }",
    );
    assert_eq!(
        code_names(&report),
        ["wrong_number_of_type_arguments_element"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The function 'g' is declared with 1 type parameters, but 2 type arguments are given."
    );
}

/// Hand-trace: `f` opens the Executable frame; `nose` resolves through
/// locals → type parameters → container face (none) → extension pool →
/// `lookup_scoped` → `NotFound` (`Missing`); the argless send never opens
/// an argument group, so the row stays a read and emits.
#[test]
fn bare_miss_reports_undefined_identifier() {
    let report = report("void f() { nose; }");
    assert_eq!(code_names(&report), ["undefined_identifier"]);
    let d = &report.diagnostics[0];
    assert_eq!(d.message, "Undefined name 'nose'.");
    assert_eq!(d.severity, Severity::Error);
    assert_eq!(d.range.offset, u32::try_from("void f() { ".len()).unwrap());
    assert_eq!(report.names.len(), 1);
    assert_eq!(report.names[0].use_kind, NameUse::Read);
    assert!(!report.names[0].qualified);
}

/// Locals shadow the library scope while in scope; `var` drops any stale
/// declaration type so the bound name stays an unknown receiver.
///
/// Hand-trace: `var x = 0;` clears `last_type`, adds `x` to the Executable
/// frame; `x;` resolves `Plain` (no row); `nose;` reports.
#[test]
fn locals_shadow_and_stay_scoped() {
    let report = report("void f() { var x = 0; x; nose; }");
    assert_eq!(code_names(&report), ["undefined_identifier"]);
    assert_eq!(report.diagnostics[0].message, "Undefined name 'nose'.");
}

/// An unresolved identifier inside a creation's argument list still
/// reports, and co-fires the creation's own const rows — the oracle
/// triple on the `const_eval_type_string` corpus: the creation-level
/// fault at the creation node, the non-constant argument at `x`, the
/// undefined name at `x`. The declaration's own
/// `const_initialized_with_non_constant_value` stays silent: the
/// oracle's evaluation of `B(x)` is unknown (`hasKnownValue` false —
/// the undefined name is the reported root cause), not known-invalid
/// (#2138; oracle 3.13.1 probe on the corpus fixture).
///
/// Hand-trace: `B` resolves as a `BareInfo::Container` callee, so
/// `consume_send` folds the creation and drops the rows above the
/// callee — the argument's `BareInfo::Missing` read defers to pass 2
/// instead of dying with the fold. `class int {}`/`class String {}`
/// follow the harness convention (core names are modeled through
/// concrete declarations — `BUILTIN_TYPES` carries only `dynamic`);
/// the member keeps the `o.length` initializer's walk silent, since a
/// local declaration shadows the external-primitive member face.
#[test]
fn creation_argument_miss_reports_undefined_identifier() {
    let report = report(
        "class int {} class String { final int length = 0; } \
             class B { final l; const B(String o) : l = o.length; } const y = B(x);",
    );
    assert_eq!(
        code_names(&report),
        [
            "const_eval_type_string",
            "const_with_non_constant_argument",
            "undefined_identifier",
        ],
        "{:?}",
        code_names(&report)
    );
    let d = &report.diagnostics[2];
    assert_eq!(d.message, "Undefined name 'x'.");
    let prefix = "class int {} class String { final int length = 0; } \
             class B { final l; const B(String o) : l = o.length; } const y = B(";
    assert_eq!(d.range.offset, u32::try_from(prefix.len()).unwrap());
    assert_eq!(d.range.length, 1);
    assert_eq!(report.names[0].use_kind, NameUse::Read);
    assert!(!report.names[0].qualified);
}

/// An invocation-callee miss is `undefined_function`; a bare read stays
/// `undefined_identifier` (`foo;` never opens an argument group).
///
/// Hand-trace: `foo(` opens a group registering the callee row; at `)`
/// it becomes `pending_send` and `handle_send` consumes it (invoke). The
/// inner identifier of `f(g)` would not: its `handle_send` finds no
/// pending group and leaves the read intact.
#[test]
fn invoke_versus_read() {
    let report = report("void f() { foo(); bar; }");
    assert_eq!(
        code_names(&report),
        ["undefined_function", "undefined_identifier"]
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The function 'foo' isn't defined."
    );
    assert_eq!(report.names[0].use_kind, NameUse::Invoke);
}

/// An assignment target reports `undefined_identifier` (the oracle's
/// code for a fully missing write target); the literal row to the right
/// of the operator is skipped by the scan.
#[test]
fn assignment_target_reports_identifier() {
    let report = report("void f() { x = 1; }");
    assert_eq!(code_names(&report), ["undefined_identifier"]);
    assert_eq!(report.names[0].use_kind, NameUse::Write);
}

/// Type positions report `undefined_class`; `dynamic` is a builtin;
/// in-scope type parameters resolve; file-local classes never reach the
/// type path.
///
/// Hand-trace: `T` sits in `type_params` while the Container frame is
/// open → `Plain`; `Missing` walks `C`'s member face (absent), then the
/// library scope → `NotFound` → `TypeUse(Missing)` → emit; `dynamic`
/// short-circuits on the builtin table.
#[test]
fn type_positions_report_undefined_class() {
    let report = report("class C<T> { T? t; Missing m; dynamic d; }");
    assert_eq!(code_names(&report), ["undefined_class"]);
    assert_eq!(report.diagnostics[0].message, "Undefined class 'Missing'.");
    assert_eq!(report.names[0].use_kind, NameUse::Type);
}

// —— #1445: `augment` with the augmentations gate off ——————————————————
// The parser face of these spellings (identifier fall-back, zero
// diagnostics) is pinned in the parser suite
// (`augment_headed_declaration_name_recovery` and the member-spelling
// pin beside it); the undefined-family face lives here — the parse
// fires the NamedType events, and the member scope chain decides.

/// The gated-off member spellings (`member_declarations.dart` lines
/// 2/4, 3.13.1 oracle): `augment` rides the identifier path, so the
/// member-level type uses resolve through the member scope chain —
/// the recovery member parse leaves no `augment` type here, so both
/// heads miss and report `undefined_class`; the `=> null` getter's
/// host is `InvalidType`, so no `return_of_invalid_type` fires.
#[test]
fn augment_gate_off_member_heads_report_undefined_class() {
    let report = report_augmentations_off(
        "class C {\n  augment method() {}\n  augment get getter => null;\n}\n",
    );
    assert_eq!(code_names(&report), ["undefined_class", "undefined_class"]);
    assert_eq!(report.diagnostics[0].message, "Undefined class 'augment'.");
    assert_eq!(report.diagnostics[1].message, "Undefined class 'augment'.");
}

/// `augment int field;` parses as `<type=augment> <name=int>` followed
/// by an unexpected `field` — the parse rows sit at `int`
/// (`expected_token`) and `field` (`missing_const_final_var_or_type`),
/// and the `augment` type head reports `undefined_class`. The member
/// NAME `int` never re-enters the type chain, so no second
/// `undefined_class` fires — the oracle evidence (`member_declarations
/// .dart` line 11, 3.13.1 machine rows) lists only the `augment` row
/// beside the parse pair; the `Undefined class 'int'.` rows on that
/// file belong to the `late`/`static` variants (lines 14/19+), where
/// recovery lands `int` in the type slot.
#[test]
fn augment_gate_off_recovered_field_name_shadows_type_lookup() {
    let report = report_augmentations_off("class C {\n  augment int field;\n}\n");
    let mut codes = code_names(&report);
    codes.sort_unstable();
    assert_eq!(
        codes,
        [
            "expected_token",
            "missing_const_final_var_or_type",
            "undefined_class"
        ]
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.message == "Undefined class 'augment'.")
    );
}

/// The top-level host face (`top_level_declarations.dart` line 3): the
/// getter's `augment` head resolves to the top-level variable — the
/// resolver's `notAType` face reports `'augment' isn't a type.` at the
/// written head (the oracle's 3.13 row), no `undefined_class` beside it —
/// and the host is `InvalidType`, so the `=> null` body fires no
/// `return_of_invalid_type`.
#[test]
fn augment_gate_off_non_type_return_host_skips_return_check() {
    let report = report_augmentations_off("var augment = 0;\naugment get getter => null;\n");
    assert_eq!(
        code_names(&report),
        ["not_a_type"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].range.offset,
        offset_of(
            "var augment = 0;\naugment get getter => null;\n",
            "augment get"
        )
    );
}

/// The `not_a_type` row carries the oracle's declaration context row
/// when the resolved element's location is carried
/// (named_type_resolver.dart:794-806): "The declaration of 'x' is
/// here." anchored at the parameter's name token.
#[test]
fn not_a_type_carries_the_declaration_context() {
    let source = "class C {\n  void m(int x) {\n    x y;\n  }\n}\n";
    let report = report(source);
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "not_a_type")
        .expect("the not_a_type row");
    let context = row.context.as_ref().expect("the declaration context row");
    assert_eq!(context.len(), 1);
    assert_eq!(&*context[0].message, "The declaration of 'x' is here.");
    assert_eq!(context[0].file, row.file);
    assert_eq!(
        context[0].range.offset,
        offset_of(source, "int x") + u32::try_from("int ".len()).unwrap()
    );
}

/// A genuinely-typed host keeps its return check: the gate keys on the
/// declared head's resolution, never on the return expression.
#[test]
fn resolved_return_host_keeps_the_return_check() {
    let report = report_augmentations_off("class C {}\nC f() => null;\n");
    assert_eq!(code_names(&report), ["return_of_invalid_type"]);
}

/// A declaration header's type parameters scope over its own bounds:
/// the parameter's own bound (`T extends A<T>` — the pinned 3.13.1
/// oracle on `enum_instantiated_to_bounds_is_not_well_bounded.dart` fires
/// no `undefined_class`) and a parameter declared after the reference
/// (`T extends U, U extends num` — the whole list is in scope in every
/// bound). Neither the bound's type argument nor the enum element's
/// explicit arguments report here.
///
/// Hand-trace: `T` parks in `pending_type_params` while its header
/// parses (no frame is open yet), so `resolve_bare` answers `Plain` for
/// `A<T>`'s argument; `U`'s row — pushed `Missing` before `U`'s own
/// `begin_type_variable` — rewrites to `Value` when the frame joins the
/// parked parameters. `Never` and `int` are modeled through concrete
/// declarations (the harness convention).
///
/// Boundedness face on the enum case: the oracle reports two rows —
/// `ENUM_INSTANTIATED_TO_BOUNDS_IS_NOT_WELL_BOUNDED` at the enum head
/// (the self-referential alias bound, `error_verifier.dart:5061-5079`)
/// and `TYPE_ARGUMENT_NOT_MATCHING_BOUNDS` at the `Never` argument
/// (`type_arguments_verifier.dart:176-202`: the local `Never` class is
/// not a subtype of the substituted bound `A<Never>`). We emit the
/// first; the second stays silent because the alias-shaped bound
/// `A<T>` is unmodelable in the bounds snapshot and maps to `dynamic`
/// (the under-report direction, bounds.rs's pre-alias map). With the
/// real `dart:core` `Never` — the corpus fixture — the oracle emits
/// only the enum row and we match.
#[test]
fn header_bounds_scope_their_own_parameters() {
    let enum_case = report(
        "class int {} typedef A<X> = X Function(X); class Never {} \
             enum E<T extends A<T>, U> { v<Never, int>() }",
    );
    assert_eq!(
        code_names(&enum_case),
        ["enum_instantiated_to_bounds_is_not_well_bounded"],
        "{:?}",
        code_names(&enum_case)
    );

    let forward = report("class num {} class C<T extends U, U extends num> {}");
    assert!(
        code_names(&forward).is_empty(),
        "{:?}",
        code_names(&forward)
    );
}

/// The super-bounded-path report carries the oracle's inverted-type
/// context row anchored at the whole type node
/// (`type_arguments_verifier.dart:427-465` — `node.offset/length`, not
/// the offending argument).
#[test]
fn bounds_violation_carries_the_inverted_type_context() {
    let source =
        "class int {} class String {}\nclass A<T extends int> {}\nvoid f(A<String> x) {}\n";
    let report = report(source);
    assert_eq!(
        code_names(&report),
        ["type_argument_not_matching_bounds"],
        "{:?}",
        code_names(&report)
    );
    let context = report.diagnostics[0]
        .context
        .as_ref()
        .expect("the inverted-type context row");
    assert_eq!(context.len(), 1);
    assert_eq!(
        &*context[0].message,
        "The inverted type 'A<String>' is also not regular-bounded, so the type is not well-bounded."
    );
    assert_eq!(context[0].range.offset, offset_of(source, "A<String>"));
    assert_eq!(
        context[0].range.length,
        u32::try_from("A<String>".len()).unwrap()
    );
}

/// `new C()` on a defined class is silent; `new Missing()` is the
/// oracle's `creation_with_non_type` ("The name 'Missing' isn't a
/// class.") — a creation-head miss never reaches `undefined_class`
/// (upstream Dart SDK source, pkg/analyzer/lib/src/dart/resolver/
/// named_type_resolver.dart:313-330 — `resolveConstructorTypeReference`'s
/// tail reports `newWithNonType`/`constWithNonType` for a missing or
/// non-class element; the captured oracle row for the unresolved
/// `new C(;)` recovery head — the bracket_mismatch_06 corpus case —
/// carries the same code).
///
/// Hand-trace: `begin_new_expression` raises `ctor_pos`, so the argument
/// group registers no callee and no send consumption can misfire; the
/// `ConstructorReference` identifier resolves as a type use.
#[test]
fn constructor_reference_resolves_as_type() {
    let report = report("class C {} void f() { new C(); new Missing(); }");
    assert_eq!(code_names(&report), ["creation_with_non_type"]);
    let d = &report.diagnostics[0];
    assert_eq!(d.message, "The name 'Missing' isn't a class.");
    assert_eq!(
        d.correction.as_deref(),
        Some("Try correcting the name to match an existing class.")
    );
}

/// `duplicate_definition` folds from the collector's log, pointing at the
/// SECOND declaration's name token, ahead of the row diagnostics; the
/// model keeps the first entry.
#[test]
fn duplicates_fold_first() {
    let report = report("var x = 0; void f() { nose; } var x = 1;");
    assert_eq!(
        code_names(&report),
        ["duplicate_definition", "undefined_identifier"]
    );
    let dup = &report.diagnostics[0];
    assert_eq!(dup.message, "The name 'x' is already defined.");
    // The second `x`'s token: after `var ` of the third declaration.
    assert_eq!(
        dup.range.offset,
        u32::try_from("var x = 0; void f() { nose; } var ".len()).unwrap()
    );
}

/// The duplicate row carries the oracle's first-definition context row
/// (`diagnostic_factory.dart` `duplicateDefinition`) anchored at the
/// kept declaration's name token.
#[test]
fn duplicate_definition_carries_the_first_definition_context() {
    let report = report("var x = 0;\nvar x = 1;\n");
    assert_eq!(code_names(&report), ["duplicate_definition"]);
    let dup = &report.diagnostics[0];
    let context = dup
        .context
        .as_ref()
        .expect("the first-definition context row");
    assert_eq!(context.len(), 1);
    assert_eq!(&*context[0].message, "The first definition of this name.");
    assert_eq!(context[0].file, dup.file);
    // The kept `x`'s name token: after `var ` of the first declaration.
    assert_eq!(
        context[0].range.offset,
        u32::try_from("var ".len()).unwrap()
    );
    assert_eq!(context[0].range.length, 1);
}

/// A member duplicate anchors at its container's kept member — the
/// capture-grade scope the fold resolves from ([`first_decl_site`]).
#[test]
fn member_duplicate_definition_anchors_the_kept_member() {
    let source = "class T {} class C {\n  T? x;\n  T? x;\n}\n";
    let report = report(source);
    assert_eq!(code_names(&report), ["duplicate_definition"]);
    let context = report.diagnostics[0]
        .context
        .as_ref()
        .expect("the first-definition context row");
    assert_eq!(
        context[0].range.offset,
        offset_of(source, "T? x") + u32::try_from("T? ".len()).unwrap()
    );
}

/// A type-parameter duplicate has no declaration site in the fold's
/// capture: the plain row — the degrade direction, never a foreign
/// anchor.
#[test]
fn type_parameter_duplicate_degrades_to_the_plain_row() {
    let report = report("class C<T, T> {}");
    assert_eq!(code_names(&report), ["duplicate_definition"]);
    assert!(report.diagnostics[0].context.is_none());
}

/// Getter/setter pairs coexist (distinct model spellings); two members
/// of the same static/instance bit are duplicates.
///
/// Hand-trace: `g` and `g=` are distinct keys; both `m` commits hit the
/// same key with the same instance bit — the second logs a duplicate.
#[test]
fn member_duplicates_respect_the_static_bit() {
    let report = report("class N { N get g => N(); set g(N v) {} void m() {} void m() {} }");
    assert_eq!(code_names(&report), ["duplicate_definition"]);
    assert_eq!(
        report.diagnostics[0].message,
        "The name 'm' is already defined."
    );
}

/// A block-local duplicate is `duplicate_definition` at the second
/// declaration — the statement-list face
/// (`duplicate_definition_verifier.dart` `checkStatements`), the local
/// grade the capture-time log's file-level faces don't cover (3.13.1
/// probe: `void f() { var x = 1; var x = 2; x; }` — the duplicate ERROR
/// at the second `x`, the first-declaration context row riding it, one
/// `unused_local_variable` beside it).
#[test]
fn block_local_duplicate_reports_the_second_declaration() {
    let source = "void f() { var x = 1; var x = 2; x; }";
    let report = report(source);
    let mut names = code_names(&report);
    names.sort_unstable();
    assert_eq!(
        names,
        ["duplicate_definition", "unused_local_variable"],
        "{:?}",
        report.diagnostics
    );
    let dup = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "duplicate_definition")
        .unwrap();
    assert_eq!(dup.message, "The name 'x' is already defined.");
    assert_eq!(
        dup.range.offset,
        u32::try_from("void f() { var x = 1; var ".len()).unwrap()
    );
    let context = dup.context.as_ref().expect("the context row");
    assert_eq!(&*context[0].message, "The first definition of this name.");
    assert_eq!(
        context[0].range.offset,
        u32::try_from("void f() { var ".len()).unwrap()
    );
}

/// Nested-scope shadowing stays legal — a nested block opens its own
/// statement-list scope (`checkStatements` per `visitBlock`), so the
/// inner `x` never collides with the outer one.
#[test]
fn nested_scope_shadowing_stays_legal() {
    let report = report("void f() { var x = 1; { var x = 2; x; } x; }");
    assert_eq!(
        report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "duplicate_definition")
            .count(),
        0,
        "{:?}",
        code_names(&report)
    );
}

/// The wildcard-variables feature: `_` never collides (the verifier's
/// `isWildcard` arm skips the name before the scope comparison).
#[test]
fn wildcard_locals_never_collide() {
    let report = report("void f() { var _ = 1; var _ = 2; }");
    assert!(report.diagnostics.is_empty(), "{:?}", code_names(&report));
}

/// A local function's name joins its statement list's scope
/// (`checkStatements`' `FunctionDeclarationStatement` arm): two `g`
/// declarations in one block collide.
#[test]
fn local_function_duplicate_reports() {
    let report = report("void f() { void g() {} void g() {} g(); }");
    assert_eq!(
        report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "duplicate_definition")
            .count(),
        1,
        "{:?}",
        code_names(&report)
    );
}

/// Each switch member's statement list scopes on its own
/// (`checkStatements` per `visitSwitchCase`): a repeat inside one
/// member fires, the same name in the next member stays legal.
#[test]
fn switch_member_duplicate_scopes_per_member() {
    let source = "void f() { switch (1) { case 1: var a = 1; var a = 2; a; break; \
                      case 2: var a = 3; break; } }";
    let report = report(source);
    assert_eq!(
        report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "duplicate_definition")
            .count(),
        1,
        "{:?}",
        code_names(&report)
    );
}

/// A for head's declaration list is its own duplicate scope
/// (`checkForVariables` — the loop's list checks inside itself): a
/// repeat inside one head fires, a fresh head starts clean, and a loop
/// variable never collides with an enclosing block local.
#[test]
fn for_head_duplicates_scope_per_loop() {
    let repeat = report("void f() { for (var i = 0, i = 1; i < 2; i++) {} }");
    assert_eq!(
        repeat
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "duplicate_definition")
            .count(),
        1,
        "{:?}",
        code_names(&repeat)
    );
    let fresh =
        report("void f() { for (var i = 0; i < 1; i++) {} for (var i = 0; i < 1; i++) {} }");
    assert_eq!(
        fresh
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "duplicate_definition")
            .count(),
        0,
        "{:?}",
        code_names(&fresh)
    );
    let shadow = report("void f() { var x = 1; for (var x in [1, 2]) x; }");
    assert_eq!(
        shadow
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "duplicate_definition")
            .count(),
        0,
        "{:?}",
        code_names(&shadow)
    );
}

/// A statement-level pattern declaration's variables join the
/// enclosing statement list's scope (`checkStatements`'
/// `PatternVariableDeclarationStatement` arm): a repeated pair fires
/// twice, once per name.
#[test]
fn pattern_declaration_duplicate_reports_per_name() {
    let source = "void f() { var (a, b) = (1, 2); var (a, b) = (3, 4); a; b; }";
    let report = report(source);
    assert_eq!(
        report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "duplicate_definition")
            .count(),
        2,
        "{:?}",
        code_names(&report)
    );
}

/// An operator on a dart:core collection stub — the generic class type
/// literal (`List < 1`, 3.13.1 probe) — applies to the literal's static
/// type `Type`, which declares no user operator: `undefined_operator`
/// at the operator token. `==` resolves through `Object` and stays
/// silent.
#[test]
fn generic_core_stub_type_literal_operator_reports() {
    let source = "void f() { List < 1; }";
    let report = report_with_core_stubs(source);
    assert_eq!(code_names(&report), ["undefined_operator"]);
    assert_eq!(
        report.diagnostics[0].message,
        "The operator '<' isn't defined for the type 'Type'."
    );
    assert_eq!(report.diagnostics[0].range.offset, offset_of(source, "<"));

    let plus = report_with_core_stubs("void f() { Map + 1; }");
    assert_eq!(
        code_names(&plus),
        ["undefined_operator"],
        "{:?}",
        plus.diagnostics
    );

    let eq = report_with_core_stubs("void f() { List == 1; }");
    assert!(eq.diagnostics.is_empty(), "{:?}", code_names(&eq));
}

/// A name shown by a broken import is suppressed
/// (`shouldIgnoreUndefined`) but still recorded in the names table.
///
/// Hand-trace: the edge matches (target `None`, prefix `None`) and
/// `shows(gone)` holds — the guard fires before the emit.
#[test]
fn broken_import_shown_name_is_suppressed() {
    let report = report_with_edge("void f() { gone; }", |strings| {
        Some(ImportEdge {
            target: None,
            prefix: None,
            deferred: false,
            combinators: Combinators::new(
                [CombinatorClause::Show(
                    vec![strings.intern("gone")].into_boxed_slice(),
                )],
                strings,
            ),
        })
    });
    assert!(report.diagnostics.is_empty());
    assert_eq!(report.names.len(), 1);
    assert_eq!(report.names[0].use_kind, NameUse::Read);
}

/// The const cascade (oracle-pinned, dart 3.15): a prefixed name
/// through a broken import is an unresolved reference, so a const list
/// holding it reports `non_constant_list_element` AND the declaration
/// reports `const_initialized_with_non_constant_value` at the same
/// element; a nested literal's failure surfaces once per code; map
/// entries name the failing side (`non_constant_map_key` / `_value`).
#[test]
fn const_literal_failures_cascade_to_the_declaration() {
    let source = "int nc = 1; const a = [diag.x]; const b = [[nc]]; const c = {nc: 1}; \
                      const d = (nc, 1);";
    let report = report_with_edge(source, |strings| {
        Some(ImportEdge {
            target: None,
            prefix: Some(strings.intern("diag")),
            deferred: false,
            combinators: Combinators::unfiltered(),
        })
    });
    let rows: Vec<(&str, &str)> = report
        .diagnostics
        .iter()
        .map(|d| {
            let at = &source[d.range.offset as usize..d.range.end() as usize];
            (d.code.name, at)
        })
        .collect();
    let expect = [
        ("non_constant_list_element", "diag.x"),
        ("const_initialized_with_non_constant_value", "diag.x"),
        ("non_constant_list_element", "nc"),
        ("const_initialized_with_non_constant_value", "nc"),
        ("non_constant_map_key", "nc"),
        ("non_constant_record_field", "nc"),
    ];
    for row in expect {
        assert!(rows.contains(&row), "{row:?} missing from {rows:?}");
    }
    let nested = rows
        .iter()
        .filter(|r| r.0 == "non_constant_list_element" && r.1 == "nc")
        .count();
    assert_eq!(nested, 1, "{rows:?}");
    assert_eq!(
        rows.iter()
            .filter(|r| r.0 == "const_initialized_with_non_constant_value")
            .count(),
        4,
        "{rows:?}"
    );
}

/// A bare miss under an *unfiltered* broken import still reports — the
/// oracle ignores only show-listed names (`shouldIgnoreUndefined`,
/// element.dart:8217-8250; the C7 shape).
///
/// Hand-trace: the edge matches (target `None`, prefix `None`) but has
/// no `show` clause, so neither guard arm fires and the emit happens.
#[test]
fn unfiltered_broken_import_keeps_bare_miss() {
    let report = report_with_edge("void f() { gone; }", |_| {
        Some(ImportEdge {
            target: None,
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        })
    });
    assert_eq!(code_names(&report), ["undefined_identifier"]);
}

/// A defined prefixed function resolves silently; a prefixed invocation
/// miss through a resolved prefix reports `undefined_function` (the
/// qualified shape of the family's trigger).
///
/// Hand-trace: `p.real` merges with the `Prefix` receiver and
/// `lookup_prefixed` finds `real` — no report; `gone` misses the prefix
/// table and the guard skips resolved imports — the emit fires.
#[test]
fn prefixed_invoke_miss_reports_undefined_function() {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file_a = files.intern(Path::new("a.dart"));
    let file_m = files.intern(Path::new("main.dart"));
    let source_a = "void real() {}";
    let source_m = "import 'a.dart' as p; void f() { p.real(); p.gone(); }";
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let scanned_a = dartq_scanner::scan(
        source_a,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file_a,
    );
    let scanned_m = dartq_scanner::scan(
        source_m,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file_m,
    );
    let elements_a = collect(source_a, &scanned_a.tokens, features, &strings);
    let elements_m = collect(source_m, &scanned_m.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib_a = builder.intern("package:t/a.dart");
    let lib_m = builder.intern("package:t/main.dart");
    let mut index = ElementIndex::default();
    index.insert(file_a, elements_a.clone());
    index.insert(file_m, elements_m.clone());
    for (name, _) in elements_a.declarations() {
        builder.declare(lib_a, name, file_a);
    }
    for (name, decl) in elements_m.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_m, name, file_m);
    }
    builder.import(
        lib_m,
        ImportEdge {
            target: Some(lib_a),
            prefix: Some(strings.intern("p")),
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let report = diagnose(
        source_m,
        &scanned_m.tokens,
        features,
        &strings,
        file_m,
        lib_m,
        &graph,
        &namespaces,
        &index,
    );
    assert_eq!(code_names(&report), ["undefined_function"]);
    assert_eq!(
        report.diagnostics[0].message,
        "The function 'gone' isn't defined."
    );
    assert!(report.names[0].qualified);
}

/// The outside-library rows anchor on their written supertype entries
/// while the subtype-requirement row keeps the declaring name (the E1
/// span parity): the oracle reports the outside-library faces at the
/// supertype's whole type annotation — `.at(superclass)` and the
/// written `implements` NamedType, sealed faces `.at(namedType)` — and
/// the subtype requirement at `.at(nameToken)` (upstream Dart SDK
/// source, pkg/analyzer, error_verifier.dart:5625/:5657/:5927/:7431
/// against the `base_or_final_type_verifier` drive's name anchor). One
/// library declares the modifier faces; the use sites sit in another.
///
/// Hand-trace: each declaration arms at its name event, the clause
/// entries stream their spans through the header parse, and the body
/// open finalizes the use-site rows — `G<int>`'s anchor extends through
/// its closer (the head's completion re-fire), `W` carries the
/// double-row shape (the base-implemented walk at its entry, the
/// subtype requirement at `W` itself).
#[test]
fn modifier_use_site_rows_anchor_written_entries() {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file_a = files.intern(Path::new("a.dart"));
    let file_m = files.intern(Path::new("main.dart"));
    let source_a = "sealed class S {}\nfinal class F {}\nbase class B {}\nsealed class G<T> {}";
    let source_m = concat!(
        "import 'a.dart';\n",
        "\n",
        "class int {}\n",
        "class X extends S {}\n",
        "class Y implements S {}\n",
        "class Z extends F {}\n",
        "class W implements B {}\n",
        "class T implements G<int> {}\n",
    );
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let scanned_a = dartq_scanner::scan(
        source_a,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file_a,
    );
    let scanned_m = dartq_scanner::scan(
        source_m,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file_m,
    );
    let elements_a = collect(source_a, &scanned_a.tokens, features, &strings);
    let elements_m = collect(source_m, &scanned_m.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib_a = builder.intern("package:t/a.dart");
    let lib_m = builder.intern("package:t/main.dart");
    let mut index = ElementIndex::default();
    index.insert(file_a, elements_a.clone());
    index.insert(file_m, elements_m.clone());
    for (name, decl) in elements_a.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_a, name, file_a);
    }
    for (name, decl) in elements_m.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_m, name, file_m);
    }
    builder.import(
        lib_m,
        ImportEdge {
            target: Some(lib_a),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let report = diagnose(
        source_m,
        &scanned_m.tokens,
        features,
        &strings,
        file_m,
        lib_m,
        &graph,
        &namespaces,
        &index,
    );
    let rows: Vec<(&str, u32, u32)> = report
        .diagnostics
        .iter()
        .map(|d| (d.code.name, d.range.offset, d.range.length))
        .collect();
    assert_eq!(
        rows,
        [
            // `class int {}` (the harness has no core names — the
            // `G<int>` argument must resolve and stay silent) opens at
            // 18, so `class X extends S {}` follows at 31: the extends
            // entry `S` sits at 47.
            ("invalid_use_of_type_outside_library", 47, 1),
            // `class Y implements S {}` at 52 — the implements entry
            // `S` (71).
            ("invalid_use_of_type_outside_library", 71, 1),
            // `class Z extends F {}` at 76 — the extends entry `F` (92).
            ("invalid_use_of_type_outside_library", 92, 1),
            // `class W implements B {}` at 97 — the implements entry
            // `B` (116), then the subtype requirement at `W` itself
            // (103).
            ("invalid_use_of_type_outside_library", 116, 1),
            (
                "subtype_of_base_or_final_is_not_base_final_or_sealed",
                103,
                1
            ),
            // `class T implements G<int> {}` at 121 — the entry
            // `G<int>`, name through the closer (140..146); the oracle
            // resolves the cross-library declaration's argument and
            // stays silent (no `non_type_as_type_argument`).
            ("invalid_use_of_type_outside_library", 140, 6),
        ],
        "{:?}",
        rows
    );
}

/// The `@sealed` contract rows anchor on the whole declaring node: the
/// oracle reports `.at(node)` (upstream Dart SDK source, pkg/analyzer,
/// best_practices_verifier.dart:1195/:1202), so the subtype face of a
/// package:meta `@sealed` supertype spans the use-site declaration from
/// its first non-metadata token to its close brace — not the declaring
/// name.
///
/// Hand-trace: the contract drains at the declaration's end event, when
/// the node's close is first known; the sealed supertype sits in
/// another package (the workspace gate restricts other packages only),
/// recognized through `a.dart`'s package:meta import.
#[test]
fn sealed_contract_row_anchors_declaring_node() {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file_a = files.intern(Path::new("a.dart"));
    let file_m = files.intern(Path::new("main.dart"));
    let source_a = "import 'package:meta/meta.dart';\n\n@sealed class S {}";
    let source_m = "import 'a.dart';\n\nclass X extends S {}\n";
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let scanned_a = dartq_scanner::scan(
        source_a,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file_a,
    );
    let scanned_m = dartq_scanner::scan(
        source_m,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file_m,
    );
    let elements_a = collect(source_a, &scanned_a.tokens, features, &strings);
    let elements_m = collect(source_m, &scanned_m.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib_meta = builder.intern("package:meta/meta.dart");
    builder.library_name(lib_meta, "meta");
    let lib_a = builder.intern("package:other/a.dart");
    let lib_m = builder.intern("package:t/main.dart");
    let mut index = ElementIndex::default();
    index.insert(file_a, elements_a.clone());
    index.insert(file_m, elements_m.clone());
    for (name, decl) in elements_a.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_a, name, file_a);
    }
    for (name, decl) in elements_m.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_m, name, file_m);
    }
    builder.import(
        lib_a,
        ImportEdge {
            target: Some(lib_meta),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    builder.import(
        lib_m,
        ImportEdge {
            target: Some(lib_a),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let report = diagnose(
        source_m,
        &scanned_m.tokens,
        features,
        &strings,
        file_m,
        lib_m,
        &graph,
        &namespaces,
        &index,
    );
    let rows: Vec<(&str, u32, u32)> = report
        .diagnostics
        .iter()
        .map(|d| (d.code.name, d.range.offset, d.range.length))
        .collect();
    assert_eq!(
        rows,
        [("subtype_of_sealed_class", 18, 20)],
        // `class X extends S {}` starts at 18 (the two `import` line
        // bytes plus the blank line) and closes at 37: the whole
        // declaring node from its first token, not the name.
        "{:?}",
        rows
    );
}

/// Named arguments are membership-tested against the callee's formals:
/// a resolved callee with a missing label reports
/// `undefined_named_parameter` at the label; an unresolved callee stays
/// silent for the label (its own invocation code reports).
///
/// Hand-trace: `g`'s group is open while the labels fire, so
/// `top_level_decl(g)` resolves and its named formals are `["n"]`; `n`
/// is contained (silent), `bad` misses a concrete formal table → emit +
/// record. `nos` misses `top_level_decl` (unresolved) → `bad2` records
/// only, and the invocation itself is `undefined_function`. The formal
/// carries no `var` — the ^3.13 walk-end gate
/// (parser_impl.dart:2281-2285; landing #1999) reports
/// `extraneous_modifier` on it, so the fixture spells it untyped.
#[test]
fn named_argument_miss_reports_on_resolved_callee() {
    let report = report("void g({n}) {} void f() { g(n: 1, bad: 2); nos(bad2: 3); }");
    assert_eq!(
        code_names(&report),
        ["undefined_named_parameter", "undefined_function"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The named parameter 'bad' isn't defined."
    );
    let named: Vec<_> = report
        .names
        .iter()
        .filter(|row| row.use_kind == NameUse::NamedArgument)
        .collect();
    assert_eq!(named.len(), 2, "both labels feed the names table");
}

/// The membership test also covers an invoked qualified selector:
/// `c.m(b: 1)` against a method whose formals are positional-only
/// resolves `m` through the typed receiver's member formals and reports
/// `undefined_named_parameter` at the label (`type_analyzer.dart:5820`).
/// The arity check runs beside it — `undefined_named_parameter` does
/// NOT suppress `not_enough_positional_arguments` (`type_analyzer.dart
/// :5843`: the named loop and the deficit check are independent), and
/// the named face carries the written member name
/// (`NamedFunctionInvocation`, ast.dart:37828-37834 — capture-proven on
/// the exact shape).
///
/// Hand-trace: the parser fires the dot event for an invoked selector
/// after the send, so the label sees the callee row still as the
/// selector's placeholder sitting on its receiver row — the check
/// classifies the pair (`receiver_of_bare` → `Receiver::Typed{C}`) and
/// walks `C.m`'s formals (positional `a`, no named entry) → resolved,
/// not defined → emit at the `b` label. The deficit row anchors on the
/// same label (the first token inside the parens at zero positional
/// arguments, `type_analyzer.dart:5843-5849`).
///
/// The fixture's `var` return spelling carries the parser's own rider:
/// `var` before a member spelling is the `varReturnType` syntactic
/// error (`parser_impl.dart:4380-4390` reports it whenever the member
/// is followed by `(`/`{`/`<`/`.`/`=>`; registry row diagnostic.g.dart
/// :18381, "The return type can't be 'var'."), so the pin carries the
/// third code beside the invocation faces. The formal itself is
/// untyped: a `var` formal would take the ^3.13 walk-end gate's
/// `extraneous_modifier` row (parser_impl.dart:2281-2285; landing
/// #1999) beside the pinned faces.
#[test]
fn named_argument_miss_reports_on_member_callee() {
    let report = report("class C { var m(a) => a; } void use(C c) { c.m(b: 1); }");
    let mut codes = code_names(&report);
    codes.sort_unstable();
    assert_eq!(
        codes,
        [
            "not_enough_positional_arguments",
            "undefined_named_parameter",
            "var_return_type"
        ],
        "{:?}",
        code_names(&report)
    );
    let named = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "undefined_named_parameter")
        .expect("named-parameter row present");
    assert_eq!(named.message, "The named parameter 'b' isn't defined.");
    // The label's own span: the `b` in `c.m(b: 1)` (offset 47, length 1).
    assert_eq!(named.range.offset, 47);
    assert_eq!(named.range.length, 1);
    let arity = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "not_enough_positional_arguments")
        .expect("arity row present");
    // The oracle anchors the deficit at the same label and names the
    // member (`NamedFunctionInvocation` covers qualified sends).
    assert_eq!(arity.range.offset, 47);
    assert_eq!(
        arity.message,
        "1 positional argument expected by 'm', but 0 found."
    );
}

/// The declaration-shape discriminator for the corpus miss
/// (`undefined_member_matrix.dart:17`): the pin above covers only the
/// `var`/`var` shape — its return the parser drops as a modifier
/// (members.rs:1070) — so no green pin exercised a typed return through
/// this face. The oracle runs the membership test on the resolved
/// invocation whatever the return type's spelling
/// (`type_analyzer.dart:5742-5822`), so `int m(int a) => a;` must
/// report the label exactly like `var m(var a) => a;`.
///
/// Hand-trace: the label fires with the callee row still the qualified
/// placeholder sitting on its receiver (`c` binds `Typed{C}`), the walk
/// resolves `C.m` (the corpus diff shows no `undefined_method` for this
/// invocation), so the membership test runs on `C.m`'s captured formals
/// — positional `a`, no named entry → resolved, not defined → emit at
/// the `b` label.
///
/// The fixture carries the harness's `class int {}` stub (`report()`
/// has no core library): without it the head's bare `int` lands as a
/// `TypeUse(Missing)` row — an `undefined_class` at pass 2 that would
/// crowd the exact-code pin (the #537 convention).
#[test]
fn named_argument_miss_reports_on_typed_member_callee() {
    let report = report("class int {} class C { int m(int a) => a; } void use(C c) { c.m(b: 1); }");
    let mut codes = code_names(&report);
    codes.sort_unstable();
    assert_eq!(
        codes,
        [
            "not_enough_positional_arguments",
            "undefined_named_parameter"
        ],
        "{:?}",
        code_names(&report)
    );
    let named = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "undefined_named_parameter")
        .expect("named-parameter row present");
    assert_eq!(named.message, "The named parameter 'b' isn't defined.");
    // The label's own span: the `b` in `c.m(b: 1)` (length 1 — the typed
    // declaration is the var one's exact width; the 13-byte `class int
    // {} ` stub shifts the label from the var pin's 51 to 64).
    assert_eq!(named.range.offset, 64);
    assert_eq!(named.range.length, 1);
    let arity = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "not_enough_positional_arguments")
        .expect("arity row present");
    // Co-fired at the same label anchor, member-named (oracle capture:
    // the named loop and the deficit check are independent).
    assert_eq!(arity.range.offset, 64);
    assert_eq!(
        arity.message,
        "1 positional argument expected by 'm', but 0 found."
    );
}

/// The full `undefined_member_matrix.dart` corpus face in one engine
/// run: the member-undefined family on one nominal receiver — getter
/// read, setter write, bare invocation, an undeclared named label on a
/// declared method, and a binary operator the class declares none of.
/// `print` is declared locally because the unit harness has no
/// dart:core priming (its absence would add `undefined_function` rows
/// the SDK-backed oracle run never sees).
///
/// The five codes assert in source order (offset-sorted; the
/// named-parameter row emits at label time during the walk, the other
/// four at pass 2, so the report's raw order interleaves differently).
/// The named-parameter row is #591's acceptance test: the corpus file
/// flips to matched once the typed member callee's captured formals
/// ride the membership read.
#[test]
fn undefined_member_matrix_corpus_matches_oracle() {
    // The `class int {}` stub is the harness's core stand-in (`report()`
    // has no dart:core priming — the #537 convention): without it every
    // bare `int` in the declaration heads lands as a `TypeUse(Missing)`
    // row, an `undefined_class` at pass 2 that crowds the five-code pin.
    let source = "class int {}\n\
             class C {\n\
             \x20 int m(int a) => a;\n\
             }\n\
             \n\
             void print(int x) {}\n\
             \n\
             void use(C c) {\n\
             \x20 print(c.noSuchGetter);\n\
             \x20 c.noSuchSetter = 1;\n\
             \x20 c.noSuchMemberHere();\n\
             \x20 c.m(b: 1);\n\
             \x20 print(c + c);\n\
             }\n";
    // The named-parameter row anchors on the label: the `b` of
    // `c.m(b: 1);` — the source prefix through the label's opening paren,
    // so the expected offset derives from the fixture instead of a
    // hand-count.
    let label_head = "class int {}\n\
             class C {\n\
             \x20 int m(int a) => a;\n\
             }\n\
             \n\
             void print(int x) {}\n\
             \n\
             void use(C c) {\n\
             \x20 print(c.noSuchGetter);\n\
             \x20 c.noSuchSetter = 1;\n\
             \x20 c.noSuchMemberHere();\n\
             \x20 c.m(";
    let report = report(source);
    let mut by_offset: Vec<(u32, &str)> = report
        .diagnostics
        .iter()
        .map(|d| (d.range.offset, d.code.name))
        .collect();
    // Stable: the named-parameter and arity rows share the label anchor
    // and the walk's raw order (named at label time, arity at the send's
    // close) is the pinned order.
    by_offset.sort_by_key(|(offset, _)| *offset);
    let codes: Vec<&str> = by_offset.into_iter().map(|(_, code)| code).collect();
    // The arity row co-fires beside `undefined_named_parameter`
    // (`type_analyzer.dart:5843` — the named loop and the deficit check
    // are independent; capture-proven on this exact corpus shape at
    // 17:7, `1 positional argument expected by 'm', but 0 found.`).
    assert_eq!(
        codes,
        [
            "undefined_getter",
            "undefined_setter",
            "undefined_method",
            "undefined_named_parameter",
            "not_enough_positional_arguments",
            "undefined_operator",
        ],
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| &d.code.name)
            .collect::<Vec<_>>()
    );
    // The named-parameter row anchors on the label: the `b` of
    // `c.m(b: 1);` — the label sits right after the fixture's `c.m(`
    // prefix, so the expected offset derives from that prefix instead of
    // a hand-count.
    let named = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "undefined_named_parameter")
        .expect("named-parameter row present");
    let expected_offset = u32::try_from(label_head.len()).unwrap();
    assert_eq!(named.range.offset, expected_offset);
    assert_eq!(named.range.length, 1);
    assert_eq!(named.message, "The named parameter 'b' isn't defined.");
}

/// The named-argument membership test runs only against a real
/// top-level function's formals (`type_analyzer.dart:5811-5824`: the
/// code fires when the invoked target's named-parameter table exists
/// and lacks the name). A class callee is a constructor invocation — a
/// constructor accepting the label is silent — and a getter callee is a
/// property access; both previously false-positived on their
/// `formals: None` declarations.
///
/// Hand-trace: `C(x: 1)`'s bare head resolves `Container`, and the
/// filtered `top_level_decl` leaves it unresolved (silent); `g(x: 1)`'s
/// getter declaration carries no formals (silent); `bad` on the real
/// function `g` still reports. The formals carry no `var`: at the ^3.13
/// floor the parser's walk-end gate reports `extraneous_modifier` on a
/// `var`/`final` formal of any non-primary-constructor head
/// (parser_impl.dart:2281-2285; landing #1999), so the fixtures use
/// untyped formals.
#[test]
fn named_argument_gates_on_a_real_function_formals_table() {
    let class_callee = report("class C { C({x}); } void f() { C(x: 1); }");
    assert!(
        class_callee.diagnostics.is_empty(),
        "{:?}",
        code_names(&class_callee)
    );
    let getter_callee = report("class C {} C get g => C(); void f() { g(x: 1); }");
    assert!(
        getter_callee.diagnostics.is_empty(),
        "{:?}",
        code_names(&getter_callee)
    );
    let real_miss = report("void g({n}) {} void f() { g(bad: 1); }");
    assert_eq!(code_names(&real_miss), ["undefined_named_parameter"]);
    assert_eq!(
        real_miss.diagnostics[0].message,
        "The named parameter 'bad' isn't defined."
    );
}

/// `await x;` in a sync body keeps the official declaration parse
/// (`gate_351.rs::await_statement_shapes_follow_official_dispatch`):
/// the engine sees `await` as an unresolved type-use row and reports
/// `undefined_identifier_await` at the type name's span (no arguments);
/// `x` is the declared variable — no `undefined_identifier`. In `g`'s
/// async body the parser reads `await` as a real await-expression
/// reading the defined `f` — nothing fires, the vector stays a single
/// row (named_type_resolver.dart:814-818, `property_element_resolver`
/// .dart:2143; `COMPILE_TIME_ERROR`, severity `Error`).
#[test]
fn await_in_sync_body_reports_await_code() {
    let report = report("void f() { await x; } void g() async { await f; }");
    // In a sync body `await x` recovers as the type `await` declaring
    // the local `x`, which is never read — the official read-only use
    // rule reports `unused_local_variable` at its name.
    assert_eq!(
        code_names(&report),
        ["undefined_identifier_await", "unused_local_variable"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "Undefined name 'await' in function body not marked with 'async'."
    );
    // The NamedType node's span: the `await` name token (offset 11,
    // length 5).
    assert_eq!(report.diagnostics[0].range.offset, 11);
    assert_eq!(report.diagnostics[0].range.length, 5);
}

/// Defined names never report: a top-level function as a value, and a
/// static member through its class face.
///
/// Hand-trace: `fn` resolves `Plain`; `C` resolves `Container` and the
/// merged member row hits `C`'s member map (`s`) — both silent, no names-table
/// rows (defined references are the index's rows, not the recovery
/// surface's).
#[test]
fn defined_names_never_report() {
    let report = report("class C { static var s = 0; } void fn() {} void f() { fn; C.s; }");
    assert!(report.diagnostics.is_empty(), "{:?}", code_names(&report));
    assert!(report.names.is_empty());
}

// —— batch 2 (#121): wrong_number_of_type_arguments ——————————————————

/// A type annotation's arity is the declaration's type-parameter count.
///
/// Hand-trace: `C` fires `TypeReference` (resolves — no row) and arms the
/// pending head; `begin_type_arguments` moves the head into the group
/// slot; `end_type_arguments(1)` pops it and checks `C`'s declared count
/// (2, from the model) against 1 → base code at the group. `A` resolves
/// (silent); `handle_type(C)` then binds `x`'s declared type.
#[test]
fn annotation_arity_reports_base_code() {
    let report = report("class A {} class C<T, U> {} void f() { C<A> x; }");
    // `x` is declared, never read — the official read-only use rule
    // reports `unused_local_variable` after the arity row.
    assert_eq!(
        code_names(&report),
        ["wrong_number_of_type_arguments", "unused_local_variable",],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The type 'C' is declared with 2 type parameters, but 1 type arguments were given."
    );
    assert!(report.names.is_empty());
}

/// A creation type reference reports the base code too
/// (oracle `test_const_nonGeneric`).
///
/// Wave-3 event shape: `C` fires `ConstructorReference` inside the
/// `const` context (a resolved head pushes no row), so the group checks
/// the class face through `creation_ref` — declared count (0) against 1
/// → base code at the group. `new C()` stays silent. The `const`
/// keyword's own face (`const_with_non_const` — the class declares no
/// const constructor) anchors at the `const` keyword, which precedes the
/// `<A>` group, so the machine output lists it before the arity row
/// (oracle probe: `CONST_WITH_NON_CONST` at the `const` keyword then
/// `WRONG_NUMBER_OF_TYPE_ARGUMENTS` at `<A>`).
#[test]
fn creation_arity_reports_base_code() {
    let report = report("class A {} class C {} void f() { new C(); const C<A>(); }");
    assert_eq!(
        code_names(&report),
        ["const_with_non_const", "wrong_number_of_type_arguments",],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[1].message,
        "The type 'C' is declared with 0 type parameters, but 1 type arguments were given."
    );
}

/// Type arguments on a named constructor segment report `_constructor`:
/// constructors never declare type parameters (`ast_rewrite.dart:549`).
///
/// Wave-3 event shape: `C` fires `ConstructorReference` and `named`
/// fires `ConstructorReferenceContinuation` — both land in
/// `creation_ref` (resolved heads push no rows), so the group reports
/// the `(class, ctor)` pair for any count while the class face
/// resolves. `D` resolves, so no other code fires.
///
/// Oracle (single-file `dart analyze --format=machine` run):
/// `WRONG_NUMBER_OF_TYPE_ARGUMENTS_CONSTRUCTOR` on `<D>` —
/// "The constructor 'C.named' doesn't have type parameters." — the only
/// diagnostic; matched exactly.
#[test]
fn named_constructor_type_args_report_constructor_code() {
    let report = report("class D {} class C { C.named(); } void f() { new C.named<D>(); }");
    assert_eq!(
        code_names(&report),
        ["wrong_number_of_type_arguments_constructor"]
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The constructor 'C.named' doesn't have type parameters."
    );
}

/// An enum constant's constructor type arguments report `_enum`
/// (`type_arguments_verifier.dart:163`).
///
/// Hand-trace: `handle_enum_element` snapshots the enclosing enum's
/// declared count (2, from the collector's depth-1 group); the constant's
/// group closes with 1 → counts differ → `_enum` at the group. `a`
/// itself fires `EnumValueDeclaration` (no row), so the trailing `()`
/// opens a group with no callee — silent.
#[test]
fn enum_constant_type_args_report_enum_code() {
    let report = report("class D {} enum E<T, U> { a<D>(); }");
    assert_eq!(code_names(&report), ["wrong_number_of_type_arguments_enum"]);
    assert_eq!(
        report.diagnostics[0].message,
        "The enum is declared with 2 type parameters, but 1 type arguments were given."
    );
}

/// The `_enum` snapshot owns only the constant's own type-argument
/// group: a generic invocation inside the constructor arguments checks
/// its own head (`h<A>` matches `h`'s declared count) — the enum's
/// declared count never leaks into it.
///
/// Hand-trace: `a`'s `(` opens an argument list before `h<A>`'s group
/// closes, so the group runs with `arg_levels` non-empty and the
/// `in_enum_element` arm skips; `end_implicit_creation` checks `h`
/// (declares 1, given 1 — silent). The constant's own group is absent,
/// so no `_enum` either.
#[test]
fn enum_constant_argument_groups_check_their_own_heads() {
    let report = report("class A {} void h<T>() {} enum E<T, U> { a(h<A>()); }");
    assert!(
        code_names(&report)
            .iter()
            .all(|name| !name.starts_with("wrong_number_of_type_arguments")),
        "{:?}",
        code_names(&report)
    );
}

/// The selector phrase checks the same snapshot: the constant's type
/// arguments attach to the HEAD identifier (`a<int, int>.other()`,
/// `parseEnumElement` parses one group at the head —
/// parser_impl.dart:2904-2947; type arguments after the selector are
/// the `constructorWithTypeArguments` recovery error,
/// parser_impl.dart:8884-8899, never a semantic code). 2 declared,
/// 2 given, and the declared selector stay silent.
#[test]
fn enum_constant_selector_type_args_check_the_enum_count() {
    let report = report("class int {} enum E<T, U> { a<int, int>.other(); const E.other(); }");
    assert!(code_names(&report).is_empty());
}

/// The enum constant's implicit constructor invocation checks its
/// argument list against the selected constructor's formals: a type
/// mismatch reports `const_constructor_param_type_mismatch` +
/// `argument_type_not_assignable` at the argument's span (the oracle's
/// const-constructor check runs before the general assignability
/// verifier's row). `low(1)` into `const Level5(this.describe)` — the
/// `int` literal refuses `String`.
#[test]
fn enum_constant_argument_checks_the_constructor_formals() {
    let report = report(
        "class int {} class String {} \
             enum Level5 { low(1), high(2); const Level5(this.describe); final String describe; }",
    );
    assert_eq!(
        code_names(&report),
        [
            "const_constructor_param_type_mismatch",
            "argument_type_not_assignable",
            "const_constructor_param_type_mismatch",
            "argument_type_not_assignable",
        ],
        "{:?}",
        code_names(&report)
    );
    // Both rows anchor at the argument (the `1` / `2` literal), not the
    // constant name.
    let arg0 = report.diagnostics[0].range;
    let arg1 = report.diagnostics[2].range;
    assert_eq!(
        report.diagnostics[0].message,
        "A value of type 'int' can't be assigned to a parameter of type 'String' in a const constructor."
    );
    assert_eq!(
        report.diagnostics[1].message,
        "The argument type 'int' can't be assigned to the parameter type 'String'. "
    );
    assert_ne!(arg0, arg1);
}

/// A named-selector constant (`low.named(1)`) checks the named
/// constructor's formals — the selector names `ctor_formals`'s key.
#[test]
fn enum_constant_named_selector_checks_its_own_formals() {
    let report = report(
        "class int {} class String {} \
             enum E { low.named(1), high.named('x'); const E(this.d); const E.named(this.d); final String d; }",
    );
    assert_eq!(
        code_names(&report),
        [
            "const_constructor_param_type_mismatch",
            "argument_type_not_assignable",
        ],
        "{:?}",
        code_names(&report)
    );
}

/// A positional deficit on the constant reports
/// `not_enough_positional_arguments` with the enum's name in the message;
/// a surplus reports `extra_positional_arguments` at the extra argument.
#[test]
fn enum_constant_argument_count_faces() {
    let deficit = report(
        "class int {} class String {} \
             enum E { low(); const E(this.d, this.c); final String d; final int c; }",
    );
    assert_eq!(
        code_names(&deficit),
        ["not_enough_positional_arguments"],
        "{:?}",
        code_names(&deficit)
    );
    assert_eq!(
        deficit.diagnostics[0].message,
        "2 positional arguments expected by 'E', but 0 found."
    );
    let surplus = report(
        "class int {} class String {} \
             enum E { low('a', 'b', 'c'); const E(this.d, this.c); final String d; final int c; }",
    );
    assert!(
        code_names(&surplus).contains(&"extra_positional_arguments"),
        "{:?}",
        code_names(&surplus)
    );
}

/// An invoked generic function with explicit type arguments reports
/// `_element` with the `function` kind word (the oracle's target
/// dispatch, `type_instantiation_target.dart`: the invocation syntax
/// parses as an implicit creation and resolution reports through
/// `InvocationTargetExecutableElement`).
///
/// Hand-trace: `g<A, A>()` parses as an implicit creation (`ctor_pos`
/// raised), so `g` stays a read and the group defers;
/// `end_implicit_creation` pops the deferred entry, sees a bare
/// top-level function head, and checks its declared count (1) against 2.
/// The `A` arguments resolve — no other code.
#[test]
fn generic_invocation_arity_reports_element_function() {
    let report = report("class A {} void g<T>() {} void f() { g<A, A>(); }");
    assert_eq!(
        code_names(&report),
        ["wrong_number_of_type_arguments_element"]
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The function 'g' is declared with 1 type parameters, but 2 type arguments are given."
    );
}

/// An invoked generic member method reports `_element` with the `method`
/// kind word through the send path.
///
/// Wave-3 event shape: the send closes with the group deferred against
/// `m`'s unmerged member placeholder row, and only then does the
/// dot-access event merge `c.m` — the deferred check classifies the
/// receiver from the row below the placeholder and walks `C`'s members,
/// finding `m` with 2 type parameters against the given 1. `D`
/// resolves, so no other code fires.
///
/// Oracle (single-file `dart analyze --format=machine` run):
/// `WRONG_NUMBER_OF_TYPE_ARGUMENTS_ELEMENT` on `<D>` — "The method 'm'
/// is declared with 2 type parameters, but 1 type arguments are
/// given." — the only diagnostic; matched exactly.
#[test]
fn member_invocation_arity_reports_element_method() {
    let report = report("class D {} class C { void m<T, U>() {} } void f(C c) { c.m<D>(); }");
    assert_eq!(
        code_names(&report),
        ["wrong_number_of_type_arguments_element"]
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The method 'm' is declared with 2 type parameters, but 1 type arguments are given."
    );
}

/// A getter invoked or applied with type arguments stays silent: the
/// oracle names the getter kind word (or the function type display),
/// neither of which M1 models — under-report, never a wrong code.
///
/// Hand-trace: `g`'s top-level declaration carries no formals (no
/// parameter list), so the `formals.is_some()` filter suppresses both
/// the send path and the application path.
#[test]
fn getter_type_arguments_suppress() {
    let report = report("class A {} class C {} C get g => C(); void f() { g<A, A>(); g<A>; }");
    assert!(report.diagnostics.is_empty(), "{:?}", code_names(&report));
}

/// An applied (non-invoked) top-level function tear-off reports
/// `_element` with the `function` kind word
/// (`function_reference_resolver.dart` →
/// `InvocationTargetExecutableElement`).
///
/// Hand-trace: `g<A, A>;` never opens an argument group; the application
/// event consumes the deferred entry against `g`'s declared count.
///
/// RED (parser gap, #339): in expression-statement context the
/// declaration probe consumes `g<A, A>` as a parameterized type (the
/// `;` follower reads as a declaration end), so the statement parses as
/// a nameless declaration head and the engine receives no identifier or
/// application events (observed output: `[]`). Un-reds when the
/// `<...>`-group scan in statement position falls back to the expression
/// grammar once no name follows (official computeType posture).
#[test]
fn function_reference_arity_reports_element_function() {
    let report = report("class A {} void g<T>() {} void f() { g<A, A>; }");
    assert_eq!(
        code_names(&report),
        ["wrong_number_of_type_arguments_element"]
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The function 'g' is declared with 1 type parameters, but 2 type arguments are given."
    );
}

/// An implicit call reference (`c<int>;` on a callable class) reports
/// `_element` naming `call` with the `method` kind word (the oracle's
/// `_getCallMethod` resolves the `call` method element).
///
/// Wave-3 event shape: the statement parses as a send closed at `<`
/// followed by a real type-argument group and its application event —
/// the group defers against the row at the group's base mark (`c`'s,
/// below its own `TypeUse` argument rows), and the application walks
/// `C.call`, finding a method with 1 type parameter against the given
/// 2.
///
/// Oracle (single-file `dart analyze --format=machine` run): the same
/// fixture reports `WRONG_NUMBER_OF_TYPE_ARGUMENTS_ELEMENT` on
/// `<A, A>` followed by `NON_TYPE_AS_TYPE_ARGUMENT` ×2 on the `A`
/// arguments (this fixture defines no `A`). The modeled report is the
/// element code then the two context faces, in the oracle's order.
#[test]
fn implicit_call_reference_reports_element_call() {
    let report = report("class C { T call<T>(T x) => x; } void f(C c) { c<A, A>; }");
    assert_eq!(
        code_names(&report),
        [
            "wrong_number_of_type_arguments_element",
            "non_type_as_type_argument",
            "non_type_as_type_argument",
        ]
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The method 'call' is declared with 1 type parameters, but 2 type arguments are given."
    );
    // The group span `<A, A>`, then the two argument names.
    assert_eq!(report.diagnostics[0].range.offset, 48);
    assert_eq!(report.diagnostics[0].range.length, 6);
    assert_eq!(report.diagnostics[1].range.offset, 49);
    assert_eq!(report.diagnostics[1].range.length, 1);
    assert_eq!(report.diagnostics[2].range.offset, 52);
    assert_eq!(report.diagnostics[2].range.length, 1);
}

/// An explicit extension override with mismatched type arguments reports
/// `_extension` (`extension_member_resolver.dart:380`).
///
/// Wave-3 event shape: `Ex<A>(a);` parses as a plain invocation — no
/// implicit-creation events — so the group defers against `Ex`'s
/// container row and the send's deferred check dispatches the extension
/// face: declared count (0) against 1, reported at the group. The
/// trailing `(a)` stays silent.
///
/// Oracle (single-file `dart analyze --format=machine` run): the same
/// fixture also reports `extension_override_without_access` on
/// `Ex<A>(a)` — outside the modeled subset, so the modeled report is
/// `_extension` alone.
#[test]
fn extension_override_arity_reports_extension_code() {
    let report = report("class A {} extension Ex on A {} void f(A a) { Ex<A>(a); }");
    assert_eq!(
        code_names(&report),
        ["wrong_number_of_type_arguments_extension"]
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The extension 'Ex' is declared with 0 type parameters, but 1 type arguments were given."
    );
}

/// An unnamed extension's on-clause type reference reads the extended
/// class's declared count: a matching group stays silent (the oracle's
/// `generic_extension_members.dart` shape — `extension<T> on I<T>` over
/// `class I<T>`; measured `dart analyze` carries no row on the
/// declaration).
///
/// Hand-trace of the over-report this pins against: the unnamed
/// extension's container frame name interns the empty spelling, which
/// resolves to nothing (unnamed extensions are absent from the container
/// index), so `resolve_bare`'s enclosing-frame member arm breaks its
/// walk and answers the on-clause head `I` as a member `Plain`; the
/// `Plain` arm's `TopDecl` fallback then read the class's bare
/// declaration (count 0) and reported `wrong_number_of_type_arguments`
/// ("declared with 0 type parameters") at the group. The `Plain` arm now
/// consults the container model first, so the declared count is the
/// class's own 1 and the group stays silent.
#[test]
fn unnamed_extension_on_clause_matching_arity_stays_silent() {
    let report = report("class I<T> {} extension<T> on I<T> {}");
    assert!(code_names(&report).is_empty());
}

/// The unnamed-extension on-clause face itself stays armed: a genuine
/// mismatch still reports the base code with the class's real count (the
/// repair fixes the declared-count read, it does not retire the face —
/// the oracle checks every resolved `NamedType`, on-clause types
/// included, `_buildTypeArguments` in `named_type_resolver.dart`).
#[test]
fn unnamed_extension_on_clause_arity_mismatch_reports() {
    let report = report("class I<T> {} class S {} class R {} extension<T> on I<S, R> {}");
    assert_eq!(code_names(&report), ["wrong_number_of_type_arguments"]);
    assert_eq!(
        report.diagnostics[0].message,
        "The type 'I' is declared with 1 type parameters, but 2 type arguments were given."
    );
}

/// A named extension's on-clause keeps the resolved-container read (the
/// red-line face): the frame name resolves, `resolve_bare` falls through
/// its member arm to the scope walk, and the mismatch reports against
/// the class's declared count.
#[test]
fn named_extension_on_clause_arity_mismatch_reports() {
    let report = report("class I<T> {} class S {} class R {} extension Ex on I<S, R> {}");
    assert_eq!(code_names(&report), ["wrong_number_of_type_arguments"]);
    assert_eq!(
        report.diagnostics[0].message,
        "The type 'I' is declared with 1 type parameters, but 2 type arguments were given."
    );
}

// —— #537: the m1-gate r2 faces (corpus lib/targeted shapes) ——————————
// The r2 record's five oracle-only misses, pinned on the corpus shapes.
// Every fixture carries the harness's core stubs (`class int {}`,
// `class String {}`, `void print(x) {}` — `report()` has no core
// library): without them the group's `int`/`String` arguments land as
// `TypeUse(Missing)` rows — each an `undefined_class` at pass 2 AND a
// phantom row that displaces the true callee in `end_type_arguments`'
// creation-branch last-row scan, silently killing the arity check. The
// oracle citations are the record's measured message samples
// (dart 3.13.1, goal #125's drift table): the invocation family reports
// `_element` (type_instantiation_target.dart:40,
// `InvocationTargetExecutableElement`), applied non-invoked groups
// report `_function` (:139, `InvocationTargetFunctionTypedExpression`)
// or the type-defining base code (:221), and explicit extension
// applications report `_extension` (:116).

/// A no-keyword constructor reference `C<int, String>.named` reports the
/// base type-argument code against the class face — the `_constructor`
/// variant stays unreachable for it (goal #125 drift: the keyword shapes
/// own that code; r2 corpus `wrong_number_of_type_arguments_constructor`).
///
/// Hand-trace: `C` parses as a send whose `<`-group fails the `(`-only
/// gate (expressions.rs:1570-1577), so `parse_method_type_args_block`
/// (:256) re-reads the group — follower `.` passes `_mayFollowTypeArgs`
/// (:674) — and the group defers against `C`'s container row
/// (`end_type_arguments`' `base - 1`); the application event consumes it
/// through the `Bare(Container)` arm → `check_annotation_arity`. The
/// `.named` selector resolves through the creation face
/// (`check_container_member`'s constructor exclusion) and stays silent;
/// `c` is read by `print`.
#[test]
fn constructor_reference_arity_reports_base_code() {
    let report = report(
        "class int {} class String {} void print(x) {} class C<T> { C.named(); } void main() { var c = C<int, String>.named; print(c); }",
    );
    assert_eq!(
        code_names(&report),
        ["wrong_number_of_type_arguments"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The type 'C' is declared with 1 type parameters, but 2 type arguments were given."
    );
    // The group span `<int, String>`.
    assert_eq!(report.diagnostics[0].range.offset, 95);
    assert_eq!(report.diagnostics[0].range.length, 13);
}

/// A static-method invocation through the class (`C.sm<int, String>()`)
/// reports `_element` with the `method` kind word (r2 corpus
/// `wrong_number_of_type_arguments_element`, the drift's measured
/// canonical invocation shape).
///
/// Hand-trace: the `.sm` dot-access merges into a `Member{recv}` row
/// before the group (expressions.rs:439-445), the group defers against
/// it, and `consume_send`'s deferred check walks `C`'s members
/// (`check_invocation_arity`'s `Member` arm) finding `sm` with 1 type
/// parameter against the given 2.
#[test]
fn static_invocation_arity_reports_element_method() {
    let report = report(
        "class int {} class String {} void print(x) {} class C<T> { static int sm<T>() => 0; } void main() { print(C.sm<int, String>()); }",
    );
    assert_eq!(
        code_names(&report),
        ["wrong_number_of_type_arguments_element"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The method 'sm' is declared with 1 type parameters, but 2 type arguments are given."
    );
    // The group span `<int, String>`.
    assert_eq!(report.diagnostics[0].range.offset, 110);
    assert_eq!(report.diagnostics[0].range.length, 13);
}

/// An explicit extension application with an invocation
/// (`E<int, String>(1).g`) reports `_extension` at the group (r2 corpus
/// `wrong_number_of_type_arguments_extension`).
///
/// Hand-trace: `E` followed by `<...>(` is a send-head group, so
/// `end_type_arguments`' creation branch classifies the head — `E`'s
/// `Container{Extension}` row, once the stubs keep the group's type
/// arguments from stacking `TypeUse` rows over it — dispatching the
/// extension face (declared 1 against the given 2). The trailing `.g`
/// reads the extension's own member — silent.
#[test]
fn extension_application_arity_reports_extension_code() {
    let report = report(
        "class int {} class String {} void print(x) {} extension E<T> on int { int get g => 1; } void f() { print(E<int, String>(1).g); }",
    );
    assert_eq!(
        code_names(&report),
        ["wrong_number_of_type_arguments_extension"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The extension 'E' is declared with 1 type parameters, but 2 type arguments were given."
    );
    // The group span `<int, String>`.
    assert_eq!(report.diagnostics[0].range.offset, 106);
    assert_eq!(report.diagnostics[0].range.length, 13);
}

/// A static-method property-access tear-off (`C.sm<int, String>`)
/// reports `_function` leading with the uninstantiated signature
/// (r2 corpus `wrong_number_of_type_arguments_function`).
///
/// Hand-trace: the merged `Member{recv: Container{C}}` row defers the
/// group; no `(` follows, so the application event consumes it
/// (`consume_type_argument_application`'s `Member` arm) and the method
/// face renders its captured signature `int Function<T>()`.
#[test]
fn static_tearoff_arity_reports_function_code() {
    let report = report(
        "class int {} class String {} void print(x) {} class C<T> { static int sm<T>() => 0; } void main() { var g = C.sm<int, String>; print(g); }",
    );
    assert_eq!(
        code_names(&report),
        ["wrong_number_of_type_arguments_function"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The type of this function is 'int Function<T>()', which has 1 type parameters, but 2 type arguments were given."
    );
    // The group span `<int, String>`.
    assert_eq!(report.diagnostics[0].range.offset, 112);
    assert_eq!(report.diagnostics[0].range.length, 13);
}

/// An instance-method property-access tear-off (`C().m<int, String>`)
/// reports `_function` too — the `_method` variant stays unreachable
/// (goal #125 drift, r2 corpus `wrong_number_of_type_arguments_method`).
///
/// Hand-trace: `C()`'s callee is Member-filtered out of `pending_send`
/// (`begin_arguments`' `ctor_pos` gate), so the container row survives the
/// send unf folded and the `.m` merge classifies it
/// (`Receiver::Container`) — the group defers, the application event
/// renders `m`'s captured signature, identical to the static tear-off's
/// report.
#[test]
fn method_tearoff_arity_reports_function_code() {
    let report = report(
        "class int {} class String {} void print(x) {} class C { int m<T>() => 0; } void main() { var h = C().m<int, String>; print(h); }",
    );
    assert_eq!(
        code_names(&report),
        ["wrong_number_of_type_arguments_function"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The type of this function is 'int Function<T>()', which has 1 type parameters, but 2 type arguments were given."
    );
    // The group span `<int, String>`.
    assert_eq!(report.diagnostics[0].range.offset, 102);
    assert_eq!(report.diagnostics[0].range.length, 13);
}

// —— batch 2 (#121): static-nullability four ——————————————————————————

/// `null` into a non-nullable target reports `invalid_assignment`
/// (`assignment_expression_resolver.dart:795`).
///
/// Hand-trace: `int x` arms the group type and binds `x` =
/// `int` (non-nullable); `x = null` finds the target row before `=`,
/// exactly one right-hand row (`null`, typed `Null`), and
/// `Ty::assignable_from(Null → int)` = `Some(false)` (Left Null,
/// subtype.dart:349) → the code. The `int` class models the core type in
/// the no-core harness.
#[test]
fn null_assignment_reports_invalid_assignment() {
    let report = report("class int {} void f() { int x; x = null; }");
    // `x` is written but never read: the official read-only use rule
    // reports `unused_local_variable` at its name, before the
    // assignment row.
    assert_eq!(
        code_names(&report),
        ["unused_local_variable", "invalid_assignment"]
    );
    assert_eq!(
        report.diagnostics[1].message,
        "A value of type 'Null' can't be assigned to a variable of type 'int'."
    );
    assert_eq!(report.diagnostics[1].severity, Severity::Error);
}

/// A declaration initializer runs the same decision
/// (`variable_declaration_resolver.dart:140`); a nullable target accepts
/// `null` silently.
///
/// Hand-trace: `int x = null` checks the single-row initializer against
/// the group type → `invalid_assignment`; `int? y = null` is Right Top
/// (subtype.dart:263) → silent. The nullable-declaration-plus-
/// initializer shape parses since the #339 declaration probe accepted
/// `Type? name =` (the RED note below the old expectation is retired).
#[test]
fn declaration_initializer_null_reports_invalid_assignment() {
    let report = report("class int {} void f() { int x = null; int? y = null; }");
    // `x` and `y` are never read (the official read-only use rule);
    // only `x`'s null initializer reports the assignment.
    assert_eq!(
        code_names(&report),
        [
            "unused_local_variable",
            "invalid_assignment",
            "unused_local_variable",
        ]
    );
    assert_eq!(
        report.diagnostics[1].message,
        "A value of type 'Null' can't be assigned to a variable of type 'int'."
    );
}

/// A nullable right-hand side into a non-nullable target is decidable
/// from nullability alone (Left Nullable, subtype.dart:389-392).
///
/// Hand-trace: `y`'s row snapshots its declared type `int?`; the check
/// needs no name lattice → `Some(false)`; the display texts are `int?`
/// and `int`.
#[test]
fn nullable_assignment_reports_invalid_assignment() {
    let report = report("class int {} void f() { int? y; int x; x = y; }");
    // `x` is written only (never read — the official read-only use
    // rule); `y` is read by the assignment and stays silent.
    assert_eq!(
        code_names(&report),
        ["unused_local_variable", "invalid_assignment"]
    );
    assert_eq!(
        report.diagnostics[1].message,
        "A value of type 'int?' can't be assigned to a variable of type 'int'."
    );
}

/// `null` (and nullable values) into a known non-nullable parameter
/// reports `argument_type_not_assignable` (helpers.dart:96-142). The
/// message keeps the template's trailing record appendix (empty at M1).
///
/// Hand-trace: `g(null)` — the argument list collects one terminal row
/// (typed `Null`); the callee's declared positional formal is `int`
/// (non-nullable) → `Some(false)`. `g(y)` with `int? y` reads `y`'s
/// snapshot (`int?`).
#[test]
fn null_argument_reports_argument_type_not_assignable() {
    let report = report("class int {} void g(int p) {} void f() { int? y; g(null); g(y); }");
    assert_eq!(
        code_names(&report),
        [
            "argument_type_not_assignable",
            "argument_type_not_assignable"
        ]
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The argument type 'Null' can't be assigned to the parameter type 'int'. "
    );
    assert_eq!(
        report.diagnostics[1].message,
        "The argument type 'int?' can't be assigned to the parameter type 'int'. "
    );
}

/// The member invocation columns check positional arguments through the
/// unmerged placeholder (`argument_type_not_assignable`, the official
/// `checkForArgumentTypeNotAssignableForArgument` — the argument's
/// `correspondingParameter` is resolution-filled for every resolved
/// invocation kind, instance, static, creation-folded, cascade and
/// call-result alike). The send closes before the dot-access event
/// merges `receiver.name`, so the callee is the qualified placeholder
/// with the receiver row directly below (`check_invocation_arity`'s
/// shape); one receiver classification per form — local read `b.m`,
/// static column `A.s`,
/// folded creation `C().m`, cascade section `b..m`, call result
/// `g().m` (the result's static type is `g`'s declared return type).
#[test]
fn member_columns_report_unassignable_positional_arguments() {
    let report = report(
        "class int {} class B { void m(int x) {} } \
             class A { static void s(int x) {} } \
             class C { void m(int x) {} } \
             B g() => B(); \
             void f() { \
               B b = B(); \
               b.m(null); A.s(null); C().m(null); b..m(null); g().m(null); \
             }",
    );
    assert_eq!(
        code_names(&report),
        ["argument_type_not_assignable"; 5],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The argument type 'Null' can't be assigned to the parameter type 'int'. "
    );
}

/// The same five columns stay silent when every positional argument
/// assigns (the under-report direction keeps the matching face quiet,
/// not the placeholder gate).
#[test]
fn member_columns_stay_silent_on_assignable_positional_arguments() {
    let report = report(
        "class int {} class B { void m(int x) {} } \
             class A { static void s(int x) {} } \
             class C { void m(int x) {} } \
             B g() => B(); \
             void f() { \
               B b = B(); \
               b.m(1); A.s(1); C().m(1); b..m(1); g().m(1); \
             }",
    );
    assert_eq!(
        code_names(&report),
        [] as [&str; 0],
        "{:?}",
        code_names(&report)
    );
}

/// The placeholder resolution feeds the full formal table, so a named
/// argument mixed into the same column checks against its own formal
/// (named by spelling, positional by index — `b.m(null, y: null)`
/// reports both slots: `Null` into the non-nullable positional `int`
/// and into the non-nullable named `C`).
#[test]
fn member_columns_check_mixed_positional_and_named_arguments() {
    let report = report(
        "class int {} class C {} \
             class B { void m(int x, {C y}) {} } \
             void f() { B b = B(); b.m(null, y: null); }",
    );
    // The optional named `{C y}` carries no default against a block
    // body — `defaultValuesAreExpected` answers true for the method
    // face (error_verifier.dart:8327-8372), and the declaration row
    // precedes the call's two argument rows.
    assert_eq!(
        code_names(&report),
        [
            "missing_default_value_for_parameter",
            "argument_type_not_assignable",
            "argument_type_not_assignable",
        ],
        "{:?}",
        code_names(&report)
    );
}

/// `return null` in a non-nullable function reports
/// `return_of_invalid_type` with the per-host message
/// (`rtv.dart:209-249` sync matrix).
///
/// Hand-trace: `f`'s return type latches at the name identifier (the
/// return type's `handle_type` precedes it); `return null`'s single row
/// fails Left Null → the function-host message. `m`'s method shape picks
/// the method-host word.
#[test]
fn null_return_reports_return_of_invalid_type() {
    let report =
        report("class int {} class C { int m() { return null; } } int f() { return null; }");
    assert_eq!(
        code_names(&report),
        ["return_of_invalid_type", "return_of_invalid_type"]
    );
    assert_eq!(
        report.diagnostics[0].message,
        "A value of type 'Null' can't be returned from the method 'm' because it has a return type of 'int'."
    );
    assert_eq!(
        report.diagnostics[1].message,
        "A value of type 'Null' can't be returned from the function 'f' because it has a return type of 'int'."
    );
}

/// A function literal's `return`/`=>` body checks against the literal's
/// own context-inferred return, never the enclosing method's declared
/// return: `onTimeout: () => null` inside `firstDepleted`'s `.timeout`
/// call stays silent (the `Null` operand is legal for the literal's
/// `FutureOr<InventoryEvent?>` face — the enclosing `Future<InventoryEvent?>`
/// is not the literal's return type). Before the boundary fix the inner
/// `=>` leaked to the method's frame and over-fired.
#[test]
fn closure_return_stays_inside_the_literal_frame() {
    let report = report(
        "class int {} class Future<T> {} class InventoryEvent {} \
             class C { \
               Future<InventoryEvent?> firstDepleted() { \
                 return fetch().timeout(t, onTimeout: () => null); \
               } \
               Future<InventoryEvent?> fetch() => first; \
             }",
    );
    assert!(
        !code_names(&report).contains(&"return_of_invalid_type"),
        "{:?}",
        code_names(&report)
    );
}

/// The void-host arm of the return matrix: `return <non-void>` in a
/// `void` function reports; `return null` in a void function is
/// oracle-legal.
///
/// Hand-trace: host `void` accepts `void`/`dynamic`/`Null` — `return x`
/// with `x: int` is none of them → report; `return null` matches the
/// `Null` accept set → silent.
#[test]
fn void_return_matrix() {
    let report = report("class int {} void f(int x) { return x; } void g() { return null; }");
    assert_eq!(code_names(&report), ["return_of_invalid_type"]);
    assert_eq!(
        report.diagnostics[0].message,
        "A value of type 'int' can't be returned from the function 'f' because it has a return type of 'void'."
    );
}

/// The corpus faces of a `void` host's string return with the `;`
/// missing (#1823; corpus 8e7bfa84/e0f42062/f59c79c8 — `string2/3/4.dart`
/// regression shapes): the recovery still completes the return
/// statement, so the expression face reports `return_of_invalid_type`
/// anchored at the whole literal token, exactly where the oracle pins it.
///
/// Focused asserts (the mixed-tier row set also carries the parser's
/// `expected_token` recovery rows, owned by the parser pins).
fn assert_void_string_return(source: &str, prefix: &str, literal_len: u32, host: &str) {
    let report = report_with_scanner_residue(source);
    let d = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "return_of_invalid_type")
        .unwrap_or_else(|| panic!("no return row: {:?}", code_names(&report)));
    assert_eq!(
        &*d.message,
        format!(
            "A value of type 'String' can't be returned from the function '{host}' \
                 because it has a return type of 'void'."
        )
    );
    assert_eq!(d.range.offset, u32::try_from(prefix.len()).unwrap());
    assert_eq!(d.range.length, literal_len);
}

#[test]
fn missing_semicolon_string_return_reports() {
    // Plain triple-quoted literal (8e7bfa84 `bar`).
    self::assert_void_string_return(
        "void bar() {\n  return \"\"\"æbler\"\"\"\n}\n",
        "void bar() {\n  return ",
        11,
        "bar",
    );
    // Raw literal (8e7bfa84 `baz`).
    self::assert_void_string_return(
        "void baz() {\n  return r\"\"\"hello\"\"\"\n}\n",
        "void baz() {\n  return ",
        12,
        "baz",
    );
    // Multiline literal spanning newlines (8e7bfa84 `qux`).
    self::assert_void_string_return(
        "void qux() {\n  return \"\"\"\næbler\n\"\"\"\n}\n",
        "void qux() {\n  return ",
        13,
        "qux",
    );
}

#[test]
fn semicolon_terminated_raw_multiline_return_reports() {
    // e0f42062 `foo`: the healthy control — the semicolon present, the
    // raw multiline literal returns from a void host.
    self::assert_void_string_return(
        "void foo() {\n  return r\"\"\"\næbler\næbler\"\"\";\n}\n",
        "void foo() {\n  return ",
        19,
        "foo",
    );
}

#[test]
fn eof_truncated_string_return_reports() {
    // The literal runs to EOF (8e7bfa84 `quux`, e0f42062 `bar`,
    // f59c79c8 `foo`): the scanner's recovered single token is the
    // whole literal, and the return check still sees it.
    self::assert_void_string_return(
        "void quux() {\n  // ends in eof.\n  return \"\"\"æbler\\",
        "void quux() {\n  // ends in eof.\n  return ",
        9,
        "quux",
    );
    self::assert_void_string_return(
        "void bar() {\n  return r\"\"\"\næbler",
        "void bar() {\n  return ",
        10,
        "bar",
    );
    self::assert_void_string_return(
        "void foo() {\n  return \"\"\"\næbler",
        "void foo() {\n  return ",
        9,
        "foo",
    );
}

#[test]
fn interpolated_string_return_reports_with_its_read_as_residue() {
    // 8e7bfa84 `foo`: `$bar`'s read row joins the return window below
    // the literal's completion row; the oracle's expression is the
    // `StringInterpolation` node, so the read is residue and the
    // literal's String answers the void host (#1823).
    let report = report("void foo(dynamic bar) {\n  return \"\"\"æbler$bar\"\"\"\n}\n");
    let d = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "return_of_invalid_type")
        .unwrap_or_else(|| panic!("no return row: {:?}", code_names(&report)));
    assert_eq!(
        d.message,
        "A value of type 'String' can't be returned from the function 'foo' because it has a return type of 'void'."
    );
    assert_eq!(
        d.range.offset,
        u32::try_from("void foo(dynamic bar) {\n  return ".len()).unwrap()
    );
    // The completion row spans the whole literal: the parser hands the
    // last consumed String token (the closing quote, upstream
    // `endLiteralString`'s endToken contract) and the row widens to the
    // oracle's `StringInterpolation` extent (#1916).
    assert_eq!(d.range.length, 15);
}

/// The whole-file corpus faces (#1823; corpus 8e7bfa84/e0f42062/f59c79c8
/// — `string2/3/4.dart` verbatim, no trailing newline): every function
/// shares one engine pass, so the exact row counts pin the cross-function
/// interaction the isolated shapes cannot see (memoized frames, per-frame
/// return faces, eof recovery swallowing the file tail). The parser's
/// `expected_token` faces ride along owned by the parser pins.
#[test]
fn string2_whole_file_reports_all_five_returns() {
    let report = report_with_scanner_residue(
        "void foo(dynamic bar) {\n  return \"\"\"æbler$bar\"\"\"\n}\
             \n\
             void bar() {\n  return \"\"\"æbler\"\"\"\n}\
             \n\
             void baz() {\n  return r\"\"\"hello\"\"\"\n}\
             \n\
             void qux() {\n  return \"\"\"\næbler\n\"\"\"\n}\
             \n\
             void quux() {\n  // ends in eof.\n  return \"\"\"æbler\\",
    );
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "return_of_invalid_type")
        .collect();
    assert_eq!(rows.len(), 5, "{:?}", code_names(&report));
    for (d, host) in rows.iter().zip(["foo", "bar", "baz", "qux", "quux"]) {
        assert_eq!(
            &*d.message,
            format!(
                "A value of type 'String' can't be returned from the function '{host}' \
                     because it has a return type of 'void'."
            ),
            "{:?}",
            code_names(&report)
        );
    }
}

#[test]
fn string4_whole_file_reports_both_returns() {
    // e0f42062: the semicolon-terminated `foo` must survive the
    // eof-truncated `bar` tail (the unterminated literal must not poison
    // the earlier frame's rows).
    let report = report_with_scanner_residue(
        "void foo() {\n  return r\"\"\"\næbler\næbler\"\"\";\n}\
             \n\
             void bar() {\n  return r\"\"\"\næbler",
    );
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "return_of_invalid_type")
        .collect();
    assert_eq!(rows.len(), 2, "{:?}", code_names(&report));
    for (d, host) in rows.iter().zip(["foo", "bar"]) {
        assert_eq!(
            &*d.message,
            format!(
                "A value of type 'String' can't be returned from the function '{host}' \
                     because it has a return type of 'void'."
            ),
            "{:?}",
            code_names(&report)
        );
    }
}

#[test]
fn string3_whole_file_reports_its_single_return() {
    // f59c79c8: the whole file is one eof-truncated plain multiline
    // return.
    let report = report_with_scanner_residue("void foo() {\n  return \"\"\"\næbler");
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "return_of_invalid_type")
        .collect();
    assert_eq!(rows.len(), 1, "{:?}", code_names(&report));
    assert_eq!(
        &*rows[0].message,
        "A value of type 'String' can't be returned from the function 'foo' \
             because it has a return type of 'void'.",
        "{:?}",
        code_names(&report)
    );
}

/// The illegal-character merge faces (#1823; corpus 4caf0953,
/// dbf4285a): the scanner folds the flagged characters into the
/// adjacent identifier token, and the merged name reports
/// `undefined_identifier` exactly as the oracle's single row does.
#[test]
fn merged_illegal_char_identifier_reports_undefined() {
    let merged = report_with_scanner_residue("a = ShcheгlovЩx;\n");
    let d = merged
        .diagnostics
        .iter()
        .find(|d| d.code.name == "undefined_identifier")
        .unwrap_or_else(|| panic!("no undefined row: {:?}", code_names(&merged)));
    assert_eq!(d.message, "Undefined name 'ShcheгlovЩx'.");
    assert_eq!(d.range.offset, u32::try_from("a = ".len()).unwrap());
    assert_eq!(d.range.length, 11);

    let single = report_with_scanner_residue("a = Щ;\n");
    let d = single
        .diagnostics
        .iter()
        .find(|d| d.code.name == "undefined_identifier")
        .unwrap_or_else(|| panic!("no undefined row: {:?}", code_names(&single)));
    assert_eq!(d.message, "Undefined name 'Щ'.");
    assert_eq!(d.range.offset, u32::try_from("a = ".len()).unwrap());
    assert_eq!(d.range.length, 1);
}

/// The astral identifier face (#1823; corpus 8f05c31e): each UTF-16
/// surrogate pins its own `illegal_character` row, and the merged
/// two-unit identifier reports `undefined_identifier` over both.
/// `f`'s `int?` formal carries the harness's unresolved-`int` residue —
/// a harness artifact, not the face under pin.
#[test]
fn astral_surrogate_identifier_reports_undefined() {
    let report = report_with_scanner_residue(
        "// Targets illegal_character: an astral character in code position is scanned\n\
             // as two unpaired surrogates — illegal_character emitted once per surrogate.\n\
             void f([int? a]) {}\nvoid main() {\n  f(😀);\n}\n",
    );
    let d = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "undefined_identifier")
        .unwrap_or_else(|| panic!("no undefined row: {:?}", code_names(&report)));
    assert_eq!(d.message, "Undefined name '😀'.");
    // SourceRange units are UTF-16 code units (the oracle's column
    // semantics), so the prefix counts units — the comment's em dash is
    // one unit, three bytes.
    assert_eq!(
        d.range.offset,
        u32::try_from(
            "// Targets illegal_character: an astral character in code position is scanned\n\
                 // as two unpaired surrogates — illegal_character emitted once per surrogate.\n\
                 void f([int? a]) {}\nvoid main() {\n  f("
                .encode_utf16()
                .count()
        )
        .unwrap()
    );
    assert_eq!(d.range.length, 2);
}

/// The local-function return host: the declaration's name latches the
/// frame's return face like every executable's.
///
/// Hand-trace: `f() { return 42; }` arms no annotation before the name,
/// so the latch falls to `unknown` and the matrix stays silent — the
/// oracle's local function returns `dynamic` (the same face covers the
/// recovery shape where a dangling member access consumed the written
/// type, `issue_49477_prime.dart`). `int f() { return 's'; }` latches
/// the written `int`, and the failing row names the LOCAL function —
/// never the enclosing executable the frame walk used to fall through
/// to (upstream Dart SDK source,
/// pkg/analyzer/lib/src/dart/resolver/return_type_verifier.dart).
#[test]
fn local_function_return_host_latches_at_its_name() {
    let dynamic_host = report("void main() { f() { return 42; } f(); }");
    assert!(
        dynamic_host.diagnostics.is_empty(),
        "{:?}",
        code_names(&dynamic_host)
    );
    let written_host = report("class int {} void main() { int f() { return 's'; } f(); }");
    assert_eq!(code_names(&written_host), ["return_of_invalid_type"]);
    assert_eq!(
        written_host.diagnostics[0].message,
        "A value of type 'String' can't be returned from the function 'f' because it has a return type of 'int'."
    );
}

/// The recovered accessor-in-function shape (`getter_in_function`
/// recovery): `int get` parses as a local variable missing `;`, and
/// `g => 1;` rides the local-function missing-formals recovery — the
/// recovered frame latches `unknown` at its name (no annotation armed
/// past the closed variable group), so the arrow body never feeds the
/// enclosing `void` host's return matrix. The oracle under-reports
/// local-function bodies for this shape (upstream Dart SDK source,
/// pkg/analyzer/lib/src/dart/resolver/return_type_verifier.dart — the
/// enclosing executable's check walks only its own body, and the
/// recovered getter's `FunctionDeclarationImpl` is not part of it).
#[test]
fn recovered_getter_in_function_keeps_the_enclosing_matrix_silent() {
    let getter = report("void getterInFunction() {\n  int get g => 1;\n}\n");
    assert!(
        !getter
            .diagnostics
            .iter()
            .any(|d| d.code.name == "return_of_invalid_type"),
        "{:?}",
        code_names(&getter)
    );
    let setter = report("void setterInFunction() {\n  set s(int v) {}\n}\n");
    assert!(
        !setter
            .diagnostics
            .iter()
            .any(|d| d.code.name == "return_of_invalid_type"),
        "{:?}",
        code_names(&setter)
    );
}

/// The no-type `late` declaration family (upstream Dart SDK source,
/// error_recovery/best_practices_no_late.dart corpus face — SDK issues
/// 43811-43813): the declared name binds UNKNOWN and joins its frame
/// (or the library/container element table), so later references
/// resolve — no undefined row. Each lane carries exactly its parser's
/// oracle-faithful `missing_const_final_var_or_type` row at the name
/// (upstream `handleRecoverableError(MissingConstFinalVarOrType, x, x)`
/// on the same three shapes — the intertwined expect for
/// `late_without_var_etc.dart`). All three declaration lanes pin.
#[test]
fn no_type_late_declarations_declare_their_names() {
    let local = report("void main() {\n  late x;\n  x;\n}\n");
    // The read `x;` is a definitely-unassigned late local: the flow
    // fact's row rides beside the declaration's own parser row (the
    // oracle's definitelyUnassignedLateLocalVariable at the read).
    assert_eq!(
        code_names(&local),
        [
            "missing_const_final_var_or_type",
            "definitely_unassigned_late_local_variable"
        ],
        "{:?}",
        code_names(&local)
    );
    let top_level = report("late y;\nvoid main() {\n  y;\n}\n");
    assert_eq!(
        code_names(&top_level),
        ["missing_const_final_var_or_type"],
        "{:?}",
        code_names(&top_level)
    );
    let field = report("class Foo {\n  late z;\n  void foo() {\n    z;\n  }\n}\n");
    assert_eq!(
        code_names(&field),
        ["missing_const_final_var_or_type"],
        "{:?}",
        code_names(&field)
    );
}

/// The `late;` statement face: `late` is a built-in keyword —
/// `isIdentifier` true, so `ensureIdentifier` returns it as a real
/// `SimpleIdentifier` and the oracle reports `undefined_identifier`
/// (probe: `late;` reports `Undefined name 'late'`). Only the
/// declaration-start heads (`var`/`final`/`const`/`dynamic`/`void`) stay
/// silent — upstream parses them as declarations, never identifiers.
#[test]
fn late_keyword_statement_reports_undefined_identifier() {
    let r = report("void main() {\n  late;\n}\n");
    assert_eq!(
        code_names(&r),
        ["undefined_identifier"],
        "{:?}",
        code_names(&r)
    );
}

/// The declaration-start gate: `var`/`final`/`const` parse as
/// declaration heads upstream, so their bare rows emit no
/// `undefined_identifier` (their recovery rows are syntactic, out of
/// this test's filter).
#[test]
fn declaration_start_keywords_stay_silent() {
    let r = report("void main() {\n  var;\n  final;\n  const;\n}\n");
    let names = code_names(&r);
    assert!(
        !names.iter().any(|n| n.starts_with("undefined_")),
        "{names:?}"
    );
}

/// The synthetic-identifier reference skip: a recovery-inserted
/// identifier carries a zero-length span (`x.;` mints the selector), and
/// the oracle's resolver skips synthetic tokens — no undefined member
/// face reports for it. The parse tier carries the oracle's own single
/// row (`ExpressionIdentifierContext.ensureIdentifier` reports at the
/// `;`, upstream Dart SDK source,
/// pkg/_fe_analyzer_shared/lib/src/parser/identifier_context_impl.dart
/// :455-466; the dot arm adds no second anchor) — the pin is that the
/// merged report holds exactly that row and nothing else.
#[test]
fn synthetic_selector_identifier_stays_undefined_silent() {
    let r = report("class C {}\nvoid f(C x) {\n  x.;\n}\n");
    assert_eq!(
        code_names(&r),
        ["missing_identifier"],
        "{:?}",
        code_names(&r)
    );
}

/// The empty-name selector fallback: neither the multi-declarator
/// `late` locals' mis-resolution from a closure nor the prefixed
/// `identical` call may synthesize a getter row with an empty selector
/// name — the oracle's selector faces always carry the written name
/// (the empty-name shape is a synthetic token, which the resolution
/// pass skips).
#[test]
fn empty_name_selector_rows_stay_silent() {
    let empty_name_row = |r: &FileReport| {
        r.diagnostics
            .iter()
            .any(|d| d.code.name.starts_with("undefined_") && d.message.contains("''"))
    };
    let late_locals =
        report("void f() {\n  late int v1, v2;\n  void g() {\n    v2;\n  }\n  g();\n}\n");
    assert!(
        !empty_name_row(&late_locals),
        "{:?}",
        code_names(&late_locals)
    );
    let prefixed =
        report("import 'dart:core' as core;\nvoid f() {\n  core.identical(null, null);\n}\n");
    assert!(!empty_name_row(&prefixed), "{:?}", code_names(&prefixed));
}

/// The invalidated configuration literal: an interpolated `==` value
/// in an `if (...)` configuration is the parser's
/// invalid_literal_in_configuration face, and the configuration is
/// never resolved as an expression (the oracle resolves only the
/// dottedName) — the interpolation's name references stay silent
/// (upstream Dart SDK source, pkg/analyzer/lib/src/dart/analysis/
/// library_analyzer.dart, the Configuration walk). The parser's own
/// face remains in the report; the pin is the name-silence.
#[test]
fn invalidated_configuration_literal_keeps_its_names_silent() {
    let r = report("import 'a.dart' if (x == 'v$w') 'b.dart';\nvoid f() {}\n");
    assert!(
        !r.diagnostics
            .iter()
            .any(|d| d.code.name.starts_with("undefined_")),
        "{:?}",
        code_names(&r)
    );
}

/// The local-prefix face: a prefixed type whose prefix head names a
/// local or parameter element answers the importPrefix gate's
/// fall-through — `prefix_shadowed_by_local_declaration` at the
/// prefix token, the continuation unresolved with no
/// `undefined_class`, and the shadowing local still unused (the gate
/// lookup is a shadow check, not a use; upstream Dart SDK source,
/// pkg/analyzer/lib/src/dart/resolver/named_type_resolver.dart:86-120,
/// the oracle probe l1/l2: the face at `p` plus `unused_local_variable`
/// for `p` and `q`).
#[test]
fn local_prefix_head_reports_the_shadowed_prefix() {
    let local = report("void f() {\n  var p = 1;\n  p.T q;\n}\n");
    let shadowed: Vec<_> = local
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "prefix_shadowed_by_local_declaration")
        .collect();
    assert_eq!(shadowed.len(), 1, "{:?}", code_names(&local));
    assert_eq!(
        shadowed[0].message,
        "The prefix 'p' can't be used here because it's shadowed by a local declaration."
    );
    let p_at = offset_of("void f() {\n  var p = 1;\n  p.T q;\n}\n", "p.T");
    assert_eq!(shadowed[0].range.offset, p_at);
    assert_eq!(shadowed[0].range.length, 1);
    assert!(
        !local
            .diagnostics
            .iter()
            .any(|d| d.code.name == "undefined_class"),
        "{:?}",
        code_names(&local)
    );
    let unused = local
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_local_variable")
        .count();
    assert_eq!(unused, 2, "{:?}", code_names(&local));
    let parameter = report("class int {}\nvoid f(int p) {\n  p.T q;\n}\n");
    let shadowed: Vec<_> = parameter
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "prefix_shadowed_by_local_declaration")
        .collect();
    assert_eq!(shadowed.len(), 1, "{:?}", code_names(&parameter));
    assert_eq!(
        shadowed[0].range.offset,
        offset_of("class int {}\nvoid f(int p) {\n  p.T q;\n}\n", "p.T")
    );
    assert!(
        !parameter
            .diagnostics
            .iter()
            .any(|d| d.code.name == "undefined_class"),
        "{:?}",
        code_names(&parameter)
    );
}

/// The recovered extends-after-implements tail (`implements_before_
/// extends` ordering recovery): the parser keeps the tail clause's
/// types, and the unresolvable tail head codes `extends_non_class` —
/// the `ExtendsClause` arm of the oracle's clause switch — never the
/// bare `undefined_class`, while the implements head keeps its own
/// clause face (upstream Dart SDK source,
/// pkg/analyzer/lib/src/dart/resolver/scope_analyzer.dart
/// `_resolveType`; the oracle pin: implements_non_class at `B`,
/// extends_non_class at `C`).
#[test]
fn recovered_extends_tail_codes_extends_non_class() {
    let r = report("class A implements B extends C {}\n");
    let codes = code_names(&r);
    assert!(
        codes.contains(&"implements_non_class") && codes.contains(&"extends_non_class"),
        "{:?}",
        codes
    );
    assert!(!codes.contains(&"undefined_class"), "{:?}", codes);
    let ext = r
        .diagnostics
        .iter()
        .find(|d| d.code.name == "extends_non_class")
        .expect("extends face");
    assert_eq!(ext.range.offset, 29, "{:?}", ext.range);
    assert_eq!(ext.message, "Classes can only extend other classes.");
    assert_eq!(
        ext.correction.as_deref(),
        Some("Try specifying a different superclass, or removing the extends clause.")
    );
}

/// The zero-case switch-expression recovery: the arm fold over zero
/// arms is the oracle's InvalidType, so the enclosing `=>` return
/// check asks no question of the expression — and the poisoned value
/// row stops the scrutinee's own row from being the body's single row
/// (upstream Dart SDK source, pkg/analyzer/lib/src/dart/ast/ast.dart
/// `SwitchExpressionImpl.staticType`).
#[test]
fn zero_case_switch_expression_keeps_the_return_matrix_silent() {
    let r = report("int f(int x) => switch (x) {}\n");
    assert!(
        !r.diagnostics
            .iter()
            .any(|d| d.code.name == "return_of_invalid_type"),
        "{:?}",
        code_names(&r)
    );
}

/// The recovered `is` operand: `return is(x-1);` parses as
/// `<synthetic> is (x-1)` — the operand-splice recovery — and the
/// expression still types `bool`, so the return matrix reports the
/// oracle's row naming the `is` host
/// (keyword_named_top_level_methods_2.dart:183, upstream Dart SDK
/// source, pkg/analyzer/lib/src/dart/ast/ast.dart
/// `IsExpressionImpl.staticType`).
///
/// Hand-trace: the synthetic send head fired no identifier event (no
/// operand row), the recovered record type left one suppressed
/// `TypeUse` row, and the operator's event drops it under a `bool` row
/// spanning the `is` keyword — the single row the return check reads.
#[test]
fn recovered_is_expression_types_bool() {
    let report = report("int is(int x) {\n  if (x == 0) return 42;\n  return is(x-1) + 1;\n}\n");
    let returns: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "return_of_invalid_type")
        .collect();
    assert_eq!(returns.len(), 1, "{:?}", code_names(&report));
    assert_eq!(
        returns[0].message,
        "A value of type 'bool' can't be returned from the function 'is' because it has a return type of 'int'."
    );
}

/// The record literal's operator face: the single-positional literal
/// folds its field window into a record-typed result row, so an applied
/// operator resolves against a type that declares none — the oracle's
/// `undefined_operator` with the record display (`(int,)`),
/// keyword_named_class_methods_2.dart:49 (`const(x-1) + 1`, the
/// const-keyword recovery shape) — and the enclosing return stays
/// silent (the failed invocation types `dynamic`).
///
/// Hand-trace: `(x,)`'s window holds the one int row the field read
/// left (`const(x-1)`'s holds the int read and the int literal), the
/// record row replaces the opaque value row, and the `+` mark lands on
/// it; pass 2 spells the miss at the operator's own span.
#[test]
fn record_literal_operator_is_undefined() {
    let well_formed = report("class int {} int f(int x) { return (x,) + 1; }");
    let ops: Vec<_> = well_formed
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_operator")
        .collect();
    assert_eq!(ops.len(), 1, "{:?}", code_names(&well_formed));
    assert_eq!(
        ops[0].message,
        "The operator '+' isn't defined for the type '(int,)'."
    );
    assert!(
        well_formed
            .diagnostics
            .iter()
            .all(|d| d.code.name != "return_of_invalid_type")
    );

    // The oracle's `const(x-1)` recovery rides the same record path:
    // a const-prefixed group is ALWAYS a record literal on both sides —
    // the parser mirrors upstream `wasRecord = constKeyword != null`
    // (upstream Dart SDK source, pkg/_fe_analyzer_shared/lib/src/parser/
    // parser_impl.dart parseParenthesizedExpressionOrRecordLiteral: the
    // const keyword forces the record reading, the comma only upgrades
    // the plain-paren reading), reporting the one-positional-no-trailing-
    // comma face and emitting `end_record_literal` with the single field;
    // the fold then types the window `(int,)` and the `+` miss spells at
    // the operator. The `operator -` stub keeps the field's binary
    // operator oracle-clean (dart:core `int` declares it) so only the
    // record's `+` face remains.
    let recovered = report(
        "class int { int operator -(int o) => 0; } \
             int f(int x) { return const(x-1) + 1; }",
    );
    let ops: Vec<_> = recovered
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_operator")
        .collect();
    assert_eq!(ops.len(), 1, "{:?}", code_names(&recovered));
    assert_eq!(
        ops[0].message,
        "The operator '+' isn't defined for the type '(int,)'."
    );
}

/// `?.` on a strictly non-nullable receiver reports the
/// `invalid_null_aware_operator` `STATIC_WARNING`; a nullable receiver
/// stays silent (`type_system.dart:1921-` `isStrictlyNonNullable`).
///
/// Hand-trace: `c`'s declared type is `C` (no `?`) → strictly
/// non-nullable → the warning at the merge; `c2`'s type is `C?` →
/// suppress. `m` is declared, so the member walk resolves both accesses.
#[test]
fn null_aware_on_non_nullable_reports_static_warning() {
    let report = report("class C { void m() {} } void f(C c, C? c2) { c?.m(); c2?.m(); }");
    assert_eq!(code_names(&report), ["invalid_null_aware_operator"]);
    let d = &report.diagnostics[0];
    assert_eq!(
        d.message,
        "The receiver can't be null, so the null-aware operator '?.' is unnecessary."
    );
    assert_eq!(d.severity, Severity::Warning);
    assert_eq!(
        d.code.dtype,
        dartq_diagnostics::DiagnosticType::StaticWarning
    );
}

/// Declaration-arity violations: the operator count table folds from
/// the collector's log ahead of the row diagnostics, in capture order
/// (`error_verifier.dart:5688-5740`); the setter shape rule rides the
/// parse tier's own row (`_ensureSetterFormalParameter` at the setter
/// name, upstream Dart SDK source,
/// pkg/analyzer/lib/src/fasta/ast_builder.dart:6464-6495 — the merged
/// report keeps one row, the collector's log entry is retired).
///
/// Hand-trace: `operator +(a, b)` → expected 1, found 2; `set s(a, b)` →
/// not one required-positional → the syntactic-tier setter text; the
/// correct `operator [](i)` and `set ok(v)` stay silent. The `int`
/// annotations model `dart:core` (the harness has no core library —
/// `report()`'s contract — so core names join as concrete declarations,
/// as in `null_assignment`).
#[test]
fn declaration_arity_folds_operator_and_setter() {
    let report = report(
        "class int {} class C { int operator +(int a, int b) => 1; set s(int x, int y) {} int operator [](int i) => 1; set ok(int v) {} }",
    );
    assert_eq!(
        code_names(&report),
        [
            "wrong_number_of_parameters_for_operator",
            "wrong_number_of_parameters_for_setter",
        ]
    );
    assert_eq!(
        report.diagnostics[0].message,
        "Operator '+' should declare exactly 1 parameters, but 2 found."
    );
    assert_eq!(
        report.diagnostics[1].message,
        "Setters must declare exactly one required positional parameter."
    );
    assert_eq!(report.diagnostics[1].severity, Severity::Error);
}

/// The minus operator keeps its own template under the same analyzerCode
/// (0 or 1 parameters — unary and binary minus coexist).
///
/// Hand-trace: `operator -(a, b)` declares 2 parameters; the table's
/// fall-through arm (`error_verifier.dart:5731-5740`) fires the minus
/// variant with the found count only. `int` models `dart:core` (the
/// `report()` harness contract).
#[test]
fn minus_operator_arity_uses_the_minus_template() {
    let report = report("class int {} class C { C operator -(int a, int b) => C(); }");
    assert_eq!(
        code_names(&report),
        ["wrong_number_of_parameters_for_operator"]
    );
    assert_eq!(
        report.diagnostics[0].message,
        "Operator '-' should declare 0 or 1 parameter, but 2 found."
    );
}

/// The arity checks suppress on unresolved heads: a second code at the
/// type-argument group would contaminate the diff (rule 0c — invalid
/// types never cascade).
///
/// Hand-trace: `Missing<A>()` parses as a bare invocation, not a
/// creation — no `new`/`const` prefix, so the official parser runs it
/// through the send path and the resolver reports the identifier miss
/// (`undefined_function`, the static-context invocation miss). The
/// group's arity check finds no resolvable declaration and defers past
/// it → exactly one code.
#[test]
fn unresolved_creation_head_suppresses_arity() {
    let report = report("class A {} void f() { Missing<A>(); }");
    assert_eq!(code_names(&report), ["undefined_function"]);
}

// —— batch 2 (#197): the member family ————————————————————————————

/// Instance member misses report `undefined_method` /
/// `undefined_getter` / `undefined_setter` by access intent
/// (`InstanceLookupDomain`); a write survives on any same-named
/// declaration (getter-only: the oracle's assignment-to-final family)
/// or on a setter spelling; a setter-only name still reads as a
/// getter miss.
///
/// Hand-trace: the harness has no core library, so `int` — undeclared
/// here — is a missing type reference at each member declaration
/// (`undefined_class` ×2, the oracle's annotation miss); `c.h` walks
/// `C` (no `h`, no superclass) → clean miss → `undefined_getter`;
/// `c.g` finds the getter (silent); `c.s = 1` misses `s` and `s=` →
/// `undefined_setter`; `c.g = 1` finds the getter `g` (write survives,
/// unfrozen assignment-to-final); `c.p` on the setter-only `p=` misses
/// the read → `undefined_getter`.
#[test]
fn instance_member_family_reports_by_intent() {
    let report = report(
        "class C { int get g => 1; set p(int v) {} } \
             void f(C c) { c.h; c.s = 1; c.g = 2; c.p; }",
    );
    assert_eq!(
        code_names(&report),
        [
            "undefined_class",
            "undefined_class",
            "undefined_getter",
            "undefined_setter",
            "undefined_getter",
        ],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[2].message,
        "The getter 'h' isn't defined for the type 'C'."
    );
    assert_eq!(
        report.diagnostics[3].message,
        "The setter 's' isn't defined for the type 'C'."
    );
    assert_eq!(
        report.diagnostics[2].range.offset,
        u32::try_from("class C { int get g => 1; set p(int v) {} } void f(C c) { c.".len())
            .unwrap()
    );
}

/// A declared method invoked through an instance receiver reports
/// `undefined_method` (`ReadSyntax.invocation`).
///
/// Hand-trace: `c.m()`'s send marks the member row invoke; the walk on
/// `C` clean-misses `m` → the method code, not the getter code.
#[test]
fn instance_invocation_miss_reports_undefined_method() {
    let report = report("class C {} void f(C c) { c.m(); }");
    assert_eq!(code_names(&report), ["undefined_method"]);
    assert_eq!(
        report.diagnostics[0].message,
        "The method 'm' isn't defined for the type 'C'."
    );
}

/// The static face (`StaticLookupDomain`): interface shapes report the
/// `undefined_method`/`_getter`/`_setter` trio (the type-literal
/// invocation row shares the registry name and message), enum reads
/// report `undefined_enum_constant`, and enum writes report
/// `undefined_setter`.
///
/// Hand-trace: every `X.member` merges to a `Container` receiver whose
/// face is the declaration's own member map; `E.x` reads fall to the
/// enum-constant text, `E.z = 1` writes to the uniform setter arm.
#[test]
fn static_face_reports_interface_trio_and_enum_constant() {
    let report = report(
        "class C {} enum E { a } \
             void f() { C.missing(); C.g; C.s = 1; E.a; E.x; E.y(); E.z = 1; }",
    );
    assert_eq!(
        code_names(&report),
        [
            "undefined_method",
            "undefined_getter",
            "undefined_setter",
            "undefined_enum_constant",
            "undefined_method",
            "undefined_setter",
        ],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The method 'missing' isn't defined for the type 'C'."
    );
    assert_eq!(
        report.diagnostics[3].message,
        "There's no constant named 'x' in 'E'."
    );
}

/// The extension face reports its dedicated trio
/// (`StaticLookupDomain._extensionReadFailure` / the setter arm).
///
/// Hand-trace: `X` resolves as an `Extension` container; the member map
/// misses each name → the extension texts with the extension name.
#[test]
fn extension_static_face_reports_extension_trio() {
    let report = report(
        "class int {} extension X on int {} \
             void f() { X.g; X.m(); X.s = 1; }",
    );
    assert_eq!(
        code_names(&report),
        [
            "undefined_extension_getter",
            "undefined_extension_method",
            "undefined_extension_setter",
        ],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The getter 'g' isn't defined for the extension 'X'."
    );
}

/// `super.member` reports `undefined_super_member` with the
/// per-shape message against the enclosing container (`SuperLookupDomain`);
/// members found through the superclass edge stay silent.
///
/// Hand-trace: the harness has no core library, so B's `int`-typed
/// members carry two `undefined_class` rows; `super.g/m/s` resolve in
/// `B` (the superclass edge reaches it) — silent; the `q*` misses walk
/// B cleanly → getter/method/setter texts.
#[test]
fn super_member_family_reports_by_shape() {
    let report = report(
        "class B { int get g => 1; void m() {} set s(int v) {} } \
             class C extends B { void f() { super.g; super.m(); super.s = 1; super.q; super.q2(); super.q3 = 2; } }",
    );
    assert_eq!(
        code_names(&report),
        [
            "undefined_class",
            "undefined_class",
            "undefined_super_member",
            "undefined_super_member",
            "undefined_super_member",
        ],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[2].message,
        "The getter 'q' isn't defined in a superclass of 'C'."
    );
    assert_eq!(
        report.diagnostics[3].message,
        "The method 'q2' isn't defined in a superclass of 'C'."
    );
    assert_eq!(
        report.diagnostics[4].message,
        "The setter 'q3' isn't defined in a superclass of 'C'."
    );
}

/// Prefixed property reads and writes report `undefined_prefixed_name`
/// (`PrefixedLookupDomain`); the resolved name stays silent.
///
/// Hand-trace: `p.real` hits the prefix table (silent); `p.gone` and the
/// `p.gone2 = 1` write miss it and pass the guard → the two-part text.
#[test]
fn prefixed_property_miss_reports_undefined_prefixed_name() {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file_a = files.intern(Path::new("a.dart"));
    let file_m = files.intern(Path::new("main.dart"));
    let source_a = "void real() {}";
    let source_m = "import 'a.dart' as p; void f() { p.real; p.gone; p.gone2 = 1; }";
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let scanned_a = dartq_scanner::scan(
        source_a,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file_a,
    );
    let scanned_m = dartq_scanner::scan(
        source_m,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file_m,
    );
    let elements_a = collect(source_a, &scanned_a.tokens, features, &strings);
    let elements_m = collect(source_m, &scanned_m.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib_a = builder.intern("package:t/a.dart");
    let lib_m = builder.intern("package:t/main.dart");
    let mut index = ElementIndex::default();
    index.insert(file_a, elements_a.clone());
    index.insert(file_m, elements_m.clone());
    for (name, _) in elements_a.declarations() {
        builder.declare(lib_a, name, file_a);
    }
    for (name, decl) in elements_m.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_m, name, file_m);
    }
    builder.import(
        lib_m,
        ImportEdge {
            target: Some(lib_a),
            prefix: Some(strings.intern("p")),
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let report = diagnose(
        source_m,
        &scanned_m.tokens,
        features,
        &strings,
        file_m,
        lib_m,
        &graph,
        &namespaces,
        &index,
    );
    assert_eq!(
        code_names(&report),
        ["undefined_prefixed_name", "undefined_prefixed_name"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The name 'gone' is being referenced through the prefix 'p', but it isn't defined \
             in any of the libraries imported using that prefix."
    );
}

/// A bare invocation miss inside a class or enum instance member names
/// `this`'s type (`UnqualifiedLookupDomain` invocation arm,
/// method_invocation_resolver.dart:223-276): every enclosing instance
/// element's `thisType` keys the miss — a *static* member's bare invoke
/// still names the enclosing type (`C.g` is `undefined_method(C)`),
/// matching the oracle's `UnqualifiedLookupDomain(thisType)`.
/// Top-level bodies keep `undefined_function`.
///
/// Hand-trace: `missing()` in `C.f` — the container walk clean-misses
/// (no `this` member, no extension pool, no library scope) and the
/// instance context maps the row to `undefined_method(C)`; `C.g`'s
/// static body resolves the same way, and the enum's instance body
/// picks `E`.
#[test]
fn bare_invoke_in_instance_reports_undefined_method() {
    let report = report(
        "class C { void f() { missing(); } static void g() { missing2(); } } \
             enum E { a; void f() { missing3(); } }",
    );
    assert_eq!(
        code_names(&report),
        ["undefined_method", "undefined_method", "undefined_method"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The method 'missing' isn't defined for the type 'C'."
    );
    assert_eq!(
        report.diagnostics[1].message,
        "The method 'missing2' isn't defined for the type 'C'."
    );
    assert_eq!(
        report.diagnostics[2].message,
        "The method 'missing3' isn't defined for the type 'E'."
    );
}

/// A stale type-parameter reference follows the live invocation context:
/// invoked inside a class instance member it reports `undefined_method`
/// naming the enclosing type (the `UnqualifiedLookupDomain` arm), not
/// the context-free `undefined_function`.
///
/// Hand-trace: `T` is out of scope in `D.m` (the engine's type-parameter
/// stack truncated when `C`'s frame closed), but the scoped lookup still
/// resolves the per-unit tag to `C`'s `TypeParam` declaration;
/// `bare_info_of` snapshots the live frames (`this` of `D`) instead of a
/// context-free missing row → `undefined_method(D)`.
#[test]
fn stale_type_param_invocation_follows_live_frame() {
    let report = report("class C<T> {} class D { void m() { T(); } }");
    assert_eq!(
        code_names(&report),
        ["undefined_method"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The method 'T' isn't defined for the type 'D'."
    );
}

/// A bare miss inside an extension-type body keeps the class-family
/// face: the extension type is an `InterfaceElement` upstream
/// (`ExtensionTypeElementImpl extends InterfaceElementImpl` — the
/// implicit `this` of the body carries the extension type itself), so
/// the UnqualifiedLookupDomain names the head exactly like a class
/// body names its class, never the context-free `undefined_function`.
/// The rig has no dart:core, so the `int i` field formal's undeclared
/// `int` spelling carries its `undefined_class` row beside the miss.
#[test]
fn extension_type_body_miss_names_the_extension_type() {
    let report = report("extension type E(int i) { void f() { miss(); } }");
    assert_eq!(
        code_names(&report),
        ["undefined_class", "undefined_method"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[1].message,
        "The method 'miss' isn't defined for the type 'E'."
    );
}

/// The `on`-face member union inside instance bodies (#2186 S2, emit
/// side #2230): a bare name or `this.member` access resolves against
/// the constraint face — hits stay silent; a member the constraint
/// provably lacks reports, naming `this`'s static type (the oracle's
/// `InstanceLookupDomain` receiver type).
///
/// Hand-trace (probed 3.13.1: all three bare/`this` misses name the
/// face type — the bare walk rides the same on-face as `this`, there
/// is no top-level `undefined_function` fallback): `real` hits `C`'s
/// method through `mixin M on C`'s constraint walk (silent).
/// `this.real2` misses and names `M`. `real3` is the bare decidable
/// miss: `extension X on C` fully walks `C`, `C` lacks the name, the
/// member scope is final — the miss names the on-type head `C`.
/// `this.real4` misses and names `C` too.
#[test]
fn mixin_and_extension_bodies_decidable_misses_report_on_this_type() {
    let report = report(
        "class C { void real() {} } \
             mixin M on C { void f() { real(); this.real2(); } } \
             extension X on C { void g() { real3(); this.real4(); } }",
    );
    assert_eq!(
        code_names(&report),
        [
            // real() — `C` declares it through M's constraint walk.
            // this.real2() — a mixin body's `this` is the mixin itself.
            "undefined_method",
            // real3() — the bare miss names the extension's on type.
            "undefined_method",
            // this.real4() — an extension body's `this` is the on type.
            "undefined_method",
        ],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The method 'real2' isn't defined for the type 'M'."
    );
    assert_eq!(
        report.diagnostics[1].message,
        "The method 'real3' isn't defined for the type 'C'."
    );
    assert_eq!(
        report.diagnostics[2].message,
        "The method 'real4' isn't defined for the type 'C'."
    );
}

// —— batch 2 (#197): undefined_operator (#120 residual 2) ————————————
// (index tests below double as residual 4's subscript-intent evidence)

/// A binary operator on a declared-typed receiver reports
/// `undefined_operator` at the operator token
/// (`binary_expression_resolver.dart`); a declared operator and
/// `this`-receiver hits stay silent; `super.operator` reports the super
/// operator shape.
///
/// Hand-trace: `c + d` — `c`'s row carries `Operator('+', token)`;
/// `C` declares `+` (silent). `d + d2` — `D` lacks `+` → the operator
/// token anchors `undefined_operator('+', D)`. `this - o` walks `E`'s
/// face; `super * this` walks `B` and reports the super text.
#[test]
fn binary_operator_misses_report_undefined_operator() {
    let report = report(
        "class C { C operator +(C o) => this; } class D {} \
             void f(C c, D d, D d2) { c + d; d + d2; } \
             class E { void g(E o) { this - o; } } \
             class B {} class S extends B { void h() { super * this; } }",
    );
    assert_eq!(
        code_names(&report),
        [
            // The binary fold's argument face joins the operator
            // checks (`c + d` resolves `C.operator +(C)`, whose formal
            // `D` cannot satisfy).
            "argument_type_not_assignable",
            "undefined_operator",
            "undefined_operator",
            "undefined_super_member",
        ],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The argument type 'D' can't be assigned to the parameter type 'C'. "
    );
    assert_eq!(
        report.diagnostics[1].message,
        "The operator '+' isn't defined for the type 'D'."
    );
    assert_eq!(
        report.diagnostics[2].message,
        "The operator '-' isn't defined for the type 'E'."
    );
    assert_eq!(
        report.diagnostics[3].message,
        "The operator '*' isn't defined in a superclass of 'S'."
    );
    assert_eq!(
        report.diagnostics[1].range.offset,
        u32::try_from(
            "class C { C operator +(C o) => this; } class D {} void f(C c, D d, D d2) { c + d; d "
                .len()
        )
        .unwrap()
    );
}

/// A binary operator on a member result applies to the member's
/// declared type (`c.j + r` checks `+` on the field's type); a missing
/// member reports the getter read instead — the operator's invalid
/// type never cascades.
///
/// Hand-trace: `j`'s row carries `Operator('+')`; the walk finds the
/// field `j: R` → the operator check runs on `R` → `undefined_operator`.
/// `c.k` has no declaration in `C` → clean miss → `undefined_getter`
/// only (a declared field would read cleanly and lend its own type to
/// the operator, so `k` must stay undeclared for the no-cascade shape).
/// `C` has no generative constructor, so the declaration loop owns the
/// `j` face (the targeted corpus fixture `class C { int z; }` is
/// nail-pinned oracle output).
#[test]
fn member_result_operator_checks_the_result_type() {
    let report = report(
        "class R {} class C { R j; } class R2 {} \
             void f(C c, R r, R2 r2) { c.j + r; c.k + r2; }",
    );
    assert_eq!(
        code_names(&report),
        [
            "not_initialized_non_nullable_instance_field",
            "undefined_operator",
            "undefined_getter",
        ],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[1].message,
        "The operator '+' isn't defined for the type 'R'."
    );
}

/// `this[0]` inside a GENERIC extension resolves the on-type's member
/// table, not the extension's own (`this` carries the `on` static
/// face — `extension_member_resolver.dart`). The on-type is an
/// unindexed core type: no member walk can answer, so the operator
/// suppresses. A non-generic header keeps the bare head verdict the
/// textual read already covered; the generic case is the regression —
/// the `extension E<T> on` needle hit the `<` where the `on` keyword
/// read expected it and the on-type never reached the receiver.
#[test]
fn extension_this_index_reads_the_generic_on_type() {
    // The on-type is a declared container whose member table carries no
    // `[]`; the oracle resolves the operator against `C`, not `E`, and
    // reports the miss on `C`'s name — not the extension's. Before the
    // generic-header skip, `extension_on_type_args` returned `None` for
    // `extension E<T> on C` and the operator walked `E`'s own members.
    let generic_report = report(
        "class C { C operator +(C o) => this; } \
             extension E<T> on C { void pick() { this[0]; } }",
    );
    assert_eq!(
        code_names(&generic_report),
        ["undefined_operator"],
        "{:?}",
        code_names(&generic_report)
    );
    assert_eq!(
        generic_report.diagnostics[0].message,
        "The operator '[]' isn't defined for the type 'C'."
    );
    // The same source without the parameter list keeps the bare-head
    // verdict (`this` reads `C` either way — the non-generic header
    // pins the generic-skip did not change the resolved face).
    let plain_report = report(
        "class C { C operator +(C o) => this; } \
             extension E on C { void pick() { this[0]; } }",
    );
    assert_eq!(code_names(&plain_report), ["undefined_operator"]);
    assert_eq!(
        plain_report.diagnostics[0].message,
        "The operator '[]' isn't defined for the type 'C'."
    );
}

/// A parameterized named-constructor reference (`C<T>.named(x)`)
/// resolves through `creation_ref`, not the row stack — its `.named`
/// selector pushes no row on a live head. The argument list's callee
/// read must therefore not reach back past the creation head to a stale
/// member row from an earlier statement. Before the adjacency gate in
/// `open_arguments`, `C<T>.named(x)` inside a mixin body picked up the
/// leftover `super.put` member row (two required formals) and reported
/// `not_enough_positional_arguments` on the one-argument `named` call.
#[test]
fn named_ctor_args_do_not_read_a_stale_member_callee() {
    // `MBuf.on MG` — `super.put(k, v)` leaves a `Member{Super}` row on
    // the stack; `Box<int>.named(seen)` on the next statement is the
    // parameterized named-ctor whose `(seen)` must not resolve `put` as
    // its callee.
    let report = report(
        "class MG<T> { void put(String k, T v) {} } \
             class Box<T> { Box.named(T v); } \
             mixin MBuf<T> on MG<T> { \
               void put(String k, T v) { super.put(k, v); } \
               Box<int> make() => Box<int>.named(0); } \
             ",
    );
    assert!(
        !code_names(&report).contains(&"not_enough_positional_arguments"),
        "{:?}",
        code_names(&report)
    );
}

/// A cascade-assign write types the variable: `x = C()..m()` carries the
/// creation's named type as its written type, so a `C?` local promotes
/// to `C` and a later member read stays silent
/// (`unchecked_use_of_nullable_value` only fires when the read's flow
/// type is still nullable). The flow layer used to type every
/// constructor-reference head `UNKNOWN`, so the write never promoted
/// and `x.rows` read `x` as still-nullable.
#[test]
fn cascade_creation_write_promotes_the_nullable_local() {
    let report = report(
        "class C { final int rows = 0; void row(int x) {} } \
             void f() { \
               C? maybe; \
               maybe?..row(1)..row(2); \
               maybe = C()..row(3); \
               int n = maybe.rows; \
             }",
    );
    assert!(
        !code_names(&report).contains(&"unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&report)
    );
}

/// The binary fold's result-type face (`_resolveUserDefinableType` +
/// `_refineNumericInvocationTypeNullSafe`): `a + b` is one node of the
/// refined numeric type — `int`-receiver `+` yields `num` unless both
/// operands read `int`, so a `String` operand drags the result to
/// `num` and the enclosing `int` return reports both rows (the oracle
/// anchors the argument at the right operand and the return at the
/// whole expression).
///
/// Hand-trace: `a + b` — `a`'s row carries `Operator('+')`; the fold
/// resolves `num.+`'s `(num) -> num` signature, runs the argument's
/// assignability cut (`String` vs `num` fails), and leaves one `num`
/// row the `int` return host rejects.
#[test]
fn binary_arithmetic_folds_to_the_numeric_result() {
    // `int`/`double` extend the declared `num` — the member walk
    // resolves `num.+` on their receivers exactly as it does on the
    // SDK face, and the numeric refinement's `num?` gate applies to
    // the subtypes identically.
    let report = report(
        "class num { num operator +(num o) => this; } \
             class int extends num {} \
             class double extends num {} \
             class String {} \
             int sum(int a, String b) { return a + b; } \
             int clean(int a, int b) { return a + b; } \
             double widen(int a, double b) => a + b;",
    );
    assert_eq!(
        code_names(&report),
        ["return_of_invalid_type", "argument_type_not_assignable",],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[1].message,
        "The argument type 'String' can't be assigned to the parameter type 'num'. "
    );
    assert_eq!(
        report.diagnostics[0].message,
        "A value of type 'num' can't be returned from the function 'sum' because it has a return type of 'int'."
    );
}

/// The user-operator arm of the same fold: an indexed container's
/// declared signature checks the right operand and yields the
/// declared return (`Money`'s `operator <` answers `bool` — the `int`
/// return host rejects it).
#[test]
fn binary_user_operator_checks_against_the_declared_signature() {
    let report = report(
        "class int {} class bool {} class String {} \
             class M { M operator +(M other) => this; \
             bool operator <(M other) => true; } \
             int f(M m, String s) { return m < s; }",
    );
    assert_eq!(
        code_names(&report),
        ["return_of_invalid_type", "argument_type_not_assignable",],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[1].message,
        "The argument type 'String' can't be assigned to the parameter type 'M'. "
    );
}

// —— #2172 (real-corpus family-4 remainder) ——————————————————————

/// The core static-return table types the send (`RangeError.checkValidRange`
/// declares `int`, sdk/lib/core/errors.dart:375): the known non-null RHS
/// promotes `end`, so the staged `int?`-against-`num` argument face at the
/// promoted read filters — the collection corpus's `binarySearchBy` row
/// dies at the root instead of per-read.
#[test]
fn core_static_return_types_the_range_error_send() {
    let source = "class bool {} \
                      class num { bool operator <(num other) => true; } \
                      class int extends num {} \
                      void f(int? e) { e = RangeError.checkValidRange(0, e, 3); if (e < 3) {} }";
    let names = code_names(&report_with_core_faces(source));
    assert!(names.is_empty(), "{names:?}");
}

/// The numeric refinement keeps the node unresolved over an unresolved
/// operand (`t + mystery()`'s declared `num` answer over-reports
/// downstream — the corpus's `_writeToList`/`_preGrow` `num`-to-`int`
/// rows). The callee's own miss is the only row left.
#[test]
fn unresolved_operand_keeps_the_numeric_fold_unresolved() {
    let source = "class int { int operator +(int other) => this; } \
                      class C { int t = 0; int f() { var fs = mystery() - 1; return t + fs; } }";
    let names = code_names(&report(source));
    assert_eq!(names, ["undefined_method"], "{names:?}");
}

/// The `is` expression folds to its own `bool` row: the operand row
/// defers (its resolution still runs in pass 2), so the expression-body
/// return reads the test's type — never the promoted operand (`bool
/// isValidKey(Object? o) => o is Key;`, the corpus's equality.dart
/// family).
#[test]
fn is_expression_folds_to_its_own_bool_row() {
    let source = "class Object {} class bool {} class Key {} \
                      bool isValidKey(Object? o) => o is Key;";
    let names = code_names(&report(source));
    assert!(names.is_empty(), "{names:?}");
}

/// The `as` expression folds to the cast target's type: the chain's next
/// segment resolves on the target, not the operand's type
/// (`(v as Comparable<Object?>).compareTo(o)` reads Comparable's face —
/// the corpus's utils.dart `defaultCompare` row).
#[test]
fn as_expression_folds_to_the_cast_target() {
    let source = "class Object {} class int {} \
                      class Comparable<T> { int compareTo(T other) => 1; } \
                      int defaultCompare(Object? a, Object? b) => \
                      (a as Comparable<Object?>).compareTo(b);";
    let names = code_names(&report(source));
    assert!(names.is_empty(), "{names:?}");
}

/// The dart:core `Comparator<T>` alias face: the callable verdict (the
/// invocation face spares it) and the send's `int` return both read it —
/// the corpus's `isSorted`/`compareBy` `invocation_of_non_function`
/// family (the SDK typedef is unindexed, the bounds tables never carry
/// it).
#[test]
fn core_comparator_alias_invokes_and_returns_int() {
    let source = "class int {} class Comparator<T> {} \
                      int f<T>(Comparator<T> compare, T a) { return compare(a, a); }";
    let names = code_names(&report(source));
    assert!(names.is_empty(), "{names:?}");
}

/// The raw-generic argument face: a member formal that still spells the
/// declaring container's own type parameter (`Equality` used raw leaves
/// `equals`'s `E` unsubstituted — the oracle's raw-type erasure answers
/// the bound) never stages an `argument_type_not_assignable` row — the
/// corpus's `_base.equals(e1, e2)` family.
#[test]
fn raw_generic_receiver_suppresses_the_unbound_formal_face() {
    let source = "class Object {} class bool {} \
                      class Equality<E> { bool equals(E a, E b) => true; } \
                      class Deep { Equality base = Equality(); } \
                      bool f(Deep d, Object? a) => d.base.equals(a, a);";
    let names = code_names(&report(source));
    assert!(names.is_empty(), "{names:?}");
}

/// The accessor-twin face: an inherited FIELD induces the setter face
/// (`_CastQueueList`'s `set _head` overrides `QueueList`'s
/// `int _head` field — the corpus's queue_list.dart setter rows).
#[test]
fn inherited_field_induces_the_setter_override_face() {
    let source = "class int {} \
                      class Q { int _head = 0; int read() => _head; } \
                      class CQ extends Q { @override set _head(int v) {} }";
    let names = code_names(&report(source));
    assert!(names.is_empty(), "{names:?}");
}

/// The dead-coalesce left operand is the index READ, not the receiver:
/// `m[k] ??= v` reads through `Map.[]`'s `V?` — the receiver row's
/// non-nullability is not the read's (the corpus's `groupSetsBy`/
/// `groupBy` rows).
#[test]
fn index_compound_write_never_reads_the_receiver_nullability() {
    let source = "class Object {} class V {} \
                      class M { V? operator [](Object? k) => null; \
                      void operator []=(Object? k, V v) {} } \
                      void f(M m, Object? k, V v) { m[k] ??= v; }";
    let names = code_names(&report(source));
    assert!(names.is_empty(), "{names:?}");
}

/// A superclass-constructor invocation counts as the named constructor's
/// reference: `super._(...)` spares `A._` from `unused_element` (the
/// corpus's `QueueList._`/`BoolList._`, whose only uses are subclass
/// `super._` initializers).
#[test]
fn super_constructor_invocation_spared_the_named_ctor() {
    let source = "class int {} \
                      class A { A._(int x); A.p(int y); } \
                      class B extends A { B() : super._(1); } \
                      void f() { B(); }";
    let names = code_names(&report(source));
    assert!(names.is_empty(), "{names:?}");
}

/// A scoped container the broken chain preempted is not an instance
/// member: `HashSet<T>()` in a field initializer of a class whose
/// `implements Iterator<T>` hop is unindexed keeps the initializer face
/// silent (the corpus's `_DeduplicatingIterator._emitted` row).
#[test]
fn broken_chain_does_not_claim_the_field_initializer_face() {
    let source = "class HashSet<T> {} \
                      class D<T> implements Iterator<T> { final h = HashSet<T>(); }";
    let names = code_names(&report_with_core_faces(source));
    assert!(names.is_empty(), "{names:?}");
}

/// An in-scope type-parameter cast target rides its bound: `null as T`
/// over an unbounded `T` is a legal null cast, the folded row types `T`,
/// and the return face accepts it (the corpus's `CombinedIterator.current`
/// `return null as T` rows).
#[test]
fn null_cast_to_a_type_parameter_target_stays_silent() {
    let source = "class I<T> { T get current => null as T; }";
    let names = code_names(&report(source));
    assert!(names.is_empty(), "{names:?}");
}

/// An extension's `this` types as its `on` type, and the unindexed
/// `Comparator<T>` alias head is the callable face (`compareBy`'s
/// `(R a, R b) => this(keyOf(a), keyOf(b))` — the corpus's
/// iterable_extensions.dart `this(...)` rows).
#[test]
fn extension_this_invokes_through_the_on_type_alias() {
    let source = "class int {} class Comparator<T> {} \
                      extension E<T> on Comparator<T> { int inv(T x) => this(x, x); }";
    let names = code_names(&report(source));
    assert!(names.is_empty(), "{names:?}");
}

/// The dead-coalesce target search skips the key send's row too:
/// `m[k(v)] ??= v` reads through `Map.[]`'s `V?` — the invoked key's
/// non-null return type is not the read's (the corpus's `groupBy` row,
/// `functions.dart:59`).
#[test]
fn invoked_key_never_anchors_the_dead_coalesce() {
    let source = "class Object {} class V {} \
                      class M { V? operator [](Object? k) => null; \
                      void operator []=(Object? k, V v) {} } \
                      Object? key(Object? o) => o; \
                      void f(M m, V v) { m[key(v)] ??= v; }";
    let names = code_names(&report(source));
    assert!(names.is_empty(), "{names:?}");
}

/// The compound-write mark rides the receiver row BELOW the right
/// operand's rows, and bare rows between them (`k`, a non-null `K`) must
/// not become the target: the mark scan filters past the RHS rows
/// instead of stopping at the first one (the corpus's `groupBy` row,
/// `functions.dart:59`). The for-in header reports the oracle's own row
/// (probed 3.13.1: `I<M>` implements no `Iterable` — "The type 'I<M>'
/// used in the 'for' loop must implement 'Iterable'."); the loop
/// variable reads `dynamic` and the compound body stays silent — the
/// mark scan's under-report posture still holds beside the header row.
#[test]
fn compound_mark_scan_reaches_through_the_right_operand_rows() {
    let source = "class Object {} class V {} class K {} class I<T> {} \
                      class M { V? operator [](Object? k) => null; \
                      void operator []=(Object? k, V v) {} } \
                      void f(I<M> ms, K k, V v) { for (var m in ms) { (m[k] ??= v); } }";
    let names = code_names(&report(source));
    assert_eq!(names, ["for_in_of_invalid_type"], "{names:?}");
}

/// `const x = Missing()` — an unresolved callee inside a const
/// initializer is known-invalid at the initializer (the oracle pairs
/// `const_initialized_with_non_constant_value` with the callee's own
/// resolution row, anchored at the whole call).
#[test]
fn const_initializer_unresolved_call_reports_both_faces() {
    let report = report("const config = Config();");
    assert_eq!(
        code_names(&report),
        [
            "undefined_function",
            "const_initialized_with_non_constant_value",
        ],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[1].message,
        "Const variables must be initialized with a constant value."
    );
    // The row anchors at the `Config()` call span (the call's own
    // range — `config = ` precedes it).
    assert_eq!(
        report.diagnostics[1].range.offset,
        u32::try_from("const config = ".len()).unwrap()
    );
    assert_eq!(
        report.diagnostics[1].range.length,
        u32::try_from("Config()".len()).unwrap()
    );
}

/// The flat positional destructure bind (`final (a, b) = pair()` — the
/// generated corpus's `sumN` shape): the bound locals read the matched
/// record's positional slots, so `a + b` checks `int + String` and the
/// refined `num` result faults the `int` return. Named fields, nested
/// records, and a var-count mismatch keep the under-report.
///
/// Hand-trace: `pair()`'s row is `Ty::Record (int, String)`; the
/// statement close distributes slots 0/1 onto `a`/`b`, so `a + b` is
/// `num.+`'s `(num) -> num` signature checking `String` and folding
/// `num`.
#[test]
fn record_pattern_binds_the_scrutinee_positional_slots() {
    let rep = report(
        "class num { num operator +(num o) => this; } \
             class int extends num {} \
             class String {} \
             (int, String) pair() => (1, 'x'); \
             int sum() { final (a, b) = pair(); return a + b; } \
             (int, int) size() => (3, 4); \
             int clean() { final (a, b) = size(); return a + b; }",
    );
    assert_eq!(
        code_names(&rep),
        ["return_of_invalid_type", "argument_type_not_assignable",],
        "{:?}",
        code_names(&rep)
    );
    assert_eq!(
        rep.diagnostics[1].message,
        "The argument type 'String' can't be assigned to the parameter type 'num'. "
    );
    // A named field keeps the window non-flat — the bind suppresses
    // and `a`'s unknown type never reaches the return check.
    let named = report(
        "({int a, String b}) pair() => (a: 1, b: 'x'); \
             int sum() { final (:a, :b) = pair(); return a + b; }",
    );
    assert!(
        named
            .diagnostics
            .iter()
            .all(|d| d.code.name != "argument_type_not_assignable"
                && d.code.name != "return_of_invalid_type"),
        "{:?}",
        code_names(&named)
    );
}

/// The record-field selector face the destructure feeds (`rect.width`
/// on a `({int width, int height})`-typed local): the member row reads
/// the field's declared slot type, so the full `sum10` chain
/// `a + b + rect.width * rect.height` folds to `num` and both oracle
/// rows land.
#[test]
fn record_field_access_types_the_member_row() {
    let rep = report(
        "class num { num operator +(num o) => this; num operator *(num o) => this; } \
             class int extends num {} \
             class String {} \
             (int, String) pair() => (1, 'x'); \
             ({int width, int height}) size() => (width: 3, height: 4); \
             int sum() { \
             final (a, b) = pair(); \
             final rect = size(); \
             return a + b + rect.width * rect.height; }",
    );
    assert_eq!(
        code_names(&rep),
        ["return_of_invalid_type", "argument_type_not_assignable",],
        "{:?}",
        code_names(&rep)
    );
    assert_eq!(
        rep.diagnostics[0].message,
        "A value of type 'num' can't be returned from the function 'sum' because it has a return type of 'int'."
    );
}

// —— batch 2 (#197): index intents (#120 residual 4) —————————————————

/// Index reads and writes check `[]` / `[]=` independently, anchored on
/// the bracket pair (`property_element_resolver.dart`); compound writes
/// check both; declared operators stay silent — and the index argument
/// never takes the assignment's write intent.
///
/// Hand-trace: `d[0]` folds `d`'s row to `Operator('[]', brackets)` →
/// the read spelling misses on `D`. `d[0] = 1` records the write form →
/// only `[]=` is checked. `d[0] += 1` checks both. The declared pair on
/// `C` resolves both forms of `c[0] = c[1]` silently, and `c[0] = null`
/// does NOT read `c`'s own type as the destination.
#[test]
fn index_read_write_and_compound_check_spellings() {
    let index_report = report(
        "class int {} class D {} \
             void f(D d, int i) { d[i]; d[i] = 1; d[i] += 1; }",
    );
    assert_eq!(
        code_names(&index_report),
        [
            "undefined_operator",
            "undefined_operator",
            "undefined_operator",
            "undefined_operator",
        ],
        "{:?}",
        code_names(&index_report)
    );
    assert_eq!(
        index_report.diagnostics[0].message,
        "The operator '[]' isn't defined for the type 'D'."
    );
    assert_eq!(
        index_report.diagnostics[1].message,
        "The operator '[]=' isn't defined for the type 'D'."
    );
    assert_eq!(
        index_report.diagnostics[2].message,
        "The operator '[]' isn't defined for the type 'D'."
    );
    assert_eq!(
        index_report.diagnostics[3].message,
        "The operator '[]=' isn't defined for the type 'D'."
    );
    let defined_report = report(
        "class int {} class C { C operator [](int i) => this; C operator []=(int i, C v) {} } \
             void f(C c, int i) { c[i] = c[i]; c[i] = null; }",
    );
    assert!(
        defined_report.diagnostics.is_empty(),
        "{:?}",
        code_names(&defined_report)
    );
}

/// The `c.j[i] = v` subscript intent (#120 residual 4): the write goes
/// through `[]=` on the member's result type; the index argument keeps
/// its read; no setter code touches `j`; a missing `j` reports the
/// getter read and suppresses the operator.
///
/// Hand-trace: the fold at `[` marks `j`'s member row
/// `Operator('[]', brackets)` and consumes `i`'s row; the assignment
/// records the `[]=` write form; the walk finds getter `j: V` → the
/// operator check runs on `V` → one `undefined_operator('[]=', V)`. In
/// the second statement `k` clean-misses → `undefined_getter(k)`, no
/// cascade.
#[test]
fn subscript_write_targets_the_member_result_type() {
    let report = report(
        "class int {} class V {} class C { V get j => V(); } \
             void f(C c, int i, V v) { c.j[i] = v; c.k[i] = v; }",
    );
    assert_eq!(
        code_names(&report),
        ["undefined_operator", "undefined_getter"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The operator '[]=' isn't defined for the type 'V'."
    );
    assert_eq!(
        report.diagnostics[0].range.offset,
        u32::try_from(
            "class int {} class V {} class C { V get j => V(); } void f(C c, int i, V v) { c.j"
                .len()
        )
        .unwrap()
    );
}

/// A compound member write checks both faces (`c.g += 1` needs the
/// getter read and the setter write) — the getter error first, matching
/// the oracle's resolution order.
///
/// Hand-trace: `g`'s row upgrades to `ReadWrite`; the getter walk
/// clean-misses → `undefined_getter`; the write walk misses `g` and
/// `g=` → `undefined_setter`.
#[test]
fn compound_member_write_reports_both_faces() {
    let report = report("class C {} void f(C c) { c.g += 1; }");
    assert_eq!(
        code_names(&report),
        ["undefined_getter", "undefined_setter"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The getter 'g' isn't defined for the type 'C'."
    );
    assert_eq!(
        report.diagnostics[1].message,
        "The setter 'g' isn't defined for the type 'C'."
    );
}

// —— records/patterns/exhaustiveness (M2 face) ————————————————————————

/// A statement switch over an enum with every constant covered is
/// exhaustive: no diagnostics.
///
/// Hand-trace: the scrutinee row carries `Color` (formal type); each
/// `case Color.red:` classifies through the constant-tail walk (the
/// `.red` member row over the bare `Color` row of an enum container)
/// into `EnumValue`; the engine's covering search finds no gap.
#[test]
fn enum_switch_all_constants_is_exhaustive() {
    let report = report(
        "enum Color { red, green, blue } void f(Color c) { switch (c) { case Color.red: break; case Color.green: break; case Color.blue: break; } }",
    );
    assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
}

/// A missing enum constant reports `non_exhaustive_switch_statement`
/// at the `switch` keyword, with the oracle's verbatim text and the
/// first missing constant as the suggested correction pattern.
///
/// Hand-trace: `blue` never covers → `NonExhaustive([Color.blue])`;
/// the statement is mustBeExhaustive (enums are always exhaustive,
/// type_system.dart:846-887) and no `default` is present.
#[test]
fn enum_switch_missing_constant_reports_correction() {
    let report = report(
        "enum Color { red, green, blue } void f(Color c) { switch (c) { case Color.red: break; case Color.green: break; } }",
    );
    assert_eq!(
        code_names(&report),
        ["non_exhaustive_switch_statement"],
        "{:?}",
        code_names(&report)
    );
    let d = &report.diagnostics[0];
    assert_eq!(
        d.message,
        "The type 'Color' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'Color.blue'."
    );
    // The oracle row's correctionMessage carries the suggested pattern
    // as `{2}` (diagnostic.g.dart:13079).
    assert_eq!(
        d.correction.as_deref(),
        Some("Try adding a default case or cases that match 'Color.blue'.")
    );
    assert_eq!(d.severity, Severity::Error);
    let switch_at =
        u32::try_from("enum Color { red, green, blue } void f(Color c) { ".len()).unwrap();
    assert_eq!(d.range.offset, switch_at);
}

/// A `default` case suppresses the non-exhaustive report even when
/// constants are missing (the oracle's `reportNonExhaustive` gate,
/// constant_verifier.dart:1122-1140).
#[test]
fn default_case_suppresses_non_exhaustive() {
    let report = report(
        "enum Color { red, green, blue } void f(Color c) { switch (c) { case Color.red: break; default: break; } }",
    );
    assert_eq!(report.diagnostics.len(), 0);
}

/// A duplicate constant after total coverage is
/// `unreachable_switch_case` (`STATIC_WARNING`) at the repeated case's
/// keyword — reported per case before the covering check, matching the
/// oracle's decision order (constant_verifier.dart:1080-1088).
#[test]
fn duplicate_constant_case_is_unreachable() {
    let report = report(
        "enum Color { red, green, blue } void f(Color c) { switch (c) { case Color.red: break; case Color.green: break; case Color.blue: break; case Color.red: break; } }",
    );
    assert_eq!(
        code_names(&report),
        ["unreachable_switch_case"],
        "{:?}",
        code_names(&report)
    );
    let d = &report.diagnostics[0];
    assert_eq!(d.message, "This case is covered by the previous cases.");
    assert_eq!(d.severity, Severity::Warning);
    let repeat_at = u32::try_from(
            "enum Color { red, green, blue } void f(Color c) { switch (c) { case Color.red: break; case Color.green: break; case Color.blue: break; ".len(),
        )
        .unwrap();
    assert_eq!(d.range.offset, repeat_at);
}

/// A `when`-guarded case never covers: the guarded case is excluded
/// from the covering search (`constant_verifier.dart` guarded rows) and
/// the missing constant still reports.
#[test]
fn guarded_case_does_not_cover() {
    let report = report(
        "enum Color { red, green, blue } void f(Color c) { switch (c) { case Color.red: break; case Color.green: break; case Color.blue when true: break; } }",
    );
    assert_eq!(code_names(&report), ["non_exhaustive_switch_statement"]);
    assert_eq!(
        report.diagnostics[0].message,
        "The type 'Color' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'Color.blue'."
    );
}

/// Switch expressions are always mustBeExhaustive: a missing constant
/// reports `non_exhaustive_switch_expression` even though a statement
/// over the same cases would stay silent only without the seal — and
/// the same shape over a *statement* is the T2 report, not this code.
#[test]
fn switch_expression_requires_all_constants() {
    let report = report(
        "enum Color { red, green, blue } void f(Color c) { var x = switch (c) { Color.red => 1, Color.green => 2 }; }",
    );
    // `x` binds the switch expression's value and is never read (the
    // official read-only use rule), so it reports before the
    // exhaustiveness row.
    assert_eq!(
        code_names(&report),
        ["unused_local_variable", "non_exhaustive_switch_expression"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[1].message,
        "The type 'Color' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'Color.blue'."
    );
    // The expression-face correctionMessage (diagnostic.g.dart:13018).
    assert_eq!(
        report.diagnostics[1].correction.as_deref(),
        Some("Try adding a wildcard pattern or cases that match 'Color.blue'.")
    );
}

/// A switch expression over a sealed class missing one subclass reports
/// the subclass name as the witness; covering every declared subclass
/// is exhaustive.
///
/// Hand-trace: `S` is sealed; `subtypes` walks the unit's containers in
/// declaration order over superclass edges → `[A, B]`; `case A a:`
/// leaves `B` uncovered.
#[test]
fn sealed_class_switch_reports_missing_subclass() {
    let missing = report(
        "sealed class S {} class A extends S {} class B extends S {} void f(S s) { var x = switch (s) { A a => 1 }; }",
    );
    // `x` is unread before the switch; the case variable `a` is unread
    // after it — the covering row anchors at the switch expression
    // (constant_verifier.dart:1080-1088), so source order interleaves
    // the two unused rows around it.
    assert_eq!(
        code_names(&missing),
        [
            "unused_local_variable",
            "non_exhaustive_switch_expression",
            "unused_local_variable",
        ],
        "{:?}",
        code_names(&missing)
    );
    assert_eq!(
        missing.diagnostics[1].message,
        "The type 'S' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'B'."
    );

    let covered = report(
        "sealed class S {} class A extends S {} class B extends S {} void f(S s) { var x = switch (s) { A a => 1, B b => 2 }; }",
    );
    // `x`, `a`, and `b` are all unread.
    assert_eq!(
        code_names(&covered),
        [
            "unused_local_variable",
            "unused_local_variable",
            "unused_local_variable",
        ]
    );
}

/// A switch expression over a nullable enum needs the `null` case: the
/// nullable projection splits into `[Color, null]` and the constants
/// alone leave the null half uncovered.
#[test]
fn nullable_enum_switch_needs_null_case() {
    let missing = report(
        "enum Color { red, green, blue } void f(Color? c) { var x = switch (c) { Color.red => 1, Color.green => 2, Color.blue => 3 }; }",
    );
    // `x` is unread (the official read-only use rule).
    assert_eq!(
        code_names(&missing),
        ["unused_local_variable", "non_exhaustive_switch_expression"]
    );
    assert_eq!(
        missing.diagnostics[1].message,
        "The type 'Color?' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'null'."
    );

    let with_null = report(
        "enum Color { red, green, blue } void f(Color? c) { var x = switch (c) { Color.red => 1, Color.green => 2, Color.blue => 3, null => 4 }; }",
    );
    // `x` is unread.
    assert_eq!(code_names(&with_null), ["unused_local_variable"]);
}

/// A single non-sealed type case covers the whole (non-nullable)
/// scrutinee: exhaustiveness does not require a seal.
///
/// Hand-trace: `case C v:` is a type-free-variable frame whose context
/// substitutes to `Named(C, false)` — the full space of the scrutinee.
#[test]
fn single_type_case_covers_non_sealed_scrutinee() {
    // `x` and the case variable `v` are never read (the official
    // read-only use rule).
    let report = report("class C {} void f(C c) { var x = switch (c) { C v => 1 }; }");
    assert_eq!(
        code_names(&report),
        ["unused_local_variable", "unused_local_variable"]
    );
}

/// A cyclic sealed hierarchy terminates: the covering search walks the
/// subclass closure through the file-scoped facts, which cannot reject
/// `sealed S extends S` (self-edge dropped at the source) or a mutual
/// `A`/`B` cycle (dropped by the walk's visited set) — without the
/// guard the FIFO expansion re-enqueues the same type forever.
///
/// Hand-trace: the `1` constant is the unknown space (never a
/// must-match), so the sealed walk must expand its subtypes — the
/// exact shape the cycle breaks. Both verdicts are exhaustive (no
/// witness over a sealed closure), so only the constant-pattern rows
/// emit: an `int` constant can never equal an `S`/`A` value
/// (`visitConstantPattern`'s `_canBeEqual` gate,
/// constant_verifier.dart:140-165).
#[test]
fn cyclic_sealed_hierarchy_terminates_without_false_positive() {
    let self_cycle =
        report("sealed class S extends S {} void f(S s) { var x = switch (s) { 1 => 2 }; }");
    // `x` is unread (the official read-only use rule); the constant
    // row anchors after it in source order.
    assert_eq!(
        code_names(&self_cycle),
        [
            "unused_local_variable",
            "constant_pattern_never_matches_value_type"
        ],
        "{:?}",
        code_names(&self_cycle)
    );
    assert_eq!(
        self_cycle.diagnostics[1].message,
        "The matched value type 'S' can never be equal to this constant of type 'int'."
    );
    let mutual_cycle = report(
        "sealed class A extends B {} sealed class B extends A {} void f(A a) { var x = switch (a) { 1 => 2 }; }",
    );
    assert_eq!(
        code_names(&mutual_cycle),
        [
            "unused_local_variable",
            "constant_pattern_never_matches_value_type"
        ],
        "{:?}",
        code_names(&mutual_cycle)
    );
    assert_eq!(
        mutual_cycle.diagnostics[1].message,
        "The matched value type 'A' can never be equal to this constant of type 'int'."
    );
}

/// An unresolvable scrutinee skips the whole exhaustiveness check (the
/// oracle's `InvalidType` gate at `constant_verifier.dart:1035` and
/// `:1095`): a switch expression over a call whose type never resolves
/// emits nothing — mapping it to the nullable top would drop every row
/// and false-positive the covering gap.
#[test]
fn unknown_scrutinee_switch_expression_emits_nothing() {
    // `x` is unread (the official read-only use rule); the `_`
    // wildcard never reports.
    let report = report("void g() {} void f() { var x = switch (g()) { _ => 0 }; }");
    assert_eq!(code_names(&report), ["unused_local_variable"]);
}

/// The extension-type representation face decides with the faces'
/// type arguments: `int`'s `Comparable` instantiation is
/// `Comparable<num>`, so `implements Comparable<Score>` is not a
/// supertype of the representation (the torture `extt` shape, oracle
/// 3.13.1: the row anchors the face's whole named type and renders it
/// with its arguments). `Comparable<num>` and `Object` hold.
#[test]
fn extension_type_argument_bearing_face_reports_not_supertype() {
    let source = "extension type Score(int raw) implements Comparable<Score> { \
                      int compareTo(Score other) => 0; }";
    let r = report(source);
    // The single-file rig carries no dart:core face: `int` itself
    // reports `undefined_class` here; the face under pin is the
    // representation row.
    assert_eq!(
        report_text_codes(source),
        ["extension_type_implements_not_supertype"]
    );
    let row = r
        .diagnostics
        .iter()
        .find(|d| d.code.name == "extension_type_implements_not_supertype")
        .expect("the representation row");
    assert_eq!(
        row.message,
        "'Comparable<Score>' is not a supertype of 'int', the representation type."
    );
    let at = source.find("Comparable<Score>").unwrap();
    assert_eq!(row.range.offset as usize, at);
    assert_eq!(row.range.length as usize, "Comparable<Score>".len());
    let holds = report_text_codes(
        "extension type A(int raw) implements Comparable<num> {} \
             extension type B(int raw) implements Object {}",
    );
    assert!(holds.is_empty(), "{holds:?}");
    // `Comparable<int>` is not `int`'s instantiation either.
    let exact = report_text_codes("extension type C(int raw) implements Comparable<int> {}");
    assert_eq!(exact, ["extension_type_implements_not_supertype"]);
}

/// The bare-head fallback answers the in-file class lattice: an
/// unrelated class face over an in-file class representation reports;
/// a face whose edges the world cannot see stays silent.
#[test]
fn extension_type_bare_face_falls_back_to_the_name_lattice() {
    let codes = report_text_codes("class A {} class C {} extension type E(A x) implements C {}");
    assert_eq!(codes, ["extension_type_implements_not_supertype"]);
    let sub =
        report_text_codes("class C {} class A extends C {} extension type E(A x) implements C {}");
    assert!(sub.is_empty(), "{sub:?}");
    let opaque = report("extension type E(Missing m) implements AlsoMissing {}");
    assert!(
        !code_names(&opaque).contains(&"extension_type_implements_not_supertype"),
        "{:?}",
        code_names(&opaque)
    );
}

/// The rig's code list without its missing-core `undefined_class` rows.
fn report_text_codes(source: &str) -> Vec<&'static str> {
    // The core-face cascade: `undefined_class` on the missing core
    // names and the type-argument face their arguments report.
    code_names(&report(source))
        .into_iter()
        .filter(|code| *code != "undefined_class" && *code != "non_type_as_type_argument")
        .collect()
}

/// A switch expression over a name declared outside the file emits no
/// covering gap: the file-scoped world cannot decide the name's
/// sealedness, so the expression face suppresses (the cross-file-sealed
/// interim seam) instead of false-positiving off an opaque non-sealed
/// reading.
///
/// Hand-trace: `S`/`A`/`B` have no in-file containers, so pre-fix the
/// scrutinee read as non-sealed with rows that never must-match it —
/// a witness over any `S` value, emitted as
/// `non_exhaustive_switch_expression`. The `undefined_class` rows are
/// the unresolved parameter type and the two object-pattern heads —
/// the oracle resolves `ObjectPattern.type` through the ordinary
/// `visitNamedType` pass and reports the same misses (see
/// `type_positions_report_undefined_class` and #2184).
#[test]
fn cross_file_sealed_scrutinee_expression_emits_no_covering_gap() {
    let report = report("void f(S s) { var x = switch (s) { A() => 1, B() => 2 }; }");
    // `x` is unread (the official read-only use rule), interleaved
    // with the three undefined heads/param.
    assert_eq!(
        code_names(&report),
        [
            "undefined_class",
            "unused_local_variable",
            "undefined_class",
            "undefined_class"
        ],
        "{:?}",
        code_names(&report)
    );
}

/// A switch expression's duplicate case is `unreachable_switch_case`
/// anchored at the repeated case's `=>` (the expression-case anchor,
/// constant_verifier.dart:1080-1088).
#[test]
fn switch_expression_duplicate_case_is_unreachable_at_arrow() {
    let report = report(
        "enum Color { red, green, blue } void f(Color c) { var x = switch (c) { Color.red => 1, Color.red => 2, Color.green => 3, Color.blue => 4 }; }",
    );
    // `x` is unread (the official read-only use rule), before the
    // duplicate-case row.
    assert_eq!(
        code_names(&report),
        ["unused_local_variable", "unreachable_switch_case"]
    );
    // The repeated case's `=>` (the expression-case anchor).
    let arrow_at = u32::try_from(
            "enum Color { red, green, blue } void f(Color c) { var x = switch (c) { Color.red => 1, Color.red ".len(),
        )
        .unwrap();
    assert_eq!(report.diagnostics[1].range.offset, arrow_at);
}

/// A typed variable pattern binds its local for the body: the bound
/// name reads cleanly, and the case narrows to the pattern's type.
///
/// Hand-trace: `handle_declared_variable_pattern` adds `other` to the
/// flat local frame with the armed `Color` type — `other;` resolves
/// through locals, so no `undefined_identifier`.
#[test]
fn typed_pattern_binds_local() {
    let report = report(
        "enum Color { red } void f(Color c) { switch (c) { case Color other: other; break; } }",
    );
    assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
}

/// An if-case binds the same way: `if (o case Type x)` declares `x`
/// for the then-block.
#[test]
fn if_case_binds_local() {
    let report =
        report("enum Color { red } void f(Color c) { if (c case Color other) { other; } }");
    assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
}

/// A pattern assignment's assigned variables are references: each
/// undeclared name reports `undefined_identifier` (the official
/// assigned-variable-pattern resolution), the RHS is a plain read, and
/// a declared local spares. The `var`/typed-declaration faces are the
/// parser's `pattern_assignment_declares_variable`, never this pass.
///
/// Corpus shapes (m0-syntax `declaredVariable_inPatternAssignment_
/// usingVar.dart`, `wildcard_inPatternAssignment_usingType.dart`):
/// `void f() { [a, var d] = y; }` carries `undefined_identifier` 'a'
/// at 2:4 and 'y' at 2:16.
#[test]
fn pattern_assignment_variables_resolve() {
    let r = report("void f() {\n  [a, var d] = y;\n}\n");
    let misses: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_identifier")
        .collect();
    assert_eq!(misses.len(), 2, "{:?}", code_names(&r));
    assert_eq!(misses[0].message, "Undefined name 'a'.");
    assert_eq!(misses[1].message, "Undefined name 'y'.");

    let r2 = report("void f() {\n  [a, int _] = y;\n}\n");
    let misses2: Vec<_> = r2
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_identifier")
        .collect();
    assert_eq!(misses2.len(), 2, "{:?}", code_names(&r2));
    assert_eq!(misses2[0].message, "Undefined name 'a'.");
    assert_eq!(misses2[1].message, "Undefined name 'y'.");

    let r3 = report("void f() {\n  var a = 0;\n  [a] = y;\n}\n");
    let misses3: Vec<_> = r3
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_identifier")
        .collect();
    assert_eq!(misses3.len(), 1, "{:?}", code_names(&r3));
    assert_eq!(misses3[0].message, "Undefined name 'y'.");
}

/// A type-free `var` pattern binds without a type and covers its
/// context (substituted with the scrutinee's type at case close).
#[test]
fn untyped_pattern_binds_and_covers() {
    let report = report("class C {} void f(C c) { switch (c) { case var y: y; break; } }");
    assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
}

/// Record patterns destructuring by shape: the fields bind as typed
/// locals and the name level emits no shape diagnostic (record shape
/// checking is the types-crate face — the documented M2 name-level
/// blind spot).
///
/// Hand-trace: the record pattern bracket pushes the unknown-space leaf
/// (never a false must-match); the field patterns bind `a`/`b` through
/// their declared-variable events, so both reads resolve.
#[test]
fn record_pattern_binds_fields_without_shape_diagnostic() {
    let report = report("class C {} void f(C c) { if (c case (C a, C b)) { a; b; } }");
    assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
}

// —— case-root capture (the case-sink window) ———————————————————————

/// An if-case root before a switch never feeds its cases: roots are
/// captured only across a case header's pattern window, so the stale
/// coverage from `if (y case bool _)` can neither cover a case nor
/// trip the empty fallback.
///
/// Hand-trace (pre-fix): the stale root sat below the case roots; the
/// second case's `end_switch_case` drained it and reported a false
/// `non_exhaustive_switch_statement` over `bool`.
#[test]
fn stale_if_case_root_never_feeds_a_case() {
    let report = report(
        "class bool {} void g(bool y, bool x) { if (y case bool _) {} switch (x) { case false: break; case true: break; } }",
    );
    assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
}

/// An if-case inside one case's body never feeds the next case: each
/// header captures at its own `end_case_expression`, so the stale root
/// cannot shift the case/space pairing.
///
/// Hand-trace (pre-fix): `case E.a`'s body if-case pushed above `E.a`'s
/// root and the LIFO pop handed the *next* case the wrong spaces —
/// a false `non_exhaustive_switch_statement` (missing `E.b`).
#[test]
fn stale_case_body_root_never_feeds_the_next_case() {
    let report = report(
        "class bool {} enum E { a, b } void f(bool y, E e) { switch (e) { case E.a: if (y case bool _) {} case E.b: break; } }",
    );
    assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
}

/// A nested switch opens after the enclosing case's root was captured:
/// the `open_switch` debris-clear must not destroy live case spaces.
#[test]
fn nested_switch_preserves_the_outer_case_root() {
    let report = report(
        "enum E { a, b } void f(E a, E b) { switch (a) { case E.a: switch (b) { case E.a: break; case E.b: break; } case E.b: break; } }",
    );
    assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
}

/// A shared-body case group (`case A: case B:`) captures one case per
/// header, matching the oracle's per-`case` nodes.
///
/// Hand-trace (pre-fix): one body event drained a single root, so the
/// group collapsed to its *last* header's space and `E.a` went
/// uncovered — a false `non_exhaustive_switch_statement`.
#[test]
fn shared_body_case_group_captures_each_header() {
    let report =
        report("enum E { a, b } void f(E e) { switch (e) { case E.a: case E.b: break; } }");
    assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
}

/// A labeled duplicate case anchors `unreachable_switch_case` at the
/// `case` keyword itself, not the label (the oracle's
/// `caseNode.keyword`, constant_verifier.dart:1076-1085).
#[test]
fn labeled_case_anchors_unreachable_at_case_keyword() {
    let report = report(
        "enum Color { red, green } void f(Color c) { switch (c) { case Color.red: break; case Color.green: break; l: case Color.red: break; } }",
    );
    assert_eq!(code_names(&report), ["unreachable_switch_case"]);
    let case_at = u32::try_from(
            "enum Color { red, green } void f(Color c) { switch (c) { case Color.red: break; case Color.green: break; l: ".len(),
        )
        .unwrap();
    assert_eq!(report.diagnostics[0].range.offset, case_at);
}

/// A `default` after total coverage is `unreachable_switch_default`
/// (`STATIC_WARNING`) at the `default` keyword — the case spaces
/// already cover the scrutinee and the enum face is always
/// mustBeExhaustive (`nonExhaustiveness == null && defaultNode != null
/// && mustBeExhaustive`, constant_verifier.dart:1141-1148).
#[test]
fn default_after_total_coverage_is_unreachable() {
    let report = report(
        "enum Color { red, green, blue } void f(Color c) { switch (c) { case Color.red: break; case Color.green: break; case Color.blue: break; default: break; } }",
    );
    assert_eq!(
        code_names(&report),
        ["unreachable_switch_default"],
        "{:?}",
        code_names(&report)
    );
    let d = &report.diagnostics[0];
    assert_eq!(
        d.message,
        "This default clause is covered by the previous cases."
    );
    assert_eq!(d.severity, Severity::Warning);
    let default_at = u32::try_from(
            "enum Color { red, green, blue } void f(Color c) { switch (c) { case Color.red: break; case Color.green: break; case Color.blue: break; ".len(),
        )
        .unwrap();
    assert_eq!(d.range.offset, default_at);
}

/// A `default` covering a real gap stays reachable: the
/// unreachable-default report lives only in the covering search's
/// exhaustive branch (the `nonExhaustiveness != null` gate silences
/// the whole else arm), and a switch without a default that covers
/// stays silent.
#[test]
fn reachable_default_and_no_default_stay_silent() {
    let covering_gap = report(
        "enum Color { red, green, blue } void f(Color c) { switch (c) { case Color.red: break; default: break; } }",
    );
    assert_eq!(
        covering_gap.diagnostics.len(),
        0,
        "{:?}",
        code_names(&covering_gap)
    );
    let no_default = report(
        "enum Color { red, green, blue } void f(Color c) { switch (c) { case Color.red: break; case Color.green: break; case Color.blue: break; } }",
    );
    assert_eq!(
        no_default.diagnostics.len(),
        0,
        "{:?}",
        code_names(&no_default)
    );
}

/// A statement switch over a *nullable* sealed scrutinee is
/// mustBeExhaustive — the official check reads the scrutinee's element
/// face and is nullability-agnostic (type_system.dart:846-866,
/// `isAlwaysExhaustive(S?)` over a sealed `S`) — so the missing `null`
/// half reports; covering it silences.
#[test]
fn nullable_sealed_statement_switch_needs_null() {
    let missing = report(
        "sealed class S {} class A extends S {} class B extends S {} void f(S? s) { switch (s) { case A a: break; case B b: break; } }",
    );
    // The case variables `a`/`b` are unread (the official read-only
    // use rule), after the exhaustiveness row.
    assert_eq!(
        code_names(&missing),
        [
            "non_exhaustive_switch_statement",
            "unused_local_variable",
            "unused_local_variable",
        ],
        "{:?}",
        code_names(&missing)
    );
    assert_eq!(
        missing.diagnostics[0].message,
        "The type 'S?' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'null'."
    );

    let with_null = report(
        "sealed class S {} class A extends S {} class B extends S {} void f(S? s) { switch (s) { case A a: break; case B b: break; case null: break; } }",
    );
    // The case variables `a`/`b` are unread.
    assert_eq!(
        code_names(&with_null),
        ["unused_local_variable", "unused_local_variable"],
        "{:?}",
        code_names(&with_null)
    );
}

/// A nullable *non-sealed* scrutinee stays off the mustBeExhaustive
/// gate (the element is not sealed, type_system.dart:846-866): a
/// statement switch without a default emits nothing.
#[test]
fn nullable_non_sealed_statement_switch_stays_unconditional() {
    // The case variable `c` is unread (the official read-only use
    // rule); the exhaustiveness gate stays off for the non-sealed
    // nullable scrutinee.
    let report = report("class C {} void f(C? c) { switch (c) { case C c: break; } }");
    assert_eq!(code_names(&report), ["unused_local_variable"]);
}

/// `NonNullableStaticType.isSubtypeOf` (static_type.dart:415-421):
/// every non-nullable static type — named, enum constant, or bool
/// value — subtypes the non-nullable top; `Null` and the nullable
/// projections do not.
#[test]
fn non_nullable_types_subtype_the_non_nullable_top() {
    use crate::member_face::FINAL_PRIMITIVES;
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let index = ElementIndex::default();
    let world = NameWorld {
        index: &index,
        file,
        strings: &strings,
        scope: None,
        resolved: std::cell::RefCell::default(),
        bool_id: strings.intern("bool"),
        object_id: strings.intern("Object"),
        primitive_ids: FINAL_PRIMITIVES.map(|p| strings.intern(p)),
        core_supertypes: CORE_SUPERTYPES.map(|(b, p)| (strings.intern(b), strings.intern(p))),
        unknowns: std::cell::Cell::new(0),
    };
    let c = strings.intern("C");
    let e = strings.intern("E");
    assert!(world.is_subtype_of(&NameTy::Named(c, false), &NameTy::TopNonNullable));
    assert!(world.is_subtype_of(&NameTy::EnumValue(e, c), &NameTy::TopNonNullable));
    assert!(world.is_subtype_of(&NameTy::BoolValue(true), &NameTy::TopNonNullable));
    assert!(!world.is_subtype_of(&NameTy::Null, &NameTy::TopNonNullable));
    assert!(!world.is_subtype_of(&NameTy::Named(c, true), &NameTy::TopNonNullable));
}

/// `decide_subtype`'s implicit-`Object` answers: the non-nullable top
/// is reflexive, and every core primitive subtypes it with no written
/// clause (`int`/`double`/`num` through the core chain, `bool`/
/// `String` through the primitive set — the written-clause walk cannot
/// see their edge). Undecided shapes stay `None`, never decisive: an
/// undeclared head against any supertype, and a primitive-vs-primitive
/// incomparable pair (the M1 assignability table owns those pairs
/// before the lattice is asked).
#[test]
fn decide_subtype_implicit_object_and_undecided_edges() {
    use crate::member_face::FINAL_PRIMITIVES;
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let index = ElementIndex::default();
    let world = NameWorld {
        index: &index,
        file,
        strings: &strings,
        scope: None,
        resolved: std::cell::RefCell::default(),
        bool_id: strings.intern("bool"),
        object_id: strings.intern("Object"),
        primitive_ids: FINAL_PRIMITIVES.map(|p| strings.intern(p)),
        core_supertypes: CORE_SUPERTYPES.map(|(b, p)| (strings.intern(b), strings.intern(p))),
        unknowns: std::cell::Cell::new(0),
    };
    let object = strings.intern("Object");
    assert_eq!(world.decide_subtype(object, object), Some(true));
    for name in ["int", "double", "num", "bool", "String"] {
        assert_eq!(
            world.decide_subtype(strings.intern(name), object),
            Some(true),
            "{name} <: Object"
        );
    }
    // The undeclared head: undecided against `Object` and against a
    // core primitive alike.
    let z = strings.intern("Z");
    assert_eq!(world.decide_subtype(z, object), None);
    assert_eq!(world.decide_subtype(z, strings.intern("bool")), None);
    // Two core primitives are incomparable at the lattice (decided
    // earlier by the assignability table): the closed core chain ends
    // in `Object`, not in the other primitive, so the walk cuts.
    assert_eq!(
        world.decide_subtype(strings.intern("num"), strings.intern("String")),
        None
    );
}

/// An object pattern's typed field covers its column when the member's
/// declared type subtypes the field's type test
/// ([`NameWorld::property_type`], the member-table walk, climbed from
/// the subclass here): the row lowers to the bare head, so `A()` plus
/// `B(x: int _)` is exhaustive over the sealed pair. Pre-fix the field
/// leaf `and`-folded against the head into the unknown space and the
/// covering gap mis-reported.
///
/// The genuinely non-exhaustive direction stays reported: a narrower
/// field test (`int` against `num`) does not cover the column, the row
/// stays unknown-shaped, and the gap names `B`.
#[test]
fn object_pattern_typed_field_covers_when_the_member_type_subtypes_it() {
    let covered = report(
        "class int {} sealed class S {} class A extends S {} sealed class Base extends S { int x = 0; } class B extends Base {} \
             void f(S s) { var v = switch (s) { A() => 1, B(x: int _) => 2 }; }",
    );
    // `v` is unread (the official read-only use rule); the `int _`
    // wildcard never reports.
    assert_eq!(code_names(&covered), ["unused_local_variable"]);

    let narrower = report(
        "class int {} class num {} sealed class S {} class A extends S {} sealed class Base extends S { num x = 0; } class B extends Base {} \
             void f(S s) { var v = switch (s) { A() => 1, B(x: int _) => 2 }; }",
    );
    // `v` is unread, before the exhaustiveness row.
    assert_eq!(
        code_names(&narrower),
        ["unused_local_variable", "non_exhaustive_switch_expression"],
        "{:?}",
        code_names(&narrower)
    );
}

/// A type-free object field (`B(:var x)`) covers its column without
/// consulting the member table, and an unresolvable head
/// (`undefined_class`, the cross-file shape) keeps the whole row in
/// the unknown space — no new report beyond the name resolution one.
#[test]
fn object_pattern_type_free_field_and_opaque_head_stay_quiet() {
    let type_free = report(
        "sealed class S {} class A extends S {} class B extends S {} \
             void f(S s) { var v = switch (s) { A() => 1, B(:var x) => 2 }; }",
    );
    // `v` and the bound field variable `x` are unread (the official
    // read-only use rule); `B` declares no `x`, so the shorthand's
    // getter read is `undefined_getter` at the variable (probed:
    // dart analyze 3.13.1 — 2:53 `The getter 'x' isn't defined for the
    // type 'B'.`).
    let mut codes = code_names(&type_free);
    codes.sort_unstable();
    assert_eq!(
        codes,
        [
            "undefined_getter",
            "unused_local_variable",
            "unused_local_variable"
        ]
    );
    let opaque = report("void f(S s) { var v = switch (s) { A() => 1, B(:var x) => 2 }; }");
    // `v` and `x` are unread; `S`, `A` and `B` are all unresolvable —
    // the parameter type and both object-pattern heads report
    // `undefined_class` (#2184: the head rides the ordinary
    // `visitNamedType` pass now).
    assert_eq!(
        code_names(&opaque),
        [
            "undefined_class",
            "unused_local_variable",
            "undefined_class",
            "undefined_class",
            "unused_local_variable",
        ]
    );
}

// —— restricted nullable projections (the Null|value fold) ———————————

/// `nullable` over a restricted value keeps the restriction: the
/// `Null|BoolValue(true)` fold lowers to `true?`, so `case null` after
/// `case true || null` is unreachable while `case false` stays
/// reachable and the `false` gap still reports.
///
/// Hand-trace: the fold space covers `{true, null}` only
/// (`NullableStaticType(_BoolValueStaticType)`, `static_type.dart:350`);
/// lowering it to `bool?` would swallow `false` on both questions.
/// Both `null` constants additionally carry
/// `constant_pattern_never_matches_value_type` (a `null` constant can
/// never equal a `bool` value — `visitConstantPattern`'s `_canBeEqual`
/// gate), independent of the covering verdicts.
#[test]
fn bool_value_nullable_keeps_the_restriction() {
    let report = report(
        "class bool {} void f(bool b) { switch (b) { case true || null: break; case null: break; } }",
    );
    let mut names = code_names(&report);
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "constant_pattern_never_matches_value_type",
            "constant_pattern_never_matches_value_type",
            // The trailing `case null`'s body dies with it (the flow
            // equality-check face — a non-nullable `bool` can never be
            // null), oracle-pinned on the `case null`-over-non-nullable
            // probe.
            "dead_code",
            "non_exhaustive_switch_statement",
            "unreachable_switch_case",
        ],
        "{:?}",
        code_names(&report)
    );
    // The `false` gap, not a swallowed one.
    let gap = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "non_exhaustive_switch_statement")
        .expect("non-exhaustive row present");
    assert_eq!(
        gap.message,
        "The type 'bool' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'false'."
    );
    // The unreachable verdict anchors at its own `case` keyword
    // (constant_verifier.dart:1076-1085).
    let null_case_at = u32::try_from(
        "class bool {} void f(bool b) { switch (b) { case true || null: break; ".len(),
    )
    .unwrap();
    let unreachable = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "unreachable_switch_case")
        .expect("unreachable row present");
    assert_eq!(unreachable.range.offset, null_case_at);
}

/// The same precision for enum constants: `case E.a || null` covers
/// `{E.a, null}` only — the sibling `E.b` stays reachable, and the
/// nullable-scrutinee statement face emits nothing else.
///
/// Hand-trace (pre-fix): the fold lowered to `E?`, whose split covers
/// every constant — a false `unreachable_switch_case` on `E.b`.
#[test]
fn enum_value_nullable_keeps_the_restriction() {
    let report = report(
        "enum E { a, b } void f(E? e) { switch (e) { case E.a || null: break; case E.b: break; } }",
    );
    assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
}

// —— enum/mixin supertype edges toward sealed classes —————————————————

/// An enum's `implements` and a mixin's `on` edge reach the sealed
/// splitter: both join the sealed class's closed subtype set, so
/// covering the enum's constants and the mixin covers everything.
///
/// Hand-trace: `subtypes(S)` walks the unit's containers over the
/// superclass and interface edges → `[E, M]`; `E` splits over its
/// constants and `M` is covered by the typed case.
#[test]
fn enum_and_mixin_edges_split_the_sealed_class() {
    let covered = report(
        "sealed class S {} enum E implements S { a, b } mixin M on S {} void f(S s) { switch (s) { case E.a: break; case E.b: break; case M m: break; } }",
    );
    // The case variable `m` is unread (the official read-only use rule).
    assert_eq!(code_names(&covered), ["unused_local_variable"]);

    let missing = report(
        "sealed class S {} enum E implements S { a, b } mixin M on S {} void f(S s) { switch (s) { case E.a: break; case E.b: break; } }",
    );
    assert_eq!(
        code_names(&missing),
        ["non_exhaustive_switch_statement"],
        "{:?}",
        code_names(&missing)
    );
    assert_eq!(
        missing.diagnostics[0].message,
        "The type 'S' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'M'."
    );
}

/// A typed wildcard (`M _`) covers its declared type's space: the
/// canonical sealed-coverage style turns exhaustive.
///
/// Hand-trace (pre-fix): the wildcard dropped its type, each case
/// collapsed to the context space, and the second case reported a false
/// `unreachable_switch_case`.
#[test]
fn typed_wildcard_covers_its_type() {
    let report = report(
        "sealed class S {} class A extends S {} class B extends S {} void f(S s) { switch (s) { case A _: break; case B _: break; } }",
    );
    assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
}

/// A bare `_` still covers the whole context (no leaf of its own).
#[test]
fn bare_wildcard_covers_the_context() {
    // `x` is unread (the official read-only use rule); the `_`
    // wildcard never reports.
    let report = report("class C {} void f(C c) { var x = switch (c) { _ => 1 }; }");
    assert_eq!(code_names(&report), ["unused_local_variable"]);
}

// —— #279: the constant-evaluation family ———————————————————————————
//
// Anchors: the reporting sites in upstream Dart SDK source,
// `pkg/analyzer/lib/src/dart/constant/constant_verifier.dart`
// (`visitVariableDeclaration` :552, the
// `_reportError` allow-list :717+), the operator matrix in
// `.../constant/value.dart`, the invocation faults in
// `.../constant/evaluation.dart` (`_invalidConstantForMethodInvocation`
// :2197), the verbatim texts in
// `.../analyzer/lib/src/diagnostic/diagnostic.g.dart`, and the corpus
// `pkg/analyzer/test/src/dart/constant/evaluation_test.dart` /
// `tests/language/operator/integer_division_by_zero_test.dart`.

/// Clean const initializers over every subset shape: arithmetic,
/// adjacent strings, collections, records, const-ctor creations
/// (explicit and implicit), forward const references — no rows at all.
#[test]
fn const_initializer_subset_shapes_stay_clean() {
    let rep = report(
        "class int {} class C { const C(); } \
             const a = 1 + 2; \
             const b = 'x' 'y'; \
             const c = [1, 2]; \
             const d = {'k': 1}; \
             const e = {}; \
             const g = C(); \
             const h = const C(); \
             const fwd = base; const base = 1; \
             const neg = -a; \
             const shift = 1 << 2;",
    );
    assert_eq!(rep.diagnostics.len(), 0, "{:?}", code_names(&rep));
}

/// A provably non-const operand reports the site default
/// `const_initialized_with_non_constant_value` anchored at the
/// offending subexpression (`constant_verifier.dart:552`; the
/// `InvalidConstant` error entity).
#[test]
fn non_const_initializer_reports_the_site_default() {
    let rep = report("class int {} int nc = 0; const x = 1 + nc;");
    assert_eq!(
        code_names(&rep),
        ["const_initialized_with_non_constant_value"],
        "{:?}",
        code_names(&rep)
    );
    // The local/formal shape proves non-constancy too; the const
    // local `y` is never read (the official read-only use rule), and
    // the const verdict anchors at the offending subexpression `l`.
    let rep = report("void m() { var l = 1; const y = l; }");
    assert_eq!(
        code_names(&rep),
        [
            "unused_local_variable",
            "const_initialized_with_non_constant_value",
        ],
        "{:?}",
        code_names(&rep)
    );
}

// —— #747: equal_elements_in_set / equal_keys_in_map ————————————————

/// A repeated constant element reports `equal_elements_in_set` at the
/// LATER element's own span, once per repeat, with the verbatim oracle
/// message (`best_practices_verifier.dart:1077-1109`, messages.yaml
/// :26907).
#[test]
fn duplicate_set_element_reports_at_the_later_element() {
    let rep = report("var s = {1, 1};");
    assert_eq!(
        code_names(&rep),
        ["equal_elements_in_set"],
        "{:?}",
        code_names(&rep)
    );
    let d = &rep.diagnostics[0];
    assert_eq!(
        d.message,
        "Two elements in a set literal shouldn't be equal."
    );
    assert_eq!(d.range.offset, u32::try_from("var s = {1, ".len()).unwrap());
    assert_eq!(d.range.length, 1);
    // Multiple repeats each report, in element order.
    let rep = report("var s = {1, 1, 1};");
    let at: Vec<u32> = rep.diagnostics.iter().map(|d| d.range.offset).collect();
    assert_eq!(
        at,
        [
            u32::try_from("var s = {1, ".len()).unwrap(),
            u32::try_from("var s = {1, 1, ".len()).unwrap(),
        ]
    );
}

/// A repeated constant KEY reports `equal_keys_in_map` at the later
/// key's own span with the verbatim oracle message and correction
/// (messages.yaml :26954); equal VALUES under distinct keys never
/// report (map values are not participants).
#[test]
fn duplicate_map_key_reports_at_the_later_key() {
    let rep = report("var m = {'a': 1, 'a': 2};");
    assert_eq!(
        code_names(&rep),
        ["equal_keys_in_map"],
        "{:?}",
        code_names(&rep)
    );
    let d = &rep.diagnostics[0];
    assert_eq!(d.message, "Two keys in a map literal shouldn't be equal.");
    assert_eq!(
        d.correction.as_deref(),
        Some("Change or remove the duplicate key.")
    );
    assert_eq!(
        d.range.offset,
        u32::try_from("var m = {'a': 1, ".len()).unwrap()
    );
    assert_eq!(d.range.length, 3);

    let rep = report("var m = {'a': 1, 'b': 1};");
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
}

/// The silent faces: evaluation failure is silence
/// (`computeConstantValue` fails) and ints and doubles never unify.
/// Explicitly-`const` literals report the ERROR-tier
/// `equal_elements_in_const_set` row themselves.
#[test]
fn const_and_non_constant_duplicates_stay_silent() {
    let rep = report("var x = 0; var s = {x, x};");
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    let rep = report("var s = {1, 1.0};");
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    // Const literals carry the ERROR-tier row for their own duplicate.
    let rep = report("const s = {1, 1};");
    assert_eq!(
        code_names(&rep),
        ["equal_elements_in_const_set"],
        "{:?}",
        code_names(&rep)
    );
    let rep = report("const c = [{1, 1}];");
    assert_eq!(
        code_names(&rep),
        ["equal_elements_in_const_set"],
        "{:?}",
        code_names(&rep)
    );
}

/// Spread/if/for elements skip the whole literal's duplicate check —
/// the capture cannot tell their folded operands apart from plain
/// elements (the documented under-report posture; the oracle excludes
/// only those elements themselves).
#[test]
fn control_flow_elements_skip_the_duplicate_check() {
    let rep = report("var y = [1]; var s = {1, ...y, 1};");
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
}

/// A literal nested inside a dup-checked literal leaves its opaque
/// placeholder on the parent stack, keeping the parent's element and
/// entry pairing aligned: a nested map as a VALUE keeps its entry (so
/// its key participates), and a nested literal as an ELEMENT never
/// reports (the oracle's `computeConstantValue` fails on it).
#[test]
fn nested_literals_keep_parent_pairing_aligned() {
    // Without the placeholder the first key would lose its entry and
    // this repeat would go unreported.
    let rep = report("var m = {1: {2: 3}, 1: 4};");
    assert_eq!(
        code_names(&rep),
        ["equal_keys_in_map"],
        "{:?}",
        code_names(&rep)
    );
    assert_eq!(
        rep.diagnostics[0].range.offset,
        u32::try_from("var m = {1: {2: 3}, ".len()).unwrap()
    );
    let rep = report("var s = {1, {2: 3}, 1};");
    assert_eq!(
        code_names(&rep),
        ["equal_elements_in_set"],
        "{:?}",
        code_names(&rep)
    );
    assert_eq!(
        rep.diagnostics[0].range.offset,
        u32::try_from("var s = {1, {2: 3}, ".len()).unwrap()
    );
}

/// Unknown names co-fire the const verdict (evaluation.dart:2046-2049:
/// an unresolved operand is a `genericError`, which upgrades inside a
/// const *variable* initializer to the default
/// `const_initialized_with_non_constant_value` at the same anchor —
/// constant_verifier.dart:717-790's fall-through). The defer framing
/// was the under-report posture; the oracle emits both rows.
#[test]
fn unknown_names_defer_without_a_const_verdict() {
    let rep = report("const x = gone;");
    assert_eq!(
        code_names(&rep),
        [
            "const_initialized_with_non_constant_value",
            "undefined_identifier",
        ],
        "{:?}",
        code_names(&rep)
    );
}

/// A self-reference inside its own initializer is the cycle fault
/// (the allow-list's `recursive_compile_time_constant` override,
/// `constant_verifier.dart:717`+).
#[test]
fn self_referential_constant_reports_the_cycle() {
    let rep = report("const a = a + 1;");
    assert_eq!(
        code_names(&rep),
        ["recursive_compile_time_constant"],
        "{:?}",
        code_names(&rep)
    );
}

/// A member static const's value joins `const_field_values` (#511: the
/// field-context routing): `A.x` folds through it, `y` binds a real
/// value in the file const scope, and the fold is observable at a
/// value-dependent consumer — `z` divides by `y - 4`, zero exactly
/// when `y` folded to 4. Before the routing fix `A.x` degraded to the
/// symbolic `Verdict::Constant`, `y` never folded, and this fixture
/// was silent.
#[test]
fn static_const_field_folds_through_the_const_environment() {
    let rep = report(
        "class int {} class A { static const x = 1 + 2; } \
             const y = A.x + 1; \
             const z = 1 ~/ (y - 4);",
    );
    assert_eq!(
        code_names(&rep),
        ["const_eval_throws_idbze"],
        "{:?}",
        code_names(&rep)
    );
}

/// A non-const field group never arms the const window: `j`'s
/// initializer closes with nothing pending and no `(C, j)` entry.
#[test]
fn non_const_field_group_stays_out_of_the_const_environment() {
    let rep = report("class int {} class C { int j = 1 + 2; }");
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
}

/// A closure inside a field initializer carries its own locals: `a`'s
/// window opens deeper than the field group and closes on the local
/// face (the #511 misroute guard) — no const face, no entry, no row.
#[test]
fn closure_locals_inside_field_initializers_stay_local() {
    let rep = report("class int {} class C { int i = (() { int a = 1; return a; })(); }");
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
}

/// Const-creation arguments check per argument with
/// `const_with_non_constant_argument`
/// (`_validateConstantArguments`, `constant_verifier.dart:843)`: the
/// explicit `const C(l)` and the implicit `C(l)` forms, and enum
/// constants with a declared const constructor.
#[test]
fn const_creation_arguments_check_per_argument() {
    let rep = report(
        "class int {} class C { const C(int v); } \
             void m() { var l = 1; const a = C(l); const b = const C(l); }",
    );
    // The const locals `a`/`b` are never read (the official read-only
    // use rule); the argument verdicts anchor at their `l` arguments,
    // each carrying the initializer row beside it (the oracle's
    // const_with_non_constant_argument fixture emits both codes at the
    // argument).
    assert_eq!(
        code_names(&rep),
        [
            "unused_local_variable",
            "const_with_non_constant_argument",
            "const_initialized_with_non_constant_value",
            "unused_local_variable",
            "const_with_non_constant_argument",
            "const_initialized_with_non_constant_value",
        ],
        "{:?}",
        code_names(&rep)
    );

    let rep =
        report("class int {} int l = 0; enum E { a.named(l); const E.named(int v); final int v; }");
    // The `final int v;` is uninitialized and `const E.named(int v)`
    // never initializes it: the per-constructor final-fields row rides
    // beside the argument verdict (the named-head anchor the flush
    // gained — the pre-rework pin carried the argument row only, the
    // named-selector under-report).
    assert_eq!(
        code_names(&rep),
        [
            "const_with_non_constant_argument",
            "final_not_initialized_constructor",
        ],
        "{:?}",
        code_names(&rep)
    );
}

/// A multi-positional const creation evaluates each argument on its
/// own: the parser emits `handle_positional_argument` once per
/// positional operand (parser_impl.dart:9608), so `const v = C(0, 0)`
/// captures two `Value` nodes — not one merged `Other`. Before the
/// event landed every positional list collapsed to a single
/// non-constant leaf and over-fired both `const_with_non_constant_argument`
/// and `const_initialized_with_non_constant_value` at the argument.
#[test]
fn const_creation_positional_arguments_fold_per_argument() {
    let rep = report(
        "class int {} class C { const C(int a, int b); } \
             const C v = C(0, 0);",
    );
    assert!(
        !code_names(&rep).iter().any(|c| c.starts_with("const_")),
        "{:?}",
        code_names(&rep)
    );
}

/// A const-creation target this file declared with a const
/// constructor keeps clean arguments silent, and the creation binds
/// its folded value (`const x = C();` has static type `C`). The
/// negative face (a foreign host type) defers: user-class pairs sit
/// outside the M1's final-primitive table, which is the engine's
/// documented under-report direction.
#[test]
fn const_creation_type_folds_to_its_container() {
    // `ok` is never read (the official read-only use rule); the const
    // chain itself stays clean.
    let rep = report(
        "class C { const C(); } \
             const x = C(); \
             const w = x; \
             void m() { C ok = x; }",
    );
    assert_eq!(code_names(&rep), ["unused_local_variable"]);
}

/// A container without a const constructor reports both initializer
/// rows at the creation (the oracle's const_with_non_const fixture:
/// `const x = C()` over `class C { C(); }` emits
/// `const_with_non_const` AND `const_initialized_with_non_constant_
/// value` at the same span); an unresolvable name keeps only the
/// `const_initialized` half beside its own undefined row (probed
/// 3.13.1: `const y = Gone()` emits `undefined_function` +
/// `const_initialized_with_non_constant_value` — the non-const-
/// constructor row needs a resolved constructor).
#[test]
fn non_const_and_unknown_creation_targets_defer() {
    let rep = report("class C { C(); } const x = C();");
    assert_eq!(
        code_names(&rep),
        [
            "const_initialized_with_non_constant_value",
            "const_with_non_const",
        ],
        "{:?}",
        code_names(&rep)
    );

    let rep = report("const y = Gone();");
    assert_eq!(
        code_names(&rep),
        [
            "undefined_function",
            "const_initialized_with_non_constant_value",
        ],
        "{:?}",
        code_names(&rep)
    );
}

/// An invocable non-constructor callee faults with
/// `const_eval_method_invocation` at the invocation
/// (evaluation.dart:2197, diagnostic.g.dart:2846).
#[test]
fn invocable_callee_reports_method_invocation() {
    let rep = report("void f() {} const x = f();");
    assert_eq!(
        code_names(&rep),
        ["const_eval_method_invocation"],
        "{:?}",
        code_names(&rep)
    );
    assert_eq!(
        rep.diagnostics[0].message,
        "Methods can't be invoked in constant expressions."
    );
    // A local callable faults the same way; the const local `y` is
    // never read (the official read-only use rule), before the
    // invocation anchor. The `var`-inferred `g` now carries its `int`
    // static type, so the invocation of a non-function faults too —
    // the oracle's own co-row (probed 3.13.1: `error -
    // invocation_of_non_function_expression` at `g()`).
    let rep = report("void m() { var g = 1; const y = g(); }");
    assert_eq!(
        code_names(&rep),
        [
            "unused_local_variable",
            "const_eval_method_invocation",
            "invocation_of_non_function_expression"
        ],
        "{:?}",
        code_names(&rep)
    );
}

/// Explicit const literals check per element with their
/// `non_constant_*_element` codes (the `_ConstLiteralVerifier` sites).
#[test]
fn const_literal_elements_check_per_element() {
    let rep = report(
        "void m() { var l = 1; \
             const a = [l]; \
             const b = {l}; \
             const c = {l: 1}; \
             const d = (l,); }",
    );
    // The const locals `a`/`b`/`c`/`d` are never read (the official
    // read-only use rule); each element verdict anchors at its `l`
    // element, and the failed literal fails the declaration at the same
    // element (oracle-pinned); map entries name the failing side.
    assert_eq!(
        code_names(&rep),
        [
            "unused_local_variable",
            "non_constant_list_element",
            "const_initialized_with_non_constant_value",
            "unused_local_variable",
            "non_constant_set_element",
            "const_initialized_with_non_constant_value",
            "unused_local_variable",
            "non_constant_map_key",
            "const_initialized_with_non_constant_value",
            "unused_local_variable",
            "non_constant_record_field",
            "const_initialized_with_non_constant_value",
        ],
        "{:?}",
        code_names(&rep)
    );
    // Clean elements stay silent, including the fault-deciding lazy
    // shapes (`false && l` never evaluates its right operand); the
    // const locals `e`/`f`/`g` are unread. The constant-false `&&`
    // left does make the right operand run dead (the official
    // `LogicalAnd` arm re-anchors `dead_code` at the operator spanning
    // the operand, dead_code_verifier.dart:311-322) — a separate lane
    // from the const-eval laziness above.
    let rep = report(
        "void m() { var l = 1; const e = [0, 1]; const f = {0: 1}; const g = false && l == 1; }",
    );
    assert_eq!(
        code_names(&rep),
        [
            "unused_local_variable",
            "unused_local_variable",
            "unused_local_variable",
            "dead_code",
        ],
        "{:?}",
        code_names(&rep)
    );
}

/// The `constEval*` fault family overrides the site default, one code
/// per subfamily (the `_reportError` allow-list): division throws,
/// operand-type asserts, interpolation operand gate.
#[test]
fn const_eval_faults_match_the_official_codes() {
    // `~/ 0` throws IntegerDivisionByZeroException
    // (tests/language/operator/integer_division_by_zero_test.dart);
    // `% 0` throws the generic exception
    // (evaluation_test.dart:1765).
    let rep = report("const a = 1 ~/ 0; const b = 1 % 0;");
    assert_eq!(
        code_names(&rep),
        ["const_eval_throws_idbze", "const_eval_throws_exception"],
        "{:?}",
        code_names(&rep)
    );
    // Operand-type asserts, left operand first
    // (evaluation_test.dart:1895 anchors the bool case). The binary
    // fold's argument face joins the const-eval faults — probed
    // 3.13.1: `const c = 1 + 'a'` emits `const_eval_type_num` AND
    // `argument_type_not_assignable` (String vs num); `1 & true` and
    // `'a' + 1` pair their fault rows the same way. Each argument row
    // anchors at its right operand, after the fault at the whole
    // binary (probed 3.13.1, source order: type_bool 1|11, type_num
    // 1|25, argument 1|29, type_bool_int 1|44, argument 1|48,
    // throws_exception 1|64, argument 1|70).
    let rep = report("const c = !1; const d = 1 + 'a'; const e = 1 & true; const f = 'a' + 1;");
    assert_eq!(
        code_names(&rep),
        [
            "const_eval_type_bool",
            "const_eval_type_num",
            "argument_type_not_assignable",
            "const_eval_type_bool_int",
            "argument_type_not_assignable",
            "const_eval_throws_exception",
            "argument_type_not_assignable",
        ],
        "{:?}",
        code_names(&rep)
    );
    // Interpolation operand gate (`isBoolNumStringOrNull`,
    // value.dart:545): a constant operand of a non-convertible type
    // faults; a NON-CONSTANT operand keeps the site default (the
    // oracle fails its evaluation first).
    let rep = report("const g = 'a${[1]}';");
    assert_eq!(
        code_names(&rep),
        ["const_eval_type_bool_num_string"],
        "{:?}",
        code_names(&rep)
    );
    // The const local `h` is never read (the official read-only use
    // rule); the verdict anchors at the interpolated `l`.
    let rep = report("void m() { var l = 1; const h = 'a${l}'; }");
    assert_eq!(
        code_names(&rep),
        [
            "unused_local_variable",
            "const_initialized_with_non_constant_value",
        ],
        "{:?}",
        code_names(&rep)
    );
}

/// The dynamic operand face (tests/corpus/m1-semantic
/// lib/targeted/const_eval_type_num_string.dart:9): a `dynamic` right
/// operand is assignable to the operator's formal — the argument face
/// stays silent and only the const-eval fault reports, const context
/// or not. The literal operand keeps the co-fire the oracle probes
/// under [`Self::const_eval_faults_match_the_official_codes`] pin.
#[test]
fn binary_argument_face_spares_the_dynamic_operand() {
    let rep = report("const dynamic b = true;\nconst x = 'a' + b;\n");
    assert_eq!(
        code_names(&rep),
        ["const_eval_type_num_string"],
        "{:?}",
        code_names(&rep)
    );
    let rep = report("void f(dynamic b) {\n  'a' + b;\n}");
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
}

/// The throws-exception face links its throw site (#1876 W5): the
/// `constEvalThrowsException` row carries one context message, the
/// upstream wrap text over the inner problemMessage — every generic
/// runtime path in `value.dart` throws that code itself, so the
/// formatted message is its problemMessage constant — anchored at the
/// fault span (the deepest construct, here the `%` operator: the same
/// entity upstream's context row anchors at,
/// evaluation.dart:352-373). The non-exception faults carry no
/// context (`const_eval_throws_idbze` reports bare — the wrapped
/// constructor-call face is out of scope until the corpus exercises
/// it).
#[test]
fn throws_exception_links_the_throw_site_context() {
    let rep = report("const b = 1 % 0;");
    assert_eq!(code_names(&rep), ["const_eval_throws_exception"]);
    let row = &rep.diagnostics[0];
    let context = row.context.as_ref().expect("the wrap attaches its row");
    assert_eq!(context.len(), 1);
    assert_eq!(
        &*context[0].message,
        "The exception is 'Evaluation of this constant expression throws an \
             exception.' and occurs here."
    );
    assert_eq!(context[0].file, row.file);
    assert_eq!(context[0].range, row.range);
    // The idbze twin keeps the bare face.
    let rep = report("const a = 1 ~/ 0;");
    assert_eq!(code_names(&rep), ["const_eval_throws_idbze"]);
    assert!(rep.diagnostics[0].context.is_none());
}

/// `is` is not on the spec's constant-expression list (the site
/// default reports); `as` is, and its unknown result defers —
/// `(1 as int) + 1` stays clean on the const side. Both lines
/// additionally carry the provably-check family's rows, which the
/// oracle co-fires: `1 is int` is constant-true
/// (`unnecessary_type_check`), `1 as int` a dead cast
/// (`unnecessary_cast`).
#[test]
fn is_reports_and_as_defers() {
    let rep = report("class int {} const x = 1 is int;");
    let mut names = code_names(&rep);
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "const_initialized_with_non_constant_value",
            "unnecessary_type_check"
        ],
        "{:?}",
        code_names(&rep)
    );
    let rep = report("class int {} const y = 1 as int; const z = (1 as int) + 1;");
    // The typed paren group routes the `+` against the shadowing user
    // `int` (no operator+ declared): `undefined_operator` rides beside
    // the dead casts. PENDING ORACLE ADJUDICATION — the pre-paren-fold
    // pin carried the two casts only (the opaque group silenced the
    // resolver face); whether the const context suppresses it too is
    // the morning differential rerun's call.
    let mut names = code_names(&rep);
    names.sort_unstable();
    assert_eq!(
        names,
        ["undefined_operator", "unnecessary_cast", "unnecessary_cast",],
        "{:?}",
        code_names(&rep)
    );
}

// —— #747: the non-const duplicate faces (the
// `_checkForDuplications` sites) ———————————————————————————————————

/// A duplicate element (map: key) in a NON-const literal reports at
/// the later part's own span with the verbatim texts and correction;
/// map values never participate, and equal-in-type-but-not-value
/// parts stay silent.
#[test]
fn equal_duplicates_report_at_the_later_part() {
    let rep = report("var s = {1, 1};");
    assert_eq!(code_names(&rep), ["equal_elements_in_set"]);
    let d = &rep.diagnostics[0];
    assert_eq!(
        d.message,
        "Two elements in a set literal shouldn't be equal."
    );
    assert_eq!(
        d.correction.as_deref(),
        Some("Change or remove the duplicate element.")
    );
    assert_eq!(d.range.offset, u32::try_from("var s = {1, ".len()).unwrap());
    assert_eq!(d.range.length, 1);

    let rep = report("var m = {'a': 1, 'a': 2};");
    assert_eq!(code_names(&rep), ["equal_keys_in_map"]);
    let d = &rep.diagnostics[0];
    assert_eq!(d.message, "Two keys in a map literal shouldn't be equal.");
    assert_eq!(
        d.correction.as_deref(),
        Some("Change or remove the duplicate key.")
    );
    assert_eq!(
        d.range.offset,
        u32::try_from("var m = {'a': 1, ".len()).unwrap()
    );
    assert_eq!(d.range.length, 3);

    // Distinct keys with equal values: values are not participants.
    let rep = report("var m = {'a': 1, 'b': 1};");
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));

    // Ints and doubles never unify (`{1, 1.0}` is silent).
    let rep = report("var s = {1, 1.0};");
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
}

/// The silent faces: an element whose constant evaluation fails never
/// reports. Const literals early-return here — their duplicate face is
/// the ERROR-tier `equal_elements_in_const_set` row.
#[test]
fn non_const_duplicate_silences() {
    // A non-constant variable's evaluation fails — silent.
    let rep = report("var x = 0; var s = {x, x};");
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    // `const` literal — carries the ERROR-tier row for its duplicate.
    let rep = report("const s = {1, 1};");
    assert_eq!(
        code_names(&rep),
        ["equal_elements_in_const_set"],
        "{:?}",
        code_names(&rep)
    );
    // An implicitly const literal inside a const context.
    let rep = report("const c = [{1, 1}];");
    assert_eq!(
        code_names(&rep),
        ["equal_elements_in_const_set"],
        "{:?}",
        code_names(&rep)
    );
}

/// Constant-evaluation equality, not syntactic identity: distinct
/// spellings of the same constant report; the oracle's `null` value
/// participates (its `value != null` guard is the failed-evaluation
/// wrapper, not a value exclusion).
#[test]
fn const_eval_equal_elements_report() {
    // The official test_constant_constant shape.
    let rep = report("const a = 1; const b = 1; var s = {a, b};");
    assert_eq!(code_names(&rep), ["equal_elements_in_set"]);
    assert_eq!(
        rep.diagnostics[0].range.offset,
        u32::try_from("const a = 1; const b = 1; var s = {a, ".len()).unwrap()
    );
    let rep = report("var s = {null, null};");
    assert_eq!(code_names(&rep), ["equal_elements_in_set"]);
}

/// Spread/if/for elements are structurally excluded in the oracle
/// (`whereType<Expression>`); their folded operands cannot be told
/// apart here, so a literal holding any skips the check wholesale
/// (under-report, the capture's deferral posture). A nested non-const
/// set literal is its own literal: its own window runs its own check.
/// The skipping itself stays silent, but the target variable `s` is
/// never read, so the oracle's `unused_local_variable` row is the only
/// report (probed: dart analyze 3.13.1, one row on `s`; the spread- or
/// condition-read `l`/`b` never report).
#[test]
fn control_flow_elements_skip_and_nested_literals_own_their_check() {
    // Two equal const list spreads: the dup check is silent (each
    // spread excluded); `s` is unread.
    let rep = report("void m() { const l = [1]; var s = {...l, ...l}; }");
    assert_eq!(
        code_names(&rep),
        ["unused_local_variable"],
        "{:?}",
        code_names(&rep)
    );
    assert_eq!(
        rep.diagnostics[0].message,
        "The value of the local variable 's' isn't used."
    );
    // An if-element plus a duplicate plain element: the dup check is
    // silent; `s` is unread.
    let rep = report("void m() { const b = true; var s = {if (b) 1, 1}; }");
    assert_eq!(
        code_names(&rep),
        ["unused_local_variable"],
        "{:?}",
        code_names(&rep)
    );
    // Nested non-const collections are foreign elements — the dup
    // check skips them, and the inner set reports as its own literal.
    let rep = report("var s = {{1, 1}, 2};");
    assert_eq!(code_names(&rep), ["equal_elements_in_set"]);
    assert_eq!(
        rep.diagnostics[0].range.offset,
        u32::try_from("var s = {{1, ".len()).unwrap()
    );
    let rep = report("var s = {[1], [1]};");
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
}

/// A clean const variable's type folds from its initializer: `const
/// z = 1;` gives `z` static type `int` (the official inference), so a
/// String host rejects the read while an int host stays silent.
#[test]
fn const_variable_type_folds_per_the_official_rules() {
    let rep = report(
        "class int {} class String {} const z = 1; \
             void m() { int ok = z; String bad = z; }",
    );
    // `ok`/`bad` are never read (the official read-only use rule);
    // `bad`'s initializer reports the assignment.
    assert_eq!(
        code_names(&rep),
        [
            "unused_local_variable",
            "unused_local_variable",
            "invalid_assignment",
        ],
        "{:?}",
        code_names(&rep)
    );
    assert_eq!(
        rep.diagnostics[2].message,
        "A value of type 'int' can't be assigned to a variable of type 'String'."
    );
    // Enum constants fold to their container too (the negative face
    // against a user class defers — outside the final-primitive
    // table); `ok` is never read.
    let rep = report("enum E { a, b } void m() { E ok = E.a; }");
    assert_eq!(code_names(&rep), ["unused_local_variable"]);
}

/// Deferral shapes never fabricate a verdict: interpolation text,
/// string repetition, index expressions, member invocations — all
/// silent here (documented under-report, see `const_eval` module
/// docs).
#[test]
fn deferred_shapes_stay_silent() {
    let rep = report(
        "class int {} const n = 3; const a = 'x${n}'; const b = 'ab' * 2; \
             class C { const C(); } const c = C(); \
             const d = 'a$b'; const w = c;",
    );
    // `d` interpolates an unknown constant (`b` folds deferred) —
    // clean; `w` reads a deferred creation — clean.
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
}

// —— invalid_override (#453) ——————————————————————————————————————

/// A `covariant` INHERITED parameter flips the parameter face
/// covariant: the override may narrow it to any subtype of the
/// inherited type (`covariant Parser other` overridden with
/// `ConstantParser other` — the petitparser `hasEqualProperties`
/// shape; the live oracle reports no `invalid_override` on the
/// package). Narrowing beyond the inherited type still breaks.
#[test]
fn covariant_inherited_parameter_allows_narrowing() {
    let rep = report(
        "class Parser {} class Const extends Parser {} \
             class A { void f(covariant Parser x) {} } \
             class B extends A { void f(Const x) {} }",
    );
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    // Narrowing past the inherited type is still the broken face.
    let rep = report(
        "class Parser {} class Other {} \
             class A { void f(covariant Parser x) {} } \
             class B extends A { void f(Other x) {} }",
    );
    assert_eq!(
        code_names(&rep),
        ["invalid_override"],
        "{:?}",
        code_names(&rep)
    );
}

/// An equal-kind method whose positional arity differs from the
/// inherited member emits `invalid_override` at the declaring member's
/// name span.
///
/// Hand-trace: `B.f` (1 positional) vs the nearest `f` on the `A` face
/// (0 positional) — the arity arm decides before any type question.
#[test]
fn arity_mismatch_emits_invalid_override() {
    let rep = report("class int {} class A { void f() {} } class B extends A { void f(int x) {} }");
    assert_eq!(
        code_names(&rep),
        ["invalid_override"],
        "{:?}",
        code_names(&rep)
    );
    assert_eq!(
        rep.diagnostics[0].message,
        "'B.f' ('void Function(int)') isn't a valid override of 'A.f' ('void Function()')."
    );
}

/// An equal-arity method whose named-parameter set narrows the
/// inherited face emits: dropping an inherited named parameter, or
/// adding one as `required`, rejects calls the super accepts. An added
/// optional named parameter is the valid widening and stays silent.
#[test]
fn named_set_mismatch_emits_invalid_override() {
    let rep = report(
        "class int {} class A { void f(int x) {} } \
             class B extends A { void f(int x, {required int y}) {} }",
    );
    assert_eq!(
        code_names(&rep),
        ["invalid_override"],
        "{:?}",
        code_names(&rep)
    );
    assert_eq!(
        rep.diagnostics[0].message,
        "'B.f' ('void Function(int, {required int y})') isn't a valid override of 'A.f' ('void Function(int)')."
    );
    // The widening arm: `{int? y}` added accepts every call the super
    // accepts — no report (oracle-verified).
    let rep = report(
        "class int {} class A { void f(int x) {} } \
             class B extends A { void f(int x, {int? y}) {} }",
    );
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
}

/// The type arm emits only when nullability decides: a nullable getter
/// result over a non-nullable one is a decidable violation; two
/// distinct non-nullable named types are incomparable at the name
/// level (`assignable_from` answers `None`) — suppress, the
/// documented under-report direction.
#[test]
fn type_verdict_follows_assignable_from() {
    let rep =
        report("class int {} class A { int get f => 1; } class B extends A { int? get f => 2; }");
    assert_eq!(
        code_names(&rep),
        ["invalid_override"],
        "{:?}",
        code_names(&rep)
    );
    assert_eq!(
        rep.diagnostics[0].message,
        "'B.f' ('int? Function()') isn't a valid override of 'A.f' ('int Function()')."
    );
    let rep = report(
        "class int {} class String {} class A { String f() => 'x'; } \
             class B extends A { int f() => 1; }",
    );
    // The result-covariance face runs the full M1 assignability: two
    // final primitives are decidable (`int` is no subtype of `String`),
    // so this reports — the oracle's verdict — instead of suppressing
    // the way the pool-less `Ty::assignable_from` did.
    assert_eq!(
        code_names(&rep),
        ["invalid_override"],
        "{:?}",
        code_names(&rep)
    );
}

/// The primitive-pair routing: `int` narrows a `num` result validly
/// (the final-primitive edge proves the covariance), the reverse
/// widening breaks it (`num` is no subtype of `int`). Both directions
/// are decided by the assignability face before the name lattice is
/// asked.
#[test]
fn primitive_return_narrowing_decides_both_ways() {
    let rep = report(
        "class int {} class num {} class A { num f() => 1; } \
             class B extends A { int f() => 2; }",
    );
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    let rep = report(
        "class int {} class num {} class A { int f() => 1; } \
             class B extends A { num f() => 2; }",
    );
    assert_eq!(
        code_names(&rep),
        ["invalid_override"],
        "{:?}",
        code_names(&rep)
    );
}

/// The declared-edge routing on the result face: a subclass return
/// type narrows the inherited member validly (the lattice proves
/// `B <: A`), the reverse widening breaks it (`A` is no subtype of
/// `B` — the walk closes over the unit's declarations without a hit).
/// Both pairs were `assignable`'s `None` before the lattice routing.
#[test]
fn declared_subtype_return_decides_both_ways() {
    let rep = report(
        "class A {} class B extends A {} class S { A f() => 1; } \
             class C extends S { B f() => 2; }",
    );
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    let rep = report(
        "class A {} class B extends A {} class S { B f() => 1; } \
             class C extends S { A f() => 2; }",
    );
    assert_eq!(
        code_names(&rep),
        ["invalid_override"],
        "{:?}",
        code_names(&rep)
    );
}

/// The implicit `Object` edge: an in-file `class B {}` subtypes
/// `Object` with no written clause, so narrowing an `Object`-typed
/// member's result is the legal override and stays silent (pre-fix the
/// lattice closed its walk past the missing edge and answered decisive
/// incomparable — a false positive against the oracle).
#[test]
fn implicit_object_result_narrowing_stays_silent() {
    let rep =
        report("class Object {} class A { Object m() => 1; } class B extends A { B m() => 2; }");
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
}

/// The parameter face rides the same edge: widening an inherited
/// `A`-typed parameter to `Object` accepts every call the super
/// accepts — the contravariant arm's legal direction, no report.
#[test]
fn implicit_object_parameter_widening_stays_silent() {
    let rep = report(
        "class Object {} class A {} class S { void f(A x) {} } \
             class C extends S { void f(Object x) {} }",
    );
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
}

/// The gate answers on the `Object` supertype only: with `Object`
/// declared in-file, a genuinely incomparable pair still closes its
/// walk decisive — the widening direction keeps reporting.
#[test]
fn incomparable_pair_with_object_declared_still_reports() {
    let rep = report(
        "class Object {} class A {} class B {} class S { A f() => 1; } \
             class C extends S { B f() => 2; }",
    );
    assert_eq!(
        code_names(&rep),
        ["invalid_override"],
        "{:?}",
        code_names(&rep)
    );
}

/// The mixin-conflict face decides on the same verdict: an inherited
/// concrete `num` implementation over the interface map's `int` member
/// is a decided break, while the narrowing direction (`int` over
/// `num`) is the valid override and stays silent — the pair
/// linearizes, so no conflict row is owed.
#[test]
fn mixin_implementation_face_decides_int_vs_num() {
    let rep = report(
        "class int {} class num {} mixin M { int f() => 1; } \
             abstract class I { num f(); } class C with M implements I {}",
    );
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    let rep = report(
        "class int {} class num {} mixin M { num f() => 1; } \
             abstract class I { int f(); } class C with M implements I {}",
    );
    assert_eq!(
        code_names(&rep),
        ["invalid_implementation_override"],
        "{:?}",
        code_names(&rep)
    );
}

/// The parameter face's contravariance along a declared edge: the
/// override may widen a parameter up the lattice (`A` accepts every
/// `B` the super passes), and narrowing it down breaks the override —
/// the function-subtype rule's parameter arm
/// (upstream Dart SDK source,
/// `pkg/analyzer/lib/src/dart/element/subtype.dart:117`), encoded once
/// in the verdict face.
#[test]
fn parameter_widening_along_declared_edge_decides() {
    let rep = report(
        "class A {} class B extends A {} class S { void f(B x) {} } \
             class C extends S { void f(A x) {} }",
    );
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    let rep = report(
        "class A {} class B extends A {} class S { void f(A x) {} } \
             class C extends S { void f(B x) {} }",
    );
    assert_eq!(
        code_names(&rep),
        ["invalid_override"],
        "{:?}",
        code_names(&rep)
    );
}

/// The lattice-undecided pair still suppresses: an unresolvable head
/// (written, never declared) cuts the walk — its own `undefined_class`
/// owns the report and the override verdict stays silent, the
/// conservative direction the oracle keeps for `InvalidType` faces.
#[test]
fn unresolvable_head_stays_suppressed() {
    let rep = report(
        "class A { Z f() => 1; } class B extends A { int f() => 2; } \
             class int {}",
    );
    assert_eq!(
        code_names(&rep),
        ["undefined_class"],
        "{:?}",
        code_names(&rep)
    );
}

/// Static members take no part in overriding: a static inherited
/// member is invisible to the face walk, and a static declaration is
/// skipped before any comparison.
#[test]
fn static_members_are_skipped() {
    let rep = report(
        "class int {} class A { static void f() {} } class B extends A { void f(int x) {} }",
    );
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    let rep = report(
        "class int {} class A { void f() {} } class B extends A { static void f(int x) {} }",
    );
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
}

/// An unresolvable superclass suppresses the whole override check for
/// its dependents — every member walk from `B` chain-breaks, so no
/// `invalid_override`-family row fires. The header's own unresolved
/// name reports the extends clause face (`extends_non_class`): the
/// oracle's `_resolveType` clause switch codes the class-declaration
/// extends miss `EXTENDS_NON_CLASS` when no with clause stands
/// (upstream Dart SDK source, pkg/analyzer/lib/src/dart/resolver/
/// scope_analyzer.dart:1216-1220 — oracle probe: `class B extends
/// Missing` emits EXTENDS_NON_CLASS at the name, `undefined_class`
/// does not survive the clause context).
#[test]
fn unresolved_superclass_suppresses() {
    let rep = report("class B extends Missing { void f(int x) {} } class int {}");
    assert_eq!(
        code_names(&rep),
        ["extends_non_class"],
        "{:?}",
        code_names(&rep)
    );
}

/// A mixin's `on` constraints are super-faces for its own members: a
/// mixin member clashing with an `on`-type member emits.
#[test]
fn mixin_on_constraint_is_a_super_face() {
    let rep = report("class int {} class A { void f(int x) {} } mixin M on A { void f() {} }");
    assert_eq!(
        code_names(&rep),
        ["invalid_override"],
        "{:?}",
        code_names(&rep)
    );
    assert_eq!(
        rep.diagnostics[0].message,
        "'M.f' ('void Function()') isn't a valid override of 'A.f' ('void Function(int)')."
    );
}

/// A field's face is its declared type: a nullable field type over a
/// non-nullable inherited field emits (the `FieldDeclaration` hook).
#[test]
fn field_over_field_decidable_nullability_emits() {
    let rep = report("class int {} class A { int x = 1; } class B extends A { int? x = 2; }");
    assert_eq!(
        code_names(&rep),
        ["invalid_override"],
        "{:?}",
        code_names(&rep)
    );
    assert_eq!(
        rep.diagnostics[0].message,
        "'B.x' ('int? Function()') isn't a valid override of 'A.x' ('int Function()')."
    );
}

/// The setter variant keeps the `invalid_override` code name and
/// swaps the message text; a decidable contravariant parameter
/// conflict (non-nullable declared setter parameter over a nullable
/// inherited one) emits.
#[test]
fn setter_variant_keeps_the_code_name() {
    let rep =
        report("class int {} class A { set f(int? x) {} } class B extends A { set f(int y) {} }");
    assert_eq!(
        code_names(&rep),
        ["invalid_override"],
        "{:?}",
        code_names(&rep)
    );
    assert_eq!(
        rep.diagnostics[0].message,
        "The setter 'B.f' ('void Function(int)') isn't a valid override of 'A.f' ('void Function(int?)')."
    );
}

// —— #314: the annotation-driven override residue ————————————————

/// `@override` on a member no superinterface carries reports
/// `override_on_non_overriding_member` at the member's name; the same
/// annotation on a genuine override stays silent.
#[test]
fn override_annotation_on_non_member_reports() {
    let rep =
        report("class int {} class A { void f() {} } class B extends A { @override void g() {} }");
    assert_eq!(
        code_names(&rep),
        ["override_on_non_overriding_member"],
        "{:?}",
        code_names(&rep)
    );
    assert_eq!(
        rep.diagnostics[0].message,
        "The method doesn't override an inherited method."
    );
    let spare =
        report("class int {} class A { void f() {} } class B extends A { @override void f() {} }");
    assert!(code_names(&spare).is_empty(), "{:?}", code_names(&spare));
}

/// The field face spares on either half: an inherited field (getter
/// name) spares, an unrelated name reports the field text; the setter
/// face swaps the message text.
#[test]
fn override_annotation_field_and_setter_faces() {
    let spare =
        report("class int {} class A { int x = 1; } class B extends A { @override int x = 2; }");
    assert!(code_names(&spare).is_empty(), "{:?}", code_names(&spare));
    let rep =
        report("class int {} class A { void f() {} } class B extends A { @override int y = 2; }");
    assert_eq!(
        code_names(&rep),
        ["override_on_non_overriding_member"],
        "{:?}",
        code_names(&rep)
    );
    assert_eq!(
        rep.diagnostics[0].message,
        "The field doesn't override an inherited getter or setter."
    );
    let setter = report(
        "class int {} class A { void f() {} } class B extends A { @override set s(int v) {} }",
    );
    assert_eq!(
        code_names(&setter),
        ["override_on_non_overriding_member"],
        "{:?}",
        code_names(&setter)
    );
    assert_eq!(
        setter.diagnostics[0].message,
        "The setter doesn't override an inherited setter."
    );
}

/// Overriding a `@mustCallSuper`-annotated member without touching
/// `super` reports `must_call_super` at the overriding member's name
/// (a static warning); the body's `super.f()` invocation spares it.
#[test]
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn must_call_super_requires_the_super_invocation() {
    let source = "class int {} class mustCallSuper { const mustCallSuper(); } \
             class A { @mustCallSuper void f() {} } \
             class B extends A { void f() {} }";
    let rep = report(source);
    assert_eq!(
        code_names(&rep),
        ["must_call_super"],
        "{:?}",
        code_names(&rep)
    );
    let d = &rep.diagnostics[0];
    assert_eq!(d.severity, Severity::Warning);
    assert_eq!(
        d.message,
        "This method overrides a method annotated as '@mustCallSuper' in 'A', but doesn't invoke the overridden method."
    );
    // The name-token anchor (the oracle's `node.name`).
    assert_eq!(
        d.range.offset,
        offset_of(source, "class B extends A { void ") + "class B extends A { void ".len() as u32
    );
    let spared = report(
        "class int {} class mustCallSuper { const mustCallSuper(); } \
             class A { @mustCallSuper void f() {} } \
             class B extends A { void f() { super.f(); } }",
    );
    assert!(code_names(&spared).is_empty(), "{:?}", code_names(&spared));
}

/// The `@mustBeOverridden` walk: a concrete subclass without the member
/// reports the missing-override row at the class name; a concrete
/// override spares it.
#[test]
fn must_be_overridden_requires_a_concrete_member() {
    let rep = report(
        "class int {} class mustBeOverridden { const mustBeOverridden(); } \
             class A { @mustBeOverridden void f() {} } \
             class B extends A {}",
    );
    assert_eq!(
        code_names(&rep),
        ["missing_override_of_must_be_overridden"],
        "{:?}",
        code_names(&rep)
    );
    assert_eq!(rep.diagnostics[0].severity, Severity::Warning);
    assert_eq!(
        rep.diagnostics[0].message,
        "Missing a required override of 'f'."
    );
    let spared = report(
        "class int {} class mustBeOverridden { const mustBeOverridden(); } \
             class A { @mustBeOverridden void f() {} } \
             class B extends A { void f() {} }",
    );
    assert!(code_names(&spared).is_empty(), "{:?}", code_names(&spared));
}

/// The concrete-implementation face across the two inherited shapes: the
/// mixin application (`C with M implements I`) and the superclass pair
/// (`C extends A implements I`) report at the applying class's name
/// when the inherited concrete signature does not satisfy the more
/// specific interface member. The conflict axis is nullability — the
/// model's `Ty` decides it both ways (`int?` accepts `int` makes `I.f`
/// the unique most-specific member; `int` rejects `int?` breaks the
/// concrete face) — two distinct named types stay undecidable at the
/// name level (`assignable_from` answers `None`, the suppression
/// direction). An own declaration never double-reports (the per-member
/// `invalid_override` pass owns that face).
#[test]
fn mixin_application_conflict_emits_invalid_implementation_override() {
    let rep = report(
        "class int {} class I { void f(int? x) {} } \
             mixin M { void f(int x) {} } \
             class C with M implements I {}",
    );
    assert_eq!(
        code_names(&rep),
        ["invalid_implementation_override"],
        "{:?}",
        code_names(&rep)
    );
    let d = &rep.diagnostics[0];
    assert_eq!(d.severity, Severity::Error);
    assert_eq!(
        d.message,
        "'M.f' ('void Function(int)') isn't a valid concrete implementation of 'I.f' ('void Function(int?)')."
    );
    let canonical = report(
        "class int {} class A { void f(int x) {} } \
             class I { void f(int? x) {} } \
             class C extends A implements I {}",
    );
    assert_eq!(
        code_names(&canonical),
        ["invalid_implementation_override"],
        "{:?}",
        code_names(&canonical)
    );
    assert_eq!(
        canonical.diagnostics[0].message,
        "'A.f' ('void Function(int)') isn't a valid concrete implementation of 'I.f' ('void Function(int?)')."
    );
}

/// `invalid_override` reaches through every super-face kind: the
/// `implements` path (an interface member against an own declaration)
/// and the applied-mixin path emit like the superclass one.
#[test]
fn invalid_override_hits_the_implements_and_mixin_paths() {
    let rep =
        report("class int {} class A { void f() {} } class B implements A { void f(int x) {} }");
    assert_eq!(
        code_names(&rep),
        ["invalid_override"],
        "{:?}",
        code_names(&rep)
    );
    assert_eq!(
        rep.diagnostics[0].message,
        "'B.f' ('void Function(int)') isn't a valid override of 'A.f' ('void Function()')."
    );
    let applied = report(
        "class int {} class A { void f() {} } mixin M { void f(int x) {} } \
             class C extends A with M { void f() {} }",
    );
    // Oracle-verified (#2367): the live analyzer reports both faces — the
    // mixin-application verdict `M.f` vs `A.f` at `with M`, and the member
    // verdict `C.f` vs `M.f` at the declaration.
    assert_eq!(
        code_names(&applied),
        ["invalid_override", "invalid_override"],
        "{:?}",
        code_names(&applied)
    );
    assert!(
        applied
            .diagnostics
            .iter()
            .any(|d| d.message.contains("'M.f' ('void Function(int)') isn't a valid override of 'A.f'")),
        "{:?}",
        applied.diagnostics
    );
    assert!(
        applied
            .diagnostics
            .iter()
            .any(|d| d.message.contains("'C.f' ('void Function()') isn't a valid override of 'M.f'")),
        "{:?}",
        applied.diagnostics
    );
}

/// The residency widening pays off: a superclass in another resident
/// library (same package, two files) resolves through the namespace
/// hop, so the override verdict fires across the file seam.
#[test]
fn cross_file_superclass_emits() {
    let (rep, _) = cross_file_override_report();
    assert_eq!(
        code_names(&rep),
        ["invalid_override"],
        "{:?}",
        code_names(&rep)
    );
    assert_eq!(
        rep.diagnostics[0].message,
        "'B.f' ('void Function(int)') isn't a valid override of 'A.f' ('void Function()')."
    );
}

/// The two-file override fixture: `A` and its member in `a.dart`, the
/// signature-breaking `B` in `main.dart` (the namespace hop carries the
/// superclass edge). Returns the report and `a.dart`'s file id — the
/// context row's cross-file anchor target.
fn cross_file_override_report() -> (FileReport, FileId) {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file_a = files.intern(Path::new("a.dart"));
    let file_m = files.intern(Path::new("main.dart"));
    let source_a = "class A { void f() {} }";
    let source_m = "import 'a.dart'; class int {} class B extends A { void f(int x) {} }";
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let scanned_a = dartq_scanner::scan(
        source_a,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file_a,
    );
    let scanned_m = dartq_scanner::scan(
        source_m,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file_m,
    );
    let elements_a = collect(source_a, &scanned_a.tokens, features, &strings);
    let elements_m = collect(source_m, &scanned_m.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib_a = builder.intern("package:t/a.dart");
    let lib_m = builder.intern("package:t/main.dart");
    let mut index = ElementIndex::default();
    index.insert(file_a, elements_a.clone());
    index.insert(file_m, elements_m.clone());
    for (name, _) in elements_a.declarations() {
        builder.declare(lib_a, name, file_a);
    }
    for (name, decl) in elements_m.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_m, name, file_m);
    }
    builder.import(
        lib_m,
        ImportEdge {
            target: Some(lib_a),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let rep = diagnose(
        source_m,
        &scanned_m.tokens,
        features,
        &strings,
        file_m,
        lib_m,
        &graph,
        &namespaces,
        &index,
    );
    (rep, file_a)
}

/// A record type declared in another unit re-interns into the
/// diagnosing pool: the foreign shape slot indexes the owner's pool, so
/// reading it through the diagnosing pool overran (a panic on the real
/// `archive`/`analyzer`/`drift`/`riverpod` corpora) or silently named a
/// different shape. The projected record's fields resolve.
#[test]
fn cross_file_record_type_projects_into_the_diagnosing_pool() {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file_a = files.intern(Path::new("a.dart"));
    let file_m = files.intern(Path::new("main.dart"));
    // `a.dart` interns two record shapes, so its second slot (1) is
    // past the empty diagnosing pool's only slot.
    let source_a = "(int, int) pair() => (1, 2);\n({int x, int y}) point() => (x: 1, y: 2);";
    let source_m = "import 'a.dart'; void f() { var p = point(); p.x; p.y; pair().$1; }";
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let config = ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let scanned_a = dartq_scanner::scan(source_a, config, file_a);
    let scanned_m = dartq_scanner::scan(source_m, config, file_m);
    let elements_a = collect(source_a, &scanned_a.tokens, features, &strings);
    let elements_m = collect(source_m, &scanned_m.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib_a = builder.intern("package:t/a.dart");
    let lib_m = builder.intern("package:t/main.dart");
    let mut index = ElementIndex::default();
    index.insert(file_a, elements_a.clone());
    index.insert(file_m, elements_m.clone());
    for (name, _) in elements_a.declarations() {
        builder.declare(lib_a, name, file_a);
    }
    for (name, decl) in elements_m.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_m, name, file_m);
    }
    builder.import(
        lib_m,
        ImportEdge {
            target: Some(lib_a),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let rep = diagnose(
        source_m,
        &scanned_m.tokens,
        features,
        &strings,
        file_m,
        lib_m,
        &graph,
        &namespaces,
        &index,
    );
    assert!(
        !code_names(&rep).contains(&"undefined_getter"),
        "{:?}",
        code_names(&rep)
    );
}

/// The W2 anchor face: the cross-file `invalid_override` row carries the
/// oracle's context message anchored at the overridden member's name
/// token in its own file (`diagnostic_factory.dart` `invalidOverride` —
/// the super fragment's `nameOffset`, `filePath` = the defining source).
#[test]
fn cross_file_override_carries_the_overridden_member_context() {
    let (rep, file_a) = cross_file_override_report();
    assert_eq!(code_names(&rep), ["invalid_override"]);
    let context = rep.diagnostics[0]
        .context
        .as_ref()
        .expect("the overridden-member context row");
    assert_eq!(context.len(), 1);
    assert_eq!(&*context[0].message, "The member being overridden.");
    assert_eq!(context[0].file, file_a);
    // `f`'s name token in a.dart: `class A { void ` precedes it.
    assert_eq!(
        context[0].range.offset,
        u32::try_from("class A { void ".len()).unwrap()
    );
    assert_eq!(context[0].range.length, 1);
}

/// The setter variant swaps the context text, keeping the anchor face
/// (`diagnostic_factory.dart` — "The setter being overridden." at the
/// same super-member fragment).
#[test]
fn setter_override_carries_the_setter_being_overridden_context() {
    let rep =
        report("class int {} class A { set f(int? x) {} } class B extends A { set f(int y) {} }");
    assert_eq!(code_names(&rep), ["invalid_override"]);
    let context = rep.diagnostics[0]
        .context
        .as_ref()
        .expect("the overridden-setter context row");
    assert_eq!(context.len(), 1);
    assert_eq!(&*context[0].message, "The setter being overridden.");
    assert_eq!(context[0].file, rep.diagnostics[0].file);
    // `A`'s setter `f`: `class int {} class A { set ` precedes it.
    assert_eq!(
        context[0].range.offset,
        u32::try_from("class int {} class A { set ".len()).unwrap()
    );
    assert_eq!(context[0].range.length, 1);
}

/// The degrade face: a member whose anchor never shipped (a payload
/// restored before the name-span column existed) yields no context rows —
/// the diagnostic never drops, the anchor never guesses.
#[test]
fn overridden_member_context_degrades_on_a_missing_span() {
    let mut files = FileTable::new();
    let file = files.intern(Path::new("a.dart"));
    let entry = MemberEntry {
        kind: MemberKind::Method,
        is_static: false,
        is_abstract: false,
        is_external: false,
        is_final: false,
        type_params: 0,
        type_param_names: Box::new([]),
        var_ty: Ty::UNKNOWN,
        return_ty: Ty::UNKNOWN,
        formals: None,
        annotations: Box::default(),
        name_range: MemberEntry::MISSING_NAME_RANGE,
    };
    assert!(overridden_member_context(&entry, file, false).is_empty());
    assert!(overridden_member_context(&entry, file, true).is_empty());
    // The populated half, for the same-test contrast: the anchor rides
    // verbatim with the variant's message text.
    let anchored = MemberEntry {
        name_range: SourceRange {
            offset: 7,
            length: 3,
        },
        ..entry.clone()
    };
    let rows = overridden_member_context(&anchored, file, true);
    assert_eq!(rows.len(), 1);
    assert_eq!(&*rows[0].message, "The setter being overridden.");
    assert_eq!(rows[0].file, file);
    assert_eq!(rows[0].range.offset, 7);
    assert_eq!(rows[0].range.length, 3);
}

// —— abstract completeness + interface conflicts (#498) ————————————

/// The five message tiers, with the oracle-verified texts: the list is
/// the sorted `Enclosing.member` descriptions (`getter ` prefixed for
/// getter shapes, plain for methods — the oracle's sort places the
/// unprefixed method first).
///
/// Parse-tier contract (verified against the 2026-10-16 rider log): the
/// body-less `;` members here carry NO `missing_function_body` row —
/// the parse gate mirrors upstream `parseFunctionBody`'s `allowAbstract`
/// (upstream Dart SDK source, pkg/_fe_analyzer_shared/lib/src/parser/
/// parser_impl.dart:6075-6078: `(static == null || external ||
/// augmentations) && inPlainSync`), so a plain instance sync member
/// accepts the `;` body silently, and the semantic crate has no
/// `missing_function_body` emitter of its own. A stray pair of those
/// rows in this report would mean a parser recovery path re-entering
/// the body parse with a rejected `;` — a parser-side regression, not
/// an abstract-class classification face.
#[test]
fn missing_member_tiers_render_the_five_shapes() {
    let rep = report("abstract class A { void m(); } class B extends A {}");
    assert_eq!(
        code_names(&rep),
        ["non_abstract_class_inherits_abstract_member"],
        "{:?}",
        code_names(&rep)
    );
    assert_eq!(
        rep.diagnostics[0].message,
        "Missing concrete implementation of 'A.m'."
    );
    for (count, expected) in [
        (2, "Missing concrete implementations of 'A.a' and 'A.b'."),
        (
            3,
            "Missing concrete implementations of 'A.a', 'A.b', and 'A.c'.",
        ),
        (
            4,
            "Missing concrete implementations of 'A.a', 'A.b', 'A.c', and 'A.d'.",
        ),
        (
            5,
            "Missing concrete implementations of 'A.a', 'A.b', 'A.c', 'A.d', and 1 more.",
        ),
    ] {
        let members = [
            "void a();",
            "void b();",
            "void c();",
            "void d();",
            "void e();",
        ]
        .into_iter()
        .take(count)
        .collect::<Vec<_>>()
        .join(" ");
        let rep = report(&format!(
            "abstract class A {{ {members} }} class B extends A {{}}"
        ));
        assert_eq!(
            code_names(&rep),
            ["non_abstract_class_inherits_abstract_member"],
            "{count}"
        );
        assert_eq!(rep.diagnostics[0].message, expected, "{count}");
    }
    // The description sort is the oracle's: the unprefixed method sorts
    // before the prefixed getter.
    let rep = report("abstract class A { void z(); int get a; } class B extends A {}");
    assert_eq!(
        rep.diagnostics[0].message,
        "Missing concrete implementations of 'A.z' and 'getter A.a'."
    );
}

/// `implements` contributes interface members but never
/// implementations (`_addImplemented`'s sources are the super chain,
/// the mixins, and own members only): a concrete member on an
/// `implements` face still misses; the same member through `extends`
/// resolves.
#[test]
fn implements_never_implements_but_extends_resolves() {
    let rep =
        report("class A { void m() {} } abstract class B { void m(); } class C implements A, B {}");
    assert_eq!(
        code_names(&rep),
        ["non_abstract_class_inherits_abstract_member"],
        "{:?}",
        code_names(&rep)
    );
    // The interface representative is the most specific candidate — the
    // super-side one wins the description.
    let rep = report(
        "class A { void m() {} } abstract class B { void m(); } class C extends B implements A {}",
    );
    assert_eq!(
        rep.diagnostics[0].message,
        "Missing concrete implementation of 'B.m'."
    );
    let rep = report("class A { void m() {} } class C extends A {}");
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
}

/// The concrete-superclass exemption (`_isNotImplementedInConcreteSuper
/// Class`): the issue belongs to the concrete class that already lacks
/// the member. An abstract class exempts itself from the check; its own
/// abstract members are the `concrete_class_with_abstract_member`
/// report's business — not implemented here, so they suppress.
#[test]
fn abstract_and_concrete_super_exemptions() {
    // An abstract class never reports.
    let rep = report("abstract class A { void m(); } abstract class B extends A {}");
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    // A concrete class's own abstract member is the other code's row.
    let rep = report("class C { void m(); }");
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    // A concrete superclass whose interface has the name blocks the
    // report for its subclasses.
    let rep = report("class C { void m(); } class D extends C {}");
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
}

/// A concrete `noSuchMethod` on the implemented sources (the class
/// itself, the super chain, a `with` mixin) gates the whole check — the
/// oracle's forwarder machinery implements every interface name.
#[test]
fn no_such_method_gates_completeness() {
    let rep = report(
        "class Invocation {} abstract class A { void m(); } \
             class B extends A { noSuchMethod(Invocation i) => null; }",
    );
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    let rep = report(
        "class Invocation {} abstract class A { void m(); } \
             class S { noSuchMethod(Invocation i) => super.noSuchMethod(i); } \
             class C extends S implements A {}",
    );
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
}

/// The getter/method kind collision is unresolvable — the class-local
/// declaration does not silence it (oracle-verified; the row anchors at
/// the class name here, the span divergence the module docs carry).
#[test]
fn getter_method_kind_collision_reports_despite_local_member() {
    let rep = report(
        "class int {} abstract class A { int get m; } abstract class B { void m(); } \
             class C implements A, B {} \
             class D implements A, B { int get m => 1; } \
             class E implements A, B { void m() {} }",
    );
    assert_eq!(
        code_names(&rep),
        [
            "inconsistent_inheritance_getter_and_method",
            "inconsistent_inheritance_getter_and_method",
            "inconsistent_inheritance_getter_and_method",
        ],
        "{:?}",
        code_names(&rep)
    );
    assert_eq!(
        rep.diagnostics[0].message,
        "'m' is inherited as a getter (from 'A') and also a method (from 'B')."
    );
}

/// The same-kind signature conflict reports the candidates in face
/// order with the name-level signatures; a class-local member whose
/// signature validly overrides every candidate resolves it (the
/// official `map.containsKey` gate).
#[test]
fn candidates_conflict_and_its_local_resolution() {
    let rep = report(
        "class int {} class A { void m({int? a}) {} } class B { void m({int? b}) {} } \
             class C extends A implements B {}",
    );
    assert_eq!(
        code_names(&rep),
        ["inconsistent_inheritance"],
        "{:?}",
        code_names(&rep)
    );
    assert_eq!(
        rep.diagnostics[0].message,
        "Superinterfaces don't have a valid override for 'm': \
             A.m (void Function({int? a})), B.m (void Function({int? b}))."
    );
    // The resolutions split (oracle-verified): the widened signature
    // validly overrides both candidates, while `E.m` still rejects
    // `B`'s `b:` calls — the per-member `invalid_override` reports
    // (independent of the conflict row `C` carries above).
    let rep = report(
        "class int {} class A { void m({int? a}) {} } class B { void m({int? b}) {} } \
             class D extends A implements B { void m({int? a, int? b}) {} } \
             class E extends A implements B { void m({int? a}) {} }",
    );
    assert_eq!(
        code_names(&rep),
        ["invalid_override"],
        "{:?} {:#?}",
        code_names(&rep),
        rep.diagnostics
    );
    assert_eq!(
        rep.diagnostics[0].message,
        "'E.m' ('void Function({int? a})') isn't a valid override of 'B.m' ('void Function({int? b})')."
    );
}

/// A `with` mixin's own member replaces the accumulated candidates (the
/// official application sequence): a kind mismatch in the replacement
/// is itself the getter/method conflict, a same-kind replacement is
/// clean, and a stacked application whose member validly overrides
/// every side resolves the whole name.
#[test]
fn mixin_replacement_resolves_and_conflicts() {
    // Kind mismatch through the replacement: the getter survived on the
    // superclass face, the mixin's own method replaces it.
    let rep = report(
        "class int {} abstract class A { int get m; } mixin M { void m() {} } class C extends A with M {}",
    );
    assert_eq!(
        code_names(&rep),
        ["inconsistent_inheritance_getter_and_method"],
        "{:?}",
        code_names(&rep)
    );
    assert_eq!(
        rep.diagnostics[0].message,
        "'m' is inherited as a getter (from 'A') and also a method (from 'M')."
    );
    // Same-kind replacement: clean.
    let rep = report(
        "abstract class B { void m(); } mixin M { void m() {} } class C extends B with M {}",
    );
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    // The stacked resolution: the mixin's widened member validly
    // overrides both the replaced super candidate and the interface
    // candidate (oracle-verified — no row).
    let rep = report(
        "class int {} class A { void m({int? a}) {} } class B { void m({int? b}) {} } \
             mixin M { void m({int? a, int? b}) {} } class C extends A with M implements B {}",
    );
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    // Without the mixin's member the conflict surfaces.
    let rep = report(
        "class int {} class A { void m({int? a}) {} } class B { void m({int? b}) {} } \
             mixin N { void other() {} } class C extends A with N implements B {}",
    );
    assert_eq!(code_names(&rep), ["inconsistent_inheritance"]);
}

/// A mixin declaration participates in the conflicts through its `on`
/// constraints (the completeness loop skips mixins entirely).
#[test]
fn mixin_on_clause_conflicts_but_mixins_never_miss() {
    let rep = report(
        "class int {} mixin A { void m({int? a}) {} } mixin B { void m({int? b}) {} } mixin C on A, B {}",
    );
    assert_eq!(
        code_names(&rep),
        ["inconsistent_inheritance"],
        "{:?}",
        code_names(&rep)
    );
    // Mixins take no part in the abstract-completeness loop.
    let rep = report("class int {} mixin M { void m(); }");
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
}

/// Enums participate in the completeness check; an enum's synthesized
/// `values`/`values=` stay exempt (the official exemption arm), while
/// foreign abstract members report.
#[test]
fn enum_participates_with_values_exempt() {
    let rep = report("abstract class A { void a(); void b(); } enum E implements A { v1, v2; }");
    assert_eq!(
        code_names(&rep),
        ["non_abstract_class_inherits_abstract_member"],
        "{:?}",
        code_names(&rep)
    );
    assert_eq!(
        rep.diagnostics[0].message,
        "Missing concrete implementations of 'A.a' and 'A.b'."
    );
    // The synthesized `values` getter satisfies the interface name.
    let rep = report("class int {} abstract class A { int get values; } enum E implements A { v }");
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
}

/// An abstract field's non-final shape misses both halves (getter and
/// setter, the setter displayed without the `=`); a final field misses
/// the getter only. A concrete field implements both.
#[test]
fn abstract_field_halves_split_by_final() {
    let rep = report(
        "class int {} abstract class A { abstract int x; abstract final int y; } class B extends A {}",
    );
    assert_eq!(
        code_names(&rep),
        ["non_abstract_class_inherits_abstract_member"],
        "{:?}",
        code_names(&rep)
    );
    assert_eq!(
        rep.diagnostics[0].message,
        "Missing concrete implementations of 'getter A.x', 'getter A.y', and 'setter A.x'."
    );
    let rep = report("class int {} class A { abstract int x; } class B extends A { int x = 0; }");
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
}

/// A private name inherited from another library can never be
/// implemented — the oracle skips it (`isAccessibleFor`); the public
/// sibling still reports.
#[test]
fn cross_library_private_names_are_inaccessible() {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file_a = files.intern(Path::new("a.dart"));
    let file_m = files.intern(Path::new("main.dart"));
    let source_a = "abstract class A { void _priv(); void pub(); }";
    let source_m = "import 'a.dart'; class B extends A {}";
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let scanned_a = dartq_scanner::scan(
        source_a,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file_a,
    );
    let scanned_m = dartq_scanner::scan(
        source_m,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file_m,
    );
    let elements_a = collect(source_a, &scanned_a.tokens, features, &strings);
    let elements_m = collect(source_m, &scanned_m.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib_a = builder.intern("package:t/a.dart");
    let lib_m = builder.intern("package:t/main.dart");
    let mut index = ElementIndex::default();
    index.insert(file_a, elements_a.clone());
    index.insert(file_m, elements_m.clone());
    for (name, _) in elements_a.declarations() {
        builder.declare(lib_a, name, file_a);
    }
    for (name, decl) in elements_m.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_m, name, file_m);
    }
    builder.import(
        lib_m,
        ImportEdge {
            target: Some(lib_a),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let rep = diagnose(
        source_m,
        &scanned_m.tokens,
        features,
        &strings,
        file_m,
        lib_m,
        &graph,
        &namespaces,
        &index,
    );
    assert_eq!(
        code_names(&rep),
        ["non_abstract_class_inherits_abstract_member"],
        "{:?}",
        code_names(&rep)
    );
    assert_eq!(
        rep.diagnostics[0].message,
        "Missing concrete implementation of 'A.pub'."
    );
}

/// Empty clause lists and faceless declarations are silent; a named
/// mixin application participates like any concrete class.
#[test]
fn empty_clauses_are_silent_and_mixin_applications_participate() {
    let rep = report("class C {} mixin M {} enum E { v }");
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    // The application resolves the member through the mixin.
    let rep = report("abstract class A { void m(); } mixin M { void m() {} } class C = A with M;");
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    // Without the mixin's member the application misses.
    let rep =
        report("abstract class A { void m(); } mixin N { void other() {} } class C = A with N;");
    assert_eq!(
        code_names(&rep),
        ["non_abstract_class_inherits_abstract_member"],
        "{:?}",
        code_names(&rep)
    );
    assert_eq!(
        rep.diagnostics[0].message,
        "Missing concrete implementation of 'A.m'."
    );
    // An abstract named mixin application exempts itself.
    let rep = report(
        "abstract class A { void m(); } mixin N { void other() {} } abstract class C = A with N;",
    );
    assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
}

// —— #449: the flow-analysis mount (dead_code first milestone) ————————

/// UTF-16 offsets of the report's `dead_code` rows.
fn dead_code_offsets(report: &FileReport) -> Vec<u32> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .map(|d| d.range.offset)
        .collect()
}

#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn offset_of(source: &str, needle: &str) -> u32 {
    source.find(needle).expect("needle present") as u32
}

/// A statement after `return` reports `dead_code` (a static warning) at
/// the dead statement's anchor, with the verbatim oracle texts.
#[test]
fn dead_code_after_return_emits() {
    let source = "void f() { return; 1; }";
    let report = report(source);
    assert_eq!(code_names(&report), ["dead_code"]);
    let d = &report.diagnostics[0];
    assert_eq!(d.severity, Severity::Warning);
    assert_eq!(d.message, "Dead code.");
    assert_eq!(
        d.correction.as_deref(),
        Some("Try removing the code, or fixing the code before it so that it can be reached.")
    );
    assert_eq!(dead_code_offsets(&report), [offset_of(source, "1;")]);
}

/// One wide row per dead interval: the row anchors at the first dead
/// statement and its span runs to the covering statement list's last
/// statement's end, trailing `;` included — the official interval
/// model (`dead_code_verifier.dart:253-279` trims the enclosing
/// construct to `node.statements.last` and the report spans
/// `node.end - offset`, :326-332). The flow fact widens to that
/// boundary via `Ev::StmtEnd` (the flow crate's
/// `dead_interval_folds_sequential_statements` twin).
#[test]
fn dead_code_reports_one_row_per_dead_interval() {
    let source = "void f() { return; 1; 2; }";
    let report = report(source);
    assert_eq!(code_names(&report), ["dead_code"]);
    let dead = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "dead_code")
        .expect("dead_code present");
    assert_eq!(dead.range.offset, offset_of(source, "1;"));
    assert_eq!(
        dead.range.length, 5,
        "the row spans `1; 2;` — official `node.end` includes the \
             trailing `;` (dead_code_verifier.dart:330: `node.end - offset`)"
    );
}

/// A dead local declaration reports at the declared name (the fact
/// layer's declaration anchor), not the declaration's head token.
#[test]
fn dead_local_declaration_emits_at_its_name() {
    let source = "void f() { return; var x = 1; }";
    let report = report(source);
    // The dead declaration's `x` is also never read (the official
    // read-only use rule); both rows anchor at `x`, dead_code first.
    assert_eq!(code_names(&report), ["dead_code", "unused_local_variable"]);
    assert_eq!(dead_code_offsets(&report), [offset_of(source, "x")]);
}

/// A closure body's own dead code reports inside the closure; the
/// enclosing function's tail stays reachable — the facts are
/// per-executable, so the closure's `return` cannot kill the outer
/// statements after it.
#[test]
fn closure_return_does_not_kill_the_enclosing_tail() {
    let source = "void f() { var h = () { return; 1; }; 2; }";
    let report = report(source);
    // `h` is never read (the official read-only use rule), before the
    // dead-statement row inside the closure.
    assert_eq!(code_names(&report), ["unused_local_variable", "dead_code"]);
    assert_eq!(dead_code_offsets(&report), [offset_of(source, "1;")]);
}

/// Code after a `return` inside a branch stays live on the other path:
/// no over-report on partially dead control flow.
#[test]
fn partial_dead_branch_does_not_report() {
    let report = report("void f(c) { if (c) { return; } 1; }");
    assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
}

// —— #491 S1: header-window attribution —————————————————————————————

/// The audit #489 pin: `extends A, B` reports the parser's
/// `multiple_extends` (in this report, as in the oracle's CFE face);
/// `B` is residue, so `C.f` never checks against it and no dartq-only
/// `invalid_override` fires (`ast_builder.dart:3972-3982` throws the
/// overflow away).
///
/// Hand-trace: `B` reaches the buffer after `A` latched the superclass;
/// `handle_implements(None, 0)` claims nothing, so `B` lands in
/// `header_residue` — `check_override`'s face list never sees it.
#[test]
fn extends_overflow_is_never_a_super_face() {
    let report = report(
        "class int {} class A {} class B { void f(int x) {} } \
             class C extends A, B { void f() {} }",
    );
    assert_eq!(
        code_names(&report),
        ["multiple_extends_clauses"],
        "{:?}",
        code_names(&report)
    );
}

/// A named mixin application's `implements` names wait for the parser's
/// real `handle_implements(Some, count)` emission (ticket #460, parser
/// lane): until it lands they are residue and the interface edge
/// under-reports. Kept as the cross-lane pin — expected red until #460
/// lands, never worked around here.
#[test]
fn named_mixin_application_implements_capture() {
    let strings = StringTable::new();
    let source = "class S {} mixin M {} class I {} class C = S with M implements I;";
    let file = FileTable::new().intern(Path::new("nma.dart"));
    let scanned = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(scanned.diagnostics.is_empty());
    let elements = collect(
        source,
        &scanned.tokens,
        ParserFeatures::from_feature_set(FeatureSet::for_language_version(LanguageVersion::new(
            3, 15,
        ))),
        &strings,
    );
    let intern = |text: &str| strings.intern(text);
    let c = elements.container(intern("C")).expect("container C");
    assert_eq!(
        c.implements_clause.as_ref(),
        vec![intern("I")],
        "the interface edge must attribute to the implements clause"
    );
}

/// A three-file hop resolves the superclass name in the declaring
/// container's library (the oracle climbs the declaring library's scope
/// chain, not the diagnosing one): `S` lives in n2 and extends n3's `T`;
/// the diagnosing library imports n1's conflicting `T`. The climb must
/// reach n3's `T.m(int)` — arity-broken against `C.m()` — not n1's
/// compatible `T.m()`.
///
/// Hand-trace: hop 1 `S` resolves through main's import face to n2's
/// file; hop 2 probes the library whose locals declare `T` in n2's file —
/// n3's — and resolves its `T` through n3's namespace.
#[test]
fn superclass_hop_resolves_in_the_declaring_library() {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let sources = [
        ("n1.dart", "class T { void m() {} }"),
        ("n3.dart", "class int {} class T { void m(int x) {} }"),
        ("n2.dart", "import 'n3.dart'; class S extends T {}"),
        (
            "main.dart",
            "import 'n1.dart'; import 'n2.dart'; class C extends S { void m() {} }",
        ),
    ];
    let mut index = ElementIndex::default();
    let mut tokens_by_file = Vec::new();
    for (path, source) in sources {
        let file = files.intern(Path::new(path));
        let scanned = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(scanned.diagnostics.is_empty(), "{path}");
        let elements = collect(source, &scanned.tokens, features, &strings);
        index.insert(file, elements);
        tokens_by_file.push(scanned.tokens);
    }
    let mut builder = LibraryGraphBuilder::new(&strings);
    let n1 = builder.intern("package:t/n1.dart");
    let n3 = builder.intern("package:t/n3.dart");
    let n2 = builder.intern("package:t/n2.dart");
    let main = builder.intern("package:t/main.dart");
    for (at, (path, _source)) in sources.iter().enumerate() {
        let lib = match at {
            0 => n1,
            1 => n3,
            2 => n2,
            _ => main,
        };
        let file = files.intern(Path::new(path));
        let elements = collect(sources[at].1, &tokens_by_file[at], features, &strings);
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
    }
    builder.import(
        n2,
        ImportEdge {
            target: Some(n3),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    builder.import(
        main,
        ImportEdge {
            target: Some(n1),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    builder.import(
        main,
        ImportEdge {
            target: Some(n2),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let main_file = files.intern(Path::new("main.dart"));
    let report = diagnose(
        sources[3].1,
        &tokens_by_file[3],
        features,
        &strings,
        main_file,
        main,
        &graph,
        &namespaces,
        &index,
    );
    // The `n1` import contributes nothing `main` references (`T` is
    // only used inside `n2` through its own import): the official
    // `unused_import` fires at its URI, before the override row.
    assert_eq!(
        code_names(&report),
        ["unused_import", "invalid_override"],
        "{:?}",
        code_names(&report)
    );
}

/// One applicable extension defines the access; the same-named member of
/// a *non-applicable* extension does not (the applicability filter —
/// `applicable_extensions.dart:157-171` gates on the receiver type, so
/// an extension `on` another type never suppresses an `undefined_*`).
///
/// Hand-trace: `E.on_type == "int"` never matches the resolved `C`
/// receiver; the flat-pool predecessor suppressed this report.
#[test]
fn inapplicable_extension_member_reports_undefined() {
    let report = report(
        "class int {} class C {} extension E on int { void m() {} } \
             void main() { C().m(); }",
    );
    assert_eq!(
        code_names(&report),
        ["undefined_method"],
        "{:?}",
        code_names(&report)
    );
}

/// A single applicable extension defines the access silently.
#[test]
fn single_applicable_extension_defines_the_access() {
    let report = report(
        "class C {} extension E on C { void m() {} } \
             void main() { C().m(); C().m2; }",
    );
    assert_eq!(
        code_names(&report),
        ["undefined_getter"],
        "{:?}",
        code_names(&report)
    );
}

/// Two applicable extensions both declaring the member report
/// `ambiguous_extension_member_access` once (the two-candidate message
/// text; `diagnostic.g.dart:363-374`) — and the access counts as
/// resolved, so no `undefined_method` stacks on top.
///
/// Hand-trace: both faces carry `on_type == "C"`; `_isMoreSpecific`
/// cannot separate same-`on`-type candidates at the M1 name level.
#[test]
fn two_applicable_extensions_report_the_ambiguity() {
    let report = report(
        "class C {} extension E1 on C { void m() {} } \
             extension E2 on C { void m() {} } \
             void main() { C().m(); }",
    );
    assert_eq!(
        code_names(&report),
        ["ambiguous_extension_member_access"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "A member named 'm' is defined in 'extension E1 on C' and 'extension E2 on C', and neither is more specific."
    );
}

/// Three or more candidates take the joined-list variant with the
/// `commaSeparatedWithAnd` rendering
/// (`utilities/extensions/string.dart:41-104`: Oxford 'and'), including
/// the unnamed-extension display. The unnamed extension's `m` carries
/// its own row: an unnamed extension is never publicly accessible, so
/// its members check like private declarations — and an ambiguous
/// application resolves to no unique member, so nothing credits it
/// (`usedElements.members` joins on a resolved element only).
#[test]
fn three_applicable_extensions_use_the_list_variant() {
    let report = report(
        "class C {} extension E1 on C { void m() {} } \
             extension E2 on C { void m() {} } \
             extension on C { void m() {} } \
             void main() { C().m(); }",
    );
    assert_eq!(
        code_names(&report),
        ["unused_element", "ambiguous_extension_member_access"]
    );
    assert_eq!(
        report.diagnostics[1].message,
        "A member named 'm' is defined in extension 'E1', extension 'E2', and unnamed extension on 'C', and none are more specific."
    );
}

/// The `on` clause's argument list gates beside the head
/// (`applicable_extensions.dart:205-281` `applicableTo`: the oracle
/// instantiates the extended type's parameters against the receiver,
/// so `on Map<K, FieldItem>` never applies to a `Map<K, MethodItem>`
/// receiver). Two faces sharing the `Map` head split on the argument —
/// only the matching one applies, silently; a receiver matching
/// neither reports.
///
/// Hand-trace: `FieldItem` and `MethodItem` are siblings — neither
/// reaches the other, so each `on` face accepts exactly its own
/// argument row.
#[test]
fn on_type_arguments_gate_applicability() {
    let field = report(
        "class Map<K, V> {} class LookupName {} class FieldItem {} \
             class MethodItem {} \
             extension IdExt on Map<LookupName, FieldItem> { void write() {} } \
             extension ItemExt on Map<LookupName, FieldItem> { void write() {} } \
             extension CtorExt on Map<LookupName, MethodItem> { void write() {} } \
             void f(Map<LookupName, FieldItem> x) { x.write(); }",
    );
    assert_eq!(
        code_names(&field),
        ["ambiguous_extension_member_access"],
        "{:?}",
        code_names(&field)
    );
    let method = report(
        "class Map<K, V> {} class LookupName {} class FieldItem {} \
             class MethodItem {} \
             extension IdExt on Map<LookupName, FieldItem> { void write() {} } \
             extension CtorExt on Map<LookupName, MethodItem> { void write() {} } \
             void f(Map<LookupName, MethodItem> x) { x.write(); }",
    );
    assert!(code_names(&method).is_empty(), "{:?}", code_names(&method));
    let neither = report(
        "class Map<K, V> {} class LookupName {} class FieldItem {} \
             class MethodItem {} class SetterItem {} \
             extension IdExt on Map<LookupName, FieldItem> { void write() {} } \
             extension CtorExt on Map<LookupName, MethodItem> { void write() {} } \
             void f(Map<LookupName, SetterItem> x) { x.write(); }",
    );
    assert_eq!(
        code_names(&neither),
        ["undefined_method"],
        "{:?}",
        code_names(&neither)
    );
}

/// An `on` argument spelling one of the extension's own type
/// parameters instantiates against the receiver rather than comparing
/// concretely (`applicableTo` binds the fresh parameter): `extension
/// E<T> on List<T>` applies to `List<Item>`. The negative control: a
/// `T` written on a *parameterless* extension is a concrete (and here
/// undeclared) type name, never an instantiable parameter — `List<int>`
/// does not reach it.
#[test]
fn on_type_parameter_argument_instantiates() {
    let applied = report(
        "class List<T> {} class Item {} \
             extension E<T> on List<T> { void m() {} } \
             void f(List<Item> x) { x.m(); }",
    );
    assert!(
        code_names(&applied).is_empty(),
        "{:?}",
        code_names(&applied)
    );
    // A preceding declaration's own type-variable group must not leak
    // into a parameterless extension's `on` type: `T` below is `class
    // C`'s parameter, not `E`'s — it resolves to nothing and reports
    // `non_type_as_type_argument`. The failed argument slots the `on`
    // type as the oracle's `InvalidType` (`List<InvalidType>`), which
    // `List<int>` still satisfies — the face applies and the access
    // resolves silently.
    let leaked = report(
        "class C<T> {} class List<T> {} \
             extension E on List<T> { void m() {} } \
             void f(List<int> x) { x.m(); }",
    );
    assert_eq!(
        code_names(&leaked),
        ["non_type_as_type_argument", "non_type_as_type_argument"],
        "{:?}",
        code_names(&leaked)
    );
}

/// A face reachable through several routes enters the application
/// pool once (`LibraryFragmentScope.accessibleExtensions`,
/// scope.dart:482-487 — the oracle's list is a set of elements):
/// `main` imports `a.dart` twice, so `E` arrives through both edges'
/// export scopes, and the access resolves silently instead of
/// reporting the same extension twice as ambiguous.
#[test]
fn multiply_imported_extension_applies_once() {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let sources = [
        ("a.dart", "class C {} extension E on C { void m() {} }"),
        (
            "main.dart",
            "import 'a.dart'; import 'a.dart'; void f(C x) { x.m(); }",
        ),
    ];
    let mut index = ElementIndex::default();
    let mut tokens_by_file = Vec::new();
    for (path, source) in sources {
        let file = files.intern(Path::new(path));
        let scanned = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(scanned.diagnostics.is_empty(), "{path}");
        let elements = collect(source, &scanned.tokens, features, &strings);
        index.insert(file, elements);
        tokens_by_file.push(scanned.tokens);
    }
    let mut builder = LibraryGraphBuilder::new(&strings);
    let a = builder.intern("package:t/a.dart");
    let main = builder.intern("package:t/main.dart");
    for (at, (path, source)) in sources.iter().enumerate() {
        let lib = if at == 0 { a } else { main };
        let file = files.intern(Path::new(path));
        let elements = collect(source, &tokens_by_file[at], features, &strings);
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
    }
    for _ in 0..2 {
        builder.import(
            main,
            ImportEdge {
                target: Some(a),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let main_file = files.intern(Path::new("main.dart"));
    let report = diagnose(
        sources[1].1,
        &tokens_by_file[1],
        features,
        &strings,
        main_file,
        main,
        &graph,
        &namespaces,
        &index,
    );
    assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
}

/// The on-type's own member wins: interface members precede extension
/// application (`findExtension` runs only after the interface lookup
/// misses), so no ambiguity fires against an extension on the same type.
#[test]
fn own_member_shadows_the_applicable_extensions() {
    let report = report(
        "class int {} class String {} class C { void m() {} } \
             extension E1 on C { void m(int x) {} } \
             extension E2 on C { void m(String x) {} } \
             void main() { C().m(); }",
    );
    assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
}

/// An unresolved `on` type suppresses the face from application
/// entirely: `extension E on Undefined` never gates an `undefined_*`
/// (the oracle's `extendedType` fails to resolve), while the type
/// reference itself reports.
#[test]
fn unresolved_on_type_suppresses_the_face() {
    let report = report(
        "extension E on Undefined { void m() {} } \
             class C {} void main() { C().m(); }",
    );
    assert_eq!(
        code_names(&report),
        ["undefined_class", "undefined_method"],
        "{:?}",
        code_names(&report)
    );
}

/// A setter-only extension gates a write through the setter spelling
/// (`havingMemberWithBaseName` couples the field's getter/setter pair;
/// `applicable_extensions.dart:104-115`).
#[test]
fn setter_only_extension_defines_the_write() {
    let report = report(
        "class int {} class C {} extension E on C { set s(int v) {} } \
             void main() { C().s = 1; }",
    );
    assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
}

/// The extension index face (#1455 F-F): an implicit-application
/// receiver resolves `x[0]` through an applicable extension's
/// `operator []` and `x[0] = v` / `x[0] += 1` through `operator []=` —
/// read, write, compound, and null-aware shapes stay silent (the
/// oracle's `if_null.dart` extension-index corpus is clean). Negative
/// control: a receiver whose container genuinely lacks the operator
/// still reports `undefined_operator`.
///
/// Hand-trace: the index fold arms `Intent::Operator('[]', bracket)` on
/// the receiver row (the write form for `=`/`+=`); `check_operator_expr`
/// walks the container (`HasProp` declares only `prop`), misses, and
/// consults the extension faces — `on HasProp` declares the spelling,
/// so the access is defined.
#[test]
fn extension_index_operator_applies() {
    let faces = report(
        "class int {} class HasProp { int? prop; } \
             extension Ext<T> on HasProp { \
               T operator [](int i) => prop; \
               operator []=(int i, T v) { prop = v; } \
             } \
             void reads(HasProp x) { x[0]; } \
             void writes(HasProp x) { x[0] = 1; } \
             void compound(HasProp x) { x[0] += 1; } \
             void null_aware(HasProp? x) { x?[0]; }",
    );
    assert!(faces.diagnostics.is_empty(), "{:?}", code_names(&faces));
    let miss = report("class int {} class C {} void f(C c) { c[0]; }");
    assert_eq!(code_names(&miss), ["undefined_operator"]);
}

/// Each index spelling is its own extension lookup: a face declaring
/// only `operator []=` defines the write (`x[0] = 1` stays silent) and
/// never the read (`x[0]` reports `undefined_operator` '[]') — the read
/// lookup keys `[]` exactly, the setter spelling answers write targets
/// only (upstream Dart SDK source,
/// `pkg/analyzer/lib/src/dart/resolver/extension_member_resolver.dart`,
/// the `[]`-operator lookup on the instantiated `on` type).
#[test]
fn index_write_never_satisfies_the_read() {
    let report = report(
        "class int {} class S {} \
             extension SIdx on S { operator []=(int i, int v) {} } \
             void writes(S x) { x[0] = 1; } \
             void reads(S x) { x[0]; }",
    );
    assert_eq!(
        code_names(&report),
        ["undefined_operator"],
        "{:?}",
        code_names(&report)
    );
}

/// Two applicable faces declaring the operator are never separable at
/// the name level (`_isMoreSpecific` answers equal): the ambiguity
/// verdict at the bracket pair replaces the operator family.
#[test]
fn ambiguous_index_extensions_report_the_ambiguity() {
    let report = report(
        "class int {} class A {} \
             extension E1 on A { int operator [](int i) => 0; } \
             extension E2 on A { int operator [](int i) => 0; } \
             void f(A x) { x[0]; }",
    );
    assert_eq!(
        code_names(&report),
        ["ambiguous_extension_member_access"],
        "{:?}",
        code_names(&report)
    );
}

/// Extension-type members are not implicit-application candidates: the
/// face set holds plain extensions only, so an unrelated class's access
/// reports (the old merged pool suppressed it), while the extension
/// type's own receiver still resolves through its container face.
#[test]
fn extension_type_members_stay_out_of_the_faces() {
    let miss = report(
        "class int {} extension type T(int x) { void m() {} } \
             class C {} void main() { C().m(); }",
    );
    assert_eq!(
        code_names(&miss),
        ["undefined_method"],
        "{:?}",
        code_names(&miss)
    );
    let hit = report(
        "class int {} extension type T(int x) { void m() {} } \
             void main() { T(1).m(); }",
    );
    assert!(code_names(&hit).is_empty(), "{:?}", code_names(&hit));
}

/// F6: an extension type's `implements` clause is its instance-member
/// interface — `e.exists()` on `extension type E(Directory d) implements
/// Directory` resolves through `Directory`'s members (oracle: only the
/// absent name reports). A transitive face (`implements Directory` where
/// `Directory` has its own supertype chain) resolves through the climb.
#[test]
fn extension_type_implements_members_resolve() {
    let r = report(
        "class Directory { void exists() {} void close() {} void cancel() {} } \
             extension type E(Directory d) implements Directory {} \
             void f(E e) { e.exists(); e.close(); e.cancel(); e.missing(); }",
    );
    assert_eq!(code_names(&r), ["undefined_method"], "{:?}", code_names(&r));
    // A member absent from every interface reports, but a member on a
    // transitively-implemented face still resolves.
    let r = report(
        "class Base { void exists() {} } \
             class Directory extends Base {} \
             extension type E(Directory d) implements Directory {} \
             void f(E e) { e.exists(); e.missing(); }",
    );
    assert_eq!(code_names(&r), ["undefined_method"], "{:?}", code_names(&r));
}

/// The resolved-prefix constructor face (#2094-c): `io.Directory.new`
/// resolves the prefix's container and its unnamed constructor — both
/// the tear-off and the invocation stay silent; a member absent from
/// the container reports the container family (`undefined_getter` /
/// `undefined_method` "for the type 'Directory'"), not the prefix-miss
/// `undefined_prefixed_name`. The oracle's ast_rewrite keeps the
/// qualified type as the selector's receiver static type (probe:
/// `dart analyze` on `io.Directory.gone` → `UNDEFINED_GETTER` over the
/// selector).
///
/// Hand-trace: `io` pushes `Bare(Prefix)`; the `.Directory` merge folds
/// `Receiver::Prefix`; the `.new` merge pops that member row and
/// reclassifies it through [`prefixed_container`] into
/// `Receiver::PrefixedContainer`, so the selector reads the foreign
/// container's static face (`Receiver::Container` cannot resolve — the
/// name sits behind the prefix, not in this library's scope).
#[test]
fn prefixed_container_members_resolve_through_the_prefix() {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file_a = files.intern(Path::new("a.dart"));
    let file_m = files.intern(Path::new("main.dart"));
    let source_a = "class Directory { Directory(); void exists() {} }";
    let source_m = "import 'a.dart' as io; void f() { io.Directory.new; io.Directory.new(); io.Directory.gone; io.Directory.gone(); }";
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let scanned_a = dartq_scanner::scan(
        source_a,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file_a,
    );
    let scanned_m = dartq_scanner::scan(
        source_m,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file_m,
    );
    let elements_a = collect(source_a, &scanned_a.tokens, features, &strings);
    let elements_m = collect(source_m, &scanned_m.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib_a = builder.intern("package:t/a.dart");
    let lib_m = builder.intern("package:t/main.dart");
    let mut index = ElementIndex::default();
    index.insert(file_a, elements_a.clone());
    index.insert(file_m, elements_m.clone());
    for (name, _) in elements_a.declarations() {
        builder.declare(lib_a, name, file_a);
    }
    for (name, decl) in elements_m.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_m, name, file_m);
    }
    builder.import(
        lib_m,
        ImportEdge {
            target: Some(lib_a),
            prefix: Some(strings.intern("io")),
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let report = diagnose(
        source_m,
        &scanned_m.tokens,
        features,
        &strings,
        file_m,
        lib_m,
        &graph,
        &namespaces,
        &index,
    );
    assert_eq!(
        code_names(&report),
        ["undefined_getter", "undefined_method"]
    );
}

/// A constructor selector carrying type arguments
/// (`C.new<int,int>()`) reports `wrong_number_of_type_arguments_constructor`
/// — the dedicated code, not the `_element` dispatch a member takes:
/// constructors never declare type parameters, so any non-empty group
/// mismatches (probe: `dart analyze` → `WRONG_NUMBER_OF_TYPE_ARGUMENTS_CONSTRUCTOR`
/// at the `C.new` span).
#[test]
fn constructor_type_arguments_report_the_ctor_code() {
    let report = report("class int {} class C<T> { C.new(); } void f() { C.new<int,int>(); }");
    assert_eq!(
        code_names(&report),
        ["wrong_number_of_type_arguments_constructor"]
    );
}

/// A null-aware access on a nullable receiver still applies the
/// non-nullable `on` face (#2094-d): `x?.extendedProp` where `x: C?`
/// and `extension E on C` resolves the member — the `?.` unwraps the
/// receiver before application, so the member is accessed on `C`, never
/// `C?` (probe: `dart analyze` on `x?.extendedProp` over a non-nullable
/// `on C` face is silent; the truly-absent `x?.missing` still reports
/// `undefined_getter`).
///
/// The non-null-aware companion (`x.m()` on `C?` where `extension E on
/// C` declares `m`) resolves the member but flags the unsafe access —
/// `unchecked_use_of_nullable_value`, never `undefined_method` (probe:
/// `dart analyze` reports `UNCHECKED_USE_OF_NULLABLE_VALUE`).
#[test]
fn extension_applies_through_null_aware_and_flags_unsafe_plain() {
    let null_aware = report(
        "class C {} extension E on C { void extendedProp() {} } \
             void f(C? x) { x?.extendedProp; x?.missing; }",
    );
    assert_eq!(code_names(&null_aware), ["undefined_getter"]);
    let plain = report(
        "class C {} extension E on C { void m() {} } \
             void f(C? x) { x.m(); }",
    );
    assert_eq!(code_names(&plain), ["unchecked_use_of_nullable_value"]);
}

/// Extension applicability is the receiver's subtype relation to the
/// `on` type, not name equality (oracle-probed): an extension on a
/// supertype — generic `I<T>`, a mixin, `Object` — applies to the
/// subtype receiver; a strictly more specific
/// `on` type wins; equal-rank unrelated `on` types stay ambiguous.
#[test]
fn extension_applies_to_subtype_receivers_by_specificity() {
    let applies = report(
        "class I<T> {} class J<T> extends I<T> {} class K extends I<J> {} \
             mixin M {} class D with M {} \
             extension<T> on I<T> { void get a {} } \
             extension on M { void get b {} } \
             extension on Object { void get c {} } \
             void f(J<K> j, K k, D d) { j.a; k.a; d.b; d.c; }",
    );
    let misses: Vec<&str> = code_names(&applies)
        .into_iter()
        .filter(|c| c.starts_with("undefined_getter") || c.starts_with("ambiguous"))
        .collect();
    assert!(misses.is_empty(), "{:?}", code_names(&applies));
    let specific = report(
        "class I {} class J extends I {} \
             extension on I { void get m {} } \
             extension on J { void get m {} } \
             void f(J j) { j.m; }",
    );
    assert!(
        !code_names(&specific).contains(&"ambiguous_extension_member_access"),
        "{:?}",
        code_names(&specific)
    );
    let ambiguous = report(
        "abstract class A {} abstract class B {} class C implements A, B {} \
             extension on A { void get m {} } \
             extension on B { void get m {} } \
             void f(C c) { c.m; }",
    );
    assert!(
        code_names(&ambiguous).contains(&"ambiguous_extension_member_access"),
        "{:?}",
        code_names(&ambiguous)
    );
}

// —— #476 item 2: extension-type representation ———————————————————————

/// The representation field sharing the extension type's name reports
/// `member_with_class_name` at the field's name
/// (`error_verifier.dart:5411-5413`).
#[test]
fn representation_named_like_the_type_reports() {
    let report = report("class int {} extension type E(int E) {}");
    assert_eq!(
        code_names(&report),
        ["member_with_class_name"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "A class member can't have the same name as the enclosing class."
    );
}

/// The representation field named like an `Object` member reports
/// `extension_type_declares_member_of_object`
/// (`error_verifier.dart:5416-5423`).
#[test]
fn representation_named_like_an_object_member_reports() {
    let report = report("class int {} extension type E(int hashCode) {}");
    assert_eq!(
        code_names(&report),
        ["extension_type_declares_member_of_object"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "Extension types can't declare members with the same name as a member declared by 'Object'."
    );
}

/// A representation field typed with the extension type itself reports
/// `extension_type_representation_depends_on_itself` at the type's name
/// (`error_verifier.dart:5343-5354`; the M1 name-level approximation
/// covers direct references — `List<E>` shapes stay an under-report).
#[test]
fn self_referential_representation_reports() {
    let report = report("extension type E(E x) {}");
    assert_eq!(
        code_names(&report),
        ["extension_type_representation_depends_on_itself"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The extension type representation can't depend on itself."
    );
}

/// A clean representation emits nothing from the semantic side, and the
/// 3.15 language version runs the official primary-constructors-enabled
/// arm (`enabled_in: 3.13`), where the typeless-representation row is
/// deliberately absent (`error_verifier.dart:5425` feature branch).
#[test]
fn clean_representation_stays_silent() {
    let clean = report("class int {} extension type E(int x) {}");
    assert!(code_names(&clean).is_empty(), "{:?}", code_names(&clean));
    let typeless = report("class int {} extension type E(it) {}");
    assert!(
        code_names(&typeless).is_empty(),
        "{:?}",
        code_names(&typeless)
    );
}

// —— #476 item 3: the mixin application chain —————————————————————————

/// A mixin-only member resolves through the stacked application:
/// `C().m()` finds `M1.m` even though the superclass chain lacks it (the
/// flat predecessor walked the superclass only and reported).
#[test]
fn mixin_application_member_resolves() {
    let report = report(
        "class B {} mixin M1 { void m() {} } \
             class C extends B with M1 {} \
             void main() { C().m(); }",
    );
    assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
}

/// Later applications win: with `M1.m: int` and `M2.m: String` fields,
/// the `m` read resolves M2's `String` — proven by the return-type
/// mismatch against an `int` host (`inheritance_manager3.dart:571-579`:
/// each mixin replaces the members of the interface).
#[test]
fn later_mixin_application_wins() {
    let report = report(
        "class B {} class int {} class String {} \
             mixin M1 { int m = 1; } \
             mixin M2 { String m = ''; } \
             class C extends B with M1, M2 { int f() { return m; } }",
    );
    assert_eq!(
        code_names(&report),
        ["return_of_invalid_type"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "A value of type 'String' can't be returned from the method 'f' because it has a return type of 'int'."
    );
}

/// With the later mixin lacking the member, the earlier one's surfaces
/// through the application chain (`M1` over `S`) — the read resolves
/// M1's `int` field, so the `int` host stays clean.
#[test]
fn earlier_mixin_member_surfaces_through_the_chain() {
    let report = report(
        "class B {} class int {} class String {} \
             mixin M1 { int m = 1; } \
             mixin M2 { void other() {} } \
             class C extends B with M1, M2 { int f() { return m; } }",
    );
    assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
}

/// `super.member` walks the applied chain too: `super.h()` in
/// `class C extends B with M1` resolves M1's `h` (the official
/// superclass of `C` is the application `B&M1`,
/// `inheritance_override.dart:344-351`).
#[test]
fn super_walk_sees_the_stacked_chain() {
    let report = report(
        "class B {} mixin M1 { void h() {} } \
             class C extends B with M1 { void f() { super.h(); } }",
    );
    assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
}

// —— #443: the member-face walk memo ——————————————————————————————————

/// Repeated probes keep their verdicts under the memo, and the two
/// climb flavors of one member stay distinct keys: `m()` hits the
/// instance probe (`Found` through `C`'s own face) while `super.m()`
/// misses the super-climb probe of the *same* member — a key that
/// conflated the two would silence the `undefined_super_member`. The
/// two `nope()` rows share one cached `Missing` and still both report.
#[test]
fn member_face_memo_keeps_repeated_probe_verdicts() {
    let report = report(
        "class B {} \
             class C extends B { \
               void m() {} void f() { m(); nope(); nope(); super.m(); } \
             }",
    );
    assert_eq!(
        code_names(&report),
        [
            "undefined_method",
            "undefined_method",
            "undefined_super_member",
        ],
        "{:?}",
        code_names(&report)
    );
}

/// The override check sees the stacked winner: `C.m()` is arity-broken
/// against `M2.m(int)` (the interface winner per
/// `inheritance_manager3.dart:571-579`) — reported once for the stacked
/// superclass face (the same inherited entry, pointer-deduped). Oracle-
/// verified (#2367): the live analyzer reports four `invalid_override`
/// rows on this shape — the two mixin-application verdicts (`M1.m` vs
/// `B.m` at `with M1`, `M2.m` vs `M1.m` at `with M2`) and the two member
/// verdicts (`C.m` vs `B.m`, `C.m` vs `M2.m`).
#[test]
fn override_check_reports_the_stacked_winner() {
    let report = report(
        "class int {} class B { void m(int x) {} } \
             mixin M1 { void m() {} } \
             mixin M2 { void m(int x) {} } \
             class C extends B with M1, M2 { void m() {} }",
    );
    assert_eq!(
        code_names(&report),
        [
            "invalid_override",
            "invalid_override",
            "invalid_override",
            "invalid_override"
        ],
        "{:?}",
        code_names(&report)
    );
    // The stacked face's report names the interface winner.
    let stacked = report
        .diagnostics
        .iter()
        .find(|d| d.message.contains("M2.m"))
        .expect("the stacked winner's report");
    assert!(
        stacked
            .message
            .starts_with("'C.m' ('void Function()') isn't a valid override of 'M2.m'"),
        "{}",
        stacked.message
    );
}
// —— #504: the usage cluster ————————————————————————————————————————

/// An unread local reports; a plain read spares; a write alone never
/// uses; the useless-statement reads (`!v;`, `v++;`) never count.
///
/// Hand-trace: `dead` has no hit; `live`'s argument read counts; `gone`
/// takes only the write (the local hit cancels at the assignment);
/// `discarded`'s two reads both land in statement windows whose parent
/// shape is a unary form (`_isUsefulRead`).
#[test]
fn unused_local_variable_reports_and_reads_spare() {
    let r = report(
        "class int {} void take(int x) {} \
             void main() { int dead = 0; int live = 0; take(live); \
             int gone = 0; gone = 1; int discarded = 0; !discarded; discarded++; }",
    );
    assert_eq!(
        code_names(&r),
        [
            "unused_local_variable",
            "unused_local_variable",
            "unused_local_variable",
            // The bare `!discarded;` statement's operand is statically
            // `int` — `LogicalNotResolver.resolve` runs the negation
            // face unconditionally (logical_not_resolver.dart:27), so
            // the oracle reports it beside the unused reads.
            "non_bool_negation_expression"
        ],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "The value of the local variable 'dead' isn't used."
    );
}

/// `v;` and `v + 1;` are useful reads (the official `_isUsefulRead`
/// spares everything but the unary statement shapes).
#[test]
fn plain_and_operator_reads_spare_the_local() {
    // Both reads are useful per the official `_isUsefulRead` (a plain
    // statement expression and a binary-operator operand), so the
    // local never reports `unused_local_variable`; `class int {}`
    // declares no `+`, so the operator read reports the oracle's
    // `undefined_operator` at the site.
    let r = report("class int {} void main() { int v = 0; v; v + 1; }");
    assert_eq!(
        code_names(&r),
        ["undefined_operator"],
        "{:?}",
        code_names(&r)
    );
}

/// The wildcard-variables face: an all-underscores name never reports;
/// a mixed name still does.
#[test]
fn wildcard_locals_never_report() {
    let r = report("class int {} void main() { var _ = 1; int _z = 2; }");
    assert_eq!(
        code_names(&r),
        ["unused_local_variable"],
        "{:?}",
        code_names(&r)
    );
    assert!(
        r.diagnostics[0].message.contains("'_z'"),
        "{}",
        r.diagnostics[0].message
    );
}

/// A pattern-variable-declaration group reports all-or-nothing: one
/// used member spares the rest (official `visitPatternVariableDeclaration`).
#[test]
fn pattern_groups_report_all_or_nothing() {
    let r = report("void main() { var (a, b) = (1, 2); }");
    assert_eq!(
        code_names(&r),
        ["unused_local_variable", "unused_local_variable"],
        "{:?}",
        code_names(&r)
    );
    // The local `int` stands in for the core type (the harness has no
    // seeded dart:core); the read of `a` spares the whole pattern group
    // (official visitPatternVariableDeclaration all-or-nothing).
    let r2 =
        report("class int {} void take(int x) {} void main() { var (a, b) = (1, 2); take(a); }");
    assert!(code_names(&r2).is_empty(), "{:?}", code_names(&r2));
}

/// A pattern window dies with its frame: `f`'s if-case opens the
/// window, `f` pops (truncating the locals stack), and `g`'s if-case
/// must open a fresh group — not read the popped frame's slot (the
/// index-keyed window panicked `self.locals[base]` here, "the len is 1
/// but the index is 1").
#[test]
fn pattern_window_closes_at_frame_pop() {
    let r = report(
        "class int {} void f(int p) { if (p case int a) {} } \
             void g(int q) { if (q case int b) {} }",
    );
    assert_eq!(
        code_names(&r),
        ["unused_local_variable", "unused_local_variable"],
        "{:?}",
        code_names(&r)
    );
    // Each window is its own all-or-nothing set: using `a` spares only
    // `a`'s group; `g`'s unused `b` still reports.
    let r2 = report(
        "class int {} void take(int x) {} \
             void f(int p) { if (p case int a) { take(a); } } \
             void g(int q) { if (q case int b) {} }",
    );
    assert_eq!(
        code_names(&r2),
        ["unused_local_variable"],
        "{:?}",
        code_names(&r2)
    );
}

/// An intervening non-pattern local closes the window: two adjacent
/// if-case windows separated by a plain local are two independent
/// all-or-nothing sets, not one.
#[test]
fn pattern_window_closes_on_intervening_local() {
    let r = report(
        "class int {} void take(int x) {} void f(int p) { \
             if (p case int a) { take(a); } int q = 1; if (p case int b) {} }",
    );
    // `q` and `b` each report; `a`'s use does not spare `b` (the
    // index-keyed window joined `b` into `a`'s group here).
    assert_eq!(
        code_names(&r),
        ["unused_local_variable", "unused_local_variable"],
        "{:?}",
        code_names(&r)
    );
}

/// An unreferenced local function reports `unused_element`; its own
/// self-reference never counts (`_enclosingExec`), but an outer
/// invocation does.
#[test]
fn unused_local_function_reports_unused_element() {
    let r = report("void main() { void inner() {} }");
    assert_eq!(code_names(&r), ["unused_element"], "{:?}", code_names(&r));
    assert_eq!(
        r.diagnostics[0].message,
        "The declaration 'inner' isn't referenced."
    );
    let r2 = report("void main() { void inner() {} inner(); }");
    assert!(code_names(&r2).is_empty(), "{:?}", code_names(&r2));
}

/// Private top-level functions and classes report when nothing outside
/// themselves references them; public names never do.
#[test]
fn unused_private_library_declarations_report() {
    let r = report("void _dead() {} class _Gone {} void main() {}");
    assert_eq!(
        code_names(&r),
        ["unused_element", "unused_element"],
        "{:?}",
        code_names(&r)
    );
    let r2 = report("void _live() {} class _Kept {} void main() { _live(); _Kept(); }");
    assert!(code_names(&r2).is_empty(), "{:?}", code_names(&r2));
}

/// A resolved instance creation's arguments resolve like any other
/// expression: a bare tear-off argument (`onPressed: _bump`) marks its
/// use at pass 2 (`_useIdentifierElement`), so the method is spared
/// while a genuinely unreferenced sibling still reports.
#[test]
fn creation_argument_tear_off_spares_method() {
    let r = report(
        "class Btn { Btn({onPressed, child}); } \
             class _S { void _bump() {} void _dead() {} \
             void build() { Btn(onPressed: _bump); } } \
             void main() { _S().build(); }",
    );
    assert_eq!(code_names(&r), ["unused_element"], "{:?}", code_names(&r));
    assert_eq!(
        r.diagnostics[0].message,
        "The declaration '_dead' isn't referenced."
    );
}

/// The `unused_field` rule reads only: a write never spares, a read
/// anywhere in the library does (the implicit-getter face).
#[test]
fn unused_field_reads_only() {
    let r = report(
        "class int {} class _C { int _read = 0; int _written = 0; int _dead = 0; int live = 0; \
             int f() => _read; int g() => live; void w() { _written = 1; } } \
             void main() { _C().f(); _C().g(); }",
    );
    assert_eq!(
        code_names(&r),
        ["unused_field", "unused_field"],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "The value of the field '_written' isn't used."
    );
    assert_eq!(
        r.diagnostics[1].message,
        "The value of the field '_dead' isn't used."
    );
}

// —— #911: the primary-constructor field face ———————————————————————————

/// An unread `var`/`final` primary-constructor field reports
/// `unused_field_from_primary_constructor` at the parameter's name; a
/// read twin stays silent; the row is a `STATIC_WARNING` with the
/// keyword-specific correction.
///
/// Verbatim oracle probe (dart analyze 3.13.1; primary constructors
/// shipped 3.13.0): `class Q(var _u, final int _v) {}` —
/// `WARNING|STATIC_WARNING|UNUSED_FIELD_FROM_PRIMARY_CONSTRUCTOR|
/// …|1|13|2|The value of the field '_u' isn't used.` and the same for
/// `_v`; a getter read of `_v` leaves only the `_u` row.
#[test]
fn unused_primary_constructor_field_reports() {
    let r = report("class int {} class P(var _a, final int _b) { int get r => _b; }");
    assert_eq!(
        code_names(&r),
        ["unused_field_from_primary_constructor"],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "The value of the field '_a' isn't used."
    );
    let at = u32::try_from("class int {} class P(var ".len()).unwrap();
    assert_eq!(r.diagnostics[0].range.offset, at);
    assert_eq!(r.diagnostics[0].range.length, 2);
    assert_eq!(
        r.diagnostics[0].correction.as_deref(),
        Some(
            "Try removing the 'var' keyword to avoid declaring a field, or try using the field, or removing it."
        )
    );
    // The read twin never reports.
    let r2 = report("class int {} class P(var _a, final int _b) { int f() => _a + _b; }");
    assert!(code_names(&r2).is_empty(), "{:?}", code_names(&r2));
}

/// The exemption faces: a public name is always readable
/// (`_isReadMember`'s public-instance arm answers used), a
/// non-declaring formal (`int _a` — no `var`/`final` keyword) declares
/// nothing, and an extension type's representation formal never reports
/// (the official switch's third arm).
#[test]
fn primary_constructor_field_exemptions_stay_silent() {
    let r = report("class P(var a) {}");
    assert!(code_names(&r).is_empty(), "{:?}", code_names(&r));
    let r2 = report("class int {} class P(int _a) {}");
    assert!(code_names(&r2).is_empty(), "{:?}", code_names(&r2));
    let r3 = report("extension type ET(final int _x) {}");
    assert!(
        !code_names(&r3).contains(&"unused_field_from_primary_constructor"),
        "{:?}",
        code_names(&r3)
    );
}

/// Optional parameters of private executables report when no body use
/// resolves to them and no invocation supplies them: positionally by
/// given-count, named by name.
#[test]
fn unused_optional_parameters_report() {
    let r = report(
        "class int {} class _C { void _m(int a, [int b = 0]) {} void _n(int a, {int c = 0}) {} } \
             void main() { _C()._m(1); _C()._n(1); }",
    );
    assert_eq!(
        code_names(&r),
        ["unused_element_parameter", "unused_element_parameter"],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "A value for optional parameter 'b' isn't ever given."
    );
    assert_eq!(
        r.diagnostics[1].message,
        "A value for optional parameter 'c' isn't ever given."
    );
    let r2 = report(
        "class int {} class _C { void _m(int a, [int b = 0]) {} } void main() { _C()._m(1, 2); }",
    );
    assert!(code_names(&r2).is_empty(), "{:?}", code_names(&r2));
    let r3 = report(
        "class int {} class _C { void _m(int a, [int b = 0]) { b; } } void main() { _C()._m(1); }",
    );
    assert!(code_names(&r3).is_empty(), "{:?}", code_names(&r3));
}

/// A named constructor's optional parameter reports when never given
/// (the class keeps more than one constructor); the invocation spares
/// the constructor itself.
#[test]
fn unused_named_constructor_parameter_reports() {
    let r = report(
        "class int {} class _D { _D.named([int x = 0]); _D.other(); } \
             void main() { _D.named(); _D.other(); }",
    );
    assert_eq!(
        code_names(&r),
        ["unused_element_parameter"],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "A value for optional parameter 'x' isn't ever given."
    );
}

/// The given-parameter faces spare the report: one supplied positional
/// at the invocation marks the parameter given (the merged `.named`
/// member row's spelling), and the unnamed constructor's face
/// fires/spares under its class-name key (the folded `Created` head
/// row's spelling). `_C`'s single constructor stays unreported itself
/// (the >1-constructor rule), the parameter pass runs regardless.
#[test]
fn unused_constructor_parameters_spare_when_given() {
    let r = report(
        "class int {} class _D { _D.named([int x = 0]); _D.other(); } \
             void main() { _D.named(1); _D.other(); }",
    );
    assert!(code_names(&r).is_empty(), "{:?}", code_names(&r));
    let r2 = report("class int {} class _C { _C([int x = 0]); } void main() { _C(); }");
    assert_eq!(
        code_names(&r2),
        ["unused_element_parameter"],
        "{:?}",
        code_names(&r2)
    );
    assert_eq!(
        r2.diagnostics[0].message,
        "A value for optional parameter 'x' isn't ever given."
    );
    let r3 = report("class int {} class _C { _C([int x = 0]); } void main() { _C(1); }");
    assert!(code_names(&r3).is_empty(), "{:?}", code_names(&r3));
}

// —— #2307: constructor-selector element credit ———————————————————————

/// A private named constructor created through an explicit `const` or
/// `new` keyword is referenced: the selector rides the creation's
/// head events (no member row), so the element credit spares it, and
/// the genuinely unreferenced sibling still reports. Pre-fix both
/// spurious rows fired (the const/new forms never credited the
/// selector).
#[test]
fn unused_private_constructor_const_and_new_uses_spare() {
    let r = report(
        "class int {} class A { const A._ca(); const A._da(); } \
             class B { B._nb(); B._db(); } \
             const a = A._ca(); final b = new B._nb(); void main() {}",
    );
    assert_eq!(
        code_names(&r),
        ["unused_element", "unused_element"],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "The declaration 'A._da' isn't referenced."
    );
    assert_eq!(
        r.diagnostics[1].message,
        "The declaration 'B._db' isn't referenced."
    );
}

/// An annotation's `@A._x` selector names the class's constructor
/// (the metadata continuation carries no member row): the annotated
/// constructor stays silent while its unreferenced sibling reports.
#[test]
fn unused_private_constructor_annotation_use_spares() {
    let r = report(
        "class int {} class A { const A._ann(); const A._dead(); } \
             @A._ann class B {} void main() { B(); }",
    );
    assert_eq!(code_names(&r), ["unused_element"], "{:?}", code_names(&r));
    assert_eq!(
        r.diagnostics[0].message,
        "The declaration 'A._dead' isn't referenced."
    );
    // The negative twin: without the annotation both report.
    let r2 = report(
        "class int {} class A { const A._ann2(); const A._dead2(); } \
             class B {} void main() { B(); }",
    );
    assert_eq!(
        code_names(&r2),
        ["unused_element", "unused_element"],
        "{:?}",
        code_names(&r2)
    );
}

/// A parameterized creation (`C<int>._x()`, `const C<int>._y()`) hands
/// its selector to the after-type-arguments continuation instead of
/// the plain one: the head must still credit the constructor element.
/// Same-spelling rescue would mask the miss, so each class carries its
/// own selector spelling.
#[test]
fn unused_private_constructor_parameterized_creation_spares() {
    let r = report(
        "class int {} class A<T> { A._pa(); A._deadA(); } \
             class B<T> { const B._pb(); const B._deadB(); } \
             final a = A<int>._pa(); const b = B<int>._pb(); void main() {}",
    );
    assert_eq!(
        code_names(&r),
        ["unused_element", "unused_element"],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "The declaration 'A._deadA' isn't referenced."
    );
    assert_eq!(
        r.diagnostics[1].message,
        "The declaration 'B._deadB' isn't referenced."
    );
}

/// A record literal's fields drain into the literal's own row, but each
/// field still owes its reference credit: a bare call inside the
/// literal marks its callee used, a read marks the field read, and an
/// unreferenced sibling still reports. Pre-fix the drain kept only the
/// unresolved shapes, so every resolved field reference was lost.
#[test]
fn unused_record_literal_field_reference_spares() {
    let r = report(
        "class int {} int _live(int x) => x; int _gone(int x) => x; \
             void f() { var r = (a: _live(1), b: 2); r; } void main() { f(); }",
    );
    assert_eq!(code_names(&r), ["unused_element"], "{:?}", code_names(&r));
    assert_eq!(
        r.diagnostics[0].message,
        "The declaration '_gone' isn't referenced."
    );
    let r2 = report(
        "class int {} class C { int _read = 1; int _written = 2; \
             void g() { var r = (a: _read); r; } void w() { _written = 3; } } \
             void main() { C().g(); C().w(); }",
    );
    assert_eq!(code_names(&r2), ["unused_field"], "{:?}", code_names(&r2));
    assert_eq!(
        r2.diagnostics[0].message,
        "The value of the field '_written' isn't used."
    );
}

// —— #505: the import/flow cluster ———————————————————————————————————

/// An import that contributed nothing reports at its URI; the same
/// import with a used prefix stays silent.
#[test]
fn unused_import_reports_at_the_uri() {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let source = "import 'other.dart'; void main() {}";
    let scanned = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(scanned.diagnostics.is_empty());
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let other = builder.intern("package:t/other.dart");
    builder.declare(other, strings.intern("thing"), file);
    builder.import(
        lib,
        ImportEdge {
            target: Some(other),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    let r = diagnose(
        source,
        &scanned.tokens,
        features,
        &strings,
        file,
        lib,
        &graph,
        &namespaces,
        &index,
    );
    assert_eq!(code_names(&r), ["unused_import"], "{:?}", code_names(&r));
    assert_eq!(r.diagnostics[0].message, "Unused import: 'other.dart'.");
}

#[test]
fn used_prefix_import_stays_silent() {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let source = "import 'other.dart' as p; void main() { p.thing; }";
    let scanned = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(scanned.diagnostics.is_empty());
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let other = builder.intern("package:t/other.dart");
    builder.declare(other, strings.intern("thing"), file);
    builder.import(
        lib,
        ImportEdge {
            target: Some(other),
            prefix: Some(strings.intern("p")),
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    let r = diagnose(
        source,
        &scanned.tokens,
        features,
        &strings,
        file,
        lib,
        &graph,
        &namespaces,
        &index,
    );
    assert!(
        !code_names(&r).contains(&"unused_import"),
        "{:?}",
        code_names(&r)
    );
}

/// The #768 rig: `source` imports `other.dart`, which declares
/// `other_names`; each import is a `(prefix, show list)` pair — an
/// empty prefix is the unprefixed face, an empty show list is the
/// unfiltered edge.
fn report_with_imports(
    source: &str,
    other_names: &[&str],
    imports: &[(&str, &[&str])],
    core_prelude: bool,
) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let scanned = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(scanned.diagnostics.is_empty());
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let other = builder.intern("package:t/other.dart");
    for name in other_names {
        builder.declare(other, strings.intern(name), file);
    }
    // The engine's merge shape: the implicit `dart:core` import joins in
    // front of every written directive (`build_library_graph`'s serial
    // fold — the harness default builds no such edge).
    if core_prelude {
        let core = builder.intern("dart:core");
        builder.import(
            lib,
            ImportEdge {
                target: Some(core),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
    }
    for (prefix, show) in imports {
        let combinators = if show.is_empty() {
            Combinators::unfiltered()
        } else {
            Combinators::new(
                [CombinatorClause::Show(
                    show.iter()
                        .map(|name| strings.intern(name))
                        .collect::<Vec<_>>()
                        .into_boxed_slice(),
                )],
                &strings,
            )
        };
        builder.import(
            lib,
            ImportEdge {
                target: Some(other),
                prefix: (!prefix.is_empty()).then(|| strings.intern(prefix)),
                deferred: false,
                combinators,
            },
        );
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    diagnose(
        source,
        &scanned.tokens,
        features,
        &strings,
        file,
        lib,
        &graph,
        &namespaces,
        &index,
    )
}

/// A partially used `show` list reports each unused name at its own
/// token (probed: dart analyze 3.13.1 — `UNUSED_SHOWN_NAME` on the name
/// lexeme, `The name max is shown, but isn't used.`).
#[test]
fn unused_shown_name_reports_at_the_name_token() {
    let source = "import 'other.dart' show alpha, beta; void main() { beta; }";
    let r = report_with_imports(
        source,
        &["alpha", "beta"],
        &[("", &["alpha", "beta"])],
        false,
    );
    assert_eq!(
        code_names(&r),
        ["unused_shown_name"],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "The name alpha is shown, but isn't used."
    );
    assert_eq!(
        r.diagnostics[0].range.offset,
        offset_of(source, "alpha"),
        "{:?}",
        r.diagnostics[0].range
    );
    assert_eq!(
        r.diagnostics[0].correction.as_deref(),
        Some("Try removing the name from the list of shown members.")
    );
}

/// A wholly unused import keeps its single `unused_import` row — the
/// oracle skips the per-name hints for it (`imports_verifier.dart:283`:
/// `if (_unusedImports.contains(importDirective)) { continue; }`).
#[test]
fn wholly_unused_show_import_keeps_one_unused_import_row() {
    let source = "import 'other.dart' show alpha, beta; void main() {}";
    let r = report_with_imports(
        source,
        &["alpha", "beta"],
        &[("", &["alpha", "beta"])],
        false,
    );
    assert_eq!(code_names(&r), ["unused_import"], "{:?}", code_names(&r));
}

/// The engine's library shape (`build_library_graph`'s merge prepends
/// the implicit `dart:core` import in front of every written edge):
/// the written-directive alignment excludes exactly that prelude edge,
/// so the pass stays live in engine mode. Pre-fix the count gate saw
/// two edges against one written directive and suppressed every
/// `unused_import`/`unused_shown_name` row of every importing file.
#[test]
fn engine_core_prelude_keeps_the_written_import_alignment() {
    // A partially used show list still reports its unused name.
    let source = "import 'other.dart' show alpha, beta; void main() { alpha; }";
    let r = report_with_imports(
        source,
        &["alpha", "beta"],
        &[("", &["alpha", "beta"])],
        true,
    );
    assert_eq!(
        code_names(&r),
        ["unused_shown_name"],
        "{:?}",
        code_names(&r)
    );
    // A wholly unused written import reports at its URI; the implicit
    // prelude edge itself never reports (`dart:core` target skip).
    let source = "import 'other.dart'; void main() {}";
    let r = report_with_imports(source, &["thing"], &[("", &[])], true);
    assert_eq!(code_names(&r), ["unused_import"], "{:?}", code_names(&r));
    assert_eq!(
        r.diagnostics[0].range.offset,
        offset_of(source, "'other.dart'")
    );
}

/// Every shown name used — silent (the oracle's both-used face).
#[test]
fn fully_used_show_list_stays_silent() {
    let source = "import 'other.dart' show alpha, beta; void main() { alpha; beta; }";
    let r = report_with_imports(
        source,
        &["alpha", "beta"],
        &[("", &["alpha", "beta"])],
        false,
    );
    assert!(code_names(&r).is_empty(), "{:?}", code_names(&r));
}

/// A prefixed import's show list runs the same per-name check, and a
/// prefix-qualified use (`m.alpha`) credits the member name (probed:
/// `import 'dart:math' as m show max, min; m.max(1, 2);` reports `min`).
#[test]
fn prefixed_shown_name_reports_on_the_partially_used_prefix() {
    let source = "import 'other.dart' as m show alpha, beta; void main() { m.alpha; }";
    let r = report_with_imports(
        source,
        &["alpha", "beta"],
        &[("m", &["alpha", "beta"])],
        false,
    );
    assert_eq!(
        code_names(&r),
        ["unused_shown_name"],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(r.diagnostics[0].range.offset, offset_of(source, "beta"));
}

/// A name's use credits every import that provides it: two imports of
/// one library, the used `alpha` shown by both, leaves only the first
/// import's `beta` (probed: the `max`-shown-twice face — the shared
/// import reports its other shown name, the fully-used one is silent).
#[test]
fn shown_name_use_credits_every_import_that_provides_it() {
    let source = "import 'other.dart' show alpha, beta; \
                      import 'other.dart' show alpha; void main() { alpha; }";
    let r = report_with_imports(
        source,
        &["alpha", "beta"],
        &[("", &["alpha", "beta"]), ("", &["alpha"])],
        false,
    );
    assert_eq!(
        code_names(&r),
        ["unused_shown_name"],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(r.diagnostics[0].range.offset, offset_of(source, "beta"));
}

/// An undefined shown name is not the unused face: the resolution gate
/// skips it (probed: the oracle reports `undefined_shown_name` there
/// and no `unused_shown_name`; that undefined row is the engine
/// pipeline's face — `dartq-engine` l1.rs — so the semantic pass is
/// silent on the name).
#[test]
fn undefined_shown_name_is_not_the_unused_face() {
    let source = "import 'other.dart' show gone, alpha; void main() { alpha; }";
    let r = report_with_imports(source, &["alpha"], &[("", &["gone", "alpha"])], false);
    assert!(code_names(&r).is_empty(), "{:?}", code_names(&r));
}

/// The catch-variable routing: `on T catch (e)` without a stack trace
/// reports the clause; a stack trace variable always reports when
/// unread; a plain `catch (e)` auto-uses its exception.
#[test]
fn unused_catch_variables_route_by_kind() {
    let r =
        report("class int {} void main() { try {} on int catch (e) {} try {} catch (e2, st) {} }");
    assert_eq!(
        code_names(&r),
        ["unused_catch_clause", "unused_catch_stack"],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "The exception variable 'e' isn't used, so the 'catch' clause can be removed."
    );
    assert_eq!(
        r.diagnostics[1].message,
        "The stack trace variable 'st' isn't used and can be removed."
    );
}

/// A catch-all clause followed by more clauses makes the tail dead
/// (`_CatchClausesVerifier.nextCatchClause`); the span covers the
/// following clauses.
#[test]
fn dead_catch_tail_reports() {
    let r = report(
        "class int {} class Object {} void main() { try {} on Object catch (e) {} on int {} }",
    );
    assert_eq!(
        code_names(&r),
        ["unused_catch_clause", "dead_code_catch_following_catch"],
        "{:?}",
        code_names(&r)
    );
}

/// A `??` whose left operand is strictly non-nullable anchors the
/// report at the right operand; `??=` anchors at the value. Both reads
/// count (the assignment target never loses its use). The m2 corpus
/// face writes the operand types as bare core-type parameters: the
/// written type's nullability is syntactic, no core-library
/// declaration needed. The `??` arm's generic `dead_code` row
/// coexists with the dedicated row — the official emits both
/// (`_checkForDeadNullCoalesce` plus the flow model's dead region).
#[test]
fn dead_null_aware_right_operands_report() {
    let r = report("class int {} void main() { int x = 1; int y = 2; x ?? y; y ??= x; }");
    assert_eq!(
        code_names(&r),
        [
            "dead_code",
            "dead_null_aware_expression",
            "dead_null_aware_expression"
        ],
        "{:?}",
        code_names(&r)
    );
    // `class int {}` follows the harness convention (#537): the
    // parameter types resolve through a concrete declaration, their
    // `undefined_class` rows stay out of the dead-face pin.
    let r = report("class int {} void f(int x, int y) { x ?? y; }");
    assert_eq!(
        code_names(&r),
        ["dead_code", "dead_null_aware_expression"],
        "{:?}",
        code_names(&r)
    );
}

/// The two rows' exact spans (oracle: `dead_code` over `?? y` —
/// operator start through the right operand's end — and
/// `dead_null_aware_expression` over `y`). The flow fact's raw end
/// rides the parser's expression-end token one token late (`?? y)`);
/// the merge clamps it to the staged row's end. `print` carries the
/// harness's declaration stub so the call leaves no undefined row.
#[test]
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn ifnull_dead_code_and_dead_null_aware_coexist() {
    let source = "class int {} void print(int x) {} void f(int x, int y) { print(x ?? y); }";
    let report = report(source);
    assert_eq!(
        code_names(&report),
        ["dead_code", "dead_null_aware_expression"],
        "{:?}",
        code_names(&report)
    );
    let dead = &report.diagnostics[0];
    assert_eq!(dead.code.name, "dead_code");
    assert_eq!(dead.range.offset, offset_of(source, "?? y"));
    assert_eq!(dead.range.length, "?? y".len() as u32);
    let rhs = &report.diagnostics[1];
    assert_eq!(rhs.code.name, "dead_null_aware_expression");
    assert_eq!(rhs.range.offset, offset_of(source, "?? y") + 3);
    assert_eq!(rhs.range.length, 1);
}

/// Labels declare through the labeled-statement scope chain: a
/// `break` into an outer scope spares that label, the unreferenced one
/// reports at its own statement end.
#[test]
fn unused_labels_report() {
    let r = report("void main() { used: for (;;) { break used; } dead: for (;;) { break used; } }");
    assert_eq!(code_names(&r), ["unused_label"], "{:?}", code_names(&r));
    assert_eq!(r.diagnostics[0].message, "The label 'dead' isn't used.");
}

// —— #506: the provably-check cluster —————————————————————————————————

/// `x as T` is dead only under structural type equality
/// (`_isUnnecessaryCast`, best_practices_verifier.dart:1708-1741):
/// `x as num` over `int` is a necessary cast and stays silent.
#[test]
fn unnecessary_cast_pins_equality() {
    // The casts sit in initializers on purpose: a statement-position
    // `x as int;` currently drops its leading operand in the parser's
    // statement disambiguation (the `as`/`is` rescue at statements.rs
    // parses from the `as` without rewinding; the official
    // parser_impl.dart:10026-10046 re-parses from the statement start) —
    // reported to the parser lane, not worked around here with a
    // broken-shape pin.
    let r = report(
        "class int {} class num {} void main() { int x = 1; var a = x as int; var b = x as num; }",
    );
    // `a`/`b` are declared and never read: the official fires
    // unused_local_variable for both, exactly as the port does.
    let mut names = code_names(&r);
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "unnecessary_cast",
            "unused_local_variable",
            "unused_local_variable",
        ],
        "{:?}",
        code_names(&r)
    );
    let cast = r
        .diagnostics
        .iter()
        .find(|d| d.code.name == "unnecessary_cast")
        .expect("the `x as int` cast is dead");
    assert_eq!(cast.message, "Unnecessary cast.");
    assert_eq!(cast.correction.as_deref(), Some("Try removing the cast."));
    // The oracle anchors at the whole AsExpression (`diag.at(node)`) —
    // `x as int`, operand start through the cast type's end.
    let src =
        "class int {} class num {} void main() { int x = 1; var a = x as int; var b = x as num; }";
    assert_eq!(
        cast.range,
        SourceRange {
            offset: offset_of(src, "x as int"),
            length: "x as int".len() as u32,
        }
    );
}

/// The type-check polarity pair (`_checkAllTypeChecks`,
/// best_practices_verifier.dart:987-1041): `x is T` is constant-true
/// over a subtype operand, `x is! T` reports the complement row.
#[test]
fn unnecessary_type_check_polarity() {
    let r = report("class int {} void main() { int x = 1; if (x is int) {} if (x is! int) {} }");
    assert_eq!(
        code_names(&r),
        ["unnecessary_type_check", "unnecessary_type_check"],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "Unnecessary type check; the result is always 'true'."
    );
    assert_eq!(
        r.diagnostics[1].message,
        "Unnecessary type check; the result is always 'false'."
    );
}

/// The alias operand face of the same walk: a formal whose written
/// type is a type alias folds to the aliased type before the subtype
/// question, so `IntAlias x` tested `is int` is constant-true (the
/// oracle's answer for `typedef IntAlias = int; void f(IntAlias x) {
/// if (x is int) {} }` — probed `dart analyze`: one
/// `unnecessary_type_check`), and a `dynamic` operand stays
/// informative.
#[test]
fn unnecessary_type_check_folds_a_typed_formal_alias() {
    let r = report(
        "class int {} typedef IntAlias = int;\nvoid f(IntAlias x) {\n  if (x is int) {}\n}\n",
    );
    assert_eq!(
        code_names(&r),
        ["unnecessary_type_check"],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "Unnecessary type check; the result is always 'true'."
    );
    let silent = report("class int {} void g(dynamic x) {\n  if (x is int) {}\n}\n");
    assert!(
        silent
            .diagnostics
            .iter()
            .all(|d| d.code.name != "unnecessary_type_check"),
        "{:?}",
        code_names(&silent)
    );
}

/// The alias TARGET face of the same walk: an `is` target whose head is
/// a type alias folds to the aliased type before the subtype question,
/// so `int x` tested `is IntAlias` is constant-true (the oracle's
/// answer for `typedef IntAlias = int; ... if (x is IntAlias) {}` —
/// one `unnecessary_type_check`).
#[test]
fn unnecessary_type_check_folds_an_alias_target() {
    let r = report(
        "class int {} typedef IntAlias = int;\nvoid main() {\n  int x = 1;\n  if (x is IntAlias) {}\n}\n",
    );
    assert_eq!(
        code_names(&r),
        ["unnecessary_type_check"],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "Unnecessary type check; the result is always 'true'."
    );
}

/// The alias cast TARGET face (`_isUnnecessaryCast`): an `as` target
/// whose head is a type alias folds to the aliased type before the
/// equality question, so the cast over an `int` operand is dead (the
/// oracle's `unnecessary_cast`).
#[test]
fn unnecessary_cast_folds_an_alias_target() {
    let r = report(
        "class int {} typedef IntAlias = int;\nvoid main() {\n  int x = 1;\n  var a = x as IntAlias;\n}\n",
    );
    let mut names = code_names(&r);
    names.sort_unstable();
    assert_eq!(
        names,
        ["unnecessary_cast", "unused_local_variable"],
        "{:?}",
        code_names(&r)
    );
}

/// The alias cast OPERAND face (the pre-existing green half of the
/// target fix): the operand-side fold binds the resolved type, so
/// `x as int` over an `IntAlias`-declared operand is dead.
#[test]
fn unnecessary_cast_folds_an_alias_operand() {
    let r = report(
        "class int {} typedef IntAlias = int;\nvoid f(IntAlias x) {\n  var a = x as int;\n}\n",
    );
    let mut names = code_names(&r);
    names.sort_unstable();
    assert_eq!(
        names,
        ["unnecessary_cast", "unused_local_variable"],
        "{:?}",
        code_names(&r)
    );
}

/// The member declared-read face: a field whose written type is a type
/// alias folds at the member face answer, so `c.f as int` over
/// `IntAlias f` is dead (the oracle's `unnecessary_cast` — the read's
/// static type is the aliased one). The uninitialized non-nullable
/// field row stands in both universes (the alias resolves to
/// non-nullable `int`).
#[test]
fn unnecessary_cast_folds_a_member_field_alias() {
    let r = report(
        "class int {} typedef IntAlias = int;\nclass C {\n  IntAlias f;\n}\nvoid main() {\n  C c = C();\n  var a = c.f as int;\n}\n",
    );
    let mut names = code_names(&r);
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "not_initialized_non_nullable_instance_field",
            "unnecessary_cast",
            "unused_local_variable",
        ],
        "{:?}",
        code_names(&r)
    );
}

/// The `==` invocation's argument face over an alias formal (probed:
/// dart 3.13.1 `dart analyze` — `typedef IntAlias = int;` with
/// `operator ==(covariant IntAlias other)` makes `B() == B()` report
/// `argument_type_not_assignable` at the invocation with the WRITTEN
/// spelling (`'IntAlias?'`), the alias-to-`Object` formal stays
/// silent (the fold answers the top type), and the raw-`int` control
/// reports the same error with `'int?'` — the verdict reads the
/// folded type, the message the written one.
#[test]
fn argument_type_check_folds_an_equals_formal_alias() {
    let r = report(
        "class int {} class bool {} typedef IntAlias = int;\n\
             class B {\n  bool operator ==(covariant IntAlias other) => true;\n}\n\
             void main() {\n  if (B() == B()) {}\n}\n",
    );
    assert_eq!(
        code_names(&r),
        ["argument_type_not_assignable"],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "The argument type 'B' can't be assigned to the parameter type 'IntAlias?'. "
    );

    // The alias-to-`Object` formal folds to the top type — silent.
    let silent = report(
        "class bool {} class Object {} typedef ObjAlias = Object;\n\
             class A {\n  bool operator ==(covariant ObjAlias other) => true;\n}\n\
             void main() {\n  if (A() == A()) {}\n}\n",
    );
    assert!(code_names(&silent).is_empty(), "{:?}", code_names(&silent));

    // The raw-`int` control: the same error with the resolved
    // spelling (the probe's `p3b` attribution control).
    let raw = report(
        "class int {} class bool {}\n\
             class BRaw {\n  bool operator ==(covariant int other) => true;\n}\n\
             void main() {\n  if (BRaw() == BRaw()) {}\n}\n",
    );
    assert_eq!(
        code_names(&raw),
        ["argument_type_not_assignable"],
        "{:?}",
        code_names(&raw)
    );
    assert_eq!(
        raw.diagnostics[0].message,
        "The argument type 'BRaw' can't be assigned to the parameter type 'int?'. "
    );
}

/// The declared-pattern never-match face over an alias required type
/// (probed: dart 3.13.1 `dart analyze` — a declared pattern whose
/// required type is an alias over a disjoint type reports
/// `pattern_never_matches_value_type` with the WRITTEN spelling
/// `'IntAlias'`, the raw-`int` contrast the resolved `'int'`, and the
/// scrutinee's own alias stays silent — the verdict reads the folded
/// type, the message the written one). The harness scrutinee is the
/// closed enum the `canBeSubtypeOf` cut decides on; the pin rides the
/// switch-case window the check serves (the if-case root face is the
/// sibling pin below).
#[test]
fn declared_pattern_folds_a_never_match_alias() {
    // Two independent switches: the two dead cases cover the same
    // space, so one switch would flag the follower unreachable.
    let r = report(
        "enum E { a } class A {} typedef AliasA = A;\n\
             void f(E e) {\n  switch (e) {\n    case AliasA x: x; break;\n    case E _: break;\n  }\n  switch (e) {\n    case A y: y; break;\n    case E _: break;\n  }\n}\n",
    );
    assert_eq!(
        code_names(&r),
        [
            "pattern_never_matches_value_type",
            "pattern_never_matches_value_type",
        ],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "The matched value type 'E' can never match the required type 'AliasA'."
    );
    assert_eq!(
        r.diagnostics[1].message,
        "The matched value type 'E' can never match the required type 'A'."
    );

    // The scrutinee's own alias folds onto the matched type — silent
    // (and the aliased case alone covers the enum exhaustively).
    let silent = report(
        "enum E { a } typedef AliasE = E;\n\
             void f(E e) {\n  switch (e) {\n    case AliasE x: x; break;\n  }\n}\n",
    );
    assert!(code_names(&silent).is_empty(), "{:?}", code_names(&silent));
}

/// The if-case root runs the same never-match family: the language
/// gives the pattern the condition value as its matched type (a
/// one-case switch), so its window feeds the shared check — the alias
/// fold answers and both of the probe p4 shapes fire (probed:
/// dart 3.13.1 `dart analyze` emits `pattern_never_matches_value_type`
/// at BOTH if-cases, the alias spelling rendered in the message). The
/// anchor is `pattern.type` — the written type's span (6:14 length 8
/// for `IntAlias`, 10:14 length 3 for `int`), not the bound variable.
/// The harness scrutinee is the closed enum (the probe's String/int
/// pair is two open classes — the name level's conservative
/// "possible"), the same decidable disjoint pair the switch-case pin
/// above uses.
#[test]
fn if_case_root_never_match_fires_with_pattern_type_span() {
    let src = "class int {} enum E { a } typedef IntAlias = int;\n\
                   void f(E s) {\n\
                     if (s case IntAlias x) {\n\
                       x;\n\
                     }\n\
                     if (s case int y) {\n\
                       y;\n\
                     }\n\
                   }\n";
    let r = report(src);
    assert_eq!(
        code_names(&r),
        [
            "pattern_never_matches_value_type",
            "pattern_never_matches_value_type",
        ],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "The matched value type 'E' can never match the required type 'IntAlias'."
    );
    assert_eq!(
        r.diagnostics[1].message,
        "The matched value type 'E' can never match the required type 'int'."
    );
    let alias_at = src.find("case IntAlias").unwrap() + "case ".len();
    let raw_at = src.find("case int ").unwrap() + "case ".len();
    assert_eq!(r.diagnostics[0].range.offset, alias_at as u32);
    assert_eq!(r.diagnostics[0].range.length, "IntAlias".len() as u32);
    assert_eq!(r.diagnostics[1].range.offset, raw_at as u32);
    assert_eq!(r.diagnostics[1].range.length, "int".len() as u32);
}

/// The collection-element if-case runs the same family (the element
/// face of the statement pin above): the official routes a case-clause
/// element through `analyzeIfCaseElement` (type_analyzer.dart:3334),
/// whose pattern gets the condition value as its matched type — the
/// same one-case window — so the shared root checks fire at the
/// written type's span, and the case clause spares the plain
/// condition's `non_bool_condition` checkpoint (`s` is `E`, which a
/// bare element condition would report). The boolean-condition control
/// opens no window and stays silent.
#[test]
fn if_case_element_never_match_fires_with_pattern_type_span() {
    let src = "class int {} class bool {} enum E { a } typedef IntAlias = int;\n\
                   void f(E s, bool flag) {\n\
                     [if (s case IntAlias x) x];\n\
                     [if (s case int y) y];\n\
                     [if (flag) 1];\n\
                   }\n";
    let r = report(src);
    assert_eq!(
        code_names(&r),
        [
            "pattern_never_matches_value_type",
            "pattern_never_matches_value_type",
        ],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "The matched value type 'E' can never match the required type 'IntAlias'."
    );
    assert_eq!(
        r.diagnostics[1].message,
        "The matched value type 'E' can never match the required type 'int'."
    );
    let alias_at = src.find("case IntAlias").unwrap() + "case ".len();
    let raw_at = src.find("case int ").unwrap() + "case ".len();
    assert_eq!(r.diagnostics[0].range.offset, alias_at as u32);
    assert_eq!(r.diagnostics[0].range.length, "IntAlias".len() as u32);
    assert_eq!(r.diagnostics[1].range.offset, raw_at as u32);
    assert_eq!(r.diagnostics[1].range.length, "int".len() as u32);
}

/// `is Null`/`is! Null` route to the `type_check_with_null` family;
/// the `null` literal keeps the family row constant and downgrades to
/// the type-check row (`null is Null` is always 'true', `null is! Null`
/// always 'false'); `null is int` stays silent — the official's
/// `isSubtypeOf(Null, int)` is false and the walk has no negative
/// verdict (best_practices_verifier.dart:987-1041). The bare-unit
/// harness has no SDK face, so each `Null` spelling also surfaces the
/// new `type_test_with_undefined_name` operand row — the CLI pipeline
/// (with the SDK face) keeps it silent, probed identical to the
/// oracle; filtered below.
#[test]
fn type_check_with_null_family() {
    let r = report(
        "class int {} void main() { int? x = null; if (x is Null) {} if (x is! Null) {} null is int; null is Null; null is! Null; }",
    );
    let names: Vec<&str> = code_names(&r)
        .into_iter()
        .filter(|name| *name != "type_test_with_undefined_name")
        .collect();
    assert_eq!(
        names,
        [
            "type_check_with_null",
            "type_check_with_null",
            "unnecessary_type_check",
            "unnecessary_type_check",
        ],
        "{:?}",
        code_names(&r)
    );
    let family: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name != "type_test_with_undefined_name")
        .collect();
    assert_eq!(
        family[0].message,
        "Tests for null should be done with '== null'."
    );
    assert_eq!(
        family[1].message,
        "Tests for non-null should be done with '!= null'."
    );
    assert_eq!(
        family[2].message,
        "Unnecessary type check; the result is always 'true'."
    );
    assert_eq!(
        family[3].message,
        "Unnecessary type check; the result is always 'false'."
    );
}

/// The `e!` pair (`visitNullAssertionExpression`,
/// best_practices_verifier.dart:795-799): a strictly-non-nullable
/// operand makes the `!` dead; a `Null`-typed operand (here the
/// literal) always throws. `x!;` in an expression statement is the
/// official's discarded read (`_isUsefulRead`,
/// unused_local_elements_verifier.dart:603-619 — a `NullAssertion` parent
/// under an expression statement never counts), so `x` stays unused and
/// the declaration reports; the `null!` operand is a value row and
/// marks nothing.
#[test]
fn null_assert_pair_routes_by_operand() {
    let r = report("class int {} void main() { int x = 1; x!; null!; }");
    assert_eq!(
        code_names(&r),
        [
            "unused_local_variable",
            "unnecessary_non_null_assertion",
            "null_check_always_fails",
        ],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[1].message,
        "The '!' will have no effect because the receiver can't be null."
    );
    assert_eq!(
        r.diagnostics[2].message,
        "This null-check will always throw an exception because the expression will always evaluate to 'null'."
    );
}

/// `_checkForInvariantNullComparison`
/// (best_practices_verifier.dart:1216-1281): a `null` comparison over
/// a never-null operand reports both orders and both polarities; a
/// nullable operand stays silent.
#[test]
fn unnecessary_null_comparison_orders() {
    let r = report(
        "class int {} void main() { int x = 1; int? y = null; x == null; x != null; null == x; y == null; }",
    );
    assert_eq!(
        code_names(&r),
        [
            "unnecessary_null_comparison",
            "unnecessary_null_comparison",
            "unnecessary_null_comparison"
        ],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "The operand can't be 'null', so the condition is always 'false'."
    );
    assert_eq!(
        r.diagnostics[1].message,
        "The operand can't be 'null', so the condition is always 'true'."
    );
    assert_eq!(
        r.diagnostics[2].message,
        "The operand can't be 'null', so the condition is always 'false'."
    );
}

// —— #743: the W1-micro single-condition cluster ——————————————————————

/// An on-clause whose type sits below an earlier clause's is dead
/// (`_CatchClausesVerifier.nextCatchClause`'s subtype arm,
/// dead_code_verifier.dart:589-604); the message names both types.
/// The supertype clause comes first (`on A`, then `on B`): the
/// verifier reports the CURRENT clause when its type is a subtype of
/// a previous one, so the corpus's original `on B {} on A {}` order
/// is silent in the oracle — expectation corrected, not weakened.
#[test]
fn dead_code_on_catch_subtype_reports() {
    let r = report("class A {} class B extends A {} void main() { try {} on A {} on B {} }");
    assert_eq!(
        code_names(&r),
        ["dead_code_on_catch_subtype"],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "Dead code: This on-catch block won't be executed because 'B' is a subtype of 'A' and hence will have been caught already."
    );
}

/// `Completer<T>.complete(null)` with a non-nullable `T` reports the
/// null-argument row (`NullSafeApiVerifier._checkTypes`); a nullable
/// type argument stays silent. `class int {}` follows the harness
/// convention (core names are modeled through concrete declarations —
/// `BUILTIN_TYPES` carries only `dynamic`), without it every `int`
/// type use reports its own `undefined_class`.
#[test]
fn null_argument_to_non_null_type_reports() {
    let r = report(
        "class int {} class Completer<T> { void complete(int? value) {} } void main() { Completer<int> c = Completer<int>(); c.complete(null); Completer<int?> d = Completer<int?>(); d.complete(null); }",
    );
    assert_eq!(
        code_names(&r),
        ["null_argument_to_non_null_type"],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "'Completer.complete' shouldn't be called with a 'null' argument for the non-nullable type argument 'int'."
    );
    assert_eq!(
        r.diagnostics[0].correction.as_deref(),
        Some("Try adding a non-null argument.")
    );
}

/// The written-argument receiver faces across every binding path
/// (#611 step 5 retirement): parameter and local annotations carry
/// their arguments in the armed head's pooled slot, so the
/// null-argument face reads `int` off each — while the raw receiver's
/// argument-less slot suppresses (the retired side channel's
/// poison/Dynamic ruling: an argument-less shape returns `None`,
/// never a fabricated argument).
#[test]
fn null_argument_reads_written_args_off_every_binding_path() {
    let r = report(
        "class int {} class Completer<T> { void complete(int? value) {} } \
             void parameter(Completer<int> c) { c.complete(null); } \
             void local() { Completer<int> c; c.complete(null); } \
             void raw() { Completer c; c.complete(null); }",
    );
    let rows: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "null_argument_to_non_null_type")
        .collect();
    assert_eq!(rows.len(), 2, "{:?}", code_names(&r));
    for row in rows {
        assert_eq!(
            row.message,
            "'Completer.complete' shouldn't be called with a 'null' argument for the non-nullable type argument 'int'."
        );
    }
}

/// A nullable on-clause type reports (`_checkForNullableTypeInCatchClause`).
#[test]
fn nullable_type_in_catch_clause_reports() {
    let r = report("class int {} void main() { try {} on int? {} }");
    assert_eq!(
        code_names(&r),
        ["nullable_type_in_catch_clause"],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "A potentially nullable type can't be used in an 'on' clause because it isn't valid to throw a nullable expression."
    );
}

/// An unresolved on-clause type codes `non_type_in_catch_clause`, not
/// the resolver's `undefined_class` (the F-P recovered-clause routing;
/// nonTypeInCatchClause, messages.yaml :13748-13753). Two missing
/// clauses in one try each report at their own type name; a resolved
/// on-type beside them stays silent.
#[test]
fn catch_on_type_miss_codes_non_type_in_catch_clause() {
    let src = "class int {} void main() { try {} on Missing {} }";
    let r = report(src);
    assert_eq!(
        code_names(&r),
        ["non_type_in_catch_clause"],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "The name 'Missing' isn't a type and can't be used in an on-catch clause."
    );
    assert_eq!(
        r.diagnostics[0].correction.as_deref(),
        Some("Try correcting the name to match an existing class.")
    );
    assert_eq!(r.diagnostics[0].range.offset, offset_of(src, "Missing"));
    let src2 = "class int {} void main() { try {} on Missing {} on Gone {} on int {} }";
    let r = report(src2);
    assert_eq!(
        code_names(&r),
        ["non_type_in_catch_clause", "non_type_in_catch_clause",],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(r.diagnostics[1].range.offset, offset_of(src2, "Gone"));
}

/// Case 9497f84a: a record-shaped on-clause (`on(e) { }`) routes its
/// field name through the plain type path — `undefined_class`, never
/// `non_type_in_catch_clause` (the records-feature ambiguity recovery:
/// the parenthesized shape parses as a record type, so its fields are
/// ordinary type positions — `record_type_depth` guards the catch face's
/// miss latch). `on("")` reports the same under-typed shape.
#[test]
fn catch_on_record_recovery_codes_undefined_class() {
    let src = "void main() { try {} catch (e) {} on Foo {} on(e) {} on(\"\"); }";
    let r = report(src);
    assert_eq!(
        code_names(&r),
        [
            "dead_code_catch_following_catch",
            "non_type_in_catch_clause",
            "undefined_class",
            "record_type_one_positional_no_trailing_comma",
            "expected_type_name",
            "expected_token",
            "record_type_one_positional_no_trailing_comma",
            "expected_body",
        ],
        "{:?}",
        code_names(&r)
    );
    // `Foo` anchors `non_type_in_catch_clause`; `e` inside the record
    // parens anchors `undefined_class` — the record field never joins
    // the catch face's miss latch.
    assert_eq!(r.diagnostics[1].range.offset, offset_of(src, "Foo {}"));
    // `on(e)` — the record's field `e` sits three bytes past the `o`.
    assert_eq!(r.diagnostics[2].range.offset, offset_of(src, "on(e)") + 3);
}

/// `double.nan` comparisons are constant in both polarities
/// (`checkLeftRight` + `isDoubleNan`).
#[test]
fn unnecessary_nan_comparison_polarities() {
    let r = report(
        "class double { static double get nan => 0; } void main() { double x = 1; if (x == double.nan) {} if (x != double.nan) {} }",
    );
    assert_eq!(
        code_names(&r),
        ["unnecessary_nan_comparison", "unnecessary_nan_comparison",],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "A double can't equal 'double.nan', so the condition is always 'false'."
    );
    assert_eq!(
        r.diagnostics[1].message,
        "A double can't equal 'double.nan', so the condition is always 'true'."
    );
}

/// The m2 corpus face: the receiver is a bare dart:core primitive
/// with no in-file declaration — no indexed container answers `double`,
/// but the getter read still resolves against the class-static face
/// (`isDoubleNan` reads the resolved getter's enclosing element). The
/// report may carry unresolved-name error rows alongside (core
/// library names are deliberately absent here), so only the two
/// comparison rows are counted.
#[test]
fn unnecessary_nan_comparison_primitive_receiver() {
    let r = report("void f(double x) { if (x == double.nan) {} if (x != double.nan) {} }");
    assert_eq!(
        code_names(&r)
            .into_iter()
            .filter(|name| *name == "unnecessary_nan_comparison")
            .count(),
        2,
        "{:?}",
        code_names(&r)
    );
}

/// [`report`] over a library wired to a synthetic `dart:core` face node
/// — the pipeline's SDK shape: surface names declared into an unindexed
/// `<dart:core>` file, joined by the implicit unfiltered unprefixed core
/// import edge ([`build_library_graph`]'s graft). `double` resolves
/// through the namespace to the unindexed declaration — the
/// `BareInfo::Container` reclassification's exact trigger.
fn report_with_core_face(source: &str, core_names: &[&str]) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
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
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let core_lib = builder.intern("dart:core");
    let core_file = files.intern(Path::new("<dart:core>"));
    for name in core_names {
        builder.declare(core_lib, strings.intern(name), core_file);
    }
    builder.import(
        lib,
        ImportEdge {
            target: Some(core_lib),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    diagnose_with(
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
    )
}

/// The full-pipeline face of the m2 corpus fixture: `double` resolves
/// through the implicit core import to the SDK's unindexed `<dart:core>`
/// declaration — the bare receiver classifies as the class-static
/// container face (`isDoubleNan`'s `enclosingElement.name == 'double'`
/// at name level) instead of the `Plain`/`Receiver::Unknown` shape that
/// silenced the rule end-to-end. The oracle's spans run from the
/// operator through the right operand (`== double.nan`).
#[test]
fn unnecessary_nan_comparison_sdk_face() {
    let src = "void f(double x) { if (x == double.nan) {} if (double.nan != x) {} }";
    let r = report_with_core_face(src, &["double"]);
    let rows: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unnecessary_nan_comparison")
        .collect();
    assert_eq!(rows.len(), 2, "{:?}", code_names(&r));
    // The oracle's `reportStartEnd`: the right-NaN face spans the
    // operator through the operand (`== double.nan`), the left-NaN face
    // the operand through the operator (`double.nan !=` — the merged
    // row anchors at the `nan` selector, so the receiver's start comes
    // from the member-read span).
    let expected = ["== double.nan", "double.nan !="];
    for (row, span) in rows.iter().zip(expected) {
        assert_eq!(
            row.range,
            SourceRange {
                offset: offset_of(src, span),
                length: span.len() as u32,
            }
        );
    }
    assert_eq!(
        rows[0].message,
        "A double can't equal 'double.nan', so the condition is always 'false'."
    );
    assert_eq!(
        rows[1].message,
        "A double can't equal 'double.nan', so the condition is always 'true'."
    );
}

/// `dynamic?` and `Null?` gain nothing from the `?`
/// (`visitNamedType`'s question-mark check).
#[test]
fn unnecessary_question_mark_reports() {
    let r = report("class Null {} void main() { dynamic? a; a; Null? b; b; }");
    assert_eq!(
        code_names(&r),
        ["unnecessary_question_mark", "unnecessary_question_mark",],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "The '?' is unnecessary because 'dynamic' is nullable without it."
    );
    assert_eq!(
        r.diagnostics[1].message,
        "The '?' is unnecessary because 'Null' is nullable without it."
    );
}

/// A wildcard pattern as a logical-and operand always matches
/// (`unnecessaryWildcardPattern`, the logical-and kind); both operands
/// report.
#[test]
fn unnecessary_wildcard_pattern_reports() {
    let r = report(
        "class Object {} void main() { Object o = Object(); switch (o) { case _ && _: break; } }",
    );
    assert_eq!(
        code_names(&r),
        [
            "unnecessary_wildcard_pattern",
            "unnecessary_wildcard_pattern",
        ],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(r.diagnostics[0].message, "Unnecessary wildcard pattern.");
}

// —— #745: the W2a warning pair ———————————————————————————————————————

/// `cast_from_null_always_fails` (`visitAsExpression`,
/// best_practices_verifier.dart:180-190): a `Null` operand under a
/// non-nullable target always throws; a nullable target and a
/// non-`Null` operand stay silent. The casts sit in initializers — a
/// statement-position cast drops its leading operand in the parser's
/// statement disambiguation (see the `unnecessary_cast` pin); the
/// `var` targets keep the assignment face out. `0 as int` reports the
/// oracle's `unnecessary_cast` row (structural equality) and nothing
/// from this code.
#[test]
fn cast_from_null_always_fails_routes_by_operand_and_target() {
    let r = report("void main() { var a = null as int; var b = null as int?; var c = 0 as int; }");
    let mut names: Vec<&str> = code_names(&r)
        .into_iter()
        .filter(|name| *name != "cast_to_non_type")
        .collect();
    names.sort_unstable();
    // The oracle emits `dead_code` over the statements following the
    // always-throwing cast (`var b …; var c …`). The bare-unit harness
    // has no SDK face, so each `int` cast target also surfaces
    // `cast_to_non_type` — the CLI pipeline (with the SDK face) keeps
    // it silent, probed identical to the oracle; filtered above.
    assert_eq!(
        names,
        [
            "cast_from_null_always_fails",
            "dead_code",
            "unnecessary_cast",
            "unused_local_variable",
            "unused_local_variable",
            "unused_local_variable",
        ],
        "{:?}",
        code_names(&r)
    );
    let cast = r
        .diagnostics
        .iter()
        .find(|d| d.code.name == "cast_from_null_always_fails")
        .expect("the `null as int` cast always throws");
    assert_eq!(
        cast.message,
        "This cast always throws an exception because the expression always evaluates to 'null'."
    );
    assert_eq!(
        cast.correction.as_deref(),
        None,
        "the registry row declares no correctionMessage"
    );
    // The oracle anchors at the whole AsExpression (`.at(node)`): the
    // operand's start through the cast type's end — `null as int`, not
    // the `as` token.
    assert_eq!(
        cast.range,
        SourceRange {
            offset: offset_of(
                "void main() { var a = null as int; var b = null as int?; var c = 0 as int; }",
                "null as int"
            ),
            length: "null as int".len() as u32,
        }
    );
}

/// The pattern face (`visitCastPattern`,
/// best_practices_verifier.dart:227-239): a `Null` matched value under
/// a non-nullable target reports beside the disjointness row the same
/// pair provably fires (`pattern_never_matches_value_type` — the
/// oracle emits both at the node). The `default` clause keeps the
/// exhaustiveness face out of the pin. The cast target `int` is
/// undeclared in the bare-unit harness — a cast pattern's type reports
/// `undefined_class` (the `castToNonType` arm requires an
/// `AsExpression` parent; a CastPattern falls through to the plain
/// arms, #2184).
#[test]
fn cast_from_null_pattern_reports() {
    let r = report("void main() { switch (null) { case null as int: break; default: break; } }");
    assert_eq!(
        code_names(&r),
        [
            "pattern_never_matches_value_type",
            "cast_from_null_always_fails",
            "undefined_class",
        ],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[1].message,
        "This cast always throws an exception because the expression always evaluates to 'null'."
    );
}

/// `receiver_of_type_never` (messages.yaml :29566, the
/// property-access face of the thirteen collapsed into the receiver
/// junction): `f()`'s result type is `f`'s declared return type, so
/// the `.foo` access on it reports at the receiver, and the receiver's
/// flow exit leaves the call tail dead (probed: dart analyze 3.13.1
/// emits `dead_code` after the receiver row — the dead interval trims
/// to the argument list through the statement's `;`).
/// `class Never {}` follows the harness convention (core names are
/// modeled through concrete declarations; the never-identity keys on
/// the name — see `never_promotion_does_not_clean_the_row`), without
/// it the `Never` annotation itself reports `undefined_class`.
#[test]
fn receiver_of_type_never_reports_at_the_property_junction() {
    let r = report(
        "class Object {} class Never {} Never f() => throw Object(); void main() { f().foo(); }",
    );
    assert_eq!(
        code_names(&r),
        ["receiver_of_type_never", "dead_code"],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "The receiver is of type 'Never', and will never complete with a value."
    );
    assert_eq!(
        r.diagnostics[0].correction.as_deref(),
        Some("Try checking for throw expressions or type errors in the receiver")
    );
    assert_eq!(r.diagnostics[1].message, "Dead code.");
    // The `as` operand face keys on `Null`, not `Never` — no receiver
    // row; the Never operand still leaves the cast type dead inside
    // the initializer (probed: `DEAD_CODE` anchored at the cast type,
    // `Dead code.` — the oracle's row trims through the statement's
    // `;`, a trailing-span fidelity the flow window owns) and the
    // discarded initializer leaves the oracle's
    // `unused_local_variable`.
    let r = report(
        "class Object {} class Never {} Never f() => throw Object(); void main() { var a = f() as int; }",
    );
    // The bare-unit harness has no SDK face, so the undeclared `int`
    // cast target also surfaces `cast_to_non_type` — the CLI pipeline
    // (with the SDK face) keeps it silent, probed identical to the
    // oracle; filtered below.
    let names: Vec<&str> = code_names(&r)
        .into_iter()
        .filter(|name| *name != "cast_to_non_type")
        .collect();
    assert_eq!(
        names,
        ["unused_local_variable", "dead_code"],
        "{:?}",
        code_names(&r)
    );
    let dead = r
        .diagnostics
        .iter()
        .find(|d| d.code.name == "dead_code")
        .expect("dead row present");
    assert_eq!(dead.message, "Dead code.");
}

// —— #746: the W2b flow-fact consumers ——————————————————————————————————

/// `cast_from_nullable_always_fails` (type_analyzer.dart:2107-2126): an
/// unassigned nullable local's cast to a non-nullable target reports at
/// the read's identifier. The cast sits in an initializer on purpose (a
/// statement-position cast drops its leading operand in the parser's
/// statement disambiguation — see the `unnecessary_cast` pin). The
/// assigned read joins no `UnassignedRead` fact and the nullable target
/// fails the staging gate — both stay silent. `class int {}` follows
/// the harness convention (core names are modeled through concrete
/// declarations — `BUILTIN_TYPES` carries only `dynamic`), without it
/// the `int?` annotation reports its own `undefined_class`.
#[test]
fn cast_from_nullable_always_fails_joins_unassigned_reads() {
    let r = report("class int {} void main() { int? x; var y = x as int; }");
    let mut names = code_names(&r);
    names.sort_unstable();
    assert_eq!(
        names,
        ["cast_from_nullable_always_fails", "unused_local_variable",],
        "{:?}",
        code_names(&r)
    );
    let cast = r
        .diagnostics
        .iter()
        .find(|d| d.code.name == "cast_from_nullable_always_fails")
        .expect("the unassigned read's cast always throws");
    assert_eq!(cast.range.offset, 43, "the read's identifier, not the cast");
    assert_eq!(cast.range.length, 1);
    assert_eq!(
        cast.message,
        "This cast will always throw an exception because the nullable local variable 'x' is not assigned."
    );
    assert_eq!(
        cast.correction.as_deref(),
        Some(
            "Try giving it an initializer expression, or ensure that it's assigned on every execution path."
        )
    );
    // The assigned read carries no `UnassignedRead` fact — and it still
    // counts as a use, so `x` is spared: the usage cluster's only row is
    // the never-read `y`.
    let r = report("class int {} void main() { int? x; x = 1; var y = x as int; }");
    let mut names = code_names(&r);
    names.sort_unstable();
    assert_eq!(names, ["unused_local_variable",], "{:?}", code_names(&r));
}

/// `body_might_complete_normally_nullable` (type_analyzer.dart:639-704):
/// a completable block body under a nullable return type reports at the
/// declaring name with the declared type's display text — the top-level
/// and member faces alike (the oracle's `atSourceRange` anchor: the
/// declaring name for every staged shape, probed 3.13.1). The
/// potentially non-nullable ERROR twin
/// (`body_might_complete_normally`) is the never-return code's business
/// and never fires here; the `void` face, the expression body (no
/// `BlockFunctionBody`), and the returning body (its exit is
/// unreachable) stay silent. `class int {}` follows the harness
/// convention (core names are modeled through concrete declarations —
/// `BUILTIN_TYPES` carries only `dynamic`), without it every `int`
/// type use reports its own `undefined_class`.
#[test]
fn body_might_complete_normally_nullable_reports_at_the_declaring_name() {
    let r = report("class int {} int? f() {}");
    assert_eq!(
        code_names(&r),
        ["body_might_complete_normally_nullable",],
        "{:?}",
        code_names(&r)
    );
    let row = &r.diagnostics[0];
    assert_eq!(
        row.range.offset, 18,
        "the declaring name, not the block's left brace or the end token"
    );
    assert_eq!(row.range.length, 1);
    assert_eq!(
        row.message,
        "This function has a nullable return type of 'int?', but ends without returning a value."
    );
    assert_eq!(
        row.correction.as_deref(),
        Some(
            "Try adding a return statement, or if no value is ever returned, try changing the return type to 'void'."
        )
    );
    let r = report("class int {} class C { int? m() {} }");
    assert_eq!(
        code_names(&r),
        ["body_might_complete_normally_nullable",],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(r.diagnostics[0].range.offset, 28);
    // The silent faces: `void`, the expression body, and the
    // returning body (the non-nullable twin reports the
    // COMPILE_TIME_ERROR code — see
    // `body_might_complete_normally_reports_at_the_declaring_name`).
    assert_eq!(
        code_names(&report("class int {} int f() {}")),
        ["body_might_complete_normally"]
    );
    assert_eq!(code_names(&report("void f() {}")), [] as [&str; 0]);
    assert_eq!(
        code_names(&report("class int {} int? f() => null;")),
        [] as [&str; 0]
    );
    assert_eq!(
        code_names(&report("class int {} int? f() { return null; }")),
        [] as [&str; 0]
    );
}

/// The completion warning's modifier gates (`checkForBodyMayComplete
/// Normally`, type_analyzer.dart:658-681): a generator body always
/// completes normally by the generator contract (`body.isGenerator`
/// early return — the official test's own `async*`/`sync*` shapes stay
/// silent), and an `async` body under a return type that cannot carry
/// awaits (`Future<Never> ⊄ int?`, the `_isLegalReturnType` shape)
/// spares the second row. The plain sync twin keeps reporting — the
/// pin's control face. `class Future<T> {}`/`class Iterable<T> {}`/
/// `class int {}` follow the harness convention (core names are
/// modeled through concrete declarations) — the `int` type arguments
/// are type-reference uses like any annotation head, so each reports
/// its own `undefined_class` without the stub.
#[test]
fn body_might_complete_normally_nullable_spares_generator_and_illegal_async() {
    assert_eq!(
        code_names(&report(
            "class int {} class Future<T> {} class Iterable<T> {} \
                 Future<int>? f() async* {} \
                 Iterable<int>? g() sync* {}"
        )),
        [] as [&str; 0]
    );
    assert_eq!(
        code_names(&report("class int {} int? h() async {}")),
        [] as [&str; 0]
    );
    assert_eq!(
        code_names(&report("class int {} int? k() {}")),
        ["body_might_complete_normally_nullable"]
    );
}

/// An `async` body's completion verdict reads the declared return's
/// future-flattened face (`TypeSystem.flatten`,
/// type_analyzer.dart:682-704), not the `Future<...>` head: a
/// `Future<void>` async body flattens to `void` and stays silent, a
/// `FutureOr<int>` async body flattens to the potentially non-nullable
/// `int` and reports. The declared `Future<int>` head itself is
/// non-nullable — reading it raw would over-fire (the
/// `notifications/audit_log.dart` `record()` row).
#[test]
fn body_might_complete_normally_reads_the_async_flattened_return() {
    assert_eq!(
        code_names(&report("class Future<T> {} Future<void> f() async {}")),
        [] as [&str; 0],
        "{:?}",
        code_names(&report("class Future<T> {} Future<void> f() async {}"))
    );
    assert_eq!(
        code_names(&report(
            "class int {} class Future<T> {} class FutureOr<T> {} \
                 FutureOr<int> g() async {}"
        )),
        ["body_might_complete_normally"],
        "{:?}",
        code_names(&report(
            "class int {} class Future<T> {} class FutureOr<T> {} \
                 FutureOr<int> g() async {}"
        ))
    );
}

/// The COMPILE_TIME_ERROR twin of the same walk
/// (`isPotentiallyNonNullable`'s pick, type_analyzer.dart:688): a
/// potentially non-nullable return type — `Never` included — reports
/// `body_might_complete_normally` at the declaring name (the corpus
/// `vendor/test_api` `Never`-returning `expect` face).
#[test]
fn body_might_complete_normally_reports_at_the_declaring_name() {
    let r = report("class int {} int f() {}");
    assert_eq!(
        code_names(&r),
        ["body_might_complete_normally"],
        "{:?}",
        code_names(&r)
    );
    let row = &r.diagnostics[0];
    assert_eq!(
        row.range.offset, 17,
        "the declaring name 'f', not the block brace"
    );
    assert_eq!(row.range.length, 1);
    assert_eq!(
        row.message,
        "The body might complete normally, causing 'null' to be returned, but the return type, 'int', is a potentially non-nullable type."
    );
    assert_eq!(
        row.correction.as_deref(),
        Some("Try adding either a return or a throw statement at the end.")
    );
    // The `Never` face — the vendored `expect` shape.
    let r = report("class Never {} class String {} Never expect(String m) {}");
    assert_eq!(
        code_names(&r),
        ["body_might_complete_normally"],
        "{:?}",
        code_names(&r)
    );
}

/// The switch-end `isExhaustive` arm (flow_analysis.dart `switch_end`,
/// type_analyzer.dart:638-712 reads `isReachable` at the body end): a
/// `default`-less statement switch over a sealed scrutinee whose cases
/// all return leaves the body end unreachable — silent; the same shape
/// over a non-exhaustive scrutinee still reports (#2134).
#[test]
fn body_might_complete_normally_respects_an_exhaustive_switch() {
    let r = report(concat!(
        "class int {} sealed class S {} class A extends S {} class B extends S {}",
        " int f(S s) { switch (s) { case A _: return 0; case B _: return 1; } }"
    ));
    assert!(
        !code_names(&r).contains(&"body_might_complete_normally"),
        "{:?}",
        code_names(&r)
    );
    let r = report(concat!(
        "class int {} class S {} class A extends S {} class B extends S {}",
        " int f(S s) { switch (s) { case A _: return 0; case B _: return 1; } }"
    ));
    assert!(
        code_names(&r).contains(&"body_might_complete_normally"),
        "{:?}",
        code_names(&r)
    );
}

/// `_checkReadOfNotAssignedLocalVariable`'s late arm
/// (type_analyzer.dart:4999-5008): a `late` local read before every
/// assignment reports at the read; a read after a partial-path
/// assignment (potentially, not definitely, unassigned) stays silent.
#[test]
fn definitely_unassigned_late_local_variable_reports_at_the_read() {
    let late = |r: &FileReport| -> Vec<(u32, String)> {
        r.diagnostics
            .iter()
            .filter(|d| d.code.name == "definitely_unassigned_late_local_variable")
            .map(|d| (d.range.offset, d.message.to_string()))
            .collect()
    };
    let r = report("class int {} void f() { late int x; try { x; x = 1; } finally { } }");
    assert_eq!(
        late(&r),
        [(
            42,
            "The late local variable 'x' is definitely unassigned at this point.".to_string()
        )]
    );
    let r =
        report("class int {} class bool {} void f(bool b) { late int x; if (b) { x = 1; } x; }");
    assert!(late(&r).is_empty(), "{:?}", code_names(&r));
}

/// The operand's resolved static type decides `x is T`
/// (`leftNode.typeOrThrow`, best_practices_verifier.dart:1011): a
/// `var`-inferred local reads its initializer's type, so the
/// `var self = this` face over the own container is constant-true and
/// mutation plays no part in the verdict; a non-matching pair stays
/// informative (the upstream `test_noDiagnostic_mutated` shape's
/// `self is D` — unrelated types).
#[test]
fn unnecessary_type_check_reads_the_inferred_var_type() {
    let r = report(
        "class TAMutated { void m() { var self = this; if (self is TAMutated) {} self = TAMutated(); } }",
    );
    assert!(
        r.diagnostics
            .iter()
            .any(|d| d.code.name == "unnecessary_type_check"),
        "{:?}",
        code_names(&r)
    );
    let silent = report(
        "class C { void m() { var self = this; if (self is D) {} self = C(); } } class D extends C {}",
    );
    assert!(
        silent
            .diagnostics
            .iter()
            .all(|d| d.code.name != "unnecessary_type_check"),
        "{:?}",
        code_names(&silent)
    );
}

/// The required-named coverage (`RequiredParametersVerifier._check`,
/// required_parameters_verifier.dart:123-156): a constructor invoked
/// without a `required` named parameter reports
/// `missing_required_argument` at the callee entity (the corpus
/// `KMissing({required this.children})` shape).
#[test]
fn missing_required_argument_reports_at_the_invocation() {
    let r = report(
        "class List<T> {} class KMissing { const KMissing({required this.children}); final List<KMissing> children; } void f() { KMissing(); }",
    );
    let rows: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "missing_required_argument")
        .collect();
    assert_eq!(rows.len(), 1, "{:?}", code_names(&r));
    assert_eq!(
        rows[0].message,
        "The named parameter 'children' is required, but there's no corresponding argument."
    );
    // The supplied face stays silent.
    let silent = report(
        "class List<T> {} class KMissing { const KMissing({required this.children}); final List<KMissing> children; } void f() { KMissing(children: []); }",
    );
    assert!(
        silent
            .diagnostics
            .iter()
            .all(|d| d.code.name != "missing_required_argument"),
        "{:?}",
        code_names(&silent)
    );
}

// —— #507: the pattern/type-space disjointness cluster —————————————————

/// A cast pattern over a matched type already below the target is dead
/// (shared_type_analyzer.dart:145-152): `Object?` matched, `Object?`
/// cast.
#[test]
fn unnecessary_cast_pattern_reports() {
    let r =
        report("class Object {} void f(Object? o) { switch (o) { case _ as Object?: break; } }");
    assert_eq!(
        code_names(&r),
        ["unnecessary_cast_pattern"],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(r.diagnostics[0].message, "Unnecessary cast pattern.");
}

/// A null-check/null-assert pattern over a non-nullable matched type
/// is dead (shared_type_analyzer.dart:129-139); over a nullable one
/// both stay silent.
#[test]
fn null_check_and_assert_patterns_report() {
    let r = report(
        "class int {} void f(int n) { switch (n) { case _?: break; } } \
             void g(int n) { switch (n) { case _!: break; } } \
             void h(int? n) { switch (n) { case _?: break; } } \
             void k(int? n) { switch (n) { case _!: break; } }",
    );
    assert_eq!(
        code_names(&r),
        [
            "unnecessary_null_check_pattern",
            "unnecessary_null_assert_pattern"
        ],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "The null-check pattern will have no effect because the matched type isn't nullable."
    );
    assert_eq!(
        r.diagnostics[1].message,
        "The null-assert pattern will have no effect because the matched type isn't nullable."
    );
}

/// The enum-left closure decides `pattern_never_matches_value_type`
/// (`checkPatternNeverMatchesValueType` via `canBeSubtypeOf`): no `E`
/// value can be an `A`, while the same-enum constant patterns stay
/// silent — and the fully-covered switch emits nothing else.
#[test]
fn pattern_never_matches_on_closed_left() {
    let r = report(
        "enum E { a, b } class A {} void f(E e) { switch (e) { case A _: break; case E.a: break; case E.b: break; } }",
    );
    assert_eq!(
        code_names(&r),
        ["pattern_never_matches_value_type"],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "The matched value type 'E' can never match the required type 'A'."
    );
}

// —— #1314 FAM7: the optional-parameter default-value cluster ————————

/// `missing_default_value_for_parameter`
/// (error_verifier.dart:8327-8419): an optional parameter without a
/// default over a potentially-non-nullable written type reports at the
/// parameter's name token; the named face and the positional face
/// split by the parameter group.
#[test]
fn missing_default_value_reports() {
    let src = "void f([int a]) {} void g({int b}) {}";
    let r = report(src);
    let rows: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "missing_default_value_for_parameter")
        .collect();
    assert_eq!(rows.len(), 2, "{:?}", code_names(&r));
    assert_eq!(
        rows[0].message,
        "The parameter 'a' can't have a value of 'null' because of its type, but the implicit default value is 'null'."
    );
    assert_eq!(
        rows[0].correction.as_deref(),
        Some("Try adding an explicit non-'null' default value.")
    );
    assert_eq!(
        rows[1].message,
        "The parameter 'b' can't have a value of 'null' because of its type, but the implicit default value is 'null'."
    );
    assert_eq!(
        rows[1].correction.as_deref(),
        Some("Try adding either an explicit non-'null' default value or the 'required' modifier.")
    );
    assert_eq!(rows[0].range.offset as usize, 12);
    assert_eq!(rows[1].range.offset as usize, 31);
}

/// The default-value cluster's silence faces: an explicit default, a
/// nullable written type, `dynamic`, a required positional, an untyped
/// formal, an in-scope type parameter's head (the bound decides the
/// resolved nullability the name-level model does not keep), an
/// abstract method's `;` body, and a function-typed context all stay
/// silent (error_verifier.dart:8327-8372 + type_system.dart:1257).
#[test]
fn missing_default_value_silence_faces() {
    let r = report(
        "void a([int b = 1]) {} void c([int? d]) {} void e([dynamic f]) {} \
             void g(int h) {} void i([j]) {} \
             void k<T>([T l]) {} \
             class C { void m([int n]); } \
             typedef F = void Function([int o]);",
    );
    let rows: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "missing_default_value_for_parameter")
        .collect();
    assert!(rows.is_empty(), "{:?}\n{:?}", rows, code_names(&r));
}

/// `obsolete_colon_for_default_value`
/// (best_practices_verifier.dart:1065-1082): a named optional's
/// `:`-separated default reports at the colon; the `=` separator and
/// the positional-optional colon (a parser-recovery face) stay silent.
#[test]
fn obsolete_colon_default_value_reports() {
    let src = "void f({int a: 1}) {} void g({int b = 1}) {}";
    let r = report(src);
    let rows: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "obsolete_colon_for_default_value")
        .collect();
    assert_eq!(rows.len(), 1, "{:?}", code_names(&r));
    assert_eq!(
        rows[0].message,
        "Using a colon as the separator before a default value is no longer supported."
    );
    assert_eq!(
        rows[0].correction.as_deref(),
        Some("Try replacing the colon with an equal sign.")
    );
    assert_eq!(rows[0].range.offset as usize, 13);
}

/// `non_type_as_type_argument` (named_type_resolver.dart:720-770):
/// an unresolvable name inside a type-argument list reports the
/// context face instead of `undefined_class`; the creation-head face
/// (`newWithNonType`, resolveInstanceCreationExpression) takes the
/// `new` form's head.
#[test]
fn type_argument_context_replaces_undefined_class() {
    // Type-argument position: `List<Undefined>` in a parameter type. The
    // `class List<T> {}` stub is the harness's core stand-in (#537): the
    // head `List` would otherwise land as a second `TypeUse(Missing)` row
    // — an `undefined_class` at pass 2 that crowds the context-replace
    // pin (the oracle resolves `List` through dart:core).
    let stub = "class List<T> {} ";
    let src = "void f(List<Undefined> a) {}";
    let src = format!("{stub}{src}");
    let r = report(&src);
    assert_eq!(code_names(&r), ["non_type_as_type_argument"],);
    let d = &r.diagnostics[0];
    assert_eq!(
        d.message,
        "The name 'Undefined' isn't a type, so it can't be used as a type argument."
    );
    assert_eq!(
        d.correction.as_deref(),
        Some("Try correcting the name to an existing type, or defining a type named 'Undefined'.")
    );
    let shift = u32::try_from(stub.len()).unwrap();
    assert_eq!(d.range.offset, 12 + shift);
    assert_eq!(d.range.length as usize, 9);
}

/// The creation-head face: `new Undefined()` — an unresolved head name
/// reports `creation_with_non_type` ("The name 'Undefined' isn't a
/// class."), not `undefined_class` (upstream Dart SDK source,
/// pkg/analyzer/lib/src/dart/resolver/named_type_resolver.dart:313-330
/// — `resolveConstructorTypeReference`'s tail reports
/// `newWithNonType` for a missing element; `undefined_class` is the
/// plain type path's fallthrough, a different entry point). `x` still
/// reports unused (the oracle's unused-local face is independent).
#[test]
fn creation_head_missing_reports_new_with_non_type() {
    let src = "void f() { var x = new Undefined(); }";
    let r = report(src);
    assert_eq!(
        code_names(&r),
        ["unused_local_variable", "creation_with_non_type"],
        "{:?}",
        code_names(&r)
    );
    let d = &r.diagnostics[1];
    assert_eq!(d.message, "The name 'Undefined' isn't a class.");
    assert_eq!(
        d.correction.as_deref(),
        Some("Try correcting the name to match an existing class.")
    );
    assert_eq!(d.range.offset as usize, 23);
}

/// #1314 FAM10 (`inferred_type_arguments` corpus): a call argument to
/// a bare type-parameter parameter checks against the parameter's
/// solution. An unbounded parameter solves to the argument's own type
/// (`null` solves `T` to `Null` — no report); a bounded parameter's
/// solution clamps to the bound when it violates it
/// (`f<T extends num>(null)` reports against `num`, the solver's
/// bound-respecting solution).
#[test]
fn type_parameter_argument_solves_and_clamps() {
    // Unbounded: the argument's own type names T — everything assigns.
    let src = "void f<T>(T t) {} void g() { f(null); f(0); f('x'); }";
    let r = report(src);
    assert!(r.diagnostics.is_empty(), "{:?}", code_names(&r));
    // Bounded: `null` and `String` violate `num`; the reports display
    // the bound. `class num {}` follows the harness convention (core
    // names are modeled through concrete declarations — an unmodeled
    // bound leaves the clamp's assignability undecidable, which
    // suppresses).
    let src = "class num {} void f<T extends num>(T t) {} void g() { f(null); f('x'); f(0); }";
    let r = report(src);
    let rows: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "argument_type_not_assignable")
        .collect();
    assert_eq!(rows.len(), 2, "{:?}", code_names(&r));
    // Trailing space per the oracle template (empty record appendix,
    // upstream Dart SDK source, diagnostic.g.dart:641-651).
    assert_eq!(
        rows[0].message,
        "The argument type 'Null' can't be assigned to the parameter type 'num'. "
    );
    assert_eq!(
        rows[1].message,
        "The argument type 'String' can't be assigned to the parameter type 'num'. "
    );
}

/// The downward face: a call whose return type is exactly a type
/// parameter solves it from the surrounding declaration's type
/// (`int x = f(0.5)` — the oracle's display reads `int`, and the
/// argument checks against it).
#[test]
fn return_type_parameter_solves_from_context() {
    // `class int {} class double {} class num {}` follow the harness
    // convention (core names are modeled through concrete declarations):
    // the solved check targets must resolve, or the assignability pair
    // stays undecidable and the row suppresses.
    let src = "class int {} class double {} class num {}\n\
                   T f<T>(T t) => t;\nvoid g() { int x = f(0.5); num y = f('x'); }";
    let r = report(src);
    let rows: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "argument_type_not_assignable")
        .collect();
    assert_eq!(rows.len(), 2, "{:?}", code_names(&r));
    // Trailing space per the oracle template (empty record appendix,
    // upstream Dart SDK source, diagnostic.g.dart:641-651).
    assert_eq!(
        rows[0].message,
        "The argument type 'double' can't be assigned to the parameter type 'int'. "
    );
    assert_eq!(
        rows[1].message,
        "The argument type 'String' can't be assigned to the parameter type 'num'. "
    );
}

/// The bound clamp respects a context solution that fits: `int`
/// solves a `T extends num` parameter (no clamp), and the argument
/// checks against `int`.
#[test]
fn bounded_context_solution_keeps_context_type() {
    let src = "T f<T extends num>(T t) => t;\nvoid g() { int x = f(0.5); }";
    let r = report(src);
    let rows: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "argument_type_not_assignable")
        .collect();
    assert_eq!(rows.len(), 1, "{:?}", code_names(&r));
    // Trailing space per the oracle template (empty record appendix,
    // upstream Dart SDK source, diagnostic.g.dart:641-651).
    assert_eq!(
        rows[0].message,
        "The argument type 'double' can't be assigned to the parameter type 'int'. "
    );
}

/// The inference-failure face (`couldNotInfer`, generic_inferrer.dart
/// `tryChooseFinalTypes`): the creation's return constraint against
/// the outer formal's `C<List<_>>` schema solves `X := List<Object?>`,
/// which the function literal's `void Function(List<int>)` violates.
/// Oracle-pinned (3.13.1): the row sits on the constructor name with
/// the exact multi-line detail.
#[test]
fn could_not_infer_reports_return_solved_violation() {
    let src = "class C<X> {\n  C(void Function(X) x);\n}\n\
                   T check<T>(C<List<T>> f) => throw 0;\n\
                   void main() {\n  var x = check(C((List<int> x) {}));\n  print(x);\n}\n";
    let r = report(src);
    let rows: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "could_not_infer")
        .collect();
    assert_eq!(rows.len(), 1, "{:?}", code_names(&r));
    assert_eq!(
        &src[rows[0].range.offset as usize..rows[0].range.end() as usize],
        "C"
    );
    assert_eq!(
        rows[0].message,
        "Couldn't infer type parameter 'X'.\n\n\
             Tried to infer 'List<Object?>' for 'X' which doesn't work:\n  \
             Parameter 'x' declared as     'void Function(X)'\n                \
             but argument is 'void Function(List<int>)'.\n\
             The type 'List<Object?>' was inferred from:\n  \
             Return type declared as 'C<X>'\n              \
             used where  'C<List<_>>' is required.\n\n\
             Consider passing explicit type argument(s) to the generic.\n\n"
    );
}

/// The satisfiable and context-fixed neighbours stay silent (all
/// oracle-clean for `could_not_infer`): a literal accepting the solved
/// `List<Object?>`, explicit type arguments on the outer call, an
/// annotated declaration (its context fixes `T`), an enclosing
/// argument, and an untyped literal formal.
#[test]
fn could_not_infer_stays_silent_when_satisfied_or_fixed() {
    let head = "class C<X> {\n  C(void Function(X) x);\n}\n\
                    T check<T>(C<List<T>> f) => throw 0;\nvoid sink(int v) {}\n";
    for body in [
        "var a = check(C((List<Object?> x) {}));",
        "var b = check<int>(C((List<int> x) {}));",
        "int c = check(C((List<int> x) {}));",
        "sink(check(C((List<int> x) {})));",
        "var e = check(C((x) {}));",
    ] {
        let src = format!("{head}void main() {{\n  {body}\n}}\n");
        let r = report(&src);
        assert!(
            !code_names(&r).contains(&"could_not_infer"),
            "{body}: {:?}",
            code_names(&r)
        );
    }
}

/// #1314 FAM14 (scope_analyzer.dart `_resolveType`'s clause switch):
/// an unresolved clause name reports the clause face — `implements`
/// and `with` replace `undefined_class`, the alias superclass reports
/// `mixin_with_non_class_superclass`, and the mixin `on` constraint is
/// additive (the oracle keeps `undefined_class` beside it).
#[test]
fn clause_names_report_clause_faces() {
    // implements.
    let src = "class C implements Missing {}";
    let r = report(src);
    assert_eq!(
        code_names(&r),
        ["implements_non_class"],
        "{:?}",
        code_names(&r)
    );
    let d = &r.diagnostics[0];
    assert_eq!(
        d.message,
        "Classes and mixins can only implement other classes and mixins."
    );
    assert_eq!(
        d.correction.as_deref(),
        Some("Try specifying a class or mixin, or remove the name from the list.")
    );
    assert_eq!(d.range.offset as usize, 19);
    // with. `class Object {}` follows the harness convention (#537): the
    // superclass resolves through a concrete declaration, its
    // `undefined_class` stays out of the with-face pin.
    let src = "class Object {} class C extends Object with Missing {}";
    let r = report(src);
    assert_eq!(
        code_names(&r),
        ["mixin_of_non_class"],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "Classes can only mix in mixins and classes."
    );
    // Named mixin application: the superclass face, then the with face.
    let src = "class C = Missing with M;";
    let r = report(src);
    assert_eq!(
        code_names(&r),
        ["mixin_with_non_class_superclass", "mixin_of_non_class"],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(r.diagnostics[0].range.offset as usize, 10);
    assert_eq!(r.diagnostics[1].range.offset as usize, 23);
    // The additive on constraint.
    let src = "mixin M on Missing {}";
    let r = report(src);
    let mut names = code_names(&r);
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "mixin_super_class_constraint_non_interface",
            "undefined_class"
        ],
        "{:?}",
        code_names(&r)
    );
    assert!(r.diagnostics.iter().all(|d| d.range.offset as usize == 11));
}

/// #1444 (named_type_resolver.dart `reportNullOrNonTypeElement`'s
/// clause-parent guard — "Ignored. The error will be reported
/// elsewhere"; ast_builder.dart `endExtensionTypeDeclaration` pops
/// only the implements clause): an extension-type header's clause
/// names never report `undefined_class` — whichever clause they stand
/// in, however many entries the clause carries, and also in the
/// recovery positions (a clause written after an illegal one, or a
/// duplicate). The representation's own type keeps the ordinary
/// written-type face.
#[test]
fn extension_type_clause_names_stay_off_undefined_class() {
    // The `int` stubs keep the representation types themselves defined
    // — the representation keeps the ordinary written-type face (the
    // closing block pins it), so an undefined representation name
    // would defeat the clause filter below.
    for src in [
        "class int {} extension type E(int i) extends Missing {}",
        "class int {} extension type E(int i) with Missing {}",
        "class int {} extension type E(int i) with Missing, AlsoMissing {}",
        "class int {} extension type E(int i) implements Missing {}",
        "class int {} extension type E(int i) implements Missing, AlsoMissing {}",
        "class int {} extension type E(int i) extends Missing with AlsoMissing implements Third {}",
        "class int {} extension type E(int i) implements Missing extends AlsoMissing {}",
        "class int {} extension type E(int i) extends Missing implements AlsoMissing {}",
        "class int {} extension type E(int i) implements Missing implements AlsoMissing {}",
        // The `;`-body shape: no representation, the clause still
        // silent.
        "extension type E;",
    ] {
        let r = report(src);
        let undefined: Vec<_> = r
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "undefined_class")
            .collect();
        assert!(undefined.is_empty(), "{:?} -> {:?}", src, code_names(&r));
    }
    // The clause path itself was exercised: the illegal-clause keyword
    // faces land (the parser owns them), and the argument position of
    // a parameterized clause head is not an entry (the head captures
    // once, at its re-fire).
    let r = report("class int {} extension type E(int i) extends Missing implements G<int> {}");
    assert_eq!(
        code_names(&r),
        ["extension_type_extends"],
        "{:?}",
        code_names(&r)
    );
    // The representation type is not a clause position: its miss keeps
    // the ordinary face at the type's own offset.
    let r = report("extension type E(Missing m) implements AlsoMissing {}");
    let undefined: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_class")
        .collect();
    assert_eq!(undefined.len(), 1, "{:?}", code_names(&r));
    assert_eq!(undefined[0].range.offset as usize, 17);
}

/// #1314 FAM18 (error_verifier.dart `_checkForNotInitializedField`
/// family): a non-nullable, non-final, non-late, non-const declaration
/// without an initializer reports at its name — the variable code for
/// top-level variables and static fields, the instance-field code when
/// the declaring container has no generative constructor.
#[test]
fn uninitialized_non_nullable_reports() {
    let src = "class int {}\nint x1;\nclass Foo {\n  int z1;\n}";
    let r = report(src);
    let rows: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name.starts_with("not_initialized"))
        .collect();
    assert_eq!(rows.len(), 2, "{:?}", code_names(&r));
    assert_eq!(
        rows[0].message, "The non-nullable variable 'x1' must be initialized.",
        "{:?}",
        rows[0]
    );
    assert_eq!(rows[0].range.offset as usize, 17);
    assert_eq!(
        rows[1].message, "Non-nullable instance field 'z1' must be initialized.",
        "{:?}",
        rows[1]
    );
    assert_eq!(rows[1].range.offset as usize, 39);
}

/// The gates and the constructor-owned half: `late`/`const` groups,
/// initializers, an untyped/nullable type stay silent, while a
/// generative constructor moves its uninitialized non-final fields'
/// rows onto the constructor span (`ConstructorFieldsVerifier` —
/// `int`/`final` modeled concretely; the harness has no core names).
/// The oracle's `final` faces (`final_not_initialized` at the
/// top-level `b`, `final_not_initialized_constructor` beside the
/// instance row) are pinned in full.
#[test]
fn uninitialized_non_nullable_gates_stay_silent() {
    let src = "class int {}\nlate int a;\nfinal int b;\nconst int c = 1;\nint d = 2;\nvar e;\nint? f;\n\
                   class C {\n  late int g;\n  final int h;\n  int i = 3;\n  int j;\n  C(this.i);}";
    let r = report(src);
    let rows: Vec<(&str, u32)> = r
        .diagnostics
        .iter()
        .map(|d| (d.code.name, d.range.offset))
        .collect();
    assert_eq!(
        rows,
        [
            // `final int b;` — the declaration loop's final face.
            ("final_not_initialized", 35),
            // The constructor owns the remaining fields: the final
            // variant first, then the non-final non-nullable `j` —
            // both at `C`'s constructor span (the unnamed spelling's
            // class-name token).
            ("final_not_initialized_constructor", 144),
            ("not_initialized_non_nullable_instance_field", 144),
        ],
        "{:?}",
        code_names(&r)
    );
}

/// A static field reports the variable code and skips the constructor
/// gate (a generative constructor never initializes it). `int` is
/// modeled concretely (the harness has no core names).
#[test]
fn static_field_reports_variable_code() {
    let src = "class int {}\nclass C {\n  static int s;\n}";
    let r = report(src);
    let rows: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name.starts_with("not_initialized"))
        .collect();
    assert_eq!(rows.len(), 1, "{:?}", code_names(&r));
    assert_eq!(rows[0].code.name, "not_initialized_non_nullable_variable");
    assert_eq!(rows[0].range.offset as usize, 36);
}

/// `constant_pattern_never_matches_value_type`
/// (constant_verifier.dart:140-165): a `String` constant can never
/// equal an `int` matched value; the int-constant-vs-double-value
/// cross answers "possible" (the official's JavaScript note). The
/// `case null` shape's body also dies (the flow equality-check face,
/// oracle probe: `DEAD_CODE` over the `break;`).
#[test]
fn constant_pattern_never_matches_pins() {
    let r = report(
        "class int {} class double {} class String {} \
             void f(int n) { switch (n) { case 'x': break; } } \
             void g(double d) { switch (d) { case 1: break; } } \
             void h(int n) { switch (n) { case null: break; } }",
    );
    assert_eq!(
        code_names(&r),
        [
            "constant_pattern_never_matches_value_type",
            "constant_pattern_never_matches_value_type",
            "dead_code",
        ],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "The matched value type 'int' can never be equal to this constant of type 'String'."
    );
    assert_eq!(
        r.diagnostics[1].message,
        "The matched value type 'int' can never be equal to this constant of type 'Null'."
    );
    assert_eq!(
        r.diagnostics[2].message, "Dead code.",
        "{:?}",
        r.diagnostics[2].message
    );
}

// —— #480: the analyzer language-strictness modes ——————————————————

/// `strict-casts` rejects the `dynamic`-source downcasts the default
/// posture silently allows, at every verdict site the oracle gates
/// through `isAssignableTo` (`type_system.dart:891-930`: the mode
/// "disables all downcasts, including casts from `dynamic`") — the
/// ordinary assignability codes mint, never a strict-casts code.
///
/// Hand-trace (mode on): `return d` in `int g` answers the return
/// matrix's dynamic arm (`Some(false)` — `int` is no top type),
/// `f(d)` and `int y = d` answer [`Engine::assignable`]'s overlay; mode
/// off all three are `Some(true)` and the file is clean.
#[test]
fn strict_casts_rejects_dynamic_downcasts_at_every_verdict_site() {
    let src = "class int {} \
             void f(int x) {} \
             int g(dynamic d) { return d; } \
             void main() { dynamic d; f(d); int y = d; g(y); }";
    let on = report_strict(
        src,
        StrictModes {
            casts: true,
            ..StrictModes::default()
        },
    );
    assert_eq!(
        code_names(&on),
        [
            "return_of_invalid_type",
            "argument_type_not_assignable",
            "invalid_assignment",
        ],
        "{:?}",
        code_names(&on)
    );
    assert_eq!(
        on.diagnostics[1].message,
        "The argument type 'dynamic' can't be assigned to the parameter type 'int'. "
    );
    assert_eq!(
        on.diagnostics[2].message,
        "A value of type 'dynamic' can't be assigned to a variable of type 'int'."
    );
    // The default posture: every dynamic downcast is silent.
    assert!(report(src).diagnostics.is_empty());
}

/// The strict-casts boundaries: `dynamic` and `void` stay top types
/// (assignable from `dynamic` even under the mode), while a nullable
/// named target is still a rejected downcast — at the assignment and
/// the argument call site alike. `d` is a local: the collector records
/// a top-level variable's annotation as written (`Ty::named("dynamic")`,
/// the element model's as-written shape), which the overlay's
/// canonical-source predicate cannot see; the local face types it
/// `Ty::Dynamic`. The fixture carries no `return` statement, so a
/// return verdict is structurally impossible, and both locals are read
/// (`f(y)`), so no unused-local row mints.
#[test]
fn strict_casts_keeps_the_top_types_and_rejects_nullable_targets() {
    let src = "class int {} \
             void f(int? x) {} \
             void h(dynamic x) {} \
             void t(void x) {} \
             void main() { dynamic d; int? y = d; f(d); h(d); t(d); f(y); }";
    let on = report_strict(
        src,
        StrictModes {
            casts: true,
            ..StrictModes::default()
        },
    );
    assert_eq!(
        code_names(&on),
        [
            // `int? y = d` is the assignment-site downcast, `f(d)` the
            // argument-site one; `h(d)` and `t(d)` assign into the
            // `dynamic`/`void` top types and stay clean.
            "invalid_assignment",
            "argument_type_not_assignable",
        ],
        "{:?}",
        code_names(&on)
    );
    assert!(report(src).diagnostics.is_empty());
}

/// The collector element model canonicalizes the built-in spellings: a
/// TOP-LEVEL `dynamic d;` reads `Ty::Dynamic` (not the as-written
/// `Named`), so the strict-casts overlay sees it at every verdict site —
/// the assignment and the argument — while the `h(d)` call into the
/// canonical-`dynamic` formal target stays clean. A regression to the
/// as-written shape mutes the two rows (the overlay's canonical-source
/// predicate) and mints a third one (the `Named` target rejection).
#[test]
fn strict_casts_sees_a_top_level_dynamic_variable_canonically() {
    let src = "class int {} \
             void f(int? x) {} \
             void h(dynamic x) {} \
             dynamic d; \
             void main() { int? y = d; f(d); h(d); f(y); }";
    let on = report_strict(
        src,
        StrictModes {
            casts: true,
            ..StrictModes::default()
        },
    );
    assert_eq!(
        code_names(&on),
        ["invalid_assignment", "argument_type_not_assignable",],
        "{:?}",
        code_names(&on)
    );
    assert!(report(src).diagnostics.is_empty());
}

/// `strict-raw-types` reports a group-less generic type annotation whose
/// declaring container keeps at least one unbounded type parameter (the
/// oracle fires exactly when a type argument instantiates to `dynamic`,
/// `_isMissingTypeArguments`, `type_arguments_verifier.dart:711-735`);
/// explicit type arguments and fully-bounded containers stay clean, and
/// the mode is off by default. The `{0}` display is the oracle's
/// resolved-type face — the instantiated-to-bounds form
/// (`Box<dynamic>`, the `instantiateToBounds` projection) — and the
/// correction template reuses `{0}` (diagnostic.g.dart:15797-15801).
#[test]
fn strict_raw_types_reports_the_unbounded_generic_only() {
    let src = "class int {} \
             class Box<T> {} \
             class Bound<T extends int> {} \
             void f(Box a) {} \
             void g(Box<int> b) {} \
             void h(Bound c) {}";
    let on = report_strict(
        src,
        StrictModes {
            raw_types: true,
            ..StrictModes::default()
        },
    );
    assert_eq!(
        code_names(&on),
        ["strict_raw_type"],
        "{:?}",
        code_names(&on)
    );
    assert_eq!(
        on.diagnostics[0].message,
        "The generic type 'Box<dynamic>' should have explicit type arguments but doesn't."
    );
    assert_eq!(
        on.diagnostics[0].correction.as_deref(),
        Some("Use explicit type arguments for 'Box<dynamic>'.")
    );
    assert_eq!(on.diagnostics[0].severity, Severity::Warning);
    assert!(report(src).diagnostics.is_empty());
}

/// The strict-raw-types boundaries: a raw type nested in type arguments
/// reports (the oracle's `parentEscapingTypeArguments` walk only exempts
/// the noisy parents), `is`/`as` operands never report, and an
/// in-scope type parameter shadows a same-named generic container.
#[test]
fn strict_raw_types_honors_the_nesting_and_suppression_faces() {
    let src = "class Box<T> {} \
             class Object {} \
             class List<T> {} \
             void nested(List<Box> x) {} \
             void suppress(Object o) { if (o is Box) { o as Box; } } \
             void shadow<Box>(Box p) {}";
    let on = report_strict(
        src,
        StrictModes {
            raw_types: true,
            ..StrictModes::default()
        },
    );
    assert_eq!(
        code_names(&on),
        // `nested`'s `Box` argument is the one raw use; the `is`/`as`
        // operands sit in the suppression window and `shadow`'s `Box`
        // resolves to its own type parameter.
        ["strict_raw_type"],
        "{:?}",
        code_names(&on)
    );
}

/// A raw generic bound reports too (`class W<T extends Box>` — the
/// bound is a `NamedType` the oracle checks; the window's bound count and
/// the raw verdict run off the same event without confusing each
/// other).
#[test]
fn strict_raw_types_reports_a_raw_bound() {
    let src = "class Box<T> {} class W<T extends Box> {}";
    let on = report_strict(
        src,
        StrictModes {
            raw_types: true,
            ..StrictModes::default()
        },
    );
    assert_eq!(
        code_names(&on),
        ["strict_raw_type"],
        "{:?}",
        code_names(&on)
    );
    assert!(report(src).diagnostics.is_empty());
}

/// `strict-inference` reports a top-level function that declares no
/// return type (`_checkStrictInferenceReturnType`,
/// best_practices_verifier.dart:1587-1606); typed declarations and
/// setters stay clean, and the mode is off by default.
#[test]
fn strict_inference_reports_untyped_top_level_declarations() {
    let src = "class int {} \
             f() {} \
             int t() { return 1; } \
             int get g => 1; \
             set s(int v) {}";
    let on = report_strict(
        src,
        StrictModes {
            inference: true,
            ..StrictModes::default()
        },
    );
    assert_eq!(
        code_names(&on),
        ["inference_failure_on_function_return_type"],
        "{:?}",
        code_names(&on)
    );
    assert_eq!(
        on.diagnostics[0].message,
        "The return type of 'f' can't be inferred."
    );
    assert_eq!(
        on.diagnostics[0].correction.as_deref(),
        Some("Declare the return type of 'f'.")
    );
    assert_eq!(on.diagnostics[0].severity, Severity::Warning);
    assert!(report(src).diagnostics.is_empty());
}

/// The exemption: a local function never reports (the oracle's own
/// visit skips the declaration-statement form — an upstream TODO there).
#[test]
fn strict_inference_keeps_local_functions_silent() {
    let on = report_strict(
        "void main() { void local() {} local(); }",
        StrictModes {
            inference: true,
            ..StrictModes::default()
        },
    );
    assert!(on.diagnostics.is_empty(), "{:?}", code_names(&on));
}

/// The untyped pair: both the function and the getter report, each at
/// its own name.
#[test]
fn strict_inference_reports_both_untyped_declaration_kinds() {
    let on = report_strict(
        "f() {} get g => 1;",
        StrictModes {
            inference: true,
            ..StrictModes::default()
        },
    );
    assert_eq!(
        code_names(&on),
        [
            "inference_failure_on_function_return_type",
            "inference_failure_on_function_return_type"
        ],
        "{:?}",
        code_names(&on)
    );
    assert_eq!(
        on.diagnostics[0].message,
        "The return type of 'f' can't be inferred."
    );
    assert_eq!(
        on.diagnostics[1].message,
        "The return type of 'g' can't be inferred."
    );
}

/// The unbounded-window latch is scoped to its owner by token order: a
/// generic non-container declaration (`typedef F<T> = int;` — likewise
/// a bodyless generic function) closes a window no container head owns,
/// so the following non-generic `C` neither adopts its count nor
/// reports on a raw use; only the genuinely generic `Box` does.
#[test]
fn strict_raw_types_latch_never_reaches_a_later_non_generic_container() {
    let src = "class int {} \
             typedef F<T> = int; \
             class C {} \
             class Box<T> {} \
             void g(C c) {} \
             void f(Box a) {}";
    let on = report_strict(
        src,
        StrictModes {
            raw_types: true,
            ..StrictModes::default()
        },
    );
    assert_eq!(
        code_names(&on),
        ["strict_raw_type"],
        "{:?}",
        code_names(&on)
    );
    assert_eq!(
        on.diagnostics[0].message,
        "The generic type 'Box<dynamic>' should have explicit type arguments but doesn't."
    );
    // The default posture: nothing reports.
    assert!(report(src).diagnostics.is_empty());
}

/// A generic-function-type bound declares a bound (`class W<T extends
/// Function()>`): the GFT's absent return type emits `handle_no_type`
/// inside the type-variables window anchored away from the variable's
/// name token, so it must not count as an unbounded variable — raw
/// uses of `W` stay silent while the generic `Box` still reports.
#[test]
fn strict_raw_types_keeps_a_generic_function_type_bound_bounded() {
    let src = "class Box<T> {} \
             class W<T extends Function()> {} \
             void f(W w) {} \
             void g(Box b) {}";
    let on = report_strict(
        src,
        StrictModes {
            raw_types: true,
            ..StrictModes::default()
        },
    );
    assert_eq!(
        code_names(&on),
        ["strict_raw_type"],
        "{:?}",
        code_names(&on)
    );
    assert_eq!(
        on.diagnostics[0].message,
        "The generic type 'Box<dynamic>' should have explicit type arguments but doesn't."
    );
    assert!(report(src).diagnostics.is_empty());
}

// —— #612: the variance family ——————————————————————————————————————————
// Oracle anchors: error_verifier.dart:7999-8088 (the superinterface
// walk and `_checkForWrongVariancePosition`), :7910-7996 (the member
// faces); texts from diagnostic.g.dart:18498-18519/:18771-18783/
// :18809-18825. Gate posture (#742): the legacy superinterface face
// answers in the default posture (official default-corpus expectations:
// wrong_type_parameter_variance_in_superinterface_test.dart); the
// explicit-keyword faces keep the experiment gate.

/// The legacy superinterface face fires in the default posture (#742):
/// the oracle's walk is not experiment-gated and its default-corpus
/// expectation anchors the row —
/// `wrong_type_parameter_variance_in_superinterface_test.dart`
/// `test_class_extends_function_parameterType` (`typedef F<X> = void
/// Function(X); class B<X> extends A<F<X>> {}` reports at the
/// parameter's name with the verbatim text). The alias-expansion edge
/// (generic typedefs answer by declared variance, a recorded
/// divergence) leaves the modeled shape writing the function type
/// inline — same code, same message template, span on `X`.
#[test]
fn variance_superinterface_legacy_face_fires_by_default() {
    let src = "class A<X> {} \
                   class B<X> extends A<void Function(X)> {}";
    let report = report(src);
    assert_eq!(
        code_names(&report),
        ["wrong_type_parameter_variance_in_superinterface"]
    );
    let row = &report.diagnostics[0];
    assert_eq!(
        row.message,
        "'X' can't be used contravariantly or invariantly in 'A<void Function(X)>'."
    );
    assert_eq!(
        &src[row.range.offset as usize..][..row.range.length as usize],
        "X"
    );
}

/// The explicit-keyword faces keep the experiment gate (#742): without
/// the grant the capture still sees the keyword (the header always
/// allows variance — the shared parser's `parseVariables` accepts it
/// structurally), but every semantic variance face stays silent. The
/// one face the fixture answers is the parse verdict itself: the
/// analyzer's `endTypeVariable` reports `experimentNotEnabled` at the
/// keyword when the flag is off (upstream Dart SDK source,
/// pkg/analyzer/lib/src/fasta/ast_builder.dart —
/// `reportVarianceModifierNotEnabled`).
#[test]
fn variance_explicit_faces_stay_gated_without_the_experiment() {
    let explicit = "class B<T> {} \
                        class A<out T> implements B<void Function(T)> {}";
    assert_eq!(
        code_names(&report_ungranted(explicit)),
        ["experiment_not_enabled_off_by_default"],
        "{:?}",
        code_names(&report_ungranted(explicit))
    );
}

/// The legacy-covariant superinterface face: a class type parameter
/// used contravariantly in a direct superinterface reports at the
/// parameter's name with the oracle's verbatim text.
#[test]
fn variance_superinterface_legacy_face_fires_under_the_experiment() {
    let src = "class A<X> {} \
                   class B<X> extends A<void Function(X)> {}";
    let report = report_variance(src);
    assert_eq!(
        code_names(&report),
        ["wrong_type_parameter_variance_in_superinterface"]
    );
    let row = &report.diagnostics[0];
    assert_eq!(
        row.message,
        "'X' can't be used contravariantly or invariantly in 'A<void Function(X)>'."
    );
    assert_eq!(
        &src[row.range.offset as usize..][..row.range.length as usize],
        "X"
    );
}

/// The explicit-keyword face picks its text by the parameter's own
/// keyword and the position's computed variance
/// (`wrongExplicitTypeParameterVarianceInSuperinterface`).
#[test]
fn variance_superinterface_explicit_face_by_keyword() {
    let out_src = "class B<T> {} \
                       class A<out T> implements B<void Function(T)> {}";
    let out = report_variance(out_src);
    assert_eq!(
        code_names(&out),
        ["wrong_explicit_type_parameter_variance_in_superinterface"]
    );
    assert_eq!(
        out.diagnostics[0].message,
        "'T' is an 'out' type parameter and can't be used in an 'in' position in \
             'B<void Function(T)>'."
    );
    let row = &out.diagnostics[0];
    assert_eq!(
        &out_src[row.range.offset as usize..][..row.range.length as usize],
        "T"
    );

    let in_src = "class B<T> {} \
                      class A<in T> implements B<T Function()> {}";
    let in_report = report_variance(in_src);
    assert_eq!(
        code_names(&in_report),
        ["wrong_explicit_type_parameter_variance_in_superinterface"]
    );
    assert_eq!(
        in_report.diagnostics[0].message,
        "'T' is an 'in' type parameter and can't be used in an 'out' position in \
             'B<T Function()>'."
    );
}

/// Passing positions per keyword: covariant under `out`, contravariant
/// under `in`, the invariant meet under `inout` — no rows.
#[test]
fn variance_superinterface_passing_positions_stay_silent() {
    let src = "class B<T> {} \
                   class A1<out T> implements B<T> {} \
                   class A2<in T> implements B<void Function(T)> {} \
                   class A3<inout T> implements B<T Function(T)> {}";
    assert!(report_variance(src).diagnostics.is_empty());
}

/// The named mixin application has no body: the declaration-end event
/// finalizes the header, so the head type checks.
#[test]
fn variance_named_mixin_application_checks_its_head() {
    let src = "class A<X> {} \
                   mixin M {} \
                   class B<X> = A<void Function(X)> with M;";
    let report = report_variance(src);
    assert_eq!(
        code_names(&report),
        ["wrong_type_parameter_variance_in_superinterface"]
    );
}

/// The enum face: a `with` mixin instantiation in a contravariant
/// position checks through the enum header.
#[test]
fn variance_enum_with_clause_checks() {
    let src = "class A<X> {} \
                   enum E<X> with A<void Function(X)> { v }";
    let report = report_variance(src);
    assert_eq!(
        code_names(&report),
        ["wrong_type_parameter_variance_in_superinterface"]
    );
}

/// The mixin face: an `on` constraint checks like the official walk's
/// superclass constraints (:8050-8052).
#[test]
fn variance_mixin_on_constraint_checks() {
    let src = "class A<X> {} \
                   mixin B<X> on A<void Function(X)> {}";
    let report = report_variance(src);
    assert_eq!(
        code_names(&report),
        ["wrong_type_parameter_variance_in_superinterface"]
    );
}

/// The extension-type face: the representation formal's type does not
/// pollute the implements capture (the rep-formals latch), and the
/// official test's shape reports (:1194 call site).
#[test]
fn variance_extension_type_checks_implements() {
    let src = "class A<T> {} \
                   extension type B<T>(A<void Function(T)> it) \
                   implements A<void Function(T)> {}";
    let report = report_variance(src);
    assert_eq!(
        code_names(&report),
        ["wrong_type_parameter_variance_in_superinterface"]
    );
}

/// The member faces (`_checkForWrongTypeParameterVarianceInField`
/// :7910-7933 and `...InMethod` :7936-7996): a field's declared type
/// and its non-final implicit setter, a method's return type and
/// formals, each with the verbatim `_position` text.
#[test]
fn variance_member_faces_report_forbidden_positions() {
    // `out T` field: the declared type passes, the implicit setter
    // fails.
    let field_src = "class A<out T> { T x; }";
    let field = report_variance(field_src);
    assert_eq!(
        code_names(&field),
        ["wrong_type_parameter_variance_position"]
    );
    assert_eq!(
        field.diagnostics[0].message,
        "The 'out' type parameter 'T' can't be used in an 'in' position."
    );
    assert_eq!(
        &field_src[field.diagnostics[0].range.offset as usize..]
            [..field.diagnostics[0].range.length as usize],
        "x"
    );

    // `in T` final field: the declared type fails, the (absent)
    // setter face spares. `late` keeps the fixture at this one face —
    // an uninitialized non-late final would add the declaration walk's
    // `final_not_initialized` (error_verifier.dart's `variableList
    // .isLate` arm returns before the final check).
    let final_src = "class A<in T> { late final T x; }";
    let final_report = report_variance(final_src);
    assert_eq!(
        code_names(&final_report),
        ["wrong_type_parameter_variance_position"]
    );
    assert_eq!(
        final_report.diagnostics[0].message,
        "The 'in' type parameter 'T' can't be used in an 'out' position."
    );

    // `in T` return type fails; `out T` formal fails.
    let method_src = "class A<in T> { T m() => throw ''; } \
                          class B<out T> { void m(T x) {} }";
    let method = report_variance(method_src);
    assert_eq!(
        code_names(&method),
        [
            "wrong_type_parameter_variance_position",
            "wrong_type_parameter_variance_position"
        ]
    );
    assert_eq!(
        method.diagnostics[0].message,
        "The 'in' type parameter 'T' can't be used in an 'out' position."
    );
    assert_eq!(
        method.diagnostics[1].message,
        "The 'out' type parameter 'T' can't be used in an 'in' position."
    );
}

/// Legacy parameters never fire the member faces (the official
/// `isLegacyCovariant` skips, :7918/:7945) — and the superinterface
/// face only through the legacy row.
#[test]
fn variance_legacy_parameters_stay_silent_on_member_faces() {
    let src = "class A<T> { T x; void m(T y) {} T n() => throw ''; }";
    assert!(report_variance(src).diagnostics.is_empty());
}

/// The walk composes: a contravariant occurrence through a covariant
/// argument stays contravariant, and an occurrence in a function
/// type's own parameter bound forces `invariant`
/// (variance_builder.dart:131-140; the official `test_typeParameter_
/// bound` shape — the display renders the written shape without the
/// signature's `<...>` section, a documented divergence).
#[test]
fn variance_walk_composes_nesting_and_signature_bounds() {
    let nested = report_variance(
        "class A<X> {} \
             class B<Y> extends A<A<void Function(Y)>> {}",
    );
    assert_eq!(
        code_names(&nested),
        ["wrong_type_parameter_variance_in_superinterface"]
    );

    let bound = report_variance(
        "class B<X> {} \
             class C<out X> extends B<void Function<Y extends X>()> {}",
    );
    assert_eq!(
        code_names(&bound),
        ["wrong_explicit_type_parameter_variance_in_superinterface"]
    );
    assert_eq!(
        bound.diagnostics[0].message,
        "'X' is an 'out' type parameter and can't be used in an 'inout' position in \
             'B<void Function()>'."
    );
}

// —— #769: the wildcard gate —————————————————————————————————————————

/// At the modern posture the exact `_` is exempt while `__` still
/// reports (`_isNamedWildcard`).
#[test]
fn wildcard_underscore_exempt_double_underscore_reports() {
    let report = report("void main() {\n  var _ = 1;\n  var __ = 2;\n}");
    assert_eq!(code_names(&report), ["unused_local_variable"]);
}

/// The catch clause follows the same gate on the clause the official
/// walk reports: an `on`-typed catch's unread exception variable
/// reports `unused_catch_clause` with the exact `_` exempt and `__`
/// still reporting (`_isNamedWildcard` — the exemption lives in the
/// catch arm of `_visitLocalVariableElement`). A plain `catch (e)`
/// never reaches the gate: its auto-use (visitCatchClause's
/// `onKeyword == null` arm marks the element used) fires first.
#[test]
fn wildcard_catch_clause_follows_the_gate() {
    let exempt = report("class int {} void main() { try {} on int catch (_) {} }");
    assert!(exempt.diagnostics.is_empty(), "{:?}", code_names(&exempt));
    let flagged = report("class int {} void main() { try {} on int catch (__) {} }");
    assert_eq!(code_names(&flagged), ["unused_catch_clause"]);
}
// —— #890: the W3 semantic dead-code queue ———————————————————————————

/// A total-match member (`case _`) drives the official flow `unmatched`
/// state unreachable, so the following member is entered dead: the
/// keyword row anchors at the bare `case` token and the body row spans
/// the member's statements (`analyzeSwitchStatement`'s
/// `switch_beginAlternative` reads the dead `_unmatched`; the
/// `dead_code_verifier`'s `SwitchMember` faces). Exhaustiveness-only
/// coverage (the oracle's `unreachable_switch_case`) co-fires and stays
/// the sole verdict for the duplicate constant — probe.dart s2/v2,
/// oracle rows `30|5|4` + `30|5|4` + `31|7|28`.
#[test]
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn switch_wildcard_chain_dead_rows() {
    let source = "enum E { a, b, c } void f(E e) { switch (e) { case E.a: break; case _: break; case E.c: print2('dead2'); break; } } void print2(String s) {}";
    let report = report(source);
    let dead: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .map(|d| (d.range.offset, d.range.length))
        .collect();
    assert_eq!(
        dead,
        [
            (offset_of(source, "case E.c:"), "case".len() as u32),
            (
                offset_of(source, "print2"),
                ("print2('dead2'); break;".len()) as u32
            ),
        ],
        "{dead:?}",
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "unreachable_switch_case")
    );
}

/// A member whose space is disjoint from the scrutinee never matches:
/// only its body dies (the official's `_EqualityCheckHasKnownResult`
/// makes the matched state unreachable at the pattern) — enum.dart
/// `unreachableCase3`, oracle `322|7|27`, keyword row absent. The
/// gate is constant roots only: a disjoint TYPE TEST (`case A _` over
/// an enum) takes the restriction path, which joins — its body stays
/// live and `pattern_never_matches_value_type` reports alone (oracle
/// probes: the `case null` shape reports the body row, the `case A _`
/// shape reports no `dead_code` at all).
#[test]
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn switch_never_match_member_body_row() {
    let source = "enum E { a, b, c } void f(E e) { switch (e) { case E.a: break; case null: print2('n'); break; } } void print2(String s) {}";
    let null_case = report(source);
    let dead: Vec<_> = null_case
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .map(|d| (d.range.offset, d.range.length))
        .collect();
    assert_eq!(
        dead,
        [(
            offset_of(source, "print2"),
            "print2('n'); break;".len() as u32
        )],
        "{dead:?}",
    );
    let type_test = report(
        "enum E { a, b } class A {} void f(E e) { switch (e) { case A _: 1; 2; case E.a: break; case E.b: break; } }",
    );
    assert!(
        dead_code_offsets(&type_test).is_empty(),
        "{:?}",
        code_names(&type_test)
    );
}

/// A duplicate constant over a fully covered scrutinee is NOT
/// flow-dead: the flow `unmatched` state stays reachable (constant
/// patterns join without exhausting it), so exhaustiveness coverage
/// reports `unreachable_switch_case` alone — probe.dart s1/v1, oracle
/// `15|5|4` USC with no `dead_code` row.
#[test]
fn switch_covered_constant_not_flow_dead() {
    let source = "enum E { a, b, c } void f(E e) { switch (e) { case E.a: break; case E.b: break; case E.c: break; case E.a: break; } }";
    let report = report(source);
    assert!(
        dead_code_offsets(&report).is_empty(),
        "{:?}",
        code_names(&report)
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "unreachable_switch_case")
    );
}

/// A variable pattern over the scrutinee's own type is a total match
/// (`case A a:` over sealed `A`): the following member is chain-dead —
/// `sealed_class.dart` `unreachableCase2`, oracle `238|5|4` + `239|7|27`.
#[test]
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn switch_same_type_variable_pattern_chain_dead() {
    let source = "sealed class A {} class B extends A {} void f(A a) { switch (a) { case A a2: break; case null: print2('n'); break; } } void print2(String s) {}";
    let report = report(source);
    let dead: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .map(|d| (d.range.offset, d.range.length))
        .collect();
    assert_eq!(
        dead,
        [
            (offset_of(source, "case null:"), "case".len() as u32),
            (
                offset_of(source, "print2"),
                "print2('n'); break;".len() as u32
            ),
        ],
        "{dead:?}",
    );
}

/// A guarded total pattern never starts the chain (the guard may
/// fail) — probe.dart x3: no dead rows, the non-exhaustive covering
/// error aside.
#[test]
fn switch_guarded_wildcard_keeps_members_live() {
    let source = "enum E { a, b, c } void f(E e, bool c) { switch (e) { case E.a: break; case _ when c: break; case E.c: break; } }";
    let report = report(source);
    assert!(
        dead_code_offsets(&report).is_empty(),
        "{:?}",
        code_names(&report)
    );
}

/// `x != null` as a whole `??` left operand is `bool` — strictly
/// non-nullable — so the right operand reports
/// `dead_null_aware_expression` at its own range (oracle q1:
/// `DEAD_NULL_AWARE_EXPRESSION 4|20|5` over `false`).
#[test]
fn ifnull_equality_lhs_reports_dead_rhs() {
    let source = "class int {} void g() {} void f(int x) { if (x != null ?? false) { g(); } }";
    let report = report(source);
    let dead = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "dead_null_aware_expression")
        .expect("dead_null_aware_expression present");
    assert_eq!(dead.range.offset, offset_of(source, "false"));
    assert_eq!(
        dead.message,
        "The left operand can't be null, so the right operand is never executed."
    );
}

/// A nullable left operand stays silent (`x ?? y` over `int? x`) — the
/// existing `dead_null_aware_right_operands_report` covers the
/// non-nullable read face; this pins the nullable control.
#[test]
fn ifnull_nullable_lhs_stays_silent() {
    let source = "class int {} void f(int? x, int y) { x ?? y; }";
    let report = report(source);
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "dead_null_aware_expression"),
        "{:?}",
        code_names(&report)
    );
}

/// `c1?[0]` over a definitely-unassigned nullable local reports
/// `dead_code` spanning the operator through the index close
/// (`_verifyUnassignedVariable`'s index face, oracle p5 z1: `5|5|4`
/// over `?[0]`); an assigned receiver stays live (z3).
#[test]
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn null_aware_index_unassigned_receiver_reports() {
    let source = "class C1 {} void f() { C1? c1; c1?[0]; }";
    let report = report(source);
    let dead = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "dead_code" && d.range.offset == offset_of(source, "?[0]"))
        .expect("dead_code at the null-aware index");
    assert_eq!(dead.range.length, "?[0]".len() as u32);
    assert_eq!(dead.message, "Dead code.");
}

#[test]
fn null_aware_index_assigned_receiver_stays_live() {
    let source = "class C1 {} void f() { C1? c1; c1 = C1(); c1?[0]; }";
    let report = report(source);
    assert!(
        dead_code_offsets(&report).is_empty(),
        "{:?}",
        code_names(&report)
    );
}

/// The dot form reports over the same face (`c1?.g()`, oracle p5 z2:
/// `10|5|7` over `?.g()`); the row anchors at the operator. A field
/// receiver is not definitely unassigned and stays silent (z6).
#[test]
fn null_aware_dot_unassigned_receiver_reports() {
    let source = "class C1 { void g() {} } void f() { C1? c1; c1?.g(); }";
    let report = report(source);
    let dead = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "dead_code" && d.range.offset == offset_of(source, "?.g()"))
        .expect("dead_code at the null-aware access");
    assert!(dead.range.length >= 2, "{:?}", dead.range);
}

/// A field receiver is not a definitely-unassigned local: the chain
/// stays silent (oracle p5 z6).
#[test]
fn null_aware_field_receiver_stays_live() {
    let source =
        "class C1 { void g() {} }\nclass H {\n  C1? c1;\n  void f() {\n    c1?.g();\n  }\n}";
    let report = report(source);
    assert!(
        dead_code_offsets(&report).is_empty(),
        "{:?}",
        code_names(&report)
    );
}

/// A boolean literal as the whole `??` left operand is `bool` —
/// strictly non-nullable — so the right operand reports (`true ?? x`,
/// oracle q2's `DEAD_NULL_AWARE_EXPRESSION`; the row anchors at the
/// right operand).
#[test]
fn ifnull_bool_literal_lhs_reports_dead_rhs() {
    let source = "class int {} void f(int x) { true ?? x; }";
    let report = report(source);
    let dead = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "dead_null_aware_expression")
        .expect("dead_null_aware_expression present");
    assert_eq!(dead.range.offset, offset_of(source, "?? x") + 3);
}

/// A Never-typed callee kills the flow state at the send's end, so the
/// dead interval anchors at the FIRST statement after the call (the
/// official `checkUnreachableNode` at the next statement's entry —
/// section I's correction: the row starts at `print`, never at the
/// call) and folds to the covering block's end.
#[test]
fn never_call_dead_row_starts_at_the_next_statement() {
    let source = "Never boom() => throw 0; void f() { boom(); g(); } void g() {}";
    let report = report(source);
    assert_eq!(dead_code_offsets(&report), [offset_of(source, "g();")]);
}

/// An `_`-named local function is dead over the whole declaration,
/// call or not (`DeadCodeVerifier`'s visitFunctionDeclaration,
/// dead_code_verifier.dart:72-82; oracle p4 y1: `50|3|31`).
#[test]
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn local_wildcard_function_reports_dead() {
    let source = "void g() {} void f() { void _() { g(); } _(); }";
    let report = report(source);
    let dead = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "dead_code")
        .expect("dead_code present");
    assert_eq!(dead.range.offset, offset_of(source, "void _()"));
    assert_eq!(dead.range.length, "void _() { g(); }".len() as u32);
    assert_eq!(dead.message, "Dead code.");
}

/// A named local function stays live; named function expressions share
/// the identifier context but never report (the official face is
/// `visitFunctionDeclaration`).
#[test]
fn local_named_function_and_expression_stay_live() {
    let source = "void g() {} void f() { void h() { g(); } h(); }";
    let report = report(source);
    assert!(
        dead_code_offsets(&report).is_empty(),
        "{:?}",
        code_names(&report)
    );
}
// —— #748: non_nullable_equals_parameter / unnecessary_no_such_method ——
//
// Behavioral oracle: dart 3.13.1 `dart analyze --format=machine` probes
// (see the fixture headers under tests/corpus/m2-warning for the live
// rows). Official sources:
// best_practices_verifier.dart:1345-1374 (`_checkForNullableEqualsPara-
// meterType`) and :1433-1478 (`_checkForUnnecessaryNoSuchMethod`).

/// An `==` operator with a single `Object?` parameter reports at the
/// `==` name token with the verbatim oracle message and correction
/// (probed: `WARNING|STATIC_WARNING|NON_NULLABLE_EQUALS_PARAMETER` at
/// the `==` token, length 2).
#[test]
fn nullable_equals_parameter_reports_at_the_operator_token() {
    let src = "class bool {}\nclass Object {}\n\
                   class P {\n  @override\n  bool operator ==(Object? other) => true;\n}\n";
    let rep = report(src);
    assert_eq!(
        code_names(&rep),
        ["non_nullable_equals_parameter"],
        "{:?}",
        code_names(&rep)
    );
    let d = &rep.diagnostics[0];
    assert_eq!(
        d.message,
        "The parameter type of '==' operators should be non-nullable."
    );
    assert_eq!(
        d.correction.as_deref(),
        Some("Try using a non-nullable type.")
    );
    let at = src.find("==").expect("operator token");
    assert_eq!(d.range.offset, u32::try_from(at).unwrap());
    assert_eq!(d.range.length, 2);
}

/// The silent `==` faces (each probed silent on 3.13.1): bare `Object`,
/// `dynamic`, an unwritten annotation (resolved `dynamic`), a
/// non-participant type (`int?` — the official's `dartCoreObject`/
/// `DynamicType` guard returns before the nullability question), two
/// parameters, a plain method named `equals`, and a function-typed
/// formal (the resolved type is a function type, never `Object`).
#[test]
fn non_nullable_equals_parameter_silent_faces() {
    let src = "class bool {}\nclass Object {}\nclass int {}\n\
                   class A { @override bool operator ==(Object other) => true; }\n\
                   class B { @override bool operator ==(dynamic other) => true; }\n\
                   class C { @override bool operator ==(other) => true; }\n\
                   class D { @override bool operator ==(int? other) => true; }\n\
                   class F { bool equals(Object? other) => true; }\n\
                   class G { @override bool operator ==(void Function() other) => true; }\n";
    let rep = report(src);
    assert!(
        !code_names(&rep).contains(&"non_nullable_equals_parameter"),
        "{:?}",
        code_names(&rep)
    );
    // Two parameters: the official arity gate returns first (the
    // wrong-number-of-parameters ERROR is the collector's own face).
    let rep = report(
        "class bool {}\nclass Object {}\n\
             class U { @override bool operator ==(Object? a, Object? b) => true; }\n",
    );
    assert!(
        !code_names(&rep).contains(&"non_nullable_equals_parameter"),
        "{:?}",
        code_names(&rep)
    );
}

/// An abstract `==` declaration still reports — the official check
/// never reads the body (probed: `abstract class V { bool operator
/// ==(Object? other); }` emits the row).
#[test]
fn abstract_equals_parameter_still_reports() {
    let src = "class bool {}\nclass Object {}\n\
                   abstract class V {\n  @override\n  bool operator ==(Object? other);\n}\n";
    let rep = report(src);
    assert_eq!(
        code_names(&rep),
        ["non_nullable_equals_parameter"],
        "{:?}",
        code_names(&rep)
    );
}

/// A `noSuchMethod` declaration whose body is exactly
/// `super.noSuchMethod(<one argument>)` reports at the declaration's
/// name token with the verbatim oracle message and correction, when
/// the superchain carries a concrete declaration (probed: length 12
/// at the name). Both allowed body shapes fire — the arrow form and a
/// block whose single statement is that return.
#[test]
fn no_such_method_delegation_reports_at_the_name_token() {
    let src = "class Invocation {}\n\
                   class A {\n  @override\n  dynamic noSuchMethod(Invocation invocation) => 42;\n}\n\
                   class C extends A {\n  @override\n  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);\n}\n";
    let rep = report(src);
    assert_eq!(
        code_names(&rep),
        ["unnecessary_no_such_method"],
        "{:?}",
        code_names(&rep)
    );
    let d = &rep.diagnostics[0];
    assert_eq!(d.message, "Unnecessary 'noSuchMethod' declaration.");
    assert_eq!(
        d.correction.as_deref(),
        Some("Try removing the declaration of 'noSuchMethod'.")
    );
    let at = src[src.find("class C").unwrap()..]
        .find("noSuchMethod")
        .expect("delegate name");
    let at = src.find("class C").unwrap() + at;
    assert_eq!(d.range.offset, u32::try_from(at).unwrap());
    assert_eq!(d.range.length, 12);

    // The block form: one statement, that return.
    let src = "class Invocation {}\n\
                   class A {\n  @override\n  dynamic noSuchMethod(Invocation invocation) => 42;\n}\n\
                   class B extends A {\n  @override\n  dynamic noSuchMethod(Invocation invocation) {\n    return super.noSuchMethod(invocation);\n  }\n}\n";
    let rep = report(src);
    assert_eq!(
        code_names(&rep),
        ["unnecessary_no_such_method"],
        "{:?}",
        code_names(&rep)
    );
}

/// The resolution climbs THROUGH clean intermediates: each delegating
/// class in `A <- C <- L2` reports (the concrete declaration one hop
/// up decides, probed through two levels). Argument content is
/// irrelevant — exactly one argument of any shape matches (probed
/// with a dotted argument).
#[test]
fn no_such_method_resolution_climbs_clean_intermediates() {
    let src = "class Invocation {}\n\
                   class A {\n  dynamic noSuchMethod(Invocation invocation) => 42;\n}\n\
                   class C extends A {\n  @override\n  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);\n}\n\
                   class L2 extends C {\n  @override\n  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);\n}\n";
    let rep = report(src);
    assert_eq!(
        code_names(&rep),
        ["unnecessary_no_such_method", "unnecessary_no_such_method"],
        "{:?}",
        code_names(&rep)
    );

    // A dotted argument stays one argument.
    let src = "class Invocation {}\n\
                   class A {\n  dynamic noSuchMethod(Invocation invocation) => 42;\n}\n\
                   class D extends A {\n  Invocation other = Invocation();\n  @override\n  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(other);\n}\n";
    let rep = report(src);
    assert_eq!(
        code_names(&rep),
        ["unnecessary_no_such_method"],
        "{:?}",
        code_names(&rep)
    );
}

/// The silent faces (each probed silent on 3.13.1): the direct
/// `Object` resolution (no `extends`, or `extends` a clean chain to
/// `Object`), a coalesced double delegation, a block with more than
/// one statement, a delegation to a different member, a bare self
/// call, and one/zero arguments.
#[test]
fn no_such_method_silent_faces() {
    // The Object default: silent with and without `extends Object`.
    let src = "class Invocation {}\nclass Object {}\n\
                   class E {\n  @override\n  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);\n}\n";
    let rep = report(src);
    assert!(
        !code_names(&rep).contains(&"unnecessary_no_such_method"),
        "{:?}",
        code_names(&rep)
    );
    let src = "class Invocation {}\nclass Object {}\n\
                   class E2 extends Object {\n  @override\n  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);\n}\n";
    let rep = report(src);
    assert!(
        !code_names(&rep).contains(&"unnecessary_no_such_method"),
        "{:?}",
        code_names(&rep)
    );
    // Compound expressions never match the one-invocation shape.
    let src = "class Invocation {}\n\
                   class A {\n  dynamic noSuchMethod(Invocation invocation) => 42;\n}\n\
                   class K extends A {\n  @override\n  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation) ?? super.noSuchMethod(invocation);\n}\n";
    let rep = report(src);
    assert!(
        !code_names(&rep).contains(&"unnecessary_no_such_method"),
        "{:?}",
        code_names(&rep)
    );
    // A block with more than one statement never matches.
    let src = "class Invocation {}\n\
                   class A {\n  dynamic noSuchMethod(Invocation invocation) => 42;\n}\n\
                   class T2 extends A {\n  @override\n  dynamic noSuchMethod(Invocation invocation) {\n    noSuchMethod(invocation);\n    return super.noSuchMethod(invocation);\n  }\n}\n";
    let rep = report(src);
    assert!(
        !code_names(&rep).contains(&"unnecessary_no_such_method"),
        "{:?}",
        code_names(&rep)
    );
    // A delegation to a different member, a bare self call, and the
    // argument-count gates.
    let src = "class Invocation {}\n\
                   class A {\n  dynamic noSuchMethod(Invocation invocation) => 42;\n}\n\
                   class F2 extends A {\n  @override\n  dynamic noSuchMethod(Invocation invocation) => super.foo(invocation);\n}\n";
    let rep = report(src);
    assert!(
        !code_names(&rep).contains(&"unnecessary_no_such_method"),
        "{:?}",
        code_names(&rep)
    );
    let src = "class Invocation {}\n\
                   class A {\n  dynamic noSuchMethod(Invocation invocation) => 42;\n}\n\
                   class N2 extends A {\n  @override\n  dynamic noSuchMethod(Invocation invocation) => noSuchMethod(invocation);\n}\n";
    let rep = report(src);
    assert!(
        !code_names(&rep).contains(&"unnecessary_no_such_method"),
        "{:?}",
        code_names(&rep)
    );
    let src = "class Invocation {}\n\
                   class A {\n  dynamic noSuchMethod(Invocation invocation) => 42;\n}\n\
                   class W2 extends A {\n  @override\n  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation, invocation);\n}\n";
    let rep = report(src);
    assert!(
        !code_names(&rep).contains(&"unnecessary_no_such_method"),
        "{:?}",
        code_names(&rep)
    );
    let src = "class Invocation {}\n\
                   class A {\n  dynamic noSuchMethod(Invocation invocation) => 42;\n}\n\
                   class Z2 extends A {\n  @override\n  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod();\n}\n";
    let rep = report(src);
    assert!(
        !code_names(&rep).contains(&"unnecessary_no_such_method"),
        "{:?}",
        code_names(&rep)
    );
    // A trailing selector on the delegation breaks the node shape.
    let src = "class Invocation {}\n\
                   class A {\n  dynamic noSuchMethod(Invocation invocation) => 42;\n}\n\
                   class S2 extends A {\n  @override\n  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation).x;\n}\n";
    let rep = report(src);
    assert!(
        !code_names(&rep).contains(&"unnecessary_no_such_method"),
        "{:?}",
        code_names(&rep)
    );
}

/// Abstract declarations are TRANSPARENT to the concrete dispatch
/// (probed on 3.13.1): an abstract declaration one hop closer never
/// stops the walk (the concrete declaration behind it still reports),
/// while an abstract-only chain resolves to `Object` and stays silent.
#[test]
fn no_such_method_abstract_declarations_are_transparent() {
    let src = "class Invocation {}\n\
                   class A2 {\n  dynamic noSuchMethod(Invocation invocation) => 3;\n}\n\
                   abstract class Mid extends A2 {\n  @override\n  dynamic noSuchMethod(Invocation invocation);\n}\n\
                   class W extends Mid {\n  @override\n  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);\n}\n";
    let rep = report(src);
    assert_eq!(
        code_names(&rep),
        ["unnecessary_no_such_method"],
        "{:?}",
        code_names(&rep)
    );
    let src = "class Invocation {}\n\
                   abstract class Abs {\n  dynamic noSuchMethod(Invocation invocation);\n}\n\
                   class V2 extends Abs {\n  @override\n  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);\n}\n";
    let rep = report(src);
    assert!(
        !code_names(&rep).contains(&"unnecessary_no_such_method"),
        "{:?}",
        code_names(&rep)
    );
}

/// A CONCRETE mixin declaration of `noSuchMethod` poisons the
/// resolution (probed silent on 3.13.1: the delegating class's own
/// `with M`, an intermediate application's `with M`, and the
/// direct `extends B with M` all stay silent — even when the nominal
/// superclass ALSO declares one), while an ABSTRACT mixin declaration
/// is transparent and the chain behind it still reports.
#[test]
fn no_such_method_concrete_mixin_declarations_poison() {
    // The delegating class's own mixin declares.
    let src = "class Invocation {}\n\
                   mixin M {\n  dynamic noSuchMethod(Invocation invocation) => 7;\n}\n\
                   class Base {}\n\
                   class W extends Base with M {\n  @override\n  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);\n}\n";
    let rep = report(src);
    assert!(
        !code_names(&rep).contains(&"unnecessary_no_such_method"),
        "{:?}",
        code_names(&rep)
    );
    // A mixin declares on an intermediate application, shadowing the
    // concrete nominal declaration behind it.
    let src = "class Invocation {}\n\
                   mixin Mx {\n  dynamic noSuchMethod(Invocation invocation) => 13;\n}\n\
                   class Base3 {\n  dynamic noSuchMethod(Invocation invocation) => 15;\n}\n\
                   class Mid extends Base3 with Mx {}\n\
                   class W3 extends Mid {\n  @override\n  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);\n}\n";
    let rep = report(src);
    assert!(
        !code_names(&rep).contains(&"unnecessary_no_such_method"),
        "{:?}",
        code_names(&rep)
    );
    // A mixin without the name leaves the nominal chain decisive.
    let src = "class Invocation {}\n\
                   mixin Mh {\n  void helper() {}\n}\n\
                   class Base4 {\n  dynamic noSuchMethod(Invocation invocation) => 15;\n}\n\
                   class Mid4 extends Base4 with Mh {}\n\
                   class W4 extends Mid4 {\n  @override\n  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);\n}\n";
    let rep = report(src);
    assert_eq!(
        code_names(&rep),
        ["unnecessary_no_such_method"],
        "{:?}",
        code_names(&rep)
    );
    // An abstract mixin declaration never poisons.
    let src = "class Invocation {}\n\
                   mixin Ma {\n  dynamic noSuchMethod(Invocation invocation);\n}\n\
                   class Base7 {\n  dynamic noSuchMethod(Invocation invocation) => 5;\n}\n\
                   class W7 extends Base7 with Ma {\n  @override\n  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);\n}\n";
    let rep = report(src);
    assert_eq!(
        code_names(&rep),
        ["unnecessary_no_such_method"],
        "{:?}",
        code_names(&rep)
    );
}

// —— #611 consumption: written type arguments join the verdicts ————
// The harness has no dart:core, so every source declares the core
// stubs its shapes use — otherwise the undefined-class noise drowns
// the face under test.

/// The assignment face consults written arguments: `List<num>` does not
/// fit `List<int>` (the `num`→`int` element is a decidable
/// final-primitive mismatch), so the member-read source reports against
/// the declared target.
#[test]
fn argument_mismatch_reports_invalid_assignment() {
    let report = report(
        "class int {} class num {} class List<T> {} \
             class A { List<num> a = []; } \
             class B { List<int> b = []; } \
             void f(A x, B y) { y.b = x.a; }",
    );
    assert_eq!(
        code_names(&report),
        ["invalid_assignment"],
        "{:?}",
        code_names(&report)
    );
}

/// The covariant direction stays clean: `List<int>` fits `List<num>`.
#[test]
fn covariant_arguments_stay_clean() {
    let report = report(
        "class int {} class num {} class List<T> {} \
             class A { List<int> a = []; } \
             class B { List<num> b = []; } \
             void f(A x, B y) { y.b = x.a; }",
    );
    assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
}

/// Distinct unrelated arguments decide the miss: with both element
/// graphs closed and indexed, no edge can relate `T1` and `T2`, so the
/// covariant element check answers a definite false and the assignment
/// face reports (live-oracle-probed on this exact source:
/// `INVALID_ASSIGNMENT` on `y.b = x.a`). The under-report `None`
/// survives only for elements the index cannot close (dead or foreign
/// edges).
#[test]
fn unseen_hierarchy_arguments_decide_the_miss() {
    let report = report(
        "class List<T> {} class T1 {} class T2 {} \
             class A { List<T1> a = []; } \
             class B { List<T2> b = []; } \
             void f(A x, B y) { y.b = x.a; }",
    );
    assert_eq!(
        code_names(&report),
        ["invalid_assignment"],
        "{:?}",
        code_names(&report)
    );
}

/// The return face runs the same rule.
#[test]
fn argument_mismatch_reports_return_of_invalid_type() {
    let report = report(
        "class int {} class num {} class List<T> {} \
             class A { List<num> a = []; } \
             List<int> f(A x) { return x.a; }",
    );
    assert_eq!(
        code_names(&report),
        ["return_of_invalid_type"],
        "{:?}",
        code_names(&report)
    );
}

/// The argument face runs the same rule.
#[test]
fn argument_mismatch_reports_argument_type_not_assignable() {
    let report = report(
        "class int {} class num {} class List<T> {} \
             class A { List<num> a = []; } \
             void g(List<int> x) {} \
             void f(A y) { g(y.a); }",
    );
    assert_eq!(
        code_names(&report),
        ["argument_type_not_assignable"],
        "{:?}",
        code_names(&report)
    );
}

/// The function-subtype face (m1 pin `c3ee9065`): a formal's written
/// generic-function-type signature decides a tear-off argument
/// structurally — formal contravariance, return covariance, the
/// required-positional counts, and the named required-presence rule
/// (subtype.dart:143) each report their violation, the sound
/// directions stay silent, and the messages render the written
/// shapes (`void Function({required int x})`).
///
/// Hand-trace: `useNum(void Function(num) f)` — the annotation's
/// capture adopts the armed `void` return at the `Function` head, the
/// nested `num` joins the positional section, and `f`'s name claims
/// the signature. `useNum(onlyInt)` reads `onlyInt`'s own signature
/// off its declaration and decides `num <: int` false.
#[test]
fn function_type_tear_off_assignability_decides_structurally() {
    let src = "class int {} class num {} \
             void useNum(void Function(num) f) => f(1.5); \
             void useNullable(void Function(int?) f) => f(null); \
             void useIntReturn(int Function() f) => f(); \
             void useNamed(void Function({int x}) f) => f(); \
             void useRequiredNamed(void Function({required int x}) f) => f(x: 1); \
             void onlyInt(int x) {} \
             void fromNum(num n) {} \
             int justInt() => 1; \
             int? maybeInt() => null; \
             void requiredNamedX({required int x}) {} \
             void optionalNamedX({int x}) {} \
             void main() { \
               useNum(onlyInt); \
               useNum(fromNum); \
               useNullable(onlyInt); \
               useIntReturn(maybeInt); \
               useIntReturn(justInt); \
               useNamed(onlyInt); \
               useNamed(requiredNamedX); \
               useRequiredNamed(optionalNamedX); \
             }";
    let report = report(src);
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "argument_type_not_assignable")
        .collect();
    assert_eq!(rows.len(), 5, "{:?}", code_names(&report));
    assert_eq!(
        rows[0].message,
        "The argument type 'void Function(int)' can't be assigned to the parameter type 'void Function(num)'. "
    );
    assert_eq!(
        rows[1].message,
        "The argument type 'void Function(int)' can't be assigned to the parameter type 'void Function(int?)'. "
    );
    assert_eq!(
        rows[2].message,
        "The argument type 'int? Function()' can't be assigned to the parameter type 'int Function()'. "
    );
    assert_eq!(
        rows[3].message,
        "The argument type 'void Function(int)' can't be assigned to the parameter type 'void Function({int x})'. "
    );
    assert_eq!(
        rows[4].message,
        "The argument type 'void Function({required int x})' can't be assigned to the parameter type 'void Function({int x})'. "
    );
    assert_eq!(
        rows[0].range.offset,
        u32::try_from(src.find("useNum(onlyInt)").unwrap() + "useNum(".len()).unwrap()
    );
    assert_eq!(rows[0].range.length, 7);
}

/// The arity and required-count faces of the function-subtype check:
/// a one-formal tear-off into a two-required target and the reverse
/// both report; the wide-but-compatible directions stay silent.
#[test]
fn function_type_arity_and_required_counts_decide() {
    let src = "class int {} \
             void useTwo(void Function(int, int) f) => f(1, 2); \
             void useOne(void Function(int) f) => f(1); \
             void one(int a) {} \
             void two(int a, int b) {} \
             void main() { useTwo(one); useOne(two); }";
    let report = report(src);
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "argument_type_not_assignable")
        .collect();
    assert_eq!(rows.len(), 2, "{:?}", code_names(&report));
    assert_eq!(
        rows[0].message,
        "The argument type 'void Function(int)' can't be assigned to the parameter type 'void Function(int, int)'. "
    );
    assert_eq!(
        rows[1].message,
        "The argument type 'void Function(int, int)' can't be assigned to the parameter type 'void Function(int)'. "
    );
}

/// The override face's contravariance consults arguments: a declared
/// `List<int>` parameter does not accept the inherited `List<num>`
/// values. The covariant declaration (`List<num>` over `List<int>`)
/// stays clean.
#[test]
fn argument_mismatch_breaks_override_contravariance() {
    let broken = report(
        "class int {} class num {} class List<T> {} \
             class S { void m(List<num> x) {} } \
             class C extends S { void m(List<int> x) {} }",
    );
    assert_eq!(
        code_names(&broken),
        ["invalid_override"],
        "{:?}",
        code_names(&broken)
    );
    let clean = report(
        "class int {} class num {} class List<T> {} \
             class S { void m(List<int> x) {} } \
             class C extends S { void m(List<num> x) {} }",
    );
    assert!(code_names(&clean).is_empty(), "{:?}", code_names(&clean));
}

/// The cast faces see structural equality: the identical written type
/// is an unnecessary cast, the mismatched one is live.
#[test]
fn unnecessary_cast_stays_argument_exact() {
    let report = report(
        "class int {} class num {} class List<T> {} \
             class A { List<int> a = []; } \
             void f(A x) { x.a as List<int>; x.a as List<num>; }",
    );
    assert_eq!(
        code_names(&report),
        ["unnecessary_cast"],
        "{:?}",
        code_names(&report)
    );
}
/// The `e!` null-aware trio (#1329 FAM12): the `!` upgrades the read's
/// walk-time type, so the `?.` of the same chain reports
/// `invalid_null_aware_operator` at the operator; the second read is
/// flow-promoted, so its `?.` re-decides at teardown; the `?.[`
/// recovery shapes resolve as the member `'['` at the bracket
/// (getter on the read, setter under the assignment).
///
/// The parse tier rides the capture face (regression case 35ed6821,
/// the null_shorting_index rows): each `?.[` link takes ONE
/// `missing_identifier` anchored on its `[` — "Expected an identifier.",
/// length 1 (upstream Dart SDK source,
/// pkg/analyzer/lib/src/dart/ast/ast_builder.dart doDotExpression
/// recovery). The rows are the capture's own parse face and stay; the
/// pin holds the merged six-row set.
#[test]
fn null_assert_upgrade_drives_the_null_aware_faces() {
    let report = report(
        "class int {} class C { int operator [](int i) => i; } \
             void f() { C? c = C(); c!?.[0]; c?.[0] = 1; }",
    );
    let mut names = code_names(&report);
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "invalid_null_aware_operator",
            "invalid_null_aware_operator",
            "missing_identifier",
            "missing_identifier",
            "undefined_getter",
            "undefined_setter",
            // `c!` on the flow-promoted non-null `c` (probed: dart
            // analyze 3.13.1 — `UNNECESSARY_NON_NULL_ASSERTION` at the
            // `!`, "The '!' will have no effect because the receiver
            // can't be null.").
            "unnecessary_non_null_assertion",
        ],
        "{:?}",
        names
    );
    let missing: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "missing_identifier")
        .collect();
    assert_eq!(missing.len(), 2, "{:?}", code_names(&report));
    for row in &missing {
        assert_eq!(row.message, "Expected an identifier.");
        assert_eq!(row.range.length, 1);
    }
    let getter = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "undefined_getter")
        .expect("getter face");
    assert_eq!(
        getter.message,
        "The getter '[' isn't defined for the type 'C'."
    );
}

/// The enum constructor selector recovery (#1329 FAM17): the
/// parser-inserted synthetic selector keeps the constructor check under
/// the empty name, and the unreferenced `E.foo` reports
/// `unused_element` at its declared name (the corpus's
/// `issue_48380_1` shape).
///
/// The parse tier rides the capture face (regression case 5ad4a307):
/// the selector's `missing_identifier` at the `;` ("Expected an
/// identifier.", length 1) plus the argument tail's `expected_token`
/// ("Expected to find '('.", length 1, anchored on the follower —
/// upstream Dart SDK source, pkg/_fe_analyzer_shared/lib/src/parser/
/// parser_impl.dart parseConstructorInvocationArguments). The pin
/// holds the merged four-row set.
#[test]
fn synthetic_enum_selector_reports_the_recovery_faces() {
    let report = report(
        "class int {}\nenum E<F> {\n  v<int>./*about to write foo()*/;\n\n  const E();\n  const E.foo();\n}",
    );
    let mut names = code_names(&report);
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "expected_token",
            "missing_identifier",
            "undefined_enum_constructor",
            "unused_element",
        ],
        "{:?}",
        names
    );
    let semi = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "expected_token")
        .expect("argument-tail face");
    assert_eq!(semi.message, "Expected to find '('.");
    assert_eq!(semi.range.length, 1);
    let selector = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "missing_identifier")
        .expect("selector face");
    assert_eq!(selector.message, "Expected an identifier.");
    assert_eq!(selector.range.length, 1);
    let ctor = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "undefined_enum_constructor")
        .expect("selector face");
    assert_eq!(
        ctor.message,
        "The enum doesn't have a constructor named ''."
    );
    let unused = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "unused_element")
        .expect("constructor use face");
    assert_eq!(unused.message, "The declaration 'E.foo' isn't referenced.");
}

/// A const creation whose target the file declares without a const
/// constructor reports `const_with_non_const` at the const keyword;
/// a const target stays silent (#1329 FAM9).
#[test]
fn const_creation_over_a_non_const_target_reports_at_the_keyword() {
    let report = report(
        "class C { C(); } var a = const C(); \
             class K { const K(); } var b = const K();",
    );
    assert_eq!(
        code_names(&report),
        ["const_with_non_const"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The constructor being called isn't a const constructor."
    );
}

/// The nameless recovery declaration (#1329 FAM4): a top-level
/// variable whose name the parser never produced joins under the
/// empty name and reports `<unnamed>` at the file-start anchor.
#[test]
fn nameless_recovery_declaration_reports_unnamed() {
    let report = report("var class V {}");
    let unused = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "unused_element")
        .expect("nameless face");
    assert_eq!(
        unused.message,
        "The declaration '<unnamed>' isn't referenced."
    );
    assert_eq!(unused.range.offset, 0);
}

// —— #1456: the R780 parity micro-faces (F-N receiver faces, F-O
// code swaps) ———————————————————————————————————————————————————————
// Each pin mirrors one residual evidence row (the N2 serial capture,
// dart 3.13.1 oracle): the oracle text is asserted verbatim where the
// message renders.

/// `super?.x` keeps the super-member resolution after the (matched)
/// operator error: the `?.`-on-super report never suppresses the
/// selector, and a class with no explicit supertype still resolves
/// `super.member` against `Object` (walk_super's Object face —
/// upstream Dart SDK source, pkg/analyzer/lib/src/generated/
/// element_resolver.dart super resolution).
///
/// Oracle: invalid_operator_questionmark_period_for_super 4:19 +
/// undefined_super_member 4:21 ("The getter 'x' isn't defined in a
/// superclass of 'C'.", invalid_operator_questionmark_period_for_super
/// .dart).
#[test]
fn super_null_aware_keeps_the_super_member_resolution() {
    // `class int {}` follows the harness convention (#537): the return
    // type resolves through a concrete declaration, its
    // `undefined_class` stays out of the super-face pin.
    let report = report("class int {} class C { int m() => super?.x; }");
    assert_eq!(
        code_names(&report),
        [
            "invalid_operator_questionmark_period_for_super",
            "undefined_super_member",
        ],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[1].message,
        "The getter 'x' isn't defined in a superclass of 'C'."
    );
}

/// Invoking an extension NAME is not a call: the plain invocation
/// reports `undefined_function` while the type-argumented override
/// (`Ext<int>(l)`) stays callable with its own arity face
/// (extension_named_type.dart 7:14; upstream Dart SDK source,
/// pkg/analyzer/lib/src/dart/element/element.dart — extension elements
/// are not executable).
/// The explicit extension application without a type-argument group is
/// an override exactly like its parameterized twin: the resolver's
/// send rewrite answers `ExtensionElement` before any function shape
/// (upstream Dart SDK source, pkg/analyzer/lib/src/dart/resolver/
/// ast_rewrite.dart — the `ExtensionElementImpl` arm), so the
/// applicable application stays silent.
#[test]
fn extension_name_invocation_is_a_silent_override() {
    let report = report("class A {} extension Ext on A { void m() {} } void f(A a) { Ext(a); }");
    assert!(report.diagnostics.is_empty(), "{:?}", code_names(&report));
}

/// The extension override shape keeps its invocation: a type-argument
/// group marks the head as an override, so no `undefined_function`
/// fires and the arity check owns the group.
#[test]
fn extension_override_invocation_stays_silent() {
    let report = report(
        "class int {} class List<T> {} class A {} extension Ext<T> on List<T> { void m() {} } \
             void f(List<int> l) { Ext<int>(l); }",
    );
    assert!(report.diagnostics.is_empty(), "{:?}", code_names(&report));
}

/// A selector after a symbol literal resolves against `Symbol`:
/// `#void.foo` reports `undefined_getter 'foo'` on the `Symbol` face
/// while the bare `#void;` statement stays silent (symbols.dart 3:9).
#[test]
fn symbol_literal_member_chain_resolves_on_symbol() {
    let report = report("void main() { #void; #void.foo; }");
    assert_eq!(
        code_names(&report),
        ["undefined_getter"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The getter 'foo' isn't defined for the type 'Symbol'."
    );
}

/// The cascade section's index operand still resolves: the fold that
/// rewrites the receiver's `[]` intent defers the operand's unresolved
/// row, so both the cascade target and the index operand report
/// (issue_42267 2:3/2:16 — upstream Dart SDK source,
/// pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart resolves
/// every subexpression).
#[test]
fn cascade_index_operand_resolves() {
    let report = report("void main() { builder..foo[bar]; }");
    assert_eq!(
        code_names(&report),
        ["undefined_identifier", "undefined_identifier"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(report.diagnostics[0].message, "Undefined name 'builder'.");
    assert_eq!(report.diagnostics[1].message, "Undefined name 'bar'.");
}

/// Indexing a type literal applies the operator to the literal's
/// static type `Type`, which declares no `[]`: the report anchors on
/// the bracket pair (issue_45251_list 21:36 — "The operator '[]' isn't
/// defined for the type 'Type'.").
#[test]
fn type_literal_index_reports_type_operator() {
    let source = "class List {} void f() { List[null]; }";
    let report = report(source);
    assert_eq!(
        code_names(&report),
        ["undefined_operator"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The operator '[]' isn't defined for the type 'Type'."
    );
    let bracket = offset_of(source, "[null]");
    assert_eq!(report.diagnostics[0].range.offset, bracket);
    assert_eq!(report.diagnostics[0].range.length, 6);
}

/// The recovered enum element's argument keeps the plain-name coding:
/// `this(int x)` recovers `(int, x)` as two positional arguments and
/// `x` reports `undefined_identifier` (the oracle's 6:12 row), never
/// `undefined_function` — the assert-initializer group that follows
/// must not adopt `x` as its callee (the open-paren adjacency gate).
#[test]
fn recovered_enum_element_argument_stays_undefined_identifier() {
    let report = report(
        "class int {} \
             enum const E2() { this(int x) : assert(1 > 2) {} } \
             enum const E4() { this(int x) : assert(1 > 2); }",
    );
    let names = code_names(&report);
    assert!(!names.contains(&"undefined_function"), "{:?}", names);
    assert_eq!(
        names
            .iter()
            .filter(|name| **name == "undefined_identifier")
            .count(),
        2,
        "{:?}",
        names
    );
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "undefined_identifier")
        .expect("recovered argument miss");
    assert_eq!(row.message, "Undefined name 'x'.");
}

/// The constant-pattern creation head codes as an invoked callee: a
/// missing `const A()` head reports `undefined_function 'A'` (the
/// oracle's coding at issue50591_example2 3:11), never
/// `undefined_identifier` — the pattern path's argument group closes
/// without a send event, so the close resolves the head's link.
#[test]
fn constant_pattern_creation_head_reports_undefined_function() {
    let report =
        report("class Object {} void f(Object? x) { (switch (x) { const A() => 0, _ => 1, }); }");
    assert_eq!(
        code_names(&report),
        ["undefined_function"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The function 'A' isn't defined."
    );
}
// —— #2046 W-E pass pins (write-only uses, for-in frames, catch twins,
// —— unnamed constructors, ignore comments) ————————————————

/// Every diagnostic as `(code name, covered source text)` — the pin
/// shape below asserts both the verdict and its anchor token.
fn anchors<'a>(source: &'a str, report: &FileReport) -> Vec<(&'static str, &'a str)> {
    report
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.name,
                &source[d.range.offset as usize..d.range.end() as usize],
            )
        })
        .collect()
}

/// A write-only compound assignment counts as no use: the official
/// `visitCompoundAssignment`'s `readCountsAsUse` is
/// `parent is! ExpressionStatement` — inside a for-in body just like a
/// straight body (`check_statement_use` cancels the target's read row
/// at the statement window; the loop's merged frame must close the
/// local with its cancel in effect).
#[test]
fn unused_local_write_only_compound_in_for_in_body() {
    let source = "void f(List<String> l) { var b = ''; for (final s in l) { b += s; } }";
    let report = report(source);
    let unused: Vec<_> = anchors(source, &report)
        .into_iter()
        .filter(|(code, _)| code.starts_with("unused"))
        .collect();
    assert_eq!(unused, vec![("unused_local_variable", "b")]);
}

/// The statement for-in's own frame closes with the loop: the loop
/// variable's scope ends at the loop (`end_for_in` pops the frame
/// `begin_for_statement` opened — `end_for_statement` never fires for a
/// for-in), and the enclosing frame's locals must still close after it.
#[test]
fn unused_local_statement_for_in_loop_variable() {
    let source = "void f(x) { for (final i in x) {} }";
    let report = report(source);
    assert_eq!(
        anchors(source, &report),
        vec![("unused_local_variable", "i")]
    );
}

/// A used loop variable stays silent.
#[test]
fn used_for_in_loop_variable_not_reported() {
    let source = "void f(x) { for (final e in x) { print(e); } }";
    let report = report(source);
    assert!(
        !anchors(source, &report)
            .iter()
            .any(|(code, _)| code.starts_with("unused"))
    );
}

/// The collection-element form's loop variable closes with its element
/// scope: the frame opens at `begin_for_control_flow` (the loop var's
/// declaration events fire inside the for-parts head) and closes at
/// `end_for_in_control_flow`.
#[test]
fn unused_local_collection_for_loop_variable() {
    let source = "var c = [for (var i in [1]) 'f'];";
    let report = report(source);
    assert_eq!(
        anchors(source, &report),
        vec![("unused_local_variable", "i")]
    );
}

/// Compound write-only cancellation is statement-scoped: two operand
/// rows (`b += s`) cancel the target row, the RHS read stays a use.
#[test]
fn unused_local_compound_write_two_operand_statement() {
    let source = "void f(int s) { var b = ''; b += s; }";
    let report = report(source);
    let unused: Vec<_> = anchors(source, &report)
        .into_iter()
        .filter(|(code, _)| code.starts_with("unused"))
        .collect();
    assert_eq!(unused, vec![("unused_local_variable", "b")]);
}

/// Catch clauses' double-pass parse must not ghost a second verdict:
/// both parameters used through calls report nothing.
#[test]
fn catch_variables_used_via_calls_not_reported() {
    let source =
        "void f() { try { throw 1; } on int catch (e, stack) { print(e); print(stack); } }";
    let report = report(source);
    assert!(
        !anchors(source, &report)
            .iter()
            .any(|(code, _)| code.starts_with("unused"))
    );
}

/// A plain `catch (e)` auto-uses its exception variable (official
/// visitCatchClause, unused_local_elements_verifier.dart:67-82) — but
/// the stack parameter carries no such exemption even in a bare catch:
/// `print(e)` spares only `e`.
#[test]
fn bare_catch_exception_auto_used() {
    let source = "void f() { try { throw 1; } catch (e, stack) { print(e); } }";
    let report = report(source);
    let unused: Vec<_> = anchors(source, &report)
        .into_iter()
        .filter(|(code, _)| code.starts_with("unused"))
        .collect();
    assert_eq!(unused, vec![("unused_catch_stack", "stack")]);
}

/// The stack parameter carries no auto-use: an untouched `stack`
/// reports while its twin exception variable stays auto-used.
#[test]
fn unused_catch_stack_reports_alone() {
    let source = "void f() { try { throw 1; } catch (e, stack) {} }";
    let report = report(source);
    let unused: Vec<_> = anchors(source, &report)
        .into_iter()
        .filter(|(code, _)| code.starts_with("unused"))
        .collect();
    assert_eq!(unused, vec![("unused_catch_stack", "stack")]);
}

/// `_visitConstructorElement` visits named constructors only
/// (`node.name != null`, unused_local_elements_verifier.dart:716-723):
/// a single unnamed constructor of a private class is never reported —
/// the class face owns the reference — and the method-declaration
/// event's member twin must not resurrect it.
#[test]
fn private_unnamed_constructor_not_reported() {
    let source = "class _Priv { _Priv(r); }";
    let report = report(source);
    assert_eq!(anchors(source, &report), vec![("unused_element", "_Priv")]);
}

/// A named constructor the file never references reports at its
/// selector token; the unnamed sibling stays exempt.
#[test]
fn enum_named_constructor_unreferenced_reports() {
    let source = "enum E { a; E(); E.named(); }";
    let report = report(source);
    assert_eq!(anchors(source, &report), vec![("unused_element", "named")]);
}

/// The standalone `// ignore:` comment suppresses the NEXT line's
/// matching row.
#[test]
fn ignore_line_suppresses_next_line_unused_element() {
    let source = "// ignore: unused_element\nvar _y = 0;";
    let report = report(source);
    assert!(anchors(source, &report).is_empty());
}

/// `ignore_for_file:` suppresses the whole file.
#[test]
fn ignore_for_file_suppresses_unused_element() {
    let source = "// ignore_for_file: unused_element\nvar _y = 0;";
    let report = report(source);
    assert!(anchors(source, &report).is_empty());
}

/// Extra leading slashes still count for `ignore:` — upstream
/// `isIgnoreComment` requires two slashes and allows more
/// (ignore_info.dart:217-249) — so a `///` doc comment carrying the
/// marker suppresses just like `//`.
#[test]
fn doc_comment_ignore_still_suppresses_next_line() {
    let source = "/// ignore: unused_element\nvar _y = 0;";
    let report = report(source);
    assert!(anchors(source, &report).is_empty());
}

/// The extra-slash tolerance is `ignore:`-only: `ignore_for_file:`
/// demands exactly two slashes (ignore_info.dart:251-268 leaves the
/// extra-slash loop commented out), so a doc comment carrying it is
/// inert and the row stands.
#[test]
fn doc_comment_ignore_for_file_is_inert() {
    let source = "/// ignore_for_file: unused_element\nvar _y = 0;";
    let report = report(source);
    assert_eq!(anchors(source, &report), vec![("unused_element", "_y")]);
}

/// ERROR-severity rows never suppress (ignore_info.dart's gate).
#[test]
fn ignore_comment_never_suppresses_error_severity() {
    let source = "// ignore: undefined_class\nC x = C();";
    let report = report(source);
    assert!(
        anchors(source, &report)
            .iter()
            .any(|(code, _)| *code == "undefined_class")
    );
}

/// A parenthesized group's row carries the inner expression's type —
/// `(e)` has exactly e's static type (the official
/// ParenthesizedExpression.staticType) — so a member access on the
/// group resolves: the ambiguity face fires through the parens, an
/// interface member stays silent, and a miss still reports.
#[test]
fn parenthesized_receiver_carries_the_inner_type() {
    let ambiguous = concat!(
        "class C {}\n",
        "extension E1 on C { C get g => C(); }\n",
        "extension E2 on C { C get g => C(); }\n",
        "void f(C c) { (c).g; }\n",
    );
    assert_eq!(
        code_names(&report(ambiguous)),
        ["ambiguous_extension_member_access"],
    );
    let used = concat!("class C { C get g => C(); }\n", "void f(C c) { (c).g; }\n",);
    assert!(code_names(&report(used)).is_empty(),);
    let missed = concat!("class C {}\n", "void f(C c) { (c).nope; }\n",);
    assert_eq!(code_names(&report(missed)), ["undefined_getter"],);
}

/// Two applicable faces on a nullable `on` type against a nullable
/// receiver report the ambiguity AND the unchecked-access face (the
/// oracle's paired rows at the same access: the verdict leaves the
/// property unresolved for the null-safety walk).
#[test]
fn ambiguous_nullable_extension_reports_unchecked_companion() {
    let source = concat!(
        "class C {}\n",
        "extension E1 on C? { C get g => C(); C m() => C(); }\n",
        "extension E2 on C? { C get g => C(); C m() => C(); }\n",
        "void f(C? c) { (c).g; (c).m(); }\n",
    );
    let r = report(source);
    let mut codes = code_names(&r);
    codes.sort_unstable();
    assert_eq!(
        codes,
        [
            "ambiguous_extension_member_access",
            "ambiguous_extension_member_access",
            "unchecked_use_of_nullable_value",
            "unchecked_use_of_nullable_value",
        ],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
            r.diagnostics
                .iter()
                .find(|d| d.code.name == "unchecked_use_of_nullable_value")
                .map(|d| d.message.to_string()),
            Some(
                "The property 'g' can't be unconditionally accessed because the receiver can be 'null'."
                    .to_string()
            )
        );
}

/// A single nullable-on face applies to the nullable receiver
/// directly: the access is legal and silent (the null-safety face
/// rides the ambiguity verdict only).
#[test]
fn single_nullable_extension_access_stays_silent() {
    let source = concat!(
        "class C {}\n",
        "extension E1 on C? { C get g => C(); }\n",
        "void f(C? c) { (c).g; }\n",
    );
    assert!(code_names(&report(source)).is_empty(),);
}

/// A function-typed parameter is invocable: the written signature is
/// the callable record (the element model poisons the annotation into
/// a `Named`, so the callable face rides the captured signature), and
/// the invocation's own argument check reads the same signature.
#[test]
fn function_typed_parameter_is_invocable() {
    let source = concat!(
        "class int {}\n",
        "void f(void Function(int) fn, int a) {\n",
        "  fn(a);\n",
        "}\n",
    );
    assert!(
        code_names(&report(source)).is_empty(),
        "{:?}",
        code_names(&report(source))
    );
}

/// The dart:core facade: an unshadowed bare `print` read carries the
/// core signature (`void print(Object?)`), so the argument faces read
/// it — a non-function formal refuses with the written shapes at the
/// identifier, and the function supertypes (`Object`) stay silent. A
/// same-file tear-off rides the same refusal. The harness has no core
/// model: the unresolved `print` read keeps its own downstream
/// `undefined_identifier` row beside the assignability face (the
/// oracle resolves print against dart:core and reports neither).
#[test]
fn core_print_tear_off_argument_checks_against_the_formal() {
    let source = concat!(
        "class int {}\n",
        "class Object {}\n",
        "void c(int x) {}\n",
        "void h(Object o) {}\n",
        "void d() {\n",
        "  c(print);\n",
        "  h(print);\n",
        "}\n",
        "int top() => 0;\n",
        "void c2(int x) {}\n",
        "void d2() {\n",
        "  c2(top);\n",
        "}\n",
    );
    let r = report(source);
    let mut codes = code_names(&r);
    codes.sort_unstable();
    assert_eq!(
        codes,
        [
            "argument_type_not_assignable",
            "argument_type_not_assignable",
            "undefined_identifier",
            "undefined_identifier",
        ],
        "{:?}",
        code_names(&r)
    );
    let mut anchored = anchors(source, &r);
    anchored.sort_unstable();
    assert_eq!(
        anchored,
        [
            ("argument_type_not_assignable", "print"),
            ("argument_type_not_assignable", "top"),
            ("undefined_identifier", "print"),
            ("undefined_identifier", "print"),
        ]
    );
    assert_eq!(
        r.diagnostics[0].message.to_string(),
        "The argument type 'void Function(Object?)' can't be assigned to the parameter type 'int'. "
    );
    assert_eq!(
        r.diagnostics[1].message.to_string(),
        "The argument type 'int Function()' can't be assigned to the parameter type 'int'. "
    );
}

/// Members of an unnamed extension are checked like private
/// declarations (`_isPubliclyAccessible`'s extension arm): an
/// unreferenced member reports, an applied member is spared, and a
/// named extension's members stay publicly accessible.
/// The `ConstructorFieldsVerifier` per-constructor rows: a
/// non-nullable non-final instance field reports at EVERY generative
/// constructor that fails to initialize it, anchored at the head the
/// constructor starts at (the class-name token, named selectors
/// included); a constructor that initializes the field — through its
/// own initializer list or an initializing formal — spares only
/// itself. A generative constructor with no explicit super
/// satisfaction beside it also reports the implicit-super face when
/// the superclass's unnamed constructor declares required positional
/// parameters.
#[test]
fn uninitialized_field_reports_per_generative_constructor() {
    let source = concat!(
        "class int {}\n",
        "class C {\n",
        "  int p;\n",
        "  C(int a);\n",
        "  C.named({int c = 0});\n",
        "  C.used(int b) : p = b;\n",
        "}\n",
        "class D extends C {\n",
        "  int q;\n",
        "  D(this.q, int used) : super.named(c: used);\n",
        "  D.wild(int _, int __);\n",
        "  D.deprecated(int old);\n",
        "}\n",
    );
    let r = report(source);
    assert_eq!(
        anchors(source, &r),
        vec![
            ("not_initialized_non_nullable_instance_field", "C"),
            ("not_initialized_non_nullable_instance_field", "C"),
            ("implicit_super_initializer_missing_arguments", "D"),
            ("not_initialized_non_nullable_instance_field", "D"),
            ("implicit_super_initializer_missing_arguments", "D"),
            ("not_initialized_non_nullable_instance_field", "D"),
        ],
        "{:?}",
        anchors(source, &r)
    );
    assert_eq!(
        r.diagnostics
            .iter()
            .find(|d| d.code.name == "implicit_super_initializer_missing_arguments")
            .map(|d| d.message.to_string()),
        Some(
            "The implicitly invoked unnamed constructor from 'C' has required parameters."
                .to_string()
        )
    );
}

/// A non-final instance field in an enum body reports at its name
/// (`non_final_field_in_enum` — enums can only declare final fields).
#[test]
fn non_final_enum_field_reports_at_the_name() {
    let source = "class int {} enum E { a; E(); int x = 0; E.named(); }";
    let r = report(source);
    assert_eq!(
        anchors(source, &r),
        vec![
            ("non_final_field_in_enum", "x"),
            ("unused_element", "named"),
        ],
        "{:?}",
        anchors(source, &r)
    );
}

#[test]
fn unnamed_extension_member_reports_when_unused() {
    let unused = "class C {} extension on C { C get g => C(); C m() => C(); }";
    let r = report(unused);
    let mut codes = code_names(&r);
    codes.sort_unstable();
    assert_eq!(
        codes,
        ["unused_element", "unused_element"],
        "{:?}",
        code_names(&r)
    );
    let applied = concat!(
        "class C {} extension on C { C get g => C(); C m() => C(); }\n",
        "void f(C c) { c.g; c.m(); }\n",
    );
    assert!(code_names(&report(applied)).is_empty(),);
    let named = concat!("class C {} extension E on C { C get g => C(); }\n",);
    assert!(code_names(&report(named)).is_empty(),);
}

/// #2097 F2: a positional `super.x` associates against the *resolved*
/// super-constructor target at its index among super formals —
/// `super.named()` picks the named table's positional at index 0 and
/// stays silent; only an index past that table's arity reports
/// `super_formal_parameter_without_associated_positional` at the name
/// (error_verifier.dart:2219-2251, `indexIn` at element.dart:10779).
#[test]
fn super_formal_resolves_against_the_invoked_target() {
    // Offset match: `this.w` claims no super slot; `super.x`/`super.y`
    // line up with `C.named`'s two positionals — silent.
    let r = report(
        "class int {} \
             class C { C.named(int x, int y); } \
             class D extends C { D(this.w, super.x, super.y) : super.named(); \
             final int w; }",
    );
    assert_eq!(code_names(&r), Vec::<&str>::new(), "{:?}", code_names(&r));

    // Too many: the second super formal passes the unnamed table's
    // arity (1) — one positional row at its own `x`.
    let r = report(
        "class int {} \
             class C { C(int x); } \
             class D extends C { D(this.w, super.y, super.x); final int w; }",
    );
    assert_eq!(
        code_names(&r),
        ["super_formal_parameter_without_associated_positional"],
        "{:?}",
        code_names(&r)
    );
    // The row anchors at the second super formal's name (`x` of
    // `super.x` — the earlier `super.y` claims index 0).
    let second = r.diagnostics[0].range.offset as usize;
    let src = "class int {} \
             class C { C(int x); } \
             class D extends C { D(this.w, super.y, super.x); final int w; }";
    assert_eq!(&src[second..second + 1], "x");
    assert_eq!(second, src.rfind("x);").unwrap());
}

/// #2097 F2 (primary-constructor face): a primary ctor's `super.`
/// formals wait for the class body's `this : super.named()` —
/// `end_class_declaration` drains them against the captured target;
/// an implicit invocation resolves the unnamed table.
#[test]
fn super_formal_primary_ctor_waits_for_the_body_redirect() {
    // `this : super.named()` — the primary header's formals associate
    // against `C.named`'s table (silent).
    let r = report(
        "class int {} \
             class C { C.named(int x, int y); } \
             class D(this.w, super.x, super.y) extends C { \
             final int w; this : super.named(); }",
    );
    assert_eq!(code_names(&r), Vec::<&str>::new(), "{:?}", code_names(&r));

    // No `this` member — implicit `super()` — the unnamed table's
    // single positional leaves `super.y` unassociated.
    let r = report(
        "class int {} \
             class C { C(int x); } \
             class D(this.w, super.x, super.y) extends C { final int w; }",
    );
    assert_eq!(
        code_names(&r),
        ["super_formal_parameter_without_associated_positional"],
        "{:?}",
        code_names(&r)
    );
}

/// The named arm associates by spelling: `super.z` under
/// `: super.named()` resolves against the target's `{required this.z}`
/// entry — a `this`-selector formal registers under its plain name in
/// the target table's named slot (`element.dart`'s
/// `SuperFormalParameterElement.namedParameter` walk). The inverse
/// shape keeps the check able to fire: no matching named entry in the
/// target's table reports
/// `super_formal_parameter_without_associated_named`.
#[test]
fn super_formal_named_associates_with_this_selector_formal() {
    // lints-new/matching_super_parameters.dart D3/C3 (oracle-silent):
    let r = report(
        "class int {} \
             class C3 { final int x; final int y; final int z; \
             C3.named(this.x, this.y, {required this.z}); } \
             class D3 extends C3 { final int w; \
             D3(this.w, super.x, super.y, {required super.z}) : super.named(); }",
    );
    assert_eq!(code_names(&r), Vec::<&str>::new(), "{:?}", code_names(&r));

    // The target declares no `z` — the named row fires at `super.z`:
    let r = report(
        "class int {} \
             class C { C.named(int x); } \
             class D extends C { D({required super.z}) : super.named(); }",
    );
    assert_eq!(
        code_names(&r),
        ["super_formal_parameter_without_associated_named"],
        "{:?}",
        code_names(&r)
    );
}

/// The reserved `new` head IS the unnamed constructor: `new();` /
/// `const new();` keep `C()` / `const C()` creations callable (the
/// FACTORY_CONSTRUCTOR_NEW_NAME head semantics; oracle-silent at
/// lints-new `CNewSyntax`/`ANewSyntax`).
#[test]
fn new_head_declares_the_unnamed_constructor() {
    let r = report("class int {} class CNewSyntax { new(); } void f() { CNewSyntax(); }");
    assert!(
        code_names(&r).is_empty(),
        "{:?} at {:?}",
        code_names(&r),
        r.diagnostics
    );
    let r =
        report("class int {} class ANewSyntax { const new(); } void f() { const ANewSyntax(); }");
    assert!(
        code_names(&r).is_empty(),
        "{:?} at {:?}",
        code_names(&r),
        r.diagnostics
    );
}

/// The `new`/`factory` head family names its constructors the way the
/// oracle does (package:xml shapes): `const new q(...)` is the named
/// `q` (not the unnamed face), a bare `const new(...)` is the const
/// unnamed face a const factory may redirect to, `factory()` declares
/// the unnamed face, two `factory a(` / `factory b(` heads are distinct
/// constructors, and an enum's `const new(...)` answers its constants.
/// A nested implicit `C.named(...)` argument is itself constant.
#[test]
fn new_and_factory_heads_name_their_constructors() {
    let clean = [
        "class String {} class N { const new q(this.v); final String v; } \
             const n = N.q('x');",
        "class int {} abstract class F { const factory F.a(int x) = _F; } \
             class _F implements F { const new(this.x); final int x; } \
             const f = F.a(1);",
        "class int {} class L { factory() => L._(1); new _(this.i); final int i; } \
             final l = L();",
        "class String {} class T { T(this.m); factory a(String x) => T(x); \
             factory b(String x) => T(x); final String m; } final t = T.a('x');",
        "class String {} enum E { one('a'); const new(this.t); final String t; }",
        "class Object {} class O { const O.q(this.v); final Object v; } \
             class P { const P(this.n); final Object n; } const p = P(O.q(1));",
        // A redirect references its target class and supplies every
        // optional formal of the target constructor.
        "class int {} abstract class G { const factory G.a({int? y}) = _G.named; } \
             class _G implements G { const new named({this.y}); final int? y; }",
    ];
    for src in clean {
        let r = report(src);
        assert!(code_names(&r).is_empty(), "{src}: {:?}", r.diagnostics);
    }
}

/// A primary constructor registers under its header shape: the
/// selector-less header is the unnamed face (`C(1)` resolves) and a
/// `.named` selector registers the named spelling (`C.named(2)`
/// resolves — lints-new `use_enums.dart` `UEPrimary*._` /
/// `prefer_const_constructors.dart` `CPrimaryNamed.named`).
#[test]
fn primary_ctor_registers_its_creation_faces() {
    let r = report("class int {} class C(int x); void f() { C(1); }");
    assert!(
        code_names(&r).is_empty(),
        "{:?} at {:?}",
        code_names(&r),
        r.diagnostics
    );
    let r = report("class int {} class C.named(int x); void f() { C.named(2); }");
    assert!(
        code_names(&r).is_empty(),
        "{:?} at {:?}",
        code_names(&r),
        r.diagnostics
    );
}

/// Explicit type arguments on the unnamed selector face check against
/// the CLASS's declared count (constructors never declare type
/// parameters — m0-syntax `const_patterns.dart:81`
/// `GenericClass<int>.new()` is oracle-silent; a mismatched count
/// reports the base `wrong_number_of_type_arguments` face).
#[test]
fn creation_unnamed_selector_checks_class_arity() {
    let r = report("class int {} class G<T> { G.new(); } void f() { G<int>.new(); }");
    assert!(
        code_names(&r).is_empty(),
        "{:?} at {:?}",
        code_names(&r),
        r.diagnostics
    );
    let r = report("class int {} class G<T> { G.new(); } void f() { G<int, int>.new(); }");
    assert_eq!(
        code_names(&r),
        ["wrong_number_of_type_arguments"],
        "{:?}",
        code_names(&r)
    );
}

/// A const constructor whose own formal default const-instantiates the
/// same constructor depends on itself (oracle
/// `recursive_constant_constructor` at lints-new `use_enums.dart:87`):
/// the unnamed-face default `const A()` and the selector default
/// `const A.sel()` both match; defaults naming other constructors stay
/// silent.
#[test]
fn const_ctor_default_self_instantiation_reports() {
    let r = report("class int {} class A { const A({A e = const A()}); }");
    assert_eq!(
        code_names(&r),
        ["recursive_constant_constructor"],
        "{:?}",
        code_names(&r)
    );
    let r = report("class int {} class A { const A.named({A e = const A.named()}); }");
    assert_eq!(
        code_names(&r),
        ["recursive_constant_constructor"],
        "{:?}",
        code_names(&r)
    );
    let r = report("class int {} class A { const A({int e = 0}); }");
    assert_eq!(code_names(&r), Vec::<&str>::new(), "{:?}", code_names(&r));
}

/// A bare `Future` return type flattens to dynamic: an `async` body
/// completing normally stays oracle-silent (lints-new
/// `close_sinks.dart:189` — the over-fired
/// `body_might_complete_normally`); a non-nullable value return keeps
/// the row.
#[test]
fn bare_future_async_body_completes_normally_silent() {
    let r = report("class int {} class Future<T> {} class C { Future m1() async {} }");
    assert_eq!(code_names(&r), Vec::<&str>::new(), "{:?}", code_names(&r));
    let r = report("class int {} class C { int m1() {} }");
    assert_eq!(
        code_names(&r),
        ["body_might_complete_normally"],
        "{:?}",
        code_names(&r)
    );
}

/// `this` in an extension-on-nullable keeps the written `on` type's
/// nullability: `this == null` is a real comparison, not an
/// always-true one (lints-new `unnecessary_this_alias.dart` — the
/// over-fired `unnecessary_null_comparison`/`unnecessary_non_null_
/// assertion`/`invalid_null_aware_operator` family).
#[test]
fn this_in_extension_on_nullable_stays_nullable() {
    let r = report(
        "class int {} class C {} extension E on C? { \
             void m() { if (this == null) { return; } } }",
    );
    assert_eq!(code_names(&r), Vec::<&str>::new(), "{:?}", code_names(&r));
    // The plate face is the suppressed `unnecessary_non_null_assertion`
    // (the oracle leaves `self!` on a nullable receiver unflagged):
    let r = report(
        "class int {} class C {} extension E on C? { \
             void m() { var self = this; self!; } }",
    );
    assert!(
        !code_names(&r)
            .iter()
            .any(|name| *name == "unnecessary_non_null_assertion"),
        "{:?}",
        code_names(&r)
    );
}

/// A bare WRITE resolves a declared setter with no getter — top-level
/// (`fTopSetter = 1`) and instance (`m = 2`) alike; the pre-fix face
/// over-fired `undefined_identifier` (lints-new
/// `deprecated_member_use_from_same_package.dart:584`).
#[test]
fn bare_write_resolves_a_declared_setter() {
    let r = report("class int {} set fTopSetter(int v) {} void f() { fTopSetter = 1; }");
    assert_eq!(code_names(&r), Vec::<&str>::new(), "{:?}", code_names(&r));
    let r = report("class int {} class C { set m(int v) {} void f() { m = 2; } }");
    assert_eq!(code_names(&r), Vec::<&str>::new(), "{:?}", code_names(&r));
}

/// The `unused_field` read rule is receiver-scoped: a read of one
/// class's `_f` spares only that class's field — a same-spelled
/// unread sibling still reports (lints-new `close_sinks.dart`: the
/// reads spared every `_sink` in the unit, killing the `unused_field`
/// face and firing `unnecessary_ignore` on its comments).
#[test]
fn unused_field_read_is_receiver_scoped() {
    let r = report("class int {} class A { int _f = 0; } class B { int _f = 0; void g() { _f; } }");
    assert_eq!(code_names(&r), ["unused_field"], "{:?}", code_names(&r));
}

/// A named argument GIVEN at an enum-constant instantiation or a
/// `this(...)` redirect counts as a use of the parameter — the
/// over-fired `unused_element_parameter` at lints-new
/// `avoid_redundant_argument_values.dart:70` (`a(test: false)`).
#[test]
fn named_argument_use_spares_the_parameter() {
    let r = report(
        "class int {} enum TestEnumRedundant { \
             a(test: 0); const TestEnumRedundant({this.test = 0}); final int test; }",
    );
    assert_eq!(code_names(&r), Vec::<&str>::new(), "{:?}", code_names(&r));
}

/// #2097 F1: a primary constructor counts as a generative constructor
/// (constructor_fields_verifier.dart `_addPrimaryConstructor`; the
/// `hasGenerativeConstructor` fold reads `PrimaryConstructorBodyImpl`,
/// error_verifier.dart:7111-7122) — a `this.x` formal or a
/// `this : _x = v` body initializer initializes the field, so the
/// declaration walk emits no `final_not_initialized_constructor`.
#[test]
fn primary_ctor_field_formals_satisfy_final_field_init() {
    // `this.x` field formal initializes `x`; the class's own `final w`
    // stays uninitialized and reports once at its declaration.
    let r = report("class int {} class C(this.x) { final int x; final int w; }");
    assert_eq!(
        code_names(&r),
        ["final_not_initialized_constructor"],
        "{:?}",
        code_names(&r)
    );

    // `this : _x = v` in the primary body initializes through the
    // initializer write (updateWithInitializers) — `final` satisfied;
    // `unused_field` rides alongside (a write-only field IS unused,
    // upstream's read-only rule).
    let r = report("class int {} class C(int v) { final int _x; this : _x = v; }");
    let names = code_names(&r);
    assert!(
        !names.contains(&"final_not_initialized_constructor"),
        "{names:?}"
    );
    assert!(names.contains(&"unused_field"), "{names:?}");

    // A primary ctor with no `this.` formal and no initializer — the
    // field reports beside the declaration (error_verifier.dart:7055-
    // 7085 walk, ctor-site anchored).
    let r = report("class int {} class C(int v) { final int x; }");
    assert_eq!(
        code_names(&r),
        ["final_not_initialized_constructor"],
        "{:?}",
        code_names(&r)
    );
}

/// #2097 F10: a factory's `= C.name` redirect participates in the
/// `recursive_constructor_redirect` cycle walk — `foo = bar; bar = foo`
/// reports once on each edge (upstream anchors the redirect type's full
/// span, error_verifier.dart's `visitRedirectingFactoryConstructor`
/// cycle leg).
#[test]
fn factory_redirect_cycle_reports_on_each_edge() {
    let r = report("class A { factory A.foo() = A.bar; factory A.bar() = A.foo; }");
    assert_eq!(
        code_names(&r),
        [
            "recursive_constructor_redirect",
            "recursive_constructor_redirect",
        ],
        "{:?}",
        code_names(&r)
    );

    // A non-cyclic redirect stays silent.
    let r = report("class A { factory A.foo() = A.bar; A.bar(); }");
    assert_eq!(code_names(&r), Vec::<&str>::new(), "{:?}", code_names(&r));
}

/// #2097 F8: a bare `C()` miss inside *any* class member (static
/// included) names the enclosing type as `undefined_method`, not the
/// top-level `undefined_function` (error_verifier.dart's unqualified-
/// reference domain); the return-type `C` still reports
/// `undefined_class`.
#[test]
fn static_member_bare_invoke_miss_names_the_enclosing_type() {
    let r = report("class CStaticTear<T> { static C<X> m<X>() => C<X>(); }");
    let names = code_names(&r);
    assert!(names.contains(&"undefined_class"), "{names:?}");
    assert!(names.contains(&"undefined_method"), "{names:?}");
    assert!(!names.contains(&"undefined_function"), "{names:?}");
}

/// #2097 F10: a `String this.p` formal binds its declared type into the
/// ctor body's expression walk — `assert(p != null)` is an invariant
/// comparison (best_practices_verifier.dart `_checkForInvariantNull-
/// Comparison`), reporting `unnecessary_null_comparison` at `!= null`.
#[test]
fn this_formal_declared_type_reaches_the_body_assert() {
    let r = report(
        "class int {} class String {} \
             class C { C(String this.p) : assert(p != null); final String p; }",
    );
    assert_eq!(
        code_names(&r),
        ["unnecessary_null_comparison"],
        "{:?}",
        code_names(&r)
    );
    assert_eq!(
        r.diagnostics[0].message,
        "The operand can't be 'null', so the condition is always 'true'."
    );
}

/// #2097 F9-1: `const C()` against a *forward-declared* const
/// constructor stays silent — the verdict reads the element model's
/// const-constructor bit, which is whole-file by construction.
#[test]
fn forward_declared_const_ctor_stays_silent() {
    let r = report("void m() { const C(); } class C { const C(); }");
    assert_eq!(code_names(&r), Vec::<&str>::new(), "{:?}", code_names(&r));

    // A forward class with no const constructor still emits at the
    // `const` keyword.
    let r = report("void m() { const C(); } class C { C(); }");
    assert_eq!(
        code_names(&r),
        ["const_with_non_const"],
        "{:?}",
        code_names(&r)
    );
}

/// #2097 F9-2: `const APatternIf(0)` inside a pattern arms nothing for
/// the *literal* — `top_is_constant_pattern` is false inside the
/// nested ctor window, so `0` never claims the `int` constant slot and
/// the never-matches check (constant_verifier.dart:130-165) stays
/// silent on a class without primitive equality.
#[test]
fn nested_const_ctor_literal_does_not_arm_the_pattern_constant() {
    let r = report(
        "class int {} class A { const A(this.v); final int v; } \
             void f(A a) { if (a case const A(0)) {} }",
    );
    assert!(
        !code_names(&r).contains(&"constant_pattern_never_matches_value_type"),
        "{:?}",
        code_names(&r)
    );
}

// —— #2184: pattern-context type-name resolution ————————————————————————
// The object-pattern head reaches the diagnoser through the dedicated
// `handle_object_pattern` event (no `handle_identifier(TypeReference)` +
// `handle_type` pair), so it never entered the `push_type_use` →
// `RowShape::TypeUse` → `resolve_type_row` chain that reports
// `undefined_class` in every other type position — the oracle resolves
// `ObjectPattern.type` through the ordinary `visitNamedType` pass
// (named_type_resolver.dart's `reportNullOrNonTypeElement` fallthrough).
// The cast pattern's `as` type is the second face: it rode the
// expression-cast suppression bracket, but the oracle's `castToNonType`
// arm requires an `AsExpression` parent — a CastPattern's type falls
// through to the plain arms.

/// The diagnostic rows' `(code, offset)` pairs, in source order.
fn code_offsets(report: &FileReport) -> Vec<(&'static str, u32)> {
    report
        .diagnostics
        .iter()
        .map(|d| (d.code.name, d.range.offset))
        .collect()
}

/// F1: a bare object-pattern head in a switch-statement case resolves
/// through the plain type-name pass — the miss reports `undefined_class`
/// at the head identifier, a defined head stays silent (`class Object {}`
/// stands in for the core type, the harness's no-core convention).
#[test]
fn object_pattern_head_reports_undefined_class() {
    let source = "class Ok {}\nclass Object {}\nvoid f(Object v) {\n  switch (v) {\n    case Missing1():\n      break;\n    case Ok():\n      break;\n  }\n}";
    let r = report(source);
    assert_eq!(
        code_offsets(&r),
        [("undefined_class", offset_of(source, "Missing1"))],
        "{:?}",
        code_offsets(&r)
    );
}

/// F2: switch-expression arm heads — both operands of a logical-or
/// pattern — report independently; the defined arm stays silent. The
/// bound variable `x` keeps its ordinary `unused_local_variable` row,
/// and the open switch keeps its `non_exhaustive_switch_expression`
/// verdict (a wildcard arm would close it).
#[test]
fn switch_expression_arm_heads_report() {
    let source = "class Ok {}\nclass Object {}\nvoid f(Object v) {\n  var r = switch (v) {\n    Missing2(:var x) || Missing3() => 1,\n    Ok() => 2,\n  };\n  return r;\n}";
    let r = report(source);
    assert_eq!(
        code_offsets(&r),
        [
            (
                "non_exhaustive_switch_expression",
                offset_of(source, "switch (v)"),
            ),
            ("undefined_class", offset_of(source, "Missing2")),
            ("unused_local_variable", offset_of(source, "x)")),
            ("undefined_class", offset_of(source, "Missing3")),
        ],
        "{:?}",
        code_offsets(&r)
    );
}

/// F3: an if-case head reports; a defined typed field sub-pattern and
/// a defined head stay silent. The bound variables keep their ordinary
/// `unused_local_variable` rows (`class String {}` stands in for the
/// core type).
#[test]
fn if_case_head_reports_typed_field_silent() {
    let source = "class Ok {\n  var y;\n}\nclass String {}\nclass Object {}\nvoid f(Object v) {\n  if (v case Missing4(:String s)) {}\n  if (v case Ok(:var y)) {}\n}";
    let r = report(source);
    assert_eq!(
        code_offsets(&r),
        [
            ("undefined_class", offset_of(source, "Missing4")),
            ("unused_local_variable", offset_of(source, " s") + 1),
            ("unused_local_variable", offset_of(source, " y)") + 1),
        ],
        "{:?}",
        code_offsets(&r)
    );
}

/// F4: the head's own type-argument group keeps the type-arg face —
/// the head reports `undefined_class`, a missing argument reports
/// `non_type_as_type_argument` (the oracle's
/// `_isTypeInTypeArgumentList` arm outranks the fallthrough).
#[test]
fn object_pattern_head_with_type_args_reports_both_faces() {
    let source = "class int {}\nclass Object {}\nvoid f(Object v) {\n  switch (v) {\n    case Missing5<int, Missing6>():\n      break;\n  }\n}";
    let r = report(source);
    assert_eq!(
        code_offsets(&r),
        [
            ("non_type_as_type_argument", offset_of(source, "Missing6")),
            ("undefined_class", offset_of(source, "Missing5")),
        ],
        "{:?}",
        code_offsets(&r)
    );
}

/// F5: the cast pattern's `as` type never reaches the `cast_to_non_type`
/// arm (its parent is a CastPattern, not an AsExpression) — a miss
/// reports `undefined_class`, a resolved non-type `not_a_type`. The
/// expression cast keeps its dedicated face. The bound `x`/`y` keep
/// their `unused_local_variable` rows.
#[test]
fn cast_pattern_type_reports_plain_faces() {
    let source = "class Object {}\nvoid f(Object v) {\n  switch (v) {\n    case var x as Missing7:\n      break;\n  }\n}";
    let r = report(source);
    assert_eq!(
        code_offsets(&r),
        [
            ("unused_local_variable", offset_of(source, " x as") + 1),
            ("undefined_class", offset_of(source, "Missing7")),
        ],
        "{:?}",
        code_offsets(&r)
    );

    // A parameter head is a resolved non-type: `not_a_type` with the
    // declaration context (the oracle's `element != null` arm).
    let source = "class int {}\nclass Object {}\nvoid f(int notAType, Object v) {\n  switch (v) {\n    case var y as notAType:\n      break;\n  }\n}";
    let r = report(source);
    let rows: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == "not_a_type" && d.range.offset == offset_of(source, "as notAType") + 3
        })
        .collect();
    assert_eq!(rows.len(), 1, "{:?}", code_offsets(&r));
    let context = rows[0].context.as_deref().unwrap_or_default();
    assert_eq!(
        context.len(),
        1,
        "the parameter declaration anchor rides: {:?}",
        code_offsets(&r)
    );

    // The expression-cast control: `v as Missing` keeps
    // `cast_to_non_type` — the bracket change must not flip it.
    let source = "class Object {}\nvoid f(Object v) { var w = v as Missing8; }";
    let r = report(source);
    assert_eq!(
        code_offsets(&r),
        [
            ("unused_local_variable", offset_of(source, " w =") + 1),
            ("cast_to_non_type", offset_of(source, "Missing8")),
        ],
        "{:?}",
        code_offsets(&r)
    );
}

/// F6: declared-variable patterns and typed wildcards already ride the
/// normal type events — regression pin, both heads report. `n` keeps
/// its `unused_local_variable` row.
#[test]
fn declared_variable_and_typed_wildcard_heads_report() {
    let source = "class Object {}\nvoid f(Object v) {\n  switch (v) {\n    case Missing8 n:\n      break;\n    case Missing9 _:\n      break;\n  }\n}";
    let r = report(source);
    assert_eq!(
        code_offsets(&r),
        [
            ("undefined_class", offset_of(source, "Missing8")),
            ("unused_local_variable", offset_of(source, " n:") + 1),
            ("undefined_class", offset_of(source, "Missing9")),
        ],
        "{:?}",
        code_offsets(&r)
    );
}

/// F7: a head resolving to a non-type element (a top-level function)
/// reports `not_a_type` — the plain path's `element != null` arm — with
/// the same-file declaration context.
#[test]
fn object_pattern_non_type_head_reports_not_a_type() {
    let source = "class int {}\nclass Object {}\nint topFn() => 0;\nvoid f(Object v) {\n  if (v case topFn()) {}\n}";
    let r = report(source);
    let rows: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == "not_a_type" && d.range.offset == offset_of(source, "case topFn") + 5
        })
        .collect();
    assert_eq!(rows.len(), 1, "{:?}", code_offsets(&r));
    let context = rows[0].context.as_deref().unwrap_or_default();
    assert_eq!(
        context.len(),
        1,
        "the declaration anchor rides: {:?}",
        code_offsets(&r)
    );
}

/// F8: silence pin — defined heads across all three pattern homes push
/// no diagnostic row.
#[test]
fn defined_object_pattern_heads_stay_silent() {
    let source = "class Ok {\n  var field;\n}\nclass Object {}\nvoid f(Object v) {\n  if (v case Ok()) {}\n  if (v case Ok(field: _)) {}\n  var r = switch (v) { Ok() => 1, _ => 2 };\n  return r;\n}";
    let r = report(source);
    assert!(r.diagnostics.is_empty(), "{:?}", code_offsets(&r));
}

// —— #2186 S2 pins — `on`-face member scope inside mixin/extension ————

/// The unresolved `on` anchor: `extension E on Gone` carries an
/// `InvalidType` extended type — the member scope is the extension's own
/// map alone, so a bare miss inside the body reports
/// `undefined_identifier` (lexical_lookup.dart's fall-through) instead
/// of riding the old blanket `on_opaque` suppression. The oracle's
/// `undefined_class` on `Gone` stands beside it (the clause's own
/// type-miss face).
#[test]
fn extension_on_unresolved_reports_bare_miss() {
    let r = report("extension E on Gone {\n  void m() { nope; }\n}");
    assert_eq!(
        code_names(&r),
        ["undefined_class", "undefined_identifier"],
        "{:?}",
        code_names(&r)
    );
}

/// A mixin whose `on` constraint never resolves reports its body's bare
/// miss the same way — the dead edge contributes nothing to the member
/// scope (the oracle's `InvalidType`), and the clause's own `on`-face
/// miss is the type family's row, not this one's.
#[test]
fn mixin_on_unresolved_reports_bare_miss() {
    let r = report("mixin M on Gone {\n  void m() { nope; }\n}");
    // `mixin_super_class_constraint_non_interface` is the constraint's
    // own face (a mixin `on` name must resolve to a class-family
    // element); `undefined_class` is `Gone`'s type-miss row; the bare
    // body's `undefined_identifier` is the row the blanket `on_opaque`
    // skip used to steal.
    assert_eq!(
        code_names(&r),
        [
            "mixin_super_class_constraint_non_interface",
            "undefined_class",
            "undefined_identifier"
        ],
        "{:?}",
        code_names(&r)
    );
}

/// The decidable constraint: `mixin M on C` where `C` is indexed walks
/// `C`'s whole member face — a bare read of a `C` member resolves
/// silently (the `on`-face hit joins the member scope, the oracle's
/// `unpromotedThisType`), a name `C` lacks reports.
#[test]
fn mixin_on_indexed_constraint_decides_members() {
    let r = report(
        "class int {}\n\
             class C {\n  int cf = 0;\n}\n\
             mixin M on C {\n  void m() { cf; gone; }\n}",
    );
    assert_eq!(
        code_names(&r),
        ["undefined_identifier"],
        "{:?}",
        code_names(&r)
    );
}

/// The extension twin: `extension E on C` resolves the `on` type's
/// members bare (upstream's `ExtensionMemberScope`), reports what the
/// extended type lacks.
#[test]
fn extension_on_indexed_type_decides_members() {
    let r = report(
        "class int {}\n\
             class C {\n  int cf = 0;\n}\n\
             extension E on C {\n  void m() { cf; gone; }\n}",
    );
    assert_eq!(
        code_names(&r),
        ["undefined_identifier"],
        "{:?}",
        code_names(&r)
    );
}

/// The `on` type's INHERITED members are in scope too (the face-list
/// climb walks supertypes): a constraint member on `C`'s superclass
/// resolves bare inside `mixin M on C`, and the miss beside it reports.
#[test]
fn mixin_on_constraint_inherits_members() {
    let r = report(
        "class int {}\n\
             class S {\n  int sf = 0;\n}\n\
             class C extends S {}\n\
             mixin M on C {\n  void m() { sf; gone; }\n}",
    );
    assert_eq!(
        code_names(&r),
        ["undefined_identifier"],
        "{:?}",
        code_names(&r)
    );
}

/// A constraint member found through the `on`-face walk resolves bare
/// and stays silent — the read joins the member resolution exactly like
/// an own-container field hit (a second bare read of `gone` beside it
/// reports, pinning that the silence comes from the walk's Found arm,
/// not a suppression).
#[test]
fn on_face_member_carries_the_declared_type() {
    let r = report(
        "class int {}\n\
             class C {\n  int cf = 0;\n}\n\
             mixin M on C {\n  void m() { cf; gone; }\n}",
    );
    assert_eq!(
        code_names(&r),
        ["undefined_identifier"],
        "{:?}",
        code_names(&r)
    );
}

/// The unnamed extension reads the frame-latched face: `extension on
/// Gone` has no container entry, yet its `on` type still decides the
/// bare miss the same way a named face does. (The `unused_element`
/// row is the member's own declaration face — unrelated to this arm.)
#[test]
fn unnamed_extension_on_unresolved_reports_bare_miss() {
    let r = report("extension on Gone {\n  void m() { nope; }\n}");
    let names = code_names(&r);
    assert!(names.contains(&"undefined_identifier"), "{names:?}");
    assert!(names.contains(&"undefined_class"), "{names:?}");
}

/// A named extension whose `on` type resolves to a second EXTENSION's
/// application face is still just an `on` clause — the constraint's
/// member scope is the extended type's members, and an unresolvable
/// `on` name contributes nothing either way.
#[test]
fn extension_on_extension_still_decides_bare_misses() {
    let r = report(
        "class int {}\n\
             extension X on int {}\n\
             extension E on Gone {\n  void m() { nope; }\n}",
    );
    assert_eq!(
        code_names(&r),
        ["undefined_class", "undefined_identifier"],
        "{:?}",
        code_names(&r)
    );
}

/// The interface-clause twin: `mixin M implements C` walks `C`'s
/// members through `walk_members`'s own `implements` faces — a hit
/// resolves, a miss reports through the same `on_opaque`-free path
/// (the implements face was never blanket-suppressed).
#[test]
fn mixin_implements_members_resolve_and_miss_reports() {
    let r = report(
        "class int {}\n\
             class C {\n  int cf = 0;\n}\n\
             mixin M implements C {\n  void m() { cf; gone; }\n}",
    );
    assert_eq!(
        code_names(&r),
        ["undefined_identifier"],
        "{:?}",
        code_names(&r)
    );
}

/// The negative control — an `on` anchor the index cannot model: `on
/// Iterator` where `Iterator` resolves to an unindexed `dart:core` face
/// keeps the suppression (a member could hide on the face we cannot
/// walk). The mixin body's miss stays silent exactly like the old
/// blanket verdict — but only because the climb broke, not because the
/// container kind asked for it.
#[test]
fn mixin_on_unindexed_constraint_keeps_the_miss_silent() {
    let r = report_with_core_faces(
        "import 'dart:core';\n\
             mixin M on Iterator {\n  void m() { nope; }\n}",
    );
    assert!(r.diagnostics.is_empty(), "{:?}", code_names(&r));
}

/// The invoke miss's `UnqualifiedLookupDomain`: inside `mixin M` the
/// bare callee names the enclosing interface element — `undefined_method`
/// on `M` (upstream's `enclosingClass` is an `InterfaceElement`, which
/// mixins are). An extension's bare miss names its `on` type the same
/// way (probed 3.13.1: `Bar` reports "for the type 'int'" — the
/// extended type is `this`'s static type in the member lookup, there
/// is no top-level `undefined_function` fallback).
#[test]
fn mixin_and_extension_invoke_misses_name_the_face_type() {
    let r = report(
        "class int {}\n\
             mixin M {\n  void m() { Foo(); }\n}\n\
             extension E on int {\n  void g() { Bar(); }\n}",
    );
    let names = code_names(&r);
    assert_eq!(names, ["undefined_method", "undefined_method"], "{names:?}");
    assert!(
        r.diagnostics[0].message.contains("for the type 'M'"),
        "{:?}",
        r.diagnostics[0].message
    );
    assert_eq!(
        r.diagnostics[1].message,
        "The method 'Bar' isn't defined for the type 'int'."
    );
}

/// The class-region baseline: `class C { m() { Foo(); } }` still names
/// `C` on the invoke miss — the `this_type` arm's pre-existing face,
/// unchanged by the mixin join.
#[test]
fn class_invoke_miss_still_names_the_class() {
    let r = report("class C {\n  void m() { Foo(); }\n}");
    let names = code_names(&r);
    assert_eq!(names, ["undefined_method"], "{names:?}");
    assert!(
        r.diagnostics[0].message.contains("for the type 'C'"),
        "{:?}",
        r.diagnostics[0].message
    );
}

/// A bare WRITE inside a decidable `on`-face scope reports through the
/// write arm like the oracle — `mixin M on C`'s member scope was final,
/// so `gone = 1` is `undefined_identifier`, never a swallowed
/// constraint-member write.
#[test]
fn mixin_on_indexed_write_miss_reports() {
    let r = report(
        "class int {}\n\
             class C {\n  int cf = 0;\n}\n\
             mixin M on C {\n  void m() { gone = 1; }\n}",
    );
    assert_eq!(
        code_names(&r),
        ["undefined_identifier"],
        "{:?}",
        code_names(&r)
    );
}

/// The constraint setter side: a write to a `C` member the `on` face
/// carries resolves the write too — `cf = 1` inside `mixin M on C` is
/// the field's own assignment, silent like the oracle.
#[test]
fn mixin_on_indexed_write_hit_stays_silent() {
    let r = report(
        "class int {}\n\
             class C {\n  int cf = 0;\n}\n\
             mixin M on C {\n  void m() { cf = 1; }\n}",
    );
    assert!(r.diagnostics.is_empty(), "{:?}", code_names(&r));
}

// —— #2298 T6: constructor/declaration faces ————————————————————————

/// An explicit `super(...)` initializer satisfies the
/// implicit-super face regardless of its argument shapes: the bare
/// positional form (the corpus archive row,
/// `PcSHA1Digest() : super(Endian.big, 5, 80);`), the named-selector
/// form (the corpus html row, `: super(name, selfClosing);`), and the
/// mixed-initializer-list form (the corpus intl row,
/// `: _fullPattern = pattern, super(_patchQuotes(pattern), parent);`)
/// all fold to a `SuperConstructorInvocation` — the early return the
/// official `_checkForUndefinedConstructorInInitializerImplicit` takes
/// (upstream Dart SDK source, error_verifier.dart).
#[test]
fn explicit_super_invocation_satisfies_implicit_super_face() {
    // Bare positional `super(...)`.
    let r = report(
        "class int {} class S { S(int a, int b, int c) {} }\n\
             class D extends S { D() : super(1, 2, 3); }",
    );
    assert!(
        !code_names(&r).contains(&"implicit_super_initializer_missing_arguments"),
        "{:?}",
        code_names(&r)
    );
    // Named selector `super.n(...)`.
    let r = report(
        "class int {} class S { S.n(int a) {} }\n\
             class D extends S { D() : super.n(1); }",
    );
    assert!(
        !code_names(&r).contains(&"implicit_super_initializer_missing_arguments"),
        "{:?}",
        code_names(&r)
    );
    // Mixed list: a field initializer, then the super invocation.
    let r = report(
        "class int {} class S { S(int a) {} }\n\
             class D extends S {\n  int f;\n  D() : f = 0, super(1);\n}",
    );
    assert!(
        !code_names(&r).contains(&"implicit_super_initializer_missing_arguments"),
        "{:?}",
        code_names(&r)
    );
    // The control: no super invocation at all — the face fires at the
    // constructor head.
    let r = report(
        "class int {} class S { S(int a) {} }\n\
             class D extends S { D() {} }",
    );
    assert_eq!(
        code_names(&r),
        ["implicit_super_initializer_missing_arguments"],
        "{:?}",
        code_names(&r)
    );
}

/// An extension type's representation field never reports
/// `final_not_initialized`: upstream's representation constructor is
/// generative (`element.constructors.any(isGenerative)`,
/// error_verifier.dart:7114), so the `ConstructorFieldsVerifier` owns
/// the container and the primary's formal binds the rep — even when
/// the only other constructor is a factory (the corpus drift row,
/// `extension type _SerializedRequest._(JSObject inner)` beside
/// `external factory _SerializedRequest(...)`).
#[test]
fn extension_type_rep_field_never_reports_uninitialized() {
    // Named rep primary + an unnamed external factory: the factory
    // registers no generative site, but the rep is bound regardless.
    let r = report("class int {} extension type E._(int i) { external factory E(); }");
    assert!(
        !code_names(&r).contains(&"final_not_initialized"),
        "{:?}",
        code_names(&r)
    );
    // A named-rep primary with no other declaration at all.
    let r = report("class int {} extension type F._(int i) {}");
    assert!(r.diagnostics.is_empty(), "{:?}", code_names(&r));
    // A redirecting generative secondary keeps the declaration face
    // off (the redirect target's primary binds the rep).
    let r = report("class int {} extension type G._(int i) { G.other(int v) : this._(v); }");
    assert!(
        !code_names(&r).contains(&"final_not_initialized"),
        "{:?}",
        code_names(&r)
    );
}

/// A bare miss in a creation's ARGUMENTS is an ordinary expression:
/// the oracle names it `undefined_identifier` (the corpus intl row,
/// `List<String>.from(dateTimeSymbols.keys)` at 525:25, and the
/// analyzer row, `List<TypeImpl>.filled(_, DynamicTypeImpl.instance)`
/// at 510:53). Only a `new`/`const` creation HEAD is a type position —
/// `creation_with_non_type` at its name (upstream Dart SDK source,
/// error_verifier.dart's `visitInstanceCreationExpression`).
#[test]
fn new_const_creation_argument_miss_is_undefined_identifier() {
    // Resolved head, missing argument: the T6 corpus shape. The const
    // creation's argument face rides beside the undefined name (the
    // oracle's row set on `const C.n(missX); new C.n(missY);`).
    let r = report(
        "class int {} class C { const C.n(int x); }\n\
             void f() { const C.n(missX); new C.n(missY); }",
    );
    assert_eq!(
        code_names(&r),
        [
            "const_with_non_constant_argument",
            "undefined_identifier",
            "undefined_identifier",
        ],
        "{:?}",
        code_names(&r)
    );
    // Missing head stays `creation_with_non_type`; a nested creation's
    // head reads the same way, its own argument stays ordinary.
    let r = report(
        "class int {} class H { H(int x); }\n\
             void g() { new MissZ(new H(missA)); }",
    );
    let names = code_names(&r);
    assert!(names.contains(&"creation_with_non_type"), "{names:?}");
    assert!(!names.contains(&"undefined_class"), "{names:?}");
}

/// A file-local function-type alias declared AFTER the use's enclosing
/// declaration still types the tear-off argument check: the streaming
/// bounds capture has not registered the typedef when the check reads
/// it, so the callable verdict falls back to the element model's
/// declaration-ordered armed type (the corpus html rows,
/// `_data._skipUntil(_isSpaceOrAngleBracket)` at 226:32 and
/// `data._skipUntil(isWhitespace)` at 359:27 over the file-trailing
/// `typedef _CharPredicate = bool Function(String char);`).
#[test]
fn typedef_after_use_still_types_tear_off_argument() {
    // The corpus shape: a top-level function tear-off into a
    // cross-class formal typed by the trailing alias.
    let r = report(
        "class int {} class bool {} class String {} class Data {\n\
             \x20 String? skipUntil(Pred until) => null;\n\
             }\n\
             class Parser {\n\
             \x20 final Data data = Data();\n\
             \x20 void step() { data.skipUntil(isSpace); }\n\
             }\n\
             bool isSpace(String char) => char == ' ';\n\
             typedef Pred = bool Function(String char);",
    );
    assert!(r.diagnostics.is_empty(), "{:?}", code_names(&r));
    // The genuine mismatch still reports against the trailing alias:
    // an arity-wider tear-off refuses at the argument.
    let source = "class int {} class bool {} class String {} class Data {\n\
             \x20 String? skipUntil(Pred until) => null;\n\
             }\n\
             class Parser {\n\
             \x20 final Data data = Data();\n\
             \x20 void step() { data.skipUntil(wider); }\n\
             }\n\
             bool wider(String a, String b) => a == b;\n\
             typedef Pred = bool Function(String char);";
    let r = report(source);
    let rows: Vec<(&str, u32)> = r
        .diagnostics
        .iter()
        .map(|d| (d.code.name, d.range.offset))
        .collect();
    let at = u32::try_from(
        source
            .find("wider);")
            .unwrap_or_else(|| panic!("{source:?}")),
    )
    .expect("offset fits u32");
    assert_eq!(rows, [("argument_type_not_assignable", at)], "{rows:?}");
}
