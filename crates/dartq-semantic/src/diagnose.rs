//! The M1 semantic diagnosis engine: the frozen `undefined_*` /
//! `duplicate_definition` family (batch 1, #120) plus the
//! `wrong_number_of_*` family and the static-nullability four (batch 2,
//! #121) — a pass over the parser's event stream that resolves every
//! reference a unit makes against the #117 lookup surface and the #119
//! element model, and emits the implemented frozen rows as [`Diagnostic`]s.
//!
//! # Architecture
//!
//! One parse with a reference-collecting listener (declaration capture stays
//! in [`crate::collector::collect`] — the model and the diagnostic surface
//! share the parser event stream, not the listener). The listener keeps an
//! expression row stack (the same shape the AST builder reconstructs from
//! these events): bare identifiers, member accesses merged at the dot event,
//! typed receivers (`this`/`super`/container names/declaration-typed names),
//! and per-row access intent (read / write / invoke / operator) finalized by
//! the send/assignment/operator events. Rows snapshot their resolution
//! verdict and declared type at push time (the frames tear down before
//! pass 2) and resolve in source order. Declaration-surface diagnostics —
//! `duplicate_definition` and the arity log — fold into the report from the
//! collector's capture-time logs.
//!
//! # Batch 2: arity and static nullability (#121)
//!
//! The type-argument family keys off `begin/end_type_arguments`, with the
//! code chosen by the use-site shape (the pinned oracle registry,
//! `.third/dart-sdk` `diagnostic.g.dart`):
//! a pending *type head* (a `TypeReference` identifier that has not seen
//! its `handle_type` yet) makes the group an annotation or a
//! creation type reference → the base code; a head consumed inside an
//! enum constant → `_enum`; an expression head under `new`/`const`/
//! implicit-creation position → base for a resolved container, `_constructor`
//! for a named constructor segment; any other expression head defers to the
//! send or to the type-argument application event, both of which report
//! `_element` for a resolved executable — the message names the
//! executable's oracle kind word, `function` or `method`
//! (`type_instantiation_target.dart`,
//! `InvocationTargetExecutableElement`) — or `_extension` for extension
//! overrides. The `_function` row (a function-typed *expression*
//! instantiation) displays the function type, a shape M1 cannot render, so
//! it has no emit site. Only resolvable *functions and methods* produce
//! counts — external (`dart:`) and unresolved heads suppress, and
//! getter-shaped callees (whose oracle kind word M1 does not model)
//! suppress too: the under-report direction.
//!
//! The static-nullability checks decide on declared types and literal
//! types (types-nullability-spec §2–§3): the engine tracks the declared
//! type of locals, formals, fields, top-level variables, and return types
//! (armed by `handle_type`/`handle_void_keyword`, disarmed by
//! `handle_no_type` — every declaration head runs one `parse_type`, so the
//! arm cannot leak), and types expressions minimally: the `null` literal is
//! `Null`, the string/int/double/bool literals carry their final primitives,
//! a name with a captured declared type is that type, everything else is
//! `Unknown` — the oracle's `InvalidType`, which suppresses every question
//! it touches. The decidable set is the nullability decisions
//! (`null`/`T?` into a non-nullable target) plus the final-primitive pairs
//! ([`FINAL_PRIMITIVES`]), and `?.`/`?[`/`?..` on a strictly non-nullable
//! receiver (`invalid_null_aware_operator`, a `STATIC_WARNING`).
//! Assignment/initializer/argument/return checks apply only to single-row
//! expressions (one pushed row) so compound operands never lend their type
//! to the whole expression.
//!
//! # The frozen-family emit matrix (batch 1 #120, batch 2 #197)
//!
//! The batch-1 core (bare names, type positions, prefixed invocations,
//! `duplicate_definition`) plus the batch-2 member family cover the frozen
//! set's live triggers. Every member emission keys off the access intent and
//! the receiver domain, mirroring the oracle's single-point dispatch
//! (`.third/dart-sdk` `lookup_failure_reporter.dart`, domains × read/write ×
//! read-syntax):
//!
//! * instance receivers (`c.m` on a `C c`, `this.m`) report
//!   `undefined_method` / `undefined_getter` / `undefined_setter`
//!   (`InstanceLookupDomain`); a write survives when any same-named
//!   declaration exists (the oracle's assignment-to-* family) or a setter
//!   spelling (`name=`) defines it;
//! * the static face (`C.m`, `E.x`, `Ext.g`) reports the same interface
//!   trio — the type-literal invocation shape shares the `undefined_method`
//!   registry name and message — plus `undefined_enum_constant` on enum
//!   reads, and the extension trio on the extension face
//!   (`StaticLookupDomain`);
//! * `super.member` reports `undefined_super_member` with the per-shape
//!   message (`SuperLookupDomain`; one analyzerCode, four texts);
//! * prefixed reads and writes report `undefined_prefixed_name`
//!   (`PrefixedLookupDomain`);
//! * a bare invocation miss inside a class or enum instance member reports
//!   `undefined_method` naming `this`'s type (`UnqualifiedLookupDomain`);
//! * a named argument label on a resolved callee reports
//!   `undefined_named_parameter` (`type_analyzer.dart:5820` — an unresolved
//!   callee stays silent, its own invocation code reports);
//! * `await` in a sync body reports `undefined_identifier_await`;
//! * user-definable operators report `undefined_operator` at the operator
//!   token (index accesses at the bracket pair) when the receiver's
//!   declared type — direct (`c + d`, `c[i]`) or a member result
//!   (`c.j + 1`, `c.j[i] = v`) — genuinely lacks the read/write spelling.
//!
//! Under-reported by adjudication (each emits no code): operator intents on
//! static faces and prefix receivers, index chains through unknown
//! receivers (`f()[i]`), and `this[i]`-style accesses inside extension and
//! mixin bodies, whose `on` faces the model cannot see.
//!
//! # The names table seam (index-design-spec §8)
//!
//! Unresolved names are a **byproduct of lookup**, written into
//! [`FileReport::names`] — the same rows the §8 index names table wants
//! (`(name, kind, range, qualified)`), kept here until #122 mounts
//! `dartq-index` and the table moves behind it. Resolved references are not
//! recorded: they are the reference index's rows, not the recovery surface's.
//!
//! # The undefined guard (analyzer `shouldIgnoreUndefined`)
//!
//! `element.dart:8217-8250`: a name referenced through an import whose target
//! did not resolve is suppressed — but the filter face is show-list-only. The
//! mirror reads the graph's import edges (`target: None`) and suppresses a
//! name listed in a broken import's `show` clause, or a prefixed name whose
//! broken import has no `show` clause; a bare miss under an unfiltered or
//! hide-only broken import still reports ([`UndefinedGuard`]). The
//! `_$`-generated-part clause needs the part tree; the parser has no part
//! events yet, so it is deferred with the directive milestone (the clause
//! only ever suppresses). Harnesses model core-library names through
//! concrete declarations or shown broken edges.
//!
//! # False-positive defenses (each mirrors a verified analyzer behavior)
//!
//! * Locals, parameters, catch/pattern variables, and type parameters are
//!   scope-tracked in this pass (the model does not capture them) — a body
//!   reference to a local never reaches the library lookup.
//! * `dynamic` is a language builtin, not an import — never `undefined_class`.
//! * `await` as a bare read/type name inside an executable reports
//!   `undefined_identifier_await` instead (dart-sdk main
//!   `named_type_resolver.dart:815`, `property_element_resolver.dart:2143`).
//! * `is`/`as` type operands and type arguments carry other oracle codes
//!   (`type_test_with_undefined_name`, `cast_to_non_type`,
//!   `non_type_as_type_argument`) — none frozen, so this pass under-reports
//!   them rather than emitting a wrong code.
//! * Member walks distinguish a cleanly walked chain (member genuinely
//!   absent) from a broken one (an unresolvable superclass — external type or
//!   mixin `on` constraint) and record only the former; enum synthesized
//!   members (`values`/`index`/`name`) and `Object` members are defined on
//!   every receiver shape the walk can reach.
//! * Member checks need a *declaration* receiver type (`this`, `super`, an
//!   extension or class name, or a name whose declared type is an indexed
//!   container — M1 deliberately under-reports inferred receivers,
//!   index-design-spec §12.2). A named constructor is invisible to the model,
//!   so `@Class.member` second-part checks are suppressed for classes
//!   (extensions have no constructors and stay checked).
//! * Constructor invocations are outside the model: a class-name callee is
//!   defined without inspection, and `new`/`const`-position callees resolve
//!   as type references (the oracle's `undefined_class`), never invocations —
//!   their constructor-family codes stay deferred. Implicit creations
//!   (`C<T>(...)` without `new`) bracket the same suppression window.
//! * Locals leak only where the parser offers no frame pairing (switch-case
//!   bodies share the switch block's frame) — a leak can only mask a miss
//!   (under-report), never fabricate one.
//!
//! # The #350 gate closure (the remaining frozen rows)
//!
//! * Assignability trio: literals carry their final-primitive types and
//!   [`Engine::assignable`] extends [`Ty::assignable_from`] with the
//!   final-primitive table (a value of one final primitive never fits
//!   another; `int`/`double` fit `num`; `int`→`double` stays undecided —
//!   the oracle's literal conversion is below the M1 facts).
//! * `undefined_annotation`: a fully missing `MetadataReference` name,
//!   anchored over the `@` (the 3.13.1 caliber — removed upstream in 3.14).
//! * `undefined_constructor_in_initializer`: `super.named()` inside a
//!   constructor-initializer list checks the superclass's constructor face
//!   (captured per container: named selectors + the unnamed flag) and folds
//!   the row before pass 2's super-member dispatch can mis-report it.
//! * `undefined_enum_constructor`: an enum constant's constructor selector
//!   checks the enum's constructor face.
//! * The extension-override face: a container-callee send (`E('x')`,
//!   `C()`) folds into a [`Receiver::Created`] row, so member and operator
//!   accesses resolve against the instance-shaped face — extensions report
//!   their dedicated trio and operator row, class creations walk the
//!   superclass chain with the `Object` members defined.
//! * The external-primitive member face: a receiver type that resolves to
//!   no container but names a dart:core primitive checks
//!   [`CORE_TYPE_MEMBERS`] instead of suppressing.
//! * The `_function` tear-off row renders the function type from the
//!   captured member signature (type-parameter names joined the model);
//!   `C<...>.named`-shaped constructor references fall back to the base
//!   type-argument code. Both fire from the type-argument application
//!   event — the current parser consumes `id.id<...>` as a type head
//!   (#351's gap), so the emission activates when that parse lands.
//!
//! # Still deferred
//!
//! `undefined_referenced_parameter` (`@UseResult(unless:)` check,
//! `annotation_verifier.dart:519-541`), `undefined_hidden_name` /
//! `undefined_shown_name` (directives are consumed diet-style and the
//! combinator model carries clause names without source spans — both arrive
//! with the directive milestone), the unnamed-selector message variants of
//! the two constructor rows, and the `int`→`double` assignability
//! decision. `wrong_number_of_parameters_for_setter` is emitted here although
//! the oracle fires it from the parser: dartq's parser does not emit the code
//! yet, and when it does, the collector-side emission must be removed or the
//! code double-reports. Every deferral under-reports.

use std::cell::RefCell;
use std::rc::Rc;

use dartq_diagnostics::{Diagnostic, Severity};
use dartq_parser::{
    DeclarationKind, IdentifierContext, Parser, ParserFeatures, ParserListener, TokenSpan as Token,
};
use dartq_resolution::{ImportEdge, LibraryGraph, LibraryId, NamespaceEntry, Namespaces};
use dartq_scanner::TokenTable;
use dartq_source::{FileId, SourceRange, StringId, StringTable};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::codes::{ExecKind, SemanticCode, SuperShape};
use crate::library::{ElementIndex, ScopedHit, lookup_scoped, resolve_container};
use crate::model::{
    ArityViolation, Container, ContainerKind, FileElements, Formals, MemberEntry, MemberKind,
    MicroKind, TopDecl, Ty,
};
use dartq_types::{
    Case, Exhaustiveness, ExhaustivenessWorld, Key, Single, Space, Witness as EngineWitness,
    compute,
};

/// Members of `dart:core` `Object`: defined on every class-shaped receiver
/// the M1 walk can reach (the superclass chain ends at an unindexed root).
const OBJECT_MEMBERS: [&str; 5] = ["==", "hashCode", "toString", "noSuchMethod", "runtimeType"];

/// Synthesized enum members absent from the model; suppressed on enum
/// receivers so the member walk cannot false-positive on them.
const ENUM_SYNTH_MEMBERS: [&str; 3] = ["values", "index", "name"];

/// Builtin type names: always defined, independent of any import.
const BUILTIN_TYPES: [&str; 1] = ["dynamic"];

/// The dart:core primitive types whose member face the engine answers
/// directly: their declarations are external (the library graph carries
/// their top-level names only), so a member walk chain-breaks — but the
/// oracle still reports a genuinely absent member on them. The listed
/// members (instance getters/methods; setters none of them has) keep the
/// check on the oracle's side: a name on the list is defined, everything
/// else reports. Derived from the reference SDK (`sdk/lib/core/*.dart`).
const CORE_TYPE_MEMBERS: [(&str, &[&str]); 5] = [
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
];

/// The dart:core primitives whose assignability the name level decides: a
/// value typed by one of these names never fits another (the types are
/// final — no user subclass or implementation can close the gap). `num` is
/// the one supertype edge (`int`/`double` fit it).
const FINAL_PRIMITIVES: [&str; 5] = ["int", "double", "num", "bool", "String"];

/// User-definable binary operators (operator-name spellings the model keeps).
const USER_OPERATORS: [&str; 16] = [
    "+", "-", "*", "/", "%", "~/", "|", "&", "^", "<<", ">>", ">>>", "<", ">", "<=", ">=",
];

/// The reference shape a recorded name stands for (the §8 names-table `kind`
/// column).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameUse {
    /// Value read.
    Read,
    /// Assignment target.
    Write,
    /// Invocation callee.
    Invoke,
    /// Type annotation position.
    Type,
    /// Metadata annotation position.
    Annotation,
    /// Named argument label.
    NamedArgument,
}

/// One unresolved reference: the byproduct row the index names table consumes
/// (index-design-spec §8). Recorded exactly when a lookup ran and missed —
/// diagnosed or suppressed — never for resolved references.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NameRow {
    /// The referenced name (member/prefixed uses keep the member spelling).
    pub name: StringId,
    /// The reference shape.
    pub use_kind: NameUse,
    /// Source span of the reference.
    pub range: SourceRange,
    /// Whether the reference was qualified (member access or prefix).
    pub qualified: bool,
}

/// The diagnosis of one compilation unit: emitted diagnostics plus the
/// unresolved-name byproduct table.
#[derive(Debug, Default)]
pub struct FileReport {
    /// Frozen-family diagnostics, duplicates first, then in source order.
    pub diagnostics: Vec<Diagnostic>,
    /// Unresolved-name rows (the §8 recovery surface), in source order.
    pub names: Vec<NameRow>,
}

/// The `shouldIgnoreUndefined` mirror (`element.dart:8217-8250`) over the
/// library's import edges: a name referenced through an import whose target
/// did not resolve is suppressed, because the missing target is the reported
/// root cause. The filter face is show-list-only — only `show` clauses
/// qualify a name, so a bare miss under an unfiltered (or hide-only) broken
/// import still reports.
#[derive(Clone, Copy, Debug)]
pub struct UndefinedGuard<'a> {
    edges: &'a [ImportEdge],
}

impl<'a> UndefinedGuard<'a> {
    /// Builds the guard over `lib`'s import edges.
    #[must_use]
    pub fn build(graph: &'a LibraryGraph, lib: LibraryId) -> Self {
        Self {
            edges: graph.import_edges(lib),
        }
    }

    /// Whether a reference to `name` (optionally through `prefix`) must not
    /// be reported as undefined.
    ///
    /// Mirrors the oracle loop (`element.dart:8217-8250`) over the broken
    /// imports matching the prefix: the name is suppressed when it sits in a
    /// `show` list (`shownNames.contains`, hide clauses never consulted), or
    /// when it is prefixed and the broken import has no `show` clause at
    /// all. Everything else — a bare miss under an unfiltered or hide-only
    /// broken import, a prefixed name absent from the show list — reports.
    /// The `_$`-generated-part clause is deferred with the directive
    /// milestone (it only ever suppresses; see the module docs).
    #[must_use]
    pub fn ignore(&self, prefix: Option<StringId>, name: StringId) -> bool {
        for edge in self.edges {
            if edge.target.is_some() || edge.prefix != prefix {
                continue;
            }
            if edge.combinators.shows(name) {
                return true;
            }
            if prefix.is_some() && !edge.combinators.has_show() {
                return true;
            }
        }
        false
    }
}

/// What a bare-name hit looked like — the receiver classification a later
/// member access consumes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BareInfo {
    /// Defined, and a named container (static/extension/enum access face).
    Container { name: StringId, kind: ContainerKind },
    /// Defined as an import prefix.
    Prefix { name: StringId },
    /// Defined, but not a receiver face; `ty` is the declared type when the
    /// model captured one (the static-nullability checks read it).
    Plain { ty: Ty },
    /// Not defined (the row's intent decides the emitted code). The scope
    /// context is snapshotted at push time — pass 2 runs after the frames
    /// tear down:
    Missing {
        /// `this`'s type when the miss sits inside a class or enum instance
        /// member — the `undefined_method` host of a bare invocation miss.
        this_type: Option<StringId>,
        /// The miss sits inside some executable body (the `await` trigger).
        in_exec: bool,
        /// The enclosing container's face cannot answer `on`-constraint
        /// members (mixin/extension bodies): a bare miss may be defined.
        on_opaque: bool,
    },
}

/// Per-row access intent, finalized by the send/assignment/operator events.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Intent {
    /// Tentative read; an assignment upgrade turns it into [`Intent::Write`]
    /// or [`Intent::ReadWrite`].
    Read,
    Write,
    ReadWrite,
    Invoke,
    /// Operator-shaped access (`a + b`, `a[i]`): the operator spelling and
    /// the report span (the binary operator token, or the bracket pair for
    /// index accesses — the oracle's `undefined_operator` anchors).
    Operator(StringId, SourceRange),
}

/// The write form an assignment applied to an operator row (`a[i] = v`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpWrite {
    /// Plain `=`: the write-side operator (`[]=` for index rows).
    Assign,
    /// Compound (`+=`): a read of the operator plus its write side.
    Compound,
}

/// A typed receiver recorded at member-merge time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Receiver {
    /// `this.m` inside a container member.
    This {
        container: StringId,
        kind: ContainerKind,
    },
    /// `super.m` inside a container member.
    Super {
        container: StringId,
        kind: ContainerKind,
    },
    /// A container name used as a receiver (`C.m`, `E.x`, `Ext.g`).
    Container { name: StringId, kind: ContainerKind },
    /// An import prefix (`p.m`).
    Prefix { name: StringId },
    /// A declaration-typed name (`C c; c.m()`); `ty` is the declared type
    /// the static-nullability checks read.
    Typed { type_name: StringId, ty: Ty },
    /// A folded creation/extension-override result (`C()`, `E('x')`): the
    /// container the send resolves against, with the instance-shaped face —
    /// superclass chains and `Object` members apply, the static-face
    /// dispatch does not.
    Created { name: StringId, kind: ContainerKind },
    /// Unknown type — checks are skipped (under-report).
    Unknown,
}

/// One recorded expression reference.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    name: StringId,
    range: SourceRange,
    qualified: bool,
    shape: RowShape,
    intent: Intent,
    /// The write form an assignment applied to an operator row (`a[i] = v`).
    write_op: Option<OpWrite>,
    /// The row's expression type as far as M1 knows it (push-time snapshot:
    /// `Null` for the `null` literal, a captured declared type for names,
    /// `Unknown` — the suppress-everything sentinel — for everything else).
    ty: Ty,
}

/// The structural shape of a row.
#[derive(Clone, Debug, PartialEq, Eq)]
enum RowShape {
    /// A bare identifier, resolved at push time.
    Bare(BareInfo),
    /// A `receiver.member` access, merged at the dot event.
    Member { recv: Receiver },
    /// A typed receiver awaiting its member (`this`, `super`).
    Receiver(Receiver),
    /// A type-annotation position, verdict snapshotted at push time, plus
    /// the `is`/`as` suppression flag.
    TypeUse(TypeVerdict, bool),
    /// A resolved non-receiver value (call results, literals).
    Value,
}

/// Scope stack frame.
struct Frame {
    kind: FrameKind,
    /// Names declared directly in this frame's scope.
    locals: FxHashSet<StringId>,
    /// Declared-type bindings: name → declared type.
    var_types: FxHashMap<StringId, Ty>,
    /// `type_params` length to truncate back to when this frame closes.
    type_mark: usize,
    /// The row-stack length when this frame opened: type parameters that
    /// join mid-header (a method's own `<T>`, parsed after its return type)
    /// rewrite their not-yet-scoped `undefined_class` rows only above this
    /// mark, never an outer declaration's stale row.
    row_mark: usize,
}

enum FrameKind {
    /// A class/mixin/enum/extension body: fields and the member face.
    Container { name: StringId, kind: ContainerKind },
    /// An executable (method, constructor, function, function literal);
    /// `ret` carries the declared return type, the oracle host word, and the
    /// host name when the declaration shape exposes them (constructors and
    /// local functions stay `None` — their return checks under-report).
    Executable {
        instance: bool,
        ret: Option<ExecReturn>,
    },
    /// A statement block, catch clause, or typedef body.
    Block,
}

/// The return-check host of an executable frame (`return_of_invalid_type`'s
/// per-host message picks the word and the name).
struct ExecReturn {
    ty: Ty,
    host: ExecKind,
    name: StringId,
}

/// One open argument list: the arguments collected so far (name, type,
/// range — the range of the argument's expression row) plus the argument
/// currently being parsed. `mark` is the row-stack index where the current
/// argument's rows begin — the parser fires the positional-argument event
/// *after* the expression (rows already pushed) and the named-argument
/// events *before* the value, so the window from `mark` to the row count is
/// the event-order-agnostic shape of one argument.
struct ArgLevel {
    args: Vec<(Option<StringId>, Ty, SourceRange)>,
    /// An argument is open between its `open_argument` and the next flush.
    arg_open: bool,
    /// The open argument's name (`None` = positional).
    pending_name: Option<StringId>,
    mark: usize,
}

/// The member-walk outcome: found, genuinely absent after a clean walk, or a
/// broken chain (an unresolvable container hop — suppression territory).
enum Walk<'a> {
    Found(&'a crate::model::MemberEntry),
    Missing,
    ChainBroken,
}

/// A qualified-type prefix awaiting its name (`p` of `p.C`); dead when the
/// prefix itself already reported and the name must not.
struct TypePrefix {
    prefix: StringId,
    live: bool,
}

/// The push-time verdict of a type-annotation position. Rows resolve after
/// the scope frames tear down, so the verdict snapshots what the scope stack
/// said while the type was in context. Defined types push no row at all (a
/// non-type used as a type carries an unfrozen oracle code — no row, no
/// report).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TypeVerdict {
    /// The head `p` of `p.C`, not defined: `undefined_identifier` at the
    /// head (guarded); the continuation contributes nothing.
    PrefixHead,
    /// `p.C` with `p` defined and `C` absent from the prefix: the guarded
    /// miss is `undefined_class` at `C`.
    Prefixed { prefix: StringId },
    /// Not defined anywhere: `undefined_class` unless the position is
    /// suppressed (`is`/`as` operands carry other oracle codes).
    Missing,
}

/// Which part of a type annotation an identifier occupies. The prefix head
/// takes the dedicated [`Engine::arm_type_prefix`] path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TypeHead {
    /// A bare type name (`Foo`).
    Bare,
    /// The name after the dot of a qualified type (`C` of `p.C`).
    Continuation,
}

/// The engine: one parse, then the resolution pass.
struct Engine<'a> {
    graph: &'a LibraryGraph,
    namespaces: &'a Namespaces,
    index: &'a ElementIndex,
    strings: &'a StringTable,
    source: &'a str,
    tokens: &'a TokenTable,
    file: FileId,
    lib: LibraryId,
    guard: UndefinedGuard<'a>,
    report: FileReport,
    rows: Vec<Row>,
    frames: Vec<Frame>,
    /// In-scope type parameters, outermost first (truncated per frame).
    type_params: Vec<StringId>,
    /// Type variables declared since the last frame opened. A declaration's
    /// header events fire before its frame (`begin_class_declaration` opens
    /// after the type variables, official `parseClassOrNamedMixinApplication`),
    /// so they park here and drain into the frame that owns them.
    pending_type_params: Vec<StringId>,
    /// The armed declared type: `handle_type`/`handle_void_keyword` arm,
    /// `handle_no_type` disarms. Consumed by declaration bindings and by the
    /// executable return-type latches.
    armed_ty: Option<Ty>,
    /// The open variable-declaration group's declared type (one
    /// `parse_type` per group), for the initializer checks.
    pending_decl_ty: Option<Ty>,
    /// Per open type-argument group: the type head it belongs to, when the
    /// group follows a type-annotation identifier (`C<int> x`, `const
    /// C<int>()`, `p.C<int>`). A group without a head belongs to an
    /// expression (`g<int>()`, `g<int>`).
    group_heads: Vec<Option<(StringId, SourceRange)>>,
    /// The type head awaiting its group (`C` of `C<int>`), armed at the
    /// identifier and moved into `group_heads` at `begin_type_arguments`.
    pending_type_head: Option<(StringId, SourceRange)>,
    /// Expression type-argument groups awaiting their use: the callee row
    /// index, the given count, and the group span.
    deferred_args: Vec<(usize, u16, SourceRange)>,
    /// Per open argument list, the argument types collected so far.
    arg_levels: Vec<ArgLevel>,
    /// The enclosing enum constant's declared type-parameter count (the
    /// `_enum` code's declared side).
    in_enum_element: Option<u16>,
    /// The row-stack mark where the current function body's expression
    /// began (the arrow-body return check reads it).
    body_mark: Option<usize>,
    /// Open switch contexts (statements and expressions): the scrutinee's
    /// push-time type, the cases completed so far, and the anchors the
    /// diagnostics attach to.
    switches: Vec<SwitchCtx>,
    /// The open pattern-nesting stack (`begin_pattern`/`end_pattern`
    /// brackets); each frame collects the leaf restrictions it spans.
    pattern_stack: Vec<PatternFrame>,
    /// The number of currently open patterns.
    pattern_depth: usize,
    /// Completed root restrictions per open case (switch cases only).
    pattern_roots: Vec<Restriction>,
    /// A switch-case header's pattern window is open
    /// (`begin_case_expression`/`begin_switch_expression_case` through the
    /// pattern's completion): the only window whose root pattern feeds
    /// exhaustiveness. Roots outside it — if-case, pattern assignments —
    /// are not coverage questions and never enter the set.
    case_sink_open: bool,
    /// The in-flight constant pattern's classification, armed by the
    /// literal/identifier events inside it.
    pending_const: Option<Restriction>,
    /// The `switch` keyword of the open switch statement/expression (the
    /// non-exhaustive anchor; the block events only carry the `{`).
    switch_kw_latch: Option<SourceRange>,
    /// The pending object-pattern type name (the first identifier).
    pending_object_ty: Option<StringId>,
    /// The row-stack mark where the current variable initializer's
    /// expression began (the initializer check reads it).
    init_mark: Option<usize>,
    /// The row-stack mark where the current return statement's expression
    /// began (the return check reads it).
    return_mark: Option<usize>,
    /// Inside a cascade (`a?..b` picks the `?..` message shape).
    in_cascade: bool,
    /// Per open argument list: the callee row index, when identifiable
    /// (`None` while inside a `new`/`const`/implicit-creation reference —
    /// its class name is a type use, never an invocation callee).
    invokes: Vec<Option<usize>>,
    /// The argument group that just closed, awaiting the send event that
    /// consumes it (an argless send never opens a group, so a bare read
    /// stays a read).
    pending_send: Option<usize>,
    type_prefix: Option<TypePrefix>,
    /// Suppression depth inside `is`/`as` type operands.
    non_type_pos: u32,
    /// Suppression depth inside `new`/`const`/implicit-creation expressions.
    ctor_pos: u32,
    /// Interned reserved-member spellings (populated once in [`Engine::new`]:
    /// the hot `resolve_bare` path compares ids, never re-interns).
    object_members: [StringId; OBJECT_MEMBERS.len()],
    enum_synth_members: [StringId; ENUM_SYNTH_MEMBERS.len()],
    builtin_types: [StringId; BUILTIN_TYPES.len()],
    /// `name` → `name=` lookup spellings already interned (write targets
    /// repeat across a unit; the cache is bounded by their distinct names).
    setter_ids: FxHashMap<StringId, StringId>,
    /// The five final primitives' interned names (the assignability table
    /// and the literal types compare ids, never text).
    primitive_ids: [StringId; 5],
    /// The open constructor-initializer list depth (`super.named()` in an
    /// initializer is the `undefined_constructor_in_initializer` shape).
    initializer_depth: u32,
    /// The `@` token of the open metadata entry (the
    /// `undefined_annotation` anchor spans `@` through the name).
    annotation_at: Option<SourceRange>,
}

impl<'a> Engine<'a> {
    // —— plumbing ————————————————————————————————————————————————————

    #[allow(clippy::too_many_arguments)] // the analysis context, one field each
    fn new(
        graph: &'a LibraryGraph,
        namespaces: &'a Namespaces,
        index: &'a ElementIndex,
        strings: &'a StringTable,
        source: &'a str,
        tokens: &'a TokenTable,
        file: FileId,
        lib: LibraryId,
    ) -> Self {
        let guard = UndefinedGuard::build(graph, lib);
        let object_members = OBJECT_MEMBERS.map(|m| strings.intern(m));
        let enum_synth_members = ENUM_SYNTH_MEMBERS.map(|m| strings.intern(m));
        let builtin_types = BUILTIN_TYPES.map(|m| strings.intern(m));
        let primitive_ids = FINAL_PRIMITIVES.map(|m| strings.intern(m));
        Self {
            graph,
            namespaces,
            index,
            strings,
            source,
            tokens,
            file,
            lib,
            guard,
            report: FileReport::default(),
            rows: Vec::new(),
            frames: Vec::new(),
            type_params: Vec::new(),
            pending_type_params: Vec::new(),
            armed_ty: None,
            pending_decl_ty: None,
            group_heads: Vec::new(),
            pending_type_head: None,
            deferred_args: Vec::new(),
            arg_levels: Vec::new(),
            in_enum_element: None,
            body_mark: None,
            init_mark: None,
            return_mark: None,
            switches: Vec::new(),
            pattern_stack: Vec::new(),
            pattern_depth: 0,
            pattern_roots: Vec::new(),
            case_sink_open: false,
            pending_const: None,
            switch_kw_latch: None,
            pending_object_ty: None,
            in_cascade: false,
            invokes: Vec::new(),
            pending_send: None,
            type_prefix: None,
            non_type_pos: 0,
            ctor_pos: 0,
            object_members,
            enum_synth_members,
            builtin_types,
            setter_ids: FxHashMap::default(),
            primitive_ids,
            initializer_depth: 0,
            annotation_at: None,
        }
    }

