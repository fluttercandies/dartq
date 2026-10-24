//! The M2 constant-evaluation subset (#279): a bounded,
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
    /// A symbol literal by its name (`#foo`) — equality is name equality
    /// (the upstream `SymbolState`).
    Symbol(StringId),
    /// A const list by its element values — structural equality, the
    /// upstream `ListState`.
    List(Box<[ConstantValue]>),
    /// A const set by its element values in source order (the upstream
    /// `SetState` equality is order-insensitive; source order is the
    /// written order every observed duplicate shares).
    Set(Box<[ConstantValue]>),
    /// A const map by its key/value pairs in source order (the upstream
    /// `MapState` equality is order-insensitive).
    Map(Box<[(ConstantValue, ConstantValue)]>),
    /// A const record by its fields — positional first, named pairs
    /// sorted by label so label order does not decide equality (the
    /// upstream `RecordState`: shape then per-field equality).
    Record {
        positional: Box<[ConstantValue]>,
        named: Box<[(StringId, ConstantValue)]>,
    },
    /// An opaque constant instance whose identity no M2 diagnostic
    /// observes.
    Instance(InstanceKind),
}

/// The opaque-instance kinds: function tear-offs, const-constructor
/// instances.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstanceKind {
    List,
    Set,
    Map,
    Record,
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
    /// `const_eval_throws_exception` — evaluation throws. The wrapped
    /// context face (#1876 W5) pairs this variant with the verdict's fault
    /// span (the throw site — every production site passes the operator
    /// node's span, the entity the upstream context row anchors at) and
    /// the inner formatted message, which is constant: every generic
    /// runtime path in upstream `value.dart` throws
    /// `EvaluationException(diag.constEvalThrowsException)` itself, so the
    /// inner text is that code's problemMessage. The emitter reads the
    /// pairing at `Engine::emit_const_fault`.
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
    /// A reference the resolution layer cannot name (`undefined_identifier`
    /// territory): the oracle's evaluator substitutes an unknown object, so
    /// the argument face still reports (`const_with_non_constant_argument`)
    /// while the enclosing declaration's own verdict stays silent
    /// (`hasKnownValue` false — the undefined name is the reported root
    /// cause). Consumed like [`Verdict::NotConstant`] at declaration roots
    /// and merged into it by every combining fold.
    Unresolved(SourceRange),
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
    /// Classifies `prefix.name` when `prefix` is an import prefix; `None`
    /// when it is not one (the member path decides). A prefixed name the
    /// prefix's namespace cannot answer — a broken import or a missing
    /// export — is [`IdentVerdict::Unresolved`] (the oracle's `InvalidType`
    /// reference, `isUnresolved: true`).
    fn prefixed(&self, _prefix: StringId, _name: StringId) -> Option<IdentVerdict> {
        None
    }
    /// Whether `name` is the callee spelling of `identical` (the one
    /// invocable function).
    fn is_identical(&self, name: StringId) -> bool;
    /// The oracle `InstanceState` family a [`ConstantValue::Created`]
    /// container folds to — `bool.fromEnvironment` pushes `Created(bool)`
    /// whose `!` operand face is `BoolState` (valid, value unknown), not a
    /// generic `assertBool` failure (value.dart's per-state methods).
    /// [`CreatedState::Generic`] is the default: the lookup does not model
    /// the container, so the operand asserts fault like the base states.
    fn created_state(&self, _container: StringId) -> CreatedState {
        CreatedState::Generic
    }
    /// Whether `span` anchors a leading-dot shorthand name expression
    /// (`.name` / `.new` — `DotShorthandNameExpression` /
    /// `DotShorthandConstructorInvocation`, which resolve against the
    /// context type, never a bare name). The capture folds a shorthand's
    /// member to a bare `Expr::Ident`, so a context-typed member reads as
    /// an unresolved name unless this probe reclassifies it; `false` is
    /// the default (the lookup has no source access).
    fn shorthand_context(&self, _span: SourceRange) -> bool {
        false
    }
}

/// The oracle `InstanceState` family of a [`ConstantValue::Created`]
/// container (`value.dart` 1627-2024: each state's operator methods assert
/// its own operand kind and throw the `constEval*` fault otherwise; a
/// state with an unknown value — `bool.fromEnvironment`'s `BoolState` —
/// answers the operator with an unknown-but-constant result).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreatedState {
    /// `BoolState` — `assertBool` admits it.
    Bool,
    /// `IntState` — `assertIntOrNull`/`assertNumOrNull` admit it.
    Int,
    /// `DoubleState`/`NumState` — `assertNumOrNull` admits it (the int/num
    /// distinction is unobservable at the assert gates).
    Num,
    /// `StringState` — every operand gate fails it.
    String,
    /// `GenericState`/unmodeled container — every operand gate fails it.
    Generic,
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
    /// A name the resolution layer cannot classify (`B(x)`'s undefined
    /// `x`): argument-face reportable, declaration-silent — see
    /// [`Verdict::Unresolved`].
    Unresolved,
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

/// The potentially-constant leaf scan (upstream Dart SDK source,
/// `pkg/analyzer/lib/src/dart/constant/potentially_constant.dart`,
/// `_Collector.collect`): a constructor-initializer's super/redirect
/// argument list and an assert's operands report `invalid_constant` at
/// each node that is *not potentially constant* — a shape the const
/// grammar can never produce a constant from, chiefly an unresolved or
/// non-const name expression. The scan descends into composite operands
/// (a binary's operand, a literal's element, an interpolation's `$
/// {expr}`) and collects the failing leaf's span; a whole non-constant
/// composite (`new`, a non-`identical` call, `x!`, an assignment)
/// collects at its own node.
///
/// The predicate is the weaker of the two const gates — it answers the
/// *shapes* the grammar admits (`getNotPotentiallyConstants`), not a
/// value: a bare name is potentially constant when the const resolver
/// can classify it (any of the `Value`/`Constant`/`InProgress` verdicts
/// — the oracle's "resolves to a const variable or const-constructor
/// formal"), while a `NotConstant`/`Unresolved` leaf collects. `Opaque`
/// is the deferred-valid shape (it stands in for the subset's
/// unmodeled-but-legal constructs); `Other` is the always-invalid one.
pub fn collect_not_potentially_constant(
    node: &Node,
    lookup: &dyn ConstLookup,
    out: &mut Vec<SourceRange>,
) {
    collect_npc(node, lookup, out, 0);
}

fn collect_npc(node: &Node, lookup: &dyn ConstLookup, out: &mut Vec<SourceRange>, depth: u32) {
    if depth > MAX_EVAL_DEPTH {
        return;
    }
    let depth = depth + 1;
    match &node.expr {
        Expr::Value(_) | Expr::Opaque => {}
        Expr::Other => out.push(node.span),
        Expr::Ident(name) => match lookup.ident(*name) {
            IdentVerdict::NotConstant => out.push(node.span),
            // A leading-dot shorthand (`.name`) resolves against the
            // context type upstream — `potentially_constant.dart`'s
            // `DotShorthandNameExpression` arm collects only a member the
            // type cannot answer. The lookup cannot name that type, so the
            // unresolved-name verdict is ambiguous: under-report rather
            // than invent a failure row for the valid shorthands.
            IdentVerdict::Unresolved => {
                if !lookup.shorthand_context(node.span) {
                    out.push(node.span);
                }
            }
            // A const variable, a resolvable foreign const, or the
            // in-progress root — the oracle's const-element arms all pass
            // the potentially-constant gate.
            IdentVerdict::Value(_) | IdentVerdict::Constant | IdentVerdict::InProgress => {}
        },
        Expr::Member { recv, name } => {
            // `prefix.name`: the import-prefixed face decides through the
            // same name rule (`_nameExpression` via `prefixed`); a
            // non-prefixed `recv.name` is potentially constant exactly
            // when the receiver is (a type or const object head) — the
            // member spelling itself stays silent (an enum's constant
            // resolves, and a wrong member's own face reports).
            if let Expr::Ident(prefix) = recv.expr
                && let Some(verdict) = lookup.prefixed(prefix, *name)
            {
                match verdict {
                    IdentVerdict::NotConstant | IdentVerdict::Unresolved => {
                        out.push(node.span);
                    }
                    IdentVerdict::Value(_)
                    | IdentVerdict::Constant
                    | IdentVerdict::InProgress => {}
                }
                return;
            }
            // A type-literal receiver (`E.a`, `C.v`) is a potentially
            // constant head only when the container name resolves to a
            // const-bearing element — the bare-name rule reads `recv`;
            // every other receiver recurses like a plain operand.
            collect_npc(recv, lookup, out, depth);
        }

        Expr::Binary { left, right, .. } => {
            collect_npc(left, lookup, out, depth);
            collect_npc(right, lookup, out, depth);
        }
        Expr::Unary { op, operand } => match op {
            // `++`/`--` is never a constant operator — the whole node
            // collects (the oracle's `_Collector` adds the operator
            // application, not the operand).
            UnOp::Inc => out.push(node.span),
            UnOp::Neg | UnOp::Not | UnOp::BitNot => collect_npc(operand, lookup, out, depth),
        },
        Expr::Cond { cond, then, els } => {
            collect_npc(cond, lookup, out, depth);
            collect_npc(then, lookup, out, depth);
            collect_npc(els, lookup, out, depth);
        }
        Expr::Interp { parts } => {
            for part in parts {
                collect_npc(part, lookup, out, depth);
            }
        }
        Expr::Adjacent(parts)
        | Expr::List(parts)
        | Expr::Set(parts)
        | Expr::Record(parts) => {
            for part in parts {
                collect_npc(part, lookup, out, depth);
            }
        }
        Expr::Map(entries) => {
            for (key, value) in entries {
                collect_npc(key, lookup, out, depth);
                collect_npc(value, lookup, out, depth);
            }
        }
        Expr::Entry(key, value) => {
            collect_npc(key, lookup, out, depth);
            collect_npc(value, lookup, out, depth);
        }
        // Only `identical(a, b)` is a constant call — its arguments
        // potentially-const like any operand; every other callee is the
        // whole-node not-constant shape.
        Expr::Call { name, args } if lookup.is_identical(*name) => {
            for arg in args {
                collect_npc(arg, lookup, out, depth);
            }
        }
        Expr::Call { .. } => out.push(node.span),
    }
}

