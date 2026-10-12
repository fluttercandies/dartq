//! The M2 constant-evaluation subset (#279, PRD §10.2): a bounded,
//! compile-time evaluator over the parser's event stream that decides
//! whether a const-context expression is constant, and which
//! `constEval*`/`non_constant_*` diagnostic the oracle reports when it is
//! not.
//!
//! # What is in the subset
//!
//! Per the official contract (`pkg/analyzer/lib/src/dart/ast/
//! constant_evaluator.dart:16-84`, the spec transcription) and the live
//! `ConstantVisitor` (`pkg/analyzer/lib/src/dart/constant/evaluation.dart`):
//!
//! * literals — `null`, bool, int, double, string (adjacent and
//!   interpolated), and identifier references to const variables / enum
//!   constants / type literals / top-level function tear-offs;
//! * `const` constructor invocations — every argument must be constant
//!   (`const_with_non_constant_argument`);
//! * string concatenation (adjacent strings, `'+'` on two strings) and
//!   const list / set / map / record literals with constant elements
//!   (`non_constant_list_element` / `_set_element` / `_map_element` /
//!   `_record_field`);
//! * constant arithmetic, comparison, and logic — the operator set of
//!   `DartObjectImpl` (`pkg/analyzer/lib/src/dart/constant/value.dart`):
//!   binary `+ - * / ~/ % & | ^ << >> >>> < > <= >= == != && || ??`,
//!   unary `- ! ~`, `?:` — with the oracle's per-operator operand-type
//!   faults (`const_eval_type_bool`, `_int`, `_num`, `_num_string`,
//!   `_string`, `_bool_int`, `_bool_num_string`, `_primitive_equality`)
//!   and runtime throws (`const_eval_throws_exception`,
//!   `_throws_idbze`).
//!
//! # What defers (under-report, never a false positive)
//!
//! * Values this subset cannot produce: symbols, const-class instances,
//!   foreign (cross-file) constants, `as` results, index expressions,
//!   string interpolation text — these evaluate to [`Verdict::Constant`]
//!   ("a valid constant of unknown value") and no diagnostic can arise
//!   from them.
//! * Identifiers the pass cannot prove constant *or* non-constant (a
//!   not-yet-seen or imported name): they evaluate to
//!   [`Verdict::Constant`]; only an in-scope local/formal proves
//!   non-constancy.
//! * Call shapes the oracle codes outside this family: member
//!   invocations (`a.b(...)`, property-access/method codes),
//!   containers without a declared const constructor
//!   (`const_with_non_const*`), spread/if-for elements.
//! * Non-const constructor targets (`const_with_non_const`), annotation
//!   arguments, formal-parameter default values, and
//!   constructor-initializer lists.
//!
//! # Shape and termination
//!
//! The listener-facing half ([`ConstCapture`]) folds the parser event
//! stream into a finite expression tree while a const context is open;
//! the pure half ([`eval`]) is a total fold over that tree — every
//! recursion descends into a strict subtree, so evaluation terminates by
//! construction and adds no unbounded surface (the engine-wide
//! termination invariant). Errors carry the span of the deepest
//! construct that produced them (the oracle's `InvalidConstant` error
//! entity); a `constEval*` fault keeps its own span and code through
//! enclosing nodes.

use dartq_source::{SourceRange, StringId};

/// A compile-time constant value (the Rust-native stand-in for the
/// oracle's `DartObjectImpl` states — no class mirroring).
///
/// String payloads are the raw source interior: escapes are not decoded
/// and interpolated text chunks are not materialized, because no
/// diagnostic in this subset observes string *contents* — only operand
/// types do. The payloads exist so concatenation and equality stay
/// well-typed.
#[derive(Clone, Debug, PartialEq)]
pub enum ConstantValue {
    Null,
    Bool(bool),
    Int(i64),
    Double(f64),
    Str(Box<str>),
    /// An enum constant instance, by its container and constant names.
    Enum {
        container: StringId,
        name: StringId,
    },
    /// A type literal (`C` in `const x = C;`), by its type name — the
    /// receiver face `E.a` resolves through.
    Type(StringId),
    /// A const-constructor instance by its container name — the face the
    /// const variable's declared type folds to (`const x = C();` has
    /// static type `C`).
    Created(StringId),
    /// An opaque constant instance whose identity no M2 diagnostic
    /// observes.
    Instance(InstanceKind),
}

/// The opaque-instance kinds: const collections, symbols, function
/// tear-offs, const-constructor instances, records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstanceKind {
    List,
    Set,
    Map,
    Record,
    Symbol,
    Function,
    Object,
}

impl ConstantValue {
    /// `isBoolNumStringOrNull` (`value.dart`, the interpolation operand
    /// gate).
    fn is_bool_num_string_or_null(&self) -> bool {
        matches!(
            self,
            ConstantValue::Null
                | ConstantValue::Bool(_)
                | ConstantValue::Int(_)
                | ConstantValue::Double(_)
                | ConstantValue::Str(_)
        )
    }
}

/// A decidable const-evaluation fault — the oracle's `InvalidConstant`
/// carrying an allow-listed `constEval*` diagnostic (`constant_verifier.dart`
/// `_reportError`: these codes override the reporting site's default).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstFault {
    /// `const_eval_type_bool` — `!`, `&&`/`||`, `?:` condition.
    TypeBool,
    /// `const_eval_type_bool_int` — `&`, `|`, `^` on mixed kinds.
    TypeBoolInt,
    /// `const_eval_type_bool_num_string` — interpolation operands.
    TypeBoolNumString,
    /// `const_eval_type_int` — shifts and `~` on non-int.
    TypeInt,
    /// `const_eval_type_num` — numeric operators on non-num.
    TypeNum,
    /// `const_eval_type_num_string` — `+` operand neither num, String,
    /// nor null.
    TypeNumString,
    /// `const_eval_type_string` — string concatenation right operand.
    TypeString,
    /// `const_eval_throws_exception` — evaluation throws.
    ThrowsException,
    /// `const_eval_throws_idbze` — integer division by zero.
    ThrowsIdbze,
    /// `const_eval_primitive_equality` — `==` operand without primitive
    /// equality (the patterns-era default).
    PrimitiveEquality,
    /// `recursive_compile_time_constant` — the initializer depends on
    /// itself.
    Recursive,
}

/// The verdict of evaluating one expression.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// A constant whose value this subset knows.
    Value(ConstantValue),
    /// A valid compile-time constant of unknown value — no diagnostic may
    /// arise from it (the deferral verdict).
    Constant,
    /// Provably not a constant expression; the span anchors the deepest
    /// construct that broke const-ness (the oracle's error entity), and
    /// the reporting site supplies its default code.
    NotConstant(SourceRange),
    /// A decidable `constEval*` fault at its own span — reported verbatim,
    /// overriding any site default.
    Fault(ConstFault, SourceRange),
}

/// The capture-time expression tree. Spans live on every node: faults
/// anchor at the node the oracle anchors at.
#[derive(Clone, Debug)]
pub struct Node {
    pub expr: Expr,
    pub span: SourceRange,
}

#[derive(Clone, Debug)]
pub enum Expr {
    Value(ConstantValue),
    /// A bare identifier; resolved through [`ConstLookup`] at eval time.
    Ident(StringId),
    /// `recv.name` (`E.a`, `C.v`, `obj.field` in a const context).
    Member {
        recv: Box<Node>,
        name: StringId,
    },
    Binary {
        op: BinOp,
        left: Box<Node>,
        right: Box<Node>,
    },
    Unary {
        op: UnOp,
        operand: Box<Node>,
    },
    Cond {
        cond: Box<Node>,
        then: Box<Node>,
        els: Box<Node>,
    },
    /// A string interpolation: only the `${...}` operands are captured —
    /// the text chunks are unobservable.
    Interp {
        parts: Vec<Node>,
    },
    Adjacent(Vec<Node>),
    List(Vec<Node>),
    Set(Vec<Node>),
    Map(Vec<(Node, Node)>),
    Record(Vec<Node>),
    /// A function invocation. Only `identical(a, b)` evaluates; every
    /// other call is provably non-constant.
    Call {
        name: StringId,
        args: Vec<Node>,
    },
    /// A map-entry pseudo-operand (stack staging between the entry event
    /// and the literal close).
    Entry(Box<Node>, Box<Node>),
    /// A valid constant of unknown value — constructs the subset defers
    /// on (`e as T`, index expressions, unresolvable call shapes): the
    /// fold carries no verdict through them, so no diagnostic can arise.
    Opaque,
    /// Provably not a constant expression (assignment, `await`, `throw`,
    /// `new`, function literal, `++`/`--`, cascade, `x!`, `is`).
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    IntDiv,
    Mod,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Ushr,
    Lt,
    Gt,
    Le,
    Ge,
    Eq,
    Ne,
    And,
    Or,
    IfNull,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    BitNot,
    /// `++`/`--` — captures as [`Expr::Other`] at eval.
    Inc,
}

/// The bare-name resolution face the evaluator needs; implemented by the
/// diagnose engine (const scopes, element model, const bindings).
pub trait ConstLookup {
    /// Classifies a bare identifier in the current const context.
    fn ident(&self, name: StringId) -> IdentVerdict;
    /// Resolves `recv.name` where `recv` evaluated to a non-null value.
    fn member(&self, recv: &ConstantValue, name: StringId) -> Verdict;
    /// Whether `name` is the callee spelling of `identical` (the one
    /// invocable function).
    fn is_identical(&self, name: StringId) -> bool;
}

/// The bare-identifier classification.
#[derive(Clone, Debug, PartialEq)]
pub enum IdentVerdict {
    /// A known const value (an already-evaluated const variable).
    Value(ConstantValue),
    /// A valid constant of unknown value (tear-off, foreign or forward
    /// const).
    Constant,
    /// The initializer currently being evaluated — a
    /// `recursive_compile_time_constant` cycle.
    InProgress,
    /// Provably non-const: an in-scope local/formal, or a top-level
    /// variable this pass saw declared without `const`.
    NotConstant,
}

/// The evaluation-depth fuse: beyond it, the value defers to
/// [`Verdict::Constant`] instead of recursing (the engine-wide
/// termination invariant — a pathological expression must never crash
/// the analyzer).
const MAX_EVAL_DEPTH: u32 = 256;

/// Total evaluation fold over a captured tree. Every recursive call
/// descends into a strict subtree — termination is structural, and the
/// depth fuse bounds the recursion for machine-generated expression
/// chains.
pub fn eval(node: &Node, lookup: &dyn ConstLookup) -> Verdict {
    fold(node, lookup, 0)
}

