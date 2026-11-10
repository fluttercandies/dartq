//! #3700 pins — `unused_element` enqueue/exemption verdicts against the
//! oracle's `UnusedLocalElementsVerifier` (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/error/unused_local_elements_verifier.dart`).
//!
//! Clusters covered:
//!   - the enum declared-constructor self-leak (`enum_declares_ctor`): the
//!     oracle leaks `usedElements.elements` only through a *declared*
//!     constructor's `returnType` NamedType — an implicit unnamed
//!     constructor writes no `NamedType`, so a constructorless enum still
//!     reports (`visitEnumDeclaration` → `_visitClassElement`).
//!   - the operator display-name fold: `operator []=`'s element
//!     `displayName` keeps `[]=` — the setter-encoding strip must not run
//!     on operator spellings.
//!   - wildcard `_` containers: a `_` spelled class/mixin/enum/extension
//!     type is a private-named declaration like any other — the wildcard
//!     exemption applies to LOCALS only (`_isNamedWildcard`,
//!     upstream :985-994).

use super::test_rig::{assert_silent, report_ops, report_ops_wildcards, rows};

fn unused_element(source: &str) -> Vec<String> {
    rows(&report_ops(source))
        .into_iter()
        .filter(|row| row.starts_with("unused_element"))
        .collect()
}

// —— enum declared-constructor self-leak ——————————————————————————————

/// `enum _E { e0; }` declares no constructor: the implicit unnamed one
/// writes no `returnType` NamedType, so the oracle's self-leak never arms
/// and `_E` reports (upstream :590-595 → `_isUsedElement`).
#[test]
fn enum_with_only_implicit_ctor_reports() {
    let rows = unused_element("enum _E { e0; }\nmain() {}\n");
    assert_eq!(
        rows.len(),
        1,
        "expected one unused_element row, got {rows:?}"
    );
    assert!(
        rows[0].ends_with("The declaration '_E' isn't referenced."),
        "{rows:?}"
    );
}

/// The leak needs a DECLARED constructor: `const _E(this._f)`'s
/// `returnType` NamedType visits `_useIdentifierElement` with
/// `_enclosingClass == null` — the enum element joins
/// `usedElements.elements` and the declaration stays silent.
#[test]
fn enum_with_declared_ctor_is_silent() {
    let rows = unused_element("enum _E { e0(0); const _E(this._f); final int _f; }\nmain() {}\n");
    assert!(rows.is_empty(), "{rows:?}");
}

/// A same-spelling class must not inherit the enum's leak: `class _`
/// after `enum _ {e1;}` resolves to the enum's binding, but the verdict
/// belongs to the class element — both report (the oracle walks every
/// declaration's own element).
#[test]
fn enum_leak_does_not_spare_same_named_class() {
    let rows = unused_element("enum _ {e1;}\nclass _ {}\nmain() {}\n");
    assert_eq!(
        rows.len(),
        2,
        "expected unused_element for both `_` decls, got {rows:?}"
    );
    assert!(
        rows.iter()
            .all(|row| row.ends_with("The declaration '_' isn't referenced.")),
        "{rows:?}"
    );
}

// —— operator display name ————————————————————————————————————————————

/// `test_method_notUsed_privateExtension_indexEqOperator`: a member of a
/// private-named extension checks like a private member, and `[]=`'s
/// `displayName` keeps the `=` — the oracle reports `'[]='`, never `'[]'`.
#[test]
fn private_extension_index_setter_reports_bare_brackets() {
    let rows = unused_element(
        "extension _A on bool {\n  operator []=(int index, int value) {}\n}\nmain() {}\n",
    );
    assert_eq!(
        rows.len(),
        1,
        "expected one unused_element row, got {rows:?}"
    );
    assert!(
        rows[0].ends_with("The declaration '[]=' isn't referenced."),
        "{rows:?}"
    );
}

/// Silence: a public extension's members are publicly accessible — the
/// `_isPubliclyAccessible` walk reaches `operator []=` without any use.
#[test]
fn public_extension_index_setter_is_silent() {
    let rows = unused_element(
        "extension A on bool {\n  operator []=(int index, int value) {}\n}\nmain() {}\n",
    );
    assert!(rows.is_empty(), "{rows:?}");
}

// —— wildcard `_` containers ——————————————————————————————————————————

/// `other_declarations_A04_t05` (wildcard-variables enabled): top-level
/// `_` declarations are ordinary names, not wildcards — every private `_`
/// container reports `unused_element`.
#[test]
fn wildcard_spelled_containers_all_report() {
    let report = report_ops_wildcards(
        "class _ {}\nmixin _ {}\nenum _ {e1;}\nextension type _(int id) {}\nmain() {}\n",
        true,
    );
    let unused: Vec<String> = rows(&report)
        .into_iter()
        .filter(|row| {
            row.starts_with("unused_element")
                && row.ends_with("The declaration '_' isn't referenced.")
        })
        .collect();
    assert_eq!(unused.len(), 4, "{:#?}", rows(&report));
}

/// Silence: a `_` spelling that is genuinely read stays silent — the
/// `_()` creation resolves the class's unnamed constructor (the folded
/// creation head's element credit).
#[test]
fn wildcard_class_used_by_creation_is_silent() {
    let report = report_ops_wildcards("class _ {}\nmain() { _(); }\n", true);
    let unused: Vec<String> = rows(&report)
        .into_iter()
        .filter(|row| {
            row.starts_with("unused_element")
                && row.ends_with("The declaration '_' isn't referenced.")
        })
        .collect();
    assert!(unused.is_empty(), "{:#?}", rows(&report));
}

/// Silence: a named private extension is never visited upstream (the
/// verifier has no `visitExtensionDeclaration`), so `extension _ on A`
/// reports nothing — while the unrelated `class _` still reports.
#[test]
fn private_extension_decl_is_never_visited() {
    let report = report_ops_wildcards(
        "class _ {}\nclass A {}\nextension _ on A {}\nmain() {}\n",
        true,
    );
    let unused: Vec<String> = rows(&report)
        .into_iter()
        .filter(|row| row.starts_with("unused_element"))
        .collect();
    assert_eq!(
        unused.len(),
        1,
        "expected only `class _` to report, got {unused:?}"
    );
    assert!(
        unused[0].ends_with("The declaration '_' isn't referenced."),
        "{unused:?}"
    );
}

// —— named function expressions ———————————————————————————————————————

/// A named function expression (`void f() {}` in expression position) is
/// `visitFunctionExpression`'s `addElement` face upstream — its element is
/// used by construction and never reports `unused_element`
/// (unused_local_elements_verifier.dart:167-172). Our local-function
/// enqueue (listener.rs `C::LocalFunctionDeclaration` →
/// `finish_local_fn_site` under `end_named_function_expression`) feeds it
/// to the `LocalKind::LocalFunction` arm instead — the
/// `named_function_expression` parser row is correct, the
/// `unused_element 'f'` is the extra.
#[test]
fn named_function_expression_never_reports_unused_element() {
    let rows = unused_element("f() { assert (a, void f() {} }\n");
    assert!(rows.is_empty(), "{rows:?}");
}
