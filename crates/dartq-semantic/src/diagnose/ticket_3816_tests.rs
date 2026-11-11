//! #3816 — the `unchecked_use_of_nullable_value` rows the audit attributed
//! to a `super_target` over-suppression, pinned per case with the oracle's
//! exact `(code, offset, length, message)` contract.
//!
//! The ticket's named mechanism — a `Receiver::Super`/"super is never
//! null" exemption leaking out of the unnecessary-null-aware decision —
//! does not exist on this path at HEAD. The only super exemption
//! (`coalesce_operand_never_null`, scopes_bodies.rs) is already confined
//! to the `??`/`??=` dead-operand verdict, exactly the end-state the
//! ticket prescribes, so these pins stand alone as the oracle contract:
//!
//! 1. `this[0]` / `this[0] = v` inside `extension E on A?` report
//!    `The method '[]' can't be unconditionally invoked` at the opening
//!    bracket with length 1 — the nullable implicit `this`'s `isNullable`
//!    tail precedes the member lookup (type_property_resolver.dart).
//!    Green at HEAD through `resolve_receiver_row`'s `Receiver::This` arm
//!    + `nullable_this_receiver`.
//! 2. `x.first()` over `List<T?>` (`T extends Function`) reports the
//!    FUNCTION variant on the whole callee `x.first` — the accessor's
//!    `CoreRet::ElemArg` face yields `T?`, whose invocation is the
//!    `FunctionExpressionInvocation` row. **Red at HEAD**: the rig's
//!    `List`/`Iterable` carry no `first` member and the frozen member
//!    verdict on a resolvable (non-`dart:`) container reads the index
//!    alone, so `member_defined`'s `Walk::Found` accessor-callee
//!    `Intent::Invoke` check (row_resolution.rs:12342) never runs and the
//!    class-family miss tail emits `undefined_method` instead — the same
//!    gap the SDK-stub `Missing`+`Defined` arm (:8469) opens for
//!    `dart:`-stub anchors. Pin kept red as the divergence latch; the fix
//!    lives in `check_instance_member`/`member_defined`
//!    (row_resolution.rs) — see the #3816 report.
//!
//! Oracle provenance: `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze
//! --format=machine` on the three minimal fixtures —
//! `UNCHECKED_USE_OF_NULLABLE_VALUE …|4|9|1|The method '[]' …`,
//! `…|2|3|7|The function can't be unconditionally invoked …`.

use crate::diagnose::FileReport;
use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops, rows};

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

const METHOD_INDEX_TEXT: &str =
    "The method '[]' can't be unconditionally invoked because the receiver can be 'null'.";
const FUNCTION_TEXT: &str =
    "The function can't be unconditionally invoked because it can be 'null'.";

/// `this[0]` inside `extension E on A?`: `The method '[]' can't be
/// unconditionally invoked` at `[` with length 1. Oracle:
/// `UNCHECKED_USE_OF_NULLABLE_VALUE|…|4|9|1|The method '[]' …`.
#[test]
fn this_index_read_reports_at_bracket() {
    let src = "class A { A operator [](int i) => this; }\n\
               extension E on A? {\n  void f() {\n    this[0];\n  }\n}\n";
    let report = report_ops(src);
    assert_unchecked(&report, at(src, "[0];"), 1, METHOD_INDEX_TEXT);
}

/// `this[0] = A()` inside `extension E on A?`: the same `'[]'` spelling at
/// `[` length 1 — the element lookup's `[]=` question is never the
/// dereference's text. Oracle: `…|4|9|1|The method '[]' …`.
#[test]
fn this_index_write_reports_at_bracket() {
    let src = "class A { A operator [](int i) => this; void operator []=(int i, A v) {} }\n\
               extension E on A? {\n  void f() {\n    this[0] = A();\n  }\n}\n";
    let report = report_ops(src);
    assert_unchecked(&report, at(src, "[0] ="), 1, METHOD_INDEX_TEXT);
}

/// `x.first()` over `List<T?>` (`T extends Function`): the resolved member
/// is the `first` getter whose `CoreRet::ElemArg` face yields `T?`; its
/// invocation reports the FUNCTION variant anchored on the whole callee
/// `x.first`. Oracle: `…|2|3|7|The function can't be unconditionally
/// invoked …`.
#[test]
fn member_callee_function_nullable() {
    let src = "m<T extends Function>(List<T?> x) {\n  x.first();\n}\n";
    let report = report_ops(src);
    assert_unchecked(&report, at(src, "x.first"), 7, FUNCTION_TEXT);
}
