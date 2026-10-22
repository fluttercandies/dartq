//! The core primitives' closed lattice against declared containers: `int`,
//! `double`, `num`, `String` and `bool` are `final`/`sealed` in
//! `dart:core`, so no class, mixin or enum declared outside the SDK sits
//! under any of them, and none of them sits under such a declaration.
//! `Item` is never an `int` and `int` never an `Item` — a decidable
//! negative the name-level primitive table alone cannot give (it only
//! orders the primitives among themselves). An extension type stays
//! undecided (`extension type Id(int i) implements int` IS an `int`), as
//! does a container the SDK itself declares (`dart:core`'s own `int`
//! implementations).

use super::*;

impl Engine<'_> {
    /// Whether `name` resolves to a class, mixin or enum declared outside
    /// the SDK — a container the core primitives' closed lattice can never
    /// relate to.
    pub(crate) fn declared_outside_core(&self, name: StringId) -> bool {
        let Some((container, owner)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        ) else {
            return false;
        };
        if !matches!(
            container.kind,
            ContainerKind::Class | ContainerKind::Mixin | ContainerKind::Enum
        ) {
            return false;
        }
        self.graph
            .library_of_file(owner)
            .is_some_and(|lib| !self.graph.uri(lib).starts_with("dart:"))
    }
}
