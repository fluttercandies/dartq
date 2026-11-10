//! The #3127 pins: `invalid_override` — the member-override contract.
//!
//! A declared instance member must satisfy the override contract against
//! every superinterface it overrides: the function-subtype rule (the
//! override must accept every call the inherited member accepts —
//! contravariant parameters, covariant return, arity and named-parameter
//! supersets) plus the `CovariantParametersVerifier` relation for
//! `covariant` formals (upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/correct_override.dart:36-39 and :96-138;
//! wired from
//! pkg/analyzer/lib/src/error/inheritance_override.dart:498-546
//! `_checkDeclaredMember`/`_checkDeclaredField`). A field rides its
//! implicit getter and — non-final only — its implicit setter, so a field
//! type mismatch reports BOTH faces.
//!
//! Oracle provenance: `dart analyze` 3.13.7 over the /tmp/io3127
//! override battery (`cases/` — field/param/return/arity/named/operator/
//! covariant/final faces plus the legal-override silences).
//!
//! The message anchor is the member's name token; the signature text
//! renders `{ret} Function({params})` on both sides, and the member's
//! display name keeps an operator's own `=` (`==`, `[]=`) while a real
//! setter's `name=` marker strips back to `name` (`member_display_name`).

use super::test_rig::{CORE_PRELUDE, assert_silent, report_ops, rows};

/// The absolute fixture offset of `needle` (the prelude the rig prepends
/// included; the fixtures are ASCII, so byte offset == UTF-16 offset).
fn at(fixture: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + fixture.find(needle).unwrap()) as u32
}

