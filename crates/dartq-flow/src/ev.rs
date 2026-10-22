//! The recorded event subset: what the recorder stores per executable from
//! the parser's listener stream.
//!
//! Only flow-relevant events are recorded (the full listener contract is
//! three times wider); each variant carries exactly the payload the replay
//! interpreter needs. `Ev::span` gives every event a partition anchor so
//! the interpreter can split a C-style `for` header into its
//! initializers/condition/updaters regions by token offset.

use dartq_parser::{IdentifierContext, TokenSpan};
use dartq_source::SourceRange;

/// Block kinds the interpreter distinguishes (subset of the parser's
/// `BlockKind`): plain statement blocks scope locals and can be dead code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Statement,
    FunctionBody,
    CatchClause,
    Other,
}

/// The recorded events.
#[derive(Clone, Copy, Debug)]
pub enum Ev {
    // —— executable framing ——
    ExecBegin {
        is_closure: bool,
    },
    /// `is_expr`: the executable ended as an expression (function
    /// expression) and nets one value for its consumer; a local function
    /// declaration ends as a statement and nets none. `end`: the
    /// executable's end token (the completion fact's anchor).
    ExecEnd {
        is_expr: bool,
        end: TokenSpan,
    },
    BlockBodyBegin,
    BlockBodyEnd,
    /// `=>` body: consumes the body's value; the function's own value for
    /// the consumer is netted at `ExecEnd { is_expr: true }`.
    ExprBody,
    EmptyBody,
    AsyncModifier {
        is_async: bool,
        is_generator: bool,
    },

    // —— declarations ——
    Formal {
        /// Interned name id (record time).
        name: u32,
        this_or_super: bool,
        /// The parameter's name token (the write-site face's declaration
        /// anchor).
        token: TokenSpan,
    },
    VarDeclBegin {
        late: bool,
        is_final: bool,
        span: TokenSpan,
    },
    VarDeclEnd,
    InitializedIdent {
        name: u32,
        /// The declared identifier's own token (the write-site face's
        /// declaration anchor).
        token: TokenSpan,
    },
    NoInitializer,
    InitializerBegin,
    InitializerEnd,
    PatternVarDeclStmt {
        span: TokenSpan,
    },
    Block {
        kind: BlockKind,
        span: TokenSpan,
    },
    /// The block's end offset: a statement block's end trims a dead-code
    /// interval to the official last-statement end (the covering
    /// construct's own body block never extends — its parent frame says
    /// which).
    EndBlock {
        end: u32,
    },

    // —— types ——
    Type {
        /// The name-level base: a named type's interned id, or one of the
        /// keyword bases (`dynamic`/`void`/`Never`) resolved at record
        /// time.
        base: crate::ty::Base,
        nullable: bool,
        /// The type's written argument list, interned in the recorder's
        /// arena ([`crate::ty::EMPTY_ARGS`] when argument-less — the fast
        /// path). The promotion gate compares these trees for real instead
        /// of suppressing every arg-bearing test outright (#611 step 4).
        args: u32,
    },
    NoType,
    /// A function type's end (`int Function(E, E)`), carrying only the
    /// written nullability — the name-level lattice spells no signature.
    /// The head's nullability is the promotion-relevant bit: a nullable
    /// function-typed declaration promotes through `??=` and null tests
    /// exactly like a named one, and every signature question stays the
    /// semantic layer's.
    FunctionType {
        /// The trailing `?` was present.
        nullable: bool,
    },
    VoidType,

