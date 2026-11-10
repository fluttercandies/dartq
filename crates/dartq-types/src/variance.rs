//! Type-parameter variance: the in/out/inout lattice, per-parameter facts,
//! and the variance-aware interface-argument relation.
//!
//! Fact sources (behavior anchored, form native):
//!
//! * `pkg/_fe_analyzer_shared/lib/src/types/shared_type.dart:6-134` — the
//!   four-variance lattice (`unrelated` top, `invariant` bottom), the
//!   keyword spellings (`out`/`in`/`inout`), `combine` (:65-107),
//!   `greaterThanOrEqual` (:109-127), and `meet` (:129-134, the bitwise
//!   `index | index` over the declared enum order — the Rust discriminants
//!   below keep that encoding load-bearing).
//! * `pkg/analyzer/lib/src/dart/element/element.dart:11493-11564` — the
//!   legacy-covariant default: an absent variance keyword means
//!   `Variance.covariant` (`_variance ?? shared.Variance.covariant`,
//!   element.dart:11560-11562), and `isLegacyCovariant` is exactly "no
//!   keyword was written" (element.dart:11544-11546).
//! * `pkg/analyzer/lib/src/dart/element/subtype.dart:44-83` — the
//!   enforcement point: `_interfaceArguments` relates one argument pair per
//!   declared variance (covariant left-to-right, contravariant
//!   right-to-left, invariant both ways).
//! * `pkg/analyzer/lib/src/summary2/variance_builder.dart:57-151` — the
//!   variance *computation* over a type (the `_compute` decision matrix and
//!   the generic-function bound-invariance rule :131-140), mirrored by
//!   `TypeParameterElementImpl.computeVarianceInType`
//!   (element.dart:11576-11623).
//! * `pkg/analyzer/lib/src/summary2/variance_builder.dart:153-242` — the
//!   alias entries (`_functionTypeAlias`/`_genericTypeAlias`): a typedef's
//!   parameter variance is *computed* from the aliased body and stored on
//!   the element (`_setVariance`, :258-261), not read from a keyword. That
//!   stored value is what the default-type face of instantiate-to-bound
//!   seeds (`_computeDefaultType`, default_types_builder.dart:153-161/
//!   171-179; the to-bounds face, `instantiateTypeFormalsToBounds`, reads
//!   no variance): a typedef parameter contravariant in its own body
//!   grounds to `Never`, while a class parameter — declared only, legacy
//!   covariant — still grounds to `dynamic`.
//!
//! # Enforcement-point status
//!
//! [`arguments_related`] is the variance-aware decision the interface
//! subtype walk consumes; [`TypeSystem::is_subtype`] still runs the M1
//! all-covariant argument check (`type_system.rs`), which is byte-identical
//! on every declared `out`-less Dart program (class parameters are legacy
//! covariant, element.dart:11560-11562) and diverges only under the
//! experimental `variance` feature's `in`/`inout` keywords. Wiring the
//! facts snapshot into the subtype walk is the semantic layer's edit (see
//! [`arguments_related`]).
//!
//! Declared variances are only `in`/`out`/`inout` plus the legacy default,
//! so [`Variance::Unrelated`] is a *computation* result (it says "the
//! parameter does not occur free in the type"), never a declared variance.

use crate::ids::{TypeDeclId, TypeId, TypeParamId};
use crate::system::{DEPTH_LIMIT, TypeSystem};
use crate::table::TypeTable;
use crate::ty::TypeKind;

/// The variance of a type parameter in a type
/// (`shared_type.dart:6-18`). Discriminant order is load-bearing: `meet`
/// is `index | index` over these values (`shared_type.dart:129-134`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Variance {
    /// `X` does not occur free in the type (`shared_type.dart:7-8`).
    Unrelated = 0,
    /// `U <: V` implies `[U/X]T <: [V/X]T`; keyword `out`
    /// (`shared_type.dart:9-11`).
    Covariant = 1,
    /// `U <: V` implies `[V/X]T <: [U/X]T`; keyword `in`
    /// (`shared_type.dart:12-14`).
    Contravariant = 2,
    /// Some `U <: V` exist with `[U/X]T` and `[V/X]T` incomparable;
    /// keyword `inout` (`shared_type.dart:15-18`).
    Invariant = 3,
}

