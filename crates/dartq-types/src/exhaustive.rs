//! Exhaustiveness checking over pattern spaces — the
//! `_fe_analyzer_shared` algorithm
//! (`pkg/_fe_analyzer_shared/lib/src/exhaustiveness/`), ported by
//! decision order, not by class shape.
//!
//! # Fact sources (behavior is anchored, form is native)
//!
//! * `exhaustive.dart` — `computeExhaustiveness` (:42-82) and the
//!   `_Checker` walk (:107-429): the reachability probe per case
//!   (:54-72), the guarded-case row filter (:69-71), the FIFO subtype
//!   queue (:199-264), the `Never` skip (:205-208), and the
//!   filter-and-expand recursion of `_filterByType` (:271-365). The
//!   `returnMultipleWitnesses` policy is load-bearing: the top-level
//!   witness search collects multiple witnesses while every nested probe
//!   stops at the first (the shortest-witness contract, :140-188). One
//!   deviation: the queue walk carries a visited set — the official
//!   hierarchy is cycle-validated upstream, a weaker fact source (an
//!   in-file index) is not, so a revisited type is dropped to keep the
//!   walk terminating (a revisit would only reproduce its earlier
//!   answer).
//! * `space.dart` — `Space` as a union of `SingleSpace`s (:64-131) with
//!   the construction merges: empty singles dropped, duplicates deduped,
//!   and a `Null|T` pair folded to `T?` (:93-120).
//! * `key.dart` — the record index/name key order (record index keys
//!   before named keys, :186-226). Cross-kind ordering between plain
//!   names and record names never co-occurs in one column set (a record
//!   type has only record fields, an object type only members), so the
//!   derived Rust ordering below is observationally identical.
//! * `witness.dart` + `static_type.dart` — the predicate list rebuilt
//!   into a property tree and rendered as pattern text (:51-82,
//!   :90-136); the model owns the per-type head rendering
//!   (`witnessToDart`, static_type.dart:214-239).
//!
//! # Rust form
//!
//! The official class hierarchy (`StaticType`/`NullableStaticType`/
//! `EnumStaticType`/…) becomes the [`ExhaustivenessWorld`] contract: the
//! engine is generic over the ten questions it actually asks, and each
//! backend answers them over its own type world. [`TypedModel`] is the
//! production backend over [`crate::TypeSystem`] (+ sealed/enum facts on
//! [`crate::TypeHierarchy`]); higher layers may bring narrower models
//! until the interned type wiring lands.
//!
//! The value spaces carry no list/map columns: list and map patterns
//! split their element spaces through the `keysOfInterest` channel and
//! additional-property keys (key.dart `HeadKey`/`RestKey`/`TailKey`/
//! `MapKey`), which this face does not consume yet — a list/map case
//! row arrives as the *unknown* space from the space builder and
//! suppresses (under-reports) instead of mis-answering.

use std::collections::VecDeque;
use std::fmt::Debug;
use std::hash::{Hash, Hasher};

use rustc_hash::{FxHashMap, FxHashSet};

use crate::ids::{TypeDeclId, TypeId};
use crate::system::TypeSystem;
use crate::ty::{TypeKind, UninternedType};

/// A field identity inside a matched value (official `Key`, key.dart): a
/// positional or named record field, or an object member. Ordering mirrors
/// the official `compareTo` where it can be observed: record index keys
/// sort before named keys (key.dart:186-198, :206-226), names compare
/// alphabetically within a kind.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Key {
    /// A positional record field `$1`, `$2`, … (`RecordIndexKey`).
    RecordIndex(usize),
    /// A named record field (`RecordNameKey`).
    RecordName(String),
    /// An object member (`NameKey`).
    Name(String),
}

/// The engine's view of a static type — the questions
/// `_fe_analyzer_shared`'s `StaticType` contract answers
/// (static_type.dart:11-133), reduced to the ten the checker walks.
pub trait ExhaustivenessWorld {
    /// The static type values the world hands out.
    type Ty: Clone + PartialEq + Eq + Hash + Debug;

    /// `StaticType.nullableObject` (static_type.dart:13-15) — every value's
    /// type, also the model of `dynamic` (shared.dart:202-205).
    fn object_nullable(&self) -> Self::Ty;

    /// `StaticType.neverType` (`static_type.dart:24`).
    fn never(&self) -> Self::Ty;

    /// `StaticType.nullType` (`static_type.dart:21`).
    fn null(&self) -> Self::Ty;

    /// A unique unknown static type (official `getUnknownStaticType`,
    /// shared.dart:310-320): each call yields a type equal only to itself,
    /// so a space built from it neither matches anything nor is matched by
    /// anything — the representation of patterns whose coverage the type
    /// world cannot express (relational patterns, unresolvable constants,
    /// and the list/map face).
    fn unknown(&self) -> Self::Ty;

    /// The nullable projection of `ty` (`NullableStaticType`,
    /// static_type.dart:350-353).
    fn nullable(&self, ty: &Self::Ty) -> Self::Ty;

    /// `isSubtypeOf` including the nullability and restriction relations
    /// (`static_type.dart:47`).
    fn is_subtype_of(&self, sub: &Self::Ty, sup: &Self::Ty) -> bool;

    /// `isSealed` (`static_type.dart:67`): sealed classes, enums, `bool`,
    /// `FutureOr`, and every nullable projection.
    fn is_sealed(&self, ty: &Self::Ty) -> bool;

    /// `getSubtypes` (`static_type.dart:113`): the closed subtype set `ty`
    /// splits over — subclasses of a sealed class (instantiated to this
    /// type), the constants of an enum, `true`/`false`, `T`/`Future<T>`,
    /// and `T`/`Null` for `T?`.
    fn subtypes(&self, ty: &Self::Ty) -> Vec<Self::Ty>;

    /// `getPropertyType` (`static_type.dart:36`): the static type of the
    /// field `key` exposes, or `None` when unknown (the checker then uses
    /// the nullable object as the column type, exhaustive.dart:396-406).
    fn property_type(&self, ty: &Self::Ty, key: &Key) -> Option<Self::Ty>;

    /// `ty.isSubtypeOf(NeverType)` — the queue skip test
    /// (exhaustive.dart:205-208).
    fn is_bottom(&self, ty: &Self::Ty) -> bool;

    /// `ty == StaticType.nullType` — the `Null|T` space merge test
    /// (space.dart:109-117).
    fn is_null(&self, ty: &Self::Ty) -> bool;

