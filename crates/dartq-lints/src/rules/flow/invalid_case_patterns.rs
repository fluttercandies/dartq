use core::ops::Range;

use dartq_ast::NodeKind;

use super::{PayloadField, Check, RuleSpec};

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/invalid_case_patterns.dart` — a legacy
/// `case` head whose expression is not a valid 3.0 constant-pattern
/// spelling reports at the expression (:50-88): a non-`const`
/// list/set-or-map literal, a `dart:core` `identical(...)` call, a
/// unary prefix whose operand is not an integer literal, any binary
/// or conditional expression, a `dart:core` `length` property access,
/// an `is` check, or the bare `_` identifier.
///
/// Gate: the official registration bails WITH the feature (:41 — the
/// lint exists for pre-3.0 libraries only); the same
/// [`crate::engine::Features::patterns`] bit keys the parser's
/// case-head branch, so with patterns on the operands are patterns
/// and the rule is silent.
///
/// Faces: the legacy case operand sits in a [`NodeKind::CaseExpression`]
/// wrapper whose window's last row is the expression root (the fixed
/// parse surface wraps every operand; the parenthesized wrappers peel
/// the same way). The dart:core membership of `identical`/`length` is
/// name-level (a user `identical`/`length` over-reports). The
/// implicit-const constructor-call face (`C()` where the constructor
/// is const, :78-81) needs constructor resolution and under-reports.
/// Message: `diagnostic.g.dart` `invalidCasePatterns`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "invalid_case_patterns",
    &[NodeKind::SwitchCase],
    check,
);

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
        // The run bound is the case clause's own extent: the adopted
        // window anchors at earlier statements' tail tokens on the
        // recovery shapes, and their rows must not join the head.
        let Some((operand, run)) = operand_root(c, wrapper, c.byte_span(c.node())) else {
            continue;
        };
        if is_invalid(c, &run, operand) {
            // The unary-recovery face reports the whole head run (the
            // `-o` operator is a bare recovery marker, so the operand
            // row carries only the operand); every other face reports
            // the operand row (the official `reportAtNode(expression)`).
            let range = if unary_recovery(c, &run) {
                let mut span = c.utf16_span(run[0]);
                for &row in &run[1..] {
                    let other = c.utf16_span(row);
                    span.start = span.start.min(other.start);
                    span.end = span.end.max(other.end);
                }
                span
            } else {
                c.utf16_span(operand)
            };
            c.report(range, &[]);
        }
    }
}

/// The operand root and the row run that holds it: the wrapper
/// window's rows within the wrapper's own extent (earlier statements'
/// rows leak into the adopted window), peeling the parenthesized
/// wrappers and the constant-pattern wrap the patterns parse adds
/// (`unParenthesized`, :51). A wrap whose window is empty seals
/// childless — its rows precede it in the run — so the peel pops the
/// wrap row in place.
fn operand_root(c: &Check<'_>, wrapper: u32, extent: Range<usize>) -> Option<(u32, Vec<u32>)> {
    let mut run: Vec<u32> = c
        .children(wrapper)
        .filter(|&row| {
            let span = c.byte_span(row);
            span.start >= extent.start && span.end <= extent.end
        })
        .collect();
    loop {
        let Some(&last) = run.last() else {
            return None;
        };
        match c.kind(last) {
            NodeKind::ParenthesizedExpression
            | NodeKind::ParenthesizedPattern
            | NodeKind::ConstantPattern => {
                // The `ConstantPattern` row is payload-free — the
                // explicit-const skip (:52-59, :78-81) had no word to
                // read and never fired, so a const operand peels like
                // any parenthesized wrapper (the pinned behavior).
                run.pop();
            }
            _ => return Some((last, run)),
        }
    }
}

/// Whether the peeled run opens with a unary-prefix recovery (the
/// `-`/`!` operator tiles as a leading [`NodeKind::UnaryPrefix`]
/// marker or a bare [`NodeKind::RecoverableError`]).
fn unary_recovery(c: &Check<'_>, run: &[u32]) -> bool {
    run.len() > 1
        && matches!(
            c.kind(run[0]),
            NodeKind::UnaryPrefix | NodeKind::RecoverableError
        )
}

/// The official visitor's expression faces (:52-87). The patterns
/// parse spells the legacy heads under other names: the collection
/// literals become list/map patterns, `&&`/`||` become a binary
/// pattern, an `is` check recovers as an `IsExpression`, the `-`/`!`
/// prefix recovers into a leading marker, `_` becomes the wildcard
/// pattern, and `identical(...)` becomes an object pattern whose type
/// names the callee. A `ListLiteral`/`SetOrMapLiteral` row is the
/// `const`-expression form (`const [1, 2]`) — the official
/// const-literal face, never invalid.
fn is_invalid(c: &Check<'_>, run: &[u32], operand: u32) -> bool {
    // The unary prefix face (:64-67 — `-1` is valid, `-o`/`!b` are
    // not): the recovered operator marker leads the run and only a
    // single integer-literal operand is exempt.
    if unary_recovery(c, run) {
        return !(run.len() == 2 && c.kind(run[1]) == NodeKind::LiteralInt);
    }
    match c.kind(operand) {
        // A non-`const` collection literal (:52-59), any binary or
        // conditional expression, any `is` check (:68-77).
        NodeKind::ListPattern
        | NodeKind::MapPattern
        | NodeKind::BinaryExpression
        | NodeKind::BinaryPattern
        | NodeKind::Conditional
        | NodeKind::IsExpression
        | NodeKind::CastPattern => true,
        // `identical(...)` (:60-63) — the name-level dart:core face
        // over the object-pattern spelling of the call.
        NodeKind::ObjectPattern => c
            .field(operand, PayloadField::FirstName)
            .is_some_and(|head| c.lexeme(head) == "identical"),
        // `.length` (:72-75) — the name-level dart:core face.
        NodeKind::DotAccess => c.lexeme(c.head_token(operand)) == "length",
        // The bare `_` identifier (:82-87) — the wildcard spelling
        // under the patterns parse.
        NodeKind::Identifier => c.lexeme(c.head_token(operand)) == "_",
        NodeKind::WildcardPattern => true,
        // The pattern grammar rejects the head's tail outright (the
        // `true ? 1 : 2` ternary recovers into a `?`-marker and error
        // rows) — under the official 2.19 parse the same head is an
        // expression face this rule reports (:50-88).
        NodeKind::RecoverableError => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use rustc_hash::FxHashSet;

    use crate::engine::{Features, Unit};
    use crate::testsupport::one_rule;

    /// The legacy parse posture: the bridge parses with the patterns
    /// bit off, so the `case` heads are plain expressions (the same
    /// face the official `LanguageVersion219` corpus runs under).
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
            legacy("void f(Object o) {\n  switch (o) {\n    case true ? 1 : 2:\n  }\n}\n")
                .len(),
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
            legacy("void f(Object o) {\n  switch (o) {\n    case const [1, 2]:\n  }\n}\n")
                .len(),
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
            legacy("void f(Object o) {\n  switch (o) {\n    case identical(1, 2):\n  }\n}\n")
                .len(),
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
            legacy("class C {\n  const C();\n}\n\nf(C c) {\n  switch (c) {\n    case C():\n  }\n}\n").len(),
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
