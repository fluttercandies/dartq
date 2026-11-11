//! #3714 pins — the function-invocation face's anchor normalization for
//! `unchecked_use_of_nullable_value`. Upstream reifies an invoked
//! function-typed getter/field result as a `FunctionExpressionInvocation`
//! whose `function` node the nullable check reports on
//! (`_rewriteAsFunctionExpressionInvocation`,
//! method_invocation_resolver.dart:1347-1459 +
//! function_expression_invocation_resolver.dart:68-71), so the anchor spans
//! receiver-through-selector — `a.child.cb()`, `a?.cb()`, `(a)!.cb()`,
//! `..cb()` — never the member token alone. The null-aware hop guards the
//! member access, not the invocation of its nullable result, so `a?.cb()`
//! and `a?..cb()` report too.

use super::test_rig::{CORE_PRELUDE, report_ops, rows};

/// `(utf16 offset, length, message)` of every unchecked-nullable row.
fn unchecked_spans(source: &str) -> Vec<(usize, usize, String)> {
    let report = report_ops(source);
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unchecked_use_of_nullable_value")
        .map(|d| {
            (
                d.range.offset as usize,
                d.range.length as usize,
                d.message.to_string(),
            )
        })
        .collect()
}

/// The fixture-local byte offset of `needle` (ASCII fixtures: UTF-16
/// offsets equal byte offsets; the rig's one-line prelude shifts the base).
fn at(source: &str, needle: &str) -> usize {
    CORE_PRELUDE.len() + source.find(needle).expect("needle present")
}

const FN_MESSAGE: &str = "The function can't be unconditionally invoked because it can be 'null'.";

/// `a.child.cb()` — the callee is the whole prefixed chain, not its final
/// selector pair (`child.cb`). Oracle 3.13.7 reports three rows on this
/// fixture: the receiver `a` (property 'child' access), the prefixed
/// chain `a.child` (property 'child' access), and the invocation `a.child.cb`
/// (function 'null' invoke). Pin the complete set — `(line, col)` 1-based
/// on the fixture's own lines, converted to 0-based byte offsets shifted
/// past [`CORE_PRELUDE`].
#[test]
fn chain_callee_anchors_whole_expression() {
    let src =
        "class A {\n  A? child;\n  void Function()? cb;\n}\nvoid f(A? a) {\n  a.child.cb();\n}\n";
    let property = "The property 'child' can't be unconditionally accessed because the receiver can be 'null'.";
    let method =
        "The method 'cb' can't be unconditionally invoked because the receiver can be 'null'.";
    assert_eq!(
        unchecked_spans(src),
        vec![
            // oracle (6,5): 'child' receiver access
            (at(src, "child.cb()"), "child".len(), property.to_string()),
            // oracle (6,3): the whole function chain
            (
                at(src, "a.child.cb()"),
                "a.child.cb".len(),
                FN_MESSAGE.to_string()
            ),
            // oracle (6,11): the final method 'cb' invocation
            (at(src, "cb()"), "cb".len(), method.to_string()),
        ],
        "{:#?}",
        rows(&report_ops(src))
    );
}

/// `a.child.child!.cb()` — the `!` postfix is part of the callee chain.
/// Oracle 3.13.7 reports three rows: the receiver `a`'s `child` property
/// access at `(6,5)`, the `child!.cb` chain's inner `child` access at
/// `(6,11)`, and the whole function chain `a.child.child!.cb` at `(6,3)`.
#[test]
fn bang_chain_callee_anchors_whole_expression() {
    let src = "class A {\n  A? child;\n  void Function()? cb;\n}\nvoid f(A? a) {\n  a.child.child!.cb();\n}\n";
    let property = "The property 'child' can't be unconditionally accessed because the receiver can be 'null'.";
    assert_eq!(
        unchecked_spans(src),
        vec![
            // oracle (6,5): the receiver's first 'child' access
            (
                at(src, "a.child") + "a.".len(),
                "child".len(),
                property.to_string()
            ),
            // oracle (6,11): the inner 'child' of 'child!.cb'
            (at(src, "child!"), "child".len(), property.to_string()),
            // oracle (6,3): the whole function chain 'a.child.child!.cb'
            (
                at(src, "a.child.child!.cb()"),
                "a.child.child!.cb".len(),
                FN_MESSAGE.to_string()
            ),
        ],
        "{:#?}",
        rows(&report_ops(src))
    );
}

/// `a!.cb()` — a postfix `!` receiver keeps the whole `a!.cb` extent
/// (control: the pre-change anchor already covered the bare-name case).
#[test]
fn bang_receiver_callee_anchors_postfix_chain() {
    let src = "class A {\n  void Function()? cb;\n}\nvoid f(A? a) {\n  a!.cb();\n}\n";
    assert_eq!(
        unchecked_spans(src),
        vec![(at(src, "a!.cb()"), "a!.cb".len(), FN_MESSAGE.to_string())],
        "{:#?}",
        rows(&report_ops(src))
    );
}

/// `(a)!.cb()` — the parenthesized bang receiver anchors at the group's
/// open paren. Oracle: one row at `(a)!.cb`.
#[test]
fn paren_bang_callee_anchors_group() {
    let src = "class A {\n  void Function()? cb;\n}\nvoid f(A? a) {\n  (a)!.cb();\n}\n";
    assert_eq!(
        unchecked_spans(src),
        vec![(
            at(src, "(a)!.cb()"),
            "(a)!.cb".len(),
            FN_MESSAGE.to_string()
        )],
        "{:#?}",
        rows(&report_ops(src))
    );
}

