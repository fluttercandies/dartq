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

use crate::ty::{Base, Classification, Ty, TypeOps};

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

    /// Whether any control path reaches `r` (official `overallReachable` —
    /// the flag `isReachable` reads, `flow_analysis.dart:6396`): a split
    /// resurrects the local flag (official `Reachability.split` hardcodes
    /// `locallyReachable: true`) but never the overall one, so a case body
    /// opened after a resurrecting split still queries dead for the
    /// dead-code and completion faces.
    pub(crate) fn overall_reachable(&self, r: u32) -> bool {
        self.nodes[r as usize].overall
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
        u32::try_from(self.nodes.len()).expect("checkpoint count fits u32") - 1
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
        u32::try_from(self.nodes.len()).expect("checkpoint count fits u32") - 1
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
/// freely across states, and never mutated in place. Each cell carries its
/// type's written-argument tree beside the name-level [`Ty`] (#1010): the
/// promotion rows read the chain head's arguments, so the rows travel with
/// the chain through promotions, demotions, and joins exactly like the
/// types do.
#[derive(Default)]
pub(crate) struct Lists {
    cells: Vec<(Ty, u32, u32)>,
}

/// A cons-cell pool for non-promotion history chains (upstream
/// `NonPromotionHistory`, `flow_analysis.dart:4529-4549`): one cell per
/// dropped promoted type, newest demotion first, shared freely across
/// states like the type chains. Cells carry `(ty, args, cause, next)` —
/// compact copyable data only (no heap, no strings on the hot path).
/// Property slots store their cleared promoted types here too, tagged
/// [`crate::facts::NonPromotionCause::PropertyEvidence`] — the flat-slot
/// form of upstream's `_nonPromotableProperties` version chain
/// (`flow_analysis.dart:5684, 5707-5719`); the `PropertyNotPromoted`
/// verdict itself is decided by the consumer's element layer.
#[derive(Default)]
pub(crate) struct Reasons {
    cells: Vec<(Ty, u32, crate::facts::NonPromotionCause, u32)>,
}

impl Reasons {
    pub(crate) fn cons(
        &mut self,
        ty: Ty,
        args: u32,
        cause: crate::facts::NonPromotionCause,
        next: u32,
    ) -> u32 {
        self.cells.push((ty, args, cause, next));
        u32::try_from(self.cells.len()).expect("reason cell count fits u32") - 1
    }

    /// The chain as a slice walk (front to back — most recent demotion
    /// first, the official linked-list order `flow_analysis.dart:8844-8850`).
    pub(crate) fn iter(
        &self,
        id: u32,
    ) -> impl Iterator<Item = (Ty, u32, crate::facts::NonPromotionCause)> + '_ {
        let mut cur = id;
        std::iter::from_fn(move || {
            if cur == NIL {
                return None;
            }
            let (ty, args, cause, next) = self.cells[cur as usize];
            cur = next;
            Some((ty, args, cause))
        })
    }
}

impl Lists {
    pub(crate) fn cons(&mut self, ty: Ty, args: u32, next: u32) -> u32 {
        self.cells.push((ty, args, next));
        u32::try_from(self.cells.len()).expect("tested-set cell count fits u32") - 1
    }

    pub(crate) fn head(&self, id: u32) -> Ty {
        self.cells[id as usize].0
    }

    /// The chain head's written-argument tree (arena index) — the
    /// [`Ty`]-side pairing of [`Lists::head`] (#1010).
    pub(crate) fn head_args(&self, id: u32) -> u32 {
        self.cells[id as usize].1
    }

