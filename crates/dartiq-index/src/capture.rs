//! The unlinked declaration capture (spec §8 keep list, first face): one
//! pass over the parser event stream records every declaration the parse
//! surface carries — with its name-token span and container anchor — and
//! every identifier use as a raw reference site, refined in place by the
//! parser's send/assignment/constructor-reference events.
//!
//! # Capture mechanics
//!
//! The `src/tests/collector.rs` collecting-listener precedent, applied with
//! spans (the dartiq-semantic collector records name sets; this one records
//! rows): parser events key the capture, the listener takes the builder
//! behind an `Rc<RefCell>` shared with [`capture`], and recovery-synthetic
//! tokens (not in the token table) contribute nothing.
//!
//! * Containers (`class`/`enum`/`mixin`/`extension`/extension type, plus
//!   named mixin applications) commit their own row at `begin_*` and push
//!   their row index; members commit with the stack top as their container
//!   anchor. Dart has no nested type declarations, so the chain is at most
//!   one hop. Unnamed extensions keep capturing: their members surface with
//!   no container anchor.
//! * Superclass name edges (`extends S`, a named mixin application's head
//!   type; `p.S` keeps `S`) are recorded as [`SuperEdge`]s against the
//!   open container and resolved at library-build time.
//! * Constructors, factories, and named formals are **not** declaration
//!   rows: the kind set has no constructor kind and the formals table is
//!   #119's (`undefined_named_parameter` consumes it there). Operators
//!   commit under whatever spelling `begin_method` hands over.
//! * Setters are stored under their `name=` lookup spelling (the #117
//!   export-scope convention); the row span stays the bare name token.
//!
//! # Reference sites
//!
//! `handle_identifier` reference contexts become raw sites
//! (type contexts → [`RefKind::Referenced`], expression contexts →
//! [`RefKind::Read`]), then three parser-marked refinements upgrade kinds
//! in place, all sound (parser-decided, no lookahead heuristics):
//!
//! * `handle_send` marks an invocation — unless `handle_no_arguments` fired
//!   for the same send (official `parseSend` covers both the `f(...)` and
//!   the bare-identifier shape; only the former invokes);
//! * `end_constructor_reference` marks `new C(...)`/`const C(...)`/
//!   implicit-creation heads (which do not travel through `handle_send`);
//! * `handle_assignment_expression` marks the identifier nearest before the
//!   operator as [`RefKind::Written`] (`=`) or [`RefKind::ReadWritten`]
//!   (compound), skipping candidates inside a bracket/cascade chain —
//!   `a[i] = v` writes an element, not the index. Postfix/prefix `++`/`--`
//!   and pattern assignments keep their read kind (documented under-report;
//!   spec §12.2 posture).
//!
//! Sites are appended in source order (the committed parse is
//! left-to-right; speculative parses run on a null listener), which is what
//! makes the send/assignment lookups binary searches. Local declarations
//! (`LocalVariableDeclaration`, formals, catch parameters, labels) and
//! named-argument names are not sites: locals are outside the index by
//! design (spec §3) and named arguments resolve only against a callee's
//! formal list (#119's table, not a namespace hit).

use std::cell::RefCell;
use std::rc::Rc;

use dartiq_parser::{
    ConstructorReferenceContext, DeclarationKind, IdentifierContext, Parser, ParserFeatures,
    ParserListener, TokenSpan as Token,
};
use dartiq_scanner::TokenTable;
use dartiq_source::{FileId, StringId, StringTable};

use crate::library::RefKind;

/// The declaration family of a row: the #119 top-level kind set joined
/// with its member kinds. Import prefixes are library-face entries from
/// import edges, never parse rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DeclKind {
    /// A class (including named mixin applications).
    Class,
    /// An enum declaration.
    Enum,
    /// A mixin declaration.
    Mixin,
    /// A named extension declaration.
    Extension,
    /// An extension-type declaration.
    ExtensionType,
    /// A typedef (either style).
    TypeAlias,
    /// A top-level function, getter, or setter (setters under their
    /// `name=` spelling; the top-level face folds all three per #119).
    TopLevelFn,
    /// A top-level variable.
    TopLevelVar,
    /// A type parameter (declaration-scoped in truth; not a library-scope
    /// name — consumers must filter it accordingly).
    TypeParam,
    /// A class method (operator names keep their operator spelling).
    Method,
    /// A class getter, under its plain name.
    Getter,
    /// A class setter, under its `name=` spelling.
    Setter,
    /// A class field (enum constants count as static fields).
    Field,
}

