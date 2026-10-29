use dartq_ast::NodeKind;

use super::{Check, PayloadField, RuleSpec};
use core::ops::Range;

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/join_return_with_assignment.dart` — a
/// block whose last statement is `return e;` and whose second-to-last
/// statement assigns the same target reports at that statement
/// (:54-93): the assignment is an expression statement whose
/// unParenthesized root is an assignment (`=` or compound — the
/// LHS, :21-23) or a prefix/postfix expression (the operand, :24-28);
/// the report fires only when the target differs from the
/// third-to-last statement's target (:84-93 — a same-target
/// assignment one step further back keeps the assignment, and any
/// null side of that comparison answers false).
///
/// Faces over the syntax substrate: the official element identity
/// (`canonicalElementsFromIdentifiersAreEqual`,
/// `dart_type_utilities.dart:96-136`) is approximated by the written
/// spelling — both sides must be identifier chains (a simple
/// identifier or a `.`/`?.` selector chain whose head is neither
/// `this` nor `super`, the official `PropertyAccess` target walk dying
/// on every non-chain target) and byte-equal. Whitespace inside a
/// chain (`a . b` vs `a.b`) under-reports; same-file implicit-`this`
/// fields join as bare names, matching the official `SimpleIdentifier`
/// face. Message: `diagnostic.g.dart` `joinReturnWithAssignment`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "join_return_with_assignment",
    &[NodeKind::Block, NodeKind::BlockBody],
    check,
);

fn check(c: &mut Check<'_>) {
    // Production bodies tile as `BlockBody` whose direct children
    // interleave each statement's leaked material rows with its
    // trailing handle; the statement sequence is the statement-row
    // filter over the child run.
    let statements: Vec<u32> = c
        .collect_direct(c.node())
        .into_iter()
        .filter(|&row| crate::rules::is_statement_row(c, row))
        .collect();
    let length = statements.len();
    if length < 2 {
        return;
    }
    // The last statement: `return e;` (the expression run is the
    // window the statement adopts; a bare `return;` adopts nothing,
    // :60-66).
    let last = statements[length - 1];
    if c.kind(last) != NodeKind::ReturnStatement {
        return;
    }
    let owner = c.node();
    let Some(last_expression) = expression_span(c, owner, last) else {
        return;
    };
    let second_last = statements[length - 2];
    // The second-to-last statement must assign (:69-75).
    let Some(second_last_expression) = assignment_target(c, owner, second_last) else {
        return;
    };
    // The third-to-last target — `None` for fewer statements or any
    // other statement shape (:77-83).
    let third_last_expression = length
        .checked_sub(3)
        .and_then(|index| assignment_target(c, owner, statements[index]));
    // `!equal(second, third) && equal(last, second)` (:84-93); any
    // null side of the official comparison answers false.
    if !equal(
        c,
        &second_last_expression.span,
        third_last_expression.as_ref().map(|t| &t.span),
    ) && equal(c, &last_expression, Some(&second_last_expression.span))
    {
        // The official node is the assignment statement's expression
        // (its material rows' combined span).
        c.report(utf16_run_span(c, &second_last_expression.rows), &[]);
    }
}

/// The statement's material rows: the window the handle adopted, or —
/// the production tiling — the owner-level leak before the childless
/// handle (everything after the previous statement row). `owner` is
/// the dispatched block row.
fn material(c: &Check<'_>, owner: u32, statement: u32) -> Option<Vec<u32>> {
    let window = c.collect_direct(statement);
    if !window.is_empty() {
        return Some(window);
    }
    let kids = c.collect_direct(owner);
    let position = kids.iter().position(|&kid| kid == statement)?;
    Some(crate::rules::slice::statement_run(c, &kids[..position]))
}

/// The statement's expression byte span: the material rows' combined
/// extent, with matching outer parens peeled (`unParenthesized`). The
/// send group's marker rows anchor at the statement's trailing tokens
/// (`;`) and are excluded — only the content rows carry the
/// expression text.
fn expression_span(c: &Check<'_>, owner: u32, statement: u32) -> Option<Range<usize>> {
    let rows = material(c, owner, statement)?;
    expression_rows_span(c, &rows)
}

fn expression_rows_span(c: &Check<'_>, rows: &[u32]) -> Option<Range<usize>> {
    let content: Vec<u32> = rows
        .iter()
        .copied()
        .filter(|&row| {
            !crate::rules::slice::is_send_group_piece(c, row)
                // The childless access markers adopt the statement's
                // trailing tokens (`a.f;` seals its `DotAccess` over
                // `f;`) — they carry no expression text of their own.
                && !matches!(
                    c.kind(row),
                    NodeKind::DotAccess | NodeKind::Qualified
                )
        })
        .collect();
    let start = content.iter().map(|&row| c.byte_span(row).start).min()?;
    let end = content.iter().map(|&row| c.byte_span(row).end).max()?;
    Some(peel_parens(c, start..end))
}

fn peel_parens(c: &Check<'_>, mut span: Range<usize>) -> Range<usize> {
    let source = c.unit.source.as_bytes();
    while source.get(span.start) == Some(&b'(')
        && source.get(span.end.saturating_sub(1)) == Some(&b')')
    {
        // Trim only a pair wrapping the whole span (a `()`-free
        // interior would leave the depth negative mid-span).
        let mut depth = 0i32;
        let mut matched = true;
        for &byte in &source[span.start..span.end] {
            match byte {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth < 0 {
                        matched = false;
                        break;
                    }
                }
                _ => {}
            }
        }
        if !matched || depth != 0 {
            break;
        }
        span.start += 1;
        span.end -= 1;
    }
    span
}

/// `_getExpressionFromAssignmentStatement` (:18-30): the statement
/// must be an expression statement whose unParenthesized root is an
/// assignment (the LHS, :21-23) or a prefix/postfix expression (the
/// operand, :24-28). The target is the operand run's byte span over
/// its rows: the assignment splits at the operator token, the
/// prefix/postfix marker takes the run before it.
fn assignment_target(c: &Check<'_>, owner: u32, statement: u32) -> Option<Target> {
    if c.kind(statement) != NodeKind::ExpressionStatement {
        return None;
    }
    let rows = material(c, owner, statement)?;
    let root = *rows.last()?;
    let position = rows.iter().position(|&row| row == root)?;
    let operand_rows: Vec<u32> = match c.kind(root) {
        NodeKind::Assignment => {
            // The LHS completes before the RHS (operands×2 contract);
            // the operator byte splits the run.
            let operator = c.field(root, PayloadField::Operator)?;
            let operator_byte = c
                .unit
                .tokens
                .get(operator as usize)
                .map_or(usize::MAX, |token| token.byte_offset as usize);
            let split = rows.partition_point(|&row| c.byte_span(row).end <= operator_byte);
            rows[..split].to_vec()
        }
        NodeKind::UnaryPrefix | NodeKind::UnaryPostfix => rows[..position].to_vec(),
        _ => return None,
    };
    if operand_rows.is_empty() {
        return None;
    }
    let span = expression_rows_span(c, &operand_rows)?;
    Some(Target {
        span,
        rows: operand_rows,
    })
}

struct Target {
    /// The target's byte span over the source (the equality face).
    span: Range<usize>,
    /// The target's rows (the report span).
    rows: Vec<u32>,
}

/// The written-spelling face of the official element identity: both
/// sides must be identifier chains and byte-equal.
fn equal(c: &Check<'_>, left: &Range<usize>, right: Option<&Range<usize>>) -> bool {
    let Some(right) = right else {
        return false;
    };
    let is_chain = |span: &Range<usize>| {
        c.unit
            .source
            .get(span.clone())
            .is_some_and(|spelling| is_identifier_chain(spelling))
    };
    is_chain(left) && is_chain(right) && c.unit.source[left.clone()] == c.unit.source[right.clone()]
}

/// UTF-16 min/max over a row run — the engine's external coordinates.
fn utf16_run_span(c: &Check<'_>, rows: &[u32]) -> Range<u32> {
    let mut span = c.utf16_span(rows[0]);
    for &row in &rows[1..] {
        let other = c.utf16_span(row);
        span.start = span.start.min(other.start);
        span.end = span.end.max(other.end);
    }
    span
}

/// `ident('.' ident)*` with optional `?.` separators and a
/// non-`this`/`super` head — the `SimpleIdentifier` /
/// `PrefixedIdentifier` / `PropertyAccess` faces.
fn is_identifier_chain(spelling: &str) -> bool {
    let mut segments = spelling.split('.');
    // `split` always yields at least one (possibly empty) segment.
    let head = segments.next().unwrap_or("");
    if head == "this" || head == "super" || !is_identifier(head) {
        return false;
    }
    segments.all(|segment| is_identifier(segment.strip_suffix('?').unwrap_or(segment)))
}

fn is_identifier(spelling: &str) -> bool {
    let mut characters = spelling.chars();
    matches!(characters.next(), Some(first) if first.is_ascii_alphabetic() || first == '_' || first == '$')
        && characters.all(|rest| rest.is_ascii_alphanumeric() || rest == '_' || rest == '$')
}

#[cfg(test)]
mod tests {
    use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

    fn run(source: &str) -> Vec<crate::engine::Lint> {
        Fixture::parse(source).run(&one_rule("join_return_with_assignment"))
    }

    /// `test_localVariable_assignment` — message, correction, and the
    /// report pinning the assignment statement.
    #[test]
    fn local_variable_assignment_reports() {
        let source = "int f(int a) {\n  a = 0;\n  return a;\n}\n";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("join_return_with_assignment"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Assignment could be inlined in 'return' statement."
        );
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try inlining the assigned value in the 'return' statement.")
        );
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(source, &f.tokens, "a", 1))
        );
    }

    /// `test_localVariable_assignment_compound` — `+=` joins.
    #[test]
    fn compound_assignment_reports() {
        assert_eq!(run("int f(int a) {\n  a += 0;\n  return a;\n}\n").len(), 1);
    }

    /// `test_class_field_withoutPrefix` — `??=` joins; the
    /// implicit-`this` field reads as the bare name.
    #[test]
    fn null_aware_assignment_reports() {
        let source =
            "class A {\n  int? _a;\n  int? foo() {\n    _a ??= 0;\n    return _a;\n  }\n}\n";
        assert_eq!(run(source).len(), 1);
    }

    /// `test_localVariable_postfix` / `test_localVariable_prefix`.
    #[test]
    fn prefix_and_postfix_increment_report() {
        assert_eq!(run("int f(int a) {\n  a++;\n  return a;\n}\n").len(), 1);
        assert_eq!(run("int f(int a) {\n  ++a;\n  return a;\n}\n").len(), 1);
    }

    /// `test_class_field_propertyAccess` / `..._nested` — chains join.
    #[test]
    fn property_access_chains_report() {
        let source = "class A {\n  int f = 0;\n}\n\nint f(A a) {\n  a.f = 0;\n  return a.f;\n}\n";
        assert_eq!(run(source).len(), 1);
        let source = "class A {\n  int f = 0;\n}\n\nclass B {\n  A a = A();\n}\n\nint f(B b) {\n  b.a.f = 0;\n  return b.a.f;\n}\n";
        assert_eq!(run(source).len(), 1);
    }

    /// `test_localVariable_hasPreviousAssignment` — a same-target
    /// assignment one step further back keeps the assignment.
    #[test]
    fn three_statement_same_target_is_silent() {
        assert!(run("int f(int a) {\n  a += 1;\n  a += 2;\n  return a;\n}\n").is_empty());
    }

    /// `test_class_field_propertyAccess_notSame` /
    /// `..._nested_notSame` — different chains stay silent.
    #[test]
    fn different_targets_are_silent() {
        let source =
            "class A {\n  int f = 0;\n}\n\nint f(A a1, A a2) {\n  a1.f = 0;\n  return a2.f;\n}\n";
        assert!(run(source).is_empty());
        let source = "class A {\n  int f = 0;\n}\n\nclass B {\n  A a = A();\n}\n\nint f(B b1, B b2) {\n  b1.a.f = 0;\n  return b2.a.f;\n}\n";
        assert!(run(source).is_empty());
    }

    /// `test_localVariable_declaration` — a declaration is not an
    /// assignment.
    #[test]
    fn declaration_is_silent() {
        assert!(run("int f(int a) {\n  var a = 0;\n  return a;\n}\n").is_empty());
    }

    /// `test_patternAssignment_multiple` /
    /// `test_patternVariableDeclaration_multiple` — pattern shapes
    /// are not the assignment face.
    #[test]
    fn pattern_shapes_are_silent() {
        assert!(run("int f(int a, int b) {\n  (a, b) = (0, 1);\n  return a;\n}\n").is_empty());
        assert!(run("int f(int a, int b) {\n  var (a, b) = (0, 1);\n  return a;\n}\n").is_empty());
    }

    /// A bare `return;` has no expression (:60-66); a non-return last
    /// statement bails (:64).
    #[test]
    fn bare_return_and_wrong_order_are_silent() {
        assert!(run("int f(int a) {\n  a = 0;\n  return;\n}\n").is_empty());
        assert!(run("int f(int a) {\n  return a;\n  a = 0;\n}\n").is_empty());
    }

    /// The explicit `this` selector never joins (the official
    /// `PropertyAccess` target walk dies on `this`).
    #[test]
    fn explicit_this_chain_is_silent() {
        let source = "class A {\n  int? _a;\n  int? foo() {\n    this._a = 0;\n    return this._a;\n  }\n}\n";
        assert!(run(source).is_empty());
    }

    /// Non-chain shapes (`a[0]`) stay silent — the official
    /// comparison answers false off the identifier-chain faces.
    #[test]
    fn indexed_target_is_silent() {
        assert!(run("int f(List<int> a) {\n  a[0] = 1;\n  return a[0];\n}\n").is_empty());
    }

    /// `test_class_field_withoutPrefix_ifThenElse` — each block's
    /// tail is judged independently.
    #[test]
    fn nested_blocks_report_independently() {
        let source = "class A {\n  int? _a;\n  int? foo(bool b) {\n    if (b) {\n      _a = 0;\n      return _a;\n    } else {\n      _a = 1;\n      return _a;\n    }\n  }\n}\n";
        assert_eq!(run(source).len(), 2);
    }
}