impl Variance {
    /// The keyword spelling (`shared_type.dart:28-47`); `unrelated` has no
    /// keyword and is not spellable.
    #[must_use]
    pub const fn keyword(self) -> Option<&'static str> {
        match self {
            Self::Unrelated => None,
            Self::Covariant => Some("out"),
            Self::Contravariant => Some("in"),
            Self::Invariant => Some("inout"),
        }
    }

    /// Combines the variance of `X` in `T` with the variance of `Y` in `S`
    /// into the variance of `X` in `[Y/T]S` (`shared_type.dart:65-107`).
    #[must_use]
    pub const fn combine(self, other: Self) -> Self {
        if matches!(self, Self::Unrelated) || matches!(other, Self::Unrelated) {
            return Self::Unrelated;
        }
        if matches!(self, Self::Invariant) || matches!(other, Self::Invariant) {
            return Self::Invariant;
        }
        // `self == other` is `covariant`; two different non-invariant
        // variances are covariant and contravariant (shared_type.dart:104-106).
        match (self, other) {
            (Self::Covariant, Self::Covariant) | (Self::Contravariant, Self::Contravariant) => {
                Self::Covariant
            }
            _ => Self::Contravariant,
        }
    }

    /// `self >= other` in the lattice partial order
    /// (`shared_type.dart:109-127`).
    #[must_use]
    pub const fn greater_or_equal(self, other: Self) -> bool {
        match self {
            Self::Unrelated => true,
            Self::Covariant => matches!(other, Self::Covariant | Self::Invariant),
            Self::Contravariant => matches!(other, Self::Contravariant | Self::Invariant),
            Self::Invariant => matches!(other, Self::Invariant),
        }
    }

    /// The lattice meet — `index | index` over the declared order
    /// (`shared_type.dart:129-134`). The `u32` cast mirrors the official
    /// `fromEncoding(index | other.index)`.
    #[must_use]
    pub const fn meet(self, other: Self) -> Self {
        Self::from_encoding((self as u32) | (other as u32))
    }

    /// `fromEncoding` (`shared_type.dart:24-25`): the discriminant lookup.
    #[must_use]
    pub const fn from_encoding(encoding: u32) -> Self {
        // The official values[] lookup is total over 0..4; the declared
        // lattice has exactly these four encodings.
        match encoding {
            1 => Self::Covariant,
            2 => Self::Contravariant,
            3 => Self::Invariant,
            _ => Self::Unrelated,
        }
    }
}

/// The declared facts of one type parameter: its variance (the `in`/`out`/
/// `inout` keyword or the legacy-covariant default, element.dart:
/// 11560-11562; for a *typedef* parameter the value the `VarianceBuilder`
/// computed from the aliased body, variance_builder.dart:258-261) and its
/// declared bound (`None` = unbounded; both instantiate-to-bound faces read
/// it as `dynamic` — `default_types_builder.dart:139`,
/// `type_system.dart:754`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ParamFacts {
    /// The element's stored variance: the declared keyword (or legacy
    /// default) for a class/mixin/enum parameter, the body-computed value
    /// for a typedef parameter (variance_builder.dart:173-181/218-221).
    pub variance: Variance,
    /// The declared bound, if any.
    pub bound: Option<TypeId>,
}

impl ParamFacts {
    /// The legacy default: covariant, unbounded
    /// (element.dart:11560-11562).
    pub const LEGACY: Self = Self {
        variance: Variance::Covariant,
        bound: None,
    };

    /// Returns these facts with the declared bound set — the builder form
    /// of the `bound` field, for snapshot construction sites.
    #[must_use]
    pub const fn with_bound(self, bound: TypeId) -> Self {
        Self {
            variance: self.variance,
            bound: Some(bound),
        }
    }
}

/// The injected facts snapshot for every type parameter a query may touch.
///
/// The types crate treats parameters as opaque
/// [`TypeParamId`](crate::TypeParamId) handles; bounds and variance
/// keywords arrive from the layer above exactly like supertype edges
///. An unregistered parameter reads as
/// [`ParamFacts::LEGACY`] — the official getter's `?? covariant` with a
/// null bound (element.dart:11560-11562) — so the snapshot only needs
/// entries for parameters that carry a keyword or a bound.
#[derive(Clone, Debug, Default)]
pub struct ParamFactsTable {
    facts: Vec<Option<ParamFacts>>,
}

