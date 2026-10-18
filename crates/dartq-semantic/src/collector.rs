//! The collecting listener: drives the parser's [`ParserListener`] event
//! stream into a [`FileElements`] model (the `src/tests/collector.rs`
//! collecting-listener precedent, applied to the declaration surface).
//!
//! # Capture mechanics
//!
//! The parser takes its listener by value and hands back nothing, so
//! [`collect`] keeps the collector in its own scope and lends it to the
//! parse through the multiplexer: the builder is a plain owned field, every
//! event pays no share-count or borrow-flag check at all. Capture keys off
//! declaration-carrying events only — expression
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
//!   `parse_type`, so the arm never leaks across declarations. The
//!   written-type capture (#728) rides the same discipline: an annotation
//!   site's argument group feeds a `TreeSink`, the completing head
//!   finalizes a single-root tree beside the armed type, and the
//!   consuming declaration (`take_armed`) interns it bottom-up through
//!   [`FileElements::type_args_mut`] — the pool's only intern entry, so
//!   the pools stay capture-immutable after `collect`. Poisoned shapes
//!   (function/record types inside the tree) and group-less heads stay
//!   argument-less.
//! * Declaration-arity violations (operator parameter counts, setter
//!   parameter shape) are logged at commit into
//!   [`FileElements::arity_violations`]; hash-map order must not reach the
//!   report.
//!
//! Recovery-synthetic identifiers (parser-inserted token ids past the token
//! table) carry no honest text; their names are skipped. Setter names are
//! rewritten to `name=` at capture (one interning per setter — capture-time
//! cost, not lookup-time).

use crate::capture::{TreeSink, TypeTree};
use crate::model::{
    Annotation, ArgListId, ArityDecl, ArityViolation, Container, ContainerKind, DuplicateDecl,
    Entry, ExtensionFace, FileElements, Formal, Formals, MemberEntry, MemberKind, MicroKind,
    NameMapBuilder, RepFormal, TopDecl, Ty,
};
use dartq_parser::{
    DeclarationKind, FormalParameterKind, IdentifierContext, Parser, ParserFeatures, ParserListener,
};
use dartq_parser::{MemberKind as ParserMemberKind, TokenSpan as Token};
use dartq_resolution::{ClassModifiers, DeclKind};
use dartq_scanner::{Keyword, TokenKind, TokenTable};
use dartq_source::{StringId, StringTable};

use dartq_source::SourceRange;

/// The binary/unary operator spellings with a fixed expected parameter count
/// (`error_verifier.dart:5688-5727`): `[]=` wants 2, these want 1, `~`
/// wants 0, and `-` allows 0 or 1 (minus template).
const OPERATORS_WITH_ONE_PARAMETER: [&str; 16] = [
    "<", ">", "<=", ">=", "==", "+", "/", "~/", "*", "%", "|", "^", "&", "<<", ">>", ">>>",
];

/// An open container on the capture stack.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is one declared modifier keyword or one capture bit"
)]
struct OpenContainer {
    /// Lookup spelling with the name token's span (the duplicate log's
    /// location surface; the spelling is what the model keeps).
    name: Option<(StringId, SourceRange)>,
    kind: ContainerKind,
    superclass: Option<StringId>,
    /// The header's `abstract` modifier (class-family only).
    is_abstract: bool,
    /// The header's `base`/`final`/`interface` keywords and the `mixin
    /// class` form (the class-modifier fact set; the resolution graph's
    /// `declare_class` feed and the modifier restriction checks read them).
    is_base: bool,
    is_final: bool,
    is_interface: bool,
    is_mixin_class: bool,
    /// The member name set so far (sorted-insert; sealed into the
    /// finished [`Container::members`] at the close).
    members: NameMapBuilder<MemberEntry>,
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
    /// The named constructor selectors declared so far, and whether a
    /// declared constructor is unnamed (the implicit default joins at the
    /// container's close, where an empty `ctor_named` without the flag
    /// means no constructor was declared at all). `ctor_declared` counts
    /// declarations so a mismatched-name factory (`factory H` in
    /// `class G` — a named constructor registering no selector here)
    /// still kills the implicit default.
    ctor_named: Vec<StringId>,
    ctor_has_unnamed: bool,
    ctor_declared: u32,
    /// The constructors' formal-parameter tables by lookup spelling (the
    /// selector for `A.named`, the container's name for `A()`); flushed
    /// into the finished [`Container::ctor_formals`].
    ctor_formals: NameMapBuilder<Box<Formals>>,
    /// The declaration's metadata star (`crate::model::Annotation`),
    /// drained from the builder's pending buffer at the open.
    annotations: Vec<Annotation>,
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
#[derive(Clone, Copy)]
enum HeaderClause {
    With,
    Implements,
    On,
}

/// The attribution a clause boundary event applies to the buffered window:
/// the names the event claims join the clause list, the rest are residue
/// (`Container::header_residue`) — never consulted as super-faces.
#[derive(Clone, Copy)]
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
    /// The member's own metadata star, claimed at `begin_method` (the star
    /// precedes the begin) so body-local metadata never joins it.
    annotations: Vec<Annotation>,
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
    /// The declaration wrote a `<T>` list (`end_type_variables` latches it
    /// directly — the `type_params` count routes through
    /// [`Builder::attach_type_param_count`], whose `executable_header_done`
    /// gate a preceding typedef's own formal-shape probe already closed).
    saw_type_params: bool,
}

/// The capture state shared between the listener and [`collect`].
#[expect(clippy::struct_excessive_bools)] // latch flags mirror the parser's event stream
struct Builder<'src, 'tab> {
    source: &'src str,
    tokens: &'tab TokenTable,
    strings: &'src StringTable,
    file: FileElements,
    /// Top-level declarations captured so far; sealed into the model's
    /// `FileElements::decls` at [`Builder::finish`].
    decls: NameMapBuilder<TopDecl>,
    /// Named containers closed so far; sealed into the model's
    /// `FileElements::containers` at [`Builder::finish`] (the open
    /// containers live on [`Builder::containers`]).
    named_containers: NameMapBuilder<Container>,
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
    /// The latched on-type was written nullable (`on C?`) — the capture
    /// rides the completing `handle_type` question token.
    extension_on_type_nullable: bool,
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
    /// A factory declaration is open (`begin_factory`..`end_factory`): its
    /// name event is a constructor shape, not a method one.
    factory_ctor_open: bool,
    /// A mismatched factory selector (`factory H` in `class G`) latched at
    /// the name event — `end_factory` registers it unless a `.name`
    /// continuation already did (`factory G.named` keeps only `named`).
    factory_ctor_head: Option<StringId>,
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
    /// The open type-argument group's nesting depth (`begin_type_arguments`
    /// raises, `end_type_arguments` lowers; the parser emits them as
    /// balanced pairs per `<...>` group, nested groups recursing).
    /// Identifiers inside a group are argument positions, never header
    /// clause faces.
    type_arg_depth: u32,
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
    /// The completed written-type tree of the current annotation (the
    /// shared #610 capture, #728): armed beside [`Builder::armed_ty`] at
    /// `handle_type`, consumed with it — the only pool-intern entry.
    armed_tree: Option<TypeTree>,
    /// The open annotation-site capture: one [`TreeSink`] fed by the
    /// type events while an annotation's argument group is open.
    capture: Option<TreeSink>,
    /// The record level's node count at the open field entry — the hole
    /// latch [`TreeSink::record_entry_end`] checks against.
    record_entry_mark: usize,
    /// The type head awaiting its argument group (`List` of `List<int>`),
    /// armed at the `TypeReference` identifier and consumed at
    /// `begin_type_arguments` or the head's own completion.
    pending_type_head: Option<(StringId, SourceRange)>,
    /// The latched declared type of the open field group.
    field_group_ty: Option<Ty>,
    /// The metadata star of the declaration that opens next, in written
    /// order (`crate::model::Annotation`). Sealed at `end_metadata`,
    /// claimed by the declaration it precedes — a container open, a member
    /// begin (`begin_method`), an enum-value name, or a field group latch —
    /// and dropped at every other declaration boundary: annotations bind to
    /// the declaration they directly precede.
    pending_annotations: Vec<Annotation>,
    /// The open field group's metadata star, latched at `begin_fields` (the
    /// group's star precedes the group, not its first name) and copied into
    /// every [`MemberEntry`] the group's names build (`@meta int a, b;`
    /// annotates both). Cleared at the group's end; the top-level-variable
    /// branch never reads it, which is exactly the old drop.
    field_annotations: Vec<Annotation>,
    /// The annotation being parsed (open between `begin_metadata` and
    /// `end_metadata`): the head name latches at `MetadataReference`,
    /// qualified segments join, `end_metadata` seals the entry. `None`
    /// outside a metadata star at top level.
    open_annotation: Option<Annotation>,
}

impl<'src, 'tab> Builder<'src, 'tab> {
    fn new(source: &'src str, tokens: &'tab TokenTable, strings: &'src StringTable) -> Self {
        Self {
            source,
            tokens,
            strings,
            file: FileElements::default(),
            decls: NameMapBuilder::default(),
            named_containers: NameMapBuilder::default(),
            containers: Vec::new(),
            in_class_header: false,
            header_interfaces: Vec::new(),
            header_residue: Vec::new(),
            header_capture_open: false,
            extension_on_type: None,
            extension_on_type_nullable: false,
            field_abstract: false,
            field_external: false,
            field_final: false,
            field_static: None,
            pending_member: None,
            pending_top: None,
            pending_typedef: None,
            ctor_open: None,
            factory_ctor_open: false,
            factory_ctor_head: None,
            pending_ctor_formals: None,
            formal_depth: 0,
            positional_formals: Vec::new(),
            named_formals: Vec::new(),
            type_group_depth: 0,
            type_arg_depth: 0,
            type_var_group: Vec::new(),
            type_param_count: 0,
            header_type_params: 0,
            header_super_open: false,
            super_candidate: None,
            executable_header_done: false,
            armed_ty: None,
            armed_tree: None,
            capture: None,
            record_entry_mark: 0,
            pending_type_head: None,
            field_group_ty: None,
            pending_annotations: Vec::new(),
            field_annotations: Vec::new(),
            open_annotation: None,
        }
    }

    /// The token's name and span, `None` for parser-inserted synthetic tokens
    /// (which are not in the table and carry no honest name).
    fn capture_name(&self, token: Token) -> Option<(StringId, SourceRange)> {
        let range = token.range;
        self.name_of(token).map(|name| (name, range))
    }

