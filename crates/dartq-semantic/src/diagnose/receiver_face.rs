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
        // A flow face spelling the prefixed static head's bare leaf names
        // the same element: keep the qualified head (see
        // [`Self::flow_leaf_of`]), narrowing only its nullability.
        if let Some(head) = static_head
            && self.flow_leaf_of(head, name)
        {
            return if static_nullable && !nullable {
                Receiver::Typed {
                    type_name: head,
                    ty: Self::non_null_form(static_ty),
                }
            } else {
                recv
            };
        }
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

    /// Whether a flow-layer type head is a prefixed static head's bare
    /// leaf (`File` against `io.File`). The flow recorder keys every
    /// qualified type reference by its last identifier, so a promoted or
    /// re-read face over an `ast.C` local spells `C` — the same element.
    /// Re-resolving that leaf in the diagnosing scope would retarget a
    /// same-spelled unprefixed import's class.
    pub(crate) fn flow_leaf_of(&self, static_head: StringId, flow_head: StringId) -> bool {
        static_head != flow_head
            && self
                .strings
                .resolve(static_head)
                .rsplit_once('.')
                .is_some_and(|(_, leaf)| leaf == self.strings.resolve(flow_head))
    }

    /// The re-bound local face (#2539): a receiver read whose identifier
    /// names a promotion-latched initializer's local carries the re-bound
    /// accessor return as its re-derived static type — the flow read face
    /// still spells the parse-time bind (the staging predates the pass-2
    /// promotion re-decision), so the re-bind map is the only holder of
    /// the promoted accessor face.
    pub(crate) fn rebound_local_receiver(&self, member_offset: u32, recv: &Receiver) -> Option<Ty> {
        let Receiver::Typed { ty: static_ty, .. } = recv else {
            return None;
        };
        let ty = self.rebound_local_ty(member_offset)?;
        (ty != *static_ty).then_some(ty)
    }

    /// [`Self::rebound_local_face`]'s raw lookup: the re-bound type of
    /// whichever local the read span's identifier names, without the
    /// differs-from-static guard — the re-bind is authoritative for its
    /// own reads.
    pub(crate) fn rebound_local_ty(&self, member_offset: u32) -> Option<Ty> {
        // The identity gate: the declaring local's NAME-TOKEN offset keys
        // the re-bind map (a file-unique identity — the scoped
        // locals-stack index aliases across scopes once the frames close,
        // and a flat index key let one scope's re-bind clobber another's).
        // The receiver read's own span keys the non-moving roster; the
        // fold records its twin at the member row's offset for the
        // shapes whose recv span keys nothing (the entry can name an
        // outer declarator — `var n = c.m().k()` moves n's own hit onto
        // c's member row). The flow face gate (the consumer read's own
        // face still spelling the parse-time bind) keeps a foreign
        // re-bind from retyping an unrelated read.
        let decl = self
            .member_reads
            .binary_search_by(|read| read.offset.cmp(&member_offset))
            .ok()
            .and_then(|i| {
                let span = self.member_reads[i].recv_span;
                // The member-keyed twin names the chain's HEAD local, so it
                // only stands for a receiver that is not itself a member
                // link (`ce.fragments.firstWhere` must not re-type
                // `fragments`' result with `ce`'s face).
                self.local_decl_at_read
                    .get(&span.offset)
                    .copied()
                    .or_else(|| {
                        self.chain_node_read(span, true)
                            .is_none()
                            .then(|| self.local_decl_at_member.get(&member_offset).copied())
                            .flatten()
                    })
            })?;
        let &(parse_ty, corrected) = self.rebound_locals.get(&decl)?;
        let recv_span = self
            .member_reads
            .binary_search_by(|read| read.offset.cmp(&member_offset))
            .ok()
            .map(|i| self.member_reads[i].recv_span)?;
        // A flow face past the parse bind (a promotion, a widened join)
        // keeps itself; no flow row at all means nothing stands between
        // the read and the re-bind — an implicitly typed closure formal
        // stages no flow row of its own, and the correction is
        // authoritative there.
        match self.read_type_at(recv_span) {
            Some(ty) if ty != parse_ty => None,
            _ => Some(corrected),
        }
    }

    /// [`Self::rebound_local_ty`]'s span-keyed face: the re-bound type of
    /// whichever local the read span names, gated on the flow read still
    /// spelling the parse-time bind. This is the layering every
    /// receiver-expression face reads BEFORE the raw flow type: the flow
    /// layer's declared slot staged the parse-time face (#2140) and no
    /// promotion rewrites it, so a promotion-latched initializer's
    /// pass-2 re-bind (#2539) is the only holder of the corrected face —
    /// `var a = f.element; a.m` re-types `a`'s receiver reads with the
    /// re-bound face, while a genuinely promoted read (a flow face past
    /// the parse bind) keeps its flow face through the gate.
    pub(crate) fn rebound_read_face(&self, span: SourceRange) -> Option<Ty> {
        let decl = self.local_decl_at_read.get(&span.offset).copied()?;
        let &(parse_ty, corrected) = self.rebound_locals.get(&decl)?;
        match self.read_type_at(span) {
            Some(ty) if ty != parse_ty => None,
            _ => Some(corrected),
        }
    }
}
