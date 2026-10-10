//! The flow state machine: reachability, per-variable promotion models,
//! joins, conservative joins, and the `try`/`finally` composition.
//!
//! A faithful port of the official `Reachability`, `PromotionModel`, and
//! `FlowModel` classes (`flow_analysis.dart:3751`, `:4655`, `:5538`), with
//! one layout change: per-key promotion models live in a dense `Vec`
//! indexed by variable id (the official uses persistent linked maps for
//! cheap structural diffs; the dense form copies small copyable models at
//! branch points, which keeps the same decision order at a fraction of the
//! bookkeeping). Promoted-type and tested-type chains are persistent
//! cons-cells in a shared pool, so chain sharing across states is free.

use rustc_hash::FxHashSet;

use crate::ty::{Classification, Ty, TypeOps};

/// Absent list / null reference marker.
pub(crate) const NIL: u32 = u32::MAX;

/// A node of the reachability tree (official `Reachability`): a checkpoint
/// chain mirroring the control-flow structure, with local (from the most
/// recent checkpoint) and overall (from function entry) reachability.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ReachNode {
    parent: u32,
    /// Locally reachable from the parent checkpoint.
    locally: bool,
    /// Reachable from the beginning of the function.
    overall: bool,
    /// Parent-link distance to the initial node (common-ancestor walks).
    depth: u32,
}

/// Arena of reachability nodes; node 0 is the function-entry state.
#[derive(Default)]
pub(crate) struct ReachArena {
    nodes: Vec<ReachNode>,
}

impl ReachArena {
    pub(crate) fn new() -> Self {
        Self {
            nodes: vec![ReachNode {
                parent: NIL,
                locally: true,
                overall: true,
                depth: 0,
            }],
        }
    }

    pub(crate) fn locally_reachable(&self, r: u32) -> bool {
        self.nodes[r as usize].locally
    }

    pub(crate) fn parent(&self, r: u32) -> u32 {
        self.nodes[r as usize].parent
    }

    /// A new checkpoint under `r`, reachable (official `split`).
    pub(crate) fn split(&mut self, r: u32) -> u32 {
        let node = self.nodes[r as usize];
        self.nodes.push(ReachNode {
            parent: r,
            locally: true,
            overall: node.overall,
            depth: node.depth + 1,
        });
        self.nodes.len() as u32 - 1
    }

    /// The same checkpoint, locally unreachable (official `setUnreachable`).
    pub(crate) fn set_unreachable(&mut self, r: u32) -> u32 {
        if !self.nodes[r as usize].locally {
            return r;
        }
        let node = self.nodes[r as usize];
        self.nodes.push(ReachNode {
            parent: node.parent,
            locally: false,
            overall: false,
            depth: node.depth,
        });
        self.nodes.len() as u32 - 1
    }

    /// Drops the most recent checkpoint (official `unsplit`). The root has
    /// no checkpoint to drop: the state stays as it is.
    pub(crate) fn unsplit(&mut self, r: u32) -> u32 {
        let node = self.nodes[r as usize];
        if node.parent == NIL {
            return r;
        }
        let parent = node.parent;
        if node.locally {
            parent
        } else {
            self.set_unreachable(parent)
        }
    }

    /// Unsplit repeatedly until the parent is `target` (official
    /// `unsplitTo`, `flow_analysis.dart:4202`).
    pub(crate) fn unsplit_to(&mut self, mut r: u32, target: u32) -> u32 {
        while r != target
            && self.nodes[r as usize].parent != target
            && self.nodes[r as usize].parent != NIL
        {
            r = self.unsplit(r);
        }
        r
    }
}

/// A cons-cell pool for promoted-type and tested-type chains. Chains are
/// ordered most- to least-specific (the mirror of the official least- to
/// most-specific list order — `add_promoted` conses onto the front), shared
/// freely across states, and never mutated in place.
#[derive(Default)]
pub(crate) struct Lists {
    cells: Vec<(Ty, u32)>,
}

impl Lists {
    pub(crate) fn cons(&mut self, ty: Ty, next: u32) -> u32 {
        self.cells.push((ty, next));
        self.cells.len() as u32 - 1
    }

