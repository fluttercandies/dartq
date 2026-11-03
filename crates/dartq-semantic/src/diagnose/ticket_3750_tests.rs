//! #3750 — `type_argument_not_matching_bounds` false-positive reduction.
//!
//! Census 20261103-133158: 250 missing / 641 extra for the code. The extra
//! side clusters into the `expectStaticType<Exactly<T>>` inference family
//! (wrong receiver static type, owned by the expression-typing lanes) and
//! a self-referential generic-function-type bound family — a bound whose
//! expansion is a generic function type rejects an argument that expands
//! to the same interned type (`'exp1' doesn't conform to the bound
//! 'exp1'`). The oracle stays silent on every self-pair; these pins
//! capture the verdict.
//!
//! Oracle provenance: `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze`
//! over single-file fixtures.

use super::test_rig::{assert_silent, report_ops, rows};

/// The `type_argument_not_matching_bounds` rows of `report`.
fn bounds_rows(report: &super::FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "type_argument_not_matching_bounds")
        .map(|d| format!("{} @{}: {}", d.code.name, d.range.offset, d.message))
        .collect()
}

/// The co19 typedef3 shape: a 0-parameter alias to a generic function
/// type used both as the bound (`X extends exp1`) and as the argument
/// (`C1<exp1>`). Both sides expand to the same interned function type —
/// the oracle is silent.
#[test]
fn gft_alias_self_pair_stays_silent() {
    let report = report_ops(
        "typedef exp1 = T Function<T>(T);\n\
         typedef exp2 = void Function<T>();\n\
         typedef exp3 = T Function<T>();\n\
         typedef exp4 = void Function<T>(T);\n\
         class C<T> { C(expected); }\n\
         typedef C1<X extends exp1> = C<X>;\n\
         typedef C2<X extends exp2> = C<X>;\n\
         typedef C3<X extends exp3> = C<X>;\n\
         typedef C4<X extends exp4> = C<X>;\n\
         void f() {\n\
         \x20 C1<exp1>(1);\n\
         \x20 C2<exp2>(1);\n\
         \x20 C3<exp3>(1);\n\
         \x20 C4<exp4>(1);\n\
         }\n",
    );
    assert_eq!(bounds_rows(&report), Vec::<String>::new(), "{:#?}", rows(&report));
}

/// A container's own parameter bounded by a GFT alias rejects the same
/// alias as its argument — same self-pair, non-typedef owner.
#[test]
fn gft_alias_bound_on_class_self_pair_stays_silent() {
    let report = report_ops(
        "typedef exp1 = T Function<T>(T);\n\
         class C<T> { C(expected); }\n\
         class C1<X extends exp1> extends C<X> { C1(expected) : super(expected); }\n\
         void f() {\n\
         \x20 C1<exp1>(1);\n\
         }\n",
    );
    assert_eq!(bounds_rows(&report), Vec::<String>::new(), "{:#?}", rows(&report));
}

/// The genuine violation keeps reporting: a concrete argument that does
/// not conform to the GFT-alias bound fires exactly once (the oracle
/// reports `'int' doesn't conform to the bound 'exp1'`).
#[test]
fn gft_alias_bound_violation_reports() {
    let report = report_ops(
        "typedef exp1 = T Function<T>(T);\n\
         class C<T> { C(expected); }\n\
         typedef C1<X extends exp1> = C<X>;\n\
         void f() {\n\
         \x20 C1<int>(1);\n\
         }\n",
    );
    assert_eq!(bounds_rows(&report).len(), 1, "{:#?}", rows(&report));
}
