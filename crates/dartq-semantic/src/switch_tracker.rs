//! The switch/pattern tracker: the open-switch and pattern bookkeeping the
//! diagnosis engine drives through the parser's pattern/switch events, plus
//! the name-level exhaustiveness world those events feed (seam 1 of the
//! diagnose engine extraction, ticket #444).
//!
//! [`SwitchTracker`] owns the switch/pattern state the engine mutates from
//! its event handlers; [`NameWorld`] resolves the sealed/enum/subtype facts
//! the exhaustiveness verdict reads, over the unit's element model.

use crate::diagnose::Engine;
use crate::library::{ElementIndex, resolve_container};
use crate::model::{ArgListId, Container, ContainerKind, MemberKind, Ty};
use dartq_resolution::{LibraryGraph, LibraryId, Namespaces};
use dartq_scanner::{Keyword, TokenKind};
use dartq_source::{FileId, SourceRange, StringId, StringTable};
use dartq_types::{ExhaustivenessWorld, Key, Single, Space};
use rustc_hash::FxHashMap;
use std::cell::RefCell;

/// The open-switch context stack: a `Vec` face for every caller (the
/// `Deref` chain keeps `last`/`last_mut`/`len`/`is_empty` verbatim), with
/// one twist — `pop` RETAINS the closed context into `completed` so the
/// teardown join can re-decide the exhaustiveness verdict against the
/// flow-narrowed scrutinee type (#2345: upstream's
/// `ExhaustivenessValidator` reads the scrutinee's `typeOrThrow`, the
/// flow-narrowed `staticType` at the switch site, for every face —
/// reachability, covering, default-reachability — not the declared type
/// the push-time latch carries). `pop_discard` is the non-retaining twin
/// for contexts that never carried an exhaustiveness question (the
/// if-case window pops unreported).
#[derive(Default)]
pub(crate) struct SwitchStack {
    open: Vec<SwitchCtx>,
    /// Closed switch contexts in close order, retained by [`Self::pop`]
    /// for the teardown's narrowed-scrutinee re-verdict.
    pub(crate) completed: Vec<SwitchCtx>,
}

impl SwitchStack {
    /// Closes the top context, retaining it for the teardown's
    /// flow-narrowed re-decision. The `report_switch` caller consumes the
    /// returned value exactly as `Vec::pop`'s.
    pub(crate) fn pop(&mut self) -> Option<SwitchCtx> {
        let ctx = self.open.pop();
        if let Some(ctx) = &ctx {
            self.completed.push(SwitchCtx {
                is_expression: ctx.is_expression,
                switch_kw: ctx.switch_kw,
                scrutinee: ctx.scrutinee,
                cases: ctx.cases.clone(),
                default_kw: ctx.default_kw,
                pending_root: None,
                if_case_member_scrutinee: None,
                has_invalid_type: ctx.has_invalid_type,
                // The retained copy serves the teardown's patterns-era
                // re-verdict; legacy facts die with the open context.
                legacy_cases: Vec::new(),
            });
        }
        ctx
    }

    /// Closes the top context without retaining (the if-case window's own
    /// close — an if-case reports no exhaustiveness face).
    pub(crate) fn pop_discard(&mut self) -> Option<SwitchCtx> {
        self.open.pop()
    }
}

impl std::ops::Deref for SwitchStack {
    type Target = Vec<SwitchCtx>;

    fn deref(&self) -> &Self::Target {
        &self.open
    }
}

impl std::ops::DerefMut for SwitchStack {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.open
    }
}

/// The open-switch/pattern state: one engine owns one tracker; the state
/// and its update rules were engine fields/methods before the extraction
/// (behavior byte-identical).
pub(crate) struct SwitchTracker {
    /// Open switch contexts (statements and expressions): the scrutinee's
    /// push-time type, the cases completed so far, and the anchors the
    /// diagnostics attach to.
    pub(crate) switches: SwitchStack,
    /// The open pattern-nesting stack (`begin_pattern`/`end_pattern`
    /// brackets); each frame collects the leaf restrictions it spans.
    pub(crate) pattern_stack: Vec<PatternFrame>,
    /// The number of currently open patterns.
    pub(crate) pattern_depth: usize,
    /// Completed root restrictions per open case (switch cases only).
    pub(crate) pattern_roots: Vec<Restriction>,
    /// A case-header or if-case pattern window is open
    /// (`begin_case_expression`/`begin_switch_expression_case`/
    /// [`Self::open_if_case`] through the pattern's completion): the
    /// window whose root-pattern checks run against the matched type.
    /// Only a case header's root feeds exhaustiveness — an if-case's
    /// closes uncaptured ([`Self::close_if_case`]) and pattern
    /// assignments never open one.
    pub(crate) case_sink_open: bool,
    /// The open window is an if-case's own
    /// ([`Self::open_if_case`]/[`Self::close_if_case`], one root apart):
    /// the exact open/close pairing the engine's root branch reads to
    /// tell an if-case root from a case header's.
    pub(crate) if_case_window: bool,
    /// The in-flight constant pattern's classification, armed by the
    /// literal/identifier events inside it.
    pub(crate) pending_const: Option<Restriction>,
    /// The `switch` keyword of the open switch statement/expression (the
    /// non-exhaustive anchor; the block events only carry the `{`).
    pub(crate) switch_kw_latch: Option<SourceRange>,
    /// The UTF-16 offset of the open root pattern's first token (latched
    /// by `begin_pattern` at depth 1): the list probe's scan start
    /// ([`case_first_opener_is_list`]). A switch-expression case carries
    /// no keyword anchor, so the root's own head is the one start every
    /// clause form shares.
    pub(crate) root_begin: u32,
}

impl SwitchTracker {
    pub(crate) fn new() -> Self {
        SwitchTracker {
            switches: SwitchStack::default(),
            pattern_stack: Vec::new(),
            pattern_depth: 0,
            pattern_roots: Vec::new(),
            case_sink_open: false,
            if_case_window: false,
            pending_const: None,
            switch_kw_latch: None,
            root_begin: 0,
        }
    }

    /// Appends a leaf to the innermost open pattern frame; outside any
    /// pattern the stack is empty and the leaf is dropped.
    pub(crate) fn pattern_leaf(&mut self, leaf: Restriction) {
        if let Some(frame) = self.pattern_stack.last_mut() {
            frame.leaves.push(leaf);
        }
    }

    /// Collapses one frame's leaves into the restriction it covers
    /// (`||` unions, `&&` intersects, a bare leaf passes through, a
    /// type-free pattern covers its context). A trailing `Unknown` is the
    /// bracket's own marker leaf (`handle_list_pattern`'s,
    /// listener.rs:7874 — element sub-patterns merge as one leaf each
    /// through `end_pattern`'s parent push, so the marker is always last
    /// when the frame is a list's). The bracket's KIND comes off the
    /// token stream: the clause's first opener decides list vs
    /// map/record/paren (upstream knows the node kind; the name level
    /// only sees a trailing `Unknown` for every bracket face).
    pub(crate) fn collapse(
        world: &NameWorld<'_>,
        leaves: Vec<Restriction>,
        or_ops: usize,
    ) -> Restriction {
        if leaves.is_empty() {
            // A type-free variable/wildcard: the context full space. The
            // context is substituted when the case closes.
            return Restriction::NamedContext;
        }
        if or_ops > 0 {
            return Restriction::Union(leaves);
        }
        // The list-pattern split: the frame's last leaf is the bracket's
        // Unknown marker; the elements before it (and around a rest mark)
        // are the head, the rest, and the tail — `createListSpace`'s
        // three-way split (shared.dart:832-911). The probe only fires on
        // a trailing-Unknown frame: every other bracket face (object,
        // record, relational, constant) either carries a typed leaf or
        // stays a single `Unknown`, and a non-`[` first opener keeps the
        // reduce path untouched.
        if matches!(leaves.last(), Some(Restriction::Unknown))
            && case_first_opener_is_list()
            && let Some((list, elem)) = world.list_pattern_elem()
        {
            // The marker is never a list ELEMENT: a frame carrying only
            // the marker is `[]`, a frame carrying elements + rest mark +
            // marker is a rest-bearing list. The rest mark — when
            // present — sits one leaf after its own sub-pattern
            // (`handle_rest_pattern` fires after the sub merges), so the
            // leaf before it is the rest's sub-pattern when `had_sub`.
            let elements = &leaves[..leaves.len() - 1];
            let rest_mark = elements
                .iter()
                .position(|leaf| matches!(leaf, Restriction::RestMark { .. }));
            let (head, rest, tail) = match rest_mark {
                Some(mark) => {
                    let Restriction::RestMark { had_sub } = elements[mark] else {
                        unreachable!()
                    };
                    // A `...sub` sub-pattern's leaf immediately precedes
                    // the mark; a bare `...`'s mark ends the head run.
                    if had_sub && mark == 0 {
                        // `[...sub]` — the sub merged before the mark, so
                        // the mark cannot be first. A degenerate shape:
                        // keep the under-report arm.
                        return Restriction::Unknown;
                    }
                    let head_len = mark - usize::from(had_sub);
                    let rest = if had_sub {
                        Some(Some(Box::new(elements[mark - 1].clone())))
                    } else {
                        Some(None)
                    };
                    (
                        elements[..head_len].to_vec(),
                        rest,
                        elements[mark + 1..].to_vec(),
                    )
                }
                None => (elements.to_vec(), None, Vec::new()),
            };
            return Restriction::List {
                list,
                elem,
                head,
                rest,
                tail,
            };
        }
        // `&&` joins (or a bare single leaf) intersect at name level.
        leaves
            .into_iter()
            .reduce(|a, b| a.and_in(world, b))
            .unwrap_or(Restriction::Unknown)
    }
}

/// Whether the case clause now collapsing opens with a LIST bracket. A
/// list pattern's own frame is the only trailing-`Unknown` root frame a
/// `[` head produces — object/record/paren brackets open with `(` or
/// `{`, and a bare `[]` scans as the single `Index` token. The probe is
/// confined to the outermost collapsing frame (`end_pattern` pops the
/// frame before collapsing, so an empty `pattern_stack` marks the root):
/// element frames merge inside their bracket's frame and never reach
/// this read, so a `[>5]` element cannot ride the case's outer `[`
/// answer. The scan starts at the root pattern's own first token (the
/// statement case's `case` keyword and the expression case's missing one
/// both precede it); terminators bound it to the clause head (`:`/`=>`/
/// `,`, `when`, `;`), so a bracket-free head (`case >5`, `case 1:`)
/// declines. Only a case header's root reaches the exhaustiveness walk —
/// every other root's restriction is dropped at its close.
/// `const [...]` is a constant pattern (value equality), not a list
/// pattern — a `const` keyword immediately before the `[` declines it.
fn case_first_opener_is_list() -> bool {
    let Some(engine) = fold_engine() else {
        return false;
    };
    // Only the case's outermost frame may answer for the head's bracket —
    // a nested frame's own bracket is not necessarily the clause's first.
    if !engine.switch.pattern_stack.is_empty() {
        return false;
    }
    let tokens = engine.tokens.as_slice();
    let start = tokens.partition_point(|token| token.utf16_offset < engine.switch.root_begin);
    let mut prev_was_const = false;
    for token in &tokens[start..] {
        match token.kind {
            // `[]` scans as one `Index` token — an empty list pattern.
            TokenKind::Index | TokenKind::OpenSquare => {
                return !prev_was_const;
            }
            // Any other bracket head (map/record/paren/object) is not the
            // list face.
            TokenKind::OpenCurly | TokenKind::OpenParen => return false,
            // The clause head's own terminators: the pattern ended without
            // a bracket head (relational, constant, bare type test).
            TokenKind::Colon
            | TokenKind::Comma
            | TokenKind::Semicolon
            | TokenKind::Function
            | TokenKind::Eof => return false,
            TokenKind::Keyword(kw) if kw == Keyword::When => return false,
            TokenKind::Keyword(kw) => {
                prev_was_const = kw == Keyword::Const;
            }
            _ => prev_was_const = false,
        }
    }
    false
}

impl SwitchTracker {
    /// topmost restriction is the header pattern's coverage (pushed between
    /// the window's open and here), everything below is debris from earlier
    /// bodies. `None` — a legacy or recovered header with no pattern —
    /// falls back to the unknown space.
    pub(crate) fn close_case_sink(&mut self) -> Option<Restriction> {
        self.case_sink_open = false;
        let root = self.pattern_roots.pop();
        self.pattern_roots.clear();
        root
    }

    /// Opens an if-case's one-case window: the language gives the pattern
    /// the condition value as its matched type, so the shared root-pattern
    /// checks read a context of their own — the same sink a case header
    /// opens. `kw` is never read (an if-case reports nothing at the
    /// keyword); it fills the context's anchor slot.
    pub(crate) fn open_if_case(
        &mut self,
        scrutinee: Ty,
        kw: SourceRange,
        member_scrutinee: Option<SourceRange>,
    ) {
        self.switches.push(SwitchCtx {
            is_expression: false,
            switch_kw: kw,
            scrutinee,
            cases: Vec::new(),
            default_kw: None,
            pending_root: None,
            if_case_member_scrutinee: member_scrutinee,
            has_invalid_type: false,
            legacy_cases: Vec::new(),
        });
        self.case_sink_open = true;
        self.if_case_window = true;
    }

    /// Closes the if-case window at the root pattern's completion: the
    /// context pops unreported (an if-case carries no exhaustiveness
    /// question) and no root is captured — the stale-root pins.
    pub(crate) fn close_if_case(&mut self) {
        self.if_case_window = false;
        self.case_sink_open = false;
        self.switches.pop_discard();
    }

    /// Substitutes a captured root's type-free coverage with the
    /// scrutinee's context type.
    pub(crate) fn substitute_root(&self, root: Option<Restriction>) -> Restriction {
        let space = root.unwrap_or(Restriction::Unknown);
        if space == Restriction::NamedContext {
            return match NameTy::from_semantic(
                self.switches.last().map_or(Ty::Unknown, |c| c.scrutinee),
            ) {
                NameTy::Named(name, nullable) | NameTy::NamedArgs(name, nullable, _) => {
                    Restriction::Named(name, nullable)
                }
                // A `Null` matched type is one value: the type-free
                // pattern covers it whole, and a written `Null` type test
                // with no leaf (the typed-wildcard face) is that same
                // space — never the empty one, which reported a bare
                // `switch (null) { case Null _: }` as non-exhaustive.
                NameTy::Null => Restriction::Null,
                // A record/function context (a name-level sentinel) has no
                // head to name: the type-free pattern covers exactly that
                // context type, which [`collect_singles`] lowers verbatim.
                // The pre-#3236 `Unknown` made such a case match nothing at
                // all, so it could neither absorb a later case nor prove
                // the switch exhaustive. The top and `Never` keep their
                // old answer — `TopNullable` also carries an UNRESOLVED
                // scrutinee, where covering everything could report cases
                // the oracle keeps live.
                NameTy::Unknown(_) => Restriction::NamedContext,
                _ => Restriction::Unknown,
            };
        }
        space
    }

    /// Opens a switch context; `scrutinee` is the scrutinee's push-time
    /// type (the engine latches it from the row stack).
    pub(crate) fn open_switch(&mut self, scrutinee: Ty, is_expression: bool, kw: SourceRange) {
        // Any roots surviving from before this switch are debris (a broken
        // header's un-drained capture): they must never feed this switch's
        // cases.
        self.pattern_roots.clear();
        self.case_sink_open = false;
        self.switches.push(SwitchCtx {
            is_expression,
            switch_kw: kw,
            scrutinee,
            cases: Vec::new(),
            default_kw: None,
            pending_root: None,
            if_case_member_scrutinee: None,
            has_invalid_type: false,
            legacy_cases: Vec::new(),
        });
    }

