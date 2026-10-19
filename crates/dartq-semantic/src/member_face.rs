//! The member-face subsystem: the interned member/type spellings, the
//! member-invocation latches, and the write-target spelling cache the
//! diagnosis engine's member resolution reads (seam 3 of the diagnose
//! engine extraction, ticket #1052).
//!
//! # Census (what moved off [`crate::diagnose::Engine`])
//!
//! * Intern caches: `object_members` / `enum_synth_members` /
//!   `builtin_types` / `setter_ids` / `primitive_ids` / `core_supertypes`
//!   — populated once in [`MemberFace::new`] (the hot paths compare ids,
//!   never re-intern).
//! * Member-invocation latches: `pending_must_call_super` and the #748
//!   `equals_check`/`nsm_check` pair with their [`EqualsCheck`] /
//!   [`NsmCheck`] / [`NsmShape`] records, plus the pure latch-transition
//!   methods (`nsm_*`, `equals_*`).
//! * The member-spelling tables: [`OBJECT_MEMBERS`],
//!   [`ENUM_SYNTH_MEMBERS`], [`BUILTIN_TYPES`], [`CORE_TYPE_MEMBERS`],
//!   [`FINAL_PRIMITIVES`].
//!
//! # What stays engine-side
//!
//! The `walk_members`/`walk_from`/`walk_super` climb and the
//! `member_defined`/`member_found`/`check_*_member` verdicts read the
//! library graph, namespaces, element index, boundary pool, and the emit
//! machinery — they stay [`crate::diagnose::Engine`] methods that reach
//! this state through the `members` field, per the established recipe (the
//! `report_switch`/`check_constant_pattern` precedents); the tests call
//! `Engine::walk_members` directly. The engine's per-pass face-walk memo
//! (`FaceMemo` in `diagnose.rs`) memoizes [`crate::diagnose::Walk`]
//! outcomes, a diagnose-private type, so it stayed beside them.
//!
//! # Rulings
//!
//! * `NameWorld` stays in `switch_tracker.rs`: it is the exhaustiveness
//!   world the switch/pattern face owns; the member face contributes only
//!   the interned [`CORE_SUPERTYPES`] and [`FINAL_PRIMITIVES`] copies,
//!   handed over at the engine's `name_world()` constructor (a documented
//!   crossing, like the seam-2 engine crossings).
//! * `MemberFace::new` interns the five batches in the engine's old
//!   order; `identical_id` (the engine's, not this face's) now interns
//!   before the batch instead of between `primitive_ids` and
//!   `core_supertypes`. `StringIds` are internal: every emission path sorts
//!   by offset or by collect-time name ids, and the use-face sets answer
//!   membership queries only, so the init-batch permutation is
//!   unobservable.

use crate::model::Ty;
use crate::switch_tracker::CORE_SUPERTYPES;
use dartq_source::{SourceRange, StringId, StringTable};
use rustc_hash::FxHashMap;

/// Members of `dart:core` `Object`: defined on every class-shaped receiver
/// the M1 walk can reach (the superclass chain ends at an unindexed root).
pub(crate) const OBJECT_MEMBERS: [&str; 5] =
    ["==", "hashCode", "toString", "noSuchMethod", "runtimeType"];

/// Synthesized enum members absent from the model; suppressed on enum
/// receivers so the member walk cannot false-positive on them.
pub(crate) const ENUM_SYNTH_MEMBERS: [&str; 3] = ["values", "index", "name"];

/// Builtin type names: always defined, independent of any import.
pub(crate) const BUILTIN_TYPES: [&str; 1] = ["dynamic"];

