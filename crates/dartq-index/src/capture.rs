//! The unlinked declaration capture: one
//! pass over the parser event stream records every declaration the parse
//! surface carries — with its name-token span and container anchor — and
//! every identifier use as a raw reference site, refined in place by the
//! parser's send/assignment/constructor-reference events.
//!
//! # Capture mechanics
//!
//! The `src/tests/collector.rs` collecting-listener precedent, applied with
//! spans (the dartq-semantic collector records name sets; this one records
//! rows): parser events key the capture, the listener takes the builder
//! behind an `Rc<RefCell>` shared with [`capture`], and recovery-synthetic
//! tokens (not in the token table) contribute nothing.
//!
//! * Containers (`class`/`enum`/`mixin`/`extension`/extension type, plus
//!   named mixin applications) commit their own row at `begin_*` and push
//!   their row index; members commit with the stack top as their container
//!   anchor. Dart has no nested type declarations, so the chain is at most
//!   one hop. Unnamed extensions keep capturing: their members surface with
//!   no container anchor and the [`DeclRow::unnamed_extension_member`] bit
//!   set, which keeps them out of the library faces.
//! * Supertype name edges are clause-typed: every header clause's target
//!   names (`extends S`, `with M, N`, `implements I, J`, `mixin ... on A`;
//!   a named mixin application's head type; `p.S` keeps `S`) are recorded
//!   as [`SuperEdge`]s against the open container and resolved at
//!   library-build time. Type events buffer the clause's candidate names
//!   and the clause completion events commit them — the extends event
//!   keeps one type (under comma recovery (`extends B, D`) the official
//!   keeps the first), the with/implements/on events commit the whole
//!   clause list — and an extension type commits nothing. The one gap:
//!   an extension's `on T` has no completion event
//!   (`end_extension_declaration` carries only the keyword), so `T` is
//!   not recorded — an extension's applied type is no supertype relation
//!   anyway; a parser-side completion event is the seam if it ever must be.
//! * Constructors, factories, and named formals are **not** declaration
//!   rows: the kind set has no constructor kind and the formals table is
//!   #119's (`undefined_named_parameter` consumes it there). Operators
//!   commit under whatever spelling `begin_method` hands over.
//! * Positional columns (`DeclRow::decl_end`, `DeclRow::body_open`) ride
//!   the same events: a container's braces and every member's closing
//!   token arrive on the end events the capture already consumes, and the
//!   token table's byte face turns each end into an exact byte offset at
//!   zero extra walks — byte units hold for every lexeme, including
//!   multi-byte and astral content where the events' UTF-16 spans
//!   diverge. Shapes whose events carry no end (type parameters, enum
//!   constants) and rows whose declaration never closed — including ends
//!   closed by a parser-inserted token, which has no byte position — stay
//!   [`DeclRow::UNKNOWN`], never a plausible-looking guess.
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
//! * `handle_send` marks an invocation — unless it ends on the very token
//!   the bare-send marker armed for (`handle_no_arguments` under dartq's
//!   emission, `handle_send_without_arguments` under the official one;
//!   official `parseSend` covers both the `f(...)` and the bare-identifier
//!   shape; only the former invokes, and the bare shape's send ends on
//!   exactly the marker's token, so the unpaired firings — enum constants,
//!   bare annotations, const patterns — never poison a later send);
//! * `end_constructor_reference` marks `new C(...)`/`const C(...)`/
//!   implicit-creation heads (which do not travel through `handle_send`);
//! * `handle_assignment_expression` marks the identifier nearest before the
//!   operator as [`RefKind::Written`] (`=`) or [`RefKind::ReadWritten`]
//!   (compound), skipping candidates inside a bracket/cascade chain —
//!   `a[i] = v` writes an element, not the index. Postfix/prefix `++`/`--`
//!   and pattern assignments keep their read kind (documented under-report;
//!   the pinned under-report posture).
//!
//! Sites are appended in source order (the committed parse is
//! left-to-right; speculative parses run on a null listener), which is what
//! makes the send/assignment lookups binary searches. Local declarations
//! (`LocalVariableDeclaration`, formals, catch parameters, labels) and
//! named-argument names are not sites: locals are outside the index by
//! design and named arguments resolve only against a callee's
//! formal list (#119's table, not a namespace hit).

use std::cell::RefCell;
use std::rc::Rc;

use dartq_parser::{
    ConstructorReferenceContext, DeclarationKind, IdentifierContext, Parser, ParserFeatures,
    ParserListener, TokenSpan as Token,
};
use dartq_scanner::TokenTable;
use dartq_source::{FileId, StringId, StringTable};

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

/// One declaration row: name, kind, name-token span, positional columns,
/// and the container chain anchor.
///
/// The positional columns (`decl_end`, `body_open`) are byte offsets, not
/// UTF-16 — they feed insertion-point arithmetic (fix producers compose
/// edits by byte offset), while `range` stays UTF-16 for the external
/// convention. Both carry [`Self::UNKNOWN`] wherever the parse supplies no
/// position; a sentinel beats an `Option` pack because the row already
/// anchors with one ([`Self::NONE`]) and the row must stay its packed
/// 28-byte size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeclRow {
    /// The declared name (lookup spelling: setters under `name=`).
    pub name: StringId,
    /// The declaration family.
    pub kind: DeclKind,
    /// The row is a member of an unnamed extension: it carries no container
    /// anchor (the extension has no row to point at) and — unlike other
    /// `container == NONE` rows — is not a library face. Fits in the `kind`
    /// field's padding.
    pub unnamed_extension_member: bool,
    /// The name token's span (UTF-16 — the engine's external convention,
    /// carried verbatim from `TokenSpan.range`).
    pub range: dartq_source::SourceRange,
    /// Row index of the enclosing container declaration; [`Self::NONE`] for
    /// top-level rows and unnamed-extension members.
    pub container: u32,
    /// Exclusive end offset of the declaration's last token (byte offset one
    /// past the `}`/`;` the parse closed it with — the upstream
    /// `enclosingMember.end` convention). [`Self::UNKNOWN`] where no end
    /// event attributes the row: type parameters (the parser's
    /// `end_type_variable` carries no reliable end), enum constants (no
    /// per-element end event), and rows whose declaration never closed
    /// (bailed parse, recovery-consumed member, or a closing token the
    /// parser inserted — it has no byte position).
    pub decl_end: u32,
    /// Byte offset one past a brace body's `{` — the insertion point for
    /// member synthesis into the container. Container rows only;
    /// [`Self::UNKNOWN`] for every other row, for bodyless containers
    /// (`class C;` shapes, named mixin applications), and for a
    /// parser-inserted brace (no byte position).
    pub body_open: u32,
}

impl DeclRow {
    /// The `container` value of a row with no enclosing container.
    pub const NONE: u32 = u32::MAX;

    /// The value of a positional column the parse could not derive. Never a
    /// valid offset: consumers must narrow before use, never compare against
    /// a source bound.
    pub const UNKNOWN: u32 = u32::MAX;

    /// Whether the row is a library-scope name: a top-level row that is
    /// neither a type parameter nor an unnamed-extension member. Members
    /// and type parameters are not scope names — the filter the graph
    /// declarer and the bare-name resolver's local hop share.
    #[must_use]
    pub fn is_library_face(&self) -> bool {
        self.container == DeclRow::NONE
            && self.kind != DeclKind::TypeParam
            && !self.unnamed_extension_member
    }
}

/// The packed-row pin: name + kind(+bit) + range + container anchors (20 B)
/// plus the two positional u32 columns (8 B). The capture's standing cost is
/// ~8 bytes per declaration for the positional columns — the D2 ruling's
/// budget — and any future field must re-negotiate it here, not silently
/// pad the row.
const _: () = assert!(std::mem::size_of::<DeclRow>() == 28);

