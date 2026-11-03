//! The numeric refinement's operand face: the one consumer where the
//! engine's [`Ty::Unknown`] must split into the oracle's `InvalidType`
//! and the engine's own typing gaps.
//!
//! `_refineNumericInvocationTypeNullSafe` (type_system.dart:2058-2121)
//! types `e1 + e2` on a `num`-family receiver from the operand types, and
//! its `S <: double?` test reads `InvalidType` as a subtype of everything
//! (subtype.dart `_isSubtypeOf`: `if (T0 is InvalidTypeImpl || T1 is
//! InvalidTypeImpl) return true;`), so `int - <undefined>` answers
//! `double` and the downstream `int` sinks report. [`Ty::Unknown`]
//! conflates that face with a declared type the name-level model cannot
//! render — an untyped field, a receiver the model does not carry, a fold
//! the lattice declined. Reading every `Unknown` as `InvalidType` turned
//! each typing gap into a `double` false positive at the next `int` sink
//! (the archive `static const _n = …` family, the analyzer
//! `double`↔`int` family). The split asks the ROW: a resolution failure
//! the oracle also sees is [`OperandFace::Invalid`]; everything else the
//! engine cannot type is a [`OperandFace::Gap`] that suppresses the
//! refinement (the under-report direction).

use super::*;

/// A binary operand as the numeric refinement reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OperandFace {
    /// A static type the engine rendered.
    Typed(Ty),
    /// The oracle's `InvalidType`: an undefined bare name, or a member
    /// read on a receiver whose written head resolves to no declaration.
    Invalid,
    /// A face the engine cannot type though the oracle can.
    Gap,
}

impl<'a> Engine<'a> {
    /// The refinement face of one operand row whose static type is `ty`
    /// (the caller's [`Self::equality_operand_ty`] answer).
    pub(crate) fn operand_face(&self, row: &Row, ty: Ty) -> OperandFace {
        match ty {
            Ty::Unknown => {}
            // A type parameter's head answers through its bound in the
            // oracle (`T extends num` refines to `num`, `T extends double`
            // to `double`); the name level cannot read the bound, so the
            // refinement declines rather than guessing a branch.
            Ty::Named { name, .. } if self.type_params.contains(&name) => return OperandFace::Gap,
            _ => return OperandFace::Typed(ty),
        }
        match &row.shape {
            // The bare miss is `undefined_identifier`'s InvalidType. An
            // opaque-`on` body (a mixin or extension) may define the name
            // through its constraint faces the model cannot walk, so the
            // miss there stays a gap.
            RowShape::Bare(BareInfo::Missing { on_opaque, .. }) => {
                if *on_opaque {
                    OperandFace::Gap
                } else {
                    OperandFace::Invalid
                }
            }
            // A member read on a receiver whose written head resolves to
            // nothing in scope (`Token operator; … - operator.offset` with
            // `Token` undefined — probed 3.13.1) is the oracle's
            // InvalidType receiver: no member lookup runs, the read itself
            // types InvalidType. A `dynamic` receiver types the read
            // `dynamic` (`type_property_resolver`'s dynamic short-circuit).
            // A resolved head whose member the model cannot type is a gap
            // — including a missing member, which an extension the pass-2
            // applicability walk has not run yet may still supply.
            RowShape::Member {
                recv: Receiver::Typed { type_name, ty },
            } => {
                if *ty == Ty::Dynamic {
                    OperandFace::Typed(Ty::Dynamic)
                } else if self.type_params.contains(type_name)
                    || self.name_world().is_known_head(*type_name)
                {
                    OperandFace::Gap
                } else if self.alias_rhs(*type_name).is_some() {
                    // A receiver head spelling a type alias resolves through
                    // its aliased type upstream (`typedef P = _P;` — `P` is
                    // `_P` before any member question), so the head is never
                    // the oracle's InvalidType — the member miss or the
                    // untyped result is the known-head arm's same gap.
                    OperandFace::Gap
                } else {
                    OperandFace::Invalid
                }
            }
            // The `Unknown` receiver's InvalidType face: the member merge
            // folds every unspellable receiver — a `Bare(Missing)` head
            // (`x.y` / `Missing.x` / `p.x` with `p` undefined), a
            // non-`Named` `Plain` head (a function- or record-typed local),
            // or no head row at all — into the same `Receiver::Unknown`.
            // Upstream only the first is `InvalidType`: an undefined bare
            // head's own row reports `undefined_identifier`, and a member
            // read on an `InvalidType` receiver types `InvalidType` (the
            // resolver never runs the lookup); the other two are the
            // documented `dynamic`/model gaps. The push-time verdicts ride
            // `refs` — the folded receiver's read span (`member_reads`
            // `recv_span`) keys whether its head resolved to nothing, the
            // same answer the receiver's own row reports.
            RowShape::Member {
                recv: Receiver::Unknown,
            } => {
                let Ok(index) = self
                    .member_reads
                    .binary_search_by(|read| read.offset.cmp(&row.range.offset))
                else {
                    return OperandFace::Gap;
                };
                let span = self.member_reads[index].recv_span;
                if self
                    .refs
                    .binary_search_by(|r| r.range.offset.cmp(&span.offset))
                    .ok()
                    .is_some_and(|i| {
                        self.refs[i].range == span && self.refs[i].target == RefTarget::Missing
                    })
                {
                    OperandFace::Invalid
                } else {
                    OperandFace::Gap
                }
            }
            // A prefixed member read on an import prefix (`p.y`): the
            // oracle resolves the member inside the prefix's own export
            // scope — no member walk, no extension applicability — so the
            // verdict is decidable at this face. A miss reports
            // `undefined_prefixed_name` and the read types `InvalidType`
            // (type_property_resolver's error-element path); a hit is the
            // declared element's face, which stays the model's gap.
            RowShape::Member {
                recv: Receiver::Prefix { name: prefix },
            } => {
                if matches!(
                    self.namespaces.lookup_prefixed(
                        self.graph,
                        self.lib,
                        *prefix,
                        row.name
                    ),
                    dartq_resolution::Lookup::Found(_)
                ) {
                    OperandFace::Gap
                } else {
                    OperandFace::Invalid
                }
            }
            // A flow-blind local whose initializer typed InvalidType (the
            // pass-2 pre-pass's `invalid_init_locals`): the local's
            // inferred type is the initializer's, so the read is invalid.
            RowShape::Bare(BareInfo::Plain { .. })
                if self
                    .local_decl_at_read
                    .get(&row.range.offset)
                    .is_some_and(|decl| self.invalid_init_locals.borrow().contains(decl)) =>
            {
                OperandFace::Invalid
            }
            _ => OperandFace::Gap,
        }
    }

