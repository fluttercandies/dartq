//! M0-gate wave-4 regression pins (#604): the formal-parameter group
//! recovery keeps the *list* open instead of bailing to declaration-level
//! recovery. The official decision order (`parser_impl.dart`, Dart 3.15
//! main):
//!
//! * Inside a `[`/`{` group, the loop parses every iteration through
//!   `parseFormalParameter` with no group-boundary check between parameters
//!   (:2496-2508 / :2452-2464), so a stray token becomes a junk parameter
//!   (its name step goes through the formalParameterDeclaration template,
//!   `identifier_context_impl.dart:571-612`: grammar followers
//!   `: = , ( ) [ ] { } EOF` insert a synthetic identifier WITHOUT
//!   consuming the junk and report `missing_identifier`).
//! * The group loop then finds a non-comma non-closer, reports ONE
//!   `expectedButGot('}')`/`(']')` at the stray token (:2509-2517 /
//!   :2465-2473) and walks to the token just before the group's own
//!   scanner-guaranteed closer — never abandoning the list.
//! * Back in `parseFormalParametersRest`, the group result goes through
//!   `ensureCloseParen(token, begin)` (:1951/:1956/:1962): a `)` directly
//!   after the group is consumed silently (:5031-5033); otherwise the
//!   report anchors on the token after the group, and the jump target is
//!   the LIST `(`'s `endGroup` (:5035-5052) — any junk between (including
//!   a whole subsequent group) is skipped silently and the body parses.
//! * A function-typed formal facing the stray token keeps the name step
//!   silent: `computeType` returns `voidType` for `void cb (`
//!   (`type_info.dart:220-231`), so the tentative-name walk (`cb`) makes
//!   the inline shape and the name step's candidate is `cb` — a plain
//!   identifier, consumed without a report
//!   (`identifier_context_impl.dart:579-583`).

use super::collector::setup;

/// Parses `source` and asserts the exact in-order (code, start, end)
/// stream (UTF-16 offsets), then hands the listener to `check` for the
/// structural event/message-template assertions.
fn assert_stream(
    source: &'static str,
    expected: &[(&str, u32, u32)],
    label: &str,
) -> super::collector::Collector {
    let mut p = setup(source);
    p.parse_unit();
    let got: Vec<(String, u32, u32)> = p.listener.errors.clone();
    let want: Vec<(String, u32, u32)> = expected
        .iter()
        .map(|(c, s, e)| (ToString::to_string(c), *s, *e))
        .collect();
    assert_eq!(got, want, "{label}: diagnostic stream diverged");
    std::mem::take(p.listener)
}

/// Asserts the exact diagnostic-code multiset of one whole-unit parse.
fn assert_codes(source: &'static str, expected: &[&str], label: &str) {
    let mut p = setup(source);
    p.parse_unit();
    let mut got = p.listener.error_codes();
    got.sort();
    let mut want: Vec<String> = expected.iter().map(ToString::to_string).collect();
    want.sort();
    assert_eq!(got, want, "{label}: diagnostic multiset diverged");
}

/// Wave fixture (#604): the `{int b}` group closes cleanly, so the stray
/// token the list sees is the `,` AFTER the group — `ensureCloseParen`
/// (:5028-5051) anchors ONE `expected_token` row on it and jumps to the
/// list `(`'s endGroup, skipping the second group `[int c]` silently (one
/// optional group per list — `parseFormalParametersRest` breaks the loop
/// after the first group, :1949-1957) — and the body parses clean. The
/// official stream is that single list-local row; the pre-fix port
/// resolved the group closer instead of the list `(`, synthesized a `)`
/// at the `,`, and cascaded `expected_executable` x20 +
/// `missing_function_body` x5 into declaration recovery.
///
/// Offsets: `}`@19, `,`@20, `[`@21, `)`@28.
#[test]
fn named_group_error_stays_in_list() {
    let listener = assert_stream(
        "void f(int a,{int b},[int c]) {}",
        &[("expected_token", 20, 21)], // Expected to find ')'. at the `,`
        "named group error stays in-list",
    );
    assert_eq!(
        listener.texts,
        ["Expected to find ')'.",],
        "message templates diverged"
    );
    listener.assert_sequence(&[
        ("begin", "formal_parameters"),
        ("begin", "optional_formal_parameters"),
        ("end", "optional_formal_parameters"),
        ("end", "formal_parameters"),
        ("begin", "block_function_body"),
        ("end", "block_function_body"),
    ]);
}