fn fold(node: &Node, lookup: &dyn ConstLookup, depth: u32) -> Verdict {
    if depth > MAX_EVAL_DEPTH {
        return Verdict::Constant;
    }
    let depth = depth + 1;
    match &node.expr {
        Expr::Value(v) => Verdict::Value(v.clone()),
        Expr::Other => Verdict::NotConstant(node.span),
        Expr::Ident(name) => match lookup.ident(*name) {
            IdentVerdict::Value(v) => Verdict::Value(v),
            IdentVerdict::Constant => Verdict::Constant,
            IdentVerdict::InProgress => Verdict::Fault(ConstFault::Recursive, node.span),
            IdentVerdict::NotConstant => Verdict::NotConstant(node.span),
        },
        Expr::Member { recv, name } => {
            let r = fold(recv, lookup, depth);
            match r {
                Verdict::Value(v) if v != ConstantValue::Null => lookup.member(&v, *name),
                Verdict::Fault(..) | Verdict::NotConstant(_) => r,
                _ => Verdict::Constant,
            }
        }
        Expr::Binary { op, left, right } => eval_binary(*op, node.span, left, right, lookup, depth),
        Expr::Unary { op, operand } => match op {
            UnOp::Inc => Verdict::NotConstant(node.span),
            UnOp::Neg => match fold(operand, lookup, depth) {
                Verdict::Value(ConstantValue::Int(v)) => {
                    Verdict::Value(ConstantValue::Int(v.wrapping_neg()))
                }
                Verdict::Value(ConstantValue::Double(v)) => {
                    Verdict::Value(ConstantValue::Double(-v))
                }
                Verdict::Value(_) => Verdict::Fault(ConstFault::TypeNum, node.span),
                other => other,
            },
            UnOp::Not => match fold(operand, lookup, depth) {
                Verdict::Value(ConstantValue::Bool(v)) => Verdict::Value(ConstantValue::Bool(!v)),
                Verdict::Value(_) => Verdict::Fault(ConstFault::TypeBool, node.span),
                other => other,
            },
            UnOp::BitNot => match fold(operand, lookup, depth) {
                Verdict::Value(ConstantValue::Int(v)) => Verdict::Value(ConstantValue::Int(!v)),
                Verdict::Value(_) => Verdict::Fault(ConstFault::TypeInt, node.span),
                other => other,
            },
        },
        Expr::Cond { cond, then, els } => match fold(cond, lookup, depth) {
            Verdict::Value(ConstantValue::Bool(true)) => fold(then, lookup, depth),
            Verdict::Value(ConstantValue::Bool(false)) => fold(els, lookup, depth),
            // The oracle converts the condition to bool first
            // (`applyBooleanConversion`): a known non-bool faults.
            Verdict::Value(_) => Verdict::Fault(ConstFault::TypeBool, cond.span),
            // Unknown condition: both branches must be constant; the
            // first error propagates (the oracle evaluates `then` before
            // `else`).
            Verdict::Constant => {
                let t = fold(then, lookup, depth);
                let e = fold(els, lookup, depth);
                match (t, e) {
                    (Verdict::Fault(f, s), _) | (_, Verdict::Fault(f, s)) => Verdict::Fault(f, s),
                    (Verdict::NotConstant(s), _) | (_, Verdict::NotConstant(s)) => {
                        Verdict::NotConstant(s)
                    }
                    _ => Verdict::Constant,
                }
            }
            other => other,
        },
        Expr::Interp { parts } => {
            for part in parts {
                match fold(part, lookup, depth) {
                    Verdict::Value(v) if v.is_bool_num_string_or_null() => {}
                    Verdict::Value(_) => {
                        return Verdict::Fault(ConstFault::TypeBoolNumString, part.span);
                    }
                    other => return other,
                }
            }
            // The operand type gate is the only observable surface — the
            // concatenated text itself is not materialized (escapes are
            // undecoded, so a value would be wrong where it is observable:
            // equality, conditions). A clean interpolation defers.
            Verdict::Constant
        }
        Expr::Adjacent(parts) => {
            let mut text = String::new();
            for part in parts {
                match fold(part, lookup, depth) {
                    Verdict::Value(ConstantValue::Str(s)) => text.push_str(&s),
                    // Non-string parts fall back to the oracle's
                    // `_concatenateNodes` conversion — value unknown here.
                    Verdict::Value(_) => return Verdict::Constant,
                    other => return other,
                }
            }
            Verdict::Value(ConstantValue::Str(text.into()))
        }
        Expr::List(parts) => eval_each(parts, lookup, InstanceKind::List, depth),
        Expr::Set(parts) => eval_each(parts, lookup, InstanceKind::Set, depth),
        Expr::Map(entries) => {
            for (k, v) in entries {
                for part in [k, v] {
                    if let other @ (Verdict::Fault(..) | Verdict::NotConstant(_)) =
                        fold(part, lookup, depth)
                    {
                        return other;
                    }
                }
            }
            Verdict::Value(ConstantValue::Instance(InstanceKind::Map))
        }
        Expr::Record(fields) => eval_each(fields, lookup, InstanceKind::Record, depth),
        Expr::Entry(..) | Expr::Opaque => Verdict::Constant,
        Expr::Call { name, args } => {
            // Only `identical(a, b)` reaches eval — the capture resolves
            // every other callee shape at its close (call classification
            // needs the engine's name resolution). A directly built
            // non-identical call is a non-constant invocation.
            if args.len() != 2 || !lookup.is_identical(*name) {
                return Verdict::NotConstant(node.span);
            }
            // `identical(a, b)`: both operands must be constant; the
            // result is a bool of possibly-unknown value.
            let mut values = (None, None);
            for (i, arg) in args.iter().enumerate() {
                match fold(arg, lookup, depth) {
                    Verdict::Value(v) => {
                        if i == 0 {
                            values.0 = Some(v);
                        } else {
                            values.1 = Some(v);
                        }
                    }
                    Verdict::Constant => {}
                    other => return other,
                }
            }
            let identical = match (values.0, values.1) {
                (Some(a), Some(b)) => primarily_identical(&a, &b),
                _ => false,
            };
            Verdict::Value(ConstantValue::Bool(identical))
        }
    }
}

/// Dart `identical` over the values this subset knows: canonicalized
/// primitives are identical when their types and values coincide (`1` and
/// `1.0` are not identical); enum instances are identical to themselves
/// only.
fn primarily_identical(a: &ConstantValue, b: &ConstantValue) -> bool {
    match (a, b) {
        (ConstantValue::Null, ConstantValue::Null) => true,
        (ConstantValue::Bool(x), ConstantValue::Bool(y)) => x == y,
        (ConstantValue::Int(x), ConstantValue::Int(y)) => x == y,
        (ConstantValue::Double(x), ConstantValue::Double(y)) => x == y,
        (ConstantValue::Str(x), ConstantValue::Str(y)) => x == y,
        (
            ConstantValue::Enum {
                container: c1,
                name: n1,
            },
            ConstantValue::Enum {
                container: c2,
                name: n2,
            },
        ) => c1 == c2 && n1 == n2,
        _ => false,
    }
}

/// Collection/record element fold: every element must be constant; a
/// failing element's verdict propagates with its own anchor (the
/// variable-initializer site reports at the deepest non-const construct,
/// `constant_verifier.dart` `_evaluateAndReportError`).
fn eval_each(parts: &[Node], lookup: &dyn ConstLookup, kind: InstanceKind, depth: u32) -> Verdict {
    for part in parts {
        if let other @ (Verdict::Fault(..) | Verdict::NotConstant(_)) = fold(part, lookup, depth) {
            return other;
        }
    }
    Verdict::Value(ConstantValue::Instance(kind))
}

fn eval_binary(
    op: BinOp,
    span: SourceRange,
    left: &Node,
    right: &Node,
    lookup: &dyn ConstLookup,
    depth: u32,
) -> Verdict {
    // Lazy operators evaluate the right operand only when the left cannot
    // decide (`lazyAnd`/`lazyOr`/`lazyQuestionQuestion`).
    match op {
        BinOp::And | BinOp::Or => {
            let l = fold(left, lookup, depth);
            match l {
                Verdict::Value(ConstantValue::Bool(b)) => {
                    if (b && op == BinOp::Or) || (!b && op == BinOp::And) {
                        l
                    } else {
                        match fold(right, lookup, depth) {
                            Verdict::Value(ConstantValue::Bool(rb)) => {
                                Verdict::Value(ConstantValue::Bool(rb))
                            }
                            Verdict::Fault(f, s) => Verdict::Fault(f, s),
                            Verdict::NotConstant(s) => Verdict::NotConstant(s),
                            _ => Verdict::Constant,
                        }
                    }
                }
                Verdict::Value(_) => Verdict::Fault(ConstFault::TypeBool, left.span),
                other => other,
            }
        }
        BinOp::IfNull => {
            let l = fold(left, lookup, depth);
            match l {
                Verdict::Value(ConstantValue::Null) => fold(right, lookup, depth),
                other => other,
            }
        }
        _ => {
            let l = fold(left, lookup, depth);
            let r = fold(right, lookup, depth);
            match (l, r) {
                (Verdict::Value(a), Verdict::Value(b)) => binary_values(op, span, &a, &b),
                (Verdict::Fault(f, s), _) | (_, Verdict::Fault(f, s)) => Verdict::Fault(f, s),
                (Verdict::NotConstant(s), _) | (_, Verdict::NotConstant(s)) => {
                    Verdict::NotConstant(s)
                }
                _ => Verdict::Constant,
            }
        }
    }
}

