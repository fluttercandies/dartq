//! Least upper bound (spec §4.4: interface superclass chain + nullability
//! merge; `FutureOr` merging included, GLB/closure merging deferred to M2).
//!
//! Port of `LeastUpperBoundHelper._computeLeastUpperBound`
//! (`least_upper_bound.dart:391-672`) with the documented M1 fallbacks:
//! function/function yields bare `Function` (the official fallback when
//! correlation fails, least_upper_bound.dart:684-686) and records return the
//! declared `Record` class (`least_upper_bound.dart:637-644`) or, when no
//! `Record` class is in the snapshot and for record/record pairs, the
//! official record-vs-other fallback `UP(Object, T2)`
//! (least_upper_bound.dart:652-659).

use rustc_hash::FxHashSet;

use crate::ids::TypeId;
use crate::system::{TypeSystem, WalkScratch};
use crate::ty::{Type, TypeKind, UninternedType};

impl TypeSystem<'_> {
    /// The least upper bound of two types; the static type of `c ? a : b`
    /// and the result type of `a ?? b` (before the nullability dance the
    /// checker applies).
    #[must_use]
    pub fn least_upper_bound(&self, a: TypeId, b: TypeId) -> TypeId {
        // UP(T, T) = T (least_upper_bound.dart:393-395).
        if a == b {
            return a;
        }
        let t1 = self.table().get(a);
        let t2 = self.table().get(b);

        // TOP (least_upper_bound.dart:405-427).
        let top1 = self.is_top(a);
        let top2 = self.is_top(b);
        if top1 && top2 {
            return if self.is_more_top(a, b) { a } else { b };
        }
        if top1 {
            return a;
        }
        if top2 {
            return b;
        }

        // BOTTOM (least_upper_bound.dart:429-451). `Never` is the only bottom
        // type in M1 (`Never?` is interned as `Null`).
        if self.is_bottom(a) {
            return b;
        }
        if self.is_bottom(b) {
            return a;
        }

        // NULL (least_upper_bound.dart:487-524).
        if self.is_null(a) {
            return self.null_case(b);
        }
        if self.is_null(b) {
            return self.null_case(a);
        }

        // OBJECT (least_upper_bound.dart:526-560).
        if self.is_object(a) {
            return if self.is_non_nullable(b) {
                a
            } else {
                self.make_nullable(a)
            };
        }
        if self.is_object(b) {
            return if self.is_non_nullable(a) {
                b
            } else {
                self.make_nullable(b)
            };
        }

        // UP(T1?, T2) = UP(T1, T2)? (least_upper_bound.dart:562-571).
        if t1.is_nullable() || t2.is_nullable() {
            let s = self.least_upper_bound(self.strip(a), self.strip(b));
            return self.make_nullable(s);
        }

        // Type parameters (least_upper_bound.dart:576-609). M1 has no
        // declared bounds; the official default bound is `Object?`
        // (least_upper_bound.dart:893-899), which makes the fallback
        // `Object?` exactly.
        if matches!(t1.kind(), TypeKind::TypeParameter { .. })
            || matches!(t2.kind(), TypeKind::TypeParameter { .. })
        {
            if self.is_subtype(a, b) {
                return b;
            }
            if self.is_subtype(b, a) {
                return a;
            }
            return self.object_question();
        }

        // FUNCTION (least_upper_bound.dart:611-635).
        if matches!(t1.kind(), TypeKind::Function { .. }) {
            if self.is_bare_function_id(b) {
                return b;
            }
            if matches!(t2.kind(), TypeKind::Function { .. }) {
                // M1: no parameter-wise GLB merge — the coarse upper bound
                // `Function` (the official correlation-failure fallback).
                return self.intern_bare_function();
            }
            return self.up_object_with(b);
        }
        if matches!(t2.kind(), TypeKind::Function { .. }) {
            if self.is_bare_function_id(a) {
                return a;
            }
            return self.up_object_with(a);
        }

        // RECORD (least_upper_bound.dart:637-660). `UP((...), Record) =
        // Record` and `UP(Record, (...)) = Record`
        // (least_upper_bound.dart:637-644); record/record merging
        // (`_recordType`, least_upper_bound.dart:853-885) stays M2, so
        // everything else uses the official record-vs-other fallback
        // `UP(Object, T2)` (least_upper_bound.dart:652-659).
        if matches!(t1.kind(), TypeKind::Record { .. })
            || matches!(t2.kind(), TypeKind::Record { .. })
        {
            if let Some(record) = self.hierarchy().core().record {
                // Rule order matches the source: `T2` as the `Record` class
                // wins before `T1`.
                let is_record_class = |t: Type| matches!(t.kind(), TypeKind::Interface { decl, .. } if decl == record);
                if is_record_class(t2) {
                    return b;
                }
                if is_record_class(t1) {
                    return a;
                }
            }
            return if matches!(t1.kind(), TypeKind::Record { .. }) {
                self.up_object_with(b)
            } else {
                self.up_object_with(a)
            };
        }

        // FUTUREOR (least_upper_bound.dart:662, :796-844).
        if let Some(result) = self.future_or_lub(a, b) {
            return result;
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
    fn future_or_lub(&self, a: TypeId, b: TypeId) -> Option<TypeId> {
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
            return Some(self.intern_future_or(self.least_upper_bound(x, y)));
        }
        // UP(Future<T1>, FutureOr<T2>) = FutureOr<UP(T1, T2)>
        if let (Some(x), Some(y)) = (future(a), future_or(b)) {
            return Some(self.intern_future_or(self.least_upper_bound(x, y)));
        }
        // UP(FutureOr<T1>, Future<T2>) = FutureOr<UP(T1, T2)>
        if let (Some(x), Some(y)) = (future_or(a), future(b)) {
            return Some(self.intern_future_or(self.least_upper_bound(x, y)));
        }
        // UP(T1, FutureOr<T2>) = FutureOr<UP(T1, T2)>
        if let Some(y) = future_or(b) {
            return Some(self.intern_future_or(self.least_upper_bound(a, y)));
        }
        // UP(FutureOr<T1>, T2) = FutureOr<UP(T1, T2)>
        if let Some(x) = future_or(a) {
            return Some(self.intern_future_or(self.least_upper_bound(x, b)));
        }
        None
    }

    /// `InterfaceLeastUpperBoundHelper.compute`
    /// (least_upper_bound.dart:48-108) with nullability already merged.
    fn interface_lub(&self, a: TypeId, b: TypeId) -> TypeId {
        // Either being a supertype of the other is the best result
        // (least_upper_bound.dart:55-60).
        if self.is_subtype(a, b) {
            return b;
        }
        if self.is_subtype(b, a) {
            return a;
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
                merged.push(self.least_upper_bound(
                    self.table().arg(s1.offset() + i),
                    self.table().arg(s2.offset() + i),
                ));
            }
            return self.table().intern(
                UninternedType::Interface {
                    decl: d1,
                    args: merged,
                },
                false,
            );
        }
        // Spec-defined fallback: supertype-set intersection, picking the
        // candidate with the longest unique inheritance path
        // (least_upper_bound.dart:103, :162-177, :327-357). The closure sets
        // run on the reusable scratch; a re-entered query falls back to
        // fresh local sets (this branch never recurses into itself, so the
        // fallback is only a guard).
        if let Ok(mut scratch) = self.walk.try_borrow_mut() {
            let WalkScratch {
                set_a,
                set_b,
                candidates,
                ..
            } = &mut *scratch;
            set_a.clear();
            set_b.clear();
            self.supertype_closure(a, set_a);
            self.supertype_closure(b, set_b);
            candidates.clear();
            candidates.extend(set_a.intersection(set_b).copied());
            self.pick_max_unique_depth(candidates)
        } else {
            let mut si = FxHashSet::default();
            let mut sj = FxHashSet::default();
            self.supertype_closure(a, &mut si);
            self.supertype_closure(b, &mut sj);
            let candidates: Vec<TypeId> = si.intersection(&sj).copied().collect();
            self.pick_max_unique_depth(&candidates)
        }
    }

    /// The supertype set including the type itself
    /// (least_upper_bound.dart:167-172). Canonical interning makes `TypeId` a
    /// sound set key across differently substituted paths.
    fn supertype_closure(&self, t: TypeId, out: &mut FxHashSet<TypeId>) {
        if !out.insert(t) {
            return;
        }
        let TypeKind::Interface { decl, args } = self.table().get(t).kind() else {
            return;
        };
        let params = &self
            .hierarchy()
            .decl(decl)
            .expect("declared")
            .type_parameters;
        if params.is_empty() {
            for &edge in self.hierarchy().supertypes(decl) {
                self.supertype_closure(edge, out);
            }
        } else {
            let map: Vec<_> = params
                .iter()
                .copied()
                .zip(self.table().args(args))
                .collect();
            for &edge in self.hierarchy().supertypes(decl) {
                self.supertype_closure(self.table().substitute(edge, &map), out);
            }
        }
    }

    /// `_computeTypeAtMaxUniqueDepth` (least_upper_bound.dart:327-357).
    /// Termination is guaranteed: the intersection always contains `Object`
    /// (implicit inheritance, depth 1), and every other declaration has
    /// strictly larger depth.
    fn pick_max_unique_depth(&self, candidates: &[TypeId]) -> TypeId {
        let mut max = candidates
            .iter()
            .map(|&t| self.depth_of(t))
            .max()
            .expect("intersection is non-empty: both sides include `Object`");
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
            max -= 1;
        }
    }

    /// Longest inheritance path to `Object`
    /// (`computeLongestInheritancePathToObject`, least_upper_bound.dart:204-323,
    /// class edges only). Precomputed per declaration when the hierarchy
    /// snapshot was built; the graph was validated acyclic at build time.
    fn depth_of(&self, t: TypeId) -> u32 {
        match self.table().get(t).kind() {
            TypeKind::Interface { decl, .. } => self.hierarchy().depth(decl),
            _ => 1,
        }
    }
}
