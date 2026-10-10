//! The collecting listener: drives the parser's [`ParserListener`] event
//! stream into a [`FileElements`] model (the `src/tests/collector.rs`
//! collecting-listener precedent, applied to the declaration surface).
//!
//! # Capture mechanics
//!
//! The parser takes its listener by value and hands back nothing, so the
//! collector's state lives behind an `Rc<RefCell<Builder>>` shared between
//! the listener and [`collect`]; the per-event cost is one `RefCell` flag
//! check. Capture keys off declaration-carrying events only — expression
//! and reference identifier contexts are ignored, so bodies contribute
//! nothing:
//!
//! * Container opens (`begin_class_declaration`,
//!   `begin_named_mixin_application`, `begin_mixin_declaration`,
//!   `begin_enum_declaration`, `begin_extension_declaration`,
//!   `begin_extension_type_declaration`) push an open container and seed its
//!   top-level name; the matching end pops it. Unnamed extensions keep
//!   capturing (no top-level name); their members surface only in their
//!   [`ExtensionFace`].
//! * Superclass edges: `handle_class_extends(Some(_))` and
//!   `begin_named_mixin_application` arm a one-shot capture; the armed
//!   `TypeReference`/`TypeReferenceContinuation` identifier names the
//!   superclass (a qualified `p.S` keeps `S`, the continuation), committed
//!   at `handle_qualified`/`handle_type` — the first type after `extends`
//!   or the head type of a named mixin application. `with`/`implements`/`on`
//!   faces capture through their clause events, each event claiming only the
//!   names written inside its clause; unclaimed names (`extends A, B`
//!   overflow per `ast_builder.dart:3972-3982`, recovery residue) land in
//!   [`Container::header_residue`] and are never consulted as super-faces.
//!   Extensions latch their `on` type the same way (the [`ExtensionFace`]
//!   applicability anchor), and extension types capture their
//!   primary-constructor formals as the representation-field facts.
//! * Field groups (`begin_fields` … `end_fields`/`end_top_level_fields`)
//!   carry their `static` bit at open; each `FieldDeclaration`/
//!   `TopLevelVariableDeclaration` identifier commits immediately under
//!   that bit, with the group's declared [`Ty`] latched at the first name
//!   (one `parse_type` per group). Enum constants (`EnumValueDeclaration`)
//!   commit as static fields of the enum.
//! * Executables: `begin_method` carries name/static/get-or-set directly;
//!   a top-level method's name arrives via the
//!   `TopLevelFunctionDeclaration` identifier event and its get-or-set only
//!   at `end_top_level_method`. The formal table (positional + named, with
//!   declared types) is collected while `end_formal_parameter` reports at
//!   formal-nesting depth 1 and attached to the executable once (the
//!   attach-once guard keeps closures in bodies or default values from
//!   clobbering it). Return types latch at the name identifier event —
//!   `begin_method`/`begin_top_level_method` fire before the return type's
//!   `parse_type`, the name fires after it.
//! * Typedef names arrive via the `TypedefDeclaration` identifier event
//!   between `begin_typedef`/`end_typedef`.
//! * `begin_type_variable` names land in the unit's name set tagged
//!   `typeParam`; the depth-1 group's count latches onto the pending
//!   declaration (method, top-level function, typedef, or innermost open
//!   container). The `executable_header_done` flag keeps body declarations
//!   (local functions, closures) from clobbering a header count.
//! * Declared types arm at `handle_type`/`handle_void_keyword` and disarm
//!   at `handle_no_type` — every declaration head (typed or not) runs one
//!   `parse_type`, so the arm never leaks across declarations.
//! * Declaration-arity violations (operator parameter counts, setter
//!   parameter shape) are logged at commit into
//!   [`FileElements::arity_violations`]; hash-map order must not reach the
//!   report.
//!
//! Recovery-synthetic identifiers (parser-inserted token ids past the token
//! table) carry no honest text; their names are skipped. Setter names are
//! rewritten to `name=` at capture (one interning per setter — capture-time
//! cost, not lookup-time).

use std::cell::RefCell;
use std::rc::Rc;

use dartq_parser::{
    DeclarationKind, FormalParameterKind, IdentifierContext, Parser, ParserFeatures, ParserListener,
};
use dartq_parser::{MemberKind as ParserMemberKind, TokenSpan as Token};
use dartq_scanner::TokenTable;
use dartq_source::{StringId, StringTable};
use rustc_hash::FxHashMap;

use crate::model::{
    ArityDecl, ArityViolation, Container, ContainerKind, DuplicateDecl, ExtensionFace,
    FileElements, Formal, Formals, MemberEntry, MemberKind, MicroKind, RepFormal, TopDecl, Ty,
};

use dartq_source::SourceRange;

/// The binary/unary operator spellings with a fixed expected parameter count
/// (`error_verifier.dart:5688-5727`): `[]=` wants 2, these want 1, `~`
/// wants 0, and `-` allows 0 or 1 (minus template).
const OPERATORS_WITH_ONE_PARAMETER: [&str; 16] = [
    "<", ">", "<=", ">=", "==", "+", "/", "~/", "*", "%", "|", "^", "&", "<<", ">>", ">>>",
];

/// An open container on the capture stack.
struct OpenContainer {
    /// Lookup spelling with the name token's span (the duplicate log's
    /// location surface; the spelling is what the model keeps).
    name: Option<(StringId, SourceRange)>,
    kind: ContainerKind,
    superclass: Option<StringId>,
    /// The header's `abstract` modifier (class-family only).
    is_abstract: bool,
    members: FxHashMap<StringId, MemberEntry>,
    type_params: u16,
    /// The `sealed` modifier was present on the class header.
    sealed: bool,
    /// The header's clause type names, per clause family and in written
    /// order (the `with`/`implements`/`on` split).
    with_clause: Vec<StringId>,
    implements_clause: Vec<StringId>,
    on_clause: Vec<StringId>,
    /// Unattributed header-window names (extends overflow, recovery
    /// residue); never a super-face.
    header_residue: Vec<StringId>,
    /// Extension-type primary-constructor formals (the representation-field
    /// family's facts); empty for every other kind.
    primary_formals: Vec<RepFormal>,
    /// The enum's constants in declaration order.
    enum_constants: Vec<StringId>,
    /// The named constructor selectors declared so far, and whether an
    /// unnamed constructor exists (explicit or the implicit default).
    ctor_named: Vec<StringId>,
    ctor_has_unnamed: bool,
    /// The constructors' formal-parameter tables by lookup spelling (the
    /// selector for `A.named`, the container's name for `A()`); flushed
    /// into the finished [`Container::ctor_formals`].
    ctor_formals: FxHashMap<StringId, Box<Formals>>,
}

/// The open constructor's capture: the named selectors it declares plus
/// its unnamed flag (`A()` keeps the flag; `A.named()` records the selector
/// and clears it).
struct CtorCapture {
    named: Vec<StringId>,
    has_unnamed: bool,
}

/// The clause family a buffered header-window name belongs to; the parser's
/// clause-boundary events (`handle_class_with_clause` /
/// `handle_enum_with_clause` / `handle_mixin_on` / `handle_implements`)
/// decide it.
enum HeaderClause {
    With,
    Implements,
    On,
}

/// The attribution a clause boundary event applies to the buffered window:
/// the names the event claims join the clause list, the rest are residue
/// (`Container::header_residue`) — never consulted as super-faces.
enum ClauseClaim {
    /// The clause keyword's offset (`handle_*_with_clause`): names written
    /// after the keyword belong to the clause, names before it are the
    /// `extends` overflow the oracle discards
    /// (`ast_builder.dart:3972-3982`).
    Keyword(u32),
    /// The clause's own type count (`handle_implements` / `handle_mixin_on`
    /// carry it): the trailing `count` names belong to the clause, anything
    /// earlier is residue. A zero count claims nothing
    /// (`handle_implements(None, 0)`).
    Trailing(usize),
}

/// A class member executable between `begin_method` and `end_method`.
struct PendingMember {
    /// Lookup spelling, kind, and the name token's span; `None` when the name
    /// is unrecoverable.
    shape: Option<(StringId, MemberKind, SourceRange)>,
    is_static: bool,
    /// The `external` modifier was present.
    is_external: bool,
    /// A function body opened (`{ ... }` or `=> ...`) — its absence with a
    /// non-external member is the abstract shape.
    has_body: bool,
    type_params: u16,
    /// The member's own type-parameter names, in declaration order.
    type_param_names: Vec<StringId>,
    return_ty: Ty,
    formals: Option<Box<Formals>>,
}

/// A top-level executable between `begin_top_level_method` and its end; the
/// name arrives from the identifier event, get-or-set from the end event.
struct PendingTop {
    name: Option<(StringId, SourceRange)>,
    type_params: u16,
    return_ty: Ty,
    formals: Option<Box<Formals>>,
}

/// A typedef between `begin_typedef` and `end_typedef`.
struct PendingTypedef {
    name: Option<(StringId, SourceRange)>,
    type_params: u16,
}

