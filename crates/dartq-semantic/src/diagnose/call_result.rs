//! The static type of a completed send: the resolved callee's declared
//! return instantiated for the call site. The declaring container's
//! parameters read the receiver's written arguments (`List<int>.first()`
//! answers `int`, the same environment the `[]` read composes), the
//! callee's own parameters read the explicit type arguments (`f<int>()`),
//! and a parameter the site leaves free never escapes as a caller-scope
//! type: `var n = min(a, b)` answers `Unknown` here, never the foreign
//! spelling `T` — which the binary fold would then refine to `double`
//! and every `int` sink report. The oracle infers `int` from the
//! arguments; until the call-site inference solves it, the under-report
//! direction is the only sound stand-in.

use super::*;

impl Engine<'_> {
    /// `declared` with this send's environment applied; `Unknown` when a
    /// callee parameter stays free.
    pub(crate) fn instantiate_call_return(&self, row: &Row, recv: &Receiver, declared: Ty) -> Ty {
        if declared == Ty::UNKNOWN {
            return declared;
        }
        let Some((params, seeds, ordered)) = self.closure_callee_params(recv, row.name) else {
            return declared;
        };
        if params.is_empty() {
            return declared;
        }
        let mut keys: FxHashMap<StringId, Ty> = seeds.into_iter().collect();
        // The send-close upward inference's solution joins after the
        // fixing slots (explicit type arguments, receiver seeds): the
        // oracle infers what the call site leaves free, and the stashed
        // bindings are exactly that face — `math.max(0, n - 1)` answers
        // `int` instead of the free-parameter `Unknown` stand-in.
        if let Some(solved) = self.solved_call_params.get(&row.range.offset) {
            for (param, ty) in solved {
                keys.entry(*param).or_insert(*ty);
            }
        }
        // The explicit type-argument list pairs against the declaration's
        // own parameter order; a recovered (unordered) spelling set pairs
        // against nothing.
        if let Some(own) = ordered
            && let Some(index) = self
                .rows
                .iter()
                .position(|probe| probe.range == row.range && probe.name == row.name)
            && let Some((_, explicit)) = self.call_type_args.iter().find(|(r, _)| *r == index)
            && explicit.len() == own
        {
            for ((param, _), ty) in params.iter().zip(explicit.iter()) {
                keys.entry(*param).or_insert(*ty);
            }
        }
        let out = if keys.is_empty() {
            declared
        } else {
            self.substitute_ty(declared, &keys)
        };
        // A parameter still spelled in the result is free — unless the
        // spelling is a type parameter in scope at the call (`E` inside
        // `class List<E>` calling its own `first()`), which the result
        // legitimately names.
        let mut free = false;
        self.boundary.pool().visit_named(out, &mut |named| {
            if let Ty::Named { name, .. } = named
                && params.iter().any(|(param, _)| *param == name)
                && !self.type_params.contains(&name)
                && !self.pending_type_params.contains(&name)
            {
                free = true;
            }
        });
        if free {
            return Ty::UNKNOWN;
        }
        self.fold_alias_ty(out)
    }
}