impl ParamFactsTable {
    /// An empty snapshot: every parameter reads legacy.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the facts of one parameter.
    pub fn set(&mut self, parameter: TypeParamId, facts: ParamFacts) {
        let index = parameter.raw() as usize;
        if index >= self.facts.len() {
            self.facts.resize(index + 1, None);
        }
        self.facts[index] = Some(facts);
    }

    /// The facts of `parameter`; unregistered parameters read legacy
    /// (element.dart:11560-11562).
    #[must_use]
    pub fn get(&self, parameter: TypeParamId) -> ParamFacts {
        self.facts
            .get(parameter.raw() as usize)
            .copied()
            .flatten()
            .unwrap_or(ParamFacts::LEGACY)
    }
}

/// The variance of `parameter` in `ty` — the `_compute` decision matrix
/// (`variance_builder.dart:57-151`, mirrored by
/// `computeVarianceInType`, element.dart:11576-11623):
///
/// * a type-parameter type: `covariant` when it *is* `parameter`,
///   `unrelated` otherwise (variance_builder.dart:58-63);
/// * an interface instantiation: the meet of
///   `parameterᵢ.variance.combine(recursive)` over the arguments — the
///   target declaration's own parameter variances come from the facts
///   snapshot through the hierarchy (variance_builder.dart:64-103);
/// * `FutureOr<T>`: the official models it as an interface type whose sole
///   parameter is legacy covariant (`FutureOrTypeImpl extends
///   InterfaceTypeImpl`, type.dart:518), so the base is a covariant
///   position;
/// * a function type: the meet of the return type with
///   `contravariant.combine(…)` over the parameter types, forced to
///   `invariant` when `parameter` occurs in any of the signature's own
///   type-parameter bounds (variance_builder.dart:121-151 — "if \[variable]
///   is referenced in a bound at all, it makes the variance … invariant");
/// * a record type: the meet over all field types
///   (variance_builder.dart:111-117);
/// * unit kinds: `unrelated` (`variance_builder.dart:118`).
///
/// Recursion is bounded by `DEPTH_LIMIT` like the other type-operation
/// walks; a tripped budget answers `unrelated` at the frame that tripped.
#[must_use]
pub fn variance_in_type(
    sys: &TypeSystem<'_>,
    facts: &ParamFactsTable,
    parameter: TypeParamId,
    ty: TypeId,
) -> Variance {
    variance_in_type_with(sys, &|p| facts.get(p), parameter, ty)
}

/// [`variance_in_type`] over a caller-held facts provider instead of a
/// concrete [`ParamFactsTable`]. A snapshot whose parameter facts are split
/// across the shared table and an overlay (the generic-function binder
/// facts a signature map records per signature) answers `p`'s facts through
/// the overlay first: the bound-invariance leg (variance_builder.dart:
/// 131-140) keys on `facts_of(p).bound`, and a binder bound the shared
/// table never saw would leave the leg silent — the `Contravariant` answer
/// the `Never` default then grounds, where the oracle's `invariant` reads
/// `dynamic`.
#[must_use]
pub fn variance_in_type_with(
    sys: &TypeSystem<'_>,
    facts_of: &dyn Fn(TypeParamId) -> ParamFacts,
    parameter: TypeParamId,
    ty: TypeId,
) -> Variance {
    variance_in_type_bounded(
        sys.table(),
        facts_of,
        parameter,
        ty,
        &|decl, i| {
            sys.hierarchy()
                .decl(decl)
                .expect("declared owner")
                .type_parameters
                .get(i)
                .map(|&p| facts_of(p).variance)
        },
        DEPTH_LIMIT,
    )
}

