//! The const NON_CONSTANT-family differential pins (#2242, wave W4): the
//! remaining oracle-only rows burned down through two root patterns, each
//! probed against the fresh census (`binary 298957be`, oracle `dart
//! analyze`).
//!
//! - An UNRESOLVED callee inside a const literal (`ClassSpec(...)` over a
//!   dead import): the oracle's evaluation of the call is known-invalid at
//!   the whole invocation, and `undefined_function` is not an
//!   override-listed diagnostic (`constant_verifier.dart` `_reportError`),
//!   so the site default codes the row — `non_constant_list_element` /
//!   `non_constant_map_value` at the invocation span, and the enclosing
//!   declaration's `const_initialized_with_non_constant_value` at the
//!   FIRST failure only (the oracle evaluates the initializer once; the
//!   nested literal faces are the per-node `_ConstLiteralVerifier`).
//! - A QUALIFIED const creation (`Precedence._(NO_PRECEDENCE)` — the
//!   analyzer corpus's `precedence.dart`): the creation resolves against
//!   the in-file class's const constructor, so each argument validates
//!   against `const_with_non_constant_argument`; an UNRESOLVED argument
//!   leaves the declaration's own verdict silent (the undefined name is
//!   the reported root cause, the #2138 rule the bare-creation arm
//!   already implements), while a known non-constant argument pairs the
//!   declaration row at the argument (the pinned probe pairing).
//!
//! Out of that lane's granted files (#2248 follow-up): the
//! annotation-argument const context (`@Target({TargetKind.classType})` —
//! the drift corpus rows) now opens its capture window in the listener's
//! `begin_metadata`/`end_metadata` — pattern 3 below. Still out of the
//! model: the ANNOTATION-HEAD faces (a resolved annotation head validates
//! its arguments against the resolved const constructor —
//! `const_with_non_constant_argument` and the arity faces); those live in
//! the annotation-resolution machinery, not the capture window.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the sibling ticket rig mirrored verbatim.
fn report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let (scanned, features, set) = scan(source, file);
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
        StrictModes::default(),
        &LibraryClassModifiers::default(),
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

fn scan(source: &str, file: FileId) -> (dartq_scanner::ScanResult, ParserFeatures, FeatureSet) {
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
    (scanned, features, set)
}

