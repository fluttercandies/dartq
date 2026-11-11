//! #3913 pins — the const_eval created-primitive operand matrix, the
//! default-value face's residual, and the const-family accounting.
//!
//! Three faces were extracted from the census sweep and routed here:
//!
//! * **The created-primitive operand matrix** (the ticket's item 3, the
//!   largest const_eval.rs-rooted family). A created primitive instance
//!   (`int.fromEnvironment(...)`, `bool.fromEnvironment(...)`,
//!   `String.fromEnvironment(...)`) carries an UNKNOWN payload — upstream
//!   `IntState(null)`/`BoolState(null)`/`StringState(null)`. The fold used
//!   to answer every eager operator over such an operand with the
//!   operand-type face (`const_eval_type_num`/`_bool_int`/`_int`), because
//!   [`crate::const_eval::ConstantValue::Created`] fell through to the
//!   concrete-state arms. Upstream instead runs the state's own method,
//!   whose `value == null` branch returns `*State.UNKNOWN_VALUE`: a
//!   num/num `+`, an int/int `&`, and an int/int shift are all VALID
//!   constants of unknown value, so no row may fire.
//!   `binary_values` now routes any created operand pair through
//!   `created_operand_values`, which reads the operand's
//!   [`crate::const_eval::CreatedState`] class and reproduces the upstream
//!   gate order (upstream `value.dart` `InstanceState.add` :1636-1647,
//!   `IntState.add` :2053-2075, `eagerAnd`/`eagerOr`/`eagerXor` :446-514,
//!   `shiftLeft` :1987-2003, `equalEqual` :524-566).
//!
//!   The fixtures shadow the SDK's `bool`/`int`/`String` with in-file
//!   classes carrying a `const ...fromEnvironment(...)` factory — the
//!   rig's standing pattern for reaching the created-state fold without
//!   the SDK (`const_eval_faces.rs`'s own created-state pin does the
//!   same).
//!
//! * **The parameter default-value face** (item 1). W's note routed
//!   `parameter_default_type_checking_error_test` out of the arg-slot
//!   context; the case is a const DEFAULT VALUE face, and its six rows are
//!   `list_element_type_not_assignable` on the closure parameter's default
//!   list literal. Those six live in `assignability.rs`'s
//!   `literal_ctx_ty` channel walk, which never reads a formal's written
//!   type as a context — that file is a LIVE lane's (V), so this file
//!   carries the routing pin instead of a fix.
//!
//! * **E-op B** (item 2, the `double_in_int_op` family). The census rows
//!   are EXTRAS of `argument_type_not_assignable` on a const-`dynamic`
//!   receiver's `|` operand. The root is `engine_core.rs`'s operator
//!   argument face (Lane AP, live), not const_eval.rs: the const value's
//!   type leaks into the argument check where the oracle reads the
//!   receiver's STATIC type (`dynamic`, no check). Routed, not fixed.

use super::test_rig::report_ops;
use crate::FileReport;

/// The rows of one code, in source order.
fn rows_of(report: &FileReport, code: &str) -> Vec<String> {
    let mut at: Vec<(u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.message.to_string()))
        .collect();
    at.sort_by_key(|(offset, _)| *offset);
    at.into_iter().map(|(_, text)| text).collect()
}

/// The sorted code names of one report.
fn codes(report: &FileReport) -> Vec<String> {
    let mut out: Vec<String> = report
        .diagnostics
        .iter()
        .map(|d| d.code.name.to_string())
        .collect();
    out.sort();
    out
}

/// The created-state fixture prelude: `bool`/`int`/`String` shadowed by
/// in-file const factories whose payload is an opaque `V`.
const PRELUDE: &str = "\
class V { const V(); }
class bool { final V? v; const bool.fromEnvironment(this.v); }
class int { final V? v; const int.fromEnvironment(this.v); }
class String { final V? v; const String.fromEnvironment(this.v); }
";

/// One const initializer over the shadowed primitives.
fn probe(body: &str) -> FileReport {
    report_ops(&format!("{PRELUDE}const x = {body};\n"))
}

/// The `const_eval*` codes only (the prelude's shadowing classes carry no
/// rows of their own, so this is the whole verdict surface).
fn const_codes(report: &FileReport) -> Vec<String> {
    codes(report)
        .into_iter()
        .filter(|c| c.starts_with("const_eval"))
        .collect()
}

