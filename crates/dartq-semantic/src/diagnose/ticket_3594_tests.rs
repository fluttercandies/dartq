//! #3594 pins — generic-function-typedef bound equality: `F<S>` vs `F<T>`
//! over `typedef F<X> = void Function<Y extends X>()`.
//!
//! The co19 `Simple-bounds`/`Instantiate-to-bound`/`InferredTypeTest`
//! families assign a `toF(source)`-produced `F<BoundA>` into an
//! `F<BoundB>?` variable, where `BoundA` and `BoundB` differ only in
//! nullability or a nested argument (`A<int>?` vs `A<num>?`). Upstream's
//! `_isFunctionSubtypeOf` runs `relateTypeParameters` ahead of its
//! return/formal checks: the signatures' own type-parameter bounds must be
//! EQUAL (mutual subtypes), so `void Function<Y extends A<int>?>` is a
//! subtype of `void Function<Y extends A<num>?>` in NEITHER direction —
//! the oracle's `invalid_assignment` row fires.
//!
//! Before this ticket dartq stayed silent on the `var`-inferred shape:
//! `var fsource = toF(source)` binds the send's result, and the send-close
//! fold leaves the callee's instantiated function face on the row
//! (`void Function<Y extends A<int>?>`). `operand_static_ty`'s
//! `solved_call_params` arm misread that face as the uninvoked signature
//! and `callable_result_of` could not unwrap its own type-parameter count,
//! so the local bound `Unknown` and the bound-equality legs never ran.
//! [`Engine::var_inferred_send_init_ty`] re-derives the send's honest
//! result through `callee_return_ty` at the check, so the read spells
//! `F<A<int>?>` and the verdict decides.

use super::test_rig::{RigImport, code_names, report_ops_with};

/// The shared `lang_feature_check.dart` face the co19 corpus mounts:
/// `typedef F<X> = void Function<Y extends X>()` plus `toF`.
const LANG_FEATURE_CHECK: &str = r#"
typedef F<X> = void Function<Y extends X>();
F<X> toF<X>(X x) => <Y extends X>() {};
"#;

/// Pin: `var fsource = toF(source)` infers `F<A<int>?>`; assigning it into
/// `F<A<num>?>?` — the signatures' own `Y` bounds differ (`A<int>?` vs
/// `A<num>?`), and neither is a subtype of the other in both directions —
/// reports `invalid_assignment` on the assigned expression.
#[test]
// un-ignored 2026-11-07: passing per stale-ignore triage (ticket #3973)
fn bounds_differ_nested_nullability_reports() {
    let report = report_ops_with(
        r#"import 'package:x/lang_feature_check.dart';

class num {} class int extends num {}

class A<X extends num> {}

main() {
  A<int>? source;
  var fsource = toF(source);
  F<A<num>?>? target = fsource;
}
"#,
        &[RigImport {
            uri: "package:x/lang_feature_check.dart",
            prefix: None,
            source: LANG_FEATURE_CHECK,
        }],
        None,
    );
    assert!(
        code_names(&report).contains(&"invalid_assignment"),
        "expected invalid_assignment, got {:?}",
        code_names(&report)
    );
}

/// Control: `F<A<num>?>` into `F<A<num>?>?` — identical bounds; the oracle
/// stays silent.
#[test]
fn bounds_equal_stays_silent() {
    let report = report_ops_with(
        r#"import 'package:x/lang_feature_check.dart';

class num {} class int extends num {}

class A<X extends num> {}

main() {
  A<num>? source;
  var fsource = toF(source);
  F<A<num>?>? target = fsource;
}
"#,
        &[RigImport {
            uri: "package:x/lang_feature_check.dart",
            prefix: None,
            source: LANG_FEATURE_CHECK,
        }],
        None,
    );
    assert!(
        !code_names(&report).contains(&"invalid_assignment"),
        "expected no invalid_assignment, got {:?}",
        code_names(&report)
    );
}
