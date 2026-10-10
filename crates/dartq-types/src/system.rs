//! The subtype relation, assignability, and nullability predicates.
//!
//! Direct port of the analyzer's rule table
//! (`pkg/analyzer/lib/src/dart/element/subtype.dart:262-523`): the rule order
//! below is the source order, and short-circuiting matches it exactly.
//! M1 omissions (spec §5) are marked inline: no promoted bounds, no declared
//! bounds on type parameters, no variance annotations (class parameters are
//! covariant, `element.dart:11560-11561`), no extension types, no
//! `UnknownInferredType`.

use std::cell::{Cell, RefCell};
use std::cmp::Ordering;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::hierarchy::{CoreDecls, DeclInfo, TypeHierarchy};
use crate::ids::{FunctionSigId, TypeDeclId, TypeId};
use crate::table::TypeTable;
use crate::ty::{PoolRange, Type, TypeKind, UninternedType};

/// Recursion guard, mirroring the analyzer's `TypeOperationDepthLimitExceeded`
/// mechanism with the same limit (`type_system.dart:85`). One shared depth
/// cell — the oracle's `_typeOperationDepth` (`type_system.dart:372-382`) —
/// counts nested `is_subtype` and `least_upper_bound` frames together: a trip
/// makes `is_subtype` answer `false` at its own entry (subtype.dart:37-41)
/// and unwinds `least_upper_bound` to its outermost frame, which answers
/// `Object?` (least_upper_bound.dart:378-386). It guards recursion only: the
/// hierarchy's inheritance depths are precomputed exactly (topologically, no
/// cap) so the LUB tie-break matches the oracle's uncapped
/// `computeLongestInheritancePathToObject`, and the LUB supertype closure is
/// an iterative worklist, uncapped like the oracle's plain recursion
/// (least_upper_bound.dart:162-177).
pub(crate) const DEPTH_LIMIT: u32 = 100;

/// The `TypeOperationDepthLimitExceeded` counterpart
/// (`type_system.dart:75`): the shared depth cell tripped. `Err` carries it —
/// the subtype face answers `false` in place at its own entry (the
/// entry-local catch, subtype.dart:37-41); the LUB face propagates it to the
/// outermost frame (the rethrow, least_upper_bound.dart:383-385).
pub(crate) struct DepthLimitExceeded;

/// Visited set over interned ids: an epoch-stamp vector instead of a hash
/// set. Ids are dense `u32` pool indexes, so membership is a direct slot
/// compare, and [`StampSet::clear`] only bumps the epoch — reuse across
/// walks never rewrites old entries, and stamps are grown lazily as the
/// walk interns new ids.
#[derive(Clone)]
pub(crate) struct StampSet {
    epoch: u32,
    stamps: Vec<u32>,
}

impl Default for StampSet {
    fn default() -> Self {
        Self::new()
    }
}

impl StampSet {
    fn new() -> Self {
        Self {
            epoch: 1,
            stamps: Vec::new(),
        }
    }

    /// Whether `id` was unseen this epoch (and marks it seen).
    fn insert(&mut self, id: TypeId) -> bool {
        let index = id.raw() as usize;
        if index >= self.stamps.len() {
            self.stamps.resize(index + 1, 0);
        }
        let stamp = &mut self.stamps[index];
        if *stamp == self.epoch {
            return false;
        }
        *stamp = self.epoch;
        true
    }

    /// Clears the set in O(1): the epoch bump alone expires every stamp. On
    /// `u32` wraparound the stamps are refilled with 0 and the epoch restarts
    /// at 1 — slot value 0 never matches a live epoch, so stale entries
    /// cannot survive the reset.
    fn clear(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.stamps.fill(0);
            self.epoch = 1;
        }
    }
}

/// One walk's private containers: the visited set and the supertype
/// frontier. Pooled and recycled across walks; never shared between
/// in-flight frames (claim clears it, ownership is exclusive until return).
#[derive(Clone, Default)]
struct WalkUnit {
    visited: StampSet,
    frontier: Vec<TypeId>,
}

/// Reusable subtype-walk scratch: a pool of interface-walk units (one per
/// in-flight nesting depth), the LUB supertype-closure sets, their
/// intersection, and the closure walk's frontier. Units are claimed by
/// popping — a pooled unit is owned by no in-flight frame — and returned on
/// exit, so every nesting depth reuses its containers and steady-state
/// queries (including re-entrant rule-12 walks) allocate nothing. The LUB
/// sets and the closure frontier are cleared (not reallocated) at the start
/// of every use.
#[derive(Clone, Default)]
pub(crate) struct WalkScratch {
    units: Vec<WalkUnit>,
    pub(crate) set_a: FxHashSet<TypeId>,
    pub(crate) set_b: FxHashSet<TypeId>,
    pub(crate) candidates: Vec<TypeId>,
    /// The supertype-closure frontier: claimed by `interface_lub` for the
    /// duration of its two closure walks and returned after, so the
    /// steady-state walk reuses one buffer.
    pub(crate) closure_frontier: Vec<TypeId>,
}

impl WalkScratch {
    /// Number of pooled units — test observability for the reuse contract
    /// (stable at the maximum nesting depth, growing never).
    #[cfg(test)]
    fn pooled_units(&self) -> usize {
        self.units.len()
    }

    /// Capacity of the pooled closure frontier — test observability for the
    /// reuse contract (claimed, cleared, returned; never reallocated).
    #[cfg(test)]
    fn closure_frontier_capacity(&self) -> usize {
        self.closure_frontier.capacity()
    }
}

/// Facade over a [`TypeTable`] plus a [`TypeHierarchy`] snapshot. All queries
/// take `&self`: interning synthesized types (`Future<S>`, `Object?`, stripped
/// projections) goes through the table's interior mutability, so the table
/// grows but ids never change.
///
/// The system owns three scratch cells:
///
/// - `walk`: the interface-walk unit pool and the LUB-closure sets. A walk
///   claims a unit by popping it from the pool and returns it on exit, so
///   every re-entrant rule-12 depth owns a private unit and reuses it once
///   pooled — no per-query allocation in steady state. Isolation between
///   sibling and nested frames is structural: a pooled unit is owned by no
///   in-flight frame, and claiming clears it. A walk that finds the cell
///   busy (an external holder, e.g. a test) falls back to fresh local
///   containers.
/// - `future_memo`: `Future<S>` synthesis keyed by the argument id. It is
///   only ever held across a single lookup/insert, never across a recursive
///   query, so re-entrancy cannot arise.
/// - `type_operation_depth`: the oracle's `_typeOperationDepth`
///   (type_system.dart:85, :372-382) — one shared recursion budget across
///   the subtype and LUB faces, so a subtype leg inside a LUB continues the
///   same count. A trip answers `false` from the tripped subtype entry and
///   unwinds the LUB to its outermost frame, which answers `Object?`.
///
/// All are single-threaded, matching [`TypeTable`]'s interior `RefCell`.
#[derive(Clone)]
pub struct TypeSystem<'a> {
    table: &'a TypeTable,
    hierarchy: &'a TypeHierarchy,
    pub(crate) walk: RefCell<WalkScratch>,
    future_memo: RefCell<FxHashMap<TypeId, TypeId>>,
    type_operation_depth: Cell<u32>,
}

impl<'a> TypeSystem<'a> {
    /// Binds the table and hierarchy snapshot.
    #[must_use]
    pub fn new(table: &'a TypeTable, hierarchy: &'a TypeHierarchy) -> Self {
        Self {
            table,
            hierarchy,
            walk: RefCell::default(),
            future_memo: RefCell::default(),
            type_operation_depth: Cell::new(0),
        }
    }

    /// The underlying table (for interning the types being queried).
    #[must_use]
    pub const fn table(&self) -> &TypeTable {
        self.table
    }

    /// The hierarchy snapshot.
    #[must_use]
    pub const fn hierarchy(&self) -> &TypeHierarchy {
        self.hierarchy
    }

    /// Semantic nullability: `true` for `dynamic`, `Invalid`, `void`, `Null`,
    /// any `?` type, and `FutureOr<T>` with nullable `T`
    /// (`type_system.dart:1216-1233`).
    #[must_use]
    pub fn is_nullable(&self, t: TypeId) -> bool {
        let ty = self.table.get(t);
        if ty.is_nullable() {
            return true;
        }
        match ty.kind() {
            TypeKind::Dynamic | TypeKind::Void | TypeKind::Invalid | TypeKind::Null => true,
            TypeKind::FutureOr { base } => self.is_nullable(base),
            _ => false,
        }
    }

