//! The recorded event subset: what the recorder stores per executable from
//! the parser's listener stream.
//!
//! Only flow-relevant events are recorded (the full listener contract is
//! three times wider); each variant carries exactly the payload the replay
//! interpreter needs. `Ev::span` gives every event a partition anchor so
//! the interpreter can split a C-style `for` header into its
//! initializers/condition/updaters regions by token offset.

use dartq_parser::{IdentifierContext, TokenSpan};

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
    },
    VarDeclBegin {
        late: bool,
        is_final: bool,
        span: TokenSpan,
    },
    VarDeclEnd,
    InitializedIdent {
        name: u32,
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
        /// Whether the type spelled type arguments (`List<int>`): the
        /// recorder records only the bare name, so a promotion tested
        /// against this type must not surface as a fact carrying the
        /// erased spelling (#611). Latched from the parser's
        /// `end_type_arguments` / `handle_no_type_arguments`, which fire
        /// immediately before the row (`type_info_impl.dart:1158/1191`).
        had_type_args: bool,
    },
    NoType,
    FunctionType,
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
    YieldEnd,
    AwaitBegin,
    AwaitEnd,
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
    /// first dead node anchors its row there (dead_code_verifier.dart:232).
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
    },
    AssertBegin {
        span: TokenSpan,
    },
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
    UnaryPrefix {
        is_not: bool,
    },
    NonNullAssert,
    UnaryPrefixAssign,
    UnaryPostfixAssign,
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
    AsBegin,
    AsEnd,
    AsOperator,
    Assign {
        plain: bool,
    },
    PatternAssign,
    CondBegin,
    /// The then arm ended at `:`: a dead arm's row spans its own range
    /// (official `flowEnd(thenExpression)`, type_analyzer.dart:2588).
    /// `then_end` is the arm's last token end.
    CondColon {
        then_begin: TokenSpan,
        then_end: u32,
    },
    /// The else arm ended: a dead arm's row spans its own range (official
    /// `flowEnd(elseExpression)`, type_analyzer.dart:2616). `end` is the
    /// conditional's last token end.
    CondEnd {
        else_begin: TokenSpan,
        end: u32,
    },
    DotAccess {
        null_aware: bool,
    },
    IndexAccess {
        null_aware: bool,
    },
    Send,
    SendNoArgs,
    ArgumentsBegin,
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
    CascadeBegin,
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
    ThenControlFlow,
    ElseControlFlow,
    EndIfControlFlow {
        has_else: bool,
    },
    ForControlFlowBegin,
    ForControlFlowEnd {
        for_in: bool,
    },

    // —— patterns ——
    PatternBegin,
    PatternEnd,
    ConstantPatternBegin,
    ConstantPatternEnd,
    DeclaredVarPattern {
        name: u32,
        is_final: bool,
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
            | Ev::OtherExpr { span } => Some(*span),
            Ev::ForParts { right_sep, .. } => Some(*right_sep),
            Ev::Ident { token, .. } | Ev::Wildcard { token } => Some(*token),
            _ => None,
        }
    }
}
