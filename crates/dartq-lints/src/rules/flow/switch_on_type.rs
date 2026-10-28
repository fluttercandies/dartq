use dartq_ast::NodeKind;

use super::{Check, PayloadField, RuleSpec};
use crate::rules::NONE_TOKEN;

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/switch_on_type.dart` — a switch
/// statement/expression whose scrutinee's static type is assignable
/// to `Type` reports once at the scrutinee (:53-60, :71-121): the
/// recursion faces are the then/else arms of a conditional, both
/// operands of a `+` (string building), a nested switch expression's
/// case values, and a string interpolation's segments; the leaf faces
/// are a `.runtimeType` tail, a bare `Type`-written name, a type
/// parameter, and a type literal — a `.toString()` call walks its
/// target (:111-112).
///
/// Gate: the official registration bails without the patterns feature
/// (:34) — the same [`crate::engine::Features::patterns`] bit.
///
/// Faces over the syntax substrate: the scrutinee is the piece run of
/// the switch head (the adopted-begin tiling leaks the expression
/// rows before the condition container; the container's window rows
/// follow it). The written-type face reads same-file bindings
/// (`Type t` reports, `String s` and inferred locals stay silent, the
/// nullability-sensitive `isAssignableTo` posture); the
/// implicit-`this` `runtimeType` face is name-level (a same-file
/// override returning a non-`Type` over-reports); the type-literal
/// face reuses the `type_literal_in_constant_pattern` channel
/// (same-file type or an unshadowed built-in; a prefixed spelling
/// under-reports). The string-interpolation faces (a `$t` segment)
/// are the documented under-report — interpolation segments produce
/// no AST rows (the token-layer carry). Message:
/// `diagnostic.g.dart` `switchOnType`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "switch_on_type",
    &[NodeKind::SwitchStatement, NodeKind::SwitchExpression],
    check,
);

fn check(c: &mut Check<'_>) {
    // The registration bail (:34).
    if !c.unit.features.patterns {
        return;
    }
    let Some(rows) = scrutinee_rows(c) else {
        return;
    };
    if type_scrutinee(c, &rows) {
        // One report per switch, at the whole scrutinee run
        // (`reportAtNode(node.expression)`).
        let start = c.utf16_span(rows[0]).start;
        let end = c.utf16_span(rows[rows.len() - 1]).end;
        c.report(start..end, &[]);
    }
}

/// The scrutinee piece run: the rows before the condition container
/// plus the container's window rows (the statement face), or the rows
/// before the case block (the switch-expression face). A `case`
/// clause (`switch (x case ...)`) is not the lint's face.
fn scrutinee_rows(c: &Check<'_>) -> Option<Vec<u32>> {
    let node = c.node();
    let kids = c.collect_direct(node);
    match c.kind(node) {
        NodeKind::SwitchStatement => {
            let position = kids
                .iter()
                .position(|&kid| c.kind(kid) == NodeKind::ParenthesizedCondition)?;
            let container = kids[position];
            if c.field(container, PayloadField::CaseKeyword)
                .is_some_and(|case| case != NONE_TOKEN)
            {
                return None;
            }
            let mut rows: Vec<u32> = kids[..position].to_vec();
            rows.extend(c.children(container));
            Some(rows)
        }
        NodeKind::SwitchExpression => {
            let position = kids
                .iter()
                .position(|&kid| c.kind(kid) == NodeKind::SwitchExpressionBlock)?;
            Some(kids[..position].to_vec())
        }
        _ => None,
    }
}

/// The official `_processExpression` walk (:71-121): the first
/// `Type`-valued leaf reports once.
fn type_scrutinee(c: &Check<'_>, rows: &[u32]) -> bool {
    let Some(position) = rows
        .iter()
        .rposition(|&row| c.kind(row) == NodeKind::SwitchExpression)
    else {
        return walk(c, rows);
    };
    // A nested switch expression: its case values walk (:86-93). The
    // value rows follow the arrow payload token.
    let block = c
        .collect_direct(rows[position])
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::SwitchExpressionBlock);
    let Some(block) = block else {
        return false;
    };
    for case in c.collect_direct(block) {
        if c.kind(case) != NodeKind::SwitchExpressionCase {
            continue;
        }
        let Some(arrow) = c.field(case, PayloadField::Arrow) else {
            continue;
        };
        if arrow == NONE_TOKEN {
            continue;
        }
        let arrow_byte = token_byte(c, arrow);
        let value_rows: Vec<u32> = c
            .children(case)
            .filter(|&row| c.byte_span(row).start > arrow_byte)
            .collect();
        if type_scrutinee(c, &value_rows) {
            return true;
        }
    }
    false
}

fn walk(c: &Check<'_>, rows: &[u32]) -> bool {
    // The conditional face: the arms decide, the condition is ignored
    // (:78-84).
    if let Some(position) = rows
        .iter()
        .rposition(|&row| c.kind(row) == NodeKind::Conditional)
    {
        let arms: Vec<u32> = c.children(rows[position]).collect();
        return walk(c, &arms);
    }
    // The `+` concatenation face: both operand runs walk (:95-104).
    if let Some(position) = rows.iter().rposition(|&row| {
        c.kind(row) == NodeKind::BinaryExpression
            && c.lexeme(c.field(row, PayloadField::Operator).unwrap_or(NONE_TOKEN)) == "+"
    }) {
        let operator = c
            .field(rows[position], PayloadField::Operator)
            .unwrap_or(NONE_TOKEN);
        let operator_byte = token_byte(c, operator);
        let split = rows.partition_point(|&row| c.byte_span(row).end <= operator_byte);
        // The container row spans the operator, so it falls in neither
        // operand side: the right walk takes only the rows beginning
        // after the operator byte. Each half is therefore strictly
        // smaller than the walked run (the triggering row is in the
        // run but in neither half) — structural progress, no re-walk
        // of the same slice.
        let right: Vec<u32> = rows[split..]
            .iter()
            .copied()
            .filter(|&row| c.byte_span(row).start > operator_byte)
            .collect();
        return walk(c, &rows[..split]) || walk(c, &right);
    }
    // The invocation face: a `.toString()` selector walks its target
    // (:111-112); a bare name is the identifier face itself. A
    // `NoArguments` send is a tear-off, not the official
    // `MethodInvocation` face — the outermost selector's property
    // type decides (the static-type read of :97-107), i.e. the
    // access-chain face below (`n.runtimeType.toString` is silent,
    // `test_runtimeTypeToString_noCall`).
    if let Some(position) = rows.iter().rposition(|&row| c.kind(row) == NodeKind::Send) {
        let Some(mut head) = position.checked_sub(1) else {
            return false;
        };
        match c.kind(rows[head]) {
            NodeKind::Arguments => head -= 1,
            NodeKind::NoArguments => return access_chain_face(c, rows),
            _ => return false,
        }
        loop {
            match c.kind(rows[head]) {
                NodeKind::TypeArguments | NodeKind::NoTypeArguments => head -= 1,
                NodeKind::Identifier | NodeKind::DotAccess => break,
                _ => return false,
            }
        }
        let lexeme = c.lexeme(c.head_token(rows[head]));
        if lexeme == "toString" {
            return head > 0 && walk(c, &rows[..head]);
        }
        // A bare invocation has no real target (:111 — `realTarget?`).
        if c.kind(rows[head]) != NodeKind::Identifier {
            return false;
        }
        return name_is_type(c, rows[head], lexeme);
    }
    // The access-chain face: only the outermost selector names the
    // expression's type (`n.runtimeType` reports,
    // `n.runtimeType.toString` does not).
    access_chain_face(c, rows)
}

/// The static-type read's access-chain posture (:97-107): the
/// outermost selector names the expression's type.
fn access_chain_face(c: &Check<'_>, rows: &[u32]) -> bool {
    if let Some(&tail) = rows
        .iter()
        .rev()
        .find(|&row| c.kind(*row) == NodeKind::DotAccess)
    {
        return c.lexeme(c.head_token(tail)) == "runtimeType";
    }
    // The bare-name face: the run's root identifier.
    if let Some(&name) = rows
        .iter()
        .rev()
        .find(|&row| c.kind(*row) == NodeKind::Identifier)
    {
        return name_is_type(c, name, c.lexeme(c.head_token(name)));
    }
    false
}

/// The `Type`-valued name faces: a written `Type` binding first (the
/// shadow-correct same-file walk), then the implicit-`this`
/// `runtimeType` face, a type parameter, and the type-literal
/// channel.
fn name_is_type(c: &Check<'_>, use_row: u32, lexeme: &str) -> bool {
    if crate::rules::expressions::same_file::binding(c, use_row, lexeme).is_some() {
        return crate::rules::written_type_face(c, use_row, lexeme)
            .is_some_and(|face| face.name == Some("Type") && !face.nullable);
    }
    // `runtimeType` on implicit `this` (:94-99 — the corpus
    // `runtimeType`/`this.runtimeType` faces); a same-file override
    // returning a non-`Type` over-reports.
    if lexeme == "runtimeType" {
        return true;
    }
    if is_enclosing_type_parameter(c, lexeme) {
        return true;
    }
    crate::rules::expressions::same_file::declares_type(c, lexeme)
        || super::type_literal_in_constant_pattern::is_builtin_type_name(lexeme)
}

/// Whether the name is a type parameter of an enclosing declaration
/// (`switch (T)`, :336-343 — the type-literal face).
fn is_enclosing_type_parameter(c: &Check<'_>, name: &str) -> bool {
    for &ancestor in c.path.iter().rev().skip(1) {
        let variables = c
            .collect_direct(ancestor)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::TypeVariables);
        let Some(variables) = variables else {
            continue;
        };
        for variable in c.collect_direct(variables) {
            if c.kind(variable) == NodeKind::TypeVariable
                && c.lexeme(c.head_token(variable)) == name
            {
                return true;
            }
        }
    }
    false
}

fn token_byte(c: &Check<'_>, token: u32) -> usize {
    c.unit
        .tokens
        .get(token as usize)
        .map_or(usize::MAX, |token| token.byte_offset as usize)
}

#[cfg(test)]
mod tests {
    use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

    fn run(source: &str) -> Vec<crate::engine::Lint> {
        Fixture::parse(source).run(&one_rule("switch_on_type"))
    }

    /// `test_variableType` — a `Type`-written parameter reports.
    #[test]
    fn type_written_parameter_reports() {
        let source = "void f(Type t) {\n  switch (t) {\n    case const (int):\n      break;\n    default:\n      break;\n  }\n}\n";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("switch_on_type"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, "Avoid switch statements on a 'Type'.");
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try using pattern matching on a variable instead.")
        );
        assert_eq!(
            // The report pins the scrutinee expression
            // (`reportAtNode(node.expression)`) — the `t` inside the
            // switch head, not the parameter spelling.
            lints[0].range.start,
            utf16_at(&f, tok(source, &f.tokens, "t", 1))
        );
    }

    /// `test_runtimeType` / `test_insideClass_implicitThis` /
    /// `test_insideClass_withThis` — the `.runtimeType` faces.
    #[test]
    fn runtime_type_faces_report() {
        assert_eq!(
            run("void f(num i) {\n  switch (i.runtimeType) {\n    case const (int):\n      break;\n    default:\n      break;\n  }\n}\n").len(),
            1
        );
        assert_eq!(
            run("class A {\n  void m() {\n    switch (runtimeType) {\n      case const (A):\n        break;\n      default:\n        break;\n    }\n  }\n}\n").len(),
            1
        );
        assert_eq!(
            run("class A {\n  void m() {\n    switch (this.runtimeType) {\n      case const (A):\n        break;\n      default:\n        break;\n    }\n  }\n}\n").len(),
            1
        );
    }

    /// `test_runtimeTypeToString` / `test_stringAddition` — the
    /// `.toString()` walk.
    #[test]
    fn to_string_calls_walk_their_target() {
        assert_eq!(
            run("void f(num n) {\n  switch (n.runtimeType.toString()) {\n    case 'int':\n      break;\n    default:\n      break;\n  }\n}\n").len(),
            1
        );
        assert_eq!(
            run("void f(Type t) {\n  switch ('type: ' + t.toString()) {\n    case 'type: int':\n      break;\n    default:\n      break;\n  }\n}\n").len(),
            1
        );
    }

    /// The `+` concatenation face walks both operand runs and
    /// terminates: the container row spans the operator and belongs to
    /// neither operand side (regression pin — the walked run used to
    /// re-enter its own slice and overflow the stack).
    #[test]
    fn plus_operand_runs_walk_and_terminate() {
        // The left operand is the `Type` leaf.
        assert_eq!(
            run("void f(Type t) {\n  switch (t.toString() + '') {\n    case 'type: int':\n      break;\n    default:\n      break;\n  }\n}\n")
                .len(),
            1
        );
        // The right operand is the `Type` leaf (the crash shape: the
        // post-operator run held the container row).
        assert_eq!(
            run("void f(Type t) {\n  switch ('type: ' + t.toString()) {\n    case 'type: int':\n      break;\n    default:\n      break;\n  }\n}\n")
                .len(),
            1
        );
        // Neither operand is `Type`-valued — silent.
        assert!(
            run("void f(num n) {\n  switch (n.toString() + '') {\n    case 'int':\n      break;\n    default:\n      break;\n  }\n}\n")
                .is_empty()
        );
    }

    /// `test_runtimeTypeToString_noCall` — the tear-off's outermost
    /// selector is `toString`, not `Type`.
    #[test]
    fn tear_off_access_chain_is_silent() {
        assert!(
            run("void f(num n) {\n  switch (n.runtimeType.toString) {\n    case function:\n      break;\n    default:\n      break;\n  }\n}\n\nvoid function() {}\n")
                .is_empty()
        );
    }

    /// `test_other` / `test_dynamic` / `test_variable_typeToString` —
    /// non-`Type` scrutinees stay silent.
    #[test]
    fn non_type_scrutinees_are_silent() {
        assert!(
            run("void f(num i) {\n  switch (i) {\n    case int _:\n      break;\n    case double _:\n      break;\n  }\n}\n")
                .is_empty()
        );
        assert!(run("void foo(dynamic a) {\n  switch (a) {}\n}\n").is_empty());
        assert!(
            run("void f(Object? o) {\n  final type = o.runtimeType.toString();\n  switch (type) {\n    case 'int':\n      break;\n    default:\n      break;\n  }\n}\n")
                .is_empty()
        );
    }

    /// `test_functionToString` / `test_functionToString_prefixed` —
    /// bare and prefixed `toString` invocations have no `Type`
    /// target.
    #[test]
    fn bare_to_string_is_silent() {
        assert!(
            run("void f() {\n  switch (toString()) {\n    case 'int':\n      break;\n    default:\n      break;\n  }\n}\n\nString toString() => '';\n")
                .is_empty()
        );
        assert!(
            run("import '' as self;\n\nvoid f() {\n  switch (self.toString()) {\n    case 'int':\n      break;\n    default:\n      break;\n  }\n}\n\nString toString() => '';\n")
                .is_empty()
        );
    }

    /// `test_conditionalBoth` — the arms walk.
    #[test]
    fn conditional_arm_reports() {
        assert_eq!(
            run("void f(Type t) {\n  switch (1 == 1 ? t : 'other') {\n    case 'type: int':\n      break;\n    default:\n      break;\n  }\n}\n")
                .len(),
            1
        );
    }

    /// `test_nestedSwitch` — the nested switch expression's case
    /// values walk.
    #[test]
    fn nested_switch_expression_reports() {
        assert_eq!(
            run("void f(Type t, Object o) {\n  switch (switch (o) { _ => t }) {\n    case const (int):\n      break;\n    default:\n      break;\n  }\n}\n")
                .len(),
            1
        );
    }

    /// `test_typeParameter` — a bare type parameter is a type
    /// literal.
    #[test]
    fn type_parameter_reports() {
        assert_eq!(
            run("void f<T>() {\n  switch (T) {\n    case const (int):\n      break;\n    default:\n      break;\n  }\n}\n")
                .len(),
            1
        );
    }

    /// The switch-expression face reports on its scrutinee.
    #[test]
    fn switch_expression_scrutinee_reports() {
        assert_eq!(
            run("void f(Type t) {\n  (switch (t) { _ => null });\n}\n").len(),
            1
        );
    }

    /// The gate (:34): with the feature off the rule is silent.
    #[test]
    fn patterns_feature_off_is_silent() {
        let mut f = Fixture::parse(
            "void f(Type t) {\n  switch (t) {\n    case const (int):\n      break;\n    default:\n      break;\n  }\n}\n",
        );
        f.features.patterns = false;
        assert!(f.run(&one_rule("switch_on_type")).is_empty());
    }

    /// A `String`-written name shadows the built-in reading
    /// (`switch (int)` with a local `int` binding stays consistent
    /// with the official element read).
    #[test]
    fn value_binding_shadows_the_type_reading() {
        assert!(
            run("void f() {\n  var int = 3;\n  switch (int) {\n    case 3:\n      break;\n    default:\n      break;\n  }\n}\n")
                .is_empty()
        );
    }

    /// The report pins the scrutinee run (`[!i.runtimeType!]`).
    #[test]
    fn report_pins_the_scrutinee_run() {
        let source = "void f(num i) {\n  switch (i.runtimeType) {\n    case const (int):\n      break;\n    default:\n      break;\n  }\n}\n";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("switch_on_type"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(source, &f.tokens, "i", 1))
        );
    }
}