/// One declaration row: name, kind, name-token span, and the container
/// chain anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeclRow {
    /// The declared name (lookup spelling: setters under `name=`).
    pub name: StringId,
    /// The declaration family.
    pub kind: DeclKind,
    /// The name token's span (UTF-16 — the engine's external convention,
    /// carried verbatim from `TokenSpan.range`).
    pub range: dartiq_source::SourceRange,
    /// Row index of the enclosing container declaration; [`Self::NONE`] for
    /// top-level rows and unnamed-extension members.
    pub container: u32,
}

impl DeclRow {
    /// The `container` value of a row with no enclosing container.
    pub const NONE: u32 = u32::MAX;
}

/// A direct superclass **name** edge from the parse (`extends S`, a named
/// mixin application's head type; `p.S` keeps `S`). Resolved against the
/// owning library at build time ([`crate::LibraryBuilder`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SuperEdge {
    /// Row index of the subtype container.
    pub container: u32,
    /// The supertype's bare name.
    pub name: StringId,
}

/// The unlinked declaration surface of one file, built by [`capture`].
///
/// Rows are sorted by `(name, offset)`; equal-name runs (duplicate
/// declarations are #120's `duplicate_definition` business, not a modeling
/// event) are contiguous for binary-search probes.
#[derive(Debug, Default)]
pub struct FileIndex {
    decls: Vec<DeclRow>,
    supers: Vec<SuperEdge>,
}

impl FileIndex {
    /// The equal-name run of `name`, if the file declares it.
    #[must_use]
    pub fn by_name(&self, name: StringId) -> Option<&[DeclRow]> {
        let start = self.decls.partition_point(|row| row.name < name);
        let end = self.decls[start..].partition_point(|row| row.name == name) + start;
        (start < end).then(|| &self.decls[start..end])
    }

    /// Whether the file declares `name` (any kind, private included).
    #[must_use]
    pub fn has(&self, name: StringId) -> bool {
        self.by_name(name).is_some()
    }

    /// The row at `index` (container anchors point here).
    ///
    /// # Panics
    ///
    /// Panics when `index` is out of range — anchors are minted by
    /// [`capture`] against the same table, so an out-of-range index is a
    /// program bug.
    #[must_use]
    pub fn row(&self, index: u32) -> &DeclRow {
        &self.decls[index as usize]
    }

    /// Every declaration row, ascending by `(name, offset)`.
    #[must_use]
    pub fn decls(&self) -> &[DeclRow] {
        &self.decls
    }

    /// Every direct superclass name edge, ascending by container row.
    #[must_use]
    pub fn supers(&self) -> &[SuperEdge] {
        &self.supers
    }

    /// The number of declaration rows.
    #[must_use]
    pub fn len(&self) -> usize {
        self.decls.len()
    }

    /// Whether the file declares nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.decls.is_empty()
    }
}

/// The reference-site family of a raw site: a bare name (resolvable
/// file-locally or through the namespace) or a `.`-continuation (a
/// prefix-qualified name when a prefix receiver armed it, a member access
/// otherwise). Private encoding; [`FileCapture::sites`] carries it to the
/// library builder.
#[derive(Clone, Copy)]
#[repr(u8)]
pub(crate) enum Family {
    Bare = 0,
    Continuation = 1,
}

/// The raw reference sites of one file: parallel columns in source order
/// (offsets ascending — the property the in-place refinements binary
/// search on). Promoted into [`crate::LibraryIndex`] tables at build time.
#[derive(Debug, Default)]
pub struct RawSites {
    names: Vec<StringId>,
    kinds: Vec<u8>,
    families: Vec<u8>,
    offsets: Vec<u32>,
    lengths: Vec<u32>,
}

impl RawSites {
    /// The number of sites.
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.names.len()
    }

    /// The site's name.
    #[must_use]
    pub(crate) fn name(&self, i: usize) -> StringId {
        self.names[i]
    }

    /// The site's refined kind.
    #[must_use]
    pub(crate) fn kind(&self, i: usize) -> RefKind {
        RefKind::from_u8(self.kinds[i])
    }

    /// The site's family.
    #[must_use]
    pub(crate) fn family(&self, i: usize) -> Family {
        if self.families[i] == Family::Continuation as u8 {
            Family::Continuation
        } else {
            Family::Bare
        }
    }

    /// The site's span.
    #[must_use]
    pub(crate) fn range(&self, i: usize) -> dartiq_source::SourceRange {
        dartiq_source::SourceRange {
            offset: self.offsets[i],
            length: self.lengths[i],
        }
    }
}