    /// The witness head for `ty` with the rendered `fields` (`name:
    /// pattern` pairs, in predicate-insertion order — a record backend
    /// reorders positionals first): `_` for the nullable object, `null`
    /// for `Null`, `E.c` for an enum constant, `(a, b: c)` for a record,
    /// `Name(f: …)`/`Name` otherwise (static_type.dart:214-239,
    /// types/record.dart, types/enum.dart).
    fn witness_text(&self, ty: &Self::Ty, fields: &[(Key, String)]) -> String;

    /// `typeToString` — the scrutinee text inside diagnostics.
    fn display(&self, ty: &Self::Ty) -> String;
}

/// One conjunctive component of a value set (official `SingleSpace`,
/// space.dart:17-33): a type test plus the property sub-spaces it
/// destructures.
#[derive(Clone, Debug)]
pub struct Single<M: Clone + PartialEq + Eq + Hash + Debug> {
    ty: M,
    props: Vec<(Key, Space<M>)>,
}

impl<M: Clone + PartialEq + Eq + Hash + Debug> PartialEq for Single<M> {
    /// The official `==` (space.dart:43-56): same type, same property
    /// count, pairwise equal property values. Properties are kept sorted
    /// by key, so the pairwise walk is the map comparison.
    fn eq(&self, other: &Self) -> bool {
        self.ty == other.ty
            && self.props.len() == other.props.len()
            && self
                .props
                .iter()
                .zip(other.props.iter())
                .all(|((k0, s0), (k1, s1))| k0 == k1 && s0 == s1)
    }
}

impl<M: Clone + PartialEq + Eq + Hash + Debug> Eq for Single<M> {}

impl<M: Clone + PartialEq + Eq + Hash + Debug> Hash for Single<M> {
    /// Order-insensitive property fold (the official `hashCode` hashes the
    /// key and value sets unordered, space.dart:36-40).
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.ty.hash(state);
        for (key, space) in &self.props {
            key.hash(state);
            space.hash(state);
        }
    }
}

impl<M: Clone + PartialEq + Eq + Hash + Debug> Single<M> {
    /// The empty single (official `SingleSpace.empty`, space.dart:18): the
    /// `Never` type matches no value.
    pub fn empty<W: ExhaustivenessWorld<Ty = M>>(world: &W) -> Self {
        Single {
            ty: world.never(),
            props: Vec::new(),
        }
    }

    /// A type-only single.
    pub fn full(ty: M) -> Self {
        Single {
            ty,
            props: Vec::new(),
        }
    }

    /// A single with property sub-spaces; properties are canonicalized
    /// into key order so equality and rendering are order-insensitive.
    pub fn with_props(ty: M, mut props: Vec<(Key, Space<M>)>) -> Self {
        props.sort_by(|a, b| a.0.cmp(&b.0));
        Single { ty, props }
    }

    fn is_empty<W: ExhaustivenessWorld<Ty = M>>(&self, world: &W) -> bool {
        self.ty == world.never() && self.props.is_empty()
    }

    fn prop(&self, key: &Key) -> Option<&Space<M>> {
        self.props
            .binary_search_by(|(k, _)| k.cmp(key))
            .ok()
            .map(|index| &self.props[index].1)
    }
}

/// A set of runtime values: a union of singles under one access path
/// (official `Space`, space.dart:64-131).
#[derive(Clone, Debug)]
pub struct Space<M: Clone + PartialEq + Eq + Hash + Debug> {
    /// The getter path from the scrutinee (the witness tree's addressing).
    path: Vec<Key>,
    singles: Vec<Single<M>>,
}

impl<M: Clone + PartialEq + Eq + Hash + Debug> PartialEq for Space<M> {
    fn eq(&self, other: &Self) -> bool {
        self.singles == other.singles
    }
}

impl<M: Clone + PartialEq + Eq + Hash + Debug> Eq for Space<M> {}

impl<M: Clone + PartialEq + Eq + Hash + Debug> Hash for Space<M> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for single in &self.singles {
            single.hash(state);
        }
    }
}

impl<M: Clone + PartialEq + Eq + Hash + Debug> Space<M> {
    /// The full space of `ty` at the root path.
    pub fn full(ty: M) -> Self {
        Space {
            path: Vec::new(),
            singles: vec![Single::full(ty)],
        }
    }

    /// The unknown space (official `createUnknownSpace`, shared.dart:969):
    /// a unique type that matches nothing and is matched by nothing — a
    /// row carrying it can neither prove nor refute exhaustiveness, and
    /// the scrutinee side of it suppresses the answer.
    pub fn unknown<W: ExhaustivenessWorld<Ty = M>>(world: &W) -> Self {
        Space::full(world.unknown())
    }

    /// `Space.fromSingleSpaces` (space.dart:93-120): empties dropped,
    /// duplicates deduped, all-empty collapses to the empty single, and a
    /// property-free `Null|T` pair folds to `T?`.
    pub fn from_singles<W: ExhaustivenessWorld<Ty = M>>(
        world: &W,
        path: Vec<Key>,
        singles: Vec<Single<M>>,
    ) -> Self {
        let mut kept: Vec<Single<M>> = Vec::with_capacity(singles.len());
        for single in singles {
            if single.is_empty(world) {
                continue;
            }
            if !kept.contains(&single) {
                kept.push(single);
            }
        }
        if kept.is_empty() {
            kept.push(Single::empty(world));
        } else if kept.len() == 2 {
            let fold = |world: &W, null_side: &Single<M>, other: &Single<M>| {
                world.is_null(&null_side.ty) && null_side.props.is_empty() && other.props.is_empty()
            };
            if fold(world, &kept[0], &kept[1]) || fold(world, &kept[1], &kept[0]) {
                // The merged space is the nullable projection of the
                // NON-null side (space.dart:109-117): `1` when `kept[0]`
                // is the null side, `0` otherwise.
                let other = usize::from(world.is_null(&kept[0].ty));
                let merged = Single::full(world.nullable(&kept[other].ty));
                kept = vec![merged];
            }
        }
        Space {
            path,
            singles: kept,
        }
    }

    /// The union with `other` (space.dart:122-127).
    #[must_use]
    pub fn union<W: ExhaustivenessWorld<Ty = M>>(self, world: &W, other: Space<M>) -> Self {
        let path = self.path;
        let mut singles = self.singles;
        singles.extend(other.singles);
        Space::from_singles(world, path, singles)
    }

