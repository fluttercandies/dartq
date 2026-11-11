//! #3988 — the collector's `begin_optional_formal_parameters` relays
//! `TreeSink::function_optional_group`, so every collector-armed function
//! annotation carries the written required/optional split instead of
//! interning all-required.
//!
//! `TreeSink::function_optional_group` (`capture.rs`) records where a
//! function type's optional-positional group `[` begins — the
//! interned shape's `required_positional` boundary (upstream
//! `FunctionTypeImpl`'s identity: `type.dart` `operator ==` :255-284
//! deciding over `_equalParameters` :488-517, which compares each
//! formal's `isOptional`; the required rule is `_isFunctionSubtypeOf`,
//! `subtype.dart` :338-400 — a required source formal against a
//! non-required target formal answers false at :362-370). The diagnose
//! side relays it through `bounds.rs`'s `alias_optional_group`, but the
//! collector's own handler at `collector.rs`'s
//! `begin_optional_formal_parameters` never relayed it into
//! `builder.capture`: `typedef OptT<T> = void Function([T])` armed
//! `decl.var_ty` as `void Function(T)` — all-required — so a required
//! operand checked against it fabricated `unnecessary_type_check`
//! (constant-true) where the oracle stays silent (a required source
//! cannot implement an optional target).
//!
//! Oracle rows below were captured against `dart analyze --format
//! machine` 3.13.7.

use super::test_rig::{report_ops, rows};
use super::*;

fn type_check_rows(report: &FileReport) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unnecessary_type_check")
        .count()
}

fn all_codes(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

/// `req is OptT<int>` over `typedef OptT<T> = void Function([T])`: a
/// required source formal (`void req(int x)`) against the target's
/// optional formal is statically false (subtype.dart:362-370), so the
/// oracle reports nothing — the collector's armed shape must carry the
/// `[` boundary for the verdict to keep that polarity. Before the
/// relay the armed `decl.var_ty` interned all-required, and the check
/// read as constant-true.
#[test]
fn required_operand_vs_collector_fed_optional_alias_stays_silent() {
    let report = report_ops(
        "typedef OptT<T> = void Function([T]);\n\
         void req(int x) {}\n\
         void main() {\n\
         \x20 if (req is OptT<int>) print(1);\n\
         }",
    );
    assert!(
        type_check_rows(&report) == 0,
        "a required source cannot be a subtype of an optional target: \
         {:?}",
        all_codes(&report)
    );
}

/// The positive control: `req is ReqT<int>` over `typedef ReqT<T> =
/// void Function(T)` is constant-true — a required source formal
/// implements a required target formal, so the oracle reports one
/// `unnecessary_type_check` row. The optional-split relay must not
/// over-correct the all-required spelling.
#[test]
fn required_operand_vs_required_alias_still_reports_true() {
    let report = report_ops(
        "typedef ReqT<T> = void Function(T);\n\
         void req(int x) {}\n\
         void main() {\n\
         \x20 if (req is ReqT<int>) print(1);\n\
         }",
    );
    assert_eq!(
        type_check_rows(&report),
        1,
        "a required source implements a required target: {:?}",
        rows(&report)
    );
}