    // —— statements ——
    /// A statement's completion point: the end of its last token, the
    /// trailing `;` included where present. An open dead-code interval
    /// runs to this end (the official close trims the covering list to
    /// its last statement and reports `node.end - offset` —
    /// dead_code_verifier.dart:253-279, :326-332).
    StmtEnd {
        end: u32,
    },
    ExprStmt {
        span: TokenSpan,
    },
    IfBegin {
        span: TokenSpan,
    },
    /// `handle_parenthesized_condition` with a `case` keyword: the
    /// parenthesized if-condition ended in the pattern form
    /// (`if (e case p when g)`). The parser passes `Some(case)` only for
    /// the if-case shape, so this event is the exact marker that the
    /// standalone pattern context opened inside the condition is the
    /// clause's context; `ThenBegin` adopts it only after this event
    /// turned its kind into `IfCase`.
    IfCaseCondition,
    ThenBegin,
    ElseBegin,
    IfEnd {
        has_else: bool,
    },
    WhileBegin {
        span: TokenSpan,
    },
    WhileBodyBegin,
    WhileBodyEnd,
    WhileEnd,
    DoBegin {
        span: TokenSpan,
    },
    DoBodyBegin,
    DoBodyEnd,
    /// The `while (c);` part's range (officially the do-while has no
    /// `flowEnd` of its own: a dead back edge anchors the interval on the
    /// condition, and the part rides the event so the covering close can
    /// emit the official two-range split).
    DoEnd {
        while_range: dartq_source::SourceRange,
    },
    ForBegin {
        span: TokenSpan,
    },
    /// `handle_for_initializer_*`: the C-style header's init/cond boundary —
    /// the init's end, recorded when the initializer finishes. The index
    /// boundary the header partition needs: an expression's events precede
    /// its `ExprStmt` wrapper (the wrapper fires at the statement's end),
    /// so a literal condition's value event sits BEFORE the wrapper the
    /// offset scan keys on and is otherwise lost to the init region.
    /// Deliberately unspanned: the partition takes it as a pure index
    /// boundary, the offset scan stays wrapper-driven.
    ForInit,
    /// `handle_for_loop_parts`: the C-style header separators; fires after
    /// the updaters have been recorded (source order).
    ForParts {
        left_sep: TokenSpan,
        right_sep: TokenSpan,
    },
    ForBodyBegin,
    ForBodyEnd,
    ForEnd,
    ForInParts,
    ForInExprBegin,
    ForInExprEnd,
    ForInBodyBegin,
    ForInBodyEnd,
    ForInEnd,
    ReturnBegin {
        span: TokenSpan,
    },
    ReturnEnd {
        has_expression: bool,
    },
    YieldBegin {
        span: TokenSpan,
    },
    YieldEnd {
        /// The `yield` statement's own range (keyword through `;`) — the
        /// `DemoteViaSuspension.node` anchor the suspension demotion
        /// records.
        span: SourceRange,
    },
    /// `token` is the `await` keyword: the async-gap face anchors the gap
    /// segment it opens on it (the row's `gap_at`).
    AwaitBegin {
        token: TokenSpan,
    },
    AwaitEnd {
        /// The `await` expression's own range (keyword through operand end)
        /// — the `DemoteViaSuspension.node` anchor the suspension demotion
        /// records.
        span: SourceRange,
    },
    Break {
        label: Option<u32>,
        span: TokenSpan,
    },
    Continue {
        label: Option<u32>,
        span: TokenSpan,
    },
    LabeledBegin {
        /// Interned id of the statement's first label (`None` only when a
        /// recovered label never recorded a name — the frame exists but
        /// matches no jump).
        label: Option<u32>,
        span: TokenSpan,
    },
    /// An additional label of the innermost labeled statement (`a: b: s`):
    /// appends to the open labeled frame's match set.
    LabelDecl {
        id: u32,
    },
    LabeledEnd,
    TryBegin {
        span: TokenSpan,
    },
    CatchClauseBegin,
    CatchClauseEnd,
    FinallyBlock,
    TryEnd {
        has_finally: bool,
    },
    SwitchBegin {
        span: TokenSpan,
    },
    SwitchBlockBegin,
    SwitchBlockEnd,
    CaseBegin {
        label_count: usize,
        expression_count: usize,
        /// A default-headed group's `default` keyword (`None` for case
        /// heads): the head-time anchor of the official member check
        /// (`handleSwitchBeforeAlternative`'s `checkUnreachableNode`),
        /// which reports dead members at `node.keyword`
        /// (dead_code_verifier.dart:232-235).
        kw: Option<TokenSpan>,
    },
    /// A case clause head: `span` is the `case` keyword — the member's
    /// first dead node anchors its row there (`dead_code_verifier.dart:232`).
    CaseExprBegin {
        span: TokenSpan,
    },
    CaseNoWhen,
    CaseExprEnd {
        has_guard: bool,
        /// The head's `case` keyword: the head-time member report
        /// (`handleCaseHead`'s `flowEnd`) fires a member-anchored row here.
        kw: TokenSpan,
    },
    CaseEnd {
        statement_count: usize,
        /// The group's `default` keyword, when its heads include one: the
        /// next group's opening consumes it (the dead first-default face,
        /// probe table G rule 2).
        default_kw: Option<TokenSpan>,
    },
    SwitchEnd,
    SwitchExprBegin {
        span: TokenSpan,
    },
    SwitchExprBlockBegin,
    SwitchExprBlockEnd,
    SwitchExprCaseBegin,
    SwitchExprCasePattern,
    SwitchExprCaseEnd {
        has_guard: bool,
        /// The alternative's first token: the matched-type row's clause
        /// anchor (the parser delivers no `case` keyword face on the
        /// switch-expression path).
        begin: TokenSpan,
    },
    AssertBegin {
        span: TokenSpan,
    },
    /// The boundary between an assertion's condition and its optional
    /// message (official `assert_afterCondition`,
    /// `flow_analysis.dart:6478`): the message evaluates on the
    /// condition-false state. Fires only when a message expression follows.
    AssertAfterCondition,
    AssertEnd,

