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
        let raw = self.raw_receiver_env(recv);
        let raw_only = |raw: &[(StringId, Ty)]| {
            let keys: FxHashMap<StringId, Ty> = raw.iter().copied().collect();
            self.fold_alias_ty(self.substitute_ty(declared, &keys))
        };
        let Some((params, seeds, ordered)) = self.closure_callee_params(recv, row.name) else {
            return if raw.is_empty() { declared } else { raw_only(&raw) };
        };
        if params.is_empty() {
            return if raw.is_empty() { declared } else { raw_only(&raw) };
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
            && let Some(offset) = self.rows.get(index).map(|r| r.range.offset)
            && let Some((_, explicit)) = self.call_type_args.iter().find(|(r, _)| *r == offset)
            && explicit.len() == own
        {
            for ((param, _), ty) in params.iter().zip(explicit.iter()) {
                keys.entry(*param).or_insert(*ty);
            }
        }
        for (param, ty) in raw {
            keys.entry(param).or_insert(ty);
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

    /// The instantiate-to-bounds environment of a raw generic receiver
    /// (`Cmd? command` for `class Cmd<T>`): each declared parameter reads
    /// its bound (`dynamic` when unbounded or self-referential), never a
    /// same-named parameter of the enclosing scope. Empty for a written
    /// instantiation or a non-generic head.
    pub(crate) fn raw_receiver_env(&self, recv: &Receiver) -> Vec<(StringId, Ty)> {
        let mut raw: Vec<(StringId, Ty)> = Vec::new();
        // A raw generic receiver (`Cmd? command` for `class Cmd<T>`)
        // instantiates to bounds: an unbounded container parameter reads
        // `dynamic`, never the same-named parameter of the enclosing scope.
        if let Receiver::Typed {
            type_name,
            ty: Ty::Named {
                args: ArgListId::EMPTY,
                ..
            },
        } = *recv
            && let Some((found, _)) = resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                type_name,
            )
        {
            for (index, param) in found.type_param_names.iter().enumerate() {
                let bound = found.type_param_bounds.get(index).copied().unwrap_or(Ty::Dynamic);
                let mut self_referential = false;
                self.boundary.pool().visit_named(bound, &mut |named| {
                    if let Ty::Named { name, .. } = named
                        && found.type_param_names.contains(&name)
                    {
                        self_referential = true;
                    }
                });
                let bound = if self_referential { Ty::Dynamic } else { bound };
                raw.push((*param, bound));
            }
            
        }
        raw
    }
}