/// The dart:core primitive types whose member face the engine answers
/// directly: their declarations are external (the library graph carries
/// their top-level names only), so a member walk chain-breaks — but the
/// oracle still reports a genuinely absent member on them. The listed
/// members (instance getters/methods; setters only where the type declares
/// one — `List`'s `first=`/`last=`/`length=`) keep the check on the
/// oracle's side: a name on the list is defined, everything else reports.
/// Derived from the reference SDK (`sdk/lib/core/*.dart`,
/// `sdk/lib/async/future.dart`).
///
/// The container/future rows (`List`, `Map`, `Set`, `Iterable`, `Future`)
/// extend the same face to the core generics: `List`/`Set` fold their
/// `Iterable` supertype face, and each row folds the platform-extension
/// members that apply unconditionally at the name level
/// (`IterableExtensions`/`NullableIterableExtensions`/`EnumByName`/
/// `FutureIterable` — `firstOrNull`, `indexed`, `wait`, …). Their element
/// type bounds are invisible to the name-level model, so folding them is
/// the suppress direction (a bound violation the oracle would report stays
/// silent); omitting one would report a genuinely present member.
pub(crate) const CORE_TYPE_MEMBERS: [(&str, &[&str]); 13] = [
    (
        "Object",
        &[
            "hashCode",
            "runtimeType",
            "noSuchMethod",
            "toString",
            "==",
        ],
    ),
    (
        "Iterator",
        &["current", "moveNext", "hashCode", "runtimeType", "noSuchMethod", "toString"],
    ),
    (
        "int",
        &[
            "bitLength",
            "isEven",
            "isOdd",
            "sign",
            "isFinite",
            "isInfinite",
            "isNaN",
            "isNegative",
            "abs",
            "gcd",
            "modInverse",
            "modPow",
            "remainder",
            "round",
            "roundToDouble",
            "floor",
            "floorToDouble",
            "ceil",
            "ceilToDouble",
            "truncate",
            "truncateToDouble",
            "toDouble",
            "toInt",
            "toStringAsFixed",
            "toStringAsExponential",
            "toStringAsPrecision",
            "toRadixString",
            "toSigned",
            "toUnsigned",
            "clamp",
            "compareTo",
            "toString",
            "hashCode",
            "runtimeType",
            "noSuchMethod",
        ],
    ),
    (
        "double",
        &[
            "isFinite",
            "isInfinite",
            "isNaN",
            "isNegative",
            "sign",
            "abs",
            "round",
            "roundToDouble",
            "floor",
            "floorToDouble",
            "ceil",
            "ceilToDouble",
            "truncate",
            "truncateToDouble",
            "toInt",
            "toDouble",
            "toStringAsFixed",
            "toStringAsExponential",
            "toStringAsPrecision",
            "clamp",
            "compareTo",
            "toString",
            "hashCode",
            "runtimeType",
            "noSuchMethod",
        ],
    ),
    (
        "num",
        &[
            "isFinite",
            "isInfinite",
            "isNaN",
            "isNegative",
            "sign",
            "abs",
            "remainder",
            "round",
            "roundToDouble",
            "floor",
            "floorToDouble",
            "ceil",
            "ceilToDouble",
            "truncate",
            "truncateToDouble",
            "toInt",
            "toDouble",
            "toStringAsFixed",
            "toStringAsExponential",
            "toStringAsPrecision",
            "clamp",
            "compareTo",
            "toString",
            "hashCode",
            "runtimeType",
            "noSuchMethod",
        ],
    ),
    (
        "String",
        &[
            "codeUnits",
            "isEmpty",
            "isNotEmpty",
            "length",
            "runes",
            "allMatches",
            "codeUnitAt",
            "compareTo",
            "contains",
            "endsWith",
            "indexOf",
            "lastIndexOf",
            "matchAsPrefix",
            "padLeft",
            "padRight",
            "replaceAll",
            "replaceFirst",
            "replaceFirstMapped",
            "replaceAllMapped",
            "split",
            "splitMapJoin",
            "startsWith",
            "substring",
            "toLowerCase",
            "toUpperCase",
            "trim",
            "trimLeft",
            "trimRight",
            "toString",
            "hashCode",
            "runtimeType",
            "noSuchMethod",
        ],
    ),
    (
        "bool",
        &["toString", "hashCode", "runtimeType", "noSuchMethod"],
    ),
    (
        "Iterable",
        &[
            "any",
            "asNameMap",
            "cast",
            "contains",
            "elementAt",
            "elementAtOrNull",
            "every",
            "expand",
            "first",
            "firstOrNull",
            "firstWhere",
            "followedBy",
            "fold",
            "forEach",
            "hashCode",
            "indexed",
            "isEmpty",
            "isNotEmpty",
            "iterator",
            "join",
            "last",
            "lastOrNull",
            "lastWhere",
            "length",
            "map",
            "noSuchMethod",
            "nonNulls",
            "reduce",
            "runtimeType",
            "single",
            "singleOrNull",
            "singleWhere",
            "skip",
            "skipWhile",
            "take",
            "takeWhile",
            "toList",
            "toSet",
            "toString",
            "wait",
            "where",
            "whereType",
        ],
    ),
    (
        "List",
        &[
            "add",
            "addAll",
            "any",
            "asMap",
            "asNameMap",
            "cast",
            "clear",
            "contains",
            "elementAt",
            "elementAtOrNull",
            "every",
            "expand",
            "fillRange",
            "first",
            "first=",
            "firstOrNull",
            "firstWhere",
            "followedBy",
            "fold",
            "forEach",
            "getRange",
            "hashCode",
            "indexed",
            "indexOf",
            "indexWhere",
            "isEmpty",
            "isNotEmpty",
            "iterator",
            "join",
            "last",
            "last=",
            "lastIndexOf",
            "lastIndexWhere",
            "lastOrNull",
            "lastWhere",
            "length",
            "length=",
            "map",
            "noSuchMethod",
            "nonNulls",
            "reduce",
            "remove",
            "removeAt",
            "removeLast",
            "removeRange",
            "removeWhere",
            "replaceRange",
            "retainWhere",
            "reversed",
            "runtimeType",
            "setAll",
            "setRange",
            "shuffle",
            // The mutating/index faces `undefined_method` must not fire on
            // (#2161): `insert`/`insertAll` are declared on `List` itself
            // (sdk/lib/core/list.dart:630/645), `[]`/`[]=` are the index
            // read/write spellings the member/operator rows carry, `+` is
            // the concat operator.
            "insert",
            "insertAll",
            "[]",
            "[]=",
            "+",
            "single",
            "singleOrNull",
            "singleWhere",
            "skip",
            "skipWhile",
            "sort",
            "sublist",
            "take",
            "takeWhile",
            "toList",
            "toSet",
            "toString",
            "wait",
            "where",
            "whereType",
        ],
    ),
    (
        "Map",
        &[
            "addAll",
            "addEntries",
            "cast",
            "clear",
            "containsKey",
            "containsValue",
            "entries",
            "forEach",
            "hashCode",
            "isEmpty",
            "isNotEmpty",
            "keys",
            "length",
            "map",
            "noSuchMethod",
            "putIfAbsent",
            "remove",
            "removeWhere",
            "runtimeType",
            "toString",
            "update",
            "updateAll",
            "values",
            // The index faces `undefined_method`/`undefined_operator` must
            // not fire on (#2161): `m['k']`/`m['k']=v` are `Map`'s `[]`/
            // `[]=` operators (sdk/lib/core/map.dart) carried on the member
            // row as `[]`/`[]=`.
            "[]",
            "[]=",
        ],
    ),
    (
        "Set",
        &[
            "add",
            "addAll",
            "any",
            "asNameMap",
            "cast",
            "clear",
            "contains",
            "containsAll",
            "difference",
            "elementAt",
            "elementAtOrNull",
            "every",
            "expand",
            "first",
            "firstOrNull",
            "firstWhere",
            "followedBy",
            "fold",
            "forEach",
            "hashCode",
            "indexed",
            "intersection",
            "isEmpty",
            "isNotEmpty",
            "iterator",
            "join",
            "last",
            "lastOrNull",
            "lastWhere",
            "length",
            "lookup",
            "map",
            "noSuchMethod",
            "nonNulls",
            "reduce",
            "remove",
            "removeAll",
            "removeWhere",
            "retainAll",
            "retainWhere",
            "runtimeType",
            "single",
            "singleOrNull",
            "singleWhere",
            "skip",
            "skipWhile",
            "take",
            "takeWhile",
            "toList",
            "toSet",
            "toString",
            "union",
            "wait",
            "where",
            "whereType",
        ],
    ),
    (
        "Future",
        &[
            "asStream",
            "catchError",
            "hashCode",
            "ignore",
            "noSuchMethod",
            "onError",
            "runtimeType",
            "then",
            "timeout",
            "toString",
            "whenComplete",
        ],
    ),
    // The `dart:collection` interface faces (unindexed like `dart:core`):
    // `Queue`'s declared members plus the `Iterable` set it re-exports —
    // `class Q implements Queue { @override add }` must not over-report
    // `override_on_non_overriding_member` (#2161's DelegatingQueue).
    (
        "Queue",
        &[
            "add",
            "addAll",
            "addFirst",
            "addLast",
            "any",
            "asNameMap",
            "cast",
            "clear",
            "contains",
            "elementAt",
            "elementAtOrNull",
            "every",
            "expand",
            "first",
            "firstOrNull",
            "firstWhere",
            "followedBy",
            "fold",
            "forEach",
            "hashCode",
            "indexed",
            "isEmpty",
            "isNotEmpty",
            "iterator",
            "join",
            "last",
            "lastOrNull",
            "lastWhere",
            "length",
            "map",
            "noSuchMethod",
            "nonNulls",
            "reduce",
            "remove",
            "removeFirst",
            "removeLast",
            "removeWhere",
            "retainWhere",
            "runtimeType",
            "single",
            "singleOrNull",
            "singleWhere",
            "skip",
            "skipWhile",
            "take",
            "takeWhile",
            "toList",
            "toSet",
            "toString",
            "wait",
            "where",
            "whereType",
        ],
    ),
];

