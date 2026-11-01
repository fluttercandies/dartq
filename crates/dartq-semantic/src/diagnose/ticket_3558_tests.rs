//! The #3558 pins — the two largest `unused_local_variable` fabrication
//! clusters of the corpus census (820 EXTRA rows, 191 MISSING).
//!
//! # Cluster A — the assignment-pattern binder face misfires (≈560 rows)
//!
//! `Engine::report_pattern_assignment_binders` re-derives pattern
//! assignments from the token stream: any balanced bracket group left of a
//! plain `=` counted as one unless the token right before the group was a
//! declaration keyword. Three non-assignment shapes slipped through and
//! each binder-looking formal or element staged a phantom unused local:
//!
//! - a redirecting factory's formal list (`factory A.f(int a) = A;`,
//!   `const factory`, `augment factory`, extension-type factories) — the
//!   head is the constructor name and the keyword sits before it;
//! - a declaration pattern with an object or typed-collection head
//!   (`var Square(…) = …`, `var <Object>[…] = …`) — the declaration
//!   keyword sits before the head, not before the bracket;
//! - a function-typed formal's default (`String this.field(int value) = g`).
//!
//! Upstream mints the binder element only for a real `PatternAssignment`
//! (the parser's `pattern_assignment_declares_variable` face), so none of
//! these report. The same face also anchored `final int a` on the type
//! token `int`; the oracle anchors the declared name.
//!
//! # Cluster B — case-head binders (≈90 rows)
//!
//! A logical-or pattern's same-named variables are one
//! `JoinPatternVariableElement` upstream: a read through any spelling —
//! the `when` guard included — marks every branch's binder used
//! (`UsedLocalElements.addElement`'s `transitiveVariables` expansion). A
//! repeated spelling inside one conjunction or collection is a duplicate
//! (`duplicate_variable_pattern`): the verifier skips it (`isDuplicate`)
//! and the read binds the first binder.
//!
//! Oracle provenance: `dart analyze --format=machine` 3.13.7, one probe
//! per cluster over the fixtures below (verbatim), 2026-11-02.

use super::test_rig::{CORE_PRELUDE, report_ops};