    // —— expressions ——
    Ident {
        name: u32,
        token: TokenSpan,
        context: IdentifierContext,
    },
    Bool {
        value: bool,
    },
    Null,
    OtherExpr {
        span: TokenSpan,
    },
    /// A `this` expression: distinguished from [`Ev::OtherExpr`] so the
    /// async-gap face can read `this.mounted` as the State-side guard and
    /// `this.context` as the State-side context value (upstream resolves
    /// both through the `State` member); every other consumer treats it
    /// exactly like [`Ev::OtherExpr`].
    This {
        span: TokenSpan,
    },
    UnaryPrefix {
        is_not: bool,
    },
    NonNullAssert,
    /// Prefix `++`/`--` (`handleUnaryPrefixAssignmentExpression`): the
    /// operator's span follows the operand's events (the parser reports
    /// after the operand parse) and begins the dead-updater row; the row's
    /// end folds from the operand's own spanned events (the part has no
    /// `ExprStmt`/`StmtEnd` wrapper: the header parses bare
    /// `parse_expression` lists).
    UnaryPrefixAssign {
        span: TokenSpan,
    },
    /// Postfix `++`/`--` (`handleUnaryPostfixAssignmentExpression`): the
    /// operator's end is the updater expression's end — the anchor the
    /// dead-updater row needs (the part has no `ExprStmt`/`StmtEnd`
    /// wrapper: the header parses bare `parse_expression` lists).
    UnaryPostfixAssign {
        end: u32,
    },
    BinaryBegin {
        op: BinOp,
        /// The operator token: a dead right operand's row re-anchors at it
        /// (official `parent is LogicalAnd/LogicalOr/IfNull`,
        /// dead_code_verifier.dart:311-322).
        span: TokenSpan,
    },
    BinaryEnd {
        op: BinOp,
        /// The expression's end (the right operand's last token end): the
        /// dead-RHS row ends there.
        end: u32,
    },
    IsBegin,
    IsEnd,
    IsOperator {
        is_not: bool,
    },
    /// The `as` cast's type spelling begins; `span` is the `as` operator
    /// token — the dead-window poll for a Never-typed operand's exit (the
    /// official `checkUnreachableNode(node.type)` face,
    /// type_analyzer.dart:2090).
    AsBegin {
        span: TokenSpan,
    },
    AsEnd,
    AsOperator,
    Assign {
        plain: bool,
        /// `??=` (official `ifNullExpression_*` wiring on the
        /// `lvalue ??= e` resolution path,
        /// assignment_expression_resolver.dart:665-707): the written type is
        /// `LUB(NonNull(readType), rhsType)` and the write promotes through
        /// types-of-interest, unlike other compound operators.
        if_null: bool,
        /// The assignment expression's end (`handleAssignmentExpression`'s
        /// `end`): a dead updater's row ends here (`i = 0`, `i += 1` — the
        /// header parts carry no statement wrapper).
        end: u32,
        /// A constructor field initializer's write (`f = e` or `this.f = e`
        /// in `C(...) : f = e`): the LHS resolves to `this.f`, not a local —
        /// the fold must not write a like-named parameter/variable
        /// (upstream parses it as `ConstructorFieldInitializer`, whose
        /// field write never enters `flow.store_local`). Set at
        /// `end_initializer` on the window's last matching-end assign.
        field_init: bool,
    },
    PatternAssign,
    CondBegin,
    /// The then arm ended at `:`: a dead arm's row spans its own range
    /// (official `flowEnd(thenExpression)`, `type_analyzer.dart:2588`).
    /// `then_end` is the arm's last token end.
    CondColon {
        then_begin: TokenSpan,
        then_end: u32,
    },
    /// The else arm ended: a dead arm's row spans its own range (official
    /// `flowEnd(elseExpression)`, `type_analyzer.dart:2616`). `end` is the
    /// conditional's last token end.
    CondEnd {
        else_begin: TokenSpan,
        end: u32,
    },
    DotAccess {
        null_aware: bool,
        /// The member expression's end token: a plain-identifier member is
        /// exactly that token — the property rows' anchor (#1409), aligned
        /// with the semantic member row's own span.
        member: TokenSpan,
    },
    /// The `?[` of a null-aware index selector was just consumed — the
    /// receiver is the top of the expression stack and the operand's events
    /// have not run yet (official `nullAwareAccess_rightBegin` timing,
    /// flow_analysis.dart:9199). The skip-path state must be captured here:
    /// by [`Ev::IndexAccess`] the operand's own `throw`/Never events have
    /// already landed on `current`.
    NullAwareIndexOperandBegin,
    /// The `?.` of a null-aware member selector was just consumed — the
    /// receiver is the top of the expression stack and the member's events
    /// (including any invocation arguments) have not run yet. Same
    /// `nullAwareAccess_rightBegin` timing as
    /// [`Ev::NullAwareIndexOperandBegin`]: by [`Ev::DotAccess`] a
    /// `throw`/Never inside `x?.m(throw _)`'s argument list has already
    /// landed on `current`.
    NullAwareMemberBegin,
    IndexAccess {
        null_aware: bool,
    },
    /// An invocation's close. `args` is the invocation's own method
    /// type-argument tree (`C<int>()`'s written `<int>`, an arena index;
    /// [`crate::ty::EMPTY_ARGS`] when argument-less or absent): the group
    /// completes with no head `handle_type` to consume it — the head is
    /// the callee identifier — so the send carries it beside the callee's
    /// implicit-creation face (#2243), and taking it here keeps a
    /// dangling group from leaking into the next construct's type row.
    Send {
        args: u32,
    },
    SendNoArgs,
    ArgumentsBegin,
    /// One argument's value expression is complete. Official analyzes an
    /// argument list one expression at a time, and each argument's
    /// null-shorting finishes at ITS own end (`finishNullShorting` at the
    /// argument expression's boundary) — before the next argument runs.
    /// Deferring every in-list short to the list's close would join a
    /// pre-promotion shortcut snapshot against promotions a LATER sibling
    /// argument established (`f(a?.b, c!)`: the `!` must survive — drift
    /// wasm_setup.dart:258-263), so the boundary fires per argument,
    /// named and positional alike.
    ArgumentEnd,
    ArgumentsEnd {
        count: usize,
    },
    Throw,
    /// `rethrow` (no operand on the expression stack).
    Rethrow,
    /// A subexpression's static type is Never (official
    /// `FlowAnalysis.handleExit`, fired with the expression's end offset —
    /// flow_analysis.dart:919-925): the flow state goes unreachable from
    /// there. The subexpression's value stays on the expression stack; the
    /// enclosing construct's own close event consumes it.
    Exit {
        /// The subexpression's end offset.
        end: TokenSpan,
    },
    ParenBegin,
    ParenEnd,
    RecordLitEnd {
        count: usize,
    },
    /// List/set/map literal end: `count` elements were parsed, each of
    /// which netted exactly one expression value.
    ListLitEnd {
        count: usize,
    },
    /// One map entry (`k: v`) ended after its value's events: the key and
    /// value each left one expression value.
    MapEntryEnd,
    CascadeBegin {
        /// The cascade operator is `?..` (official
        /// `cascadeExpression_afterTarget`'s `isNullAware`): the sections
        /// run on the target's not-null path and the skip state joins at
        /// [`Ev::CascadeEnd`] (`nullAwareAccess_end`,
        /// flow_analysis.dart:7400).
        null_aware: bool,
    },
    CascadeAccess {
        null_aware: bool,
    },
    CascadeEnd,
    EndLiteralString {
        interpolation_count: usize,
    },
    Spread,
    NullAwareElement,
    IfControlFlowBegin,
    /// `arm_start`: the then-element's first token offset (official
    /// `handleThenControlFlow` fires right after the condition's `)`).
    ThenControlFlow {
        arm_start: u32,
    },
    /// `then_end`: the else keyword's offset (the then-element's
    /// exclusive end); `else_start`: the else-element's first token
    /// offset. Both are dartq-internal faces — the dead-arm row anchors
    /// the arm's own extent (official `handle_ifElement_thenEnd` /
    /// `handle_ifElement_elseEnd` → `flowEnd(ifTrue)` / `flowEnd(ifFalse)`,
    /// type_analyzer.dart:1163-1173), and the parser hands the offsets to
    /// the listener because the flow walker never sees tokens.
    ElseControlFlow {
        then_end: u32,
        else_start: u32,
    },
    /// `arm_end`: the last element's exclusive end (the parser's peek
    /// token offset at the close) — the then-element's end when
    /// `has_else` is false, the else-element's end when true.
    EndIfControlFlow {
        has_else: bool,
        arm_end: u32,
    },
    ForControlFlowBegin,
    ForControlFlowEnd {
        for_in: bool,
    },

