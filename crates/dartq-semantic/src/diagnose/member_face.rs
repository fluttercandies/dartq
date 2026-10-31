//! The `unnecessary_final` and `missing_required_param` WARNING faces
//! (#2993) — the parameter-shape members of the best-practices family.
//!
//! # `unnecessary_final`, mirrored
//!
//! `BestPracticesVerifier._checkFinalParameter`
//! (upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/best_practices_verifier.dart:1040-1049)
//! reports the `final` keyword token of a `FieldFormalParameter` or a
//! `SuperFormalParameter` only — the check runs solely from
//! `visitFieldFormalParameter`/:496-504 and `visitSuperFormalParameter`
//! /:902-910, so `final this.x`/`final super.x` are the whole fire
//! surface; every other `final` inside a formal list (plain, named,
//! optional, function-typed, closure formals) answers the parser's
//! `extraneous_modifier` face instead, never this one.
//!
//! The suppress arm rides `_inPrimaryConstructorDeclaration`
//! (:101, set/reset across `visitPrimaryConstructorDeclaration`
//! :818-824): a `final this.`/`final super.` inside a class/enum/
//! mixin/extension/extension-type declaration head's own parameter
//! list stays silent (upstream still parses the head as a
//! `PrimaryConstructorDeclaration` even on the mixin/extension error
//! arms — parser_impl.dart `parsePrimaryConstructorOpt` :3793-3843
//! calls `beginPrimaryConstructor` for every `X(…)`/`X.n(…)` head).
//!
//! The token-level mirror: a `final` is a field/super formal's iff it
//! sits inside a parenthesis and the parser's `fieldFormalParameter`
//! shape follows it — an optional type annotation, then `this`/`super`
//! + `.` (`final int this.x`, `final Map<String, int> this.m`; the
//! receiver pair can appear nowhere else in a formal list); the
//! primary-constructor suppress is the enclosing paren group's head
//! spelling `class`/`enum`/`mixin`/`extension` + name (`[.name]`) — the
//! walk-back over the `(`-preceding `Identifier`/`.` run lands on the
//! declaration keyword. The `for`/`if-case` head exclusions ride the
//! same bracket walk as before (a `for`/`if` head holds no formal).
//!
//! # `missing_required_param`, mirrored
//!
//! `RequiredParametersVerifier._check` (required_parameters_verifier.
//! dart:120-196): for every invocation shape, each OPTIONAL named formal
//! (`isOptionalNamed` — the `{@required int x}` legacy shape; the
//! `required`-keyword formals answer the `missing_required_argument`
//! ERROR face instead) whose metadata carries package:meta's `@required`
//! getter or `@Required` class (`ElementAnnotation.isRequired`,
//! element.dart:1907-1910) and whose call site supplies no matching
//! named argument reports at the invocation's callee entity
//! (`errorEntity`: the method-name / constructor-name token). The
//! `@Required('reason')` const value's `reason` field rides the
//! with-details text (`missingRequiredParamWithDetails`, :172-190 and
//! `getReason` :253-264).
//!
//! This pass rides the post-pass-2 resolution products (bare `refs`,
//! `member_refs` edges, creation `type_uses`) and re-pairs the call
//! sites' named labels from the token stream — the invocation funnel
//! itself (`level.args`) lives in a sibling lane's file, and the
//! re-derivation covers the corpus's plain, qualified, and creation
//! heads. Under-reports: `super.x()`/`this.x()` initializer forwarding
//! (the `_containsNamedExpression` enclosing-constructor arm), implicit
//! `call` invocations, and enum-constant declaration heads.

use super::*;
use crate::codes::MissingRequiredParamShape;
use dartq_scanner::{Keyword, TokenKind};

/// One open bracket group of the `unnecessary_final` walk.
#[derive(Clone, Copy)]
struct BracketFrame {
    /// Whether the group is a parenthesis (a `final` inside a `[`/`{`
    /// optional/named group still reports through its enclosing
    /// formal-list `(`; only `paren` groups can open a fresh formal
    /// context, but `[`/`{` nested in one inherit it).
    paren: bool,
    /// The token kind just before the opener (the `for`/`if` exclusion
    /// heads); meaningless for braces/brackets.
    opener_prev: TokenKind,
    /// A `case` keyword sat at the group's own level before now (the
    /// `if (x case …)` pattern arm).
    case_seen: bool,
    /// The group is, or sits inside, a **declaration formal list** — the
    /// `(…)` a `FieldFormalParameter`/`SuperFormalParameter` can live in
    /// (best_practices_verifier.dart:1040-1049 reached only from
    /// `visitFieldFormalParameter`/`visitSuperFormalParameter`
    /// :496-504/:902-910). A `(` gets it from `formal_list_paren`; a
    /// `[`/`{` inherits it from its parent (the optional/named group of
    /// that formal list) — so `C({required final this.x})` reports while
    /// `g(final this.x)` (a call's arg list) and `void Function(final
    /// this.x)` (a function-type list, parsed as `FunctionTypedParameter`,
    /// never visited) stay silent.
    formal_list: bool,
    /// The group is a **declaration body** — a class/enum/mixin/extension
    /// body (never a function body, and never the unit's own top level,
    /// which is a declaration region by having no enclosing group at all) —
    /// where an `Identifier(…)` opens a declaration's formal list rather
    /// than a statement's call. `check_unnecessary_final_parameters` feeds
    /// it to `formal_list_paren`'s `)`-`;`/`Identifier` ambiguity
    /// resolution: `class C { C(final this.x); }` (decl body) reports,
    /// `void m() { g(final this.x); }` (statement body) stays silent.
    decl_body: bool,
    /// The group sits inside a primary-constructor declaration head —
    /// the `_inPrimaryConstructorDeclaration` suppress
    /// (best_practices_verifier.dart:101/:818-824): a `class`/`enum`/
    /// `mixin`/`extension` declaration head's own `X(…)`/`X.n(…)`
    /// parameter list and every group nested in it.
    primary_ctor: bool,
}

