//! The receiver a merged member row resolves against, re-derived in pass
//! 2 from the static type of its receiver EXPRESSION: the flow read type
//! at a variable's span (a promotion stands — `t is Sub` makes `t.element`
//! resolve on `Sub`), the settled result of the chain link below (a
//! promoted head re-types every later node — `t.element.bound` reads
//! `Sub.element`'s own declared type, not the frozen base getter's), and
//! a trailing null-assert (`x.f!.m` resolves `m` on `f`'s non-null form).
//! Pass 1 merges against declared, unpromoted faces; this is the single
//! re-derivation every pass-2 receiver verdict reads
//! ([`Engine::recv_expression_ty`] is its expression face).

use super::*;

impl Engine<'_> {
    /// The static type of a receiver expression with its trailing
    /// null-assert applied: a `!` directly after the expression's span
    /// (`e!.m`, never the `!=` operator) promotes the face to its non-null
    /// form (`promoteToNonNull` — the assertion's own static type).
    pub(crate) fn null_asserted_after(&self, span: SourceRange, ty: Ty) -> Ty {
        if span.length == 0 {
            return ty;
        }
        let bang = self.skip_whitespace(span.end());
        if self.byte_at(bang) == Some(b'!') && self.byte_at(bang + 1) != Some(b'=') {
            Self::non_null_form(ty)
        } else {
            ty
        }
    }

    /// The receiver a latched member row re-decides against (#1465's
    /// promoted read, #2283's chained link): the receiver expression's
    /// re-derived static type replaces the push-time classification when
    /// it names a different container (a promotion, or the chain's settled
    /// result) or narrows a nullable face to non-null (a promotion to the
    /// non-null form, a null-assert). An equal face, a face differing only
    /// in its type arguments (the merge's solved instantiation is the
    /// better record), and a widening face (the pre-assert flow state of
    /// `x!.m`, #2161 — promotion never widens) keep the record byte for
    /// byte. Unlatched rows — cascade sections, invoked callees — never
    /// re-decide.
    pub(crate) fn rederived_receiver(&self, member_offset: u32, recv: Receiver) -> Receiver {
        let Receiver::Typed { ty: static_ty, .. } = recv else {
            return recv;
        };
        if self
            .promoted_recv_reads
            .binary_search_by(|(member, _)| member.cmp(&member_offset))
            .is_err()
        {
            return recv;
        }
        let Ok(index) = self
            .member_reads
            .binary_search_by(|read| read.offset.cmp(&member_offset))
        else {
            return recv;
        };
        let read_span = self.member_reads[index].recv_span;
        let Some(face @ Ty::Named { name, nullable, .. }) = self.recv_expression_ty(read_span)
        else {
            return recv;
        };
        let (static_head, static_nullable) = match static_ty {
            Ty::Named { name, nullable, .. } => (Some(name), nullable),
            _ => (None, false),
        };
        let renames = static_head != Some(name);
        let narrows = static_nullable && !nullable;
        if !(renames || narrows) {
            return recv;
        }
        Receiver::Typed {
            type_name: name,
            ty: face,
        }
    }
}
