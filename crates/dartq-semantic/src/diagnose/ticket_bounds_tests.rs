//! #3209 pins — bounds_conformance: the `isSubtypeOf`
//! instantiate-to-bounds residual, the super-bounds conformance arm, and
//! the generic-bound residual (F6-A+I+E+H+F3 folded into one
//! mega-ticket).
//!
//! The oracle's `type_system.dart` `isSubtypeOf` runs two passes dartq
//! already mirrors through `bounds.rs`: (a) a bare generic head whose
//! type arguments are NOT written solves each parameter to its bound —
//! `instantiate_type_formals_to_bounds` / `itb_default_args`, the SCC
//! fixpoint (`class A<X extends A<X>>` raw `A` reads `A<A<dynamic>>`) —
//! not `dynamic`; and (b) the super-bounded pass — `is_well_bounded`
//! re-checks a failing regular bound through `replaceTopAndBottom`, so a
//! written `Comparable<Never>` argument passes `T extends Comparable<T>`
//! where `Comparable<int>` fails the regular pass.
//!
//! The residual this ticket closes is the function-shape bound carry:
//! `function_shape_sig`'s `Ty::Function` arm answered a count-only
//! signature (`type_params` names+bounds dropped), so a written
//! `void Function<Y extends num>()` source against a written
//! `void Function<Y extends int>()` target reached `fn_sig_subtype`'s
//! `relateTypeParameters` equality leg with no bound to compare and
//! stayed `None` (the under-report). The arm now reads the interned
//! shape's `type_params_named` column, so the written bound decides.
//!
//! RESIDUAL (out of scope): a `var`-inferred folded-alias `from`
//! (`var fsource = toF(source)` over `typedef F<X> = void Function<Y
//! extends X>()`) still loses `Y`'s bound — `TreeSink::leaf`'s
//! `in_type_variables` early-return (capture.rs, forbidden scope) drops
//! the leaf bound head before `decl.var_ty` folds it into the element
//! model, so `fn_sig_subtype` sees a count-only entry and returns
//! `None`. The written-target side and the written-`Function<Y extends
//! B>` sides decide correctly.

use super::test_rig::report_ops;
use crate::FileReport;

/// The offsets of one code's rows, ascending.
fn offsets(report: &FileReport, code: &str) -> Vec<u32> {
    let mut at = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.range.offset)
        .collect::<Vec<_>>();
    at.sort_unstable();
    at
}

/// Instantiate-to-bounds: a raw `A` use (no written type arguments) over
/// `class A<X extends A<X>>` reads `A<A<dynamic>>` — the SCC fixpoint
/// answer, not a `dynamic` stand-in. The error message renders the
/// instantiated argument, so the diagnostic text itself is the
/// assertion. A nullable raw `A?` reads `A<A<dynamic>>?`.
#[test]
fn i2b_raw_head_renders_instantiated_bound() {
    let fixture = "\
class A<X extends A<X>> {}
void main() {
  A? source;
  int i = source;
}
";
    let report = report_ops(fixture);
    let assign = offsets(&report, "invalid_assignment");
    assert_eq!(
        assign.len(),
        1,
        "expected one invalid_assignment: {report:#?}"
    );
    let msg = &report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "invalid_assignment")
        .unwrap()
        .message;
    assert!(
        msg.contains("A<A<dynamic>>?"),
        "raw `A?` must I2B to `A<A<dynamic>>?`, got: {msg}"
    );
    assert!(
        !msg.contains("'A<dynamic>?") && !msg.contains("'A<dynamic>'"),
        "raw `A?` must not collapse to the single-level `dynamic` \
         stand-in `A<dynamic>`: {msg}"
    );
}

/// Super-bounds conformance: `T extends Comparable<T>` must terminate —
/// the F-bound shape. The written `C<Comparable<Never>>` argument fails
/// the regular bound check but passes the inverted super-bounded pass
/// (no diagnostic); `C<int>` fails both and reports
/// `type_argument_not_matching_bounds`.
#[test]
fn super_bounds_f_bound_inverted_pass() {
    let fixture = "\
class Comparable<T> {}
class C<T extends Comparable<T>> {}
void main() {
  C<Comparable<Never>> a = throw 0;
  C<int> b = throw 0;
}
";
    let report = report_ops(fixture);
    let bounds = offsets(&report, "type_argument_not_matching_bounds");
    assert_eq!(
        bounds.len(),
        1,
        "only `C<int>` must report; `C<Comparable<Never>>` passes the \
         super-bounded pass: {report:#?}"
    );
    let msg = &report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "type_argument_not_matching_bounds")
        .unwrap()
        .message;
    assert!(
        msg.contains("int"),
        "the failing argument must be `int`, got: {msg}"
    );
}