impl Engine<'_> {
    /// The whole-unit `unnecessary_final` pass: every `final` keyword
    /// token inside a **declaration formal list** that introduces a
    /// `this.`/`super.` receiver — the `FieldFormalParameter`/
    /// `SuperFormalParameter` gate (best_practices_verifier.dart:1040-1049,
    /// called only from `visitFieldFormalParameter`/`visitSuperFormalParameter`
    /// :496-504/:902-910), with the optional type annotation skipped
    /// (`final int this.x`). The formal-list reach mirrors the parser: the
    /// `final` may sit directly in the `(` or inside its `[`/`{`
    /// optional/named group (`C({required final this.x})`), and stays
    /// silent inside a `Function(…)` type list, a parenthesized
    /// expression, a call's argument list, a `for`/`if-case` head, and a
    /// primary-constructor head.
    pub(crate) fn check_unnecessary_final_parameters(&mut self) {
        let tokens = self.tokens.as_slice();
        let mut stack: Vec<BracketFrame> = Vec::new();
        for (index, token) in tokens.iter().enumerate() {
            match token.kind {
                TokenKind::OpenParen => {
                    let opener_prev = index
                        .checked_sub(1)
                        .map(|prev| tokens[prev].kind)
                        .unwrap_or(TokenKind::Eof);
                    let parent = stack.last().copied();
                    stack.push(BracketFrame {
                        paren: true,
                        opener_prev,
                        case_seen: false,
                        // A `(` opens a formal list by its own shape —
                        // `formal_list_paren` — never by inheritance: the
                        // `(final this.x,)` inside `f(…)` is a record type,
                        // not a formal list (the parser lowers it to a
                        // record, not a `FieldFormalParameter`).
                        formal_list: self.formal_list_paren(tokens, index, parent),
                        decl_body: false,
                        primary_ctor: parent.is_some_and(|f| f.primary_ctor)
                            || primary_ctor_head(tokens, index),
                    });
                }
                TokenKind::OpenCurly | TokenKind::OpenSquare => {
                    let parent = stack.last().copied();
                    stack.push(BracketFrame {
                        paren: false,
                        opener_prev: TokenKind::Eof,
                        case_seen: false,
                        // A `[`/`{` inherits the formal-list flag only
                        // when it nests DIRECTLY in a formal-list `(` —
                        // the optional/named-parameter group of that
                        // declaration's formals (`C({final this.x})`,
                        // `C([final this.x])`). Inside any other group it
                        // is a body/block, not a formal list.
                        formal_list: parent.is_some_and(|f| f.paren && f.formal_list),
                        decl_body: decl_body_brace(tokens, index, parent),
                        primary_ctor: parent.is_some_and(|f| f.primary_ctor),
                    });
                }
                TokenKind::CloseParen | TokenKind::CloseCurly | TokenKind::CloseSquare => {
                    stack.pop();
                }
                TokenKind::Keyword(Keyword::Case) => {
                    if let Some(frame) = stack.last_mut()
                        && frame.paren
                    {
                        frame.case_seen = true;
                    }
                }
                TokenKind::Keyword(Keyword::Final) => {
                    let Some(frame) = stack.last() else {
                        continue;
                    };
                    if frame.primary_ctor {
                        continue;
                    }
                    // The `final` must sit inside a declaration formal
                    // list — directly in the `(`, or in its `[`/`{`
                    // optional/named group (both flagged `formal_list`).
                    // Outside one there is no `FieldFormalParameter`/
                    // `SuperFormalParameter` to visit.
                    if !frame.formal_list {
                        continue;
                    }
                    // A `for` head declares loop variables, not formals;
                    // an `if` head's parens hold a condition and — with a
                    // `case` — a pattern (a `final` pattern variable, the
                    // pattern family's territory).
                    if frame.opener_prev == TokenKind::Keyword(Keyword::For)
                        || (frame.opener_prev == TokenKind::Keyword(Keyword::If)
                            && frame.case_seen)
                    {
                        continue;
                    }
                    // The field/super-formal gate (:1040-1049): the
                    // `final` must introduce `this.x`/`super.x` — the only
                    // formal kinds upstream visits (:496-504/:902-910).
                    // The parser's `fieldFormalParameter` shape allows a
                    // type annotation between them (`final int this.x`,
                    // `final List<T> this.items`; parser_impl.dart
                    // `parseFormalParameter` type-atom then the
                    // `this`/`super` arm), so the gate skips it.
                    if !self.field_formal_final(tokens, index) {
                        continue;
                    }
                    let span = SourceRange {
                        offset: token.utf16_offset,
                        length: token.byte_len,
                    };
                    self.emit_corrected(SemanticCode::UnnecessaryFinal, span, "");
                }
                _ => {}
            }
        }
    }

    /// Whether the `final` at `index` introduces a **field/super formal** —
    /// the parser's `fieldFormalParameter` production (`'final'
    /// finalConstVarOrType? …`, `_fe_analyzer_shared` parser_impl.dart
    /// `parseFormalParameter`: the type atom, then the `this`/`super` +
    /// `.` + name arm). A type annotation may sit between the keyword and
    /// the receiver (`final int this.x`, `final Map<String, int> this.m`,
    /// `final void Function(int)? this.f`, `final (int, String) this.r`),
    /// so the scan skips balanced groups of that annotation and stops at
    /// the first formal boundary: `,` opens the next formal, a closer ends
    /// the list, `=` starts the default value.
    fn field_formal_final(&self, tokens: &[dartq_scanner::Token], index: usize) -> bool {
        let mut cursor = index + 1;
        while let Some(token) = tokens.get(cursor) {
            match token.kind {
                TokenKind::Keyword(Keyword::This) | TokenKind::Keyword(Keyword::Super) => {
                    return tokens.get(cursor + 1).map(|t| t.kind) == Some(TokenKind::Period);
                }
                // Nested groups of a type annotation — a function type's
                // parameter list, a record type's field list, a `<…>` type
                // argument list — spell no field formal of their own.
                TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                    match self.tokens.matching_end(cursor) {
                        Some(end) => cursor = end + 1,
                        None => return false,
                    }
                }
                TokenKind::Lt => match self.angle_group_end(cursor) {
                    Some(end) => cursor = end + 1,
                    None => return false,
                },
                TokenKind::Comma
                | TokenKind::CloseParen
                | TokenKind::CloseSquare
                | TokenKind::CloseCurly
                | TokenKind::Semicolon
                | TokenKind::Eq => return false,
                _ => cursor += 1,
            }
        }
        false
    }

    /// Whether the `(` at `open` opens a **declaration formal list** —
    /// the `FormalParameterList` a `FieldFormalParameter`/`SuperFormalParameter`
    /// is parsed into. Mirrors the parser's reach through two token
    /// signatures:
    ///
    /// * **`)`-follow** — a formal list's `)` is followed by a declaration
    ///   body or terminator the expression grammar never produces after a
    ///   parenthesized group: `{` (block body), `=>` (arrow body), `async`
    ///   /`sync`/`*` (async markers), `:` (constructor initializer list),
    ///   `native`/`external`/`covariant` (decl modifiers). Any of these
    ///   proves a formal list outright (`var f = (final this.x) => x` —
    ///   a lambda — reports; upstream types it via
    ///   visitFieldFormalParameter).
    /// * **`(`-precede** — the remaining `)`-follows (`;`, `Identifier`,
    ///   `}`, `covariant`) are ambiguous between a declaration's `;`
    ///   (`C(final this.x);`) and a statement's `;` (`g(final this.x);`,
    ///   `(final this.x);`). The `(` is a formal list iff it follows a
    ///   declaration head — back over the `Identifier`/`.`/`<…>` name-run
    ///   lands on a declaration introducer or a declaration-body boundary
    ///   — and is NOT a `Function(…)` type-annotation list (whose
    ///   parameters parse as `FunctionTypedParameter`, never visited:
    ///   `void Function(final this.x)`, `void g(void Function(final
    ///   this.x) cb)`).
    ///
    /// `parent` supplies the enclosing frame's `decl_body` for the
    /// `;`/`Identifier`/`}` ambiguity (`class C { C(…); }` declares, a
    /// method body's `g(…);` calls).
    fn formal_list_paren(
        &self,
        tokens: &[dartq_scanner::Token],
        open: usize,
        parent: Option<BracketFrame>,
    ) -> bool {
        // The `(`-precede side: a `Function(…)` type-annotation list is
        // never a declaration formal list. Walk back over the name-run
        // (`Identifier`/`.`) and a balanced `<…>` group; `Function` as the
        // immediate introducer is the function-type head.
        let head = formal_head(tokens, open);
        if matches!(head, Some(TokenKind::Keyword(Keyword::Function))) {
            return false;
        }
        // The `)`-follow side.
        let Some(close) = self.tokens.matching_end(open) else {
            return false;
        };
        let follow = tokens.get(close + 1).map(|t| t.kind);
        match follow {
            // A declaration body or an explicit decl terminator — no
            // expression/argument `(` is followed by these.
            Some(
                TokenKind::OpenCurly
                | TokenKind::Function
                | TokenKind::Colon
                | TokenKind::Star
                | TokenKind::Keyword(Keyword::Async)
                | TokenKind::Keyword(Keyword::Sync)
                | TokenKind::Keyword(Keyword::Native)
                | TokenKind::Keyword(Keyword::External)
                | TokenKind::Keyword(Keyword::Covariant),
            ) => true,
            // `;`/`}`/`Identifier`/end — a declaration's `;` (`C(…);`) and
            // a statement's `;` (`g(…);`, `(…);`) both land here; split on
            // the `(`-precede declaration head. A decl-head landing inside
            // a declaration body (class/enum/extension/top-level) declares
            // a member; one inside a statement body calls.
            Some(TokenKind::Semicolon | TokenKind::CloseCurly | TokenKind::Identifier)
            // A `(` at top level (no enclosing group) is a declaration's
            // formal list — `parent == None` is the unit's own
            // declaration region, so it defaults to declaration context.
            | None => decl_head(head) && parent.map_or(true, |f| f.decl_body),
            // Any other follower — a binary/unary operator, `,`, `)`, `]`,
            // `?`, `.`, `in` — can only continue an expression, so the `(`
            // is a parenthesized expression, an argument list, or a record.
            _ => false,
        }
    }

    /// The whole-unit `missing_required_param` pass over the resolved
    /// call faces (bare references, member sends, creation heads).
    pub(crate) fn check_missing_required_params(&mut self) {
        let mut reported: FxHashSet<(u32, StringId)> = FxHashSet::default();
        let refs = std::mem::take(&mut self.refs);
        for reference in &refs {
            let RefTarget::Decl { file } = reference.target else {
                continue;
            };
            // The anchor and the argument-list walk both start at the
            // callee's name token.
            self.check_required_callee(
                reference.range,
                file,
                None,
                reference.name,
                &mut reported,
            );
        }
        self.refs = refs;
        let member_refs = std::mem::take(&mut self.report.member_refs);
        for edge in &member_refs {
            self.check_required_callee(
                edge.range,
                edge.target_file,
                Some(edge.target_container),
                edge.target_name,
                &mut reported,
            );
        }
        self.report.member_refs = member_refs;
        let type_uses = std::mem::take(&mut self.type_uses);
        for (range, name) in &type_uses {
            self.check_required_creation(*range, *name, &mut reported);
        }
        self.type_uses = type_uses;
    }

    /// One bare/member call face: the callee's optional named formals
    /// against the call site's named labels. `container` names the
    /// declaring container for a member send (`None` = a bare reference
    /// to a top-level executable).
    fn check_required_callee(
        &mut self,
        callee_range: SourceRange,
        file: FileId,
        container: Option<StringId>,
        name: StringId,
        reported: &mut FxHashSet<(u32, StringId)>,
    ) {
        // The packed-row "no container" sentinel (`StringId::NONE`, an
        // unnamed extension's edge — the reference latch's
        // `face.name.unwrap_or(StringId::NONE)`): the collector's
        // `formal_owner_key` records no `formal_annotations` row for a
        // member of a container with no name, so the required-named
        // check has nothing to join. Decline the row; resolving the
        // sentinel would panic, and joining the bare `name` could pair
        // an unrelated same-named top-level executable's annotations.
        if container == Some(StringId::NONE) {
            return;
        }
        let Some(formals) = self.member_or_decl_formals(file, container, name) else {
            return;
        };
        let owner = match container {
            None => name,
            Some(container) => self.strings.intern(&format!(
                "{}.{}",
                self.strings.resolve(container),
                self.strings.resolve(name)
            )),
        };
        let Some(open) = self.call_open_paren(callee_range) else {
            return;
        };
        self.check_required_named_formals(callee_range, open, file, formals, owner, reported);
    }

    /// One creation head: the container's constructor formal table under
    /// the written selector (`A.named(…)`; the unnamed face and the
    /// explicit `.new` both key by the container's own name), against
    /// the creation's named labels.
    fn check_required_creation(
        &mut self,
        head_range: SourceRange,
        name: StringId,
        reported: &mut FxHashSet<(u32, StringId)>,
    ) {
        let Some((formals, open, file, owner)) = self.creation_face(head_range, name) else {
            return;
        };
        self.check_required_named_formals(
            head_range,
            open,
            file,
            formals,
            self.strings.intern(&owner),
            reported,
        );
    }

    /// The resolved face behind a creation head: the constructor formal
    /// table under the written selector (`A.named(…)`; the unnamed face
    /// and the explicit `.new` both key by the container's own name),
    /// the argument list's `(` token index, the declaring file, and the
    /// `formal_annotations` owner join spelling. Shared by the
    /// `missing_required_param` walk and the
    /// `unnecessary_set_literal` closure-argument face.
    /// The shared required-named walk: each optional named formal
    /// (`isOptionalNamed` — non-required in the table) carrying a meta
    /// `@required`/`@Required` star, against the call's named labels
    /// (`_check`, required_parameters_verifier.dart:153-196). `open` is
    /// the argument list's `(` token index.
    fn check_required_named_formals(
        &mut self,
        anchor: SourceRange,
        open: usize,
        file: FileId,
        formals: &Formals,
        owner: StringId,
        reported: &mut FxHashSet<(u32, StringId)>,
    ) {
        let Some(elements) = self.index.get(file) else {
            return;
        };
        let Some(decl_lib) = self.graph.library_of_file(file) else {
            return;
        };
        let required_named: Vec<(StringId, Option<String>)> = formals
            .named
            .iter()
            .filter(|formal| !formal.required)
            .filter_map(|formal| {
                let annotations = elements.formal_annotations(owner, formal.name)?;
                let annotation = annotations.iter().find(|annotation| {
                    crate::annotation::is_meta_getter(
                        annotation,
                        "required",
                        self.strings,
                        self.graph,
                        decl_lib,
                    ) || crate::annotation::is_meta_class(
                        annotation,
                        "Required",
                        self.strings,
                        self.graph,
                        decl_lib,
                    )
                })?;
                // `getReason` (:253-264): the const value's `reason`
                // string — empty or absent answers the plain text.
                let reason = annotation
                    .args_text
                    .and_then(|text| first_quoted_string(self.strings.resolve(text)))
                    .filter(|reason| !reason.is_empty());
                Some((formal.name, reason))
            })
            .collect();
        if required_named.is_empty() {
            return;
        }
        // The call site's named labels (`_containsNamedExpression` walks
        // the argument list; only presence matters at name level).
        let labels = self.call_named_labels(open);
        for (name, reason) in required_named {
            if labels.contains(&name) || !reported.insert((anchor.offset, name)) {
                continue;
            }
            let text = self.strings.resolve(name).to_owned();
            match reason {
                Some(reason) => self.emit(
                    SemanticCode::MissingRequiredParam(MissingRequiredParamShape::WithDetails),
                    anchor,
                    &text,
                    &reason,
                ),
                None => self.emit(
                    SemanticCode::MissingRequiredParam(MissingRequiredParamShape::Plain),
                    anchor,
                    &text,
                    "",
                ),
            }
        }
    }

    /// The argument list's `(` token index after a callee name token —
    /// the callee's explicit type arguments (`g<int>(…)`) skipped.
    pub(super) fn call_open_paren(&self, callee_range: SourceRange) -> Option<usize> {
        let tokens = self.tokens.as_slice();
        let mut index = tokens.partition_point(|t| {
            t.utf16_offset < callee_range.offset + callee_range.length
        });
        if tokens.get(index).is_some_and(|t| t.kind == TokenKind::Lt) {
            index = self.angle_group_end(index)? + 1;
        }
        (tokens.get(index)?.kind == TokenKind::OpenParen).then_some(index)
    }

    /// The named labels of the argument list opened at token `open`: the
    /// top-level `name:` arguments (an argument's first tokens — an
    /// identifier immediately followed by `:` — exactly the
    /// `NamedExpression` head; a conditional's `a ? b : c` never starts
    /// an argument at its `:`).
    fn call_named_labels(&self, open: usize) -> Vec<StringId> {
        let tokens = self.tokens.as_slice();
        let Some(close) = self.tokens.matching_end(open) else {
            return Vec::new();
        };
        let mut labels = Vec::new();
        let mut cursor = open + 1;
        // Whether the next token starts an argument (after `(` and after
        // each top-level `,`).
        let mut argument_start = true;
        while cursor < close {
            let token = tokens[cursor];
            match token.kind {
                TokenKind::OpenParen | TokenKind::OpenCurly | TokenKind::OpenSquare => {
                    cursor = self.tokens.matching_end(cursor).map_or(cursor + 1, |e| e + 1);
                    argument_start = false;
                }
                TokenKind::Comma => {
                    cursor += 1;
                    argument_start = true;
                }
                TokenKind::Identifier if argument_start => {
                    if tokens.get(cursor + 1).is_some_and(|t| t.kind == TokenKind::Colon) {
                        let lexeme = self.tokens.lexeme(self.source, cursor);
                        labels.push(self.strings.intern(lexeme));
                        cursor += 2;
                    } else {
                        cursor += 1;
                    }
                    argument_start = false;
                }
                _ => {
                    cursor += 1;
                    argument_start = false;
                }
            }
        }
        labels
    }

    /// The index of the closing `>` of a balanced `<…>` group (the
    /// callee's explicit type arguments); `None` when the group never
    /// closes over the recovery boundary tokens.
    pub(super) fn angle_group_end(&self, open: usize) -> Option<usize> {
        let tokens = self.tokens.as_slice();
        let mut depth = 0i32;
        for (index, token) in tokens.iter().enumerate().skip(open) {
            match token.kind {
                TokenKind::Lt => depth += 1,
                TokenKind::Gt => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(index);
                    }
                }
                // A `>>` closes two levels (`A<List<int>>`).
                TokenKind::GtGt => {
                    depth -= 2;
                    if depth <= 0 {
                        return Some(index);
                    }
                }
                // Recovery: an unbalanced group never reaches an
                // argument list through these.
                TokenKind::Semicolon | TokenKind::OpenCurly | TokenKind::CloseCurly => {
                    return None;
                }
                _ => {}
            }
        }
        None
    }
}