/// The capture state shared between the listener and [`collect`].
struct Builder<'src, 'tab> {
    source: &'src str,
    tokens: &'tab TokenTable,
    strings: &'src StringTable,
    file: FileElements,
    containers: Vec<OpenContainer>,
    /// A class-family header is open (`with`/`implements` type names route
    /// to [`Builder::header_interfaces`]).
    in_class_header: bool,
    /// The header's clause type names buffered since the last clause
    /// boundary, with each name's source offset; the clause events
    /// (`handle_class_with_clause` / `handle_mixin_on` / `handle_implements`,
    /// ...) split it into the event-attributed clause names and
    /// [`Builder::header_residue`].
    header_interfaces: Vec<(StringId, u32)>,
    /// Header-window type names no clause event claimed: the `extends`
    /// overflow the oracle throws away (`ast_builder.dart:3972-3982`) and
    /// recovery residue. Committed to the open container's
    /// [`Container::header_residue`] at the boundary events; never a
    /// super-face.
    header_residue: Vec<StringId>,
    /// The open container's header window is still open (`begin_*_declaration`
    /// through the body-open event): the extension on-type latch and the
    /// extension-type primary-formal capture read it.
    header_capture_open: bool,
    /// The extension on-clause type name latched in the open header window
    /// (the first honest `handle_type` name); consumed at
    /// [`Builder::close_container`] into the [`ExtensionFace`].
    extension_on_type: Option<StringId>,
    /// The open field group's `abstract`/`external` modifiers (`begin_fields`
    /// latches, `end_fields` clears).
    field_abstract: bool,
    field_external: bool,
    /// The field group's `final` modifier (the abstract-field setter split).
    field_final: bool,
    /// The current field group's static bit (`None` = no group open).
    field_static: Option<bool>,
    pending_member: Option<PendingMember>,
    pending_top: Option<PendingTop>,
    pending_typedef: Option<PendingTypedef>,
    /// A constructor is open (`begin_constructor`..`end_constructor`): the
    /// named selectors it declares plus its unnamed flag. `A()` records the
    /// unnamed shape; `A.named()` records `named` only.
    ctor_open: Option<CtorCapture>,
    /// The open constructor's formal-parameter capture: its lookup spelling
    /// (the class-name token at `begin_constructor` — the unnamed face's
    /// key — rewritten to the selector at the continuation event) and the
    /// depth-0 group once it closes. `attach_formals` fills it first-wins
    /// (a body closure's own group is a no-op); `end_constructor` flushes
    /// it into the open container's `ctor_formals`.
    pending_ctor_formals: Option<(StringId, Option<Box<Formals>>)>,
    /// Formal-nesting depth; the executable's own group is depth 1.
    formal_depth: u32,
    positional_formals: Vec<Formal>,
    named_formals: Vec<Formal>,
    /// The current type-parameter list's nesting depth (the executable's
    /// own list is depth 1).
    type_group_depth: u32,
    /// Names in the current depth-1 type-parameter list (`duplicate_definition`
    /// compares per list, `duplicate_definition_verifier.dart:124-135`).
    type_var_group: Vec<StringId>,
    /// Count of the open depth-1 type-parameter list.
    type_param_count: u16,
    /// A depth-1 count that closed before its declaration's container
    /// opened: class/mixin/enum/extension headers parse their type
    /// variables before `begin_*_declaration` (official
    /// `parseClassOrNamedMixinApplication`), so the count carries here and
    /// [`Builder::open_container`] consumes it.
    header_type_params: u16,
    /// A class header is open (container opened, `extends` clause not yet
    /// committed): type references buffer as the superclass candidate. The
    /// official order parses the supertype before `handle_class_extends`
    /// (`parser_impl.dart` `parseClassExtendsSeenExtendsClause`), so the
    /// name can only be captured while the header is open.
    header_super_open: bool,
    super_candidate: Option<StringId>,
    /// Set once the pending executable's header is past its type-parameter
    /// position (formals began, or none exist); body declarations must not
    /// touch the header's counts.
    executable_header_done: bool,
    /// The armed declared type (`handle_type`/`handle_void_keyword` arm,
    /// `handle_no_type` disarms).
    armed_ty: Option<Ty>,
    /// The latched declared type of the open field group.
    field_group_ty: Option<Ty>,
}

impl<'src, 'tab> Builder<'src, 'tab> {
    fn new(source: &'src str, tokens: &'tab TokenTable, strings: &'src StringTable) -> Self {
        Self {
            source,
            tokens,
            strings,
            file: FileElements::default(),
            containers: Vec::new(),
            in_class_header: false,
            header_interfaces: Vec::new(),
            header_residue: Vec::new(),
            header_capture_open: false,
            extension_on_type: None,
            field_abstract: false,
            field_external: false,
            field_final: false,
            field_static: None,
            pending_member: None,
            pending_top: None,
            pending_typedef: None,
            ctor_open: None,
            pending_ctor_formals: None,
            formal_depth: 0,
            positional_formals: Vec::new(),
            named_formals: Vec::new(),
            type_group_depth: 0,
            type_var_group: Vec::new(),
            type_param_count: 0,
            header_type_params: 0,
            header_super_open: false,
            super_candidate: None,
            executable_header_done: false,
            armed_ty: None,
            field_group_ty: None,
        }
    }

    /// The token's name and span, `None` for parser-inserted synthetic tokens
    /// (which are not in the table and carry no honest name).
    fn capture_name(&self, token: Token) -> Option<(StringId, SourceRange)> {
        let range = token.range;
        self.name_of(token).map(|name| (name, range))
    }

    /// The named segment of an open constructor (`A.named` keeps `named`).
    /// A qualified non-constructor name is invalid Dart — ignored.
    fn note_constructor_continuation(&mut self, name: Option<StringId>) {
        if let Some(capture) = &mut self.ctor_open
            && let Some(name) = name
        {
            capture.has_unnamed = false;
            capture.named.push(name);
        }
        // The formal table's lookup key follows the selector: the unnamed
        // face's class-name key (set at `begin_constructor`) is rewritten
        // before any formal can fire (the selector precedes the parameter
        // list in the header).
        if let Some(name) = name
            && let Some((key, _)) = &mut self.pending_ctor_formals
        {
            *key = name;
        }
    }

    /// Records a duplicate (second) declaration; the name maps keep the
    /// first entry, this log is the `duplicate_definition` surface.
    fn record_duplicate(&mut self, name: StringId, range: SourceRange) {
        self.file.duplicates.push(DuplicateDecl { name, range });
    }

    /// Records a declaration-arity violation.
    fn record_arity(&mut self, violation: ArityViolation, range: SourceRange) {
        self.file
            .arity_violations
            .push(ArityDecl { violation, range });
    }

