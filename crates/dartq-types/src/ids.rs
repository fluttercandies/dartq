//! Opaque identifiers allocated by the type table and hierarchy snapshots.

use core::fmt;

/// Identity of an interned type inside a [`crate::TypeTable`].
///
/// `Eq`/`Hash` on this newtype *is* structural type equality: the table only
/// hands out ids through canonical interning, so two ids are equal exactly
/// when the types are structurally equal. This turns the analyzer's
/// structural `==` (`type.dart:876-889`, `type.dart:1589`) into an O(1) id
/// comparison without changing the semantics.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TypeId(u32);

impl TypeId {
    /// Wraps a raw index. Only useful to persist ids across snapshots.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// Returns the raw index.
    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

impl fmt::Display for TypeDeclId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// Identity of a class/mixin declaration whose instances are
/// [`crate::TypeKind::Interface`].
///
/// Allocated by the layers above (resolution/semantic); this crate treats it
/// as an opaque handle and resolves declarations only through an injected
/// [`crate::TypeHierarchy`] snapshot (R5: no reverse dependency on
/// the semantic layer).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TypeDeclId(u32);

impl TypeDeclId {
    /// Wraps a raw index.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// Returns the raw index.
    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

/// Identity of a declared type parameter, referenced by
/// [`TypeKind::TypeParameter`](crate::TypeKind::TypeParameter).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TypeParamId(u32);

impl TypeParamId {
    /// Wraps a raw index.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// Returns the raw index.
    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

impl fmt::Display for TypeParamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// Identity of a pooled function signature, referenced by
/// [`TypeKind::Function`](crate::TypeKind::Function).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FunctionSigId(u32);

impl FunctionSigId {
    /// Wraps a raw index.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// Returns the raw index.
    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

/// Identity of a pooled record shape, referenced by
/// [`TypeKind::Record`](crate::TypeKind::Record).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RecordShapeId(u32);

impl RecordShapeId {
    /// Wraps a raw index.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// Returns the raw index.
    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }
}
