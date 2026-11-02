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
//! Before this ticket dartq stayed silent: the fold produced the
//! substituted `Ty::Function` shape, but the assignability path never
//! reached the bound-equality leg, so the written `F<...>` pair read as
//! two unrelated named heads (or as an undecidable pair) and no row was
//! staged. The pins below pin the report and the agreeing control.

use super::test_rig::{RigImport, code_names, report_ops_with};

/// The shared `lang_feature_check.dart` face the co19 corpus mounts:
/// `typedef F<X> = void Function<Y extends X>()` plus `toF`.
const LANG_FEATURE_CHECK: &str = r#"
typedef F<X> = void Function<Y extends X>();
F<X> toF<X>(X x) => <Y extends X>() {};
"#;

/// Pin: `F<A<int>?>` into `F<A<num>?>?` — the signatures' own `Y` bounds
/// differ (`A<int>?` vs `A<num>?`), and neither is a subtype of the other
/// in both directions; the oracle reports `invalid_assignment` on the
/// assigned expression.
#[test]
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

#[test]
fn probe_inferred_type() {
    // If toF inference produced F<A<int>?>, assigning fsource to
    // F<A<int>?>? must be silent and to F<A<num>?>? must report.
    let report = report_ops_with(
        r#"import 'package:x/lang_feature_check.dart';

class num {} class int extends num {}

class A<X extends num> {}

main() {
  A<int>? source;
  var fsource = toF(source);
  F<A<int>?>? same = fsource;
}
"#,
        &[RigImport {
            uri: "package:x/lang_feature_check.dart",
            prefix: None,
            source: LANG_FEATURE_CHECK,
        }],
        None,
    );
    panic!("inferred-same: {:?}", code_names(&report));
}

#[test]
fn probe_inferred_shape() {
    // If fsource is F<A<int>?>, it cannot assign to int — a report proves
    // the inferred type is function-typed (not dynamic).
    let report = report_ops_with(
        r#"import 'package:x/lang_feature_check.dart';

class num {} class int extends num {}

class A<X extends num> {}

main() {
  A<int>? source;
  var fsource = toF(source);
  int n = fsource;
}
"#,
        &[RigImport {
            uri: "package:x/lang_feature_check.dart",
            prefix: None,
            source: LANG_FEATURE_CHECK,
        }],
        None,
    );
    panic!("inferred-shape: {:?}", code_names(&report));
}

#[test]
fn probe_explicit_targ() {
    let report = report_ops_with(
        r#"import 'package:x/lang_feature_check.dart';

class num {} class int extends num {}

class A<X extends num> {}

main() {
  A<int>? source;
  var fsource = toF<A<int>?>(source);
  int n = fsource;
}
"#,
        &[RigImport {
            uri: "package:x/lang_feature_check.dart",
            prefix: None,
            source: LANG_FEATURE_CHECK,
        }],
        None,
    );
    panic!("explicit-targ: {:?}", code_names(&report));
}

#[test]
fn probe_direct_call_result() {
    let report = report_ops_with(
        r#"import 'package:x/lang_feature_check.dart';

class num {} class int extends num {}

class A<X extends num> {}

main() {
  A<int>? source;
  int n = toF<A<int>?>(source);
}
"#,
        &[RigImport {
            uri: "package:x/lang_feature_check.dart",
            prefix: None,
            source: LANG_FEATURE_CHECK,
        }],
        None,
    );
    panic!("direct: {:?}", code_names(&report));
}

#[test]
fn probe_same_file_tof() {
    let report = report_ops_with(
        r#"class num {} class int extends num {}

typedef F<X> = void Function<Y extends X>();
F<X> toF<X>(X x) => <Y extends X>() {};

class A<X extends num> {}

main() {
  A<int>? source;
  int n = toF<A<int>?>(source);
}
"#,
        &[],
        None,
    );
    panic!("same-file: {:?}", code_names(&report));
}

#[test]
fn probe_nonfn_return() {
    // Control for the call-result path itself: a generic call returning a
    // NON-function type. If `int n = id<int>(5)` reports (it shouldn't)
    // and `String s = id<int>(5)` reports (it should), instantiation works.
    let report = report_ops_with(
        r#"class num {} class int extends num {} class String {}

T id<T>(T t) => t;

main() {
  String s = id<int>(5);
}
"#,
        &[],
        None,
    );
    panic!("nonfn: {:?}", code_names(&report));
}

