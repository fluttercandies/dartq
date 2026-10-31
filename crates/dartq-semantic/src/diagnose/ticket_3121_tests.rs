//! #3121 — `return_of_invalid_type` verdict pins.
//!
//! The conformance cluster's verdict roots all landed in this ticket
//! (upstream Dart SDK source, pkg/analyzer/lib/src/error/
//! return_type_verifier.dart — `rtv.dart` below):
//!
//! * **Sync void-source arm** (`return_verdict`, diagnose/assignability.rs):
//!   `return v;` of static type `void` is clean only into `void`/`dynamic`
//!   hosts — rtv.dart:222-226's `_isVoidDynamic(T)` guard. The arm was
//!   inverted (reported exactly the clean pairs, silenced the real ones).
//! * **Async flatten leg** (`async_return_verdict`): the report needs BOTH
//!   `!isAssignableTo(S, T_v)` AND `!isSubtypeOf(flatten(S), T_v)`
//!   (rtv.dart:274-289, `flatten` at type_system.dart:425-471) — a
//!   `Future<int>` expression in a `Future<int>` host is clean through the
//!   subtype leg, and an undecidable direct ask still reports when the
//!   declared hierarchy proves the pair disjoint.
//! * **`await` row anchor** (`check_return`): the oracle's
//!   `.at(expression)` is the AwaitExpression — the `await` keyword
//!   through the operand's end (rtv.dart:155-205) — not the operand row's
//!   own span; nested `await await e` anchors at the outermost keyword.
//! * **Operator head** (`C::OperatorName`, diagnose/listener.rs): an
//!   operator is a `MethodElement`, so its return check latches the armed
//!   annotation under the `method` host word — `int operator >(C o) =>
//!   "s";` reports `... from the method '>'`.
//! * **Constructor faces** (`latch_factory_return`, scopes_bodies.rs +
//!   the `ctor_head` arm of `C::MethodDeclaration`): factory AND
//!   generative heads latch `ConstructorElement.returnType` — the
//!   enclosing class's `thisType` (element.dart:1004), spelling `A<T>`
//!   for a generic container — under the `constructor` host word.

use super::test_rig::report_ops;
use super::*;

fn rows_named<'d>(report: &'d FileReport, name: &str) -> Vec<&'d Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .collect()
}

fn roit(report: &FileReport) -> Vec<&Diagnostic> {
    rows_named(report, "return_of_invalid_type")
}

// —— Sync matrix ————————————————————————————————————————————————————————

/// The textbook row: a `String` operand against an `int` host reports at
/// the operand under the function host word (rtv.dart:209-249 — the
/// non-async matrix asks the one `isAssignableTo` question).
#[test]
fn string_operand_against_int_host_reports() {
    let fixture = "\
int f() { return \"s\"; }
";
    let report = report_ops(fixture);
    let rows = roit(&report);
    assert_eq!(
        rows.len(),
        1,
        "one row: {:?}",
        report.diagnostics.iter().map(|d| (d.code.name, d.range)).collect::<Vec<_>>()
    );
    // The report's offset rides the rig's prepended prelude — the
    // stable face is the operand's own LENGTH (`"s"` is 3 columns).
    assert_eq!(rows[0].range.length, 3);
    assert!(rows[0].message.contains("from the function 'f'"));
    assert!(rows[0].message.contains("return type of 'int'"));
}

/// A value return into a `void` host reports (rtv.dart:222-226's (b) arm
/// — `T` is not `void`/`dynamic`-satisfying for an `int` source).
#[test]
fn int_operand_against_void_host_reports() {
    let fixture = "\
void vf() { return 0; }
";
    let report = report_ops(fixture);
    let rows = roit(&report);
    assert_eq!(
        rows.len(),
        1,
        "the void host rejects an int value: {:?}",
        report.diagnostics.iter().map(|d| (d.code.name, d.range)).collect::<Vec<_>>()
    );
    assert!(rows[0].message.contains("return type of 'void'"));
}

/// The void-source arm's clean faces: a `void`-typed expression returns
/// cleanly into `void` and `dynamic` hosts (rtv.dart:222-226 —
/// `_isVoidDynamic`). This is the inverted arm's regression pin: it used
/// to report exactly these pairs.
#[test]
fn void_expression_into_void_and_dynamic_hosts_silent() {
    let fixture = "\
void vvoid() {}
void dret() { return vvoid(); }
dynamic dd() { return vvoid(); }
";
    let report = report_ops(fixture);
    assert!(
        roit(&report).is_empty(),
        "void-expression returns into void/dynamic hosts are clean: {:?}",
        report.diagnostics.iter().map(|d| (d.code.name, d.range)).collect::<Vec<_>>()
    );
}

