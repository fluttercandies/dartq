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
//!   that bit. Enum constants (`EnumValueDeclaration`) commit as static
//!   fields of the enum.
//! * Executables: `begin_method` carries name/static/get-or-set directly;
//!   a top-level method's name arrives via the
//!   `TopLevelFunctionDeclaration` identifier event and its get-or-set only
//!   at `end_top_level_method`. Named formals are collected while
//!   `end_formal_parameter` reports a named kind at formal-nesting depth 1
//!   and attached to the executable once (the attach-once guard keeps
//!   closures in bodies or default values from clobbering it).
//! * Typedef names arrive via the `TypedefDeclaration` identifier event
//!   between `begin_typedef`/`end_typedef`.
//! * `begin_type_variable` names land in the unit's name set tagged
//!   `typeParam`.
//!
//! Recovery-synthetic identifiers (parser-inserted token ids past the token
//! table) carry no honest text; their names are skipped. Setter names are
//! rewritten to `name=` at capture (one interning per setter — capture-time
//! cost, not lookup-time).

use std::cell::RefCell;
use std::rc::Rc;

use dartiq_parser::{
    DeclarationKind, FormalParameterKind, IdentifierContext, Parser, ParserFeatures, ParserListener,
};
use dartiq_parser::{MemberKind as ParserMemberKind, TokenSpan as Token};
use dartiq_scanner::TokenTable;
use dartiq_source::{StringId, StringTable};
use rustc_hash::FxHashMap;

use crate::model::{
    Container, ContainerKind, DuplicateDecl, FileElements, MemberEntry, MemberKind, MicroKind,
    TopDecl,
};

use dartiq_source::SourceRange;

/// An open container on the capture stack.
struct OpenContainer {
    /// Lookup spelling with the name token's span (the duplicate log's
    /// location surface; the spelling is what the model keeps).
    name: Option<(StringId, SourceRange)>,
    kind: ContainerKind,
    superclass: Option<StringId>,
    members: FxHashMap<StringId, MemberEntry>,
}

/// A class member executable between `begin_method` and `end_method`.
struct PendingMember {
    /// Lookup spelling, kind, and the name token's span; `None` when the name
    /// is unrecoverable.
    shape: Option<(StringId, MemberKind, SourceRange)>,
    is_static: bool,
    formals: Option<Box<[StringId]>>,
}

/// A top-level executable between `begin_top_level_method` and its end; the
/// name arrives from the identifier event, get-or-set from the end event.
struct PendingTop {
    name: Option<(StringId, SourceRange)>,
    formals: Option<Box<[StringId]>>,
}

/// The capture state shared between the listener and [`collect`].
struct Builder<'src, 'tab> {
    source: &'src str,
    tokens: &'tab TokenTable,
    strings: &'src StringTable,
    file: FileElements,
    containers: Vec<OpenContainer>,
    /// The current field group's static bit (`None` = no group open).
    field_static: Option<bool>,
    super_armed: bool,
    super_pending: Option<StringId>,
    pending_member: Option<PendingMember>,
    pending_top: Option<PendingTop>,
    pending_typedef: Option<(StringId, SourceRange)>,
    /// Formal-nesting depth; the executable's own group is depth 1.
    formal_depth: u32,
    named_formals: Vec<StringId>,
    /// The current type-parameter list's names (`duplicate_definition`
    /// compares per list, `duplicate_definition_verifier.dart:124-135`).
    type_var_group: Vec<StringId>,
}

impl<'src, 'tab> Builder<'src, 'tab> {
    fn new(source: &'src str, tokens: &'tab TokenTable, strings: &'src StringTable) -> Self {
        Self {
            source,
            tokens,
            strings,
            file: FileElements::default(),
            containers: Vec::new(),
            field_static: None,
            super_armed: false,
            super_pending: None,
            pending_member: None,
            pending_top: None,
            pending_typedef: None,
            formal_depth: 0,
            named_formals: Vec::new(),
            type_var_group: Vec::new(),
        }
    }

    /// The token's name and span, `None` for parser-inserted synthetic tokens
    /// (which are not in the table and carry no honest name).
    fn capture_name(&self, token: Token) -> Option<(StringId, SourceRange)> {
        let range = token.range;
        self.name_of(token).map(|name| (name, range))
    }

