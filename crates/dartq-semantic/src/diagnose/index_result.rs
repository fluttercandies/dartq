//! The index read's static type (`c[i]`): the receiver's `[]` operator
//! declared return with the receiver's written arguments substituted for
//! the declaring container's parameters — `commands[name]` over a
//! `Map<String, Cmd>` reads `Cmd?`, so the trailing-`!` write
//! `command = commands[name]!` stages `Cmd` into the flow and the local
//! promotes (the official `IndexExpression.staticType`). Only a `[]` the
//! receiver's own container declares answers: an inherited operator's
//! environment composes through the clause hops the name-level model
//! strips, and that face stays the opaque value row (the under-report
//! direction). A raw receiver reads the top instantiation.

use super::*;

impl Engine<'_> {
    /// The `[]` read's static type on `receiver`, or `Unknown` when the
    /// receiver's own container declares no `[]` or the receiver is not a
    /// named type.
    pub(crate) fn index_result_ty(&self, receiver: Ty) -> Ty {
        let Ty::Named { name, args, .. } = receiver else {
            return Ty::UNKNOWN;
        };
        let Some((container, owner)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        ) else {
            return Ty::UNKNOWN;
        };
        let Some(entry) = container.members.get(&self.intern("[]")) else {
            return Ty::UNKNOWN;
        };
        if entry.kind != MemberKind::Method {
            return Ty::UNKNOWN;
        }
        let declared = self.boundary.ty(owner, entry.return_ty);
        let params = &container.type_param_names;
        if params.is_empty() {
            return self.fold_alias_ty(declared);
        }
        let written: Vec<Ty> = self.boundary.pool().get(args).to_vec();
        let keys: FxHashMap<StringId, Ty> = if written.is_empty() {
            params.iter().map(|&param| (param, Ty::Dynamic)).collect()
        } else if written.len() == params.len() {
            params.iter().copied().zip(written).collect()
        } else {
            return Ty::UNKNOWN;
        };
        self.fold_alias_ty(self.substitute_ty(declared, &keys))
    }
}
