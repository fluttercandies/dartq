//! Ticket #3689 pins (census-12 lane N): `conflicting_generic_interfaces`.
//!
//! Three roots were over- and under-reporting the code. Every fixture below
//! carries the verdict of the pinned oracle (`dart analyze` 3.13.7) for the
//! exact source, so a pin that disagrees with the oracle cannot pass.
//!
//! 1. **A raw mixin face in a `with` clause is *inferred*, and the bound is
//!    the fallback.** `_MixinInference` (upstream Dart SDK source,
//!    `pkg/analyzer/lib/src/summary2/types_builder.dart` :497-640,
//!    `_inferSingle` :566, `_findInterfaceTypesForConstraints` :545) matches
//!    the mixin's `on` constraints against the interfaces collected so far
//!    and binds its parameters from the match. The bounds win in exactly two
//!    situations: no solution at all, and a solution that violates a
//!    parameter's own bound (`chooseTypes` merges the bound into the
//!    parameter's constraints — `pkg/_fe_analyzer_shared/lib/src/
//!    type_inference/type_analyzer_operations.dart` :1080-1146). The three
//!    discriminator pins below are the oracle's own triage:
//!
//!    | shape | oracle |
//!    |---|---|
//!    | `mixin M1<T> on A<T>`, `class C extends A<B> with M1` | silent |
//!    | `mixin Fwd<T extends Dir> on F<T, ioDir>`, `class Local extends F<Local, ioDir> with Fwd` | `'F<Local, ioDir>'` / `'F<Dir, ioDir>'` |
//!    | same, `LocalDir extends Dir` | silent |
//!
//!    Instantiating every raw face to bounds instead fabricated
//!    `A<B>` / `A<dynamic>` — the census-12 family that dominated the extra
//!    side (`type_parameter_inference_*`, `mixin_declaration_inference_*`,
//!    `regress34404_*`).
//! 2. **The clause scanner must read fused `>>` closers one `<...>` group at
//!    a time** (the parser splits the same way, `cursor.rs` `splitGtGt`). A
//!    face written `A<List<int>>` used to fail the scan, dropping the whole
//!    header and losing every conflict the class owned (co19
//!    `override_checking_A06_t078..t150`, `Extension-types/…_A03_t04/t06`).
//! 3. **A bare generic type argument instantiates to bounds** at resolution
//!    time: `A<FutureOr>` reads `A<FutureOr<dynamic>>`, and `NORM` then
//!    collapses a `FutureOr` over a top type to that top type
//!    (`normalize.dart` `_futureOr`), so the running merged interface is
//!    `A<dynamic>` (co19 `override_checking_A06_t063/t065/t066/t150` and the
//!    silent `t064/t067/t068`).
//!
//! The pins are mostly silences — the compatible combinations the oracle
//! leaves quiet — plus the genuine errors that must keep reporting with the
//! oracle's exact wording.

use super::test_rig::{assert_silent, report_ops, rows};

/// The messages of every `conflicting_generic_interfaces` row, in report
/// order.
fn conflicts(report: &crate::diagnose::FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code.name == "conflicting_generic_interfaces")
        .map(|diagnostic| diagnostic.message.to_string())
        .collect()
}

// —— the mixin-inference discriminator ——————————————————————————————————

/// Discriminator (A): the mixin parameter has **no bound**, so the inferred
/// `T = B` is accepted and the class implements `A<B>` twice with the same
/// arguments.
///
/// Oracle: `No issues found!`
#[test]
fn raw_mixin_without_bound_infers() {
    let report = report_ops(
        "class A<T> {}\n\
         class B {}\n\
         mixin M1<T> on A<T> {}\n\
         class C extends A<B> with M1 {}\n",
    );
    assert_silent(&report);
}

/// Discriminator (B): the inferred `T = Local` violates `T extends Dir`, so
/// the whole inference is discarded and the bound `Dir` instantiates the
/// mixin — the two faces then genuinely conflict.
///
/// Oracle: `error - The class 'Local' can't implement both 'F<Local, ioDir>'
/// and 'F<Dir, ioDir>' because the type arguments are different.`
#[test]
fn inferred_argument_violating_its_bound_falls_back_to_the_bound() {
    let report = report_ops(
        "class F<T, D> {}\n\
         class Dir {}\n\
         class ioDir {}\n\
         mixin Fwd<T extends Dir> on F<T, ioDir> implements Dir {}\n\
         class Local extends F<Local, ioDir> with Fwd {}\n",
    );
    assert_eq!(
        conflicts(&report),
        [
            "The class 'Local' can't implement both 'F<Local, ioDir>' and 'F<Dir, ioDir>' because the type arguments are different."
        ],
        "{:#?}",
        rows(&report)
    );
}

