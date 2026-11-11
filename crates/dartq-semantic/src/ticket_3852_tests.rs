//! Pins for ticket #3852: the `const_eval_throws_exception` operator faces.
//!
//! Every expectation here was derived from the pinned oracle
//! (`dart analyze --format=machine`, Dart 3.13.7) and cross-checked against
//! the evaluator itself (upstream Dart SDK source, `pkg/analyzer`'s
//! `lib/src/dart/constant/value.dart`). The two shapes pinned are:
//!
//! * a **null operand** in an arithmetic / relational / shift / bitwise
//!   operator passes that operator's `assert*OrNull*` gate and the state's
//!   own method then throws — `const_eval_throws_exception` (or the
//!   bool-or-int face for `&`/`|`/`^`, whose `DartObjectImpl` wrapper gates
//!   on `isBool`/`isInt` before reaching the state);
//! * a **lazy** `&&`/`||` failure — including one thrown by the right
//!   operand — is caught at `InvalidConstant.forEntity(entity: node)`, so
//!   it anchors at the whole binary, never at the operand.
//!
//! Direction control is pinned too: a *non-constancy* keeps the failing
//! leaf's span (`Verdict::NotConstant`), only operator *faults* widen.

use super::*;
use dartq_source::StringTable;

/// A tiny fixed world: `nc` is provably non-const, `a` is `1`, every other
/// name is a valid constant of unknown value.
struct World {
    strings: StringTable,
}

impl World {
    fn id(&self, name: &str) -> StringId {
        self.strings.intern(name)
    }

    fn node(expr: Expr, offset: u32, length: u32) -> Node {
        Node {
            expr,
            span: SourceRange { offset, length },
        }
    }

    fn value(v: ConstantValue, offset: u32) -> Node {
        Self::node(Expr::Value(v), offset, 1)
    }

    /// A binary over two literal operands, anchored at `offset` with
    /// `length` — the whole-binary span the oracle reports on.
    fn bin(&self, op: BinOp, l: ConstantValue, r: ConstantValue) -> Verdict {
        let node = Self::node(
            Expr::Binary {
                op,
                left: Box::new(Self::value(l, 12)),
                right: Box::new(Self::value(r, 18)),
            },
            11,
            9,
        );
        eval(&node, self)
    }
}