/// The dart:core primitives whose assignability the name level decides: a
/// value typed by one of these names never fits another (the types are
/// final — no user subclass or implementation can close the gap). `num` is
/// the one supertype edge (`int`/`double` fit it).
pub(crate) const FINAL_PRIMITIVES: [&str; 5] = ["int", "double", "num", "bool", "String"];

/// The dart:core static members whose declared return type the member walk
/// cannot reach (the declaring containers are unindexed like every SDK
/// unit, so `C.static()`'s send types `Unknown` and every downstream face
/// over-reports against the unresolved read — `end =
/// RangeError.checkValidRange(...)` never promotes `end`, #2172). Each row
/// is `(container, member, return head)` from the reference SDK
/// (`sdk/lib/core/errors.dart:375`); the answer is the non-nullable named
/// head the declared return spells.
pub(crate) const CORE_STATIC_RETURNS: [(&str, &str, &str); 1] =
    [("RangeError", "checkValidRange", "int")];

/// The dart:core interface member faces the element index never carries:
/// `dart:core` declarations resolve through the namespace but land on
/// unindexed (external) units, so the `@override` walk's
/// [`crate::diagnose::Engine::face_member`] answers `None` for them and a
/// legal `class C implements Comparable<C> { @override compareTo }`
/// over-reports `override_on_non_overriding_member`. The listed members
/// are the reference SDK's declared instance members
/// (`sdk/lib/core/comparable.dart`, `sdk/lib/core/pattern.dart`) — the
/// name-level set the oracle's `getOverridden` would find.
pub(crate) const CORE_INTERFACE_MEMBERS: [(&str, &[&str]); 2] = [
    ("Comparable", &["compareTo"]),
    ("Pattern", &["allMatches", "matchAsPrefix"]),
];

