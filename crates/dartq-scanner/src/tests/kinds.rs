//! Token kinds, full keyword set, operator golden table, group matching, script tag, gating.

use super::{assert_codes, assert_kinds, config_off, kinds_of, scan_src};
use crate::{Keyword, KeywordStyle, TokenKind as K, keyword_from_str};

#[test]
fn empty_source_yields_only_eof() {
    let r = scan_src("");
    assert_eq!(r.tokens.len(), 1);
    assert_eq!(r.tokens.first().kind, K::Eof);
    assert!(r.diagnostics.is_empty());
    assert!(r.language_version_override.is_none());
}

#[test]
fn whitespace_only_source() {
    let r = scan_src(" \t\n\r\n  ");
    assert_eq!(r.tokens.len(), 1);
    assert!(r.diagnostics.is_empty());
}

#[test]
fn identifiers_and_dollar() {
    assert_kinds(&scan_src("foo _bar $baz x1 y_2$"), &[K::Identifier; 5]);
}

#[test]
fn all_keywords_golden() {
    // The official full set of 73 keywords; each scanned individually, verifying the lexeme ↔ kind mapping in both directions.
    const ALL: &[(&str, Keyword)] = &[
        ("abstract", Keyword::Abstract),
        ("as", Keyword::As),
        ("assert", Keyword::Assert),
        ("async", Keyword::Async),
        ("augment", Keyword::Augment),
        ("await", Keyword::Await),
        ("base", Keyword::Base),
        ("break", Keyword::Break),
        ("case", Keyword::Case),
        ("catch", Keyword::Catch),
        ("class", Keyword::Class),
        ("const", Keyword::Const),
        ("continue", Keyword::Continue),
        ("covariant", Keyword::Covariant),
        ("default", Keyword::Default),
        ("deferred", Keyword::Deferred),
        ("do", Keyword::Do),
        ("dynamic", Keyword::Dynamic),
        ("else", Keyword::Else),
        ("enum", Keyword::Enum),
        ("export", Keyword::Export),
        ("extends", Keyword::Extends),
        ("extension", Keyword::Extension),
        ("external", Keyword::External),
        ("factory", Keyword::Factory),
        ("false", Keyword::False),
        ("final", Keyword::Final),
        ("finally", Keyword::Finally),
        ("for", Keyword::For),
        ("Function", Keyword::Function),
        ("get", Keyword::Get),
        ("hide", Keyword::Hide),
        ("if", Keyword::If),
        ("implements", Keyword::Implements),
        ("import", Keyword::Import),
        ("in", Keyword::In),
        ("inout", Keyword::Inout),
        ("interface", Keyword::Interface),
        ("is", Keyword::Is),
        ("late", Keyword::Late),
        ("library", Keyword::Library),
        ("mixin", Keyword::Mixin),
        ("native", Keyword::Native),
        ("new", Keyword::New),
        ("null", Keyword::Null),
        ("of", Keyword::Of),
        ("on", Keyword::On),
        ("operator", Keyword::Operator),
        ("out", Keyword::Out),
        ("part", Keyword::Part),
        ("patch", Keyword::Patch),
        ("required", Keyword::Required),
        ("rethrow", Keyword::Rethrow),
        ("return", Keyword::Return),
        ("sealed", Keyword::Sealed),
        ("set", Keyword::Set),
        ("show", Keyword::Show),
        ("source", Keyword::Source),
        ("static", Keyword::Static),
        ("super", Keyword::Super),
        ("switch", Keyword::Switch),
        ("sync", Keyword::Sync),
        ("this", Keyword::This),
        ("throw", Keyword::Throw),
        ("true", Keyword::True),
        ("try", Keyword::Try),
        ("typedef", Keyword::Typedef),
        ("var", Keyword::Var),
        ("void", Keyword::Void),
        ("when", Keyword::When),
        ("while", Keyword::While),
        ("with", Keyword::With),
        ("yield", Keyword::Yield),
    ];
    assert_eq!(ALL.len(), 73);
    for (lexeme, kw) in ALL {
        assert_eq!(keyword_from_str(lexeme), Some(*kw), "lexeme {lexeme}");
        assert_eq!(kw.as_str(), *lexeme);
        let r = scan_src(lexeme);
        assert_kinds(&r, &[K::Keyword(*kw)]);
        assert!(r.diagnostics.is_empty(), "{lexeme}");
    }
    // Case-sensitive: `Function` is a keyword; `function`/`IF` are not.
    assert_eq!(keyword_from_str("function"), None);
    assert_eq!(keyword_from_str("IF"), None);
    assert_kinds(&scan_src("function IF If"), &[K::Identifier; 3]);
}

#[test]
fn keyword_styles_official() {
    assert_eq!(Keyword::Assert.style(), KeywordStyle::Reserved);
    assert_eq!(Keyword::Class.style(), KeywordStyle::Reserved);
    assert_eq!(Keyword::Abstract.style(), KeywordStyle::BuiltIn);
    assert_eq!(Keyword::Augment.style(), KeywordStyle::BuiltIn);
    assert_eq!(Keyword::Async.style(), KeywordStyle::Pseudo);
    assert_eq!(Keyword::Yield.style(), KeywordStyle::Pseudo);
    assert_eq!(Keyword::Show.style(), KeywordStyle::Pseudo);
}