    /// The nullable projection of this space's coverage (the null-check
    /// row widening of `createCastSpace`, shared.dart:731-735).
    #[must_use]
    pub fn union_null<W: ExhaustivenessWorld<Ty = M>>(self, world: &W) -> Self {
        self.union(world, Space::full(world.null()))
    }

    fn first_singles(&self) -> &[Single<M>] {
        &self.singles
    }
}

// —— the checker (exhaustive.dart `_Checker`) ————————————————————————————

/// One switch case: its coverage space and whether a `when` guard makes it
/// potentially non-matching (guarded rows stay out of the covering set,
/// exhaustive.dart:69-71).
#[derive(Clone, Debug)]
pub struct Case<M: Clone + PartialEq + Eq + Hash + Debug> {
    /// The case pattern's space.
    pub space: Space<M>,
    /// The case carries a `when` clause.
    pub guarded: bool,
}

/// The exhaustiveness verdict for the case list.
#[derive(Debug)]
pub enum Exhaustiveness<M: Clone + PartialEq + Eq + Hash + Debug> {
    /// Every value of the scrutinee is covered.
    Exhaustive,
    /// Uncovered values; each witness renders as a pattern
    /// (official `NonExhaustiveness`, exhaustive.dart:463-475).
    NonExhaustive(Vec<Witness<M>>),
}

/// The full reachability + exhaustiveness report for one switch.
#[derive(Debug)]
pub struct Report<M: Clone + PartialEq + Eq + Hash + Debug> {
    /// Case indexes (0-based) fully covered by earlier cases
    /// (exhaustive.dart:56-68) — the `unreachable_switch_case` sites.
    pub unreachable: Vec<usize>,
    /// The covering verdict.
    pub exhaustiveness: Exhaustiveness<M>,
}

/// A predicate recorded along the walk (official `Predicate`,
/// witness.dart:13-31): the column path, the enclosing static type, and
/// the type the predicate splits over.
#[derive(Clone, Debug)]
struct Predicate<M: Clone + PartialEq + Eq + Hash + Debug> {
    path: Vec<Key>,
    context: M,
    value: M,
}

/// A witness: the pattern text of one value that no case matches
/// (official `Witness`, witness.dart:51-82).
#[derive(Clone, Debug)]
pub struct Witness<M: Clone + PartialEq + Eq + Hash + Debug> {
    predicates: Vec<Predicate<M>>,
}

impl<M: Clone + PartialEq + Eq + Hash + Debug> Witness<M> {
    /// The pattern text including all subproperties (`asWitness`).
    pub fn as_witness<W: ExhaustivenessWorld<Ty = M>>(&self, world: &W) -> String {
        self.render(world, false)
    }

    /// The pattern text with subproperties that fully match their static
    /// type omitted (`asCorrection`).
    pub fn as_correction<W: ExhaustivenessWorld<Ty = M>>(&self, world: &W) -> String {
        self.render(world, true)
    }

    fn render<W: ExhaustivenessWorld<Ty = M>>(&self, world: &W, for_correction: bool) -> String {
        let mut root = PropertyWitness::new(world);
        for predicate in &self.predicates {
            let mut here = &mut root;
            for key in &predicate.path {
                let position = here.props.iter().position(|(k, _)| k == key);
                let index = if let Some(index) = position {
                    index
                } else {
                    here.props.push((key.clone(), PropertyWitness::new(world)));
                    here.props.len() - 1
                };
                here = &mut here.props[index].1;
            }
            here.static_ty = Some(predicate.context.clone());
            here.value_ty = predicate.value.clone();
        }
        root.render(world, for_correction)
    }
}

/// The rebuilt property tree of a witness (official `PropertyWitness`,
/// witness.dart:85-146). `static_ty` stays `None` on intermediate path
/// nodes, where the official model carries the `object_nullable` default —
/// exactly what [`Self::is_trivial`] then answers.
struct PropertyWitness<M: Clone + PartialEq + Eq + Hash + Debug> {
    static_ty: Option<M>,
    value_ty: M,
    props: Vec<(Key, PropertyWitness<M>)>,
}

impl<M: Clone + PartialEq + Eq + Hash + Debug> PropertyWitness<M> {
    fn new<W: ExhaustivenessWorld<Ty = M>>(world: &W) -> Self {
        PropertyWitness {
            static_ty: None,
            value_ty: world.object_nullable(),
            props: Vec::new(),
        }
    }

    /// `PropertyWitness.witnessToDart` (witness.dart:90-136): fielded
    /// witnesses render through the value type's head; correction mode
    /// drops trivial fields and falls back to the bare head when all drop.
    fn render<W: ExhaustivenessWorld<Ty = M>>(&self, world: &W, for_correction: bool) -> String {
        if self.props.is_empty() {
            return world.witness_text(&self.value_ty, &[]);
        }
        let fields: Vec<(Key, String)> = self
            .props
            .iter()
            .filter(|(_, child)| !for_correction || !child.is_trivial(world))
            .map(|(key, child)| (key.clone(), child.render(world, for_correction)))
            .collect();
        if fields.is_empty() && for_correction {
            world.witness_text(&self.value_ty, &[])
        } else {
            world.witness_text(&self.value_ty, &fields)
        }
    }

    /// `isTrivial` (witness.dart:138-146): the child's coverage equals its
    /// column's static type, all the way down. The official default
    /// (`object_nullable` on both sides) is trivial by construction, which
    /// the `None` case encodes directly.
    fn is_trivial<W: ExhaustivenessWorld<Ty = M>>(&self, world: &W) -> bool {
        let typed = match &self.static_ty {
            Some(static_ty) => world.is_subtype_of(static_ty, &self.value_ty),
            None => world.is_subtype_of(&world.object_nullable(), &self.value_ty),
        };
        typed && self.props.iter().all(|(_, child)| child.is_trivial(world))
    }
}

type Row<M> = Vec<Space<M>>;

/// `computeExhaustiveness` (exhaustive.dart:42-82): the per-case
/// reachability probe, then the multi-witness covering search over the
/// unguarded rows.
pub fn compute<M: Clone + PartialEq + Eq + Hash + Debug, W: ExhaustivenessWorld<Ty = M>>(
    world: &W,
    value: &M,
    cases: &[Case<M>],
) -> Report<M> {
    let mut unreachable = Vec::new();
    let mut case_rows: Vec<Row<M>> = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let row: Row<M> = vec![case.space.clone()];
        if index > 0 && unmatched(world, &case_rows, &row, &[], false).is_none() {
            unreachable.push(index);
        }
        if !case.guarded {
            case_rows.push(row);
        }
    }
    let scrutinee = [Space::full(value.clone())];
    let witnesses = unmatched(world, &case_rows, &scrutinee, &[], true);
    Report {
        unreachable,
        exhaustiveness: match witnesses {
            None => Exhaustiveness::Exhaustive,
            Some(witnesses) => Exhaustiveness::NonExhaustive(witnesses),
        },
    }
}

