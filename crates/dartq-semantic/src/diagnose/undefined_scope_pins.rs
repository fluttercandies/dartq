//! The bare-name scope-walk pins (UNDEF-WAVE): `undefined_identifier` /
//! `undefined_class` decide by whether a name resolves through the lexical
//! chain — locals → `this` face → library scope → imports — and by the two
//! oracle rules the walk under-modeled.
//!
//! 1. **Getter/setter asymmetry.** A name that exists ONLY as a setter
//!    resolves `name=` — but the oracle's `LexicalLookup.resolveGetter`
//!    returns that setter as *recovery* (its `requested` getter stays null),
//!    so a READ still reports `undefined_identifier`. Only a WRITE consults
//!    `preferSetter`. Upstream: `pkg/analyzer/lib/src/dart/resolver/
//!    lexical_lookup.dart:22-35` (`resolveGetter`'s `scopeSetter != null &&
//!    !isInstanceMember` recovery arm) and `simple_identifier_resolver.
//!    dart`'s `element == null → undefinedIdentifier`.
//!
//! 2. **Cross-library privacy.** A `_`-prefixed member found by the `this`
//!    member walk on ANOTHER library's file is not accessible
//!    (`element.isAccessibleIn(_definingLibrary)` — `pkg/analyzer/lib/src/
//!    dart/resolver/property_element_resolver.dart:463-465`), so the
//!    implicit-`this` read reports `undefined_identifier` instead of
//!    resolving the member.
//!
//! Oracle provenance: `dart analyze` 3.13.7 (each pin probed against the
//! real binary before the fix).

use super::test_rig::{
    RigImport, assert_silent, code_names, report_ops, report_ops_imports, report_ops_wildcards,
};

/// A bare name declared nowhere reports `undefined_identifier`.
#[test]
fn undefined_bare_name_reports() {
    let report = report_ops("void f() { missing; }\n");
    assert!(
        code_names(&report).contains(&"undefined_identifier"),
        "{report:?}"
    );
}

/// A defined top-level resolves silently — the library scope level the
/// walk reaches after locals and `this`.
#[test]
fn defined_top_level_resolves() {
    let report = report_ops("int counter = 0;\nvoid f() { counter; }\n");
    assert_silent(&report);
}

/// A local declaration shadows every outer scope hit — the innermost frame
/// wins (`resolve_bare`'s frame loop precedes the scope lookup).
#[test]
fn local_shadowing_wins() {
    let report = report_ops(
        "int counter = 0;\n\
         int f() { int counter = 1; return counter; }\n",
    );
    assert_silent(&report);
}

/// A prefixed import resolves through `p.X` — the `_prefixElements` scope
/// level (scope.dart:419-486, `lookup_prefixed`).
#[test]
fn prefixed_import_resolves() {
    let report = report_ops_imports(
        "void f() { p.helper; }\n",
        &[RigImport {
            uri: "package:t/lib.dart",
            prefix: Some("p"),
            source: "int helper = 0;\n",
        }],
    );
    assert_silent(&report);
}

/// A name in a type position found nowhere reports `undefined_class`.
#[test]
fn undefined_class_in_type_position_reports() {
    let report = report_ops("Missing x;\n");
    assert!(
        code_names(&report).contains(&"undefined_class"),
        "{report:?}"
    );
}

/// `set foo` exists but `foo` (the getter) does not — a bare READ of `foo`
/// reports `undefined_identifier`: the setter is recovery, never the
/// requested getter (lexical_lookup.dart:22-35).
#[test]
fn setter_only_read_reports() {
    let report = report_ops("set foo(int v) {}\nvoid f() { foo; }\n");
    assert!(
        code_names(&report).contains(&"undefined_identifier"),
        "{report:?}"
    );
}

/// `foo++` on a setter-only name reads `foo` (the compound's getter side) —
/// still `undefined_identifier`. The compound resolves `foo=` for the write
/// but `foo` for the read; the read misses.
#[test]
fn setter_only_compound_read_reports() {
    let report = report_ops("set foo(int v) {}\nvoid f() { foo++; }\n");
    assert!(
        code_names(&report).contains(&"undefined_identifier"),
        "{report:?}"
    );
}

/// `++foo` on a setter-only name reads `foo` on the prefix path too.
#[test]
fn setter_only_prefix_read_reports() {
    let report = report_ops("set foo(int v) {}\nvoid f() { ++foo; }\n");
    assert!(
        code_names(&report).contains(&"undefined_identifier"),
        "{report:?}"
    );
}

/// The mirror image: `foo = 0` on a setter-only name resolves `foo=` and
/// stays silent — the write alone consults `preferSetter`.
#[test]
fn setter_only_write_is_silent() {
    let report = report_ops("set foo(int v) {}\nvoid f() { foo = 0; }\n");
    assert_silent(&report);
}