    /// Declares a top-level name, logging a duplicate when taken.
    fn declare_top(&mut self, name: StringId, range: SourceRange, decl: TopDecl) {
        match self.file.decls.entry(name) {
            std::collections::hash_map::Entry::Occupied(_) => self.record_duplicate(name, range),
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(decl);
            }
        }
    }

    /// The token's text, `None` for parser-inserted synthetic tokens (which
    /// are not in the table and carry no honest name).
    fn name_of(&self, token: Token) -> Option<StringId> {
        self.tokens
            .get(token.id as usize)
            .map(|t| self.strings.intern(&self.source[t.byte_range()]))
    }

    fn setter_spelling(&self, name: StringId) -> StringId {
        let setter = format!("{}=", self.strings.resolve(name));
        self.strings.intern(&setter)
    }

    /// The armed declared type, or [`Ty::UNKNOWN`] when the head had none.
    fn take_armed(&mut self) -> Ty {
        self.armed_ty.take().unwrap_or(Ty::UNKNOWN)
    }

    /// The field group's declared type: latched at the group's first name,
    /// reused by its remaining names (`int a, b`).
    fn field_group_ty(&mut self) -> Ty {
        if self.field_group_ty.is_none() {
            self.field_group_ty = Some(self.take_armed());
        }
        self.field_group_ty.expect("latched above")
    }

    fn open_container(
        &mut self,
        kind: ContainerKind,
        name: Option<(StringId, SourceRange)>,
        tag: MicroKind,
    ) {
        if let Some((name, range)) = name {
            self.declare_top(name, range, TopDecl::bare(tag));
        }
        // The header's type variables closed before this open (official
        // event order); they belong to this container.
        let header_type_params = std::mem::take(&mut self.header_type_params);
        // Class, enum, and mixin headers carry supertype clauses; the
        // window ends at the header-closing event (`handle_class_header` /
        // `handle_enum_header` / `handle_mixin_header`). A named mixin
        // application fires none of those — its buffer commits at the
        // declaration's end instead. Extension and extension-type headers
        // carry no subtype edges (the representation type is not one).
        self.in_class_header = matches!(
            kind,
            ContainerKind::Class | ContainerKind::Enum | ContainerKind::Mixin
        );
        self.header_interfaces.clear();
        self.header_residue.clear();
        // The extension on-type latch and the extension-type rep-formal
        // capture open with the container and close at the body events.
        self.header_capture_open = matches!(
            kind,
            ContainerKind::Extension | ContainerKind::ExtensionType
        );
        self.extension_on_type = None;
        self.containers.push(OpenContainer {
            name,
            kind,
            superclass: None,
            is_abstract: false,
            members: FxHashMap::default(),
            type_params: header_type_params,
            sealed: false,
            with_clause: Vec::new(),
            implements_clause: Vec::new(),
            on_clause: Vec::new(),
            header_residue: Vec::new(),
            primary_formals: Vec::new(),
            enum_constants: Vec::new(),
            ctor_named: Vec::new(),
            ctor_has_unnamed: true,
            ctor_formals: FxHashMap::default(),
        });
        // The `extends` head latches on class-family declarations only
        // (including a named mixin application's head type); enum
        // `with`/`implements` and mixin `on`/`implements` names are plain
        // supertype edges and buffer into the interface set.
        self.header_super_open = matches!(kind, ContainerKind::Class);
        self.super_candidate = None;
    }

    fn close_container(&mut self) {
        self.header_super_open = false;
        self.super_candidate = None;
        self.in_class_header = false;
        self.header_capture_open = false;
        self.header_interfaces.clear();
        let extension_on_type = self.extension_on_type.take();
        let Some(open) = self.containers.pop() else {
            return;
        };
        let members = open.members;
        if open.kind == ContainerKind::Extension && open.name.is_none() {
            // An unnamed extension has no container entry: its application
            // face owns the members outright. An on-type that never produced
            // an honest name stays `None` — the face can never apply, so its
            // members only ever suppress (under-report, never
            // false-positive).
            self.file.extension_faces.push(ExtensionFace {
                name: None,
                on_type: extension_on_type,
                members,
            });
            return;
        }
        if open.kind == ContainerKind::Extension {
            // A named extension is both an application face and a
            // named container; the two hold the same member set.
            self.file.extension_faces.push(ExtensionFace {
                name: open.name.map(|(name, _)| name),
                on_type: extension_on_type,
                members: members.clone(),
            });
        }
        if let Some((name, _range)) = open.name {
            self.file.container_order.push(name);
            self.file.containers.entry(name).or_insert(Container {
                kind: open.kind,
                is_abstract: open.is_abstract,
                superclass: open.superclass,
                members,
                type_params: open.type_params,
                sealed: open.sealed,
                with_clause: open.with_clause,
                implements_clause: open.implements_clause,
                on_clause: open.on_clause,
                header_residue: open.header_residue,
                primary_formals: open.primary_formals.into_boxed_slice(),
                enum_constants: open.enum_constants,
                constructors: open.ctor_named.into_boxed_slice(),
                has_unnamed_constructor: open.ctor_has_unnamed,
                ctor_formals: open.ctor_formals,
            });
        }
        // Extension headers capture no residue (their window's types latch
        // through `note_header_reference`, not the clause buffer), so
        // nothing else to commit here.
    }

    /// Marks the open container `sealed` (the `sealed` modifier event).
    fn mark_sealed(&mut self) {
        if let Some(open) = self.containers.last_mut() {
            open.sealed = true;
        }
    }

    /// Marks the open container `abstract` (the class header's `abstract`
    /// modifier; class-family declarations only).
    fn mark_abstract(&mut self) {
        if let Some(open) = self.containers.last_mut() {
            open.is_abstract = true;
        }
    }

    /// Ends the declaration-header window (`handle_class_header` /
    /// `handle_enum_header` / `handle_mixin_header`): every still-buffered
    /// name is unattributed residue. The clause events normally drain the
    /// window first, so what reaches here is recovery residue — plus a named
    /// mixin application's `implements` names until the parser lands their
    /// real `handle_implements(Some, count)` emission (ticket #460; until
    /// then that form's interface edges under-report, never false-positive).
    fn end_header_window(&mut self) {
        self.in_class_header = false;
        let buffered = std::mem::take(&mut self.header_interfaces);
        let residue = &mut self.header_residue;
        if let Some(open) = self.containers.last_mut() {
            open.header_residue
                .extend(buffered.into_iter().map(|(name, _)| name));
        } else {
            residue.extend(buffered.into_iter().map(|(name, _)| name));
        }
    }

    /// Splits the buffered window at a clause boundary: the claimed names go
    /// to the open container's `which` clause list, the unclaimed ones to
    /// its residue.
    fn commit_header_clause(&mut self, which: HeaderClause, claim: ClauseClaim) {
        let buffered = std::mem::take(&mut self.header_interfaces);
        let (claimed, residue) = match claim {
            ClauseClaim::Keyword(offset) => {
                let at = buffered.partition_point(|(_, at)| *at <= offset);
                (buffered[at..].to_vec(), buffered[..at].to_vec())
            }
            ClauseClaim::Trailing(count) => {
                let at = buffered.len().saturating_sub(count);
                (buffered[at..].to_vec(), buffered[..at].to_vec())
            }
        };
        if let Some(open) = self.containers.last_mut() {
            match which {
                HeaderClause::With => open.with_clause.extend(claimed.into_iter().map(|(n, _)| n)),
                HeaderClause::Implements => {
                    open.implements_clause
                        .extend(claimed.into_iter().map(|(n, _)| n));
                }
                HeaderClause::On => open.on_clause.extend(claimed.into_iter().map(|(n, _)| n)),
            }
            open.header_residue
                .extend(residue.into_iter().map(|(n, _)| n));
        }
    }

    /// Buffers one header clause type name with its source offset
    /// (declarations.rs:1129-1172); the clause-boundary events later claim
    /// the names their event attributes and route the rest to residue.
    fn buffer_header_interface(&mut self, name: Option<StringId>, offset: u32) {
        if let Some(name) = name {
            self.header_interfaces.push((name, offset));
        }
    }

    /// Routes a class-header type reference: the `extends` head latches the
    /// superclass candidate (a qualified `p.S` through the continuation),
    /// and past it the `with`/`implements` names buffer toward the
    /// container's interface edges. Inside an extension header window, the
    /// first plain reference latches the `on` type and the qualified
    /// continuation overwrites it (`on p.T` keeps `T`, the model's `p.S`
    /// convention).
    fn note_header_reference(
        &mut self,
        context: IdentifierContext,
        name: Option<StringId>,
        offset: u32,
    ) {
        // The extension on-type latch (a container is always open here).
        if self.header_capture_open
            && self
                .containers
                .last()
                .is_some_and(|open| open.kind == ContainerKind::Extension)
        {
            match context {
                IdentifierContext::TypeReference => {
                    if self.extension_on_type.is_none() {
                        self.extension_on_type = name;
                    }
                }
                IdentifierContext::TypeReferenceContinuation => {
                    self.extension_on_type = name;
                }
                _ => {}
            }
            return;
        }
        match context {
            IdentifierContext::TypeReference => {
                if self.header_super_open && self.super_candidate.is_none() {
                    self.super_candidate = name;
                } else if self.in_class_header {
                    self.buffer_header_interface(name, offset);
                }
            }
            IdentifierContext::TypeReferenceContinuation if self.header_super_open => {
                // Qualified `p.S`: the continuation names the type.
                self.super_candidate = name;
            }
            _ => {}
        }
    }

    /// Commits the buffered header type reference as the superclass edge
    /// (the `extends` clause or the named mixin application's head type).
    fn commit_super_candidate(&mut self) {
        let candidate = self.super_candidate.take();
        self.header_super_open = false;
        if let Some(open) = self.containers.last_mut()
            && open.superclass.is_none()
        {
            open.superclass = candidate;
        }
    }

    /// Resolves a method's member name and kind from its get-or-set token.
    fn member_shape(&self, get_or_set: Option<Token>, name: StringId) -> (StringId, MemberKind) {
        let kw = get_or_set.and_then(|t| {
            self.tokens
                .get(t.id as usize)
                .map(|tok| &self.source[tok.byte_range()])
        });
        match kw {
            Some("get") => (name, MemberKind::Getter),
            Some("set") => (self.setter_spelling(name), MemberKind::Setter),
            _ => (name, MemberKind::Method),
        }
    }

    /// The member's arity-violation log entry, when the declaration's
    /// parameter shape is wrong (`error_verifier.dart:5688-5740` for
    /// operators, `ast_builder.dart:6283-6291` for setters).
    fn member_arity_violation(
        &self,
        spelling: StringId,
        kind: MemberKind,
        formals: Option<&Formals>,
    ) -> Option<ArityViolation> {
        let found = formals.map_or(0_u16, |f| {
            u16::try_from(f.positional.len() + f.named.len()).unwrap_or(u16::MAX)
        });
        match kind {
            MemberKind::Setter => {
                let ok = formals.is_some_and(|f| {
                    f.named.is_empty() && f.positional.len() == 1 && f.positional[0].required
                });
                (!ok).then_some(ArityViolation::Setter)
            }
            MemberKind::Method => {
                let text = self.strings.resolve(spelling);
                if text == "-" {
                    (found > 1).then_some(ArityViolation::OperatorMinus { found })
                } else if text == "[]=" {
                    (found != 2).then_some(ArityViolation::Operator {
                        name: spelling,
                        expected: 2,
                        found,
                    })
                } else if text == "~" {
                    (found != 0).then_some(ArityViolation::Operator {
                        name: spelling,
                        expected: 0,
                        found,
                    })
                } else if OPERATORS_WITH_ONE_PARAMETER.contains(&text) || text == "[]" {
                    (found != 1).then_some(ArityViolation::Operator {
                        name: spelling,
                        expected: 1,
                        found,
                    })
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Attaches the depth-0 formal group to the pending executable, once.
    /// The constructor arm is first-wins like the member arm: a body
    /// closure's own group re-fires this at depth 0→1 after the header
    /// closed, and must not displace the constructor's table.
    fn attach_formals(&mut self) {
        let formals = Formals {
            positional: std::mem::take(&mut self.positional_formals).into_boxed_slice(),
            named: std::mem::take(&mut self.named_formals).into_boxed_slice(),
        };
        if let Some(pending) = &mut self.pending_member {
            if pending.formals.is_none() {
                pending.formals = Some(Box::new(formals));
            }
        } else if let Some(pending) = &mut self.pending_top
            && pending.formals.is_none()
        {
            pending.formals = Some(Box::new(formals));
        } else if let Some((_, slot)) = &mut self.pending_ctor_formals
            && slot.is_none()
        {
            *slot = Some(Box::new(formals));
        }
    }

    /// Latches the depth-1 type-parameter count onto the pending declaration
    /// (method, top-level function, typedef, or innermost open container).
    /// Attaches the closed depth-1 count to the declaration it belongs to;
    /// `false` when nothing was open to receive it (a container header —
    /// [`Builder::open_container`] then consumes [`Builder::
    /// header_type_params`]).
    fn attach_type_param_count(&mut self) -> bool {
        if self.executable_header_done {
            return false;
        }
        let count = self.type_param_count;
        if let Some(pending) = &mut self.pending_member {
            pending.type_params = count;
            pending.type_param_names = std::mem::take(&mut self.type_var_group);
        } else if let Some(pending) = &mut self.pending_top {
            pending.type_params = count;
        } else if let Some(pending) = &mut self.pending_typedef {
            pending.type_params = count;
        } else if let Some(open) = self.containers.last_mut() {
            open.type_params = count;
        } else {
            return false;
        }
        true
    }

    /// The pending executable's arity log entries, drained at commit.
    fn drain_arity(
        &mut self,
        spelling: StringId,
        kind: MemberKind,
        range: SourceRange,
        formals: Option<&Formals>,
    ) {
        if let Some(violation) = self.member_arity_violation(spelling, kind, formals) {
            self.record_arity(violation, range);
        }
    }

    fn finish(mut self) -> FileElements {
        // A bailed parse can leave containers open; flush them so captured
        // members still surface (under-report bias).
        while !self.containers.is_empty() {
            self.close_container();
        }
        self.file
    }
}

/// The [`ParserListener`] implementation feeding a shared [`Builder`].
struct DeclarationCollector<'src, 'tab> {
    builder: Rc<RefCell<Builder<'src, 'tab>>>,
}

/// Parses one compilation unit and returns what it declares.
///
/// `tokens` must be the scan of `source` (the parser reads lexemes through
/// the same pair); `strings` receives every interned declaration name and
/// must be the table the caller resolves ids against afterwards.
///
/// # Panics
///
/// Panics never in practice: the single `expect` reclaims the shared
/// builder after the parser (its sole co-owner) is dropped, so the strong
/// count is 1 by construction.
pub fn collect(
    source: &str,
    tokens: &TokenTable,
    features: ParserFeatures,
    strings: &StringTable,
) -> FileElements {
    let builder = Rc::new(RefCell::new(Builder::new(source, tokens, strings)));
    let mut parser = Parser::new(
        source,
        tokens,
        features,
        DeclarationCollector {
            builder: Rc::clone(&builder),
        },
    );
    parser.parse_unit();
    drop(parser);
    // The parser held the only other strong reference and is gone, so the
    // unwrap cannot fail.
    Rc::into_inner(builder)
        .expect("the parser released its listener")
        .into_inner()
        .finish()
}

impl ParserListener for DeclarationCollector<'_, '_> {
    fn begin_class_declaration(
        &mut self,
        _begin: Token,
        abstract_kw: Option<Token>,
        _sealed_kw: Option<Token>,
        _base_kw: Option<Token>,
        _interface_kw: Option<Token>,
        _final_kw: Option<Token>,
        _augment_kw: Option<Token>,
        _mixin_kw: Option<Token>,
        name: Token,
    ) {
        let sealed = _sealed_kw.is_some();
        let builder = &mut *self.builder.borrow_mut();
        let name = builder.capture_name(name);
        builder.open_container(ContainerKind::Class, name, MicroKind::Class);
        if sealed {
            builder.mark_sealed();
        }
        if abstract_kw.is_some() {
            builder.mark_abstract();
        }
    }

    fn handle_class_header(&mut self, _begin: Token, _class: Token, _native: Option<Token>) {
        self.builder.borrow_mut().end_header_window();
    }

    fn end_class_declaration(&mut self, _begin: Token, _end: Token) {
        self.builder.borrow_mut().close_container();
    }

    fn begin_named_mixin_application(
        &mut self,
        _begin: Token,
        abstract_kw: Option<Token>,
        _sealed_kw: Option<Token>,
        _base_kw: Option<Token>,
        _interface_kw: Option<Token>,
        _final_kw: Option<Token>,
        _augment_kw: Option<Token>,
        _mixin_kw: Option<Token>,
        name: Token,
    ) {
        let builder = &mut *self.builder.borrow_mut();
        let name = builder.capture_name(name);
        builder.open_container(ContainerKind::Class, name, MicroKind::Class);
        if abstract_kw.is_some() {
            builder.mark_abstract();
        }
    }

    fn end_named_mixin_application(
        &mut self,
        _begin: Token,
        _class: Token,
        _equals: Token,
        _implements: Option<Token>,
        _end: Token,
    ) {
        let builder = &mut *self.builder.borrow_mut();
        // No header-closing event fires for a mixin application: the
        // buffered `with`/`implements` names commit here, before the
        // container closes and would drop them.
        builder.end_header_window();
        builder.close_container();
    }

    fn begin_mixin_declaration(
        &mut self,
        _begin: Token,
        _augment_kw: Option<Token>,
        _base_kw: Option<Token>,
        _mixin: Token,
        name: Token,
    ) {
        let builder = &mut *self.builder.borrow_mut();
        let name = builder.capture_name(name);
        builder.open_container(ContainerKind::Mixin, name, MicroKind::Mixin);
    }

    fn end_mixin_declaration(&mut self, _begin: Token, _end: Token) {
        self.builder.borrow_mut().close_container();
    }

    fn begin_enum_declaration(
        &mut self,
        _begin: Token,
        _augment_kw: Option<Token>,
        _enum_keyword: Token,
        name: Token,
    ) {
        let builder = &mut *self.builder.borrow_mut();
        let name = builder.capture_name(name);
        builder.open_container(ContainerKind::Enum, name, MicroKind::Enum);
    }

    fn end_enum_declaration(
        &mut self,
        _begin: Token,
        _enum_keyword: Token,
        _left_brace: Token,
        _member_count: usize,
        _end: Token,
    ) {
        self.builder.borrow_mut().close_container();
    }

    fn handle_enum_header(
        &mut self,
        _augment: Option<Token>,
        _enum_keyword: Token,
        _left_brace: Token,
    ) {
        self.builder.borrow_mut().end_header_window();
    }

    fn handle_mixin_header(&mut self, _mixin: Token) {
        self.builder.borrow_mut().end_header_window();
    }

    fn begin_extension_declaration(
        &mut self,
        _augment: Option<Token>,
        _extension_keyword: Token,
        name: Option<Token>,
    ) {
        let builder = &mut *self.builder.borrow_mut();
        let name = name.and_then(|token| builder.capture_name(token));
        builder.open_container(ContainerKind::Extension, name, MicroKind::Extension);
    }

    fn end_extension_declaration(
        &mut self,
        _begin: Token,
        _extension_keyword: Token,
        _on: Option<Token>,
        _end: Token,
    ) {
        self.builder.borrow_mut().close_container();
    }

    fn begin_extension_type_declaration(
        &mut self,
        _augment_kw: Option<Token>,
        _extension_keyword: Token,
        name: Token,
    ) {
        let builder = &mut *self.builder.borrow_mut();
        let name = builder.capture_name(name);
        builder.open_container(ContainerKind::ExtensionType, name, MicroKind::Extension);
    }

    fn end_extension_type_declaration(
        &mut self,
        _begin: Token,
        _augment_kw: Option<Token>,
        _extension_keyword: Token,
        _type_keyword: Token,
        _end: Token,
    ) {
        self.builder.borrow_mut().close_container();
    }

    fn handle_class_extends(&mut self, _extends: Option<Token>, _count: usize) {
        // The supertype's type events fired before this event (official
        // `parseClassExtendsSeenExtendsClause`), so the buffered header
        // reference — present only with `extends` — is the edge.
        self.builder.borrow_mut().commit_super_candidate();
    }

    fn handle_named_mixin_application_with_clause(&mut self, with: Token) {
        // The head type fired between `begin_named_mixin_application` and
        // this event; the `with` list itself is not the superclass. Names
        // written before the `with` keyword are head-overflow residue.
        let offset = with.range.offset;
        let mut builder = self.builder.borrow_mut();
        builder.commit_super_candidate();
        builder.commit_header_clause(HeaderClause::With, ClauseClaim::Keyword(offset));
    }

    fn handle_class_with_clause(&mut self, with_keyword: Token) {
        let offset = with_keyword.range.offset;
        self.builder
            .borrow_mut()
            .commit_header_clause(HeaderClause::With, ClauseClaim::Keyword(offset));
    }

    fn handle_enum_with_clause(&mut self, with_keyword: Token) {
        let offset = with_keyword.range.offset;
        self.builder
            .borrow_mut()
            .commit_header_clause(HeaderClause::With, ClauseClaim::Keyword(offset));
    }

    fn handle_mixin_on(&mut self, _on: Option<Token>, count: usize) {
        self.builder
            .borrow_mut()
            .commit_header_clause(HeaderClause::On, ClauseClaim::Trailing(count));
    }

    fn handle_implements(&mut self, _implements: Option<Token>, count: usize) {
        self.builder
            .borrow_mut()
            .commit_header_clause(HeaderClause::Implements, ClauseClaim::Trailing(count));
    }

    fn begin_class_or_mixin_or_extension_body(&mut self, kind: DeclarationKind, _token: Token) {
        let mut builder = self.builder.borrow_mut();
        // The header window closes with the body: extension on-types and
        // extension-type representation formals stop being captured. The
        // kind is informational here — the container's own kind decides.
        let _ = kind;
        builder.header_capture_open = false;
    }

    fn handle_no_class_body(&mut self, _semicolon: Token) {
        self.builder.borrow_mut().header_capture_open = false;
    }

    fn handle_no_mixin_body(&mut self, _semicolon: Token) {
        self.builder.borrow_mut().header_capture_open = false;
    }

    fn handle_no_enum_body(&mut self, _semicolon: Token) {
        self.builder.borrow_mut().header_capture_open = false;
    }

    fn handle_no_extension_body(&mut self, _semicolon: Token) {
        self.builder.borrow_mut().header_capture_open = false;
    }

    fn handle_no_extension_type_body(&mut self, _semicolon: Token) {
        self.builder.borrow_mut().header_capture_open = false;
    }

    fn handle_type(&mut self, name: Token, question: Option<Token>) {
        let mut builder = self.builder.borrow_mut();
        builder.armed_ty = builder.name_of(name).map(|name| Ty::Named {
            name,
            nullable: question.is_some(),
        });
    }
    fn handle_no_type(&mut self, _last_consumed: Token) {
        self.builder.borrow_mut().armed_ty = None;
    }

    fn handle_void_keyword(&mut self, _token: Token) {
        self.builder.borrow_mut().armed_ty = Some(Ty::Void);
    }

    fn handle_void_keyword_with_type_arguments(&mut self, _token: Token) {
        self.builder.borrow_mut().armed_ty = Some(Ty::Void);
    }

    fn handle_identifier(&mut self, token: Token, context: IdentifierContext) {
        let mut builder = self.builder.borrow_mut();
        // Resolve the name and its span up front: the branches below hold
        // borrows into the builder's state.
        let name_with_range = builder.capture_name(token);
        let name = name_with_range.map(|(name, _)| name);
        match context {
            IdentifierContext::TypeReference | IdentifierContext::TypeReferenceContinuation => {
                let offset = name_with_range.map_or(0, |(_, range)| range.offset);
                builder.note_header_reference(context, name, offset);
            }
            IdentifierContext::TopLevelFunctionDeclaration => {
                // The return type parsed between the begin event and the
                // name; the armed type is it.
                let armed = builder.take_armed();
                if let Some(pending) = &mut builder.pending_top {
                    pending.name = name_with_range;
                    pending.return_ty = armed;
                }
            }
            IdentifierContext::MethodDeclaration => {
                // The return type parsed between the begin event and the
                // name; the armed type is it. Operators never re-fire
                // their name here — their return types stay unknown.
                let armed = builder.take_armed();
                if let Some(pending) = &mut builder.pending_member {
                    pending.return_ty = armed;
                }
            }
            IdentifierContext::MethodDeclarationContinuation => {
                builder.note_constructor_continuation(name);
            }
            IdentifierContext::TopLevelVariableDeclaration
            | IdentifierContext::FieldDeclaration => {
                if let Some((name, range)) = name_with_range {
                    let ty = builder.field_group_ty();
                    let is_static = builder.field_static.unwrap_or(false);
                    let is_abstract = builder.field_abstract;
                    let is_external = builder.field_external;
                    let is_final = builder.field_final;
                    if context == IdentifierContext::TopLevelVariableDeclaration {
                        builder.declare_top(
                            name,
                            range,
                            TopDecl {
                                kind: MicroKind::TopLevelVar,
                                type_params: 0,
                                var_ty: ty,
                                return_ty: Ty::UNKNOWN,
                                formals: None,
                            },
                        );
                    } else if let Some(open) = builder.containers.last_mut() {
                        match open.members.entry(name) {
                            std::collections::hash_map::Entry::Vacant(slot) => {
                                slot.insert(MemberEntry {
                                    kind: MemberKind::Field,
                                    is_static,
                                    is_abstract,
                                    is_external,
                                    is_final,
                                    type_params: 0,
                                    type_param_names: Box::default(),
                                    var_ty: ty,
                                    return_ty: Ty::UNKNOWN,
                                    formals: None,
                                });
                            }
                            // Same-bit collision: the second field is a
                            // duplicate. Keep-first; a cross-bit coexistence
                            // is `conflicting_static_and_instance`'s
                            // business, not a duplicate.
                            std::collections::hash_map::Entry::Occupied(existing) => {
                                if existing.get().is_static == is_static {
                                    builder.record_duplicate(name, range);
                                }
                            }
                        }
                    }
                }
            }
            IdentifierContext::EnumValueDeclaration => {
                if let (Some(open), Some((name, range))) =
                    (builder.containers.last_mut(), name_with_range)
                    && open.kind == ContainerKind::Enum
                {
                    // Enum constants are static fields; a collision is a
                    // same-bit duplicate.
                    open.enum_constants.push(name);
                    match open.members.entry(name) {
                        std::collections::hash_map::Entry::Vacant(slot) => {
                            slot.insert(MemberEntry {
                                kind: MemberKind::Field,
                                is_static: true,
                                is_abstract: false,
                                is_external: false,
                                is_final: false,
                                type_params: 0,
                                type_param_names: Box::default(),
                                var_ty: Ty::UNKNOWN,
                                return_ty: Ty::UNKNOWN,
                                formals: None,
                            });
                        }
                        std::collections::hash_map::Entry::Occupied(existing) => {
                            if existing.get().is_static {
                                builder.record_duplicate(name, range);
                            }
                        }
                    }
                }
            }
            IdentifierContext::TypedefDeclaration => {
                if let Some(pending) = &mut builder.pending_typedef
                    && pending.name.is_none()
                {
                    pending.name = name_with_range;
                }
            }
            _ => {}
        }
    }

    fn begin_type_variables(&mut self, _token: Token) {
        let mut builder = self.builder.borrow_mut();
        builder.type_group_depth += 1;
        if builder.type_group_depth == 1 {
            builder.type_param_count = 0;
        }
        builder.type_var_group.clear();
    }

    fn begin_type_variable(&mut self, token: Token) {
        let mut builder = self.builder.borrow_mut();
        if let Some((name, range)) = builder.capture_name(token) {
            // `duplicate_definition` compares type parameters per list
            // (`duplicate_definition_verifier.dart:124-135`); the unit-wide
            // name set keeps its first entry only.
            if builder.type_var_group.contains(&name) {
                builder.record_duplicate(name, range);
            } else {
                builder.type_var_group.push(name);
            }
            builder
                .file
                .decls
                .entry(name)
                .or_insert(TopDecl::bare(MicroKind::TypeParam));
            if builder.type_group_depth == 1 {
                builder.type_param_count += 1;
            }
        }
    }

    fn end_type_variables(&mut self, _begin: Token, _end: Token) {
        let mut builder = self.builder.borrow_mut();
        if builder.type_group_depth == 1 && !builder.attach_type_param_count() {
            // A container header: the count carries into the container's
            // `begin_*_declaration` open (official order — the type
            // variables parse before `beginClassDeclaration`).
            builder.header_type_params = builder.type_param_count;
        }
        builder.type_group_depth -= 1;
    }

    fn begin_fields(
        &mut self,
        _kind: DeclarationKind,
        _augment_kw: Option<Token>,
        _abstract_kw: Option<Token>,
        _external_kw: Option<Token>,
        static_kw: Option<Token>,
        _covariant_kw: Option<Token>,
        _late_kw: Option<Token>,
        _var_final_or_const: Option<Token>,
        _last_consumed: Token,
    ) {
        let mut builder = self.builder.borrow_mut();
        builder.field_static = Some(static_kw.is_some());
        builder.field_abstract = _abstract_kw.is_some();
        builder.field_external = _external_kw.is_some();
        builder.field_final = _var_final_or_const
            .and_then(|t| builder.tokens.get(t.id as usize))
            .is_some_and(|tok| &builder.source[tok.byte_range()] == "final");
    }

    fn end_fields(
        &mut self,
        _kind: DeclarationKind,
        _abstract_kw: Option<Token>,
        _augment_kw: Option<Token>,
        _external_kw: Option<Token>,
        _static_kw: Option<Token>,
        _covariant_kw: Option<Token>,
        _late_kw: Option<Token>,
        _var_final_or_const: Option<Token>,
        _count: usize,
        _begin: Token,
        _end: Token,
    ) {
        let mut builder = self.builder.borrow_mut();
        builder.field_static = None;
        builder.field_group_ty = None;
        builder.armed_ty = None;
        builder.field_abstract = false;
        builder.field_external = false;
        builder.field_final = false;
    }

    fn end_top_level_fields(
        &mut self,
        _augment_kw: Option<Token>,
        _abstract_kw: Option<Token>,
        _external_kw: Option<Token>,
        _static_kw: Option<Token>,
        _covariant_kw: Option<Token>,
        _late_kw: Option<Token>,
        _var_final_or_const: Option<Token>,
        _count: usize,
        _begin: Token,
        _end: Token,
    ) {
        let mut builder = self.builder.borrow_mut();
        builder.field_static = None;
        builder.field_group_ty = None;
        builder.armed_ty = None;
        builder.field_abstract = false;
        builder.field_external = false;
        builder.field_final = false;
    }

    #[allow(clippy::too_many_arguments)]
    fn begin_method(
        &mut self,
        _kind: DeclarationKind,
        _augment_kw: Option<Token>,
        _external_kw: Option<Token>,
        static_kw: Option<Token>,
        _covariant_kw: Option<Token>,
        _var_final_or_const: Option<Token>,
        get_or_set: Option<Token>,
        name: Token,
        _enclosing_declaration_name: Option<&str>,
    ) {
        let mut builder = self.builder.borrow_mut();
        let shape = builder
            .capture_name(name)
            .map(|(n, range)| (builder.member_shape(get_or_set, n), range))
            .map(|((spelling, kind), range)| (spelling, kind, range));
        builder.pending_member = Some(PendingMember {
            shape,
            is_static: static_kw.is_some(),
            is_external: _external_kw.is_some(),
            has_body: false,
            type_params: 0,
            type_param_names: Vec::new(),
            return_ty: Ty::UNKNOWN,
            formals: None,
        });
        builder.executable_header_done = false;
    }

    /// A block function body opened inside the pending member — the
    /// has-body mark (its absence at `end_method` is the abstract shape).
    /// A closure in a formal default fires this before any real body, which
    /// can only mislabel an abstract member concrete: the under-report
    /// direction.
    fn begin_block_function_body(&mut self, _token: Token) {
        if let Some(pending) = self.builder.borrow_mut().pending_member.as_mut() {
            pending.has_body = true;
        }
    }

    fn handle_expression_function_body(&mut self, _arrow: Token, _end: Option<Token>) {
        if let Some(pending) = self.builder.borrow_mut().pending_member.as_mut() {
            pending.has_body = true;
        }
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
        let mut builder = self.builder.borrow_mut();
        if let Some(pending) = builder.pending_member.take()
            && let Some((name, kind, range)) = pending.shape
        {
            let formals = pending.formals.as_deref();
            builder.drain_arity(name, kind, range, formals);
            if let Some(open) = builder.containers.last_mut() {
                let entry = MemberEntry {
                    kind,
                    is_static: pending.is_static,
                    // A body-less non-external method/getter/setter is the
                    // abstract shape (`void m();`); the `abstract` fields'
                    // bit latched at `begin_fields`.
                    is_abstract: !pending.has_body && !pending.is_external,
                    is_external: pending.is_external,
                    is_final: false,
                    type_params: pending.type_params,
                    type_param_names: pending.type_param_names.into_boxed_slice(),
                    var_ty: Ty::UNKNOWN,
                    return_ty: pending.return_ty,
                    formals: pending.formals,
                };
                match open.members.entry(name) {
                    std::collections::hash_map::Entry::Vacant(slot) => {
                        slot.insert(entry);
                    }
                    // Same-bit collisions are duplicates; cross-bit ones are not.
                    std::collections::hash_map::Entry::Occupied(existing) => {
                        if existing.get().is_static == pending.is_static {
                            builder.record_duplicate(name, range);
                        }
                    }
                }
            }
        }
        builder.executable_header_done = false;
    }

    fn begin_top_level_method(
        &mut self,
        _last_consumed: Token,
        _augment: Option<Token>,
        _external: Option<Token>,
    ) {
        let mut builder = self.builder.borrow_mut();
        builder.pending_top = Some(PendingTop {
            name: None,
            type_params: 0,
            return_ty: Ty::UNKNOWN,
            formals: None,
        });
        builder.executable_header_done = false;
    }

    fn end_top_level_method(&mut self, _begin: Token, get_or_set: Option<Token>, _end: Token) {
        let mut builder = self.builder.borrow_mut();
        if let Some(pending) = builder.pending_top.take()
            && let Some((name, range)) = pending.name
        {
            let spelling = if get_or_set.is_some() {
                // Setter or getter: setters live under `name=`; getters keep
                // the plain name (both tagged topLevelFn — the adjudicated
                // set has no finer top-level tag).
                match get_or_set.and_then(|t| {
                    builder
                        .tokens
                        .get(t.id as usize)
                        .map(|tok| &builder.source[tok.byte_range()])
                }) {
                    Some("set") => builder.setter_spelling(name),
                    _ => name,
                }
            } else {
                name
            };
            let setter = get_or_set.is_some_and(|t| {
                builder
                    .tokens
                    .get(t.id as usize)
                    .is_some_and(|tok| &builder.source[tok.byte_range()] == "set")
            });
            let kind = if setter {
                MemberKind::Setter
            } else {
                MemberKind::Method
            };
            builder.drain_arity(spelling, kind, range, pending.formals.as_deref());
            builder.declare_top(
                spelling,
                range,
                TopDecl {
                    kind: MicroKind::TopLevelFn,
                    type_params: pending.type_params,
                    var_ty: Ty::UNKNOWN,
                    return_ty: pending.return_ty,
                    formals: pending.formals,
                },
            );
        }
        builder.executable_header_done = false;
    }

    fn begin_typedef(&mut self, _token: Token) {
        let mut builder = self.builder.borrow_mut();
        builder.pending_typedef = Some(PendingTypedef {
            name: None,
            type_params: 0,
        });
    }

    #[allow(clippy::too_many_arguments)]
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
        let mut builder = self.builder.borrow_mut();
        // `A()` (or the head recovered): the unnamed shape until a
        // `A.named` continuation says otherwise. The formal table's key is
        // the head token's name — the class name, which is exactly the
        // unnamed face's lookup spelling (`A()` creation rows carry it);
        // the continuation rewrites it to the selector.
        builder.ctor_open = Some(CtorCapture {
            named: Vec::new(),
            has_unnamed: true,
        });
        builder.pending_ctor_formals = builder
            .capture_name(_name)
            .map(|(key, _)| (key, None));
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
        let mut builder = self.builder.borrow_mut();
        if let Some(capture) = builder.ctor_open.take()
            && let Some(open) = builder.containers.last_mut()
        {
            for name in capture.named {
                if !open.ctor_named.contains(&name) {
                    open.ctor_named.push(name);
                }
            }
            // Any unnamed constructor (this one, or the implicit default the
            // container opened with) keeps the unnamed face.
            open.ctor_has_unnamed &= capture.has_unnamed;
        }
        // The captured formal table joins the container's constructor
        // formals under its lookup spelling (a redirecting or body-less
        // shape with no group simply carries nothing).
        if let Some((key, formals)) = builder.pending_ctor_formals.take()
            && let Some(formals) = formals
            && let Some(open) = builder.containers.last_mut()
        {
            open.ctor_formals.insert(key, formals);
        }
    }

    fn end_typedef(
        &mut self,
        _augment: Option<Token>,
        _typedef: Token,
        _equals: Option<Token>,
        _end: Token,
    ) {
        let mut builder = self.builder.borrow_mut();
        if let Some(pending) = builder.pending_typedef.take()
            && let Some((name, range)) = pending.name
        {
            builder.declare_top(
                name,
                range,
                TopDecl {
                    kind: MicroKind::TypeAlias,
                    type_params: pending.type_params,
                    var_ty: Ty::UNKNOWN,
                    return_ty: Ty::UNKNOWN,
                    formals: None,
                },
            );
        }
    }

    fn begin_formal_parameters(&mut self, _token: Token, _kind: ParserMemberKind) {
        let mut builder = self.builder.borrow_mut();
        if builder.formal_depth == 0 {
            builder.positional_formals.clear();
            builder.named_formals.clear();
            // The executable's type parameters must all precede its formals.
            builder.executable_header_done = true;
        }
        builder.formal_depth += 1;
    }

    fn end_formal_parameters(
        &mut self,
        _count: usize,
        _begin: Token,
        _end: Token,
        _kind: ParserMemberKind,
    ) {
        let mut builder = self.builder.borrow_mut();
        builder.formal_depth -= 1;
        if builder.formal_depth == 0 {
            builder.attach_formals();
        }
    }

    fn handle_no_formal_parameters(&mut self, _token: Token, _kind: ParserMemberKind) {
        let mut builder = self.builder.borrow_mut();
        builder.executable_header_done = true;
    }

    fn handle_async_modifier(&mut self, _async_kw: Option<Token>, _star: Option<Token>) {
        let mut builder = self.builder.borrow_mut();
        builder.executable_header_done = true;
    }

    #[allow(clippy::too_many_arguments)]
    fn end_formal_parameter(
        &mut self,
        _var_or_final: Option<Token>,
        _this_keyword: Option<Token>,
        _super_keyword: Option<Token>,
        _period_after_this_or_super: Option<Token>,
        name: Token,
        _initializer_start: Option<Token>,
        _initializer_end: Option<Token>,
        kind: FormalParameterKind,
        _member_kind: ParserMemberKind,
    ) {
        if self.builder.borrow().formal_depth != 1 {
            return;
        }
        let mut builder = self.builder.borrow_mut();
        let Some((name, range)) = builder.capture_name(name) else {
            return;
        };
        let ty = builder.take_armed();
        let required = matches!(
            kind,
            FormalParameterKind::RequiredPositional | FormalParameterKind::RequiredNamed
        );
        let formal = Formal { name, ty, required };
        if matches!(
            kind,
            FormalParameterKind::RequiredNamed | FormalParameterKind::OptionalNamed
        ) {
            // Duplicate named formals are `duplicate_definition`s
            // (`checkParameters`, duplicate_definition_verifier.dart:71-90).
            if builder.named_formals.iter().any(|f| f.name == name) {
                builder.record_duplicate(name, range);
            } else {
                builder.named_formals.push(formal);
            }
        } else {
            builder.positional_formals.push(formal);
        }
        // An extension type's primary-constructor formals double as the
        // representation-field facts (`error_verifier.dart:5340-5460`
        // reads `formalParameters.first`); the header window keeps body
        // member formals out.
        if builder.header_capture_open
            && builder
                .containers
                .last()
                .is_some_and(|open| open.kind == ContainerKind::ExtensionType)
        {
            let open = builder.containers.last_mut().expect("checked above");
            open.primary_formals.push(RepFormal {
                formal: Formal { name, ty, required },
                name_range: range,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_scanner::ScanConfig;
    use dartq_source::FileTable;
    use std::path::Path;

    fn build(source: &str, strings: &StringTable) -> FileElements {
        let mut files = FileTable::new();
        let file = files.intern(Path::new("test.dart"));
        let result = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(
            result.diagnostics.is_empty(),
            "scanner diagnostics: {:?}",
            result.diagnostics
        );
        let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
            LanguageVersion::new(3, 15),
        ));
        collect(source, &result.tokens, features, strings)
    }

    fn id(strings: &StringTable, name: &str) -> StringId {
        strings.intern(name)
    }

    /// Declaration capture across the full top-level surface.
    #[test]
    fn captures_top_level_declarations() {
        let strings = StringTable::new();
        let elements = build(
            "class C<T> {} \
             enum E { a, b } \
             mixin M {} \
             extension Ext on int {} \
             extension on String {} \
             typedef Alias = void Function(); \
             typedef OldFn(int x); \
             void fnTop(int a) {} \
             int get getterTop => 1; \
             set setterTop(int v) {} \
             final varTop = 0;",
            &strings,
        );
        let kind = |name: &str| {
            elements
                .decl(id(&strings, name))
                .map(|decl| decl.kind)
                .expect(name)
        };
        assert_eq!(kind("C"), MicroKind::Class);
        assert_eq!(kind("E"), MicroKind::Enum);
        assert_eq!(kind("M"), MicroKind::Mixin);
        assert_eq!(kind("Ext"), MicroKind::Extension);
        assert_eq!(kind("Alias"), MicroKind::TypeAlias);
        assert_eq!(kind("OldFn"), MicroKind::TypeAlias);
        assert_eq!(kind("fnTop"), MicroKind::TopLevelFn);
        assert_eq!(kind("getterTop"), MicroKind::TopLevelFn);
        // Setters live under `name=`.
        assert!(elements.decl(id(&strings, "setterTop")).is_none());
        assert_eq!(kind("setterTop="), MicroKind::TopLevelFn);
        assert_eq!(kind("varTop"), MicroKind::TopLevelVar);
        // Type parameters land in the name set tagged typeParam.
        assert_eq!(kind("T"), MicroKind::TypeParam);
    }

    /// Member sets: method/getter/setter/field with the static/instance bit,
    /// enum constants as static fields.
    #[test]
    fn captures_member_sets_with_static_bit() {
        let strings = StringTable::new();
        let elements = build(
            "class C { \
               void m() {} \
               static void sm() {} \
               int get g => 1; \
               static int get sg => 1; \
               set s(int v) {} \
               static set ss(int v) {} \
               int a = 0, b = 1; \
               static int sa = 2; \
             } \
             enum E { x, y; void m() {} }",
            &strings,
        );
        let member = |class_name: &str, name: &str| {
            elements
                .container(id(&strings, class_name))
                .expect(class_name)
                .members
                .get(&id(&strings, name))
                .unwrap_or_else(|| panic!("member {class_name}.{name}"))
        };
        assert_eq!(member("C", "m").kind, MemberKind::Method);
        assert!(!member("C", "m").is_static);
        assert_eq!(member("C", "sm").kind, MemberKind::Method);
        assert!(member("C", "sm").is_static);
        assert_eq!(member("C", "g").kind, MemberKind::Getter);
        assert_eq!(member("C", "sg").kind, MemberKind::Getter);
        assert!(member("C", "sg").is_static);
        // Setters under `name=`, tagged Setter.
        assert!(
            !elements
                .container(id(&strings, "C"))
                .unwrap()
                .members
                .contains_key(&id(&strings, "s"))
        );
        assert_eq!(member("C", "s=").kind, MemberKind::Setter);
        assert_eq!(member("C", "ss=").kind, MemberKind::Setter);
        assert!(member("C", "ss=").is_static);
        // Group fields `int a, b` both captured with the group's bit and the
        // group's declared type.
        assert_eq!(member("C", "a").kind, MemberKind::Field);
        assert!(!member("C", "a").is_static);
        assert_eq!(member("C", "b").kind, MemberKind::Field);
        assert_eq!(member("C", "a").var_ty, Ty::named(id(&strings, "int")));
        assert_eq!(member("C", "b").var_ty, Ty::named(id(&strings, "int")));
        assert!(member("C", "sa").is_static);
        // Enum constants are static fields; real members are instance ones.
        assert_eq!(member("E", "x").kind, MemberKind::Field);
        assert!(member("E", "x").is_static);
        assert!(!member("E", "m").is_static);
    }

    /// Superclass edges: extends (qualified keeps the bare name) and the
    /// named mixin application's head type; no edge without extends.
    #[test]
    fn captures_superclass_edges() {
        let strings = StringTable::new();
        let elements = build(
            "class B {} \
             class C extends B {} \
             class Q extends p.S {} \
             class D = B with M; \
             class N {}",
            &strings,
        );
        let superclass = |name: &str| {
            elements
                .container(id(&strings, name))
                .expect(name)
                .superclass
        };
        assert_eq!(superclass("C"), Some(id(&strings, "B")));
        assert_eq!(
            superclass("Q"),
            Some(id(&strings, "S")),
            "qualified keeps S"
        );
        assert_eq!(superclass("D"), Some(id(&strings, "B")), "head type");
        assert_eq!(superclass("N"), None);
    }

    /// Formal tables: positional (required-first, with types) and named;
    /// nested function-typed formals do not pollute the outer table;
    /// getters/setters carry their declared shapes.
    ///
    /// RED (parser gap, #339): a function-typed formal with a committed
    /// return type (`void cb(int inner)?`) never enters the inline
    /// function-type path — `parse_formal_parameter` detects the bare-`(`
    /// follower only when `type_info` is `NoType`
    /// (`formal_params.rs`, `before_inline_function_type` gate) — so `cb`
    /// parses as a plain formal, the orphaned `(` trips the named-group
    /// recovery ("Expected '}'"), and every following formal (`outer`)
    /// is dropped: `g`'s named table captures `["cb"]` only. Event trace:
    /// `begin_named_group | end_formal(cb) | REPORT Expected '}' |
    /// end_named_group count=1`. Un-reds when the parser detects the
    /// type+name+`(` shape (official parser_impl.dart:2221-2264).
    #[test]
    fn captures_formals() {
        let strings = StringTable::new();
        let elements = build(
            "void f(int a, {int? b, required int c}) {} \
             void g({void cb(int inner)?, int outer = 1}) {} \
             class C { \
               void m(int x, {required int y}) {} \
               int get gg => 1; \
             } \
             void plain(int a) {}",
            &strings,
        );
        let formals = |name: &str| {
            elements
                .decl(id(&strings, name))
                .expect(name)
                .formals
                .as_deref()
                .map(|f| {
                    (
                        f.positional.iter().map(|p| p.name).collect::<Vec<_>>(),
                        f.named.iter().map(|p| p.name).collect::<Vec<_>>(),
                    )
                })
        };
        let (pos, named) = formals("f").expect("f formals");
        assert_eq!(pos, vec![id(&strings, "a")]);
        assert_eq!(named, vec![id(&strings, "b"), id(&strings, "c")]);
        let (pos, named) = formals("g").expect("g formals");
        assert!(pos.is_empty(), "nested depth-2 names are excluded");
        assert_eq!(named, vec![id(&strings, "cb"), id(&strings, "outer")]);
        let (pos, _) = formals("plain").expect("plain formals");
        assert_eq!(pos, vec![id(&strings, "a")]);
        let m = elements
            .container(id(&strings, "C"))
            .unwrap()
            .members
            .get(&id(&strings, "m"))
            .unwrap();
        let f = m.formals.as_deref().unwrap();
        assert_eq!(f.named[0].name, id(&strings, "y"));
        assert!(f.named[0].required);
        // Formal declared types: `int a` non-nullable, `int? b` nullable.
        let f = elements
            .decl(id(&strings, "f"))
            .unwrap()
            .formals
            .as_deref()
            .unwrap();
        assert_eq!(f.positional[0].ty, Ty::named(id(&strings, "int")));
        assert_eq!(
            f.named[0].ty,
            Ty::Named {
                name: id(&strings, "int"),
                nullable: true,
            }
        );
        let gg = elements
            .container(id(&strings, "C"))
            .unwrap()
            .members
            .get(&id(&strings, "gg"))
            .unwrap();
        assert!(gg.formals.is_none());
    }

    /// Type-parameter counts per declaration, from the depth-1 group only:
    /// containers, generic methods, generic functions, typedefs; body
    /// declarations never clobber a header count.
    #[test]
    fn captures_type_param_counts() {
        let strings = StringTable::new();
        let elements = build(
            "class C<T, U> { void m<V>() {} } \
             enum E<X> { a; } \
             void fnTop<Y>() {} \
             typedef Alias<Z> = void Function(); \
             void outer<W>() { int inner<Q>() => 1; }",
            &strings,
        );
        assert_eq!(
            elements.container(id(&strings, "C")).unwrap().type_params,
            2
        );
        let m = elements
            .container(id(&strings, "C"))
            .unwrap()
            .members
            .get(&id(&strings, "m"))
            .unwrap();
        assert_eq!(m.type_params, 1);
        assert_eq!(
            elements.container(id(&strings, "E")).unwrap().type_params,
            1
        );
        assert_eq!(elements.decl(id(&strings, "fnTop")).unwrap().type_params, 1);
        assert_eq!(elements.decl(id(&strings, "Alias")).unwrap().type_params, 1);
        // The local function's group must not clobber the outer count.
        assert_eq!(elements.decl(id(&strings, "outer")).unwrap().type_params, 1);
    }

    /// Declared types: top-level variables, return types (functions,
    /// getters), and the nullable flag.
    #[test]
    fn captures_declared_types() {
        let strings = StringTable::new();
        let elements = build(
            "int top = 0; \
             int? topNull; \
             C make() => C(); \
             C? makeNull() => null; \
             void nothing() {} \
             int get gTop => 1; \
             class C { int? get gNull => null; }",
            &strings,
        );
        let decl = |name: &str| elements.decl(id(&strings, name)).unwrap();
        assert_eq!(decl("top").var_ty, Ty::named(id(&strings, "int")));
        assert_eq!(
            decl("topNull").var_ty,
            Ty::Named {
                name: id(&strings, "int"),
                nullable: true,
            }
        );
        assert_eq!(decl("make").return_ty, Ty::named(id(&strings, "C")));
        assert_eq!(
            decl("makeNull").return_ty,
            Ty::Named {
                name: id(&strings, "C"),
                nullable: true,
            }
        );
        assert_eq!(decl("nothing").return_ty, Ty::Void);
        assert_eq!(decl("gTop").return_ty, Ty::named(id(&strings, "int")));
        let g = elements
            .container(id(&strings, "C"))
            .unwrap()
            .members
            .get(&id(&strings, "gNull"))
            .unwrap();
        assert_eq!(
            g.return_ty,
            Ty::Named {
                name: id(&strings, "int"),
                nullable: true,
            }
        );
    }

    /// Declaration-arity violations: operator parameter counts (including
    /// the minus special case) and the setter shape rule.
    #[test]
    fn captures_arity_violations() {
        let strings = StringTable::new();
        let elements = build(
            "class C { \
               int operator +(int o, int p) => 1; \
               int operator ~(int o) => 1; \
               int operator [](int i, int j) => 1; \
               C operator -(int o, int p) => C(); \
               set bad(int a, int b) {} \
               set namedOnly({int a = 1}) {} \
               set ok(int v) {} \
               int operator [](int i) => 1; \
             } \
             set topBad([int a = 1]) {}",
            &strings,
        );
        let vs: Vec<ArityViolation> = elements
            .arity_violations()
            .iter()
            .map(|a| a.violation)
            .collect();
        assert_eq!(vs.len(), 7, "{vs:?}");
        assert!(vs.contains(&ArityViolation::Operator {
            name: id(&strings, "+"),
            expected: 1,
            found: 2,
        }));
        assert!(vs.contains(&ArityViolation::Operator {
            name: id(&strings, "~"),
            expected: 0,
            found: 1,
        }));
        assert!(vs.contains(&ArityViolation::Operator {
            name: id(&strings, "[]"),
            expected: 1,
            found: 2,
        }));
        assert!(vs.contains(&ArityViolation::OperatorMinus { found: 2 }));
        assert_eq!(
            vs.iter().filter(|v| **v == ArityViolation::Setter).count(),
            3
        );
    }

    /// Unnamed extensions keep no top-level name; named and unnamed
    /// extensions both produce one application face each, carrying the `on`
    /// type (the applicability anchor) and the members under their lookup
    /// spellings, static bit preserved.
    #[test]
    fn captures_extension_application_faces() {
        let strings = StringTable::new();
        let elements = build(
            "extension Named on int { void nm() {} static int ns = 0; } \
             extension on String { int get ue => 1; }",
            &strings,
        );
        assert_eq!(
            elements.decl(id(&strings, "Named")).map(|d| d.kind),
            Some(MicroKind::Extension)
        );
        let faces = elements.extension_faces();
        assert_eq!(faces.len(), 2, "{faces:?}");
        let named = faces
            .iter()
            .find(|f| f.name == Some(id(&strings, "Named")))
            .expect("named face");
        assert_eq!(named.on_type, Some(id(&strings, "int")));
        fn face_member<'a>(
            face: &'a crate::model::ExtensionFace,
            name: &str,
            strings: &StringTable,
        ) -> Option<&'a crate::model::MemberEntry> {
            face.members.get(&id(strings, name))
        }
        assert_eq!(
            face_member(named, "nm", &strings).map(|m| m.kind),
            Some(MemberKind::Method)
        );
        assert_eq!(
            face_member(named, "ns", &strings).map(|m| m.kind),
            Some(MemberKind::Field)
        );
        assert!(face_member(named, "ns", &strings).unwrap().is_static);
        let unnamed = faces
            .iter()
            .find(|f| f.name.is_none())
            .expect("unnamed face");
        assert_eq!(unnamed.on_type, Some(id(&strings, "String")));
        assert_eq!(
            face_member(unnamed, "ue", &strings).map(|m| m.kind),
            Some(MemberKind::Getter)
        );
    }

    /// The `extends` overflow type (`extends A, B`) is residue
    /// (`ast_builder.dart:3972-3982` throws the extras away), never an
    /// `implements` entry — #491 S1, the audit #489 pin shape.
    #[test]
    fn extends_overflow_lands_in_residue() {
        let strings = StringTable::new();
        let elements = build("class A {} class B {} class C extends A, B {}", &strings);
        let c = elements.container(id(&strings, "C")).expect("C");
        assert_eq!(c.superclass, Some(id(&strings, "A")));
        assert!(c.implements_clause.is_empty(), "{:?}", c.implements_clause);
        assert_eq!(c.header_residue, vec![id(&strings, "B")]);
    }

    /// Clause names after their keyword attribute to their clause; the
    /// overflow before it stays residue. A real `implements` clause still
    /// captures when an overflow precedes it.
    #[test]
    fn clause_events_claim_only_their_own_names() {
        let strings = StringTable::new();
        let elements = build(
            "class A {} class B {} class C {} class D {} \
             class E extends A, B with C implements D {}",
            &strings,
        );
        let e = elements.container(id(&strings, "E")).expect("E");
        assert_eq!(e.superclass, Some(id(&strings, "A")));
        assert_eq!(e.with_clause, vec![id(&strings, "C")]);
        assert_eq!(e.implements_clause, vec![id(&strings, "D")]);
        assert_eq!(e.header_residue, vec![id(&strings, "B")]);
    }

    /// An extension's `on` type latches through the continuation for
    /// qualified references (`on p.T` keeps `T`).
    #[test]
    fn extension_on_type_prefers_the_continuation() {
        let strings = StringTable::new();
        let elements = build("class T {} extension E on p.T { void m() {} }", &strings);
        let faces = elements.extension_faces();
        assert_eq!(faces.len(), 1);
        assert_eq!(faces[0].on_type, Some(id(&strings, "T")));
    }

    /// The extension type's primary-constructor formals are the
    /// representation-field capture; body members stay out.
    #[test]
    fn extension_type_captures_representation_formals() {
        let strings = StringTable::new();
        let elements = build("extension type E(int x) { void m(String y) {} }", &strings);
        let e = elements.container(id(&strings, "E")).expect("E");
        assert_eq!(e.primary_formals.len(), 1, "{:?}", e.primary_formals);
        let rep = &e.primary_formals[0];
        assert_eq!(rep.formal.name, id(&strings, "x"));
        assert_eq!(rep.formal.ty, Ty::named(id(&strings, "int")));
        assert!(e.members.get(&id(&strings, "m")).is_some());
        assert!(e.members.get(&id(&strings, "x")).is_none());
    }

    /// Constructor formals join the container facts under their lookup
    /// spellings — the selector for a named constructor, the container's
    /// own name for the unnamed face (the spellings creation invocations
    /// mark as given: the folded `Created` head row keeps the class name,
    /// a `C.named()` send keeps the member row `named`). One optional
    /// section per list (the grammar allows `[...]` or `{...}`, never
    /// both), so the named-group shape rides a third constructor.
    #[test]
    fn constructor_formals_carry_both_lookup_spellings() {
        let strings = StringTable::new();
        let elements = build(
            "class C { C.named(int w, [int x = 0]) {} C([int y = 1]) {} \
             C.other({int z = 2}) {} }",
            &strings,
        );
        let container = elements.container(id(&strings, "C")).expect("C");
        let diagnostics = |container: &crate::model::Container| {
            container
                .ctor_formals
                .keys()
                .map(|k| strings.resolve(*k).to_owned())
                .collect::<Vec<_>>()
        };
        let named = container
            .ctor_formals
            .get(&id(&strings, "named"))
            .unwrap_or_else(|| panic!("named selector key: {:?}", diagnostics(container)));
        assert_eq!(named.positional.len(), 2);
        assert!(named.positional[0].required);
        assert_eq!(named.positional[0].name, id(&strings, "w"));
        assert!(!named.positional[1].required);
        assert_eq!(named.positional[1].name, id(&strings, "x"));
        assert_eq!(named.positional[1].ty, Ty::named(id(&strings, "int")));
        let unnamed = container
            .ctor_formals
            .get(&id(&strings, "C"))
            .unwrap_or_else(|| panic!("class-name key: {:?}", diagnostics(container)));
        assert_eq!(unnamed.positional.len(), 1);
        assert!(!unnamed.positional[0].required);
        assert_eq!(unnamed.positional[0].name, id(&strings, "y"));
        let other = container
            .ctor_formals
            .get(&id(&strings, "other"))
            .unwrap_or_else(|| panic!("other selector key: {:?}", diagnostics(container)));
        assert_eq!(other.positional.len(), 0);
        assert_eq!(other.named.len(), 1);
        assert!(!other.named[0].required);
        assert_eq!(other.named[0].name, id(&strings, "z"));
    }
}
