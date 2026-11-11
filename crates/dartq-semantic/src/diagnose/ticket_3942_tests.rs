//! #3942 pins — the declared-variance SUBTYPE face on the assignable side.
//!
//! Static return shapes carry no inference parameters, so their return check
//! rides the SUBTYPE/assignable face rather than the inference-driven
//! `infer_gather` arm that #3911 wired. The subtype site is
//! `Engine::covariant_args`, whose per-parameter relation is upstream's
//! `_interfaceArguments` (pkg/analyzer/lib/src/dart/element/subtype.dart:
//! 296-333): `out` asks `sub <: super`, `in` asks the reverse, `inout`
//! both, and an unmarked parameter keeps the legacy covariant default
//! (`TypeParameterElementImpl.variance`).
//!
//! The face must read the model's durable `Container::type_param_variances`
//! column, never the parse-time `var_containers` capture: that capture is
//! drained by the variance family's own finalize pass, which runs ahead of
//! the staged rows' teardown re-decision — so a capture read answered every
//! head all-covariant on the teardown path and every variant head
//! under-reported (only the direction whose covariant leg happens to fail
//! fired). The model column also covers a head declared LATER in the unit,
//! which the incremental capture had not reached when the row staged.
//!
//! Every expectation below is the 3.13.7 oracle's, verbatim.

use super::FileReport;
use super::test_rig::{code_names, report_ops_experiments_with, rows};
use dartq_language::Feature;

/// The rows of the assignability family (`return_of_invalid_type` /
/// `argument_type_not_assignable`) — the parser's own variance residue
/// (`non_type_as_type_argument`) and the `wrong_type_parameter_variance_*`
/// position faces are separate columns and stay out of the counts.
fn assignability_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| {
            matches!(
                d.code.name,
                "return_of_invalid_type" | "argument_type_not_assignable"
            )
        })
        .map(|d| format!("{} @{}: {}", d.code.name, d.range.offset, d.message))
        .collect()
}

fn variance_report(source: &str) -> FileReport {
    report_ops_experiments_with(source, &[], None, &[Feature::Variance])
}

/// The `out` and legacy heads keep the covariant leg: only the NARROWING
/// direction reports (`Cov<C?>` into `Cov<C>` / `Leg<C?>` into `Leg<C>`),
/// the widening one passes (`Cov<C>` into `Cov<C?>`, `Leg<C>` into
/// `Leg<C?>`). A regression that dropped the variance read to
/// all-invariant or all-contravariant would flip one of these.
#[test]
fn covariant_and_legacy_heads_keep_the_covariant_leg() {
    let report = variance_report(
        r#"class C {}

class Cov<out T> {}
Cov<C?> covWiden(Cov<C> a) => a;
Cov<C> covNarrow(Cov<C?> a) => a;

class Leg<T> {}
Leg<C?> legWiden(Leg<C> a) => a;
Leg<C> legNarrow(Leg<C?> a) => a;
"#,
    );
    let stated = assignability_rows(&report).join("\n");
    assert_eq!(
        assignability_rows(&report).len(),
        2,
        "expected the two narrowing directions only, got {stated} (all: {:?})",
        rows(&report)
    );
    assert!(
        stated.contains("'covNarrow'") && stated.contains("'legNarrow'"),
        "expected the rows anchored at covNarrow/legNarrow, got {stated}"
    );
    assert!(
        !stated.contains("'covWiden'") && !stated.contains("'legWiden'"),
        "the widening directions must stay silent, got {stated}"
    );
}

/// The `inout` head rejects BOTH directions in an ARGUMENT position (the
/// assignable face's other consumer): `Inv<C>` into an `Inv<C?>` formal and
/// `Inv<C?>` into an `Inv<C>` formal both report.
#[test]
fn invariant_head_rejects_both_argument_directions() {
    let report = variance_report(
        r#"class C {}

class Inv<inout T> {}
void takesWide(Inv<C?> x) {}
void takesNarrow(Inv<C> x) {}
void call(Inv<C> a, Inv<C?> b) {
  takesWide(a);
  takesNarrow(b);
}
"#,
    );
    let names = code_names(&report);
    let rows = names
        .iter()
        .filter(|name| **name == "argument_type_not_assignable")
        .count();
    assert_eq!(
        rows,
        2,
        "expected both invariant argument directions, got {names:?} ({:?})",
        self::rows(&report)
    );
}