    fn intern(&self, name: &str) -> StringId {
        self.strings.intern(name)
    }

    /// Token text, `None` for parser-inserted synthetics (not in the table).
    fn text_of(&self, token: Token) -> Option<&'a str> {
        self.tokens
            .get(token.id as usize)
            .map(|t| &self.source[t.byte_range()])
    }

    fn name_of(&self, token: Token) -> Option<StringId> {
        self.text_of(token).map(|text| self.strings.intern(text))
    }

    fn emit(&mut self, code: SemanticCode, range: SourceRange, a0: &str, a1: &str) {
        self.report.diagnostics.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            code.message(a0, a1),
            self.file,
            range,
            None,
        ));
    }

    /// The three-placeholder emit (`message3`).
    fn emit3(&mut self, code: SemanticCode, range: SourceRange, a0: &str, a1: &str, a2: &str) {
        self.report.diagnostics.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            code.message3(a0, a1, a2),
            self.file,
            range,
            None,
        ));
    }

    /// The four-placeholder emit (`message4`).
    fn emit4(
        &mut self,
        code: SemanticCode,
        range: SourceRange,
        a0: &str,
        a1: &str,
        a2: &str,
        a3: &str,
    ) {
        self.report.diagnostics.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            code.message4(a0, a1, a2, a3),
            self.file,
            range,
            None,
        ));
    }

    fn record_miss(&mut self, name: StringId, range: SourceRange, qualified: bool, kind: NameUse) {
        self.report.names.push(NameRow {
            name,
            range,
            qualified,
            use_kind: kind,
        });
    }

    // —— scope frames ————————————————————————————————————————————————

    fn push_frame(&mut self, kind: FrameKind) {
        let type_mark = self.type_params.len();
        let row_mark = self.rows.len();
        let pending = std::mem::take(&mut self.pending_type_params);
        self.type_params.extend(pending);
        self.frames.push(Frame {
            kind,
            locals: FxHashSet::default(),
            var_types: FxHashMap::default(),
            type_mark,
            row_mark,
        });
    }

    fn pop_frame(&mut self) {
        if let Some(frame) = self.frames.pop() {
            self.type_params.truncate(frame.type_mark);
        }
    }

    fn container_context(&self) -> Option<(StringId, ContainerKind)> {
        self.frames.iter().rev().find_map(|f| match &f.kind {
            FrameKind::Container { name, kind } => Some((*name, *kind)),
            _ => None,
        })
    }

    /// Whether the innermost executable is an instance member.
    fn instance_context(&self) -> bool {
        self.frames
            .iter()
            .rev()
            .find_map(|f| match &f.kind {
                FrameKind::Executable { instance, ret: _ } => Some(*instance),
                _ => None,
            })
            .unwrap_or(false)
    }

    fn in_executable(&self) -> bool {
        self.frames
            .iter()
            .any(|f| matches!(f.kind, FrameKind::Executable { .. }))
    }

    /// Binds a local's declared type in the innermost frame (the
    /// declared-variable-pattern binding face).
    fn bind_var_type(&mut self, name: StringId, ty: Ty) {
        if let Some(frame) = self.frames.last_mut() {
            frame.var_types.insert(name, ty);
        }
    }

    /// Classifies a constant pattern whose events left no literal: a
    /// qualified `E.c` tail (enum container + member row) becomes the
    /// enum-value leaf; everything else is unrepresentable.
    fn classify_constant_tail(&self) -> Restriction {
        // The member push replaces the qualifier row, so the `E` of `E.c`
        // survives only as the member row's container receiver.
        let Some(last) = self.rows.last() else {
            return Restriction::Unknown;
        };
        let RowShape::Member { recv } = &last.shape else {
            return Restriction::Unknown;
        };
        match recv {
            Receiver::Container {
                name,
                kind: ContainerKind::Enum,
            } => Restriction::EnumValue(*name, last.name),
            _ => Restriction::Unknown,
        }
    }

    fn add_local(&mut self, name: StringId) {
        if let Some(frame) = self.frames.last_mut() {
            frame.locals.insert(name);
        }
    }

    /// Binds the open declaration group's type to `name` in the innermost
    /// frame (one `parse_type` arms the group; every name in it binds).
    fn bind_declared_type(&mut self, name: StringId) {
        let ty = self.pending_decl_ty.unwrap_or(Ty::UNKNOWN);
        if let Some(frame) = self.frames.last_mut() {
            frame.var_types.insert(name, ty);
        }
    }

    /// Binds a formal or catch parameter's own declared type (one arm per
    /// parameter, not shared with a group).
    fn bind_parameter_type(&mut self, name: StringId) {
        let ty = self.armed_ty.take().unwrap_or(Ty::UNKNOWN);
        if let Some(frame) = self.frames.last_mut() {
            frame.var_types.insert(name, ty);
        }
    }

    /// A container head: the member face and the `this`/`super` receiver for
    /// its body. Unnamed extensions get a placeholder face — their container
    /// name resolves to nothing, so member walks through them suppress.
    fn push_container_frame(&mut self, kind: ContainerKind, name: Option<StringId>) {
        self.push_frame(FrameKind::Container {
            name: name.unwrap_or_else(|| self.intern("")),
            kind,
        });
    }

    /// An executable head: formals and its own type parameters live inside.
    fn push_executable_frame(&mut self, instance: bool) {
        self.push_frame(FrameKind::Executable {
            instance,
            ret: None,
        });
    }

    /// Latches the armed declared type as the innermost executable's return
    /// type (`begin_method`/`begin_top_level_method` fire before the return
    /// type's `parse_type`; the name identifier fires after it).
    fn latch_return_type(&mut self, host: ExecKind, name: StringId) {
        let ty = self.armed_ty.take().unwrap_or(Ty::UNKNOWN);
        if let Some(frame) = self.frames.last_mut()
            && let FrameKind::Executable { ret, .. } = &mut frame.kind
            && ret.is_none()
        {
            *ret = Some(ExecReturn { ty, host, name });
        }
    }

    /// A statement block, catch clause, for body, or typedef scope.
    fn push_block_frame(&mut self) {
        self.push_frame(FrameKind::Block);
    }

    /// Enters a type parameter's scope. A declaration whose frame is already
    /// open (a method's own `<T>` parses after its return type; function
    /// expressions open before theirs) joins the frame's `type_params`
    /// directly — the frame's `type_mark` truncates it on pop — and
    /// re-scopes the signature type rows that resolved `Missing` before the
    /// parameter existed. Container headers parse their variables before
    /// `begin_*_declaration`, so those park for the frame that owns them.
    fn push_type_parameter(&mut self, name: StringId) {
        let Some(mark) = self.frames.last().map(|f| f.row_mark) else {
            self.pending_type_params.push(name);
            return;
        };
        self.type_params.push(name);
        // The signature rows pushed since the frame opened (`T m<T>(...)`'s
        // return type) never saw this parameter: a still-missing type use of
        // the same name was the parameter all along, not an undefined class.
        for row in &mut self.rows[mark..] {
            if row.name == name
                && matches!(row.shape, RowShape::TypeUse(TypeVerdict::Missing, false))
            {
                row.shape = RowShape::Value;
            }
        }
    }

    /// The name after the dot of a qualified type arrived: the prefix slot
    /// is consumed.
    fn consume_type_prefix(&mut self) {
        self.type_prefix = None;
    }

    fn enter_non_type_pos(&mut self) {
        self.non_type_pos += 1;
    }

    fn leave_non_type_pos(&mut self) {
        self.non_type_pos -= 1;
    }

    fn enter_ctor_pos(&mut self) {
        self.ctor_pos += 1;
    }

    fn leave_ctor_pos(&mut self) {
        self.ctor_pos -= 1;
    }

    /// A bare name's declared type, innermost frame first.
    fn typed_local(&self, name: StringId) -> Option<Ty> {
        self.frames
            .iter()
            .rev()
            .find_map(|f| f.var_types.get(&name).copied())
    }

    /// A container member's declared field type from a clean walk
    /// (`Unknown` for executables and broken chains).
    fn member_declared_ty(&self, container: StringId, member: StringId) -> Ty {
        match self.walk_members(container, member) {
            Walk::Found(entry) if entry.kind == MemberKind::Field => entry.var_ty,
            _ => Ty::UNKNOWN,
        }
    }

    /// Whether the formal table declares a named formal under `name`.
    fn has_named_formal(formals: Option<&Formals>, name: StringId) -> bool {
        formals.is_some_and(|f| f.named.iter().any(|formal| formal.name == name))
    }

    /// The resolved top-level declaration under `name` (`None` = no check).
    fn top_level_decl(&self, name: StringId) -> Option<&TopDecl> {
        match lookup_scoped(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        ) {
            ScopedHit::Local(decl) => Some(decl),
            ScopedHit::Found(NamespaceEntry::Decl(file)) => {
                self.index.get(file).and_then(|f| f.decl(name))
            }
            _ => None,
        }
    }

    /// The full formal table of a resolved member callee, walked from `recv`.
    fn member_formals_full(&self, recv: &Receiver, name: StringId) -> Option<&Formals> {
        let (container, super_walk) = match *recv {
            Receiver::This { container, .. } => (container, false),
            Receiver::Super { container, .. } => (container, true),
            Receiver::Typed { type_name, .. } => (type_name, false),
            // A folded creation (`C().m(...)`): the instance face — the
            // constructor-argument membership check walks the superclass
            // chain like a declaration-typed receiver.
            Receiver::Created { name, .. } => (name, false),
            Receiver::Container {
                name: container, ..
            } => {
                let entry = resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    container,
                )
                .and_then(|(c, _)| c.members.get(&name));
                return match entry {
                    Some(entry) if entry.kind == MemberKind::Method => entry.formals.as_deref(),
                    _ => None,
                };
            }
            Receiver::Prefix { .. } | Receiver::Unknown => return None,
        };
        let walk = if super_walk {
            self.walk_super(container, name)
        } else {
            self.walk_members(container, name)
        };
        match walk {
            Walk::Found(entry) if entry.kind == MemberKind::Method => entry.formals.as_deref(),
            _ => None,
        }
    }

    // —— resolution ——————————————————————————————————————————————————

    /// Resolves a bare name in value position: locals, type parameters, the
    /// member face (own-file container chain), the extension pool, enum
    /// synthesized members, then the scoped library lookup.
    fn resolve_bare(&self, name: StringId) -> BareInfo {
        if self.frames.iter().rev().any(|f| f.locals.contains(&name)) {
            return BareInfo::Plain {
                ty: self.typed_local(name).unwrap_or(Ty::UNKNOWN),
            };
        }
        if self.type_params.contains(&name) {
            return BareInfo::Plain {
                ty: Ty::named(name),
            };
        }
        if let Some((container, kind)) = self.container_context() {
            if !matches!(self.walk_members(container, name), Walk::Missing) {
                // Found, or a broken chain — both suppress.
                return BareInfo::Plain {
                    ty: self.member_declared_ty(container, name),
                };
            }
            if kind == ContainerKind::Enum && self.enum_synth_members.contains(&name) {
                return BareInfo::Plain { ty: Ty::UNKNOWN };
            }
            if self.instance_context() && self.object_members.contains(&name) {
                return BareInfo::Plain { ty: Ty::UNKNOWN };
            }
        }
        if self
            .index
            .get(self.file)
            .is_some_and(|f| f.extension_member(name).is_some())
        {
            return BareInfo::Plain { ty: Ty::UNKNOWN };
        }
        match lookup_scoped(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        ) {
            ScopedHit::Local(decl) => self.bare_info_of(name, decl),
            ScopedHit::Found(entry) => self.bare_info_of_entry(name, entry),
            ScopedHit::Ambiguous(_) => BareInfo::Plain { ty: Ty::UNKNOWN },
            ScopedHit::NotFound => self.missing_info(),
        }
    }

    /// The push-time context snapshot of a bare miss (pass 2 runs after the
    /// frames tear down — see [`BareInfo::Missing`]).
    fn missing_info(&self) -> BareInfo {
        let context = self
            .instance_context()
            .then(|| self.container_context())
            .flatten();
        BareInfo::Missing {
            this_type: context
                .filter(|(_, kind)| matches!(kind, ContainerKind::Class | ContainerKind::Enum))
                .map(|(name, _)| name),
            in_exec: self.in_executable(),
            on_opaque: context.is_some_and(|(_, kind)| {
                matches!(kind, ContainerKind::Mixin | ContainerKind::Extension)
            }),
        }
    }

    fn bare_info_of_entry(&self, name: StringId, entry: NamespaceEntry) -> BareInfo {
        match entry {
            NamespaceEntry::Prefix => BareInfo::Prefix { name },
            NamespaceEntry::Decl(file) => {
                let decl = self.index.get(file).and_then(|f| f.decl(name));
                self.bare_info_of(
                    name,
                    decl.map_or(
                        &TopDecl::bare(MicroKind::TopLevelVar),
                        std::convert::identity,
                    ),
                )
            }
        }
    }

    /// The [`BareInfo`] of a resolved top-level declaration.
    fn bare_info_of(&self, name: StringId, decl: &TopDecl) -> BareInfo {
        match decl.kind {
            // Type parameters are declaration-scoped: the per-unit tag is
            // stale outside its declaration, and in-scope uses were answered
            // by the engine's type-parameter stack. The stale reference is a
            // plain miss whose code name follows the live invocation context
            // (`undefined_method`'s host inside a class instance member).
            MicroKind::TypeParam => self.missing_info(),
            MicroKind::Class => BareInfo::Container {
                name,
                kind: ContainerKind::Class,
            },
            MicroKind::Enum => BareInfo::Container {
                name,
                kind: ContainerKind::Enum,
            },
            MicroKind::Mixin => BareInfo::Container {
                name,
                kind: ContainerKind::Mixin,
            },
            MicroKind::Extension => BareInfo::Container {
                name,
                kind: ContainerKind::Extension,
            },
            _ => BareInfo::Plain { ty: decl.var_ty },
        }
    }

    /// Walks `container`'s member face over superclass name edges. Any
    /// unresolvable hop breaks the chain (external type, unresolvable
    /// superclass name): the caller suppresses rather than reports.
    fn walk_members(&self, container: StringId, member: StringId) -> Walk<'a> {
        let mut name = container;
        let mut owner = self.file;
        let mut seen: FxHashSet<StringId> = FxHashSet::default();
        loop {
            if !seen.insert(name) {
                return Walk::ChainBroken;
            }
            let Some((found, found_owner)) = resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                owner,
                name,
            ) else {
                return Walk::ChainBroken;
            };
            if let Some(entry) = found.members.get(&member) {
                return Walk::Found(entry);
            }
            match found.superclass {
                Some(superclass) => {
                    name = superclass;
                    owner = found_owner;
                }
                None => return Walk::Missing,
            }
        }
    }

    /// `super.member`: the walk starts at the enclosing container's direct
    /// superclass (its own members are excluded). A container without a
    /// captured superclass edge (mixins: `on` constraints are not edges)
    /// breaks the chain.
    fn walk_super(&self, container: StringId, member: StringId) -> Walk<'a> {
        let Some(superclass) = self
            .index
            .get(self.file)
            .and_then(|f| f.container(container))
            .and_then(|c| c.superclass)
        else {
            return Walk::ChainBroken;
        };
        self.walk_members(superclass, member)
    }

    // —— row stack ———————————————————————————————————————————————————

    fn push_row(&mut self, row: Row) {
        self.rows.push(row);
    }

    /// A bare identifier in value position: resolved immediately (the
    /// verdict is the snapshot pass 2 emits from). A miss under `new`/`const`
    /// is a creation head: it names a class, so the guarded miss is the
    /// oracle's `undefined_class` (`new Missing()`), never an invocation
    /// read — the row reclassifies as a type use.
    fn push_bare(&mut self, name: StringId, range: SourceRange) {
        let info = self.resolve_bare(name);
        if self.ctor_pos > 0 && matches!(info, BareInfo::Missing { .. }) {
            self.push_row(Row {
                name,
                range,
                qualified: false,
                shape: RowShape::TypeUse(TypeVerdict::Missing, false),
                intent: Intent::Read,
                write_op: None,
                ty: Ty::UNKNOWN,
            });
            return;
        }
        let ty = match info {
            BareInfo::Plain { ty } => ty,
            _ => Ty::UNKNOWN,
        };
        self.push_row(Row {
            name,
            range,
            qualified: false,
            shape: RowShape::Bare(info),
            intent: Intent::Read,
            write_op: None,
            ty,
        });
    }

    /// An identifier after a dot, awaiting the merge at the dot event.
    fn push_member(&mut self, name: StringId, range: SourceRange) {
        self.push_row(Row {
            name,
            range,
            qualified: true,
            shape: RowShape::Bare(BareInfo::Plain { ty: Ty::UNKNOWN }),
            intent: Intent::Read,
            write_op: None,
            ty: Ty::UNKNOWN,
        });
    }

    /// A resolved non-receiver value (literal, parenthesized group, closure,
    /// collection): member merges past it see an unknown receiver. The row
    /// carries the construct's span so the assignment/operator scans can
    /// tell "before the operator" (candidate) from "after" (skip).
    fn push_value_row(&mut self, token: Token) {
        self.push_row(Row {
            name: self.intern(""),
            range: token.range,
            qualified: false,
            shape: RowShape::Value,
            intent: Intent::Read,
            write_op: None,
            ty: Ty::UNKNOWN,
        });
    }

    /// The `null` literal: the only literal whose type M1 knows exactly.
    fn push_null_row(&mut self, token: Token) {
        self.push_row(Row {
            name: self.intern(""),
            range: token.range,
            qualified: false,
            shape: RowShape::Value,
            intent: Intent::Read,
            write_op: None,
            ty: Ty::Null,
        });
    }

    /// `this`/`super` in expression position: a typed receiver when a
    /// container is in scope, unknown otherwise (top-level recovery).
    fn push_keyword_receiver(&mut self, token: Token, is_super: bool) {
        let recv = match self.container_context() {
            Some((container, kind)) => {
                if is_super {
                    Receiver::Super { container, kind }
                } else {
                    Receiver::This { container, kind }
                }
            }
            None => Receiver::Unknown,
        };
        self.push_row(Row {
            name: self.intern(""),
            range: token.range,
            qualified: false,
            shape: RowShape::Receiver(recv),
            intent: Intent::Read,
            write_op: None,
            ty: Ty::UNKNOWN,
        });
    }

    /// A type-annotation position. Defined types push no row; the §8 names
    /// table carries misses only. The declaration binding arms at
    /// `handle_type` now — this only classifies the reference.
    fn push_type_use(&mut self, name: StringId, range: SourceRange, head: TypeHead) {
        let suppressed = self.non_type_pos > 0;
        match head {
            TypeHead::Bare => {
                if self.builtin_types.contains(&name) {
                    return;
                }
                if !matches!(self.resolve_bare(name), BareInfo::Missing { .. }) {
                    return;
                }
                self.push_row(Row {
                    name,
                    range,
                    qualified: false,
                    shape: RowShape::TypeUse(TypeVerdict::Missing, suppressed),
                    intent: Intent::Read,
                    write_op: None,
                    ty: Ty::UNKNOWN,
                });
            }
            TypeHead::Continuation => {
                let Some(tp) = self.type_prefix.as_ref() else {
                    return;
                };
                if !tp.live {
                    return; // the head already reports; the name is dead
                }
                let prefix = tp.prefix;
                let hit = self.prefixed_hit(prefix, name);
                if !hit {
                    self.push_row(Row {
                        name,
                        range,
                        qualified: true,
                        shape: RowShape::TypeUse(TypeVerdict::Prefixed { prefix }, false),
                        intent: Intent::Read,
                        write_op: None,
                        ty: Ty::UNKNOWN,
                    });
                }
            }
        }
    }

    /// Arms the type-prefix slot for a `PrefixedTypeReference` head; a miss
    /// also pushes the head's own report row.
    fn arm_type_prefix(&mut self, name: StringId, range: SourceRange) {
        let live = !matches!(self.resolve_bare(name), BareInfo::Missing { .. });
        self.type_prefix = Some(TypePrefix { prefix: name, live });
        if !live {
            self.push_row(Row {
                name,
                range,
                qualified: false,
                shape: RowShape::TypeUse(TypeVerdict::PrefixHead, false),
                intent: Intent::Read,
                write_op: None,
                ty: Ty::UNKNOWN,
            });
        }
    }

    /// Merges `receiver.member` at the dot/cascade event: pops the member row,
    /// classifies the receiver below it, and pushes the merged row. A
    /// null-aware access on a strictly non-nullable receiver emits
    /// `invalid_null_aware_operator` (a `STATIC_WARNING` — the receiver can
    /// never be null, `type_system.dart:1921-`).
    fn merge_member(&mut self, is_null_aware: bool) {
        let Some(member) = self.rows.pop() else {
            return;
        };
        let receiver_offset = self.rows.last().map(|r| r.range.offset);
        let recv = match self.rows.pop() {
            Some(popped) => match popped.shape {
                RowShape::Bare(info) => Engine::receiver_of_bare(info),
                RowShape::Receiver(recv) => recv,
                _ => Receiver::Unknown,
            },
            None => Receiver::Unknown,
        };
        if is_null_aware {
            let receiver_ty = match recv {
                Receiver::This { container, .. } | Receiver::Super { container, .. } => {
                    Ty::named(container)
                }
                Receiver::Typed { ty, .. } => ty,
                _ => Ty::UNKNOWN,
            };
            if receiver_ty.is_strictly_non_nullable() {
                let (op, replacement): (&'static str, &'static str) = if self.in_cascade {
                    ("?..", "..")
                } else {
                    ("?.", ".")
                };
                self.emit3(
                    SemanticCode::InvalidNullAwareOperator,
                    member.range,
                    op,
                    replacement,
                    "",
                );
            }
        }
        // `super.named()` in a constructor-initializer list is a super
        // *constructor* reference, never a member access: the named shape
        // checks the superclass's constructor face
        // (`undefined_constructor_in_initializer`), and the row folds —
        // pass 2's `super.member` dispatch must never see it. The selector
        // name is the report's second slot; the anchor spans the
        // `super.named()` shape the oracle reports.
        if self.initializer_depth > 0
            && let Receiver::Super { container, .. } = recv
        {
            let span = SourceRange {
                offset: receiver_offset.unwrap_or(member.range.offset),
                length: member.range.end() - receiver_offset.unwrap_or(member.range.offset),
            };
            self.check_super_constructor(container, &member, span);
            self.push_row(Row {
                name: member.name,
                range: member.range,
                qualified: true,
                shape: RowShape::Value,
                intent: Intent::Read,
                write_op: None,
                ty: Ty::UNKNOWN,
            });
            return;
        }
        self.push_row(Row {
            name: member.name,
            range: member.range,
            qualified: true,
            shape: RowShape::Member { recv },
            intent: member.intent,
            write_op: member.write_op,
            ty: Ty::UNKNOWN,
        });
    }

    /// The super-constructor selector in an initializer list
    /// (`error_verifier.dart`, the `SuperConstructorInvocation` resolution):
    /// the direct superclass's named constructor set decides. An
    /// unresolvable superclass (external, `on`-constraint chain) suppresses.
    fn check_super_constructor(&mut self, container: StringId, member: &Row, span: SourceRange) {
        let Some(superclass) = self
            .index
            .get(self.file)
            .and_then(|f| f.container(container))
            .and_then(|c| c.superclass)
        else {
            return;
        };
        let Some((super_container, _)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            superclass,
        ) else {
            return;
        };
        if super_container.constructors.contains(&member.name) {
            return;
        }
        let class_text = self.strings.resolve(superclass).to_string();
        let name_text = self.strings.resolve(member.name).to_string();
        self.emit(
            SemanticCode::UndefinedConstructorInInitializer,
            span,
            &class_text,
            &name_text,
        );
    }

    /// Opens an argument group: the top row is its callee — unless the group
    /// belongs to a `new`/`const`/implicit-creation reference, whose class
    /// name resolves as a type use, never an invocation.
    fn open_arguments(&mut self) {
        let callee = if self.ctor_pos > 0 {
            None
        } else {
            self.callee_index()
        };
        self.invokes.push(callee);
        let mark = self.rows.len();
        self.arg_levels.push(ArgLevel {
            args: Vec::new(),
            arg_open: false,
            pending_name: None,
            mark,
        });
    }

    /// Closes an argument group; its callee awaits the send event. The
    /// collected argument types check against the callee's formals first
    /// (`argument_type_not_assignable`, decided on declared types only).
    fn close_arguments(&mut self) {
        self.flush_unclaimed();
        self.flush_argument();
        let levels = self.arg_levels.pop();
        self.check_arguments(levels);
        self.pending_send = self.invokes.pop().flatten();
    }

    /// Starts a new argument in the open argument list (the row window the
    /// argument owns closes at the next [`Engine::flush_argument`]).
    fn open_argument(&mut self, name: Option<StringId>) {
        self.flush_unclaimed();
        self.flush_argument();
        if let Some(level) = self.arg_levels.last_mut() {
            level.arg_open = true;
            level.pending_name = name;
        }
    }

    /// Recovers the unclaimed window `[mark, rows.len())` as a positional
    /// argument. The normal expression path fires no per-argument events
    /// (only the const path emits `positional_argument`), so a group whose
    /// window holds exactly one row since `mark` reads that row as the
    /// single positional argument; wider windows stay unattributed
    /// (compound operands must not lend their type — under-report).
    fn flush_unclaimed(&mut self) {
        let Some(level) = self.arg_levels.last_mut() else {
            return;
        };
        if level.arg_open {
            return;
        }
        let hi = self.rows.len();
        if hi == level.mark {
            return;
        }
        if let Some(row) = self.rows.get(level.mark).filter(|_| hi == level.mark + 1) {
            level.args.push((None, row.ty, row.range));
        }
        level.mark = hi;
    }

    /// Closes the pending argument: exactly one pushed row since the last
    /// argument boundary is a terminal expression whose type stands for the
    /// whole argument; anything else stays `Unknown` (compound operands must
    /// not lend their type). An empty window pushes nothing.
    fn flush_argument(&mut self) {
        let Some(level) = self.arg_levels.last_mut() else {
            return;
        };
        if !level.arg_open {
            return;
        }
        level.arg_open = false;
        let name = level.pending_name.take();
        let hi = self.rows.len();
        if hi == level.mark {
            return;
        }
        let (ty, range) = match self.rows.get(level.mark) {
            Some(row) if hi == level.mark + 1 => (row.ty, row.range),
            _ => (
                Ty::UNKNOWN,
                SourceRange {
                    offset: 0,
                    length: 0,
                },
            ),
        };
        level.mark = hi;
        level.args.push((name, ty, range));
    }

    /// Pairs the closed argument list against the callee's declared formals:
    /// positional by index, named by spelling. Extra positionals are the
    /// `extra_positional_arguments` family's business (unfrozen — skipped);
    /// unresolved callees suppress.
    fn check_arguments(&mut self, level: Option<ArgLevel>) {
        let Some(level) = level else { return };
        let Some(callee) = self.invokes.last().and_then(|slot| *slot) else {
            return;
        };
        let Some(row) = self.rows.get(callee) else {
            return;
        };
        let formals: Option<&Formals> = match &row.shape {
            // A top-level function's declared formals.
            RowShape::Bare(BareInfo::Plain { .. }) => self
                .top_level_decl(row.name)
                .and_then(|d| d.formals.as_deref()),
            RowShape::Member { recv } => self.member_formals_full(recv, row.name),
            _ => None,
        };
        let Some(formals) = formals else {
            return;
        };
        let mut positional_index = 0usize;
        let mut mismatches: Vec<(Ty, Ty, SourceRange)> = Vec::new();
        for (name, ty, range) in &level.args {
            let formal = if let Some(n) = name {
                formals.named_formal(*n)
            } else {
                let formal = formals.positional.get(positional_index);
                positional_index += 1;
                formal
            };
            let Some(formal) = formal else {
                continue;
            };
            if self.assignable(*ty, formal.ty) == Some(false) {
                mismatches.push((*ty, formal.ty, *range));
            }
        }
        for (from, to, range) in mismatches {
            let (a0, a1) = (self.ty_display(from), self.ty_display(to));
            self.emit3(SemanticCode::ArgumentTypeNotAssignable, range, &a0, &a1, "");
        }
    }

    /// Consumes a completed send: the closed group's callee row becomes an
    /// invocation. A send without arguments never opened a group, so a bare
    /// read stays a read (`foo;` vs `foo()`). An explicit type-argument group
    /// on the callee checks its arity (`_element`, or `_extension` for an
    /// extension override).
    ///
    /// A container-name callee (`E('x')` the explicit extension application,
    /// `C()` the implicit creation) folds into a receiver row carrying the
    /// container: the argument rows belong to the send (truncated away), and
    /// a following `.member` / operator merge must resolve against the
    /// container's face, not the last argument's row. Member callees
    /// (`C.named()`, methods) are not folds — their own rows stand.
    fn consume_send(&mut self) {
        let Some(callee) = self.pending_send.take() else {
            return;
        };
        if let Some(row) = self.rows.get_mut(callee)
            && row.intent == Intent::Read
        {
            row.intent = Intent::Invoke;
        }
        if let Some(pos) = self
            .deferred_args
            .iter()
            .rposition(|(row, _, _)| *row == callee)
        {
            let (_, count, range) = self.deferred_args.remove(pos);
            self.check_invocation_arity(callee, count, range);
        }
        if matches!(
            self.rows.get(callee).map(|r| r.shape.clone()),
            Some(RowShape::Bare(BareInfo::Container { .. }))
        ) {
            self.rows.truncate(callee + 1);
            if let Some(row) = self.rows.get_mut(callee)
                && let RowShape::Bare(BareInfo::Container { name, kind }) = row.shape
            {
                *row = Row {
                    name,
                    range: row.range,
                    qualified: false,
                    shape: RowShape::Receiver(Receiver::Created { name, kind }),
                    intent: Intent::Read,
                    write_op: None,
                    ty: Ty::UNKNOWN,
                };
            }
        }
    }

    /// An implicit creation ended (`g<int>(...)`): the oracle rewrites the
    /// shape to a plain invocation when the element resolves to a function
    /// (`ast_rewrite.dart`), so a deferred bare-function head checks its
    /// arity here (`_element`). Class/extension heads already checked at
    /// their group.
    fn end_implicit_creation(&mut self) {
        if self.ctor_pos != 1 {
            return;
        }
        let Some((row, count, range)) = self.deferred_args.pop() else {
            return;
        };
        let Some(head) = self.rows.get(row) else {
            return;
        };
        // A plain function head (`g<int>(...)`) — the oracle rewrites the
        // shape to a plain invocation (`ast_rewrite.dart`) — and a member
        // head reached through the creation syntax (`C.sm<int, String>(...)`,
        // the `id.id<...>(` form) both check as the `_element` invocation.
        // Class and extension heads checked at their type-argument group.
        if matches!(
            head.shape,
            RowShape::Bare(BareInfo::Plain { .. }) | RowShape::Member { .. }
        ) {
            self.check_invocation_arity(row, count, range);
        }
    }

    /// The invoked-callee arity check (`invocation_inferrer.dart` reports
    /// through the oracle's target dispatch, `type_instantiation_target.
    /// dart`): a resolved executable invoked with explicit type arguments
    /// reports `_element`, naming its kind word — a top-level function is
    /// `function`, a member method is `method`. Function-typed variables and
    /// getters resolve to type displays M1 cannot render (the oracle's
    /// `_function` / kind-`getter` rows) — no declared count, suppress.
    fn check_invocation_arity(&mut self, callee: usize, count: u16, range: SourceRange) {
        let Some(row) = self.rows.get(callee) else {
            return;
        };
        let declared = match &row.shape {
            // A top-level function (`g<int>()`); only real functions carry a
            // parameter list — getters stay silent.
            RowShape::Bare(BareInfo::Plain { .. }) => self
                .top_level_decl(row.name)
                .filter(|d| d.kind == MicroKind::TopLevelFn && d.formals.is_some())
                .map(|d| (d.type_params, ExecKind::Function)),
            RowShape::Member { recv } => self.member_exec_arity(recv, row.name),
            _ => None,
        };
        if let Some((declared, kind)) = declared
            && declared != count
        {
            let name_text = self.strings.resolve(row.name).to_string();
            self.emit4(
                SemanticCode::WrongNumberOfTypeArgumentsElement(kind),
                range,
                kind.word(),
                &name_text,
                &declared.to_string(),
                &count.to_string(),
            );
        }
    }

    /// A resolved container's declared type-parameter count (`None` when the
    /// name does not resolve — external types suppress).
    fn declared_type_params_of_container(&self, name: StringId) -> Option<u16> {
        resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        )
        .map(|(c, _)| c.type_params)
    }

    /// A member callee's declared type-parameter count with its oracle kind
    /// word: methods only — getters, setters, and fields resolve to oracle
    /// shapes M1 does not model, so they suppress (`None`).
    fn member_exec_arity(&self, recv: &Receiver, name: StringId) -> Option<(u16, ExecKind)> {
        let (container, super_walk) = match *recv {
            Receiver::This { container, .. } => (container, false),
            Receiver::Super { container, .. } => (container, true),
            Receiver::Typed {
                type_name: container,
                ..
            }
            | Receiver::Created {
                name: container, ..
            }
            | Receiver::Container {
                name: container, ..
            } => (container, false),
            _ => return None,
        };
        let walk = if super_walk {
            self.walk_super(container, name)
        } else {
            self.walk_members(container, name)
        };
        match walk {
            Walk::Found(entry) if entry.kind == MemberKind::Method => {
                Some((entry.type_params, ExecKind::Method))
            }
            _ => None,
        }
    }

    /// The type-argument application event (`g<int>;`, `c<int>;`): a
    /// resolved executable tear-off reports `_element` naming its kind word
    /// (`function_reference_resolver.dart` → the
    /// `InvocationTargetExecutableElement` dispatch); a type literal reports
    /// the base code; an implicit call reference reports `_element` naming
    /// `call`. Getters and function-typed variables resolve to oracle shapes
    /// M1 cannot name (the getter kind word, the `_function` type display) —
    /// suppress.
    fn consume_type_argument_application(&mut self) {
        let callee = self.rows.len().checked_sub(1);
        let Some(pos) = callee.and_then(|callee| {
            self.deferred_args
                .iter()
                .position(|(row, _, _)| *row == callee)
        }) else {
            return;
        };
        let (row_index, count, range) = self.deferred_args.remove(pos);
        let Some(row) = self.rows.get(row_index) else {
            return;
        };
        match &row.shape {
            RowShape::Bare(BareInfo::Container { name, .. }) => {
                // Type-literal instantiation `C<int>`: the base code.
                self.check_annotation_arity(*name, count, range);
            }
            RowShape::Bare(BareInfo::Plain { ty }) => {
                // A named top-level function tear-off, or an implicit call
                // reference through a declaration-typed receiver (`c<int>`
                // checks `call`).
                let named_fn = self
                    .top_level_decl(row.name)
                    .filter(|d| d.kind == MicroKind::TopLevelFn && d.formals.is_some())
                    .map(|d| (d.type_params, ExecKind::Function));
                if let Some((declared, kind)) = named_fn {
                    self.emit_element_arity(row.name, declared, kind, count, range);
                } else if let Ty::Named { name, .. } = *ty
                    && let Some(entry) = self.resolved_member(name, self.intern("call"))
                    && entry.kind == MemberKind::Method
                {
                    self.emit_element_arity(
                        self.intern("call"),
                        entry.type_params,
                        ExecKind::Method,
                        count,
                        range,
                    );
                }
            }
            RowShape::Member { recv } => {
                // A property-access tear-off (`C.sm<int, String>;`): the
                // oracle's `InvocationTargetFunctionTypedExpression` reports
                // `_function` leading with the function type — rendered from
                // the captured signature. A method tear-off is the only
                // function-typed shape the model can render: getters, fields,
                // and unknown receivers suppress. A name that is no member at
                // all is the constructor-reference shape `C<...>.named` — the
                // base type-argument code against the container.
                let container = match *recv {
                    Receiver::This { container, .. }
                    | Receiver::Super { container, .. }
                    | Receiver::Typed {
                        type_name: container,
                        ..
                    }
                    | Receiver::Container {
                        name: container, ..
                    } => Some(container),
                    _ => None,
                };
                let Some(container) = container else {
                    return;
                };
                match self.walk_members(container, row.name) {
                    Walk::Found(entry) if entry.kind == MemberKind::Method => {
                        if entry.type_params != count {
                            let display = self.function_type_display(entry);
                            self.emit3(
                                SemanticCode::WrongNumberOfTypeArgumentsFunction,
                                range,
                                &display,
                                &entry.type_params.to_string(),
                                &count.to_string(),
                            );
                        }
                    }
                    // Non-method finds (getter/field tear-offs) render to
                    // oracle shapes the model cannot name; a broken chain is
                    // opaque — both suppress.
                    Walk::Found(_) | Walk::ChainBroken => {}
                    Walk::Missing => self.check_annotation_arity(container, count, range),
                }
            }
            _ => {}
        }
    }

    /// The `_function` message's function-type display from a captured
    /// method signature: `{return} Function<{type params}>({formal types})`
    /// — `T m<T>(T x)` renders `T Function<T>(T)`, the oracle's rendering of
    /// the generic instantiation's uninstantiated type.
    fn function_type_display(&self, entry: &crate::model::MemberEntry) -> String {
        let ret = self.ty_display(entry.return_ty);
        let params = entry
            .type_param_names
            .iter()
            .map(|p| self.strings.resolve(*p))
            .collect::<Vec<_>>()
            .join(", ");
        let formals = entry
            .formals
            .as_ref()
            .map(|f| {
                f.positional
                    .iter()
                    .map(|formal| self.ty_display(formal.ty))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let group = if params.is_empty() {
            String::new()
        } else {
            format!("<{params}>")
        };
        format!("{ret} Function{group}({formals})")
    }

    /// The `_element` emit: kind word, executable name, declared vs given.
    fn emit_element_arity(
        &mut self,
        name: StringId,
        declared: u16,
        kind: ExecKind,
        count: u16,
        range: SourceRange,
    ) {
        if declared != count {
            let name_text = self.strings.resolve(name).to_string();
            self.emit4(
                SemanticCode::WrongNumberOfTypeArgumentsElement(kind),
                range,
                kind.word(),
                &name_text,
                &declared.to_string(),
                &count.to_string(),
            );
        }
    }

    /// The resolved member entry of `container.member` from a clean walk.
    fn resolved_member(
        &self,
        container: StringId,
        member: StringId,
    ) -> Option<&crate::model::MemberEntry> {
        match self.walk_members(container, member) {
            Walk::Found(entry) => Some(entry),
            _ => None,
        }
    }

    /// The annotation/creation arity check: the head's declared
    /// type-parameter count against the given count (`dynamic` and
    /// in-scope type parameters declare none; unresolved and external heads
    /// suppress).
    fn check_annotation_arity(&mut self, name: StringId, count: u16, range: SourceRange) {
        let Some(declared) = self.declared_type_params(name) else {
            return;
        };
        if declared != count {
            let name_text = self.strings.resolve(name).to_string();
            self.emit3(
                SemanticCode::WrongNumberOfTypeArguments,
                range,
                &name_text,
                &declared.to_string(),
                &count.to_string(),
            );
        }
    }

    /// A type head's declared type-parameter count: `dynamic` and type
    /// parameters declare none, a container keeps its own count, a
    /// typedef/function keeps its declaration's; anything unresolved is
    /// `None` (suppress — `undefined_class` already reports).
    fn declared_type_params(&self, name: StringId) -> Option<u16> {
        if self.builtin_types.contains(&name) || self.type_params.contains(&name) {
            return Some(0);
        }
        match self.resolve_bare(name) {
            BareInfo::Container { name, .. } => resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                name,
            )
            .map(|(c, _)| c.type_params),
            BareInfo::Plain { .. } => self.top_level_decl(name).map(|d| d.type_params),
            _ => None,
        }
    }

    /// The `end_type_arguments` dispatch — the code choice by use-site shape.
    fn end_type_arguments(&mut self, count: usize, begin: Token, end: Token) {
        let count = u16::try_from(count).unwrap_or(u16::MAX);
        let range = SourceRange {
            offset: begin.range.offset,
            length: end.range.end() - begin.range.offset,
        };
        // A type head makes the group an annotation or a creation type
        // reference.
        match self.group_heads.pop() {
            Some(Some((head, _head_range))) => {
                self.check_annotation_arity(head, count, range);
                return;
            }
            Some(None) | None => {}
        }
        // An enum constant's constructor arguments (`a<int>()`).
        if let Some(declared) = self.in_enum_element
            && declared != count
        {
            self.emit3(
                SemanticCode::WrongNumberOfTypeArgumentsEnum,
                range,
                &declared.to_string(),
                &count.to_string(),
                "",
            );
            return;
        }
        let Some(callee) = self.rows.len().checked_sub(1) else {
            return;
        };
        if self.ctor_pos > 0 {
            // A creation reference head. `parse_constructor_reference` keeps
            // its segments unmerged on the row stack: `C<int>` is a bare
            // container row (base code), `C.named<int>` is two bare rows —
            // the class face then the constructor segment (any count
            // reports: constructors never declare type parameters).
            let named_ctor = (callee >= 1)
                .then(|| {
                    let ctor = &self.rows[callee];
                    let class = &self.rows[callee - 1];
                    match (&class.shape, &ctor.shape) {
                        (
                            RowShape::Bare(BareInfo::Container {
                                name: class_name,
                                kind: ContainerKind::Class,
                            }),
                            RowShape::Bare(_),
                        ) => Some((*class_name, ctor.name)),
                        _ => None,
                    }
                })
                .flatten();
            if let Some((class, ctor)) = named_ctor {
                let class = self.strings.resolve(class).to_string();
                let ctor = self.strings.resolve(ctor).to_string();
                self.emit3(
                    SemanticCode::WrongNumberOfTypeArgumentsConstructor,
                    range,
                    &class,
                    &ctor,
                    "",
                );
                return;
            }
            let row = &self.rows[callee];
            match &row.shape {
                RowShape::Bare(BareInfo::Container {
                    name,
                    kind: ContainerKind::Extension,
                }) => {
                    // Explicit extension override `Ext<int>(list)` — any
                    // arity mismatch reports at the group
                    // (`extension_member_resolver.dart:380`).
                    let name = *name;
                    if let Some(declared) = self.declared_type_params_of_container(name)
                        && declared != count
                    {
                        let name_text = self.strings.resolve(name).to_string();
                        self.emit3(
                            SemanticCode::WrongNumberOfTypeArgumentsExtension,
                            range,
                            &name_text,
                            &declared.to_string(),
                            &count.to_string(),
                        );
                    }
                }
                RowShape::Bare(BareInfo::Container { name, .. }) => {
                    let name = *name;
                    self.check_annotation_arity(name, count, range);
                }
                _ => {
                    // A function invoked through the implicit-creation
                    // syntax resolves at the send; defer.
                    self.deferred_args.push((callee, count, range));
                }
            }
            return;
        }
        if matches!(
            self.rows[callee].shape,
            RowShape::Bare(_) | RowShape::Member { .. }
        ) {
            self.deferred_args.push((callee, count, range));
        }
    }

    /// The receiver classification of a bare row's info.
    fn receiver_of_bare(info: BareInfo) -> Receiver {
        match info {
            BareInfo::Container { name, kind } => Receiver::Container { name, kind },
            BareInfo::Prefix { name } => Receiver::Prefix { name },
            BareInfo::Plain { ty } => match ty {
                Ty::Named {
                    name: type_name, ..
                } => Receiver::Typed { type_name, ty },
                _ => Receiver::Unknown,
            },
            // The receiver itself is undefined; its own row reports, so the
            // member access must not.
            BareInfo::Missing { .. } => Receiver::Unknown,
        }
    }

    /// Arms the pending type head: the identifier just parsed a type
    /// annotation and may be followed by a type-argument group.
    fn arm_type_head(&mut self, name: StringId, range: SourceRange) {
        self.pending_type_head = Some((name, range));
    }

    /// The expression type of an already-pushed row shaped as a receiver or
    /// value (`a?[i]`'s receiver check).
    fn receiver_row_ty(row: &Row) -> Ty {
        match &row.shape {
            RowShape::Bare(info) => match info {
                BareInfo::Plain { ty } => *ty,
                BareInfo::Container { name, .. } => Ty::named(*name),
                _ => Ty::UNKNOWN,
            },
            RowShape::Receiver(recv) => match recv {
                Receiver::This { container, .. } | Receiver::Super { container, .. } => {
                    Ty::named(*container)
                }
                Receiver::Typed { ty, .. } => *ty,
                _ => Ty::UNKNOWN,
            },
            _ => Ty::UNKNOWN,
        }
    }

    /// Marks the assignment target: the closest row wholly before the
    /// operator, skipping folded value rows (index results, groups). Bare and
    /// member rows upgrade their intent; index-operator rows record the write
    /// form (`c[i] = v` writes through `[]=`, never the index argument).
    fn mark_assignment(&mut self, operator: Token) {
        let Some(op_text) = self.text_of(operator) else {
            return;
        };
        let limit = operator.range.offset;
        let compound = op_text != "=";
        let index_op = self.intern("[]");
        for row in self.rows.iter_mut().rev() {
            if row.range.end() > limit {
                continue;
            }
            if matches!(row.shape, RowShape::Value) {
                continue;
            }
            match &row.intent {
                Intent::Read => {
                    row.intent = if compound {
                        Intent::ReadWrite
                    } else {
                        Intent::Write
                    };
                }
                Intent::Operator(op, _) if *op == index_op => {
                    row.write_op = Some(if compound {
                        OpWrite::Compound
                    } else {
                        OpWrite::Assign
                    });
                }
                _ => {}
            }
            return;
        }
    }

    /// Marks an operator operand: the closest row wholly before `op`, when it
    /// is a bare identifier, a typed receiver, or a member access (`a + b`,
    /// `this + b`, `a.b + c` — the operator applies to the member result).
    fn mark_operator(&mut self, op: StringId, range: SourceRange) {
        for row in self.rows.iter_mut().rev() {
            if row.range.end() > range.offset {
                continue;
            }
            if matches!(
                row.shape,
                RowShape::Bare(_) | RowShape::Receiver(_) | RowShape::Member { .. }
            ) && row.intent == Intent::Read
            {
                row.intent = Intent::Operator(op, range);
            }
            return;
        }
    }

    /// The top row index when it can be an invocation callee.
    fn callee_index(&self) -> Option<usize> {
        let index = self.rows.len().checked_sub(1)?;
        matches!(
            self.rows[index].shape,
            RowShape::Bare(_) | RowShape::Member { .. }
        )
        .then_some(index)
    }

    /// The declared type of the assignment target row (`x = e`, `obj.f = e`):
    /// a bare name reads its snapshot, a member walks to the field's declared
    /// type; executables and unknown receivers stay `Unknown`.
    fn assignment_target_ty(&self, row: &Row) -> Ty {
        match &row.shape {
            RowShape::Bare(BareInfo::Plain { ty }) => *ty,
            RowShape::Bare(BareInfo::Container { name, kind }) => {
                // A container name is never a valid assignment target — the
                // oracle reports assignment-to-`this`-prefix shapes with
                // other codes; suppress here.
                let _ = (name, kind);
                Ty::UNKNOWN
            }
            RowShape::Member { recv } => match recv {
                Receiver::This { container, .. } | Receiver::Super { container, .. } => {
                    self.member_declared_ty(*container, row.name)
                }
                Receiver::Typed { type_name, .. } => self.member_declared_ty(*type_name, row.name),
                Receiver::Container {
                    name: container, ..
                } => resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    *container,
                )
                .and_then(|(c, _)| c.members.get(&row.name))
                .filter(|e| e.kind == MemberKind::Field)
                .map_or(Ty::UNKNOWN, |e| e.var_ty),
                _ => Ty::UNKNOWN,
            },
            _ => Ty::UNKNOWN,
        }
    }

    /// The assignment check (`invalid_assignment`,
    /// `assignment_expression_resolver.dart:795`): a single-row right-hand
    /// side whose type the M1 facts know (`Null` or a captured declared
    /// type) against the target's declared type. Plain `=` only — compound
    /// operators carry their own read semantics (under-report).
    fn check_assignment(&mut self, operator: Token) {
        let Some(op_text) = self.text_of(operator) else {
            return;
        };
        if op_text != "=" {
            return;
        }
        let limit = operator.range.offset;
        // The target: the closest row wholly before the operator, skipping
        // folded value rows (the same scan `mark_assignment` runs).
        let Some(target) = self
            .rows
            .iter()
            .enumerate()
            .rev()
            .find(|(_, row)| row.range.end() <= limit && !matches!(row.shape, RowShape::Value))
            .map(|(index, _)| index)
        else {
            return;
        };
        // `c[i] = e` writes through the `[]=` operator: the target's own
        // declared type is not the assignment's destination type.
        if matches!(self.rows[target].intent, Intent::Operator(..)) {
            return;
        }
        // The right-hand side: exactly one row strictly after the operator
        // (rows are pushed in source order, so the first post-operator row
        // and the absence of a second decide without collecting).
        let mut rhs = self.rows[target + 1..]
            .iter()
            .filter(|row| row.range.offset > limit);
        let Some(rhs_row) = rhs.next() else {
            return;
        };
        if rhs.next().is_some() {
            return;
        }
        let (from, range) = (rhs_row.ty, rhs_row.range);
        let to = self.assignment_target_ty(&self.rows[target]);
        if self.assignable(from, to) == Some(false) {
            let (a0, a1) = (self.ty_display(from), self.ty_display(to));
            self.emit3(SemanticCode::InvalidAssignment, range, &a0, &a1, "");
        }
    }

    /// The variable-declaration initializer check
    /// (`variable_declaration_resolver.dart:140`): the same decision as the
    /// assignment check against the declaration group's declared type.
    fn check_decl_initializer(&mut self) {
        let Some(to) = self.pending_decl_ty else {
            return;
        };
        let Some(mark) = self.init_mark else {
            return;
        };
        if self.rows.len() != mark + 1 {
            return;
        }
        let row = &self.rows[mark];
        if self.assignable(row.ty, to) == Some(false) {
            let (a0, a1) = (self.ty_display(row.ty), self.ty_display(to));
            self.emit3(SemanticCode::InvalidAssignment, row.range, &a0, &a1, "");
        }
        self.init_mark = None;
    }

    /// The return check (`rtv.dart:209-249` sync matrix): `void` hosts
    /// accept `void`/`dynamic`/`Null`; `void`/`dynamic` expressions only
    /// fit `void`/`dynamic` hosts; everything else defers to the
    /// assignability decision. Bare `return;` is a `void` expression.
    fn check_return(&mut self, from: Ty, range: SourceRange) {
        let Some(ret) = self.frames.iter().rev().find_map(|f| match &f.kind {
            FrameKind::Executable { ret: Some(ret), .. } => Some((ret.ty, ret.host, ret.name)),
            _ => None,
        }) else {
            return;
        };
        let (to, host, name) = ret;
        let verdict = match to {
            Ty::Void => match from {
                Ty::Void | Ty::Dynamic | Ty::Null => Some(true),
                Ty::Unknown => None,
                _ => Some(false),
            },
            Ty::Unknown => None,
            _ => match from {
                Ty::Void => Some(!matches!(to, Ty::Void | Ty::Dynamic)),
                Ty::Dynamic => Some(true),
                _ => self.assignable(from, to),
            },
        };
        if verdict == Some(false) {
            let (a0, a1, a2) = (
                self.ty_display(from),
                self.ty_display(to),
                self.strings.resolve(name).to_string(),
            );
            self.emit3(
                SemanticCode::ReturnOfInvalidType(host),
                range,
                &a0,
                &a1,
                &a2,
            );
        }
    }

    /// A single-row expression's type since `mark`, for the body/return
    /// checks; `None` when the expression is compound.
    fn single_row_since(&self, mark: usize) -> Option<(Ty, SourceRange)> {
        let row = self.rows.get(mark)?;
        (self.rows.len() == mark + 1).then_some((row.ty, row.range))
    }

    /// Enters an enum constant: its constructor type arguments check against
    /// the enclosing enum's declared type-parameter count.
    fn enter_enum_element(&mut self) {
        self.in_enum_element = self
            .container_context()
            .filter(|(_, kind)| *kind == ContainerKind::Enum)
            .and_then(|(name, _)| self.declared_type_params_of_container(name));
    }

    fn leave_enum_element(&mut self) {
        self.in_enum_element = None;
    }

    // —— pattern/switch machinery (M2 face) ————————————————————————

    /// Appends a leaf to the innermost open pattern frame; outside any
    /// pattern (pattern-assignment RHS, if-case roots) the leaf is dropped
    /// — only switch exhaustiveness consumes case spaces.
    fn pattern_leaf(&mut self, leaf: Restriction) {
        if let Some(frame) = self.pattern_stack.last_mut() {
            frame.leaves.push(leaf);
        }
    }

    /// Collapses one frame's leaves into the restriction it covers
    /// (`||` unions, `&&` intersects, a bare leaf passes through, a
    /// type-free pattern covers its context).
    fn collapse(leaves: Vec<Restriction>, or_ops: usize) -> Restriction {
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
    fn close_case_sink(&mut self) -> Option<Restriction> {
        self.case_sink_open = false;
        let root = self.pattern_roots.pop();
        self.pattern_roots.clear();
        root
    }

    /// Substitutes a captured root's type-free coverage with the
    /// scrutinee's context type.
    fn substitute_root(&self, root: Option<Restriction>) -> Restriction {
        let space = root.unwrap_or(Restriction::Unknown);
        if space == Restriction::NamedContext {
            return match NameTy::from_semantic(
                self.switches.last().map_or(Ty::Unknown, |c| c.scrutinee),
            ) {
                NameTy::Named(name, nullable) => Restriction::Named(name, nullable),
                _ => Restriction::Unknown,
            };
        }
        space
    }

    /// Opens a switch context; the scrutinee's push-time row is the last
    /// one on the row stack when the block opens. No row — the shape a
    /// complex scrutinee leaves — latches `Ty::Unknown`, and
    /// [`Self::report_switch`] skips the entire check for that (the
    /// oracle's `InvalidType` gate), so the latch gap suppresses instead
    /// of mis-answering.
    fn open_switch(&mut self, is_expression: bool, kw: SourceRange) {
        let scrutinee = self.rows.last().map_or(Ty::Unknown, |row| row.ty);
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
            default_seen: false,
            pending_root: None,
        });
    }

    /// Computes the exhaustiveness verdict and emits the switch
    /// diagnostics (per-case reachability, then the covering gap).
    fn report_switch(&mut self) {
        let Some(ctx) = self.switches.pop() else {
            return;
        };
        // An unresolvable scrutinee is the oracle's `InvalidType`
        // (constant_verifier.dart:1035): the entire exhaustiveness pass —
        // reachability and covering alike — gates off behind
        // `hasInvalidType` (:1095). Mapping it to the nullable top would
        // drop every case row and false-positive the covering gap. (The
        // push-time latch leaves `Unknown` exactly here: no row, or a row
        // whose declared type never resolved — the "complex scrutinee"
        // blind spot, now a clean whole-check suppression.)
        if ctx.scrutinee == Ty::Unknown {
            return;
        }
        let world = NameWorld {
            index: self.index,
            file: self.file,
            strings: self.strings,
            bool_id: self.intern("bool"),
            unknowns: std::cell::Cell::new(0),
        };
        let value = NameTy::from_semantic(ctx.scrutinee);
        let cases: Vec<Case<NameTy>> = ctx
            .cases
            .iter()
            .map(|c| Case {
                space: case_space(&world, &c.space, value),
                guarded: c.guarded,
            })
            .collect();
        let report = compute(&world, &value, &cases);
        let unreachable: Vec<SourceRange> = report
            .unreachable
            .iter()
            .filter_map(|index| ctx.cases.get(*index).map(|c| c.anchor))
            .collect();
        let (ty_text, unmatched, suggested) = match &report.exhaustiveness {
            Exhaustiveness::NonExhaustive(witnesses) => {
                let first = witnesses.first();
                (
                    self.ty_display(ctx.scrutinee),
                    first.map_or(String::new(), |w: &EngineWitness<NameTy>| {
                        w.as_witness(&world)
                    }),
                    first.map_or(String::new(), |w| w.as_correction(&world)),
                )
            }
            Exhaustiveness::Exhaustive => (String::new(), String::new(), String::new()),
        };
        // The oracle reports reachability per case first
        // (constant_verifier.dart:1080-1088), then the covering gap
        // (:1122-1140) at the `switch` keyword.
        for anchor in unreachable {
            self.emit(SemanticCode::UnreachableSwitchCase, anchor, "", "");
        }
        if let Exhaustiveness::NonExhaustive(_) = report.exhaustiveness {
            // A cross-file scrutinee name is opaque to the file-scoped
            // world ([`NameWorld::name_resolvable`]): its sealedness and
            // subclass set are unknowable in-file, so the covering verdict
            // may be falsely negative and the expression face — always
            // must-be-exhaustive — suppresses (under-report direction).
            // The statement face is already gated on in-file sealed facts.
            let expression_face_decides = match value {
                NameTy::Named(name, _) => world.name_resolvable(name),
                _ => true,
            };
            let must_be_exhaustive = (ctx.is_expression && expression_face_decides)
                || Self::is_always_exhaustive(&world, value);
            if must_be_exhaustive && !ctx.default_seen {
                let code = if ctx.is_expression {
                    SemanticCode::NonExhaustiveSwitchExpression
                } else {
                    SemanticCode::NonExhaustiveSwitchStatement
                };
                // The oracle row carries a correctionMessage whose `{2}` is
                // the first witness's correction text (diagnostic.g.dart
                // :13018/:13079, constant_verifier.dart:1104-1120); the
                // diagnostic carries it in the correction slot.
                self.report.diagnostics.push(Diagnostic::new(
                    code.severity(),
                    code.code_info(),
                    code.message3(&ty_text, &unmatched, &suggested),
                    self.file,
                    ctx.switch_kw,
                    code.correction_message(&suggested),
                ));
            }
        }
    }

    /// `isAlwaysExhaustive` (type_system.dart:846-887) at name level:
    /// `bool`, `Null`, enums, and sealed classes split completely; nothing
    /// else does (nullable types and `dynamic` need the null case).
    fn is_always_exhaustive(world: &NameWorld<'_>, ty: NameTy) -> bool {
        match ty {
            NameTy::Null => true,
            NameTy::Named(name, false) => world.is_sealed_name(name),
            _ => false,
        }
    }

    fn ty_display(&self, ty: Ty) -> String {
        match ty {
            Ty::Unknown => String::new(),
            Ty::Null => "Null".to_string(),
            Ty::Dynamic => "dynamic".to_string(),
            Ty::Void => "void".to_string(),
            Ty::Named { name, nullable } => {
                let text = self.strings.resolve(name);
                if nullable {
                    format!("{text}?")
                } else {
                    text.to_string()
                }
            }
        }
    }

    /// The M1 assignability decision ([`Ty::assignable_from`] plus the
    /// final-primitive table): a value typed by one final primitive never
    /// fits another — the types cannot be extended, implemented, or mixed
    /// in, so no subclass relation can close the gap, whatever declaration
    /// the names resolve to. `int`/`double` fit `num` (the one supertype
    /// edge); `int` → `double` stays undecided (an int *literal* in a
    /// double context converts, an int *value* does not — the literal/variable
    /// distinction is below the M1 facts). `None` suppresses (under-report).
    fn assignable(&self, from: Ty, to: Ty) -> Option<bool> {
        if let Some(verdict) = to.assignable_from(from) {
            return Some(verdict);
        }
        let (
            Ty::Named {
                name: from_name,
                nullable: false,
            },
            Ty::Named {
                name: to_name,
                nullable: false,
            },
        ) = (from, to)
        else {
            return None;
        };
        if from_name == to_name
            || !self.primitive_ids.contains(&from_name)
            || !self.primitive_ids.contains(&to_name)
        {
            return None;
        }
        let (int, double, num) = (
            self.primitive_ids[0],
            self.primitive_ids[1],
            self.primitive_ids[2],
        );
        if (from_name == int || from_name == double) && to_name == num {
            return Some(true);
        }
        if from_name == int && to_name == double {
            // `double d = 1;` converts the int literal; `double d = intVar;`
            // does not — the literal/variable distinction is below the M1
            // facts, so the pair stays undecided (under-report).
            return None;
        }
        Some(false)
    }

    /// The enum-constant constructor selector (`c.other()` in the
    /// element's own position): the enclosing enum's constructor face
    /// decides (`parseEnumElement`, the `ConstructorReferenceContinuation`
    /// resolution). Unresolvable enum containers suppress.
    fn check_enum_constructor_selector(&mut self, name: StringId, range: SourceRange) {
        let Some((enum_name, _)) = self.container_context() else {
            return;
        };
        let Some((container, _)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            enum_name,
        ) else {
            return;
        };
        if container.constructors.contains(&name) {
            return;
        }
        let name_text = self.strings.resolve(name).to_string();
        self.emit(
            SemanticCode::UndefinedEnumConstructor,
            range,
            &name_text,
            "",
        );
    }

    /// A simple annotation name (`@foo`): the oracle's annotation resolution
    /// needs a class (or a `foo.name` constant face); a fully missing name is
    /// `undefined_annotation` anchored over the `@` (3.13.1 caliber — the
    /// diagnostic was removed upstream in 3.14, `m1_codes.txt` freezes the
    /// measured form). Defined names stay silent: the class/constructor
    /// second-part checks sit outside the M1 model (under-report).
    fn check_annotation(&mut self, name: StringId, range: SourceRange) {
        let missing = matches!(self.resolve_bare(name), BareInfo::Missing { .. });
        let at = self.annotation_at.take();
        if missing && !self.guard.ignore(None, name) {
            let span = SourceRange {
                offset: at.map_or(range.offset, |at| at.offset),
                length: range.end() - at.map_or(range.offset, |at| at.offset),
            };
            let name_text = self.strings.resolve(name).to_string();
            self.emit(SemanticCode::UndefinedAnnotation, span, &name_text, "");
        }
        if missing {
            self.record_miss(name, range, false, NameUse::Annotation);
        }
    }

    /// A literal whose type is a final primitive (`push_value_row` plus the
    /// type the static-nullability checks decide on).
    fn push_primitive_row(&mut self, token: Token, primitive: usize) {
        self.push_row(Row {
            name: self.intern(""),
            range: token.range,
            qualified: false,
            shape: RowShape::Value,
            intent: Intent::Read,
            write_op: None,
            ty: Ty::named(self.primitive_ids[primitive]),
        });
    }

    /// The named argument membership test against the active callee
    /// (`type_analyzer.dart:5820`): a resolved callee whose formal table
    /// lacks the label reports `undefined_named_parameter` at the label; an
    /// unresolved callee stays silent (its own invocation code reports) and
    /// only feeds the §8 table.
    fn check_named_argument(&mut self, name: StringId, range: SourceRange) {
        let Some(callee) = self.invokes.last().and_then(|slot| *slot) else {
            return;
        };
        let (shape, callee_name) = match self.rows.get(callee) {
            Some(row) => (row.shape.clone(), row.name),
            None => return,
        };
        let (resolved, defined) = match &shape {
            // The membership test exists only where the oracle's does: a real
            // top-level function's formal table. A class callee is a
            // constructor invocation (a constructor accepting the label is
            // silent — constructors sit outside the M1 model) and a getter
            // callee is a property access; both stay unresolved, mirroring
            // the arity path's filter.
            RowShape::Bare(_) => {
                let decl = self
                    .top_level_decl(callee_name)
                    .filter(|d| d.kind == MicroKind::TopLevelFn && d.formals.is_some());
                let defined =
                    decl.is_some_and(|d| Self::has_named_formal(d.formals.as_deref(), name));
                (decl.is_some(), defined)
            }
            RowShape::Member { recv } => {
                let formals = self.member_formals_full(recv, callee_name);
                (
                    formals.is_some(),
                    formals.is_some_and(|f| f.named.iter().any(|entry| entry.name == name)),
                )
            }
            _ => return,
        };
        if resolved && !defined {
            let text = self.strings.resolve(name).to_string();
            self.emit(SemanticCode::UndefinedNamedParameter, range, &text, "");
        }
        if !defined {
            // The §8 table carries every real miss (the membership test
            // itself is live).
            self.record_miss(name, range, false, NameUse::NamedArgument);
        }
    }

    // —— pass 2 ———————————————————————————————————————————————————————

    /// Resolves the recorded rows in source order and emits.
    fn resolve_rows(&mut self) {
        let rows = std::mem::take(&mut self.rows);
        for row in &rows {
            match &row.shape {
                RowShape::Bare(info) => self.resolve_bare_row(row, *info),
                RowShape::Member { recv } => self.resolve_member_row(row, *recv),
                RowShape::Receiver(recv) => self.resolve_receiver_row(row, *recv),
                RowShape::TypeUse(..) => self.resolve_type_row(row),
                RowShape::Value => {}
            }
        }
    }

    /// A surviving typed-keyword receiver (`this + b`, `super[i]`): only
    /// operator intents carry checks — member merges consumed every other
    /// shape at the dot event.
    fn resolve_receiver_row(&mut self, row: &Row, recv: Receiver) {
        if !matches!(row.intent, Intent::Operator(..)) {
            return;
        }
        let (read, write) = self.operator_spellings(row);
        match recv {
            Receiver::This { container, .. } => {
                let mut missing: Vec<String> = Vec::new();
                for spelling in [read, write].into_iter().flatten() {
                    if matches!(self.walk_members(container, spelling), Walk::Missing) {
                        missing.push(self.strings.resolve(spelling).to_string());
                    }
                }
                if missing.is_empty() {
                    return;
                }
                let type_text = self.strings.resolve(container).to_string();
                for text in missing {
                    self.emit(
                        SemanticCode::UndefinedOperator,
                        row.range,
                        &text,
                        &type_text,
                    );
                }
            }
            Receiver::Super { container, .. } => {
                let mut missing: Vec<String> = Vec::new();
                for spelling in [read, write].into_iter().flatten() {
                    if matches!(self.walk_super(container, spelling), Walk::Missing) {
                        missing.push(self.strings.resolve(spelling).to_string());
                    }
                }
                if missing.is_empty() {
                    return;
                }
                let type_text = self.strings.resolve(container).to_string();
                for text in missing {
                    self.emit(
                        SemanticCode::UndefinedSuperMember(SuperShape::Operator),
                        row.range,
                        &text,
                        &type_text,
                    );
                }
            }
            // A folded creation/extension-override receiver (`C() + 1`,
            // `E(1) * 2`): the operator resolves against the container's
            // member face — extensions report their dedicated operator row.
            Receiver::Created { name, kind } => {
                let Intent::Operator(_, op_range) = row.intent else {
                    return;
                };
                let mut missing: Vec<String> = Vec::new();
                for spelling in [read, write].into_iter().flatten() {
                    if matches!(self.walk_members(name, spelling), Walk::Missing) {
                        missing.push(self.strings.resolve(spelling).to_string());
                    }
                }
                if missing.is_empty() {
                    return;
                }
                let host_text = self.strings.resolve(name).to_string();
                for text in missing {
                    let code = if kind == ContainerKind::Extension {
                        SemanticCode::UndefinedExtensionOperator
                    } else {
                        SemanticCode::UndefinedOperator
                    };
                    self.emit(code, op_range, &text, &host_text);
                }
                self.record_miss(name, op_range, true, NameUse::Read);
            }
            _ => {}
        }
    }

    fn resolve_bare_row(&mut self, row: &Row, info: BareInfo) {
        let name_text = self.strings.resolve(row.name);
        match info {
            // A prefix used bare: invoking a prefix is an error the oracle
            // names with an unfrozen code — record only.
            BareInfo::Container { .. } | BareInfo::Prefix { .. } => {}
            BareInfo::Plain { ty } => {
                // A direct operator application on a declared-typed receiver
                // (`c + d`, `c[i] = v`).
                if matches!(row.intent, Intent::Operator(..)) {
                    self.check_operator_expr(ty, row);
                }
            }
            BareInfo::Missing {
                this_type,
                in_exec,
                on_opaque,
            } => {
                // Mixin `on` constraints and extension `on` types are
                // invisible to the model: inside their instance members a
                // bare miss may be a constraint member — record only.
                if on_opaque {
                    self.record_miss(row.name, row.range, false, use_kind_of(&row.intent));
                    return;
                }
                match row.intent {
                    Intent::Invoke => {
                        // UnqualifiedLookupDomain: inside a class or enum
                        // instance member the invocation miss names `this`'s
                        // type (`undefined_method`); everywhere else it is
                        // `undefined_function`.
                        if let Some(container) = this_type {
                            let type_text = self.strings.resolve(container).to_string();
                            if !self.guard.ignore(None, row.name) {
                                self.emit(
                                    SemanticCode::UndefinedMethod,
                                    row.range,
                                    name_text,
                                    &type_text,
                                );
                            }
                        } else if !self.guard.ignore(None, row.name) {
                            self.emit(SemanticCode::UndefinedFunction, row.range, name_text, "");
                        }
                        self.record_miss(row.name, row.range, false, NameUse::Invoke);
                    }
                    Intent::Read if row.name == self.intern("await") && in_exec => {
                        // `await` in a sync body is never a lookup miss: the
                        // frozen code names the missing async marker.
                        if !self.guard.ignore(None, row.name) {
                            self.emit(SemanticCode::UndefinedIdentifierAwait, row.range, "", "");
                        }
                        self.record_miss(row.name, row.range, false, NameUse::Read);
                    }
                    Intent::Read | Intent::Write | Intent::ReadWrite => {
                        // A write to a defined getter-only name is under-reported
                        // (the oracle's setter code needs a receiver type); a
                        // fully missing name reports.
                        if !self.guard.ignore(None, row.name) {
                            self.emit(SemanticCode::UndefinedIdentifier, row.range, name_text, "");
                        }
                        let use_kind = match row.intent {
                            Intent::Write | Intent::ReadWrite => NameUse::Write,
                            _ => NameUse::Read,
                        };
                        self.record_miss(row.name, row.range, false, use_kind);
                    }
                    Intent::Operator(..) => {
                        // The undefined receiver itself reports above all; the
                        // operator on an invalid type never cascades.
                        self.record_miss(row.name, row.range, false, NameUse::Read);
                    }
                }
            }
        }
    }

    /// Emits a type-position row from its push-time verdict snapshot.
    fn resolve_type_row(&mut self, row: &Row) {
        let RowShape::TypeUse(verdict, suppressed) = row.shape else {
            return;
        };
        let name_text = self.strings.resolve(row.name);
        match verdict {
            TypeVerdict::PrefixHead => {
                if !self.guard.ignore(None, row.name) {
                    self.emit(SemanticCode::UndefinedIdentifier, row.range, name_text, "");
                }
                self.record_miss(row.name, row.range, false, NameUse::Read);
            }
            TypeVerdict::Prefixed { prefix } => {
                if !self.guard.ignore(Some(prefix), row.name) {
                    self.emit(SemanticCode::UndefinedClass, row.range, name_text, "");
                }
                self.record_miss(row.name, row.range, true, NameUse::Type);
            }
            TypeVerdict::Missing => {
                if !suppressed && !self.guard.ignore(None, row.name) {
                    self.emit(SemanticCode::UndefinedClass, row.range, name_text, "");
                }
                self.record_miss(row.name, row.range, false, NameUse::Type);
            }
        }
    }

    fn resolve_member_row(&mut self, row: &Row, recv: Receiver) {
        let name_text = self.strings.resolve(row.name);
        match recv {
            Receiver::Unknown => {}
            Receiver::Prefix { name: prefix } => {
                if self.prefixed_hit(prefix, row.name) || self.guard.ignore(Some(prefix), row.name)
                {
                    return;
                }
                match row.intent {
                    // Verified: a prefixed *invocation* miss is
                    // `undefined_function`; the property path reports
                    // `undefined_prefixed_name` (PrefixedLookupDomain).
                    Intent::Invoke => {
                        self.emit(SemanticCode::UndefinedFunction, row.range, name_text, "");
                        self.record_miss(row.name, row.range, true, NameUse::Invoke);
                    }
                    Intent::Read | Intent::Write | Intent::ReadWrite => {
                        let type_text = self.strings.resolve(prefix).to_string();
                        self.emit(
                            SemanticCode::UndefinedPrefixedName,
                            row.range,
                            name_text,
                            &type_text,
                        );
                        let use_kind = match row.intent {
                            Intent::Write | Intent::ReadWrite => NameUse::Write,
                            _ => NameUse::Read,
                        };
                        self.record_miss(row.name, row.range, true, use_kind);
                    }
                    Intent::Operator(..) => {
                        self.record_miss(row.name, row.range, true, NameUse::Read);
                    }
                }
            }
            Receiver::Container { name, kind } => {
                self.check_container_member(row, name, kind);
            }
            // A folded creation: class-shaped results resolve on the
            // instance face (superclass walk, `Object` members); an
            // extension override resolves against the extension's members —
            // its `on` type's `Object` face is the under-report.
            Receiver::Created { name, kind } => {
                if kind == ContainerKind::Extension {
                    if self.object_members.contains(&row.name) || row.name == self.intern("==") {
                        return;
                    }
                    self.check_container_member(row, name, kind);
                } else {
                    self.check_instance_member(
                        row,
                        Receiver::Typed {
                            type_name: name,
                            ty: Ty::UNKNOWN,
                        },
                    );
                }
            }
            Receiver::This { .. } | Receiver::Typed { .. } => {
                self.check_instance_member(row, recv);
            }
            Receiver::Super { container, .. } => {
                self.check_super_member(row, container);
            }
        }
    }

    fn prefixed_hit(&self, prefix: StringId, name: StringId) -> bool {
        !matches!(
            self.namespaces.lookup_prefixed(self.lib, prefix, name),
            dartq_resolution::Lookup::NotFound
        )
    }

    /// Static/enum/extension access on a container name (`C.m`, `E.x`,
    /// `Ext.g`) — the oracle's `StaticLookupDomain`
    /// (`lookup_failure_reporter.dart`): the interface face reports
    /// `undefined_method` (the type-literal shape shares the registry name
    /// and message), `undefined_getter`, `undefined_enum_constant` on enum
    /// reads, and `undefined_setter` on writes; the extension face reports
    /// its dedicated trio. Operator intents have no static-face shape the
    /// M1 model can name — record only. External containers suppress.
    fn check_container_member(&mut self, row: &Row, container: StringId, kind: ContainerKind) {
        let Some((resolved, _)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        ) else {
            return;
        };
        let write = matches!(row.intent, Intent::Write | Intent::ReadWrite);
        let mut defined = resolved.members.contains_key(&row.name);
        if !defined && write {
            defined = resolved
                .members
                .contains_key(&self.setter_spelling(row.name));
        }
        if defined {
            return;
        }
        let name_text = self.strings.resolve(row.name);
        let type_text = self.strings.resolve(container).to_string();
        // A compound write checks both faces (the oracle resolves the read
        // first): the read-face text, then the setter text.
        let read_face_missing = !resolved.members.contains_key(&row.name);
        let (invoke_code, read_code, write_code) = match kind {
            ContainerKind::Extension => (
                SemanticCode::UndefinedExtensionMethod,
                SemanticCode::UndefinedExtensionGetter,
                SemanticCode::UndefinedExtensionSetter,
            ),
            _ => (
                SemanticCode::UndefinedMethod,
                if kind == ContainerKind::Enum {
                    SemanticCode::UndefinedEnumConstant
                } else {
                    SemanticCode::UndefinedGetter
                },
                SemanticCode::UndefinedSetter,
            ),
        };
        match row.intent {
            Intent::Invoke => self.emit(invoke_code, row.range, name_text, &type_text),
            Intent::Read => self.emit(read_code, row.range, name_text, &type_text),
            Intent::Write => self.emit(write_code, row.range, name_text, &type_text),
            Intent::ReadWrite => {
                if read_face_missing {
                    self.emit(read_code, row.range, name_text, &type_text);
                }
                self.emit(write_code, row.range, name_text, &type_text);
            }
            Intent::Operator(..) => {}
        }
        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
    }

    /// Instance access through a declaration-typed receiver (`this.m`, `c.m`
    /// on a `C c`). A clean walk that misses reports the member family
    /// (`undefined_method`/`_getter`/`_setter` by access intent); a broken
    /// chain, the extension pool, enum synthesized members, and `Object`
    /// members suppress. Operator intents are the member-result shape
    /// (`c.j + 1`, `c.j[i]`): the member itself is the operator's receiver —
    /// a found member hands the operator check to its result type, a clean
    /// miss reports the getter read (the operator's invalid type never
    /// cascades). Mixin `on` constraints and extension `on` types are
    /// invisible to the model, so bodies of those containers record only.
    fn check_instance_member(&mut self, row: &Row, recv: Receiver) {
        let (Receiver::This { container, .. }
        | Receiver::Typed {
            type_name: container,
            ..
        }) = recv
        else {
            return;
        };
        if let Intent::Operator(..) = row.intent {
            match self.walk_members(container, row.name) {
                Walk::Found(entry) => {
                    let ty = Self::member_result_ty(entry);
                    self.check_operator_expr(ty, row);
                    return;
                }
                Walk::ChainBroken => return,
                Walk::Missing => {
                    let name_text = self.strings.resolve(row.name);
                    let type_text = self.strings.resolve(container).to_string();
                    self.emit(
                        SemanticCode::UndefinedGetter,
                        row.range,
                        name_text,
                        &type_text,
                    );
                    self.record_miss(row.name, row.range, true, NameUse::Read);
                    return;
                }
            }
        }
        if self.member_defined(container, row, false) {
            return;
        }
        if matches!(self.walk_members(container, row.name), Walk::ChainBroken) {
            // A chain broken at the root type (`int x; x.noSuchGetter`) is
            // the external-primitive face: the core member table decides.
            // A broken superclass hop suppresses as before.
            self.check_core_type_member(container, row);
            return;
        }
        let Some(kind) = self.container_kind_of(container) else {
            return; // external receiver type: suppress
        };
        if kind == ContainerKind::Enum && self.enum_synth_members.contains(&row.name) {
            return;
        }
        if matches!(kind, ContainerKind::Mixin | ContainerKind::Extension) {
            // The `on` face is invisible: a miss may be a constraint member.
            self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
            return;
        }
        let name_text = self.strings.resolve(row.name);
        let type_text = self.strings.resolve(container).to_string();
        match row.intent {
            Intent::Invoke => {
                self.emit(
                    SemanticCode::UndefinedMethod,
                    row.range,
                    name_text,
                    &type_text,
                );
            }
            Intent::Write | Intent::ReadWrite => {
                // A compound write checks the read face first (the oracle
                // resolves the getter before the setter).
                if row.intent == Intent::ReadWrite
                    && matches!(self.walk_members(container, row.name), Walk::Missing)
                {
                    self.emit(
                        SemanticCode::UndefinedGetter,
                        row.range,
                        name_text,
                        &type_text,
                    );
                }
                self.emit(
                    SemanticCode::UndefinedSetter,
                    row.range,
                    name_text,
                    &type_text,
                );
            }
            Intent::Read => {
                self.emit(
                    SemanticCode::UndefinedGetter,
                    row.range,
                    name_text,
                    &type_text,
                );
            }
            Intent::Operator(..) => unreachable!("operator intents returned above"),
        }
        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
    }

    /// `super.member` — the oracle's `SuperLookupDomain`: one analyzerCode
    /// (`undefined_super_member`) with a per-access-shape message, reported
    /// against the enclosing container's name. Operator intents follow the
    /// member-result shape (the direct `super + b` form lives on the
    /// receiver rows in [`Engine::resolve_receiver_row`], which reports the
    /// operator shape). A broken super chain (mixins: `on` constraints are
    /// not edges) suppresses.
    fn check_super_member(&mut self, row: &Row, container: StringId) {
        if let Intent::Operator(..) = row.intent {
            match self.walk_super(container, row.name) {
                Walk::Found(entry) => {
                    let ty = Self::member_result_ty(entry);
                    self.check_operator_expr(ty, row);
                }
                Walk::ChainBroken => {}
                Walk::Missing => {
                    let name_text = self.strings.resolve(row.name);
                    let type_text = self.strings.resolve(container).to_string();
                    self.emit(
                        SemanticCode::UndefinedSuperMember(SuperShape::Getter),
                        row.range,
                        name_text,
                        &type_text,
                    );
                    self.record_miss(row.name, row.range, true, NameUse::Read);
                }
            }
            return;
        }
        if self.member_defined(container, row, true) {
            return;
        }
        if matches!(self.walk_super(container, row.name), Walk::ChainBroken) {
            return;
        }
        let shape = match row.intent {
            Intent::Invoke => SuperShape::Method,
            Intent::Write | Intent::ReadWrite => SuperShape::Setter,
            Intent::Read => SuperShape::Getter,
            Intent::Operator(..) => unreachable!("operator intents returned above"),
        };
        let name_text = self.strings.resolve(row.name);
        let type_text = self.strings.resolve(container).to_string();
        // A compound write checks the read face first (the oracle resolves
        // the getter before the setter).
        if row.intent == Intent::ReadWrite
            && matches!(self.walk_super(container, row.name), Walk::Missing)
        {
            self.emit(
                SemanticCode::UndefinedSuperMember(SuperShape::Getter),
                row.range,
                name_text,
                &type_text,
            );
        }
        self.emit(
            SemanticCode::UndefinedSuperMember(shape),
            row.range,
            name_text,
            &type_text,
        );
        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
    }

    /// The external-primitive member face (`int x; x.noSuchGetter`): the
    /// receiver type is a dart:core primitive when the name is one of the
    /// [`CORE_TYPE_MEMBERS`] keys and resolves to no indexed container.
    /// Extension members still apply implicitly, so the pool gates first; a
    /// table hit is defined, a miss reports the access-intent trio.
    fn check_core_type_member(&mut self, container: StringId, row: &Row) {
        let container_text = self.strings.resolve(container);
        let Some((_, members)) = CORE_TYPE_MEMBERS
            .iter()
            .find(|(name, _)| container_text == *name)
        else {
            return; // not a known primitive: suppress as before
        };
        if resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        )
        .is_some()
        {
            return; // locally declared: the regular walk already decided
        }
        if self
            .index
            .get(self.file)
            .is_some_and(|f| f.extension_member(row.name).is_some())
        {
            return;
        }
        let write = matches!(row.intent, Intent::Write | Intent::ReadWrite);
        let name_text = self.strings.resolve(row.name);
        if members.contains(&name_text)
            || (write && members.contains(&self.strings.resolve(self.setter_spelling(row.name))))
        {
            return;
        }
        if row.name == self.intern("==") || self.object_members.contains(&row.name) {
            return;
        }
        let type_text = container_text.to_string();
        match row.intent {
            Intent::Invoke => self.emit(
                SemanticCode::UndefinedMethod,
                row.range,
                name_text,
                &type_text,
            ),
            Intent::Write | Intent::ReadWrite => {
                if row.intent == Intent::ReadWrite {
                    self.emit(
                        SemanticCode::UndefinedGetter,
                        row.range,
                        name_text,
                        &type_text,
                    );
                }
                self.emit(
                    SemanticCode::UndefinedSetter,
                    row.range,
                    name_text,
                    &type_text,
                );
            }
            Intent::Read => self.emit(
                SemanticCode::UndefinedGetter,
                row.range,
                name_text,
                &type_text,
            ),
            Intent::Operator(..) => {}
        }
        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
    }

    /// The read and write operator spellings an access needs (`[]` / `[]=`
    /// for index rows by their write form; the plain spelling for binary
    /// operators); `(None, None)` for non-operator rows.
    fn operator_spellings(&self, row: &Row) -> (Option<StringId>, Option<StringId>) {
        let Intent::Operator(op, _) = &row.intent else {
            return (None, None);
        };
        if *op == self.intern("[]") {
            match row.write_op {
                Some(OpWrite::Assign) => (None, Some(self.intern("[]="))),
                Some(OpWrite::Compound) => (Some(*op), Some(self.intern("[]="))),
                None => (Some(*op), None),
            }
        } else {
            (Some(*op), None)
        }
    }

    /// The `name=` lookup spelling of a write target. Cached: the `format!`
    /// plus intern allocation repeats for the same write targets across a
    /// unit, and pass-time member checks ask on every write.
    fn setter_spelling(&mut self, name: StringId) -> StringId {
        if let Some(&spelling) = self.setter_ids.get(&name) {
            return spelling;
        }
        let text = self.strings.resolve(name);
        let spelling = self.strings.intern(&format!("{text}="));
        self.setter_ids.insert(name, spelling);
        spelling
    }

    /// A member's result type as far as M1 knows it: the declared field type
    /// or getter return type; methods (function-typed tear-offs) and setters
    /// stay [`Ty::Unknown`] — every check on them suppresses.
    fn member_result_ty(entry: &MemberEntry) -> Ty {
        match entry.kind {
            MemberKind::Field => entry.var_ty,
            MemberKind::Getter => entry.return_ty,
            MemberKind::Method | MemberKind::Setter => Ty::UNKNOWN,
        }
    }

    /// The operator application on a known receiver type (`c + d`, `c[i]`,
    /// `c.j + 1` against the member result): each read/write spelling that
    /// the type's container genuinely lacks reports `undefined_operator` at
    /// the operator's anchor span. Unresolved or external types (`Unknown`,
    /// broken chains) suppress — an invalid receiver type never cascades.
    fn check_operator_expr(&mut self, ty: Ty, row: &Row) {
        let Intent::Operator(_, op_range) = row.intent else {
            return;
        };
        let (read, write) = self.operator_spellings(row);
        let Ty::Named { name, .. } = ty else {
            return;
        };
        if resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        )
        .is_none()
        {
            return;
        }
        let mut missing: Vec<String> = Vec::new();
        for spelling in [read, write].into_iter().flatten() {
            if matches!(self.walk_members(name, spelling), Walk::Missing) {
                missing.push(self.strings.resolve(spelling).to_string());
            }
        }
        if missing.is_empty() {
            return;
        }
        let type_text = self.strings.resolve(name).to_string();
        for text in missing {
            self.emit(SemanticCode::UndefinedOperator, op_range, &text, &type_text);
        }
    }

    /// Runs the walk(s) an access needs and applies the shared suppressions
    /// (broken chain, extension pool, `Object` members). A write also
    /// accepts the setter spelling (`name=`): a same-named getter, field, or
    /// method suppresses with the oracle's assignment-to-* codes, a setter
    /// defines the write.
    fn member_defined(&mut self, container: StringId, row: &Row, super_walk: bool) -> bool {
        let write = matches!(row.intent, Intent::Write | Intent::ReadWrite);
        let mut found = self.member_found(container, row.name, super_walk);
        if !found && write {
            let spelling = self.setter_spelling(row.name);
            found = self.member_found(container, spelling, super_walk);
        }
        if found {
            return true;
        }
        if row.name == self.intern("==") || self.object_members.contains(&row.name) {
            return true;
        }
        self.index
            .get(self.file)
            .is_some_and(|f| f.extension_member(row.name).is_some())
    }

    /// Whether the plain member walk finds `member` on `container`.
    fn member_found(&self, container: StringId, member: StringId, super_walk: bool) -> bool {
        let walk = if super_walk {
            self.walk_super(container, member)
        } else {
            self.walk_members(container, member)
        };
        matches!(walk, Walk::Found(_))
    }

    fn container_kind_of(&self, container: StringId) -> Option<ContainerKind> {
        resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        )
        .map(|(c, _)| c.kind)
    }
}

