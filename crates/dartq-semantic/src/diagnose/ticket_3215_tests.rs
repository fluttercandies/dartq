//! #3215 pins — the F6-A bound-head residual: a `var`-inferred
//! folded-alias `from` whose type-parameter bound is a bare leaf
//! (`var g = fsource` over `typedef F<X> = void Function<Y extends
//! X>()` and `fsource`/`getSrc` declared `F<num>`).
//!
//! The collector's written-type capture routes a bound's shape into the
//! open function level's `bounds` column through two doors: a bound with
//! written type arguments completes through `end_group` → `push_node`
//! (the `in_type_variables && function` arm collects it), but a bound
//! whose head is a bare leaf — `extends X`, the alias's own type
//! variable — completed through `TreeSink::leaf`, whose
//! `in_type_variables` early-return dropped it outright. The signature's
//! `type_params_named` column therefore interned `(Y, None)` — a
//! count-only entry — and the typedef's `TopDecl::var_ty` folded the
//! alias right-hand shape into the element model with no bound to
//! substitute. A `var`-inferred `from` reading that fold —
//! `instantiate_alias`'s `substitute_alias_rhs` through
//! `fold_alias_ty`'s member/return surfaces — reached
//! `fn_sig_subtype`'s `relateTypeParameters` with `None` for the bound
//! compare: the resolved-type spelling rendered `void Function<Y>()`
//! where the oracle renders `void Function<Y extends num>()`, and the
//! count-only face under-reported the shapes whose verdict only the
//! bound presence decides.
//!
//! `TreeSink::leaf` now routes the leaf bound head into `level.bounds`
//! under the same `in_type_variables && function` gate `push_node`
//! already applies — the re-fired-head merge rides it identically
//! (`Y extends List<int>?` still merges the `?` onto the completed
//! group node). Non-function levels keep the drop (container bounds are
//! the bounds.rs window's business). The written-target side and the
//! written `Function<Y extends B>` sides already decided correctly
//! through #3209's `instantiated_alias_sig`/`type_params_named` carry —
//! these pins close the inferred-`from` face.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block (the
//! brief's named hosts — `diagnose.rs`/`assignability.rs` — are
//! live-lane files out of this ticket's write scope; `tests.rs` keeps
//! the module inside the `diagnose` subtree so
//! `crate::diagnose::test_rig` resolves, the same convention
//! `ticket_2849_tests.rs` keeps).

use crate::diagnose::test_rig::{report_ops, rows};
use crate::diagnose::types::FileReport;

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

/// The recovered face: `var g = fsource` where `fsource` is declared
/// `F<num>` — the `var`-inferred `from` reads `decl.var_ty`'s folded
/// right-hand shape with `X := num` substituted, so the bound reads
/// `Y extends num`. Assigned into a written `void Function<Y extends
/// int>()` target the equality leg rejects (`num` vs `int` bounds).
///
/// The row's value face is the ALIAS use `F<num>` — the oracle's
/// `getDisplayString(preferTypeAlias: true)` keeps the use's written
/// alias spelling wherever the element's resolved type carries the
/// `alias` reference (`#3685`'s bind-posture wave made the value face
/// spell it too; pre-wave this pin demanded the expanded
/// `void Function<Y extends num>()`).
///
/// Oracle row (`dart analyze` 3.13.7, `~/.dartq-oracle/3.13.7`):
/// `error - main.dart:5:43 - A value of type 'F<num>' can't be assigned
/// to a variable of type 'void Function<Y extends int>()'. -
/// invalid_assignment`
#[test]
fn var_inferred_folded_alias_bound_mismatch_reports() {
    let fixture = "\
typedef F<X> = void Function<Y extends X>();
void main() {
  F<num> fsource = <Y extends num>() {};
  var g = fsource;
  void Function<Y extends int>() target = g;
}
";
    let report = report_ops(fixture);
    let assign = offsets(&report, "invalid_assignment");
    assert_eq!(
        assign.len(),
        1,
        "the num-bound inferred source into the int-bound written target \
         must report one invalid_assignment: {}",
        rows(&report).join("\n")
    );
    let msg = &report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "invalid_assignment")
        .unwrap()
        .message;
    assert!(
        msg.contains("'F<num>'"),
        "the value face spells the alias use `F<num>` — the oracle's \
         preferTypeAlias form — not the expanded signature: {msg}"
    );
}

/// A member-declared `F<num>` face reads the same `decl.var_ty` fold:
/// `var g = Box().f` resolves the field's `var_ty` through
/// `instantiate_alias` and carries `Y extends num` into the bound
/// comparison — the mismatch reports identically.
#[test]
fn var_inferred_member_alias_bound_reports() {
    let fixture = "\
typedef F<X> = void Function<Y extends X>();
class Box {
  F<num> f = <Y extends num>() {};
}
void main() {
  var g = Box().f;
  void Function<Y extends int>() target = g;
}
";
    let report = report_ops(fixture);
    let assign = offsets(&report, "invalid_assignment");
    assert_eq!(
        assign.len(),
        1,
        "the field-read folded alias must report the bound mismatch: {}",
        rows(&report).join("\n")
    );
}

/// The matching-bound direction stays silent: `var g = fsource` over
/// `F<num>` into a written `void Function<Y extends num>()` target
/// passes `relateTypeParameters`' equality leg — the carried bound
/// must not fabricate a rejection.
#[test]
fn var_inferred_folded_alias_matching_bound_silent() {
    let fixture = "\
typedef F<X> = void Function<Y extends X>();
void main() {
  F<num> fsource = <Y extends num>() {};
  var g = fsource;
  void Function<Y extends num>() target = g;
}
";
    let report = report_ops(fixture);
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name != "invalid_assignment"),
        "matching bounds must stay silent: {}",
        rows(&report).join("\n")
    );
}

/// A GROUP bound survives the same routing untouched: `typedef G<X> =
/// void Function<Y extends List<X>>()` — the bound head `List<X>`
/// completes through `end_group` (the path that always worked) — and
/// `var g = gsource` over `G<num>` substitutes `X := num` inside the
/// group bound. `List<num>` vs `List<int>` fails the equality leg.
#[test]
fn var_inferred_folded_alias_group_bound_reports() {
    let fixture = "\
typedef G<X> = void Function<Y extends List<X>>();
void main() {
  G<num> gsource = <Y extends List<num>>() {};
  var g = gsource;
  void Function<Y extends List<int>>() target = g;
}
";
    let report = report_ops(fixture);
    let assign = offsets(&report, "invalid_assignment");
    assert_eq!(
        assign.len(),
        1,
        "the group-bound mismatch must report identically: {}",
        rows(&report).join("\n")
    );
}

/// A non-generic `var` leaf is unchanged: `var s = identity(1)` over a
/// plain `num Function(num)` shape carries no type-variable group at
/// all — nothing routes into any `bounds` column and the matching
/// assignment stays silent.
#[test]
fn non_generic_var_leaf_carries_no_bound() {
    let fixture = "\
num identity(num n) => n;
void main() {
  var s = identity(1);
  num t = s;
}
";
    let report = report_ops(fixture);
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name != "invalid_assignment"),
        "a plain var inference must stay silent: {}",
        rows(&report).join("\n")
    );
}
