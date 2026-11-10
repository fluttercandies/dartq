//! The if-case pattern scope on the flat frame: a pattern variable
//! declared in `if (e case P)` is in scope in the guard and the then
//! statement only (the official `IfStatement`'s case-clause scope), so a
//! spelling it shadows reads its outer declaration again in the else
//! branch and after the statement (`if (p case Sub p) ... else p.other`
//! resolves the else's `p` on the OUTER `p`'s type). The frame binds
//! names flat; this restores what the condition's patterns overwrote.
//!
//! A switch member's binders close the same way, at the member's end (the
//! official scopes a case pattern's variables to its arm), and a binder
//! that shadowed NOTHING has nothing to restore — its `Frame::locals`
//! membership simply outlives the scope it was declared in. Both halves
//! therefore also stamp [`LocalDecl::retired`] on the rows the closing
//! scope declared, which is what [`Engine::resolve_bare`] reads to stop
//! answering a post-scope read as a local.

use super::*;
use dartq_scanner::{Keyword, TokenKind};

/// The arm-scope sentinel band of [`LocalDecl::group`] — `u32::MAX - depth`
/// for a case header's pattern variable (`super::scopes_bodies`'s
/// `PATTERN_ARM_SCOPE_BASE`, kept in lockstep). A row inside the band
/// belongs to the switch member that declared it, never to an enclosing
/// `if` case scope it merely shares a spelling with.
fn arm_scoped(group: u32) -> bool {
    group >= u32::MAX - u16::MAX as u32
}

