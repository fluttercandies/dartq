//! The node-kind face: one [`NodeKind`] per dartiq AST node, plus the two
//! static kind tables the arena relies on ([`payload_len`] and
//! [`accepts_begin`]).
//!
//! The variant set is the adjudicated collapse of the official 377 parser
//! node types (111 Begin / 152 Handle / 114 End in
//! `.third/dart-sdk/.../parser_ast_helper.dart`): Begin types never land as
//! variants (their payload merges into the matching End variant), Handles
//! land as leaf variants, field back-fills, or are derivable and dropped.
//! The per-variant mapping, payloads, and event-pairing verdicts are
//! `docs/facts/ast-collapse-map.md` (goal #31); the header/pool split that
//! carries those payloads is `docs/facts/ast-layout-spec.md` (goal #92):
//! every node's span token pair lives in its 24-byte header, and the fields
//! below are the *extra* payload words appended to the shared `u32` pool.
//!
//! Variants are declared in the collapse map's domain order; the domain
//! section comments give the map section and the official Begin/Handle/End
//! census each domain collapses.

#![allow(clippy::enum_glob_use)] // the width/pairing tables read best over the full kind set

/// The discriminant of one AST node. `u16`-sized by design (part of the
/// 24-byte [`crate::table`] node header); space for future milestone
/// extensions is ample.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum NodeKind {
    // §3.1 Compilation unit and directives (Begin 12 / Handle 6 / End 13).
    CompilationUnit,
    /// Wrapper attributing a top-level name Identifier (any of the five
    /// official preludes opens it; collapse map §2 #1-5).
    TopLevelDeclaration,
    LibraryName,
    LibraryAugmentation,
    Part,
    PartOf,
    Import,
    RecoverImport,
    Export,
    Show,
    Hide,
    ConditionalUri,
    DottedName,
    ScriptTag,

    // §3.2 Top-level and class-like declarations (Begin 16 / Handle 23 / End 11).
    /// Class/mixin/extension/extension-type body wrapper (member count is
    /// the children length).
    DeclarationBody,
    ClassDeclaration,
    MixinDeclaration,
    NamedMixinApplication,
    ExtensionDeclaration,
    ExtensionTypeDeclaration,
    EnumDeclaration,
    EnumBody,
    EnumElement,
    Typedef,
    PrimaryConstructor,
    NoPrimaryConstructor,
    PrimaryConstructorBody,
    RecoverDeclarationHeader,
    InvalidTopLevelDeclaration,

    // §3.3 Members (Begin 20 / Handle 8 / End 21).
    TopLevelFields,
    TopLevelMethod,
    Fields,
    Method,
    Constructor,
    Factory,
    RedirectingFactoryBody,
    FormalParameter,
    FormalParameters,
    OptionalFormalParameters,
    NoFormalParameters,
    ImplicitFormalParameters,
    FunctionTypedFormalParameter,
    VariablesDeclaration,
    InitializedIdentifier,
    FieldInitializer,
    VariableInitializer,
    Initializer,
    Initializers,
    MetadataStar,
    Metadata,
    InvalidMember,

    // §3.4 Statements (Begin 27 / Handle 20 / End 29).
    Block,
    InvalidTopLevelBlock,
    LabeledStatement,
    Label,
    IfStatement,
    ParenthesizedCondition,
    ForStatement,
    ForParts,
    ForIn,
    ForInParts,
    WhileStatement,
    DoWhileStatement,
    SwitchStatement,
    SwitchBlock,
    SwitchCase,
    SwitchCaseWhenClause,
    SwitchExpression,
    SwitchExpressionBlock,
    SwitchExpressionCase,
    CaseExpression,
    CatchClause,
    TryStatement,
    ReturnStatement,
    RethrowStatement,
    YieldStatement,
    InvalidYieldStatement,
    Assert,
    Break,
    Continue,
    EmptyStatement,
    ExpressionStatement,
    PatternVarDeclStatement,
    PatternAssignment,
    InvalidStatement,

    // §3.5 Expressions (Begin 22 / Handle 53 / End 24).
    Arguments,
    NamedArgument,
    Send,
    NoArguments,
    TypeApplication,
    Indexed,
    BinaryExpression,
    Assignment,
    UnaryPrefix,
    UnaryPostfix,
    NonNullAssert,
    Conditional,
    ConstExpression,
    ConstLiteral,
    NewExpression,
    NewAsIdentifier,
    AwaitExpression,
    InvalidAwaitExpression,
    Cascade,
    CascadeAccess,
    DotAccess,
    DotShorthandHead,
    ConstDotShorthand,
    ThrowExpression,
    ParenthesizedExpression,
    RecordLiteral,
    StringLiteral,
    LiteralBool,
    LiteralInt,
    LiteralDouble,
    LiteralNull,
    ListLiteral,
    SetOrMapLiteral,
    MapEntry,
    SymbolLiteral,
    SymbolVoid,
    AsExpression,
    IsExpression,
    ImplicitCreation,
    ConstructorReference,
    NoCtorRefContinuation,
    NoTypeNameInCtorRef,
    NamedFunctionExpression,
    LocalFunctionDeclaration,
    BlockBody,
    FunctionName,
    FunctionExpression,
    AnonymousMethodInvocation,
    Identifier,
    NoIdentifier,
    NoName,
    Qualified,
    SuperExpression,
    ThisExpression,
    Operator,
    OperatorName,
    InvalidExpression,
    ExtraneousExpression,
    Spread,
    NullAwareElement,

    // §3.6 Control-flow sugar in collections (Begin 2 / Handle 2 / End 4).
    ForControlFlow,
    IfControlFlow,
    ThenControlFlow,
    ElseControlFlow,

    // §3.7 Patterns (Begin 4 / Handle 18 / End 4).
    PatternGuard,
    BinaryPattern,
    ConstantPattern,
    CastPattern,
    RecordPattern,
    ParenthesizedPattern,
    ObjectPattern,
    MapPattern,
    MapPatternEntry,
    ListPattern,
    RelationalPattern,
    AssignedVariablePattern,
    DeclaredVariablePattern,
    WildcardPattern,
    PatternField,
    NamedRecordField,
    RestPattern,
    NullAssertPattern,
    NullCheckPattern,

    // §3.8 Types (Begin 8 / Handle 9 / End 8).
    /// A named type reference (the official `TypeHandle` family — the
    /// highest-frequency leaf).
    Type,
    NoType,
    VoidType,
    FunctionType,
    RecordType,
    RecordTypeEntry,
    RecordTypeNamedFields,
    TypeArguments,
    InvalidTypeArguments,
    NoTypeArguments,
    TypeVariable,
    TypeVariables,
    NoTypeVariables,
    NativeClause,

    // §3.9 Function bodies and error sentinels (Handle 13).
    EmptyBody,
    ExpressionBody,
    NoBody,
    /// Diet-parsed body carrying the skipped span (declarations milestone).
    SkippedBody,
    NativeFunctionBody,
    InvalidFunctionBody,
    InvalidTypeReference,
    RecoverableError,
    ExperimentNotEnabled,
    ErrorToken,
    UnescapeError,
}

