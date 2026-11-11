//! #3908 pins: the mixin-application family emitters of
//! `enum_verifier.rs` — the private-name collision face, the
//! const-constructor-with-mixin-field face, and the instantiated spellings
//! the `mixin_application_not_implemented_interface` message renders.
//!
//! Upstream anchors (Dart SDK 3.13.7, the pinned oracle):
//!
//! * `_checkForMixinWithConflictingPrivateMember` (error_verifier.dart
//!   :6192-6292): a mixin applied from ANOTHER library whose private member
//!   name collides with the same name on an earlier `with` entry of that
//!   library, or with a concrete member the declared supertype inherits from
//!   it, reports `private_collision_in_mixin_application` at the offending
//!   entry (`.at(namedType)`) and stops.
//! * `_checkForConstConstructorWithNonConstSuper` (:4040-4100): a const
//!   generative constructor whose applied mixins bring instance fields
//!   reports at the constructor head's `errorRange`; several fields switch
//!   the message to the plural spelling (the same machine code name).
//! * `_checkForMixinSuperclassConstraints` (:6079-6122): the message
//!   interpolates the INSTANTIATED faces — `'M<dynamic>'`, `'A<dynamic>'`.

use super::test_rig::{CORE_PRELUDE, RigImport, report_ops, report_ops_imports};

/// The prelude's UTF-16 length (the `report_ops` fixtures' line/offset
/// shift; the prelude is one line, so line numbers shift by one).
fn shift() -> u32 {
    u32::try_from(CORE_PRELUDE.encode_utf16().count()).expect("prelude fits u32")
}

/// The fixture's own rows of one code, as `(offset, message)`, in row order.
fn rows_of(report: &crate::diagnose::FileReport, code: &str, shifted: bool) -> Vec<(u32, String)> {
    let shift = if shifted { shift() } else { 0 };
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == code)
        .map(|row| (row.range.offset - shift, row.message.to_string()))
        .collect()
}

/// The UTF-16 span of `needle`'s first occurrence in `source`.
fn span_of(source: &str, needle: &str) -> (u32, u32) {
    let at = source.find(needle).expect("needle present in fixture");
    let offset = u32::try_from(source[..at].encode_utf16().count()).expect("fixture fits u32");
    let length = u32::try_from(needle.encode_utf16().count()).expect("needle fits u32");
    (offset, length)
}

const PRIVATE_COLLISION: &str = "private_collision_in_mixin_application";
const CONST_MIXIN_FIELD: &str = "const_constructor_with_mixin_with_field";
const NOT_IMPLEMENTED: &str = "mixin_application_not_implemented_interface";

// —— private_collision_in_mixin_application ——————————————————————————

/// Two `with` entries from one imported library defining the same private
/// method name: the FIRST collision reports at the second entry's name
/// token, naming both written mixin spellings.
#[test]
fn private_collision_two_mixins_reports_at_the_second_entry() {
    let source = "import 'a.dart';\n\nclass C extends Object with A, B {}\n";
    let report = report_ops_imports(
        source,
        &[RigImport {
            uri: "package:t/a.dart",
            prefix: None,
            source: "mixin A {\n  void _foo() {}\n}\n\nmixin B {\n  void _foo() {}\n}\n",
        }],
    );
    let rows = rows_of(&report, PRIVATE_COLLISION, true);
    let (offset, length) = span_of(source, "B");
    assert_eq!(
        rows,
        vec![(
            offset,
            "The private name '_foo', defined by 'B', conflicts with the same name defined by 'A'."
                .to_string()
        )],
        "one row at the second entry, got {rows:?}"
    );
    assert_eq!(length, 1, "the entry name token is one unit wide");
}

/// The supertype arm of the same face: a concrete private member the
/// declared superclass declares in the mixin's library answers the second
/// placeholder with that declaring class.
#[test]
fn private_collision_superclass_member_reports_the_declaring_class() {
    let source = "import 'a.dart';\n\nclass C extends A with B {}\n";
    let report = report_ops_imports(
        source,
        &[RigImport {
            uri: "package:t/a.dart",
            prefix: None,
            source: "class A {\n  int get _foo => 0;\n}\n\nmixin B {\n  int get _foo => 0;\n}\n",
        }],
    );
    assert_eq!(
        rows_of(&report, PRIVATE_COLLISION, true),
        vec![(
            span_of(source, "B").0,
            "The private name '_foo', defined by 'B', conflicts with the same name defined by 'A'."
                .to_string()
        )]
    );
}

/// A setter collision spells the BARE name (upstream trims the `=` off the
/// lookup spelling before interpolating it).
#[test]
fn private_collision_setter_spells_the_bare_name() {
    let source = "import 'a.dart';\n\nclass C extends A with B {}\n";
    let report = report_ops_imports(
        source,
        &[RigImport {
            uri: "package:t/a.dart",
            prefix: None,
            source: "class A {\n  set _foo(int _) {}\n}\n\nmixin B {\n  set _foo(int _) {}\n}\n",
        }],
    );
    assert_eq!(
        rows_of(&report, PRIVATE_COLLISION, true),
        vec![(
            span_of(source, "B").0,
            "The private name '_foo', defined by 'B', conflicts with the same name defined by 'A'."
                .to_string()
        )]
    );
}