/// The `invalid_override` rows as `(code, offset, len)` sorted by offset —
/// the oracle's machine-row face. Both the plain `invalidOverride` and the
/// `invalidOverrideSetter` diagnostics share the `invalid_override` machine
/// name upstream (`diagnostic.g.dart:9556-9630`'s `name:` field; the
/// `uniqueName` split is internal identity, never the emitted code), so a
/// field's induced-setter face and a written setter both read
/// `invalid_override` — the `The setter '…'` wording lives in the message.
fn overrides(report: &crate::diagnose::FileReport) -> Vec<(&'static str, u32, u32)> {
    let mut v: Vec<(&'static str, u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_override")
        .map(|d| (d.code.name, d.range.offset, d.range.length))
        .collect();
    v.sort_by_key(|(code, offset, _)| (*offset, *code));
    v
}

// —— field faces ————————————————————————————————————————————————————————

/// `String x` over `int x` reports the field's implicit GETTER face
/// (`'B.x' ('String Function()')`) and — non-final — its implicit SETTER
/// face (`The setter 'B.x' ('void Function(String)')`): two rows at the
/// field name (upstream `_checkDeclaredField` checks `field.getter` then
/// `field.setter`, inheritance_override.dart:498-546).
#[test]
fn field_type_mismatch_reports_getter_and_setter_faces() {
    let fixture = "class A{int x=0;}\nclass B extends A{String x=\"\";}\n";
    let report = report_ops(fixture);
    let at_x = at(fixture, "x=\"\"");
    assert_eq!(
        overrides(&report),
        vec![("invalid_override", at_x, 1), ("invalid_override", at_x, 1)],
        "rows: {:?}",
        rows(&report)
    );
}

/// A `final` field has no setter, so a `final int x` over `int x` stays
/// silent — and even a `final`-to-`var` narrowing of the same type is a
/// legal override (oracle emits nothing).
#[test]
fn final_to_var_same_type_is_silent() {
    let report = report_ops("class A{final int x=0;}\nclass B extends A{int x=0;}\n");
    assert!(overrides(&report).is_empty(), "rows: {:?}", rows(&report));
}

/// A field overriding an interface GETTER reports the getter face —
/// `String x` over `int get x`.
#[test]
fn field_over_getter_reports_getter_face() {
    let fixture = "class A{int get x=>0;}\nclass B extends A{String x=\"\";}\n";
    let report = report_ops(fixture);
    assert_eq!(
        overrides(&report),
        vec![("invalid_override", at(fixture, "x=\"\""), 1)],
        "rows: {:?}",
        rows(&report)
    );
}

/// A getter overriding a field reports the getter face — `String get x`
/// over `int x`.
#[test]
fn getter_over_field_reports_getter_face() {
    let fixture = "class A{int x=0;}\nclass B extends A{String get x=>\"\";}\n";
    let report = report_ops(fixture);
    assert_eq!(
        overrides(&report),
        vec![("invalid_override", at(fixture, "x=>"), 1)],
        "rows: {:?}",
        rows(&report)
    );
}

// —— parameter / return faces ——————————————————————————————————————————

/// `void m(String)` over `void m(int)` — the parameter is contravariant,
/// `String` does not accept `int`, so `invalid_override` at `m`.
#[test]
fn param_type_mismatch_reports_invalid_override() {
    let fixture = "class A{void m(int x){}}\nclass B extends A{void m(String x){}}\n";
    let report = report_ops(fixture);
    assert_eq!(
        overrides(&report),
        vec![("invalid_override", at(fixture, "m(String"), 1)],
        "rows: {:?}",
        rows(&report)
    );
}

/// `String m()` over `int m()` — the return is covariant, `String` does
/// not fit `int`, so `invalid_override` at `m`.
#[test]
fn return_type_mismatch_reports_invalid_override() {
    let fixture = "class A{int m()=>0;}\nclass B extends A{String m()=>\"\";}\n";
    let report = report_ops(fixture);
    assert_eq!(
        overrides(&report),
        vec![("invalid_override", at(fixture, "m()=>\"\""), 1)],
        "rows: {:?}",
        rows(&report)
    );
}

/// `void m(int x)` over `void m()` — a newly required positional rejects
/// the super's no-argument calls (the function-subtype arity rule,
/// correct_override.dart:36-39), so `invalid_override`.
#[test]
fn added_required_positional_reports_invalid_override() {
    let fixture = "class A{void m(){}}\nclass B extends A{void m(int x){}}\n";
    let report = report_ops(fixture);
    assert_eq!(
        overrides(&report),
        vec![("invalid_override", at(fixture, "m(int"), 1)],
        "rows: {:?}",
        rows(&report)
    );
}

/// `void m()` over `void m(int x)` — fewer positionals rejects the
/// super's one-argument calls, so `invalid_override`.
#[test]
fn dropped_positional_reports_invalid_override() {
    let fixture = "class A{void m(int x){}}\nclass B extends A{void m(){}}\n";
    let report = report_ops(fixture);
    assert_eq!(
        overrides(&report),
        vec![("invalid_override", at(fixture, "m(){}"), 1)],
        "rows: {:?}",
        rows(&report)
    );
}

/// `void m()` over `void m({int x})` — dropping the named parameter
/// rejects calls that pass it, so `invalid_override`.
#[test]
fn dropped_named_parameter_reports_invalid_override() {
    let fixture = "class A{void m({int x=0}){}}\nclass B extends A{void m(){}}\n";
    let report = report_ops(fixture);
    assert_eq!(
        overrides(&report),
        vec![("invalid_override", at(fixture, "m(){}"), 1)],
        "rows: {:?}",
        rows(&report)
    );
}

/// `void m({required int x})` over `void m({int x})` — tightening an
/// optional named parameter to required rejects calls that omit it, so
/// `invalid_override`.
#[test]
fn newly_required_named_parameter_reports_invalid_override() {
    let fixture = "class A{void m({int x=0}){}}\nclass B extends A{void m({required int x}){}}\n";
    let report = report_ops(fixture);
    assert_eq!(
        overrides(&report),
        vec![("invalid_override", at(fixture, "m({required"), 1)],
        "rows: {:?}",
        rows(&report)
    );
}

/// A setter's parameter type mismatch reports `invalid_override` — the
/// `invalidOverrideSetter` diagnostic shares the `invalid_override` machine
/// name (`diagnostic.g.dart:9616-9630`'s `name:` is `invalid_override`);
/// only its `The setter '…'` message differs. `set x(String)` over
/// `set x(int)`.
#[test]
fn setter_type_mismatch_reports_setter_message_face() {
    let fixture = "class A{set x(int v){}}\nclass B extends A{set x(String v){}}\n";
    let report = report_ops(fixture);
    assert_eq!(
        overrides(&report),
        vec![("invalid_override", at(fixture, "x(String"), 1)],
        "rows: {:?}",
        rows(&report)
    );
    let msg = &report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "invalid_override")
        .unwrap()
        .message;
    assert!(
        msg.starts_with("The setter 'B.x'"),
        "a written setter renders the 'The setter' wording: {msg}"
    );
}