#[test]
fn augment_gated_by_config() {
    // On: Keyword; off: Identifier (official tokenizeKeywordOrIdentifier :2106).
    let on = scan_src("augment");
    assert_kinds(&on, &[K::Keyword(Keyword::Augment)]);
    let off = crate::scan("augment", config_off(), super::file_id());
    assert_kinds(&off, &[K::Identifier]);
    // Not treated as a keyword when followed by identifier characters (`augments`, `augment_2`).
    assert_kinds(&scan_src("augments augment_2"), &[K::Identifier; 2]);
}

#[test]
fn operators_golden_table() {
    // All fixed-lexeme operators/punctuation (the official TokenType full set; single characters space-separated to avoid merging).
    const TABLE: &[(&str, K)] = &[
        ("&", K::Ampersand),
        ("&&", K::AmpersandAmpersand),
        ("&=", K::AmpersandEq),
        ("@", K::At),
        ("!", K::Bang),
        ("!=", K::BangEq),
        ("|", K::Bar),
        ("||", K::BarBar),
        ("|=", K::BarEq),
        ("`", K::Backping),
        ("\\", K::Backslash),
        ("^", K::Caret),
        ("^=", K::CaretEq),
        (":", K::Colon),
        (",", K::Comma),
        ("}", K::CloseCurly),
        (")", K::CloseParen),
        ("]", K::CloseSquare),
        ("=", K::Eq),
        ("==", K::EqEq),
        ("=>", K::Function),
        (">", K::Gt),
        (">=", K::GtEq),
        (">>", K::GtGt),
        (">>=", K::GtGtEq),
        (">>>", K::GtGtGt),
        (">>>=", K::GtGtGtEq),
        ("#", K::Hash),
        ("[]", K::Index),
        ("[]=", K::IndexEq),
        ("{", K::OpenCurly),
        ("(", K::OpenParen),
        ("[", K::OpenSquare),
        ("<", K::Lt),
        ("<=", K::LtEq),
        ("<<", K::LtLt),
        ("<<=", K::LtLtEq),
        ("-", K::Minus),
        ("-=", K::MinusEq),
        ("--", K::MinusMinus),
        ("%", K::Percent),
        ("%=", K::PercentEq),
        (".", K::Period),
        ("..", K::PeriodPeriod),
        ("...", K::PeriodPeriodPeriod),
        ("...?", K::PeriodPeriodPeriodQuestion),
        ("+", K::Plus),
        ("+=", K::PlusEq),
        ("++", K::PlusPlus),
        ("?", K::Question),
        ("?.", K::QuestionPeriod),
        ("?..", K::QuestionPeriodPeriod),
        ("??", K::QuestionQuestion),
        ("??=", K::QuestionQuestionEq),
        (";", K::Semicolon),
        ("/", K::Slash),
        ("/=", K::SlashEq),
        ("*", K::Star),
        ("*=", K::StarEq),
        ("~", K::Tilde),
        ("~/", K::TildeSlash),
        ("~/=", K::TildeSlashEq),
    ];
    for (lexeme, kind) in TABLE {
        let r = scan_src(lexeme);
        let got: Vec<K> = r.tokens.as_slice().iter().map(|t| t.kind).collect();
        // When a begin-group lexeme stands alone, EOF recovery synthesizes the
        // matching close token (official unmatchedBeginGroup); `<` is the
        // exception: EOF first silently discards it via discardOpenLt.
        let mut expected = vec![*kind];
        if kind.is_begin_group() && *kind != K::Lt {
            expected.push(kind.matching_close().unwrap());
        }
        assert_eq!(
            &got[..got.len() - 1],
            &expected[..],
            "lexeme {lexeme:?}; diags={:?}",
            super::codes(&r)
        );
        assert_eq!(r.tokens.first().byte_len as usize, lexeme.len());
    }
}

#[test]
fn lazy_assignment_ops_are_two_tokens() {
    // `&&=`/`||=` never form a single token: LAZY_ASSIGNMENT_ENABLED is a hard
    // `false` constant (abstract_scanner.dart:53), so the official
    // AMPERSAND_AMPERSAND_EQ / BAR_BAR_EQ branches (:1354/:1375) are dead — the
    // `=` is rescanned by the main loop as a plain EQ.
    let r = scan_src("a &&= b");
    assert_kinds(
        &r,
        &[K::Identifier, K::AmpersandAmpersand, K::Eq, K::Identifier],
    );
    assert!(r.diagnostics.is_empty());
    let r = scan_src("a ||= b");
    assert_kinds(&r, &[K::Identifier, K::BarBar, K::Eq, K::Identifier]);
    assert!(r.diagnostics.is_empty());
}

