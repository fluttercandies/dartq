//! The #3063 pins — the binary operator's node type: the numeric
//! refinement over a `num` receiver, and the operator signature
//! instantiated over the receiver's written arguments.
//!
//! Upstream decides the node through the RESOLVED operator element
//! (`pkg/analyzer/lib/src/dart/resolver/binary_expression_resolver.dart`:
//! `_resolveUserDefinableType` reads `element.returnType` at :479-515,
//! then `typeSystem.refineBinaryExpressionType`), and the refinement
//! ladder is `_refineNumericInvocationTypeNullSafe`
//! (`pkg/analyzer/lib/src/dart/element/type_system.dart:1983-2046`):
//! for `+ - * % remainder` on a `T <: num?` receiver it answers `double`
//! when `T <: double?` or `S <: double?`, `int` when both sides read
//! `int?`, `num` otherwise. Both gates are `?`-checked, so `num + int`
//! lands on the `num` tail and `num + double` on the `double` arm — the
//! receiver being literally `num` is no exemption, and `int + double`
//! follows the `S <: double?` arm.
//!
//! Oracle probes (Dart 3.13.1, `dart analyze --format machine`):
//! `num n; double d; double x = n + d;` reports nothing (the node is
//! `double`); `num n; int i; int y = n + i;` reports
//! `A value of type 'num' can't be assigned to a variable of type 'int'`
//! (the node is `num`); `int i; double d; int z = i + d;` reports
//! `A value of type 'double' …` (the node is `double`).
//!
//! The rig's prelude declares `class num {}` without an arithmetic face
//! (the real `num.+` is external and unreachable from a unit), so every
//! `num`-receiver probe also stages the member walk's `undefined_operator`
//! — an artifact of the fixture, never of the oracle. The pins below read
//! the SINK row (`invalid_assignment`), whose message renders the folded
//! node type, which is exactly what these two mechanisms decide.

use super::test_rig::report_ops;
use super::*;

/// The `invalid_assignment` rows' messages — the folded node's face as
/// the sink renders it.
fn sink_messages(folded: &FileReport) -> Vec<String> {
    folded
        .diagnostics
        .iter()
        .filter(|diag| diag.code.name == "invalid_assignment")
        .map(|diag| diag.message.to_string())
        .collect()
}

/// `num + double` refines to `double` (the ladder's first operand arm:
/// `S <: double?` decides whatever `T` is, `num` included). Before #3063
/// the shadow guard short-circuited every `num` receiver into the
/// unrefined arm, so the node answered the declared `num` and the
/// `double` sink reported.
#[test]
fn num_plus_double_stays_double() {
    let folded = report_ops(
        "void f(num n, double d) {\n\
           double x = n + d;\n\
           print(x);\n\
         }\n",
    );
    assert!(
        sink_messages(&folded).is_empty(),
        "`num + double` is `double` in the oracle: {:#?}",
        folded.diagnostics
    );
}

/// `num + int` keeps the `num` tail (neither side reads `double?`, and
/// the `int` arm needs the RECEIVER to read `int?` — `num` does not), so
/// an `int` sink reports the `num` face.
#[test]
fn num_plus_int_stays_num() {
    let folded = report_ops(
        "void f(num n, int i) {\n\
           int y = n + i;\n\
           print(y);\n\
         }\n",
    );
    let messages = sink_messages(&folded);
    assert_eq!(
        messages.len(),
        1,
        "`num + int` is `num`, so the int sink reports once: {messages:?}"
    );
    assert!(
        messages[0].contains("'num'") && messages[0].contains("'int'"),
        "the node's `num` face is what the sink renders: {:?}",
        messages[0]
    );
}

/// `int + double` reads `double` (the ladder's `S <: double?` arm —
/// probed against the 3.13.1 oracle), never the `num` tail.
#[test]
fn int_plus_double_reads_double() {
    let folded = report_ops(
        "void f(int i, double d) {\n\
           int z = i + d;\n\
           print(z);\n\
         }\n",
    );
    let messages = sink_messages(&folded);
    assert_eq!(messages.len(), 1, "one sink row: {messages:?}");
    assert!(
        messages[0].contains("'double'"),
        "`int + double` is `double` in the oracle: {:?}",
        messages[0]
    );
}

/// The operator's declared signature instantiates over the receiver's
/// written arguments: `Box<int> + Box<int>` answers `Box<int>` (never the
/// raw `Box<T>`), so the folded node carries the instantiated face.
#[test]
fn generic_operator_return_instantiates() {
    let folded = report_ops(
        "class Box<T> {\n\
           final T value;\n\
           Box(this.value);\n\
           Box<T> operator +(Box<T> other) => Box(value);\n\
         }\n\
         void f(Box<int> b) {\n\
           var r = b + b;\n\
           String s = r;\n\
           print(s);\n\
         }\n",
    );
    let messages = sink_messages(&folded);
    assert_eq!(messages.len(), 1, "one sink row: {messages:?}");
    assert!(
        messages[0].contains("'Box<int>'"),
        "the node is the instantiated `Box<int>`: {:?}",
        messages[0]
    );
}

/// The same instantiation feeds the parameter slot: the operator's formal
/// reads `Box<int>`, so a `Box<int>`-returning generic send is assignable
/// (the raw `Box<T>` formal rejected it as `argument_type_not_assignable`
/// and leaked the parameter spelling into the message).
#[test]
fn generic_operator_formal_instantiates() {
    let folded = report_ops(
        "class Box<T> {\n\
           final T value;\n\
           Box(this.value);\n\
           Box<T> operator +(Box<T> other) => Box(value);\n\
         }\n\
         Box<T> wrap<T>(T v) => Box<T>(v);\n\
         void f(Box<int> b) {\n\
           var r = b + wrap(3);\n\
           String s = r;\n\
           print(s);\n\
         }\n",
    );
    let messages = sink_messages(&folded);
    assert_eq!(
        messages.len(),
        1,
        "`wrap(3)` is `Box<int>`, assignable to the instantiated formal: {messages:?}"
    );
    assert!(
        messages[0].contains("'Box<int>'") && !messages[0].contains("'Box<T>'"),
        "no raw parameter spelling reaches the sink message: {:?}",
        messages[0]
    );
    assert!(
        !folded
            .diagnostics
            .iter()
            .any(|diag| diag.code.name == "argument_type_not_assignable"),
        "the operand check against the instantiated formal stays silent: {:#?}",
        folded.diagnostics
    );
}

/// A creation-shaped receiver carries the instance type, not the bare
/// head: `Box<int>(1) + Box<int>(2)` answers `Box<int>` (the operand read
/// instantiates the created row's written group), so the node is
/// `Box<int>` and never the free `Box<T>`.
#[test]
fn creation_receiver_operand_instantiates() {
    let folded = report_ops(
        "class Box<T> {\n\
           final T value;\n\
           Box(this.value);\n\
           Box<T> operator +(Box<T> other) => Box(value);\n\
         }\n\
         void f() {\n\
           var r = Box<int>(1) + Box<int>(2);\n\
           String s = r;\n\
           print(s);\n\
         }\n",
    );
    let messages = sink_messages(&folded);
    assert_eq!(messages.len(), 1, "one sink row: {messages:?}");
    assert!(
        messages[0].contains("'Box<int>'"),
        "the creation operand instantiates: {:?}",
        messages[0]
    );
}