    /// The named segment of an open constructor (`A.named` keeps `named`).
    /// A qualified non-constructor name is invalid Dart — ignored. The
    /// `.new` selector IS the unnamed constructor's canonical spelling
    /// (`class C { C.new(); }` keeps `C()` callable; `factory C.new()`
    /// likewise) — it flags the unnamed face instead of registering a
    /// named selector.
    fn note_constructor_continuation(&mut self, name: Option<StringId>) {
        if let Some(capture) = &mut self.ctor_open
            && let Some(name) = name
        {
            capture.has_unnamed = false;
            if self.strings.resolve(name) == "new" {
                capture.has_unnamed = true;
            } else {
                capture.named.push(name);
            }
        }
        // The formal table's lookup key follows the selector: the unnamed
        // face's class-name key (set at `begin_constructor`) is rewritten
        // before any formal can fire (the selector precedes the parameter
        // list in the header). The `.new` selector keeps the class-name
        // key — it names the unnamed constructor, whose creations look up
        // by the class spelling.
        if let Some(name) = name
            && self.strings.resolve(name) != "new"
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
        match self.decls.entry(name) {
            Entry::Occupied(_) => self.record_duplicate(name, range),
            Entry::Vacant(slot) => {
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
    ///
    /// This is the pool's only intern entry: a captured argument tree
    /// interns here, at the moment a declaration consumes the annotation —
    /// never for body noise, never after `collect` returns (the pools are
    /// capture-immutable once [`FileElements`] is `Arc`-shared). The
    /// conversion is bottom-up (model.rs:243-246, `into_ty` the working
    /// reference): each argument subtree becomes a [`Ty`] first, then the
    /// outer list interns. A poisoned tree or a group-less head stays
    /// argument-less — the under-report direction.
    fn take_armed(&mut self) -> Ty {
        let tree = self.armed_tree.take();
        let ty = self.armed_ty.take().unwrap_or(Ty::UNKNOWN);
        match (ty, tree) {
            (Ty::Named { name, nullable, .. }, Some(tree))
                if !tree.poisoned && !tree.args.is_empty() =>
            {
                self.named_from_tree(name, nullable, tree)
            }
            (_, Some(tree)) if tree.record.is_some() && !tree.poisoned => {
                self.record_from_tree(tree)
            }
            (_, Some(tree)) if tree.function.is_some() && !tree.poisoned => {
                self.function_from_tree(tree)
            }
            _ => ty,
        }
    }

    /// Builds `name<...args...>` from a captured tree: the arguments
    /// convert first (left-to-right source order, inner lists interned by
    /// the recursion), then the outer list interns canonically.
    fn named_from_tree(&mut self, name: StringId, nullable: bool, tree: TypeTree) -> Ty {
        let args: Vec<Ty> = tree.args.into_iter().map(|arg| self.arg_ty(arg)).collect();
        self.file.type_args_mut().named(name, nullable, &args)
    }

    /// Builds a record `Ty` from a captured record tree: each field tree
    /// converts through [`Self::arg_ty`], except a comma-hole leaf
    /// (`head == StringId::NONE` — the recovery splices an `InvalidType`
    /// field, upstream `pkg/analyzer`'s record conversion), which rides
    /// the list as [`Ty::Unknown`].
    fn record_from_tree(&mut self, tree: TypeTree) -> Ty {
        let (positional, named) = tree.record.expect("record arm gated");
        let positional: Vec<Ty> = positional
            .into_iter()
            .map(|field| self.record_field_ty(field))
            .collect();
        let named: Vec<(StringId, Ty)> = named
            .into_iter()
            .map(|(name, field)| (name, self.record_field_ty(field)))
            .collect();
        self.file
            .type_args_mut()
            .record(&positional, &named, tree.nullable)
    }

    fn record_field_ty(&mut self, tree: TypeTree) -> Ty {
        if tree.head == StringId::NONE {
            return Ty::Unknown;
        }
        self.arg_ty(tree)
    }

    /// Builds a function `Ty` from a captured function tree: the return
    /// tree converts through [`Self::arg_ty`] (`None` is the bare
    /// `Function(...)` head's implicit `dynamic`), each formal tree the
    /// same way, then the shape interns canonically.
    fn function_from_tree(&mut self, tree: TypeTree) -> Ty {
        let fn_tree = tree.function.expect("function arm gated");
        let ret = fn_tree.ret.map_or(Ty::Dynamic, |ret| self.arg_ty(*ret));
        let positional: Vec<Ty> = fn_tree
            .positional
            .into_iter()
            .map(|formal| self.arg_ty(formal))
            .collect();
        let named: Vec<(StringId, Ty)> = fn_tree
            .named
            .into_iter()
            .map(|(name, formal)| (name, self.arg_ty(formal)))
            .collect();
        self.file.type_args_mut().function(
            fn_tree.type_params,
            ret,
            &positional,
            &named,
            tree.nullable,
        )
    }

    /// One argument subtree. The head canonicalizes exactly like the
    /// annotation head ([`DeclarationCollector::handle_type`]'s match —
    /// `dynamic`/`void`/`Null` keep their dedicated [`Ty`] shapes); a
    /// subtree with arguments recurses, a leaf stays pool-free. A nested
    /// function shape converts through its own arm (a poisoned one
    /// suppresses to [`Ty::Unknown`]).
    fn arg_ty(&mut self, tree: TypeTree) -> Ty {
        if tree.function.is_some() {
            if tree.poisoned {
                return Ty::Unknown;
            }
            return self.function_from_tree(tree);
        }
        match self.strings.resolve(tree.head) {
            "dynamic" => Ty::Dynamic,
            "void" => Ty::Void,
            "Null" => Ty::Null,
            _ => self.named_from_tree(tree.head, tree.nullable, tree),
        }
    }

    /// The field group's declared type: latched at the group's first name,
    /// reused by its remaining names (`int a, b`).
    fn field_group_ty(&mut self) -> Ty {
        if self.field_group_ty.is_none() {
            self.field_group_ty = Some(self.take_armed());
        }
        self.field_group_ty.expect("latched above")
    }

    /// Latches the head identifier of the annotation being parsed (the
    /// star binds to whatever declaration follows, top-level or member —
    /// the claim points decide the face it lands on; the W7 constructor
    /// face rides the diagnoser's own annotation latch).
    fn note_annotation_head(&mut self, name: StringId, range: SourceRange) {
        self.open_annotation = Some(Annotation {
            name,
            range,
            args_text: None,
        });
    }

    /// Joins one qualified segment into the open annotation's name
    /// (`@meta.sealed` interns the dot-joined spelling once; the string
    /// table dedups repeated spellings).
    fn note_annotation_segment(&mut self, name: StringId, range: SourceRange) {
        if let Some(open) = &mut self.open_annotation {
            let joined = format!(
                "{}.{}",
                self.strings.resolve(open.name),
                self.strings.resolve(name)
            );
            open.name = self.strings.intern(&joined);
            open.range.length = range.offset + range.length - open.range.offset;
        }
    }

    /// Seals the open annotation into the pending star, `@` through the
    /// name's last segment, folding the argument group's verbatim source
    /// text when one parsed (the signature face hashes it, so an
    /// argument-only edit flips the L2 gate). The parser fires
    /// `end_metadata` exactly once per `begin_metadata` (linear
    /// `parse_metadata`), so `take` sees the entry when one was captured
    /// and `None` for nameless junk.
    fn seal_annotation(&mut self, at_offset: u32, end: Token) {
        if let Some(mut open) = self.open_annotation.take() {
            open.args_text = self.annotation_args_text(end);
            open.range.length += open.range.offset - at_offset;
            open.range.offset = at_offset;
            self.pending_annotations.push(open);
        }
    }

    /// The interned `(...)` source text of the sealed annotation's
    /// argument group, `None` when the metadata ended without a scanned,
    /// balanced group: no arguments (the parser fired `handle_no_arguments`
    /// and `end` is the unconsumed follower), or recovery-degenerate shapes
    /// (synthetic closers are not in the token table) — the argument-less
    /// direction.
    fn annotation_args_text(&self, end: Token) -> Option<StringId> {
        let close = self.tokens.get(end.id as usize)?;
        if close.kind != TokenKind::CloseParen {
            return None;
        }
        let mut depth = 0u32;
        let mut index = end.id;
        let open = loop {
            let token = self.tokens.get(index as usize)?;
            match token.kind {
                TokenKind::CloseParen => depth += 1,
                TokenKind::OpenParen => {
                    depth -= 1;
                    if depth == 0 {
                        break token;
                    }
                }
                _ => {}
            }
            index = index.checked_sub(1)?;
        };
        Some(
            self.strings
                .intern(&self.source[open.byte_range().start..close.byte_range().end]),
        )
    }

    /// The interned source text from `start`'s beginning through `end`'s
    /// beginning — the default-value slice (`= 3`'s expression as written;
    /// trailing spacing inside the slice is part of the face, the
    /// conservative direction). `None` when either boundary is not in the
    /// token table (a synthetic recovery token carries no honest span) or
    /// recovery inverted the order — a malformed slice must answer
    /// argument-less, never panic the capture.
    fn intern_span_text(&self, start: Token, end: Token) -> Option<StringId> {
        let from = self.tokens.get(start.id as usize)?.byte_range().start;
        let to = self.tokens.get(end.id as usize)?.byte_range().start;
        (from <= to).then(|| self.strings.intern(&self.source[from..to]))
    }

    /// Drops the pending star: a declaration boundary that claims no
    /// annotations (directives, top-level executables, typedefs, member
    /// ends — anything whose star binds to nothing) discards what pended
    /// before it.
    fn drop_pending_annotations(&mut self) {
        self.pending_annotations.clear();
    }

    /// Closes the open annotation capture at the completing head: the
    /// re-fired parameterized head merges into the root (carrying the
    /// trailing `?`), and a single-root site arms the tree beside the
    /// declared type. A completion inside an open argument group (the
    /// inner `T` of `List<T>`) routes into the site and leaves it open —
    /// the site is only claimed when its outermost group has closed. Any
    /// other shape — a leftover root, an ambiguous multi-root site —
    /// drops the capture: the argument-less direction.
    fn complete_type_site(
        &mut self,
        head: StringId,
        nullable: bool,
        range: SourceRange,
    ) -> Option<TypeTree> {
        let mut site = self.capture.take()?;
        site.leaf(head, nullable, range);
        if !site.stack.is_empty() {
            self.capture = Some(site);
            return None;
        }
        let mut roots = site.done;
        if roots.len() == 1 { roots.pop() } else { None }
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
        // The declaration's metadata star closed before this open (the
        // parser fires it first); it belongs to this container.
        let annotations = std::mem::take(&mut self.pending_annotations);
        // The header's type variables closed before this open (official
        // event order); they belong to this container.
        let header_type_params = std::mem::take(&mut self.header_type_params);
        // Class, enum, and mixin headers carry supertype clauses; the
        // window ends at the header-closing event (`handle_class_header` /
        // `handle_enum_header` / `handle_mixin_header`). A named mixin
        // application fires none of those — its buffer commits at the
        // declaration's end instead. Extension headers carry no subtype
        // edges; an extension type's `implements`/`with` names ARE clause
        // faces (its instance-member interface — the representation type
        // itself is not one and stays out through the `formal_depth` gate
        // in `note_header_reference`).
        self.in_class_header = matches!(
            kind,
            ContainerKind::Class
                | ContainerKind::Enum
                | ContainerKind::Mixin
                | ContainerKind::ExtensionType
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
        self.extension_on_type_nullable = false;
        self.containers.push(OpenContainer {
            name,
            kind,
            superclass: None,
            is_abstract: false,
            is_base: false,
            is_final: false,
            is_interface: false,
            is_mixin_class: false,
            members: NameMapBuilder::default(),
            type_params: header_type_params,
            sealed: false,
            with_clause: Vec::new(),
            implements_clause: Vec::new(),
            on_clause: Vec::new(),
            header_residue: Vec::new(),
            primary_formals: Vec::new(),
            enum_constants: Vec::new(),
            ctor_named: Vec::new(),
            ctor_has_unnamed: false,
            ctor_declared: 0,
            ctor_formals: NameMapBuilder::default(),
            annotations,
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
        // A trailing star (recovery residue after the last member) binds to
        // nothing once the body closed — drop it before it could leak into
        // the next container's own star.
        self.drop_pending_annotations();
        self.field_annotations.clear();
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
                on_type_nullable: self.extension_on_type_nullable,
                members: members.finish(),
            });
            return;
        }
        if open.kind == ContainerKind::Extension {
            // A named extension is both an application face and a
            // named container; the two hold the same member set.
            self.file.extension_faces.push(ExtensionFace {
                name: open.name.map(|(name, _)| name),
                on_type: extension_on_type,
                on_type_nullable: self.extension_on_type_nullable,
                members: members.clone().finish(),
            });
        }
        if let Some((name, _range)) = open.name {
            self.file.container_order.push(name);
            // The unnamed face: an unnamed constructor was declared, or
            // none was at all — the implicit default stands (a named-only
            // declaration kills it). Read before `ctor_named` moves into
            // `constructors`.
            let has_unnamed_constructor =
                open.ctor_has_unnamed || (open.ctor_named.is_empty() && open.ctor_declared == 0);
            self.named_containers.or_insert_with(name, || Container {
                kind: open.kind,
                is_abstract: open.is_abstract,
                is_base: open.is_base,
                is_final: open.is_final,
                is_interface: open.is_interface,
                is_mixin_class: open.is_mixin_class,
                superclass: open.superclass,
                members: members.finish(),
                type_params: open.type_params,
                sealed: open.sealed,
                with_clause: open.with_clause,
                implements_clause: open.implements_clause,
                on_clause: open.on_clause,
                header_residue: open.header_residue,
                primary_formals: open.primary_formals.into_boxed_slice(),
                enum_constants: open.enum_constants,
                constructors: open.ctor_named.into_boxed_slice(),
                has_unnamed_constructor,
                ctor_formals: open.ctor_formals.finish(),
                annotations: open.annotations.into_boxed_slice(),
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

    /// Marks one of the open container's declared class-modifier facts (the
    /// header keyword events: `base`/`final`/`interface` and the `mixin
    /// class` form).
    fn mark_modifier(&mut self, set: impl FnOnce(&mut OpenContainer)) {
        if let Some(open) = self.containers.last_mut() {
            set(open);
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

    /// Routes a class-header type reference: the type's name is the plain
    /// identifier or the continuation of a qualified `p.T` (the prefix
    /// token itself fires `PrefixedTypeReference`, which is not routed
    /// here). The extends head takes the still-open super slot (`extends
    /// p.S` keeps `S`); every later header name — a with/implements/on
    /// clause face, including a named mixin application's `with p.M` after
    /// the head latched — buffers toward the clause events. Inside an
    /// extension header window, the first plain reference latches the `on`
    /// type and the qualified continuation overwrites it (`on p.T` keeps
    /// `T`, the model's `p.S` convention). Identifiers inside a clause
    /// type's `<...>` argument list are argument positions and never route
    /// (the `type_arg_depth` gate).
    fn note_header_reference(
        &mut self,
        context: IdentifierContext,
        name: Option<StringId>,
        offset: u32,
    ) {
        // Argument-position identifiers (`List<int>`'s `int`, `Map<K, V>`'s
        // entries at any nesting depth) fire `TypeReference` like a clause
        // face, but they are parts of the enclosing type — never clause
        // entries and never an on-name. The parser delivers the groups as
        // balanced `begin_type_arguments`/`end_type_arguments` event pairs
        // (a merged `>>` closer splits one `>` per group), so the depth is
        // exact.
        if self.type_arg_depth > 0 {
            return;
        }
        // An extension type's representation formal parses its type inside
        // the primary-constructor `()` — `formal_depth` covers it, so the
        // rep type never lands in the clause buffer; only the `implements`/
        // `with` names after the group are clause entries.
        if self.formal_depth > 0 {
            return;
        }
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
            IdentifierContext::TypeReference | IdentifierContext::TypeReferenceContinuation => {
                if self.header_super_open && self.super_candidate.is_none() {
                    self.super_candidate = name;
                } else if self.in_class_header {
                    self.buffer_header_interface(name, offset);
                }
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
    /// parameter shape is wrong (`error_verifier.dart:5688-5740` for the
    /// operator count table). Setters are absent by design: the parse tier
    /// owns that face outright (the `_ensureSetterFormalParameter` mirror,
    /// upstream Dart SDK source, pkg/analyzer/lib/src/fasta/ast_builder.dart
    /// :6464-6495 — the parser reports it at the setter name), and a second
    /// capture-time log row would double it in the merged report.
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
        let file = self.file;
        FileElements {
            decls: self.decls.finish(),
            containers: self.named_containers.finish(),
            container_order: file.container_order,
            extension_faces: file.extension_faces,
            duplicates: file.duplicates,
            arity_violations: file.arity_violations,
            type_args: file.type_args,
        }
    }
}

/// The [`ParserListener`] implementation feeding a `Builder`: driven
/// through the parse multiplexer — standalone in [`collect`], or fanned
/// out with the engine's other apply-side consumers — and finished into
/// the element model once the parse ends.
pub struct DeclarationCollector<'src, 'tab> {
    builder: Builder<'src, 'tab>,
}

impl<'src, 'tab> DeclarationCollector<'src, 'tab> {
    /// Builds the collector over the `source`/`tokens` pair the parser
    /// reads, interning every declaration name into `strings`.
    pub fn new(source: &'src str, tokens: &'tab TokenTable, strings: &'src StringTable) -> Self {
        Self {
            builder: Builder::new(source, tokens, strings),
        }
    }

    /// Reclaims the element model. The multiplexer only borrows the
    /// collector, so [`collect`] and the engine's fan-out both call this
    /// on a listener whose parse has fully ended — the builder hands over
    /// by value.
    #[must_use]
    pub fn finish(self) -> FileElements {
        self.builder.finish()
    }
}

/// Parses one compilation unit and returns what it declares.
///
/// `tokens` must be the scan of `source` (the parser reads lexemes through
/// the same pair); `strings` receives every interned declaration name and
/// must be the table the caller resolves ids against afterwards.
pub fn collect(
    source: &str,
    tokens: &TokenTable,
    features: ParserFeatures,
    strings: &StringTable,
) -> FileElements {
    let mut collector = DeclarationCollector::new(source, tokens, strings);
    Parser::new(source, tokens, features, &mut collector).parse_unit();
    collector.finish()
}

/// The declared class-modifier facts of one container — the resolution
/// graph's `declare_class` feed shape (`dartq_resolution::modifiers`). The
/// kind mapping is the official element-model split: enums carry no
/// modifier face at all, mixins carry only `base`, and a `mixin class` is
/// [`DeclKind::Class`] with the flag set.
#[must_use]
pub fn container_modifiers(container: &Container) -> ClassModifiers {
    ClassModifiers {
        kind: match container.kind {
            // Extensions and extension types are not class-modifier
            // carriers; the flags are always false on them.
            ContainerKind::Class | ContainerKind::Extension | ContainerKind::ExtensionType => {
                DeclKind::Class
            }
            ContainerKind::Mixin => DeclKind::Mixin,
            ContainerKind::Enum => DeclKind::Enum,
        },
        is_abstract: container.is_abstract,
        is_base: container.is_base,
        is_final: container.is_final,
        is_interface: container.is_interface,
        is_sealed: container.sealed,
        is_mixin_class: container.is_mixin_class,
    }
}

impl ParserListener for DeclarationCollector<'_, '_> {
    // Directive boundaries: an annotated directive (`@Deprecated import ...`)
    // fires its metadata first and no declaration event after — its
    // annotations bind to the directive and drop here.
    fn begin_import(&mut self, _token: Token) {
        self.builder.drop_pending_annotations();
    }

    fn begin_export(&mut self, _token: Token) {
        self.builder.drop_pending_annotations();
    }

    fn begin_part(&mut self, _token: Token) {
        self.builder.drop_pending_annotations();
    }

    fn begin_library_name(&mut self, _token: Token) {
        self.builder.drop_pending_annotations();
    }

    fn begin_library_augmentation(&mut self, _augment_kw: Token, _library_keyword: Token) {
        self.builder.drop_pending_annotations();
    }

    fn begin_class_declaration(
        &mut self,
        _begin: Token,
        abstract_kw: Option<Token>,
        sealed_kw: Option<Token>,
        base_kw: Option<Token>,
        interface_kw: Option<Token>,
        final_kw: Option<Token>,
        _augment_kw: Option<Token>,
        mixin_kw: Option<Token>,
        name: Token,
    ) {
        let sealed = sealed_kw.is_some();
        let builder = &mut self.builder;
        let name = builder.capture_name(name);
        builder.open_container(ContainerKind::Class, name, MicroKind::Class);
        if sealed {
            builder.mark_sealed();
        }
        if abstract_kw.is_some() {
            builder.mark_abstract();
        }
        builder.mark_modifier(|open| {
            open.is_base = base_kw.is_some();
            open.is_interface = interface_kw.is_some();
            open.is_final = final_kw.is_some();
            open.is_mixin_class = mixin_kw.is_some();
        });
    }

    fn handle_class_header(&mut self, _begin: Token, _class: Token, _native: Option<Token>) {
        self.builder.end_header_window();
    }

    /// An unannotated declaration boundary: the star carried no
    /// annotation, so anything still pending binds to nothing and drops
    /// (recovery residue between declarations included).
    fn end_metadata_star(&mut self, count: usize) {
        if count == 0 {
            self.builder.drop_pending_annotations();
        }
    }

    /// Seals the open annotation: the span widens back to the `@` sign
    /// (`begin`), the official annotation-anchored report position; the
    /// argument group's text rides along when the group parsed (`end`).
    fn end_metadata(&mut self, begin: Token, _period_before_name: Option<Token>, end: Token) {
        let at_offset = begin.range.offset;
        self.builder.seal_annotation(at_offset, end);
    }

    fn end_class_declaration(&mut self, _begin: Token, _end: Token) {
        self.builder.close_container();
    }

    fn begin_named_mixin_application(
        &mut self,
        _begin: Token,
        abstract_kw: Option<Token>,
        sealed_kw: Option<Token>,
        base_kw: Option<Token>,
        interface_kw: Option<Token>,
        final_kw: Option<Token>,
        _augment_kw: Option<Token>,
        _mixin_kw: Option<Token>,
        name: Token,
    ) {
        let builder = &mut self.builder;
        let name = builder.capture_name(name);
        builder.open_container(ContainerKind::Class, name, MicroKind::Class);
        if abstract_kw.is_some() {
            builder.mark_abstract();
        }
        builder.mark_modifier(|open| {
            open.is_base = base_kw.is_some();
            open.is_interface = interface_kw.is_some();
            open.is_final = final_kw.is_some();
            open.sealed = sealed_kw.is_some();
        });
    }

    fn end_named_mixin_application(
        &mut self,
        _begin: Token,
        _class: Token,
        _equals: Token,
        _implements: Option<Token>,
        _end: Token,
    ) {
        let builder = &mut self.builder;
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
        base_kw: Option<Token>,
        mixin: Token,
        name: Token,
    ) {
        let builder = &mut self.builder;
        let name = builder.capture_name(name);
        builder.open_container(ContainerKind::Mixin, name, MicroKind::Mixin);
        // `base` is the only legal mixin modifier, and the plain-`mixin`
        // dispatch consumes it in the walk lookahead without forwarding it
        // (the head reaches this event modifierless), so recover it from the
        // token stream: `base` sits immediately before the `mixin` keyword
        // exactly when the modifier row wrote it — no other token can stand
        // between a declaration's modifier and its head keyword. The `mixin`
        // keyword is always a scanned token (the dispatch cursor sits on
        // it), so `id - 1` addresses its real predecessor.
        let wrote_base = base_kw.is_some()
            || mixin
                .id
                .checked_sub(1)
                .and_then(|previous| builder.tokens.get(previous as usize))
                .is_some_and(|token| token.kind == TokenKind::Keyword(Keyword::Base));
        builder.mark_modifier(|open| open.is_base = wrote_base);
    }

    fn end_mixin_declaration(&mut self, _begin: Token, _end: Token) {
        self.builder.close_container();
    }

    fn begin_enum_declaration(
        &mut self,
        _begin: Token,
        _augment_kw: Option<Token>,
        _enum_keyword: Token,
        name: Token,
    ) {
        let builder = &mut self.builder;
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
        self.builder.close_container();
    }

    fn handle_enum_header(
        &mut self,
        _augment: Option<Token>,
        _enum_keyword: Token,
        _left_brace: Token,
    ) {
        self.builder.end_header_window();
    }

    fn handle_mixin_header(&mut self, _mixin: Token) {
        self.builder.end_header_window();
    }

    fn begin_extension_declaration(
        &mut self,
        _augment: Option<Token>,
        _extension_keyword: Token,
        name: Option<Token>,
    ) {
        let builder = &mut self.builder;
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
        self.builder.close_container();
    }

    fn begin_extension_type_declaration(
        &mut self,
        _augment_kw: Option<Token>,
        _extension_keyword: Token,
        name: Token,
    ) {
        let builder = &mut self.builder;
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
        self.builder.close_container();
    }

    fn handle_class_extends(&mut self, _extends: Option<Token>, _count: usize) {
        // The supertype's type events fired before this event (official
        // `parseClassExtendsSeenExtendsClause`), so the buffered header
        // reference — present only with `extends` — is the edge.
        self.builder.commit_super_candidate();
    }

    fn handle_named_mixin_application_with_clause(&mut self, with: Token) {
        // The head type fired between `begin_named_mixin_application` and
        // this event; the `with` list itself is not the superclass. Names
        // written before the `with` keyword are head-overflow residue.
        let offset = with.range.offset;
        let builder = &mut self.builder;
        builder.commit_super_candidate();
        builder.commit_header_clause(HeaderClause::With, ClauseClaim::Keyword(offset));
    }

    fn handle_class_with_clause(&mut self, with_keyword: Token) {
        let offset = with_keyword.range.offset;
        self.builder
            .commit_header_clause(HeaderClause::With, ClauseClaim::Keyword(offset));
    }

    fn handle_enum_with_clause(&mut self, with_keyword: Token) {
        let offset = with_keyword.range.offset;
        self.builder
            .commit_header_clause(HeaderClause::With, ClauseClaim::Keyword(offset));
    }

    fn handle_mixin_on(&mut self, _on: Option<Token>, count: usize) {
        self.builder
            .commit_header_clause(HeaderClause::On, ClauseClaim::Trailing(count));
    }

    fn handle_implements(&mut self, _implements: Option<Token>, count: usize) {
        self.builder
            .commit_header_clause(HeaderClause::Implements, ClauseClaim::Trailing(count));
    }

    fn begin_class_or_mixin_or_extension_body(&mut self, kind: DeclarationKind, _token: Token) {
        let builder = &mut self.builder;
        // The header window closes with the body: extension on-types and
        // extension-type representation formals stop being captured. The
        // kind is informational here — the container's own kind decides.
        let _ = kind;
        builder.header_capture_open = false;
    }

    fn handle_no_class_body(&mut self, _semicolon: Token) {
        self.builder.header_capture_open = false;
    }

    fn handle_no_mixin_body(&mut self, _semicolon: Token) {
        self.builder.header_capture_open = false;
    }

    fn handle_no_enum_body(&mut self, _semicolon: Token) {
        self.builder.header_capture_open = false;
    }

    fn handle_no_extension_body(&mut self, _semicolon: Token) {
        self.builder.header_capture_open = false;
    }

    fn handle_no_extension_type_body(&mut self, _semicolon: Token) {
        self.builder.header_capture_open = false;
    }

    fn handle_type(&mut self, name: Token, question: Option<Token>) {
        let builder = &mut self.builder;
        // Built-in spellings canonicalize exactly like the diagnoser's
        // listener: verdict consumers (`bare_info_of`, the formal tables)
        // would otherwise hand assignability faces an as-written `Named`
        // where the canonical `Ty` is the observable contract (the
        // strict-casts overlay's source/target predicates, the nullability
        // faces). Every other name stays as written.
        let spelling = builder.name_of(name);
        // The head's own group (if any) closed before this completion;
        // the slot is consumed either way.
        builder.pending_type_head = None;
        // The #610 capture, collector-side: the completing head merges
        // into the open site's root (the re-fired parameterized head
        // carries the trailing `?`), and a single-root tree arms beside
        // the type for the consuming declaration to intern.
        builder.armed_tree = spelling.and_then(|spelling| {
            let range = match question {
                Some(q) => SourceRange {
                    offset: name.range.offset,
                    length: q.range.end() - name.range.offset,
                },
                None => name.range,
            };
            builder.complete_type_site(spelling, question.is_some(), range)
        });
        builder.armed_ty = spelling.map(|spelling| match builder.strings.resolve(spelling) {
            "dynamic" => Ty::Dynamic,
            "void" => Ty::Void,
            "Null" => Ty::Null,
            _ => Ty::Named {
                name: spelling,
                nullable: question.is_some(),
                args: ArgListId::EMPTY,
            },
        });
        // The extension on-latch carries the written type's nullability:
        // the header window is still open here (the latch's own
        // `note_header_reference` never sees the `?`), and the completing
        // spelling matching the latched name pins the flag to that type —
        // body members never reach this (the window closed with the body).
        if self.builder.header_capture_open
            && spelling.is_some()
            && self.builder.extension_on_type == spelling
        {
            self.builder.extension_on_type_nullable = question.is_some();
        }
    }
    fn handle_no_type(&mut self, _last_consumed: Token) {
        let builder = &mut self.builder;
        builder.armed_ty = None;
        builder.armed_tree = None;
    }

    fn handle_void_keyword(&mut self, token: Token) {
        let builder = &mut self.builder;
        // A `void` type argument (`List<void>`) is a leaf of the open
        // capture like any named head; outside a capture a stale tree
        // cannot survive the arm.
        let void_id = builder.strings.intern("void");
        if let Some(site) = builder.capture.as_mut() {
            site.leaf(void_id, false, token.range);
        } else {
            builder.armed_tree = None;
        }
        builder.armed_ty = Some(Ty::Void);
    }

    fn handle_void_keyword_with_type_arguments(&mut self, _token: Token) {
        let builder = &mut self.builder;
        builder.armed_tree = None;
        builder.armed_ty = Some(Ty::Void);
    }

    fn begin_type_arguments(&mut self, _token: Token) {
        let builder = &mut self.builder;
        builder.type_arg_depth += 1;
        let head = builder.pending_type_head.take();
        match (&mut builder.capture, head) {
            (Some(site), Some((head, range))) => site.begin_group(Some((head, range))),
            // A headless group inside a capture poisons its parent (the
            // sink's discipline); an expression group (`f<int>()`) is
            // never a site.
            (Some(site), None) => site.begin_group(None),
            (None, Some((head, range))) => {
                let mut site = TreeSink::default();
                site.begin_group(Some((head, range)));
                builder.capture = Some(site);
            }
            (None, None) => {}
        }
    }

    fn end_type_arguments(&mut self, _count: usize, begin: Token, end: Token) {
        let builder = &mut self.builder;
        debug_assert!(
            builder.type_arg_depth > 0,
            "unbalanced type-argument events",
        );
        builder.type_arg_depth -= 1;
        if let Some(site) = builder.capture.as_mut() {
            site.end_group(SourceRange {
                offset: begin.range.offset,
                length: end.range.end() - begin.range.offset,
            });
        }
    }

    fn begin_function_type(&mut self, _begin: Token) {
        // The function-type capture: the shape's columns open their own
        // level (lazily — the written return type and the formal list fold
        // into a first-class `Ty::Function`, not a poison).
        let builder = &mut self.builder;
        builder
            .capture
            .get_or_insert_with(TreeSink::default)
            .begin_function();
    }

    /// A function-type annotation closed: the level seals and the root
    /// arms like a completed record (`take_armed` interns the shape). The
    /// written `?` rides the variant's `nullable`.
    fn end_function_type(&mut self, function: Token, question: Option<Token>) {
        let builder = &mut self.builder;
        let range = SourceRange {
            offset: function.range.offset,
            length: question.map_or(function.range, |q| q.range).end() - function.range.offset,
        };
        if let Some(site) = builder.capture.as_mut() {
            site.end_function(question.is_some(), range);
        }
        // A top-level function annotation arms itself: no `handle_type`
        // head ever completes the whole shape, so the root moves onto the
        // armed slot here — `take_armed` interns it like a completed
        // named tree.
        if let Some(site) = builder.capture.as_mut()
            && site.stack.is_empty()
            && site.done.len() == 1
        {
            let mut site = builder.capture.take().expect("checked above");
            let tree = site.done.pop().expect("one root");
            builder.armed_tree = Some(tree);
            builder.armed_ty = None; // `take_armed` converts the function arm.
        }
    }

    fn begin_record_type(&mut self, _left_paren: Token) {
        let builder = &mut self.builder;
        // The record-type capture: the field list opens its own level
        // (lazily — a bare `(int, int) x;` fires no argument-group event,
        // so the sink starts here), and the completed node feeds the
        // declaration's written type like a named head does (#1511: the
        // record shape is a first-class `Ty`, not a poison).
        builder
            .capture
            .get_or_insert_with(TreeSink::default)
            .begin_record();
    }

    fn end_record_type(
        &mut self,
        left_paren: Token,
        question: Option<Token>,
        _count: usize,
        _has_named_fields: bool,
    ) {
        let builder = &mut self.builder;
        let end = question.map_or(left_paren.range, |q| SourceRange {
            offset: left_paren.range.offset,
            length: q.range.end() - left_paren.range.offset,
        });
        let range = SourceRange {
            offset: left_paren.range.offset,
            length: end.end() - left_paren.range.offset,
        };
        if let Some(site) = builder.capture.as_mut() {
            site.end_record(question.is_some(), range);
        }
        // A top-level record annotation arms itself: no `handle_type` head
        // ever fires for it (the record is the whole type), so the root
        // moves onto the armed slot here — `take_armed` interns it like a
        // completed named tree.
        if let Some(site) = builder.capture.as_mut()
            && site.stack.is_empty()
            && site.done.len() == 1
        {
            let mut site = builder.capture.take().expect("checked above");
            let tree = site.done.pop().expect("one root");
            builder.armed_tree = Some(tree);
            builder.armed_ty = None; // `take_armed` converts the record arm.
        }
    }

    fn begin_record_type_entry(&mut self, _first: Token) {
        let builder = &mut self.builder;
        builder.record_entry_mark = builder
            .capture
            .as_ref()
            .map_or(0, |site| site.record_entry_mark());
    }

    fn end_record_type_entry(&mut self, last: Token) {
        let builder = &mut self.builder;
        if let Some(site) = builder.capture.as_mut() {
            site.record_entry_end(builder.record_entry_mark, last.range);
        }
    }

    fn handle_no_name(&mut self, token: Token) {
        let builder = &mut self.builder;
        if let Some(site) = builder.capture.as_mut() {
            site.record_field_name(None);
        }
        let _ = token;
    }

    fn end_record_type_named_fields(&mut self, count: usize, _left_brace: Token) {
        let builder = &mut self.builder;
        if let Some(site) = builder.capture.as_mut() {
            site.record_named_region(count);
        }
    }

    #[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
    fn handle_identifier(&mut self, token: Token, context: IdentifierContext) {
        let builder = &mut self.builder;
        // Resolve the name and its span up front: the branches below hold
        // borrows into the builder's state.
        let name_with_range = builder.capture_name(token);
        let name = name_with_range.map(|(name, _)| name);
        match context {
            IdentifierContext::TypeReference | IdentifierContext::TypeReferenceContinuation => {
                let offset = name_with_range.map_or(0, |(_, range)| range.offset);
                // The annotation head awaits its argument group (the
                // collector-side #610 capture). A continuation never
                // carries one — a prefixed `p.C` completes its type
                // through the head token's own `handle_type`.
                if context == IdentifierContext::TypeReference {
                    builder.pending_type_head = name_with_range;
                }
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
                // A factory declaration's name is a constructor shape:
                // matching the container it is the unnamed face
                // (`factory G()`); a mismatched selector opens the capture
                // so the `.name` continuation rewrites it (`factory
                // G.named()`), else `end_factory` registers the spelling.
                if builder.factory_ctor_open
                    && let Some((name, _)) = name_with_range
                    && let Some(open) = builder.containers.last()
                {
                    let is_unnamed = open.name.is_some_and(|(container, _)| container == name);
                    builder.ctor_open = Some(CtorCapture {
                        named: Vec::new(),
                        has_unnamed: is_unnamed,
                    });
                    builder.factory_ctor_head = (!is_unnamed).then_some(name);
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
                        // The group's latched star rides every group member
                        // (`@meta int a, b;` annotates both).
                        let annotations = builder.field_annotations.clone().into_boxed_slice();
                        match open.members.entry(name) {
                            Entry::Vacant(slot) => {
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
                                    annotations,
                                    name_range: range,
                                });
                            }
                            // Same-bit collision: the second field is a
                            // duplicate. Keep-first; a cross-bit coexistence
                            // is `conflicting_static_and_instance`'s
                            // business, not a duplicate.
                            Entry::Occupied(existing) => {
                                if existing.is_static == is_static {
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
                    // The constant's own star (each value parses its
                    // metadata right before its name).
                    let annotations =
                        std::mem::take(&mut builder.pending_annotations).into_boxed_slice();
                    match open.members.entry(name) {
                        Entry::Vacant(slot) => {
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
                                annotations,
                                name_range: range,
                            });
                        }
                        Entry::Occupied(existing) => {
                            if existing.is_static {
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
            IdentifierContext::MetadataReference => {
                if let Some((name, range)) = name_with_range {
                    builder.note_annotation_head(name, range);
                }
            }
            IdentifierContext::MetadataContinuation
            | IdentifierContext::MetadataContinuationAfterTypeArguments => {
                if let Some((name, range)) = name_with_range {
                    builder.note_annotation_segment(name, range);
                }
            }
            IdentifierContext::RecordFieldDeclaration => {
                // The field's written name arrives after its type node —
                // the sink attaches it to the last entry (upstream fires
                // `handleIdentifier` under the record-field context, while
                // a missing name arrives via `handleNoName` instead).
                if let Some(site) = builder.capture.as_mut() {
                    site.record_field_name(name);
                }
            }
            _ => {}
        }
    }

    fn begin_type_variables(&mut self, _token: Token) {
        let builder = &mut self.builder;
        builder.type_group_depth += 1;
        if builder.type_group_depth == 1 {
            builder.type_param_count = 0;
        }
        builder.type_var_group.clear();
        if let Some(site) = builder.capture.as_mut() {
            site.type_variables_begin();
        }
    }

    fn begin_type_variable(&mut self, token: Token) {
        let builder = &mut self.builder;
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
                .decls
                .or_insert_with(name, || TopDecl::bare(MicroKind::TypeParam));
            if builder.type_group_depth == 1 {
                builder.type_param_count += 1;
            }
            if let Some(site) = builder.capture.as_mut() {
                site.function_type_variable();
            }
        }
    }

    fn end_type_variables(&mut self, _begin: Token, _end: Token) {
        let builder = &mut self.builder;
        if builder.type_group_depth == 1
            && let Some(pending) = &mut builder.pending_typedef
        {
            pending.saw_type_params = true;
        }
        if builder.type_group_depth == 1 && !builder.attach_type_param_count() {
            // A container header: the count carries into the container's
            // `begin_*_declaration` open (official order — the type
            // variables parse before `beginClassDeclaration`).
            builder.header_type_params = builder.type_param_count;
        }
        if let Some(site) = builder.capture.as_mut() {
            site.type_variables_end();
        }
        builder.type_group_depth -= 1;
    }

    fn begin_fields(
        &mut self,
        _kind: DeclarationKind,
        _augment_kw: Option<Token>,
        abstract_kw: Option<Token>,
        external_kw: Option<Token>,
        static_kw: Option<Token>,
        _covariant_kw: Option<Token>,
        _late_kw: Option<Token>,
        _var_final_or_const: Option<Token>,
        _last_consumed: Token,
    ) {
        let builder = &mut self.builder;
        // The star preceding the group latches here — it precedes the
        // group, not its first name, and every name the group declares
        // carries it (`@meta int a, b;` annotates both). The
        // top-level-variable branch never reads the latch, which is that
        // face's drop.
        builder.field_annotations = std::mem::take(&mut builder.pending_annotations);
        builder.field_static = Some(static_kw.is_some());
        builder.field_abstract = abstract_kw.is_some();
        builder.field_external = external_kw.is_some();
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
        let builder = &mut self.builder;
        builder.field_static = None;
        builder.field_group_ty = None;
        builder.armed_ty = None;
        builder.armed_tree = None;
        builder.field_abstract = false;
        builder.field_external = false;
        builder.field_final = false;
        // The group closed: its star latch clears with it, and anything a
        // closure initializer buffered (local-declaration metadata) binds
        // to nothing.
        builder.field_annotations.clear();
        builder.drop_pending_annotations();
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
        let builder = &mut self.builder;
        builder.field_static = None;
        builder.field_group_ty = None;
        builder.armed_ty = None;
        builder.armed_tree = None;
        builder.field_abstract = false;
        builder.field_external = false;
        builder.field_final = false;
        builder.field_annotations.clear();
        builder.drop_pending_annotations();
    }

    #[allow(clippy::too_many_arguments)]
    fn begin_method(
        &mut self,
        _kind: DeclarationKind,
        _augment_kw: Option<Token>,
        external_kw: Option<Token>,
        static_kw: Option<Token>,
        _covariant_kw: Option<Token>,
        _var_final_or_const: Option<Token>,
        get_or_set: Option<Token>,
        name: Token,
        _enclosing_declaration_name: Option<&str>,
    ) {
        let builder = &mut self.builder;
        let shape = builder
            .capture_name(name)
            .map(|(n, range)| (builder.member_shape(get_or_set, n), range))
            .map(|((spelling, kind), range)| (spelling, kind, range));
        // The star preceding the member belongs to it; body-local metadata
        // (locals, closures) pends after this claim and drops at the end.
        let annotations = std::mem::take(&mut builder.pending_annotations);
        builder.pending_member = Some(PendingMember {
            shape,
            is_static: static_kw.is_some(),
            is_external: external_kw.is_some(),
            has_body: false,
            type_params: 0,
            type_param_names: Vec::new(),
            return_ty: Ty::UNKNOWN,
            formals: None,
            annotations,
        });
        builder.executable_header_done = false;
    }

    /// A block function body opened inside the pending member — the
    /// has-body mark (its absence at `end_method` is the abstract shape).
    /// A closure in a formal default fires this before any real body, which
    /// can only mislabel an abstract member concrete: the under-report
    /// direction.
    fn begin_block_function_body(&mut self, _token: Token) {
        if let Some(pending) = self.builder.pending_member.as_mut() {
            pending.has_body = true;
        }
    }

    fn handle_expression_function_body(&mut self, _arrow: Token, _end: Option<Token>) {
        if let Some(pending) = self.builder.pending_member.as_mut() {
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
        let builder = &mut self.builder;
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
                    annotations: pending.annotations.into_boxed_slice(),
                    name_range: range,
                };
                match open.members.entry(name) {
                    Entry::Vacant(slot) => {
                        slot.insert(entry);
                    }
                    // Same-bit collisions are duplicates; cross-bit ones are not.
                    Entry::Occupied(existing) => {
                        if existing.is_static == pending.is_static {
                            builder.record_duplicate(name, range);
                        }
                    }
                }
            }
        }
        // Anything buffered inside the body (local-declaration metadata)
        // binds to no member — drop it with the executable.
        builder.drop_pending_annotations();
        builder.executable_header_done = false;
    }

    fn begin_top_level_method(
        &mut self,
        _last_consumed: Token,
        _augment: Option<Token>,
        _external: Option<Token>,
    ) {
        let builder = &mut self.builder;
        builder.drop_pending_annotations();
        builder.pending_top = Some(PendingTop {
            name: None,
            type_params: 0,
            return_ty: Ty::UNKNOWN,
            formals: None,
        });
        builder.executable_header_done = false;
    }

    fn end_top_level_method(&mut self, _begin: Token, get_or_set: Option<Token>, _end: Token) {
        let builder = &mut self.builder;
        // Anything buffered inside the body (local-declaration metadata)
        // binds to no container — drop it with the executable.
        builder.drop_pending_annotations();
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
        let builder = &mut self.builder;
        builder.drop_pending_annotations();
        builder.pending_typedef = Some(PendingTypedef {
            name: None,
            type_params: 0,
            saw_type_params: false,
        });
    }

    /// A factory declaration is a constructor declaration for the
    /// constructor-set faces: it kills the implicit default the same way
    /// (`class G { factory H() ... }` leaves `G()` undefined — the oracle's
    /// `new_with_undefined_constructor_default`). The name event captures
    /// the unnamed/named shape; `end_factory` settles the capture.
    fn begin_factory(
        &mut self,
        _kind: DeclarationKind,
        _last_consumed: Token,
        _augment_kw: Option<Token>,
        _external_kw: Option<Token>,
        _const_keyword: Option<Token>,
    ) {
        let builder = &mut self.builder;
        builder.drop_pending_annotations();
        if let Some(open) = builder.containers.last_mut() {
            open.ctor_declared += 1;
        }
        builder.factory_ctor_open = true;
        builder.factory_ctor_head = None;
    }

    fn end_factory(&mut self, _kind: DeclarationKind, _begin: Token, _factory: Token, _end: Token) {
        let builder = &mut self.builder;
        builder.factory_ctor_open = false;
        if let Some(capture) = builder.ctor_open.take()
            && let Some(open) = builder.containers.last_mut()
        {
            let mut named = capture.named;
            if named.is_empty()
                && !capture.has_unnamed
                && let Some(head) = builder.factory_ctor_head.take()
            {
                // `factory H()` — the mismatched selector is itself the
                // constructor's spelling (`G.H()` is callable). A
                // continuation already registered its own selector, so the
                // head fallback only fires on the undotted shape.
                named.push(head);
            }
            for name in named {
                if !open.ctor_named.contains(&name) {
                    open.ctor_named.push(name);
                }
            }
            open.ctor_has_unnamed |= capture.has_unnamed;
        }
        builder.factory_ctor_head = None;
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
        let builder = &mut self.builder;
        // The star preceding the constructor binds to no modeled row (the
        // model keeps no constructor entry) — claim it so it can neither
        // leak into the next member nor into the next container's star.
        builder.drop_pending_annotations();
        // The unnamed shape is the constructor's declared name matching the
        // enclosing class (`A()` — or the head recovered); a mismatched
        // factory selector (`class G { factory H() ... }`) declares a NAMED
        // constructor and kills the implicit default (the oracle reports
        // `new_with_undefined_constructor_default` on `G()`). `A.named`
        // heads still match (the head token is the class name) — the
        // continuation below rewrites them to the selector.
        let declared_unnamed = builder
            .capture_name(_name)
            .is_some_and(|(key, _)| {
                _enclosing_declaration_name
                    .is_some_and(|enclosing| builder.strings.resolve(key) == enclosing)
            });
        // `A()` (or the head recovered): the unnamed shape until a
        // `A.named` continuation says otherwise. The formal table's key is
        // the head token's name — the class name, which is exactly the
        // unnamed face's lookup spelling (`A()` creation rows carry it);
        // the continuation rewrites it to the selector.
        builder.ctor_open = Some(CtorCapture {
            named: Vec::new(),
            has_unnamed: declared_unnamed,
        });
        builder.pending_ctor_formals = builder.capture_name(_name).map(|(key, _)| (key, None));
        // Every declared constructor kills the close's "none at all" fold —
        // including a mismatched-name factory registering no selector.
        if let Some(open) = builder.containers.last_mut() {
            open.ctor_declared += 1;
        }
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
        let builder = &mut self.builder;
        if let Some(capture) = builder.ctor_open.take()
            && let Some(open) = builder.containers.last_mut()
        {
            for name in capture.named {
                if !open.ctor_named.contains(&name) {
                    open.ctor_named.push(name);
                }
            }
            // The unnamed face stands when ANY declared constructor is
            // unnamed (`E(); E.named();` keeps `E()`); a named-only
            // declaration drops it. The no-declared-constructor case folds
            // into the close (`ctor_named` stays empty there), which reads
            // the implicit default.
            open.ctor_has_unnamed |= capture.has_unnamed;
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
        // Anything buffered inside the body or the parameter list
        // (local-declaration and parameter metadata) binds to no modeled
        // row — drop it with the constructor.
        builder.drop_pending_annotations();
    }

    fn end_typedef(
        &mut self,
        _augment: Option<Token>,
        _typedef: Token,
        equals: Option<Token>,
        _end: Token,
    ) {
        let builder = &mut self.builder;
        if let Some(pending) = builder.pending_typedef.take()
            && let Some((name, range)) = pending.name
        {
            // The alias's right-hand type is the still-armed annotation: the
            // RHS parses after the name and completes before this event, so
            // `take_armed` answers it (modern `=` form only — the old-style
            // function-typed spelling carries its shape in no single armed
            // annotation, and a parameterized alias needs its substitution
            // face before the RHS can stand for uses). The diagnoser's alias
            // fold resolves uses through this type.
            let var_ty = match (equals, pending.saw_type_params) {
                (Some(_), false) => builder.take_armed(),
                _ => Ty::UNKNOWN,
            };
            builder.declare_top(
                name,
                range,
                TopDecl {
                    kind: MicroKind::TypeAlias,
                    type_params: pending.type_params,
                    var_ty,
                    return_ty: Ty::UNKNOWN,
                    formals: None,
                },
            );
        }
    }

    /// An old-style function-typed formal's shape head fired (`bool f(int)`
    /// — the parser probes the shape, then re-walks it emitting the same
    /// events the modern form uses): the capture opens its own level, and
    /// the written return type plus the re-walked formal list fold into
    /// the same `Ty::Function` the modern spelling produces.
    fn begin_function_typed_formal_parameter(&mut self, _token: Token) {
        let builder = &mut self.builder;
        builder
            .capture
            .get_or_insert_with(TreeSink::default)
            .begin_function();
    }

    /// The old-style function-typed formal closed: the level seals and the
    /// root arms exactly like `end_function_type` — the formal's own
    /// `end_formal_parameter` reads the interned shape.
    fn end_function_typed_formal_parameter(&mut self, token: Token, question: Option<Token>) {
        self.end_function_type(token, question);
    }

    fn begin_formal_parameters(&mut self, _token: Token, _kind: ParserMemberKind) {
        let builder = &mut self.builder;
        // A formal list under a function-type level is the shape's own
        // formal region: everything before it is the written return type.
        if let Some(site) = builder.capture.as_mut()
            && site.top_is_function()
        {
            site.function_formals_begin();
        }
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
        let builder = &mut self.builder;
        builder.formal_depth -= 1;
        if builder.formal_depth == 0 {
            builder.attach_formals();
        }
    }

    fn handle_no_formal_parameters(&mut self, _token: Token, _kind: ParserMemberKind) {
        let builder = &mut self.builder;
        builder.executable_header_done = true;
    }

    fn handle_async_modifier(&mut self, _async_kw: Option<Token>, _star: Option<Token>) {
        let builder = &mut self.builder;
        builder.executable_header_done = true;
    }

    #[allow(clippy::too_many_arguments)]
    fn end_formal_parameter(
        &mut self,
        var_or_final: Option<Token>,
        covariant_kw: Option<Token>,
        this_keyword: Option<Token>,
        super_keyword: Option<Token>,
        _period_after_this_or_super: Option<Token>,
        name: Token,
        initializer_start: Option<Token>,
        initializer_end: Option<Token>,
        kind: FormalParameterKind,
        member_kind: ParserMemberKind,
    ) {
        if self.builder.formal_depth != 1 {
            // A formal inside a function-type shape (depth ≥ 2): its
            // name/kind feeds the shape's columns — named formals trail
            // the positional ones, exactly like record fields.
            let builder = &mut self.builder;
            let named = matches!(
                kind,
                FormalParameterKind::RequiredNamed | FormalParameterKind::OptionalNamed
            );
            let formal_name = named.then(|| builder.name_of(name)).flatten();
            if let Some(site) = builder.capture.as_mut()
                && site.top_is_function()
            {
                site.function_formal_end(named, formal_name);
            }
            return;
        }
        let builder = &mut self.builder;
        let Some((name, range)) = builder.capture_name(name) else {
            return;
        };
        let ty = builder.take_armed();
        let required = matches!(
            kind,
            FormalParameterKind::RequiredPositional | FormalParameterKind::RequiredNamed
        );
        // The default value's verbatim text, `=`/`:` separator through the
        // expression (the renderer spells it back; the separator itself is
        // outside the start token). A synthetic boundary token (recovery)
        // is not in the token table — the argument-less direction.
        let default_value = match (initializer_start, initializer_end) {
            (Some(start), Some(end)) => builder.intern_span_text(start, end),
            _ => None,
        };
        let covariant = covariant_kw.is_some();
        // A class/enum primary-constructor formal with a `var`/`final`
        // keyword declares an instance field (`FieldFormalParameterElement
        // .isDeclaring`): bodies read it bare and instances through the
        // member walk. No field-declaration event fires for it, so the
        // capture introduces the member here; it stays out of the
        // declaration-site log — the official verifier never visits it in
        // the plain field walk, the primary-constructor face owns its
        // unused-ness (unused_local_elements_verifier.dart:887-920).
        if member_kind == ParserMemberKind::PrimaryConstructor
            && var_or_final.is_some()
            && this_keyword.is_none()
            && super_keyword.is_none()
            && builder
                .containers
                .last()
                .is_some_and(|open| matches!(open.kind, ContainerKind::Class | ContainerKind::Enum))
        {
            let open = builder.containers.last_mut().expect("checked above");
            open.members.or_insert_with(name, || MemberEntry {
                kind: MemberKind::Field,
                is_static: false,
                is_abstract: false,
                is_external: false,
                is_final: false,
                type_params: 0,
                type_param_names: Box::default(),
                var_ty: ty,
                return_ty: Ty::UNKNOWN,
                formals: None,
                annotations: Box::default(),
                name_range: range,
            });
        }
        let formal = Formal {
            name,
            ty,
            required,
            covariant,
            default_value,
        };
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
                formal,
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

    /// The alias right-hand type capture: a modern non-parameterized
    /// `typedef` keeps its RHS type (the diagnoser's alias fold resolves
    /// uses through it), a parameterized and an old-style spelling keep
    /// `Unknown` (their use faces are not modeled).
    #[test]
    fn captures_typedef_alias_right_hand_type() {
        let strings = StringTable::new();
        let elements = build(
            "typedef IntAlias = int; \
             typedef F = void Function(); \
             typedef G<T> = int; \
             typedef Old(int x);",
            &strings,
        );
        assert_eq!(
            elements
                .decl(id(&strings, "IntAlias"))
                .expect("IntAlias")
                .var_ty,
            Ty::named(id(&strings, "int"))
        );
        assert!(
            matches!(
                elements.decl(id(&strings, "F")).expect("F").var_ty,
                Ty::Function { .. }
            ),
            "the function-typed RHS capture answers"
        );
        assert_eq!(
            elements.decl(id(&strings, "G")).expect("G").var_ty,
            Ty::UNKNOWN
        );
        assert_eq!(
            elements.decl(id(&strings, "Old")).expect("Old").var_ty,
            Ty::UNKNOWN
        );
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
                args: ArgListId::EMPTY,
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

    /// Old-style function-typed formals capture the written function
    /// shape (`bool f(int)`): the shape folds into a first-class
    /// [`Ty::Function`] — return type, formals, and the trailing `?` —
    /// never the bare return head nor `Unknown`. The next ordinary formal
    /// keeps its own written type, and a formal with no written type at
    /// all stays `Unknown`.
    #[test]
    fn function_typed_formals_capture_the_written_shape() {
        let strings = StringTable::new();
        let elements = build(
            "void h(void cb(int inner), bool ok) {} \
             void k(List<int>? g()) {} \
             void n(int a, bool b) {}",
            &strings,
        );
        let positional = |name: &str| {
            elements
                .decl(id(&strings, name))
                .expect(name)
                .formals
                .as_deref()
                .expect("formals")
                .positional
                .to_vec()
        };
        let h = positional("h");
        assert_eq!(h.len(), 2);
        // `void cb(int inner)`: the whole shape is the formal's type.
        assert_eq!(h[0].name, id(&strings, "cb"));
        assert_eq!(
            h[0].ty.render(&strings, elements.type_args()),
            "void Function(int)"
        );
        assert_eq!(h[1].ty, Ty::named(id(&strings, "bool")));
        let k = positional("k");
        assert_eq!(k[0].name, id(&strings, "g"));
        // `List<int>? g()`: the return type rides the shape (`?` on the
        // return, not on the function type).
        assert_eq!(
            k[0].ty.render(&strings, elements.type_args()),
            "List<int>? Function()"
        );
        let n = positional("n");
        assert_eq!(n[0].ty, Ty::named(id(&strings, "int")));
        assert_eq!(n[1].ty, Ty::named(id(&strings, "bool")));
    }

    /// Generic function-type annotations capture the written shape the
    /// same way: `void Function(num) f` folds through
    /// `end_function_type`'s arm — the `Function` head fires no type
    /// event, so the arming comes from the sealed level, and the shape
    /// carries the written return type and formal columns. The next
    /// ordinary formal keeps its own written type, and the named section
    /// captures identically.
    #[test]
    fn generic_function_type_formals_capture_the_written_shape() {
        let strings = StringTable::new();
        let elements = build(
            "void h(void Function(num) f, bool ok) {} \
             void k(int Function() g) {} \
             void m({void Function({int x})? cb}) {}",
            &strings,
        );
        let positional = |name: &str| {
            elements
                .decl(id(&strings, name))
                .expect(name)
                .formals
                .as_deref()
                .expect("formals")
                .positional
                .to_vec()
        };
        let h = positional("h");
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].name, id(&strings, "f"));
        assert_eq!(
            h[0].ty.render(&strings, elements.type_args()),
            "void Function(num)"
        );
        assert_eq!(h[1].ty, Ty::named(id(&strings, "bool")));
        // An empty nested group is the reserved shape's identity
        // (`dynamic Function()`).
        let k = positional("k");
        assert_eq!(
            k[0].ty.render(&strings, elements.type_args()),
            "int Function()"
        );
        let named = elements
            .decl(id(&strings, "m"))
            .expect("m")
            .formals
            .as_deref()
            .expect("formals")
            .named
            .to_vec();
        assert_eq!(named[0].name, id(&strings, "cb"));
        assert_eq!(
            named[0].ty.render(&strings, elements.type_args()),
            "void Function({int x})?"
        );
    }

    /// The function-typed-formal capture's golden pins (the collector
    /// capture-order audit's five shapes), each self-contained so a
    /// capture rewrite cannot silently regress the fold order. Pins 1-2
    /// discriminate the nesting chain (a nested formal's shape folds into
    /// the outer shape's column, never replaces it), pins 3-5 guard the
    /// boundary faces (a body closure must not retrofit the closed table;
    /// a `List<int>?` return rides the shape; the modern annotation and
    /// the untyped follower keep their own faces).
    #[test]
    fn function_typed_formal_shape_golden_pins() {
        let strings = StringTable::new();
        let elements = build(
            "void h(void cb(int inner), bool ok) {} \
             void f(void cb(void inner2(int z))) {} \
             void g(void cb(int x)) { [1].map((int y) => y); } \
             void k(List<int>? g()) {} \
             void m(void Function(num) p, q) {}",
            &strings,
        );
        let positional = |name: &str| {
            elements
                .decl(id(&strings, name))
                .unwrap_or_else(|| panic!("decl {name}"))
                .formals
                .as_deref()
                .unwrap_or_else(|| panic!("formals {name}"))
                .positional
                .to_vec()
        };
        let render = |ty: Ty| ty.render(&strings, elements.type_args());
        // Pin 1 — single level: `cb` carries the whole written shape, the
        // follower keeps its own type, and the nested `inner` folds into
        // the shape's column.
        let h = positional("h");
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].name, id(&strings, "cb"));
        assert_eq!(render(h[0].ty), "void Function(int)");
        assert_eq!(h[1].name, id(&strings, "ok"));
        assert_eq!(h[1].ty, Ty::named(id(&strings, "bool")));
        // Pin 2 — two-level nesting: the inner shape folds into the outer
        // shape's formal column.
        let f = positional("f");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].name, id(&strings, "cb"));
        assert_eq!(render(f[0].ty), "void Function(void Function(int))");
        // Pin 3 — body closure: the table closed at `g`'s own `)`; the
        // closure's `(int y)` arms at depth 0 afterwards and must not
        // retrofit or extend it.
        let g = positional("g");
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].name, id(&strings, "cb"));
        assert_eq!(render(g[0].ty), "void Function(int)");
        // Pin 4 — the `List<int>?` return type rides the shape (the `?`
        // stays on the return).
        let k = positional("k");
        assert_eq!(k.len(), 1);
        assert_eq!(k[0].name, id(&strings, "g"));
        assert_eq!(render(k[0].ty), "List<int>? Function()");
        // Pin 5 — the modern `void Function(num)` annotation folds the
        // same; the untyped follower `q` has no written type and stays
        // `Unknown`.
        let m = positional("m");
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].name, id(&strings, "p"));
        assert_eq!(render(m[0].ty), "void Function(num)");
        assert_eq!(m[1].name, id(&strings, "q"));
        assert_eq!(m[1].ty, Ty::UNKNOWN);
    }

    /// #591 capture-half discriminator: the typed-return × plain-method
    /// shape behind the `undefined_member_matrix` corpus miss must carry the
    /// depth-0 formal table, exactly like the `void`-return and no-return
    /// neighbors (the diagnoser's named-argument membership test reads
    /// `MemberEntry::formals` verbatim).
    ///
    /// The control arms isolate the losing event should the typed arm miss:
    /// `void m(...)` fires `handle_void_keyword` (no type-reference
    /// identifier event), `plain(...)` fires `handle_no_type`, while
    /// `int m(...)` fires the head's `handle_identifier(TypeReference)` +
    /// `handle_type` pair before the name.
    #[test]
    fn typed_return_method_captures_formals() {
        let strings = StringTable::new();
        let elements = build(
            "class C { \
               int m(int a) => a; \
               void v(int b) {} \
               plain(int c) {} \
             }",
            &strings,
        );
        let member = |name: &str| {
            elements
                .container(id(&strings, "C"))
                .unwrap()
                .members
                .get(&id(&strings, name))
                .map(|entry| (entry.kind, entry.formals.as_deref()))
        };
        let (kind, formals) = member("m").expect("typed-return m captured");
        assert_eq!(kind, MemberKind::Method);
        let formals = formals.expect("typed-return m formals captured");
        assert_eq!(formals.positional.len(), 1);
        assert_eq!(formals.positional[0].name, id(&strings, "a"));
        let (kind, formals) = member("v").expect("void-return v captured");
        assert_eq!(kind, MemberKind::Method);
        assert!(formals.is_some(), "void-return control");
        let (kind, formals) = member("plain").expect("no-return plain captured");
        assert_eq!(kind, MemberKind::Method);
        assert!(formals.is_some(), "no-return control");
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
                args: ArgListId::EMPTY,
            }
        );
        assert_eq!(decl("make").return_ty, Ty::named(id(&strings, "C")));
        assert_eq!(
            decl("makeNull").return_ty,
            Ty::Named {
                name: id(&strings, "C"),
                nullable: true,
                args: ArgListId::EMPTY,
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
                args: ArgListId::EMPTY,
            }
        );
    }

    /// Declaration-arity violations: the operator parameter count table,
    /// including the minus special case. Setters exercise the parse tier's
    /// ownership (their shapes carry no capture-time entry).
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
        assert_eq!(vs.len(), 4, "{vs:?}");
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
        // Setters carry no capture-time entry: the parse tier reports the
        // `wrong_number_of_parameters_for_setter` row at the name (upstream
        // Dart SDK source, pkg/analyzer/lib/src/fasta/ast_builder.dart
        // :6464-6495), and a log entry here would double it in the merged
        // report — every violating shape (`(a, b)`, `([a])`, `({a})`, `()`)
        // reaches the parse gate, so the log stays operator-only.
        assert!(
            vs.iter().all(|v| !matches!(v, ArityViolation::Setter)),
            "{vs:?}"
        );
    }

    /// Unnamed extensions keep no top-level name; named and unnamed
    /// extensions both produce one application face each, carrying the `on`
    /// type (the applicability anchor) and the members under their lookup
    /// spellings, static bit preserved.
    #[test]
    fn captures_extension_application_faces() {
        fn face_member<'a>(
            face: &'a crate::model::ExtensionFace,
            name: &str,
            strings: &StringTable,
        ) -> Option<&'a crate::model::MemberEntry> {
            face.members.get(&id(strings, name))
        }
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