impl NodeKind {
    /// Number of discriminants; `NodeHeader.kind` values written by the
    /// builder are always below this.
    pub const KIND_COUNT: u16 = Self::UnescapeError as u16 + 1;

    /// The underlying discriminant.
    #[must_use]
    pub const fn raw(self) -> u16 {
        self as u16
    }

    /// The kind with discriminant `raw`. Test-only helper for table-lockstep
    /// iterations (R4); production code passes `NodeKind` values around
    /// directly and never converts back from raw.
    ///
    /// # Panics
    ///
    /// Panics when `raw >= KIND_COUNT`. The `transmute` inside is sound
    /// because `NodeKind` is `repr(u16)` with contiguous discriminants
    /// `0..KIND_COUNT` and no explicit discriminants.
    #[cfg(test)]
    pub(crate) fn from_raw(raw: u16) -> Self {
        assert!(raw < Self::KIND_COUNT);
        // SAFETY: `raw < KIND_COUNT` and the discriminants are exactly
        // `0..KIND_COUNT`.
        unsafe { core::mem::transmute::<u16, Self>(raw) }
    }
}

/// Node flag bits (`NodeHeader.flags`). A node carries an error bit when an
/// error was marked inside it, directly or through propagation from a
/// descendant at end-event time; "does this declaration contain errors" is
/// then a flag read, not a diagnostics join (layout spec §3).
pub(crate) mod flags {
    pub(crate) const ERROR: u16 = 1 << 0;
}