// —— the created-primitive operand matrix ——————————————————————————————

/// `int.fromEnvironment(v) + 1` — an `IntState` with an unknown payload
/// adds to an int by returning `UNKNOWN_VALUE` (upstream `IntState.add`,
/// value.dart:2053-2060). The pre-fix fold reported
/// `const_eval_type_num` here.
#[test]
fn created_int_plus_int_defers() {
    let report = probe("int.fromEnvironment(const V()) + 1");
    assert!(
        const_codes(&report).is_empty(),
        "{:?}",
        const_codes(&report)
    );
}

/// The mirror: `1 + int.fromEnvironment(v)` — the receiver is the known
/// int, whose `add` returns `UNKNOWN_VALUE` for an unknown int operand
/// (:2061-2066).
#[test]
fn int_plus_created_int_defers() {
    let report = probe("1 + int.fromEnvironment(const V())");
    assert!(
        const_codes(&report).is_empty(),
        "{:?}",
        const_codes(&report)
    );
}

/// `bool.fromEnvironment(v) & true` — `eagerAnd`'s `isBool &&
/// rightOperand.isBool` arm computes through `BoolState.logicalAnd`, whose
/// unknown payload answers `UNKNOWN_VALUE` (value.dart:446-461,
/// :1902-1920). The pre-fix fold reported `const_eval_type_bool_int`.
#[test]
fn created_bool_bit_and_defers() {
    let report = probe("bool.fromEnvironment(const V()) & true");
    assert!(
        const_codes(&report).is_empty(),
        "{:?}",
        const_codes(&report)
    );
}

/// `String.fromEnvironment(v) + 'y'` — a `StringState` with an unknown
/// payload concatenates through `concatenate`'s `value == null` arm
/// (value.dart:3093-3096), a valid unknown constant.
#[test]
fn created_string_plus_string_defers() {
    let report = probe("String.fromEnvironment(const V()) + 'y'");
    assert!(
        const_codes(&report).is_empty(),
        "{:?}",
        const_codes(&report)
    );
}

/// `int.fromEnvironment(v) & 1` — an int/int pair through `eagerAnd`'s
/// `isInt` arm answers `UNKNOWN_VALUE` (:462-475).
#[test]
fn created_int_bit_and_int_defers() {
    let report = probe("int.fromEnvironment(const V()) & 1");
    assert!(
        const_codes(&report).is_empty(),
        "{:?}",
        const_codes(&report)
    );
}

/// `int.fromEnvironment(v) << 1` — `shiftLeft` runs `assertIntOrNull` on
/// both and the unknown payload answers `UNKNOWN_VALUE`
/// (value.dart:1987-2003, :2265-2290). The pre-fix fold reported
/// `const_eval_type_int`.
#[test]
fn created_int_shift_int_defers() {
    let report = probe("int.fromEnvironment(const V()) << 1");
    assert!(
        const_codes(&report).is_empty(),
        "{:?}",
        const_codes(&report)
    );
}

/// The whole `assertNumOrNull` family over an unknown num payload computes
/// to an unknown constant (`NumState`/`IntState` :1956-2030): `-`, `*`,
/// `/`, `~/`, `%`, `<`, `>=`.
#[test]
fn created_int_numeric_family_defers() {
    for op in ["- 1", "* 2", "/ 2", "~/ 2", "% 2", "< 1", ">= 1"] {
        let report = probe(&format!("int.fromEnvironment(const V()) {op}"));
        assert!(
            const_codes(&report).is_empty(),
            "{op}: {:?}",
            const_codes(&report)
        );
    }
}

/// `int.fromEnvironment(v) == 1` — `equalEqual` runs the state's own
/// method, whose unknown payload answers `UNKNOWN_VALUE`
/// (value.dart:524-536, :2237-2240).
#[test]
fn created_int_equality_defers() {
    let report = probe("int.fromEnvironment(const V()) == 1");
    assert!(
        const_codes(&report).is_empty(),
        "{:?}",
        const_codes(&report)
    );
}