    /// The chain as a slice walk (front to back).
    pub(crate) fn iter(&self, id: u32) -> impl Iterator<Item = Ty> + '_ {
        let mut cur = id;
        std::iter::from_fn(move || {
            if cur == NIL {
                return None;
            }
            let (ty, _, next) = self.cells[cur as usize];
            cur = next;
            Some(ty)
        })
    }

    /// The chain as a slice walk carrying each entry's argument tree (the
    /// rebuild walks — demotion, joins, tested merges — cons the entries
    /// back with their rows).
    pub(crate) fn iter_entries(&self, id: u32) -> impl Iterator<Item = (Ty, u32)> + '_ {
        let mut cur = id;
        std::iter::from_fn(move || {
            if cur == NIL {
                return None;
            }
            let (ty, args, next) = self.cells[cur as usize];
            cur = next;
            Some((ty, args))
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
    /// Non-promotion history chain into the interpreter's [`Reasons`]
    /// pool (official `nonPromotionHistory`, `flow_analysis.dart:4686`):
    /// one entry per dropped promoted type, most recent first. For
    /// property slots the same field holds the cleared promoted types as
    /// [`crate::facts::NonPromotionCause::PropertyEvidence`] — the flat-slot form of upstream's
    /// `_nonPromotableProperties` version chain (`flow_analysis.dart:5684`,
    /// `5711-5719`).
    pub history: u32,
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
            history: NIL,
        }
    }

    pub(crate) fn captured(&self) -> bool {
        self.version == CAPTURED
    }

    /// Field equality on the official `PromotionModel` payload only —
    /// upstream's `_identicalOrNew` compares promotedTypes/tested/assigned/
    /// unassigned/version and drops `nonPromotionHistory` whenever a fresh
    /// joined model is built (`flow_analysis.dart:5355-5385`): `history`
    /// does not participate, matching the official shape where the history
    /// is a side record, not a state field.
    fn same_flow(&self, other: &Self) -> bool {
        self.promoted == other.promoted
            && self.tested == other.tested
            && self.version == other.version
            && self.assigned == other.assigned
            && self.unassigned == other.unassigned
    }
}

/// One point in the flow: a reachability node plus the per-variable models
/// (official `FlowModel`), and the per-property promotion slots beside them
/// (the Rust-native form of the official property versions nested under a
/// value version — `flow_analysis.dart:5675-5685`: each promotable property
/// of a value carries its own promotion model in the same promotion-info
/// machinery; the flat slot list is keyed through the interpreter's property
/// table, `Interp::prop_slot`).
#[derive(Clone, Debug)]
pub(crate) struct State {
    pub reach: u32,
    pub models: Vec<VarModel>,
    /// Promotion models of property keys, slot-indexed in lockstep with
    /// the interpreter's property table. The same [`VarModel`] machinery
    /// runs over them (promotions, joins, the finally attach); `version`
    /// stays 0 except the receiver-captured sentinel, which permanently
    /// blocks promotion lookups (official `_handleProperty`'s
    /// target-version gate, `flow_analysis.dart:9021-9026`, plus the
    /// `tryMarkNonNullable` write-capture bail, :4055-4057).
    pub props: Vec<VarModel>,
}