/// The supertype clause an edge came from, ordered so every declaration
/// shape's clause sort key equals its source order — a class header
/// (`extends` < `with` < `implements`), a mixin header (`on` <
/// `implements`; the other two absent), an enum header (`with` <
/// `implements`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum SuperClause {
    /// `extends B` — the superclass, or a named mixin application's head
    /// type (`class C = A with B;` names `A`).
    Extends = 0,
    /// `with M, N` — the with-clause mixins of a class, enum, or named
    /// mixin application.
    With = 1,
    /// `on A, B` — a mixin's superclass constraint.
    On = 2,
    /// `implements I, J` — the interface clause of a class, mixin, enum,
    /// or named mixin application.
    Implements = 3,
}

/// A direct supertype **name** edge from the parse: the clause it came
/// from plus the clause's target name (`p.S` keeps `S`). Resolved against
/// the owning library at build time ([`crate::LibraryBuilder`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SuperEdge {
    /// Row index of the subtype container.
    pub container: u32,
    /// The supertype's bare name.
    pub name: StringId,
    /// The clause the name was written in.
    pub clause: SuperClause,
}

/// The unlinked declaration surface of one file, built by [`capture`].
///
/// Rows are sorted by `(name, offset)`; equal-name runs (duplicate
/// declarations are #120's `duplicate_definition` business, not a modeling
/// event) are contiguous for binary-search probes.
#[derive(Clone, Debug, Default)]
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

    /// Whether the file declares `name` at library scope: a top-level row
    /// that is neither a type parameter nor an unnamed-extension member
    /// ([`DeclRow::is_library_face`]). Members and type parameters are not
    /// library faces — the filter the graph declarer and the bare-name
    /// resolver's local hop share.
    #[must_use]
    pub fn has_library_face(&self, name: StringId) -> bool {
        self.by_name(name)
            .is_some_and(|rows| rows.iter().any(DeclRow::is_library_face))
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

    /// Every direct supertype name edge, ascending by `(container row,
    /// clause)`.
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
/// Crate-internal: the only consumer is [`crate::LibraryBuilder`], and no
/// external face names this type (#547 hygiene).
#[derive(Debug)]
pub(crate) struct RawSites {
    names: Vec<StringId>,
    kinds: Vec<u8>,
    families: Vec<u8>,
    offsets: Vec<u32>,
    lengths: Vec<u32>,
}

impl RawSites {
    /// No columns reserved — the shed twin's shape
    /// ([`FileCapture::without_sites`]).
    fn empty() -> Self {
        Self {
            names: Vec::new(),
            kinds: Vec::new(),
            families: Vec::new(),
            offsets: Vec::new(),
            lengths: Vec::new(),
        }
    }

    /// Empty columns reserved to `tokens` — the file's token count, the
    /// upper bound on identifier sites (a site is one identifier token) —
    /// so the parse grows the five columns without reallocation.
    fn with_capacity(tokens: usize) -> Self {
        Self {
            names: Vec::with_capacity(tokens),
            kinds: Vec::with_capacity(tokens),
            families: Vec::with_capacity(tokens),
            offsets: Vec::with_capacity(tokens),
            lengths: Vec::with_capacity(tokens),
        }
    }

    /// The number of sites.
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.names.len()
    }

    /// The reserved column capacity in rows. `with_capacity` reserves the
    /// file's token count and the columns push in lockstep (`site` is the
    /// only push point), so one face reports the whole table.
    #[must_use]
    pub(crate) fn capacity(&self) -> usize {
        self.names.capacity()
    }

    /// Sheds the token-count reservation the columns were born with — sites
    /// are a small fraction of tokens, so without this the finished capture
    /// pins ~2/3 of its reserved site bytes for the session (the same
    /// freeze-time shrink `LibraryBuilder` applies to its tables).
    fn shrink_to_fit(&mut self) {
        self.names.shrink_to_fit();
        self.kinds.shrink_to_fit();
        self.families.shrink_to_fit();
        self.offsets.shrink_to_fit();
        self.lengths.shrink_to_fit();
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
    pub(crate) fn range(&self, i: usize) -> dartq_source::SourceRange {
        dartq_source::SourceRange {
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
    /// The raw reference sites. Crate-internal with [`RawSites`] (the
    /// only consumer is [`crate::LibraryBuilder`]).
    pub(crate) sites: RawSites,
    /// Whether the sites were shed after the capture was built (`#1859`
    /// LRU discipline): the engine's site cache capped out and replaced
    /// this capture with its [`FileCapture::without_sites`] twin. A fresh
    /// capture is never shed; the declaration surface is unaffected, and
    /// the sites re-derive purely from (content, feature set) on the next
    /// library build.
    shed: bool,
}

impl FileCapture {
    /// The site table's reserved capacity in rows — the column count
    /// `RawSites` holds heap-side. `Builder::finish` shrinks the
    /// token-count reservation down to the row count, so this reports the
    /// pinned slack (post-freeze: `== sites.len()`; the memory-accounting
    /// pin and the RSS-decomposition bench read it). Zero once the sites
    /// were shed.
    #[must_use]
    pub fn sites_capacity(&self) -> usize {
        self.sites.capacity()
    }

    /// Whether the raw sites are mounted. False only for the shed twin
    /// ([`FileCapture::without_sites`]) — a file with genuinely zero sites
    /// still reports true, so the engine's rebuild face can tell "nothing
    /// to fold" from "shed, re-derive".
    #[must_use]
    pub fn sites_present(&self) -> bool {
        !self.shed
    }

    /// The capture's owned heap bytes at reserved capacity: the
    /// declaration rows, the supertype edges, and the five site columns
    /// (`u32` name + kind byte + family byte + `u32` offset + `u32` length
    /// per site). Exact over the columns this type owns; the allocator's
    /// per-block rounding is not counted. The engine's memory
    /// decomposition reads it.
    #[must_use]
    pub fn heap_bytes(&self) -> usize {
        use std::mem::size_of;
        self.index.decls.capacity() * size_of::<DeclRow>()
            + self.index.supers.capacity() * size_of::<SuperEdge>()
            + self.sites.capacity() * (size_of::<StringId>() + 2 + 2 * size_of::<u32>())
    }

    /// The shed twin: the same file and declaration surface with the site
    /// columns emptied and [`FileCapture::sites_present`] cleared — the
    /// drop-the-payload half of the engine's site LRU (`#1859`), which
    /// swaps resident captures for this twin when the cache caps out and
    /// re-derives the sites from content on the next library build. The
    /// declaration rows are cloned, not shared: the twin replaces the
    /// original in the engine's arena, and the columns are flat
    /// `Copy`-row `Vec`s, so the clone is the whole cost.
    #[must_use]
    pub fn without_sites(&self) -> FileCapture {
        FileCapture {
            file: self.file,
            index: self.index.clone(),
            sites: RawSites::empty(),
            shed: true,
        }
    }
}

/// The capture state shared between the listener and [`capture`].
struct Builder<'src, 'tab> {
    source: &'src str,
    tokens: &'tab TokenTable,
    strings: &'src StringTable,
    file: FileId,
    decls: Vec<DeclRow>,
    /// Open containers: their decl row ([`DeclRow::NONE`] for unnamed
    /// extensions), their kind, and whether a supertype edge already
    /// committed for them.
    containers: Vec<Container>,
    supers: Vec<SuperEdge>,
    /// Depth-0 type-name candidates since the last commit/reset: each
    /// supertype parse appends its name, and the clause completion events
    /// commit one of them (see [`Builder::commit_superclass`]). The buffer
    /// holds the whole clause, so a multiple-extends recovery can commit
    /// the clause's first type the way the official listener pops to.
    super_candidates: Vec<StringId>,
    /// Token id of the last prefixed type-reference identifier. A qualified
    /// type's trailing `handle_type` carries that same token (the parser
    /// passes the head, not the tail), and the stored continuation name
    /// must survive it.
    prefixed_type: Option<u32>,
    /// A named mixin application's head type is due: set at
    /// `begin_named_mixin_application`, discharged by the with list's
    /// `begin_type_list` — the first event after the head type parse.
    super_head_armed: bool,
    pending_member: Option<PendingMember>,
    pending_top: Option<PendingTop>,
    pending_typedef: Option<PendingTop>,
    /// First row index of the open field group (`begin_fields` →
    /// `end_fields`/`end_top_level_fields`): the group's variables commit at
    /// their name tokens, but the group's end arrives at the closing event —
    /// the whole trailing run gets stamped then. Never nests (a field group
    /// contains no declarations), so one slot suffices; a bail leaves it set
    /// and the unended group's rows honestly `UNKNOWN`.
    field_group: Option<usize>,
    sites: RawSites,
    /// The token the bare-send marker last armed for (`handle_no_arguments`
    /// under dartq's emission, `handle_send_without_arguments` under the
    /// official one). A send is the bare identifier shape only when it ends
    /// on that same token; unpaired firings (enum constants, bare
    /// annotations, const patterns) are never followed by a matching send,
    /// so they cannot poison later ones.
    no_args_at: Option<u32>,
    /// Open type-argument lists: `handle_type` events inside them name type
    /// arguments (`extends B<int>` must not store `int`), never the
    /// supertype itself.
    type_arg_depth: usize,
    /// Furthest end so far of a bracket/cascade access chain; assignment
    /// candidates ending before it are inside the chain, not write targets.
    /// The nested-assignment shape `x = a[i] = v;` is the visible
    /// casualty: `x`'s candidate ends before the index chain's close, so
    /// its `Written` label is suppressed — the documented M1 under-report
    /// posture, recorded here so the shape has a name.
    access_chain_end: u32,
}

/// An open container: its decl row ([`DeclRow::NONE`] for unnamed
/// extensions), its family, and whether an extends edge already committed
/// for it (duplicate-extends recovery fires the clause event again — the
/// container commits one extends edge; the other clauses commit every
/// event's own types and need no guard).
struct Container {
    row: u32,
    kind: DeclKind,
    extends_committed: bool,
}

struct PendingMember {
    shape: Option<(StringId, DeclKind)>,
    range: dartq_source::SourceRange,
}

struct PendingTop {
    name: Option<(StringId, dartq_source::SourceRange)>,
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
            super_candidates: Vec::new(),
            prefixed_type: None,
            super_head_armed: false,
            pending_member: None,
            pending_top: None,
            pending_typedef: None,
            field_group: None,
            sites: RawSites::with_capacity(tokens.len()),
            no_args_at: None,
            type_arg_depth: 0,
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

    /// The token's exclusive end as a byte offset — the positional columns'
    /// unit, read straight off the token table's byte face (`byte_offset +
    /// byte_len`). Byte units are exact for every lexeme, including
    /// multi-byte and astral (surrogate-pair) content where the event's
    /// UTF-16 span under-counts. A parser-inserted token has no byte face
    /// (its id sits past the table): the declaration it closes never
    /// existed in the source, so the row takes [`DeclRow::UNKNOWN`].
    fn byte_end(&self, token: Token) -> u32 {
        self.tokens
            .get(token.id as usize)
            .map_or(DeclRow::UNKNOWN, |t| t.byte_offset + t.byte_len)
    }

    /// The setter's `name=` lookup spelling. Setter commits repeat a short
    /// name, so the key composes into a stack buffer and only a name longer
    /// than it falls back to the heap.
    fn setter_spelling(&self, name: StringId) -> StringId {
        let name = self.strings.resolve(name);
        const STACK_KEY: usize = 64;
        let mut buf = [0u8; STACK_KEY];
        if name.len() + 1 <= STACK_KEY {
            buf[..name.len()].copy_from_slice(name.as_bytes());
            buf[name.len()] = b'=';
            let key =
                std::str::from_utf8(&buf[..name.len() + 1]).expect("utf-8 name plus `=` is utf-8");
            self.strings.intern(key)
        } else {
            self.strings.intern(&format!("{name}="))
        }
    }

    /// The open container's decl row, `NONE` inside an unnamed extension.
    fn container(&self) -> u32 {
        self.containers.last().map_or(DeclRow::NONE, |c| c.row)
    }

    /// Commits a declaration row anchored to the open container. A row
    /// inside an unnamed extension (a container whose row is `NONE` while
    /// the stack is non-empty) carries the unnamed-extension-member bit:
    /// the crate's contract keeps members out of the library faces, and
    /// without the anchor there is nothing else distinguishing it from a
    /// top-level row.
    ///
    /// `decl_end` is the commit event's own end token as a byte offset
    /// ([`Builder::byte_end`]) where the shape commits at its end
    /// (methods, top-level functions, typedefs) and [`DeclRow::UNKNOWN`]
    /// where the end arrives later (field groups stamp their trailing run
    /// at `end_fields`) or never (type parameters, enum constants).
    fn commit(
        &mut self,
        name: StringId,
        kind: DeclKind,
        range: dartq_source::SourceRange,
        decl_end: u32,
    ) {
        let container = self.container();
        let unnamed_extension_member = container == DeclRow::NONE && !self.containers.is_empty();
        self.decls.push(DeclRow {
            name,
            kind,
            unnamed_extension_member,
            range,
            container,
            decl_end,
            body_open: DeclRow::UNKNOWN,
        });
    }

    /// Opens a container: its own row (unless unnamed) plus the stack push.
    /// The row's positional columns start `UNKNOWN` — the body events stamp
    /// them ([`Builder::stamp_container`]) as the parse delivers the braces.
    fn open_container(
        &mut self,
        kind: DeclKind,
        name: Option<(StringId, dartq_source::SourceRange)>,
    ) {
        let row = name.map_or(DeclRow::NONE, |(name, range)| {
            self.decls.push(DeclRow {
                name,
                kind,
                unnamed_extension_member: false,
                range,
                container: DeclRow::NONE,
                decl_end: DeclRow::UNKNOWN,
                body_open: DeclRow::UNKNOWN,
            });
            crate::narrow(self.decls.len() - 1)
        });
        self.containers.push(Container {
            row,
            kind,
            extends_committed: false,
        });
        self.super_candidates.clear();
        self.super_head_armed = false;
    }

    /// Stamps the open container's body-open column (one past its `{`); a
    /// no-op for unnamed containers — no row to carry the position.
    fn stamp_container_body_open(&mut self, open: Token) {
        if let Some(container) = self.containers.last()
            && container.row != DeclRow::NONE
        {
            self.decls[container.row as usize].body_open = self.byte_end(open);
        }
    }

    /// Stamps the open container's declaration-end column (one past its
    /// closing `}` or `;`); the same no-op contract as
    /// [`Builder::stamp_container_body_open`].
    fn stamp_container_decl_end(&mut self, end: Token) {
        if let Some(container) = self.containers.last()
            && container.row != DeclRow::NONE
        {
            self.decls[container.row as usize].decl_end = self.byte_end(end);
        }
    }

    /// Stamps every declaration row of the open field group with the
    /// group's end (the closing `;`'s exclusive byte end) and closes the
    /// slot. Type parameters are skipped: a generic-function-type field
    /// (`int Function<T>(T) f;`) commits its `<T>` rows between the group's
    /// start and the name commit, and they stay [`DeclRow::UNKNOWN`] —
    /// never the field's `;` end.
    fn stamp_field_group_end(&mut self, end: Token) {
        let end = self.byte_end(end);
        if let Some(start) = self.field_group.take() {
            for row in &mut self.decls[start..] {
                if row.kind != DeclKind::TypeParam {
                    row.decl_end = end;
                }
            }
        }
    }

    fn close_container(&mut self) {
        self.super_candidates.clear();
        self.super_head_armed = false;
        self.containers.pop();
    }

    /// Consumes the supertype candidates into one extends edge for the
    /// open container: `from_end` names which candidate survives, counting
    /// from the buffer's end — `1` (the last) for the shapes the parser
    /// commits right after a single type parse, and the clause's type count
    /// for `handle_class_extends`, whose comma-recovery parses several
    /// types before the event arrives (the official keeps the first type).
    ///
    /// The name is stored where the parser hands it over — `handle_type`
    /// for a plain name, the `TypeReferenceContinuation` identifier for a
    /// qualified `p.S` tail — and committed by the clause completion
    /// events, which the parser fires right after the supertype parse:
    /// `handle_class_extends` (`crates/dartq-parser/src/declarations.rs`
    /// `parse_class_extends_seen`) for the extends clause, and the with
    /// list's `begin_type_list` for a named mixin application's head type
    /// (`parse_mixin_application_rest` parses the mixin list before its own
    /// completion event, so the list's first event is the head's commit
    /// point).
    ///
    /// Nothing commits for an extension type (its `extends` is a
    /// representation-adjacent recovery error, not a supertype), for a
    /// container that already committed (duplicate-extends recovery must
    /// not yield a second edge), or for an unnamed container (no row to
    /// anchor the edge to). The buffer clears either way — the clause is
    /// over.
    fn commit_superclass(&mut self, from_end: usize) {
        let index = self.super_candidates.len().saturating_sub(from_end);
        let name = self.super_candidates.get(index).copied();
        self.super_candidates.clear();
        let Some(container) = self.containers.last_mut() else {
            return;
        };
        if container.kind == DeclKind::ExtensionType
            || container.extends_committed
            || container.row == DeclRow::NONE
        {
            return;
        }
        if let Some(name) = name {
            self.supers.push(SuperEdge {
                container: container.row,
                name,
                clause: SuperClause::Extends,
            });
            container.extends_committed = true;
        }
    }

    /// Consumes the supertype candidates into `clause` edges for the open
    /// container — the with/implements/on shape of
    /// [`Builder::commit_superclass`]. `tail` selects the candidates that
    /// belong to the clause, counting from the buffer's end: `Some(count)`
    /// for the comma-list completion events (`handle_implements` /
    /// `handle_mixin_on`, whose `count` is the clause's type count), `None`
    /// for the with clause, whose completion event carries no count and
    /// whose buffer holds exactly the clause's list (the preceding clause's
    /// event cleared it, and no other types parse in between).
    ///
    /// Nothing commits for an unnamed container (no row to anchor edges
    /// to) or for an extension type (its recovery `with`/`implements` are
    /// representation-adjacent errors, matching the extends posture). The
    /// buffer clears either way — the clause is over; recovery re-fires
    /// (duplicate out-of-order clauses) legitimately add more rows.
    fn commit_clause_supers(&mut self, clause: SuperClause, tail: Option<usize>) {
        let Some(container) = self.containers.last_mut() else {
            self.super_candidates.clear();
            return;
        };
        let start = tail.map_or(0, |count| self.super_candidates.len().saturating_sub(count));
        let end = self.super_candidates.len();
        if container.row != DeclRow::NONE && container.kind != DeclKind::ExtensionType {
            for candidate in start..end {
                self.supers.push(SuperEdge {
                    container: container.row,
                    name: self.super_candidates[candidate],
                    clause,
                });
            }
        }
        self.super_candidates.clear();
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
        // superclass edges through the permutation. The anchors remap in
        // place first (they are values, not positions), then the rows
        // permute in place — the build's only table-sized extra allocation
        // is the index vector driving the permutation.
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
        for row in &mut self.decls {
            if row.container != DeclRow::NONE {
                row.container = remap[row.container as usize];
            }
        }
        let mut supers = std::mem::take(&mut self.supers);
        for edge in &mut supers {
            edge.container = remap[edge.container as usize];
        }
        supers.sort_by_key(|edge| (edge.container, edge.clause));
        // Cycle the rows into their sorted slots through `remap` (old →
        // new): each swap settles the row at `j = remap[i]` and fixes
        // `remap[j]`, so the walk is O(n) swaps with no second table copy
        // and lands every row at exactly the slot the collected gather
        // held. Swapping along `order` instead would apply the inverse
        // permutation and scramble the table.
        for i in 0..remap.len() {
            while remap[i] as usize != i {
                let j = remap[i] as usize;
                self.decls.swap(i, j);
                remap.swap(i, j);
            }
        }
        // Freeze: the site columns move into the long-lived capture, so the
        // token-count reservation `RawSites::with_capacity` opened with must
        // be shed here — sites run well under tokens, and holding the slack
        // would keep ~2/3 of the reserved site bytes resident per file.
        self.sites.shrink_to_fit();
        FileCapture {
            file: self.file,
            index: FileIndex {
                decls: self.decls,
                supers,
            },
            sites: self.sites,
            shed: false,
        }
    }
}

/// The [`ParserListener`] implementation feeding a `Builder`: driven
/// through the parse multiplexer — standalone in [`capture`], or fanned
/// out with the engine's other apply-side consumers — and finished into
/// the file capture once the parse ends.
pub struct Capturer<'src, 'tab> {
    builder: Rc<RefCell<Builder<'src, 'tab>>>,
}

impl<'src, 'tab> Capturer<'src, 'tab> {
    /// Builds the capturer for `file` over the `source`/`tokens` pair the
    /// parser reads, interning every name into `strings`.
    pub fn new(
        file: FileId,
        source: &'src str,
        tokens: &'tab TokenTable,
        strings: &'src StringTable,
    ) -> Self {
        Self {
            builder: Rc::new(RefCell::new(Builder::new(file, source, tokens, strings))),
        }
    }

    /// Reclaims the capture. The multiplexer only borrows the capturer, so
    /// this is the sole remaining builder owner by construction and the
    /// reclaim cannot fail.
    ///
    /// # Panics
    ///
    /// Only if the builder is still shared after the parse — an internal
    /// invariant breach (a live borrow outliving the parse).
    #[must_use]
    pub fn finish(self) -> FileCapture {
        Rc::into_inner(self.builder)
            .expect("sole builder owner after the parse")
            .into_inner()
            .finish()
    }
}

/// Parses one compilation unit and captures its declaration surface and
/// raw reference sites.
///
/// `tokens` must be the scan of `source` (the parser reads lexemes through
/// the same pair); `strings` receives every interned name and must be the
/// table the caller resolves ids against afterwards.
pub fn capture(
    file: FileId,
    source: &str,
    tokens: &TokenTable,
    features: ParserFeatures,
    strings: &StringTable,
) -> FileCapture {
    let mut capturer = Capturer::new(file, source, tokens, strings);
    Parser::new(source, tokens, features, &mut capturer).parse_unit();
    capturer.finish()
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
        // The head type (`class C = A with B;`) is the superclass edge: arm
        // the commit at the with list (see `commit_superclass`).
        self.builder.borrow_mut().super_head_armed = true;
    }

    fn end_named_mixin_application(
        &mut self,
        _begin: Token,
        _class: Token,
        _equals: Token,
        _implements: Option<Token>,
        end: Token,
    ) {
        // The implements targets already came through `handle_implements`
        // (the keyword Option here is header residue); the `end` is the
        // declaration's closing `;`. Stamp, then close the container.
        let mut builder = self.builder.borrow_mut();
        builder.stamp_container_decl_end(end);
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

    // —— container bodies and field groups: the positional columns ————
    // The brace events hand over the real (or recovery-synthetic) `{`/`}`
    // tokens, so the container's row gets the exact insertion point (one
    // past `{`) and the exact end (one past `}`); the no-body events hand
    // over the `;` that closed the declaration.

    fn begin_class_or_mixin_or_extension_body(&mut self, _kind: DeclarationKind, token: Token) {
        self.builder.borrow_mut().stamp_container_body_open(token);
    }

    fn end_class_or_mixin_or_extension_body(
        &mut self,
        _kind: DeclarationKind,
        _count: usize,
        _begin: Token,
        end: Token,
    ) {
        self.builder.borrow_mut().stamp_container_decl_end(end);
    }

    fn begin_enum_body(&mut self, token: Token) {
        self.builder.borrow_mut().stamp_container_body_open(token);
    }

    fn end_enum_body(&mut self, _begin: Token, end: Token) {
        self.builder.borrow_mut().stamp_container_decl_end(end);
    }

    fn handle_no_class_body(&mut self, semicolon: Token) {
        self.builder
            .borrow_mut()
            .stamp_container_decl_end(semicolon);
    }

    fn handle_no_mixin_body(&mut self, semicolon: Token) {
        self.builder
            .borrow_mut()
            .stamp_container_decl_end(semicolon);
    }

    fn handle_no_extension_body(&mut self, semicolon: Token) {
        self.builder
            .borrow_mut()
            .stamp_container_decl_end(semicolon);
    }

    fn handle_no_extension_type_body(&mut self, semicolon: Token) {
        self.builder
            .borrow_mut()
            .stamp_container_decl_end(semicolon);
    }

    fn handle_no_enum_body(&mut self, semicolon: Token) {
        self.builder
            .borrow_mut()
            .stamp_container_decl_end(semicolon);
    }

    fn begin_fields(
        &mut self,
        _kind: DeclarationKind,
        _augment_kw: Option<Token>,
        _abstract_kw: Option<Token>,
        _external_kw: Option<Token>,
        _static_kw: Option<Token>,
        _covariant_kw: Option<Token>,
        _late_kw: Option<Token>,
        _var_final_or_const: Option<Token>,
        _last_consumed: Token,
    ) {
        // The group's variables commit at their name tokens; the slot marks
        // where the trailing run to stamp at `end_fields` begins.
        let mut builder = self.builder.borrow_mut();
        builder.field_group = Some(builder.decls.len());
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
        end: Token,
    ) {
        self.builder.borrow_mut().stamp_field_group_end(end);
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
        end: Token,
    ) {
        self.builder.borrow_mut().stamp_field_group_end(end);
    }

    fn handle_class_extends(&mut self, extends: Option<Token>, count: usize) {
        // The event follows the supertype parse (`parse_class_extends_seen`
        // parses the clause's comma-separated types first), and `count` is
        // how many it parsed: under comma recovery (`class C extends B, D`)
        // the clause's first type sits `count - 1` candidates deep, and the
        // official keeps the first (`ast_builder.dart` handleClassExtends
        // pops `typeCount - 1`).
        if extends.is_some() {
            self.builder.borrow_mut().commit_superclass(count);
        }
    }

    fn handle_class_with_clause(&mut self, _with: Token) {
        // The event follows the with list (`parse_type_list` put every
        // mixin name in the candidates); the buffer holds exactly the
        // clause's types — the preceding extends commit cleared it.
        self.builder
            .borrow_mut()
            .commit_clause_supers(SuperClause::With, None);
    }

    fn handle_enum_with_clause(&mut self, _with: Token) {
        self.builder
            .borrow_mut()
            .commit_clause_supers(SuperClause::With, None);
    }

    fn handle_named_mixin_application_with_clause(&mut self, _with: Token) {
        // The head type committed as extends at the list's `begin_type_list`
        // (see `begin_type_list`), so the buffer holds the with-clause
        // mixins alone.
        self.builder
            .borrow_mut()
            .commit_clause_supers(SuperClause::With, None);
    }

    fn handle_implements(&mut self, implements: Option<Token>, count: usize) {
        // The event follows the interface list; `count` is its type count
        // (absent clause: `handle_implements(None, 0)` — nothing parsed,
        // nothing to clear).
        if implements.is_some() {
            self.builder
                .borrow_mut()
                .commit_clause_supers(SuperClause::Implements, Some(count));
        }
    }

    fn handle_mixin_on(&mut self, on: Option<Token>, count: usize) {
        if on.is_some() {
            self.builder
                .borrow_mut()
                .commit_clause_supers(SuperClause::On, Some(count));
        }
    }

    fn begin_type_list(&mut self, _token: Token) {
        // First event of a `,`-separated type list. Only role here: the
        // named mixin application's with list opens right after the head
        // type — commit the armed head before the mixin names join the
        // candidates. Every other type list (class with-clauses, enum
        // with-clauses, recovery) arrives disarmed.
        let mut builder = self.builder.borrow_mut();
        if builder.super_head_armed {
            builder.super_head_armed = false;
            builder.commit_superclass(1);
        }
    }

    fn begin_type_arguments(&mut self, _token: Token) {
        self.builder.borrow_mut().type_arg_depth += 1;
    }

    fn end_type_arguments(&mut self, _count: usize, _begin: Token, _end: Token) {
        let mut builder = self.builder.borrow_mut();
        // Saturate: a bail can leave the bracket unpaired, and the depth is
        // only a guard — an underflow must not kill the capture.
        builder.type_arg_depth = builder.type_arg_depth.saturating_sub(1);
    }

    fn handle_type(&mut self, name: Token, _question: Option<Token>) {
        let mut builder = self.builder.borrow_mut();
        // Type arguments never name the supertype; a qualified type's
        // trailing event carries the prefix token, not the tail already
        // stored by the continuation identifier.
        if builder.type_arg_depth == 0
            && builder.prefixed_type != Some(name.id)
            && let Some(name) = builder.name_of(name)
        {
            builder.super_candidates.push(name);
        }
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
            let mut builder = self.builder.borrow_mut();
            // Supertype name carriers: a qualified `extends p.S` hands over
            // its tail as the continuation identifier (fired immediately
            // before `handle_qualified`), and the prefix token ids the
            // type's trailing `handle_type` so it cannot overwrite the
            // tail.
            match context {
                C::TypeReferenceContinuation => {
                    if let Some(name) = builder.name_of(token) {
                        builder.super_candidates.push(name);
                    }
                }
                C::PrefixedTypeReference => builder.prefixed_type = Some(token.id),
                _ => {}
            }
            builder.site(token, family, kind);
            return;
        }
        let mut builder = self.builder.borrow_mut();
        let Some(name) = builder.name_of(token) else {
            return;
        };
        match context {
            C::TopLevelVariableDeclaration => {
                builder.commit(name, DeclKind::TopLevelVar, token.range, DeclRow::UNKNOWN);
            }
            C::FieldDeclaration | C::EnumValueDeclaration => {
                builder.commit(name, DeclKind::Field, token.range, DeclRow::UNKNOWN);
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
            // The end event (`end_type_variable`) hands no reliable end
            // token — the complex-variables path fires it before the bound
            // parse with the start token — so the row stays honestly
            // `UNKNOWN`.
            builder.commit(name, DeclKind::TypeParam, token.range, DeclRow::UNKNOWN);
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
        end: Token,
    ) {
        let mut builder = self.builder.borrow_mut();
        if let Some(pending) = builder.pending_member.take()
            && let Some((name, kind)) = pending.shape
        {
            // The body parse hands back the declaration's last token (the
            // `}` of a block body, the `;` of a `;`/`=>` body).
            let end = builder.byte_end(end);
            builder.commit(name, kind, pending.range, end);
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

    fn end_top_level_method(&mut self, _begin: Token, get_or_set: Option<Token>, end: Token) {
        let mut builder = self.builder.borrow_mut();
        if let Some(pending) = builder.pending_top.take()
            && let Some((name, range)) = pending.name
        {
            let spelling = match get_or_set.and_then(|t| builder.text_of(t)) {
                Some("set") => builder.setter_spelling(name),
                _ => name,
            };
            let end = builder.byte_end(end);
            builder.commit(spelling, DeclKind::TopLevelFn, range, end);
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
        end: Token,
    ) {
        let mut builder = self.builder.borrow_mut();
        if let Some(pending) = builder.pending_typedef.take()
            && let Some((name, range)) = pending.name
        {
            let end = builder.byte_end(end);
            builder.commit(name, DeclKind::TypeAlias, range, end);
        }
    }

    fn handle_no_arguments(&mut self, token: Token) {
        self.builder.borrow_mut().no_args_at = Some(token.id);
    }

    fn handle_send_without_arguments(&mut self, _begin: Token, end: Token, _next: Token) {
        // The official bare-send arm: fired for exactly the bare shape, and
        // the official parser emits no `handle_no_arguments` for it
        // (`listener.dart` handleSendWithoutArguments). Arming the same
        // marker the no-arguments event arms keeps the refinement correct
        // under either emission contract — a paired `handle_send` ending on
        // this token classifies bare through the marker, and an unpaired
        // firing (dartq's const-pattern path, the official parseSend)
        // leaves the read site untouched.
        self.builder.borrow_mut().no_args_at = Some(end.id);
    }

    fn handle_send(&mut self, begin: Token, end: Token) {
        let mut builder = self.builder.borrow_mut();
        // Bare shape: the send ends exactly on the bare-send marker's token
        // (the official parseArgumentsOpt peeks it and the send returns
        // it). The unpaired firings — enum constants, bare annotations,
        // const patterns — carry other tokens and never match a later end.
        let bare = builder.no_args_at.take() == Some(end.id);
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
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_scanner::ScanConfig;
    use dartq_source::FileTable;
    use std::path::Path;

    fn build(source: &str, strings: &StringTable) -> FileCapture {
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
            "scanner diagnostics: {result:?}"
        );
        let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
            LanguageVersion::new(3, 15),
        ));
        capture(file, source, &result.tokens, features, strings)
    }

    /// [`build`] for sources whose scanner recovery diagnostics are
    /// expected: the bailed-parse fixtures close no braces, and the
    /// scanner's EOF recovery inserts synthetic closers with
    /// `expected_token` diagnostics by design (official
    /// `unmatchedBeginGroup`). What the test asserts is the capture, not
    /// scan cleanliness.
    fn build_recovered(source: &str, strings: &StringTable) -> FileCapture {
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
        let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
            LanguageVersion::new(3, 15),
        ));
        capture(file, source, &result.tokens, features, strings)
    }

    fn id(strings: &StringTable, name: &str) -> StringId {
        strings.intern(name)
    }

    /// The supertype name of `name`'s first row, if an edge anchors to it.
    fn super_edge_for(index: &FileIndex, strings: &StringTable, name: &str) -> Option<StringId> {
        let row = index
            .decls()
            .iter()
            .find(|row| row.name == id(strings, name))
            .unwrap_or_else(|| panic!("{name} row"));
        let position = index
            .decls()
            .iter()
            .position(|candidate| candidate == row)
            .expect("row position");
        index
            .supers()
            .iter()
            .find(|edge| edge.container as usize == position)
            .map(|edge| edge.name)
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

    /// A member of an unnamed extension carries the
    /// [`DeclRow::unnamed_extension_member`] bit: with no container anchor
    /// it would otherwise be indistinguishable from a top-level row and be
    /// admitted as a library face. A same-named top-level row stays a face,
    /// and a named extension's member keeps its anchor.
    #[test]
    fn unnamed_extension_members_are_not_library_faces() {
        let strings = StringTable::new();
        let source = "extension on String { int get ue => 1; } \
             extension Named on String { int get ne => 1; } \
             int ue = 0;";
        let captured = build(source, &strings);
        let index = &captured.index;
        let rows = |name: &str| index.by_name(id(&strings, name)).expect(name);
        let ue_rows = rows("ue");
        assert_eq!(
            ue_rows.len(),
            2,
            "the member and the top-level share a name"
        );
        let member = ue_rows
            .iter()
            .find(|row| row.kind == DeclKind::Getter)
            .expect("member row");
        assert!(member.unnamed_extension_member);
        assert_eq!(member.container, DeclRow::NONE, "no row to anchor to");
        let top = ue_rows
            .iter()
            .find(|row| row.kind == DeclKind::TopLevelVar)
            .expect("top-level row");
        assert!(!top.unnamed_extension_member);
        // The mixed run: the top-level row admits the name, the member does
        // not flip the verdict either way.
        assert!(index.has_library_face(id(&strings, "ue")));
        // A named extension's member keeps its anchor and never was a face.
        let ne = rows("ne")[0];
        assert_ne!(ne.container, DeclRow::NONE);
        assert!(!ne.unnamed_extension_member);
        assert!(!index.has_library_face(id(&strings, "ne")));

        // Without the same-named top-level, the unnamed member alone must
        // not satisfy the face probe — the regression the bit exists for.
        let strings = StringTable::new();
        let captured = build("extension on String { int get ue => 1; }", &strings);
        assert!(captured.index.by_name(id(&strings, "ue")).is_some());
        assert!(!captured.index.has_library_face(id(&strings, "ue")));
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

    /// Superclass name edges across the supertype's shapes: a plain name,
    /// a qualified name (the continuation tail wins, not the prefix), a
    /// generic name (the type argument is not the edge), a named mixin
    /// application's head type — and the clause column: extends edges stay
    /// one per container, a with-clause mixin records a `with` edge (the
    /// enum's with-list included), and a shape with no clause records
    /// nothing.
    #[test]
    fn captures_superclass_name_edges() {
        let strings = StringTable::new();
        let source = "class B {} class Q {} class G {} class H {} mixin M {} \
             enum E with M { x } \
             class C1 extends B {} \
             class C2 extends p.Q {} \
             class C3 extends G<int> {} \
             class C4 = H with M; \
             class C5 {}";
        let captured = build(source, &strings);
        let index = &captured.index;
        let supers_of = |name: &str| -> Vec<(StringId, SuperClause)> {
            let row = index
                .decls()
                .iter()
                .find(|row| row.name == id(&strings, name))
                .unwrap_or_else(|| panic!("{name} row"));
            let position = index
                .decls()
                .iter()
                .position(|candidate| candidate == row)
                .expect("row position");
            index
                .supers()
                .iter()
                .filter(|edge| edge.container as usize == position)
                .map(|edge| (edge.name, edge.clause))
                .collect()
        };
        assert_eq!(
            supers_of("C1"),
            vec![(id(&strings, "B"), SuperClause::Extends)],
            "plain extends"
        );
        assert_eq!(
            supers_of("C2"),
            vec![(id(&strings, "Q"), SuperClause::Extends)],
            "qualified keeps the continuation tail"
        );
        assert_eq!(
            supers_of("C3"),
            vec![(id(&strings, "G"), SuperClause::Extends)],
            "generic: the type argument is not the edge"
        );
        assert_eq!(
            supers_of("C4"),
            vec![
                (id(&strings, "H"), SuperClause::Extends),
                (id(&strings, "M"), SuperClause::With),
            ],
            "mixin application: head type extends, with-list mixins follow"
        );
        assert!(supers_of("C5").is_empty(), "no extends clause, no edge");
        assert_eq!(
            supers_of("E"),
            vec![(id(&strings, "M"), SuperClause::With)],
            "an enum's with-list is a with edge"
        );
        assert_eq!(index.supers().len(), 6, "no edges beyond the six shapes");
    }

    /// All four clause kinds record clause-typed edges, sorted by
    /// `(container, clause)` with the parse order inside one clause: a
    /// class's extends/with/implements chain, a mixin's on/implements
    /// pair, an enum's with/implements pair, and a named mixin
    /// application's head/with/implements trio.
    #[test]
    fn captures_clause_typed_edges_for_all_four_clauses() {
        let strings = StringTable::new();
        let source = "class B {} mixin M1 {} mixin M2 {} class I1 {} class I2 {} class A1 {} \
             class C extends B with M1, M2 implements I1, I2 {} \
             mixin MX on A1 implements I2 {} \
             enum E with M1 implements I1 { x } \
             class N = B with M2 implements I1;";
        let captured = build(source, &strings);
        let index = &captured.index;
        let supers_of = |name: &str| -> Vec<(StringId, SuperClause)> {
            let row = index
                .decls()
                .iter()
                .find(|row| row.name == id(&strings, name))
                .unwrap_or_else(|| panic!("{name} row"));
            let position = index
                .decls()
                .iter()
                .position(|candidate| candidate == row)
                .expect("row position");
            index
                .supers()
                .iter()
                .filter(|edge| edge.container as usize == position)
                .map(|edge| (edge.name, edge.clause))
                .collect()
        };
        assert_eq!(
            supers_of("C"),
            vec![
                (id(&strings, "B"), SuperClause::Extends),
                (id(&strings, "M1"), SuperClause::With),
                (id(&strings, "M2"), SuperClause::With),
                (id(&strings, "I1"), SuperClause::Implements),
                (id(&strings, "I2"), SuperClause::Implements),
            ],
            "the class carries all three class clauses, list order kept"
        );
        assert_eq!(
            supers_of("MX"),
            vec![
                (id(&strings, "A1"), SuperClause::On),
                (id(&strings, "I2"), SuperClause::Implements),
            ],
            "the mixin carries its on constraint and interfaces"
        );
        assert_eq!(
            supers_of("E"),
            vec![
                (id(&strings, "M1"), SuperClause::With),
                (id(&strings, "I1"), SuperClause::Implements),
            ],
            "the enum carries its with-list and interfaces"
        );
        assert_eq!(
            supers_of("N"),
            vec![
                (id(&strings, "B"), SuperClause::Extends),
                (id(&strings, "M2"), SuperClause::With),
                (id(&strings, "I1"), SuperClause::Implements),
            ],
            "the named mixin application splits head (extends) from mixins (with)"
        );
    }

    /// Body and member-signature type names share the candidate buffer
    /// with clause targets but never fabricate edges: only clause
    /// completion events commit, and the container close clears the rest —
    /// a member/field/param/local named like a real supertype yields no
    /// edge, and a type-argument name is guarded as before.
    #[test]
    fn body_type_names_do_not_fabricate_edges() {
        let strings = StringTable::new();
        let source = "class B {} class I {} class L {} \
             class D extends B { \
               I f; \
               void m(L x) { \
                 I y; \
                 L z; \
               } \
             } \
             class Plain { B other; List<B> many; }";
        let captured = build(source, &strings);
        let index = &captured.index;
        let supers_of = |name: &str| -> Vec<(StringId, SuperClause)> {
            let row = index
                .decls()
                .iter()
                .find(|row| row.name == id(&strings, name))
                .unwrap_or_else(|| panic!("{name} row"));
            let position = index
                .decls()
                .iter()
                .position(|candidate| candidate == row)
                .expect("row position");
            index
                .supers()
                .iter()
                .filter(|edge| edge.container as usize == position)
                .map(|edge| (edge.name, edge.clause))
                .collect()
        };
        assert_eq!(
            supers_of("D"),
            vec![(id(&strings, "B"), SuperClause::Extends)],
            "same-named body/member types add no edge to D"
        );
        assert!(
            supers_of("Plain").is_empty(),
            "the member's B and List<B> must not shadow into edges"
        );
        assert_eq!(index.supers().len(), 1, "exactly the extends edge");
    }

    /// Comma recovery in an extends clause (the parser reports
    /// `multiple_extends` and parses every type): the clause's FIRST type
    /// commits — the official listener pops `typeCount - 1` and keeps the
    /// first (`ast_builder.dart` handleClassExtends) — across plain,
    /// qualified, and generic heads.
    #[test]
    fn multiple_extends_keeps_the_first_supertype() {
        let strings = StringTable::new();
        let source = "class B {} class D {} class R {} class G {} \
             class C1 extends B, D {} \
             class C2 extends B, D, R {} \
             class C3 extends p.B, D {} \
             class C4 extends G<int>, D {}";
        let captured = build(source, &strings);
        let index = &captured.index;
        assert_eq!(
            super_edge_for(index, &strings, "C1"),
            Some(id(&strings, "B")),
            "two types: the first commits"
        );
        assert_eq!(
            super_edge_for(index, &strings, "C2"),
            Some(id(&strings, "B")),
            "three types: the first commits"
        );
        assert_eq!(
            super_edge_for(index, &strings, "C3"),
            Some(id(&strings, "B")),
            "qualified first type: the continuation tail commits"
        );
        assert_eq!(
            super_edge_for(index, &strings, "C4"),
            Some(id(&strings, "G")),
            "generic first type: the head commits, not the argument"
        );
        assert_eq!(index.supers().len(), 4, "one edge per clause");
    }

    /// Duplicate-extends recovery fires the clause completion twice for one
    /// container; the container commits at most one edge, and the first
    /// clause's type survives.
    #[test]
    fn duplicate_extends_commits_one_edge() {
        let strings = StringTable::new();
        let source = "class B {} class D {} class C extends B extends D {}";
        let captured = build(source, &strings);
        let index = &captured.index;
        assert_eq!(
            super_edge_for(index, &strings, "C"),
            Some(id(&strings, "B")),
            "the first clause's type wins"
        );
        assert_eq!(index.supers().len(), 1, "one container, one edge");
    }

    /// An extension type's recovery `extends` (the parser reports
    /// `extension_type_extends`) is not a supertype: the extension type
    /// commits no edge.
    #[test]
    fn extension_type_extends_commits_no_edge() {
        let strings = StringTable::new();
        let source = "class Foo {} extension type E(int x) extends Foo {}";
        let captured = build(source, &strings);
        let index = &captured.index;
        assert!(
            index.by_name(id(&strings, "E")).is_some(),
            "the extension type row is captured"
        );
        assert!(index.supers().is_empty(), "no edge for the extension type");
    }

    /// The positional columns on bodied shapes: container rows carry the
    /// exact body-open (one past `{`) and declaration end (one past the
    /// closing `}`), members carry their own closing token's end, a field
    /// group stamps every variable of the group, and the enum constant
    /// stays `UNKNOWN` (no per-element end event). ASCII sources, so the
    /// byte offsets read directly off the literal.
    #[test]
    fn positional_columns_pin_bodied_declarations() {
        let strings = StringTable::new();
        let source = "class C { void m() {} int get g => 1; int a = 0, b = 1; } \
             enum E { x }";
        let captured = build(source, &strings);
        let index = &captured.index;
        let row = |name: &str| index.by_name(id(&strings, name)).expect(name)[0];

        let c = row("C");
        assert_eq!(c.body_open as usize, source.find('{').unwrap() + 1);
        let c_end = source.find("int a = 0, b = 1; }").unwrap() + "int a = 0, b = 1; }".len();
        assert_eq!(c.decl_end as usize, c_end);

        let m = row("m");
        let m_end = source.find("void m() {}").unwrap() + "void m() {}".len();
        assert_eq!(m.decl_end as usize, m_end);
        assert_eq!(m.body_open, DeclRow::UNKNOWN);
        let g = row("g");
        let g_end = source.find("int get g => 1;").unwrap() + "int get g => 1;".len();
        assert_eq!(g.decl_end as usize, g_end);
        assert_eq!(g.body_open, DeclRow::UNKNOWN);

        let group_end = source.find("int a = 0, b = 1;").unwrap() + "int a = 0, b = 1;".len();
        assert_eq!(row("a").decl_end as usize, group_end);
        assert_eq!(row("b").decl_end as usize, group_end);

        let e = row("E");
        assert_eq!(
            e.body_open as usize,
            source.find("enum E {").unwrap() + "enum E {".len()
        );
        assert_eq!(e.decl_end as usize, source.len());
        let x = row("x");
        assert_eq!(x.decl_end, DeclRow::UNKNOWN);
        assert_eq!(x.body_open, DeclRow::UNKNOWN);
    }

    /// The `UNKNOWN` round-trip: shapes the parse cannot close carry the
    /// sentinel on both columns — never a plausible offset — while every
    /// shape the parse does close carries its exact end: abstract/external
    /// members at their `;`, the named mixin application at its `;`, the
    /// bodyless mixin at its braces.
    #[test]
    fn positional_columns_unknown_round_trip() {
        let strings = StringTable::new();
        let source = "class B<T> {} \
             class C { external void f(); abstract int g(); } \
             mixin M {} \
             class D = A with B; \
             enum Q { y }";
        let captured = build(source, &strings);
        let index = &captured.index;
        let row = |name: &str| index.by_name(id(&strings, name)).expect(name)[0];

        // Type parameter: `end_type_variable` carries no reliable end.
        let t = row("T");
        assert_eq!(t.decl_end, DeclRow::UNKNOWN);
        assert_eq!(t.body_open, DeclRow::UNKNOWN);

        // Abstract/external members: the `;` closes them exactly; no body.
        let f = row("f");
        let f_end = source.find("external void f();").unwrap() + "external void f();".len();
        assert_eq!(f.decl_end as usize, f_end);
        assert_eq!(f.body_open, DeclRow::UNKNOWN);
        let g = row("g");
        let g_end = source.find("abstract int g();").unwrap() + "abstract int g();".len();
        assert_eq!(g.decl_end as usize, g_end);
        assert_eq!(g.body_open, DeclRow::UNKNOWN);

        // Bodyless mixin body events: exact braces.
        let m = row("M");
        assert_eq!(
            m.body_open as usize,
            source.find("mixin M {").unwrap() + "mixin M {".len()
        );
        assert_eq!(
            m.decl_end as usize,
            source.find("mixin M {}").unwrap() + "mixin M {}".len()
        );

        // Named mixin application: `;`-closed container, no body to open.
        let d = row("D");
        let d_end = source.find("class D = A with B;").unwrap() + "class D = A with B;".len();
        assert_eq!(d.decl_end as usize, d_end);
        assert_eq!(d.body_open, DeclRow::UNKNOWN);

        // Enum constants: unknowns round-trip the (name, offset) sort.
        let y = row("y");
        assert_eq!(y.decl_end, DeclRow::UNKNOWN);
        assert_eq!(y.body_open, DeclRow::UNKNOWN);
    }

    /// The positional columns are byte offsets, not UTF-16: a prefix
    /// carrying a two-byte char (π) and an astral emoji (a surrogate pair
    /// in UTF-16) shifts every byte position past its UTF-16 counterpart,
    /// and every stamp pins the byte end off the token table's byte face —
    /// the top-level group's `;`, the container's braces, and the member
    /// group's `;`.
    #[test]
    fn positional_columns_are_byte_offsets_for_non_ascii_sources() {
        let strings = StringTable::new();
        // π is 2 bytes in UTF-8 (1 UTF-16 unit) and the emoji is 4 bytes
        // (2 UTF-16 units): the declaration positions below differ by 3
        // between the two units, so a UTF-16 stamp cannot pass.
        let source = "final s = 'π😀';\nclass C { int f = 0; }";
        let captured = build(source, &strings);
        let index = &captured.index;
        let row = |name: &str| index.by_name(id(&strings, name)).expect(name)[0];

        // Byte-layout pins: the assertions read the literal's bytes, not
        // character counts.
        let bytes = source.as_bytes();
        assert_eq!(&bytes[18..19], b";");
        assert_eq!(&bytes[19..20], b"\n");
        assert_eq!(&bytes[20..29], b"class C {");
        assert_eq!(&bytes[30..40], b"int f = 0;");
        assert_eq!(&bytes[41..42], b"}");
        assert_eq!(source.len(), 42);

        // Top-level field group: the `;` after the emoji string, at its
        // byte end (its UTF-16 end would be 16).
        assert_eq!(row("s").decl_end as usize, 19);
        // Container braces at their byte ends (UTF-16 would say 26 and 39).
        let c = row("C");
        assert_eq!(c.body_open as usize, 29);
        assert_eq!(c.decl_end as usize, 42);
        // Member field group: byte end of `int f = 0;` (UTF-16 would say
        // 37).
        assert_eq!(row("f").decl_end as usize, 40);
    }

    /// A generic-function-type field's own type parameters
    /// (`int Function<T>(T) f;`) commit their rows between the field
    /// group's start and the name commit; the group's end stamp skips them
    /// — type parameters stay `UNKNOWN`, never the field's `;` end — while
    /// the field itself carries the group's end exactly.
    #[test]
    fn field_group_end_skips_generic_function_type_parameters() {
        let strings = StringTable::new();
        let source = "class C { int Function<T>(T) f; }";
        let captured = build(source, &strings);
        let index = &captured.index;
        let row = |name: &str| index.by_name(id(&strings, name)).expect(name)[0];

        let t = row("T");
        assert_eq!(t.kind, DeclKind::TypeParam);
        assert_eq!(t.decl_end, DeclRow::UNKNOWN, "no plausible guess for T");
        assert_eq!(t.body_open, DeclRow::UNKNOWN);
        let f = row("f");
        assert_eq!(f.kind, DeclKind::Field);
        let f_end = source.find("int Function<T>(T) f;").unwrap() + "int Function<T>(T) f;".len();
        assert_eq!(f.decl_end as usize, f_end);
    }

    /// Unpaired `handle_no_arguments` firings — enum constants and bare
    /// annotations (the const-pattern path fires the same event) — must not
    /// turn later genuine invocations into bare reads: a send is bare only
    /// when it ends on the no-arguments token itself.
    #[test]
    fn unpaired_no_arguments_leave_later_sends_intact() {
        let strings = StringTable::new();
        let source = "enum E { one } \
             @deprecated \
             class C {} \
             void f() {} \
             void main() { \
               f(); \
               g; \
             }";
        let captured = build(source, &strings);
        let sites = &captured.sites;
        let kind_of = |name: &str| {
            let needle = id(&strings, name);
            let i = sites
                .names
                .iter()
                .position(|&n| n == needle)
                .unwrap_or_else(|| panic!("site {name}"));
            sites.kind(i)
        };
        assert_eq!(
            kind_of("f"),
            RefKind::Invoked,
            "f() after two unpaired firings still invokes"
        );
        assert_eq!(
            kind_of("g"),
            RefKind::Read,
            "a genuine bare send stays a read"
        );
    }

    /// `handle_send_without_arguments` arms the same bare-send marker the
    /// no-arguments event arms: a send paired with it stays a read, so the
    /// refinement holds under the official emission contract too (the
    /// official fires the event for exactly the bare shape and no
    /// `handle_no_arguments`). The control send — no marker — still
    /// invokes.
    #[test]
    fn send_without_arguments_arms_the_bare_marker() {
        let strings = StringTable::new();
        let source = "y x";
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
        let span_of = |lexeme: &str| {
            let i = (0..result.tokens.len())
                .find(|&i| result.tokens.lexeme(source, i) == lexeme)
                .unwrap_or_else(|| panic!("token {lexeme}"));
            let token = result.tokens.get(i).expect("in range");
            Token {
                id: u32::try_from(i).expect("token index fits u32"),
                range: dartq_source::SourceRange {
                    offset: token.utf16_offset,
                    length: token.byte_len,
                },
            }
        };
        let builder = Rc::new(RefCell::new(Builder::new(
            file,
            source,
            &result.tokens,
            &strings,
        )));
        let mut capturer = Capturer {
            builder: Rc::clone(&builder),
        };
        let (y, x) = (span_of("y"), span_of("x"));
        capturer.handle_identifier(y, IdentifierContext::Expression);
        capturer.handle_send_without_arguments(y, y, x);
        capturer.handle_send(y, y);
        capturer.handle_identifier(x, IdentifierContext::Expression);
        capturer.handle_send(x, x);
        drop(capturer);
        let sites = Rc::into_inner(builder)
            .expect("the listener released the builder")
            .into_inner()
            .sites;
        assert_eq!(sites.kind(0), RefKind::Read, "the armed send stays a read");
        assert_eq!(
            sites.kind(1),
            RefKind::Invoked,
            "the unmarked control invokes"
        );
    }

    /// A bailed parse still surfaces what was captured (under-report bias),
    /// and the container-stack flush keeps anchors consistent.
    #[test]
    fn bailed_parse_flushes_open_containers() {
        let strings = StringTable::new();
        // `class B {` never closes; the member must still surface.
        let captured = build_recovered("class B { void m() {}", &strings);
        assert!(captured.index.by_name(id(&strings, "B")).is_some());
        assert!(captured.index.by_name(id(&strings, "m")).is_some());
    }
}
