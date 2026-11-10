//! #2991 exact-row pins: function-typed initializing formals with type
//! parameters — `this.f<U>(args)` / `super.a<T>(args)`.
//!
//! Root: the inline-function-typed probe (upstream `parseFormalParameter`
//! :2197-2240) runs after the `this`/`super` walk on the SAME tentative-name
//! position for receivers and non-receivers alike — the walk consumes the
//! receiver head, leaving the cursor on `f`, so `computeTypeParamOrArg`
//! (:2204) classifies `f<U>(` and the `closer.next!.isA(OPEN_PAREN)` gate
//! (:2208) accepts the shape. dartq gated the probe on
//! `this_keyword.is_none()`, so `this` receivers never probed (the `<` fell
//! to the parameter tail → `expected_token ')'` cascades) and `super`
//! receivers probed but never stepped the receiver head on the re-walk
//! (`step_receiver_head`), derailing the suffix parse.
//!
//! Oracle: `dart analyze` 3.13.7 — every row below verified against the
//! oracle output on the same source.

use super::collector::{Collector, latest_features, scan_ok};
use crate::parser::Parser;

/// One expected oracle row, source-position based. `line`/`column` are the
/// 1-based TSV coordinates; `length` is the UTF-16 length (the pins are all
/// ASCII, so it matches the byte width).
type Row = (&'static str, u32, u32, u32, &'static str);

/// Parses `source` and returns the parser rows as
/// `(code, start, end, text)` sorted by position.
fn faces(source: &'static str) -> Vec<(String, u32, u32, String)> {
    let (table, text) = scan_ok(source);
    let mut parser = Parser::new(
        text,
        table,
        latest_features(),
        Box::leak(Box::new(Collector::default())),
    );
    parser.parse_unit();
    let mut rows: Vec<(String, u32, u32, String)> = parser
        .listener
        .errors
        .iter()
        .zip(parser.listener.texts.iter())
        .map(|((code, start, end), text)| (code.clone(), *start, *end, text.clone()))
        .collect();
    rows.sort_by(|a, b| (a.1, a.2, &a.0, &a.3).cmp(&(b.1, b.2, &b.0, &b.3)));
    rows
}

/// Converts 1-based `(line, column)` pins to offsets and compares the full
/// row set exactly.
fn assert_rows(source: &'static str, expected: &[Row], label: &str) {
    let line_starts: Vec<u32> = std::iter::once(0u32)
        .chain(
            source
                .bytes()
                .enumerate()
                .filter(|(_, b)| *b == b'\n')
                .map(|(i, _)| i as u32 + 1),
        )
        .collect();
    let mut want: Vec<(String, u32, u32, String)> = expected
        .iter()
        .map(|(code, line, col, len, text)| {
            let start = line_starts[(*line - 1) as usize] + col - 1;
            ((*code).to_owned(), start, start + len, (*text).to_owned())
        })
        .collect();
    want.sort_by(|a, b| (a.1, a.2, &a.0, &a.3).cmp(&(b.1, b.2, &b.0, &b.3)));
    let got = faces(source);
    assert_eq!(got, want, "{label}: oracle face set diverged");
}

/// The ticket's rep pair (upstream :2197-2240): typed receiver heads with a
/// `<...>(`-suffixed name parse as inline function-typed formals — zero
/// parser rows on both. Pre-fix, `this.f<U>` never probed (the `<` hit the
/// parameter tail → `expected_token ')'`) and `super.a<T>` probed but lost
/// the receiver head on the re-walk → `missing_identifier` +
/// `expected_token ')'` + `missing_function_parameters`.
#[test]
fn receiver_function_typed_formals_with_type_parameters() {
    assert_rows(
        "class A { A(U this.f<U>(U a)); }\nclass B { B(T super.a<T>(int b)); }\n",
        &[],
        "this.f<U>/super.a<T>",
    );
}

/// `(`-suffixed receiver faces (upstream OPEN_PAREN arm :2223-2240): both
/// `this` and `super` receivers parse clean — pre-fix `super.a(` matched
/// the probe but the re-walk missed the head → `expected_token ')'` +
/// `missing_function_parameters`.
#[test]
fn receiver_function_typed_formals_paren_suffix() {
    assert_rows(
        "class A { A(U this.f(int a)); }\nclass B { B(T super.a(int b)); }\n",
        &[],
        "this.f(/super.a(",
    );
}

/// Complex group + untyped faces: `<S,T>` classifies through the full-group
/// scan; an untyped receiver (`this.f<U>`) keeps the same shape — the
/// `NoType` reset preserves the probe position (zero parser rows).
#[test]
fn receiver_function_typed_formals_group_and_untyped() {
    assert_rows(
        "class C { C(this.f<U>(U a)); }\nclass D { D(super.a<T>(int b)); }\nclass H { H(int this.f<S,T>(S s, T t)); }\n",
        &[],
        "untyped/complex-group receivers",
    );
}

/// Receiver + missing `.` recovery (upstream :2164-2177): the walk reports
/// `expected_token '.'` once, the probe still classifies `f<U>(`, and the
/// re-walk steps the synthesized head — the oracle carries only the `.`
/// row (pre-fix added a spurious `expected_token ')'` at the `<`).
#[test]
fn receiver_function_typed_formal_missing_period() {
    assert_rows(
        "class F { F(U this f<U>(U a)); }\n",
        &[("expected_token", 1, 20, 1, "Expected to find '.'.")],
        "this f<U>( missing period",
    );
}

/// The name-less `(`-suffix face behind a receiver head (upstream
/// OPEN_PAREN arm + `ensureIdentifier(token, nameContext)` :2349): the `(` is
/// not an identifier, so the field-initializer ensure reports
/// `missing_identifier` at it (pre-fix consumed `(` as the name silently).
#[test]
fn receiver_function_typed_formal_nameless_paren() {
    assert_rows(
        "class E { E(this.(int x)); }\n",
        &[("missing_identifier", 1, 18, 1, "Expected an identifier.")],
        "this.(int x)",
    );
}

/// `var` + receiver + function-typed face: the shape-matched probe reports
/// `function_typed_parameter_var` on the modifier (upstream :2224-2236 —
/// receivers are not exempt), the walk-end gate adds `extraneous_modifier`,
/// `this.` outside a constructor reports
/// `field_initializer_outside_constructor`, and the `()` body-less function
/// reports `missing_function_body`. Row order per position ties mirrors the
/// upstream emission order.
#[test]
fn var_receiver_function_typed_formal() {
    assert_rows(
        "void g(var this.h());\n",
        &[
            (
                "extraneous_modifier",
                1,
                8,
                3,
                "Can't have modifier 'var' here.",
            ),
            (
                "function_typed_parameter_var",
                1,
                8,
                3,
                "Function-typed parameters can't specify 'const', 'final' or 'var' in place of a return type.",
            ),
            (
                "field_initializer_outside_constructor",
                1,
                12,
                4,
                "Field formal parameters can only be used in a constructor.",
            ),
            (
                "missing_function_body",
                1,
                21,
                1,
                "A function body must be provided.",
            ),
        ],
        "var this.h()",
    );
}

/// Regression guards: the non-receiver faces must stay untouched — plain
/// typed formals stay row-free, the non-receiver inline face still parses,
/// and a rejected group follower (`f<U>` with no `(`) keeps the ordinary
/// type+name reading with no rows.
#[test]
fn non_receiver_faces_unchanged() {
    assert_rows(
        "class C { C(int a, this.f, {String? b}); }\nvoid h(int Function(int) cb) {}\n",
        &[],
        "clean typed formals",
    );
}
