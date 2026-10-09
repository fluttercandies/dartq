//! The type skeleton: ten kinds plus the nullability dimension.
//!
//! Mirrors the analyzer's concrete type inventory (spec §1.1, all anchors in
//! `pkg/analyzer/lib/src/dart/element/type.dart`):
//!
//! | dartq kind | analyzer class | anchor |
//! |---|---|---|
//! | [`TypeKind::Never`] | `NeverTypeImpl` | type.dart:1190 |
//! | [`TypeKind::Dynamic`] | `DynamicTypeImpl` | type.dart:33 |
//! | [`TypeKind::Void`] | `VoidTypeImpl` | type.dart:1762 |
//! | [`TypeKind::Invalid`] | `InvalidTypeImpl` | type.dart:1136 |
//! | [`TypeKind::Null`] | `NullTypeImpl` | type.dart:1276 |
//! | [`TypeKind::Interface`] | `InterfaceTypeImpl` | type.dart:575 |
//! | [`TypeKind::TypeParameter`] | `TypeParameterTypeImpl` | type.dart:1631 |
//! | [`TypeKind::Function`] | `FunctionTypeImpl` | type.dart:100 |
//! | [`TypeKind::FutureOr`] | `FutureOrTypeImpl` | type.dart:518 |
//! | [`TypeKind::Record`] | `RecordTypeImpl` | type.dart:1296 |
//!
//! Nullability is a dimension carried by every type, not an independent
//! variant: `T?` is `T` with the question projection (`withNullability`,
//! type.dart:1558/1567-1569). The unit kinds normalize their nullability away
//! at interning time (spec §4.2): `dynamic` is always stored non-`?`
//! (type.dart:90-92), and `Never?` is interned as `Null` (official `Never?`
//! behaves as `Null`, type.dart:1207-1209).

use crate::ids::{FunctionSigId, RecordShapeId, TypeDeclId, TypeId, TypeParamId};
use crate::suffix::NullabilitySuffix;

/// The kind of a type: one of the ten analyzer shapes.
///
/// Bulk data (type arguments, parameters, signatures) lives in
/// [`crate::TypeTable`] side tables; the kind carries only an opaque range or
/// id so that the whole kind fits in 16 bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TypeKind {
    /// `Never` — the bottom type. Interning `Never` with `nullable = true`
    /// yields [`TypeKind::Null`] (type.dart:1207-1209), so a stored `Never`
    /// always has the no-suffix projection and `is_bottom` is exactly
    /// "kind is `Never`".
    Never,
    /// `dynamic` — a top type; semantically nullable despite the missing `?`.
    Dynamic,
    /// `void` — a top type for the Left/Right Top rules; not denotable.
    Void,
    /// The error-recovery sentinel. Subtyping with either side is `true`
    /// (`subtype.dart:274-276`) so resolution errors do not avalanche.
    Invalid,
    /// `Null` — the class of `null`. No `?` suffix, semantically nullable.
    Null,
    /// An interface type: element + type arguments (`type.dart:575`).
    /// `FutureOr` is *not* routed here — it gets its own variant below.
    Interface {
        /// The declared class/mixin this instantiates.
        decl: TypeDeclId,
        /// Range of the type arguments in the argument pool.
        args: PoolRange,
    },
    /// A type parameter (`type.dart:1631`). M1 carries no promoted bound —
    /// promotion is flow analysis (M2, `dartq-flow`) — and no declared
    /// bound: bounds live on the element side (spec §4.1).
    TypeParameter {
        /// The declared type parameter.
        parameter: TypeParamId,
    },
    /// A function type (`type.dart:100`); the signature is side-tabled.
    Function {
        /// The pooled signature.
        signature: FunctionSigId,
    },
    /// `FutureOr<T>` as a first-class variant. The analyzer models it as an
    /// `InterfaceTypeImpl` special case (`type.dart:518`) with dedicated
    /// subtype rules (`subtype.dart:372-385`, `418-449`); a variant keeps
    /// those rules free of element matching.
    FutureOr {
        /// The `T` in `FutureOr<T>`.
        base: TypeId,
    },
    /// A record type (`type.dart:1296`). Skeleton only: M1 interning and
    /// structural equality, no subtype/LUB rules (spec §5, PRD §10.2).
    Record {
        /// The pooled shape.
        shape: RecordShapeId,
    },
}

/// An interned type: a [`TypeKind`] plus the nullability dimension.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Type {
    kind: TypeKind,
    nullable: bool,
}

impl Type {
    pub(crate) const fn new(kind: TypeKind, nullable: bool) -> Self {
        Self { kind, nullable }
    }

    /// Returns the kind.
    #[must_use]
    pub const fn kind(self) -> TypeKind {
        self.kind
    }

    /// Returns whether the type carries the `?` projection. `false` for the
    /// unit kinds even where the type is semantically nullable (`Null`,
    /// `dynamic`, `void`) — use [`crate::TypeSystem::is_nullable`] for the
    /// semantic question.
    #[must_use]
    pub const fn is_nullable(self) -> bool {
        self.nullable
    }

