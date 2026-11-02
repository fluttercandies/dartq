//! The #3559 pre-3.0 (legacy) switch-verifier pins.
//!
//! Seven diagnostics fire only when the file's language version leaves
//! `Feature.patterns` disabled; upstream gates every emitter on
//! `!featureSet.isEnabled(Feature.patterns)` (the statement faces —
//! `constant_verifier.dart:1145-1182`,
//! `error_verifier.dart:5870-5937`,
//! `shared_type_analyzer.dart:39-53` — plus the flow-structural
//! `switchCaseCompletesNormally` arm). The pins below pair each
//! implementable face's firing probe at `// @dart=2.19` with the same
//! source silent at `// @dart=3.13` — the version gate is the point.
//!
//! Oracle rows cited verbatim from `dart analyze --format=machine`
//! (3.13.7) over each pin's source at `@dart=2.19`.
//!
//! Faces this seam documents but does not emit: the deferred-import row
//! (`nonConstantCaseExpressionFromDeferredLibrary` — the const-eval layer
//! carries no deferred-provenance face), the flow-structural
//! `switch_case_completes_normally` (a member-body "completes normally"
//! verdict lives in `dartq-flow`, not this parse-time pass), and
//! `break_label_on_switch_member` (a label-binding row that is NOT
//! version-gated upstream — it fires at 3.13 identically — so it belongs
//! to the label resolver's own emit site, not this pass).

use super::test_rig::report_ops_bare;
use crate::collector::collect;
use crate::diagnose::{FileReport, StrictModes, fold_unit_with_ops, merge_source_ordered};
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    ClassModifiers, Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder, Namespaces,
};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The (offset, length) rows of `code` in `report`, in emit order.
fn code_rows<'a>(report: &'a FileReport, code: &str) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

