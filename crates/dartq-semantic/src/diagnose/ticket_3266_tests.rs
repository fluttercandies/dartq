//! #3266 pins — the instantiate-to-bounds step behind a raw type-alias use
//! and the container face beside it.
//!
//! `instantiateTypeFormalsToBounds` (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/element/type_system.dart:762-838) is a fixed
//! point over the parameter dependency graph. A parameter completes once
//! every free parameter of its bound is ground; the survivors are exactly
//! the members of a bound cycle (`X extends A<X>`, `X extends FutureOr<X>`,
//! `X extends Comparable<X>`), which the Phi substitution rewrites to the
//! completed defaults (`dynamic` for the stuck ones). A raw use — of a
//! class (`instantiateInterfaceToBounds`, type_system.dart:719-731) or of a
//! type alias — therefore reads the *substituted* bound (`A<dynamic>`,
//! `FutureOr<dynamic>`), never a recursion into the bound and never a bare
//! `dynamic` substituted for every parameter.
//!
//! The flow layer's alias face is pinned where it lives: the snapshot-level
//! assertion that a bare alias use maps to its to-bounds default sits in
//! `resolved_ops`' own test module
//! (`raw_alias_use_maps_to_its_instantiate_to_bounds_defaults`), because the
//! assignment and argument rows this rig reports are decided by the type
//! world above the flow snapshot. What stays here guards the two directions
//! that rig can see: the container SCC answer is unchanged, and a genuinely
//! mismatched alias pair still reports.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block (`diagnose.rs`
//! is a live-lane file out of this ticket's write scope).

use crate::FileReport;
use crate::diagnose::test_rig::report_ops;

/// The number of one code's rows.
fn count(report: &FileReport, code: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .count()
}

/// The container face is untouched: a raw class use still reads its SCC
/// instantiation, `class A<X extends A<X>>` reading `A<A<dynamic>>`
/// (`instantiateInterfaceToBounds`, type_system.dart:719-731).
#[test]
fn raw_container_self_bound_is_unchanged() {
    let fixture = "\
class A<X extends A<X>> {}
void main() {
  A? source;
  int i = source;
}
";
    let report = report_ops(fixture);
    let messages: Vec<&str> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(
        messages.len(),
        1,
        "one invalid_assignment over the raw `A?`: {:?}",
        report.diagnostics
    );
    assert!(
        messages[0].contains("A<A<dynamic>>?"),
        "a raw `A?` must I2B to `A<A<dynamic>>?`, got: {}",
        messages[0]
    );
}

/// The negative twin of the alias face: the instantiate-to-bounds default
/// never hides a genuine mismatch — a closure whose parameter is `String`
/// still fails an alias whose bound is `num`.
#[test]
fn raw_alias_bound_mismatch_still_reports() {
    let fixture = "\
typedef G<T extends num> = int Function(T);
void takes(G g) {}
void main() {
  takes((String x) => 1);
}
";
    let report = report_ops(fixture);
    assert_eq!(
        count(&report, "argument_type_not_assignable"),
        1,
        "a closure taking `String` cannot fill `T extends num`: {:?}",
        report.diagnostics
    );
}