    /// `isNonNullable` (`type_system.dart:1883-1912`).
    #[must_use]
    pub fn is_non_nullable(&self, t: TypeId) -> bool {
        let ty = self.table.get(t);
        if ty.is_nullable() {
            return false;
        }
        match ty.kind() {
            // M1 has no declared bounds; the official unbound case is `false`
            // as well (type_system.dart:1910).
            TypeKind::Dynamic
            | TypeKind::Void
            | TypeKind::Invalid
            | TypeKind::Null
            | TypeKind::TypeParameter { .. } => false,
            TypeKind::FutureOr { base } => self.is_non_nullable(base),
            _ => true,
        }
    }

    /// The top-type predicate `{dynamic, Invalid, void, Object?}` plus
    /// `FutureOr<T>` with top `T` (`type_system.dart:1277-1311`).
    #[must_use]
    pub fn is_top(&self, t: TypeId) -> bool {
        let ty = self.table.get(t);
        match ty.kind() {
            TypeKind::Dynamic | TypeKind::Void | TypeKind::Invalid => true,
            TypeKind::FutureOr { base } if !ty.is_nullable() => self.is_top(base),
            _ if ty.is_nullable() => {
                // TOP(T?) iff TOP(T) or OBJECT(T) (type_system.dart:1294-1298).
                let stripped = self.strip(t);
                self.is_top(stripped) || self.is_object(stripped)
            }
            _ => false,
        }
    }

    /// The bottom type: `Never` with the no-suffix projection
    /// (`type.dart:1204`; `Never?` is interned as `Null`, so it is not bottom).
    #[must_use]
    pub fn is_bottom(&self, t: TypeId) -> bool {
        matches!(self.table.get(t).kind(), TypeKind::Never)
    }

    /// Membership in the equivalence class of `Null` (`type_system.dart:1195-1213`).
    /// `Never?` counts because it is interned as `Null` (type.dart:1207-1209).
    #[must_use]
    pub fn is_null(&self, t: TypeId) -> bool {
        matches!(self.table.get(t).kind(), TypeKind::Null)
    }

    /// Membership in the equivalence class of `Object` with no suffix
    /// (`type_system.dart:1236-1254`).
    #[must_use]
    pub fn is_object(&self, t: TypeId) -> bool {
        let ty = self.table.get(t);
        if ty.is_nullable() {
            return false;
        }
        match ty.kind() {
            TypeKind::Interface { decl, .. } => decl == self.core().object,
            TypeKind::FutureOr { base } => self.is_object(base),
            _ => false,
        }
    }

    /// `isStrictlyNonNullable` (`type_system.dart:1921-1951`): the receiver
    /// test of `invalid_null_aware_operator`. Stricter than
    /// [`TypeSystem::is_non_nullable`]: `Null` itself does not count as
    /// strictly non-nullable. M1 deviation: type parameters return `false`
    /// because declared bounds are not modeled.
    #[must_use]
    pub fn is_strictly_non_nullable(&self, t: TypeId) -> bool {
        let ty = self.table.get(t);
        if ty.is_nullable() {
            return false;
        }
        match ty.kind() {
            TypeKind::Dynamic
            | TypeKind::Void
            | TypeKind::Invalid
            | TypeKind::Null
            | TypeKind::TypeParameter { .. } => false,
            TypeKind::FutureOr { base } => self.is_strictly_non_nullable(base),
            _ => true,
        }
    }

    /// `isSubtypeOf` (`subtype.dart:33`): the relation behind all four M1
    /// diagnostics. Every frame — top-level or re-entrant — enters the
    /// shared depth cell; a trip answers `false` at its own entry (the
    /// entry-local catch, subtype.dart:37-41), and enclosing frames —
    /// including LUB callers, which share the cell — continue with it.
    #[must_use]
    pub fn is_subtype(&self, t0: TypeId, t1: TypeId) -> bool {
        if self.enter_type_operation().is_err() {
            return false;
        }
        let answer = self.subtype_impl(t0, t1);
        self.exit_type_operation();
        answer
    }

    /// `isAssignableTo` (`type_system.dart:891-925`), minus implicit `call`
    /// tear-offs (no tear-off inference in M1).
    #[must_use]
    pub fn is_assignable(&self, from: TypeId, to: TypeId, strict_casts: bool) -> bool {
        if self.is_subtype(from, to) {
            return true;
        }
        // An already-reported error: accept and suppress (type_system.dart:901-904).
        if matches!(self.table.get(from).kind(), TypeKind::Invalid) {
            return true;
        }
        // `strict-casts: true` disables all downcasts, including `dynamic`
        // (type_system.dart:919-921).
        if strict_casts {
            return false;
        }
        matches!(self.table.get(from).kind(), TypeKind::Dynamic)
    }

    /// `canBeSubtypeOf` (`type_system.dart:150-330`): whether an instance of
    /// `left` could possibly also be an instance of `right` — the disjointness
    /// question behind `pattern_never_matches_value_type`. Wider than
    /// [`TypeSystem::is_subtype`] in both directions (`num` vs `int`,
    /// `int` vs `num`), and by construction `is_subtype(left, right) ⇒
    /// can_be_subtype_of(left, right)`, so "the cast/pattern is unnecessary"
    /// (is-subtype) and "the pattern never matches" (cannot-be-subtype) are
    /// mutually exclusive — the official invariant, pinned by tests here.
    ///
    /// Decision order is the source order: `Null` arms, both-nullable,
    /// function-vs-interface pairs, `FutureOr` splits, then the
    /// interface/interface region — enum-constant enumeration, same-element
    /// argument recursion, and the closed-subclass walks
    /// (`ClassElementImpl.allSubtypes`, element.dart:167-210 — dartq's
    /// injected [`TypeHierarchy::direct_subclasses`] facts stand in; depth
    /// completeness of that closure is the fact source's responsibility, the
    /// official takes it from the same-library scan). Deviations, each
    /// answer-preserving:
    ///
    /// - Extension-type erasure is the identity (no extension types in M1).
    /// - The int/double cross ("can happen in JavaScript",
    ///   type_system.dart:223-227) is omitted as observationally dead at
    ///   this layer: neither core `int` nor `double` is sealed or an enum,
    ///   so the pair falls through to the conservative `true` default here
    ///   exactly as the official's cross answers.
    /// - Enum-constant enumeration uses the enum's this-type as the
    ///   constants' type (exact for non-generic enums; dartq carries a
    ///   constant count, not per-constant declared types) plus the
    ///   is-subtype short-circuit, which only restores answers the official
    ///   derives from per-constant concrete types of generic enums — a
    ///   subset relation can never contradict "some constant matches".
    /// - `resolveToBound` on the interface/type-parameter paths is the
    ///   identity (declared bounds are M2).
    #[must_use]
    pub fn can_be_subtype_of(&self, left: TypeId, right: TypeId) -> bool {
        let a = self.table.get(left);
        let b = self.table.get(right);

        // If one is `Null`, then the other must be nullable
        // (type_system.dart:164-169).
        let left_nullable = !self.is_non_nullable(left);
        let right_nullable = !self.is_non_nullable(right);
        if matches!(a.kind(), TypeKind::Null) {
            return right_nullable;
        }
        if matches!(b.kind(), TypeKind::Null) {
            return left_nullable;
        }
        // If none is `Null`, but both are nullable, they match at `Null`
        // (type_system.dart:172-174).
        if left_nullable && right_nullable {
            return true;
        }

        // Could be `void Function()` vs `Object`/`Function`
        // (type_system.dart:177-180) and the symmetric arm (:182-185).
        let left_functional =
            |decl: TypeDeclId| decl == self.core().function || decl == self.core().object;
        match (a.kind(), b.kind()) {
            (TypeKind::Function { .. }, TypeKind::Interface { decl, .. }) => {
                return left_functional(decl);
            }
            (TypeKind::Interface { decl, .. }, TypeKind::Function { .. }) => {
                return left_functional(decl);
            }
            _ => {}
        }

        // `FutureOr<T> = T || Future<T>`: attempt both sides to the other
        // (type_system.dart:188-201). `FutureOr<T>?` splits the same way —
        // the wrapper's `?` only adds `Null` to the value set, and the
        // `Null` arms above already settled the nullable-vs-nullable cases.
        if let TypeKind::FutureOr { base } = a.kind() {
            let future = self.intern_future(base);
            return self.can_be_subtype_of(base, right) || self.can_be_subtype_of(future, right);
        }
        if let TypeKind::FutureOr { base } = b.kind() {
            let future = self.intern_future(base);
            return self.can_be_subtype_of(left, base) || self.can_be_subtype_of(left, future);
        }

        // The interface/interface region (type_system.dart:204-278).
        if let (
            TypeKind::Interface {
                decl: left_decl, ..
            },
            TypeKind::Interface {
                decl: right_decl, ..
            },
        ) = (a.kind(), b.kind())
        {
            // The int/double cross (type_system.dart:223-227) is
            // observationally dead at this layer — see the doc comment.

            // If the left is an enum, we know the types of all its instances
            // (type_system.dart:229-241).
            if self.hierarchy.enum_constant_count(left_decl).is_some() {
                if self.is_subtype(left, right) {
                    return true;
                }
                let constants_type = self.this_type(left_decl);
                return self.is_subtype(constants_type, right);
            }

            if left_decl == right_decl {
                return self.can_be_subtype_of_interfaces(left, right);
            }

            // If we know all subtypes of the left, only they can implement
            // the right (type_system.dart:243-262). Candidates are the left
            // instantiation itself plus the closed subclass set as
            // this-types, in the official order.
            if let Some(subclasses) = self.hierarchy.direct_subclasses(left_decl) {
                if let Some(as_right) = self.as_instance_of(left, right_decl)
                    && self.can_be_equal_arguments(as_right, right)
                {
                    return true;
                }
                for &sub in subclasses {
                    let sub_ty = self.this_type(sub);
                    if let Some(as_right) = self.as_instance_of(sub_ty, right_decl)
                        && self.can_be_equal_arguments(as_right, right)
                    {
                        return true;
                    }
                }
                return false;
            }

            // Symmetric: if we know all subtypes of the right, only they can
            // implement the left (type_system.dart:263-278).
            if let Some(subclasses) = self.hierarchy.direct_subclasses(right_decl) {
                if let Some(as_left) = self.as_instance_of(right, left_decl) {
                    if self.can_be_subtype_of_interfaces(left, as_left) {
                        return true;
                    }
                }
                for &sub in subclasses {
                    let sub_ty = self.this_type(sub);
                    if let Some(as_left) = self.as_instance_of(sub_ty, left_decl)
                        && self.can_be_subtype_of_interfaces(left, as_left)
                    {
                        return true;
                    }
                }
                return false;
            }
        }

        // Record arms (type_system.dart:282-322): a record is neither a
        // function nor a non-trivial interface, and record/record needs
        // matching shapes with pairwise compatible fields; everything else
        // conservatively can.
        if let TypeKind::Record { .. } = a.kind() {
            if matches!(b.kind(), TypeKind::Function { .. }) {
                return false;
            }
            if let TypeKind::Interface { decl, .. } = b.kind() {
                return decl == self.core().object || Some(decl) == self.core().record;
            }
        }
        if let TypeKind::Record { .. } = b.kind() {
            if matches!(a.kind(), TypeKind::Function { .. }) {
                return false;
            }
            if let TypeKind::Interface { decl, .. } = a.kind() {
                return decl == self.core().object || Some(decl) == self.core().record;
            }
        }
        if let (TypeKind::Record { shape: sub }, TypeKind::Record { shape: sup }) =
            (a.kind(), b.kind())
        {
            let sub_shape = self.table.record_shape(sub);
            let sup_shape = self.table.record_shape(sup);
            if sub_shape.positional.len() != sup_shape.positional.len() {
                return false;
            }
            for i in 0..sub_shape.positional.len() {
                if !self.can_be_subtype_of(
                    self.table.arg(sub_shape.positional.offset() + i),
                    self.table.arg(sup_shape.positional.offset() + i),
                ) {
                    return false;
                }
            }
            if sub_shape.named.len() != sup_shape.named.len() {
                return false;
            }
            for i in 0..sub_shape.named.len() {
                let (x, y) = (sub_shape.named.offset() + i, sup_shape.named.offset() + i);
                if self.table.named_names_cmp(x, y) != std::cmp::Ordering::Equal {
                    return false;
                }
                if !self.can_be_subtype_of(self.table.named_ty(x), self.table.named_ty(y)) {
                    return false;
                }
            }
        }

        // Conservative default (type_system.dart:324): dynamic, void, type
        // parameters, function/function, and every undecided shape answer
        // "could be".
        true
    }