    pub(crate) fn head(&self, id: u32) -> Ty {
        self.cells[id as usize].0
    }

    /// The chain as a slice walk (front to back).
    pub(crate) fn iter(&self, id: u32) -> impl Iterator<Item = Ty> + '_ {
        let mut cur = id;
        std::iter::from_fn(move || {
            if cur == NIL {
                return None;
            }
            let (ty, next) = self.cells[cur as usize];
            cur = next;
            Some(ty)
        })
    }
}

/// Per-variable promotion/assignment model (official `PromotionModel`).
/// `version == CAPTURED` encodes the official `version == null`
/// (write-captured) state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct VarModel {
    /// Promotion chain (least- to most-specific promoted type).
    pub promoted: u32,
    /// Types tested against this variable (types of interest).
    pub tested: u32,
    /// Write version; [`CAPTURED`] once write-captured.
    pub version: u32,
    /// Definitely assigned.
    pub assigned: bool,
    /// Definitely unassigned (only meaningful pre-first-write).
    pub unassigned: bool,
}

/// The write-version sentinel for write-captured variables.
pub(crate) const CAPTURED: u32 = u32::MAX;

impl VarModel {
    /// A variable never seen before (official `PromotionModel.fresh`).
    pub(crate) const fn fresh() -> Self {
        Self {
            promoted: NIL,
            tested: NIL,
            version: 0,
            assigned: false,
            unassigned: true,
        }
    }

    pub(crate) fn captured(&self) -> bool {
        self.version == CAPTURED
    }
}

/// One point in the flow: a reachability node plus the per-variable models
/// (official `FlowModel`).
#[derive(Clone, Debug)]
pub(crate) struct State {
    pub reach: u32,
    pub models: Vec<VarModel>,
}

impl State {
    pub(crate) fn new(initial_reach: u32) -> Self {
        Self {
            reach: initial_reach,
            models: Vec::new(),
        }
    }

    pub(crate) fn model(&self, var: u32) -> VarModel {
        self.models
            .get(var as usize)
            .copied()
            .unwrap_or_else(VarModel::fresh)
    }

    fn with_model(mut self, var: u32, model: VarModel) -> Self {
        let index = var as usize;
        if index >= self.models.len() {
            self.models.resize(index + 1, VarModel::fresh());
        }
        self.models[index] = model;
        self
    }

    /// Splits control flow (official `FlowModel.split`).
    pub(crate) fn split(self, reach: &mut ReachArena) -> Self {
        let r = reach.split(self.reach);
        Self {
            reach: r,
            models: self.models,
        }
    }

    /// Marks the current path unreachable (official `setUnreachable`).
    pub(crate) fn set_unreachable(self, reach: &mut ReachArena) -> Self {
        let r = reach.set_unreachable(self.reach);
        Self {
            reach: r,
            models: self.models,
        }
    }

    /// Drops the innermost checkpoint (official `unsplit`).
    /// Unsplit repeatedly until the parent is `target` (official
    /// `unsplitTo`): merges the split chain down to the checkpoint.
    pub(crate) fn unsplit_to(self, reach: &mut ReachArena, target: u32) -> Self {
        Self {
            reach: reach.unsplit_to(self.reach, target),
            models: self.models,
        }
    }

    pub(crate) fn unsplit(self, reach: &mut ReachArena) -> Self {
        let r = reach.unsplit(self.reach);
        Self {
            reach: r,
            models: self.models,
        }
    }

    /// Declares a variable (official `FlowModel.declare`): fresh model,
    /// assigned iff `initialized`.
    pub(crate) fn declare(self, var: u32, initialized: bool) -> Self {
        let model = VarModel {
            assigned: initialized,
            unassigned: !initialized,
            ..VarModel::fresh()
        };
        self.with_model(var, model)
    }