/// The same-library gate: two mixins declared beside the class are the
/// ordinary shadowing case and stay silent.
#[test]
fn private_collision_same_library_stays_silent() {
    let source = "mixin A {\n  void _foo() {}\n}\n\nmixin B {\n  void _foo() {}\n}\n\n\
                  class C extends Object with A, B {}\n";
    assert_eq!(
        rows_of(&report_ops(source), PRIVATE_COLLISION, true),
        Vec::<(u32, String)>::new()
    );
}

/// A static private member never collides (upstream skips `isStatic`).
#[test]
fn private_collision_static_member_stays_silent() {
    let source = "import 'a.dart';\n\nclass C extends Object with A, B {}\n";
    let report = report_ops_imports(
        source,
        &[RigImport {
            uri: "package:t/a.dart",
            prefix: None,
            source: "mixin A {\n  static void _foo() {}\n}\n\nmixin B {\n  void _foo() {}\n}\n",
        }],
    );
    assert_eq!(
        rows_of(&report, PRIVATE_COLLISION, true),
        Vec::<(u32, String)>::new()
    );
}

// —— const_constructor_with_mixin_with_field ————————————————————————

/// The `typeName` constructor head anchors at its own name token and the
/// message names the mixin's field as `'Mixin.field'`.
#[test]
fn const_constructor_mixin_field_reports_at_the_name_token() {
    let source = "mixin A {\n  var a;\n}\n\nclass B extends Object with A {\n  const B();\n}\n";
    let report = report_ops(source);
    assert_eq!(
        rows_of(&report, CONST_MIXIN_FIELD, true),
        vec![(
            span_of(source, "B()").0,
            "This constructor can't be declared 'const' because a mixin adds the instance field: \
             'A.a'."
                .to_string()
        )]
    );
}

/// An unnamed `new`-headed constructor anchors at the `new` keyword.
#[test]
fn const_constructor_new_head_anchors_at_the_keyword() {
    let source = "mixin A {\n  var a;\n}\n\nclass B extends Object with A {\n  const new();\n}\n";
    let report = report_ops(source);
    assert_eq!(
        rows_of(&report, CONST_MIXIN_FIELD, true),
        vec![(
            span_of(source, "new").0,
            "This constructor can't be declared 'const' because a mixin adds the instance field: \
             'A.a'."
                .to_string()
        )]
    );
}

/// Several mixin fields switch to the plural message (the same machine code
/// name, upstream `constConstructorWithMixinWithFields`).
#[test]
fn const_constructor_plural_fields_use_the_plural_message() {
    let source =
        "mixin A {\n  var a;\n  var b;\n}\n\nclass B extends Object with A {\n  const B();\n}\n";
    let report = report_ops(source);
    assert_eq!(
        rows_of(&report, CONST_MIXIN_FIELD, true),
        vec![(
            span_of(source, "B()").0,
            "This constructor can't be declared 'const' because the mixins add the instance \
             fields: 'A.a', 'A.b'."
                .to_string()
        )]
    );
}

/// A getter-only mixin brings no instance field: the constructor stays
/// legal.
#[test]
fn const_constructor_getter_only_mixin_stays_silent() {
    let source =
        "mixin A {\n  int get a => 7;\n}\n\nclass B extends Object with A {\n  const B();\n}\n";
    assert_eq!(
        rows_of(&report_ops(source), CONST_MIXIN_FIELD, true),
        Vec::<(u32, String)>::new()
    );
}

// —— mixin_application_not_implemented_interface ————————————————————

/// The message spells the instantiated faces: the applied mixin and the
/// unsatisfied constraint both instantiate their unbounded parameter to
/// `dynamic` (upstream renders `mixinType`/`notImplementedType`, never the
/// bare names).
#[test]
fn mixin_application_message_spells_instantiated_faces() {
    let source = "class A<T> {}\n\nmixin M<T> on A<T> {}\n\nclass C extends Object with M {}\n";
    let report = report_ops(source);
    assert_eq!(
        rows_of(&report, NOT_IMPLEMENTED, true),
        vec![(
            span_of(source, "M {").0,
            "'M<dynamic>' can't be mixed onto 'Object' because 'Object' doesn't implement \
             'A<dynamic>'."
                .to_string()
        )]
    );
}

/// A written argument list renders verbatim: `with M<int>` keeps `<int>`.
#[test]
fn mixin_application_message_keeps_written_arguments() {
    let source =
        "class A<T> {}\n\nmixin M<T> on A<T> {}\n\nclass C extends Object with M<int> {}\n";
    let report = report_ops(source);
    let rows = rows_of(&report, NOT_IMPLEMENTED, true);
    assert_eq!(rows.len(), 1, "one row, got {rows:?}");
    assert!(
        rows[0]
            .1
            .starts_with("'M<int>' can't be mixed onto 'Object'"),
        "the written argument list renders verbatim, got {rows:?}"
    );
}
