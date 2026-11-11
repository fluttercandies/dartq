//! #3901 pins — the `types.rs` half of the mixin clause resolution family
//! (#3888's ~112 census rows): the two faces the mixin-application TYPE model
//! decides, namely `const_constructor_with_mixin_with_field[s]` and
//! `enum_mixin_with_instance_variable`. Both read a written mixin's DECLARED
//! field set — the type-model question the census reports as missing.
//!
//! Upstream references (Dart SDK 3.13.7, the oracle's pinned version):
//!
//! * `pkg/analyzer/lib/src/generated/error_verifier.dart`
//!   `_checkForConstConstructorWithNonConstSuper` (:4040-4130): a const
//!   GENERATIVE constructor reports the mixin's instance fields first and
//!   pre-empts its two siblings (the non-const super and the non-final
//!   field). The field set is `enclosingClass.mixins[*].element.fields` —
//!   each written mixin's own declared fields, static and the abstract+final
//!   pair excepted, `external` included, inheritance never joined.
//! * `pkg/analyzer/lib/src/error/inheritance_override.dart`
//!   `_checkMixinOfEnum` (:869-893), reached from `_checkDirectSuperTypes`'
//!   with-clause loop (:684-695): an enum's `with`-clause entry whose mixin
//!   declares an instance variable reports at the written `NamedType` node;
//!   enum and extension-type entries stand down, and this face's field test
//!   DOES exempt `external`.
//!
//! Oracle provenance: `dart analyze --format=machine` on the minimal
//! fixtures below, e.g.
//! `ERROR|COMPILE_TIME_ERROR|CONST_CONSTRUCTOR_WITH_MIXIN_WITH_FIELD|…|6|9|1|
//! This constructor can't be declared 'const' because a mixin adds the
//! instance field: 'A.a'.`

use crate::diagnose::FileReport;
use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops, rows};

const MIXIN_FIELD: &str = "const_constructor_with_mixin_with_field";
const NON_FINAL: &str = "const_constructor_with_non_final_field";
const ENUM_MIXIN: &str = "enum_mixin_with_instance_variable";

/// One row of `code`: `(offset, len, message)`, source order.
fn rows_of(report: &FileReport, code: &str) -> Vec<(u32, u32, String)> {
    let mut out: Vec<(u32, u32, String)> = report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == code)
        .map(|row| (row.range.offset, row.range.length, row.message.to_string()))
        .collect();
    out.sort_by_key(|(offset, _, _)| *offset);
    out
}

/// The absolute fixture offset of `needle` (the rig's prelude included).
fn at(fixture: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + fixture.find(needle).unwrap()) as u32
}

/// The absolute offset of the `with`-clause entry written as `entry`.
fn with_entry(fixture: &str, entry: &str) -> u32 {
    at(fixture, &format!("with {entry}")) + "with ".len() as u32
}

// —— the const-generative-constructor mixin face ——————————————————————

/// One mixin field: the singular phrasing, anchored at the constructor's
/// `errorRange` (the type name through its selector) and pre-empting the
/// non-final-field sibling.
#[test]
fn ticket_3901_const_ctor_mixin_field_singular() {
    let fixture = "mixin A {\n  final a = 0;\n}\n\nclass B with A {\n  const B();\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        rows_of(&report, MIXIN_FIELD),
        vec![(
            at(fixture, "B();"),
            1,
            "This constructor can't be declared 'const' because a mixin adds the instance field: \
             'A.a'."
                .to_string(),
        )],
        "rows: {:?}",
        rows(&report)
    );
    assert!(
        rows_of(&report, NON_FINAL).is_empty(),
        "the mixin face pre-empts: {:?}",
        rows(&report)
    );
}

