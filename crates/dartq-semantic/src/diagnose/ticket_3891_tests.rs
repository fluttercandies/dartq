//! Ticket #3891 pins — the `not_instantiated_bound` declaration face.
//!
//! The code did not exist in the registry: the oracle emits
//! `NOT_INSTANTIATED_BOUND` at every bare head inside a written type-
//! parameter bound that names a generic declaration which is *not simply
//! bounded*, where dartq was fully silent (census 2026-11-04: 41 failing
//! cases, 64 missing rows). The upstream face is the
//! `_UninstantiatedBoundChecker` (upstream Dart SDK source,
//! pkg/analyzer/lib/src/generated/error_verifier.dart:8521-8541) fired
//! from `visitTypeParameter` (:2159), and the verdict is
//! `computeSimplyBounded`'s dependency-graph answer (upstream Dart SDK
//! source, pkg/analyzer/lib/src/summary2/simply_bounded.dart:31-105).
//!
//! Shapes pinned here, one per census family: the direct bound, the deep
//! argument, the recursion cycle (SCC), the bound-argument recursion, the
//! self-referential bound, the function-type bound (formal and signature
//! variable), the typedef bound (and the alias-chain SCC), the executable
//! type parameter, plus the passing controls (instantiated bounds stay
//! silent), the unresolved head (suppresses), and the sibling
//! `type_argument_not_matching_bounds` face (still fires, no interference).

use super::test_rig::{CORE_PRELUDE, report_ops};
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

/// The byte offset of the token right after `prefix`'s first occurrence —
/// the head token a written bound anchors its row at.
fn head(source: &str, prefix: &str) -> u32 {
    let start = source.find(prefix).expect("prefix present");
    (start + prefix.len()) as u32
}

/// The direct shape (`test_direct_notInstantiated`): `A`'s own bound
/// mentions its type parameter, so `A` is not simply bounded and `C`'s
/// bare bound head reports at the head token.
#[test]
fn direct_bound_reports_at_bare_head() {
    let fixture = "\
class A<K, V extends K> {}
class C<T extends A> {}
";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "not_instantiated_bound"),
        vec![head(&full, "class C<T extends ")],
        "one row anchored at the bound's bare head: {report:#?}"
    );
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "not_instantiated_bound")
        .expect("the row reports");
    assert_eq!(
        row.message.to_string(),
        "Type parameter bound types must be instantiated."
    );
    assert_eq!(row.severity.as_machine_str(), "ERROR");
}

/// The deep-argument shape (`test_argumentDeep_notInstantiated`): the
/// walk runs through two instantiated argument levels before the
/// type-parameter leaf kills `A`'s simple-boundedness.
#[test]
fn deep_argument_bound_reports_at_bare_head() {
    let fixture = "\
class A<K, V extends List<List<K>>> {}
class C<T extends A> {}
";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "not_instantiated_bound"),
        vec![head(&full, "class C<T extends ")],
        "the bare bound head reports: {report:#?}"
    );
}

/// The instantiated-argument control (`test_argument_notInstantiated`):
/// `List<K>` is an instantiated head — its argument leaf kills `A` — and
/// the report still lands only on `C`'s bare head.
#[test]
fn instantiated_argument_bound_reports_once() {
    let fixture = "\
class A<K, V extends List<K>> {}
class C<T extends A> {}
";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "not_instantiated_bound"),
        vec![head(&full, "class C<T extends ")],
        "{report:#?}"
    );
}

/// The indirect shape (`test_indirect_notInstantiated`): a bare head
/// inside a foreign head's argument list recurses and reports at the
/// inner leaf.
#[test]
fn argument_inside_instantiated_head_reports() {
    let fixture = "\
class A<K, V extends K> {}
class C<T extends List<A>> {}
";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "not_instantiated_bound"),
        vec![head(&full, "class C<T extends List<")],
        "the bare leaf inside the argument list reports: {report:#?}"
    );
}

/// The recursion shape (`test_class_recursion_notInstantiated`): the
/// A/B cycle marks both not simply bounded and `C`'s dependency on `A`
/// propagates — three rows, declaration order.
#[test]
fn recursion_cycle_reports_every_bound_head() {
    let fixture = "\
class A<T extends B> {}
class B<T extends A> {}
class C<T extends A> {}
";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "not_instantiated_bound"),
        vec![
            head(&full, "class A<T extends "),
            head(&full, "class B<T extends "),
            head(&full, "class C<T extends "),
        ],
        "every bound head of the cycle reports: {report:#?}"
    );
}

/// The bound-argument shape (`test_class_recursion_boundArgument_`):
/// the outer heads carry type arguments, so each bound recurses into
/// them and the ARGUMENT-position bare heads report (the census rows
/// anchor at those inner heads, `2:21` / `3:21`).
#[test]
fn bound_argument_recursion_reports_inner_heads() {
    let fixture = "\
class A<T extends B<A>> {}
class B<T extends A<B>> {}
";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "not_instantiated_bound"),
        vec![
            head(&full, "class A<T extends B<"),
            head(&full, "class B<T extends A<"),
        ],
        "the bare heads inside the bound arguments report: {report:#?}"
    );
}

