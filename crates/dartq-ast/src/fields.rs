//! The named payload-field table: the payload layout of every
//! [`NodeKind`] as checked-in data. Before this table the layout of each
//! kind's shared-pool words lived only in prose (the bridge's per-event
//! contracts and the collapse-map verdicts), and every writer/reader
//! addressed words by index — a wrong index compiled and failed only in
//! some downstream test. Here one row per kind names each word in payload
//! order, [`payload_len`](crate::kind::payload_len) derives from the
//! table, and the named accessors
//! ([`SyntaxTable::field`](crate::SyntaxTable::field) /
//! [`AstBuilder::patch_field`](crate::AstBuilder::patch_field)) resolve
//! names to indices through it — the arena analogue of the typed
//! accessor layer a generated syntax tree gives an analyzer, at the cost
//! of one `const` table.
//!
//! Names are the parser-event parameters that write the words (the bridge
//! keeps one writer per word), so the table doubles as the index of the
//! per-kind write contract. The dead collapse-map rows
//! (`AnonymousMethodInvocation`, `UnescapeError`) carry
//! [`PayloadField::Reserved0`]-family placeholders: no event writes them,
//! and their widths are pinned by the width transcript
//! (`tests::COLLAPSE_MAP_WIDTHS`) like every other kind.

use crate::kind::NodeKind;