    // —— patterns ——
    PatternBegin,
    PatternEnd,
    /// An object pattern's head: `Some` in `case Some():`. The head type
    /// constrains the matched value (official `analyzeObjectPattern`'s
    /// `promoteForPattern`, shared type_analyzer.dart:1749-1772), so the
    /// flow walk promotes the switched variable inside the matched clause.
    /// Only the bare `Type(...)` shape records: a `Type.field` shorthand
    /// head names a member, not a type (the same gate the semantic layer's
    /// object-field face applies).
    ObjectPattern {
        /// The interned head type name.
        name: u32,
        /// The written type-argument tree (arena index; [`EMPTY_ARGS`]
        /// when the head spells no arguments).
        args: u32,
    },
    ConstantPatternBegin,
    ConstantPatternEnd,
    /// A pattern shape whose match can fail without pinning a named type:
    /// record, list, map, and relational patterns. At the top level of a
    /// refutable context the failure arm joins the pattern context's
    /// `unmatched` chain (official `_unmatched` accumulation — without it
    /// the else arm reads unreachable and code after an `if (v case (a, b))`
    /// folds dead). Sub-patterns constrain their own field, not the
    /// scrutinee — the depth gate in the handler keeps them out.
    ShapePattern,
    /// A null-check pattern (`sub?`): the matched arm promotes the matched
    /// value non-null; the failure arm (the null path) joins `unmatched`.
    NullCheckPattern,
    /// A declared-variable or wildcard pattern carrying a written type
    /// (`case int n`, `case num _`): a type test on the matched value — the
    /// matched arm promotes, the failure arm joins `unmatched`. An untyped
    /// `var x`/`_` always matches and never fires here.
    DeclaredVarPattern {
        name: u32,
        is_final: bool,
        /// The declared identifier's own token (the write-site face's
        /// declaration anchor).
        token: TokenSpan,
    },
    AssignedVarPattern {
        name: u32,
    },
    Wildcard {
        token: TokenSpan,
    },
    GuardBegin,
    GuardEnd,
}

