//! Pins for ticket #3860: the const-eval prefix application's span.
//!
//! A failing prefix application anchors at the whole `PrefixExpression`
//! node — the operator token through the operand's end — because upstream
//! catches the throw at `InvalidConstant.forEntity(entity: node)`
//! (upstream Dart SDK source, pkg/analyzer/lib/src/dart/constant/
//! evaluation.dart). The capture used to hand the wrapper the *operand's*
//! span instead (`unary_prefix` was told the operator kind but never its
//! token range), so a `!`/`-`/`~` fault reported at the operand alone.
//!
//! Oracle rows (the pinned `dart analyze --format=machine`, Dart 3.13.7)
//! over
//!
//! ```dart
//! const dynamic D = null;
//! const C1 = !D;
//! const C2 = -D;
//! const C3 = ~D;
//! ```
//!
//! * `ERROR|COMPILE_TIME_ERROR|CONST_EVAL_THROWS_EXCEPTION|…|2|12|2` — `!D`
//! * `ERROR|COMPILE_TIME_ERROR|CONST_EVAL_THROWS_EXCEPTION|…|3|12|2` — `-D`
//! * `ERROR|COMPILE_TIME_ERROR|CONST_EVAL_THROWS_EXCEPTION|…|4|12|2` — `~D`
//!
//! One missed or spurious row taints the whole triple, so the three shapes
//! are pinned individually, whitespace between operator and operand is
//! pinned too (`const A = -  D;` → `…|2|11|4`, one row over the whole
//! `-  D`), and both control directions run in the same file: a legal
//! unary constant emits no row at all, and a non-constancy keeps the
//! failing **leaf** (operator faults widen; `Verdict::NotConstant` never
//! does).

use super::*;
use dartq_source::StringTable;

/// The three fault shapes of the ticket, one per line after the `D`
/// declaration so the oracle's line numbers (2, 3, 4) line up.
const SOURCE: &str = "const dynamic D = null;\nconst C1 = !D;\nconst C2 = -D;\nconst C3 = ~D;\n";

/// The spans of one prefix application written in `source` as `expr`
/// (`"!D"`, `"-  D"`): the one-character operator token, the operand
/// suffix, and the whole expression the oracle anchors on.
fn spans_of(source: &str, expr: &str) -> (SourceRange, SourceRange, SourceRange) {
    let start = u32::try_from(
        source
            .find(expr)
            .expect("the fixture spells the expression"),
    )
    .expect("fixture offsets fit u32");
    let expr_len = u32::try_from(expr.len()).expect("fixture length fits u32");
    let operand_len = u32::try_from(
        expr.trim_start_matches(|c: char| !c.is_alphanumeric())
            .len(),
    )
    .expect("operand length fits u32");
    (
        SourceRange {
            offset: start,
            length: 1,
        },
        SourceRange {
            offset: start + expr_len - operand_len,
            length: operand_len,
        },
        SourceRange {
            offset: start,
            length: expr_len,
        },
    )
}

/// The source text a span covers — the guard that keeps the computed
/// offsets honest (`-  D` must slice back to `-  D`, not to `-D`).
fn text_at<'s>(source: &'s str, span: SourceRange) -> &'s str {
    &source[usize::try_from(span.offset).unwrap()..usize::try_from(span.end()).unwrap()]
}

/// One-based `(line, column)` of a UTF-16 offset — the coordinate pair the
/// oracle's machine format prints, so the pins can assert the oracle's own
/// numbers instead of local offsets.
fn line_col(source: &str, offset: u32) -> (u32, u32) {
    let offset = usize::try_from(offset).unwrap();
    let before = &source[..offset];
    let line = u32::try_from(before.matches('\n').count()).unwrap() + 1;
    let column = u32::try_from(offset - before.rfind('\n').map_or(0, |nl| nl + 1)).unwrap() + 1;
    (line, column)
}

/// A tiny fixed world: `D` is the const `dynamic` null of the ticket, `b`
/// is provably non-const (the leaf-anchor control), every other name is a
/// valid constant of unknown value.
struct World {
    strings: StringTable,
}

impl World {
    fn id(&self, name: &str) -> StringId {
        self.strings.intern(name)
    }
}

impl ConstLookup for World {
    fn ident(&self, name: StringId) -> IdentVerdict {
        if name == self.id("D") {
            IdentVerdict::Value(ConstantValue::Null)
        } else if name == self.id("b") {
            IdentVerdict::NotConstant
        } else {
            IdentVerdict::Constant
        }
    }

    fn member(&self, _: &ConstantValue, _: StringId) -> Verdict {
        Verdict::Constant
    }

    fn is_identical(&self, name: StringId) -> bool {
        name == self.id("identical")
    }
}

fn w() -> World {
    World {
        strings: StringTable::new(),
    }
}

/// Captures one `!ident`-shaped initializer in the parser's event order: the
/// operand's node lands first (the listener's `handle_unary_prefix_expression`
/// fires after `parse_precedence` completes the operand), then the operator
/// latch, then the window's close applies the prefix.
fn capture_identifier(
    world: &World,
    name: &str,
    operand: SourceRange,
    op: UnOp,
    op_span: SourceRange,
) -> Node {
    let mut capture = ConstCapture::new(world.id("identical"));
    capture.open_initializer();
    capture.identifier(world.id(name), operand);
    capture.unary_prefix(op, op_span);
    capture
        .close_initializer()
        .expect("one root closes out of the initializer window")
}