#[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
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
            IdentVerdict::Unresolved => Verdict::Unresolved(node.span),
        },
        Expr::Member { recv, name } => {
            if let Expr::Ident(prefix) = recv.expr
                && let Some(verdict) = lookup.prefixed(prefix, *name)
            {
                return match verdict {
                    IdentVerdict::Value(v) => Verdict::Value(v),
                    IdentVerdict::Constant => Verdict::Constant,
                    IdentVerdict::InProgress => Verdict::Fault(ConstFault::Recursive, node.span),
                    IdentVerdict::NotConstant => Verdict::NotConstant(node.span),
                    IdentVerdict::Unresolved => Verdict::Unresolved(node.span),
                };
            }
            let r = fold(recv, lookup, depth);
            match r {
                Verdict::Value(v) if v != ConstantValue::Null => lookup.member(&v, *name),
                Verdict::Fault(..) | Verdict::NotConstant(_) | Verdict::Unresolved(_) => r,
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
                // `-null`: `assertNumOrNull` admits null and the base
                // `InstanceState.negated` then throws (value.dart) —
                // `constEvalThrowsException`, not the operand-type face.
                Verdict::Value(ConstantValue::Null) => {
                    Verdict::Fault(ConstFault::ThrowsException, node.span)
                }
                // A created num instance of unknown value (`const
                // int.fromEnvironment(...)`): the assert passes, the
                // result is a valid-but-unknown num.
                Verdict::Value(ConstantValue::Created(c)) => {
                    match lookup.created_state(c) {
                        CreatedState::Int | CreatedState::Num => Verdict::Constant,
                        _ => Verdict::Fault(ConstFault::TypeNum, node.span),
                    }
                }
                Verdict::Value(_) => Verdict::Fault(ConstFault::TypeNum, node.span),
                other => other,
            },
            UnOp::Not => match fold(operand, lookup, depth) {
                Verdict::Value(ConstantValue::Bool(v)) => Verdict::Value(ConstantValue::Bool(!v)),
                // `!null`: `NullState.logicalNot` throws (value.dart)
                // — `constEvalThrowsException`, not `constEvalTypeBool`.
                Verdict::Value(ConstantValue::Null) => {
                    Verdict::Fault(ConstFault::ThrowsException, node.span)
                }
                // A created bool instance of unknown value
                // (`bool.fromEnvironment`): `assertBool` passes, the
                // result is a valid-but-unknown bool.
                Verdict::Value(ConstantValue::Created(c)) => {
                    match lookup.created_state(c) {
                        CreatedState::Bool => Verdict::Constant,
                        _ => Verdict::Fault(ConstFault::TypeBool, node.span),
                    }
                }
                Verdict::Value(_) => Verdict::Fault(ConstFault::TypeBool, node.span),
                other => other,
            },
            UnOp::BitNot => match fold(operand, lookup, depth) {
                Verdict::Value(ConstantValue::Int(v)) => Verdict::Value(ConstantValue::Int(!v)),
                // `~null`: `assertIntOrNull` admits null and the base
                // `InstanceState.bitNot` then throws (value.dart).
                Verdict::Value(ConstantValue::Null) => {
                    Verdict::Fault(ConstFault::ThrowsException, node.span)
                }
                Verdict::Value(ConstantValue::Created(c)) => {
                    match lookup.created_state(c) {
                        CreatedState::Int => Verdict::Constant,
                        _ => Verdict::Fault(ConstFault::TypeInt, node.span),
                    }
                }
                Verdict::Value(_) => Verdict::Fault(ConstFault::TypeInt, node.span),
                other => other,
            },
        },
        Expr::Cond { cond, then, els } => match fold(cond, lookup, depth) {
            Verdict::Value(ConstantValue::Bool(true)) => fold(then, lookup, depth),
            Verdict::Value(ConstantValue::Bool(false)) => fold(els, lookup, depth),
            // A created bool instance of unknown value (`cond ? a : b` over
            // `bool.fromEnvironment`): `applyBooleanConversion` passes and
            // the condition is unknown — both branches must be constant.
            Verdict::Value(ConstantValue::Created(c))
                if lookup.created_state(c) == CreatedState::Bool =>
            {
                let t = fold(then, lookup, depth);
                let e = fold(els, lookup, depth);
                match (t, e) {
                    (Verdict::Fault(f, s), _) | (_, Verdict::Fault(f, s)) => Verdict::Fault(f, s),
                    (other @ (Verdict::NotConstant(_) | Verdict::Unresolved(_)), _) => other,
                    (_, other @ (Verdict::NotConstant(_) | Verdict::Unresolved(_))) => other,
                    _ => Verdict::Constant,
                }
            }
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
                    (other @ (Verdict::NotConstant(_) | Verdict::Unresolved(_)), _) => other,
                    (_, other @ (Verdict::NotConstant(_) | Verdict::Unresolved(_))) => other,
                    _ => Verdict::Constant,
                }
            }
            other => other,
        },
        Expr::Interp { parts } => {
            for part in parts {
                match fold(part, lookup, depth) {
                    Verdict::Value(v) if v.is_bool_num_string_or_null() => {}
                    // A created primitive instance (`'${bool.fromEnvironment
                    // (...)}'`): its `InstanceState` family answers the same
                    // `isBoolNumStringOrNull` gate.
                    Verdict::Value(ConstantValue::Created(c))
                        if lookup.created_state(c) != CreatedState::Generic => {}
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
            let mut pairs = Vec::with_capacity(entries.len());
            for (k, v) in entries {
                match (fold(k, lookup, depth), fold(v, lookup, depth)) {
                    (Verdict::Value(key), Verdict::Value(value)) => pairs.push((key, value)),
                    (
                        other @ (Verdict::Fault(..) | Verdict::NotConstant(_)
                        | Verdict::Unresolved(_)),
                        _,
                    )
                    | (
                        _,
                        other @ (Verdict::Fault(..) | Verdict::NotConstant(_)
                        | Verdict::Unresolved(_)),
                    ) => return other,
                    // One unknown-value side leaves the whole map's value
                    // unknown (the opaque-kind fallback).
                    _ => return Verdict::Value(ConstantValue::Instance(InstanceKind::Map)),
                }
            }
            Verdict::Value(ConstantValue::Map(pairs.into()))
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
#[expect(clippy::float_cmp)] // oracle identity is exact double equality (NaN cannot intern)
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
/// Folds a literal's parts in order: every part must value — one fault
/// grades the whole literal by its own verdict, and one unknown-value
/// part leaves the whole value unknown (the opaque-kind fallback).
fn eval_each(parts: &[Node], lookup: &dyn ConstLookup, kind: InstanceKind, depth: u32) -> Verdict {
    let mut values = Vec::with_capacity(parts.len());
    for part in parts {
        match fold(part, lookup, depth) {
            Verdict::Value(value) => values.push(value),
            Verdict::Constant => return Verdict::Value(ConstantValue::Instance(kind)),
            other @ (Verdict::Fault(..) | Verdict::NotConstant(_) | Verdict::Unresolved(_)) => {
                return other;
            }
        }
    }
    Verdict::Value(match kind {
        InstanceKind::List => ConstantValue::List(values.into()),
        InstanceKind::Set => ConstantValue::Set(values.into()),
        // Records reach eval label-less (the Expr tree keeps no labels —
        // only the capture's value push sees them): every field rides
        // positional.
        InstanceKind::Record => ConstantValue::Record {
            positional: values.into(),
            named: Box::default(),
        },
        _ => ConstantValue::Instance(kind),
    })
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
                            r @ (Verdict::Fault(..)
                            | Verdict::NotConstant(_)
                            | Verdict::Unresolved(_)) => r,
                            _ => Verdict::Constant,
                        }
                    }
                }
                // An unknown bool left (`bool.fromEnvironment && x`):
                // `BoolState.logicalAnd`/`logicalOr` assert the right is
                // bool — the unknown-left value itself never decides, so
                // the right's own verdict carries the result.
                Verdict::Value(ConstantValue::Created(c))
                    if lookup.created_state(c) == CreatedState::Bool =>
                {
                    match fold(right, lookup, depth) {
                        Verdict::Value(ConstantValue::Bool(_)) => Verdict::Constant,
                        Verdict::Value(ConstantValue::Created(rc))
                            if lookup.created_state(rc) == CreatedState::Bool =>
                        {
                            Verdict::Constant
                        }
                        Verdict::Value(_) => Verdict::Fault(ConstFault::TypeBool, right.span),
                        Verdict::Fault(f, s) => Verdict::Fault(f, s),
                        r @ (Verdict::NotConstant(_) | Verdict::Unresolved(_)) => r,
                        Verdict::Constant => Verdict::Constant,
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
                // A failed operand propagates with its own grade, left
                // first (the oracle evaluates operands in order): a
                // genuinely-known-invalid leaf and an unresolved leaf
                // carry different faces at the argument check —
                // `constWithNonConstantArgument` vs `invalidConstant`
                // (upstream Dart SDK source, pkg/analyzer
                // lib/src/dart/constant/value.dart `genericError`).
                (other @ (Verdict::NotConstant(_) | Verdict::Unresolved(_)), _) => other,
                (_, other @ (Verdict::NotConstant(_) | Verdict::Unresolved(_))) => other,
                _ => Verdict::Constant,
            }
        }
    }
}

/// The eager operator matrix, mirroring `DartObjectImpl`'s per-state
/// methods (`value.dart`): the operand-type asserts decide which
/// `constEval*` fault fires, in the oracle's assert order (left first).
#[expect(
    clippy::too_many_lines,
    clippy::many_single_char_names,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::float_cmp
)] // operator table mirrors value.dart's listing; VM casts, exact equality
fn binary_values(op: BinOp, span: SourceRange, a: &ConstantValue, b: &ConstantValue) -> Verdict {
    // Every operator variant appears in the matrix below; the explicit
    // list keeps the `enum_glob_use` gate honest.
    use BinOp::{
        Add, And, BitAnd, BitOr, BitXor, Div, Eq, Ge, Gt, IfNull, IntDiv, Le, Lt, Mod, Mul, Ne, Or,
        Shl, Shr, Sub, Ushr,
    };
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
            (ConstantValue::Int(x), ConstantValue::Int(y)) => {
                Verdict::Value(ConstantValue::Int(match op {
                    BitAnd => x & y,
                    BitOr => x | y,
                    _ => x ^ y,
                }))
            }
            (
                ConstantValue::Null,
                ConstantValue::Bool(_) | ConstantValue::Int(_) | ConstantValue::Null,
            )
            | (ConstantValue::Bool(_) | ConstantValue::Int(_), ConstantValue::Null) => {
                Verdict::Constant
            }
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
                // Any remaining pair with a null, const-creation, or
                // non-comparable-value side answers false silently
                // (`BoolState.isIdentical` falls through to FALSE).
                (
                    ConstantValue::Null
                    | ConstantValue::Created(_)
                    | ConstantValue::Bool(_)
                    | ConstantValue::Int(_)
                    | ConstantValue::Double(_)
                    | ConstantValue::Str(_)
                    | ConstantValue::Enum { .. },
                    _,
                )
                | (
                    _,
                    ConstantValue::Null
                    | ConstantValue::Created(_)
                    | ConstantValue::Bool(_)
                    | ConstantValue::Int(_)
                    | ConstantValue::Double(_)
                    | ConstantValue::Str(_)
                    | ConstantValue::Enum { .. },
                ) => false,
                // Collections, type literals, symbols, records lack
                // primitive equality.
                _ => return Verdict::Fault(ConstFault::PrimitiveEquality, span),
            };
            Verdict::Value(ConstantValue::Bool(if op == Eq { eq } else { !eq }))
        }
        // Invariant: the lazy operators are evaluated in `eval_binary`
        // (short-circuit), so this eager arm never sees them; a future
        // `BinOp` variant degrades to "not a constant" instead of panicking
        // on the emission path.
        And | Or | IfNull => Verdict::NotConstant(span),
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
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::many_single_char_names
)] // Dart int arithmetic wraps and `/` widens (VM semantics)
fn numeric(op: BinOp, span: SourceRange, a: &ConstantValue, b: &ConstantValue) -> Verdict {
    use BinOp::{Add, Div, Ge, Gt, IntDiv, Le, Lt, Mod, Mul, Sub};
    if let (ConstantValue::Int(x), ConstantValue::Int(y)) = (a, b) {
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
            // Invariant: `binary_values` routes only the arithmetic/comparison
            // ops here; a future `BinOp` variant degrades to "not a constant"
            // instead of panicking on the emission path.
            _ => return Verdict::NotConstant(span),
        };
        Verdict::Value(ConstantValue::Int(v))
    } else {
        let (Some(x), Some(y)) = (as_f64(a), as_f64(b)) else {
            // Invariant: `binary_values` feeds the kernel only num operands
            // (`is_num` gate), so both conversions succeed; a future
            // `ConstantValue` variant degrades to "not a constant".
            return Verdict::NotConstant(span);
        };
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
            // Invariant: only the arithmetic/comparison ops reach the double
            // kernel; a future `BinOp` variant degrades to "not a constant".
            _ => return Verdict::NotConstant(span),
        };
        Verdict::Value(ConstantValue::Double(v))
    }
}

#[expect(clippy::cast_precision_loss)] // span arithmetic narrows under the scanner u32 offset cap
fn as_f64(v: &ConstantValue) -> Option<f64> {
    match v {
        ConstantValue::Int(x) => Some(*x as f64),
        ConstantValue::Double(x) => Some(*x),
        // Invariant: the numeric kernel's caller gates on `is_num`, so only
        // Int/Double arrive; a future `ConstantValue` variant returns `None`
        // and the kernel degrades to "not a constant", never a panic.
        _ => None,
    }
}

// —— written expressions: the lexeme-slice face ———————————————————————————

/// The outcome of evaluating one written expression's lexeme slice
/// ([`eval_written`]) for a value-comparison face — the lint equality
/// checks upstream answers through `computeConstantValue`
/// (`avoid_redundant_argument_values.dart:71-80`).
#[derive(Clone, Debug, PartialEq)]
pub enum WrittenValue {
    /// The subset knows the value.
    Value(ConstantValue),
    /// The shape evaluates but yields no known value — a `constEval*`
    /// fault (`1 ~/ 0`) or a non-constant construct. The oracle's
    /// `hasKnownValue` is false there and nothing compares equal, so a
    /// comparison face stays silent — even against an identical spelling.
    Unknown,
    /// The subset does not cover the shape (identifiers, member accesses,
    /// calls, `const`-written collections, interpolation). The comparison
    /// face keeps its written-text comparison: identical spellings then
    /// compare equal — the resolution-free posture the textual face always
    /// had — and different spellings under-report.
    NotEvaluable,
}

/// The written-slice bounds: beyond either, the slice answers
/// [`WrittenValue::NotEvaluable`] — no real default value or argument
/// needs value equality at that size, and the caps keep the parse and the
/// built tree bounded on machine-generated input (the engine-wide
/// termination invariant; the fold's own [`MAX_EVAL_DEPTH`] still bounds
/// the value recursion under them).
const MAX_WRITE_TOKENS: usize = 1024;
const MAX_WRITE_DEPTH: u32 = 128;

/// Evaluates one written constant expression from its token lexemes: the
/// from-tokens sibling of [`eval`] for callers that hold a token span, not
/// a captured event tree (a lint reading a written argument against a
/// written default). The subset covers the resolution-free constant
/// faces — numeric/string/bool/null literals (raw and cooked strings,
/// escape-decoded, adjacent runs concatenated), parenthesized and
/// conditional expressions, and Dart's binary/unary operator set — with
/// the operator semantics evaluated by the same [`fold`] the captured
/// trees take, so the two faces cannot drift. Everything else — any name
/// the slice spells — answers [`WrittenValue::NotEvaluable`]: the slice
/// alone cannot resolve it.
#[must_use]
pub fn eval_written(lexemes: &[&str]) -> WrittenValue {
    if lexemes.len() > MAX_WRITE_TOKENS {
        return WrittenValue::NotEvaluable;
    }
    let mut written = Written {
        lexemes,
        pos: 0,
        depth: 0,
    };
    let Some(expr) = written.conditional() else {
        return WrittenValue::NotEvaluable;
    };
    if written.pos != lexemes.len() {
        return WrittenValue::NotEvaluable;
    }
    match eval(&expr, &WrittenLookup) {
        Verdict::Value(v) => WrittenValue::Value(v),
        // The depth fuse and the subset's deferral shapes answer Constant:
        // a valid constant of unknown value, the same posture a
        // not-covered shape takes here (identical spellings still compare
        // equal through the caller's textual fallback, and they carry the
        // same value — the subset is resolution-free).
        Verdict::Constant => WrittenValue::NotEvaluable,
        Verdict::Fault(..) | Verdict::NotConstant(_) | Verdict::Unresolved(_) => {
            WrittenValue::Unknown
        }
    }
}

/// The lookup [`eval_written`]'s trees never consult: the subset builds no
/// `Ident`/`Member`/`Call` nodes (an unresolved name bails at the parse),
/// so every arm is a conservative default that keeps the trait object
/// total.
struct WrittenLookup;

impl ConstLookup for WrittenLookup {
    fn ident(&self, _: StringId) -> IdentVerdict {
        IdentVerdict::Constant
    }

    fn member(&self, _: &ConstantValue, _: StringId) -> Verdict {
        Verdict::Constant
    }

    fn is_identical(&self, _: StringId) -> bool {
        false
    }
}

/// Zero span — the written face's faults carry no anchor (the comparison
/// caller maps them to [`WrittenValue::Unknown`] before any span reads).
const fn unanchored() -> SourceRange {
    SourceRange {
        offset: 0,
        length: 0,
    }
}

/// The recursive-descent half of [`eval_written`]: a Dart-precedence
/// expression parser over the lexeme slice that emits [`Expr`] trees for
/// [`fold`]. Every bail is `None` (the caller's `NotEvaluable`); the depth
/// guard bounds the descent on pathological nesting.
struct Written<'t> {
    lexemes: &'t [&'t str],
    pos: usize,
    depth: u32,
}

