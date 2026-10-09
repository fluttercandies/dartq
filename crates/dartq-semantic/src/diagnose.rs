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
//! The static-nullability four decide on declared types only
//! (types-nullability-spec §2–§3): the engine tracks the declared type of
//! locals, formals, fields, top-level variables, and return types (armed by
//! `handle_type`/`handle_void_keyword`, disarmed by `handle_no_type` — every
//! declaration head runs one `parse_type`, so the arm cannot leak), and
//! types expressions minimally: the `null` literal is `Null`, a name with a
//! captured declared type is that type, everything else is `Unknown` — the
//! oracle's `InvalidType`, which suppresses every question it touches. The
//! decidable set is therefore exactly the nullability decisions:
//! `null`/`T?` into a non-nullable target (`invalid_assignment`,
//! `argument_type_not_assignable`, `return_of_invalid_type`), and `?.`/`?[`/
//! `?..` on a strictly non-nullable receiver (`invalid_null_aware_operator`,
//! a `STATIC_WARNING`). Assignment/initializer/argument/return checks apply
//! only to single-row expressions (one pushed row) so compound operands
//! never lend their type to the whole expression.
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
//! `element.dart:7470-7502`: a name referenced through an import whose target
//! did not resolve is suppressed. The mirror reads the graph's import edges
//! (`target: None`) and suppresses through the combinators' `allows`
//! predicate — narrower oracle fidelity needs show-clause enumeration, which
//! lives in `dartq_resolution` (see [`UndefinedGuard`] for the recorded
//! divergence: bare misses under an unfiltered broken import are
//! over-suppressed, an under-report direction). The `_$`-generated-part
//! clause needs the part tree; the parser has no part events yet, so it is
//! deferred with the directive milestone (the clause only ever suppresses).
//! Because `target: None` edges suppress bare misses, an M1 harness models
//! core-library names through concrete declarations or shown broken edges —
//! never a blanket unfiltered core import, or the `undefined_identifier`
//! path goes dead.
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
//! # Deferred frozen rows
//!
//! `undefined_constructor_in_initializer` / `undefined_enum_constructor`
//! (constructors outside the M1 model), `undefined_referenced_parameter`
//! (`@UseResult(unless:)` check, `annotation_verifier.dart:519-541`),
//! `undefined_hidden_name` / `undefined_shown_name` (directives are consumed
//! diet-style and combinators expose no clause enumeration — both arrive with
//! the directive milestone), and `_function` (a function-typed *expression*
//! instantiation — its message displays the function type, a shape M1 cannot
//! render). `wrong_number_of_parameters_for_setter` is emitted here although
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
    ArityViolation, ContainerKind, Formals, MemberEntry, MemberKind, MicroKind, TopDecl, Ty,
};

/// Members of `dart:core` `Object`: defined on every class-shaped receiver
/// the M1 walk can reach (the superclass chain ends at an unindexed root).
const OBJECT_MEMBERS: [&str; 5] = ["==", "hashCode", "toString", "noSuchMethod", "runtimeType"];

/// Synthesized enum members absent from the model; suppressed on enum
/// receivers so the member walk cannot false-positive on them.
const ENUM_SYNTH_MEMBERS: [&str; 3] = ["values", "index", "name"];

/// Builtin type names: always defined, independent of any import.
const BUILTIN_TYPES: [&str; 1] = ["dynamic"];

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