/// Binary operators the flow engine distinguishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    And,
    Or,
    IfNull,
    Eq,
    NotEq,
    Other,
}

impl BinOp {
    /// Classifies an operator lexeme.
    #[must_use]
    pub fn of(lexeme: &str) -> BinOp {
        match lexeme {
            "&&" => BinOp::And,
            "||" => BinOp::Or,
            "??" => BinOp::IfNull,
            "==" => BinOp::Eq,
            "!=" => BinOp::NotEq,
            _ => BinOp::Other,
        }
    }
}

impl Ev {
    /// The primary token of the event — the anchor used to partition a
    /// C-style `for` header by source offset.
    #[must_use]
    pub fn span(&self) -> Option<TokenSpan> {
        match self {
            Ev::VarDeclBegin { span, .. }
            | Ev::PatternVarDeclStmt { span }
            | Ev::Block { span, .. }
            | Ev::ReturnBegin { span }
            | Ev::YieldBegin { span }
            | Ev::LabeledBegin { span, .. }
            | Ev::AssertBegin { span }
            | Ev::IfBegin { span }
            | Ev::WhileBegin { span }
            | Ev::DoBegin { span }
            | Ev::ForBegin { span }
            | Ev::TryBegin { span }
            | Ev::SwitchBegin { span }
            | Ev::SwitchExprBegin { span }
            | Ev::ExprStmt { span }
            | Ev::Break { span, .. }
            | Ev::Continue { span, .. }
            | Ev::OtherExpr { span }
            | Ev::This { span } => Some(*span),
            Ev::ForParts { right_sep, .. } => Some(*right_sep),
            Ev::Ident { token, .. } | Ev::Wildcard { token } => Some(*token),
            _ => None,
        }
    }
}