/// The typedef-parameter variance entry — `VarianceBuilder._genericTypeAlias`
/// (variance_builder.dart:188-225) and `_functionTypeAlias` (:153-186): each
/// declared parameter's variance is computed from the aliased body itself
/// (`_compute`/`_computeFunctionType`), not read from a keyword. A
/// `FunctionTypeAlias` and a `GenericTypeAlias` flatten to the same
/// right-hand-side shape in this model — the function-type alias's
/// `_computeFunctionType` over its return type and formal parameters is
/// exactly the [`TypeKind::Function`] arm of the shared walk — so one
/// entry serves both spellings.
///
/// `argument_variance` answers the *stored* variance of an interface
/// declaration's `i`-th type parameter — the `parameter.variance` the
/// oracle reads inside `_compute` (variance_builder.dart:76-78/95-98). A
/// declaration the caller cannot model returns `None` and its argument
/// positions contribute `covariant` — the under-report direction: a
/// genuinely contravariant interface parameter read as covariant can only
/// soften the computed variance, never invent an occurrence. Callers with
/// a built hierarchy adapt `hierarchy.decl(decl).type_parameters` plus
/// `facts`; a mid-build caller (the semantic layer's own alias pass)
/// answers from its declaration-parameter map the same way.
///
/// Recursion into a co-alias (`_typeAliasElement`, :227-242) has no
/// counterpart: alias uses flatten to their right-hand sides at intern
/// time, so the walk already sees the expanded shape — including the
/// recursion-recovery covariance the official `_visit` set falls back to
/// (an unresolvable alias never reaches the walk at all).
#[must_use]
pub fn typedef_parameter_variances(
    table: &TypeTable,
    facts: &ParamFactsTable,
    parameters: &[TypeParamId],
    body: TypeId,
    argument_variance: impl Fn(TypeDeclId, usize) -> Option<Variance>,
) -> Vec<Variance> {
    typedef_parameter_variances_with(
        table,
        &|p| facts.get(p),
        parameters,
        body,
        argument_variance,
    )
}

/// [`typedef_parameter_variances`] over a caller-held facts provider — the
/// same overlay seam [`variance_in_type_with`] carries, so a snapshot whose
/// generic-function binder bounds live outside the shared table forwards
/// them to the bound-invariance leg (variance_builder.dart:131-140) instead
/// of reading `None` and computing `Contravariant` where the oracle's
/// `invariant` answers `dynamic`.
#[must_use]
pub fn typedef_parameter_variances_with(
    table: &TypeTable,
    facts_of: &dyn Fn(TypeParamId) -> ParamFacts,
    parameters: &[TypeParamId],
    body: TypeId,
    argument_variance: impl Fn(TypeDeclId, usize) -> Option<Variance>,
) -> Vec<Variance> {
    parameters
        .iter()
        .map(|&parameter| {
            variance_in_type_bounded(
                table,
                facts_of,
                parameter,
                body,
                &argument_variance,
                DEPTH_LIMIT,
            )
        })
        .collect()
}

fn variance_in_type_bounded(
    table: &TypeTable,
    facts_of: &dyn Fn(TypeParamId) -> ParamFacts,
    parameter: TypeParamId,
    ty: TypeId,
    argument_variance: &dyn Fn(TypeDeclId, usize) -> Option<Variance>,
    depth: u32,
) -> Variance {
    if depth == 0 {
        return Variance::Unrelated;
    }
    let kind = table.get(ty).kind();
    match kind {
        TypeKind::TypeParameter { parameter: p } if p == parameter => Variance::Covariant,
        TypeKind::Interface { decl, args } => {
            let mut result = Variance::Unrelated;
            // An unanswerable position reads covariant — the under-report
            // direction: `covariant.combine(x) == x`, so the argument's own
            // occurrences still count instead of fabricating a variance.
            for (i, &arg) in table.args(args).iter().enumerate() {
                let parameter_variance = argument_variance(decl, i).unwrap_or(Variance::Covariant);
                result = result.meet(parameter_variance.combine(variance_in_type_bounded(
                    table,
                    facts_of,
                    parameter,
                    arg,
                    argument_variance,
                    depth - 1,
                )));
            }
            result
        }
        TypeKind::FutureOr { base } => variance_in_type_bounded(
            table,
            facts_of,
            parameter,
            base,
            argument_variance,
            depth - 1,
        ),
        TypeKind::Function { signature } => {
            let sig = table.signature(signature);
            let mut result = variance_in_type_bounded(
                table,
                facts_of,
                parameter,
                sig.return_type,
                argument_variance,
                depth - 1,
            );
            // The bound-invariance rule (variance_builder.dart:131-140):
            // occurrence in any bound of the signature's own parameters
            // forces `invariant`.
            for p in table.function_type_parameters(sig.type_parameters) {
                let bound = facts_of(p).bound;
                if let Some(bound) = bound
                    && variance_in_type_bounded(
                        table,
                        facts_of,
                        parameter,
                        bound,
                        argument_variance,
                        depth - 1,
                    ) != Variance::Unrelated
                {
                    result = Variance::Invariant;
                }
            }
            for member in table.named_members(sig.named) {
                result = result.meet(Variance::Contravariant.combine(variance_in_type_bounded(
                    table,
                    facts_of,
                    parameter,
                    member.ty,
                    argument_variance,
                    depth - 1,
                )));
            }
            for i in 0..sig.positional.len() {
                let p = table.arg(sig.positional.offset() + i);
                result = result.meet(Variance::Contravariant.combine(variance_in_type_bounded(
                    table,
                    facts_of,
                    parameter,
                    p,
                    argument_variance,
                    depth - 1,
                )));
            }
            result
        }
        TypeKind::Record { shape } => {
            let shape = table.record_shape(shape);
            let mut result = Variance::Unrelated;
            for i in 0..shape.positional.len() {
                let p = table.arg(shape.positional.offset() + i);
                result = result.meet(variance_in_type_bounded(
                    table,
                    facts_of,
                    parameter,
                    p,
                    argument_variance,
                    depth - 1,
                ));
            }
            for member in table.named_members(shape.named) {
                result = result.meet(variance_in_type_bounded(
                    table,
                    facts_of,
                    parameter,
                    member.ty,
                    argument_variance,
                    depth - 1,
                ));
            }
            result
        }
        // Other type parameters and unit kinds never reference the queried
        // parameter (variance_builder.dart:118).
        TypeKind::TypeParameter { .. }
        | TypeKind::Never
        | TypeKind::Dynamic
        | TypeKind::Void
        | TypeKind::Invalid
        | TypeKind::Null => Variance::Unrelated,
    }
}

