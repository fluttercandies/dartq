//! #3420 pins — a member read off a created chain carries the declared
//! face, instantiated with the creation's written arguments.
//!
//! # The defect
//!
//! `Box<int>(1).slot` over `class Box<T> { final T? slot; }` reads `int?`
//! upstream: the creation's static type is the instantiated head, and the
//! property access resolves the field on that type. The member merge writes
//! the read's declared type for a `this`/`super`/`typed` receiver, and the
//! folded creation's receiver classifies as `Receiver::Created` — the arm
//! that holds only the RECEIVER'S STATIC TYPE — so the merge consulted the
//! record face (`Engine::record_field_ty`) for a class-typed receiver, the
//! consultation answered `None`, and the merged row published `Unknown`.
//! Every consumer of that row (the argument check above all) then stayed
//! silent, so the fixture produced no diagnostic at all.
//!
//! # The wiring
//!
//! [`Engine::record_field_ty`] now answers a NAMED receiver type's
//! declared accessor face as well: the member walk supplies the declared
//! type (`T?`) and [`Engine::instantiate_accessor_static`] substitutes the
//! receiver's written arguments (`int?`) — the same two steps the typed
//! receiver's arm runs at the merge. A receiver whose instantiation the
//! model cannot decide (no written arguments on a generic head, an arity
//! the declaration rejects) keeps `None`, so the raw `T?` schema never
//! leaks in as a read's static type.
//!
//! [`Engine::record_field_ty`]: super::Engine::record_field_ty
//! [`Engine::instantiate_accessor_static`]:
//!     super::Engine::instantiate_accessor_static

use super::test_rig::report_ops;
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

/// The unnamed creation's written instantiation reaches the slot read: a
/// `T?` field read off `Box<int>(1)` is `int?`, so handing it to an `int`
/// parameter reports the argument mismatch — the row the `Unknown` row
/// suppressed.
#[test]
fn created_chain_slot_read_carries_the_written_instantiation() {
    let source = r#"
class Box<T> {
  Box(this.slot);
  final T? slot;
}
void takesInt(int x) {}
void f() {
  takesInt(Box<int>(1).slot);
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "argument_type_not_assignable");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    let message = &rows[0].2;
    assert!(
        message.contains("'int?'") && message.contains("'int'"),
        "{message}"
    );
}

/// The named-constructor twin (the spelling #3412 taught the send result):
/// `Box<int>.make(1)` types the created instance `Box<int>`, so its `T?`
/// slot reads `int?` and reaches the same argument row.
#[test]
fn named_creation_chain_slot_read_carries_the_instantiation() {
    let source = r#"
class Box<T> {
  Box.make(int n) : slot = null;
  final T? slot;
}
void takesInt(int x) {}
void f() {
  takesInt(Box<int>.make(1).slot);
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "argument_type_not_assignable");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    let message = &rows[0].2;
    assert!(
        message.contains("'int?'") && message.contains("'int'"),
        "{message}"
    );
}

/// The substitution reads the receiver's OWN argument, not a fixed face: a
/// `Box<String>` slot reads `String?`, so the refusal names `String?` — a
/// raw `T?` leak (or a `dynamic` fill) would name neither.
#[test]
fn written_instantiation_substitutes_the_receiver_argument() {
    let source = r#"
class Box<T> {
  Box(this.slot);
  final T? slot;
}
void takesInt(int x) {}
void f() {
  takesInt(Box<String>('x').slot);
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "argument_type_not_assignable");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    let message = &rows[0].2;
    assert!(
        message.contains("'String?'") && message.contains("'int'"),
        "{message}"
    );
}

/// The agreeing sink: `int?` reaches the `int?` parameter, so the read now
/// types without staging anything.
#[test]
fn agreeing_nullable_sink_stays_silent() {
    let source = r#"
class Box<T> {
  Box(this.slot);
  final T? slot;
}
void takesNullable(int? x) {}
void f() {
  takesNullable(Box<int>(1).slot);
}
"#;
    let report = report_ops(source);
    assert!(
        rows_of(&report, "argument_type_not_assignable").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

/// The declared getter face is the same one: `T? get maybe` read off the
/// created chain answers the instantiated return.
#[test]
fn created_chain_getter_read_carries_the_instantiation() {
    let source = r#"
class Box<T> {
  Box(this.slot);
  final T? slot;
  T? get maybe => slot;
}
void takesInt(int x) {}
void f() {
  takesInt(Box<int>(1).maybe);
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "argument_type_not_assignable");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    let message = &rows[0].2;
    assert!(
        message.contains("'int?'") && message.contains("'int'"),
        "{message}"
    );
}

/// The non-generic control: a plain class's created chain keeps its own
/// declared face — an `int` slot reaches an `int` parameter silently, so
/// the new face never widens a non-parameterized read.
#[test]
fn non_generic_created_chain_keeps_its_declared_face() {
    let source = r#"
class IntBox {
  IntBox(this.slot);
  final int slot;
}
void takesInt(int x) {}
void f() {
  takesInt(IntBox(1).slot);
}
"#;
    let report = report_ops(source);
    assert!(
        rows_of(&report, "argument_type_not_assignable").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

/// The top face: `dynamic` is assignable to everything upstream, so the
/// `Box<dynamic>` slot stages no row — and the `?` leaf the substitution
/// cannot lift keeps the read at the under-report instead of inventing a
/// face. The pin holds on both halves: the row count is zero whether the
/// read types as `dynamic` or stays `Unknown`.
#[test]
fn dynamic_instantiation_stages_no_row() {
    let source = r#"
class Box<T> {
  Box(this.slot);
  final T? slot;
}
void takesInt(int x) {}
void f() {
  takesInt(Box<dynamic>(1).slot);
}
"#;
    let report = report_ops(source);
    assert!(
        rows_of(&report, "argument_type_not_assignable").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}
