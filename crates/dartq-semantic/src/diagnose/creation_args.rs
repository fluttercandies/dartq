//! The explicit-argument creation's static type (`var set =
//! SplayTreeSet<E>(comparison)`): the folded `Created` row carries the
//! bare head, and the written `<E>` rides `call_type_args` keyed on the
//! callee row — the inferred local's type is the instantiated head, so
//! `return set;` satisfies a `Set<E>` return through the real
//! `SplayTreeSet<E> implements Set<E>` edge instead of the raw head's
//! to-bounds instantiation. The assignment staging reads the same slot
//! for `x = C<int>()`.

use super::*;

impl Engine<'_> {
    /// The created head's instantiated static type at `offset`: the written
    /// explicit type arguments (`C<int>()`'s `<int>`, keyed on the callee
    /// row) applied to the bare fold-sentinel head, or `None` when the row
    /// registered no written arguments.
    pub(crate) fn creation_row_ty(
        &self,
        name: StringId,
        nullable: bool,
        offset: u32,
    ) -> Option<Ty> {
        let (_, tys) = self.call_type_args.iter().find(|(r, _)| *r == offset)?;
        Some(self.boundary.args.borrow_mut().named(name, nullable, tys))
    }

    /// `ty` with the creation row's explicit type arguments applied when
    /// `ty` is the bare head of a `Created` receiver at `row`; every other
    /// shape returns unchanged.
    pub(crate) fn with_explicit_creation_args(&self, row: usize, ty: Ty) -> Ty {
        let Ty::Named {
            name,
            nullable,
            args,
        } = ty
        else {
            return ty;
        };
        if args != crate::model::ArgListId::EMPTY {
            return ty;
        }
        if !matches!(
            self.rows.get(row).map(|r| &r.shape),
            Some(RowShape::Receiver(Receiver::Created { .. }))
        ) {
            return ty;
        }
        let offset = self.rows.get(row).map_or(u32::MAX, |r| r.range.offset);
        self.creation_row_ty(name, nullable, offset).unwrap_or(ty)
    }
}