/// One named payload word. The vocabulary is shared across kinds: the same
/// name means the same grammatical role wherever it appears (`ConstKeyword`
/// is the `const` token of whichever literal carries it), a kind's row
/// never repeats a name, and the word's payload order is the row order.
///
/// Value classes (documented per group, enforced by the writers):
/// token words hold a [`TokenRef`](crate::TokenRef) or
/// [`TOKEN_NONE`](crate::TOKEN_NONE) when absent; discriminant words hold a
/// parser enum's `as u32`; flag words hold `0`/`1`; count words hold a
/// `usize` count.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PayloadField {
    // —— keyword / separator token words ————————————————————————————————
    /// The `augment` keyword (augmented library/enum element/body).
    AugmentKeyword,
    /// The `library` keyword of `augment library`.
    LibraryKeyword,
    /// The import prefix's `as` keyword.
    AsKeyword,
    /// The import prefix's `deferred` keyword.
    DeferredKeyword,
    /// A `;` token (the recovery-import anchor; the arrow body's `;`).
    Semicolon,
    /// A `(` token.
    LeftParen,
    /// A `==` token (the conditional URI's DFE config equality).
    EqualSign,
    /// The `of` keyword of `part of`.
    OfKeyword,
    /// The `class` keyword (also the head keyword of a named mixin
    /// application, whose declaration starts at `class`).
    ClassKeyword,
    /// The `extends` keyword.
    ExtendsKeyword,
    /// The `with` keyword of a with-clause.
    WithKeyword,
    /// The `implements` keyword.
    ImplementsKeyword,
    /// The `on` keyword (mixin `on`, extension `on`).
    OnKeyword,
    /// The `=` token (typedef, pattern assignment/declaration).
    Equals,
    /// The `mixin` keyword of a mixin declaration.
    MixinKeyword,
    /// The `extension` keyword.
    ExtensionKeyword,
    /// The `type` keyword of an extension type.
    TypeKeyword,
    /// The `enum` keyword.
    EnumKeyword,
    /// The `{` of an enum body.
    LeftBrace,
    /// The `typedef` keyword.
    TypedefKeyword,
    /// The `new` keyword of a constructor.
    NewKeyword,
    /// The `factory` keyword.
    FactoryKeyword,
    /// The `this` head keyword of a primary-constructor body.
    ThisKeyword,
    /// The `this`/`super` receiver token of a field/super formal.
    Receiver,
    /// The `get`/`set` keyword of a getter/setter member.
    GetOrSet,
    /// The `late` keyword of a variables declaration.
    LateKeyword,
    /// The `const` keyword of a const-flavored literal/constructor.
    ConstKeyword,
    /// The `finally` keyword.
    FinallyKeyword,
    /// The `else` keyword.
    ElseKeyword,
    /// The `case` keyword of a case condition.
    CaseKeyword,
    /// The `when` keyword of a case guard.
    WhenKeyword,
    /// The `(`-side separator of a C-style for header (`;`/`:`).
    LeftSeparator,
    /// The `)`-side separator of a C-style for header (`;`/`)`).
    RightSeparator,
    /// The `await` keyword (await-for, await-for control flow).
    AwaitKeyword,
    /// The pattern-position keyword of a for-in header (`final`/`var`).
    PatternKeyword,
    /// The `in` keyword of a for-in header.
    InKeyword,
    /// The `*` of `async*`/`sync*`/`yield*`.
    Star,
    /// The `catch` keyword.
    CatchKeyword,
    /// A `,` token (catch parameter comma, assert comma).
    Comma,
    /// A `:` token (map entry/field colon, conditional colon, case colon).
    Colon,
    /// The `default` keyword.
    DefaultKeyword,
    /// The `:` after a `default` case label.
    ColonAfterDefault,
    /// The `?` token (nullable type/index, conditional condition).
    Question,
    /// The `.` token of a qualified name / object-pattern name part.
    Period,
    /// The `.`-before-name token (annotation, constructor reference).
    PeriodBeforeName,
    /// The `=>` of an arrow body or switch-expression case.
    Arrow,
    /// The `async` keyword of a function body modifier.
    AsyncKeyword,
    /// The `<` that opens applied type arguments.
    OpenAngle,
    /// The `...` of a rest pattern.
    Dots,
    /// The `void` of `#void`.
    VoidKeyword,
    /// The `Function` keyword of a function type.
    FunctionKeyword,
    /// The `extends`/`super` token of a type-variable bound.
    ExtendsOrSuper,
    /// The `variance` modifier token of a type variable.
    Variance,
    // —— name token words ———————————————————————————————————————————————
    /// A declared/referenced name token (formal, extension, enum element,
    /// operator name, field initializer, variable/wildcard pattern).
    Name,
    /// The exclusive end of an enum's element list (the token after the
    /// last element).
    ElementsEnd,
    /// The first identifier of an object pattern's (possibly qualified)
    /// type name.
    FirstName,
    /// The identifier after the `.` of an object pattern's qualified name.
    SecondName,
    /// The wildcard token of a wildcard pattern (`_`/`_x`).
    Wildcard,
    /// The variable token of a declared-variable pattern.
    Variable,
    /// The operator token (`+`, `=`, `as`, `.` access, …).
    Operator,
    // —— discriminant words (a parser enum's `as u32`) ————————————————————
    /// The `IdentifierContext` stamp.
    Context,
    /// The `DeclarationKind` of a member/primary constructor.
    DeclarationKind,
    /// The `MemberKind` of a fields row.
    MemberKind,
    /// The `FormalParameterKind` of a formal.
    FormalParameterKind,
    /// The `AssertKind` of an assert.
    AssertKind,
    /// The recovery header's kind discriminant (class/extension type/mixin).
    HeaderKind,
    /// The `ExperimentalFlag` of an experiment-not-enabled marker.
    ExperimentalFlag,
    /// The `ConstructorReferenceContext` stamp.
    ConstructorContext,
    // —— flag words (0/1) ———————————————————————————————————————————————
    /// Whether the construct carries a name (`library`, `part of`).
    HasName,
    /// Whether `break`/`continue` carries a label target.
    HasTarget,
    /// Whether the element is a set entry (`{1, 2}` vs a map literal).
    HasSetEntry,
    /// Whether the operator name is the invalid-operator recovery shape.
    IsInvalid,
    /// Whether the primary constructor has a written constructor name.
    HasConstructorName,
    /// Whether a function name belongs to a function expression.
    IsFunctionExpression,
    /// Whether the skipped body is an expression body.
    IsExpressionBody,
    /// Whether the operator is null-aware (`?.`, `?..`).
    IsNullAware,
    /// Whether the `is` carried `not`.
    IsNegated,
    /// Whether the collection control-flow element is the for-in form.
    IsForIn,
    /// Whether the rest pattern has a sub-pattern.
    HasSubPattern,
    /// Whether the map entry key is null-aware (`?`).
    NullAwareKey,
    /// Whether the map entry value is null-aware (`?`).
    NullAwareValue,
    // —— count / extent words ————————————————————————————————————————————
    /// The expression count of a for/case header.
    ExpressionCount,
    /// Where the formal parameter list starts (`(`).
    ParamStart,
    /// Where the initializer list starts (`:`).
    InitializersStart,
    /// Where a formal's default value starts (`=`).
    InitializerStart,
    /// A formal's default-value end (exclusive).
    InitializerEnd,
    // —— dead-kind placeholders ——————————————————————————————————————————
    /// Unwritten word of a dead collapse-map row (no event builds the
    /// kind); width-only, never read.
    Reserved0,
    /// See [`PayloadField::Reserved0`].
    Reserved1,
}