    /// Records a write (official `PromotionModel.write` + `_demoteViaAssignment`
    /// + `_tryPromoteToTypeOfInterest`): marks assigned, drops promotions the
    /// written type no longer satisfies, and re-promotes to a type of
    /// interest (the declared type's non-null form or a previously tested
    /// type) when the written value matches.
    pub(crate) fn write(
        self,
        ops: &dyn TypeOps,
        lists: &mut Lists,
        var: u32,
        declared: Ty,
        written: Ty,
        promote_to_interest: bool,
    ) -> Self {
        let model = self.model(var);
        if model.captured() {
            // Write-captured: stays captured, assigned, unpromoted
            // (official `PromotionModel.write` capture arm).
            return self.with_model(
                var,
                VarModel {
                    assigned: true,
                    unassigned: false,
                    ..model
                },
            );
        }
        // Demote: drop the most-specific entries the written type no longer
        // satisfies and keep the least-specific remainder (official
        // `_demoteViaAssignment`, flow_analysis.dart:4888-4928: the official
        // list is least- to most-specific and keeps `sublist(0, k)` for the
        // largest k with `written <: promotedTypes[k-1]`; dartq chains store
        // the mirror order — most-specific first — so the kept prefix of the
        // official list is a suffix here). Rebuild the kept suffix in order.
        let nodes: Vec<Ty> = lists.iter(model.promoted).collect();
        let mut promoted = NIL;
        if let Some(keep_from) = nodes.iter().position(|&ty| ops.is_subtype(written, ty)) {
            for ty in nodes[keep_from..].iter().rev() {
                promoted = lists.cons(*ty, promoted);
            }
        }
        if promote_to_interest && !promoted_is(lists, promoted, written) {
            // Types of interest: the declared type's non-null form, then the
            // tested types and their non-null forms (official
            // `_tryPromoteToTypeOfInterest`). The written type must be a
            // proper subtype of the currently promoted type.
            let current = if promoted == NIL {
                declared
            } else {
                lists.head(promoted)
            };
            let mut candidates: Vec<Ty> = Vec::new();
            let declared_non_null = ops.promote_to_non_null(declared);
            if declared_non_null != declared {
                consider(
                    ops,
                    lists,
                    written,
                    current,
                    declared_non_null,
                    &mut candidates,
                );
            }
            for tested in lists.iter(model.tested) {
                consider(ops, lists, written, current, tested, &mut candidates);
                let tested_non_null = ops.promote_to_non_null(tested);
                if tested_non_null != tested {
                    consider(
                        ops,
                        lists,
                        written,
                        current,
                        tested_non_null,
                        &mut candidates,
                    );
                }
            }
            // The unique candidate that is a subtype of all the others wins.
            if let Some(promoted_to) = unique_subtype(ops, &candidates) {
                promoted = add_promoted(ops, lists, promoted, promoted_to);
            }
        }
        self.with_model(
            var,
            VarModel {
                promoted,
                tested: model.tested,
                version: model.version.wrapping_add(1).max(1),
                assigned: true,
                unassigned: false,
            },
        )
    }

    /// Records a promotion to `promoted` after a successful type test of
    /// `tested` (official `_finishTypeTest`).
    pub(crate) fn finish_type_test(
        self,
        ops: &dyn TypeOps,
        lists: &mut Lists,
        var: u32,
        tested: Option<Ty>,
        promoted: Option<Ty>,
    ) -> Self {
        let model = self.model(var);
        let mut new_tested = model.tested;
        if let Some(tested) = tested {
            if !lists.iter(model.tested).any(|t| t == tested) {
                new_tested = append_tested(lists, model.tested, tested);
            }
        }
        let mut new_promoted = model.promoted;
        if let Some(promoted) = promoted {
            new_promoted = add_promoted(ops, lists, model.promoted, promoted);
        }
        self.with_model(
            var,
            VarModel {
                promoted: new_promoted,
                tested: new_tested,
                ..model
            },
        )
    }

    /// Joins two states (official `FlowModel.join` + `joinPromotionInfo`):
    /// reachable if either is; assigned if either is; unassigned only if
    /// both are; promotions keep the common (less specific) chain prefix.
    pub(crate) fn join(
        first: Option<Self>,
        second: Option<Self>,
        reach: &mut ReachArena,
        lists: &mut Lists,
        vars: usize,
    ) -> Self {
        let Some(first) = first else {
            return second.expect("join of two absent states");
        };
        let Some(second) = second else {
            return first;
        };
        if !reach.locally_reachable(second.reach) {
            return first;
        }
        if !reach.locally_reachable(first.reach) {
            return second;
        }
        let mut models = Vec::with_capacity(vars);
        for var in 0..vars {
            let a = first.model(var as u32);
            let b = second.model(var as u32);
            models.push(join_model(a, b, lists));
        }
        Self {
            reach: first.reach,
            models,
        }
    }