    /// The teardown pass over the two operator families the resolved lanes
    /// never see a nullable operand through: the prefix `-`/`~` whose
    /// operand is a member/receiver/group expression (shape 4.6) and the
    /// binary application whose `Intent::Operator` operand row carries a
    /// `?.`-shorted or already-nullable member result (shape 4.8).
    ///
    /// Upstream both run through `type_property_resolver.dart`'s
    /// `isNullable` tail — after the `dynamic`/`InvalidType` short-circuit
    /// and the `Object`/extension lookups — which fires
    /// `nullableDereferenceVerifier.report` keyed on the ACCESS node's
    /// parent shape: a `BinaryExpression` parent picks the operator
    /// variant, a `PrefixExpression`/`MethodInvocation` parent the method
    /// variant (type_property_resolver.dart:128-148, reached through
    /// prefix_expression_resolver.dart's `_resolve1` for `-`/`~` and
    /// binary_expression_resolver.dart for the binary spellings). A
    /// `Null`-typed operand swaps to `invalid_use_of_null_value` on the
    /// same anchor.
    ///
    /// The row model never asks the question on either face: a member row
    /// merged into `Intent::Operator` resolves `+` against the member's
    /// result type but drops its nullability (`expression_row_ty`'s
    /// `null_aware_members` widening excludes `Intent::Operator` — the
    /// row doubles as the result face, where `x?.m + 1` types `int`, and
    /// the `isNullable` consult the operand owes never runs); a prefix
    /// operand that is not a bare read (`a?.foo`, `c.bad`, `this`, a call
    /// close) carries no `Intent::Operator` at all, so neither
    /// `mark_operator` nor the bare-row adjacent arm reaches it, and the
    /// `stage_operand_faces` token scan's `-`/`~` arm grounds on
    /// `Ty::Void` only. Both families ground here at teardown, deduped
    /// against every row the resolved lanes already emitted at the same
    /// anchor (the bare `x + 1` / `-x` verdicts, the void operand's
    /// `unary-`/`~` rows staged by `stage_operand_faces`).
    pub(crate) fn stage_nullable_operator_faces(&mut self, out: &mut Vec<Diagnostic>) {
        // Shape 4.8 — the binary application's LHS operand whose static
        // type the resolved arm reads non-nullable: a `?.`-shorted chain
        // node (`x?.bitLength + 1`, `x?[0] + 1`, `x?.m() + 1`) widens at
        // `expression_row_ty` for every intent but the operator row
        // itself (the row doubles as the application's RESULT face, where
        // `x?.m + 1` types `int` — the operand's isNullable ask never
        // runs), and a nullable member result (`c.bad + 1` over `int?
        // bad`) reaches `check_operator_expr_in` as a `Named` head the
        // arm's member lookup answers without consulting the `?`. The
        // oracle's isNullable tail runs FIRST (before the interface
        // lookup), so the verdict is the unchecked row at the operator
        // token — the same anchor `emit_unchecked_nullable` uses.
        //
        // The pass cannot iterate `self.rows` — `resolve_rows` drains the
        // table into its own local and never restores it, so the rows are
        // gone by teardown. The token scan asks the same question: every
        // unambiguous binary-operator token whose left neighbor ends an
        // operand re-derives the operand's face off the receiver walk
        // (`recv_expression_ty` answers the member/call/index result at
        // the operand's tail token — the `?.`-marked member's un-widened
        // result — with the `null_aware_members` latch restoring the
        // chain's own `?`). The `<`/`>`/`<=`/`>=` spellings stay out:
        // type-argument brackets lex the same tokens, and a
        // `List<int> + x` misread would double-report what the resolved
        // arm already decides. The equality pair `==`/`!=` and the
        // short-circuit family carry their own verdicts; the
        // assignment-class spellings (`+=`, `>>=`…) ride `write_op` arms.
        let tokens = self.tokens.as_slice();
        for i in 0..tokens.len() {
            let (op_text, binary) = match tokens[i].kind {
                dartq_scanner::TokenKind::Plus => ("+", true),
                dartq_scanner::TokenKind::Minus => ("-", !self.prefix_operator_position(i)),
                dartq_scanner::TokenKind::Star => ("*", true),
                dartq_scanner::TokenKind::Slash => ("/", true),
                dartq_scanner::TokenKind::TildeSlash => ("~/", true),
                dartq_scanner::TokenKind::Percent => ("%", true),
                dartq_scanner::TokenKind::LtLt => ("<<", true),
                dartq_scanner::TokenKind::GtGt => (">>", true),
                dartq_scanner::TokenKind::GtGtGt => (">>>", true),
                dartq_scanner::TokenKind::Ampersand => ("&", true),
                dartq_scanner::TokenKind::Bar => ("|", true),
                dartq_scanner::TokenKind::Caret => ("^", true),
                _ => ("", false),
            };
            if !binary {
                continue;
            }
            let Some(tail) = self.binary_operand_tail(i) else {
                continue;
            };
            let Some(op_range) = self.operand_token_span(i) else {
                continue;
            };
            let Some((mut ty, probe)) = self.unary_operand_ty(tail) else {
                continue;
            };
            let latched = self.null_aware_members.contains(&probe.offset);
            if latched {
                ty = Self::nullable_form(ty);
            }
            ty = self.alias_expanded_operand_ty(ty);
            if ty == Ty::Null {
                self.emit_operand_null_row(op_range, out);
                continue;
            }
            if !Self::operand_ty_nullable(ty) {
                continue;
            }
            // The resolved lane already reports the operand it OWNS at this
            // same operator token: `mark_operator` flips the last
            // `Intent::Read` row (a bare local/member read, a `this`, a
            // folded value — never the `recv.name` member tail this walk
            // lands on, whose `resolve_member_row` arm answers only the
            // operator's definedness and never the `isNullable` consult).
            // Its `!null_aware_members` gate suppresses exactly the `?.`
            // short this pass exists for, so the overlap is the unlatched
            // bare operand — identified at teardown by the bare read's
            // surviving products (`local_hits`/`refs`), `self.report` being
            // already drained (diagnose.rs's `mem::take` empties it before
            // `filter_staged` runs).
            if !latched && self.bare_operator_operand(probe, true) {
                continue;
            }
            // An `on`-nullable extension whose face declares the spelling
            // resolves the application — the `Single` verdict the bare-row
            // arm's `operator_face_extension_resolved` already takes
            // (only a `Named` head consults the extension faces).
            if let Ty::Named { name: head, .. } = ty
                && matches!(
                    self.extension_application(head, ty, self.intern(op_text), false, true),
                    ExtensionApp::Single
                )
            {
                continue;
            }
            self.emit_operand_unchecked(op_range, op_text, UncheckedNullableKind::Operator, out);
        }

        // Shape 4.6 — prefix `-`/`~`: the operand is the expression AFTER
        // the token (a unary arm's operand window is forward), grounded by
        // its tail token — the member selector `a?.foo`, the field read
        // `c.bad`, the keyword `this`, the call close `c?.e()`, the group
        // `-(e)`. The resolved lanes only ever see the bare-read shape
        // (`-x`), which the report-side dedup keeps single.
        let tokens = self.tokens.as_slice();
        for i in 0..tokens.len() {
            let name = match tokens[i].kind {
                dartq_scanner::TokenKind::Minus => "unary-",
                dartq_scanner::TokenKind::Tilde => "~",
                _ => continue,
            };
            if !self.prefix_operator_position(i) {
                continue;
            }
            let Some(tail) = self.unary_operand_tail(i) else {
                continue;
            };
            let Some(op_span) = self.operand_token_span(i) else {
                continue;
            };
            let Some((mut ty, probe)) = self.unary_operand_ty(tail) else {
                continue;
            };
            let latched = self.null_aware_members.contains(&probe.offset);
            if latched {
                ty = Self::nullable_form(ty);
            }
            ty = self.alias_expanded_operand_ty(ty);
            if ty == Ty::Null {
                self.emit_operand_null_row(op_span, out);
                continue;
            }
            if !Self::operand_ty_nullable(ty) {
                continue;
            }
            // The resolved lane owns the bare `-x`/`~x` shape through the
            // adjacent-operator arm (the operand row carries no
            // `Intent::Operator` — `mark_operator` runs on binary tails
            // only — so the bare read's own row reports the same `unary-`
            // method face at the same anchor). A member tail (`-c.bad`), a
            // call close (`-c?.e()`), a keyword receiver (`-this`, whose
            // row stays `Intent::Read` and early-returns the receiver
            // arm), and every `?.`-latched operand stay with this pass:
            // the resolved member-adjacent arm answers only the
            // operator's definedness on those shapes, never the
            // `isNullable` consult.
            if !latched && self.bare_operator_operand(probe, false) {
                continue;
            }
            // The `~`/`unary-` resolution consults the extension faces on
            // the operand's named head — a `Single` application resolves
            // the send (the same suppression the binary arm above takes).
            if let Ty::Named { name: head, .. } = ty
                && matches!(
                    self.extension_application(head, ty, self.intern(name), false, true),
                    ExtensionApp::Single
                )
            {
                continue;
            }
            self.emit_operand_unchecked(op_span, name, UncheckedNullableKind::Method, out);
        }
    }