    /// Records a duplicate (second) declaration; the name maps keep the
    /// first entry, this log is the `duplicate_definition` surface.
    fn record_duplicate(&mut self, name: StringId, range: SourceRange) {
        self.file.duplicates.push(DuplicateDecl { name, range });
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

    fn open_container(
        &mut self,
        kind: ContainerKind,
        name: Option<(StringId, SourceRange)>,
        tag: MicroKind,
    ) {
        if let Some((name, range)) = name {
            self.declare_top(
                name,
                range,
                TopDecl {
                    kind: tag,
                    named_formals: None,
                },
            );
        }
        self.containers.push(OpenContainer {
            name,
            kind,
            superclass: None,
            members: FxHashMap::default(),
        });
        self.super_armed = false;
        self.super_pending = None;
    }

    fn close_container(&mut self) {
        self.super_armed = false;
        self.super_pending = None;
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
            self.file.containers.entry(name).or_insert(Container {
                kind: open.kind,
                superclass: open.superclass,
                members,
            });
        } else {
            for (name, entry) in members {
                self.file.extension_members.entry(name).or_insert(entry);
            }
        }
    }

    fn commit_superclass(&mut self) {
        if !self.super_armed {
            return;
        }
        self.super_armed = false;
        if let Some(open) = self.containers.last_mut()
            && open.superclass.is_none()
        {
            open.superclass = self.super_pending.take();
        } else {
            self.super_pending = None;
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

    /// Attaches the depth-0 formal group to the pending executable, once.
    fn attach_formals(&mut self) {
        let formals = (!self.named_formals.is_empty())
            .then(|| std::mem::take(&mut self.named_formals).into_boxed_slice());
        if let Some(pending) = &mut self.pending_member {
            if pending.formals.is_none() {
                pending.formals = formals;
            }
        } else if let Some(pending) = &mut self.pending_top
            && pending.formals.is_none()
        {
            pending.formals = formals;
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
        let builder = &mut *self.builder.borrow_mut();
        let name = builder.capture_name(name);
        builder.open_container(ContainerKind::Class, name, MicroKind::Class);
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
        // The head type (`class C = A with B;`) is the superclass edge.
        builder.super_armed = true;
        builder.super_pending = None;
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

    fn handle_class_extends(&mut self, extends: Option<Token>, _count: usize) {
        if extends.is_some() {
            let mut builder = self.builder.borrow_mut();
            builder.super_armed = true;
            builder.super_pending = None;
        }
    }

    fn handle_qualified(&mut self, _period: Token) {
        self.builder.borrow_mut().commit_superclass();
    }

    fn handle_type(&mut self, _name: Token, _question: Option<Token>) {
        self.builder.borrow_mut().commit_superclass();
    }

    fn handle_identifier(&mut self, token: Token, context: IdentifierContext) {
        let mut builder = self.builder.borrow_mut();
        // Resolve the name and its span up front: the branches below hold
        // borrows into the builder's state.
        let name_with_range = builder.capture_name(token);
        let name = name_with_range.map(|(name, _)| name);
        match context {
            IdentifierContext::TypeReference => {
                if builder.super_armed && builder.super_pending.is_none() {
                    builder.super_pending = name;
                }
            }
            IdentifierContext::TypeReferenceContinuation => {
                if builder.super_armed {
                    // Qualified `p.S`: the continuation names the type.
                    builder.super_pending = name;
                }
            }
            IdentifierContext::TopLevelFunctionDeclaration => {
                if let Some(pending) = &mut builder.pending_top {
                    pending.name = name_with_range;
                }
            }
            IdentifierContext::TopLevelVariableDeclaration => {
                if let Some((name, range)) = name_with_range {
                    builder.declare_top(
                        name,
                        range,
                        TopDecl {
                            kind: MicroKind::TopLevelVar,
                            named_formals: None,
                        },
                    );
                }
            }
            IdentifierContext::FieldDeclaration => {
                if let Some((name, range)) = name_with_range {
                    let is_static = builder.field_static.unwrap_or(false);
                    if let Some(open) = builder.containers.last_mut() {
                        match open.members.entry(name) {
                            std::collections::hash_map::Entry::Vacant(slot) => {
                                slot.insert(MemberEntry {
                                    kind: MemberKind::Field,
                                    is_static,
                                    named_formals: None,
                                });
                            }
                            // Same-bit collision: the second field is a
                            // duplicate. Keep-first; a cross-bit coexistence
                            // is `conflicting_static_and_instance`'s
                            // business, not a duplicate.
                            std::collections::hash_map::Entry::Occupied(existing)
                                if existing.get().is_static == is_static =>
                            {
                                builder.record_duplicate(name, range);
                            }
                            _ => {}
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
                    match open.members.entry(name) {
                        std::collections::hash_map::Entry::Vacant(slot) => {
                            slot.insert(MemberEntry {
                                kind: MemberKind::Field,
                                is_static: true,
                                named_formals: None,
                            });
                        }
                        std::collections::hash_map::Entry::Occupied(existing)
                            if existing.get().is_static =>
                        {
                            builder.record_duplicate(name, range);
                        }
                        _ => {}
                    }
                }
            }
            IdentifierContext::TypedefDeclaration if builder.pending_typedef.is_some() => {
                builder.pending_typedef = name_with_range.or(builder.pending_typedef);
            }
            _ => {}
        }
    }

    fn begin_type_variables(&mut self, _token: Token) {
        self.builder.borrow_mut().type_var_group.clear();
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
            builder.file.decls.entry(name).or_insert(TopDecl {
                kind: MicroKind::TypeParam,
                named_formals: None,
            });
        }
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
        self.builder.borrow_mut().field_static = None;
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
        self.builder.borrow_mut().field_static = None;
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
            formals: None,
        });
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
            && let Some(open) = builder.containers.last_mut()
        {
            match open.members.entry(name) {
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(MemberEntry {
                        kind,
                        is_static: pending.is_static,
                        named_formals: pending.formals,
                    });
                }
                // Same-bit collisions are duplicates; cross-bit ones are not.
                std::collections::hash_map::Entry::Occupied(existing)
                    if existing.get().is_static == pending.is_static =>
                {
                    builder.record_duplicate(name, range);
                }
                _ => {}
            }
        }
    }

    fn begin_top_level_method(
        &mut self,
        _last_consumed: Token,
        _augment: Option<Token>,
        _external: Option<Token>,
    ) {
        self.builder.borrow_mut().pending_top = Some(PendingTop {
            name: None,
            formals: None,
        });
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
            builder.declare_top(
                spelling,
                range,
                TopDecl {
                    kind: MicroKind::TopLevelFn,
                    named_formals: pending.formals,
                },
            );
        }
    }

    fn begin_typedef(&mut self, _token: Token) {
        self.builder.borrow_mut().pending_typedef = None;
    }

    fn end_typedef(
        &mut self,
        _augment: Option<Token>,
        _typedef: Token,
        _equals: Option<Token>,
        _end: Token,
    ) {
        let mut builder = self.builder.borrow_mut();
        if let Some((name, range)) = builder.pending_typedef.take() {
            builder.declare_top(
                name,
                range,
                TopDecl {
                    kind: MicroKind::TypeAlias,
                    named_formals: None,
                },
            );
        }
    }

    fn begin_formal_parameters(&mut self, _token: Token, _kind: ParserMemberKind) {
        let mut builder = self.builder.borrow_mut();
        if builder.formal_depth == 0 {
            builder.named_formals.clear();
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
        if !matches!(
            kind,
            FormalParameterKind::RequiredNamed | FormalParameterKind::OptionalNamed
        ) {
            return;
        }
        let mut builder = self.builder.borrow_mut();
        if builder.formal_depth == 1
            && let Some((name, range)) = builder.capture_name(name)
        {
            // Duplicate named formals are `duplicate_definition`s
            // (`checkParameters`, duplicate_definition_verifier.dart:71-90).
            if builder.named_formals.contains(&name) {
                builder.record_duplicate(name, range);
            } else {
                builder.named_formals.push(name);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dartiq_language::{FeatureSet, LanguageVersion};
    use dartiq_scanner::ScanConfig;
    use dartiq_source::FileTable;
    use std::path::Path;

    fn build(source: &str, strings: &StringTable) -> FileElements {
        let mut files = FileTable::new();
        let file = files.intern(Path::new("test.dart"));
        let result = dartiq_scanner::scan(
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
        // Group fields `int a, b` both captured with the group's bit.
        assert_eq!(member("C", "a").kind, MemberKind::Field);
        assert!(!member("C", "a").is_static);
        assert_eq!(member("C", "b").kind, MemberKind::Field);
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

    /// Named formals: top-level functions and methods; nested function-typed
    /// formals do not pollute the outer table; getters/setters have none.
    #[test]
    fn captures_named_formals() {
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
                .named_formals
                .as_ref()
                .map(|names| names.to_vec())
                .unwrap_or_default()
        };
        assert_eq!(formals("f"), vec![id(&strings, "b"), id(&strings, "c")]);
        assert_eq!(
            formals("g"),
            vec![id(&strings, "cb"), id(&strings, "outer")],
            "nested depth-2 names are excluded"
        );
        assert!(formals("plain").is_empty());
        let m = elements
            .container(id(&strings, "C"))
            .unwrap()
            .members
            .get(&id(&strings, "m"))
            .unwrap();
        assert_eq!(
            m.named_formals.as_ref().unwrap().to_vec(),
            vec![id(&strings, "y")]
        );
        let gg = elements
            .container(id(&strings, "C"))
            .unwrap()
            .members
            .get(&id(&strings, "gg"))
            .unwrap();
        assert!(gg.named_formals.is_none());
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