/// Captures one `!literal`-shaped initializer — the negative control (a legal
/// unary constant, which must produce no fault at all).
fn capture_literal(
    world: &World,
    value: ConstantValue,
    operand: SourceRange,
    op: UnOp,
    op_span: SourceRange,
) -> Node {
    let mut capture = ConstCapture::new(world.id("identical"));
    capture.open_initializer();
    capture.literal(operand, value);
    capture.unary_prefix(op, op_span);
    capture
        .close_initializer()
        .expect("one root closes out of the initializer window")
}

// —— (1) the three fault shapes anchor at the whole application ——————

/// The oracle's own rows for the fixture: `!D` / `-D` / `~D` each throw
/// (the `assert*OrNull` gates admit the null, the state's inherited
/// `logicalNot`/`negated`/`bitNot` then throws — value.dart), and the
/// caught throw anchors at the *prefix node*: line 2/3/4, column 12,
/// length 2.
#[test]
fn ticket_3860_prefix_fault_spans_the_whole_application() {
    let world = w();
    for (op, expr, line) in [
        (UnOp::Not, "!D", 2),
        (UnOp::Neg, "-D", 3),
        (UnOp::BitNot, "~D", 4),
    ] {
        let (op_span, operand, whole) = spans_of(SOURCE, expr);
        assert_eq!(text_at(SOURCE, whole), expr, "spans trace back to source");

        let node = capture_identifier(&world, "D", operand, op, op_span);
        assert_eq!(node.span, whole, "{expr} wraps operator through operand");
        assert_eq!(
            eval(&node, &world),
            Verdict::Fault(ConstFault::ThrowsException, whole),
            "{expr} throws at the whole prefix application"
        );
        assert_eq!(
            (line_col(SOURCE, whole.offset), whole.length),
            ((line, 12), 2),
            "the oracle row for {expr} is {line}|12|2"
        );
    }
}

// —— (2) whitespace between operator and operand stays inside ————————

/// `const A = -  D;` reports one row over the whole `-  D` (oracle:
/// `…|2|11|4`). The operator token's own range is what makes this exact —
/// deriving it as `operand.offset - 1` would land in the whitespace.
#[test]
fn ticket_3860_whitespace_between_operator_and_operand_stays_inside() {
    let source = "const dynamic D = null;\nconst A = -  D;\n";
    let (op_span, operand, whole) = spans_of(source, "-  D");
    assert_eq!(text_at(source, whole), "-  D");
    assert_eq!(
        (line_col(source, whole.offset), whole.length),
        ((2, 11), 4),
        "the oracle row is 2|11|4"
    );

    let world = w();
    let node = capture_identifier(&world, "D", operand, UnOp::Neg, op_span);
    assert_eq!(node.span, whole);
    assert_eq!(
        eval(&node, &world),
        Verdict::Fault(ConstFault::ThrowsException, whole)
    );
}

// —— (3) negative control: a legal unary constant emits nothing ——————

/// `const C4 = -5;` / `const C5 = !true;` are valid constants — the capture
/// widens their spans identically, but the evaluator answers a value, so no
/// row is minted (the widening must not create diagnostics).
#[test]
fn ticket_3860_legal_unary_constants_stay_silent() {
    let source = "const C4 = -5;\nconst C5 = !true;\n";
    let world = w();

    let (op_span, operand, whole) = spans_of(source, "-5");
    let negated = capture_literal(&world, ConstantValue::Int(5), operand, UnOp::Neg, op_span);
    assert_eq!(negated.span, whole, "the wrapping still covers `-5`");
    assert_eq!(
        eval(&negated, &world),
        Verdict::Value(ConstantValue::Int(-5))
    );

    let (op_span, operand, whole) = spans_of(source, "!true");
    let not = capture_literal(
        &world,
        ConstantValue::Bool(true),
        operand,
        UnOp::Not,
        op_span,
    );
    assert_eq!(not.span, whole, "the wrapping still covers `!true`");
    assert_eq!(
        eval(&not, &world),
        Verdict::Value(ConstantValue::Bool(false))
    );
}

// —— (4) direction control: a non-constancy keeps the failing leaf ————

/// `const dynamic u2 = !b;` over a non-const `b` reports at `b` alone
/// (oracle: `…|2|21|1`): the prefix node now covers `!b`, but a
/// *non-constancy* is still answered by the leaf — only operator *faults*
/// widen.
#[test]
fn ticket_3860_nonconstancy_keeps_the_leaf_anchor() {
    let source = "dynamic b = null;\nconst dynamic u2 = !b;\n";
    let (op_span, operand, whole) = spans_of(source, "!b");
    assert_eq!(text_at(source, operand), "b");

    let world = w();
    let node = capture_identifier(&world, "b", operand, UnOp::Not, op_span);
    assert_eq!(node.span, whole, "the wrapper itself spans `!b`");
    assert_eq!(
        eval(&node, &world),
        Verdict::NotConstant(operand),
        "the non-constancy still anchors at the leaf `b` (length 1)"
    );
    assert_eq!(
        (line_col(source, operand.offset), operand.length),
        ((2, 21), 1),
        "the oracle row is 2|21|1"
    );
}
