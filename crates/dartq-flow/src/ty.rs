//! Name-level type model and the promotion type-operation seam.
//!
//! The official engine delegates every type question to the client through
//! `FlowAnalysisTypeOperations` (`flow_analysis_operations.dart:60`): subtyping,
//! factoring (the remainder of a type after an `is`-check removes another),
//! and nullability classification. dartq resolves no types at flow time, so
//! the default implementation ([`NameTypeOps`]) answers those questions from
//! type *names* plus nullability; a resolved back-end (dartq-types) replaces
//! it through the same [`TypeOps`] seam.

/// The base of a [`Ty`]: an interned type name or one of the special bases.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Base {
    /// An interned named type (`int`, `Foo`, a type parameter spelling…).
    Name(u32),
    /// `dynamic`.
    Dynamic,
    /// `void` (appears only as a declared type; unusable in promotions).
    Void,
    /// `Never`.
    Never,
    /// `Null`.
    Null,
    /// No type information (`var`/inferred, or a shape the parser did not
    /// reduce to a name). Conservative everywhere.
    Unknown,
}

/// A type: base plus nullability. Interned per body into a `u32` by the
/// interpreter, but kept structural here so [`TypeOps`] implementations can
/// inspect it without a table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Ty {
    pub base: Base,
    pub nullable: bool,
}

impl Ty {
    pub const UNKNOWN: Ty = Ty {
        base: Base::Unknown,
        nullable: false,
    };
    pub const DYNAMIC: Ty = Ty {
        base: Base::Dynamic,
        nullable: false,
    };
    pub const NULL: Ty = Ty {
        base: Base::Null,
        nullable: true,
    };
    pub const NEVER: Ty = Ty {
        base: Base::Never,
        nullable: false,
    };

    /// The same base, non-nullable.
    pub const fn non_null(self) -> Ty {
        Ty {
            base: self.base,
            nullable: false,
        }
    }

    /// The same base, nullable.
    pub const fn make_nullable(self) -> Ty {
        Ty {
            base: self.base,
            nullable: true,
        }
    }
}

/// Nullability classification (official `TypeClassification`,
/// `flow_analysis_operations.dart:144`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Classification {
    /// `Null` or an equivalent (`Never?`): always null.
    NullOrEquivalent,
    /// Potentially nullable, not equivalent to `Null`.
    PotentiallyNullable,
    /// Non-nullable.
    NonNullable,
}

/// The client-supplied type operations seam (official
/// `FlowAnalysisTypeOperations`, cut to the questions the engine asks).
pub trait TypeOps {
    /// Whether `left` is a subtype of `right`.
    fn is_subtype(&self, left: Ty, right: Ty) -> bool;
    /// The "remainder" of `from` when `what` is removed by an instance
    /// check (official `factor`): what must be true in the `is`-check's
    /// false branch.
    fn factor(&self, from: Ty, what: Ty) -> Ty;
    /// Tries to promote `from` to `to`; `None` when `to` adds nothing or is
    /// incompatible (official `tryPromoteToType`).
    fn try_promote_to(&self, to: Ty, from: Ty) -> Option<Ty>;
    /// The non-null promoted version (official `promoteToNonNull`); may
    /// return the input unchanged (e.g. `dynamic`).
    fn promote_to_non_null(&self, t: Ty) -> Ty;
    /// Nullability classification (official `classifyType`).
    fn classify(&self, t: Ty) -> Classification;
    /// Whether `t` is a bottom type (`Never`, or a type variable promoted
    /// to `Never` — name-level: `Never` itself).
    fn is_bottom(&self, t: Ty) -> bool;
}

/// The name-level default implementation. Rules, in order:
///
/// * identical types are subtypes; `Never` is a subtype of everything;
/// * everything is a subtype of `dynamic` and of unknown (the unknown type
///   is treated as a top type so optimistic `is`-promotions on `var`
///   locals go through);
/// * a non-nullable type is a subtype of its nullable form;
/// * name equality with nullability subsumption otherwise.
///
/// Factoring: `from <: what` → `Never`; nullable `from` with `what` its
/// non-null form (or `Null`) → the non-null form; otherwise `from`
/// unchanged (no else-branch promotion without real subtyping).
pub struct NameTypeOps;

impl TypeOps for NameTypeOps {
    fn is_subtype(&self, left: Ty, right: Ty) -> bool {
        if left == right || left.base == Base::Never {
            return true;
        }
        if matches!(right.base, Base::Dynamic | Base::Unknown) {
            return true;
        }
        if matches!(left.base, Base::Dynamic | Base::Unknown) {
            return false;
        }
        if left.base == Base::Null {
            return right.nullable;
        }
        if right.base == Base::Null {
            return false;
        }
        left.base == right.base && (!left.nullable || right.nullable)
    }

    fn factor(&self, from: Ty, what: Ty) -> Ty {
        if self.is_subtype(from, what) {
            return Ty::NEVER;
        }
        if from.nullable
            && !matches!(
                from.base,
                Base::Dynamic | Base::Unknown | Base::Null | Base::Never
            )
        {
            let non_null = from.non_null();
            if what == non_null || what.base == Base::Null {
                return non_null;
            }
        }
        from
    }

    fn try_promote_to(&self, to: Ty, from: Ty) -> Option<Ty> {
        if self.is_subtype(to, from) {
            return Some(to);
        }
        // Optimistic: from a nameless (`var`-inferred) or `dynamic` type,
        // any `is`-check target is a valid promotion target — matches the
        // observable behavior of promoting `dynamic`.
        if matches!(from.base, Base::Dynamic | Base::Unknown) && !matches!(to.base, Base::Void) {
            return Some(to);
        }
        // Nullable promotion: `int?` tested against `int`.
        if from.nullable && to == from.non_null() {
            return Some(to);
        }
        None
    }

    fn promote_to_non_null(&self, t: Ty) -> Ty {
        match t.base {
            Base::Dynamic | Base::Unknown => t,
            _ if t.nullable && t.base != Base::Null => t.non_null(),
            Base::Null => Ty::NEVER,
            _ => t,
        }
    }

    fn classify(&self, t: Ty) -> Classification {
        match t.base {
            Base::Null => Classification::NullOrEquivalent,
            Base::Never if t.nullable => Classification::NullOrEquivalent,
            Base::Dynamic | Base::Unknown => Classification::PotentiallyNullable,
            _ if t.nullable => Classification::PotentiallyNullable,
            _ => Classification::NonNullable,
        }
    }

    fn is_bottom(&self, t: Ty) -> bool {
        t.base == Base::Never && !t.nullable
    }
}