impl Engine<'_> {
    /// A pattern variable is about to bind `name`: remember the frame's
    /// current type for the spelling (the shadowed declaration, if any).
    pub(crate) fn note_pattern_shadow(&mut self, name: StringId) {
        let (prev, was_local) = self.frames.last().map_or((None, false), |frame| {
            (
                frame.var_types.get(&name).copied(),
                frame.locals.contains(&name),
            )
        });
        self.pattern_shadows.push((name, prev, was_local));
        // An open switch member records the same pair in its own scope:
        // the binder shadows only inside its member. A member whose head
        // never fired (`default:` bodies, switch-expression arms) opens
        // lazily here — the recording IS the first evidence the member
        // is taking binders.
        let depth = self.switch.switches.len();
        if self
            .switch_arm_scopes
            .last()
            .is_none_or(|top| top.depth != depth)
        {
            self.switch_arm_scopes.push(SwitchArmScope {
                depth,
                head_done: false,
                shadows: Vec::new(),
                head_mark: self.locals.len(),
                head_rows_mark: self.rows.len(),
                head_vars: Vec::new(),
                head_offset: self.clause_anchor.map_or(u32::MAX, |anchor| anchor.offset),
                member_body: false,
                member_ended: false,
                is_default: false,
            });
        }
        if let Some(top) = self.switch_arm_scopes.last_mut() {
            if !top.shadows.iter().any(|(n, _, _)| *n == name) {
                top.shadows.push((name, prev, was_local));
            }
        }
    }

    /// A case head opens: a fresh member of the innermost open switch,
    /// unless the top scope is this switch's member mid-head-group
    /// (shared-body `case A: case B:` heads run back to back).
    pub(crate) fn switch_member_head_begin(&mut self) {
        let depth = self.switch.switches.len();
        if self
            .switch_arm_scopes
            .last()
            .is_none_or(|top| top.depth != depth || top.head_done)
        {
            self.switch_arm_scopes.push(SwitchArmScope {
                depth,
                head_done: false,
                shadows: Vec::new(),
                head_mark: self.locals.len(),
                head_rows_mark: self.rows.len(),
                head_vars: Vec::new(),
                head_offset: self.clause_anchor.map_or(u32::MAX, |anchor| anchor.offset),
                member_body: false,
                member_ended: false,
                is_default: false,
            });
        }
    }

    /// One case head completed: another `begin_case_expression` belongs
    /// to the NEXT member of the same switch. The head's pattern
    /// variables freeze into the shared-scope join set — the `(name,
    /// declared type, is-final)` triples of the `locals` rows the head
    /// opened over (the official `_SharedCaseScope.addAll` input).
    pub(crate) fn switch_member_head_done(&mut self) {
        let head_rows = self.rows.len();
        // The clause anchor latches AFTER `switch_member_head_begin` (the
        // listener sets it once `case_kw` parses), so the head's keyword
        // offset lands here — `u32::MAX` at begin is only a placeholder.
        let head_offset = self.clause_anchor.map_or(u32::MAX, |anchor| anchor.offset);
        let mark = {
            let Some(top) = self.switch_arm_scopes.last_mut() else {
                return;
            };
            top.head_done = true;
            top.head_rows_mark = head_rows;
            if head_offset != u32::MAX {
                top.head_offset = head_offset;
            }
            top.head_mark
        };
        // `head_mark` is the locals watermark the head opened over; the
        // rows it added are this head's declared set (the official
        // `_SharedCaseScope.addAll` input triples).
        let vars: Vec<(StringId, Option<Ty>, bool)> = self.locals[mark.min(self.locals.len())..]
            .iter()
            .filter(|decl| matches!(decl.kind, LocalKind::PatternVariable))
            .map(|decl| {
                (
                    decl.name,
                    self.local_declared_ty(decl.name),
                    self.pattern_var_is_final(decl.range.offset),
                )
            })
            .collect();
        if let Some(top) = self.switch_arm_scopes.last_mut() {
            top.head_vars = vars;
        }
    }

    /// A switch member closes (`end_switch_case`/one switch-expression
    /// arm): its binders go out of scope and the shadowed spellings read
    /// their outer types again. Only the member of the closing switch
    /// closes — an outer member stays open behind a nested switch.
    ///
    /// The scope stays ON the stack (#3519): the shared-case-scope join
    /// reads the trailing run of one depth's scopes as the member group
    /// feeding the next body, and `member_body` marks that group's
    /// boundary — the member carried statements when `rows` grew past
    /// the head-done watermark (`case A: case B:` empty heads never do).
    pub(crate) fn switch_member_end(&mut self) {
        let depth = self.switch.switches.len();
        if self
            .switch_arm_scopes
            .last()
            .is_some_and(|top| top.depth == depth)
        {
            // A `default:` member opens no case head, so no scope of its
            // own: the previous member's scope is still on top with its
            // `member_ended` already latched. Its statements belong to the
            // default member — mark them on a synthetic head (the official
            // `switchStatementSharedCaseScopeEmpty` join) instead of
            // corrupting the case scope's `member_body` group boundary.
            if self
                .switch_arm_scopes
                .last()
                .is_some_and(|top| top.member_ended)
            {
                self.switch_arm_scopes.push(SwitchArmScope {
                    depth,
                    head_done: true,
                    shadows: Vec::new(),
                    head_mark: self.locals.len(),
                    head_rows_mark: self.rows.len(),
                    head_vars: Vec::new(),
                    head_offset: self.clause_anchor.map_or(u32::MAX, |anchor| anchor.offset),
                    member_body: true,
                    member_ended: true,
                    is_default: true,
                });
                return;
            }
            let shadows = {
                let top = self.switch_arm_scopes.last_mut().expect("checked");
                top.member_body = self.rows.len() > top.head_rows_mark;
                top.member_ended = true;
                std::mem::take(&mut top.shadows)
            };
            self.restore_var_types(&shadows);
            // The binders retire with the member: the sentinel group names
            // this switch's stack slot exactly, so a later sibling arm's
            // read of the spelling walks past the row (the arm is out of
            // scope) while a nested switch's deeper slot and an if-case's
            // group-free rows keep their own closes.
            if !shadows.is_empty() && depth > 0 && depth <= u16::MAX as usize {
                let sentinel = u32::MAX - depth as u32;
                let lo = self.frame_local_marks.last().copied().unwrap_or(0);
                if let Some(rows) = self.locals.get_mut(lo..) {
                    for decl in rows {
                        if !decl.retired && decl.group == Some(sentinel) {
                            decl.retired = true;
                        }
                    }
                }
            }
        }
    }

    /// The `final` bit of a pattern variable's spelling: a `final`
    /// binder reads `isFinal=true` upstream, `var` and unmarked binders
    /// `false` — the shared-scope `differentFinalityOrType` half of the
    /// join. A backward scan over the written type spelling finds the
    /// keyword or bounds the head.
    fn pattern_var_is_final(&self, name_offset: u32) -> bool {
        let toks = self.tokens.as_slice();
        let mut i = toks.partition_point(|t| t.utf16_offset < name_offset);
        let mut steps = 0u8;
        while i > 0 && steps < 12 {
            i -= 1;
            steps += 1;
            match toks[i].kind {
                TokenKind::Keyword(Keyword::Final) => return true,
                TokenKind::Keyword(Keyword::Var) => return false,
                TokenKind::Identifier
                | TokenKind::Keyword(_)
                | TokenKind::Question
                | TokenKind::Period
                | TokenKind::Gt
                | TokenKind::Lt
                | TokenKind::GtGt
                | TokenKind::Comma => {}
                _ => return false,
            }
        }
        false
    }

    /// An `if` condition opens: only patterns inside it belong to the
    /// statement's case scope.
    pub(crate) fn if_condition_begin(&mut self) {
        self.pattern_shadows.clear();
        self.pattern_shadow_binders.clear();
    }

    /// A pattern binder just joined `locals` at `index` after
    /// [`Self::note_pattern_shadow`] found its spelling already tracked:
    /// the if-case scope retires it at close.
    pub(crate) fn note_pattern_binder(&mut self, index: usize, anchor: u32) {
        self.pattern_shadow_binders.push((index, anchor));
    }

    /// The then statement opens: the condition's pattern shadows become
    /// the statement's case scope.
    pub(crate) fn if_then_begin(&mut self) {
        let shadows = std::mem::take(&mut self.pattern_shadows);
        let mut binders = std::mem::take(&mut self.pattern_shadow_binders);
        // EVERY case binder retires at the close, not only the one that
        // shadowed an outer tracked local: a binder that shadows nothing
        // has no type to restore, and its `Frame::locals` membership would
        // otherwise outlive the case scope for the rest of the executable.
        // The condition's own rows are the rows to stamp — a case binder
        // carries no group, a logical-or spelling pair an odd site group
        // (`join_pattern_site_binder`). Every other same-name row is an
        // outer declaration that must stay live: a pattern-declaration
        // window's even group, an open switch member's sentinel band.
        let lo = self.frame_local_marks.last().copied().unwrap_or(0);
        if !shadows.is_empty()
            && let Some(rows) = self.locals.get(lo..)
        {
            let mut condition_binders = Vec::new();
            for (index, decl) in rows.iter().enumerate() {
                let index = lo + index;
                if !matches!(decl.kind, LocalKind::PatternVariable)
                    || !shadows.iter().any(|(name, ..)| *name == decl.name)
                    || decl
                        .group
                        .is_some_and(|group| group % 2 == 0 || arm_scoped(group))
                    || binders.iter().any(|&(known, _)| known == index)
                {
                    continue;
                }
                condition_binders.push((index, decl.range.offset));
            }
            binders.append(&mut condition_binders);
        }
        self.if_case_scopes.push(shadows);
        self.if_case_binders.push(binders);
    }

    /// The then statement ends (an else follows): the case scope closes.
    pub(crate) fn if_then_end(&mut self) {
        if let Some(top) = self.if_case_scopes.last_mut() {
            let shadows = std::mem::take(top);
            self.restore_var_types(&shadows);
        }
        if let Some(top) = self.if_case_binders.last_mut() {
            let binders = std::mem::take(top);
            self.retire_binders(&binders);
        }
    }

    /// The whole statement ends: a still-open case scope (no else) closes.
    pub(crate) fn if_statement_end(&mut self) {
        if let Some(shadows) = self.if_case_scopes.pop() {
            self.restore_var_types(&shadows);
        }
        if let Some(binders) = self.if_case_binders.pop() {
            self.retire_binders(&binders);
        }
    }

    /// Retires closed-scope binders still standing in `locals` (a frame pop
    /// may already have truncated them — the index then names a later
    /// entry, so the spelling and anchor must still match).
    fn retire_binders(&mut self, binders: &[(usize, u32)]) {
        for &(index, anchor) in binders {
            if let Some(decl) = self.locals.get_mut(index)
                && decl.range.offset == anchor
            {
                decl.retired = true;
            }
        }
    }

    fn restore_var_types(&mut self, shadows: &[(StringId, Option<Ty>, bool)]) {
        let Some(frame) = self.frames.last_mut() else {
            return;
        };
        for (name, prev, was_local) in shadows.iter().rev() {
            match prev {
                Some(ty) => {
                    frame.var_types.insert(*name, *ty);
                }
                None => {
                    frame.var_types.remove(name);
                }
            }
            // The flat locals set is untouched: `add_tracked_local` fires
            // AFTER the shadow record (was_local=false), so a remove here
            // would kill the binder's own legitimate entry — the R53
            // undefined-identifier regressions. The recorded membership is
            // kept for the scoped-locals follow-up (the F4 completion
            // needs a locals push/pop paired with this restore, gated on
            // evidence).
            let _ = was_local;
        }
    }
}