// —— legal overrides stay silent ————————————————————————————————————————

/// `void m(num)` over `void m(int)` — a widened parameter accepts every
/// `int` the super accepts, a legal contravariant override (silent).
#[test]
fn widened_parameter_is_silent() {
    let report = report_ops("class A{void m(int x){}}\nclass B extends A{void m(num x){}}\n");
    assert!(overrides(&report).is_empty(), "rows: {:?}", rows(&report));
}

/// `void m(int)` over `void m(num)` — a narrowed parameter does not
/// accept every `num`, so `invalid_override` (the narrowing is illegal
/// unless the parameter is `covariant`).
#[test]
fn narrowed_parameter_reports() {
    let fixture = "class A{void m(num x){}}\nclass B extends A{void m(int x){}}\n";
    let report = report_ops(fixture);
    assert_eq!(
        overrides(&report),
        vec![("invalid_override", at(fixture, "m(int"), 1)],
        "rows: {:?}",
        rows(&report)
    );
}

/// `int m()` over `num m()` — a narrowed return fits, a legal covariant
/// override.
#[test]
fn narrowed_return_is_silent() {
    let report = report_ops("class A{num m()=>0;}\nclass B extends A{int m()=>0;}\n");
    assert!(overrides(&report).is_empty(), "rows: {:?}", rows(&report));
}

/// A member with no same-named super member is a plain declaration — no
/// override relationship, no row.
#[test]
fn unrelated_member_is_silent() {
    let report = report_ops("class A{void m(int x){}}\nclass B extends A{void n(String x){}}\n");
    assert_silent(&report);
}

/// `void m(int)` over `void m(covariant int)` — a `covariant` super
/// parameter may be narrowed to any subtype (the covariant-propagation
/// face), so the override stays silent.
#[test]
fn covariant_super_parameter_allows_narrowing() {
    let report =
        report_ops("class A{void m(covariant int x){}}\nclass B extends A{void m(int x){}}\n");
    assert!(overrides(&report).is_empty(), "rows: {:?}", rows(&report));
}

// —— operator faces ————————————————————————————————————————————————————

/// `int operator+(int)` over `num operator+(num)` reports
/// `invalid_override` — the parameter is contravariant and `int` does not
/// accept `num`.
#[test]
fn operator_plus_param_mismatch_reports() {
    let fixture =
        "class A{num operator+(num o)=>0;}\nclass B extends A{int operator+(int o)=>0;}\n";
    let report = report_ops(fixture);
    assert_eq!(
        overrides(&report),
        vec![("invalid_override", at(fixture, "+(int"), 1)],
        "rows: {:?}",
        rows(&report)
    );
}

/// `bool operator==(B)` over `bool operator==(Object)` reports
/// `invalid_override` — the parameter is contravariant. The display keeps
/// the `==` spelling intact (the `=` is the operator's own name, not a
/// setter marker): the message reads `'B.=='`, never `'B.='`.
#[test]
fn operator_eq_param_mismatch_reports_with_eq_eq_display() {
    let fixture = "class A{bool operator==(Object o)=>true;}\nclass B extends A{bool operator==(B o)=>true;}\n";
    let report = report_ops(fixture);
    let found = overrides(&report);
    assert_eq!(
        found,
        vec![("invalid_override", at(fixture, "==(B"), 2)],
        "rows: {:?}",
        rows(&report)
    );
    let msg = &report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "invalid_override")
        .unwrap()
        .message;
    assert!(
        msg.contains("'B.=='"),
        "operator == must render intact, not stripped to '=': {msg}"
    );
}