    /// The declaration's "this-type": its instantiation over its own type
    /// parameters — the `elementThis` of the official closed-subclass scan
    /// (element.dart:183/:199).
    fn this_type(&self, decl: TypeDeclId) -> TypeId {
        let info = self.hierarchy.decl(decl).expect("declared owner");
        let params = info.type_parameters.clone();
        let args: Vec<TypeId> = params
            .iter()
            .map(|&param| {
                self.table
                    .intern(UninternedType::TypeParameter { parameter: param }, false)
            })
            .collect();
        self.table
            .intern(UninternedType::Interface { decl, args }, false)
    }

    /// `asInstanceOf` over this-type and instantiated roots: the
    /// instantiation of `from` viewed as an instance of `target`, walking
    /// the supertype edges with live substitution — `None` when `target` is
    /// not a supertype of `from`. Iterative with a visited set; claims a
    /// pooled walk unit like [`TypeSystem::interface_subtype`].
    fn as_instance_of(&self, from: TypeId, target: TypeDeclId) -> Option<TypeId> {
        let TypeKind::Interface {
            decl: from_decl,
            args: from_args,
        } = self.table.get(from).kind()
        else {
            return None;
        };
        let (mut visited, mut frontier) = match self.walk.try_borrow_mut() {
            Ok(mut scratch) => {
                let mut unit = scratch.units.pop().unwrap_or_default();
                unit.visited.clear();
                unit.frontier.clear();
                (unit.visited, unit.frontier)
            }
            Err(_) => (StampSet::new(), Vec::new()),
        };
        let info = self.hierarchy.decl(from_decl).expect("declared owner");
        for &edge in &info.supertypes {
            frontier.push(self.instantiate(edge, info, from_args));
        }
        let mut answer = None;
        while let Some(current) = frontier.pop() {
            if !visited.insert(current) {
                continue;
            }
            let TypeKind::Interface { decl, args } = self.table.get(current).kind() else {
                continue;
            };
            if decl == target {
                answer = Some(current);
                break;
            }
            let info = self.hierarchy.decl(decl).expect("declared owner");
            for &edge in &info.supertypes {
                frontier.push(self.instantiate(edge, info, args));
            }
        }
        if let Ok(mut scratch) = self.walk.try_borrow_mut() {
            scratch.units.push(WalkUnit { visited, frontier });
        }
        answer
    }

    /// `canBeSubtypeOfInterfaces` (type_system.dart:213-235): both sides
    /// instantiate the same element; the answer is the pairwise
    /// [`TypeSystem::can_be_subtype_of`] over the arguments.
    fn can_be_subtype_of_interfaces(&self, left: TypeId, right: TypeId) -> bool {
        let (
            TypeKind::Interface {
                args: left_args, ..
            },
            TypeKind::Interface {
                args: right_args, ..
            },
        ) = (self.table.get(left).kind(), self.table.get(right).kind())
        else {
            return false;
        };
        debug_assert_eq!(left_args.len(), right_args.len());
        for i in 0..left_args.len() {
            if !self.can_be_subtype_of(
                self.table.arg(left_args.offset() + i),
                self.table.arg(right_args.offset() + i),
            ) {
                return false;
            }
        }
        true
    }

    /// `_canBeEqualArguments` (type_system.dart:1786-1802): optimistically,
    /// same-element instantiations can be equal when every argument pair
    /// can (`_canBeEqualTo`, type_system.dart:1804-1814 — two interface
    /// types can be equal only under the same element; everything else
    /// conservatively can).
    fn can_be_equal_arguments(&self, left: TypeId, right: TypeId) -> bool {
        let (
            TypeKind::Interface {
                args: left_args, ..
            },
            TypeKind::Interface {
                args: right_args, ..
            },
        ) = (self.table.get(left).kind(), self.table.get(right).kind())
        else {
            return false;
        };
        debug_assert_eq!(left_args.len(), right_args.len());
        for i in 0..left_args.len() {
            let a = self.table.arg(left_args.offset() + i);
            let b = self.table.arg(right_args.offset() + i);
            let can_equal = match (self.table.get(a).kind(), self.table.get(b).kind()) {
                (
                    TypeKind::Interface { decl: decl_a, .. },
                    TypeKind::Interface { decl: decl_b, .. },
                ) => decl_a == decl_b,
                _ => true,
            };
            if !can_equal {
                return false;
            }
        }
        true
    }