/// Discriminator (C): the same mixin, applied where the inferred
/// `T = LocalDir` *does* satisfy `T extends Dir` — the inference stands and
/// the class is silent.
///
/// Oracle: `No issues found!`
#[test]
fn inferred_argument_satisfying_its_bound_stays_silent() {
    let report = report_ops(
        "class F<T, D> {}\n\
         class Dir {}\n\
         class ioDir {}\n\
         class LocalDir extends Dir {}\n\
         mixin Fwd<T extends Dir> on F<T, ioDir> implements Dir {}\n\
         class Local extends F<LocalDir, ioDir> with Fwd {}\n",
    );
    assert_silent(&report);
}

/// A named mixin application infers each raw mixin in turn, and each inferred
/// mixin joins the pool the next one is matched against
/// (`class A00 = I<int> with M1, Extra`).
///
/// Oracle: `No issues found!`
#[test]
fn named_mixin_application_infers_each_mixin_in_turn() {
    let report = report_ops(
        "class I<X> {}\n\
         mixin M1<T> on I<T> {}\n\
         mixin Extra on M1<int> {}\n\
         class A00 = I<int> with M1, Extra;\n",
    );
    assert_silent(&report);
}

/// Two `on` constraints solve two mixin parameters
/// (`mixin M4<S, T> on I<S>, J<T>` applied to a supertype chain that
/// implements `I<int>` and `J<double>`).
///
/// Oracle: `No issues found!`
#[test]
fn mixin_inference_solves_every_on_constraint() {
    let report = report_ops(
        "class I<X> {}\n\
         class J<X> {}\n\
         class C2 extends I<int> implements J<double> {}\n\
         mixin M4<S, T> on I<S>, J<T> {}\n\
         class A20 extends C2 with M4 {}\n",
    );
    assert_silent(&report);
}

/// A mixin parameter no `on` constraint mentions keeps its bound (the
/// official `chooseFinalTypes` bound fallback), so `mixin B<X, Y extends
/// void Function(X)> on A<X>` applied raw to `class C<X> extends A<X>` binds
/// `X` from the constraint and does not fabricate `A<dynamic>`.
///
/// Oracle: `conflicting_generic_interfaces` absent (the fixture's only row is
/// `wrong_type_parameter_variance_in_superinterface` for `B`).
#[test]
fn unmentioned_mixin_parameter_keeps_its_bound() {
    let report = report_ops(
        "class A<X> {}\n\
         mixin B<X, Y extends void Function(X)> on A<X> {}\n\
         class C<X> extends A<X> with B {}\n",
    );
    assert!(
        conflicts(&report).is_empty(),
        "the unbound parameter takes its bound, not `dynamic`: {:#?}",
        rows(&report)
    );
}

/// Silence pin: the inference pool is the supertype chain, so a mixin whose
/// `on` constraint names a bounded parameter still solves it when the
/// supertype's argument satisfies that bound.
///
/// Oracle (`features/_core/regress/language/regress/regress32353_test`, the
/// `_LocalDirectory` reduction): `conflicting_generic_interfaces` absent (the
/// only row is `unused_element` for the unreferenced private class).
#[test]
fn mixin_on_constraint_infers_over_a_bounded_parameter() {
    let report = report_ops(
        "class _LocalDirectory extends _LocalFileSystemEntity<_LocalDirectory, io_Directory> {}\n\
         abstract class _LocalFileSystemEntity<T extends FileSystemEntity, D extends io_FileSystemEntity> \
         extends ForwardingFileSystemEntity<T, D> {}\n\
         abstract class FileSystemEntity implements io_FileSystemEntity {}\n\
         abstract class ForwardingFileSystemEntity<T extends FileSystemEntity, D extends io_FileSystemEntity> \
         implements FileSystemEntity {}\n\
         class io_FileSystemEntity {}\n\
         class io_Directory extends io_FileSystemEntity {}\n",
    );
    assert!(
        conflicts(&report).is_empty(),
        "the chain satisfies `D extends io_FileSystemEntity`: {:#?}",
        rows(&report)
    );
}

/// Silence pin: a mixin's `on` constraint whose argument comes from the
/// mixin's own parameter stays satisfied through a long supertype chain.
///
/// Oracle (`features/_core/regress/language/regress/regress34404_flutter_test`):
/// `conflicting_generic_interfaces` absent (the only row is `unused_element`
/// for the unreferenced private class).
#[test]
fn flutter_state_mixin_chain_stays_silent() {
    let report = report_ops(
        "class State<T extends StatefulWidget> {}\n\
         abstract class StatefulWidget extends Widget {}\n\
         abstract class Widget {}\n\
         class Dismissible extends StatefulWidget {}\n\
         mixin TickerProviderStateMixin<T extends StatefulWidget> on State<T> {}\n\
         class _DismissibleState extends State<Dismissible> with TickerProviderStateMixin {}\n",
    );
    assert!(
        conflicts(&report).is_empty(),
        "`Dismissible` satisfies `T extends StatefulWidget`: {:#?}",
        rows(&report)
    );
}

// —— genuine errors that must keep reporting ——————————————————————————