/// The reference-collecting [`ParserListener`] feeding a shared [`Engine`]:
/// one parse over the same token table `collect` used, then pass 2 resolves
/// the rows.
struct DiagnoseListener<'a> {
    engine: Rc<RefCell<Engine<'a>>>,
}

/// Diagnoses one compilation unit against its library scope (#120, batch 1).
///
/// The unit must already be collected and indexed: `index` carries the
/// file's element model (its duplicate log folds into the front of the
/// report as `duplicate_definition`s) and its non-type-parameter
/// declarations are already declared into the library graph. The parse is a
/// second pass over the same token table `collect` used.
///
/// # Panics
///
/// Panics if the parser still holds the listener at teardown — impossible
/// by construction (`parser` is dropped above), asserted with `expect`.
#[allow(clippy::too_many_arguments)] // the analysis context, one field each
pub fn diagnose(
    source: &str,
    tokens: &TokenTable,
    features: ParserFeatures,
    strings: &StringTable,
    file: FileId,
    lib: LibraryId,
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &ElementIndex,
) -> FileReport {
    let engine = Rc::new(RefCell::new(Engine::new(
        graph, namespaces, index, strings, source, tokens, file, lib,
    )));
    let mut parser = Parser::new(
        source,
        tokens,
        features,
        DiagnoseListener {
            engine: Rc::clone(&engine),
        },
    );
    parser.parse_unit();
    drop(parser);
    // The parser held the only other strong reference and is gone, so the
    // unwrap cannot fail.
    let mut engine = Rc::into_inner(engine)
        .expect("the parser released its listener")
        .into_inner();
    engine.resolve_rows();
    let mut report = std::mem::take(&mut engine.report);
    // `duplicate_definition`s lead the report, each pointing at the SECOND
    // declaration's name token (the analyzer's diagnose-at-second rule).
    // Setters live under `name=` in the model; the oracle's message names
    // the bare identifier.
    let mut diagnostics = Vec::with_capacity(
        index
            .get(file)
            .map_or(0, |f| f.duplicates().len() + f.arity_violations().len()),
    );
    if let Some(elements) = index.get(file) {
        for dup in elements.duplicates() {
            let text = strings.resolve(dup.name);
            let display = text.strip_suffix('=').unwrap_or(text);
            diagnostics.push(Diagnostic::new(
                Severity::Error,
                SemanticCode::DuplicateDefinition.code_info(),
                SemanticCode::DuplicateDefinition.message(display, ""),
                file,
                dup.range,
                None,
            ));
        }
        // The declaration-arity log folds next (capture-time order =
        // source order): operator parameter counts and the setter shape
        // rule.
        for arity in elements.arity_violations() {
            let (code, message) = match arity.violation {
                ArityViolation::Operator {
                    name,
                    expected,
                    found,
                } => {
                    let code = SemanticCode::WrongNumberOfParametersForOperator;
                    (
                        code,
                        code.message3(
                            strings.resolve(name),
                            &expected.to_string(),
                            &found.to_string(),
                        ),
                    )
                }
                ArityViolation::OperatorMinus { found } => {
                    let code = SemanticCode::WrongNumberOfParametersForOperatorMinus;
                    (code, code.message3(&found.to_string(), "", ""))
                }
                ArityViolation::Setter => (
                    SemanticCode::WrongNumberOfParametersForSetter,
                    SemanticCode::WrongNumberOfParametersForSetter.message("", ""),
                ),
            };
            diagnostics.push(Diagnostic::new(
                code.severity(),
                code.code_info(),
                message,
                file,
                arity.range,
                None,
            ));
        }
    }
    diagnostics.append(&mut report.diagnostics);
    report.diagnostics = diagnostics;
    report
}

