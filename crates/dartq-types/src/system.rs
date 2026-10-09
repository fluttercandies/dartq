//! The subtype relation, assignability, and nullability predicates.
//!
//! Direct port of the analyzer's rule table
//! (`pkg/analyzer/lib/src/dart/element/subtype.dart:262-523`): the rule order
//! below is the source order, and short-circuiting matches it exactly.
//! M1 omissions (spec §5) are marked inline: no promoted bounds, no declared
//! bounds on type parameters, no variance annotations (class parameters are
//! covariant, `element.dart:11560-11561`), no extension types, no
//! `UnknownInferredType`.

use std::cell::RefCell;
use std::cmp::Ordering;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::hierarchy::{CoreDecls, DeclInfo, TypeHierarchy};
use crate::ids::{FunctionSigId, TypeDeclId, TypeId};
use crate::table::TypeTable;
use crate::ty::{PoolRange, Type, TypeKind, UninternedFunction, UninternedRecord, UninternedType};

/// Recursion guard, mirroring the analyzer's `TypeOperationDepthLimitExceeded`
/// mechanism with the same limit (`type_system.dart:85`); exceeding it makes
/// `is_subtype` return `false` (`subtype.dart:37-41`). It guards recursion
/// only: the hierarchy's inheritance depths are precomputed exactly
/// (topologically, no cap) so the LUB tie-break matches the oracle's uncapped
/// `computeLongestInheritancePathToObject`, and the LUB supertype closure is
/// an iterative worklist, uncapped like the oracle's plain recursion
/// (`least_upper_bound.dart:162-177`).
pub(crate) const DEPTH_LIMIT: u32 = 100;

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

/// Reusable subtype-walk buffers: the interface walk's visited set and LIFO
/// frontier, plus the LUB supertype-closure sets and their intersection.
/// Cleared (not reallocated) at the start of every use, so steady-state
/// queries allocate nothing.
#[derive(Clone, Default)]
pub(crate) struct WalkScratch {
    pub(crate) visited: StampSet,
    pub(crate) frontier: Vec<TypeId>,
    pub(crate) set_a: FxHashSet<TypeId>,
    pub(crate) set_b: FxHashSet<TypeId>,
    pub(crate) candidates: Vec<TypeId>,
}

