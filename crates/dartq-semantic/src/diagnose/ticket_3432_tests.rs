//! #3432 pins — the named mixin application's FORWARDED constructor face
//! (`class X = S with M;`).
//!
//! The application declares no constructor of its own: the analyzer
//! synthesizes one per GENERATIVE constructor of `S` (upstream Dart SDK
//! source, `pkg/analyzer/lib/src/dart/element/inheritance_manager3.dart`'s
//! `getMember` superclass climb — every generative `S.c` is callable as
//! `X.c`, factories never forward). The container model's own
//! `constructors`/`has_unnamed_constructor` row is the source-declared
//! face (always empty + the implicit default), so
//! [`Engine::effective_ctor_container`] walks the superclass chain to the
//! terminal class whose tables answer the face; `decl_kinds`'s
//! `is_mixin_application` row is the only marker telling the sugar form
//! (`class X = S with M;`) from a real `class X extends S with M {}`,
//! whose constructors are NOT forwarded.
//!
//! Two residual row families wait on lanes outside `invocation.rs` and
//! ride `#[ignore]`d pins below: the `undefined_method` emit verdict reads
//! `resolved.constructors` directly in `row_resolution.rs`'s
//! `container_member_verdict`, and the const faces read
//! `has_const_constructor`/`constructors` in `const_eval_faces.rs` — both
//! clear once their owners route the reads through
//! `effective_ctor_container`.

use super::test_rig::{RigImport, assert_silent, report_ops, report_ops_with};
use super::*;

/// Every `code` row of `report` as `(offset, length, message)`.
fn rows_of(report: &FileReport, code: &str) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

fn count(report: &FileReport, code: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .count()
}