/// `int.fromEnvironment(v) ~/ 0` — `integerDivide` runs its zero-divisor
/// arm before the unknown-payload return only for a KNOWN int; for the
/// created receiver the subset carries no payload, so the arm still fires
/// on the written zero divisor (`IntState.integerDivide`,
/// value.dart:2207-2216 — the oracle's `int.fromEnvironment('i')` folds to
/// a known `0` and answers idbze there too).
#[test]
fn created_int_int_div_by_zero_reports_idbze() {
    let report = probe("int.fromEnvironment(const V()) ~/ 0");
    assert_eq!(
        rows_of(&report, "const_eval_throws_idbze"),
        ["Evaluation of this constant expression throws an \
             IntegerDivisionByZeroException."]
    );
}

// —— the gate arms still fire ————————————————————————————————————————

/// `int.fromEnvironment(v) + true` fails `assertNumOrNull` on the right
/// operand, so the num face reports — exactly what a concrete `IntState`
/// would (value.dart:2053-2055).
#[test]
fn created_int_plus_bool_reports_the_num_face() {
    let report = probe("int.fromEnvironment(const V()) + true");
    assert_eq!(
        rows_of(&report, "const_eval_type_num"),
        ["In constant expressions, operands of this operator must be of type 'num'."]
    );
}

/// `bool.fromEnvironment(v) & 1` — a bool/int pair fails `eagerAnd`'s two
/// `isBool`/`isInt` arms, so the bool-or-int face reports
/// (value.dart:446-461).
#[test]
fn created_bool_bit_and_int_reports_the_bool_int_face() {
    let report = probe("bool.fromEnvironment(const V()) & 1");
    assert_eq!(
        rows_of(&report, "const_eval_type_bool_int"),
        ["In constant expressions, operands of this operator must be of type 'bool' or 'int'."]
    );
}

/// `String.fromEnvironment(v) + true` — a String receiver runs the base
/// `InstanceState.add`, whose `assertNumStringOrNull(rightOperand)` gate
/// admits only num/String/null, so the num-or-String face reports
/// (value.dart:1636-1640). (`+ 1` takes the THROW arm instead: an int
/// right passes the gate and the runtime add then throws.)
#[test]
fn created_string_plus_bool_reports_the_num_string_face() {
    let report = probe("String.fromEnvironment(const V()) + true");
    assert_eq!(
        rows_of(&report, "const_eval_type_num_string"),
        ["In constant expressions, operands of this operator must be of type 'num' or 'String'."]
    );
}

/// The dual: `String.fromEnvironment(v) + 1` — the int right passes the
/// num-or-String gate and the base add then throws
/// (value.dart:1636-1647).
#[test]
fn created_string_plus_int_throws() {
    let report = probe("String.fromEnvironment(const V()) + 1");
    assert_eq!(
        rows_of(&report, "const_eval_throws_exception"),
        ["Evaluation of this constant expression throws an exception."]
    );
}

/// `int.fromEnvironment(v) + null` — the num receiver's
/// `assertNumOrNull` admits null and the state's `add` then falls through
/// to `constEvalThrowsException` (value.dart:2053-2075).
#[test]
fn created_int_plus_null_throws() {
    let report = probe("int.fromEnvironment(const V()) + null");
    assert_eq!(
        rows_of(&report, "const_eval_throws_exception"),
        ["Evaluation of this constant expression throws an exception."]
    );
}

// —— the routed faces ————————————————————————————————————————————————

/// Routing pin (item 1, `parameter_default_type_checking_error_test`):
/// the six census rows are `list_element_type_not_assignable` on a
/// closure parameter's default list literal. The face lives in
/// `assignability.rs`'s `literal_ctx_ty` channel walk — a formal's
/// written type is never read as the default value's context — so the
/// owning file is `diagnose/assignability.rs` (Lane V), not this lane's.
#[test]
fn routed_parameter_default_list_context_reports() {
    let report = report_ops(
        "void f() {\n\
         \x20 var g = ([List<int> x = const ['foo']]) {};\n\
         }\n",
    );
    assert_eq!(
        rows_of(&report, "list_element_type_not_assignable"),
        ["The element type 'String' can't be assigned to the list type 'int'."]
    );
}