/// The `unused_local_variable` rows of `fixture`, as (offset, length).
fn local_rows(fixture: &str) -> Vec<(u32, u32)> {
    report_ops(fixture)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_local_variable")
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// The rig offset of `name`'s last occurrence inside the first `context`
/// occurrence: `report_ops` prefixes [`CORE_PRELUDE`], so every
/// fixture-local offset shifts by the prelude's length.
fn name_in(fixture: &str, context: &str, name: &str) -> (u32, u32) {
    let at = fixture
        .find(context)
        .unwrap_or_else(|| panic!("{context:?} not in fixture"));
    let inner = context
        .rfind(name)
        .unwrap_or_else(|| panic!("{name:?} not in {context:?}"));
    let offset = u32::try_from(CORE_PRELUDE.len() + at + inner).expect("fixture offsets fit u32");
    (offset, u32::try_from(name.len()).expect("short name"))
}

/// Cluster A: every redirecting-factory shape — named, optional
/// positional, optional named, `const`, unnamed, extension type. Before
/// the fix each formal staged `unused_local_variable` (8 rows).
///
/// Oracle 3.13.7 on this unit: no UNUSED_LOCAL_VARIABLE row (only
/// REDIRECT_TO_INVALID_FUNCTION_TYPE on `A.h`'s target).
#[test]
fn redirecting_factory_formals_stay_silent() {
    let fixture = "class A {\n\
                   \x20 A(this.a, [this.b = 0]);\n\
                   \x20 factory A.f(int a) = A;\n\
                   \x20 factory A.g(int a, [int b]) = A;\n\
                   \x20 factory A.h(int a, {int b}) = A;\n\
                   \x20 const factory A.k(int a) = B;\n\
                   \x20 int a;\n\
                   \x20 int b;\n\
                   }\n\
                   class B implements A {\n\
                   \x20 const B(this.a);\n\
                   \x20 final int a;\n\
                   \x20 int get b => 0;\n\
                   \x20 set a(int v) {}\n\
                   \x20 set b(int v) {}\n\
                   }\n\
                   abstract class C {\n\
                   \x20 factory C(int x) = D;\n\
                   }\n\
                   class D implements C {\n\
                   \x20 D(int x);\n\
                   }\n\
                   extension type ET(int id) {\n\
                   \x20 ET.foo(this.id);\n\
                   \x20 factory ET.bar(int id) = ET.foo;\n\
                   }\n";
    assert_eq!(local_rows(fixture), Vec::new(), "{fixture}");
}

/// Cluster A control: a genuinely unused local beside a redirecting
/// factory still reports, at its own name.
///
/// Oracle 3.13.7: UNUSED_LOCAL_VARIABLE on `unused` only.
#[test]
fn local_beside_redirecting_factory_still_reports() {
    let fixture = "class A {\n\
                   \x20 A(this.a);\n\
                   \x20 factory A.f(int a) = A;\n\
                   \x20 final int a;\n\
                   }\n\
                   void g() {\n\
                   \x20 var unused = 0;\n\
                   }\n";
    assert_eq!(
        local_rows(fixture),
        vec![name_in(fixture, "var unused", "unused")],
        "{fixture}"
    );
}

/// Cluster A: a declaration pattern whose head is an object pattern or a
/// typed collection binds through the declaration, not the assignment
/// face — every binder below is read, so nothing reports. Before the fix
/// `c1`, `d1`, `d2` and `c2` staged rows.
///
/// Oracle 3.13.7: no UNUSED_LOCAL_VARIABLE row.
#[test]
fn declaration_pattern_heads_stay_silent() {
    let fixture = "class Square {\n\
                   \x20 final int areaAsInt;\n\
                   \x20 Square(this.areaAsInt);\n\
                   }\n\
                   void sink(Object? o) {}\n\
                   void declarations() {\n\
                   \x20 var Square(areaAsInt: int c1) = Square(2);\n\
                   \x20 sink(c1);\n\
                   \x20 final Square(areaAsInt: int d1 && num d2) = Square(2);\n\
                   \x20 sink(d1);\n\
                   \x20 sink(d2);\n\
                   \x20 var <Object>[a1, Object c2] = [1, 2];\n\
                   \x20 sink(a1);\n\
                   \x20 sink(c2);\n\
                   }\n";
    assert_eq!(local_rows(fixture), Vec::new(), "{fixture}");
}

/// Cluster A control: the same declaration heads with binders nobody
/// reads still report every binder — through the declaration's
/// all-or-nothing pattern group, not the assignment face.
///
/// Oracle 3.13.7 (corpus expected rows of the same shapes —
/// co19 `Patterns/object_A03_t01` `var Rectangle(:int areaAsInt) = …`,
/// `Patterns/type_inference_A07_t01` `var <B>[A v1, …] = …`):
/// UNUSED_LOCAL_VARIABLE at each binder.
#[test]
fn unread_declaration_pattern_heads_still_report() {
    let fixture = "class Square {\n\
                   \x20 final int areaAsInt;\n\
                   \x20 Square(this.areaAsInt);\n\
                   }\n\
                   void declarations() {\n\
                   \x20 var Square(areaAsInt: int c1) = Square(2);\n\
                   \x20 final <Object>[Object c2] = [1];\n\
                   }\n";
    assert_eq!(
        local_rows(fixture),
        vec![
            name_in(fixture, "int c1", "c1"),
            name_in(fixture, "Object c2", "c2"),
        ],
        "{fixture}"
    );
}

/// Cluster A: a function-typed field formal's default value is not a
/// pattern assignment; its inner formal `value` is no local. Before the
/// fix `value` staged a row.
///
/// Oracle 3.13.7: FIELD_INITIALIZING_FORMAL_NOT_ASSIGNABLE only.
#[test]
fn function_typed_formal_default_stays_silent() {
    let fixture = "class F {\n\
                   \x20 String field;\n\
                   \x20 F({String this.field(int value) = g});\n\
                   }\n\
                   String g(int value) => '';\n";
    assert_eq!(local_rows(fixture), Vec::new(), "{fixture}");
}

/// Cluster A control and anchor: a real pattern assignment's declaring
/// binders still report, and a `final`/`var` head followed by a written
/// type anchors on the declared name, never on the type token.
///
/// Oracle 3.13.7: UNUSED_LOCAL_VARIABLE at `a` (in `(final int a)`), `b`
/// and `c` — beside PATTERN_ASSIGNMENT_DECLARES_VARIABLE on each.
#[test]
fn typed_binder_after_final_anchors_at_its_name() {
    let fixture = "void sink(Object? o) {}\n\
                   void assignments() {\n\
                   \x20 int a = 0;\n\
                   \x20 (final int a) = 42;\n\
                   \x20 [var b, final int c] = [1, 2];\n\
                   \x20 sink(a);\n\
                   }\n";
    assert_eq!(
        local_rows(fixture),
        vec![
            name_in(fixture, "(final int a)", "a"),
            name_in(fixture, "[var b", "b"),
            name_in(fixture, "final int c]", "c"),
        ],
        "{fixture}"
    );
}

/// Cluster B: a logical-or chain read only by its `when` guard (switch
/// statement, switch expression, if-case) spares every branch's binder.
/// Before the fix the left binders of `a`, `v6`, `v1` and `d` staged rows.
///
/// Oracle 3.13.7: no UNUSED_LOCAL_VARIABLE row.
#[test]
fn guarded_or_chain_binders_stay_silent() {
    let fixture = "String guarded(Object? x) {\n\
                   \x20 switch (x) {\n\
                   \x20   case int a || int a when a > 0:\n\
                   \x20     return 'a';\n\
                   \x20   case int v6 || [int v6] when v6 == 6:\n\
                   \x20     return 'v6';\n\
                   \x20   case [var v1] && [_] || var v1 when v1 == 1:\n\
                   \x20     return 'v1';\n\
                   \x20   default:\n\
                   \x20     return 'none';\n\
                   \x20 }\n\
                   }\n\
                   String expression(Object? x) => switch (x) {\n\
                   \x20 int d || [int d] when d == 1 => 'd',\n\
                   \x20 _ => 'none',\n\
                   };\n\
                   String ifCase(Object? x) {\n\
                   \x20 if (x case int c || [int c] when c == 1) return 'c';\n\
                   \x20 return 'none';\n\
                   }\n";
    assert_eq!(local_rows(fixture), Vec::new(), "{fixture}");
}

/// Cluster B: a repeated spelling inside one conjunction is a duplicate —
/// only the first binder carries the unused verdict, and a read of the
/// spelling spares it. Before the fix the duplicate `v` staged a second
/// row.
///
/// Oracle 3.13.7: UNUSED_LOCAL_VARIABLE at the first `v` only (plus
/// DUPLICATE_VARIABLE_PATTERN at both duplicates).
#[test]
fn duplicate_binder_reports_only_the_first() {
    let fixture = "String duplicate(Object? x) {\n\
                   \x20 switch (x) {\n\
                   \x20   case [var v] && [var v, ...]:\n\
                   \x20     return 'dup';\n\
                   \x20   default:\n\
                   \x20     return 'none';\n\
                   \x20 }\n\
                   }\n\
                   String duplicateRead(Object? x) {\n\
                   \x20 switch (x) {\n\
                   \x20   case [var w] && [var w, ...]:\n\
                   \x20     return '$w';\n\
                   \x20   default:\n\
                   \x20     return 'none';\n\
                   \x20 }\n\
                   }\n";
    assert_eq!(
        local_rows(fixture),
        vec![name_in(fixture, "[var v]", "v")],
        "{fixture}"
    );
}

/// Cluster B control: a logical-or chain nobody reads reports every
/// branch's binder.
///
/// Oracle 3.13.7: UNUSED_LOCAL_VARIABLE at both `e` binders.
#[test]
fn unread_or_chain_reports_every_binder() {
    let fixture = "String unread(Object? x) {\n\
                   \x20 switch (x) {\n\
                   \x20   case int e || [int e]:\n\
                   \x20     return 'e';\n\
                   \x20   default:\n\
                   \x20     return 'none';\n\
                   \x20 }\n\
                   }\n";
    assert_eq!(
        local_rows(fixture),
        vec![
            name_in(fixture, "case int e", "e"),
            name_in(fixture, "[int e]", "e"),
        ],
        "{fixture}"
    );
}