/// A stray `]` after a well-formed `{...}` group at the very end of the
/// list: one `expected_token` anchored on the stray, then the jump to the
/// list `)` (:5049-5051) — the closer `)` itself is well-formed, so the
/// body follows clean. The pre-fix port synthesized the `)` at the stray
/// and orphaned `])` into declaration recovery.
#[test]
fn group_error_at_list_end_stays_in_list() {
    let listener = assert_stream(
        "void f(int a, {int b}]) {}",
        &[("expected_token", 21, 22)],
        "stray closer at list end",
    );
    listener.assert_sequence(&[
        ("end", "optional_formal_parameters"),
        ("end", "formal_parameters"),
        ("begin", "block_function_body"),
        ("end", "block_function_body"),
    ]);
}

/// A nested function-typed parameter inside a broken group parses its own
/// parameter list normally, and the name step never sees the stray: for
/// `void cb (` official's `computeType` returns `voidType`
/// (`type_info.dart:220-231`), `skipType` stops at the return type, the
/// tentative-name walk consumes `cb` and its `(` follower makes the inline
/// shape (:2183-2225), and the name step's candidate is `cb` itself — a
/// plain identifier, the silent happy path
/// (`identifier_context_impl.dart:579-583`). The stray `]` is therefore
/// first seen by the group loop (:2509-2517): ONE `expectedButGot('}')`
/// row, then the walk to the group's real `}` and the silent
/// `ensureCloseParen` fast path at the list `)` (:5031-5033).
#[test]
fn function_typed_parameter_inside_broken_group() {
    let listener = assert_stream(
        "void f({int b, void cb(int x)]}) {}",
        &[("expected_token", 29, 30)], // Expected to find '}' — at the `]`
        "function-typed formal inside a broken group",
    );
    listener.assert_sequence(&[
        ("begin", "optional_formal_parameters"),
        ("begin", "function_typed_formal"),
        ("begin", "formal_parameters"),
        ("end", "formal_parameters"),
        ("end", "function_typed_formal"),
        ("end", "optional_formal_parameters"),
        ("end", "formal_parameters"),
        ("begin", "block_function_body"),
        ("end", "block_function_body"),
    ]);
}

/// An empty-remainder group after an in-group error (`{,` / `[,`): the
/// leading `,` takes the template keep-set row (`missing_identifier`,
/// insert-without-consuming), the loop consumes the comma and breaks at
/// the loop-head closer check (:2499-2501); `parameterCount == 1` means
/// the empty-list report (:2534-2545 / :2456-2467) does not fire.
#[test]
fn empty_remainder_after_in_group_error() {
    assert_stream(
        "void f(int a, {,}) {}",
        &[("missing_identifier", 15, 16)],
        "named group empty remainder",
    );
    assert_stream(
        "void f(int a, [,]) {}",
        &[("missing_identifier", 15, 16)],
        "positional group empty remainder",
    );
}

/// Well-formed single optional groups stay green (the fix only moves the
/// jump target after a group; a `)` directly after a group is consumed by
/// the `ensureCloseParen` fast path :5031-5033).
#[test]
fn wellformed_single_groups_stay_green() {
    assert_codes("void f(int a, [int b]) {}", &[], "positional group");
    assert_codes("void f(int a, {int b}) {}", &[], "named group");
    assert_codes("void f({int a, int b}) {}", &[], "named-only group");
}

/// Dart allows one optional group per list: after the first group closes,
/// `parseFormalParametersRest` breaks out of the loop (:1949-1957) and the
/// second group is junk skipped silently by the `ensureCloseParen` endGroup
/// jump — one `expected_token` anchored on the comma, body clean.
#[test]
fn second_group_after_closed_group_is_jumped_over() {
    let listener = assert_stream(
        "void f(int a, [int b], {int c}) {}",
        &[("expected_token", 21, 22)],
        "second group jumped over",
    );
    listener.assert_sequence(&[
        ("end", "optional_formal_parameters"),
        ("end", "formal_parameters"),
        ("begin", "block_function_body"),
        ("end", "block_function_body"),
    ]);
}
