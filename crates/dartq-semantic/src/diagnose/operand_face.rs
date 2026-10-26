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
}
