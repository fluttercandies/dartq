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
///
/// Bits 1..=7 carry the kind's modifier set (`ModSet`, collapse map §1) —
/// #174 eliminated the two-word `ModSet` payload by moving its `flags: u16`
/// here and deriving its `first` token (`flags != 0` ⇒ the first modifier is
/// the node's span head: the official parser always computes declaration
/// begin tokens as the first token after metadata, so a non-empty modifier
/// set starts the span; `flags == 0` ⇒ absent, [`crate::TOKEN_NONE`]
/// semantics). Bit *semantics* are the modifier families of collapse map §1
/// (class: augment/abstract/sealed/base/interface/final/mixin; member:
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
/// word; layout spec §1.3, #151). Per-variant fields are the collapse map §3
/// payload columns minus every derivable word: the #151 span-head pass, then
/// the #174 second pass (span-head tokens, end-of-span `;`/bracket tokens,
/// head-when-present keywords, `ModSet` into [`flags`], the nullable `?`),
/// each with its SDK citation in `docs/facts/ast-collapse-map.md` §1/§3. The
/// widest kind is [`NodeKind::FormalParameter`] at 5 words, the single
/// tolerated outlier of the ≤30 B/node average (layout spec §1.5).
///
/// Width 0 by shape: fieldless kinds, the ~64 #151 span-head eliminations
/// (map §1), and the #174 eliminations `TopLevelFields` (ModSet),
/// `Type` (`?`), `MapPattern`/`ListPattern` (braces), `NativeFunctionBody`
/// (native + `;`), `ConstantPattern` (`const`).
///
/// Exhaustive over the enum on purpose: adding a variant without declaring
/// its payload width is a compile error (R4 tables-lockstep guard).
#[must_use]
#[allow(clippy::too_many_lines)] // a 193-kind table, not logic
pub(crate) fn payload_len(kind: NodeKind) -> u8 {
    use NodeKind::*;
    match kind {
        // Widest kind: receiver/name/init range/kind (ModSet moved into the
        // header flag bits, #174).
        FormalParameter => 5,
        // ModSet → header bits (#174): class/extends/with/implements and
        // kind/(new|get_set)/param/initializers remain.
        ClassDeclaration | NamedMixinApplication | Method | Constructor => 4,
        // ForParts/ForInParts: the `for` token is the span head (map §3.4:
        // ForLoopPartsHandle/ForInLoopPartsHandle begin at it), derived;
        // lparen/left_sep/right_sep + update_count, and
        // await/lparen/pattern_kw/in, remain (width 5→4).
        ForParts | ForInParts => 4,
        MixinDeclaration
        | ExtensionTypeDeclaration
        | EnumDeclaration
        | ExtensionDeclaration
        // Assert: `assert` is the span head, derived.
        | Assert
        | ExpressionBody
        | PrimaryConstructor
        | PrimaryConstructorBody
        | SwitchCase
        | MapEntry
        | ObjectPattern => 3,
        Typedef
        | Factory
        // ConditionalUri: `if` is the span head; CatchClause: `on` is
        // head-when-present; ConstructorReference: `start` is the span head;
        // OperatorName: `operator` is the span head; ForControlFlow: `for`
        // follows `await` (both #174, map §3).
        | ConditionalUri
        | CatchClause
        | ConstructorReference
        | OperatorName
        | ForControlFlow
        | LibraryAugmentation
        | PartOf
        | Import
        | EnumElement
        | Conditional
        | CascadeAccess
        | DotAccess
        | SetOrMapLiteral
        | AnonymousMethodInvocation
        | BlockBody
        | RestPattern
        | FunctionType
        | RecordType
        | TypeVariable
        | UnescapeError
        | SwitchExpressionCase
        | CaseExpression
        | ParenthesizedCondition => 2,
        TopLevelMethod
        | Fields
        // Break/Continue: keyword = span head, `;` = last span token;
        // has_target is not derivable (`break 123;` recovery keeps a
        // non-identifier between keyword and `;`, map §3.4).
        | Break
        | Continue
        // PatternVarDeclStatement: keyword = span head, `;` = last token,
        // `=` kept. SkippedBody: begin/end = span endpoints, is_expression
        // kept. Indexed: `open` follows the optional `?`, `close` is the last
        // token. VariablesDeclaration: `late` is head-when-present.
        // YieldStatement/InvalidYieldStatement: keyword/begin = span head.
        // Identifier: token = span head, ctx kept (#174).
        // DeclaredVariablePattern/WildcardPattern: `final`/`var` is
        // head-when-present. TryStatement: `try` is the span head.
        | PatternVarDeclStatement
        | Indexed
        | SkippedBody
        | VariablesDeclaration
        | YieldStatement
        | InvalidYieldStatement
        | Identifier
        | DeclaredVariablePattern
        | WildcardPattern
        | TryStatement
        // Kinds whose single field is real data: bool / Option / k:u8 / a
        // non-head token (colon-like). All Fielded — built via begin/patch/end.
        | LibraryName
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
        | CastPattern
        | MapPatternEntry
        | PatternField
        | NamedRecordField
        | ExperimentNotEnabled => 1,
        // Span-only and token-leaf kinds: no payload words. First block: the
        // original fieldless set plus the #151 derivable-word0 eliminations
        // and `Send`/`ExpressionStatement` (span endpoints). Second block:
        // the #174 eliminations — `TopLevelFields` (ModSet → header bits),
        // `Type` (`?` = last span token, map §3.8), `MapPattern`/
        // `ListPattern` (braces = span endpoints), `NativeFunctionBody`
        // (native = span head, `;` = last token), `ConstantPattern`
        // (`const` = head-when-present, map §3.7).
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
        | ExpressionStatement
        | TopLevelFields
        | Type
        | MapPattern
        | ListPattern
        | NativeFunctionBody
        | ConstantPattern => 0,
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
    /// its slot. The pattern (every word written explicitly; debug builds
    /// poison-fill the slot at `begin` and sweep it at `end`, so a missed
    /// write fails there — release trusts the parser):
    ///
    /// ```
    /// use dartiq_ast::{AstBuilder, NodeKind};
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