    /// Closes an object pattern (the parser fires `handle_object_pattern`
    /// *after* the rest — fields and boundaries included): the row lowers
    /// to the bare head type exactly when every field's drained restriction
    /// covers its member's whole column ([`NameWorld::property_type`], the
    /// member-table walk, superclass chain included). Any residual narrows
    /// the row below the name level's expressible spaces, so the row is
    /// the unknown space (the established under-report). `head` is `None`
    /// for the unmodeled `Type.field` shorthand shapes.
    pub(crate) fn close_object_fields(
        &mut self,
        world: &NameWorld<'_>,
        head: Option<StringId>,
        fields: &[ObjectField],
    ) {
        let Some(ty) = head else {
            self.pattern_leaf(Restriction::Unknown);
            return;
        };
        // A `dynamic(...)` object pattern's head is the nullable top: the
        // pattern is the total catch-all (its space covers everything —
        // `dynamic(:var runtimeType) => ...` closing interpreter.dart's
        // `switch (value)` over `Object?`, the oracle's matched-by-everything
        // arm). The head spelling decides; no member column is consulted.
        if world.strings.resolve(ty) == "dynamic" {
            self.pattern_leaf(Restriction::Named(world.object_id, true));
            return;
        }
        // The per-field constraints the name level can express: a leaf that
        // covers its whole column (or the type-free `var x`/`_`) contributes
        // nothing, a `null` leaf or a refining type-test/value leaf rides the
        // single's property sub-space, and anything else (a conjunctive
        // residual, a positional field's constraining leaf — the member
        // table keeps no declaration order) degrades the pattern to the
        // unknown space, the pre-existing under-report posture.
        let mut carried: Vec<(StringId, Restriction)> = Vec::new();
        let mut expressible = true;
        for field in fields {
            match field.leaves.as_slice() {
                // A type-free sub-pattern (`var x`, `_`, the `:x` shorthand)
                // takes the HEAD's column type (`createVariableSpace` /
                // the wildcard space over the head's property type): it
                // covers that column, yet not a subtype whose merged
                // column is wider (a mixin's `String? value` re-surfacing
                // over the head's own `String value`). Carried only when
                // a subtype can widen it, i.e. a non-nullable column.
                [] => {
                    if let Some(name) = field.name
                        && let Some(NameTy::Named(column, false)) = world.member_column(ty, name)
                        && world.is_known_head(column)
                    {
                        carried.push((name, Restriction::Named(column, false)));
                    }
                }
                [leaf] => {
                    let Some(name) = field.name else {
                        expressible = false;
                        break;
                    };
                    let Some(column) = world.member_column(ty, name) else {
                        expressible = false;
                        break;
                    };
                    match leaf {
                        Restriction::Null => carried.push((name, Restriction::Null)),
                        Restriction::Named(lt, ln) if field.binder_non_null => {
                            // The null-check binder's leaf names the
                            // SCRUTINEE (the root face the `?` handler
                            // builds from the switch context); the field's
                            // constraint is its OWN column's non-null
                            // projection (`createNullCheckSpace` over the
                            // property's static type).
                            match column {
                                NameTy::Named(column_name, _) => {
                                    carried.push((name, Restriction::Named(column_name, false)));
                                }
                                _ => {
                                    expressible = false;
                                    break;
                                }
                            }
                        }
                        Restriction::Named(lt, ln) => {
                            if !world.is_subtype_of(&column, &NameTy::Named(*lt, *ln)) {
                                carried.push((name, leaf.clone()));
                            }
                        }
                        // The value-precise constants the name level does
                        // express (`A(b: true)`, `A(e: E.a)`) narrow their
                        // column exactly like a type test, so the row keeps
                        // its real shape.
                        Restriction::BoolValue(_) | Restriction::EnumValue(..) => {
                            if leaf_ty(leaf)
                                .is_some_and(|leaf_ty| !world.is_subtype_of(&column, &leaf_ty))
                            {
                                carried.push((name, leaf.clone()));
                            }
                        }
                        // An unexplorable leaf (a relational or container
                        // sub-pattern's unknown, an unrepresentable
                        // constant) is the official unknown space
                        // (`createRelationalSpace` → `createUnknownSpace`,
                        // shared.dart:624-627: the pattern "does not add to
                        // the exhaustiveness coverage"). The COLUMN still
                        // rides along as the empty sub-space, so the row
                        // covers "the head whose column matches nothing" and
                        // the gap witnesses the column at its own static type
                        // (`Map(length: <= 0)` → `Map<bool, bool>(length:
                        // int())`, co19 exhaustiveness_map_A01_t02). Only an
                        // unnamed/unknown column still degrades the whole
                        // pattern.
                        Restriction::Unknown => carried.push((name, Restriction::Unknown)),
                        _ => {
                            expressible = false;
                            break;
                        }
                    }
                }
                _ => {
                    expressible = false;
                    break;
                }
            }
        }
        let leaf = if !expressible {
            Restriction::Unknown
        } else if carried.is_empty() {
            Restriction::Named(ty, false)
        } else {
            Restriction::Object(ty, carried)
        };
        self.pattern_leaf(leaf);
    }
}

/// Whether every leaf of the case's root pattern is a value-precise
/// constant (`Null`/`BoolValue`/`EnumValue`): only constant roots run
/// the official flow's equality-check path
/// (`_handleEqualityCheckPattern`), where a known-false result makes
/// the matched state unreachable and the member's own body dies. A
/// disjoint type test (`case A _` over an enum) takes the restriction
/// path, which joins — its body stays live (oracle probes: `case
/// null:` over a non-nullable enum reports `dead_code` over the body,
/// `case A _:` reports `pattern_never_matches_value_type` alone).
pub(crate) fn restriction_is_constant(space: &Restriction) -> bool {
    match space {
        Restriction::Null | Restriction::BoolValue(_) | Restriction::EnumValue(..) => true,
        Restriction::Union(parts) => parts.iter().all(restriction_is_constant),
        _ => false,
    }
}

/// Whether every leaf of the case's root pattern is the `null` constant.
/// The body-death companion of [`restriction_is_constant`]: the flow's
/// known-false equality (`_typesAreDisjointDueToNullability`) needs the
/// whole union null — a mixed `null || true` keeps its body live through
/// the non-null leaf (oracle 3.13.1, probe2444 pg: the two never-match
/// warnings report, no `dead_code`).
pub(crate) fn restriction_is_null_only(space: &Restriction) -> bool {
    match space {
        Restriction::Null => true,
        Restriction::Union(parts) => parts.iter().all(restriction_is_null_only),
        _ => false,
    }
}

/// Whether the case space can hold no value of the matched type: every
/// leaf's single type must be unable to inhabit `matched`
/// ([`NameWorld::can_be_subtype_of`], the mutual-exclusion companion of
/// the covering walk's subtype filter). A leaf that may overlap — an enum
/// constant against the `Enum` root its declaration implicitly implements,
/// a constant against the scrutinee's own enum — keeps the case live no
/// matter what the covering witnesses say: the space model drops such a
/// row (`is_subtype_of(matched, constant)` fails toward the value) and
/// regenerates the bare scrutinee witness, so witness-set equality alone
/// cannot see the overlap and must never produce a never-match here. The
/// leaf set mirrors [`collect_singles`]'s lowering (each leaf *is* the
/// single the case space carries); a logical-or never matches only when
/// both alternatives do.
pub(crate) fn restriction_disjoint_from(
    world: &NameWorld<'_>,
    space: &Restriction,
    matched: NameTy,
) -> bool {
    match space {
        Restriction::Union(parts) => parts
            .iter()
            .all(|part| restriction_disjoint_from(world, part, matched)),
        // The unknown space and the context leaf match by construction —
        // the engine under-reports instead of mis-answering.
        Restriction::Unknown | Restriction::NamedContext | Restriction::RestMark { .. } => false,
        Restriction::Null => !world.can_be_subtype_of(&NameTy::Null, &matched),
        // A list pattern overlaps the matched type when its `List<E>`
        // head can be one (`canBeSubtypeOf`'s type ask over the
        // unrestricted list face — the element/size refinements only
        // narrow the case's own space, the `Restriction::Object` rule).
        Restriction::List { elem, .. } => {
            !world.can_be_subtype_of(&world.list_ty(*elem), &matched)
        }
        // The object pattern's head test decides the overlap; the property
        // refinements only narrow the case's own space.
        Restriction::Object(name, _) => {
            !world.can_be_subtype_of(&NameTy::Named(*name, false), &matched)
        }
        Restriction::Named(name, nullable) => {
            !world.can_be_subtype_of(&NameTy::Named(*name, *nullable), &matched)
        }
        Restriction::NamedArgs(name, nullable, args) => {
            !world.can_be_subtype_of(&NameTy::NamedArgs(*name, *nullable, *args), &matched)
        }
        Restriction::BoolValue(value) => {
            !world.can_be_subtype_of(&NameTy::BoolValue(*value), &matched)
        }
        Restriction::EnumValue(enum_name, constant) => {
            !world.can_be_subtype_of(&NameTy::EnumValue(*enum_name, *constant), &matched)
        }
    }
}

/// `isAlwaysExhaustive` (type_system.dart:846-887) at name level:
/// `bool`, `Null`, enums, and sealed classes split completely; nothing
/// else does. The official check reads the scrutinee type's *element*
/// face (`isDartCoreBool`/enum/`isSealed`) and is nullability-agnostic
/// — `S?` of a sealed `S` stays always-exhaustive, the null half being
/// the space model's nullable split rather than this gate (`dynamic`
/// has no element and stays `false`). The official's extension-type arm
/// is modelled through [`NameWorld::extension_erasure`]: an extension
/// type's element face recurses on its representation type
/// (type_system.dart:862-868), so `ET(E _)` over an enum `E` is
/// always-exhaustive like `E` itself. The `FutureOr<T>` arm recurses
/// into the type argument (type_system.dart:869-871) and the
/// `TypeParameterType` arm into the declared bound (:873-879) — both
/// read through the parse-time engine's boundary pool and bound tables
/// (`fold_engine`; outside a window the pre-existing `false` answer
/// stands, under-report never a false positive). The promoted-bound
/// half of the parameter arm is not modelled at this level — the
/// promoted face only exists inside a flow model this world lacks —
/// matching the flow side's conservative subset
/// (`dartq_flow::Interp::exhaustive_within` reads the same declared
/// bound). A recursion budget keeps cyclic bounds (`T extends U,
/// U extends T` — ill-formed, already reported upstream) terminating.
pub(crate) fn is_always_exhaustive(world: &NameWorld<'_>, ty: NameTy) -> bool {
    always_exhaustive_within(world, ty, 8)
}

/// [`is_always_exhaustive`] with a recursion budget: `FutureOr` chains
/// (`FutureOr<FutureOr<S>>`) and bound chains spend one unit each; an
/// exhausted chain answers `false`, the conservative arm.
fn always_exhaustive_within(world: &NameWorld<'_>, ty: NameTy, budget: u8) -> bool {
    if budget == 0 {
        return false;
    }
    match world.extension_erasure(&ty) {
        NameTy::Null => true,
        NameTy::Named(name, _) | NameTy::NamedArgs(name, _, _) => {
            if world.is_sealed_name(name) {
                return true;
            }
            let NameTy::NamedArgs(fut_name, _, args) = world.extension_erasure(&ty) else {
                return always_exhaustive_param(world, ty, budget);
            };
            // `FutureOr<T>` recurses into `T` (type_system.dart:869-871):
            // the union is always-exhaustive exactly when its base is.
            // A shadowed spelling — a declared container or an in-scope
            // parameter named `FutureOr` — never takes this arm (the
            // parameter check `is_scope_param` runs first through the
            // shadow gate).
            if world.strings.resolve(fut_name) == "FutureOr"
                && !future_or_shadowed(world, fut_name)
                && let Some(engine) = fold_engine()
            {
                let base = engine.boundary.pool().get(args).first().copied();
                if let Some(base) = base {
                    return always_exhaustive_within(
                        world,
                        NameTy::from_semantic(base),
                        budget - 1,
                    );
                }
            }
            always_exhaustive_param(world, ty, budget)
        }
        _ => false,
    }
}

/// Whether `name` is shadowed off the dart:async `FutureOr` union: a
/// declared container (an enum/sealed arm already answered), an
/// in-scope type parameter (`is_scope_param`), or a declaration-less
/// spelling that could still be a parameter bound the world cannot see.
/// Only the third shape defers to the engine's scope table — inside a
/// window the scope answer is exact; outside one the head is treated
/// as core's (the sealed/enum checks already filtered the closed
/// shadows).
fn future_or_shadowed(world: &NameWorld<'_>, name: StringId) -> bool {
    if world.container(name).is_some() {
        return true;
    }
    fold_engine().is_some_and(|engine| engine.is_scope_param(name))
}

/// The `TypeParameterType` arm (type_system.dart:873-879): an in-scope
/// type parameter is always-exhaustive when its declared bound is
/// (`T extends St` over the enum `St` splits completely). The promoted
/// bound is unmodelled at this level — the conservative subset. Only a
/// name the scope table declares as a parameter takes the arm; any
/// other spelling answers `false`.
fn always_exhaustive_param(world: &NameWorld<'_>, ty: NameTy, budget: u8) -> bool {
    let (NameTy::Named(name, _) | NameTy::NamedArgs(name, _, _)) = world.extension_erasure(&ty)
    else {
        return false;
    };
    let Some(engine) = fold_engine() else {
        return false;
    };
    if !engine.is_scope_param(name) {
        return false;
    }
    let Some(bound) = engine.param_bound(name) else {
        return false;
    };
    always_exhaustive_within(world, NameTy::from_semantic(bound), budget - 1)
}

/// The solved scrutinee of an unsolved `T extends Enum` switch (#2456): the
/// verdict the oracle reaches because its call-site inference solved the
/// generic callee (`T readEnum<T extends Enum>(…)` called with one enum's
/// `values`), while this name level still reads the declared return's bare
/// parameter spelling — a head the world cannot place, so neither the space
/// model nor the always-exhaustive gate can decide it.
///
/// Such a scrutinee is *recoverable* from the case set: the bound is the
/// `Enum` root, so the oracle's solved type is some enum — always-exhaustive
/// at the statement face (type_system.dart:846-888's enum arm) — and every
/// constant case can only name constants of that enum. When every case root
/// is a constant of ONE enum `E`, `E` is the only family the cases witness,
/// and the verdict over `E` is the solved verdict:
///
/// - the cases cover `E`'s constants ⇔ the solved switch is exhaustive
///   (they are the same set), so a full set stays silent;
/// - a missing constant is a missing constant of the solved type, so the
///   witness and the correction text the row renders are the oracle's own.
///
/// Declines (the pre-#2456 silence) whenever a fact the equivalence needs is
/// missing: a resolvable head, a parameter still in scope (the ordinary
/// bound arm owns it — [`always_exhaustive_param`]), a bound that is not the
/// `Enum` root — an unbounded `T` or one over a concrete class has no
/// always-exhaustive solved face, so guessing a family could emit a row the
/// oracle never writes — a case root that is no enum constant (a `null`, a
/// type test, an unrepresentable constant), or case constants from two
/// different enums. The bound lookup is spelling-keyed
/// ([`Engine::param_bound_of`]): among several captured windows declaring the
/// same one-letter parameter the first match stands, which can only narrow
/// this face to a *weaker* bound (`Enum` mis-seen for a concrete bound is the
/// one unsound direction) — the gate therefore also requires that every case
/// root is an enum constant, the shape only an enum-bounded scrutinee can
/// inhabit.
pub(crate) fn enum_bound_scrutinee(world: &NameWorld<'_>, ctx: &SwitchCtx) -> Option<Ty> {
    let NameTy::Named(name, false) = NameTy::from_semantic(ctx.scrutinee) else {
        return None;
    };
    if world.is_known_head(name) {
        return None;
    }
    let engine = fold_engine()?;
    if engine.is_scope_param(name) {
        return None;
    }
    // The parameter's declared bound, read through the captured declaration
    // windows rather than the live scope (the declaring executable is
    // another one — the official arm reads the *element*, type_system.dart
    // :873-879, so a use site outside the declaring scope still decides).
    let (NameTy::Named(bound_head, false) | NameTy::NamedArgs(bound_head, false, _)) =
        NameTy::from_semantic(engine.param_bound_of(name)?)
    else {
        return None;
    };
    if bound_head != world.enum_id {
        return None;
    }
    let mut family: Option<StringId> = None;
    for case in &ctx.cases {
        let mut leaves = Vec::new();
        enum_constant_leaves(&case.space, &mut leaves)?;
        for enum_name in leaves {
            match family {
                Some(found) if found != enum_name => return None,
                Some(_) => {}
                None => family = Some(enum_name),
            }
        }
    }
    family.map(Ty::named)
}

/// Collects the enum-constant leaves of one case root; `None` when any leaf
/// is not an enum constant (the family is then not witnessed by the cases
/// alone — the caller declines).
fn enum_constant_leaves(root: &Restriction, out: &mut Vec<StringId>) -> Option<()> {
    match root {
        Restriction::EnumValue(enum_name, _) => {
            out.push(*enum_name);
            Some(())
        }
        Restriction::Union(parts) => {
            for part in parts {
                enum_constant_leaves(part, out)?;
            }
            Some(())
        }
        _ => None,
    }
}

// —— records/patterns/exhaustiveness (M2 face) ————————————————————————————

/// The static type the semantic-level exhaustiveness walk uses: the M1
/// [`Ty`] lattice plus the restriction leaves the case patterns produce
/// (enum constants, `bool` values). Names resolve through the unit's
/// element model, which is file-scoped — the cross-unit-sealed blind spot
/// is no longer a silent under-report: an unresolvable scrutinee name
/// suppresses the expression-face covering emit outright
/// ([`NameWorld::name_resolvable`]), and a cyclic in-file sealed
/// hierarchy terminates through the walk's visited set.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum NameTy {
    Never,
    Null,
    /// `dynamic` — the nullable top.
    TopNullable,
    TopNonNullable,
    /// A named type as written, with the syntactic `?`. The nullable
    /// projection doubles as the engine's `Nullable` variant.
    Named(StringId, bool),
    /// A named type whose written type arguments the projection keeps
    /// ([`ArgListId`] — the semantic pool slot, canonical within one
    /// diagnosis). Bare `Named` collapses every instantiation of a head
    /// into one shape, which is the right posture for the exhaustiveness
    /// columns (a pattern never writes arguments) but folds the is-check
    /// verdicts over differing instantiations: `Stream<List<int>>` and
    /// `Stream<Uint8List>` must not compare equal at the fold. The
    /// subtype arms treat the bare head as the wildcard side (every
    /// instantiation fits a bare-head ask — the covariant fit the
    /// oracle's interface rule gives) and decide the pair over
    /// argument-carrying sides through the semantic subtype face
    /// ([`NameWorld::args_subtype`]).
    NamedArgs(StringId, bool, ArgListId),
    /// An enum constant: the enum's name and the constant's.
    EnumValue(StringId, StringId),
    BoolValue(bool),
    /// The nullable projection of an enum constant
    /// (`NullableStaticType(EnumElementStaticType)`, `static_type.dart:350`):
    /// the constant plus `null`, never its siblings.
    EnumValueOrNull(StringId, StringId),
    /// The nullable projection of a `true`/`false` value
    /// (`NullableStaticType(_BoolValueStaticType)`, `static_type.dart:350`):
    /// the value plus `null`, never the other boolean.
    BoolOrNull(bool),
    /// A list-pattern static type (`ListPatternStaticType` over
    /// `ListTypeRestriction`, types/list.dart:124-313): the underlying
    /// `List` element type, the fixed element count (head + tail), and
    /// the rest flag — `size == 0 && has_rest` is the unrestricted
    /// `List<E>` face (`isUnrestricted`, list.dart:264-267).
    ListPat {
        /// The element type the list's slots read.
        elem: Ty,
        /// Fixed elements the pattern pins (head + tail).
        size: u32,
        /// A rest element (`...`) covers the middle.
        has_rest: bool,
    },
    /// A unique unknown (relational patterns, unrepresentable constants):
    /// matched by nothing.
    Unknown(u64),
}

