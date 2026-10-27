//! The if-case pattern scope on the flat frame: a pattern variable
//! declared in `if (e case P)` is in scope in the guard and the then
//! statement only (the official `IfStatement`'s case-clause scope), so a
//! spelling it shadows reads its outer declaration again in the else
//! branch and after the statement (`if (p case Sub p) ... else p.other`
//! resolves the else's `p` on the OUTER `p`'s type). The frame binds
//! names flat; this restores what the condition's patterns overwrote.

use super::*;

impl Engine<'_> {
    /// A pattern variable is about to bind `name`: remember the frame's
    /// current type for the spelling (the shadowed declaration, if any).
    pub(crate) fn note_pattern_shadow(&mut self, name: StringId) {
        let (prev, was_local) = self
            .frames
            .last()
            .map_or((None, false), |frame| {
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
            });
        }
    }

    /// One case head completed: another `begin_case_expression` belongs
    /// to the NEXT member of the same switch.
    pub(crate) fn switch_member_head_done(&mut self) {
        if let Some(top) = self.switch_arm_scopes.last_mut() {
            top.head_done = true;
        }
    }

    /// A switch member closes (`end_switch_case`/one switch-expression
    /// arm): its binders go out of scope and the shadowed spellings read
    /// their outer types again. Only the member of the closing switch
    /// pops — an outer member stays open behind a nested switch.
    pub(crate) fn switch_member_end(&mut self) {
        let depth = self.switch.switches.len();
        if self
            .switch_arm_scopes
            .last()
            .is_some_and(|top| top.depth == depth)
        {
            let member = self.switch_arm_scopes.pop().expect("checked");
            let shadows = member.shadows;
            self.restore_var_types(&shadows);
        }
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
        self.if_case_scopes.push(shadows);
        let binders = std::mem::take(&mut self.pattern_shadow_binders);
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