    /// Total order on top types (`isMoreTop`, `type_system.dart:1124-1189`).
    #[must_use]
    pub fn is_more_top(&self, t: TypeId, s: TypeId) -> bool {
        let ty = self.table.get(t);
        let sy = self.table.get(s);
        if matches!(ty.kind(), TypeKind::Void) {
            return true;
        }
        if matches!(sy.kind(), TypeKind::Void) {
            return false;
        }
        if matches!(ty.kind(), TypeKind::Dynamic | TypeKind::Invalid) {
            return true;
        }
        if matches!(sy.kind(), TypeKind::Dynamic | TypeKind::Invalid) {
            return false;
        }
        if !ty.is_nullable() && self.is_object_declaration(ty) {
            return true;
        }
        if !sy.is_nullable() && self.is_object_declaration(sy) {
            return false;
        }
        if ty.is_nullable() && sy.is_nullable() {
            return self.is_more_top(self.strip(t), self.strip(s));
        }
        if sy.is_nullable() {
            return true;
        }
        if ty.is_nullable() {
            return false;
        }
        if let (TypeKind::FutureOr { base: tb }, TypeKind::FutureOr { base: sb }) =
            (ty.kind(), sy.kind())
        {
            return self.is_more_top(tb, sb);
        }
        false
    }

    // ---- internals ----

    /// Enters a recursive type operation (`enterTypeOperation`,
    /// type_system.dart:372-382): `Err` when the shared cell would pass
    /// [`DEPTH_LIMIT`]. The cell is left untouched on a trip — the oracle
    /// increments before the check and its `finally` decrement nets the
    /// throw to zero, so a tripped caller may unwind without
    /// [`Self::exit_type_operation`].
    pub(crate) fn enter_type_operation(&self) -> Result<(), DepthLimitExceeded> {
        let depth = self.type_operation_depth.get() + 1;
        if depth > DEPTH_LIMIT {
            return Err(DepthLimitExceeded);
        }
        self.type_operation_depth.set(depth);
        Ok(())
    }

    /// Leaves a recursive type operation (`exitTypeOperation`,
    /// type_system.dart:381-383).
    pub(crate) fn exit_type_operation(&self) {
        self.type_operation_depth
            .set(self.type_operation_depth.get() - 1);
    }

    const fn core(&self) -> CoreDecls {
        self.hierarchy.core()
    }

    fn object_none(&self) -> TypeId {
        self.hierarchy.sentinels().object_none
    }

    pub(crate) fn object_question(&self) -> TypeId {
        self.hierarchy.sentinels().object_question
    }

    /// Whether `t` is the bare `Function` interface type.
    pub(crate) fn is_bare_function_id(&self, t: TypeId) -> bool {
        matches!(
            self.table.get(t).kind(),
            TypeKind::Interface { decl, .. } if decl == self.core().function
        )
    }

    fn null_type(&self) -> TypeId {
        self.hierarchy.sentinels().null
    }

    pub(crate) fn core_object_none(&self) -> TypeId {
        self.object_none()
    }

    /// Interns the bare `Function` interface type.
    pub(crate) fn intern_bare_function(&self) -> TypeId {
        self.table.intern(
            UninternedType::Interface {
                decl: self.core().function,
                args: Vec::new(),
            },
            false,
        )
    }

    /// Re-interns `t` with the given projection. Unit kinds keep their fixed
    /// nullability (the table normalizes; `dynamic` returns itself,
    /// type.dart:90-92) and `Never?` collapses to `Null` (type.dart:1207).
    /// The flip is one bit on the interned structure, so the table answers
    /// from its dedup maps without building any buffer.
    pub(crate) fn with_nullability(&self, t: TypeId, nullable: bool) -> TypeId {
        self.table.with_nullability(t, nullable)
    }

    pub(crate) fn strip(&self, t: TypeId) -> TypeId {
        self.with_nullability(t, false)
    }

    pub(crate) fn make_nullable(&self, t: TypeId) -> TypeId {
        self.with_nullability(t, true)
    }

    /// Synthesizes `Future<S>` for the `FutureOr` rules
    /// (`subtype.dart:378-382`, `424-427`). Memoized by the argument id:
    /// interning is canonical, so the synthesized id is stable for the
    /// table's lifetime and the cache stays valid as the table grows. The
    /// map is bounded by the number of distinct bases queried.
    pub(crate) fn intern_future(&self, base: TypeId) -> TypeId {
        if let Some(&future) = self.future_memo.borrow().get(&base) {
            return future;
        }
        let future = self.table.intern(
            UninternedType::Interface {
                decl: self.core().future,
                args: vec![base],
            },
            false,
        );
        self.future_memo.borrow_mut().insert(base, future);
        future
    }

    /// Interns `FutureOr<T>` with the no-suffix projection.
    pub(crate) fn intern_future_or(&self, base: TypeId) -> TypeId {
        self.table.intern(UninternedType::FutureOr { base }, false)
    }

    /// The core subtype walk. The depth guard lives in the shared cell
    /// entered by [`TypeSystem::is_subtype`]: the oracle's
    /// `enterTypeOperation` (type_system.dart:372-378) runs before
    /// `_isSubtypeOf` consults reflexivity, so identical pairs beyond the
    /// limit answer `false` too, and internal recursion re-enters per level
    /// exactly like the oracle's `isSubtypeOf` calls.
    fn subtype_impl(&self, t0: TypeId, t1: TypeId) -> bool {
        // 0a Reflexivity (subtype.dart:264-266). Interned ids make this a
        // structural comparison in O(1).
        if t0 == t1 {
            return true;
        }
        let a = self.table.get(t0);
        let b = self.table.get(t1);

        // 0c InvalidType: suppress error avalanches (subtype.dart:274-276).
        if matches!(a.kind(), TypeKind::Invalid) || matches!(b.kind(), TypeKind::Invalid) {
            return true;
        }

        // Rule 1 Right Top (subtype.dart:283-288). `Object?` is the only
        // nullable type here — rule 4 below relies on that.
        if matches!(b.kind(), TypeKind::Dynamic | TypeKind::Void)
            || (b.is_nullable() && self.is_object_declaration(b))
        {
            return true;
        }

        // Rule 2 Left Top (subtype.dart:292-296); falls through on failure.
        if matches!(a.kind(), TypeKind::Dynamic | TypeKind::Void)
            && self.is_subtype(self.object_question(), t1)
        {
            return true;
        }

        // Rule 3 Left Bottom (subtype.dart:299-301). `Never?` never reaches
        // here — it is interned as `Null`.
        if matches!(a.kind(), TypeKind::Never) {
            return true;
        }

        // Rule 4 Right Object (subtype.dart:304-346). `b` is non-`?` here:
        // `Object?` was consumed by rule 1.
        if self.is_object_declaration(b) {
            if let TypeKind::TypeParameter { .. } = a.kind() {
                // Bound-based (subtype.dart:309-318); M1 has no bounds, and
                // the official unbound default `Object?` is not a subtype of
                // `Object`, so `false` either way.
                return false;
            }
            if let Some(base) = Self::non_null_future_or_base(a) {
                // `FutureOr<S> <: Object` iff `S <: Object` (subtype.dart:321-325);
                // a nullable `FutureOr` falls to the `S?` case below.
                return self.is_subtype(base, t1);
            }
            return !matches!(
                a.kind(),
                TypeKind::Null | TypeKind::Dynamic | TypeKind::Void
            ) && !a.is_nullable();
        }

        // Rule 5 Left Null (subtype.dart:349-369).
        if matches!(a.kind(), TypeKind::Null) {
            if matches!(b.kind(), TypeKind::FutureOr { .. }) {
                // `Null <: FutureOr<S>` iff `Null <: S` (subtype.dart:352-357);
                // nullable `FutureOr<S>` is `S?` and accepts `Null`.
                if !b.is_nullable() {
                    let s1 = Self::future_or_base(b);
                    return self.is_subtype(self.null_type(), s1);
                }
                return true;
            }
            return matches!(b.kind(), TypeKind::Null) || b.is_nullable();
        }

        // Rule 6 Left FutureOr (subtype.dart:372-385).
        if let Some(s0) = Self::non_null_future_or_base(a) {
            if !self.is_subtype(s0, t1) {
                return false;
            }
            let future_s0 = self.intern_future(s0);
            return self.is_subtype(future_s0, t1);
        }

        // Rule 7 Left Nullable (subtype.dart:389-392).
        if a.is_nullable() {
            let s0 = self.strip(t0);
            return self.is_subtype(s0, t1) && self.is_subtype(self.null_type(), t1);
        }

        // Rules 8-9 (subtype.dart:397-415): promoted-bound reflexivity and
        // promotion — M1 has no promoted bounds, and unpromoted type-variable
        // reflexivity is subsumed by rule 0a.

        // Rule 10 Right FutureOr (subtype.dart:418-449).
        if let Some(s1) = Self::non_null_future_or_base(b) {
            let future_s1 = self.intern_future(s1);
            if self.is_subtype(t0, future_s1) {
                return true;
            }
            if self.is_subtype(t0, s1) {
                return true;
            }
            // Type-variable bound paths (subtype.dart:437-446): M1 has no
            // declared bounds.
            return false;
        }

        // Rule 11 Right Nullable (subtype.dart:452-477).
        if b.is_nullable() {
            let s1 = self.strip(t1);
            if self.is_subtype(t0, s1) {
                return true;
            }
            if self.is_subtype(t0, self.null_type()) {
                return true;
            }
            return false;
        }

        // Rule 12 Interface/Interface (subtype.dart:482-484).
        if let (
            TypeKind::Interface { decl: d0, args: a0 },
            TypeKind::Interface { decl: d1, args: a1 },
        ) = (a.kind(), b.kind())
        {
            return self.interface_subtype(d0, a0, d1, a1);
        }

        // Left Type Variable Bound (subtype.dart:490-500): M1 has no bounds.

        // Rule 13 Function (subtype.dart:502-510).
        if let TypeKind::Function { signature: f } = a.kind() {
            if self.is_bare_function(b) {
                return true;
            }
            if let TypeKind::Function { signature: g } = b.kind() {
                return self.function_subtype(f, g);
            }
        }

        // Rule 14 Record (subtype.dart:512-519): `T0` is a record type —
        // `T1` = `Record` answers `true` (the Record Type/Record rule,
        // subtype.dart:513-515), `T1` a record type answers
        // [`Self::record_subtype`] (subtype.dart:516-518).
        if let TypeKind::Record { shape } = a.kind() {
            if self.is_core_record(b) {
                return true;
            }
            if let TypeKind::Record { shape: other } = b.kind() {
                return self.record_subtype(shape, other);
            }
        }
        false
    }