/// One legacy-era (`< 3.0`, patterns disabled) statement-case head's
/// evaluated head expression: the expression span and its const verdict —
/// the input the pre-patterns verifier's three per-head rows read
/// (`_validateSwitchStatement_nullSafety`'s primitive-equality gate, the
/// shared analyzer's type-mismatch gate, and the enum-constant coverage
/// walk — upstream Dart SDK source,
/// pkg/analyzer/lib/src/dart/constant/constant_verifier.dart:1145-1182 and
/// pkg/_fe_analyzer_shared/lib/src/type_inference/type_analyzer.dart,
/// `analyzeConstantPattern`'s `caseExpressionTypeMismatch` arm). `None`
/// verdicts — a non-constant head already reported at its own span — carry
/// no entry.
pub(crate) struct LegacyCase {
    /// The head expression's span (the per-head rows' anchor — upstream
    /// anchors `at(expression)`, the whole expression node).
    pub(crate) span: SourceRange,
    /// The evaluated head constant. `None` when the verdict was the
    /// opaque `Verdict::Constant` (a constant the subset does not fold):
    /// the enum coverage walk and the value-typed checks cannot read it,
    /// matching upstream's `expressionValue is! DartObjectImpl` skip.
    pub(crate) value: Option<crate::const_eval::ConstantValue>,
    /// The head referenced a deferred-import prefix: the identifier's own
    /// span (upstream's `errorTarget` — `evaluation.dart`'s
    /// `_getDeferredLibraryError` anchors the prefix). The
    /// `non_constant_case_expression_from_deferred_library` row already
    /// emitted at the head's close; the field marks the case so the
    /// switch-close pass runs no further checks on it.
    pub(crate) deferred: Option<SourceRange>,
}

/// One completed case: its coverage restriction, the guard flag, the
/// token the diagnostics anchor to (`case` for statement cases, `=>` for
/// expression cases), and — statement cases, last head of a member — the
/// member body's whitespace-trimmed span (the flow-dead member rows ride
/// it, `dead_code_verifier.dart`'s `flowEnd` member trim). `head_colon` is
/// the head's own `:` — the parser only reports the member colon at
/// `end_switch_case` for a `default` head (`colon_after_default`), so the
/// case-headed member's body boundary latches here.
#[derive(Clone)]
pub(crate) struct NameCase {
    pub(crate) space: Restriction,
    pub(crate) guarded: bool,
    pub(crate) anchor: SourceRange,
    /// The flow `MatchedType` fact's join key: the `case` keyword for a
    /// statement clause, the alternative's first token on the
    /// switch-expression path (the parser delivers no keyword face there,
    /// `end_switch_expression_case`'s `begin`) — distinct from `anchor`,
    /// the diagnostic's own token (`=>` for an alternative).
    pub(crate) flow_anchor: SourceRange,
    pub(crate) head_colon: Option<SourceRange>,
    pub(crate) body: Option<SourceRange>,
}

/// The open switch (statement or expression): the scrutinee's push-time
/// type, its keyword anchor, and the cases completed so far.
pub(crate) struct SwitchCtx {
    pub(crate) is_expression: bool,
    pub(crate) switch_kw: SourceRange,
    pub(crate) scrutinee: Ty,
    pub(crate) cases: Vec<NameCase>,
    /// The `default` clause's keyword anchor (the unreachable-default row's
    /// token, `constant_verifier.dart:1141-1148`); `None` when the switch
    /// has no default.
    pub(crate) default_kw: Option<SourceRange>,
    /// The captured coverage of an in-flight switch-expression case (the
    /// space latches at the pattern's completion, the guard and anchor at
    /// the case's end). Statement cases capture in one step and leave this
    /// `None`.
    pub(crate) pending_root: Option<Restriction>,
    /// An if-case's MEMBER scrutinee row (`transformer?.value case …`):
    /// the chain-head row's span, set only when the scrutinee row is a
    /// member access. The null-check binder gate re-derives the chain's
    /// accessor face from the recorded member read at exactly this span;
    /// a bare-variable scrutinee (whatever its latched type) never
    /// re-derives — the nearest-earlier member read would be a stale,
    /// unrelated face.
    pub(crate) if_case_member_scrutinee: Option<SourceRange>,
    /// A case pattern's type head resolved to `InvalidType` (an
    /// unresolvable or non-type name): the official `hasInvalidType` gate
    /// (constant_verifier.dart:1070, set by `createStaticType`) skips the
    /// whole reachability/covering pass.
    pub(crate) has_invalid_type: bool,
    /// The legacy-era head facts: one entry per completed statement-case
    /// head while `patterns_enabled` is off ([`LegacyCase`]). Separate
    /// from `cases` because a head's evaluated constant is not name-level
    /// coverage — the space model would have no leaf for it. Empty under
    /// patterns (the constant window never opens) and popped unread with
    /// the context either way.
    pub(crate) legacy_cases: Vec<LegacyCase>,
}

/// One open `begin_pattern`..`end_pattern` bracket: the leaf restrictions
/// collected inside it, whether `||` joined any of them (an `||` union
/// keeps its alternatives apart; everything else intersects), and the
/// object-pattern field buckets the `handle_pattern_field` boundaries
/// drained into it (the property columns of the official object-pattern
/// space — frame-scoped, so a record pattern's unused buckets die with its
/// own bracket).
pub(crate) struct PatternFrame {
    pub(crate) leaves: Vec<Restriction>,
    pub(crate) or_ops: usize,
    pub(crate) fields: Vec<ObjectField>,
    /// The frame is a logical-or/and operand (`Some(is_or)`): its matched
    /// type is still the switch scrutinee, so constant leaves under it
    /// check against the same space as root leaves (field sub-pattern
    /// frames are `None` — their matched type is the property column,
    /// which the name level does not descend into).
    pub(crate) binary: Option<bool>,
    /// The frame's wildcard events `(anchor, declared type)`, pending the
    /// `&&` that would make them the left operand of a logical-and (the
    /// operator event follows the left operand, so its binary context
    /// cannot arm the wildcard's own check — see
    /// [`Engine::check_pending_wildcards`]). Cleared once checked.
    pub(crate) wildcards: Vec<(SourceRange, Option<Ty>)>,
}

/// One object-pattern field's drained sub-restrictions (empty for a
/// type-free `var`/`_` sub-pattern, which covers the whole column).
pub(crate) struct ObjectField {
    /// The field's member name (`x` of `x: pattern`). `None` for
    /// positional fields — the member table keeps no declaration order, so
    /// their columns are unresolvable at name level.
    pub(crate) name: Option<StringId>,
    pub(crate) leaves: Vec<Restriction>,
    /// The getter the field reads and its anchor: the written name
    /// (`x: p`), or the shorthand's variable (`:var x`, `:x`) — the
    /// oracle's `resolveObjectPatternPropertyGet` name token. `None` for a
    /// positional field.
    pub(crate) getter: Option<(StringId, SourceRange)>,
    /// The shorthand's bound variable (`:var x` / `:x` binds `x`): the
    /// binder's name and declaration span, taken with the getter (the
    /// variable latch). `None` for written-name fields (`x: p` — the
    /// sub-pattern's own binder is not the getter) and positional fields.
    pub(crate) binder: Option<(StringId, SourceRange)>,
    /// The shorthand's null-check suffix (`:final x?`): the oracle binds
    /// `x` to the property type's NON-NULL projection (the null-check
    /// arm's promotion applies to the bound variable, official
    /// `_handleNullCheckPattern`), not the raw property face.
    pub(crate) binder_non_null: bool,
}

/// The name-level coverage a pattern leaf contributes (the space-building
/// face of `PatternConverter`, constant_verifier.dart:1031-1050).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Restriction {
    /// A type test as written (`X`, `X?`): cast patterns, typed variable
    /// patterns, object patterns.
    Named(StringId, bool),
    /// A type test with its WRITTEN arguments (`List<int>`, `Map<K, V>`):
    /// the case space keeps the instantiation, so two same-head cases with
    /// distinct arguments never cover one another (invariant generics —
    /// neither instantiation is a subtype of the other; the official
    /// `convertTypePattern` lowers the resolved type with its arguments).
    NamedArgs(StringId, bool, ArgListId),
    /// The `null` constant (the only value-precise constant at name level).
    Null,
    /// `true`/`false`.
    BoolValue(bool),
    /// A qualified enum constant `E.c`.
    EnumValue(StringId, StringId),
    /// Alternatives (`a || b`).
    Union(Vec<Restriction>),
    /// The type-free coverage (`var x`, `_`): substituted with the
    /// scrutinee's context type when the case closes.
    NamedContext,
    /// An object pattern's head with its per-field REFINING sub-restrictions
    /// (`A(d: null)`, `A(:final e?)`): each carried field lowers into the
    /// single's property sub-space, so a null-split pair of cases over one
    /// head covers it exactly (the official `convertObjectPattern`'s field
    /// columns, space_builder.dart:146-176). A field whose leaf covers its
    /// whole column contributes nothing; a constraint the name level cannot
    /// express degrades the whole pattern to [`Restriction::Unknown`] at
    /// the close.
    Object(StringId, Vec<(StringId, Restriction)>),
    /// The rest-element marker a `...` fires inside its own bracket
    /// (`handle_rest_pattern`): the collapse rebuilds the list pattern's
    /// head/rest/tail split from it (`createListSpace`,
    /// shared.dart:832-911). `had_sub` records whether the rest carried a
    /// sub-pattern (`...p` vs a bare `...`), which decides whether the leaf
    /// before the mark is the rest's sub-pattern or the last head element.
    /// Never survives the frame — [`SwitchTracker::collapse`] either folds
    /// it into [`Restriction::List`] (a list frame) or drops the whole
    /// pattern to [`Restriction::Unknown`] (every other bracket).
    RestMark { had_sub: bool },
    /// A list pattern's coverage: `head` fixed elements, an optional rest
    /// (`Bare` `...` or `Sub` `...p`), and `tail` fixed elements after it
    /// (`ListPatternStaticType` over `ListTypeRestriction`,
    /// types/list.dart:124-313, built by `createListSpace`,
    /// shared.dart:832-911). `elem` is the resolved element type — the
    /// scrutinee's `List<E>` argument when the matched type supplies one,
    /// else `Object?` (the official's `elementType ?? objectQuestionType`,
    /// list_pattern_resolver.dart:40-45). `list` is the `List` head's name
    /// — the restriction's static type rides on `List<elem>`.
    List {
        /// The `List` container spelling (`dart:core`'s, unless shadowed).
        list: StringId,
        /// The resolved element type.
        elem: Ty,
        /// The fixed head elements' restrictions.
        head: Vec<Restriction>,
        /// The rest element: `None` — no `...`; `Some(None)` — a bare
        /// `...`; `Some(Some(sub))` — `...sub`.
        rest: Option<Option<Box<Restriction>>>,
        /// The fixed tail elements' restrictions.
        tail: Vec<Restriction>,
    },
    /// The unknown space: relational patterns, map patterns, and
    /// constants the name level cannot represent. Never must-matches, so
    /// the engine under-reports instead of mis-answering.
    Unknown,
}

/// The name-level type a leaf denotes, for the leaves whose coverage IS one
/// type test (upstream `_BoolValueStaticType`/`EnumElementStaticType`
/// intersect to themselves against their head type). `None` for the shapes
/// that carry no type of their own (`Null` stays a leaf: the `p!`
/// union-merge owns its coverage whole).
fn leaf_ty(leaf: &Restriction) -> Option<NameTy> {
    match *leaf {
        Restriction::Named(name, nullable) => Some(NameTy::Named(name, nullable)),
        Restriction::NamedArgs(name, nullable, args) => {
            Some(NameTy::NamedArgs(name, nullable, args))
        }
        Restriction::BoolValue(value) => Some(NameTy::BoolValue(value)),
        Restriction::EnumValue(enum_name, constant) => Some(NameTy::EnumValue(enum_name, constant)),
        _ => None,
    }
}

impl Restriction {
    /// `&&`-conjunction at name level: the single side when both spell the
    /// same test, unknown otherwise (an incomparable conjunction's
    /// coverage is not one row type). By value because
    /// `reduce(Restriction::and)` consumes the frame's leaves.
    #[allow(clippy::needless_pass_by_value)]
    pub(crate) fn and(self, other: Restriction) -> Restriction {
        if self == Restriction::Unknown || other == Restriction::Unknown {
            return Restriction::Unknown;
        }
        if self == other {
            return self;
        }
        Restriction::Unknown
    }

    /// `&&`-conjunction with the subtype lattice: two type tests intersect
    /// to the narrower one when one subtypes the other, and a type-free
    /// operand (`var x`, `_`: the context's full space) is the identity
    /// (official `_createSingleSpaceIntersection`, upstream Dart SDK source,
    /// pkg/_fe_analyzer_shared/lib/src/exhaustiveness/shared.dart:975-1009).
    /// Every other pair keeps [`Self::and`]'s answer — save the
    /// postfix-wrap pair: a `Union` tail ending in `Null` is the
    /// null-assert leaf's shape (`p!` = the subpattern restricted to
    /// non-null UNION the whole `Null` space, `createNullAssertSpace`,
    /// shared.dart:790-797). It is not a `&&` operand to intersect — the
    /// `!` *replaces* the subpattern's own coverage: `P ∩ scrut-nonNull`
    /// (the narrowed left side) joined by the preserved `Null` half, so
    /// `BoolET _!` over `bool?` covers `bool|Null` whole and `true!`
    /// leaves `false` uncovered. A leaf `Null` inside a union a user
    /// wrote (`x || null`) cannot reach this arm with a trailing position
    /// — the `||` collapse owns the in-frame union build and postfix
    /// wraps are the only sibling-leaf merges that ever see it.
    pub(crate) fn and_in(self, world: &NameWorld<'_>, other: Restriction) -> Restriction {
        match (&self, &other) {
            (Restriction::NamedContext, _) => other,
            (_, Restriction::NamedContext) => self,
            // `p` then `!` (either order in the leaf list): the union's
            // non-null half intersects the subpattern, its `Null` half
            // stays whole — `createNullAssertSpace`'s union verbatim.
            (Restriction::Union(parts), leaf) | (leaf, Restriction::Union(parts))
                if matches!(parts.last(), Some(Restriction::Null)) =>
            {
                let mut kept = Vec::with_capacity(parts.len());
                for part in &parts[..parts.len() - 1] {
                    let narrowed = leaf.clone().and_in(world, part.clone());
                    if narrowed != Restriction::Unknown {
                        kept.push(narrowed);
                    }
                }
                kept.push(Restriction::Null);
                Restriction::Union(kept)
            }
            _ => match (leaf_ty(&self), leaf_ty(&other)) {
                (Some(a), Some(b)) if world.is_subtype_of(&a, &b) => self,
                (Some(a), Some(b)) if world.is_subtype_of(&b, &a) => other,
                _ => self.and(other),
            },
        }
    }
}

/// Builds one case's engine space from its restriction over `world`.
pub(crate) fn case_space(
    world: &NameWorld<'_>,
    space: &Restriction,
    context: NameTy,
) -> Space<NameTy> {
    let mut singles = Vec::new();
    collect_singles(world, space, context, &mut singles);
    Space::from_singles(world, Vec::new(), singles)
}

