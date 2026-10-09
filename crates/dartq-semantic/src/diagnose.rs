//! The M1 semantic diagnosis engine (`undefined_*` + `duplicate_definition`):
//! a pass over the parser's event stream that resolves every reference a unit
//! makes against the #117 lookup surface and the #119 element model, and
//! emits the frozen family as [`Diagnostic`]s.
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
//! verdict at push time (the frames tear down before pass 2) and resolve in
//! source order. Duplicate declarations are not this pass's output —
//! the collector logs them at capture time and [`diagnose`] folds that log
//! into the report.
//!
//! # The batch-1 gate (#131 first-batch ruling)
//!
//! This engine emits exactly four codes: `undefined_identifier`,
//! `undefined_class`, `undefined_function`, and `duplicate_definition` (plus
//! `undefined_function` on the prefixed-invoke path, which is that family's
//! qualified trigger). Every other frozen trigger — the member family
//! (`undefined_getter`/`undefined_method`/`undefined_setter`/`undefined_operator`
//! and their extension/`super`/enum-constant variants),
//! `undefined_prefixed_name`, `undefined_identifier_await`,
//! `undefined_named_parameter` — is recorded into the §8 names table without
//! emitting: under-reporting is the M1 contract, and a wrong code is worse
//! than no code. Batch 2 lights these up at the same call sites.
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
//!   `undefined_identifier_await` instead (`dart-sdk main
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
//!   their constructor-family codes stay deferred.
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
//! the directive milestone). Every deferral under-reports.

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

use crate::codes::SemanticCode;
use crate::library::{ElementIndex, ScopedHit, lookup_scoped, resolve_container};
use crate::model::{ContainerKind, MemberKind, MicroKind};

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
    /// Defined, but not a receiver face.
    Plain,
    /// Not defined (the row's intent decides the emitted code).
    Missing,
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
    /// Operator-shaped access (`a + b`, `a[i]`, `-a`): the operator spelling.
    Operator(StringId),
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
    /// A declaration-typed name (`C c; c.m()`).
    Typed { type_name: StringId },
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
    /// The write form an assignment applied (operator rows only).
    write_op: Option<OpWrite>,
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
    /// Declared-type bindings: name → declared type's container name.
    var_types: FxHashMap<StringId, StringId>,
    /// `type_params` length to truncate back to when this frame closes.
    type_mark: usize,
}

enum FrameKind {
    /// A class/mixin/enum/extension body: fields and the member face.
    Container { name: StringId, kind: ContainerKind },
    /// An executable (method, constructor, function, function literal).
    Executable { instance: bool },
    /// A statement block, catch clause, or typedef body.
    Block,
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
    /// The last bare type annotation's container name, for declaration
    /// binding.
    last_type: Option<StringId>,
    /// Per open argument list: the callee row index, when identifiable
    /// (`None` while inside a `new`/`const` reference — its class name is a
    /// type use, never an invocation callee).
    invokes: Vec<Option<usize>>,
    /// The argument group that just closed, awaiting the send event that
    /// consumes it (an argless send never opens a group, so a bare read
    /// stays a read).
    pending_send: Option<usize>,
    type_prefix: Option<TypePrefix>,
    /// Suppression depth inside `is`/`as` type operands.
    non_type_pos: u32,
    /// Suppression depth inside `new`/`const` expressions.
    ctor_pos: u32,
}

impl<'a> Engine<'a> {
    // —— plumbing ————————————————————————————————————————————————————

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
            last_type: None,
            invokes: Vec::new(),
            pending_send: None,
            type_prefix: None,
            non_type_pos: 0,
            ctor_pos: 0,
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
                FrameKind::Executable { instance } => Some(*instance),
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

    /// Binds the pending declared type to `name` in the innermost frame.
    fn bind_declared_type(&mut self, name: StringId) {
        let Some(type_name) = self.last_type.take() else {
            return;
        };
        if let Some(frame) = self.frames.last_mut() {
            frame.var_types.insert(name, type_name);
        }
    }

    /// Drops a stale pending declaration type (`var`/`final`/`const` carry
    /// no type name).
    fn clear_last_type(&mut self) {
        self.last_type = None;
    }