/// The `dart:collection` mixin/base faces that share a `dart:core`
/// interface's member set (the `getOverridden` walk lands on them through
/// `extends`/`with`/`implements`, but their units are unindexed like
/// `dart:core` — #2161's `ListBase`/`ListMixin` `replaceRange`/`[]=` rows).
/// Each maps to the [`CORE_TYPE_MEMBERS`] row whose members it re-exports:
/// `ListMixin` implements `List` (which is `Iterable`), `SetMixin`
/// implements `Set`, `MapMixin`/`MapView` implement `Map`, `IterableBase`/
/// `IterableMixin` implement `Iterable`. The union of the mapped rows is the
/// face's member set; the base/mixin's OWN members (`ListMixin.add`…) are a
/// subset of that face.
pub(crate) const COLLECTION_INTERFACE_ALIASES: [(&str, &[&str]); 33] = [
    ("ListBase", &["List", "Iterable"]),
    ("ListMixin", &["List", "Iterable"]),
    ("UnmodifiableListBase", &["List", "Iterable"]),
    ("UnmodifiableListMixin", &["List", "Iterable"]),
    ("UnmodifiableListView", &["List", "Iterable"]),
    ("LinkedList", &["List", "Iterable"]),
    ("SetBase", &["Set", "Iterable"]),
    ("SetMixin", &["Set", "Iterable"]),
    ("UnmodifiableSetMixin", &["Set", "Iterable"]),
    ("UnmodifiableSetView", &["Set", "Iterable"]),
    ("HashSet", &["Set", "Iterable"]),
    ("LinkedHashSet", &["Set", "Iterable"]),
    ("SplayTreeSet", &["Set", "Iterable"]),
    ("MapBase", &["Map"]),
    ("MapMixin", &["Map"]),
    ("MapView", &["Map"]),
    ("UnmodifiableMapBase", &["Map"]),
    ("UnmodifiableMapMixin", &["Map"]),
    ("UnmodifiableMapView", &["Map"]),
    ("HashMap", &["Map"]),
    ("LinkedHashMap", &["Map"]),
    ("SplayTreeMap", &["Map"]),
    ("IterableBase", &["Iterable"]),
    ("IterableMixin", &["Iterable"]),
    ("UnmodifiableIterableBase", &["Iterable"]),
    ("Queue", &["Queue", "Iterable"]),
    ("Deque", &["Queue", "Iterable"]),
    ("ListQueue", &["Queue", "Iterable"]),
    ("DoubleLinkedQueue", &["Queue", "Iterable"]),
    ("UnmodifiableQueueView", &["Queue", "Iterable"]),
    ("HasNext", &["Iterator"]),
    ("HasNextIterator", &["Iterator"]),
    ("LinkedListEntry", &["Object"]),
];

