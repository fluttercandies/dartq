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

/// Reserved arena index for a written-argument-less type: the fast path
/// (`int`, a raw `List`, `dynamic`) never touches the recorder's arena.
pub const EMPTY_ARGS: u32 = 0;

/// One node of a recorded type-argument tree: the base spelling plus its
/// own written arguments (#611 step 4). Nested lists reference the same
/// arena by index, so the tree compares structurally without materializing
/// per-visit nodes. The cross-crate read face of the fact rows' `args`
/// ([`crate::Flow::type_args_at`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArgTy {
    pub base: Base,
    pub nullable: bool,
    /// The node's own written arguments (arena index; [`EMPTY_ARGS`] when
    /// argument-less).
    pub args: u32,
}

impl Ty {
    pub const UNKNOWN: Ty = Ty {
        base: Base::Unknown,
        nullable: false,
    };
    /// The potentially-nullable unknown face: the marker type of a
    /// property read whose static type the flow layer cannot resolve (the
    /// field's declared type lives in the semantic layer). Its non-null
    /// face is [`Ty::UNKNOWN`] — the promotion marker a null check records
    /// on a property slot (#1409).
    pub const UNKNOWN_NULLABLE: Ty = Ty {
        base: Base::Unknown,
        nullable: true,
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
    #[must_use]
    pub const fn non_null(self) -> Ty {
        Ty {
            base: self.base,
            nullable: false,
        }
    }

    /// The same base, nullable.
    #[must_use]
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

/// One type parameter's declared-bound nullability face. The official
/// `classifyType` is a subtype question, not a spelling check (upstream
/// Dart SDK source, pkg/analyzer/lib/src/dart/resolver/flow_analysis_visitor.dart:615-632):
/// `T <: Object` → nonNullable, `T <: Null` → nullOrEquivalent, else
/// potentiallyNullable — and for a type variable those rules consult its
/// bound: `T <: Object` iff `B <: Object`, the implicit bound being
/// `Object?` (pkg/analyzer/lib/src/dart/element/subtype.dart:303-318). A
/// `?`-less reference to `T extends Object?` therefore classifies
/// potentially-nullable where the syntactic shape alone says non-nullable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Boundary {
    /// The classification of a `?`-less reference to the parameter.
    pub(crate) class: Classification,
    /// The bound's non-null face when it differs from the bound — the
    /// `tryMarkNonNullable` promotion target beside its written-argument
    /// tree (official `NonNull(X) = X & NonNull(B)` with the
    /// identical-bound collapse, pkg/analyzer/lib/src/dart/element/type_system.dart:1467-1495:
    /// a top-type or `FutureOr`-of-top bound promotes to nothing, its
    /// non-null form being itself). `None` → a null check promotes nothing.
    pub(crate) target: Option<(Ty, u32)>,
}

/// The closed-declaration facts a switch scrutinee's always-exhaustive
/// verdict reads (upstream `TypeSystem.isAlwaysExhaustive`,
/// type_system.dart:846-888), captured at record time from the unit's own
/// container declarations. A declaration parsed after the executable that
/// reads it arrives too late — the under-report direction (the verdict
/// answers `false`, keeping the post-switch state live).
#[derive(Default)]
pub(crate) struct ClosedNames {
    /// Interned name ids of the unit's `sealed` classes and enums (enums
    /// are sealed by construction, static_type.dart:67).
    pub(crate) names: rustc_hash::FxHashSet<u32>,
    /// `bool`'s interned id — `isDartCoreBool` is a name check at this
    /// level; `None` until the recorder interns it.
    pub(crate) bool_name: Option<u32>,
    /// `FutureOr`'s interned id — the verdict recurses into its single
    /// argument (type_system.dart:865); `None` until interned.
    pub(crate) future_or: Option<u32>,
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
/// Factoring (official `factorType`,
/// `pkg/front_end/lib/src/type_inference/factor_type.dart:11-45`): the
/// remainder of `from` after an `is`-check against `what` removed it —
/// `from <: what` → `Never`; nullable `from = R?` → `factor(R, what)` when
/// `Null <: what` (i.e. `what` is nullable), else `factor(R, what)?`; the
/// `FutureOr` arms need a real hierarchy (unmodeled); otherwise `from`
/// unchanged. So the else branch of `x is T` on `T? x` promotes to `Never?`
/// (the Null knowledge), and a bottom remainder marks the branch
/// unreachable (sound flow — `sound-flow-analysis` is enabled-by-default
/// and expired, `experiments/flags.dart:309-315`).
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
                Base::Dynamic | Base::Unknown | Base::Void | Base::Null | Base::Never
            )
        {
            // Rule 2 of factorType: `T = R?` factors as `factor(R, what)`
            // when `Null <: what`, else `factor(R, what)?`. `Null <: what`
            // at the name level is exactly `what` being nullable.
            let r = from.non_null();
            if what.nullable {
                return self.factor(r, what);
            }
            return self.factor(r, what).make_nullable();
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