/// One file's capture output: the declaration surface plus the raw
/// reference sites. The declaration surface is additionally stored in
/// [`crate::SymbolIndex`]; both are consumed by
/// [`crate::LibraryBuilder`].
#[derive(Debug)]
pub struct FileCapture {
    /// The captured file.
    pub file: FileId,
    /// The unlinked declaration surface.
    pub index: FileIndex,
    /// The raw reference sites.
    pub sites: RawSites,
}

/// The capture state shared between the listener and [`capture`].
struct Builder<'src, 'tab> {
    source: &'src str,
    tokens: &'tab TokenTable,
    strings: &'src StringTable,
    file: FileId,
    decls: Vec<DeclRow>,
    /// Open containers: their decl row ([`DeclRow::NONE`] for unnamed
    /// extensions).
    containers: Vec<u32>,
    supers: Vec<SuperEdge>,
    super_armed: bool,
    super_pending: Option<StringId>,
    pending_member: Option<PendingMember>,
    pending_top: Option<PendingTop>,
    pending_typedef: Option<PendingTop>,
    sites: RawSites,
    /// `handle_no_arguments` fired since the last send — the next
    /// `handle_send` is a bare identifier (a read), not an invocation.
    no_args_since: bool,
    /// Furthest end so far of a bracket/cascade access chain; assignment
    /// candidates ending before it are inside the chain, not write targets.
    access_chain_end: u32,
}

struct PendingMember {
    shape: Option<(StringId, DeclKind)>,
    range: dartiq_source::SourceRange,
}

struct PendingTop {
    name: Option<(StringId, dartiq_source::SourceRange)>,
}

impl<'src, 'tab> Builder<'src, 'tab> {
    fn new(
        file: FileId,
        source: &'src str,
        tokens: &'tab TokenTable,
        strings: &'src StringTable,
    ) -> Self {
        Self {
            source,
            tokens,
            strings,
            file,
            decls: Vec::new(),
            containers: Vec::new(),
            supers: Vec::new(),
            super_armed: false,
            super_pending: None,
            pending_member: None,
            pending_top: None,
            pending_typedef: None,
            sites: RawSites::default(),
            no_args_since: false,
            access_chain_end: 0,
        }
    }

    /// The token's text, `None` for parser-inserted synthetic tokens (which
    /// are not in the table and carry no honest name).
    fn name_of(&self, token: Token) -> Option<StringId> {
        self.tokens
            .get(token.id as usize)
            .map(|t| self.strings.intern(&self.source[t.byte_range()]))
    }