/// `void operator[]=(int, String)` over `void operator[]=(int, num)`
/// reports `invalid_override` — the second parameter is contravariant;
/// the `[]=` spelling keeps its `=` intact in the message.
#[test]
fn operator_index_set_param_mismatch_reports() {
    let fixture = "class A{void operator[]=(int i,num v){}}\nclass B extends A{void operator[]=(int i,String v){}}\n";
    let report = report_ops(fixture);
    assert_eq!(
        overrides(&report),
        // B's `[]=` head — the second `[]=` in the fixture (A's own
        // declaration carries the first).
        vec![("invalid_override", at(fixture, "[]=(int i,String"), 3)],
        "rows: {:?}",
        rows(&report)
    );
}

/// `String operator[]=(int,num)` over `num operator[]=(int,num)` reports
/// `invalid_override` — the RETURN face of an index-set operator reads
/// its `return_ty` like any method's; `String` does not fit `num`.
#[test]
fn operator_index_set_return_mismatch_reports() {
    let fixture = "class A{num operator[]=(int i,num v){}}\nclass B extends A{String operator[]=(int i,num v){}}\n";
    let report = report_ops(fixture);
    assert_eq!(
        overrides(&report),
        // B's `[]=` head — the needle anchors at B's `String` return, and
        // `+15` walks `String operator` to the `[]=` token.
        vec![(
            "invalid_override",
            at(fixture, "String operator[]=") + 15,
            3
        )],
        "rows: {:?}",
        rows(&report)
    );
}

/// `bool operator<(String)` over `bool operator<(num)` reports
/// `invalid_override` — the single-parameter operator face, display `<`.
#[test]
fn operator_lt_param_mismatch_reports() {
    let fixture = "class A{bool operator<(num o)=>true;}\nclass B extends A{bool operator<(String o)=>true;}\n";
    let report = report_ops(fixture);
    assert_eq!(
        overrides(&report),
        vec![("invalid_override", at(fixture, "<(String"), 1)],
        "rows: {:?}",
        rows(&report)
    );
}

// —— interface / mixin-on / generic faces ———————————————————————————————

/// `implements` interfaces carry the same override contract as `extends`:
/// `void m(String)` implementing `void m(int)` reports `invalid_override`.
#[test]
fn implements_param_mismatch_reports() {
    let fixture = "class A{void m(int x){}}\nclass B implements A{void m(String x){}}\n";
    let report = report_ops(fixture);
    assert_eq!(
        overrides(&report),
        vec![("invalid_override", at(fixture, "m(String"), 1)],
        "rows: {:?}",
        rows(&report)
    );
}

/// A mixin's `on` constraint is an override face too — `void m(String)`
/// in `mixin M on A` where `A` declares `void m(int)` reports
/// `invalid_override` at `m`.
#[test]
fn mixin_on_constraint_param_mismatch_reports() {
    let fixture = "class A{void m(int x){}}\nmixin M on A{void m(String x){}}\n";
    let report = report_ops(fixture);
    assert_eq!(
        overrides(&report),
        vec![("invalid_override", at(fixture, "m(String"), 1)],
        "rows: {:?}",
        rows(&report)
    );
}

/// `void m(int)` over `void m(T)` through `A<int>` — the super signature
/// instantiates `T := int`, a legal override.
#[test]
fn generic_exact_substitution_is_silent() {
    let report = report_ops("class A<T>{void m(T x){}}\nclass B extends A<int>{void m(int x){}}\n");
    assert!(overrides(&report).is_empty(), "rows: {:?}", rows(&report));
}

/// `void m(int)` over `void m(T)` through `A<num>` — the super signature
/// instantiates `T := num`, so the `int` parameter narrows and reports
/// `invalid_override`.
#[test]
fn generic_substitution_narrowing_reports() {
    let fixture = "class A<T>{void m(T x){}}\nclass B extends A<num>{void m(int x){}}\n";
    let report = report_ops(fixture);
    assert_eq!(
        overrides(&report),
        vec![("invalid_override", at(fixture, "m(int"), 1)],
        "rows: {:?}",
        rows(&report)
    );
}
