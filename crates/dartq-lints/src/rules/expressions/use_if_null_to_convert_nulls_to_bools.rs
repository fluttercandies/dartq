use super::{Check, Group, NodeKind, RuleSpec, same_file, slice, value_run_end};
use crate::rules::type_face;

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/use_if_null_to_convert_nulls_to_bools.dart`
/// — `x == true` (:50-57) or `x != false` (:59-64) where `x` is a
/// *nullable* `bool` reports at the comparison (the rewrite `x ?? true`
/// / `x ?? false` keeps the semantics only on that type). The written-
/// type face is the same-file channel: the left operand is a bare name
/// bound to a written `bool?` (the `isDartCoreBool` + `isNullable`
/// face — a non-nullable `bool` and an inferred or untyped name stay
/// silent exactly like the official). Approximations (documented,
/// under-report): member-access and invocation operands, and
/// parenthesized left operands, stay silent.
/// Message: `diagnostic.g.dart` `useIfNullToConvertNullsToBools`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "use_if_null_to_convert_nulls_to_bools",
    group: Group::Standalone,
    problem: "Use an if-null operator to convert a 'null' to a 'bool'.",
    correction: "Try using an if-null operator.",
    kinds: &[NodeKind::BinaryExpression],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    let Some(&op) = c.payload(node).first() else {
        return;
    };
    let wanted = match c.lexeme(op) {
        "==" => "true",
        "!=" => "false",
        _ => return,
    };
    let Some((left, right)) = slice::binary_operands(c) else {
        return;
    };
    if !left_is_nullable_bool(c, node, &left) || !right_is_bool_literal(c, &right, wanted) {
        return;
    }
    // The official reports at the comparison node (:57/:63); the
    // container row opens at the operator's tile anchor, so the extent
    // runs from the left operand run's first token to the right run's
    // last value row.
    let Some(start) = slice::operand_utf16_start(c, &left, op) else {
        return;
    };
    let end = value_run_end(c, &right, node);
    c.report(start..end, &[]);
}

/// The `isNullableBool` face (:44-47): a bare-name operand bound to a
/// written `bool?`.
fn left_is_nullable_bool(c: &Check<'_>, node: u32, rows: &[u32]) -> bool {
    let run = slice::access_run(c, rows);
    let [only] = run[..] else {
        return false;
    };
    if c.kind(only) != NodeKind::Identifier {
        return false;
    }
    let name = c.lexeme(c.head_token(only));
    let Some((_, Some(ty))) = same_file::binding(c, node, name) else {
        return false;
    };
    let face = type_face(c, ty);
    face.name == Some("bool") && face.nullable
}

/// The literal half: exactly a `true` (for `==`) or `false` (for `!=`)
/// literal (:55-56/:61-62 — the other pairing never matches).
fn right_is_bool_literal(c: &Check<'_>, rows: &[u32], wanted: &str) -> bool {
    let run = slice::access_run(c, rows);
    let [only] = run[..] else {
        return false;
    };
    c.kind(only) == NodeKind::LiteralBool && c.lexeme(c.head_token(only)) == wanted
}

#[cfg(test)]
mod tests {
    use crate::testsupport::{Fixture, one_rule};

    fn run(source: &str) -> Vec<crate::engine::Lint> {
        Fixture::parse(source).run(&one_rule("use_if_null_to_convert_nulls_to_bools"))
    }

    /// The nullable-`bool` faces report at the comparison; a
    /// non-nullable `bool`, a non-`bool` type, and the flipped literal
    /// pairings stay silent.
    #[test]
    fn nullable_bool_comparison_faces() {
        let lints = run("void f(bool? flag) {\n  if (flag == true) {}\n}\n");
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Use an if-null operator to convert a 'null' to a 'bool'."
        );

        assert_eq!(
            run("void f(bool? flag) {\n  if (flag != false) {}\n}\n").len(),
            1
        );

        for source in [
            // The non-nullable gate (:47 — the rewrite changes nothing).
            "void f(bool flag) {\n  if (flag == true) {}\n}\n",
            // A non-`bool` type never matches (:46).
            "void f(int? n) {\n  if (n == true) {}\n}\n",
            // The flipped pairings are not the shape (:55-56, :61-62).
            "void f(bool? flag) {\n  if (flag == false) {}\n}\n",
            "void f(bool? flag) {\n  if (flag != true) {}\n}\n",
        ] {
            assert!(run(source).is_empty(), "{source}");
        }
    }
}