/// The shared call-face resolvers, `'a`-tied: the passes hold their
/// formal tables across `&mut self` emissions.
impl<'a> Engine<'a> {
    /// The formal table behind a bare/member call face — the declaring
    /// file's model row (`None` = the name declares no formals worth
    /// modeling, or the join misses). Shared by the
    /// `missing_required_param` walk and the
    /// `unnecessary_set_literal` closure-argument face.
    pub(super) fn member_or_decl_formals(
        &self,
        file: FileId,
        container: Option<StringId>,
        name: StringId,
    ) -> Option<&'a Formals> {
        let index = self.index;
        let elements = index.get(file)?;
        match container {
            None => elements.decl(name)?.formals.as_deref(),
            Some(container) => {
                let entry = elements
                    .container(container)
                    .and_then(|row| row.members.get(&name))
                    .or_else(|| {
                        elements.extension_faces().iter().find_map(|face| {
                            (face.name == Some(container)
                                || (face.name.is_none() && container == StringId::NONE))
                                .then(|| face.members.get(&name))
                                .flatten()
                        })
                    });
                entry?.formals.as_deref()
            }
        }
    }

    /// The resolved face behind a creation head: the constructor formal
    /// table under the written selector (`A.named(…)`; the unnamed face
    /// and the explicit `.new` both key by the container's own name),
    /// the argument list's `(` token index, the declaring file, and the
    /// `formal_annotations` owner join spelling. Shared by the
    /// `missing_required_param` walk and the
    /// `unnecessary_set_literal` closure-argument face.
    pub(super) fn creation_face(
        &self,
        head_range: SourceRange,
        name: StringId,
    ) -> Option<(&'a Formals, usize, FileId, String)> {
        let file = match self.namespaces.lookup(self.graph, self.lib, name) {
            Lookup::Found(NamespaceEntry::Decl(file)) => file,
            _ => return None,
        };
        let index = self.index;
        let row = index.get(file)?.container(name)?;
        let tokens = self.tokens.as_slice();
        let after =
            tokens.partition_point(|t| t.utf16_offset < head_range.offset + head_range.length);
        // The written selector between the type head and the argument
        // list (`A.named(`), when the tokens spell one.
        let mut cursor = after;
        let mut key = name;
        if tokens.get(cursor).is_some_and(|t| t.kind == TokenKind::Period)
            && tokens
                .get(cursor + 1)
                .is_some_and(|t| t.kind == TokenKind::Identifier)
            && tokens
                .get(cursor + 2)
                .is_some_and(|t| t.kind == TokenKind::OpenParen)
        {
            key = self
                .strings
                .intern(self.tokens.lexeme(self.source, cursor + 1));
            cursor += 2;
        }
        // `A.new(…)` keys the unnamed table (`ctor_formals`'s own
        // spelling — the selector for `A.named`, the class name for the
        // unnamed face).
        if self.strings.resolve(key) == "new" {
            key = name;
        }
        if tokens.get(cursor)?.kind != TokenKind::OpenParen {
            return None;
        }
        let formals = row
            .ctor_formals
            .get(&key)
            .or_else(|| row.factory_formals.get(&key))?;
        let owner = format!(
            "{}.{}",
            self.strings.resolve(name),
            self.strings.resolve(key)
        );
        Some((formals, cursor, file, owner))
    }
}