#[test]
fn probe_same_file_var_infer() {
    let report = report_ops_with(
        r#"class num {} class int extends num {}

typedef F<X> = void Function<Y extends X>();
F<X> toF<X>(X x) => <Y extends X>() {};

class A<X extends num> {}

main() {
  A<int>? source;
  var fsource = toF(source);
  F<A<num>?>? target = fsource;
}
"#,
        &[],
        None,
    );
    panic!("same-file-infer: {:?}", code_names(&report));
}

#[test]
fn probe_var_infer_nongeneric_ret() {
    let report = report_ops_with(
        r#"class num {} class int extends num {} class String {}

T id<T>(T t) => t;

main() {
  var x = id<int>(5);
  String s = x;
}
"#,
        &[],
        None,
    );
    panic!("var-nonfn: {:?}", code_names(&report));
}

#[test]
fn probe_var_infer_inferred() {
    let report = report_ops_with(
        r#"class num {} class int extends num {} class String {}

T id<T>(T t) => t;

main() {
  var x = id(5);
  String s = x;
}
"#,
        &[],
        None,
    );
    panic!("var-infer: {:?}", code_names(&report));
}

#[test]
fn probe_typed_decl_send() {
    let report = report_ops_with(
        r#"class num {} class int extends num {}

typedef F<X> = void Function<Y extends X>();
F<X> toF<X>(X x) => <Y extends X>() {};

class A<X extends num> {}

main() {
  A<int>? source;
  F<A<int>?>? fsource = toF(source);
  F<A<num>?>? target = fsource;
}
"#,
        &[],
        None,
    );
    panic!("typed-send: {:?}", code_names(&report));
}

#[test]
fn probe_var_infer_var_arg() {
    let report = report_ops_with(
        r#"class num {} class int extends num {} class String {}

T id<T>(T t) => t;

main() {
  int y = 5;
  var x = id(y);
  String s = x;
}
"#,
        &[],
        None,
    );
    panic!("var-arg: {:?}", code_names(&report));
}

#[test]
fn probe_var_bind_direct() {
    let report = report_ops_with(
        r#"class num {} class int extends num {}

typedef F<X> = void Function<Y extends X>();
F<X> toF<X>(X x) => <Y extends X>() {};

class A<X extends num> {}

main() {
  A<int>? source;
  var fsource = toF(source);
  int n = fsource;
}
"#,
        &[],
        None,
    );
    panic!("varbind: {:?}", code_names(&report));
}

#[test]
fn probe_var_nongeneric_alias_ret() {
    let report = report_ops_with(
        r#"class num {} class int extends num {}

typedef G = void Function();
G g() => () {};

main() {
  var f = g();
  int n = f;
}
"#,
        &[],
        None,
    );
    panic!("nongeneric-alias: {:?}", code_names(&report));
}

#[test]
fn probe_var_generic_int_ret() {
    // `F<X>` where F is a typedef whose RHS is a NAMED type, not function.
    let report = report_ops_with(
        r#"class num {} class int extends num {} class String {}

typedef Box<T> = List<T>;
Box<X> mk<X>(X x) => [x];

class List<E> {}

main() {
  var b = mk<int>(5);
  String s = b;
}
"#,
        &[],
        None,
    );
    panic!("named-alias: {:?}", code_names(&report));
}

#[test]
fn probe_var_bind_final() {
    let report = report_ops_with(
        r#"class num {} class int extends num {}

typedef F<X> = void Function<Y extends X>();
F<X> toF<X>(X x) => <Y extends X>() {};

class A<X extends num> {}

main() {
  A<int>? source;
  final fsource = toF(source);
  int n = fsource;
}
"#,
        &[],
        None,
    );
    panic!("finalbind: {:?}", code_names(&report));
}

#[test]
fn probe_var_bind_nosolve() {
    // A non-generic function returning a Function-typed alias value:
    // `var f = mk()` where `mk` returns `void Function()` — does `f` bind?
    let report = report_ops_with(
        r#"class num {} class int extends num {}

typedef G = void Function();
G mk() => () {};

main() {
  var f = mk();
  int n = f;
}
"#,
        &[],
        None,
    );
    panic!("nosolve: {:?}", code_names(&report));
}

#[test]
fn probe_explicit_decl() {
    let report = report_ops_with(
        r#"import 'package:x/lang_feature_check.dart';

class num {} class int extends num {}

class A<X extends num> {}

main() {
  F<A<int>?>? fsource;
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
    panic!("explicit-decl: {:?}", code_names(&report));
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
