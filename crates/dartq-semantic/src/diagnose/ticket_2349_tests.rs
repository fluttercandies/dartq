//! The #2349 pins — the numeric refinement's operand face at its error-
//! recovery edges, and the numeric CALL refinement's literal receiver.
//!
//! Upstream decides the node through `_refineNumericInvocationTypeNullSafe`
//! (`pkg/analyzer/lib/src/dart/element/type_system.dart:1983-2046`): for
//! `+ - * % remainder` on a `T <: num?` receiver it answers `double` when
//! `T <: double?` or `S <: double?` (S non-Never), `int` when both read
//! `int?`, `num` otherwise — and `InvalidType` is a subtype of everything
//! (`subtype.dart`'s `InvalidTypeImpl` arm), so an undefined operand
//! decides the `double` branch. Oracle probes (Dart 3.13.1, `dart analyze
//! --format machine`):
//!
//! - `int i; int x = i + noSuchName;` reports `double`→`int` plus
//!   `undefined_identifier` (S = InvalidType → `double`).
//! - `int i; dynamic d; int y = i + d;` reports `num`→`int`.
//! - `int z = noSuchThing + 1;` reports ONLY `undefined_identifier` — the
//!   receiver is InvalidType, no operator resolves, the node is silent.
//! - `void f<T extends num>(int i, T t) { int x = i + t; }` reports
//!   `num`→`int`; `S extends int` answers `int` (silent); `U extends
//!   double` answers `double`. The parameter's interface IS its bound's.
//! - `int a = 3.clamp(0, i);` is silent (`int.clamp` over two ints reads
//!   `int`); `3.5.clamp(0, i)` reads `num`; `3.remainder(i + 1)` reads
//!   `int`. Literal receivers carry their own primitive.
//!
//! The engine's `Ty::Unknown` still conflates the oracle's InvalidType
//! with the model's own typing gaps, so these pins read the SINK row
//! (`invalid_assignment`), which renders the folded node type — exactly
//! the face this ticket owns. The rig's prelude declares `class num {}`
//! without an arithmetic face, so a `num` receiver also stages the member
//! walk's `undefined_operator` artifact; the pins filter to the sink.

use super::test_rig::report_ops;
use super::*;

/// The folded node's face as the `int` sink renders it.
fn sink_messages(folded: &FileReport) -> Vec<String> {
    folded
        .diagnostics
        .iter()
        .filter(|diag| diag.code.name == "invalid_assignment")
        .map(|diag| diag.message.to_string())
        .collect()
}

/// `S = InvalidType` decides the `double` branch (`int - <undefined>` is
/// `double` in the oracle), so the `int` sink reports the `double` face.
#[test]
fn invalid_operand_releases_to_double() {
    let folded = report_ops(
        "void f(int i) {\n\
           int x = i + noSuchName;\n\
           print(x);\n\
         }\n",
    );
    let messages = sink_messages(&folded);
    assert_eq!(messages.len(), 1, "one sink row: {messages:?}");
    assert!(
        messages[0].contains("'double'"),
        "S = InvalidType answers double: {:?}",
        messages[0]
    );
}

/// `S = dynamic` is Left Top: neither the `double?` nor the `int?` arm
/// fires and the node answers `num` (`int + dynamic` is `num`).
#[test]
fn dynamic_operand_reads_num() {
    let folded = report_ops(
        "void f(int i, dynamic d) {\n\
           int y = i + d;\n\
           print(y);\n",
    );
    let messages = sink_messages(&folded);
    assert_eq!(messages.len(), 1, "one sink row: {messages:?}");
    assert!(
        messages[0].contains("'num'"),
        "S = dynamic answers num: {:?}",
        messages[0]
    );
}

/// An InvalidType RECEIVER never resolves an operator, so the node reports
/// nothing of its own — only the receiver's `undefined_identifier`.
#[test]
fn invalid_receiver_stays_silent() {
    let folded = report_ops(
        "void f() {\n\
           int z = noSuchThing + 1;\n\
           print(z);\n\
         }\n",
    );
    assert!(
        sink_messages(&folded).is_empty(),
        "an invalid receiver folds nothing: {:#?}",
        folded.diagnostics
    );
    let undefined = folded
        .diagnostics
        .iter()
        .filter(|diag| diag.code.name == "undefined_identifier")
        .count();
    assert_eq!(
        undefined, 1,
        "the receiver reports on its own: {:#?}",
        folded.diagnostics
    );
}

/// A declared `int` operand keeps the `int` arm — the blanket
/// `Unknown -> double` release this ticket replaced turned every
/// `int + <unmodeled read>` into a spurious `double`.
#[test]
fn declared_int_operand_keeps_int() {
    let folded = report_ops(
        "void f(int i, int j) {\n\
           int v = i + j;\n\
           print(v);\n",
    );
    assert!(
        sink_messages(&folded).is_empty(),
        "`int + int` is `int`: {:#?}",
        folded.diagnostics
    );
}

/// A bounded type parameter reads its BOUND (`T extends num` is `num`,
/// `S extends int` is `int`, `U extends double` is `double`): the oracle
/// resolves the operator and refines on the parameter's interface.
#[test]
fn bounded_parameter_operand_reads_its_bound() {
    let folded = report_ops(
        "void f<T extends num, S extends int, U extends double>(\n\
           T t, S s, U u, int i) {\n\
           int a = i + t;\n\
           int b = i + s;\n\
           int c = i + u;\n\
           int d = t + i;\n\
           print('$a$b$c$d');\n\
         }\n",
    );
    let messages = sink_messages(&folded);
    assert_eq!(
        messages.len(),
        3,
        "num, double and the `num` receiver's tail report; `S extends int` stays silent: {messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.contains("'num'"))
            && messages.iter().any(|m| m.contains("'double'")),
        "the bound faces reach the sinks: {messages:?}"
    );
}

/// A type parameter with NO captured bound keeps the under-report: the
/// implicit `Object?` interface is no `num` face, and releasing it on an
/// unknown would re-open the spurious-`double` hole.
#[test]
fn unbound_parameter_operand_suppresses() {
    let folded = report_ops(
        "void f<T>(int i, T t) {\n\
           int x = i + t;\n\
           print(x);\n\
         }\n",
    );
    assert!(
        sink_messages(&folded).is_empty(),
        "an unbound parameter's operand stays unreleased: {:#?}",
        folded.diagnostics
    );
}

/// The numeric CALL refinement reads a literal receiver's own primitive:
/// `3.clamp(0, i)` is `int` (silent at an `int` sink) while
/// `3.5.clamp(0, i)` answers `num`, exactly as the oracle does.
#[test]
fn literal_receiver_clamp_refines() {
    let folded = report_ops(
        "void f(int i) {\n\
           int a = 3.clamp(0, i);\n\
           int b = 3.5.clamp(0, i);\n\
           int c = 3.remainder(i + 1);\n\
           print('$a$b$c');\n\
         }\n",
    );
    let messages = sink_messages(&folded);
    assert_eq!(
        messages.len(),
        1,
        "only the mixed `3.5.clamp(0, i)` reports (`num`): {messages:?}"
    );
    assert!(
        messages[0].contains("'num'"),
        "the mixed-double clamp reads num: {:?}",
        messages[0]
    );
}
