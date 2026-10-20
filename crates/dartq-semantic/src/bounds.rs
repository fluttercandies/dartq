//! Explicit type-argument boundedness: the written-type capture and the
//! `type_argument_not_matching_bounds` face (#610), the first semantic
//! consumer of the #601 `is_well_bounded` machinery.
//!
//! Fact sources (behavior anchored, form native):
//!
//! * `pkg/analyzer/lib/src/error/type_arguments_verifier.dart:342-540` —
//!   `_checkForTypeArgumentNotMatchingBounds`: at every resolved type
//!   annotation with explicit type arguments, each argument is checked
//!   against its parameter's substituted bound; a regular violation
//!   reports `type_argument_not_matching_bounds` at the offending
//!   argument (`_typeArgumentErrorNode`, :757-768); where super-bounded
//!   types are allowed the failing type is re-checked inverted
//!   (`replaceTopAndBottom`) and only the still-failing *inverted*
//!   arguments report (:490-539).
//! * `type_arguments_verifier.dart:737-755` —
//!   `_shouldAllowSuperBoundedTypes`: supertype clauses, type aliases,
//!   and extension-type elements forbid the super-bounded pass. dartq
//!   keeps the extension-type arm exact; the clause/alias contexts are
//!   not distinguished at the site, which only moves a potential report
//!   toward suppression (the under-report direction).
//! * `pkg/analyzer/lib/src/generated/error_verifier.dart:5061-5079` —
//!   `_checkForEnumInstantiatedToBoundsIsNotWellBounded`: the enum's
//!   implicit `values` field type's single type argument (the enum
//!   instantiated to bounds) must be well-bounded with super-boundedness
//!   allowed, else `enum_instantiated_to_bounds_is_not_well_bounded` at
//!   the enum name.
//!
//! # The capture
//!
//! The model's declared-type slots carry canonical argument lists (#611),
//! but the structures this face needs — the written *bound* of each
//! declared type parameter, the per-site argument subtrees it checks, and
//! the shapes the slot cannot represent (function types, record types,
//! a headless group) — are captured as `TypeTree`s from the parse events
//! while a bound window (a `<...>` type-variables group) or a container
//! header window (extends/with/implements clauses) is open. The engine
//! routes `handle_type` and the group events into the open sink; any
//! shape the face cannot model (function types, record types, a headless
//! group) poisons the enclosing tree, and a poisoned tree suppresses its
//! check — the under-report direction, never a false positive.
//!
//! # The snapshot
//!
//! Checks run against a lazily built `dartq_types` snapshot: one
//! [`TypeTable`]/[`TypeHierarchy`] pair declaring every in-file container
//! (its captured parameter bounds become a `ParamFactsTable`) plus a
//! curated dart:core builtin face whose supertype edges are the fixed
//! public declarations (`int extends num`, `List<T>` implements
//! `Iterable<T>`, ...). A container whose captured supertype clause fails
//! to map (an unresolvable head, a poisoned tree, a clause/count
//! mismatch) is *open*: its edges are unfaithful, so its supertype
//! closure is unsafe and every check whose argument root reaches it is
//! suppressed — the subtype walk would otherwise answer `false` where
//! the truth is `true`, the false-positive direction.

use std::cell::{Cell, RefCell};

use dartq_source::{SourceRange, StringId, StringTable};
use rustc_hash::{FxHashMap, FxHashSet};

use dartq_types::{
    Boundedness, CoreDecls, NamedMember, ParamFacts, ParamFactsTable, TypeDeclId, TypeHierarchy,
    TypeId, TypeKind, TypeParamId, TypeSystem, TypeTable, UninternedFunction, UninternedType,
    Variance, instantiate_type_formals_to_bounds, is_well_bounded, replace_top_bottom,
};

// The written-type capture is shared with the collector's pool feeding
// (#728); the single `pub(crate) use` both imports the names for this
// module and keeps `crate::bounds::TypeTree` as the import face.
pub(crate) use crate::capture::{TreeSink, TypeTree};

/// The supertype-capture cursor of one open container header: clause
/// events fire after their types parse (`declarations.rs` parses the
/// clause's type list, then fires the `handle_*` event), so each event
/// claims the roots completed since the previous claim.
pub(crate) struct HeaderSink {
    sink: TreeSink,
    claimed: usize,
    /// A clause's count disagreed with its captured roots, or an
    /// unattributed root remained at body time: the capture is
    /// unfaithful.
    open: bool,
    /// An extension type's header: the representation type and the
    /// `implements` faces as written, argument-bearing (the
    /// `extension_type_implements_not_supertype` face's facts).
    extension: Option<ExtFaceCapture>,
}

/// One extension type's written representation type and `implements`
/// faces. The representation is `None` when the primary constructor did
/// not leave exactly one completed type root (an untyped or recovered
/// formal list) — its faces then decide nothing.
#[derive(Clone)]
pub(crate) struct ExtFaceCapture {
    pub(crate) rep: Option<TypeTree>,
    pub(crate) faces: Vec<TypeTree>,
}

/// Which clause completed. `On` trees are constraints, not supertypes —
/// claimed and discarded.
#[derive(Clone, Copy)]
pub(crate) enum Clause {
    /// `extends T` (the named-mixin-application head included): the
    /// given type count.
    Extends(usize),
    /// `with M...`: the with-clause mixins (counted by the cursor).
    With,
    /// `implements I...`: the given count.
    Implements(usize),
    /// `on C...` (mixins): the given count, discarded.
    On(usize),
}

impl HeaderSink {
    /// Claims the roots since the cursor for the completing clause;
    /// returns the claimed supertypes (`On` answers empty).
    fn clause(&mut self, clause: Clause) -> Vec<TypeTree> {
        let (expected, keep) = match clause {
            Clause::Extends(count) | Clause::Implements(count) | Clause::On(count) => {
                (Some(count), !matches!(clause, Clause::On(_)))
            }
            Clause::With => (None, true),
        };
        let since = self.sink.unclaimed(self.claimed);
        if let Some(expected) = expected
            && since.len() != expected
        {
            self.open = true;
        }
        let claimed = since.to_vec();
        self.claimed = self.sink.done.len();
        if keep { claimed } else { Vec::new() }
    }

    /// Body time: a root no clause claimed means the capture is not the
    /// faithful supertype set.
    fn finish(mut self) -> (Vec<TypeTree>, bool, Option<ExtFaceCapture>) {
        if !self.sink.unclaimed(self.claimed).is_empty() || !self.sink.stack.is_empty() {
            self.open = true;
        }
        (self.sink.done, self.open, self.extension)
    }
}

/// One in-file container's captured boundedness facts.
struct ContainerCapture {
    /// Type-parameter names in declaration order.
    params: Vec<StringId>,
    /// Each parameter's written bound (`None` = unbounded; the bool is
    /// the poisoned bit — bounded by an unmodelable shape, which
    /// suppresses the container's checks).
    bounds: Vec<(Option<TypeTree>, bool)>,
    /// Captured supertype trees (extends/with/implements).
    supers: Vec<TypeTree>,
    /// The supertype capture is unfaithful.
    open: bool,
    /// `extension type` elements forbid the super pass
    /// (`_shouldAllowSuperBoundedTypes`, :750-752).
    is_extension_type: bool,
    /// Enums run the values-field check at the declaration.
    is_enum: bool,
}

/// The parse-time capture index, keyed by container name (keep-first: a
/// same-named duplicate's second header is the `duplicate_definition`
/// face's business).
#[derive(Default)]
struct BoundsIndex {
    containers: FxHashMap<StringId, ContainerCapture>,
    /// Type-parameter names declared WITH a bound (function scopes
    /// included): an argument of that name suppresses — the official
    /// subtype test succeeds through the parameter's own bound, which
    /// the M1 face cannot answer. Sticky across scopes: a name is
    /// bounded if any of its declarations was, the safe direction.
    bounded_params: FxHashSet<StringId>,
    /// Captured function-type aliases (`typedef A<X> = X Function(X);`),
    /// keyed by the alias name (keep-first like the containers). The
    /// bounds snapshot expands an alias head through its captured RHS —
    /// the only alias face the M1 decision order observes (#1351).
    aliases: FxHashMap<StringId, AliasCapture>,
    /// The file's extension types with a captured representation, in
    /// declaration order: `(name, capture)`.
    ext_faces: Vec<(StringId, ExtFaceCapture)>,
    /// Every extension type this file declares (a primary constructor
    /// opened under its header). Non-generic ones register in the snapshot
    /// as bare declarations so a face's type argument can name them.
    ext_names: Vec<StringId>,
}

/// One extension type `implements` face, decided where the snapshot can:
/// the oracle's `isSubtypeOf(declaredRepresentation, face)`
/// (scope_analyzer.dart:1312-1360).
pub(crate) struct ExtFaceRow {
    /// The face's own span (the oracle anchors the row at the face's
    /// named type).
    pub(crate) range: SourceRange,
    /// The face's head name when written bare (no type arguments) — the
    /// name-level fallback's input.
    pub(crate) face_bare: Option<StringId>,
    /// The representation's head name when written bare (no arguments, no
    /// `?`, not a record/function shape).
    pub(crate) rep_bare: Option<StringId>,
    /// `Some` when the snapshot decided the subtype question.
    pub(crate) verdict: Option<bool>,
    /// The snapshot's displays (`Comparable<Score5>`, `int`), when it
    /// mapped both sides.
    pub(crate) displays: Option<(String, String)>,
}

/// One function-type alias's captured declaration: its type parameters
/// (the same shape as a container's — name, written bound, poison bit)
/// and the RHS the head expands to.
struct AliasCapture {
    params: Vec<(StringId, Option<TypeTree>, bool)>,
    rhs: Box<AliasRhs>,
}

/// The captured RHS of a function-type alias: the shape the snapshot's
/// expansion substitutes the alias's parameters into.
enum AliasRhs {
    /// `X Function(Y)`: the return type and the formal types as written.
    /// Named-formal names and their `required` bits are captured (the
    /// expansion's `UninternedFunction` needs both); optional-positional
    /// formals fold into the positional list with the required count
    /// alongside (`None` = every positional formal required).
    Fn {
        ret: Option<TypeTree>,
        positional: Vec<TypeTree>,
        named: Vec<(StringId, TypeTree, bool)>,
        required_positional: Option<usize>,
        /// The RHS spelled a trailing `?` after the formals group
        /// (`typedef W = void Function()?;`) — the expansion is that
        /// nullable function type.
        nullable: bool,
    },
    /// `List<X>`: a named-type RHS expands by substitution like an
    /// interface head.
    Named(TypeTree),
}