/// `_Checker._unmatched` (exhaustive.dart:118-269), decision order
/// verbatim: empty columns settle by remaining rows; an empty row set
/// yields the shortest witness unless multiple witnesses are wanted; the
/// first column walks a FIFO subtype queue, expanding sealed types whose
/// filter did not fully cover them and collecting witnesses elsewhere.
fn unmatched<M: Clone + PartialEq + Eq + Hash + Debug, W: ExhaustivenessWorld<Ty = M>>(
    world: &W,
    case_rows: &[Row<M>],
    value_patterns: &[Space<M>],
    predicates: &[Predicate<M>],
    multiple: bool,
) -> Option<Vec<Witness<M>>> {
    // Row/column invariant (exhaustive.dart:121-125): every row spans the
    // same columns as the value patterns.
    debug_assert!(
        case_rows
            .iter()
            .all(|row| row.len() == value_patterns.len()),
        "row/column mismatch: {} value patterns, rows {:?}",
        value_patterns.len(),
        case_rows.iter().map(Row::len).collect::<Vec<_>>(),
    );
    if value_patterns.is_empty() {
        // No columns left: surviving rows absorb every remaining value;
        // no rows at all means the predicate stack describes a witness.
        return if case_rows.is_empty() {
            Some(vec![Witness {
                predicates: predicates.to_vec(),
            }])
        } else {
            None
        };
    }
    if case_rows.is_empty() && !multiple {
        return Some(vec![Witness {
            predicates: predicates.to_vec(),
        }]);
    }

    let first = &value_patterns[0];

    // Cycle guard, reused across the singles' walks (`clear` keeps the
    // allocation; visits are not shared between walks because a type's
    // expansion depends on `single`). `subtypes` feeds a FIFO expansion,
    // and a fact source whose sealed closure is not well-founded (an
    // in-file `sealed S extends S`, or a mutual A/B cycle a file-scoped
    // index cannot reject) would re-enqueue the same type forever. The
    // official walk (exhaustive.dart:209-228) needs no guard because the
    // oracle's hierarchy is cycle-validated upstream; this one guards
    // itself. Dropping a revisit is observationally free: within one
    // walk a type's expansion is a pure function of fixed inputs (rows,
    // single, predicates), so the repeated visit would only reproduce
    // its earlier answer. Progress: every iteration either processes a
    // fresh type or stops — the walk is bounded by the finite reachable
    // type set.
    let mut visited: FxHashSet<M> = FxHashSet::default();
    for single in first.first_singles() {
        // Witnesses are per single space (exhaustive.dart:202): the first
        // single that yields any settles the answer (:261-263).
        let mut witnesses: Option<Vec<Witness<M>>> = None;
        let context_ty = single.ty.clone();
        let mut stack: VecDeque<M> = VecDeque::new();
        stack.push_back(context_ty.clone());
        visited.clear();
        while let Some(ty) = stack.pop_front() {
            if !visited.insert(ty.clone()) {
                continue;
            }
            if world.is_bottom(&ty) {
                // Don't try to exhaust the Never type (exhaustive.dart:205-208).
                continue;
            }
            if world.is_sealed(&ty) {
                let result = filter_by_type(
                    world,
                    &context_ty,
                    &ty,
                    case_rows,
                    single,
                    value_patterns,
                    predicates,
                    &first.path,
                    false,
                );
                if result.is_some() {
                    // Not fully handled: split over the closed subtypes.
                    for subtype in world.subtypes(&ty) {
                        stack.push_back(subtype);
                    }
                }
            } else {
                let result = filter_by_type(
                    world,
                    &context_ty,
                    &ty,
                    case_rows,
                    single,
                    value_patterns,
                    predicates,
                    &first.path,
                    false,
                );
                if let Some(result) = result {
                    witnesses.get_or_insert_with(Vec::new).extend(result);
                    if !multiple {
                        return witnesses;
                    }
                }
            }
        }
        if witnesses.is_some() {
            return witnesses;
        }
    }
    None
}

/// `_Checker._filterByType` (exhaustive.dart:271-365): keep the rows that
/// *must* match `ty` (their single's type is a supertype), lift every
/// interested property into its own column, and recurse.
#[expect(
    clippy::too_many_arguments,
    reason = "parameters map 1:1 onto the official _Checker._filterByType recursion locals; \
              grouping them would leave the port mapping"
)]
fn filter_by_type<M: Clone + PartialEq + Eq + Hash + Debug, W: ExhaustivenessWorld<Ty = M>>(
    world: &W,
    context_ty: &M,
    ty: &M,
    case_rows: &[Row<M>],
    value_single: &Single<M>,
    value_patterns: &[Space<M>],
    predicates: &[Predicate<M>],
    path: &[Key],
    multiple: bool,
) -> Option<Vec<Witness<M>>> {
    let mut extended = predicates.to_vec();
    extended.push(Predicate {
        path: path.to_vec(),
        context: context_ty.clone(),
        value: ty.clone(),
    });

    // Rows whose first single's type is a supertype of `ty` must match.
    let mut remaining_rows: Vec<Row<M>> = Vec::new();
    let mut remaining_singles: Vec<&Single<M>> = Vec::new();
    for row in case_rows {
        for single in row[0].first_singles() {
            if world.is_subtype_of(ty, &single.ty) {
                remaining_singles.push(single);
                remaining_rows.push(row.clone());
            }
        }
    }

    // The union of the value's and the surviving rows' property keys,
    // sorted for deterministic column order (exhaustive.dart:313-328).
    let mut property_keys: Vec<Key> = Vec::new();
    for (key, _) in &value_single.props {
        property_keys.push(key.clone());
    }
    for single in &remaining_singles {
        for (key, _) in &single.props {
            property_keys.push(key.clone());
        }
    }
    property_keys.sort();
    property_keys.dedup();

    let mut value_spaces = expand_properties(world, &property_keys, value_single, ty, path);
    value_spaces.extend(value_patterns.iter().skip(1).cloned());

    for (row, single) in remaining_rows.iter_mut().zip(remaining_singles) {
        let mut spaces = expand_properties(world, &property_keys, single, &single.ty, path);
        spaces.extend(row.iter().skip(1).cloned());
        *row = spaces;
    }

    unmatched(world, &remaining_rows, &value_spaces, &extended, multiple)
}