impl State {
    pub(crate) fn new(initial_reach: u32) -> Self {
        Self {
            reach: initial_reach,
            models: Vec::new(),
            props: Vec::new(),
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

    pub(crate) fn prop(&self, slot: u32) -> VarModel {
        self.props
            .get(slot as usize)
            .copied()
            .unwrap_or_else(VarModel::fresh)
    }

    fn with_prop(mut self, slot: u32, model: VarModel) -> Self {
        let index = slot as usize;
        if index >= self.props.len() {
            self.props.resize(index + 1, VarModel::fresh());
        }
        self.props[index] = model;
        self
    }

    /// Splits control flow (official `FlowModel.split`).
    pub(crate) fn split(self, reach: &mut ReachArena) -> Self {
        let r = reach.split(self.reach);
        Self {
            reach: r,
            models: self.models,
            props: self.props,
        }
    }

    /// Marks the current path unreachable (official `setUnreachable`).
    pub(crate) fn set_unreachable(self, reach: &mut ReachArena) -> Self {
        let r = reach.set_unreachable(self.reach);
        Self {
            reach: r,
            models: self.models,
            props: self.props,
        }
    }

    /// Drops the innermost checkpoint (official `unsplit`).
    /// Unsplit repeatedly until the parent is `target` (official
    /// `unsplitTo`): merges the split chain down to the checkpoint.
    pub(crate) fn unsplit_to(self, reach: &mut ReachArena, target: u32) -> Self {
        Self {
            reach: reach.unsplit_to(self.reach, target),
            models: self.models,
            props: self.props,
        }
    }

    pub(crate) fn unsplit(self, reach: &mut ReachArena) -> Self {
        let r = reach.unsplit(self.reach);
        Self {
            reach: r,
            models: self.models,
            props: self.props,
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

    /// Records a write (official `PromotionModel.write`, `_demoteViaAssignment`,
    /// `_tryPromoteToTypeOfInterest`): marks assigned, drops promotions the
    /// written type no longer satisfies, and re-promotes to a type of
    /// interest (the declared type's non-null form or a previously tested
    /// type) when the written value matches. `declared_args` is the
    /// declared type's written-argument tree — the declared non-null
    /// candidate's rows (#1010); `written_args` the written type's (#2204
    /// phase D — the demotion's coverage questions compare instantiated
    /// spellings through the typed seam). `arena`/`names` are the
    /// recorder's argument arena and interned-name lexeme table the typed
    /// ops read. `write_span` is the write expression's own range — the
    /// `DemoteViaExplicitWrite.node` anchor (upstream passes the
    /// assignment node, `assignment_expression_resolver.dart:324`);
    /// `None` for writes that are not the official's real-write events
    /// (declaration initializers, the declared for-in head — upstream's
    /// `declare`/`_initialize` carry no demotion reason).
    #[expect(clippy::too_many_arguments)] // one slot per fact dimension of the official write event
    pub(crate) fn write(
        self,
        ops: &dyn TypeOps,
        lists: &mut Lists,
        reasons: &mut Reasons,
        var: u32,
        declared: Ty,
        declared_args: u32,
        written: Ty,
        written_args: u32,
        promote_to_interest: bool,
        write_span: Option<dartq_source::SourceRange>,
        arena: &[Vec<crate::ty::ArgTy>],
        names: &[Box<str>],
    ) -> Self {
        let model = self.model(var);
        if model.captured() {
            // Write-captured: stays captured, assigned, unpromoted — and the
            // history drops with it (the official capture arm builds a fresh
            // `PromotionModel` without `nonPromotionHistory`,
            // flow_analysis.dart:4811-4818).
            return self.with_model(
                var,
                VarModel {
                    assigned: true,
                    unassigned: false,
                    history: NIL,
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
        // The coverage questions run over the instantiated spellings: a
        // written `List<int>` keeps a `List<num>` promotion the bare
        // name-equality would drop, and drops a `Map<..>` promotion through
        // the resolved hierarchy where the name level guessed.
        let nodes: Vec<(Ty, u32)> = lists.iter_entries(model.promoted).collect();
        let keep_from = nodes
            .iter()
            .position(|&(ty, args)| covers(written, written_args, ty, args, ops, arena, names))
            .unwrap_or(nodes.len());
        let mut promoted = NIL;
        for (ty, args) in nodes[keep_from..].iter().rev() {
            promoted = lists.cons(*ty, *args, promoted);
        }
        // Each dropped entry appends its non-promotion link, most specific
        // first — the official append order (`flow_analysis.dart:4901-4923`
        // iterates `numElementsToKeep` downward = most-specific first). The
        // reason attaches only to real writes: the official's
        // `nonPromotionReason` is null for declare/initialize writes (the
        // `assert(false)` arm at :4914-4916), which would record nothing.
        let mut history = model.history;
        if let Some(span) = write_span {
            for (ty, args) in &nodes[..keep_from] {
                history = reasons.cons(
                    *ty,
                    *args,
                    crate::facts::NonPromotionCause::ExplicitWrite { span },
                    history,
                );
            }
        }
        // The rebuilt chain id before type-of-interest re-promotion (see
        // the no-change early return below).
        let post_demote = promoted;
        if promote_to_interest
            && !promoted_is(ops, lists, promoted, written, written_args, arena, names)
        {
            // Types of interest: the declared type's non-null form, then the
            // tested types and their non-null forms (official
            // `_tryPromoteToTypeOfInterest`). The written type must be a
            // proper subtype of the currently promoted type. Every coverage
            // question compares the instantiated spellings — the written
            // type's `written_args`, the current head's chain rows, each
            // candidate's own rows.
            let (current, current_args) = if promoted == NIL {
                (declared, declared_args)
            } else {
                (lists.head(promoted), lists.head_args(promoted))
            };
            let mut candidates: Vec<(Ty, u32)> = Vec::new();
            let declared_non_null = ops.promote_to_non_null(declared);
            if declared_non_null != declared {
                consider(
                    ops,
                    written,
                    written_args,
                    current,
                    current_args,
                    declared_non_null,
                    declared_args,
                    arena,
                    names,
                    &mut candidates,
                );
            }
            for (tested, args) in lists.iter_entries(model.tested) {
                consider(
                    ops,
                    written,
                    written_args,
                    current,
                    current_args,
                    tested,
                    args,
                    arena,
                    names,
                    &mut candidates,
                );
                let tested_non_null = ops.promote_to_non_null(tested);
                if tested_non_null != tested {
                    consider(
                        ops,
                        written,
                        written_args,
                        current,
                        current_args,
                        tested_non_null,
                        args,
                        arena,
                        names,
                        &mut candidates,
                    );
                }
            }
            // The exact-written-type shortcut (official
            // `_tryPromoteToTypeOfInterest`'s `type == writtenType` arm,
            // `flow_analysis.dart:4987-4990`): a candidate equal to the
            // written type promotes immediately, ahead of the unique-subtype
            // question — without it an `x is T`-tested `x = <T value>`
            // fails to promote whenever a second interest type is
            // incomparable. Otherwise the unique candidate that is a
            // subtype of all the others wins.
            if let Some((_, args)) = candidates.iter().find(|(c, _)| *c == written) {
                let args = *args;
                promoted = add_promoted(ops, lists, promoted, written, args, arena, names);
            } else if let Some((promoted_to, args)) =
                unique_subtype(ops, &candidates, arena, names)
            {
                promoted = add_promoted(ops, lists, promoted, promoted_to, args, arena, names);
            }
        }
        // A write that changed nothing (no demotion, no re-promotion) on an
        // already-assigned variable takes the official's identical-model
        // early return (`flow_analysis.dart:4838-4846`), which builds a
        // fresh `PromotionModel` without `nonPromotionHistory` — the prior
        // history drops with it (an upstream quirk mirrored verbatim:
        // `x = 1` after `x = null` erases the earlier demotion link).
        let history = if keep_from == 0 && promoted == post_demote && model.assigned {
            NIL
        } else {
            history
        };
        self.with_model(
            var,
            VarModel {
                promoted,
                tested: model.tested,
                version: model.version.wrapping_add(1).max(1),
                assigned: true,
                unassigned: false,
                history,
            },
        )
    }

    /// Records a promotion to `promoted` after a successful type test of
    /// `tested` (official `_finishTypeTest`). Each optional side is a
    /// `(type, written-argument tree)` pair (#1010): the tested side joins
    /// the types-of-interest chain, the promoted side the promotion chain,
    /// both with their rows. `arena`/`names` are the recorder's argument
    /// arena and interned-name lexeme table the typed seam reads (#2204
    /// phase D).
    pub(crate) fn finish_type_test(
        self,
        ops: &dyn TypeOps,
        lists: &mut Lists,
        var: u32,
        tested: Option<(Ty, u32)>,
        promoted: Option<(Ty, u32)>,
        arena: &[Vec<crate::ty::ArgTy>],
        names: &[Box<str>],
    ) -> Self {
        let model = self.model(var);
        let mut new_tested = model.tested;
        if let Some((tested, args)) = tested
            && !lists.iter(model.tested).any(|t| t == tested)
        {
            new_tested = append_tested(lists, model.tested, tested, args);
        }
        let mut new_promoted = model.promoted;
        if let Some((promoted, args)) = promoted {
            new_promoted =
                add_promoted(ops, lists, model.promoted, promoted, args, arena, names);
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

    /// Records the non-null promotion on a property slot (the property
    /// mirror of the null-check face of [`State::finish_type_test`] —
    /// official `tryMarkNonNullable` through `_finishTypeTest`,
    /// `flow_analysis.dart:4046-4073`; the upstream call passes no tested
    /// type, so only the promotion chain moves). A receiver-captured slot
    /// promotes nothing (the official `info.writeCaptured` bail, :4055-4057
    /// — the receiver's value version association is permanently gone).
    /// `marker` is the non-null face of the property's marker type; a chain
    /// that already heads with it is the `isValidPromotionStep` no-op.
    pub(crate) fn promote_prop_non_null(
        self,
        ops: &dyn TypeOps,
        lists: &mut Lists,
        slot: u32,
        marker: Ty,
        arena: &[Vec<crate::ty::ArgTy>],
        names: &[Box<str>],
    ) -> Self {
        let model = self.prop(slot);
        if model.captured() {
            return self;
        }
        let promoted = add_promoted(
            ops,
            lists,
            model.promoted,
            marker,
            crate::ty::EMPTY_ARGS,
            arena,
            names,
        );
        if promoted == model.promoted {
            return self;
        }
        self.with_prop(slot, VarModel { promoted, ..model })
    }

    /// Joins two states (official `FlowModel.join` + `joinPromotionInfo`):
    /// reachable if either is; assigned if either is; unassigned only if
    /// both are; promotions keep the common (less specific) chain prefix.
    /// The property slots join under the same per-key rule (official
    /// `joinPromotionInfo` walks property keys beside variable keys,
    /// `flow_analysis.dart:5830-5940`), over the union of the two sides'
    /// slot extents — a promotion survives a merge only when it stands on
    /// every incoming path.
    pub(crate) fn join(
        first: Option<Self>,
        second: Option<Self>,
        reach: &mut ReachArena,
        lists: &mut Lists,
        vars: usize,
    ) -> Self {
        let Some(first) = first else {
            // Both sides absent (a recovered construct can join nothing):
            // degrade to the fresh reachable state — no facts either way.
            return second.unwrap_or_default();
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
        let vars = u32::try_from(vars).expect("variable count fits u32");
        let mut models = Vec::with_capacity(vars as usize);
        for var in 0..vars {
            let a = first.model(var);
            let b = second.model(var);
            models.push(join_model(a, b, lists));
        }
        let slots = first.props.len().max(second.props.len());
        let mut props = Vec::with_capacity(slots);
        for slot in 0..slots {
            let slot = u32::try_from(slot).expect("property slot count fits u32");
            let a = first.prop(slot);
            let b = second.prop(slot);
            props.push(join_model(a, b, lists));
        }
        Self {
            reach: first.reach,
            models,
            props,
        }
    }

    /// Conservative join at loop heads and closure entries (official
    /// `FlowModel.conservativeJoin`): written variables lose promotions
    /// (finals keep them) and definite-unassignedness; captured variables
    /// become write-captured and lose all promotions. `reason` is the
    /// official `getNonPromotionReason` payload — `Some` only at
    /// suspension points (the official's sole call site,
    /// `flow_analysis.dart:7935-7947`): each dropped promoted type appends
    /// a `DemoteViaSuspension` link. Every other conservative join passes
    /// `None`, which drops the history outright — the official's
    /// `discardPromotionsAndMarkNotUnassigned` with a null reason leaves
    /// `nonPromotionHistory` null (`flow_analysis.dart:4732-4741`), so a
    /// loop head or closure entry erases earlier demotion links.
    pub(crate) fn conservative_join(
        self,
        written: &FxHashSet<u32>,
        captured: &FxHashSet<u32>,
        finals: &FxHashSet<u32>,
        lists: &Lists,
        reasons: &mut Reasons,
        reason: Option<crate::facts::NonPromotionCause>,
    ) -> Self {
        let mut this = self;
        for &var in written {
            let model = this.model(var);
            if model.assigned && model.promoted == NIL && !model.unassigned {
                continue;
            }
            let new = if finals.contains(&var) {
                // Final variables keep promotions (their value cannot
                // change) but are no longer definitely unassigned
                // (official `markNotUnassigned` — the history rides
                // through `..model`).
                VarModel {
                    unassigned: false,
                    ..model
                }
            } else {
                // Non-final demotion: every dropped promoted type appends
                // its reason link, most specific first (official
                // `discardPromotionsAndMarkNotUnassigned`,
                // `flow_analysis.dart:4734-4741`); no reason → the whole
                // history drops.
                let history = match reason {
                    Some(demotion) => {
                        let mut history = model.history;
                        for (ty, args) in lists.iter_entries(model.promoted) {
                            history = reasons.cons(ty, args, demotion, history);
                        }
                        history
                    }
                    None => NIL,
                };
                VarModel {
                    promoted: NIL,
                    unassigned: false,
                    history,
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
                    // Official `writeCapture`: the history dies with the
                    // promotions (`flow_analysis.dart:4872-4880`).
                    history: NIL,
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
        let var_count = u32::try_from(this.models.len().max(other.models.len()))
            .expect("variable count fits u32");
        for var in 0..var_count {
            let a = this.model(var);
            let b = other.model(var);
            if a.tested == b.tested {
                continue;
            }
            let mut merged = a.tested;
            let additions: Vec<(Ty, u32)> = lists.iter_entries(b.tested).collect();
            for (ty, args) in additions {
                let present = lists.iter(merged).any(|t| t == ty);
                if !present {
                    merged = append_tested(lists, merged, ty, args);
                }
            }
            this = this.with_model(
                var,
                VarModel {
                    tested: merged,
                    // The official `inheritTested` constructor omits
                    // `nonPromotionHistory` — a changed tested set drops it
                    // (`flow_analysis.dart:5054-5068`).
                    history: NIL,
                    ..a
                },
            );
        }
        this
    }
}

/// Joins two per-variable models (official `PromotionModel.join` +
/// `_identicalOrNew`, `flow_analysis.dart:5084-5146, 5355-5385`): when the
/// joined payload equals one side's, that side's model — history included —
/// survives verbatim; a genuinely new joined model drops the history
/// (the official constructor omits `nonPromotionHistory`).
fn join_model(a: VarModel, b: VarModel, lists: &mut Lists) -> VarModel {
    if a == b {
        return a;
    }
    let captured = a.captured() || b.captured();
    // Written on one path since the branch or not, the conservative join is
    // the common (less specific) chain prefix either way; write-captured on
    // either path keeps no promotion at all (official `joinPromotionInfo`)
    // — and no types of interest either: the official's write-capture join
    // hands back an empty `tested` set (`flow_analysis.dart:5123-5125`, the
    // same shape `PromotionModel.writeCapture` builds at :4873-4879).
    let promoted = if captured {
        NIL
    } else {
        join_promoted(a.promoted, b.promoted, lists)
    };
    let tested = if captured {
        NIL
    } else {
        union_tested(a.tested, b.tested, lists)
    };
    let joined = VarModel {
        promoted,
        tested,
        version: if captured {
            CAPTURED
        } else {
            a.version.max(b.version)
        },
        assigned: a.assigned && b.assigned,
        unassigned: a.unassigned && b.unassigned,
        history: NIL,
    };
    // Official `_identicalOrNew`: a side whose five official fields match
    // the join result returns its own model (its history survives); a
    // rebuilt model carries none.
    if a.same_flow(&joined) {
        a
    } else if b.same_flow(&joined) {
        b
    } else {
        joined
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
    let mut kept: Vec<(Ty, u32)> = Vec::new();
    let mut total = 0usize;
    for entry in lists.iter_entries(a) {
        total += 1;
        if lists.iter(b).any(|other| other == entry.0) {
            kept.push(entry);
        }
    }
    if kept.len() == total {
        return a;
    }
    let mut rebuilt = NIL;
    for (ty, args) in kept.into_iter().rev() {
        rebuilt = lists.cons(ty, args, rebuilt);
    }
    rebuilt
}

fn union_tested(a: u32, b: u32, lists: &mut Lists) -> u32 {
    if a == b {
        return a;
    }
    let additions: Vec<(Ty, u32)> = lists.iter_entries(b).collect();
    let mut merged = a;
    for (ty, args) in additions {
        let present = lists.iter(merged).any(|t| t == ty);
        if !present {
            merged = append_tested(lists, merged, ty, args);
        }
    }
    merged
}

fn append_tested(lists: &mut Lists, chain: u32, ty: Ty, args: u32) -> u32 {
    // Append at the end to keep the official order (order is cosmetic; the
    // membership is what drives types-of-interest). Rebuild by consing onto
    // a reversed copy.
    let mut items: Vec<(Ty, u32)> = lists.iter_entries(chain).collect();
    items.push((ty, args));
    let mut rebuilt = NIL;
    for (ty, args) in items.into_iter().rev() {
        rebuilt = lists.cons(ty, args, rebuilt);
    }
    rebuilt
}

fn add_promoted(
    ops: &dyn TypeOps,
    lists: &mut Lists,
    chain: u32,
    ty: Ty,
    args: u32,
    arena: &[Vec<crate::ty::ArgTy>],
    names: &[Box<str>],
) -> u32 {
    // The official invariant: each promoted type is a strict subtype of the
    // previous; a repeated or mutual promotion is rejected by
    // `isValidPromotionStep` before this call. The comparisons run over the
    // instantiated spellings (#2204 phase D) — a `List<int>` chain head and
    // a `List<num>` candidate are no longer name-equal, so the
    // strict-subtype gate is the hierarchy's verdict, not a spelling
    // coincidence.
    if promoted_is(ops, lists, chain, ty, args, arena, names) {
        return chain;
    }
    if chain != NIL
        && ops.is_subtype_typed(
            lists.head(chain),
            lists.head_args(chain),
            ty,
            args,
            arena,
            names,
        )
    {
        // New type is a supertype of the current head — not a refinement.
        return chain;
    }
    lists.cons(ty, args, chain)
}

/// Whether a written value keeps a promotion entry (the coverage question
/// of [`State::write`]'s demotion, official `_demoteViaAssignment`'s
/// `written <: promotedTypes[k]`). A non-null unknown entry is the opaque
/// marker promotion (#2243 — a `var`-inferred local whose declared type
/// the lattice cannot spell): its only content is non-nullness, so the
/// written value keeps it exactly when it cannot be null — the bare
/// unknown face is top and would admit a null write.
fn covers(
    written: Ty,
    written_args: u32,
    entry: Ty,
    entry_args: u32,
    ops: &dyn TypeOps,
    arena: &[Vec<crate::ty::ArgTy>],
    names: &[Box<str>],
) -> bool {
    if entry.base == Base::Unknown && !entry.nullable {
        return ops.classify(written) == Classification::NonNullable;
    }
    ops.is_subtype_typed(written, written_args, entry, entry_args, arena, names)
}

/// The same-type dedup of the promotion chain (official `isSameType` —
/// mutual subtyping over the instantiated spellings): `x is int` then
/// `x is int` again stands down, while a `List<int>` head and a
/// `List<num>` candidate stay distinct — the name-level `t == ty` read
/// them as separate spellings too, but could not see that
/// `Response<dynamic>` and `Response<T>` are *not* the same type.
fn promoted_is(
    ops: &dyn TypeOps,
    lists: &Lists,
    chain: u32,
    ty: Ty,
    args: u32,
    arena: &[Vec<crate::ty::ArgTy>],
    names: &[Box<str>],
) -> bool {
    lists.iter_entries(chain).any(|(t, t_args)| {
        ops.is_subtype_typed(t, t_args, ty, args, arena, names)
            && ops.is_subtype_typed(ty, args, t, t_args, arena, names)
    })
}

#[expect(clippy::too_many_arguments)] // one slot per operand spelling of the official gate
fn consider(
    ops: &dyn TypeOps,
    written: Ty,
    written_args: u32,
    current: Ty,
    current_args: u32,
    candidate: Ty,
    args: u32,
    arena: &[Vec<crate::ty::ArgTy>],
    names: &[Box<str>],
    out: &mut Vec<(Ty, u32)>,
) {
    // Official `_tryPromoteToTypeOfInterest`'s per-candidate gate
    // (flow_analysis.dart:4958-4999): the written value must satisfy the
    // candidate, the candidate must be a STRICT subtype of the current
    // type (`isValidPromotionStep` — `candidate <: current` and not the
    // reverse), and the candidate must not already be in the running.
    // Every subtype question compares the instantiated spellings.
    if ops.is_subtype_typed(written, written_args, candidate, args, arena, names)
        && ops.is_subtype_typed(candidate, args, current, current_args, arena, names)
        && !ops.is_subtype_typed(current, current_args, candidate, args, arena, names)
        && !out.iter().any(|&(existing, _)| existing == candidate)
    {
        out.push((candidate, args));
    }
}

fn unique_subtype(
    ops: &dyn TypeOps,
    candidates: &[(Ty, u32)],
    arena: &[Vec<crate::ty::ArgTy>],
    names: &[Box<str>],
) -> Option<(Ty, u32)> {
    let mut found: Option<(Ty, u32)> = None;
    for &(candidate, args) in candidates {
        // "Subtype of every candidate, itself included" — the typed seam's
        // reflexivity makes the self-pair trivially true, so no identity
        // shortcut is needed; two same-name candidates with different
        // written arguments still compare their trees.
        if candidates.iter().all(|&(other, other_args)| {
            ops.is_subtype_typed(candidate, args, other, other_args, arena, names)
        }) {
            if found.is_some() {
                return None; // not unique
            }
            found = Some((candidate, args));
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
        // Official `_identicalOrNew` (`flow_analysis.dart:8614-8626,
        // 5355-5385`): when the rebuilt payload matches one side, that
        // side's model — history included — returns; a genuinely rebuilt
        // model carries none.
        let _ = lists;
        let joined = VarModel {
            promoted: after_try.promoted,
            tested: after_try.tested,
            version: after_finally.version,
            assigned: after_try.assigned || after_finally.assigned,
            unassigned: after_try.unassigned && after_finally.unassigned,
            history: NIL,
        };
        if after_try.same_flow(&joined) {
            after_try
        } else if after_finally.same_flow(&joined) {
            after_finally
        } else {
            joined
        }
    }
}