impl AliasRhs {
    /// Whether some formal's type mentions one of the alias's own
    /// parameters — the contravariant-occurrence shape whose inverted
    /// bounds pass fails (`enum E<T extends A<T>, U>` with
    /// `A<X> = X Function(X)`; the return-only alias
    /// `A<X> = X Function()` inverts clean and never fires).
    fn param_in_formals(&self, params: &[StringId]) -> bool {
        let mentions = |tree: &TypeTree| tree_mentions(tree, params);
        match self {
            AliasRhs::Fn {
                ret: _,
                positional,
                named,
                required_positional: _,
                nullable: _,
            } => positional.iter().any(mentions) || named.iter().any(|(_, tree, _)| mentions(tree)),
            AliasRhs::Named(_) => false,
        }
    }

    /// Whether any captured tree is poisoned: the alias never expands.
    fn poisoned(&self) -> bool {
        let mentions_poison = |tree: &TypeTree| tree.poisoned;
        match self {
            AliasRhs::Fn {
                ret,
                positional,
                named,
                required_positional: _,
                nullable: _,
            } => {
                ret.as_ref().is_some_and(mentions_poison)
                    || positional.iter().any(mentions_poison)
                    || named.iter().any(|(_, tree, _)| mentions_poison(tree))
            }
            AliasRhs::Named(tree) => tree.poisoned,
        }
    }
}

/// Whether a written tree mentions any of the parameter names (the head
/// itself or any argument root; a poisoned tree answers `true` — the
/// conservative direction for the caller's gate).
fn tree_mentions(tree: &TypeTree, params: &[StringId]) -> bool {
    if tree.poisoned {
        return true;
    }
    if let Some((positional, named)) = &tree.record {
        return positional
            .iter()
            .chain(named.iter().map(|(_, ty)| ty))
            .any(|field| tree_mentions(field, params));
    }
    params.contains(&tree.head) || tree.args.iter().any(|arg| tree_mentions(arg, params))
}

/// Whether any named head in the tree (the root or any argument, record
/// field, or function column) satisfies `pred`; a poisoned tree answers
/// `true` (the conservative direction for the caller's gate).
fn tree_any_head(tree: &TypeTree, pred: &dyn Fn(StringId) -> bool) -> bool {
    if tree.poisoned {
        return true;
    }
    if let Some((positional, named)) = &tree.record {
        return positional
            .iter()
            .chain(named.iter().map(|(_, ty)| ty))
            .any(|field| tree_any_head(field, pred));
    }
    if tree.function.is_some() {
        return true;
    }
    pred(tree.head) || tree.args.iter().any(|arg| tree_any_head(arg, pred))
}

