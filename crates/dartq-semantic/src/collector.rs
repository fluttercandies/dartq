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
//!   capturing (no top-level name); their members surface only in the
//!   extension pool.
//! * Superclass edges: `handle_class_extends(Some(_))` and
//!   `begin_named_mixin_application` arm a one-shot capture; the armed
//!   `TypeReference`/`TypeReferenceContinuation` identifier names the
//!   superclass (a qualified `p.S` keeps `S`, the continuation), committed
//!   at `handle_qualified`/`handle_type` — the first type after `extends`
//!   or the head type of a named mixin application. `on`/`with`/`implements`
//!   faces are not captured (adjudicated cut).
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
    ArityDecl, ArityViolation, Container, ContainerKind, DuplicateDecl, FileElements, Formal,
    Formals, MemberEntry, MemberKind, MicroKind, TopDecl, Ty,
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
    members: FxHashMap<StringId, MemberEntry>,
    type_params: u16,
    /// The `sealed` modifier was present on the class header.
    sealed: bool,
    /// The header's `with`/`implements` type names, in written order.
    interfaces: Vec<StringId>,
    /// The enum's constants in declaration order.
    enum_constants: Vec<StringId>,
    /// The named constructor selectors declared so far, and whether an
    /// unnamed constructor exists (explicit or the implicit default).
    ctor_named: Vec<StringId>,
    ctor_has_unnamed: bool,
}

/// The open constructor's capture: the named selectors it declares plus
/// its unnamed flag (`A()` keeps the flag; `A.named()` records the selector
/// and clears it).
struct CtorCapture {
    named: Vec<StringId>,
    has_unnamed: bool,
}

