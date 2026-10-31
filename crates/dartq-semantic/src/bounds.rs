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
//! routes `handle_type` and the group events into the open sink; a
//! function-type node maps to a first-class function type (the shape a
//! written bound spells, `class C<X extends void Function()>`), while any
//! shape the face cannot model (record types, a headless group) poisons
//! the enclosing tree, and a poisoned tree suppresses its check — the
//! under-report direction, never a false positive.
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

use std::cell::{Cell, Ref, RefCell};

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
pub(crate) use crate::capture::{FnTree, TreeSink, TypeTree};

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
    /// returns the claimed trees (`On` clauses keep theirs too — the
    /// bounds face checks constraint annotations; the supertype set
    /// excludes them at the caller).
    fn clause(&mut self, clause: Clause) -> Vec<TypeTree> {
        let expected = match clause {
            Clause::Extends(count) | Clause::Implements(count) | Clause::On(count) => Some(count),
            Clause::With => None,
        };
        let since = self.sink.unclaimed(self.claimed);
        if let Some(expected) = expected
            && since.len() != expected
        {
            self.open = true;
        }
        let claimed = since.to_vec();
        self.claimed = self.sink.done.len();
        claimed
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
    /// Every clause tree the header wrote, `on` clauses included — the
    /// bounds face checks each written supertype/constraint annotation
    /// independently (`checkNamedType` runs at every `NamedType`;
    /// `_shouldAllowSuperBoundedTypes` denies the super pass on all the
    /// clause contexts, `type_arguments_verifier.dart:737-748`).
    bound_sites: Vec<TypeTree>,
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
        /// The RHS's own type variables (`typedef F<X> = void Function<Y
        /// extends X>()`): a generic function-type RHS declares ITS
        /// variables — the alias's parameters are the declaration's, the
        /// function type's are these (#2838, the analyzer element model).
        /// The bound trees map in the alias's own parameter scope.
        type_params: Vec<(StringId, Option<TypeTree>, bool)>,
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
                type_params: _,
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
                type_params: _,
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
            type_params,
            nullable,
        } => {
            // The RHS's own type variables ride the signature (#2838):
            // their ids were allocated into the alias's scope at
            // registration (the RHS trees' `Y` leaves resolve through the
            // same map), and the interned function type declares them —
            // the analyzer's GenericFunctionType element.
            let type_parameters: Vec<TypeParamId> = type_params
                .iter()
                .map(|(var, _, _)| scope.get(var).copied())
                .collect::<Option<Vec<_>>>()?;
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
                        type_parameters,
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
    /// A type-variables window closed after the RHS parse started: the
    /// RHS function type's OWN group (`void Function<Y extends X>()`),
    /// latched here — the declaring `params` stay (#2838).
    rhs_vars: Vec<(StringId, Option<TypeTree>, bool)>,
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
    /// The checked annotation's own span (the head through its argument
    /// group) — the context row's anchor where the super-bounded pass
    /// ran (the verifier anchors `contextMessages` at the whole
    /// `NamedType`, `type_arguments_verifier.dart:401-406`).
    pub site: SourceRange,
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
    pub(crate) inverted_type: Option<String>,
    /// The raw head's instantiate-to-bounds display when the checked
    /// annotation spelled no type arguments (`extends A` resolving to
    /// `A<B<dynamic>>`) — the `contextMessages` row's
    /// `"The raw type was instantiated as '...'"` text
    /// (`type_arguments_verifier.dart:441-446`). `None` at every
    /// written-argument site.
    pub(crate) raw_instantiated: Option<String>,
}

/// One raw-constructor inference failure (the [`Bounds::
/// check_creation_inference`] answer, #2831 P1): the failing parameter
/// with the message pieces of its two rows — the `could_not_infer` detail
/// and the creation-site `type_argument_not_matching_bounds` twin.
pub(crate) struct CreationInferenceRow {
    /// The parameter's own name.
    pub(crate) parameter: String,
    /// The greatest-closure seed the solver chose.
    pub(crate) seed: String,
    /// The declared bound as written (the parameter's own spellings).
    pub(crate) raw_bound: String,
    /// The bound substituted with every seed.
    pub(crate) bound: String,
}

/// `greatestClosure` (type_system.dart:592-620 over least_greatest_
/// closure.dart:12-105): every target type parameter eliminated to the
/// top type in covariant positions and the bottom type in contravariant
/// ones. Interface arguments keep the incoming variance — an invariant
/// parameter's argument included (`ReplacementVisitor.visitTypeArgument`
/// visits unperturbed), which is why `FutureOr<A<X>>` closes to
/// `FutureOr<A<Object?>>` for `class A<X extends FutureOr<A<X>>>`.
/// Function signatures eliminate through their columns (return covariant,
/// formals contravariant); record kinds never mention a target (a bound
/// whose tree was one reads `dynamic` at the snapshot build), so they
/// pass through unchanged.
fn greatest_closure(
    sys: &TypeSystem<'_>,
    ty: TypeId,
    targets: &FxHashSet<TypeParamId>,
    covariant: bool,
    depth: u32,
) -> TypeId {
    const DEPTH_LIMIT: u32 = 100;
    if depth > DEPTH_LIMIT {
        return ty;
    }
    let table = sys.table();
    match table.get(ty).kind() {
        TypeKind::TypeParameter { parameter } if targets.contains(&parameter) => {
            let nullable = table.get(ty).is_nullable();
            if covariant {
                table.intern(
                    UninternedType::Interface {
                        decl: sys.hierarchy().core().object,
                        args: Vec::new(),
                    },
                    true,
                )
            } else {
                // `uniteNullabilities`: a nullable occurrence unites with
                // the bottom to `Never?` (the table normalizes to `Null`).
                table.intern(UninternedType::Never, nullable)
            }
        }
        TypeKind::Interface { decl, args } => {
            let new_args: Vec<TypeId> = (0..args.len())
                .map(|i| {
                    greatest_closure(
                        sys,
                        table.arg(args.offset() + i),
                        targets,
                        covariant,
                        depth + 1,
                    )
                })
                .collect();
            table.intern(
                UninternedType::Interface {
                    decl,
                    args: new_args,
                },
                table.get(ty).is_nullable(),
            )
        }
        TypeKind::FutureOr { base } => {
            let new_base = greatest_closure(sys, base, targets, covariant, depth + 1);
            if new_base == base {
                ty
            } else {
                table.intern(
                    UninternedType::FutureOr { base: new_base },
                    table.get(ty).is_nullable(),
                )
            }
        }
        TypeKind::Function { signature } => {
            let sig = table.signature(signature);
            let ret = greatest_closure(sys, sig.return_type, targets, covariant, depth + 1);
            let positional = (0..sig.positional.len())
                .map(|i| {
                    greatest_closure(
                        sys,
                        table.arg(sig.positional.offset() + i),
                        targets,
                        !covariant,
                        depth + 1,
                    )
                })
                .collect::<Vec<_>>();
            let named = table
                .named_members(sig.named)
                .into_iter()
                .map(|mut member| {
                    member.ty = greatest_closure(sys, member.ty, targets, !covariant, depth + 1);
                    member
                })
                .collect::<Vec<_>>();
            table
                .intern_function(
                    UninternedFunction {
                        return_type: ret,
                        type_parameters: table.function_type_parameters(sig.type_parameters),
                        positional,
                        named,
                        required_positional: sig.required_positional,
                    },
                    table.get(ty).is_nullable(),
                )
                .unwrap_or(ty)
        }
        _ => ty,
    }
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
    /// The enclosing type-variables groups' in-flight capture. A bound's
    /// own function-type variables (`void f<T extends void Function<X>>>()`)
    /// open a group INSIDE the declaring header's group; their events must
    /// not disturb the header's window, sink, or latch (#2897).
    frames: Vec<(Vec<(StringId, Option<TypeTree>, bool)>, Option<TreeSink>)>,
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
    /// The lazy `dartq_types` snapshot (interior-mutable so the
    /// `&self` compare faces — #2838's raw-type arm — can build it
    /// mid-parse; the rebuild-aware count keeps a later teardown build
    /// honest about declarations captured since).
    snapshot: RefCell<Option<Snapshot<'a>>>,
    /// The `containers` + `aliases` count the built snapshot froze (#2831):
    /// both maps only ever grow, so a mismatch means the capture index
    /// gained declarations and the snapshot rebuilds — a mid-parse
    /// consumer (the raw-constructor inference face) must not leave the
    /// teardown's checks a build that predates later declarations.
    snapshot_decl_count: Cell<usize>,
}

