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
/// span-only and token-leaf kinds — the span token pair lives in the header,
/// and a token leaf's token *is* its span head, so both store no payload
/// word; layout spec §1.3, #151). Per-variant fields are the collapse map §3
/// payload columns minus the span words and minus span-head words dropped as
/// derivable; the widest kind is [`NodeKind::FormalParameter`] at 7 words,
/// the single tolerated outlier of the ≤30 B/node average (layout spec
/// §1.5).
///
/// Width 0 by shape: every kind whose map payload had no fields, plus the
/// former width-1 kinds whose single word was the span-head token (~64 of
/// them — literals, keyword/sentinel leaves, and head-token containers), plus
/// `Send`/`ExpressionStatement` whose two words were the span endpoints.
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
        // ParenthesizedCondition: case_/when only — the former span-head
        // `token` word is derivable from `header.first_token` (#151).
        LibraryAugmentation
        | PartOf
        | Import
        | EnumElement
        | TopLevelFields
        | VariablesDeclaration
        | YieldStatement
        | InvalidYieldStatement
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
        | TryStatement
        | ParenthesizedCondition => 2,
        // Kinds whose single field is real data: bool / Option / k:u8 / a
        // non-head token (colon-like). All Fielded — built via begin/patch/end.
        LibraryName
        | RecoverImport
        | NoPrimaryConstructor
        | RecoverDeclarationHeader
        | FunctionTypedFormalParameter
        | FieldInitializer
        | Metadata
        | IfStatement
        | PatternAssignment
        | NamedArgument
        | TypeApplication
        | BinaryExpression
        | Assignment
        | RecordLiteral
        | ListLiteral
        | SymbolVoid
        | AsExpression
        | IsExpression
        | ImplicitCreation
        | FunctionName
        | Qualified
        | BinaryPattern
        | ConstantPattern
        | CastPattern
        | MapPatternEntry
        | PatternField
        | NamedRecordField
        | ExperimentNotEnabled => 1,
        // Span-only and token-leaf kinds: no payload words. The first block
        // is the original fieldless set; the second is the #151
        // derivable-word0 elimination — former `token: T` width-1 kinds whose
        // word was the span head (read it from `header.first_token`), and
        // `Send`/`ExpressionStatement` whose two words were the span
        // endpoints.
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
        | ErrorToken
        | Show
        | Hide
        | ScriptTag
        | DottedName
        | FormalParameters
        | OptionalFormalParameters
        | NoFormalParameters
        | ImplicitFormalParameters
        | InitializedIdentifier
        | VariableInitializer
        | Label
        | SwitchStatement
        | SwitchCaseWhenClause
        | SwitchExpression
        | ReturnStatement
        | EmptyStatement
        | Arguments
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
        | LiteralBool
        | LiteralInt
        | LiteralDouble
        | LiteralNull
        | SymbolLiteral
        | NoCtorRefContinuation
        | NoTypeNameInCtorRef
        | FunctionExpression
        | SuperExpression
        | ThisExpression
        | Operator
        | Spread
        | NullAwareElement
        | IfControlFlow
        | ThenControlFlow
        | ElseControlFlow
        | PatternGuard
        | RecordPattern
        | ParenthesizedPattern
        | RelationalPattern
        | AssignedVariablePattern
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
        | Send
        | ExpressionStatement => 0,
    }
}

/// The payload-construction class of a kind (#151 contract): how the parser
/// must materialize it in the arena. Every kind is exactly one of the three,
/// and the class fully determines the legal builder entry point.
///
/// The classification is payload-based; whether a kind takes children is the
/// parser's responsibility (a leaf event creates a childless node, so only
/// childless kinds may go through [`crate::AstBuilder::leaf`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LeafShape {
    /// No payload words; the span may cover any token range. Childless
    /// instances go through `leaf`/`leaf_span`; width-0 containers go through
    /// `begin`/`end` (with nothing to patch).
    SpanOnly,
    /// The node *is* one token: its token identity is `header.first_token`
    /// (the former `token: T` payload word was eliminated as derivable,
    /// #151) and the span must be exactly `[token, token + 1)`. Built with
    /// `leaf` only.
    TokenLeaf,
    /// Carries real payload words (bool / Option / count / k:u8 / non-head
    /// token fields). Built only through `begin`/`patch`/`end` — `patch`
    /// reaches open nodes, so a leaf-emitted Fielded kind could never fill
    /// its slot. The pattern (every word written explicitly; zero-fill is
    /// never a value):
    ///
    /// ```
    /// use dartiq_ast::{AstBuilder, NodeKind};
    ///
    /// // Identifier { token, ctx } — token == span head, ctx is word 1.
    /// let mut b = AstBuilder::new();
    /// b.begin(NodeKind::CompilationUnit, 0);
    /// b.begin(NodeKind::Identifier, 7);
    /// b.patch(0, 7); // word 0: the token (explicit, even when == first_token)
    /// b.patch(1, 3); // word 1: IdentifierContext discriminant
    /// b.end(NodeKind::Identifier, 8);
    /// b.end(NodeKind::CompilationUnit, 8);
    /// let ast = b.finish();
    /// assert_eq!(ast.payload(0), &[7, 3]);
    /// ```
    Fielded,
}

/// The kind's [`LeafShape`]. Derived, not a third parallel table: width > 0
/// is exactly [`LeafShape::Fielded`], the token-leaf set is enumerated, and
/// every remaining width-0 kind is [`LeafShape::SpanOnly`].
#[must_use]
pub fn leaf_shape(kind: NodeKind) -> LeafShape {
    use NodeKind::*;
    if payload_len(kind) > 0 {
        return LeafShape::Fielded;
    }
    // Single-token nodes: the whole node is one token, so the eliminated
    // `token: T` word is exactly `header.first_token` and the span is forced
    // to `[token, token + 1)`.
    match kind {
        ScriptTag
        | NoFormalParameters
        | ImplicitFormalParameters
        | Label
        | EmptyStatement
        | NewAsIdentifier
        | DotShorthandHead
        | ConstDotShorthand
        | NoCtorRefContinuation
        | NoTypeNameInCtorRef
        | LiteralBool
        | LiteralInt
        | LiteralDouble
        | LiteralNull
        | SuperExpression
        | ThisExpression
        | Operator
        | UnaryPrefix
        | UnaryPostfix
        | NonNullAssert
        | Spread
        | NullAwareElement
        | ThenControlFlow
        | ElseControlFlow
        | AssignedVariablePattern
        | NullAssertPattern
        | NullCheckPattern
        | NoType
        | VoidType
        | InvalidTypeArguments
        | NoTypeArguments
        | NoTypeVariables
        | EmptyBody
        | NoBody => LeafShape::TokenLeaf,
        _ => LeafShape::SpanOnly,
    }
}

/// Whether a node opened as `open` may legally seal as `end` — the Rust-side
/// form of the official begin/end pairing compatibility table (collapse map
/// §2, 13 groups). All groups collapse to same-kind pairing except the five
/// whose begin cannot decide the end shape up front (shared-begin ends);
/// every other `end` accepts only a node opened as the same kind.
///
/// The shared-begin groups always open as the *canonical* (widest) kind of
/// the group — the payload slot is reserved at `begin` and must fit every
/// end the node may seal as. For #12/#13 that canonical kind is
/// `RecordLiteral` (width 1) since #151 dropped `ParenthesizedExpression` to
/// width 0.
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
        // #12/#13: canonical open flipped to the wider RecordLiteral (#151).
        | (ParenthesizedExpression, RecordLiteral)
        | (RecordLiteral, RecordLiteral) => true,
        (end, open) => end == open,
    }
}
