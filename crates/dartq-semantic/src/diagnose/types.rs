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
    /// dispatch does not.
    Created { name: StringId, kind: ContainerKind },
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
            return ty;
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
                let (type_params, return_ty, positional, named) = {
                    let shape = foreign.function_shape(shape);
                    (
                        shape.type_params,
                        shape.return_ty,
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
                return self.args.borrow_mut().function(
                    type_params,
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
        let same = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.own_lib,
            self.own,
            leaf_id,
        )
        .is_some_and(|(_, own_file)| own_file == foreign_file);
        if same { leaf_id } else { name }
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
    /// equality on the substituted face stays structural.
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

/// One annotation in flight: the written name plus, when captured, the
/// `UseResult.unless` `parameterDefined` face (the official gate's value
/// face and the written expression's span).
pub(crate) struct PendingAnnotation {
    pub(crate) name: StringId,
    pub(crate) unless: Option<(UnlessArg, SourceRange)>,
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
pub(crate) struct SwitchArmScope {
    pub(crate) depth: usize,
    pub(crate) head_done: bool,
    pub(crate) shadows: Vec<(StringId, Option<Ty>, bool)>,
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
    /// The declaration is an `augment` (constructor sites only — the
    /// `duplicate_constructor` pass and the generative-constructor gate
    /// skip them; the fragment chain, not a second declaration, carries
    /// the augmentation).
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
    /// The open variable-declaration group's declared type (one
    /// `parse_type` per group), for the initializer checks.
    pub(crate) pending_decl_ty: Option<Ty>,
    /// [`Self::armed_ty_unresolved`] moved with the group's declared type.
    pub(crate) pending_decl_ty_unresolved: bool,
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
    /// parallel to [`Self::record_marks`]. The trailing `names.len()`
    /// fields of the split window are the named pairs.
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
    /// #1314 FAM10: the diagnosing file's own executables' type parameters
    /// with their written bounds, keyed by declaring name (`void f<T
    /// extends num>(T t)` → `[(T, num)]`). The argument-check
    /// substitution reads it for same-file generic callees.
    pub(crate) exec_bounds: FxHashMap<StringId, Box<[(StringId, Option<Ty>)]>>,
    /// The enclosing-method shape of the same capture (`void m<T extends
    /// C?>(T t)` → `[(T, C?)]`, keyed by declaring name): the
    /// nullability/never-match gates read the enclosing executable's own
    /// parameter bounds through it. Held apart from [`Engine::exec_bounds`]
    /// so the callee-side substitution faces — whose member-send
    /// inference sources its own parameters from the member walk — never
    /// see a method entry race them.
    pub(crate) method_exec_bounds: FxHashMap<StringId, Box<[(StringId, Option<Ty>)]>>,
    /// Whether the latched [`Engine::pending_exec_name`] header is a
    /// method (`MethodDeclaration` — routed to
    /// [`Engine::method_exec_bounds`]) or a top-level function (routed to
    /// [`Engine::exec_bounds`]).
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
    /// Explicit type arguments of pending calls, keyed by the callee's row
    /// index (pushed at the group's close, consumed by the argument check
    /// or the tear-off/implicit-creation faces).
    pub(crate) call_type_args: Vec<(u32, Box<[Ty]>)>,
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
    /// The const group's declared names, in initializer order.
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
    pub(crate) init_declarators: Vec<Option<(StringId, SourceRange)>>,
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
    /// Implicit-call initializers (`var f = promotedLocal(args)`): the
    /// callee's promoted read re-decides the send's result in pass 2 —
    /// the entry keys the declaring local's name-token offset, the
    /// callee's own row span, and the parse-time face.
    pub(crate) deferred_call_rebinds: Vec<(u32, SourceRange, Ty)>,
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
    /// The parameters of the executable declaration in flight attach to
    /// this declaration site (set at the site-creating begin events,
    /// read by the formal-parameter identifiers).
    pub(crate) open_exec_site: Option<usize>,
    /// Declaration sites of this file, parse order (= source order): the
    /// library-level side of the usage cluster.
    pub(crate) decl_sites: Vec<DeclSite>,
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
    /// Catch parameters seen in the clause in flight (0 = none, 1 =
    /// exception, 2 = exception + stack trace).
    pub(crate) catch_params: u8,
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
    /// `(operator offset, chain end, receiver read span)` latched at the
    /// operator, resolved at teardown when the flow facts are in.
    pub(crate) dead_null_chains: Vec<(u32, u32, SourceRange)>,
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
    /// rows' anchor.
    pub(crate) body_completions: Vec<(SourceRange, Ty, Option<SourceRange>, bool, SourceRange)>,
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
    /// The completed member types.
    pub(crate) members: Vec<Ty>,
    /// The leaf head awaiting its re-fired completion (same name at the
    /// same offset replaces instead of pushing a second member).
    pub(crate) pending: Option<(StringId, u32)>,
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
        // The drop rule: the official promotion only subtype-narrows
        // (`type_system.dart:1729` `tryPromoteToType`), so a `void`
        // promotion cannot arise, and a `Never` promotion marks a path
        // this verifier does not reach — the M1 lattice has no such source
        // type, and staging one would decide from a fiction.
        FlowBase::Void | FlowBase::Never => return None,
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
        Some(container_modifiers(container))
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
}

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
