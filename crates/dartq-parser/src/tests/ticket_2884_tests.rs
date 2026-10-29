//! #2884 exact-row pins: class-level (`parseClassModifiers`) and
//! directive/declaration-level (`parse_declaration_modifiers` — the
//! enum/mixin/extension/typedef walk and the directive families'
//! base walk) duplicate-modifier recovery. Every duplicate is consumed and
//! reports exactly one `duplicated_modifier` row (upstream `_parseModifiers`
//! per-keyword handlers, modifier_context.dart:391-649); the first token
//! stays recorded, so the declaration keyword is reached and the family rows
//! anchor on the first occurrence — a duplicate never slips into the
//! name/declaration lane. The census has no class/directive-level duplicate
//! shape (report.tsv `duplicated_modifier` rows are all member/formal-level,
//! the #2880 faces), so these rows are pinned straight from the upstream
//! semantics.

use super::collector::{latest_features, scan_ok, Collector};
use crate::parser::Parser;

/// One expected oracle row, source-position based. `line`/`column` are the
/// 1-based TSV coordinates; `length` is the UTF-16 length (the pins are all
/// ASCII, so it matches the byte width).
type Row = (&'static str, u32, u32, u32, &'static str);

/// Parses `source` and returns the parser rows as
/// `(code, start, end, text)` sorted by position (gate_2791 harness shape).
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

/// Class-level duplicates (`parseClassModifiers`): every doubled run
/// consumes its second keyword with one `duplicated_modifier` row and the
/// class still parses on its own keyword with the recorded FIRST modifier
/// (`abstract abstract class C1` pins the fall-through fix — before it, the
/// second `abstract` took the `class` seat and mangled the whole header).
/// Record-only runs (abstract/augment/final, :406/:437/:543) carry the dup
/// row alone; face-bearing runs add the first-token row exactly once
/// (`externalClass` :682, `constClass` :679, the covariant/required/static
/// `extraneousModifier`s :131-135). `var`/`late`/`final final`/`const
/// const` first-token runs route to the member walk upstream (the
/// `parseTopLevelDeclarationImpl` special cases, parser_impl.dart:577-586),
/// so their class-level faces are only reachable behind a routing modifier
/// (lines 7-8).
#[test]
fn class_level_duplicate_modifier_rows() {
    assert_rows(
        "abstract abstract class C1 {}\naugment augment class C2 {}\nexternal external class C3 {}\nstatic static class C4 {}\ncovariant covariant class C5 {}\nrequired required class C6 {}\nabstract final final class C7 {}\nabstract const const class C8 {}\n",
        &[
            (
                "duplicated_modifier",
                1,
                10,
                8,
                "The modifier 'abstract' was already specified.",
            ),
            (
                "duplicated_modifier",
                2,
                9,
                7,
                "The modifier 'augment' was already specified.",
            ),
            (
                "external_class",
                3,
                1,
                8,
                "Classes can't be declared to be 'external'.",
            ),
            (
                "duplicated_modifier",
                3,
                10,
                8,
                "The modifier 'external' was already specified.",
            ),
            (
                "extraneous_modifier",
                4,
                1,
                6,
                "Can't have modifier 'static' here.",
            ),
            (
                "duplicated_modifier",
                4,
                8,
                6,
                "The modifier 'static' was already specified.",
            ),
            (
                "extraneous_modifier",
                5,
                1,
                9,
                "Can't have modifier 'covariant' here.",
            ),
            (
                "duplicated_modifier",
                5,
                11,
                9,
                "The modifier 'covariant' was already specified.",
            ),
            (
                "extraneous_modifier",
                6,
                1,
                8,
                "Can't have modifier 'required' here.",
            ),
            (
                "duplicated_modifier",
                6,
                10,
                8,
                "The modifier 'required' was already specified.",
            ),
            (
                "duplicated_modifier",
                7,
                16,
                5,
                "The modifier 'final' was already specified.",
            ),
            (
                "const_class",
                8,
                10,
                5,
                "Classes can't be declared to be 'const'.",
            ),
            (
                "duplicated_modifier",
                8,
                16,
                5,
                "The modifier 'const' was already specified.",
            ),
        ],
        "class-level duplicate modifier runs",
    );
}

/// Directive-level duplicates (`parse_declaration_modifiers` as the base of
/// `directives::parse_directive_modifiers`, upstream
/// `parseTopLevelKeywordModifiers` modifier_context.dart:183-196): the
/// second keyword reports `duplicated_modifier` in the walk (`_parseAugment`
/// :437, `_parseExternal` :529) and the recorded FIRST token carries the
/// walk's own extraneous row (`reportExtraneousModifier(augmentToken)` :188;
/// `reportTopLevelModifierError` :680-692 dispatches non-class/enum/typedef
/// followers to `extraneousModifier`). The imports still parse.
#[test]
fn directive_level_duplicate_modifier_rows() {
    assert_rows(
        "augment augment import 'a.dart';\nexternal external import 'b.dart';\n",
        &[
            (
                "duplicated_modifier",
                1,
                9,
                7,
                "The modifier 'augment' was already specified.",
            ),
            (
                "extraneous_modifier",
                1,
                1,
                7,
                "Can't have modifier 'augment' here.",
            ),
            (
                "duplicated_modifier",
                2,
                10,
                8,
                "The modifier 'external' was already specified.",
            ),
            (
                "extraneous_modifier",
                2,
                1,
                8,
                "Can't have modifier 'external' here.",
            ),
        ],
        "directive-level duplicate modifier runs",
    );
}

/// Regression pin: clean modifier runs stay clean — the legal
/// class-modifier combinations parse with zero rows, so the duplicate
/// recovery introduced no rider on the happy paths.
#[test]
fn clean_class_modifiers_stay_clean() {
    assert_rows(
        "abstract final class C {}\nfinal class D {}\nabstract class E {}\naugment class F {}\nbase mixin class G {}\nsealed class H {}\n",
        &[],
        "clean class modifier runs",
    );
}
