use super::{Check, NodeKind, PayloadField, RuleSpec, same_file, slice};
use crate::rules::{
    byte_span_of_token, initialized_identifier_name, is_statement_row,
    variables_declaration_keyword,
};

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/cascade_invocations.dart`
/// — consecutive statements on one cascadable reference report at the
/// joined run (`_checkStatements` :348-386): a block or switch-member
/// statement run classifies each statement as a cascadable box — a
/// single-variable declaration (non-`await` initializer :26-37), an
/// assignment over a bare target or a variable-prefix setter
/// (:179-202), a non-null-aware member invocation or property access
/// over a simple receiver (:215-249), or a cascade over a simple
/// target (:204-213) — and a run joins while the boxes share the
/// element and the capability bits hold (`canJoin` over the previous
/// `canReceive`, at least one `canBeCascaded`, :253-259). The report
/// covers the second statement through the last of the run — the
/// anchor statement stays out (:370-373).
///
/// dartq faces over the production tiling: the statement run is the
/// statement rows of the block window (`Block` is the nested-statement
/// form, `BlockBody` every function/literal body — the bridge emits no
/// `Block` there; a switch member's run sits after the matched
/// expressions and the optional `when` clause). An `ExpressionStatement`
/// seals as a span-only leaf — its expression pieces tile as flat block
/// siblings inside its span, which [`statement_face`] reads off the
/// owner slice. The
/// critical-dependency gate (`_hasCriticalDependencies`) is the
/// name-level face: any `Identifier` lexeme equal to the anchor's
/// element inside the upstream criticalNodes spans — the assignment
/// right side, the selector + argument list, the accessor name, the
/// cascade sections, the variable initializer (its own name row
/// excluded) — blocks the join. The element-model gates are the
/// same-file channel: the invocation receiver, the setter prefix, and
/// the cascade target must bind to a same-file value (the upstream
/// non-static faces — a class-name receiver `C.m()` stays silent).
/// Approximations (documented): imported or untyped receivers stay
/// silent (under-report); the setter face accepts any same-file value
/// prefix (a static-setter prefix over-reports); the visitor's
/// sibling-executable and closure arms are out of the substrate; two
/// same-named elements (shadowing) join as one (over-report).
/// Message: `diagnostic.g.dart` `cascadeInvocations`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "cascade_invocations",
    &[NodeKind::Block, NodeKind::BlockBody, NodeKind::SwitchCase],
    check,
);

/// One statement's cascadable face: the element it anchors or joins,
/// the upstream capability bits, the critical scan spans (the
/// upstream criticalNodes sets), and a span to exclude from the scan
/// (the declaration's own name row).
struct CascadeBox<'a> {
    element: Option<&'a str>,
    can_join: bool,
    can_receive: bool,
    can_be_cascaded: bool,
    critical: Vec<core::ops::Range<usize>>,
    skip: Option<core::ops::Range<usize>>,
}

impl CascadeBox<'_> {
    fn null() -> Self {
        Self {
            element: None,
            can_join: false,
            can_receive: false,
            can_be_cascaded: false,
            critical: Vec::new(),
            skip: None,
        }
    }
}

fn check(c: &mut Check<'_>) {
    let node = c.node();
    let statements: Vec<u32> = match c.kind(node) {
        // `Block` is the nested-statement `{ ... }`; `BlockBody` is
        // every function/literal body — the production bridge emits
        // `BlockBody` for a body context, never `Block`.
        NodeKind::Block | NodeKind::BlockBody => c
            .collect_direct(node)
            .into_iter()
            .filter(|&row| is_statement_row(c, row))
            .collect(),
        // The member's statements tile after the matched expressions
        // (the `ExpressionCount` word) and the optional `when` clause.
        NodeKind::SwitchCase => {
            let expressions = c.field(node, PayloadField::ExpressionCount).unwrap_or(0) as usize;
            let kids = c.collect_direct(node);
            let mut start = expressions;
            if kids
                .get(start)
                .is_some_and(|&row| c.kind(row) == NodeKind::SwitchCaseWhenClause)
            {
                start += 1;
            }
            kids[start..]
                .iter()
                .copied()
                .filter(|&row| is_statement_row(c, row))
                .collect()
        }
        _ => return,
    };
    if statements.len() < 2 {
        return;
    }
    let block_kids = c.collect_direct(node);
    let mut previous = CascadeBox::null();
    let mut previous_statement: Option<u32> = None;
    let mut first: Option<u32> = None;
    for &statement in &statements {
        let current = statement_box(c, statement);
        if compatible(
            c,
            &block_kids,
            statement,
            previous_statement,
            &current,
            &previous,
        ) {
            first = first.or(Some(statement));
        } else if let (Some(first_stmt), Some(prev_stmt)) = (first, previous_statement) {
            report_run(c, first_stmt, prev_stmt);
            first = None;
        }
        previous = current;
        previous_statement = Some(statement);
    }
    if let (Some(first_stmt), Some(prev_stmt)) = (first, previous_statement) {
        report_run(c, first_stmt, prev_stmt);
    }
}

fn report_run(c: &mut Check<'_>, first: u32, last: u32) {
    let range = c.utf16_span(first).start..c.utf16_span(last).end;
    c.report(range, &[]);
}

/// The upstream `compatibleWith` (:253-259) plus the critical gate.
fn compatible(
    c: &Check<'_>,
    block_kids: &[u32],
    stmt: u32,
    previous_statement: Option<u32>,
    current: &CascadeBox<'_>,
    previous: &CascadeBox<'_>,
) -> bool {
    let (Some(element), Some(previous_element)) = (current.element, previous.element) else {
        return false;
    };
    if element != previous_element
        || !previous.can_receive
        || !current.can_join
        || !(current.can_be_cascaded || previous.can_be_cascaded)
    {
        return false;
    }
    // `_hasCriticalDependencies` (:261-270): NEITHER box's critical
    // spans may carry a reference to the element — the previous side
    // is negated too (the upstream `!` wraps the whole scan).
    !critical_hit(
        c,
        block_kids,
        stmt,
        &current.critical,
        current.skip.as_ref(),
        element,
    ) && previous_statement.is_some_and(|prev_stmt| {
        !critical_hit(
            c,
            block_kids,
            prev_stmt,
            &previous.critical,
            previous.skip.as_ref(),
            element,
        )
    })
}

/// Whether any `Identifier` row under `stmt`'s face carries `name`
/// inside one of `spans` (the subtree walk descends only inside the
/// spans, so the receiver itself never blocks its own join).
fn critical_hit(
    c: &Check<'_>,
    block_kids: &[u32],
    stmt: u32,
    spans: &[core::ops::Range<usize>],
    skip: Option<&core::ops::Range<usize>>,
    name: &str,
) -> bool {
    if spans.is_empty() {
        return false;
    }
    let mut universe: Vec<u32> = block_kids.to_vec();
    universe.extend(c.collect_direct(stmt));
    let window = c.children(stmt);
    universe.extend(window.start..window.end);
    universe
        .iter()
        .copied()
        .any(|row| subtree_hit(c, row, spans, skip, name))
}

fn subtree_hit(
    c: &Check<'_>,
    row: u32,
    spans: &[core::ops::Range<usize>],
    skip: Option<&core::ops::Range<usize>>,
    name: &str,
) -> bool {
    let span = c.byte_span(row);
    if Some(&span) == skip {
        return false;
    }
    let inside = spans
        .iter()
        .any(|s| span.start >= s.start && span.end <= s.end);
    if !inside {
        return false;
    }
    if c.kind(row) == NodeKind::Identifier && c.lexeme(c.head_token(row)) == name {
        return true;
    }
    let window = c.children(row);
    (window.start..window.end).any(|kid| subtree_hit(c, kid, spans, skip, name))
}

fn statement_box<'a>(c: &Check<'a>, stmt: u32) -> CascadeBox<'a> {
    match c.kind(stmt) {
        NodeKind::VariablesDeclaration => var_box(c, stmt),
        NodeKind::ExpressionStatement => expression_box(c, stmt),
        _ => CascadeBox::null(),
    }
}

/// `fromVariableDeclarationStatement` (:160-168): one variable with a
/// non-`await` initializer anchors a critical, `canReceive` box (a
/// `const` declaration cannot receive, :165).
fn var_box<'a>(c: &Check<'a>, stmt: u32) -> CascadeBox<'a> {
    let vars: Vec<u32> = c
        .collect_direct(stmt)
        .into_iter()
        .filter(|&kid| c.kind(kid) == NodeKind::InitializedIdentifier)
        .collect();
    let [var] = vars[..] else {
        return CascadeBox::null();
    };
    // `await` binds looser than it looks (:27-33): an awaited
    // initializer never anchors a run.
    let awaited = c
        .collect_direct(stmt)
        .into_iter()
        .chain(c.children(stmt))
        .any(|row| c.kind(row) == NodeKind::AwaitExpression);
    if awaited {
        return CascadeBox::null();
    }
    let Some(name_token) = initialized_identifier_name(c, var) else {
        return CascadeBox::null();
    };
    let const_decl =
        variables_declaration_keyword(c, stmt).is_some_and(|kw| c.lexeme(kw) == "const");
    CascadeBox {
        element: Some(c.lexeme(name_token)),
        can_join: false,
        can_receive: !const_decl,
        can_be_cascaded: false,
        critical: vec![c.byte_span(var)],
        skip: Some(byte_span_of_token(c, name_token)),
    }
}

/// The statement's expression face: `collect_direct` when the
/// statement seals a window (`VariablesDeclaration`), else the owner
/// pieces the span-only `ExpressionStatement` leaf covers — its
/// expression rows tile as flat owner siblings inside the statement's
/// byte span.
fn statement_face(c: &Check<'_>, stmt: u32) -> Vec<u32> {
    let kids = c.collect_direct(stmt);
    if !kids.is_empty() {
        return kids;
    }
    let Some(parent) = c.unit.ast.parent(stmt) else {
        return Vec::new();
    };
    let span = c.byte_span(stmt);
    c.collect_direct(parent)
        .into_iter()
        .filter(|&row| {
            row != stmt && {
                let s = c.byte_span(row);
                s.start >= span.start && s.end <= span.end
            }
        })
        .collect()
}

/// `fromExpressionStatement` (:139-158) over the statement's piece run.
fn expression_box<'a>(c: &Check<'a>, stmt: u32) -> CascadeBox<'a> {
    let kids = statement_face(c, stmt);
    if let Some(assign) = kids
        .iter()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::Assignment)
    {
        return assignment_box(c, assign);
    }
    // The outermost send is the invocation's selector group — the
    // receiver's own tear-off send precedes it.
    if let Some(pos) = kids.iter().rposition(|&kid| c.kind(kid) == NodeKind::Send) {
        return invocation_box(c, stmt, &kids, pos);
    }
    if let Some(cascade) = kids
        .iter()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::Cascade)
    {
        return cascade_box(c, cascade);
    }
    property_box(c, &kids)
}

/// `fromAssignmentExpression` (:179-202): a bare target anchors a
/// critical box; a setter over a same-file variable prefix is fully
/// cascadable (:189-194). `??=` cannot receive (:185/:192).
fn assignment_box<'a>(c: &Check<'a>, assign: u32) -> CascadeBox<'a> {
    // The shared operand split pulls the left member access's lazily
    // sealed `Send` + `DotAccess` pair back across the operator byte.
    let Some((left, right)) = slice::binary_operands_at(c, assign) else {
        return CascadeBox::null();
    };
    let Some(op) = c.field(assign, PayloadField::Operator) else {
        return CascadeBox::null();
    };
    let receives = c.lexeme(op) != "??=";
    let right_span = merged_span(c, &right);
    let Some(marker) = left
        .iter()
        .copied()
        .find(|&row| c.kind(row) == NodeKind::DotAccess)
    else {
        // The bare-target face: exactly one identifier left of the `=`.
        let [only] = left[..] else {
            return CascadeBox::null();
        };
        if c.kind(only) != NodeKind::Identifier {
            return CascadeBox::null();
        }
        return CascadeBox {
            element: Some(c.lexeme(c.head_token(only))),
            can_join: false,
            can_receive: receives,
            can_be_cascaded: false,
            critical: right_span.into_iter().collect(),
            skip: None,
        };
    };
    // The setter face: the plain-dot prefix must bind to a same-file
    // value (the upstream non-static variable gate). The marker's
    // `Operator` word is the access-operator token.
    if c.field(marker, PayloadField::Operator)
        .is_none_or(|dot| c.lexeme(dot) != ".")
    {
        return CascadeBox::null();
    }
    let Some(prefix) = left
        .iter()
        .copied()
        .find(|&row| c.kind(row) == NodeKind::Identifier)
    else {
        return CascadeBox::null();
    };
    let name = c.lexeme(c.head_token(prefix));
    if same_file::binding_in_block(c, assign, name).is_none() {
        return CascadeBox::null();
    }
    CascadeBox {
        element: Some(name),
        can_join: true,
        can_receive: receives,
        can_be_cascaded: true,
        critical: right_span.into_iter().collect(),
        skip: None,
    }
}

/// `fromMethodInvocation` (:215-229): a same-file receiver's member
/// invocation joins; the critical spans cover the selector + argument
/// list only (the target is the element, never a blocker).
fn invocation_box<'a>(c: &Check<'a>, stmt: u32, kids: &[u32], pos: usize) -> CascadeBox<'a> {
    let Some((head, Some(arguments))) = slice::send_parts(c, kids, pos) else {
        return CascadeBox::null();
    };
    // A simple receiver directly before the selector (:219).
    let Some(receiver) = slice::bare_receiver(c, kids, head) else {
        return CascadeBox::null();
    };
    let name = c.lexeme(c.head_token(receiver));
    if same_file::binding_in_block(c, stmt, name).is_none() {
        return CascadeBox::null();
    }
    // The plain-operator gate (:69-70): a null-aware access stays out.
    // The marker's `Operator` word is the access-operator token.
    if let Some(marker) = kids
        .iter()
        .copied()
        .find(|&row| c.kind(row) == NodeKind::DotAccess)
        && c.field(marker, PayloadField::Operator)
            .is_some_and(|dot| c.lexeme(dot) != ".")
    {
        return CascadeBox::null();
    }
    let start = c.byte_span(kids[head]).start;
    #[expect(
        clippy::single_range_in_vec_init,
        reason = "the box's critical spans stay a Vec for the cascade                       face's variable list; this face has exactly one"
    )]
    let critical = vec![start..c.byte_span(arguments).end];
    CascadeBox {
        element: Some(name),
        can_join: true,
        can_receive: true,
        can_be_cascaded: true,
        critical,
        skip: None,
    }
}

/// `fromCascadeExpression` (:204-213): a same-file simple target joins.
fn cascade_box<'a>(c: &Check<'a>, cascade: u32) -> CascadeBox<'a> {
    let kids = c.collect_direct(cascade);
    let sections: Vec<core::ops::Range<usize>> = kids
        .iter()
        .copied()
        .filter(|&row| c.kind(row) == NodeKind::CascadeAccess)
        .map(|row| c.byte_span(row))
        .collect();
    let Some(target) = kids
        .iter()
        .copied()
        .find(|&row| c.kind(row) != NodeKind::CascadeAccess)
    else {
        return CascadeBox::null();
    };
    if c.kind(target) != NodeKind::Identifier {
        return CascadeBox::null();
    }
    let name = c.lexeme(c.head_token(target));
    if same_file::binding_in_block(c, cascade, name).is_none() {
        return CascadeBox::null();
    }
    CascadeBox {
        element: Some(name),
        can_join: true,
        can_receive: true,
        can_be_cascaded: true,
        critical: sections,
        skip: None,
    }
}

/// `fromPropertyAccess`/`fromPrefixedIdentifier` (:150-157, :231-249):
/// a plain-dot accessor over a same-file bare receiver joins; the
/// critical span is the accessor name only.
fn property_box<'a>(c: &Check<'a>, kids: &[u32]) -> CascadeBox<'a> {
    let Some(marker) = kids
        .iter()
        .copied()
        .find(|&row| c.kind(row) == NodeKind::DotAccess)
    else {
        return CascadeBox::null();
    };
    if c.field(marker, PayloadField::Operator)
        .is_none_or(|dot| c.lexeme(dot) != ".")
    {
        return CascadeBox::null();
    }
    let Some(name_index) = slice::access_name_index(c, kids, marker) else {
        return CascadeBox::null();
    };
    let Some(receiver) = slice::bare_receiver(c, kids, name_index) else {
        return CascadeBox::null();
    };
    let name = c.lexeme(c.head_token(receiver));
    if same_file::binding_in_block(c, marker, name).is_none() {
        return CascadeBox::null();
    }
    let critical = vec![c.byte_span(kids[name_index])];
    CascadeBox {
        element: Some(name),
        can_join: true,
        can_receive: true,
        can_be_cascaded: true,
        critical,
        skip: None,
    }
}

fn merged_span(c: &Check<'_>, rows: &[u32]) -> Option<core::ops::Range<usize>> {
    let first = rows.first()?;
    let mut span = c.byte_span(*first);
    for row in rows {
        let other = c.byte_span(*row);
        span.start = span.start.min(other.start);
        span.end = span.end.max(other.end);
    }
    Some(span)
}

#[cfg(test)]
mod tests {
    use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

    fn run(source: &str) -> Vec<crate::engine::Lint> {
        Fixture::parse(source).run(&one_rule("cascade_invocations"))
    }

    /// `test_assignmentStatements` / `test_cascadeStatements` — a run
    /// of assignments over one declaration reports from the second
    /// statement through the last.
    #[test]
    fn assignment_run_over_a_declaration_reports() {
        let source = "class A {\n  int y = 0;\n  int z = 0;\n}\nvoid f() {\n  var x = A();\n  x.y = 1;\n  x.z = 2;\n}\n";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("cascade_invocations"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, "Unnecessary duplication of receiver.");
        // The report starts at the second statement and ends with the
        // last (:370-373) — the declaration anchor stays out.
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(source, &f.tokens, "x", 1))
        );
        assert_eq!(
            lints[0].range.end,
            utf16_at(&f, tok(source, &f.tokens, ";", 4)) + 1
        );
    }

    /// `test_methodInvocations` — a chain of same-receiver invocations
    /// joins; a different receiver breaks the run.
    #[test]
    fn invocation_run_joins_and_different_receiver_breaks() {
        let source = "class A {\n  void m1() {}\n  void m2() {}\n}\nvoid f(A a) {\n  a.m1();\n  a.m2();\n}\n";
        assert_eq!(run(source).len(), 1);

        let source = "class A {\n  void m1() {}\n}\nvoid f(A a, A b) {\n  a.m1();\n  b.m1();\n}\n";
        assert!(run(source).is_empty());
    }

    /// The critical gate: a right side referencing the anchor's element
    /// blocks the join (`x.y = probe(x);` — the assigned value depends
    /// on the receiver), and a `const` declaration cannot receive.
    #[test]
    fn critical_references_block_the_join() {
        let source = "class A {\n  int y = 0;\n  int z = 0;\n}\nbool probe(A a) => true;\nvoid f() {\n  var x = A();\n  x.y = probe(x);\n  x.z = 1;\n}\n";
        assert!(run(source).is_empty());

        let source = "class A {\n  int y = 0;\n}\nvoid f() {\n  const x = A();\n  x.y = 1;\n}\n";
        assert!(run(source).is_empty());
    }
}