/// The payload fields of every kind, row per discriminant, in payload
/// order — the single source of [`payload_len`](crate::kind::payload_len)
/// and the name→index resolver behind the named accessors. The width
/// transcript (`tests::COLLAPSE_MAP_WIDTHS`) pins every row's length.
#[rustfmt::skip]
pub const PAYLOAD_FIELDS: [&[PayloadField]; NodeKind::KIND_COUNT as usize] = {
    use PayloadField as F;
    [
        // Group 3.1: compilation unit and directives.
        /* CompilationUnit */ &[],
        /* TopLevelDeclaration */ &[],
        /* LibraryName */ &[F::HasName],
        /* LibraryAugmentation */ &[F::AugmentKeyword, F::LibraryKeyword],
        /* Part */ &[],
        /* PartOf */ &[F::OfKeyword, F::HasName],
        /* Import */ &[F::AsKeyword, F::DeferredKeyword],
        /* RecoverImport */ &[F::Semicolon],
        /* Export */ &[],
        /* Show */ &[],
        /* Hide */ &[],
        /* ConditionalUri */ &[F::LeftParen, F::EqualSign],
        /* DottedName */ &[],
        /* ScriptTag */ &[],
        // Group 3.2: top-level and class-like declarations.
        /* DeclarationBody */ &[],
        /* ClassDeclaration */ &[F::ClassKeyword, F::ExtendsKeyword, F::WithKeyword, F::ImplementsKeyword],
        /* MixinDeclaration */ &[F::MixinKeyword, F::OnKeyword, F::WithKeyword],
        /* NamedMixinApplication */ &[F::ClassKeyword, F::Equals, F::WithKeyword, F::ImplementsKeyword],
        /* ExtensionDeclaration */ &[F::ExtensionKeyword, F::Name, F::OnKeyword],
        /* ExtensionTypeDeclaration */ &[F::ExtensionKeyword, F::TypeKeyword, F::Name],
        /* EnumDeclaration */ &[F::EnumKeyword, F::LeftBrace, F::ElementsEnd],
        /* EnumBody */ &[],
        /* EnumElement */ &[F::Name, F::AugmentKeyword],
        /* Typedef */ &[F::TypedefKeyword, F::Equals],
        /* PrimaryConstructor */ &[F::DeclarationKind, F::ConstKeyword, F::HasConstructorName],
        /* NoPrimaryConstructor */ &[F::ConstKeyword],
        /* PrimaryConstructorBody */ &[F::ThisKeyword, F::InitializersStart, F::AugmentKeyword],
        /* RecoverDeclarationHeader */ &[F::HeaderKind],
        /* InvalidTopLevelDeclaration */ &[],
        // Group 3.3: members.
        /* TopLevelFields */ &[],
        /* TopLevelMethod */ &[F::GetOrSet],
        /* Fields */ &[F::MemberKind],
        /* Method */ &[F::DeclarationKind, F::GetOrSet, F::ParamStart, F::InitializersStart],
        /* Constructor */ &[F::DeclarationKind, F::NewKeyword, F::ParamStart, F::InitializersStart],
        /* Factory */ &[F::DeclarationKind, F::FactoryKeyword],
        /* RedirectingFactoryBody */ &[],
        /* FormalParameter */ &[F::Receiver, F::Name, F::InitializerStart, F::InitializerEnd, F::FormalParameterKind],
        /* FormalParameters */ &[],
        /* OptionalFormalParameters */ &[],
        /* NoFormalParameters */ &[],
        /* ImplicitFormalParameters */ &[],
        /* FunctionTypedFormalParameter */ &[F::Question],
        /* VariablesDeclaration */ &[F::LateKeyword],
        /* InitializedIdentifier */ &[],
        /* FieldInitializer */ &[F::Name],
        /* VariableInitializer */ &[],
        /* Initializer */ &[],
        /* Initializers */ &[],
        /* MetadataStar */ &[],
        /* Metadata */ &[F::PeriodBeforeName],
        /* InvalidMember */ &[],
        // Group 3.4: statements.
        /* Block */ &[],
        /* InvalidTopLevelBlock */ &[],
        /* LabeledStatement */ &[],
        /* Label */ &[],
        /* IfStatement */ &[F::ElseKeyword],
        /* ParenthesizedCondition */ &[F::CaseKeyword, F::WhenKeyword],
        /* ForStatement */ &[],
        /* ForParts */ &[F::LeftParen, F::LeftSeparator, F::RightSeparator, F::ExpressionCount],
        /* ForIn */ &[],
        /* ForInParts */ &[F::AwaitKeyword, F::LeftParen, F::PatternKeyword, F::InKeyword],
        /* WhileStatement */ &[],
        /* DoWhileStatement */ &[],
        /* SwitchStatement */ &[],
        /* SwitchBlock */ &[],
        /* SwitchCase */ &[F::DefaultKeyword, F::ColonAfterDefault, F::ExpressionCount],
        /* SwitchCaseWhenClause */ &[],
        /* SwitchExpression */ &[],
        /* SwitchExpressionBlock */ &[],
        /* SwitchExpressionCase */ &[F::WhenKeyword, F::Arrow],
        /* CaseExpression */ &[F::WhenKeyword, F::Colon],
        /* CatchClause */ &[F::CatchKeyword, F::Comma],
        /* TryStatement */ &[F::FinallyKeyword],
        /* ReturnStatement */ &[],
        /* RethrowStatement */ &[],
        /* YieldStatement */ &[F::Star],
        /* InvalidYieldStatement */ &[F::Star],
        /* Assert */ &[F::AssertKind, F::LeftParen, F::Comma],
        /* Break */ &[F::HasTarget],
        /* Continue */ &[F::HasTarget],
        /* EmptyStatement */ &[],
        /* ExpressionStatement */ &[],
        /* PatternVarDeclStatement */ &[F::Equals],
        /* PatternAssignment */ &[F::Equals],
        /* InvalidStatement */ &[],
        // Group 3.5: expressions.
        /* Arguments */ &[],
        /* NamedArgument */ &[F::Colon],
        /* Send */ &[],
        /* NoArguments */ &[],
        /* TypeApplication */ &[F::OpenAngle],
        /* Indexed */ &[F::Question],
        /* BinaryExpression */ &[F::Operator],
        /* Assignment */ &[F::Operator],
        /* UnaryPrefix */ &[],
        /* UnaryPostfix */ &[],
        /* NonNullAssert */ &[],
        /* Conditional */ &[F::Question, F::Colon],
        /* ConstExpression */ &[],
        /* ConstLiteral */ &[],
        /* NewExpression */ &[],
        /* NewAsIdentifier */ &[],
        /* AwaitExpression */ &[],
        /* InvalidAwaitExpression */ &[],
        /* Cascade */ &[],
        /* CascadeAccess */ &[F::Operator, F::IsNullAware],
        /* DotAccess */ &[F::Operator, F::IsNullAware],
        /* DotShorthandHead */ &[],
        /* ConstDotShorthand */ &[],
        /* ThrowExpression */ &[],
        /* ParenthesizedExpression */ &[],
        /* RecordLiteral */ &[F::ConstKeyword],
        /* StringLiteral */ &[],
        /* LiteralBool */ &[],
        /* LiteralInt */ &[],
        /* LiteralDouble */ &[],
        /* LiteralNull */ &[],
        /* ListLiteral */ &[F::ConstKeyword],
        /* SetOrMapLiteral */ &[F::ConstKeyword, F::HasSetEntry],
        /* MapEntry */ &[F::Colon, F::NullAwareKey, F::NullAwareValue],
        /* SymbolLiteral */ &[],
        /* SymbolVoid */ &[F::VoidKeyword],
        /* AsExpression */ &[F::Operator],
        /* IsExpression */ &[F::IsNegated],
        /* ImplicitCreation */ &[F::OpenAngle],
        /* ConstructorReference */ &[F::PeriodBeforeName, F::ConstructorContext],
        /* NoCtorRefContinuation */ &[],
        /* NoTypeNameInCtorRef */ &[],
        /* NamedFunctionExpression */ &[],
        /* LocalFunctionDeclaration */ &[],
        /* BlockBody */ &[F::AsyncKeyword, F::Star],
        /* FunctionName */ &[F::IsFunctionExpression],
        /* FunctionExpression */ &[],
        /* AnonymousMethodInvocation */ &[F::Reserved0, F::Reserved1],
        /* Identifier */ &[F::Context],
        /* NoIdentifier */ &[],
        /* NoName */ &[],
        /* Qualified */ &[F::Period],
        /* SuperExpression */ &[],
        /* ThisExpression */ &[],
        /* Operator */ &[],
        /* OperatorName */ &[F::Name, F::IsInvalid],
        /* InvalidExpression */ &[],
        /* ExtraneousExpression */ &[],
        /* Spread */ &[],
        /* NullAwareElement */ &[],
        // Group 3.6: control-flow sugar in collections.
        /* ForControlFlow */ &[F::AwaitKeyword, F::IsForIn],
        /* IfControlFlow */ &[],
        /* ThenControlFlow */ &[],
        /* ElseControlFlow */ &[],
        // Group 3.7: patterns.
        /* PatternGuard */ &[],
        /* BinaryPattern */ &[F::Operator],
        /* ConstantPattern */ &[],
        /* CastPattern */ &[F::Operator],
        /* RecordPattern */ &[],
        /* ParenthesizedPattern */ &[],
        /* ObjectPattern */ &[F::FirstName, F::Period, F::SecondName],
        /* MapPattern */ &[],
        /* MapPatternEntry */ &[F::Colon],
        /* ListPattern */ &[],
        /* RelationalPattern */ &[F::Operator],
        /* AssignedVariablePattern */ &[],
        /* DeclaredVariablePattern */ &[F::Variable],
        /* WildcardPattern */ &[F::Wildcard],
        /* PatternField */ &[F::Colon],
        /* NamedRecordField */ &[F::Colon],
        /* RestPattern */ &[F::Dots, F::HasSubPattern],
        /* NullAssertPattern */ &[],
        /* NullCheckPattern */ &[],
        // Group 3.8: types.
        /* Type */ &[],
        /* NoType */ &[],
        /* VoidType */ &[],
        /* FunctionType */ &[F::FunctionKeyword, F::Question],
        /* RecordType */ &[F::LeftParen, F::Question],
        /* RecordTypeEntry */ &[],
        /* RecordTypeNamedFields */ &[],
        /* TypeArguments */ &[],
        /* InvalidTypeArguments */ &[],
        /* NoTypeArguments */ &[],
        /* TypeVariable */ &[F::ExtendsOrSuper, F::Variance],
        /* TypeVariables */ &[],
        /* NoTypeVariables */ &[],
        /* NativeClause */ &[],
        // Group 3.9: function bodies and error sentinels.
        /* EmptyBody */ &[],
        /* ExpressionBody */ &[F::Arrow, F::Semicolon, F::AsyncKeyword, F::Star],
        /* NoBody */ &[],
        /* SkippedBody */ &[F::IsExpressionBody],
        /* NativeFunctionBody */ &[],
        /* InvalidFunctionBody */ &[],
        /* InvalidTypeReference */ &[],
        /* RecoverableError */ &[],
        /* ExperimentNotEnabled */ &[F::ExperimentalFlag],
        /* ErrorToken */ &[],
        /* UnescapeError */ &[F::Reserved0, F::Reserved1],
    ]
};