/// The binary precedence ladder, loosest first (the official grammar's
/// operator ordering); `?:` and `??` are handled at their own associativity
/// outside it.
const LEVELS: [&[&str]; 10] = [
    &["||"],
    &["&&"],
    &["==", "!="],
    &["<", ">", "<=", ">="],
    &["|"],
    &["^"],
    &["&"],
    &["<<", ">>", ">>>"],
    &["+", "-"],
    &["*", "/", "~/", "%"],
];

impl<'t> Written<'t> {
    /// Runs `step` one nesting level deeper, `None` past the cap (the
    /// caller's `NotEvaluable`).
    fn nested<T>(&mut self, step: fn(&mut Self) -> Option<T>) -> Option<T> {
        self.depth += 1;
        if self.depth > MAX_WRITE_DEPTH {
            self.depth -= 1;
            return None;
        }
        let out = step(self);
        self.depth -= 1;
        out
    }

    fn eat(&mut self, lexeme: &str) -> bool {
        if self.lexemes.get(self.pos) == Some(&lexeme) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// `?:` — the conditional's branches recurse through `conditional`
    /// (right-assoc, the official grammar's `expressionWithoutCascade`).
    fn conditional(&mut self) -> Option<Node> {
        let cond = Box::new(self.binary(0)?);
        if !self.eat("?") {
            return Some(*cond);
        }
        let then = Box::new(self.nested(Self::conditional)?);
        if !self.eat(":") {
            return None;
        }
        let els = Box::new(self.nested(Self::conditional)?);
        Some(Node {
            expr: Expr::Cond { cond, then, els },
            span: unanchored(),
        })
    }

    /// The [`LEVELS`] ladder, left-assoc per level (value-equal to the
    /// official associativity for every operator in the table).
    fn binary(&mut self, level: usize) -> Option<Node> {
        let Some(ops) = LEVELS.get(level) else {
            return self.unary();
        };
        let mut left = Box::new(self.binary(level + 1)?);
        while let Some(&lexeme) = self.lexemes.get(self.pos) {
            if !ops.contains(&lexeme) {
                break;
            }
            let op = binary_op(lexeme);
            self.pos += 1;
            let right = Box::new(self.binary(level + 1)?);
            left = Box::new(Node {
                expr: Expr::Binary { op, left, right },
                span: unanchored(),
            });
        }
        Some(*left)
    }

    fn unary(&mut self) -> Option<Node> {
        let Some(&lexeme) = self.lexemes.get(self.pos) else {
            return None;
        };
        let op = match lexeme {
            "-" => UnOp::Neg,
            "!" => UnOp::Not,
            "~" => UnOp::BitNot,
            _ => return self.primary(),
        };
        self.pos += 1;
        let operand = Box::new(self.nested(Self::unary)?);
        Some(Node {
            expr: Expr::Unary { op, operand },
            span: unanchored(),
        })
    }

    fn primary(&mut self) -> Option<Node> {
        let Some(&lexeme) = self.lexemes.get(self.pos) else {
            return None;
        };
        let expr = match lexeme {
            "(" => {
                self.pos += 1;
                let inner = self.nested(Self::conditional)?;
                if !self.eat(")") {
                    return None;
                }
                return Some(inner);
            }
            "true" => {
                self.pos += 1;
                Expr::Value(ConstantValue::Bool(true))
            }
            "false" => {
                self.pos += 1;
                Expr::Value(ConstantValue::Bool(false))
            }
            "null" => {
                self.pos += 1;
                Expr::Value(ConstantValue::Null)
            }
            lexeme => {
                if let Some(value) = number_value(lexeme) {
                    self.pos += 1;
                    Expr::Value(value)
                } else if is_string_start(lexeme) {
                    let mut parts = Vec::new();
                    while let Some(&spelling) = self.lexemes.get(self.pos) {
                        if !is_string_start(spelling) {
                            break;
                        }
                        let text = string_value(spelling)?;
                        parts.push(Node {
                            expr: Expr::Value(ConstantValue::Str(text)),
                            span: unanchored(),
                        });
                        self.pos += 1;
                    }
                    if parts.len() == 1 {
                        return Some(parts.remove(0));
                    }
                    Expr::Adjacent(parts)
                } else {
                    // An identifier, a member access, a call, `const`-written
                    // collections — every shape the slice alone cannot value.
                    return None;
                }
            }
        };
        Some(Node {
            expr,
            span: unanchored(),
        })
    }
}

fn binary_op(lexeme: &str) -> BinOp {
    match lexeme {
        "??" => BinOp::IfNull,
        "||" => BinOp::Or,
        "&&" => BinOp::And,
        "==" => BinOp::Eq,
        "!=" => BinOp::Ne,
        "<" => BinOp::Lt,
        ">" => BinOp::Gt,
        "<=" => BinOp::Le,
        ">=" => BinOp::Ge,
        "|" => BinOp::BitOr,
        "^" => BinOp::BitXor,
        "&" => BinOp::BitAnd,
        "<<" => BinOp::Shl,
        ">>" => BinOp::Shr,
        ">>>" => BinOp::Ushr,
        "+" => BinOp::Add,
        "-" => BinOp::Sub,
        "*" => BinOp::Mul,
        "/" => BinOp::Div,
        "~/" => BinOp::IntDiv,
        _ => BinOp::Mod,
    }
}

/// The literal's [`ConstantValue`], `None` when the lexeme is not a
/// numeric literal or carries no representable value (a hex literal past
/// 64 bits, a decimal past `i64` — error literals upstream).
#[expect(clippy::cast_possible_wrap)] // a 64-bit hex literal wraps to i64 (the VM's bit pattern)
fn number_value(lexeme: &str) -> Option<ConstantValue> {
    if let Some(hex) = lexeme
        .strip_prefix("0x")
        .or_else(|| lexeme.strip_prefix("0X"))
    {
        let value = u64::from_str_radix(hex, 16).ok()?;
        return Some(ConstantValue::Int(value as i64));
    }
    if lexeme.bytes().all(|b| b.is_ascii_digit()) {
        return Some(ConstantValue::Int(lexeme.parse().ok()?));
    }
    // The remaining literal shapes are doubles (`1.5`, `1.`, `.5`, `1e3` —
    // the scanner's NUMBER grammar); anything else fails the parse.
    Some(ConstantValue::Double(lexeme.parse().ok()?))
}

/// Whether the lexeme opens a string literal (raw or cooked, single- or
/// triple-quoted) — the discriminator `primary`'s string arm keys on.
fn is_string_start(lexeme: &str) -> bool {
    let rest = lexeme.strip_prefix('r').unwrap_or(lexeme);
    rest.starts_with('\'') || rest.starts_with('"')
}

/// The string literal's interior value: quotes stripped, escapes decoded
/// for cooked spellings, `None` for every shape outside the subset
/// (interpolation — an unescaped `$` — an unterminated spelling, an
/// invalid escape). Raw spellings keep their interior verbatim.
fn string_value(lexeme: &str) -> Option<Box<str>> {
    let (raw, rest) = match lexeme.strip_prefix('r') {
        Some(rest) => (true, rest),
        None => (false, lexeme),
    };
    let interior = if let Some(stripped) = rest.strip_prefix("'''") {
        stripped.strip_suffix("'''")?
    } else if let Some(stripped) = rest.strip_prefix("\"\"\"") {
        stripped.strip_suffix("\"\"\"")?
    } else if let Some(stripped) = rest.strip_prefix('\'') {
        if stripped == "'" {
            ""
        } else {
            stripped.strip_suffix('\'')?
        }
    } else {
        let stripped = rest.strip_prefix('"')?;
        if stripped == "\"" {
            ""
        } else {
            stripped.strip_suffix('"')?
        }
    };
    if raw {
        return (!interior.contains('$')).then(|| Box::from(interior));
    }
    decode_cooked(interior).map(Into::into)
}

/// The cooked string's escape decoder (the official escape set:
/// `pkg/front_end`'s single-line/triple string grammar). `None` on an
/// unescaped `$` (interpolation), a trailing backslash, or an invalid
/// `\x`/`\u` spelling — every face where the source itself is in error.
fn decode_cooked(interior: &str) -> Option<String> {
    let mut out = String::with_capacity(interior.len());
    let mut rest = interior;
    while let Some(head) = rest.chars().next() {
        if head != '\\' {
            if head == '$' {
                return None; // interpolation — outside the subset
            }
            out.push(head);
            rest = &rest[head.len_utf8()..];
            continue;
        }
        let mut escapes = rest[1..].chars();
        let Some(escape) = escapes.next() else {
            return None; // a trailing backslash
        };
        rest = escapes.as_str();
        match escape {
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            'f' => out.push('\u{0C}'),
            'b' => out.push('\u{08}'),
            't' => out.push('\t'),
            'v' => out.push('\u{0B}'),
            'x' => {
                let hex = rest.get(..2)?;
                let value = u32::from_str_radix(hex, 16).ok()?;
                rest = &rest[2..];
                out.push(char::from_u32(value)?);
            }
            'u' => {
                let (hex, consumed) = if let Some(braced) = rest.strip_prefix('{') {
                    let end = braced.find('}')?;
                    let hex = braced.get(..end)?;
                    if hex.is_empty() || hex.len() > 6 {
                        return None;
                    }
                    (hex, end + 2)
                } else {
                    (rest.get(..4)?, 4)
                };
                let value = u32::from_str_radix(hex, 16).ok()?;
                rest = &rest[consumed..];
                out.push(char::from_u32(value)?);
            }
            // The identity escapes (`\'`, `\"`, `\\`, `\$`, …); a non-ASCII
            // escape byte stays outside the subset.
            escape if escape.is_ascii() => out.push(escape),
            _ => return None,
        }
    }
    Some(out)
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
    /// A constant pattern's expression (the `ConstantPattern` node — bare
    /// in a `case`/if-case or `const`-prefixed): the root verdict reports
    /// `constant_pattern_with_non_constant_expression`
    /// (`constant_verifier.dart`'s `visitConstantPattern` →
    /// `_evaluateAndReportError`, :128-142).
    ConstantPattern,
    /// A map-pattern entry's key expression (the `key` of a `key: value`
    /// `MapPatternEntry`): the root verdict reports
    /// `non_constant_map_pattern_key` (`visitMapPattern`'s per-entry
    /// `_evaluateAndReportError`, constant_verifier.dart:342-358). Opens per
    /// key, never nested — a key is a leaf constant context.
    MapPatternKey,
    /// An annotation (`@Name(...)` metadata): every argument is a constant
    /// expression by language rule, so the literals inside capture
    /// implicitly-const and their element faces fire at the literal closes
    /// (`@Target({TargetKind.classType})` over an unresolved `TargetKind`
    /// reports `non_constant_set_element` at the element). No
    /// declaration-level verdict — the oracle's annotation evaluation has
    /// no initializer-style site default, so the window's capture discards.
    Annotation,
}

struct Window {
    kind: WindowKind,
    /// Operand-stack length at open — every pop/truncate clamps here.
    base: usize,
    /// Spread/if/for elements seen inside the window. Two consumers read
    /// it: the [`WindowKind::NonConstDup`] dup check skips the whole
    /// literal (the oracle's `whereType<Expression>` excludes those
    /// elements, and their folded operands cannot be told apart here —
    /// under-reporting by design), and the [`WindowKind::LiteralCtor`]
    /// candidate folds a control-flow-bearing literal to `Expr::Other`
    /// (a partial fold could value a literal the oracle's `canBeConst`
    /// probe calls non-constant).
    ctl_elements: u32,
    /// The creation head a `ConstructorReference` identifier latched (a
    /// type head never reaches the operand stack, so a ctor-invocation
    /// window's [`ClosedArgs::callee`] would otherwise close `None`).
    head: Option<StringId>,
}

/// One open argument list: the callee node latched at `begin_arguments`
/// (the stack node just below the base), the operand mark, and the
/// collected arguments.
struct ArgLevel {
    callee: Option<Node>,
    mark: usize,
    named_pending: bool,
    args: Vec<Node>,
    /// One namedness flag per flushed argument (same order as `args`).
    named: Vec<bool>,
    /// `windows.len()` at open — an argument list is its window's own
    /// when no level of the same window remains below it.
    window_seq: usize,
    /// The callee identifier arrived through a dot selector
    /// (`ExpressionContinuation`): only a selector has a receiver, so
    /// only its close may extend the call span back over the stack's
    /// top node (`x.f(`). A bare-expression callee (`f(`, `C(` — and a
    /// call element following a sibling list element, `[
    /// missing, Missing() ]`) has no receiver: the neighboring node is
    /// an unrelated operand, never part of the call.
    selector_callee: bool,
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
    /// Operand-stack length at each open `(` (a parenthesized expression
    /// or a record literal): [`Self::record_literal`]'s field pops clamp
    /// above the record's own open, so a trailing-comma-inflated field
    /// count cannot eat the operand below it (the map key an enclosing
    /// entry already staged).
    record_stack_marks: Vec<usize>,
    /// Per open string literal: the collected `${...}` operands.
    string_interps: Vec<Vec<Node>>,
    /// Operand-stack length at each `begin_literal_string`.
    string_marks: Vec<usize>,
    /// Latched stack lengths of open opaque groups (`await`, `new`,
    /// cascades, function literals): body debris truncates away.
    opaque_marks: Vec<usize>,
    /// The const set/map literals whose parts await the ERROR-tier
    /// duplicate check (#2261): staged at `literal_set_or_map`'s const
    /// path, drained LIFO by the engine at the literal's value push
    /// (the oracle's `visitSetOrMapLiteral` checks every `isConst`
    /// literal, constant_verifier.dart:430-475).
    staged_dups: Vec<(SetOrMap, SourceRange)>,
    /// The element-check leaf spans recorded while an annotation window
    /// was open (#2261): the annotation's argument faces consume them at
    /// the window's own `(` close, pairing the const-argument face at
    /// each leaf (the literal folds carry no argument spans for the
    /// shapes the oracle keys on). The flag marks a set/map element face
    /// that a type-declaration host retracts there (the probed host
    /// split — the element check has no host view, the drain does).
    annotation_leaves: Vec<(SourceRange, bool)>,
    /// The selector flag the next `begin_arguments` latches onto its
    /// level (the just-pushed callee ident was a dot selector).
    selector_callee: bool,
    /// The callee operands an armed literal-ctor window drained at its
    /// open ([`Self::open_literal_ctor`]): the level's callee hands them
    /// back, or the head's idents sit below the window's base as debris
    /// and every later pairing (a map entry's key pop) reads one operand
    /// past its value.
    pending_ctor_callee: Option<Node>,
    /// The explicit `const C(...)` creation spans (`const` keyword
    /// through the closing token): an argument value folded inside one
    /// belongs to the creation, not to the enclosing call — the
    /// resolution-side argument faces read the creation's container
    /// type, never an inner literal's (`isExp(const Constant(null))`
    /// checks `Constant`, not `null`).
    creation_spans: Vec<SourceRange>,
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
#[derive(Clone, Debug)]
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
    #[must_use]
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
            record_stack_marks: Vec::new(),
            opaque_marks: Vec::new(),
            staged_dups: Vec::new(),
            annotation_leaves: Vec::new(),
            selector_callee: false,
            pending_ctor_callee: None,
            creation_spans: Vec::new(),
        }
    }

    #[must_use]
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

    /// Opens an annotation's const-capture window (the `@` token): the
    /// metadata's arguments are a const context by language rule.
    pub fn open_annotation(&mut self) {
        self.open_window(WindowKind::Annotation);
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
                // The armed head's operands leave the parent's stack HERE:
                // the window's base latches at the current length, so a
                // head left staged would sit below the base as debris
                // forever (the level's pops clamp at the new base) and
                // every later pairing — a map entry's key pop, a record's
                // field pop — reads one operand past its value. The drain
                // hands the operands to the level's callee so the
                // candidate keeps its head (`Point(`'s `Point`,
                // `BigInt.parse(`'s merged member).
                let base = self.base();
                let mut drained = None;
                if self.selector_callee {
                    let sel = self.pop_clamped(base);
                    let recv = self.pop_clamped(base);
                    if let (Some(sel), Some(recv)) = (sel, recv)
                        && let Expr::Ident(name) = sel.expr
                    {
                        let span = SourceRange {
                            offset: recv.span.offset,
                            length: sel.span.end().saturating_sub(recv.span.offset),
                        };
                        drained = Some(Node {
                            expr: Expr::Member {
                                recv: Box::new(recv),
                                name,
                            },
                            span,
                        });
                    }
                } else {
                    drained = self.pop_clamped(base);
                }
                self.pending_ctor_callee = drained;
                self.open_window(WindowKind::LiteralCtor);
                true
            }
            Some(_) => false,
        }
    }

    /// Whether a literal-constructor candidate window is innermost.
    #[must_use]
    pub fn top_is_literal_ctor(&self) -> bool {
        self.windows
            .last()
            .is_some_and(|w| w.kind == WindowKind::LiteralCtor)
    }

    /// Closes a literal-constructor candidate window, returning its
    /// captured argument trees (empty without such a window on top). A
    /// candidate nested in an enclosing candidate or dup literal leaves a
    /// non-constant placeholder on the parent stack
    /// (`Self::non_const_dup_placeholder`'s shape widened to both
    /// non-const kinds). The dup face needs the non-constant reading. The
    /// candidate parent is a known under-report against the oracle there:
    /// its `canBeConst` probe re-wraps the whole creation in `const`, the
    /// nested creation re-evaluates as const inside it, and the official
    /// reports the outer creation too when the inner one is const-valid —
    /// where the placeholder keeps dartq's outer face silent.
    pub fn close_literal_ctor(&mut self, span: SourceRange) -> Vec<Node> {
        if !self.top_is_literal_ctor() {
            return Vec::new();
        }
        self.apply_unaries_at(self.depth);
        // The candidate's own argument level still holds the args flushed
        // by `positional_argument` (the candidate window opened at its
        // `begin_arguments`, so the level's `window_seq` is this window's
        // own); the stack above the base is the unflushed tail. Both
        // halves join — earlier args alone never reached the stack drain.
        let mut args = match self.args.last_mut() {
            Some(level) if level.window_seq == self.windows.len() => {
                std::mem::take(&mut level.args)
            }
            _ => Vec::new(),
        };
        let base = self.base();
        args.extend(self.stack.split_off(base));
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

    /// Opens a constant-pattern window (the `ConstantPattern` node's
    /// expression — `case e:` and `if (x case e)` alike, bare or
    /// `const`-prefixed). No nesting rule: the oracle's
    /// `visitConstantPattern` evaluates every constant pattern's
    /// expression, and a constant pattern inside another constant
    /// pattern's *expression* (a `const` collection literal's elements are
    /// sub-patterns, not this node) cannot occur — the window nests by
    /// construction.
    pub fn open_constant_pattern(&mut self) {
        self.open_window(WindowKind::ConstantPattern);
    }

    /// Whether a constant-pattern window is innermost (the engine only
    /// evaluates when it owns the top — a nested constant pattern's close
    /// fires before its parent's).
    #[must_use]
    pub fn top_is_constant_pattern(&self) -> bool {
        self.windows
            .last()
            .is_some_and(|w| w.kind == WindowKind::ConstantPattern)
    }

    /// Closes a constant-pattern window, returning its root node for the
    /// caller to evaluate — mirrors [`Self::close_initializer`] (the engine
    /// supplies the [`ConstLookup`], so the borrows stay split). `None`
    /// without such a window on top.
    pub fn close_constant_pattern(&mut self) -> Option<Node> {
        if !self.top_is_constant_pattern() {
            return None;
        }
        self.apply_unaries();
        let base = self.base();
        let root = self.take_root();
        self.stack.truncate(base);
        self.drop_latches_above(base);
        self.windows.pop();
        // The closed window's node does not join a parent window's
        // operands — a pattern expression is a leaf context, nothing below
        // reads it.
        root
    }

    /// Opens a map-pattern-key window (the `key` of a `key: value`
    /// `MapPatternEntry`). A key is a leaf constant context — the window
    /// nests by construction (no key contains a sub-key).
    pub fn open_map_pattern_key(&mut self) {
        self.open_window(WindowKind::MapPatternKey);
    }

    /// Whether a map-pattern-key window is innermost.
    #[must_use]
    pub fn top_is_map_pattern_key(&self) -> bool {
        self.windows
            .last()
            .is_some_and(|w| w.kind == WindowKind::MapPatternKey)
    }

    /// Closes a map-pattern-key window, returning its root node for the
    /// caller to evaluate. `None` without such a window on top.
    pub fn close_map_pattern_key(&mut self) -> Option<Node> {
        if !self.top_is_map_pattern_key() {
            return None;
        }
        self.apply_unaries();
        let base = self.base();
        let root = self.take_root();
        self.stack.truncate(base);
        self.drop_latches_above(base);
        self.windows.pop();
        root
    }

    /// A spread/if/for element opened inside the innermost window. The
    /// dup check reads the latch as its wholesale-skip flag; the
    /// literal-constructor candidate reads it as a conservative-silence
    /// flag (a control-flow element's operands stay several stack nodes,
    /// so folding the literal would drop the condition operand and could
    /// value a collection the oracle's `canBeConst` probe calls
    /// non-constant — `Other` keeps the under-report direction).
    pub fn begin_ctl_element(&mut self) {
        if let Some(window) = self.windows.last_mut()
            && matches!(
                window.kind,
                WindowKind::NonConstDup | WindowKind::LiteralCtor
            )
        {
            window.ctl_elements += 1;
        }
    }

    fn open_window(&mut self, kind: WindowKind) {
        self.windows.push(Window {
            kind,
            base: self.stack.len(),
            ctl_elements: 0,
            head: None,
        });
    }

    /// Whether the top window is an enum-constant element's capture: its
    /// own argument list runs `_validateConstantArguments` upstream
    /// (constant_verifier.dart — `constWithNonConstantArgument` is the
    /// *default* code there), unlike a const ConstructorInvocation's args
    /// whose failing leaf reports its own `genericError` code.
    pub fn top_is_enum_element(&self) -> bool {
        self.windows
            .last()
            .is_some_and(|w| w.kind == WindowKind::EnumElement)
    }

    /// Latches the creation head on the open ctor-invocation window (the
    /// `const C` head is a type reference — the operand stack never sees
    /// it, and the window's close needs the name for the
    /// `const_with_non_const` target face).
    pub fn note_ctor_head(&mut self, name: StringId) {
        if let Some(window) = self
            .windows
            .last_mut()
            .filter(|w| w.kind == WindowKind::CtorInvocation && w.head.is_none())
        {
            window.head = Some(name);
        }
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

    /// Closes an annotation window: the literal element faces already
    /// reported at their closes; the annotation's own value feeds nothing
    /// (no declaration-level verdict), so the capture discards. No-op
    /// unless the window is on top (parse recovery symmetry).
    pub fn close_annotation(&mut self) {
        if self
            .windows
            .last()
            .is_some_and(|w| w.kind == WindowKind::Annotation)
        {
            self.discard_window();
        }
    }

    /// Closes a non-const duplicate window: the literal's parts return
    /// (bare operands are set elements, staged `Entry` nodes are map
    /// entries, `{}` is a map) with a flag telling whether spread/if/for
    /// elements occurred — the dup check skips those literals wholesale.
    /// `None` unless a [`WindowKind::NonConstDup`] window is on top (the
    /// const windows close through [`Self::literal_set_or_map`]). A
    /// literal nested in an enclosing dup literal leaves an opaque
    /// placeholder on the parent stack (`Self::non_const_dup_placeholder`).
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
    /// `NonConstDup` window is on top.
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
        self.selector_callee = false;
        self.push(
            Node {
                expr: Expr::Ident(name),
                span,
            },
            false,
        );
    }

    /// A dot-selector identifier (`x.f(`'s `f`): the same operand push as
    /// [`Self::identifier`], plus the selector flag the call's
    /// `begin_arguments` latches — a selector's close may extend the call
    /// span over the stack's receiver node, a bare callee's may not.
    pub fn selector_identifier(&mut self, name: StringId, span: SourceRange) {
        if !self.open() {
            return;
        }
        self.selector_callee = true;
        self.push(
            Node {
                expr: Expr::Ident(name),
                span,
            },
            false,
        );
    }

    /// `#foo` — a valid constant, by its name (the upstream
    /// `SymbolState` equality: same name equal, different names not).
    pub fn symbol(&mut self, name: StringId, span: SourceRange) {
        if self.open() {
            self.push(
                Node {
                    expr: Expr::Value(ConstantValue::Symbol(name)),
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

    /// A postfix `++`/`--` (its operand is already pushed). The captured
    /// node spans operand-through-operator (`o++`), matching the oracle's
    /// non-constant-pattern anchor.
    pub fn unary_postfix(&mut self, span: SourceRange) {
        if self.open() {
            let base = self.base();
            let start = self
                .pop_clamped(base)
                .map_or(span.offset, |operand| operand.span.offset);
            self.push(
                Node {
                    expr: Expr::Other,
                    span: SourceRange {
                        offset: start,
                        length: span.end().saturating_sub(start),
                    },
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
        let (Some(els), Some(then), Some(cond)) = (
            self.pop_clamped(mark),
            self.pop_clamped(mark),
            self.pop_clamped(mark),
        ) else {
            return;
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
        let name_end = name.span.end();
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
        // The access ends at its selector: `end` is the parser's following
        // token, so the selector node's own span is the oracle anchor.
        let span = SourceRange {
            offset: recv.span.offset,
            length: name_end.saturating_sub(recv.span.offset),
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
    /// operands drain and nothing checks. Under a literal-constructor
    /// candidate window the literal is one argument of a non-const
    /// creation — there is no const context, so no element check runs
    /// (the oracle element-checks only `const` literals); the operands
    /// fold into a single [`Expr::List`] node so the candidate's
    /// `canBeConst` probe evaluates the literal's own const-ness.
    pub fn literal_list(&mut self, count: usize, span: SourceRange) -> Option<Vec<Node>> {
        if !self.open() {
            return None;
        }
        if self.top_is_non_const_dup() {
            self.apply_unaries_at(self.depth);
            self.pop_operands(count);
            self.non_const_dup_placeholder(span);
            return None;
        }
        self.apply_unaries_at(self.depth);
        if self.fold_literal_ctor_element(count, span, Expr::List, self.base()) {
            return None;
        }
        Some(self.pop_operands(count))
    }

    /// The literal-constructor candidate fold shared by the collection
    /// and record literals: only under a [`WindowKind::LiteralCtor`] top —
    /// the literal is a non-const creation's argument, not a const-checked
    /// face, so `build` folds its operands into one expression node on the
    /// stack (a control-flow element folds `Expr::Other` instead: its
    /// operand count the `count` arity cannot see, and a partial fold
    /// could value a literal the oracle's `canBeConst` probe rejects).
    /// Returns `true` when the fold ran; `false` leaves the operands for
    /// the caller's per-element path.
    fn fold_literal_ctor_element(
        &mut self,
        count: usize,
        span: SourceRange,
        build: impl FnOnce(Vec<Node>) -> Expr,
        floor: usize,
    ) -> bool {
        if !self.top_is_literal_ctor() {
            return false;
        }
        let has_ctl = self.windows.last().is_some_and(|w| w.ctl_elements > 0);
        let elements = self.pop_operands_at(count, floor);
        let expr = if has_ctl { Expr::Other } else { build(elements) };
        self.push(Node { expr, span }, true);
        true
    }

    /// A set-or-map literal (explicit or implicit const): staged `Entry`
    /// pseudo-operands are map entries, bare operands set elements; the
    /// parts return for per-element checking. `{}` is a map (the official
    /// `isMap` default). Non-const duplicate windows never reach here —
    /// [`Self::close_non_const_dup`] owns them (the engine's literal
    /// handler tries it first). Under a literal-constructor candidate the
    /// literal is a non-const creation's argument: no element check runs,
    /// and the operands fold into one [`Expr::Set`]/[`Expr::Map`] node for
    /// the candidate's `canBeConst` probe (mirroring
    /// [`Self::literal_list`]).
    pub fn literal_set_or_map(
        &mut self,
        count: usize,
        span: SourceRange,
        has_set_entry: bool,
    ) -> Option<SetOrMap> {
        if !self.open() || self.top_is_non_const_dup() {
            return None;
        }
        self.apply_unaries_at(self.depth);
        if self.fold_literal_ctor_element(count, span, |staged| {
            match split_set_or_map(staged, has_set_entry) {
                SetOrMap::Set(elements) => Expr::Set(elements),
                SetOrMap::Map(entries) => Expr::Map(entries),
            }
        }, self.base()) {
            return None;
        }
        let staged = self.pop_operands(count);
        let shape = split_set_or_map(staged, has_set_entry);
        // The const path stages the parts for the ERROR-tier duplicate
        // check (#2261): the engine drains them at this literal's value
        // push (`push_literal_value`), so the rows land beside the
        // element faces. Non-const literals never reach here (the
        // `close_non_const_dup` branch owns them upstream).
        self.staged_dups.push((shape.clone(), span));
        Some(shape)
    }

    /// Records an element-check leaf span while an annotation window is
    /// open (the annotation-argument faces' pairing input, #2261); the
    /// flag marks a retractable set/map element face.
    pub fn note_annotation_leaf(&mut self, span: SourceRange, retractable: bool) {
        if self.annotation_open() {
            self.annotation_leaves.push((span, retractable));
        }
    }

    /// Whether any annotation window is open.
    pub fn annotation_open(&self) -> bool {
        self.windows.iter().any(|w| w.kind == WindowKind::Annotation)
    }

    /// Takes every recorded leaf span, keeping only those inside
    /// `[start, end)`; a nested annotation's own drain (its `(` closes
    /// first) strips its leaves, so the outer window's drain sees only
    /// its own — the LIFO close order makes the removal-all safe.
    pub fn take_annotation_leaves(&mut self, start: u32, end: u32) -> Vec<(SourceRange, bool)> {
        let (kept, taken) = self
            .annotation_leaves
            .drain(..)
            .partition(|(span, _)| span.offset < start || span.end() > end);
        self.annotation_leaves = kept;
        taken
    }

    /// Records one explicit `const C(...)` creation's written span (the
    /// `const` keyword through the creation's closing token). File-bounded
    /// like the operand stack: one entry per written creation, consumed
    /// only through the read-only [`Self::creation_covers`].
    pub fn note_creation_span(&mut self, span: SourceRange) {
        self.creation_spans.push(span);
    }

    /// Whether `offset` sits inside a recorded const creation: an
    /// argument row folding there belongs to the creation's own
    /// evaluation — the resolution-side faces read the creation's
    /// container type, never the inner literal's.
    pub fn creation_covers(&self, offset: u32) -> bool {
        self.creation_spans
            .iter()
            .any(|span| span.offset <= offset && offset < span.end())
    }

    /// Pops the staged const-literal parts whose shape matches `want`
    /// (the engine calls it at the literal's value push — LIFO pairs the
    /// staging with its own literal; a mismatched top stays for recovery
    /// worlds and only ever pairs a later same-kind push).
    pub fn take_staged_dup(&mut self, want: InstanceKind) -> Option<(SetOrMap, SourceRange)> {
        let matches_kind = self.staged_dups.last().is_some_and(|(shape, _)| {
            matches!(
                (want, shape),
                (InstanceKind::Map, SetOrMap::Map(_)) | (InstanceKind::Set, SetOrMap::Set(_))
            )
        });
        if matches_kind {
            self.staged_dups.pop()
        } else {
            None
        }
    }

    /// Takes the annotation window's own argument list: `Some((args,
    /// named))` when the top window is an Annotation and its own
    /// argument level is on top (the annotation head's `(`). The
    /// engine's argument-list close calls it BEFORE the generic close so
    /// the annotation-argument faces run in source order; the generic
    /// close then finds no level and stays silent. The paren depth stays
    /// here untouched — the generic close's unconditional decrement
    /// balances the open. An INNER argument list's close (`@T(g())`)
    /// shares the window and must leave both stacks be — the same
    /// no-older-level-of-this-window test the generic close applies to
    /// the creation kinds (end_arguments).
    pub fn take_annotation_args(&mut self) -> Option<(Vec<Node>, Vec<bool>)> {
        if !self
            .windows
            .last()
            .is_some_and(|w| w.kind == WindowKind::Annotation)
        {
            return None;
        }
        let level_seq = self.args.last()?.window_seq;
        if !self.args[..self.args.len() - 1]
            .iter()
            .all(|l| l.window_seq < level_seq)
        {
            return None;
        }
        self.apply_unaries_at(self.depth);
        // Complete a trailing argument the listener has not flushed yet;
        // a no-argument close flushes nothing (a bare flush would push a
        // namedness flag without an argument behind it).
        if self
            .args
            .last()
            .is_some_and(|level| self.stack.len() > level.mark)
        {
            self.flush_argument();
        }
        let level = self.args.pop()?;
        Some((level.args, level.named))
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
    /// operands drain and nothing checks. Under a literal-constructor
    /// candidate the record is a non-const creation's argument: no field
    /// check runs and the operands fold into one [`Expr::Record`] node
    /// (mirroring [`Self::literal_list`]).
    pub fn record_literal(&mut self, count: usize, span: SourceRange) -> Option<Vec<Node>> {
        if !self.open() {
            return None;
        }
        // The record's own `(` is the pop floor: a trailing comma
        // inflates the parser's `count` past the staged fields, and an
        // unclamped pop eats the operand below (the map key an enclosing
        // entry staged — the shifted pairing fed duplicate-check keys
        // that were never keys).
        let floor = self
            .base()
            .max(self.record_stack_marks.last().copied().unwrap_or(0));
        if self.top_is_non_const_dup() {
            self.apply_unaries_at(self.depth);
            self.pop_operands_at(count, floor);
            self.record_stack_marks.pop();
            self.non_const_dup_placeholder(span);
            return None;
        }
        self.apply_unaries_at(self.depth);
        if self.fold_literal_ctor_element(count, span, Expr::Record, floor) {
            self.record_stack_marks.pop();
            return None;
        }
        let fields = self.pop_operands_at(count, floor);
        self.record_stack_marks.pop();
        Some(fields)
    }

    fn pop_operands(&mut self, count: usize) -> Vec<Node> {
        self.pop_operands_at(count, self.base())
    }

    /// [`Self::pop_operands`] with an explicit floor: a record literal's
    /// fields pop above `max(base, mark)` — the record's own open.
    fn pop_operands_at(&mut self, count: usize, floor: usize) -> Vec<Node> {
        let mut nodes = Vec::new();
        for _ in 0..count {
            if let Some(node) = self.pop_clamped(floor) {
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
        // arguments. A `new C(...)` list pushes no callee operand (the
        // constructor reference is a type, not an expression) and latches
        // its opaque mark at the `new` keyword — the pop must not eat the
        // enclosing literal's staged node in that shape (#2187: the eaten
        // map key shifted every later entry pairing, feeding the duplicate
        // check keys that were never keys).
        let callee_pushed = self
            .opaque_marks
            .last()
            .is_none_or(|&mark| self.stack.len() > mark);
        let selector = std::mem::take(&mut self.selector_callee);
        if callee_pushed && self.stack.len() > base {
            self.stack.pop();
        }
        let callee = callee.filter(|_| callee_pushed);
        // An armed candidate's head was drained at its window's open —
        // the level's callee is that drained node (nothing remains on the
        // stack to pop). Every other callee keeps the bare shape #2194
        // pins: the receiver stays below for the close's span extension,
        // the dot merge following it.
        let callee = self.pending_ctor_callee.take().or(callee);
        self.args.push(ArgLevel {
            callee,
            mark: self.stack.len(),
            named_pending: false,
            args: Vec::new(),
            named: Vec::new(),
            window_seq: self.windows.len(),
            selector_callee: selector,
        });
    }

    /// A named-argument label. The listener's event lands after the
    /// value's operands are staged (`NamedArgumentExpression` event
    /// order), so the flag sets first and the value flushes here — the
    /// same one-node flush a positional argument gets. Without the flush,
    /// consecutive named values pool on the stack and the next flush
    /// coalesces them into one compound-recovery `Other` node under a
    /// single namedness flag — the creation-argument faces then read a
    /// non-constant where the oracle folds a constant (`P(a: 1, b: 2)`),
    /// and the annotation faces' flag count drifts from the argument
    /// count.
    pub fn named_argument(&mut self) {
        if let Some(level) = self.args.last_mut() {
            level.named_pending = true;
        }
        if self.open() {
            self.apply_unaries_at(self.depth);
            self.flush_argument();
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
        // Nothing staged and no flag pending: a bare flush — an empty
        // argument list's close, or the close after a named event already
        // flushed its own value — must not grow `named` past `args` (the
        // annotation faces pair the two vectors).
        if level.mark.min(self.stack.len()) == self.stack.len() {
            return;
        }
        // One namedness flag per argument, in flush order — the
        // annotation-argument faces read it (the official's
        // `genericError` distinguishes the named value's leaf by the
        // parameter resolution, value.dart:2550-2561).
        level.named.push(level.named_pending);
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
            // The creation head rides the window (`note_ctor_head`) when a
            // type head named it; an expression callee reaches here through
            // the operand stack instead.
            let head = self
                .windows
                .last()
                .and_then(|w| w.head)
                .or_else(|| callee.and_then(head_name));
            return Some(ClosedArgs {
                callee: head,
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
                // A bare call inside a non-const face window is an ordinary
                // runtime call — provably not a constant for that window's
                // own question (the `canBeConst` probe of `LiteralCtor`, the
                // duplicate check of `NonConstDup`). Neither window is a
                // const context, so the const-context interpretations below
                // (`const_eval_method_invocation` on `f(x)`, the implicit
                // `const C(x)` creation) never apply; the call folds to a
                // non-constant leaf and the caller sees no `ClosedArgs`.
                if self.top_is_literal_ctor()
                    || self
                        .windows
                        .last()
                        .is_some_and(|w| w.kind == WindowKind::NonConstDup)
                {
                    self.push(
                        Node {
                            expr: Expr::Other,
                            span: node.span,
                        },
                        true,
                    );
                    return None;
                }
                // The oracle's invocation node spans the receiver: `x.f()`,
                // `a().f()`, `DateTime.now().toString()` all report the
                // whole `recv.name(args)` span at `constEvalMethodInvocation`.
                // `dot_access` merges the receiver operand AFTER this close,
                // so the callee arrives here bare — the node left on the
                // stack when the receiver immediately precedes the selector
                // (the `.`/`?.`/`()` gap is at most three bytes: `x.f` reads
                // one, `a().f` reads one past the `)` leaf, `x?.f` reads
                // two) extends the span back to it. A binary/conditional
                // operand at the same position (`x + f()`, `c ? f() : y`)
                // is latched, not a receiver — the latches discriminate
                // where the byte-gap cannot. Only a selector callee may
                // take the extension: a bare `f(` has no receiver, so a
                // small gap to the stack's top node is a sibling operand
                // (`[missing, Missing()]`'s preceding element, `C.n(`'s
                // receiver above an inner argument) whose span must not
                // widen the call.
                let offset = if level.selector_callee {
                    self.stack
                        .last()
                        .filter(|prev| {
                            let mark = self.stack.len() - 1;
                            let gap = node.span.offset.saturating_sub(prev.span.end());
                            (1..=3).contains(&gap)
                                && !self.bin_latches.iter().any(|&(_, m, _)| m == mark)
                                && !self.cond_latches.iter().any(|&(m, _)| m == mark)
                        })
                        .map_or(node.span.offset, |prev| prev.span.offset)
                } else {
                    node.span.offset
                };
                let span = SourceRange {
                    offset,
                    length: end.end().saturating_sub(offset),
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
            // runtime member call is simply not constant-evaluatable. A
            // constant-pattern window answers its own question — `this()`
            // / `super()` / `x.y()` in a case operand is provably not a
            // constant expression, so the leaf marks the whole invocation
            // (callee start through the closing paren) non-constant.
            let non_constant = self.top_is_literal_ctor() || self.top_is_constant_pattern();
            self.push(
                Node {
                    expr: if non_constant {
                        Expr::Other
                    } else {
                        Expr::Opaque
                    },
                    span: if non_constant {
                        SourceRange {
                            offset: node.span.offset,
                            length: end.end().saturating_sub(node.span.offset),
                        }
                    } else {
                        node.span
                    },
                },
                true,
            );
        }
        None
    }

    /// Closes a constructor-initializer's super/redirect-constructor
    /// argument list (`super(...)` / `this(...)` / `super.named(...)`): the
    /// invocation folds to a non-constant leaf for the enclosing
    /// initializer window exactly like the generic close's non-ident
    /// callee path, but the argument nodes return to the engine for the
    /// `_reportNotPotentialConstantsArguments` scan — the oracle's
    /// `invalid_constant` face lives on the arglist, not on the folded
    /// value. Returns `None` when no window is open (the args were never
    /// captured).
    pub fn end_init_invocation_arguments(&mut self, end: SourceRange) -> Option<ClosedArgs> {
        if !self.open() {
            self.args.clear();
            return None;
        }
        if !self.opaque_marks.is_empty() {
            self.depth -= 1;
            self.args.pop();
            return None;
        }
        self.apply_unaries_at(self.depth);
        self.flush_argument();
        self.depth -= 1;
        let level = self.args.pop()?;
        // The whole invocation is a runtime construct — provably not a
        // constant for the enclosing window (a redirect or super call can
        // never fold into a const value). It joins the operand stack as the
        // initializer window's own node so the root verdict still stands.
        let span = level
            .callee
            .map_or(end, |callee| SourceRange {
                offset: callee.span.offset,
                length: end.end().saturating_sub(callee.span.offset),
            });
        self.push(
            Node {
                expr: Expr::Other,
                span,
            },
            true,
        );
        Some(ClosedArgs {
            callee: None,
            span,
            creation_owned: false,
            ctor_invocation: false,
            args: level.args,
        })
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

    /// Pushes a provably non-constant leaf anchored at `span` (a const
    /// literal whose element failed).
    pub fn push_non_constant(&mut self, span: SourceRange) {
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

    /// Whether the capture sits inside an open argument list (of any
    /// window).
    #[must_use]
    pub fn in_arguments(&self) -> bool {
        !self.args.is_empty()
    }

    /// Pushes a known constant leaf (an opaque collection/creation value
    /// the engine built at a construct's close).
    ///
    /// A qualified creation (`C.named(...)`) closes with its receiver head
    /// still on the operand stack and the engine reporting the SELECTOR as
    /// the callee — the value replaces the head (the
    /// [`Self::push_named_creation`] mechanics) and the instance names the
    /// CONTAINER: the head is the type the creation folds to, the selector
    /// is not.
    pub fn push_value(&mut self, span: SourceRange, value: ConstantValue) {
        if !self.open() {
            return;
        }
        let mut span = span;
        let value = if let ConstantValue::Created(callee) = value {
            let base = self.base();
            let head = self.stack.last();
            if self.stack.len() > base
                && let Some(Node {
                    expr: Expr::Ident(head_name),
                    span: head_span,
                }) = head
                // The receiver head is the call span's first token: the
                // close extended the span back over it only when the head
                // is a plain (unlatched) operand, so equality separates it
                // from an unrelated binary/conditional operand below.
                && head_span.offset == span.offset
            {
                span = SourceRange {
                    offset: head_span.offset,
                    length: span.end().saturating_sub(head_span.offset),
                };
                let value = ConstantValue::Created(*head_name);
                self.stack.pop();
                value
            } else {
                ConstantValue::Created(callee)
            }
        } else {
            value
        };
        self.push(Node { expr: Expr::Value(value), span }, true);
    }

    /// The implicit named creation `C.named(...)`: the capture keeps the
    /// `C` head as its own operand (the selector is the call's callee), so
    /// the value replaces that head and spans from it through the call.
    pub fn push_named_creation(&mut self, container: StringId, span: SourceRange) {
        if !self.open() {
            return;
        }
        let mut span = span;
        let base = self.base();
        if self.stack.len() > base
            && let Some(head) = self.stack.last()
            && matches!(head.expr, Expr::Ident(n) if n == container)
            && head.span.offset <= span.offset
        {
            span = SourceRange {
                offset: head.span.offset,
                length: span.end() - head.span.offset,
            };
            self.stack.pop();
        }
        self.push_value(span, ConstantValue::Created(container));
    }

    /// Takes the capture stack's top node (the const constructor's
    /// field-initializer RHS, stashed before the assignment folds it —
    /// the #1351 creation-time walk reads it from the engine's own
    /// stash). `None` when the recovery left nothing.
    pub fn take_top_node(&mut self) -> Option<Node> {
        self.stack.pop()
    }

    // —— depth-bearing brackets ——————————————————————————————————————

    pub fn parenthesized_begin(&mut self) {
        if self.open() {
            self.depth += 1;
            self.record_stack_marks.push(self.stack.len());
        }
    }

    pub fn parenthesized_end(&mut self) {
        if self.open() {
            self.apply_unaries_at(self.depth - 1);
            self.depth -= 1;
            self.record_stack_marks.pop();
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

        fn node(expr: Expr, offset: u32) -> Node {
            Node {
                expr,
                span: SourceRange { offset, length: 1 },
            }
        }

        fn value_node(v: ConstantValue) -> Node {
            Self::node(Expr::Value(v), 0)
        }

        fn eval(&self, node: &Node) -> Verdict {
            eval(node, self)
        }

        fn bin(&self, op: BinOp, l: ConstantValue, r: ConstantValue) -> Verdict {
            let left = Self::value_node(l);
            let right = Self::value_node(r);
            let node = Self::node(
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

        /// Mirrors `members.primitive_ids`: `bool`/`int`/`double`/`num`/
        /// `String` map to their `InstanceState` families.
        fn created_state(&self, container: StringId) -> CreatedState {
            if container == self.id("bool") {
                CreatedState::Bool
            } else if container == self.id("int") {
                CreatedState::Int
            } else if container == self.id("double") || container == self.id("num") {
                CreatedState::Num
            } else if container == self.id("String") {
                CreatedState::String
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

    // —— arithmetic (the `IntState`/`DoubleState` matrix) ————————————

    #[test]
    fn int_arithmetic_wraps_and_div_always_doubles() {
        use BinOp::*;
        let world = w();
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
        let neg = World::node(
            Expr::Unary {
                op: UnOp::Neg,
                operand: Box::new(World::value_node(ConstantValue::Int(i64::MIN))),
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
        use BinOp::*;
        let world = w();
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
        use BinOp::*;
        let world = w();
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
        use BinOp::*;
        let world = w();
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
        use BinOp::*;
        let world = w();
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
        use BinOp::*;
        let world = w();
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
        use BinOp::*;
        let world = w();
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
        let not = World::node(
            Expr::Unary {
                op: UnOp::BitNot,
                operand: Box::new(World::value_node(ConstantValue::Double(1.0))),
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
        let false_and_nc = World::node(
            Expr::Binary {
                op: BinOp::And,
                left: Box::new(World::value_node(ConstantValue::Bool(false))),
                right: Box::new(World::node(Expr::Ident(world.id("nc")), 9)),
            },
            7,
        );
        assert_eq!(
            world.eval(&false_and_nc),
            Verdict::Value(ConstantValue::Bool(false))
        );
        // `true || <non-const>`: same.
        let true_or_nc = World::node(
            Expr::Binary {
                op: BinOp::Or,
                left: Box::new(World::value_node(ConstantValue::Bool(true))),
                right: Box::new(World::node(Expr::Ident(world.id("nc")), 9)),
            },
            7,
        );
        assert_eq!(
            world.eval(&true_or_nc),
            Verdict::Value(ConstantValue::Bool(true))
        );
        // A non-bool left operand faults with const_eval_type_bool at the
        // operand's span.
        let int_and = World::node(
            Expr::Binary {
                op: BinOp::And,
                left: Box::new(World::value_node(ConstantValue::Int(1))),
                right: Box::new(World::value_node(ConstantValue::Bool(true))),
            },
            7,
        );
        assert_eq!(
            world.eval(&int_and),
            Verdict::Fault(ConstFault::TypeBool, span(0))
        );
        // `??`: a non-null left short-circuits; a null left takes the
        // right.
        let ifnull = World::node(
            Expr::Binary {
                op: BinOp::IfNull,
                left: Box::new(World::value_node(ConstantValue::Null)),
                right: Box::new(World::value_node(ConstantValue::Int(2))),
            },
            7,
        );
        assert_eq!(world.eval(&ifnull), Verdict::Value(ConstantValue::Int(2)));
    }

    #[test]
    fn conditional_takes_one_branch_and_checks_bool() {
        let world = w();
        // A known condition skips the dead branch's non-constancy.
        let cond = World::node(
            Expr::Cond {
                cond: Box::new(World::value_node(ConstantValue::Bool(true))),
                then: Box::new(World::value_node(ConstantValue::Int(1))),
                els: Box::new(World::node(Expr::Ident(world.id("nc")), 9)),
            },
            7,
        );
        assert_eq!(world.eval(&cond), Verdict::Value(ConstantValue::Int(1)));
        // A non-bool condition faults at the condition's span
        // (applyBooleanConversion → const_eval_type_bool).
        let bad = World::node(
            Expr::Cond {
                cond: Box::new(World::value_node(ConstantValue::Int(1))),
                then: Box::new(World::value_node(ConstantValue::Int(1))),
                els: Box::new(World::value_node(ConstantValue::Int(2))),
            },
            7,
        );
        assert_eq!(
            world.eval(&bad),
            Verdict::Fault(ConstFault::TypeBool, span(0))
        );
    }

    /// The `InstanceState` family map for `Created` and the `NullState`
    /// throw arms: `!`/`-`/`~` on `null` throw `constEvalThrowsException`
    /// (the base operator admits null through its assert, then throws —
    /// `NullState.logicalNot` throws directly); a created primitive of
    /// unknown value answers a valid constant where its state admits the
    /// operand, and the non-state containers fault their gates
    /// (`value.dart`'s `InstanceState` methods).
    #[test]
    fn unary_operators_read_created_and_null_states() {
        let world = w();
        let unary = |op: UnOp, v: ConstantValue| {
            world.eval(&World::node(
                Expr::Unary {
                    op,
                    operand: Box::new(World::value_node(v)),
                },
                3,
            ))
        };
        // `!null`/`-null`/`~null` all throw (not the operand-type faults).
        for op in [UnOp::Not, UnOp::Neg, UnOp::BitNot] {
            assert_eq!(
                unary(op, ConstantValue::Null),
                Verdict::Fault(ConstFault::ThrowsException, span(3))
            );
        }
        // `bool.fromEnvironment`'s `Created(bool)`: `!`, `&&`, `?:`, and
        // the interpolation gate all admit `BoolState` — the drift
        // `const _enable = !_releaseMode` shape.
        let created_bool = ConstantValue::Created(world.id("bool"));
        assert_eq!(unary(UnOp::Not, created_bool.clone()), Verdict::Constant);
        let and = World::node(
            Expr::Binary {
                op: BinOp::And,
                left: Box::new(World::value_node(created_bool.clone())),
                right: Box::new(World::value_node(ConstantValue::Bool(true))),
            },
            7,
        );
        assert_eq!(world.eval(&and), Verdict::Constant);
        // A non-bool right still faults its gate at the right's span.
        let and_bad = World::node(
            Expr::Binary {
                op: BinOp::And,
                left: Box::new(World::value_node(created_bool.clone())),
                right: Box::new(World::value_node(ConstantValue::Int(1))),
            },
            7,
        );
        assert_eq!(
            world.eval(&and_bad),
            Verdict::Fault(ConstFault::TypeBool, span(0))
        );
        let cond = World::node(
            Expr::Cond {
                cond: Box::new(World::value_node(created_bool.clone())),
                then: Box::new(World::value_node(ConstantValue::Int(1))),
                els: Box::new(World::value_node(ConstantValue::Int(2))),
            },
            7,
        );
        assert_eq!(world.eval(&cond), Verdict::Constant);
        // `int.fromEnvironment`'s `Created(int)`: `-`/`~` admit `IntState`.
        let created_int = ConstantValue::Created(world.id("int"));
        assert_eq!(unary(UnOp::Neg, created_int.clone()), Verdict::Constant);
        assert_eq!(unary(UnOp::BitNot, created_int), Verdict::Constant);
        // A `Created` num answers `-` but not `~` (`assertIntOrNull`
        // faults a `DoubleState`/`NumState`).
        let created_num = ConstantValue::Created(world.id("num"));
        assert_eq!(unary(UnOp::Neg, created_num.clone()), Verdict::Constant);
        assert_eq!(
            unary(UnOp::BitNot, created_num),
            Verdict::Fault(ConstFault::TypeInt, span(3))
        );
        // Non-state containers fault every gate.
        let created_c = ConstantValue::Created(world.id("C"));
        assert_eq!(
            unary(UnOp::Not, created_c.clone()),
            Verdict::Fault(ConstFault::TypeBool, span(3))
        );
        let created_s = ConstantValue::Created(world.id("String"));
        assert_eq!(
            unary(UnOp::Neg, created_s),
            Verdict::Fault(ConstFault::TypeNum, span(3))
        );
    }

    // —— strings, interpolation, calls ———————————————————————————————

    #[test]
    fn interpolation_requires_bool_num_string_or_null() {
        let world = w();
        // `'a${[1]}'`: a list operand faults with
        // const_eval_type_bool_num_string at the interpolation.
        let interp = World::node(
            Expr::Interp {
                parts: vec![
                    World::value_node(ConstantValue::Str("a".into())),
                    World::value_node(ConstantValue::Instance(InstanceKind::List)),
                ],
            },
            7,
        );
        assert_eq!(
            world.eval(&interp),
            Verdict::Fault(ConstFault::TypeBoolNumString, span(0))
        );
        // Adjacent strings concatenate.
        let adjacent = World::node(
            Expr::Adjacent(vec![
                World::value_node(ConstantValue::Str("a".into())),
                World::value_node(ConstantValue::Str("b".into())),
            ]),
            7,
        );
        assert_eq!(
            world.eval(&adjacent),
            Verdict::Value(ConstantValue::Str("ab".into()))
        );
        // A clean interpolation defers its value (escapes are undecoded —
        // a fabricated text would be wrong where values are observable).
        let clean = World::node(
            Expr::Interp {
                parts: vec![World::value_node(ConstantValue::Str("a".into()))],
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
        use BinOp::*;
        let world = w();
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
        assert_eq!(world.eval(&World::node(Expr::Opaque, 3)), Verdict::Constant);
        let add = World::node(
            Expr::Binary {
                op: BinOp::Add,
                left: Box::new(World::node(Expr::Opaque, 0)),
                right: Box::new(World::value_node(ConstantValue::Int(1))),
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
        let call = World::node(
            Expr::Call {
                name: world.id("identical"),
                args: vec![
                    World::node(Expr::Ident(world.id("e1")), 0),
                    World::node(Expr::Ident(world.id("e2")), 0),
                ],
            },
            7,
        );
        assert_eq!(
            world.eval(&call),
            Verdict::Value(ConstantValue::Bool(false))
        );
        // Any other invocation is provably non-constant.
        let other = World::node(
            Expr::Call {
                name: world.id("other"),
                args: vec![World::value_node(ConstantValue::Int(1))],
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
        let add_unknown = World::node(
            Expr::Binary {
                op: BinOp::Add,
                left: Box::new(World::node(Expr::Ident(world.id("foreign")), 0)),
                right: Box::new(World::value_node(ConstantValue::Int(1))),
            },
            7,
        );
        assert_eq!(world.eval(&add_unknown), Verdict::Constant);
        // An enum constant's member reads defer (synthesized faces).
        let member = World::node(
            Expr::Member {
                recv: Box::new(World::node(Expr::Ident(world.id("e1")), 0)),
                name: world.id("index"),
            },
            7,
        );
        assert_eq!(world.eval(&member), Verdict::Constant);
        // Enum constants are values with identity.
        let same = World::node(
            Expr::Binary {
                op: BinOp::Eq,
                left: Box::new(World::node(Expr::Ident(world.id("e1")), 0)),
                right: Box::new(World::node(Expr::Ident(world.id("e1")), 0)),
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
        let add = World::node(
            Expr::Binary {
                op: BinOp::Add,
                left: Box::new(World::value_node(ConstantValue::Int(1))),
                right: Box::new(World::node(Expr::Ident(world.id("nc")), 9)),
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
        let mut node = World::value_node(ConstantValue::Int(1));
        for i in 0..1000 {
            node = World::node(
                Expr::Unary {
                    op: UnOp::Neg,
                    operand: Box::new(node),
                },
                i,
            );
        }
        assert_eq!(world.eval(&node), Verdict::Constant);
        let mut node = World::value_node(ConstantValue::Int(1));
        for i in 0..99 {
            node = World::node(
                Expr::Unary {
                    op: UnOp::Neg,
                    operand: Box::new(node),
                },
                i,
            );
        }
        assert_eq!(world.eval(&node), Verdict::Value(ConstantValue::Int(-1)));
    }

    /// The batch-H degrade posture: the kernel wildcard arms return "not a
    /// constant" instead of panicking. Three fallbacks are constructible
    /// cheaply today — a non-num operand through `numeric`'s double kernel
    /// (the `as_f64` degrade; `binary_values` gates on `is_num`, but the
    /// free function accepts any operands), the eager arm's lazy-operator
    /// wildcard through a direct `binary_values` call, and the int kernel's
    /// op wildcard via `Eq` (one arm shared with every other non-arithmetic
    /// op). Only a future `BinOp` or `ConstantValue` variant needs a
    /// variant that does not exist yet, and stays desk-pinned to the same
    /// posture.
    #[test]
    fn kernel_wildcards_degrade_to_not_constant() {
        let span = span(3);
        // A non-num operand takes the double kernel's `as_f64` degrade.
        assert_eq!(
            numeric(
                BinOp::Add,
                span,
                &ConstantValue::Str("s".into()),
                &ConstantValue::Str("t".into()),
            ),
            Verdict::NotConstant(span)
        );
        // The eager arm's lazy-operator wildcard degrades the same way (the
        // short-circuit evaluation happens in `eval_binary`).
        assert_eq!(
            binary_values(
                BinOp::And,
                span,
                &ConstantValue::Bool(true),
                &ConstantValue::Bool(false),
            ),
            Verdict::NotConstant(span)
        );
        // The int kernel's op wildcard: `Eq` routes to `binary_values`'
        // equality arm in production, but a direct kernel call constructs
        // the degrade.
        assert_eq!(
            numeric(
                BinOp::Eq,
                span,
                &ConstantValue::Int(1),
                &ConstantValue::Int(2),
            ),
            Verdict::NotConstant(span)
        );
    }

    // —— the literal-constructor candidate window (#2114) ————————————
    //
    // `WindowKind::LiteralCtor` brackets a NON-const creation's argument
    // list for the `non_const_call_to_literal_constructor` `canBeConst`
    // probe. A collection/record literal inside it is one argument of a
    // runtime creation — the oracle element-checks only `const` literals —
    // so the literal APIs return `None` (no element rows) and fold the
    // operands into a single expression node the probe evaluates.

    /// `A([i])` — a const constructor's plain send with a list argument:
    /// the candidate window opens, the list folds into one `Expr::List`
    /// node, and evaluating the folded argument rejects `i` — the
    /// `non_constant_list_element` rows the corpus over-emitted never
    /// existed, because no const context was open.
    #[test]
    fn literal_ctor_window_folds_list_argument() {
        let world = w();
        let mut capture = ConstCapture::new(world.id("identical"));
        assert!(capture.open_literal_ctor());
        capture.begin_arguments();
        capture.literal(span(1), ConstantValue::Int(5));
        capture.positional_argument();
        // The nested list literal: one element, a provably non-const read
        // (the rig's `nc` — an in-scope local/formal face).
        capture.identifier(world.id("nc"), span(2));
        assert!(
            capture.literal_list(1, span(2)).is_none(),
            "a non-const creation's list argument is no element-checked face"
        );
        capture.positional_argument();
        // The engine closes the candidate window before the argument-list
        // close drains the stack (`close_literal_ctor_args` precedes
        // `const_capture.end_arguments`), so the stack still holds the
        // operand nodes here.
        let args = capture.close_literal_ctor(span(3));
        capture.end_arguments(span(3));
        // Two argument nodes: `5`, then the folded list. The list node
        // evaluates its elements — `nc` is provably non-constant, so the
        // whole argument is non-constant (the candidate's silence face).
        assert_eq!(args.len(), 2);
        match &args[1].expr {
            Expr::List(elements) => {
                assert_eq!(elements.len(), 1);
            }
            other => panic!("expected Expr::List, got {other:?}"),
        }
        assert_eq!(
            eval(&args[1], &world),
            Verdict::NotConstant(span(2)),
            "the folded list rejects its non-const element"
        );
    }

    /// `A({x: x})` — the map literal inside the candidate folds into one
    /// `Expr::Map`; no `non_constant_map_element` rows exist because the
    /// literal is not in a const context.
    #[test]
    fn literal_ctor_window_folds_map_argument() {
        let world = w();
        let mut capture = ConstCapture::new(world.id("identical"));
        assert!(capture.open_literal_ctor());
        capture.begin_arguments();
        // `{nc: nc}` — key then value stage as one Entry pseudo-operand
        // (the rig's `nc` is the provably non-const face).
        capture.identifier(world.id("nc"), span(1));
        capture.identifier(world.id("nc"), span(2));
        capture.literal_map_entry(span(2));
        assert!(
            capture.literal_set_or_map(1, span(3), false).is_none(),
            "a non-const creation's map argument is no element-checked face"
        );
        capture.positional_argument();
        let args = capture.close_literal_ctor(span(4));
        capture.end_arguments(span(4));
        assert_eq!(args.len(), 1);
        match &args[0].expr {
            Expr::Map(entries) => {
                assert_eq!(entries.len(), 1);
            }
            other => panic!("expected Expr::Map, got {other:?}"),
        }
        assert_eq!(
            eval(&args[0], &world),
            Verdict::NotConstant(span(1)),
            "the folded map rejects its non-const key"
        );
    }

    /// `A({1, 2})` — a set literal folds the same way (`has_set_entry`
    /// splits `Expr::Entry` staging from bare elements); the folded node
    /// keeps the collection's own value for `canBeConst`.
    #[test]
    fn literal_ctor_window_folds_set_argument() {
        let world = w();
        let mut capture = ConstCapture::new(world.id("identical"));
        assert!(capture.open_literal_ctor());
        capture.begin_arguments();
        capture.literal(span(1), ConstantValue::Int(1));
        capture.literal(span(2), ConstantValue::Int(2));
        assert!(
            capture.literal_set_or_map(2, span(3), true).is_none(),
            "a non-const creation's set argument is no element-checked face"
        );
        capture.positional_argument();
        let args = capture.close_literal_ctor(span(4));
        capture.end_arguments(span(4));
        assert_eq!(args.len(), 1);
        assert_eq!(
            eval(&args[0], &world),
            Verdict::Value(ConstantValue::Set([ConstantValue::Int(1), ConstantValue::Int(2)].into())),
            "a constant set stays const-feasible for the probe, by its element values"
        );
    }

    /// `A([for (...) e])` — a control-flow element's operand count is not
    /// the literal's element arity: folding conservatively to `Expr::Other`
    /// keeps the probe's non-constant verdict instead of valuing a partial
    /// fold (the oracle's `canBeConst` rejects the shape anyway once a
    /// single operand is non-constant).
    #[test]
    fn literal_ctor_window_ctl_element_folds_other() {
        let world = w();
        let mut capture = ConstCapture::new(world.id("identical"));
        assert!(capture.open_literal_ctor());
        capture.begin_arguments();
        capture.begin_ctl_element();
        capture.identifier(world.id("x"), span(1));
        capture.literal(span(2), ConstantValue::Int(1));
        assert!(
            capture.literal_list(1, span(3)).is_none(),
            "a control-flow element folds conservatively, no element face"
        );
        capture.positional_argument();
        let args = capture.close_literal_ctor(span(4));
        capture.end_arguments(span(4));
        // The control-flow element's stray operand stays on the stack as a
        // sibling node (the conservative-drift posture); the folded last
        // node is `Expr::Other`.
        assert!(matches!(args.last().map(|n| &n.expr), Some(Expr::Other)));
    }

    /// The const-context faces are unchanged: inside an explicit `const`
    /// literal window the element list still returns for per-element
    /// checking (the `non_constant_list_element` face itself).
    #[test]
    fn const_literal_window_still_returns_elements() {
        let world = w();
        let mut capture = ConstCapture::new(world.id("identical"));
        capture.open_explicit_literal();
        capture.identifier(world.id("nc"), span(1));
        let elements = capture.literal_list(1, span(2));
        assert_eq!(
            elements.map(|e| e.len()),
            Some(1),
            "a const list's elements still return for the element check"
        );
    }

    fn span(offset: u32) -> SourceRange {
        SourceRange { offset, length: 1 }
    }

    // —— eval_written: the lexeme-slice face ——————————————————————————

    /// The slice's value, panicking with the verdict on any deferral (the
    /// positive pins' helper).
    fn written_value(lexemes: &[&str]) -> ConstantValue {
        match eval_written(lexemes) {
            WrittenValue::Value(v) => v,
            other => panic!("expected a value for {lexemes:?}, got {other:?}"),
        }
    }

    /// The comparison faces' values: the documented textual under-report
    /// (`2` vs `1 + 1`) closes, precedence and the numeric kernel match the
    /// captured fold (same [`eval`]), and the boundary literals keep the
    /// VM's shapes.
    #[test]
    fn eval_written_routes_the_comparison_values() {
        assert_eq!(written_value(&["1", "+", "1"]), written_value(&["2"]));
        assert_ne!(written_value(&["1", "+", "2"]), written_value(&["2"]));
        assert_eq!(
            written_value(&["1", "+", "2", "*", "3"]),
            ConstantValue::Int(7)
        );
        assert_eq!(
            written_value(&["(", "1", "+", "2", ")", "*", "3"]),
            ConstantValue::Int(9)
        );
        // `/` widens, `~/` stays int, unary minus and the hex shape.
        assert_eq!(written_value(&["5", "/", "2"]), ConstantValue::Double(2.5));
        assert_eq!(written_value(&["5", "~/", "2"]), ConstantValue::Int(2));
        assert_eq!(written_value(&["-", "1"]), ConstantValue::Int(-1));
        assert_eq!(written_value(&["0xFF"]), ConstantValue::Int(255));
        // A hex literal at the sign boundary keeps the 64-bit pattern.
        assert_eq!(
            written_value(&["0x8000000000000000"]),
            ConstantValue::Int(i64::MIN)
        );
        // `2` and `2.0` stay distinct (the `DartObjectImpl` type-equal
        // posture the comparison face inherits).
        assert_ne!(written_value(&["2"]), written_value(&["2.0"]));
        assert_eq!(written_value(&["true"]), ConstantValue::Bool(true));
        assert_eq!(written_value(&["null"]), ConstantValue::Null);
        assert_eq!(
            written_value(&["1", "==", "1", "?", "'y'", ":", "'n'"]),
            ConstantValue::Str("y".into())
        );
    }

    /// The string faces: adjacent runs and `+` concatenate alike, cooked
    /// escapes decode, raw interiors stay verbatim, and an escaped `\$` is
    /// not interpolation.
    #[test]
    fn eval_written_decodes_strings() {
        assert_eq!(
            written_value(&["'a'", "'b'"]),
            ConstantValue::Str("ab".into())
        );
        assert_eq!(
            written_value(&["'a'", "+", "'b'"]),
            ConstantValue::Str("ab".into())
        );
        assert_eq!(
            written_value(&["'\\x41\\u0042\\u{43}'"]),
            ConstantValue::Str("ABC".into())
        );
        assert_eq!(
            written_value(&["'a\\nb'"]),
            ConstantValue::Str("a\nb".into())
        );
        assert_eq!(
            written_value(&["r'a\\nb'"]),
            ConstantValue::Str("a\\nb".into())
        );
        assert_eq!(written_value(&["''"]), ConstantValue::Str("".into()));
        assert_eq!(
            written_value(&["'a\\$b'"]),
            ConstantValue::Str("a$b".into())
        );
    }

    /// The deferral faces: a fault has no known value (the comparison
    /// face's silence), every resolution-needing or uncovered shape is not
    /// evaluable, and the nesting/token caps keep pathological slices on
    /// the deferral path.
    #[test]
    fn eval_written_defers_and_bails() {
        assert_eq!(eval_written(&["1", "~/", "0"]), WrittenValue::Unknown);
        assert_eq!(eval_written(&["1", "+", "'a'"]), WrittenValue::Unknown);
        for lexemes in [
            &["foo"][..],
            &["a", ".", "b"][..],
            &["const", "[", "1", "]"][..],
            &["'a$v'"][..],
            &["'unterminated"][..],
            &["2", "x"][..],
            &["("][..],
            &["'''"][..],
            &[][..],
        ] {
            assert_eq!(
                eval_written(lexemes),
                WrittenValue::NotEvaluable,
                "{lexemes:?}"
            );
        }
        let deep = [vec!["("; 200], vec!["1"], vec![")"; 200]].concat();
        assert_eq!(eval_written(&deep), WrittenValue::NotEvaluable);
        let long = vec!["1"; 2000];
        assert_eq!(eval_written(&long), WrittenValue::NotEvaluable);
    }
}

/// Source-level pins for the non-const duplicate face's capture window
/// (#2187): each fixture replays one intl census shape through the live
/// pipeline and asserts the exact `equal_keys_in_map` /
/// `equal_elements_in_set` row set. The oracle emits these codes only for
/// genuinely duplicated plain-literal keys — the capture must feed the
/// checker each literal's own top-level entries and nothing else (named
/// arguments, nested literals, and cross-literal debris are not keys).
#[cfg(test)]
mod equal_keys_2187 {
    use crate::collector::collect;
    use crate::diagnose::{StrictModes, fold_unit};
    use crate::library::ElementIndex;
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_parser::ParserFeatures;
    use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder, Namespaces};
    use dartq_scanner::{ScanConfig, scan};
    use dartq_source::{FileTable, SourceRange, StringTable};
    use std::path::Path;

    /// The `(code, start, end)` triples for the two duplicate-face codes,
    /// in report order. Parse diagnostics fail the pin first — every
    /// fixture is syntactically clean.
    fn duplicate_spans(source: &str) -> Vec<(&'static str, u32, u32)> {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file = files.intern(Path::new("main.dart"));
        let scanned = scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(scanned.diagnostics.is_empty(), "scanner diagnostics: {source:?}");
        let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
        let features = ParserFeatures::from_feature_set(set);
        let elements = collect(source, &scanned.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib = builder.intern("package:t/main.dart");
        let mut index = ElementIndex::default();
        index.insert(file, elements.clone());
        for (name, decl) in elements.declarations() {
            if decl.kind == crate::model::MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let fold = fold_unit(
            source,
            &scanned.tokens,
            features,
            &strings,
            file,
            lib,
            &graph,
            &namespaces,
            &index,
            StrictModes::default(),
            &LibraryClassModifiers::default(),
            set.is_enabled(dartq_language::Feature::InferenceUpdate2),
        );
        assert!(
            fold.parse_diagnostics.is_empty(),
            "parse diagnostics: {:?}",
            fold.parse_diagnostics
        );
        fold.report
            .diagnostics
            .iter()
            .filter(|d| {
                matches!(
                    d.code.name,
                    "equal_keys_in_map" | "equal_elements_in_set"
                )
            })
            .map(|d| (d.code.name, d.range.offset, d.range.end()))
            .collect()
    }

    /// The `(code, text-at-span)` rows for the two duplicate-face codes,
    /// in report order. Row offsets are UTF-16 code units; the source
    /// slice converts to bytes first (ASCII sources slice identically).
    fn duplicate_rows(source: &str) -> Vec<(&'static str, String)> {
        let u16_to_byte = |u16_off: u32| -> usize {
            let mut units = 0u32;
            for (byte_idx, ch) in source.char_indices() {
                if units >= u16_off {
                    return byte_idx;
                }
                units += ch.len_utf16() as u32;
            }
            source.len()
        };
        duplicate_spans(source)
            .into_iter()
            .map(|(code, start, end)| {
                let s = u16_to_byte(start);
                let e = u16_to_byte(end);
                (code, source[s..e].to_owned())
            })
            .collect()
    }

    /// F1 (the census #6 repro, minimized): named-argument values of `new`
    /// creations are not map keys — a repeated NAME across blocks stays
    /// silent. The date_symbol_data_local shape (const list arguments).
    #[test]
    fn f1_named_arg_values_are_not_map_keys() {
        let rows = duplicate_rows(
            r#"
class Foo {
  Foo({String? name, bool? flag});
}
final m = <String, Object>{
  "a": new Foo(NAME: "a", FLAG: true),
  "b": new Foo(NAME: "b", FLAG: true),
};
"#,
        );
        assert_eq!(rows, [], "named-argument values must never dup-check");
    }

    /// F1 variant (the DateSymbols shape): `const [...]` nested argument
    /// literals under the same non-const map.
    #[test]
    fn f1_named_arg_with_const_list_argument_silent() {
        let rows = duplicate_rows(
            r#"
class Bar {
  Bar({String? name, List<String>? tags});
}
final m = <String, Object>{
  "a": new Bar(NAME: "a", TAGS: const ["x"]),
  "b": new Bar(NAME: "b", TAGS: const ["y"]),
};
"#,
        );
        assert_eq!(rows, [], "const list arguments must not shift the pairing");
    }

    /// F2 (the date_time_patterns shape): nested const-map values under a
    /// non-const parent map — the parent's own unique keys stay silent.
    #[test]
    fn f2_nested_const_map_values_silent() {
        let rows = duplicate_rows(
            r#"final m = <String, Map<String, int>>{
  "a": const {"k": 1},
  "b": const {"k": 2},
  "c": const {"k": 3},
};"#,
        );
        assert_eq!(rows, [], "nested const maps must not surface as keys");
    }

    /// F3 — the over-correction gate: plain-literal true positives must
    /// keep firing at the later duplicate's own span, and the const twin
    /// stays silent (upstream `_checkForDuplications` skips const
    /// literals — the compile-error face owns those).
    #[test]
    fn f3_plain_literal_duplicates_still_fire() {
        let map_src = r#"var m = {"a": 1, "b": 2, "a": 3};"#;
        assert_eq!(
            duplicate_rows(map_src),
            [("equal_keys_in_map", String::from(r#""a""#))],
            "the plain map's later duplicate key must report at its own span"
        );

        let set_rows = duplicate_rows(r#"var s = {1, 2, 1};"#);
        assert_eq!(
            set_rows,
            [("equal_elements_in_set", String::from("1"))],
            "the plain set's later duplicate element must report"
        );

        let const_rows = duplicate_rows(r#"const m2 = {"a": 1, "a": 2};"#);
        assert_eq!(
            const_rows,
            [],
            "const literals are the compile-error face, not this check"
        );
    }

    /// F4 (the legit oracle case): repeated constructor-argument values
    /// under distinct keys are not duplicates, while genuinely equal keys
    /// still report at the later key.
    #[test]
    fn f4_ctor_call_values_are_not_keys() {
        let rows = duplicate_rows(
            r#"
class Foo {
  Foo({String? name});
}
final m = <String, Foo>{
  "a": Foo(NAME: "x"),
  "b": Foo(NAME: "x"),
};
"#,
        );
        assert_eq!(rows, [], "repeated values are not repeated keys");
        assert_eq!(
            duplicate_rows(r#"final m2 = {"x": 1, "y": 2, "x": 3};"#),
            [("equal_keys_in_map", String::from(r#""x""#))],
            "genuinely equal keys still report"
        );
    }

    /// F5 (the constants.dart shape): a flat non-const map initializer
    /// after a preceding top-level invocation initializer — unique keys
    /// must stay silent across the top-level boundary.
    #[test]
    fn f5_flat_final_initializer_silent() {
        let rows = duplicate_rows(
            r#"
final int z = '0'.codeUnitAt(0);
final Map<String, String> cur = {"A": "A", "B": "B", "C": "C"};
"#,
        );
        assert_eq!(rows, [], "unique keys of a later initializer stay silent");
    }

    /// The census repro at corpus scale: 60 `NAME == outer key` blocks in
    /// one outer literal — the per-block pairing must stay exact.
    #[test]
    fn f1_scale_named_arg_blocks_stay_silent() {
        let mut src = String::from(
            "class Foo {\n  Foo({String? name, bool? flag});\n}\nfinal m = <String, Object>{\n",
        );
        for i in 0..60 {
            src.push_str(&format!(
                "  \"k{i}\": new Foo(NAME: \"k{i}\", FLAG: true),\n"
            ));
        }
        src.push_str("};\n");
        let rows = duplicate_rows(&src);
        assert_eq!(rows, [], "60-block scale: {rows:?}");
    }

    /// The strongest pins: the four real intl corpus files whose 234
    /// spurious rows founded this ticket (#2187 census cluster #6). Every
    /// one must stay silent — the oracle emits `equal_keys_in_map` /
    /// `equal_elements_in_set` nowhere in these files.
    #[test]
    fn real_intl_corpus_files_stay_silent() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/corpus/real/intl/lib");
        let files = [
            "date_symbol_data_local.dart",
            "date_time_patterns.dart",
            "number_symbols_data.dart",
            "src/intl/constants.dart",
        ];
        for rel in files {
            let source =
                std::fs::read_to_string(format!("{root}/{rel}")).expect("corpus file");
            let rows = duplicate_spans(&source);
            assert_eq!(rows.len(), 0, "{rel} fired {}: first {rows:?}", rows.len());
        }
    }

    /// A multi-byte literal before a map must not corrupt the key
    /// payloads: the string-leaf interior cuts in bytes through the
    /// engine's UTF-16→byte conversion, so `£` (2 UTF-8 bytes, 1 UTF-16
    /// unit) in a neighbour no longer shifts every later literal's value
    /// (the shift made junk payloads collide — the #2187 constants face).
    #[test]
    fn multibyte_neighbour_keeps_key_payloads_true() {
        let rows = duplicate_rows(
            "final a = '£';\nfinal m = {\n  \"AB\": 1,\n\"AB\": 2,\n};\n",
        );
        assert_eq!(
            rows,
            [("equal_keys_in_map", String::from("\"AB\""))],
            "the true interiors collide, regardless of the £ byte drift"
        );
    }

    /// One-unit test span at `offset`.
    fn span(offset: u32) -> SourceRange {
        SourceRange { offset, length: 1 }
    }

    /// Capture-level pin for the `new C(...)` argument list: the list
    /// pushes no callee operand, so `begin_arguments` must leave the
    /// enclosing literal's staged key alone — the entry pairs the entry's
    /// own key with the creation's opaque fold (the eaten key shifted
    /// every later pairing and fed the duplicate check keys that were
    /// never keys).
    #[test]
    fn new_creation_arguments_leave_the_enclosing_key_staged() {
        use super::{ConstCapture, ConstantValue, Expr, SetOrMap};
        use dartq_source::StringTable;

        let strings = StringTable::new();
        let mut capture = ConstCapture::new(strings.intern("identical"));
        capture.open_non_const_dup();
        // Entry 1: key leaf, then the `new Foo(...)` value — the opaque
        // group latches at `new`, the argument list pushes nothing for the
        // constructor reference, and the group folds to one opaque leaf.
        capture.literal(span(1), ConstantValue::Str("a".into()));
        capture.begin_opaque_group();
        capture.begin_arguments();
        capture.literal(span(2), ConstantValue::Str("x".into()));
        capture.named_argument();
        capture.end_arguments(span(3));
        capture.end_opaque_group(span(3));
        capture.literal_map_entry(span(3));
        // Entry 2: the same shape — the second key must stage against its
        // own value, not against entry 1's leftover.
        capture.literal(span(4), ConstantValue::Str("b".into()));
        capture.begin_opaque_group();
        capture.begin_arguments();
        capture.literal(span(5), ConstantValue::Str("y".into()));
        capture.named_argument();
        capture.end_arguments(span(6));
        capture.end_opaque_group(span(6));
        capture.literal_map_entry(span(6));
        let (shape, _) = capture
            .close_non_const_dup(2, span(7), false)
            .expect("the literal's own window closes with its entries");
        let SetOrMap::Map(entries) = shape else {
            panic!("expected a map shape");
        };
        assert_eq!(entries.len(), 2);
        assert!(
            matches!(&entries[0].0.expr, Expr::Value(ConstantValue::Str(s)) if s.as_ref() == "a"),
            "entry 1 keeps its own key, got {:?}",
            entries[0].0.expr
        );
        assert!(
            matches!(&entries[1].0.expr, Expr::Value(ConstantValue::Str(s)) if s.as_ref() == "b"),
            "entry 2 keeps its own key (the argument list must not eat it), got {:?}",
            entries[1].0.expr
        );
    }
}

/// #2194 F2 — `DateTime.now().toString()` in a const initializer: the oracle
/// (dart 3.13.1) reports `const_eval_method_invocation` at the whole
/// `recv.name(args)` invocation and `const_with_non_const` at the
/// `DateTime.now()` creation. The engine-level pair rides the
/// `ClosedArgs::span` the capture hands up — these capture-level pins lock
/// the span extension's receiver-vs-operand discrimination (`dot_access`
/// merges `recv.name` only AFTER the argument list closes, so the callee
/// arrives here bare with the receiver below it).
#[cfg(test)]
mod method_invocation_2194 {
    use super::ConstCapture;
    use dartq_source::{SourceRange, StringTable};

    fn span(offset: u32, end: u32) -> SourceRange {
        SourceRange {
            offset,
            length: end - offset,
        }
    }

    /// `x.m(` — the selector's send closes with the receiver operand below
    /// it: `ClosedArgs::span` extends back through the dot to `x` (the
    /// oracle's method-invocation node is the whole `x.m()`).
    #[test]
    fn selector_call_span_extends_to_the_receiver() {
        let strings = StringTable::new();
        let mut capture = ConstCapture::new(strings.intern("identical"));
        capture.open_initializer();
        // `x` receiver operand, `.m` selector callee, then `()`.
        capture.identifier(strings.intern("x"), span(0, 1));
        capture.selector_identifier(strings.intern("m"), span(2, 3));
        capture.begin_arguments();
        let closed = capture
            .end_arguments(span(4, 5))
            .expect("a bare identifier callee closes into ClosedArgs");
        assert_eq!(closed.callee, Some(strings.intern("m")));
        assert_eq!(
            (closed.span.offset, closed.span.end()),
            (0, 5),
            "the invocation node covers `x.m()` — receiver through `)`"
        );
    }

    /// `x + m(` — a binary operand is latched, not a receiver: the span
    /// stays at `m()` (the plus's left operand must not extend it).
    #[test]
    fn binary_left_operand_does_not_extend_the_call_span() {
        let strings = StringTable::new();
        let mut capture = ConstCapture::new(strings.intern("identical"));
        capture.open_initializer();
        capture.identifier(strings.intern("x"), span(0, 1));
        capture.begin_binary(super::BinOp::Add, span(1, 2));
        capture.identifier(strings.intern("m"), span(2, 3));
        capture.begin_arguments();
        let closed = capture
            .end_arguments(span(4, 5))
            .expect("a bare identifier callee closes into ClosedArgs");
        assert_eq!(closed.callee, Some(strings.intern("m")));
        assert_eq!(
            (closed.span.offset, closed.span.end()),
            (2, 5),
            "the `+` left operand is not a receiver — the span stays at `m()`"
        );
    }

    /// `f(` with nothing below the callee — the bare-name span is the
    /// call's own (`const_eval_method_invocation` on `void f() {}` reports
    /// `f()`, not a receiver-extended span).
    #[test]
    fn bare_call_span_is_the_call() {
        let strings = StringTable::new();
        let mut capture = ConstCapture::new(strings.intern("identical"));
        capture.open_initializer();
        capture.identifier(strings.intern("f"), span(0, 1));
        capture.begin_arguments();
        let closed = capture
            .end_arguments(span(2, 3))
            .expect("a bare identifier callee closes into ClosedArgs");
        assert_eq!(closed.callee, Some(strings.intern("f")));
        assert_eq!((closed.span.offset, closed.span.end()), (0, 3));
    }
}