/// The token kind of the declaration/head that introduces the `(` at
/// `open`, walked back over the `Identifier`/`.` name-run, a balanced
/// `<…>` type-parameter/argument group, and a trailing operator-symbol
/// run (`operator +`, `operator []=`). `None` when `open` leads the unit.
/// This is the `(`-precede signature `formal_list_paren` and `decl_head`
/// share — the same backward shape `primary_ctor_head` walks.
fn formal_head(tokens: &[dartq_scanner::Token], open: usize) -> Option<TokenKind> {
    let mut cursor = open;
    // A trailing operator-symbol run (`C operator +(` — the operator's
    // own spelling sits just before `(`); `<`/`>` are excluded — those
    // belong to a `<…>` group handled below.
    while cursor > 0
        && is_operator_spelling(tokens[cursor - 1].kind)
        && !matches!(
            tokens[cursor - 1].kind,
            TokenKind::Lt | TokenKind::Gt | TokenKind::GtGt | TokenKind::GtGtGt
        )
    {
        cursor -= 1;
    }
    // The `Identifier`/`.` run (the declaration or callee name, plus a
    // `.named`/`.ctorName` selector).
    while cursor > 0
        && matches!(
            tokens[cursor - 1].kind,
            TokenKind::Identifier | TokenKind::Period
        )
    {
        cursor -= 1;
    }
    // A balanced `<…>` group (declaration type parameters `f<T>(`,
    // explicit type arguments `g<int>(`), skipped backward; `(`/ `{`/
    // `;` aborts the skip (unbalanced recovery).
    if cursor > 0 {
        let width = |kind: TokenKind| -> i32 {
            match kind {
                TokenKind::Gt => 1,
                TokenKind::GtGt => 2,
                TokenKind::GtGtGt => 3,
                _ => 0,
            }
        };
        let mut depth = width(tokens[cursor - 1].kind);
        if depth > 0 {
            let mut i = cursor - 1;
            while depth > 0 && i > 0 {
                i -= 1;
                match tokens[i].kind {
                    TokenKind::Lt => depth -= 1,
                    TokenKind::Gt | TokenKind::GtGt | TokenKind::GtGtGt => {
                        depth += width(tokens[i].kind);
                    }
                    TokenKind::OpenParen | TokenKind::OpenCurly | TokenKind::Semicolon => {
                        return Some(tokens[i].kind);
                    }
                    _ => {}
                }
            }
            if depth == 0 {
                cursor = i;
                // The declaration name ahead of the type parameters.
                while cursor > 0
                    && matches!(
                        tokens[cursor - 1].kind,
                        TokenKind::Identifier | TokenKind::Period
                    )
                {
                    cursor -= 1;
                }
            }
        }
    }
    (cursor > 0).then(|| tokens[cursor - 1].kind)
}

