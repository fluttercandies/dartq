//! #3148 pins: the `unnecessary_final` rule's parameter and for-each faces
//! against the pinned `dart analyze` 3.13.7 oracle.
//!
//! Two language postures decide the parameter faces. Under
//! `primary_constructors` (Dart 3.13+, `unnecessary_final.dart:64-68`) the
//! official parameter loop is skipped outright — the rows the 3.13.7 oracle
//! still prints for `C(final this.x)` come from the analyzer's own
//! `unnecessaryFinal` static warning (`best_practices_verifier.dart:1046`), a
//! semantic-tier row in dartq, never from this rule. Under a pre-3.13
//! language version (probed with a hand-written `package_config.json`
//! pinning `languageVersion: 3.0`) the loop arms and reports every `final`
//! formal, typed or not, from `this`/`super` field formals to plain
//! parameters — the pins below mount that armed face by clearing the
//! fixture's default `primary_constructors` bit (`[`assert_armed_report`]`).
//!
//! The for-each face is version-independent: the oracle registers
//! `visitForStatement` and reports the loop variable at every language
//! version, while the collection for-element (`ForElement`) stays unvisited.

use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

/// The UTF-16 offset of the `nth` occurrence of `needle` in `f`.
fn at(f: &Fixture, needle: &str, nth: usize) -> u32 {
    utf16_at(f, tok(&f.source, &f.tokens, needle, nth))
}

/// Pins one armed-face shape: exactly one row, anchored at the `final`
/// keyword (`rule.reportAtToken(node.constFinalOrVarKeyword)`,
/// unnecessary_final.dart:73-77). The fixture's default posture carries
/// `primary_constructors` (the shipped 3.13 feature), which gates the
/// parameter loop, so the pin mounts the pre-3.13 posture the face arms
/// under.
fn assert_armed_report(source: &str) {
    let mut f = Fixture::parse(source);
    f.features.primary_constructors = false;
    let lints = f.run(&one_rule("unnecessary_final"));
    assert_eq!(lints.len(), 1, "{source:?}: {lints:?}");
    assert_eq!(lints[0].range.start, at(&f, "final", 0), "{source:?}");
    assert_eq!(
        lints[0].message,
        "Local variables should not be marked as 'final'."
    );
}

// —— face 2: formal parameters (armed pre-3.13 posture) ——————————————
// The type annotation may sit between the keyword and the `this` receiver
// (`parseFormalParameter`'s type atom, then the `this`/`super` + `.` + name
// arm). The written type is what keys the with-type variant; the fixture
// pin only needs the row, the offset and the (shared) message.

#[test]
fn uf3148_typed_this_formal_reports() {
    assert_armed_report("class C { C(final int this.x); }");
}

#[test]
fn uf3148_untyped_this_formal_reports() {
    assert_armed_report("class C { C(final this.x); }");
}

#[test]
fn uf3148_generic_typed_this_formal_reports() {
    assert_armed_report("class C { C(final Map<String, int> this.m); }");
}

#[test]
fn uf3148_function_typed_this_formal_reports() {
    assert_armed_report("class C { C(final void Function(int)? this.f); }");
}

#[test]
fn uf3148_record_typed_this_formal_reports() {
    assert_armed_report("class C { C(final (int, String) this.r); }");
}

#[test]
fn uf3148_optional_positional_this_formal_reports() {
    assert_armed_report("class C { C([final int this.x = 0]); }");
}

#[test]
fn uf3148_required_named_this_formal_reports() {
    assert_armed_report("class C { C({required final int this.x = 0}); }");
}

#[test]
fn uf3148_covariant_this_formal_reports() {
    assert_armed_report("class C { C(covariant final int this.x); }");
}

#[test]
fn uf3148_plain_typed_formal_reports() {
    assert_armed_report("void f(final int x) {}");
}

/// The 3.13+ posture: `Feature.primary_constructors` skips the whole
/// parameter loop (`unnecessary_final.dart:66-68`), typed field formals and
/// plain parameters alike — the oracle's rows for these shapes are the
/// analyzer's static warning, not this rule.
#[test]
fn uf3148_primary_constructors_gates_every_parameter_face() {
    for source in [
        "class C { C(final int this.x); }",
        "class C { C(final this.x); }",
        "void f(final int x) {}",
    ] {
        let mut f = Fixture::parse(source);
        f.features.primary_constructors = true;
        assert!(
            f.run(&one_rule("unnecessary_final")).is_empty(),
            "{source:?}"
        );
    }
}

/// A statement-position `C(final this.x)` is a call, not a declaration: the
/// oracle prints its static warning there only through the analyzer's
/// declaration-body walk, and this rule — which visits formal parameter
/// lists — stays silent (`void f() { C(final this.x); }` reports no lint
/// row under 3.13.7).
#[test]
fn uf3148_statement_body_call_reports_nothing() {
    let mut f =
        Fixture::parse("class C { C(this.x); final int x; }\nvoid f(C c) { C(final this.x); }\n");
    f.features.primary_constructors = true;
    assert!(f.run(&one_rule("unnecessary_final")).is_empty());
}

/// The same for a recovered local `class` (`void f() { class C { C(final
/// this.x); } }`): no local declaration exists, so its `Identifier(…)` stays
/// a call.
#[test]
fn uf3148_local_class_reports_nothing() {
    let mut f = Fixture::parse("void f() { class C { C(final this.x); } }");
    f.features.primary_constructors = true;
    assert!(f.run(&one_rule("unnecessary_final")).is_empty());
}

// —— face 3: the for-each loop variable ——————————————————————————————
// The loop container seals as `ForIn` for a statement and as
// `ForControlFlow` for a collection for-element; the gate is that kind.

#[test]
fn uf3148_for_each_statement_reports() {
    assert_armed_report("void f(List<int> xs) { for (final x in xs) { print(x); } }");
}

#[test]
fn uf3148_typed_for_each_statement_reports() {
    let f = Fixture::parse("void f(List<int> xs) { for (final int x in xs) { print(x); } }");
    let lints = f.run(&one_rule("unnecessary_final"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].range.start, at(&f, "final", 0));
}

#[test]
fn uf3148_pattern_for_each_statement_reports() {
    let f = Fixture::parse("void f() { for (final (a, b) in [(1, 2)]) { print(a); } }");
    let lints = f.run(&one_rule("unnecessary_final"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].range.start, at(&f, "final", 0));
}

#[test]
fn uf3148_await_for_statement_reports() {
    let f =
        Fixture::parse("void f(Stream<int> s) async { await for (final x in s) { print(x); } }");
    let lints = f.run(&one_rule("unnecessary_final"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].range.start, at(&f, "final", 0));
}

/// A statement for-each nested in a collection for-element: only the
/// statement reports (the element's container is `ForControlFlow`) — the
/// oracle emits one row, at the inner keyword.
#[test]
fn uf3148_statement_for_each_inside_a_collection_element_reports_once() {
    let f = Fixture::parse(
        "void f(List<int> xs, List<int> ys) {\n  var l = [for (final x in xs) () { for (final y in ys) { print(y); } }];\n  print(l);\n}\n",
    );
    let lints = f.run(&one_rule("unnecessary_final"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].range.start, at(&f, "final", 1));
}
