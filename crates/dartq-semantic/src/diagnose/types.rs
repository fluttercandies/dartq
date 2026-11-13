use super::*;

/// The dart:core top-level annotation names (`sdk/lib/core/annotations.dart`:
/// `Deprecated` :63, `deprecated` :212, `override` :262, `pragma` :297) —
/// dart:core is implicitly imported into every library, so an annotation of
/// one of them is defined even where the SDK summary faces carry no
/// top-level constants. The oracle resolves `@override` inside its own
/// hosted sources (meta 1.19.0 `meta_meta.dart:202`) and reports nothing.
pub(crate) const CORE_TOP_LEVEL_ANNOTATIONS: [&str; 4] =
    ["Deprecated", "deprecated", "override", "pragma"];

/// The dart:core top-level function facade — the names the unqualified
/// invocation face resolves through the implicit core import (the
/// signature stand `fn_var_sig`'s `print` arm) and the flow layer's
/// resolved back-end refuses to treat as an unresolved-call face
/// (`call_head_unresolved`) — the same fact, one table.
pub(crate) const CORE_TOP_LEVEL_FNS: [&str; 1] = ["print"];

/// The external dart: type-parameter counts, keyed by the spellings the
/// member face keeps ([`crate::member_face::CORE_TYPE_MEMBERS`]'s ten rows —
/// `sdk/lib/core/int.dart`, `double.dart`, `num.dart`, `string.dart`,
/// `bools.dart`, `list.dart` :29 `List<E>`, `map.dart` :35 `Map<K, V>`,
/// `set.dart` :33 `Set<E>`, `iterable.dart` :31 `Iterable<E>`,
/// `sdk/lib/async/future.dart` :58 `Future<T>`, upstream Dart SDK source)
/// plus `FutureOr` — the union typedef the front end synthesizes into
/// dart:async's element face with one type parameter (`sdk/lib/async/
/// future.dart:29` documents the union; the source tree carries only its
/// docs). The graph carries their top-level names only, so a type head
/// resolves to no indexed container and the count check needs this frozen
/// face; the arities are declared by the SDK, so the answer never depends on
/// imports. Other resolving names outside the member face (`Stream`, …) stay
/// absent — their member faces chain-break already, and a broader table
/// would presuppose the external-type boundary that face owns.
pub(crate) const EXTERNAL_CORE_TYPE_PARAMS: [(&str, u16); 11] = [
    ("bool", 0),
    ("double", 0),
    ("int", 0),
    ("num", 0),
    ("String", 0),
    ("Iterable", 1),
    ("List", 1),
    ("Map", 2),
    ("Set", 1),
    ("Future", 1),
    ("FutureOr", 1),
];

/// User-definable binary operators (operator-name spellings the model keeps).
pub(crate) const USER_OPERATORS: [&str; 16] = [
    "+", "-", "*", "/", "%", "~/", "|", "&", "^", "<<", ">>", ">>>", "<", ">", "<=", ">=",
];

/// The `dart:core` binary-operator signatures the member walk cannot reach
/// (`sdk/lib/core/{num,int,double,bool,string}.dart`): `(param head, result
/// head)` per `(receiver, operator)` — `int` inherits `num`'s arithmetic and
/// relational face (`num operator +(num)` etc.) and adds the bit operators
/// (`int operator &(int)`); `double` overrides the arithmetic result as
/// `double`; `bool` declares only the boolean `&`/`|`/`^` face; `String`'s
/// one user operator is `+`. The numeric-refinement face
/// (`_refineNumericInvocationTypeNullSafe`, type_system.dart:2059-2121)
/// post-processes `+`/`-`/`*`/`%` on `num`-family receivers, so the table's
/// declared result is the refinement's input, not the node's answer.
pub(crate) const CORE_BINARY_OPERATORS: [(&str, &str, &str, &str); 26] = [
    ("num", "+", "num", "num"),
    ("num", "-", "num", "num"),
    ("num", "*", "num", "num"),
    ("num", "%", "num", "num"),
    ("num", "/", "num", "double"),
    ("num", "~/", "num", "int"),
    ("num", "<", "num", "bool"),
    ("num", "<=", "num", "bool"),
    ("num", ">", "num", "bool"),
    ("num", ">=", "num", "bool"),
    ("int", "&", "int", "int"),
    ("int", "|", "int", "int"),
    ("int", "^", "int", "int"),
    ("int", "<<", "int", "int"),
    ("int", ">>", "int", "int"),
    ("int", ">>>", "int", "int"),
    ("double", "+", "num", "double"),
    ("double", "-", "num", "double"),
    ("double", "*", "num", "double"),
    ("double", "%", "num", "double"),
    ("double", "/", "num", "double"),
    ("double", "~/", "num", "int"),
    ("bool", "&", "bool", "bool"),
    ("bool", "|", "bool", "bool"),
    ("bool", "^", "bool", "bool"),
    ("String", "+", "String", "String"),
];

/// The reference shape a recorded name stands for (the names-table `kind`
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
/// (a `(name, kind, range, qualified)` row). Recorded exactly when a lookup ran and missed —
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
    /// Unresolved-name rows (the recovery surface), in source order.
    pub names: Vec<NameRow>,
    /// The usage-cluster emissions staged while the parse runs (scope
    /// frames close out of source order); merged into `diagnostics` by
    /// offset at the end of the pass.
    pub(crate) usage: Vec<Diagnostic>,
    /// Resolved member-access reference edges (#2618), in resolution
    /// order: one row per `receiver.member` / implicit-this use the
    /// pass resolved onto a declaration the element index carries. The
    /// engine folds them into the library reference surface beside the
    /// parser-level sites (member sends resolve only with type
    /// information, so the capture layer cannot produce them).
    pub member_refs: Vec<MemberRef>,
}

/// The relation kind of a resolved member-access edge (#2618) — the
/// dartq-index `RefKind` subset member accesses mint (the engine maps at
/// the library-surface boundary; `Referenced` is a type-position kind no
/// member send mints).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberRefKind {
    /// A value read of a field or getter (a method read as a value is a
    /// tear-off and mints [`Self::Invoked`], matching the index's
    /// tear-off-shaped `Invoked` face).
    Read,
    /// An invocation or tear-off-shaped use.
    Invoked,
    /// The target of a plain `=` assignment.
    Written,
    /// The target of a compound assignment.
    ReadWritten,
}

/// One resolved member-access reference edge (#2618): the member name
/// token's span and the declaration the pass's resolution landed on —
/// the target the diagnostics' own member verdicts trust, so the edge
/// inherits the resolution fidelity the differential harness pins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemberRef {
    /// The member name token's span (UTF-16).
    pub range: SourceRange,
    /// The relation kind.
    pub kind: MemberRefKind,
    /// The file declaring the resolved member.
    pub target_file: FileId,
    /// The member's lookup spelling (setter write targets under their
    /// `name=` spelling).
    pub target_name: StringId,
    /// The declaring container's name — `(file, name)` alone cannot name
    /// a member (two classes in one file may declare the same member
    /// name), so the container rides the edge and the reference row.
    pub target_container: StringId,
}

/// The `shouldIgnoreUndefined` mirror (`element.dart:8217-8250`) over the
/// library's import edges: a name referenced through an import whose target
/// did not resolve is suppressed, because the missing target is the reported
/// root cause. The filter face is show-list-only — only `show` clauses
/// qualify a name, so a bare miss under an unfiltered (or hide-only) broken
/// import still reports.
#[derive(Clone, Copy, Debug)]
pub struct UndefinedGuard<'a> {
    pub(crate) edges: &'a [ImportEdge],
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
pub(crate) enum BareInfo {
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
        /// The `undefined_method` host for an invocation miss inside an
        /// `extension` body — `Some` when the enclosing container is a
        /// plain extension (the arm replaces `this_type`, which stays
        /// `None` for extensions), carrying the display name the oracle
        /// reports: `Some(name)` for a resolved `on` head, `None` for an
        /// `InvalidType`/nullable `this` whose lookup is silent (upstream
        /// `unpromotedThisType` — `extension on Missing` never reports).
        ext_invoke: Option<Option<StringId>>,
        /// The miss sits inside an `extension` body whose `this` is a
        /// resolvable nullable type (`on C?`, `on T?`): the implicit-`this`
        /// lookup (`ThisLookup`) runs the nullable receiver arm and reports
        /// `unchecked_use_of_nullable_value` at the name.
        ext_nullable_this: bool,
    },
}

/// Where a bare-reference resolution landed (#1056) — the same decision
/// `Engine::resolve_bare` encodes into `BareInfo`, kept alongside it
/// so the query face's `binding_of` publishes it without re-resolving.
/// Mirrors the resolved-reference row face (target file, target name,
/// resolution class) at name level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefTarget {
    /// A top-level declaration, in-file or imported through the library
    /// namespace: `file`'s model owns it.
    Decl { file: FileId },
    /// A local binding of the enclosing executables (the flow layer owns
    /// its declaration span).
    Local,
    /// An in-scope type parameter.
    TypeParam,
    /// An import prefix.
    Prefix,
    /// An instance member of the enclosing container reached by the
    /// member walk.
    Member { container: StringId },
    /// The `this` keyword itself — the enclosing container's instance
    /// face (#1869-2). The container's type answers through the read-type
    /// row published beside it, so `member_of`/`type_of` resolve off the
    /// same span.
    This,
    /// Resolved, but the M1 model cannot name the target: extension-pool
    /// members, enum/`Object` synthesized members, ambiguous imports.
    Opaque,
    /// Not resolved — the `undefined_*` families own the report.
    Missing,
}

/// One published bare-reference resolution: the reference's span and
/// spelling, and where the fold's push-time decision landed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnitRef {
    /// The reference's span (the identifier token).
    pub range: SourceRange,
    /// The referenced spelling as written.
    pub name: StringId,
    /// The push-time resolution.
    pub target: RefTarget,
}

/// Per-row access intent, finalized by the send/assignment/operator events.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Intent {
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

/// The `handleExit` face a completed send fired (the listener maps each to
/// its token: the callee name's begin, or the send's end). The faces are
/// exclusive in practice — a bottom callee read resolves through locals,
/// whose declared return type suppresses — so one value carries the send.
pub(crate) enum SendExit {
    /// The callee read's own static type is `Never` (`Never f; f()`;
    /// `method_invocation_resolver.dart:129/:351/:489/:547`).
    Callee,
    /// The send's own static type is `Never` — the callee's declared return
    /// type (`type_analyzer.dart:815-819`).
    Result,
}

/// The write form an assignment applied to an operator row (`a[i] = v`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OpWrite {
    /// Plain `=`: the write-side operator (`[]=` for index rows).
    Assign,
    /// Compound (`+=`): a read of the operator plus its write side.
    Compound,
}

/// The null-aware operator face a staged re-decision reports through
/// (`_NullAwareKind`'s access/cascaded/indexExpression trio).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NullAwareKind {
    /// `?.` — the operator lexeme and its `.` replacement.
    Access,
    /// `?..` — the cascade section lexeme and its `..` replacement.
    Cascade,
    /// `?[` — the index lexeme and its `[` replacement.
    Index,
}

impl NullAwareKind {
    /// The `(operator, replacement)` message pair.
    pub(crate) fn lexemes(self) -> (&'static str, &'static str) {
        match self {
            NullAwareKind::Access => ("?.", "."),
            NullAwareKind::Cascade => ("?..", ".."),
            NullAwareKind::Index => ("?[", "["),
        }
    }
}

/// The verifier face a dead null-aware chain answers
/// (dead_code_verifier.dart's call-site split): the climb faces extend
/// through the parent chain, the cascade faces hold only while the first
/// section's body stays bare (`verifyCascadeExpression`'s
/// extraction/invocation/index-expression gate).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeadChainKind {
    /// `?.member` — `verifyNullAwareAccess` (`a?.x.gg` reports the whole
    /// climbed chain).
    Member,
    /// `?.[i]` / `?[i]` — `verifyReceiverIndexExpression`.
    Index,
    /// `?..member` first cascade section — `verifyCascadeExpression`'s
    /// property-extraction / method-invocation body gate.
    CascadeMember,
    /// `?..[i]` first cascade section (the `?..` stand-in question) — the
    /// same gate's index-expression arm.
    CascadeIndex,
}

/// A typed receiver recorded at member-merge time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Receiver {
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
    /// A container reached through a resolved prefix (`io.Directory.gone`):
    /// the middle segment resolved `name` behind `prefix`; member verdicts
    /// resolve it with [`prefixed_container`], not the bare-scope
    /// [`resolve_container`] (the name is absent from the diagnosing
    /// library's own scope).
    PrefixedContainer {
        prefix: StringId,
        name: StringId,
        kind: ContainerKind,
    },
    /// A declaration-typed name (`C c; c.m()`); `ty` is the declared type
    /// the static-nullability checks read.
    Typed { type_name: StringId, ty: Ty },
    /// A folded creation/extension-override result (`C()`, `E('x')`): the
    /// container the send resolves against, with the instance-shaped face —
    /// superclass chains and `Object` members apply, the static-face
    /// dispatch does not. `ty` is the creation's resolved instance type
    /// (`Holder<int>(x)` carries `Holder<int>`): when populated it
    /// instantiates the member face the way a declaration-typed receiver's
    /// written type does, so `member_formals_full` substitutes the
    /// container parameters the member's formals spell (#3398). `None`
    /// keeps the pre-#3398 under-report — the unsubstituted declared table.
    Created {
        name: StringId,
        kind: ContainerKind,
        ty: Option<Ty>,
    },
    /// Unknown type — checks are skipped (under-report).
    Unknown,
}

/// One recorded expression reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) name: StringId,
    pub(crate) range: SourceRange,
    pub(crate) qualified: bool,
    pub(crate) shape: RowShape,
    pub(crate) intent: Intent,
    /// The write form an assignment applied to an operator row (`a[i] = v`).
    pub(crate) write_op: Option<OpWrite>,
    /// The row's expression type as far as M1 knows it (push-time snapshot:
    /// `Null` for the `null` literal, a captured declared type for names,
    /// `Unknown` — the suppress-everything sentinel — for everything else).
    pub(crate) ty: Ty,
}

/// The structural shape of a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RowShape {
    /// A bare identifier, resolved at push time.
    Bare(BareInfo),
    /// A `receiver.member` access, merged at the dot event.
    Member { recv: Receiver },
    /// A typed receiver awaiting its member (`this`, `super`).
    Receiver(Receiver),
    /// A type-annotation position, verdict snapshotted at push time, plus
    /// the `is`/`as` suppression flag.
    TypeUse(TypeVerdict, bool),
    /// A record literal whose single positional field's type M1 decided
    /// (`(x-1)`'s `(int,)`): the operator face resolves on it (a record
    /// declares no user operators); every other consumer treats it as an
    /// opaque value.
    Record(Box<[Ty]>),
    /// A resolved non-receiver value (call results, literals).
    Value,
}

/// The #749 literal-constructor candidate facts latched at an argument
/// list's open: the callee row whose send adjudicates and the invocation's
/// head start (the class-name token for the unnamed shape; the merged
/// receiver's start for `C.named` — the whole-invocation span keys on the
/// start only).
pub(crate) struct LiteralCtorArm {
    pub(crate) callee: usize,
    pub(crate) head: SourceRange,
}

/// Scope stack frame.
pub(crate) struct Frame {
    pub(crate) kind: FrameKind,
    /// Names declared directly in this frame's scope.
    pub(crate) locals: FxHashSet<StringId>,
    /// Declared-type bindings: name → declared type. A written annotation
    /// interns with its arguments (#611), so the slot carries the written
    /// shape directly — no side channel.
    pub(crate) var_types: FxHashMap<StringId, Ty>,
    /// An old-style function-typed formal's written RETURN type
    /// (`int f()` → `int`): the formal's own `var_types` slot is `Unknown`
    /// (the element model poisons function-type annotations, rule 0c), but
    /// the invocation face's `callee_return_ty` still needs the annotation
    /// that armed before the name — the `f()?.m()` non-null verdict reads
    /// it (#1758). Keyed alongside `var_types` so the scope walk answers it
    /// the same way.
    pub(crate) fn_formal_rets: FxHashMap<StringId, Ty>,
    /// The local-function tear-off capture (#2842): a declared local
    /// function's own value IS its tear-off, and its signature is only
    /// whole at the function's own frame close — so it rides the
    /// ENCLOSING frame here, keyed by name, dropped with the scope (the
    /// same lifetime the name's `locals` entry has). Filled by
    /// [`Engine::capture_local_fn_tearoff`], read by
    /// [`Engine::local_fn_sig`].
    pub(crate) local_fn_sigs: FxHashMap<StringId, Box<FnVarSig>>,
    /// The frame's own formal list's optional-positional boundary: the
    /// required-formal count latched when `[` opens (the no-capture arm
    /// of [`Engine::fn_capture_optional_group`], the frame's own depth-1
    /// list only). The tear-off capture's required/optional split; `None`
    /// when the list has no optional group.
    pub(crate) formal_optional_from: Option<u16>,
    /// A `{` opened in the frame's own formal list: the named section the
    /// tear-off capture refuses (the closure capture's named-poison
    /// precedent — the declared-face model carries no named-formal face).
    pub(crate) formal_named: bool,
    /// `type_params` length to truncate back to when this frame closes.
    pub(crate) type_mark: usize,
    /// The row-stack length when this frame opened: type parameters that
    /// join mid-header (a method's own `<T>`, parsed after its return type)
    /// rewrite their not-yet-scoped `undefined_class` rows only above this
    /// mark, never an outer declaration's stale row.
    pub(crate) row_mark: usize,
    /// The executable's push-order serial — unique across the file's
    /// executables; containers and blocks carry 0. The `staged_write_tys`
    /// inference feed keys on it ([`Engine::staged_read_ty`]): a write
    /// staged inside one executable must never type another executable's
    /// `var` initializer — a constructor initializer's write and a later
    /// method's read share the bare-name key and, one pop later, the frame
    /// depth (#2460 A2c).
    pub(crate) serial: usize,
}

pub(crate) enum FrameKind {
    /// A class/mixin/enum/extension body: fields and the member face.
    /// `extension_face` is the unnamed-extension face-rank latch (#1701):
    /// `Some(i)` only on an unnamed extension's frame — the frame's
    /// own-member face is the i-th nameless entry of the frozen
    /// `extension_faces` pool (see [`Engine::push_container_frame`]). Every
    /// other container frame carries `None` and resolves through the
    /// named-container walk.
    Container {
        name: StringId,
        kind: ContainerKind,
        extension_face: Option<usize>,
    },
    /// An executable (method, constructor, function, function literal);
    /// `ret` carries the declared return type, the oracle host word, and the
    /// host name when the declaration shape exposes them (constructors and
    /// local functions stay `None` — their return checks under-report).
    /// `super_invoked` is the per-element `invokesSuperSelf` face: any
    /// `super` expression while this frame is the innermost executable.
    /// `body_brace` is the executable's block-body `{` (`begin_block_
    /// function_body` latches it, first brace wins — a nested closure's
    /// body latches the closure's own frame), the
    /// `body_might_complete_normally_nullable` anchor. `is_async`/`
    /// generator` latch the body's async modifier (`handle_async_modifier`):
    /// the completion warning's generator and async-legality gates
    /// (`checkForBodyMayCompleteNormally`, type_analyzer.dart:658-681).
    Executable {
        instance: bool,
        ret: Option<ExecReturn>,
        /// A function literal's completion context return — the official
        /// `imposedType` (the context function type's return with the
        /// dynamic/unknown heads dropped,
        /// function_expression_resolver.dart:44-53); declared shapes
        /// carry `None` (their return rides `ret`). The completion
        /// staging's closure face reads it, anchored at the body brace.
        ctx_ret: Option<Ty>,
        super_invoked: bool,
        body_brace: Option<SourceRange>,
        is_async: bool,
        generator: bool,
    },
    /// A statement block, catch clause, or typedef body.
    Block,
}

/// The return-check host of an executable frame (`return_of_invalid_type`'s
/// per-host message picks the word and the name).
pub(crate) struct ExecReturn {
    pub(crate) ty: Ty,
    pub(crate) host: ExecKind,
    pub(crate) name: StringId,
    /// The message face's display name (`return_of_invalid_type`'s `{2}`):
    /// a named factory qualifies its selector (`C.named`,
    /// `ConstructorElement.displayName`). `None` keeps the bare `name` —
    /// the identity key the formal tables and the self-reference credit
    /// read stays untouched.
    pub(crate) display: Option<StringId>,
    /// The declaring name's own range — the `body_might_complete_normally`
    /// family's anchor (the oracle reports at the declaring name for every
    /// shape that latches a return here; the block `{` is the closure
    /// literal's anchor, and literals latch no return).
    pub(crate) name_span: SourceRange,
    /// A factory-latched return (the container itself): the verdict may
    /// consult the declared-container negative — the oracle's constructor
    /// return check runs on the nominal faces (`factory new() => 0;`),
    /// where the written matrix suppresses.
    pub(crate) ctor: bool,
}

/// One open argument list: the arguments collected so far (name, type,
/// range — the range of the argument's expression row) plus the argument
/// currently being parsed. `mark` is the row-stack index where the current
/// argument's rows begin — the parser fires the positional-argument event
/// *after* the expression (rows already pushed) and the named-argument
/// events *before* the value, so the window from `mark` to the row count is
/// the event-order-agnostic shape of one argument.
pub(crate) struct ArgLevel {
    /// `(name, type, primitive-literal flag, range)` per argument — the
    /// flag feeds the int→double literal conversion
    /// ([`Engine::assignable`]).
    pub(crate) args: Vec<(Option<StringId>, Ty, bool, SourceRange)>,
    /// An argument is open between its `open_argument` and the next flush.
    pub(crate) arg_open: bool,
    /// The open argument's name (`None` = positional).
    pub(crate) pending_name: Option<StringId>,
    /// The first named label's token range: upstream anchors the deficit at
    /// `argumentList.leftParenthesis.next` — the first token inside the
    /// parens, the label on a named-only list — whenever no positional
    /// argument supplies the after-the-last face.
    pub(crate) first_label: Option<SourceRange>,
    /// The callee's row index at group open — [`Engine::open_arguments`]'s
    /// `top` detection, kept even when `invokes` drops the link (a bare
    /// `super(`/`this(` initializer-invocation callee, excluded from the
    /// send walk but still the group head the per-arg faces read).
    /// `mark` alone cannot serve: `flush_argument` advances it past each
    /// argument's row.
    pub(crate) callee_row: Option<usize>,
    pub(crate) mark: usize,
    /// The written signatures of the function literals that closed
    /// directly inside this list (`C((List<int> x) {})`), keyed by the
    /// literal's `(start, end)` offsets — the inference-failure face reads
    /// them as the argument types the row recovery cannot carry (a
    /// literal's row is a bare value row over its closing token).
    pub(crate) closures: Vec<(u32, u32, Box<FnVarSig>)>,
    /// This level is an enum constant's own argument list (`low(1)`): the
    /// callee is the enclosing enum's selected constructor, and the
    /// type-mismatch rows anchor at the constant's constructor reference
    /// (the constant name, or its `.named` selector) — upstream resolves the
    /// `EnumConstantDeclaration`'s implicit constructor invocation, not a
    /// member send, so the row stack's callee machinery never sees it.
    pub(crate) enum_const: bool,
    /// `(name, label range)` per named argument, in source order — the
    /// `undefined_named_parameter` face anchors at the label, not the value.
    pub(crate) named_labels: Vec<(StringId, SourceRange)>,
    /// This level's own head when it spells the UNNAMED creation posture
    /// (`const C<T>(`, `const p.C(`): the engine slot is single, so a
    /// nested argument group's open overwrites it and its close's report
    /// path may consume it — the unnamed arms re-read the latch here,
    /// level-scoped and invisible to every other slot consumer. A
    /// named-selector head (`C.named(…)`, whose arguments can span whole
    /// function literals) never latches: its fold stays with the callee
    /// transport.
    pub(crate) unnamed_head: Option<(StringId, SourceRange)>,
    /// This level's own named-selector creation head (`C<T>.named(`), the
    /// [`Self::unnamed_head`] sibling for the named fold alone: a nested
    /// creation among the arguments (`Future<Never>.error(StateError(..))`)
    /// overwrites the single engine slot before this level closes.
    pub(crate) named_head: Option<(StringId, SourceRange)>,
}

/// One open function literal's written signature capture
/// (`begin_function_expression`..`end_function_expression`): the literal's
/// own formals' written types in declaration order. Anything the capture
/// cannot render exactly (an untyped, named, or function-typed formal; an
/// `async`/generator body) poisons it, and the consumers under-report. The
/// literal's return renders `void`: the only consumer admits a
/// `void`-returning formal context, under which the official inference
/// types a synchronous literal's return as `void` for both the block and
/// the `=>` body (oracle-pinned: `C((List<int> x) => 1)` against
/// `void Function(X)` prints `void Function(List<int>)`).
pub(crate) struct ClosureCapture {
    /// The literal's start offset (its `(`).
    pub(crate) offset: u32,
    /// `frames.len()` with the literal's own frame open.
    pub(crate) frame_depth: usize,
    /// `formal_list_depth` of the literal's own formal list (0 until it
    /// opens).
    pub(crate) list_depth: u32,
    /// The literal's own formal list is open (formals of later lists — a
    /// local function in the body — never join the capture).
    pub(crate) list_open: bool,
    pub(crate) positional: Vec<Ty>,
    /// The index of the first optional positional formal (`[`), if any.
    pub(crate) optional_from: Option<usize>,
    pub(crate) poisoned: bool,
    /// The literal's context type `K`, matched and folded at open time
    /// (`inferLocalFunction`, inference_visitor_base.dart:2229-2263): the
    /// enclosing site's function type when it supplies one — the
    /// declaration's written type, the assignment target's declared type,
    /// the argument slot's formal type, or the enclosing return position's
    /// function shape. `None` keeps the no-context posture (dynamic /
    /// bare `Function` / non-function context / wrong type-parameter
    /// count — the last is re-checked at close, once the literal's own
    /// group is counted).
    pub(crate) ctx: Option<Ty>,
    /// `ctx` latched through the argument channel (the literal IS an
    /// argument): the per-formal bind keeps the live [`Engine::closure_
    /// param_context_ty`] schema path — a promotion-latched receiver below
    /// the callee re-derives at pass 2, and the begin-time latch would
    /// freeze the unpromoted face (`h.items.map((e) => …)` under `h is!
    /// Impl`). The latched `ctx` still supplies the body's return context
    /// and the final Value-row signature.
    pub(crate) ctx_from_arg: bool,
    /// The literal's own `<T…>` group size, counted while its type
    /// variables close (0 for the plain `(x) => …` shape) — the upstream
    /// match gate pairs it against `ctx`'s type-parameter count.
    pub(crate) type_params: u16,
    /// The literal's own `<T extends B…>` group with names and mapped
    /// bounds, seeded from the just-closed type-variables window the same
    /// way [`Self::type_params`] is (the parser closes the group before
    /// `begin_function_expression`). The context path's final type interns
    /// through it (`function_generic`) so the literal's static type spells
    /// `void Function<Y extends num>()` — the official
    /// `FunctionExpression.staticType` carries the declared group, and the
    /// `relateTypeParameters` equality leg decides over the real bounds
    /// instead of rejecting a count-only face.
    pub(crate) type_param_group: Vec<(StringId, Option<Ty>)>,
}

/// One open record-type annotation's captured fields: `fields` holds one
/// `Ty` per `record_type_entry` in source order (a recovered comma hole
/// rides as [`Ty::Unknown`] — the analyzer's `InvalidType` splice), and
/// `names` runs parallel for the named region (`None` = positional).
/// `named_count` latches the `{…}` region's field count at
/// `end_record_type_named_fields`: its entries are the trailing `count`
/// fields, in order.
#[derive(Default)]
pub(crate) struct RecordTyLevel {
    pub(crate) fields: Vec<Ty>,
    pub(crate) names: Vec<Option<StringId>>,
    pub(crate) named_count: usize,
    /// The in-progress field's type (latched at `handle_type`/`void`/
    /// nested `end_record_type`, consumed at `end_record_type_entry`).
    pub(crate) pending: Option<Ty>,
}

/// A model value that crossed the diagnosis boundary: a same-file entry is
/// shared in place (its slots are canonical in the unit pool the seeded
/// scratch pool preserves); a cross-file entry was re-interned there
/// and is shared behind an `Rc`, so repeat probes pay
/// one refcount bump and the entry keeps a stable address for the
/// face-chain pointer-identity dedup paths.
#[derive(Debug)]
pub(crate) enum Projected<'a, T> {
    Own(&'a T),
    Foreign(std::rc::Rc<T>),
}

impl<T> Projected<'_, T> {
    /// The underlying value's address — the identity the override faces'
    /// dedup paths compare (the same underlying member surfaced through
    /// several direct faces must stay pointer-equal, which the boundary's
    /// memo guarantees for projected entries).
    pub(crate) fn ptr(&self) -> *const T {
        match self {
            Projected::Own(entry) => *entry,
            Projected::Foreign(entry) => std::rc::Rc::as_ptr(entry),
        }
    }
}

impl<T> Clone for Projected<'_, T> {
    fn clone(&self) -> Self {
        match self {
            Projected::Own(entry) => Projected::Own(entry),
            Projected::Foreign(entry) => Projected::Foreign(std::rc::Rc::clone(entry)),
        }
    }
}

impl<T> std::ops::Deref for Projected<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        match self {
            Projected::Own(entry) => entry,
            Projected::Foreign(entry) => entry,
        }
    }
}

/// The cross-pool projection boundary (the G4 phase-2 ruling): every
/// foreign-origin `Ty` the pass reads re-interns here, so every downstream
/// consumer — render, equality, assignability — stays single-pool. Lives
/// and dies with one diagnosis; memory is bounded by the foreign slots and
/// members the file actually reads.
pub(crate) struct Boundary<'a> {
    /// The diagnosing universe's pool: the unit's own pool seeded in at
    /// construction (local slots keep their numbering — the seeded copy is
    /// what makes local/projected `Ty`s comparable and renderable through
    /// one pool), extended with the foreign lists the pass projects.
    pub(crate) args: RefCell<crate::model::ArgPool>,
    /// `(owner file, owner slot) → local slot` — one deep copy per foreign
    /// argument list per diagnosis; foreign pools are frozen behind `Arc`,
    /// so a memo entry cannot go stale.
    pub(crate) slots:
        RefCell<FxHashMap<(FileId, crate::model::ArgListId), crate::model::ArgListId>>,
    /// `(owner file, container, member) → projected entry` — one deep copy
    /// per foreign member, and the stable identity [`Projected::ptr`]
    /// compares.
    members:
        RefCell<FxHashMap<(FileId, StringId, StringId), Projected<'a, crate::model::MemberEntry>>>,
    /// `(owner file, name) → projected top-level declaration` — repeat
    /// lookups of one imported name share the copy.
    pub(crate) decls: RefCell<FxHashMap<(FileId, StringId), Projected<'a, crate::model::TopDecl>>>,
    /// `(entry address) → defining file` — the cross-file context rows'
    /// anchor origin (#1877), registered for every entry that crosses this
    /// boundary (same-file entries register the owning file too). Bounded
    /// by the members the pass projects; dropped with the diagnosis.
    pub(crate) origins: RefCell<FxHashMap<*const crate::model::MemberEntry, FileId>>,
    /// The diagnosing file — same-file values never project.
    pub(crate) own: FileId,
    /// The diagnosing library — the scope a canonicalized `prefix.Name`
    /// head re-resolves in (the same-element guard, [`Boundary::ty`]).
    own_lib: LibraryId,
    pub(crate) index: &'a ElementIndex,
    /// The resolution context a foreign `prefix.Name` head canonicalizes
    /// through (`Boundary::ty`): the owner file's own library answers the
    /// prefix's export scope, which the diagnosing library cannot spell.
    graph: &'a LibraryGraph,
    namespaces: &'a Namespaces,
    strings: &'a StringTable,
}

impl<'a> Boundary<'a> {
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn new(
        index: &'a ElementIndex,
        own: FileId,
        own_lib: LibraryId,
        unit_args: Option<&crate::model::ArgPool>,
        graph: &'a LibraryGraph,
        namespaces: &'a Namespaces,
        strings: &'a StringTable,
    ) -> Self {
        Self {
            args: RefCell::new(unit_args.cloned().unwrap_or_default()),
            slots: RefCell::new(FxHashMap::default()),
            members: RefCell::new(FxHashMap::default()),
            decls: RefCell::new(FxHashMap::default()),
            origins: RefCell::new(FxHashMap::default()),
            own,
            own_lib,
            index,
            graph,
            namespaces,
            strings,
        }
    }

    /// The pool every `Ty` this pass renders or compares resolves through.
    pub(crate) fn pool(&self) -> std::cell::Ref<'_, crate::model::ArgPool> {
        self.args.borrow()
    }

    /// `owner`'s own unit pool — the universe its entries' slots index.
    /// `None` is the stale-owner face: a foreign `Ty` cached in a member
    /// row can outlive its owner unit's residency (the unit left the index
    /// while this diagnosis still holds slots derived from it). The caller
    /// skips the projection onto the unknown face — the same stale-edge
    /// drop the reachability fold applies to cached edges — never reading
    /// a foreign slot through the diagnosing pool.
    pub(crate) fn foreign_pool(&self, owner: FileId) -> Option<&'a crate::model::ArgPool> {
        Some(self.index.get(owner)?.type_args())
    }

    /// Projects a `Ty` owned by `owner`'s pool into the diagnosing
    /// universe: same-file and argument-less types pass through untouched,
    /// a foreign named type re-interns its argument list bottom-up (the
    /// persisted-restore order — inner lists are canonical before the
    /// outer interns), memoized per `(owner, slot)`.
    pub(crate) fn ty(&self, owner: FileId, ty: Ty) -> Ty {
        if owner == self.own {
            // Same-file faces carry no pool translation, but a spelled
            // `p.T` head still canonicalizes to the element name — the
            // diagnosing file's own import prefixes resolve the leaf
            // against the identical declaration (`core.List` and `List`
            // are one element; the subtype lattice compares heads by
            // name, so the qualified spelling must fold the same way a
            // foreign signature's head already does below).
            return self.canonical_dotted_ty(ty);
        }
        match ty {
            // Record and function shapes are pool slots too: a foreign
            // slot read through the diagnosing pool indexes another
            // universe (an out-of-bounds read, or silently a different
            // shape), so both re-intern bottom-up like named arguments.
            Ty::Record { shape, nullable } => {
                if shape == crate::model::RecordShapeId::EMPTY {
                    return ty;
                }
                let Some(foreign) = self.foreign_pool(owner) else {
                    // Stale owner: no pool holds the shape — degrade to the
                    // unknown face rather than read a foreign slot through
                    // the diagnosing pool.
                    return Ty::Dynamic;
                };
                let (positional, named) = {
                    let shape = foreign.record_shape(shape);
                    (shape.positional.to_vec(), shape.named.to_vec())
                };
                let positional: Vec<Ty> =
                    positional.into_iter().map(|t| self.ty(owner, t)).collect();
                let named: Vec<(StringId, Ty)> = named
                    .into_iter()
                    .map(|(n, t)| (n, self.ty(owner, t)))
                    .collect();
                return self.args.borrow_mut().record(&positional, &named, nullable);
            }
            Ty::Function { shape, nullable } => {
                if shape == crate::model::FnShapeId::EMPTY {
                    return ty;
                }
                let Some(foreign) = self.foreign_pool(owner) else {
                    // Stale owner: no pool holds the shape — degrade to the
                    // unknown face rather than read a foreign slot through
                    // the diagnosing pool.
                    return Ty::Dynamic;
                };
                let (
                    type_params,
                    type_params_named,
                    return_ty,
                    required_positional,
                    positional,
                    named,
                ) = {
                    let shape = foreign.function_shape(shape);
                    (
                        shape.type_params,
                        shape.type_params_named.to_vec(),
                        shape.return_ty,
                        shape.required_positional,
                        shape.positional.to_vec(),
                        shape.named.to_vec(),
                    )
                };
                let return_ty = self.ty(owner, return_ty);
                let positional: Vec<Ty> =
                    positional.into_iter().map(|t| self.ty(owner, t)).collect();
                let named: Vec<(StringId, Ty)> = named
                    .into_iter()
                    .map(|(n, t)| (n, self.ty(owner, t)))
                    .collect();
                // The signature's own `<T extends B>` group rides across
                // pools verbatim when the source shape recorded it (the
                // `function_generic` producer — the count-only `function`
                // would strip the very names the generic tear-off's
                // display spells); a count-only shape keeps its count. The
                // required-positional split is identity, so it rides too.
                if type_params_named.is_empty() {
                    return self.args.borrow_mut().function_required(
                        type_params,
                        required_positional,
                        return_ty,
                        &positional,
                        &named,
                        nullable,
                    );
                }
                let type_params_named: Vec<(StringId, Option<Ty>)> = type_params_named
                    .into_iter()
                    .map(|(n, bound)| (n, bound.map(|b| self.ty(owner, b))))
                    .collect();
                return self.args.borrow_mut().function_generic_required(
                    &type_params_named,
                    required_positional,
                    return_ty,
                    &positional,
                    &named,
                    nullable,
                );
            }
            _ => {}
        }
        let Ty::Named {
            name,
            nullable,
            args,
        } = ty
        else {
            return ty;
        };
        // A `prefix.Name` head (`ast.AstNode` of a foreign signature's
        // declared type) interned its qualified spelling as ONE name — the
        // collector's `join_qualified` one-head form. In the diagnosing
        // file that name resolves nowhere (the `ast` prefix lives in the
        // owner file's own imports, not here): the subtype lattice and the
        // flow promotion gate both read it as an unresolvable head, so a
        // value typed `ast.AstNode?` never promotes through `is
        // ClassDeclaration` and the arg check compares the declared face.
        // Canonicalize to the element's bare name — the leaf after `.` is
        // the declaration the prefix's export scope resolves, identical to
        // the unqualified `AstNode` the diagnosing file imports (oracle
        // semantics: the element, never the import spelling). Only a head
        // whose prefix is a REAL import prefix of `owner`'s library is
        // rewritten; an undecidable prefix keeps the qualified face.
        let name = self.canonical_head(owner, name);
        if args == crate::model::ArgListId::EMPTY {
            return Ty::Named {
                name,
                nullable,
                args,
            };
        }
        if let Some(&local) = self.slots.borrow().get(&(owner, args)) {
            return Ty::Named {
                name,
                nullable,
                args: local,
            };
        }
        // The slot came from `owner`'s own entries; a stale owner (the unit
        // left residency under this diagnosis) has no pool to fold the list
        // from — degrade to the unknown face, never a foreign slot read.
        let Some(foreign) = self.foreign_pool(owner) else {
            return Ty::Dynamic;
        };
        let list: Vec<Ty> = foreign.get(args).to_vec();
        let projected: Vec<Ty> = list.iter().map(|&arg| self.ty(owner, arg)).collect();
        let Ty::Named { args: local, .. } =
            self.args.borrow_mut().named(name, nullable, &projected)
        else {
            unreachable!("named() always builds a named type")
        };
        self.slots.borrow_mut().insert((owner, args), local);
        Ty::Named {
            name,
            nullable,
            args: local,
        }
    }

    /// The element's bare name for a foreign `prefix.Name` head — the
    /// leaf after the `.`, but only when it denotes the SAME declaration
    /// the diagnosing file spells bare. A dotted `Ty::Named` head is
    /// always a prefixed type-use (Dart has no `Outer.Inner` type
    /// reference), never `Class.member`; the leaf the prefix's export
    /// scope returns is one element. Stripping the prefix hands the
    /// subtype lattice and flow a bare `StringId` that re-resolves in the
    /// DIAGNOSING file's scope — safe only when that scope binds the leaf
    /// to the identical declaration file (`ast.AstNode` and `AstNode`
    /// naming `ast.dart`'s one `AstNode`). A different binding — drift's
    /// own `SharedWorker` vs `web.SharedWorker` — would silently retarget
    /// the type onto a foreign element, so the qualified head stays.
    ///
    /// `None`-direction guards: the prefix registers no `Decl` in
    /// `owner`'s library, the owner has no library row, the name carries
    /// no `.`, or the bare leaf resolves to a different (or no) container
    /// in the diagnosing scope — all keep the written head, the
    /// under-report posture the qualified face already paid.
    fn canonical_head(&self, owner: FileId, name: StringId) -> StringId {
        let text = self.strings.resolve(name);
        let Some((prefix, leaf)) = text.rsplit_once('.') else {
            return name;
        };
        let Some(owner_lib) = self.graph.library_of_file(owner) else {
            return name;
        };
        let leaf_id = self.strings.intern(leaf);
        let foreign = self.namespaces.lookup_prefixed(
            self.graph,
            owner_lib,
            self.strings.intern(prefix),
            leaf_id,
        );
        let Lookup::Found(NamespaceEntry::Decl(foreign_file)) = foreign else {
            return name;
        };
        // The same-element guard: the diagnosing file must reach the
        // identical declaration file spelling the leaf bare — a local
        // container or an unqualified import binding to `foreign_file`.
        let resolved = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.own_lib,
            self.own,
            leaf_id,
        );
        let same = resolved.is_some_and(|(_, own_file)| own_file == foreign_file);
        if same { leaf_id } else { name }
    }

    /// `canonical_head` applied through a whole diagnosing-file `Ty`: the
    /// deferred type-check verdict compares an `is` operand or checked
    /// type whose spelled `p.T` heads never crossed a pool boundary — the
    /// same-element rewrite the foreign projection performs on entry,
    /// applied here for the diagnosing file's own import prefixes
    /// (`core.List` vs `List` name one element; without the rewrite the
    /// subtype lattice reads the qualified spelling as a different head
    /// and the always-true check stays silent). Bare heads and undecidable
    /// prefixes pass through untouched; argument lists and function/record
    /// leaves rewrite only where a head actually changed.
    pub(crate) fn canonical_dotted_ty(&self, ty: Ty) -> Ty {
        match ty {
            Ty::Named {
                name,
                nullable,
                args,
            } => {
                let name = self.canonical_head(self.own, name);
                if args == crate::model::ArgListId::EMPTY {
                    return Ty::Named {
                        name,
                        nullable,
                        args,
                    };
                }
                let list: Vec<Ty> = self.pool().get(args).to_vec();
                let rewritten: Vec<Ty> = list
                    .iter()
                    .map(|&arg| self.canonical_dotted_ty(arg))
                    .collect();
                if rewritten == list {
                    return Ty::Named {
                        name,
                        nullable,
                        args,
                    };
                }
                self.args.borrow_mut().named(name, nullable, &rewritten)
            }
            Ty::Record { shape, nullable } => {
                if shape == crate::model::RecordShapeId::EMPTY {
                    return ty;
                }
                let (positional, named) = {
                    let pool = self.pool();
                    let shape = pool.record_shape(shape);
                    (shape.positional.to_vec(), shape.named.to_vec())
                };
                let new_positional: Vec<Ty> = positional
                    .iter()
                    .map(|&t| self.canonical_dotted_ty(t))
                    .collect();
                let new_named: Vec<(StringId, Ty)> = named
                    .iter()
                    .map(|&(n, t)| (n, self.canonical_dotted_ty(t)))
                    .collect();
                if new_positional == positional
                    && new_named.iter().zip(named.iter()).all(|(a, b)| a == b)
                {
                    return ty;
                }
                self.args
                    .borrow_mut()
                    .record(&new_positional, &new_named, nullable)
            }
            Ty::Function { shape, nullable } => {
                if shape == crate::model::FnShapeId::EMPTY {
                    return ty;
                }
                let (
                    type_params,
                    type_params_named,
                    return_ty,
                    required_positional,
                    positional,
                    named,
                ) = {
                    let pool = self.pool();
                    let shape = pool.function_shape(shape);
                    (
                        shape.type_params,
                        shape.type_params_named.to_vec(),
                        shape.return_ty,
                        shape.required_positional,
                        shape.positional.to_vec(),
                        shape.named.to_vec(),
                    )
                };
                let new_return_ty = self.canonical_dotted_ty(return_ty);
                let new_positional: Vec<Ty> = positional
                    .iter()
                    .map(|&t| self.canonical_dotted_ty(t))
                    .collect();
                let new_named: Vec<(StringId, Ty)> = named
                    .iter()
                    .map(|&(n, t)| (n, self.canonical_dotted_ty(t)))
                    .collect();
                let new_type_params_named: Vec<(StringId, Option<Ty>)> = type_params_named
                    .iter()
                    .map(|&(n, bound)| (n, bound.map(|b| self.canonical_dotted_ty(b))))
                    .collect();
                if new_return_ty == return_ty
                    && new_positional == positional
                    && new_named.iter().zip(named.iter()).all(|(a, b)| a == b)
                    && new_type_params_named
                        .iter()
                        .zip(type_params_named.iter())
                        .all(|(a, b)| a == b)
                {
                    return ty;
                }
                // The signature's `<T extends B>` group survives the
                // canonicalization verbatim — a named shape re-interns
                // through `function_generic_required`, a count-only one
                // keeps its count (the generic tear-off's display reads
                // the group). The required-positional split is a
                // positional-LAYOUT fact the leaf rewrite preserves, so it
                // copies across onto the re-interned shape.
                if new_type_params_named.is_empty() {
                    return self.args.borrow_mut().function_required(
                        type_params,
                        required_positional,
                        new_return_ty,
                        &new_positional,
                        &new_named,
                        nullable,
                    );
                }
                self.args.borrow_mut().function_generic_required(
                    &new_type_params_named,
                    required_positional,
                    new_return_ty,
                    &new_positional,
                    &new_named,
                    nullable,
                )
            }
            _ => ty,
        }
    }

    /// Projects a member entry found in `owner`'s container `container`
    /// under `spelling`: same-file entries pass through, foreign entries
    /// deep-copy once per diagnosis with every declared type projected.
    pub(crate) fn member(
        &self,
        owner: FileId,
        container: StringId,
        spelling: StringId,
        entry: &'a crate::model::MemberEntry,
    ) -> Projected<'a, crate::model::MemberEntry> {
        // The context rows' cross-file anchor origin (#1877): registered for
        // same-file and projected entries alike — the address is the
        // canonical slot or the memo's stable Rc, so the key holds for the
        // diagnosis's lifetime.
        self.origins
            .borrow_mut()
            .insert(std::ptr::from_ref(entry), owner);
        if owner == self.own {
            return Projected::Own(entry);
        }
        if let Some(hit) = self.members.borrow().get(&(owner, container, spelling)) {
            return hit.clone();
        }
        let mut copy = entry.clone();
        copy.var_ty = self.ty(owner, copy.var_ty);
        copy.return_ty = self.ty(owner, copy.return_ty);
        for bound in copy.type_param_bounds.iter_mut() {
            *bound = self.ty(owner, *bound);
        }
        if let Some(formals) = &mut copy.formals {
            for formal in formals
                .positional
                .iter_mut()
                .chain(formals.named.iter_mut())
            {
                formal.ty = self.ty(owner, formal.ty);
            }
        }
        let projected = Projected::Foreign(std::rc::Rc::new(copy));
        self.members
            .borrow_mut()
            .insert((owner, container, spelling), projected.clone());
        projected
    }

    /// The defining file of one projected entry — the cross-file context
    /// rows' anchor origin (#1877). Total for every entry this boundary
    /// handed out: the registration runs at projection time.
    pub(crate) fn member_origin(
        &self,
        entry: &Projected<'_, crate::model::MemberEntry>,
    ) -> Option<FileId> {
        self.origins.borrow().get(&entry.ptr()).copied()
    }

    /// Projects a top-level declaration found in `owner`'s unit under
    /// `name` — the member projection's top-level twin.
    pub(crate) fn top(
        &self,
        owner: FileId,
        name: StringId,
        decl: &'a crate::model::TopDecl,
    ) -> Projected<'a, crate::model::TopDecl> {
        if owner == self.own {
            return Projected::Own(decl);
        }
        if let Some(hit) = self.decls.borrow().get(&(owner, name)) {
            return hit.clone();
        }
        let mut copy = decl.clone();
        copy.var_ty = self.ty(owner, copy.var_ty);
        copy.return_ty = self.ty(owner, copy.return_ty);
        if let Some(formals) = &mut copy.formals {
            for formal in formals
                .positional
                .iter_mut()
                .chain(formals.named.iter_mut())
            {
                formal.ty = self.ty(owner, formal.ty);
            }
        }
        let projected = Projected::Foreign(std::rc::Rc::new(copy));
        self.decls
            .borrow_mut()
            .insert((owner, name), projected.clone());
        projected
    }

    /// Interns one written argument list into the diagnosing pool. The
    /// unit capture freezes at collect time, so the pass's own written
    /// arguments (#611 consumption — the armed-head fold) intern here: the
    /// same pool every comparison and render resolves through, keeping the
    /// slots single-universe. Elements carry their slots already (trees
    /// fold bottom-up), matching the collector's intern order.
    pub(crate) fn intern_list(&self, args: &[Ty]) -> crate::model::ArgListId {
        self.args.borrow_mut().intern(args)
    }

    /// Interns a substituted function shape into the diagnosing pool —
    /// the substitution's `Ty::Function` leg re-interns its parts through
    /// the same canonical pool every written shape lives in, so derived
    /// equality on the substituted face stays structural. Every positional
    /// formal reads required: a caller that holds the source shape's split
    /// passes it through [`Self::intern_function_ty_required`], or the
    /// substituted face would part ways with the very spelling it
    /// substitutes.
    pub(crate) fn intern_function_ty(
        &self,
        type_params: u16,
        return_ty: Ty,
        positional: &[Ty],
        named: &[(StringId, Ty)],
        nullable: bool,
    ) -> Ty {
        self.args
            .borrow_mut()
            .function(type_params, return_ty, positional, named, nullable)
    }

    /// [`Self::intern_function_ty`] with the source shape's
    /// required-positional count — the split is identity, so a rebuild
    /// that kept the columns but not the count would intern a different
    /// shape.
    pub(crate) fn intern_function_ty_required(
        &self,
        type_params: u16,
        required_positional: u16,
        return_ty: Ty,
        positional: &[Ty],
        named: &[(StringId, Ty)],
        nullable: bool,
    ) -> Ty {
        self.args.borrow_mut().function_required(
            type_params,
            required_positional,
            return_ty,
            positional,
            named,
            nullable,
        )
    }

    /// [`Self::intern_function_ty`] with the signature's own
    /// `<T extends B>` group spelled (`function_generic`): a generic
    /// member's tear-off carries its declaration's parameters into the
    /// substituted face, and the `undefined_operator`/`ty_display` render
    /// spells the group verbatim (`<T>void Function(T)`). A count-only
    /// source delegates to the count-only producer (its `type_params`
    /// would be lost through the names-only slice).
    pub(crate) fn intern_function_ty_generic(
        &self,
        type_params: u16,
        type_params_named: &[(StringId, Option<Ty>)],
        return_ty: Ty,
        positional: &[Ty],
        named: &[(StringId, Ty)],
        nullable: bool,
    ) -> Ty {
        if type_params_named.is_empty() {
            return self.intern_function_ty(type_params, return_ty, positional, named, nullable);
        }
        self.args.borrow_mut().function_generic(
            type_params_named,
            return_ty,
            positional,
            named,
            nullable,
        )
    }
}

/// The member-walk outcome: found, genuinely absent after a clean walk, or a
/// broken chain (an unresolvable container hop — suppression territory).
#[derive(Clone, Debug)]
pub(crate) enum Walk<'a> {
    Found(Projected<'a, crate::model::MemberEntry>),
    Missing,
    ChainBroken,
}

/// The pass's memo over the member-face climbs ([`Engine::walk_from`]): one
/// outcome per `(container, member, super-climb)` probe. Sound because the
/// element model is frozen once `collect` completes (the pass reads it
/// through shared references) and the climb's start — the engine's
/// `file`/`lib` — is fixed for the run, so a probe's answer never goes
/// stale. Bounded by the distinct container-member pairs the pass touches;
/// dropped with the engine, never a cross-pass cache. (The member-face
/// *state* lives in [`crate::member_face::MemberFace`]; this memo stays
/// beside the diagnose-private [`Walk`] outcome it caches.)
#[derive(Default)]
pub(crate) struct FaceMemo<'a> {
    pub(crate) walks: RefCell<FxHashMap<(StringId, StringId, bool), Walk<'a>>>,
    /// The declaring container of each memoized `Found` outcome — the hop
    /// the climb found the member on (`List.map` declares on `Iterable`),
    /// the key the receiver-instantiation environment composes against —
    /// paired with that container's declaring file (#2618: the
    /// member-reference edge target).
    pub(crate) origins: RefCell<FxHashMap<(StringId, StringId, bool), (StringId, FileId)>>,
    /// The declaring container (and its file) the climb in flight recorded
    /// for its `Found` return; read back by [`Self::walk`] into `origins`,
    /// and by the caller as the probe's origin.
    pub(crate) found_origin: std::cell::Cell<Option<(StringId, FileId)>>,
    /// The prefixed supertype-clause faces of the diagnosing file's
    /// container headers, scanned once per pass on the first prefixed-face
    /// question (`(writer container, face local name)` → the written prefix
    /// head): the element model keeps only the clause continuation
    /// (`implements idl.X` records `X`), so the dotted spellings live only
    /// in the source tokens. `None` until scanned; an empty map is a real
    /// result (no dotted clause face), not an unscanned state. Bounded by
    /// the file's dotted clause faces; dropped with the engine.
    pub(crate) prefixed_clauses: RefCell<Option<FxHashMap<(StringId, StringId), StringId>>>,
}

impl<'a> FaceMemo<'a> {
    /// The memoized climb: `climb` runs only on the probe's first visit;
    /// every later probe of the same face pays one hash lookup (a found
    /// outcome clones its [`Projected`] handle — a refcount bump, never a
    /// deep copy).
    pub(crate) fn walk(
        &self,
        container: StringId,
        member: StringId,
        super_walk: bool,
        climb: impl FnOnce() -> Walk<'a>,
    ) -> Walk<'a> {
        let key = (container, member, super_walk);
        if let Some(hit) = self.walks.borrow().get(&key) {
            self.found_origin
                .set(self.origins.borrow().get(&key).copied());
            return hit.clone();
        }
        self.found_origin.set(None);
        let walk = climb();
        if let Some(origin) = self.found_origin.get() {
            self.origins.borrow_mut().insert(key, origin);
        }
        self.walks.borrow_mut().insert(key, walk.clone());
        walk
    }

    /// The declaring container of the most recent probe's `Found` outcome
    /// (memoized or fresh); `None` for a miss or a broken chain.
    pub(crate) fn last_origin(&self) -> Option<StringId> {
        self.found_origin.get().map(|(origin, _)| origin)
    }

    /// The file declaring the container of the most recent probe's `Found`
    /// outcome (#2618) — the member-edge target's declaring file; `None`
    /// for a miss or a broken chain.
    pub(crate) fn last_found_file(&self) -> Option<FileId> {
        self.found_origin.get().map(|(_, file)| file)
    }
}

/// One non-static member hit of a face-chain walk
/// ([`Engine::face_entries`]).
#[derive(Clone)]
pub(crate) struct FaceEntry<'a> {
    /// The member's lookup spelling (setters under `name=`).
    pub(crate) name: StringId,
    /// The member entry, projected into the diagnosing pool when the
    /// declaring container is foreign (the boundary funnel — the signature
    /// renderers read its declared types).
    pub(crate) entry: Projected<'a, MemberEntry>,
    /// The declaring container's name (the message's enclosing class).
    pub(crate) owner_name: StringId,
    /// The file that owns the declaring container (the private-name
    /// accessibility test).
    pub(crate) owner_file: FileId,
    /// The hop contributes to the official `implemented` map — own members,
    /// `with` mixins, superclass hops; never an `implements` hop
    /// (`inheritance_manager3.dart:333` `_addImplemented`).
    pub(crate) concrete_source: bool,
}

/// The abstract-completeness check's interface view
/// ([`Engine::collect_interface`]): the interface names in face order with a
/// representative hit each, plus the names a concrete source implements.
pub(crate) struct InterfaceView<'a> {
    pub(crate) names: Vec<StringId>,
    pub(crate) representative: FxHashMap<StringId, FaceEntry<'a>>,
    pub(crate) implemented: FxHashSet<StringId>,
    /// Every face's linearized hits in face order (the
    /// concrete-implementation and `@mustBeOverridden` walks re-read them).
    pub(crate) face_hits: Vec<Vec<FaceEntry<'a>>>,
    /// The faces behind [`InterfaceView::face_hits`], same order (the
    /// concrete-implementation scan needs the per-face split to apply the
    /// oracle's mixin-before-superclass source order).
    pub(crate) face_sources: Vec<StringId>,
    /// A concrete `noSuchMethod` on the implemented sources implements every
    /// interface name. The completeness check returns on it; the
    /// concrete-implementation loop ignores it (the oracle's dispatch runs
    /// the interface loop before any noSuchMethod reasoning).
    pub(crate) no_such_method_gate: bool,
}

/// The implicit extension-application outcome for one member access
/// ([`Engine::extension_application`]).
pub(crate) enum ExtensionApp {
    /// No applicable extension declares the member: the `undefined_*` path
    /// proceeds.
    None,
    /// Exactly one applicable extension declares it: the access is defined.
    Single,
    /// Exactly one applicable extension declares it, but its `on` type is
    /// non-nullable while the receiver reads nullable (`x.m` on `C?`): the
    /// member resolves — no `undefined_*` — yet the access is unsafe
    /// (`unchecked_use_of_nullable_value`). Distinguishing it from `Single`
    /// lets the caller emit the unsafe row beside the resolution.
    UnsafeSingle,
    /// Two or more applicable extensions declare it and none is more
    /// specific: `ambiguous_extension_member_access`, with the candidates'
    /// display pair (element-shape, inline-shape) in face declaration order.
    Ambiguous(Vec<(String, String)>),
}

/// A qualified-type prefix awaiting its name (`p` of `p.C`); dead when the
/// prefix itself already reported and the name must not.
pub(crate) struct TypePrefix {
    pub(crate) prefix: StringId,
    pub(crate) live: bool,
}

/// The push-time verdict of a type-annotation position. Rows resolve after
/// the scope frames tear down, so the verdict snapshots what the scope stack
/// said while the type was in context. Defined types push no row at all (a
/// non-type used as a type carries an unfrozen oracle code — no row, no
/// report).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TypeVerdict {
    /// The head `p` of `p.C`, not defined: `undefined_identifier` at the
    /// head (guarded); the continuation contributes nothing.
    PrefixHead,
    /// `p.C` with `p` defined and `C` absent from the prefix: the guarded
    /// miss is `undefined_class` at `C`. `local` snapshots the push-time
    /// local/parameter face of the head (the frame stack is gone by the
    /// resolve pass, so the decision must ride the verdict).
    Prefixed { prefix: StringId, local: bool },
    /// Not defined anywhere: `undefined_class` unless the position is
    /// suppressed (`is`/`as` operands carry other oracle codes).
    Missing,
    /// A redirecting-factory target head (`= C.new;`): a miss is the
    /// oracle's `redirect_to_non_class` — outside the frozen gate, so the
    /// row records without a report (a plain type use would carry the
    /// scored `undefined_class` the oracle never emits here).
    RedirectTarget,
    /// Defined but not a type: the plain resolver path reports
    /// `not_a_type` at the name (named_type_resolver.dart:794-806) — a
    /// static container member, a top-level function or variable, a
    /// formal parameter, or an import prefix answering a type position.
    /// Local variables and local functions route to the oracle's
    /// `referenced_before_declaration` instead (named_type_resolver.dart:
    /// 780-786) — outside this face, silent here.
    NonType,
}

/// The plain resolver path's three answers for a bare type head.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TypeResolution {
    /// A type-valued element (or an undecidable one): the existing silent
    /// defined face.
    Type,
    /// A resolving non-type element: `not_a_type` at the name.
    NonType,
    /// Nothing resolves: the existing miss machinery.
    Missing,
}

/// The `this`/`super` context verdict ([`Engine::keyword_receiver_state`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReceiverState {
    /// The keyword resolves (its static type is the enclosing container's).
    Valid,
    /// The keyword reports its declaration-context face
    /// (`invalid_reference_to_this` / `super_in_invalid_context`).
    Invalid,
    /// The context carries dedicated faces (`superInExtension` and
    /// `superInExtensionType`) — outside this engine's code set, silent.
    Skip,
}

/// The scoped-declaration kind's type answer (containers and aliases are
/// types; executables and variables are not; a scoped type parameter or
/// prefix entry stays silent — a prefix element's own notAType face rides
/// the [`NamespaceEntry::Prefix`] arm).
pub(crate) fn type_resolution_of(kind: MicroKind) -> TypeResolution {
    match kind {
        MicroKind::Class
        | MicroKind::Enum
        | MicroKind::Mixin
        | MicroKind::Extension
        | MicroKind::TypeAlias => TypeResolution::Type,
        // The collector registers every type variable file-scoped (the
        // unit-wide name set `duplicate_definition` checks). Outside its
        // declaration the element is not in scope — an in-scope use was
        // answered by the engine's type-parameter stack above, so a hit on
        // the file-scoped tag is a stale read and must answer `Missing`
        // (the same stale-reference posture `bare_info_of` takes for the
        // expression face).
        MicroKind::TypeParam => TypeResolution::Missing,
        MicroKind::TopLevelFn | MicroKind::TopLevelVar => TypeResolution::NonType,
        MicroKind::Prefix => TypeResolution::Type,
    }
}

/// Which part of a type annotation an identifier occupies. The prefix head
/// takes the dedicated [`Engine::arm_type_prefix`] path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TypeHead {
    /// A bare type name (`Foo`).
    Bare,
    /// The name after the dot of a qualified type (`C` of `p.C`).
    Continuation,
}

/// One local declaration of the open frame stack (the usage cluster's
/// declaration side). `uses` counts the finalized reads that resolved here;
/// the routing at frame close follows the official
/// `_visitLocalVariableElement` split (`unused_local_elements_verifier.dart`:
/// 1259-1280).
/// One deferred closure-formal re-bind: the formal's declaration
/// name-token offset (the `rebound_locals` key), the pass-1 bound face,
/// and the ingredients [`Engine::closure_context_schema`] re-runs over
/// the re-derived receiver face (the callee's row is re-found by range —
/// fold pops shift row indices between the passes).
pub(crate) struct DeferredClosureFormal {
    pub(crate) decl_offset: u32,
    pub(crate) parse_ty: Ty,
    pub(crate) callee_name: StringId,
    pub(crate) slot: super::scopes_bodies::SchemaSlot,
    pub(crate) formal: super::scopes_bodies::SchemaSlot,
    pub(crate) below: SourceRange,
}

pub(crate) struct LocalDecl {
    pub(crate) name: StringId,
    /// The name token's span (the diagnostic anchor).
    pub(crate) range: SourceRange,
    pub(crate) kind: LocalKind,
    pub(crate) uses: u32,
    /// The pattern-variable-declaration group (`None` = independent).
    pub(crate) group: Option<u32>,
    /// An if-case binder whose case scope closed: later reads of the
    /// spelling resolve past it to the outer declaration it shadowed.
    pub(crate) retired: bool,
}

/// An unannotated collection literal's element face: the shape the return
/// check re-types against the host's declared return
/// (typed_literal_resolver.dart — the context decides list/set/map, the
/// elements decide set versus map, an empty `{}` defaults to a map).
#[derive(Clone, Copy)]
pub(crate) enum UntypedLiteral {
    /// A `[...]` literal — `List<E>` in every resolution.
    List,
    /// A `{...}` literal carrying at least one top-level set element.
    SetElements,
    /// A `{...}` literal whose top-level entries are map entries (or
    /// control-flow shapes the element scan cannot classify — the map
    /// default keeps the under-report direction).
    MapEntries,
    /// The empty `{}` literal: a map in a null context, the context's own
    /// shape where one resolves.
    EmptyBraces,
}

/// The local declaration kinds the usage cluster routes on.
#[derive(Clone, Copy)]
pub(crate) enum LocalKind {
    /// A plain local variable.
    Variable,
    /// A pattern-declared variable (`var (a, b) = …`, `case var x`).
    PatternVariable,
    /// A formal parameter; the declaration site it belongs to feeds the
    /// `unused_element_parameter` body-reference rule.
    Parameter { site: Option<usize> },
    /// A catch clause's exception variable; `auto` records the official
    /// visitCatchClause auto-use (a plain `catch` without an `on` type, or
    /// a stack-trace sibling present) — patched in at `handle_catch_block`
    /// once both facts are known.
    CatchException { auto: bool },
    /// A catch clause's stack trace variable.
    CatchStack,
    /// A named local function (reports as `unused_element`).
    LocalFunction,
}

/// The captured `parameterDefined` argument: a string literal's cooked
/// value, or a bare reference's written name. The literal's value is both
/// the official gate (`computeConstantValue`'s `toStringValue`) and the
/// message face (the `SimpleStringLiteral` arm,
/// `annotation_verifier.dart:534-537`); a reference messages its written
/// text (the `toString` fallback) while its gate value resolves from the
/// file's const environment at the check.
#[derive(Clone, Copy)]
pub(crate) enum UnlessArg {
    Literal(StringId),
    Reference(StringId),
}

/// One folded index expression awaiting its argument check (#2937): the
/// operand's static type and span, plus the receiver's named head the
/// `[]`/`[]=` first formals resolve through at emission time. The write
/// form is decided by the `OpWrite` an assignment/compound mark lands on
/// the operator row; a `None` at drain is a pure read (`[]` face only).
#[derive(Clone, Copy, Debug)]
pub(crate) struct IndexArgFace {
    /// The index argument's static type at the fold.
    pub(crate) index_ty: Ty,
    /// The index argument's span — the report anchor.
    pub(crate) index_range: SourceRange,
    /// The index operand was a literal (the `argument_verdict` `literal`
    /// flag — `1` against `double` stays silent).
    pub(crate) literal: bool,
    /// The receiver's named head (`a` of `a[i]`); `None` when the
    /// receiver resolves to no named container (dynamic/unknown — the
    /// check suppresses).
    pub(crate) recv: Option<StringId>,
}

/// One annotation in flight: the written name plus, when captured, the
/// `UseResult.unless` `parameterDefined` face (the official gate's value
/// face and the written expression's span). `node` is the whole
/// Annotation node's span (`@` through the argument list's close),
/// latched at the metadata end — the upstream verdict anchor every
/// consumed entry reports on.
pub(crate) struct PendingAnnotation {
    pub(crate) name: StringId,
    pub(crate) unless: Option<(UnlessArg, SourceRange)>,
    /// The whole Annotation node span; `None` until `end_metadata`
    /// latches it (the name event pushes the entry first).
    pub(crate) node: Option<SourceRange>,
}

/// One primary-constructor formal that declares a field: the name token's
/// span anchors the report and the `var`/`final` keyword text feeds the
/// correction (`finalOrVarKeyword.lexeme`).
pub(crate) struct PrimaryFieldSite {
    pub(crate) container: StringId,
    pub(crate) name: StringId,
    pub(crate) range: SourceRange,
    pub(crate) keyword: StringId,
}

/// One optional parameter of the open executable awaiting the body verdict
/// (#1314 FAM7, `_checkUseOfDefaultValuesInParameters`): the facts the
/// `missing_default_value_for_parameter` emission needs, staged because the
/// `;`/native-body question lands only at the executable's body event.
pub(crate) struct PendingDefaultCheck {
    pub(crate) member_kind: dartq_parser::MemberKind,
    pub(crate) name: StringId,
    pub(crate) name_range: SourceRange,
    pub(crate) written: Ty,
    pub(crate) named: bool,
}

/// One non-nullable variable awaiting the initializer verdict (#1314 FAM18,
/// error_verifier.dart `_checkForNotInitializedFieldDeclaration` /
/// `visitTopLevelVariableDeclaration`): the group bits gated at the name,
/// the initializer presence marks as it parses, and the instance-field
/// half waits for the container's end (its constructor set is only then
/// complete — a generative constructor owns the
/// `ConstructorFieldsVerifier` face).
pub(crate) struct PendingInitVariable {
    pub(crate) name: StringId,
    pub(crate) range: SourceRange,
    /// The declared type was decided potentially-non-nullable at the name
    /// (`bool x`, not `dynamic`/`var`/`T`).
    pub(crate) container: Option<StringId>,
    pub(crate) is_static: bool,
    pub(crate) initialized: bool,
    /// The group carried the `final` modifier (non-`late` — a late final
    /// defers its initialization legally): the declaration walk's
    /// `final_not_initialized` face and the
    /// `ConstructorFieldsVerifier`'s constructor-span face key on it
    /// (error_verifier.dart `_visitVariableList`'s `isFinal` arms).
    pub(crate) is_final: bool,
}

/// One open switch member's case scope: `depth` is the open `switch`
/// context count at open time (the member belongs to that switch — a
/// nested switch inside a member body carries `depth + 1`), `head_done`
/// marks a completed head (a second `begin_case_expression` opens a NEW
/// member, not another head of the same one), and `shadows` holds the
/// frame types the member's pattern binders overwrote.
///
/// `head_mark`/`head_rows_mark`/`head_vars`/`head_offset`/`member_body`
/// feed the shared-case-scope join (#3519): the `locals` watermark at
/// head open, the `rows` watermark at head done (statements after it
/// mean the member carried a body — the official shared-scope group
/// boundary), the `(name, declared ty, is-final)` set the head's pattern
/// declared, and the clause anchor's offset (a member label before it
/// belongs to the label reason). A closed member's scope STAYS on the
/// stack as the group's bookkeeping row — [`Self::member_body`] marks
/// the group's end.
pub(crate) struct SwitchArmScope {
    pub(crate) depth: usize,
    pub(crate) head_done: bool,
    pub(crate) shadows: Vec<(StringId, Option<Ty>, bool)>,
    /// The `locals` watermark at head open (`#3519`): the head's pattern
    /// variables are `locals[head_mark..]` rows of its own kind.
    pub(crate) head_mark: usize,
    /// The `rows` watermark at head done: statements after it mean the
    /// member carried a body — the shared-scope group's boundary.
    pub(crate) head_rows_mark: usize,
    /// The `(name, declared ty, is-final)` set the head's pattern
    /// declared — the official `_SharedCaseScope.addAll` join input.
    pub(crate) head_vars: Vec<(StringId, Option<Ty>, bool)>,
    /// The clause anchor's offset at head open (the member-label
    /// ordering key for the shared-scope reason split).
    pub(crate) head_offset: u32,
    /// Whether the member carried statements (`rows` grew past
    /// [`Self::head_rows_mark`] at member end): the scope stays on the
    /// stack after its member closes — the trailing run of one depth's
    /// scopes is the shared-body group this flag bounds.
    pub(crate) member_body: bool,
    /// The member's `end_switch_case` already ran once on this scope: a
    /// SECOND member-end at the same depth is a `default:` member (it
    /// opens no case head, so no scope of its own) — its statements must
    /// not mark this scope's `member_body`.
    pub(crate) member_ended: bool,
    /// A synthetic `default:` member's head (the official
    /// `switchStatementSharedCaseScopeEmpty` join — `hasLabel=true` with
    /// an empty variable set).
    pub(crate) is_default: bool,
}

/// F-C: a folded `super.`-selector candidate in the open
/// constructor-initializer list, awaiting its shape — only an argument
/// group makes it the super-constructor reference; an assignment
/// (`super.name = v`, the field-initializer world) or a bare selector
/// (the `super().foo` recovery residue) reports nothing member-face.
pub(crate) struct PendingInitSuper {
    /// The folded row's offset (the selector token) — the `(` event
    /// matches it against the top row.
    pub(crate) selector_offset: u32,
    pub(crate) container: StringId,
    pub(crate) name: StringId,
    /// The report span (`super.name`; the recovery shape spans
    /// `super().name`).
    pub(crate) span: SourceRange,
    pub(crate) invoked: bool,
    /// The `super` keyword heads the initializer item (`super.name(...)` is
    /// a `SuperConstructorInvocation` upstream; `x = super.name()` inside
    /// the item is an ordinary member call that never redirects the
    /// super-constructor association).
    pub(crate) head: bool,
}

/// One redirecting-constructor edge: a `this(...)` invocation in an
/// initializer list, or a redirecting factory's `= C.name` target — the
/// enclosing constructor's spelling and container, the target's
/// (`target_container` + `target` selector, or the container's unnamed
/// face), and the report span. `factory` separates the two origins (the
/// oracle's faces differ: a factory target never reports
/// `redirect_generative_to_non_generative_constructor`, a `this(...)`
/// initializer never reports the factory-only return-type face);
/// `is_const` carries the enclosing constructor's `const` modifier for the
/// `redirect_to_non_const_constructor` query; `target_span` is the target
/// selector's own token (the generative faces anchor
/// `constructorSelector?.name2 ?? thisKeyword`).
pub(crate) struct RedirectEdge {
    pub(crate) container: StringId,
    pub(crate) ctor: StringId,
    pub(crate) target_container: StringId,
    pub(crate) target: StringId,
    pub(crate) span: SourceRange,
    pub(crate) target_span: SourceRange,
    pub(crate) factory: bool,
    pub(crate) is_const: bool,
}

/// One library-level declaration site of this file (the usage cluster's
/// library-level side), captured at the declaration events with the name
/// token's span.
#[derive(Clone)]
pub(crate) struct DeclSite {
    /// Lookup spelling — setters under `name=` (the model's convention;
    /// use checks compare against the bare name).
    pub(crate) name: StringId,
    pub(crate) range: SourceRange,
    pub(crate) kind: DeclKind,
    /// Any metadata annotation was present (the pragma/JS exemption gates'
    /// under-report superset: an annotated declaration is never reported).
    pub(crate) annotated: bool,
    /// The annotation names themselves (the `@mustCallSuper` /
    /// `@mustBeOverridden` / `@override` inheritance gates read these).
    pub(crate) annotations: Box<[StringId]>,
    /// The package:meta `@UseResult` face (`_getUseResultMetadata`,
    /// `use_result_verifier.dart`): the declaration-level input of the
    /// discarded-invocation pass; `unless` carries the
    /// `parameterDefined` value + written span.
    pub(crate) use_result: Option<UrFace>,
    /// The declaration carried the `const` modifier (constructors and
    /// factories only — the latch is clear elsewhere). The
    /// literal-constructor pass's const face reads it: the oracle's
    /// `canBeConst` opens with `!element.isConst → false`
    /// (ast.dart:13743-13744), so a non-const constructor never reports.
    pub(crate) const_ctor: bool,
    /// The declaration is a factory (`factory` keyword head): the
    /// `redirect_generative_to_non_generative_constructor` face reads it —
    /// a generative `this(...)` initializer whose target resolves here
    /// reports (error_verifier.dart:4300-4306). Only constructor-kind
    /// sites set it (the unnamed factory's member-captured head never
    /// joins a `this()` edge, whose selector is what this bit keys).
    pub(crate) is_factory: bool,
    /// The declaration carries its own `augment` keyword. Constructor
    /// sites take the head's `ctor_augment_pending` latch (the
    /// `duplicate_constructor` pass and the generative-constructor gate
    /// skip them; the fragment chain, not a second declaration, carries
    /// the augmentation); member sites take their own keyword's latch —
    /// `open_exec_augment` for the executable kinds, `group_augment` for
    /// a field declarator — so the `conflicting_static_and_instance`
    /// own-scope claim side can keep an augmenting fragment out of the
    /// introductory instance scope (member_duplicate_definition_verifier
    /// .dart:307-309 skips augmentation fragments on claims).
    pub(crate) is_augment: bool,
}

/// The package:meta `@UseResult` face of one declaration.
#[derive(Clone, Copy)]
pub(crate) struct UrFace {
    pub(crate) unless: Option<(UnlessArg, SourceRange)>,
}

/// The library-level declaration kinds the usage cluster reports on.
#[derive(Clone, Copy)]
pub(crate) enum DeclKind {
    /// A top-level function, getter, or setter.
    TopLevelFn,
    /// A top-level variable.
    TopLevelVar,
    /// A typedef (either style).
    TypeAlias,
    /// A named class/mixin/enum/extension-type declaration. Unnamed and
    /// named extensions never join: the official verifier has no
    /// extension-declaration visit (`unused_local_elements_verifier.dart`
    /// visits classes, enums, mixins, and extension types only).
    Container,
    /// A class member; `container` names the declaring container.
    Member {
        kind: MemberKind,
        is_static: bool,
        container: StringId,
    },
    /// A named constructor of `container`.
    Constructor { container: StringId },
}

/// One const generative constructor of a container: the oracle's
/// `errorRange` (the const-constructor rows' span) and the head token the
/// initializer-list scan starts from.
#[derive(Clone, Copy)]
pub(crate) struct ConstCtorSite {
    pub(crate) range: SourceRange,
    pub(crate) head: SourceRange,
}

/// The `_checkForWrongVariancePosition` member-face node spans of one
/// declaration (`error_verifier.dart:7565-7658`): the model keeps no
/// per-node spans, so the member walk anchors at the member name —
/// these slots carry the annotation-level anchors upstream emits.
#[derive(Clone, Debug, Default)]
pub(crate) struct MemberFaceSpans {
    /// The member's `DeclKind::Member` `decl_sites` index.
    pub(crate) site: usize,
    /// The declaring frame's `Engine::frames` index at the site's
    /// registration — header events of nested executables fire deeper.
    pub(crate) depth: usize,
    /// The written return annotation's span (`open_return_span`, latched
    /// at `end_method`); `None` for an absent annotation — the check
    /// then stays silent anyway (`unrelated` satisfies every keyword).
    pub(crate) return_span: Option<SourceRange>,
    /// One `X extends B` TypeParameter node span per own type variable,
    /// aligned with [`MemberEntry::type_param_names`] — an unbounded
    /// variable keeps its name span (upstream's `bound == null` arm
    /// never reads it).
    pub(crate) bounds: Vec<SourceRange>,
    /// One FormalParameter node span per member-level formal, aligned
    /// with `formals.positional` then `formals.named` — both sides keep
    /// written order (the `required` keyword rides the node).
    pub(crate) formals: Vec<SourceRange>,
    /// Open member-level formal starts, pushed at `begin_formal_parameter`
    /// and popped at the matching `end_formal_parameter` — a stack because
    /// a function-typed formal's own events nest inside (its inner
    /// formals carry a different member kind and stay out).
    pub(crate) formal_starts: Vec<u32>,
}

/// One import directive of this file (the `unused_import` side): the
/// written URI, its prefix, and the source-matched graph edge.
pub(crate) struct ImportSite {
    /// The URI as written (quotes stripped; the message's `{0}`).
    pub(crate) uri_text: String,
    /// The URI literal's span (the diagnostic anchor).
    pub(crate) uri_range: SourceRange,
    pub(crate) prefix: Option<StringId>,
    /// The `show`-combinator names with their token spans, in written
    /// order (`generateUnusedShownNameHints` walks the same list; `hide`
    /// names have no hint of their own and don't join).
    pub(crate) shown: Vec<(StringId, SourceRange)>,
}

/// One label scope (`_LabelTracker`): the labels declared in it and their
/// used flags.
pub(crate) struct LabelScope {
    pub(crate) labels: Vec<(StringId, SourceRange, bool)>,
}

/// One completed catch clause of a try (the
/// `_CatchClausesVerifier.nextCatchClause` inputs).
pub(crate) struct CatchClauseFact {
    /// The `on T` type's name (`None` = plain `catch`, and `Object` plays
    /// the same catch-all role).
    pub(crate) on_type: Option<StringId>,
    /// Whether the `on T` type was written `T?` (the subtype walk's type
    /// is the written shape — `int?` is not a subtype of `int`).
    pub(crate) on_nullable: bool,
    /// The clause's first token (`on`/`catch`).
    pub(crate) start: SourceRange,
    /// The clause block's closing brace.
    pub(crate) end: SourceRange,
}

/// The parser↔listener pairing ledger (the lints bridge's `expect_paired`
/// posture): the first miss's invariant text is retained, later misses only
/// bump the saturating count. A miss degrades its event — never the rows —
/// and the whole ledger is two words: no allocation on the healthy path.
#[derive(Default)]
pub(crate) struct PairingLedger {
    /// First miss this unit saw, as the violated invariant's text.
    pub(crate) first: Option<&'static str>,
    /// Total misses, saturating — repeated mismatches never grow anything
    /// past this word.
    pub(crate) count: u32,
}

impl PairingLedger {
    /// Records one pairing miss. Release builds stay silent — the miss only
    /// degrades its event; debug builds note the first miss once on stderr
    /// (the crate has no log facade and none is added for this).
    pub(crate) fn miss(&mut self, what: &'static str) {
        if self.first.is_none() {
            self.first = Some(what);
            #[cfg(debug_assertions)]
            eprintln!("dartq semantic: pairing miss: {what}");
        }
        self.count = self.count.saturating_add(1);
    }
}

/// The model slot a `var`/`final`-inferred declarator's resolved
/// initializer type binds into (the element model's own declared-type slot
/// carries `Unknown` for every unwritten annotation — the collector runs
/// before any inference).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum VarInferSlot {
    /// A local: the innermost frame's `var_types` slot.
    Local,
    /// A top-level variable: [`Engine::top_var_inferred_tys`].
    TopLevel,
    /// A field of `container`: [`Engine::field_inferred_tys`].
    Field(StringId),
}

/// One captured executable type-parameter window (`void f<T extends
/// num>(T t)`), in declaration order inside [`Engine::exec_windows`].
///
/// The declaration offset is the window's ordering key: a post-walk
/// name-only lookup picks the nearest declaration at or before the
/// reference, deterministically. Hash order must never decide — parallel
/// scanning interns names in a scheduler-dependent order, so a
/// `FxHashMap<StringId, _>` scan is a per-run nondeterminism source
/// (#4202).
pub(crate) struct ExecWindow {
    /// The declaring name (`f`).
    pub(crate) owner: StringId,
    /// The declaring name token's offset.
    pub(crate) at: u32,
    /// The declaring header is a method (the former `method_exec_bounds`
    /// side: the callee-side substitution faces must not see method
    /// entries race their own parameter sources).
    pub(crate) is_method: bool,
    /// The window's `(param, written bound)` pairs in declaration order.
    pub(crate) params: Box<[(StringId, Option<Ty>)]>,
}

/// The engine: one parse, then the resolution pass.
#[expect(clippy::struct_excessive_bools)] // flag set mirrors the oracle's verifier latches
pub(crate) struct Engine<'a> {
    pub(crate) graph: &'a LibraryGraph,
    pub(crate) namespaces: &'a Namespaces,
    pub(crate) index: &'a ElementIndex,
    pub(crate) strings: &'a StringTable,
    /// The boundary every foreign-origin `Ty` this pass reads crosses
    /// (G4 phase-2 ruling): foreign entries project into the diagnosis-local
    /// [`Boundary`] at the member-walk funnels, so render and equality stay
    /// single-pool and [`crate::model::ArgPool::get`]'s panic remains the
    /// tripwire for any unprojected escape.
    pub(crate) boundary: Boundary<'a>,
    pub(crate) source: &'a str,
    pub(crate) tokens: &'a TokenTable,
    /// Sparse UTF-16 → byte knots for the source-peeking helpers (row
    /// ranges are UTF-16, `source` slices by byte): one entry per non-ASCII
    /// char, built on first use; empty for an ASCII file (the rust-analyzer
    /// wide-char layout).
    pub(crate) wide_chars: std::cell::OnceCell<Vec<(u32, u32)>>,
    /// Every import prefix registered anywhere in the library graph —
    /// [`Engine::prefix_head_ty`]'s graph-wide face. Built on first use: a
    /// run whose argument walk never meets a named-type head pays nothing.
    pub(crate) prefix_names: std::cell::OnceCell<FxHashSet<StringId>>,
    /// The `prefix_head_ty` verdict per head name (the namespaces probe,
    /// the `name_resolvable` probe and the prefix-set hit are all
    /// run-stable), so a repeated `from` argument — the named-argument
    /// prologue's dominant shape — answers without re-walking any of the
    /// three legs.
    pub(crate) prefix_head_memo: std::cell::RefCell<FxHashMap<StringId, bool>>,
    /// The `refine_numeric_binary` shadow-guard verdicts, one slot per
    /// core numeric head (`int`/`double`/`num` — the only heads the guard
    /// admits). The guard's `resolve_container` probes answer run-stable
    /// (the element index and namespaces are built before the fold), so
    /// the first arithmetic send on a core-numeric receiver pays them and
    /// every later send on the same head reads the cached verdict.
    pub(crate) numeric_shadow_memo: [std::cell::Cell<Option<bool>>; 3],
    pub(crate) file: FileId,
    pub(crate) lib: LibraryId,
    pub(crate) guard: UndefinedGuard<'a>,
    pub(crate) report: FileReport,
    pub(crate) rows: Vec<Row>,
    /// Published bare-reference resolutions (#1056): one row per bare
    /// identifier the pass resolved at push time (`push_bare`) — the
    /// query face's `binding_of` publication. Parse order.
    pub(crate) refs: Vec<UnitRef>,
    /// Published bare type-reference positions (#1056): `(span, name)`
    /// per type-reference identifier (`push_type_use`) — the query
    /// face's `resolve_constructor` head resolution. Parse order.
    pub(crate) type_uses: Vec<(SourceRange, StringId)>,
    /// Unresolved-name rows a creation fold removed from `rows` (the fold
    /// truncates the stack above the callee so the folded receiver owns it).
    /// The oracle resolves an instance creation's arguments regardless of the
    /// fold, so their undefined-family verdicts still owe a pass-2 report —
    /// merged back into [`Engine::resolve_rows`] by offset.
    pub(crate) deferred_miss_rows: Vec<Row>,
    pub(crate) frames: Vec<Frame>,
    /// The open statement-list duplicate scopes (the local
    /// `duplicate_definition` face, `duplicate_definition_verifier.dart`
    /// `checkStatements`): one per block/function body/switch member, each
    /// mapping a declared name to its first span. Nested blocks push their
    /// own scope, so shadowing across scopes stays legal.
    pub(crate) dup_scopes: Vec<FxHashMap<StringId, SourceRange>>,
    /// The open for-head's declaration-list scope (`checkForVariables`):
    /// a loop head's own list is a duplicate scope of its own, disjoint
    /// from the enclosing block and reset per loop.
    pub(crate) dup_for_parts: FxHashMap<StringId, SourceRange>,
    /// Unannotated collection literals whose completed row carries no type
    /// (`push_value_row`'s opaque row), keyed by the row offset: the kind
    /// the return check re-types against the host's declared return
    /// (typed_literal_resolver.dart's context-driven resolution).
    pub(crate) untyped_literals: FxHashMap<u32, UntypedLiteral>,
    /// Completed function literals' Value-row offsets: the assignability
    /// teardown's invocation/property span widening reads `call_ends` at
    /// the RHS offset, and a literal's own `(`-formals sits in that table —
    /// widening a literal row would anchor the diagnostic inside the
    /// formal list. The literal's own span is the oracle's
    /// FunctionExpression anchor, so these offsets refuse the widening.
    pub(crate) closure_literal_starts: FxHashSet<u32>,
    /// The dart:core collection heads pre-interned: the assignability
    /// matrix's collection-vs-primitive negative and the SDK type-literal
    /// operator face compare ids, never text.
    pub(crate) collection_heads: [StringId; 4],
    /// In-scope type parameters, outermost first (truncated per frame).
    pub(crate) type_params: Vec<StringId>,
    /// Type variables declared since the last frame opened. A declaration's
    /// header events fire before its frame (`begin_class_declaration` opens
    /// after the type variables, official `parseClassOrNamedMixinApplication`),
    /// so they park here and drain into the frame that owns them.
    pub(crate) pending_type_params: Vec<StringId>,
    /// Row-stack height when the current header's `begin_type_variables`
    /// fired: the span holding the header's own bound rows. A bound can
    /// reference a parameter declared *after* it (`T extends U, U extends
    /// num` — the whole list is in scope, official element building binds
    /// every name before the deferred bound types resolve), so the rows
    /// parked parameters justify on frame open truncate back to here, never
    /// to the frame's own mark.
    pub(crate) pending_rows_mark: usize,
    /// The armed declared type: `handle_type`/`handle_void_keyword` arm,
    /// `handle_no_type` disarms. Consumed by declaration bindings and by the
    /// executable return-type latches.
    pub(crate) armed_ty: Option<Ty>,
    /// Whether [`Self::armed_ty`]'s head resolved to nothing (the `TypeUse
    /// Missing` row at the head's own offset): the oracle's element type is
    /// `InvalidType` there, and `isPotentiallyNonNullable` answers false —
    /// the uninitialized latch stays off.
    pub(crate) armed_ty_unresolved: bool,
    /// [`Self::armed_ty`]'s written extent (the same span
    /// `arm_return_span` latches): `pattern.type` for the pattern faces —
    /// the never-match family's anchor (the official pins
    /// `errorNode = pattern.type` for every kind,
    /// type_analyzer.dart:732-748; probed 3.13.1: the type name, not the
    /// bound variable). Armed by the events that arm [`Self::armed_ty`]
    /// and read only while that arm is unconsumed at the pattern sites.
    pub(crate) armed_ty_span: Option<SourceRange>,
    /// The last closed type-arguments group's end offset (UTF-16, the
    /// `>` included), latched by the listener's `end_type_arguments`.
    /// The is-check verdict's whole-expression span extends the armed
    /// type's extent to this group (`x is List<int>` ends at `>`), the
    /// oracle's `.at(node)` anchor — the armed slot alone stops at the
    /// head identifier (`handle_type` carries the name token only).
    /// Stale groups stay harmless: the verdict reads it only past the
    /// `is` operator, which an operand-side group never crosses.
    pub(crate) type_args_group_end: Option<u32>,
    /// The most recently completed written bracket type's extent: a
    /// generalized function type (`void Function(int, [List<T>])
    /// Function(int x)` — the trailing formal list, its names, and the
    /// formals' closing paren extend past the armed head and any
    /// type-arguments group) or a record type (`(int, int)`). Latched by
    /// the listener's `end_function_type`/`end_record_type`; the is-check
    /// verdict extends its span through it — the oracle's `.at(node)`
    /// ends at the TypeAnnotation's endToken, the bracket group's
    /// closer. Stale writes stay harmless: the verdict reads it only
    /// when the production starts past the `is` operator, which source
    /// order never lets an earlier declaration cross.
    pub(crate) written_ty_span: Option<SourceRange>,
    /// The closer-pushed operand row's opener: the literal faces
    /// (parenthesized expression, record/list/set/map literal) push the
    /// operand's expression row at the closing bracket, but the oracle's
    /// `.at(node)` anchors at the node offset — the opening bracket
    /// (`(null is int) is bool` anchors at `(`, not `)`). Stored as
    /// `(close_offset, open_offset)`, consumed by the is-check verdict
    /// when the operand row sits at the recorded close; any other face
    /// overwrites or consumes it first.
    pub(crate) literal_row_open: Option<(u32, u32)>,
    /// Every closed collection/record literal's `(open_offset,
    /// close_offset)` extent. A collection literal folds to ONE expression
    /// node upstream — its interior rows (a for-element's iterable, a map
    /// entry's key and value, sibling elements) are parts of that node,
    /// never separate argument expressions. The argument-window run split
    /// ([`Engine::flush_argument`]'s named-anywhere boundary) consults this
    /// table so a row starting inside a literal never opens a new run
    /// (`f(k: {for (var e in xs) e.a: e.b})` stays one argument, the
    /// iterable's type never leaks as the named argument's face).
    pub(crate) literal_extents: Vec<(u32, u32)>,
    /// The last closed formal-parameter list's closer end offset, for
    /// generalized-function-type kinds. `end_function_type` carries the
    /// `Function` keyword and the trailing `?`, never the formals' `)`
    /// the type's written extent ends at — this latch (written by the
    /// listener's `end_formal_parameters`, which always fires for the
    /// same production immediately before) supplies it.
    pub(crate) fn_formal_closer_end: Option<u32>,
    /// The open variable-declaration group's declared type (one
    /// `parse_type` per group), for the initializer checks.
    pub(crate) pending_decl_ty: Option<Ty>,
    /// [`Self::armed_ty_unresolved`] moved with the group's declared type.
    pub(crate) pending_decl_ty_unresolved: bool,
    /// This unit's `var`/`final`-inferred TOP-LEVEL variable types, keyed
    /// by declaration name: the element model's `TopDecl::var_ty` carries
    /// `Unknown` for every unwritten annotation (the collector runs before
    /// any inference), so the diagnosing pass overlays the type it infers
    /// from the initializer at [`Self::bind_var_type`] and every read face
    /// ([`Self::bare_info_of`]-style lookups, `accessor_read_ty`) answers
    /// through it instead of the `Unknown` slot.
    pub(crate) top_var_inferred_tys: FxHashMap<StringId, Ty>,
    /// The field twin of [`Self::top_var_inferred_tys`], keyed by
    /// `(declaring container, field)`: `MemberEntry::var_ty` carries the
    /// same `Unknown` for an inferred field.
    pub(crate) field_inferred_tys: FxHashMap<(StringId, StringId), Ty>,
    /// The open field/top-level variable group's declarators awaiting their
    /// initializers' inferred types. The parser folds those names before
    /// the group head fires (unlike a local group, whose names interleave
    /// with their initializers), so the single
    /// [`Self::var_infer_pending`] slot would keep only the last name —
    /// the group's declarators queue here and
    /// [`Self::take_model_declarator`] pairs each initializer with the
    /// nearest preceding name.
    pub(crate) pending_model_declarators: Vec<(StringId, SourceRange, VarInferSlot)>,
    /// Instance fields a constructor initializer list assigns (`b = e` /
    /// `this.b = e`): the `ConstructorFieldsVerifier`'s `initInInitializer`
    /// state, consulted by the container's flush. Keyed by the container so
    /// another class's initializer never releases this one's latch.
    pub(crate) ctor_init_writes: FxHashSet<(usize, StringId)>,
    /// The constructor being parsed's declaration site (`begin_constructor`
    /// opens, `end_method` closes): the per-constructor key the
    /// initializing-formal and initializer-list captures file under.
    pub(crate) open_ctor_site: Option<usize>,
    /// Per-constructor initializing formals (`this.q`) — the
    /// `ConstructorFieldsVerifier`'s `initInParameter` state, keyed by the
    /// declaring constructor's site.
    pub(crate) ctor_formals_init: FxHashSet<(usize, StringId)>,
    /// Per-constructor explicit super-call satisfaction: a `super(...)`
    /// initializer or a `super.x` formal (both invoke the superclass).
    pub(crate) ctor_super_init: FxHashSet<usize>,
    /// Redirecting generative ctors (`C.named(...) : this(...)`): their
    /// final fields initialize through the redirect target's `this.`
    /// formals, so `final_not_initialized_constructor` skips them
    /// (`ConstructorFieldsVerifier` walks non-redirecting generative
    /// ctors only). Keyed by the ctor's `decl_sites` index.
    pub(crate) ctor_redirects: FxHashSet<usize>,
    /// Per-constructor head anchors (the class-name token a generative head
    /// starts at): the `not_initialized_non_nullable_instance_field` and
    /// `implicit_super_initializer_missing_arguments` rows anchor there for
    /// named and unnamed constructors alike — the site's own range is
    /// rewritten to the selector for named heads.
    pub(crate) ctor_head_anchors: FxHashMap<usize, SourceRange>,
    /// The open factory's head name span (`K` of `factory K.a()`), latched
    /// at its name event, and whether a selector named it.
    pub(crate) factory_head: Option<(SourceRange, bool)>,
    /// Named factory sites → their head span: the `duplicate_constructor`
    /// row spans `errorRange` (type name through selector).
    pub(crate) factory_head_anchors: FxHashMap<usize, SourceRange>,
    /// Unnamed factories (`factory K()`) per container — they register no
    /// constructor site, but key as `new` in the duplicate pass.
    pub(crate) unnamed_factory_heads: Vec<(StringId, StringId, SourceRange)>,
    /// `(container, constructor)` targets of redirecting factories
    /// (`= C.named`, `= C`): the redirect forwards every parameter, so the
    /// target's optional formals are supplied (`unused_element_parameter`).
    pub(crate) factory_redirect_targets: FxHashSet<(StringId, StringId)>,
    /// Per open type-argument group: the type head it belongs to, when the
    /// group follows a type-annotation identifier (`C<int> x`, `const
    /// C<int>()`, `p.C<int>`), plus the row-stack height at `<` — the rows
    /// pushed inside the group are its own type arguments (unresolved ones
    /// report as `TypeUse` rows), so the head the group applies to sits at
    /// `base - 1`. A headless group belongs to an expression (`g<int>()`,
    /// `g<int>`).
    pub(crate) group_heads: Vec<(Option<(StringId, SourceRange)>, usize)>,
    /// The type head awaiting its group (`C` of `C<int>`), armed at the
    /// identifier and moved into `group_heads` at `begin_type_arguments`.
    pub(crate) pending_type_head: Option<(StringId, SourceRange)>,
    /// A switch-case / if-case object pattern's NamedType type-argument
    /// group, latched at `end_type_arguments` when its head row is not yet
    /// on the stack (the pattern's `Box<int, String>` head pushes only at
    /// `handle_object_pattern`, after its group closed): `(argument count,
    /// the `<`'s offset, the `>`'s end offset)` consumed by
    /// `handle_object_pattern` to emit the NamedType arity verdict the
    /// oracle anchors on `NamedType` (`_buildTypeArguments`,
    /// named_type_resolver.dart — `case Box<int, String>():` reports
    /// `wrong_number_of_type_arguments_element` at the type node).
    pub(crate) pending_pattern_type_args: Option<(u16, u32, u32)>,
    /// Expression type-argument groups awaiting their use: the callee row
    /// index, the given count, and the group span.
    pub(crate) deferred_args: Vec<(usize, u16, SourceRange)>,
    /// Per open argument list, the argument types collected so far.
    pub(crate) arg_levels: Vec<ArgLevel>,
    /// The enclosing enum constant's declared type-parameter count (the
    /// `_enum` code's declared side). Consulted only by a type-argument
    /// group at the constant's own position — outside any open argument
    /// list (`end_type_arguments`); groups inside the constructor
    /// arguments check their own heads.
    pub(crate) in_enum_element: Option<u16>,
    /// The current enum constant's implicit constructor reference: its
    /// declared name's span (the unnamed invocation's error anchor), plus
    /// the written `.named` selector when the constant picked one
    /// (the named invocation's anchor and `ctor_formals` key). Set at the
    /// `EnumValueDeclaration` event, consumed by the constant's own
    /// argument list at `close_arguments`.
    pub(crate) enum_const_call: Option<(SourceRange, Option<(StringId, SourceRange)>)>,
    /// The row-stack mark where the current function body's expression
    /// began (the arrow-body return check reads it).
    pub(crate) body_mark: Option<usize>,
    /// The open switch/pattern state: open switch contexts, the pattern
    /// bracket stack, the case-header pattern windows, and the in-flight
    /// constant pattern's classification ([`SwitchTracker`], #444).
    pub(crate) switch: SwitchTracker,
    /// The next operand pattern is a logical-and/or operand (`Some(is_or)`
    /// — the wildcard check needs the operator: only the `&&` kind is
    /// wired in the official, `type_analyzer.dart:1347-1359`).
    pub(crate) pending_binary_operand: Option<bool>,
    /// The in-flight constant pattern's armed static type and anchor
    /// ([`ConstTracker`], #1050): the same literal events arm it, the
    /// pattern's end consumes it (`visitConstantPattern`'s `_canBeEqual`
    /// inputs, constant_verifier.dart:140-165).
    pub(crate) const_fold: ConstTracker,
    /// The member-face subsystem ([`MemberFace`], #1052): the interned
    /// member/type spellings, the write-target spelling cache, and the
    /// member-invocation latches the member checks read.
    pub(crate) members: MemberFace,
    /// [`EXTERNAL_CORE_TYPE_PARAMS`] pre-interned once per engine: the
    /// external type-arity lookups compare ids, never re-resolve text.
    pub(crate) external_type_params: [(StringId, u16); 11],
    /// Per open `==`/`!=` comparison: the row-stack mark at the operator
    /// (the left operand's row sits at `mark - 1`, the right's is the last
    /// row at the close) and the operator span — the
    /// `_checkForInvariantNullComparison` inputs.
    pub(crate) null_cmp_groups: Vec<(usize, SourceRange)>,
    /// The row-stack mark where the current variable initializer's
    /// expression began (the initializer check reads it).
    pub(crate) init_mark: Option<usize>,
    /// The row-stack mark where the current return statement's expression
    /// began (the return check reads it).
    pub(crate) return_mark: Option<usize>,
    /// Inside a cascade (`a?..b` picks the `?..` message shape).
    pub(crate) in_cascade: bool,
    /// Per open argument list: the callee row index, when identifiable
    /// (`None` while inside a `new`/`const`/implicit-creation reference —
    /// its class name is a type use, never an invocation callee).
    pub(crate) invokes: Vec<Option<usize>>,
    /// The argument group that just closed, awaiting the send event that
    /// consumes it (an argless send never opens a group, so a bare read
    /// stays a read). The closing-paren end offset pairs with the index so
    /// `consume_send` honors the link only for the send whose own closing
    /// token is that paren — a constructor invocation's `()` arms the link
    /// but emits no `send` event, so the stale index must not fold a later
    /// statement's send (`var b = C.new;` stealing `prefix.C<int>.new()`'s
    /// callee, the d1b3378f theft class).
    pub(crate) pending_send: Option<(usize, u32)>,
    /// The callee placeholder index and the closing-token end offset of the
    /// last completed send (single slot, FAM7 latch-group pattern): a
    /// resolved send leaves its member row in-stack, so a following
    /// `.member` merge can re-anchor on it when recovery shapes left the
    /// argument window's rows above it (the F-H geometric-mismatch drain).
    /// The end bound lets the drain tell a row genuinely inside the
    /// completed send (ends at or before its closing token) from a later
    /// expression's own receiver that merely sits above the latch.
    /// Single-shot: read and cleared at the next merge.
    pub(crate) last_send_callee: Option<(usize, u32)>,
    /// The follower-token offset of a send whose head was a recovery-spliced
    /// synthetic (`return is(x-1)`'s `<synthetic> is`): such a head fired no
    /// identifier event, so the send leaves no row and the operand side of
    /// the following `is` operator is the recovery face. Matched and cleared
    /// at the operator's own event.
    pub(crate) recovered_send: Option<u32>,
    /// The row-stack mark at `begin_is_operator_type`: the rows above it are
    /// exactly the `is` type's own residue (recovery shapes land as
    /// suppressed `TypeUse` rows), so the recovered-operand fold drops the
    /// window without touching the rows below it.
    pub(crate) is_type_mark: Option<usize>,
    /// The row-stack marks of the open parenthesized/record literals (LIFO:
    /// a field's own record nests inside its parent's window), each paired
    /// with its `(` token: the record face reads its field rows between its
    /// mark and the literal's end event, splitting fields on the window's
    /// top-level commas (the token side of the pair locates them); the
    /// parenthesized face discards its mark.
    pub(crate) record_marks: Vec<(usize, Token)>,
    /// Per open literal, the named-field labels in source order
    /// (`handle_named_record_field` — the name token precedes the colon);
    /// parallel to [`Self::record_marks`]. Consumers pair the labels with
    /// the fields each window's own `ident :` prefix marks named — source
    /// order is not the partition (a named field may precede a positional
    /// one, `(n: 1, 2)`).
    pub(crate) record_literal_names: Vec<Vec<StringId>>,
    /// The open record-type annotations' field stack: each level collects
    /// one field `Ty` per `record_type_entry` plus the named region's field
    /// names — the record's `Ty` interned at `end_record_type` and fed into
    /// the parent's pending slot (or the armed slot at the top level).
    /// The M1 model's written-record face (F-R).
    pub(crate) record_ty_stack: Vec<RecordTyLevel>,
    pub(crate) type_prefix: Option<TypePrefix>,
    /// A group-less prefixed type (`p.C`) awaiting its completion (#2523):
    /// the prefix segment's token (the completing `handle_type` fires AT
    /// the prefix token — the official `PrefixedType.parseType` protocol)
    /// and the leaf segment's name plus span. The completion arms the
    /// LEAF's name (`p.C` keeps `C`); the prefix's own lexeme would name
    /// no container anywhere downstream.
    pub(crate) pending_prefixed: Option<(SourceRange, Option<(StringId, SourceRange)>)>,
    /// Suppression depth inside `is`/`as` type operands.
    pub(crate) non_type_pos: u32,
    /// Suppression depth inside `is` type operands only (the usage
    /// cluster's `_insideIsExpression` gate — `as` operands still count as
    /// type uses).
    pub(crate) is_type_pos: u32,
    /// Row-window marker for the `as` operand's type parse (the `is` twin
    /// is [`Engine::is_type_mark`]): the cast face scans the window at the
    /// operator event for the suppressed `Missing` row.
    pub(crate) as_type_mark: Option<usize>,
    /// The with-clause entry spans (`class_used_as_mixin`'s candidates):
    /// the clause ledger answers only unresolvable heads, so a resolved
    /// entry's shape re-decides at the teardown against the whole-file
    /// container set.
    pub(crate) pending_mixin_uses: Vec<SourceRange>,
    /// Suppression depth inside `new`/`const`/implicit-creation expressions.
    pub(crate) ctor_pos: u32,
    /// The open argument-list depth ([`Self::arg_levels`]) at each
    /// `ctor_pos` entry: a call nested in a creation's own argument list
    /// is a plain send, not the creation head.
    pub(crate) ctor_pos_levels: Vec<usize>,
    /// The unit's [`InvalidTypeNames`], latched before the resolve pass
    /// (which takes `rows`): the InvalidType spellings member and
    /// extension faces consult.
    pub(crate) invalid_type_names: InvalidTypeNames,
    /// Depth inside the keyword-less implicit-creation form
    /// (`prefix.C<int>.new()`, `g<int>(...)`). The keyword forms keep the
    /// `ConstructorReference` shape — the `.new` selector resolves silently
    /// on a dead head — while the implicit form's ast_rewrite keeps the
    /// selector chain, whose `new` reports the `name == 'new'` method face
    /// on the Invalid receiver (type_property_resolver.dart:78-83).
    pub(crate) implicit_creation_depth: u32,
    /// The open constructor reference's segments `(class, ctor-selector)`:
    /// the wave-3 parser emits reference heads as `ConstructorReference`
    /// type uses (resolved names push no row), so the reference's
    /// type-argument group is matched to its head through this state
    /// instead of the row stack. The selector carries its name range — a
    /// two-segment qualified reference's (`Missing.new`) importPrefix face
    /// spans the whole written head. Cleared at `end_constructor_reference`.
    pub(crate) creation_ref: Option<(StringId, Option<(StringId, SourceRange)>)>,
    /// Constructor-reference heads that resolved to an import prefix
    /// (`fb.RuleState.stable()`): the oracle's `resolveConstructorTypeReference`
    /// importPrefix arm looks the class segment up in the prefix's scope — a
    /// hit stays silent, a miss (or a non-type element) reports
    /// `creation_with_non_type` AT the class-name token, never the prefix's
    /// span (named_type_resolver.dart:134-332). The head offset latches here
    /// at `push_type_use`; `end_constructor_reference` resolves the latched
    /// head against `creation_ref`'s class segment and reports or clears.
    pub(crate) ctor_prefix_heads: Vec<u32>,
    /// Open redirecting-factory body depth (`= C.new;`): the target
    /// reference's head resolves through the redirect face — a miss is the
    /// oracle's `redirect_to_non_class` (outside the frozen gate), never
    /// the plain `undefined_class` a type use would carry.
    pub(crate) redirect_target_depth: u32,
    /// The open redirecting factory's parsed `= C.name` reference —
    /// `(target container head, target constructor selector, whole-head
    /// span)`, latched by the member merge while `redirect_target_depth`
    /// covers the body and consumed by `end_redirecting_factory_body`,
    /// which writes the `recursive_constructor_redirect` edge for the open
    /// factory's own constructor key.
    pub(crate) factory_redirect_target: Option<(StringId, StringId, SourceRange)>,
    /// executable's return type, so `=> body` runs the official return
    /// check even on the `new`-recovery heads (`factory new() => 0;`).
    pub(crate) factory_ret_open: bool,
    /// Containers whose implicit unnamed constructor is gone because the
    /// recovery turned a `new.head` declaration into a NAMED constructor
    /// (`new.named();` — upstream `newConstructorDotName` recovery names it
    /// `named`, killing the implicit unnamed face). The collector's capture
    /// cannot see the dot-name shape (its selector event never fires), so
    /// the diagnoser corrects the face for the enum-constant check.
    pub(crate) ctor_new_head_kills: FxHashSet<StringId>,
    /// Enum constants without a constructor selector, armed at the name and
    /// cancelled by a selector event: the flush at the enum's close reports
    /// `undefined_enum_constructor` when the enum has no unnamed
    /// constructor (the official `visitEnumConstantDeclaration` unnamed
    /// arm, `type_analyzer.dart:2940-2946`). Buffered because the kill
    /// (a member declaration) always follows the constants in source.
    pub(crate) pending_unnamed_enum_consts: Vec<(StringId, StringId, SourceRange)>,
    /// Enums whose constants' selector check missed at least once: each
    /// missed constant's element still references the unnamed constructor
    /// upstream, so the enum's unnamed-ctor site counts as used — the
    /// enum's close applies the mark (the constructor sites complete with
    /// the body).
    pub(crate) enum_unnamed_ctor_uses: Vec<StringId>,
    /// The open enum declaration's `(name range, is augmentation)` pair,
    /// latched at `begin_enum_declaration`: `handle_enum_elements` reads it
    /// when the constant list closes — an empty list on a non-augmentation
    /// enum reports `enum_without_constants` at the name (the official
    /// `checkEnumDeclaration` gate, error_verifier.dart
    /// `!isAugmentation && constants.isEmpty`, reported at
    /// `node.namePart.typeName`).
    pub(crate) enum_decl_site: Option<(SourceRange, bool)>,
    /// One literal-constructor slot per open argument list (#749): `Some`
    /// when the list armed a candidate — the facts the send adjudicates,
    /// paired LIFO with the capture's
    /// [`crate::const_eval::WindowKind::LiteralCtor`] windows — `None` for
    /// every other list. The unconditional slot keys each close to its own
    /// list's `end_arguments`: a non-candidate nested list can never steal
    /// the candidate's arm.
    pub(crate) literal_ctor_arms: Vec<Option<LiteralCtorArm>>,
    /// Closed candidate argument lists awaiting their send: the callee row
    /// index, the captured argument trees, and the whole-invocation span.
    /// Adjudicated (and popped) at the send's `consume_send`.
    pub(crate) literal_ctor_stash: Vec<(usize, Vec<crate::const_eval::Node>, SourceRange)>,
    /// The pairing ledger behind the audited stash pops (see
    /// [`PairingLedger`]) — degraded events record here, the pass never
    /// panics on a pairing miss.
    pub(crate) pairing: PairingLedger,
    /// Formal-parameter default values are const contexts (a candidate
    /// creation inside one is implicitly `const` — the oracle is silent).
    pub(crate) default_value_depth: u32,
    /// #1314 FAM7: the current formal's written type, snapshotted at its
    /// name event (the annotation is still armed there; a nested
    /// function-typed formal's own list parks and restores it).
    pub(crate) formal_written: Option<Ty>,
    /// The parked outer-formal snapshot across a nested formal list
    /// (`void f([void g(int x)])` — the inner formals re-arm the type).
    pub(crate) formal_written_saved: Option<Ty>,
    /// Formal-parameter lists open around the current one (1 = the
    /// executable's own list; 2+ = a function-typed formal's nested list).
    pub(crate) formal_list_depth: u32,
    /// Open `Function(...)` type formal lists: their parameter names are
    /// type-level spellings, never locals of the enclosing body.
    pub(crate) fn_type_formal_lists: u32,
    /// The open constructor's `this.x` formal names: in scope through the
    /// initializer list only — the body block reads the FIELD (the
    /// initializing-formal scope ends at the body).
    pub(crate) this_formal_names: Vec<(StringId, usize)>,
    /// Row-stack marks of the open switch EXPRESSIONS (LIFO): the closed
    /// expression folds its window into one opaque value row.
    pub(crate) switch_expr_marks: Vec<usize>,
    /// The open constructor's `super.x` formals awaiting the
    /// initializer-list's super-invocation target — association is a
    /// resolution product (upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/generated/error_verifier.dart:2219-2251` reads
    /// `constructorElement.superConstructor`, the *actual* redirect
    /// target): `super.named(...)` picks that ctor's table, the unnamed
    /// face otherwise. Each entry is `(owner, positional index among SUPER
    /// formals only, named?, name, name span, container)` — `owner` is the
    /// enclosing constructor's decl site (`open_ctor_site` at the formal's
    /// close): a secondary constructor's entries drain at its own
    /// `end_initializers`/`end_constructor`, while the primary
    /// constructor's buffer at the class header and resolve at its
    /// `this : super.named()` body (or the class's own close when no body
    /// exists). `indexIn` counts `SuperFormalParameter`s alone
    /// (element.dart:10779-10783).
    pub(crate) pending_super_formals:
        Vec<(Option<usize>, u32, bool, StringId, SourceRange, StringId)>,
    /// The index of the current formal among the list's `super.`
    /// positionals — not among all positionals (`D(super.x, this.w,
    /// super.y)` associates `super.y` at index 1, never 2).
    pub(crate) super_formal_positional_index: u32,
    /// The class's primary constructor's decl site (`DeclKind::Constructor`
    /// — the `ConstructorFieldsVerifier`'s generative-constructor set
    /// upstream, `_addPrimaryConstructor`, constructor_fields_verifier.dart).
    /// Registered at `begin_primary_constructor` so its `this.x` formals
    /// feed `ctor_formals_init` like a secondary constructor's; re-armed
    /// into `open_ctor_site` across the `this : ...` body so the body's
    /// field initializers release the same latches.
    pub(crate) primary_ctor_site: Option<usize>,
    /// #1463: an old-style function-typed formal (`void f(int g())`) is in
    /// flight. Its name rides the parser's inline-name path — no identifier
    /// event fires for it, so the `FormalParameterDeclaration` mux arm
    /// cannot bind it; the formal's own `end_formal_parameter` consumes
    /// this arm and binds the name instead. The close event re-arms across
    /// a nested formal (`void f(void g(void h()))` — h's take consumes the
    /// arm g's own take still needs).
    pub(crate) fn_typed_formal_open: bool,
    /// #1758: the in-flight old-style function-typed formals' written
    /// RETURN types (`int f()` → `int`). `begin_function_typed_formal_
    /// parameter` pushes a `None` slot; the first type completion after it
    /// (the parser re-walks the return annotation before the nested list,
    /// formal_params.rs:706-719) fills the innermost unfilled slot whose
    /// recorded `group_heads` depth matches the completion's — so a type
    /// argument inside the annotation (`List<int>?`s `int`) never claims a
    /// slot that belongs to the head completion — and the formal's
    /// `end_formal_parameter` pops its own slot into the frame's
    /// `fn_formal_rets` — `callee_return_ty`'s local arm answers it for the
    /// `f()?.m()` non-null verdict. A stack because the formals nest
    /// (`int f(int g())`).
    pub(crate) fn_formal_ret_pending: Vec<(usize, Option<Ty>)>,
    /// The innermost closed function-typed formal's captured return type,
    /// staged by `end_function_typed_formal_parameter` for the owning
    /// `end_formal_parameter` (the `dynamic` param of a nested
    /// `Function(dynamic)` inside the return annotation ends first, so the
    /// slot pop cannot wait for `end_formal_parameter`, #1758).
    pub(crate) last_fn_formal_ret: Option<Ty>,
    /// The current formal's `:` default separator (the default-value open
    /// event's token spells it).
    pub(crate) formal_colon: Option<SourceRange>,
    /// The current formal carries a default value (its separator was seen).
    pub(crate) formal_has_default: bool,
    /// The open executable's `augment` keyword (`begin_top_level_method`/
    /// `begin_method`/`begin_factory`): the oracle's
    /// `defaultValuesAreExpected` excludes augmented executables.
    pub(crate) open_exec_augment: bool,
    /// #1314 FAM7: the open executable's optional-parameter rows awaiting
    /// the body verdict (`_checkUseOfDefaultValuesInParameters`, the
    /// `defaultValuesAreExpected` gate). Flushed at the executable's body
    /// events, dropped on the empty/native ones.
    pub(crate) pending_default_checks: Vec<PendingDefaultCheck>,
    /// #1314 FAM13: type-argument groups open around the current type
    /// reference (`List<Undefined>` — the annotation itself is not an
    /// argument, its references are).
    pub(crate) type_arg_depth: u32,
    /// The type-argument depth saved at each open function/record type
    /// (`List<L Function()>`): the oracle's type-argument face is the
    /// DIRECT parent (`node.parent is TypeArgumentList`,
    /// named_type_resolver.dart:877), so a function/record type's own
    /// return/formal/field types are plain positions — the depth resets to
    /// zero inside and restores on close. A type-argument group nested
    /// inside (`void Function(List<L>)`) re-enters the face.
    pub(crate) type_arg_depth_saved: Vec<u32>,
    /// #1314 FAM13: the offsets of unresolved type references pushed inside
    /// a type-argument group (the `non_type_as_type_argument` face) and
    /// inside a creation head context (the `creation_with_non_type` face) —
    /// the resolver's context chain picks the code
    /// (named_type_resolver.dart:720-770); ascending, consumed by
    /// [`Engine::resolve_type_row`].
    pub(crate) type_arg_miss_rows: Vec<u32>,
    /// The `not_a_type` context anchors: the resolved non-type element's
    /// declaration (file, name-token span) keyed by the type use's offset —
    /// the oracle's conditional context row
    /// (named_type_resolver.dart:794-806). Ascending, consumed by
    /// [`Engine::resolve_type_row`].
    pub(crate) non_type_anchors: Vec<(u32, (FileId, SourceRange))>,
    /// The creation-head miss offsets (`#1314 FAM13`): each entry pairs the
    /// head's offset with the explicit-keyword flag — a `new`/`const` head
    /// that fails to resolve reports `creation_with_non_type` outright
    /// (upstream Dart SDK source, named_type_resolver.dart:285-289/:313-330
    /// — the `ConstructorInvocation` parent routes both the unresolved and
    /// the resolved-non-class shapes), while an implicit creation's
    /// unresolved head keeps `undefined_class`.
    pub(crate) creation_miss_rows: Vec<(u32, bool)>,
    /// #1314 FAM14: the header-clause entries' offsets with their clause
    /// face, in parse order (ascending). The resolver's context chain
    /// picks the clause code for an unresolved clause name
    /// (scope_analyzer.dart `_resolveType`); consumed by
    /// [`Engine::resolve_type_row`].
    pub(crate) clause_miss: Vec<(u32, ClauseTag)>,
    /// The open header's implements-clause seen bit: the recovered
    /// extends-after-implements tail keys its `extends_non_class` routing
    /// on it (reset at every header arm).
    pub(crate) header_implements_seen: bool,
    /// Rows pushed inside an `if (...)` conditional-URI configuration: the
    /// configuration is never resolved as an expression (the oracle
    /// resolves only the dottedName), so the interpolated `==` value's
    /// name references resolve to nothing — the parser's
    /// invalid_literal_in_configuration span carries them and the
    /// resolution pass drops the rows.
    pub(crate) conditional_uri_rows: FxHashSet<u32>,
    /// Whether the parse currently sits inside a conditional-URI
    /// configuration (`begin_conditional_uri`..`end_conditional_uri`).
    pub(crate) conditional_uri_window: bool,
    /// #1444: the open extension-type header's clause-capture phase. The
    /// capture opens at the declaration begin and closes at the body; the
    /// representation's formals park it (their types are ordinary written
    /// types, not clause entries).
    pub(crate) extension_clause_phase: ExtensionClausePhase,
    /// #1444: the extension-type headers' clause-entry offsets (the
    /// depth-0 type heads written in `extends`/`with`/`implements`
    /// positions), ascending. A hit suppresses the resolver's
    /// `undefined_class` fallthrough — the official clause-parent guard
    /// defers the face (named_type_resolver.dart, `reportNullOrNonType
    /// Element`: "Ignored. The error will be reported elsewhere"), and the
    /// extends/with clause nodes never attach to the extension-type AST at
    /// all (ast_builder.dart `endExtensionTypeDeclaration` pops only the
    /// implements clause). Consumed by [`Engine::resolve_type_row`].
    pub(crate) extension_clause_miss: Vec<u32>,
    /// #1314 FAM18: the latched non-nullable variables (the group bits
    /// gated at each name; the flush splits at the group's end).
    pub(crate) pending_init_variables: Vec<PendingInitVariable>,
    /// #1314 FAM18: the open group's `late`/`final`/`const` bits (the
    /// uninitialized-non-nullable latch's gates).
    pub(crate) group_late: bool,
    pub(crate) group_final: bool,
    pub(crate) group_const: bool,
    /// #2867: the open group's `augment` bit — an `augment` field declares
    /// no new field, so the `final_not_initialized` /
    /// `not_initialized_non_nullable_variable` latches never arm on it
    /// (the oracle's `checkElement` walks the field element the augment
    /// merges into; the augment fragment owns no initializer obligation).
    pub(crate) group_augment: bool,
    /// #1314 FAM10: the diagnosing file's own executables' type parameters
    /// with their written bounds (`void f<T extends num>(T t)` →
    /// `[(T, num)]`), in DECLARATION order. The argument-check
    /// substitution reads them for same-file generic callees; the post-walk
    /// name-only scans (`exec_param_bound` / `param_bound_of`) read them
    /// too, and that order is load-bearing: a `FxHashMap` keyed by
    /// `StringId` iterates in hash order, which moves run to run because
    /// parallel scanning interns names in a scheduler-dependent order —
    /// a first-match scan over it flips diagnostics between byte-identical
    /// inputs (#4202). One ordered list is the single ordering source;
    /// exact-owner lookups scan it from the back (keep-last, matching the
    /// former map inserts).
    pub(crate) exec_windows: Vec<ExecWindow>,
    /// The inference parameters whose argument round FAILED the ground
    /// subtype match (`infer_gather` returned false — `_tryMatchSubtypeOf`'s
    /// `success` gate rewinding the round): the effective-formal face reads
    /// them as undecidable (`Ty::UNKNOWN`) so the argument walk suppresses
    /// the failed slot exactly as the oracle's
    /// `argumentTypeNotAssignable` walk does beside its `couldNotInfer`
    /// (#3794). Cleared before each send's constraint solve.
    pub(crate) infer_poisoned_params: FxHashSet<StringId>,
    /// Whether the latched [`Engine::pending_exec_name`] header is a
    /// method (`MethodDeclaration` — the window's `is_method` bit) or a
    /// top-level function.
    pub(crate) pending_exec_is_method: bool,
    /// The written signatures of function-typed variables (top-level
    /// variables and fields annotated `T Function<T>(T)`), keyed by
    /// variable name — the invocation face's formals for callees the
    /// element model cannot see (a variable's function-type annotation
    /// poisons the declared-type slot).
    pub(crate) fn_var_sigs: FxHashMap<StringId, Box<FnVarSig>>,
    /// The fn_var_sigs subset claimed at FORMAL declarations only (a
    /// parameter whose written annotation is a function type): the arity
    /// gate reads this table to count a shadowed local's call against its
    /// own written signature without opening the stale-stash attribution
    /// the general table carries (a local function's same-spelled entry
    /// must never lend formals).
    pub(crate) fn_param_sigs: FxHashMap<StringId, Box<FnVarSig>>,
    /// The written generic-function-type signatures of formals, keyed by
    /// declaring executable spelling (`void useNum(void Function(num) f)`
    /// → `useNum`). The argument check reads them for the
    /// function-subtype face: the element model carries `Unknown` for the
    /// annotation (rule 0c), so this table is the only structural record
    /// of the written shape.
    pub(crate) fn_formal_sigs: FxHashMap<StringId, FnFormalSigs>,
    /// The function-typed tear-off signature of each open argument,
    /// keyed by the argument row's offset: a bare unqualified read of a
    /// top-level function is a tear-off whose type is the function's own
    /// signature (the row's `Ty` stays `Unknown` — the model has no
    /// function variant). Consumed by the argument check.
    pub(crate) arg_sigs: FxHashMap<u32, Box<FnVarSig>>,
    /// The open function literals' signature captures (nested literals
    /// stack); a closed literal's signature joins the enclosing argument
    /// list's [`ArgLevel::closures`].
    pub(crate) closure_caps: Vec<ClosureCapture>,
    /// The open formal's written `required` keyword (`begin_formal_parameter`
    /// latches, the written-signature capture's named slot consumes).
    pub(crate) capture_formal_required: bool,
    /// The open formal list's section for the written-signature claims
    /// (`[`/`{` flips it; a capture-open group — the annotation's own
    /// list — leaves it). The claim routes on it: named signatures key
    /// by spelling, positional ones join the declaration-order vector.
    pub(crate) fn_formal_named_section: bool,
    /// The open written function-type captures (nested annotations stack).
    pub(crate) fn_type_capture: Vec<FnTypeCapture>,
    /// The last inner function-type close left the enclosing capture
    /// unclaimed (recovery poison face, read once by the next
    /// `fn_capture_begin` over an open capture).
    pub(crate) fn_nested_unclaimed: bool,
    /// The last completed written signature, claimed by the next variable
    /// group's names and dropped at every declaration boundary in between.
    pub(crate) fn_type_stash: Option<Box<FnVarSig>>,
    /// The written required-positional count of each interned function
    /// shape the walk sealed (`end_function_type`). [`crate::model::FnShape`]
    /// merges required and optional positionals into one column — the
    /// canonical interning makes `Function(int)` and `Function([int])` the
    /// same `FnShapeId`, so the required-vs-optional split the subtype
    /// function rule (`_isFunctionSubtypeOf`, subtype.dart:86-171) gates on
    /// cannot ride the shape. This side table records the count the
    /// written capture carried at the shape's own close; a shape produced
    /// by a path that never spelled it (a declaration signature, an alias
    /// RHS folded through the pool) reads `positional.len()` — the
    /// all-required face, the under-report posture.
    pub(crate) fn_shape_required: FxHashMap<crate::model::FnShapeId, u16>,
    /// The most recently sealed written function signature's own
    /// `(FnShapeId, required)` pair — the per-site answer
    /// `fn_shape_required` cannot give when two spellings share one
    /// interned id (`g: int Function(int)` and `is int Function([int])`
    /// are the same `FnShapeId`, and the map answers one count for every
    /// face reading it). The `is` verdict's written target is the last
    /// signature the walk sealed, so a shape match here recovers ITS
    /// count even when the map already answered a different spelling's.
    pub(crate) last_fn_seal: Option<(crate::model::FnShapeId, u16)>,
    /// Explicit type arguments of pending calls, keyed by the callee's row
    /// index (pushed at the group's close, consumed by the argument check
    /// or the tear-off/implicit-creation faces).
    pub(crate) call_type_args: Vec<(u32, CallTypeArgs)>,
    /// The send-close upward inference's solved parameters, keyed by the
    /// callee row's range offset (stable while row indexes churn under the
    /// folds): a generic call's typed arguments unify against the callee's
    /// formals once at the argument group's close (`math.max(0, n - 1)`
    /// binds `T := int`), and every later return read
    /// ([`Engine::instantiate_call_return`]) adopts the solution — the
    /// oracle's call-site inference, replacing the free-parameter
    /// under-report stand-in.
    pub(crate) solved_call_params: FxHashMap<u32, Vec<(StringId, Ty)>>,
    /// The last completed send's resolved return beside its callee row's
    /// range offset (#2567 R18): `consume_send` computes the static type
    /// for its `Never` exit check; the listener stages the same answer
    /// into the flow recorder (`stage_call_result`) so the send fold —
    /// whose own callee read is an unknown face — adopts the real return
    /// (a switch over the call then answers the always-exhaustive
    /// verdict). Single slot, taken by the listener at the same event.
    pub(crate) send_return_staging: Option<(u32, Ty)>,
    /// The last completed send's genuinely-unresolved bare head beside its
    /// callee row's range offset (#2860): `consume_send` records the pair
    /// when the send's return reads `Unknown` AND the head resolves nowhere
    /// (the same universe the resolved back-end's `call_head_unresolved`
    /// certifies — no registered container, no non-container declaration,
    /// no invokable member, no core facade). The listener translates the
    /// slot into the flow recorder's `stage_call_result` as the head's own
    /// interned name — the flow lattice's `InvalidType` stand-in (a
    /// nowhere-resolving spelling maps to the invalid face, every relation
    /// over it vacuous) — so a switch whose scrutinee is the call answers
    /// the vacuous-cover chain instead of the undecidable unknown. Single
    /// slot, taken by the listener at the same event. A raw `Unknown`
    /// return from an SDK-static or local modeling gap never takes the
    /// slot — the over-deading guard keeps those on the unknown face.
    pub(crate) send_invalid_staging: Option<(u32, StringId)>,
    /// The invokable-member spelling set (`Engine::invokable_member_spelling`'s
    /// verdict cache): the core member tables' spellings plus this file's own
    /// container members, interned once on the first unresolved-head send —
    /// the same name-keyed over-approximation the snapshot's invokable face
    /// carries, rebuilt from the faces this engine already holds. Empty for
    /// a run whose sends all resolve.
    pub(crate) member_spellings: std::cell::OnceCell<FxHashSet<StringId>>,
    /// The numeric-call refinement's return (`e1.clamp(e2, e3)`,
    /// `e1.remainder(e2)`), keyed by the callee row's range offset —
    /// computed at the argument group's close where the typed arguments
    /// are live, adopted by [`Engine::callee_return_ty`]
    /// (`_refineNumericInvocationTypeNullSafe`, type_system.dart:2058-
    /// 2165: `int` when every operand reads `int?`, `double` when all
    /// read `double?`, the declared return otherwise).
    pub(crate) refined_call_returns: FxHashMap<u32, Ty>,
    /// The open call-site type-argument capture (the written member types
    /// of one `<...>` group at an invocation).
    pub(crate) call_args_capture: Option<CallArgsCapture>,
    /// The solved static type of a generic creation's send, keyed by the
    /// callee row: the fold below replaces that row (its type lost), so
    /// the instantiation rides here and lands on the merged row.
    pub(crate) pending_send_ty: Option<(usize, Ty)>,
    /// #1314 FAM10: the executable header's declaring name
    /// (`TopLevelFunctionDeclaration`) — its type-variable window pairs
    /// with it at the window's close; cleared when the formal list opens
    /// without one.
    pub(crate) pending_exec_name: Option<(StringId, u32)>,
    /// The constructor declaration in flight carries the `const` modifier
    /// (its initializer list is a const context; bodies are not).
    pub(crate) in_const_ctor: bool,
    /// The constructor declaration in flight is an `augment` (the
    /// site-consumed latch — `note_decl_site` takes it): the
    /// `duplicate_constructor` pass skips augment sites (the merge rule
    /// folds them into the introductory element — no second declaration),
    /// and `hasGenerativeConstructor` reads origin declarations only.
    pub(crate) ctor_augment_pending: bool,
    /// The open constructor-initializer list depth (`super.named()` in an
    /// initializer is the `undefined_constructor_in_initializer` shape).
    /// The depth arms the F-C routing: bare and `this.`-selector rows
    /// pushed inside the list resolve through the dedicated
    /// field-initializer face (`init_rows`), the plain assignment check
    /// suppresses, and folded `super.` selectors await their shape in
    /// `pending_init_super`.
    pub(crate) initializer_depth: u32,
    /// F-C: the offsets of rows pushed while the constructor-initializer
    /// list was open (bare names and `this.` selectors). The pass-2
    /// dispatch folds them instead of the ordinary faces — the oracle
    /// resolves this list through the dedicated initializer world
    /// (`element_resolver.dart`'s `ConstructorFieldInitializer` /
    /// `RedirectingConstructorInvocation` visits) and never reports the
    /// member trio or `undefined_identifier` there.
    pub(crate) init_rows: FxHashMap<u32, StringId>,
    /// The offsets of bare rows pushed inside an `extension` body whose
    /// `on` type is a resolvable nullable named type (`extension E on
    /// A?`) AND whose name resolved through the `on` face's member scope
    /// — the `RefTarget::Member { container }` the `on_face_walk` arm
    /// published, or a `Missing` row whose WRITE side (`name=` spelling)
    /// the `on` face declares. Pass 2 runs after the container frames
    /// tear down, so the implicit-`this`-on-nullable receiver fact rides
    /// this latch: the bare-send arms of `resolve_bare_row` emit the
    /// shared `unchecked_use_of_nullable_value` at the name before the
    /// member bookkeeping (#2926 F2 — upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/resolver/type_property_resolver.dart:
    /// 96-148, the `isNullable` tail preceding every member resolution
    /// on the extended type; `ThisLookup`'s bare-name arm).
    pub(crate) ext_nullable_member_rows: FxHashSet<u32>,
    /// The offsets of bare rows pushed inside an `extension` body whose
    /// WRITE side (`name=` spelling) resolved on the extension's own
    /// member map or the `on` face's member scope at push time — the
    /// lexical `ExtensionMemberScope` hit or the implicit-`this`
    /// `ThisLookup` hit upstream's `resolveSimpleIdentifier` records.
    /// Pass 2 cannot re-walk either scope (the frames are gone), so the
    /// `Missing` write arm reads the union for `undefined_identifier`
    /// suppression, and the nullable-`on` unchecked emit stands down when
    /// the hit was the extension's own member (a lexical resolution never
    /// consults `this` — `bar = 0` inside `extension E on A?` where `E`
    /// declares `set bar` is silent, not the nullable receiver's
    /// question).
    pub(crate) ext_write_target_rows: FxHashSet<u32>,
    /// The offsets of bare rows pushed inside an instance field's
    /// declaration initializer whose name resolved to an INSTANCE member
    /// of the enclosing container (the rows that ride `this`) — statics
    /// and non-members never do. Snapshotted at push time
    /// (`in_instance_field_initializer` is live only then — pass 2 runs
    /// after the field group tears down); the implicit-this face consults
    /// it at resolve time. Qualified member rows never enter: the oracle
    /// runs the face from the unqualified nodes only.
    pub(crate) field_init_this_rows: FxHashSet<u32>,
    /// The frame types the current `if` condition's pattern variables
    /// shadowed (name, previous type) — the statement's case scope
    /// ([`super::pattern_scope`]).
    pub(crate) pattern_shadows: Vec<(StringId, Option<Ty>, bool)>,
    /// The open `if` statements' case scopes, innermost last; restored
    /// when the then statement ends.
    pub(crate) if_case_scopes: Vec<Vec<(StringId, Option<Ty>, bool)>>,
    /// The current `if` condition's shadowing binders (their `locals`
    /// indices), and the open statements' twins: a binder that shadowed an
    /// outer tracked local retires when its case scope closes.
    pub(crate) pattern_shadow_binders: Vec<(usize, u32)>,
    pub(crate) if_case_binders: Vec<Vec<(usize, u32)>>,
    /// The open switch members' case scopes, innermost last (the arm
    /// counterpart of [`Self::if_case_scopes`]): a `case Sub x:` binder
    /// shadows only inside its own member — later members' reads of the
    /// scrutinee spelling resolve on the outer declaration again.
    pub(crate) switch_arm_scopes: Vec<SwitchArmScope>,
    /// F-C: the folded `super.`-selector candidates of the open list
    /// ([`PendingInitSuper`]); drained at `end_initializers`, where the
    /// invocation shape decides
    /// `undefined_constructor_in_initializer`.
    pub(crate) pending_init_super: Vec<PendingInitSuper>,
    /// F-C: the head token's offset of the open initializer item
    /// (`begin_initializer`'s `head`). A `this`/`super` AT that offset is
    /// the keyword head of a constructor-initializer construct (the field
    /// initializer, the redirecting invocation, the super-constructor
    /// invocation — upstream's `buildInitializer` folds them into
    /// `ConstructorFieldInitializer`/`RedirectingConstructorInvocation`/
    /// `SuperConstructorInvocation`, where the keyword is a bare token, not
    /// a `ThisExpression`/`SuperReference`), so the invalid-context faces
    /// never fire on it. `this`/`super` deeper in the item (an
    /// initializer's RHS) keeps the expression face.
    pub(crate) init_item_head: Option<u32>,
    /// F-C: the redirecting invocations of this file (`this(...)` shapes in
    /// constructor-initializer lists) as `(enclosing constructor → target)`
    /// edges — the `recursive_constructor_redirect` cycle walk's input
    /// (`_checkForRecursiveConstructorRedirect` reads the element model's
    /// `isInRedirectingConstructorCycle`; the per-file edge list is the M1
    /// stand-in, walked once at the pass end).
    pub(crate) redirect_edges: Vec<RedirectEdge>,
    /// The `@` token of the open metadata entry (the
    /// `undefined_annotation` anchor spans `@` through the name).
    pub(crate) annotation_at: Option<SourceRange>,
    /// The representation checks gate on the primary-constructors feature
    /// arm (`error_verifier.dart:5425` branches on
    /// `Feature.primary_constructors`; the expected-representation-type row
    /// lives in the disabled arm only).
    pub(crate) primary_constructors_enabled: bool,
    /// `isAugmentationsFeatureEnabled`: the `augment` declaration family.
    /// `_checkForFactoryBodyCompleteness` gates on it
    /// (error_verifier.dart:5079-5081).
    pub(crate) augmentations_enabled: bool,
    /// The `variance` experiment grant (`FeatureSet::is_enabled(Feature::
    /// Variance)`): the #612 capture runs regardless of it (#742 — the
    /// legacy superinterface face answers in the default posture), so the
    /// grant now scopes only the explicit-keyword faces.
    pub(crate) variance_enabled: bool,
    /// The field-promotion grant (`inference-update-2`, the unit's settled
    /// language version ≥ 3.2): AND-ed into promotion eligibility (the
    /// official `typeAnalyzerOptions.fieldPromotionEnabled`,
    /// `type_analyzer_options.dart:17`, built from the *accessing*
    /// library's feature set, `ast_resolver.dart:139-140` — pre-3.2
    /// libraries get no property promotion at all).
    pub(crate) field_promotion_enabled: bool,
    /// The patterns grant (`FeatureSet::is_enabled(Feature::Patterns)`): a
    /// `< 3.0` toolchain parses a switch-statement `case` head as a plain
    /// expression — its const requirement reports
    /// `non_constant_case_expression` instead of the pattern face.
    pub(crate) patterns_enabled: bool,
    /// The `triple-shift` grant (`FeatureSet::is_enabled(Feature::
    /// TripleShift)`, `enabledIn: 2.14`): under a pre-2.14 library a
    /// recovery-shaped `operator >>>` declaration contributes NO
    /// consultable member — the use site reports `undefined_operator`
    /// even with the declaration present (the oracle's element model
    /// leaves the split declaration unnamed; probe 3.13.7 `@dart=2.12`
    /// `operator >>>` + use: `unused_element` names it `<unnamed>` and the
    /// use reports the miss).
    pub(crate) triple_shift_enabled: bool,
    /// The open type-variables window's captured parameters `(name,
    /// declared keyword, name range)` — the container-begin event adopts
    /// them ([`Engine::note_container_type_variables`]).
    pub(crate) var_tv_window: Vec<(StringId, Option<Variance>, SourceRange)>,
    /// The `begin_type_variable` names of the open group, in declaration
    /// order: the Simple1 fast path fires `end_type_variable` with the
    /// group's closer token (the official event shape), so the completing
    /// end pairs with its name through this stack (LIFO — the general path
    /// completes variables in reverse declaration order).
    pub(crate) var_name_stack: Vec<(StringId, SourceRange)>,
    /// The written type arguments of the argument group that just closed on
    /// its head `(head name, head offset, arguments as written)`: the
    /// completing `handle_type` claims the entry when it re-fires that head
    /// and interns the arguments into the armed head's own [`ArgListId`]
    /// slot (#611), so the declaration that binds next carries the written
    /// shape inside its `Ty`. A non-`Named` armed head (`dynamic<int>`
    /// recovery residue) drops the arguments — suppression, never a
    /// fabricated shape.
    pub(crate) completed_group: Option<(StringId, u32, Vec<Ty>)>,
    /// The open container-header capture ([`VarHeader`]); `None` outside
    /// container headers and whenever the experiment is off.
    pub(crate) var_header: Option<VarHeader>,
    /// The finished header captures, in declaration order (keep-first by
    /// name — a duplicate header is the `duplicate_definition` face's
    /// business, but its clause trees still write through).
    pub(crate) var_containers: Vec<VarContainer>,
    /// The hop-namespace memo (`(file, probed name)` → the owning library):
    /// #491 S2a's reverse lookup, cached per probe.
    pub(crate) owner_libs: RefCell<FxHashMap<(FileId, StringId), Option<LibraryId>>>,
    /// The member-face walk memo (`(container, member, super-climb)` →
    /// outcome): the miss chains re-probe the same face several times per
    /// row (#438), so every probe after the first is one hash lookup. See
    /// [`FaceMemo`] for the soundness argument.
    pub(crate) member_faces: FaceMemo<'a>,
    /// Extension types whose representation checks already ran this pass
    /// (one emission per container: an augmentation re-fires the event).
    pub(crate) reported_extension_types: FxHashSet<StringId>,
    /// Containers whose face-inheritance checks (#498) already ran this
    /// pass (one emission per container: an augmentation re-fires the
    /// event).
    pub(crate) reported_face_checks: FxHashSet<StringId>,
    /// Containers whose class-modifier restriction checks (#600) already
    /// ran this pass (one emission per container: an augmentation re-fires
    /// the event).
    pub(crate) reported_modifier_checks: FxHashSet<StringId>,
    /// Containers whose `@sealed` supertype contract already ran this pass:
    /// the same one-emission-per-container contract as
    /// [`Self::reported_modifier_checks`] for its own finalize face.
    pub(crate) reported_sealed_checks: FxHashSet<StringId>,
    /// The open container header's deferred class-modifier restriction
    /// checks (#1289 E1): armed at the declaration's name event — where the
    /// official drives mount, but the supertype entries have not streamed
    /// yet — and finalized in two halves. The use-site families emit at the
    /// body open (their clause-entry spans are complete by then); the
    /// `@sealed` contract emits at the declaration's end event (the
    /// official anchor is the whole declaring node, best known at its
    /// close). Capture state and finalize halves on
    /// [`DeferredModifierChecks`].
    pub(crate) deferred_modifier: Option<DeferredModifierChecks>,
    /// The clause-entry spans of every completed container header, keyed by
    /// the container's name — the `dart:ffi` verifier's post-parse pass
    /// ([`super::ffi`]) anchors its `subtype_of_struct_class` rows on the
    /// written extends/implements/with entries, which the element model
    /// keeps name-only. Captured at the use-site drain
    /// ([`Engine::check_modifier_use_sites`]) where the spans are complete.
    pub(crate) ffi_clause_spans: FxHashMap<StringId, FfiClauseSpans>,
    /// The FFI invocation candidates captured at each argument-list close
    /// ([`super::ffi`]'s invocation pass input).
    pub(crate) ffi_invocations: Vec<FfiInvocation>,
    /// The `.address` member reads in source order (the
    /// `address_position`/`address_receiver` input).
    pub(crate) ffi_address_reads: Vec<FfiAddressRead>,
    /// The FFI-named metadata annotations of the annotation currently
    /// being closed, awaiting the declaration they annotate (drained at
    /// [`Engine::note_decl_site`]).
    pub(crate) ffi_pending_annos: Vec<FfiDeclAnno>,
    /// The FFI-named metadata annotations by declaration site (the
    /// `note_decl_site` index): the `@Native`/`@Array`/`@Packed`/
    /// `@AbiSpecificIntegerMapping`/`@DefaultAsset` faces the `dart:ffi`
    /// verifier's post-parse pass reads, source-parsed once at the
    /// annotation's close.
    pub(crate) ffi_decl_annos: FxHashMap<usize, Vec<FfiDeclAnno>>,
    /// The #279 constant-evaluation capture: expression folding while a
    /// const context is open ([`crate::const_eval`]).
    pub(crate) const_capture: ConstCapture,
    /// Const-value scopes, innermost last; scope 0 is the file scope and
    /// never pops. Const locals/fields bind their evaluated value here as
    /// they close (file order).
    pub(crate) const_scopes: Vec<FxHashMap<StringId, ConstBinding>>,
    /// The open declaration group is `const` (locals or fields) — its
    /// initializer expressions are const contexts.
    pub(crate) const_group: bool,
    /// A member field group is open (`begin_fields` … `end_fields`): the
    /// group's initializer windows close on the field face
    /// ([`Engine::finish_const_initializer`]'s `local: false`), never the
    /// local face. Top-level groups leave this false — their initializers
    /// route by the file-scope test at the close site instead.
    pub(crate) field_group_open: bool,
    /// The frame-stack depth when the field group opened (the container
    /// frame's depth): a variable initializer that began deeper — a
    /// closure-local inside a field initializer's closure — closes on the
    /// local face even while the group is open.
    pub(crate) field_group_depth: usize,
    /// The frame-stack depth when the current initializer began: a deeper
    /// stack at close means a function frame opened inside (a closure's
    /// own locals) — those close on the local face regardless (`#511`'s
    /// misroute guard).
    pub(crate) init_frame_depth: usize,
    /// Whether an initializer is open (`begin_variable_initializer` /
    /// `end_variable_initializer`): the as-cast flow-tail check keys on
    /// this besides [`Self::init_frame_depth`], whose last value would
    /// otherwise read stale outside initializers.
    pub(crate) in_initializer: bool,
    /// Open conditional-expression depth (`begin_conditional_expression` /
    /// `end_conditional_expression`): an `as` cast inside a `?:` arm gets
    /// its dead-code row from the flow layer's `cast_tail_row` — the
    /// official `flowEnd(thenExpression)`/`flowEnd(elseExpression)` close
    /// (resolver.dart:2458/2484) reports `[type.offset, arm.end)` — so the
    /// initializer face [`Self::in_initializer`] must defer to it.
    pub(crate) cond_expr_depth: u32,
    /// The const group's declared names, in initializer order. A name
    /// whose slot the cursor never consumes owns a `const_not_initialized`
    /// row at its name token.
    pub(crate) const_group_names: Vec<StringId>,
    /// The next const-group name an initializer window consumes.
    pub(crate) const_group_cursor: usize,
    /// Top-level variables this pass saw declared *without* `const` (the
    /// provable side of the identifier classification; use-before-decl
    /// stays suppressed).
    pub(crate) nonconst_top_level: FxHashSet<StringId>,
    /// Static const field values captured this pass, keyed
    /// `(container, member)` (file order: uses after the declaration).
    pub(crate) const_field_values: FxHashMap<(StringId, StringId), ConstantValue>,
    /// `const_with_non_const` verdicts awaiting the file-tail finalize —
    /// `(target container, const-keyword span)` per `end_const_expression`;
    /// staged so they merge source-ordered with the other deferred const
    /// faces.
    pub(crate) pending_const_non_const: Vec<(StringId, SourceRange)>,
    /// One const-declaration initializer failure candidate per creation or
    /// checked argument seen inside the open window (innermost
    /// initializer's entries in source order): the whole-file
    /// const-constructor set is incomplete mid-parse, so the verdict
    /// defers to the tail pass.
    pub(crate) open_const_creations: Vec<(StringId, SourceRange, ConstCreationEntry)>,
    /// The open event's source-spelled bare head when the row transport
    /// dropped the callee row outright (the engine's creation posture for
    /// targets whose unnamed resolution fails): the close gate re-asks the
    /// unnamed-face question from the spelling.
    pub(crate) pending_bare_creation: Option<(StringId, SourceRange)>,
    /// Entries promoted at the initializer close (the verdict passed
    /// silently): the tail pass re-decides each against the complete
    /// const-ctor set — the first invalid entry per initializer anchors
    /// `const_initialized_with_non_constant_value`, every invalid creation
    /// with an existing non-const constructor adds the
    /// `const_with_non_const` twin.
    pub(crate) pending_const_creations: Vec<(StringId, SourceRange, ConstCreationEntry)>,
    /// The interned `identical` spelling (the one invocable const call).
    pub(crate) identical_id: StringId,
    /// Names of the const initializer windows currently open (innermost
    /// last) — the binding key at close.
    pub(crate) pending_init_names: Vec<StringId>,
    /// The arg-check gate per open const-creation window (enum element /
    /// explicit const ctor), innermost last.
    pub(crate) arg_gates: Vec<bool>,
    /// A `var`/`final`-inferred local declarator awaiting its
    /// initializer's close: the resolved expression type binds as the
    /// local's static type (the `var self = this` face — upstream's
    /// inferred `this` type decides `self is T` verdicts) and stages as
    /// the flow layer's declared type (#2140), keyed on the declarator
    /// name token.
    pub(crate) var_infer_pending: Option<(StringId, SourceRange)>,
    /// The open variable initializers' own inferred declarators, innermost
    /// last: `begin_variable_initializer` moves the latched declarator
    /// here so a declaration nested in the initializer (a for-element's
    /// loop variable, a closure's local) cannot re-latch the slot before
    /// the outer initializer closes — the close binds ITS declarator.
    pub(crate) init_declarators: Vec<Option<(StringId, SourceRange, VarInferSlot)>>,
    /// The staged assignment RHS types by write-target name (#2174's
    /// inference feed): the latest plain `=` write whose resolved RHS the
    /// flow staged, as `(write offset, executable serial, resolved type)`.
    /// A `var`-inferred declarator whose initializer is a bare read of that
    /// name infers the staged type — the oracle's inference is flow-aware
    /// (the promoted read type), and the declared slot the loop-head join
    /// demotes to must carry it (`var max = end` after
    /// `end = RangeError.checkValidRange(...)` infers `int`, not `int?`).
    /// The executable serial keeps the feed inside the executable that
    /// wrote it (#2460 A2c) — two executables at the same frame depth
    /// never share a face.
    pub(crate) staged_write_tys: FxHashMap<StringId, (u32, usize, Ty)>,
    /// Statement-block depth inside the innermost executable (0 at the
    /// function body's own top level — the body block rides its own event).
    /// A write staged at depth > 0 sits on a conditional arm: the variable's
    /// type at any later join is the arms' join, never this arm's write, so
    /// the [`Engine::staged_write_tys`] feed must not claim it (the entry
    /// drops and the static face decides).
    pub(crate) stmt_block_depth: u32,
    /// The next executable serial handed to a pushed [`Frame`]
    /// ([`Frame::serial`]).
    pub(crate) next_exec_serial: usize,
    /// An enum-constant window is open (`handle_enum_element` doubles as
    /// the previous element's close).
    pub(crate) enum_element_open: bool,
    /// Begin spans of open string literals (the `end_literal_string`
    /// event carries only the end token).
    pub(crate) const_string_begins: Vec<SourceRange>,
    // —— #1351: the const constructor's initializer walk ——————————————————
    /// The open const-constructor initializer capture: its container and
    /// the constructor's lookup spelling (the same key the creation-side
    /// walk reads).
    pub(crate) ctor_init_latch: Option<(StringId, StringId)>,
    /// Field-initializer RHS trees captured under the open latch, one per
    /// `=` (the assignment merge folds them out of the capture window).
    pub(crate) ctor_init_pending: Vec<crate::const_eval::Node>,
    /// Captured const-constructor initializer trees by (container, ctor
    /// spelling); the unnamed face keys on the container's own name.
    pub(crate) ctor_inits: FxHashMap<(StringId, StringId), Vec<crate::const_eval::Node>>,
    /// The passing constant-pattern candidates (#1351): the parse-time
    /// check ran against the declared scrutinee; the flow's promoted
    /// matched type re-decides at the teardown join.
    pub(crate) pending_matched: Vec<PendingMatched>,
    pub(crate) pending_switch_coverings: Vec<PendingSwitchCovering>,
    /// Inferred locals whose initializer is a promotion-latched member
    /// read (#2539): the parse-time bind wrote the accessor's return over
    /// the UNPROMOTED declared receiver, and the promotion re-decision —
    /// a pass-2 face — cannot reach the bind. Each row defers the re-bind
    /// (the declaring local's NAME-TOKEN offset — file-unique, unlike the
    /// scoped locals-stack index — the member row's offset, the parse-time
    /// type); pass 2 recomputes the accessor over the re-derived receiver
    /// and re-binds before any row resolves.
    pub(crate) deferred_var_rebinds: Vec<(u32, u32, Ty)>,
    /// The bare-identifier initializer re-binds (#2567): the declaring
    /// local's name-token offset, the initializer read's span, and the
    /// parse-time bound face — the pass-2 flow read type re-binds it.
    pub(crate) deferred_bare_rebinds: Vec<(u32, SourceRange, Ty)>,
    /// An implicitly typed closure formal whose context schema derived
    /// from a staged receiver row (`rebound.fragments.where((f) => …)`'s
    /// `f`): pass 1 freezes the schema entry over the unre-derived
    /// receiver face, so the ingredients defer and the re-bind pass
    /// recomputes the schema over the re-derived link face.
    pub(crate) deferred_closure_formals: Vec<DeferredClosureFormal>,
    /// Deferred invocation faces (#2547): a bare-local callee the
    /// parse-time pass typed as non-callable may stand flow-promoted at
    /// the send (a switch-case pattern promoted the local to a `call`
    /// class) — the read-type fact drains only at pass 2, so the
    /// `invocation_of_non_function` row defers and re-decides on the
    /// flow face there.
    pub(crate) deferred_invocation_faces: Vec<(SourceRange, StringId)>,
    /// The flow read rows whose promoted face is the signature-less
    /// `Function` head (`FlowBase::Function`), keyed by the read's
    /// offset. The signature-less head has no semantic `Ty` spelling (the
    /// shape needs an interned `FnShapeId` the flow row cannot carry —
    /// the read-type row itself degrades to `Unknown`), so the deferred
    /// invocation face consults this set: a read the flow promoted to a
    /// function form IS callable
    /// (function_expression_invocation_resolver.dart:67-79 — the
    /// promoted static type's `FunctionTypeImpl` answer).
    pub(crate) promotion_function_reads: FxHashSet<u32>,
    /// The resolved function faces of un-negated `is` tests over a
    /// local's read (#3593) — the SIGNATURE twin of the flow layer's
    /// function-form promotion: the flow lattice spells every function
    /// type `Base::Function` (the signature dropped at the event seam),
    /// so a promoted read's fact can name only the form, never the face
    /// it promoted TO. Each entry carries the test operand's read
    /// offset, the operand's spelling, and the checked side's fully
    /// resolved `Ty::Function` ([`Engine::check_type_check`]'s record);
    /// the pass-2 re-decide of the if-null composite join consults it
    /// ([`Engine::redecide_promoted_if_null_faces`]). Appended in source
    /// order; the insert gate mirrors the promotion chain's own step
    /// decision, and a negated test retires the name's entries (its
    /// factored remainder is undecidable at the seam).
    pub(crate) fn_is_promotions: Vec<(u32, StringId, Ty)>,
    /// Implicit-call initializers (`var f = promotedLocal(args)`): the
    /// callee's promoted read re-decides the send's result in pass 2 —
    /// the entry keys the declaring local's name-token offset, the
    /// callee's own row span, and the parse-time face.
    pub(crate) deferred_call_rebinds: Vec<(u32, SourceRange, Ty)>,
    /// Composite-initializer re-binds (#4265): a `?:`/`??`/binary fold's
    /// `Value` row binds the inferred local at parse time, but the fold's
    /// arm `read_type_at` consult starved (`read_types` fills only after
    /// `parse_unit`), so a promotion standing inside an arm (the
    /// `x is T ? x : null` shape) leaves the bound face at the declared
    /// LUB. The entry keys the declaring local's name-token offset, the
    /// composite node's span (the `Value` row's range — the
    /// `PendingCompositeFold` join key), and the parse-time face; pass 2
    /// reads the drained [`Self::composite_rewrites`] and re-binds under
    /// the assignability gate.
    pub(crate) deferred_composite_rebinds: Vec<(u32, SourceRange, Ty)>,
    /// The composite folds' recomputed result types (#3980), drained once
    /// `read_types` fills and before `resolve_rows`: `(node_range → ty)`
    /// offset-sorted, holding only the records whose recompute differed
    /// from the fold-time answer. `filter_staged`'s staged-check `from`
    /// ladder and [`Self::deferred_composite_rebinds`]' pass-2 re-bind
    /// both consult it — the single drain keeps the two consumers on one
    /// answer.
    pub(crate) composite_rewrites: Vec<(SourceRange, Ty)>,
    /// The names a deferred re-bind actually retyped (#2539): the
    /// receiver re-decision consults the frame's var-types for these
    /// names when the flow read face carries no promotion — the local's
    /// re-bound type is the receiver's re-derived static type. Keyed by
    /// the declaring local's NAME-TOKEN offset: the locals stack is
    /// scoped (truncated as frames close), so a stack index aliases
    /// across scopes and a flat index key would let one scope's re-bind
    /// clobber another's.
    pub(crate) rebound_locals: FxHashMap<u32, (Ty, Ty)>,
    /// The current case clause's `case` keyword anchor (the flow
    /// `MatchedType` fact's join key); a switch-expression alternative
    /// clears it (the parser delivers no keyword face there).
    pub(crate) clause_anchor: Option<SourceRange>,
    // —— #504: the usage cluster ———————————————————————————————————————————
    /// Flat stack of the open frames' local declarations; frame `i` owns
    /// `[frame_local_marks[i], len)` (the #504 usage cluster's declaration
    /// side, `unused_local_elements_verifier.dart`).
    pub(crate) locals: Vec<LocalDecl>,
    /// Per open frame, the locals-stack length at frame open.
    pub(crate) frame_local_marks: Vec<usize>,
    /// Local-referencing rows: the row's range offset → the local
    /// declaration the bare read resolved to. Consumed by the write-only /
    /// useless-read cancellations and the receiver merge (the row stack
    /// shifts, so rows carry no back-pointer).
    pub(crate) local_hits: FxHashMap<u32, u32>,
    /// The non-moving twin of [`Engine::local_hits`]: each bare read's
    /// offset to its resolved local's DECLARATION name-token offset,
    /// never relocated by the merge folds. The value is the declaration's
    /// own span offset (file-unique), not the scoped locals-stack index —
    /// the stack truncates as frames close, so an index read in pass 2
    /// aliases across scopes. The fold records the twin at the member
    /// row's offset too (the merge's hit move), so identity questions key
    /// on the receiver read's own span here and on the member row there.
    pub(crate) local_decl_at_read: FxHashMap<u32, u32>,
    /// Bare reads whose push-time scope lookup landed on a local, a type
    /// parameter, or a member of the enclosing container: a same-spelled
    /// top-level declaration is shadowed there, so its faces (the
    /// `@useResult` discard check, the tear-off signature) never apply.
    pub(crate) shadowed_bare_rows: FxHashSet<u32>,
    /// The flow-blind locals' initializer rows: each UNANNOTATED local
    /// declaration's name-token offset to its initializer expression's row
    /// offset (`for_in` marks a loop variable whose type is the
    /// initializer's ELEMENT, not the iterable). The pass-2 pre-pass
    /// derives the semantic face (`init_derived_types`) for the receiver
    /// faces the flow lattice cannot spell — member returns, abstract
    /// getters through the on-face walk, parameterized iterables.
    pub(crate) local_init_rows: FxHashMap<u32, (u32, bool)>,
    /// The pass-2 derived initializer faces, keyed by the declaring
    /// local's name-token offset. `RefCell`: the pre-pass STAGES as it
    /// walks (source order is the topological order — an initializer
    /// reading an earlier flow-blind local sees its derived face through
    /// the same table the receiver faces read), so the immutable
    /// expression walk and the staging interleave.
    pub(crate) init_derived_types: std::cell::RefCell<FxHashMap<u32, Ty>>,
    /// The typed closure-literal faces the parse staged, keyed by the
    /// literal's own offset (its `(`): `resolve_rows` takes `rows` into
    /// its walk-local slice, so the `&self` re-derivations
    /// (`closure_init_ty`) cannot read the rows there — this side table
    /// carries the face instead (`var g = () => 0` binds `int
    /// Function()`, the oracle's `FunctionExpressionImpl.staticType`).
    pub(crate) closure_lit_rows: FxHashMap<u32, Ty>,
    /// The flow-blind locals whose initializer types as the oracle's
    /// `InvalidType` — a member chain rooted in a receiver whose head
    /// resolves to no declaration (`var at = node.operator.offset` with
    /// `operator`'s type undefined). Keyed like [`Self::init_derived_types`];
    /// the numeric refinement reads such a local as an invalid operand.
    pub(crate) invalid_init_locals: std::cell::RefCell<FxHashSet<u32>>,
    /// The fold's twin of [`Engine::local_decl_at_read`]: each member
    /// row's offset to the receiver read's local declaration offset. The
    /// receiver re-bind fallback keys it (the receiver read's own span is
    /// the primary entry's key); the receiver-expression faces must NOT
    /// consult it — a member row's span names the LINK node, whose face
    /// the chain leg re-derives, never the receiver's own local.
    pub(crate) local_decl_at_member: FxHashMap<u32, u32>,
    /// The open pattern-variable-declaration group window: the group id
    /// every pattern-declared variable joins until the window closes (one
    /// used member spares the whole group; official
    /// `visitPatternVariableDeclaration`, `unused_local_elements_verifier`.
    /// dart:856-884). The window opens at the first declared variable after
    /// a boundary and closes at any statement boundary, case head,
    /// intervening non-pattern local, or frame pop — a group can never span
    /// frames, so the id never outlives the locals that hold its members.
    /// (An earlier index-keyed window survived frame pops and read a
    /// truncated locals slot.)
    pub(crate) pattern_group_open: Option<u32>,
    /// The pattern-group sequence (group ids).
    pub(crate) pattern_group_seq: u32,
    /// The untyped bare variables the in-flight pattern declaration bound,
    /// in field order (`final (a, b) = …`): [`Engine::destructure_record`]
    /// distributes the scrutinee's positional record slots over them when
    /// the whole pattern is one flat record. A variable already carrying a
    /// written type (`final (int a, b)`) never enters the list — its own
    /// bind runs at declaration.
    pub(crate) pattern_bind_vars: Vec<(StringId, SourceRange)>,
    /// The record-pattern closes observed inside the in-flight pattern
    /// declaration (a flat `(a, b)` closes exactly once; nesting closes
    /// more). Any other shape marks the window non-record.
    pub(crate) pattern_bind_records: u16,
    /// Non-flat-record evidence inside the in-flight pattern declaration:
    /// a named field (`:a`/`a: p`), an object/list/map/parenthesized/
    /// rest/cast/relational pattern, or a typed leaf — anything whose
    /// field-to-variable map isn't the flat positional walk.
    pub(crate) pattern_bind_dirty: bool,
    /// The last declared/assigned variable-pattern name token: an
    /// object-pattern shorthand field (`:var x`) reads the getter the
    /// variable names (the oracle's `field.pattern.variablePattern?.name`).
    pub(crate) last_pattern_variable: Option<(StringId, SourceRange)>,
    /// A null-check pattern (`p?`) completed since the last pattern-field
    /// boundary over the variable it wrapped: the object-pattern shorthand
    /// (`:final x?`) binds `x` to the property type's non-null projection.
    /// The latch carries the wrapped variable's name so a stale latch from
    /// another sub-pattern never projects an unrelated field's binder.
    pub(crate) pending_pattern_null_check: Option<StringId>,
    /// The most recent object-pattern head inside a case/if-case pattern
    /// window (`AsyncData()`): the head types a type-free binder on the
    /// RIGHT operand of a logical-and (`AsyncData() && final transformer`
    /// binds `transformer` to `AsyncData` — the oracle's matched-type
    /// binding for the and-pattern's variable). Latched at the object
    /// pattern's close, consumed by the first type-free declared-variable
    /// pattern in a binary-and frame.
    pub(crate) last_case_object_head: Option<StringId>,
    /// Type-free variable binders a null-check pattern wrapped
    /// (`case final transformer?` over a member-chain scrutinee): their
    /// face is the matched type's non-null projection — the scrutinee's
    /// own static type — applied at the root pattern's close unless the
    /// object-pattern field path claims the binder first (`:final x?`
    /// binds from the property face, never the scrutinee).
    pub(crate) null_check_binder_candidates: Vec<(StringId, SourceRange)>,
    /// Object-pattern field reads awaiting the file-tail verdict
    /// (`resolveObjectPatternPropertyGet`, type_analyzer.dart:1669-1715):
    /// `(head type, getter, anchor)`.
    pub(crate) pending_pattern_getters: Vec<(StringId, StringId, SourceRange)>,
    /// The enclosing container's primary constructor formal names. Upstream
    /// scopes them narrowly — the `ConstructorInitializerScope` of the
    /// non-late INSTANCE field initializers (`scope_context.dart:432-461`)
    /// and the `PrimaryParameterScope` of the `this : ...` body
    /// (`:406-430`) — and never the member scope: a read from a STATIC or
    /// `late` field initializer, or from a method body, answers
    /// `undefined_identifier` (oracle 3.13.7). The parser/engine registers
    /// these formals into the container frame's locals (the body needs
    /// them), so the set marks the names that must not answer there; it is
    /// cleared with the container frame.
    pub(crate) primary_formals: FxHashSet<StringId>,
    /// Whether the enclosing primary constructor's formals are OUT of scope
    /// at the current position — the DEFAULT posture. Upstream scopes them
    /// from two planes alone
    /// (pkg/analyzer/lib/src/dart/resolver/scope_context.dart:406-461): the
    /// `ConstructorInitializerScope` of a non-static, non-`late` instance
    /// field initializer (`begin_fields` clears this for such a group) and
    /// the `PrimaryParameterScope` of the `this : ...` body
    /// (`begin_primary_constructor_body` clears it there). Every other
    /// member plane — a method or getter body, a static or `late` field
    /// initializer, a secondary constructor's initializer list — reads
    /// `undefined_identifier` (oracle 3.13.7), so the flag stays set.
    pub(crate) primary_formals_muted: bool,
    /// The parameters of the executable declaration in flight attach to
    /// this declaration site (set at the site-creating begin events,
    /// read by the formal-parameter identifiers).
    pub(crate) open_exec_site: Option<usize>,
    /// Declaration sites of this file, parse order (= source order): the
    /// library-level side of the usage cluster.
    pub(crate) decl_sites: Vec<DeclSite>,
    /// The `_checkForWrongVariancePosition` node spans of the member
    /// currently being parsed, opened when its `DeclKind::Member` site
    /// registers. `depth` is the declaring frame's `frames` index: the
    /// header's own type variables and formals fire while it is
    /// innermost, so a nested executable's deeper frame keeps its header
    /// events out of the member's slots.
    pub(crate) open_member_face: Option<MemberFaceSpans>,
    /// Completed member faces, consumed by `finalize_variance`'s member
    /// walk — the model carries no per-node spans, so the return
    /// annotation's, each formal's, and each own type parameter's node
    /// anchor ride this side table (a missing entry keeps the member
    /// name anchor, the under-report direction).
    pub(crate) member_face_spans: Vec<MemberFaceSpans>,
    /// The open field group's static bit (`begin_fields` arms it, the
    /// group's `FieldDeclaration` identifiers read it).
    /// Metadata of the declaration in flight (`MetadataReference`
    /// collects the name; `end_metadata` attaches the captured
    /// `UseResult.unless` face; a declaration site consumes the entries
    /// into its `annotated` exemption bit — any annotation exempts, the
    /// official pragma/JS gates' under-report superset — and its
    /// [`DeclSite::use_result`] face).
    pub(crate) pending_annotations: Vec<PendingAnnotation>,
    /// The discarded-value machine (the `_isUsed` parent-chain mirror,
    /// `use_result_verifier.dart`): completed UseResult-candidate
    /// expressions awaiting a sealing context event, keyed by their name
    /// token's offset. The registration points are the send/selector
    /// completions; the seals are the context events that answer the
    /// parent-chain walk (argument groups, literals, assignments, binary
    /// operands, cascade targets, statement ends).
    pub(crate) ur_pending: Vec<u32>,
    /// Offsets the machine sealed as used (`_isUsed` true): the value
    /// flows somewhere observable.
    pub(crate) ur_used: FxHashSet<u32>,
    /// Report-span overrides for the candidates whose official anchor is
    /// wider than the name token — the creation fold's whole-`A()` span
    /// (`nodeToAnnotate`, `use_result_verifier.dart`).
    pub(crate) ur_spans: FxHashMap<u32, SourceRange>,
    /// The creation folds (`A()` → `Receiver::Created`) with their head
    /// token offset, whole-invocation report span, and container: the
    /// unnamed-constructor candidates the resolve pass walks.
    pub(crate) ur_creations: Vec<(u32, SourceRange, StringId)>,
    /// The open for-parts windows: 1 = the statement form, 2 = the
    /// collection-element form. While a window tops the stack, an
    /// expression statement is the for-condition (the official
    /// `ForParts` condition arm — used); the updaters' leftovers discard
    /// at `handle_for_loop_parts`. Pops are type-conditional (each form's
    /// pop-sites are mutually exclusive) so nesting never steals an
    /// enclosing window.
    pub(crate) in_for_parts: Vec<u8>,
    /// UseResult-carrying declaration sites keyed `(container, spelling)`
    /// — `None` for top-level — built once at resolve time from the
    /// face-bearing sites; the resolve arms fold up against it.
    pub(crate) ur_sites: FxHashMap<(Option<StringId>, StringId), usize>,
    /// The `ur_pending` watermark at each open argument list (`begin_`
    /// arguments / `end_arguments` bracket): the list seals only the
    /// pendings registered INSIDE it used (the official `ArgumentList`
    /// parent arm) — the callee's own candidate registered before the
    /// bracket opens and stays pending for the enclosing context.
    pub(crate) ur_arg_base: Vec<usize>,
    /// Selector-row offsets merged at the `..` event (`f()..g()`): the
    /// official walk answers the section send itself
    /// (`CascadeExpression.cascadeSections`), so its discarded value
    /// reports at the selector even when the target's type is unknown to
    /// the model. Consulted only in the unknown-receiver resolve arm —
    /// known receivers reach the member faces anyway.
    pub(crate) cascade_sections: FxHashSet<u32>,
    /// Member-row offsets merged through a `?.`/`?..` operator: the
    /// null-aware access unwraps the receiver before the member resolves
    /// (`x?.m` accesses `m` on the non-null `C`, never on `C?`), so the
    /// extension-applicability nullability gate must read the receiver as
    /// non-nullable — a non-nullable `on C` face still applies.
    pub(crate) null_aware_members: FxHashSet<u32>,
    /// The DIRECT `?.`/`?..`-merged member rows (row offset → the merge's
    /// operator offset): a row here stored its member's declared face
    /// pre-lift (`row.ty`; the record arm's baked face lives in
    /// `record_short_fields`), so a chained null-aware operator reads the
    /// pre-lift face for its gate — upstream's null shorting strips the
    /// SHORT's appended null, never the member's own (`s?.length?.isEven`
    /// decides the second `?.` on `int`) — and the row's own operator is
    /// the `previousShortCircuitingOperator` answer (the variant's causing
    /// operator). A plain continuation (`.b` of `a?.b.c`) never joins, so
    /// its chain-lifted entry in `null_aware_members` (a different fact)
    /// stays out of this map.
    pub(crate) null_aware_member_ops: FxHashMap<u32, u32>,
    /// The cascade targets' read spans, pushed at each `..`/`?..` and
    /// popped at the cascade's close: every section selector of that
    /// cascade re-reads this span (`builder..a = x`'s `..a` reads
    /// `builder` exactly as a plain access does), so the promoted-receiver
    /// re-decision latch keys every section on it — the flow's promotion
    /// of the target read answers the receiver verdict.
    pub(crate) cascade_target_spans: Vec<SourceRange>,
    /// The parser reopens a cascade for each `..` section after the
    /// first one's assignment (`t..a = x..b = y` parses `..b` as a fresh
    /// cascade whose "target" row is `x`). The last closed cascade's
    /// `(operator offset, target span)` lets a directly following section
    /// inherit the real target span instead.
    pub(crate) cascade_last: Option<(u32, SourceRange)>,
    /// The open cascades' `(operator offset, target span)` pairs, moved to
    /// [`Self::cascade_last`] at their close.
    pub(crate) cascade_open: Vec<(u32, SourceRange)>,
    /// The last closed cascade's captured target type (the type twin of
    /// [`Self::cascade_last`]): a reopened cascade (`a..b = x..c = y` — one
    /// begin/end per section) sees the PREVIOUS SECTION'S RHS as the stack
    /// top, never the real target, so its begin reuses this stash for the
    /// value face instead of reading the leftover row.
    pub(crate) cascade_last_ty: Option<Ty>,
    /// The row-stack depth at each open cascade's begin — the target row
    /// is the entry below it. A cascade expression's static type is its
    /// TARGET's (`CascadeExpression`'s `staticType`), never the last
    /// section's value: at close, the section rows truncate back here so
    /// the target row stands for the whole expression (`(b..f).m` reads
    /// `.m` on `b`'s type, not the section's result).
    /// The captured `(static type, operator range)` of each open cascade's
    /// target row: a section's merge leaves the SECTION's result as the
    /// stack top, so the close pushes a value row carrying the target's
    /// type — the cascade expression's value face (`CascadeExpression`'s
    /// static type is the target's, never the last section's).
    pub(crate) cascade_target_tys: Vec<(Ty, SourceRange)>,
    /// The stack offsets of the cascade value rows [`Self::cascade_target_
    /// tys`] pushed at close: a directly-following `..` reopens the
    /// cascade (the parser fires one begin/end per section), and the
    /// reopened cascade's target is the row BELOW the value row — the
    /// begin pops its own value stand-in so the span latch and the
    /// section merges read the real target exactly as a fresh cascade
    /// would.
    pub(crate) cascade_value_rows: Vec<u32>,
    /// A null-aware RECORD-field read (`result?.read` on
    /// `({M read, …})?`): the record arm bakes the shorted `?` into the
    /// row's stored face (its consumers read the written nullable), so
    /// the field's own non-null face rides this map — the chain
    /// continuation's null-aware latch (`result?.read.icc`) reads it the
    /// way the class-member design reads the row's non-nullable type.
    pub(crate) record_short_fields: FxHashMap<u32, Ty>,
    /// A `?..` cascade whose FIRST section has not yet merged: the
    /// null-aware check's operator range (`f()?..[i]`'s index section
    /// parses as `handle_indexed_expression` with no `?` — the `?` rode
    /// the `?..` token, so the pending range stands in for it, #1758).
    /// Consumed by the first section's access event; `..`-cascades hold
    /// `None`.
    pub(crate) cascade_null_aware_op: Option<SourceRange>,
    /// Library-level reference index: every name some reference resolved
    /// to in this file (locals included — shadowing only ever suppresses).
    pub(crate) used_names: FxHashSet<StringId>,
    /// The `unused_import`-only credit set (`ImportsTracking`,
    /// scope.dart:228-339): every name that resolved through the namespace
    /// — including the positions `used_names` deliberately skips for the
    /// `unused_element` face (the `_insideIsExpression` exclusion is the
    /// used-elements visitor's, not the import tracker's — an `is`/`as`
    /// operand type still spares the import that provided it, and a
    /// self-scope reference still consulted the namespace). The face reads
    /// `used_names ∪ import_used`.
    pub(crate) import_used: FxHashSet<StringId>,
    /// Extension members some implicit application resolved to (the
    /// official `usedElements.members` face — an ELEMENT set keyed by the
    /// applied extension: `(declaring file, index of the face in that
    /// file's `extension_faces()`, member spelling)`). The element key is
    /// what keeps the oracle's `identical(element.library)` gate and the
    /// per-extension attribution: two same-spelled members of sibling
    /// extensions never share a credit, and an extension member declared
    /// in another library can join here without ever sparing the same
    /// spelling in this library's faces.
    pub(crate) extension_member_used: FxHashSet<(FileId, u32, StringId)>,
    /// The unit's `accessibleExtensions` face (scope.dart:473-488), built
    /// on first member-miss: every extension face declared in this library's
    /// units plus every extension the import edges' export scopes admit
    /// under their combinators (prefixed imports' faces included — the
    /// oracle's prefix scopes contribute them to implicit application).
    /// Each entry pairs the face with its declaring file; `None` until the
    /// first `extension_application` probe.
    pub(crate) accessible_extensions: Option<Vec<(&'a ExtensionFace, FileId)>>,
    /// Names a READ-shaped member access resolved to (the `unused_field`
    /// read-only rule: writes never spare a field).
    pub(crate) read_names: FxHashSet<StringId>,
    /// The precise member-read index for the `unused_field` faces:
    /// `name → the declaring-container handles` a read resolved against.
    /// `read_names` keys by spelling alone — one class's read of
    /// `_subscription` spares every class's `_subscription` in the unit,
    /// while the oracle's `readMembers` is an element set (identity, not
    /// spelling). The receiver's static container decides: a field of a
    /// base class is read through a subtype receiver too, so the consumer
    /// joins against the candidate's full supertype closure.
    pub(crate) member_read_owners: FxHashMap<StringId, FxHashSet<(FileId, StringId)>>,
    /// Primary-constructor formals that declare a field (`var`/`final`
    /// keyword, no `this`/`super` selector — the official
    /// `FieldFormalParameterElement.isDeclaring`), in source order: the
    /// `unused_field_from_primary_constructor` face's candidates
    /// (unused_local_elements_verifier.dart:887-920).
    pub(crate) primary_fields: Vec<PrimaryFieldSite>,
    /// Resolved annotation names this unit referenced (the use-face's
    /// annotation uses; `check_annotation` inserts on the resolved side
    /// only — an unresolved annotation is its own error and never counts).
    pub(crate) annotation_uses: FxHashSet<StringId>,
    /// `(callee spelling, named argument)` pairs some invocation gave (the
    /// `_addParametersForArguments` face of the official use index).
    pub(crate) named_given: FxHashSet<(StringId, StringId)>,
    /// Callee spelling → the largest positional argument count any
    /// invocation gave (optional-positional parameters past it stay
    /// ungiven).
    pub(crate) positional_given: FxHashMap<StringId, u16>,
    /// Declaration site → the parameter names some body use resolved to.
    pub(crate) site_param_uses: FxHashMap<usize, FxHashSet<StringId>>,
    // —— #505: the import/flow cluster —————————————————————————————————————
    /// The import directives of this file, parse order: the written URI
    /// and the prefix (`ImportPrefixDeclaration` identifier).
    pub(crate) imports: Vec<ImportSite>,
    /// Whether the identifier in flight sits in a `show` list (the
    /// `begin_show`/`end_show` bracket); only `show` names join
    /// [`ImportSite::shown`].
    pub(crate) in_show: bool,
    /// Whether the directive in flight is an import (`begin_import` /
    /// `end_import`): `export` directives carry show lists too, and their
    /// names must not join the last import's site.
    pub(crate) in_import: bool,
    /// Whether the directive in flight is an export (`begin_export` /
    /// `end_export`): its show names join the use-face's re-export record.
    pub(crate) in_export: bool,
    /// The export directives' show faces so far, declaration order (the
    /// use-face's re-export record; `C::Combinator` appends the in-flight
    /// entry).
    pub(crate) exports: Vec<ExportUseFace>,
    /// Open label scopes (`_LabelTracker`, `dead_code_verifier.dart`:
    /// 618-661): labeled statements and switch member label groups.
    pub(crate) label_scopes: Vec<LabelScope>,
    /// The `on`-type name of the catch clause in flight (the first type
    /// reference before the catch parameters).
    pub(crate) catch_on_type: Option<StringId>,
    /// The clause in flight's `on` head opened a record type (`on (A, B)`,
    /// or a paren head the recovered parse walks as one). Upstream resolves
    /// such a head to a null/record `exceptionType`, which the
    /// `_CatchClausesVerifier` reads as the catch-all face (a record type is
    /// never `isDartCoreObject`) — so the clause fact carries `on_type:
    /// None` even though the head's field types fired `TypeReference`
    /// events that would otherwise claim [`Self::catch_on_type`].
    pub(crate) catch_on_record: bool,
    /// The catch clauses' `on`-type row offsets: a TypeUse miss at one of
    /// them codes `non_type_in_catch_clause` instead of `undefined_class`
    /// (the F-P recovered-clause routing, pinned by the parser's gate
    /// tripwires). Offsets accumulate across clauses until pass 2 — the
    /// TypeUse rows resolve at unit end, after the clauses closed — and
    /// each entry is consumed by exact match, so an unconsumed one (its
    /// type resolved) is inert.
    pub(crate) catch_on_miss_rows: Vec<u32>,
    /// Whether the clause in flight's `on` type completed with a trailing
    /// `?` (the head's own `handle_type` completions overwrite, so the
    /// parameterized re-fire carrying the `?` wins).
    pub(crate) catch_on_nullable: Option<bool>,
    /// The catch clause's first token (`on`/`catch`) — the clause fact's
    /// start anchor.
    pub(crate) catch_clause_start: Option<SourceRange>,
    /// Where the clause's head ended (the token after it, as an exclusive
    /// end offset). Only a clause whose body block never materialized
    /// (`on X;` — the `expected_body` recovery) reads it: the fact ends at
    /// the head instead of at a block end that never came (upstream's
    /// `CatchClause.end` is the recovered head's last consumed token).
    pub(crate) catch_head_end: Option<u32>,
    /// Catch parameters seen in the clause in flight (0 = none, 1 =
    /// exception, 2 = exception + stack trace).
    pub(crate) catch_params: u8,
    /// The clause in flight's parameter slot: `false` fills the exception
    /// slot, `true` the stack-trace slot. The parser flips it on the written
    /// comma (`handle_catch_trace_parameter`); a synthetic comma is the
    /// single-parameter recovery and never flips — the slot a parameter
    /// fills is the parser's comma fact, never the arrival order of the
    /// events (`catch (var e)` recovers its exception name synthetically,
    /// so the real `e` sits in the trace ensure yet stays the exception).
    pub(crate) catch_trace_slot: bool,
    /// The open catch re-parse's formal-list depth (latched at
    /// `begin_formal_parameters` with [`dartq_parser::MemberKind::Catch`]),
    /// gating the ordinal count to the group's own list against nested
    /// function-typed formals.
    pub(crate) catch_formal_group: Option<u32>,
    /// The catch re-parse's first/second real formal-name rows — the
    /// official AST's exception/trace faces (`handleCatchBlock`:
    /// exception = `catchParameters[0].name`, stack trace =
    /// `catchParameters[1].name`). Consumed by
    /// [`Engine::reconcile_catch_slots`] at the list's close.
    pub(crate) catch_ord_exc: Option<usize>,
    pub(crate) catch_ord_trace: Option<usize>,
    /// Per open try, the completed catch-clause facts (the
    /// `_CatchClausesVerifier` state).
    pub(crate) try_clauses: Vec<Vec<CatchClauseFact>>,
    /// Open `??` binary groups: rows-stack mark at `begin_binary_expression`,
    /// for the strictly-non-nullable left operand check at close, beside the
    /// right operand's first-token offset (the `dead_null_aware_expression`
    /// row's start — the official range is the right operand's own node
    /// range, which a degenerate value row cannot spell).
    pub(crate) if_null_groups: Vec<(usize, SourceRange, u32)>,
    /// The open `??`'s right operand's own `node.end` (the
    /// `handle_binary_rhs_end` latch). Set by the rhs-end hook, consumed
    /// (and cleared) by `end_if_null`.
    pub(crate) if_null_rhs_end: Option<u32>,
    /// Closed `??` expressions' static types, keyed by the right operand
    /// row's offset (the group's last row) → (left operand row offset,
    /// `LUB(NonNull(lhs), rhs)`). Only name-decidable joins land here; the
    /// parenthesized close reads it to type `(a ?? b)`.
    pub(crate) if_null_results: FxHashMap<u32, (u32, Ty)>,
    /// `??=` composites whose context S-branch is promotion-pending
    /// (#3820): every decidable input admits the branch (`S` from the
    /// argument's formal schema, `T <!: S`, `T2 <: S`) and only
    /// `NonNull(T1) <: S` waits on the target's promoted read — a fact the
    /// flow replay publishes at the executable's end, after every mid-walk
    /// consumer. Keyed like [`Self::if_null_results`]; the
    /// extension-bounds seam abstains over these composites (the
    /// parse-time join face is provisional there).
    pub(crate) if_null_s_pending: FxHashSet<u32>,
    /// Extension-member sends whose parse-time bounds check deferred past
    /// the if-null re-decide (#3593): the receiver composite's staged
    /// join degraded to the bare `Function` interface at parse (the
    /// signature-vs-interface collapse), so the bounds verdict over it
    /// waits for `redecide_promoted_if_null_faces` to join the
    /// flow-proven signature twin — the pass-2 re-run
    /// (`run_deferred_if_null_bounds`) re-pairs the send's already
    /// consumed written-argument group with the re-decided face. Holds
    /// (callee row offset, written type arguments), in deferral order.
    pub(crate) deferred_if_null_bounds: Vec<(u32, CallTypeArgs)>,
    /// Completed `==`/`!=` comparison extents: an equality's static type is
    /// `bool`, so an equality that ends exactly at the `??` left operand's
    /// rows makes that operand strictly non-nullable (the dead-coalesce
    /// face of `x != null ?? false`). The exact-span match against the
    /// operand rows keeps stale spans harmless; a parenthesized comparison
    /// buried in an earlier `??`'s right operand can still alias the next
    /// `??`'s operand rows (`y ?? (x != null) ?? false`) — a documented
    /// niche false-positive the row-level signals cannot separate.
    pub(crate) eq_spans: Vec<SourceRange>,
    /// Offsets of condition operands already classified for
    /// `unchecked_use_of_nullable_value` (the `&&`/`||` operands are
    /// condition contexts too, and an enclosing junction re-reads the same
    /// last row): the dedup keeps one staged check per operand site. A
    /// small ring — junction nesting is shallow.
    pub(crate) condition_staged: Vec<u32>,
    /// Extents statically decided `bool` (`x == y`, `a && b`, `x is T`,
    /// `x as bool`, a negation's operand): the `non_bool_condition`
    /// checkpoint reads the last row, whose span sits inside the deciding
    /// composite's extent there, so the condition face stays silent. The
    /// same small-ring bound as [`Self::eq_spans`]; exact-span matched.
    pub(crate) bool_decided: Vec<SourceRange>,
    /// The written return-type annotation of the executable currently being
    /// declared (armed by the type events while the frame's return type is
    /// still unset — exactly the head position), cleared when the
    /// executable's frame opens or closes. The `non_void_return_for_setter`
    /// face reads it at the declaration's end: the oracle reports at the
    /// written annotation when a setter's return type is neither `void`
    /// nor absent (error_verifier.dart:7015-7025).
    pub(crate) open_return_span: Option<SourceRange>,
    /// The row-stack height at the condition statement's keyword (`if`,
    /// `while`): the rows above it are exactly the condition's operand
    /// rows, and the first one starts the `non_bool_condition` anchor.
    /// Consumed by the condition's own checkpoint.
    pub(crate) cond_mark: Option<usize>,
    /// The row-stack heights at each open for-in iterable expression's
    /// start (`begin_for_in_expression` — the statement and collection
    /// forms fire the same event pair, `begin_for_in_expression`..
    /// `end_for_in_expression`, inside `parse_for_in_loop_parts_rest`):
    /// the rows above the top are exactly the iterable's operand rows, and
    /// `handle_for_in_loop_parts` — which carries the `await` keyword —
    /// runs the `_checkForEachParts` assignability face
    /// (`for_in_of_invalid_type`) over them. A stack, not a flag: a
    /// collection for-element inside an iterable expression nests a second
    /// begin/handle pair inside the outer window.
    pub(crate) for_in_marks: Vec<usize>,
    /// The for-in head's declarator latch (#2506): the loop variable's
    /// name token, captured at its declaration event (the head parses
    /// before the iterable) and consumed at `handle_for_in_loop_parts`
    /// to stage the element type on the flow declared-type seam. Every
    /// loop head resets it, so a classic-for declarator's stale latch
    /// never reaches a later for-in's consume.
    pub(crate) for_in_loop_var: Option<(StringId, SourceRange)>,
    /// The operand extents of this unit's `await` expressions
    /// (`(after-the-await-keyword, expression-end)`): the close-time row
    /// rewrite cannot flatten a member operand (its row resolves only in
    /// the later pass), so the extents re-apply the flatten where the
    /// resolved type is consumed — the `var`-inferred local's bind (#2506
    /// await face). One entry per await, file-scoped, read-only after the
    /// event.
    pub(crate) await_operand_extents: Vec<(u32, u32)>,
    /// The condition carried a pattern clause (`if (e case p)`): the
    /// official driver routes such statements through `analyzeIfCaseStatement`
    /// (type_analyzer.dart:3399-3412) and never reaches
    /// `handle_ifStatement_conditionEnd`, so the plain condition's
    /// `non_bool_condition` checkpoint must not run. Latched by
    /// `begin_pattern` while a condition window is open, consumed by the
    /// checkpoint.
    pub(crate) cond_case: bool,
    /// Open collection if-element conditions (`begin_if_control_flow`..
    /// `handle_then_control_flow`): the element form carries the same
    /// pattern-clause corner as the statement form — the official routes a
    /// case-clause element through `analyzeIfCaseElement`
    /// (type_analyzer.dart:3334-3346), which never reaches
    /// `handle_ifElement_conditionEnd` (`analyzeIfElement`, the shared
    /// analyzer, is the only caller), so its pattern opens the same
    /// one-case window and skips the plain condition checkpoint. A depth,
    /// not a flag: an element condition can nest a collection literal
    /// with if-elements of its own, whose `handle_then_control_flow` must
    /// not tear down the outer condition's window.
    pub(crate) if_element_cond: u32,
    /// Open record-type nesting depth: the field types of `on (A, B)` and
    /// friends are plain type positions, never the catch clause's `on`
    /// head.
    pub(crate) record_type_depth: u32,
    /// Null-aware chains over definitely-unassigned nullable locals (the
    /// `_verifyUnassignedVariable` face, dead_code_verifier.dart:460-505):
    /// `(operator offset, chain end, receiver read span, root kind, grown by
    /// a later section)` latched at the operator, resolved at teardown when
    /// the flow facts are in. The kind carries the verifier face the chain
    /// answers: `Member`/`Index` climb through the parent chain
    /// (`verifyNullAwareAccess` / `verifyReceiverIndexExpression`),
    /// `CascadeMember`/`CascadeIndex` fire only while the cascade's first
    /// section body stays bare (`verifyCascadeExpression`'s body-class
    /// gate). The growth bit marks a cascade chain a later section already
    /// extended (`verifyCascadeExpression` reports operator..cascade end) —
    /// a write then targets a later section, a node the verifier never
    /// sees, so the write retires nothing.
    pub(crate) dead_null_chains: Vec<(u32, u32, SourceRange, DeadChainKind, bool)>,
    /// `e!` over a local's read whose walk-time type decides nothing (the
    /// operand is the declared nullable/dynamic shape): `(read span, bang
    /// span)` re-decided at teardown against the flow's promoted type —
    /// the promoted-Never read after an earlier assertion makes the `!`
    /// dead (`unnecessary_non_null_assertion`). The third slot snapshots
    /// the in-scope type parameters that are potentially nullable at the
    /// `!` (no bound, or a nullable bound): a read promoted to such a
    /// parameter type (`E? item` reassigned from an `E` call) keeps the
    /// assertion live, and the scope is torn down by the flush.
    pub(crate) null_assert_staged: Vec<(SourceRange, SourceRange, Box<[StringId]>)>,
    /// Null-aware access/index over a local's read whose walk-time type is
    /// nullable: `(read span, operator span, kind)` re-decided at teardown
    /// against the promoted receiver type (`_checkForUnnecessaryNullAware`'s
    /// gate on the flow type).
    pub(crate) null_aware_staged: Vec<(SourceRange, SourceRange, NullAwareKind)>,
    /// `??`/`??=` dead-coalesce operands whose walk-time declared type did
    /// NOT anchor the dead face: `(operand read span, report range)`
    /// re-decided at teardown against the flow's promoted type —
    /// `_checkForDeadNullCoalesce` reads `leftOperand.staticType` / the
    /// `??=` target's `readResolution.type`, both flow-augmented (upstream
    /// Dart SDK source, pkg/analyzer/lib/src/generated/
    /// error_verifier.dart:4733-4739). The read types only materialize at
    /// the teardown join (`Engine::read_types` fills after pass 1 — the
    /// walk sees them empty), so a promoted operand (`x` non-null after
    /// `if (x != null)`, a `??=` write promotion) anchors the dead face
    /// only here. The report range is decided at stage time (the `??`
    /// right operand's node range, the `??=` value's expression span) —
    /// the row stack the ranges derive from is gone by teardown. Only
    /// LOCAL reads stage: the flow promotion table keys on the local
    /// read's own span (`local_hits`), so a member/unknown operand's
    /// staging would never join.
    pub(crate) coalesce_staged: Vec<(SourceRange, SourceRange)>,
    /// The flow layer's dead intervals (`FlowFact::DeadCode` ranges),
    /// re-homed for pass 2 before `resolve_rows` runs: a row anchored
    /// inside one sits in the oracle's dead region, where the official
    /// `ErrorVerifier` visits the node but the flow marks it dead and the
    /// non-`dead_code` verification never fires. Only the spans
    /// `emit_unchecked_nullable` would otherwise report on read through
    /// this table — the `dead_null_aware_expression` arm stays a live
    /// face (the official emits it INSIDE the dead window, so it never
    /// consults the table).
    pub(crate) dead_windows: Vec<SourceRange>,
    /// The promoted-receiver candidates (#1465): `receiver.member` merges
    /// whose receiver was a bare declared-type read, keyed by the merged
    /// member row's offset and carrying the receiver read's span — the same
    /// span the flow layer's read-type rows key on. Latched at the dot
    /// event, re-decided in pass 2 when the read types are in.
    pub(crate) promoted_recv_reads: Vec<(u32, SourceRange)>,
    /// The promoted-property spans (#1409) re-homed for pass 2: the flow
    /// fold completes before `resolve_rows`, so the nullable-dereference
    /// verdict inside `member_defined` can ask the same promotable-field
    /// question the teardown join asks — `x.m` on a promoted private
    /// final field never was an unchecked use.
    pub(crate) prop_promotion_rows: PropPromotionStaging,
    /// One record per `receiver.member` fold (#1409/B5): the merged row's
    /// name, receiver classification, and the receiver read's span,
    /// offset-keyed. The staged-join re-decides against it after pass 2 —
    /// `resolve_rows` takes the live stack before `filter_staged` runs, so
    /// the property-promotion substitution and the condition-face's
    /// property-access anchor both key on this record.
    pub(crate) member_reads: Vec<MemberRead>,
    /// [`Engine::member_reads`] keyed by member-token END offset (last
    /// insert wins, matching the old reverse scan's highest-offset match):
    /// the chain walk's O(1) hop lookup. A cascade of N sections walks N
    /// hops, and the linear scan per hop made that O(N²) — unbounded time
    /// on a bounded input (#41983's 20k-section cascade).
    pub(crate) member_read_ends: FxHashMap<u32, u32>,
    /// The pass-2 memo of [`Engine::recv_expression_ty`] answers, keyed by
    /// read-span offset. Active only across the `resolve_rows` row loop
    /// (every table the answer reads — `rebound_locals`,
    /// `init_derived_types`, the flow read types — is frozen by then), it
    /// collapses the per-row chain re-derivation to one walk per node:
    /// source order resolves each receiver before its members, so every
    /// walk bottoms out at the first memoized ancestor. Interior mutability
    /// because the walk runs through `&self`.
    pub(crate) recv_memo: RefCell<FxHashMap<u32, Option<Ty>>>,
    /// Whether [`Engine::recv_memo`] may be consulted (the `resolve_rows`
    /// row loop only; see the field's staleness contract).
    pub(crate) recv_memo_active: bool,
    /// The cascade streak index: every section selector's offset keyed to
    /// the recv_span of its cascade's first contiguous section (the record
    /// that carries the target's own span). Built once per unit in
    /// `resolve_rows` — the per-row walk-back it replaces paid O(N²) on a
    /// 20k-section cascade (#41983).
    pub(crate) cascade_first_spans: FxHashMap<u32, SourceRange>,
    /// The live [`Engine::recv_expression_ty`] recursion depth (interior
    /// mutability — the walk runs through `&self`). The walk descends one
    /// member token per hop; a pathological chain longer than
    /// `RECV_WALK_DEPTH_CAP` (row_resolution.rs) degrades to the record's
    /// classified receiver instead of recursing toward a worker-thread
    /// stack overflow.
    pub(crate) recv_walk_depth: Cell<u32>,
    /// The local receivers whose member merge classified as `Unknown`
    /// (#2521): the receiver's static type at the fold (a function value,
    /// a record, a `dynamic` — the shapes that name no container), keyed
    /// by the merged member row's offset. The receiver row itself folds
    /// away in the merge, so the send typing re-reads the answer when the
    /// `Object` member face (`toString`/`hashCode`/`runtimeType`) resolves
    /// through it — the official resolver's chain for every one of these
    /// heads.
    pub(crate) member_recv_tys: FxHashMap<u32, Ty>,
    /// The folded index expression's argument-check staging (#2937): each
    /// `a[i]`/`a[i] op= v`/`++a[i]` folds its operand rows away at
    /// `handle_indexed_expression`, so the index argument's static type and
    /// span ride this latch — keyed by the bracket's open `[` offset —
    /// until the write form finalizes (`write_op` lands on the operator
    /// row only when an assignment/compound mark follows). The read and
    /// write `[]`/`[]=` first formals resolve at stage time (upstream Dart
    /// SDK source, `checkIndexExpressionIndex`,
    /// error_detection_helpers.dart:260-289); the emission then applies the
    /// write form's element pair at the argument's span — read-only for a
    /// pure `a[i]`, `[]=` for `=`, both for `+=`/`++`.
    pub(crate) index_arg_faces: FxHashMap<u32, IndexArgFace>,
    /// The folded index reads (`a[i]`, `c.l[i]`): the result row's offset
    /// (the closing `]`) keyed to the indexed receiver's own span and
    /// whether the access was null-aware (`?[`). Pass 2 re-derives the
    /// result face from the receiver expression's re-derived type — a
    /// promoted or re-bound receiver (`fr[i]` over `var fr = e.fragments`
    /// past `e is PrefixElement`) indexes its promoted element type.
    pub(crate) index_result_recvs: FxHashMap<u32, (SourceRange, bool)>,
    /// Receiver classes bound to closure formals by the call-time
    /// inference (the solved callee-own parameter's substituted head —
    /// see [`Engine::closure_param_context_ty`]). A closure formal's
    /// type is OUR approximation of the oracle's inference; the member
    /// walk on it under-resolves the extension/import members the oracle
    /// resolves through, so the undefined-member rows against these
    /// containers stay silent — the under-report posture.
    pub(crate) inferred_formal_containers: FxHashSet<StringId>,
    /// Member spellings whose resolution the inference-bound receiver gate
    /// suppressed ([`Engine::inferred_formal_containers`]): the oracle
    /// resolves every one of those accesses (the under-report posture's
    /// premise), so the spelling stays a RESOLVED use — without the credit
    /// the member walk never ran and the declaration would flip to
    /// `unused_element` (manifest_item.dart's extension `write` family,
    /// whose only uses ride inferred formals).
    pub(crate) inferred_recv_used_members: FxHashSet<StringId>,
    /// The spellings of closure-formal parameters
    /// ([`Engine::closure_param_context_ty`] records each): a formal the
    /// callee inference never solved still roots an oracle-resolved member
    /// chain, and the extension-use re-attribution walks `sub.source.meta`
    /// chains off a receiver identifier spelled like one of these to
    /// re-credit the reads the silent walk never attributed
    /// (`devtool.dart`'s `map((sub) => sub.source.meta)`).
    pub(crate) closure_formal_names: FxHashSet<StringId>,
    /// Member rows that resolved with an `Unknown` receiver, recorded at
    /// resolution time (the rows table is emptied before finalization):
    /// `(name-ident offset, name)`. The extension-use re-attribution's
    /// closure-formal-chain arm reads them — the chains the merged-read
    /// record never captured (`sub.impl` after an untyped-call local).
    pub(crate) unresolved_member_rows: Vec<(u32, StringId)>,
    /// The closure-return invalidity probe (the resolve_file corpus face):
    /// per closure offset, whether any counted return expression typed a
    /// named head ([`Self::closure_return_counted`]) and whether any
    /// counted return was VALID at its origin
    /// ([`Self::closure_return_valid`]). A closure whose counted returns
    /// are all invalid-at-origin ([`Self::closure_invalid_returns`]) makes
    /// the oracle infer an `InvalidType` type argument — the send whose
    /// `T Function(...)` formal rode it answers an invalid-typed result.
    pub(crate) closure_return_counted: FxHashSet<u32>,
    pub(crate) closure_return_valid: FxHashSet<u32>,
    pub(crate) closure_invalid_returns: FxHashSet<u32>,
    /// The spans (start, end) of the all-invalid closures — available for
    /// argument matching even when the signature capture poisoned.
    pub(crate) invalid_closure_spans: Vec<(u32, u32)>,
    /// Local names whose initializer send the closure probe proved
    /// invalid-typed (an unsolved inference parameter constrained only by
    /// an all-invalid closure) — the oracle's static type for the local is
    /// `InvalidType`, so no extension applies over it and the
    /// extension-import credit denies. Keyed by name: the locals stack is
    /// consumed before the usage pass, and the receiver span match
    /// re-interns the spelling.
    pub(crate) invalid_typed_locals: FxHashSet<StringId>,
    /// Member-row offsets whose merged receiver is a typed head the
    /// resolver PROVED invalid at merge time — the only moment the proof
    /// is honest: a method's type-parameter receiver (`DB extends
    /// GeneratedDatabase`, an extension's `on DB` target) is a valid
    /// static type while its scope lives, and the finalization-time
    /// `is_invalid_ty` re-ask cannot see scope parameters anymore (the
    /// scope stack is consumed before the usage pass). The
    /// extension-credit Typed arm reads this witness instead of re-asking.
    pub(crate) invalid_typed_member_rows: FxHashSet<u32>,
    /// Member rows that resolved through a nullable-applicable extension
    /// face on a nullable receiver ([`Engine::member_defined`]'s safe
    /// `Single` verdict): the access needs no null check and the member's
    /// result keeps the extension's own nullability — the staged
    /// nullable-condition face (a `&&` right operand reading the member)
    /// consults this and stands down (the census's
    /// `element.supertype.isAbiSpecificInteger` over `extension on
    /// DartType?`, ffi_verifier.dart:2644).
    pub(crate) extension_nullable_resolved: FxHashSet<u32>,
    /// One record per completed send: the callee row's offset through the
    /// closing `)` (offset-keyed, kept sorted — nested calls complete
    /// inner-first, so pushes are not monotonic). The assignability emit
    /// anchors a call-expression operand at the whole invocation span the
    /// oracle's `FunctionExpressionInvocation`/`MethodInvocation` node
    /// carries (`int x = f()` reports over `f()`, not `f`); the callee
    /// row's own range stays name-only for the count faces' callee anchor.
    pub(crate) call_ends: Vec<(u32, u32)>,
    /// The non-promotion substrate rows (#1877), keyed by the read's offset
    /// (the `WhyNotPromoted` read token, the `NonPromotedPropertyRead`
    /// member token, the `ThisNotPromoted` `this` token): the flow facts'
    /// reason rows pre-filtered to the shape the context producer renders.
    /// Staged in the teardown fold (facts arrive offset-sorted, so pushes
    /// keep the sort) and read by the context face at attach time.
    pub(crate) non_promotions: Vec<(u32, StagedNonPromotion)>,
    /// The why-not-promoted context requests the parse-time emissions
    /// deferred (#1877): the diagnostic's report index and the read offset
    /// its context rows hang on. The flow facts only exist after an
    /// executable replays, so a parse-time report site cannot attach
    /// inline; the teardown drains the list once the fact rows are staged.
    /// Indices hold — nothing removes report rows between push and drain.
    pub(crate) pending_non_promotion: Vec<(usize, u32)>,
    /// Open string literals' row marks (#1823): the row count at each
    /// `begin_literal_string`, stacked for nested literals
    /// (`"${'x$y'}"`). `end_literal_string` pops its mark and records every
    /// row pushed inside the literal — the interpolation's own reads — so
    /// the single-expression-window faces ([`Engine::single_row_since`])
    /// can attribute the literal's completion row to the whole expression
    /// the way the oracle's `StringInterpolation` node does.
    pub(crate) string_row_marks: Vec<usize>,
    /// The interpolation-child row offsets recorded by
    /// [`Engine::string_row_marks`]'s drain: reads that live *inside* a
    /// string literal, never the enclosing expression's value. Offset-keyed
    /// (row folds splice the vec, indices lie; an interpolation read's
    /// offset is inside a string token, where no other row can anchor).
    pub(crate) interpolation_child_rows: FxHashSet<u32>,
    /// The read-type rows (#1056) joined into the semantic `Ty` space,
    /// offset-sorted: the flow type at every reachable read. Staged before
    /// pass 2 for the member re-decision, re-homed into the [`UnitFold`]
    /// product at teardown.
    pub(crate) read_types: Vec<(SourceRange, Ty)>,
    /// The `as` expressions' whole spans with their resolved cast targets,
    /// in arrival order — a staged verdict anchored inside a cast's span
    /// reads the rewritten static type (`AsExpression.staticType`), never
    /// the operand's parse-time capture.
    pub(crate) cast_rewrites: Vec<(SourceRange, Ty)>,
    /// The record-typed receiver reads' member offsets with the parse-time
    /// static type, in arrival order — the receiver classification has no
    /// record face (the flow lattice cannot spell records either), so this
    /// side table is the only holder of the record the pass-2
    /// record-extension application reads (`source.unwrapSentinel(..)`
    /// over the `(DataSource?,)?` formal).
    pub(crate) record_recv_tys: Vec<(u32, Ty)>,
    /// The semantic-side query-type rows (#1869-2): spans whose static
    /// type the walk itself knows but the flow layer cannot answer — the
    /// `this` keyword (the enclosing container's type) and the
    /// implicit-`this` bare member reads (the member's declared type).
    /// Merged into `read_types` at the teardown join, ahead of the sort;
    /// the flow layer types neither span, so the merge cannot collide.
    pub(crate) query_type_rows: Vec<(SourceRange, Ty)>,
    /// The flow's `void`-typed reads (#3632): `staged_promotion_typed` has
    /// no `Ty` spelling for a `void` head, so the shared `read_types` join
    /// drops them — but the statement-form operand faces
    /// (`…AsIterator`/`…InYieldEach`/the `unary-` operator face) decide
    /// `isPotentiallyNullable(void)` = true on exactly these rows. Kept
    /// beside the join instead of inside it: the shared table's
    /// receiver/scrutinee consumers answer on resolved interface types,
    /// and a `void` row there would re-decide faces the oracle reports
    /// through other codes. Only the operand-face staging reads this.
    pub(crate) void_read_rows: Vec<(SourceRange, Ty)>,
    /// Unnamed-extension frames opened so far (#1701): the walk visits
    /// container heads in source order and the pool appends the matching
    /// application faces in the same order, so the k-th unnamed-extension
    /// frame latches the k-th nameless face. `collect` finishes before the
    /// walk starts, so a length read at push time cannot identify the
    /// declaration — the count is the only source-order correlation.
    pub(crate) unnamed_extension_faces_opened: usize,
    /// Pattern variables declared outside every frame — the top-level
    /// variable-initializer world (`final x = switch … int(:var v) …`),
    /// where no executable frame exists to join. The enclosing clause's
    /// reads (the case guard, the case body) resolve through this set; it
    /// clears with the declaration that staged it.
    pub(crate) initializer_locals: FxHashSet<StringId>,
    /// The open explicit `const` creation keywords: each
    /// `begin_const_expression` pushes, its close pops. The innermost span
    /// is the `const_with_non_const` anchor.
    pub(crate) const_kw_spans: Vec<SourceRange>,
    /// The const creation whose target check awaits the window close:
    /// `(callee head, const-keyword span)` latched at the creation's
    /// argument close, decided at `end_const_expression`.
    pub(crate) const_creation_pending: Option<(Option<StringId>, SourceRange)>,
    /// Open local-function declarations as `(declaration start, wildcard
    /// verdict)`: an `_`-named local function reports dead over the whole
    /// declaration (`DeadCodeVerifier`'s visitFunctionDeclaration,
    /// dead_code_verifier.dart:72-82).
    pub(crate) local_fn_wildcard: Vec<(u32, bool)>,
    /// Emission cursor over [`crate::model::FileElements::wildcard_duplicates`]:
    /// the wildcard-dup rows drain incrementally as the walk reaches the
    /// drain hooks (a pre-feature unit's `_` duplicates report at their
    /// source order), each hook emitting the rows past this index so the
    /// whole log emits exactly once per unit.
    pub(crate) wildcard_dups_emitted: usize,
    /// Formal-parameter name spans per declaration site (the
    /// `unused_element_parameter` anchors).
    pub(crate) site_params: FxHashMap<usize, Vec<(StringId, SourceRange)>>,
    /// Rows pushed while the named scope referred to itself (the official
    /// `_enclosingClass`/`_enclosingExec` self-reference exclusions): the
    /// used-name marking at resolve time skips exactly these rows.
    pub(crate) self_scope_rows: FxHashSet<u32>,
    /// Rows pushed while the bare name matched a field of the enclosing
    /// container: a non-write resolution is the field's implicit-getter
    /// read (the official `readMembers` face the `unused_field` rule
    /// consults). The value is the matched container, snapshotted at push
    /// time — the container frame is gone by pass 2, and a bare field read
    /// spares exactly the field of that container (a primary-constructor
    /// field read inside a getter included).
    pub(crate) field_read_rows: FxHashMap<u32, StringId>,
    /// Bare rows whose statement-window read is discarded (the official
    /// `_isReadIdentifier`, upstream Dart SDK source,
    /// pkg/analyzer/lib/src/error/unused_local_elements_verifier.dart:418-
    /// 440: an identifier directly under a compound-assignment LHS, a
    /// prefix, or a postfix whose own parent is the ExpressionStatement is
    /// no read — `_x += 1;`, `_x++;`, `-_x;`; `??=` and member targets
    /// (`this._x += 1;`) keep theirs). The offset joins at the statement
    /// close and pass 2's bare read-credit gate consults it for the
    /// `unused_field` face — the same discard the local `uses` decrement
    /// already carries.
    pub(crate) statement_discarded_rows: FxHashSet<u32>,
    /// The container of the constructor/factory being parsed (`None`
    /// outside one).
    /// The open constructor/factory's container id plus the provisional
    /// declaration site the constructor face registered at its head (the
    /// class-name token — the unnamed constructor's own spelling). The
    /// selector continuation rewrites the site in place (`A.named` is one
    /// site named `named`, never a second push); factories carry no
    /// provisional (their optional parameters stay out of the
    /// `unused_element_parameter` face).
    pub(crate) ctor_pending: Option<(StringId, Option<usize>)>,
    /// The constructor's own executable-frame depth (`frames.len()` right
    /// after `push_executable_frame`) plus whether that frame's body brace
    /// has been noted. `this`/`super` legality asks "lexically between the
    /// initializer `:` and the body `{`" — a braceless closure opened in
    /// the body has no `body_brace` of its own, so the ctor's own latch,
    /// not the innermost frame's, answers (the R53 ctor-body `this` face:
    /// `_state = AsyncResult.guard(() => initState(this))`).
    pub(crate) ctor_body_open: bool,
    pub(crate) ctor_frame_len: usize,
    /// The top-level function declaration site awaiting its setter
    /// spelling fix-up at `end_top_level_method`.
    pub(crate) open_toplevel_site: Option<usize>,
    /// The member declaration's `set` head (`begin_method` latch; the
    /// member site's lookup spelling).
    pub(crate) open_member_setter: bool,
    /// The member declaration's `get`/`set` head (`begin_method` latch):
    /// the accessor face — upstream a `PropertyAccessorElement`, whose
    /// return-check host word is `function`, never `method`
    /// (error_verifier.dart:164-166, `EnclosingExecutableContext.isFunction`).
    pub(crate) open_member_accessor: bool,
    /// The member/field declaration's `static` head (`begin_method` /
    /// `begin_fields` latch; the member site's static bit).
    pub(crate) open_member_static: bool,
    /// The label name captured at `LabelDeclaration`, consumed by the
    /// `handle_label` colon.
    pub(crate) pending_label: Option<(StringId, SourceRange)>,
    /// The last block's closing-brace span (the catch body anchor).
    pub(crate) last_block_end: Option<SourceRange>,
    /// The local function whose name has fired but whose declaration has
    /// not closed: the tracked `LocalFunction` entry materializes into the
    /// ENCLOSING frame's window at the end event (the name resolves in the
    /// function's own frame, but its use-counting scope is the enclosing
    /// one).
    pub(crate) pending_local_fn: Option<(StringId, SourceRange)>,
    /// Open local-function frames as `(frame depth, name)`: the official
    /// `_enclosingExec` spelling for bodies inside a local function (a
    /// self-reference never marks the function used).
    pub(crate) local_fn_stack: Vec<(usize, StringId)>,
    /// The #480 language-strictness modes the caller resolved from the
    /// library's analysis options (all off = the official defaults).
    pub(crate) strict: StrictModes,
    /// The per-library `class-modifiers` posture (#1294): each analyzed
    /// library's defining file's settled language version resolved through
    /// the feature registry, injected per analyze. Absent libraries answer
    /// the modern posture.
    pub(crate) class_modifiers: &'a LibraryClassModifiers,
    /// The open type-variables window's bound accounting (`class C<T, S
    /// extends num>`): each variable's name token lands here, and the
    /// parser emits `handle_no_type` — anchored at that name token — for
    /// an unbounded variable, so the unbounded count closes complete at
    /// `end_type_variables`.
    pub(crate) tv_window: Option<TvWindow>,
    /// The just-closed type-variables window's unbounded count with the
    /// group's closing offset, awaiting the declaration head it belongs to
    /// (the window closes before `begin_class_declaration` and friends
    /// name the declaration).
    pub(crate) pending_tv: Option<(u16, u32)>,
    /// This file's containers with at least one unbounded type parameter,
    /// captured at the container heads of this pass (the
    /// `strict_raw_type` decidable set: the oracle reports a raw use exactly
    /// when some type argument instantiates to `dynamic` — an unbounded
    /// parameter — `type_arguments_verifier.dart:711-735`). Cross-file
    /// containers and fully-bounded ones stay out: no fact, no report.
    pub(crate) container_unbounded: FxHashMap<StringId, u16>,
    /// The library's resolved-type face (#2204): the `LibraryTypes`
    /// snapshot this unit's diagnosing fold asks structural type questions
    /// against — the extension application's subtype/specificity/bounds
    /// checks read it instead of the name-level approximations whenever it
    /// is mounted (`None` keeps the name-level posture, the rig default).
    pub(crate) ops: Option<&'a crate::resolved_ops::ResolvedTypeOps<'a>>,
    /// The #610 boundedness capture (the `bounds` module): the parse-time
    /// type-tree index plus the lazy `dartq_types` snapshot the checks run
    /// against.
    pub(crate) bounds: Bounds<'a>,
    /// The explicit type-argument sites and enum declarations awaiting the
    /// end-of-unit boundedness checks, in source order — the snapshot must
    /// see every in-file container, so nothing checks before the parse
    /// completes.
    pub(crate) pending_bounded_checks: Vec<PendingBoundedCheck>,
    /// The `strict_raw_type` report sites (name, anchor): their
    /// instantiated-to-bounds displays need the whole-file snapshot too.
    pub(crate) raw_type_sites: Vec<(StringId, SourceRange)>,
    /// The #477 S1 staged negative verdicts of the assignability families,
    /// parse order: flow's promotion facts are not answerable mid-parse,
    /// so the teardown re-decides each against the promoted-read staging
    /// before the row emits.
    pub(crate) staged_checks: Vec<StagedCheck>,
    /// The verdict-side alias fold's memo ([`Engine::fold_alias_verdict`]):
    /// one interned `Ty` folds once per unit — the subtype ask re-enters
    /// per staged check and per covariant element, and the fold's miss
    /// path (scoped lookup, declaring-libraries probe, the to-bounds fill)
    /// is the cost to amortize. Entries never go stale: the alias table is
    /// final before the walk runs.
    pub(crate) alias_fold_memo: std::cell::RefCell<FxHashMap<Ty, Ty>>,
    /// The #746 `cast_from_nullable_always_fails` candidates, offset-sorted
    /// by the read's identifier span: the cast junction's four decidable
    /// conditions held (simple local read, declared type spelled nullable,
    /// non-nullable named target); the definite-unassignment face joins at
    /// the teardown against the flow layer's `UnassignedRead` facts. The
    /// name rides along (the message's placeholder — the row span is a
    /// UTF-16 range, the interned id is the exact spelling).
    pub(crate) cast_nullable: Vec<(SourceRange, StringId)>,
    /// The #746 executable-completion rows, offset-sorted by the body's
    /// end token (the `BodyMayCompleteNormally` fact's span): the declared
    /// return type plus the block body's `{` when the body is a block.
    /// One row per method/top-level declaration ([`Engine::
    /// stage_body_completion`]). The `bool` is the body's `async` flag —
    /// the completion verdict reads the future-flattened return for it
    /// (`TypeSystem.flatten`), not the declared `Future<...>` head. The
    /// trailing range is the declaring name's own span — the completion
    /// rows' anchor. The last `bool` marks a closure-context row
    /// (`ctx_ret`-sourced): its `FutureOr` heads are synthetic
    /// (`core_unindexed_future_formals`) or in-scope, never a written
    /// unimported spelling, so the completion walk's `FutureOr` scope
    /// gate does not apply (#4265).
    pub(crate) body_completions: Vec<(
        SourceRange,
        Ty,
        Option<SourceRange>,
        bool,
        SourceRange,
        bool,
    )>,
}

/// One deferred boundedness check (the [`Engine::pending_bounded_checks`]
/// entries, captured in source order).
pub(crate) enum PendingBoundedCheck {
    /// The enum `name` declared at `range` (the name-token anchor): its
    /// values-field face runs at the finalize pass.
    Enum { name: StringId, range: SourceRange },
    /// An explicit type-argument group at `head`: the captured argument
    /// trees and the type parameters in scope at the site. `site` is the
    /// whole type node's span (head through the group closer) — the
    /// context rows' anchor (the official `node.offset/length`).
    Site {
        head: StringId,
        args: Vec<TypeTree>,
        in_scope: Vec<StringId>,
        site: SourceRange,
    },
}

/// The bound accounting of one open type-variables window.
#[derive(Default)]
pub(crate) struct TvWindow {
    /// The window's variable name tokens (`begin_type_variable`): an
    /// in-window `handle_no_type` counts as an unbounded variable only
    /// when anchored at one of them — the parser emits the variable's own
    /// name token there — while a generic-function-type bound's absent
    /// return type (`T extends Function()`) anchors elsewhere and must
    /// not count.
    pub(crate) vars: Vec<Token>,
    /// Variables declared without a bound.
    pub(crate) unbounded: u16,
}

// —— #612: the variance-family capture —————————————————————————————————————
// Written-type trees captured from the same parse events the #610 bounds
// capture consumes, scoped to a container's header (from the container-begin
// event, where the capture opens, to the body-begin event). The consuming
// check runs in [`Engine::finalize_variance`]; the capture opens regardless
// of the experiment (#742 — the legacy superinterface face answers in the
// default posture) and the trees drop at the finalize's take.

/// One written type inside a container header's clause region
/// (`extends A<F<T>>`, `implements void Function(T)` shapes included — the
/// shapes the #610 `TypeTree` poisons are exactly the ones this family's
/// verdicts live in).
pub(crate) struct VarTree {
    pub(crate) kind: VarTreeKind,
    /// The trailing `?` was written.
    pub(crate) nullable: bool,
    /// An unmodelable or ambiguous capture was seen inside: the whole tree
    /// suppresses its face (the poison propagates to the root it rides on).
    pub(crate) poisoned: bool,
}

pub(crate) enum VarTreeKind {
    /// A named type as written (type parameters keep their name — the
    /// model's bare-name convention), with the written arguments.
    Head { name: StringId, args: Vec<VarTree> },
    /// A function type: the return type (`None` for `void`/recovery — an
    /// unrelated contribution), the formal types, and the bounds of the
    /// signature's own type parameters (an occurrence forces `invariant`,
    /// `variance_builder.dart:131-140`).
    Fn {
        ret: Option<Box<VarTree>>,
        params: Vec<VarTree>,
        bounds: Vec<VarTree>,
    },
    /// A record type: the meet runs over every field (the written field
    /// names are not captured; the display renders the positional shape).
    Record { fields: Vec<VarTree> },
    /// A poison blob (a headless argument group): contributes nothing.
    Unit,
}

/// Which clause a captured superinterface tree belongs to (the official
/// walk's order is the supertype, the interfaces, the mixins, then a
/// mixin's constraints — the staged rows sort by anchor offset, so the
/// slot only feeds the capture bookkeeping).
#[derive(Clone, Copy)]
pub(crate) enum VarSlot {
    Extends,
    With,
    Implements,
    On,
}

/// One open nested capture frame inside a header: a type-argument group, a
/// function type, or a record type.
pub(crate) enum VarFrame {
    /// `<...>`: `head` is `Some` until its group closes (the parser re-fires
    /// a parameterized head after the group — the merge latch lives on the
    /// sink, keyed by the head's name and offset).
    Group {
        head: Option<(StringId, u32)>,
        nodes: Vec<VarTree>,
        poisoned: bool,
    },
    Fn(VarFnFrame),
    Record {
        nodes: Vec<VarTree>,
        poisoned: bool,
    },
}

/// The open function-type frame: trees route by region (bounds of the
/// signature's own type parameters, then the formals after the boundary,
/// then the return).
pub(crate) struct VarFnFrame {
    pub(crate) ret: Option<VarTree>,
    pub(crate) params: Vec<VarTree>,
    pub(crate) bounds: Vec<VarTree>,
    pub(crate) in_params: bool,
    pub(crate) in_bounds: bool,
    pub(crate) poisoned: bool,
}

impl VarFnFrame {
    pub(crate) fn route(&mut self, node: VarTree) {
        self.poisoned |= node.poisoned;
        if self.in_bounds {
            self.bounds.push(node);
        } else if self.in_params {
            self.params.push(node);
        } else if self.ret.is_none() {
            self.ret = Some(node);
        } else {
            // A second root before the formals is a capture this face
            // cannot attribute (a leaked bound, recovery residue).
            self.poisoned = true;
        }
    }

    pub(crate) fn into_tree(mut self, nullable: bool) -> VarTree {
        VarTree {
            nullable,
            poisoned: std::mem::take(&mut self.poisoned),
            kind: VarTreeKind::Fn {
                ret: self.ret.map(Box::new),
                params: self.params,
                bounds: self.bounds,
            },
        }
    }
}

/// The written-type capture of one open container header.
#[derive(Default)]
pub(crate) struct VarSink {
    pub(crate) roots: Vec<VarTree>,
    pub(crate) stack: Vec<VarFrame>,
    /// A parameterized group just closed; the re-fired head
    /// (`handle_type`, same name at the same offset) completes it with its
    /// trailing `?`.
    pub(crate) pending_head: Option<(StringId, u32, Vec<VarTree>)>,
}

impl VarSink {
    /// Routes a completed node into the innermost open frame (or the
    /// roots), propagating the poison bit.
    pub(crate) fn route(&mut self, node: VarTree) {
        match self.stack.last_mut() {
            Some(
                VarFrame::Group {
                    nodes, poisoned, ..
                }
                | VarFrame::Record { nodes, poisoned },
            ) => {
                *poisoned |= node.poisoned;
                nodes.push(node);
            }
            Some(VarFrame::Fn(frame)) => frame.route(node),
            None => self.roots.push(node),
        }
    }

    pub(crate) fn poison_top(&mut self) {
        match self.stack.last_mut() {
            Some(VarFrame::Group { poisoned, .. } | VarFrame::Record { poisoned, .. }) => {
                *poisoned = true;
            }
            Some(VarFrame::Fn(frame)) => frame.poisoned = true,
            None => self.roots.push(VarTree {
                kind: VarTreeKind::Unit,
                nullable: false,
                poisoned: true,
            }),
        }
    }

    /// The innermost function-type frame consumed the formal boundary;
    /// answers `false` when no frame did (the header-level rep-formals
    /// latch takes over).
    pub(crate) fn formal_boundary(&mut self) -> bool {
        match self.stack.last_mut() {
            Some(VarFrame::Fn(frame)) => {
                frame.in_params = true;
                true
            }
            _ => false,
        }
    }

    pub(crate) fn open_bounds(&mut self) {
        if let Some(VarFrame::Fn(frame)) = self.stack.last_mut() {
            frame.in_bounds = true;
        }
    }

    pub(crate) fn close_bounds(&mut self) {
        if let Some(VarFrame::Fn(frame)) = self.stack.last_mut() {
            frame.in_bounds = false;
        }
    }
}

/// The written signature of a function-typed variable annotation
/// (`T Function<T extends num>(T, [U])`), captured from the parse events.
/// The element model poisons function-type annotations to `Ty::Unknown`,
/// so this is the only structural face a variable's invocation checks
/// have; every capture failure suppresses it (the under-report direction).
#[derive(Clone, Debug)]
pub(crate) struct FnVarSig {
    /// The signature's own type parameters with their written bounds.
    pub(crate) params: Box<[(StringId, Option<Ty>)]>,
    /// Positional parameter types, in declaration order.
    pub(crate) positional: Box<[Ty]>,
    /// How many leading positional parameters the required group holds.
    pub(crate) required_positional: u16,
    /// Named parameters (name, type, written `required` keyword) — the
    /// flag decides the function-subtype required-presence rule
    /// (subtype.dart:143, callers may omit a non-required named formal).
    pub(crate) named: Box<[(StringId, Ty, bool)]>,
    /// The written return type.
    pub(crate) return_ty: Ty,
}

impl FnVarSig {
    /// The declared-formals view the argument check consumes. Positional
    /// formals carry no names the checks read; they take the interned
    /// empty name.
    pub(crate) fn formals(&self, empty: StringId) -> Formals {
        let required_boundary = self.required_positional as usize;
        let positional = self
            .positional
            .iter()
            .enumerate()
            .map(|(index, ty)| Formal {
                name: empty,
                ty: *ty,
                required: index < required_boundary,
                // A written function-type annotation carries neither a
                // covariant modifier nor default values — the honest
                // absent on both columns.
                covariant: false,
                default_value: None,
            })
            .collect();
        let named = self
            .named
            .iter()
            .map(|(name, ty, required)| Formal {
                name: *name,
                ty: *ty,
                required: *required,
                covariant: false,
                default_value: None,
            })
            .collect();
        Formals { positional, named }
    }
}

/// One executable's written formal signatures: positional signatures in
/// declaration order (the argument check matches them by position —
/// [`Formals`] erases positional names by design), named signatures by
/// spelling.
#[derive(Default)]
pub(crate) struct FnFormalSigs {
    pub(crate) positional: Vec<Box<FnVarSig>>,
    pub(crate) named: FxHashMap<StringId, Box<FnVarSig>>,
}

/// One open written function-type capture
/// (`begin_function_type`..`end_function_type`); nested annotations stack.
#[derive(Default)]
pub(crate) struct FnTypeCapture {
    /// The signature's own type variables, in declaration order — the
    /// in-scope set for the annotation's own parameter and return types
    /// (the official scopes them to the function type; they never leak
    /// into the initializer that follows).
    pub(crate) scope: Vec<StringId>,
    /// The type variables with their written bounds, bound at each
    /// variable's end event.
    pub(crate) params: Vec<(StringId, Option<Ty>)>,
    /// Positional parameter types captured so far.
    pub(crate) positional: Vec<Ty>,
    /// How many of them the required group holds.
    pub(crate) required_positional: usize,
    /// Named parameters captured so far, with each one's written `required`
    /// keyword (`required` fires as a formal-head token, latched per
    /// formal).
    pub(crate) named: Vec<(StringId, Ty, bool)>,
    /// The written return type, latched with its completing head (the
    /// region's last completion — the parameterized head's re-fire
    /// replaces its leaf and type-argument completions).
    pub(crate) ret: Option<(Ty, StringId, u32)>,
    /// The armed type of the formal currently being parsed.
    pub(crate) pending_formal: Option<Ty>,
    /// Inside the formal-parameter list.
    pub(crate) in_params: bool,
    /// The signature's own type-variable group is open (the `<T extends …>`
    /// pass before the return region): bound completions belong to the
    /// variables, never to the return.
    pub(crate) in_variables: bool,
    /// The current formal section (required positional, optional
    /// positional, named).
    pub(crate) section: FnFormalSection,
    /// An unattributable completion (a nested function type, a stray
    /// second return head): the capture suppresses.
    pub(crate) poisoned: bool,
}

/// The formal section a capture is currently inside.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum FnFormalSection {
    #[default]
    Required,
    OptionalPositional,
    Named,
}

/// The open call-site type-argument capture: the written member types of
/// one `<...>` group at an invocation (`f<int>`, `C<num>(0)`), destined
/// for the argument check's explicit grounding.
#[derive(Default)]
pub(crate) struct CallArgsCapture {
    /// The completed member types with their written spans (each member's
    /// annotation range — the bounds face's per-argument anchor).
    pub(crate) members: Vec<(Ty, SourceRange)>,
    /// The leaf head awaiting its re-fired completion (same name at the
    /// same offset replaces instead of pushing a second member).
    pub(crate) pending: Option<(StringId, u32)>,
    /// The group's own `type_arg_depth` (latched at the open): only
    /// completions at this depth are members — a member's own nested
    /// group (`List<int>` inside `f<List<int>>(...)`) completes at deeper
    /// depths and its inner heads must never join the member list.
    pub(crate) depth: u32,
}

/// The latched explicit type arguments of one call: the written types and
/// each one's own annotation span.
#[derive(Clone)]
pub(crate) struct CallTypeArgs {
    /// The written types, group order.
    pub(crate) tys: Box<[Ty]>,
    /// Each type's written span, aligned with [`Self::tys`].
    pub(crate) spans: Box<[SourceRange]>,
}

/// One container's captured header: its type parameters (name, declared
/// keyword, name range — the emission anchor) and the clause-classified
/// superinterface trees.
pub(crate) struct VarHeader {
    pub(crate) name: StringId,
    pub(crate) params: Vec<(StringId, Option<Variance>, SourceRange)>,
    pub(crate) sink: VarSink,
    pub(crate) supers: Vec<(VarSlot, VarTree)>,
    /// A clause count disagreed with its captured roots, or roots remained
    /// unclaimed at body time: the capture is unfaithful, the container's
    /// checks suppress.
    pub(crate) open: bool,
    /// The extension-type primary constructor's formals region (their
    /// types are not superinterfaces).
    pub(crate) rep_formals: bool,
    /// The formals regions nested inside the rep-formals latch (a function
    /// type formal's own region re-enters on the suppressed, frame-less
    /// sink): the latch lifts only when the region that armed it closes.
    pub(crate) rep_depth: u32,
}

impl VarHeader {
    pub(crate) fn new(name: StringId) -> Self {
        Self {
            name,
            params: Vec::new(),
            sink: VarSink::default(),
            supers: Vec::new(),
            open: false,
            rep_formals: false,
            rep_depth: 0,
        }
    }

    /// A clause completed: claims the roots since the last clause. A
    /// count-bearing clause whose roots disagree marks the capture open.
    /// Mixin `on` constraints join the set — the official checks them
    /// (`element.superclassConstraints.forEach(checkOne)`,
    /// error_verifier.dart:8050-8052).
    pub(crate) fn clause(&mut self, slot: VarSlot, expected: Option<usize>) {
        self.rep_formals = false;
        if let Some(expected) = expected
            && self.sink.roots.len() != expected
        {
            self.open = true;
        }
        let trees = std::mem::take(&mut self.sink.roots);
        self.supers
            .extend(trees.into_iter().map(|tree| (slot, tree)));
    }
}

/// One container's finished header capture ([`Engine::finalize_variance`]'s
/// input).
pub(crate) struct VarContainer {
    pub(crate) name: StringId,
    pub(crate) params: Vec<(StringId, Option<Variance>, SourceRange)>,
    pub(crate) supers: Vec<(VarSlot, VarTree)>,
    pub(crate) open: bool,
}

/// The written-tree walk's depth budget: nesting beyond it answers
/// `unrelated` (the parameter is treated as absent — the under-report
/// direction), never recurses unbounded.
pub(crate) const VARIANCE_WALK_DEPTH: u32 = 64;

/// The variance of `parameter` in a written type — the
/// `computeVarianceInType` decision matrix over the capture's trees
/// (`variance_builder.dart:57-151`):
///
/// * a bare head naming the parameter: `covariant`;
/// * a parameterized head: the meet of
///   `declaredᵢ.combine(recursive)` over the arguments, the declaration's
///   parameter variances read from `table` (unknown heads read
///   all-covariant — every generic of the default SDK is legacy
///   covariant, element.dart:11560-11562); an arity mismatch or a poison
///   answers `unrelated` (the face suppresses);
/// * a function type: the meet of the return with
///   `contravariant.combine(…)` over the formals, forced `invariant` when
///   the parameter occurs in any bound of the signature's own type
///   parameters (variance_builder.dart:131-140);
/// * a record type: the meet over the fields;
/// * a poison blob: `unrelated`.
pub(crate) fn variance_in_tree(
    tree: &VarTree,
    parameter: StringId,
    table: &FxHashMap<StringId, Vec<Variance>>,
    depth: u32,
) -> Variance {
    if depth == 0 || tree.poisoned {
        return Variance::Unrelated;
    }
    match &tree.kind {
        VarTreeKind::Head { name, args } => {
            if args.is_empty() && *name == parameter {
                return Variance::Covariant;
            }
            let declared = table
                .get(name)
                .cloned()
                .unwrap_or_else(|| vec![Variance::Covariant; args.len()]);
            if declared.len() != args.len() {
                return Variance::Unrelated;
            }
            let mut result = Variance::Unrelated;
            for (variance, arg) in declared.iter().zip(args.iter()) {
                result = result.meet(variance.combine(variance_in_tree(
                    arg,
                    parameter,
                    table,
                    depth - 1,
                )));
            }
            result
        }
        VarTreeKind::Fn {
            ret,
            params,
            bounds,
        } => {
            let mut result = ret.as_ref().map_or(Variance::Unrelated, |ret| {
                variance_in_tree(ret, parameter, table, depth - 1)
            });
            for bound in bounds {
                if variance_in_tree(bound, parameter, table, depth - 1) != Variance::Unrelated {
                    result = Variance::Invariant;
                }
            }
            for param in params {
                result = result.meet(Variance::Contravariant.combine(variance_in_tree(
                    param,
                    parameter,
                    table,
                    depth - 1,
                )));
            }
            result
        }
        VarTreeKind::Record { fields } => {
            let mut result = Variance::Unrelated;
            for field in fields {
                result = result.meet(variance_in_tree(field, parameter, table, depth - 1));
            }
            result
        }
        VarTreeKind::Unit => Variance::Unrelated,
    }
}

/// A const-environment binding.
pub(crate) enum ConstBinding {
    /// The evaluated value of a completed const variable.
    Value(ConstantValue),
    /// The initializer currently being evaluated — a self-reference is a
    /// `recursive_compile_time_constant` cycle.
    InProgress,
    /// The initializer failed its own evaluation: every later read folds
    /// as a deferring constant and stays silent (the oracle's
    /// `InvalidConstant.avoidReporting` — the error was reported at the
    /// declaring initializer and never re-reports).
    Failed,
}

/// The callee classification a closed bare-identifier call resolves to
/// (`evaluation.dart` `_invalidConstantForMethodInvocation`).
pub(crate) enum CallClass {
    /// An in-file container with a declared const constructor — the
    /// implicit creation form checks its arguments.
    ConstCtor,
    /// A resolvable invocable name (top-level function/variable, local
    /// callable): the invocation itself faults.
    Invocable,
    /// A call through a LOCAL variable or formal parameter. The resolver
    /// rewrites the bare-identifier `p(...)` from `MethodInvocation` to
    /// `FunctionExpressionInvocation` (`_rewriteToFunctionExpressionInvocation`,
    /// resolution_visitor.dart — the `RelationalPatternResolutionTest
    /// .test_rewrite_operand` fixture), and `ConstantVisitor` has no
    /// `visitFunctionExpressionInvocation`: the node falls through to
    /// `visitNode`'s `InvalidConstant.genericError`, whose `invalidConstant`
    /// code is NOT on `_reportError`'s allow list — so the enclosing site's
    /// default code reports at the invocation (a map pattern key's
    /// `non_constant_map_pattern_key`, a relational operand's
    /// `non_constant_relational_pattern_expression`, a constant pattern's
    /// `constant_pattern_with_non_constant_expression`, a const
    /// initializer's `const_initialized_with_non_constant_value`) instead
    /// of `const_eval_method_invocation`.
    ValueCall,
    /// Unresolvable, or a container without a const constructor (the
    /// `const_with_non_const*` family) — out of subset, defer.
    Unknown,
}
/// One const-declaration initializer failure candidate's shape: the
/// verdict each entry re-decides against the whole-file const-ctor set.
pub(crate) enum ConstCreationEntry {
    /// An instance creation (`C(...)` / `C.named(...)`) inside the
    /// initializer — constant iff a const constructor answers the call.
    Creation { container: StringId },
    /// A checked argument of a const creation evaluated non-constant —
    /// the initializer fails at the argument unconditionally.
    ArgFailure,
}

/// Where an unresolved-callee creation staged at its own span sits in its
/// const initializer's evaluation — the oracle's failure-face site
/// (upstream Dart SDK source, pkg/analyzer/lib/src/dart/constant/
/// evaluation.dart `_valueOf`'s `isUnresolved` report path vs the
/// variable-face substitution in constant_verifier.dart's
/// `visitVariableDeclaration`).
pub(crate) enum CreationFailureSite {
    /// The creation is the initializer root: the evaluation error
    /// propagates to the variable, whose default substitution reports
    /// `const_initialized_with_non_constant_value` at the creation.
    Root,
    /// An element of the initializer's collection literal (`open` — the
    /// literal's opening bracket, UTF-16): the element evaluation aborts
    /// the literal, and the same variable face anchors at the first
    /// failing element; later elements' errors stay silent.
    Collection { open: u32 },
    /// An argument operand of a resolved const creation (`open` — the
    /// creation's `(`, UTF-16): the argument's evaluation reports each
    /// unresolved creation directly (`invalid_constant`, value.dart
    /// `genericError`), independent of sibling arguments.
    ConstCreation { open: u32 },
    /// An enclosing construct rejects the subtree wholesale (a non-const
    /// method invocation faults `const_eval_method_invocation` at the
    /// call without evaluating its arguments) — the creation's own error
    /// never surfaces.
    Other,
}

/// One formal's binding in the creation-time initializer walk (#1351): a
/// known constant value, a valid-but-opaque deference, or an invalid
/// (`dynamic`) binding.
pub(crate) enum CtorBinding {
    Value(ConstantValue),
    Opaque,
    Invalid,
}

/// The const-constructor initializer's evaluation scope: the creation's
/// arguments overlay the engine's own lookup (the official inlines the
/// constructor's initializers under the invocation's argument values).
pub(crate) struct CtorScope<'a, 'e> {
    pub(crate) engine: &'e Engine<'a>,
    pub(crate) env: &'e FxHashMap<StringId, CtorBinding>,
}
/// One deferred negative assignability verdict (#477 S1): the check ran on
/// the unpromoted parse-time types and staged instead of emitting; the
/// teardown re-decides it against the flow layer's promoted-read facts.
pub(crate) struct StagedCheck {
    /// The family: its re-check matrix and emission shape.
    pub(crate) kind: StagedKind,
    /// The source expression's own span — the join key. Only a fact on
    /// exactly this range can stand for the expression: a promoted-read
    /// fact spans the bare read's identifier token, and any compound shape
    /// (`x.foo`, `x + 1`) spans wider and misses.
    pub(crate) range: SourceRange,
    /// The parse-time source type (the declared type of an unpromoted
    /// read, or a compound expression's row type).
    pub(crate) from: Ty,
    /// The destination type (the check family's decided side).
    pub(crate) to: Ty,
    /// Whether `from` is a primitive literal (the int→double conversion
    /// fact [`Engine::assignable`] consumes).
    pub(crate) literal: bool,
}

/// The staging family ([`StagedCheck::kind`]).
pub(crate) enum StagedKind {
    /// `invalid_assignment` (`x = e` targets and declaration initializers).
    Assignment,
    /// `argument_type_not_assignable`.
    Argument,
    /// `return_of_invalid_type`: the host kind and executable name ride
    /// along (the message's third placeholder); `ctor` re-arms the
    /// declared-subtype negative at the teardown re-decision; `is_async`
    /// re-arms the flattened async matrix (`rtv.dart:258-289`) — the
    /// teardown re-decides through the same face the parse-time stage
    /// asked.
    Return {
        host: ExecKind,
        name: StringId,
        /// The message face's display name when it differs from `name`
        /// (a named factory's qualified `C.named`).
        display: Option<StringId>,
        ctor: bool,
        is_async: bool,
    },
    /// `receiver_of_type_never`: the receiver row's parse-time type rides
    /// in `from`; the teardown join substitutes a promoted-read fact staged
    /// on the receiver's exact span (the official read's flow-aware
    /// staticType).
    NeverReceiver,
    /// `unchecked_use_of_nullable_value`: the condition operand's
    /// parse-time type rode in `from`; the teardown join substitutes the
    /// flow's condition-type row at the operand read (the flow-aware
    /// static type the official verdict reads, #1349/#1351).
    NullableCondition,
    /// `yield_of_invalid_type`: the generator body's element verdict —
    /// the yield expression's promoted read re-decides at the teardown
    /// join, the same face [`StagedKind::Return`] stages (the official
    /// row reads the expression's flow-aware static type,
    /// `yield_statement_resolver.dart`'s `expression.staticType`).
    Yield,
    /// The `yield*` face: the star arm's parse-time raw type stands for
    /// the no-fact re-decision (byte-identical to the parse-time face),
    /// while a promoted-read fact on the source substitutes the promoted
    /// type itself — the official implied type IS the expression's
    /// resolved static type, unwrapped
    /// (`yield_statement_resolver.dart` `_checkForYieldOfInvalidType`
    /// sets `impliedReturnType = expressionType` for the isYieldEach ask,
    /// so a null-guard promotion cleans the verdict and a subtype head
    /// with its own fixed sequence instantiation answers through its
    /// clause).
    YieldEach,
    /// `for_in_of_invalid_type`: the iterable's parse-time type rides in
    /// `from`, the sequence type (`Iterable<dynamic>` / `Stream<dynamic>`)
    /// in `to`; a promoted-read fact on the iterable re-decides at the
    /// teardown join (`_checkForEachParts` reads the iterable's flow-aware
    /// static type — `if (v is List<Object>) for (var e in v)` is clean).
    ForIn,
}

/// A passing constant-pattern candidate (#1351): the parse-time check ran
/// against the declared scrutinee and answered "possible"; the flow's
/// promoted matched type (`getMatchedValueType`'s head) re-decides at the
/// teardown join.
pub(crate) struct PendingMatched {
    /// The constant pattern's own type.
    pub(crate) constant: Ty,
    /// The constant's span (the official row's anchor).
    pub(crate) at: SourceRange,
    /// The clause's `case` keyword anchor (the flow fact's join key).
    pub(crate) clause: SourceRange,
}

/// A deferred switch-covering verdict (#2538): the parse-time pass
/// computed `non_exhaustive` over a NULLABLE scrutinee, but the flow
/// layer's per-clause matched type (`FlowFact::MatchedType`, the
/// official `getMatchedValueType`) may promote the scrutinee (an
/// `if (n == null) return;` before the switch strips the null witness
/// the parse-time verdict reports). The row stages here instead of
/// reporting, and the teardown join re-decides it against the refined
/// matched type: exhaustive after refinement drops the row, anything
/// else re-renders it against the refined value.
pub(crate) struct PendingSwitchCovering {
    /// The covering code (statement vs expression face).
    pub(crate) code: SemanticCode,
    /// The `switch` keyword (the row's anchor).
    pub(crate) kw: SourceRange,
    /// The first case clause's anchor (the flow fact's join key).
    pub(crate) first_anchor: SourceRange,
    /// The parse-time scrutinee (the unrefined verdict's value).
    pub(crate) scrutinee: Ty,
    /// The parse-time row's rendered arguments.
    pub(crate) ty_text: String,
    pub(crate) unmatched: String,
    pub(crate) suggested: String,
    /// The private-enum template applies (the shared code under the private
    /// problem text, messages.yaml's `nonExhaustiveSwitch*Private` rows).
    pub(crate) private: bool,
    /// The case roots, re-decided against the refined value at teardown.
    pub(crate) spaces: Vec<crate::switch_tracker::Restriction>,
    pub(crate) guarded: Vec<bool>,
}

/// The promoted-read staging (#477 S1): the flow facts' promotion rows as
/// `(read span, promoted type)`, kept offset-sorted for the teardown join.
#[derive(Default)]
pub(crate) struct PromotionStaging {
    pub(crate) rows: Vec<(SourceRange, Ty)>,
}

/// The promoted-property-read spans (#1409): the flow facts' property
/// promotion rows, kept offset-sorted for the teardown join. Boolean rows —
/// the flow layer's promotion marker is the non-null face of a static type
/// it cannot spell, so the join substitutes the non-null face of the
/// parse-time type the staged check already carries.
#[derive(Default)]
pub(crate) struct PropPromotionStaging {
    pub(crate) rows: Vec<SourceRange>,
}

/// The single field-promotion eligibility verdict
/// ([`Engine::property_promotion_verdict`]) every promotion path and the
/// why-not-promoted context face share. Three outcomes, because the two
/// consumers split on the middle one: only an INHERENT non-promotability
/// renders a context row; a library-level name conflict is the official
/// non-inherent reason and renders none (this engine cannot enumerate the
/// conflicting declarations the official message names).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PropertyPromotionVerdict {
    /// A promotable instance field (private, final, not external, no
    /// same-named conflict in the library).
    Promotable,
    /// Non-promotable for a reason intrinsic to the member itself — the
    /// why-not-promoted context row names it.
    Inherent(PropertyNonPromotability),
    /// Non-promotable because a same-named non-final field or concrete
    /// getter exists elsewhere in the library (the official name-poisoning
    /// rule) — a non-inherent reason with no rendered context row.
    NameConflict,
}

/// One merged `receiver.member` read (B5): the facts the staged-join
/// needs after pass 2 emptied the live stack — the member's spelling, the
/// receiver classification (the container walk's entry), and the
/// receiver's read span (the property-access anchor).
#[derive(Clone, Copy)]
pub(crate) struct MemberRead {
    pub(crate) offset: u32,
    pub(crate) name: StringId,
    pub(crate) recv: Receiver,
    pub(crate) recv_span: SourceRange,
}

/// One non-promotion fact row in the diagnosing universe (#1877): the
/// flow layer's `WhyNotPromoted`/`NonPromotedPropertyRead`/`ThisNotPromoted`
/// payload, resolved to the reason rows the context producer renders.
pub(crate) struct StagedNonPromotion {
    /// The read's own span — the `ThisNotPromoted` row's anchor and the
    /// variable family's `<name>` source.
    pub(crate) read: SourceRange,
    /// The read's spelling (the variable or property identifier),
    /// interned — the `'<name>'`/`'<prop>'` of the rendered rows.
    pub(crate) name: StringId,
    /// The resolved reasons, potentially-nullable keys already dropped
    /// (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/resolver/type_analyzer.dart:783-791 —
    /// the consumer skips nullable keys and takes the FIRST contributing
    /// entry), most recent demotion first.
    pub(crate) reasons: Vec<NonPromotionReason>,
}

/// The `UnassignedRead` fact rows (#746), kept offset-sorted: the
/// definite-unassignment faces (`cast_from_nullable_always_fails`, the
/// dead-null-chain face) consume them. The flow fact unions two official
/// gates under one row —
/// `isDefinitelyUnassigned` (the `unassigned` bit) and the non-late
/// reads' `!isAssigned` (the potentially-unassigned join face) — so each
/// row carries its `definitely_unassigned` bit and the consumers split
/// by it: a variable assigned on one branch of an `if` is *possibly*
/// unassigned (the wide row fires) but not *definitely* unassigned, and
/// the official definite-unassignment faces (`cast_from_nullable_always_fails`,
/// `_verifyUnassignedVariable`'s dead chain) must not fire on it.
#[derive(Default)]
pub(crate) struct UnassignedStaging {
    /// `(span, definitely_unassigned)`, offset-sorted.
    pub(crate) rows: Vec<(SourceRange, bool)>,
}

impl UnassignedStaging {
    /// Stages one unassigned-read span with its definite-unassignment
    /// bit. One read event fires one fact, so the repeat insert is a
    /// no-op; a diverging repeat (a loop body walked under two states)
    /// keeps the first row.
    pub(crate) fn stage(&mut self, range: SourceRange, definitely_unassigned: bool) {
        if let Err(index) = self
            .rows
            .binary_search_by(|r| r.0.offset.cmp(&range.offset))
        {
            self.rows.insert(index, (range, definitely_unassigned));
        }
    }

    /// Whether the read at exactly `range` stood definitely unassigned —
    /// the official `isDefinitelyUnassigned` gate both consumer faces
    /// pin. A merely possibly-unassigned read (the wide row) answers
    /// `false`.
    pub(crate) fn staged(&self, range: SourceRange) -> bool {
        self.rows
            .binary_search_by(|r| r.0.offset.cmp(&range.offset))
            .is_ok_and(|index| self.rows[index].0 == range && self.rows[index].1)
    }
}

impl PropPromotionStaging {
    /// Stages one promoted-property span (#1409). One read event fires one
    /// fact, so the repeat insert is a no-op; a loop body walked under two
    /// states emits only when the promotion holds on both.
    pub(crate) fn stage(&mut self, range: SourceRange) {
        if let Err(index) = self.rows.binary_search_by(|r| r.offset.cmp(&range.offset)) {
            self.rows.insert(index, range);
        }
    }

    /// Whether the property read at exactly `range` stood promoted.
    pub(crate) fn staged(&self, range: SourceRange) -> bool {
        self.rows
            .binary_search_by(|r| r.offset.cmp(&range.offset))
            .is_ok_and(|index| self.rows[index] == range)
    }
}

impl PromotionStaging {
    /// Stages one promotion. One read event fires one fact, so equal
    /// offsets are not expected; a diverging repeat (a loop body walked
    /// under two states) would leave no single type that stands for the
    /// span — poisoned, the join misses, the unpromoted verdict stands.
    pub(crate) fn stage(&mut self, range: SourceRange, ty: Ty) {
        match self
            .rows
            .binary_search_by(|(r, _)| r.offset.cmp(&range.offset))
        {
            Ok(index) => {
                if self.rows[index] != (range, ty) {
                    self.rows.remove(index);
                }
            }
            Err(index) => self.rows.insert(index, (range, ty)),
        }
    }

    /// The promoted type of the read at exactly `range`, when one staged.
    pub(crate) fn promoted(&self, range: SourceRange) -> Option<Ty> {
        self.rows
            .binary_search_by(|(r, _)| r.offset.cmp(&range.offset))
            .ok()
            .and_then(|index| {
                let (staged, ty) = self.rows[index];
                (staged == range).then_some(ty)
            })
    }

    /// The promoted type of a read staged INSIDE `range` — the junction
    /// operand's retargeted extent (`a && !x` stages over `!x` while the
    /// flow's promotion fact anchors at the read `x` itself; upstream
    /// consults the flow model at that read, bool_expression_verifier
    /// .dart:45-73, and the oracle's own row anchors there too). The
    /// exact-span lookup above stays the first ask; this is the
    /// negation-operand rescue, not a general relaxation — one operand's
    /// extent only ever contains its own subexpression reads (#4265).
    pub(crate) fn promoted_within(&self, range: SourceRange) -> Option<Ty> {
        let mut index = self
            .rows
            .partition_point(|(r, _)| r.offset < range.offset);
        while let Some(&(staged, ty)) = self.rows.get(index) {
            if staged.offset >= range.end() {
                break;
            }
            if staged.end() <= range.end() {
                return Some(ty);
            }
            index += 1;
        }
        None
    }
}

/// The invalid-type name set (#2193): the names a `TypeUse` row's
/// push-time verdict snapshotted `Missing` or `NonType` — the oracle's
/// `InvalidType` positions. A promoted/read type spelling whose
/// argument list contains one of these names carries the oracle's
/// `InvalidType` argument, never a usable `Ty::Named` (the is-test/cast
/// target whose type-argument name failed the type verdict reports
/// `non_type_as_type_argument` and suppresses the typed check — a
/// `Ty::Named` argument here would feed member diagnostics and
/// promotion decisions the oracle never runs). Keyed by name, not
/// position: the verdict rows carry spans, but a flow argument name does
/// not — and a `Missing`/`NonType` verdict is scope-stable for these
/// rows (a use in a scope where the name resolves pushes no verdict
/// row).
pub(crate) type InvalidTypeNames = rustc_hash::FxHashSet<StringId>;

/// Collects the [`InvalidTypeNames`] of one unit: every `TypeUse` row
/// whose snapshot is `Missing` or `NonType` contributes its name.
pub(crate) fn invalid_type_names(rows: &[Row]) -> InvalidTypeNames {
    let mut set = InvalidTypeNames::default();
    for row in rows {
        if matches!(
            row.shape,
            RowShape::TypeUse(TypeVerdict::Missing | TypeVerdict::NonType, _)
        ) {
            set.insert(row.name);
        }
    }
    set
}

/// A flow fact's promoted type in the semantic lattice; `None` for the
/// promotions that never stage. A named flow type recovers its spelling
/// through [`Flow::name_lexeme`] (the S0 parallel name table) and interns
/// into the live string table, so repeat reads share one id.
///
/// The written-argument face (#1010 transport): the row carries the
/// promotion-chain head's written argument tree (`args`, an index into the
/// recorder's arena), and this function is the join point — the tree
/// interns bottom-up into `pool`, the diagnosing universe's argument pool,
/// so the staged type's slots are canonical in the same universe
/// [`Engine::filter_staged`]'s re-decision reads. A `void`/`Never`
/// argument anywhere in the tree makes the row unrepresentable in the M1
/// lattice and drops the whole staging (the head drop rule's argument
/// face — staging from a fiction would decide from nothing; the join
/// misses and the unpromoted verdict stands, the under-report direction).
/// Kept for the pinned call sites in `tests` (the invalid-name face is a
/// #2193 add — the pre-#2193 signature has no set to pass).
#[cfg(test)]
pub(crate) fn staged_promotion(
    ty: FlowTy,
    args: u32,
    flow: &Flow<'_>,
    strings: &StringTable,
    pool: &mut ArgPool,
) -> Option<Ty> {
    staged_promotion_typed(ty, args, flow, strings, &InvalidTypeNames::default(), pool)
}

/// [`staged_promotion`] gated on the unit's [`InvalidTypeNames`] (#2193):
/// a flow argument spelling whose name a `TypeUse` row snapshotted
/// `Missing`/`NonType` interns as `Ty::Unknown` — the oracle's
/// `InvalidType` argument — so the staged type never materializes a
/// usable `Ty::Named` over a failed type verdict.
pub(crate) fn staged_promotion_typed(
    ty: FlowTy,
    args: u32,
    flow: &Flow<'_>,
    strings: &StringTable,
    invalid: &InvalidTypeNames,
    pool: &mut ArgPool,
) -> Option<Ty> {
    let name = match ty.base {
        FlowBase::Name(id) => strings.intern(flow.name_lexeme(id)),
        FlowBase::Dynamic => return Some(Ty::Dynamic),
        FlowBase::Null => return Some(Ty::Null),
        // The signature-less function head (#2174): the flow lattice spells
        // no shape, so the promotion's re-decision substitutes the parse-time
        // declared type with its `?` stripped (`filter_staged`) — the real
        // promoted type the nullability decision produces.
        FlowBase::Function => return Some(Ty::UNKNOWN),
        FlowBase::Unknown => return Some(Ty::UNKNOWN),
        // A `void` promotion cannot arise (`tryPromoteToType` only
        // subtype-narrows, type_system.dart:1729) — the drop rule stands.
        FlowBase::Void => return None,
        // `Never` is a real flow type the consumer faces read: a `Null`
        // receiver promoted to non-null (`x != null` on `Null x` types the
        // read `Never`, promote_to_non_null's `Base::Null → NEVER` arm), an
        // `is Never` test, and an impossible-branch factor all stage it.
        // `typeOrThrow` answers `Never` there, so the staged-check join —
        // `receiver_of_type_never` first, and every assignability face the
        // bottom-type vacuously cleans (`Never <: T`, subtype.dart Right
        // Bottom) — needs the spelled type, not a drop. `Never?` spells
        // `Null` at this layer (the same fold resolved_ops.rs:6606 makes).
        FlowBase::Never => return Some(if ty.nullable { Ty::Null } else { Ty::Never }),
    };
    let args = intern_flow_args(args, flow, strings, invalid, pool)?;
    Some(Ty::Named {
        name,
        nullable: ty.nullable,
        args,
    })
}

/// Interns one flow argument tree into `pool` (bottom-up: nested lists
/// canonical before their outer list interns); `None` when a `void` or
/// `Never` argument makes the row unrepresentable (see
/// [`staged_promotion`]). `EMPTY_ARGS` short-circuits to the reserved
/// empty slot.
pub(crate) fn intern_flow_args(
    args: u32,
    flow: &Flow<'_>,
    strings: &StringTable,
    invalid: &InvalidTypeNames,
    pool: &mut ArgPool,
) -> Option<ArgListId> {
    if args == dartq_flow::ty::EMPTY_ARGS {
        return Some(ArgListId::EMPTY);
    }
    let mut list = Vec::with_capacity(flow.type_args_at(args).len());
    for entry in flow.type_args_at(args) {
        let arg = match entry.base {
            FlowBase::Name(id) => {
                let name = strings.intern(flow.name_lexeme(id));
                // An argument name whose type verdict failed is the
                // oracle's `InvalidType` slot (#2193).
                if invalid.contains(&name) {
                    Ty::UNKNOWN
                } else {
                    Ty::Named {
                        name,
                        nullable: entry.nullable,
                        args: intern_flow_args(entry.args, flow, strings, invalid, pool)?,
                    }
                }
            }
            FlowBase::Dynamic => Ty::Dynamic,
            FlowBase::Null => Ty::Null,
            FlowBase::Void | FlowBase::Never => return None,
            // The signature-less function head: no modeled shape for an
            // argument position — the unrepresentable-argument drop rule.
            FlowBase::Function | FlowBase::Unknown => Ty::UNKNOWN,
        };
        list.push(arg);
    }
    Some(pool.intern(&list))
}
/// One planned class-modifier emission: the code, its filled message
/// arguments (the outside-library family fills `{0}` with the super/ancestor
/// display name; the subtype family fills `{0}` with the subtype and `{1}`
/// with the explicit ancestor), and the anchor the oracle reports at — the
/// written supertype entry for the outside-library rows, the declaring name
/// for the subtype rows (#1289 E1).
pub(crate) struct ModifierFinding {
    pub(crate) code: ModifierCode,
    pub(crate) a0: String,
    pub(crate) a1: String,
    pub(crate) anchor: SourceRange,
}

/// The written clause-entry spans of one open container header, captured
/// positionally beside the collector's clause lists (#1289 E1): the same
/// header events, the same claim discipline (the extends head latches the
/// first entry while the super slot is open; the implements/`on` events
/// claim their trailing counts), so entry `i` of the element model's
/// `implements_clause`/`on_clause` pairs with span `i` here. The `with`
/// faces are captured only as unclaimed buffer — no consumer reads them
/// (the sealed family excludes `with` and the subtype rows anchor the
/// declaring name). A span missing at emission time (a claim/count
/// disagreement in a recovery shape, or an unmodelable entry) falls back to
/// the declaring name's span.
#[derive(Default)]
pub(crate) struct HeaderClauseSpans {
    pub(crate) extends: Option<SourceRange>,
    pub(crate) implements: Vec<SourceRange>,
    pub(crate) on: Vec<SourceRange>,
    /// Entries completed since the last claim (with-clause faces included).
    pub(crate) buffered: Vec<SourceRange>,
    /// The extends slot is open: the first entry latches as the head, the
    /// rest buffer (the collector's `header_super_open` shape).
    pub(crate) super_open: bool,
    /// The latched extends-head span, committed at the extends event.
    pub(crate) super_candidate: Option<SourceRange>,
    /// Type-argument nesting depth within the header window: entries are
    /// depth-0 completions only (`List<int>`'s `int` is an argument
    /// position, never an entry).
    pub(crate) arg_depth: u32,
    /// The outermost type-argument group that just closed, awaiting its
    /// head's completion re-fire: the entry span extends through the
    /// closer (`G<int>` anchors name through `>`; the re-fire carries any
    /// trailing `?`, which wins when present).
    pub(crate) args_closer: Option<SourceRange>,
}

/// The open container header's deferred class-modifier restriction state:
/// the checks' identity (`container`, the declaring name's span — the
/// subtype family's anchor), the declaring node's start (the `@sealed`
/// contract's anchor start), the use-site half's finalize flag, and the
/// clause-entry capture.
/// #1314 FAM14: the clause face an unresolved header-clause entry reports
/// through (scope_analyzer.dart `_resolveType`'s clause switch). The `on`
/// constraint is additive — the oracle keeps the resolver's
/// `undefined_class` beside it (probe: the recovered `implements B on A`
/// header carries both rows at `A`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClauseTag {
    /// `with M...` — `mixin_of_non_class`.
    With,
    /// `implements I...` — `implements_non_class`.
    Implements,
    /// The recovered extends-after-implements tail clause —
    /// `extends_non_class` (the `ExtendsClause` arm of the same clause
    /// switch).
    Extends,
    /// `on C...` — `mixin_super_class_constraint_non_interface`, additive
    /// over `undefined_class`.
    OnConstraint,
    /// A named mixin application's superclass (`class C = S with ...`) —
    /// `mixin_with_non_class_superclass`.
    AliasSuper,
}

/// #1444: the open extension-type header's clause-capture phase.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExtensionClausePhase {
    /// No extension-type header in flight.
    Off,
    /// Clause positions are live (`extends`/`with`/`implements`).
    Clauses,
    /// Inside the representation's formal list — its types are ordinary
    /// written types, not clause entries.
    Representation,
}

pub(crate) struct DeferredModifierChecks {
    pub(crate) container: Option<StringId>,
    pub(crate) name_range: SourceRange,
    pub(crate) decl_start: SourceRange,
    pub(crate) use_sites_done: bool,
    pub(crate) spans: HeaderClauseSpans,
}

/// The `dart:ffi` verifier's clause-entry anchors ([`super::ffi`]): the
/// written `extends` head, the `implements` entries, and the `with`
/// entries — recovered from [`HeaderClauseSpans`] at the use-site drain
/// (the with faces sit in `buffered`'s tail, claimed by nothing).
#[derive(Clone, Debug, Default)]
pub(crate) struct FfiClauseSpans {
    /// The `extends` entry (`None` when the header has no extends clause).
    pub(crate) extends: Option<SourceRange>,
    /// The `implements` entries in written order.
    pub(crate) implements: Vec<SourceRange>,
    /// The `with` entries in written order.
    pub(crate) withs: Vec<SourceRange>,
}

/// One invocation the `dart:ffi` verifier's post-parse pass
/// ([`super::ffi`]) may own, captured at the argument-list close: the
/// head's spelling (and the named-selector creation's selector segment,
/// resolved from the source at capture), the spans, the written type
/// arguments, and the arguments' types and shapes.
#[derive(Clone, Debug)]
pub(crate) struct FfiInvocation {
    /// The head spelling: the member-send selector (`fromFunction`,
    /// `asFunction`, `addressOf`, `sizeOf`, `create`, `elementAt`) or the
    /// bare creation head (`C()`'s `C`, `NativeCallable<...>.listener`'s
    /// `NativeCallable`).
    pub(crate) callee: StringId,
    /// A named-selector creation's selector segment (`listener` of
    /// `NativeCallable<T>.listener(f)`); `None` for plain sends and
    /// unnamed creations.
    pub(crate) selector: Option<StringId>,
    /// The span the oracle anchors whole-invocation rows at (the send
    /// head for a member send, the constructor name for a creation).
    pub(crate) head: SourceRange,
    /// The written `<...>` type argument after the head, source-parsed.
    pub(crate) type_arg: Option<FfiTypeArg>,
    /// `(name, shape)` per argument — positional arguments carry `None`
    /// names.
    pub(crate) args: Vec<(Option<StringId>, FfiArgShape)>,
    /// The receiver's static type for member sends (`p` of
    /// `p.asFunction`); [`Ty::Unknown`] when the row carried none.
    pub(crate) recv_ty: Ty,
    /// The invocation's own span (head through `)`), the
    /// whole-expression anchor.
    pub(crate) whole: SourceRange,
    /// The head is a bare creation head (no callee row linked the group).
    pub(crate) is_creation: bool,
}

/// The written type argument of a captured invocation (`<...>` after the
/// head), source-parsed: the send-side type-argument application pushes
/// no `TypeUse` rows, so the capture reads the bytes. The checks consume
/// the head spelling, the written display/span, and — when the text
/// reads `R Function(P...)` — the signature's head shapes; a bare
/// spelling's function-form answer (the alias-expanded face) re-derives
/// through the element model.
#[derive(Clone, Debug)]
pub(crate) struct FfiTypeArg {
    /// The written text, trimmed (the message display).
    pub(crate) display: Box<str>,
    /// The written span (inside the `<...>`).
    pub(crate) span: SourceRange,
    /// The written text's first identifier.
    pub(crate) head: StringId,
    /// `Some((return head, parameter heads))` when the text parses as
    /// `R Function(P...)`.
    pub(crate) fn_sig: Option<(StringId, Box<[StringId]>)>,
}

/// One argument's decided shape at the capture: the static type plus the
/// source facts the constant-argument faces read (`_isConst`'s
/// name-level stand-in — a literal or a const-constructor creation is
/// const; an invoked non-const head is not).
#[derive(Clone, Debug)]
pub(crate) struct FfiArgShape {
    /// The argument expression's own span.
    pub(crate) range: SourceRange,
    /// The argument's static type at the capture.
    pub(crate) ty: Ty,
    /// The argument reads as an invocation (`head(`…) — the shape the
    /// `argument_must_be_a_constant` face re-decides against the head's
    /// const-constructor bit.
    pub(crate) invocation_shaped: bool,
    /// The invocation-shaped argument's head spelling (a creation head or
    /// callee name); empty otherwise.
    pub(crate) head: StringId,
}

/// One `.address` member read, captured at the merge: the selector's
/// span plus the receiver facts the `address_position`/`address_receiver`
/// faces decide on — the receiver expression's own span, its static type,
/// its classification head's type (the chain root's declared type), and
/// whether the receiver is an index expression.
#[derive(Clone, Debug)]
pub(crate) struct FfiAddressRead {
    /// The `address` selector's span (the row anchor).
    pub(crate) selector: SourceRange,
    /// The receiver expression's own span (the leaf-invocation containment
    /// probe's subject).
    pub(crate) recv_span: SourceRange,
    /// The receiver's static type (the whole receiver expression).
    pub(crate) recv_ty: Ty,
    /// The receiver classification's head type: the declared type behind a
    /// `Typed` receiver (`data` of `data.address`, `struct` of
    /// `struct.y.address`'s chain root); [`Ty::Unknown`] elsewhere.
    pub(crate) head_ty: Ty,
    /// The receiver expression is an index (`data[0].address`).
    pub(crate) recv_is_index: bool,
}

/// One FFI-named metadata annotation (`@Native`, `@Array`, `@Packed`,
/// `@AbiSpecificIntegerMapping`, `@DefaultAsset`), source-parsed at its
/// close: the spans the verifier's rows anchor at plus the argument
/// faces its checks read.
#[derive(Clone, Debug)]
pub(crate) struct FfiDeclAnno {
    /// The annotation's whole span, `@` through the argument group's `)`
    /// (or through the name when argument-less).
    pub(crate) at: SourceRange,
    /// The annotation's written name (dot-joined for qualified spellings).
    pub(crate) name: StringId,
    /// The parsed argument face.
    pub(crate) face: FfiAnnoFace,
}

/// The source-parsed face of one FFI annotation's arguments: only the
/// shapes the owning code name reads are filled; every field defaults to
/// its absent form.
#[derive(Clone, Debug)]
pub(crate) struct FfiAnnoFace {
    /// The annotation name's own span (`@` excluded) — the
    /// `abi_specific_integer_mapping_extra`/`ffi_native_invalid_multiple_
    /// annotations`/duplicate-`@DefaultAsset` anchors.
    pub(crate) name_span: SourceRange,
    /// The argument group's span, `(` through `)`, when present.
    pub(crate) args: Option<SourceRange>,
    /// `@Native`/`@Packed`/`@Array`: the written `<T>` type argument
    /// (display text plus the parsed head shapes).
    pub(crate) sig: Option<FfiSigFace>,
    /// `@Native(isLeaf: true)`: the flag's written truth.
    pub(crate) is_leaf: bool,
    /// `@Array`: the dimension constants with their written spans (the
    /// variable dimension first, `0` standing in for `@Array.variable()`'s
    /// preset).
    pub(crate) dims: Vec<(i64, SourceRange)>,
    /// `@Array.variable*`: the variable-length posture.
    pub(crate) variable_length: bool,
    /// `@Packed`: the alignment literal and its span.
    pub(crate) alignment: Option<(i64, SourceRange)>,
    /// `@AbiSpecificIntegerMapping`: each map value's head type spelling
    /// with its written span.
    pub(crate) mapping_values: Vec<(String, SourceRange)>,
}

/// The written `<T>` of an `@Native` annotation: the display text plus the
/// parsed function-type shape (return head, per-parameter heads) when the
/// written form is `R Function(P...)`; a bare type keeps its head in
/// [`Self::ret_head`] with no parameters.
#[derive(Clone, Debug)]
pub(crate) struct FfiSigFace {
    /// The written type argument's own span (inside the `<...>`).
    pub(crate) range: SourceRange,
    /// The written text (the message display).
    pub(crate) display: Box<str>,
    /// The return type's head spelling (`Void`, `Int8`, `Pointer`,
    /// `Handle`).
    pub(crate) ret_head: StringId,
    /// Each normal parameter's head spelling, in order.
    pub(crate) param_heads: Box<[StringId]>,
}

/// The `_isLibraryInWorkspacePackage` face at the graph's uri level: a
/// declaring library sits in the diagnosed library's package when the two
/// canonical uris are `package:` form naming the same package
/// (`PubWorkspacePackage.contains`, workspace/pub.dart:546-554, the
/// package-config workspace). Any other uri pair answers cross-package —
/// the analyzed-set universe of the sealed contract is `package:` libraries
/// (the SDK synthetics carry no `@sealed`, and the oracle's file-scheme
/// containment branch, pub.dart:556-559, keys a path containment the graph
/// does not carry).
pub(crate) fn same_workspace_package(
    graph: &LibraryGraph,
    candidate: LibraryId,
    own: LibraryId,
) -> bool {
    let package = |lib: LibraryId| {
        graph
            .uri(lib)
            .strip_prefix("package:")
            .and_then(|rest| rest.split('/').next())
    };
    match (package(candidate), package(own)) {
        (Some(candidate), Some(own)) => candidate == own,
        _ => false,
    }
}

/// One resolved super face of the diagnosed container: the declaring
/// handle, its library (#491 S2a attribution), the clause the name was
/// written through, and the written entry's span (the outside-library
/// anchor; the declaring name's span when the capture has none).
pub(crate) struct ResolvedSuper {
    pub(crate) handle: (FileId, StringId),
    pub(crate) lib: LibraryId,
    pub(crate) relation: Relation,
    pub(crate) span: SourceRange,
}

/// The clause event's trailing-count claim over the buffered header entries
/// (the collector's `ClauseClaim::Trailing`): the last `count` entries join
/// the claiming clause, the earlier ones stay buffered for the next claim.
pub(crate) fn claim_trailing(
    claimed: &mut Vec<SourceRange>,
    buffered: &mut Vec<SourceRange>,
    count: usize,
) {
    let at = buffered.len().saturating_sub(count);
    claimed.extend(buffered.drain(at..));
}

/// The [`ModifierHierarchy`] over the fold's element index: declarations are
/// `(owner file, container name)` pairs resolved through the same namespace
/// hop the member walks use, with the walk order the official preorder
/// (extends head, `implements`, `with`, `on` — `base_or_final_type_verifier`
/// .dart:115-132). The effective-modifier memo computes the sealed-class
/// induced flags (`_setInducedModifier`, `instance_member_inferrer`
/// .dart:649-719) lazily over the same index; a cycle revisit answers
/// declared-only flags (a revisit cannot change the induced answer).
pub(crate) struct FoldHierarchy<'a> {
    pub(crate) graph: &'a LibraryGraph,
    pub(crate) namespaces: &'a Namespaces,
    pub(crate) index: &'a ElementIndex,
    pub(crate) file: FileId,
    pub(crate) lib: LibraryId,
    pub(crate) memo: RefCell<FxHashMap<(FileId, StringId), ClassModifiers>>,
    pub(crate) in_progress: RefCell<FxHashSet<(FileId, StringId)>>,
    /// The per-library `class-modifiers` language posture the engine injects
    /// per analyze ([`LibraryClassModifiers`]): a declaration in a library
    /// that predates the feature carries NO modifier facts — upstream gates
    /// the parser on it (`enableClassModifiers =
    /// _featureSet.isEnabled(Feature.class_modifiers)`, upstream Dart SDK
    /// source, `pkg/analyzer/lib/src/fasta/ast_builder.dart`), so the
    /// `base`/`final`/`interface`/`sealed`/`mixin class` tokens written in
    /// such a library only ever reported `experiment_not_enabled` and never
    /// reached the element model. [`Self::declared`] applies the mask at the
    /// one producer every modifier consumer reads.
    pub(crate) class_modifiers: &'a LibraryClassModifiers,
    /// The clause-face alias fold (`with ListMixin<E>` — SDK 3.47 spells
    /// `typedef ListMixin<E> = ListBase<E>`; the application's real face is
    /// ListBase): supplied by the engine-backed constructors so the
    /// supertype walk crosses alias-written clause faces exactly like
    /// [`crate::diagnose::Engine::resolve_clause_face`] does. `None` keeps
    /// the bare-world rigs on the direct resolution alone.
    #[expect(clippy::type_complexity)]
    pub(crate) alias_head:
        Option<&'a dyn Fn(StringId, FileId, LibraryId) -> Option<(FileId, StringId)>>,
}

/// One resolved super handle: the declaring file with the container name.
pub(crate) type FaceHandle = (FileId, StringId);

/// The super-handle columns: the extends head, then implements, with, on.
pub(crate) type FaceHandles = (
    Option<FaceHandle>,
    Vec<FaceHandle>,
    Vec<FaceHandle>,
    Vec<FaceHandle>,
);

impl FoldHierarchy<'_> {
    /// The declared modifiers of `handle`, or `None` when the name resolves
    /// to no container in the index (a broken face — suppress).
    ///
    /// The flags are masked to none when the DECLARING library predates the
    /// `class-modifiers` feature (`LibraryClassModifiers::is_enabled`, the
    /// `enabledIn 3.0.0` boundary the `sealed-class` gate shares): upstream
    /// parses such a library with the feature off, so a written
    /// `base`/`final`/`interface`/`sealed`/`mixin class` produces only
    /// `experiment_not_enabled` and the element model keeps no modifier —
    /// every restriction family then stays silent, which is exactly what the
    /// oracle answers for a pre-feature declaration consumed from any
    /// library. Masking at this producer covers all of them at once: the
    /// outside-library passes, the subtype verifier, the sealed induction
    /// below, and the explicit-base/final ancestor walk.
    pub(crate) fn declared(&self, handle: (FileId, StringId)) -> Option<ClassModifiers> {
        let (owner_file, name) = handle;
        let (container, _) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            owner_file,
            name,
        )?;
        let modifiers = container_modifiers(container);
        if self.class_modifiers.is_enabled(self.library_of(handle)) {
            return Some(modifiers);
        }
        Some(ClassModifiers {
            is_base: false,
            is_final: false,
            is_interface: false,
            is_sealed: false,
            is_mixin_class: false,
            ..modifiers
        })
    }

    /// The effective modifiers: declared flags plus the sealed-class induced
    /// flag computed from the direct faces' effective flags
    /// (`_setInducedModifier`), memoized per fold.
    pub(crate) fn effective(&self, handle: (FileId, StringId)) -> ClassModifiers {
        if let Some(&cached) = self.memo.borrow().get(&handle) {
            return cached;
        }
        let Some(mut mods) = self.declared(handle) else {
            return ClassModifiers::default();
        };
        // Only sealed classes carry induced modifiers (:651-653).
        if !mods.sealed_face() {
            self.memo.borrow_mut().insert(handle, mods);
            return mods;
        }
        if !self.in_progress.borrow_mut().insert(handle) {
            return mods; // cycle revisit: the outer frame completes the value
        }
        // The induction reads the extends head, the `implements` list, and
        // the `with` list (:655-718); `on` constraints do not participate
        // for the sealed classes that carry induced flags.
        let (supertype, interfaces, withs, _) = self.face_handles(handle);
        let supertype = supertype.map(|supertype| self.effective(supertype));
        let interfaces: Vec<ClassModifiers> = interfaces
            .iter()
            .map(|handle| self.effective(*handle))
            .collect();
        let mixins: Vec<ClassModifiers> =
            withs.iter().map(|handle| self.effective(*handle)).collect();
        match induced_modifier(&mods, supertype.as_ref(), &interfaces, &mixins) {
            Some(InducedModifier::Final) => mods.is_final = true,
            Some(InducedModifier::Base) => mods.is_base = true,
            Some(InducedModifier::Interface) => mods.is_interface = true,
            None => {}
        }
        self.in_progress.borrow_mut().remove(&handle);
        self.memo.borrow_mut().insert(handle, mods);
        mods
    }

    /// The resolved direct super handles: the extends head, then the
    /// `implements` list, then the `with` list, then the `on` constraints
    /// (base_or_final_type_verifier.dart:115-132). A super name resolves
    /// through its declaring container's library (the #491 S2a hop);
    /// unresolvable names drop out.
    pub(crate) fn face_handles(&self, handle: FaceHandle) -> FaceHandles {
        let (owner_file, name) = handle;
        let Some((container, _)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            owner_file,
            name,
        ) else {
            return (None, Vec::new(), Vec::new(), Vec::new());
        };
        let scope_lib = owner_library(self.graph, self.lib, owner_file, name).unwrap_or(self.lib);
        let hop = |name: StringId| {
            resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                scope_lib,
                owner_file,
                name,
            )
            .map(|(_, owner)| (owner, name))
            .or_else(|| {
                self.alias_head
                    .and_then(|fold| fold(name, owner_file, scope_lib))
            })
        };
        (
            container.superclass.and_then(&hop),
            container
                .implements_clause
                .iter()
                .filter_map(|name| hop(*name))
                .collect(),
            container
                .with_clause
                .iter()
                .filter_map(|name| hop(*name))
                .collect(),
            container
                .on_clause
                .iter()
                .filter_map(|name| hop(*name))
                .collect(),
        )
    }

    /// The hierarchy walk order: the extends head, then `implements`, then
    /// `with`, then `on`.
    pub(crate) fn walk_supertypes(&self, handle: (FileId, StringId)) -> Vec<(FileId, StringId)> {
        let (supertype, implements, withs, ons) = self.face_handles(handle);
        supertype
            .into_iter()
            .chain(implements)
            .chain(withs)
            .chain(ons)
            .collect()
    }

    /// The resolved supertype closure (the official `allSupertypes` face):
    /// BFS over [`Self::walk_supertypes`], cycle-guarded; the start is not
    /// included.
    pub(crate) fn closure(&self, start: (FileId, StringId)) -> Vec<(FileId, StringId)> {
        let mut out = Vec::new();
        let mut visited = FxHashSet::default();
        visited.insert(start);
        let mut queue = self.walk_supertypes(start);
        let mut cursor = 0;
        while cursor < queue.len() {
            let handle = queue[cursor];
            cursor += 1;
            if visited.insert(handle) {
                out.push(handle);
                queue.extend(self.walk_supertypes(handle));
            }
        }
        out
    }

    /// The library whose scope owns `handle`'s declaration (#491 S2a); the
    /// fold's own library when the probe cannot attribute it (under-report).
    pub(crate) fn library_of(&self, handle: (FileId, StringId)) -> LibraryId {
        let (owner_file, name) = handle;
        if owner_file == self.file {
            return self.lib;
        }
        owner_library(self.graph, self.lib, owner_file, name).unwrap_or(self.lib)
    }
}

impl ModifierHierarchy for FoldHierarchy<'_> {
    type Decl = (FileId, StringId);

    fn modifiers(&self, decl: Self::Decl) -> ClassModifiers {
        self.effective(decl)
    }

    fn supertypes(&self, decl: Self::Decl) -> Vec<Self::Decl> {
        self.walk_supertypes(decl)
    }
}

impl<'a> Engine<'a> {
    /// The `?`-suffixed face (`NullabilitySuffix.question`) — the
    /// dead-chain receiver gate: a written-nullable named type, not
    /// `dynamic` and not the `Null` type.
    pub(crate) fn is_nullable_ty(ty: Ty) -> bool {
        matches!(ty, Ty::Named { nullable: true, .. })
    }

    /// The potentially-nullable verdict (`isPotentiallyNullable = !
    /// isNonNullable`, upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/element/type_system.dart:1161-1179 — the
    /// receiver gate `type_property_resolver.dart`'s nullable branch and
    /// the bare-invocation face both read): true for every `?`-suffixed
    /// spelling (named — `Never?` included, a question-suffixed Never is
    /// NOT the `Null` instance and keeps the unchecked code — function,
    /// and record), and for a `?`-less type-parameter reference whose
    /// declared bound is nullable or absent (the implicit `Object?`,
    /// walked post-walk through the executable windows' capture
    /// [`Engine::param_bound_of`] — a container's own parameter spelling
    /// reads non-nullable, the closure-formal schema's leaked-parameter
    /// discipline). False for `dynamic`, `void`,
    /// and the sentinel — the top faces are suffix-less; and for the
    /// `Null` type itself: it IS potentially nullable upstream, but its
    /// dereference rows swap to `invalid_use_of_null_value`
    /// (`nullable_dereference_verifier.dart:55-58`), so it stays OUT of
    /// this predicate's consumers rather than routing `Null` receivers
    /// into the unchecked code.
    pub(crate) fn potentially_nullable(&self, ty: Ty) -> bool {
        self.potentially_nullable_at(ty, None)
    }

    /// [`Self::potentially_nullable`] with the reference's own offset
    /// where the caller knows it (a post-walk row always does): the
    /// parameter-bound lookup then reads the nearest declaring window at
    /// or before the reference instead of the first declaration.
    pub(crate) fn potentially_nullable_at(&self, ty: Ty, at: Option<u32>) -> bool {
        match ty {
            Ty::Named { nullable: true, .. }
            | Ty::Function { nullable: true, .. }
            | Ty::Record { nullable: true, .. } => true,
            Ty::Named {
                name,
                nullable: false,
                ..
            } => self.param_bound_potentially_nullable_at(name, at),
            _ => false,
        }
    }

    /// Whether a `?`-less type-parameter spelling reads potentially
    /// nullable through its declared bound (the `isNonNullable`
    /// TypeParameterType arm recursing): an unbounded parameter (the
    /// implicit `Object?`) and a nullable-or-top bound answer yes, a
    /// strictly non-nullable bound no. One level of parameter-to-parameter
    /// bounds (`T extends S`) walks through, the loop cap keeping a cyclic
    /// capture from hanging the pass (a cycle falls out non-nullable, the
    /// under-report direction). The bound table is the EXECUTABLE windows'
    /// post-walk capture alone: a container's own parameter spelling
    /// deliberately reads non-nullable here — under a raw receiver the
    /// callee's schema leaves such formals `dynamic` (the closure-formal
    /// solver's leaked-parameter discipline), and this layer cannot tell
    /// a leaked schema spelling from a user-annotated one (the
    /// under-report direction, the class-generic census rows).
    fn param_bound_potentially_nullable_at(&self, name: StringId, at: Option<u32>) -> bool {
        let mut next = self.exec_param_bound(name, at);
        for _ in 0..4 {
            let Some(ty) = next else { return false };
            match ty {
                Ty::Named { nullable: true, .. }
                | Ty::Function { nullable: true, .. }
                | Ty::Record { nullable: true, .. } => return true,
                Ty::Named {
                    name: param,
                    nullable: false,
                    ..
                } => {
                    if self.future_or_potentially_nullable(ty) {
                        return true;
                    }
                    next = self.exec_param_bound(param, at);
                }
                _ => return false,
            }
        }
        false
    }

    /// [`Engine::param_bound_of`] restricted to executable-owned windows,
    /// with the container-owned windows as the outer fallback (#2926 F4):
    /// a window whose owning name declares a CONTAINER in this unit is the
    /// container header's own capture (a class, mixin, or extension
    /// declaration latches `pending_exec_name` the same way a function
    /// does). It answers AFTER every executable window missed — the
    /// innermost scope's parameter wins (`class C<T> { void f<T>(T t) }`'s
    /// `f`-window `T` shadows the header's). The container fallback feeds
    /// the receiver nullability question (`(t).foo` where `t` is a
    /// method's `T`-spelled formal and `T` is the class's own parameter —
    /// an unbounded `T` reads its implicit `Object?` bound, the
    /// potentially-nullable verdict upstream's `isNonNullable` gives
    /// through the `TypeParameterType` bound). The callee-schema caveat
    /// the exclusion documented stands only where a leaked formal could
    /// spell `T` without a written `T` annotation — that face binds
    /// `dynamic`, never `Named { T }`, so the window answer stays honest
    /// wherever this predicate actually asks it.
    /// Iteration order is [`Engine::exec_windows`]' declaration order —
    /// the single ordering a name-only lookup may read (#4202: a
    /// `FxHashMap` scan flips verdicts between runs because parallel
    /// interning moves `StringId` values, and with them hash order); `at`
    /// (the reference offset) picks the nearest declaring window at or
    /// before the reference, the first declaration otherwise.
    fn exec_param_bound(&self, name: StringId, at: Option<u32>) -> Option<Ty> {
        let implicit_object = || Ty::Named {
            name: self.intern("Object"),
            nullable: true,
            args: ArgListId::EMPTY,
        };
        let mut container_owned = None;
        let mut first_exec = None;
        let mut nearest_exec = None;
        for window in &self.exec_windows {
            let Some((_, bound)) = window.params.iter().find(|(param, _)| *param == name) else {
                continue;
            };
            let bound = bound.unwrap_or_else(implicit_object);
            if self
                .index
                .get(self.file)
                .is_some_and(|elements| elements.container(window.owner).is_some())
            {
                container_owned.get_or_insert(bound);
                continue;
            }
            first_exec.get_or_insert(bound);
            if at.is_some_and(|at| window.at < at) {
                nearest_exec = Some(bound);
            }
        }
        if let Some(bound) = nearest_exec.or(first_exec) {
            return Some(bound);
        }
        // A container's own type-parameter spelling (`class C<T extends
        // num?>` member face — `t.isFinite` over `T t`): its bound lives in
        // the container capture, never an executable window, so the loop
        // above missed it. The captured bound decides exactly like an
        // executable's — `num?` reads nullable, `num` does not — and an
        // unbounded slot keeps the implicit `Object?` face. The
        // leaked-schema caveat the doc above carries stays honored: a
        // spelling no own container declares still falls to
        // `container_owned` (the under-report direction).
        if container_owned.is_none()
            && let Some(bound) = self.bounds.container_param_bound(name, at)
        {
            return Some(bound.map_or_else(implicit_object, |tree| self.tree_ty(&tree)));
        }
        container_owned
    }

    /// A `?`-suffixed named type joined onto an `e!` junction is its
    /// non-null projection; every other shape is already its own
    /// `promoteToNonNull` form (`dynamic` stays, `Null` has no modeled
    /// `Never` and keeps the under-report direction). The suffixed
    /// function and record shapes strip the suffix the same way (#2174 —
    /// the staged-promotion re-decision reads the same projection).
    pub(crate) fn non_null_form(ty: Ty) -> Ty {
        match ty {
            Ty::Named {
                name,
                nullable: true,
                args,
            } => Ty::Named {
                name,
                nullable: false,
                args,
            },
            Ty::Function {
                shape,
                nullable: true,
            } => Ty::Function {
                shape,
                nullable: false,
            },
            Ty::Record {
                shape,
                nullable: true,
            } => Ty::Record {
                shape,
                nullable: false,
            },
            other => other,
        }
    }

    /// The `?`-joined face of a written type (`makeNullable`): the named,
    /// record, and function shapes take the suffix, every other shape is
    /// its own nullable form (`dynamic`/`void` are suffix-less tops).
    pub(crate) fn nullable_form(ty: Ty) -> Ty {
        match ty {
            Ty::Named { name, args, .. } => Ty::Named {
                name,
                nullable: true,
                args,
            },
            Ty::Record { shape, .. } => Ty::Record {
                shape,
                nullable: true,
            },
            Ty::Function { shape, .. } => Ty::Function {
                shape,
                nullable: true,
            },
            other => other,
        }
    }

    /// The `unnecessary_type_check` deferred re-decision (#3005): the
    /// pass-1 `check_type_check` runs before the flow replay materializes
    /// `PromotedRead`/`ReadType`, so an `is`/`is!` whose operand read
    /// stands promoted is decided on the declared face — the under-report
    /// the two #3002 pins close (`int? q = 0; q is int`, `o is String`
    /// inside `if (o is String)`). Upstream's verdict reads the operand's
    /// flow-aware static type (`leftNode.typeOrThrow`,
    /// best_practices_verifier.dart:1011), so once the read rows exist the
    /// verdict re-runs here against the resolved flow type.
    ///
    /// The replay reuses `check_type_check` itself: the `keyword_operand`
    /// slot carries the flow-typed operand (the same channel the
    /// `this`/`super` face uses — the operand's flow type supplied
    /// externally), and `armed_ty` carries the checked type reconstructed
    /// from the token table (resolved type heads push no rows, so the
    /// spelling is re-derived — bare/prefixed/generic/nullable heads only;
    /// `Function()` and record spellings bail, the under-report direction
    /// which is still better than the parse-time silence they replace).
    /// Emissions ride `stage_usage` into `report.usage`, merged with the
    /// rest of the verifier tail in source order. A site already holding
    /// one of this walk's rows (`unnecessary_type_check`,
    /// `type_check_with_null`, `type_check_with_not_null` — the three codes
    /// `check_type_check`/`stage_type_check_null` emit) at the same span
    /// dedupes: the oracle emits at most one row per `is` expression and a
    /// promotion can only flip the verdict, never stack a second row.
    pub(crate) fn deferred_type_checks(&mut self, promotions: &PromotionStaging) {
        use dartq_scanner::{Keyword, TokenKind};
        const GLUE: [TokenKind; 7] = [
            TokenKind::Period,
            TokenKind::QuestionPeriod,
            TokenKind::PeriodPeriod,
            TokenKind::QuestionPeriodPeriod,
            TokenKind::Minus,
            TokenKind::Bang,
            TokenKind::Tilde,
        ];
        let tokens = self.tokens.as_slice();
        for i in 0..tokens.len() {
            if tokens[i].kind != TokenKind::Keyword(Keyword::Is) {
                continue;
            }
            // `is!` carries a separate `!` token; the type head follows it.
            let negated = tokens.get(i + 1).is_some_and(|t| t.kind == TokenKind::Bang);
            let type_idx = i + 1 + usize::from(negated);
            // The operand must be a bare identifier read: the token left of
            // `is` is the read's identifier, and no member/prefix-operator
            // token glues it into a wider operand (`x.f is T`, `-x is T`,
            // `x as T is S` — the `as`/`is` target tokens left of the read
            // carry no read row and drop out at the join anyway).
            let Some(operand) = i
                .checked_sub(1)
                .and_then(|j| tokens.get(j))
                .filter(|t| t.kind == TokenKind::Identifier)
            else {
                continue;
            };
            if i >= 2 && GLUE.contains(&tokens[i - 2].kind) {
                continue;
            }
            // The read's span in UTF-16 (the join keys' convention) —
            // byte_len coincides on ASCII identifiers; a wide-char
            // identifier's utf16 length differs, so walk the lexeme.
            let span = {
                let text = &self.source[operand.byte_range()];
                SourceRange {
                    offset: operand.utf16_offset,
                    length: text.chars().map(char::len_utf16).sum::<usize>() as u32,
                }
            };
            // The operand's flow-aware static type: a `PromotedRead` fact
            // at the read's own span substitutes outright (an opaque
            // `Unknown` marker is the nullability-only face — defer to
            // the read-type row, which spells the type); otherwise the
            // `ReadType` row answers (the promoted or declared flow type —
            // covering write promotions `x ??= SubImpl` that stage no
            // `PromotedRead` fact). No row → not a promotable read →
            // nothing changed since the parse-time verdict.
            let operand_ty = match promotions.promoted(span) {
                Some(ty) if ty != Ty::Unknown => ty,
                _ => match self
                    .read_types
                    .binary_search_by(|(r, _)| r.offset.cmp(&span.offset))
                {
                    Ok(join) if self.read_types[join].0 == span => self.read_types[join].1,
                    _ => continue,
                },
            };
            // The read row spells `p.T` heads as written (`x: core.List`
            // stages `core.List`): canonicalize to the element name before
            // the invalid-face gate and the verdict — the same-element
            // rewrite `boundary.ty` already performs for foreign faces.
            let operand_ty = self.boundary.canonical_dotted_ty(operand_ty);
            if operand_ty == Ty::Unknown || self.is_invalid_ty(operand_ty) {
                continue;
            }
            let Some((checked, type_end)) = self.reconstruct_is_ty(type_idx) else {
                continue;
            };
            // The checked side's `p.T` heads canonicalize the same way —
            // `x is core.List` over `x: List` is always-true upstream.
            let checked = self.boundary.canonical_dotted_ty(checked);
            // A type-parameter spelling (`b is Box<String>` over
            // `b: Box<T>`) cannot re-decide post-walk: `type_params`/
            // `pending_type_params` are torn down, so the bound arm of
            // `check_type_check`'s verdict reads an empty scope — a param
            // head lands as an unresolved name and the vacuous-subtype arm
            // answers true where the parse-time verdict (and the oracle)
            // stayed silent (#2462's conservative pin). The gate covers
            // the operand's whole spelling and the checked side's
            // ARGUMENTS (the checked head is already the resolved
            // written name — a prefixed `p.C` leaf needs no unqualified
            // scope): any named head failing `resolve_bare`'s
            // declaration face — a scope param, a local read, a prefix,
            // `this`, a miss — keeps the site's parse-time verdict.
            if self.ty_has_scope_head(operand_ty) || self.ty_args_have_scope_head(checked) {
                continue;
            }
            // The verdict anchors at the whole `is` expression — operand
            // start through the reconstructed type end, the same extent
            // `verdict_at` takes in the listener.
            let at = SourceRange {
                offset: span.offset,
                length: type_end.saturating_sub(span.offset),
            };
            // One row per `is` expression: pass-1's emissions at this span
            // (either polarity of `unnecessary_type_check`, or the
            // `is Null` family's rows) stand — a promotion only re-decides
            // silence.
            if self.report.usage.iter().any(|d| {
                d.range == at
                    && matches!(
                        d.code.name,
                        "unnecessary_type_check"
                            | "type_check_with_null"
                            | "type_check_with_not_null"
                    )
            }) {
                continue;
            }
            let saved = self.armed_ty.replace(checked);
            self.check_type_check(at, Some((operand_ty, span)), negated);
            self.armed_ty = saved;
        }
    }

    /// Re-derives the `is`/`is!` target's written `Ty` from the token
    /// table for [`Self::deferred_type_checks`] — the pass-1 `armed_ty` is
    /// consumed by the verdict and resolved heads push no rows, so the
    /// checked spelling must be rebuilt (the ticket's "type_uses keeps
    /// (range, head_name) not resolved Ty" gap). Covers the bare
    /// (`int`/`Null`/`dynamic`), prefixed (`p.T` → the resolved leaf, the
    /// substitution `check_type_check` itself makes), generic (`T<args>`,
    /// nested; `>>`/`>>=` multi-closers counted by `carry`), and `?`
    /// (adjacent only — `x is int ? a : b` keeps the conditional out).
    /// Function-type (`int Function(...)`) and record (`(a, b)`) spellings
    /// return `None` — the verdict would misread a truncated head; the
    /// site keeps its parse-time silence instead. Returns the type and the
    /// written extent's utf16 end (the verdict span's right edge).
    fn reconstruct_is_ty(&self, start: usize) -> Option<(Ty, u32)> {
        use dartq_scanner::TokenKind;
        let tokens = self.tokens.as_slice();
        let mut carry = 0u32; // half-consumed `>>`/`>>>` closers
        let (ty, next) = self.reconstruct_ty_at(start, &mut carry)?;
        if carry != 0 {
            return None;
        }
        let last = tokens.get(next.wrapping_sub(1))?;
        let end = self.utf16_of((last.byte_offset + last.byte_len) as usize);
        // A token that could continue the type (`Function` for a
        // function-type tail, `(`/`[` for a record or indexed spelling)
        // means the reconstruction is truncated — bail rather than judge
        // the wrong head. Everything else (statement separators,
        // operators, `{`/`)`/`]`/`>`) is a legal end.
        if tokens.get(next).is_some_and(|t| {
            matches!(t.kind, TokenKind::OpenParen | TokenKind::OpenSquare)
                || matches!(t.kind, TokenKind::Identifier | TokenKind::Keyword(_))
                    && &self.source[t.byte_range()] == "Function"
        }) {
            return None;
        }
        Some((ty, end))
    }

    /// Whether `ty` names a head the post-walk verdict cannot scope —
    /// a type parameter, a local read, an import prefix, `this`, or an
    /// unresolved name — the spellings [`Self::deferred_type_checks`]
    /// refuses to replay (the parse-time scope the param/bound verdict
    /// arms need is gone; a vacuous re-decision would over-report where
    /// the oracle stays silent). Declaration-resolving heads (`Decl`,
    /// `Opaque`) pass; argument trees walk recursively.
    fn ty_has_scope_head(&self, ty: Ty) -> bool {
        match ty {
            Ty::Named { name, .. } => {
                if matches!(
                    self.resolve_bare(name).1,
                    RefTarget::Local
                        | RefTarget::TypeParam
                        | RefTarget::Prefix
                        | RefTarget::This
                        | RefTarget::Missing
                ) {
                    return true;
                }
                // A `Decl` answer needs the same kind probe the
                // member-face walk uses (`member_free_params`,
                // resolution.rs:1640-1645): a type parameter is an
                // element-model declaration, so `resolve_bare` can land
                // `Decl` on a scope spelling — the bound arm still
                // cannot read it post-walk.
                if self
                    .index
                    .get(self.file)
                    .and_then(|f| f.decl(name))
                    .is_some_and(|decl| decl.kind == crate::model::MicroKind::TypeParam)
                {
                    return true;
                }
                self.ty_args_have_scope_head(ty)
            }
            _ => false,
        }
    }

    /// The argument-tree half of [`Self::ty_has_scope_head`]: each arg
    /// gets the full head check (a `T` argument is a scope spelling; the
    /// checked head itself is exempt — see the call site).
    fn ty_args_have_scope_head(&self, ty: Ty) -> bool {
        let Ty::Named { args, .. } = ty else {
            return false;
        };
        if args == ArgListId::EMPTY {
            return false;
        }
        let args = self.boundary.args.borrow().get(args).to_vec();
        args.iter().any(|&arg| self.ty_has_scope_head(arg))
    }

    /// One type spelling inside [`Self::reconstruct_is_ty`]'s token window
    /// (recursive for `<` arguments). Returns `(ty, next_index)` — the
    /// index of the first token the type does not own. `carry` is the
    /// shared half-consumed-`>` count: a nested list whose closer lexed
    /// multi-wide (`>>`) leaves a pending `>` for the enclosing list.
    /// The irrefutable-destructure walk (`scopes_bodies`) reuses it for
    /// the written element/key/value/object-head spellings.
    pub(crate) fn reconstruct_ty_at(&self, start: usize, carry: &mut u32) -> Option<(Ty, usize)> {
        use dartq_scanner::TokenKind;
        let tokens = self.tokens.as_slice();
        let head = *tokens.get(start)?;
        let text = &self.source[head.byte_range()];
        let mut idx = start + 1;
        // `dynamic`/`Null` keep their dedicated `Ty` faces (the
        // `handle_type` table, listener.rs:3655-3663).
        let mut name = match text {
            "dynamic" => return Some((Ty::Dynamic, idx)),
            "Null" => return Some((Ty::Null, idx)),
            _ => self.strings.intern(text),
        };
        if !matches!(head.kind, TokenKind::Identifier | TokenKind::Keyword(_)) {
            return None;
        }
        // `p.T`: the prefixed head resolves to its leaf segment — the
        // same substitution `check_type_check` makes against the last
        // TypeUse row at parse time.
        if tokens.get(idx).is_some_and(|t| t.kind == TokenKind::Period) {
            let leaf = tokens.get(idx + 1)?;
            if !matches!(leaf.kind, TokenKind::Identifier | TokenKind::Keyword(_)) {
                return None;
            }
            name = self.strings.intern(&self.source[leaf.byte_range()]);
            idx += 2;
        }
        // The type-arguments group: `T<a, b<c>, ...>` — each argument
        // recurses; a closer lexed multi-wide (`>>`, `>>>`, `>>=`, `>=`)
        // repays through `carry` so the outer list sees its own `>`.
        let mut args: Vec<Ty> = Vec::new();
        if tokens.get(idx).is_some_and(|t| t.kind == TokenKind::Lt) {
            idx += 1;
            loop {
                if *carry > 0 {
                    *carry -= 1;
                    break;
                }
                match tokens.get(idx).map(|t| t.kind) {
                    Some(TokenKind::Gt) => {
                        idx += 1;
                        break;
                    }
                    Some(TokenKind::GtGt) => {
                        *carry += 1;
                        idx += 1;
                        break;
                    }
                    Some(TokenKind::GtGtGt) => {
                        *carry += 2;
                        idx += 1;
                        break;
                    }
                    // `>=`/`>>=` close the group and leave a `=`/`>` the
                    // type never owns — same legal end as `>`.
                    Some(TokenKind::GtEq | TokenKind::GtGtEq | TokenKind::GtGtGtEq) => {
                        idx += 1;
                        break;
                    }
                    Some(TokenKind::Comma) => {
                        idx += 1;
                    }
                    _ => {
                        let (arg, next) = self.reconstruct_ty_at(idx, carry)?;
                        args.push(arg);
                        idx = next;
                    }
                }
            }
        }
        // `T?`: the suffix attaches only adjacent — `x is int ? a : b`
        // keeps the conditional's `?` out (a whitespace gap separates
        // them; the parser's own ambiguity rule reads the same way).
        let mut nullable = false;
        let last = tokens.get(idx - 1)?;
        let end = self.utf16_of((last.byte_offset + last.byte_len) as usize);
        if tokens
            .get(idx)
            .is_some_and(|t| t.kind == TokenKind::Question && t.utf16_offset == end)
        {
            nullable = true;
            idx += 1;
        }
        let args = if args.is_empty() {
            ArgListId::EMPTY
        } else {
            self.boundary.intern_list(&args)
        };
        Some((
            Ty::Named {
                name,
                nullable,
                args,
            },
            idx,
        ))
    }

    /// The const-ness of one redirect target constructor — the oracle's
    /// `redirectedElement.isConst` in `_checkForRedirectToNonConstConstructor`
    /// (error_verifier.dart:7330-7347): `Some(true)`/`Some(false)` when the
    /// target resolves to a constructor face of this file, `None` when it
    /// resolves to no constructor at all (the oracle's null
    /// `redirectedElement`: the gate never reports).
    ///
    /// An enum's generative constructors are implicitly const — with the
    /// primary-constructors feature a generative constructor in an enum body
    /// may drop the `const` keyword, and an enum primary constructor carries
    /// it implicitly (`const_constructor_with_non_final_field`'s upstream
    /// sibling rule, upstream `enum_implicit_const_constructor_test`). The
    /// written `const` bit alone would read a `const`-less enum constructor
    /// as non-const, and a primary constructor holds no declaration site at
    /// all — both over-report a `const` redirect inside the enum. Every
    /// generative target face inside an enum therefore reads const; an enum
    /// factory keeps its own written bit (its constness is declarative).
    pub(crate) fn redirect_target_is_const(
        &self,
        container: StringId,
        selector: StringId,
    ) -> Option<bool> {
        let resolved = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        );
        // A foreign container's constructor set is not modeled here (the
        // under-report posture the gate already held).
        let decl = resolved.and_then(|(decl, owner)| (owner == self.file).then_some(decl));
        let enum_target = decl.is_some_and(|decl| decl.kind == ContainerKind::Enum);
        if let Some(site) = self.ctor_decl_site(container, selector) {
            return Some(site.const_ctor || (enum_target && !site.is_factory));
        }
        let decl = decl?;
        if !self.ctor_face_exists(decl, container, selector) {
            return None;
        }
        // No declaration site: the implicit unnamed default is never const
        // upstream, while an enum's is the implicit const constructor its
        // values are instances of.
        Some(enum_target)
    }

    /// The declarative const-constructor faces of the container that just
    /// closed — `_checkForConstConstructorWithNonFinalField`
    /// (error_verifier.dart:4132-4160, reached from the two constructor
    /// visitors) and the enum `with`-clause face below, which shares this
    /// call site because `flush_init_instance_fields` (type_capture.rs) is
    /// the one declaration-close hook every container kind reaches.
    ///
    /// The `const_constructor_with_mixin_with_field[s]` sibling reports from
    /// the whole-unit mixin-application post-pass instead
    /// (`enum_verifier.rs`'s `verify_const_constructor_mixin_fields`) —
    /// upstream decides it first and then stops, so THIS pass stands its own
    /// row down whenever the mixin list is non-empty (the pre-emption,
    /// mirrored rather than re-decided). The anchor of both is the
    /// constructor's `errorRange`: a secondary head's type name through its
    /// selector, a primary head's `const` keyword through the constructor
    /// name.
    pub(crate) fn flush_const_ctor_non_final_face(&mut self, container: StringId) {
        let Some(decl) = self
            .index
            .get(self.file)
            .and_then(|elements| elements.container(container))
        else {
            return;
        };
        // The enum `with`-clause face is not constructor-driven: it reports
        // per mixin entry, once per declaration close, and it needs the
        // clause spans this pass still holds (the teardown consumes them).
        if decl.kind == ContainerKind::Enum {
            self.flush_enum_mixin_instance_variable(container, decl);
        }
        // `enclosingElement is! ClassElement` exempts enums, mixins, and
        // extension types (:4105-4108), and `hasNonFinalField` gates the rest.
        if decl.kind != ContainerKind::Class
            || !self.face_has_non_final_field(
                self.file,
                self.lib,
                container,
                &mut FxHashSet::default(),
            )
        {
            return;
        }
        if self.mixin_declares_instance_field(decl, container) {
            return;
        }
        for site in self.const_ctor_sites(container) {
            if self.ctor_has_non_const_super(decl, site.head) {
                continue;
            }
            self.emit(
                SemanticCode::ConstConstructorWithNonFinalField,
                site.range,
                "",
                "",
            );
        }
    }

    /// One const generative constructor's anchors: the oracle's `errorRange`
    /// (the row's span) and the head token (the initializer-list scan's
    /// start).
    fn const_ctor_sites(&self, container: StringId) -> Vec<ConstCtorSite> {
        let mut sites: Vec<ConstCtorSite> = Vec::new();
        for (index, site) in self.decl_sites.iter().enumerate() {
            // A factory is not generative (upstream `isGenerative` false), an
            // augment declaration carries no row of its own, and every site
            // without a latched head anchor (nameless recovery) stays out.
            if site.is_factory
                || site.is_augment
                || !matches!(site.kind, DeclKind::Constructor { container: c } if c == container)
            {
                continue;
            }
            let Some(head) = self.ctor_head_anchors.get(&index).copied() else {
                continue;
            };
            let range = if matches!(self.lexeme_at(head.offset), Some("(" | ".")) {
                // A primary constructor's const bit lives on the class
                // header, not the constructor (the site opens before the
                // keyword is visited): the written `const` decides.
                let Some(range) = self.primary_ctor_error_range(container, head) else {
                    continue;
                };
                if self.lexeme_at(range.offset) != Some("const") {
                    continue;
                }
                range
            } else {
                if !site.const_ctor {
                    continue;
                }
                if site.range.offset > head.offset {
                    // A named secondary: the type name through the selector
                    // (the site range was rewritten to the selector token).
                    SourceRange {
                        offset: head.offset,
                        length: site.range.end() - head.offset,
                    }
                } else {
                    // An unnamed secondary (`C(...)`, `new(...)`): the head
                    // token IS the oracle's whole `errorRange`.
                    head
                }
            };
            sites.push(ConstCtorSite { range, head });
        }
        sites
    }

    /// Stages one `const_constructor_with_mixin_with_field[s]` row at
    /// `range` into the usage tail (the post-pass rows merge source-ordered
    /// with the parse-time ones). The singular and plural phrasings share one
    /// diagnostic code (upstream `diag.constConstructorWithMixinWithField`
    /// and `constConstructorWithMixinWithFields` both carry the `name`
    /// `const_constructor_with_mixin_with_field`,
    /// diagnostic.g.dart:2620-2653), so only the singular text comes from the
    /// table and the plural one is composed here; the attached correction
    /// stays the table's singular text until the table grows the plural row.
    /// Whether one of the container's `with`-clause faces declares an
    /// instance field — the `const_constructor_with_mixin_with_field[s]`
    /// pre-emption of this file's non-final-field face
    /// (`_checkForConstConstructorWithNonConstSuper`'s mixin loop,
    /// error_verifier.dart:4053-4085; the row itself is staged by the
    /// whole-unit post-pass, `enum_verifier.rs`'s
    /// `verify_const_constructor_mixin_fields`).
    ///
    /// The set is each written mixin's DECLARED fields
    /// (`enclosingClass.mixins[*].element.fields`): a field counts unless it
    /// is static or the abstract+final pair (an abstract final field is a
    /// getter alone), while an `external` field counts — this loop carries no
    /// `isExternal` test. Inheritance never joins here: a field the mixin
    /// merely inherits belongs to `hasNonFinalField`, whose row the oracle
    /// reports for such a class instead of this one.
    fn mixin_declares_instance_field(&self, decl: &Container, container: StringId) -> bool {
        if decl.with_clause.is_empty() {
            return false;
        }
        let scope_lib = self
            .owner_library_of(self.file, container)
            .unwrap_or(self.lib);
        decl.with_clause.iter().any(|&mixin| {
            self.resolve_clause_face(container, self.file, scope_lib, mixin)
                .is_some_and(|(face, _)| {
                    face.members.iter().any(|(_, entry)| {
                        entry.kind == MemberKind::Field
                            && !entry.is_static
                            && !(entry.is_abstract && entry.is_final)
                    })
                })
        })
    }

    /// The `enum_mixin_with_instance_variable` face: an enum's `with`-clause
    /// entry whose mixin declares an instance variable
    /// (`inheritance_override.dart:869-893` `_checkMixinOfEnum`, reached from
    /// `_checkDirectSuperTypes`' with-clause loop at :684-695). The row
    /// anchors at the written `NamedType` node.
    fn flush_enum_mixin_instance_variable(&mut self, container: StringId, decl: &Container) {
        if decl.with_clause.is_empty() {
            return;
        }
        let Some(name_range) = self.container_name_range(container) else {
            return;
        };
        let Some(spans) = self.with_clause_entry_spans(decl, name_range) else {
            return;
        };
        let scope_lib = self
            .owner_library_of(self.file, container)
            .unwrap_or(self.lib);
        // The rows are collected before any emission: the span slice borrows
        // the engine immutably.
        let rows: Vec<SourceRange> = decl
            .with_clause
            .iter()
            .zip(spans)
            .filter_map(|(&mixin, &span)| {
                let (face, _) = self.resolve_clause_face(container, self.file, scope_lib, mixin)?;
                // `interfaceElement is EnumElement || ExtensionTypeElement`
                // stands the check down (:873-876).
                if matches!(
                    face.kind,
                    ContainerKind::Enum | ContainerKind::ExtensionType
                ) {
                    return None;
                }
                face.members
                    .iter()
                    .any(|(_, entry)| {
                        entry.kind == MemberKind::Field
                            && !entry.is_static
                            && !entry.is_abstract
                            && !entry.is_external
                    })
                    .then_some(span)
            })
            .collect();
        for span in rows {
            self.emit(SemanticCode::EnumMixinWithInstanceVariable, span, "", "");
        }
    }

    /// The written `with`-clause entry spans of the declaration that just
    /// closed: the tail `drain_with_clause_spans` moved into
    /// [`Engine::pending_mixin_uses`] at the clause's end, in application
    /// order. Each span is the oracle's whole-`NamedType` extent — the head
    /// name through the type-argument group's closer or a trailing `?`.
    ///
    /// A recovery shape (fewer completions than the element's claimed
    /// entries) would leave an earlier declaration's spans in the tail, so
    /// every entry must sit past this declaration's own name; a mismatch
    /// answers `None` and the caller under-reports rather than anchoring a
    /// row at a foreign offset.
    fn with_clause_entry_spans(
        &self,
        decl: &Container,
        name_range: SourceRange,
    ) -> Option<&[SourceRange]> {
        let start = self
            .pending_mixin_uses
            .len()
            .checked_sub(decl.with_clause.len())?;
        let spans = &self.pending_mixin_uses[start..];
        spans
            .iter()
            .all(|span| span.offset > name_range.offset)
            .then_some(spans)
    }

    /// Whether the class `container` (or any superclass/mixin it climbs
    /// through) has a non-final instance field with storage — the summary2
    /// `hasNonFinalField` computation (link.dart:295-322): a field with
    /// storage (the origin declaration is not abstract, or the field is a
    /// declaring formal parameter) must be `final`.
    fn face_has_non_final_field(
        &self,
        file: FileId,
        lib: LibraryId,
        name: StringId,
        seen: &mut FxHashSet<(FileId, StringId)>,
    ) -> bool {
        let Some((decl, owner)) =
            resolve_container(self.graph, self.namespaces, self.index, lib, file, name)
        else {
            return false;
        };
        if !seen.insert((owner, name)) {
            return false;
        }
        if decl.members.values().any(|entry| {
            !entry.is_static
                && matches!(entry.kind, crate::model::MemberKind::Field)
                && !entry.is_abstract
                && !entry.is_final
        }) {
            return true;
        }
        let scope_lib = owner_library(self.graph, self.lib, owner, name).unwrap_or(lib);
        decl.superclass.is_some_and(|superclass| {
            self.face_has_non_final_field(owner, scope_lib, superclass, seen)
        }) || decl
            .with_clause
            .iter()
            .any(|mixin| self.face_has_non_final_field(owner, scope_lib, *mixin, seen))
    }

    /// Whether the constructor whose head anchor is `head` invokes a
    /// non-const super constructor — the
    /// `_checkForConstConstructorWithNonConstSuper` pre-emption
    /// (error_verifier.dart:4034-4130). An unresolvable super face answers
    /// `false`: the oracle's null `superConstructor` never pre-empts.
    fn ctor_has_non_const_super(&self, decl: &Container, head: SourceRange) -> bool {
        let Some(super_name) = decl.superclass else {
            return false;
        };
        let Some((super_decl, super_file)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            super_name,
        ) else {
            return false;
        };
        // An enum's generative constructors are implicitly const.
        if super_decl.kind == ContainerKind::Enum {
            return false;
        }
        let invoked = self.invoked_super_selector(head).unwrap_or(super_name);
        let unnamed = invoked == super_name || invoked == self.intern("new");
        if super_file == self.file {
            if let Some(target) = self.ctor_decl_site(super_name, invoked) {
                // `super.x(...)` never invokes a factory (upstream's
                // `invokedSuper` is the generative face).
                return !target.is_factory && !target.const_ctor;
            }
            // No declaration site: the unnamed spelling resolves to the
            // implicit default (never const, and only present when the
            // class is constructible unnamed); a named spelling resolves to
            // no generative face upstream — no pre-emption.
            return unnamed && super_decl.has_unnamed_constructor;
        }
        // A foreign superclass answers no per-selector declaration site; the
        // container-level const bit is the coarse face (an unnamed
        // invocation against a class with no unnamed constructor is an
        // undefined face upstream — no pre-emption).
        if unnamed && !super_decl.has_unnamed_constructor {
            return false;
        }
        unnamed && !super_decl.has_const_constructor
    }

    /// The super constructor a constructor head's initializer list invokes:
    /// `None` for the implicit or explicit unnamed invocation, `Some(sel)`
    /// for `super.sel(...)`. The scan runs past the formal-parameters group
    /// up to the body brace or the terminating `;` (nested groups skip
    /// whole), so a `super.x = v` field initializer never reads as an
    /// invocation.
    fn invoked_super_selector(&self, head: SourceRange) -> Option<StringId> {
        let tokens = self.tokens.as_slice();
        let start = tokens.partition_point(|token| token.utf16_offset < head.offset);
        let open = (start..tokens.len())
            .find(|&index| tokens[index].kind == dartq_scanner::TokenKind::OpenParen)?;
        let mut index = self.tokens.matching_end(open)? + 1;
        while let Some(token) = tokens.get(index) {
            match token.kind {
                dartq_scanner::TokenKind::Semicolon | dartq_scanner::TokenKind::OpenCurly => break,
                dartq_scanner::TokenKind::OpenParen | dartq_scanner::TokenKind::OpenSquare => {
                    index = self.tokens.matching_end(index)? + 1;
                }
                dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Super) => {
                    match tokens.get(index + 1).map(|next| &next.kind) {
                        Some(dartq_scanner::TokenKind::OpenParen) => return None,
                        Some(dartq_scanner::TokenKind::Period) => {
                            let selector = tokens.get(index + 2)?;
                            if selector.kind == dartq_scanner::TokenKind::Identifier
                                && tokens.get(index + 3).map(|next| &next.kind)
                                    == Some(&dartq_scanner::TokenKind::OpenParen)
                            {
                                return Some(
                                    self.intern(self.tokens.lexeme(self.source, index + 2)),
                                );
                            }
                        }
                        _ => {}
                    }
                    index += 1;
                }
                _ => index += 1,
            }
        }
        None
    }

    /// The declaring name token's span of `container` — the anchor the
    /// container-level faces compose their `errorRange`s from.
    fn container_name_range(&self, container: StringId) -> Option<SourceRange> {
        self.decl_sites
            .iter()
            .rev()
            .find(|site| matches!(site.kind, DeclKind::Container) && site.name == container)
            .map(|site| site.range)
    }

    /// A primary constructor's `errorRange` (`PrimaryConstructorDeclaration`
    /// overrides it): `beginToken` — the `const` keyword when written, else
    /// the class name — through the constructor name. The `const` keyword
    /// sits immediately before the class name (`class const A.named(...)`);
    /// an unnamed head ends at the begin token itself.
    fn primary_ctor_error_range(
        &self,
        container: StringId,
        head: SourceRange,
    ) -> Option<SourceRange> {
        let name = self.container_name_range(container)?;
        let start = self
            .token_before(name.offset)
            .filter(|previous| self.lexeme_at(previous.offset) == Some("const"))
            .unwrap_or(name);
        let end = if self.lexeme_at(head.offset) == Some(".") {
            self.token_after(head.offset)
                .map_or(start.end(), |selector| selector.end())
        } else {
            start.end()
        };
        Some(SourceRange {
            offset: start.offset,
            length: end - start.offset,
        })
    }

    /// The token immediately BEFORE the one starting at `offset` (UTF-16).
    fn token_before(&self, offset: u32) -> Option<SourceRange> {
        let tokens = self.tokens.as_slice();
        let at = tokens.partition_point(|token| token.utf16_offset < offset);
        let token = tokens.get(at.checked_sub(1)?)?;
        Some(SourceRange {
            offset: token.utf16_offset,
            length: self.utf16_of((token.byte_offset + token.byte_len) as usize)
                - token.utf16_offset,
        })
    }
}

// The #2815 pins (the B2 unchecked-nullable family). Registered here,
// not in `diagnose.rs` (the #2807 lane's file this wave), via `#[path]`:
// the module stays inside the `diagnose` subtree so the test rig's
// `pub(super)` items remain visible. Moving the declaration into
// `diagnose.rs`'s sibling block is a mechanical follow-up for whoever
// owns that file next.
#[cfg(test)]
#[path = "ticket_2815_tests.rs"]
mod ticket_2815_tests;

#[cfg(test)]
mod extension_type_ctors {
    //! #2212 (S1): extension types are constructor-carrying containers —
    //! `X._(x)`, `X.named(x)`, and `X(x)` resolve through the same
    //! `constructors`/`has_unnamed_constructor` face a class's creations
    //! read (constructor_invocation_resolver.dart). The collector's
    //! `end_primary_constructor` capture feeds the face; these pins fix the
    //! oracle-verified verdicts at the semantic tier (`dart analyze`
    //! 3.35.0, probed): a private-selector primary ctor invocation, a
    //! factory-selector invocation, and an unnamed-primary-ctor invocation
    //! are silent; a named-only extension type's bare `X()` reports
    //! `new_with_undefined_constructor_default` with the class-family text.
    //!
    //! Production-side note: the element collector observes primary-
    //! constructor events through the engine's `MultiplexListener` fan-out
    //! (`dartq-engine l1.rs`), which does not forward
    //! `begin_primary_constructor`/`end_primary_constructor` — the corpus
    //! rows this family produces (`XFlags._(bits)` →
    //! `undefined_extension_method`, `BaseName(str)` →
    //! `new_with_undefined_constructor_default`) close only when that
    //! forwarding lands; the in-test `collect` path is single-listener, so
    //! the pins below exercise the verdict contract directly.

    use crate::collector::collect;
    use crate::diagnose::{FileReport, StrictModes, diagnose_with};
    use crate::library::ElementIndex;
    use crate::model::MicroKind;
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_parser::ParserFeatures;
    use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder, Namespaces};
    use dartq_scanner::ScanConfig;
    use dartq_source::{FileTable, StringTable};
    use std::path::Path;

    /// The one-library rig: `source` is the sole analyzed file of
    /// `package:t`, no imports (the same shape `tests.rs`'s `report`
    /// builds — `collect` feeds the fold directly).
    fn unit_report(source: &str) -> FileReport {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file = files.intern(Path::new("main.dart"));
        let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
        let features = ParserFeatures::from_feature_set(set);
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
        let elements = collect(source, &scanned.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib = builder.intern("package:t/main.dart");
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let mut index = ElementIndex::default();
        index.insert(file, elements);
        diagnose_with(
            source,
            &scanned.tokens,
            features,
            &strings,
            file,
            lib,
            &graph,
            &namespaces,
            &index,
            StrictModes::default(),
            &LibraryClassModifiers::default(),
            set.is_enabled(dartq_language::Feature::InferenceUpdate2),
        )
    }

    fn count_code(report: &FileReport, name: &str) -> usize {
        report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == name)
            .count()
    }

    fn code_names(report: &FileReport) -> Vec<&'static str> {
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name.as_ref())
            .collect()
    }

    /// The private-selector primary constructor (`X._(bits)` — the
    /// `XFlags._(bits)` corpus shape): the selector resolves through the
    /// container's `constructors` face, never the extension-member family —
    /// no `undefined_extension_method`.
    #[test]
    fn private_primary_ctor_invocation_resolves() {
        let report = unit_report(
            "class int {} extension type E._(int bits) {\n  factory E.read(int x) => E._(x);\n}\n\
             void f() { var a = E._(1); var b = E.read(2); }",
        );
        assert_eq!(
            count_code(&report, "undefined_extension_method"),
            0,
            "{:?}",
            code_names(&report)
        );
    }

    /// A public named primary constructor resolves identically
    /// (`E.named(x)` — the selector the `A.named` face carries).
    #[test]
    fn named_primary_ctor_invocation_resolves() {
        let report = unit_report(
            "class int {} extension type E.named(int bits) {}\n\
             void f() { var a = E.named(1); }",
        );
        assert_eq!(
            count_code(&report, "undefined_extension_method"),
            0,
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            count_code(&report, "undefined_method"),
            0,
            "{:?}",
            code_names(&report)
        );
    }

    /// The unnamed primary constructor (`X(x)`): the face answers through
    /// `has_unnamed_constructor` — no `new_with_undefined_constructor_default`.
    #[test]
    fn unnamed_primary_ctor_invocation_resolves() {
        let report = unit_report(
            "class int {} extension type F(int it) {}\n\
             void f() { var c = F(4); }",
        );
        assert_eq!(
            count_code(&report, "new_with_undefined_constructor_default"),
            0,
            "{:?}",
            code_names(&report)
        );
    }

    /// An unnamed primary constructor beside a factory keeps the unnamed
    /// face (`BaseName(str)` — the lookup_name.dart corpus shape):
    /// `has_unnamed_constructor` reads the primary-ctor flag, not the
    /// factory's declared count.
    #[test]
    fn unnamed_primary_ctor_beside_factory_resolves() {
        let report = unit_report(
            "class int {} class String {} extension type BaseName(String it) {\n\
             factory BaseName.read(int r) => BaseName(\"x\");\n}\n\
             void f() { var d = BaseName(\"s\"); }",
        );
        assert_eq!(
            count_code(&report, "new_with_undefined_constructor_default"),
            0,
            "{:?}",
            code_names(&report)
        );
    }

    /// A named-only extension type's bare `X()` is the same
    /// `new_with_undefined_constructor_default` a named-only class
    /// reports — the oracle's probe on `dart analyze` 3.35.0 confirms the
    /// class-family text ("The class 'E' doesn't have an unnamed
    /// constructor.") even on the extension-type head.
    #[test]
    fn named_only_extension_type_reports_default_ctor_code() {
        let report = unit_report(
            "class int {} extension type E.named(int i) {}\n\
             void f() { var e = E(0); }",
        );
        assert_eq!(
            count_code(&report, "new_with_undefined_constructor_default"),
            1,
            "{:?}",
            code_names(&report)
        );
        let report = unit_report(
            "class int {} extension type E._(int i) {}\n\
             void f() { var e = E(0); }",
        );
        assert_eq!(
            count_code(&report, "new_with_undefined_constructor_default"),
            1,
            "{:?}",
            code_names(&report)
        );
    }
}

#[cfg(test)]
mod boundary_projection {
    //! The boundary projector's stale-owner degrade: a foreign `Ty` cached
    //! in a member row can outlive its owner unit's residency, so every
    //! pool read behind [`Boundary::foreign_pool`] answers `None` and the
    //! projection lands on the unknown face — a foreign slot id must never
    //! reach the diagnosing pool's reads (the former residency `.expect`).

    use super::{Boundary, PairingLedger};
    use crate::library::ElementIndex;
    use crate::model::{ArgListId, FnShapeId, RecordShapeId, Ty};
    use dartq_resolution::{LibraryGraphBuilder, Namespaces};
    use dartq_source::{FileTable, StringTable};
    use std::path::Path;

    /// A stale owner's named, record, and function faces all project to the
    /// unknown face — the skip the reachability stale-edge drop applies,
    /// never a foreign slot read through the diagnosing pool.
    #[test]
    fn stale_foreign_owner_projects_to_the_unknown_face() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let own = files.intern(Path::new("main.dart"));
        // The stale unit: interned, never inserted into the index — the
        // left-residency face.
        let foreign = files.intern(Path::new("gone.dart"));
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib = builder.intern("package:t/main.dart");
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let index = ElementIndex::default();
        let boundary = Boundary::new(&index, own, lib, None, &graph, &namespaces, &strings);

        // Foreign-shaped slots, minted fresh so the ids are non-reserved:
        // the projection must degrade on the owner lookup, before any pool
        // read — a wrong-slot read would panic here, not degrade.
        let args = boundary.intern_list(&[Ty::named(strings.intern("int"))]);
        let record = boundary
            .args
            .borrow_mut()
            .intern_record(&[Ty::named(strings.intern("int"))], &[]);
        let function = boundary.args.borrow_mut().intern_function(
            0,
            0,
            &[],
            Ty::named(strings.intern("int")),
            &[],
            &[],
        );
        assert_ne!(args, ArgListId::EMPTY);
        assert_ne!(record, RecordShapeId::EMPTY);
        assert_ne!(function, FnShapeId::EMPTY);

        assert!(
            boundary.foreign_pool(foreign).is_none(),
            "the stale owner answers no pool"
        );
        let named = boundary.ty(
            foreign,
            Ty::Named {
                name: strings.intern("List"),
                nullable: false,
                args,
            },
        );
        assert_eq!(named, Ty::Dynamic, "a stale named face reads unknown");
        assert_eq!(
            boundary.ty(
                foreign,
                Ty::Record {
                    shape: record,
                    nullable: false
                }
            ),
            Ty::Dynamic,
            "a stale record face reads unknown"
        );
        assert_eq!(
            boundary.ty(
                foreign,
                Ty::Function {
                    shape: function,
                    nullable: false
                }
            ),
            Ty::Dynamic,
            "a stale function face reads unknown"
        );
    }

    /// The pairing ledger (the lints bridge's `expect_paired` posture):
    /// the first miss's invariant text is retained, later misses only bump
    /// the count, and the whole ledger stays two words.
    #[test]
    fn pairing_ledger_retains_the_first_miss_and_counts_the_rest() {
        let mut ledger = PairingLedger::default();
        assert_eq!(ledger.count, 0, "a healthy stream records nothing");
        ledger.miss("first miss");
        ledger.miss("second miss");
        ledger.miss("third miss");
        assert_eq!(ledger.first, Some("first miss"));
        assert_eq!(ledger.count, 3);
    }
}

// The #3150 pins (the `undefined_identifier` InvalidType operand face
// and the bare/prefixed code-split table). Registered here, not in
// `diagnose.rs` (other lanes' file), via `#[path]`: the module stays
// inside the `diagnose` subtree so the test rig's `pub(super)` items
// remain visible.
#[cfg(test)]
#[path = "ticket_3150_tests.rs"]
mod ticket_3150_tests;

// The #3177 pins (`Ty::Function.type_params_named` across the pool
// projection/dotted-canonicalization seams — the generic tear-off's
// verbatim `<T extends B>` group, plus the `ty.render` function/record
// faces). Same `#[path]` registration as #3150: the module stays inside
// the `diagnose` subtree so `super::test_rig` resolves.
#[cfg(test)]
#[path = "ticket_3177_tests.rs"]
mod ticket_3177_tests;

// The #2496 pins (the `FutureOr`-aware receiver nullability + the
// `FutureOr` member face — a `FutureOr<int>` receiver stays non-nullable,
// `FutureOr<int?>`/dynamic/void/unresolved read nullable through the
// dead-chain root gates, and `x.then(...)` over `FutureOr<T>` resolves
// `defined` through the `Future` member set). Same `#[path]` registration
// as #3150/#3177: the module stays inside the `diagnose` subtree so
// `super::test_rig` resolves.
#[cfg(test)]
#[path = "ticket_2496_tests.rs"]
mod ticket_2496_tests;

// The #2353 pins (the `Ty::Unknown` three-state faces — the closure-
// inferred `UnknownInferredType` receiver face reports the shared
// `unchecked_use_of_nullable_value` code, already converged via the
// `closure_ctx` `Object?` recovery; `dynamic`/`InvalidType`/unresolved-
// name/`?.` receivers stay silent). Same `#[path]` registration as
// #3150/#3177/#2496: the module stays inside the `diagnose` subtree so
// `super::test_rig` resolves.
#[cfg(test)]
#[path = "ticket_2353_tests.rs"]
mod ticket_2353_tests;
