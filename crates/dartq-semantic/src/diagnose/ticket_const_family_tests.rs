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
//! `begin_metadata`/`end_metadata` — pattern 3 below. Pattern 4 (#2261)
//! closes the window lane's deferred heads, each probe-paired: the
//! ANNOTATION-HEAD faces (a resolved annotation head validates its
//! arguments against the resolved const constructor —
//! `const_with_non_constant_argument` with the label-resolution split to
//! `invalid_constant`, plus the argument-count faces), the rider
//! `invalid_constant` at an unresolved leaf inside a resolved creation's
//! arguments (the corpus's `ast.dart` `GenerateNodeProperty('element',
//! type: ExtensionElementImpl)` row), and the ERROR-tier duplicate twins
//! (`equal_keys_in_const_map` / `equal_elements_in_const_set`) over const
//! literals.

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
        rows.contains(&("const_initialized_with_non_constant_value", "ClassSpec()")),
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
        rows.contains(&("const_initialized_with_non_constant_value", "LibrarySpec()")),
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
        rows.contains(&(
            "non_constant_map_value",
            "LibrarySpec(classes: [ClassSpec()])"
        )),
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
    assert!(
        rows.contains(&("const_with_non_constant_argument", "g")),
        "{rows:?}"
    );
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
    assert_eq!(
        count(&rows, "const_with_non_constant_argument"),
        0,
        "{rows:?}"
    );
    assert_eq!(
        count(&rows, "const_initialized_with_non_constant_value"),
        0,
        "{rows:?}"
    );
    assert_eq!(count(&rows, "const_with_non_const"), 0, "{rows:?}");
    assert_eq!(
        count(&rows, "const_with_undefined_constructor"),
        0,
        "{rows:?}"
    );
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
    assert_eq!(
        count(&rows, "const_with_non_constant_argument"),
        0,
        "{rows:?}"
    );
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
    assert!(
        rows.contains(&("undefined_annotation", "@Target")),
        "{rows:?}"
    );
    assert!(
        rows.contains(&("undefined_identifier", "TargetKind")),
        "{rows:?}"
    );
    assert!(
        rows.contains(&("non_constant_set_element", "TargetKind")),
        "{rows:?}"
    );
    assert_eq!(count(&rows, "non_constant_set_element"), 1, "{rows:?}");
    assert_eq!(
        count(&rows, "const_initialized_with_non_constant_value"),
        0,
        "{rows:?}"
    );
}