    /// Whether `t` is the core `Record` class (`T1.isDartCoreRecord`,
    /// subtype.dart:513). Snapshots without a `Record` declaration answer
    /// `false` — the class cannot be named there.
    fn is_core_record(&self, t: Type) -> bool {
        matches!(t.kind(), TypeKind::Interface { decl, .. } if Some(decl) == self.core().record)
    }

    /// `_isRecordSubtypeOf` (subtype.dart:227-259): equal positional and
    /// named arities, positional fields pairwise covariant, named fields
    /// pairwise covariant under equal names. Both named lists are in the
    /// interner's canonical (name-sorted) order, so the index zip is exactly
    /// the oracle's name-matched pairing.
    fn record_subtype(
        &self,
        sub: crate::ids::RecordShapeId,
        sup: crate::ids::RecordShapeId,
    ) -> bool {
        let sub = self.table.record_shape(sub);
        let sup = self.table.record_shape(sup);
        if sub.positional.len() != sup.positional.len() || sub.named.len() != sup.named.len() {
            return false;
        }
        for i in 0..sub.positional.len() {
            if !self.is_subtype(
                self.table.arg(sub.positional.offset() + i),
                self.table.arg(sup.positional.offset() + i),
            ) {
                return false;
            }
        }
        for i in 0..sub.named.len() {
            let (si, ti) = (sub.named.offset() + i, sup.named.offset() + i);
            // Name equality rides the canonical order: two sorted lists of
            // equal length match pairwise iff every position's name does.
            if self.table.named_names_cmp(si, ti) != Ordering::Equal {
                return false;
            }
            if !self.is_subtype(self.table.named_ty(si), self.table.named_ty(ti)) {
                return false;
            }
        }
        true
    }

    /// The `base` of `t` when `t` is a non-`?` `FutureOr`.
    fn non_null_future_or_base(t: Type) -> Option<TypeId> {
        match t.kind() {
            TypeKind::FutureOr { base } if !t.is_nullable() => Some(base),
            _ => None,
        }
    }

    fn future_or_base(t: Type) -> TypeId {
        match t.kind() {
            TypeKind::FutureOr { base } => base,
            _ => unreachable!("caller checked the FutureOr kind"),
        }
    }

    fn is_object_declaration(&self, t: Type) -> bool {
        matches!(t.kind(), TypeKind::Interface { decl, .. } if decl == self.core().object)
    }

    fn is_bare_function(&self, t: Type) -> bool {
        matches!(t.kind(), TypeKind::Interface { decl, .. } if decl == self.core().function)
    }

    /// `_isInterfaceSubtypeOf` (subtype.dart:184-224).
    fn interface_subtype(
        &self,
        d0: TypeDeclId,
        a0: PoolRange,
        d1: TypeDeclId,
        a1: PoolRange,
    ) -> bool {
        let core = self.core();
        // Rule-order note: `superType.isDartCoreObject → true`
        // (subtype.dart:191-193) is unreachable under the ordering (rule 4
        // consumes `Object` first) but is kept in the source's position;
        // `Object` subtypes nothing but itself (subtype.dart:196-198).
        if d1 == core.object {
            return true;
        }
        if d0 == core.object {
            return false;
        }
        if d0 == d1 {
            return self.arguments_subtype(a0, a1);
        }
        // Class types cannot subtype `Function` or vice versa
        // (subtype.dart:206-209).
        if d0 == core.function || d1 == core.function {
            return false;
        }
        // Depth-first walk over instantiated supertypes, the counterpart of
        // `allSupertypes` + substitution (subtype.dart:211-221). Canonical
        // interning makes `TypeId` a sound visited-set key. The walk claims
        // a scratch unit by popping it from the pool — a pooled unit is
        // owned by no in-flight frame, so re-entrant rule-12 walks get
        // private containers at every depth and reuse them once pooled; a
        // walk that finds the cell busy (an external holder) falls back to
        // fresh local containers. See the `TypeSystem` scratch docs.
        let (mut visited, mut frontier) = match self.walk.try_borrow_mut() {
            Ok(mut scratch) => {
                let mut unit = scratch.units.pop().unwrap_or_default();
                unit.visited.clear();
                unit.frontier.clear();
                (unit.visited, unit.frontier)
            }
            Err(_) => (StampSet::new(), Vec::new()),
        };
        self.seed_frontier(d0, a0, &mut frontier);
        let answer = self.interface_walk(d1, a1, &mut visited, &mut frontier);
        if let Ok(mut scratch) = self.walk.try_borrow_mut() {
            scratch.units.push(WalkUnit { visited, frontier });
        }
        answer
    }

    /// Pushes the instantiated supertypes of `d0` — the walk's starting
    /// frontier, in declaration order (subtype.dart:211-214).
    fn seed_frontier(&self, d0: TypeDeclId, a0: PoolRange, frontier: &mut Vec<TypeId>) {
        let info = self.hierarchy.decl(d0).expect("declared owner");
        for &edge in &info.supertypes {
            frontier.push(self.instantiate(edge, info, a0));
        }
    }

    /// The frontier loop of the interface walk
    /// (subtype.dart:215-221). Traversal order — LIFO pops over
    /// declaration-order supertype pushes — is part of the observable
    /// answer: the first frontier entry whose declaration matches `d1`
    /// decides via `arguments_subtype`.
    fn interface_walk(
        &self,
        d1: TypeDeclId,
        a1: PoolRange,
        visited: &mut StampSet,
        frontier: &mut Vec<TypeId>,
    ) -> bool {
        let core = self.core();
        while let Some(current) = frontier.pop() {
            if !visited.insert(current) {
                continue;
            }
            let TypeKind::Interface { decl, args } = self.table.get(current).kind() else {
                continue;
            };
            if decl == d1 {
                return self.arguments_subtype(args, a1);
            }
            if decl == core.function {
                continue;
            }
            let info = self.hierarchy.decl(decl).expect("declared owner");
            for &edge in &info.supertypes {
                frontier.push(self.instantiate(edge, info, args));
            }
        }
        false
    }

    /// Instantiates a supertype template of `owner` with the concrete
    /// arguments `args` (the `Substitution.fromInterfaceType` step,
    /// subtype.dart:213-214). `owner` is the declaration info of the
    /// supertype template's owning class, already resolved by the caller.
    /// The table zips `params` with the argument pool inside one borrow, so
    /// the per-edge hot path materializes no intermediate `Vec`.
    fn instantiate(&self, edge: TypeId, owner: &DeclInfo, args: PoolRange) -> TypeId {
        self.table.instantiate(edge, &owner.type_parameters, args)
    }

    /// `_interfaceArguments` (subtype.dart:44-83) with the M1 variance
    /// model: class type parameters are covariant
    /// (`element.dart:11560-11561`); variance annotations are M2.
    fn arguments_subtype(&self, sub: PoolRange, sup: PoolRange) -> bool {
        assert_eq!(
            sub.len(),
            sup.len(),
            "interface arity mismatch: resolution must validate argument counts"
        );
        for i in 0..sup.len() {
            if !self.is_subtype(
                self.table.arg(sub.offset() + i),
                self.table.arg(sup.offset() + i),
            ) {
                return false;
            }
        }
        true
    }

