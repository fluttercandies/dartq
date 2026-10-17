//! Least upper bound (interface superclass chain + nullability merge;
//! `FutureOr` merging included, GLB/closure merging deferred to M2).
//!
//! Port of `LeastUpperBoundHelper._computeLeastUpperBound`
//! (`least_upper_bound.dart:391-672`) with the documented M1 fallbacks:
//! function/function yields bare `Function` (the official fallback when
//! correlation fails, least_upper_bound.dart:684-686). Record/record pairs
//! run the official field-wise merge (`_recordType`,
//! least_upper_bound.dart:852-885): arity or field-name mismatch answers the
//! declared `Record` class, mergeable shapes merge field-wise. Only
//! record-vs-OTHER (and snapshots without a `Record` class — the semantic
//! layer must supply one; Dart always has it) take the official fallback
//! `UP(Object, T2)` (least_upper_bound.dart:652-659).

use std::cmp::Ordering;

use rustc_hash::FxHashSet;

use crate::ids::TypeId;
use crate::system::{DepthLimitExceeded, TypeSystem, WalkScratch};
use crate::ty::{NamedMember, Type, TypeKind, UninternedRecord, UninternedType};

impl TypeSystem<'_> {
    /// The least upper bound of two types; the static type of `c ? a : b`
    /// and the result type of `a ?? b` (before the nullability dance the
    /// checker applies).
    ///
    /// Depth is bounded by the shared type-operation cell
    /// (`type_system.dart:85`, `:372-382`): a >100-deep LUB nesting unwinds
    /// to this outermost frame, which degrades to `Object?` exactly like the
    /// oracle (`least_upper_bound.dart:378-386`).
    #[must_use]
    pub fn least_upper_bound(&self, a: TypeId, b: TypeId) -> TypeId {
        self.lub_step(a, b)
            .unwrap_or_else(|DepthLimitExceeded| self.object_question())
    }

    /// One `getLeastUpperBound` frame: enters the shared depth cell (the
    /// oracle re-enters per recursion level, `least_upper_bound.dart:379`) and
    /// runs the rule table. `Err` is the oracle's rethrow
    /// (least_upper_bound.dart:383-385) — it unwinds every nested frame to
    /// the outermost entry, which is [`TypeSystem::least_upper_bound`].
    fn lub_step(&self, a: TypeId, b: TypeId) -> Result<TypeId, DepthLimitExceeded> {
        self.enter_type_operation()?;
        let answer = self.lub_compute(a, b);
        self.exit_type_operation();
        answer
    }

    /// `_computeLeastUpperBound` (least_upper_bound.dart:391-672) with the
    /// documented M1 fallbacks; recursion goes through [`Self::lub_step`] so
    /// every level re-enters the shared depth cell.
    #[expect(
        clippy::too_many_lines,
        reason = "port of least_upper_bound.dart's recursive case analysis; the arm order is \
                  the oracle's, so the body stays one function"
    )]
    fn lub_compute(&self, a: TypeId, b: TypeId) -> Result<TypeId, DepthLimitExceeded> {
        // UP(T, T) = T (least_upper_bound.dart:393-395).
        if a == b {
            return Ok(a);
        }
        let t1 = self.table().get(a);
        let t2 = self.table().get(b);

        // TOP (least_upper_bound.dart:405-427).
        let top1 = self.is_top(a);
        let top2 = self.is_top(b);
        if top1 && top2 {
            return Ok(if self.is_more_top(a, b) { a } else { b });
        }
        if top1 {
            return Ok(a);
        }
        if top2 {
            return Ok(b);
        }

        // BOTTOM (least_upper_bound.dart:429-451). `Never` is the only bottom
        // type in M1 (`Never?` is interned as `Null`).
        if self.is_bottom(a) {
            return Ok(b);
        }
        if self.is_bottom(b) {
            return Ok(a);
        }

        // NULL (least_upper_bound.dart:487-524).
        if self.is_null(a) {
            return Ok(self.null_case(b));
        }
        if self.is_null(b) {
            return Ok(self.null_case(a));
        }

        // OBJECT (least_upper_bound.dart:526-560). The both-object rule runs
        // first: `FutureOr<Object>` is an object type
        // (`isObject(FutureOr<T>) = isObject(T)`, type_system.dart:1236-1254),
        // so `UP(FutureOr<Object>, Object)` must resolve by MORETOP instead
        // of returning the first single-object hit.
        let a_is_object = self.is_object(a);
        let b_is_object = self.is_object(b);
        if a_is_object && b_is_object {
            return Ok(if self.is_more_top(a, b) { a } else { b });
        }
        if a_is_object {
            return Ok(if self.is_non_nullable(b) {
                a
            } else {
                self.make_nullable(a)
            });
        }
        if b_is_object {
            return Ok(if self.is_non_nullable(a) {
                b
            } else {
                self.make_nullable(b)
            });
        }

        // UP(T1?, T2) = UP(T1, T2)? (least_upper_bound.dart:562-571).
        if t1.is_nullable() || t2.is_nullable() {
            let s = self.lub_step(self.strip(a), self.strip(b))?;
            return Ok(self.make_nullable(s));
        }

        // Type parameters (least_upper_bound.dart:576-609). M1 has no
        // declared bounds; the official default bound is `Object?`
        // (least_upper_bound.dart:893-899), which makes the fallback
        // `Object?` exactly.
        if matches!(t1.kind(), TypeKind::TypeParameter { .. })
            || matches!(t2.kind(), TypeKind::TypeParameter { .. })
        {
            if self.is_subtype(a, b) {
                return Ok(b);
            }
            if self.is_subtype(b, a) {
                return Ok(a);
            }
            return Ok(self.object_question());
        }

        // FUNCTION (least_upper_bound.dart:611-635).
        if matches!(t1.kind(), TypeKind::Function { .. }) {
            if self.is_bare_function_id(b) {
                return Ok(b);
            }
            if matches!(t2.kind(), TypeKind::Function { .. }) {
                // M1: no parameter-wise GLB merge — the coarse upper bound
                // `Function` (the official correlation-failure fallback).
                return Ok(self.intern_bare_function());
            }
            return Ok(self.up_object_with(b));
        }
        if matches!(t2.kind(), TypeKind::Function { .. }) {
            if self.is_bare_function_id(a) {
                return Ok(a);
            }
            return Ok(self.up_object_with(a));
        }

        // RECORD (least_upper_bound.dart:637-660). `UP((...), Record) =
        // Record` and `UP(Record, (...)) = Record`
        // (least_upper_bound.dart:637-644). The both-record arm dispatches to
        // `_recordType` (least_upper_bound.dart:648-650, :852-885), which
        // answers the bare `Record` class for non-mergeable shapes
        // (positional/named arity mismatch, :855-863; field-name mismatch,
        // :873-876) and field-merges otherwise (:865-884). Only
        // record-vs-OTHER falls through to the official fallback
        // `UP(RecordType, T2) = UP(Object, T2)` / `UP(T1, RecordType) =
        // UP(T1, Object)` (least_upper_bound.dart:652-659); snapshots
        // without a `Record` class use it for every record pair too.
        if matches!(t1.kind(), TypeKind::Record { .. })
            || matches!(t2.kind(), TypeKind::Record { .. })
        {
            if let Some(record) = self.hierarchy().core().record {
                // Rule order matches the source: the `Record` class wins
                // before a record shape, on either side.
                let is_record_class = |t: Type| matches!(t.kind(), TypeKind::Interface { decl, .. } if decl == record);
                if is_record_class(t2) {
                    return Ok(b);
                }
                if is_record_class(t1) {
                    return Ok(a);
                }
                if matches!(t1.kind(), TypeKind::Record { .. })
                    && matches!(t2.kind(), TypeKind::Record { .. })
                {
                    return self.record_merge(a, b, record);
                }
            }
            return Ok(if matches!(t1.kind(), TypeKind::Record { .. }) {
                self.up_object_with(b)
            } else {
                self.up_object_with(a)
            });
        }

        // FUTUREOR (least_upper_bound.dart:662, :796-844).
        if let Some(result) = self.future_or_lub(a, b)? {
            return Ok(result);
        }

        // INTERFACE (least_upper_bound.dart:670-671).
        self.interface_lub(a, b)
    }

    /// UP(NULL, T) = T if T is nullable, `T?` otherwise
    /// (least_upper_bound.dart:505-513).
    fn null_case(&self, other: TypeId) -> TypeId {
        if self.is_nullable(other) {
            other
        } else {
            self.make_nullable(other)
        }
    }

    /// `_recordType` (least_upper_bound.dart:852-885): field-wise merge of
    /// two record types — positional fields pairwise LUB in order, named
    /// fields pairwise LUB under equal names. An arity mismatch or a
    /// name mismatch answers the bare `Record` class (the source's
    /// `typeProvider.recordType` fallback, least_upper_bound.dart:855-863,
    /// :873-876). Both named lists are canonically name-sorted, so the index
    /// zip is exactly the oracle's name-paired loop.
    fn record_merge(
        &self,
        a: TypeId,
        b: TypeId,
        record_decl: crate::ids::TypeDeclId,
    ) -> Result<TypeId, DepthLimitExceeded> {
        let record_class = || {
            self.table().intern(
                UninternedType::Interface {
                    decl: record_decl,
                    args: Vec::new(),
                },
                false,
            )
        };
        let TypeKind::Record { shape: sa } = self.table().get(a).kind() else {
            unreachable!("caller checked the Record kind");
        };
        let TypeKind::Record { shape: sb } = self.table().get(b).kind() else {
            unreachable!("caller checked the Record kind");
        };
        let (sa, sb) = (self.table().record_shape(sa), self.table().record_shape(sb));
        if sa.positional.len() != sb.positional.len() || sa.named.len() != sb.named.len() {
            return Ok(record_class());
        }
        let mut positional = Vec::with_capacity(sa.positional.len());
        for i in 0..sa.positional.len() {
            positional.push(self.lub_step(
                self.table().arg(sa.positional.offset() + i),
                self.table().arg(sb.positional.offset() + i),
            )?);
        }
        let mut named = Vec::with_capacity(sa.named.len());
        for i in 0..sa.named.len() {
            let (ai, bi) = (sa.named.offset() + i, sb.named.offset() + i);
            if self.table().named_names_cmp(ai, bi) != Ordering::Equal {
                return Ok(record_class());
            }
            named.push(NamedMember {
                name: self.table().named_name(ai),
                ty: self.lub_step(self.table().named_ty(ai), self.table().named_ty(bi))?,
                required: false,
            });
        }
        Ok(self
            .table()
            .intern_record(UninternedRecord { positional, named }, false)
            .expect("merging well-formed records preserves well-formedness"))
    }

    /// UP(Object, T2) resolved inline (least_upper_bound.dart:541-549).
    fn up_object_with(&self, t: TypeId) -> TypeId {
        if self.is_non_nullable(t) {
            self.core_object_none()
        } else {
            self.object_question()
        }
    }

    /// `_futureOr` (least_upper_bound.dart:796-844). Only no-suffix
    /// projections reach here (the nullability merge ran earlier).
    fn future_or_lub(&self, a: TypeId, b: TypeId) -> Result<Option<TypeId>, DepthLimitExceeded> {
        let future_or = |t: TypeId| match self.table().get(t).kind() {
            TypeKind::FutureOr { base } => Some(base),
            _ => None,
        };
        let future = |t: TypeId| match self.table().get(t).kind() {
            TypeKind::Interface { decl, args }
                if decl == self.hierarchy().core().future && args.len() == 1 =>
            {
                Some(self.table().arg(args.offset()))
            }
            _ => None,
        };
        // UP(FutureOr<T1>, FutureOr<T2>) = FutureOr<UP(T1, T2)>
        if let (Some(x), Some(y)) = (future_or(a), future_or(b)) {
            return Ok(Some(self.intern_future_or(self.lub_step(x, y)?)));
        }
        // UP(Future<T1>, FutureOr<T2>) = FutureOr<UP(T1, T2)>
        if let (Some(x), Some(y)) = (future(a), future_or(b)) {
            return Ok(Some(self.intern_future_or(self.lub_step(x, y)?)));
        }
        // UP(FutureOr<T1>, Future<T2>) = FutureOr<UP(T1, T2)>
        if let (Some(x), Some(y)) = (future_or(a), future(b)) {
            return Ok(Some(self.intern_future_or(self.lub_step(x, y)?)));
        }
        // UP(T1, FutureOr<T2>) = FutureOr<UP(T1, T2)>
        if let Some(y) = future_or(b) {
            return Ok(Some(self.intern_future_or(self.lub_step(a, y)?)));
        }
        // UP(FutureOr<T1>, T2) = FutureOr<UP(T1, T2)>
        if let Some(x) = future_or(a) {
            return Ok(Some(self.intern_future_or(self.lub_step(x, b)?)));
        }
        Ok(None)
    }

    /// `InterfaceLeastUpperBoundHelper.compute`
    /// (least_upper_bound.dart:48-108) with nullability already merged.
    fn interface_lub(&self, a: TypeId, b: TypeId) -> Result<TypeId, DepthLimitExceeded> {
        // Either being a supertype of the other is the best result
        // (least_upper_bound.dart:55-60).
        if self.is_subtype(a, b) {
            return Ok(b);
        }
        if self.is_subtype(b, a) {
            return Ok(a);
        }
        let (
            TypeKind::Interface { decl: d1, args: s1 },
            TypeKind::Interface { decl: d2, args: s2 },
        ) = (self.table().get(a).kind(), self.table().get(b).kind())
        else {
            unreachable!("both operands are interface types here");
        };
        if d1 == d2 {
            // Pointwise merge of covariant arguments
            // (least_upper_bound.dart:62-100); class type parameters are
            // covariant in M1 (element.dart:11560-11561).
            let arg_count = s1.len();
            let mut merged = Vec::with_capacity(arg_count);
            for i in 0..arg_count {
                merged.push(self.lub_step(
                    self.table().arg(s1.offset() + i),
                    self.table().arg(s2.offset() + i),
                )?);
            }
            return Ok(self.table().intern(
                UninternedType::Interface {
                    decl: d1,
                    args: merged,
                },
                false,
            ));
        }
        // Spec-defined fallback: supertype-set intersection, picking the
        // candidate with the longest unique inheritance path
        // (least_upper_bound.dart:103, :162-177, :327-357). The closure sets
        // and their frontier run on the reusable scratch; a re-entered query
        // falls back to fresh local containers (this branch never recurses
        // into itself, so the fallback is only a guard).
        if let Ok(mut scratch) = self.walk.try_borrow_mut() {
            let WalkScratch {
                set_a,
                set_b,
                candidates,
                closure_frontier,
                ..
            } = &mut *scratch;
            set_a.clear();
            set_b.clear();
            // Claim the pooled frontier (exclusive while taken) and return it
            // after the walks, so the steady-state path reuses one buffer.
            let mut frontier = std::mem::take(closure_frontier);
            self.supertype_closure(a, &mut frontier, set_a);
            self.supertype_closure(b, &mut frontier, set_b);
            *closure_frontier = frontier;
            candidates.clear();
            candidates.extend(set_a.intersection(set_b).copied());
            return Ok(self.pick_max_unique_depth(candidates));
        }
        let mut si = FxHashSet::default();
        let mut sj = FxHashSet::default();
        let mut frontier = Vec::new();
        self.supertype_closure(a, &mut frontier, &mut si);
        self.supertype_closure(b, &mut frontier, &mut sj);
        let candidates: Vec<TypeId> = si.intersection(&sj).copied().collect();
        Ok(self.pick_max_unique_depth(&candidates))
    }

    /// The supertype set including the type itself
    /// (least_upper_bound.dart:167-172) — the oracle's plain uncapped
    /// recursion (least_upper_bound.dart:162-177) rendered as a
    /// frontier+visited worklist, so deep-but-valid hierarchies cannot
    /// overflow the thread stack (the subtype face,
    /// `TypeSystem::interface_walk`, is iterative for the same reason).
    /// Canonical interning makes `TypeId` a sound set key across
    /// differently substituted paths, and the answer only reads the set,
    /// never the visit order.
    ///
    /// Every visited node instantiates its supertype templates against its
    /// own already-concrete arguments — the same per-edge
    /// `Substitution.fromInterfaceType` step the interface walk runs
    /// (subtype.dart:213-214) — so the worklist carries plain `TypeId`s and
    /// no substitution stack is needed.
    ///
    /// `frontier` is caller-owned scratch — the pooled
    /// `WalkScratch::closure_frontier` on the steady-state path — cleared
    /// here and reused across both closure walks of an interface LUB, so the
    /// walk itself allocates nothing.
    fn supertype_closure(
        &self,
        root: TypeId,
        frontier: &mut Vec<TypeId>,
        out: &mut FxHashSet<TypeId>,
    ) {
        frontier.clear();
        frontier.push(root);
        while let Some(t) = frontier.pop() {
            if !out.insert(t) {
                continue;
            }
            let TypeKind::Interface { decl, args } = self.table().get(t).kind() else {
                continue;
            };
            let params = &self
                .hierarchy()
                .decl(decl)
                .expect("declared")
                .type_parameters;
            for &edge in self.hierarchy().supertypes(decl) {
                // `instantiate` zips params with the argument range and
                // leaves a malformed short-arity tail unbound, exactly like
                // the substitution the recursive form pushed per node.
                let instantiated = self.table().instantiate(edge, params, args);
                frontier.push(instantiated);
            }
        }
    }

    /// `_computeTypeAtMaxUniqueDepth` (least_upper_bound.dart:327-357).
    /// Termination is guaranteed: a validated snapshot's intersection always
    /// contains `Object` (implicit inheritance, depth 1), so the walk finds a
    /// unique candidate before the depth runs out; a malformed snapshot
    /// saturates at 0 and falls back to `Object` instead of looping.
    fn pick_max_unique_depth(&self, candidates: &[TypeId]) -> TypeId {
        let mut max = candidates
            .iter()
            .map(|&t| self.depth_of(t))
            .max()
            .unwrap_or(0);
        loop {
            let mut unique = None;
            let mut count = 0;
            for &t in candidates {
                if self.depth_of(t) == max {
                    count += 1;
                    unique = Some(t);
                }
            }
            if count == 1 {
                return unique.expect("exactly one candidate at this depth");
            }
            let next = max.saturating_sub(1);
            if next == max {
                // Saturated at 0 with no unique candidate — unreachable for
                // validated snapshots, so the input is malformed: return the
                // safe upper bound instead of looping.
                return self.core_object_none();
            }
            max = next;
        }
    }

    /// Longest inheritance path to `Object`
    /// (`computeLongestInheritancePathToObject`, least_upper_bound.dart:204-323,
    /// over all direct supertype edges). Precomputed per declaration when the
    /// hierarchy snapshot was built; the graph was validated acyclic at build
    /// time.
    fn depth_of(&self, t: TypeId) -> u32 {
        match self.table().get(t).kind() {
            TypeKind::Interface { decl, .. } => self.hierarchy().depth(decl),
            _ => 1,
        }
    }
}