impl ParserListener for DiagnoseListener<'_> {
    // —— frames: containers ——————————————————————————————————————————

    fn begin_class_declaration(
        &mut self,
        _begin: Token,
        _abstract_kw: Option<Token>,
        _sealed_kw: Option<Token>,
        _base_kw: Option<Token>,
        _interface_kw: Option<Token>,
        _final_kw: Option<Token>,
        _augment_kw: Option<Token>,
        _mixin_kw: Option<Token>,
        name: Token,
    ) {
        let mut engine = self.engine.borrow_mut();
        let name = engine.name_of(name);
        engine.push_container_frame(ContainerKind::Class, name);
    }

    fn end_class_declaration(&mut self, _begin: Token, _end: Token) {
        self.engine.borrow_mut().pop_frame();
    }

    fn begin_named_mixin_application(
        &mut self,
        _begin: Token,
        _abstract_kw: Option<Token>,
        _sealed_kw: Option<Token>,
        _base_kw: Option<Token>,
        _interface_kw: Option<Token>,
        _final_kw: Option<Token>,
        _augment_kw: Option<Token>,
        _mixin_kw: Option<Token>,
        name: Token,
    ) {
        let mut engine = self.engine.borrow_mut();
        let name = engine.name_of(name);
        engine.push_container_frame(ContainerKind::Class, name);
    }

    fn end_named_mixin_application(
        &mut self,
        _begin: Token,
        _class: Token,
        _equals: Token,
        _implements: Option<Token>,
        _end: Token,
    ) {
        self.engine.borrow_mut().pop_frame();
    }

    fn begin_mixin_declaration(
        &mut self,
        _begin: Token,
        _augment_kw: Option<Token>,
        _base_kw: Option<Token>,
        _mixin: Token,
        name: Token,
    ) {
        let mut engine = self.engine.borrow_mut();
        let name = engine.name_of(name);
        engine.push_container_frame(ContainerKind::Mixin, name);
    }

    fn end_mixin_declaration(&mut self, _begin: Token, _end: Token) {
        self.engine.borrow_mut().pop_frame();
    }

    fn begin_enum_declaration(
        &mut self,
        _begin: Token,
        _augment_kw: Option<Token>,
        _enum_keyword: Token,
        name: Token,
    ) {
        let mut engine = self.engine.borrow_mut();
        let name = engine.name_of(name);
        engine.push_container_frame(ContainerKind::Enum, name);
    }

    fn end_enum_declaration(
        &mut self,
        _begin: Token,
        _enum_keyword: Token,
        _left_brace: Token,
        _member_count: usize,
        _end: Token,
    ) {
        self.engine.borrow_mut().pop_frame();
    }

    fn begin_extension_declaration(
        &mut self,
        _augment: Option<Token>,
        _extension_keyword: Token,
        name: Option<Token>,
    ) {
        let mut engine = self.engine.borrow_mut();
        let name = name.and_then(|token| engine.name_of(token));
        engine.push_container_frame(ContainerKind::Extension, name);
    }

    fn end_extension_declaration(
        &mut self,
        _begin: Token,
        _extension_keyword: Token,
        _on: Option<Token>,
        _end: Token,
    ) {
        self.engine.borrow_mut().pop_frame();
    }

    fn begin_extension_type_declaration(
        &mut self,
        _augment_kw: Option<Token>,
        _extension_keyword: Token,
        name: Token,
    ) {
        let mut engine = self.engine.borrow_mut();
        let name = engine.name_of(name);
        engine.push_container_frame(ContainerKind::Extension, name);
    }

    fn end_extension_type_declaration(
        &mut self,
        _begin: Token,
        _augment_kw: Option<Token>,
        _extension_keyword: Token,
        _type_keyword: Token,
        _end: Token,
    ) {
        self.engine.borrow_mut().pop_frame();
    }

    // —— frames: executables ——————————————————————————————————————————
    // The Executable frame opens at the declaration head so its type
    // parameters and formals live inside it; the body block nests within.

    fn begin_method(
        &mut self,
        _kind: DeclarationKind,
        _augment_kw: Option<Token>,
        _external_kw: Option<Token>,
        static_kw: Option<Token>,
        _covariant_kw: Option<Token>,
        _var_final_or_const: Option<Token>,
        _get_or_set: Option<Token>,
        _name: Token,
        _enclosing_declaration_name: Option<&str>,
    ) {
        self.engine
            .borrow_mut()
            .push_executable_frame(static_kw.is_none());
    }

    fn end_method(
        &mut self,
        _kind: DeclarationKind,
        _get_or_set: Option<Token>,
        _begin: Token,
        _begin_param: Token,
        _begin_initializers: Option<Token>,
        _end: Token,
    ) {
        self.engine.borrow_mut().pop_frame();
    }

    fn begin_top_level_method(
        &mut self,
        _last_consumed: Token,
        _augment: Option<Token>,
        _external: Option<Token>,
    ) {
        self.engine.borrow_mut().push_executable_frame(false);
    }

    fn end_top_level_method(&mut self, _begin: Token, _get_or_set: Option<Token>, _end: Token) {
        self.engine.borrow_mut().pop_frame();
    }

    fn begin_constructor(
        &mut self,
        _kind: DeclarationKind,
        _augment_kw: Option<Token>,
        _external_kw: Option<Token>,
        _static_kw: Option<Token>,
        _covariant_kw: Option<Token>,
        _var_final_or_const: Option<Token>,
        _get_or_set: Option<Token>,
        _new_kw: Option<Token>,
        _name: Token,
        _enclosing_declaration_name: Option<&str>,
    ) {
        self.engine.borrow_mut().push_executable_frame(true);
    }

    fn end_constructor(
        &mut self,
        _kind: DeclarationKind,
        _begin: Token,
        _new_kw: Option<Token>,
        _begin_param: Token,
        _begin_initializers: Option<Token>,
        _end: Token,
    ) {
        self.engine.borrow_mut().pop_frame();
    }

    fn begin_factory(
        &mut self,
        _kind: DeclarationKind,
        _last_consumed: Token,
        _augment_kw: Option<Token>,
        _external_kw: Option<Token>,
        _const_keyword: Option<Token>,
    ) {
        // Factories have no `this`.
        self.engine.borrow_mut().push_executable_frame(false);
    }

    fn end_factory(&mut self, _kind: DeclarationKind, _begin: Token, _factory: Token, _end: Token) {
        self.engine.borrow_mut().pop_frame();
    }

    fn begin_local_function_declaration(&mut self, _token: Token) {
        let instance = self.engine.borrow().instance_context();
        self.engine.borrow_mut().push_executable_frame(instance);
    }

    fn end_local_function_declaration(&mut self, _end: Token) {
        self.engine.borrow_mut().pop_frame();
    }

    fn begin_function_expression(&mut self, _token: Token) {
        let instance = self.engine.borrow().instance_context();
        self.engine.borrow_mut().push_executable_frame(instance);
    }

    fn end_function_expression(&mut self, _begin: Token, end: Token) {
        let mut engine = self.engine.borrow_mut();
        engine.pop_frame();
        engine.push_value_row(end);
    }

    fn begin_named_function_expression(&mut self, _token: Token) {
        let instance = self.engine.borrow().instance_context();
        self.engine.borrow_mut().push_executable_frame(instance);
    }

    fn end_named_function_expression(&mut self, end: Token) {
        let mut engine = self.engine.borrow_mut();
        engine.pop_frame();
        engine.push_value_row(end);
    }

    // —— frames: scopes ———————————————————————————————————————————————

    fn begin_typedef(&mut self, _token: Token) {
        self.engine.borrow_mut().push_block_frame();
    }

    fn end_typedef(
        &mut self,
        _augment: Option<Token>,
        _typedef: Token,
        _equals: Option<Token>,
        _end: Token,
    ) {
        self.engine.borrow_mut().pop_frame();
    }

    fn begin_catch_clause(&mut self, _token: Token) {
        // The catch frame owns the catch parameters; the body block's own
        // events are not overridable from outside dartq-parser (BlockKind
        // is crate-private), so block scoping comes from the executable and
        // catch frames only.
        self.engine.borrow_mut().push_block_frame();
    }

    fn end_catch_clause(&mut self, _token: Token) {
        self.engine.borrow_mut().pop_frame();
    }

    fn begin_for_statement(&mut self, _token: Token) {
        self.engine.borrow_mut().push_block_frame();
    }

    fn end_for_statement(&mut self, _end: Token) {
        self.engine.borrow_mut().pop_frame();
    }

    fn handle_for_in_loop_parts(
        &mut self,
        _await_kw: Option<Token>,
        _for_kw: Token,
        _left_paren: Token,
        _pattern_keyword: Option<Token>,
        _in_kw: Token,
    ) {
        self.engine.borrow_mut().push_block_frame();
    }

    fn end_for_in(&mut self, _end: Token) {
        self.engine.borrow_mut().pop_frame();
    }

    // —— scopes: locals and type parameters ———————————————————————————

    fn begin_variables_declaration(
        &mut self,
        _token: Token,
        _late_kw: Option<Token>,
        _var_final_or_const: Option<Token>,
    ) {
        let mut engine = self.engine.borrow_mut();
        // The group's declared type arms here (one `parse_type` per group,
        // `handle_no_type` disarmed it for untyped groups); every name in
        // the group binds it.
        engine.pending_decl_ty = engine.armed_ty.take();
    }

    fn end_variables_declaration(&mut self, _count: usize, _end: Option<Token>) {
        let mut engine = self.engine.borrow_mut();
        engine.pending_decl_ty = None;
        engine.armed_ty = None;
    }

    fn begin_variable_initializer(&mut self, _token: Token) {
        let mut engine = self.engine.borrow_mut();
        engine.init_mark = Some(engine.rows.len());
    }

    fn end_variable_initializer(&mut self, _token: Token) {
        self.engine.borrow_mut().check_decl_initializer();
    }

    fn begin_type_variable(&mut self, token: Token) {
        let mut engine = self.engine.borrow_mut();
        if let Some(name) = engine.name_of(token) {
            engine.push_type_parameter(name);
        }
    }

    fn begin_type_variables(&mut self, _token: Token) {
        // A new header started: a previous declaration's parked variables
        // never met their frame (a typedef's, say) — drop them.
        self.engine.borrow_mut().pending_type_params.clear();
    }

    // —— identifiers ——————————————————————————————————————————————————

    fn handle_identifier(&mut self, token: Token, context: IdentifierContext) {
        use IdentifierContext as C;
        let mut engine = self.engine.borrow_mut();
        let name = engine.name_of(token);
        match context {
            C::Expression => {
                if let Some(name) = name {
                    engine.push_bare(name, token.range);
                }
            }
            C::ExpressionContinuation => {
                if let Some(name) = name {
                    engine.push_member(name, token.range);
                }
            }
            C::TypeReference | C::TypeReferenceContinuation => {
                if let Some(name) = name {
                    let head = if context == C::TypeReference {
                        TypeHead::Bare
                    } else {
                        TypeHead::Continuation
                    };
                    engine.push_type_use(name, token.range, head);
                    // The head awaits its type-argument group (or its
                    // completing `handle_type`).
                    engine.arm_type_head(name, token.range);
                }
            }
            C::PrefixedTypeReference => {
                if let Some(name) = name {
                    engine.arm_type_prefix(name, token.range);
                }
            }
            // A constructor reference names a class: the guarded miss is the
            // oracle's `undefined_class` (`new Missing()`), never an
            // invocation.
            C::ConstructorReference => {
                if let Some(name) = name {
                    engine.push_type_use(name, token.range, TypeHead::Bare);
                }
            }
            C::LocalVariableDeclaration => {
                if let Some(name) = name {
                    engine.add_local(name);
                    engine.bind_declared_type(name);
                }
            }
            C::FormalParameterDeclaration | C::CatchParameter => {
                if let Some(name) = name {
                    engine.add_local(name);
                    engine.bind_parameter_type(name);
                }
            }
            C::NamedArgumentReference => {
                if let Some(name) = name {
                    engine.check_named_argument(name, token.range);
                    engine.open_argument(Some(name));
                }
            }
            C::EnumValueDeclaration => {
                // The enum constant's window opens here: its type arguments
                // check against the enclosing enum's declared count (the
                // `_enum` code). `handle_enum_element` closes it.
                engine.enter_enum_element();
            }
            C::ConstructorReferenceContinuationAfterTypeArguments
                if engine.in_enum_element.is_some() && engine.ctor_pos == 0 =>
            {
                // The enum constant's constructor selector (`c.other()`):
                // outside argument expressions (ctor_pos 0), the name must
                // be a constructor the enclosing enum declares
                // (`undefined_enum_constructor`).
                if let Some(name) = name {
                    engine.check_enum_constructor_selector(name, token.range);
                }
            }
            C::MetadataReference => {
                if let Some(name) = name {
                    engine.check_annotation(name, token.range);
                }
            }
            C::TopLevelFunctionDeclaration => {
                if let Some(name) = name {
                    engine.latch_return_type(ExecKind::Function, name);
                }
            }
            C::MethodDeclaration => {
                if let Some(name) = name {
                    engine.latch_return_type(ExecKind::Method, name);
                }
            }
            _ => {}
        }
    }

    fn handle_qualified(&mut self, _period: Token) {
        self.engine.borrow_mut().consume_type_prefix();
    }

    // —— declared types ——————————————————————————————————————————————

    fn handle_type(&mut self, name: Token, question: Option<Token>) {
        let mut engine = self.engine.borrow_mut();
        // The type completed without a group; its head slot is consumed.
        engine.pending_type_head = None;
        let Some(text) = engine.text_of(name) else {
            return;
        };
        // `dynamic` is a top type and `Null` is the null-type singleton —
        // both have dedicated [`Ty`] shapes.
        let ty = match text {
            "dynamic" => Ty::Dynamic,
            "Null" => Ty::Null,
            _ => Ty::Named {
                name: engine.strings.intern(text),
                nullable: question.is_some(),
            },
        };
        engine.armed_ty = Some(ty);
    }

    fn handle_no_type(&mut self, _last_consumed: Token) {
        self.engine.borrow_mut().armed_ty = None;
    }

    fn handle_void_keyword(&mut self, _token: Token) {
        self.engine.borrow_mut().armed_ty = Some(Ty::Void);
    }

    fn handle_void_keyword_with_type_arguments(&mut self, _token: Token) {
        self.engine.borrow_mut().armed_ty = Some(Ty::Void);
    }

    fn begin_initializers(&mut self, _token: Token) {
        self.engine.borrow_mut().initializer_depth += 1;
    }

    fn end_initializers(&mut self, _count: usize, _begin: Token, _end: Token) {
        self.engine.borrow_mut().initializer_depth -= 1;
    }

    fn begin_metadata(&mut self, token: Token) {
        self.engine.borrow_mut().annotation_at = Some(token.range);
    }

    fn end_metadata(&mut self, _begin: Token, _period_before_name: Option<Token>, _end: Token) {
        self.engine.borrow_mut().annotation_at = None;
    }

    // —— receivers and selectors ——————————————————————————————————————

    fn handle_this_expression(&mut self, token: Token, _context: IdentifierContext) {
        self.engine.borrow_mut().push_keyword_receiver(token, false);
    }

    fn handle_super_expression(&mut self, token: Token, _context: IdentifierContext) {
        self.engine.borrow_mut().push_keyword_receiver(token, true);
    }

    fn handle_dot_access(&mut self, _token: Token, _end: Token, is_null_aware: bool) {
        self.engine.borrow_mut().merge_member(is_null_aware);
    }

    fn handle_cascade_access(&mut self, _operator: Token, _end: Token, is_null_aware: bool) {
        self.engine.borrow_mut().merge_member(is_null_aware);
    }

    fn handle_send(&mut self, _begin: Token, _end: Token) {
        self.engine.borrow_mut().consume_send();
    }

    fn handle_assignment_expression(&mut self, operator: Token, _end: Token) {
        let mut engine = self.engine.borrow_mut();
        engine.mark_assignment(operator);
        engine.check_assignment(operator);
    }

    fn begin_binary_expression(&mut self, operator: Token) {
        let mut engine = self.engine.borrow_mut();
        let Some(text) = engine.text_of(operator) else {
            return;
        };
        if USER_OPERATORS.contains(&text) {
            let op = engine.intern(text);
            engine.mark_operator(op, operator.range);
        }
    }

    // —— suppression brackets —————————————————————————————————————————

    fn begin_is_operator_type(&mut self, _operator: Token) {
        self.engine.borrow_mut().enter_non_type_pos();
    }

    fn end_is_operator_type(&mut self, _operator: Token) {
        self.engine.borrow_mut().leave_non_type_pos();
    }

    fn begin_as_operator_type(&mut self, _operator: Token) {
        self.engine.borrow_mut().enter_non_type_pos();
    }

    fn end_as_operator_type(&mut self, _operator: Token) {
        self.engine.borrow_mut().leave_non_type_pos();
    }

    fn begin_new_expression(&mut self, _token: Token) {
        self.engine.borrow_mut().enter_ctor_pos();
    }

    fn end_new_expression(&mut self, _token: Token) {
        self.engine.borrow_mut().leave_ctor_pos();
    }

    fn begin_const_expression(&mut self, _const_keyword: Token) {
        self.engine.borrow_mut().enter_ctor_pos();
    }

    fn end_const_expression(&mut self, _token: Token) {
        self.engine.borrow_mut().leave_ctor_pos();
    }

    fn begin_implicit_creation_expression(&mut self, _token: Token) {
        self.engine.borrow_mut().enter_ctor_pos();
    }

    fn end_implicit_creation_expression(&mut self, _begin: Token, _open_angle: Token) {
        let mut engine = self.engine.borrow_mut();
        engine.end_implicit_creation();
        engine.leave_ctor_pos();
    }

    fn begin_arguments(&mut self, _token: Token) {
        self.engine.borrow_mut().open_arguments();
    }

    fn end_arguments(&mut self, _count: usize, _begin: Token, _end: Token) {
        self.engine.borrow_mut().close_arguments();
    }

    fn handle_positional_argument(&mut self, _token: Token) {
        self.engine.borrow_mut().open_argument(None);
    }

    // `handle_named_argument` needs no override: the parser fires it before
    // the value expression, so the named argument opened at its identifier
    // event stays pending until the next argument boundary closes it.

    fn begin_return_statement(&mut self, _token: Token) {
        let mut engine = self.engine.borrow_mut();
        engine.return_mark = Some(engine.rows.len());
    }

    fn end_return_statement(&mut self, has_expression: bool, _begin: Token, end: Token) {
        let mut engine = self.engine.borrow_mut();
        let mark = engine.return_mark.take();
        if !has_expression {
            // Bare `return;` is a `void` expression in the oracle's matrix.
            engine.check_return(Ty::Void, end.range);
            return;
        }
        if let Some(mark) = mark
            && let Some((ty, range)) = engine.single_row_since(mark)
        {
            engine.check_return(ty, range);
        }
    }

    fn handle_expression_function_body(&mut self, _arrow: Token, _end: Option<Token>) {
        let mut engine = self.engine.borrow_mut();
        let Some(mark) = engine.body_mark.take() else {
            return;
        };
        if let Some((ty, range)) = engine.single_row_since(mark) {
            engine.check_return(ty, range);
        }
    }

    fn handle_async_modifier(&mut self, _async_kw: Option<Token>, _star: Option<Token>) {
        let mut engine = self.engine.borrow_mut();
        engine.body_mark = Some(engine.rows.len());
    }

    fn begin_cascade(&mut self, _operator: Token) {
        self.engine.borrow_mut().in_cascade = true;
    }

    fn end_cascade(&mut self) {
        self.engine.borrow_mut().in_cascade = false;
    }

    fn handle_indexed_expression(&mut self, question: Option<Token>, open: Token, close: Token) {
        let mut engine = self.engine.borrow_mut();
        // The oracle anchors index-access reports on the bracket pair
        // (`property_element_resolver.dart`, `_reportUnresolvedIndex`).
        let bracket = SourceRange {
            offset: open.range.offset,
            length: close.range.end() - open.range.offset,
        };
        // The receiver: the closest row wholly before the `[`.
        let Some(pos) = engine
            .rows
            .iter()
            .rposition(|row| row.range.end() <= open.range.offset)
        else {
            return;
        };
        if question.is_some() {
            let ty = Engine::receiver_row_ty(&engine.rows[pos]);
            if ty.is_strictly_non_nullable() {
                let range = engine.rows[pos].range;
                engine.emit3(SemanticCode::InvalidNullAwareOperator, range, "?[", "[", "");
            }
        }
        // Fold (`a[i]`, `c.j[i] = v`): the index arguments leave the stack —
        // the receiver row carries the `[]` access with the bracket span, and
        // a value row stands for the result so compound-expression detection
        // (argument types, initializer checks) still sees one opaque row. A
        // receiver whose own resolution left no name row (call results,
        // groups) stays unfolded — its operator checks suppress anyway.
        if matches!(
            engine.rows[pos].shape,
            RowShape::Bare(_) | RowShape::Member { .. } | RowShape::Receiver(_)
        ) && engine.rows[pos].intent == Intent::Read
        {
            let index_op = engine.intern("[]");
            engine.rows.truncate(pos + 1);
            engine.rows[pos].intent = Intent::Operator(index_op, bracket);
            engine.push_value_row(close);
        }
    }

    // —— patterns & switch exhaustiveness (M2 face) ——————————————————————

    fn begin_pattern(&mut self, _token: Token) {
        let mut engine = self.engine.borrow_mut();
        engine.pattern_depth += 1;
        engine.pattern_stack.push(PatternFrame {
            leaves: Vec::new(),
            or_ops: 0,
        });
    }

    fn end_pattern(&mut self, _token: Token) {
        let mut engine = self.engine.borrow_mut();
        engine.pattern_depth = engine.pattern_depth.saturating_sub(1);
        let Some(frame) = engine.pattern_stack.pop() else {
            return;
        };
        let merged = Engine::collapse(frame.leaves, frame.or_ops);
        if engine.pattern_stack.is_empty() {
            // A root pattern: only an open case header's coverage feeds the
            // exhaustiveness walk; other roots (if-case, pattern
            // assignments) are dropped here, so debris can neither leak
            // into a later case nor grow the set.
            if engine.case_sink_open {
                engine.pattern_roots.push(merged);
            }
        } else if let Some(parent) = engine.pattern_stack.last_mut() {
            parent.leaves.push(merged);
        }
    }

    fn begin_binary_pattern(&mut self, operator: Token) {
        let mut engine = self.engine.borrow_mut();
        let is_or = engine.text_of(operator) == Some("||");
        if is_or && let Some(frame) = engine.pattern_stack.last_mut() {
            frame.or_ops += 1;
        }
    }

    fn end_binary_pattern(&mut self, _operator: Token) {}

    fn handle_cast_pattern(&mut self, _operator: Token) {
        let mut engine = self.engine.borrow_mut();
        // `p as X` covers X's values (official `castPatternToSpace`).
        let leaf = match engine.armed_ty.take() {
            Some(Ty::Named { name, nullable }) => Restriction::Named(name, nullable),
            _ => Restriction::Unknown,
        };
        engine.pattern_leaf(leaf);
    }

    fn handle_null_assert_pattern(&mut self, _bang: Token) {
        // `p!` covers the non-null side of the context.
        let mut engine = self.engine.borrow_mut();
        let leaf = match NameTy::from_semantic(
            engine.switches.last().map_or(Ty::Unknown, |c| c.scrutinee),
        ) {
            NameTy::Named(name, _) => Restriction::Named(name, false),
            _ => Restriction::Unknown,
        };
        engine.pattern_leaf(leaf);
    }

    fn handle_null_check_pattern(&mut self, _question: Token) {
        // `p?` covers the context values plus `null` (`nullCheckToSpace`):
        // a nullable context already covers null; a non-null context
        // unions with it.
        let mut engine = self.engine.borrow_mut();
        let leaf = match NameTy::from_semantic(
            engine.switches.last().map_or(Ty::Unknown, |c| c.scrutinee),
        ) {
            NameTy::Named(name, true) => Restriction::Named(name, true),
            NameTy::Named(name, false) => {
                Restriction::Union(vec![Restriction::Named(name, false), Restriction::Null])
            }
            _ => Restriction::Unknown,
        };
        engine.pattern_leaf(leaf);
    }

    fn handle_relational_pattern(&mut self, _operator: Token) {
        // Value comparisons are not representable at name level.
        self.engine.borrow_mut().pattern_leaf(Restriction::Unknown);
    }

    fn handle_list_pattern(&mut self, _count: usize, _left: Token, _right: Token) {
        // The list face arrives through keysOfInterest/additional properties
        // — not modeled at name level (documented under-report).
        self.engine.borrow_mut().pattern_leaf(Restriction::Unknown);
    }

    fn handle_map_pattern(&mut self, _count: usize, _left: Token, _right: Token) {
        self.engine.borrow_mut().pattern_leaf(Restriction::Unknown);
    }

    fn handle_record_pattern(&mut self, _left_paren: Token, _count: usize) {
        // Record shapes live in the interned lattice (`dartq-types`); the
        // name level has no record type, so the row is unknown-shaped and
        // the engine under-reports.
        self.engine.borrow_mut().pattern_leaf(Restriction::Unknown);
    }

    fn begin_constant_pattern(&mut self, _const_keyword: Option<Token>) {
        self.engine.borrow_mut().pending_const = None;
    }

    fn end_constant_pattern(&mut self, _const_keyword: Option<Token>) {
        let mut engine = self.engine.borrow_mut();
        let leaf = engine
            .pending_const
            .take()
            .unwrap_or_else(|| engine.classify_constant_tail());
        engine.pattern_leaf(leaf);
    }

    fn handle_object_pattern(
        &mut self,
        first_identifier: Token,
        dot: Option<Token>,
        _second_identifier: Option<Token>,
    ) {
        let mut engine = self.engine.borrow_mut();
        // `Type(...)` shapes: the first identifier is the type when no dot
        // follows; the `Type.field`/`:field` shorthands stay unmodeled.
        engine.pending_object_ty = if dot.is_none() {
            engine.name_of(first_identifier)
        } else {
            None
        };
    }

    fn handle_object_pattern_fields(&mut self, _count: usize, _begin: Token, _end: Token) {
        let mut engine = self.engine.borrow_mut();
        let leaf = engine
            .pending_object_ty
            .take()
            .map_or(Restriction::Unknown, |name| Restriction::Named(name, false));
        engine.pattern_leaf(leaf);
    }

    fn handle_declared_variable_pattern(
        &mut self,
        _keyword: Option<Token>,
        variable: Token,
        _in_assignment_pattern: bool,
    ) {
        let mut engine = self.engine.borrow_mut();
        // The bound variable joins the enclosing scope (the flat M1 frame;
        // the oracle's pattern scoping is finer, and a wider binding can
        // only under-report undefined-name, never false-positive).
        if let Some(name) = engine.name_of(variable) {
            engine.add_local(name);
            match engine.armed_ty.take() {
                Some(Ty::Named {
                    name: ty_name,
                    nullable,
                }) => {
                    engine.bind_var_type(
                        name,
                        Ty::Named {
                            name: ty_name,
                            nullable,
                        },
                    );
                    engine.pattern_leaf(Restriction::Named(ty_name, nullable));
                }
                ty => {
                    if let Some(ty) = ty {
                        engine.bind_var_type(name, ty);
                    }
                    // A type-free variable covers its context: no leaf.
                }
            }
        } else {
            engine.armed_ty = None;
        }
    }

    fn handle_wildcard_pattern(&mut self, _keyword: Option<Token>, _wildcard: Token) {
        // A typed wildcard (`M _`) covers the declared type's space (the
        // official wildcard converts through its type); a bare `_` (or
        // `var _`) covers the context: no leaf.
        let mut engine = self.engine.borrow_mut();
        let Some(Ty::Named { name, nullable }) = engine.armed_ty.take() else {
            return;
        };
        engine.pattern_leaf(Restriction::Named(name, nullable));
    }

    // —— switch statements ————————————————————————————————————————————

    fn begin_switch_block(&mut self, token: Token) {
        let kw = self
            .engine
            .borrow_mut()
            .switch_kw_latch
            .take()
            .unwrap_or(token.range);
        self.engine.borrow_mut().open_switch(false, kw);
    }

    /// Latches the `switch` keyword: the non-exhaustive anchor (the block
    /// event that follows only carries the `{`).
    fn begin_switch_statement(&mut self, token: Token) {
        self.engine.borrow_mut().switch_kw_latch = Some(token.range);
    }

    fn end_switch_block(&mut self, _count: usize, _begin: Token, _end: Token) {
        self.engine.borrow_mut().report_switch();
    }

    fn begin_switch_case_when_clause(&mut self, _token: Token) {}

    /// The case header's pattern window opens before its pattern parses;
    /// the root it produces is captured at [`Self::end_case_expression`].
    fn begin_case_expression(&mut self, _case_kw: Token) {
        self.engine.borrow_mut().case_sink_open = true;
    }

    /// One statement-case header completed: its pattern's coverage latches
    /// here with the guard flag and the `case`-keyword anchor (the oracle's
    /// `caseNode.keyword`, constant_verifier.dart:1076-1085). Shared-body
    /// groups (`case A: case B:`) capture one case per header.
    fn end_case_expression(&mut self, case_kw: Token, when: Option<Token>, _colon: Token) {
        let mut engine = self.engine.borrow_mut();
        let root = engine.close_case_sink();
        let space = engine.substitute_root(root);
        if let Some(ctx) = engine.switches.last_mut() {
            ctx.cases.push(NameCase {
                space,
                guarded: when.is_some(),
                anchor: case_kw.range,
            });
        }
    }

    fn end_switch_case(
        &mut self,
        _label_count: usize,
        _expression_count: usize,
        default_kw: Option<Token>,
        _colon_after_default: Option<Token>,
        _statement_count: usize,
        _begin: Token,
        _end: Token,
    ) {
        // The case headers already captured their spaces
        // ([`Self::end_case_expression`]); the body-level event only
        // latches `default` and closes any window a broken header left
        // open.
        let mut engine = self.engine.borrow_mut();
        engine.case_sink_open = false;
        if default_kw.is_some()
            && let Some(ctx) = engine.switches.last_mut()
        {
            ctx.default_seen = true;
        }
    }

    // —— switch expressions ————————————————————————————————————————————

    fn begin_switch_expression_block(&mut self, token: Token) {
        let kw = self
            .engine
            .borrow_mut()
            .switch_kw_latch
            .take()
            .unwrap_or(token.range);
        self.engine.borrow_mut().open_switch(true, kw);
    }

    /// Latches the `switch` keyword of a switch expression.
    fn begin_switch_expression(&mut self, token: Token) {
        self.engine.borrow_mut().switch_kw_latch = Some(token.range);
    }

    /// The expression-case header's pattern window opens before its
    /// pattern parses.
    fn begin_switch_expression_case(&mut self) {
        self.engine.borrow_mut().case_sink_open = true;
    }

    /// The expression case's pattern completed: its coverage latches now
    /// (the guard and the `=>` anchor only exist at the case's end).
    fn handle_switch_expression_case_pattern(&mut self, _token: Token) {
        let mut engine = self.engine.borrow_mut();
        let root = engine.close_case_sink();
        let space = engine.substitute_root(root);
        if let Some(ctx) = engine.switches.last_mut() {
            ctx.pending_root = Some(space);
        }
    }

    fn end_switch_expression_case(
        &mut self,
        _begin: Token,
        when: Option<Token>,
        arrow: Token,
        _end: Token,
    ) {
        let mut engine = self.engine.borrow_mut();
        let space = engine
            .switches
            .last_mut()
            .and_then(|ctx| ctx.pending_root.take())
            .unwrap_or(Restriction::Unknown);
        if let Some(ctx) = engine.switches.last_mut() {
            ctx.cases.push(NameCase {
                space,
                guarded: when.is_some(),
                anchor: arrow.range,
            });
        }
    }

    fn end_switch_expression_block(&mut self, _count: usize, _begin: Token, _end: Token) {
        self.engine.borrow_mut().report_switch();
    }

    // —— if-case ————————————————————————————————————————————————————————

    fn handle_parenthesized_condition(
        &mut self,
        _begin: Token,
        case_kw: Option<Token>,
        _when: Option<Token>,
    ) {
        // An if-case's pattern events follow; the binding face works
        // through `handle_declared_variable_pattern`, and the condition
        // itself carries no exhaustiveness question. Its root pattern is
        // never captured: the case-sink window is only open across a
        // switch-case header.
        let _ = case_kw;
    }

    fn handle_enum_element(&mut self, _begin: Token, _augment: Option<Token>) {
        // The parser fires this after the constant's type arguments and
        // constructor arguments: the constant's window closed (it opened at
        // the EnumValueDeclaration identifier).
        self.engine.borrow_mut().leave_enum_element();
    }

    fn handle_enum_elements(&mut self, _elements_end: Token, _count: usize) {
        self.engine.borrow_mut().leave_enum_element();
    }

    fn begin_type_arguments(&mut self, _token: Token) {
        let mut engine = self.engine.borrow_mut();
        let head = engine.pending_type_head.take();
        engine.group_heads.push(head);
    }

    fn end_type_arguments(&mut self, count: usize, begin: Token, end: Token) {
        self.engine
            .borrow_mut()
            .end_type_arguments(count, begin, end);
    }

    fn handle_no_type_arguments(&mut self, _token: Token) {
        // The type completed without a group; the head slot is consumed.
        self.engine.borrow_mut().pending_type_head = None;
    }

    fn handle_type_argument_application(&mut self, _open_angle: Token) {
        self.engine.borrow_mut().consume_type_argument_application();
    }

    // —— value rows: literals and groups ———————————————————————————————
    // Adjacent string literals push no extra row: each constituent string
    // already did.

    fn handle_literal_int(&mut self, token: Token) {
        self.engine.borrow_mut().push_primitive_row(token, 0);
    }

    fn handle_literal_int_with_separators(&mut self, token: Token) {
        self.engine.borrow_mut().push_primitive_row(token, 0);
    }

    fn handle_literal_double(&mut self, token: Token) {
        self.engine.borrow_mut().push_primitive_row(token, 1);
    }

    fn handle_literal_double_with_separators(&mut self, token: Token) {
        self.engine.borrow_mut().push_primitive_row(token, 1);
    }

    fn handle_literal_bool(&mut self, token: Token) {
        let mut engine = self.engine.borrow_mut();
        if engine.pattern_depth > 0 {
            let is_true = engine.text_of(token) == Some("true");
            engine.pending_const = Some(Restriction::BoolValue(is_true));
        }
        engine.push_primitive_row(token, 3);
    }

    fn handle_literal_null(&mut self, token: Token) {
        let mut engine = self.engine.borrow_mut();
        if engine.pattern_depth > 0 {
            engine.pending_const = Some(Restriction::Null);
        }
        engine.push_null_row(token);
    }

    fn end_literal_string(&mut self, _interpolation_count: usize, end: Token) {
        self.engine.borrow_mut().push_primitive_row(end, 4);
    }

    fn end_literal_symbol(&mut self, hash: Token, _identifier_count: usize) {
        self.engine.borrow_mut().push_value_row(hash);
    }

    fn handle_literal_list(
        &mut self,
        _count: usize,
        _left: Token,
        _const_keyword: Option<Token>,
        right: Token,
    ) {
        self.engine.borrow_mut().push_value_row(right);
    }

    fn handle_literal_set_or_map(
        &mut self,
        _count: usize,
        _left: Token,
        _const_keyword: Option<Token>,
        right: Token,
        _has_set_entry: bool,
    ) {
        self.engine.borrow_mut().push_value_row(right);
    }

    fn end_parenthesized_expression(&mut self, token: Token) {
        self.engine.borrow_mut().push_value_row(token);
    }

    fn end_record_literal(&mut self, token: Token, _count: usize, _const_keyword: Option<Token>) {
        self.engine.borrow_mut().push_value_row(token);
    }

    fn end_const_literal(&mut self, end: Token) {
        self.engine.borrow_mut().push_value_row(end);
    }
}

