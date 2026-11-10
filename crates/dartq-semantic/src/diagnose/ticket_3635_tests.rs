//! #3635 pins — the co19 `toF` family's value face is the alias use
//! `F<G<…>>`, never the expanded signature `void Function<Y extends
//! G<…>>()`.
//!
//! # The defect
//!
//! The 1,101 missing `invalid_assignment` rows of the co19 `toF` family
//! (`Simple-bounds` / `Instantiate-to-bound` / `nonfunction_typedef` /
//! `class`) were a pure DISPLAY defect: the engine emitted the
//! verdict-correct row at the oracle's exact (file, position, length)
//! but spelled the substituted send result from the callee's EXPANDED
//! signature face. [`Engine::instantiate_call_return`] folded the
//! alias-spelled `Named{F, [solved args]}` through `fold_alias_ty`,
//! publishing `void Function<Y extends G<A<Never>, A<Never>>>()` where
//! the oracle prints `F<G<A<Never>, A<Never>>>` (upstream the
//! instantiated `FunctionTypeImpl.alias` rides the static type and
//! `getDisplayString(preferTypeAlias: true)` renders the use form).
//! The send's substituted return now keeps its `Named{F, …}` face; the
//! verdict layer unfolds it at the ask (`fold_alias_verdict`), so no
//! judgment reads a weaker face.
//!
//! # The rig
//!
//! The real `Instantiate-to-bound/typedef/static/typedef_l2_03_t15`
//! fixture verbatim: `typedef G<X extends Y, Y extends A<Y>>` raw-use
//! instantiated to `G<A<Never>, A<Never>>`, bound into `F<X> toF<X>(X
//! x)`'s `X` — 25 target declarations, 23 reporting the alias face and
//! the two expansion-equal silences (`F<G<A<dynamic>, A<Never>>>` and
//! `F<G<A<Never>, A<Never>>>` — `G`'s expansion keeps only `Y`, so
//! those targets expand equal to the value and stay silent) staying
//! silent exactly as the oracle does.

use super::FileReport;
use super::test_rig::{RigImport, report_ops_with, rows};

/// The foreign `co19/Utils/lang_feature_check.dart` face the corpus
/// mounts (`part of 'expect.dart'`, imported across the library edge):
/// `typedef F<X> = void Function<Y extends X>()` plus `toF`.
const LANG_FEATURE_CHECK: &str = r#"
typedef F<X> = void Function<Y extends X>();
F<X> toF<X>(X x) => <Y extends X>() {};
"#;

/// The real `typedef_l2_03_t15` body: `class A<X>` plus the two-parameter
/// bound-cycle typedef `G`, then the 25 target declarations of the
/// `test(G source)` body.
const CASE_BODY: &str = r#"
class A<X> {}
typedef G<X extends Y, Y extends A<Y>> = void Function(Y);

test(G source) {
  var fsource = toF(source);

  F<G<A<dynamic>, Never>> target1 = fsource;
  F<G<A<Never>, Never>> target2 = fsource;
  F<G<A<Null>, Never>> target3 = fsource;
  F<G<dynamic, Never>> target4 = fsource;
  F<G<Never, Never>> target5 = fsource;
  F<G<Null, Never>> target6 = fsource;
  F<G<A<dynamic>, dynamic>> target7 = fsource;
  F<G<A<Never>, dynamic>> target8 = fsource;
  F<G<A<Null>, dynamic>> target9 = fsource;
  F<G<dynamic, dynamic>> target10 = fsource;
  F<G<Never, dynamic>> target11 = fsource;
  F<G<Null, dynamic>> target12 = fsource;
  F<G<A<dynamic>, Null>> target13 = fsource;
  F<G<A<Never>, Null>> target14 = fsource;
  F<G<A<Null>, Null>> target15 = fsource;
  F<G<dynamic, Null>> target16 = fsource;
  F<G<Never, Null>> target17 = fsource;
  F<G<Null, Null>> target18 = fsource;
  F<G<A<Never>, A<dynamic>>> target19 = fsource;
  F<G<A<dynamic>, A<dynamic>>> target20 = fsource;
  F<G<A<dynamic>, A<Never>>> target21 = fsource;
  F<G<A<Never>, A<Never>>> target22 = fsource;
  F<G<A<dynamic>, A<Null>>> target23 = fsource;
  F<G<A<Null>, A<dynamic>>> target24 = fsource;
  F<G<A<Null>,  A<Null>>> target25 = fsource;
}

main() {
  G? source;
  G == int;
}
"#;

fn case_report() -> FileReport {
    report_ops_with(
        CASE_BODY,
        &[RigImport {
            uri: "package:x/lang_feature_check.dart",
            prefix: None,
            source: LANG_FEATURE_CHECK,
        }],
        None,
    )
}

/// Every emitted row's value face is the alias-spelled
/// `F<G<A<Never>, A<Never>>>` — the oracle's `preferTypeAlias` form —
/// and no row carries the expanded signature the defect published.
#[test]
fn t3635_value_face_is_alias_spelled() {
    let report = case_report();
    let misses: Vec<&str> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(
        misses.len(),
        23,
        "expected the 23 oracle rows, got {:?}",
        rows(&report)
    );
    for miss in &misses {
        assert!(
            miss.contains("type 'F<G<A<Never>, A<Never>>>'"),
            "the value face must spell the alias use, got {miss:?}"
        );
        assert!(
            !miss.contains("Function<Y extends"),
            "the expanded signature must not leak, got {miss:?}"
        );
    }
}

/// The two expansion-equal targets stay silent: `G`'s expansion keeps
/// only its `Y` parameter, so `F<G<A<dynamic>, A<Never>>>` (target21)
/// and `F<G<A<Never>, A<Never>>>` (target22) expand equal to the
/// value's own `G<A<Never>, A<Never>>` — the verdict layer already
/// proved it, and the alias-spelled face must not resurrect them.
#[test]
fn t3635_expansion_equal_targets_stay_silent() {
    let report = case_report();
    for target in ["F<G<A<dynamic>, A<Never>>>", "F<G<A<Never>, A<Never>>>"] {
        assert!(
            !report
                .diagnostics
                .iter()
                .any(|d| d.message.contains(&format!("variable of type '{target}'"))),
            "expected no row against target {target}, got {:?}",
            rows(&report)
        );
    }
}
