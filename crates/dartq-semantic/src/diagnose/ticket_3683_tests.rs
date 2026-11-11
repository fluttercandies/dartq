//! The #3683 pins — the statement-position as-cast promotion (~10 census
//! rows, the `expectStaticType`/implicit-tearoff family).
//!
//! Upstream `asExpression_end` runs `tryPromoteForTypeCast` on the
//! operand's reference at every `as` expression (official Dart SDK source,
//! `pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart`
//! :5296-5316 routing to :3140-3164), so a cast read as a statement
//! promotes its operand variable for the reads that follow: `x as T;`
//! leaves `x` at `T` until a write demotes or the block closes. The
//! semantic face (`row_resolution.rs::stage_as_cast_promotions`)
//! re-derives that promotion on the row face and publishes it on the
//! shared read-type table under the declared-face contract, so every
//! post-walk consumer — member resolution (`recv_expression_ty`) and the
//! extension `on T` bound face (`extension_member_bounds`) — reads the
//! promoted type exactly where the oracle's promoted receiver does.
//!
//! The pins discriminate on the function-typed cast target
//! (`x as Object Function();` — the census's residual shape, whose
//! promotion the flow layer's own publication does not cover) through
//! the member-access observable (`.call` resolves on the promoted
//! function face, reports `undefined_method` at the declared `Object`),
//! plus the window's three edges: no cast, a demoting write, and a read
//! that precedes the cast. The class-typed bound-face pin guards the
//! corpus witness the flow layer already keeps silent — the
//! declared-face contract defers to a differing staged face.

use super::FileReport;
use super::test_rig::{report_ops, rows};

/// The `undefined_method` rows of `report`, rendered for the failure
/// message.
fn undefined_methods(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_method")
        .map(|d| format!("{}@{}: {}", d.code.name, d.range.offset, d.message))
        .collect()
}

/// The `type_argument_not_matching_bounds` rows of `report`.
fn bounds(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "type_argument_not_matching_bounds")
        .map(|d| format!("{}@{}: {}", d.code.name, d.range.offset, d.message))
        .collect()
}

/// The corpus's `static_type_helper.dart` shapes (`extension
/// StaticType<T> on T` with `R extends Exactly<T>` — the explicit type
/// argument must conform to the receiver-instantiated bound).
const HELPERS: &str = "typedef Exactly<T> = T Function(T);\n\
     extension StaticType<T> on T {\n\
     \x20 T expectStaticType<R extends Exactly<T>>() => this;\n\
     }\n";

/// A function-typed cast target promotes the operand: the following
/// `.call` read resolves on the promoted `Object Function()` face and
/// stays silent. The flow layer's own publication leaves this shape at
/// the declared face — the staging is the publication the member walk
/// reads (the census's residual 94:22 mechanism).
#[test]
fn function_typed_as_cast_promotes_the_following_read() {
    let fixture = "\
void f(Object Function() g) {
  Object x = g;
  x as Object Function();
  x.call();
}
";
    let report = report_ops(fixture);
    let bad = undefined_methods(&report);
    assert!(
        bad.is_empty(),
        "the promoted read must resolve `.call` on the cast type: {bad:?}\n{}",
        rows(&report).join("\n")
    );
}

/// The unpromoted baseline: without the cast, the read stays at the
/// declared `Object` and `.call` reports — the staging must not widen
/// faces no cast promoted.
#[test]
fn unpromoted_baseline_still_reports_the_member_miss() {
    let fixture = "\
void f(Object Function() g) {
  Object x = g;
  x.call();
}
";
    let report = report_ops(fixture);
    let bad = undefined_methods(&report);
    assert!(
        bad.len() == 1,
        "the unpromoted read must report the declared face once: {bad:?}\n{}",
        rows(&report).join("\n")
    );
}

/// A demoting write ends the promotion: after `x = g;` the variable is
/// back at its declared face and the read reports again (the official
/// assignment retraction — `writeCapture` removes the standing
/// promotions at the write).
#[test]
fn demoting_write_ends_the_promotion() {
    let fixture = "\
void f(Object Function() g) {
  Object x = g;
  x as Object Function();
  x = g;
  x.call();
}
";
    let report = report_ops(fixture);
    let bad = undefined_methods(&report);
    assert!(
        bad.len() == 1,
        "the post-write read must report the demoted face once: {bad:?}\n{}",
        rows(&report).join("\n")
    );
}

/// A read that precedes the cast keeps the declared face: the promotion
/// window opens at the cast statement, never before it.
#[test]
fn read_before_the_cast_stays_at_the_declared_face() {
    let fixture = "\
void f(Object Function() g) {
  Object x = g;
  x.call();
  x as Object Function();
}
";
    let report = report_ops(fixture);
    let bad = undefined_methods(&report);
    assert!(
        bad.len() == 1,
        "the pre-cast read must report the declared face once: {bad:?}\n{}",
        rows(&report).join("\n")
    );
}

/// The class-typed bound-face witness (the corpus's 30:22/32:22 family)
/// stays silent: the flow layer already publishes that promotion, and
/// the declared-face contract defers to a differing staged face — the
/// bound instantiates `T := C` and `Exactly<C>` conforms.
#[test]
fn class_typed_as_cast_promotion_stays_silent() {
    let fixture = format!(
        "{HELPERS}\
class B {{}}
class C extends B {{}}
void f() {{{{
  B x = C();
  x as C;
  x.expectStaticType<Exactly<C>>();
}}}}
"
    );
    let report = report_ops(&fixture);
    let bad = bounds(&report);
    assert!(
        bad.is_empty(),
        "the class-typed promotion must keep the bound silent: {bad:?}\n{}",
        rows(&report).join("\n")
    );
}

/// The unpromoted bound-face baseline (named types, decidable): without
/// the cast the bound instantiates `T := B` and `Exactly<C>` does not
/// conform — one row, the corpus family's verdict direction.
#[test]
fn unpromoted_bound_baseline_still_reports() {
    let fixture = format!(
        "{HELPERS}\
class B {{}}
class C extends B {{}}
void f() {{{{
  B x = C();
  x.expectStaticType<Exactly<C>>();
}}}}
"
    );
    let report = report_ops(&fixture);
    let bad = bounds(&report);
    assert!(
        bad.len() == 1 && bad[0].contains("'Exactly<B>'"),
        "the unpromoted read must report the declared bound once: {bad:?}\n{}",
        rows(&report).join("\n")
    );
}
