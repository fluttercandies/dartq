//! #3202 — the `args` RefCell re-entrancy panic in `subst_ty`'s function
//! leg (and its `closure_subst_param_ty` twin).
//!
//! The frame pair, confirmed on the release binary
//! (`RefCell already mutably borrowed` at `types.rs`'s `pool()`,
//! reached through `check_arguments → assignable → fn_sig_subtype →
//! subst_ty → subst_ty`): passing a generic tear-off whose return is
//! itself a function type makes `fn_sig_subtype` run `subst_ty` over
//! that return face; the function leg read the shape, substituted its
//! formals, then held `args.borrow_mut()` across the RECURSIVE
//! `subst_ty(return_ty)` — and the inner call re-entered the pool
//! through `pool()`'s `borrow()` the moment that inner return was
//! compound (`List<int>` — a named head with a written argument list).
//! Every existing higher-order fixture spelled the inner return
//! `void`/`int` (non-compound leaves that never read the pool), which
//! is why the rig never fired the pair before.
//!
//! The fix owns the return face before the re-intern guard, the same
//! discipline `canonical_dotted_ty`'s function leg and #2992's
//! alias-fold rebuild already follow; interning is content-deduped, so
//! the hoist moves no verdict.
//!
//! The live repro the sweep crashed on is the third fixture (extension
//! types): the crash path is the generic assignability, not the
//! extension-type face itself.

use crate::diagnose::test_rig::report_ops;
use crate::FileReport;

fn all_codes(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

/// The frame pair distilled: a generic tear-off (`mk`, whose return
/// face is the function type `List<int> Function(int)`) checked
/// against its instantiated non-generic host — `assignable`'s
/// generic-instantiation arm runs `subst_ty` over the return face, the
/// function leg substitutes the formals, holds the re-intern guard,
/// and the inner return substitution reads the pool for `List<int>`'s
/// argument list. Oracle 3.13.1: the instantiated tear-off meets the
/// declared type exactly (ops face silent apart from the
/// unused-variable lint).
#[test]
fn instantiated_tear_off_compound_return_completes() {
    let report = report_ops(
        "List<int> Function(int) mk<T>(T val) => (int i) => <int>[];\n\
         void main() {\n  \
         List<int> Function(int) Function(int) g = mk;\n\
         }",
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name == "unused_local_variable"),
        "{:?}",
        all_codes(&report)
    );
}

/// The same shape with the compound return face mismatched
/// (`List<String>` vs `List<int>`): the hoisted substitution must
/// still RUN and still DECIDE — oracle reports `invalid_assignment`
/// at the initializer.
#[test]
fn instantiated_tear_off_compound_return_mismatch_reports() {
    let report = report_ops(
        "List<String> Function(int) mk<T>(T val) => (int i) => <String>[];\n\
         void main() {\n  \
         List<int> Function(int) Function(int) g = mk;\n\
         }",
    );
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .collect();
    assert_eq!(
        rows.len(),
        1,
        "the mismatched compound return still reports: {:?}",
        all_codes(&report)
    );
}

/// The `closure_subst_param_ty` twin (scopes_bodies): a closure literal
/// under an OLD-STYLE generic typedef context (`typedef List<X>
/// Function(int) F<X>();` — the spelling `fold_alias_ty` misses, so
/// `closure_typedef_function_ty` instantiates the RHS itself) runs the
/// use-site substitution through the same function leg on the return
/// face `List<X> Function(int)` — the identical guard-held return
/// recursion, fixed by the identical hoist. Oracle 3.13.1: the
/// initializer meets `F<int>` exactly (ops face silent apart from the
/// unused-variable lint). The rig's closure-context channel does not
/// reach the twin's guarded line for this fixture (the old-style arm
/// bails earlier here), so this pin guards the face's verdicts, not
/// the hoist itself — the release-binary sweep corpus covers the twin
/// path with real inputs.
#[test]
fn closure_typedef_instantiation_compound_return_completes() {
    let report = report_ops(
        "typedef List<X> Function(int) F<X>();\n\
         void main() {\n  \
         F<int> g = () => (int i) => <int>[];\n\
         }",
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name == "unused_local_variable"),
        "{:?}",
        all_codes(&report)
    );
}

/// The mismatched twin inner return (`List<String>` vs the
/// instantiated `List<int>`): the hoisted substitution completes and
/// never invents a row — oracle's only verdict here is the
/// closure-context return row (`return_of_invalid_type_from_closure`,
/// an under-report face outside this ticket), so every row the engine
/// does emit must be one of the two oracle faces.
#[test]
fn closure_typedef_instantiation_compound_return_mismatch_completes() {
    let report = report_ops(
        "typedef List<X> Function(int) F<X>();\n\
         void main() {\n  \
         F<int> g = () => (int i) => <String>[];\n\
         }",
    );
    assert!(
        report.diagnostics.iter().all(|d| {
            d.code.name == "unused_local_variable"
                || d.code.name == "return_of_invalid_type_from_closure"
        }),
        "{:?}",
        all_codes(&report)
    );
}

/// The sweep's live repro (release binary): the extension-type
/// primary/secondary-constructor corpus case whose generic
/// assignability crossed the panicking frame pair. Completing without
/// panicking is the pin (oracle 3.13.1 reports the file's real defect —
/// `duplicate_constructor`, two unnamed constructors on `G` — plus the
/// unused-variable lint, neither a crash).
#[test]
fn extension_type_repro_completes() {
    let report = report_ops(
        "extension type E(int it) {}\n\
         extension type const G(int it) { const G(this.it); }\n\
         void main() { var e = E(1); }",
    );
    let codes = all_codes(&report);
    assert!(
        codes
            .iter()
            .all(|code| *code == "duplicate_constructor" || *code == "unused_local_variable"),
        "{codes:?}"
    );
}
