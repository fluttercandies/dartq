//! #3609 pins — `argument_type_not_assignable` private-foreign-typedef family.
//!
//! The census (20261102-095230) reports the code at 498 missing / 563 extra;
//! the dominant extra family (70 rows, co19 `Expect.throws`) spells a
//! formal through an unnameable private typedef — `_CheckExceptionFn?` —
//! whose resolved `staticType` is `bool Function(dynamic)?` inside the
//! declaring library regardless of use-site visibility. Each pin below
//! carries the oracle's own answer (dart analyze 3.13.7,
//! `--format=machine`).

use super::test_rig::{code_names, report_ops_imports, rows, RigImport};

/// The co19 `Expect.throws` face verbatim: a class-static send whose
/// optional `check` formal spells a PRIVATE function typedef of the
/// imported library. The oracle folds the alias inside the declaring
/// library — `bool Function(dynamic)?` — and a `bool Function(dynamic)`
/// closure argument assigns clean. Oracle 3.13.7: silent.
#[test]
fn private_foreign_fn_typedef_formal_accepts_closure() {
    let report = report_ops_imports(
        "void main() { Expect.throws(() {}, (e) => e is int); }",
        &[RigImport {
            uri: "package:t/expect.dart",
            prefix: None,
            source: "typedef bool _CheckExceptionFn(exception); class Expect { static void throws(void func(), [_CheckExceptionFn? check, String reason = '']) {} }",
        }],
    );
    assert!(
        code_names(&report).is_empty(),
        "unexpected rows: {:?}",
        rows(&report)
    );
}

/// Public-name control: the same shape with an exported typedef was
/// already clean — the fold's scoped arm never regressed it. Oracle:
/// silent.
#[test]
fn public_foreign_fn_typedef_formal_accepts_closure() {
    let report = report_ops_imports(
        "void main() { Expect.throws(() {}, (e) => e is int); }",
        &[RigImport {
            uri: "package:t/expect.dart",
            prefix: None,
            source: "typedef bool CheckExceptionFn(exception); class Expect { static void throws(void func(), [CheckExceptionFn? check, String reason = '']) {} }",
        }],
    );
    assert!(
        code_names(&report).is_empty(),
        "unexpected rows: {:?}",
        rows(&report)
    );
}

/// The private typedef folded to a NAMED rhs: `typedef _C = int;` — a
/// private nominal alias's formal accepts the matching argument. Oracle
/// 3.13.7 on `void take2(int x) {}`-equivalent: silent.
#[test]
fn private_foreign_named_typedef_formal_accepts() {
    let report = report_ops_imports(
        "void main() { take2(42); }",
        &[RigImport {
            uri: "package:t/alias.dart",
            prefix: None,
            source: "typedef _C = int; void take2(_C x) {}",
        }],
    );
    assert!(
        code_names(&report).is_empty(),
        "unexpected rows: {:?}",
        rows(&report)
    );
}

/// Fabrication guard — the chase's fold decides BOTH directions through
/// the refused arm: a foreign private alias folded to a NON-function
/// head (`typedef _F = int;`) still refuses a function argument, so the
/// fold-to-function gate narrows fabrication without silencing genuine
/// mismatches. Oracle: `argument_type_not_assignable` with the written
/// `_F` display.
#[test]
fn private_alias_still_reports_genuine_mismatch() {
    let report = report_ops_imports(
        "void main() { take(() => 1); }",
        &[RigImport {
            uri: "package:t/alias.dart",
            prefix: None,
            source: "typedef _F = int; void take(_F f) {}",
        }],
    );
    assert_eq!(
        code_names(&report),
        vec!["argument_type_not_assignable"],
        "rows: {:?}",
        rows(&report)
    );
}

/// Ambiguity guard — two unrelated libraries declaring the same private
/// typedef spelling decline the fold (under-report direction, never a
/// fabricated match): the prefixed import's `b._F` is unreachable through
/// the bare seam, so the verdict walks the single reachable declaration.
/// The `int` argument fits `int?` — oracle silent, and the unreachable
/// same-named declaration must not flip that either way.
#[test]
fn ambiguous_private_typedef_declines_fold() {
    let report = report_ops_imports(
        "void main() { take2(7); }",
        &[
            RigImport {
                uri: "package:t/a.dart",
                prefix: None,
                source: "typedef _F = int; void take2(_F? x) {}",
            },
            RigImport {
                uri: "package:t/b.dart",
                prefix: Some("b"),
                source: "typedef _F = int;",
            },
        ],
    );
    assert!(
        code_names(&report).is_empty(),
        "unexpected rows: {:?}",
        rows(&report)
    );
}