/// The `dart:core` classes whose constructors are const-constructible but
/// whose units are never indexed (the CLI's SDK posture): a `const C(...)` /
/// `C.named(...)` — or the implicit `const` inside a const-declaration
/// initializer — over one of these containers is a valid constant creation,
/// never a `const_initialized_with_non_constant_value` failure. Each row
/// maps the container spelling to its const-constructible selector set: `""`
/// is the unnamed-face entry (`pragma(` calls the const factory `pragma`,
/// which redirects to `pragma._`), and the named spellings cover the
/// factory/generative selectors (`Deprecated.implement`, `bool.fromEnvironment`).
/// Asserted against upstream `pkg:core` — a container absent here falls
/// through to the indexed-container walk unchanged.
pub(crate) const CORE_CONST_CTOR: [(&str, &[&str]); 15] = [
    ("bool", &["fromEnvironment", "hasEnvironment"]),
    ("Deprecated", &["", "extend", "implement", "instantiate", "mixin", "optional", "subclass"]),
    ("Duration", &[""]),
    ("FormatException", &[""]),
    ("int", &["fromEnvironment"]),
    ("IntegerDivisionByZeroException", &[""]),
    ("Iterable", &["", "empty"]),
    ("MapEntry", &[""]),
    ("Object", &[""]),
    ("OutOfMemoryError", &[""]),
    ("pragma", &[""]),
    ("StackOverflowError", &[""]),
    ("String", &["fromEnvironment"]),
    ("Symbol", &[""]),
    ("_Enum", &[""]),
];

/// The member-face state: one engine owns one face; the caches and latches
/// were engine fields before the extraction (behavior byte-identical).
pub(crate) struct MemberFace {
    /// Interned reserved-member spellings (populated once in
    /// [`MemberFace::new`]: the hot `resolve_bare` path compares ids, never
    /// re-interns).
    pub(crate) object_members: [StringId; OBJECT_MEMBERS.len()],
    pub(crate) enum_synth_members: [StringId; ENUM_SYNTH_MEMBERS.len()],
    pub(crate) builtin_types: [StringId; BUILTIN_TYPES.len()],
    /// `name` → `name=` lookup spellings already interned (write targets
    /// repeat across a unit; the cache is bounded by their distinct names).
    pub(crate) setter_ids: FxHashMap<StringId, StringId>,
    /// The five final primitives' interned names (the assignability table
    /// and the literal types compare ids, never text).
    pub(crate) primitive_ids: [StringId; 5],
    /// [`CORE_SUPERTYPES`] pre-interned once per engine: the
    /// subtyping BFS compares ids, never re-resolves/re-interns.
    pub(crate) core_supertypes: [(StringId, StringId); 3],
    /// The open method's `must_call_super` latch: the member's name span and
    /// the declaring container of the `@mustCallSuper` ancestor, probed at
    /// the declaration head and adjudicated at the body's end.
    pub(crate) pending_must_call_super: Option<(SourceRange, StringId)>,
    /// The #748 `==` operator-parameter latch.
    pub(crate) equals_check: Option<EqualsCheck>,
    /// The #748 `noSuchMethod` body-shape latch.
    pub(crate) nsm_check: Option<NsmCheck>,
}