/// Flattens a restriction into the union's single spaces.
fn collect_singles(
    world: &NameWorld<'_>,
    space: &Restriction,
    context: NameTy,
    out: &mut Vec<Single<NameTy>>,
) {
    match space {
        Restriction::Union(parts) => {
            for part in parts {
                collect_singles(world, part, context, out);
            }
        }
        Restriction::Unknown => {
            out.push(Single::full(world.unknown()));
        }
        Restriction::Null => out.push(Single::full(NameTy::Null)),
        // Typed leaves (`X`, `X?`, `E.c`, `true`/`false`): one type-test
        // single (official `convertTypePattern`/`convertConstantPattern`).
        Restriction::Named(name, nullable) => {
            // A written `Null` type test IS the `Null` static type
            // (static_type.dart:21), so `Null _` and the `null` constant
            // cover one space (the subsumption pins of #3518).
            out.push(Single::full(
                world.fold_written_null(&NameTy::Named(*name, *nullable)),
            ));
        }
        Restriction::NamedArgs(name, nullable, args) => {
            out.push(Single::full(NameTy::NamedArgs(*name, *nullable, *args)));
        }
        Restriction::BoolValue(value) => out.push(Single::full(NameTy::BoolValue(*value))),
        Restriction::EnumValue(enum_name, constant) => {
            out.push(Single::full(NameTy::EnumValue(*enum_name, *constant)));
        }
        // Substituted before the case is recorded; unreachable here.
        Restriction::NamedContext => out.push(Single::full(context)),
        Restriction::Object(name, fields) => {
            let head = NameTy::Named(*name, false);
            let mut props = Vec::with_capacity(fields.len());
            for (field, rest) in fields {
                let mut singles = Vec::new();
                collect_singles(world, rest, world.unknown(), &mut singles);
                let key = Key::Name(world.strings.resolve(*field).to_string());
                let space = Space::from_singles(world, Vec::new(), singles);
                props.push((key, space));
            }
            out.push(Single::with_props(head, props));
        }
        // A list pattern's single: `ListPatternStaticType` over the
        // head+tail size and rest flag, with the `HeadKey`/`RestKey`/
        // `TailKey` additional-property columns `createListSpace` builds
        // (shared.dart:865-905). `TailKey(i)` is the `i`th-last element —
        // `tail[tail.len() - 1 - i]`.
        Restriction::List {
            elem,
            head,
            rest,
            tail,
            ..
        } => {
            let ty = NameTy::ListPat {
                elem: *elem,
                size: u32::try_from(head.len() + tail.len()).unwrap_or(u32::MAX),
                has_rest: rest.is_some(),
            };
            // The head/tail columns type as the element (`HeadKey`/`TailKey`
            // → `getListElementType`), the rest column as the `List<E>`
            // face itself (`RestKey` → `getListType`,
            // shared.dart:402-411). A type-free element leaf
            // (`var r1`, `_`) is that column's full space.
            let column = NameTy::from_semantic(*elem);
            let rest_column = world.list_ty(*elem);
            let mut additional =
                Vec::with_capacity(head.len() + tail.len() + usize::from(rest.is_some()));
            for (index, leaf) in head.iter().enumerate() {
                let mut singles = Vec::new();
                collect_singles(world, leaf, column, &mut singles);
                additional.push((
                    Key::Head(index),
                    Space::from_singles(world, Vec::new(), singles),
                ));
            }
            if let Some(sub) = rest {
                let space = match sub {
                    Some(sub) => {
                        let mut singles = Vec::new();
                        collect_singles(world, sub, rest_column, &mut singles);
                        Space::from_singles(world, Vec::new(), singles)
                    }
                    // A bare `...` covers its whole `List<E>` column
                    // (`staticType.getAdditionalPropertyType(RestKey)`,
                    // shared.dart:888-892).
                    None => Space::full(rest_column),
                };
                additional.push((
                    Key::Rest {
                        head: head.len(),
                        tail: tail.len(),
                    },
                    space,
                ));
            }
            for index in 0..tail.len() {
                let leaf = &tail[tail.len() - index - 1];
                let mut singles = Vec::new();
                collect_singles(world, leaf, column, &mut singles);
                additional.push((
                    Key::Tail(index),
                    Space::from_singles(world, Vec::new(), singles),
                ));
            }
            out.push(Single::with_additional(ty, additional));
        }
        // The rest mark never survives `collapse` (a non-list frame drops
        // the whole pattern to `Unknown` before this point).
        Restriction::RestMark { .. } => {
            out.push(Single::full(world.unknown()));
        }
    }
}

/// The name-level [`ExhaustivenessWorld`]: sealed/enum facts come from the
/// element model, subtype edges from the superclass and
/// `with`/`implements` names of the containers, plus the fixed core chain
/// (`int`/`double` <: `num` <: `Object`) the nullability codes already
/// assume.
///
/// Names resolve file-local first (the unchanged fast path), then — when
/// the world carries a [`NameScope`] — through the library graph: the
/// diagnosing library's namespace (imports, exports, combinators), then a
/// graph-wide unique-declaration fallback for the hops a foreign container
/// writes in its own library's scope (`ChainB255 extends ChainB254`, where
/// `ChainB254` is imported by the declaring library, not the diagnosing
/// one). An ambiguous or absent name stays unresolved — the walks treat it
/// as a cut (undecided), the under-report direction. A sealed or enum
/// root's closed subclass set is enumerated over its own library's units
/// (the language confines a sealed class's direct subtypes to its
/// library), in unit then declaration order — the oracle's witness order.
pub(crate) struct NameWorld<'a> {
    pub(crate) index: &'a ElementIndex,
    pub(crate) file: FileId,
    pub(crate) strings: &'a StringTable,
    /// The library-graph face of the cross-file resolution; `None` keeps
    /// the world file-scoped (the unit-test rigs that build a bare world).
    pub(crate) scope: Option<NameScope<'a>>,
    /// Per-world memo of the cross-file resolutions (file-local hits never
    /// enter it). One world serves one verdict, so the memo is bounded by
    /// the names that verdict touches.
    pub(crate) resolved: RefCell<FxHashMap<StringId, Option<(&'a Container, FileId)>>>,
    /// The interned `bool` spelling (`BoolValue` subtyping compares ids,
    /// never re-interns per query).
    pub(crate) bool_id: StringId,
    /// The interned `Object` spelling: the implicit superclass edge every
    /// interface type carries without a written clause
    /// ([`Self::decide_subtype`]).
    pub(crate) object_id: StringId,
    /// The interned dart:core `Enum` spelling: the implicit supertype edge
    /// every enum declaration carries without a written clause
    /// ([`Self::direct_edges`]; the type-hierarchy side synthesizes the
    /// same edge, `DeclAction::Enum` — #2370).
    pub(crate) enum_id: StringId,
    /// The core primitives' interned names
    /// ([`crate::member_face::FINAL_PRIMITIVES`] → ids): their `Object`
    /// edge is invisible to the written-clause walk (`int`/`double`/`num`
    /// reach it through [`Self::core_supertypes`]; `bool`/`String` only
    /// through this set).
    pub(crate) primitive_ids: [StringId; 5],
    /// The core chain pre-interned ([`CORE_SUPERTYPES`] → id pairs): the
    /// BFS compares ids, never re-resolving/re-interning per dequeued node.
    pub(crate) core_supertypes: [(StringId, StringId); 3],
    pub(crate) unknowns: std::cell::Cell<u64>,
}

/// The library-graph face a [`NameWorld`] resolves cross-file names
/// through: the graph, its namespaces, and the diagnosing library.
#[derive(Clone, Copy)]
pub(crate) struct NameScope<'a> {
    pub(crate) graph: &'a LibraryGraph,
    pub(crate) namespaces: &'a Namespaces,
    pub(crate) lib: LibraryId,
}

/// The two core edges every unit sees without a declaration (`num` has no
/// container; the nullability face already treats these names specially).
pub(crate) const CORE_SUPERTYPES: [(&str, &str); 3] =
    [("int", "num"), ("double", "num"), ("num", "Object")];

impl NameTy {
    pub(crate) fn from_semantic(ty: Ty) -> Self {
        match ty {
            Ty::Unknown | Ty::Dynamic | Ty::Void => NameTy::TopNullable,
            Ty::Null => NameTy::Null,
            // The throw expression's bottom type joins the world as the
            // exhaustiveness `Never` (`switch (throw e)` matches
            // everything).
            Ty::Never => NameTy::Never,
            // The S0 side channel keeps its name-level shape; the argument
            // slot rides along by pool id (`NamedArgs`) when written — the
            // is-check folds compare instantiations, the exhaustiveness
            // columns never write arguments and keep the bare head.
            Ty::Named {
                name,
                nullable,
                args: ArgListId::EMPTY,
            } => NameTy::Named(name, nullable),
            Ty::Named {
                name,
                nullable,
                args,
            } => NameTy::NamedArgs(name, nullable, args),
            // Record and function types have no name-level decomposition
            // (the columns live in the `Ty` shape, not here): a sentinel
            // keyed by the canonically-interned shape id keeps equal
            // spellings self-equal (`void Function()` covers itself)
            // while different shapes land on different sentinels — the
            // shared-unknown equality must not fire across two different
            // types, where it reports a false constant-true
            // `unnecessary_type_check` (a `FutureOr<R> Function(T)`
            // operand checked `is R Function(T)` — drift async.dart).
            // The tag separates the two shape-id pools and lifts the key
            // above the fresh-`unknown()` counter's range, so a keyed
            // sentinel never collides with a minted one. The nullability
            // face rides the key: an `F`-vs-`F?` pair stays unprovable at
            // this level (the under-report direction — never a false
            // constant verdict).
            Ty::Record { shape, nullable } => {
                NameTy::Unknown((1 << 41) | (u64::from(shape.raw()) << 1) | u64::from(nullable))
            }
            Ty::Function { shape, nullable } => {
                NameTy::Unknown((2 << 41) | (u64::from(shape.raw()) << 1) | u64::from(nullable))
            }
        }
    }
}

// The parse-time engine stack ([`push_fold_engine`]): the argument-aware
// subtype arm ([`NameWorld::args_subtype`]) re-asks the semantic subtype
// face through it, because the world carries only the long-lived
// `&'a`-borrowed facts while the boundary pool — where `ArgListId`s
// resolve — lives inside the engine owned by the fold's own frame.
//
// Raw addresses, never dereferenced outside a push window: the window
// opens after the fold's listener is built and closes (guard drop) before
// that listener is destructured, so no stored address can outlive its
// owner. Re-entrant folds stack; a query reads the topmost window, which
// is the engine whose verdict is being computed. Faces that run outside
// any window (the teardown's column asks, the bare-world rigs) find an
// empty stack and keep the pre-#2462 answer — the collapse, never a
// panic.
thread_local! {
    static FOLD_ENGINES: RefCell<Vec<*const Engine<'static>>> =
        RefCell::new(Vec::new());
}

/// Opens a parse-time window for `engine` ([`FOLD_ENGINES`]); the returned
/// guard closes it. The erasure to `Engine<'static>` is address-only — the
/// pointee is used exclusively between this call and the guard's drop,
/// inside the caller's frame that owns the engine.
pub(crate) fn push_fold_engine(engine: &Engine<'_>) -> FoldEngineGuard {
    FOLD_ENGINES.with(|stack| {
        stack
            .borrow_mut()
            .push(engine as *const Engine<'_> as *const Engine<'static>);
    });
    FoldEngineGuard
}

/// The window guard ([`push_fold_engine`]).
pub(crate) struct FoldEngineGuard;

impl Drop for FoldEngineGuard {
    fn drop(&mut self) {
        FOLD_ENGINES.with(|stack| {
            stack.borrow_mut().pop();
        });
    }
}

/// The topmost window's engine, when one is open.
fn fold_engine() -> Option<&'static Engine<'static>> {
    FOLD_ENGINES.with(|stack| stack.borrow().last().copied().map(|ptr| unsafe { &*ptr }))
}