/// Maps an alias's captured RHS in the alias's own parameter scope: the
/// expansion keeps the alias's parameter ids as its leaves, so a use
/// site substitutes the written arguments into it (the official
/// resolves the alias element and instantiates; `instantiateTypeAliasToBounds`
/// covers the bare form through the same substitution). `None` = some
/// tree did not map (a foreign head, an argument arity mismatch, or a
/// co-alias not yet expanded); the build loop retries the next round and
/// drops a never-mapping alias — uses then read `dynamic`, the
/// under-report direction.
fn expand_alias(
    capture: &AliasCapture,
    ctx: &MapCtx<'_, '_>,
    scope: &FxHashMap<StringId, TypeParamId>,
) -> Option<TypeId> {
    match &*capture.rhs {
        AliasRhs::Fn {
            ret,
            positional,
            named,
            required_positional,
            nullable,
        } => {
            // An unwritten return type reads `dynamic` (`Function` bare).
            // A tree that fails to map defers the whole alias.
            let ret = match ret {
                Some(tree) => ctx.map_tree(tree, scope)?,
                None => ctx.table.intern(UninternedType::Dynamic, false),
            };
            let positional = positional
                .iter()
                .map(|tree| ctx.map_tree(tree, scope))
                .collect::<Option<Vec<_>>>()?;
            let named = named
                .iter()
                .map(|(name, tree, required)| {
                    Some(NamedMember {
                        name: ctx.strings.resolve(*name).to_string(),
                        ty: ctx.map_tree(tree, scope)?,
                        required: *required,
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            let required_positional =
                u32::try_from(required_positional.unwrap_or(positional.len())).ok()?;
            ctx.table
                .intern_function(
                    UninternedFunction {
                        return_type: ret,
                        type_parameters: Vec::new(),
                        positional,
                        named,
                        required_positional,
                    },
                    *nullable,
                )
                .ok()
        }
        AliasRhs::Named(tree) => ctx.map_tree(tree, scope),
    }
}

/// The alias-RHS capture cursor: which region of the function type the
/// completed type trees belong to.
#[derive(Clone, Copy, PartialEq)]
enum AliasRegion {
    /// Before the formals: the return type (claimed at the formals
    /// boundary).
    Ret,
    /// Inside the formals region — `named` selects the `{...}` group
    /// over the required/`[...]` positional list.
    Formals { named: bool },
}

/// The open alias capture: the tree sink for the RHS plus the region
/// cursor. Named-formal names attach at the formal's end event, popping
/// the tree that formal's type pushed.
struct AliasSink {
    name: StringId,
    /// The declaring group's captured parameters (joined at the group's
    /// close — the name precedes its own `<...>` group).
    params: Vec<(StringId, Option<TypeTree>, bool)>,
    sink: TreeSink,
    region: Option<AliasRegion>,
    ret: Option<TypeTree>,
    positional: Vec<TypeTree>,
    named: Vec<(StringId, TypeTree, bool)>,
    /// The positional formals captured before the first `[` group (the
    /// expansion's `requiredPositional`); `None` means every positional
    /// formal is required.
    required_positional: Option<usize>,
    /// The RHS parsed a function-type formals region.
    fn_seen: bool,
    /// The RHS spelled a trailing `?` after the function type's formals
    /// (`typedef W = void Function()?;`) — the expansion is nullable.
    rhs_nullable: bool,
    /// Recovery debris: the alias never expands.
    poisoned: bool,
}

/// A mapped ITB violation: what to emit and where.
pub(crate) struct BoundedIssue {
    /// The offending argument's span (the official argument-node anchor).
    pub range: SourceRange,
    /// The argument display (`getDisplayString` face).
    pub argument: String,
    /// The violated parameter's name.
    pub parameter: String,
    /// The substituted bound display.
    pub bound: String,
    /// The inverted type's display when the report rode the super-bounded
    /// pass — the `contextMessages` row's `'<X<invArgs>>'` text
    /// (`type_arguments_verifier.dart:455-463`). `None` on the
    /// extension-type face, which skips the super pass upstream too.
    pub inverted_type: Option<String>,
}

/// The engine's boundedness state: the parse-time capture plus the lazy
/// `dartq_types` snapshot the checks run against. `'a` is the engine's
/// string-table lifetime (the snapshot borrows it for display text).
#[derive(Default)]
pub(crate) struct Bounds<'a> {
    index: BoundsIndex,
    /// The open `<...>` type-variables bound window.
    bound_sink: Option<TreeSink>,
    /// The variables completed in the open window, completion order.
    window: Vec<(StringId, Option<TypeTree>, bool)>,
    /// The just-closed window, awaiting its container's begin event.
    pending: Option<Vec<(StringId, Option<TypeTree>, bool)>>,
    /// The open container-header supertype capture: its container's name
    /// (the capture was created by [`Self::note_container`] at the same
    /// begin event) and the clause cursor.
    header: Option<(StringId, HeaderSink)>,
    /// The outermost explicit type-argument site capture.
    site: Option<TreeSink>,
    /// The open header's root count when an extension type's primary
    /// constructor formals opened: the roots completed since are the
    /// representation formal's type, not supertype clauses.
    rep_mark: Option<usize>,
    /// The open `typedef A<X> = ...` RHS capture (#1351). Top-level
    /// declarations cannot nest, so one slot serves; a container's
    /// `VarHeader`-equivalent overwrite path never coexists with it.
    alias: Option<AliasSink>,
    snapshot: Option<Snapshot<'a>>,
}

impl<'a> Bounds<'a> {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// A `<...>` type-variables group opened: its variables' bounds are
    /// captured (a new window also invalidates any unclaimed latch).
    pub(crate) fn begin_type_variables(&mut self) {
        self.pending = None;
        self.bound_sink = Some(TreeSink::default());
        self.window = Vec::new();
    }

    /// One type variable completed (`end_type_variable`). The parser
    /// completes the group in reverse declaration order, re-parsing each
    /// bound anchored at its own `extends` token, so the roots completed
    /// since the previous completion are this variable's bound.
    pub(crate) fn end_type_variable(&mut self, name: StringId, has_bound: bool) {
        let Some(sink) = self.bound_sink.as_mut() else {
            return;
        };
        let (bound, poisoned) = if has_bound {
            let mut roots = sink.take_done();
            match roots.len() {
                1 => {
                    let poisoned = roots[0].poisoned;
                    (roots.pop(), poisoned)
                }
                _ => (None, true),
            }
        } else {
            (None, false)
        };
        self.window.push((name, bound, poisoned));
    }

    /// The group closed: latch for the owning declaration's begin event.
    /// The parser completes the variables in reverse declaration order,
    /// so the window reverses back — a capture's parameter order is the
    /// written one. Names bounded here latch the sticky bounded-parameter
    /// set — function scopes included. An open alias capture adopts the
    /// window as its declaring parameters (#1351: the typedef's name
    /// precedes the group, so the close always lands inside its capture).
    pub(crate) fn end_type_variables(&mut self) {
        let mut window = std::mem::take(&mut self.window);
        self.bound_sink = None;
        window.reverse();
        for (name, bound, poisoned) in &window {
            if bound.is_some() || *poisoned {
                self.index.bounded_params.insert(*name);
            }
        }
        self.alias_type_variables(&window);
        self.pending = (!window.is_empty()).then_some(window);
    }

    /// The latched window of the last closed type-variable group (#1314
    /// FAM10): function headers adopt it by their declaring name at the
    /// close event — the container adoption consumes its own window, so a
    /// function's latch is always its own.
    pub(crate) fn pending_window(&self) -> Option<&[(StringId, Option<TypeTree>, bool)]> {
        self.pending.as_deref()
    }

    /// The container begin event: adopts the latch when the declaring
    /// name precedes the window's close (the same token-order guard the
    /// raw-type latch uses — a foreign window never adopts).
    pub(crate) fn note_container(
        &mut self,
        name: Option<StringId>,
        name_offset: u32,
        window_end: u32,
        is_extension_type: bool,
        is_enum: bool,
    ) {
        let Some(window) = self.pending.take() else {
            return;
        };
        let Some(name) = name else {
            return;
        };
        if name_offset >= window_end {
            return;
        }
        self.index
            .containers
            .entry(name)
            .or_insert_with(|| ContainerCapture {
                params: window.iter().map(|(n, _, _)| *n).collect(),
                bounds: window.iter().map(|(_, b, p)| (b.clone(), *p)).collect(),
                supers: Vec::new(),
                open: false,
                is_extension_type,
                is_enum,
            });
    }

    /// The container's header window opened (its supertype clauses are
    /// captured from here to the body).
    pub(crate) fn open_header(&mut self, name: StringId) {
        self.header = Some((
            name,
            HeaderSink {
                sink: TreeSink::default(),
                claimed: 0,
                open: false,
                extension: None,
            },
        ));
    }

    /// An extension type's primary-constructor formals opened inside the
    /// header window: the roots they complete are the representation type.
    pub(crate) fn begin_representation(&mut self) {
        if let Some((name, header)) = self.header.as_ref() {
            self.rep_mark = Some(header.sink.done.len());
            if !self.index.ext_names.contains(name) {
                self.index.ext_names.push(*name);
            }
        }
    }

    /// The representation formals closed: their completed roots leave the
    /// header sink (they are no supertype clause) and latch as the
    /// representation type — exactly one root, else none.
    pub(crate) fn end_representation(&mut self) {
        let Some(mark) = self.rep_mark.take() else {
            return;
        };
        let Some((_, header)) = self.header.as_mut() else {
            return;
        };
        if !header.sink.stack.is_empty() || mark > header.sink.done.len() {
            header.extension = Some(ExtFaceCapture {
                rep: None,
                faces: Vec::new(),
            });
            return;
        }
        let mut roots: Vec<TypeTree> = header.sink.done.drain(mark..).collect();
        let rep = if roots.len() == 1 { roots.pop() } else { None };
        header.extension = Some(ExtFaceCapture {
            rep,
            faces: Vec::new(),
        });
    }

    /// A header clause completed; its trees join the supertype set
    /// (`On` trees are discarded — constraints, not supertypes).
    pub(crate) fn header_clause(&mut self, clause: Clause) {
        let Some((name, header)) = self.header.as_mut() else {
            return;
        };
        let trees = header.clause(clause);
        // An extension type's `implements` faces latch argument-bearing;
        // a count mismatch leaves the faces unattributable.
        if matches!(clause, Clause::Implements(_))
            && let Some(extension) = header.extension.as_mut()
        {
            if header.open {
                extension.rep = None;
            }
            extension.faces.extend(trees.iter().cloned());
        }
        if trees.is_empty() {
            return;
        }
        if let Some(capture) = self.index.containers.get_mut(name) {
            capture.supers.extend(trees);
        }
    }

    /// The body opened: the header capture finalizes onto the container.
    pub(crate) fn finish_header(&mut self) {
        let Some((name, header)) = self.header.take() else {
            return;
        };
        let (supers, open, extension) = header.finish();
        if let Some(extension) = extension
            && extension.rep.is_some()
            && !extension.faces.is_empty()
        {
            self.index.ext_faces.push((name, extension));
        }
        if let Some(capture) = self.index.containers.get_mut(&name) {
            capture.supers = supers;
            // An extension type's supertype face is not its class-style
            // edge set (the representation type is no supertype and the
            // faces need not be interfaces of it): the capture stays
            // unfaithful, the snapshot never walks through it.
            capture.open = open || capture.is_extension_type;
        }
    }

    /// Declaration end: a safety-net drop (the capture normally closed
    /// at the body).
    pub(crate) fn drop_header(&mut self) {
        self.header = None;
        self.rep_mark = None;
    }

    /// A simple type completed inside an open capture window.
    pub(crate) fn leaf(&mut self, head: StringId, nullable: bool, range: SourceRange) {
        if let Some((_, header)) = self.header.as_mut() {
            header.sink.leaf(head, nullable, range);
        } else if let Some(sink) = self.bound_sink.as_mut() {
            sink.leaf(head, nullable, range);
        } else if let Some(site) = self.site.as_mut() {
            site.leaf(head, nullable, range);
        }
    }

    /// A type-argument group opened. `head` is `Some` for annotation
    /// heads; the outermost such group outside the capture windows opens
    /// the site capture, and a group inside an open capture nests into
    /// its sink.
    pub(crate) fn begin_group(&mut self, head: Option<(StringId, SourceRange)>, in_site: bool) {
        if let Some((_, header)) = self.header.as_mut() {
            header.sink.begin_group(head);
        } else if let Some(sink) = self.bound_sink.as_mut() {
            sink.begin_group(head);
        } else if let Some(site) = self.site.as_mut() {
            site.begin_group(head);
        } else if in_site && head.is_some() {
            let mut site = TreeSink::default();
            site.begin_group(head);
            self.site = Some(site);
        }
    }

    /// The group closed; `Some` back means the site capture closed with
    /// it (its outermost group) and its single root is the site tree.
    pub(crate) fn end_group(&mut self, end: SourceRange) -> Option<Vec<TypeTree>> {
        if let Some((_, header)) = self.header.as_mut() {
            header.sink.end_group(end);
            return None;
        }
        if let Some(sink) = self.bound_sink.as_mut() {
            sink.end_group(end);
            return None;
        }
        let site = self.site.as_mut()?;
        site.end_group(end);
        // A nested group's closer leaves the site's own level open; only
        // the outermost closer (empty stack) ends the capture.
        if !site.stack.is_empty() {
            return None;
        }
        let site = self.site.take()?;
        Some(site.done)
    }

    /// An unmodelable shape inside any open capture.
    pub(crate) fn poison(&mut self) {
        if let Some((_, header)) = self.header.as_mut() {
            header.sink.poison();
        }
        if let Some(sink) = self.bound_sink.as_mut() {
            sink.poison();
        }
        if let Some(site) = self.site.as_mut() {
            site.poison();
        }
    }

    /// Poisons the open alias capture (#1351): the RHS parsed a shape the
    /// model cannot represent (a record type). The RHS's own function
    /// type poisons through [`Self::alias_formal_boundary`] instead —
    /// [`Self::poison`] deliberately leaves the alias alone, or every
    /// `X Function(...)` RHS would die at its own `Function(`.
    pub(crate) fn alias_poison(&mut self) {
        if let Some(alias) = self.alias.as_mut() {
            alias.poisoned = true;
        }
    }

    // —— #1351: the function-type-alias RHS capture ————————————————————

    /// A typedef's declaring name completed: the RHS capture opens. The
    /// parameters join when the declaring group's window closes (the name
    /// precedes its own `<...>` group); any stale capture dies here —
    /// top-level declarations cannot nest.
    pub(crate) fn begin_alias(&mut self, name: StringId) {
        self.alias = Some(AliasSink {
            name,
            params: Vec::new(),
            sink: TreeSink::default(),
            region: Some(AliasRegion::Ret),
            ret: None,
            positional: Vec::new(),
            named: Vec::new(),
            required_positional: None,
            fn_seen: false,
            rhs_nullable: false,
            poisoned: false,
        });
    }

    /// The declaring group's window closed while an alias capture is
    /// open: the window is the typedef's own parameters (the name always
    /// precedes the group). The latch itself behaves exactly as before —
    /// the token-order guard already kept a typedef's window from any
    /// container's adoption. The sink resets here too: the bound window's
    /// trees joined the parameters, and everything captured from here on
    /// is the RHS (`=` separates the two; the window close is the only
    /// event between them). A function-type *bound* left debris the
    /// boundary already claimed — the reset clears its `fn_seen`/`ret`
    /// traces as well; the poison bit stays sticky.
    pub(crate) fn alias_type_variables(&mut self, window: &[(StringId, Option<TypeTree>, bool)]) {
        if let Some(alias) = self.alias.as_mut() {
            alias.params = window.to_vec();
            alias.sink = TreeSink::default();
            alias.ret = None;
            alias.fn_seen = false;
        }
    }

    /// A simple type head completed inside the open alias capture.
    pub(crate) fn alias_leaf(&mut self, head: StringId, nullable: bool, range: SourceRange) {
        if let Some(alias) = self.alias.as_mut() {
            alias.sink.leaf(head, nullable, range);
        }
    }

    /// A type-argument group opened inside the open alias capture.
    pub(crate) fn alias_begin_group(&mut self, head: Option<(StringId, SourceRange)>) {
        if let Some(alias) = self.alias.as_mut() {
            alias.sink.begin_group(head);
        }
    }

    /// A type-argument group closed inside the open alias capture.
    pub(crate) fn alias_end_group(&mut self, end: SourceRange) {
        if let Some(alias) = self.alias.as_mut() {
            alias.sink.end_group(end);
        }
    }

    /// The RHS's function-type head event: `fn_seen` marks the RHS a
    /// function type (a named RHS finalizes differently at the close). The
    /// parser fires this before the return type parses (upstream Dart SDK
    /// source, pkg/_fe_analyzer_shared/lib/src/parser/type_info_impl.dart,
    /// `ComplexTypeInfo.parseType`'s pass 1 — outermost `Function` first),
    /// so the return's trees keep joining the sink; the return is claimed
    /// at the formals' open. A second head event inside the RHS is a
    /// nested function type (a function-typed formal) — outside the
    /// capture's model.
    pub(crate) fn alias_formal_boundary(&mut self) {
        let Some(alias) = self.alias.as_mut() else {
            return;
        };
        if alias.region != Some(AliasRegion::Ret) || alias.fn_seen {
            alias.poisoned = true;
            return;
        }
        alias.fn_seen = true;
    }

    /// The RHS function type's formal list opened: the trees completed so
    /// far are the return type (none written — bare `Function` — reads
    /// `dynamic` at the expansion). Non-function RHS formals lists and
    /// already-closed captures leave the cursor alone.
    pub(crate) fn alias_formals_region_open(&mut self) {
        let Some(alias) = self.alias.as_mut() else {
            return;
        };
        if !alias.fn_seen || alias.region != Some(AliasRegion::Ret) {
            return;
        }
        match alias.sink.done.len() {
            0 => alias.ret = None,
            1 => alias.ret = alias.sink.done.pop(),
            _ => alias.poisoned = true,
        }
        alias.region = Some(AliasRegion::Formals { named: false });
    }

    /// `[` or `{` opened inside the formals region: the group's spelling
    /// selects the named list; the positional group latches the
    /// required-positional count (optional formals follow it).
    pub(crate) fn alias_optional_group(&mut self, opener: &str) {
        let Some(alias) = self.alias.as_mut() else {
            return;
        };
        if !alias
            .region
            .is_some_and(|region| matches!(region, AliasRegion::Formals { .. }))
        {
            return;
        }
        if opener == "[" && alias.required_positional.is_none() {
            alias.required_positional = Some(alias.positional.len());
        }
        alias.region = Some(AliasRegion::Formals {
            named: opener == "{",
        });
    }

    /// One formal completed: the trees since the previous formal are its
    /// type (exactly one; nested groups merged through the sink). A
    /// named-group formal whose "name" token is the type head itself (the
    /// same offset — `{int}`) reads back positional.
    pub(crate) fn alias_formal_end(&mut self, name: StringId, name_offset: u32, required: bool) {
        let Some(alias) = self.alias.as_mut() else {
            return;
        };
        if !matches!(alias.region, Some(AliasRegion::Formals { .. })) {
            alias.poisoned = true;
            return;
        }
        if alias.sink.done.len() != 1 {
            alias.poisoned = true;
            return;
        }
        let tree = alias.sink.done.pop().expect("one root");
        let AliasRegion::Formals { named } = alias.region.expect("checked above") else {
            return;
        };
        if named && name_offset != tree.range.offset {
            alias.named.push((name, tree, required));
        } else {
            alias.positional.push(tree);
        }
    }

    /// The formals region closed: stray trees past this point poison.
    pub(crate) fn alias_formals_closed(&mut self) {
        if let Some(alias) = self.alias.as_mut() {
            alias.region = None;
        }
    }

    /// The RHS function type closed with (`?`) or without a nullability
    /// suffix (`end_function_type`'s question token): only the outermost
    /// function type's own close answers — the formals region is already
    /// closed and `fn_seen` holds; a nested function-typed formal's close
    /// (region still open) leaves the latch alone.
    pub(crate) fn alias_rhs_question(&mut self, nullable: bool) {
        let Some(alias) = self.alias.as_mut() else {
            return;
        };
        if !alias.fn_seen || alias.region.is_some() {
            return;
        }
        alias.rhs_nullable = nullable;
    }

    /// The typedef closed: the capture finalizes into the index
    /// (keep-first like the containers). A poisoned capture, a
    /// function-type RHS with debris, or a named RHS that is not exactly
    /// one root all drop — the alias never expands (the under-report).
    pub(crate) fn end_alias(&mut self) {
        let Some(mut alias) = self.alias.take() else {
            return;
        };
        let stray = alias.sink.done.len();
        let rhs = if alias.fn_seen {
            (stray == 0).then(|| AliasRhs::Fn {
                ret: alias.ret,
                positional: alias.positional,
                named: alias.named,
                required_positional: alias.required_positional,
                nullable: alias.rhs_nullable,
            })
        } else if stray == 1 {
            alias.sink.done.pop().map(AliasRhs::Named)
        } else {
            None
        };
        if let Some(rhs) = rhs
            && !alias.poisoned
            && !rhs.poisoned()
        {
            self.index
                .aliases
                .entry(alias.name)
                .or_insert(AliasCapture {
                    params: alias.params,
                    rhs: Box::new(rhs),
                });
        }
    }

    // —— checks ————————————————————————————————————————————————————————

    /// The G1 face (`_checkForTypeArgumentNotMatchingBounds`): the
    /// explicit type arguments at `head` must satisfy their parameters'
    /// substituted bounds. Answers the violations to emit, in issue
    /// order; an empty answer covers every suppression path.
    pub(crate) fn check_site(
        &mut self,
        strings: &'a StringTable,
        head: StringId,
        args: &[TypeTree],
        in_scope_params: &[StringId],
    ) -> Vec<BoundedIssue> {
        // A type parameter shadows any container of the same name, and
        // `dynamic` is not a container: neither takes bounded arguments.
        if in_scope_params.contains(&head) {
            return Vec::new();
        }
        let Some(capture) = self.index.containers.get(&head) else {
            return Vec::new();
        };
        // A wrong argument count reports as
        // `wrong_number_of_type_arguments`; the official bounds check
        // then has no instantiation to walk and skips. A poisoned
        // argument tree or an unmodelable parameter bound suppresses, as
        // does an in-scope type-parameter argument declared with a bound
        // (the official test succeeds through the parameter's own bound;
        // the M1 face would answer a false positive).
        let arity_ok = args.len() == capture.params.len();
        let allow_super = !capture.is_extension_type;
        let bounds_decidable = !capture.bounds.iter().any(|(_, poisoned)| *poisoned);
        if !arity_ok
            || !bounds_decidable
            || args.iter().any(|arg| arg.poisoned)
            || args.iter().any(|arg| {
                in_scope_params.contains(&arg.head) && self.index.bounded_params.contains(&arg.head)
            })
        {
            return Vec::new();
        }
        let snapshot = self.snapshot(strings);
        let Some(decl) = snapshot.registry.get(&head).copied() else {
            return Vec::new();
        };
        let ctx = snapshot.ctx();
        let scope = ctx.param_scope(in_scope_params);
        let mut mapped_args = Vec::with_capacity(args.len());
        for arg in args {
            let Some(ty) = ctx.map_tree(arg, &scope) else {
                return Vec::new();
            };
            if !snapshot.type_is_safe(ty) {
                return Vec::new();
            };
            mapped_args.push(ty);
        }
        let instantiation = snapshot.table.intern(
            UninternedType::Interface {
                decl,
                args: mapped_args,
            },
            false,
        );
        // The super pass is forbidden at extension-type elements
        // (:750-752); annotation sites allow it.
        let system = snapshot.system();
        match is_well_bounded(&system, &snapshot.facts, instantiation, allow_super) {
            Boundedness::NotWellBounded(issues) => {
                // The context face (`type_arguments_verifier.dart:427-465`):
                // where the super-bounded pass ran, every report carries the
                // inverted type's row — and the issues ARE the inverted
                // pass's there (`is_well_bounded` reports those), so the
                // display re-runs the same inversion. The extension-type
                // face skips the pass upstream too, and the raw-type row
                // keys on absent written arguments, which this capture's
                // sites always have.
                let inverted = allow_super
                    .then(|| snapshot.display(replace_top_bottom(&system, instantiation)));
                issues
                    .into_iter()
                    .map(|issue| BoundedIssue {
                        range: args[issue.index].range,
                        argument: snapshot.display(issue.argument),
                        parameter: snapshot.param_text(issue.parameter),
                        bound: snapshot.display(issue.bound),
                        inverted_type: inverted.clone(),
                    })
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    /// The G2 face (`_checkForEnumInstantiatedToBoundsIsNotWellBounded`,
    /// error_verifier.dart:5061-5079): instantiate the enum's type
    /// parameters to bounds and demand a well-bounded instantiation with
    /// the super pass allowed. Answers `true` when the report fires.
    pub(crate) fn check_enum_values(&mut self, strings: &'a StringTable, name: StringId) -> bool {
        let Some(capture) = self.index.containers.get(&name) else {
            return false;
        };
        if !capture.is_enum
            || capture.params.is_empty()
            || capture.bounds.iter().any(|(_, poisoned)| *poisoned)
        {
            return false;
        }
        // The alias-bound gate (#1351): the official preserves an alias
        // bound's spelling through the inverted (super-bounded) pass and
        // unaliases inside the subtype test — the decision order observes
        // the alias identity. The snapshot's model has no alias kinds, so
        // the map fully expands; the expansion answers the regular pass
        // and every non-divergent argument, but a self-referential bound
        // over a contravariant-occurrence alias passes the expanded
        // inverted pass where the official fails (`enum E<T extends
        // A<T>, U>` with `A<X> = X Function(X)`: the preserved spelling
        // tests `Never Function(dynamic) <: Never` inverted — false —
        // while the expansion's dynamic-topped shape passes). This gate
        // re-derives exactly that shape at the written level: a bound
        // applying a captured alias (arity-matched) to an argument
        // mentioning the enum's own parameter, where the alias's formals
        // — not its return — use the alias's own parameters (the
        // return-only alias `A<X> = X Function()` inverts clean and the
        // expanded pass answers it correctly).
        for (param, (bound, poisoned)) in capture.params.iter().zip(&capture.bounds) {
            let Some(tree) = bound else {
                continue;
            };
            if *poisoned || tree.poisoned {
                continue;
            }
            let Some(alias) = self.index.aliases.get(&tree.head) else {
                continue;
            };
            if tree.args.len() != alias.params.len() {
                continue;
            }
            let self_referential = tree
                .args
                .iter()
                .any(|arg| tree_mentions(arg, std::slice::from_ref(param)));
            let alias_params: Vec<StringId> =
                alias.params.iter().map(|(name, _, _)| *name).collect();
            if self_referential && alias.rhs.param_in_formals(&alias_params) {
                return true;
            }
        }
        let snapshot = self.snapshot(strings);
        let Some(decl) = snapshot.registry.get(&name).copied() else {
            return false;
        };
        let Some(params) = snapshot.decl_params.get(&decl) else {
            return false;
        };
        let system = snapshot.system();
        let instantiated =
            instantiate_type_formals_to_bounds(&snapshot.table, &snapshot.facts, params);
        let values_element = snapshot.table.intern(
            UninternedType::Interface {
                decl,
                args: instantiated.defaults,
            },
            false,
        );
        matches!(
            is_well_bounded(&system, &snapshot.facts, values_element, true),
            Boundedness::NotWellBounded(_)
        )
    }

    /// A container's captured type parameters (name, written bound, poison
    /// bit) in declaration order — the creation-inference face's parameter
    /// names (#1351). `None` when the head has no capture; owned, so the
    /// caller's later mutations never fight the borrow.
    pub(crate) fn container_params(
        &self,
        name: StringId,
    ) -> Option<Vec<(StringId, Option<TypeTree>, bool)>> {
        let capture = self.index.containers.get(&name)?;
        Some(
            capture
                .params
                .iter()
                .zip(&capture.bounds)
                .map(|(param, (bound, poisoned))| (*param, bound.clone(), *poisoned))
                .collect(),
        )
    }

    /// The captured right-hand function shape of the file's own alias
    /// `name` (`typedef P<T> = bool Function(T)`): the type-parameter
    /// triples in declaration order and the RHS's written pieces. `None`
    /// when `name` has no capture or its RHS is a named-type alias — the
    /// use-site faces that read a callable answer (`Predicate<T> p` invoked
    /// as `p(x)`) treat `None` as uncallable-shaped.
    /// This-file declarations only — a cross-file alias's capture lives in
    /// that unit's own parse; callers fall back silent.
    pub(crate) fn alias_fn_shape(
        &self,
        name: StringId,
    ) -> Option<(
        &[(StringId, Option<TypeTree>, bool)],
        Option<&TypeTree>,
        &[TypeTree],
        &[(StringId, TypeTree, bool)],
        Option<usize>,
    )> {
        let capture = self.index.aliases.get(&name)?;
        match &*capture.rhs {
            AliasRhs::Fn {
                ret,
                positional,
                named,
                required_positional,
                ..
            } => Some((
                &capture.params,
                ret.as_ref(),
                positional,
                named,
                *required_positional,
            )),
            AliasRhs::Named(_) => None,
        }
    }

    /// The captured right-hand named type of the file's own alias `name`
    /// (`typedef B = A<int, String>`): the type-parameter triples in
    /// declaration order and the named RHS tree. `None` when `name` has no
    /// capture or its RHS is a function type — the use-site callers read
    /// this to chase a co-alias's function shape (`B`'s invocable face is
    /// `A`'s RHS with `B`'s arguments substituted).
    pub(crate) fn alias_named_shape(
        &self,
        name: StringId,
    ) -> Option<(&[(StringId, Option<TypeTree>, bool)], &TypeTree)> {
        let capture = self.index.aliases.get(&name)?;
        match &*capture.rhs {
            AliasRhs::Named(tree) => Some((&capture.params, tree)),
            AliasRhs::Fn { .. } => None,
        }
    }

    /// The `strict_raw_type` display face (`_checkForRawTypeName`,
    /// type_arguments_verifier.dart:303-336): the written head with its
    /// instantiated-to-bounds arguments (`Box<dynamic>`), which the
    /// oracle's resolved-`type` display shows. Answers `None` when the
    /// head has no capture or a bound cannot render — the caller then
    /// falls back to the bare name.
    pub(crate) fn raw_display(
        &mut self,
        strings: &'a StringTable,
        head: StringId,
    ) -> Option<String> {
        if self
            .index
            .containers
            .get(&head)?
            .bounds
            .iter()
            .any(|(_, poisoned)| *poisoned)
        {
            return None;
        }
        let snapshot = self.snapshot(strings);
        let decl = *snapshot.registry.get(&head)?;
        let params = snapshot.decl_params.get(&decl)?;
        let name = snapshot.decl_text.get(&decl)?.clone();
        let defaults = instantiate_type_formals_to_bounds(&snapshot.table, &snapshot.facts, params);
        let rendered: Vec<String> = defaults
            .defaults
            .iter()
            .map(|&ty| snapshot.display(ty))
            .collect();
        Some(format!("{name}<{}>", rendered.join(", ")))
    }

    /// The extension-type `implements` faces of this file, in declaration
    /// then clause order. A face the snapshot cannot model leaves
    /// `verdict: None` (the caller may still decide it at name level); a
    /// face that is not an interface-typed superinterface the
    /// representation check reads (an extension type — the
    /// representation-not-supertype twin, unmodeled — or an invalid
    /// superinterface shape that reports its own code) yields no row.
    ///
    /// `declared` answers whether this file's element model declares a
    /// container of the name: such a name the snapshot did not register
    /// (a non-generic class — only generic heads are captured) would map
    /// to a same-named builtin or not at all, so a tree naming one is left
    /// to the name-level fallback.
    pub(crate) fn ext_face_rows(
        &mut self,
        strings: &'a StringTable,
        declared: impl Fn(StringId) -> bool,
    ) -> Vec<ExtFaceRow> {
        if self.index.ext_faces.is_empty() {
            return Vec::new();
        }
        let ext_faces = std::mem::take(&mut self.index.ext_faces);
        let invalid_heads = ["dynamic", "void", "Never", "Null", "FutureOr", "Function", "Record"];
        // Plan with the capture index borrowed, decide with the snapshot:
        // each row carries the tree pair the snapshot may map.
        let mut plan: Vec<(ExtFaceRow, Option<(&TypeTree, &TypeTree)>)> = Vec::new();
        {
            let index = &self.index;
            let unregistered = |name: StringId| {
                declared(name)
                    && !index.containers.contains_key(&name)
                    && !index.ext_names.contains(&name)
            };
            for (name, capture) in &ext_faces {
                let Some(rep) = capture.rep.as_ref() else {
                    continue;
                };
                let own_params: &[StringId] = index
                    .containers
                    .get(name)
                    .map_or(&[], |c| c.params.as_slice());
                let rep_bare = (rep.record.is_none()
                    && rep.function.is_none()
                    && !rep.poisoned
                    && !rep.nullable
                    && rep.args.is_empty())
                .then_some(rep.head);
                // The representation may name neither the extension type's
                // own parameters (their bounds are not modeled here) nor
                // any extension type (registered edge-less, so its own
                // faces would be missing from the walk), nor a container
                // the snapshot did not register.
                let rep_mappable = !tree_mentions(rep, own_params)
                    && !tree_mentions(rep, &index.ext_names)
                    && !tree_any_head(rep, &unregistered);
                for face in &capture.faces {
                    if face.poisoned || face.record.is_some() || face.function.is_some() {
                        continue;
                    }
                    let face_text = strings.resolve(face.head);
                    if face.nullable
                        || invalid_heads.contains(&face_text)
                        || index.ext_names.contains(&face.head)
                    {
                        continue;
                    }
                    let row = ExtFaceRow {
                        range: face.range,
                        face_bare: face.args.is_empty().then_some(face.head),
                        rep_bare,
                        verdict: None,
                        displays: None,
                    };
                    let mappable = rep_mappable
                        && !tree_mentions(face, own_params)
                        && !tree_any_head(face, &unregistered);
                    plan.push((row, mappable.then_some((rep, face))));
                }
            }
        }
        if plan.iter().all(|(_, trees)| trees.is_none()) {
            return plan.into_iter().map(|(row, _)| row).collect();
        }
        let snapshot = self.snapshot(strings);
        let ctx = snapshot.ctx();
        let empty = FxHashMap::default();
        plan.into_iter()
            .map(|(mut row, trees)| {
                if let Some((rep, face)) = trees
                    && let (Some(rep_ty), Some(face_ty)) =
                        (ctx.map_tree(rep, &empty), ctx.map_tree(face, &empty))
                    && snapshot.type_is_safe(rep_ty)
                    && matches!(snapshot.table.get(face_ty).kind(), TypeKind::Interface { .. })
                {
                    row.verdict = Some(snapshot.system().is_subtype(rep_ty, face_ty));
                    row.displays = Some((snapshot.display(face_ty), snapshot.display(rep_ty)));
                }
                row
            })
            .collect()
    }

    /// Builds (once) or returns the snapshot.
    fn snapshot<'s>(&'s mut self, strings: &'a StringTable) -> &'s Snapshot<'a> {
        if self.snapshot.is_none() {
            self.snapshot = Some(Snapshot::build(&self.index, strings));
        }
        self.snapshot.as_ref().unwrap_or_else(|| unreachable!())
    }
}

/// Core-declaration raw ids (the hierarchy requires `Object`, `Future`,
/// and bare `Function`).
const OBJECT_RAW: u32 = 1;
const FUTURE_RAW: u32 = 2;
const FUNCTION_RAW: u32 = 3;
const FIRST_BUILTIN_RAW: u32 = 4;
const FIRST_FILE_RAW: u32 = 100;

/// The curated dart:core builtin face: `(name, type-parameter names,
/// supertypes as written heads)`. The edges are the fixed public
/// declarations of the reference SDK (`sdk/lib/core/*.dart`): `sealed
/// class num implements Comparable<num>`, `abstract final class int
/// extends num` and its `double` twin (neither re-implements
/// `Comparable` — `int`'s `Comparable` instantiation is `Comparable<num>`,
/// so `int` does not satisfy `T extends Comparable<T>`), `abstract class String implements
/// Comparable<String>, Pattern`, and the collection roots (`List<E>`
/// implements `Iterable<E>`, `Set<E>` reachable to `Iterable<E>` through
/// its public face). A builtin missing from the table simply suppresses
/// the checks it would enter.
const BUILTINS: &[(&str, &[&str], &[&str])] = &[
    ("num", &[], &["Comparable<num>"]),
    ("int", &[], &["num"]),
    ("double", &[], &["num"]),
    ("String", &[], &["Comparable<String>", "Pattern"]),
    ("bool", &[], &[]),
    ("Pattern", &[], &[]),
    ("Comparable", &["T"], &[]),
    ("Iterable", &["E"], &[]),
    ("List", &["E"], &["Iterable<E>"]),
    ("Set", &["E"], &["Iterable<E>"]),
    ("Map", &["K", "V"], &[]),
];

/// The lazy `dartq_types` snapshot: the table/hierarchy pair, the name
/// registry, the parameter facts, and the safety closure. Single-threaded
/// like [`TypeTable`] (its interior `RefCell`).
struct Snapshot<'a> {
    strings: &'a StringTable,
    table: TypeTable,
    hierarchy: TypeHierarchy,
    /// Written name → declaration (in-file containers shadow the
    /// builtins, matching library-scoped shadowing).
    registry: FxHashMap<StringId, TypeDeclId>,
    /// Declaration → display text.
    decl_text: FxHashMap<TypeDeclId, String>,
    /// Declaration → its type parameters, declaration order.
    decl_params: FxHashMap<TypeDeclId, Vec<TypeParamId>>,
    /// Parameter → display text.
    param_text: FxHashMap<TypeParamId, String>,
    /// In-file declarations whose supertype closure is unfaithful (an
    /// open capture, or reachable from one). Absence = safe; the
    /// builtins are never here.
    unsafe_decls: FxHashSet<TypeDeclId>,
    /// The parameter bounds face (in-file and builtin parameters).
    facts: ParamFactsTable,
    /// The captured aliases' expansions (declaration → RHS type); empty
    /// when the file declares no function-type alias (#1351).
    alias_expansions: FxHashMap<TypeDeclId, TypeId>,
    /// Every captured alias's declaration id (the unexpanded-head guard).
    alias_decls: FxHashSet<TypeDeclId>,
    /// Site-side type-parameter allocation: an in-scope parameter name
    /// maps to one id for the file's lifetime (name-keyed — same-name
    /// parameters in distinct scopes share it, which can only answer
    /// `true` where the official answers `false`, the under-report
    /// direction).
    lazy_params: RefCell<FxHashMap<StringId, (TypeParamId, String)>>,
    next_param: Cell<u32>,
}

/// The tree-mapping context: the shared pieces of the build-time edge
/// mapping and the check-time argument mapping.
struct MapCtx<'a, 'b> {
    table: &'b TypeTable,
    strings: &'a StringTable,
    registry: &'b FxHashMap<StringId, TypeDeclId>,
    decl_params: &'b FxHashMap<TypeDeclId, Vec<TypeParamId>>,
    facts: &'b ParamFactsTable,
    /// The captured aliases' expansions: declaration → the RHS mapped in
    /// the alias's own parameter scope (a function or interface type
    /// whose type-parameter leaves are the alias's own parameter ids).
    /// `map_tree` expands an alias head by substituting the call-site
    /// arguments into it (#1351); an alias absent here never expands.
    alias_expansions: &'b FxHashMap<TypeDeclId, TypeId>,
    /// Every captured alias's declaration id: the guard that keeps an
    /// unexpanded alias head out of the interface arm — the hierarchy has
    /// no such declaration and a subtype walk would reject it.
    alias_decls: &'b FxHashSet<TypeDeclId>,
    lazy_params: &'b RefCell<FxHashMap<StringId, (TypeParamId, String)>>,
    next_param: &'b Cell<u32>,
}

impl MapCtx<'_, '_> {
    /// Allocates (once per name) the site-side parameter ids; the text
    /// travels with the id (the shared `param_text` face cannot take a
    /// mutable insert).
    fn param_scope(&self, names: &[StringId]) -> FxHashMap<StringId, TypeParamId> {
        let mut scope = FxHashMap::default();
        let mut lazy = self.lazy_params.borrow_mut();
        for &name in names {
            let id = lazy
                .entry(name)
                .or_insert_with(|| {
                    (
                        TypeParamId::from_raw(self.next_param.get()),
                        self.strings.resolve(name).to_string(),
                    )
                })
                .0;
            scope.insert(name, id);
        }
        scope
    }

    /// A bare generic inside a type: the official static type
    /// instantiates to bounds (`instantiateToBounds`,
    /// type_system.dart:719-731) — `List` reads as `List<dynamic>`, a
    /// bounded head as its bound.
    fn bounds_instantiation(&self, decl: TypeDeclId) -> Option<Vec<TypeId>> {
        let params = self.decl_params.get(&decl)?;
        Some(instantiate_type_formals_to_bounds(self.table, self.facts, params).defaults)
    }

    /// Maps a written tree onto the snapshot; `None` = the shape is not
    /// modelable (an unregistered head, an arity mismatch, a poisoned
    /// subtree) and the enclosing check suppresses.
    fn map_tree(
        &self,
        tree: &TypeTree,
        scope: &FxHashMap<StringId, TypeParamId>,
    ) -> Option<TypeId> {
        if tree.poisoned {
            return None;
        }
        let text = self.strings.resolve(tree.head);
        // A user-declared `class Never`/`Null`/`FutureOr` shadows the
        // dart:core face — the official resolves the written name through
        // the ordinary scope chain before the builtin answers. The
        // registry wins; only its miss falls to the builtin map.
        // (`dynamic`/`void` are keywords and can never be declared.)
        let shadowed =
            matches!(text, "Null" | "Never" | "FutureOr") && self.registry.contains_key(&tree.head);
        match text {
            "dynamic" if !shadowed => Some(self.table.intern(UninternedType::Dynamic, false)),
            "void" if !shadowed => Some(self.table.intern(UninternedType::Void, false)),
            "Null" if !shadowed => Some(self.table.intern(UninternedType::Null, false)),
            "Never" if !shadowed => Some(self.table.intern(UninternedType::Never, tree.nullable)),
            "FutureOr" if !shadowed => {
                if tree.args.len() != 1 {
                    return None;
                }
                let base = self.map_tree(&tree.args[0], scope)?;
                Some(
                    self.table
                        .intern(UninternedType::FutureOr { base }, tree.nullable),
                )
            }
            _ => match self.registry.get(&tree.head) {
                Some(&decl) => {
                    // An alias declaration without an expansion (a chain
                    // still pending, a cycle, a poisoned RHS) is not
                    // modelable as an interface: the hierarchy has no
                    // such declaration. `None` = the enclosing check
                    // suppresses (the under-report direction).
                    if self.alias_decls.contains(&decl) {
                        return None;
                    }
                    // An alias head expands to its captured RHS with the
                    // written arguments substituted in (the official
                    // resolves the alias element and instantiates it —
                    // `instantiateTypeAliasToBounds` for the bare form,
                    // the alias's own `instantiate` otherwise). The
                    // decision order observes only the expanded shape;
                    // the alias identity itself never outlives the map
                    // (the under-report posture: an unexpanded alias
                    // suppresses its check).
                    if let Some(&expansion) = self.alias_expansions.get(&decl) {
                        let params = self.decl_params.get(&decl)?;
                        let args = if tree.args.len() == params.len() {
                            let mut args = Vec::with_capacity(tree.args.len());
                            for arg in &tree.args {
                                args.push(self.map_tree(arg, scope)?);
                            }
                            args
                        } else if tree.args.is_empty() {
                            self.bounds_instantiation(decl)?
                        } else {
                            return None;
                        };
                        let map: Vec<(TypeParamId, TypeId)> =
                            params.iter().copied().zip(args).collect();
                        return Some(self.table.substitute(expansion, &map));
                    }
                    let params = self.decl_params.get(&decl)?;
                    let args = if tree.args.len() == params.len() {
                        let mut args = Vec::with_capacity(tree.args.len());
                        for arg in &tree.args {
                            args.push(self.map_tree(arg, scope)?);
                        }
                        args
                    } else if tree.args.is_empty() {
                        self.bounds_instantiation(decl)?
                    } else {
                        return None;
                    };
                    Some(
                        self.table
                            .intern(UninternedType::Interface { decl, args }, tree.nullable),
                    )
                }
                None => scope.get(&tree.head).map(|&parameter| {
                    self.table
                        .intern(UninternedType::TypeParameter { parameter }, tree.nullable)
                }),
            },
        }
    }
}

impl<'a> Snapshot<'a> {
    #[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
    fn build(index: &BoundsIndex, strings: &'a StringTable) -> Self {
        let table = TypeTable::new();
        let object = TypeDeclId::from_raw(OBJECT_RAW);
        let future = TypeDeclId::from_raw(FUTURE_RAW);
        let function = TypeDeclId::from_raw(FUNCTION_RAW);
        let mut builder = TypeHierarchy::builder(CoreDecls {
            object,
            future,
            function,
            record: None,
        })
        .decl(object, Vec::new())
        .decl(future, vec![TypeParamId::from_raw(1)])
        .decl(function, Vec::new());
        let mut registry = FxHashMap::default();
        let mut decl_text: FxHashMap<TypeDeclId, String> = FxHashMap::default();
        let mut decl_params: FxHashMap<TypeDeclId, Vec<TypeParamId>> = FxHashMap::default();
        let mut param_text: FxHashMap<TypeParamId, String> = FxHashMap::default();
        let mut facts = ParamFactsTable::new();
        // Decl raw ids 4.. for the builtins, 100.. for the in-file
        // containers; parameter ids 2.. (1 is `Future`'s).
        let mut next_decl = FIRST_BUILTIN_RAW;
        let next_param = Cell::new(2_u32);

        // The builtin face: fresh parameter ids, the `Object` edge
        // arrives implicitly at the build.
        let mut builtin_edges: Vec<(TypeDeclId, Vec<String>)> = Vec::new();
        for (name, params, supers) in BUILTINS {
            let decl = TypeDeclId::from_raw(next_decl);
            next_decl += 1;
            let mut param_ids = Vec::with_capacity(params.len());
            for param in *params {
                let id = TypeParamId::from_raw(next_param.get());
                next_param.set(next_param.get() + 1);
                param_ids.push(id);
                param_text.insert(id, (*param).to_string());
                facts.set(
                    id,
                    ParamFacts {
                        variance: Variance::Covariant,
                        bound: None,
                    },
                );
            }
            registry.insert(strings.intern(name), decl);
            decl_text.insert(decl, (*name).to_string());
            builder = builder.decl(decl, param_ids.clone());
            decl_params.insert(decl, param_ids);
            builtin_edges.push((
                decl,
                supers.iter().map(|text| (*text).to_string()).collect(),
            ));
        }

        // The in-file containers, two passes: register every declaration
        // first (so a bound or supertype may reference any co-declared
        // container), then map the bounds against each container's own
        // parameter scope (a bound may reference its co-parameters). A
        // bound that fails to map reads `dynamic` — the
        // always-satisfied bound, so the parameter's check can never
        // fire.
        let mut file_scopes: FxHashMap<TypeDeclId, FxHashMap<StringId, TypeParamId>> =
            FxHashMap::default();
        next_decl = FIRST_FILE_RAW;
        for (name, capture) in &index.containers {
            let decl = TypeDeclId::from_raw(next_decl);
            next_decl += 1;
            let mut param_ids = Vec::with_capacity(capture.params.len());
            let mut scope: FxHashMap<StringId, TypeParamId> = FxHashMap::default();
            for param in &capture.params {
                let id = TypeParamId::from_raw(next_param.get());
                next_param.set(next_param.get() + 1);
                param_ids.push(id);
                scope.insert(*param, id);
                param_text.insert(id, strings.resolve(*param).to_string());
            }
            registry.insert(*name, decl);
            decl_text.insert(decl, strings.resolve(*name).to_string());
            builder = builder.decl(decl, param_ids.clone());
            decl_params.insert(decl, param_ids);
            file_scopes.insert(decl, scope);
        }

        // The file's non-generic extension types register edge-less (a
        // generic one is a container capture above): a face's type
        // argument may name them (`implements Comparable<Score>`). Their
        // closure is never walked as a representation — the face check
        // excludes representations that mention any extension type.
        for name in &index.ext_names {
            if index.containers.contains_key(name) {
                continue;
            }
            let decl = TypeDeclId::from_raw(next_decl);
            next_decl += 1;
            registry.insert(*name, decl);
            decl_text.insert(decl, strings.resolve(*name).to_string());
            builder = builder.decl(decl, Vec::new());
            decl_params.insert(decl, Vec::new());
        }

        // The fresh in-file declarations are all registered by now. The
        // bounds map first, against a scratch facts face (the real one is
        // being written; a bare generic inside a bound then instantiates
        // to `dynamic`, the under-report direction), then the mapped
        // bounds apply.
        let scratch_facts = ParamFactsTable::new();
        // The pre-alias map passes (container bounds, supertype edges)
        // run with no expansions and no alias declarations in the
        // registry: the aliases have not registered yet, and a bound
        // referencing one is not modelable here — the container takes
        // the under-report direction.
        let empty_alias_expansions: FxHashMap<TypeDeclId, TypeId> = FxHashMap::default();
        let empty_alias_decls: FxHashSet<TypeDeclId> = FxHashSet::default();
        let ctx = MapCtx {
            table: &table,
            strings,
            registry: &registry,
            decl_params: &decl_params,
            facts: &scratch_facts,
            alias_expansions: &empty_alias_expansions,
            alias_decls: &empty_alias_decls,
            lazy_params: &RefCell::new(FxHashMap::default()),
            next_param: &next_param,
        };
        let mut mapped_bounds: Vec<(TypeParamId, TypeId)> = Vec::new();
        for (name, capture) in &index.containers {
            let Some(&decl) = registry.get(name) else {
                continue;
            };
            let Some(scope) = file_scopes.get(&decl) else {
                continue;
            };
            for (&param, (bound, _)) in capture.params.iter().zip(&capture.bounds) {
                let Some(&id) = scope.get(&param) else {
                    continue;
                };
                let mapped = match bound {
                    Some(tree) => ctx
                        .map_tree(tree, scope)
                        .unwrap_or_else(|| table.intern(UninternedType::Dynamic, false)),
                    None => table.intern(UninternedType::Dynamic, false),
                };
                mapped_bounds.push((id, mapped));
            }
        }
        for (id, bound) in mapped_bounds {
            facts.set(
                id,
                ParamFacts {
                    variance: Variance::Covariant,
                    bound: Some(bound),
                },
            );
        }

        // The edge sweeps run against the real facts face (a bare
        // generic in a supertype instantiates to its true bounds);
        // map the builtin supertype edges through the complete
        // registry (edges are written heads like `Comparable<num>`).
        let ctx = MapCtx {
            table: &table,
            strings,
            registry: &registry,
            decl_params: &decl_params,
            facts: &facts,
            alias_expansions: &empty_alias_expansions,
            alias_decls: &empty_alias_decls,
            lazy_params: &RefCell::new(FxHashMap::default()),
            next_param: &next_param,
        };
        for (decl, supers) in &builtin_edges {
            for text in supers {
                if let Some(tree) = parse_written_type(text, strings)
                    && let Some(ty) = ctx.map_tree(&tree, &FxHashMap::default())
                {
                    builder = builder.supertype(*decl, ty);
                }
            }
        }
        // The in-file supertype edges. A tree that fails to map (or a
        // nullable one — the hierarchy rejects nullable supertypes)
        // leaves the capture unfaithful: the declaration goes unsafe.
        // Faithful edges also join the hierarchy — the subtype walk reads
        // the closure, so an argument satisfies its bound through an
        // in-file edge only when the edge is fed.
        let mut edges_of: FxHashMap<TypeDeclId, Vec<TypeDeclId>> = FxHashMap::default();
        let mut unsafe_decls: FxHashSet<TypeDeclId> = FxHashSet::default();
        for (name, capture) in &index.containers {
            let Some(&decl) = registry.get(name) else {
                continue;
            };
            let Some(scope) = file_scopes.get(&decl) else {
                continue;
            };
            if capture.open {
                unsafe_decls.insert(decl);
                continue;
            }
            let mut targets = Vec::new();
            let mut edge_types = Vec::new();
            let mut faithful = true;
            for tree in &capture.supers {
                if tree.nullable {
                    faithful = false;
                    continue;
                }
                match ctx.map_tree(tree, scope) {
                    Some(ty) => match table.get(ty).kind() {
                        TypeKind::Interface { decl: target, .. } => {
                            targets.push(target);
                            edge_types.push(ty);
                        }
                        // A valid hierarchy edge is always an interface;
                        // anything else means the capture was not one.
                        _ => faithful = false,
                    },
                    None => faithful = false,
                }
            }
            if faithful {
                for ty in edge_types {
                    builder = builder.supertype(decl, ty);
                }
                edges_of.insert(decl, targets);
            } else {
                unsafe_decls.insert(decl);
            }
        }

        // Freeze the snapshot. An invalid captured graph (a cycle
        // through invalid sources) falls back to the bare core; the
        // declarations it drops from the hierarchy are marked unsafe so
        // no walk can ever reach them.
        let hierarchy = match builder.build(&table) {
            Ok(hierarchy) => hierarchy,
            Err(_) => TypeHierarchy::builder(CoreDecls {
                object,
                future,
                function,
                record: None,
            })
            .decl(object, Vec::new())
            .decl(future, vec![TypeParamId::from_raw(1)])
            .decl(function, Vec::new())
            .build(&table)
            .expect("the bare core hierarchy is always valid"),
        };
        for decl in file_scopes.keys() {
            if hierarchy.decl(*decl).is_none() {
                unsafe_decls.insert(*decl);
            }
        }
        // The safety closure: a declaration is safe when its capture is
        // closed and every edge target is safe. Iterate to a fixpoint —
        // a cycle never resolves, the unsafe direction.
        let mut changed = true;
        while changed {
            changed = false;
            for (decl, targets) in &edges_of {
                if unsafe_decls.contains(decl) {
                    continue;
                }
                if targets.iter().any(|target| unsafe_decls.contains(target)) {
                    unsafe_decls.insert(*decl);
                    changed = true;
                }
            }
        }

        // The captured aliases (#1351): registered for the expansion map
        // only — an alias never enters a subtype walk as itself, so it
        // needs no hierarchy face. Each RHS maps in the alias's own
        // parameter scope; a bound or RHS referencing a not-yet-expanded
        // alias defers to the next round (a chain terminates, a cycle
        // drops — the under-report direction).
        let mut alias_expansions: FxHashMap<TypeDeclId, TypeId> = FxHashMap::default();
        let mut alias_decls: FxHashSet<TypeDeclId> = FxHashSet::default();
        let mut alias_scopes: FxHashMap<TypeDeclId, FxHashMap<StringId, TypeParamId>> =
            FxHashMap::default();
        for (name, capture) in &index.aliases {
            let decl = TypeDeclId::from_raw(next_decl);
            next_decl += 1;
            let mut param_ids = Vec::with_capacity(capture.params.len());
            let mut scope: FxHashMap<StringId, TypeParamId> = FxHashMap::default();
            for param in &capture.params {
                let id = TypeParamId::from_raw(next_param.get());
                next_param.set(next_param.get() + 1);
                param_ids.push(id);
                scope.insert(param.0, id);
                param_text.insert(id, strings.resolve(param.0).to_string());
            }
            registry.insert(*name, decl);
            decl_params.insert(decl, param_ids);
            alias_scopes.insert(decl, scope);
            alias_decls.insert(decl);
        }
        let mut pending: Vec<(StringId, &AliasCapture)> =
            index.aliases.iter().map(|(name, c)| (*name, c)).collect();
        let mut rounds = index.aliases.len() + 1;
        while !pending.is_empty() && rounds > 0 {
            rounds -= 1;
            let mut deferred: Vec<(StringId, &AliasCapture)> = Vec::new();
            for (name, capture) in pending.drain(..) {
                let Some(&decl) = registry.get(&name) else {
                    continue;
                };
                let Some(scope) = alias_scopes.get(&decl) else {
                    continue;
                };
                // The expansion and the parameter-bounds mapping run in a
                // scope that drops the `ctx` before the results apply —
                // the map and the facts face update after the borrow.
                let outcome = {
                    let ctx = MapCtx {
                        table: &table,
                        strings,
                        registry: &registry,
                        decl_params: &decl_params,
                        facts: &scratch_facts,
                        alias_expansions: &alias_expansions,
                        alias_decls: &alias_decls,
                        lazy_params: &RefCell::new(FxHashMap::default()),
                        next_param: &next_param,
                    };
                    match expand_alias(capture, &ctx, scope) {
                        Some(ty) => {
                            let mapped_bounds = decl_params
                                .get(&decl)
                                .into_iter()
                                .flatten()
                                .zip(capture.params.iter())
                                .map(|(&id, (_, bound, poisoned))| {
                                    let mapped = match (bound, poisoned) {
                                        (Some(tree), false) => {
                                            ctx.map_tree(tree, scope).unwrap_or_else(|| {
                                                table.intern(UninternedType::Dynamic, false)
                                            })
                                        }
                                        _ => table.intern(UninternedType::Dynamic, false),
                                    };
                                    (id, mapped)
                                })
                                .collect::<Vec<_>>();
                            Some((ty, mapped_bounds))
                        }
                        None => None,
                    }
                };
                match outcome {
                    Some((ty, mapped_bounds)) => {
                        alias_expansions.insert(decl, ty);
                        for (id, mapped) in mapped_bounds {
                            facts.set(
                                id,
                                ParamFacts {
                                    variance: Variance::Covariant,
                                    bound: Some(mapped),
                                },
                            );
                        }
                    }
                    None => deferred.push((name, capture)),
                }
            }
            if deferred.len() == pending.len() && !deferred.is_empty() {
                break; // no progress: an alias cycle, the drops are final
            }
            pending = deferred;
        }

        Snapshot {
            strings,
            table,
            hierarchy,
            registry,
            decl_text,
            decl_params,
            param_text,
            unsafe_decls,
            facts,
            alias_expansions,
            alias_decls,
            lazy_params: RefCell::new(FxHashMap::default()),
            next_param,
        }
    }

    fn system(&self) -> TypeSystem<'_> {
        TypeSystem::new(&self.table, &self.hierarchy)
    }

    fn ctx(&self) -> MapCtx<'_, '_> {
        MapCtx {
            table: &self.table,
            strings: self.strings,
            registry: &self.registry,
            decl_params: &self.decl_params,
            facts: &self.facts,
            alias_expansions: &self.alias_expansions,
            alias_decls: &self.alias_decls,
            lazy_params: &self.lazy_params,
            next_param: &self.next_param,
        }
    }

    fn param_text(&self, parameter: TypeParamId) -> String {
        if let Some(text) = self.param_text.get(&parameter) {
            return text.clone();
        }
        // A site-side lazy parameter: the text travels with the id in
        // the lazy allocation map.
        for (id, text) in self.lazy_params.borrow().values() {
            if *id == parameter {
                return text.clone();
            }
        }
        String::new()
    }

    /// Whether a type's supertype closure over the snapshot's edges is
    /// faithful: the walk may only start from declarations whose edges
    /// are exact (`FutureOr` recurses into its base through rule 6; the
    /// interface arguments are only ever compared by equality, which the
    /// structural interning answers soundly).
    fn type_is_safe(&self, ty: TypeId) -> bool {
        match self.table.get(ty).kind() {
            TypeKind::FutureOr { base } => self.type_is_safe(base),
            TypeKind::Interface { decl, .. } => !self.unsafe_decls.contains(&decl),
            _ => true,
        }
    }

    /// The `getDisplayString` face of a snapshot type: the declaration's
    /// display name, the argument list, the `?` projection.
    fn display(&self, ty: TypeId) -> String {
        let t = self.table.get(ty);
        let suffix = if t.is_nullable() { "?" } else { "" };
        match t.kind() {
            TypeKind::Never => format!("Never{suffix}"),
            // The error sentinel never enters this face (poisoned trees
            // suppress first); render the neutral top type.
            TypeKind::Dynamic | TypeKind::Invalid => "dynamic".to_string(),
            TypeKind::Void => "void".to_string(),
            TypeKind::Null => "Null".to_string(),
            TypeKind::TypeParameter { parameter } => {
                format!("{}{suffix}", self.param_text(parameter))
            }
            TypeKind::FutureOr { base } => {
                format!("FutureOr<{}>{suffix}", self.display(base))
            }
            TypeKind::Interface { decl, args } => {
                let name = self.decl_text.get(&decl).cloned().unwrap_or_default();
                if args.is_empty() {
                    format!("{name}{suffix}")
                } else {
                    let rendered: Vec<String> = (0..args.len())
                        .map(|i| self.display(self.table.arg(args.offset() + i)))
                        .collect();
                    format!("{name}<{}>{suffix}", rendered.join(", "))
                }
            }
            // Function and record kinds never enter this face (the
            // capture poisons on their syntax).
            TypeKind::Function { .. } => format!("Function{suffix}"),
            TypeKind::Record { .. } => format!("Record{suffix}"),
        }
    }
}

/// Parses a written builtin supertype head (`Comparable<num>`, or the
/// bare `num`) into a tree, for the curated builtin edge table.
fn parse_written_type(text: &str, strings: &StringTable) -> Option<TypeTree> {
    let range = SourceRange {
        offset: 0,
        length: 0,
    };
    match text.split_once('<') {
        Some((head, rest)) => {
            let args_text = rest.strip_suffix('>')?;
            let mut tree = TypeTree::leaf(strings.intern(head), false, range);
            for arg in args_text.split(',') {
                tree.args
                    .push(TypeTree::leaf(strings.intern(arg.trim()), false, range));
            }
            Some(tree)
        }
        None => Some(TypeTree::leaf(strings.intern(text), false, range)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(offset: u32, length: u32) -> SourceRange {
        SourceRange { offset, length }
    }

    /// One unbounded type variable (`class C<T>`): the window opens and
    /// closes around the completion event, and the container's begin event
    /// adopts the latch.
    fn unbounded_variable(bounds: &mut Bounds<'_>, name: StringId) {
        bounds.begin_type_variables();
        bounds.end_type_variable(name, false);
        bounds.end_type_variables();
    }

    /// One parameterized type reference (`Box<int>`) inside the open
    /// window: the head travels in the group's begin event (the engine's
    /// `pending_type_head`), the arguments fire `handle_type` inside, and
    /// the parser's trailing head re-fire merges into the completed node —
    /// a no-op for a non-nullable head, omitted here.
    fn parameterized(bounds: &mut Bounds<'_>, head: StringId, arg: StringId, head_offset: u32) {
        bounds.begin_group(Some((head, range(head_offset, 3))), true);
        bounds.leaf(arg, false, range(head_offset + 4, 3));
        bounds.end_group(range(head_offset + 7, 1));
    }

    /// Closes the site capture and peels the root the way the engine's
    /// `end_type_arguments` does: the check receives the head and its
    /// argument subtrees, not the whole root tree.
    fn close_site(bounds: &mut Bounds<'_>, end: SourceRange) -> (StringId, Vec<TypeTree>) {
        let mut roots = bounds
            .end_group(end)
            .expect("the site capture closes with its root");
        let site = roots.pop().expect("the site root");
        (site.head, site.args)
    }

    /// The snapshot declares every registered name into the hierarchy —
    /// builtin and in-file alike. Missing declarations fail the builder's
    /// `validate_edge` (`UndeclaredDecl`), the build falls back to the
    /// bare core, and every check silently suppresses.
    #[test]
    fn snapshot_declares_builtin_and_in_file_decls() {
        let strings = StringTable::new();
        let c = strings.intern("C");
        let t = strings.intern("T");
        let int = strings.intern("int");
        let mut bounds = Bounds::new();
        // `class C<T extends int> {}`
        bounds.begin_type_variables();
        bounds.leaf(int, false, range(13, 3));
        bounds.end_type_variable(t, true);
        bounds.end_type_variables();
        bounds.note_container(Some(c), 6, 21, false, false);

        let snapshot = bounds.snapshot(&strings);
        let decl = snapshot
            .registry
            .get(&c)
            .copied()
            .expect("in-file decl registered");
        let info = snapshot
            .hierarchy
            .decl(decl)
            .expect("in-file decl declared in the hierarchy");
        assert_eq!(info.type_parameters, snapshot.decl_params[&decl]);
        let list = *snapshot
            .registry
            .get(&strings.intern("List"))
            .expect("builtin registered");
        assert!(
            snapshot.hierarchy.decl(list).is_some(),
            "builtin decl declared in the hierarchy"
        );
    }

    /// An argument satisfying its parameter's bound through an in-file
    /// supertype edge stays silent: `class IntBox<X> extends Box<int>`
    /// makes `C<IntBox>` well-bounded for `C<T extends Box<int>>`.
    /// Without the in-file edge feed the hierarchy closure holds only the
    /// implicit `Object` and the site false-positives.
    #[test]
    fn in_file_supertype_edge_satisfies_the_bound() {
        let strings = StringTable::new();
        let box_ = strings.intern("Box");
        let t = strings.intern("T");
        let int = strings.intern("int");
        let int_box = strings.intern("IntBox");
        let x = strings.intern("X");
        let c = strings.intern("C");
        let mut bounds = Bounds::new();
        // `class Box<T> {}`
        unbounded_variable(&mut bounds, t);
        bounds.note_container(Some(box_), 6, 7, false, false);
        // `class IntBox<X> extends Box<int> {}`
        unbounded_variable(&mut bounds, x);
        bounds.note_container(Some(int_box), 18, 19, false, false);
        bounds.open_header(int_box);
        parameterized(&mut bounds, box_, int, 32);
        bounds.header_clause(Clause::Extends(1));
        bounds.finish_header();
        // `class C<T extends Box<int>> {}`
        bounds.begin_type_variables();
        parameterized(&mut bounds, box_, int, 52);
        bounds.end_type_variable(t, true);
        bounds.end_type_variables();
        bounds.note_container(Some(c), 48, 62, false, false);
        // `void f(C<IntBox> c) {}`
        bounds.begin_group(Some((c, range(70, 1))), true);
        bounds.leaf(int_box, false, range(72, 6));
        let (head, args) = close_site(&mut bounds, range(78, 1));
        let issues = bounds.check_site(&strings, head, &args, &[]);
        assert!(issues.is_empty());
    }

    /// A genuine violation against an in-file bound reports at the
    /// argument: `String` does not satisfy `C<T extends Box<int>>`'s
    /// bound, and the issue carries the written argument, the parameter,
    /// and the substituted bound.
    #[test]
    fn in_file_bound_violation_reports_at_the_argument() {
        let strings = StringTable::new();
        let box_ = strings.intern("Box");
        let t = strings.intern("T");
        let int = strings.intern("int");
        let string = strings.intern("String");
        let c = strings.intern("C");
        let mut bounds = Bounds::new();
        // `class Box<T> {}`
        unbounded_variable(&mut bounds, t);
        bounds.note_container(Some(box_), 6, 7, false, false);
        // `class C<T extends Box<int>> {}`
        bounds.begin_type_variables();
        parameterized(&mut bounds, box_, int, 21);
        bounds.end_type_variable(t, true);
        bounds.end_type_variables();
        bounds.note_container(Some(c), 18, 31, false, false);
        // `void f(C<String> c) {}`
        bounds.begin_group(Some((c, range(39, 1))), true);
        bounds.leaf(string, false, range(41, 6));
        let (head, args) = close_site(&mut bounds, range(47, 1));
        let issues = bounds.check_site(&strings, head, &args, &[]);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].argument, "String");
        assert_eq!(issues[0].parameter, "T");
        assert_eq!(issues[0].bound, "Box<int>");
        assert_eq!(issues[0].range.offset, 41);
    }
}
