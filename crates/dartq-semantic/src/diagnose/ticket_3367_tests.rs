//! #3367 pins — the `undefined_*` EXTRA-row clusters the recon's C1–C3
//! table routes to `diagnose/row_resolution.rs`.
//!
//! # The defects
//!
//! C1: the extension read face's viability gate ([`extension_on_applies`])
//! required `Ty::Named` head equality, so an `on` clause spelling the
//! extension's OWN bare type parameter (`extension E<T> on T`) could never
//! be the static-type answer — the oracle binds `T := receiverType` and
//! asks the parameter's declared bound (`applicableTo`,
//! `applicable_extensions.dart:205-281`). The function- and
//! record-receiver twins ([`function_extension_face`],
//! [`record_extension_face`]) skipped every param-carrying face outright,
//! so the same `on T` face never applied to a `Function`/record receiver
//! either.
//!
//! C3: `p.dynamic` reported `undefined_class` — the `dynamic` keyword is
//! dart:core's synthetic `DynamicElementImpl`, which the export scope
//! never carries, so the prefixed scoped probe misses it by construction.
//! Upstream resolves it through the prefix's core namespace
//! (`named_type_resolver.dart:85-121`): [`prefixed_hit`] now answers
//! `true` when the prefix's own import edge targets `dart:core` and its
//! combinators admit the name.
//!
//! # Residual (out of scope — `invocation.rs`/`engine_core.rs`)
//!
//! The `Receiver::Container`/`cascade_sections` latch that emits
//! `undefined_method` naming `Type` for `Future.value(C())
//! .expectStaticType<..>()` after a `..`-cascade, and the bare-`dynamic`
//! type-position MISSING direction, ride products of files this ticket
//! does not own; the pins below cover only the in-scope arms.
//!
//! [`extension_on_applies`]: crate::diagnose::row_resolution
//! [`function_extension_face`]: crate::diagnose::row_resolution
//! [`record_extension_face`]: crate::diagnose::row_resolution
//! [`prefixed_hit`]: crate::diagnose::row_resolution

use crate::diagnose::test_rig::{
    RigImport, assert_silent, code_names, report_ops, report_ops_imports,
};

/// `extension E<T> on T` applies to every receiver (the unbounded
/// parameter is the top): the send resolves and reports nothing.
#[test]
fn t3367_on_own_param_applies() {
    let report = report_ops(
        "extension E<T> on T { void m() {} } \
         void f() { C().m(); } \
         class C {}",
    );
    assert_silent(&report);
}

/// The `on T?` twin: a nullable own-parameter `on` still binds the
/// receiver wholesale — `void m()` resolves on the non-null `C`.
#[test]
fn t3367_on_own_param_nullable() {
    let report = report_ops(
        "extension E<T> on T? { void m() {} } \
         void f() { C().m(); } \
         class C {}",
    );
    assert_silent(&report);
}

/// A bare own-parameter `on` head over a FUNCTION-typed receiver: `T`
/// binds the function type, the unbounded bound accepts, and the member
/// resolves — the gate `function_extension_face` used to skip wholesale.
#[test]
fn t3367_on_own_param_function_receiver() {
    let report = report_ops(
        "extension E<T> on T { void m() {} } \
         void g() {} \
         void f() { g.m(); }",
    );
    assert_silent(&report);
}

/// The same own-parameter `on` over a RECORD-typed receiver: the record
/// face's named-`on` destructure skipped `T` before, so the member never
/// applied; now `T := (int, String)` binds and resolves.
#[test]
fn t3367_on_own_param_record_receiver() {
    let report = report_ops(
        "extension E<T> on T { void m() {} } \
         void f() { (1, 'x').m(); }",
    );
    assert_silent(&report);
}

/// A bounded own-parameter `on` rejects the receiver whose non-null core
/// misses the bound: `extension E<T extends num> on T` never applies to
/// `String`, so `s.m()` reports `undefined_method` naming `String`.
#[test]
fn t3367_on_own_param_bound_rejects() {
    let report = report_ops(
        "extension E<T extends num> on T { void m() {} } \
         void f() { String s = ''; s.m(); }",
    );
    assert_eq!(
        code_names(&report),
        ["undefined_method"],
        "unexpected rows: {:#?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// The bounded twin's admit side: `int` satisfies `num`, so `i.m()`
/// resolves silently.
#[test]
fn t3367_on_own_param_bound_admits() {
    let report = report_ops(
        "extension E<T extends num> on T { void m() {} } \
         void f() { int i = 0; i.m(); }",
    );
    assert_silent(&report);
}

/// `p.dynamic` where `p` prefixes a NON-core library stays an
/// `undefined_class` miss — the `dynamic` face is dart:core's synthetic
/// element, which a `package:` prefix never exports.
#[test]
fn t3367_prefixed_dynamic_non_core() {
    let report = report_ops_imports(
        "p.dynamic d = 0;",
        &[RigImport {
            uri: "package:a/a.dart",
            prefix: Some("p"),
            source: "class A {}",
        }],
    );
    assert_eq!(
        code_names(&report),
        ["undefined_class"],
        "unexpected rows: {:#?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// `core.dynamic` where `core` prefixes `dart:core` itself resolves —
/// upstream's prefixed `myCore.dynamic` twin (the synthetic dynamic face
/// the scoped probe cannot carry). The rig mounts a `dart:core` library
/// with an unprefixed import; a second edge under `core` reproduces
/// `import 'dart:core' as core`.
#[test]
fn t3367_prefixed_dynamic_core() {
    let report = report_ops_imports(
        "core.dynamic d = 0;",
        &[RigImport {
            uri: "dart:core",
            prefix: Some("core"),
            source: "",
        }],
    );
    assert_silent(&report);
}

/// A genuinely missing type name still reports `undefined_class` — the
/// `dynamic` gate is name-keyed, never a blanket prefixed suppression.
#[test]
fn t3367_undefined_class_negative() {
    let report = report_ops("MissingType m;");
    assert_eq!(
        code_names(&report),
        ["undefined_class"],
        "unexpected rows: {:#?}",
        crate::diagnose::test_rig::rows(&report)
    );
}
