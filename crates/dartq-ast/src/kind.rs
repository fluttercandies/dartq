//! The node-kind face: one [`NodeKind`] per dartq AST node, plus the two
//! static kind tables the arena relies on ([`payload_len`] — the length of
//! the kind's row in [`crate::fields::PAYLOAD_FIELDS`] — and
//! [`accepts_begin`]).
//!
//! The variant set is the adjudicated collapse of the official 377 parser
//! node types (111 Begin / 152 Handle / 114 End in upstream Dart SDK
//! source, `parser_ast_helper.dart`): Begin types never land as
//! variants (their payload merges into the matching End variant), Handles
//! land as leaf variants, field back-fills, or are derivable and dropped.
//! The per-variant mapping, payloads, and event-pairing verdicts were
//! adjudicated as #31 and are embodied in this crate (the per-domain
//! section comments below and the crate-test width transcript); the
//! header/pool split that carries those payloads was adjudicated as #92:
//! every node's span token pair lives in its 24-byte header, and the fields
//! below are the *extra* payload words appended to the shared `u32` pool.
//!
//! Variants are declared in the collapse map's domain order; the domain
//! section comments give the map section and the official Begin/Handle/End
//! census each domain collapses.

#![allow(clippy::enum_glob_use)] // the width/pairing tables read best over the full kind set

/// The discriminant of one AST node. `u16`-sized by design (part of the
/// 24-byte `crate::table` node header); space for future milestone
/// extensions is ample.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum NodeKind {
    // Group 3.1: compilation unit and directives (Begin 12 / Handle 6 / End 13).
    CompilationUnit,
    /// Wrapper attributing a top-level name Identifier (any of the five
    /// official preludes opens it; one of the five shared-begin groups).
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
    /// Dead today: the unit parser consumes the `#!` line without a
    /// listener event, so the bridge cannot build this leaf; kept as the
    /// collapse-map row for the official node.
    ScriptTag,

    // Group 3.2: top-level and class-like declarations (Begin 16 / Handle 23 / End 11).
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

    // Group 3.3: members (Begin 20 / Handle 8 / End 21).
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
    /// Dead today: no listener event builds this kind; kept as the
    /// collapse-map row for the official node.
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

    // Group 3.4: statements (Begin 27 / Handle 20 / End 29).
    Block,
    /// Dead today: no recovery path emits this sentinel; kept as the
    /// collapse-map row for the official node.
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

    // Group 3.5: expressions (Begin 22 / Handle 53 / End 24).
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
    /// Dead today: `new`-as-identifier resolves through `Identifier` (the
    /// expression-continuation context), never this leaf; kept as the
    /// collapse-map row for the official handle.
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
    /// Dead today: no listener event builds this kind; kept as the
    /// collapse-map row for the official node.
    AnonymousMethodInvocation,
    Identifier,
    NoIdentifier,
    NoName,
    Qualified,
    SuperExpression,
    ThisExpression,
    Operator,
    OperatorName,
    /// Dead today: no recovery path emits this sentinel; kept as the
    /// collapse-map row for the official node.
    InvalidExpression,
    /// Dead today: no recovery path emits this sentinel; kept as the
    /// collapse-map row for the official node.
    ExtraneousExpression,
    Spread,
    NullAwareElement,

    // Group 3.6: control-flow sugar in collections (Begin 2 / Handle 2 / End 4).
    ForControlFlow,
    IfControlFlow,
    ThenControlFlow,
    ElseControlFlow,

    // Group 3.7: patterns (Begin 4 / Handle 18 / End 4).
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

    // Group 3.8: types (Begin 8 / Handle 9 / End 8).
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

    // Group 3.9: function bodies and error sentinels (Handle 13).
    EmptyBody,
    ExpressionBody,
    /// Dead today: missing bodies resolve to `EmptyBody`/`SkippedBody`; no
    /// listener event builds this kind — kept as the collapse-map row for
    /// the official node.
    NoBody,
    /// Diet-parsed body carrying the skipped span (declarations milestone).
    SkippedBody,
    NativeFunctionBody,
    InvalidFunctionBody,
    /// Dead today: no recovery path emits this sentinel; kept as the
    /// collapse-map row for the official node.
    InvalidTypeReference,
    RecoverableError,
    ExperimentNotEnabled,
    /// Dead today: scanner-error faces are never stored — diagnostics flow
    /// through the listener and the AST carries only the error flag; kept
    /// as the collapse-map row for the official handle.
    ErrorToken,
    /// Dead today: scanner-error faces are never stored — diagnostics flow
    /// through the listener and the AST carries only the error flag; kept
    /// as the collapse-map row for the official handle.
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
/// then a flag read, not a diagnostics join.
///
/// Bits 1..=7 carry the kind's modifier set —
/// #174 eliminated the two-word `ModSet` payload by moving its `flags: u16`
/// here and deriving its `first` token (`flags != 0` ⇒ the first modifier is
/// the node's span head: the official parser always computes declaration
/// begin tokens as the first token after metadata, so a non-empty modifier
/// set starts the span; `flags == 0` ⇒ absent, [`crate::TOKEN_NONE`]
/// semantics). Bit *semantics* are the modifier families (class: augment/abstract/sealed/base/interface/final/mixin; member:
/// augment/external/static/covariant/late/varFinalOrConst) — the parser lane
/// assigns bit values inside the mask.
pub(crate) mod flags {
    pub(crate) const ERROR: u16 = 1 << 0;
    /// Low bit of the `ModSet` field inside `NodeHeader.flags`.
    pub(crate) const MOD_SHIFT: u16 = 1;
    /// Mask of the 7 modifier bits (bits 1..=7); the rest of the `u16` stays
    /// spare.
    pub(crate) const MOD_MASK: u16 = 0b0111_1111 << MOD_SHIFT;
}

