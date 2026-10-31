//! Recovery primitives.
//!
//! Official policy: every syntax error is recoverable — report through the
//! listener, insert a synthetic to keep the event stream well-formed, and
//! continue (`parser_impl.dart` class docs :248-273). The primitives here are
//! the Rust equivalents of the official `ensure*`/`rewriteAndRecover`/
//! `moveSynthetic`/skip family, adjusted for an immutable token table:
//!
//! * no `insertSyntheticIdentifier` into the stream — synthetics are queued
//!   in the cursor and referenced by id;
//! * `move_synthetic_close` — when the scanner already synthesized a closing
//!   token for an unclosed group (recording the unmatched pair on the token
//!   table instead of reporting), adopting it REPORTS the row at the
//!   relocation point (the official translation reads
//!   `begin.endGroup.charOffset` post-parse) and the parse-end
//!   [`Self::report_unmatched_groups`] pass reports the never-adopted pairs
//!   at their parked positions;
//! * skip-to-sync — group-aware jump to a boundary set, with a no-progress
//!   guard at every loop level (official strategy C, `parser_impl.dart:445`).

use dartq_scanner::{Keyword, TokenKind};

use crate::listener::IdentifierContext;
use crate::members::is_keyword_or_identifier;
use crate::message::msg;
use crate::parser::{AsyncModifier, Parser};
use crate::token_id::TokenSpan;

/// Backward-scan budget of [`Parser::locate_spill_mark`]: the stranded run
/// of a primary bracket error begins at the mismatching token, so the
/// distance from a spill head back to the parked synthetic is the run plus
/// at most the statement parse that followed it — well under this bound;
/// heads beyond it keep the official lone-junk stall shape.
const SPILL_SCAN_LIMIT: usize = 64;