/// `a?.cb()` — the `?.` guards the member access, not the invocation of
/// its nullable result: the function face reports at the whole `a?.cb`.
#[test]
fn null_aware_getter_invoke_anchors_guarded_chain() {
    let src = "class A {\n  void Function()? cb;\n}\nvoid f(A? a) {\n  a?.cb();\n}\n";
    assert_eq!(
        unchecked_spans(src),
        vec![(at(src, "a?.cb()"), "a?.cb".len(), FN_MESSAGE.to_string())],
        "{:#?}",
        rows(&report_ops(src))
    );
}

/// `a?..cb()` — the null-aware cascade section's invocation reports with
/// the section anchor `?..cb` (the `?..` included, the target excluded).
/// Oracle 3.13.7 reports the row at `(5,4)` — the `?` of the `?..`
/// marker, not the `a` target.
#[test]
fn null_aware_cascade_invoke_anchors_section() {
    let src = "class A {\n  void Function()? cb;\n}\nvoid f(A? a) {\n  a?..cb();\n}\n";
    assert_eq!(
        unchecked_spans(src),
        vec![(
            at(src, "a?..cb()") + "a".len(),
            "?..cb".len(),
            FN_MESSAGE.to_string()
        )],
        "{:#?}",
        rows(&report_ops(src))
    );
}

/// `a..cb()` — the plain cascade section anchors at its own `..`, never at
/// the cascade target. Oracle 3.13.7 reports two rows: the section's
/// `..cb` at `(5,4)` and the `cb` method invocation at `(5,6)`.
#[test]
fn cascade_invoke_anchors_section() {
    let src = "class A {\n  void Function()? cb;\n}\nvoid f(A? a) {\n  a..cb();\n}\n";
    let method =
        "The method 'cb' can't be unconditionally invoked because the receiver can be 'null'.";
    assert_eq!(
        unchecked_spans(src),
        vec![
            // oracle (5,4): the '..cb' section span
            (
                at(src, "a..cb()") + "a".len(),
                "..cb".len(),
                FN_MESSAGE.to_string()
            ),
            // oracle (5,6): the 'cb' method call
            (at(src, "cb()"), "cb".len(), method.to_string()),
        ],
        "{:#?}",
        rows(&report_ops(src))
    );
}

/// `a.cb()` over a nullable receiver — the plain callee keeps its pair:
/// the function face at `a.cb` plus the method face at the name.
#[test]
fn plain_getter_invoke_keeps_pair() {
    let src = "class A {\n  void Function()? cb;\n}\nvoid f(A? a) {\n  a.cb();\n}\n";
    let method =
        "The method 'cb' can't be unconditionally invoked because the receiver can be 'null'.";
    assert_eq!(
        unchecked_spans(src),
        vec![
            (at(src, "a.cb()"), "a.cb".len(), FN_MESSAGE.to_string()),
            (at(src, "cb()"), "cb".len(), method.to_string()),
        ],
        "{:#?}",
        rows(&report_ops(src))
    );
}

/// `a.m()` — a real method send keeps the method face at the name and
/// never grows a function-spelling row (the walk-back is the
/// accessor-invocation face's, not the method send's). Oracle 3.13.7
/// reports the row at `(5,5)` — the `m` of the call site, not the `m`
/// of `void m() {}` (`src.find` would return the declaration first;
/// anchor the call's `a.m` instead).
#[test]
fn method_invoke_keeps_name_anchor() {
    let src = "class A {\n  void m() {}\n}\nvoid f(A? a) {\n  a.m();\n}\n";
    let method =
        "The method 'm' can't be unconditionally invoked because the receiver can be 'null'.";
    assert_eq!(
        unchecked_spans(src),
        vec![(at(src, "a.m()") + "a.".len(), "m".len(), method.to_string())],
        "{:#?}",
        rows(&report_ops(src))
    );
}

/// `a?.m()` — the null-aware method send stays guarded: no row.
#[test]
fn null_aware_method_invoke_stays_silent() {
    let src = "class A {\n  void m() {}\n}\nvoid f(A? a) {\n  a?.m();\n}\n";
    assert_eq!(
        unchecked_spans(src),
        Vec::new(),
        "{:#?}",
        rows(&report_ops(src))
    );
}

/// `a?.cb;` / `a?..cb;` — the null-aware property reads stay guarded: no
/// row (the function face is the invocation's, never the read's).
#[test]
fn null_aware_property_read_stays_silent() {
    let src = "class A {\n  void Function()? cb;\n}\nvoid f(A? a) {\n  a?.cb;\n  a?..cb;\n}\n";
    assert_eq!(
        unchecked_spans(src),
        Vec::new(),
        "{:#?}",
        rows(&report_ops(src))
    );
}

/// A non-nullable function-typed field invoked on a non-nullable receiver
/// resolves the send: no row.
#[test]
fn non_nullable_function_field_stays_silent() {
    let src = "class A {\n  void Function() cb;\n}\nvoid f(A a) {\n  a.cb();\n}\n";
    assert_eq!(
        unchecked_spans(src),
        Vec::new(),
        "{:#?}",
        rows(&report_ops(src))
    );
}