impl ConstLookup for World {
    fn ident(&self, name: StringId) -> IdentVerdict {
        if name == self.id("nc") {
            IdentVerdict::NotConstant
        } else if name == self.id("a") {
            IdentVerdict::Value(ConstantValue::Int(1))
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

    fn created_state(&self, container: StringId) -> CreatedState {
        if container == self.id("bool") {
            CreatedState::Bool
        } else if container == self.id("int") {
            CreatedState::Int
        } else {
            CreatedState::Generic
        }
    }
}

fn w() -> World {
    World {
        strings: StringTable::new(),
    }
}

/// The whole-binary span `World::bin` builds: offset 11, length 9.
const BINARY: SourceRange = SourceRange {
    offset: 11,
    length: 9,
};

use BinOp::{
    Add, BitAnd, BitOr, BitXor, Div, Ge, Gt, IntDiv, Le, Lt, Mod, Mul, Shl, Shr, Sub, Ushr,
};

// —— (1) a null operand in an arithmetic / relational operator ————————

/// `const dynamic D = null; const C = D + 5;` and the mirrored
/// `5 + D` both answer `const_eval_throws_exception` at the whole binary:
/// `NullState` inherits the base `InstanceState.add`, whose num/String/null
/// gates admit both operands and whose runtime add then throws
/// (value.dart:1659-1666), while `IntState.add` admits a null right operand
/// through `assertNumOrNull` and then throws too (value.dart:2053-2075).
#[test]
fn ticket_3852_null_plus_throws_at_the_binary() {
    let world = w();
    let null = ConstantValue::Null;
    let five = ConstantValue::Int(5);
    for (l, r) in [
        (null.clone(), five.clone()),
        (five.clone(), null.clone()),
        (null.clone(), null.clone()),
    ] {
        assert_eq!(
            world.bin(Add, l, r),
            Verdict::Fault(ConstFault::ThrowsException, BINARY)
        );
    }
    // A String left operand is the same base `add`: `'a' + D` throws.
    assert_eq!(
        world.bin(Add, ConstantValue::Str("a".into()), null),
        Verdict::Fault(ConstFault::ThrowsException, BINARY)
    );
}

/// The relational and multiplicative operators share one base shape:
/// `assertNumOrNull` on both sides admits null, then the state's own
/// `minus`/`times`/`divide`/`remainder`/`integerDivide`/`lessThan`/… throws
/// (value.dart:1956-2030). `null - null` throws as well.
#[test]
fn ticket_3852_null_numeric_and_relational_families_throw() {
    let world = w();
    let null = ConstantValue::Null;
    let five = ConstantValue::Double(5.0);
    for op in [Sub, Mul, Div, IntDiv, Mod, Lt, Gt, Le, Ge] {
        for (l, r) in [
            (null.clone(), five.clone()),
            (five.clone(), null.clone()),
            (null.clone(), null.clone()),
        ] {
            assert_eq!(
                world.bin(op, l, r),
                Verdict::Fault(ConstFault::ThrowsException, BINARY),
                "op {op:?} with a null operand must throw at the binary"
            );
        }
    }
}

/// The silent direction: two real numbers still evaluate, and `==`/`!=`
/// answer a value for a null operand (`NullState.equalEqual` is
/// `isIdentical`, value.dart:2810-2820).
#[test]
fn ticket_3852_real_numbers_and_null_equality_stay_silent() {
    let world = w();
    assert_eq!(
        world.bin(Add, ConstantValue::Int(1), ConstantValue::Int(2)),
        Verdict::Value(ConstantValue::Int(3))
    );
    assert_eq!(
        world.bin(Sub, ConstantValue::Int(5), ConstantValue::Double(1.5)),
        Verdict::Value(ConstantValue::Double(3.5))
    );
    assert_eq!(
        world.bin(BinOp::Eq, ConstantValue::Null, ConstantValue::Int(5)),
        Verdict::Value(ConstantValue::Bool(false))
    );
    assert_eq!(
        world.bin(BinOp::Ne, ConstantValue::Null, ConstantValue::Int(5)),
        Verdict::Value(ConstantValue::Bool(true))
    );
}

/// `&`/`|`/`^` answer the bool-or-int face for a null operand:
/// `DartObjectImpl.eagerAnd`/`eagerOr`/`eagerXor` dispatch on
/// `isBool`/`isInt` (value.dart:446-514) and `NullState` is neither.
#[test]
fn ticket_3852_null_bitwise_answers_the_bool_or_int_face() {
    let world = w();
    let null = ConstantValue::Null;
    for op in [BitAnd, BitOr, BitXor] {
        for (l, r) in [
            (null.clone(), ConstantValue::Int(5)),
            (ConstantValue::Int(5), null.clone()),
            (ConstantValue::Bool(true), null.clone()),
            (null.clone(), null.clone()),
        ] {
            assert_eq!(
                world.bin(op, l, r),
                Verdict::Fault(ConstFault::TypeBoolInt, BINARY),
                "op {op:?} with a null operand must answer const_eval_type_bool_int"
            );
        }
    }
    // The real bool/int pairs still evaluate.
    assert_eq!(
        world.bin(
            BitAnd,
            ConstantValue::Bool(true),
            ConstantValue::Bool(false)
        ),
        Verdict::Value(ConstantValue::Bool(false))
    );
    assert_eq!(
        world.bin(BitOr, ConstantValue::Int(5), ConstantValue::Int(3)),
        Verdict::Value(ConstantValue::Int(7))
    );
}

/// A null operand of a shift throws: `assertIntOrNull` admits null on both
/// sides and the state's own `shiftLeft`/`shiftRight`/`logicalShiftRight`
/// then throws (value.dart:1987-2003, inherited by `NullState`).
#[test]
fn ticket_3852_null_shift_throws_at_the_binary() {
    let world = w();
    let null = ConstantValue::Null;
    for op in [Shl, Shr, Ushr] {
        assert_eq!(
            world.bin(op, null.clone(), ConstantValue::Int(1)),
            Verdict::Fault(ConstFault::ThrowsException, BINARY),
            "op {op:?} with a null left operand must throw"
        );
        assert_eq!(
            world.bin(op, ConstantValue::Int(1), null.clone()),
            Verdict::Fault(ConstFault::ThrowsException, BINARY),
            "op {op:?} with a null right operand must throw"
        );
    }
    // A non-int right operand is still the int face, not a throw.
    assert_eq!(
        world.bin(Shl, ConstantValue::Int(1), ConstantValue::Bool(true)),
        Verdict::Fault(ConstFault::TypeInt, BINARY)
    );
    // And a real pair evaluates.
    assert_eq!(
        world.bin(Shl, ConstantValue::Int(1), ConstantValue::Int(3)),
        Verdict::Value(ConstantValue::Int(8))
    );
}

/// A created BOOL container's `+` against an int: the base
/// `InstanceState.add` gate is `assertNumStringOrNull` on BOTH operands
/// (`value.dart:1636-1647`), so the num-or-String face reports — not the
/// num face. The oracle confirms on
/// `const b = bool.fromEnvironment('b'); const x = b + 1;` →
/// `const_eval_type_num_string`. (#3913 corrected this expectation; the
/// previous num-face pin recorded a deviation the oracle contradicts.)
#[test]
fn ticket_3852_created_container_plus_keeps_the_num_face() {
    let world = w();
    assert_eq!(
        world.bin(
            Add,
            ConstantValue::Created(world.id("bool")),
            ConstantValue::Int(1)
        ),
        Verdict::Fault(ConstFault::TypeNumString, BINARY)
    );
}

// —— (2) lazy `&&`/`||` re-anchor every fault at the whole binary ————

/// A known non-bool left operand fails `assertBool(this)` inside
/// `lazyAnd`/`lazyOr`; `DartObjectComputer` catches the throw at
/// `InvalidConstant.forEntity(entity: node)`, the whole binary
/// (evaluation.dart:2495-2530). Oracle: `const dynamic D = null;
/// const l1 = D && true;` reports at the `D && true` extent.
#[test]
fn ticket_3852_lazy_left_operand_faults_anchor_at_the_binary() {
    let world = w();
    let null = ConstantValue::Null;
    for op in [BinOp::And, BinOp::Or] {
        assert_eq!(
            world.bin(op, null.clone(), ConstantValue::Bool(true)),
            Verdict::Fault(ConstFault::TypeBool, BINARY),
            "op {op:?} with a null left operand must answer const_eval_type_bool at the binary"
        );
    }
    // A non-bool non-null left takes the same face.
    assert_eq!(
        world.bin(BinOp::And, ConstantValue::Int(1), ConstantValue::Bool(true)),
        Verdict::Fault(ConstFault::TypeBool, BINARY)
    );
}

/// A non-bool **right** operand fails `assertBool(rightOperand)` after the
/// left decided to evaluate it — same catch, same whole-binary anchor.
#[test]
fn ticket_3852_lazy_right_operand_faults_anchor_at_the_binary() {
    let world = w();
    let null = ConstantValue::Null;
    assert_eq!(
        world.bin(BinOp::And, ConstantValue::Bool(true), null.clone()),
        Verdict::Fault(ConstFault::TypeBool, BINARY)
    );
    assert_eq!(
        world.bin(BinOp::Or, ConstantValue::Bool(false), null),
        Verdict::Fault(ConstFault::TypeBool, BINARY)
    );
}

/// A fault thrown *inside* the right operand is re-anchored too: the code
/// survives, the span becomes the lazy expression. Oracle:
/// `const dynamic D = null; const q1 = true && (D + 5);` reports
/// `CONST_EVAL_THROWS_EXCEPTION` over the whole `true && (D + 5)`.
#[test]
fn ticket_3852_lazy_inner_fault_is_reanchored_at_the_binary() {
    let world = w();
    // `true && (D + 5)` — the inner `D + 5` would anchor at offset 19.
    let node = World::node(
        Expr::Binary {
            op: BinOp::And,
            left: Box::new(World::value(ConstantValue::Bool(true), 0)),
            right: Box::new(World::node(
                Expr::Binary {
                    op: Add,
                    left: Box::new(World::value(ConstantValue::Null, 19)),
                    right: Box::new(World::value(ConstantValue::Int(5), 25)),
                },
                19,
                5,
            )),
        },
        0,
        15,
    );
    assert_eq!(
        eval(&node, &world),
        Verdict::Fault(
            ConstFault::ThrowsException,
            SourceRange {
                offset: 0,
                length: 15
            }
        )
    );
}

/// The clean direction of the lazy family: a settled pair evaluates, `??`
/// hands the right operand's verdict through unchanged
/// (evaluation.dart:724-736), and a skipped non-const operand is reported
/// by the scan rather than valued (evaluation.dart:708-723).
#[test]
fn ticket_3852_lazy_clean_paths_stay_silent() {
    let world = w();
    assert_eq!(
        world.bin(
            BinOp::And,
            ConstantValue::Bool(true),
            ConstantValue::Bool(false)
        ),
        Verdict::Value(ConstantValue::Bool(false))
    );
    assert_eq!(
        world.bin(
            BinOp::Or,
            ConstantValue::Bool(false),
            ConstantValue::Bool(true)
        ),
        Verdict::Value(ConstantValue::Bool(true))
    );
    assert_eq!(
        world.bin(BinOp::IfNull, ConstantValue::Null, ConstantValue::Int(2)),
        Verdict::Value(ConstantValue::Int(2))
    );
    // A skipped right operand that is provably non-const replaces the
    // settled value with the leaf's non-constancy (not a value).
    let short_circuit = World::node(
        Expr::Binary {
            op: BinOp::And,
            left: Box::new(World::value(ConstantValue::Bool(false), 0)),
            right: Box::new(World::node(Expr::Ident(world.id("nc")), 9, 2)),
        },
        0,
        12,
    );
    assert_eq!(
        eval(&short_circuit, &world),
        Verdict::NotConstant(SourceRange {
            offset: 9,
            length: 2
        })
    );
}

// —— direction control: a non-constancy keeps the failing leaf ——————

/// Only **operator faults** widen. A non-constancy keeps the failing leaf's
/// span: `const u2 = !b;` with a non-const `b` reports at `b`
/// (`NonConstant`), never over the `!b` node.
#[test]
fn ticket_3852_nonconstancy_keeps_the_leaf_anchor() {
    let world = w();
    let leaf = SourceRange {
        offset: 9,
        length: 2,
    };
    let negated = World::node(
        Expr::Unary {
            op: UnOp::Not,
            operand: Box::new(World::node(Expr::Ident(world.id("nc")), 9, 2)),
        },
        7,
        4,
    );
    assert_eq!(eval(&negated, &world), Verdict::NotConstant(leaf));
    // The eager binary path propagates the leaf the same way.
    let sum = World::node(
        Expr::Binary {
            op: Add,
            left: Box::new(World::node(Expr::Ident(world.id("nc")), 9, 2)),
            right: Box::new(World::value(ConstantValue::Int(1), 14)),
        },
        7,
        9,
    );
    assert_eq!(eval(&sum, &world), Verdict::NotConstant(leaf));
}