/// The void-source arm's reporting face: a `void` expression into an
/// `int` host reports (the arm answers true only for `void`/`dynamic`).
#[test]
fn void_expression_into_int_host_reports() {
    let fixture = "\
void vvoid() {}
int bad() { return vvoid(); }
";
    let report = report_ops(fixture);
    let rows = roit(&report);
    assert_eq!(
        rows.len(),
        1,
        "void into int reports: {:?}",
        report.diagnostics.iter().map(|d| (d.code.name, d.range)).collect::<Vec<_>>()
    );
}

/// A declared type-parameter target asks `S <: T` — every interface
/// source rejects (subtype.dart's Left Variable arm via rtv.dart's
/// `isAssignableTo`).
#[test]
fn interface_operand_against_type_parameter_target_reports() {
    let fixture = "\
T gen<T>(T x) { return 3; }
";
    let report = report_ops(fixture);
    let rows = roit(&report);
    assert_eq!(
        rows.len(),
        1,
        "int into T reports: {:?}",
        report.diagnostics.iter().map(|d| (d.code.name, d.range)).collect::<Vec<_>>()
    );
    assert!(rows[0].message.contains("return type of 'T'"));
}

// —— Async matrix (the flatten divergence) ——————————————————————————————

/// The async direct-failure row: `String` is neither assignable to `int`
/// (`T_v` of `Future<int>`) nor flattens to a subtype of it — the
/// message keeps the RAW `S` and the declared `T` (rtv.dart:155-205 —
/// the projections are verdict-side only).
#[test]
fn async_string_operand_against_future_int_reports_declared_face() {
    let fixture = "\
Future<int> h() async { return \"s\"; }
";
    let report = report_ops(fixture);
    let rows = roit(&report);
    assert_eq!(
        rows.len(),
        1,
        "one row: {:?}",
        report.diagnostics.iter().map(|d| (d.code.name, d.range)).collect::<Vec<_>>()
    );
    assert!(
        rows[0].message.contains("return type of 'Future<int>'"),
        "the message renders the declared face, not the flattened T_v: {:?}",
        rows[0].message
    );
}

/// The flatten subtype leg: a `Future<int>` expression in a `Future<int>`
/// host is clean even though `Future<int>` never assigns to `int` —
/// `flatten(Future<int>)` is `int`, a subtype of `T_v` (rtv.dart:274-289,
/// type_system.dart:425-471). THE divergence pin.
#[test]
fn async_future_operand_clean_through_flatten_subtype_leg() {
    let fixture = "\
Future<int> fut() async => 0;
Future<int> fi() async { return fut(); }
";
    let report = report_ops(fixture);
    assert!(
        roit(&report).is_empty(),
        "Future<int> into Future<int> is clean through the flatten leg: {:?}",
        report.diagnostics.iter().map(|d| (d.code.name, d.range)).collect::<Vec<_>>()
    );
}

/// The declared-negative leg: a `Future<String>` expression against a
/// `Future<int>` host — the direct ask is undecidable on the name-level
/// matrix but the closed hierarchy proves `String` fits no `int` face
/// (the flattened `String <: int` fails too), so the oracle reports.
#[test]
fn async_future_string_against_future_int_reports() {
    let fixture = "\
Future<String> fs() async => \"s\";
Future<int> fi() async { return fs(); }
";
    let report = report_ops(fixture);
    let rows = roit(&report);
    assert_eq!(
        rows.len(),
        1,
        "Future<String> into Future<int> reports: {:?}",
        report.diagnostics.iter().map(|d| (d.code.name, d.range)).collect::<Vec<_>>()
    );
}

// —— `await` anchor —————————————————————————————————————————————————————

/// The oracle's `.at(expression)` is the AwaitExpression: the row starts
/// at the `await` keyword and runs through the operand's end
/// (rtv.dart:155-205), not at the operand's own span.
#[test]
fn await_return_row_anchors_at_keyword() {
    // The fixture's `await` sits 6 columns after `return`; the row's span
    // backs up to the keyword (operand start − 5) and runs through the
    // operand's own end — `await s()` is 8 columns. The operand's type is
    // a closed-declared `String` (the rig prelude's real class) so the
    // verdict is decidable without the `Object` source arm.
    let fixture = "String s() => \"x\";\nFuture<int> aw() async { return await s(); }\n";
    let report = report_ops(fixture);
    let rows = roit(&report);
    assert_eq!(
        rows.len(),
        1,
        "one row: {:?}",
        report.diagnostics.iter().map(|d| (d.code.name, d.range)).collect::<Vec<_>>()
    );
    assert_eq!(
        rows[0].range.length, 8,
        "the row spans `await s()` — keyword through operand end"
    );
}