    /// Prefixed clause names keep the continuation (`implements p.A`
    /// buffers `A`, the model's bare-name convention — the same discipline
    /// as the qualified extends head). The prefix token fires
    /// `PrefixedTypeReference`, which the collector never routed, and the
    /// continuation was super-window-only — every with/implements/on face
    /// lost the prefixed entry entirely.
    #[test]
    fn prefixed_clause_names_keep_the_continuation() {
        let strings = StringTable::new();
        let elements = build(
            "class T {} mixin M {} class A {} \
             class C extends p.S implements p.A {} \
             class W with p.M implements p.A {} \
             mixin N on p.T implements p.A {} \
             enum E with p.M implements p.A { one }",
            &strings,
        );
        let c = elements.container(id(&strings, "C")).expect("C");
        assert_eq!(c.superclass, Some(id(&strings, "S")));
        assert_eq!(c.implements_clause, vec![id(&strings, "A")]);
        let w = elements.container(id(&strings, "W")).expect("W");
        assert_eq!(w.with_clause, vec![id(&strings, "M")]);
        assert_eq!(w.implements_clause, vec![id(&strings, "A")]);
        let n = elements.container(id(&strings, "N")).expect("N");
        assert_eq!(n.on_clause, vec![id(&strings, "T")]);
        assert_eq!(n.implements_clause, vec![id(&strings, "A")]);
        let e = elements.container(id(&strings, "E")).expect("E");
        assert_eq!(e.with_clause, vec![id(&strings, "M")]);
        assert_eq!(e.implements_clause, vec![id(&strings, "A")]);
    }