    /// `_isFunctionSubtypeOf` (subtype.dart:86-182). Type-parameter
    /// correlation is positional (`relateTypeParameters`, subtype.dart:87-96);
    /// bound compatibility is M2. Pool payloads are read by index and the
    /// correlation runs as a live `Rename` substitution, so the per-query
    /// hot path materializes no `Vec` and clones no `String`.
    fn function_subtype(&self, f_id: FunctionSigId, g_id: FunctionSigId) -> bool {
        let f = self.table.signature(f_id);
        let g = self.table.signature(g_id);
        // Universally quantified correlation: rename f's parameters to g's.
        let (f_params, g_params) = (f.type_parameters, g.type_parameters);
        if f_params.len() != g_params.len() {
            return false;
        }
        // Return type is covariant (subtype.dart:98-100).
        if !self.is_subtype(
            self.table
                .substitute_rename(f.return_type, f_params, g_params),
            g.return_type,
        ) {
            return false;
        }
        // Positional parameters are contravariant (subtype.dart:107-131).
        // f must accept every call g accepts: no fewer positional slots and
        // no stronger required prefix.
        if f.positional.len() < g.positional.len() {
            return false;
        }
        if f.required_positional > g.required_positional {
            return false;
        }
        for i in 0..g.positional.len() {
            let fp = self.table.substitute_rename(
                self.table.arg(f.positional.offset() + i),
                f_params,
                g_params,
            );
            let gp = self.table.arg(g.positional.offset() + i);
            if !self.is_subtype(gp, fp) {
                return false;
            }
        }
        // Named parameters match by name in canonical (sorted) order
        // (subtype.dart:132-179).
        let (f_named, g_named) = (f.named, g.named);
        let (mut fi, mut gi) = (f_named.offset(), g_named.offset());
        let (f_end, g_end) = (
            f_named.offset() + f_named.len(),
            g_named.offset() + g_named.len(),
        );
        while fi < f_end && gi < g_end {
            match self.table.named_names_cmp(fi, gi) {
                Ordering::Less => {
                    if self.table.named_required(fi) {
                        return false;
                    }
                    fi += 1;
                }
                Ordering::Greater => return false,
                Ordering::Equal => {
                    if self.table.named_required(fi) && !self.table.named_required(gi) {
                        return false;
                    }
                    let ft =
                        self.table
                            .substitute_rename(self.table.named_ty(fi), f_params, g_params);
                    if !self.is_subtype(self.table.named_ty(gi), ft) {
                        return false;
                    }
                    fi += 1;
                    gi += 1;
                }
            }
        }
        // Remaining f parameters must not be required (subtype.dart:168-173).
        while fi < f_end {
            if self.table.named_required(fi) {
                return false;
            }
            fi += 1;
        }
        // g must not have parameters f lacks (subtype.dart:176-179).
        gi >= g_end
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{TypeDeclId, TypeParamId};

    const OBJECT: TypeDeclId = TypeDeclId::from_raw(1);
    const FUTURE: TypeDeclId = TypeDeclId::from_raw(2);
    const FUNCTION: TypeDeclId = TypeDeclId::from_raw(3);
    const NUM: TypeDeclId = TypeDeclId::from_raw(4);
    const INT: TypeDeclId = TypeDeclId::from_raw(5);
    const STRING: TypeDeclId = TypeDeclId::from_raw(6);
    const ITERABLE: TypeDeclId = TypeDeclId::from_raw(7);
    const LIST: TypeDeclId = TypeDeclId::from_raw(8);

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
        let (iterable_e, list_e) = (TypeParamId::from_raw(1), TypeParamId::from_raw(2));
        let list_of_e = table.intern(UninternedType::TypeParameter { parameter: list_e }, false);
        let hierarchy = TypeHierarchy::builder(CoreDecls {
            object: OBJECT,
            future: FUTURE,
            function: FUNCTION,
            record: None,
        })
        .decl(OBJECT, Vec::new())
        .decl(FUTURE, vec![iterable_e])
        .decl(FUNCTION, Vec::new())
        .decl(NUM, Vec::new())
        .decl(INT, Vec::new())
        .decl(STRING, Vec::new())
        .decl(ITERABLE, vec![iterable_e])
        .decl(LIST, vec![list_e])
        .supertype(NUM, itf(&table, OBJECT, &[]))
        .supertype(INT, itf(&table, NUM, &[]))
        .supertype(STRING, itf(&table, OBJECT, &[]))
        .supertype(ITERABLE, itf(&table, OBJECT, &[]))
        .supertype(LIST, itf(&table, ITERABLE, &[list_of_e]))
        .build(&table)
        .expect("fixture hierarchy is valid");
        (table, hierarchy)
    }

    /// The re-entrant walk: `List<int> <: Iterable<num>` reaches rule 12 for
    /// `int <: num` while the outer interface walk owns its scratch unit, so
    /// the inner query claims its own unit from the pool (fresh local
    /// containers while an external holder keeps the cell busy). The pinned
    /// contract: the nested answer equals the flat answer either way.
    #[test]
    fn reentrant_walk_matches_the_flat_answer() {
        let (table, hierarchy) = fixture();
        let sys = TypeSystem::new(&table, &hierarchy);
        let int = itf(&table, INT, &[]);
        let num = itf(&table, NUM, &[]);
        let string = itf(&table, STRING, &[]);
        let list_int = itf(&table, LIST, &[int]);
        let iterable_num = itf(&table, ITERABLE, &[num]);
        let iterable_string = itf(&table, ITERABLE, &[string]);

        // Flat: the outer walk owns the scratch, so the inner `int <: num`
        // rule-12 query takes the re-entrant fallback.
        assert!(sys.is_subtype(list_int, iterable_num));
        assert!(!sys.is_subtype(list_int, iterable_string));

        // Externally holding the scratch forces *every* walk onto the
        // fallback; answers must not change.
        let forced = {
            let _held = sys.walk.try_borrow_mut().expect("scratch is free here");
            (
                sys.is_subtype(list_int, iterable_num),
                sys.is_subtype(list_int, iterable_string),
            )
        };
        assert_eq!(forced, (true, false));
    }

    /// Deep rule-12 re-entry: `List<List<int>> <: Iterable<Iterable<num>>`
    /// nests three interface walks (List vs Iterable, List vs Iterable
    /// through the argument check, Int vs Num through the second argument
    /// check). Pins the nested answers and the steady-state allocation
    /// story: after the first query, repeated identical nested queries
    /// reuse one pooled unit per nesting level and grow no pool.
    #[test]
    fn nested_rule12_reentry_answers_and_scratch_reuse() {
        let (table, hierarchy) = fixture();
        let sys = TypeSystem::new(&table, &hierarchy);
        let int = itf(&table, INT, &[]);
        let num = itf(&table, NUM, &[]);
        let string = itf(&table, STRING, &[]);
        let list_list_int = itf(&table, LIST, &[itf(&table, LIST, &[int])]);
        let iterable_iterable_num = itf(&table, ITERABLE, &[itf(&table, ITERABLE, &[num])]);
        let iterable_iterable_string = itf(&table, ITERABLE, &[itf(&table, ITERABLE, &[string])]);

        // First pass pins the nested answers (three re-entry levels).
        assert!(sys.is_subtype(list_list_int, iterable_iterable_num));
        assert!(!sys.is_subtype(list_list_int, iterable_iterable_string));

        // Warmup done: the pool holds exactly one unit per nesting level and
        // repeated identical queries return identical answers, reuse the
        // units, and grow no pool — neither the type pool (every interface
        // intern below is a dedup hit) nor the side pools (the dedup-hit
        // rollback must reclaim the arguments `instantiate` appends).
        let len = table.len();
        let side_pools = table.side_pool_lens();
        let units = sys.walk.borrow().pooled_units();
        assert_eq!(units, 3, "one recycled unit per re-entry level");
        for _ in 0..8 {
            assert!(sys.is_subtype(list_list_int, iterable_iterable_num));
            assert!(!sys.is_subtype(list_list_int, iterable_iterable_string));
            assert_eq!(sys.walk.borrow().pooled_units(), units);
            assert_eq!(
                table.len(),
                len,
                "repeated nested queries must not grow the type pool"
            );
            assert_eq!(
                table.side_pool_lens(),
                side_pools,
                "repeated nested queries must not grow the side pools"
            );
        }
    }

