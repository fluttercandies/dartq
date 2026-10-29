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
    /// Some function type (`int Function(E, E)`), signature unknown — the
    /// name-level lattice spells no shape. Carries only the head's
    /// nullability, which is the promotion-relevant bit: it subtypes only
    /// itself and the top types, promotes to its non-null form, and every
    /// signature question stays the semantic layer's.
    Function,
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

/// One node of a staged type's written-argument tree as spelled from
/// outside the event stream (#2364). The recorder interns the tree into
/// the same arena as the written ones ([`ArgTy`]), so the interpreter's
/// id equality stays tree equality.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StagedArg {
    /// A named spelling (`Issue`, the `List` of `List<List<int>>`).
    Named {
        lexeme: String,
        nullable: bool,
        /// The node's own written arguments.
        args: Vec<StagedArg>,
    },
    /// `dynamic` — the top face, admitting every co-argument.
    Dynamic,
    /// The invalid face ([`Base::Unknown`]) — a spelling that resolves
    /// nowhere (`Und` of `<Und>[]`), the oracle's `InvalidType` stand-in
    /// (`InvalidType <: X` for every `X`, subtype.dart:274, so a
    /// `List<Invalid>` RHS keeps the write promotable — probe2360 case
    /// `i` reports only the element's `non_type_as_type_argument`).
    Invalid,
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
    /// The bound type as written, beside its written-argument tree — the
    /// `TypeParameterTypeImpl` arm of the official `isAlwaysExhaustive`
    /// (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/element/type_system.dart:868-877): the
    /// verdict recurses into the parameter's bound, so `T extends St` over
    /// the enum `St` splits completely. Unlike [`Self::target`] this keeps
    /// every derivable shape, not only the ones whose non-null face
    /// differs — a `?`-less named bound collapses `target` to `None` and
    /// still decides here.
    pub(crate) bound: (Ty, u32),
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
    /// Whether this back-end is the resolved (element-indexed) layer —
    /// `false` for the name-level default the unit tests run under. An
    /// `Unknown` face under the resolved seam is the layer's own
    /// resolution gap (the stager dropped an unresolvable head), never a
    /// lattice spelling the name level chose.
    fn is_resolved_seam(&self) -> bool {
        false
    }
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
    /// `TypeSystem.isAlwaysExhaustive` (type_system.dart:846-888): whether a
    /// switch over `ty`'s values is closed by construction — `Null`/`bool`
    /// split completely, enums and sealed classes close by their element
    /// faces, `FutureOr<T>` recurses into `T`. `args` is the scrutinee's
    /// written-argument arena index into `arena` ([`EMPTY_ARGS`] when
    /// argument-less) and `names` the recorder's interned-name lexeme table —
    /// a resolved back-end resolves `Base::Name` ids through it and reads the
    /// `FutureOr` argument through `args`/`arena`. The default answers
    /// `false`: the under-report direction (a missed verdict keeps the
    /// post-switch state live — conservative, never a false dead row).
    /// Implementations may only answer `true` on a certain verdict.
    fn is_always_exhaustive(
        &self,
        ty: Ty,
        args: u32,
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
    ) -> bool {
        let _ = (ty, args, arena, names);
        false
    }

    /// Whether `ty` — a pattern's required type — is the resolved
    /// `Invalid` type (official `promoteForPattern`'s
    /// `knownType is SharedInvalidType` arm,
    /// `flow_analysis.dart:7720-7723`). `args`/`arena`/`names` carry the
    /// written spelling exactly like [`TypeOps::is_subtype_typed`]. The
    /// default answers `false`: the name-level lattice has no `Invalid`
    /// type. A resolved back-end overrides it so a pattern whose written
    /// type resolves nowhere neither tests nor promotes — the official
    /// rejoins the live state into the unmatched chain and keeps the body
    /// running, where the `Invalid` factor would vacuously answer `Never`
    /// (subtype.dart's 0c arm) and dead every pattern body.
    fn is_invalid_typed(
        &self,
        ty: Ty,
        args: u32,
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
    ) -> bool {
        let _ = (ty, args, arena, names);
        false
    }

    /// Publishes the recorder's interned-name lexeme table to the back-end
    /// before a replay begins (`Interp::run`'s single call). The base ops
    /// take bare `Ty`s — a `Base::Name(id)` carries no lexeme of its own —
    /// so a resolved back-end builds its `name id → type` bindings here,
    /// incrementally: `names` only ever grows inside a unit, one forward
    /// scan covers the ids seen since the last call. The default ignores the
    /// table: [`NameTypeOps`] decides on the `Ty` spelling alone.
    fn observe_names(&self, names: &[Box<str>]) {
        let _ = names;
    }

    /// Whether a bare-head invocation's spelling (`name` an id into
    /// `names`) resolves nowhere — the callee-side `InvalidType` face: an
    /// unresolved call's result is the official's `InvalidType`, so the
    /// write it feeds types invalid, and every `is`-test on it succeeds
    /// vacuously (the 0c arm) deading the failing arm — `var t = nowhere();
    /// if (t is C) … else …` reports the else as dead code exactly like
    /// the analyzer does. The default answers `false`: the name-level
    /// lattice cannot know resolution, and `Unknown` keeps every test
    /// decidable-agnostic (the refusal trio's posture); only a resolved
    /// back-end that can prove the spelling answers as nothing invokable
    /// may answer `true`.
    fn call_head_unresolved(&self, name: u32, names: &[Box<str>]) -> bool {
        let _ = (name, names);
        false
    }

    /// Whether an object pattern's property sub-pattern carries an invalid
    /// required type: the member `field`'s own declared (a field) or
    /// returned (a getter) type head on the owner `owner` resolves
    /// nowhere. The official's `promoteForPattern` refuses the invalid
    /// known type outright (`flow_analysis.dart:7720-7723`), so the
    /// sub-pattern may fail and its case cannot cover vacuously however
    /// invalid the scrutinee (`C1(:var name)` with `name`'s type broken
    /// keeps the later clauses live; oracle-probed on the getter and field
    /// variants). The default answers `false`: the name-level lattice has
    /// no resolution.
    fn object_property_invalid(&self, owner: u32, field: u32, names: &[Box<str>]) -> bool {
        let _ = (owner, field, names);
        false
    }

    /// [`TypeOps::factor`] with the operand spellings' written-argument
    /// trees: `from_args`/`what_args` are arena indices into `arena`
    /// ([`EMPTY_ARGS`] when argument-less) and `names` the recorder's
    /// lexeme table. The argument face decides questions the bare `Ty`s
    /// cannot — `factor(Response, Response<T>)`: the bare `Ty`s compare
    /// equal (same name, same nullability) yet the instantiated forms
    /// `Response<dynamic>` vs `Response<T>` are not mutual subtypes, so the
    /// remainder is `Response`, not `Never` (factor_type.dart's
    /// instantiated-type arm). The default delegates to
    /// [`TypeOps::factor`] — identical answers to the pre-hook behavior.
    fn factor_typed(
        &self,
        from: Ty,
        from_args: u32,
        what: Ty,
        what_args: u32,
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
    ) -> Ty {
        let _ = (from_args, what_args, arena, names);
        self.factor(from, what)
    }

    /// [`TypeOps::is_subtype`] over the instantiated spellings:
    /// `left_args`/`right_args` are arena indices into `arena`
    /// ([`EMPTY_ARGS`] when argument-less) and `names` the recorder's
    /// lexeme table. The default keeps the name-level posture: different
    /// bases answer the bare [`TypeOps::is_subtype`] verdict; equal-base
    /// spellings compare argument trees element-wise below the declared
    /// arity (covariant — subtype.dart's `arguments_subtype`), which is
    /// stricter than the bare verdict exactly where the written arguments
    /// decide (`List<int>` vs `List<num>`: undecidable at the name level,
    /// name-level-admitting where `dynamic`/`Unknown` stands in for a
    /// raw-typed declaration). A resolved back-end answers through the
    /// real hierarchy instead.
    fn is_subtype_typed(
        &self,
        left: Ty,
        left_args: u32,
        right: Ty,
        right_args: u32,
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
    ) -> bool {
        let _ = names;
        if left.base != right.base {
            return self.is_subtype(left, right);
        }
        if !self.is_subtype(left, right) {
            return false;
        }
        name_args_subtype(left_args, right_args, arena)
    }

    /// [`TypeOps::try_promote_to`] over the instantiated spellings (the
    /// official `tryPromoteToType` gate, type_system.dart:1729-1760:
    /// promote iff `to` is a *strict* subtype of `from` —
    /// `isValidPromotionStep`, flow_analysis.dart:7229-7247). The
    /// default's decision order reproduces the name-level posture the
    /// call sites pinned: a target the current type already satisfies
    /// (`x is int?` on `int x`, `x is int` on `int x` — identical types
    /// land here through the mutual-subtype arm) is no step; a
    /// covered subtype (`to <: from` — `x is int` on `int? x`) is the
    /// step; a same-base target whose written arguments cannot sit
    /// under the current ones (`x is List<num>` on `List<int> x` — an
    /// illegal promotion step the bare `Ty`s cannot see) refuses; every
    /// other shape promotes optimistically — the undecidable cross-base
    /// question (`int <: num` needs a real hierarchy, and the oracle
    /// pins `num n; n is int` ON). A resolved back-end answers
    /// `Some(to)` exactly on `is_subtype(to, from) && !is_subtype(from,
    /// to)` — the oracle's strict-subtype gate verbatim, the name-level
    /// optimistic arm tightened to the truth.
    fn try_promote_to_typed(
        &self,
        to: Ty,
        to_args: u32,
        from: Ty,
        from_args: u32,
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
    ) -> Option<Ty> {
        // A test the current type already satisfies (`x is num` on
        // `int x`, `x is int?` on `int x`, `x is int` on `int x`) is no
        // step — `from <: to` refuses before `to <: from` admits, so the
        // mutual-subtype arm lands here.
        if self.is_subtype_typed(from, from_args, to, to_args, arena, names) {
            return None;
        }
        if self.is_subtype_typed(to, to_args, from, from_args, arena, names) {
            return Some(to);
        }
        if to.base == from.base
            && !matches!(to.base, Base::Dynamic | Base::Unknown | Base::Void)
            && !name_args_admit(to.base, to_args, from_args, arena)
        {
            return None;
        }
        Some(to)
    }

    /// The object pattern's downward-inferred required type (official
    /// `analyzeObjectPattern`'s inference — shared type_analyzer.dart
    /// :1749-1772 — with the matched value's static type as the context):
    /// a RAW pattern spelling's own type parameters solve along its
    /// superclass chain, so `case AL():` over an `AV<int>` scrutinee tests
    /// and promotes `AL<int>`, not the raw face. `Some` carries the
    /// inferred spelling beside its written-argument arena index;
    /// `None` keeps the un-inferred posture. The name-level default is
    /// `None` — there is no hierarchy to climb.
    fn downcast_infer(
        &self,
        tested: Ty,
        tested_args: u32,
        from: Ty,
        from_args: u32,
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
    ) -> Option<(Ty, u32)> {
        let _ = (tested, tested_args, from, from_args, arena, names);
        None
    }
}