impl ExhaustivenessWorld for NameWorld<'_> {
    type Ty = NameTy;

    fn object_nullable(&self) -> NameTy {
        NameTy::TopNullable
    }

    fn never(&self) -> NameTy {
        NameTy::Never
    }

    fn null(&self) -> NameTy {
        NameTy::Null
    }

    fn unknown(&self) -> NameTy {
        let next = self.unknowns.get();
        self.unknowns.set(next.wrapping_add(1));
        NameTy::Unknown(next)
    }

    fn nullable(&self, ty: &NameTy) -> NameTy {
        match ty {
            // The nullable projection of a name.
            NameTy::Named(name, _) => NameTy::Named(*name, true),
            NameTy::NamedArgs(name, _, args) => NameTy::NamedArgs(*name, true, *args),
            // A restricted value's projection keeps the restriction
            // (`NullableStaticType` over the value static type,
            // static_type.dart:350-383, `NonNullableStaticType.nullable`):
            // it covers the value and `null`, never the siblings.
            NameTy::EnumValue(enum_name, constant) => {
                NameTy::EnumValueOrNull(*enum_name, *constant)
            }
            NameTy::BoolValue(value) => NameTy::BoolOrNull(*value),
            NameTy::TopNonNullable => NameTy::TopNullable,
            other => *other,
        }
    }

    // A type lattice: several arms legitimately answer plain `true`.
    #[allow(clippy::match_same_arms)]
    fn is_subtype_of(&self, sub: &NameTy, sup: &NameTy) -> bool {
        let sub = &self.fold_written_null(&self.extension_erasure(&self.fold_written_never(sub)));
        let sup = &self.fold_written_null(&self.extension_erasure(&self.fold_written_never(sup)));
        if sub == sup {
            return true;
        }
        match (sub, sup) {
            // Everything subtypes the nullable top (static_type.dart:166);
            // `Never` subtypes everything; `Null <: T?` — through the
            // restricted-value nullable projections too
            // (`NullableStaticType.isSubtypeOf`, the underlying chain ends
            // in `Never`).
            (_, NameTy::TopNullable)
            | (NameTy::Never, _)
            | (
                NameTy::Null,
                NameTy::Named(_, true)
                | NameTy::NamedArgs(_, true, _)
                | NameTy::EnumValueOrNull(..)
                | NameTy::BoolOrNull(_),
            ) => true,
            // `S <: T?` iff `S <: T` (static_type.dart:365-372); `S? <: T?`
            // likewise; `S? <: T` never (null matches no plain T).
            (NameTy::Named(sn, s), NameTy::Named(tn, t)) => (!*s || *t) && self.named_sub(*sn, *tn),
            // An argument-carrying pair decides through the semantic
            // subtype face ([`Self::args_subtype`]) — the interface rule's
            // pairwise argument ask (upstream Dart SDK source,
            // pkg/analyzer/lib/src/dart/element/subtype.dart, the interface
            // type case: same declaration answers through the type
            // argument list, distinct heads through the supertype graph).
            // A bare side stays the wildcard it always was: the
            // instantiate-to-bound head fits any instantiation covariantly,
            // so the declared-edge walk alone answers (the pre-#2462
            // collapse answer, byte-identical).
            (NameTy::NamedArgs(sn, s, sa), NameTy::NamedArgs(tn, t, ta)) => {
                (!*s || *t) && self.args_subtype(*sn, *s, *sa, *tn, *t, *ta)
            }
            (NameTy::NamedArgs(sn, s, _), NameTy::Named(tn, t))
            | (NameTy::Named(sn, s), NameTy::NamedArgs(tn, t, _)) => {
                (!*s || *t) && self.named_sub(*sn, *tn)
            }
            // The bare value fits its own nullable projection (the
            // must-match rule: `true <: true?` through the identical
            // restriction, types.dart:52-56).
            (NameTy::EnumValue(en, c), NameTy::EnumValueOrNull(fn_, d)) => en == fn_ && c == d,
            (NameTy::BoolValue(v), NameTy::BoolOrNull(w)) => v == w,
            // All non-nullable types are subtypes of Object
            // (`NonNullableStaticType.isSubtypeOf`, static_type.dart
            // :415-421).
            (
                NameTy::Named(_, false)
                | NameTy::NamedArgs(_, false, _)
                | NameTy::EnumValue(..)
                | NameTy::BoolValue(_)
                | NameTy::ListPat { .. },
                NameTy::TopNonNullable,
            ) => true,
            // A constant's type is its declared instantiation
            // (`enumField.type`, `EnumElementStaticType._type`): `E.b`
            // of `enum E<T> { b<String>(…) }` is `E<String>`, not the
            // bare head — an `E<num>` type-test case cannot cover it.
            // [`Self::enum_constant_ty`] carries the instantiation;
            // `UNKNOWN`/foreign rows keep the bare head.
            (NameTy::EnumValue(en, constant), sup) => {
                self.is_subtype_of(&self.enum_constant_ty(*en, *constant), sup)
            }
            // The restricted nullable inherits the value's type edges, but
            // only toward nullable targets (its `null` half never fits a
            // non-nullable type).
            (
                NameTy::EnumValueOrNull(en, constant),
                NameTy::Named(tn, true) | NameTy::NamedArgs(tn, true, _),
            ) => {
                let declared = self.enum_constant_ty(*en, *constant);
                self.is_subtype_of(&declared, &NameTy::Named(*tn, true))
            }
            (NameTy::BoolOrNull(_), NameTy::Named(tn, true)) => self.is_subtype_of(
                &NameTy::Named(self.bool_id, false),
                &NameTy::Named(*tn, true),
            ),
            (NameTy::BoolValue(_), sup) => {
                self.is_subtype_of(&NameTy::Named(self.bool_id, false), sup)
            }
            (NameTy::TopNonNullable, NameTy::TopNonNullable) => true,
            // The non-nullable top IS `Object` (static_type.dart's
            // `nonNullableObject`): it fits the `Object`/`Object?` name
            // exactly as every non-nullable name fits it.
            (NameTy::TopNonNullable, NameTy::Named(tn, _) | NameTy::NamedArgs(tn, _, _)) => {
                *tn == self.object_id
            }
            // `ListTypeRestriction.isSubtypeOf` (types/list.dart:279-292):
            // a restricted pair compares element types then the size/rest
            // shape.
            (
                NameTy::ListPat {
                    elem: se,
                    size: ss,
                    has_rest: sr,
                },
                NameTy::ListPat {
                    elem: te,
                    size: ts,
                    has_rest: tr,
                },
            ) => {
                // The unrestricted target absorbs (`other.isUnrestricted`
                // first, list.dart:280): `[...]` covers every list.
                (*tr && *ts == 0)
                    || (self.elem_sub(*se, *te)
                        && if *tr { ss >= ts } else if *sr { false } else { ss == ts })
            }
            // A list pattern type is still a `List<E>` at the interface
            // face (`TypeBasedStaticType.isSubtypeOfInternal` — the
            // restriction is a subtype of every UNRESTRICTED target,
            // types.dart:224-241): the pair decides as `List<elem> <: sup`.
            (NameTy::ListPat { elem, .. }, NameTy::Named(..) | NameTy::NamedArgs(..)) => {
                self.is_subtype_of(&self.list_ty(*elem), sup)
            }
            // Any non-list single against a list pattern: the target's
            // restriction only absorbs when it is itself unrestricted
            // (`has_rest && size == 0` IS `List<E>` — `[...]`); every
            // restricted target rejects an unrestricted source
            // (types.dart:236-241).
            (sub, NameTy::ListPat {
                elem,
                size: 0,
                has_rest: true,
            }) => self.is_subtype_of(sub, &self.list_ty(*elem)),
            (_, NameTy::ListPat { .. }) => false,
            _ => false,
        }
    }

    fn is_sealed(&self, ty: &NameTy) -> bool {
        match &self.extension_erasure(ty) {
            // Nullable projections split (static_type.dart:356).
            NameTy::Named(_, true)
            | NameTy::NamedArgs(_, true, _)
            | NameTy::TopNullable
            | NameTy::EnumValueOrNull(..)
            | NameTy::BoolOrNull(_) => true,
            // `ListTypeStaticType.isSealed` (types/list.dart:62): the core
            // `List` face splits over the size buckets. The restricted
            // `ListPat` face inherits `TypeBasedStaticType.isSealed` —
            // `false` (types/list.dart:124-135, list.dart:59).
            NameTy::Named(name, false) | NameTy::NamedArgs(name, false, _)
                if *name == self.list_head() =>
            {
                true
            }
            NameTy::Named(name, false) | NameTy::NamedArgs(name, false, _) => {
                self.is_sealed_name(*name)
            }
            _ => false,
        }
    }

    fn subtypes(&self, ty: &NameTy, keys: &rustc_hash::FxHashSet<Key>) -> Vec<NameTy> {
        let erased = self.extension_erasure(ty);
        match &erased {
            NameTy::Named(name, true) => vec![NameTy::Named(*name, false), NameTy::Null],
            NameTy::NamedArgs(name, true, args) => {
                vec![NameTy::NamedArgs(*name, false, *args), NameTy::Null]
            }
            NameTy::TopNullable => vec![NameTy::TopNonNullable, NameTy::Null],
            // NullableStaticType.getSubtypes: the value plus `null`
            // (static_type.dart:359-362).
            NameTy::EnumValueOrNull(enum_name, constant) => {
                vec![NameTy::EnumValue(*enum_name, *constant), NameTy::Null]
            }
            NameTy::BoolOrNull(value) => vec![NameTy::BoolValue(*value), NameTy::Null],
            // `ListTypeStaticType.getSubtypes` (types/list.dart:65-121):
            // the case rows' head/tail keys bound the fixed-size split —
            // sizes `0..maxSize-1` then `size=maxSize, hasRest` — over the
            // scrutinee's own element type.
            NameTy::Named(name, false) | NameTy::NamedArgs(name, false, _)
                if *name == self.list_head() =>
            {
                let mut max_head = 0usize;
                let mut max_tail = 0usize;
                for key in keys {
                    match *key {
                        Key::Head(index) => max_head = max_head.max(index + 1),
                        Key::Tail(index) => max_tail = max_tail.max(index + 1),
                        _ => {}
                    }
                }
                let max_size = max_head + max_tail;
                let elem = self.list_elem_of(&erased);
                (0..max_size)
                    .map(|size| NameTy::ListPat {
                        elem,
                        size: u32::try_from(size).unwrap_or(u32::MAX),
                        has_rest: false,
                    })
                    .chain(std::iter::once(NameTy::ListPat {
                        elem,
                        size: u32::try_from(max_size).unwrap_or(u32::MAX),
                        has_rest: true,
                    }))
                    .collect()
            }
            NameTy::Named(name, false) | NameTy::NamedArgs(name, false, _) => {
                self.name_subtypes(*name, &erased)
            }
            _ => Vec::new(),
        }
    }

    /// `getPropertyType` (static_type.dart:36-46): the declared type of
    /// the instance member `key` names, or `None` when the world does not
    /// know it (the checker's columns then fall back to the nullable
    /// object, exhaustive.dart:396-406).
    fn property_type(&self, ty: &NameTy, key: &Key) -> Option<NameTy> {
        let (NameTy::Named(name, _) | NameTy::NamedArgs(name, _, _)) = &self.extension_erasure(ty)
        else {
            return None;
        };
        let Key::Name(member) = key else {
            // Record keys are not modeled at name level.
            return None;
        };
        self.member_column(*name, self.strings.intern(member))
    }

    /// `getAdditionalPropertyType` → `getAdditionalFieldType`
    /// (shared.dart:396-416): a head/tail slot reads the list's element
    /// type, the rest slot reads the `List<E>` face itself, a map key
    /// reads the map's value type — `None` (the nullable-object
    /// fallback) for every other key and for a `ty` that cannot answer
    /// (`getListElementType`/`getMapValueType` decline).
    fn additional_property_type(&self, ty: &NameTy, key: &Key) -> Option<NameTy> {
        let ty = &self.extension_erasure(ty);
        match key {
            Key::Head(_) | Key::Tail(_) => match ty {
                NameTy::Named(name, false) | NameTy::NamedArgs(name, false, _)
                    if *name == self.list_head() =>
                {
                    Some(NameTy::from_semantic(self.list_elem_of(ty)))
                }
                // The pattern face answers through its own element —
                // a `HeadKey`/`TailKey` column under a `ListPat` single
                // (a nested list element's own slots, or the same
                // column re-asked) reads the restriction's element.
                NameTy::ListPat { elem, .. } => Some(NameTy::from_semantic(*elem)),
                _ => None,
            },
            Key::Rest { .. } => match ty {
                NameTy::Named(name, false) | NameTy::NamedArgs(name, false, _)
                    if *name == self.list_head() =>
                {
                    // The rest column IS the `List<E>` face (upstream
                    // `getListType` → `ListTypeStaticType` — the sealed
                    // list, not the unrestricted pattern face).
                    Some(ty.clone())
                }
                NameTy::ListPat { elem, .. } => Some(self.list_ty(*elem)),
                _ => None,
            },
            Key::MapKey(_) => None,
            _ => None,
        }
    }

    fn is_bottom(&self, ty: &NameTy) -> bool {
        matches!(ty, NameTy::Never)
    }

    fn is_null(&self, ty: &NameTy) -> bool {
        matches!(ty, NameTy::Null)
    }

    fn witness_text(&self, ty: &NameTy, fields: &[(Key, String)]) -> String {
        let ty = &self.extension_erasure(ty);
        // `ListPatternStaticType.witnessToDart` (types/list.dart:160-238):
        // the head/tail/rest fields lay out positionally, missing slots
        // render `_`, a short head pads to the restriction's size, and a
        // non-list field trails as `&& Object(k: w)` (mirroring the
        // record/object tail below).
        if let NameTy::ListPat { size, has_rest, .. } = ty {
            let mut max_head = 0usize;
            let mut max_tail = 0usize;
            for (key, _) in fields {
                match *key {
                    Key::Head(index) if index >= max_head => max_head = index + 1,
                    Key::Tail(index) if index >= max_tail => max_tail = index + 1,
                    _ => {}
                }
            }
            if max_head + max_tail < *size as usize {
                max_head = *size as usize - max_tail;
            }
            let field = |key: &Key| fields.iter().find(|(k, _)| k == key).map(|(_, t)| t.as_str());
            let mut text = String::from("[");
            let mut comma = "";
            for index in 0..max_head {
                text.push_str(comma);
                text.push_str(field(&Key::Head(index)).unwrap_or("_"));
                comma = ", ";
            }
            if *has_rest {
                text.push_str(comma);
                text.push_str("...");
                // A rest sub-witness renders after the `...`
                // (list.dart:200-203): the single `Rest` field, if any.
                if let Some(rest) = fields.iter().find(|(k, _)| matches!(k, Key::Rest { .. })) {
                    text.push_str(&rest.1);
                }
                comma = ", ";
            }
            for index in (0..max_tail).rev() {
                text.push_str(comma);
                text.push_str(field(&Key::Tail(index)).unwrap_or("_"));
                comma = ", ";
            }
            text.push(']');
            let mut start = " && Object(";
            let mut close = "";
            let mut inner_comma = "";
            for (key, field) in fields {
                if matches!(key, Key::Head(_) | Key::Rest { .. } | Key::Tail(_)) {
                    continue;
                }
                text.push_str(start);
                start = "";
                close = ")";
                text.push_str(inner_comma);
                inner_comma = ", ";
                text.push_str(&format!("{}: {field}", key.name_text()));
            }
            text.push_str(close);
            return text;
        }
        if fields.is_empty() {
            // The instance-shape witness always carries the
            // constructor-form parentheses, empty or not
            // (`_BaseStaticType.witnessToDart`, static_type.dart:214-239 —
            // `typeToDart` then `(` `)` even with no fields); the value
            // faces (`null`, `_`, `true`, `E.c`) render bare.
            return match ty {
                NameTy::Named(..) | NameTy::NamedArgs(..) | NameTy::TopNonNullable => {
                    format!("{}()", self.ty_text(ty))
                }
                _ => self.ty_text(ty),
            };
        }
        let inner = fields
            .iter()
            .map(|(key, text)| match key {
                Key::RecordIndex(index) => format!("${}:{text}", index + 1),
                Key::RecordName(name) | Key::Name(name) => format!("{name}: {text}"),
                // A list/map key trailing a non-list head renders through
                // its `name` (key.dart) the way `render_fields` does.
                other => format!("{}: {text}", other.name_text()),
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}({inner})", self.ty_text(ty))
    }

    fn display(&self, ty: &NameTy) -> String {
        self.ty_text(ty)
    }
}

impl<'a> NameWorld<'a> {
    /// The name-level `Never` fold: a written `Never` annotation reaches
    /// the world as `NameTy::Named` — the identifier's own spelling —
    /// while the canonical bottom the exhaustiveness lattice answers is
    /// [`NameTy::Never`] (the throw expression's bottom, the
    /// `object_nullable`/`never` factory). Without the fold a
    /// `Named(Never, false)` side falls through every bottom arm to a
    /// plain false, and the argument/operator verdicts that stage on this
    /// lattice re-report the pair the oracle's own `isSubtypeOf` accepts
    /// unconditionally (subtype.dart:299-301's `T1 <: T2` when `T1` is
    /// `Never`). Resolving the interned name keeps the fold honest — a
    /// user class literally named `Never` in its own file collides, but
    /// that spelling is already the declared-edge lattice's blind spot
    /// (a local `Never` shadows dart:core's for every name-level ask,
    /// not just this one).
    fn fold_written_never(&self, ty: &NameTy) -> NameTy {
        match ty {
            NameTy::Named(name, false) if self.strings.resolve(*name) == "Never" => NameTy::Never,
            other => *other,
        }
    }

    /// The written-`Null` fold, the sibling of [`Self::fold_written_never`]:
    /// `Null _` spells its type test as `Named("Null", false)`, while the
    /// canonical leaf the lattice (and the `null` constant pattern) speaks
    /// is [`NameTy::Null`]. Without the fold the two spellings cover
    /// different spaces, so `null` followed by `Null _` under-claims the
    /// subsumption (and the reverse order over-claims it). The same
    /// local-shadowing caveat as the `Never` fold applies.
    fn fold_written_null(&self, ty: &NameTy) -> NameTy {
        match ty {
            NameTy::Named(name, false) if self.strings.resolve(*name) == "Null" => NameTy::Null,
            other => *other,
        }
    }

    /// Whether some runtime value can inhabit both regions:
    /// `canBeSubtypeOf` (type_system.dart:138-323), the verified
    /// `TypeSystem::can_be_subtype_of` port mirrored at name level — the
    /// mutual-exclusion companion of [`ExhaustivenessWorld::is_subtype_of`].
    /// The Null arms decide before everything (type_system.dart:147-159),
    /// which is the pinned corner where the two relations legitimately
    /// disagree: `Never <: Null` (subtype.dart:299-301) yet no `Never`
    /// value can sit in a `Null` slot. Undecided shapes answer `true` (the
    /// official's conservative default, type_system.dart:322).
    ///
    /// The arm order mirrors the official body: `FutureOr` splits into
    /// `base`/`Future<base>` before any head comparison
    /// (type_system.dart:173-189), the same-declaration pair compares its
    /// type argument lists pairwise (:201-234 — the pre-#3236 same-head
    /// `true` collapsed `A<int>` against `A<String>`), the closed-set scans
    /// below read `allSubtypes != null` rather than sealedness alone
    /// (:236-266, [`Self::has_known_subtypes`]), and a record/function
    /// shape — a name-level sentinel, never a head — keeps the
    /// conservative `true` (:269-320's field walk is not modelled).
    pub(crate) fn can_be_subtype_of(&self, left: &NameTy, right: &NameTy) -> bool {
        self.can_be_subtype_of_within(left, right, 8)
    }

    /// [`Self::can_be_subtype_of`] with a recursion budget: the pair walks
    /// once per written type argument and once per `FutureOr` half, so the
    /// budget bounds the walk on a self-referential pool entry — an
    /// exhausted chain answers the official's conservative `true`
    /// (type_system.dart:322).
    fn can_be_subtype_of_within(&self, left: &NameTy, right: &NameTy, budget: u8) -> bool {
        if budget == 0 {
            return true;
        }
        let left = &self.extension_erasure(left);
        let right = &self.extension_erasure(right);
        let left_nullable = Self::is_potentially_nullable(left);
        let right_nullable = Self::is_potentially_nullable(right);
        if matches!(right, NameTy::Null) {
            return left_nullable;
        }
        if matches!(left, NameTy::Null) {
            return right_nullable;
        }
        // Both nullable meet at `null` (type_system.dart:172-174).
        if left_nullable && right_nullable {
            return true;
        }
        // `FutureOr<T>` is the union `T | Future<T>`: match both halves
        // against the other side (type_system.dart:173-189). The split
        // reads the union's element, so a nullable `FutureOr<T>?` takes it
        // too and the base is compared bare — the official
        // `futureOrBase` drops the projection.
        if let Some(base) = self.future_or_base(left) {
            return self.can_be_subtype_of_within(&base, right, budget - 1)
                || self.future_half_can_be(right, &base, true, budget - 1);
        }
        if let Some(base) = self.future_or_base(right) {
            return self.can_be_subtype_of_within(left, &base, budget - 1)
                || self.future_half_can_be(left, &base, false, budget - 1);
        }
        let (Some(left_name), Some(right_name)) = (self.head_name(left), self.head_name(right))
        else {
            // `Never` (already a subtype of everything), top, and unknown
            // shapes stay undecided.
            return true;
        };
        if left_name == right_name {
            return self.same_head_can_be(left, right, budget - 1);
        }
        // `int` and `double` cross in the JavaScript compilation model
        // (type_system.dart:195-199) — before either closed-set scan, which
        // would otherwise read the missing edge as a mutual exclusion.
        if (left_name == self.primitive_ids[0] && right_name == self.primitive_ids[1])
            || (left_name == self.primitive_ids[1] && right_name == self.primitive_ids[0])
        {
            return true;
        }
        // An opaque right head (a raw type-parameter spelling, an
        // unresolvable name) is the world's blind spot: whether the left
        // family can inhabit it is undecidable at name level, and the
        // closed-set scans below would read the missing edges as a mutual
        // exclusion. The official answers through the parameter's bound
        // (`T extends Enum` admits every enum), falling to the
        // conservative `true` without one (type_system.dart:322).
        if !self.is_known_head(right_name) {
            return true;
        }
        let left_non_null = NameTy::Named(left_name, false);
        // The enum's instances are exactly its constants, typed by the
        // enum (type_system.dart:221-230's this-type scan).
        if self
            .container(left_name)
            .is_some_and(|c| c.kind == ContainerKind::Enum)
        {
            return self.is_subtype_of(&left_non_null, right);
        }
        // Closed left: its instance set is the declared subclass closure —
        // the left instantiation itself, then every transitive subclass —
        // and the official loop answers outright, `false` included
        // (type_system.dart:236-250).
        if self.has_known_subtypes(left_name) {
            return self.is_subtype_of(&left_non_null, right)
                || self.any_subclass_reaches(left_name, right);
        }
        // Symmetric closed right (type_system.dart:252-266): the right's
        // candidates (itself, then its transitive subclasses) must be able
        // to view the left.
        if self.has_known_subtypes(right_name) {
            let right_non_null = NameTy::Named(right_name, false);
            return self.is_subtype_of(&right_non_null, &left_non_null)
                || self.any_subclass_reaches(right_name, &left_non_null);
        }
        true
    }

    /// The same-declaration arm: `canBeSubtypeOfInterfaces`
    /// (type_system.dart:201-219) compares the two argument lists pairwise
    /// with the same can-be ask — a covariant fit, and deliberately not the
    /// declared variance: the relation answers "could one value be both",
    /// so `A<num>` and `A<int>` still overlap while `A<int>` and
    /// `A<String>` do not. A bare side stays the wildcard it always was
    /// (the instantiate-to-bound head fits every instantiation); outside a
    /// parse-time window the argument slots cannot be resolved and the
    /// pre-#3236 collapse answer stands.
    fn same_head_can_be(&self, left: &NameTy, right: &NameTy, budget: u8) -> bool {
        let (NameTy::NamedArgs(_, _, left_args), NameTy::NamedArgs(_, _, right_args)) = (left, right)
        else {
            return true;
        };
        let Some(engine) = fold_engine() else {
            return true;
        };
        let pool = engine.boundary.pool();
        let left_args = pool.get(*left_args);
        let right_args = pool.get(*right_args);
        if left_args.len() != right_args.len() {
            return true;
        }
        left_args.iter().zip(right_args).all(|(left, right)| {
            self.can_be_subtype_of_within(
                &NameTy::from_semantic(*left),
                &NameTy::from_semantic(*right),
                budget,
            )
        })
    }