/// `const dynamic i1 = 3; var x = i1 | 2.2;` (item 2, the
/// `double_in_int_op_test_id1..id6` family): the census rows are EXTRAS of
/// `argument_type_not_assignable` on a const-`dynamic` receiver's `|`
/// operand. The oracle reads the receiver's STATIC type (`dynamic`, so no
/// check); dartq read the constant's VALUE type. The root is NOT the
/// operator argument face (`engine_core.rs` was already correct — `final
/// dynamic` stayed silent): the const initializer's close overwrote the
/// frame's declared slot with the folded value type for ANNOTATED consts
/// too, and upstream re-types a local from its initializer only under
/// `parent.type == null` (`variable_declaration_resolver.dart:84`). Fixed
/// in `const_eval_faces.rs`'s `finish_const_initializer`; the wider pin
/// set rides #3952's own module.
#[test]
fn routed_dynamic_receiver_const_operand_is_silent() {
    let report = report_ops(
        "void f() {\n\
         \x20 const dynamic i1 = 3;\n\
         \x20 var x = i1 | 2.2;\n\
         }\n",
    );
    assert!(
        !codes(&report)
            .iter()
            .any(|c| c == "argument_type_not_assignable"),
        "{:?}",
        codes(&report)
    );
}

// —— #3921: the leading-dot shorthand's bare-name fold ————————————————

/// `const A a = .new(.a)` — the argument `.a` is a leading-dot shorthand
/// resolving against `A`'s parameter type `E` (an enum constant), NOT the
/// const variable `a` being declared. The capture folds the shorthand's
/// selector to a bare `Ident`, so the fold's const-scope lookup answered
/// the in-progress `a` and reported a false
/// `recursive_compile_time_constant`. `Expr::Ident` now probes
/// [`crate::const_eval::ConstLookup::shorthand_context`] first and defers
/// (upstream `visitDotShorthandPropertyAccess`, evaluation.dart:935-944,
/// reads `node.propertyName.element` — the context type's member, never a
/// bare name).
#[test]
fn shorthand_operand_does_not_read_the_declaring_const() {
    let report = report_ops(
        "enum E { a, b }\n\
         class A {\n\
         \x20 const A(E e) : assert(e != .a);\n\
         }\n\
         const A a = .new(.a);\n",
    );
    assert!(
        !codes(&report)
            .iter()
            .any(|c| c == "recursive_compile_time_constant"),
        "{:?}",
        codes(&report)
    );
}

/// The control: a real self-reference still reports the cycle. `const a =
/// a;` reads the in-progress binding through the same fold, and no dot
/// precedes the name, so `shorthand_context` answers false and the fault
/// stands.
#[test]
fn plain_self_reference_still_reports_the_cycle() {
    let report = report_ops("const a = a;\n");
    assert_eq!(
        rows_of(&report, "recursive_compile_time_constant"),
        ["The compile-time constant expression depends on itself."]
    );
}

/// Routing pin (#3921 residual), LANDED at #3956: a leading-dot shorthand
/// CALL (`const A a = .method();` over
/// `class A { static A method() => A(); }`) owes
/// `const_eval_method_invocation` at the whole shorthand. The shorthand's
/// callee reaches `classify_call` as a bare selector whose
/// `dot_shorthand_context` probe answers true; `shorthand_callee` now
/// resolves it against the context type's member set (upstream
/// `method_invocation_resolver.dart`
/// `_resolveReceiverTypeLiteralForDotShorthand` probes the member face
/// before the constructor rewrite) and takes the method-invocation arm for
/// a static method.
#[test]
fn routed_shorthand_method_call_reports_method_invocation() {
    let report = report_ops(
        "class A {\n\
         \x20 static A method() => A();\n\
         }\n\
         const A a = .method();\n",
    );
    assert_eq!(
        rows_of(&report, "const_eval_method_invocation"),
        ["Methods can't be invoked in constant expressions."]
    );
}

/// Routing pin (#3921 residual), LANDED at #3956: `const A a = .new();`
/// over a const constructor whose `assert` fails owes
/// `const_eval_throws_exception` at the shorthand. `classify_call` now
/// takes the creation face for a constructor shorthand, and
/// `check_ctor_initializers`'s `ctor_init_key` re-derives the head from the
/// declaration's context type, so the initializer walk reaches the stash.
#[test]
fn routed_shorthand_constructor_assert_fault_reports() {
    let report = report_ops(
        "class A {\n\
         \x20 const A() : assert(false);\n\
         }\n\
         const A a = .new();\n",
    );
    assert_eq!(
        rows_of(&report, "const_eval_throws_exception"),
        ["Evaluation of this constant expression throws an exception."]
    );
}