/// The payload word index of `field` under `kind` — `None` when the kind's
/// row has no such field. `const` so every static (kind, field) call site
/// folds to its index at compile time; the dynamic-kind face scans the
/// kind's own row, whose length is bounded by the widest kind (5).
#[must_use]
pub const fn field_index(kind: NodeKind, field: PayloadField) -> Option<u8> {
    let row = PAYLOAD_FIELDS[kind as usize];
    // Discriminant compare: the derived `PartialEq` is not const-callable,
    // and both sides are fieldless `repr(u8)` enums.
    let target = field as u8;
    let mut index = 0;
    while index < row.len() {
        if row[index] as u8 == target {
            return Some(index as u8);
        }
        index += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kind::{NodeKind, payload_len};

    /// Every kind's row: names unique within the row, positions agreeing
    /// with [`field_index`], and the width agreeing with `payload_len`
    /// (itself pinned against the ratified width transcript by
    /// `tests::payload_width_lockstep`, so this loop closes the chain
    /// table → lengths → transcript).
    #[test]
    fn rows_are_unique_indexed_and_width_faithful() {
        for raw in 0..NodeKind::KIND_COUNT {
            let kind = NodeKind::from_raw(raw);
            let row = PAYLOAD_FIELDS[raw as usize];
            assert_eq!(
                row.len(),
                usize::from(payload_len(kind)),
                "{kind:?}: row length must equal payload_len"
            );
            for (position, &field) in row.iter().enumerate() {
                assert_eq!(
                    field_index(kind, field),
                    Some(position as u8),
                    "{kind:?}: {field:?} must sit at its row position"
                );
                assert!(
                    row.iter().filter(|&&f| f == field).count() == 1,
                    "{kind:?}: repeated field {field:?} in one row"
                );
            }
        }
    }

    /// The table is positional (row per discriminant), so a variant
    /// inserted mid-enum would shift rows without any width error. These
    /// anchor rows — one per domain group — pin the alignment: a shifted
    /// table fails here even where adjacent widths coincide.
    #[test]
    fn anchor_rows_pin_the_alignment() {
        use PayloadField as F;
        let anchors: &[(NodeKind, &[PayloadField])] = &[
            (NodeKind::LibraryName, &[F::HasName]),
            (
                NodeKind::ClassDeclaration,
                &[
                    F::ClassKeyword,
                    F::ExtendsKeyword,
                    F::WithKeyword,
                    F::ImplementsKeyword,
                ],
            ),
            (
                NodeKind::FormalParameter,
                &[
                    F::Receiver,
                    F::Name,
                    F::InitializerStart,
                    F::InitializerEnd,
                    F::FormalParameterKind,
                ],
            ),
            (NodeKind::IfStatement, &[F::ElseKeyword]),
            (
                NodeKind::ExpressionBody,
                &[F::Arrow, F::Semicolon, F::AsyncKeyword, F::Star],
            ),
            (
                NodeKind::ObjectPattern,
                &[F::FirstName, F::Period, F::SecondName],
            ),
            (NodeKind::TypeVariable, &[F::ExtendsOrSuper, F::Variance]),
            (NodeKind::ExperimentNotEnabled, &[F::ExperimentalFlag]),
        ];
        for &(kind, row) in anchors {
            assert_eq!(
                PAYLOAD_FIELDS[kind as usize],
                row,
                "{kind:?}: row must match its anchor"
            );
        }
    }

    /// A field of another kind's row never resolves — the accessor's
    /// `None` contract (spot pairs across the vocabulary's value classes).
    #[test]
    fn foreign_fields_do_not_resolve() {
        assert_eq!(field_index(NodeKind::Import, PayloadField::Name), None);
        assert_eq!(field_index(NodeKind::Method, PayloadField::Star), None);
        assert_eq!(
            field_index(NodeKind::Identifier, PayloadField::Operator),
            None
        );
        assert_eq!(
            field_index(NodeKind::FormalParameter, PayloadField::Arrow),
            None
        );
        assert_eq!(
            field_index(NodeKind::ExpressionBody, PayloadField::Question),
            None
        );
    }

    /// The named write/read round trip over a hand-built node: the builder
    /// resolves the name against the open kind, the table resolves the
    /// read against the sealed kind, and both agree on the word.
    #[test]
    fn named_write_and_read_agree() {
        use crate::AstBuilder;
        let mut b = AstBuilder::new();
        b.begin(NodeKind::FormalParameter, 3);
        b.patch_field(PayloadField::Receiver, crate::TOKEN_NONE);
        b.patch_field(PayloadField::Name, 7);
        b.patch_field(PayloadField::InitializerStart, crate::TOKEN_NONE);
        b.patch_field(PayloadField::InitializerEnd, crate::TOKEN_NONE);
        b.patch_field(PayloadField::FormalParameterKind, 2);
        b.end(NodeKind::FormalParameter, 8);
        let ast = b.finish();
        let id = u32::try_from(ast.len()).expect("small") - 1;
        assert_eq!(ast.field(id, PayloadField::Name), Some(7));
        assert_eq!(ast.field(id, PayloadField::FormalParameterKind), Some(2));
        assert_eq!(
            ast.field(id, PayloadField::Receiver),
            Some(crate::TOKEN_NONE)
        );
    }
}