    /// A container head (`class C`, unnamed extension placeholder): the
    /// member face and the `this` receiver for its body.
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
        self.push_frame(FrameKind::Executable { instance });
    }

    /// A statement block, catch clause, for body, or typedef scope.
    fn push_block_frame(&mut self) {
        self.push_frame(FrameKind::Block);
    }

    /// Enters a type parameter's scope (its frame's `type_mark` truncates it
    /// away on pop).
    fn push_type_parameter(&mut self, name: StringId) {
        self.type_params.push(name);
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

    /// A bare name's declared type's container, innermost frame first.
    fn typed_local(&self, name: StringId) -> Option<StringId> {
        self.frames
            .iter()
            .rev()
            .find_map(|f| f.var_types.get(&name).copied())
    }

    // —— resolution ——————————————————————————————————————————————————

    /// Resolves a bare name in value position: locals, type parameters, the
    /// member face (own-file container chain), the extension pool, enum
    /// synthesized members, then the scoped library lookup.
    fn resolve_bare(&self, name: StringId) -> BareInfo {
        if self.frames.iter().rev().any(|f| f.locals.contains(&name)) {
            return BareInfo::Plain;
        }
        if self.type_params.contains(&name) {
            return BareInfo::Plain;
        }
        if let Some((container, kind)) = self.container_context() {
            if !matches!(self.walk_members(container, name), Walk::Missing) {
                // Found, or a broken chain — both suppress.
                return BareInfo::Plain;
            }
            if kind == ContainerKind::Enum && self.is_reserved(name, &ENUM_SYNTH_MEMBERS) {
                return BareInfo::Plain;
            }
            if self.instance_context() && self.is_reserved(name, &OBJECT_MEMBERS) {
                return BareInfo::Plain;
            }
        }
        if self
            .index
            .get(self.file)
            .is_some_and(|f| f.extension_member(name).is_some())
        {
            return BareInfo::Plain;
        }
        match lookup_scoped(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        ) {
            ScopedHit::Local(decl) => bare_info_of(name, decl.kind),
            ScopedHit::Found(entry) => self.bare_info_of_entry(name, entry),
            ScopedHit::Ambiguous(_) => BareInfo::Plain,
            ScopedHit::NotFound => BareInfo::Missing,
        }
    }

    fn bare_info_of_entry(&self, name: StringId, entry: NamespaceEntry) -> BareInfo {
        match entry {
            NamespaceEntry::Prefix => BareInfo::Prefix { name },
            NamespaceEntry::Decl(file) => {
                let kind = self
                    .index
                    .get(file)
                    .and_then(|f| f.decl(name))
                    .map_or(MicroKind::TopLevelVar, |d| d.kind);
                bare_info_of(name, kind)
            }
        }
    }

    fn is_reserved(&self, name: StringId, set: &[&str]) -> bool {
        set.iter().any(|&m| self.intern(m) == name)
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

    /// The named formals of a resolved top-level callee (`None` = no check),
    /// for the named-argument membership test.
    fn top_level_formals(&self, name: StringId) -> Option<&[StringId]> {
        let decl = match lookup_scoped(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        ) {
            ScopedHit::Local(decl) => decl,
            ScopedHit::Found(NamespaceEntry::Decl(file)) => {
                self.index.get(file).and_then(|f| f.decl(name))?
            }
            _ => return None,
        };
        match decl.kind {
            MicroKind::TopLevelFn => Some(decl.named_formals.as_deref().unwrap_or(&[])),
            _ => None,
        }
    }

    /// The named formals of a resolved member callee, walked from `recv`.
    fn member_formals(&self, recv: &Receiver, name: StringId) -> Option<&[StringId]> {
        let (container, super_walk) = match *recv {
            Receiver::This { container, .. } => (container, false),
            Receiver::Super { container, .. } => (container, true),
            Receiver::Typed { type_name } => (type_name, false),
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
                    Some(entry) if entry.kind == MemberKind::Method => {
                        Some(entry.named_formals.as_deref().unwrap_or(&[]))
                    }
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
            Walk::Found(entry) if entry.kind == MemberKind::Method => {
                Some(entry.named_formals.as_deref().unwrap_or(&[]))
            }
            _ => None,
        }
    }

    // —— row stack ———————————————————————————————————————————————————

    fn push_row(&mut self, row: Row) {
        self.rows.push(row);
    }

    /// A bare identifier in value position: resolved immediately (the
    /// verdict is the snapshot pass 2 emits from).
    fn push_bare(&mut self, name: StringId, range: SourceRange) {
        let info = self.resolve_bare(name);
        self.push_row(Row {
            name,
            range,
            qualified: false,
            shape: RowShape::Bare(info),
            intent: Intent::Read,
            write_op: None,
        });
    }

    /// An identifier after a dot, awaiting the merge at the dot event.
    fn push_member(&mut self, name: StringId, range: SourceRange) {
        self.push_row(Row {
            name,
            range,
            qualified: true,
            shape: RowShape::Bare(BareInfo::Plain),
            intent: Intent::Read,
            write_op: None,
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
        });
    }

    /// A type-annotation position. Defined types push no row; the §8 names
    /// table carries misses only.
    fn push_type_use(&mut self, name: StringId, range: SourceRange, head: TypeHead) {
        let suppressed = self.non_type_pos > 0;
        match head {
            TypeHead::Bare => {
                self.last_type = Some(name);
                if self.is_reserved(name, &BUILTIN_TYPES) {
                    return;
                }
                if !matches!(self.resolve_bare(name), BareInfo::Missing) {
                    return;
                }
                self.push_row(Row {
                    name,
                    range,
                    qualified: false,
                    shape: RowShape::TypeUse(TypeVerdict::Missing, suppressed),
                    intent: Intent::Read,
                    write_op: None,
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
                self.last_type = Some(name);
                if !hit {
                    self.push_row(Row {
                        name,
                        range,
                        qualified: true,
                        shape: RowShape::TypeUse(TypeVerdict::Prefixed { prefix }, false),
                        intent: Intent::Read,
                        write_op: None,
                    });
                }
            }
        }
    }

    /// Arms the type-prefix slot for a `PrefixedTypeReference` head; a miss
    /// also pushes the head's own report row.
    fn arm_type_prefix(&mut self, name: StringId, range: SourceRange) {
        let live = !matches!(self.resolve_bare(name), BareInfo::Missing);
        self.type_prefix = Some(TypePrefix { prefix: name, live });
        if !live {
            self.push_row(Row {
                name,
                range,
                qualified: false,
                shape: RowShape::TypeUse(TypeVerdict::PrefixHead, false),
                intent: Intent::Read,
                write_op: None,
            });
        }
    }

    /// Merges `receiver.member` at the dot/cascade event: pops the member row,
    /// classifies the receiver below it, and pushes the merged row.
    fn merge_member(&mut self) {
        let Some(member) = self.rows.pop() else {
            return;
        };
        let recv = match self.rows.pop() {
            Some(popped) => match popped.shape {
                RowShape::Bare(info) => self.receiver_of_bare(info, popped.name),
                RowShape::Receiver(recv) => recv,
                _ => Receiver::Unknown,
            },
            None => Receiver::Unknown,
        };
        self.push_row(Row {
            name: member.name,
            range: member.range,
            qualified: true,
            shape: RowShape::Member { recv },
            intent: member.intent,
            write_op: member.write_op,
        });
    }

    /// Opens an argument group: the top row is its callee — unless the group
    /// belongs to a `new`/`const` reference, whose class name resolves as a
    /// type use, never an invocation.
    fn open_arguments(&mut self) {
        let callee = if self.ctor_pos > 0 {
            None
        } else {
            self.callee_index()
        };
        self.invokes.push(callee);
    }

    /// Closes an argument group; its callee awaits the send event.
    fn close_arguments(&mut self) {
        self.pending_send = self.invokes.pop().flatten();
    }

    /// Consumes a completed send: the closed group's callee row becomes an
    /// invocation. A send without arguments never opened a group, so a bare
    /// read stays a read (`foo;` vs `foo()`).
    fn consume_send(&mut self) {
        let Some(callee) = self.pending_send.take() else {
            return;
        };
        if let Some(row) = self.rows.get_mut(callee)
            && row.intent == Intent::Read
        {
            row.intent = Intent::Invoke;
        }
    }

    fn receiver_of_bare(&self, info: BareInfo, name: StringId) -> Receiver {
        match info {
            BareInfo::Container { name, kind } => Receiver::Container { name, kind },
            BareInfo::Prefix { name } => Receiver::Prefix { name },
            BareInfo::Plain => self
                .typed_local(name)
                .map_or(Receiver::Unknown, |type_name| Receiver::Typed { type_name }),
            // The receiver itself is undefined; its own row reports, so the
            // member access must not.
            BareInfo::Missing => Receiver::Unknown,
        }
    }

    /// Marks the assignment target: the closest row wholly before the
    /// operator. Bare/member rows upgrade their intent; index-operator rows
    /// record the write form.
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
            match &row.intent {
                Intent::Read => {
                    row.intent = if compound {
                        Intent::ReadWrite
                    } else {
                        Intent::Write
                    };
                }
                Intent::Operator(op) => {
                    if *op == index_op {
                        row.write_op = Some(if compound {
                            OpWrite::Compound
                        } else {
                            OpWrite::Assign
                        });
                    }
                }
                _ => {}
            }
            return;
        }
    }

    /// Marks an operator operand: the closest row wholly before `op`, when it
    /// is a bare identifier or a typed receiver (`a + b`, `this + b`); member
    /// rows are skipped (the operator applies to the getter result).
    fn mark_operator(&mut self, op: StringId, limit: u32) {
        for row in self.rows.iter_mut().rev() {
            if row.range.end() > limit {
                continue;
            }
            if matches!(row.shape, RowShape::Bare(_) | RowShape::Receiver(_))
                && row.intent == Intent::Read
            {
                row.intent = Intent::Operator(op);
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

    /// The named argument membership test against the active callee.
    fn check_named_argument(&mut self, name: StringId, range: SourceRange) {
        let Some(formals) = self
            .invokes
            .last()
            .and_then(|slot| *slot)
            .and_then(|index| self.rows.get(index))
            .and_then(|callee| match &callee.shape {
                RowShape::Bare(_) => self.top_level_formals(callee.name),
                RowShape::Member { recv } => self.member_formals(recv, callee.name),
                _ => None,
            })
        else {
            return;
        };
        if formals.contains(&name) {
            return;
        }
        // `undefined_named_parameter` is a batch-2 emission; batch 1 records
        // the miss (the membership test itself is live — the §8 table only
        // carries real misses).
        self.record_miss(name, range, false, NameUse::NamedArgument);
    }

    // —— pass 2 ———————————————————————————————————————————————————————

    /// Resolves the recorded rows in source order and emits.
    fn resolve_rows(&mut self) {
        let rows = std::mem::take(&mut self.rows);
        for row in &rows {
            match &row.shape {
                RowShape::Bare(info) => self.resolve_bare_row(row, *info),
                RowShape::Member { recv } => self.resolve_member_row(row, *recv),
                RowShape::TypeUse(..) => self.resolve_type_row(row),
                RowShape::Receiver(_) | RowShape::Value => {}
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

    fn resolve_bare_row(&mut self, row: &Row, info: BareInfo) {
        let name_text = self.strings.resolve(row.name);
        match info {
            BareInfo::Container { .. } | BareInfo::Plain => {}
            // A prefix used bare: invoking a prefix is an error the oracle
            // names with an unfrozen code — record only.
            BareInfo::Prefix { .. } => {}
            BareInfo::Missing => match row.intent {
                Intent::Invoke => {
                    if !self.guard.ignore(None, row.name) {
                        self.emit(SemanticCode::UndefinedFunction, row.range, name_text, "");
                        self.record_miss(row.name, row.range, false, NameUse::Invoke);
                    }
                }
                Intent::Read if row.name == self.intern("await") && self.in_executable() => {
                    // `undefined_identifier_await` is a batch-2 emission;
                    // batch 1 records (emitting plain `undefined_identifier`
                    // here would contaminate the oracle diff).
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
                Intent::Operator(_) => {}
            },
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
                    // `undefined_function` (batch 1); the property path's
                    // `undefined_prefixed_name` is batch 2 — record only.
                    Intent::Invoke => {
                        self.emit(SemanticCode::UndefinedFunction, row.range, name_text, "");
                        self.record_miss(row.name, row.range, true, NameUse::Invoke);
                    }
                    Intent::Read | Intent::Write | Intent::ReadWrite => {
                        let use_kind = match row.intent {
                            Intent::Write | Intent::ReadWrite => NameUse::Write,
                            _ => NameUse::Read,
                        };
                        self.record_miss(row.name, row.range, true, use_kind);
                    }
                    Intent::Operator(_) => {}
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
    /// `Ext.g`). External containers suppress (kind unknown). The member
    /// family's codes (`undefined_getter`/`undefined_method`/
    /// `undefined_enum_constant`/…) are batch-2 emissions: batch 1 records
    /// the miss.
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
        if resolved.members.contains_key(&row.name) {
            return;
        }
        if kind != ContainerKind::Enum || matches!(row.intent, Intent::Read | Intent::Invoke) {
            self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
        }
    }

    /// Instance access through a declaration-typed receiver (`this.m`, `c.m`
    /// on a `C c`): a clean walk that misses records; a broken chain, the
    /// extension pool, enum synthesized members, and `Object` members
    /// suppress. The member family's codes are batch-2 emissions.
    fn check_instance_member(&mut self, row: &Row, recv: Receiver) {
        let container = match recv {
            Receiver::This { container, .. }
            | Receiver::Typed {
                type_name: container,
            } => container,
            _ => return,
        };
        let (op_read, op_write) = self.operator_spellings(row);
        if self.member_defined(container, row.name, false, op_read, op_write) {
            return;
        }
        let Some(kind) = self.container_kind_of(container) else {
            return; // external receiver type: suppress
        };
        if kind == ContainerKind::Enum && self.is_reserved(row.name, &ENUM_SYNTH_MEMBERS) {
            return;
        }
        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
    }

    /// `super.member` misses: `undefined_super_member` (one code name, four
    /// per-shape messages) is a batch-2 emission; batch 1 records.
    fn check_super_member(&mut self, row: &Row, container: StringId) {
        let (op_read, op_write) = self.operator_spellings(row);
        if self.member_defined(container, row.name, true, op_read, op_write) {
            return;
        }
        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
    }

    /// The read and write operator spellings of an operator row (`[]` / `[]=`);
    /// `(None, None)` for non-operator rows.
    fn operator_spellings(&self, row: &Row) -> (Option<StringId>, Option<StringId>) {
        let Intent::Operator(op) = &row.intent else {
            return (None, None);
        };
        if *op == self.intern("[]") {
            match row.write_op {
                Some(OpWrite::Assign) => (None, Some(*op)),
                Some(OpWrite::Compound) => (Some(*op), Some(*op)),
                None => (Some(*op), None),
            }
        } else {
            (Some(*op), None)
        }
    }

    /// Runs the walk(s) an access needs and applies the shared suppressions
    /// (broken chain, extension pool, `Object` members). Operator rows walk
    /// their read/write operator spellings.
    #[allow(clippy::too_many_arguments)]
    fn member_defined(
        &self,
        container: StringId,
        name: StringId,
        super_walk: bool,
        op_read: Option<StringId>,
        op_write: Option<StringId>,
    ) -> bool {
        let walk = |member: StringId| {
            let walk = if super_walk {
                self.walk_super(container, member)
            } else {
                self.walk_members(container, member)
            };
            matches!(walk, Walk::Found(_))
        };
        let found = match (op_read, op_write) {
            (Some(read), Some(_)) => walk(read) || walk(self.intern("[]=")),
            (Some(single), None) | (None, Some(single)) => walk(single),
            (None, None) => walk(name),
        };
        if found {
            return true;
        }
        if name == self.intern("==") || self.is_reserved(name, &OBJECT_MEMBERS) {
            return true;
        }
        self.index
            .get(self.file)
            .is_some_and(|f| f.extension_member(name).is_some())
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
    // `duplicate_definition`s lead the report; each points at the SECOND
    // declaration's name token (the analyzer's diagnose-at-second rule).
    // Setters live under `name=` in the model; the oracle's message names
    // the bare identifier.
    let mut diagnostics = Vec::with_capacity(index.get(file).map_or(0, |f| f.duplicates().len()));
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
        var_final_or_const: Option<Token>,
    ) {
        let mut engine = self.engine.borrow_mut();
        // `var`/`final`/`const` carry no type name: drop any stale
        // declaration type so the bound names stay untyped (unknown
        // receivers suppress, never fabricate).
        if var_final_or_const.is_some() {
            engine.clear_last_type();
        }
    }

    fn begin_type_variable(&mut self, token: Token) {
        let mut engine = self.engine.borrow_mut();
        if let Some(name) = engine.name_of(token) {
            engine.push_type_parameter(name);
        }
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
            C::TypeReference => {
                if let Some(name) = name {
                    engine.push_type_use(name, token.range, TypeHead::Bare);
                }
            }
            C::PrefixedTypeReference => {
                if let Some(name) = name {
                    engine.arm_type_prefix(name, token.range);
                }
            }
            C::TypeReferenceContinuation => {
                if let Some(name) = name {
                    engine.push_type_use(name, token.range, TypeHead::Continuation);
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
            C::LocalVariableDeclaration | C::FormalParameterDeclaration | C::CatchParameter => {
                if let Some(name) = name {
                    engine.add_local(name);
                    engine.bind_declared_type(name);
                }
            }
            C::NamedArgumentReference => {
                if let Some(name) = name {
                    engine.check_named_argument(name, token.range);
                }
            }
            _ => {}
        }
    }

    fn handle_qualified(&mut self, _period: Token) {
        self.engine.borrow_mut().consume_type_prefix();
    }

    // —— receivers and selectors ——————————————————————————————————————

    fn handle_this_expression(&mut self, token: Token, _context: IdentifierContext) {
        self.engine.borrow_mut().push_keyword_receiver(token, false);
    }

    fn handle_super_expression(&mut self, token: Token, _context: IdentifierContext) {
        self.engine.borrow_mut().push_keyword_receiver(token, true);
    }

    fn handle_dot_access(&mut self, _token: Token, _end: Token, _is_null_aware: bool) {
        self.engine.borrow_mut().merge_member();
    }

    fn handle_cascade_access(&mut self, _operator: Token, _end: Token, _is_null_aware: bool) {
        self.engine.borrow_mut().merge_member();
    }

    fn handle_send(&mut self, _begin: Token, _end: Token) {
        self.engine.borrow_mut().consume_send();
    }

    fn handle_assignment_expression(&mut self, operator: Token, _end: Token) {
        self.engine.borrow_mut().mark_assignment(operator);
    }

    fn begin_binary_expression(&mut self, operator: Token) {
        let mut engine = self.engine.borrow_mut();
        let Some(text) = engine.text_of(operator) else {
            return;
        };
        if USER_OPERATORS.contains(&text) {
            let op = engine.intern(text);
            engine.mark_operator(op, operator.range.offset);
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

    fn begin_arguments(&mut self, _token: Token) {
        self.engine.borrow_mut().open_arguments();
    }

    fn end_arguments(&mut self, _count: usize, _begin: Token, _end: Token) {
        self.engine.borrow_mut().close_arguments();
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
        self.engine.borrow_mut().push_value_row(token);
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

fn bare_info_of(name: StringId, kind: MicroKind) -> BareInfo {
    match kind {
        // Type parameters are declaration-scoped: the per-unit tag is stale
        // outside its declaration, and in-scope uses were answered by the
        // engine's type-parameter stack.
        MicroKind::TypeParam => BareInfo::Missing,
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
        _ => BareInfo::Plain,
    }
}

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
        assert_eq!(d.range.offset, "void f() { ".len() as u32);
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
            "var x = 0; void f() { nose; } var ".len() as u32
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
    /// a real miss lands in the §8 table without a batch-1 diagnostic.
    ///
    /// Hand-trace: `g`'s group is open while the labels fire, so
    /// `top_level_formals` sees `[n]`; `n` is contained (silent), `bad` is
    /// recorded (`undefined_named_parameter` is batch 2).
    #[test]
    fn named_argument_miss_is_recorded_not_emitted() {
        let report = report("void g({var n}) {} void f() { g(n: 1, bad: 2); }");
        assert!(report.diagnostics.is_empty(), "{:?}", code_names(&report));
        assert_eq!(report.names.len(), 1);
        assert_eq!(report.names[0].use_kind, NameUse::NamedArgument);
    }

    /// `await` in a sync body is the `undefined_identifier_await` trigger —
    /// batch 2's emission; batch 1 records without emitting.
    ///
    /// Hand-trace: the parser reads `await` as a plain identifier in a sync
    /// body; `in_executable` holds (the Executable frame is open), so the
    /// row records and only `x` emits.
    #[test]
    fn await_in_sync_body_is_recorded_not_emitted() {
        let report = report("void f() { await x; }");
        assert_eq!(code_names(&report), ["undefined_identifier"]);
        assert_eq!(report.names.len(), 2);
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
}