/// Two mixin fields: the plural phrasing, every field listed in DECLARATION
/// order (`var b; var a;` answers `'A.b', 'A.a'` — not the name-map order).
#[test]
fn ticket_3901_const_ctor_mixin_fields_plural_in_declaration_order() {
    let fixture = "mixin A {\n  var b;\n  var a;\n}\n\nclass B with A {\n  const B();\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        rows_of(&report, MIXIN_FIELD),
        vec![(
            at(fixture, "B();"),
            1,
            "This constructor can't be declared 'const' because the mixins add the instance \
             fields: 'A.b', 'A.a'."
                .to_string(),
        )],
        "rows: {:?}",
        rows(&report)
    );
}

/// The field test's two exemptions and one non-exemption: an abstract
/// NON-final field counts (its setter makes it storage), an abstract FINAL
/// field is a getter alone, and an `external` field counts — this face has
/// no `isExternal` arm.
#[test]
fn ticket_3901_const_ctor_mixin_field_predicate() {
    let abstract_field = "mixin A {\n  abstract int a;\n}\n\nclass B with A {\n  @override\n  \
                         int a = 0;\n  const B();\n}\n";
    let report = report_ops(abstract_field);
    assert_eq!(
        rows_of(&report, MIXIN_FIELD).len(),
        1,
        "an abstract non-final field counts: {:?}",
        rows(&report)
    );

    let abstract_final = "mixin A {\n  abstract final int a;\n}\n\nclass B with A {\n  @override\n  \
                          final int a = 0;\n  const B();\n}\n";
    let report = report_ops(abstract_final);
    assert!(
        rows_of(&report, MIXIN_FIELD).is_empty(),
        "an abstract final field is a getter alone: {:?}",
        rows(&report)
    );

    let external = "mixin A {\n  external final int a;\n}\n\nclass B with A {\n  const B();\n}\n";
    let report = report_ops(external);
    assert_eq!(
        rows_of(&report, MIXIN_FIELD),
        vec![(
            at(external, "B();"),
            1,
            "This constructor can't be declared 'const' because a mixin adds the instance field: \
             'A.a'."
                .to_string(),
        )],
        "external counts here: {:?}",
        rows(&report)
    );
}

/// A class used as a mixin contributes only its DECLARED fields: the field
/// it inherits from its own superclass is the `hasNonFinalField` face's,
/// which reports the non-final-field row instead of the mixin one.
#[test]
fn ticket_3901_const_ctor_mixin_field_ignores_inherited_fields() {
    let fixture = "class S {\n  var x = 0;\n}\n\nclass A extends S {}\n\nclass B with A {\n  \
                   const B();\n}\n";
    let report = report_ops(fixture);
    assert!(
        rows_of(&report, MIXIN_FIELD).is_empty(),
        "an inherited field is not a mixin field: {:?}",
        rows(&report)
    );
    assert_eq!(
        rows_of(&report, NON_FINAL).len(),
        1,
        "the non-final-field sibling owns it: {:?}",
        rows(&report)
    );
}

/// A getter-only mixin answers no row, and the class's own non-final field
/// keeps its own face (the mixin loop reports nothing there).
#[test]
fn ticket_3901_const_ctor_getter_only_mixin_keeps_the_non_final_row() {
    let fixture =
        "mixin A {\n  int get a => 0;\n}\n\nclass B with A {\n  var v;\n  const B();\n}\n";
    let report = report_ops(fixture);
    assert!(
        rows_of(&report, MIXIN_FIELD).is_empty(),
        "a getter is not an instance field: {:?}",
        rows(&report)
    );
    assert_eq!(
        rows_of(&report, NON_FINAL).len(),
        1,
        "rows: {:?}",
        rows(&report)
    );
}

// —— the enum `with`-clause face ——————————————————————————————————————

/// One offending entry: the row anchors at the written `NamedType` node and
/// the message is the code's constant text.
#[test]
fn ticket_3901_enum_mixin_instance_variable_reports_at_the_entry() {
    let fixture = "mixin Mix {\n  int x = 0;\n}\n\nenum E with Mix {\n  a\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        rows_of(&report, ENUM_MIXIN),
        vec![(
            with_entry(fixture, "Mix {"),
            3,
            "Mixins applied to enums can't have instance variables.".to_string(),
        )],
        "rows: {:?}",
        rows(&report)
    );
}