/// `_Checker._expandProperties` (exhaustive.dart:380-429): one column per
/// interested key — the single's sub-space when it tests the property,
/// otherwise the full space of the property's static type (nullable object
/// when the type world does not know it).
fn expand_properties<M: Clone + PartialEq + Eq + Hash + Debug, W: ExhaustivenessWorld<Ty = M>>(
    world: &W,
    keys: &[Key],
    single: &Single<M>,
    ty: &M,
    path: &[Key],
) -> Vec<Space<M>> {
    let mut result = Vec::with_capacity(keys.len());
    for key in keys {
        let space = match single.prop(key) {
            Some(space) => space.clone(),
            None => Space::full(
                world
                    .property_type(ty, key)
                    .unwrap_or_else(|| world.object_nullable()),
            ),
        };
        let mut child_path = path.to_vec();
        child_path.push(key.clone());
        result.push(Space {
            path: child_path,
            singles: space.singles,
        });
    }
    result
}

// —— the production backend over the interned type system —————————————————

/// The static type the [`TypedModel`] walks. The official class hierarchy
/// (`StaticType` → `NullableStaticType` → `TypeBasedStaticType` →
/// `SealedClassStaticType`/`EnumStaticType`/`RecordStaticType`/
/// `FutureOrStaticType`/`BoolStaticType`/restricted values) becomes one
/// enum: the nullable split is a variant, the type-based leaves carry their
/// restriction as a kind, and the two `Object` projections are explicit so
/// the world's type constructors ([`ExhaustivenessWorld::object_nullable`],
/// [`ExhaustivenessWorld::unknown`]) need no hierarchy access.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum TypedTy {
    /// `StaticType.neverType`.
    Never,
    /// `StaticType.nullType`.
    Null,
    /// `StaticType.nullableObject` — every value's type; also the model of
    /// `dynamic` (shared.dart:202-205).
    TopNullable,
    /// `StaticType.nonNullableObject` (`static_type.dart:18`).
    TopNonNullable,
    /// `NullableStaticType(underlying)`.
    Nullable(Box<TypedTy>),
    /// A non-nullable type-based static type with its restriction.
    Base(BaseTy),
    /// A fresh unknown (official `getUnknownStaticType`, shared.dart:310):
    /// unique per id, matched by nothing.
    Unknown(u64),
}

/// The non-nullable leaves. `BaseKind` is the restriction face the subtype
/// rule consults (types.dart `Restriction`).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct BaseTy {
    ty: TypeId,
    kind: BaseKind,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum BaseKind {
    /// `TypeBasedStaticType` (unrestricted).
    Plain,
    /// `SealedClassStaticType` — `isSealed` with subclass splitting.
    Sealed,
    /// `EnumStaticType` — `isSealed` with constant splitting.
    Enum,
    /// `BoolStaticType` — `isSealed` with the `true`/`false` split
    /// (types/bool.dart:15-16: `true` and `false` are the only
    /// inhabitants).
    Bool,
    /// `EnumElementStaticType` — unique per constant index.
    EnumValue(u32),
    /// `RecordStaticType`.
    Record,
    /// `FutureOrStaticType` — `isSealed` over `T`/`Future<T>`.
    FutureOr,
    /// `_BoolValueStaticType` — unique per value.
    BoolValue(bool),
}

impl BaseTy {
    /// The restriction identity, if this leaf carries one
    /// (types.dart `IdentityRestriction`).
    fn identity(&self) -> Option<u64> {
        match self.kind {
            BaseKind::EnumValue(value) => Some(2 + u64::from(value) * 2),
            BaseKind::BoolValue(value) => Some(1 | u64::from(value)),
            BaseKind::Plain
            | BaseKind::Sealed
            | BaseKind::Enum
            | BaseKind::Bool
            | BaseKind::Record
            | BaseKind::FutureOr => None,
        }
    }
}

/// Process-wide uniqueness source for [`TypedTy::Unknown`] (the official
/// `new Object()` identity token, shared.dart:315-319).
fn next_unknown() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Display names for witnesses and diagnostics: declaration names and the
/// constant names of enums. The type world's declarations are opaque
/// [`TypeDeclId`]s, so display text is injected; production callers fill
/// both from the element model.
#[derive(Default)]
pub struct TypedNames {
    decls: FxHashMap<TypeDeclId, String>,
    enum_constants: FxHashMap<TypeDeclId, Vec<String>>,
}

impl TypedNames {
    /// Names a declaration.
    pub fn decl(&mut self, decl: TypeDeclId, name: impl Into<String>) -> &mut Self {
        self.decls.insert(decl, name.into());
        self
    }

    /// Names an enum's constants, in declaration order (the witness text
    /// and the subtype order both follow it).
    pub fn enum_constants(&mut self, decl: TypeDeclId, constants: Vec<String>) -> &mut Self {
        self.enum_constants.insert(decl, constants);
        self
    }
}

/// The [`ExhaustivenessWorld`] over the interned type system: subtype queries
/// delegate to [`TypeSystem`], sealed/enum facts to [`crate::TypeHierarchy`],
/// record fields to the interned shapes.
pub struct TypedModel<'a> {
    sys: &'a TypeSystem<'a>,
    names: TypedNames,
}

impl<'a> TypedModel<'a> {
    /// Builds a model with empty display names (witnesses render
    /// `<decl N>` placeholders).
    pub fn new(sys: &'a TypeSystem<'a>) -> Self {
        TypedModel {
            sys,
            names: TypedNames::default(),
        }
    }

    /// Builds a model with display names.
    pub fn with_names(sys: &'a TypeSystem<'a>, names: TypedNames) -> Self {
        TypedModel { sys, names }
    }