/// Facade over a [`TypeTable`] plus a [`TypeHierarchy`] snapshot. All queries
/// take `&self`: interning synthesized types (`Future<S>`, `Object?`, stripped
/// projections) goes through the table's interior mutability, so the table
/// grows but ids never change.
///
/// The system owns two non-reentrant scratch cells:
///
/// - `walk`: the subtype-walk and LUB-closure buffers. A walk claims the
///   cell for its whole duration; a *re-entrant* query (rule 12 reached
///   again through an argument check while an outer interface walk is in
///   flight) cannot claim it and transparently falls back to fresh local
///   containers. Claim detection is `try_borrow_mut`, so no nesting depth
///   bookkeeping is needed.
/// - `future_memo`: `Future<S>` synthesis keyed by the argument id. It is
///   only ever held across a single lookup/insert, never across a recursive
///   query, so re-entrancy cannot arise.
///
/// Both are single-threaded, matching [`TypeTable`]'s interior `RefCell`.
#[derive(Clone)]
pub struct TypeSystem<'a> {
    table: &'a TypeTable,
    hierarchy: &'a TypeHierarchy,
    pub(crate) walk: RefCell<WalkScratch>,
    future_memo: RefCell<FxHashMap<TypeId, TypeId>>,
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
    /// diagnostics.
    #[must_use]
    pub fn is_subtype(&self, t0: TypeId, t1: TypeId) -> bool {
        self.subtype_impl(t0, t1, 0)
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
    pub(crate) fn with_nullability(&self, t: TypeId, nullable: bool) -> TypeId {
        let ty = self.table.get(t);
        if ty.is_nullable() == nullable {
            return t;
        }
        match ty.kind() {
            TypeKind::Interface { decl, args } => {
                let rebuilt = UninternedType::Interface {
                    decl,
                    args: self.table.args(args),
                };
                self.table.intern(rebuilt, nullable)
            }
            TypeKind::Function { signature } => self
                .table
                .intern_function(self.unintern_function(signature), nullable)
                .expect("re-interning an interned signature cannot fail"),
            TypeKind::Record { shape } => self
                .table
                .intern_record(self.unintern_record(shape), nullable)
                .expect("re-interning an interned shape cannot fail"),
            other => self.table.intern(unintern(other), nullable),
        }
    }

    pub(crate) fn strip(&self, t: TypeId) -> TypeId {
        self.with_nullability(t, false)
    }

    pub(crate) fn make_nullable(&self, t: TypeId) -> TypeId {
        self.with_nullability(t, true)
    }

    fn unintern_function(&self, id: FunctionSigId) -> UninternedFunction {
        let sig = self.table.signature(id);
        UninternedFunction {
            return_type: sig.return_type,
            type_parameters: self.table.function_type_parameters(sig.type_parameters),
            positional: self.table.args(sig.positional),
            named: self.table.named_members(sig.named),
            required_positional: sig.required_positional,
        }
    }

    fn unintern_record(&self, id: crate::ids::RecordShapeId) -> UninternedRecord {
        let shape = self.table.record_shape(id);
        UninternedRecord {
            positional: self.table.args(shape.positional),
            named: self.table.named_members(shape.named),
        }
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

    /// The core subtype walk; `depth` guards runaway instantiation chains.
    fn subtype_impl(&self, t0: TypeId, t1: TypeId, depth: u32) -> bool {
        // Depth guard before any rule (subtype.dart:33-41): the oracle's
        // `enterTypeOperation` (type_system.dart:372-378) runs before
        // `_isSubtypeOf` consults reflexivity, so identical pairs beyond the
        // limit answer `false` too. Top-level queries start at depth 0 and
        // never trip it.
        if depth >= DEPTH_LIMIT {
            return false;
        }
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
            && self.subtype_impl(self.object_question(), t1, depth + 1)
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
                return self.subtype_impl(base, t1, depth + 1);
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
                    return self.subtype_impl(self.null_type(), s1, depth + 1);
                }
                return true;
            }
            return matches!(b.kind(), TypeKind::Null) || b.is_nullable();
        }

        // Rule 6 Left FutureOr (subtype.dart:372-385).
        if let Some(s0) = Self::non_null_future_or_base(a) {
            if !self.subtype_impl(s0, t1, depth + 1) {
                return false;
            }
            let future_s0 = self.intern_future(s0);
            return self.subtype_impl(future_s0, t1, depth + 1);
        }

        // Rule 7 Left Nullable (subtype.dart:389-392).
        if a.is_nullable() {
            let s0 = self.strip(t0);
            return self.subtype_impl(s0, t1, depth + 1)
                && self.subtype_impl(self.null_type(), t1, depth + 1);
        }

        // Rules 8-9 (subtype.dart:397-415): promoted-bound reflexivity and
        // promotion — M1 has no promoted bounds, and unpromoted type-variable
        // reflexivity is subsumed by rule 0a.

        // Rule 10 Right FutureOr (subtype.dart:418-449).
        if let Some(s1) = Self::non_null_future_or_base(b) {
            let future_s1 = self.intern_future(s1);
            if self.subtype_impl(t0, future_s1, depth + 1) {
                return true;
            }
            if self.subtype_impl(t0, s1, depth + 1) {
                return true;
            }
            // Type-variable bound paths (subtype.dart:437-446): M1 has no
            // declared bounds.
            return false;
        }

        // Rule 11 Right Nullable (subtype.dart:452-477).
        if b.is_nullable() {
            let s1 = self.strip(t1);
            if self.subtype_impl(t0, s1, depth + 1) {
                return true;
            }
            if self.subtype_impl(t0, self.null_type(), depth + 1) {
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
            return self.interface_subtype(d0, a0, d1, a1, depth);
        }

        // Left Type Variable Bound (subtype.dart:490-500): M1 has no bounds.

        // Rule 13 Function (subtype.dart:502-510).
        if let TypeKind::Function { signature: f } = a.kind() {
            if self.is_bare_function(b) {
                return true;
            }
            if let TypeKind::Function { signature: g } = b.kind() {
                return self.function_subtype(f, g, depth);
            }
        }

        // Rule 14 Record/Record (subtype.dart:512-519): M2 (spec §5).
        false
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
        depth: u32,
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
            return self.arguments_subtype(a0, a1, depth);
        }
        // Class types cannot subtype `Function` or vice versa
        // (subtype.dart:206-209).
        if d0 == core.function || d1 == core.function {
            return false;
        }
        // Depth-first walk over instantiated supertypes, the counterpart of
        // `allSupertypes` + substitution (subtype.dart:211-221). Canonical
        // interning makes `TypeId` a sound visited-set key. The reusable
        // scratch is claimed for the whole walk; a re-entrant walk (rule 12
        // reached through an argument check of an outer walk) falls back to
        // fresh local containers — see the `TypeSystem` scratch docs.
        if let Ok(mut scratch) = self.walk.try_borrow_mut() {
            scratch.visited.clear();
            scratch.frontier.clear();
            self.seed_frontier(d0, a0, &mut scratch.frontier);
            let WalkScratch {
                visited, frontier, ..
            } = &mut *scratch;
            self.interface_walk(d1, a1, depth, visited, frontier)
        } else {
            // Re-entrant walk (rule 12 reached through an argument check of
            // an outer walk that owns the scratch): fresh local containers.
            let mut visited = StampSet::new();
            let mut frontier = Vec::new();
            self.seed_frontier(d0, a0, &mut frontier);
            self.interface_walk(d1, a1, depth, &mut visited, &mut frontier)
        }
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
        depth: u32,
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
                return self.arguments_subtype(args, a1, depth);
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
    fn arguments_subtype(&self, sub: PoolRange, sup: PoolRange, depth: u32) -> bool {
        assert_eq!(
            sub.len(),
            sup.len(),
            "interface arity mismatch: resolution must validate argument counts"
        );
        for i in 0..sup.len() {
            if !self.subtype_impl(
                self.table.arg(sub.offset() + i),
                self.table.arg(sup.offset() + i),
                depth + 1,
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
    fn function_subtype(&self, f_id: FunctionSigId, g_id: FunctionSigId, depth: u32) -> bool {
        let f = self.table.signature(f_id);
        let g = self.table.signature(g_id);
        // Universally quantified correlation: rename f's parameters to g's.
        let (f_params, g_params) = (f.type_parameters, g.type_parameters);
        if f_params.len() != g_params.len() {
            return false;
        }
        // Return type is covariant (subtype.dart:98-100).
        if !self.subtype_impl(
            self.table
                .substitute_rename(f.return_type, f_params, g_params),
            g.return_type,
            depth + 1,
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
            if !self.subtype_impl(gp, fp, depth + 1) {
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
                    if !self.subtype_impl(self.table.named_ty(gi), ft, depth + 1) {
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

/// Rebuilds the uninterned form of a kind with no pooled payload.
fn unintern(kind: TypeKind) -> UninternedType {
    match kind {
        TypeKind::Never => UninternedType::Never,
        TypeKind::Dynamic => UninternedType::Dynamic,
        TypeKind::Void => UninternedType::Void,
        TypeKind::Invalid => UninternedType::Invalid,
        TypeKind::Null => UninternedType::Null,
        TypeKind::TypeParameter { parameter } => UninternedType::TypeParameter { parameter },
        TypeKind::FutureOr { base } => UninternedType::FutureOr { base },
        TypeKind::Interface { .. } | TypeKind::Function { .. } | TypeKind::Record { .. } => {
            unreachable!("compound kinds are rebuilt by `TypeSystem::with_nullability`")
        }
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
    /// `int <: num` while the outer interface walk still owns the scratch,
    /// so the inner query runs on fresh local containers. The pinned
    /// contract: the fallback answers exactly what the flat query answers.
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