    /// The token index of a binary operator's LHS operand tail: the
    /// non-synthetic token immediately before the operator, gated to the
    /// operand-ending kinds — an identifier, a literal, a `)`/`]` group
    /// close, a keyword receiver (`this`/`super`/`true`/`false`/`null`),
    /// a null-assert `!`, or a postfix `++`/`--`. Anything else means the
    /// token infix-position read is wrong (a `List<int> + x`-style
    /// type-argument close, a leading operator, a keyword boundary), and
    /// the pass under-reports rather than guess.
    fn binary_operand_tail(&self, op_idx: usize) -> Option<usize> {
        let tokens = self.tokens.as_slice();
        let mut tail = op_idx;
        while tail > 0 && tokens[tail - 1].is_synthetic() {
            tail -= 1;
        }
        tail = tail.checked_sub(1)?;
        match tokens[tail].kind {
            dartq_scanner::TokenKind::Identifier
            | dartq_scanner::TokenKind::Int
            | dartq_scanner::TokenKind::Double
            | dartq_scanner::TokenKind::Hexadecimal
            | dartq_scanner::TokenKind::String
            | dartq_scanner::TokenKind::CloseParen
            | dartq_scanner::TokenKind::CloseSquare
            | dartq_scanner::TokenKind::Bang
            | dartq_scanner::TokenKind::PlusPlus
            | dartq_scanner::TokenKind::MinusMinus
            | dartq_scanner::TokenKind::Keyword(
                dartq_scanner::Keyword::This
                    | dartq_scanner::Keyword::Super
                    | dartq_scanner::Keyword::True
                    | dartq_scanner::Keyword::False
                    | dartq_scanner::Keyword::Null,
            ) => Some(tail),
            _ => None,
        }
    }