/// A token spelling usable as a user-defined `operator` name (`+`,
/// `[]`, `==`, …) — the `operator` keyword itself is excluded (it
/// introduces the name, never spells it).
fn is_operator_spelling(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Percent
            | TokenKind::Tilde
            | TokenKind::TildeSlash
            | TokenKind::Ampersand
            | TokenKind::Bar
            | TokenKind::Caret
            | TokenKind::LtLt
            | TokenKind::GtGt
            | TokenKind::GtGtGt
            | TokenKind::Index
            | TokenKind::IndexEq
            | TokenKind::Lt
            | TokenKind::LtEq
            | TokenKind::Gt
            | TokenKind::GtEq
            | TokenKind::EqEq
            | TokenKind::BangEq
    )
}

/// Whether the head introducing the `(` at `open` is a **declaration
/// introducer or a declaration-body boundary** — the `(`-precede half of
/// the `;`/`Identifier`/`}` ambiguity in `formal_list_paren`. `head` is
/// `formal_head`'s answer: the token just before the name-run.
///
/// A declaration introducer: a return type or modifier (`void`, `set`,
/// `get`, `operator`, `factory`, `typedef`, `external`, `static`,
/// `abstract`, `covariant`, `late`, `final`, `const`, `var`, `extends`,
/// `implements`, `with`, `on`), or a declaration-body boundary (`;`,
/// `{`, `}`, a `<…>` group's `>`, the unit start `None`). A call or an
/// expression lands on `=`, `,`, `(`, `?`, `return`, `await`, `case`,
/// `in`, `=>`, a binary/unary operator, or `Function` — none declares.
fn decl_head(head: Option<TokenKind>) -> bool {
    match head {
        // Unit start — a top-level declaration's `(`.
        None => true,
        Some(TokenKind::Keyword(
            Keyword::Void
            | Keyword::Set
            | Keyword::Get
            | Keyword::Operator
            | Keyword::Factory
            | Keyword::Typedef
            | Keyword::External
            | Keyword::Static
            | Keyword::Abstract
            | Keyword::Covariant
            | Keyword::Late
            | Keyword::Final
            | Keyword::Const
            | Keyword::Var
            | Keyword::Extends
            | Keyword::Implements
            | Keyword::With
            | Keyword::On
            | Keyword::Class
            | Keyword::Enum
            | Keyword::Mixin
            | Keyword::Extension,
        )) => true,
        // A declaration-body boundary — a member/`;` boundary inside a
        // declaration body (`class C { C(…); }`, `; void m(…)`), or the
        // closing `>` of a type-parameter group.
        Some(TokenKind::Semicolon | TokenKind::OpenCurly | TokenKind::CloseCurly | TokenKind::Gt) => {
            true
        }
        // Anything else — `=`, `,`, `(`, `?`, `return`, `await`, `case`,
        // `in`, `=>`, `Function`, an operator, a literal — makes the `(`
        // an expression, a call's argument list, or a function-type list.
        _ => false,
    }
}