    /// `ExhaustivenessCache.getStaticType` (shared.dart:194-298): unit
    /// kinds first, then the null split, then the record/`FutureOr`/enum/
    /// sealed dispatch on the non-null projection.
    #[must_use]
    pub fn static_type(&self, ty: TypeId) -> TypedTy {
        match self.sys.table().get(ty).kind() {
            TypeKind::Never => return TypedTy::Never,
            TypeKind::Null => return TypedTy::Null,
            // `dynamic` models as the nullable object (shared.dart:202);
            // `void`/`Invalid` keep their own unrestricted type so their
            // (already erroneous) switches under-report instead of
            // mis-answering.
            TypeKind::Dynamic => return TypedTy::TopNullable,
            _ => {}
        }
        // The nullable projection wraps the *non-null* type's static type
        // (shared.dart:237-244): strip the nullability suffix first, or the
        // base would carry a nullable instantiation and mismatch every
        // non-null twin in subtype/restriction queries.
        let base = self.base_static_type(self.sys.strip(ty));
        if self.sys.is_nullable(ty) {
            TypedTy::Nullable(Box::new(base))
        } else {
            base
        }
    }

    /// The non-null projection's static type, in the `getStaticType`
    /// dispatch order (shared.dart:217-291).
    fn base_static_type(&self, ty: TypeId) -> TypedTy {
        match self.sys.table().get(ty).kind() {
            TypeKind::Record { .. } => Self::base(ty, BaseKind::Record),
            TypeKind::FutureOr { .. } => Self::base(ty, BaseKind::FutureOr),
            TypeKind::Interface { decl, .. } => {
                if self.is_bool_decl(decl) {
                    // `BoolStaticType` (types/bool.dart): sealed over its
                    // two value constants.
                    return Self::base(ty, BaseKind::Bool);
                }
                if self.sys.hierarchy().enum_constant_count(decl).is_some() {
                    return Self::base(ty, BaseKind::Enum);
                }
                if self.sys.hierarchy().is_sealed(decl) {
                    return Self::base(ty, BaseKind::Sealed);
                }
                Self::base(ty, BaseKind::Plain)
            }
            _ => Self::base(ty, BaseKind::Plain),
        }
    }

    fn base(ty: TypeId, kind: BaseKind) -> TypedTy {
        TypedTy::Base(BaseTy { ty, kind })
    }

    /// The static type of the `index`th constant of the enum `decl` — the
    /// case space of a constant pattern `E.c` (`EnumElementStaticType`,
    /// types/enum.dart). The index is the constant's declaration position.
    #[must_use]
    pub fn enum_constant(&self, decl: TypeDeclId, index: u32) -> TypedTy {
        Self::base(
            self.sys.table().intern(
                UninternedType::Interface {
                    decl,
                    args: Vec::new(),
                },
                false,
            ),
            BaseKind::EnumValue(index),
        )
    }

    fn is_bool_decl(&self, decl: TypeDeclId) -> bool {
        self.names.decls.get(&decl).map(String::as_str) == Some("bool")
    }

    fn display_name(&self, decl: TypeDeclId) -> String {
        self.names
            .decls
            .get(&decl)
            .cloned()
            .unwrap_or_else(|| format!("<decl {}>", decl.raw()))
    }

    /// The declaration's "this-type": its instantiation over its own type
    /// parameters.
    fn this_type(&self, decl: TypeDeclId) -> TypeId {
        let info = self
            .sys
            .hierarchy()
            .decl(decl)
            .expect("hierarchy declarations are declared");
        let args: Vec<TypeId> = info
            .type_parameters
            .iter()
            .map(|&param| {
                self.sys
                    .table()
                    .intern(UninternedType::TypeParameter { parameter: param }, false)
            })
            .collect();
        self.sys
            .table()
            .intern(UninternedType::Interface { decl, args }, false)
    }

    /// `AnalyzerSealedClassOperations.getSubclassAsInstanceOf`, the
    /// trivial-substitution path only (M2 has no declared bounds, so the
    /// official overapproximation fallback keeps the subclass's raw
    /// this-type — the wrapped subtype relation then rests on the real
    /// supertype edges): the subclass's direct edge to this sealed class
    /// must instantiate its own type parameters positionally, and the
    /// arities must align with the scrutinee's arguments.
    fn subclass_instantiation(&self, sub: TypeDeclId, sealed_ty: TypeId) -> TypeId {
        let TypeKind::Interface {
            decl: sealed_decl,
            args: sealed_args,
        } = self.sys.table().get(sealed_ty).kind()
        else {
            return self.this_type(sub);
        };
        let sub_params = self
            .sys
            .hierarchy()
            .decl(sub)
            .expect("declared subclass")
            .type_parameters
            .clone();
        if sub_params.len() != sealed_args.len() {
            return self.this_type(sub);
        }
        // The subclass's direct edge to the sealed declaration, its template
        // instantiated with the subclass's own parameters.
        let info = self.sys.hierarchy().decl(sub).expect("declared subclass");
        for edge in info.supertypes.clone() {
            if let TypeKind::Interface {
                decl: edge_decl,
                args: edge_args,
            } = self.sys.table().get(edge).kind()
                && edge_decl == sealed_decl
                && edge_args.len() == sub_params.len()
                && (0..edge_args.len()).all(|i| {
                    self.sys.table().arg(edge_args.offset() + i)
                        == self.sys.table().intern(
                            UninternedType::TypeParameter {
                                parameter: sub_params[i],
                            },
                            false,
                        )
                })
            {
                let concrete: Vec<TypeId> = (0..sealed_args.len())
                    .map(|i| self.sys.table().arg(sealed_args.offset() + i))
                    .collect();
                return self.sys.table().intern(
                    UninternedType::Interface {
                        decl: sub,
                        args: concrete,
                    },
                    false,
                );
            }
        }
        self.this_type(sub)
    }

    /// Witness/display text for a record type. Witness mode (non-empty
    /// `fields`) renders present sub-witnesses and `_` for the rest
    /// (types/record.dart `witnessToDart`); display mode renders the field
    /// types (`spaceToText`).
    fn record_text(&self, ty: TypeId, fields: &[(Key, String)]) -> String {
        let TypeKind::Record { shape } = self.sys.table().get(ty).kind() else {
            return self.display_type(ty);
        };
        let shape = self.sys.table().record_shape(shape);
        let witness_mode = !fields.is_empty();
        let lookup = |key: &Key| -> Option<&String> {
            fields.iter().find(|(k, _)| k == key).map(|(_, text)| text)
        };
        let mut text = String::from("(");
        for index in 0..shape.positional.len() {
            if index > 0 {
                text.push_str(", ");
            }
            let key = Key::RecordIndex(index);
            match lookup(&key) {
                Some(rendered) => text.push_str(rendered),
                None if witness_mode => text.push('_'),
                None => {
                    let field =
                        self.static_type(self.sys.table().arg(shape.positional.offset() + index));
                    text.push_str(&self.display(&field));
                }
            }
        }
        for slot in 0..shape.named.len() {
            if slot > 0 || !shape.positional.is_empty() {
                text.push_str(", ");
            }
            let name = self.sys.table().named_name(shape.named.offset() + slot);
            text.push_str(&name);
            text.push_str(": ");
            let key = Key::RecordName(name.clone());
            match lookup(&key) {
                Some(rendered) => text.push_str(rendered),
                None if witness_mode => text.push('_'),
                None => {
                    let field =
                        self.static_type(self.sys.table().named_ty(shape.named.offset() + slot));
                    text.push_str(&self.display(&field));
                }
            }
        }
        text.push(')');
        text
    }

