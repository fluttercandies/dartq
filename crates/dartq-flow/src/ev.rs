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
    ExecEnd,
    BlockBodyBegin,
    BlockBodyEnd,
    /// `=>` body: the expression was already processed; this is the exit.
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
    EndBlock,

    // —— types ——
    Type {
        /// The name-level base: a named type's interned id, or one of the
        /// keyword bases (`dynamic`/`void`/`Never`) resolved at record
        /// time.
        base: crate::ty::Base,
        nullable: bool,
    },
    NoType,
    FunctionType,
    VoidType,

    // —— statements ——
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
    DoEnd,
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
        span: TokenSpan,
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
    },
    CaseExprBegin,
    CaseNoWhen,
    CaseExprEnd {
        has_guard: bool,
    },
    CaseEnd {
        statement_count: usize,
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
    ExpressionFunctionBodyEnd,

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
    },
    BinaryEnd {
        op: BinOp,
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
    CondColon,
    CondEnd,
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
    ParenBegin,
    ParenEnd,
    RecordLitEnd {
        count: usize,
    },
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
    pub fn span(&self) -> Option<TokenSpan> {
        match self {
            Ev::VarDeclBegin { span, .. }
            | Ev::PatternVarDeclStmt { span }
            | Ev::Block { span, .. }
            | Ev::ReturnBegin { span }
            | Ev::YieldBegin { span }
            | Ev::LabeledBegin { span }
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