    /// The `T` of a `FutureOr<T>` spelling (type_system.dart:173-189): the
    /// union's first argument, `None` for every other shape and for a head
    /// the unit shadows (`class FutureOr`), where the spelling no longer
    /// names dart:async's union. The argument list resolves only inside a
    /// parse-time window — outside one the shape stays unsplit, the
    /// conservative direction.
    fn future_or_base(&self, ty: &NameTy) -> Option<NameTy> {
        let NameTy::NamedArgs(name, _, args) = ty else {
            return None;
        };
        if self.strings.resolve(*name) != "FutureOr" || future_or_shadowed(self, *name) {
            return None;
        }
        let engine = fold_engine()?;
        Some(NameTy::from_semantic(
            *engine.boundary.pool().get(*args).first()?,
        ))
    }

    /// The `Future<T>` half of a `FutureOr` split: `canBeSubtypeOf(Future<
    /// base>, other)` when `future_half_is_left` (the union stood on the
    /// left, type_system.dart:175-180) and `canBeSubtypeOf(other,
    /// Future<base>)` otherwise (:184-189). dart:async's `Future` has no
    /// container here, so the head is compared by spelling: a
    /// `Future`-spelled side compares its single argument against the base,
    /// any other side can only sit on the family through a declared edge,
    /// and a side whose instance set is fully known
    /// ([`Self::has_known_subtypes`]) and reaches no such edge decides the
    /// official `false`. An open or shadowed side stays undecided.
    fn future_half_can_be(
        &self,
        other: &NameTy,
        base: &NameTy,
        future_half_is_left: bool,
        budget: u8,
    ) -> bool {
        let Some(future) = self.future_head() else {
            return true;
        };
        let Some((head, args)) = Self::head_and_args(other) else {
            return true;
        };
        if head == future {
            let Some(args) = args else {
                return true;
            };
            let Some(engine) = fold_engine() else {
                return true;
            };
            let Some(other_base) = engine.boundary.pool().get(args).first().copied() else {
                return true;
            };
            let other_base = NameTy::from_semantic(other_base);
            return if future_half_is_left {
                self.can_be_subtype_of_within(base, &other_base, budget)
            } else {
                self.can_be_subtype_of_within(&other_base, base, budget)
            };
        }
        if self.named_sub(head, future) {
            return true;
        }
        !self.has_known_subtypes(head)
    }

    /// The interned `Future` spelling, `None` when the unit declares its own
    /// `Future` (the spelling no longer names dart:async's class) or an
    /// in-scope type parameter shadows it.
    fn future_head(&self) -> Option<StringId> {
        let future = self.strings.intern("Future");
        if self.container(future).is_some()
            || fold_engine().is_some_and(|engine| engine.is_scope_param(future))
        {
            return None;
        }
        Some(future)
    }

    /// The head and the written argument slot of a named or instantiated
    /// leaf; `None` for every other shape.
    fn head_and_args(ty: &NameTy) -> Option<(StringId, Option<ArgListId>)> {
        match ty {
            NameTy::Named(name, _) => Some((*name, None)),
            NameTy::NamedArgs(name, _, args) => Some((*name, Some(*args))),
            _ => None,
        }
    }

    /// Whether any transitive subclass of the closed `root` can
    /// reach `target` (the official candidate scan's subclass half,
    /// type_system.dart:243-262): a sealed root's subclass tree is fully
    /// known even through non-sealed intermediate nodes, so the walk
    /// follows plain extends/with/implements edges instead of the
    /// exhaustiveness walk's sealed-gated direct set — a direct-only scan
    /// misses deep members (`sealed S` → open `Base` → `B`) and answers a
    /// false mutual exclusion. The visited set replays the oracle's cycle
    /// guards (element.dart:167-210).
    fn any_subclass_reaches(&self, root: StringId, target: &NameTy) -> bool {
        let mut seen = rustc_hash::FxHashSet::default();
        seen.insert(root);
        let mut queue = self.direct_subcontainers(root);
        // The core chain's subtype edges sit outside the declared-clause
        // scan (`num` has no container of its own): a closed `num` whose
        // candidates ARE `int`/`double` must still try them, or
        // `case num n:` over an `int` scrutinee would read as a mutual
        // exclusion the oracle never reports.
        for (base, parent) in self.core_supertypes {
            if parent == root {
                queue.push(base);
            }
        }
        while let Some(current) = queue.pop() {
            if !seen.insert(current) {
                continue;
            }
            if self.is_subtype_of(&NameTy::Named(current, false), target) {
                return true;
            }
            queue.extend(self.direct_subcontainers(current));
        }
        false
    }

    /// The containers of `name`'s library ([`Self::scan_order`]) directly
    /// extending/with/implementing/on `name` (the same edge scan
    /// [`Self::name_subtypes`] uses, without its sealed gate).
    fn direct_subcontainers(&self, name: StringId) -> Vec<StringId> {
        let mut out = Vec::new();
        for (decl, declared) in self.scan_order(name) {
            if decl == name {
                continue;
            }
            let edge = declared.superclass == Some(name)
                || declared.with_clause.contains(&name)
                || declared.implements_clause.contains(&name)
                || declared.on_clause.contains(&name);
            if edge {
                out.push(decl);
            }
        }
        out
    }

    /// `getExtensionTypeErasure` (type.dart's `extensionTypeErasure`, fed
    /// into every `StaticType` the shared exhaustiveness walk builds —
    /// `getStaticType` erases at entry, shared.dart:195): an extension type
    /// stands for its representation type at every name-level ask. The
    /// representation formal's declared `Ty` converts through
    /// [`NameTy::from_semantic`] and joins the use-site `?` (`ET<num>?`
    /// erases to `E<num>?`, `extension_type.dart`'s
    /// `extensionTypeErasure` recursing under the nullability suffix);
    /// the representation's own written argument list stands verbatim —
    /// the name level compares heads, never substitutions (the
    /// exhaustiveness columns pattern-match on heads, matching the
    /// pre-existing `NamedArgs` posture). A name that resolves to no
    /// extension-type declaration, or to one whose representation formal
    /// is absent (a body-less recovery shape), keeps the written face.
    /// Chained representations (`extension type ET2(ET1 _)`) recurse the
    /// same way the official erasure does — the loop bound keeps a cyclic
    /// self-representation from hanging the walk (the element model's own
    /// `extension_type_representation_depends_on_itself` row already
    /// reported it; the face here just cannot loop).
    fn extension_erasure(&self, ty: &NameTy) -> NameTy {
        let mut current = *ty;
        for _ in 0..8 {
            let (name, nullable) = match current {
                NameTy::Named(name, nullable) | NameTy::NamedArgs(name, nullable, _) => {
                    (name, nullable)
                }
                _ => return current,
            };
            let erased = self.locate(name).and_then(|(container, owner)| {
                (container.kind == ContainerKind::ExtensionType)
                    .then(|| {
                        container
                            .primary_formals
                            .first()
                            .map(|rep| (rep.formal.ty, owner, container))
                    })
                    .flatten()
            });
            let Some((repr, owner, container)) = erased else {
                return current;
            };
            // An unknown representation (an unresolved annotation the
            // element model latched `Ty::Unknown` for) is the oracle's
            // `InvalidType` corner — erasing to the nullable top would
            // answer every ask `true` and false-positive the covering
            // gap; the written face keeps the under-report direction.
            if repr == Ty::Unknown {
                return current;
            }
            // The use-site instantiation substitutes the declaration's
            // parameters in the representation: `extension type ET<T>(List<T> _)
            // ` read at `ET<int>` erases to `List<int>`, never `List<T>`
            // (`extensionTypeErasure` substitutes through the element's
            // `instantiate`, type.dart). A bare use or a closed fold window
            // keeps the written representation (the under-report direction).
            let repr = match (current, fold_engine()) {
                (NameTy::NamedArgs(_, _, use_args), Some(engine))
                    if use_args != ArgListId::EMPTY
                        && !container.type_param_names.is_empty() =>
                {
                    let projected = engine.boundary.ty(owner, repr);
                    let use_args: Vec<Ty> =
                        engine.boundary.pool().get(use_args).to_vec();
                    Self::substitute_params(
                        engine,
                        projected,
                        &container.type_param_names,
                        &use_args,
                        8,
                    )
                }
                _ => repr,
            };
            let folded = NameTy::from_semantic(repr);
            current = match folded {
                // The representation's own nullability joins the use-site
                // `?` (`typedef`-free chains keep the OR on every hop).
                NameTy::Named(head, repr_nullable) => NameTy::Named(head, repr_nullable | nullable),
                NameTy::NamedArgs(head, repr_nullable, args) => {
                    NameTy::NamedArgs(head, repr_nullable | nullable, args)
                }
                // Non-named representation shapes (a record or function
                // type's name-level sentinel, `dynamic`, `Null`) stand
                // whole — the use-site `?` rides the projection faces the
                // way the written-face pair does elsewhere.
                other => other,
            };
        }
        current
    }

    /// The use-site substitution of `substitute_params`: `ty` is the
    /// representation's declared type, `params`/`use_args` the declaration's
    /// type-parameter spellings paired with the use-site argument list —
    /// `extension type ET<K, V>(Map<K, V> _)` read at `ET<int, bool>` maps
    /// `Map<K, V>` to `Map<int, bool>`. A bare parameter leaf
    /// (`Ty::Named` with an empty argument list) replaces positionally; a
    /// nested argument list recurses so `List<T>` inside `Map<String,
    /// List<T>>` substitutes too. The nullability the parameter leaf was
    /// written with ORs onto its replacement (`T?` over `int` reads
    /// `int?`). A parameter spelling without a use-site argument keeps the
    /// written leaf — the arity-mismatch shape is no honest instantiation
    /// and stays on the under-report side.
    fn substitute_params(
        engine: &Engine<'_>,
        ty: Ty,
        params: &[StringId],
        use_args: &[Ty],
        budget: u8,
    ) -> Ty {
        if budget == 0 {
            return ty;
        }
        let Ty::Named {
            name,
            nullable,
            args,
        } = ty
        else {
            return ty;
        };
        if args == ArgListId::EMPTY {
            let Some(index) = params.iter().position(|param| *param == name) else {
                return ty;
            };
            let Some(&replacement) = use_args.get(index) else {
                return ty;
            };
            return if nullable {
                match replacement {
                    Ty::Named {
                        name,
                        nullable: _,
                        args,
                    } => Ty::Named {
                        name,
                        nullable: true,
                        args,
                    },
                    other => other,
                }
            } else {
                replacement
            };
        }
        let inner: Vec<Ty> = engine.boundary.pool().get(args).to_vec();
        let mapped: Vec<Ty> = inner
            .iter()
            .map(|&arg| Self::substitute_params(engine, arg, params, use_args, budget - 1))
            .collect();
        if mapped == inner {
            return ty;
        }
        engine
            .boundary
            .args
            .borrow_mut()
            .named(name, nullable, &mapped)
    }

    /// `isPotentiallyNullable` (`type_system.dart:1260`): everything except
    /// the non-nullable shapes. The cast-pattern close reads it for the
    /// `null` half of `createCastSpace` (shared.dart:731-735).
    pub(crate) fn is_potentially_nullable(ty: &NameTy) -> bool {
        !matches!(
            ty,
            NameTy::Named(_, false)
                | NameTy::NamedArgs(_, false, _)
                | NameTy::TopNonNullable
                | NameTy::EnumValue(..)
                | NameTy::BoolValue(_)
                | NameTy::ListPat { .. }
                | NameTy::Never
        )
    }

    /// The declaration head a restricted shape answers through (the enum
    /// arm's this-type; the boolean constants' `bool`; a list pattern's
    /// `List`).
    fn head_name(&self, ty: &NameTy) -> Option<StringId> {
        match ty {
            NameTy::Named(name, _)
            | NameTy::NamedArgs(name, _, _)
            | NameTy::EnumValue(name, _)
            | NameTy::EnumValueOrNull(name, _) => Some(*name),
            NameTy::BoolValue(_) | NameTy::BoolOrNull(_) => Some(self.bool_id),
            NameTy::ListPat { .. } => Some(self.list_head()),
            _ => None,
        }
    }

    /// Whether the world decides `name`'s subtype face: a resolvable
    /// container, a core sealed type, the `Enum` root, the top `Object`, or
    /// a core primitive (whose chain [`CORE_SUPERTYPES`] carries). An
    /// unknown head leaves every mutual-exclusion question about it
    /// undecided — the conservative default, never a proven disjoint pair.
    /// The constant-verdict's matched face reads the same test
    /// ([`crate::diagnose::Engine::constant_never_matches`]): an unknown
    /// head there is the official's boundless-type-parameter/invalid
    /// value, unprovable disjointness.
    pub(crate) fn is_known_head(&self, name: StringId) -> bool {
        name == self.enum_id
            || name == self.object_id
            || self.primitive_ids.contains(&name)
            || self.container(name).is_some()
            || self.is_sealed_name(name)
    }

    /// `_canBeEqual` (constant_verifier.dart:561-596, the
    /// `TypeSystem::can_be_equal` port mirrored at name level): `false`
    /// proves `constant == value` always answers `false`. Only interface
    /// and `Null` constants reach the relation (the official's gate);
    /// named/`Null` values decide through the declared subtype relation
    /// (the greatest closure is the identity without type parameters);
    /// every other shape answers `true` (the official's type-parameter
    /// and non-interface fallbacks).
    pub(crate) fn can_be_equal(&self, constant: &NameTy, value: &NameTy) -> bool {
        let constant = &self.extension_erasure(constant);
        let value = &self.extension_erasure(value);
        if !matches!(
            constant,
            NameTy::Named(..) | NameTy::NamedArgs(..) | NameTy::Null
        ) {
            return true;
        }
        match value {
            NameTy::Named(..) | NameTy::NamedArgs(..) | NameTy::Null => {
                self.is_subtype_of(constant, value)
            }
            _ => true,
        }
    }

