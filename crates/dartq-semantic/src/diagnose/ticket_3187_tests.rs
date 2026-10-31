//! #3187 — the instantiated type literal as a MEMBER target.
//!
//! `C<T>.sel<T>()` / `Alias<T>.sel<T>()`: the selector's own type
//! arguments keep the send a `MethodInvocation`, and upstream
//! `_resolveReceiverTypeLiteral`
//! (pkg/analyzer/lib/src/dart/resolver/method_invocation_resolver.dart:
//! 1212-1268) resolves `sel` against the `Type` INSTANCE — the class's or
//! the alias's statics never answer, only the `Object` face resolves, and a
//! clean miss is `undefined_method` naming `Type`. dartq resolved the
//! receiver to the container and found the static, so the row was missing.
//!
//! The CREATION faces stay untouched, because the same spelling without
//! the selector's type arguments belongs to them (oracle 3.13.7, probed in
//! `/tmp/t3187/m1.dart`):
//! - `C<T>.sel()` → `new_with_undefined_constructor` ("The class 'A'
//!   doesn't have a constructor named 'sel'.")
//! - `C<T>.sel`   → `class_instantiation_access_to_member`
//! - `C<T>.new()` → resolves; `C<T>().sel<T>()` → instance face.

use super::test_rig::report_ops;
use crate::FileReport;

/// The messages of one code, in report order.
fn messages(report: &FileReport, code: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.message.to_string())
        .collect()
}

/// The class head: a generic class with a static generic method (the
/// method the oracle refuses to find through the instantiated literal) and
/// an instance generic method for the value-receiver control.
const FIXTURE: &str = "class A<X> {\n\
       A();\n\
       static void sm<Y>() {}\n\
       void im<Y>() {}\n\
     }\n\
     typedef T<X> = A<X>;\n";

/// `A<int>.sm<num>()`: one row, `undefined_method` naming `Type`.
#[test]
fn instantiated_class_literal_method_names_type() {
    let report = report_ops(&format!(
        "{FIXTURE}void main() {{ A<num>.sm<num>(); }}\n"
    ));
    let methods = messages(&report, "undefined_method");
    assert_eq!(
        methods,
        ["The method 'sm' isn't defined for the type 'Type'."],
        "{methods:?}"
    );
}

/// The alias-of-class twin: `T<int>.sm<num>()` names `Type` too.
#[test]
fn instantiated_alias_literal_method_names_type() {
    let report = report_ops(&format!(
        "{FIXTURE}void main() {{ T<num>.sm<num>(); }}\n"
    ));
    let methods = messages(&report, "undefined_method");
    assert_eq!(
        methods,
        ["The method 'sm' isn't defined for the type 'Type'."],
        "{methods:?}"
    );
}

/// Control: without the selector's type arguments the spelling is the
/// CREATION face — it must not be claimed by the `Type` member face (that
/// family's own row is a separate lane).
#[test]
fn creation_face_is_not_claimed() {
    let report = report_ops(&format!("{FIXTURE}void main() {{ A<num>.sm(); }}\n"));
    let methods = messages(&report, "undefined_method");
    assert!(
        methods.iter().all(|m| !m.contains("'Type'")),
        "{methods:?}"
    );
}

/// Control: a value receiver (`A<num>()`) keeps the instance face — the
/// generic instance method resolves and stays row-free.
#[test]
fn value_receiver_keeps_instance_face() {
    let report = report_ops(&format!(
        "{FIXTURE}void main() {{ A<num>().im<num>(); }}\n"
    ));
    let methods = messages(&report, "undefined_method");
    assert!(methods.is_empty(), "{methods:?}");
}

/// Control: the unnamed-constructor selector on an instantiated literal
/// resolves (`A<num>.new()`), so the split leaves nothing behind.
#[test]
fn instantiated_literal_new_resolves() {
    let report = report_ops(&format!("{FIXTURE}void main() {{ A<num>.new(); }}\n"));
    let methods = messages(&report, "undefined_method");
    assert!(methods.is_empty(), "{methods:?}");
}
