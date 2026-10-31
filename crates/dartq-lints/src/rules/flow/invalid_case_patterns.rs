use dartq_ast::NodeKind;
use dartq_scanner::{Keyword, TokenKind};

use super::{Check, PayloadField, RuleSpec};

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
/// lint exists for pre-3.0 libraries only); the same
/// [`crate::engine::Features::patterns`] bit keys this rule's
/// registration, so with patterns on the rule never runs.
///
/// The head is classified from its TOKENS, not from the arena rows. The
/// lint bridge parses every unit on the fixed Dart 3.15 surface
/// ([`crate::bridge`] `PARSE_FEATURES`), so a 2.19 head is parsed as a
/// *pattern*, and the pattern grammar truncates the legacy expression
/// surface (`case 1 + 2:` folds to the constant pattern `1`, the `+ 2`
/// tail recovered) — the rows cannot answer the official arms. The token
/// run between `case` and the head's own `:` is exactly the surface the
/// official expression parse consumed, so every arm reads off it; the
/// report range is that run's extent (the official
/// `reportAtNode(expression)`).
///
/// Boundaries: the `identical`/`length` membership faces are name-level
/// (a user `identical`/`length` over-reports); the implicit-const
/// constructor-call arm (`C()` where the constructor is const, :78-81)
/// needs constructor resolution the rule context cannot reach and
/// under-reports. Message: `diagnostic.g.dart` `invalidCasePatterns`.
pub(crate) static SPEC: RuleSpec =
    crate::spec!("invalid_case_patterns", &[NodeKind::SwitchCase], check,);

fn check(c: &mut Check<'_>) {
    // The registration bail (:41).
    if c.unit.features.patterns {
        return;
    }
    // Expressions precede the statements (the `SwitchCase` row's
    // `ExpressionCount` word counts them; the Label prefix reads off).
    let count = c
        .field(c.node(), PayloadField::ExpressionCount)
        .unwrap_or(0) as usize;
    if count == 0 {
        return;
    }
    let kids = c.collect_direct(c.node());
    let wrappers: Vec<u32> = kids
        .iter()
        .copied()
        .filter(|&kid| c.kind(kid) != NodeKind::Label)
        .take(count)
        .collect();
    for wrapper in wrappers {
        let Some(head) = head_tokens(c, wrapper) else {
            continue;
        };
        let head = unparenthesized(c, &head);
        if head.is_empty() || !is_invalid(c, head) {
            continue;
        }
        let (Some(start), Some(end)) = (
            c.utf16_of_token(head[0]),
            c.utf16_end_of_token(head[head.len() - 1]),
        ) else {
            continue;
        };
        c.report(start..end, &[]);
    }
}

/// The head's token run: every token between the `case` keyword and the
/// head's own `:`.
///
/// The run is read off the token table (source order), never the arena:
/// the parse recovers the legacy tail as junk (`expect_kind` splices a
/// synthetic colon AT the offending token and consumes it), so the case
/// row's extent stops short of the surface the official rule inspects. A
/// depth-0 `:` ends the head, a ternary's colon is consumed by its
/// pending `?`, and a `when` clause (`when` is a 3.0 suffix, outside this
/// rule's gate) ends it too.
fn head_tokens(c: &Check<'_>, wrapper: u32) -> Option<Vec<u32>> {
    let case_token = c.head_token(wrapper);
    if c.token_is_synthetic(case_token) {
        return None;
    }
    let tokens = c.unit.tokens;
    let mut run = Vec::new();
    let mut depth = 0usize;
    let mut ternary = 0usize;
    let mut index = case_token + 1;
    while let Some(token) = tokens.get(index as usize) {
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
    Some(run)
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

    /// The 2.19 posture: the bit gates the rule's registration (the
    /// official `LanguageVersion219` corpus), while the grammar stays the
    /// bridge's fixed 3.15 surface — the head is read off its tokens.
    fn legacy(source: &str) -> Vec<crate::engine::Lint> {
        let features = Features {
            patterns: false,
            ..Features::default()
        };
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
        let parsed = crate::parse_unit(
            source,
            Features {
                patterns: false,
                ..Features::default()
            },
        );
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
        let lints = crate::engine::run(
            &unit,
            &one_rule("invalid_case_patterns"),
            &FxHashSet::default(),
        );
        assert_eq!(lints.len(), 1, "{lints:?}");
        // The third `{` token: the first opens the function body, the
        // second the switch block, the third the case's map literal.
        let brace = parsed
            .tokens
            .as_slice()
            .iter()
            .enumerate()
            .filter(|(_, token)| parsed.source[token.byte_range().clone()] == *"{")
            .nth(2)
            .map(|(index, _)| index)
            .expect("the literal brace");
        assert_eq!(
            lints[0].range.start,
            parsed.tokens.as_slice()[brace].utf16_offset
        );
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

    /// The gate (:41): with the feature on the rule is silent.
    #[test]
    fn patterns_feature_on_is_silent() {
        let f = crate::testsupport::Fixture::parse(
            "void f(Object o) {\n  switch (o) {\n    case 1 + 2:\n  }\n}\n",
        );
        assert!(f.run(&one_rule("invalid_case_patterns")).is_empty());
    }

    /// A `const _ = 3;` shadow and plain constants never report.
    #[test]
    fn plain_constant_cases_are_silent() {
        assert_eq!(
            legacy("void f(Object o) {\n  switch (o) {\n    case 1:\n    case 'x':\n    case true:\n  }\n}\n").len(),
            0
        );
    }
}