    /// The container record `name` resolves to: this unit's own first,
    /// then the library graph ([`Self::resolve_foreign`]).
    fn container(&self, name: StringId) -> Option<&'a Container> {
        self.locate(name).map(|(container, _)| container)
    }

    /// The container `name` resolves to with its declaring file.
    fn locate(&self, name: StringId) -> Option<(&'a Container, FileId)> {
        if let Some(container) = self.index.get(self.file).and_then(|f| f.container(name)) {
            return Some((container, self.file));
        }
        let scope = self.scope?;
        if let Some(&hit) = self.resolved.borrow().get(&name) {
            return hit;
        }
        let hit = self.resolve_foreign(scope, name);
        self.resolved.borrow_mut().insert(name, hit);
        hit
    }

    /// The cross-file resolution: the diagnosing library's namespace, then
    /// the unique graph-wide class declaration (a foreign container's own
    /// clause names resolve in its declaring library, which the diagnosing
    /// library need not import). Several declaring libraries answer `None`
    /// — the name level cannot pick the one the declaring scope sees.
    fn resolve_foreign(
        &self,
        scope: NameScope<'a>,
        name: StringId,
    ) -> Option<(&'a Container, FileId)> {
        if let Some(hit) = resolve_container(
            scope.graph,
            scope.namespaces,
            self.index,
            scope.lib,
            self.file,
            name,
        ) {
            return Some(hit);
        }
        let [owner_lib] = scope.graph.declaring_libraries(name) else {
            return None;
        };
        scope
            .graph
            .unit_files(*owner_lib)
            .iter()
            .find_map(|&file| self.index.get(file)?.container(name).map(|c| (c, file)))
    }

    /// The declaration-ordered containers a closed subclass scan over
    /// `root` walks: the units of the library declaring `root` (sealed
    /// subtypes live in their root's library), else this unit's own. Each
    /// candidate is read from its own unit — never re-resolved by name
    /// from the diagnosing file, where a same-named local could shadow it.
    fn scan_order(&self, root: StringId) -> Vec<(StringId, &'a Container)> {
        let units: Vec<FileId> = match (self.scope, self.locate(root)) {
            (Some(scope), Some((_, owner))) => scope
                .graph
                .library_of_file(owner)
                .map_or_else(|| vec![owner], |lib| scope.graph.unit_files(lib).to_vec()),
            _ => vec![self.file],
        };
        let mut order = Vec::new();
        for unit in units {
            let Some(elements) = self.index.get(unit) else {
                continue;
            };
            for &name in elements.container_order() {
                if let Some(container) = elements.container(name) {
                    order.push((name, container));
                }
            }
        }
        order
    }

    /// Whether `name` resolves to a container declared outside the
    /// diagnosing library (the private-enum face gate of the covering
    /// report, constant_verifier.dart:1110-1121).
    pub(crate) fn is_foreign_library(&self, name: StringId) -> bool {
        let Some(scope) = self.scope else {
            return false;
        };
        self.locate(name).is_some_and(|(_, owner)| {
            owner != self.file && scope.graph.library_of_file(owner) != Some(scope.lib)
        })
    }

    /// Whether `name` resolves to an enum declaration.
    pub(crate) fn is_enum_name(&self, name: StringId) -> bool {
        self.container(name)
            .is_some_and(|c| c.kind == ContainerKind::Enum)
    }

    /// The interned `List` spelling. A local `class List` does not
    /// shadow the pattern's required type — the list pattern's container
    /// is dart:core's own (`typeProvider.listType`,
    /// list_pattern_resolver.dart:40-45), and the name level reads the
    /// spelling's edges as declared (a file-local `List` mirrors the
    /// core face through its own clauses).
    fn list_head(&self) -> StringId {
        self.strings.intern("List")
    }

    /// `List<elem>` as the world sees it: `NamedArgs` inside a fold
    /// window (the pool carries the single-argument list), the bare head
    /// otherwise — the same under-report collapse `NamedArgs` itself
    /// takes.
    fn list_ty(&self, elem: Ty) -> NameTy {
        match fold_engine() {
            Some(engine) => {
                let ty = engine.boundary.args.borrow_mut().named(
                    self.list_head(),
                    false,
                    &[elem],
                );
                match ty {
                    Ty::Named { name, args, .. } => NameTy::NamedArgs(name, false, args),
                    _ => NameTy::Named(self.list_head(), false),
                }
            }
            None => NameTy::Named(self.list_head(), false),
        }
    }

    /// The element-type ask inside the list restriction
    /// (`typeOperations.isSubtypeOf(elementType, other.elementType)`,
    /// types/list.dart:282-284): the two `Ty`s convert through
    /// [`NameTy::from_semantic`] and ask the lattice directly. An
    /// unnameable element (`dynamic` → `TopNullable`) subtypes only the
    /// same face, matching the official's `dynamic <: dynamic`.
    fn elem_sub(&self, sub: Ty, sup: Ty) -> bool {
        self.is_subtype_of(&NameTy::from_semantic(sub), &NameTy::from_semantic(sup))
    }

    /// The element type of a `List`-headed static type: the written
    /// `List<E>` argument resolved through the boundary pool, else
    /// `Object?` (upstream's `elementType ?? objectQuestionType` — the
    /// bare-`List` face's element is `dynamic` upstream, but the
    /// argument-less name carries no pool slot here, and `Object?` is
    /// the widest honest face the name level spells).
    fn list_elem_of(&self, ty: &NameTy) -> Ty {
        if let NameTy::NamedArgs(_, _, args) = ty
            && let Some(engine) = fold_engine()
            && let Some(arg) = engine.boundary.pool().get(*args).first().copied()
        {
            return arg;
        }
        self.nullable_form_obj()
    }

    /// The `(list head, element type)` pair a list pattern's restriction
    /// rides on — `None` when the scrutinee provably can never be a
    /// `List` (a `Map`-headed or otherwise declared head with no `List`
    /// edge either way — where a rest mark inside a `{...}` frame is a
    /// MAP's rest, which this face must not mistake for the list split).
    /// The element reads off the open switch's scrutinee (`List<E>`'s
    /// written argument, else `Object?`).
    fn list_pattern_elem(&self) -> Option<(StringId, Ty)> {
        let scrutinee = fold_engine()
            .and_then(|engine| engine.switch.switches.last())
            .map(|ctx| ctx.scrutinee)?;
        // The rest-marked frame only becomes the LIST split when the
        // scrutinee can be one: a `Map`-headed scrutinee makes the mark
        // a map rest (`{...}`), and a provably non-list head makes
        // either spelling a never-match — the unknown space answers both
        // (under-report, never a false coverage claim).
        let list = self.list_head();
        let can_be_list = match scrutinee {
            Ty::Named { name, .. } => {
                name == list
                    || self.named_sub(name, list)
                    || self.named_sub(list, name)
                    || !self.is_known_head(name)
            }
            // `dynamic`/unknown/record-or-function sentinels: a list is
            // always possible (under-report for the map-rest twin).
            _ => true,
        };
        if !can_be_list {
            return None;
        }
        let elem = match scrutinee {
            Ty::Named { name, args, .. } if name == list => fold_engine()
                .and_then(|engine| engine.boundary.pool().get(args).first().copied())
                .unwrap_or_else(|| self.nullable_form_obj()),
            _ => self.nullable_form_obj(),
        };
        Some((list, elem))
    }

    /// `Object?` as a `Ty` — the element-type fallback
    /// (`objectQuestionType`, list_pattern_resolver.dart:40-45).
    fn nullable_form_obj(&self) -> Ty {
        let Ty::Named { name, args, .. } = Ty::named(self.object_id) else {
            unreachable!()
        };
        Ty::Named {
            name,
            nullable: true,
            args,
        }
    }

    /// Whether `name` declares an interface type in this unit: a class
    /// family head, mixin, enum, or extension type — a declaration whose
    /// name denotes a value's type. An `extension` is not: its name is no
    /// type in the subtyping lattice (upstream `_isInterfaceSubtypeOf`
    /// never sees one). Extension types reach `Object` through their
    /// representation chain; the name level pins them `Some(true)` without
    /// descending into it (a generic representation's bound corner stays
    /// unpinned — the under-report direction).
    fn is_interface_container(&self, name: StringId) -> bool {
        self.container(name)
            .is_some_and(|c| c.kind != ContainerKind::Extension)
    }

    /// The declared type of the instance member `member` on `head`,
    /// climbing the superclass chain (the official property lookup
    /// includes inherited members): fields through
    /// [`MemberEntry::var_ty`], getters through their return type. A name
    /// taken by a method or setter is no property; a cyclic hierarchy
    /// terminates on the visited set.
    pub(crate) fn member_column(&self, head: StringId, member: StringId) -> Option<NameTy> {
        let mut memo = FxHashMap::default();
        self.member_column_in(head, member, &mut memo).flatten()
    }

    /// [`Self::member_column`] over the official field-type merge
    /// (`_getInterfaceFieldTypes`, upstream Dart SDK source,
    /// pkg/analyzer/lib/src/generated/exhaustiveness.dart:382-414): an own
    /// declaration wins; otherwise every supertype's merged table is added
    /// in `allSupertypes` order and the LAST one naming the member wins —
    /// so a mixin's `String? value` reached after the superclass replaces
    /// the superclass's `String value`. The outer `Option` is "the table
    /// names the member", the inner the member's property face (`None` for
    /// a method/setter name). `memo` bounds the walk to one visit per name
    /// (and breaks hierarchy cycles).
    fn member_column_in(
        &self,
        head: StringId,
        member: StringId,
        memo: &mut FxHashMap<StringId, Option<Option<NameTy>>>,
    ) -> Option<Option<NameTy>> {
        if let Some(hit) = memo.get(&head) {
            return *hit;
        }
        memo.insert(head, None);
        let container = self.container(head)?;
        let answer = if let Some(entry) = container.members.get(&member)
            && !entry.is_static
        {
            Some(match entry.kind {
                MemberKind::Field => Some(NameTy::from_semantic(entry.var_ty)),
                MemberKind::Getter => Some(NameTy::from_semantic(entry.return_ty)),
                _ => None,
            })
        } else {
            self.all_supertypes(head)
                .into_iter()
                .rev()
                .find_map(|sup| self.member_column_in(sup, member, memo))
        };
        memo.insert(head, answer);
        answer
    }

    /// The `allSupertypes` order of `name` (`ClassHierarchy._getHierarchy`,
    /// upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/element/class_hierarchy.dart:39-97): each
    /// direct supertype (superclass, `on` constraints, interfaces, mixins —
    /// in that order) followed by its own `allSupertypes`, deduplicated by
    /// first insertion.
    fn all_supertypes(&self, name: StringId) -> Vec<StringId> {
        let mut order: Vec<StringId> = Vec::new();
        let mut seen = rustc_hash::FxHashSet::default();
        seen.insert(name);
        self.append_supertypes(name, &mut order, &mut seen);
        order
    }

    fn append_supertypes(
        &self,
        name: StringId,
        order: &mut Vec<StringId>,
        seen: &mut rustc_hash::FxHashSet<StringId>,
    ) {
        let Some(container) = self.container(name) else {
            return;
        };
        let direct = container
            .superclass
            .iter()
            .chain(container.on_clause.iter())
            .chain(container.implements_clause.iter())
            .chain(container.with_clause.iter());
        for &sup in direct {
            if seen.insert(sup) {
                order.push(sup);
                self.append_supertypes(sup, order, seen);
            }
        }
    }

    /// The written argument group of a `NamedArgs` static type in the
    /// official display form (`<bool, bool>`, leading `<` included); `None`
    /// when no fold window stands or the boundary pool cannot resolve the
    /// slot — the caller then renders the bare head.
    fn args_text(&self, args: ArgListId) -> Option<String> {
        let engine = fold_engine()?;
        // Copied out of the pool first: rendering re-borrows the same slots.
        let list: Vec<Ty> = engine.boundary.pool().get(args).to_vec();
        if list.is_empty() {
            return None;
        }
        let rendered: Vec<String> = list.iter().map(|arg| engine.render_ty(*arg)).collect();
        Some(format!("<{}>", rendered.join(", ")))
    }

    /// The witness/diagnostic text for a static type.
    fn ty_text(&self, ty: &NameTy) -> String {
        match ty {
            NameTy::Never => "Never".to_string(),
            NameTy::Null => "null".to_string(),
            NameTy::TopNullable => "_".to_string(),
            NameTy::TopNonNullable => "Object".to_string(),
            NameTy::Named(name, nullable) => {
                let text = self.strings.resolve(*name);
                if *nullable {
                    format!("{text}?")
                } else {
                    text.to_string()
                }
            }
            // The official witness text is the static type's own display
            // (`_BaseStaticType.typeToDart` → `typeToDart` writes the
            // resolved type, arguments included, static_type.dart:214-239):
            // a `Map<bool, bool>` scrutinee witnesses `Map<bool, bool>()`,
            // never the bare head. A slot the boundary cannot project (no
            // fold window) keeps the head alone — the under-report.
            NameTy::NamedArgs(name, nullable, args) => {
                let text = match self.args_text(*args) {
                    Some(args) => format!("{}{args}", self.strings.resolve(*name)),
                    None => self.strings.resolve(*name).to_string(),
                };
                if *nullable {
                    format!("{text}?")
                } else {
                    text
                }
            }
            NameTy::EnumValue(enum_name, constant) => {
                format!(
                    "{}.{}",
                    self.strings.resolve(*enum_name),
                    self.strings.resolve(*constant)
                )
            }
            // NullableStaticType.name: the underlying head plus `?`
            // (static_type.dart:375-378).
            NameTy::EnumValueOrNull(enum_name, constant) => {
                format!(
                    "{}.{}?",
                    self.strings.resolve(*enum_name),
                    self.strings.resolve(*constant)
                )
            }
            NameTy::BoolValue(value) => value.to_string(),
            NameTy::BoolOrNull(value) => format!("{value}?"),
            NameTy::Unknown(_) => "?".to_string(),
            // `ListTypeRestriction.toString` (types/list.dart:295-312): the
            // element type's argument text then `()`-per-slot and `...`
            // for the rest — the space-text shape `<[(), (), ...]>`.
            NameTy::ListPat {
                elem,
                size,
                has_rest,
            } => {
                let args = match elem {
                    Ty::Named { args, .. } if *args != ArgListId::EMPTY => {
                        self.args_text(*args).unwrap_or_default()
                    }
                    _ => String::new(),
                };
                let mut text = format!("{args}[");
                let mut comma = "";
                for _ in 0..*size {
                    text.push_str(comma);
                    text.push_str("()");
                    comma = ", ";
                }
                if *has_rest {
                    text.push_str(comma);
                    text.push_str("...");
                }
                text.push(']');
                text
            }
        }
    }

    /// Whether `sub`'s supertype chain reaches `sup`: the unit's declared
    /// superclass and `with`/`implements` edges plus the fixed core chain.
    /// The implicit `Object` edge sits beside the walk (the same arm
    /// [`Self::decide_subtype`] carries): the model records only written
    /// clauses, so a declaration-less `class C {}` and the core primitives
    /// (`bool`, `String`) carry no `Object` row, yet every interface type
    /// subtypes the non-nullable top (upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/dart/element/subtype.dart:304-340`). Without it
    /// the sealed-right arm of [`Self::can_be_subtype_of`] answers a false
    /// mutual exclusion (`Object` vs `bool` reports
    /// `pattern_never_matches_value_type` — oracle probes keep it silent).
    /// An unresolvable name still answers `false` here: whether its denoted
    /// type subtypes `Object` is undecidable at name level, and
    /// `can_be_subtype_of`'s undecided shapes take the `true` default on
    /// their own (type_system.dart:324).
    pub(crate) fn named_sub(&self, sub: StringId, sup: StringId) -> bool {
        if sub == sup {
            return true;
        }
        if sup == self.object_id
            && (self.is_interface_container(sub) || self.primitive_ids.contains(&sub))
        {
            return true;
        }
        let mut queue = std::collections::VecDeque::new();
        let mut seen = rustc_hash::FxHashSet::default();
        queue.push_back(sub);
        seen.insert(sub);
        while let Some(name) = queue.pop_front() {
            for edge in self.direct_edges(name) {
                if edge == sup {
                    return true;
                }
                if seen.insert(edge) {
                    queue.push_back(edge);
                }
            }
            for (base, parent) in self.core_supertypes {
                if name == base {
                    if parent == sup {
                        return true;
                    }
                    if seen.insert(parent) {
                        queue.push_back(parent);
                    }
                }
            }
        }
        false
    }

    /// The argument-carrying pair's verdict: the semantic subtype face
    /// decides the full pair (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/element/subtype.dart — the interface type
    /// case: same declaration answers through the type argument list,
    /// distinct heads through the supertype graph). `Some(true)` folds the
    /// check; a decidable miss and an unprovable pair both decline it (the
    /// test stays live, promotion stays conservative — the
    /// `_checkAllTypeChecks` verdict only ever fires on a proven
    /// constant). The ask runs through the parse-time engine window
    /// ([`fold_engine`]) because the argument slots resolve in the
    /// boundary pool the engine owns; outside a window (the bare-world
    /// rigs, faces past the parse) the pre-#2462 collapse answer stands.
    fn args_subtype(
        &self,
        sub_name: StringId,
        sub_nullable: bool,
        sub_args: ArgListId,
        sup_name: StringId,
        sup_nullable: bool,
        sup_args: ArgListId,
    ) -> bool {
        match fold_engine() {
            Some(engine) => {
                let sub = Ty::Named {
                    name: sub_name,
                    nullable: sub_nullable,
                    args: sub_args,
                };
                let sup = Ty::Named {
                    name: sup_name,
                    nullable: sup_nullable,
                    args: sup_args,
                };
                match engine.slot_subtype(sub, sup) {
                    Some(verdict) => verdict,
                    // The semantic face declined (an undeclared head cut the
                    // walk). A same-head pair still decides argument-wise:
                    // `E<String> <: E<num>` walks the single covariant
                    // argument ask `String <: num` — a proven miss the
                    // element-level `isSubtypeOf` also answers `false` for,
                    // so the constant `E.b` never populates `E<num>`'s set
                    // (enum.dart:111-150).
                    None if sub_name == sup_name => {
                        self.args_decide_fallback(engine, sub_args, sup_args)
                    }
                    // A different-head pair the semantic face could not
                    // relate keeps the unproven answer `false`: an is-check
                    // fold must never read a constant `true` off two
                    // distinct declarations (`async.B<int>` against
                    // `await.B` — different import prefixes, different
                    // classes).
                    None => false,
                }
            }
            None => true,
        }
    }

    /// The same-head argument fallback of [`Self::args_subtype`]:
    /// `slot_subtype` declined a pair whose heads match, so the interface
    /// rule's pairwise argument ask decides the pair directly. Every
    /// argument must prove `sub_arg <: sup_arg` through the name-level
    /// closed walk; one proven `false` (`String <: num`) declines the
    /// whole pair, an undecided leaf keeps the conservative `true`.
    fn args_decide_fallback(
        &self,
        engine: &Engine<'_>,
        sub_args: ArgListId,
        sup_args: ArgListId,
    ) -> bool {
        let sub_list = engine.boundary.pool().get(sub_args).to_vec();
        let sup_list = engine.boundary.pool().get(sup_args).to_vec();
        if sub_list.len() != sup_list.len() || sub_list.is_empty() {
            return true;
        }
        for (sub_arg, sup_arg) in sub_list.iter().zip(sup_list.iter()) {
            let (
                Ty::Named {
                    name: sub_name, ..
                },
                Ty::Named {
                    name: sup_name, ..
                },
            ) = (sub_arg, sup_arg)
            else {
                return true;
            };
            match self.decide_subtype(*sub_name, *sup_name) {
                Some(false) => return false,
                Some(true) => {}
                // `Object` is the root interface: its only supertypes are
                // the tops, so `Object <: S` fails for every other head
                // (`List<Object>` is no `List<int>`) — the walk cuts at the
                // declaration-less `Object` but the answer is closed.
                None if *sub_name == self.object_id => return false,
                // Two distinct dart:core primitives the closed walk cannot
                // reach (no container, no written clause — `String` and
                // `num` carry no `core_supertypes` row) are still a proven
                // miss: every primitive is `final`/`sealed` in dart:core,
                // so `String <: num` decides `false` where the walk's
                // `cut` answers `None`. The primitive faces the model
                // already trusts ([`Self::primitive_ids`]) decide it.
                None
                    if self.primitive_ids.contains(sub_name)
                        && self.primitive_ids.contains(sup_name) =>
                {
                    return false;
                }
                None => return true,
            }
        }
        true
    }

    /// The closed-walk subtype verdict over the same lattice
    /// [`Self::named_sub`] answers `bool` on: `Some(true)` — a declared or
    /// core edge path proves `sub <: sup`; `Some(false)` — the walk closed
    /// over a fully visible hierarchy (every visited head declared in this
    /// unit or carrying a core edge) without a hit, so no edge exists;
    /// `None` — the walk visited a head with neither a declaration nor a
    /// core edge (a cross-file or undeclared name whose real supertypes
    /// are invisible here), so absence is unprovable. The override verdict
    /// consumes this on the pairs its assignability face cannot decide: a
    /// found edge is a proof and answers before any cut is reached, a cut
    /// only demotes the decisive `false` to undecided.
    ///
    /// The implicit `Object` edge sits outside the walk: the model records
    /// only written clauses, so a declaration-less `class C {}` carries no
    /// `Object` row and a closed walk would wrongly answer `Some(false)`.
    /// Every interface type subtypes the non-nullable top regardless
    /// (upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/dart/element/subtype.dart:304-340`, the
    /// right-`Object` rule's fall-through), so when the supertype *is*
    /// `Object` the question degenerates: an in-file interface type or a
    /// core primitive answers `Some(true)` outright. Everything else — a
    /// cross-file head, a type parameter (its bound is invisible at name
    /// level, `subtype.dart:306-317`), `Never` — stays undecided, the
    /// under-report direction.
    pub(crate) fn decide_subtype(&self, sub: StringId, sup: StringId) -> Option<bool> {
        if sub == sup {
            return Some(true);
        }
        if sup == self.object_id
            && (self.is_interface_container(sub) || self.primitive_ids.contains(&sub))
        {
            return Some(true);
        }
        // An unresolvable TARGET is undecidable, not disproved: a type
        // parameter (`Self` in `Self retuned(int)` over `Tunable<Self>`), a
        // cross-file head, or a misspelling has no declaration or core edge
        // the walk could ever reach — a closed walk would answer
        // `Some(false)` having never compared the real face. The walk's own
        // `cut` covers undeclared nodes it dequeues; the target needs the
        // same gate up front.
        if self.container(sup).is_none()
            && !self.primitive_ids.contains(&sup)
            && sup != self.object_id
            && !self
                .core_supertypes
                .iter()
                .any(|(_, parent)| *parent == sup)
        {
            return None;
        }
        let mut queue = std::collections::VecDeque::new();
        let mut seen = rustc_hash::FxHashSet::default();
        queue.push_back(sub);
        seen.insert(sub);
        let mut cut = false;
        while let Some(name) = queue.pop_front() {
            // A declared head's edge set is complete; a declaration-less
            // head with no core edge cuts the walk (undecided).
            let mut closed = false;
            if self.container(name).is_some() {
                closed = true;
                for edge in self.direct_edges(name) {
                    if edge == sup {
                        return Some(true);
                    }
                    if seen.insert(edge) {
                        queue.push_back(edge);
                    }
                }
            }
            for (base, parent) in self.core_supertypes {
                if name == base {
                    closed = true;
                    if parent == sup {
                        return Some(true);
                    }
                    if seen.insert(parent) {
                        queue.push_back(parent);
                    }
                }
            }
            cut |= !closed;
        }
        if cut { None } else { Some(false) }
    }

    /// The declared direct edges out of `name` (superclass + interfaces).
    /// An enum declaration adds dart:core's `Enum`: the analyzer's element
    /// model hangs the interface on every enum without a written clause
    /// (the mirror of the hierarchy snapshot's synthesized edge —
    /// `E <: Enum` holds, so `e is Enum` is a guaranteed-true test both
    /// the flow and the check tiers must answer; #2370/#2380). `Enum`
    /// itself carries no self-edge: a class (and the `bool` builtin, a
    /// plain class there) keeps `x is Enum` decidable.
    fn direct_edges(&self, name: StringId) -> Vec<StringId> {
        let Some(container) = self.container(name) else {
            return Vec::new();
        };
        let mut edges = Vec::new();
        if let Some(superclass) = container.superclass {
            edges.push(superclass);
        }
        edges.extend(container.with_clause.iter().copied());
        edges.extend(container.implements_clause.iter().copied());
        edges.extend(container.on_clause.iter().copied());
        if container.kind == ContainerKind::Enum && name != self.enum_id {
            edges.push(self.enum_id);
        }
        edges
    }

    pub(crate) fn is_sealed_name(&self, name: StringId) -> bool {
        if name == self.bool_id {
            return true;
        }
        self.container(name)
            .is_some_and(|c| c.sealed || c.kind == ContainerKind::Enum)
    }

    /// Whether the world knows `name`'s complete subtype set — the
    /// `ClassElementImpl.allSubtypes` non-null condition (upstream Dart SDK
    /// source, pkg/analyzer/lib/src/dart/element/element.dart:159-217): a
    /// `final` class (its subtypes are confined to its library), a `sealed`
    /// class or enum (whose closed set the walk enumerates), and the
    /// dart:core primitives — `int`/`double`/`num`/`bool`/`String` are
    /// `final`/`sealed` in dart:core, so `int` against an ordinary
    /// interface requirement is decided, not undecided (#3236: the dominant
    /// firing arm of `pattern_never_matches_value_type`).
    ///
    /// Deliberately NOT [`Self::is_sealed_name`]: the exhaustiveness gate
    /// asks `isAlwaysExhaustive`'s question, whose class arm is `isSealed`
    /// alone (type_system.dart:815-857 — a `final` class's switch is not
    /// exhaustive), while `canBeSubtypeOf`'s closed-set scan asks
    /// `allSubtypes != null` (type_system.dart:236-266), which `final` also
    /// answers. One predicate cannot serve both: folding `final` into the
    /// sealed gate would call a `final`-class switch exhaustive and report
    /// its `default` unreachable.
    pub(crate) fn has_known_subtypes(&self, name: StringId) -> bool {
        self.is_sealed_name(name)
            || self.primitive_ids.contains(&name)
            || self
                .container(name)
                .is_some_and(|c| c.kind == ContainerKind::Class && c.is_final)
    }

    /// Whether the world decides `name`'s sealedness and subclass set:
    /// the name resolves to a container (this unit's, or one the library
    /// graph reaches — [`Self::locate`]), it is a core sealed type
    /// (`bool`), or it is dart:core's `Enum` root — no unit ever declares
    /// that container, yet its exhaustiveness face is fully decided here
    /// (not sealed, no coverable subtypes — [`Self::name_subtypes`]), so
    /// the covering verdict over it is the world's answer, not a blind
    /// spot. An unresolvable name is opaque — its sealed classes and
    /// enum constants are invisible, so a non-exhaustive verdict over it
    /// may be false and the expression-face covering emit suppresses.
    pub(crate) fn name_resolvable(&self, name: StringId) -> bool {
        name == self.enum_id || self.container(name).is_some() || self.is_sealed_name(name)
    }

    /// The constant's declared instantiation as a [`NameTy`]
    /// (`enumField.type`, `AnalyzerEnumOperations.getEnumElementType`):
    /// the member row's `var_ty` — `E<String>` for `b<String>`,
    /// `E<num>` for an argument-less constant on `E<T extends num>`,
    /// the bare head for a non-generic enum or a pre-field payload —
    /// projected across the pool boundary through the parse-time
    /// engine. `UNKNOWN` or a missing window keeps the bare-head answer
    /// (the pre-#3026 face, never a false verdict).
    fn enum_constant_ty(&self, enum_name: StringId, constant: StringId) -> NameTy {
        let Some((container, owner)) = self.locate(enum_name) else {
            return NameTy::Named(enum_name, false);
        };
        let Some(engine) = fold_engine() else {
            return NameTy::Named(enum_name, false);
        };
        let Some(entry) = container.members.get(&constant) else {
            return NameTy::Named(enum_name, false);
        };
        if entry.var_ty == Ty::Unknown {
            return NameTy::Named(enum_name, false);
        }
        NameTy::from_semantic(engine.boundary.ty(owner, entry.var_ty))
    }

    /// The generic-enum filter of `EnumStaticType._createEnumElements`
    /// (upstream Dart SDK source, pkg/_fe_analyzer_shared/lib/src/
    /// exhaustiveness/types/enum.dart:99-134): a constant only populates
    /// the enum's closed subtype set when its declared type is a subtype
    /// of the scrutinee's overapproximation — `E.b` (`E<String>`) can
    /// never be a value of `E<num>` or of `E<T>` over `T extends num`
    /// (whose overapproximation is `E<num>`), so an `E.a || E.c` case
    /// set is already exhaustive and `E.b` is a dead constant. The
    /// constant's declared type is the member row's `var_ty` the
    /// collector seals from the element's written instantiation;
    /// `EMPTY`/foreign slots and any undecidable ask keep the constant
    /// (the pre-existing under-report direction).
    fn enum_constant_populates(
        &self,
        container: &'a Container,
        owner: FileId,
        constant: StringId,
        scrutinee: &NameTy,
    ) -> bool {
        let Some(engine) = fold_engine() else {
            return true;
        };
        let Some(entry) = container.members.get(&constant) else {
            return true;
        };
        if entry.var_ty == Ty::Unknown {
            return true;
        }
        // Project the declared type across the pool boundary (a foreign
        // enum's slot indexes its own pool), then overapproximate the
        // scrutinee's written arguments — each in-scope type parameter
        // substitutes its declared bound (`overapproximate(E<T>)` is
        // `E<num>` for `T extends num`, shared.dart's
        // `typeOperations.overapproximate`).
        let declared = engine.boundary.ty(owner, entry.var_ty);
        let Some(over) = Self::overapproximate(engine, scrutinee) else {
            return true;
        };
        match engine.slot_subtype(declared, over) {
            Some(verdict) => verdict,
            // `slot_subtype` declined — an undeclared head cut the walk. A
            // same-head pair (`E<String>` against `E<num>`) still decides
            // argument-wise: `String <: num` is a proven miss, so `E.b`
            // never populates `E<num>`'s set (enum.dart:111-150).
            None => {
                let (
                    Ty::Named {
                        name: declared_head,
                        args: declared_args,
                        ..
                    },
                    Ty::Named {
                        name: over_head,
                        args: over_args,
                        ..
                    },
                ) = (declared, over)
                else {
                    return true;
                };
                if declared_head == over_head {
                    self.args_decide_fallback(engine, declared_args, over_args)
                } else {
                    true
                }
            }
        }
    }

    /// The scrutinee's overapproximation as a semantic `Ty`
    /// (`typeOperations.overapproximate`, enum.dart:124): the written
    /// head's argument list with each in-scope type parameter replaced
    /// by its declared bound, recursively. A bare head, an unknown
    /// shape, or a list the pool cannot resolve stays `None` — the
    /// constant keeps its place (under-report).
    fn overapproximate(engine: &Engine<'_>, ty: &NameTy) -> Option<Ty> {
        let NameTy::NamedArgs(name, nullable, args) = *ty else {
            return None;
        };
        let args = engine.boundary.pool().get(args).to_vec();
        let mapped: Vec<Ty> = args
            .iter()
            .map(|&arg| Self::overapprox_ty(engine, arg, 8))
            .collect();
        Some(
            engine
                .boundary
                .args
                .borrow_mut()
                .named(name, nullable, &mapped),
        )
    }

    /// One argument's overapproximation (`TypeSystem.overapproximate`'s
    /// type-parameter arm): an in-scope parameter spelling substitutes
    /// its declared bound, any other leaf passes through, and a nested
    /// argument list recurses. The budget keeps a cyclic bound chain
    /// structurally terminating; an exhausted chain keeps the written
    /// face (under-report — the unprovable answer).
    fn overapprox_ty(engine: &Engine<'_>, ty: Ty, budget: u8) -> Ty {
        if budget == 0 {
            return ty;
        }
        match ty {
            Ty::Named {
                name,
                nullable,
                args,
            } => {
                if engine.is_scope_param(name)
                    && let Some(bound) = engine.param_bound(name)
                {
                    let bound = Self::overapprox_ty(engine, bound, budget - 1);
                    return if nullable {
                        match bound {
                            Ty::Named {
                                name,
                                nullable: _,
                                args,
                            } => Ty::Named {
                                name,
                                nullable: true,
                                args,
                            },
                            other => other,
                        }
                    } else {
                        bound
                    };
                }
                if args == ArgListId::EMPTY {
                    return ty;
                }
                let inner = engine.boundary.pool().get(args).to_vec();
                let mapped: Vec<Ty> = inner
                    .iter()
                    .map(|&arg| Self::overapprox_ty(engine, arg, budget - 1))
                    .collect();
                if mapped == inner {
                    return ty;
                }
                engine
                    .boundary
                    .args
                    .borrow_mut()
                    .named(name, nullable, &mapped)
            }
            other => other,
        }
    }

    /// `_isContainedIn(type, space)` (upstream Dart SDK source,
    /// pkg/_fe_analyzer_shared/lib/src/exhaustiveness/shared.dart:660-698,
    /// the cast-pattern containment test): every value of `ty` is matched by
    /// `space`, where a type-free leaf covers the case's `context`.
    ///
    /// A union is ONE space, not a set of independent alternatives: a
    /// sealed/enum `ty` is contained when the union covers its whole
    /// closed subtype set (`expandSealedSubtypes`, shared.dart:682-696 —
    /// `(A() || B()) as M` over sealed `M` is contained, so the cast's
    /// throw path covers the rest and the whole context is matched). The
    /// per-part walk alone answered "some single part contains `ty`",
    /// which never holds for a supertype against its own subtypes.
    ///
    /// The property columns of an object single are not walked here (a cast
    /// whose sub-pattern constrains fields answers the conservative `false`,
    /// so the row keeps the narrower space — never a false full coverage).
    pub(crate) fn contained_in(
        &self,
        ty: NameTy,
        space: &Restriction,
        context: NameTy,
    ) -> bool {
        match space {
            Restriction::NamedContext => self.is_subtype_of(&ty, &context),
            Restriction::Union(parts) => {
                if parts
                    .iter()
                    .any(|part| self.contained_in(ty, part, context))
                {
                    return true;
                }
                if !self.is_sealed(&ty) {
                    return false;
                }
                let subtypes = self.subtypes(&ty, &rustc_hash::FxHashSet::default());
                !subtypes.is_empty()
                    && subtypes
                        .iter()
                        .all(|sub| parts.iter().any(|part| self.contained_in(*sub, part, context)))
            }
            Restriction::Null => matches!(ty, NameTy::Null),
            // The official `_isContainedIn` gate is `_isUnrestricted(single)`
            // (shared.dart:660-671): a single whose property columns carry
            // constraints proves only its own narrowed region — `A(b: true)
            // as A` covers `A(b: true)`, never the whole context.
            Restriction::Object(name, fields) => {
                fields.is_empty() && self.is_subtype_of(&ty, &NameTy::Named(*name, false))
            }
            _ => leaf_ty(space).is_some_and(|leaf| self.is_subtype_of(&ty, &leaf)),
        }
    }

    /// The closed subtype set of a sealed class or enum, in declaration
    /// order (types/sealed.dart, types/enum.dart).
    fn name_subtypes(&self, name: StringId, scrutinee: &NameTy) -> Vec<NameTy> {
        if name == self.bool_id {
            return vec![NameTy::BoolValue(true), NameTy::BoolValue(false)];
        }
        if name == self.enum_id {
            // The dart:core `Enum` root's split is empty: the root is no
            // exhaustiveness-closed set (every library may declare more
            // enums), so its member declarations never enumerate as
            // coverable subtypes. Constant patterns over an `Enum`-typed
            // scrutinee leave the root's own space as the witness
            // (`Enum()`, the oracle's `non_exhaustive` witness text), never
            // an enumeration of the visible enum declarations.
            return Vec::new();
        }
        let Some((container, owner)) = self.locate(name) else {
            return Vec::new();
        };
        if container.kind == ContainerKind::Enum {
            return container
                .enum_constants
                .iter()
                .filter(|constant| {
                    self.enum_constant_populates(container, owner, **constant, scrutinee)
                })
                .map(|constant| NameTy::EnumValue(name, *constant))
                .collect();
        }
        if !container.sealed {
            return Vec::new();
        }
        // The sealed split instantiates each subtype as a view of the
        // scrutinee's OWN arguments (`getSubclassAsInstanceOf`,
        // upstream Dart SDK source,
        // pkg/_fe_analyzer_shared/lib/src/exhaustiveness/types/sealed.dart:
        // 77-119 — `Face<num>` splits into `Jack<num>`, `Queen<num>`, …).
        // A case written over one instantiation (`Queen<int>`) therefore
        // never covers the sibling instantiations, which the bare-head split
        // pretended it did. Only a subtype declaring exactly as many
        // parameters as the argument list carries is instantiated this way
        // (the identity-substitution shape `class Jack<T> extends Face<T>`);
        // every other edge keeps its bare head — the under-report direction.
        let args = match scrutinee {
            NameTy::NamedArgs(_, _, args) => Some(*args),
            _ => None,
        };
        let mut subtypes = Vec::new();
        for (decl, declared) in self.scan_order(name) {
            // A well-founded closed set never contains its root: a
            // self-edge (`sealed S extends S`) drops here, before the
            // walk's cycle guard would discard it.
            if decl == name {
                continue;
            }
            let edge = declared.superclass == Some(name)
                || declared.with_clause.contains(&name)
                || declared.implements_clause.contains(&name)
                || declared.on_clause.contains(&name);
            if !edge {
                continue;
            }
            let instantiated = args.filter(|args| self.same_arity(decl, *args));
            subtypes.push(match instantiated {
                Some(args) => NameTy::NamedArgs(decl, false, args),
                None => NameTy::Named(decl, false),
            });
        }
        subtypes
    }

    /// Whether `decl` declares exactly as many type parameters as the
    /// argument list `args` carries — the identity substitution the sealed
    /// split applies to it.
    fn same_arity(&self, decl: StringId, args: ArgListId) -> bool {
        let Some(container) = self.container(decl) else {
            return false;
        };
        let Some(engine) = fold_engine() else {
            return false;
        };
        usize::from(container.type_params) == engine.boundary.pool().get(args).len()
    }
}