    /// The supertype-closure frontier is pooled like the walk units: claimed
    /// by the LUB fallback for its two closure walks and returned after, so
    /// repeated unrelated-pair LUBs reuse one buffer instead of allocating a
    /// fresh `vec![root]` per call.
    #[test]
    fn lub_closure_frontier_is_pooled() {
        let (table, hierarchy) = fixture();
        let sys = TypeSystem::new(&table, &hierarchy);
        let int = itf(&table, INT, &[]);
        let string = itf(&table, STRING, &[]);
        let object = itf(&table, OBJECT, &[]);
        // The unrelated-pair LUB runs the supertype-closure fallback.
        assert_eq!(sys.least_upper_bound(int, string), object);
        let cap = sys.walk.borrow().closure_frontier_capacity();
        assert!(cap > 0, "the closure frontier must be pooled");
        for _ in 0..8 {
            assert_eq!(sys.least_upper_bound(int, string), object);
            assert_eq!(
                sys.walk.borrow().closure_frontier_capacity(),
                cap,
                "the pooled frontier must be reused, never reallocated"
            );
        }
    }

    #[test]
    fn stamp_set_epoch_reuse() {
        let mut stamps = StampSet::new();
        let a = TypeId::from_raw(3);
        assert!(stamps.insert(a));
        assert!(!stamps.insert(a));
        // A clear must make previously stamped ids unseen again without
        // rewriting their slots.
        stamps.clear();
        assert!(stamps.insert(a));
    }

    #[test]
    fn stamp_set_grows_lazily_and_keeps_old_stamps() {
        let mut stamps = StampSet::new();
        assert!(stamps.insert(TypeId::from_raw(0)));
        // Growth past the current length (the walk interns new ids).
        assert!(stamps.insert(TypeId::from_raw(10)));
        assert!(!stamps.insert(TypeId::from_raw(0)));
        assert!(!stamps.insert(TypeId::from_raw(10)));
        assert!(stamps.insert(TypeId::from_raw(11)));
    }

    #[test]
    fn stamp_set_wraparound_resets_stamps() {
        let mut stamps = StampSet::new();
        let id = TypeId::from_raw(0);
        assert!(stamps.insert(id));
        // Drive the epoch to the wraparound point: the next clear must
        // refill the slots instead of reusing epoch 0 (which would alias
        // the zero-initialized slots and resurrect stale entries).
        stamps.epoch = u32::MAX;
        stamps.clear();
        assert_eq!(stamps.epoch, 1);
        assert!(stamps.insert(id));
        assert!(!stamps.insert(id));
    }
}

/// `can_be_subtype_of` pins and the mutual-exclusion law
/// (`canBeSubtypeOf ⊇ isSubtypeOf`, type_system.dart:150-330).
#[cfg(test)]
mod can_be_subtype_tests {
    use super::*;
    use crate::hierarchy::CoreDecls;
    use crate::ids::{TypeDeclId, TypeParamId};
    use crate::ty::{NamedMember, UninternedFunction, UninternedRecord};

    const OBJECT: TypeDeclId = TypeDeclId::from_raw(1);
    const FUTURE: TypeDeclId = TypeDeclId::from_raw(2);
    const FUNCTION: TypeDeclId = TypeDeclId::from_raw(3);
    const RECORD: TypeDeclId = TypeDeclId::from_raw(9);
    const NUM: TypeDeclId = TypeDeclId::from_raw(4);
    const INT: TypeDeclId = TypeDeclId::from_raw(5);
    const STRING: TypeDeclId = TypeDeclId::from_raw(6);
    const SEALED: TypeDeclId = TypeDeclId::from_raw(20);
    const SUB_A: TypeDeclId = TypeDeclId::from_raw(21);
    const SUB_B: TypeDeclId = TypeDeclId::from_raw(22);
    const SEALED_G: TypeDeclId = TypeDeclId::from_raw(23);
    const SUB_G_A: TypeDeclId = TypeDeclId::from_raw(24);
    const ENUM: TypeDeclId = TypeDeclId::from_raw(30);
    const OTHER: TypeDeclId = TypeDeclId::from_raw(40);
    const BOX: TypeDeclId = TypeDeclId::from_raw(41);

    fn itf(table: &TypeTable, decl: TypeDeclId, args: &[TypeId]) -> TypeId {
        table.intern(
            UninternedType::Interface {
                decl,
                args: args.to_vec(),
            },
            false,
        )
    }

    fn nullable(table: &TypeTable, ty: UninternedType) -> TypeId {
        table.intern(ty, true)
    }

    fn fixture() -> (TypeTable, TypeHierarchy) {
        let table = TypeTable::new();
        let (g, box_e) = (TypeParamId::from_raw(1), TypeParamId::from_raw(2));
        let g_ty = table.intern(UninternedType::TypeParameter { parameter: g }, false);
        let hierarchy = TypeHierarchy::builder(CoreDecls {
            object: OBJECT,
            future: FUTURE,
            function: FUNCTION,
            record: Some(RECORD),
        })
        .decl(OBJECT, Vec::new())
        .decl(FUTURE, vec![box_e])
        .decl(FUNCTION, Vec::new())
        .decl(RECORD, Vec::new())
        .decl(NUM, Vec::new())
        .decl(INT, Vec::new())
        .decl(STRING, Vec::new())
        .decl(SEALED, Vec::new())
        .decl(SUB_A, Vec::new())
        .decl(SUB_B, Vec::new())
        .decl(SEALED_G, vec![g])
        .decl(SUB_G_A, Vec::new())
        .decl(ENUM, Vec::new())
        .decl(OTHER, Vec::new())
        .decl(BOX, vec![box_e])
        .supertype(NUM, itf(&table, OBJECT, &[]))
        .supertype(INT, itf(&table, NUM, &[]))
        .supertype(STRING, itf(&table, OBJECT, &[]))
        .supertype(SEALED, itf(&table, OBJECT, &[]))
        .supertype(SUB_A, itf(&table, SEALED, &[]))
        .supertype(SUB_B, itf(&table, SEALED, &[]))
        .supertype(SEALED_G, itf(&table, OBJECT, &[]))
        .supertype(SUB_G_A, itf(&table, SEALED_G, &[g_ty]))
        .supertype(ENUM, itf(&table, OBJECT, &[]))
        .supertype(OTHER, itf(&table, OBJECT, &[]))
        .supertype(BOX, itf(&table, OBJECT, &[]))
        .sealed(SEALED)
        .sealed(SEALED_G)
        .enumeration(ENUM, 3)
        .build(&table)
        .expect("fixture hierarchy is valid");
        (table, hierarchy)
    }