/// `(code, source text at the row)` pairs — the span assertions key on the
/// written text so the anchors stay readable.
fn rows<'a>(report: &FileReport, source: &'a str) -> Vec<(&'static str, &'a str)> {
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

fn count(rows: &[(&'static str, &str)], code: &str) -> usize {
    rows.iter().filter(|(c, _)| *c == code).count()
}

// —— pattern 1: unresolved callee inside a const literal ————————

/// `const l = [ClassSpec()];` over an undefined `ClassSpec` (the
/// `mock_sdk_elements.dart` shape): the element face reports at the whole
/// invocation, the declaration's verdict anchors at the same span, and no
/// method-invocation fault substitutes (the callee names nothing — the
/// oracle's `undefined_function` is the root cause row).
#[test]
fn unresolved_callee_in_const_list_reports_element_face() {
    let source = "const l = [ClassSpec()];";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert!(
        rows.contains(&("undefined_function", "ClassSpec")),
        "{rows:?}"
    );
    assert!(
        rows.contains(&("non_constant_list_element", "ClassSpec()")),
        "{rows:?}"
    );
    assert!(
        rows.contains(&(
            "const_initialized_with_non_constant_value",
            "ClassSpec()"
        )),
        "{rows:?}"
    );
    assert_eq!(count(&rows, "const_initialized_with_non_constant_value"), 1);
    assert_eq!(count(&rows, "const_eval_method_invocation"), 0, "{rows:?}");
    assert_eq!(count(&rows, "const_with_non_const"), 0, "{rows:?}");
}

/// `const m = {'k': LibrarySpec()};` — the map entry names the failing
/// side (`non_constant_map_value` at the value), one declaration row.
#[test]
fn unresolved_callee_in_const_map_value_reports_value_face() {
    let source = "const m = {'k': LibrarySpec()};";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert!(
        rows.contains(&("non_constant_map_value", "LibrarySpec()")),
        "{rows:?}"
    );
    assert!(
        rows.contains(&(
            "const_initialized_with_non_constant_value",
            "LibrarySpec()"
        )),
        "{rows:?}"
    );
    assert_eq!(count(&rows, "non_constant_map_key"), 0, "{rows:?}");
}

/// The nested shape (`LibrarySpec(classes: [ClassSpec()])`): every
/// literal node in the const context verifies per element — the map
/// value's row at the outer invocation, the inner list's element row at
/// the inner one — and the declaration's verdict anchors at the FIRST
/// failure (the outer value), once.
#[test]
fn nested_const_literal_elements_report_per_literal() {
    let source = "const m = {'k': LibrarySpec(classes: [ClassSpec()])};";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert!(
        rows.contains(&("non_constant_map_value", "LibrarySpec(classes: [ClassSpec()])")),
        "{rows:?}"
    );
    assert!(
        rows.contains(&("non_constant_list_element", "ClassSpec()")),
        "{rows:?}"
    );
    assert_eq!(
        count(&rows, "const_initialized_with_non_constant_value"),
        1,
        "{rows:?}"
    );
}

/// An opaque leaf that is NOT an unresolved call stays a valid constant
/// of unknown value: `as` and index deferrals keep the literal silent
/// (the probe keys on the bare-miss reference row at the span's head).
#[test]
fn opaque_deferrals_stay_constant() {
    for source in ["const a = [1 as int];", "const a = ['ab'[0]];"] {
        let rep = report(source);
        let rows = rows(&rep, source);
        assert_eq!(count(&rows, "non_constant_list_element"), 0, "{rows:?}");
        assert_eq!(
            count(&rows, "const_initialized_with_non_constant_value"),
            0,
            "{rows:?}"
        );
    }
}

// —— pattern 2: qualified const creation argument validation ——————

/// `static const none = Precedence._(NO_PRECEDENCE);` over an in-file
/// const constructor (the `precedence.dart` shape): the creation
/// resolves, so the argument validates — `const_with_non_constant_argument`
/// at the argument beside its own `undefined_identifier` — and the
/// declaration's verdict stays silent (the unresolved argument is the
/// #2138 rule: the oracle's evaluation is unknown, not known-invalid).
#[test]
fn qualified_const_creation_checks_unresolved_argument() {
    let source = "class Precedence {\n  static const none = Precedence._(NO_PRECEDENCE);\n  final int _index;\n  const Precedence._(this._index);\n}";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert!(
        rows.contains(&("undefined_identifier", "NO_PRECEDENCE")),
        "{rows:?}"
    );
    assert!(
        rows.contains(&("const_with_non_constant_argument", "NO_PRECEDENCE")),
        "{rows:?}"
    );
    assert_eq!(
        count(&rows, "const_with_non_constant_argument"),
        1,
        "{rows:?}"
    );
    assert_eq!(
        count(&rows, "const_initialized_with_non_constant_value"),
        0,
        "{rows:?}"
    );
    assert_eq!(count(&rows, "const_with_non_const"), 0, "{rows:?}");
}

/// A known non-constant argument pairs the declaration row at the
/// argument (the pinned bare-creation pairing — `const C(l)` over a
/// non-const top-level `l`), and the creation's own face stays silent.
#[test]
fn qualified_const_creation_pairs_known_bad_argument() {
    let source = "int g = 1;\nclass P {\n  static const p = P._(g);\n  final int i;\n  const P._(this.i);\n}";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert!(rows.contains(&("const_with_non_constant_argument", "g")), "{rows:?}");
    assert!(
        rows.contains(&("const_initialized_with_non_constant_value", "g")),
        "{rows:?}"
    );
    assert_eq!(count(&rows, "const_with_non_const"), 0, "{rows:?}");
}

/// A valid qualified creation stays fully silent (`const x = C.named(1)`
/// over a declared const named constructor): the argument check passes,
/// the creation folds to its CONTAINER's value (the selector callee the
/// close reports must not become the instance's type name), and no
/// deferred-creation row stages.
#[test]
fn valid_qualified_const_creation_stays_silent() {
    let source = "class C {\n  final int i;\n  const C.named(this.i);\n}\nconst x = C.named(1);";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert_eq!(count(&rows, "const_with_non_constant_argument"), 0, "{rows:?}");
    assert_eq!(
        count(&rows, "const_initialized_with_non_constant_value"),
        0,
        "{rows:?}"
    );
    assert_eq!(count(&rows, "const_with_non_const"), 0, "{rows:?}");
    assert_eq!(count(&rows, "const_with_undefined_constructor"), 0, "{rows:?}");
}

/// A qualified creation whose selector answers no constructor keeps the
/// unknown-callee face (the creation row at the whole invocation, no
/// per-argument rows — the oracle's
/// `const_with_undefined_constructor` world, coded by the staging).
#[test]
fn qualified_creation_with_unknown_selector_defers() {
    let source = "class C {\n  final int i;\n  const C.named(this.i);\n}\nconst x = C.other(1);";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert_eq!(count(&rows, "const_with_non_constant_argument"), 0, "{rows:?}");
    assert!(
        rows.contains(&("const_initialized_with_non_constant_value", "C.other(1)")),
        "{rows:?}"
    );
}

/// The unresolved-callee ROOT (`const y = Gone();` — the probe row the
/// oracle pairs with `undefined_function`): exactly one declaration row
/// at the invocation, via the verdict (the per-call staging drops — the
/// oracle codes one row per initializer).
#[test]
fn unresolved_callee_root_reports_single_declaration_row() {
    let source = "const y = Gone();";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert!(rows.contains(&("undefined_function", "Gone")), "{rows:?}");
    assert_eq!(
        count(&rows, "const_initialized_with_non_constant_value"),
        1,
        "{rows:?}"
    );
    assert_eq!(count(&rows, "const_with_non_const"), 0, "{rows:?}");
}

// —— pattern 3: annotation-argument const context (#2248) ——————————

/// `@Target({TargetKind.classType})` over an unresolved `TargetKind` (the
/// drift corpus's `dsl/table.dart` rows): the annotation's argument list
/// is a const context, so the set literal's element face reports at the
/// unresolved prefix (`TargetKind` — the verdict's span, not the whole
/// `TargetKind.classType` access) beside its own `undefined_identifier`,
/// and no declaration-level verdict substitutes (the annotation
/// evaluation has no initializer-style site default).
#[test]
fn annotation_set_element_reports_at_unresolved_prefix() {
    let source = "@Target({TargetKind.classType})\nclass C {}";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert!(rows.contains(&("undefined_annotation", "@Target")), "{rows:?}");
    assert!(
        rows.contains(&("undefined_identifier", "TargetKind")),
        "{rows:?}"
    );
    assert!(
        rows.contains(&("non_constant_set_element", "TargetKind")),
        "{rows:?}"
    );
    assert_eq!(
        count(&rows, "non_constant_set_element"),
        1,
        "{rows:?}"
    );
    assert_eq!(
        count(&rows, "const_initialized_with_non_constant_value"),
        0,
        "{rows:?}"
    );
}

/// An unresolved callee inside an annotation's set literal: the element
/// face reports at the whole invocation (the W4 pattern-1 pairing — the
/// oracle's evaluation is known-invalid there), on class, enum-constant,
/// and parameter annotations alike. The callee's own face is the plain
/// `undefined_method` (dartq resolves the argument position as an
/// ordinary invocation; the oracle's const-creation reading pairs
/// `undefined_function` — a head-context face outside this window).
#[test]
fn unresolved_callee_in_annotation_set_reports_element_face() {
    for source in [
        "class T { const T(); }\n@T({Missing()})\nclass C {}",
        "class T { const T(); }\nenum E {\n  @T({Missing()}) a, b,\n}",
        "class T { const T(); }\nvoid h(@T({Missing()}) int p) {}",
    ] {
        let rep = report(source);
        let rows = rows(&rep, source);
        assert!(
            rows.contains(&("non_constant_set_element", "Missing()")),
            "{rows:?}"
        );
        assert_eq!(
            count(&rows, "const_initialized_with_non_constant_value"),
            0,
            "{rows:?}"
        );
        assert_eq!(
            count(&rows, "const_with_non_constant_argument"),
            0,
            "{rows:?}"
        );
    }
}

/// A known function invocation inside an annotation's literal: the
/// element's fault IS the row (`const_eval_method_invocation` — probe
/// paired) and the element-face code stays silent. The list element
/// anchors at the invocation; the map value's row rides the shared
/// call-span heuristic (the `'k'` string key two bytes before the callee
/// reads as a receiver — the oracle anchors at `g()`, a span-only
/// divergence outside the (severity, code, file) gate).
#[test]
fn fault_element_in_annotation_literal_beats_element_face() {
    let list = "class T { const T(); }\nvoid g() {}\n@T([g()])\nclass C {}";
    let rep = report(list);
    let got = rows(&rep, list);
    assert!(
        got.contains(&("const_eval_method_invocation", "g()")),
        "{got:?}"
    );
    assert_eq!(count(&got, "non_constant_list_element"), 0, "{got:?}");

    let map = "class T { const T(); }\nvoid g() {}\n@T({'k': g()})\nclass C {}";
    let rep = report(map);
    let got = rows(&rep, map);
    assert!(
        got.contains(&("const_eval_method_invocation", "'k': g()")),
        "{got:?}"
    );
    assert_eq!(count(&got, "non_constant_map_value"), 0, "{got:?}");
    assert_eq!(count(&got, "non_constant_map_key"), 0, "{got:?}");
}

/// A constant annotation literal stays fully silent — including on enum
/// constants, where the annotation window nests over the previous
/// element's still-open window (the capture discards back to it).
#[test]
fn constant_annotation_literal_stays_silent() {
    for source in [
        "class T { const T(); }\n@T({1, 2})\nclass C {}",
        "class T { const T(); }\nenum E {\n  @T({1}) a, @T({2}) b,\n}",
    ] {
        let rep = report(source);
        let rows = rows(&rep, source);
        assert_eq!(count(&rows, "non_constant_set_element"), 0, "{rows:?}");
        assert_eq!(
            count(&rows, "const_initialized_with_non_constant_value"),
            0,
            "{rows:?}"
        );
        assert_eq!(count(&rows, "const_eval_method_invocation"), 0, "{rows:?}");
    }
}
