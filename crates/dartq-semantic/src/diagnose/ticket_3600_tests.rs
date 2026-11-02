use super::test_rig::{code_names, report_ops, rows};

// Missing-row pin: the explicit generic tear-off `var g = f<int>` binds
// `int Function(int)` — the oracle's `TypeInstantiation.staticType` —
// and the stored value's call `g('Hello')` reports the argument pair the
// generic face dropped. Oracle 3.13.7:
//   The argument type 'String' can't be assigned to the parameter type 'int'.
#[test]
fn generic_tearoff_stored_call_reports() {
    let report = report_ops(
        r#"
void f<T>(T a) {}
var g = f<int>;
var x = g('Hello');
"#,
    );
    assert!(
        code_names(&report).contains(&"argument_type_not_assignable"),
        "expected argument_type_not_assignable: {:#?}",
        rows(&report)
    );
}

// Same face inside a body — the stored generic tear-off invokes against
// the instantiated formals.
#[test]
fn generic_tearoff_stored_call_in_body_reports() {
    let report = report_ops(
        r#"
T id<T>(T t) => t;
void main() {
  var f = id<int>;
  f("s");
}
"#,
    );
    assert!(
        code_names(&report).contains(&"argument_type_not_assignable"),
        "expected argument_type_not_assignable: {:#?}",
        rows(&report)
    );
}

// Negative control (no fabrication): the same stored tear-off called with
// a satisfying argument stays silent — the instantiation must not widen
// the check beyond the written pair. Oracle: silent.
#[test]
fn generic_tearoff_stored_call_satisfying_stays_silent() {
    let report = report_ops(
        r#"
T id<T>(T t) => t;
void main() {
  var f = id<int>;
  f(0);
}
"#,
    );
    assert!(
        !code_names(&report).contains(&"argument_type_not_assignable"),
        "unexpected argument_type_not_assignable: {:#?}",
        rows(&report)
    );
}

// Missing-row pin (member face): the generic METHOD tear-off
// `var x = a.m<int>` binds `int Function(int)` — the same
// `TypeInstantiation.staticType` transport, through the receiver-walked
// member entry — and `x("s")` reports the pair the raw generic face kept.
// Oracle 3.13.7:
//   The argument type 'String' can't be assigned to the parameter type 'int'.
#[test]
fn generic_method_tearoff_stored_call_reports() {
    let report = report_ops(
        r#"
class A {
  T m<T>(T t) => t;
}
void main() {
  var a = A();
  var x = a.m<int>;
  x("s");
}
"#,
    );
    assert!(
        code_names(&report).contains(&"argument_type_not_assignable"),
        "expected argument_type_not_assignable: {:#?}",
        rows(&report)
    );
}

// Negative control (member face): the stored generic method tear-off
// called with a satisfying argument stays silent. Oracle: silent.
#[test]
fn generic_method_tearoff_stored_call_satisfying_stays_silent() {
    let report = report_ops(
        r#"
class A {
  T m<T>(T t) => t;
}
void main() {
  var a = A();
  var x = a.m<int>;
  x(0);
}
"#,
    );
    assert!(
        !code_names(&report).contains(&"argument_type_not_assignable"),
        "unexpected argument_type_not_assignable: {:#?}",
        rows(&report)
    );
}