/// The eager operator matrix, mirroring `DartObjectImpl`'s per-state
/// methods (`value.dart`): the operand-type asserts decide which
/// `constEval*` fault fires, in the oracle's assert order (left first).
fn binary_values(op: BinOp, span: SourceRange, a: &ConstantValue, b: &ConstantValue) -> Verdict {
    use BinOp::*;
    // String repetition (`'ab' * 3`) is a spec constant the subset cannot
    // value (escapes undecoded) — defer; the oracle's StringState keeps it
    // clean, so the numeric operand fault must never fire here.
    if op == Mul && matches!(a, ConstantValue::Str(_)) {
        return Verdict::Constant;
    }
    match op {
        Add => match a {
            ConstantValue::Str(x) => match b {
                ConstantValue::Str(y) => {
                    let mut t = String::from(&**x);
                    t.push_str(y);
                    Verdict::Value(ConstantValue::Str(t.into()))
                }
                // `StringState.add`: the right operand passes the
                // num/String/null assert, then the runtime add throws.
                ConstantValue::Int(_) | ConstantValue::Double(_) => {
                    Verdict::Fault(ConstFault::ThrowsException, span)
                }
                ConstantValue::Null => Verdict::Constant,
                _ => Verdict::Fault(ConstFault::TypeNumString, span),
            },
            ConstantValue::Int(_) | ConstantValue::Double(_) | ConstantValue::Null => match b {
                ConstantValue::Int(_) | ConstantValue::Double(_) | ConstantValue::Null => {
                    if is_num(a) && is_num(b) {
                        numeric(op, span, a, b)
                    } else {
                        Verdict::Constant
                    }
                }
                _ => Verdict::Fault(ConstFault::TypeNum, span),
            },
            // `assertNumOrNull(this)` fires before the right operand's.
            _ => Verdict::Fault(ConstFault::TypeNum, span),
        },
        Sub | Mul | Div | IntDiv | Mod | Lt | Gt | Le | Ge => {
            if is_num(a) && is_num(b) {
                numeric(op, span, a, b)
            } else if is_num_or_null(a) && is_num_or_null(b) {
                Verdict::Constant
            } else {
                Verdict::Fault(ConstFault::TypeNum, span)
            }
        }
        BitAnd | BitOr | BitXor => match (a, b) {
            (ConstantValue::Bool(x), ConstantValue::Bool(y)) => {
                Verdict::Value(ConstantValue::Bool(match op {
                    BitAnd => x & y,
                    BitOr => x | y,
                    _ => x ^ y,
                }))
            }
            (ConstantValue::Int(_), ConstantValue::Int(_)) => {
                let (ConstantValue::Int(x), ConstantValue::Int(y)) = (a, b) else {
                    unreachable!()
                };
                Verdict::Value(ConstantValue::Int(match op {
                    BitAnd => x & y,
                    BitOr => x | y,
                    _ => x ^ y,
                }))
            }
            (ConstantValue::Null, ConstantValue::Bool(_) | ConstantValue::Int(_))
            | (ConstantValue::Bool(_) | ConstantValue::Int(_), ConstantValue::Null)
            | (ConstantValue::Null, ConstantValue::Null) => Verdict::Constant,
            _ => Verdict::Fault(ConstFault::TypeBoolInt, span),
        },
        Shl | Shr | Ushr => match (a, b) {
            (ConstantValue::Null, ConstantValue::Int(_) | ConstantValue::Null)
            | (ConstantValue::Int(_), ConstantValue::Null) => Verdict::Constant,
            (ConstantValue::Int(x), ConstantValue::Int(shift)) => {
                let v = match op {
                    Shl => {
                        if *shift < 0 {
                            return Verdict::Fault(ConstFault::ThrowsException, span);
                        }
                        if *shift >= 1 << 31 {
                            return Verdict::Constant;
                        }
                        if *shift >= 64 { 0 } else { x << *shift }
                    }
                    Shr => {
                        if *shift < 0 {
                            return Verdict::Fault(ConstFault::ThrowsException, span);
                        }
                        if *shift >= 1 << 31 {
                            return Verdict::Constant;
                        }
                        if *shift >= 64 {
                            if *x < 0 { -1 } else { 0 }
                        } else {
                            x >> *shift
                        }
                    }
                    _ => {
                        if *shift < 0 {
                            return Verdict::Fault(ConstFault::ThrowsException, span);
                        }
                        if *shift >= 1 << 31 {
                            return Verdict::Constant;
                        }
                        if *shift >= 64 {
                            0
                        } else {
                            ((*x as u64) >> *shift) as i64
                        }
                    }
                };
                Verdict::Value(ConstantValue::Int(v))
            }
            _ => Verdict::Fault(ConstFault::TypeInt, span),
        },
        Eq | Ne => {
            // The oracle's `==` (`equalEqual`, value.dart:524): null pairs
            // decide first, then the primitive-equality gate faults
            // collection/type operands; primitive and enum operands
            // compare — cross-kind pairs answer false silently
            // (`BoolState.isIdentical` falls through to FALSE).
            let eq = match (a, b) {
                (ConstantValue::Null, ConstantValue::Null) => true,
                (ConstantValue::Null, _) | (_, ConstantValue::Null) => false,
                // Two const-creation instances: field identity is beyond
                // the subset — defer (a user `==` override may also lift
                // the gate).
                (ConstantValue::Created(_), ConstantValue::Created(_)) => {
                    return Verdict::Constant;
                }
                (ConstantValue::Bool(x), ConstantValue::Bool(y)) => x == y,
                (ConstantValue::Int(x), ConstantValue::Int(y)) => x == y,
                (ConstantValue::Double(x), ConstantValue::Double(y)) => x == y,
                (ConstantValue::Int(x), ConstantValue::Double(y))
                | (ConstantValue::Double(y), ConstantValue::Int(x)) => *x as f64 == *y,
                (ConstantValue::Str(x), ConstantValue::Str(y)) => x == y,
                // Enum constants: same constant true, different false.
                (
                    ConstantValue::Enum {
                        container: c1,
                        name: n1,
                    },
                    ConstantValue::Enum {
                        container: c2,
                        name: n2,
                    },
                ) => c1 == c2 && n1 == n2,
                // A creation instance against a known-state operand:
                // distinct runtime identities.
                (ConstantValue::Created(_), _) | (_, ConstantValue::Created(_)) => false,
                // Cross-primitive pairs.
                (ConstantValue::Bool(_), _)
                | (_, ConstantValue::Bool(_))
                | (ConstantValue::Int(_), _)
                | (_, ConstantValue::Int(_))
                | (ConstantValue::Double(_), _)
                | (_, ConstantValue::Double(_))
                | (ConstantValue::Str(_), _)
                | (_, ConstantValue::Str(_))
                | (ConstantValue::Enum { .. }, _)
                | (_, ConstantValue::Enum { .. }) => false,
                // Collections, type literals, symbols, records lack
                // primitive equality.
                _ => return Verdict::Fault(ConstFault::PrimitiveEquality, span),
            };
            Verdict::Value(ConstantValue::Bool(if op == Eq { eq } else { !eq }))
        }
        And | Or | IfNull => unreachable!("lazy operators handled in eval_binary"),
    }
}

fn is_num(v: &ConstantValue) -> bool {
    matches!(v, ConstantValue::Int(_) | ConstantValue::Double(_))
}

fn is_num_or_null(v: &ConstantValue) -> bool {
    is_num(v) || *v == ConstantValue::Null
}

/// The numeric kernel: int/int stays int (except `/`, always double),
/// any double widens, exactly as `IntState`/`DoubleState` do. Dart int
/// arithmetic wraps (64-bit two's complement, the VM semantics).
fn numeric(op: BinOp, span: SourceRange, a: &ConstantValue, b: &ConstantValue) -> Verdict {
    use BinOp::*;
    if matches!((a, b), (ConstantValue::Int(_), ConstantValue::Int(_))) {
        let (ConstantValue::Int(x), ConstantValue::Int(y)) = (a, b) else {
            unreachable!()
        };
        let v = match op {
            Add => x.wrapping_add(*y),
            Sub => x.wrapping_sub(*y),
            Mul => x.wrapping_mul(*y),
            Div => return Verdict::Value(ConstantValue::Double(*x as f64 / *y as f64)),
            IntDiv => {
                if *y == 0 {
                    return Verdict::Fault(ConstFault::ThrowsIdbze, span);
                }
                return Verdict::Value(ConstantValue::Int(x.wrapping_div(*y)));
            }
            Mod => {
                if *y == 0 {
                    return Verdict::Fault(ConstFault::ThrowsException, span);
                }
                // Dart `%` is floor-mod: the result takes the divisor's
                // sign domain (`5 % -3 == -1`, `-5 % 3 == 1`).
                let r = x.wrapping_rem(*y);
                let r = if r != 0 && (r < 0) != (*y < 0) {
                    r.wrapping_add(*y)
                } else {
                    r
                };
                return Verdict::Value(ConstantValue::Int(r));
            }
            Lt => return Verdict::Value(ConstantValue::Bool(x < y)),
            Gt => return Verdict::Value(ConstantValue::Bool(x > y)),
            Le => return Verdict::Value(ConstantValue::Bool(x <= y)),
            Ge => return Verdict::Value(ConstantValue::Bool(x >= y)),
            _ => unreachable!("non-arithmetic op in numeric kernel"),
        };
        Verdict::Value(ConstantValue::Int(v))
    } else {
        let x = as_f64(a);
        let y = as_f64(b);
        let v = match op {
            Add => x + y,
            Sub => x - y,
            Mul => x * y,
            Div => x / y,
            IntDiv => {
                let q = x / y;
                if q.is_finite() {
                    return Verdict::Value(ConstantValue::Int(q.trunc() as i64));
                }
                return Verdict::Fault(ConstFault::ThrowsException, span);
            }
            Mod => {
                // Floor-mod for doubles too (`5.0 % -3.0 == -1.0`).
                let r = x % y;
                let r = if r != 0.0 && (r < 0.0) != (y < 0.0) {
                    r + y
                } else {
                    r
                };
                return Verdict::Value(ConstantValue::Double(r));
            }
            Lt => return Verdict::Value(ConstantValue::Bool(x < y)),
            Gt => return Verdict::Value(ConstantValue::Bool(x > y)),
            Le => return Verdict::Value(ConstantValue::Bool(x <= y)),
            Ge => return Verdict::Value(ConstantValue::Bool(x >= y)),
            _ => unreachable!("non-arithmetic op in numeric kernel"),
        };
        Verdict::Value(ConstantValue::Double(v))
    }
}

fn as_f64(v: &ConstantValue) -> f64 {
    match v {
        ConstantValue::Int(x) => *x as f64,
        ConstantValue::Double(x) => *x,
        _ => unreachable!("numeric kernel fed a non-num"),
    }
}

// —— capture: the parser-event fold ——————————————————————————————————————

/// Which const context a window brackets; the kind decides the reporting
/// site's default diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowKind {
    /// A `const` variable/field initializer: the root verdict reports
    /// `const_initialized_with_non_constant_value` (inner faults override).
    Initializer,
    /// An enum constant: each constructor argument reports
    /// `const_with_non_constant_argument`.
    EnumElement,
    /// An explicit `const C(...)`: each argument reports
    /// `const_with_non_constant_argument` (only when the target is a
    /// provably const constructor).
    CtorInvocation,
    /// An explicit `const [...]` / `{...}` / `(...)`: each element reports
    /// `non_constant_list_element` / `_set_element` / `_map_element`.
    ExplicitLiteral,
    /// A NON-const set/map literal (#747): its elements constant-evaluate
    /// for duplicate detection (`equal_elements_in_set` / `equal_keys_in_map`,
    /// `best_practices_verifier.dart:1077-1109`). Never opens inside a const
    /// window — an implicitly const literal's duplicate face is the
    /// ERROR-tier const twins, not this wave's.
    NonConstDup,
    /// A NON-const instance creation's argument list whose constructor
    /// carries the meta `@literal` annotation (#749): the arguments
    /// constant-evaluate for the `canBeConst` face of
    /// `non_const_call_to_literal_constructor`
    /// (`best_practices_verifier.dart:1276-1290`). Opens under the same
    /// nesting rule as [`WindowKind::NonConstDup`] — never inside a const
    /// window, where the creation is implicitly `const` and the oracle is
    /// silent.
    LiteralCtor,
}

