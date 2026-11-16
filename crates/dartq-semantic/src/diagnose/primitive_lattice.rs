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

#![allow(clippy::semicolon_if_nothing_returned)]
use super::*;

impl Engine<'_> {
    /// Whether `name` resolves to a class, mixin or enum declared outside
    /// the SDK — a container the core primitives' closed lattice can never
    /// relate to. The head reads through [`Engine::head_container`]: a
    /// RESOLVED prefixed annotation (`p.Real`) is a real interface type
    /// behind the import prefix, and the bare-scope walk alone would take
    /// the miss for "unknown head" — the under-report arm that let every
    /// primitive-against-prefixed pair (`p.Real x = "s"`, `List<p.Real> l =
    /// <int>[]`, `void f(p.Real r) {} f(42)`) escape its decidable
    /// negative.
    pub(crate) fn declared_outside_core(&self, name: StringId) -> bool {
        let Some((container, owner)) = self.head_container(name) else {
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