/// `_interfaceArguments` (`subtype.dart:44-83`): relates one instantiated
/// interface's arguments to another's under the target declaration's
/// declared parameter variances, positionally.
///
/// A declared [`Variance::Unrelated`] answers [`None`] — declared
/// variances are only `in`/`out`/`inout` plus the legacy default, so the
/// official `StateError` for an unknown variance (`subtype.dart:71-77`)
/// maps to this undecidable-relation result instead of a crash. The wired
/// caller decides what an undecidable pair means (conservative rejection
/// or a diagnostic); answering `true`/`false` here would silently corrupt
/// the subtype relation.
///
/// # Panics
/// Panics when the argument lists differ in length (the walk's arity
/// precondition, `subtype.dart:48-50`).
#[must_use]
pub fn arguments_related(
    sys: &TypeSystem<'_>,
    variances: &[Variance],
    sub: &[TypeId],
    sup: &[TypeId],
) -> Option<bool> {
    assert_eq!(
        sub.len(),
        sup.len(),
        "interface arity mismatch: resolution must validate argument counts"
    );
    assert!(
        variances.len() >= sub.len(),
        "variance list shorter than the argument lists"
    );
    for i in 0..sub.len() {
        let variance = variances[i];
        let sub_argument = sub[i];
        let super_argument = sup[i];
        match variance {
            Variance::Covariant => {
                if !sys.is_subtype(sub_argument, super_argument) {
                    return Some(false);
                }
            }
            Variance::Contravariant => {
                if !sys.is_subtype(super_argument, sub_argument) {
                    return Some(false);
                }
            }
            Variance::Invariant => {
                if !sys.is_subtype(sub_argument, super_argument)
                    || !sys.is_subtype(super_argument, sub_argument)
                {
                    return Some(false);
                }
            }
            // Undecidable, not a relation: see the doc comment.
            Variance::Unrelated => return None,
        }
    }
    Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hierarchy::{CoreDecls, TypeHierarchy};
    use crate::ids::TypeDeclId;
    use crate::table::TypeTable;
    use crate::ty::{UninternedFunction, UninternedType};

    const OBJECT: TypeDeclId = TypeDeclId::from_raw(1);
    const FUTURE: TypeDeclId = TypeDeclId::from_raw(2);
    const FUNCTION: TypeDeclId = TypeDeclId::from_raw(3);
    const NUM: TypeDeclId = TypeDeclId::from_raw(4);
    const INT: TypeDeclId = TypeDeclId::from_raw(5);
    const ITERABLE: TypeDeclId = TypeDeclId::from_raw(6);

    fn itf(table: &TypeTable, decl: TypeDeclId, args: &[TypeId]) -> TypeId {
        table.intern(
            UninternedType::Interface {
                decl,
                args: args.to_vec(),
            },
            false,
        )
    }

    fn fixture() -> (TypeTable, TypeHierarchy) {
        let table = TypeTable::new();
        let (num_e, iterable_e) = (TypeParamId::from_raw(1), TypeParamId::from_raw(2));
        let hierarchy = TypeHierarchy::builder(CoreDecls {
            object: OBJECT,
            future: FUTURE,
            function: FUNCTION,
            record: None,
        })
        .decl(OBJECT, Vec::new())
        .decl(FUTURE, vec![num_e])
        .decl(FUNCTION, Vec::new())
        .decl(NUM, Vec::new())
        .decl(INT, Vec::new())
        .decl(ITERABLE, vec![iterable_e])
        .supertype(NUM, itf(&table, OBJECT, &[]))
        .supertype(INT, itf(&table, NUM, &[]))
        .supertype(ITERABLE, itf(&table, OBJECT, &[]))
        .build(&table)
        .expect("fixture hierarchy is valid");
        (table, hierarchy)
    }

    #[test]
    fn lattice_operations_match_the_official_tables() {
        // combine examples from shared_type.dart:65-107.
        assert_eq!(
            Variance::Contravariant.combine(Variance::Covariant),
            Variance::Contravariant
        );
        assert_eq!(
            Variance::Covariant.combine(Variance::Contravariant),
            Variance::Contravariant
        );
        assert_eq!(
            Variance::Contravariant.combine(Variance::Contravariant),
            Variance::Covariant
        );
        assert_eq!(
            Variance::Unrelated.combine(Variance::Covariant),
            Variance::Unrelated
        );
        assert_eq!(
            Variance::Invariant.combine(Variance::Covariant),
            Variance::Invariant
        );
        // meet = index | index (shared_type.dart:129-134).
        assert_eq!(
            Variance::Covariant.meet(Variance::Contravariant),
            Variance::Invariant
        );
        // `Unrelated` is the lattice top: its encoding 0 ORs away, so the
        // meet of top with anything is that thing.
        assert_eq!(
            Variance::Unrelated.meet(Variance::Invariant),
            Variance::Invariant
        );
        assert_eq!(
            Variance::Covariant.meet(Variance::Invariant),
            Variance::Invariant
        );
        // greaterThanOrEqual (shared_type.dart:109-127).
        assert!(Variance::Unrelated.greater_or_equal(Variance::Invariant));
        assert!(Variance::Covariant.greater_or_equal(Variance::Invariant));
        assert!(!Variance::Covariant.greater_or_equal(Variance::Contravariant));
        assert!(Variance::Invariant.greater_or_equal(Variance::Invariant));
        // Keyword spellings (shared_type.dart:28-47).
        assert_eq!(Variance::Covariant.keyword(), Some("out"));
        assert_eq!(Variance::Contravariant.keyword(), Some("in"));
        assert_eq!(Variance::Invariant.keyword(), Some("inout"));
        assert_eq!(Variance::Unrelated.keyword(), None);
    }

    #[test]
    fn unregistered_parameters_read_legacy_covariant_unbounded() {
        let facts = ParamFactsTable::new();
        let p = TypeParamId::from_raw(9);
        assert_eq!(facts.get(p), ParamFacts::LEGACY);
        assert_eq!(facts.get(p).variance, Variance::Covariant);
        assert_eq!(facts.get(p).bound, None);
    }

    #[test]
    fn variance_in_type_follows_the_compute_matrix() {
        let (table, hierarchy) = fixture();
        let sys = TypeSystem::new(&table, &hierarchy);
        let mut facts = ParamFactsTable::new();
        let t = TypeParamId::from_raw(50);
        let t_ty = table.intern(UninternedType::TypeParameter { parameter: t }, false);
        // `T` itself: covariant (variance_builder.dart:58-60).
        assert_eq!(variance_in_type(&sys, &facts, t, t_ty), Variance::Covariant);
        // `T` in `num`: unrelated (unit kinds, :118).
        let num = itf(&table, NUM, &[]);
        assert_eq!(variance_in_type(&sys, &facts, t, num), Variance::Unrelated);
        // `T` in `Iterable<T>`: covariant (legacy-covariant parameter).
        let iterable_t = itf(&table, ITERABLE, &[t_ty]);
        assert_eq!(
            variance_in_type(&sys, &facts, t, iterable_t),
            Variance::Covariant
        );
        // `T` in `Function(T)`: contravariant (:142-148).
        let fn_t = table
            .intern_function(
                UninternedFunction {
                    return_type: num,
                    type_parameters: Vec::new(),
                    positional: vec![t_ty],
                    named: Vec::new(),
                    required_positional: 1,
                },
                false,
            )
            .expect("signature is well-formed");
        assert_eq!(
            variance_in_type(&sys, &facts, t, fn_t),
            Variance::Contravariant
        );
        // `T` in `List<Function(T)>`: contravariant — the docstring example
        // (shared_type.dart:70-73) via covariant.combine(contravariant);
        // not pinned separately because the Function arm above already
        // exercises the contravariant combine.
        // `T` in a bound of the function type's own parameter: invariant
        // (variance_builder.dart:131-140).
        let u = TypeParamId::from_raw(51);
        facts.set(
            u,
            ParamFacts {
                variance: Variance::Covariant,
                bound: Some(t_ty),
            },
        );
        let fn_u = table
            .intern_function(
                UninternedFunction {
                    return_type: num,
                    type_parameters: vec![u],
                    positional: Vec::new(),
                    named: Vec::new(),
                    required_positional: 0,
                },
                false,
            )
            .expect("signature is well-formed");
        assert_eq!(variance_in_type(&sys, &facts, t, fn_u), Variance::Invariant);
        // `FutureOr<T>`: covariant — the official FutureOr parameter is
        // legacy covariant (type.dart:518).
        let future_or_t = table.intern(UninternedType::FutureOr { base: t_ty }, false);
        assert_eq!(
            variance_in_type(&sys, &facts, t, future_or_t),
            Variance::Covariant
        );
    }

    #[test]
    fn arguments_related_split_matches_interface_arguments() {
        let (table, hierarchy) = fixture();
        let sys = TypeSystem::new(&table, &hierarchy);
        let num = itf(&table, NUM, &[]);
        let int = itf(&table, INT, &[]);
        // int <: num: covariant pair agrees, contravariant pair flips,
        // invariant pair accepts only the symmetric order.
        assert_eq!(
            arguments_related(&sys, &[Variance::Covariant], &[int], &[num]),
            Some(true)
        );
        assert_eq!(
            arguments_related(&sys, &[Variance::Contravariant], &[int], &[num]),
            Some(false)
        );
        assert_eq!(
            arguments_related(&sys, &[Variance::Contravariant], &[num], &[int]),
            Some(true)
        );
        assert_eq!(
            arguments_related(&sys, &[Variance::Invariant], &[int], &[int]),
            Some(true)
        );
        assert_eq!(
            arguments_related(&sys, &[Variance::Invariant], &[int], &[num]),
            Some(false)
        );
    }

    #[test]
    fn unrelated_declared_variance_answers_none() {
        let (table, hierarchy) = fixture();
        let sys = TypeSystem::new(&table, &hierarchy);
        let num = itf(&table, NUM, &[]);
        // A computed variance fed into a declared slot is the undecidable
        // shape: `None`, not a crash (the official StateError at
        // subtype.dart:71-77, structured).
        assert_eq!(
            arguments_related(&sys, &[Variance::Unrelated], &[num], &[num]),
            None
        );
    }

    /// The depth budget keeps pathological nesting from overflowing the
    /// stack: a `Unrelated`-answering trip, not a crash.
    #[test]
    fn variance_walk_budget_trips_to_unrelated() {
        let (table, hierarchy) = fixture();
        let sys = TypeSystem::new(&table, &hierarchy);
        let facts = ParamFactsTable::new();
        let t = TypeParamId::from_raw(60);
        // Nest Iterable<Iterable<…T…>> beyond the budget.
        let mut ty = table.intern(UninternedType::TypeParameter { parameter: t }, false);
        for _ in 0..(DEPTH_LIMIT as usize + 10) {
            ty = itf(&table, ITERABLE, &[ty]);
        }
        assert_eq!(variance_in_type(&sys, &facts, t, ty), Variance::Unrelated);
    }
}