    /// `typeToString` for an interned type.
    fn display_type(&self, ty: TypeId) -> String {
        match self.sys.table().get(ty).kind() {
            TypeKind::Never => "Never".to_string(),
            TypeKind::Dynamic => "dynamic".to_string(),
            TypeKind::Void => "void".to_string(),
            TypeKind::Invalid => "Invalid".to_string(),
            TypeKind::Null => "Null".to_string(),
            TypeKind::Interface { decl, .. } => self.display_name(decl),
            TypeKind::TypeParameter { .. } => "T".to_string(),
            TypeKind::Function { .. } => "Function".to_string(),
            TypeKind::FutureOr { base } => format!("FutureOr<{}>", self.display_type(base)),
            TypeKind::Record { .. } => self.record_text(ty, &[]),
        }
    }
}

impl ExhaustivenessWorld for TypedModel<'_> {
    type Ty = TypedTy;

    fn object_nullable(&self) -> TypedTy {
        TypedTy::TopNullable
    }

    fn never(&self) -> TypedTy {
        TypedTy::Never
    }

    fn null(&self) -> TypedTy {
        TypedTy::Null
    }

    fn unknown(&self) -> TypedTy {
        TypedTy::Unknown(next_unknown())
    }

    fn nullable(&self, ty: &TypedTy) -> TypedTy {
        match ty {
            TypedTy::Nullable(_) | TypedTy::Null | TypedTy::TopNullable => ty.clone(),
            // `Never?` is `Null` (type.dart:1207-1209).
            TypedTy::Never => TypedTy::Null,
            TypedTy::TopNonNullable => TypedTy::TopNullable,
            other @ (TypedTy::Base(_) | TypedTy::Unknown(_)) => {
                TypedTy::Nullable(Box::new(other.clone()))
            }
        }
    }

    fn is_subtype_of(&self, sub: &TypedTy, sup: &TypedTy) -> bool {
        if sub == sup {
            return true;
        }
        // All types are subtypes of `Object?` (static_type.dart:166-168).
        if matches!(sup, TypedTy::TopNullable) {
            return true;
        }
        match sub {
            // Never subtypes everything (static_type.dart:290-294).
            TypedTy::Never => true,
            // `Null <: T?` through the `Never` underlying
            // (static_type.dart:365-372).
            TypedTy::Null => matches!(sup, TypedTy::Nullable(_)),
            // `S? <: T?` iff `S <: T` (static_type.dart:365-372).
            TypedTy::Nullable(inner) => match sup {
                TypedTy::Nullable(o) => self.is_subtype_of(inner, o),
                _ => false,
            },
            // `Object` accepts every non-null type (static_type.dart:425-427).
            TypedTy::Base(base) => match sup {
                TypedTy::TopNonNullable => true,
                TypedTy::Nullable(inner) => self.base_is_subtype_of(base, inner),
                TypedTy::Base(o) => self.sys.is_subtype(base.ty, o.ty) && restriction_sub(base, o),
                _ => false,
            },
            // `Object?` only subtypes itself (the early return); the unknown
            // matches nothing (shared.dart:310-320); `Object` only itself.
            _ => false,
        }
    }

    fn is_sealed(&self, ty: &TypedTy) -> bool {
        match ty {
            // NullableStaticType.isSealed (static_type.dart:356).
            TypedTy::Nullable(_) | TypedTy::TopNullable => true,
            TypedTy::Base(base) => matches!(
                base.kind,
                BaseKind::Sealed | BaseKind::Enum | BaseKind::Bool | BaseKind::FutureOr
            ),
            _ => false,
        }
    }

    fn subtypes(&self, ty: &TypedTy) -> Vec<TypedTy> {
        match ty {
            // NullableStaticType.getSubtypes (static_type.dart:359-362).
            TypedTy::Nullable(inner) => vec![(**inner).clone(), TypedTy::Null],
            TypedTy::TopNullable => vec![TypedTy::TopNonNullable, TypedTy::Null],
            TypedTy::Base(base) => match base.kind {
                // SealedClassStaticType._createSubtypes (types/sealed.dart):
                // direct subclasses, non-generic ones filtered to those that
                // can inhabit this instantiation.
                BaseKind::Sealed => {
                    let TypeKind::Interface { decl, .. } = self.sys.table().get(base.ty).kind()
                    else {
                        return Vec::new();
                    };
                    let Some(subclasses) = self.sys.hierarchy().direct_subclasses(decl) else {
                        return Vec::new();
                    };
                    let mut result = Vec::new();
                    for &sub in subclasses {
                        let sub_ty = self.subclass_instantiation(sub, base.ty);
                        let generic = matches!(
                            self.sys.table().get(sub_ty).kind(),
                            TypeKind::Interface { args, .. } if !args.is_empty()
                        );
                        // Non-generic subtypes must inhabit the instantiation;
                        // generic ones stay (the official overapproximation
                        // fallback keeps them, types/sealed.dart:60-88).
                        if !generic && !self.sys.is_subtype(sub_ty, base.ty) {
                            continue;
                        }
                        result.push(self.base_static_type(sub_ty));
                    }
                    result
                }
                // EnumStaticType (types/enum.dart): one unique value per
                // constant, declaration order.
                BaseKind::Enum => {
                    let TypeKind::Interface { decl, .. } = self.sys.table().get(base.ty).kind()
                    else {
                        return Vec::new();
                    };
                    match self.names.enum_constants.get(&decl) {
                        None => Vec::new(),
                        // The hierarchy caps enum arity at u16, so the
                        // widening conversion cannot truncate.
                        Some(constants) => (0..u32::try_from(constants.len()).unwrap_or(u32::MAX))
                            .map(|index| Self::base(base.ty, BaseKind::EnumValue(index)))
                            .collect(),
                    }
                }
                // BoolStaticType (types/bool.dart:15-16): `true` and
                // `false` are the only inhabitants.
                BaseKind::Bool => vec![
                    Self::base(base.ty, BaseKind::BoolValue(true)),
                    Self::base(base.ty, BaseKind::BoolValue(false)),
                ],
                // FutureOrStaticType (types/future_or.dart): T | Future<T>.
                BaseKind::FutureOr => {
                    let TypeKind::FutureOr { base: argument } =
                        self.sys.table().get(base.ty).kind()
                    else {
                        return Vec::new();
                    };
                    let future = self.sys.intern_future(argument);
                    vec![self.static_type(argument), self.static_type(future)]
                }
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    fn property_type(&self, ty: &TypedTy, key: &Key) -> Option<TypedTy> {
        let TypedTy::Base(base) = ty else {
            return None;
        };
        let TypeKind::Record { shape } = self.sys.table().get(base.ty).kind() else {
            // Field-opaque outside records: the checker's columns then type
            // as the nullable object (exhaustive.dart:396-406).
            return None;
        };
        let shape = self.sys.table().record_shape(shape);
        match key {
            Key::RecordIndex(index) => (*index < shape.positional.len())
                .then(|| self.static_type(self.sys.table().arg(shape.positional.offset() + index))),
            Key::RecordName(name) => (0..shape.named.len())
                .map(|slot| {
                    (
                        slot,
                        self.sys.table().named_name(shape.named.offset() + slot),
                    )
                })
                .find(|(_, at)| at == name.as_str())
                .map(|(slot, _)| {
                    self.static_type(self.sys.table().named_ty(shape.named.offset() + slot))
                }),
            Key::Name(_) => None,
        }
    }

    fn is_bottom(&self, ty: &TypedTy) -> bool {
        match ty {
            TypedTy::Never => true,
            TypedTy::Base(base) => {
                let never = self.sys.table().intern(UninternedType::Never, false);
                self.sys.is_subtype(base.ty, never)
            }
            _ => false,
        }
    }

    fn is_null(&self, ty: &TypedTy) -> bool {
        matches!(ty, TypedTy::Null)
    }

    fn witness_text(&self, ty: &TypedTy, fields: &[(Key, String)]) -> String {
        match ty {
            TypedTy::TopNullable if fields.is_empty() => "_".to_string(),
            TypedTy::TopNullable => format!("Object?({})", render_fields(fields)),
            TypedTy::TopNonNullable if fields.is_empty() => "Object".to_string(),
            TypedTy::TopNonNullable => format!("Object({})", render_fields(fields)),
            TypedTy::Null if fields.is_empty() => "null".to_string(),
            TypedTy::Null => format!("null({})", render_fields(fields)),
            TypedTy::Never => "Never".to_string(),
            TypedTy::Unknown(_) => "?".to_string(),
            // Nullable heads render the underlying head, then `?`, then the
            // fielded parens (static_type.dart:397-402 + :214-239).
            TypedTy::Nullable(inner) => {
                let head = self.witness_text(inner, &[]);
                if fields.is_empty() {
                    format!("{head}?")
                } else {
                    format!("{head}?({})", render_fields(fields))
                }
            }
            TypedTy::Base(base) => match base.kind {
                BaseKind::EnumValue(index) => {
                    let TypeKind::Interface { decl, .. } = self.sys.table().get(base.ty).kind()
                    else {
                        return "E".to_string();
                    };
                    let constant = self
                        .names
                        .enum_constants
                        .get(&decl)
                        .and_then(|constants| constants.get(index as usize))
                        .map_or("?", String::as_str);
                    format!("{}.{constant}", self.display_name(decl))
                }
                BaseKind::BoolValue(value) => value.to_string(),
                _ => {
                    if let TypeKind::Record { .. } = self.sys.table().get(base.ty).kind() {
                        return self.record_text(base.ty, fields);
                    }
                    let head = self.display_type(base.ty);
                    if fields.is_empty() {
                        head
                    } else {
                        format!("{head}({})", render_fields(fields))
                    }
                }
            },
        }
    }

    fn display(&self, ty: &TypedTy) -> String {
        match ty {
            TypedTy::TopNullable => "Object?".to_string(),
            TypedTy::TopNonNullable => "Object".to_string(),
            TypedTy::Null => "Null".to_string(),
            TypedTy::Never => "Never".to_string(),
            TypedTy::Unknown(_) => "?".to_string(),
            TypedTy::Nullable(inner) => format!("{}?", self.display(inner)),
            TypedTy::Base(base) => self.display_type(base.ty),
        }
    }
}

impl TypedModel<'_> {
    /// The non-null subtype rule (static_type.dart:422-437 + the
    /// `isSubtypeOfInternal` restriction check, types.dart:52-56): a `Base`
    /// subtypes a nullable target by peeling nullability, and subtypes a
    /// non-null `Object` target unless it is `Null` itself.
    fn base_is_subtype_of(&self, base: &BaseTy, sup: &TypedTy) -> bool {
        match sup {
            TypedTy::Base(o) if self.is_object_none(o.ty) => !self.is_null_type(base.ty),
            TypedTy::Nullable(inner) => self.base_is_subtype_of(base, inner),
            TypedTy::Base(o) => self.sys.is_subtype(base.ty, o.ty) && restriction_sub(base, o),
            _ => false,
        }
    }

    fn is_object_none(&self, ty: TypeId) -> bool {
        matches!(self.sys.table().get(ty).kind(), TypeKind::Interface { decl, .. } if decl == self.sys.hierarchy().core().object)
            && !self.sys.is_nullable(ty)
    }

    fn is_null_type(&self, ty: TypeId) -> bool {
        matches!(self.sys.table().get(ty).kind(), TypeKind::Null)
    }
}

/// `Restriction.isSubtypeOf` (types.dart:224-258): an identity subtypes
/// unrestricted or itself; unrestricted subtypes unrestricted only.
fn restriction_sub(sub: &BaseTy, sup: &BaseTy) -> bool {
    match (sub.identity(), sup.identity()) {
        (_, None) => true,
        (None, Some(_)) => false,
        (Some(a), Some(b)) => a == b,
    }
}

/// `name: pattern` pairs joined for a fielded witness head.
fn render_fields(fields: &[(Key, String)]) -> String {
    fields
        .iter()
        .map(|(key, text)| match key {
            Key::RecordIndex(index) => format!("${}:{text}", index + 1),
            Key::RecordName(name) | Key::Name(name) => format!("{name}: {text}"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}
