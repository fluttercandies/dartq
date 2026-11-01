use dartq_ast::NodeKind;
use dartq_scanner::{Keyword, TokenKind};

use super::{Check, RuleSpec};

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/invalid_case_patterns.dart` — a legacy
/// `case` head whose expression is not a valid 3.0 constant-pattern
/// spelling reports at the head's unparenthesized extent (:50-88). The
/// arms: a non-`const` list or set/map literal (:52-59), a `dart:core`
/// `identical(...)` call (:60-63), a prefix expression whose operand is
/// not an integer literal (:64-67), any binary expression (:68-70), any
/// conditional expression (:71-73), a `dart:core` `length` property
/// access (:72-75), an `is` check (:76-77), an implicit-const instance
/// creation (:78-81), and the bare `_` identifier (:82-87).
///
/// Gate: the official registration bails WITH the feature (:41 — the
/// lint exists for pre-3.0 libraries only). The registration table keys
/// the same bit ([`crate::registry`] `REGISTRATION_GATES`), so the rule
/// never runs at a 3.0+ unit; the in-rule mirror below keeps the bail
/// next to the code that depends on it. The gate therefore lives in the
/// rule's own enablement — the one layer that sees the file's language
/// version. The *parse* cannot carry it: the lint bridge parses every
/// unit on the fixed Dart 3.15 surface ([`crate::bridge`]
/// `PARSE_FEATURES`), so a 2.19 head is parsed as a *pattern* and the
/// pattern grammar truncates the legacy expression surface (`case 1 + 2:`
/// folds to the constant pattern `1`, the `+ 2` tail recovered). A gate
/// derived from that shape would fire on modern code (whose heads are
/// patterns by construction) and silence the legacy files the rule
/// exists for.
///
/// The head is therefore classified from its TOKENS, never from the arena
/// rows: the run between `case` and the head's own `:` is exactly the
/// surface the official expression parse consumed, so every arm reads off
/// it; the report range is that run's extent (the official
/// `reportAtNode(expression)`). The whole switch block is walked once per
/// statement (depth-tracked, so a nested switch's heads stay out and its
/// own dispatch reports them), which keeps the face free of the case
/// row's shape — the materialized children and their expression count are
/// parse-posture artifacts, the token run is not.
///
/// Boundaries: the `identical`/`length` membership faces are name-level
/// (a user `identical`/`length` over-reports); the implicit-const
/// constructor-call arm (`C()` where the constructor is const, :78-81)
/// needs constructor resolution the rule context cannot reach and
/// under-reports. Message: `diagnostic.g.dart` `invalidCasePatterns`.
pub(crate) static SPEC: RuleSpec =
    crate::spec!("invalid_case_patterns", &[NodeKind::SwitchStatement], check,);

fn check(c: &mut Check<'_>) {
    // The registration bail (:41).
    if c.unit.features.patterns {
        return;
    }
    let Some(block) = c
        .collect_direct(c.node())
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::SwitchBlock)
    else {
        return;
    };
    let span = c.unit.ast.token_range(block);
    let mut depth = 0usize;
    let mut index = span.start;
    while index < span.end {
        let Some(kind) = c.unit.tokens.get(index as usize).map(|token| token.kind) else {
            break;
        };
        match kind {
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.saturating_sub(1);
            }
            // The block's own `{` sits at depth 1, so a head's `case`
            // keyword is the only one read at that depth — a nested
            // switch's block is one level deeper and dispatches itself.
            TokenKind::Keyword(Keyword::Case) if depth == 1 => {
                report_head(c, index, span.end);
            }
            _ => {}
        }
        index += 1;
    }
}

/// Reports `case` head `case_token` when it matches one of the official
/// arms.
fn report_head(c: &mut Check<'_>, case_token: u32, limit: u32) {
    let head = case_head(c, case_token, limit);
    let head = unparenthesized(c, &head);
    if head.is_empty() || !is_invalid(c, head) {
        return;
    }
    let (Some(start), Some(end)) = (
        c.utf16_of_token(head[0]),
        c.utf16_end_of_token(head[head.len() - 1]),
    ) else {
        return;
    };
    c.report(start..end, &[]);
}

/// The head's token run: every token between the `case` keyword and the
/// head's own `:`, bounded by the switch block's end.
///
/// The run is read off the token table (source order), never the arena:
/// the parse recovers the legacy tail as junk (`expect_kind` splices a
/// synthetic colon AT the offending token and consumes it), so the case
/// row's extent stops short of the surface the official rule inspects. A
/// depth-0 `:` ends the head, a ternary's colon is consumed by its
/// pending `?`, and a `when` clause (`when` is a 3.0 suffix, outside this
/// rule's gate) ends it too.
fn case_head(c: &Check<'_>, case_token: u32, limit: u32) -> Vec<u32> {
    let tokens = c.unit.tokens;
    let mut run = Vec::new();
    let mut depth = 0usize;
    let mut ternary = 0usize;
    let mut index = case_token + 1;
    while index < limit {
        let Some(token) = tokens.get(index as usize) else {
            break;
        };
        let kind = token.kind;
        if depth == 0 {
            match kind {
                TokenKind::Eof
                | TokenKind::CloseCurly
                | TokenKind::Keyword(Keyword::Case | Keyword::Default | Keyword::When) => break,
                TokenKind::Colon if ternary == 0 => break,
                TokenKind::Question => ternary += 1,
                TokenKind::Colon => ternary -= 1,
                _ => {}
            }
        }
        match kind {
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
        if !token.is_synthetic() {
            run.push(index);
        }
        index += 1;
    }
    run
}

/// The official `unParenthesized` (:51): enclosing parenthesized groups
/// peel, so the report stays inside the parens.
fn unparenthesized<'r>(c: &Check<'_>, mut run: &'r [u32]) -> &'r [u32] {
    while run.len() >= 2 && kind(c, run[0]) == TokenKind::OpenParen {
        let Some(close) = group_close(c, run) else {
            break;
        };
        if close + 1 != run.len() {
            break;
        }
        run = &run[1..close];
    }
    run
}

/// The run index where the group opened at `run[0]` closes.
fn group_close(c: &Check<'_>, run: &[u32]) -> Option<usize> {
    let mut depth = 0usize;
    for (index, &token) in run.iter().enumerate() {
        match kind(c, token) {
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether a head run matches one of the official reporting arms.
fn is_invalid(c: &Check<'_>, run: &[u32]) -> bool {
    // The explicit-`const` spellings are never invalid: the literals'
    // `constKeyword == null` guard (:53-57) and the instance-creation
    // `keyword?.type != CONST` guard (:79-81).
    if kind(c, run[0]) == TokenKind::Keyword(Keyword::Const) {
        return false;
    }
    if is_non_const_literal(c, run) {
        return true;
    }
    if let Some(operand) = prefix_operand(c, run) {
        // `-1` is the one exempt prefix spelling (:64-67): its operand is
        // an integer literal.
        return !(operand.len() == 1
            && matches!(kind(c, operand[0]), TokenKind::Int | TokenKind::Hexadecimal));
    }
    if is_identical_call(c, run) {
        return true;
    }
    // The bare `_` identifier (:82-87).
    if run.len() == 1 && kind(c, run[0]) == TokenKind::Identifier && c.lexeme(run[0]) == "_" {
        return true;
    }
    has_invalid_top_level_operator(c, run)
}

/// The non-`const` list / set-or-map literal arm (:52-59).
fn is_non_const_literal(c: &Check<'_>, run: &[u32]) -> bool {
    let Some(start) = literal_start(c, run) else {
        return false;
    };
    match kind(c, run[start]) {
        // `[]` scans as one self-closed token.
        TokenKind::Index => start + 1 == run.len(),
        _ => group_close(c, &run[start..]) == Some(run.len() - start - 1),
    }
}

/// The index of the literal's opening bracket, past optional type
/// arguments (`<int>[1, 2]` — the official literal's extent covers them,
/// so the report does too).
fn literal_start(c: &Check<'_>, run: &[u32]) -> Option<usize> {
    let start = if kind(c, run[0]) == TokenKind::Lt {
        type_arguments_end(c, run, 0)? + 1
    } else {
        0
    };
    matches!(
        kind(c, *run.get(start)?),
        TokenKind::OpenSquare | TokenKind::OpenCurly | TokenKind::Index
    )
    .then_some(start)
}

/// The run index of the closer for the type-argument group opened at
/// `open` (`<<`/`>>`/`>>>` count as two/three, so a nested
/// `<List<int>>` closes at the combined token).
fn type_arguments_end(c: &Check<'_>, run: &[u32], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (index, &token) in run.iter().enumerate().skip(open) {
        match kind(c, token) {
            TokenKind::Lt => depth += 1,
            TokenKind::LtLt => depth += 2,
            TokenKind::Gt => depth -= 1,
            TokenKind::GtGt => depth -= 2,
            TokenKind::GtGtGt => depth -= 3,
            _ => {}
        }
        if depth <= 0 && index > open {
            return Some(index);
        }
    }
    None
}

/// The operand run of a leading prefix operator (`-`/`!`/`~`/`++`/`--`).
fn prefix_operand<'r>(c: &Check<'_>, run: &'r [u32]) -> Option<&'r [u32]> {
    if run.len() < 2 {
        return None;
    }
    match kind(c, run[0]) {
        TokenKind::Minus
        | TokenKind::Bang
        | TokenKind::Tilde
        | TokenKind::PlusPlus
        | TokenKind::MinusMinus => Some(&run[1..]),
        _ => None,
    }
}

/// The `dart:core` `identical(...)` call arm (:60-63) — the name-level
/// face over the call's spelling.
fn is_identical_call(c: &Check<'_>, run: &[u32]) -> bool {
    if run.len() < 3
        || kind(c, run[0]) != TokenKind::Identifier
        || kind(c, run[1]) != TokenKind::OpenParen
        || c.lexeme(run[0]) != "identical"
    {
        return false;
    }
    group_close(c, &run[1..]) == Some(run.len() - 2)
}

/// The expression arms read off the head's top level: `is` (:76-77), the
/// binary operators (:68-70), the conditional `?` (:71-73), and the
/// trailing `.length` property access (:72-75).
fn has_invalid_top_level_operator(c: &Check<'_>, run: &[u32]) -> bool {
    // `run[0]` is the operand head; a leading operator is the prefix arm.
    let mut index = 1;
    while index < run.len() {
        let token = run[index];
        // A type-argument group ahead of a call or literal is part of that
        // expression, not a comparison (`f<int>(1)` is a method invocation
        // the official rule leaves alone).
        if kind(c, token) == TokenKind::Lt
            && let Some(close) = type_arguments_end(c, run, index)
            && let Some(&after) = run.get(close + 1)
            && matches!(
                kind(c, after),
                TokenKind::OpenParen
                    | TokenKind::OpenSquare
                    | TokenKind::OpenCurly
                    | TokenKind::Index
            )
        {
            index = close + 1;
            continue;
        }
        // Nested groups keep their operators off the head's top level.
        if matches!(
            kind(c, token),
            TokenKind::OpenParen
                | TokenKind::OpenSquare
                | TokenKind::OpenCurly
                | TokenKind::StringInterpolationExpression
        ) {
            index = group_close(c, &run[index..]).map_or(run.len(), |close| index + close + 1);
            continue;
        }
        match kind(c, token) {
            TokenKind::Keyword(Keyword::Is) => return true,
            // A bare `?` at the head's top level is the conditional
            // operator (`?.`/`??`/`?..` scan as their own tokens, and the
            // legacy expression parse reads a lone `?` as the ternary).
            TokenKind::Question => return true,
            TokenKind::Period | TokenKind::QuestionPeriod
                if index + 2 == run.len() && c.lexeme(run[index + 1]) == "length" =>
            {
                return true;
            }
            kind if is_binary_operator(kind) => return true,
            _ => {}
        }
        index += 1;
    }
    false
}

/// The operators that make the head a `BinaryExpression`. `as` is a
/// cast (the official rule has no arm for it), `..` a cascade and the
/// assignment forms are neither.
fn is_binary_operator(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Percent
            | TokenKind::TildeSlash
            | TokenKind::LtLt
            | TokenKind::GtGt
            | TokenKind::GtGtGt
            | TokenKind::Ampersand
            | TokenKind::Bar
            | TokenKind::Caret
            | TokenKind::EqEq
            | TokenKind::BangEq
            | TokenKind::Lt
            | TokenKind::Gt
            | TokenKind::LtEq
            | TokenKind::GtEq
            | TokenKind::AmpersandAmpersand
            | TokenKind::BarBar
            | TokenKind::QuestionQuestion
    )
}

fn kind(c: &Check<'_>, token: u32) -> TokenKind {
    c.token(token).map_or(TokenKind::Eof, |token| token.kind)
}

#[cfg(test)]
mod tests {
    use rustc_hash::FxHashSet;

    use crate::engine::{Features, Unit};
    use crate::testsupport::one_rule;

    /// The 2.19 posture: a pre-3.0 unit (`@dart=2.19`, or a package whose
    /// language version pins 2.19) carries `patterns: false`, which is the
    /// rule's own registration input. The grammar stays the bridge's fixed
    /// 3.15 surface — the head is read off its tokens.
    fn legacy(source: &str) -> Vec<crate::engine::Lint> {
        run_with(
            source,
            Features {
                patterns: false,
                ..Features::default()
            },
        )
    }

    /// The 3.0+ posture (the default unit): the registration bail keeps the
    /// rule silent, and the same source never reports.
    fn modern(source: &str) -> Vec<crate::engine::Lint> {
        run_with(source, Features::default())
    }

    fn run_with(source: &str, features: Features) -> Vec<crate::engine::Lint> {
        let parsed = crate::parse_unit(source, features);
        let unit = Unit {
            source: &parsed.source,
            tokens: &parsed.tokens,
            ast: &parsed.ast,
            lines: &parsed.lines,
            path: Some(std::path::Path::new("lint_fixture.dart")),
            features: parsed.features,
            version: None,
            workspace: None,
            reachability: None,
            async_context_uses: None,
            library_type_uses: None,
        };
        crate::engine::run(
            &unit,
            &one_rule("invalid_case_patterns"),
            &FxHashSet::default(),
        )
    }

    /// `test_binaryExpression_plus` — the message is verbatim.
    #[test]
    fn binary_expression_reports() {
        let lints = legacy("void f(Object o) {\n  switch (o) {\n    case 1 + 2:\n  }\n}\n");
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "This expression is not valid in a 'case' clause in Dart 3.0."
        );
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try refactoring the expression to be valid in 3.0.")
        );
    }

    /// The gate's two polarities, side by side: the legacy unit reports the
    /// head, and the same unit at the 3.0+ posture stays silent (the
    /// official registration bail — a gate read off the parse shape would
    /// invert both).
    #[test]
    fn the_language_version_gate_is_the_units_own_enablement() {
        let source = "void f(Object o) {\n  switch (o) {\n    case 1 + 2:\n  }\n}\n";
        assert_eq!(legacy(source).len(), 1);
        assert!(modern(source).is_empty(), "a 3.0+ unit registers the rule off");
        // A modern head (a valid constant pattern) is silent either way —
        // the gate silences it, not the classification.
        let valid = "void f(Object o) {\n  switch (o) {\n    case 1:\n  }\n}\n";
        assert!(modern(valid).is_empty());
        assert!(legacy(valid).is_empty());
    }

    /// Every head of one switch reports exactly once — the block walk is
    /// depth-tracked, so a nested switch's head is left to that switch's
    /// own dispatch (never double-reported) — and `default` never reports.
    #[test]
    fn every_head_of_the_block_reports_once_and_default_never_does() {
        let source = concat!(
            "void f(Object o, Object p) {\n",
            "  switch (o) {\n",
            "    case 1 + 2:\n",
            "    case 3 is int:\n",
            "    default:\n",
            "      switch (p) {\n",
            "        case 4 * 5:\n",
            "      }\n",
            "  }\n",
            "}\n",
        );
        let lints = legacy(source);
        assert_eq!(lints.len(), 3, "{lints:?}");
        let heads: Vec<&str> = lints
            .iter()
            .map(|lint| &source[lint.range.start as usize..lint.range.end as usize])
            .collect();
        assert_eq!(heads, vec!["1 + 2", "3 is int", "4 * 5"]);
    }

    /// `test_binaryExpression_logicalAnd` / `test_conditionalExpression`
    /// / `test_isExpression`.
    #[test]
    fn binary_conditional_and_is_faces_report() {
        assert_eq!(
            legacy("f(bool b) {\n  switch (b) {\n    case true && false:\n  }\n}\n").len(),
            1
        );
        assert_eq!(
            legacy("void f(Object o) {\n  switch (o) {\n    case true ? 1 : 2:\n  }\n}\n").len(),
            1
        );
        assert_eq!(
            legacy("void f(Object o) {\n  switch (o) {\n    case 1 is int:\n  }\n}\n").len(),
            1
        );
    }

    /// `test_listLiteral` / `test_mapLiteral` / `test_setLiteral` and
    /// their `const`/type-argument spellings.
    #[test]
    fn collection_literals_report_unless_const() {
        assert_eq!(
            legacy("void f(Object o) {\n  switch (o) {\n    case [1, 2]:\n  }\n}\n").len(),
            1
        );
        assert_eq!(
            legacy("void f(Object o) {\n  switch (o) {\n    case const [1, 2]:\n  }\n}\n").len(),
            0
        );
        assert_eq!(
            legacy("void f(Object o) {\n  switch (o) {\n    case {'k': 'v'}:\n  }\n}\n").len(),
            1
        );
        assert_eq!(
            legacy("void f(Object o) {\n  switch (o) {\n    case {1}:\n  }\n}\n").len(),
            1
        );
        assert_eq!(
            legacy("void f(Object o) {\n  switch (o) {\n    case <int>[1, 2]:\n  }\n}\n").len(),
            1
        );
    }

    /// `test_mapLiteral_parenthesized` / `..._twice` — the peel walks
    /// every wrapper and the report stays inside the parens.
    #[test]
    fn parenthesized_form_reports_inside_the_parens() {
        let source = "void f(Object o) {\n  switch (o) {\n   case (({'k': 'v'})):\n  }\n}\n";
        let lints = legacy(source);
        assert_eq!(lints.len(), 1, "{lints:?}");
        let reported = &source[lints[0].range.start as usize..lints[0].range.end as usize];
        assert_eq!(reported, "{'k': 'v'}");
    }

    /// `test_prefixedExpression_intLiteral_ok` / `test_unaryOperator_minus`
    /// / `test_unaryOperator_not` — only the non-integer operand
    /// reports.
    #[test]
    fn unary_prefix_follows_the_integer_literal_exemption() {
        assert_eq!(
            legacy("void f(Object o) {\n  switch (o) {\n    case -1:\n  }\n}\n").len(),
            0
        );
        assert_eq!(
            legacy("void f() {\n  const o = 1;\n  switch (1) {\n    case -o:\n  }\n}\n").len(),
            1
        );
        assert_eq!(
            legacy("void f() {\n  const b = false;\n  switch (true) {\n    case !b:\n  }\n}\n")
                .len(),
            1
        );
    }

    /// `test_identicalCall` / `test_lengthCall`.
    #[test]
    fn identical_and_length_faces_report() {
        assert_eq!(
            legacy("void f(Object o) {\n  switch (o) {\n    case identical(1, 2):\n  }\n}\n").len(),
            1
        );
        assert_eq!(
            legacy("void f(Object o) {\n  switch (o) {\n    case ''.length:\n  }\n}\n").len(),
            1
        );
    }

    /// `test_wildcard` — the bare `_` identifier.
    #[test]
    fn bare_wildcard_reports() {
        assert_eq!(
            legacy("f(int n) {\n  const _ = 3;\n  switch (n) {\n    case _:\n  }\n}\n").len(),
            1
        );
    }

    /// `test_parenthesizedExpression_ok` — `(1)` stays silent.
    #[test]
    fn parenthesized_integer_is_silent() {
        assert_eq!(
            legacy("void f(Object o) {\n  switch (o) {\n    case (1):\n  }\n}\n").len(),
            0
        );
    }

    /// `test_constConstructorCall` — the implicit-const constructor
    /// face is the documented under-report (constructor resolution).
    #[test]
    fn constructor_call_is_the_documented_under_report() {
        assert_eq!(
            legacy(
                "class C {\n  const C();\n}\n\nf(C c) {\n  switch (c) {\n    case C():\n  }\n}\n"
            )
            .len(),
            0
        );
    }

    /// A `const _ = 3;` shadow and plain constants never report.
    #[test]
    fn plain_constant_cases_are_silent() {
        assert_eq!(
            legacy(
                "void f(Object o) {\n  switch (o) {\n    case 1:\n    case 'x':\n    case true:\n  }\n}\n"
            )
            .len(),
            0
        );
    }
}