/// The #748 `==` operator-parameter latch: the operator's name-token anchor
/// plus the formal-list progress. The written type is snapshotted from the
/// first outer-list formal (`end_formal_parameter`); function/record type
/// shapes poison the snapshot (their written heads cannot decide — the
/// official's resolved `dartCoreObject` question never reads as `Object?`
/// there, so the poison only preserves silence).
pub(crate) struct EqualsCheck {
    pub(crate) name_range: SourceRange,
    /// 0 = the method's own parameter list has not opened; 1 = inside it;
    /// higher = nested (function-typed formal) lists.
    pub(crate) depth: u32,
    /// The first outer-list formal's written type, `None` before the first
    /// formal ends (or when no annotation was written).
    pub(crate) first_ty: Option<Ty>,
    /// A function/record type shape intervened before the first formal's
    /// type completed.
    pub(crate) poisoned: bool,
    /// A type annotation completed inside the method's own parameter list
    /// (`handle_type`): the formal-name snapshot reads it. Operator
    /// declarations never latch their return type (`C::OperatorName`), so
    /// the stale pre-list arming must not masquerade as the formal's.
    pub(crate) ty_seen: bool,
}

/// The #748 `noSuchMethod` body-shape latch: tracks the
/// `super.noSuchMethod(<one argument>)` shape across the body's events.
/// The machine arms only after the formals close (closure bodies in
/// parameter defaults predate the body and must not drive it), and the
/// expression-body face adjudicates at `handle_expression_function_body`
/// while the block face latches its candidate return for the block's end.
#[expect(clippy::struct_excessive_bools)] // gates mirror the official noSuchMethod checks
pub(crate) struct NsmCheck {
    pub(crate) name_range: SourceRange,
    pub(crate) shape: NsmShape,
    /// The method's own block-body `{` once seen (`None` until then — a
    /// closure's body block in the defaults window also predates it).
    pub(crate) body_block: Option<SourceRange>,
    /// Formals have closed: body events may drive the machine.
    pub(crate) armed: bool,
    /// The machine is inside a return statement's expression (the block
    /// face's poison window; the expression face is always "inside").
    pub(crate) in_return: bool,
    /// A candidate return was seen in the body block (`return_seen`) and
    /// whether its expression completed the shape (`return_ok`), with the
    /// candidate's span ends (`return_kw` — the `return` keyword's offset;
    /// `return_semi_end` — the `;`'s exclusive end) for the block-face
    /// statement-kind gate.
    pub(crate) return_seen: bool,
    pub(crate) return_ok: bool,
    pub(crate) return_kw: Option<u32>,
    pub(crate) return_semi_end: Option<u32>,
    /// The selector name matched `noSuchMethod` at the identifier event
    /// (the dot event's token is the send's end, not the name — the name
    /// rides the `ExpressionContinuation` identifier).
    pub(crate) name_ok: bool,
}

/// The `super.noSuchMethod(<one argument>)` shape machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NsmShape {
    /// Awaiting the body's `super`.
    Open,
    /// `super` seen, awaiting `.` `noSuchMethod`.
    Super,
    /// The member name seen, awaiting `(`.
    Method,
    /// Inside the argument list; `depth` counts nested lists (argument
    /// content is opaque — only the outermost count decides). The
    /// selector's arguments parse BEFORE the member-name dot event (the
    /// parser's `super.m(x)` order: `super`, the send, then the dot).
    Args { depth: u32 },
    /// The send closed; the dot event validating the member name is due.
    Sent { count: u32 },
    /// The shape completed.
    Done,
    /// The shape is broken.
    Dead,
}