    /// Returns the syntactic suffix; never
    /// [`NullabilitySuffix::Star`](crate::NullabilitySuffix::Star).
    #[must_use]
    pub const fn nullability_suffix(self) -> NullabilitySuffix {
        if self.nullable {
            NullabilitySuffix::Question
        } else {
            NullabilitySuffix::None
        }
    }
}

/// `(offset, length)` range into a side-table pool of the owning
/// [`crate::TypeTable`].
///
/// Pools are append-only, so the range is a stable *position* handle, but
/// equal contents need not sit at equal offsets (empty ranges, dedup
/// rollback). Only the length is intrinsic to the shape; content equality
/// is the table's structural comparison
/// ([`crate::TypeTable`] internals), which is why `PartialEq`/`Hash`
/// deliberately ignore the offset.
#[derive(Clone, Copy, Debug)]
pub struct PoolRange {
    pub(crate) offset: u32,
    pub(crate) len: u32,
}

impl PartialEq for PoolRange {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len
    }
}

impl Eq for PoolRange {}

impl core::hash::Hash for PoolRange {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.len.hash(state);
    }
}

impl PoolRange {
    pub(crate) const fn new(offset: u32, len: u32) -> Self {
        Self { offset, len }
    }

    /// Start index into the pool.
    #[must_use]
    pub const fn offset(self) -> usize {
        self.offset as usize
    }

    /// Number of entries.
    #[must_use]
    pub const fn len(self) -> usize {
        self.len as usize
    }

    /// Whether the range covers no entries.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }
}

/// A named member of a function signature (named parameter) or record shape
/// (named field). Canonical order is sorted by `name`, enforced at interning.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NamedMember {
    /// Member name.
    pub name: String,
    /// Member type.
    pub ty: TypeId,
    /// Whether the named *parameter* is required; ignored for record fields.
    pub required: bool,
}

/// Uninterned input for every non-signature, non-record kind; the accepted
/// form of [`crate::TypeTable::intern`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum UninternedType {
    /// `Never`. Interning with `nullable = true` yields `Null`.
    Never,
    /// `dynamic`. Always stored non-`?` (type.dart:90-92).
    Dynamic,
    /// `void`. Always stored non-`?`.
    Void,
    /// The error-recovery sentinel. Always stored non-`?`.
    Invalid,
    /// `Null`. Always stored non-`?` (`Null` is nullable without a suffix).
    Null,
    /// An interface instantiation.
    Interface {
        /// The declared class/mixin.
        decl: TypeDeclId,
        /// Type arguments; the caller owns arity validation via the
        /// hierarchy snapshot (the table treats declarations as opaque).
        args: Vec<TypeId>,
    },
    /// A type parameter reference.
    TypeParameter {
        /// The declared type parameter.
        parameter: TypeParamId,
    },
    /// `FutureOr<T>`.
    FutureOr {
        /// The `T` in `FutureOr<T>`.
        base: TypeId,
    },
}

/// Uninterned input for [`crate::TypeTable::intern_function`]
/// (`FunctionTypeImpl`, type.dart:100-132).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct UninternedFunction {
    /// The return type.
    pub return_type: TypeId,
    /// Declared type parameters `<T, ...>`; correlated positionally during
    /// function subtyping (`subtype.dart:87-96`).
    pub type_parameters: Vec<TypeParamId>,
    /// Positional parameter types, in declaration order.
    pub positional: Vec<TypeId>,
    /// Named parameters; sorted by name into canonical order at interning.
    pub named: Vec<NamedMember>,
    /// `requiredPositionalParameterCount` (type.dart:132).
    pub required_positional: u32,
}

/// Uninterned input for [`crate::TypeTable::intern_record`]
/// (`RecordTypeImpl`, type.dart:1296).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct UninternedRecord {
    /// Positional field types, in order.
    pub positional: Vec<TypeId>,
    /// Named fields; sorted by name into canonical order at interning
    /// (field `required` flags are normalized to `false`).
    pub named: Vec<NamedMember>,
}

/// Pooled view of a function signature (copyable snapshot of
/// [`UninternedFunction`] after normalization).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FunctionSigData {
    /// The return type.
    pub return_type: TypeId,
    /// Declared type parameters, in the parameter pool.
    pub type_parameters: PoolRange,
    /// Positional parameter types, in the argument pool.
    pub positional: PoolRange,
    /// Named parameters (sorted by name), in the named-member pool.
    pub named: PoolRange,
    /// `requiredPositionalParameterCount` (type.dart:132).
    pub required_positional: u32,
}

/// Pooled view of a record shape.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct RecordShapeData {
    /// Positional field types, in the argument pool.
    pub positional: PoolRange,
    /// Named fields (sorted by name), in the named-member pool.
    pub named: PoolRange,
}
