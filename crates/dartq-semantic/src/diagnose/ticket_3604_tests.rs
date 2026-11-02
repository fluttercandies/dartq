use super::test_rig::{code_names, report_ops, rows};

// Extra-row pin (the public-typedef exposure face,
// `visitGenericTypeAlias`, unused_local_elements_verifier.dart:175-190): a
// non-private `typedef A = _E` adds every non-private constructor of the
// aliased interface type to `usedElements.elements`, so the private
// extension type's public-named constructor resolves through the alias
// and must NOT report — telling the user `_E.named` is unreachable while
// `A.named()` exists is the fabrication this face prevents. Oracle
// 3.13.7: silent.
#[test]
fn private_extension_type_ctor_exposed_via_public_typedef_stays_silent() {
    let report = report_ops(
        r#"
extension type _E(int i) {
  _E.named(this.i);
}
typedef A = _E;
"#,
    );
    assert!(
        !code_names(&report).contains(&"unused_element"),
        "unexpected unused_element: {:#?}",
        rows(&report)
    );
}

// The same exposure through a private middle alias: `typedef _A = _E`
// contributes nothing itself (the oracle's `isPrivateName` gate on the
// alias name), but `typedef B = _A` resolves the chain to `_E` and
// exposes its public-named constructors. Oracle 3.13.7: silent.
#[test]
fn private_extension_type_ctor_exposed_via_indirect_typedef_stays_silent() {
    let report = report_ops(
        r#"
extension type _E(int i) {
  _E.named(this.i);
}
typedef _A = _E;
typedef B = _A;
"#,
    );
    assert!(
        !code_names(&report).contains(&"unused_element"),
        "unexpected unused_element: {:#?}",
        rows(&report)
    );
}

// The private-constructor half of the same face: the alias exposes only
// the NON-private constructor spellings (`!Identifier.isPrivateName` in
// the oracle's loop), so `_C._hidden` still reports while `Pub` spares
// `_C.named`. Oracle 3.13.7:
//   The declaration '_C._hidden' isn't referenced.
#[test]
fn private_ctor_of_aliased_class_still_reports() {
    let report = report_ops(
        r#"
class _C {
  _C();
  _C.named();
  _C._hidden();
}
typedef Pub = _C;
"#,
    );
    let codes = code_names(&report);
    assert!(
        codes.contains(&"unused_element"),
        "expected unused_element for _C._hidden: {:#?}",
        rows(&report)
    );
    let hidden_reported = rows(&report)
        .iter()
        .any(|row| row.contains("_C._hidden"));
    assert!(
        hidden_reported,
        "expected the _C._hidden row specifically: {:#?}",
        rows(&report)
    );
    let named_reported = rows(&report)
        .iter()
        .any(|row| row.contains("_C.named'"));
    assert!(
        !named_reported,
        "the alias must spare _C.named: {:#?}",
        rows(&report)
    );
}

// Genuine-unused control: the same private extension type WITHOUT the
// exposing typedef still reports its named constructor — the face spares
// through the alias, not through the declaration shape. Oracle 3.13.7:
//   The declaration '_F.named' isn't referenced.
#[test]
fn private_extension_type_ctor_without_typedef_still_reports() {
    let report = report_ops(
        r#"
extension type _F(int i) {
  _F.named(this.i);
}
"#,
    );
    assert!(
        code_names(&report).contains(&"unused_element"),
        "expected unused_element for _F.named: {:#?}",
        rows(&report)
    );
}

// The private-alias arm of the oracle gate: `typedef _Priv = _E` is a
// private-named declaration, so `visitGenericTypeAlias` returns before
// the constructor walk — `_E.named` keeps reporting. Oracle 3.13.7:
//   The declaration '_E.named' isn't referenced.
#[test]
fn private_typedef_does_not_expose() {
    let report = report_ops(
        r#"
extension type _E(int i) {
  _E.named(this.i);
}
typedef _Priv = _E;
"#,
    );
    assert!(
        code_names(&report).contains(&"unused_element"),
        "expected unused_element for _E.named: {:#?}",
        rows(&report)
    );
}