impl MemberFace {
    pub(crate) fn new(strings: &StringTable) -> Self {
        MemberFace {
            object_members: OBJECT_MEMBERS.map(|m| strings.intern(m)),
            enum_synth_members: ENUM_SYNTH_MEMBERS.map(|m| strings.intern(m)),
            builtin_types: BUILTIN_TYPES.map(|m| strings.intern(m)),
            setter_ids: FxHashMap::default(),
            primitive_ids: FINAL_PRIMITIVES.map(|m| strings.intern(m)),
            core_supertypes: CORE_SUPERTYPES.map(|(b, p)| (strings.intern(b), strings.intern(p))),
            pending_must_call_super: None,
            equals_check: None,
            nsm_check: None,
        }
    }

    /// The `==` latch's parameter-list progress: `begin_formal_parameters`
    /// nesting (the outermost list is the method's own; function-typed
    /// formals nest theirs inside it).
    pub(crate) fn equals_formal_boundary(&mut self) {
        let Some(check) = &mut self.equals_check else {
            return;
        };
        check.depth += 1;
    }

    /// `handle_type` completed an annotation inside the method's own
    /// parameter list: the pending first-formal read may consume it.
    pub(crate) fn equals_mark_type(&mut self) {
        let Some(check) = &mut self.equals_check else {
            return;
        };
        if check.depth >= 1 && check.first_ty.is_none() {
            check.ty_seen = true;
        }
    }

    /// The `==` first-formal type question is unanswerable past a
    /// function/record type shape (the written heads can never read as a
    /// plain `Object?` annotation; the official's resolved type is not
    /// `dartCoreObject` there either).
    pub(crate) fn equals_type_poison(&mut self) {
        let Some(check) = &mut self.equals_check else {
            return;
        };
        if check.depth >= 1 && check.first_ty.is_none() {
            check.poisoned = true;
        }
    }

    /// Breaks the `noSuchMethod` shape: any structural expression event
    /// outside the argument window that is not part of
    /// `super . noSuchMethod (` kills it (the official matches ONE
    /// `ReceiverMethodInvocation` node — compound expressions never
    /// match). Poison applies in the expression face and inside a return
    /// statement's expression; argument content is opaque.
    pub(crate) fn nsm_poison(&mut self) {
        let Some(check) = &mut self.nsm_check else {
            return;
        };
        if !check.armed || (check.body_block.is_some() && !check.in_return) {
            return;
        }
        if !matches!(check.shape, NsmShape::Args { .. } | NsmShape::Dead) {
            check.shape = NsmShape::Dead;
        }
    }

    /// The shape machine's `super` transition.
    pub(crate) fn nsm_super(&mut self) {
        let Some(check) = &mut self.nsm_check else {
            return;
        };
        if !check.armed {
            return;
        }
        if check.shape == NsmShape::Open {
            check.name_ok = false;
            check.shape = NsmShape::Super;
        } else if !matches!(check.shape, NsmShape::Args { .. }) {
            check.shape = NsmShape::Dead;
        }
    }

    /// The selector's member name at the identifier event (`super` face
    /// only — the name precedes the send): latches whether the dot event
    /// will validate.
    pub(crate) fn nsm_member_name(&mut self, hits: bool) {
        let Some(check) = &mut self.nsm_check else {
            return;
        };
        if !check.armed || check.shape != NsmShape::Super {
            return;
        }
        check.name_ok = hits;
    }

    /// The shape machine's `.` validation (`end` is the member name
    /// token). The parser fires the selector's dot AFTER its send for the
    /// `super.m(x)` face, so the name check validates the closed send;
    /// argument content stays opaque. The null-aware spelling is never
    /// the shape.
    pub(crate) fn nsm_dot(&mut self, is_null_aware: bool) {
        let Some(check) = &mut self.nsm_check else {
            return;
        };
        if !check.armed {
            return;
        }
        let hits = !is_null_aware && check.name_ok;
        match check.shape {
            NsmShape::Args { .. } | NsmShape::Dead => {}
            NsmShape::Sent { count } if count == 1 && hits => check.shape = NsmShape::Done,
            NsmShape::Super if hits => check.shape = NsmShape::Method,
            _ => check.shape = NsmShape::Dead,
        }
    }

    /// The shape machine's argument-list transitions.
    pub(crate) fn nsm_arguments_open(&mut self) {
        let Some(check) = &mut self.nsm_check else {
            return;
        };
        if !check.armed {
            return;
        }
        check.shape = match check.shape {
            NsmShape::Super | NsmShape::Method => NsmShape::Args { depth: 1 },
            NsmShape::Args { depth } => NsmShape::Args { depth: depth + 1 },
            _ => NsmShape::Dead,
        };
    }