impl<'a> Bounds<'a> {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// A `<...>` type-variables group opened: its variables' bounds are
    /// captured. The outermost group also invalidates any unclaimed
    /// window latch; a nested group (a bound's function-type variables)
    /// keeps it — the enclosing header's adopter has not run yet (#2897).
    pub(crate) fn begin_type_variables(&mut self) {
        if self.frames.is_empty() {
            self.pending = None;
        }
        self.frames
            .push((std::mem::take(&mut self.window), self.bound_sink.take()));
        self.bound_sink = Some(TreeSink::default());
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

    /// The group closed: the outermost close latches for the owning
    /// declaration's begin event and feeds the alias capture; a nested
    /// close (a bound's function-type variables) belongs to the function
    /// type's own scope — its window never parks for a declaring header,
    /// and the enclosing group's in-flight capture is restored (#2897).
    /// The parser completes the variables in reverse declaration order,
    /// so the window reverses back — a capture's parameter order is the
    /// written one. Names bounded here latch the sticky bounded-parameter
    /// set — function scopes included.
    pub(crate) fn end_type_variables(&mut self) {
        let mut window = std::mem::take(&mut self.window);
        self.bound_sink = None;
        window.reverse();
        for (name, bound, poisoned) in &window {
            if bound.is_some() || *poisoned {
                self.index.bounded_params.insert(*name);
            }
        }
        let Some((outer_window, outer_sink)) = self.frames.pop() else {
            return;
        };
        self.window = outer_window;
        self.bound_sink = outer_sink;
        if !self.frames.is_empty() {
            return;
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

    /// Whether a type-variables group is still open: a nested group's
    /// close (a bound's function-type variables) must never pair a
    /// declaring header's executable latch (#2897).
    pub(crate) fn in_type_variables(&self) -> bool {
        !self.frames.is_empty()
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
        if name_offset >= window_end {
            return;
        }
        let Some(name) = name else {
            return;
        };
        let capture = self
            .index
            .containers
            .entry(name)
            .or_insert_with(|| ContainerCapture {
                params: Vec::new(),
                bounds: Vec::new(),
                supers: Vec::new(),
                bound_sites: Vec::new(),
                open: false,
                is_extension_type,
                is_enum,
            });
        // The kind flags were stated at the header open (keep-first: a
        // same-named duplicate's second header never re-registers).
        capture.params = window.iter().map(|(n, _, _)| *n).collect();
        capture.bounds = window.iter().map(|(_, b, p)| (b.clone(), *p)).collect();
    }

    /// The container's header window opened (its supertype clauses are
    /// captured from here to the body). The container registers here with
    /// its declaration's kind flags — a windowless container (no type
    /// variables, the common `class C extends G<B> {}` shape) never
    /// reaches [`Self::note_container`], whose window latch is its
    /// precondition, and its clause trees would drop.
    pub(crate) fn open_header(&mut self, name: StringId, is_extension_type: bool, is_enum: bool) {
        self.index
            .containers
            .entry(name)
            .or_insert_with(|| ContainerCapture {
                params: Vec::new(),
                bounds: Vec::new(),
                supers: Vec::new(),
                bound_sites: Vec::new(),
                open: false,
                is_extension_type,
                is_enum,
            });
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
    /// (`On` trees are constraints, not supertypes).
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
            if !matches!(clause, Clause::On(_)) {
                capture.supers.extend(trees.iter().cloned());
            }
            capture.bound_sites.extend(trees);
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

    /// A function type opened inside an open capture window: the shape's
    /// columns open their own level, so the completed node folds into a
    /// first-class function tree (`Handle Function()` survives as a
    /// capture fact) instead of poisoning the window. Only an already
    /// open window routes — a bare function annotation outside the
    /// capture windows opens no site here, matching [`Self::leaf`].
    pub(crate) fn begin_function(&mut self) {
        if let Some((_, header)) = self.header.as_mut() {
            header.sink.begin_function();
        } else if let Some(sink) = self.bound_sink.as_mut() {
            sink.begin_function();
        } else if let Some(site) = self.site.as_mut() {
            site.begin_function();
        }
    }

    /// A function type closed: the level seals into its function node in
    /// the same window [`Self::begin_function`] opened it (the cascade is
    /// static, and the parser pairs the two events).
    pub(crate) fn end_function(&mut self, nullable: bool, end: SourceRange) {
        if let Some((_, header)) = self.header.as_mut() {
            header.sink.end_function(nullable, end);
        } else if let Some(sink) = self.bound_sink.as_mut() {
            sink.end_function(nullable, end);
        } else if let Some(site) = self.site.as_mut() {
            site.end_function(nullable, end);
        }
    }

    /// A formal list opened under a captured function level: everything
    /// before it is the written return type. A no-op unless the open
    /// window's top level is a function level (the sink guards itself).
    pub(crate) fn function_formals_begin(&mut self) {
        if let Some((_, header)) = self.header.as_mut() {
            header.sink.function_formals_begin();
        } else if let Some(sink) = self.bound_sink.as_mut() {
            sink.function_formals_begin();
        } else if let Some(site) = self.site.as_mut() {
            site.function_formals_begin();
        }
    }

    /// One function-level formal closed: its name pairs with the type
    /// node that just landed (the sink guards itself).
    pub(crate) fn function_formal_end(&mut self, named: bool, name: Option<StringId>) {
        if let Some((_, header)) = self.header.as_mut() {
            header.sink.function_formal_end(named, name);
        } else if let Some(sink) = self.bound_sink.as_mut() {
            sink.function_formal_end(named, name);
        } else if let Some(site) = self.site.as_mut() {
            site.function_formal_end(named, name);
        }
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
            rhs_vars: Vec::new(),
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
    ///
    /// A window closed after the RHS parse started (`fn_seen` — the
    /// `Function` head event always precedes its own `<...>` group) is
    /// the RHS function type's OWN variables
    /// (`void Function<Y extends X>()`): they latch as the signature's
    /// variables and the declaring parameters stay (#2838 — the analyzer
    /// element model: the alias's parameters are the declaration's, the
    /// RHS's own variables are the function type's).
    pub(crate) fn alias_type_variables(&mut self, window: &[(StringId, Option<TypeTree>, bool)]) {
        if let Some(alias) = self.alias.as_mut() {
            if alias.fn_seen {
                alias.rhs_vars = window.to_vec();
                return;
            }
            alias.params = window.to_vec();
            alias.sink = TreeSink::default();
            alias.ret = None;
            alias.fn_seen = false;
        }
    }

    /// A simple type head completed inside the open alias capture. An
    /// open type-variables window claims every completion for its bound
    /// (`Y extends X` inside the RHS's own group) — the listener fires
    /// the alias face unconditionally, and the bound's trees must not
    /// strand in the RHS sink as debris (#2838).
    pub(crate) fn alias_leaf(&mut self, head: StringId, nullable: bool, range: SourceRange) {
        if self.bound_sink.is_some() {
            return;
        }
        if let Some(alias) = self.alias.as_mut() {
            alias.sink.leaf(head, nullable, range);
        }
    }

    /// A type-argument group opened inside the open alias capture. Bound
    /// windows gate the same as [`Self::alias_leaf`].
    pub(crate) fn alias_begin_group(&mut self, head: Option<(StringId, SourceRange)>) {
        if self.bound_sink.is_some() {
            return;
        }
        if let Some(alias) = self.alias.as_mut() {
            alias.sink.begin_group(head);
        }
    }

    /// A type-argument group closed inside the open alias capture. Bound
    /// windows gate the same as [`Self::alias_leaf`].
    pub(crate) fn alias_end_group(&mut self, end: SourceRange) {
        if self.bound_sink.is_some() {
            return;
        }
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
                type_params: alias.rhs_vars,
                ret: alias.ret,
                positional: alias.positional,
                named: alias.named,
                required_positional: alias.required_positional,
                nullable: alias.rhs_nullable,
            })
        } else if stray == 1 {
            alias.sink.done.pop().map(AliasRhs::Named)
        } else if stray == 2
            && !alias.sink.done[0].args.is_empty()
            && alias.sink.done[1].args.is_empty()
            && alias.sink.done[1].record.is_none()
            && alias.sink.done[1].function.is_none()
            && alias.sink.done[1].range.offset < alias.sink.done[0].range.offset
        {
            // A prefixed generic RHS (`typedef A = p.C<T>`): the argument
            // group's node lands first (the group closes inside the
            // qualified name), then `handle_type` completes on the
            // prefix token and drops the `p` leaf behind it. Keep the
            // head tree (`done[0]`); the trailing prefix leaf rides the
            // same spelling — the groundedness walk re-derives the
            // prefix from the source (`prefixed_alias_head`).
            let head_tree = alias.sink.done.remove(0);
            Some(AliasRhs::Named(head_tree))
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
        site: SourceRange,
    ) -> Vec<BoundedIssue> {
        // The annotation-site posture: the super-bounded pass is forbidden
        // only at extension-type elements (`_shouldAllowSuperBoundedTypes`,
        // :750-752). An alias head keeps the pass — the alias identity is
        // not in the deny list, and the contexts that deny it (the
        // declaration/clause parents) call [`Self::check_clause_sites`] or
        // [`Self::check_bound_annotation_sites`] with the pass denied
        // explicitly.
        let allow_super = self
            .index
            .containers
            .get(&head)
            .is_none_or(|capture| !capture.is_extension_type);
        self.check_site_inner(strings, head, args, in_scope_params, allow_super, site)
    }

    /// The clause-site face: every written supertype/constraint annotation
    /// of every captured container checks its arguments against the
    /// written head's bounds (`checkNamedType` runs at every `NamedType`;
    /// `_shouldAllowSuperBoundedTypes` denies the super pass on the
    /// clause contexts — `ClassTypeAlias`/`ExtendsClause`/
    /// `ImplementsClause`/`MixinOnClause`/`WithClause`,
    /// `type_arguments_verifier.dart:737-748` — so a regular violation
    /// reports with no inverted-type context row). A bare clause head
    /// resolves to its instantiate-to-bounds form and still answers the
    /// verdict (the raw-type context row rides the report). Declaration
    /// order (the sites' own offsets) decides the report order.
    pub(crate) fn check_clause_sites(&mut self, strings: &'a StringTable) -> Vec<BoundedIssue> {
        let mut sites: Vec<(u32, TypeTree, StringId)> = self
            .index
            .containers
            .iter()
            .flat_map(|(&name, capture)| {
                capture
                    .bound_sites
                    .iter()
                    .filter(|tree| !tree.poisoned)
                    .map(move |tree| (tree.range.offset, tree.clone(), name))
            })
            .collect();
        sites.sort_by_key(|(offset, _, _)| *offset);
        let mut issues = Vec::new();
        for (_, tree, owner) in sites {
            let Some(scope) = self.owner_scope(strings, owner) else {
                continue;
            };
            issues.extend(self.check_named(
                strings,
                tree.head,
                &tree.args,
                tree.range,
                &scope,
                true,
                false,
            ));
            self.walk_named_args(strings, &tree.args, &scope, true, false, &mut issues);
        }
        issues
    }

    /// The declaring container/alias's own parameter scope for a
    /// decl-scoped check — `None` when the owner never registered (a
    /// keep-first duplicate's second header), the suppression direction.
    fn owner_scope(
        &mut self,
        strings: &'a StringTable,
        owner: StringId,
    ) -> Option<FxHashMap<StringId, TypeParamId>> {
        let params: Vec<StringId> = match self.index.containers.get(&owner) {
            Some(capture) => capture.params.clone(),
            None => self
                .index
                .aliases
                .get(&owner)?
                .params
                .iter()
                .map(|(name, _, _)| *name)
                .collect(),
        };
        let snapshot = self.snapshot(strings);
        let &decl = snapshot.registry.get(&owner)?;
        Some(snapshot.ctx().decl_scope(decl, &params))
    }

    /// The bound-annotation face (#2908): `checkNamedType` runs at every
    /// `NamedType`, and a written type-parameter bound is one — the
    /// `G<B>` of `typedef H<P extends G<B>> = ...;` or of
    /// `class C<T extends G<B>> {}`. These trees parse inside the
    /// declaring `<...>` window, which claims them before the use-site
    /// capture can open, so the site and clause faces never saw them.
    /// The parent is the `TypeParameter` — not in
    /// `_shouldAllowSuperBoundedTypes`' deny list
    /// (type_arguments_verifier.dart:737-748) — so the super pass runs.
    /// A bare bound head resolves to its instantiate-to-bounds form like
    /// any `NamedType` (the raw-type context row rides the report), and
    /// every argument position recurses (`FutureOr<A<X?>>` checks `A<X?>`
    /// then `X?` against their own heads' bounds — #2936).
    /// Declaration order (the sites' own offsets) decides the report
    /// order.
    pub(crate) fn check_bound_annotation_sites(
        &mut self,
        strings: &'a StringTable,
    ) -> Vec<BoundedIssue> {
        let mut sites: Vec<(u32, TypeTree, StringId)> = Vec::new();
        for (name, capture) in &self.index.containers {
            for (bound, poisoned) in &capture.bounds {
                let Some(tree) = bound else {
                    continue;
                };
                if *poisoned || tree.poisoned {
                    continue;
                }
                sites.push((tree.range.offset, tree.clone(), *name));
            }
        }
        for (name, capture) in &self.index.aliases {
            for (_, bound, poisoned) in &capture.params {
                let Some(tree) = bound else {
                    continue;
                };
                if *poisoned || tree.poisoned {
                    continue;
                }
                sites.push((tree.range.offset, tree.clone(), *name));
            }
        }
        sites.sort_by_key(|(offset, _, _)| *offset);
        let mut issues = Vec::new();
        for (_, tree, owner) in sites {
            let Some(scope) = self.owner_scope(strings, owner) else {
                continue;
            };
            issues.extend(self.check_named(
                strings,
                tree.head,
                &tree.args,
                tree.range,
                &scope,
                true,
                true,
            ));
            self.walk_named_args(strings, &tree.args, &scope, true, false, &mut issues);
        }
        issues
    }

    /// The alias-RHS face (#2936): a function-type alias's `typedef B =
    /// A<X>;` right-hand `NamedType` is itself a checked annotation — its
    /// parent `GenericTypeAlias` denies the super pass
    /// (`_shouldAllowSuperBoundedTypes`, type_arguments_verifier.dart:742),
    /// exactly like the clause contexts. A function-shaped RHS's own
    /// return/argument `NamedType`s keep the pass (their parents are the
    /// function type's nodes, absent from the deny list). Declaration
    /// order (the sites' own offsets) decides the report order.
    pub(crate) fn check_alias_rhs_sites(&mut self, strings: &'a StringTable) -> Vec<BoundedIssue> {
        let mut sites: Vec<(u32, TypeTree, StringId, bool)> = Vec::new();
        for (name, capture) in &self.index.aliases {
            match &*capture.rhs {
                AliasRhs::Named(tree) => {
                    if !tree.poisoned {
                        sites.push((tree.range.offset, tree.clone(), *name, false));
                    }
                }
                AliasRhs::Fn {
                    ret,
                    positional,
                    named,
                    ..
                } => {
                    for tree in ret
                        .iter()
                        .chain(positional.iter())
                        .chain(named.iter().map(|(_, tree, _)| tree))
                    {
                        if !tree.poisoned {
                            sites.push((tree.range.offset, tree.clone(), *name, true));
                        }
                    }
                }
            }
        }
        sites.sort_by_key(|(offset, _, _, _)| *offset);
        let mut issues = Vec::new();
        for (_, tree, owner, allow_super) in sites {
            let Some(scope) = self.owner_scope(strings, owner) else {
                continue;
            };
            issues.extend(self.check_named(
                strings,
                tree.head,
                &tree.args,
                tree.range,
                &scope,
                true,
                allow_super,
            ));
            self.walk_named_args(strings, &tree.args, &scope, true, false, &mut issues);
        }
        issues
    }

    /// The nested-`NamedType` walk: upstream's verifier visits every
    /// `NamedType` node independently (`checkNamedType`,
    /// type_arguments_verifier.dart:243), so a type argument that itself
    /// carries arguments answers its own head's bounds — the `A<X?>` of
    /// `X extends FutureOr<A<X?>>` or the `G<U>` of `extends X<G<U>>`.
    /// An argument position's parent is a `TypeArgumentList`/`NamedType`,
    /// never in `_shouldAllowSuperBoundedTypes`' deny list, so the super
    /// pass always runs at nested depth. A poisoned subtree suppresses
    /// its whole descent (the under-report direction).
    ///
    /// `from_site` marks the written-site faces whose trees the engine
    /// also feeds through the #2831 P2 nested-site queue
    /// (`queue_nested_bounded_sites`: every unpoisoned in-file captured
    /// head with written arguments becomes its own `check_site` call).
    /// Those heads are skipped here — checking them in both places
    /// double-emits their rows (the queue's call carries the per-head
    /// `allow_super` posture `check_site` computes, which is also the
    /// upstream one: `_shouldAllowSuperBoundedTypes`' extension-type arm
    /// keys on the head's element, :712-714). The decl-scoped faces
    /// (clause, bound-annotation, alias-RHS) parse inside capture
    /// windows, no site capture opens and no queue exists for them —
    /// they keep the full walk.
    fn walk_named_args(
        &mut self,
        strings: &'a StringTable,
        args: &[TypeTree],
        scope: &FxHashMap<StringId, TypeParamId>,
        decl_scoped: bool,
        from_site: bool,
        issues: &mut Vec<BoundedIssue>,
    ) {
        for arg in args {
            if arg.poisoned {
                continue;
            }
            let queued = from_site && !arg.args.is_empty() && self.has_capture(arg.head);
            if !queued {
                issues.extend(self.check_named(
                    strings,
                    arg.head,
                    &arg.args,
                    arg.range,
                    scope,
                    decl_scoped,
                    true,
                ));
            }
            self.walk_named_args(strings, &arg.args, scope, decl_scoped, from_site, issues);
        }
    }

    /// The shared verdict: `head`'s captured parameters' substituted
    /// bounds against `args`; `allow_super` picks the regular face or the
    /// inverted re-check (`is_well_bounded`). `site` is the checked
    /// annotation's own span (the context row's anchor, and the report
    /// anchor when no written argument exists — `_typeArgumentErrorNode`
    /// falls back to the `NamedType` itself, type_arguments_verifier.dart:
    /// 757-768).
    ///
    /// A bare `head` (no written arguments) resolves to its
    /// instantiate-to-bounds form — the oracle's resolved `NamedType.type`
    /// — and answers the same verdict; the report then carries the
    /// raw-type context row (`"The raw type was instantiated as '...'"`,
    /// type_arguments_verifier.dart:441-446).
    ///
    /// `scope` maps the in-scope type-parameter names to their snapshot
    /// ids. `decl_scoped` says the ids are the declaring declaration's own
    /// (clause, bound-annotation, and alias-RHS sites): their declared
    /// bounds live in `facts`, so a type-parameter argument decides `X <:
    /// B` as `bound(X) <: B` — the official left-variable-bound rule. A
    /// lazy `param_scope` carries no declared bounds: a NON-nullable
    /// bounded in-scope argument keeps suppressing (the unmodelable `X <:
    /// B` direction) and a nullable `X?` decides on the `Null <: B`
    /// conjunct alone (#2936).
    fn check_named(
        &mut self,
        strings: &'a StringTable,
        head: StringId,
        args: &[TypeTree],
        site: SourceRange,
        scope: &FxHashMap<StringId, TypeParamId>,
        decl_scoped: bool,
        allow_super: bool,
    ) -> Vec<BoundedIssue> {
        // A type parameter shadows any container of the same name, and
        // `dynamic` is not a container: neither takes bounded arguments.
        if scope.contains_key(&head) {
            return Vec::new();
        }
        // The head's own parameter facts: a container capture, or — the
        // #2908 arm — a captured type ALIAS (`type.alias != null`,
        // type_arguments_verifier.dart:321-325: at an alias instantiation
        // the ALIAS element's own parameters are the checked pair, at
        // annotation and clause sites alike).
        let (head_params, head_bounds) = match self.index.containers.get(&head) {
            Some(capture) => (capture.params.clone(), capture.bounds.clone()),
            None => match self.index.aliases.get(&head) {
                Some(alias) => (
                    alias.params.iter().map(|(name, _, _)| *name).collect(),
                    alias
                        .params
                        .iter()
                        .map(|(_, bound, poisoned)| (bound.clone(), *poisoned))
                        .collect(),
                ),
                None => return Vec::new(),
            },
        };
        if head_params.is_empty() {
            return Vec::new();
        }
        let snapshot = self.snapshot(strings);
        let Some(decl) = snapshot.registry.get(&head).copied() else {
            return Vec::new();
        };
        // An alias declaration the bare-core fallback lost
        // (`build_lenient`'s Err arm registers no alias face) keeps the
        // silence — `regular_bounded_issues` pins the hierarchy lookup.
        if snapshot.alias_decls.contains(&decl) && snapshot.hierarchy.decl(decl).is_none() {
            return Vec::new();
        }
        let ctx = snapshot.ctx();
        // The instantiation under check: the written arguments mapped, or —
        // a bare head — the oracle's `instantiateToBounds` answer. A wrong
        // written count reports as `wrong_number_of_type_arguments` and the
        // official bounds check skips (no instantiation to walk); a
        // poisoned argument tree or an unmodelable parameter bound
        // suppresses, as does an in-scope type-parameter argument declared
        // with a bound (the official test succeeds through the parameter's
        // own bound; the M1 face would answer a false positive).
        let (instantiation, raw) = if args.is_empty() {
            let Some(defaults) = ctx.bounds_instantiation(decl) else {
                return Vec::new();
            };
            (
                snapshot.table.intern(
                    UninternedType::Interface {
                        decl,
                        args: defaults,
                    },
                    false,
                ),
                true,
            )
        } else {
            // A NON-nullable in-scope type-parameter argument declared
            // with a bound suppresses at LAZY scopes (`X <: B` rides X's
            // declared bound, which lazy ids do not carry — the
            // under-report direction); decl-scoped ids decide it in the
            // post-verdict filter instead. A NULLABLE bounded argument
            // always runs (`X? <: B` conjuncts `Null <: B`, which the
            // snapshot answers exactly — #2936, `class A<X extends
            // FutureOr<A<X?>>>`'s `X?`).
            if args.len() != head_params.len()
                || head_bounds.iter().any(|(_, poisoned)| *poisoned)
                || args.iter().any(|arg| arg.poisoned)
                || args.iter().any(|arg| {
                    !decl_scoped
                        && !arg.nullable
                        && scope.contains_key(&arg.head)
                        && self.index.bounded_params.contains(&arg.head)
                })
            {
                return Vec::new();
            }
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
            (
                snapshot.table.intern(
                    UninternedType::Interface {
                        decl,
                        args: mapped_args,
                    },
                    false,
                ),
                false,
            )
        };
        // The super pass is forbidden at extension-type elements
        // (:750-752); annotation sites allow it.
        let system = snapshot.system();
        match is_well_bounded(&system, &snapshot.facts, instantiation, allow_super) {
            Boundedness::NotWellBounded(issues) => {
                // The type-parameter-argument gate (see `check_named`).
                // An issue on an in-scope parameter argument `X`/`X?`
                // keeps only when `X <: B` is decided false — the official
                // left-variable-bound rule `bound(X) <: B`
                // (subtype.dart:490-500):
                // * a DECL-scoped id reads its declared bound from `facts`
                //   (`dynamic`-mapped reads as the implicit `Object?`), so
                //   `X <: B` is honest both ways;
                // * a lazy-scope bounded name is unmodelable — non-nullable
                //   args never reach here (pre-gate), and a nullable `X?`
                //   keeps only when `Null <: B` fails;
                // * any other in-scope name is an unbounded parameter —
                //   the implicit `Object?` bound decides it.
                // An unsafe declared bound reads as unmodelable and drops
                // (the under-report direction).
                let null = snapshot.table.intern(UninternedType::Null, false);
                let object_q = snapshot.table.intern(
                    UninternedType::Interface {
                        decl: snapshot.hierarchy.core().object,
                        args: Vec::new(),
                    },
                    true,
                );
                let bound_of = |arg: &TypeTree| -> Option<TypeId> {
                    if decl_scoped {
                        let &id = scope.get(&arg.head)?;
                        snapshot.facts.get(id).bound.and_then(|bound| {
                            if matches!(
                                snapshot.table.get(bound).kind(),
                                TypeKind::Dynamic
                            ) {
                                None
                            } else {
                                Some(bound)
                            }
                        })
                    } else {
                        None
                    }
                };
                let issues: Vec<_> = issues
                    .into_iter()
                    .filter(|issue| {
                        let Some(arg) = args.get(issue.index) else {
                            return true;
                        };
                        if !scope.contains_key(&arg.head) {
                            return true;
                        }
                        if !decl_scoped && self.index.bounded_params.contains(&arg.head) {
                            return arg.nullable && !system.is_subtype(null, issue.bound);
                        }
                        let param_bound = match bound_of(arg) {
                            Some(bound) if snapshot.type_is_safe(bound) => bound,
                            Some(_) => return false,
                            None => object_q,
                        };
                        !system.is_subtype(param_bound, issue.bound)
                            || (arg.nullable && !system.is_subtype(null, issue.bound))
                    })
                    .collect();
                if issues.is_empty() {
                    return Vec::new();
                }
                // The context face (`type_arguments_verifier.dart:427-465`):
                // where the super-bounded pass ran, every report carries the
                // inverted type's row — and the issues ARE the inverted
                // pass's there (`is_well_bounded` reports those), so the
                // display re-runs the same inversion. The extension-type
                // face skips the pass upstream too. A bare head keys the
                // raw-type context row on the i2b instantiation.
                let inverted = allow_super
                    .then(|| snapshot.display(replace_top_bottom(&system, instantiation)));
                let raw_instantiated = raw.then(|| snapshot.display(instantiation));
                issues
                    .into_iter()
                    .map(|issue| BoundedIssue {
                        range: args.get(issue.index).map_or(site, |arg| arg.range),
                        site,
                        argument: snapshot.display(issue.argument),
                        parameter: snapshot.param_text(issue.parameter),
                        bound: snapshot.display(issue.bound),
                        inverted_type: inverted.clone(),
                        raw_instantiated: raw_instantiated.clone(),
                    })
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    /// The written-argument site face: `check_named` at the peeled site
    /// root, then the nested-`NamedType` recursion over the argument
    /// subtrees (upstream runs the verdict at every `NamedType`;
    /// #2936). `site` is the checked annotation's own span. The scope is
    /// lazy (no declared bounds — the bounded-parameter gates stay on).
    fn check_site_inner(
        &mut self,
        strings: &'a StringTable,
        head: StringId,
        args: &[TypeTree],
        in_scope_params: &[StringId],
        allow_super: bool,
        site: SourceRange,
    ) -> Vec<BoundedIssue> {
        let scope = {
            let snapshot = self.snapshot(strings);
            snapshot.ctx().param_scope(in_scope_params)
        };
        let mut issues = self.check_named(strings, head, args, site, &scope, false, allow_super);
        self.walk_named_args(strings, args, &scope, false, true, &mut issues);
        issues
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

    /// A captured type alias's own parameters (name, written bound, poison
    /// bit) in declaration order — the alias-creation bounds face's
    /// parameter facts (#2869: the ALIAS's own bounds decide on a
    /// creation spelled through the alias). `None` when the head captured
    /// no alias; owned, mirroring [`Self::container_params`].
    pub(crate) fn alias_params(
        &self,
        name: StringId,
    ) -> Option<Vec<(StringId, Option<TypeTree>, bool)>> {
        let capture = self.index.aliases.get(&name)?;
        Some(
            capture
                .params
                .iter()
                .map(|(param, bound, poisoned)| (*param, bound.clone(), *poisoned))
                .collect(),
        )
    }

    /// #2831 P1 — the raw-constructor inference face. A creation with no
    /// explicit type arguments and no downward context seeds every
    /// unconstrained parameter with the greatest closure of its bound
    /// (`GenericInferrer.tryChooseFinalTypes` → `chooseTypes` →
    /// `chooseTypeFromConstraint(grounded)`, upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/element/generic_inferrer.dart:298-380 and
    /// pkg/_fe_analyzer_shared/lib/src/type_inference/
    /// type_analyzer_operations.dart:1018-1076), then the extends constraint
    /// re-checks the seed against the bound substituted with all seeds
    /// (`TypeConstraintFromExtendsClause.isSatisfiedBy`,
    /// type_constraint.dart:250-300). Answers one row per failing
    /// parameter, declaration order: the seed, the declared bound as
    /// written, and the substituted bound — the message pieces of both the
    /// `could_not_infer` report and its creation-site
    /// `type_argument_not_matching_bounds` twin (the failed inference's
    /// kept types are the constructor's implicit arguments, the oracle's
    /// error heuristic at generic_inferrer.dart:385-395). A non-function
    /// type alias used as a constructor callee answers the same question
    /// over the ALIAS's own parameters (#2841 — upstream infers the alias
    /// application's arguments, so the `could_not_infer` text spells the
    /// alias's type variable). An empty answer covers every suppression
    /// path (no capture, a poisoned or unmapped bound reads `dynamic` and
    /// always satisfies, an unsafe closure).
    pub(crate) fn check_creation_inference(
        &mut self,
        strings: &'a StringTable,
        head: StringId,
    ) -> Vec<CreationInferenceRow> {
        // The capture gate is shape-agnostic (container header or alias
        // declaration): both captures carry the same non-empty unpoisoned
        // parameter contract, and the snapshot's registry/facts below
        // already key the alias declaration's own parameters and bounds
        // (the `itb_default_args` alias branch proves the reach).
        let unmodelable = match (
            self.index.containers.get(&head),
            self.index.aliases.get(&head),
        ) {
            (Some(capture), _) => {
                capture.params.is_empty() || capture.bounds.iter().any(|(_, p)| *p)
            }
            (None, Some(capture)) => {
                capture.params.is_empty() || capture.params.iter().any(|(_, _, p)| *p)
            }
            (None, None) => return Vec::new(),
        };
        if unmodelable {
            return Vec::new();
        }
        let snapshot = self.snapshot(strings);
        let Some(&decl) = snapshot.registry.get(&head) else {
            return Vec::new();
        };
        let Some(params) = snapshot.decl_params.get(&decl) else {
            return Vec::new();
        };
        let dynamic = snapshot.table.intern(UninternedType::Dynamic, false);
        let bounds: Vec<TypeId> = params
            .iter()
            .map(|&param| snapshot.facts.get(param).bound.unwrap_or(dynamic))
            .collect();
        // The seeds, declaration order: each bound with the EARLIER seeds
        // substituted, its remaining parameters eliminated to the greatest
        // closure — exactly `chooseTypes`'s sequential schema solve (later
        // parameters are `unknown` there, and `_` eliminates to the same
        // top/bottom the parameter elimination uses).
        let targets: FxHashSet<TypeParamId> = params.iter().copied().collect();
        let mut seeds = Vec::with_capacity(params.len());
        for (index, &bound) in bounds.iter().enumerate() {
            let partial = if seeds.is_empty() {
                bound
            } else {
                let map: Vec<(TypeParamId, TypeId)> = params[..index]
                    .iter()
                    .copied()
                    .zip(seeds.iter().copied())
                    .collect();
                snapshot.table.substitute(bound, &map)
            };
            seeds.push(greatest_closure(
                &snapshot.system(),
                partial,
                &targets,
                true,
                0,
            ));
        }
        let system = snapshot.system();
        let mut rows = Vec::new();
        for (index, (&param, &bound)) in params.iter().zip(&bounds).enumerate() {
            if !snapshot.type_is_safe(bound) {
                continue;
            }
            let map: Vec<(TypeParamId, TypeId)> =
                params.iter().copied().zip(seeds.iter().copied()).collect();
            let substituted = snapshot.table.substitute(bound, &map);
            if system.is_subtype(seeds[index], substituted) {
                continue;
            }
            rows.push(CreationInferenceRow {
                parameter: snapshot.param_text(param),
                seed: snapshot.display(seeds[index]),
                raw_bound: snapshot.display(bound),
                bound: snapshot.display(substituted),
            });
        }
        rows
    }

    /// Whether `name` is one of this file's captured declarations (a
    /// container or a type alias) — the nested site-queue gate (#2831
    /// P2): only an in-file head has a `check_site` answer, so only
    /// those nested heads queue.
    pub(crate) fn has_capture(&self, name: StringId) -> bool {
        self.index.containers.contains_key(&name) || self.index.aliases.contains_key(&name)
    }

    /// The captured right-hand function shape of the file's own alias
    /// `name` (`typedef P<T> = bool Function(T)`): the type-parameter
    /// triples in declaration order, the RHS's own type variables (the
    /// generic-function-type RHS's signature parameters, #2838), and the
    /// RHS's written pieces. `None` when `name` has no capture or its
    /// RHS is a named-type alias — the use-site faces that read a
    /// callable answer (`Predicate<T> p` invoked as `p(x)`) treat `None`
    /// as uncallable-shaped. This-file declarations only — a cross-file
    /// alias's capture lives in that unit's own parse; callers fall back
    /// silent.
    #[allow(clippy::type_complexity)] // one positional tuple, six capture pieces
    pub(crate) fn alias_fn_shape(
        &self,
        name: StringId,
    ) -> Option<(
        &[(StringId, Option<TypeTree>, bool)],
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
                type_params,
                ..
            } => Some((
                &capture.params,
                type_params,
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

    /// The raw-type compare face (#2838): the head's parameters'
    /// instantiate-to-bounds defaults as written trees — the snapshot's
    /// `instantiateTypeFormalsToBounds` fixpoint (type_system.dart:762-838
    /// through the `instantiate_to_bound` port), so a raw `A` at a
    /// same-head comparison reads as its defaulted instantiation
    /// (`instantiateToBounds`, type_system.dart:719-731; the oracle's
    /// resolved types never disagree in arity — the mismatch is this
    /// face's raw residue). The ITB fixpoint lives on the private
    /// [`Snapshot`]; this is the narrowest seam that exposes its answer.
    /// `None` keeps the caller's arity silence: no this-file capture, a
    /// poisoned parameter bound (the snapshot read it `dynamic` — a
    /// default the written shape never spells, the false-positive
    /// direction), or an unconvertible default kind.
    pub(crate) fn itb_default_args(
        &self,
        strings: &'a StringTable,
        head: StringId,
    ) -> Option<Vec<TypeTree>> {
        if let Some(capture) = self.index.containers.get(&head)
            && capture.bounds.iter().any(|(_, poisoned)| *poisoned)
        {
            return None;
        }
        if let Some(capture) = self.index.aliases.get(&head)
            && capture.params.iter().any(|(_, _, poisoned)| *poisoned)
        {
            return None;
        }
        let snapshot = self.snapshot(strings);
        let decl = *snapshot.registry.get(&head)?;
        let params = snapshot.decl_params.get(&decl)?;
        let defaults = instantiate_type_formals_to_bounds(&snapshot.table, &snapshot.facts, params);
        defaults
            .defaults
            .iter()
            .map(|&ty| snapshot.tree_of(ty))
            .collect()
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
        let invalid_heads = [
            "dynamic", "void", "Never", "Null", "FutureOr", "Function", "Record",
        ];
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
                    && matches!(
                        snapshot.table.get(face_ty).kind(),
                        TypeKind::Interface { .. }
                    )
                {
                    row.verdict = Some(snapshot.system().is_subtype(rep_ty, face_ty));
                    row.displays = Some((snapshot.display(face_ty), snapshot.display(rep_ty)));
                }
                row
            })
            .collect()
    }

    /// Builds (once) or returns the snapshot, rebuilt when the capture
    /// index grew since the last build (see [`Self::snapshot_decl_count`]).
    /// Takes `&self` (the #2838 raw-type compare arm asks from the
    /// `&self` assignability chain): a mid-parse ask builds against the
    /// declarations captured so far — an incomplete world answers through
    /// registry misses and the arity gate, the under-report direction —
    /// and the count check rebuilds for every later consumer.
    fn snapshot<'s>(&'s self, strings: &'a StringTable) -> Ref<'s, Snapshot<'a>> {
        let decl_count = self.index.containers.len() + self.index.aliases.len();
        {
            let mut slot = self.snapshot.borrow_mut();
            if slot.is_none() || decl_count != self.snapshot_decl_count.get() {
                *slot = Some(Snapshot::build(&self.index, strings));
                self.snapshot_decl_count.set(decl_count);
            }
        }
        Ref::map(self.snapshot.borrow(), |slot| {
            slot.as_ref().expect("built or cached above")
        })
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
    ("Type", &[], &[]),
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
    /// The declaring declaration's own parameter scope: the snapshot's
    /// `decl_params` ids keyed by their written names. Unlike the lazy
    /// `param_scope` allocations these ids carry their declared bounds in
    /// `facts`, so a type-parameter argument at a decl-scoped site
    /// (clause, bound-annotation, alias-RHS) answers `X <: B` through
    /// `X's bound <: B` — the official left-variable-bound rule
    /// (subtype.dart:490-500) this face can compute (#2936).
    fn decl_scope(&self, decl: TypeDeclId, names: &[StringId]) -> FxHashMap<StringId, TypeParamId> {
        let mut scope = FxHashMap::default();
        for (name, &id) in names
            .iter()
            .zip(self.decl_params.get(&decl).into_iter().flatten())
        {
            scope.insert(*name, id);
        }
        scope
    }

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

    /// Maps a written function-type node (`X Function(int)`,
    /// `dynamic Function(Object)`) to an interned function type — the
    /// shape a written bound spells directly (`class C<X extends
    /// void Function()>`, `typedef F<X extends Function(X)>`) and a type
    /// argument may carry. The capture folds the formals without
    /// required flags, so every positional interns as required and every
    /// named formal as required — exact for the required-only signatures
    /// the corpus's bounds spell, and monotone for the rest: a written
    /// optional formal read required can only make the written side
    /// accept MORE (a bound) or report nothing new (the official
    /// optional formal admits every call the required reading does).
    /// The signature's own type variables reuse the shared per-name lazy
    /// ids (their written bounds stay unmapped — the official checks
    /// those at the declaration site, #2878); a formal that fails to map
    /// defers the whole function (`None`, the under-report direction).
    fn map_function(
        &self,
        function: &FnTree,
        scope: &FxHashMap<StringId, TypeParamId>,
        nullable: bool,
    ) -> Option<TypeId> {
        let ret = match &function.ret {
            Some(tree) => self.map_tree(tree, scope)?,
            // An unwritten return type reads `dynamic` (a bare
            // `Function(...)` head).
            None => self.table.intern(UninternedType::Dynamic, false),
        };
        let mut type_parameters = Vec::with_capacity(function.type_params_named.len());
        {
            let mut lazy = self.lazy_params.borrow_mut();
            for (name, _) in &function.type_params_named {
                let id = lazy
                    .entry(*name)
                    .or_insert_with(|| {
                        let id = TypeParamId::from_raw(self.next_param.get());
                        self.next_param.set(self.next_param.get() + 1);
                        (id, self.strings.resolve(*name).to_string())
                    })
                    .0;
                type_parameters.push(id);
            }
        }
        let positional = function
            .positional
            .iter()
            .map(|tree| self.map_tree(tree, scope))
            .collect::<Option<Vec<_>>>()?;
        let named = function
            .named
            .iter()
            .map(|(name, tree)| {
                Some(NamedMember {
                    name: self.strings.resolve(*name).to_string(),
                    ty: self.map_tree(tree, scope)?,
                    required: true,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        let required_positional = u32::try_from(positional.len()).ok()?;
        self.table
            .intern_function(
                UninternedFunction {
                    return_type: ret,
                    type_parameters,
                    positional,
                    named,
                    required_positional,
                },
                nullable,
            )
            .ok()
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
        // A record-form node has no registry head (its `head` slot is the
        // `StringId::NONE` sentinel) and the snapshot has no record kinds:
        // the same refusal the clause trees apply (`clause_tree_ty`), the
        // under-report direction. A function-form node interns as a
        // first-class function type (`map_function`).
        if tree.record.is_some() {
            return None;
        }
        if let Some(function) = &tree.function {
            return self.map_function(function, scope, tree.nullable);
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
            _ => {
                // An alias head an in-scope parameter shadows reads as
                // the parameter (a library-scope declaration loses to the
                // local parameter); any other alias is not modelable
                // until its expansion exists (the `alias_decls` guard
                // below, the under-report direction). Containers keep the
                // registry-wins order.
                let alias_shadowed = scope.contains_key(&tree.head)
                    && self
                        .registry
                        .get(&tree.head)
                        .is_some_and(|&decl| self.alias_decls.contains(&decl));
                match self.registry.get(&tree.head) {
                    Some(&decl) if !alias_shadowed => {
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
                    _ => scope.get(&tree.head).map(|&parameter| {
                        self.table
                            .intern(UninternedType::TypeParameter { parameter }, tree.nullable)
                    }),
                }
            }
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
        // The core `Object` declaration is outside the builtin table (its
        // edge is the build's own root); the #2831 greatest-closure seed
        // interns `Object?` against it and the display needs the name.
        // `Object`/`Future`/`Function` join the registry so a written bound
        // or argument naming one maps (`class A<X extends Object>`,
        // `extends Future<int>`) instead of falling to `dynamic`.
        decl_text.insert(object, "Object".to_string());
        decl_text.insert(future, "Future".to_string());
        decl_text.insert(function, "Function".to_string());
        registry.insert(strings.intern("Object"), object);
        registry.insert(strings.intern("Future"), future);
        registry.insert(strings.intern("Function"), function);
        decl_params.insert(future, vec![TypeParamId::from_raw(1)]);
        decl_params.insert(object, Vec::new());
        decl_params.insert(function, Vec::new());
        param_text.insert(TypeParamId::from_raw(1), "T".to_string());
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

        // The captured aliases (#1351) register BEFORE the hierarchy
        // freeze: the instantiation-site bounds face (#2908) checks a use
        // site's written arguments against the ALIAS's own parameters
        // (`type.alias != null`, type_arguments_verifier.dart:321-325),
        // and the verdict reads the parameter list off the hierarchy. No
        // supertype edge ever targets or leaves an alias — the expansion
        // map keeps alias heads out of every walked type, and an
        // unexpanded alias head is refused in `map_tree` (`alias_decls`)
        // — so the declarations are inert for the subtype walks.
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
            // The RHS's own type variables join the same scope (#2838):
            // the RHS trees (`(Y y)` formals) resolve their leaves through
            // it, and the interned function signature declares their ids.
            // Their written bounds stay capture-level facts — the snapshot
            // reads no boundedness question through a function binder.
            if let AliasRhs::Fn { type_params, .. } = &*capture.rhs {
                for (var, _, _) in type_params {
                    let id = TypeParamId::from_raw(next_param.get());
                    next_param.set(next_param.get() + 1);
                    scope.insert(*var, id);
                    param_text.insert(id, strings.resolve(*var).to_string());
                }
            }
            registry.insert(*name, decl);
            decl_text.insert(decl, strings.resolve(*name).to_string());
            builder = builder.decl(decl, param_ids.clone());
            decl_params.insert(decl, param_ids);
            alias_scopes.insert(decl, scope);
            alias_decls.insert(decl);
        }

        // The fresh in-file declarations are all registered by now. The
        // alias expansions map FIRST: a container bound referencing a
        // captured alias (`class A<X extends G<A<X>>>` with
        // `typedef G<X> = X Function()`) resolves through the expansion —
        // the pre-alias order once read those `dynamic` (the
        // always-satisfied bound), silently suppressing every check the
        // alias-carried bound decides. The expansion pass itself reads no
        // facts but the scratch face, so moving it up changes nothing for
        // the aliases.
        let scratch_facts = ParamFactsTable::new();
        // The captured aliases' expansions (#1351): each RHS maps in the
        // alias's own parameter scope (the declarations registered above);
        // a bound or RHS referencing a not-yet-expanded alias defers to
        // the next round (a chain terminates, a cycle drops — the
        // under-report direction).
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

        // The container bounds map against the REAL facts face and the
        // completed alias expansions: an alias-carried bound expands
        // exactly like a use-site alias application, and a bare generic
        // inside a bound instantiates to the alias parameters' already
        // mapped bounds. A bound that still fails to map (an unexpanded
        // co-alias, an unregistered head) reads `dynamic` — the
        // always-satisfied bound, so the parameter's check can never
        // fire (the under-report direction). The mapping runs inside a
        // scope that drops the ctx before the results apply (the facts
        // face updates after the borrow).
        let mapped_bounds: Vec<(TypeParamId, TypeId)> = {
            let ctx = MapCtx {
                table: &table,
                strings,
                registry: &registry,
                decl_params: &decl_params,
                facts: &facts,
                alias_expansions: &alias_expansions,
                alias_decls: &alias_decls,
                lazy_params: &RefCell::new(FxHashMap::default()),
                next_param: &next_param,
            };
            let mut mapped: Vec<(TypeParamId, TypeId)> = Vec::new();
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
                    let bound = match bound {
                        Some(tree) => ctx
                            .map_tree(tree, scope)
                            .unwrap_or_else(|| table.intern(UninternedType::Dynamic, false)),
                        None => table.intern(UninternedType::Dynamic, false),
                    };
                    mapped.push((id, bound));
                }
            }
            mapped
        };
        for (id, bound) in mapped_bounds {
            facts.set(
                id,
                ParamFacts {
                    variance: Variance::Covariant,
                    bound: Some(bound),
                },
            );
        }

        // The edge sweeps run against a fresh complete ctx over the now
        // applied facts (a bare generic in a supertype instantiates to
        // its true bounds, an alias-carried supertype to its expansion);
        // map the builtin supertype edges through the complete registry
        // (edges are written heads like `Comparable<num>`).
        let ctx = MapCtx {
            table: &table,
            strings,
            registry: &registry,
            decl_params: &decl_params,
            facts: &facts,
            alias_expansions: &alias_expansions,
            alias_decls: &alias_decls,
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

        // Freeze the snapshot edge by edge: an edge the build rejects (a
        // back edge of a cyclic `extends` pair, an arity-mismatched
        // instantiation) drops out alone and its owner goes unsafe — its
        // supertype closure is incomplete from that edge on, so no bound
        // check may answer through it — while every other declaration
        // keeps its face. The declaration-level contract never fails from
        // source shape; the bare core answers the unreachable arm.
        let hierarchy = match builder.build_lenient(&table) {
            Ok((hierarchy, dropped)) => {
                unsafe_decls.extend(dropped.iter().map(|edge| edge.owner));
                hierarchy
            }
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
        // The declared bounds arm the subtype walks: the left-type-variable
        // rule (`X0 <: T1` via `bound(X0) <: T1`, subtype.dart:490-500)
        // decides every bounded-parameter question the verifier poses — a
        // self-referential bound's own site (`class A<X extends A<A<X>>>`
        // annotation `A<A<X>>`) asks `A<X> <: A<A<A<X>>>`, which holds
        // through `bound(X) <: A<A<X>>` by reflexivity, and a bound-blind
        // system answers it `false` (the oracle's silent row reported).
        TypeSystem::new(&self.table, &self.hierarchy).with_param_facts(&self.facts)
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
            // A function type renders like the official `getDisplayString`:
            // `<return> Function<...>(formals)` — the shape the
            // `type_argument_not_matching_bounds` message spells for a
            // function-typed bound or argument ('dynamic Function(Object)',
            // 'Never Function(dynamic Function(Object))'). An unwritten
            // return interns as `dynamic`, matching the official display
            // of a bare `Function(...)` head.
            TypeKind::Function { signature } => {
                let sig = self.table.signature(signature);
                let mut text = format!("{} Function", self.display(sig.return_type));
                let type_parameters = self.table.function_type_parameters(sig.type_parameters);
                if !type_parameters.is_empty() {
                    let names: Vec<String> =
                        type_parameters.iter().map(|&id| self.param_text(id)).collect();
                    text.push_str(&format!("<{}>", names.join(", ")));
                }
                let mut formals: Vec<String> = (0..sig.positional.len())
                    .map(|i| self.display(self.table.arg(sig.positional.offset() + i)))
                    .collect();
                let named: Vec<String> = self
                    .table
                    .named_members(sig.named)
                    .into_iter()
                    .map(|member| {
                        let prefix = if member.required { "required " } else { "" };
                        format!("{prefix}{} {}", self.display(member.ty), member.name)
                    })
                    .collect();
                if !named.is_empty() {
                    formals.push(format!("{{{}}}", named.join(", ")));
                }
                format!("{text}({}){suffix}", formals.join(", "))
            }
            TypeKind::Record { .. } => format!("Record{suffix}"),
        }
    }

    /// The written-tree projection of a snapshot type (#2838): the ITB
    /// defaults ride back to the engine as the trees it converts with its
    /// own written-face discipline (`tree_ty`), so the defaulted
    /// arguments compare exactly like written ones. The names re-intern
    /// canonically (the declaration and parameter display texts are the
    /// written spellings), `FutureOr` keeps its written head spelling,
    /// and a kind the written world cannot spell (a function or record
    /// default — the snapshot's bounds never intern one — or the error
    /// sentinel) answers `None`: the caller keeps the silence.
    fn tree_of(&self, ty: TypeId) -> Option<TypeTree> {
        let range = SourceRange {
            offset: 0,
            length: 0,
        };
        let t = self.table.get(ty);
        let tree = match t.kind() {
            TypeKind::Dynamic => TypeTree::leaf(self.strings.intern("dynamic"), false, range),
            TypeKind::Void => TypeTree::leaf(self.strings.intern("void"), false, range),
            TypeKind::Null => TypeTree::leaf(self.strings.intern("Null"), false, range),
            TypeKind::Never => TypeTree::leaf(self.strings.intern("Never"), t.is_nullable(), range),
            TypeKind::TypeParameter { parameter } => {
                let text = self.param_text(parameter);
                if text.is_empty() {
                    return None;
                }
                TypeTree::leaf(self.strings.intern(&text), t.is_nullable(), range)
            }
            TypeKind::FutureOr { base } => {
                let mut tree =
                    TypeTree::leaf(self.strings.intern("FutureOr"), t.is_nullable(), range);
                tree.args.push(self.tree_of(base)?);
                tree
            }
            TypeKind::Interface { decl, args } => {
                let text = self.decl_text.get(&decl)?;
                let mut tree = TypeTree::leaf(self.strings.intern(text), t.is_nullable(), range);
                for index in 0..args.len() {
                    tree.args
                        .push(self.tree_of(self.table.arg(args.offset() + index))?);
                }
                tree
            }
            TypeKind::Function { .. } | TypeKind::Record { .. } | TypeKind::Invalid => {
                return None;
            }
        };
        Some(tree)
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
        bounds.open_header(int_box, false, false);
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
        let issues = bounds.check_site(&strings, head, &args, &[], range(70, 9));
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
        let issues = bounds.check_site(&strings, head, &args, &[], range(39, 9));
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].argument, "String");
        assert_eq!(issues[0].parameter, "T");
        assert_eq!(issues[0].bound, "Box<int>");
        assert_eq!(issues[0].range.offset, 41);
    }

    /// The generic-function-type RHS capture (#2838): `typedef F<X> =
    /// void Function<Y extends X>()` — the declaring parameters stay the
    /// LHS `<X>` (the corruption adopted the RHS's own window over them
    /// and reset the capture mid-RHS), the RHS's `<Y extends X>` latches
    /// as the function type's own variables, and the `void` return
    /// survives to the formals boundary.
    #[test]
    fn generic_function_rhs_keeps_declaring_params_and_latches_its_own() {
        let strings = StringTable::new();
        let f = strings.intern("F");
        let x = strings.intern("X");
        let y = strings.intern("Y");
        let void_id = strings.intern("void");
        let mut bounds = Bounds::new();
        bounds.begin_alias(f);
        // `typedef F<X> = ...` — the declaring window.
        bounds.begin_type_variables();
        bounds.end_type_variable(x, false);
        bounds.end_type_variables();
        // The RHS: `void Function<Y extends X>()`.
        bounds.alias_formal_boundary();
        bounds.alias_leaf(void_id, false, range(24, 4));
        bounds.begin_type_variables();
        bounds.leaf(x, false, range(41, 1));
        bounds.end_type_variable(y, true);
        bounds.end_type_variables();
        bounds.alias_formals_region_open();
        bounds.alias_formals_closed();
        bounds.alias_rhs_question(false);
        bounds.end_alias();

        let (params, type_params, ret, positional, named, required_positional) =
            bounds.alias_fn_shape(f).expect("the generic RHS captures");
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].0, x);
        assert!(
            params[0].1.is_none(),
            "the declaring parameter keeps its window"
        );
        assert_eq!(type_params.len(), 1);
        assert_eq!(type_params[0].0, y);
        let bound = type_params[0]
            .1
            .as_ref()
            .expect("the written bound rides the signature");
        assert_eq!(bound.head, x);
        assert_eq!(ret.expect("the return survives").head, void_id);
        assert!(positional.is_empty() && named.is_empty());
        assert_eq!(required_positional, None);
    }

    /// A written function-type bound decides like the official
    /// `X Function()` shape: `class C<X extends void Function()>` rejects
    /// `C<int>` (the super pass re-checks the unchanged inverted type and
    /// still fails) with the bound displayed as the analyzer spells it
    /// (`type_arguments_verifier.dart:342-540`; the bound carries the
    /// substitution and `isSubtypeOf` decides, :368-386).
    #[test]
    fn function_bound_violation_reports_with_display() {
        let strings = StringTable::new();
        let c = strings.intern("C");
        let x = strings.intern("X");
        let void_id = strings.intern("void");
        let int = strings.intern("int");
        let mut bounds = Bounds::new();
        // `class C<X extends void Function()> {}`
        bounds.begin_type_variables();
        bounds.begin_function();
        bounds.leaf(void_id, false, range(20, 4));
        bounds.function_formals_begin();
        bounds.end_function(false, range(34, 1));
        bounds.end_type_variable(x, true);
        bounds.end_type_variables();
        bounds.note_container(Some(c), 6, 25, false, false);
        // `void f(C<int> c) {}`
        bounds.begin_group(Some((c, range(33, 1))), true);
        bounds.leaf(int, false, range(35, 3));
        let (head, args) = close_site(&mut bounds, range(38, 1));
        let issues = bounds.check_site(&strings, head, &args, &[], range(33, 6));
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].argument, "int");
        assert_eq!(issues[0].parameter, "X");
        assert_eq!(issues[0].bound, "void Function()");
    }

    /// The bound-hit side of the function face stays silent: a written
    /// function argument that satisfies the function bound reports
    /// nothing (`C<void Function()>` against `X extends void Function()`)
    /// — the over-emit direction is the face's regression risk.
    #[test]
    fn function_bound_hit_stays_silent() {
        let strings = StringTable::new();
        let c = strings.intern("C");
        let x = strings.intern("X");
        let void_id = strings.intern("void");
        let mut bounds = Bounds::new();
        // `class C<X extends void Function()> {}`
        bounds.begin_type_variables();
        bounds.begin_function();
        bounds.leaf(void_id, false, range(20, 4));
        bounds.function_formals_begin();
        bounds.end_function(false, range(34, 1));
        bounds.end_type_variable(x, true);
        bounds.end_type_variables();
        bounds.note_container(Some(c), 6, 25, false, false);
        // `void f(C<void Function()> c) {}`
        bounds.begin_group(Some((c, range(33, 1))), true);
        bounds.begin_function();
        bounds.leaf(void_id, false, range(35, 4));
        bounds.function_formals_begin();
        bounds.end_function(false, range(47, 1));
        let (head, args) = close_site(&mut bounds, range(48, 1));
        let issues = bounds.check_site(&strings, head, &args, &[], range(33, 16));
        assert!(issues.is_empty());
    }

    /// A container bound carried by a function-type alias decides: with
    /// `typedef G<X> = X Function()` and `class A<X extends G<A<X>>>`,
    /// the bound expands through the alias (`A<X> Function()` after
    /// substitution), so `A<Object>` violates where a use spelling the
    /// alias as its argument satisfies it. The pre-alias-expansion build
    /// read this bound `dynamic` and never fired (the missing-row family
    /// behind the Instantiate-to-bound `class_typedef` cases).
    #[test]
    fn alias_carried_container_bound_decides() {
        let strings = StringTable::new();
        let g = strings.intern("G");
        let xg = strings.intern("X");
        let a = strings.intern("A");
        let xa = strings.intern("X");
        let object = strings.intern("Object");
        let mut bounds = Bounds::new();
        // `typedef G<X> = X Function();`
        bounds.begin_alias(g);
        bounds.begin_type_variables();
        bounds.end_type_variable(xg, false);
        bounds.end_type_variables();
        bounds.alias_formal_boundary();
        bounds.alias_leaf(xg, false, range(20, 1));
        bounds.alias_formals_region_open();
        bounds.alias_formals_closed();
        bounds.alias_rhs_question(false);
        bounds.end_alias();
        // `class A<X extends G<A<X>>> {}`
        bounds.begin_type_variables();
        bounds.begin_group(Some((g, range(32, 1))), false);
        bounds.begin_group(Some((a, range(34, 1))), false);
        bounds.leaf(xa, false, range(36, 1));
        bounds.end_group(range(37, 1));
        bounds.end_group(range(38, 1));
        bounds.end_type_variable(xa, true);
        bounds.end_type_variables();
        bounds.note_container(Some(a), 25, 39, false, false);
        // `void f(A<Object> a) {}` — the argument misses the expanded
        // bound `A<Object> Function()` (the alias display itself is the
        // message-level residual; the verdict is the face under test).
        bounds.begin_group(Some((a, range(48, 1))), true);
        bounds.leaf(object, false, range(50, 6));
        let (head, args) = close_site(&mut bounds, range(56, 1));
        let issues = bounds.check_site(&strings, head, &args, &[], range(48, 9));
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].argument, "Object");
        assert_eq!(issues[0].parameter, "X");
    }

    /// The site-path walk defers queued heads: an in-file captured head
    /// with written arguments is the engine's #2831 P2 nested site's own
    /// `check_site` call, so the outer site's walk stays silent for it —
    /// the pair once double-emitted the nested row (the duplicate-extra
    /// family behind the Instantiate-to-bound `class_l1_*` cases).
    #[test]
    fn site_walk_defers_queued_nested_heads() {
        let strings = StringTable::new();
        let a = strings.intern("A");
        let t = strings.intern("T");
        let num = strings.intern("num");
        let string = strings.intern("String");
        let f = strings.intern("F");
        let mut bounds = Bounds::new();
        // `class A<T extends num> {}`
        bounds.begin_type_variables();
        bounds.leaf(num, false, range(13, 3));
        bounds.end_type_variable(t, true);
        bounds.end_type_variables();
        bounds.note_container(Some(a), 6, 21, false, false);
        // The outer site `F<A<String>>` (`F` a foreign head) and the
        // nested site the engine queues for the captured `A` head.
        let nested_arg = TypeTree::leaf(string, false, range(42, 6));
        let mut nested = TypeTree::leaf(a, false, range(40, 1));
        nested.args.push(nested_arg.clone());
        let outer_issues =
            bounds.check_site(&strings, f, &[nested.clone()], &[], range(38, 12));
        assert!(outer_issues.is_empty());
        let nested_issues = bounds.check_site(&strings, a, &[nested_arg], &[], range(40, 9));
        assert_eq!(nested_issues.len(), 1);
        assert_eq!(nested_issues[0].argument, "String");
        // A bare (argument-less) captured head never queues — the walk
        // still checks it through the instantiate-to-bounds face.
        let bare_issues = bounds.check_site(&strings, f, &[TypeTree::leaf(a, false, range(60, 1))], &[], range(58, 5));
        assert!(bare_issues.is_empty());
    }
}