impl<L: crate::listener::ParserListener> Parser<'_, '_, '_, L> {
    /// Reports a recoverable error pinned to `at` (official
    /// `reportRecoverableError`, `parser_impl.dart:11399`).
    ///
    /// Identical `(offset, length, code)` rows collapse to one
    /// ([`Self::admit_row`]). Counted against the per-file diagnostics cap:
    /// suppressed once the bailout latched; a deduped duplicate burns no
    /// capacity. The official zero-width-synthetic anchor walk
    /// (`findNonZeroLengthToken`, parser/util.dart:56) is applied at the call
    /// sites that can anchor on a parser-inserted synthetic (see
    /// `parse_constructor_invocation_arguments`); every other site anchors on
    /// a real token by construction.
    pub(crate) fn report(&mut self, message: crate::message::Message, at: TokenSpan) {
        if self.admit_row(at.range.offset, at.range.length, &message) && self.admit_diagnostic() {
            self.listener.handle_recoverable_error(message, at, at);
        }
    }

    /// Whether this row is the first of its `(offset, length, code, text)`
    /// quadruple. The analyzer dedupes same-offset same-code errors with the
    /// same rendered text, so a recovery cascade re-reporting a row at an
    /// already-reported anchor surfaces once (oracle `const_patterns` 46:24
    /// `expected_token ';'` and `const_patterns_binary` 85:12: two recovery
    /// points, one row; the differently-coded `missing_identifier` at 85:12
    /// survives). The text is part of the key: at the fasta layer the deduped
    /// reports are distinct diagnostics (`expectedButGot` vs `expectedToken`)
    /// sharing one registry code, and the oracle keeps both when their texts
    /// differ (76dd104c 12:23 surfaces `Expected to find ','.` AND
    /// `Expected to find ';'.` over one span).
    fn admit_row(&mut self, offset: u32, length: u32, message: &crate::message::Message) -> bool {
        // Post-bail rows are suppressed by `admit_diagnostic` anyway (which
        // is also why the `&&` operand order in [`Self::report`] must not be
        // swapped: an early `admit_diagnostic` would burn cap capacity — and
        // could trip the fuse — on rows the dedup would have dropped).
        // Skipping the dedup work here is behavior-identical: `bailed`
        // short-circuits `admit_diagnostic` with no side effects.
        if self.is_bailed() {
            return false;
        }
        // Hash pre-filter keyed on `(offset, length, code)`: a triple never
        // seen is definitionally a new row (`insert` reports fresh keys as
        // `true`), so the common case is one O(1) hash insert and the exact
        // four-field scan (O(k) expected per report versus the previous
        // O(k²) per-cascade linear scan) only runs on triple collisions —
        // distinct-text rows over one span are rare (76dd104c 12:23 is the
        // pinned face).
        if !self.row_keys.insert((offset, length, message.code.name))
            && self.reported_rows.iter().any(|(o, l, c, t)| {
                *o == offset && *l == length && *c == message.code.name && *t == message.text
            })
        {
            return false;
        }
        self.reported_rows
            .push((offset, length, message.code.name, message.text.clone()));
        true
    }

    /// Reports with an explicit span end (official
    /// `reportRecoverableErrorWithEnd`); same cap accounting as [`Self::report`].
    pub(crate) fn report_with_end(
        &mut self,
        message: crate::message::Message,
        start: TokenSpan,
        end: TokenSpan,
    ) {
        if self.admit_diagnostic() {
            self.listener.handle_recoverable_error(message, start, end);
        }
    }

    /// Consumes a required token or reports `Expected to find '<lexeme>'.`
    /// and synthesizes it (official `ensure*` family). Returns the consumed
    /// or synthetic id.
    pub(crate) fn expect_kind(&mut self, kind: TokenKind) -> TokenSpan {
        if let Some(id) = self.cursor.opt_kind(kind) {
            return self.span_of(id);
        }
        let expected = kind.fixed_lexeme().unwrap_or_default();
        let at = self.peek_span();
        self.report(msg::expected_token(expected), at);
        let id = self.cursor.insert_synthetic(kind, at.range.offset);
        self.cursor.advance();
        self.span_of(id)
    }

    /// `expectedButGot` flavor; the analyzer converter re-renders every
    /// `PseudoSharedCode.expectedToken` message with the registry template,
    /// so the rendered text matches [`Self::expect_kind`]'s row (see
    /// `msg::expected_but_got`).
    pub(crate) fn expect_lexeme_but_got(&mut self, expected: &str) -> TokenSpan {
        let at = self.peek_span();
        self.expect_lexeme_but_got_anchored(expected, at)
    }

    /// [`Self::expect_lexeme_but_got`] with an explicit report anchor: the
    /// synthetic still splices at the current token, but the row anchors on
    /// `anchor` — the #2024 separator recovery anchor (the token the
    /// recovery lands behind, upstream Dart SDK source,
    /// pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart
    /// `rewriteAndRecover` reports at the resumed token).
    pub(crate) fn expect_lexeme_but_got_anchored(
        &mut self,
        expected: &str,
        anchor: TokenSpan,
    ) -> TokenSpan {
        if let Some(id) = self.cursor.opt_lexeme(expected) {
            return self.span_of(id);
        }
        let at = self.peek_span();
        self.report(msg::expected_but_got(expected), anchor);
        let kind = lexeme_kind(expected);
        let id = self.cursor.insert_synthetic(kind, at.range.offset);
        self.cursor.advance();
        self.span_of(id)
    }

    /// Whether a reserved word at the cursor taking `context`'s name
    /// position takes the official insert-synthetic face instead of the
    /// keyword-tolerant consume. Per-context recovery templates
    /// (`identifier_context_impl.dart`): `LibraryName` branch 2b (:977-991)
    /// and `TopLevelVariableDeclaration` branch 1 (:1321-1332) insert when
    /// the keyword heads a top-level declaration and the follower does not
    /// continue the name; `Combinator` (:133-165) inserts when the keyword
    /// heads a top-level declaration and the follower is not one of the
    /// context's values (`;` `,` `if` `as` `show` `hide` eof);
    /// `LocalVariableDeclaration` (:1011-1050) inserts on a statement
    /// start — a reserved non-statement word (`var class = 1;`) keeps the
    /// tolerant consume face, exactly like upstream.
    ///
    /// `MetadataReference` and its continuation flavors (:1055-1120): the
    /// keyword is never eaten as the annotation name when it heads a
    /// statement, class member, or top-level declaration — the synthetic
    /// name splices in front of it so the annotation carries an empty name
    /// and the keyword stays for the declaration/statement parse (oracle
    /// partial_code/annotation `@ assert (true);` / `@ var x;`:
    /// `missing_identifier` at the keyword, `undefined_annotation` on the
    /// empty name). `ImportPrefixDeclaration` branch 3 (:706-716): a
    /// declaration head after `as` splices a synthetic prefix (`import
    /// 'a.dart' as class A {}` — `;` anchored at `as`, the class parses at
    /// top level).
    fn reserved_name_recovery_at(&self, context: crate::listener::IdentifierContext) -> bool {
        match context {
            // `LibraryIdentifierContext.ensureIdentifier` recovery branch 2b
            // (identifier_context_impl.dart:976-983) is shared by the
            // library and `part of` name positions AND their `.`
            // continuations (the upstream impl class backs all four
            // `IdentifierContext` constants): a top-level-declaration head
            // after `library`/`part of`/`lib.` splices a synthetic name and
            // stays for the declaration loop — `part of lib. final a = 0;`
            // reports `missing_identifier` at `final`, and `final a = 0;`
            // reparses as a variable declaration. The keep-set is the same
            // `.`/`;`/EOF for every flavor (:947-951).
            crate::listener::IdentifierContext::LibraryName
            | crate::listener::IdentifierContext::LibraryNameContinuation
            | crate::listener::IdentifierContext::PartName
            | crate::listener::IdentifierContext::PartNameContinuation
            | crate::listener::IdentifierContext::TopLevelVariableDeclaration => {
                self.looks_like_start_of_next_top_level_declaration_at(0)
                    && !self.name_continuation_follower(context)
            }
            crate::listener::IdentifierContext::Combinator => {
                self.looks_like_start_of_next_top_level_declaration_at(0)
                    && !matches!(
                        self.cursor.look_kind(1),
                        TokenKind::Semicolon
                            | TokenKind::Comma
                            | TokenKind::Keyword(
                                Keyword::If | Keyword::As | Keyword::Show | Keyword::Hide
                            )
                            | TokenKind::Eof
                    )
            }
            crate::listener::IdentifierContext::LocalVariableDeclaration => {
                self.looks_like_statement_start_at(0)
            }
            // `EnumValueDeclarationIdentifierContext.ensureIdentifier`
            // (identifier_context_impl.dart:346-357): the value-name slot's
            // first recovery arm inserts a synthetic name when the head is a
            // top-level-declaration keyword — `void foo() {}` inside an
            // enum body without a `;` reparses `void` as the member head,
            // so the oracle reports `missing_identifier` at `void` (never
            // `expectedIdentifierButGotKeyword`) and the `}`/`;` recovery
            // lands on the same anchor.
            crate::listener::IdentifierContext::EnumValueDeclaration => {
                self.looks_like_start_of_next_top_level_declaration_at(0)
            }
            // `ClassOrMixinOrExtensionIdentifierContext.ensureIdentifier`
            // (identifier_context_impl.dart:80-86): a top-level-declaration
            // keyword head in the class/mixin/extension name slot splices a
            // synthetic name and stays for the declaration loop when the
            // FOLLOWER does not continue the declaration header —
            // `class enum E { v }` reports `missing_identifier` at `enum`
            // (never `builtInIdentifierInDeclaration`), leaving `enum E { v }`
            // to reparse as an enum declaration; `class enum { }` /
            // `class enum <T> { }` keep the keyword as the name because `{`/`<`
            // are in the `_isOneOfFollowingValues` keep-set (`<` `{`
            // `extends` `with` `implements` `on` `=` `(` `.` EOF, :58-69).
            crate::listener::IdentifierContext::ClassOrMixinOrExtensionDeclaration => {
                self.looks_like_start_of_next_top_level_declaration_at(0)
                    && !matches!(
                        self.cursor.look_kind(1),
                        TokenKind::Lt
                            | TokenKind::OpenCurly
                            | TokenKind::Keyword(
                                Keyword::Extends
                                    | Keyword::With
                                    | Keyword::Implements
                                    | Keyword::On
                            )
                            | TokenKind::Eq
                            | TokenKind::OpenParen
                            | TokenKind::Period
                            | TokenKind::Eof
                    )
            }
            crate::listener::IdentifierContext::MetadataReference
            | crate::listener::IdentifierContext::MetadataContinuation
            | crate::listener::IdentifierContext::MetadataContinuationAfterTypeArguments => {
                self.looks_like_statement_start_at(0)
                    || self.looks_like_start_of_next_top_level_declaration_at(0)
            }
            crate::listener::IdentifierContext::ImportPrefixDeclaration => {
                self.looks_like_start_of_next_top_level_declaration_at(0)
                    && !matches!(
                        self.cursor.look_kind(1),
                        TokenKind::Semicolon
                            | TokenKind::Eof
                            | TokenKind::Keyword(
                                Keyword::If
                                    | Keyword::Show
                                    | Keyword::Hide
                                    | Keyword::Deferred
                                    | Keyword::As
                            )
                    )
            }
            // `TypedefDeclarationIdentifierContext.ensureIdentifier`
            // (identifier_context_impl.dart:1417-1449): a declaration-head
            // keyword in the name slot splices a synthetic name and stays for
            // the declaration loop, but a BUILT-IN head is kept as the name
            // first — `isBuiltIn && _isOneOfFollowingValues(identifier.next)`
            // (:1419-1424) consumes `mixin`/`get`/`set`/`export`/`import`/
            // `library`/`part`/`typedef`/`extension` after the
            // `builtInIdentifierInDeclaration` row when the follower is `(`
            // `<` `=` `;` or EOF (oracle `typedef int get;` / `typedef int
            // mixin;`: builtIn row only, no splice). RESERVED heads (`class`/
            // `enum`/`void`/`var`/`const`/`final`) fail the `isBuiltIn` gate
            // and always splice — oracle `typedef int enum;` splices even
            // with the `;` follower. The splice arm itself is
            // `looksLikeStartOfNextTopLevelDeclaration(identifier) ||
            // _isOneOfFollowingValues(identifier)` (:1425-1426); the second
            // disjunct is dead for keywords (a keyword is never `(`/`<`/`=`/
            // `;`/EOF), and the `(`/`=` keeps that survive the built-in gate
            // reach here via `ensureIdentifierPotentiallyRecovered` anyway.
            crate::listener::IdentifierContext::TypedefDeclaration => {
                self.looks_like_start_of_next_top_level_declaration_at(0)
                    && !(matches!(
                        self.cursor.peek_kind(),
                        TokenKind::Keyword(kw)
                            if kw.style() == dartq_scanner::KeywordStyle::BuiltIn
                    ) && matches!(
                        self.cursor.look_kind(1),
                        TokenKind::OpenParen
                            | TokenKind::Lt
                            | TokenKind::Eq
                            | TokenKind::Semicolon
                            | TokenKind::Eof
                    ))
            }
            // `TypeVariableDeclarationIdentifierContext.ensureIdentifier`
            // (identifier_context_impl.dart:1589-1603): the keep-set joins
            // the declaration/member/statement starts with the type-variable
            // grammar followers (`<` `>` `>>` `>>>` `;` `}` `extends`
            // `super` `=` `>=` EOF) — an `extends`/`super` head in the name
            // slot splices a synthetic name and stays for the bound parse
            // (oracle `void b< extends int>()`: one `missing_identifier` at
            // `extends`, never `expectedIdentifierButGotKeyword` + a
            // missing-comma `expected_token ','` cascade). A member-start
            // keyword (`get`/`set`/`void`/`final`/`var`/`const`/`late`/
            // `external`/`static`/`covariant`/`abstract`) belongs to the
            // same keep-set upstream (`looksLikeStartOfNextClassMember`,
            // identifier_context_impl.dart:1641-1647).
            crate::listener::IdentifierContext::TypeVariableDeclaration => {
                matches!(
                    self.cursor.peek_kind(),
                    TokenKind::Keyword(
                        Keyword::Extends
                            | Keyword::Super
                            | Keyword::Get
                            | Keyword::Set
                            | Keyword::Void
                            | Keyword::Final
                            | Keyword::Var
                            | Keyword::Const
                            | Keyword::Late
                            | Keyword::External
                            | Keyword::Static
                            | Keyword::Covariant
                            | Keyword::Abstract
                            | Keyword::Required
                    )
                ) || self.looks_like_start_of_next_top_level_declaration_at(0)
                    || self.looks_like_statement_start_at(0)
            }
            _ => false,
        }
    }

    /// Official `ensureIdentifier` (`parser_impl.dart:4076`) dispatched
    /// through the context's recovery template
    /// (`identifier_context_impl.dart`): consumes an identifier-shaped
    /// token; a reserved word in a keyword-tolerant context is reported
    /// (`expectedIdentifierButGotKeyword`) and consumed as the name; any
    /// other junk reports `missing_identifier` and inserts a synthetic
    /// identifier without consuming the junk.
    pub(crate) fn ensure_identifier(
        &mut self,
        context: crate::listener::IdentifierContext,
    ) -> TokenSpan {
        let peek = self.cursor.peek();
        // `_tryRewriteNewToIdentifier` (parser_impl.dart:4101-4111): in
        // continuation contexts `new` after `.` is rewritten to an
        // identifier (the unnamed constructor) with no report
        // (`allowsNewAsIdentifier => isContinuation`,
        // identifier_context_impl.dart:200/391) — the gate is the
        // continuation property itself, so every continuation context
        // takes it. Cascade sections consume their `..new` in
        // `parse_send_continuation` before any identifier entry, so the
        // set here covers the dot-continuation paths that do reach this
        // gate (selector chains, dot shorthands, pattern selectors).
        // `allowsNewAsIdentifier` is `true` for exactly five upstream
        // context classes (:200, :391, :544, :1112, :1141) — the two
        // constructor-reference continuations, `expressionContinuation`,
        // `fieldInitializer` (unconditionally), the two metadata
        // continuations, and `methodDeclarationContinuation` +
        // `primaryConstructorDeclaration` (the primary-constructor head
        // reaches this gate only when the feature flag enabled the face).
        if peek.kind == TokenKind::Keyword(Keyword::New)
            && matches!(
                context,
                crate::listener::IdentifierContext::ConstructorReferenceContinuation
                    | crate::listener::IdentifierContext::ConstructorReferenceContinuationAfterTypeArguments
                    | crate::listener::IdentifierContext::ExpressionContinuation
                    | crate::listener::IdentifierContext::FieldInitializer
                    | crate::listener::IdentifierContext::MetadataContinuation
                    | crate::listener::IdentifierContext::MetadataContinuationAfterTypeArguments
                    | crate::listener::IdentifierContext::MethodDeclarationContinuation
                    | crate::listener::IdentifierContext::PrimaryConstructorDeclaration
            )
        {
            let id = self.cursor.advance();
            let span = self.span_of(id);
            self.listener.handle_identifier(span, context);
            return span;
        }
        // Keyword names go through the context's official recovery template
        // first: class/mixin/extension names reject built-in identifiers
        // (`builtInIdentifierInDeclaration`), keyword-tolerant contexts
        // report `expectedIdentifierButGotKeyword` for reserved words and
        // consume them as the name; whatever the template rejects falls
        // through to the generic synthetic path.
        // `LibraryIdentifierContext.ensureIdentifier` recovery branch 2b
        // (identifier_context_impl.dart:977-991) and
        // `TopLevelDeclarationIdentifierContext.ensureIdentifier` recovery
        // branch 1 (:1321-1332): a reserved word that heads a top-level
        // declaration is never eaten as the library name / variable name —
        // report `missing_identifier` at it, splice the synthetic name, and
        // leave the keyword current for the declaration loop (oracle
        // `library` + `void f()` and `var class VarClass {}`). The head is
        // only a declaration head when the FOLLOWER does not continue the
        // name — the official templates accept the keyword as the name when
        // the next token is one of the context's continuation values
        // (`_isOneOfFollowingValues`: `.` `;` EOF for the library name;
        // `followingValues` `=` `,` `;` for the variable name), so
        // `library enum;` and `int class = 42;` take the
        // `expected_identifier_but_got_keyword` face (oracle
        // issue_48371_prime1 / keyword_named_top_level_fields line 9).
        // Reserved words route every name-inserting context through the
        // recovery template; the import-prefix position additionally takes
        // the insert for BUILT-IN top-level heads before the built-in name
        // face — `ImportPrefixIdentifierContext` branch 3
        // (identifier_context_impl.dart:706-716) precedes branch 2, and the
        // oracle carries `missing_identifier` for `as set` / `as import` /
        // `as mixin M {}` (partial_code/import_directive), while the
        // branch-2 face survives when the follower continues the clause
        // (`as mixin;`).
        let keyword_name_insert = match peek.kind {
            TokenKind::Keyword(kw) => match kw.style() {
                dartq_scanner::KeywordStyle::Reserved => matches!(
                    context,
                    crate::listener::IdentifierContext::LibraryName
                        | crate::listener::IdentifierContext::LibraryNameContinuation
                        | crate::listener::IdentifierContext::PartName
                        | crate::listener::IdentifierContext::PartNameContinuation
                        | crate::listener::IdentifierContext::TopLevelVariableDeclaration
                        | crate::listener::IdentifierContext::Combinator
                        | crate::listener::IdentifierContext::LocalVariableDeclaration
                        | crate::listener::IdentifierContext::MetadataReference
                        | crate::listener::IdentifierContext::MetadataContinuation
                        | crate::listener::IdentifierContext::MetadataContinuationAfterTypeArguments
                        | crate::listener::IdentifierContext::ImportPrefixDeclaration
                        | crate::listener::IdentifierContext::TypedefDeclaration
                        | crate::listener::IdentifierContext::TypeVariableDeclaration
                        | crate::listener::IdentifierContext::EnumValueDeclaration
                        | crate::listener::IdentifierContext::ClassOrMixinOrExtensionDeclaration
                ),
                dartq_scanner::KeywordStyle::BuiltIn => {
                    // The `looksLikeStartOfNextTopLevelDeclaration` recovery
                    // arm makes no reserved/built-in distinction upstream —
                    // `enum`/`mixin`/`typedef`/`extension`/`get`/`set` are
                    // built-in keywords yet still head a top-level
                    // declaration, so a name slot whose recovery template
                    // keep-tests that head set splices the synthetic name for
                    // a built-in head exactly as for a reserved one (`library
                    // mixin M {}` / `typedef T set a(b) {}` / `part of lib.
                    // final a = 0;`: all `missing_identifier` at the head,
                    // never `builtInIdentifierInDeclaration` /
                    // `expectedIdentifierButGotKeyword`). The admit set is
                    // therefore every context whose `reserved_name_recovery_at`
                    // arm keep-tests the top-level-declaration head — NOT the
                    // statement-start contexts (`LocalVariableDeclaration`,
                    // `Metadata*`), where a built-in `set`/`get`/`mixin` is a
                    // legal name.
                    context == crate::listener::IdentifierContext::ImportPrefixDeclaration
                        || context == crate::listener::IdentifierContext::LibraryName
                        || context == crate::listener::IdentifierContext::LibraryNameContinuation
                        || context == crate::listener::IdentifierContext::PartName
                        || context == crate::listener::IdentifierContext::PartNameContinuation
                        || context == crate::listener::IdentifierContext::TopLevelVariableDeclaration
                        || context == crate::listener::IdentifierContext::Combinator
                        || context == crate::listener::IdentifierContext::TypedefDeclaration
                        // `TypeVariableDeclarationIdentifierContext.ensureIdentifier`
                        // (identifier_context_impl.dart:1589-1603) tests the
                        // keep-set BEFORE the built-in face: a built-in that
                        // heads a top-level declaration, class member, or
                        // statement (`typedef`/`get`/`set`/`required`/...)
                        // splices the synthetic name instead of being eaten
                        // (oracle `void b< typedef int>();` and the
                        // immediate-closer shapes `void b<typedef>() {}` /
                        // `class A<set> {}` / `void b< get >() {}`: all
                        // `missing_identifier` at the keyword, never the
                        // `builtInIdentifierInDeclaration` consume). The
                        // keep face is safe for the single-token `< set >`
                        // group shape too: the Simple1 scan gate
                        // (`simple1_closer`) admits only identifier/pseudo
                        // names (type_info.dart:445), so those groups take
                        // the Complex recovery loop and the closer assert in
                        // the Simple1 arm is unreachable for them.
                        || context == crate::listener::IdentifierContext::TypeVariableDeclaration
                        // `EnumValueDeclarationIdentifierContext.ensureIdentifier`
                        // (:346-357) early-returns `identifier.isIdentifier`
                        // BEFORE the recovery arm, so a built-in
                        // top-level-declaration head (`mixin`/`typedef`/`get`/
                        // `set`/`export`/`import`/`library`/`part`/`extension`)
                        // is a legal enum value name with ZERO reports —
                        // `enum E { mixin }` parses clean (token.dart:794:
                        // `isIdentifier => isPseudo || isBuiltIn`). The
                        // recovery arm's `looksLikeStartOfNextTopLevelDeclaration`
                        // disjunct is reachable only for RESERVED heads
                        // (`class`/`const`/`enum`/`final`/`var`/`void`), which
                        // the Reserved arm above already covers — admitting
                        // this context here splices a synthetic at a built-in
                        // head the enum-elements loop never consumes and spins
                        // forever on (#3184).
                        // `ClassOrMixinOrExtensionIdentifierContext`
                        // (:80-86): a built-in top-level-declaration head
                        // (`enum`/`mixin`/`typedef`/`extension`/`import`/
                        // `export`/`library`/`part`) in the declaration-name
                        // slot takes the same synthetic-insert arm as a
                        // reserved head — `class enum E { v }` reports
                        // `missing_identifier` at `enum`, never
                        // `builtInIdentifierInDeclaration`.
                        || context == crate::listener::IdentifierContext::ClassOrMixinOrExtensionDeclaration
                }
                _ => false,
            },
            _ => false,
        };
        if keyword_name_insert && self.reserved_name_recovery_at(context) {
            self.report(msg::missing_identifier(), peek.span);
            let id = self
                .cursor
                .insert_synthetic(TokenKind::Identifier, peek.span.range.offset);
            self.cursor.advance();
            // Official `ensureIdentifier` fires `handleIdentifier` on the
            // recovery template's returned identifier (parser_impl.dart:
            // 4083) — the spliced nameless declaration's event, so the
            // declaration joins the elements under the empty name.
            let span = self.span_of(id);
            self.listener.handle_identifier(span, context);
            return span;
        }
        if matches!(peek.kind, TokenKind::Keyword(_))
            && let Some(message) = keyword_as_name_message(
                context,
                &peek,
                &self.cursor,
                self.features.variance_enabled,
            )
        {
            self.report(message, peek.span);
            // The verifier adds its own compile-time name face on every
            // surviving type-parameter keyword name (`visitTypeParameter` →
            // `_checkForBuiltInIdentifierAsName`, error_verifier.dart:
            // 3365-3377: any keyword token whose style is not pseudo). The
            // built-in branch above already carries that face; the
            // reserved-word branch carries only the tolerant-consume row, so
            // the built-in face rides the same token here (oracle `void b<
            // is int>();`: expectedIdentifierButGotKeyword +
            // builtInIdentifierInDeclaration, both on the `is`).
            if context == crate::listener::IdentifierContext::TypeVariableDeclaration
                && matches!(
                    peek.kind,
                    TokenKind::Keyword(kw) if kw.style() == dartq_scanner::KeywordStyle::Reserved
                )
            {
                self.report(
                    msg::built_in_identifier_as_name(
                        crate::message::msg::BuiltInNameKind::TypeParameter,
                        self.cursor.lexeme(peek.span.id),
                    ),
                    peek.span,
                );
            }
            let id = self.cursor.advance();
            let span = self.span_of(id);
            self.listener.handle_identifier(span, context);
            return span;
        }
        if is_identifier(peek.kind) {
            // ExpressionIdentifierContext.ensureIdentifier
            // (identifier_context_impl.dart:397-406): `await` followed by
            // another identifier does not form a valid expression-name —
            // report `unexpectedToken` on the `await` token itself and take
            // the next token as the name (skipping the `await`).
            if matches!(
                context,
                crate::listener::IdentifierContext::Expression
                    | crate::listener::IdentifierContext::ExpressionContinuation
            ) && peek.kind == TokenKind::Keyword(Keyword::Await)
                && is_identifier(self.cursor.look_kind(1))
            {
                self.report(
                    msg::unexpected_token(self.cursor.lexeme(peek.span.id)),
                    peek.span,
                );
                self.cursor.advance(); // `await`
                let id = self.cursor.advance(); // the name token
                let span = self.span_of(id);
                self.listener.handle_identifier(span, context);
                return span;
            }
            // `CombinatorIdentifierContext.ensureIdentifier` first arm
            // (identifier_context_impl.dart:134-141): an identifier-shaped
            // head that starts the next top-level declaration is NOT the
            // combinator name when its own follower leaves the combinator
            // grammar (`;` `,` `if` `as` `show` `hide` EOF — a kept follower
            // means the head is genuinely the last name, e.g.
            // `import 'a' show export;`). Any other follower means the list
            // ran out mid-declaration — report `missing_identifier` at the
            // head, splice the synthetic, and leave the head for the
            // declaration loop (oracle `export 'a.dart' hide A, export
            // 'a.dart';`: the combinator list ends at `export`, which the
            // top-level loop reparses as the next directive).
            if context == crate::listener::IdentifierContext::Combinator
                && self.looks_like_start_of_next_top_level_declaration_at(0)
                && !matches!(
                    self.cursor.look_kind(1),
                    TokenKind::Semicolon
                        | TokenKind::Comma
                        | TokenKind::Keyword(
                            Keyword::If | Keyword::As | Keyword::Show | Keyword::Hide
                        )
                        | TokenKind::Eof
                )
            {
                self.report(msg::missing_identifier(), peek.span);
                let id = self
                    .cursor
                    .insert_synthetic(TokenKind::Identifier, peek.span.range.offset);
                self.cursor.drain_to(id);
                let span = self.span_of(id);
                self.listener.handle_identifier(span, context);
                return span;
            }
            // Analyzer field/variable name check: a built-in keyword that
            // names a top-level declaration (`mixin`, `extension`, ...) is a
            // legal variable name only while the next token continues the
            // declaration (`=`, `,`, `;`); otherwise it reads as the next
            // declaration's head — report `missing_identifier`, splice the
            // synthetic name, and leave the keyword for the declaration loop
            // (oracle: `augment mixin M {}` reports at `mixin` and reparses
            // the mixin; `int mixin = 3;` stays clean). Non-declaration
            // built-ins (`abstract`, `covariant`, ...) stay accepted as
            // names like any identifier (`augment abstract class C {}`
            // keeps `abstract` as the field name).
            if matches!(
                context,
                crate::listener::IdentifierContext::FieldDeclaration
                    | crate::listener::IdentifierContext::TopLevelVariableDeclaration
            ) && matches!(
                // Only non-reserved (built-in) heads reach here — `class`/
                // `enum` are reserved and never pass `is_identifier`.
                peek.kind,
                TokenKind::Keyword(
                    Keyword::Augment
                        | Keyword::Extension
                        | Keyword::Export
                        | Keyword::Import
                        | Keyword::Library
                        | Keyword::Mixin
                        | Keyword::Part
                        | Keyword::Typedef
                )
            ) && !matches!(
                self.cursor.look_kind(1),
                TokenKind::Eq | TokenKind::Comma | TokenKind::Semicolon
            ) {
                self.report(msg::missing_identifier(), peek.span);
                let id = self
                    .cursor
                    .insert_synthetic(TokenKind::Identifier, peek.span.range.offset);
                self.cursor.advance();
                let span = self.span_of(id);
                // Same unconditional `handleIdentifier` as the tail below
                // (parser_impl.dart:4038): the spliced nameless declaration
                // must carry its slot event — `augment mixin M {}` declares
                // the `<unnamed>` variable upstream, and the miss is the
                // same Face-A element hole the other arms closed.
                self.listener.handle_identifier(span, context);
                return span;
            }
            self.check_async_keyword_as_identifier(&peek, context);
            let id = self.cursor.advance();
            let span = self.span_of(id);
            self.listener.handle_identifier(span, context);
            return span;
        }
        // `TypeReference.ensureIdentifier` (identifier_context_impl.dart:1528):
        // `var` in type-reference position is reported and consumed as the
        // type name (the caller emits its own type event for the span).
        if matches!(
            context,
            crate::listener::IdentifierContext::TypeReference
                | crate::listener::IdentifierContext::PrefixedTypeReference
        ) && peek.kind == TokenKind::Keyword(Keyword::Var)
        {
            self.report(msg::var_as_type_name(), peek.span);
            let id = self.cursor.advance();
            return self.span_of(id);
        }
        if matches!(
            context,
            crate::listener::IdentifierContext::Expression
                | crate::listener::IdentifierContext::ExpressionContinuation
        ) && !is_identifier(peek.kind)
        {
            return self.ensure_expression_identifier(context);
        }
        // Official recovery template, three branches
        // (identifier_context_impl.dart:25-45): follower keep-sets route to
        // insert-WITHOUT-consuming (branch 2 — the offending token belongs
        // to the caller's grammar); other junk takes the consume-in-doubt
        // branch (branch 3, "when in doubt, consume the token", :39-42).
        // Identifier-shaped heads never reach here.
        //
        // The keep set joins the official `_isOneOfFollowingValues` grammar
        // followers (:572-584, formalParameterDeclaration: `:` `=` `,` `(`
        // `)` `[` `]` `{` `}` EOF) with `;`/`>` (the same follower class)
        // and the generic statement-start probe: kept tokens leave the
        // junk current, the synthetic riding in front of it. The consume
        // posture is PER-CONTEXT (#564 r4): only the templates whose
        // official else-branch consumes true junk take it —
        // formalParameterDeclaration (:604-611, oracle `void f(5) {}`: one
        // `missing_identifier` at the `5`, junk consumed) and the
        // library/part name template (:991-1000, oracle `library 5;`: one
        // `missing_identifier` at the `5`); every other context keeps the
        // leave posture so the caller's cascade stands (oracle
        // `class C { new = 42; }`: two rows at the `42`).
        self.report(msg::missing_identifier(), peek.span);
        // The combinator keep-set is the context's own
        // `_isOneOfFollowingValues` (`;` `,` `if` `as` `show` `hide` EOF,
        // identifier_context_impl.dart:123-131) — the generic
        // statement-start probe does NOT apply: `@`/`{`/`}` junk takes the
        // consume-in-doubt else-branch, so `import 'a' show @` absorbs the
        // `@` and the directive tail's `expected_token ';'` anchors on it
        // instead of reparsing the `@` as metadata.
        let leave_for_grammar = if context == crate::listener::IdentifierContext::Combinator {
            matches!(
                peek.kind,
                TokenKind::Semicolon
                    | TokenKind::Comma
                    | TokenKind::Keyword(
                        Keyword::If | Keyword::As | Keyword::Show | Keyword::Hide
                    )
                    | TokenKind::Eof
            )
        } else if context == crate::listener::IdentifierContext::MetadataReference {
            // MetadataReferenceIdentifierContext (:1065-1074): the keep set
            // is the brace/paren groupers, the close square, EOF, the
            // next-declaration probes, and the statement-start probe; other
            // junk (`;` `:` literals) takes the consume-in-doubt branch
            // below, so `@ ;` / `@ 5;` / `@ :` absorb the junk and the
            // member-head no-progress guard reports on the FOLLOWER (oracle
            // `class C { @ ; }`: one missing_identifier at the junk plus
            // `expected_class_member` at the `}`; keeping the junk current
            // minted the member row on the junk itself).
            matches!(
                peek.kind,
                TokenKind::OpenCurly
                    | TokenKind::CloseCurly
                    | TokenKind::OpenParen
                    | TokenKind::CloseParen
                    | TokenKind::CloseSquare
                    | TokenKind::Eof
            ) || self.looks_like_start_of_next_top_level_declaration_at(0)
                || self.looks_like_start_of_next_class_member_at(0)
                || self.looks_like_statement_start_at(0)
        } else if context == crate::listener::IdentifierContext::LocalVariableDeclaration {
            // LocalVariableDeclarationIdentifierContext (:1011-1029): keep
            // `;` `=` `,` `{` `}` EOF, string literals, statement-start
            // heads, and reserved-keyword junk; the scanner-parked SYNTHETIC
            // closers (an unclosed `(` pins a `)` at the stall offset) keep
            // like their real counterparts — they are recovery furniture,
            // not junk. Other junk consumes in doubt, so `var 5 = 1;`
            // absorbs the `5` behind the parse pointer and the initializer
            // reads cleanly (oracle: one missing_identifier at the `5`, no
            // `expected_token ';'` cascade). Reserved keywords keep: the
            // official ensureIdentifier leaves the junk standing, so the
            // following ensureSemicolon anchors on the last REAL token
            // (oracle `await for x in y;` body: `Expected ';'` at the `x`,
            // not at the consumed `in`).
            let (_, peek_synthetic) = self.cursor.resolve(peek.span.id);
            matches!(
                peek.kind,
                TokenKind::Semicolon
                    | TokenKind::Eq
                    | TokenKind::Comma
                    | TokenKind::OpenCurly
                    | TokenKind::CloseCurly
                    | TokenKind::Eof
                    | TokenKind::String
            ) || (peek_synthetic
                && matches!(
                    peek.kind,
                    TokenKind::CloseParen
                        | TokenKind::CloseSquare
                        | TokenKind::OpenParen
                        | TokenKind::OpenSquare
                ))
                || matches!(peek.kind, TokenKind::Keyword(kw) if !is_identifier(TokenKind::Keyword(kw)))
                || self.looks_like_statement_start_at(0)
        } else {
            matches!(
                peek.kind,
                TokenKind::Comma
                    | TokenKind::CloseParen
                    | TokenKind::Colon
                    | TokenKind::Semicolon
                    | TokenKind::Eq
                    | TokenKind::Gt
                    | TokenKind::OpenParen
                    | TokenKind::OpenSquare
                    | TokenKind::CloseSquare
                    | TokenKind::OpenCurly
                    | TokenKind::CloseCurly
                    | TokenKind::Eof
            ) || (context == crate::listener::IdentifierContext::TypedefDeclaration
                && peek.kind == TokenKind::Lt)
                || self.looks_like_statement_start_at(0)
        };
        // Branch 3 for the variable-name contexts
        // (TopLevelDeclarationIdentifierContext:1338-1345 and
        // FieldDeclarationIdentifierContext:493-501) and the typedef-name
        // template (TypedefDeclarationIdentifierContext.ensureIdentifier
        // else-branch, identifier_context_impl.dart:1433-1439): non-keep-set
        // junk is consumed in doubt and the synthetic splices in AFTER it
        // (`rewriter.insertSyntheticIdentifier(identifier)` — the junk lands
        // behind the parse pointer), so a following ensureSemicolon's
        // findPreviousNonZeroLengthToken walk-back anchors on the junk token
        // itself (oracle `void<int> x, ?y;`: the `?` is absorbed by the
        // member and `expected_token ';'` anchors at the `?`; oracle
        // `typedef void 5();` reports the single `missing_identifier` at `5`).
        let id = if !leave_for_grammar
            && matches!(
                context,
                crate::listener::IdentifierContext::TopLevelVariableDeclaration
                    | crate::listener::IdentifierContext::FieldDeclaration
                    | crate::listener::IdentifierContext::TypedefDeclaration
                    | crate::listener::IdentifierContext::Combinator
                    | crate::listener::IdentifierContext::MetadataReference
                    | crate::listener::IdentifierContext::LocalVariableDeclaration
            ) {
            let junk_end = peek.span.range.offset + peek.span.range.length;
            self.cursor.advance();
            let id = self
                .cursor
                .insert_synthetic(TokenKind::Identifier, junk_end);
            // Official `ensureIdentifier` returns the synthetic as the
            // consumed name (`identifier = rewriter.insertSyntheticIdentifier
            // (identifier)`, identifier_context_impl.dart:1436-1438) — the
            // caller's next token is the junk's follower (`typedef void 5();`
            // resumes on `(`). The variable contexts keep the synthetic
            // queued; the typedef template consumes it now. The combinator
            // template consumes the synthetic too — its else-branch
            // (`identifier_context_impl.dart:158-164`) is the same
            // consume-in-doubt posture, so `import 'a' show @` resumes on
            // the post-`@` token for `ensureSemicolon`.
            if matches!(
                context,
                crate::listener::IdentifierContext::TypedefDeclaration
                    | crate::listener::IdentifierContext::Combinator
                    // The metadata and local-variable name slots read their
                    // follower (`.`/`(`-hug check, `=`-initializer check)
                    // straight off the cursor, so the spliced name must be
                    // consumed now — `var 5 = 1;` resumes on the `=`,
                    // `@ ;` on the member tail (upstream resumes from the
                    // RETURNED synthetic, whose next is junk.next).
                    | crate::listener::IdentifierContext::MetadataReference
                    | crate::listener::IdentifierContext::LocalVariableDeclaration
            ) {
                self.cursor.advance();
            }
            id
        } else {
            let id = self
                .cursor
                .insert_synthetic(TokenKind::Identifier, peek.span.range.offset);
            if leave_for_grammar {
                // Branch 2: the official `insertSyntheticIdentifier(token)`
                // (token_stream_rewriter.dart:238) inserts after the anchor and
                // RETURNS the synthetic, so the official parse resumes with the
                // synthetic as the current token and the offending follower as
                // `token.next`. Drain to the queued synthetic (the
                // ensureSemicolon idiom) for the same posture — an undrained
                // queue resurfaces the zero-width identifier as a phantom
                // parse: `void f(int,, int b)` then re-enters the parameter
                // loop's tail recovery and stacks an `expected_token` on the
                // oracle's single missing_identifier.
                self.cursor.drain_to(id);
            } else if matches!(
                context,
                crate::listener::IdentifierContext::FormalParameterDeclaration
                    | crate::listener::IdentifierContext::LibraryName
            ) {
                // Branch 3, aligned: the rewriter splices the synthetic after
                // the junk (insertToken, :84), so the junk lands behind the
                // parse pointer. Drain to the synthetic, then consume the junk
                // for the same stream shape.
                self.cursor.drain_to(id);
                self.cursor.advance();
            } else {
                // Leave posture (the pre-#564 branch-3 shape): the junk stays
                // current; the caller's grammar cascades against it.
                self.cursor.advance();
            }
            id
        };
        let span = self.span_of(id);
        // Official `ensureIdentifier` fires `handleIdentifier` on the
        // recovery template's returned synthetic UNCONDITIONALLY
        // (parser_impl.dart:4031-4040 — the context template reports
        // `missing_identifier` and inserts the synthetic first, then the
        // parser-level wrapper fires the event at :4038 whatever identifier
        // came back; the enum-constant constructor selector's empty-name
        // event rides it, official trace issue_48380_1:
        // `handleIdentifier(, constructorReferenceContinuationAfterTypeArguments)`).
        // Fired for the constructor-reference continuations (their
        // downstream consumers are name-gated) and the name-producing
        // declaration contexts: the nameless declaration's slot event is
        // what the downstream capture reads (dartq-semantic's
        // declaration-name arms), and the order matches upstream — the
        // recovery report lands BEFORE the event, in every posture here
        // (report at the template entry, event after the postures).
        if matches!(
            context,
            crate::listener::IdentifierContext::ConstructorReferenceContinuation
                | crate::listener::IdentifierContext::ConstructorReferenceContinuationAfterTypeArguments
                | crate::listener::IdentifierContext::TypedefDeclaration
                | crate::listener::IdentifierContext::FieldDeclaration
                | crate::listener::IdentifierContext::TopLevelVariableDeclaration
                | crate::listener::IdentifierContext::TopLevelFunctionDeclaration
                | crate::listener::IdentifierContext::MethodDeclaration
                | crate::listener::IdentifierContext::LocalVariableDeclaration
                | crate::listener::IdentifierContext::LocalFunctionDeclaration
                | crate::listener::IdentifierContext::RecordFieldDeclaration
        ) {
            self.listener.handle_identifier(span, context);
        }
        span
    }

    /// Official missing-identifier report span (upstream Dart SDK source,
    /// pkg/_fe_analyzer_shared/lib/src/parser/util.dart
    /// `findNonZeroLengthToken` + the analyzer's `handleRecoverableError`
    /// offset/end formula): a real got token anchors on its own span; a
    /// zero-width got (the scanner's synthetic interpolation closer parked
    /// at the discarded `${`) keeps the got offset and borrows the first
    /// non-zero-length token's length (frozen oracle 76dd104c 11:12: row
    /// (182, length 4) — got at 182, walked-to segment `foo.` length 4).
    /// The message is always the registry text `Expected an identifier.` —
    /// the template carries no lexeme parameter.
    fn report_missing_identifier(&mut self, got: crate::token_id::TokenSpan) {
        if got.range.length > 0 {
            self.report(msg::missing_identifier(), got);
            return;
        }
        let mut n = 0usize;
        let mut length = 0u32;
        while self.cursor.look_kind(n) != TokenKind::Eof {
            let span = self.look_span_at(n);
            if span.range.length > 0 {
                length = span.range.length;
                break;
            }
            n += 1;
        }
        self.report(
            msg::missing_identifier(),
            crate::token_id::TokenSpan::new(
                got.id,
                dartq_source::SourceRange {
                    offset: got.range.offset,
                    length,
                },
            ),
        );
    }

    /// `ExpressionIdentifierContext.ensureIdentifier`
    /// (`identifier_context_impl.dart:393-472`): a keyword in expression
    /// position is consumed as the name with
    /// `expected_identifier_but_got_keyword` (except `as`/`is`/EOF in a
    /// non-continuation context); other junk either makes way for a
    /// synthetic or falls through to `expected_identifier` + synthetic.
    fn ensure_expression_identifier(
        &mut self,
        context: crate::listener::IdentifierContext,
    ) -> TokenSpan {
        let peek = self.cursor.peek();
        let is_continuation = context == crate::listener::IdentifierContext::ExpressionContinuation;
        let junk_kind = peek.kind;
        // `looksLikeStatementStart(identifier)` (identifier_context_impl.dart
        // :411): the candidate token itself is classified — a keyword that
        // starts a statement is left for the statement parser, not eaten as
        // a name. The cursor sits on the candidate.
        if !self.looks_like_statement_start_at(0) {
            if is_keyword_or_identifier(junk_kind) {
                let lexeme = self.cursor.lexeme(peek.span.id);
                if is_continuation
                    || (!matches!(junk_kind, TokenKind::Eof) && lexeme != "as" && lexeme != "is")
                {
                    self.report(msg::expected_identifier_but_got_keyword(lexeme), peek.span);
                    let id = self.cursor.advance();
                    let span = self.span_of(id);
                    self.listener.handle_identifier(span, context);
                    return span;
                }
            } else if !crate::members::is_user_definable_operator_kind(junk_kind)
                && !matches!(
                    junk_kind,
                    TokenKind::Eq
                        | TokenKind::Bang
                        | TokenKind::Comma
                        | TokenKind::Question
                        | TokenKind::Period
                        | TokenKind::QuestionPeriod
                        | TokenKind::OpenParen
                        | TokenKind::CloseParen
                        | TokenKind::OpenSquare
                        | TokenKind::CloseSquare
                        | TokenKind::OpenCurly
                        | TokenKind::CloseCurly
                        | TokenKind::Colon
                        | TokenKind::Semicolon
                        | TokenKind::Eof
                        // The remaining operator-token flags of the official
                        // `TokenType.isOperator` set (upstream Dart SDK
                        // source, pkg/_fe_analyzer_shared scanner token.dart):
                        // these stay in the stream behind the spliced name so
                        // the precedence loop consumes them — swallowing one
                        // splits the statement and re-anchors the enclosing
                        // semicolon recovery on the operator (#2024, frozen
                        // oracle 0c9fda4b L85: `?? 2` parses as ONE statement
                        // with the `;` row on the `2`).
                        | TokenKind::AmpersandAmpersand
                        | TokenKind::BarBar
                        | TokenKind::MinusMinus
                        | TokenKind::PeriodPeriod
                        | TokenKind::PlusPlus
                        | TokenKind::QuestionQuestion
                        | TokenKind::QuestionQuestionEq
                )
            {
                // When in doubt, consume the token to ensure progress; the
                // report anchors on it and the synthetic lands after.
                self.report_missing_identifier(peek.span);
                self.cursor.advance();
                let id = self
                    .cursor
                    .insert_synthetic(TokenKind::Identifier, peek.span.range.offset);
                self.cursor.advance();
                return self.span_of(id);
            }
        }
        self.report_missing_identifier(peek.span);
        let id = self
            .cursor
            .insert_synthetic(TokenKind::Identifier, peek.span.range.offset);
        self.cursor.advance();
        self.span_of(id)
    }

    /// `looksLikeStatementStart` (`identifier_context.dart:402`) evaluated
    /// at cursor-relative offset `n`.
    fn looks_like_statement_start_at(&self, n: usize) -> bool {
        matches!(
            self.cursor.look_kind(n),
            TokenKind::At
                | TokenKind::Keyword(
                    Keyword::Assert
                        | Keyword::Break
                        | Keyword::Continue
                        | Keyword::Do
                        | Keyword::Else
                        | Keyword::Final
                        | Keyword::For
                        | Keyword::If
                        | Keyword::Return
                        | Keyword::Switch
                        | Keyword::Try
                        | Keyword::Var
                        | Keyword::Void
                        | Keyword::While
                )
                | TokenKind::Eof
        )
    }

    /// `looksLikeStartOfNextTopLevelDeclaration`
    /// (`identifier_context_impl.dart:1650)`: the `isTopLevelKeyword` heads
    /// (token.dart: class/enum/export/extension/import/library/mixin/part/
    /// typedef) plus const/get/final/set/var/void.
    fn looks_like_start_of_next_top_level_declaration_at(&self, n: usize) -> bool {
        matches!(
            self.cursor.look_kind(n),
            TokenKind::Keyword(
                Keyword::Class
                    | Keyword::Const
                    | Keyword::Enum
                    | Keyword::Export
                    | Keyword::Extension
                    | Keyword::Final
                    | Keyword::Get
                    | Keyword::Import
                    | Keyword::Library
                    | Keyword::Mixin
                    | Keyword::Part
                    | Keyword::Set
                    | Keyword::Typedef
                    | Keyword::Var
                    | Keyword::Void
            )
        )
    }

    /// `looksLikeStartOfNextClassMember` (identifier_context_impl.dart:1641)
    /// evaluated at cursor-relative offset `n`: a member modifier, an
    /// annotation head, `get`/`set`/`void`, or EOF.
    fn looks_like_start_of_next_class_member_at(&self, n: usize) -> bool {
        matches!(
            self.cursor.look_kind(n),
            TokenKind::At
                | TokenKind::Keyword(Keyword::Get | Keyword::Set | Keyword::Void)
                | TokenKind::Eof
        ) || self.is_modifier_at(n)
    }

    /// The follower keep-set that lets a declaration-head keyword be eaten
    /// as the name: `LibraryIdentifierContext._isOneOfFollowingValues`
    /// (identifier_context_impl.dart:953-957 — `.` `;` EOF) and the
    /// top-level declaration context's `followingValues` (`=` `,` `;` —
    /// oracle `int class = 42;` names the field, `var class VarClass {}`
    /// reparses the class).
    fn name_continuation_follower(&self, context: crate::listener::IdentifierContext) -> bool {
        let follower = self.cursor.look_kind(1);
        match context {
            crate::listener::IdentifierContext::LibraryName
            | crate::listener::IdentifierContext::LibraryNameContinuation
            | crate::listener::IdentifierContext::PartName
            | crate::listener::IdentifierContext::PartNameContinuation => {
                matches!(
                    follower,
                    TokenKind::Period | TokenKind::Semicolon | TokenKind::Eof
                )
            }
            _ => matches!(
                follower,
                TokenKind::Eq | TokenKind::Comma | TokenKind::Semicolon
            ),
        }
    }

    /// Official `checkAsyncAwaitYieldAsIdentifier`
    /// (`identifier_context_impl.dart:1632`): `await`/`yield` consumed as a
    /// plain name inside an async or generator function reports
    /// `async_keyword_used_as_identifier`.
    fn check_async_keyword_as_identifier(
        &mut self,
        view: &crate::cursor::TokenView,
        context: crate::listener::IdentifierContext,
    ) {
        if self.async_state == AsyncModifier::Sync || !context.checks_async_keyword() {
            return;
        }
        let lexeme = self.cursor.lexeme(view.span.id);
        if lexeme == "await" || lexeme == "yield" {
            self.report(msg::async_keyword_used_as_identifier(), view.span);
        }
    }

    /// `IdentifierContext.catchParameter.ensureIdentifier`
    /// (`identifier_context_impl.dart:25`): the catch template reports
    /// `catch_syntax` (not `missing_identifier`) for non-identifier junk,
    /// then routes exactly like the official template — statement starts /
    /// `,` / `)` / EOF keep the token behind a synthetic name (:36-38); other
    /// non-keyword junk does too ("consume in doubt", :39-42 — the rewriter
    /// inserts before the token, keeping it in the stream); a reserved word
    /// itself becomes the name after the report (:44 `return identifier`).
    /// Every route — the synthetic ones included — fires
    /// `handleIdentifier(CatchParameter)`: the rewriter's synthetic
    /// identifier exists "to satisfy listeners", and the slot's event (a
    /// real name or the nameless synthetic) is what the engine's catch
    /// classification reads.
    pub(crate) fn ensure_catch_parameter(&mut self) -> TokenSpan {
        let peek = self.cursor.peek();
        if is_identifier(peek.kind) {
            self.check_async_keyword_as_identifier(
                &peek,
                crate::listener::IdentifierContext::CatchParameter,
            );
            let id = self.cursor.advance();
            let span = self.span_of(id);
            self.listener
                .handle_identifier(span, crate::listener::IdentifierContext::CatchParameter);
            return span;
        }
        // Anchor on the real follower when the head is a scanner-parked
        // synthetic closer — the `(`-group's `endGroup` `)` sits inline at the
        // follower's offset with zero width, while upstream reads the name
        // slot as the real next token (`catch ( }`: `openParens.next` is `}`
        // upstream, not the parked `)`; the `catchSyntax` row is the
        // follower's full span, never the synthetic's zero-width one).
        let report_at = if peek.is_synthetic && peek.span.range.length == 0 {
            self.look_span_at(1)
        } else {
            peek.span
        };
        self.report(msg::catch_syntax(), report_at);
        if !self.looks_like_statement_start_at(0)
            && !matches!(
                peek.kind,
                TokenKind::Comma | TokenKind::CloseParen | TokenKind::Eof
            )
            && is_keyword_or_identifier(peek.kind)
        {
            let id = self.cursor.advance();
            let span = self.span_of(id);
            self.listener
                .handle_identifier(span, crate::listener::IdentifierContext::CatchParameter);
            return span;
        }
        let id = self
            .cursor
            .insert_synthetic(TokenKind::Identifier, peek.span.range.offset);
        self.cursor.advance();
        let span = self.span_of(id);
        self.listener
            .handle_identifier(span, crate::listener::IdentifierContext::CatchParameter);
        span
    }

    /// Official `ensureCloseParen` (`parser_impl.dart:5028`): consumes `)`
    /// and returns its span.
    ///
    /// The row text is the shared `EXPECTED_TOKEN` problem text `Expected
    /// to find ')'.`: the 3.13.1 oracle's machine output for the
    /// parameter-group face (`void f(int 5) {}` probe — one row at the
    /// junk); 3.15's source passes `diag.expectedButGot` (:5040), whose
    /// `pseudoSharedCode: PseudoSharedCode.expectedToken`
    /// (diagnostic.g.dart:646-652) surfaces as the same analyzer code, and
    /// the ensureSemicolon adjudication (#459) fixed the same divergence
    /// for `;`.
    ///
    /// Missing `)` with a scanner-synthesized closer for the group: adopt it
    /// (`moveSynthetic`) and report the unmatched-group row at the
    /// relocation point — the official `moveSynthetic` re-offsets the closer
    /// to the insertion point (`_setOffset(endGroup, next.offset)`), and the
    /// analyzer translation reads `begin.endGroup.charOffset` post-parse, so
    /// the row lands exactly here (oracle `new C(;` → `expected_token` ')'
    /// @ the `;`; `f('${{` → ')' @ the `${`). The begin id latches
    /// [`Self::unmatched_reported`] so the parse-end pass skips the pair.
    /// A well-formed but distant `)` is reported once and jumped to.
    /// Adopting the scanner's synthetic marks the spill run of the primary
    /// bracket error (see [`Self::drain_spill_run`]).
    pub(crate) fn ensure_close_paren(&mut self, open: TokenSpan) -> TokenSpan {
        if let Some(id) = self.cursor.opt_kind(TokenKind::CloseParen) {
            self.mark_spill_run(id);
            return self.span_of(id);
        }
        // Where does the group actually close?
        if let Some(close_id) = self.cursor.table_matching_end(open.id) {
            let (_, synthetic) = self.cursor.resolve(close_id);
            if synthetic {
                // Scanner-synthesized closer parked at the wrong spot (EOF
                // for an unclosed `(`): relocate it (:5035-5037).
                return self.move_synthetic_close(open, close_id, TokenKind::CloseParen, ")");
            }
            let at = self.peek_span();
            self.report(msg::expected_token(")"), at);
            self.skip_to_token(close_id);
            let close = self.peek_span();
            self.cursor.advance();
            return close;
        }
        // No matching closer at all: report and synthesize here.
        let at = self.peek_span();
        self.report(msg::expected_token(")"), at);
        let id = self
            .cursor
            .insert_synthetic(TokenKind::CloseParen, at.range.offset);
        self.cursor.advance();
        self.span_of(id)
    }

    /// Official `moveSynthetic`: relocate the scanner-synthesized closer
    /// `close_id` of `open` in front of the current token and consume it,
    /// reporting the unmatched-group row the post-parse translation emits at
    /// the relocated offset (it reads `begin.endGroup.charOffset`); the
    /// enclosing parse resumes at the current token. The begin id latches
    /// [`Self::unmatched_reported`] so the parse-end pass skips the pair.
    ///
    /// The relocation point latches the spill mark: tokens stranded behind
    /// it (a run of closers/commas the enclosing jumps consume silently)
    /// are the spill run, and the mark arms the ensureSemicolon
    /// relocation-anchor override — a construct that resumes here and
    /// consumes nothing real beyond it anchors past the mark (sii-probes
    /// double_dollar: the `;` on the resumed `$$` head), while a recovery
    /// that mints its own zero-width tail walks back instead
    /// (oracle for_each_statement in_block: the body `;` on the iterable's
    /// `}`, not the resume token).
    ///
    /// `parked` is the scanner-synthesized closer being adopted — the
    /// official rewriter relinks it out of its parked slot, so the flat
    /// table hides the id instead (an adopted `)` parked at EOF would
    /// otherwise re-surface past the enclosing `}` and re-enter the class
    /// member loop, minting a spurious `expected_class_member` on the
    /// 0-width synthetic — the `noType_leftParen` family).
    pub(crate) fn move_synthetic_close(
        &mut self,
        open: TokenSpan,
        parked: crate::token_id::TokenId,
        kind: TokenKind,
        lexeme: &'static str,
    ) -> TokenSpan {
        let at = self.peek_span();
        self.report_unmatched_row(lexeme, at);
        self.unmatched_reported.push(open.id);
        self.cursor.hide(parked);
        self.spill_after = Some(at.range.offset);
        let id = self.cursor.insert_synthetic(kind, at.range.offset);
        self.cursor.advance();
        self.span_of(id)
    }

    /// One unmatched-group row (official `translateErrorToken`'s
    /// `unmatchedToken` arm: `expected_token '<closer>'` at
    /// `begin.endToken.charOffset`, length always 1 —
    /// translate_error_token.dart:89-104 reports
    /// `atOffset(offset: charOffset, length: 1)` (:28), no zero-width shape
    /// exists). At the source end the closer parks at the eof offset
    /// (`appendEofToken` → `unmatchedBeginGroup`, abstract_scanner.dart:529-543,
    /// :901-951) and the eof decrement in `makeError` (:21-27) never fires:
    /// scanner error tokens chain in one run at the stream front
    /// (`prependErrorToken`, abstract_scanner.dart:677-689, seeded by the -1
    /// EOF sentinel), so `_isAtEnd`'s BAD_INPUT-skipping walk (:114-126) hits
    /// the begin token, not EOF — the row is length-1 AT the eof offset
    /// (oracle machine rows: `f('${` → '}' @2:1|1, `void main() {` → '}' at
    /// offset = source length). Identical rows collapse: the oracle dedupes
    /// them downstream (two_unclosed_curly's two `}`-groups parked at EOF
    /// surface as a single row). `at` carries the closer's handle + its
    /// as-scanned span; the encoded range is the translation's law above, not
    /// `at` verbatim.
    pub(crate) fn report_unmatched_row(&mut self, lexeme: &'static str, at: TokenSpan) {
        let eof = self.table.last().utf16_offset;
        let offset = if at.range.offset >= eof {
            eof
        } else {
            at.range.offset
        };
        let key = (lexeme, offset);
        if self.unmatched_seen.iter().any(|&k| k == key) {
            return;
        }
        self.unmatched_seen.push(key);
        self.report(
            msg::expected_token(lexeme),
            TokenSpan::new(at.id, dartq_source::SourceRange { offset, length: 1 }),
        );
    }

    /// The parse-end pass (official `reportAllErrorTokens`,
    /// `parser_impl.dart:11433-11439`): reports every scanner-unmatched
    /// group whose closer was never adopted at a relocation point — the row
    /// sits at the parked closer's position (`begin.endGroup.charOffset`
    /// post-parse; a never-adopted closer keeps its scan-time position: EOF
    /// for `appendEofToken` closers, the mismatching closer for
    /// `insertSyntheticClosers`). Bounded by the pair list; capped through
    /// the shared diagnostic fuse.
    pub(crate) fn report_unmatched_groups(&mut self) {
        let pairs = self.table.unmatched_groups();
        if pairs.is_empty() {
            return;
        }
        for &(begin, close) in pairs {
            if self.unmatched_reported.contains(&begin) {
                continue;
            }
            // Scanner pair indices are stable (original tokens keep their
            // indices through `append_inserted`), so the lookup always
            // succeeds; a miss degrades by skipping the row (under-report
            // only — the parity face is untouched) instead of panicking.
            let closer = match self.table.get(close as usize) {
                Some(t) => t,
                None => {
                    debug_assert!(false, "pair indexes the table");
                    continue;
                }
            };
            let lexeme = closer.kind.fixed_lexeme().unwrap_or("}");
            // The span length is a placeholder — report_unmatched_row encodes
            // the translation's law (length 1; the offset clamps to the eof
            // anchor).
            let at = TokenSpan::new(
                close,
                dartq_source::SourceRange {
                    offset: closer.utf16_offset,
                    length: 0,
                },
            );
            self.report_unmatched_row(lexeme, at);
        }
    }

    /// Skips forward until `id` is the current token (group boundaries in the
    /// table are respected implicitly: ids are absolute). EOF pins the
    /// cursor, so a stale target cannot loop forever.
    pub(crate) fn skip_to_token(&mut self, id: crate::token_id::TokenId) {
        while self.cursor.peek_id() != id && !self.cursor.at_eof() {
            self.cursor.advance();
        }
    }

    /// Marks [`Self::drain_spill_run`]'s region when `id` is the scanner's
    /// synthetic closer of an unmatched begin: the parse reports that primary
    /// bracket error for the pair (at this closer's parked position or its
    /// relocation point), so everything stranded behind it is the spill run
    /// the official enclosing jumps consume silently (`moveSynthetic`
    /// rationale, `parser_impl.dart:5034-5037`). Parser-inserted synthetics
    /// (ids past the table) mark nothing.
    pub(crate) fn mark_spill_run(&mut self, id: crate::token_id::TokenId) {
        if (id as usize) >= self.table.len() {
            return;
        }
        let (_, synthetic) = self.cursor.resolve(id);
        if synthetic {
            self.spill_after = Some(self.span_of(id).range.offset);
        }
    }

    /// Whether the cursor sits in the spill run of a primary bracket error:
    /// a stranded closer or comma strictly behind the scanner's parked
    /// synthetic for an unmatched begin. The mismatching token itself (at
    /// the marked offset) belongs to the enclosing group parses, which
    /// consume it by kind; only what follows is spill. When no mark is
    /// latched yet, [`Self::locate_spill_mark`] scans the consumed prefix.
    pub(crate) fn in_spill_region(&mut self) -> bool {
        if !matches!(
            self.peek_kind(),
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::Comma
        ) {
            return false;
        }
        let head = self.peek_span().range.offset;
        if self.spill_after.is_none_or(|at| head <= at) {
            self.locate_spill_mark(head);
        }
        self.spill_after.is_some_and(|at| head > at)
    }

    /// Backward scan for the scanner's parked synthetic of a primary
    /// bracket error, covering the adoptions that bypass
    /// [`Self::mark_spill_run`] (the argument/list loop heads consume their
    /// closers directly): the consumed table prefix is walked down to
    /// [`Parser::spill_scan_floor`] — the block entry — looking for the
    /// nearest scanner-synthetic closer at or before the head's offset; a
    /// real `{` ends the walk (the block boundary). A find latches
    /// `spill_after`, so later checks are O(1); misses are bounded by
    /// [`SPILL_SCAN_LIMIT`] tokens, keeping repeated stalls linear.
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    fn locate_spill_mark(&mut self, head: u32) {
        #[allow(clippy::cast_possible_truncation)] // ids are u32 by design
        let mut id = self.cursor.real_pos();
        let mut budget = SPILL_SCAN_LIMIT;
        while id > self.spill_scan_floor && budget > 0 {
            id -= 1;
            budget -= 1;
            let tid = id as crate::token_id::TokenId;
            let (kind, synthetic) = self.cursor.resolve(tid);
            let offset = self.span_of(tid).range.offset;
            if offset > head {
                continue; // EOF-parked or later-inserted: no verdict here
            }
            if synthetic
                && matches!(
                    kind,
                    TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly
                )
            {
                self.spill_after = Some(offset);
                return;
            }
            if !synthetic && kind == TokenKind::OpenCurly {
                break; // the enclosing block's `{`: the region ends here
            }
        }
    }

    /// Consumes the spill run of a primary bracket error: the contiguous
    /// stranded closers and commas behind the mismatch. Official consumes
    /// this region through the enclosing group drains — `ensureCloseParen`'s
    /// endGroup jump (:5043-5049), the list drain (:8569-8575), the argument
    /// drain (:9588-9600) — reporting at most the unmatched group's own row,
    /// never one row per token. The run stops at any token an enclosing parse can
    /// still consume (`;`, `}`) or that re-enters parsing (the official
    /// `looksLikeExpressionStart`/`looksLikeLiteralEntry` comma forks,
    /// :9588-9600 / :8563). Structurally advancing: every iteration consumes
    /// one token and EOF pins the cursor.
    pub(crate) fn drain_spill_run(&mut self) {
        while !self.cursor.at_eof() {
            match self.peek_kind() {
                TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::Comma => {
                    self.cursor.advance();
                }
                _ => break,
            }
        }
    }

    /// Skip-to-sync over a boundary set (official `skip*` strategy B). Group
    /// tokens are entered and left via their matched ends so the scan does
    /// not stop inside a nested construct. Terminates: each iteration
    /// advances at least one real token, EOF pins. Public recovery surface
    /// for the declaration/statement/expression milestones (#18/#19/#20).
    pub fn skip_to_sync(&mut self, boundaries: &[TokenKind]) {
        while !self.cursor.at_eof() {
            let kind = self.cursor.peek().kind;
            if boundaries.contains(&kind) {
                return;
            }
            if let Some(end) = self.current_matching_end() {
                self.skip_to_token(end);
                continue;
            }
            self.cursor.advance();
        }
    }

    /// Official `ensureIdentifierPotentiallyRecovered` (recovered-names
    /// flavor, `identifier_context_impl.dart:1491`): for every context this
    /// crate reaches it with, the recovered flavor consumes the keyword with
    /// the same report as the normal template, so it shares
    /// [`Self::ensure_identifier`].
    /// `ensureIdentifierPotentiallyRecovered(token, parser, true)`
    /// (`identifier_context_impl.dart:1452-1475`): on the recovered route a
    /// keyword/identifier head is used as the name under
    /// `expectedIdentifierButGotKeyword` with no declaration-head reparse —
    /// only non-keyword junk falls back to the normal `ensureIdentifier`
    /// template. The typedef callers pass `isIdentifierRecovered` for the
    /// `name (`/`name <T>(` group shapes (parser_impl.dart:1494/:1590).
    pub(crate) fn ensure_identifier_recovered(
        &mut self,
        context: crate::listener::IdentifierContext,
    ) -> TokenSpan {
        if context == crate::listener::IdentifierContext::TypedefDeclaration
            && let TokenKind::Keyword(kw) = self.cursor.peek().kind
        {
            let peek = self.cursor.peek();
            // Official pseudo-keyword fast path (:1459-1466): legal names
            // consumed silently; reserved and built-in heads — `Function`
            // included — take `expectedIdentifierButGotKeyword` (:1473-1474).
            if kw.style() != dartq_scanner::KeywordStyle::Pseudo {
                self.report(
                    msg::expected_identifier_but_got_keyword(
                        self.cursor.lexeme(peek.span.id),
                    ),
                    peek.span,
                );
            }
            let id = self.cursor.advance();
            let span = self.span_of(id);
            self.listener.handle_identifier(span, context);
            return span;
        }
        self.ensure_identifier(context)
    }

    /// Official `ensureSemicolon` (`parser_impl.dart:5093`): consumes `;` or
    /// reports and synthesizes one.
    ///
    /// Three official details live here. The report anchors on
    /// `findPreviousNonZeroLengthToken(token)` (`parser_impl.dart:5106`,
    /// helper at `:11400`) — the last consumed token, walked back over
    /// zero-width synthetics — and the diag spans that token, so
    /// `void f() { x }` reports at the `x`, offset 11 length 1. The
    /// walk-back crosses every zero-width inserted token, including a
    /// relocated scanner closer the recovery consumed — but when a
    /// relocation (an adopted closer or a spill mark) resumed the parse
    /// ahead of the construct's tail and the construct consumed nothing
    /// real beyond it, the frozen oracle anchors past the relocation on the
    /// resume token instead (`findNonZeroLengthToken` over the resume
    /// point, `util.dart`; the #2024 drain pins and sii-probes
    /// double_dollar) — while a recovery that mints its own zero-width tail
    /// after the resume point ends the walk on the construct's real tail
    /// (`f() { for (var a in {} }` anchors the body `;` on the iterable's
    /// `}`: the missing-identifier insert is the walk's starting synthetic).
    /// The message text is the shared `EXPECTED_TOKEN` problem text
    /// `Expected to find ';'.`: the 3.13.1 oracle's machine output for this
    /// face (`dart analyze --format=machine` probe); the 3.15 source passes
    /// `diag.expectedAfterButGot` (`:5107`), which surfaces as the same
    /// analyzer shared code. Finally the inserted `;` drains everything
    /// queued in front of it, so the parse resumes at the token the
    /// official stream carries as `token.next`.
    pub(crate) fn ensure_semicolon(&mut self) -> TokenSpan {
        if let Some(id) = self.cursor.opt_kind(TokenKind::Semicolon) {
            return self.span_of(id);
        }
        let at = self.peek_span();
        let mut anchor = self.last_consumed_anchor();
        // Relocation override: only while the construct consumed nothing
        // beyond the relocation — a parser-inserted tail (id past the real
        // table) means the walk-back legitimately crosses it.
        let minted_tail = self
            .cursor
            .last_consumed_id()
            .is_some_and(|id| (id as usize) >= self.table.len());
        if !minted_tail
            && self
                .spill_after
                .is_some_and(|mark| anchor.range.offset <= mark)
        {
            let mut n = 0usize;
            let mut forward = at;
            while forward.range.length == 0 && self.cursor.look_kind(n) != TokenKind::Eof {
                n += 1;
                forward = self.look_span_at(n);
            }
            anchor = forward;
        }
        self.report(msg::expected_token(";"), anchor);
        let id = self
            .cursor
            .insert_synthetic(TokenKind::Semicolon, at.range.offset);
        self.cursor.drain_to(id);
        self.span_of(id)
    }

    /// Official report anchor for `reportRecoverableError(token, ...)`: the
    /// last consumed token walked back over zero-width synthetics
    /// (`findPreviousNonZeroLengthToken`, `util.dart:39` — the walk skips only
    /// `isSynthetic && length == 0`, so an inserted identifier in front of the
    /// insertion point must not capture the anchor while a synthetic carrying
    /// real text does: the unterminated string is the official
    /// `SyntheticStringToken` shape (synthetic, non-zero length) and anchors
    /// the ensureSemicolon row on itself — corpus f59c79c8:
    /// `expected_token ';'` at the `"""` string, not the `return`). Falls
    /// back to the current token when nothing non-zero-length was consumed
    /// yet.
    #[allow(clippy::cast_possible_truncation)] // ids are u32 by design
    pub(crate) fn last_consumed_anchor(&self) -> TokenSpan {
        let table_len = self.table.len() as crate::token_id::TokenId;
        let Some(mut id) = self.cursor.last_consumed_id() else {
            return self.peek_span();
        };
        loop {
            let (kind, synthetic) = self.cursor.resolve(id);
            if id >= table_len {
                // Inserted token. Split remainders carry real text and are
                // valid anchors; a consumed synthetic steps to the previous
                // inserted entry, or to the last real token consumed before
                // the insertions.
                if !synthetic {
                    break;
                }
                if id == table_len {
                    if self.cursor.real_pos() == 0 {
                        return self.peek_span();
                    }
                    id = (self.cursor.real_pos() - 1) as crate::token_id::TokenId;
                } else {
                    id -= 1;
                }
                continue;
            }
            // Zero-width synthetics are stepped over; anything carrying real
            // text anchors — synthetic or not (the official walk skips only
            // `isSynthetic && length == 0`). EOF never anchors the walk-back.
            if kind != TokenKind::Eof && self.span_of(id).range.length > 0 {
                break;
            }
            if id == 0 {
                return self.peek_span();
            }
            id -= 1;
        }
        self.span_of(id)
    }

    /// Official `ensureBlock` (declaration `BlockKind`s): consumes the
    /// scanner-paired `{`...`}` group or reports + synthesizes an empty
    /// zero-width block. Returns `(open, close)`; for synthetic blocks the
    /// caller jumps to `close.id` (inserted ids resolve in the cursor).
    pub(crate) fn ensure_block(
        &mut self,
        kind: crate::message::BlockKind,
    ) -> (TokenSpan, TokenSpan) {
        if self.peek_kind() == TokenKind::OpenCurly {
            let open = self.peek_span();
            if let Some(close_id) = self.current_matching_end() {
                let close = self.span_of(close_id);
                return (open, close);
            }
        }
        // Message-carrying kinds report on the token BEFORE the missing `{`
        // (official ensureBlock: `reportRecoverableError(token, message)`,
        // parser_impl.dart:4995-5002), anchored per `findNonZeroLengthToken`
        // (util.dart:56-61): the last consumed token walked FORWARD over
        // zero-width synthetics — a synthetic name inserted in front of the
        // follower must not capture the anchor (bare `class` at EOF anchors
        // on the EOF position, oracle `issue_46346_prime_2` col 6 length 0),
        // while a real last token carries it (`class C(int a)` at EOF anchors
        // on the `)`). Template flavors (`expectedEnumBody`/`expectedFunction
        // Body`, block_kind.dart:31-34/47-50) report on `next` instead
        // (:5003-5005) — through the SAME `reportRecoverableError`
        // (:11399-11401), so a zero-width synthetic follower walks forward
        // to the next real token too (bare `enum` + a next declaration: the
        // synthetic name inserted behind a reserved head anchors the body
        // row on that head, oracle `issue_48371_prime3` col 1 length 5). The
        // switch lanes pin the same last-consumed anchor (the
        // switch-expression report lands on the `)` of `switch (n);`; the
        // switch-statement report on the `)` of `switch (x-1) + 1;` —
        // oracle keyword_named_top_level_methods 289|20). The
        // try/catch/finally lanes keep the follower anchor of their own
        // milestones pending their differential passes.
        let follower = self.peek_span();
        let at = match kind {
            crate::message::BlockKind::Class
            | crate::message::BlockKind::Mixin
            | crate::message::BlockKind::Extension
            | crate::message::BlockKind::ExtensionType
            | crate::message::BlockKind::SwitchExpression
            | crate::message::BlockKind::SwitchStatement => match self.cursor.last_consumed_id() {
                Some(id) if self.span_of(id).range.length > 0 => self.span_of(id),
                _ => follower,
            },
            // findNonZeroLengthToken over the follower: forward while
            // zero-width, EOF terminates the walk and stays as-is.
            _ => {
                let mut at = follower;
                let mut n = 0usize;
                while at.range.length == 0 && self.cursor.look_kind(n) != TokenKind::Eof {
                    n += 1;
                    at = self.look_span_at(n);
                }
                at
            }
        };
        let lexeme = self.cursor.lexeme(follower.id);
        self.report(msg::expected_block_body(kind, lexeme), at);
        let offset = follower.range.offset;
        let open = self.synthetic(TokenKind::OpenCurly, offset);
        let close = self.synthetic(TokenKind::CloseCurly, offset);
        (open, close)
    }

    /// Diet-mode block consumption (`skipBlock`): `ensure_block` and a jump
    /// to the closing `}`.
    pub(crate) fn skip_block(&mut self, kind: crate::message::BlockKind) -> TokenSpan {
        let (_, close) = self.ensure_block(kind);
        self.skip_to_token(close.id);
        let close_span = self.peek_span();
        self.cursor.advance();
        close_span
    }

    /// No-progress guard (official strategy C): returns `true` when the
    /// caller made no progress since `before` and must force-advance.
    pub(crate) fn no_progress(&self, before: u64) -> bool {
        self.cursor.progress() == before
    }

    /// Forces one token of progress (official :445-456 / :10530-10537).
    pub(crate) fn force_advance(&mut self) {
        self.cursor.advance();
    }

    /// Official `recoverFromStackOverflow` (`parser_impl.dart:11353-11365`),
    /// the statement-depth fuse (`parseStatement`, :6686-6695): report
    /// `stackOverflow` once on the statement's first token, insert a
    /// synthetic `;` with its `handleEmptyStatement` event, and skip to the
    /// enclosing `}` (or EOF) — then CONTINUE parsing. The official guard is
    /// a local recursion counter (`statementDepth`, decremented on unwind),
    /// not a terminal latch: the enclosing frames unwind through their
    /// closers by kind, and the abandoned remainder falls to the ordinary
    /// recovery faces — the unit member loop reports `expected_executable`
    /// per stray `}`, which is exactly the oracle's post-fuse cascade on the
    /// corpus case `deep_nesting_statement_test_01` (9,753 rows: the fuse
    /// report, 9,750 per-stray-`}` rows, the tail declaration faces). One
    /// report per breach site: the counter trips only on the 502nd nested
    /// statement entry, and the unwind brings it back below the cap.
    pub(crate) fn recover_from_stack_overflow(&mut self) {
        let at = self.peek_span();
        self.report(msg::stack_overflow(crate::parser::MAX_DEPTH), at);
        let semi = self.synthetic(TokenKind::Semicolon, at.range.offset);
        self.listener.handle_empty_statement(semi);
        while !self.cursor.at_eof() && self.peek_kind() != TokenKind::CloseCurly {
            self.cursor.advance();
        }
    }

    /// Official `parseExpression` stack-overflow recovery
    /// (`parser_impl.dart:6936-6963`), the expression-depth fuse: report
    /// once on the expression start, skip to the upcoming token's
    /// scanner-paired group end — or, when the token carries no group, to
    /// the next `)`/`]`/`}`/`;` — splice a synthetic identifier in front of
    /// it (`insertSyntheticIdentifier` + `handleIdentifier(expression)`),
    /// and return that identifier as the expression.
    ///
    /// Deliberately NOT a terminal bailout: the official parse keeps
    /// running, so the enclosing frames unwind through their closers by
    /// kind and the stranded leftovers fall to the ordinary recovery faces
    /// (`ensureSemicolon`'s walked-back anchor, the top-level invalid
    /// declaration). Those faces are where the oracle's post-fuse rows come
    /// from — oracle `stack_overflow`: `expected_token ';'` on the last
    /// adopted closer, `expected_executable` on the leftover `)`,
    /// `unexpected_token` on the final `;`.
    pub(crate) fn recover_stack_overflow_expression(&mut self) -> TokenSpan {
        let at = self.peek_span();
        self.report(msg::stack_overflow(crate::parser::MAX_DEPTH), at);
        if let Some(end) = self.current_matching_end() {
            self.skip_to_token(end);
        } else {
            while !self.cursor.at_eof()
                && !matches!(
                    self.peek_kind(),
                    TokenKind::CloseParen
                        | TokenKind::CloseSquare
                        | TokenKind::CloseCurly
                        | TokenKind::Semicolon
                )
            {
                self.cursor.advance();
            }
        }
        if self.cursor.at_eof() {
            return self.peek_span();
        }
        // The synthetic head rides the queue in front of the stop token; the
        // single advance consumes it (the missing-name splice's contract), so
        // the cursor sits on the stop token for the unwinding frames.
        let at = self.peek_span();
        let id = self
            .cursor
            .insert_synthetic(TokenKind::Identifier, at.range.offset);
        self.cursor.advance();
        let span = self.span_of(id);
        self.listener
            .handle_identifier(span, IdentifierContext::Expression);
        span
    }
}