/// The self-referential shapes (the SimpleParserTest rows): a bound
/// naming its own declaration is a self-loop (`class C<A, B extends C,
/// D>`), and an unresolved head with arguments recurses past it
/// (`class C<A extends B<C>>` — `B` resolves to nothing, the inner `C`
/// is the cycle member that reports).
#[test]
fn self_referential_bounds_report() {
    let fixture = "\
class C<A, B extends C, D> {}
";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "not_instantiated_bound"),
        vec![head(&full, "class C<A, B extends ")],
        "the self-loop marks C not simply bounded: {report:#?}"
    );
    let fixture = "\
class C<A extends B<C>> {}
";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "not_instantiated_bound"),
        vec![head(&full, "class C<A extends B<")],
        "the unresolved head skips, the inner leaf reports: {report:#?}"
    );
}

/// The function-type formal shape
/// (`test_class_recursion_notInstantiated_genericFunctionType`): a
/// bound's function-type columns descend, so the bare formal head
/// reports.
#[test]
fn function_type_formal_bound_reports() {
    let fixture = "\
class A<T extends void Function(A)> {}
";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "not_instantiated_bound"),
        vec![head(&full, "class A<T extends void Function(")],
        "the formal's bare head reports: {report:#?}"
    );
}

/// The signature-variable shape
/// (`..._genericFunctionType2`): a bound's function type's own variable
/// bound is a report site too.
#[test]
fn signature_variable_bound_reports() {
    let fixture = "\
class A<T extends void Function<U extends A>()> {}
";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "not_instantiated_bound"),
        vec![head(&full, "class A<T extends void Function<U extends ")],
        "the signature variable's bound head reports: {report:#?}"
    );
}

/// The typedef-bound shape (`test_typedef_direct_notInstantiated`): an
/// alias's parameter bound is a report site, and the alias itself joins
/// the graph — the chain `typedef F` → `class C` → `class D` marks both
/// classes not simply bounded (`test_class_recursion_typedef_`).
#[test]
fn typedef_bounds_report() {
    let fixture = "\
class A<K, V extends K> {}
typedef void F<T extends A>();
";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "not_instantiated_bound"),
        vec![head(&full, "typedef void F<T extends ")],
        "the alias's parameter bound reports: {report:#?}"
    );
    let fixture = "\
typedef F(C value);
class C<T extends F> {}
class D<T extends C> {}
";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "not_instantiated_bound"),
        vec![
            head(&full, "class C<T extends "),
            head(&full, "class D<T extends "),
        ],
        "the alias-class cycle marks both bounds: {report:#?}"
    );
    // The sibling face still fires at the alias's name, one row each —
    // the two faces coexist upstream (no suppression interplay).
    assert_eq!(
        offsets(&report, "type_alias_cannot_reference_itself").len(),
        1
    );
}

/// The executable-type-parameter shape: a function declaration's own
/// `<...>` window is a report site, same as any declaration's.
#[test]
fn executable_type_parameter_bound_reports() {
    let fixture = "\
class A<K, V extends K> {}
void f<T extends A>() {}
";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "not_instantiated_bound"),
        vec![head(&full, "void f<T extends ")],
        "{report:#?}"
    );
}

/// The passing controls (`test_class_instantiated` and friends): simply
/// bounded targets, instantiated references, function-type-only bounds —
/// zero rows.
#[test]
fn simply_bounded_targets_stay_silent() {
    let fixture = "\
class A<T extends int> {}
class C1<T extends A> {}
class C2<T extends List<A>> {}
class B<T extends void Function()> {}
class D<T extends A> {}
typedef void F<T extends int>();
class E<T extends F> {}
class G<T extends A<int>> {}
class H<T extends G> {}
";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert!(
        offsets(&report, "not_instantiated_bound").is_empty(),
        "every bound is instantiated or simply bounded: {report:#?}"
    );
}

/// An unresolved bare head suppresses (upstream answers `true` for an
/// element-less `NamedType`, simply_bounded.dart:301-305).
#[test]
fn unresolved_bound_head_stays_silent() {
    let fixture = "\
class C<T extends Missing> {}
";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert!(
        offsets(&report, "not_instantiated_bound").is_empty(),
        "the unresolvable head decides nothing: {report:#?}"
    );
}

/// The sibling face control: a real `type_argument_not_matching_bounds`
/// violation still fires, and the declaration face emits nothing for it.
#[test]
fn sibling_argument_bounds_face_unaffected() {
    let fixture = "\
class C<X extends num> {}
void main() {
  C<String>();
}
";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    let at_rows = offsets(&report, "type_argument_not_matching_bounds");
    assert_eq!(at_rows.len(), 1, "the violation reports: {report:#?}");
    assert!(
        offsets(&report, "not_instantiated_bound").is_empty(),
        "the declaration face stays silent: {report:#?}"
    );
}