/// The `in` head reverses the accepted pair in an ARGUMENT position:
/// `Contra<C?>` fits a `Contra<C>` formal (the reverse leg `C <: C?`), and
/// `Contra<C>` does NOT fit a `Contra<C?>` formal (the reverse leg `C? <:
/// C` fails).
#[test]
fn contravariant_head_reverses_the_argument_pair() {
    let report = variance_report(
        r#"class C {}

class Contra<in T> {}
void takesWide(Contra<C?> x) {}
void takesNarrow(Contra<C> x) {}
void call(Contra<C> a, Contra<C?> b) {
  takesWide(a);
  takesNarrow(b);
}
"#,
    );
    let stated = assignability_rows(&report).join("\n");
    assert_eq!(
        assignability_rows(&report).len(),
        1,
        "expected exactly one rejected argument, got {stated} (all: {:?})",
        rows(&report)
    );
    // The rejected call is `takesWide(a)` — the `Contra<C>` source. The
    // message names the pair in the rejected direction, which is what
    // distinguishes it from the accepted `takesNarrow(b)`.
    assert!(
        stated.contains(
            "The argument type 'Contra<C>' can't be assigned to the parameter type 'Contra<C?>'"
        ),
        "expected the rejected Contra<C> into Contra<C?> direction, got {stated}"
    );
    assert!(
        !stated.contains(
            "The argument type 'Contra<C?>' can't be assigned to the parameter type 'Contra<C>'"
        ),
        "the reversed direction must be accepted, got {stated}"
    );
}

/// An `inout` head with SEVERAL parameters rejects every mixed widening: a
/// pair differing in either slot is invariant there, so all four
/// combinations report.
#[test]
fn invariant_multi_parameter_rejects_every_mixed_pair() {
    let report = variance_report(
        r#"class C {}

class Pair<inout K, inout V> {}
Pair<C?, C?> bothWide(Pair<C, C> a) => a;
Pair<C, C> bothNarrow(Pair<C?, C?> a) => a;
Pair<C?, C> mixLeft(Pair<C, C?> a) => a;
Pair<C, C?> mixRight(Pair<C?, C> a) => a;
"#,
    );
    let names = code_names(&report);
    let rows = names
        .iter()
        .filter(|name| **name == "return_of_invalid_type")
        .count();
    assert_eq!(
        rows,
        4,
        "expected all four invariant directions, got {names:?} ({:?})",
        self::rows(&report)
    );
}

/// Variance nests: an `inout` head inside an `inout` head rejects both
/// directions at the OUTER level (each argument pair is itself invariant,
/// so neither direction's inner ask holds both ways).
#[test]
fn nested_invariant_head_rejects_both_directions() {
    let report = variance_report(
        r#"class C {}

class Inv<inout T> {}
Inv<Inv<C?>> widen(Inv<Inv<C>> a) => a;
Inv<Inv<C>> narrow(Inv<Inv<C?>> a) => a;
"#,
    );
    let names = code_names(&report);
    let rows = names
        .iter()
        .filter(|name| **name == "return_of_invalid_type")
        .count();
    assert_eq!(
        rows,
        2,
        "expected both nested invariant directions, got {names:?} ({:?})",
        self::rows(&report)
    );
}

/// The unspellable-head control: a head the index carries no container row
/// for (the rig's `dart:core` stub declares `Future` as a NAME only — the
/// SDK projection's names-only grade) reads all-covariant, the legacy
/// default every generic of the default SDK carries
/// (`element.dart:11560-11562`). `Future<C?>` into `Future<C>` reports and
/// the widening direction stays silent — the empty-column posture must not
/// collapse to invariant (which would report both) nor contravariant (which
/// would flip them).
#[test]
fn unspellable_head_keeps_the_legacy_covariant_reading() {
    let report = variance_report(
        r#"class C {}

Future<C?> widenFuture(Future<C> a) => a;
Future<C> narrowFuture(Future<C?> a) => a;
"#,
    );
    let stated = assignability_rows(&report).join("\n");
    assert_eq!(
        assignability_rows(&report).len(),
        1,
        "expected only the narrowing Future direction, got {stated} (all: {:?})",
        rows(&report)
    );
    assert!(
        stated.contains("'narrowFuture'"),
        "expected the row anchored at narrowFuture, got {stated}"
    );
}

/// A head declared AFTER the row's function still answers its variance: the
/// teardown re-decision runs once the whole unit is indexed, so the model
/// column covers a forward reference the incremental parse-time capture had
/// not reached. The oracle reports both directions here.
#[test]
fn forward_declared_invariant_head_rejects_both_directions() {
    let report = variance_report(
        r#"Inv<C?> h(Inv<C> a) => a;
Inv<C> k(Inv<C?> a) => a;

class C {}

class Inv<inout T> {}
"#,
    );
    let names = code_names(&report);
    let rows = names
        .iter()
        .filter(|name| **name == "return_of_invalid_type")
        .count();
    assert_eq!(
        rows,
        2,
        "expected both directions over the forward-declared head, got {names:?} ({:?})",
        self::rows(&report)
    );
}