struct Window {
    kind: WindowKind,
    /// Operand-stack length at open — every pop/truncate clamps here.
    base: usize,
    /// Spread/if/for elements seen inside the window (only the
    /// [`WindowKind::NonConstDup`] consumer reads it: the oracle's
    /// `whereType<Expression>` excludes those elements, and their folded
    /// operands cannot be told apart here — the dup check skips the whole
    /// literal instead, under-reporting by design).
    ctl_elements: u32,
}

/// One open argument list: the callee node latched at `begin_arguments`
/// (the stack node just below the base), the operand mark, and the
/// collected arguments.
struct ArgLevel {
    callee: Option<Node>,
    mark: usize,
    named_pending: bool,
    args: Vec<Node>,
    /// `windows.len()` at open — an argument list is its window's own
    /// when no level of the same window remains below it.
    window_seq: usize,
}

/// The listener-facing capture half: folds expression events into a tree
/// while a const context is open. Every method is a no-op with no window
/// open, so non-const code pays one `windows.is_empty()` check per event.
pub struct ConstCapture {
    identical_id: StringId,
    windows: Vec<Window>,
    stack: Vec<Node>,
    /// Paren/argument/string nesting (the prefix-application depth).
    depth: u32,
    /// Prefix operators awaiting their operand, with latch depth.
    pending_unary: Vec<(UnOp, u32)>,
    /// `(op, stack len at begin, depth)` per open binary operator.
    bin_latches: Vec<(BinOp, usize, u32)>,
    /// `(stack len at begin, depth)` per open conditional.
    cond_latches: Vec<(usize, u32)>,
    args: Vec<ArgLevel>,
    /// Per open string literal: the collected `${...}` operands.
    string_interps: Vec<Vec<Node>>,
    /// Operand-stack length at each `begin_literal_string`.
    string_marks: Vec<usize>,
    /// Latched stack lengths of open opaque groups (`await`, `new`,
    /// cascades, function literals): body debris truncates away.
    opaque_marks: Vec<usize>,
}

/// The closed argument list of a call whose const-ness the engine
/// decides: the bare callee name when the head was an identifier (call
/// classification needs resolution), the call span, and whether the list
/// was the window's own creation (its gate lives until the window
/// closes).
pub struct ClosedArgs {
    pub callee: Option<StringId>,
    pub span: SourceRange,
    pub creation_owned: bool,
    pub ctor_invocation: bool,
    pub args: Vec<Node>,
}

/// The shape a set-or-map literal resolved to, with the parts whose
/// const-ness the explicit window checks.
pub enum SetOrMap {
    Set(Vec<Node>),
    Map(Vec<(Node, Node)>),
}

/// Splits the popped operands into entries vs bare elements; the
/// parser's `has_set_entry` is the official `hasSetEntry`: true when the
/// first simple entry had NO colon (a set), false with a colon, and
/// false-for-empty `{}` — the official `isMap` default is map.
fn split_set_or_map(staged: Vec<Node>, has_set_entry: bool) -> SetOrMap {
    let mut entries = Vec::new();
    let mut elements = Vec::new();
    for node in staged {
        match node.expr {
            Expr::Entry(k, v) => entries.push((*k, *v)),
            _ => elements.push(node),
        }
    }
    if has_set_entry {
        SetOrMap::Set(elements)
    } else {
        SetOrMap::Map(entries)
    }
}

impl ConstCapture {
    /// `identical_id` is the interned `identical` spelling — the one
    /// invocation the subset evaluates.
    pub fn new(identical_id: StringId) -> Self {
        Self {
            identical_id,
            windows: Vec::new(),
            stack: Vec::new(),
            depth: 0,
            pending_unary: Vec::new(),
            bin_latches: Vec::new(),
            cond_latches: Vec::new(),
            args: Vec::new(),
            string_interps: Vec::new(),
            string_marks: Vec::new(),
            opaque_marks: Vec::new(),
        }
    }

    pub fn open(&self) -> bool {
        !self.windows.is_empty()
    }

    fn base(&self) -> usize {
        self.windows.last().map_or(0, |w| w.base)
    }

    // —— windows ————————————————————————————————————————————————————

    pub fn open_initializer(&mut self) {
        self.open_window(WindowKind::Initializer);
    }

    /// Opens an enum-constant window; a still-open element closes first
    /// (`handle_enum_element` doubles as the previous element's end).
    pub fn open_enum_element(&mut self) {
        if self
            .windows
            .last()
            .is_some_and(|w| w.kind == WindowKind::EnumElement)
        {
            self.discard_window();
        }
        self.open_window(WindowKind::EnumElement);
    }

    pub fn open_ctor_invocation(&mut self) {
        self.open_window(WindowKind::CtorInvocation);
    }

    pub fn open_explicit_literal(&mut self) {
        self.open_window(WindowKind::ExplicitLiteral);
    }

    /// Opens a non-const duplicate window for a set/map literal WITHOUT a
    /// `const` keyword. Nested non-const set/map literals each get their
    /// own window (the oracle's visitor checks every literal node);
    /// inside a const window the literal is implicitly const and the
    /// const path owns it — nothing opens.
    pub fn open_non_const_dup(&mut self) {
        match self.windows.last() {
            None
            | Some(Window {
                kind: WindowKind::NonConstDup,
                ..
            }) => {
                self.open_window(WindowKind::NonConstDup);
            }
            Some(_) => {}
        }
    }

    /// Opens a literal-constructor candidate window (#749) for a NON-const
    /// instance creation's argument list. Same nesting rule as
    /// [`Self::open_non_const_dup`], plus sibling-kind nesting: a candidate
    /// may open inside another candidate's or a dup literal's window (the
    /// oracle checks every creation node — `Point(Point(1, 2), 3)` reports
    /// the inner face), while a const window's implicit const-ness owns
    /// everything below it. Returns whether the window opened (the caller
    /// latches ownership only then).
    pub fn open_literal_ctor(&mut self) -> bool {
        match self.windows.last() {
            None
            | Some(Window {
                kind: WindowKind::NonConstDup | WindowKind::LiteralCtor,
                ..
            }) => {
                self.open_window(WindowKind::LiteralCtor);
                true
            }
            Some(_) => false,
        }
    }

    /// Whether a literal-constructor candidate window is innermost.
    pub fn top_is_literal_ctor(&self) -> bool {
        self.windows
            .last()
            .is_some_and(|w| w.kind == WindowKind::LiteralCtor)
    }

    /// Closes a literal-constructor candidate window, returning its
    /// captured argument trees (empty without such a window on top). A
    /// candidate nested in an enclosing candidate or dup literal leaves the
    /// non-constant placeholder on the parent stack
    /// ([`Self::non_const_dup_placeholder`]'s shape widened to both
    /// non-const kinds): the parent's `canBeConst`/dup face must read the
    /// nested creation as not constant-evaluatable, never as a deferring
    /// constant.
    pub fn close_literal_ctor(&mut self, span: SourceRange) -> Vec<Node> {
        if !self.top_is_literal_ctor() {
            return Vec::new();
        }
        self.apply_unaries_at(self.depth);
        let base = self.base();
        let args = self.stack.split_off(base);
        self.drop_latches_above(base);
        self.windows.pop();
        if self
            .windows
            .last()
            .is_some_and(|w| matches!(w.kind, WindowKind::NonConstDup | WindowKind::LiteralCtor))
        {
            self.push(
                Node {
                    expr: Expr::Other,
                    span,
                },
                false,
            );
        }
        args
    }

    /// A spread/if/for element opened inside the innermost window (the
    /// dup check's wholesale-skip latch; no other window kind reads it).
    pub fn begin_ctl_element(&mut self) {
        if let Some(window) = self.windows.last_mut() {
            if window.kind == WindowKind::NonConstDup {
                window.ctl_elements += 1;
            }
        }
    }

    fn open_window(&mut self, kind: WindowKind) {
        self.windows.push(Window {
            kind,
            base: self.stack.len(),
            ctl_elements: 0,
        });
    }

    /// Closes an initializer window, returning its root node for the
    /// caller to evaluate (the engine supplies the [`ConstLookup`], so
    /// the borrows stay split).
    pub fn close_initializer(&mut self) -> Option<Node> {
        self.apply_unaries();
        let root = self.take_root();
        self.discard_window();
        root
    }

    pub fn close_enum_element(&mut self) {
        self.discard_window();
    }

    pub fn close_ctor_invocation(&mut self) {
        self.apply_unaries();
        self.keep_top_above_base();
        self.windows.pop();
    }

    /// Closes an explicit-const literal: its (already element-checked)
    /// value node stays on the stack for the parent window.
    pub fn close_explicit_literal(&mut self) {
        self.apply_unaries_at(self.depth);
        self.keep_top_above_base();
        self.windows.pop();
    }

    /// Closes a non-const duplicate window: the literal's parts return
    /// (bare operands are set elements, staged `Entry` nodes are map
    /// entries, `{}` is a map) with a flag telling whether spread/if/for
    /// elements occurred — the dup check skips those literals wholesale.
    /// `None` unless a [`WindowKind::NonConstDup`] window is on top (the
    /// const windows close through [`Self::literal_set_or_map`]). A
    /// literal nested in an enclosing dup literal leaves an opaque
    /// placeholder on the parent stack ([`Self::non_const_dup_placeholder`]).
    pub fn close_non_const_dup(
        &mut self,
        count: usize,
        span: SourceRange,
        has_set_entry: bool,
    ) -> Option<(SetOrMap, bool)> {
        if !self.top_is_non_const_dup() {
            return None;
        }
        let has_ctl = self.windows.last().is_some_and(|w| w.ctl_elements > 0);
        self.apply_unaries_at(self.depth);
        let staged = self.pop_operands(count);
        let shape = split_set_or_map(staged, has_set_entry);
        self.discard_window();
        self.non_const_dup_placeholder(span);
        Some((shape, has_ctl))
    }