#[test]
fn bang_period_is_two_tokens() {
    // The official scanner has no `!.` single token: BANG + PERIOD.
    assert_kinds(
        &scan_src("a!.b"),
        &[K::Identifier, K::Bang, K::Period, K::Identifier],
    );
}

#[test]
fn triple_shift_gating() {
    // On: `>>>` is a single token, `>>>=` a single token.
    assert_kinds(
        &scan_src("a >>> b"),
        &[K::Identifier, K::GtGtGt, K::Identifier],
    );
    assert_kinds(
        &scan_src("a >>>= b"),
        &[K::Identifier, K::GtGtGtEq, K::Identifier],
    );
    // Off: `>>>` = GtGt + Gt; `>>>=` = GtGt + GtEq (official tokenizeGreaterThan).
    let r = crate::scan("a >>> b", config_off(), super::file_id());
    assert_kinds(&r, &[K::Identifier, K::GtGt, K::Gt, K::Identifier]);
    let r = crate::scan("a >>>= b", config_off(), super::file_id());
    assert_kinds(&r, &[K::Identifier, K::GtGt, K::GtEq, K::Identifier]);
    // `>>` is a single token under both configs (the official scanner never splits it).
    for off in [true, false] {
        assert_eq!(
            kinds_of("a >> b", off),
            vec![K::Identifier, K::GtGt, K::Identifier]
        );
    }
}

#[test]
fn double_triple_shift_runs() {
    // `>>>>`: gate on = `>>>` + `>`; gate off = `>>` + `>>` (longest-match semantics).
    assert_kinds(
        &scan_src("x >>>> y"),
        &[K::Identifier, K::GtGtGt, K::Gt, K::Identifier],
    );
    assert_kinds(
        &scan_src("x >>>>> y"),
        &[K::Identifier, K::GtGtGt, K::GtGt, K::Identifier],
    );
    assert_eq!(
        kinds_of("x >>>> y", false),
        vec![K::Identifier, K::GtGtGt, K::Gt, K::Identifier]
    );
    assert_eq!(
        kinds_of("x >>>> y", true),
        vec![K::Identifier, K::GtGt, K::GtGt, K::Identifier]
    );
}

#[test]
fn unsupported_operators() {
    // `===`/`!==`: token emitted as-is + unsupported_operator (official UnsupportedOperator);
    // the message is the registry template with the lexeme filled in.
    let r = scan_src("a === b");
    assert_kinds(&r, &[K::Identifier, K::EqEqEq, K::Identifier]);
    assert_codes(&r, &["unsupported_operator"]);
    assert_eq!(
        r.diagnostics[0].message,
        "The '===' operator is not supported."
    );
    let r = scan_src("a !== b");
    assert_kinds(&r, &[K::Identifier, K::BangEqEq, K::Identifier]);
    assert_codes(&r, &["unsupported_operator"]);
    assert_eq!(
        r.diagnostics[0].message,
        "The '!==' operator is not supported."
    );
}

#[test]
fn group_matching_indices() {
    // f(a, (b)): begin/end paired indices are correct.
    let r = scan_src("f(a, (b))");
    let t = &r.tokens;
    // indices: 0 f, 1 (, 2 a, 3 ,, 4 (, 5 b, 6 ), 7 ), 8 Eof
    assert_eq!(t.get(1).unwrap().kind, K::OpenParen);
    assert_eq!(t.get(4).unwrap().kind, K::OpenParen);
    assert_eq!(t.matching_end(1), Some(7));
    assert_eq!(t.matching_end(4), Some(6));
    assert_eq!(t.matching_end(0), None);
    // Chained navigation.
    assert_eq!(t.first().kind, K::Identifier);
    assert_eq!(t.last().kind, K::Eof);
    assert_eq!(t.next(7), Some(8));
    assert_eq!(t.next(8), None);
    // Lexeme slices.
    assert_eq!(t.lexeme("f(a, (b))", 0), "f");
    assert_eq!(t.lexeme("f(a, (b))", 2), "a");
}

#[test]
fn script_tag_first_line_only() {
    let r = scan_src("#!/usr/bin/env dart\nvoid main() {}");
    assert_kinds(
        &r,
        &[
            K::ScriptTag,
            K::Keyword(Keyword::Void),
            K::Identifier,
            K::OpenParen,
            K::CloseParen,
            K::OpenCurly,
            K::CloseCurly,
        ],
    );
    assert_eq!(r.tokens.first().byte_range(), 0..19);
    // `#!` at a non-zero offset is HASH + BANG (official scanOffset==0 check).
    let r = scan_src("\n#!/bin/dart");
    assert_kinds(
        &r,
        &[
            K::Hash,
            K::Bang,
            K::Slash,
            K::Identifier,
            K::Slash,
            K::Identifier,
        ],
    );
}

#[test]
fn eof_offsets() {
    let src = "var x;";
    let r = scan_src(src);
    let eof = r.tokens.last();
    assert_eq!(eof.kind, K::Eof);
    assert_eq!(eof.byte_offset as usize, src.len());
    assert_eq!(eof.byte_len, 0);
    assert_eq!(eof.utf16_offset as usize, src.len());
}
