//! The structural-world adapters the extension-application row reads:
//! an extension face's type parameters paired with their declared bounds
//! as the `dartq-types` snapshot sees them.
//!
//! The snapshot's parameter registry (`LibraryTypes::type_params`) is
//! name-keyed, so two faces spelling the same parameter name would share
//! one `ParamFacts.bound` there. Every structural question about a face —
//! applicability (`applicableTo`'s bound gate) and the `_isMoreSpecific`
//! step-5 instantiate-to-bounds projection — therefore carries the face's
//! OWN bounds explicitly, mapped from the name-level `ExtensionFace`
//! columns (`type_param_names` ∥ `type_param_bounds`).

use super::*;

impl<'a> Engine<'a> {
    /// The face's parameters with their declared bounds mapped into the
    /// structural world: `(id, None)` for an unbounded parameter
    /// (`Ty::Dynamic` is the absent bound), `(id, Some(bound))` for a
    /// written one. `None` when no snapshot is mounted, or when a declared
    /// bound the snapshot cannot map leaves the whole structural question
    /// undecided — the name-level arms run their own bound check; a bound
    /// is never silently dropped.
    ///
    /// The bounds project across the face's file boundary FIRST (the
    /// projection interns into the diagnosing pool — a mutable borrow),
    /// then the pool is read for the strict mapping.
    pub(crate) fn strict_face_params(
        &self,
        face: &ExtensionFace,
        face_file: FileId,
    ) -> Option<Vec<(dartq_types::TypeParamId, Option<dartq_types::TypeId>)>> {
        let ops = self.ops?;
        let projected: Vec<(dartq_types::TypeParamId, Option<Ty>)> = face
            .type_param_names
            .iter()
            .enumerate()
            .filter_map(|(index, &name)| {
                let id = ops.param_id(name)?;
                let bound = match face.type_param_bounds.get(index).copied() {
                    None | Some(Ty::Dynamic) => None,
                    Some(bound) => Some(self.boundary.ty(face_file, bound)),
                };
                Some((id, bound))
            })
            .collect();
        let pool = self.boundary.args.borrow();
        let mut params = Vec::with_capacity(projected.len());
        for (id, bound) in projected {
            let bound = match bound {
                None => None,
                Some(bound) => Some(ops.map_ty_strict(bound, &pool)?),
            };
            params.push((id, bound));
        }
        Some(params)
    }
}