/// The name-level argument-subtype question: element-wise covariance over
/// two written argument lists (`List<int>` <: `List<int?>`). Entries are
/// [`ArgTy`] nodes; the question is undecidable on distinct entry names —
/// refuse (the official lattice refuses unless a real subclass relation
/// holds, invisible at this level). Arity mismatch refuses. An
/// argument-less `right` list (`EMPTY_ARGS` = the raw-type face, read as
/// `Name<dynamic>`) admits everything; an argument-less `left` over a
/// written `right` refuses, matching the raw-to-instantiated direction.
pub(crate) fn name_args_subtype(left_args: u32, right_args: u32, arena: &[Vec<ArgTy>]) -> bool {
    let ls = arena.get(left_args as usize).map_or(&[][..], Vec::as_slice);
    let rs = arena
        .get(right_args as usize)
        .map_or(&[][..], Vec::as_slice);
    match (ls.is_empty(), rs.is_empty()) {
        (_, true) => true,
        (true, false) => false,
        (false, false) => {
            ls.len() == rs.len()
                && ls
                    .iter()
                    .zip(rs)
                    .all(|(a, b)| name_entry_subtype(a, b, arena))
        }
    }
}

/// One argument-tree entry pair's name-level subtype answer — mirrors
/// [`TypeOps::is_subtype`]'s decidable arms on the [`Base`] spelling,
/// recursing into each entry's own written arguments.
fn name_entry_subtype(a: &ArgTy, b: &ArgTy, arena: &[Vec<ArgTy>]) -> bool {
    match (a.base, b.base) {
        (Base::Dynamic | Base::Unknown | Base::Void, _)
        | (_, Base::Dynamic | Base::Unknown | Base::Void) => true,
        (Base::Null, _) => b.nullable,
        (_, Base::Null) => false,
        _ => {
            a.base == b.base
                && (!a.nullable || b.nullable)
                && (a.args == b.args || name_args_subtype(a.args, b.args, arena))
        }
    }
}

/// Whether a tested spelling's written arguments can sit under the
/// declared ones — the same-base arm of the promotion gate
/// ([`TypeOps::try_promote_to_typed`]'s refuse check). Both argument-less
/// is the fast path (no information either way); a written `tested` list
/// over an argument-less declared list admits (the raw type reads
/// `Name<dynamic>`, whose entries admit everything); the reverse refuses
/// except `Never` — a subtype of every instantiation.
pub(crate) fn name_args_admit(
    tested_base: Base,
    tested_args: u32,
    declared_args: u32,
    arena: &[Vec<ArgTy>],
) -> bool {
    let ts = arena
        .get(tested_args as usize)
        .map_or(&[][..], Vec::as_slice);
    let ds = arena
        .get(declared_args as usize)
        .map_or(&[][..], Vec::as_slice);
    match (ts.is_empty(), ds.is_empty()) {
        (_, true) => true,
        (true, false) => tested_base == Base::Never,
        (false, false) => name_args_subtype(tested_args, declared_args, arena),
    }
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