/// The §8 names-table use kind of a row's finalized intent.
fn use_kind_of(intent: &Intent) -> NameUse {
    match intent {
        Intent::Invoke => NameUse::Invoke,
        Intent::Write | Intent::ReadWrite => NameUse::Write,
        _ => NameUse::Read,
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
enum NameTy {
    Never,
    Null,
    /// `dynamic` — the nullable top.
    TopNullable,
    TopNonNullable,
    /// A named type as written, with the syntactic `?`. The nullable
    /// projection doubles as the engine's `Nullable` variant.
    Named(StringId, bool),
    /// An enum constant: the enum's name and the constant's.
    EnumValue(StringId, StringId),
    BoolValue(bool),
    /// The nullable projection of an enum constant
    /// (`NullableStaticType(EnumElementStaticType)`, static_type.dart:350):
    /// the constant plus `null`, never its siblings.
    EnumValueOrNull(StringId, StringId),
    /// The nullable projection of a `true`/`false` value
    /// (`NullableStaticType(_BoolValueStaticType)`, static_type.dart:350):
    /// the value plus `null`, never the other boolean.
    BoolOrNull(bool),
    /// A unique unknown (relational patterns, unrepresentable constants):
    /// matched by nothing.
    Unknown(u64),
}

/// One completed case: its coverage restriction, the guard flag, and the
/// token the diagnostics anchor to (`case` for statement cases, `=>` for
/// expression cases).
struct NameCase {
    space: Restriction,
    guarded: bool,
    anchor: SourceRange,
}

/// The open switch (statement or expression): the scrutinee's push-time
/// type, its keyword anchor, and the cases completed so far.
struct SwitchCtx {
    is_expression: bool,
    switch_kw: SourceRange,
    scrutinee: Ty,
    cases: Vec<NameCase>,
    default_seen: bool,
    /// The captured coverage of an in-flight switch-expression case (the
    /// space latches at the pattern's completion, the guard and anchor at
    /// the case's end). Statement cases capture in one step and leave this
    /// `None`.
    pending_root: Option<Restriction>,
}

/// One open `begin_pattern`..`end_pattern` bracket: the leaf restrictions
/// collected inside it and whether `||` joined any of them (an `||` union
/// keeps its alternatives apart; everything else intersects).
struct PatternFrame {
    leaves: Vec<Restriction>,
    or_ops: usize,
}

/// The name-level coverage a pattern leaf contributes (the space-building
/// face of `PatternConverter`, constant_verifier.dart:1031-1050).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Restriction {
    /// A type test as written (`X`, `X?`): cast patterns, typed variable
    /// patterns, object patterns.
    Named(StringId, bool),
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
    fn and(self, other: Restriction) -> Restriction {
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
fn case_space(world: &NameWorld<'_>, space: &Restriction, context: NameTy) -> Space<NameTy> {
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
        Restriction::BoolValue(value) => out.push(Single::full(NameTy::BoolValue(*value))),
        Restriction::EnumValue(enum_name, constant) => {
            out.push(Single::full(NameTy::EnumValue(*enum_name, *constant)));
        }
        // Substituted before the case is recorded; unreachable here.
        Restriction::NamedContext => out.push(Single::full(context)),
    }
}

/// The name-level [`ExhaustivenessWorld`]: sealed/enum facts come from the
/// unit's element model, subtype edges from the superclass and
/// `with`/`implements` names of the unit's containers, plus the fixed core
/// chain (`int`/`double` <: `num` <: `Object`) the nullability codes
/// already assume.
struct NameWorld<'a> {
    index: &'a ElementIndex,
    file: FileId,
    strings: &'a StringTable,
    /// The interned `bool` spelling (`BoolValue` subtyping compares ids,
    /// never re-interns per query).
    bool_id: StringId,
    unknowns: std::cell::Cell<u64>,
}

/// The two core edges every unit sees without a declaration (`num` has no
/// container; the nullability face already treats these names specially).
const CORE_SUPERTYPES: [(&str, &str); 3] = [("int", "num"), ("double", "num"), ("num", "Object")];

impl NameTy {
    fn from_semantic(ty: Ty) -> Self {
        match ty {
            Ty::Unknown | Ty::Dynamic | Ty::Void => NameTy::TopNullable,
            Ty::Null => NameTy::Null,
            Ty::Named { name, nullable } => NameTy::Named(name, nullable),
        }
    }
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
                NameTy::Named(_, true) | NameTy::EnumValueOrNull(..) | NameTy::BoolOrNull(_),
            ) => true,
            // `S <: T?` iff `S <: T` (static_type.dart:365-372); `S? <: T?`
            // likewise; `S? <: T` never (null matches no plain T).
            (NameTy::Named(sn, s), NameTy::Named(tn, t)) => (!*s || *t) && self.named_sub(*sn, *tn),
            // The bare value fits its own nullable projection (the
            // must-match rule: `true <: true?` through the identical
            // restriction, types.dart:52-56).
            (NameTy::EnumValue(en, c), NameTy::EnumValueOrNull(fn_, d)) => en == fn_ && c == d,
            (NameTy::BoolValue(v), NameTy::BoolOrNull(w)) => v == w,
            // A constant's type is its enum, non-null.
            (NameTy::EnumValue(en, _), sup) => self.is_subtype_of(&NameTy::Named(*en, false), sup),
            // The restricted nullable inherits the value's type edges, but
            // only toward nullable targets (its `null` half never fits a
            // non-nullable type).
            (NameTy::EnumValueOrNull(en, _), NameTy::Named(tn, true)) => {
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
            _ => false,
        }
    }

    fn is_sealed(&self, ty: &NameTy) -> bool {
        match ty {
            // Nullable projections split (static_type.dart:356).
            NameTy::Named(_, true)
            | NameTy::TopNullable
            | NameTy::EnumValueOrNull(..)
            | NameTy::BoolOrNull(_) => true,
            NameTy::Named(name, false) => self.is_sealed_name(*name),
            _ => false,
        }
    }

    fn subtypes(&self, ty: &NameTy) -> Vec<NameTy> {
        match ty {
            NameTy::Named(name, true) => vec![NameTy::Named(*name, false), NameTy::Null],
            NameTy::TopNullable => vec![NameTy::TopNonNullable, NameTy::Null],
            // NullableStaticType.getSubtypes: the value plus `null`
            // (static_type.dart:359-362).
            NameTy::EnumValueOrNull(enum_name, constant) => {
                vec![NameTy::EnumValue(*enum_name, *constant), NameTy::Null]
            }
            NameTy::BoolOrNull(value) => vec![NameTy::BoolValue(*value), NameTy::Null],
            NameTy::Named(name, false) => self.name_subtypes(*name),
            _ => Vec::new(),
        }
    }

    fn property_type(&self, _ty: &NameTy, _key: &Key) -> Option<NameTy> {
        // Records are not modeled at name level: record-pattern columns
        // fall back to the nullable object (exhaustive.dart:396-406).
        None
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

impl NameWorld<'_> {
    /// The unit's container record for `name`.
    fn container(&self, name: StringId) -> Option<&Container> {
        self.index.get(self.file).and_then(|f| f.container(name))
    }

    /// The unit's container names in declaration order.
    fn container_order(&self) -> &[StringId] {
        self.index
            .get(self.file)
            .map_or(&[], FileElements::container_order)
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
    fn named_sub(&self, sub: StringId, sup: StringId) -> bool {
        if sub == sup {
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
            for (base, parent) in CORE_SUPERTYPES {
                if self.strings.resolve(name) == base {
                    let parent_id = self.strings.intern(parent);
                    if parent_id == sup {
                        return true;
                    }
                    if seen.insert(parent_id) {
                        queue.push_back(parent_id);
                    }
                }
            }
        }
        false
    }

    /// The declared direct edges out of `name` (superclass + interfaces).
    fn direct_edges(&self, name: StringId) -> Vec<StringId> {
        let Some(container) = self.container(name) else {
            return Vec::new();
        };
        let mut edges = Vec::new();
        if let Some(superclass) = container.superclass {
            edges.push(superclass);
        }
        edges.extend(container.interfaces.iter().copied());
        edges
    }

    fn is_sealed_name(&self, name: StringId) -> bool {
        if name == self.bool_id {
            return true;
        }
        self.container(name)
            .is_some_and(|c| c.sealed || c.kind == ContainerKind::Enum)
    }

    /// Whether the unit's own facts decide `name`'s sealedness and
    /// subclass set: the declaration lives in this file, or the name is a
    /// core sealed type (`bool`). A cross-file name is opaque to the
    /// file-scoped index — its sealed classes and enum constants are
    /// invisible here, so a non-exhaustive verdict over it may be false.
    ///
    /// Interim under-report seam: the expression-face covering emit
    /// suppresses while this answers false. The oracle resolves
    /// cross-library facts through the whole library graph, down to the
    /// private-enum variant rows (`diagnostic.g.dart`
    /// `non_exhaustive_switch_expression_private`); feeding sealed and
    /// subclass facts from the library graph is future work, deferred
    /// together with those variants.
    fn name_resolvable(&self, name: StringId) -> bool {
        self.container(name).is_some() || self.is_sealed_name(name)
    }

    /// The closed subtype set of a sealed class or enum, in declaration
    /// order (types/sealed.dart, types/enum.dart).
    fn name_subtypes(&self, name: StringId) -> Vec<NameTy> {
        if name == self.bool_id {
            return vec![NameTy::BoolValue(true), NameTy::BoolValue(false)];
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
        for &decl in self.container_order() {
            // A well-founded closed set never contains its root: a
            // self-edge (`sealed S extends S`) drops here, before the
            // walk's cycle guard would discard it.
            if decl == name {
                continue;
            }
            let Some(declared) = self.container(decl) else {
                continue;
            };
            let edge = declared.superclass == Some(name) || declared.interfaces.contains(&name);
            if edge {
                subtypes.push(NameTy::Named(decl, false));
            }
        }
        subtypes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::collect;
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_resolution::{CombinatorClause, Combinators, LibraryGraphBuilder};
    use dartq_scanner::ScanConfig;
    use dartq_source::FileTable;
    use std::path::Path;

    /// Diagnoses `source` as the sole file of a library with no imports.
    /// Core-library names are deliberately absent: an M1 harness must model
    /// them through concrete declarations or shown broken edges — see the
    /// undefined-guard module docs.
    fn report(source: &str) -> FileReport {
        report_with_edge(source, |_| None)
    }

    /// [`report`], plus one extra import edge built with the live string
    /// table (combinator names need interning).
    fn report_with_edge(
        source: &str,
        make_edge: impl FnOnce(&StringTable) -> Option<ImportEdge>,
    ) -> FileReport {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file = files.intern(Path::new("main.dart"));
        let scanned = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(
            scanned.diagnostics.is_empty(),
            "scanner diagnostics: {source:?}"
        );
        let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
            LanguageVersion::new(3, 15),
        ));
        let elements = collect(source, &scanned.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib = builder.intern("package:t/main.dart");
        let mut index = ElementIndex::default();
        index.insert(file, elements.clone());
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
        if let Some(edge) = make_edge(&strings) {
            builder.import(lib, edge);
        }
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        diagnose(
            source,
            &scanned.tokens,
            features,
            &strings,
            file,
            lib,
            &graph,
            &namespaces,
            &index,
        )
    }

    fn code_names(report: &FileReport) -> Vec<&'static str> {
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    }

    /// A bare undefined read reports `undefined_identifier` with the oracle's
    /// verbatim text; the miss also lands in the §8 names table.
    ///
    /// The static-nullability trio with primitive literal types (#350): a
    /// String literal into an int parameter / variable / return host reports
    /// each verbatim text; the same-name and int-literal-in-double-context
    /// shapes stay silent (the oracle's literal conversion).
    ///
    /// Hand-trace: `f('s')` — the argument flush attributes the single
    /// String-typed row; the formal's declared `int` answers
    /// [`Ty::assignable_from`] `None` (distinct named types), the final-
    /// primitive table decides `false`. The declaration initializer and the
    /// return check read the same table.
    #[test]
    fn primitive_literals_drive_the_assignability_trio() {
        let report = report(
            "class int {} class double {} \
             void f(int x) {} int g() { return 's'; } \
             void main() { f('s'); int y = 's'; double d = 1; int ok = 1; }",
        );
        assert_eq!(
            code_names(&report),
            [
                "return_of_invalid_type",
                "argument_type_not_assignable",
                "invalid_assignment",
            ],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "A value of type 'String' can't be returned from the function 'g' because it has a return type of 'int'."
        );
        assert_eq!(
            report.diagnostics[1].message,
            "The argument type 'String' can't be assigned to the parameter type 'int'. "
        );
        assert_eq!(
            report.diagnostics[2].message,
            "A value of type 'String' can't be assigned to a variable of type 'int'."
        );
    }

    /// A fully missing annotation name reports `undefined_annotation` over
    /// the `@` (#350; the 3.13.1 caliber — the diagnostic was removed
    /// upstream in 3.14). A defined class annotation stays silent.
    ///
    /// Hand-trace: `@gone` — `begin_metadata` latches the `@`; the
    /// MetadataReference identifier resolves `Missing` (the annotation name
    /// is no declaration) -> the emit spans `@gone`.
    #[test]
    fn missing_annotation_reports_over_the_at_sign() {
        let report = report("class A {} @gone void f() {} @A void g() {}");
        assert_eq!(code_names(&report), ["undefined_annotation"]);
        let d = &report.diagnostics[0];
        assert_eq!(d.message, "Undefined name 'gone' used as an annotation.");
        assert_eq!(u32::try_from("class A {}@".len()).unwrap(), d.range.offset);
        assert_eq!(u32::try_from("@gone".len()).unwrap(), d.range.length);
    }

    /// `super.named()` in a constructor-initializer list is a super
    /// *constructor* reference: the named shape reports
    /// `undefined_constructor_in_initializer` against the superclass, and
    /// the member pass never mis-reports `undefined_super_member` (#350).
    /// A declared selector stays silent.
    ///
    /// Hand-trace: the initializer bracket raises the depth; `super.named()`
    /// merges at the dot with a `Super` receiver — the merge emits from the
    /// superclass constructor face and folds the row, so pass 2 sees only a
    /// value.
    #[test]
    fn super_initializer_selector_reports_the_class_constructor() {
        let report = report(
            "class A { A.named(); } \
             class B extends A { B.bad() : super.missing(); B.good() : super.named(); }",
        );
        assert_eq!(
            code_names(&report),
            ["undefined_constructor_in_initializer"]
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The class 'A' doesn't have a constructor named 'missing'."
        );
    }

    /// An enum constant's constructor selector names a constructor the enum
    /// does not declare: `undefined_enum_constructor` (#350). A declared
    /// selector stays silent.
    ///
    /// Hand-trace: `c.bad()` — the selector identifier fires inside the
    /// enum-element window; the enum's constructor face is `{other}` (the
    /// named selector) with no unnamed constructor; `bad` misses -> the
    /// emit at the selector.
    #[test]
    fn enum_constant_selector_reports_the_enum_constructor() {
        let report = report("enum E { c.bad(); const E.other(); }");
        assert_eq!(code_names(&report), ["undefined_enum_constructor"]);
        assert_eq!(
            report.diagnostics[0].message,
            "The enum doesn't have a constructor named 'bad'."
        );
    }

    /// The explicit extension application resolves its members against the
    /// extension's face — `E('x').h` reports the extension trio (#350),
    /// including the operator shape at the operator token — and `Object`
    /// members (the `on` type's face) stay silent.
    ///
    /// Hand-trace: `E(1)` folds at the send into a `Created` receiver; the
    /// merged member resolves through the extension kind's
    /// `check_container_member`; `toString` suppresses (the `on` type
    /// carries it).
    #[test]
    fn extension_application_reports_the_extension_trio() {
        let report = report(
            "class int {} \
             extension E on int { int get g => 1; void m() {} set s(int v) {} int operator +(int o) => 1; } \
             void f() { E(1).h; E(1).n(); E(1).t = 1; E(1) * 2; E(1).toString; }",
        );
        assert_eq!(
            code_names(&report),
            [
                "undefined_extension_getter",
                "undefined_extension_method",
                "undefined_extension_setter",
                "undefined_extension_operator",
            ],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The getter 'h' isn't defined for the extension 'E'."
        );
        assert_eq!(
            report.diagnostics[3].message,
            "The operator '*' isn't defined for the extension 'E'."
        );
    }

    /// The external-primitive member face (#350): a member access on a
    /// dart:core primitive receiver reports the interface trio when the
    /// primitive genuinely lacks the name, and stays silent on its real
    /// surface (`isEven`, `toString`).
    ///
    /// Hand-trace: `int x` types the receiver `int` — unresolvable as a
    /// container, the walk chain-breaks at the root and
    /// [`CORE_TYPE_MEMBERS`] decides. Locally declared faces keep the
    /// regular walk.
    #[test]
    fn primitive_receivers_report_the_member_trio() {
        let report = report(
            "void f() { int x = 1; x.noSuchGetter; x.noSuchMethodHere(); x.noSuchSetter = 2; x.isEven; x.toString; }",
        );
        // The receiver type `int` is undeclared in the harness: its type use
        // carries the known `undefined_class` row ahead of the member trio.
        assert_eq!(
            code_names(&report),
            [
                "undefined_class",
                "undefined_getter",
                "undefined_method",
                "undefined_setter",
            ],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[1].message,
            "The getter 'noSuchGetter' isn't defined for the type 'int'."
        );
        assert_eq!(
            report.diagnostics[2].message,
            "The method 'noSuchMethodHere' isn't defined for the type 'int'."
        );
        assert_eq!(
            report.diagnostics[3].message,
            "The setter 'noSuchSetter' isn't defined for the type 'int'."
        );
    }

    /// A creation result used as an operator receiver reports
    /// `undefined_operator` at the operator token (#350); the creation's own
    /// instance face still resolves (`Object` members, superclass chains).
    ///
    /// Hand-trace: `B() + 1` — the send folds the class callee into a
    /// `Created` receiver; the operator intent walks `B`'s member face and
    /// misses `+`. `B().m()` resolves through the superclass edge;
    /// `B().toString` suppresses (the Object member every class carries).
    #[test]
    fn creation_operator_reports_undefined_operator() {
        let report = report(
            "class A { void m() {} } class B extends A {} \
             void f() { B() + 1; B().m(); B().toString; C() + 1; } class C { void nosuch() {} }",
        );
        assert_eq!(
            code_names(&report),
            ["undefined_operator", "undefined_operator"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The operator '+' isn't defined for the type 'B'."
        );
        assert_eq!(
            report.diagnostics[1].message,
            "The operator '+' isn't defined for the type 'C'."
        );
    }

    /// A member's own type parameters scope over its whole signature
    /// (#350): the return-type row pushed before `<T>` parsed re-scopes, so
    /// no `undefined_class` fires — and the invocation arity checks still
    /// answer.
    ///
    /// Hand-trace: `T m<T>(T x)` — the Executable frame opens before the
    /// return type; `begin_type_variable` joins the frame's `type_params`
    /// and rewrites the still-missing `T` rows above the frame's row mark.
    #[test]
    fn member_type_parameters_scope_over_the_signature() {
        let report = report(
            "class D {} class C { T m<T>(T x) => x; static T sm<T>(T x) => x; } \
             void g<T>(D x) {} \
             void f() { var d = D(); g<D, D>(d); }",
        );
        assert_eq!(
            code_names(&report),
            ["wrong_number_of_type_arguments_element"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The function 'g' is declared with 1 type parameters, but 2 type arguments are given."
        );
    }

    /// Hand-trace: `f` opens the Executable frame; `nose` resolves through
    /// locals → type parameters → container face (none) → extension pool →
    /// `lookup_scoped` → `NotFound` (`Missing`); the argless send never opens
    /// an argument group, so the row stays a read and emits.
    #[test]
    fn bare_miss_reports_undefined_identifier() {
        let report = report("void f() { nose; }");
        assert_eq!(code_names(&report), ["undefined_identifier"]);
        let d = &report.diagnostics[0];
        assert_eq!(d.message, "Undefined name 'nose'.");
        assert_eq!(d.severity, Severity::Error);
        assert_eq!(d.range.offset, u32::try_from("void f() { ".len()).unwrap());
        assert_eq!(report.names.len(), 1);
        assert_eq!(report.names[0].use_kind, NameUse::Read);
        assert!(!report.names[0].qualified);
    }

    /// Locals shadow the library scope while in scope; `var` drops any stale
    /// declaration type so the bound name stays an unknown receiver.
    ///
    /// Hand-trace: `var x = 0;` clears `last_type`, adds `x` to the Executable
    /// frame; `x;` resolves `Plain` (no row); `nose;` reports.
    #[test]
    fn locals_shadow_and_stay_scoped() {
        let report = report("void f() { var x = 0; x; nose; }");
        assert_eq!(code_names(&report), ["undefined_identifier"]);
        assert_eq!(report.diagnostics[0].message, "Undefined name 'nose'.");
    }

    /// An invocation-callee miss is `undefined_function`; a bare read stays
    /// `undefined_identifier` (`foo;` never opens an argument group).
    ///
    /// Hand-trace: `foo(` opens a group registering the callee row; at `)`
    /// it becomes `pending_send` and `handle_send` consumes it (invoke). The
    /// inner identifier of `f(g)` would not: its `handle_send` finds no
    /// pending group and leaves the read intact.
    #[test]
    fn invoke_versus_read() {
        let report = report("void f() { foo(); bar; }");
        assert_eq!(
            code_names(&report),
            ["undefined_function", "undefined_identifier"]
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The function 'foo' isn't defined."
        );
        assert_eq!(report.names[0].use_kind, NameUse::Invoke);
    }

    /// An assignment target reports `undefined_identifier` (the oracle's
    /// code for a fully missing write target); the literal row to the right
    /// of the operator is skipped by the scan.
    #[test]
    fn assignment_target_reports_identifier() {
        let report = report("void f() { x = 1; }");
        assert_eq!(code_names(&report), ["undefined_identifier"]);
        assert_eq!(report.names[0].use_kind, NameUse::Write);
    }

    /// Type positions report `undefined_class`; `dynamic` is a builtin;
    /// in-scope type parameters resolve; file-local classes never reach the
    /// type path.
    ///
    /// Hand-trace: `T` sits in `type_params` while the Container frame is
    /// open → `Plain`; `Missing` walks `C`'s member face (absent), then the
    /// library scope → `NotFound` → `TypeUse(Missing)` → emit; `dynamic`
    /// short-circuits on the builtin table.
    #[test]
    fn type_positions_report_undefined_class() {
        let report = report("class C<T> { T? t; Missing m; dynamic d; }");
        assert_eq!(code_names(&report), ["undefined_class"]);
        assert_eq!(report.diagnostics[0].message, "Undefined class 'Missing'.");
        assert_eq!(report.names[0].use_kind, NameUse::Type);
    }

    /// `new C()` on a defined class is silent; `new Missing()` is the
    /// oracle's `undefined_class` (a constructor reference names a class,
    /// never an invocation).
    ///
    /// Hand-trace: `begin_new_expression` raises `ctor_pos`, so the argument
    /// group registers no callee and no send consumption can misfire; the
    /// `ConstructorReference` identifier resolves as a type use.
    #[test]
    fn constructor_reference_resolves_as_type() {
        let report = report("class C {} void f() { new C(); new Missing(); }");
        assert_eq!(code_names(&report), ["undefined_class"]);
        assert_eq!(report.diagnostics[0].message, "Undefined class 'Missing'.");
    }

    /// `duplicate_definition` folds from the collector's log, pointing at the
    /// SECOND declaration's name token, ahead of the row diagnostics; the
    /// model keeps the first entry.
    #[test]
    fn duplicates_fold_first() {
        let report = report("var x = 0; void f() { nose; } var x = 1;");
        assert_eq!(
            code_names(&report),
            ["duplicate_definition", "undefined_identifier"]
        );
        let dup = &report.diagnostics[0];
        assert_eq!(dup.message, "The name 'x' is already defined.");
        // The second `x`'s token: after `var ` of the third declaration.
        assert_eq!(
            dup.range.offset,
            u32::try_from("var x = 0; void f() { nose; } var ".len()).unwrap()
        );
    }

    /// Getter/setter pairs coexist (distinct model spellings); two members
    /// of the same static/instance bit are duplicates.
    ///
    /// Hand-trace: `g` and `g=` are distinct keys; both `m` commits hit the
    /// same key with the same instance bit — the second logs a duplicate.
    #[test]
    fn member_duplicates_respect_the_static_bit() {
        let report = report("class N { N get g => N(); set g(N v) {} void m() {} void m() {} }");
        assert_eq!(code_names(&report), ["duplicate_definition"]);
        assert_eq!(
            report.diagnostics[0].message,
            "The name 'm' is already defined."
        );
    }

    /// A name shown by a broken import is suppressed
    /// (`shouldIgnoreUndefined`) but still recorded in the §8 names table.
    ///
    /// Hand-trace: the edge matches (target `None`, prefix `None`) and
    /// `shows(gone)` holds — the guard fires before the emit.
    #[test]
    fn broken_import_shown_name_is_suppressed() {
        let report = report_with_edge("void f() { gone; }", |strings| {
            Some(ImportEdge {
                target: None,
                prefix: None,
                deferred: false,
                combinators: Combinators::new(
                    [CombinatorClause::Show(
                        vec![strings.intern("gone")].into_boxed_slice(),
                    )],
                    strings,
                ),
            })
        });
        assert!(report.diagnostics.is_empty());
        assert_eq!(report.names.len(), 1);
        assert_eq!(report.names[0].use_kind, NameUse::Read);
    }

    /// A bare miss under an *unfiltered* broken import still reports — the
    /// oracle ignores only show-listed names (`shouldIgnoreUndefined`,
    /// element.dart:8217-8250; the C7 shape).
    ///
    /// Hand-trace: the edge matches (target `None`, prefix `None`) but has
    /// no `show` clause, so neither guard arm fires and the emit happens.
    #[test]
    fn unfiltered_broken_import_keeps_bare_miss() {
        let report = report_with_edge("void f() { gone; }", |_| {
            Some(ImportEdge {
                target: None,
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            })
        });
        assert_eq!(code_names(&report), ["undefined_identifier"]);
    }

    /// A defined prefixed function resolves silently; a prefixed invocation
    /// miss through a resolved prefix reports `undefined_function` (the
    /// qualified shape of the family's trigger).
    ///
    /// Hand-trace: `p.real` merges with the `Prefix` receiver and
    /// `lookup_prefixed` finds `real` — no report; `gone` misses the prefix
    /// table and the guard skips resolved imports — the emit fires.
    #[test]
    fn prefixed_invoke_miss_reports_undefined_function() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file_a = files.intern(Path::new("a.dart"));
        let file_m = files.intern(Path::new("main.dart"));
        let source_a = "void real() {}";
        let source_m = "import 'a.dart' as p; void f() { p.real(); p.gone(); }";
        let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
            LanguageVersion::new(3, 15),
        ));
        let scanned_a = dartq_scanner::scan(
            source_a,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file_a,
        );
        let scanned_m = dartq_scanner::scan(
            source_m,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file_m,
        );
        let elements_a = collect(source_a, &scanned_a.tokens, features, &strings);
        let elements_m = collect(source_m, &scanned_m.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib_a = builder.intern("package:t/a.dart");
        let lib_m = builder.intern("package:t/main.dart");
        let mut index = ElementIndex::default();
        index.insert(file_a, elements_a.clone());
        index.insert(file_m, elements_m.clone());
        for (name, _) in elements_a.declarations() {
            builder.declare(lib_a, name, file_a);
        }
        for (name, decl) in elements_m.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib_m, name, file_m);
        }
        builder.import(
            lib_m,
            ImportEdge {
                target: Some(lib_a),
                prefix: Some(strings.intern("p")),
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let report = diagnose(
            source_m,
            &scanned_m.tokens,
            features,
            &strings,
            file_m,
            lib_m,
            &graph,
            &namespaces,
            &index,
        );
        assert_eq!(code_names(&report), ["undefined_function"]);
        assert_eq!(
            report.diagnostics[0].message,
            "The function 'gone' isn't defined."
        );
        assert!(report.names[0].qualified);
    }

    /// Named arguments are membership-tested against the callee's formals:
    /// a resolved callee with a missing label reports
    /// `undefined_named_parameter` at the label; an unresolved callee stays
    /// silent for the label (its own invocation code reports).
    ///
    /// Hand-trace: `g`'s group is open while the labels fire, so
    /// `top_level_decl(g)` resolves and its named formals are `["n"]`; `n`
    /// is contained (silent), `bad` misses a concrete formal table → emit +
    /// record. `nos` misses `top_level_decl` (unresolved) → `bad2` records
    /// only, and the invocation itself is `undefined_function`.
    #[test]
    fn named_argument_miss_reports_on_resolved_callee() {
        let report = report("void g({var n}) {} void f() { g(n: 1, bad: 2); nos(bad2: 3); }");
        assert_eq!(
            code_names(&report),
            ["undefined_named_parameter", "undefined_function"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The named parameter 'bad' isn't defined."
        );
        let named: Vec<_> = report
            .names
            .iter()
            .filter(|row| row.use_kind == NameUse::NamedArgument)
            .collect();
        assert_eq!(named.len(), 2, "both labels feed the §8 table");
    }

    /// The named-argument membership test runs only against a real
    /// top-level function's formals (`type_analyzer.dart:5811-5824`: the
    /// code fires when the invoked target's named-parameter table exists
    /// and lacks the name). A class callee is a constructor invocation — a
    /// constructor accepting the label is silent — and a getter callee is a
    /// property access; both previously false-positived on their
    /// `formals: None` declarations.
    ///
    /// Hand-trace: `C(x: 1)`'s bare head resolves `Container`, and the
    /// filtered `top_level_decl` leaves it unresolved (silent); `g(x: 1)`'s
    /// getter declaration carries no formals (silent); `bad` on the real
    /// function `g` still reports.
    #[test]
    fn named_argument_gates_on_a_real_function_formals_table() {
        let class_callee = report("class C { C({var x}); } void f() { C(x: 1); }");
        assert!(
            class_callee.diagnostics.is_empty(),
            "{:?}",
            code_names(&class_callee)
        );
        let getter_callee = report("class C {} C get g => C(); void f() { g(x: 1); }");
        assert!(
            getter_callee.diagnostics.is_empty(),
            "{:?}",
            code_names(&getter_callee)
        );
        let real_miss = report("void g({var n}) {} void f() { g(bad: 1); }");
        assert_eq!(code_names(&real_miss), ["undefined_named_parameter"]);
        assert_eq!(
            real_miss.diagnostics[0].message,
            "The named parameter 'bad' isn't defined."
        );
    }

    /// `await` in a sync body is `undefined_identifier_await` (no
    /// arguments); in an async body the parser reads `await` as a real
    /// await-expression and nothing fires.
    ///
    /// Hand-trace: the parser reads `await` as a plain identifier in `f`'s
    /// sync body; `in_executable` holds and the guard passes → the
    /// no-argument emit; `x` still reports `undefined_identifier`. In `g`
    /// the async modifier makes `await` an expression — no identifier event.
    ///
    /// RED (parser gap, #339): the statement-position await probe
    /// (`looks_like_await_or_yield_expression`, statements.rs) returns
    /// "declaration" for `await x;` in a sync body, so `await x` parses as
    /// a variable declaration (type `await`, name `x`) — the engine sees a
    /// missing type use (`undefined_class`) and no `x` read. Un-reds when
    /// the probe stops reading `await name` as a declaration head in sync
    /// contexts.
    #[test]
    fn await_in_sync_body_reports_await_code() {
        let report = report("void f() { await x; } void g() async { await f; }");
        assert_eq!(
            code_names(&report),
            ["undefined_identifier_await", "undefined_identifier"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "Undefined name 'await' in function body not marked with 'async'."
        );
    }

    /// Defined names never report: a top-level function as a value, and a
    /// static member through its class face.
    ///
    /// Hand-trace: `fn` resolves `Plain`; `C` resolves `Container` and the
    /// merged member row hits `C`'s member map (`s`) — both silent, no §8
    /// rows (defined references are the index's rows, not the recovery
    /// surface's).
    #[test]
    fn defined_names_never_report() {
        let report = report("class C { static var s = 0; } void fn() {} void f() { fn; C.s; }");
        assert!(report.diagnostics.is_empty(), "{:?}", code_names(&report));
        assert!(report.names.is_empty());
    }

    // —— batch 2 (#121): wrong_number_of_type_arguments ——————————————————

    /// A type annotation's arity is the declaration's type-parameter count.
    ///
    /// Hand-trace: `C` fires `TypeReference` (resolves — no row) and arms the
    /// pending head; `begin_type_arguments` moves the head into the group
    /// slot; `end_type_arguments(1)` pops it and checks `C`'s declared count
    /// (2, from the model) against 1 → base code at the group. `A` resolves
    /// (silent); `handle_type(C)` then binds `x`'s declared type.
    #[test]
    fn annotation_arity_reports_base_code() {
        let report = report("class A {} class C<T, U> {} void f() { C<A> x; }");
        assert_eq!(
            code_names(&report),
            ["wrong_number_of_type_arguments"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The type 'C' is declared with 2 type parameters, but 1 type arguments were given."
        );
        assert!(report.names.is_empty());
    }

    /// A creation type reference reports the base code too
    /// (oracle `test_const_nonGeneric`).
    ///
    /// Hand-trace: `const` raises `ctor_pos`; `C` fires `Expression` (a
    /// creation head, never a type reference) resolving as a container; the
    /// group's head slot is empty and the `ctor_pos` branch checks the
    /// container's count (0) against 1 → base code. `new C()` stays silent.
    ///
    /// RED (parser gap, #339): the `const` statement probe
    /// (`parse_expression_statement_or_const_declaration` →
    /// `compute_type`) consumes `C<A>` as a declaration type, so
    /// `const C<A>();` parses as a const-declaration head instead of a
    /// const-creation expression — the engine receives no creation head or
    /// type-argument events at all (observed output: `[]`). Un-reds when
    /// `compute_type` refuses the `Type<args>(` shape in expression-statement
    /// context (the official `(`-follower veto).
    #[test]
    fn creation_arity_reports_base_code() {
        let report = report("class A {} class C {} void f() { new C(); const C<A>(); }");
        assert_eq!(code_names(&report), ["wrong_number_of_type_arguments"]);
        assert_eq!(
            report.diagnostics[0].message,
            "The type 'C' is declared with 0 type parameters, but 1 type arguments were given."
        );
    }

    /// Type arguments on a named constructor segment report `_constructor`:
    /// constructors never declare type parameters (`ast_rewrite.dart:549`).
    ///
    /// Hand-trace: `parse_constructor_reference` keeps `C` and `named`
    /// unmerged on the row stack (no dot event inside the reference), so the
    /// `ctor_pos` branch sees two bare rows — the class face (a resolved
    /// class container) then the segment — and reports the pair. `D`
    /// resolves, so no other code fires.
    #[test]
    fn named_constructor_type_args_report_constructor_code() {
        let report = report("class D {} class C { C.named(); } void f() { new C.named<D>(); }");
        assert_eq!(
            code_names(&report),
            ["wrong_number_of_type_arguments_constructor"]
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The constructor 'C.named' doesn't have type parameters."
        );
    }

    /// An enum constant's constructor type arguments report `_enum`
    /// (`type_arguments_verifier.dart:163`).
    ///
    /// Hand-trace: `handle_enum_element` snapshots the enclosing enum's
    /// declared count (2, from the collector's depth-1 group); the constant's
    /// group closes with 1 → counts differ → `_enum` at the group. `a`
    /// itself fires `EnumValueDeclaration` (no row), so the trailing `()`
    /// opens a group with no callee — silent.
    #[test]
    fn enum_constant_type_args_report_enum_code() {
        let report = report("class D {} enum E<T, U> { a<D>(); }");
        assert_eq!(code_names(&report), ["wrong_number_of_type_arguments_enum"]);
        assert_eq!(
            report.diagnostics[0].message,
            "The enum is declared with 2 type parameters, but 1 type arguments were given."
        );
    }

    /// An invoked generic function with explicit type arguments reports
    /// `_element` with the `function` kind word (the oracle's target
    /// dispatch, `type_instantiation_target.dart`: the invocation syntax
    /// parses as an implicit creation and resolution reports through
    /// `InvocationTargetExecutableElement`).
    ///
    /// Hand-trace: `g<A, A>()` parses as an implicit creation (`ctor_pos`
    /// raised), so `g` stays a read and the group defers;
    /// `end_implicit_creation` pops the deferred entry, sees a bare
    /// top-level function head, and checks its declared count (1) against 2.
    /// The `A` arguments resolve — no other code.
    #[test]
    fn generic_invocation_arity_reports_element_function() {
        let report = report("class A {} void g<T>() {} void f() { g<A, A>(); }");
        assert_eq!(
            code_names(&report),
            ["wrong_number_of_type_arguments_element"]
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The function 'g' is declared with 1 type parameters, but 2 type arguments are given."
        );
    }

    /// An invoked generic member method reports `_element` with the `method`
    /// kind word through the send path.
    ///
    /// Hand-trace: `c.m<D>()` keeps a normal send (dot access, not an
    /// implicit creation): the group defers, `consume_send` matches the
    /// deferred entry to `m`'s callee row, and the member walk finds `m`'s
    /// declared count (2) → `_element`/`method`. `c`'s declared type comes
    /// from the formal binding.
    ///
    /// RED (parser gap, #339): the statement declaration-probe's
    /// `compute_type` consumes `c.m<D>` as a (prefixed, parameterized) type,
    /// so `c.m<D>();` parses as a declaration head — the engine sees a
    /// missing type use (`undefined_class`) and no send/type-argument
    /// events. Un-reds when `compute_type` stops at the `.` of an
    /// expression-statement head (`c.` can never open a declaration).
    #[test]
    fn member_invocation_arity_reports_element_method() {
        let report = report("class D {} class C { void m<T, U>() {} } void f(C c) { c.m<D>(); }");
        assert_eq!(
            code_names(&report),
            ["wrong_number_of_type_arguments_element"]
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The method 'm' is declared with 2 type parameters, but 1 type arguments are given."
        );
    }

    /// A getter invoked or applied with type arguments stays silent: the
    /// oracle names the getter kind word (or the function type display),
    /// neither of which M1 models — under-report, never a wrong code.
    ///
    /// Hand-trace: `g`'s top-level declaration carries no formals (no
    /// parameter list), so the `formals.is_some()` filter suppresses both
    /// the send path and the application path.
    #[test]
    fn getter_type_arguments_suppress() {
        let report = report("class A {} class C {} C get g => C(); void f() { g<A, A>(); g<A>; }");
        assert!(report.diagnostics.is_empty(), "{:?}", code_names(&report));
    }

    /// An applied (non-invoked) top-level function tear-off reports
    /// `_element` with the `function` kind word
    /// (`function_reference_resolver.dart` →
    /// `InvocationTargetExecutableElement`).
    ///
    /// Hand-trace: `g<A, A>;` never opens an argument group; the application
    /// event consumes the deferred entry against `g`'s declared count.
    ///
    /// RED (parser gap, #339): in expression-statement context the
    /// declaration probe consumes `g<A, A>` as a parameterized type (the
    /// `;` follower reads as a declaration end), so the statement parses as
    /// a nameless declaration head and the engine receives no identifier or
    /// application events (observed output: `[]`). Un-reds when the
    /// `<...>`-group scan in statement position falls back to the expression
    /// grammar once no name follows (official computeType posture).
    #[test]
    fn function_reference_arity_reports_element_function() {
        let report = report("class A {} void g<T>() {} void f() { g<A, A>; }");
        assert_eq!(
            code_names(&report),
            ["wrong_number_of_type_arguments_element"]
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The function 'g' is declared with 1 type parameters, but 2 type arguments are given."
        );
    }

    /// An implicit call reference (`c<int>;` on a callable class) reports
    /// `_element` naming `call` with the `method` kind word (the oracle's
    /// `_getCallMethod` resolves the `call` method element).
    ///
    /// Hand-trace: `c`'s row snapshots its declared type `C`; the
    /// application event walks `C.call`, finds a method with 1 type
    /// parameter, and checks it against the given 2.
    ///
    /// RED (parser gap, #339): `c<A, A>;` in statement position has no
    /// declaration shape, but the `<` group is not rescanned as type
    /// arguments either — the statement parses as relational operators
    /// (`c < A`, `,`, `A >` …), producing `undefined_class` ×2,
    /// `undefined_operator`, `undefined_identifier`. Same family as
    /// `function_reference_arity` (the `<A, A>` complex-group scan in
    /// expression context). Un-reds with the same parser fix.
    #[test]
    fn implicit_call_reference_reports_element_call() {
        let report = report("class C { T call<T>(T x) => x; } void f(C c) { c<A, A>; }");
        assert_eq!(
            code_names(&report),
            ["wrong_number_of_type_arguments_element"]
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The method 'call' is declared with 1 type parameters, but 2 type arguments are given."
        );
    }

    /// An explicit extension override with mismatched type arguments reports
    /// `_extension` (`extension_member_resolver.dart:380`).
    ///
    /// Hand-trace: `Ex<A>(a)` parses as an implicit creation; the group's
    /// `ctor_pos` branch resolves `Ex` as an extension container and checks
    /// its declared count (0) against 1 immediately. The trailing `(a)` runs
    /// under `ctor_pos` (callee `None`) and stays silent.
    ///
    /// RED (parser gap, #339): the statement declaration-probe's
    /// `compute_type` consumes `Ex<A>` as a parameterized type and the `(`
    /// follower reads as an invocation-headed declaration, so the statement
    /// parses as a declaration head — no identifier, type-argument, or
    /// creation events reach the engine (observed output: `[]`). Un-reds
    /// with the same `Type<args>(` expression-statement veto as
    /// `creation_arity`.
    #[test]
    fn extension_override_arity_reports_extension_code() {
        let report = report("class A {} extension Ex on A {} void f(A a) { Ex<A>(a); }");
        assert_eq!(
            code_names(&report),
            ["wrong_number_of_type_arguments_extension"]
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The extension 'Ex' is declared with 0 type parameters, but 1 type arguments were given."
        );
    }

    // —— batch 2 (#121): static-nullability four ——————————————————————————

    /// `null` into a non-nullable target reports `invalid_assignment`
    /// (`assignment_expression_resolver.dart:795`).
    ///
    /// Hand-trace: `int x` arms the group type and binds `x` =
    /// `int` (non-nullable); `x = null` finds the target row before `=`,
    /// exactly one right-hand row (`null`, typed `Null`), and
    /// `Ty::assignable_from(Null → int)` = `Some(false)` (Left Null,
    /// subtype.dart:349) → the code. The `int` class models the core type in
    /// the no-core harness.
    #[test]
    fn null_assignment_reports_invalid_assignment() {
        let report = report("class int {} void f() { int x; x = null; }");
        assert_eq!(code_names(&report), ["invalid_assignment"]);
        assert_eq!(
            report.diagnostics[0].message,
            "A value of type 'Null' can't be assigned to a variable of type 'int'."
        );
        assert_eq!(report.diagnostics[0].severity, Severity::Error);
    }

    /// A declaration initializer runs the same decision
    /// (`variable_declaration_resolver.dart:140`); a nullable target accepts
    /// `null` silently.
    ///
    /// Hand-trace: `int x = null` checks the single-row initializer against
    /// the group type → `invalid_assignment`; `int? y = null` is Right Top
    /// (subtype.dart:263) → silent.
    ///
    /// RED (parser gap, #339): a nullable declared type followed by an
    /// initializer (`int? y = null;`) loses the declaration: the `?`-type
    /// plus `=`-initializer statement shape parses as the expression
    /// `int ? y …`, `y` never binds, and its name surfaces as
    /// `undefined_identifier` (probe: `int? y = 1;` alone reports it too;
    /// `int? y;` without an initializer parses fine). Un-reds when the
    /// declaration probe accepts `Type? name =` as a variable declaration.
    #[test]
    fn declaration_initializer_null_reports_invalid_assignment() {
        let report = report("class int {} void f() { int x = null; int? y = null; }");
        assert_eq!(code_names(&report), ["invalid_assignment"]);
        assert_eq!(
            report.diagnostics[0].message,
            "A value of type 'Null' can't be assigned to a variable of type 'int'."
        );
    }

    /// A nullable right-hand side into a non-nullable target is decidable
    /// from nullability alone (Left Nullable, subtype.dart:389-392).
    ///
    /// Hand-trace: `y`'s row snapshots its declared type `int?`; the check
    /// needs no name lattice → `Some(false)`; the display texts are `int?`
    /// and `int`.
    #[test]
    fn nullable_assignment_reports_invalid_assignment() {
        let report = report("class int {} void f() { int? y; int x; x = y; }");
        assert_eq!(code_names(&report), ["invalid_assignment"]);
        assert_eq!(
            report.diagnostics[0].message,
            "A value of type 'int?' can't be assigned to a variable of type 'int'."
        );
    }

    /// `null` (and nullable values) into a known non-nullable parameter
    /// reports `argument_type_not_assignable` (helpers.dart:96-142). The
    /// message keeps the template's trailing record appendix (empty at M1).
    ///
    /// Hand-trace: `g(null)` — the argument list collects one terminal row
    /// (typed `Null`); the callee's declared positional formal is `int`
    /// (non-nullable) → `Some(false)`. `g(y)` with `int? y` reads `y`'s
    /// snapshot (`int?`).
    #[test]
    fn null_argument_reports_argument_type_not_assignable() {
        let report = report("class int {} void g(int p) {} void f() { int? y; g(null); g(y); }");
        assert_eq!(
            code_names(&report),
            [
                "argument_type_not_assignable",
                "argument_type_not_assignable"
            ]
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The argument type 'Null' can't be assigned to the parameter type 'int'. "
        );
        assert_eq!(
            report.diagnostics[1].message,
            "The argument type 'int?' can't be assigned to the parameter type 'int'. "
        );
    }

    /// `return null` in a non-nullable function reports
    /// `return_of_invalid_type` with the per-host message
    /// (`rtv.dart:209-249` sync matrix).
    ///
    /// Hand-trace: `f`'s return type latches at the name identifier (the
    /// return type's `handle_type` precedes it); `return null`'s single row
    /// fails Left Null → the function-host message. `m`'s method shape picks
    /// the method-host word.
    #[test]
    fn null_return_reports_return_of_invalid_type() {
        let report =
            report("class int {} class C { int m() { return null; } } int f() { return null; }");
        assert_eq!(
            code_names(&report),
            ["return_of_invalid_type", "return_of_invalid_type"]
        );
        assert_eq!(
            report.diagnostics[0].message,
            "A value of type 'Null' can't be returned from the method 'm' because it has a return type of 'int'."
        );
        assert_eq!(
            report.diagnostics[1].message,
            "A value of type 'Null' can't be returned from the function 'f' because it has a return type of 'int'."
        );
    }

    /// The void-host arm of the return matrix: `return <non-void>` in a
    /// `void` function reports; `return null` in a void function is
    /// oracle-legal.
    ///
    /// Hand-trace: host `void` accepts `void`/`dynamic`/`Null` — `return x`
    /// with `x: int` is none of them → report; `return null` matches the
    /// `Null` accept set → silent.
    #[test]
    fn void_return_matrix() {
        let report = report("class int {} void f(int x) { return x; } void g() { return null; }");
        assert_eq!(code_names(&report), ["return_of_invalid_type"]);
        assert_eq!(
            report.diagnostics[0].message,
            "A value of type 'int' can't be returned from the function 'f' because it has a return type of 'void'."
        );
    }

    /// `?.` on a strictly non-nullable receiver reports the
    /// `invalid_null_aware_operator` `STATIC_WARNING`; a nullable receiver
    /// stays silent (`type_system.dart:1921-` `isStrictlyNonNullable`).
    ///
    /// Hand-trace: `c`'s declared type is `C` (no `?`) → strictly
    /// non-nullable → the warning at the merge; `c2`'s type is `C?` →
    /// suppress. `m` is declared, so the member walk resolves both accesses.
    #[test]
    fn null_aware_on_non_nullable_reports_static_warning() {
        let report = report("class C { void m() {} } void f(C c, C? c2) { c?.m(); c2?.m(); }");
        assert_eq!(code_names(&report), ["invalid_null_aware_operator"]);
        let d = &report.diagnostics[0];
        assert_eq!(
            d.message,
            "The receiver can't be null, so the null-aware operator '?.' is unnecessary."
        );
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(
            d.code.dtype,
            dartq_diagnostics::DiagnosticType::StaticWarning
        );
    }

    /// Declaration-arity violations fold from the collector's log ahead of
    /// the row diagnostics, in capture order: the operator count table
    /// (`error_verifier.dart:5688-5740`) and the setter shape rule
    /// (`ast_builder.dart:6283-6291`).
    ///
    /// Hand-trace: `operator +(a, b)` → expected 1, found 2; `set s(a, b)` →
    /// not one required-positional → the syntactic-tier setter text; the
    /// correct `operator [](i)` and `set ok(v)` stay silent. The `int`
    /// annotations model `dart:core` (the harness has no core library —
    /// `report()`'s contract — so core names join as concrete declarations,
    /// as in `null_assignment`).
    #[test]
    fn declaration_arity_folds_operator_and_setter() {
        let report = report(
            "class int {} class C { int operator +(int a, int b) => 1; set s(int x, int y) {} int operator [](int i) => 1; set ok(int v) {} }",
        );
        assert_eq!(
            code_names(&report),
            [
                "wrong_number_of_parameters_for_operator",
                "wrong_number_of_parameters_for_setter",
            ]
        );
        assert_eq!(
            report.diagnostics[0].message,
            "Operator '+' should declare exactly 1 parameters, but 2 found."
        );
        assert_eq!(
            report.diagnostics[1].message,
            "Setters must declare exactly one required positional parameter."
        );
        assert_eq!(report.diagnostics[1].severity, Severity::Error);
    }

    /// The minus operator keeps its own template under the same analyzerCode
    /// (0 or 1 parameters — unary and binary minus coexist).
    ///
    /// Hand-trace: `operator -(a, b)` declares 2 parameters; the table's
    /// fall-through arm (`error_verifier.dart:5731-5740`) fires the minus
    /// variant with the found count only. `int` models `dart:core` (the
    /// `report()` harness contract).
    #[test]
    fn minus_operator_arity_uses_the_minus_template() {
        let report = report("class int {} class C { C operator -(int a, int b) => C(); }");
        assert_eq!(
            code_names(&report),
            ["wrong_number_of_parameters_for_operator"]
        );
        assert_eq!(
            report.diagnostics[0].message,
            "Operator '-' should declare 0 or 1 parameter, but 2 found."
        );
    }

    /// The arity checks suppress on unresolved heads: a second code at the
    /// type-argument group would contaminate the diff (rule 0c — invalid
    /// types never cascade).
    ///
    /// Hand-trace: `Missing<A>()` parses as a bare invocation, not a
    /// creation — no `new`/`const` prefix, so the official parser runs it
    /// through the send path and the resolver reports the identifier miss
    /// (`undefined_function`, the static-context invocation miss). The
    /// group's arity check finds no resolvable declaration and defers past
    /// it → exactly one code.
    #[test]
    fn unresolved_creation_head_suppresses_arity() {
        let report = report("class A {} void f() { Missing<A>(); }");
        assert_eq!(code_names(&report), ["undefined_function"]);
    }

    // —— batch 2 (#197): the member family ————————————————————————————

    /// Instance member misses report `undefined_method` /
    /// `undefined_getter` / `undefined_setter` by access intent
    /// (`InstanceLookupDomain`); a write survives on any same-named
    /// declaration (getter-only: the oracle's assignment-to-final family)
    /// or on a setter spelling; a setter-only name still reads as a
    /// getter miss.
    ///
    /// Hand-trace: the harness has no core library, so `int` — undeclared
    /// here — is a missing type reference at each member declaration
    /// (`undefined_class` ×2, the oracle's annotation miss); `c.h` walks
    /// `C` (no `h`, no superclass) → clean miss → `undefined_getter`;
    /// `c.g` finds the getter (silent); `c.s = 1` misses `s` and `s=` →
    /// `undefined_setter`; `c.g = 1` finds the getter `g` (write survives,
    /// unfrozen assignment-to-final); `c.p` on the setter-only `p=` misses
    /// the read → `undefined_getter`.
    #[test]
    fn instance_member_family_reports_by_intent() {
        let report = report(
            "class C { int get g => 1; set p(int v) {} } \
             void f(C c) { c.h; c.s = 1; c.g = 2; c.p; }",
        );
        assert_eq!(
            code_names(&report),
            [
                "undefined_class",
                "undefined_class",
                "undefined_getter",
                "undefined_setter",
                "undefined_getter",
            ],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[2].message,
            "The getter 'h' isn't defined for the type 'C'."
        );
        assert_eq!(
            report.diagnostics[3].message,
            "The setter 's' isn't defined for the type 'C'."
        );
        assert_eq!(
            report.diagnostics[2].range.offset,
            u32::try_from("class C { int get g => 1; set p(int v) {} } void f(C c) { c.".len())
                .unwrap()
        );
    }

    /// A declared method invoked through an instance receiver reports
    /// `undefined_method` (`ReadSyntax.invocation`).
    ///
    /// Hand-trace: `c.m()`'s send marks the member row invoke; the walk on
    /// `C` clean-misses `m` → the method code, not the getter code.
    #[test]
    fn instance_invocation_miss_reports_undefined_method() {
        let report = report("class C {} void f(C c) { c.m(); }");
        assert_eq!(code_names(&report), ["undefined_method"]);
        assert_eq!(
            report.diagnostics[0].message,
            "The method 'm' isn't defined for the type 'C'."
        );
    }

    /// The static face (`StaticLookupDomain`): interface shapes report the
    /// `undefined_method`/`_getter`/`_setter` trio (the type-literal
    /// invocation row shares the registry name and message), enum reads
    /// report `undefined_enum_constant`, and enum writes report
    /// `undefined_setter`.
    ///
    /// Hand-trace: every `X.member` merges to a `Container` receiver whose
    /// face is the declaration's own member map; `E.x` reads fall to the
    /// enum-constant text, `E.z = 1` writes to the uniform setter arm.
    #[test]
    fn static_face_reports_interface_trio_and_enum_constant() {
        let report = report(
            "class C {} enum E { a } \
             void f() { C.missing(); C.g; C.s = 1; E.a; E.x; E.y(); E.z = 1; }",
        );
        assert_eq!(
            code_names(&report),
            [
                "undefined_method",
                "undefined_getter",
                "undefined_setter",
                "undefined_enum_constant",
                "undefined_method",
                "undefined_setter",
            ],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The method 'missing' isn't defined for the type 'C'."
        );
        assert_eq!(
            report.diagnostics[3].message,
            "There's no constant named 'x' in 'E'."
        );
    }

    /// The extension face reports its dedicated trio
    /// (`StaticLookupDomain._extensionReadFailure` / the setter arm).
    ///
    /// Hand-trace: `X` resolves as an `Extension` container; the member map
    /// misses each name → the extension texts with the extension name.
    #[test]
    fn extension_static_face_reports_extension_trio() {
        let report = report(
            "class int {} extension X on int {} \
             void f() { X.g; X.m(); X.s = 1; }",
        );
        assert_eq!(
            code_names(&report),
            [
                "undefined_extension_getter",
                "undefined_extension_method",
                "undefined_extension_setter",
            ],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The getter 'g' isn't defined for the extension 'X'."
        );
    }

    /// `super.member` reports `undefined_super_member` with the
    /// per-shape message against the enclosing container (`SuperLookupDomain`);
    /// members found through the superclass edge stay silent.
    ///
    /// Hand-trace: the harness has no core library, so B's `int`-typed
    /// members carry two `undefined_class` rows; `super.g/m/s` resolve in
    /// `B` (the superclass edge reaches it) — silent; the `q*` misses walk
    /// B cleanly → getter/method/setter texts.
    #[test]
    fn super_member_family_reports_by_shape() {
        let report = report(
            "class B { int get g => 1; void m() {} set s(int v) {} } \
             class C extends B { void f() { super.g; super.m(); super.s = 1; super.q; super.q2(); super.q3 = 2; } }",
        );
        assert_eq!(
            code_names(&report),
            [
                "undefined_class",
                "undefined_class",
                "undefined_super_member",
                "undefined_super_member",
                "undefined_super_member",
            ],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[2].message,
            "The getter 'q' isn't defined in a superclass of 'C'."
        );
        assert_eq!(
            report.diagnostics[3].message,
            "The method 'q2' isn't defined in a superclass of 'C'."
        );
        assert_eq!(
            report.diagnostics[4].message,
            "The setter 'q3' isn't defined in a superclass of 'C'."
        );
    }

    /// Prefixed property reads and writes report `undefined_prefixed_name`
    /// (`PrefixedLookupDomain`); the resolved name stays silent.
    ///
    /// Hand-trace: `p.real` hits the prefix table (silent); `p.gone` and the
    /// `p.gone2 = 1` write miss it and pass the guard → the two-part text.
    #[test]
    fn prefixed_property_miss_reports_undefined_prefixed_name() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file_a = files.intern(Path::new("a.dart"));
        let file_m = files.intern(Path::new("main.dart"));
        let source_a = "void real() {}";
        let source_m = "import 'a.dart' as p; void f() { p.real; p.gone; p.gone2 = 1; }";
        let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
            LanguageVersion::new(3, 15),
        ));
        let scanned_a = dartq_scanner::scan(
            source_a,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file_a,
        );
        let scanned_m = dartq_scanner::scan(
            source_m,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file_m,
        );
        let elements_a = collect(source_a, &scanned_a.tokens, features, &strings);
        let elements_m = collect(source_m, &scanned_m.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib_a = builder.intern("package:t/a.dart");
        let lib_m = builder.intern("package:t/main.dart");
        let mut index = ElementIndex::default();
        index.insert(file_a, elements_a.clone());
        index.insert(file_m, elements_m.clone());
        for (name, _) in elements_a.declarations() {
            builder.declare(lib_a, name, file_a);
        }
        for (name, decl) in elements_m.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib_m, name, file_m);
        }
        builder.import(
            lib_m,
            ImportEdge {
                target: Some(lib_a),
                prefix: Some(strings.intern("p")),
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let report = diagnose(
            source_m,
            &scanned_m.tokens,
            features,
            &strings,
            file_m,
            lib_m,
            &graph,
            &namespaces,
            &index,
        );
        assert_eq!(
            code_names(&report),
            ["undefined_prefixed_name", "undefined_prefixed_name"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The name 'gone' is being referenced through the prefix 'p', but it isn't defined \
             in any of the libraries imported using that prefix."
        );
    }

    /// A bare invocation miss inside a class or enum instance member names
    /// `this`'s type (`UnqualifiedLookupDomain` invocation arm); static and
    /// top-level bodies keep `undefined_function`.
    ///
    /// Hand-trace: `missing()` in `C.f` — the container walk clean-misses
    /// (no `this` member, no extension pool, no library scope) and the
    /// instance context maps the row to `undefined_method(C)`; `C.g`'s
    /// static body and the enum's instance body pick their own domains.
    #[test]
    fn bare_invoke_in_instance_reports_undefined_method() {
        let report = report(
            "class C { void f() { missing(); } static void g() { missing2(); } } \
             enum E { a; void f() { missing3(); } }",
        );
        assert_eq!(
            code_names(&report),
            ["undefined_method", "undefined_function", "undefined_method"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The method 'missing' isn't defined for the type 'C'."
        );
        assert_eq!(
            report.diagnostics[1].message,
            "The function 'missing2' isn't defined."
        );
        assert_eq!(
            report.diagnostics[2].message,
            "The method 'missing3' isn't defined for the type 'E'."
        );
    }

    /// A stale type-parameter reference follows the live invocation context:
    /// invoked inside a class instance member it reports `undefined_method`
    /// naming the enclosing type (the `UnqualifiedLookupDomain` arm), not
    /// the context-free `undefined_function`.
    ///
    /// Hand-trace: `T` is out of scope in `D.m` (the engine's type-parameter
    /// stack truncated when `C`'s frame closed), but the scoped lookup still
    /// resolves the per-unit tag to `C`'s `TypeParam` declaration;
    /// `bare_info_of` snapshots the live frames (`this` of `D`) instead of a
    /// context-free missing row → `undefined_method(D)`.
    #[test]
    fn stale_type_param_invocation_follows_live_frame() {
        let report = report("class C<T> {} class D { void m() { T(); } }");
        assert_eq!(
            code_names(&report),
            ["undefined_method"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The method 'T' isn't defined for the type 'D'."
        );
    }

    /// Mixin `on` constraints and extension `on` types are invisible to the
    /// model: bare names and `this.member` accesses inside their instance
    /// bodies record only — the miss may be a constraint member.
    ///
    /// Hand-trace: `real`/`real2` resolve against `C` in the oracle (the
    /// `on` face), so emitting would false-positive; the walk on `M`/`X`
    /// clean-misses but the container-kind gate suppresses.
    #[test]
    fn mixin_and_extension_bodies_suppress_member_misses() {
        let report = report(
            "class C { void real() {} } \
             mixin M on C { void f() { real(); this.real2(); } } \
             extension X on C { void g() { real3(); this.real4(); } }",
        );
        assert!(report.diagnostics.is_empty(), "{:?}", code_names(&report));
    }

    // —— batch 2 (#197): undefined_operator (#120 residual 2) ————————————
    // (index tests below double as residual 4's subscript-intent evidence)

    /// A binary operator on a declared-typed receiver reports
    /// `undefined_operator` at the operator token
    /// (`binary_expression_resolver.dart`); a declared operator and
    /// `this`-receiver hits stay silent; `super.operator` reports the super
    /// operator shape.
    ///
    /// Hand-trace: `c + d` — `c`'s row carries `Operator('+', token)`;
    /// `C` declares `+` (silent). `d + d2` — `D` lacks `+` → the operator
    /// token anchors `undefined_operator('+', D)`. `this - o` walks `E`'s
    /// face; `super * this` walks `B` and reports the super text.
    #[test]
    fn binary_operator_misses_report_undefined_operator() {
        let report = report(
            "class C { C operator +(C o) => this; } class D {} \
             void f(C c, D d, D d2) { c + d; d + d2; } \
             class E { void g(E o) { this - o; } } \
             class B {} class S extends B { void h() { super * this; } }",
        );
        assert_eq!(
            code_names(&report),
            [
                "undefined_operator",
                "undefined_operator",
                "undefined_super_member",
            ],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The operator '+' isn't defined for the type 'D'."
        );
        assert_eq!(
            report.diagnostics[1].message,
            "The operator '-' isn't defined for the type 'E'."
        );
        assert_eq!(
            report.diagnostics[2].message,
            "The operator '*' isn't defined in a superclass of 'S'."
        );
        assert_eq!(
            report.diagnostics[0].range.offset,
            u32::try_from(
                "class C { C operator +(C o) => this; } class D {} void f(C c, D d, D d2) { c + d; d "
                    .len()
            )
            .unwrap()
        );
    }

    /// A binary operator on a member result applies to the member's
    /// declared type (`c.j + r` checks `+` on the field's type); a missing
    /// member reports the getter read instead — the operator's invalid
    /// type never cascades.
    ///
    /// Hand-trace: `j`'s row carries `Operator('+')`; the walk finds the
    /// field `j: R` → the operator check runs on `R` → `undefined_operator`.
    /// `c.k` has no declaration in `C` → clean miss → `undefined_getter`
    /// only (a declared field would read cleanly and lend its own type to
    /// the operator, so `k` must stay undeclared for the no-cascade shape).
    #[test]
    fn member_result_operator_checks_the_result_type() {
        let report = report(
            "class R {} class C { R j; } class R2 {} \
             void f(C c, R r, R2 r2) { c.j + r; c.k + r2; }",
        );
        assert_eq!(
            code_names(&report),
            ["undefined_operator", "undefined_getter"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The operator '+' isn't defined for the type 'R'."
        );
    }

    // —— batch 2 (#197): index intents (#120 residual 4) —————————————————

    /// Index reads and writes check `[]` / `[]=` independently, anchored on
    /// the bracket pair (`property_element_resolver.dart`); compound writes
    /// check both; declared operators stay silent — and the index argument
    /// never takes the assignment's write intent.
    ///
    /// Hand-trace: `d[0]` folds `d`'s row to `Operator('[]', brackets)` →
    /// the read spelling misses on `D`. `d[0] = 1` records the write form →
    /// only `[]=` is checked. `d[0] += 1` checks both. The declared pair on
    /// `C` resolves both forms of `c[0] = c[1]` silently, and `c[0] = null`
    /// does NOT read `c`'s own type as the destination.
    #[test]
    fn index_read_write_and_compound_check_spellings() {
        let index_report = report(
            "class int {} class D {} \
             void f(D d, int i) { d[i]; d[i] = 1; d[i] += 1; }",
        );
        assert_eq!(
            code_names(&index_report),
            [
                "undefined_operator",
                "undefined_operator",
                "undefined_operator",
                "undefined_operator",
            ],
            "{:?}",
            code_names(&index_report)
        );
        assert_eq!(
            index_report.diagnostics[0].message,
            "The operator '[]' isn't defined for the type 'D'."
        );
        assert_eq!(
            index_report.diagnostics[1].message,
            "The operator '[]=' isn't defined for the type 'D'."
        );
        assert_eq!(
            index_report.diagnostics[2].message,
            "The operator '[]' isn't defined for the type 'D'."
        );
        assert_eq!(
            index_report.diagnostics[3].message,
            "The operator '[]=' isn't defined for the type 'D'."
        );
        let defined_report = report(
            "class int {} class C { C operator [](int i) => this; C operator []=(int i, C v) {} } \
             void f(C c, int i) { c[i] = c[i]; c[i] = null; }",
        );
        assert!(
            defined_report.diagnostics.is_empty(),
            "{:?}",
            code_names(&defined_report)
        );
    }

    /// The `c.j[i] = v` subscript intent (#120 residual 4): the write goes
    /// through `[]=` on the member's result type; the index argument keeps
    /// its read; no setter code touches `j`; a missing `j` reports the
    /// getter read and suppresses the operator.
    ///
    /// Hand-trace: the fold at `[` marks `j`'s member row
    /// `Operator('[]', brackets)` and consumes `i`'s row; the assignment
    /// records the `[]=` write form; the walk finds getter `j: V` → the
    /// operator check runs on `V` → one `undefined_operator('[]=', V)`. In
    /// the second statement `k` clean-misses → `undefined_getter(k)`, no
    /// cascade.
    #[test]
    fn subscript_write_targets_the_member_result_type() {
        let report = report(
            "class int {} class V {} class C { V get j => V(); } \
             void f(C c, int i, V v) { c.j[i] = v; c.k[i] = v; }",
        );
        assert_eq!(
            code_names(&report),
            ["undefined_operator", "undefined_getter"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The operator '[]=' isn't defined for the type 'V'."
        );
        assert_eq!(
            report.diagnostics[0].range.offset,
            u32::try_from(
                "class int {} class V {} class C { V get j => V(); } void f(C c, int i, V v) { c.j"
                    .len()
            )
            .unwrap()
        );
    }

    /// A compound member write checks both faces (`c.g += 1` needs the
    /// getter read and the setter write) — the getter error first, matching
    /// the oracle's resolution order.
    ///
    /// Hand-trace: `g`'s row upgrades to `ReadWrite`; the getter walk
    /// clean-misses → `undefined_getter`; the write walk misses `g` and
    /// `g=` → `undefined_setter`.
    #[test]
    fn compound_member_write_reports_both_faces() {
        let report = report("class C {} void f(C c) { c.g += 1; }");
        assert_eq!(
            code_names(&report),
            ["undefined_getter", "undefined_setter"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The getter 'g' isn't defined for the type 'C'."
        );
        assert_eq!(
            report.diagnostics[1].message,
            "The setter 'g' isn't defined for the type 'C'."
        );
    }

    // —— records/patterns/exhaustiveness (M2 face) ————————————————————————

    /// A statement switch over an enum with every constant covered is
    /// exhaustive: no diagnostics.
    ///
    /// Hand-trace: the scrutinee row carries `Color` (formal type); each
    /// `case Color.red:` classifies through the constant-tail walk (the
    /// `.red` member row over the bare `Color` row of an enum container)
    /// into `EnumValue`; the engine's covering search finds no gap.
    #[test]
    fn enum_switch_all_constants_is_exhaustive() {
        let report = report(
            "enum Color { red, green, blue } void f(Color c) { switch (c) { case Color.red: break; case Color.green: break; case Color.blue: break; } }",
        );
        assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
    }

    /// A missing enum constant reports `non_exhaustive_switch_statement`
    /// at the `switch` keyword, with the oracle's verbatim text and the
    /// first missing constant as the suggested correction pattern.
    ///
    /// Hand-trace: `blue` never covers → `NonExhaustive([Color.blue])`;
    /// the statement is mustBeExhaustive (enums are always exhaustive,
    /// type_system.dart:846-887) and no `default` is present.
    #[test]
    fn enum_switch_missing_constant_reports_correction() {
        let report = report(
            "enum Color { red, green, blue } void f(Color c) { switch (c) { case Color.red: break; case Color.green: break; } }",
        );
        assert_eq!(
            code_names(&report),
            ["non_exhaustive_switch_statement"],
            "{:?}",
            code_names(&report)
        );
        let d = &report.diagnostics[0];
        assert_eq!(
            d.message,
            "The type 'Color' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'Color.blue'."
        );
        // The oracle row's correctionMessage carries the suggested pattern
        // as `{2}` (diagnostic.g.dart:13079).
        assert_eq!(
            d.correction.as_deref(),
            Some("Try adding a default case or cases that match 'Color.blue'.")
        );
        assert_eq!(d.severity, Severity::Error);
        let switch_at =
            u32::try_from("enum Color { red, green, blue } void f(Color c) { ".len()).unwrap();
        assert_eq!(d.range.offset, switch_at);
    }

    /// A `default` case suppresses the non-exhaustive report even when
    /// constants are missing (the oracle's `reportNonExhaustive` gate,
    /// constant_verifier.dart:1122-1140).
    #[test]
    fn default_case_suppresses_non_exhaustive() {
        let report = report(
            "enum Color { red, green, blue } void f(Color c) { switch (c) { case Color.red: break; default: break; } }",
        );
        assert_eq!(report.diagnostics.len(), 0);
    }

    /// A duplicate constant after total coverage is
    /// `unreachable_switch_case` (`STATIC_WARNING`) at the repeated case's
    /// keyword — reported per case before the covering check, matching the
    /// oracle's decision order (constant_verifier.dart:1080-1088).
    #[test]
    fn duplicate_constant_case_is_unreachable() {
        let report = report(
            "enum Color { red, green, blue } void f(Color c) { switch (c) { case Color.red: break; case Color.green: break; case Color.blue: break; case Color.red: break; } }",
        );
        assert_eq!(
            code_names(&report),
            ["unreachable_switch_case"],
            "{:?}",
            code_names(&report)
        );
        let d = &report.diagnostics[0];
        assert_eq!(d.message, "This case is covered by the previous cases.");
        assert_eq!(d.severity, Severity::Warning);
        let repeat_at = u32::try_from(
            "enum Color { red, green, blue } void f(Color c) { switch (c) { case Color.red: break; case Color.green: break; case Color.blue: break; ".len(),
        )
        .unwrap();
        assert_eq!(d.range.offset, repeat_at);
    }

    /// A `when`-guarded case never covers: the guarded case is excluded
    /// from the covering search (`constant_verifier.dart` guarded rows) and
    /// the missing constant still reports.
    #[test]
    fn guarded_case_does_not_cover() {
        let report = report(
            "enum Color { red, green, blue } void f(Color c) { switch (c) { case Color.red: break; case Color.green: break; case Color.blue when true: break; } }",
        );
        assert_eq!(code_names(&report), ["non_exhaustive_switch_statement"]);
        assert_eq!(
            report.diagnostics[0].message,
            "The type 'Color' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'Color.blue'."
        );
    }

    /// Switch expressions are always mustBeExhaustive: a missing constant
    /// reports `non_exhaustive_switch_expression` even though a statement
    /// over the same cases would stay silent only without the seal — and
    /// the same shape over a *statement* is the T2 report, not this code.
    #[test]
    fn switch_expression_requires_all_constants() {
        let report = report(
            "enum Color { red, green, blue } void f(Color c) { var x = switch (c) { Color.red => 1, Color.green => 2 }; }",
        );
        assert_eq!(
            code_names(&report),
            ["non_exhaustive_switch_expression"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The type 'Color' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'Color.blue'."
        );
        // The expression-face correctionMessage (diagnostic.g.dart:13018).
        assert_eq!(
            report.diagnostics[0].correction.as_deref(),
            Some("Try adding a wildcard pattern or cases that match 'Color.blue'.")
        );
    }

    /// A switch expression over a sealed class missing one subclass reports
    /// the subclass name as the witness; covering every declared subclass
    /// is exhaustive.
    ///
    /// Hand-trace: `S` is sealed; `subtypes` walks the unit's containers in
    /// declaration order over superclass edges → `[A, B]`; `case A a:`
    /// leaves `B` uncovered.
    #[test]
    fn sealed_class_switch_reports_missing_subclass() {
        let missing = report(
            "sealed class S {} class A extends S {} class B extends S {} void f(S s) { var x = switch (s) { A a => 1 }; }",
        );
        assert_eq!(
            code_names(&missing),
            ["non_exhaustive_switch_expression"],
            "{:?}",
            code_names(&missing)
        );
        assert_eq!(
            missing.diagnostics[0].message,
            "The type 'S' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'B'."
        );

        let covered = report(
            "sealed class S {} class A extends S {} class B extends S {} void f(S s) { var x = switch (s) { A a => 1, B b => 2 }; }",
        );
        assert_eq!(covered.diagnostics.len(), 0);
    }

    /// A switch expression over a nullable enum needs the `null` case: the
    /// nullable projection splits into `[Color, null]` and the constants
    /// alone leave the null half uncovered.
    #[test]
    fn nullable_enum_switch_needs_null_case() {
        let missing = report(
            "enum Color { red, green, blue } void f(Color? c) { var x = switch (c) { Color.red => 1, Color.green => 2, Color.blue => 3 }; }",
        );
        assert_eq!(code_names(&missing), ["non_exhaustive_switch_expression"]);
        assert_eq!(
            missing.diagnostics[0].message,
            "The type 'Color?' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'null'."
        );

        let with_null = report(
            "enum Color { red, green, blue } void f(Color? c) { var x = switch (c) { Color.red => 1, Color.green => 2, Color.blue => 3, null => 4 }; }",
        );
        assert_eq!(with_null.diagnostics.len(), 0);
    }

    /// A single non-sealed type case covers the whole (non-nullable)
    /// scrutinee: exhaustiveness does not require a seal.
    ///
    /// Hand-trace: `case C v:` is a type-free-variable frame whose context
    /// substitutes to `Named(C, false)` — the full space of the scrutinee.
    #[test]
    fn single_type_case_covers_non_sealed_scrutinee() {
        let report = report("class C {} void f(C c) { var x = switch (c) { C v => 1 }; }");
        assert_eq!(report.diagnostics.len(), 0);
    }

    /// A cyclic sealed hierarchy terminates: the covering search walks the
    /// subclass closure through the file-scoped facts, which cannot reject
    /// `sealed S extends S` (self-edge dropped at the source) or a mutual
    /// `A`/`B` cycle (dropped by the walk's visited set) — without the
    /// guard the FIFO expansion re-enqueues the same type forever.
    ///
    /// Hand-trace: the `1` constant is the unknown space (never a
    /// must-match), so the sealed walk must expand its subtypes — the
    /// exact shape the cycle breaks. Both verdicts are exhaustive (no
    /// witness over a sealed closure), so nothing emits.
    #[test]
    fn cyclic_sealed_hierarchy_terminates_without_false_positive() {
        let self_cycle =
            report("sealed class S extends S {} void f(S s) { var x = switch (s) { 1 => 2 }; }");
        assert_eq!(
            self_cycle.diagnostics.len(),
            0,
            "{:?}",
            code_names(&self_cycle)
        );
        let mutual_cycle = report(
            "sealed class A extends B {} sealed class B extends A {} void f(A a) { var x = switch (a) { 1 => 2 }; }",
        );
        assert_eq!(
            mutual_cycle.diagnostics.len(),
            0,
            "{:?}",
            code_names(&mutual_cycle)
        );
    }

    /// An unresolvable scrutinee skips the whole exhaustiveness check (the
    /// oracle's `InvalidType` gate at `constant_verifier.dart:1035` and
    /// `:1095`): a switch expression over a call whose type never resolves
    /// emits nothing — mapping it to the nullable top would drop every row
    /// and false-positive the covering gap.
    #[test]
    fn unknown_scrutinee_switch_expression_emits_nothing() {
        let report = report("void g() {} void f() { var x = switch (g()) { _ => 0 }; }");
        assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
    }

    /// A switch expression over a name declared outside the file emits no
    /// covering gap: the file-scoped world cannot decide the name's
    /// sealedness, so the expression face suppresses (the cross-file-sealed
    /// interim seam) instead of false-positiving off an opaque non-sealed
    /// reading.
    ///
    /// Hand-trace: `S`/`A`/`B` have no in-file containers, so pre-fix the
    /// scrutinee read as non-sealed with rows that never must-match it —
    /// a witness over any `S` value, emitted as
    /// `non_exhaustive_switch_expression`. The one remaining `undefined_class`
    /// is the unresolved parameter type itself (oracle-matching, see
    /// `type_positions_report_undefined_class`); the object patterns
    /// `A()`/`B()` stay silent.
    #[test]
    fn cross_file_sealed_scrutinee_expression_emits_no_covering_gap() {
        let report = report("void f(S s) { var x = switch (s) { A() => 1, B() => 2 }; }");
        assert_eq!(
            code_names(&report),
            ["undefined_class"],
            "{:?}",
            code_names(&report)
        );
    }

    /// A switch expression's duplicate case is `unreachable_switch_case`
    /// anchored at the repeated case's `=>` (the expression-case anchor,
    /// constant_verifier.dart:1080-1088).
    #[test]
    fn switch_expression_duplicate_case_is_unreachable_at_arrow() {
        let report = report(
            "enum Color { red, green, blue } void f(Color c) { var x = switch (c) { Color.red => 1, Color.red => 2, Color.green => 3, Color.blue => 4 }; }",
        );
        assert_eq!(code_names(&report), ["unreachable_switch_case"]);
        // The repeated case's `=>` (the expression-case anchor).
        let arrow_at = u32::try_from(
            "enum Color { red, green, blue } void f(Color c) { var x = switch (c) { Color.red => 1, Color.red ".len(),
        )
        .unwrap();
        assert_eq!(report.diagnostics[0].range.offset, arrow_at);
    }

    /// A typed variable pattern binds its local for the body: the bound
    /// name reads cleanly, and the case narrows to the pattern's type.
    ///
    /// Hand-trace: `handle_declared_variable_pattern` adds `other` to the
    /// flat local frame with the armed `Color` type — `other;` resolves
    /// through locals, so no `undefined_identifier`.
    #[test]
    fn typed_pattern_binds_local() {
        let report = report(
            "enum Color { red } void f(Color c) { switch (c) { case Color other: other; break; } }",
        );
        assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
    }

    /// An if-case binds the same way: `if (o case Type x)` declares `x`
    /// for the then-block.
    #[test]
    fn if_case_binds_local() {
        let report =
            report("enum Color { red } void f(Color c) { if (c case Color other) { other; } }");
        assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
    }

    /// A type-free `var` pattern binds without a type and covers its
    /// context (substituted with the scrutinee's type at case close).
    #[test]
    fn untyped_pattern_binds_and_covers() {
        let report = report("class C {} void f(C c) { switch (c) { case var y: y; break; } }");
        assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
    }

    /// Record patterns destructuring by shape: the fields bind as typed
    /// locals and the name level emits no shape diagnostic (record shape
    /// checking is the types-crate face — the documented M2 name-level
    /// blind spot).
    ///
    /// Hand-trace: the record pattern bracket pushes the unknown-space leaf
    /// (never a false must-match); the field patterns bind `a`/`b` through
    /// their declared-variable events, so both reads resolve.
    #[test]
    fn record_pattern_binds_fields_without_shape_diagnostic() {
        let report = report("class C {} void f(C c) { if (c case (C a, C b)) { a; b; } }");
        assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
    }

    // —— case-root capture (the case-sink window) ———————————————————————

    /// An if-case root before a switch never feeds its cases: roots are
    /// captured only across a case header's pattern window, so the stale
    /// unknown from `if (y case 42)` can neither cover a case nor trip the
    /// empty fallback.
    ///
    /// Hand-trace (pre-fix): the stale root sat below the case roots; the
    /// second case's `end_switch_case` drained it and reported a false
    /// `non_exhaustive_switch_statement` over `bool`.
    #[test]
    fn stale_if_case_root_never_feeds_a_case() {
        let report = report(
            "class bool {} void g(bool y, bool x) { if (y case 42) {} switch (x) { case false: break; case true: break; } }",
        );
        assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
    }

    /// An if-case inside one case's body never feeds the next case: each
    /// header captures at its own `end_case_expression`, so the stale root
    /// cannot shift the case/space pairing.
    ///
    /// Hand-trace (pre-fix): `case E.a`'s body if-case pushed above `E.a`'s
    /// root and the LIFO pop handed the *next* case the wrong spaces —
    /// a false `non_exhaustive_switch_statement` (missing `E.b`).
    #[test]
    fn stale_case_body_root_never_feeds_the_next_case() {
        let report = report(
            "class bool {} enum E { a, b } void f(bool y, E e) { switch (e) { case E.a: if (y case 42) {} case E.b: break; } }",
        );
        assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
    }

    /// A nested switch opens after the enclosing case's root was captured:
    /// the `open_switch` debris-clear must not destroy live case spaces.
    #[test]
    fn nested_switch_preserves_the_outer_case_root() {
        let report = report(
            "enum E { a, b } void f(E a, E b) { switch (a) { case E.a: switch (b) { case E.a: break; case E.b: break; } case E.b: break; } }",
        );
        assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
    }

    /// A shared-body case group (`case A: case B:`) captures one case per
    /// header, matching the oracle's per-`case` nodes.
    ///
    /// Hand-trace (pre-fix): one body event drained a single root, so the
    /// group collapsed to its *last* header's space and `E.a` went
    /// uncovered — a false `non_exhaustive_switch_statement`.
    #[test]
    fn shared_body_case_group_captures_each_header() {
        let report =
            report("enum E { a, b } void f(E e) { switch (e) { case E.a: case E.b: break; } }");
        assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
    }

    /// A labeled duplicate case anchors `unreachable_switch_case` at the
    /// `case` keyword itself, not the label (the oracle's
    /// `caseNode.keyword`, constant_verifier.dart:1076-1085).
    #[test]
    fn labeled_case_anchors_unreachable_at_case_keyword() {
        let report = report(
            "enum Color { red, green } void f(Color c) { switch (c) { case Color.red: break; case Color.green: break; l: case Color.red: break; } }",
        );
        assert_eq!(code_names(&report), ["unreachable_switch_case"]);
        let case_at = u32::try_from(
            "enum Color { red, green } void f(Color c) { switch (c) { case Color.red: break; case Color.green: break; l: ".len(),
        )
        .unwrap();
        assert_eq!(report.diagnostics[0].range.offset, case_at);
    }

    // —— restricted nullable projections (the Null|value fold) ———————————

    /// `nullable` over a restricted value keeps the restriction: the
    /// `Null|BoolValue(true)` fold lowers to `true?`, so `case null` after
    /// `case true || null` is unreachable while `case false` stays
    /// reachable and the `false` gap still reports.
    ///
    /// Hand-trace: the fold space covers `{true, null}` only
    /// (`NullableStaticType(_BoolValueStaticType)`, static_type.dart:350);
    /// lowering it to `bool?` would swallow `false` on both questions.
    #[test]
    fn bool_value_nullable_keeps_the_restriction() {
        let report = report(
            "class bool {} void f(bool b) { switch (b) { case true || null: break; case null: break; } }",
        );
        assert_eq!(
            code_names(&report),
            ["unreachable_switch_case", "non_exhaustive_switch_statement"],
            "{:?}",
            code_names(&report)
        );
        // The `false` gap, not a swallowed one.
        assert_eq!(
            report.diagnostics[1].message,
            "The type 'bool' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'false'."
        );
        // `case null` anchors at its own `case` keyword.
        let null_case_at = u32::try_from(
            "class bool {} void f(bool b) { switch (b) { case true || null: break; ".len(),
        )
        .unwrap();
        assert_eq!(report.diagnostics[0].range.offset, null_case_at);
    }

    /// The same precision for enum constants: `case E.a || null` covers
    /// `{E.a, null}` only — the sibling `E.b` stays reachable, and the
    /// nullable-scrutinee statement face emits nothing else.
    ///
    /// Hand-trace (pre-fix): the fold lowered to `E?`, whose split covers
    /// every constant — a false `unreachable_switch_case` on `E.b`.
    #[test]
    fn enum_value_nullable_keeps_the_restriction() {
        let report = report(
            "enum E { a, b } void f(E? e) { switch (e) { case E.a || null: break; case E.b: break; } }",
        );
        assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
    }

    // —— enum/mixin supertype edges toward sealed classes —————————————————

    /// An enum's `implements` and a mixin's `on` edge reach the sealed
    /// splitter: both join the sealed class's closed subtype set, so
    /// covering the enum's constants and the mixin covers everything.
    ///
    /// Hand-trace: `subtypes(S)` walks the unit's containers over the
    /// superclass and interface edges → `[E, M]`; `E` splits over its
    /// constants and `M` is covered by the typed case.
    #[test]
    fn enum_and_mixin_edges_split_the_sealed_class() {
        let covered = report(
            "sealed class S {} enum E implements S { a, b } mixin M on S {} void f(S s) { switch (s) { case E.a: break; case E.b: break; case M m: break; } }",
        );
        assert_eq!(covered.diagnostics.len(), 0, "{:?}", code_names(&covered));

        let missing = report(
            "sealed class S {} enum E implements S { a, b } mixin M on S {} void f(S s) { switch (s) { case E.a: break; case E.b: break; } }",
        );
        assert_eq!(
            code_names(&missing),
            ["non_exhaustive_switch_statement"],
            "{:?}",
            code_names(&missing)
        );
        assert_eq!(
            missing.diagnostics[0].message,
            "The type 'S' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'M'."
        );
    }

    /// A typed wildcard (`M _`) covers its declared type's space: the
    /// canonical sealed-coverage style turns exhaustive.
    ///
    /// Hand-trace (pre-fix): the wildcard dropped its type, each case
    /// collapsed to the context space, and the second case reported a false
    /// `unreachable_switch_case`.
    #[test]
    fn typed_wildcard_covers_its_type() {
        let report = report(
            "sealed class S {} class A extends S {} class B extends S {} void f(S s) { switch (s) { case A _: break; case B _: break; } }",
        );
        assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
    }

    /// A bare `_` still covers the whole context (no leaf of its own).
    #[test]
    fn bare_wildcard_covers_the_context() {
        let report = report("class C {} void f(C c) { var x = switch (c) { _ => 1 }; }");
        assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
    }
}