    /// The token's lexeme, `None` for synthetic tokens.
    fn text_of(&self, token: Token) -> Option<&'src str> {
        self.tokens
            .get(token.id as usize)
            .map(|t| &self.source[t.byte_range()])
    }

    fn setter_spelling(&self, name: StringId) -> StringId {
        let setter = format!("{}=", self.strings.resolve(name));
        self.strings.intern(&setter)
    }

    /// The open container's decl row, `NONE` inside an unnamed extension.
    fn container(&self) -> u32 {
        self.containers.last().copied().unwrap_or(DeclRow::NONE)
    }

    /// Commits a declaration row anchored to the open container.
    fn commit(&mut self, name: StringId, kind: DeclKind, range: dartiq_source::SourceRange) {
        let container = self.container();
        self.decls.push(DeclRow {
            name,
            kind,
            range,
            container,
        });
    }

    /// Opens a container: its own row (unless unnamed) plus the stack push.
    fn open_container(
        &mut self,
        kind: DeclKind,
        name: Option<(StringId, dartiq_source::SourceRange)>,
    ) {
        let row = name.map_or(DeclRow::NONE, |(name, range)| {
            self.decls.push(DeclRow {
                name,
                kind,
                range,
                container: DeclRow::NONE,
            });
            crate::narrow(self.decls.len() - 1)
        });
        self.containers.push(row);
        self.super_armed = false;
        self.super_pending = None;
    }

    fn close_container(&mut self) {
        self.super_armed = false;
        self.super_pending = None;
        self.containers.pop();
    }

    /// Commits the armed superclass edge to the open container (the armed
    /// flag makes this one-shot; the first type after `extends` or the head
    /// type of a named mixin application wins).
    fn commit_superclass(&mut self) {
        if !self.super_armed {
            return;
        }
        self.super_armed = false;
        if let (Some(name), container) = (self.super_pending.take(), self.container())
            && container != DeclRow::NONE
        {
            self.supers.push(SuperEdge { container, name });
        }
    }

    /// Resolves a method's member name and kind from its get-or-set token.
    fn member_shape(&self, get_or_set: Option<Token>, name: StringId) -> (StringId, DeclKind) {
        match get_or_set.and_then(|t| self.text_of(t)) {
            Some("get") => (name, DeclKind::Getter),
            Some("set") => (self.setter_spelling(name), DeclKind::Setter),
            _ => (name, DeclKind::Method),
        }
    }

    /// Appends a raw reference site (synthetic tokens contribute nothing).
    fn site(&mut self, token: Token, family: Family, kind: RefKind) {
        let Some(name) = self.name_of(token) else {
            return;
        };
        self.sites.names.push(name);
        self.sites.kinds.push(kind as u8);
        self.sites.families.push(family as u8);
        self.sites.offsets.push(token.range.offset);
        self.sites.lengths.push(token.range.length);
    }

    /// Upgrades the site at `token`'s offset to [`RefKind::Invoked`] (the
    /// send and constructor-reference events carry the target's span).
    fn mark_invoked(&mut self, token: Token) {
        if let Ok(i) = self.sites.offsets.binary_search(&token.range.offset) {
            self.sites.kinds[i] = RefKind::Invoked as u8;
        }
    }

    fn finish(mut self) -> FileCapture {
        // A bailed parse can leave containers open; flush them so captured
        // members still surface (under-report bias).
        while !self.containers.is_empty() {
            self.close_container();
        }
        // Sort rows by (name, offset) and remap the container anchors and
        // superclass edges through the permutation.
        let mut order: Vec<u32> = (0..crate::narrow(self.decls.len())).collect();
        order.sort_by_key(|&i| {
            (
                self.decls[i as usize].name,
                self.decls[i as usize].range.offset,
            )
        });
        let mut remap = vec![DeclRow::NONE; self.decls.len()];
        for (new, &old) in order.iter().enumerate() {
            remap[old as usize] = crate::narrow(new);
        }
        let decls: Vec<DeclRow> = order
            .iter()
            .map(|&old| {
                let mut row = self.decls[old as usize];
                if row.container != DeclRow::NONE {
                    row.container = remap[row.container as usize];
                }
                row
            })
            .collect();
        let supers: Vec<SuperEdge> = std::mem::take(&mut self.supers);
        let mut supers = supers
            .into_iter()
            .map(|mut edge| {
                edge.container = remap[edge.container as usize];
                edge
            })
            .collect::<Vec<SuperEdge>>();
        supers.sort_by_key(|edge| edge.container);
        FileCapture {
            file: self.file,
            index: FileIndex { decls, supers },
            sites: self.sites,
        }
    }
}

/// The [`ParserListener`] implementation feeding a shared [`Builder`].
struct Capturer<'src, 'tab> {
    builder: Rc<RefCell<Builder<'src, 'tab>>>,
}

impl Capturer<'_, '_> {
    /// Container-opening event shared body: the name row plus the stack
    /// push (a synthetic name opens an unnamed container and keeps the
    /// stack balanced).
    fn open_container(&self, token: Token, kind: DeclKind) {
        let mut builder = self.builder.borrow_mut();
        let named = builder.name_of(token).map(|name| (name, token.range));
        builder.open_container(kind, named);
    }
}

