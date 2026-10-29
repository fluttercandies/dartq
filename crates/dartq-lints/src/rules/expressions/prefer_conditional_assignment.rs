use super::prefer_null_aware_method_calls::then_branch_run;
use super::{Check, NONE_TOKEN, NodeKind, PayloadField, RuleSpec};

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/prefer_conditional_assignment.dart`
/// — an `if` without `else` (`visitIfStatement` :70-79) whose condition
/// is a null check `x == null` — either side after peeling the whole
/// condition and each operand's parens (`_getExpressionCondition`
/// :37-48) — and whose then arm is one expression statement assigning
/// to `x` (`_checkStatement` :27-34, `_checkExpression` :20-24)
/// reports at the `if` extent. The assignment carries no operator gate
/// (`+=` and chained `x = y = 1`'s outer assignment fire), and the
/// identity rides the identifier-chain walk
/// (`canonicalElementsFromIdentifiersAreEqual`,
/// `util/dart_type_utilities.dart:96-126`): both sides must be
/// identifier chains — a bare name, a prefixed name, or a property
/// chain (a wrapped chain target reads through its container parens,
/// the walk's per-target `unParenthesized`) — so `this.x`, `a[0]`, and
/// invocation targets never pass.
/// The token-lexeme sequence comparison stands in for the element
/// identity; the difference is confined to renames across scopes a
/// lint cannot see either. Message: `diagnostic.g.dart`
/// `preferConditionalAssignment`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_conditional_assignment",
    &[NodeKind::IfStatement],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `node.elseStatement != null` bails (:71-73).
    if c.field(node, PayloadField::ElseKeyword) != Some(NONE_TOKEN) {
        return;
    }
    let kids = c.collect_direct(node);
    let Some(cond_pos) = kids
        .iter()
        .position(|&kid| c.kind(kid) == NodeKind::ParenthesizedCondition)
    else {
        return;
    };
    // `if (expr case pattern)` — the pattern face has no expression
    // condition to test.
    if c.field(kids[cond_pos], PayloadField::CaseKeyword) != Some(NONE_TOKEN) {
        return;
    }
    // The condition binary: the adopted condition's tail row, peeling
    // paren containers (`rawExpression.unParenthesized`, :38).
    let Some(mut condition) = c.collect_direct(kids[cond_pos]).last().copied() else {
        return;
    };
    while c.kind(condition) == NodeKind::ParenthesizedExpression {
        let Some(inner) = c.collect_direct(condition).last().copied() else {
            return;
        };
        condition = inner;
    }
    if c.kind(condition) != NodeKind::BinaryExpression {
        return;
    }
    let Some(op) = c.field_token(condition, PayloadField::Operator) else {
        return;
    };
    // `operator.type == TokenType.EQ_EQ` (:40) — `!=` never fires.
    if c.lexeme(op) != "==" {
        return;
    }
    // The window rows split at the operator; each side's root peels
    // its wrapped parens. Upstream order (:40-46): the right `null`
    // takes the left operand, else the mirrored left `null` takes the
    // right one.
    let left: Vec<u32> = c
        .children(condition)
        .filter(|&row| c.head_token(row) < op)
        .collect();
    let right: Vec<u32> = c
        .children(condition)
        .filter(|&row| c.head_token(row) >= op)
        .collect();
    let Some(left_root) = left
        .last()
        .copied()
        .and_then(|root| unparenthesized_tail(c, root))
    else {
        return;
    };
    let Some(right_root) = right
        .last()
        .copied()
        .and_then(|root| unparenthesized_tail(c, root))
    else {
        return;
    };
    let operand_is_left = if c.kind(right_root) == NodeKind::LiteralNull {
        true
    } else if c.kind(left_root) == NodeKind::LiteralNull {
        false
    } else {
        return;
    };
    // The operand run: the binary's own window rows on the operand
    // side, prefixed with every ancestor level's pieces left of the
    // operator. The adopted-begin parks the operand's completion
    // pieces at whatever enclosing row absorbed them — the condition
    // container, a wrapped `(...)` level between, or the `if` row
    // itself (`x == null` keeps the `x` send-group's receiver pieces
    // as direct `IfStatement` children; `a.b == null` parks the whole
    // receiver chain there). The walk ends at `node` — no operand
    // piece can escape above the statement.
    let mut operand: Vec<u32> = if operand_is_left {
        left.clone()
    } else {
        right.clone()
    };
    {
        let mut batches: Vec<Vec<u32>> = Vec::new();
        let mut level = condition;
        while let Some(parent) = c.unit.ast.parent(level) {
            let owner_kids = c.collect_direct(parent);
            let Some(pos) = owner_kids.iter().position(|&kid| kid == level) else {
                break;
            };
            batches.push(
                owner_kids[..pos]
                    .iter()
                    .copied()
                    .filter(|&row| c.head_token(row) < op)
                    .collect(),
            );
            if parent == node {
                break;
            }
            level = parent;
        }
        for batch in batches.into_iter().rev() {
            let mut prefixed = batch;
            prefixed.extend(core::mem::take(&mut operand));
            operand = prefixed;
        }
    }

    // `_checkStatement` (:27-34): one expression statement — a bare
    // statement or the single-statement block unwrap.
    let Some(branch) = then_branch_run(c, &kids[cond_pos + 1..]) else {
        return;
    };
    // `_checkExpression` (:20-24) has no operator gate: the branch's
    // last `AssignmentExpression` row (root-last, so the outer
    // assignment of a chain `x = y = 1`) is the tested expression.
    let Some(assign_pos) = branch
        .iter()
        .rposition(|&row| c.kind(row) == NodeKind::Assignment)
    else {
        return;
    };
    let assign = branch[assign_pos];
    let Some(assign_op) = c.field_token(assign, PayloadField::Operator) else {
        return;
    };
    // `expression.leftHandSide`: the owner-side pieces before the
    // assignment row plus its adopted window's rows, both cut at the
    // operator token; a wrapped LHS keeps its container row at the
    // tail — peeled below.
    let mut lhs: Vec<u32> = branch[..assign_pos]
        .iter()
        .copied()
        .filter(|&row| c.head_token(row) < assign_op)
        .collect();
    lhs.extend(
        c.children(assign)
            .filter(|&row| c.head_token(row) < assign_op),
    );

    // `canonicalElementsFromIdentifiersAreEqual`
    // (`util/dart_type_utilities.dart:96-126`): both sides fold to
    // their canonical identifier-name chains — parens peel, markers
    // contribute their selector, and a selector whose token the run
    // already carries as an identifier row is the completion
    // duplicate. `this.x`, `super.x`, `a[0]`, and invocation targets
    // fold to nothing and never match.
    let operand_chain = identifier_chain(c, &operand);
    let lhs_chain = identifier_chain(c, &lhs);
    if operand_chain.is_none() || operand_chain != lhs_chain {
        return;
    }
    let range = c.utf16_span(node);
    c.report(range, &[]);
}

/// The row's root after peeling wrapped `(...)` containers — the
/// row-level `unParenthesized` (:38). `None` past an empty recovery
/// window.
fn unparenthesized_tail(c: &Check<'_>, mut root: u32) -> Option<u32> {
    while c.kind(root) == NodeKind::ParenthesizedExpression {
        let window = c.children(root);
        root = window
            .end
            .checked_sub(1)
            .filter(|&last| last >= window.start)?;
    }
    Some(root)
}

/// The run's canonical identifier-name chain — the production-tiling
/// stand-in for `canonicalElementsFromIdentifiersAreEqual`
/// (`util/dart_type_utilities.dart:96-126`). Identifier rows name
/// themselves; a dot-access marker names the token after its own dot
/// (payload word 0), unless that token already sits in the run as an
/// identifier row (the LHS tiling stamps the selector beside the
/// marker, the condition tiling leaves it only on the marker — the
/// token check equates both tilings). Parens peel recursively; the
/// send-group markers carry no element; any other row (`this`, index,
/// arguments, a literal target) is not an identifier chain and folds
/// to `None`.
fn identifier_chain(c: &Check<'_>, run: &[u32]) -> Option<Vec<String>> {
    let mut names = Vec::new();
    let mut row_tokens = Vec::new();
    chain_names(c, run, &mut names, &mut row_tokens)?;
    (!names.is_empty()).then_some(names)
}

fn chain_names(
    c: &Check<'_>,
    run: &[u32],
    names: &mut Vec<String>,
    row_tokens: &mut Vec<u32>,
) -> Option<()> {
    for &row in run {
        match c.kind(row) {
            NodeKind::Identifier => {
                // A wrapped subtree's pieces tile beside the container
                // in the same run — a paren recursion would re-count the
                // name; the head token dedupes it.
                let head = c.head_token(row);
                if !row_tokens.contains(&head) {
                    row_tokens.push(head);
                    names.push(c.lexeme(head).to_owned());
                }
            }
            NodeKind::DotAccess => {
                let dot = c.field(row, PayloadField::Operator)?;
                let selector = c.unit.tokens.get(dot as usize + 1)?;
                let name = c.unit.tokens.lexeme(c.unit.source, dot as usize + 1);
                if !row_tokens.contains(&(dot + 1)) {
                    let _ = selector;
                    names.push(name.to_owned());
                    row_tokens.push(dot + 1);
                }
            }
            NodeKind::ParenthesizedExpression => {
                chain_names(c, &c.collect_direct(row), names, row_tokens)?;
            }
            NodeKind::Send | NodeKind::NoTypeArguments | NodeKind::NoArguments => {}
            _ => return None,
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

    fn run(source: &str) -> Vec<crate::engine::Lint> {
        Fixture::parse(source).run(&one_rule("prefer_conditional_assignment"))
    }

    /// Bare statement form: one report at the whole `if` extent.
    #[test]
    fn bare_statement_form_reports_at_the_if() {
        let source = "void f(int? x, int s) {\n  if (x == null) x = s;\n}\n";
        let lints = run(source);
        assert_eq!(lints.len(), 1);
        let f = Fixture::parse(source);
        let if_kw = tok(&f.source, &f.tokens, "if", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, if_kw));
    }

    /// The corpus face: a brace-less `if` assigning a string literal
    /// over a nullable parameter reports at the whole `if` extent
    /// (34 chars of `if (label == null) label = 'none';`).
    #[test]
    fn corpus_if_null_assign_reports() {
        let f =
            Fixture::parse("void f(String? label) {\n  if (label == null) label = 'none';\n}\n");
        let lints = f.run(&one_rule("prefer_conditional_assignment"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.end - lints[0].range.start, 34);
    }

    /// The single-statement block unwrap reports once.
    #[test]
    fn single_statement_block_form_reports() {
        let source = "void f(int? x, int s) {\n  if (x == null) {\n    x = s;\n  }\n}\n";
        assert_eq!(run(source).len(), 1);
    }

    /// The whole condition wrapped in extra parens reports (one peel
    /// step over the adopted condition's tail).
    #[test]
    fn wrapped_condition_reports() {
        let source = "void f(int? x, int s) {\n  if ((x == null)) x = s;\n}\n";
        assert_eq!(run(source).len(), 1);
    }

    /// Both operands wrapped: the null side's root and the operand run
    /// peel their containers (`(x) == (null)` fires).
    #[test]
    fn wrapped_sides_report() {
        let source = "void f(int? x, int s) {\n  if ((x) == (null)) x = s;\n}\n";
        assert_eq!(run(source).len(), 1);
    }

    /// A prefixed chain matches itself on both sides.
    #[test]
    fn chain_target_reports() {
        let source = "class C {\n  String? b;\n}\nvoid f(C a) {\n  if (a.b == null) a.b = '';\n}\n";
        assert_eq!(run(source).len(), 1);
    }

    /// A parenthesized chain target matches itself: the container row
    /// rides mid-run before the selector pair, and the chain walk
    /// unparenthesizes it (the upstream per-target `unParenthesized`).
    #[test]
    fn parenthesized_target_reports() {
        let source =
            "class C {\n  String? b;\n}\nvoid f(C a) {\n  if ((a).b == null) (a).b = '';\n}\n";
        assert_eq!(run(source).len(), 1);
    }

    /// The parenthesized target matches its unwrapped forms in both
    /// directions — the same canonical chain.
    #[test]
    fn mixed_parenthesized_forms_report() {
        for source in [
            "class C {\n  String? b;\n}\nvoid f(C a) {\n  if (a.b == null) (a).b = '';\n}\n",
            "class C {\n  String? b;\n}\nvoid f(C a) {\n  if ((a).b == null) a.b = '';\n}\n",
        ] {
            assert_eq!(run(source).len(), 1, "{source}");
        }
    }

    /// A double-wrapped target unparenthesizes recursively, matching
    /// the singly wrapped shape.
    #[test]
    fn double_wrapped_target_reports() {
        let source =
            "class C {\n  String? b;\n}\nvoid f(C a) {\n  if (((a)).b == null) ((a)).b = '';\n}\n";
        assert_eq!(run(source).len(), 1);
    }

    /// A non-chain inner root keeps its container row: a `null` target
    /// fails the identifier-chain gate and stays silent.
    #[test]
    fn parenthesized_target_gates() {
        let source = "void f(int s) {\n  if ((null).b == null) (null).b = s;\n}\n";
        assert!(run(source).is_empty());
    }

    /// An `else` clause (even empty), `!=`, an unrelated LHS, a
    /// two-statement block, and a different chain target stay silent.
    #[test]
    fn silences() {
        for source in [
            "void f(int? x, int s) {\n  if (x == null) x = s; else {}\n}\n",
            "void f(int? x, int s) {\n  if (x != null) x = s;\n}\n",
            "void f(int? x, int y, int s) {\n  if (x == null) y = s;\n}\n",
            "void f(int? x, int y, int s) {\n  if (x == null) {\n    x = s;\n    y = s;\n  }\n}\n",
            "class C {\n  String? b, d;\n}\nvoid f(C a, C c) {\n  if (a.b == null) c.d = '';\n}\n",
        ] {
            assert!(run(source).is_empty(), "{source}");
        }
    }

    /// No operator gate on the assignment: `+=` fires like `=`.
    #[test]
    fn compound_assignment_fires() {
        let source = "void f(int? x) {\n  if (x == null) x += 1;\n}\n";
        assert_eq!(run(source).len(), 1);
    }

    /// Index and invocation operands fail the identifier-chain gate;
    /// `(x) = s` parses as a `PatternAssignment` and stays silent;
    /// `this.x` never passes.
    #[test]
    fn shape_gates() {
        assert!(
            run("void f(List<int?> a, int s) {\n  if (a[0] == null) a[0] = s;\n}\n").is_empty()
        );
        assert!(
            run("void f(int? Function(int) a, int i) {\n  if (a(i) == null) a(i) = 1;\n}\n")
                .is_empty()
        );
        // `(x) = s` is a pattern assignment, not an
        // `AssignmentExpression` — the upstream `_checkExpression` gate
        // keeps it silent.
        assert!(run("void f(int? x, int s) {\n  if (x == null) (x) = s;\n}\n").is_empty());
        assert!(
            run(
                "class C {\n  int? x;\n  void f() {\n    if (this.x == null) this.x = 1;\n  }\n}\n"
            )
            .is_empty()
        );
    }
}
