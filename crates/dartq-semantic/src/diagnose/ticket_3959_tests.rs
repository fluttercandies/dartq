//! #3959 — the alias-fold's re-interns carry the honest
//! required-positional count.
//!
//! The diagnosing fold's own re-interns were split-blind:
//! `substitute_alias_rhs` (the parameterized-alias use-site substitution)
//! and `with_use_question` (the `?`-unit rebuild) rebuilt every
//! `Ty::Function` through the all-required producers, so a substituted
//! use re-merged into the all-required spelling and shared one
//! `FnShapeId` with it — BE's `repair_alias_fold_split` gates on bare
//! alias heads (`args == EMPTY`), so parameterized uses escaped the
//! is-verdict repair. The element-model signature fallback
//! (`element_model_alias_sig`) claimed `positional.len()` for the same
//! reason. The rebuilds now route the honest count through the
//! `function_required` / `function_generic_required` producers: the
//! split is identity ([`FnShape::required_positional`]; upstream Dart
//! SDK source, pkg/analyzer/lib/src/dart/element/type.dart
//! `FunctionTypeImpl` `operator ==` :255-284 deciding over
//! `_equalParameters` :488-517, which compares each formal's
//! `isOptional`), and the required rule (`_isFunctionSubtypeOf`,
//! pkg/analyzer/lib/src/dart/element/subtype.dart :338-400 — a required
//! source formal against a non-required target formal answers false at
//! :362-370) decides on the real split.
//!
//! Oracle rows below were captured against `dart analyze --format
//! machine` 3.13.7. Residuals routed (not pinned here — their owning
//! capture/consumer faces sit outside this ticket's file set): the
//! COLLECTOR's armed function-type tree never latches the optional
//! group (`begin_optional_formal_parameters` relays
//! `TreeSink::function_optional_group` in bounds.rs :1584-1594 only —
//! the collector's own handler at collector.rs :6543 does not), so a
//! `typedef OptT<T> = void Function([T])` RHS arms its declaration type
//! all-required and the substituted use inherits that shape until that
//! relay lands; the same relay is absent from the clause-tree sinks
//! (clause_capture.rs). The bare-alias override face and the
//! trailing-typedef arity face additionally read `function_shape_sig`'s
//! all-required `FnVarSig` rebuild in assignability.rs — the invocation
//! arity row a trailing `typedef int Ot([int c])` callable fabricates
//! (`f()` over an optional-positional callable, oracle silent) rides
//! that rebuild, not this ticket's fallback claim.

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

// —— The substituted face keeps its identity (true-direction controls) ——

/// `opt is ReqT<int>` over `typedef ReqT<T> = void Function(T)` — an
/// optional source formal implements a required target formal, so the
/// check is constant-true (oracle: one row). The target substitutes
/// through the rebuilt face, so the honest producers must not
/// over-correct the true direction.
#[test]
fn optional_operand_vs_substituted_required_alias_reports_true() {
    let report = report_ops(
        "typedef ReqT<T> = void Function(T);\n\
         void opt([int? x]) {}\n\
         void main() {\n\
         \x20 if (opt is ReqT<int>) print(1);\n\
         }",
    );
    assert_eq!(
        type_check_rows(&report),
        1,
        "an optional source implements a required target: {:?}",
        rows(&report)
    );
}

/// Identical spellings through the substituted face stay equal:
/// `OptT<int> v` declared and checked against the same spelling —
/// constant-true (oracle: one row). The operand and target fold through
/// the same substitution rebuild, so their shapes must intern to one
/// slot, not split apart.
#[test]
fn substituted_alias_identical_spellings_stay_equal() {
    let report = report_ops(
        "typedef OptT<T> = void Function([T]);\n\
         void probe(OptT<int> v) {\n\
         \x20 if (v is OptT<int>) print(1);\n\
         }",
    );
    assert_eq!(
        type_check_rows(&report),
        1,
        "the identical spelling is constant-true: {:?}",
        rows(&report)
    );
}

/// The same identity through the capture-fed face: `int g1([int? c])`
/// against `F1<int>` over the old-style generic alias
/// `typedef int F1<T>([T? c])` — the identical spelling, constant-true
/// (oracle: one row).
#[test]
fn old_style_alias_identical_spellings_stay_equal() {
    let report = report_ops(
        "typedef int F1<T>([T? c]);\n\
         int g1([int? c]) => 0;\n\
         void main() {\n\
         \x20 if (g1 is F1<int>) print(1);\n\
         }",
    );
    assert_eq!(
        type_check_rows(&report),
        1,
        "the identical spelling is constant-true: {:?}",
        rows(&report)
    );
}

/// The capture-fed face's silence control: `int f1(int x)` against
/// `F1<int>` — a required source formal against the optional target
/// formal is statically false (subtype.dart:362-370), so the oracle
/// stays silent and the capture-fed verdict must keep that polarity.
#[test]
fn required_operand_vs_capture_fed_optional_alias_stays_silent() {
    let report = report_ops(
        "typedef int F1<T>([T? c]);\n\
         int f1(int x) => 0;\n\
         void main() {\n\
         \x20 if (f1 is F1<int>) print(1);\n\
         }",
    );
    assert!(
        type_check_rows(&report) == 0,
        "a required source cannot be a subtype of an optional target: \
         {:?}",
        all_codes(&report)
    );
}
