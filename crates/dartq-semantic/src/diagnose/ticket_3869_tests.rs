//! #3869 — the two member-walk miss closures, pinned at the oracle rows:
//!
//! 1. The accessor-callee `Function` variant on the MISS arms: the
//!    member-walk misses (`frozen_member_verdict` `Defined`, the SDK-stub
//!    `Missing`+`Defined` face, `check_core_type_member`'s table arm,
//!    `member_defined`'s core-table claim) run the same callable-result
//!    check the `Walk::Found` arm does — `receiver_static_ty` →
//!    `potentially_nullable` → `invoke_callee_function_shaped` →
//!    `emit_unchecked_nullable(_, "", Function)` — because
//!    `x.first()` over `List<T?>` (`T extends Function`) resolves the
//!    accessor through the core face, never through a walked entry
//!    (function_expression_invocation_resolver.dart's nullable-function
//!    arm; the oracle rewrites the section to a `FunctionExpressionInvocation`
//!    and reports on its whole `function` node).
//! 2. The callee's type on that miss path: `callee_return_ty` projects
//!    `core_member_return`'s `CoreRet::ElemArg(0)` receiver-argument face
//!    instead of answering `Unknown` — `Iterable<E>.first` is
//!    `E get first` (iterable.dart), so the callee reads the receiver's
//!    written element type verbatim (`T?`), which the read face's own
//!    fallback ([`Engine::core_element_read_ty`]) already supplied for
//!    `??=` and this arm now supplies for `Invoke`.
//! 3. The abstention gate behind the same ticket's assignability face:
//!    `fn_sig_subtype`'s `sig_has_unknown` worklist asks the `InvalidType`
//!    FACE ([`Engine::is_invalid_ty`]), not bare-sentinel equality — the
//!    844-row family's rendered `InvalidType` is an UNRESOLVABLE `Named`
//!    head (`void Function(x)` over an out-of-scope `x`), which the
//!    sentinel alone never answers (upstream Dart SDK source,
//!    subtype.dart:45-47's InvalidType-suppresses rule). A decided
//!    mismatch keeps its row — the negative side rides
//!    `ticket_3993_tests::decided_fn_mismatch_still_reports`.
//!
//! Oracle provenance: `dart analyze --format=machine` (3.13.7 oracle) on
//! the three minimal fixtures — `UNCHECKED_USE_OF_NULLABLE_VALUE|2|3|7|The
//! function can't be unconditionally invoked because it can be 'null'.`
//! for the callee face, ZERO rows for the InvalidType-bearing pair, and
//! `ARGUMENT_TYPE_NOT_ASSIGNABLE|1|54|1` for the decided mismatch.

use super::test_rig::{CORE_PRELUDE, report_ops, rows};
use crate::diagnose::FileReport;

/// The absolute fixture offset of `needle` (the rig's prelude included).
fn at(fixture: &str, needle: &str) -> usize {
    CORE_PRELUDE.len() + fixture.find(needle).unwrap()
}

/// One `unchecked_use_of_nullable_value` row anchored at `offset:len`
/// carrying `message`.
#[track_caller]
fn assert_unchecked(report: &FileReport, offset: usize, len: u32, message: &str) {
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == "unchecked_use_of_nullable_value"
                && d.range.offset as usize == offset
                && d.range.length == len
        })
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "expected one unchecked row: {:#?}",
        rows(report)
    );
    assert_eq!(hits[0].message, message, "{:#?}", rows(report));
}

const FUNCTION_TEXT: &str =
    "The function can't be unconditionally invoked because it can be 'null'.";

/// `x.first()` over `List<T?>` (`T extends Function`): the walk misses
/// `first` on the rig `List`, the core-table claim resolves the accessor,
/// `callee_return_ty`'s `ElemArg(0)` face types the callee `T?`, and the
/// miss arm's function-variant check reports at the whole callee
/// `x.first` (offset 2/col 3/length 7 in the machine row). The acceptance
/// latch also rides `ticket_2919_tests::pin_member_callee_function_nullable`
/// and `ticket_3816_tests::member_callee_function_nullable`.
#[test]
fn member_callee_function_miss_path_oracle_row() {
    let src = "m<T extends Function>(List<T?> x) {\n  x.first();\n}\n";
    let report = report_ops(src);
    assert_unchecked(&report, at(src, "x.first"), 7, FUNCTION_TEXT);
}

/// `void argfx(void f(x)) {} void topx(x) {} main() { argfx(topx); }`:
/// both signatures bear the `InvalidType` face (the unresolvable `x`
/// head), so `fn_sig_subtype` abstains ahead of its arity gates and the
/// argument check stays silent — the oracle reports nothing on this
/// fixture. Sentinel equality alone (`ty == Ty::Unknown`) missed the
/// unresolvable `Named` head and let the row invent.
#[test]
fn invalid_type_bearing_fn_sig_abstains() {
    let report = report_ops(
        "void argfx(void f(x)) {}\n\
         void topx(x) {}\n\
         void main() { argfx(topx); }\n",
    );
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "argument_type_not_assignable")
        .collect();
    assert!(
        hits.is_empty(),
        "the InvalidType-bearing pair abstains: {:#?}",
        rows(&report)
    );
}