    /// Leaves the opaque element a drained foreign literal (a nested
    /// set/map/list/record literal) contributes to an enclosing
    /// [`WindowKind::NonConstDup`] stack: without it the parent's
    /// element/entry pairing would drift (a map value slot would swallow
    /// the next key). The dup check skips the node — the oracle's
    /// `computeConstantValue` fails on the same shapes. No-op unless a
    /// NonConstDup window is on top.
    fn non_const_dup_placeholder(&mut self, span: SourceRange) {
        if self.top_is_non_const_dup() {
            self.push(
                Node {
                    expr: Expr::Other,
                    span,
                },
                false,
            );
        }
    }

    fn top_is_non_const_dup(&self) -> bool {
        self.windows
            .last()
            .is_some_and(|w| w.kind == WindowKind::NonConstDup)
    }

    /// The root node above the window base, clamping recovery debris.
    fn take_root(&mut self) -> Option<Node> {
        let base = self.base();
        if self.stack.len() > base + 1 {
            let top = self.stack.pop().expect("len > base");
            self.stack.truncate(base);
            self.stack.push(top);
        }
        self.stack.pop()
    }

    /// Clamps recovery debris: keeps at most one operand above the window
    /// base.
    fn keep_top_above_base(&mut self) {
        let base = self.base();
        if self.stack.len() > base + 1 {
            let top = self.stack.pop().expect("len > base");
            self.stack.truncate(base);
            self.stack.push(top);
        }
    }

    fn discard_window(&mut self) {
        self.apply_unaries();
        let base = self.base();
        self.stack.truncate(base);
        self.drop_latches_above(base);
        self.windows.pop();
    }

    /// Drops every latch whose latched stack position the closed window
    /// owned (a window brackets a complete expression; only recovery
    /// debris can leave latches behind).
    fn drop_latches_above(&mut self, floor: usize) {
        self.bin_latches.retain(|&(_, len, _)| len < floor);
        self.cond_latches.retain(|&(len, _)| len < floor);
    }

    // —— leaves ——————————————————————————————————————————————————————

    /// A literal leaf (`value` already parsed from the token text by the
    /// engine); completes any pending prefix operators.
    pub fn literal(&mut self, span: SourceRange, value: ConstantValue) {
        if !self.open() {
            return;
        }
        self.push(
            Node {
                expr: Expr::Value(value),
                span,
            },
            true,
        );
    }

    pub fn identifier(&mut self, name: StringId, span: SourceRange) {
        if !self.open() {
            return;
        }
        self.push(
            Node {
                expr: Expr::Ident(name),
                span,
            },
            false,
        );
    }

    /// `#foo` — a valid constant of opaque value.
    pub fn symbol(&mut self, span: SourceRange) {
        if self.open() {
            self.push(
                Node {
                    expr: Expr::Value(ConstantValue::Instance(InstanceKind::Symbol)),
                    span,
                },
                true,
            );
        }
    }

    /// `this` / `super` — provably non-constant.
    pub fn this_or_super(&mut self, span: SourceRange) {
        if self.open() {
            self.push(
                Node {
                    expr: Expr::Other,
                    span,
                },
                false,
            );
        }
    }

    // —— operators ———————————————————————————————————————————————————

    pub fn unary_prefix(&mut self, op: UnOp) {
        if self.open() {
            self.pending_unary.push((op, self.depth));
        }
    }

    /// A postfix `++`/`--` (its operand is already pushed).
    pub fn unary_postfix(&mut self, span: SourceRange) {
        if self.open() {
            let base = self.base();
            self.pop_clamped(base);
            self.push(
                Node {
                    expr: Expr::Other,
                    span,
                },
                true,
            );
        }
    }

    pub fn begin_binary(&mut self, op: BinOp, span: SourceRange) {
        if !self.open() {
            return;
        }
        let _ = span;
        self.apply_unaries_at(self.depth);
        // The left operand is on the stack when the operator latches: the
        // floor sits below it.
        self.bin_latches
            .push((op, self.stack.len().saturating_sub(1), self.depth));
    }

    pub fn end_binary(&mut self, end: SourceRange) {
        if !self.open() {
            return;
        }
        self.apply_unaries_at(self.depth);
        let Some((op, mark, _)) = self.bin_latches.pop() else {
            return;
        };
        let (right, left) = match (self.pop_clamped(mark), self.pop_clamped(mark)) {
            (Some(r), Some(l)) => (r, l),
            (Some(r), None) => {
                let span = r.span;
                (
                    r,
                    Node {
                        expr: Expr::Other,
                        span,
                    },
                )
            }
            (None, _) => return,
        };
        let span = SourceRange {
            offset: left.span.offset,
            length: end.end().saturating_sub(left.span.offset),
        };
        self.push(
            Node {
                expr: Expr::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            },
            true,
        );
    }

    pub fn begin_conditional(&mut self) {
        if self.open() {
            self.apply_unaries_at(self.depth);
            // The condition is on the stack when the `?` latches: the
            // floor sits below it.
            self.cond_latches
                .push((self.stack.len().saturating_sub(1), self.depth));
        }
    }

    pub fn conditional_colon(&mut self) {
        if self.open() {
            self.apply_unaries_at(self.depth);
        }
    }

    pub fn end_conditional(&mut self, end: SourceRange) {
        if !self.open() {
            return;
        }
        self.apply_unaries_at(self.depth);
        let Some((mark, _)) = self.cond_latches.pop() else {
            return;
        };
        let (els, then, cond) = match (
            self.pop_clamped(mark),
            self.pop_clamped(mark),
            self.pop_clamped(mark),
        ) {
            (Some(e), Some(t), Some(c)) => (e, t, c),
            _ => return,
        };
        let span = SourceRange {
            offset: cond.span.offset,
            length: end.end().saturating_sub(cond.span.offset),
        };
        self.push(
            Node {
                expr: Expr::Cond {
                    cond: Box::new(cond),
                    then: Box::new(then),
                    els: Box::new(els),
                },
                span,
            },
            true,
        );
    }

    /// `receiver.member` — merges the top two operands.
    pub fn dot_access(&mut self, is_null_aware: bool, end: SourceRange) {
        if !self.open() {
            return;
        }
        self.apply_unaries_at(self.depth);
        let base = self.base();
        let Some(name) = self.pop_clamped(base) else {
            return;
        };
        let Expr::Ident(name) = name.expr else {
            // A malformed receiver part (recovery): restore and drop.
            self.stack.push(name);
            return;
        };
        if is_null_aware {
            self.pop_clamped(base);
            self.push(
                Node {
                    expr: Expr::Other,
                    span: end,
                },
                true,
            );
            return;
        }
        let Some(recv) = self.pop_clamped(base) else {
            return;
        };
        let span = SourceRange {
            offset: recv.span.offset,
            length: end.end().saturating_sub(recv.span.offset),
        };
        self.push(
            Node {
                expr: Expr::Member {
                    recv: Box::new(recv),
                    name,
                },
                span,
            },
            true,
        );
    }

    /// Wraps the top operand with every prefix operator whose operand
    /// just completed (latched at the current depth).
    fn apply_unaries_at(&mut self, depth: u32) {
        while self.pending_unary.last().is_some_and(|&(_, d)| d == depth) {
            let Some(mut node) = self.pop_clamped(self.base()) else {
                self.pending_unary.pop();
                continue;
            };
            let (op, _) = self.pending_unary.pop().expect("checked above");
            let span = node.span;
            node = Node {
                expr: Expr::Unary {
                    op,
                    operand: Box::new(node),
                },
                span,
            };
            self.stack.push(node);
        }
    }

    /// Window teardown: applies every surviving prefix (recovery debris
    /// has no completing event).
    fn apply_unaries(&mut self) {
        while let Some((op, _)) = self.pending_unary.pop() {
            if let Some(mut node) = self.pop_clamped(self.base()) {
                let span = node.span;
                node = Node {
                    expr: Expr::Unary {
                        op,
                        operand: Box::new(node),
                    },
                    span,
                };
                self.stack.push(node);
            }
        }
    }

    // —— strings —————————————————————————————————————————————————————

    pub fn begin_literal_string(&mut self) {
        if self.open() {
            self.depth += 1;
            self.string_interps.push(Vec::new());
            self.string_marks.push(self.stack.len());
        }
    }

    /// One `${expr}` / `$id` — its operand is on the stack; `span` is the
    /// interpolation's own span (the recovery fallback anchor).
    pub fn interpolation_expression(&mut self, span: SourceRange) {
        if !self.open() {
            return;
        }
        self.apply_unaries_at(self.depth);
        let mark = *self.string_marks.last().unwrap_or(&0);
        let part = self.pop_clamped(mark).unwrap_or(Node {
            expr: Expr::Other,
            span,
        });
        if let Some(parts) = self.string_interps.last_mut() {
            parts.push(part);
        }
    }

    /// Closes one string literal; `text` is the raw-interior leaf for the
    /// interpolation-free form (unobservable, see the module docs).
    pub fn end_literal_string(
        &mut self,
        count: usize,
        begin: SourceRange,
        end: SourceRange,
        text: ConstantValue,
    ) {
        if !self.open() {
            return;
        }
        self.apply_unaries_at(self.depth - 1);
        self.depth -= 1;
        self.string_marks.pop();
        let parts = self.string_interps.pop().unwrap_or_default();
        let span = SourceRange {
            offset: begin.offset,
            length: end.end().saturating_sub(begin.offset),
        };
        let expr = if count == 0 {
            Expr::Value(text)
        } else {
            Expr::Interp { parts }
        };
        self.push(Node { expr, span }, true);
    }

    /// Adjacent string literals — merges the operands.
    pub fn adjacent_strings(&mut self, count: usize, span: SourceRange) {
        if !self.open() {
            return;
        }
        self.apply_unaries_at(self.depth);
        let base = self.base();
        let mut parts = Vec::new();
        for _ in 0..count {
            if let Some(part) = self.pop_clamped(base) {
                parts.push(part);
            }
        }
        parts.reverse();
        self.push(
            Node {
                expr: Expr::Adjacent(parts),
                span,
            },
            true,
        );
    }

    // —— collections and records —————————————————————————————————————

    /// A list literal (explicit `const [...]` or implicit in a const
    /// context): its elements return for per-element checking; the caller
    /// pushes the opaque value node afterwards. `None` with no window.
    /// Under a non-const duplicate window the literal is a foreign element
    /// of the enclosing dup literal, not a const-checked face: its
    /// operands drain and nothing checks.
    pub fn literal_list(&mut self, count: usize, _span: SourceRange) -> Option<Vec<Node>> {
        if !self.open() {
            return None;
        }
        if self.top_is_non_const_dup() {
            self.apply_unaries_at(self.depth);
            self.pop_operands(count);
            self.non_const_dup_placeholder(_span);
            return None;
        }
        self.apply_unaries_at(self.depth);
        Some(self.pop_operands(count))
    }