    /// A prefix `-`/`~` position: the token before the operator does not
    /// end an operand (the `y - x` minus reads binary). The whitelist is
    /// the same operator-boundary set `unary_void_operand_ty` consults —
    /// every other neighborhood (an identifier, literal, `)`, `]`,
    /// keyword receiver) means the token infix-binds.
    fn prefix_operator_position(&self, op_idx: usize) -> bool {
        let tokens = self.tokens.as_slice();
        let mut prior = op_idx;
        while prior > 0 && tokens[prior - 1].is_synthetic() {
            prior -= 1;
        }
        if prior == 0 {
            return true;
        }
        matches!(
            &self.source[tokens[prior - 1].byte_range()],
            "(" | "["
                | "{"
                | ","
                | "="
                | "=="
                | "!="
                | "=>"
                | "return"
                | "case"
                | ";"
                | ":"
                | "?"
                | "&&"
                | "||"
                | "??"
                | "??="
        )
    }

    /// The token index of a prefix operand's tail: the expression after
    /// `-`/`~` minus its trailing binary/statement boundary — the
    /// postfix-selector chain's last token. `None` when the operand is a
    /// shape the walk cannot span (a nested prefix `- -x` re-enters on
    /// the inner operator's own iteration).
    fn unary_operand_tail(&self, op_idx: usize) -> Option<usize> {
        let tokens = self.tokens.as_slice();
        let mut head = op_idx + 1;
        while head < tokens.len() && tokens[head].is_synthetic() {
            head += 1;
        }
        let first = tokens.get(head)?;
        // A nested prefix operand (`- -x`) belongs to the inner token's
        // own window — it re-enters on the inner iteration.
        if matches!(
            first.kind,
            dartq_scanner::TokenKind::Minus
                | dartq_scanner::TokenKind::Tilde
                | dartq_scanner::TokenKind::Bang
                | dartq_scanner::TokenKind::MinusMinus
                | dartq_scanner::TokenKind::PlusPlus
        ) {
            return None;
        }
        let mut tail = head;
        // The operand's head: a group `-(e)` closes at its match (the
        // group face answers there); every other operand starts on a
        // primary token — an identifier, literal, or keyword receiver —
        // whose postfix chain the walk below extends.
        if first.kind == dartq_scanner::TokenKind::OpenParen {
            tail = self.tokens.matching_end(head)?;
        } else if !matches!(
            first.kind,
            dartq_scanner::TokenKind::Identifier
                | dartq_scanner::TokenKind::Int
                | dartq_scanner::TokenKind::Double
                | dartq_scanner::TokenKind::Hexadecimal
                | dartq_scanner::TokenKind::String
                | dartq_scanner::TokenKind::Keyword(
                    dartq_scanner::Keyword::This
                        | dartq_scanner::Keyword::Super
                        | dartq_scanner::Keyword::True
                        | dartq_scanner::Keyword::False
                        | dartq_scanner::Keyword::Null
                )
        ) {
            return None;
        }
        // The postfix walk: `.name`, `?.name`, `!`, `(...)`, `[...]`,
        // `?[...]` (`?` + `[` — the null-aware index lexes split),
        // postfix `++`/`--`. `?..`/conditional `?`/binary boundaries end
        // the operand.
        loop {
            let mut probe = tail + 1;
            while probe < tokens.len() && tokens[probe].is_synthetic() {
                probe += 1;
            }
            let Some(kind) = tokens.get(probe).map(|t| t.kind) else {
                break;
            };
            match kind {
                dartq_scanner::TokenKind::Period | dartq_scanner::TokenKind::QuestionPeriod => {
                    let mut sel = probe + 1;
                    while sel < tokens.len() && tokens[sel].is_synthetic() {
                        sel += 1;
                    }
                    if tokens
                        .get(sel)
                        .is_some_and(|t| matches!(t.kind, dartq_scanner::TokenKind::Identifier))
                    {
                        tail = sel;
                    } else {
                        break;
                    }
                }
                dartq_scanner::TokenKind::Bang => tail = probe,
                dartq_scanner::TokenKind::OpenParen => {
                    tail = self.tokens.matching_end(probe)?;
                }
                dartq_scanner::TokenKind::OpenSquare => {
                    tail = self.tokens.matching_end(probe)?;
                }
                dartq_scanner::TokenKind::Question => {
                    // `?[` — the null-aware index splits into `?` + `[`.
                    let mut bracket = probe + 1;
                    while bracket < tokens.len() && tokens[bracket].is_synthetic() {
                        bracket += 1;
                    }
                    if tokens
                        .get(bracket)
                        .is_some_and(|t| t.kind == dartq_scanner::TokenKind::OpenSquare)
                    {
                        tail = self.tokens.matching_end(bracket)?;
                    } else {
                        break;
                    }
                }
                dartq_scanner::TokenKind::PlusPlus | dartq_scanner::TokenKind::MinusMinus => {
                    tail = probe;
                }
                _ => break,
            }
        }
        Some(tail)
    }