/// Whether the `(` at `open` opens a primary-constructor declaration
/// head — upstream `parsePrimaryConstructorOpt` (parser_impl.dart
/// :3789-3792) fires `beginPrimaryConstructor` whenever a declaration
/// head's `classNamePart` (`typeIdentifier typeParameters?` plus an
/// optional `.ctorName`) is followed by `(` or `.name(`, for
/// `class`/`enum`/`mixin`/`extension`/`extension type` alike (the
/// mixin/extension arms report a recoverable error but still build the
/// node, so their `final`s stay suppressed too —
/// best_practices_verifier.dart:818-824).
///
/// The token-level shape, walked backward from `open`: an optional
/// `.ctorName`/`Name` `Identifier`/`Period` run, an optional balanced
/// `<…>` type-parameter group, the declaration `Name` run, then the
/// `class`/`enum`/`mixin`/`extension` keyword (`extension type`'s
/// `type` scans as an Identifier, so the run covers it). A regular
/// constructor's `(` inside a body lands on `{`/`}`/`;`, a function's
/// on a return type/`void` — none is a declaration keyword.
fn primary_ctor_head(tokens: &[dartq_scanner::Token], open: usize) -> bool {
    // One `Identifier`/`.` run (a name, or `.ctorName`, or `type` in
    // `extension type`); never crosses the group's own opener.
    fn name_run(tokens: &[dartq_scanner::Token], cursor: &mut usize) {
        while *cursor > 0
            && matches!(
                tokens[*cursor - 1].kind,
                TokenKind::Identifier | TokenKind::Period
            )
        {
            *cursor -= 1;
        }
    }
    let mut cursor = open;
    // The optional `.ctorName` and/or the declaration name.
    name_run(tokens, &mut cursor);
    // The optional `<T…>` type-parameter group, skipped backward:
    // `>`/`>>`/`>>>` open the skip at their width and each `<` closes
    // one level; a `(`, `{`, or `;` aborts (unbalanced recovery).
    if cursor > 0 {
        let width = |kind: TokenKind| -> i32 {
            match kind {
                TokenKind::Gt => 1,
                TokenKind::GtGt => 2,
                TokenKind::GtGtGt => 3,
                _ => 0,
            }
        };
        let mut depth = width(tokens[cursor - 1].kind);
        if depth > 0 {
            let mut i = cursor - 1;
            while depth > 0 && i > 0 {
                i -= 1;
                match tokens[i].kind {
                    TokenKind::Lt => depth -= 1,
                    TokenKind::Gt | TokenKind::GtGt | TokenKind::GtGtGt => {
                        depth += width(tokens[i].kind);
                    }
                    TokenKind::OpenParen | TokenKind::OpenCurly | TokenKind::Semicolon => {
                        return false;
                    }
                    _ => {}
                }
            }
            if depth != 0 {
                return false;
            }
            cursor = i;
        }
    }
    // The declaration name ahead of the type parameters.
    name_run(tokens, &mut cursor);
    cursor > 0
        && matches!(
            tokens[cursor - 1].kind,
            TokenKind::Keyword(Keyword::Class)
                | TokenKind::Keyword(Keyword::Enum)
                | TokenKind::Keyword(Keyword::Mixin)
                | TokenKind::Keyword(Keyword::Extension)
        )
}