    /// A set-or-map literal (explicit or implicit const): staged `Entry`
    /// pseudo-operands are map entries, bare operands set elements; the
    /// parts return for per-element checking. `{}` is a map (the official
    /// `isMap` default). Non-const duplicate windows never reach here —
    /// [`Self::close_non_const_dup`] owns them (the engine's literal
    /// handler tries it first).
    pub fn literal_set_or_map(
        &mut self,
        count: usize,
        _span: SourceRange,
        has_set_entry: bool,
    ) -> Option<SetOrMap> {
        if !self.open() || self.top_is_non_const_dup() {
            return None;
        }
        self.apply_unaries_at(self.depth);
        let staged = self.pop_operands(count);
        Some(split_set_or_map(staged, has_set_entry))
    }

    /// A map entry: stages key and value as one pseudo-operand.
    pub fn literal_map_entry(&mut self, span: SourceRange) {
        if !self.open() {
            return;
        }
        self.apply_unaries_at(self.depth);
        let base = self.base();
        let value = self.pop_clamped(base);
        let key = self.pop_clamped(base);
        match (key, value) {
            (Some(k), Some(v)) => {
                self.push(
                    Node {
                        expr: Expr::Entry(Box::new(k), Box::new(v)),
                        span,
                    },
                    false,
                );
            }
            (None, Some(v)) | (Some(v), None) => self.push(v, false),
            (None, None) => {}
        }
    }

    /// A record literal (explicit or implicit const): its fields return
    /// for per-element checking. Under a non-const duplicate window the
    /// record is a foreign element of the enclosing dup literal: its
    /// operands drain and nothing checks.
    pub fn record_literal(&mut self, count: usize, _span: SourceRange) -> Option<Vec<Node>> {
        if !self.open() {
            return None;
        }
        if self.top_is_non_const_dup() {
            self.apply_unaries_at(self.depth);
            self.pop_operands(count);
            self.non_const_dup_placeholder(_span);
            return None;
        }
        self.apply_unaries_at(self.depth);
        Some(self.pop_operands(count))
    }

    fn pop_operands(&mut self, count: usize) -> Vec<Node> {
        let base = self.base();
        let mut nodes = Vec::new();
        for _ in 0..count {
            if let Some(node) = self.pop_clamped(base) {
                nodes.push(node);
            }
        }
        nodes.reverse();
        nodes
    }

    // —— calls ————————————————————————————————————————————————————————

    pub fn begin_arguments(&mut self) {
        if !self.open() {
            return;
        }
        self.depth += 1;
        let callee = self.stack.last().cloned();
        let base = self.base();
        // The callee leaves the operand stack: it never merges with the
        // arguments.
        if self.stack.len() > base {
            self.stack.pop();
        }
        self.args.push(ArgLevel {
            callee,
            mark: self.stack.len(),
            named_pending: false,
            args: Vec::new(),
            window_seq: self.windows.len(),
        });
    }

    /// A named-argument label (fires before the value).
    pub fn named_argument(&mut self) {
        if let Some(level) = self.args.last_mut() {
            level.named_pending = true;
            level.mark = self.stack.len();
        }
    }

    /// A positional argument (fires after the value).
    pub fn positional_argument(&mut self) {
        if self.open() {
            self.apply_unaries_at(self.depth);
            self.flush_argument();
        }
    }

    fn flush_argument(&mut self) {
        let Some(level) = self.args.last_mut() else {
            return;
        };
        level.named_pending = false;
        let mark = level.mark;
        let mut nodes = self.stack.split_off(mark.min(self.stack.len()));
        level.mark = self.stack.len();
        if nodes.len() == 1 {
            level.args.push(nodes.pop().expect("len 1"));
        } else if !nodes.is_empty() {
            // A compound window (recovery): the argument is not constant.
            let span = nodes[0].span;
            level.args.push(Node {
                expr: Expr::Other,
                span,
            });
        }
    }

    /// Closes the argument list. Three outcomes:
    ///
    /// * the window's own creation list (enum element / explicit `const
    ///   C(...)`) returns [`ClosedArgs`] with `creation_owned` — the
    ///   engine checks the arguments and, for ctor invocations, pushes
    ///   the creation's value;
    /// * a bare-identifier callee returns [`ClosedArgs`] for the engine's
    ///   call classification (`C()` is an implicit const creation,
    ///   `f()` a method-invocation fault, unknown names defer);
    /// * every other shape (member invocations, calls inside opaque
    ///   groups) stays in the capture as a deferring leaf or debris.
    pub fn end_arguments(&mut self, end: SourceRange) -> Option<ClosedArgs> {
        if !self.open() {
            self.args.clear();
            return None;
        }
        // Inside an opaque group (a function-literal body): the group's
        // end truncates the debris — nothing here can report.
        if !self.opaque_marks.is_empty() {
            self.depth -= 1;
            self.args.pop();
            return None;
        }
        self.apply_unaries_at(self.depth);
        self.flush_argument();
        self.depth -= 1;
        let level = self.args.pop()?;
        let kind = self.windows.last().map(|w| w.kind);
        let creation = matches!(
            kind,
            Some(WindowKind::EnumElement | WindowKind::CtorInvocation)
        );
        if creation && self.args.iter().all(|l| l.window_seq < level.window_seq) {
            let callee = level.callee.as_ref();
            let span = callee.map_or(ORIGIN, |c| c.span);
            return Some(ClosedArgs {
                callee: callee.and_then(|c| head_name(c)),
                span: SourceRange {
                    offset: span.offset,
                    length: end.end().saturating_sub(span.offset),
                },
                creation_owned: true,
                ctor_invocation: kind == Some(WindowKind::CtorInvocation),
                args: level.args,
            });
        }
        if let Some(node) = level.callee {
            if let Expr::Ident(name) = node.expr {
                // `identical(a, b)` is the one call the pure fold
                // evaluates — reconstruct it in place.
                if name == self.identical_id && level.args.len() == 2 {
                    self.push(
                        Node {
                            expr: Expr::Call {
                                name,
                                args: level.args,
                            },
                            span: node.span,
                        },
                        true,
                    );
                    return None;
                }
                // A bare call inside a literal-ctor candidate window is an
                // ordinary runtime call: provably not a constant for the
                // candidate's `canBeConst` face. The const-context
                // interpretations below never apply — the window only
                // opens outside const contexts.
                if self.top_is_literal_ctor() {
                    self.push(
                        Node {
                            expr: Expr::Other,
                            span: node.span,
                        },
                        true,
                    );
                    return None;
                }
                let span = SourceRange {
                    offset: node.span.offset,
                    length: end.end().saturating_sub(node.span.offset),
                };
                return Some(ClosedArgs {
                    callee: Some(name),
                    span,
                    creation_owned: false,
                    ctor_invocation: false,
                    args: level.args,
                });
            }
            // A member invocation (`a.b(...)`) — the oracle's code there
            // is resolution-shape-specific (property access vs method),
            // out of the M2 subset: defer. Under a literal-ctor candidate
            // window there is no enclosing const window to defer to: a
            // runtime member call is simply not constant-evaluatable.
            self.push(
                Node {
                    expr: if self.top_is_literal_ctor() {
                        Expr::Other
                    } else {
                        Expr::Opaque
                    },
                    span: node.span,
                },
                true,
            );
        }
        None
    }

    // —— the non-const constructs ————————————————————————————————————

    pub fn assignment(&mut self, span: SourceRange) {
        if !self.open() {
            return;
        }
        let base = self.base();
        self.pop_clamped(base);
        self.pop_clamped(base);
        self.push(
            Node {
                expr: Expr::Other,
                span,
            },
            true,
        );
    }

    pub fn throw_expression(&mut self, span: SourceRange) {
        if self.open() {
            let base = self.base();
            self.pop_clamped(base);
            self.push(
                Node {
                    expr: Expr::Other,
                    span,
                },
                true,
            );
        }
    }

    pub fn non_null_assert(&mut self, span: SourceRange) {
        if self.open() {
            let base = self.base();
            self.pop_clamped(base);
            self.push(
                Node {
                    expr: Expr::Other,
                    span,
                },
                true,
            );
        }
    }

    /// `is` — not a constant expression (the spec's list omits it; the
    /// oracle anchors the reporting site's default at the operator).
    pub fn not_constant(&mut self, span: SourceRange) {
        if self.open() {
            self.push(
                Node {
                    expr: Expr::Other,
                    span,
                },
                true,
            );
        }
    }

    /// `as` — a valid constant of unknown value (`(1 as int) + 1` stays
    /// clean in the oracle): the fold defers through it.
    pub fn opaque_leaf(&mut self, span: SourceRange) {
        if self.open() {
            self.push(
                Node {
                    expr: Expr::Opaque,
                    span,
                },
                true,
            );
        }
    }

    /// `e1[e2]` — a spec constant the subset cannot value: consumes both
    /// operands and defers.
    pub fn indexed(&mut self, span: SourceRange) {
        if !self.open() {
            return;
        }
        let base = self.base();
        self.pop_clamped(base);
        self.pop_clamped(base);
        self.push(
            Node {
                expr: Expr::Opaque,
                span,
            },
            true,
        );
    }

    /// Pushes a known constant leaf (an opaque collection/creation value
    /// the engine built at a construct's close).
    pub fn push_value(&mut self, span: SourceRange, value: ConstantValue) {
        if self.open() {
            self.push(
                Node {
                    expr: Expr::Value(value),
                    span,
                },
                true,
            );
        }
    }

    // —— depth-bearing brackets ——————————————————————————————————————

    pub fn parenthesized_begin(&mut self) {
        if self.open() {
            self.depth += 1;
        }
    }

    pub fn parenthesized_end(&mut self) {
        if self.open() {
            self.apply_unaries_at(self.depth - 1);
            self.depth -= 1;
        }
    }

    /// `await` / `new` / cascade / function-literal open: latches the
    /// stack so body debris cannot leak into the operand stack.
    pub fn begin_opaque_group(&mut self) {
        if self.open() {
            self.depth += 1;
            self.opaque_marks.push(self.stack.len());
        }
    }

    pub fn end_opaque_group(&mut self, span: SourceRange) {
        if !self.open() {
            return;
        }
        self.apply_unaries_at(self.depth - 1);
        self.depth -= 1;
        if let Some(mark) = self.opaque_marks.pop() {
            self.stack.truncate(mark);
        }
        self.push(
            Node {
                expr: Expr::Other,
                span,
            },
            true,
        );
    }