/// An unresolved callee inside an annotation's set literal: the argument
/// pairing (`const_with_non_constant_argument` at the whole invocation)
/// pairs on every host beside the callee's own miss face, and the
/// call-shaped element's own face follows the probed host split — a
/// type-declaration host's evaluation owns the report (no element face on
/// `class`), an enum-constant or parameter host keeps it (probed
/// 3.47.1). The declaration-level verdict never substitutes.
#[test]
fn unresolved_callee_in_annotation_set_reports_element_face() {
    let class_source = "class T { const T(); }\n@T({Missing()})\nclass C {}";
    let rep = report(class_source);
    let got = rows(&rep, class_source);
    assert!(
        got.contains(&("const_with_non_constant_argument", "Missing()")),
        "{got:?}"
    );
    assert!(got.contains(&("undefined_function", "Missing")), "{got:?}");
    assert_eq!(count(&got, "non_constant_set_element"), 0, "{got:?}");
    assert_eq!(
        count(&got, "const_initialized_with_non_constant_value"),
        0,
        "{got:?}"
    );

    // The enum-constant and parameter hosts keep the element face. The
    // enum host's callee face reads `undefined_method` (the row's
    // instance-scope this-type snapshot names the enum) where the oracle
    // pairs `undefined_function` — the snapshot is taken at the
    // identifier's push in the resolution layer, outside this window's
    // files (recorded for the owning lane).
    let enum_source = "class T { const T(); }\nenum E {\n  @T({Missing()}) a, b,\n}";
    let rep = report(enum_source);
    let got = rows(&rep, enum_source);
    assert!(
        got.contains(&("const_with_non_constant_argument", "Missing()")),
        "{got:?}"
    );
    assert!(
        got.contains(&("non_constant_set_element", "Missing()")),
        "{got:?}"
    );
    assert!(got.contains(&("undefined_method", "Missing")), "{got:?}");
    assert_eq!(count(&got, "undefined_function"), 0, "{got:?}");

    let param_source = "class T { const T(); }\nvoid h(@T({Missing()}) int p) {}";
    let rep = report(param_source);
    let got = rows(&rep, param_source);
    assert!(
        got.contains(&("const_with_non_constant_argument", "Missing()")),
        "{got:?}"
    );
    assert!(
        got.contains(&("non_constant_set_element", "Missing()")),
        "{got:?}"
    );
    assert!(got.contains(&("undefined_function", "Missing")), "{got:?}");
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

// —— pattern 4: the annotation const-context deferred faces (#2261) ———

/// The rider (the corpus's `ast.dart` `GenerateNodeProperty('element',
/// type: ExtensionElementImpl)` shape): an unresolved leaf inside a
/// RESOLVED const creation's argument list reports `invalid_constant` at
/// the leaf — the oracle's creation evaluation reports the leaf directly
/// (value.dart `genericError`) and the literal's element face never
/// substitutes. The `type:` named-argument leaf never reaches the
/// positional argument check, so nothing else pairs at the span.
#[test]
fn rider_named_leaf_in_resolved_creation_reports_invalid_constant() {
    let source = "class Prop {\n  final String name;\n  final Type? type;\n  const Prop(this.name, {this.type});\n}\n\
                  class Gen {\n  final List<Prop> props;\n  const Gen({required this.props});\n}\n\
                  @Gen(props: [Prop('a'), Prop('e', type: Missing)])\nclass C {}";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert!(rows.contains(&("invalid_constant", "Missing")), "{rows:?}");
    assert!(
        rows.contains(&("undefined_identifier", "Missing")),
        "{rows:?}"
    );
    assert_eq!(count(&rows, "invalid_constant"), 1, "{rows:?}");
    assert_eq!(count(&rows, "non_constant_list_element"), 0, "{rows:?}");
    assert_eq!(
        count(&rows, "const_with_non_constant_argument"),
        0,
        "{rows:?}"
    );
}

/// The same leaf world in a const initializer (`const x = [Prop('g',
/// type: Missing)]`): `invalid_constant` at the leaf, the declaration's
/// own verdict silent (the propagation is avoid-reporting upstream —
/// nothing but the undefined name reports beside it).
#[test]
fn rider_initializer_named_leaf_reports_invalid_constant() {
    let source = "class Prop {\n  final String name;\n  final Type? type;\n  const Prop(this.name, {this.type});\n}\n\
                  const x = [Prop('g', type: Missing)];";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert!(rows.contains(&("invalid_constant", "Missing")), "{rows:?}");
    assert!(
        rows.contains(&("undefined_identifier", "Missing")),
        "{rows:?}"
    );
    assert_eq!(count(&rows, "invalid_constant"), 1, "{rows:?}");
    assert_eq!(count(&rows, "non_constant_list_element"), 0, "{rows:?}");
    assert_eq!(
        count(&rows, "const_initialized_with_non_constant_value"),
        0,
        "{rows:?}"
    );
}

/// A POSITIONAL unresolved leaf keeps the creation's
/// `const_with_non_constant_argument` (the pinned `Precedence._`
/// pairing) — never an `invalid_constant` twin, and the list element
/// face stays silent at that leaf (one argument, or several: the
/// preceding valid element changes nothing).
#[test]
fn positional_leaf_keeps_const_with_non_constant_argument() {
    for tail in [
        "const q = [Prop(Missing)];",
        "const z = [Prop('a'), Prop(Missing)];",
    ] {
        let source =
            format!("class Prop {{\n  final String name;\n  const Prop(this.name);\n}}\n{tail}");
        let rep = report(&source);
        let rows = rows(&rep, &source);
        assert!(
            rows.contains(&("const_with_non_constant_argument", "Missing")),
            "{rows:?}"
        );
        assert_eq!(
            count(&rows, "const_with_non_constant_argument"),
            1,
            "{rows:?}"
        );
        assert_eq!(count(&rows, "invalid_constant"), 0, "{rows:?}");
        assert_eq!(count(&rows, "non_constant_list_element"), 0, "{rows:?}");
    }
}

/// A bare unresolved leaf as the list element itself (the annotation's
/// const creation takes the list): the element face AND the argument
/// face pair at the leaf (the probed `@P([Missing])` triple) — the leaf
/// is not inside a nested creation, so no `invalid_constant` world.
#[test]
fn annotation_bare_list_element_pairs_argument_face() {
    let source = "class W {\n  final List<int> xs;\n  const W(this.xs);\n}\n\
                  @W([Missing])\nclass C {}";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert!(
        rows.contains(&("const_with_non_constant_argument", "Missing")),
        "{rows:?}"
    );
    assert!(
        rows.contains(&("non_constant_list_element", "Missing")),
        "{rows:?}"
    );
    assert!(
        rows.contains(&("undefined_identifier", "Missing")),
        "{rows:?}"
    );
    assert_eq!(count(&rows, "invalid_constant"), 0, "{rows:?}");
}

/// A resolved const constructor in annotation position validates its
/// arguments: a known non-const argument reports
/// `const_with_non_constant_argument` at it, the declaration-side
/// verdicts stay silent (the annotation evaluation has no initializer
/// site default).
#[test]
fn annotation_head_checks_non_constant_argument() {
    let source = "class P {\n  final int a;\n  const P(this.a);\n}\n\
                  var g = 1;\n\
                  @P(g)\nclass C {}";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert!(
        rows.contains(&("const_with_non_constant_argument", "g")),
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
}

/// The argument-count faces in annotation position: a deficit anchors at
/// the head, a surplus at the first extra positional argument (the
/// probed `@P()` / `@P(1, 2, 3)` pairings — capacity two puts the
/// surplus on the third).
#[test]
fn annotation_head_arity_faces() {
    let source = "class P {\n  final int a;\n  final int b;\n  const P(this.a, [this.b = 1]);\n}\n\
                  @P()\nclass C {}\n\
                  @P(1, 2, 3)\nclass D {}";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert!(
        rows.contains(&("not_enough_positional_arguments", "P")),
        "{rows:?}"
    );
    assert!(
        rows.contains(&("extra_positional_arguments", "3")),
        "{rows:?}"
    );
    assert_eq!(
        count(&rows, "not_enough_positional_arguments"),
        1,
        "{rows:?}"
    );
    assert_eq!(count(&rows, "extra_positional_arguments"), 1, "{rows:?}");
}

/// The named-argument worlds: a label that resolves to a formal pairs
/// the argument check (`const_with_non_constant_argument` at the value);
/// a label naming no formal never pairs — the official's evaluation
/// reports the leaf directly (`invalid_constant`).
#[test]
fn annotation_head_named_argument_worlds() {
    let source = "class Q {\n  final Type? t;\n  const Q({this.t});\n}\n\
                  class R {\n  final int a;\n  const R(this.a);\n}\n\
                  @Q(t: Missing)\nclass A {}\n\
                  @R(zz: Missing)\nclass B {}";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert!(
        rows.contains(&("const_with_non_constant_argument", "Missing")),
        "{rows:?}"
    );
    assert!(rows.contains(&("invalid_constant", "Missing")), "{rows:?}");
    assert_eq!(
        count(&rows, "const_with_non_constant_argument"),
        1,
        "{rows:?}"
    );
    assert_eq!(count(&rows, "invalid_constant"), 1, "{rows:?}");
}

/// An unresolved callee invocation inside the annotation's list: the
/// callee face stays `undefined_function` (the oracle's
/// invocation-context choice), the element face reports at the whole
/// invocation, and the annotation's argument check pairs
/// `const_with_non_constant_argument` there — beside the head's own
/// argument-count face (the list is one positional over the zero-arg
/// `T()`).
#[test]
fn annotation_unresolved_callee_argument_faces() {
    let source = "class T { const T(); }\n@T([Missing()])\nclass C {}";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert!(
        rows.contains(&("undefined_function", "Missing")),
        "{rows:?}"
    );
    assert!(
        rows.contains(&("non_constant_list_element", "Missing()")),
        "{rows:?}"
    );
    assert!(
        rows.contains(&("const_with_non_constant_argument", "Missing()")),
        "{rows:?}"
    );
    assert!(
        rows.contains(&("extra_positional_arguments", "[Missing()]")),
        "{rows:?}"
    );
}

/// The ERROR-tier duplicate twins over const literals — annotation
/// arguments and const initializers alike (the WARNING twins of the
/// non-const literals keep their lane): the later equal key/element
/// reports at its own span, once per literal.
#[test]
fn const_literal_duplicates_report_error_twins() {
    let source = "class T { const T(); }\n\
                  @T({1: 'a', 1: 'b'})\nclass C {}\n\
                  @T({1, 2, 1})\nclass D {}\n\
                  const m = {1: 'a', 1: 'b'};\n\
                  const s = {1, 2, 1};";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert_eq!(count(&rows, "equal_keys_in_const_map"), 2, "{rows:?}");
    assert_eq!(count(&rows, "equal_elements_in_const_set"), 2, "{rows:?}");
    // The WARNING twins never fire on const literals.
    assert_eq!(count(&rows, "equal_keys_in_map"), 0, "{rows:?}");
    assert_eq!(count(&rows, "equal_elements_in_set"), 0, "{rows:?}");
}

/// Valid annotation arguments of a resolved const constructor keep
/// every face silent — the argument check, the count faces, and the
/// duplicate twins included.
#[test]
fn valid_annotation_arguments_stay_silent() {
    for source in [
        "class W {\n  final List<int> xs;\n  const W(this.xs);\n}\n@W([1, 2])\nclass C {}",
        "class Q {\n  final Type? t;\n  const Q({this.t});\n}\nconst k = 1;\n@Q(t: k)\nclass C {}",
        "class T { final Object m; const T(this.m); }\n@T({'a': 1, 'b': 2})\nclass C {}",
    ] {
        let rep = report(source);
        let rows = rows(&rep, source);
        assert_eq!(count(&rows, "invalid_constant"), 0, "{rows:?}");
        assert_eq!(
            count(&rows, "const_with_non_constant_argument"),
            0,
            "{rows:?}"
        );
        assert_eq!(
            count(&rows, "not_enough_positional_arguments"),
            0,
            "{rows:?}"
        );
        assert_eq!(count(&rows, "extra_positional_arguments"), 0, "{rows:?}");
        assert_eq!(count(&rows, "equal_keys_in_const_map"), 0, "{rows:?}");
        assert_eq!(count(&rows, "equal_elements_in_const_set"), 0, "{rows:?}");
        assert_eq!(count(&rows, "non_constant_list_element"), 0, "{rows:?}");
        assert_eq!(count(&rows, "non_constant_set_element"), 0, "{rows:?}");
    }
}