/// Number of shared-pool `u32` words the kind's payload occupies (zero for
/// span-only and token-leaf kinds — the span token pair lives in the header,
/// and a token leaf's token *is* its span head, so both store no payload
/// word; the #151 layout pass). Per-variant fields are the payload
/// columns minus every derivable word: the #151 span-head pass, then
/// the #174 second pass (span-head tokens, end-of-span `;`/bracket tokens,
/// head-when-present keywords, `ModSet` into [`flags`], the nullable `?`),
/// each traceable to its upstream SDK source. The
/// widest kind is [`NodeKind::FormalParameter`] at 5 words, the single
/// tolerated outlier of the ≤30 B/node average.
///
/// The width IS the length of the kind's row in
/// [`crate::fields::PAYLOAD_FIELDS`] (the named layout table); the
/// per-kind derivability rationale lives in the width transcript
/// (`tests::COLLAPSE_MAP_WIDTHS`), and `tests::payload_width_lockstep`
/// keeps the table and the transcript in lockstep — adding a variant
/// without a table row is a compile error (the array length is
/// `KIND_COUNT`), and a wrong-width row fails the transcript test (R4
/// tables-lockstep guard).
#[must_use]
pub(crate) fn payload_len(kind: NodeKind) -> u8 {
    crate::fields::PAYLOAD_FIELDS[kind as usize].len() as u8
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
    /// its slot. The pattern (every word written explicitly; debug builds
    /// poison-fill the slot when it materializes and sweep it at `end`, so a
    /// missed write fails there — release trusts the parser):
    ///
    /// ```
    /// use dartq_ast::{AstBuilder, NodeKind};
    ///
    /// // Identifier { ctx } — the token was eliminated as the span head
    /// // (#174); ctx is word 0.
    /// let mut b = AstBuilder::new();
    /// b.begin(NodeKind::CompilationUnit, 0);
    /// b.begin(NodeKind::Identifier, 7);
    /// b.patch(0, 3); // word 0: IdentifierContext discriminant
    /// b.end(NodeKind::Identifier, 8);
    /// b.end(NodeKind::CompilationUnit, 8);
    /// let ast = b.finish();
    /// assert_eq!(ast.payload(0), &[3]);
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
/// form of the official begin/end pairing compatibility table (13
/// groups). All groups collapse to same-kind pairing except the five
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
        // The five shared-begin exception groups.
        (TopLevelFields, Fields | TopLevelFields)
        | (ForIn, ForStatement | ForIn)
        | (InvalidAwaitExpression, AwaitExpression | InvalidAwaitExpression)
        | (InvalidYieldStatement, YieldStatement | InvalidYieldStatement)
        // #12/#13: canonical open flipped to the wider RecordLiteral (#151).
        | (ParenthesizedExpression | RecordLiteral, RecordLiteral) => true,
        (end, open) => end == open,
    }
}