    #[test]
    fn can_be_subtype_of_official_pins() {
        let (table, hierarchy) = fixture();
        let sys = TypeSystem::new(&table, &hierarchy);
        let int = itf(&table, INT, &[]);
        let int_q = nullable(
            &table,
            UninternedType::Interface {
                decl: INT,
                args: vec![],
            },
        );
        let num = itf(&table, NUM, &[]);
        let num_q = nullable(
            &table,
            UninternedType::Interface {
                decl: NUM,
                args: vec![],
            },
        );
        let string = itf(&table, STRING, &[]);
        let null = table.intern(UninternedType::Null, false);
        let never = table.intern(UninternedType::Never, false);
        let dynamic = table.intern(UninternedType::Dynamic, false);
        let void_ty = table.intern(UninternedType::Void, false);
        let object = itf(&table, OBJECT, &[]);

        // The doc example: both directions of the num/int pair.
        assert!(sys.can_be_subtype_of(num, int));
        assert!(sys.can_be_subtype_of(int, num));
        assert!(sys.can_be_subtype_of(int, int));

        // Nullability arms.
        assert!(sys.can_be_subtype_of(int, int_q));
        assert!(sys.can_be_subtype_of(int_q, int));
        assert!(sys.can_be_subtype_of(int_q, int_q));
        assert!(sys.can_be_subtype_of(num_q, int_q));
        assert!(!sys.can_be_subtype_of(null, int));
        assert!(sys.can_be_subtype_of(null, int_q));
        assert!(sys.can_be_subtype_of(null, null));
        assert!(sys.can_be_subtype_of(never, int));
        assert!(sys.can_be_subtype_of(never, never));

        // Unsealed leaves are conservative-true (official parity: neither
        // side has a closed subtype set, and dynamic/void fall to the
        // default arm).
        assert!(sys.can_be_subtype_of(int, string));
        assert!(sys.can_be_subtype_of(dynamic, int));
        assert!(sys.can_be_subtype_of(void_ty, object));

        // The sealed closure: only the closed candidate set can implement
        // the other side.
        let sealed = itf(&table, SEALED, &[]);
        let sub_a = itf(&table, SUB_A, &[]);
        let sub_b = itf(&table, SUB_B, &[]);
        let other = itf(&table, OTHER, &[]);
        assert!(sys.can_be_subtype_of(sealed, sub_a));
        assert!(sys.can_be_subtype_of(sealed, sub_b));
        assert!(!sys.can_be_subtype_of(sealed, other));
        assert!(sys.can_be_subtype_of(sub_a, sealed));
        assert!(!sys.can_be_subtype_of(sealed, num));
        // Subclasses of a sealed class are themselves open — conservative.
        assert!(sys.can_be_subtype_of(sub_a, other));

        // Enum-constant enumeration: an enum can be its supertypes and
        // itself, but not an unrelated sealed class.
        let enum_ty = itf(&table, ENUM, &[]);
        assert!(sys.can_be_subtype_of(enum_ty, enum_ty));
        assert!(sys.can_be_subtype_of(enum_ty, object));
        assert!(!sys.can_be_subtype_of(enum_ty, sealed));
        let enum_q = nullable(
            &table,
            UninternedType::Interface {
                decl: ENUM,
                args: vec![],
            },
        );
        assert!(sys.can_be_subtype_of(enum_q, object));
        assert!(!sys.can_be_subtype_of(enum_q, sealed));

        // Generic sealed classes: the right-closure walk finds the left
        // among the candidates (each candidate-as-left is checked against
        // the left's own arguments — the official is conservative here and
        // never consults the right's instantiation).
        let sealed_g_int = itf(&table, SEALED_G, &[int]);
        let sealed_g_string = itf(&table, SEALED_G, &[string]);
        let sub_g_a = itf(&table, SUB_G_A, &[]);
        assert!(sys.can_be_subtype_of(sub_g_a, sealed_g_int));
        assert!(sys.can_be_subtype_of(sub_g_a, sealed_g_string));

        // Function/interface pairs: functions meet Object and bare
        // Function; a concrete interface meets a concrete function type
        // under neither (type_system.dart:182-185 answers `false`), while
        // the bare-`Function` pair falls to the conservative default.
        let fn_int = table
            .intern_function(
                UninternedFunction {
                    return_type: int,
                    type_parameters: Vec::new(),
                    positional: Vec::new(),
                    named: Vec::new(),
                    required_positional: 0,
                },
                false,
            )
            .expect("valid signature");
        let bare_function = itf(&table, FUNCTION, &[]);
        assert!(sys.can_be_subtype_of(fn_int, object));
        assert!(sys.can_be_subtype_of(fn_int, bare_function));
        assert!(!sys.can_be_subtype_of(fn_int, int));
        assert!(!sys.can_be_subtype_of(int, fn_int));
        assert!(sys.can_be_subtype_of(bare_function, fn_int));

        // Records: never functions; interfaces only at Object/Record.
        let pair_int = table
            .intern_record(
                UninternedRecord {
                    positional: vec![int],
                    named: Vec::new(),
                },
                false,
            )
            .expect("valid shape");
        let pair_string = table
            .intern_record(
                UninternedRecord {
                    positional: vec![string],
                    named: Vec::new(),
                },
                false,
            )
            .expect("valid shape");
        assert!(sys.can_be_subtype_of(pair_int, pair_int));
        assert!(sys.can_be_subtype_of(pair_int, pair_string));
        assert!(!sys.can_be_subtype_of(pair_int, fn_int));
        assert!(sys.can_be_subtype_of(pair_int, object));
        let arity_mismatch = table
            .intern_record(
                UninternedRecord {
                    positional: vec![int, int],
                    named: Vec::new(),
                },
                false,
            )
            .expect("valid shape");
        assert!(!sys.can_be_subtype_of(pair_int, arity_mismatch));
    }

    #[test]
    fn can_be_subtype_of_futureor_splits() {
        let (table, hierarchy) = fixture();
        let sys = TypeSystem::new(&table, &hierarchy);
        let int = itf(&table, INT, &[]);
        let string = itf(&table, STRING, &[]);
        let future_or_int = table.intern(UninternedType::FutureOr { base: int }, false);
        let future_or_int_q = table.intern(UninternedType::FutureOr { base: int }, true);
        let future_or_string = table.intern(UninternedType::FutureOr { base: string }, false);

        // `FutureOr<T> = T || Future<T>`: both legs are candidates, in both
        // directions, and the nullable wrapper only adds `Null`.
        assert!(sys.can_be_subtype_of(future_or_int, int));
        assert!(sys.can_be_subtype_of(int, future_or_int));
        assert!(sys.can_be_subtype_of(future_or_int_q, int));
        assert!(sys.can_be_subtype_of(future_or_int, string));
        assert!(sys.can_be_subtype_of(string, future_or_int));
        // Both FutureOr pairs split into unsealed leaf pairs on at least one
        // leg (`Future<int>` vs `String`, `int` vs `Future<String>`), and
        // unsealed leaves are conservatively true.
        assert!(sys.can_be_subtype_of(future_or_int, future_or_string));
        assert!(sys.can_be_subtype_of(future_or_string, future_or_int));
    }

    /// The load-bearing invariant: `isSubtypeOf(a, b) ⇒ canBeSubtypeOf(a, b)`
    /// over every ordered pair of a corpus spanning all ten kinds. This is
    /// what keeps "the cast/subtype check is unnecessary" and "the pattern
    /// never matches" mutually exclusive.
    #[test]
    fn mutual_exclusion_law_over_fixture_corpus() {
        let (table, hierarchy) = fixture();
        let sys = TypeSystem::new(&table, &hierarchy);
        let int = itf(&table, INT, &[]);
        let int_q = nullable(
            &table,
            UninternedType::Interface {
                decl: INT,
                args: vec![],
            },
        );
        let num = itf(&table, NUM, &[]);
        let num_q = nullable(
            &table,
            UninternedType::Interface {
                decl: NUM,
                args: vec![],
            },
        );
        let string = itf(&table, STRING, &[]);
        let param = TypeParamId::from_raw(9);
        let t = table.intern(UninternedType::TypeParameter { parameter: param }, false);
        let t_q = table.intern(UninternedType::TypeParameter { parameter: param }, true);
        let fn_int = table
            .intern_function(
                UninternedFunction {
                    return_type: int,
                    type_parameters: Vec::new(),
                    positional: vec![num],
                    named: vec![NamedMember {
                        name: "x".to_string(),
                        ty: int,
                        required: true,
                    }],
                    required_positional: 1,
                },
                false,
            )
            .expect("valid signature");
        let fn_q = table
            .intern_function(
                UninternedFunction {
                    return_type: int,
                    type_parameters: Vec::new(),
                    positional: Vec::new(),
                    named: Vec::new(),
                    required_positional: 0,
                },
                true,
            )
            .expect("valid signature");
        let record = table
            .intern_record(
                UninternedRecord {
                    positional: vec![int],
                    named: vec![NamedMember {
                        name: "y".to_string(),
                        ty: num,
                        required: false,
                    }],
                },
                false,
            )
            .expect("valid shape");
        let record_q = table
            .intern_record(
                UninternedRecord {
                    positional: vec![int],
                    named: Vec::new(),
                },
                true,
            )
            .expect("valid shape");

        let corpus = vec![
            int,
            int_q,
            num,
            num_q,
            string,
            itf(&table, OBJECT, &[]),
            itf(&table, SEALED, &[]),
            itf(&table, SUB_A, &[]),
            itf(&table, SUB_B, &[]),
            itf(&table, SEALED_G, &[int]),
            itf(&table, SUB_G_A, &[]),
            itf(&table, ENUM, &[]),
            nullable(
                &table,
                UninternedType::Interface {
                    decl: ENUM,
                    args: vec![],
                },
            ),
            itf(&table, OTHER, &[]),
            itf(&table, FUNCTION, &[]),
            itf(&table, BOX, &[int]),
            table.intern(UninternedType::Null, false),
            table.intern(UninternedType::Never, false),
            table.intern(UninternedType::Dynamic, false),
            table.intern(UninternedType::Void, false),
            table.intern(UninternedType::Invalid, false),
            table.intern(UninternedType::FutureOr { base: int }, false),
            table.intern(UninternedType::FutureOr { base: int }, true),
            table.intern(UninternedType::FutureOr { base: int_q }, false),
            t,
            t_q,
            fn_int,
            fn_q,
            record,
            record_q,
        ];
        for (i, &a) in corpus.iter().enumerate() {
            for &b in corpus.iter().skip(i) {
                if sys.is_subtype(a, b) {
                    assert!(
                        sys.can_be_subtype_of(a, b),
                        "law violated: is_subtype({a:?}, {b:?}) but !can_be_subtype_of"
                    );
                }
                if sys.is_subtype(b, a) {
                    assert!(
                        sys.can_be_subtype_of(b, a),
                        "law violated: is_subtype({b:?}, {a:?}) but !can_be_subtype_of"
                    );
                }
            }
        }
    }
}