    /// Conservative join at loop heads and closure entries (official
    /// `FlowModel.conservativeJoin`): written variables lose promotions
    /// (finals keep them) and definite-unassignedness; captured variables
    /// become write-captured and lose all promotions.
    pub(crate) fn conservative_join(
        self,
        written: &FxHashSet<u32>,
        captured: &FxHashSet<u32>,
        finals: &FxHashSet<u32>,
    ) -> Self {
        let mut this = self;
        for &var in written {
            let model = this.model(var);
            if model.assigned && model.promoted == NIL && !model.unassigned {
                continue;
            }
            let new = if finals.contains(&var) {
                // Final variables keep promotions (their value cannot
                // change) but are no longer definitely unassigned.
                VarModel {
                    unassigned: false,
                    ..model
                }
            } else {
                VarModel {
                    promoted: NIL,
                    unassigned: false,
                    ..model
                }
            };
            this = this.with_model(var, new);
        }
        for &var in captured {
            let model = this.model(var);
            if model.captured() {
                continue;
            }
            this = this.with_model(
                var,
                VarModel {
                    promoted: NIL,
                    tested: NIL,
                    version: CAPTURED,
                    assigned: model.assigned,
                    unassigned: false,
                },
            );
        }
        this
    }

    /// Extends the `tested` sets with the other state's (official
    /// `inheritTested`): after a loop, types tested inside the body are
    /// types of interest for writes after the loop.
    pub(crate) fn inherit_tested(self, other: &Self, lists: &mut Lists) -> Self {
        let mut this = self;
        for var in 0..this.models.len().max(other.models.len()) {
            let a = this.model(var as u32);
            let b = other.model(var as u32);
            if a.tested == b.tested {
                continue;
            }
            let mut merged = a.tested;
            let additions: Vec<Ty> = lists.iter(b.tested).collect();
            for ty in additions {
                let present = lists.iter(merged).any(|t| t == ty);
                if !present {
                    merged = append_tested(lists, merged, ty);
                }
            }
            let var = var as u32;
            this = this.with_model(
                var,
                VarModel {
                    tested: merged,
                    ..a
                },
            );
        }
        this
    }
}

/// Joins two per-variable models (official `PromotionModel.join`).
fn join_model(a: VarModel, b: VarModel, lists: &mut Lists) -> VarModel {
    if a == b {
        return a;
    }
    let captured = a.captured() || b.captured();
    // Written on one path since the branch or not, the conservative join is
    // the common (less specific) chain prefix either way; write-captured on
    // either path keeps no promotion at all (official `joinPromotionInfo`).
    let promoted = if captured { NIL } else { join_promoted(a.promoted, b.promoted, lists) };
    let tested = union_tested(a.tested, b.tested, lists);
    VarModel {
        promoted,
        tested,
        version: if captured {
            CAPTURED
        } else {
            a.version.max(b.version)
        },
        assigned: a.assigned && b.assigned,
        unassigned: a.unassigned && b.unassigned,
    }
}

/// Order-preserving intersection of two promotion chains (official
/// `joinPromotedTypes`, `flow_analysis.dart:5154`): the greatest common
/// subsequence — every type of `a` that also occurs in `b`
/// (`chain1.where(chain2.contains)`), so a mid-chain fork keeps the deeper
/// common elements instead of stopping at the first mismatch. Chains are
/// stored most-specific first and the result preserves `a`'s order. The
/// official O(m+n) merge exploits subtype ordering across a real types
/// backend; until one lands, equality is the intersection sense, and the
/// chains are tiny enough that the scan is trivially cheap.
fn join_promoted(a: u32, b: u32, lists: &mut Lists) -> u32 {
    if a == b {
        return a;
    }
    let mut kept: Vec<Ty> = Vec::new();
    let mut total = 0usize;
    for ty in lists.iter(a) {
        total += 1;
        if lists.iter(b).any(|other| other == ty) {
            kept.push(ty);
        }
    }
    if kept.len() == total {
        return a;
    }
    let mut rebuilt = NIL;
    for ty in kept.into_iter().rev() {
        rebuilt = lists.cons(ty, rebuilt);
    }
    rebuilt
}