fn all_names(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

// —— `missing_enum_constant_in_switch` ————————————————————————————————————

/// A bare-version fixture: `report_ops_bare` reads the `// @dart=` marker
/// only when it leads the file (no prelude), so each fixture spells its own
/// types.
const ENUM_FIXTURE: &str = "enum E { a, b }\nvoid f(E e) {\n  switch (e) {\n    case E.a:\n      break;\n  }\n}\n";
const NULL_ENUM_FIXTURE: &str =
    "enum E { a, b }\nvoid f(E? e) {\n  switch (e) {\n    case E.a:\n      break;\n    case E.b:\n      break;\n  }\n}\n";
const EQUALS_FIXTURE: &str = "class Object {}\nclass C {\n  const C();\n  bool operator ==(Object o) => true;\n}\nconst c = C();\nvoid f(Object o) {\n  switch (o) {\n    case c:\n      break;\n  }\n}\n";
const SUBTYPE_FIXTURE: &str =
    "class int {}\nclass String {}\nvoid f(int i) {\n  switch (i) {\n    case 'x':\n      break;\n  }\n}\n";

/// `case E.a` over `enum E {a, b}` with no `default`: the uncovered `b`
/// reports at the `switch (e)` span. Oracle:
/// `WARNING|STATIC_WARNING|MISSING_ENUM_CONSTANT_IN_SWITCH|…|4|3|10|Missing
/// case clause for 'b'.` — the `10`-wide span covers `switch (e)`.
#[test]
fn missing_enum_constant_fires_at_2_19() {
    let report = report_ops_bare(&format!("// @dart=2.19\n{ENUM_FIXTURE}"));
    let rows = code_rows(&report, "missing_enum_constant_in_switch");
    assert_eq!(
        rows.len(),
        1,
        "one uncovered-constant row expected: {:?}",
        all_names(&report)
    );
    // The oracle's `switch (e)` head span starts at the `switch` keyword
    // (col 3 of line 4 upstream; the keyword offset here — upstream's
    // `statement.offset` equals `switch_kw.offset`).
    let fixture = format!("// @dart=2.19\n{ENUM_FIXTURE}");
    assert_eq!(rows[0].0, fixture.find("switch").unwrap() as u32);
}

/// The same source at 3.13 is patterns-era: `SwitchCase` never exists, the
/// legacy coverage walk never runs — the enum gap lands on the patterns
/// exhaustiveness faces instead (silent to this code).
#[test]
fn missing_enum_constant_silent_at_3_13() {
    let report = report_ops_bare(&format!("// @dart=3.13\n{ENUM_FIXTURE}"));
    assert!(
        code_rows(&report, "missing_enum_constant_in_switch").is_empty(),
        "the pre-3.0 coverage walk must not run under patterns: {:?}",
        all_names(&report)
    );
}

/// The nullable tail: `E?` covers both constants but no `case null` —
/// oracle emits `Missing case clause for 'null'` with the same span.
/// `WARNING|STATIC_WARNING|MISSING_ENUM_CONSTANT_IN_SWITCH|…|4|3|10|Missing
/// case clause for 'null'.`
#[test]
fn missing_enum_null_constant_fires_at_2_19() {
    let report = report_ops_bare(&format!("// @dart=2.19\n{NULL_ENUM_FIXTURE}"));
    let rows = code_rows(&report, "missing_enum_constant_in_switch");
    assert_eq!(rows.len(), 1, "one 'null' row expected: {:?}", all_names(&report));
}

/// Same nullable source at 3.13: silent to the legacy code.
#[test]
fn missing_enum_null_constant_silent_at_3_13() {
    let report = report_ops_bare(&format!("// @dart=3.13\n{NULL_ENUM_FIXTURE}"));
    assert!(
        code_rows(&report, "missing_enum_constant_in_switch").is_empty(),
        "the pre-3.0 coverage walk must not run under patterns: {:?}",
        all_names(&report)
    );
}

// —— `case_expression_type_implements_equals` ————————————————————————————

/// A const `C()` instance whose `==` is `C`'s own (not `Object`'s): the
/// `hasPrimitiveEquality` fail arm. Oracle:
/// `ERROR|COMPILE_TIME_ERROR|CASE_EXPRESSION_TYPE_IMPLEMENTS_EQUALS|…|9|10|1|
/// The switch case expression type 'C' can't override the '==' operator.`
/// — anchored at `c`'s one-char span.
#[test]
fn case_expression_implements_equals_fires_at_2_19() {
    let report = report_ops_bare(&format!("// @dart=2.19\n{EQUALS_FIXTURE}"));
    assert_eq!(
        code_rows(&report, "case_expression_type_implements_equals").len(),
        1,
        "the non-primitive-equality head reports: {:?}",
        all_names(&report)
    );
}

/// The patterns-era face of the same source: the const is a constant
/// *pattern* now and the legacy `validateExpression` arm never runs.
#[test]
fn case_expression_implements_equals_silent_at_3_13() {
    let report = report_ops_bare(&format!("// @dart=3.13\n{EQUALS_FIXTURE}"));
    assert!(
        code_rows(&report, "case_expression_type_implements_equals").is_empty(),
        "the pre-3.0 primitive-equality gate must not run under patterns: {:?}",
        all_names(&report)
    );
}

// —— `case_expression_type_is_not_switch_expression_subtype` ————————————

/// `case 'x'` over an `int` scrutinee: `String` is no subtype of `int`.
/// Oracle: `ERROR|COMPILE_TIME_ERROR|CASE_EXPRESSION_TYPE_IS_NOT_SWITCH_
/// EXPRESSION_SUBTYPE|…|4|10|3|The switch case expression type 'String'
/// must be a subtype of the switch expression type 'int'.` — anchored at
/// `'x'`'s 3-char span.
#[test]
fn case_expression_type_mismatch_fires_at_2_19() {
    let report = report_ops_bare(&format!("// @dart=2.19\n{SUBTYPE_FIXTURE}"));
    assert_eq!(
        code_rows(&report, "case_expression_type_is_not_switch_expression_subtype").len(),
        1,
        "the type-mismatch head reports: {:?}",
        all_names(&report)
    );
}

/// The same source at 3.13: the constant pattern's coverage is the
/// never-match face instead — this row never exists.
#[test]
fn case_expression_type_mismatch_silent_at_3_13() {
    let report = report_ops_bare(&format!("// @dart=3.13\n{SUBTYPE_FIXTURE}"));
    assert!(
        code_rows(&report, "case_expression_type_is_not_switch_expression_subtype").is_empty(),
        "the pre-3.0 type-mismatch gate must not run under patterns: {:?}",
        all_names(&report)
    );
}

// —— deferred/not-implemented faces: silence pins ————————————————————————

/// A deferred-import rig: `main` at `// @dart=2.19` imports `def.dart`
/// deferred as `d`; `def.dart` declares `const c = 1`. The shared rig
/// cannot express the edge (`report_ops_with` hardcodes `deferred:
/// false`), so this fixture builds the two-library graph by hand — the
/// `deferred: true` flag is the only difference.
fn deferred_report() -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let mut builder = LibraryGraphBuilder::new(&strings);
    let core_lib = builder.intern("dart:core");
    let core_file = files.intern(Path::new("<dart:core>"));
    for name in [
        "Object", "int", "bool", "num", "String", "Null", "Enum",
    ] {
        builder.declare(core_lib, strings.intern(name), core_file);
    }
    let scan = |source: &str, file: dartq_source::FileId| {
        let scanned = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(scanned.diagnostics.is_empty(), "{source:?}");
        scanned
    };
    let main = "// @dart=2.19\nimport 'def.dart' deferred as d;\nvoid f(int i) {\n  switch (i) {\n    case d.c:\n      break;\n  }\n}\n";
    let dep = "const c = 1;\n";
    let features =
        ParserFeatures::from_feature_set(FeatureSet::for_language_version(
            LanguageVersion::new(2, 19),
        ));
    let mut index = ElementIndex::default();
    let mut units = Vec::new();
    for (uri, source) in [("package:t/main.dart", main), ("package:t/def.dart", dep)] {
        let file = files.intern(Path::new(uri));
        let scanned = scan(source, file);
        let elements = collect(source, &scanned.tokens, features, &strings);
        let lib_id = builder.intern(uri);
        builder.unit(lib_id, file);
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib_id, name, file);
        }
        builder.import(
            lib_id,
            ImportEdge {
                target: Some(core_lib),
                prefix: None,
                deferred: false,
                non_library: false,
                combinators: Combinators::unfiltered(),
            },
        );
        index.insert(file, elements);
        units.push((lib_id, file, scanned, source));
    }
    // `main` imports `def` deferred under prefix `d`.
    builder.import(
        units[0].0,
        ImportEdge {
            target: Some(units[1].0),
            prefix: Some(strings.intern("d")),
            deferred: true,
            non_library: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let (lib_id, file, scanned, source) = &units[0];
    let fold = fold_unit_with_ops(
        source,
        &scanned.tokens,
        features,
        &strings,
        *file,
        *lib_id,
        &graph,
        &namespaces,
        &index,
        StrictModes::default(),
        &LibraryClassModifiers::default(),
        true,
        None,
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

/// `case d.c` through a deferred prefix: oracle emits
/// `ERROR|COMPILE_TIME_ERROR|NON_CONSTANT_CASE_EXPRESSION_FROM_DEFERRED_
/// LIBRARY|…|5|12|1|Constant values from a deferred library can't be used
/// as a case expression.` — anchored at `d` (the prefix's 1-char span,
/// `errorTarget` is the identifier).
#[test]
fn deferred_library_head_fires_at_2_19() {
    let report = deferred_report();
    let rows = code_rows(&report, "non_constant_case_expression_from_deferred_library");
    assert_eq!(
        rows.len(),
        1,
        "the deferred-prefix head reports its own row: {:?}",
        all_names(&report)
    );
    // Oracle anchors `d` — the prefix token's own 1-char span.
    let source = "// @dart=2.19\nimport 'def.dart' deferred as d;\nvoid f(int i) {\n  switch (i) {\n    case d.c:\n      break;\n  }\n}\n";
    let at = source.find("case d").unwrap() + "case ".len();
    assert_eq!(rows[0], (at as u32, 1));
}

/// `switch_case_completes_normally` needs the flow layer's "member body
/// completes normally" verdict — a parse-time pass cannot answer it. The
/// fall-through pair reports nothing here (upstream reports
/// `SWITCH_CASE_COMPLETES_NORMALLY` at the first `case`'s keyword).
#[test]
fn completes_normally_is_flow_owned() {
    let report = report_ops_bare(concat!(
        "// @dart=2.19\n",
        "void f(int i) {\n",
        "  switch (i) {\n",
        "    case 1:\n",
        "      print(1);\n",
        "    case 2:\n",
        "      print(2);\n",
        "      break;\n",
        "  }\n",
        "}\n",
    ));
    assert!(
        code_rows(&report, "switch_case_completes_normally").is_empty(),
        "the completes-normally verdict lives in the flow layer: {:?}",
        all_names(&report)
    );
}

/// `break_label_on_switch_member` is not version-gated at all — the
/// oracle emits it at 3.13 identically to 2.19 — so it is the label
/// resolver's own row, not this pass's. The pin documents that the
/// pre-3.0 verifier does not emit it.
#[test]
fn break_label_on_switch_member_is_label_owned() {
    let report = report_ops_bare(concat!(
        "// @dart=2.19\n",
        "void f(int i) {\n",
        "  switch (i) {\n",
        "    L: case 1:\n",
        "      break L;\n",
        "  }\n",
        "}\n",
    ));
    assert!(
        code_rows(&report, "break_label_on_switch_member").is_empty(),
        "the label-on-member row belongs to the label resolver: {:?}",
        all_names(&report)
    );
}