// —— Operator / constructor faces ———————————————————————————————————————

/// An operator is a `MethodElement` — its return check latches the armed
/// annotation under the `method` host word (`int operator >(C o) => "s";`
/// reports `... from the method '>'`, oracle-verified).
#[test]
fn operator_return_reports_method_host_word() {
    let fixture = "\
class C {
  int operator >(C o) => \"s\";
}
";
    let report = report_ops(fixture);
    let rows = roit(&report);
    assert_eq!(
        rows.len(),
        1,
        "the operator's return reports: {:?}",
        report.diagnostics.iter().map(|d| (d.code.name, d.range)).collect::<Vec<_>>()
    );
    assert!(rows[0].message.contains("from the method '>'"));
    assert!(rows[0].message.contains("return type of 'int'"));
}

/// A named factory head's return check is the constructor face:
/// `factory C.f() => 3` reports `int` can't be returned from the
/// constructor 'C.f' — `ConstructorElement.returnType` is the enclosing
/// `thisType` (element.dart:1004) and `displayName` qualifies the
/// selector.
#[test]
fn named_factory_reports_constructor_host_and_qualified_display() {
    let fixture = "\
class C {
  factory C.f() => 3;
}
";
    let report = report_ops(fixture);
    let rows = roit(&report);
    assert_eq!(
        rows.len(),
        1,
        "the factory's return reports: {:?}",
        report.diagnostics.iter().map(|d| (d.code.name, d.range)).collect::<Vec<_>>()
    );
    assert!(rows[0].message.contains("from the constructor 'C.f'"));
    assert!(rows[0].message.contains("return type of 'C'"));
}

/// A generic container's factory spells `A<T>` — the constructor
/// element's return type is `enclosingElement.thisType` with the
/// DECLARED parameters (element.dart:1004), never the bare head and
/// never instantiate-to-bounds `A<dynamic>`.
#[test]
fn generic_factory_display_spells_declared_parameters() {
    let fixture = "\
class A<T> {
  factory A.g() => 0;
}
";
    let report = report_ops(fixture);
    let rows = roit(&report);
    assert_eq!(
        rows.len(),
        1,
        "the generic factory's return reports: {:?}",
        report.diagnostics.iter().map(|d| (d.code.name, d.range)).collect::<Vec<_>>()
    );
    assert!(
        rows[0].message.contains("return type of 'A<T>'"),
        "the declared-parameter face renders: {:?}",
        rows[0].message
    );
}

/// A generative `=>` constructor runs the same return check under the
/// `constructor` host word: `C() => 4` reports `int` against `C`
/// (oracle-verified — the `return_in_generative_constructor` row is a
/// separate code and not pinned here).
#[test]
fn generative_arrow_constructor_reports_constructor_host() {
    let fixture = "\
class C {
  C() => 4;
}
";
    let report = report_ops(fixture);
    let rows = roit(&report);
    assert_eq!(
        rows.len(),
        1,
        "the generative arrow's return reports: {:?}",
        report.diagnostics.iter().map(|d| (d.code.name, d.range)).collect::<Vec<_>>()
    );
    assert!(rows[0].message.contains("from the constructor 'C'"));
}

/// The operator backfill's honesty bound: an UNTYPED executable's
/// `dynamic` return must survive the typed locals its body completes —
/// `foo() { int x = 0; return "s"; }` stays silent (the `int` lands
/// after `body_mark`, outside the annotation window), and an
/// annotation-free setter reports no `non_void_return_for_setter`
/// (oracle-verified silent pair).
#[test]
fn untyped_executables_never_backfill_the_dynamic_return() {
    let fixture = "\
foo() { int x = 0; return \"s\"; }
class C { set s(int v) {} }
";
    let report = report_ops(fixture);
    assert!(
        roit(&report).is_empty()
            && rows_named(&report, "non_void_return_for_setter").is_empty(),
        "untyped executables stay silent: {:?}",
        report.diagnostics.iter().map(|d| (d.code.name, d.range)).collect::<Vec<_>>()
    );
}

/// The named generative face: `C.named() => null` reports `Null` can't
/// be returned from the constructor 'C.named' — the selector joins the
/// display at the continuation event (oracle-verified).
#[test]
fn named_generative_arrow_constructor_reports_qualified_display() {
    let fixture = "\
class C {
  C.named() => null;
}
";
    let report = report_ops(fixture);
    let rows = roit(&report);
    assert_eq!(
        rows.len(),
        1,
        "the named generative's return reports: {:?}",
        report.diagnostics.iter().map(|d| (d.code.name, d.range)).collect::<Vec<_>>()
    );
    assert!(rows[0].message.contains("from the constructor 'C.named'"));
    assert!(rows[0].message.contains("'Null'"));
}
