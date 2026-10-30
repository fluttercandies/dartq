//! The #2918 pins: the generic creation's instantiated display face in
//! diagnostic messages.
//!
//! 1. A bare creation with no written group (`var x = C()` over
//!    constrained parameters) binds the instantiate-to-bounds face —
//!    `C<num>` over `C<S extends num>` — through
//!    `with_explicit_creation_args`'s residual arm, never the fold's
//!    raw head or the unknown sentinel.
//! 2. A headed-group creation whose `call_type_args` entry never
//!    latched still displays its written arguments:
//!    `creation_row_ty`'s byte-walk re-derives the group the
//!    `anchored` gate dropped, the peek-not-consume twin of
//!    `call_args_for`.
//! 3. The named-selector headless group (`C<T>.named()` bare): the
//!    entry keys on the receiver row, so `call_args_for` re-keys
//!    through `rows[callee - 1]`'s `Bare(Container)` segment.
//!
//! The dominant census face — the raw-head instantiate-to-bounds
//! display of every NON-creation `Named{_, EMPTY}` (`G source`,
//! `G?`, nested `F<G>` inference residue) — lands through
//! [`Engine::instantiated_head_ty`] (creation_args.rs), adopted at the
//! declared-type and display sites in scopes_bodies.rs / resolution.rs
//! / assignability.rs; those call sites are sibling-lane owned this
//! round (recorded dependency, not pinned here).
//!
//! Oracle provenance: the #2918 census rows — `Instantiate-to-bound`
//! `typedef_l1_t01`/`t05`/`l2`, `StrongModeLocalInferenceTest`
//! `test_instanceCreation` (a4/a5), expected.tsv machine authority;
//! upstream anchors `type.dart` `instantiateTypeFormalsToBounds` /
//! `DartType.getDisplayString` (Dart SDK 3.13).

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

fn invalid_assignment_rows(report: &FileReport) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

/// Byte offset of `needle` inside the fixture source, shifted by the
/// rig's core prelude (the fixture rides behind it on one line).
fn at(source: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + source.find(needle).expect("needle in fixture")) as u32
}

// —— shape 1: the bare-creation instantiate-to-bounds residual ——
//
// `var x = C()` over `C<S extends num>`: the oracle binds `C<num>`
// (I2B fills the written bound), and assigning `x` into `C<int>`
// reports that face — `num` is not assignable to `int`.

/// `var x = C()` binds `C<num>` — the instantiate-to-bounds
/// residual `with_explicit_creation_args` now lands on the created
/// row — and the downstream assignment displays it.
#[test]
fn bounded_bare_creation_displays_the_bound() {
    let source = r#"
class C<S extends num> {}
main() {
  var x = C();
  C<int> y = x;
}
"#;
    let report = report_ops(source);
    assert_eq!(
        invalid_assignment_rows(&report),
        [(
            at(source, "x;"),
            1,
            "A value of type 'C<num>' can't be assigned to a variable of type 'C<int>'."
                .to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// The two-parameter bare creation: `C<S extends num, T extends String>`
/// displays `C<num, String>` — both slots fill by their I2B bounds.
#[test]
fn two_param_bare_creation_displays_both_bounds() {
    let source = r#"
class C<S extends num, T extends String> {}
main() {
  var x = C();
  C<int, String> y = x;
}
"#;
    let report = report_ops(source);
    assert_eq!(
        invalid_assignment_rows(&report),
        [(
            at(source, "x;"),
            1,
            "A value of type 'C<num, String>' can't be assigned to a variable of type 'C<int, String>'."
                .to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// The compatible instantiation stays silent: `var x = C()` binds
/// `C<num>` which assigns into `C<num>` — the I2B fill never invents
/// a mismatch.
#[test]
fn compatible_bare_creation_stays_silent() {
    let source = r#"
class C<S extends num> {}
main() {
  var x = C();
  C<num> y = x;
}
"#;
    let report = report_ops(source);
    assert!(
        invalid_assignment_rows(&report).is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// `var x = C()` over `C<S>` unconstrained binds `C<dynamic>`: a
/// supertype target stays silent (`dynamic` assigns to `num`), and a
/// `C<Never>` target reports the instantiated `C<dynamic>` face — the
/// `dynamic` residual is the oracle's own instantiation.
#[test]
fn unconstrained_bare_creation_displays_dynamic() {
    let source = r#"
class C<S> {}
main() {
  var x = C();
  C<Never> y = x;
}
"#;
    let report = report_ops(source);
    assert_eq!(
        invalid_assignment_rows(&report),
        [(
            at(source, "x;"),
            1,
            "A value of type 'C<dynamic>' can't be assigned to a variable of type 'C<Never>'."
                .to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

// —— shape 2: the solved-inference precedence ——
//
// The I2B residual never overrides a solved `pending_send_ty`: an
// inference-answered creation keeps its solved display.

/// `var x = C(0)` — the argument solves `S = int`, so the display is
/// `C<int>`, never the I2B `C<num>`: the residual arm runs only when
/// no written group AND no solve answered.
#[test]
fn solved_creation_keeps_its_inferred_display() {
    let source = r#"
class C<S> {
  S x = throw 0;
  C(this.x);
}
main() {
  var x = C(0);
  C<String> y = x;
}
"#;
    let report = report_ops(source);
    let rows = invalid_assignment_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", report.diagnostics);
    assert!(
        rows[0].2.contains("'C<int>'"),
        "the solved C<int> display: {:?}",
        report.diagnostics
    );
}

// —— shape 3: the written-group byte-walk on the `var` bind ——
//
// `var x = new C<int, dynamic>()`: the headed `new` group's
// `call_type_args` entry never latched (the `anchored` gate reads the
// class segment's missing row), and the fold-time `literal_group_args`
// already rides `row.ty` — the `var` bind reads the written
// instantiation through `with_explicit_creation_args`, the same face
// the keyword fold produced.

/// `var x = new C<int, dynamic>()` binds `C<int, dynamic>` — the
/// written group recovered when the headed group left no
/// `call_type_args` entry.
#[test]
fn keyword_creation_var_binds_written_args() {
    let source = r#"
class C<S, T> {
  S x = throw 0;
  T y = throw 0;
  C(this.x, this.y);
}
main() {
  var x = new C<int, dynamic>(1, "x");
  C<int, String> y = x;
}
"#;
    let report = report_ops(source);
    assert_eq!(
        invalid_assignment_rows(&report),
        [(
            at(source, "x;"),
            1,
            "A value of type 'C<int, dynamic>' can't be assigned to a variable of type 'C<int, String>'."
                .to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// `var x = C<int, dynamic>()` (the implicit creation): the headless
/// `call_type_args` entry latches on the class row, and the bind reads
/// the same written instantiation — the twin of the keyword spelling.
#[test]
fn implicit_creation_var_binds_written_args() {
    let source = r#"
class C<S, T> {
  S x = throw 0;
  T y = throw 0;
  C(this.x, this.y);
}
main() {
  var x = C<int, dynamic>(1, "x");
  C<int, String> y = x;
}
"#;
    let report = report_ops(source);
    assert_eq!(
        invalid_assignment_rows(&report),
        [(
            at(source, "x;"),
            1,
            "A value of type 'C<int, dynamic>' can't be assigned to a variable of type 'C<int, String>'."
                .to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}
