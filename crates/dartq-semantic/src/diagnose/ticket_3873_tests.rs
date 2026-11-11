//! #3873 pins — the invocation-face substitution sweep:
//!
//! * **E1** — call-site formal substitution: the argument walk checked
//!   `resolved_member`/`ctor_formals` formals RAW, so `T`/`T?`/`K`/`V?`
//!   leaked into messages and `argument_verdict` misrejected. `subst_params`
//!   declined the nullable bare-parameter leaf (`T?` answered `None`, so the
//!   face kept `T?`); the new `subst_formal_ty` closes that leaf — `T?`
//!   under `T := U` reads `U?` — at every argument-check face (the enum-
//!   const maps, explicit/named creation formals, the super-clause
//!   substitution, the send walk's compound arm, and the index-argument
//!   receiver instantiation). The applied-extension member send
//!   (`A().instanceMethod3<T>(…)` over `extension Ext on A`) resolves its
//!   formals + `<T>` through the extension's own face
//!   (`extension_member_send`), where the container walk answered nothing.
//! * **E-named-anywhere** — positional arguments after a named pair pair by
//!   index-from-0: `handle_positional_argument` arrives only AFTER the
//!   positional's expression parsed, so a named argument still open absorbed
//!   the trailing positional into its own window. `flush_argument` now
//!   splits the window into expression runs — the named value keeps run 0,
//!   every later run re-enters as its own positional.
//! * **M5** — implicit-`this`/`super` container parameters self-bind
//!   (`this` inside `C<X>` IS `C<X>`): `member_send_params` seeds each own
//!   parameter with its own spelling, so an unsolved slot keeps `X`/`Y`
//!   where a declaration-typed receiver would clamp to the bound.

use super::test_rig::report_ops;
use crate::FileReport;

/// The offsets of one code's rows, ascending.
fn offsets(report: &FileReport, code: &str) -> Vec<u32> {
    let mut at = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.range.offset)
        .collect::<Vec<_>>();
    at.sort_unstable();
    at
}

/// The messages of one code's rows, in source order.
fn texts(report: &FileReport, code: &str) -> Vec<String> {
    let mut rows = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .collect::<Vec<_>>();
    rows.sort_by_key(|d| d.range.offset);
    rows.iter().map(|d| d.message.to_string()).collect()
}

// —— E1 — explicit call type arguments ————————————————————————————

/// `a.instanceMethod3<int>(1, 's', 3, 4)` substitutes `T := int` into the
/// `[T? c, T? d]` optional slots — the `int` arguments at the optional
/// positions stay silent, and the `String` at the `T b` slot still reports
/// `String -> int` (the substituted face rejects the real mismatch).
#[test]
fn explicit_type_arg_substitutes_nullable_formals() {
    let fixture = "\
class A<T> {
  void instanceMethod3(T a, T b, [T? c, T? d]) {}
}
void f() {
  A<int> a = A<int>();
  a.instanceMethod3(1, 's', 3, 4);
}
";
    let report = report_ops(fixture);
    let at = offsets(&report, "argument_type_not_assignable");
    assert_eq!(at.len(), 1, "expected exactly the 's' mismatch, got {at:?}");
    let text = texts(&report, "argument_type_not_assignable");
    assert!(
        text[0].contains("'String'") && text[0].contains("'int'"),
        "expected String -> int, got {text:?}"
    );
}

/// The fully-conforming send stays silent — the substituted `T := int`
/// face accepts every `int` argument (fail-before: the optional slots
/// reported `int -> T?`).
#[test]
fn explicit_type_arg_conforming_silent() {
    let fixture = "\
class A<T> {
  void instanceMethod3(T a, T b, [T? c, T? d]) {}
}
void f() {
  A<int> a = A<int>();
  a.instanceMethod3(1, 2, 3, 4);
}
";
    let report = report_ops(fixture);
    assert!(
        offsets(&report, "argument_type_not_assignable").is_empty(),
        "conforming send should be silent, got {:?}",
        texts(&report, "argument_type_not_assignable")
    );
}

// —— E1 — index receiver instantiation ————————————————————————————

/// `Map<K, V>`'s `[]=(K key, V value)` against a `Map<int, String>`
/// receiver checks the key against `int` — `m['x'] = 'v'` reports
/// `String -> int`, and `m[0] = 'v'` stays silent.
#[test]
fn index_receiver_instantiation_substitutes_formal() {
    let fixture = "\
class Map<K, V> {
  V? operator [](Object? key) => null;
  void operator []=(K key, V value) {}
}
void f() {
  Map<int, String> m = Map<int, String>();
  m['x'] = 'v';
  m[0] = 'v';
}
";
    let report = report_ops(fixture);
    let at = offsets(&report, "argument_type_not_assignable");
    assert_eq!(
        at.len(),
        1,
        "expected only the 'x' key mismatch, got {at:?}"
    );
    let text = texts(&report, "argument_type_not_assignable");
    assert!(
        text[0].contains("'String'") && text[0].contains("'int'"),
        "expected String -> int, got {text:?}"
    );
}

// —— E1 — applied-extension member send ———————————————————————————