    /// The operand's static type at its tail token: the
    /// receiver-expression walk first (member chains, call results,
    /// groups, index folds), then a `)`-close's callee leg (the
    /// completed-send swap at the callee's own span answers the call's
    /// result — `unary_void_operand_ty`'s same grounding, falling back to
    /// [`Self::callee_result_ty`]'s declaration-level derivation when the
    /// expression walk cannot spell the callee — a bare callee carries no
    /// flow read row and a `?.`-shorted member callee resolves on a
    /// nullable receiver the chain walk declines), then the keyword
    /// receiver's classified face (`this`/`super` inside
    /// `extension on A?`, whose row is gone at teardown —
    /// `resolve_rows` drains the table), then the flow read at the same
    /// span (the bare-read tail `-x`). `None` under-reports.
    ///
    /// The returned [`SourceRange`] is the PROBE the caller consults for
    /// the `null_aware_members` latch — the chain node whose `?.` marks
    /// the whole operand's result nullable: the tail token itself for a
    /// member/index tail, the callee token for a call close (`c?.e()`'s
    /// `?.` marks `e`, never the `)`).
    fn unary_operand_ty(&self, tail: usize) -> Option<(Ty, SourceRange)> {
        let tokens = self.tokens.as_slice();
        let span = self.operand_token_span(tail)?;
        // A call-close tail probes the CALLEE — the token before the
        // matching `(` is the member selector of `recv.m()` (or the bare
        // callee of `f()`), whose own span keys both the send's result
        // (the receiver walk's invoked-receiver swap) and the `?.`
        // latch (`c?.e()` marks `e`, never the `)`).
        if tokens[tail].kind == dartq_scanner::TokenKind::CloseParen {
            // The matching `(` — depth-counted, nested calls included.
            let mut depth = 0usize;
            let mut open = tail;
            while open > 0 {
                open -= 1;
                match tokens[open].kind {
                    dartq_scanner::TokenKind::CloseParen
                    | dartq_scanner::TokenKind::CloseSquare
                    | dartq_scanner::TokenKind::CloseCurly => depth += 1,
                    dartq_scanner::TokenKind::OpenParen
                    | dartq_scanner::TokenKind::OpenSquare
                    | dartq_scanner::TokenKind::OpenCurly => {
                        if depth == 0 {
                            break;
                        }
                        depth -= 1;
                    }
                    _ => {}
                }
            }
            let mut callee = open;
            while callee > 0 && tokens[callee - 1].is_synthetic() {
                callee -= 1;
            }
            if tokens[open].kind == dartq_scanner::TokenKind::OpenParen
                && let Some(callee_idx) = callee.checked_sub(1)
            {
                let callee_span = self.operand_token_span(callee_idx)?;
                if let Some(ty) = self
                    .recv_expression_ty(callee_span)
                    .filter(|ty| *ty != Ty::Unknown)
                    .or_else(|| self.callee_result_ty(callee_span))
                {
                    return Some((ty, callee_span));
                }
            }
            // A GROUP close (`(x)`, `(x?.m)` — the `(` carried no callee):
            // the operand's row-bearing tail is the last token INSIDE the
            // parens, so the probe re-keys there — a bare `(x)` read is the
            // resolved lane's own `Intent::Operator` operand, while
            // `(x?.m)` keeps the member selector the `?.` latch marks.
            // Nested groups descend the same way (`((x))` probes `x`).
            if tokens[open].kind == dartq_scanner::TokenKind::OpenParen
                && let Some(ty) = self
                    .recv_expression_ty(span)
                    .filter(|ty| *ty != Ty::Unknown)
            {
                let mut inner = tail;
                loop {
                    while inner > open && tokens[inner - 1].is_synthetic() {
                        inner -= 1;
                    }
                    if inner <= open {
                        return Some((ty, span));
                    }
                    inner -= 1;
                    if tokens[inner].kind != dartq_scanner::TokenKind::CloseParen {
                        break;
                    }
                    // The inner `)`'s own group: re-derive its `(` and keep
                    // descending (a nested `((x))` re-probes at `x`).
                    let mut depth = 0usize;
                    let mut inner_open = inner;
                    while inner_open > open {
                        inner_open -= 1;
                        match tokens[inner_open].kind {
                            dartq_scanner::TokenKind::CloseParen
                            | dartq_scanner::TokenKind::CloseSquare
                            | dartq_scanner::TokenKind::CloseCurly => depth += 1,
                            dartq_scanner::TokenKind::OpenParen
                            | dartq_scanner::TokenKind::OpenSquare
                            | dartq_scanner::TokenKind::OpenCurly => {
                                if depth == 0 {
                                    break;
                                }
                                depth -= 1;
                            }
                            _ => {}
                        }
                    }
                    if inner_open <= open {
                        return Some((ty, span));
                    }
                    inner = inner_open;
                }
                return self
                    .operand_token_span(inner)
                    .map(|probe| (ty, probe))
                    .or(Some((ty, span)));
            }
        }
        if let Some(ty) = self
            .recv_expression_ty(span)
            .filter(|ty| *ty != Ty::Unknown)
        {
            return Some((ty, span));
        }
        // The keyword receiver's own face: `this` inside a plain
        // `extension` body types the written `on` clause
        // (`extension E on A?` makes `this : A?` —
        // `ExtensionImpl.extendedType`), and `enclosing_container_at`
        // answers which declaration owns the token's offset. `super`
        // never lands here — inside an extension body its context is
        // `InvalidType` upstream (the `SuperContext.of != valid` face the
        // receiver arm already stands down for), and inside a class it is
        // non-nullable, so answering `None` keeps both silent. An unnamed
        // extension's `""` placeholder finds no named face — the
        // under-report posture stands there (the frame-latched rank the
        // resolved arm reads died with the row).
        if tokens[tail].kind
            == dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::This)
            && let Some(container) = self.enclosing_container_at(span.offset)
        {
            let is_extension = self
                .index
                .get(self.file)
                .is_some_and(|elements| {
                    elements
                        .extension_faces()
                        .iter()
                        .any(|face| face.name == Some(container))
                });
            let kind = if is_extension {
                ContainerKind::Extension
            } else {
                ContainerKind::Class
            };
            return Some((self.this_static_ty(container, kind), span));
        }
        self.read_type_at(span)
            .filter(|ty| *ty != Ty::Unknown)
            .map(|ty| (ty, span))
    }

    /// The completed send's RESULT type when [`Self::recv_expression_ty`]
    /// cannot answer the callee span at teardown: the callee's declared
    /// signature is the authoritative result source (upstream's
    /// `staticType.returnType` read), reachable at teardown through the
    /// push-time products that outlive the drained rows — `member_reads`
    /// for a qualified callee (`c?.e()`, `c.e()`) and `refs` for a bare
    /// callee (`f()`). `None` keeps the under-report posture.
    fn callee_result_ty(&self, callee_span: SourceRange) -> Option<Ty> {
        // The qualified callee: the member-read record at the callee token
        // carries the selector name and the receiver span; the receiver's
        // container keys the member walk, and `member_invoke_result` spells
        // the send's result (method/getter return, callable field's result).
        if let Ok(index) = self
            .member_reads
            .binary_search_by(|read| read.offset.cmp(&callee_span.offset))
        {
            let read = &self.member_reads[index];
            let recv_ty = self
                .recv_expression_ty(read.recv_span)
                .filter(|ty| *ty != Ty::Unknown)
                .or_else(|| {
                    self.read_type_at(read.recv_span)
                        .filter(|ty| *ty != Ty::Unknown)
                })
                .or(match &read.recv {
                    Receiver::Typed { ty, .. } => Some(*ty),
                    Receiver::This { container, .. }
                    | Receiver::Super { container, .. }
                    | Receiver::Container { name: container, .. }
                    | Receiver::Created {
                        name: container, ..
                    } => Some(Ty::named(*container)),
                    _ => None,
                });
            if let Some(Ty::Named { name: container, .. }) = recv_ty
                && let Walk::Found(entry) = self.walk_members(container, read.name)
            {
                let ty = self.member_invoke_result(&entry);
                if ty != Ty::Unknown {
                    return Some(ty);
                }
            }
            return None;
        }
        // The bare callee: the push-time `refs` verdict names where the
        // callee resolved — a top-level declaration's return, a member of
        // the enclosing container's return, or a local function literal's
        // signature result.
        let reference = self
            .refs
            .iter()
            .find(|reference| reference.range.offset == callee_span.offset)?;
        match reference.target {
            RefTarget::Decl { .. } => {
                let decl = self.top_level_decl(reference.name)?;
                let ty = match decl.kind {
                    MicroKind::TopLevelVar => {
                        self.callable_result_of(decl.var_ty).unwrap_or(Ty::Unknown)
                    }
                    _ => decl.return_ty,
                };
                (ty != Ty::Unknown).then_some(ty)
            }
            RefTarget::Member { container } => {
                let Walk::Found(entry) = self.walk_members(container, reference.name) else {
                    return None;
                };
                let ty = self.member_invoke_result(&entry);
                (ty != Ty::Unknown).then_some(ty)
            }
            RefTarget::Local => {
                // A local function's signature: the declaration's `(` group
                // grounds `closure_init_ty` (the same arm
                // `receiver_decl_shape` reads), then the callable result.
                let decl_offset = *self.local_decl_at_read.get(&callee_span.offset)?;
                let name_len = self.strings.resolve(reference.name).len() as u32;
                let bytes = self.source.as_bytes();
                let mut at = self.byte_of(decl_offset + name_len);
                while bytes.get(at).is_some_and(|b| b.is_ascii_whitespace()) {
                    at += 1;
                }
                if bytes.get(at) == Some(&b'(') {
                    self.callable_result_of(self.closure_init_ty(bytes, at))
                        .filter(|ty| *ty != Ty::Unknown)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// A token's UTF-16 span (the engine's external position convention).
    fn operand_token_span(&self, idx: usize) -> Option<SourceRange> {
        let token = self.tokens.get(idx)?;
        let end = self.utf16_of(token.byte_offset as usize + token.byte_len as usize);
        Some(SourceRange {
            offset: token.utf16_offset,
            length: end.saturating_sub(token.utf16_offset),
        })
    }

    /// The `isPotentiallyNullable` posture of an operand's static type
    /// (mirrors `operand_face_ty_nullable`'s oracle contract: nullable
    /// named/record/function shapes, `void`, and `Null`; `dynamic` and
    /// `Never` excluded).
    fn operand_ty_nullable(ty: Ty) -> bool {
        match ty {
            Ty::Void | Ty::Null => true,
            Ty::Named { nullable, .. }
            | Ty::Record { nullable, .. }
            | Ty::Function { nullable, .. } => nullable,
            _ => false,
        }
    }

    /// The operand's static type with a type-alias head's right-hand-side
    /// nullability expanded (#3724): an operand the lattice spells by its
    /// written alias head (`IAlias pick()`'s call result, a member whose
    /// declared type the model captured as the alias spelling) carries the
    /// alias's `?` on the right-hand side — invisible at the name level,
    /// so the [`Self::operand_ty_nullable`] consult answered `false` and
    /// the pass under-reported where the oracle emits the unchecked row
    /// (the oracle's `staticType` IS the resolved interface type; the
    /// `isNullable` tail upstream runs on it). The expansion is
    /// [`Engine::fold_alias_ty`] — the same resolved face the verdict
    /// layer unfolds at the ask — gated on the head actually naming an
    /// alias (the bare-head/parameterized-head pair
    /// `bind_declared_type`'s keep-at-bind tests use), so every other
    /// named face keeps the written spelling and its nullability bit.
    fn alias_expanded_operand_ty(&self, ty: Ty) -> Ty {
        if let Ty::Named { name, args, .. } = ty
            && ((args == ArgListId::EMPTY && self.alias_rhs(name).is_some())
                || (args != ArgListId::EMPTY && self.alias_rhs_any(name).is_some()))
        {
            return self.fold_alias_ty(ty);
        }
        ty
    }

    /// Whether an unchecked/null row already stands at `offset` in the
    /// staged batch — either code the pass can emit
    /// (`unchecked_use_of_nullable_value` and the `Null`-operand swap
    /// `invalid_use_of_null_value` — the resolved lane's own `Ty::Null`
    /// arm reports the same anchor). The keyed face is the offset alone:
    /// any code sharing the anchor already speaks for the node (the
    /// oracle emits at most one row per token here). The resolved-send
    /// lane's own rows are deduped through [`Self::bare_operator_operand`]
    /// — `self.report` is already drained when this pass runs
    /// (`resolve_rows` `mem::take`s it before `filter_staged`), so the
    /// diagnostics list can never see them.
    fn operand_row_staged(&self, offset: u32, out: &[Diagnostic]) -> bool {
        const CODES: [&str; 2] = [
            "unchecked_use_of_nullable_value",
            "invalid_use_of_null_value",
        ];
        out.iter()
            .any(|d| d.range.offset == offset && CODES.contains(&d.code.name))
            || self.report.diagnostics.iter().any(|d| {
                d.range.offset == offset && CODES.contains(&d.code.name)
            })
    }

    /// Whether `probe` (the operand's tail-token span) names a bare read
    /// the resolved operator lanes already report on — a local read
    /// (`local_hits`), or a bare name the push-time `refs` verdict
    /// resolved to a local, a member, or a top-level declaration (every
    /// `Intent::Read`-shaped bare row `mark_operator` flips and the
    /// resolved `Intent::Operator`/adjacent arms emit the unchecked row
    /// for). `binary` adds the `this` face: `mark_operator` flips the
    /// `Receiver::This` row on a binary tail (`this + 1` resolves through
    /// `resolve_receiver_row`'s nullable-`on` arm), while a prefix
    /// operand keeps `Intent::Read` and that arm early-returns — `-this`
    /// belongs to this pass. A member tail (`recv.name`) is NOT owned:
    /// the `resolve_member_row` operator arm answers only the operator's
    /// definedness on the member's result face, so this pass holds the
    /// `isNullable` verdict there (`c.bad + 1`, `-c.bad`).
    fn bare_operator_operand(&self, probe: SourceRange, binary: bool) -> bool {
        // A callee token carries `Intent::Invoke`, not `Intent::Read` —
        // `mark_operator` never flips it, so `f() + 1`'s operand belongs
        // to this pass even though `f` itself is a resolved bare name.
        if self
            .call_ends
            .binary_search_by_key(&probe.offset, |(offset, _)| *offset)
            .is_ok()
        {
            return false;
        }
        if self.local_hits.contains_key(&probe.offset) {
            return true;
        }
        // A member tail never reaches this arm — `member_reads` keys the
        // selector's own span, and the `recv.name` walk's probe IS that
        // selector. Excluding it here keeps `c.bad + 1`/`-c.bad` emitting
        // (the resolved member arm never runs the `isNullable` consult).
        if self
            .member_reads
            .binary_search_by(|read| read.offset.cmp(&probe.offset))
            .is_ok()
        {
            return false;
        }
        self.refs
            .binary_search_by(|reference| reference.range.offset.cmp(&probe.offset))
            .ok()
            .is_some_and(|index| {
                match self.refs[index].target {
                    RefTarget::Local
                    | RefTarget::Member { .. }
                    | RefTarget::Decl { .. }
                    | RefTarget::Opaque => true,
                    RefTarget::This => binary,
                    _ => false,
                }
            })
    }

    /// Push the unchecked row into the staged batch, deduped against the
    /// resolved lanes and the earlier operand faces.
    fn emit_operand_unchecked(
        &self,
        range: SourceRange,
        name: &str,
        kind: UncheckedNullableKind,
        out: &mut Vec<Diagnostic>,
    ) {
        if self.in_dead_window(range) || self.operand_row_staged(range.offset, out) {
            return;
        }
        let code = SemanticCode::UncheckedUseOfNullableValue;
        out.push(
            Diagnostic::new(
                code.severity(),
                code.code_info(),
                SemanticCode::unchecked_nullable_message(kind, name),
                self.file,
                range,
                Some(SemanticCode::unchecked_nullable_correction(kind).to_string()),
            )
            .with_context(self.non_promotion_context_at(range.offset)),
        );
    }

    /// The `Null`-operand row: the verifier's `NullType` arm swaps the
    /// reported code to `invalid_use_of_null_value` at the same anchor
    /// (nullable_dereference_verifier.dart:47-49, reached through the
    /// isNullable tail — `Null` is potentially nullable).
    fn emit_operand_null_row(&self, range: SourceRange, out: &mut Vec<Diagnostic>) {
        if self.in_dead_window(range) || self.operand_row_staged(range.offset, out) {
            return;
        }
        let code = SemanticCode::InvalidUseOfNullValue;
        out.push(
            Diagnostic::new(
                code.severity(),
                code.code_info(),
                code.message("", ""),
                self.file,
                range,
                None,
            )
            .with_context(self.non_promotion_context_at(range.offset)),
        );
    }
}
