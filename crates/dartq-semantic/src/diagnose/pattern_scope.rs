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
        let prev = self
            .frames
            .last()
            .and_then(|frame| frame.var_types.get(&name).copied());
        self.pattern_shadows.push((name, prev));
    }

    /// An `if` condition opens: only patterns inside it belong to the
    /// statement's case scope.
    pub(crate) fn if_condition_begin(&mut self) {
        self.pattern_shadows.clear();
    }

    /// The then statement opens: the condition's pattern shadows become
    /// the statement's case scope.
    pub(crate) fn if_then_begin(&mut self) {
        let shadows = std::mem::take(&mut self.pattern_shadows);
        self.if_case_scopes.push(shadows);
    }

    /// The then statement ends (an else follows): the case scope closes.
    pub(crate) fn if_then_end(&mut self) {
        if let Some(top) = self.if_case_scopes.last_mut() {
            let shadows = std::mem::take(top);
            self.restore_var_types(&shadows);
        }
    }

    /// The whole statement ends: a still-open case scope (no else) closes.
    pub(crate) fn if_statement_end(&mut self) {
        if let Some(shadows) = self.if_case_scopes.pop() {
            self.restore_var_types(&shadows);
        }
    }

    fn restore_var_types(&mut self, shadows: &[(StringId, Option<Ty>)]) {
        let Some(frame) = self.frames.last_mut() else {
            return;
        };
        for (name, prev) in shadows.iter().rev() {
            match prev {
                Some(ty) => {
                    frame.var_types.insert(*name, *ty);
                }
                None => {
                    frame.var_types.remove(name);
                }
            }
        }
    }
}