/// Parses one compilation unit and captures its declaration surface and
/// raw reference sites.
///
/// `tokens` must be the scan of `source` (the parser reads lexemes through
/// the same pair); `strings` receives every interned name and must be the
/// table the caller resolves ids against afterwards.
///
/// # Panics
///
/// Panics never in practice: the single `expect` reclaims the shared
/// builder after the parser (its sole co-owner) is dropped, so the strong
/// count is 1 by construction.
pub fn capture(
    file: FileId,
    source: &str,
    tokens: &TokenTable,
    features: ParserFeatures,
    strings: &StringTable,
) -> FileCapture {
    let builder = Rc::new(RefCell::new(Builder::new(file, source, tokens, strings)));
    let mut parser = Parser::new(
        source,
        tokens,
        features,
        Capturer {
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

impl ParserListener for Capturer<'_, '_> {
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
        self.open_container(name, DeclKind::Class);
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
        self.open_container(name, DeclKind::Class);
        // The head type (`class C = A with B;`) is the superclass edge.
        let mut builder = self.builder.borrow_mut();
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
        self.open_container(name, DeclKind::Mixin);
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
        self.open_container(name, DeclKind::Enum);
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
        let mut builder = self.builder.borrow_mut();
        let named = name.and_then(|token| builder.name_of(token).map(|n| (n, token.range)));
        builder.open_container(DeclKind::Extension, named);
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
        self.open_container(name, DeclKind::ExtensionType);
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
        use IdentifierContext as C;
        // Reference contexts first (the hot path); declaration contexts
        // below.
        let site = match context {
            C::TypeReference | C::PrefixedTypeReference | C::MetadataReference => {
                Some((Family::Bare, RefKind::Referenced))
            }
            C::TypeReferenceContinuation
            | C::MetadataContinuation
            | C::MetadataContinuationAfterTypeArguments => {
                Some((Family::Continuation, RefKind::Referenced))
            }
            C::Expression => Some((Family::Bare, RefKind::Read)),
            C::ExpressionContinuation => Some((Family::Continuation, RefKind::Read)),
            C::ConstructorReference => Some((Family::Bare, RefKind::Invoked)),
            C::ConstructorReferenceContinuation
            | C::ConstructorReferenceContinuationAfterTypeArguments => {
                Some((Family::Continuation, RefKind::Invoked))
            }
            _ => None,
        };
        if let Some((family, kind)) = site {
            self.builder.borrow_mut().site(token, family, kind);
            return;
        }
        let mut builder = self.builder.borrow_mut();
        let Some(name) = builder.name_of(token) else {
            return;
        };
        match context {
            C::TopLevelVariableDeclaration => {
                builder.commit(name, DeclKind::TopLevelVar, token.range)
            }
            C::FieldDeclaration | C::EnumValueDeclaration => {
                builder.commit(name, DeclKind::Field, token.range);
            }
            C::TopLevelFunctionDeclaration => {
                if let Some(pending) = &mut builder.pending_top {
                    pending.name = Some((name, token.range));
                }
            }
            C::TypedefDeclaration if builder.pending_typedef.is_some() => {
                if let Some(pending) = &mut builder.pending_typedef {
                    pending.name = Some((name, token.range));
                }
            }
            _ => {}
        }
    }

    fn begin_type_variable(&mut self, token: Token) {
        let mut builder = self.builder.borrow_mut();
        if let Some(name) = builder.name_of(token) {
            builder.commit(name, DeclKind::TypeParam, token.range);
        }
    }

    fn begin_method(
        &mut self,
        _kind: DeclarationKind,
        _augment_kw: Option<Token>,
        _external_kw: Option<Token>,
        _static_kw: Option<Token>,
        _covariant_kw: Option<Token>,
        _var_final_or_const: Option<Token>,
        get_or_set: Option<Token>,
        name: Token,
        _enclosing_declaration_name: Option<&str>,
    ) {
        let mut builder = self.builder.borrow_mut();
        let shape = builder
            .name_of(name)
            .map(|n| builder.member_shape(get_or_set, n));
        builder.pending_member = Some(PendingMember {
            shape,
            range: name.range,
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
            && let Some((name, kind)) = pending.shape
        {
            builder.commit(name, kind, pending.range);
        }
    }

    fn begin_top_level_method(
        &mut self,
        _last_consumed: Token,
        _augment: Option<Token>,
        _external: Option<Token>,
    ) {
        self.builder.borrow_mut().pending_top = Some(PendingTop { name: None });
    }

    fn end_top_level_method(&mut self, _begin: Token, get_or_set: Option<Token>, _end: Token) {
        let mut builder = self.builder.borrow_mut();
        if let Some(pending) = builder.pending_top.take()
            && let Some((name, range)) = pending.name
        {
            let spelling = match get_or_set.and_then(|t| builder.text_of(t)) {
                Some("set") => builder.setter_spelling(name),
                _ => name,
            };
            builder.commit(spelling, DeclKind::TopLevelFn, range);
        }
    }

    fn begin_typedef(&mut self, _token: Token) {
        self.builder.borrow_mut().pending_typedef = Some(PendingTop { name: None });
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
            builder.commit(name, DeclKind::TypeAlias, range);
        }
    }

    fn handle_no_arguments(&mut self, _token: Token) {
        self.builder.borrow_mut().no_args_since = true;
    }

    fn handle_send(&mut self, begin: Token, _end: Token) {
        let mut builder = self.builder.borrow_mut();
        let bare = std::mem::take(&mut builder.no_args_since);
        if !bare {
            builder.mark_invoked(begin);
        }
    }

    fn end_constructor_reference(
        &mut self,
        start: Token,
        _period_before_name: Option<Token>,
        _end: Token,
        _context: ConstructorReferenceContext,
    ) {
        self.builder.borrow_mut().mark_invoked(start);
    }

    fn handle_assignment_expression(&mut self, operator: Token, _end: Token) {
        let mut builder = self.builder.borrow_mut();
        let Some(op) = builder.text_of(operator) else {
            return;
        };
        let count = builder
            .sites
            .offsets
            .partition_point(|&o| o < operator.range.offset);
        if count == 0 {
            return;
        }
        let i = count - 1;
        let candidate_end = builder.sites.offsets[i] + builder.sites.lengths[i];
        if candidate_end >= builder.access_chain_end {
            let kind = if op == "=" {
                RefKind::Written
            } else {
                RefKind::ReadWritten
            };
            builder.sites.kinds[i] = kind as u8;
        }
    }

    fn handle_indexed_expression(&mut self, _question: Option<Token>, _open: Token, close: Token) {
        let mut builder = self.builder.borrow_mut();
        builder.access_chain_end = builder.access_chain_end.max(close.range.end());
    }

    fn handle_cascade_access(&mut self, _operator: Token, end: Token, _is_null_aware: bool) {
        let mut builder = self.builder.borrow_mut();
        builder.access_chain_end = builder.access_chain_end.max(end.range.end());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dartiq_language::{FeatureSet, LanguageVersion};
    use dartiq_scanner::ScanConfig;
    use dartiq_source::FileTable;
    use std::path::Path;

    fn build(source: &str, strings: &StringTable) -> FileCapture {
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
            "scanner diagnostics: {result:?}"
        );
        let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
            LanguageVersion::new(3, 15),
        ));
        capture(file, source, &result.tokens, features, strings)
    }

    fn id(strings: &StringTable, name: &str) -> StringId {
        strings.intern(name)
    }

    /// Declaration rows across the full surface: kinds, name spans, and
    /// the container anchor (members point at their container's row;
    /// duplicates keep both rows in one run; setters live under `name=`
    /// with the bare name token's span).
    #[test]
    fn captures_declaration_rows_with_spans_and_containers() {
        let strings = StringTable::new();
        let source = "class C { void m() {} int f = 0; int get g => 1; set s(int v) {} } \
             enum E { x } \
             mixin M {} \
             extension Ext on int { void em() {} } \
             extension on String { int get ue => 1; } \
             typedef T = int; \
             void top() {} \
             int get tg => 1; \
             set ts(int v) {} \
             final tv = 0; \
             class D<Q> extends B {} \
             class C {}";
        let captured = build(source, &strings);
        let index = &captured.index;
        assert_eq!(index.len(), 19);
        let rows = |name: &str| index.by_name(id(&strings, name)).expect(name);

        // Container anchors survive the (name, offset) sort: members point
        // at their container's row, the container row itself is top-level.
        let c_row = index
            .decls()
            .iter()
            .find(|row| row.name == id(&strings, "C") && row.kind == DeclKind::Class)
            .expect("class C row");
        let c_position = index
            .decls()
            .iter()
            .position(|row| row == c_row)
            .expect("C in table");
        let m = rows("m")[0];
        assert_eq!(m.kind, DeclKind::Method);
        assert_eq!(m.container as usize, c_position);
        assert_eq!(c_row.container, DeclRow::NONE);
        assert_eq!(rows("f")[0].kind, DeclKind::Field);
        assert_eq!(rows("g")[0].kind, DeclKind::Getter);
        // Setters under `name=`; the span stays the bare name token.
        let s = rows("s=")[0];
        assert_eq!(s.kind, DeclKind::Setter);
        let s_offset = source.find("set s(int v)").expect("set s") + "set ".len();
        assert_eq!(s.range.offset as usize, s_offset, "ASCII: utf16 == byte");
        assert_eq!(s.range.length as usize, 1);
        // Unnamed extension member: captured, no container anchor.
        assert_eq!(rows("ue")[0].kind, DeclKind::Getter);
        assert_eq!(rows("ue")[0].container, DeclRow::NONE);
        // Enum constant as a field; type parameter as its own row.
        assert_eq!(rows("x")[0].kind, DeclKind::Field);
        assert_eq!(rows("Q")[0].kind, DeclKind::TypeParam);
        // Top-level getters/setters fold into TopLevelFn with `name=`.
        assert_eq!(rows("tg")[0].kind, DeclKind::TopLevelFn);
        assert!(index.by_name(id(&strings, "ts")).is_none());
        assert_eq!(rows("ts=")[0].kind, DeclKind::TopLevelFn);
        assert_eq!(rows("tv")[0].kind, DeclKind::TopLevelVar);
        // Duplicate names keep both rows as one contiguous run.
        assert_eq!(rows("C").len(), 2);
        // Supertype name edge, container remapped through the sort.
        let d_position = {
            let target = rows("D")[0];
            index
                .decls()
                .iter()
                .position(|row| *row == target)
                .expect("D")
        };
        let supertype = index
            .supers()
            .iter()
            .find(|edge| edge.container as usize == d_position)
            .expect("D extends edge");
        assert_eq!(supertype.name, id(&strings, "B"));
    }

    /// Reference-site capture with the parser-marked refinements: sends
    /// invoke, bare sends stay reads, constructor references invoke,
    /// assignments write (compound → read-written), and bracket-chain
    /// candidates are skipped.
    #[test]
    fn captures_reference_sites_with_refinements() {
        let strings = StringTable::new();
        let source = "void main() { \
               fa(); \
               int x = fb; \
               x = 2; \
               x += 3; \
               a[i] = v; \
               list[0] = 1; \
               p.q(); \
               new K(); \
             } \
             void fa() {} \
             void fb() {}";
        let captured = build(source, &strings);
        let sites = &captured.sites;
        let nth_kind = |name: &str, occurrence: usize| {
            let needle = id(&strings, name);
            let seen = sites
                .names
                .iter()
                .enumerate()
                .filter(|&(_, &n)| n == needle)
                .map(|(i, _)| i)
                .nth(occurrence)
                .unwrap_or_else(|| panic!("site {name} #{occurrence}"));
            sites.kind(seen)
        };
        assert_eq!(nth_kind("fa", 0), RefKind::Invoked, "called: f()");
        assert_eq!(nth_kind("fb", 0), RefKind::Read, "bare value use");
        assert_eq!(nth_kind("x", 0), RefKind::Written, "x = 2");
        assert_eq!(nth_kind("x", 1), RefKind::ReadWritten, "x += 3");
        assert_eq!(
            nth_kind("a", 0),
            RefKind::Read,
            "receiver of a bracket write"
        );
        assert_eq!(nth_kind("i", 0), RefKind::Read, "index read stays a read");
        assert_eq!(nth_kind("v", 0), RefKind::Read);
        assert_eq!(
            nth_kind("list", 0),
            RefKind::Read,
            "receiver of a literal-index write: inside the chain, stays a read"
        );
        assert_eq!(nth_kind("p", 0), RefKind::Read, "prefix-shaped receiver");
        assert_eq!(
            nth_kind("q", 0),
            RefKind::Invoked,
            "p.q() invokes the member"
        );
        assert_eq!(nth_kind("K", 0), RefKind::Invoked, "new K()");
        // Sites are captured in source order with ascending offsets.
        let mut last = 0;
        for i in 0..sites.len() {
            assert!(sites.range(i).offset >= last);
            last = sites.range(i).offset;
        }
        // Declaration names are not sites.
        assert!(sites.names.iter().all(|&n| n != id(&strings, "main")));
    }

    /// A bailed parse still surfaces what was captured (under-report bias),
    /// and the container-stack flush keeps anchors consistent.
    #[test]
    fn bailed_parse_flushes_open_containers() {
        let strings = StringTable::new();
        // `class B {` never closes; the member must still surface.
        let captured = build("class B { void m() {}", &strings);
        assert!(captured.index.by_name(id(&strings, "B")).is_some());
        assert!(captured.index.by_name(id(&strings, "m")).is_some());
    }
}