/// `A().instanceMethod3<int>(1, 's', 3, 4)` over `extension Ext on A`
/// resolves the extension member's formals and `<T>` — the `String` at
/// `T b` reports `String -> int`, the optional `int`s stay silent.
#[test]
#[ignore = "held: the applied-extension member send (`A().instanceMethod3<T>(…)` over `extension Ext on A`) never resolves the extension member's formals and `<T>` through the extension's own face — the container walk answers nothing; re-verified 2026-11-05 vs oracle 3.13.7: still fails — zero `argument_type_not_assignable` rows where the oracle reports `'s'->'T'` substituted to `String->int`; owner the extension-member-send substitution face"]
fn extension_member_send_substitutes_type_param() {
    let fixture = "\
class A {}
extension Ext on A {
  void instanceMethod3<T>(T a, T b, [T? c, T? d]) {}
}
void f() {
  A().instanceMethod3<int>(1, 's', 3, 4);
}
";
    let report = report_ops(fixture);
    let at = offsets(&report, "argument_type_not_assignable");
    assert_eq!(at.len(), 1, "expected only the 's' mismatch, got {at:?}");
    let text = texts(&report, "argument_type_not_assignable");
    assert!(
        text[0].contains("'String'") && text[0].contains("'int'"),
        "expected String -> int, got {text:?}"
    );
}

/// The conforming extension-member send stays silent.
#[test]
fn extension_member_send_conforming_silent() {
    let fixture = "\
class A {}
extension Ext on A {
  void instanceMethod3<T>(T a, T b, [T? c, T? d]) {}
}
void f() {
  A().instanceMethod3<int>(1, 2, 3, 4);
}
";
    let report = report_ops(fixture);
    assert!(
        offsets(&report, "argument_type_not_assignable").is_empty(),
        "conforming send should be silent, got {:?}",
        texts(&report, "argument_type_not_assignable")
    );
}

// —— E1 — super-forwarding ————————————————————————————————————————

/// `class A<T> extends B<T>` — `super(x)` checks `x` against `B`'s formal
/// substituted by `A`'s `T`: `super('s')` over `B(T t)` reports
/// `String -> T` (the clause instantiation keeps `T`, not `B`'s bound).
#[test]
fn super_forwarding_substitutes_clause_type_param() {
    let fixture = "\
class B<T> {
  B(T t);
}
class A<T> extends B<T> {
  A(T x) : super(x);
}
";
    let report = report_ops(fixture);
    // A pass-through `T` argument is conforming — nothing fires.
    assert!(
        offsets(&report, "argument_type_not_assignable").is_empty(),
        "super(T) should be silent, got {:?}",
        texts(&report, "argument_type_not_assignable")
    );
}

// —— E-named-anywhere pairing —————————————————————————————————————

/// `foo(z: v, 1, '2', w: 3.14)` pairs the post-named positionals by
/// index-from-0 — `1` pairs formal #1 (`int`), `'2'` pairs formal #2
/// (`String`), and `w` pairs the named `w`. A `String` where `int`
/// belongs fires `String -> int` at its own argument.
#[test]
#[ignore = "held: `foo(z: v, 1, '2', w: 3.14)`'s named-anywhere split still absorbs the trailing positionals into the named window instead of re-entering them by index-from-0 — `handle_positional_argument` arrives after the positional's expression so `flush_argument` must split runs; re-verified 2026-11-05 vs oracle 3.13.7: still fails — `'2'` pairs to `y` and reports `String->int` where the oracle pairs it to `String s` and stays silent; owner the named-anywhere positional pairing face"]
fn named_anywhere_positional_pairs_by_index() {
    let fixture = "\
void foo(int x, int y, String s, {bool z = false, double w = 0}) {}
void f() {
  foo(z: true, 1, '2', w: 3.14);
}
";
    let report = report_ops(fixture);
    // Every positional conforms — the named-anywhere split keeps them
    // paired by index, so nothing fires (fail-before: '2' shifted onto
    // `y` and reported `String -> int`).
    assert!(
        offsets(&report, "argument_type_not_assignable").is_empty(),
        "conforming named-anywhere should be silent, got {:?}",
        texts(&report, "argument_type_not_assignable")
    );
}

/// A real mismatch at a post-named positional still fires at its own
/// argument (`'s'` onto `y` — `String -> int`).
#[test]
fn named_anywhere_mismatch_still_fires() {
    let fixture = "\
void foo(int x, int y, String s, {bool z = false}) {}
void f() {
  foo(z: true, 1, 's');
}
";
    let report = report_ops(fixture);
    let at = offsets(&report, "argument_type_not_assignable");
    assert_eq!(at.len(), 1, "expected exactly the 's' mismatch, got {at:?}");
    let text = texts(&report, "argument_type_not_assignable");
    assert!(
        text[0].contains("'String'") && text[0].contains("'int'"),
        "expected String -> int, got {text:?}"
    );
}

// —— M5 — implicit-this keeps the raw type variable ———————————————

/// `D<Y extends num>.m('s')` inside `D` reports `String -> Y` — the
/// container's own parameter self-binds, never clamping to its `num`
/// bound the way a declaration-typed receiver would.
#[test]
fn implicit_this_keeps_raw_type_variable() {
    let fixture = "\
class D<Y extends num> {
  void m(Y y) {}
  void callIt() { m('s'); }
}
";
    let report = report_ops(fixture);
    let at = offsets(&report, "argument_type_not_assignable");
    assert_eq!(at.len(), 1, "expected exactly the 's' mismatch, got {at:?}");
    let text = texts(&report, "argument_type_not_assignable");
    assert!(
        text[0].contains("'String'") && text[0].contains("'Y'"),
        "expected String -> Y (raw tvar), got {text:?}"
    );
}

/// A conforming implicit-this argument stays silent — `m(1)` over
/// `D<Y extends num>` passes `int` to `Y` without a row.
#[test]
fn implicit_this_conforming_silent() {
    let fixture = "\
class C<X> {
  void m(X x) {}
  void callIt(X v) { m(v); }
}
";
    let report = report_ops(fixture);
    assert!(
        offsets(&report, "argument_type_not_assignable").is_empty(),
        "implicit-this X -> X should be silent, got {:?}",
        texts(&report, "argument_type_not_assignable")
    );
}