/// Both halves defined (`get` + `set`): read, write, and compound all
/// resolve — the symmetric pair the recovery arm never reaches.
#[test]
fn getter_and_setter_pair_resolves() {
    let report = report_ops(
        "int _backing = 0;\n\
         int get foo => _backing;\n\
         set foo(int v) { _backing = v; }\n\
         void f() { foo; foo = 0; foo++; }\n",
    );
    assert_silent(&report);
}

/// A private getter on a superclass in ANOTHER library is not accessible to
/// this unit's `this` — `_foo` reports `undefined_identifier`
/// (`isAccessibleIn`, property_element_resolver.dart:463-465).
#[test]
fn cross_library_private_member_read_reports() {
    let report = report_ops_imports(
        "class B extends A { void m() { _foo; } }\n",
        &[RigImport {
            uri: "package:t/lib.dart",
            prefix: None,
            source: "class A { int get _foo => 0; }\n",
        }],
    );
    assert!(
        code_names(&report).contains(&"undefined_identifier"),
        "{report:?}"
    );
}

/// A private member in the SAME library resolves — privacy is scoped to the
/// library boundary, not the file.
#[test]
fn same_library_private_member_resolves() {
    let report = report_ops(
        "class A { int get _foo => 0; }\n\
         class B extends A { void m() { _foo; } }\n",
    );
    assert_silent(&report);
}

/// A PUBLIC member across a library boundary resolves — the accessibility
/// gate only stops `_`-prefixed names.
#[test]
fn cross_library_public_member_resolves() {
    let report = report_ops_imports(
        "class B extends A { void m() { foo; } }\n",
        &[RigImport {
            uri: "package:t/lib.dart",
            prefix: None,
            source: "class A { int get foo => 0; }\n",
        }],
    );
    assert_silent(&report);
}

// --- Wildcard `_` declares nothing (wildcard-variables on) ---

// A `_` parameter binds nothing: a read reports `undefined_identifier`
// (FormalParameterScope skips the name — upstream Dart SDK source,
// pkg/analyzer/lib/src/dart/element/scope.dart:117-118; LocalScope's
// formal arm scope.dart:550-554).
#[test]
fn wildcard_parameter_read_reports() {
    let report = report_ops("void f(int _) { _; }\n");
    assert!(
        code_names(&report).contains(&"undefined_identifier"),
        "{report:?}"
    );
}

// A `_` local binds nothing: the read reports (`LocalScope.add`'s skip,
// scope.dart:550-554).
#[test]
fn wildcard_local_read_reports() {
    let report = report_ops("void f() { var _ = 1; _; }\n");
    assert!(
        code_names(&report).contains(&"undefined_identifier"),
        "{report:?}"
    );
}

// A `_` pattern binder binds nothing: after a record-pattern declaration
// the named element resolves and the `_` read reports (a declared `_`
// pattern variable is skipped — scope.dart:550-554).
#[test]
fn wildcard_pattern_binder_read_reports() {
    let report = report_ops("void f() { var (_, y) = (1, 2); y; _; }\n");
    let codes = code_names(&report);
    assert!(
        codes.contains(&"undefined_identifier"),
        "{codes:?} {report:?}"
    );
}

// A name merely starting with `_` is ordinary: `_x` binds and the read is
// silent (the rule is the exact spelling `_`, extensions.dart:81).
#[test]
fn underscore_prefixed_name_still_resolves() {
    let report = report_ops("void f() { var _x = 1; _x; }\n");
    assert_silent(&report);
}

// A TOP-LEVEL `_` keeps binding — the skip is a frame-scope rule
// (LocalScope/formal scopes); library-scope names are untouched
// (scope.dart:537-557 covers locals only).
#[test]
fn top_level_wildcard_still_resolves() {
    let report = report_ops("int _ = 0;\nvoid g() { _; }\n");
    assert_silent(&report);
}

// A `_` catch clause parameter binds nothing and stays exempt from
// unused-catch reporting (the tracked row remains; only the binding is
// skipped).
#[test]
fn wildcard_catch_parameter_is_silent() {
    let report = report_ops("void f() { try {} catch (_) {} }\n");
    assert_silent(&report);
}

// The tracked-row split: an unused `_` local stays exempt from
// `unused_local_variable` (the wildcard exemption walks the tracked vec,
// not the resolution set), while an unused ordinary name still reports —
// the vec kept working.
#[test]
fn unused_wildcard_local_stays_exempt() {
    let report = report_ops("void f() { var _ = 1; var q = 2; }\n");
    let codes = code_names(&report);
    assert!(codes.contains(&"unused_local_variable"), "{codes:?}");
    assert_eq!(
        codes
            .iter()
            .filter(|code| **code == "unused_local_variable")
            .count(),
        1,
        "the `_` row must stay exempt: {codes:?}"
    );
}

// Pre-feature posture (a pre-3.7 library): `_` is an ordinary name — the
// local binds and the read is silent (extensions.dart:81's feature gate;
// scope.dart:550-554's skip is feature-conditioned for locals).
#[test]
fn pre_feature_underscore_local_binds() {
    let report = report_ops_wildcards("void f() { var _ = 1; _; }\n", false);
    assert_silent(&report);
}