/// A `with` face whose mixin *implements* the generic interface with
/// different arguments genuinely conflicts.
///
/// Oracle: `error - The class 'C' can't implement both 'A<int>' and
/// 'A<String>' because the type arguments are different.`
#[test]
fn mixin_implements_face_with_other_arguments_conflicts() {
    let report = report_ops(
        "class A<T> {}\n\
         mixin M<T> implements A<T> {}\n\
         class C extends A<int> with M<String> {}\n",
    );
    assert_eq!(
        conflicts(&report),
        [
            "The class 'C' can't implement both 'A<int>' and 'A<String>' because the type arguments are different."
        ],
        "{:#?}",
        rows(&report)
    );
}

/// A mixin whose own `on` constraints disagree reports on the *mixin*
/// declaration, with the mixin kind word.
///
/// Oracle (`co19 Super-mixins/compatible_t05`): `error - The mixin 'M' can't
/// implement both 'A<X>' and 'A<Y>' because the type arguments are
/// different.`
#[test]
fn conflicting_on_constraints_report_on_the_mixin() {
    let report = report_ops(
        "class A<T> {}\n\
         mixin M<X, Y> on A<X>, A<Y> {}\n",
    );
    assert_eq!(
        conflicts(&report),
        [
            "The mixin 'M' can't implement both 'A<X>' and 'A<Y>' because the type arguments are different."
        ],
        "{:#?}",
        rows(&report)
    );
}

// —— the fused-`>>` closer ————————————————————————————————————————————

/// Silence pin (fused closers): `implements A<List<int>>` closes two groups
/// with the scanner's single `>>` token; the face must still parse, and the
/// repeated element then merges with identical arguments.
#[test]
fn nested_type_arguments_with_a_fused_closer_still_merge() {
    let report = report_ops(
        "class A<T> {}\n\
         class B implements A<List<int>> {}\n\
         class D extends B implements A<List<int>> {}\n",
    );
    assert_silent(&report);
}

/// The genuine error behind a fused closer: `A<List<int>>` versus
/// `A<String>` at the same element is a conflict, and the message spells the
/// nested group inside the `>>` run.
#[test]
fn nested_type_arguments_with_a_fused_closer_conflict() {
    let report = report_ops(
        "class A<T> {}\n\
         class B implements A<List<int>> {}\n\
         class D extends B implements A<String> {}\n",
    );
    assert_eq!(
        conflicts(&report),
        [
            "The class 'D' can't implement both 'A<List<int>>' and 'A<String>' because the type arguments are different."
        ],
        "{:#?}",
        rows(&report)
    );
}

// —— the bare generic argument ————————————————————————————————————————

/// A bare generic type argument instantiates to bounds, and the message
/// spells the instantiation: `A<List>` is `A<List<dynamic>>`.
#[test]
fn bare_generic_argument_instantiates_to_bounds() {
    let report = report_ops(
        "class A<T> {}\n\
         class B implements A<List> {}\n\
         class D extends B implements A<String> {}\n",
    );
    assert_eq!(
        conflicts(&report),
        [
            "The class 'D' can't implement both 'A<List<dynamic>>' and 'A<String>' because the type arguments are different."
        ],
        "{:#?}",
        rows(&report)
    );
}

/// `NORM(FutureOr<dynamic>)` is `dynamic` (a top type), so the running merged
/// interface reads `A<dynamic>` — the first operand of the oracle's message.
///
/// The rig's names-only `dart:core` stub carries `FutureOr`, so the fixture
/// must not declare one of its own: a user-declared `FutureOr` is an ordinary
/// class and does not collapse (`A<FutureOr<dynamic>>`).
///
/// Oracle (`override_checking_A06_t063`, `dart:async` imported): `'A<dynamic>'
/// and 'A<int>'`.
#[test]
fn future_or_over_a_top_argument_collapses_to_the_top_type() {
    let report = report_ops(
        "class A<T> {}\n\
         class B implements A<FutureOr> {}\n\
         class D extends B implements A<int> {}\n",
    );
    assert_eq!(
        conflicts(&report),
        [
            "The class 'D' can't implement both 'A<dynamic>' and 'A<int>' because the type arguments are different."
        ],
        "{:#?}",
        rows(&report)
    );
}

/// Silence pin: `FutureOr<dynamic>` collapses to a top type, which NORM leaves
/// alone, so the `implements` chain's `CheckTopMerge<FutureOr>` becomes
/// `CheckTopMerge<dynamic>` and merges with the supertype's own face.
///
/// Oracle (`override_checking_A06_t067`): no `conflicting_generic_interfaces`
/// rows.
#[test]
fn future_or_over_dynamic_merges_with_dynamic() {
    let report = report_ops(
        "class CheckTopMerge<T> {}\n\
         class B extends CheckTopMerge<FutureOr> {}\n\
         class C extends CheckTopMerge<dynamic> {}\n\
         class D1 extends C implements B {}\n",
    );
    assert_silent(&report);
}
