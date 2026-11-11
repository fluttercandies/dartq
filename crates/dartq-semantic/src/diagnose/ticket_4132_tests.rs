//! The #4132 pins: the raw-creation inference face's alias twin and named
//! creations.
//!
//! Each fixture is byte-exact against the pinned oracle (Dart SDK 3.13.7)
//! `dart analyze --format=machine` rows for exactly that source:
//!
//! - **the alias twin spells the CLASS parameter** — an alias callee
//!   constructs its expanded class type, so the creation-site
//!   `type_argument_not_matching_bounds` row is the class's own check
//!   (`Y`), while `could_not_infer` keys the alias's parameter (`X`);
//! - **an alias-headed named creation** (`A.foo()`) runs the same
//!   inference over the alias's parameters and anchors the whole
//!   constructor name — a factory's creation-site twin is absent upstream;
//! - **a declared named constructor** reports one inference row per
//!   parameter: the argument solver does not re-report what the creation
//!   seam already reported.

use super::test_rig::{CORE_PRELUDE, report_ops, rows};
use super::*;

/// The absolute fixture offset of `needle`'s LAST occurrence (the prelude
/// the rig prepends included).
fn at_last(fixture: &str, needle: &str) -> usize {
    CORE_PRELUDE.len() + fixture.rfind(needle).expect("needle present")
}

/// Every `(name, offset, length, message)` row whose code is one of the
/// creation-inference faces.
fn inference_rows(report: &FileReport) -> Vec<(&str, usize, usize, &str)> {
    report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == "could_not_infer" || d.code.name == "type_argument_not_matching_bounds"
        })
        .map(|d| {
            (
                d.code.name,
                d.range.offset as usize,
                d.range.length as usize,
                d.message.as_str(),
            )
        })
        .collect()
}

/// `could_not_infer` text for a `C<X>`-style F-bound whose seed is
/// `C<Object?>` (the parameter and head vary per fixture).
fn f_bound_message(parameter: &str, head: &str) -> String {
    format!(
        "Couldn't infer type parameter '{parameter}'.\n\n\
         Tried to infer '{head}<Object?>' for '{parameter}' which doesn't work:\n  \
         Type parameter '{parameter}' is declared to extend '{head}<{parameter}>' \
         producing '{head}<{head}<Object?>>'.\n\n\
         Consider passing explicit type argument(s) to the generic.\n\n"
    )
}

/// `class C<Y extends C<Y>>` behind `typedef A<X extends C<X>> = C<X>;`
/// with `A();` — the inference row names the alias's `X`, the twin names
/// the class's `Y`. Oracle 4:3, both rows length 1.
#[test]
fn alias_twin_names_the_class_parameter() {
    let src = r#"
class C<Y extends C<Y>> {}
typedef A<X extends C<X>> = C<X>;
main() {
  A();
}
"#;
    let report = report_ops(src);
    let offset = at_last(src, "A();");
    let infer = f_bound_message("X", "C");
    assert_eq!(
        inference_rows(&report),
        [
            ("could_not_infer", offset, 1, infer.as_str()),
            (
                "type_argument_not_matching_bounds",
                offset,
                1,
                "'C<Object?>' doesn't conform to the bound 'C<C<Object?>>' of the type parameter 'Y'.",
            ),
        ],
        "{:#?}",
        rows(&report)
    );
}

/// `A.foo()` under `typedef A<X extends C<X>> = C<X>;` with a factory
/// `C.foo` — one `could_not_infer` over the alias's `X` anchored at the
/// whole `A.foo` (length 5), and no twin. Oracle 7:3.
#[test]
fn alias_named_creation_reports_over_the_alias_parameter() {
    let src = r#"
class C<X> {
  C();
  factory C.foo() => C();
}
typedef A<X extends C<X>> = C<X>;
void f() {
  A.foo();
}
"#;
    let report = report_ops(src);
    let offset = at_last(src, "A.foo();");
    let infer = f_bound_message("X", "C");
    assert_eq!(
        inference_rows(&report),
        [("could_not_infer", offset, 5, infer.as_str())],
        "{:#?}",
        rows(&report)
    );
}

/// `class B<X extends B<X>> { B(); B.named(); }` with `B.named();` — one
/// `could_not_infer` at `B.named` (length 7) plus the head-anchored twin;
/// the declared constructor's argument solve adds no second inference row.
/// Oracle 3:3.
#[test]
fn declared_named_constructor_reports_once() {
    let src = r#"
class B<X extends B<X>> { B(); B.named(); }
main() {
  B.named();
}
"#;
    let report = report_ops(src);
    let offset = at_last(src, "B.named();");
    let infer = f_bound_message("X", "B");
    assert_eq!(
        inference_rows(&report),
        [
            ("could_not_infer", offset, 7, infer.as_str()),
            (
                "type_argument_not_matching_bounds",
                offset,
                1,
                "'B<Object?>' doesn't conform to the bound 'B<B<Object?>>' of the type parameter 'X'.",
            ),
        ],
        "{:#?}",
        rows(&report)
    );
}