fn names(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

/// The ticket's own shape: a generic superclass's named generative
/// constructor is callable through the application — the created value
/// types the cascade member (`check` resolves on the `A1` instance
/// through `M1`, never the `Type` literal the unclassified head used to
/// answer). The `b1` member-send row itself rides the
/// `row_resolution.rs` emit gate, so the pin asserts the CREATED face:
/// no row may name `check`.
#[test]
fn named_ctor_forwards_through_mixin_application() {
    let report = report_ops(
        "class B<T> { B.b1(T t); } \
         mixin M1 { void check(int x, Object y) {} } \
         class A1 = B<int> with M1; \
         void f() { A1.b1(1).check(1, 2); A1.b1(1)..check(1, 2); }",
    );
    assert!(
        rows_of(&report, "undefined_method")
            .iter()
            .all(|row| !row.2.contains("'check'")),
        "the created instance must resolve `check`: {:?}",
        names(&report)
    );
}

/// The unnamed face forwards the same way: `A1(1, 2)` pairs `B`'s
/// unnamed formals — a deficit on the required positional reports the
/// count face (the application's own `ctor_formals` row is empty and
/// used to swallow it).
#[test]
fn unnamed_ctor_forwards_through_mixin_application() {
    let report = report_ops(
        "class B<T> { const B(T x, [Object y = 0]); } \
         mixin M1 {} \
         class A1 = B<int> with M1; \
         void f() { A1(); }",
    );
    let rows = rows_of(&report, "not_enough_positional_arguments");
    assert_eq!(rows.len(), 1, "rows: {:?}", names(&report));
}

/// `const` unnamed-creation argument pairing reads the forwarded table
/// the same way (`const A1()` misses `B`'s required `T x`).
#[test]
fn explicit_creation_reads_forwarded_formals() {
    let report = report_ops(
        "class B<T> { const B(T t); } \
         mixin M1 {} \
         class A1 = B<int> with M1; \
         void f() { const A1(); }",
    );
    let rows = rows_of(&report, "not_enough_positional_arguments");
    assert_eq!(rows.len(), 1, "rows: {:?}", names(&report));
}

/// The `.new` selector spelling reaches the forwarded unnamed face
/// (`A1.new(1)` is a creation, so the selector carries no member miss
/// and the created value types the next send).
#[test]
fn new_selector_forwards_through_mixin_application() {
    let report = report_ops(
        "class B<T> { B(T t); } \
         mixin M1 { void check(int x, Object y) {} } \
         class A1 = B<int> with M1; \
         void f() { A1.new(1).check(1, 2); }",
    );
    assert!(
        rows_of(&report, "undefined_method")
            .iter()
            .all(|row| !row.2.contains("'check'")),
        "the created instance must resolve `check`: {:?}",
        names(&report)
    );
}

/// A name on neither the superclass nor the mixin still reports — the
/// forwarded set is the superclass's constructors exactly.
#[test]
fn missing_ctor_on_mixin_application_reports() {
    let report = report_ops(
        "class B<T> { B.b1(T t); } \
         mixin M1 {} \
         class A1 = B<int> with M1; \
         void f() { A1.gone(1); }",
    );
    assert_eq!(
        count(&report, "undefined_method"),
        1,
        "rows: {:?}",
        names(&report)
    );
}

/// A real `class X extends S with M {}` is NOT the sugar form: its
/// superclass's constructors never forward — `X.named()` reports exactly
/// as before.
#[test]
fn extends_with_class_does_not_forward() {
    let report = report_ops(
        "class S { S.named(); } \
         mixin M {} \
         class X extends S with M {} \
         void f() { X.named(); }",
    );
    assert_eq!(
        count(&report, "undefined_method"),
        1,
        "rows: {:?}",
        names(&report)
    );
}

/// Chained applications: `class Y = X with N` over the mixin application
/// `X` forwards `X`'s own forwarded set — `S.named` reaches `Y`, and the
/// created `Y` instance types the cascade member.
#[test]
fn chained_mixin_applications_forward() {
    let report = report_ops(
        "class S { S.named(); } \
         mixin M {} \
         mixin N { void check() {} } \
         class X = S with M; \
         class Y = X with N; \
         void f() { Y.named().check(); }",
    );
    assert!(
        rows_of(&report, "undefined_method")
            .iter()
            .all(|row| !row.2.contains("'check'")),
        "the created instance must resolve `check`: {:?}",
        names(&report)
    );
}

/// Factories never forward: `factory S.make` is no constructor of `X`,
/// and the selector reports through the ordinary member miss.
#[test]
fn factory_ctor_does_not_forward() {
    let report = report_ops(
        "class S { factory S.make() => S._(); S._(); } \
         mixin M {} \
         class X = S with M; \
         void f() { X.make(); }",
    );
    assert_eq!(
        count(&report, "undefined_method"),
        1,
        "rows: {:?}",
        names(&report)
    );
}

/// A factory-only unnamed face is not a generative unnamed constructor:
/// `class S { factory S() => S._(); S._(); }` forwards NOTHING unnamed,
/// so `X()` reports `new_with_undefined_constructor_default` like a
/// superclass with no unnamed generative face would.
#[test]
fn factory_only_unnamed_does_not_forward() {
    let report = report_ops(
        "class S { factory S() => S._(); S._(); } \
         mixin M {} \
         class X = S with M; \
         void f() { X(); }",
    );
    assert_eq!(
        count(&report, "new_with_undefined_constructor_default"),
        1,
        "rows: {:?}",
        names(&report)
    );
}

/// A named-only superclass forwards no unnamed face: `X()` reports the
/// default-constructor miss (the application's spurious
/// `has_unnamed_constructor` row used to swallow it).
#[test]
fn named_only_super_leaves_unnamed_face_absent() {
    let report = report_ops(
        "class S { S.named(); } \
         mixin M {} \
         class X = S with M; \
         void f() { X(); }",
    );
    assert_eq!(
        count(&report, "new_with_undefined_constructor_default"),
        1,
        "rows: {:?}",
        names(&report)
    );
}

/// `super.t1` through a mixin-application superclass resolves the
/// forwarded selector (`class S = T with M;` declares `T.t1`), and a
/// selector the terminal class lacks still reports.
#[test]
fn super_initializer_through_mixin_application_superclass() {
    let report = report_ops(
        "class T { T.t1(int x); } \
         mixin M {} \
         class S = T with M; \
         class X extends S { X.n() : super.t1(1); }",
    );
    assert_silent(&report);
}

#[test]
fn super_initializer_miss_through_mixin_application_superclass() {
    let report = report_ops(
        "class T { T.t1(int x); } \
         mixin M {} \
         class S = T with M; \
         class X extends S { X.n() : super.gone(1); }",
    );
    assert_eq!(
        count(&report, "undefined_constructor_in_initializer"),
        1,
        "rows: {:?}",
        names(&report)
    );
}

/// The cross-file shape (`P1 = B2 with M2`): the application's
/// `decl_kinds` row lives in its own unit while the superclass resolves
/// through the import graph — the forwarded unnamed face still answers
/// (here: its absence, `B2` declaring `B2.named` only).
#[test]
fn cross_file_mixin_application_forwards() {
    // No core prelude: the mounted import directive must stay the first
    // directive of the analyzed unit.
    let report = report_ops_with(
        "import 'package:t/helper.dart';\n\
         class P1 = B2 with M2;\n\
         mixin M2 {}\n\
         void f() { P1(); }\n",
        &[RigImport {
            uri: "package:t/helper.dart",
            prefix: None,
            source: "class B2 { B2.named(); }",
        }],
        None,
    );
    assert_eq!(
        count(&report, "new_with_undefined_constructor_default"),
        1,
        "rows: {:?}",
        names(&report)
    );
}

/// The `.b1` member-send row: `container_member_verdict` in
/// `row_resolution.rs` reads `resolved.constructors` directly, outside
/// this lane — the pin holds the oracle's final answer against the day
/// that read joins `ctor_face_exists`'s effective face.
#[test]
#[ignore = "row_resolution.rs container_member_verdict reads resolved.constructors directly; waits on that lane routing through the forwarded face"]
fn named_ctor_member_send_is_silent() {
    let report = report_ops(
        "class B<T> { B.b1(T t); } \
         mixin M1 {} \
         class A1 = B<int> with M1; \
         void f() { A1.b1(1); }",
    );
    assert_silent(&report);
}

/// The `const` creation faces read `has_const_constructor` and
/// `constructors` in `const_eval_faces.rs` (`const_ctor_target`,
/// `const_creation_valid`, `const_non_const_ctor_answered`) — the
/// application's own flags say `false`/empty, so the rows fire until
/// that lane reads the effective face.
#[test]
#[ignore = "const_eval_faces.rs reads decl.has_const_constructor/constructors directly; waits on that lane routing through the forwarded face"]
fn const_creation_through_forwarded_ctor_is_silent() {
    let report = report_ops(
        "class B<T> { const B.b1(T x, {Object y = 0}); } \
         mixin M1 {} \
         class A1 = B<int> with M1; \
         const a = A1.b1(1, y: 2); \
         void f() { const A1.b1(1, y: 2); }",
    );
    assert_silent(&report);
}