    /// A named mixin application's `with p.M` cannot steal the head's
    /// super slot: the head `S` latched first, so the qualified with-name
    /// buffers into the with clause (the continuation used to overwrite
    /// the candidate unconditionally, leaving superclass = M and an empty
    /// with clause).
    #[test]
    fn prefixed_with_name_keeps_the_mixin_application_head() {
        let strings = StringTable::new();
        let elements = build("class S {} mixin M {} class C = S with p.M;", &strings);
        let c = elements.container(id(&strings, "C")).expect("C");
        assert_eq!(c.superclass, Some(id(&strings, "S")));
        assert_eq!(c.with_clause, vec![id(&strings, "M")]);
    }

    /// Generic clause types keep only their head names: identifiers inside
    /// a clause type's argument list are argument positions, not clause
    /// faces (`implements List<int>` buffers exactly [List], nested
    /// `Map<K, V>` arguments at any depth stay out). The arguments used to
    /// join the clause buffer as extra entries and the clause events'
    /// trailing-count claim then picked the wrong tail.
    #[test]
    fn generic_clause_types_keep_only_their_heads() {
        let strings = StringTable::new();
        let elements = build(
            "class C implements List<int> {} \
             class W with M<A>, N<Map<K, V>> {} \
             mixin Mx on p.T<R> {}",
            &strings,
        );
        let c = elements.container(id(&strings, "C")).expect("C");
        assert_eq!(c.implements_clause, vec![id(&strings, "List")]);
        assert!(c.header_residue.is_empty(), "{:?}", c.header_residue);
        let w = elements.container(id(&strings, "W")).expect("W");
        assert_eq!(w.with_clause, vec![id(&strings, "M"), id(&strings, "N")]);
        assert!(w.header_residue.is_empty(), "{:?}", w.header_residue);
        let mx = elements.container(id(&strings, "Mx")).expect("Mx");
        assert_eq!(mx.on_clause, vec![id(&strings, "T")]);
        assert!(mx.header_residue.is_empty(), "{:?}", mx.header_residue);
    }