/// The anchor spans the whole `NamedType` node: a parameterized entry covers
/// its type-argument group (`G<int>` — six characters), exactly the oracle's
/// `.at(namedType)` extent.
#[test]
fn ticket_3901_enum_mixin_instance_variable_spans_the_type_arguments() {
    let fixture = "mixin G<T> {\n  T? x;\n}\n\nenum E with G<int> {\n  a\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        rows_of(&report, ENUM_MIXIN),
        vec![(
            with_entry(fixture, "G<int> {"),
            6,
            "Mixins applied to enums can't have instance variables.".to_string(),
        )],
        "rows: {:?}",
        rows(&report)
    );
}

/// Per entry, in application order: each enum reports only its own `with`
/// clause entries (the span pairing never leaks across declarations).
#[test]
fn ticket_3901_enum_mixin_instance_variable_pairs_per_declaration() {
    let fixture = "mixin P {\n  int p = 0;\n}\n\nmixin Q {\n  int q = 0;\n}\n\nmixin R {}\n\n\
                   enum E1 with P, R {\n  a\n}\n\nenum E2 with Q {\n  b\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        rows_of(&report, ENUM_MIXIN),
        vec![
            (
                with_entry(fixture, "P, R {"),
                1,
                "Mixins applied to enums can't have instance variables.".to_string(),
            ),
            (
                with_entry(fixture, "Q {"),
                1,
                "Mixins applied to enums can't have instance variables.".to_string(),
            ),
        ],
        "rows: {:?}",
        rows(&report)
    );
}

/// The field predicates: `var` and `final` fields report, while a getter, a
/// setter, a static field, an abstract field and an `external` field stand
/// down (`_checkMixinOfEnum`'s four-way exemption).
#[test]
fn ticket_3901_enum_mixin_instance_variable_field_predicate() {
    for exempt in [
        "int get x => 0;",
        "set x(int v) {}",
        "static var x = 0;",
        "abstract int x;",
        "external final int x;",
    ] {
        let fixture = format!("mixin Mix {{\n  {exempt}\n}}\n\nenum E with Mix {{\n  a\n}}\n");
        let report = report_ops(&fixture);
        assert!(
            rows_of(&report, ENUM_MIXIN).is_empty(),
            "`{exempt}` is exempt: {:?}",
            rows(&report)
        );
    }
    for reporting in ["var x = 0;", "final x = 0;", "late int x;"] {
        let fixture = format!("mixin Mix {{\n  {reporting}\n}}\n\nenum E with Mix {{\n  a\n}}\n");
        let report = report_ops(&fixture);
        assert_eq!(
            rows_of(&report, ENUM_MIXIN).len(),
            1,
            "`{reporting}` reports: {:?}",
            rows(&report)
        );
    }
}

/// An enum or extension-type entry stands down (`interfaceElement is
/// EnumElement || ExtensionTypeElement`), and an unresolvable entry reports
/// nothing at all.
#[test]
fn ticket_3901_enum_mixin_instance_variable_kind_gate() {
    let enum_entry = "enum X {\n  a;\n  int get x => 0;\n}\n\nenum E with X {\n  b\n}\n";
    let report = report_ops(enum_entry);
    assert!(
        rows_of(&report, ENUM_MIXIN).is_empty(),
        "an enum entry is exempt: {:?}",
        rows(&report)
    );

    let extension_type = "extension type ET(int i) {\n  int get x => i;\n}\n\nenum E with ET {\n  \
                          a\n}\n";
    let report = report_ops(extension_type);
    assert!(
        rows_of(&report, ENUM_MIXIN).is_empty(),
        "an extension-type entry is exempt: {:?}",
        rows(&report)
    );
}
