//! #3495 pins — the extension-MEMBER send's type-argument bounds face.
//!
//! # The defect
//!
//! A member that lives on an APPLIED extension (not on the receiver's
//! container) never reached the type-argument bounds check: the send lands
//! in `check_arguments` with no formals at all (the member table walks the
//! receiver's container only), so the early return there skipped the
//! declared-bounds table and the written type arguments checked nothing.
//! The census rows behind the ticket are the SDK helper's
//! `expectStaticType<Exactly<..>>` calls — `extension StaticType<T> on T`
//! whose member declares `R extends Exactly<T>` — where the oracle reports
//! `type_argument_not_matching_bounds` and dartq was silent.
//!
//! # The contract
//!
//! Upstream reads the INSTANTIATED member type before checking the bounds
//! (`checkMethodInvocation` passes `node.function.staticType`, upstream
//! Dart SDK source,
//! pkg/analyzer/lib/src/error/type_arguments_verifier.dart:236-240 and
//! :570-633): the extension's own parameters bind by the application (the
//! `on` clause against the receiver) and the member's own by the written
//! group, so `R extends Exactly<T>` over an `int` receiver asks
//! `Exactly<String> <: Exactly<int>` and reports against `Exactly<int>`.
//!
//! Every expectation below is the message the oracle's template renders for
//! that pair (`'{0}' doesn't conform to the bound '{2}' of the type
//! parameter '{1}'.`).

use super::test_rig::{report_ops, rows};

/// The `type_argument_not_matching_bounds` messages of `report`, in source
/// order — the only rows these fixtures assert on.
fn bounds_rows(source: &str) -> Vec<String> {
    let report = report_ops(source);
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "type_argument_not_matching_bounds")
        .map(|d| d.message.to_string())
        .collect()
}

/// The bare own-parameter `on` head binds the extension's parameter to the
/// receiver wholesale, so the member's bound instantiates: `Exactly<String>`
/// against `Exactly<int>` over an `int` receiver reports the oracle's row.
#[test]
fn bare_own_parameter_extension_bound_fires() {
    let rows = bounds_rows(
        "class Exactly<S> {}\n\
         extension StaticType<T> on T {\n\
         \x20 T expectStaticType<R extends Exactly<T>>() => this;\n\
         }\n\
         void main() {\n\
         \x20 int i = 1;\n\
         \x20 i.expectStaticType<Exactly<String>>();\n\
         }\n",
    );
    assert_eq!(
        rows,
        ["'Exactly<String>' doesn't conform to the bound 'Exactly<int>' of the type parameter 'R'."],
    );
}

/// The silent twin: the same call whose argument MATCHES the instantiated
/// bound — `Exactly<int>` against `Exactly<int>` — resolves clean.
#[test]
fn bare_own_parameter_extension_bound_conforming_is_silent() {
    let report = report_ops(
        "class Exactly<S> {}\n\
         extension StaticType<T> on T {\n\
         \x20 T expectStaticType<R extends Exactly<T>>() => this;\n\
         }\n\
         void main() {\n\
         \x20 int i = 1;\n\
         \x20 i.expectStaticType<Exactly<int>>();\n\
         }\n",
    );
    assert!(
        report.diagnostics.is_empty(),
        "expected no rows, got {:#?}",
        rows(&report)
    );
}

/// The census shape itself: the same extension with `Exactly` spelled as a
/// function-type alias (`typedef Exactly<T> = T Function(T)`, the
/// `expect` package helper's form) — the argument and the substituted bound
/// still display through their alias heads.
#[test]
fn function_alias_bound_prints_its_alias_head() {
    let rows = bounds_rows(
        "typedef Exactly<T> = T Function(T);\n\
         extension StaticType<T> on T {\n\
         \x20 T expectStaticType<R extends Exactly<T>>() => this;\n\
         }\n\
         void main() {\n\
         \x20 int i = 1;\n\
         \x20 i.expectStaticType<Exactly<String>>();\n\
         }\n",
    );
    assert_eq!(
        rows,
        ["'Exactly<String>' doesn't conform to the bound 'Exactly<int>' of the type parameter 'R'."],
    );
}

/// A generic `on` clause binds its argument positions from the receiver's
/// written arguments (`extension Boxed<T> on Box<T>` over `Box<int>` binds
/// `T := int`), so the bounds check reads the same instantiated face.
#[test]
fn generic_on_clause_binds_the_receiver_argument() {
    let rows = bounds_rows(
        "class Box<E> {}\n\
         class Exactly<S> {}\n\
         extension Boxed<T> on Box<T> {\n\
         \x20 T same<R extends Exactly<T>>() => throw 0;\n\
         }\n\
         void f(Box<int> b) {\n\
         \x20 b.same<Exactly<String>>();\n\
         }\n",
    );
    assert_eq!(
        rows,
        ["'Exactly<String>' doesn't conform to the bound 'Exactly<int>' of the type parameter 'R'."],
    );
}

/// A bound that never mentions the extension's parameters needs no
/// binding: `R extends num` checks the written argument against `num`
/// alone, and `String` misses it.
#[test]
fn parameter_independent_bound_fires() {
    let rows = bounds_rows(
        "extension Palindromic on int {\n\
         \x20 int same<R extends num>() => this;\n\
         }\n\
         void main() {\n\
         \x20 int i = 1;\n\
         \x20 i.same<String>();\n\
         }\n",
    );
    assert_eq!(
        rows,
        ["'String' doesn't conform to the bound 'num' of the type parameter 'R'."],
    );
}

/// A conforming send on an extension member reports NOTHING: the face adds
/// the bounds row alone, never a companion miss or arity row.
#[test]
fn conforming_member_send_reports_no_rows() {
    let report = report_ops(
        "class Exactly<S> {}\n\
         extension StaticType<T> on T {\n\
         \x20 T expectStaticType<R extends Exactly<T>>() => this;\n\
         }\n\
         void main() {\n\
         \x20 String s = '';\n\
         \x20 s.expectStaticType<Exactly<String>>();\n\
         }\n",
    );
    assert!(
        report.diagnostics.is_empty(),
        "expected no rows, got {:#?}",
        rows(&report)
    );
}
