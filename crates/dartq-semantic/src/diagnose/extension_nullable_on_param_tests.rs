//! The nullable own-parameter `on` clause (`extension<T> on T?`): the
//! parameter's nullable instantiation accepts every receiver whose core
//! satisfies the parameter's bound — an absent bound is the top, so the
//! face applies to any receiver, and a nullable receiver resolves through
//! it safely (no `unchecked_use_of_nullable_value`, the member credited).
//! Oracle provenance: `dart analyze` 3.13.1 — the analyzer's own
//! `extension<T> on T? { mapOrNull }` over `UnlinkedLibraryImportPrefix?`
//! (summary2/library_builder.dart census shape).

use super::test_rig::{assert_silent, code_names, report_ops};

/// `extension<T> on T?` over a nullable receiver: the member resolves
/// through the applicable nullable-`on` face — silent, never the unsafe
/// dereference row.
#[test]
fn nullable_own_param_extension_resolves_nullable_receivers() {
    let report = report_ops(
        "class Prefix {\n\
         \x20 String get name => '';\n\
         }\n\
         extension<T> on T? {\n\
         \x20 R? mapOrNull<R>(R Function(T) mapper) => null;\n\
         }\n\
         void f(Prefix? p) {\n\
         \x20 p.mapOrNull((t) => t.name);\n\
         }\n",
    );
    assert_silent(&report);
}

/// The same face over a non-null receiver applies too — `T?` accepts the
/// core type directly.
#[test]
fn nullable_own_param_extension_resolves_non_null_receivers() {
    let report = report_ops(
        "class Prefix {}\n\
         extension<T> on T? {\n\
         \x20 R? mapOrNull<R>(R Function(T) mapper) => null;\n\
         }\n\
         void f(Prefix p) {\n\
         \x20 p.mapOrNull((t) => 1);\n\
         }\n",
    );
    assert_silent(&report);
}

/// A bound still orders the core: `extension<T extends num> on T?` never
/// applies to a `String` receiver, so the access reports the undefined
/// member exactly like the oracle's failed `applicableTo`.
#[test]
fn nullable_own_param_extension_keeps_the_bound_gate() {
    let report = report_ops(
        "extension<T extends num> on T? {\n\
         \x20 R? mapOrNull<R>(R Function(T) mapper) => null;\n\
         }\n\
         void f(String s) {\n\
         \x20 s.mapOrNull((t) => 1);\n\
         }\n",
    );
    assert_eq!(code_names(&report), ["undefined_method"]);
}