/// Token kind for a fixed lexeme, for synthesizing recovery tokens. Only
/// the lexemes this crate synthesizes are mapped; the scanner's
/// `TokenKind::fixed_lexeme` is the forward direction.
pub(crate) fn lexeme_kind(lexeme: &str) -> TokenKind {
    match lexeme {
        "," => TokenKind::Comma,
        ":" => TokenKind::Colon,
        "}" => TokenKind::CloseCurly,
        ")" => TokenKind::CloseParen,
        "]" => TokenKind::CloseSquare,
        ">" => TokenKind::Gt,
        ";" => TokenKind::Semicolon,
        "=>" => TokenKind::Function,
        _ => TokenKind::Identifier,
    }
}

/// Identifier-shaped tokens: identifiers plus built-in/pseudo keywords in
/// identifier positions (official `token.isIdentifier` accepts
/// pseudo/built-in keywords; reserved words never).
pub(crate) fn is_identifier(kind: TokenKind) -> bool {
    match kind {
        TokenKind::Identifier => true,
        TokenKind::Keyword(kw) => kw.style() != dartq_scanner::KeywordStyle::Reserved,
        _ => false,
    }
}

/// Contexts whose official recovery template keeps the identifier-position
/// keyword sequence intact: a reserved word is reported
/// (`expectedIdentifierButGotKeyword`) and consumed as the name. The
/// exceptions (`identifier_context_impl.dart`) are the expression contexts
/// (their own `as`/`is`/`EOF`-sensitive template), the type-reference
/// contexts (`expectedType` family), `catchParameter` (`catchSyntax`
/// template) and `fieldInitializer` (always `expectedIdentifier`).
fn context_tolerates_keyword_as_name(context: crate::listener::IdentifierContext) -> bool {
    !matches!(
        context,
        crate::listener::IdentifierContext::Expression
            | crate::listener::IdentifierContext::ExpressionContinuation
            | crate::listener::IdentifierContext::TypeReference
            | crate::listener::IdentifierContext::PrefixedTypeReference
            | crate::listener::IdentifierContext::TypeReferenceContinuation
            | crate::listener::IdentifierContext::CatchParameter
            | crate::listener::IdentifierContext::FieldInitializer
    )
}

