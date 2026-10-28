use super::{Check, NodeKind, PayloadField, RuleSpec, same_file, slice, value_run_end};
use crate::rules::in_const_context;

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/use_is_even_rather_than_modulo.dart`
/// — `x % 2 == 0` (or `== 1`) over `int` operands reports at the
/// comparison with `isEven`/`isOdd` (:37-81): the operator must be
/// `==` (:40), the node must not sit in a constant context (:41), the
/// right side is the literal `0`/`1` (:48, :67-68), the left side is a
/// `%` binary over the literal `2` (:54-60) with `int` operands
/// (:48-50, :63-65), and a `const` constructor's assert initializer is
/// exempt (:69-78 — `isEven` is not constant-evaluable there).
///
/// dartq faces over the production tiling: the outer `==`'s left run
/// roots the `%` container, whose own operand rows split at the
/// operator token; the `int` face is the same-file written-type channel
/// over the modulo's bare-name operand (the right literal is `int` by
/// syntax). Approximations (documented, under-report): member-access
/// and compound modulo operands (`v.length % 2`, `a.b % 2`), a
/// non-bare-name left operand, and a hexadecimal `0x2` stay silent.
/// Message: `diagnostic.g.dart` `useIsEvenRatherThanModulo`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "use_is_even_rather_than_modulo",
    &[NodeKind::BinaryExpression],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // The operator gate (:40).
    let Some(op) = c.field(node, PayloadField::Operator) else {
        return;
    };
    if c.lexeme(op) != "==" {
        return;
    }
    // The constant-context gate (:41).
    if in_const_context(c) {
        return;
    }

    // The `const` constructor's assert initializer (:69-78).
    if in_const_constructor_assert(c) {
        return;
    }
    let Some((left, right)) = slice::binary_operands(c) else {
        return;
    };
    // The literal half (:48, :67-68).
    let right_run = slice::access_run(c, &right);
    let [literal] = right_run[..] else {
        return;
    };
    if c.kind(literal) != NodeKind::LiteralInt {
        return;
    }
    let name = match c.lexeme(c.head_token(literal)) {
        "0" => "isEven",
        "1" => "isOdd",
        _ => return,
    };
    // The modulo half (:54-65).
    let Some(modulo) = modulo_root(c, &left) else {
        return;
    };
    let Some((operand, divisor)) = modulo_operands(c, modulo) else {
        return;
    };
    if c.lexeme(c.head_token(divisor)) != "2" {
        return;
    }
    let operand_name = c.lexeme(c.head_token(operand));
    // The `int` face (:48-50): the written type of the bare-name
    // operand (a `int?` read answers like the official — nullability
    // does not change `isDartCoreInt`).
    let Some((_, Some(ty))) = same_file::binding(c, node, operand_name) else {
        return;
    };
    if type_face_name(c, ty) != Some("int") {
        return;
    }
    // The official reports at the comparison with the member name
    // (:80); the container row opens at the `==`'s tile anchor, so the
    // extent runs from the left operand run's first token to the right
    // run's last value row.
    let Some(start) = slice::operand_utf16_start(c, &left, op) else {
        return;
    };
    let end = value_run_end(c, &right, node);
    c.report(start..end, &[name]);
}

/// The `%` container rooting the left run: the binary row whose span
/// contains every other left row in token space (the adopted
/// container's row span covers its operand tokens; a nested chain
/// roots at the outermost container, whose span covers the rest).
fn modulo_root(c: &Check<'_>, left: &[u32]) -> Option<u32> {
    let root = left.iter().copied().rev().find(|&row| {
        c.kind(row) == NodeKind::BinaryExpression
            && left.iter().all(|&other| {
                other == row || {
                    let inner = c.unit.ast.token_range(other);
                    let outer = c.unit.ast.token_range(row);
                    inner.start >= outer.start && inner.end <= outer.end
                }
            })
    })?;
    let op = c.field(root, PayloadField::Operator)?;
    (c.lexeme(op) == "%").then_some(root)
}

/// The modulo container's `(operand send, divisor)` rows: the window
/// rows split at the operator token, exactly one per side — the
/// operand side is the bare-name send group's Send (its head carries
/// the operand identifier; the identifier pieces tile in the owner
/// window), and a member or compound operand carries extra rows and
/// stays silent (the documented face).
fn modulo_operands(c: &Check<'_>, modulo: u32) -> Option<(u32, u32)> {
    let op = c.field(modulo, PayloadField::Operator)?;
    let mut rows: Vec<u32> = c.collect_direct(modulo);
    let window = c.children(modulo);
    rows.extend(window.start..window.end);
    rows.sort_unstable();
    rows.dedup();
    let split = rows.partition_point(|&row| c.unit.ast.token_range(row).start <= op);
    if split != 1 || rows.len() - split != 1 {
        return None;
    }
    let (operand, divisor) = (rows[0], rows[split]);
    if c.kind(operand) != NodeKind::Send || c.kind(divisor) != NodeKind::LiteralInt {
        return None;
    }
    Some((operand, divisor))
}

/// The written head name of a type row (`bool`-face shape).
fn type_face_name<'a>(c: &'a Check<'a>, ty: u32) -> Option<&'a str> {
    (c.kind(ty) == NodeKind::Type).then(|| c.lexeme(c.head_token(ty)))
}

/// The assert-initializer exemption (:69-78): a `%`-equality inside an
/// assert of a `const` constructor stays silent.
fn in_const_constructor_assert(c: &Check<'_>) -> bool {
    let mut seen_assert = false;
    for &ancestor in c.path.iter().rev().skip(1) {
        match c.kind(ancestor) {
            NodeKind::Assert => seen_assert = true,
            NodeKind::Constructor | NodeKind::Factory => {
                return seen_assert && c.lexeme(c.head_token(ancestor)) == "const";
            }
            NodeKind::Method | NodeKind::TopLevelMethod | NodeKind::CompilationUnit => {
                return false;
            }
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use crate::testsupport::{Fixture, one_rule};

    fn run(source: &str) -> Vec<crate::engine::Lint> {
        Fixture::parse(source).run(&one_rule("use_is_even_rather_than_modulo"))
    }

    /// `x % 2 == 0/1` over an `int` name reports with `isEven`/`isOdd`;
    /// other values, non-`int` and member-access operands, and the
    /// constant context stay silent.
    #[test]
    fn modulo_two_equality_faces() {
        let lints = run("void f(int v) {\n  var a = v % 2 == 0;\n  var b = v % 2 == 1;\n}\n");
        assert_eq!(lints.len(), 2, "{lints:?}");
        assert_eq!(lints[0].message, "Use 'isEven' rather than '% 2'.");
        assert_eq!(lints[1].message, "Use 'isOdd' rather than '% 2'.");

        for source in [
            // A value other than 0/1 is not the shape (:67-68).
            "void f(int v) {\n  var a = v % 2 == 2;\n}\n",
            // A non-`int` operand stays silent (:48-50).
            "void f(String v) {\n  var a = v.length % 2 == 0;\n}\n",
            // The constant-context gate (:41).
            "void f() {\n  const int v = 4;\n  const a = v % 2 == 0;\n}\n",
        ] {
            assert!(run(source).is_empty(), "{source}");
        }
    }
}