    // —— plumbing ————————————————————————————————————————————————————

    fn push(&mut self, node: Node, applies: bool) {
        self.stack.push(node);
        if applies {
            self.apply_unaries_at(self.depth);
        }
    }

    /// Pops above `floor` (a window base or a latch mark), returning the
    /// node; `None` when recovery left nothing.
    fn pop_clamped(&mut self, floor: usize) -> Option<Node> {
        if self.stack.len() > floor {
            self.stack.pop()
        } else {
            None
        }
    }
}

/// A zero span (debris without a token to anchor to).
const ORIGIN: SourceRange = SourceRange {
    offset: 0,
    length: 0,
};

/// The container name a call head carries: `C` or `C.named` — a bare
/// identifier or a member whose receiver is one. `None` leaves the call
/// to the deferral path.
fn head_name(callee: &Node) -> Option<StringId> {
    match &callee.expr {
        Expr::Ident(name) => Some(*name),
        Expr::Member { recv, .. } => match &recv.expr {
            Expr::Ident(name) => Some(*name),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dartq_source::StringTable;

    /// A lookup over a tiny fixed world: `a` → 1, `e1`/`e2` → the same
    /// enum's constants, `nc` → provably non-const, anything else → the
    /// deferral verdict.
    struct World {
        strings: StringTable,
    }

    impl World {
        fn id(&self, name: &str) -> StringId {
            self.strings.intern(name)
        }

        fn node(&self, expr: Expr, offset: u32) -> Node {
            Node {
                expr,
                span: SourceRange { offset, length: 1 },
            }
        }

        fn value_node(&self, v: ConstantValue) -> Node {
            self.node(Expr::Value(v), 0)
        }

        fn eval(&self, node: &Node) -> Verdict {
            eval(node, self)
        }

        fn bin(&self, op: BinOp, l: ConstantValue, r: ConstantValue) -> Verdict {
            let left = self.value_node(l);
            let right = self.value_node(r);
            let node = self.node(
                Expr::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                7,
            );
            self.eval(&node)
        }
    }

    impl ConstLookup for World {
        fn ident(&self, name: StringId) -> IdentVerdict {
            if name == self.id("a") {
                IdentVerdict::Value(ConstantValue::Int(1))
            } else if name == self.id("nc") {
                IdentVerdict::NotConstant
            } else if name == self.id("e1") || name == self.id("e2") {
                IdentVerdict::Value(ConstantValue::Enum {
                    container: self.id("E"),
                    name,
                })
            } else {
                IdentVerdict::Constant
            }
        }

        fn member(&self, recv: &ConstantValue, name: StringId) -> Verdict {
            match recv {
                ConstantValue::Type(c) if *c == self.id("E") => {
                    Verdict::Value(ConstantValue::Enum {
                        container: *c,
                        name,
                    })
                }
                _ => Verdict::Constant,
            }
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

    // —— arithmetic (the `IntState`/`DoubleState` matrix) ————————————

    #[test]
    fn int_arithmetic_wraps_and_div_always_doubles() {
        let world = w();
        use BinOp::*;
        assert_eq!(
            world.bin(Add, ConstantValue::Int(1), ConstantValue::Int(2)),
            Verdict::Value(ConstantValue::Int(3))
        );
        // Dart int arithmetic wraps at 64 bits (VM semantics).
        assert_eq!(
            world.bin(Mul, ConstantValue::Int(i64::MAX), ConstantValue::Int(2)),
            Verdict::Value(ConstantValue::Int(-2))
        );
        // `/` widens even for int/int (value.dart `divide`).
        assert_eq!(
            world.bin(Div, ConstantValue::Int(3), ConstantValue::Int(2)),
            Verdict::Value(ConstantValue::Double(1.5))
        );
        // Any double operand widens the result.
        assert_eq!(
            world.bin(Add, ConstantValue::Int(1), ConstantValue::Double(0.5)),
            Verdict::Value(ConstantValue::Double(1.5))
        );
        // Unary minus wraps too (`-(-2^63)` stays `-2^63`).
        let neg = world.node(
            Expr::Unary {
                op: UnOp::Neg,
                operand: Box::new(world.value_node(ConstantValue::Int(i64::MIN))),
            },
            0,
        );
        assert_eq!(
            world.eval(&neg),
            Verdict::Value(ConstantValue::Int(i64::MIN))
        );
    }

    #[test]
    fn division_by_zero_faults_match_the_oracle() {
        let world = w();
        use BinOp::*;
        // `~/ 0` is the IntegerDivisionByZeroException throw
        // (value.dart `integerDivide`).
        assert_eq!(
            world.bin(IntDiv, ConstantValue::Int(1), ConstantValue::Int(0)),
            Verdict::Fault(ConstFault::ThrowsIdbze, span(7))
        );
        // `% 0` throws the generic exception (`remainder` falls through).
        assert_eq!(
            world.bin(Mod, ConstantValue::Int(1), ConstantValue::Int(0)),
            Verdict::Fault(ConstFault::ThrowsException, span(7))
        );
    }

    #[test]
    fn modulo_takes_the_divisor_sign_domain_like_dart() {
        let world = w();
        use BinOp::*;
        // Dart `%` takes the divisor's sign domain: `-5 % 3 == 1`.
        assert_eq!(
            world.bin(Mod, ConstantValue::Int(-5), ConstantValue::Int(3)),
            Verdict::Value(ConstantValue::Int(1))
        );
        assert_eq!(
            world.bin(Mod, ConstantValue::Int(5), ConstantValue::Int(-3)),
            Verdict::Value(ConstantValue::Int(-1))
        );
        assert_eq!(
            world.bin(Mod, ConstantValue::Double(-5.0), ConstantValue::Double(3.0)),
            Verdict::Value(ConstantValue::Double(1.0))
        );
    }

    #[test]
    fn shifts_follow_the_vm_rules() {
        let world = w();
        use BinOp::*;
        // Counts ≥ 64: `<<` zeroes, `>>` sign-fills, `>>>` zeroes.
        assert_eq!(
            world.bin(Shl, ConstantValue::Int(1), ConstantValue::Int(64)),
            Verdict::Value(ConstantValue::Int(0))
        );
        assert_eq!(
            world.bin(Shr, ConstantValue::Int(-8), ConstantValue::Int(64)),
            Verdict::Value(ConstantValue::Int(-1))
        );
        assert_eq!(
            world.bin(Ushr, ConstantValue::Int(-1), ConstantValue::Int(64)),
            Verdict::Value(ConstantValue::Int(0))
        );
        // A count beyond 2^31 defers to unknown (value.dart's UNKNOWN).
        assert_eq!(
            world.bin(Shl, ConstantValue::Int(1), ConstantValue::Int(1 << 40)),
            Verdict::Constant
        );
        // A negative count throws.
        assert_eq!(
            world.bin(Shr, ConstantValue::Int(1), ConstantValue::Int(-1)),
            Verdict::Fault(ConstFault::ThrowsException, span(7))
        );
        // `>>>` is the logical shift: `-1 >>> 60` clears the sign.
        assert_eq!(
            world.bin(Ushr, ConstantValue::Int(-1), ConstantValue::Int(60)),
            Verdict::Value(ConstantValue::Int(15))
        );
    }

    // —— operand-type faults (the assert order) ——————————————————————

    #[test]
    fn plus_faults_follow_the_state_assert_order() {
        let world = w();
        use BinOp::*;
        // String + String concatenates.
        assert_eq!(
            world.bin(
                Add,
                ConstantValue::Str("a".into()),
                ConstantValue::Str("b".into())
            ),
            Verdict::Value(ConstantValue::Str("ab".into()))
        );
        // `"a" + 1`: the right operand passes the num assert, then the
        // runtime add throws (StringState.add).
        assert_eq!(
            world.bin(Add, ConstantValue::Str("a".into()), ConstantValue::Int(1)),
            Verdict::Fault(ConstFault::ThrowsException, span(7))
        );
        // `1 + "a"`: the right operand fails `assertNumOrNull`
        // (const_eval_type_num).
        assert_eq!(
            world.bin(Add, ConstantValue::Int(1), ConstantValue::Str("a".into())),
            Verdict::Fault(ConstFault::TypeNum, span(7))
        );
        // `true + 1`: the LEFT operand fails the assert first.
        assert_eq!(
            world.bin(Add, ConstantValue::Bool(true), ConstantValue::Int(1)),
            Verdict::Fault(ConstFault::TypeNum, span(7))
        );
    }

    #[test]
    fn equality_is_primitive_and_cross_numeric() {
        let world = w();
        use BinOp::*;
        // Dart: `1 == 1.0` is true, `identical(1, 1.0)` is not.
        assert_eq!(
            world.bin(Eq, ConstantValue::Int(1), ConstantValue::Double(1.0)),
            Verdict::Value(ConstantValue::Bool(true))
        );
        // Cross-primitive `true == 1` answers false silently (patterns-era
        // isIdentical).
        assert_eq!(
            world.bin(Eq, ConstantValue::Bool(true), ConstantValue::Int(1)),
            Verdict::Value(ConstantValue::Bool(false))
        );
        // Non-primitive operands fault with primitive_equality.
        assert_eq!(
            world.bin(
                Eq,
                ConstantValue::Instance(InstanceKind::List),
                ConstantValue::Instance(InstanceKind::List)
            ),
            Verdict::Fault(ConstFault::PrimitiveEquality, span(7))
        );
        // null == null, null == 1.
        assert_eq!(
            world.bin(Eq, ConstantValue::Null, ConstantValue::Null),
            Verdict::Value(ConstantValue::Bool(true))
        );
        assert_eq!(
            world.bin(Ne, ConstantValue::Null, ConstantValue::Int(1)),
            Verdict::Value(ConstantValue::Bool(true))
        );
        // NaN equality is false (DoubleState.isIdentical).
        assert_eq!(
            world.bin(
                Eq,
                ConstantValue::Double(f64::NAN),
                ConstantValue::Double(f64::NAN)
            ),
            Verdict::Value(ConstantValue::Bool(false))
        );
    }

    #[test]
    fn bitwise_ops_split_bool_and_int() {
        let world = w();
        use BinOp::*;
        assert_eq!(
            world.bin(
                BitAnd,
                ConstantValue::Bool(true),
                ConstantValue::Bool(false)
            ),
            Verdict::Value(ConstantValue::Bool(false))
        );
        assert_eq!(
            world.bin(BitXor, ConstantValue::Int(5), ConstantValue::Int(3)),
            Verdict::Value(ConstantValue::Int(6))
        );
        // Mixed kinds fault with const_eval_type_bool_int (eagerAnd).
        assert_eq!(
            world.bin(BitAnd, ConstantValue::Bool(true), ConstantValue::Int(1)),
            Verdict::Fault(ConstFault::TypeBoolInt, span(7))
        );
        // `~` on a non-int faults with const_eval_type_int.
        let not = world.node(
            Expr::Unary {
                op: UnOp::BitNot,
                operand: Box::new(world.value_node(ConstantValue::Double(1.0))),
            },
            3,
        );
        assert_eq!(
            world.eval(&not),
            Verdict::Fault(ConstFault::TypeInt, span(3))
        );
    }

    // —— lazy operators ————————————————————————————————————————————

    #[test]
    fn logical_operators_short_circuit_like_lazyand() {
        let world = w();
        // `false && <provably non-const>`: the right operand never
        // evaluates — no diagnostic (evaluation.dart `lazyAnd`).
        let false_and_nc = world.node(
            Expr::Binary {
                op: BinOp::And,
                left: Box::new(world.value_node(ConstantValue::Bool(false))),
                right: Box::new(world.node(Expr::Ident(world.id("nc")), 9)),
            },
            7,
        );
        assert_eq!(
            world.eval(&false_and_nc),
            Verdict::Value(ConstantValue::Bool(false))
        );
        // `true || <non-const>`: same.
        let true_or_nc = world.node(
            Expr::Binary {
                op: BinOp::Or,
                left: Box::new(world.value_node(ConstantValue::Bool(true))),
                right: Box::new(world.node(Expr::Ident(world.id("nc")), 9)),
            },
            7,
        );
        assert_eq!(
            world.eval(&true_or_nc),
            Verdict::Value(ConstantValue::Bool(true))
        );
        // A non-bool left operand faults with const_eval_type_bool at the
        // operand's span.
        let int_and = world.node(
            Expr::Binary {
                op: BinOp::And,
                left: Box::new(world.value_node(ConstantValue::Int(1))),
                right: Box::new(world.value_node(ConstantValue::Bool(true))),
            },
            7,
        );
        assert_eq!(
            world.eval(&int_and),
            Verdict::Fault(ConstFault::TypeBool, span(0))
        );
        // `??`: a non-null left short-circuits; a null left takes the
        // right.
        let ifnull = world.node(
            Expr::Binary {
                op: BinOp::IfNull,
                left: Box::new(world.value_node(ConstantValue::Null)),
                right: Box::new(world.value_node(ConstantValue::Int(2))),
            },
            7,
        );
        assert_eq!(world.eval(&ifnull), Verdict::Value(ConstantValue::Int(2)));
    }

    #[test]
    fn conditional_takes_one_branch_and_checks_bool() {
        let world = w();
        // A known condition skips the dead branch's non-constancy.
        let cond = world.node(
            Expr::Cond {
                cond: Box::new(world.value_node(ConstantValue::Bool(true))),
                then: Box::new(world.value_node(ConstantValue::Int(1))),
                els: Box::new(world.node(Expr::Ident(world.id("nc")), 9)),
            },
            7,
        );
        assert_eq!(world.eval(&cond), Verdict::Value(ConstantValue::Int(1)));
        // A non-bool condition faults at the condition's span
        // (applyBooleanConversion → const_eval_type_bool).
        let bad = world.node(
            Expr::Cond {
                cond: Box::new(world.value_node(ConstantValue::Int(1))),
                then: Box::new(world.value_node(ConstantValue::Int(1))),
                els: Box::new(world.value_node(ConstantValue::Int(2))),
            },
            7,
        );
        assert_eq!(
            world.eval(&bad),
            Verdict::Fault(ConstFault::TypeBool, span(0))
        );
    }

    // —— strings, interpolation, calls ———————————————————————————————

    #[test]
    fn interpolation_requires_bool_num_string_or_null() {
        let world = w();
        // `'a${[1]}'`: a list operand faults with
        // const_eval_type_bool_num_string at the interpolation.
        let interp = world.node(
            Expr::Interp {
                parts: vec![
                    world.value_node(ConstantValue::Str("a".into())),
                    world.value_node(ConstantValue::Instance(InstanceKind::List)),
                ],
            },
            7,
        );
        assert_eq!(
            world.eval(&interp),
            Verdict::Fault(ConstFault::TypeBoolNumString, span(0))
        );
        // Adjacent strings concatenate.
        let adjacent = world.node(
            Expr::Adjacent(vec![
                world.value_node(ConstantValue::Str("a".into())),
                world.value_node(ConstantValue::Str("b".into())),
            ]),
            7,
        );
        assert_eq!(
            world.eval(&adjacent),
            Verdict::Value(ConstantValue::Str("ab".into()))
        );
        // A clean interpolation defers its value (escapes are undecoded —
        // a fabricated text would be wrong where values are observable).
        let clean = world.node(
            Expr::Interp {
                parts: vec![world.value_node(ConstantValue::Str("a".into()))],
            },
            7,
        );
        assert_eq!(world.eval(&clean), Verdict::Constant);
    }

    /// `'ab' * 3` is a spec constant the oracle keeps clean
    /// (`StringState`); the subset cannot value it, so it defers — the
    /// numeric operand fault must never fire.
    #[test]
    fn string_repetition_defers_instead_of_faulting() {
        let world = w();
        use BinOp::*;
        assert_eq!(
            world.bin(Mul, ConstantValue::Str("ab".into()), ConstantValue::Int(3)),
            Verdict::Constant
        );
        // A non-int repeat count is still not a numeric fault.
        assert_eq!(
            world.bin(
                Mul,
                ConstantValue::Str("a".into()),
                ConstantValue::Str("b".into())
            ),
            Verdict::Constant
        );
    }

    /// `as` results, index expressions, and deferred call shapes carry no
    /// verdict: an `Expr::Opaque` leaf folds to [`Verdict::Constant`] and
    /// propagates through enclosing operators.
    #[test]
    fn opaque_leaves_defer_through_enclosing_operators() {
        let world = w();
        assert_eq!(world.eval(&world.node(Expr::Opaque, 3)), Verdict::Constant);
        let add = world.node(
            Expr::Binary {
                op: BinOp::Add,
                left: Box::new(world.node(Expr::Opaque, 0)),
                right: Box::new(world.value_node(ConstantValue::Int(1))),
            },
            7,
        );
        assert_eq!(world.eval(&add), Verdict::Constant);
        // Two const-creation instances defer (field identity — and a user
        // `==` override — are beyond the subset).
        let eq = world.bin(
            BinOp::Eq,
            ConstantValue::Created(world.id("C")),
            ConstantValue::Created(world.id("C")),
        );
        assert_eq!(eq, Verdict::Constant);
        // A creation instance against a known-state operand answers false
        // (distinct runtime identities).
        let eq = world.bin(
            BinOp::Eq,
            ConstantValue::Created(world.id("C")),
            ConstantValue::Int(1),
        );
        assert_eq!(eq, Verdict::Value(ConstantValue::Bool(false)));
    }

    #[test]
    fn identical_evaluates_other_calls_do_not() {
        let world = w();
        // identical(e1, e2): same enum constant → true.
        let call = world.node(
            Expr::Call {
                name: world.id("identical"),
                args: vec![
                    world.node(Expr::Ident(world.id("e1")), 0),
                    world.node(Expr::Ident(world.id("e2")), 0),
                ],
            },
            7,
        );
        assert_eq!(
            world.eval(&call),
            Verdict::Value(ConstantValue::Bool(false))
        );
        // Any other invocation is provably non-constant.
        let other = world.node(
            Expr::Call {
                name: world.id("other"),
                args: vec![world.value_node(ConstantValue::Int(1))],
            },
            7,
        );
        assert_eq!(world.eval(&other), Verdict::NotConstant(span(7)));
    }

    // —— deferral ————————————————————————————————————————————————————

    #[test]
    fn unknown_values_propagate_without_diagnostics() {
        let world = w();
        // An opaque constant (a foreign const) in arithmetic: the result
        // defers — never a wrong verdict.
        let add_unknown = world.node(
            Expr::Binary {
                op: BinOp::Add,
                left: Box::new(world.node(Expr::Ident(world.id("foreign")), 0)),
                right: Box::new(world.value_node(ConstantValue::Int(1))),
            },
            7,
        );
        assert_eq!(world.eval(&add_unknown), Verdict::Constant);
        // An enum constant's member reads defer (synthesized faces).
        let member = world.node(
            Expr::Member {
                recv: Box::new(world.node(Expr::Ident(world.id("e1")), 0)),
                name: world.id("index"),
            },
            7,
        );
        assert_eq!(world.eval(&member), Verdict::Constant);
        // Enum constants are values with identity.
        let same = world.node(
            Expr::Binary {
                op: BinOp::Eq,
                left: Box::new(world.node(Expr::Ident(world.id("e1")), 0)),
                right: Box::new(world.node(Expr::Ident(world.id("e1")), 0)),
            },
            7,
        );
        assert_eq!(world.eval(&same), Verdict::Value(ConstantValue::Bool(true)));
    }

    #[test]
    fn not_constant_keeps_the_deepest_span() {
        let world = w();
        // `1 + nc`: the NotConstant verdict anchors at `nc` (the oracle's
        // error entity), not at the binary.
        let add = world.node(
            Expr::Binary {
                op: BinOp::Add,
                left: Box::new(world.value_node(ConstantValue::Int(1))),
                right: Box::new(world.node(Expr::Ident(world.id("nc")), 9)),
            },
            7,
        );
        assert_eq!(world.eval(&add), Verdict::NotConstant(span(9)));
    }

    /// A pathological tree (1000 nesting levels): the structural
    /// recursion terminates and the depth fuse defers beyond
    /// [`MAX_EVAL_DEPTH`] instead of overflowing the stack — the guard
    /// the engine requires of every recovery loop. A tree within the
    /// fuse still evaluates exactly.
    #[test]
    fn deep_trees_terminate() {
        let world = w();
        let mut node = world.value_node(ConstantValue::Int(1));
        for i in 0..1000 {
            node = world.node(
                Expr::Unary {
                    op: UnOp::Neg,
                    operand: Box::new(node),
                },
                i,
            );
        }
        assert_eq!(world.eval(&node), Verdict::Constant);
        let mut node = world.value_node(ConstantValue::Int(1));
        for i in 0..99 {
            node = world.node(
                Expr::Unary {
                    op: UnOp::Neg,
                    operand: Box::new(node),
                },
                i,
            );
        }
        assert_eq!(world.eval(&node), Verdict::Value(ConstantValue::Int(-1)));
    }

    fn span(offset: u32) -> SourceRange {
        SourceRange { offset, length: 1 }
    }
}