/// Whether the `{`/`[` at `open` opens a **declaration body** — the body of
/// a `class`/`enum`/`mixin`/`extension`/`extension type` declaration, where
/// an `Identifier(…)` declares a member's formal list. The head walk runs
/// backward to the nearest `{`/`}`/`;` boundary and asks whether an
/// introducer keyword sits inside that span (`class C<T> implements I with
/// M`, `extension type E(int it)`, `enum E`, `augment class C`). A
/// function, method, getter, setter, constructor, or lambda body (`{` after
/// `)`, `=>`, `=`, or a non-introducer head such as `int get g`) reaches the
/// boundary first and stays a statement region — `void f() { g(final
/// this.x); }` calls, `class C { C(final this.x); }` declares. A `[` is
/// never a declaration body, and neither is a `{` nested in a statement
/// region: `void f() { class C { … } }` has no local declaration (the
/// parser reads that `class` as an identifier and recovers), so its
/// `Identifier(…)` stays a call.
fn decl_body_brace(
    tokens: &[dartq_scanner::Token],
    open: usize,
    parent: Option<BracketFrame>,
) -> bool {
    if tokens[open].kind == TokenKind::OpenSquare {
        return false;
    }
    if parent.is_some_and(|frame| !frame.decl_body) {
        return false;
    }
    let mut cursor = open;
    while cursor > 0 {
        cursor -= 1;
        match tokens[cursor].kind {
            TokenKind::OpenCurly | TokenKind::CloseCurly | TokenKind::Semicolon => return false,
            TokenKind::Keyword(
                Keyword::Class | Keyword::Enum | Keyword::Mixin | Keyword::Extension,
            ) => return true,
            _ => {}
        }
    }
    false
}

/// The cooked first quoted string of an annotation argument group
/// (`('reason')`, `(reason: 'reason')` — the `getReason` value face; a
/// plain scan is exact for the corpus's literal shapes).
fn first_quoted_string(text: &str) -> Option<String> {
    let start = text.find(['\'', '"'])?;
    let quote = text.as_bytes()[start];
    let rest = &text[start + 1..];
    let end = rest.find(quote as char)?;
    Some(rest[..end].to_owned())
}