/// The `shouldIgnoreUndefined` mirror (`element.dart:7470-7502`) over the
/// library's import edges: a name referenced through an import whose target
/// did not resolve is suppressed, because the missing target is the reported
/// root cause.
///
/// # Known divergence from the oracle (deliberate, under-report direction)
///
/// The oracle ignores a *bare* name only when it sits in a broken import's
/// `show` list (`shownNames.contains`), and a prefixed name when the broken
/// import has no `show` clause or shows it. `dartq_resolution::Combinators`
/// exposes only the merged `allows` predicate — clause enumeration lives in
/// that crate — so a bare name through ANY broken import matching the
/// (empty) prefix is suppressed here. That over-suppresses exactly one shape:
/// a bare miss in a file that declares an unfiltered broken import (where the
/// oracle reports `uri_does_not_exist` anyway). When resolution exposes
/// show-clause membership, tighten `ignore` to `shownNames.contains` for the
/// bare path; the emit sites need no change.
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
    /// Mirrors the analyzer's loop over broken imports matching the prefix,
    /// within the `allows`-only combinator surface (see the type doc for the
    /// deliberate divergence): a broken edge matching the prefix suppresses
    /// the name when it survives the combinators; a prefixed reference is
    /// suppressed regardless (the oracle ignores everything under a broken
    /// prefix whose `show` list does not exclude it — clause enumeration is
    /// not visible here).
    #[must_use]
    pub fn ignore(&self, prefix: Option<StringId>, name: StringId) -> bool {
        for edge in self.edges {
            if edge.target.is_some() || edge.prefix != prefix {
                continue;
            }
            if edge.combinators.allows(name) {
                return true;
            }
            if prefix.is_some() {
                // Analyzer: everything under a broken prefix is ignored
                // unless a `show` clause exists and excludes the name; clause
                // enumeration is not visible here, so suppress.
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
/// currently being parsed.
struct ArgLevel {
    args: Vec<(Option<StringId>, Ty, SourceRange)>,
    pending: Option<(Option<StringId>, usize)>,
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
        let pending = std::mem::take(&mut self.pending_type_params);
        self.type_params.extend(pending);
        self.frames.push(Frame {
            kind,
            locals: FxHashSet::default(),
            var_types: FxHashMap::default(),
            type_mark,
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

    /// Enters a type parameter's scope: parked until the declaration's
    /// frame opens ([`Engine::push_frame`] drains), whose `type_mark`
    /// truncates it away on pop.
    fn push_type_parameter(&mut self, name: StringId) {
        self.pending_type_params.push(name);
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
            if kind == ContainerKind::Enum && self.is_reserved(name, &self.enum_synth_members) {
                return BareInfo::Plain { ty: Ty::UNKNOWN };
            }
            if self.instance_context() && self.is_reserved(name, &self.object_members) {
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

    /// Whether `name` is in a precomputed reserved-id set.
    fn is_reserved(&self, name: StringId, set: &[StringId]) -> bool {
        set.contains(&name)
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
    /// verdict is the snapshot pass 2 emits from).
    fn push_bare(&mut self, name: StringId, range: SourceRange) {
        let info = self.resolve_bare(name);
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
                if self.is_reserved(name, &self.builtin_types) {
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
        self.arg_levels.push(ArgLevel {
            args: Vec::new(),
            pending: None,
        });
    }

    /// Closes an argument group; its callee awaits the send event. The
    /// collected argument types check against the callee's formals first
    /// (`argument_type_not_assignable`, decided on declared types only).
    fn close_arguments(&mut self) {
        self.flush_argument();
        let levels = self.arg_levels.pop();
        self.check_arguments(levels);
        self.pending_send = self.invokes.pop().flatten();
    }

    /// Starts a new argument in the open argument list.
    fn open_argument(&mut self, name: Option<StringId>) {
        self.flush_argument();
        if let Some(level) = self.arg_levels.last_mut() {
            level.pending = Some((name, self.rows.len()));
        }
    }

    /// Closes the pending argument: exactly one pushed row is a terminal
    /// expression whose type stands for the whole argument; anything else
    /// stays `Unknown` (compound operands must not lend their type).
    fn flush_argument(&mut self) {
        let Some(level) = self.arg_levels.last_mut() else {
            return;
        };
        let Some((name, mark)) = level.pending.take() else {
            return;
        };
        let (ty, range) = match self.rows.get(mark) {
            Some(row) if self.rows.len() == mark + 1 => (row.ty, row.range),
            _ => (
                Ty::UNKNOWN,
                SourceRange {
                    offset: 0,
                    length: 0,
                },
            ),
        };
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
            if formal.ty.assignable_from(*ty) == Some(false) {
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
        if matches!(head.shape, RowShape::Bare(BareInfo::Plain { .. })) {
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
            _ => {}
        }
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
        if self.is_reserved(name, &self.builtin_types) || self.type_params.contains(&name) {
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
        if to.assignable_from(from) == Some(false) {
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
        if to.assignable_from(row.ty) == Some(false) {
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
                _ => to.assignable_from(from),
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

    /// The display text of an M1 type (`int?`, `Null`, `dynamic`).
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
            return;
        }
        let Some(kind) = self.container_kind_of(container) else {
            return; // external receiver type: suppress
        };
        if kind == ContainerKind::Enum && self.is_reserved(row.name, &self.enum_synth_members) {
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
        if row.name == self.intern("==") || self.is_reserved(row.name, &self.object_members) {
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

    fn handle_named_argument(&mut self, _colon: Token) {
        // Fires after the value: the pending named argument closes here.
        self.engine.borrow_mut().flush_argument();
    }

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

    fn handle_enum_element(&mut self, _begin: Token, _augment: Option<Token>) {
        self.engine.borrow_mut().enter_enum_element();
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
        self.engine.borrow_mut().push_value_row(token);
    }

    fn handle_literal_int_with_separators(&mut self, token: Token) {
        self.engine.borrow_mut().push_value_row(token);
    }

    fn handle_literal_double(&mut self, token: Token) {
        self.engine.borrow_mut().push_value_row(token);
    }

    fn handle_literal_double_with_separators(&mut self, token: Token) {
        self.engine.borrow_mut().push_value_row(token);
    }

    fn handle_literal_bool(&mut self, token: Token) {
        self.engine.borrow_mut().push_value_row(token);
    }

    fn handle_literal_null(&mut self, token: Token) {
        self.engine.borrow_mut().push_null_row(token);
    }

    fn end_literal_string(&mut self, _interpolation_count: usize, end: Token) {
        self.engine.borrow_mut().push_value_row(end);
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
    /// them through concrete declarations or shown broken edges (a blanket
    /// unfiltered `dart:core` edge would suppress every bare miss — see the
    /// undefined-guard module docs).
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
    /// `allows(gone)` holds — the guard fires before the emit.
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
    /// correct `operator [](i)` and `set ok(v)` stay silent.
    #[test]
    fn declaration_arity_folds_operator_and_setter() {
        let report = report(
            "class C { int operator +(int a, int b) => 1; set s(int x, int y) {} int operator [](int i) => 1; set ok(int v) {} }",
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
    /// variant with the found count only.
    #[test]
    fn minus_operator_arity_uses_the_minus_template() {
        let report = report("class C { C operator -(int a, int b) => C(); }");
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
                "class C { C operator +(C o) => this; } class D {} void f(C c, D d, D d2) { c "
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
    /// `c.k + r` clean-misses `k` → `undefined_getter` only.
    #[test]
    fn member_result_operator_checks_the_result_type() {
        let report = report(
            "class R {} class C { R j; R2 k; } class R2 {} \
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
}
