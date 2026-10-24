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
use dartq_source::{FileId, SourceRange, StringId, StringTable};
use dartq_types::{ExhaustivenessWorld, Key, Single, Space};
use rustc_hash::FxHashMap;
use std::cell::RefCell;

/// The open-switch/pattern state: one engine owns one tracker; the state
/// and its update rules were engine fields/methods before the extraction
/// (behavior byte-identical).
pub(crate) struct SwitchTracker {
    /// Open switch contexts (statements and expressions): the scrutinee's
    /// push-time type, the cases completed so far, and the anchors the
    /// diagnostics attach to.
    pub(crate) switches: Vec<SwitchCtx>,
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
}

impl SwitchTracker {
    pub(crate) fn new() -> Self {
        SwitchTracker {
            switches: Vec::new(),
            pattern_stack: Vec::new(),
            pattern_depth: 0,
            pattern_roots: Vec::new(),
            case_sink_open: false,
            if_case_window: false,
            pending_const: None,
            switch_kw_latch: None,
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
    /// type-free pattern covers its context).
    pub(crate) fn collapse(leaves: Vec<Restriction>, or_ops: usize) -> Restriction {
        if leaves.is_empty() {
            // A type-free variable/wildcard: the context full space. The
            // context is substituted when the case closes.
            return Restriction::NamedContext;
        }
        if or_ops > 0 {
            return Restriction::Union(leaves);
        }
        // `&&` joins (or a bare single leaf) intersect at name level.
        leaves
            .into_iter()
            .reduce(Restriction::and)
            .unwrap_or(Restriction::Unknown)
    }

    /// Closes the case-header pattern window and drains its root: the
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
    pub(crate) fn open_if_case(&mut self, scrutinee: Ty, kw: SourceRange) {
        self.switches.push(SwitchCtx {
            is_expression: false,
            switch_kw: kw,
            scrutinee,
            cases: Vec::new(),
            default_kw: None,
            default_pos: None,
            pending_root: None,
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
        self.switches.pop();
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
            default_pos: None,
            pending_root: None,
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
        let covered = fields.iter().all(|field| match field.leaves.as_slice() {
            // A type-free sub-pattern (`var x`, `_`, the `:x` shorthand)
            // covers the whole column.
            [] => true,
            // A conjunctive residual is not one name-level row.
            [leaf] => match field.name {
                Some(name) => {
                    let Restriction::Named(lt, ln) = leaf else {
                        return false;
                    };
                    world.member_column(ty, name).is_some_and(|column| {
                        world.is_subtype_of(&column, &NameTy::Named(*lt, *ln))
                    })
                }
                // Positional fields: the member table keeps no declaration
                // order, so a constraining leaf is unattributable.
                None => false,
            },
            _ => false,
        });
        let leaf = if covered {
            Restriction::Named(ty, false)
        } else {
            Restriction::Unknown
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
        Restriction::Unknown | Restriction::NamedContext => false,
        Restriction::Null => !world.can_be_subtype_of(&NameTy::Null, &matched),
        Restriction::Named(name, nullable) => {
            !world.can_be_subtype_of(&NameTy::Named(*name, *nullable), &matched)
        }
        Restriction::NamedArgs(name, nullable, args) => !world
            .can_be_subtype_of(
                &NameTy::NamedArgs(*name, *nullable, *args),
                &matched,
            ),
        Restriction::BoolValue(value) => {
            !world.can_be_subtype_of(&NameTy::BoolValue(*value), &matched)
        }
        Restriction::EnumValue(enum_name, constant) => !world
            .can_be_subtype_of(&NameTy::EnumValue(*enum_name, *constant), &matched),
    }
}

/// `isAlwaysExhaustive` (type_system.dart:846-887) at name level:
/// `bool`, `Null`, enums, and sealed classes split completely; nothing
/// else does. The official check reads the scrutinee type's *element*
/// face (`isDartCoreBool`/enum/`isSealed`) and is nullability-agnostic
/// — `S?` of a sealed `S` stays always-exhaustive, the null half being
/// the space model's nullable split rather than this gate (`dynamic`
/// has no element and stays `false`). The official check carries three
/// arms this name-level port does not model: extension types (erased to
/// the representation type, then recursed — the candidate belongs to the
/// extension-type member-model ticket, not this site), `FutureOr<T>`
/// (recursed into the type argument), and promoted type parameters
/// (recursed into the promoted bound, then the declared bound). All
/// three fall to the `false` arm here — under-report, never a false
/// positive. The declared-bound half of the type-parameter arm is
/// modelled on the flow side instead (`dartq_flow::Interp`'s
/// `is_always_exhaustive`, which reads the recorder's bound table): this
/// level's [`NameWorld`] is built without any bound source (its
/// constructor takes a resolution snapshot that carries names and
/// supertypes only), so the parameter is indistinguishable from any other
/// unresolved named head here.
pub(crate) fn is_always_exhaustive(world: &NameWorld<'_>, ty: NameTy) -> bool {
    match ty {
        NameTy::Null => true,
        NameTy::Named(name, _) => world.is_sealed_name(name),
        _ => false,
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
    /// A unique unknown (relational patterns, unrepresentable constants):
    /// matched by nothing.
    Unknown(u64),
}

/// One completed case: its coverage restriction, the guard flag, the
/// token the diagnostics anchor to (`case` for statement cases, `=>` for
/// expression cases), and — statement cases, last head of a member — the
/// member body's whitespace-trimmed span (the flow-dead member rows ride
/// it, `dead_code_verifier.dart`'s `flowEnd` member trim). `head_colon` is
/// the head's own `:` — the parser only reports the member colon at
/// `end_switch_case` for a `default` head (`colon_after_default`), so the
/// case-headed member's body boundary latches here.
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
    /// The case-head count that precedes the first `default` member (the
    /// chain split: the flow layer owns the default-driven dead members,
    /// so the exhaustiveness-driven chain stops there); `None` when the
    /// switch has no default.
    pub(crate) default_pos: Option<usize>,
    /// The captured coverage of an in-flight switch-expression case (the
    /// space latches at the pattern's completion, the guard and anchor at
    /// the case's end). Statement cases capture in one step and leave this
    /// `None`.
    pub(crate) pending_root: Option<Restriction>,
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
    /// The unknown space: relational patterns, list/map patterns, and
    /// constants the name level cannot represent. Never must-matches, so
    /// the engine under-reports instead of mis-answering.
    Unknown,
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
            out.push(Single::full(NameTy::Named(*name, *nullable)));
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
    FOLD_ENGINES.with(|stack| {
        stack
            .borrow()
            .last()
            .copied()
            .map(|ptr| unsafe { &*ptr })
    })
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
                | NameTy::BoolValue(_),
                NameTy::TopNonNullable,
            ) => true,
            // A constant's type is its enum, non-null.
            (NameTy::EnumValue(en, _), sup) => self.is_subtype_of(&NameTy::Named(*en, false), sup),
            // The restricted nullable inherits the value's type edges, but
            // only toward nullable targets (its `null` half never fits a
            // non-nullable type).
            (NameTy::EnumValueOrNull(en, _), NameTy::Named(tn, true) | NameTy::NamedArgs(tn, true, _)) => {
                self.is_subtype_of(&NameTy::Named(*en, false), &NameTy::Named(*tn, true))
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
            _ => false,
        }
    }

    fn is_sealed(&self, ty: &NameTy) -> bool {
        match ty {
            // Nullable projections split (static_type.dart:356).
            NameTy::Named(_, true)
            | NameTy::NamedArgs(_, true, _)
            | NameTy::TopNullable
            | NameTy::EnumValueOrNull(..)
            | NameTy::BoolOrNull(_) => true,
            NameTy::Named(name, false) | NameTy::NamedArgs(name, false, _) => {
                self.is_sealed_name(*name)
            }
            _ => false,
        }
    }

    fn subtypes(&self, ty: &NameTy) -> Vec<NameTy> {
        match ty {
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
            NameTy::Named(name, false) | NameTy::NamedArgs(name, false, _) => {
                self.name_subtypes(*name)
            }
            _ => Vec::new(),
        }
    }

    /// `getPropertyType` (static_type.dart:36-46): the declared type of
    /// the instance member `key` names, or `None` when the world does not
    /// know it (the checker's columns then fall back to the nullable
    /// object, exhaustive.dart:396-406).
    fn property_type(&self, ty: &NameTy, key: &Key) -> Option<NameTy> {
        let (NameTy::Named(name, _) | NameTy::NamedArgs(name, _, _)) = ty else {
            return None;
        };
        let Key::Name(member) = key else {
            // Record keys are not modeled at name level.
            return None;
        };
        self.member_column(*name, self.strings.intern(member))
    }

    fn is_bottom(&self, ty: &NameTy) -> bool {
        matches!(ty, NameTy::Never)
    }

    fn is_null(&self, ty: &NameTy) -> bool {
        matches!(ty, NameTy::Null)
    }

    fn witness_text(&self, ty: &NameTy, fields: &[(Key, String)]) -> String {
        if fields.is_empty() {
            return self.ty_text(ty);
        }
        let inner = fields
            .iter()
            .map(|(key, text)| match key {
                Key::RecordIndex(index) => format!("${}:{text}", index + 1),
                Key::RecordName(name) | Key::Name(name) => format!("{name}: {text}"),
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
    /// Whether some runtime value can inhabit both regions:
    /// `canBeSubtypeOf` (type_system.dart:150-330), the verified
    /// `TypeSystem::can_be_subtype_of` port mirrored at name level — the
    /// mutual-exclusion companion of [`ExhaustivenessWorld::is_subtype_of`].
    /// The Null arms decide before everything (type_system.dart:160-169),
    /// which is the pinned corner where the two relations legitimately
    /// disagree: `Never <: Null` (subtype.dart:299-301) yet no `Never`
    /// value can sit in a `Null` slot. Undecided shapes answer `true` (the
    /// official's conservative default, `type_system.dart:324`); function,
    /// `FutureOr`, and record shapes do not exist at name level and fall
    /// to the same default.
    pub(crate) fn can_be_subtype_of(&self, left: &NameTy, right: &NameTy) -> bool {
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
        let (Some(left_name), Some(right_name)) = (self.head_name(left), self.head_name(right))
        else {
            // `Never` (already a subtype of everything), top, and unknown
            // shapes stay undecided.
            return true;
        };
        if left_name == right_name {
            // Same declaration, and the name level carries no type
            // arguments to disagree over.
            return true;
        }
        // An opaque right head (a raw type-parameter spelling, an
        // unresolvable name) is the world's blind spot: whether the left
        // family can inhabit it is undecidable at name level, and the
        // closed-set scans below would read the missing edges as a mutual
        // exclusion. The official answers through the parameter's bound
        // (`T extends Enum` admits every enum), falling to the
        // conservative `true` without one (type_system.dart:324).
        if !self.is_known_head(right_name) {
            return true;
        }
        let left_non_null = NameTy::Named(left_name, false);
        // The enum's instances are exactly its constants, typed by the
        // enum (type_system.dart:229-241's this-type scan).
        if self
            .container(left_name)
            .is_some_and(|c| c.kind == ContainerKind::Enum)
        {
            return self.is_subtype_of(&left_non_null, right);
        }
        // Closed left: its instance set is the declared subclass closure —
        // the left instantiation itself, then every transitive subclass
        // (type_system.dart:243-262, `allSubtypes`, element.dart:167-210).
        if self.is_sealed_name(left_name) {
            if self.is_subtype_of(&left_non_null, right) {
                return true;
            }
            return self.any_subclass_reaches(left_name, right);
        }
        // Symmetric closed right (type_system.dart:263-278): the right's
        // candidates (itself, then its transitive subclasses) must be able
        // to view the left.
        if self.is_sealed_name(right_name) {
            let right_non_null = NameTy::Named(right_name, false);
            if self.is_subtype_of(&right_non_null, &left_non_null) {
                return true;
            }
            return self.any_subclass_reaches(right_name, &left_non_null);
        }
        true
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

    /// `isPotentiallyNullable` (`type_system.dart:1260`): everything except
    /// the non-nullable shapes.
    fn is_potentially_nullable(ty: &NameTy) -> bool {
        !matches!(
            ty,
            NameTy::Named(_, false)
                | NameTy::NamedArgs(_, false, _)
                | NameTy::TopNonNullable
                | NameTy::EnumValue(..)
                | NameTy::BoolValue(_)
                | NameTy::Never
        )
    }

    /// The declaration head a restricted shape answers through (the enum
    /// arm's this-type; the boolean constants' `bool`).
    fn head_name(&self, ty: &NameTy) -> Option<StringId> {
        match ty {
            NameTy::Named(name, _)
            | NameTy::NamedArgs(name, _, _)
            | NameTy::EnumValue(name, _)
            | NameTy::EnumValueOrNull(name, _) => Some(*name),
            NameTy::BoolValue(_) | NameTy::BoolOrNull(_) => Some(self.bool_id),
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
    fn resolve_foreign(&self, scope: NameScope<'a>, name: StringId) -> Option<(&'a Container, FileId)> {
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
        let mut current = head;
        let mut seen = rustc_hash::FxHashSet::default();
        while seen.insert(current) {
            let container = self.container(current)?;
            if let Some(entry) = container.members.get(&member)
                && !entry.is_static
            {
                return match entry.kind {
                    MemberKind::Field => Some(NameTy::from_semantic(entry.var_ty)),
                    MemberKind::Getter => Some(NameTy::from_semantic(entry.return_ty)),
                    _ => None,
                };
            }
            current = container.superclass?;
        }
        None
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
            // The witness text keeps the head spelling (the exhaustiveness
            // witnesses never write arguments — the pre-#2462 text).
            NameTy::NamedArgs(name, nullable, _) => {
                let text = self.strings.resolve(*name);
                if *nullable {
                    format!("{text}?")
                } else {
                    text.to_string()
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
                engine.slot_subtype(sub, sup) == Some(true)
            }
            None => true,
        }
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
            && !self.core_supertypes.iter().any(|(_, parent)| *parent == sup)
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

    /// The closed subtype set of a sealed class or enum, in declaration
    /// order (types/sealed.dart, types/enum.dart).
    fn name_subtypes(&self, name: StringId) -> Vec<NameTy> {
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
        let Some(container) = self.container(name) else {
            return Vec::new();
        };
        if container.kind == ContainerKind::Enum {
            return container
                .enum_constants
                .iter()
                .map(|constant| NameTy::EnumValue(name, *constant))
                .collect();
        }
        if !container.sealed {
            return Vec::new();
        }
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
            if edge {
                subtypes.push(NameTy::Named(decl, false));
            }
        }
        subtypes
    }
}