fn union_tested(a: u32, b: u32, lists: &mut Lists) -> u32 {
    if a == b {
        return a;
    }
    let additions: Vec<Ty> = lists.iter(b).collect();
    let mut merged = a;
    for ty in additions {
        let present = lists.iter(merged).any(|t| t == ty);
        if !present {
            merged = append_tested(lists, merged, ty);
        }
    }
    merged
}

fn append_tested(lists: &mut Lists, chain: u32, ty: Ty) -> u32 {
    // Append at the end to keep the official order (order is cosmetic; the
    // membership is what drives types-of-interest). Rebuild by consing onto
    // a reversed copy.
    let mut items: Vec<Ty> = lists.iter(chain).collect();
    items.push(ty);
    let mut rebuilt = NIL;
    for ty in items.into_iter().rev() {
        rebuilt = lists.cons(ty, rebuilt);
    }
    rebuilt
}

fn add_promoted(ops: &dyn TypeOps, lists: &mut Lists, chain: u32, ty: Ty) -> u32 {
    // The official invariant: each promoted type is a strict subtype of the
    // previous; a repeated or mutual promotion is rejected by
    // `isValidPromotionStep` before this call.
    if promoted_is(lists, chain, ty) {
        return chain;
    }
    if chain != NIL && ops.is_subtype(lists.head(chain), ty) {
        // New type is a supertype of the current head — not a refinement.
        return chain;
    }
    lists.cons(ty, chain)
}

fn promoted_is(lists: &Lists, chain: u32, ty: Ty) -> bool {
    lists.iter(chain).any(|t| t == ty)
}

fn consider(
    ops: &dyn TypeOps,
    lists: &Lists,
    written: Ty,
    current: Ty,
    candidate: Ty,
    out: &mut Vec<Ty>,
) {
    if ops.is_subtype(written, candidate)
        && ops.is_subtype(candidate, current)
        && candidate != current
        && !ops.is_subtype(current, candidate)
        && !out.contains(&candidate)
    {
        let _ = lists;
        out.push(candidate);
    }
}

fn unique_subtype(ops: &dyn TypeOps, candidates: &[Ty]) -> Option<Ty> {
    let mut found: Option<Ty> = None;
    for &candidate in candidates {
        if candidates
            .iter()
            .all(|&other| other == candidate || ops.is_subtype(candidate, other))
        {
            if found.is_some() {
                return None; // not unique
            }
            found = Some(candidate);
        }
    }
    found
}

/// The per-variable half of `attachFinally` (official `_attachFinallyPM`):
/// if the variable's value might have changed inside the finally block, its
/// promotions are the finally-exit promotions; otherwise the try-exit
/// promotions survive. Returns the model to install.
pub(crate) fn attach_finally_model(
    lists: &Lists,
    after_try: VarModel,
    before_finally: Option<VarModel>,
    after_finally: VarModel,
) -> VarModel {
    let might_have_changed = after_finally.captured()
        || before_finally.is_none()
        || before_finally.is_some_and(|before| before.version != after_finally.version);
    if might_have_changed {
        after_finally
    } else {
        // Keep the try-exit promotions and assignment facts, but the value
        // version advances to the finally-exit version (writes in the
        // finally block did not touch it — versions equal — so keep it).
        let _ = lists;
        VarModel {
            promoted: after_try.promoted,
            tested: after_try.tested,
            version: after_finally.version,
            assigned: after_try.assigned || after_finally.assigned,
            unassigned: after_try.unassigned && after_finally.unassigned,
        }
    }
}

pub(crate) fn is_non_nullable(ops: &dyn TypeOps, t: Ty) -> bool {
    ops.classify(t) == Classification::NonNullable
}