    /// The shape machine's argument-list close: only the outermost list's
    /// count decides (exactly one argument).
    pub(crate) fn nsm_arguments_closed(&mut self, count: usize) {
        let Some(check) = &mut self.nsm_check else {
            return;
        };
        if !check.armed {
            return;
        }
        check.shape = match check.shape {
            NsmShape::Args { depth } if depth > 1 => NsmShape::Args { depth: depth - 1 },
            NsmShape::Args { .. } => NsmShape::Sent {
                count: u32::try_from(count).unwrap_or(u32::MAX),
            },
            _ => NsmShape::Dead,
        };
    }

    /// A return statement inside the method's own body block: resets the
    /// machine for the statement's expression, marks the window (the
    /// block face's poison gate), and latches the candidate's `return`
    /// keyword for the block-face statement-kind gate. Outside a block
    /// body a return statement cannot be part of the shape
    /// (`=> () { return ...; }()` closures).
    pub(crate) fn nsm_return_open(&mut self, offset: u32) {
        let Some(check) = &mut self.nsm_check else {
            return;
        };
        if !check.armed {
            return;
        }
        if check.body_block.is_some() {
            check.in_return = true;
            check.shape = NsmShape::Open;
            if !check.return_seen {
                check.return_kw = Some(offset);
            }
        } else if !matches!(check.shape, NsmShape::Dead) {
            check.shape = NsmShape::Dead;
        }
    }

    /// A return statement's close: the block face latches its candidate —
    /// the FIRST return of the body block only (later returns are other
    /// statements' business; the statement-count gate rules them out).
    pub(crate) fn nsm_return_closed(&mut self, has_expression: bool, semi_end: u32) {
        let Some(check) = &mut self.nsm_check else {
            return;
        };
        if !check.armed {
            return;
        }
        check.in_return = false;
        if check.body_block.is_some() && !check.return_seen {
            check.return_seen = true;
            if has_expression && check.shape == NsmShape::Done {
                check.return_ok = true;
                check.return_semi_end = Some(semi_end);
            }
        }
        check.shape = NsmShape::Open;
    }

    /// The method's own block body opened (`begin_block_function_body`):
    /// the first body-shaped block latches; nested blocks and closure
    /// bodies (in the defaults window, before arming) never do.
    pub(crate) fn nsm_block_open(&mut self, range: SourceRange) {
        let Some(check) = &mut self.nsm_check else {
            return;
        };
        if check.armed && check.body_block.is_none() && check.shape == NsmShape::Open {
            check.body_block = Some(range);
        } else if !check.armed && !matches!(check.shape, NsmShape::Dead) {
            // A block before the formals close is a defaults-window
            // closure's body — poison (a closure body is not the shape).
            check.shape = NsmShape::Dead;
        }
    }

    /// Opens the shape machine to body events (the method's own formals
    /// closed — a getter's absent list counts through
    /// `handle_no_formal_parameters`).
    pub(crate) fn arm_no_such_method(&mut self) {
        if let Some(check) = &mut self.nsm_check {
            check.armed = true;
        }
    }

    /// The send-shaped poison for argless sends: an invocation-completed
    /// shape is the send's own business (`Args`/`Done` stay), a send where
    /// a shape part was expected is broken.
    pub(crate) fn nsm_send_poison(&mut self) {
        let Some(check) = &mut self.nsm_check else {
            return;
        };
        if !check.armed {
            return;
        }
        if matches!(
            check.shape,
            NsmShape::Open | NsmShape::Super | NsmShape::Method
        ) {
            check.shape = NsmShape::Dead;
        }
    }

    /// The `name=` lookup spelling of a write target, interned once per
    /// distinct name (the engine's member checks ask on every write; the
    /// one-line delegate on the engine keeps the call sites unchanged).
    pub(crate) fn setter_spelling(&mut self, strings: &StringTable, name: StringId) -> StringId {
        if let Some(&spelling) = self.setter_ids.get(&name) {
            return spelling;
        }
        let text = strings.resolve(name);
        let spelling = strings.intern(&format!("{text}="));
        self.setter_ids.insert(name, spelling);
        spelling
    }
}