/// Number of shared-pool `u32` words the kind's payload occupies (zero for
/// span-only nodes — the span token pair lives in the header, layout spec
/// §1.3). Per-variant fields are the collapse map §3 payload columns minus
/// the span words; the widest kind is [`NodeKind::FormalParameter`] at 7
/// words, the single tolerated outlier of the ≤30 B/node average (layout
/// spec §1.5).
///
/// Exhaustive over the enum on purpose: adding a variant without declaring
/// its payload width is a compile error (R4 tables-lockstep guard).
#[must_use]
#[allow(clippy::too_many_lines)] // a 193-kind table, not logic
pub(crate) fn payload_len(kind: NodeKind) -> u8 {
    use NodeKind::*;
    match kind {
        // ModSet-bearing declaration kinds: { flags: u16, first token } is
        // 2 words (collapse map §1).
        FormalParameter => 7,
        ClassDeclaration | NamedMixinApplication | Method | Constructor => 6,
        MixinDeclaration | ExtensionTypeDeclaration | EnumDeclaration | ForParts | ForInParts => 5,
        ExtensionDeclaration | Typedef | Factory | Assert | ExpressionBody => 4,
        ConditionalUri
        | TopLevelMethod
        | Fields
        | PrimaryConstructor
        | PrimaryConstructorBody
        | ParenthesizedCondition
        | SwitchCase
        | CatchClause
        | Break
        | Continue
        | PatternVarDeclStatement
        | Indexed
        | ConstructorReference
        | OperatorName
        | MapEntry
        | ObjectPattern
        | ForControlFlow
        | SkippedBody => 3,
        LibraryAugmentation
        | PartOf
        | Import
        | EnumElement
        | TopLevelFields
        | VariablesDeclaration
        | ExpressionStatement
        | YieldStatement
        | InvalidYieldStatement
        | Send
        | Conditional
        | CascadeAccess
        | DotAccess
        | SetOrMapLiteral
        | AnonymousMethodInvocation
        | Identifier
        | BlockBody
        | MapPattern
        | ListPattern
        | DeclaredVariablePattern
        | WildcardPattern
        | RestPattern
        | Type
        | FunctionType
        | RecordType
        | TypeVariable
        | NativeFunctionBody
        | UnescapeError
        | SwitchExpressionCase
        | CaseExpression
        | TryStatement => 2,
        LibraryName
        | RecoverImport
        | Show
        | Hide
        | ScriptTag
        | DottedName
        | NoPrimaryConstructor
        | RecoverDeclarationHeader
        | FormalParameters
        | OptionalFormalParameters
        | NoFormalParameters
        | ImplicitFormalParameters
        | FunctionTypedFormalParameter
        | InitializedIdentifier
        | FieldInitializer
        | VariableInitializer
        | Metadata
        | Label
        | IfStatement
        | SwitchStatement
        | SwitchCaseWhenClause
        | SwitchExpression
        | ReturnStatement
        | EmptyStatement
        | PatternAssignment
        | Arguments
        | NamedArgument
        | TypeApplication
        | BinaryExpression
        | Assignment
        | UnaryPrefix
        | UnaryPostfix
        | NonNullAssert
        | ConstExpression
        | ConstLiteral
        | NewExpression
        | NewAsIdentifier
        | AwaitExpression
        | Cascade
        | DotShorthandHead
        | ConstDotShorthand
        | ThrowExpression
        | ParenthesizedExpression
        | RecordLiteral
        | LiteralBool
        | LiteralInt
        | LiteralDouble
        | LiteralNull
        | ListLiteral
        | SymbolLiteral
        | SymbolVoid
        | AsExpression
        | IsExpression
        | ImplicitCreation
        | NoCtorRefContinuation
        | NoTypeNameInCtorRef
        | FunctionName
        | FunctionExpression
        | Qualified
        | SuperExpression
        | ThisExpression
        | Operator
        | Spread
        | NullAwareElement
        | IfControlFlow
        | ThenControlFlow
        | ElseControlFlow
        | PatternGuard
        | BinaryPattern
        | ConstantPattern
        | CastPattern
        | RecordPattern
        | ParenthesizedPattern
        | MapPatternEntry
        | RelationalPattern
        | AssignedVariablePattern
        | PatternField
        | NamedRecordField
        | NullAssertPattern
        | NullCheckPattern
        | NoType
        | VoidType
        | RecordTypeNamedFields
        | TypeArguments
        | InvalidTypeArguments
        | NoTypeArguments
        | TypeVariables
        | NoTypeVariables
        | NativeClause
        | EmptyBody
        | NoBody
        | ExperimentNotEnabled => 1,
        CompilationUnit
        | TopLevelDeclaration
        | Part
        | Export
        | DeclarationBody
        | EnumBody
        | InvalidTopLevelDeclaration
        | RedirectingFactoryBody
        | Initializer
        | Initializers
        | MetadataStar
        | InvalidMember
        | Block
        | InvalidTopLevelBlock
        | LabeledStatement
        | ForStatement
        | ForIn
        | WhileStatement
        | DoWhileStatement
        | SwitchBlock
        | SwitchExpressionBlock
        | RethrowStatement
        | InvalidStatement
        | NoArguments
        | InvalidAwaitExpression
        | StringLiteral
        | InvalidExpression
        | ExtraneousExpression
        | NoIdentifier
        | NoName
        | NamedFunctionExpression
        | LocalFunctionDeclaration
        | RecordTypeEntry
        | InvalidFunctionBody
        | InvalidTypeReference
        | RecoverableError
        | ErrorToken => 0,
    }
}

/// Whether a node opened as `open` may legally seal as `end` — the Rust-side
/// form of the official begin/end pairing compatibility table (collapse map
/// §2, 13 groups). All groups collapse to same-kind pairing except the five
/// whose begin cannot decide the end shape up front (shared-begin ends);
/// every other `end` accepts only a node opened as the same kind.
///
/// The builder debug-asserts this on every `end` event (R4).
#[must_use]
pub(crate) fn accepts_begin(end: NodeKind, open: NodeKind) -> bool {
    use NodeKind::*;
    match (end, open) {
        // Collapse map §2 #6, #7, #10, #11, #12/#13.
        (TopLevelFields, Fields | TopLevelFields)
        | (ForIn, ForStatement | ForIn)
        | (InvalidAwaitExpression, AwaitExpression | InvalidAwaitExpression)
        | (InvalidYieldStatement, YieldStatement | InvalidYieldStatement)
        | (RecordLiteral, ParenthesizedExpression | RecordLiteral) => true,
        (end, open) => end == open,
    }
}