/// Super-bounds divergence guard: `T extends Comparable<T>` resolves the
/// F-bound without recursing on `T`'s own bound (the divergent raw
/// subtype question). The declaration itself reports nothing.
#[test]
fn super_bounds_f_bound_declaration_clean() {
    let fixture = "\
class Comparable<T> {}
class C<T extends Comparable<T>> {}
void main() {
  C<Comparable<Never>> a = throw 0;
}
";
    let report = report_ops(fixture);
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name != "type_argument_not_matching_bounds"),
        "F-bound declaration must be clean: {report:#?}"
    );
}

/// Generic-bound residual — the recovered face: a WRITTEN
/// `void Function<Y extends num>()` source against a WRITTEN
/// `void Function<Y extends int>()` target. `relateTypeParameters`
/// requires EQUAL bounds (mutual subtype), so `num` vs `int` is a
/// rejection in BOTH directions — the oracle reports
/// `invalid_assignment` on both assignments. The count-only shape
/// (bounds dropped) returned `None` and stayed silent; the carried
/// bound now decides.
#[test]
fn generic_bound_equality_written_function_pair() {
    let fixture = "\
void main() {
  void Function<Y extends num>() from_num = <Y extends num>() {};
  void Function<Y extends int>() to_int = from_num;
  void Function<Y extends int>() from_int = <Y extends int>() {};
  void Function<Y extends num>() to_num = from_int;
}
";
    let report = report_ops(fixture);
    let assign = offsets(&report, "invalid_assignment");
    assert_eq!(
        assign.len(),
        2,
        "both bound-mismatched generic assignments must report: {report:#?}"
    );
}

/// Generic-bound residual — equal bounds stay silent: matching
/// `extends num` on both sides passes the `relateTypeParameters`
/// equality leg, and matching `extends int` does too. No assignment
/// diagnostic.
#[test]
fn generic_bound_equality_matching_bounds_silent() {
    let fixture = "\
void main() {
  void Function<Y extends num>() a = <Y extends num>() {};
  void Function<Y extends num>() b = a;
  void Function<Y extends int>() c = <Y extends int>() {};
  void Function<Y extends int>() d = c;
}
";
    let report = report_ops(fixture);
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name != "invalid_assignment"),
        "matching bounds must stay silent: {report:#?}"
    );
}

/// The written-target alias face: `typedef F<X> = void Function<Y
/// extends X>()` used as `F<int>` reads `void Function<Y extends int>()`
/// through `instantiated_alias_sig`'s `rhs_vars` — the use-site bound
/// the count-only fold cannot carry. A written `Function<Y extends num>`
/// source against `F<int>` reports (num vs int bound mismatch); a
/// written `Function<Y extends int>` source against `F<int>` is silent.
#[test]
fn generic_bound_alias_instantiated_target() {
    let fixture = "\
typedef F<X> = void Function<Y extends X>();
void main() {
  void Function<Y extends num>() src_num = <Y extends num>() {};
  F<int> tgt_int = src_num;
  void Function<Y extends int>() src_int = <Y extends int>() {};
  F<int> tgt_int2 = src_int;
}
";
    let report = report_ops(fixture);
    let assign = offsets(&report, "invalid_assignment");
    assert_eq!(
        assign.len(),
        1,
        "only the num-vs-int bound mismatch must report: {report:#?}"
    );
}

/// A `var`-inferred `F<...>` target (written, not folded): `var t =
/// src` where `src: void Function<Y extends num>()` and `t` is later
/// assigned into `F<int>`. The written `F<int>` target recovers its
/// bound through `instantiated_alias_sig` even though the source's
/// inferred face may be count-only — the bound-equality leg only fails
/// when BOTH bounds are present and unequal, so a real written mismatch
/// still reports through the annotation site.
#[test]
fn generic_bound_written_alias_target_reports() {
    let fixture = "\
typedef F<X> = void Function<Y extends X>();
void main() {
  void Function<Y extends num>() src = <Y extends num>() {};
  F<int> t = src;
}
";
    let report = report_ops(fixture);
    let assign = offsets(&report, "invalid_assignment");
    assert_eq!(
        assign.len(),
        1,
        "num-source into int-target alias must report: {report:#?}"
    );
}