/// A class member executable between `begin_method` and `end_method`.
struct PendingMember {
    /// Lookup spelling, kind, and the name token's span; `None` when the name
    /// is unrecoverable.
    shape: Option<(StringId, MemberKind, SourceRange)>,
    is_static: bool,
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
    /// The buffered `with`/`implements` type names of the open header.
    header_interfaces: Vec<StringId>,
    /// The current field group's static bit (`None` = no group open).
    field_static: Option<bool>,
    pending_member: Option<PendingMember>,
    pending_top: Option<PendingTop>,
    pending_typedef: Option<PendingTypedef>,
    /// A constructor is open (`begin_constructor`..`end_constructor`): the
    /// named selectors it declares plus its unnamed flag. `A()` records the
    /// unnamed shape; `A.named()` records `named` only.
    ctor_open: Option<CtorCapture>,
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
            field_static: None,
            pending_member: None,
            pending_top: None,
            pending_typedef: None,
            ctor_open: None,
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
        // Only class-family headers carry `with`/`implements` lists; the
        // window ends at `handle_class_header` (the `{` or `;`).
        self.in_class_header = matches!(kind, ContainerKind::Class);
        self.header_interfaces.clear();
        self.containers.push(OpenContainer {
            name,
            kind,
            superclass: None,
            members: FxHashMap::default(),
            type_params: header_type_params,
            sealed: false,
            interfaces: Vec::new(),
            enum_constants: Vec::new(),
            ctor_named: Vec::new(),
            ctor_has_unnamed: true,
        });
        self.header_super_open = true;
        self.super_candidate = None;
    }

    fn close_container(&mut self) {
        self.header_super_open = false;
        self.super_candidate = None;
        self.in_class_header = false;
        self.header_interfaces.clear();
        let Some(open) = self.containers.pop() else {
            return;
        };
        let members = open.members;
        if matches!(
            open.kind,
            ContainerKind::Extension | ContainerKind::ExtensionType
        ) {
            // Conservative suppression: every extension member (both static
            // bits) joins the instance pool, so implicit extension
            // application can only under-report, never false-positive.
            for (name, entry) in &members {
                self.file
                    .extension_members
                    .entry(*name)
                    .or_insert_with(|| entry.clone());
            }
        }
        if let Some((name, _range)) = open.name {
            self.file.container_order.push(name);
            self.file.containers.entry(name).or_insert(Container {
                kind: open.kind,
                superclass: open.superclass,
                members,
                type_params: open.type_params,
                sealed: open.sealed,
                interfaces: open.interfaces,
                enum_constants: open.enum_constants,
                constructors: open.ctor_named.into_boxed_slice(),
                has_unnamed_constructor: open.ctor_has_unnamed,
            });
        } else {
            for (name, entry) in members {
                self.file.extension_members.entry(name).or_insert(entry);
            }
        }
    }

    /// Marks the open container `sealed` (the `sealed` modifier event).
    fn mark_sealed(&mut self) {
        if let Some(open) = self.containers.last_mut() {
            open.sealed = true;
        }
    }

    /// Ends the class-header window (`handle_class_header`): the buffered
    /// `with`/`implements` names join the open container.
    fn end_class_header(&mut self) {
        self.in_class_header = false;
        let buffered = std::mem::take(&mut self.header_interfaces);
        if let Some(open) = self.containers.last_mut() {
            open.interfaces.extend(buffered);
        }
    }

    /// Buffers one header `with`/`implements` type name (declarations.rs
    /// :1129-1172): subclass-edge sources toward sealed supertypes.
    fn buffer_header_interface(&mut self, name: Option<StringId>) {
        if let Some(name) = name {
            self.header_interfaces.push(name);
        }
    }

    /// Routes a class-header type reference: the `extends` head latches the
    /// superclass candidate (a qualified `p.S` through the continuation),
    /// and past it the `with`/`implements` names buffer toward the
    /// container's interface edges.
    fn note_header_reference(&mut self, context: IdentifierContext, name: Option<StringId>) {
        match context {
            IdentifierContext::TypeReference => {
                if self.header_super_open && self.super_candidate.is_none() {
                    self.super_candidate = name;
                } else if self.in_class_header {
                    self.buffer_header_interface(name);
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
        _abstract_kw: Option<Token>,
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
    }

    fn handle_class_header(&mut self, _begin: Token, _class: Token, _native: Option<Token>) {
        self.builder.borrow_mut().end_class_header();
    }

    fn end_class_declaration(&mut self, _begin: Token, _end: Token) {
        self.builder.borrow_mut().close_container();
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
        let builder = &mut *self.builder.borrow_mut();
        let name = builder.capture_name(name);
        builder.open_container(ContainerKind::Class, name, MicroKind::Class);
    }

    fn end_named_mixin_application(
        &mut self,
        _begin: Token,
        _class: Token,
        _equals: Token,
        _implements: Option<Token>,
        _end: Token,
    ) {
        self.builder.borrow_mut().close_container();
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

    fn handle_named_mixin_application_with_clause(&mut self, _with: Token) {
        // The head type fired between `begin_named_mixin_application` and
        // this event; the `with` list itself is not the superclass.
        self.builder.borrow_mut().commit_super_candidate();
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
                builder.note_header_reference(context, name);
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
            type_params: 0,
            type_param_names: Vec::new(),
            return_ty: Ty::UNKNOWN,
            formals: None,
        });
        builder.executable_header_done = false;
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
        // `A.named` continuation says otherwise.
        builder.ctor_open = Some(CtorCapture {
            named: Vec::new(),
            has_unnamed: true,
        });
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
        if matches!(
            kind,
            FormalParameterKind::RequiredNamed | FormalParameterKind::OptionalNamed
        ) {
            // Duplicate named formals are `duplicate_definition`s
            // (`checkParameters`, duplicate_definition_verifier.dart:71-90).
            if builder.named_formals.iter().any(|f| f.name == name) {
                builder.record_duplicate(name, range);
            } else {
                builder.named_formals.push(Formal {
                    name,
                    ty,
                    required: kind == FormalParameterKind::RequiredNamed,
                });
            }
        } else {
            builder.positional_formals.push(Formal {
                name,
                ty,
                required: kind == FormalParameterKind::RequiredPositional,
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

    /// Unnamed extensions keep no top-level name; their members and named
    /// extensions' members both surface in the instance pool.
    #[test]
    fn merges_extension_members_into_pool() {
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
        let pool_member = |name: &str| elements.extension_member(id(&strings, name));
        assert_eq!(pool_member("nm").map(|m| m.kind), Some(MemberKind::Method));
        assert_eq!(pool_member("ns").map(|m| m.kind), Some(MemberKind::Field));
        assert!(pool_member("ns").unwrap().is_static);
        assert_eq!(pool_member("ue").map(|m| m.kind), Some(MemberKind::Getter));
    }
}