/// The report (if any) for consuming `view` as an identifier in `context`,
/// mirroring the official context templates: keyword-tolerant contexts
/// report `expectedIdentifierButGotKeyword` for reserved words; the
/// declaration-name contexts carry the per-context built-in face (the
/// converter re-faces the shared parser's `builtInIdentifierInDeclaration`
/// through `_checkForBuiltInIdentifierAsName`, error_verifier.dart:3760);
/// the type-reference family reports its own built-in/`expectedType` faces
/// (`TypeReferenceIdentifierContext.ensureIdentifier`); everything else
/// falls back to the generic `missing_identifier` path.
/// `variance_enabled` gates the verifier's variance branch
/// (error_verifier.dart:3774-3780): the pseudo pair `inout`/`out` takes the
/// built-in face exactly like built-in keywords when variance is on.
fn keyword_as_name_message(
    context: crate::listener::IdentifierContext,
    view: &crate::cursor::TokenView,
    cursor: &crate::cursor::TokenCursor<'_, '_>,
    variance_enabled: bool,
) -> Option<crate::message::Message> {
    let TokenKind::Keyword(kw) = view.kind else {
        return None;
    };
    let lexeme = cursor.lexeme(view.span.id);
    let built_in_face = matches!(kw.style(), dartq_scanner::KeywordStyle::BuiltIn)
        || (variance_enabled
            && matches!(
                kw,
                dartq_scanner::Keyword::Inout | dartq_scanner::Keyword::Out
            ));
    if built_in_face {
        use crate::message::msg::BuiltInNameKind;
        let kind = match context {
            crate::listener::IdentifierContext::EnumDeclaration => Some(BuiltInNameKind::Type),
            crate::listener::IdentifierContext::ImportPrefixDeclaration => {
                Some(BuiltInNameKind::Prefix)
            }
            crate::listener::IdentifierContext::TypeVariableDeclaration => {
                Some(BuiltInNameKind::TypeParameter)
            }
            _ => None,
        };
        if let Some(kind) = kind {
            return Some(msg::built_in_identifier_as_name(kind, lexeme));
        }
    }
    // Type-reference family (`TypeReferenceIdentifierContext.ensureIdentifier`,
    // identifier_context_impl.dart:1507-1560): every keyword branch consumes
    // the keyword as the type name. A built-in reports
    // `builtInIdentifierAsType` on the plain/continuation faces
    // (`isBuiltInIdentifierAllowed: false`, :1512/:1542-1549) and passes
    // silently on the prefixed face (`isBuiltInIdentifierAllowed: true`,
    // :1523-1533); a reserved word takes the `expectedType` else-branch
    // (:1553-1556). `void` and the valid-type built-ins `dynamic`/`Function`
    // never reach the keyword branch — `isValidNonRecordTypeReference`
    // early-returns them as valid types (type_info.dart:139-151) — and
    // `var` owns its `varAsTypeName` arm in `ensure_identifier`, so all
    // three fall through. Pseudo keywords are valid type names
    // (type_info.dart:144).
    if matches!(
        context,
        crate::listener::IdentifierContext::TypeReference
            | crate::listener::IdentifierContext::PrefixedTypeReference
            | crate::listener::IdentifierContext::TypeReferenceContinuation
    ) {
        return match kw.style() {
            dartq_scanner::KeywordStyle::BuiltIn
                if context != crate::listener::IdentifierContext::PrefixedTypeReference
                    && !matches!(
                        kw,
                        dartq_scanner::Keyword::Dynamic | dartq_scanner::Keyword::Function
                    ) =>
            {
                Some(msg::built_in_identifier_as_type(lexeme))
            }
            dartq_scanner::KeywordStyle::Reserved
                if !matches!(kw, dartq_scanner::Keyword::Void | dartq_scanner::Keyword::Var) =>
            {
                Some(msg::expected_type_name())
            }
            _ => None,
        };
    }
    match kw.style() {
        dartq_scanner::KeywordStyle::Reserved => {
            if context_tolerates_keyword_as_name(context) {
                Some(msg::expected_identifier_but_got_keyword(lexeme))
            } else {
                None
            }
        }
        // Typedef names: the `(`/`=` recovered shapes route through
        // `ensureIdentifierPotentiallyRecovered` (which reports
        // `expectedIdentifierButGotKeyword` itself), and a built-in kept by
        // the `_isOneOfFollowingValues` gate on `;`/`<`/EOF emits only the
        // caller's `builtInIdentifierInDeclaration` verifier row
        // (`builtin_typedef_name_row`); oracle `typedef int get;` carries the
        // single built-in face, never the keyword-name row. Reaching this
        // fallthrough means the head stayed the name, so nothing is added.
        _ => None,
    }
}
