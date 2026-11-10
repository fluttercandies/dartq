//! The #3821 `unnecessary_type_check` precision pins.
//!
//! Upstream reports this family from a single gate —
//! `_checkAllTypeChecks` (upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/best_practices_verifier.dart:877-930): an
//! `InvalidType` operand or target is silent, `is dynamic` is constant,
//! `is Null` belongs to the `type_check_with_null` family, and the only
//! remaining report is `TypeSystem.isSubtypeOf(leftType, rightType)` —
//! `true` for `is`, `false` for `is!`. Everything else, including a
//! `false` subtype verdict, is silent: there is no "constant false"
//! verdict in the subtype relation.
//!
//! The pins below cover the part of the family the gate already decides
//! correctly (function-type aliases whose nested faces are compared
//! contravariantly), so a future change to the verdict walk cannot
//! silently widen the report. The `#[ignore]`d cases are the measured
//! over-reports: the operand's own type carries the *unsubstituted* type
//! parameter of the declaring class (a mixin application's `M<V>` field,
//! and a tearing-off generic function inside a large unit), so the gate
//! compares a face the official analyzer would have substituted before
//! the verdict. They are recorded as acceptance criteria for the
//! substitution fix rather than silenced here — silencing them at the
//! gate would also silence the sibling rows the oracle does report
//! (`is F13<int>` beside `is F13<bool>`, `is A<int>` beside
//! `is A<String>`), turning precision debt into coverage debt.

use crate::diagnose::test_rig::report_ops;

/// The diagnostics of `source` carrying the family's code, in report
/// order.
fn unnecessary_type_checks(source: &str) -> Vec<String> {
    report_ops(source)
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code.name == "unnecessary_type_check")
        .map(|diagnostic| diagnostic.message.to_string())
        .collect()
}

/// A function-typed tear-off against a function-type alias whose nested
/// *named* formal differs (`{List<bool> x}` against the alias's
/// `{List<int> x}`) is informative: upstream asks
/// `isSubtypeOf(int Function({List<bool>}), int Function({List<int>}))`,
/// the contravariant leg asks `List<int> <: List<bool>`, which refuses
/// (`_isFunctionSubtypeOf`, subtype.dart:384-402) — no constant-true
/// verdict, no row.
#[test]
fn ticket_3821_nested_named_formal_mismatch_stays_silent() {
    let source = "typedef P = void Function({List<int> x});\n\
                  void h({List<bool> x}) {}\n\
                  void g() {\n\
                  \x20 var b = h is P;\n\
                  }";
    assert_eq!(
        unnecessary_type_checks(source),
        Vec::<String>::new(),
        "{:?}",
        unnecessary_type_checks(source)
    );
}

/// The same shape with the formal types equal IS the constant-true row:
/// upstream's `isSubtypeOf` holds reflexively for the two identical
/// signatures, so `is` reports `unnecessary_type_check`.
#[test]
fn ticket_3821_identical_signature_alias_reports_true() {
    let source = "typedef Q = void Function({List<int> x});\n\
                  void k({List<int> x}) {}\n\
                  void g() {\n\
                  \x20 var b = k is Q;\n\
                  }";
    let messages = unnecessary_type_checks(source);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("always 'true'"), "{:?}", messages[0]);
}

/// A field inherited from a mixin application (`class C1<V> = Object with
/// M<V>`) is typed `A<V>` instantiated at the use site: the official
/// analyzer substitutes the mixin's own parameter through the application
/// before the verdict, so `C1<int>().field is A<String>` is
/// `A<int> <: A<String>` — false, silent. Recorded here as the acceptance
/// criterion for the substitution fix; the gate itself cannot tell this
/// pair from the reported `is A<int>` sibling without the substituted
/// argument.
#[ignore = "blocked: mixin-application type arguments are not substituted into inherited field types"]
#[test]
fn ticket_3821_mixin_application_field_argument_stays_silent() {
    let source = "mixin M<T> {\n\
                  \x20 var field = A<T>();\n\
                  }\n\
                  class A<U> {}\n\
                  class C1<V> = Object with M<V>;\n\
                  void g() {\n\
                  \x20 var b = C1<int>().field is A<String>;\n\
                  }";
    assert_eq!(
        unnecessary_type_checks(source),
        Vec::<String>::new(),
        "{:?}",
        unnecessary_type_checks(source)
    );
}

/// The same-head interface argument is a real subtype question: a
/// `List<int>` operand against `List<String>` is informative — the
/// covariant argument leg asks `int <: String` and refuses
/// (`_isInterfaceSubtypeOf`'s `_interfaceArguments`, subtype.dart:296-335),
/// so the gate must stay silent. Guards the constant-true verdict against
/// regressing to a head-only comparison.
#[test]
fn ticket_3821_interface_argument_mismatch_stays_silent() {
    let source = "void g(List<int> l) {\n\
                  \x20 var b = l is List<String>;\n\
                  }";
    assert_eq!(
        unnecessary_type_checks(source),
        Vec::<String>::new(),
        "{:?}",
        unnecessary_type_checks(source)
    );
}