    /// The `extends` head still latches the head name only: `extends S<T>`
    /// keeps superclass = S with the argument staying out of the clause
    /// window entirely (no residue — the argument used to fall through the
    /// occupied super slot into the clause buffer).
    #[test]
    fn generic_extends_head_latches_only_the_head() {
        let strings = StringTable::new();
        let elements = build("class C extends S<T> {}", &strings);
        let c = elements.container(id(&strings, "C")).expect("C");
        assert_eq!(c.superclass, Some(id(&strings, "S")));
        assert!(c.with_clause.is_empty());
        assert!(c.implements_clause.is_empty());
        assert!(c.header_residue.is_empty(), "{:?}", c.header_residue);
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
        assert!(e.members.contains_key(&id(&strings, "m")));
        assert!(!e.members.contains_key(&id(&strings, "x")));
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

    /// The written-type capture feeds the pool (#728): a generic formal's
    /// declared type rides a non-empty slot (`List<T>` keeps its argument,
    /// a type parameter stays argument-less inside it), and the `?`
    /// projection lands on the annotated root. The reference pool interns
    /// the same lists manually — slot equality across the two pools pins
    /// the capture's numbering.
    #[test]
    fn declared_types_intern_their_arguments() {
        let strings = StringTable::new();
        let elements = build("class C<T> { List<T> f; List<int>? g; }", &strings);
        let member = |name: &str| {
            elements
                .container(id(&strings, "C"))
                .expect("C")
                .members
                .get(&id(&strings, name))
                .unwrap_or_else(|| panic!("member {name}"))
                .var_ty
        };
        let (list, t, int) = (id(&strings, "List"), id(&strings, "T"), id(&strings, "int"));
        let mut reference = crate::model::ArgPool::default();
        let list_t = reference.named(list, false, &[Ty::named(t)]);
        let list_int_q = reference.named(list, true, &[Ty::named(int)]);
        assert_eq!(member("f"), list_t, "`List<T>` rides a non-empty slot");
        assert_eq!(member("g"), list_int_q, "the `?` projects on the root");
        assert_eq!(elements.type_args().len(), 2);
    }

    /// Nested arguments intern bottom-up (model.rs:243-246): the inner
    /// `List<int>` takes its slot before the outer `List<List<int>>`, in
    /// left-to-right source order — the capture order the warm/cold
    /// byte-equality discipline keys on. A capture that interned the
    /// outer list first would number the slots in the other order and
    /// fail the cross-pool equality below.
    #[test]
    fn nested_arguments_intern_inner_first() {
        let strings = StringTable::new();
        let elements = build("class C { List<List<int>> f; }", &strings);
        let (list, int) = (id(&strings, "List"), id(&strings, "int"));
        let f = elements
            .container(id(&strings, "C"))
            .expect("C")
            .members
            .get(&id(&strings, "f"))
            .expect("f")
            .var_ty;
        let mut reference = crate::model::ArgPool::default();
        let inner = reference.named(list, false, &[Ty::named(int)]);
        let outer = reference.named(list, false, &[inner]);
        assert_eq!(f, outer);
        assert_eq!(elements.type_args().len(), 2);
        assert_eq!(f.render(&strings, elements.type_args()), "List<List<int>>");
    }

    /// A function type inside an argument list nests as a first-class
    /// column: the declared type carries the argument it was written
    /// with, and the shape interns exactly once.
    #[test]
    fn function_type_arguments_stay_nested() {
        let strings = StringTable::new();
        let elements = build("class C { Map<String, void Function()> f; }", &strings);
        let f = elements
            .container(id(&strings, "C"))
            .expect("C")
            .members
            .get(&id(&strings, "f"))
            .expect("f")
            .var_ty;
        assert_eq!(
            f.render(&strings, elements.type_args()),
            "Map<String, void Function()>"
        );
        // One interned entry: the argument list (the function shape
        // interns in the pool's function column).
        assert_eq!(elements.type_args().len(), 1);
    }

    /// Only declarations intern: an `is`/`as` type inside a body arms a
    /// capture like any annotation, but no declaration ever consumes it —
    /// the pool must come out empty (capture-immutable after `collect`,
    /// never fed by body noise).
    #[test]
    fn body_annotations_never_intern() {
        let strings = StringTable::new();
        let elements = build(
            "class C { int f; } \
             void m() { var x = y as List<int>; if (x is Map<String, int>) print(x); } \
             String top = '';",
            &strings,
        );
        assert!(
            elements.type_args().is_empty(),
            "body type arguments must not intern"
        );
        assert_eq!(
            elements.decl(id(&strings, "top")).expect("top").var_ty,
            Ty::named(id(&strings, "String"))
        );
    }

    /// The declaration's metadata lands in the container face, name-level,
    /// for every container kind; the span is `@` through the name's last
    /// segment (the official annotation-anchored report position) and a
    /// qualified spelling interns dot-joined (#705).
    #[test]
    fn container_annotations_capture_name_and_span() {
        let strings = StringTable::new();
        let elements = build(
            "@sealed class C {}\n@x.y mixin M {}\n@literal enum E { e }\n",
            &strings,
        );
        let annotations_of = |name: &str| {
            elements
                .container(id(&strings, name))
                .expect(name)
                .annotations
                .to_vec()
        };
        let class = annotations_of("C");
        assert_eq!(class.len(), 1);
        assert_eq!(class[0].name, id(&strings, "sealed"));
        assert_eq!(class[0].range.offset, 0);
        assert_eq!(class[0].range.length, 7);
        let mixin = annotations_of("M");
        assert_eq!(mixin.len(), 1);
        assert_eq!(mixin[0].name, id(&strings, "x.y"));
        assert_eq!(mixin[0].range.offset, 19);
        assert_eq!(mixin[0].range.length, 4);
        let enum_ = annotations_of("E");
        assert_eq!(enum_.len(), 1);
        assert_eq!(enum_[0].name, id(&strings, "literal"));
        assert_eq!(enum_[0].range.offset, 35);
        assert_eq!(enum_[0].range.length, 8);
    }

    /// A member's metadata is not the container face: the class carrying
    /// it and the next declaration both stay clean — but the member entry
    /// itself now claims the star (the #1637 substrate). A member star
    /// reaches [`MemberEntry`] and only [`MemberEntry`].
    #[test]
    fn member_annotations_never_reach_containers() {
        let strings = StringTable::new();
        let elements = build("class A { @override void m() {} }\nclass B {}", &strings);
        for name in ["A", "B"] {
            let container = elements.container(id(&strings, name)).expect(name);
            assert!(container.annotations.is_empty(), "{name}");
        }
        let a = elements.container(id(&strings, "A")).expect("A");
        let m = a.members.get(&id(&strings, "m")).expect("m");
        assert_eq!(m.annotations.len(), 1);
        assert_eq!(m.annotations[0].name, id(&strings, "override"));
        assert_eq!(m.annotations[0].range.offset, 10);
    }

    /// Every member kind claims its own star: methods at `begin_method`,
    /// each enum constant at its name, and a field group's star rides
    /// every group member (`@meta int a, b;` annotates both). A
    /// body-local star (a local declaration's metadata) never leaks into
    /// the next member.
    #[test]
    fn member_annotations_claim_into_member_entries() {
        let strings = StringTable::new();
        let elements = build(
            "enum E { @v1 a, @v2 b }\n\
             class C {\n\
             @m1 int x, y;\n\
             @m2 void m() { @local var q = 0; }\n\
             int clean = 0;\n\
             }",
            &strings,
        );
        let e = elements.container(id(&strings, "E")).expect("E");
        let names = ["a", "b"];
        for (constant, star) in names.iter().zip(["v1", "v2"]) {
            let entry = e.members.get(&id(&strings, constant)).expect(constant);
            assert_eq!(entry.annotations.len(), 1, "{constant}");
            assert_eq!(entry.annotations[0].name, id(&strings, star));
        }
        let c = elements.container(id(&strings, "C")).expect("C");
        // The group star rides both group members; the unannotated
        // `clean` stays clean.
        for name in ["x", "y"] {
            let entry = c.members.get(&id(&strings, name)).expect(name);
            assert_eq!(entry.annotations.len(), 1, "{name}");
            assert_eq!(entry.annotations[0].name, id(&strings, "m1"));
        }
        let m = c.members.get(&id(&strings, "m")).expect("m");
        assert_eq!(m.annotations.len(), 1);
        assert_eq!(m.annotations[0].name, id(&strings, "m2"));
        let clean = c.members.get(&id(&strings, "clean")).expect("clean");
        assert!(clean.annotations.is_empty(), "body-local star dropped");
    }

    /// The `covariant` modifier rides the `end_formal_parameter` seam into
    /// the captured rows (positional and named tables), and an extension
    /// type's representation formal captures the same bit.
    #[test]
    fn formal_covariant_bit_rounds_the_seam() {
        let strings = StringTable::new();
        let elements = build(
            "class C {\n\
             void m(covariant int a, int b, {covariant int n = 1, int o}) {}\n\
             }\n\
             extension type E(covariant final int r) {}",
            &strings,
        );
        let c = elements.container(id(&strings, "C")).expect("C");
        let formals = c
            .members
            .get(&id(&strings, "m"))
            .expect("m")
            .formals
            .as_deref()
            .expect("m's formal table");
        let covariant = |name: &str| {
            formals
                .named_formal(id(&strings, name))
                .or_else(|| {
                    formals
                        .positional
                        .iter()
                        .find(|f| f.name == id(&strings, name))
                })
                .map(|f| f.covariant)
                .unwrap_or_else(|| panic!("formal {name}"))
        };
        assert!(covariant("a"), "written covariant captures true");
        assert!(!covariant("b"), "unmodified formal stays false");
        assert!(covariant("n"), "a named formal captures the bit too");
        assert!(!covariant("o"));
        let e = elements.container(id(&strings, "E")).expect("E");
        assert!(e.primary_formals[0].formal.covariant);
    }

    /// A default value interns the initializer's verbatim source text
    /// (`=`/`:` separator outside the slice); a formal without one answers
    /// `None` — including the extension-type representation formal.
    #[test]
    fn formal_default_value_interns_the_written_text() {
        let strings = StringTable::new();
        let source = "void f({int x = 3, int y = 1 + 2, int z}) {}\n\
                      void g([int w = 4]) {}\n\
                      extension type E(int r) {}";
        let elements = build(source, &strings);
        let resolve = |text: Option<StringId>| text.map(|at| strings.resolve(at).to_string());
        let formals = elements
            .decl(id(&strings, "f"))
            .and_then(|decl| decl.formals.clone())
            .expect("f's formals");
        let default_of = |name: &str| {
            resolve(
                formals
                    .named_formal(id(&strings, name))
                    .and_then(|row| row.default_value),
            )
        };
        assert_eq!(default_of("x").as_deref(), Some("3"));
        assert_eq!(default_of("y").as_deref(), Some("1 + 2"));
        assert_eq!(default_of("z"), None, "no initializer captures None");
        let g = elements
            .decl(id(&strings, "g"))
            .and_then(|decl| decl.formals.clone())
            .expect("g's formals");
        assert_eq!(
            resolve(g.positional[0].default_value).as_deref(),
            Some("4"),
            "the legacy `:` positional captures too"
        );
        let e = elements.container(id(&strings, "E")).expect("E");
        assert_eq!(e.primary_formals[0].formal.default_value, None);
    }

    /// A non-container declaration claims (and discards) the metadata
    /// preceding it: only the class's own star reaches its face — the
    /// function, typedef, and top-level variable boundaries drop theirs.
    #[test]
    fn non_container_boundaries_drop_their_annotations() {
        let strings = StringTable::new();
        let elements = build(
            "@Deprecated() f() {}\n@x typedef T = int;\n@y const z = 1;\n@sealed class C {}",
            &strings,
        );
        let c = elements.container(id(&strings, "C")).expect("C");
        assert_eq!(c.annotations.len(), 1);
        assert_eq!(c.annotations[0].name, id(&strings, "sealed"));
    }

    /// Metadata inside a function body (a local declaration's) binds to
    /// nothing and drops with the executable.
    #[test]
    fn body_local_annotations_drop_with_the_function() {
        let strings = StringTable::new();
        let elements = build("void f() { @pragma('x') var q = 1; }\nclass C {}", &strings);
        let c = elements.container(id(&strings, "C")).expect("C");
        assert!(c.annotations.is_empty());
    }

    /// An annotated directive claims its metadata: the import's
    /// `@Deprecated` never reaches the following class.
    #[test]
    fn annotated_directive_drops_its_annotations() {
        let strings = StringTable::new();
        let elements = build("@Deprecated import 'a.dart';\n@sealed class C {}", &strings);
        let c = elements.container(id(&strings, "C")).expect("C");
        assert_eq!(c.annotations.len(), 1);
        assert_eq!(c.annotations[0].name, id(&strings, "sealed"));
    }

    /// The pool-discipline law: a `Ty` slot is canonical only within its
    /// own universe — reading it through another unit's pool fails loudly
    /// (model.rs `ArgPool::get`'s documented panic), never falls back.
    #[test]
    #[should_panic(expected = "index out of bounds")]
    fn a_foreign_slot_read_panics() {
        let owner = StringTable::new();
        let elements = build("class C { List<int> f; }", &owner);
        let other = StringTable::new();
        let foreign = build("class D {}", &other);
        let f = elements
            .container(id(&owner, "C"))
            .expect("C")
            .members
            .get(&id(&owner, "f"))
            .expect("f")
            .var_ty;
        // The foreign pool is empty; the slot indexes out of range.
        let _ = f.render(&owner, foreign.type_args());
    }

    /// A nested declared type round-trips through the persisted payload
    /// (plan T-A acceptance): `of` projects the pool structurally,
    /// `into_elements` re-interns bottom-up, and the restored model equals
    /// the captured one — same `Ty`s, same slots, same display.
    #[test]
    fn nested_declared_type_round_trips_through_the_payload() {
        use crate::persisted::PersistedElements;
        let strings = StringTable::new();
        let elements = build("class C { Map<String, List<int>> f; }", &strings);
        let restored = PersistedElements::of(&elements, &strings).into_elements(&strings);
        let f = |e: &FileElements| {
            e.container(id(&strings, "C"))
                .expect("C")
                .members
                .get(&id(&strings, "f"))
                .expect("f")
                .var_ty
        };
        let (captured, restored_ty) = (f(&elements), f(&restored));
        assert_eq!(captured, restored_ty, "same slots, bottom-up both ways");
        assert_eq!(
            captured.render(&strings, elements.type_args()),
            "Map<String, List<int>>"
        );
        assert_eq!(
            restored_ty.render(&strings, restored.type_args()),
            "Map<String, List<int>>"
        );
        assert_eq!(restored.type_args().len(), 2);
    }
}
