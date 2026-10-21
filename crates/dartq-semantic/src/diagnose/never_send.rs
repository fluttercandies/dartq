//! The member send's bottom face: a qualified invocation (`x.m(...)`,
//! `this.m(...)`, `C.m(...)`) whose resolved callee returns `Never` ends
//! the flow state at the send's end (the official expression-level bottom
//! check, `type_analyzer.dart:815-819`, reads the invocation's static
//! type). The send event precedes the dot-access event in the parser's
//! stream, so the send's own exit check (`consume_send`) sees the member
//! row unmerged — no receiver shape, no resolved callee; the merged row
//! the dot-access event folds is where the callee's declared return is
//! first readable, and the exit latches there. A null-aware access
//! (`x?.m(...)`) types `Never?` and is no exit.

use super::*;

impl Engine<'_> {
    /// Whether the just-merged member row (the stack top after
    /// `merge_member`) is an invocation of a `Never`-returning member
    /// resolved through its receiver's static type.
    pub(crate) fn merged_send_returns_never(&self) -> bool {
        let Some(row) = self.rows.last() else {
            return false;
        };
        row.intent == Intent::Invoke
            && matches!(
                row.shape,
                RowShape::Member {
                    recv: Receiver::Typed { .. }
                        | Receiver::This { .. }
                        | Receiver::Super { .. }
                        | Receiver::Container { .. }
                }
            )
            && self.is_never_ty(self.callee_return_ty(row))
    }
}
