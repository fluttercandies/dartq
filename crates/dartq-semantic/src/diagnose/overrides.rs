use super::*;

/// One optional-parameter row held for the fold-tail augmentation-chain
/// verdict (#3557): `parameterElement.hasDefaultValue` is the MERGED
/// element's flag, so a `missing_default_value_for_parameter` candidate can
/// be silenced by a default clause written on a fragment the parse has not
/// reached yet (`void f([int a]);` before `augment void f([int a = 0]) {}`).
struct DeferredDefault {
    /// `MissingDefaultValueForParameter` or its `*Positional` sibling.
    code: SemanticCode,
    /// The parameter name token's span — the row anchor and the chain
    /// lookup's owner fingerprint.
    range: SourceRange,
    /// The parameter's name (fragments match parameters by name).
    param: StringId,
    /// The deferring parameter is named (`{a}`) or positional (`[a]`) —
    /// augmentation-chain fragments merge named parameters by name and
    /// positional parameters by POSITION (upstream merges the merged
    /// element's formal table; a renamed positional augment still
    /// supplies the same slot).
    named: bool,
}

/// The current unit's deferred optional-parameter rows: staged at the
/// executable's body event, drained by [`Engine::finalize_default_checks`]
/// at the fold tail. Thread-local rather than an engine field so the fold
/// state stays untouched — a sequential fold drains it before the next
/// unit parses.
thread_local! {
    static DEFERRED_DEFAULTS: std::cell::RefCell<Vec<DeferredDefault>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// The frozen `dart:collection` list-base face (upstream Dart SDK source,
/// `sdk/lib/collection/list.dart` — `abstract mixin class ListBase<E>
/// implements List<E>`, `typedef ListMixin<E> = ListBase<E>`): every
/// member the SDK projection's names-only stubs cannot carry, split by
/// the declaration's abstractness and kind. The base implements all of
/// `List`'s interface except the four forwarding points its doc names
/// ("Forwarding just the four `length` and `[]` read/write operations")
/// — those stay abstract, so a `with ListMixin` class missing the `length`
/// setter reports `setter List.length` missing exactly like the oracle
/// (`DriftResultSet with ListMixin<DriftRow>`); everything else joins the
/// concrete-implementation set. Kinds matter only to the getter/method
/// conflict face — they mirror the SDK declarations. `(member, abstract,
/// kind)` with kind 0 = method, 1 = setter, 2 = getter.
const LIST_BASE_FACE: &[(&str, bool, u8)] = &[
    ("length", true, 2),
    ("length=", true, 1),
    ("[]", true, 0),
    ("[]=", true, 0),
    ("iterator", false, 2),
    ("isEmpty", false, 2),
    ("isNotEmpty", false, 2),
    ("first", false, 2),
    ("first=", false, 1),
    ("last", false, 2),
    ("last=", false, 1),
    ("single", false, 2),
    ("reversed", false, 2),
    ("elementAt", false, 0),
    ("followedBy", false, 0),
    ("forEach", false, 0),
    ("contains", false, 0),
    ("every", false, 0),
    ("any", false, 0),
    ("firstWhere", false, 0),
    ("lastWhere", false, 0),
    ("singleWhere", false, 0),
    ("join", false, 0),
    ("where", false, 0),
    ("whereType", false, 0),
    ("map", false, 0),
    ("expand", false, 0),
    ("reduce", false, 0),
    ("fold", false, 0),
    ("skip", false, 0),
    ("skipWhile", false, 0),
    ("take", false, 0),
    ("takeWhile", false, 0),
    ("toList", false, 0),
    ("toSet", false, 0),
    ("add", false, 0),
    ("addAll", false, 0),
    ("remove", false, 0),
    ("removeWhere", false, 0),
    ("retainWhere", false, 0),
    ("clear", false, 0),
    ("cast", false, 0),
    ("removeLast", false, 0),
    ("sort", false, 0),
    ("shuffle", false, 0),
    ("asMap", false, 0),
    ("+", false, 0),
    ("sublist", false, 0),
    ("getRange", false, 0),
    ("removeRange", false, 0),
    ("fillRange", false, 0),
    ("setRange", false, 0),
    ("replaceRange", false, 0),
    ("indexOf", false, 0),
    ("indexWhere", false, 0),
    ("lastIndexOf", false, 0),
    ("lastIndexWhere", false, 0),
    ("insert", false, 0),
    ("removeAt", false, 0),
    ("insertAll", false, 0),
    ("setAll", false, 0),
];

/// The frozen `dart:collection` set-base face (upstream Dart SDK source,
/// `sdk/lib/collection/set.dart` — `abstract mixin class SetBase<E>
/// implements Set<E>`, `typedef SetMixin<E> = SetBase<E>`): the same
/// names-only-stub complement as [`LIST_BASE_FACE`]. `SetBase` implements
/// all of `Set`'s interface except the seven forwarding points its doc
/// names ("depends only on the abstract members: add, contains, lookup,
/// remove, iterator, length and toSet") — those stay abstract, so a
/// concrete `extends SetBase` class missing one reports it exactly like
/// the oracle; everything else joins the concrete-implementation set
/// (html's `_CssClassSetImpl extends SetBase<String> implements
/// CssClassSet` — `SetBase.addAll`/`removeAll` implement the
/// `CssClassSet` interface names). Kinds mirror the SDK declarations;
/// `SetBase` declares no setter. `(member, abstract, kind)` with kind
/// 0 = method, 1 = setter, 2 = getter.
const SET_BASE_FACE: &[(&str, bool, u8)] = &[
    ("add", true, 0),
    ("contains", true, 0),
    ("lookup", true, 0),
    ("remove", true, 0),
    ("iterator", true, 2),
    ("toSet", true, 0),
    ("length", true, 2),
    ("isEmpty", false, 2),
    ("isNotEmpty", false, 2),
    ("single", false, 2),
    ("first", false, 2),
    ("last", false, 2),
    ("cast", false, 0),
    ("followedBy", false, 0),
    ("whereType", false, 0),
    ("clear", false, 0),
    ("addAll", false, 0),
    ("removeAll", false, 0),
    ("retainAll", false, 0),
    ("removeWhere", false, 0),
    ("retainWhere", false, 0),
    ("containsAll", false, 0),
    ("union", false, 0),
    ("intersection", false, 0),
    ("difference", false, 0),
    ("toList", false, 0),
    ("map", false, 0),
    ("where", false, 0),
    ("expand", false, 0),
    ("forEach", false, 0),
    ("reduce", false, 0),
    ("fold", false, 0),
    ("every", false, 0),
    ("join", false, 0),
    ("any", false, 0),
    ("take", false, 0),
    ("takeWhile", false, 0),
    ("skip", false, 0),
    ("skipWhile", false, 0),
    ("firstWhere", false, 0),
    ("lastWhere", false, 0),
    ("singleWhere", false, 0),
    ("elementAt", false, 0),
];

/// The outcome of the inherited-signature instantiation face: the
/// signature carried no super-face type parameters (compare as read), the
/// edge arguments resolved to a substitution environment (compare
/// substituted), or the environment is unknowable at the M1 name level
/// (the face comparison suppresses — the under-report direction, never a
/// false positive).
enum Instantiated {
    Unchanged,
    Applied(FxHashMap<StringId, Ty>),
    Undecidable,
}

/// The written supertype clause of one current-file container header: its
/// own type-parameter spellings in declaration order, and each
/// `extends`/`with`/`implements` face as (head, written argument types —
/// one per parameter). Built by a single bounded token walk
/// ([`Engine::scan_header_clauses`]); a header whose clauses parse
/// unfaithfully (a function- or record-typed argument, recovery residue)
/// contributes no entry — its edges instantiate to unknown, the
/// suppression direction.
struct HeaderClause {
    params: Vec<StringId>,
    supers: Vec<SuperEntry>,
}

/// The current file's scanned header clauses. `by_offset` keys each entry
/// on its declaring name token's UTF-16 offset — a per-declaration
/// identity, so a repeated container name (`class C` written eighty times,
/// the parser-fe recovery cases) still reports each header's own clauses
/// exactly once. `last_of_name` maps the declaring name to the LAST
/// declaration's offset — the element model collapses repeats into one
/// container, so the name-keyed substitution faces (which read the model's
/// collapsed decl) keep the scan's prior last-wins overwrite semantics.
struct HeaderClauses {
    by_offset: FxHashMap<u32, HeaderClause>,
    last_of_name: FxHashMap<StringId, u32>,
}

impl HeaderClauses {
    /// The clause facts of the declaration `name` resolves to in the
    /// element model — the last declaration carrying that name (the
    /// model's own collapse keeps one container per name).
    fn for_name(&self, name: StringId) -> Option<&HeaderClause> {
        self.by_offset.get(self.last_of_name.get(&name)?)
    }
}

/// The clause a written supertype entry belongs to (upstream Dart SDK
/// source, `pkg/analyzer/lib/src/dart/resolver/resolution_visitor.dart`
/// `_resolveType`'s clause switch :1209-1227 — the diagnostic code keys on
/// the clause kind, not on the face's own position). `AliasHead` is the
/// named mixin application's superclass slot (`class C = S with ...`) and
/// a plain extends head when the declaration also carries a `with` clause:
/// both spell the `mixin_with_non_class_superclass` code upstream
/// (:1212-1219).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SuperTag {
    /// A class declaration's `extends` head.
    Extends,
    /// A named mixin application's head type or an extends head on a
    /// `with`-carrying declaration.
    AliasHead,
    /// A `with` clause entry.
    With,
    /// An `implements` clause entry.
    Implements,
    /// A mixin's `on` constraint entry.
    On,
}

/// One written supertype-clause entry as scanned from the source tokens:
/// the clause it sits in, the head's bare-leaf spelling (`StringId::NONE`
/// for the `void`/`dynamic` keyword heads — the entry carries a dedicated
/// [`Ty`] then), the import prefix when the face was spelled `p.C`, the
/// oracle's anchor span (the bare head, or prefix-head through local end
/// for `p.C` — `resolution_visitor.dart:1235-1237`), and the written type
/// list (`vec![whole_named_ty]`, the same convention the old
/// `(StringId, Vec<Ty>)` entries kept for [`Engine::clause_instantiation`]).
struct SuperEntry {
    tag: SuperTag,
    head: StringId,
    prefix: Option<StringId>,
    span: SourceRange,
    args: Vec<Ty>,
}

/// One inherited-member candidate the override faces resolved: the entry
/// itself (a same-file shared or cross-file projected model row, or a
/// synthesized entry — an induced setter, a frozen top-face member), the
/// file whose container declared it, the hop spelling that answered (the
/// oracle's `overriddenInterfaceName`), and the full hop chain (the
/// substitution walk's edges).
struct FaceHit<'a> {
    entry: HitEntry<'a>,
    origin: FileId,
    face_name: StringId,
    chain: Vec<(StringId, FileId)>,
}

/// The entry half of a [`FaceHit`]: a model row behind the boundary
/// projection (its pointer is the dedup identity, stable across faces), or
/// an owned synthesized entry (no stable address — it never joins the
/// dedup set, reporting per face like the oracle's per-superinterface
/// iteration).
enum HitEntry<'a> {
    Projected(Projected<'a, MemberEntry>),
    Frozen(MemberEntry),
}

impl<'a> HitEntry<'a> {
    fn get(&self) -> &MemberEntry {
        match self {
            HitEntry::Projected(entry) => entry,
            HitEntry::Frozen(entry) => entry,
        }
    }
}

impl<'a> FaceHit<'a> {
    fn entry(&self) -> &MemberEntry {
        self.entry.get()
    }

    /// The dedup identity: `None` for a synthesized entry — it dedups
    /// never, staying per-face.
    fn identity(&self) -> Option<*const MemberEntry> {
        match &self.entry {
            HitEntry::Projected(entry) => Some(entry.ptr()),
            HitEntry::Frozen(_) => None,
        }
    }
}

/// One candidate set's combined answer — upstream
/// `inheritance_manager3.dart`'s `_getValidOverrides` + `_topMerge` arms.
enum Merged {
    /// No candidate is a valid override of every other (an empty
    /// `_getValidOverrides`): the interface map carries no name for it.
    Conflict,
    /// One candidate already spells the combined signature — the identity
    /// arms (a single valid override, or a merged type equal to an
    /// existing candidate's).
    Candidate(usize),
    /// The combined signature belongs to no candidate: the merge target's
    /// own synthesized member (`_topMerge`'s build arm).
    Synthetic(MemberEntry),
}

/// Whether the declared member carries an implicit type slot — the oracle's
/// `hasImplicitType` face (`instance_member_inferrer.dart`): the model
/// records an unwritten column as [`Ty::Unknown`], so the field's type, the
/// getter/method's return type, or the setter's parameter type reading
/// `Unknown` is the inference trigger.
fn has_implicit_slot(entry: &MemberEntry) -> bool {
    match entry.kind {
        MemberKind::Field => entry.var_ty == Ty::UNKNOWN,
        MemberKind::Getter | MemberKind::Method => {
            entry.return_ty == Ty::UNKNOWN
                || entry.formals.as_deref().is_some_and(|f| {
                    f.positional
                        .iter()
                        .chain(f.named.iter())
                        .any(|p| p.ty == Ty::UNKNOWN)
                })
        }
        MemberKind::Setter => entry
            .formals
            .as_deref()
            .and_then(|f| f.positional.first())
            .is_some_and(|p| p.ty == Ty::UNKNOWN),
    }
}

/// The `invalid_override_of_non_virtual_member` code (upstream Dart SDK
/// source, `pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart`'s
/// `invalidOverrideOfNonVirtualMember` row — a `WarningCode`
/// `STATIC_WARNING`). The semantic code table carries no variant for it
/// because this file owns its only emit site, so the descriptor lives
/// beside that site.
const INVALID_OVERRIDE_OF_NON_VIRTUAL_MEMBER: CodeInfo = CodeInfo::new(
    "invalid_override_of_non_virtual_member",
    "INVALID_OVERRIDE_OF_NON_VIRTUAL_MEMBER",
    DiagnosticType::StaticWarning,
    true,
);

/// The parameter list a SETTER's function type carries: exactly one
/// required positional slot, typed by the declared first parameter or
/// `dynamic` when the declaration wrote none. Upstream builds a setter
/// element's type from the single parameter it owns (`SetterElementImpl`,
/// upstream Dart SDK source, `pkg/analyzer/lib/src/dart/element/
/// element.dart`), never from the written list — a malformed
/// `set foo()` still types `void Function(dynamic)`. Comparing the
/// WRITTEN lists instead reads that malformed arity as a break and
/// fabricates an override row the oracle does not report
/// (`test_setter_single_setter_withoutParameter`).
fn setter_type_formals(formals: Option<&Formals>) -> Formals {
    let first = formals.and_then(|f| f.positional.first());
    Formals {
        positional: Box::new([Formal {
            name: first.map_or(StringId::NONE, |f| f.name),
            ty: first.map_or(Ty::Dynamic, |f| f.ty),
            required: true,
            covariant: first.is_some_and(|f| f.covariant),
            default_value: None,
        }]),
        named: Box::new([]),
    }
}

/// Pushes one level's inherited faces so a LIFO walk visits them in the
/// oracle's most-specific-first order: the last-written mixin leads (each
/// later application's whole interface shadows the earlier ones and the
/// superclass — `_addImplemented` (`inheritance_manager3.dart:333-352`)
/// overlays them into one map in application order), then the
/// superclass edge. A mixin's `on` constraints are NOT inherited concrete
/// members (the map carries only the element's own methods/getters/setters
/// plus its own application faces), so they never join the walk.
fn push_concrete_faces(
    decl: &Container,
    writer: StringId,
    writer_file: FileId,
    lib: LibraryId,
    pending: &mut Vec<(StringId, StringId, FileId, LibraryId)>,
) {
    if let Some(superclass) = decl.superclass {
        pending.push((superclass, writer, writer_file, lib));
    }
    for mixin in decl.with_clause.iter() {
        pending.push((*mixin, writer, writer_file, lib));
    }
}

/// The concrete instance member a container answers for the lookup name —
/// the `_addImplemented` filter (`inheritance_manager3.dart:339-347`: only
/// a non-abstract, non-static member joins the inherited map). A stored
/// FIELD answers the SETTER lookup through its induced setter (the official
/// map keys a non-final field's setter under `name=`).
///
/// Distinct from `diagnose::enum_verifier`'s `concrete_member` predicate
/// (`entry.is_abstract`-free static/concrete test) and from that module's
/// `Engine::inherited_concrete_member`, which answers a container spelling
/// for the enum-constant superset face: this one answers the member ENTRY
/// (its metadata is the question) and kind-matches the getter/setter lookup
/// forms the `@nonVirtual` face asks in.
fn concrete_member_of<'c>(
    decl: &'c Container,
    plain: StringId,
    setter: StringId,
    setter_face: bool,
) -> Option<&'c MemberEntry> {
    let joinable = |entry: &MemberEntry| !entry.is_static && !entry.is_abstract;
    if setter_face {
        if let Some(entry) = decl.members.get(&setter)
            && joinable(entry)
            && entry.kind == MemberKind::Setter
        {
            return Some(entry);
        }
        let field = decl.members.get(&plain)?;
        return (joinable(field) && field.kind == MemberKind::Field && !field.is_final)
            .then_some(field);
    }
    let entry = decl.members.get(&plain)?;
    (joinable(entry)
        && matches!(
            entry.kind,
            MemberKind::Getter | MemberKind::Method | MemberKind::Field
        ))
    .then_some(entry)
}

impl<'a> Engine<'a> {
    /// Walks `container`'s member face through the linearized supertype
    /// chain: the container's own members, then its `with` mixins in reverse
    /// application order (the official application sequence
    /// `inheritance_manager3.dart:571-579` — `class X extends S with M1, M2`
    /// is `S&M1 extends S`, `S&M2 extends S&M1`, `X extends S&M2`, so each
    /// later mixin shadows the earlier ones and the superclass), then the
    /// superclass name edges. Any unresolvable hop breaks the chain
    /// (external type, unresolvable superclass name): the caller suppresses
    /// rather than reports. Probes memoize per pass ([`FaceMemo`]): the
    /// miss chains re-ask the same face several times per row.
    pub(crate) fn walk_members(&self, container: StringId, member: StringId) -> Walk<'a> {
        self.member_faces.walk(container, member, false, || {
            self.walk_from(container, member, false)
        })
    }

    /// The climb behind [`Engine::walk_members`]: `skip_own` starts at the
    /// container but excludes its own member map (the `super.member` shape —
    /// the application classes `S&M2` are still the chain). The super climb
    /// is a concrete-dispatch lookup first (official `getMember(...,
    /// forSuper: true)`: an abstract redeclaration on the chain — `mixin M
    /// on S { T get x; }` — is no dispatch target); when no concrete member
    /// answers, the inherited lookup (`getInherited`) types the read.
    pub(crate) fn walk_from(
        &self,
        container: StringId,
        member: StringId,
        skip_own: bool,
    ) -> Walk<'a> {
        if skip_own && let found @ Walk::Found(_) = self.walk_climb(container, member, true, true) {
            return found;
        }
        self.walk_climb(container, member, skip_own, false)
    }

    /// The `super.member` dispatch walk — the official `getMember(element,
    /// name, forSuper: true)` (`inheritance_manager3.dart:223-265`): the
    /// enclosing type's `superImplemented.last` map — the concrete-dispatch
    /// members on the superclass-and-mixin spine only. Abstract members are
    /// skipped (they never enter `implemented` — `_addMixinMembers` and
    /// `_addImplemented` both filter on `isAbstract`), `implements`/`on`
    /// signature faces never answer (the interface merge contributes no
    /// dispatch target), and a mixin's `on` heads are not edges here. The
    /// chain still reports `ChainBroken` on an unresolvable hop — the
    /// under-report boundary stays identical to the union climb's.
    ///
    /// Not memoized through [`FaceMemo::walk`]: the `(container, member,
    /// super)` key already caches the union answer [`Self::walk_super`]
    /// rides, and a dispatch probe of the same key would collide with it.
    pub(crate) fn walk_dispatch(&self, container: StringId, member: StringId) -> Walk<'a> {
        self.walk_climb_impl(container, member, true, true, true)
    }

    /// The `super.member` interface walk — the official `getInherited`
    /// (`inheritance_manager3.dart:152-155`, `inheritedMap` over the union of
    /// every super-interface's signatures): the answer to "does any
    /// supertype carry the name at all", abstract members and `implements`
    /// faces included. The oracle consults it only after the dispatch walk
    /// misses — a hit there reports `abstract_super_member_reference`, a
    /// miss the `undefined_super_*` face.
    ///
    /// A mixin's interface walk covers its `on` constraints AND its
    /// `implements` faces (the `overridden` candidate map,
    /// `_getInterfaceMixin`:1001-1057); every other container climbs the
    /// union spine (`skip_own`, abstract entries live).
    pub(crate) fn walk_super_interface(&self, container: StringId, member: StringId) -> Walk<'a> {
        if let Some((container_decl, _)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        ) && container_decl.kind == ContainerKind::Mixin
        && container_decl.superclass.is_none()
        && container_decl.with_clause.is_empty()
        {
            let mut seen: FxHashSet<StringId> = FxHashSet::default();
            seen.insert(container);
            return self.walk_face_list(
                container_decl
                    .on_clause
                    .iter()
                    .chain(container_decl.implements_clause.iter())
                    .map(|face| (*face, self.file, container)),
                self.lib,
                member,
                &mut seen,
                true,
            );
        }
        self.walk_climb(container, member, true, false)
    }

    fn walk_climb(
        &self,
        container: StringId,
        member: StringId,
        skip_own: bool,
        for_super: bool,
    ) -> Walk<'a> {
        self.walk_climb_impl(container, member, skip_own, for_super, false)
    }

    fn walk_climb_impl(
        &self,
        container: StringId,
        member: StringId,
        mut skip_own: bool,
        for_super: bool,
        dispatch: bool,
    ) -> Walk<'a> {
        let mut name = container;
        // The container whose supertype clauses wrote the current `name` —
        // the prefixed-clause probe's writer key (a hop's superclass,
        // mixins, and faces live in the found container's header; a
        // prefixed spelling there binds through its import prefix,
        // `implements idl.X`). The anchor's own name is a declaration, not
        // a clause face, and keeps the bare resolution.
        let mut writer = container;
        let mut owner = self.file;
        let mut scope_lib = self.lib;
        let mut seen: FxHashSet<StringId> = FxHashSet::default();
        let mut at_anchor = true;
        // The `implements` faces met along the `extends`/`with` climb: each
        // level's interfaces join that level's interface (an implementor
        // inherits its interfaces' members — the oracle's interface build,
        // inheritance_manager3), but a superclass's own interface — which
        // already merges its `implements` faces — shadows an implementor's
        // faces. Collecting per level and seeding deepest-first preserves
        // that order while the chain climbs.
        let mut pending_faces: Vec<Vec<(StringId, FileId, StringId)>> = Vec::new();
        // The current file's written supertype clauses, built on first
        // need — a found mixin member whose signature carries the mixin's
        // own type parameters (the application-site substitution face). The
        // scanner only covers `self.file`, so a foreign application class
        // (the `with M<Arg>` edge living in another unit) stays undecidable
        // and the raw entry passes through — the under-report direction.
        let mut facts: Option<HeaderClauses> = None;
        // The unique-declaration re-anchor bypasses the seen gate once
        // (the anchor's spelling re-resolves in its declaring library).
        let mut retried = false;
        loop {
            if !retried && !seen.insert(name) {
                return Walk::ChainBroken;
            }
            retried = false;
            let found = if at_anchor {
                resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    scope_lib,
                    owner,
                    name,
                )
                // The dot-joined anchor (`io.File`, the intern table's
                // one-head form the prefixed annotations arm): the bare
                // probe misses by construction, the prefix-aware split
                // owns it.
                .or_else(|| {
                    resolve_joined_container(
                        self.graph,
                        self.namespaces,
                        self.index,
                        scope_lib,
                        self.strings,
                        name,
                    )
                })
            } else {
                self.resolve_clause_face(writer, owner, scope_lib, name)
            };
            let Some((found, found_owner)) = found else {
                if at_anchor {
                    // A receiver head that arrived through a resolved
                    // member signature (a foreign getter's declared
                    // return, never spelled in this file) resolves in its
                    // DECLARING library's scope — the oracle walks the
                    // declaring scope, not the use site's. The re-anchor
                    // bypasses the seen gate once.
                    if let Some((file, lib)) = self.unique_declared_elsewhere(name, scope_lib) {
                        owner = file;
                        scope_lib = lib;
                        at_anchor = false;
                        retried = true;
                        continue;
                    }
                    // The receiver-type spelling itself unresolvable is the
                    // oracle's InvalidType: member reads ride dynamic and
                    // stay silent (the broken chain).
                    return Walk::ChainBroken;
                }
                if !self.clause_hop_dead(writer, owner, scope_lib, name) {
                    // A declaration the index cannot model stays the broken
                    // chain either way — the oracle sees members we cannot
                    // walk.
                    return Walk::ChainBroken;
                }
                // A dead-edge hop (undefined, ambiguous, or non-class
                // supertype spelling — the oracle's InvalidType) contributes
                // nothing to the interface: the `implements` faces collected
                // on the climb still merge exactly like the `superclass:
                // None` arm's tail (the oracle's interface merge); none is
                // the clean miss the member lookups report.
                if pending_faces.is_empty() {
                    return Walk::Missing;
                }
                return self.walk_face_list(
                    pending_faces.iter().rev().flatten().copied(),
                    scope_lib,
                    member,
                    &mut seen,
                    false,
                );
            };
            at_anchor = false;
            if !skip_own
                && let Some(entry) = found.members.get(&member)
                && !(for_super && entry.is_abstract)
            {
                // F1: the found entry belongs to `found_owner`'s pool —
                // project it before it flows into rows or message text.
                self.member_faces
                    .found_origin
                    .set(Some((name, found_owner)));
                let projected = self.boundary.member(found_owner, name, member, entry);
                // An ancestor hop's found entry reads through the writer's
                // written clause arguments — `class B extends A<int>`
                // finds `A`'s `T get value` as `int` — the same
                // application-site substitution the mixin hop runs, keyed
                // on this climb's own edge (`writer extends name<Args>`).
                // The anchor's own members need no hop; an undecidable or
                // unchanged face keeps the raw entry (the under-report
                // posture).
                if name != container {
                    let found_entry =
                        match self.mixin_env(writer, name, found_owner, &projected, &mut facts) {
                            Instantiated::Applied(env) => Projected::Foreign(std::rc::Rc::new(
                                self.substitute_entry(&projected, &env),
                            )),
                            Instantiated::Unchanged | Instantiated::Undecidable => projected,
                        };
                    return Walk::Found(found_entry);
                }
                return Walk::Found(projected);
            }
            skip_own = false;
            // The stacked applications, later first: each shadows the members
            // of the earlier ones and of the superclass. The mixin names are
            // written in `found_owner` and resolve through that file's
            // library (probed by the container name it declares, #491 S2a).
            let owner_lib = self
                .owner_library_of(found_owner, name)
                .unwrap_or(scope_lib);
            // The dispatch walk keeps a worklist: an applied mixin CLASS's own
            // `with` chain is part of the application's `implemented` map
            // upstream (`_addMixinMembers` folds the mixin's implemented in,
            // which a mixin class builds from its own `with` spine), so a
            // member miss on `A` of `with A` keeps descending into `A`'s
            // applications — depth-first, most-applied first. The union walk
            // instead routes those faces through `pending_faces` (the
            // interface merge), so its seed stays the written clause.
            let mut mixin_queue: VecDeque<(StringId, FileId, StringId)> = found
                .with_clause
                .iter()
                .rev()
                .map(|mixin| (*mixin, found_owner, name))
                .collect();
            while let Some((mixin, mixin_writer_owner, mixin_writer)) = mixin_queue.pop_front() {
                if !seen.insert(mixin) {
                    return Walk::ChainBroken;
                }
                let mixin_lib = self
                    .owner_library_of(mixin_writer_owner, mixin_writer)
                    .unwrap_or(owner_lib);
                let mixin_resolved =
                    self.resolve_clause_face(mixin_writer, mixin_writer_owner, mixin_lib, mixin);
                if let Some((mixin_container, mixin_owner)) = &mixin_resolved
                    && mixin_container
                        .members
                        .get(&member)
                        .is_none_or(|entry| dispatch && entry.is_abstract)
                {
                    if dispatch {
                        // The mixin's own `with` chain is part of its
                        // `implemented` (a mixin class is a class — its
                        // interface build merges the applied members as
                        // concrete targets), so it recurses into the
                        // dispatch queue ahead of the shallower seeds —
                        // `with A, B` where `B = X with B2` asks B2 before X
                        // before A (application order, most specific first).
                        for (index, face) in mixin_container.with_clause.iter().enumerate() {
                            mixin_queue.insert(index, (*face, *mixin_owner, mixin));
                        }
                    } else {
                        // The application's interface keeps the mixin's own
                        // faces: a resolved mixin whose members miss the lookup
                        // still contributes its `on` constraint heads and
                        // `implements` faces to the chain's interface climb
                        // (`with ListMixin<E>` — SDK 3.47's ListBase `implements
                        // List<E>` is where `[]`/`length` live; skipping them
                        // left a fully-decidable chain answering Missing, which
                        // flagged legal overrides as non-overriding once the
                        // alias fold made the hop resolvable).
                        //
                        // A mixin CLASS applied through `with` also contributes
                        // its own `with` faces: a named application `mixin class
                        // A<U, V> = Object with M<...>` denotes the application
                        // type, so `class C ... with A<...>` puts M's members on
                        // C's interface exactly like a directly-written `with M`
                        // (the oracle resolves `c.m()` on `class C<T> = S with
                        // B<List<T>>`, `B = Object with A<...>`, `A = Object
                        // with M<...>` through that chain — tests/language/
                        // mixin5_test.dart). The superclass stays out: an
                        // applied mixin's superclass is always `Object`
                        // (`MIXIN_CLASS_DECLARATION_EXTENDS_NOT_OBJECT` forbids
                        // anything else), whose members the object-members gate
                        // owns. A plain mixin's `with` clause is empty — this
                        // only fires for mixin classes and named applications.
                        let level: Vec<(StringId, FileId, StringId)> = mixin_container
                            .with_clause
                            .iter()
                            .chain(mixin_container.on_clause.iter())
                            .chain(mixin_container.implements_clause.iter())
                            .map(|face| (*face, *mixin_owner, mixin))
                            .collect();
                        if !level.is_empty() {
                            pending_faces.push(level);
                        }
                    }
                }
                if let Some((mixin_container, mixin_owner)) = mixin_resolved
                    && let Some(entry) = mixin_container.members.get(&member)
                    && !(for_super && entry.is_abstract)
                {
                    // The application-site substitution face: the member
                    // was found through `name`'s `with M<Arg>` edge, so its
                    // declaring signature reads through the application's
                    // written arguments (`XmlHasChildren<T>`'s `children`
                    // types `XmlNodeList<XmlNode>` on `XmlElement`, not
                    // `XmlNodeList<T>`). `mixin_env` composes the
                    // environment from `name`'s own header clause — the
                    // same source `check_mixin_application_overrides`
                    // reads — and stays undecidable when the clause facts
                    // are unknowable (a foreign application class), where
                    // the raw entry keeps the under-report posture.
                    let projected = self.boundary.member(mixin_owner, mixin, member, entry);
                    let found_entry =
                        match self.mixin_env(mixin_writer, mixin, mixin_owner, &projected, &mut facts) {
                            Instantiated::Applied(env) => Projected::Foreign(std::rc::Rc::new(
                                self.substitute_entry(&projected, &env),
                            )),
                            Instantiated::Unchanged | Instantiated::Undecidable => projected,
                        };
                    self.member_faces
                        .found_origin
                        .set(Some((mixin, mixin_owner)));
                    return Walk::Found(found_entry);
                }
            }
            // A mixin's `on` clause is its superclass face: the application
            // `S with M` constrains `S` below every `on` head, so a receiver
            // typed as the mixin answers through the on-types before its
            // `implements` faces — the on-position shadows the interface
            // position exactly like a superclass does (the official
            // interface build treats `on A, B` as the superclass `A` plus
            // the additional constraint faces `B`). The first head rides
            // the superclass climb; the remaining heads join this level's
            // faces ahead of the interfaces. The dispatch walk has no `on`
            // edges: a mixin's constraints sit on its `superImplemented`
            // face, never on the application's `implemented` — `super.m`
            // inside `class C extends S with M` sees S's concrete chain,
            // never M's constraint members.
            let mixin_on_head = (!dispatch && found.kind == ContainerKind::Mixin)
                .then(|| found.on_clause.split_first().map(|(head, _)| *head))
                .flatten();
            let mut level: Vec<(StringId, FileId, StringId)> = found
                .on_clause
                .iter()
                .skip(usize::from(mixin_on_head.is_none()))
                .chain(found.implements_clause.iter())
                .map(|face| (*face, found_owner, name))
                .collect();
            if !level.is_empty() && !dispatch {
                pending_faces.push(std::mem::take(&mut level));
            }
            let Some(superclass) = found.superclass.or(mixin_on_head) else {
                // The dispatch chain ends at a class without a superclass:
                // no `implements`/`on` face ever answers it (the oracle's
                // `implemented` map carries no interface signatures). An
                // enum still reports its implicit `Enum` superinterface —
                // it IS the dispatch superclass.
                if dispatch && found.kind != ContainerKind::Enum {
                    return Walk::Missing;
                }
                // An enum's implicit superinterface (`Enum`, dart:core —
                // every enum declaration implements it, so `index` and the
                // `EnumName` extension's receiver resolve through it): the
                // interface joins the climb as the enum's own level, written
                // by the enum in its declaring file.
                if found.kind == ContainerKind::Enum {
                    pending_faces.push(vec![(self.intern("Enum"), found_owner, name)]);
                }
                // The `extends` chain ends: the collected interfaces still
                // contribute members — every face met on the climb, deepest
                // level first (a superclass interface already merged its
                // `implements` faces, so it shadows the implementor's). The
                // shared face-list climb then expands each face's own
                // supertypes transitively. No faces at all is the clean
                // miss — an SDK-stub anchor routes to the frozen consult.
                if pending_faces.is_empty() {
                    return Walk::Missing;
                }
                return self.walk_face_list(
                    pending_faces.iter().rev().flatten().copied(),
                    owner_lib,
                    member,
                    &mut seen,
                    false,
                );
            };
            // The superclass name is written in the declaring container's
            // file and resolves through that container's library (#491 S2a).
            scope_lib = owner_lib;
            writer = name;
            name = superclass;
            owner = found_owner;
        }
    }

    /// Whether an unresolvable supertype hop is a dead edge rather than a
    /// broken chain — the shared probe ([`supertype_hop_dead`]) the member
    /// walk, the face-list climb, and the super-formal face ask.
    pub(crate) fn supertype_hop_dead(&self, name: StringId, lib: LibraryId, file: FileId) -> bool {
        supertype_hop_dead(self.graph, self.namespaces, self.index, lib, file, name)
    }

    /// The shared breadth-first face-list climb behind the
    /// `implements`-face merges of [`Engine::walk_from`], the super walk,
    /// and the mixin's `on`-constraint super walk: each seed `(name,
    /// declaring file, writer container)` contributes its own (non-static)
    /// members and, transitively, its supertypes' (a class climbs
    /// `extends`/`with`/`implements`, a mixin interface its own interfaces,
    /// a nested extension type its `implements`). A resolved member ends the
    /// walk (`Found`); an unresolvable or cyclic face poisons the chain
    /// (`ChainBroken`, the under-report boundary) only when nothing
    /// answered. Each face name is written in its declaring file and
    /// resolves through that container's library; a prefixed spelling in the
    /// writer's own header binds through the prefix
    /// ([`Engine::resolve_clause_face`]).
    pub(crate) fn walk_face_list(
        &self,
        seeds: impl Iterator<Item = (StringId, FileId, StringId)>,
        mut scope_lib: LibraryId,
        member: StringId,
        seen: &mut FxHashSet<StringId>,
        climb_on: bool,
    ) -> Walk<'a> {
        // Breadth-first: the declaration order is the oracle's face order,
        // and a shallower hit shadows a deeper one.
        let mut queue: VecDeque<(StringId, FileId, StringId)> = seeds.collect();
        let mut missing = false;
        while let Some((face, face_owner, face_writer)) = queue.pop_front() {
            if !seen.insert(face) {
                continue;
            }
            // The seed's spelling is written in `face_owner`'s file and
            // resolves through that file's library — not a probe keyed on the
            // face declaring there (a queued `implements` face usually
            // declares elsewhere, #2313).
            let face_lib = self.graph.library_of_file(face_owner).unwrap_or(scope_lib);
            let Some((face_container, face_container_owner)) =
                self.resolve_clause_face(face_writer, face_owner, face_lib, face)
            else {
                // An unresolvable interface face — a dead edge (undefined,
                // non-class, or prefixed-miss spelling, the oracle's
                // InvalidType) contributes nothing; only a declaration the
                // index cannot model (the synthetic SDK face) records the
                // broken hop. Either way the remaining faces keep walking.
                if !self.clause_hop_dead(face_writer, face_owner, face_lib, face) {
                    missing = true;
                }
                continue;
            };
            scope_lib = face_lib;
            if let Some(entry) = face_container.members.get(&member)
                && !entry.is_static
            {
                self.member_faces
                    .found_origin
                    .set(Some((face, face_container_owner)));
                return Walk::Found(self.boundary.member(
                    face_container_owner,
                    face,
                    member,
                    entry,
                ));
            }
            // Climb the face's own supertypes: `extends` and `with` for
            // classes, `implements` for extension types, and a mixin
            // interface's own interfaces. A mixin constraint face also climbs
            // its own `on` constraints — the oracle's `superLookupType`
            // interface includes them — but only on the mixin-super walk;
            // the extension-type `implements` walk leaves `on` untouched.
            let on_clause = if climb_on {
                &face_container.on_clause[..]
            } else {
                &[]
            };
            for super_face in face_container
                .superclass
                .into_iter()
                .chain(face_container.with_clause.iter().copied())
                .chain(face_container.implements_clause.iter().copied())
                .chain(on_clause.iter().copied())
            {
                queue.push_back((super_face, face_container_owner, face));
            }
        }
        if missing {
            Walk::ChainBroken
        } else {
            Walk::Missing
        }
    }

    /// `super.member`: the DISPATCH walk — the official `getMember(element,
    /// name, forSuper: true)` (`inheritance_manager3.dart:223-265`), the
    /// concrete members on the superclass-and-applied-mixin spine only.
    /// The walk starts at the enclosing container but skips its own
    /// members — the `S&M2` application sequence still applies (`super.m`
    /// in `class C extends S with M1, M2` resolves M2 over M1 over S).
    /// Abstract members never answer here (they are not `implemented`), and
    /// `implements`/`on` faces contribute signatures only — the interface
    /// leg lives in [`Self::walk_super_interface`], consulted for the
    /// `abstract_super_member_reference` verdict.
    ///
    /// A container without a captured superclass edge and without mixins
    /// (mixins: `on` constraints are not edges) answers `Missing` — the
    /// official walk still reaches `Object`, whose members the caller's
    /// `member_defined` gate resolves.
    pub(crate) fn walk_super(&self, container: StringId, member: StringId) -> Walk<'a> {
        let Some((container_decl, _)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        ) else {
            return Walk::ChainBroken;
        };
        if container_decl.superclass.is_none() && container_decl.with_clause.is_empty() {
            // A mixin's super face is its `on` clause: `super.m` inside
            // `mixin M on A, B` resolves `m` against the superclass
            // constraints' member pools (each constraint's own members, then
            // its supertypes — the oracle's `unpromotedThisType` interface,
            // type_analyzer.dart `superLookupType`). The constraints'
            // members answer abstract or concrete — the official
            // `superInterface` merge keeps both
            // (inheritance_manager3.dart:1013-1015), so this arm never runs
            // the abstract filter the dispatch climb applies. Constraints
            // are not supertype edges in the model, so the regular
            // `walk_from` never sees them; seed the shared face-list climb
            // with the `on` list.
            if container_decl.kind == ContainerKind::Mixin && !container_decl.on_clause.is_empty() {
                let mut seen: FxHashSet<StringId> = FxHashSet::default();
                seen.insert(container);
                return self.walk_face_list(
                    container_decl
                        .on_clause
                        .iter()
                        .map(|face| (*face, self.file, container)),
                    self.lib,
                    member,
                    &mut seen,
                    true,
                );
            }
            // No explicit supertype: the super face is still `Object`. The
            // official walk answers every class through it, so a clean miss
            // reports (`undefined_super_member` keeps firing after the
            // `?.`-on-super operator error —
            // invalid_operator_questionmark_period_for_super.dart) while
            // `Object` members resolve through the caller's `member_defined`
            // gate.
            return Walk::Missing;
        }
        // An `augment` fragment's container row carries the introduction's
        // clauses through the merged element face (`resolve_container` →
        // `fragment_face` — `super.foo` inside `augment class B` climbs the
        // BASE declaration's `extends`/`with` spine, exactly like the
        // merged element upstream). A fragment without a resolvable
        // introduction keeps the bare row's empty clauses and the `Missing`
        // verdict above — the `undefined_super_member` report the oracle
        // emits on the augment-without-declaration corpus face.
        self.member_faces
            .walk(container, member, true, || {
                self.walk_dispatch(container, member)
            })
    }

    // —— invalid_override (#453: the inheritance-override family) ——————

    /// Checks one instance-member declaration against the container's direct
    /// super-faces (upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/error/inheritance_override.dart:498-546`
    /// `_checkDeclaredMember` / `_checkDeclaredField`): for each face, the
    /// nearest same-named non-static member; kind mismatches skip (other
    /// checks own them), and a decidable name-level signature conflict
    /// emits `invalid_override` at the member's name span. A field checks
    /// through its implicit getter and — non-final only — implicit setter
    /// faces, so a field overriding a getter of an unfit declared type
    /// reports exactly like the oracle (`mixin XmlHasAttributes
    /// implements XmlAttributesBase` with a `XmlNodeList<XmlAttribute>`
    /// field over `List<XmlAttribute> get attributes`). Every
    /// unresolvable hop, cycle, or incomparable type suppresses — the
    /// under-report direction, never a false positive.
    ///
    /// Runs from the parse listener, so the element model (built by the
    /// earlier `collect`) is complete and the declaration's name token
    /// supplies the anchor span. Emissions repeat per failing face, matching
    /// the oracle's per-superinterface loop. Each row carries the oracle's
    /// context row anchored at the overridden member's declaration
    /// ([`overridden_member_context`]).
    /// The member name as the oracle's `invalid_override` message renders
    /// it: a setter's model spelling is `name=` (the setter marker
    /// [`Self::setter_spelling`] appends), rendered back as the bare `name`;
    /// an operator whose own spelling ends in `=` (`==`, `[]=`) is NOT a
    /// setter spelling — its trailing `=` is part of the declared name and
    /// renders intact (`bool operator==(Object)` displays `'B.=='`, never
    /// `'B.='`). The discriminator is the character before the `=`: a real
    /// setter spelling follows an identifier char, an operator's `=` follows
    /// `=`/`]`/another operator glyph (upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/dart/element/element.dart`'s
    /// `Element.displayName` — the element name carries no setter marker).
    fn member_display_name<'n>(&self, spelling_text: &'n str) -> &'n str {
        match spelling_text.strip_suffix('=') {
            Some(bare)
                if bare
                    .chars()
                    .last()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$') =>
            {
                bare
            }
            _ => spelling_text,
        }
    }

    /// `invalid_override_of_non_virtual_member` (upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/error/best_practices_verifier.dart:405-451`
    /// `visitFieldDeclaration` and `:595-648` `visitMethodDeclaration`): an
    /// instance member whose same-named INHERITED CONCRETE member —
    /// `InterfaceElement.getInheritedConcreteMember`
    /// (`inheritance_manager3.dart:159-174`, the superclass-and-mixins map,
    /// never the `implements` faces) — carries `@nonVirtual` metadata
    /// (`_hasNonVirtualAnnotation`, :1550-1557).
    ///
    /// The lookup names follow the oracle's per-declaration shape: a method
    /// and an accessor answer their own spelling (a setter's model spelling
    /// already carries the `=` marker), while a FIELD tries its getter name
    /// first and only falls back to the setter name when the getter misses
    /// (:427-430's `??` — the annotation of the entry the first lookup
    /// found is the only one read). A static member never overrides, and a
    /// duplicate declaration's report belongs to `duplicate_definition`.
    pub(crate) fn check_non_virtual_override(&mut self, spelling: StringId, range: SourceRange) {
        let Some((container, kind)) = self.container_context() else {
            return;
        };
        if matches!(
            kind,
            ContainerKind::Extension | ContainerKind::ExtensionType
        ) {
            return;
        }
        let owner = {
            let Some(elements) = self.index.get(self.file) else {
                return;
            };
            let Some(decl) = elements.container(container) else {
                return;
            };
            let Some(entry) = decl.members.get(&spelling) else {
                return;
            };
            if entry.is_static {
                return;
            }
            if elements
                .duplicates()
                .iter()
                .any(|d| d.name == spelling && d.range == range)
            {
                return;
            }
            // The lookup pair. A written setter's model spelling carries the
            // `=` marker, so the bare name is stripped back for the walk and
            // the face it asks is the setter face; every other member
            // answers its written spelling as the getter-face name. A FIELD
            // tries the getter face first and falls back to the setter face
            // (the oracle's `??` at :427-430).
            let setter_face = entry.kind == MemberKind::Setter;
            let bare = if setter_face {
                let text = self.strings.resolve(spelling);
                self.intern(self.member_display_name(text))
            } else {
                spelling
            };
            let setter = self.setter_spelling(bare);
            let mut hit =
                self.non_virtual_inherited_hit(decl, container, bare, setter, setter_face);
            if hit.is_none() && entry.kind == MemberKind::Field {
                hit = self.non_virtual_inherited_hit(decl, container, bare, setter, true);
            }
            let Some((found, found_owner)) = hit else {
                return;
            };
            if !self.declares_non_virtual(found) {
                return;
            }
            found_owner
        };
        let display = self.strings.resolve(spelling);
        let display = self.member_display_name(display);
        let owner = self.strings.resolve(owner);
        let message = format!(
            "The member '{display}' is declared non-virtual in '{owner}' and can't be \
             overridden in subclasses."
        );
        self.report.diagnostics.push(Diagnostic::new(
            INVALID_OVERRIDE_OF_NON_VIRTUAL_MEMBER.dtype.severity(),
            INVALID_OVERRIDE_OF_NON_VIRTUAL_MEMBER,
            message,
            self.file,
            range,
            None,
        ));
    }

    /// The most specific CONCRETE instance member the container inherits
    /// for the lookup name — upstream `getInheritedConcreteMap`'s
    /// `superImplemented.last` (`inheritance_manager3.dart:159-174`): the
    /// superclass-and-mixins map, built by overlaying each face's own
    /// concrete members in application order (`_addImplemented` :333-352,
    /// so a later mixin's interface shadows an earlier one's and the
    /// superclass's). The walk is pre-order over the written
    /// `with`/`extends` edges — each face's own members before its
    /// ancestors', and each level's faces visited last-written mixin first
    /// ([`push_concrete_faces`]) — which reproduces that overlay order. An
    /// unresolvable hop and a cycle are skipped, and an abstract
    /// redeclaration never shadows (it is absent from the official map), so
    /// the search keeps running past both.
    ///
    /// Narrower than `diagnose::enum_verifier`'s `inherited_concrete_member`
    /// on purpose: that face answers which container declares the name (for
    /// the enum-constant superset checks) and stops at one mixin level,
    /// while this one answers the inherited ENTRY (the `@nonVirtual` metadata
    /// is the question), kind-matches the getter/setter lookup forms, and
    /// descends through a mixin's own application faces.
    fn non_virtual_inherited_hit(
        &self,
        decl: &'a Container,
        container: StringId,
        plain: StringId,
        setter: StringId,
        setter_face: bool,
    ) -> Option<(&'a MemberEntry, StringId)> {
        if decl.superclass.is_none() && decl.with_clause.is_empty() {
            // Only the implicit `Object` face is inherited, and its members
            // carry no annotation this face reads.
            return None;
        }
        let mut pending: Vec<(StringId, StringId, FileId, LibraryId)> = Vec::new();
        push_concrete_faces(decl, container, self.file, self.lib, &mut pending);
        let mut seen: FxHashSet<StringId> = FxHashSet::default();
        seen.insert(container);
        while let Some((face, writer, writer_file, lib)) = pending.pop() {
            if !seen.insert(face) {
                continue;
            }
            let Some((face_decl, face_file)) =
                self.resolve_clause_face(writer, writer_file, lib, face)
            else {
                continue;
            };
            if let Some(entry) = concrete_member_of(face_decl, plain, setter, setter_face) {
                return Some((entry, face));
            }
            let face_lib = self.owner_library_of(face_file, face).unwrap_or(lib);
            push_concrete_faces(face_decl, face, face_file, face_lib, &mut pending);
        }
        None
    }

    /// Whether the entry carries the `nonVirtual` annotation — name-level
    /// recognition, the posture `site_annotation` keeps for
    /// `@mustCallSuper` (a prefixed `@meta.nonVirtual` joins through its
    /// last segment). The model keeps the written star on the entry itself,
    /// so a cross-file ancestor answers too.
    fn declares_non_virtual(&self, entry: &MemberEntry) -> bool {
        entry.annotations.iter().any(|annotation| {
            self.strings
                .resolve(annotation.name)
                .rsplit('.')
                .next()
                .is_some_and(|segment| segment == "nonVirtual")
        })
    }

    pub(crate) fn check_override(&mut self, spelling: StringId, range: SourceRange) {
        // Member declarations parse inside a container frame; extensions and
        // extension types have no inheritance faces.
        let Some((container, kind)) = self.container_context() else {
            return;
        };
        if matches!(
            kind,
            ContainerKind::Extension | ContainerKind::ExtensionType
        ) {
            return;
        }
        // The annotation-driven `@nonVirtual` face runs first: upstream it
        // lives in a separate verifier
        // (`best_practices_verifier.dart`'s `visitMethodDeclaration` and
        // `visitFieldDeclaration`, :595-648 / :405-451), so neither the
        // disallowed-clause gate nor the declared-face loop below
        // suppresses it.
        self.check_non_virtual_override(spelling, range);
        let Some(elements) = self.index.get(self.file) else {
            return;
        };
        let Some(decl) = elements.container(container) else {
            return;
        };
        // The clause gate, the first thing the oracle's `verify()` does
        // (:312-314): a disallowed direct-supertype clause reports there and
        // returns, so the declared-face loop below never runs on this
        // container's members.
        if self.disallowed_clause_suppresses_members(decl, kind) {
            return;
        }
        let Some(entry) = decl.members.get(&spelling) else {
            return;
        };
        if entry.is_static {
            return;
        }
        // A duplicate (second) declaration keeps the first entry in the
        // model — this span's signature is not the one recorded, and the
        // duplicate_definition owns that report.
        if elements
            .duplicates()
            .iter()
            .any(|d| d.name == spelling && d.range == range)
        {
            return;
        }
        // The direct super-faces, in the oracle's order: the supertype, the
        // `with` mixins in application order, the `implements` interfaces,
        // and the `on` constraints (mixins only — the model leaves the clause
        // empty for every other kind). The implicit superinterfaces join the
        // supertype slot exactly as the analyzer's `element.supertype` does:
        // a `class C {}` supertype is `Object` (upstream
        // `element.supertype` is never null for a class), an enum's is
        // `Enum` — both names-only faces here, so their members resolve
        // through the frozen object/enum signatures
        // ([`Engine::override_face_member`]'s third arm).
        let mut faces = Vec::with_capacity(
            usize::from(decl.superclass.is_some())
                + decl.with_clause.len()
                + decl.implements_clause.len()
                + decl.on_clause.len(),
        );
        match decl.superclass {
            Some(superclass) => faces.push(superclass),
            None => {
                match kind {
                    ContainerKind::Class => faces.push(self.intern("Object")),
                    ContainerKind::Enum => faces.push(self.intern("Enum")),
                    // An `on`-less mixin constrains `Object` (the analyzer's
                    // `superclassConstraints` default).
                    ContainerKind::Mixin if decl.on_clause.is_empty() => {
                        faces.push(self.intern("Object"))
                    }
                    _ => {}
                }
            }
        }
        faces.extend(decl.with_clause.iter().copied());
        faces.extend(decl.implements_clause.iter().copied());
        faces.extend(decl.on_clause.iter().copied());
        // The written clause facts, built on first need — an inherited
        // signature carrying free type parameters — and shared across the
        // face loop and the combined-signature inference (one token walk
        // per check).
        let mut facts: Option<HeaderClauses> = None;
        // The combined-signature inference (upstream Dart SDK source,
        // `pkg/analyzer/lib/src/summary2/instance_member_inferrer.dart`
        // `_inferAccessorOrField`/`_inferExecutable`): an implicit type slot
        // on the declared member infers from the combined superinterface
        // signature — the member type the oracle compares — rather than
        // staying `dynamic`. The covariant-propagation flag rides with it
        // (an overridden covariant setter marks the induced
        // one).
        let setter_name = self.setter_spelling(spelling);
        let (effective, inferred_setter_covariant) =
            self.effective_declared(container, entry, spelling, setter_name, &faces, &mut facts);
        // The `no_combined_super_signature` face (upstream Dart SDK source,
        // `inheritance_override.dart` `_reportNoCombinedSuperSignature`, fed
        // by `instance_member_inferrer.dart` `_inferExecutable`'s null
        // `combineSignatureTypes`): a METHOD with an implicit type slot whose
        // multi-candidate set has no combined signature reports at the
        // method name — and the declared-face override loop below skips the
        // member entirely (the oracle's `continue`), the inference's
        // `dynamic` fallback never being a face worth judging.
        if entry.kind == MemberKind::Method
            && has_implicit_slot(entry)
            && let Some(signatures) =
                self.no_combined_super_candidates(container, spelling, &faces, &mut facts)
        {
            let class_name = self.strings.resolve(container);
            self.emit(SemanticCode::NoCombinedSuperSignature, range, class_name, &signatures);
            return;
        }
        let entry = &effective;
        // The declared faces the oracle checks (upstream Dart SDK source,
        // `pkg/analyzer/lib/src/error/inheritance_override.dart`
        // `_checkDeclaredField` → `_checkDeclaredMember(name, field.getter)`
        // / `(name, field.setter)`): a field rides its implicit getter and —
        // a non-final field only — its implicit setter, each a separate
        // executable element; every other member shape rides itself. A
        // field's getter face compares through the `Field` entry itself
        // (the override faces read a field's declared type exactly like a
        // getter's return type, and the signature text renders
        // `{ty} Function()`); its setter face is a synthesized setter entry
        // (one required positional of the declared type) so the parameter
        // arm compares it like any written setter.
        let mut declared_faces: Vec<(MemberKind, Option<MemberEntry>)> = Vec::with_capacity(2);
        if entry.kind == MemberKind::Field {
            declared_faces.push((MemberKind::Getter, None));
            if !entry.is_final {
                let setter_face = MemberEntry {
                    kind: MemberKind::Setter,
                    formals: Some(Box::new(Formals {
                        positional: Box::new([Formal {
                            name: spelling,
                            ty: entry.var_ty,
                            required: true,
                            // The `covariant` field keyword marks the
                            // induced setter's parameter directly
                            // (`element_builder.dart:504`
                            // `valueFragment.isExplicitlyCovariant`);
                            // `inferred_setter_covariant` is the
                            // propagation arm (an overridden covariant
                            // setter marks this one).
                            covariant: entry.covariant || inferred_setter_covariant,
                            default_value: None,
                        }]),
                        named: Box::new([]),
                    })),
                    ..entry.clone()
                };
                declared_faces.push((MemberKind::Setter, Some(setter_face)));
            }
        } else {
            declared_faces.push((entry.kind, None));
        }
        // Per declared face (getter / setter) — the oracle's
        // `_checkDeclaredField` runs one `_checkDeclaredMember` per executable
        // element, each with its own helper: the function-subtype pass over
        // the direct super-faces, then the `CovariantParametersVerifier`.
        for (face_kind, face_entry) in &declared_faces {
            let setter_face = *face_kind == MemberKind::Setter;
            // A field's setter face resolves under the setter spelling;
            // every other face keeps the written name (a real setter
            // declaration arrives spelled `name=` already).
            let lookup = if face_entry.is_some() {
                self.setter_spelling(spelling)
            } else {
                spelling
            };
            // The per-face merged members, in the oracle's
            // `directSuperInterfaces` order — each written face contributes
            // its interface-map signature for the name, one hit per face,
            // kind-matched for the declared face's element kind. The face
            // each hit answered rides along: the `namedCandidates` build
            // below needs the super-and-`with` prefix boundary.
            let mut face_hits: Vec<(StringId, FaceHit<'a>)> = Vec::new();
            for face in faces.iter().copied() {
                let Some(hit) = self.override_face_member(container, face, lookup, &mut facts)
                else {
                    // Unresolvable or cyclic face chain: suppression
                    // territory (recovery residue in a clause lands here
                    // too).
                    continue;
                };
                // The inherited entry's element kind: a stored `Field` is its
                // implicit getter for the getter face (the oracle's
                // `field.getter`/`field.setter` split — `A { int x }` answers
                // the getter face exactly like a written `int get x`), while
                // the setter face resolves under `name=` where fields never
                // live — a non-final field answers it through the induced
                // setter the lookup synthesized, a written `Setter` as
                // itself.
                let kind_matches = match *face_kind {
                    MemberKind::Getter => {
                        hit.entry().kind == MemberKind::Getter
                            || hit.entry().kind == MemberKind::Field
                    }
                    _ => hit.entry().kind == *face_kind,
                };
                if kind_matches {
                    face_hits.push((face, hit));
                }
            }
            // The oracle's `namedCandidates[name]` (`inheritance_manager3.dart`'s
            // interface build, the `overridden` map `getOverridden` answers):
            // the superclass-and-`with` run collapses to ONE candidate — a
            // mixin's own declaration replaces the running candidate, a
            // member the mixin inherited merges with it — while every
            // `implements`/`on` face appends its interface-map member to the
            // list. The covariance-propagation read happens on THIS list —
            // never the transitive own-declaration walk — so a covariant
            // mark on an overridden-out ancestor (the superclass member a
            // mixin replaced) does not leak into the declared member.
            // `super_with_end` is the hit index boundary: the implicit
            // supertype slot plus the `with` mixins lead `faces` (the same
            // order the face list above was built), and dropped faces can
            // not desync it because `face_hits` records each hit's face.
            let super_with_end = decl.with_clause.len() + 1;
            // `running` indexes `face_hits`: the fold's live candidate (or
            // `None` after a conflicting merge — upstream drops the name).
            let mut running: Option<usize> = None;
            let mut tail: Vec<usize> = Vec::new();
            for (index, (face, hit)) in face_hits.iter().enumerate() {
                if index >= super_with_end {
                    tail.push(index);
                    continue;
                }
                let Some(current) = running else {
                    running = Some(index);
                    continue;
                };
                // `candidate.enclosingElement == mixinElement` — the hit's
                // declaring hop is the written mixin face itself (an
                // applied name spells its declaring container): the mixin
                // member replaces the running candidate outright. An
                // inherited-through member merges with it.
                if hit.face_name == *face {
                    running = Some(index);
                } else {
                    running = match self.merged_candidate_index(
                        &[&face_hits[current].1, hit],
                        container,
                        &mut facts,
                    ) {
                        // A conflicting pair keeps the running candidate —
                        // upstream writes nothing back into `namedCandidates`
                        // on a failed merge, so the previous entry stands.
                        Some(winner) => Some(if winner == 0 { current } else { index }),
                        None => Some(current),
                    };
                }
            }
            let mut candidates: Vec<&FaceHit<'a>> = Vec::new();
            if let Some(winner) = running {
                candidates.push(&face_hits[winner].1);
            }
            candidates.extend(tail.iter().map(|index| &face_hits[*index].1));
            // The covariance faces: every transitive superinterface's OWN
            // member (upstream `correct_override.dart:96-138`
            // `CovariantParametersVerifier._superParameters` over
            // `classHierarchy.implementedInterfaces`; `_correspondingMember`
            // reads each interface element's own declarations). They carry
            // the verifier rows — NOT the propagation marks.
            let super_faces = self.covariant_own_faces(container, lookup, *face_kind, &mut facts);
            // The `_inheritCovariance` propagation
            // (`inheritance_manager3.dart:1063-1130`): a parameter is
            // covariant when this member or any overridden member declares it
            // so. The marks drive `_computeThisTypeForSubtype`'s `Object?`
            // rewrite (the subtype pass skips every covariant pair) and the
            // verifier gate.
            let base = face_entry.as_ref().unwrap_or(entry);
            let marked = self.inherit_covariance(base, lookup, &candidates, &mut facts);
            let declared_cmp: &MemberEntry = marked.as_ref().unwrap_or(base);
            // The inherited members already judged broken within this
            // declared face, keyed by pointer — one diagnostic per inherited
            // member.
            let mut broken_seen: Vec<*const MemberEntry> = Vec::new();
            // The rows this declared face already emitted, keyed by
            // `(setter code, declared text, face text, inherited text)`:
            // identical rows collapse (the oracle's diagnostic dedup), and
            // the verifier pass below skips a row the subtype pass landed
            // (`String m(covariant String a)` over `int m(num a)` texts the
            // same row from both gates).
            let mut emitted: Vec<(bool, String, String, String)> = Vec::new();
            for (_, hit) in &face_hits {
                // The stacked climb can surface one inherited member through
                // several direct faces (the applied `S&M2` superclass face
                // and the `M2` face both answer M2's member); the oracle's
                // verdict per inherited member is one diagnostic. The
                // boundary's memo keeps a projected entry's identity stable
                // across faces; synthesized hits (a field's induced setter
                // face, an `Object` member) identity by their underlying
                // entry where one exists and stay per-face where the oracle
                // reports per superinterface.
                if let Some(ptr) = hit.identity() {
                    if broken_seen.contains(&ptr) {
                        continue;
                    }
                }
                // The inherited member's effective signature first
                // (`instance_member_inferrer.dart` on the DECLARING
                // container): the model row stores unwritten slots as
                // `Unknown`, but the element model the oracle compares
                // already inferred them — `B { method29(a) }` over `A {
                // method29(int a) }` compares `String` against `int`, never
                // against an `Unknown` that suppresses the verdict.
                let inferred;
                let inherited_entry: &MemberEntry = match self.inferred_inherited(
                    hit,
                    lookup,
                    &mut facts,
                ) {
                    Some(entry) => {
                        inferred = entry;
                        &inferred
                    }
                    None => hit.entry(),
                };
                // The super-face type-argument substitution
                // (`Substitution.fromInterfaceType`): the inherited signature
                // read through a generic super face carries that face's own
                // type parameters — the oracle compares the signature
                // instantiated by the edge's written arguments. Free
                // parameters in the inherited signature are the M1 model's
                // unsubstituted reads; the substitution face resolves them
                // from the written clause arguments (exact where the chain's
                // edges live in this file), suppresses the face where they do
                // not (the under-report direction).
                let substituted;
                let inherited_ref: &MemberEntry = match self.instantiate_inherited(
                    &hit.chain,
                    inherited_entry,
                    container,
                    &mut facts,
                ) {
                    Instantiated::Unchanged => inherited_entry,
                    Instantiated::Applied(env) => {
                        substituted = self.substitute_entry(inherited_entry, &env);
                        &substituted
                    }
                    Instantiated::Undecidable => continue,
                };
                if !self.override_broken(
                    declared_cmp,
                    (self.file, container),
                    inherited_ref,
                    (hit.origin, hit.face_name),
                ) {
                    continue;
                }
                if let Some(ptr) = hit.identity() {
                    broken_seen.push(ptr);
                }
                let code = if setter_face {
                    SemanticCode::InvalidOverrideSetter
                } else {
                    SemanticCode::InvalidOverride
                };
                let display = self.strings.resolve(spelling);
                let display = self.member_display_name(display);
                let container_text = self.strings.resolve(container);
                let declared_text = self.override_signature_text(declared_cmp);
                let face_text = self.strings.resolve(hit.face_name);
                let inherited_text = self.override_signature_text(inherited_ref);
                emitted.push((
                    setter_face,
                    declared_text.clone(),
                    face_text.to_owned(),
                    inherited_text.clone(),
                ));
                self.emit5_with_context(
                    code,
                    range,
                    display,
                    container_text,
                    &declared_text,
                    face_text,
                    &inherited_text,
                    overridden_member_context(inherited_ref, hit.origin, setter_face),
                );
            }
            // The `CovariantParametersVerifier` pass (upstream
            // `correct_override.dart:96-138`, wired from
            // `inheritance_override.dart:548-555` for the declared-member
            // path only — never the mixin-application face): every parameter
            // the propagated marks carry covariant must be RELATED to the
            // corresponding parameter of every transitive superinterface's
            // own member — `thisType <: superType || superType <: thisType`.
            // A pair related in neither direction lands one plain
            // `invalid_override` row per super member (never the setter
            // code), anchored at this member's name with that super member's
            // declaring interface and instantiated signature in the slots.
            let Some(declared_formals) = declared_cmp.formals.as_deref() else {
                continue;
            };
            for (super_entry, super_name, super_file) in &super_faces {
                let Some(super_formals) = super_entry.formals.as_deref() else {
                    continue;
                };
                let super_owner = (*super_file, *super_name);
                for (index, d) in declared_formals.positional.iter().enumerate() {
                    if !d.covariant {
                        continue;
                    }
                    let Some(i) = super_formals.positional.get(index) else {
                        continue;
                    };
                    if !self.covariant_pair_unrelated(
                        declared_cmp,
                        (self.file, container),
                        super_entry,
                        super_owner,
                        d,
                        i,
                    ) {
                        continue;
                    }
                    self.emit_covariant_row(
                        &mut emitted,
                        range,
                        spelling,
                        container,
                        declared_cmp,
                        super_entry,
                        *super_name,
                        *super_file,
                    );
                }
                for d in &declared_formals.named {
                    if !d.covariant {
                        continue;
                    }
                    let Some(i) = super_formals.named_formal(d.name) else {
                        continue;
                    };
                    if !self.covariant_pair_unrelated(
                        declared_cmp,
                        (self.file, container),
                        super_entry,
                        super_owner,
                        d,
                        i,
                    ) {
                        continue;
                    }
                    self.emit_covariant_row(
                        &mut emitted,
                        range,
                        spelling,
                        container,
                        declared_cmp,
                        super_entry,
                        *super_name,
                        *super_file,
                    );
                }
            }
        }
    }

    /// One `CovariantParametersVerifier` row, skipped when the subtype pass
    /// already emitted the identical row (`code`, declaring text, face text,
    /// inherited text are the oracle's diagnostic identity).
    #[allow(clippy::too_many_arguments)] // the row's slots, mirroring emit5
    fn emit_covariant_row(
        &mut self,
        emitted: &mut Vec<(bool, String, String, String)>,
        range: SourceRange,
        spelling: StringId,
        container: StringId,
        declared: &MemberEntry,
        inherited: &MemberEntry,
        face: StringId,
        face_file: FileId,
    ) {
        let display = self.strings.resolve(spelling);
        let display = self.member_display_name(display);
        let container_text = self.strings.resolve(container);
        let declared_text = self.override_signature_text(declared);
        let face_text = self.strings.resolve(face);
        let inherited_text = self.override_signature_text(inherited);
        let key = (
            false,
            declared_text.clone(),
            face_text.to_owned(),
            inherited_text.clone(),
        );
        if emitted.contains(&key) {
            return;
        }
        emitted.push(key);
        self.emit5_with_context(
            SemanticCode::InvalidOverride,
            range,
            display,
            container_text,
            &declared_text,
            face_text,
            &inherited_text,
            overridden_member_context(inherited, face_file, false),
        );
    }

    /// One `CovariantParametersVerifier` pair's relatedness (upstream
    /// `correct_override.dart:113-118`):
    /// `thisType <: superType || superType <: thisType`, asked through the
    /// shared subtype decision. Unrelated only when BOTH directions fail —
    /// an undecidable direction (`None`, an unresolvable head or a type
    /// parameter) keeps the under-report the subtype face grants.
    fn covariant_pair_unrelated(
        &self,
        declared: &MemberEntry,
        declared_owner: (FileId, StringId),
        inherited: &MemberEntry,
        inherited_owner: (FileId, StringId),
        d: &Formal,
        i: &Formal,
    ) -> bool {
        let world = self.name_world();
        let forward = self.override_decide(
            d.ty,
            declared_owner,
            declared,
            i.ty,
            inherited_owner,
            inherited,
            &world,
        );
        let backward = self.override_decide(
            i.ty,
            inherited_owner,
            inherited,
            d.ty,
            declared_owner,
            declared,
            &world,
        );
        matches!(forward, Some(false)) && matches!(backward, Some(false))
    }

    /// The with-clause mixin-application anchor face (upstream Dart SDK
    /// source, `pkg/analyzer/lib/src/error/inheritance_override.dart`
    /// `InheritanceOverrideVerifier.verify` lines 338-364): as each mixin
    /// application point completes, the oracle runs `_checkDeclaredMembers`
    /// for the mixin's own members against the super-interfaces accumulated
    /// so far — the superclass plus the mixins already applied, in with
    /// order — and only after the mixin loop adds `implements`/`on`. The
    /// diagnostic anchors at the written mixin name INCLUDING its type
    /// arguments (`XmlHasChildren<XmlNode>` = the whole `NamedType` span),
    /// and the message carries the mixin's declared signature instantiated
    /// at the application point. Fires once per container header at the
    /// with-clause event, while `container_context` is still the open
    /// container and `deferred_modifier.spans.buffered` still holds the
    /// with entries (the tail `with_clause.len()` entries — an `extends`
    /// overflow residue lands before them, never inside the tail window).
    ///
    /// The `element.interfaces`/`on` faces are deliberately absent from the
    /// per-mixin check set: upstream adds them only AFTER the mixin loop,
    /// so a mixin member that conflicts with an `implements` interface is
    /// `check_override`'s business at the member site, never an extra
    /// `INVALID_OVERRIDE` at the mixin span. Every unresolvable hop, cycle,
    /// or incomparable type suppresses — the under-report direction.
    ///
    /// `container` is the open container frame's name (the class / named
    /// mixin application / enum being declared). Wired from the
    /// with-clause listener events before `drain_with_clause_spans` moves
    /// the buffered spans into `pending_mixin_uses`.
    pub(crate) fn check_mixin_application_overrides(&mut self, container: StringId) {
        let Some(elements) = self.index.get(self.file) else {
            return;
        };
        let Some(decl) = elements.container(container) else {
            return;
        };
        if decl.with_clause.is_empty() {
            return;
        }
        // The same `verify()` clause gate (`:312-314`) that stands
        // `check_override` down: the mixin-member checks ride the verifier's
        // later `_checkDeclaredMembers` loop, one statement past the early
        // return, so a broken header suppresses them too.
        if self.disallowed_clause_suppresses_members(decl, decl.kind) {
            return;
        }
        // The written mixin spans, in application order — the tail window
        // of the clause buffer at the with-clause event. A claim/count
        // disagreement (recovery shape, unmodelable entry) leaves no spans
        // to pair: bail rather than anchor at a wrong offset.
        let with_count = decl.with_clause.len();
        let spans: Vec<SourceRange> = match self.deferred_modifier.as_ref() {
            Some(deferred) if deferred.spans.buffered.len() >= with_count => {
                deferred.spans.buffered[deferred.spans.buffered.len() - with_count..].to_vec()
            }
            _ => return,
        };
        // The oracle's accumulation set: the direct super-interfaces already
        // applied before each mixin — `[supertype]` then each prior mixin.
        // `element.interfaces`/`on` join only after the mixin loop, so they
        // never enter this face set.
        let mut accumulated: Vec<StringId> = Vec::with_capacity(with_count + 1);
        match decl.superclass {
            Some(superclass) => accumulated.push(superclass),
            // A `class C with M` supertype is still `Object` upstream
            // (`element.supertype` is never null for a class), an `enum E
            // with M`'s is `Enum` — the names-only faces join the
            // accumulation, their members the frozen signatures.
            None => match decl.kind {
                ContainerKind::Class => accumulated.push(self.intern("Object")),
                ContainerKind::Enum => accumulated.push(self.intern("Enum")),
                _ => {}
            },
        }
        // The written clause facts, built on first need — a mixin's own
        // signature carrying free type parameters — and shared across the
        // mixin loop (one token walk per check).
        let mut facts: Option<HeaderClauses> = None;
        for (index, &mixin) in decl.with_clause.iter().enumerate() {
            // Resolve the mixin through the diagnosing file's library: the
            // with-entry spellings are written in this file and resolve
            // through its scope, whatever file declares the container. A
            // with-entry that is not a class-or-mixin element is not a
            // valid superinterface upstream (`mixinType.isValidSuperinterface`)
            // — the element never enters `directSuperInterfaces`, so neither
            // its members nor its own accumulation slot contribute.
            let Some((mixin_container, mixin_owner)) =
                self.resolve_clause_face(container, self.file, self.lib, mixin)
            else {
                continue;
            };
            if !matches!(
                mixin_container.kind,
                ContainerKind::Class | ContainerKind::Mixin
            ) {
                continue;
            }
            // The declared side: the mixin's own members, methods then
            // getters then setters (upstream `_checkDeclaredMembers`
            // iterates `type.methods`, then `type.getters`, then
            // `type.setters`), statics skipped, a field riding its implicit
            // getter plus — non-final only — its implicit setter, the same
            // two-face split `check_override` applies. Each member is
            // boundary-projected first so its `Ty` slots read through the
            // diagnosing pool (the mixin body is a foreign file's pool).
            // Ordering the faces keeps the oracle's per-member loop
            // deterministic; NameMap's ascending-id order is only the
            // bucket source.
            let mut declared: Vec<(
                StringId,
                MemberKind,
                Option<MemberEntry>,
                Projected<'a, MemberEntry>,
            )> = Vec::new();
            for (spelling, member) in mixin_container.members.iter() {
                if member.is_static {
                    continue;
                }
                let projected = self.boundary.member(mixin_owner, mixin, *spelling, member);
                match member.kind {
                    MemberKind::Method => {
                        declared.push((*spelling, MemberKind::Method, None, projected))
                    }
                    MemberKind::Getter => {
                        declared.push((*spelling, MemberKind::Getter, None, projected))
                    }
                    MemberKind::Setter => {
                        declared.push((*spelling, MemberKind::Setter, None, projected))
                    }
                    MemberKind::Field => {
                        declared.push((*spelling, MemberKind::Getter, None, projected.clone()));
                        if !projected.is_final {
                            let setter_face = MemberEntry {
                                kind: MemberKind::Setter,
                                formals: Some(Box::new(Formals {
                                    positional: Box::new([Formal {
                                        name: *spelling,
                                        ty: projected.var_ty,
                                        required: true,
                                        // The `covariant` field keyword
                                        // marks the induced setter's
                                        // parameter
                                        // (`element_builder.dart:504`).
                                        covariant: projected.covariant,
                                        default_value: None,
                                    }]),
                                    named: Box::new([]),
                                })),
                                ..(*projected).clone()
                            };
                            declared.push((
                                *spelling,
                                MemberKind::Setter,
                                Some(setter_face),
                                projected,
                            ));
                        }
                    }
                }
            }
            // methods → getters → setters, matching `type.methods` /
            // `type.getters` / `type.setters`: fields contribute one getter
            // and one setter slot in-place, so a stable bucket order over
            // the tagged kinds reproduces the upstream sequence.
            declared.sort_by_key(|(_, kind, _, _)| match kind {
                MemberKind::Method => 0u8,
                MemberKind::Getter | MemberKind::Field => 1,
                MemberKind::Setter => 2,
            });
            // Per declared face (getter / setter): the inherited members
            // already judged broken, keyed by pointer — one diagnostic per
            // inherited member.
            let mut broken_seen: Vec<(bool, *const MemberEntry)> = Vec::new();
            for (spelling, face_kind, face_entry, projected) in &declared {
                let setter_face = *face_kind == MemberKind::Setter;
                // The declared face's signature under the application-site
                // substitution: a generic mixin member read through
                // `Mixin<XmlNode>` carries the application's written
                // arguments. Foreign single-parameter hops map the one free
                // spelling to the one written argument (the name-forced
                // rule); multi-parameter foreign hops stay undecidable and
                // suppress the face.
                let declared_cmp_owned;
                let declared_cmp: &MemberEntry =
                    match self.mixin_env(container, mixin, mixin_owner, projected, &mut facts) {
                        Instantiated::Unchanged => face_entry.as_ref().unwrap_or(projected),
                        Instantiated::Applied(env) => {
                            declared_cmp_owned = self
                                .substitute_entry(face_entry.as_ref().unwrap_or(projected), &env);
                            &declared_cmp_owned
                        }
                        Instantiated::Undecidable => continue,
                    };
                let lookup = if setter_face {
                    self.setter_spelling(*spelling)
                } else {
                    *spelling
                };
                for &face in &accumulated {
                    let Some(hit) = self.override_face_member(container, face, lookup, &mut facts)
                    else {
                        continue;
                    };
                    // The inherited entry's element kind against the
                    // declared face: a stored `Field` answers the getter
                    // face through its implicit getter (the same
                    // `field.getter` split `check_override` applies); the
                    // setter face resolves the induced setter the lookup
                    // synthesized.
                    let kind_matches = match *face_kind {
                        MemberKind::Getter => {
                            hit.entry().kind == MemberKind::Getter
                                || hit.entry().kind == MemberKind::Field
                        }
                        _ => hit.entry().kind == *face_kind,
                    };
                    if !kind_matches {
                        continue;
                    }
                    if let Some(ptr) = hit.identity() {
                        if broken_seen
                            .iter()
                            .any(|(tag, p)| *tag == setter_face && *p == ptr)
                        {
                            continue;
                        }
                    }
                    let substituted;
                    let inherited_ref: &MemberEntry = match self.instantiate_inherited(
                        &hit.chain,
                        hit.entry(),
                        container,
                        &mut facts,
                    ) {
                        Instantiated::Unchanged => hit.entry(),
                        Instantiated::Applied(env) => {
                            substituted = self.substitute_entry(hit.entry(), &env);
                            &substituted
                        }
                        Instantiated::Undecidable => continue,
                    };
                    if self.override_broken(
                        declared_cmp,
                        (mixin_owner, mixin),
                        inherited_ref,
                        (hit.origin, hit.face_name),
                    ) {
                        if let Some(ptr) = hit.identity() {
                            broken_seen.push((setter_face, ptr));
                        }
                        let display = self.strings.resolve(*spelling);
                        let display = self.member_display_name(display);
                        let container_text = self.strings.resolve(mixin);
                        let declared_text = self.override_signature_text(declared_cmp);
                        let face_text = self.strings.resolve(hit.face_name);
                        let inherited_text = self.override_signature_text(inherited_ref);
                        let code = if setter_face {
                            SemanticCode::InvalidOverrideSetter
                        } else {
                            SemanticCode::InvalidOverride
                        };
                        self.emit5_with_context(
                            code,
                            spans[index],
                            display,
                            container_text,
                            &declared_text,
                            face_text,
                            &inherited_text,
                            overridden_member_context(inherited_ref, hit.origin, setter_face),
                        );
                    }
                }
            }
            // The mixin joins the accumulated super-interfaces after its own
            // members were checked — the oracle's `directSuperInterfaces
            // .add(mixinType)` at the end of the iteration.
            accumulated.push(mixin);
        }
    }

    /// The mixin-application substitution environment for a declared mixin
    /// member's free container parameters: same-file mixins zip the written
    /// arguments onto the declaring header's own parameter spellings,
    /// foreign single-parameter hops map the one free spelling to the one
    /// written argument (the name-forced rule `instantiate_inherited`
    /// keeps), anything else stays undecidable — the under-report
    /// direction. The written arguments live in the diagnosing file's
    /// clause facts as the whole `Ty::Named` the clause scanner stored, so
    /// its own `args` list — not the one-element wrapping — supplies the
    /// substitution operands.
    fn mixin_env(
        &self,
        container: StringId,
        mixin: StringId,
        mixin_owner: FileId,
        member: &MemberEntry,
        facts: &mut Option<HeaderClauses>,
    ) -> Instantiated {
        // The declaring container's parameters that occur anywhere in this
        // member's signature — bare heads AND nested type-argument slots
        // (`XmlNodeList<T>`'s `T` is a free container parameter the bare
        // `super_free_params` walk deliberately misses, since the
        // substitution must reach into argument lists). Each spelling maps
        // once, in first-seen order.
        let mut free: Vec<StringId> = Vec::new();
        {
            let mut collect = |ty: Ty| self.free_params_in(ty, member, mixin_owner, &mut free);
            collect(member.var_ty);
            collect(member.return_ty);
            if let Some(formals) = member.formals.as_deref() {
                for formal in formals.positional.iter().chain(formals.named.iter()) {
                    collect(formal.ty);
                }
            }
        }
        if free.is_empty() {
            return Instantiated::Unchanged;
        }
        // The written arguments at the application site: the clause
        // scanner stores each super face as `(head, vec![whole_named_ty])`,
        // so the real argument list is the stored `Ty::Named`'s own `args`
        // ([`Self::face_args`] — an empty list is a known non-generic edge,
        // not an undecidable one).
        let written: Option<Vec<Ty>> = {
            let clause = facts.get_or_insert_with(|| self.scan_header_clauses());
            clause
                .for_name(container)
                .and_then(|c| c.supers.iter().find(|entry| entry.head == mixin))
                .and_then(|entry| self.face_args(&entry.args))
        };
        let Some(written) = written else {
            return Instantiated::Undecidable;
        };
        if mixin_owner == self.file {
            let clause = facts.get_or_insert_with(|| self.scan_header_clauses());
            let Some(params) = clause.for_name(mixin).map(|c| c.params.clone()) else {
                return Instantiated::Undecidable;
            };
            if params.len() == written.len() {
                return Instantiated::Applied(params.into_iter().zip(written).collect());
            }
            return Instantiated::Undecidable;
        }
        // Foreign hop: name-forced only when the declaration exposes exactly
        // one free parameter and the application writes exactly one argument.
        if free.len() == 1 && written.len() == 1 {
            let mut map = FxHashMap::default();
            map.insert(free[0], written[0]);
            return Instantiated::Applied(map);
        }
        Instantiated::Undecidable
    }

    /// One type's contribution to the free-parameter set the
    /// mixin-application environment resolves ([`Self::mixin_env`]):
    /// descends into argument lists so a parameter nested inside
    /// `Wrapper<T>` counts exactly like a bare `T`. A spelling qualifies
    /// when the declaring file's declaration set records it as a
    /// `TypeParam` and the member's own generics do not shadow it — the
    /// same gate [`Self::super_free_params`] applies, widened to nested
    /// argument positions.
    fn free_params_in(
        &self,
        ty: Ty,
        member: &MemberEntry,
        scope_file: FileId,
        free: &mut Vec<StringId>,
    ) {
        // The structural visit ([`crate::ty_fold`]) reaches every named
        // node — a parameter nested in a function-typed formal or a record
        // field is as free as a bare one.
        self.boundary.pool().visit_named(ty, &mut |named| {
            let Ty::Named { name, .. } = named else {
                return;
            };
            let is_type_param = !member.type_param_names.contains(&name)
                && self
                    .index
                    .get(scope_file)
                    .and_then(|f| f.decl(name))
                    .is_some_and(|decl| decl.kind == crate::model::MicroKind::TypeParam);
            if is_type_param && !free.contains(&name) {
                free.push(name);
            }
        });
    }

    /// The `@override` presence face (`override_verifier.dart`): an
    /// `@override`-annotated instance member must override *something* — the
    /// oracle's `_isOverride` is the name-level `getOverridden(name) !=
    /// null` across every superinterface, so any same-named non-static
    /// super member spares the report regardless of kind. Fields spare on
    /// either half (getter or, for a non-final field, the setter spelling);
    /// Object members always override (every class's superinterface carries
    /// them), so skipping them is exact. Extensions never join the oracle's
    /// `_currentClass` walk — every `@override` member there reports.
    /// Top-level shapes: plain functions fall through the oracle's switch
    /// (silent) while its getters/setters report, but this event cannot
    /// separate them — suppressed, the under-report direction.
    pub(crate) fn check_override_annotation(&mut self, spelling: StringId, range: SourceRange) {
        if !self.pending_annotation_is("override") {
            return;
        }
        let Some((container, kind)) = self.container_context() else {
            return;
        };
        let Some(elements) = self.index.get(self.file) else {
            return;
        };
        let Some(decl) = elements.container(container) else {
            return;
        };
        let Some(entry) = decl.members.get(&spelling) else {
            return;
        };
        // A duplicate declaration's span is not the model entry's.
        if elements
            .duplicates()
            .iter()
            .any(|d| d.name == spelling && d.range == range)
        {
            return;
        }
        if self.members.object_members.contains(&spelling) {
            return;
        }
        let in_extension = matches!(
            kind,
            ContainerKind::Extension | ContainerKind::ExtensionType
        );
        let mut spared = !in_extension && self.face_member_any(decl, container, spelling);
        if !spared && !in_extension && entry.kind == MemberKind::Field {
            let setter = self.setter_spelling(spelling);
            spared = self.face_member_any(decl, container, setter);
        }
        if !spared && !in_extension && entry.kind == MemberKind::Setter {
            // The accessor-twin face: an inherited FIELD induces the setter
            // face too (`_CastQueueList`'s `set _head` overrides
            // `QueueList`'s `int _head` field — the field's implicit setter,
            // #2172). The mirror arm above answers a declared field against
            // an inherited setter's spelling.
            let base_text = self.strings.resolve(spelling);
            let base_text = base_text.strip_suffix('=').unwrap_or(base_text);
            spared = self.face_member_any(decl, container, self.intern(base_text));
        }
        if spared {
            return;
        }
        let code = match entry.kind {
            MemberKind::Field => SemanticCode::OverrideOnNonOverridingField,
            MemberKind::Getter => SemanticCode::OverrideOnNonOverridingGetter,
            MemberKind::Setter => SemanticCode::OverrideOnNonOverridingSetter,
            MemberKind::Method => SemanticCode::OverrideOnNonOverridingMethod,
        };
        self.emit(code, range, "", "");
    }

    /// Whether any direct super-face carries a same-named non-static member
    /// (the oracle's transitive `getOverridden` — upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/error/override_verifier.dart:135`
    /// `currentClass.element.getOverridden(name)`, which reads the element's
    /// interface map, so the implicit supertype is part of the face set —
    /// via [`Engine::face_member`]'s per-face climb). An unresolvable face
    /// simply contributes nothing — except an unindexed dart:core interface,
    /// whose declared members the frozen [`CORE_INTERFACE_MEMBERS`] face
    /// answers (a user-declared same-named container resolves indexed and
    /// wins the face first). `container` is the declaring container whose
    /// clauses wrote the faces (the prefixed-clause probe's writer key).
    ///
    /// The implicit supertype joins the face set exactly as
    /// [`Self::check_override`] seeds it: `element.supertype` is never null
    /// for a class (`Object`), an enum's is `Enum` (its `index`/`toString`
    /// face answers the `@override` presence check — `enum E { @override
    /// int get index }` is a real override), and an `on`-less mixin
    /// constrains `Object`. Their members resolve through
    /// [`Engine::frozen_top_member`]'s frozen signatures when the element
    /// index carries no `Object`/`Enum` container.
    pub(crate) fn face_member_any(
        &self,
        decl: &Container,
        container: StringId,
        spelling: StringId,
    ) -> bool {
        let mut faces = Vec::with_capacity(
            usize::from(decl.superclass.is_some())
                + decl.with_clause.len()
                + decl.implements_clause.len()
                + decl.on_clause.len(),
        );
        match decl.superclass {
            Some(superclass) => faces.push(superclass),
            None => match decl.kind {
                ContainerKind::Class => faces.push(self.intern("Object")),
                ContainerKind::Enum => faces.push(self.intern("Enum")),
                ContainerKind::Mixin if decl.on_clause.is_empty() => {
                    faces.push(self.intern("Object"))
                }
                _ => {}
            },
        }
        faces.extend(decl.with_clause.iter().copied());
        faces.extend(decl.implements_clause.iter().copied());
        faces.extend(decl.on_clause.iter().copied());
        faces.into_iter().any(|face| {
            self.face_member(container, face, spelling).is_some()
                || self.frozen_top_member(face, spelling).is_some()
                || self.face_member_frozen(container, face, spelling)
        })
    }

    /// The frozen-table half of [`Self::face_member_any`]: climbs the face's
    /// resolvable supertype chain like [`Self::face_member`], but answers at
    /// the hops the element index cannot carry — an unindexed dart:core /
    /// `dart:collection` container decides by its frozen member face
    /// ([`CORE_TYPE_MEMBERS`] / [`CORE_INTERFACE_MEMBERS`], aliased for the
    /// collection mixins/bases that re-export a `dart:core` interface's
    /// members, #2161). A hop that resolves to an indexed container whose
    /// own member walk misses is a real miss and stops the climb. `writer`
    /// is the container whose clauses wrote `face`.
    pub(crate) fn face_member_frozen(
        &self,
        writer: StringId,
        face: StringId,
        spelling: StringId,
    ) -> bool {
        let spelling_text = self.strings.resolve(spelling);
        let frozen_hit = |name: &str| {
            let aliases: &[&str] = COLLECTION_INTERFACE_ALIASES
                .iter()
                .find(|(alias, _)| name == *alias)
                .map_or(&[][..], |(_, aliases)| aliases);
            CORE_INTERFACE_MEMBERS
                .iter()
                .chain(CORE_TYPE_MEMBERS.iter())
                .any(|(container, members)| {
                    (name == *container || aliases.contains(container))
                        && members.contains(&spelling_text)
                })
        };
        let mut name = face;
        let mut hop_writer = writer;
        let mut owner = self.file;
        let mut scope_lib = self.lib;
        let mut seen = FxHashSet::default();
        // Same pending queue [`Self::face_member`] drains: a resolved hop's
        // `implements`/`on` faces keep the frozen climb alive when the linear
        // `extends`/`with` chain ends (`UnmodifiableSetMixin implements Set`
        // reaching `Set`'s frozen members). Each entry carries the writer
        // hop's library — the queued spelling resolves through it, whatever
        // other hops the drain climbs in between (#2313).
        let mut pending: VecDeque<(StringId, FileId, LibraryId, StringId)> = VecDeque::new();
        loop {
            if !seen.insert(name) {
                return false;
            }
            let (found, found_owner) =
                match self.resolve_clause_face(hop_writer, owner, scope_lib, name) {
                    Some(pair) => pair,
                    None => {
                        // The unindexed hop — a dart:core/`dart:collection` face —
                        // answers by its frozen member face.
                        if frozen_hit(self.strings.resolve(name)) {
                            return true;
                        }
                        let Some((next, next_owner, next_lib, next_writer)) = pending.pop_front()
                        else {
                            return false;
                        };
                        name = next;
                        owner = next_owner;
                        scope_lib = next_lib;
                        hop_writer = next_writer;
                        continue;
                    }
                };
            if let Some(entry) = found.members.get(&spelling)
                && !entry.is_static
                && self.member_visible_to(found_owner, name, spelling, scope_lib)
            {
                return true;
            }
            // A resolved hop whose indexed members miss the spelling still
            // answers by its frozen face: the SDK units index only
            // concrete-bodied members (dart:core `List`'s abstract
            // `E operator [](int)`, `int get length` carry no entry), so a
            // RESOLVED core container decides by the same frozen table an
            // unresolvable one does — the name-keyed tables keep this to
            // the core heads.
            if frozen_hit(self.strings.resolve(name)) {
                return true;
            }
            let owner_lib = self
                .owner_library_of(found_owner, name)
                .unwrap_or(scope_lib);
            let mut mixin_hit = false;
            for mixin in found.with_clause.iter().rev() {
                if !seen.insert(*mixin) {
                    return false;
                }
                match self.resolve_clause_face(name, owner, owner_lib, *mixin) {
                    Some((mixin_container, _))
                        if mixin_container
                            .members
                            .get(&spelling)
                            .is_some_and(|entry| !entry.is_static) =>
                    {
                        return true;
                    }
                    // A resolved mixin hop's frozen face answers the same
                    // way (`with ListMixin<E>` — the alias folds to the
                    // indexed ListBase, whose abstract-only members miss,
                    // but List's frozen face owns them).
                    Some(_) if frozen_hit(self.strings.resolve(*mixin)) => {
                        mixin_hit = true;
                    }
                    None if frozen_hit(self.strings.resolve(*mixin)) => {
                        mixin_hit = true;
                    }
                    // An applied mixin CLASS contributes its own `with`
                    // faces to the interface (the named-application rule —
                    // see the member walk's mixin hop); the pending drain
                    // resolves them through the mixin's declaring file.
                    Some((mixin_container, mixin_owner)) => {
                        let mixin_owner_lib = self
                            .owner_library_of(mixin_owner, *mixin)
                            .unwrap_or(owner_lib);
                        for mixin_face in mixin_container.with_clause.iter() {
                            pending.push_back((
                                *mixin_face,
                                mixin_owner,
                                mixin_owner_lib,
                                *mixin,
                            ));
                        }
                    }
                    _ => {}
                }
            }
            if mixin_hit {
                return true;
            }
            for super_face in found.implements_clause.iter().chain(found.on_clause.iter()) {
                pending.push_back((*super_face, found_owner, owner_lib, name));
            }
            match found.superclass {
                Some(superclass) => {
                    scope_lib = owner_lib;
                    hop_writer = name;
                    name = superclass;
                    owner = found_owner;
                }
                None => {
                    let Some((next, next_owner, next_lib, next_writer)) = pending.pop_front()
                    else {
                        return false;
                    };
                    name = next;
                    owner = next_owner;
                    scope_lib = next_lib;
                    hop_writer = next_writer;
                }
            }
        }
    }

    /// The `must_call_super` probe (`must_call_super_verifier.dart:19-175`),
    /// run at the declaration head: an instance member overriding a
    /// `@mustCallSuper`-annotated member latches the verdict for the body's
    /// end. The walk covers the mixin/superclass faces only (never
    /// `implements`), mixins before the supertype, transitively with the
    /// visited-set climb [`Engine::face_entries`] supplies; the concrete
    /// gate is the oracle's `_hasConcreteSuperMethod` /
    /// `lookupInheritedConcreteMember` pair, approximated by the same
    /// concrete hit. Annotation lookups read this file's collected
    /// declaration sites — ancestors from other files, or later in the
    /// unit, under-report.
    pub(crate) fn probe_must_call_super(&mut self, spelling: StringId, range: SourceRange) {
        self.members.pending_must_call_super = None;
        let Some((container, kind)) = self.container_context() else {
            return;
        };
        if matches!(
            kind,
            ContainerKind::Extension | ContainerKind::ExtensionType
        ) {
            return;
        }
        let Some(elements) = self.index.get(self.file) else {
            return;
        };
        let Some(decl) = elements.container(container) else {
            return;
        };
        let Some(entry) = decl.members.get(&spelling) else {
            return;
        };
        if entry.is_static || entry.is_abstract {
            return;
        }
        if elements
            .duplicates()
            .iter()
            .any(|d| d.name == spelling && d.range == range)
        {
            return;
        }
        let mut faces: Vec<StringId> = decl.with_clause.to_vec();
        faces.extend(decl.superclass);
        let mut annotated_owner: Option<StringId> = None;
        let mut concrete = false;
        for face in faces {
            let Some(hits) = self.face_entries(face, true) else {
                continue;
            };
            for hit in hits {
                if hit.name != spelling {
                    continue;
                }
                if annotated_owner.is_none()
                    && hit.owner_file == self.file
                    && self.site_annotation(hit.owner_name, spelling, "mustCallSuper")
                {
                    annotated_owner = Some(hit.owner_name);
                }
                if !hit.entry.is_abstract {
                    concrete = true;
                }
            }
        }
        if let (Some(owner), true) = (annotated_owner, concrete) {
            self.members.pending_must_call_super = Some((range, owner));
        }
    }

    /// The body's-end adjudication of the [`Engine::probe_must_call_super`]
    /// latch: the report fires when the body never touched `super` (the
    /// per-frame `invokesSuperSelf` face — nested function literals carry
    /// their own frame). The `{0}` slot is the annotated ancestor's
    /// container name.
    pub(crate) fn finish_must_call_super(&mut self, super_invoked: Option<bool>) {
        let Some((range, owner)) = self.members.pending_must_call_super.take() else {
            return;
        };
        if super_invoked != Some(false) {
            return;
        }
        let text = self.strings.resolve(owner);
        self.emit(SemanticCode::MustCallSuper, range, text, "");
    }

    // —— the W5 member-invocation pair (#748) ————————————————————————————

    /// Arms the `==` operator-parameter latch at the method head
    /// (`_checkForNullableEqualsParameterType`,
    /// best_practices_verifier.dart:1345-1374): the check keys on the
    /// OPERATOR token — a plain method named `equals` never matches (the
    /// official reads `node.name.type == TokenType.EQ_EQ`). The parameter
    /// question adjudicates when the method's own parameter list closes.
    pub(crate) fn begin_equals_parameter(&mut self, name: Token) {
        if self.text_of(name) != Some("==") {
            return;
        }
        self.members.equals_check = Some(EqualsCheck {
            name_range: name.range,
            depth: 0,
            first_ty: None,
            poisoned: false,
            ty_seen: false,
        });
    }

    /// Arms the `noSuchMethod` body-shape latch at the method head
    /// (`_checkForUnnecessaryNoSuchMethod`,
    /// best_practices_verifier.dart:1433-1478): the official keys on the
    /// raw name lexeme, so getters/setters spelled `noSuchMethod` join the
    /// same face (probed: both delegate shapes report there).
    pub(crate) fn begin_no_such_method(&mut self, name: Token) {
        if self.text_of(name) != Some("noSuchMethod") {
            return;
        }
        self.members.nsm_check = Some(NsmCheck {
            name_range: name.range,
            shape: NsmShape::Open,
            body_block: None,
            armed: false,
            in_return: false,
            return_seen: false,
            return_ok: false,
            return_kw: None,
            return_semi_end: None,
            name_ok: false,
        });
    }

    /// The `==` latch's parameter-list progress: `begin_formal_parameters`
    /// nesting (the outermost list is the method's own; function-typed
    /// formals nest theirs inside it).
    pub(crate) fn equals_formal_boundary(&mut self) {
        self.members.equals_formal_boundary();
    }

    /// Snapshots the first outer-list formal's written type. The written
    /// face decides (the briefing pins it over the official's resolved
    /// type — a type-alias-to-`Object` under-reports, the documented
    /// posture). Fires at the formal's close and again at a default-value
    /// expression's open, whichever lands first before any closure or
    /// function-type event inside the default can poison.
    pub(crate) fn equals_formal_type(&mut self) {
        let pending = self
            .members
            .equals_check
            .as_ref()
            .is_some_and(|check| check.depth == 1 && check.first_ty.is_none() && check.ty_seen);
        if !pending {
            return;
        }
        if let Some(check) = &mut self.members.equals_check {
            check.ty_seen = false;
        }
        match self.armed_ty {
            None => {}
            // `int Function(...)`, `(int, int)`, `void` heads and friends:
            // poison rather than leave a nested head's stale write behind.
            Some(ty @ (Ty::Named { .. } | Ty::Dynamic)) => {
                if let Some(check) = &mut self.members.equals_check {
                    check.first_ty = Some(ty);
                }
            }
            Some(_) => {
                if let Some(check) = &mut self.members.equals_check {
                    check.poisoned = true;
                }
            }
        }
    }

    /// The `==` adjudication at the method's own parameter-list close
    /// (the official order: arity first, then the `Object`/`dynamic`
    /// participant guard, then the nullability question — reported at the
    /// `==` name token, `diagnostic.g.dart` :13186-13187).
    pub(crate) fn equals_parameters_closed(&mut self, count: usize) {
        let Some(check) = self.members.equals_check.take() else {
            return;
        };
        if check.depth != 1 {
            self.members.equals_check = Some(check);
            return;
        }
        if count != 1 || check.poisoned {
            return;
        }
        let Some(Ty::Named { name, nullable, .. }) = check.first_ty else {
            // `dynamic`, an unwritten annotation (the resolved type is
            // `dynamic`, never nullable), or an unresolved head: silent.
            return;
        };
        if name == self.intern("Object") && nullable {
            self.stage_usage(
                SemanticCode::NonNullableEqualsParameter,
                check.name_range,
                "",
            );
        }
    }

    // —— #1314 FAM7: the optional-parameter default-value cluster ————————

    /// The written type is potentially non-nullable
    /// (`isPotentiallyNonNullable` = `!isNullable`,
    /// type_system.dart:1257/1216-1233): `?`-suffixed,
    /// `dynamic`/`void`/`Null`/`Unknown` (the `InvalidType` splice) are
    /// nullable; `FutureOr<T>` recurses its argument (a bare `FutureOr`
    /// reads `FutureOr<dynamic>` — nullable); a bare in-scope type
    /// parameter `T` is NOT — the declared element carries no
    /// `promotedBound` and `isNullable` never consults `bound`, so `T`
    /// answers non-nullable and `T?` answers nullable (:1223-1226).
    /// `Record`/`Function` heads decide on their own `?`; `Never` is the
    /// non-nullable bottom.
    pub(crate) fn written_potentially_non_nullable(&self, ty: Ty) -> bool {
        !self.written_nullable_pnn(ty, 0)
    }

    /// The name-level `isNullable` (type_system.dart:1216-1233) — the
    /// `written_potentially_non_nullable` negation over the declared-type
    /// lattice. `FutureOr`'s argument recursion is name-matched (a user
    /// `FutureOr` shadowing the dart:async spelling takes the generic arm —
    /// the under-report direction), depth-capped at the engine's
    /// normalization bound.
    fn written_nullable_pnn(&self, ty: Ty, depth: u32) -> bool {
        if depth > 16 {
            return false;
        }
        match ty {
            Ty::Dynamic | Ty::Void | Ty::Null | Ty::Unknown => true,
            Ty::Named {
                name, nullable, ..
            } if nullable
                || self.strings.resolve(name) != "FutureOr" =>
            {
                // An unresolvable head is the oracle's `InvalidType`
                // (`void f({A a})` over an undeclared `A` resolves to the
                // invalid face, which answers `isPotentiallyNullable` like
                // `dynamic` — no `missing_default_value_for_parameter`
                // row ever fires on it). Type parameters keep the written
                // answer (their own bound face, not a resolution miss);
                // every resolvable container/typedef keeps it too —
                // `resolve_container` covers classes, mixins, enums and
                // extension types, the namespace `Decl` probe covers SDK
                // faces and typedefs the index cannot project.
                if !nullable && !self.is_scope_param(name) && !self.type_head_resolves(name)
                {
                    return true;
                }
                nullable
            }
            Ty::Named {
                args: crate::model::ArgListId::EMPTY,
                ..
            } => {
                // `FutureOr` bare is `FutureOr<dynamic>` — nullable.
                true
            }
            Ty::Named { args, .. } => {
                let arg = {
                    let pool = self.boundary.pool();
                    pool.get(args).first().copied()
                };
                arg.is_none_or(|arg| self.written_nullable_pnn(arg, depth + 1))
            }
            Ty::Record { nullable, .. } | Ty::Function { nullable, .. } => nullable,
            Ty::Never => false,
        }
    }

    /// Whether `name` denotes a type in this file's scope — the
    /// `written_nullable_pnn` `InvalidType` gate's positive leg. A
    /// container (class/mixin/enum/extension type, local or imported) and
    /// any scope `Decl` the namespace answers (SDK faces like `int`, whose
    /// declaring unit carries no element model, and typedefs) resolve; an
    /// `Ambiguous`/`NotFound` name, a prefix, or a non-type decl the probe
    /// cannot classify keep the `InvalidType` face — the under-report
    /// direction (`true` = nullable above).
    fn type_head_resolves(&self, name: StringId) -> bool {
        if resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        )
        .is_some()
        {
            return true;
        }
        // The namespace probe catches what the container walk cannot: an
        // SDK declaration (its unit is never indexed — `index.get` misses)
        // and a type alias (`decl` carries `TypeAlias`, never a container
        // row). A `Decl` in an indexed unit resolves only when its own
        // kind is a type — a function or variable `A` used in type
        // position is the oracle's `InvalidType` even though the bare name
        // binds.
        match self.namespaces.lookup(self.graph, self.lib, name) {
            Lookup::Found(NamespaceEntry::Decl(owner)) => match self.index.get(owner) {
                Some(elements) => elements.decl(name).is_some_and(|decl| {
                    matches!(
                        decl.kind,
                        crate::model::MicroKind::Class
                            | crate::model::MicroKind::Enum
                            | crate::model::MicroKind::Mixin
                            | crate::model::MicroKind::Extension
                            | crate::model::MicroKind::TypeAlias
                    )
                }),
                // An unindexed owner is an SDK face — SDK names in type
                // position are type declarations.
                None => true,
            },
            _ => false,
        }
    }

    /// Stages the current optional formal for the body-verdict flush: only
    /// a potentially-non-nullable written type without a default value
    /// stages (error_verifier.dart:8394-8418); primary constructors decide
    /// immediately (`defaultValuesAreExpected` answers true unconditionally
    /// there, :8368-8370). An `augment` fragment's formal stages like any
    /// other: every fragment's formal list is checked against the merged
    /// element (upstream MissingDefaultValueForParameterTest.
    /// test_function_nonNullable_positional_optional_noDefault_augmentation:
    /// `void f([int a]);` + `augment void f([int a]) {}` reports at both
    /// `a`s); the renamed-slot and merged-type exceptions live in
    /// [`Self::chain_supplies_default`]. Without the augmentation merge
    /// (the feature off) no chain answers an `augment` fragment's formal —
    /// its merged type and slot belong to the introductory formal, which
    /// carries the row alone.
    pub(crate) fn stage_default_check(&mut self, check: PendingDefaultCheck) {
        if !self.augmentations_enabled
            && self.index.get(self.file).is_some_and(|elements| {
                elements.augmentations().iter().any(|aug| {
                    aug.formals
                        .iter()
                        .any(|formal| formal.name_range.offset == check.name_range.offset)
                })
            })
        {
            return;
        }
        if self.written_potentially_non_nullable(check.written) {
            self.pending_default_checks.push(check);
        }
    }

    /// Emits the staged optional-parameter rows per the body verdict
    /// (`defaultValuesAreExpected`, error_verifier.dart:8328-8381): a
    /// block/arrow body expects defaults for every executable face. An
    /// empty `;` body answers by the parent's arm — a
    /// `ConstructorDeclaration` expects them outright (generative heads
    /// and non-redirecting factories alike; `isFactory && isRedirecting`
    /// is the only ctor exclusion, and an `= G;` redirect never reaches
    /// the empty-body event, :8330-8343), the
    /// `FunctionExpression`/`TopLevelGetter` arms expect them too (local
    /// functions and anonymous faces join through the same arm,
    /// :8344-8356/:8368-8376), while a `;`-bodied `MethodDeclaration` is
    /// `isAbstract` and answers false (:8357-8367). `ctor_pending` is the
    /// live-window marker — `open_ctor_site` outlives its constructor
    /// (`end_constructor` never clears it), so a sibling `;`-method after
    /// a `;` ctor would false-positive off the stale site.
    pub(crate) fn flush_default_checks(&mut self, body_present: bool) {
        let ctor_open = self.ctor_pending.is_some();
        let checks = std::mem::take(&mut self.pending_default_checks);
        for check in checks {
            let expects = body_present
                || ctor_open
                || !matches!(
                    check.member_kind,
                    dartq_parser::MemberKind::NonStaticMethod
                        | dartq_parser::MemberKind::StaticMethod
                        | dartq_parser::MemberKind::ExtensionNonStaticMethod
                        | dartq_parser::MemberKind::ExtensionStaticMethod
                        | dartq_parser::MemberKind::ExtensionTypeNonStaticMethod
                        | dartq_parser::MemberKind::ExtensionTypeStaticMethod
                );
            if !expects {
                continue;
            }
            // Every arm still excludes an `external` header
            // (`defaultValuesAreExpected`'s augment/external/native
            // exclusions, error_verifier.dart:8337/8350/8362/8371) —
            // `external C([int x]);` takes defaults in its embedding,
            // never from Dart. The listener's own external bit is owned
            // elsewhere (#2286), so this face reads the header tokens
            // directly.
            if self.header_is_external(check.name_range) {
                continue;
            }
            let code = if check.named {
                SemanticCode::MissingDefaultValueForParameter
            } else {
                SemanticCode::MissingDefaultValueForParameterPositional
            };
            // The merged element answers `parameterElement.hasDefaultValue`
            // (error_verifier.dart:8385): a default written on ANY fragment
            // of the augmentation chain silences the row — including a
            // fragment the parse has not reached yet. With the feature on
            // the rows defer to [`Self::finalize_default_checks`]; with it
            // off `augment` never lexes, no chain exists, and the row emits
            // here (the oracle's plain-declaration arm).
            if self.augmentations_enabled {
                DEFERRED_DEFAULTS.with(|queue| {
                    queue.borrow_mut().push(DeferredDefault {
                        code,
                        range: check.name_range,
                        param: check.name,
                        named: check.named,
                    });
                });
            } else {
                let name = self.strings.resolve(check.name).to_string();
                self.stage_usage(code, check.name_range, &name);
            }
        }
    }

    /// The fold-tail verdict for the staged optional-parameter rows
    /// (upstream `parameterElement.hasDefaultValue` is the merged element's
    /// flag — one fragment's written default settles every fragment of the
    /// chain, error_verifier.dart `_checkUseOfDefaultValuesInParameters`).
    /// Runs once per unit inside [`Self::verify_enum_member_set`], after all
    /// `augment` declarations are captured.
    pub(crate) fn finalize_default_checks(&mut self) {
        let pending = DEFERRED_DEFAULTS.with(|queue| std::mem::take(&mut *queue.borrow_mut()));
        for item in pending {
            if !self.chain_supplies_default(&item) {
                let name = self.strings.resolve(item.param).to_string();
                self.stage_usage(item.code, item.range, &name);
            }
        }
    }

    /// Whether any fragment of the item's augmentation chain supplies a
    /// default for the same-named parameter — the `hasDefaultValue` merge —
    /// or the item is an augment formal renaming its merged slot (the
    /// introductory formal then owns the row).
    /// The chain key resolves like the augmentation verifier's
    /// (`augment_key`): a member augment keys `(container, name, static)`,
    /// a top-level executable keys `name`. Intro fragments read the model's
    /// own formal tables (`Formal::default_value`); same-file augment
    /// fragments detect the written `=`/`:` clause by token scan — a
    /// cross-file augment's default stays undetected (no token table rides
    /// the index), the documented under-coverage of this face.
    fn chain_supplies_default(&self, item: &DeferredDefault) -> bool {
        let own = self.index.get(self.file);
        // The owner key: an `augment` formal matches by its name-token span;
        // an introductory formal resolves to the nearest preceding
        // executable declaration site. A positional item also records its
        // position among the fragment's positional parameters — the chain
        // merges positional parameters by POSITION (the merged element's
        // formal table pairs fragments slot-by-slot), so a renamed
        // positional augment still supplies the intro parameter's default.
        let mut owner: Option<(Option<StringId>, StringId, bool, bool)> = None;
        let mut item_pos: Option<usize> = None;
        let mut from_aug = false;
        if let Some(elements) = own {
            'aug: for aug in elements.augmentations() {
                if !matches!(
                    aug.kind,
                    crate::model::AugKind::Function
                        | crate::model::AugKind::Method
                        | crate::model::AugKind::Getter
                        | crate::model::AugKind::Setter
                        | crate::model::AugKind::Constructor
                ) {
                    continue;
                }
                for (at, formal) in aug.formals.iter().enumerate() {
                    if formal.name_range.offset == item.range.offset {
                        owner = Some((
                            aug.container,
                            aug.name,
                            aug.is_static,
                            aug.kind == crate::model::AugKind::Constructor,
                        ));
                        from_aug = true;
                        if item.named {
                            // A wildcard `_` named formal merges by position
                            // among the NAMED parameters (the `_`-or-same-
                            // name rule); record its named index.
                            if self.strings.resolve(item.param) == "_" {
                                item_pos = Some(
                                    aug.formals[..at]
                                        .iter()
                                        .filter(|f| f.named)
                                        .count(),
                                );
                            }
                        } else {
                            item_pos = Some(
                                aug.formals[..at]
                                    .iter()
                                    .filter(|f| f.required_positional || f.optional_positional)
                                    .count(),
                            );
                        }
                        break 'aug;
                    }
                }
            }
        }
        if owner.is_none() {
            for site in self.decl_sites.iter() {
                if site.range.offset >= item.range.offset {
                    continue;
                }
                owner = match site.kind {
                    DeclKind::Member {
                        kind,
                        container,
                        is_static,
                    } if kind != MemberKind::Field => {
                        Some((Some(container), site.name, is_static, false))
                    }
                    DeclKind::TopLevelFn => Some((None, site.name, false, false)),
                    DeclKind::Constructor { container } => {
                        Some((Some(container), site.name, false, true))
                    }
                    _ => owner,
                };
            }
        }
        let Some((container, name, is_static, ctor)) = owner else {
            return false;
        };
        let units = self.graph.unit_files(self.lib);
        let own_pos = units
            .iter()
            .position(|&unit| unit == self.file)
            .unwrap_or(units.len());
        for (position, &unit) in units.iter().enumerate() {
            if position > own_pos {
                break;
            }
            let Some(elements) = self.index.get(unit) else {
                continue;
            };
            // The introductory fragment's formal table (model-side
            // `default_value` carries the written clause).
            let intro_formals = match (container, ctor) {
                (Some(owner_name), true) => elements
                    .container(owner_name)
                    .and_then(|found| {
                        found
                            .ctor_formals
                            .get(&name)
                            .or_else(|| found.factory_formals.get(&name))
                            .map(|formals| formals.as_ref())
                    }),
                (Some(owner_name), false) => elements
                    .container(owner_name)
                    .and_then(|found| found.members.get(&name))
                    .and_then(|entry| entry.formals.as_deref()),
                (None, _) => elements
                    .decl(name)
                    .and_then(|decl| decl.formals.as_deref()),
            };
            if let Some(formals) = intro_formals {
                // Positional parameters merge by position: the deferring
                // fragment's own index decides — an aug item reads its
                // captured `item_pos`, an intro item its own table slot,
                // latched here so the augment-fragment matching below
                // compares the same slot across fragments.
                if item_pos.is_none() {
                    if !item.named {
                        item_pos = formals
                            .positional
                            .iter()
                            .position(|formal| formal.name == item.param);
                    } else if self.strings.resolve(item.param) == "_" {
                        // An intro-deferred wildcard `_` carries its own
                        // index among the named parameters — the same slot
                        // the augment-fragment matching compares below.
                        item_pos = formals
                            .named
                            .iter()
                            .position(|formal| formal.name == item.param);
                    }
                }
                // An augment formal that renames its merged slot (`_` or a
                // fresh name over the introductory `x`) leaves the row to
                // the introductory formal: `void f([int x])` + `augment
                // void f([int _])` reports once, at `x` (the #3625 oracle
                // observation). A same-named augment formal reports on its
                // own fragment — against the MERGED element's type, which is
                // the introductory formal's (`augment void f({int a})` over
                // `void f({int? a})` asks `int?`: silent).
                if from_aug {
                    let intro = match item_pos {
                        Some(at) if item.named => formals.named.get(at),
                        Some(at) => formals.positional.get(at),
                        None if item.named => {
                            formals.named.iter().find(|formal| formal.name == item.param)
                        }
                        None => None,
                    };
                    if intro.is_some_and(|formal| {
                        formal.name != item.param
                            || !self.written_potentially_non_nullable(formal.ty)
                    }) {
                        return true;
                    }
                }
                let supplied = if item.named {
                    match item_pos {
                        // A wildcard `_` aug item carries its named index —
                        // the merged named slot at that index supplies.
                        Some(at) => formals
                            .named
                            .get(at)
                            .is_some_and(|formal| formal.default_value.is_some()),
                        None => formals
                            .named
                            .iter()
                            .any(|formal| formal.name == item.param && formal.default_value.is_some()),
                    }
                } else {
                    let at = item_pos.or_else(|| {
                        formals
                            .positional
                            .iter()
                            .position(|formal| formal.name == item.param)
                    });
                    at.is_some_and(|at| {
                        formals
                            .positional
                            .get(at)
                            .is_some_and(|formal| formal.default_value.is_some())
                    })
                };
                if supplied {
                    return true;
                }
            }
            for aug in elements.augmentations() {
                if !matches!(
                    aug.kind,
                    crate::model::AugKind::Function
                        | crate::model::AugKind::Method
                        | crate::model::AugKind::Getter
                        | crate::model::AugKind::Setter
                        | crate::model::AugKind::Constructor
                ) || aug.container != container
                    || aug.name != name
                    || (aug.kind == crate::model::AugKind::Constructor) != ctor
                    || (!ctor && aug.is_static != is_static)
                {
                    continue;
                }
                // The fragment's own `default_separator` marks the written
                // `=`/`:` clause (upstream
                // `formalParameter.defaultClause.separator`) — a cross-file
                // augment's default reads it too; the token scan stays as
                // the same-file fallback for rows captured before the
                // separator latch. Positional parameters merge by
                // position, named by name.
                let supplied = aug.formals.iter().enumerate().any(|(at, formal)| {
                    let slot_matches = if item.named {
                        match item_pos {
                            // The wildcard `_` item's named index merges
                            // this fragment's named formal at the same
                            // index.
                            Some(item_pos) => {
                                let pos = aug.formals[..at]
                                    .iter()
                                    .filter(|f| f.named)
                                    .count();
                                formal.named && pos == item_pos
                            }
                            None => formal.name == Some(item.param),
                        }
                    } else {
                        // An aug-deferred positional matches its captured
                        // slot; an intro-deferred one (position resolved
                        // only against the introductory table, not this
                        // fragment's) falls back to the name match — the
                        // position it occupies in THIS fragment is the same
                        // slot only when names agree anyway.
                        match item_pos {
                            Some(item_pos) => {
                                let pos = aug.formals[..at]
                                    .iter()
                                    .filter(|f| {
                                        f.required_positional || f.optional_positional
                                    })
                                    .count();
                                pos == item_pos
                            }
                            None => formal.name == Some(item.param),
                        }
                    };
                    slot_matches
                        && (formal.default_separator.is_some()
                            || (unit == self.file
                                && self.augment_formal_has_default(formal.name_range)))
                });
                if supplied {
                    return true;
                }
            }
        }
        false
    }

    /// Whether the augment-fragment formal whose name token sits at
    /// `name_range` carries a default clause: tokens between the name and
    /// the parameter's closing delimiter at paren depth 0, `=` (new style)
    /// or `:` (legacy style) mark it.
    fn augment_formal_has_default(&self, name_range: SourceRange) -> bool {
        if name_range.offset == u32::MAX {
            return false;
        }
        let tokens = self.tokens.as_slice();
        let mut at = tokens
            .partition_point(|token| token.utf16_offset < name_range.offset + name_range.length);
        let mut depth: i32 = 0;
        while let Some(token) = tokens.get(at) {
            let text = &self.source[token.byte_range()];
            match text {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                }
                "," | ";" if depth == 0 => return false,
                "=" | ":" if depth == 0 => return true,
                _ => {}
            }
            at += 1;
        }
        false
    }

    /// Whether the executable header holding the formal at `name_range` was
    /// written `external`: walk back from the formal's name token to the
    /// header's `;`/`{`/`}` boundary (the previous member's terminator) and
    /// look for the `external` modifier. Comments never intervene (the
    /// scanner drops them), and the boundary stops the walk inside the
    /// previous member, so the scan cannot misread one. A 512-token cap
    /// bounds a pathological unclosed header; the first declaration in a
    /// file walks to index 0.
    ///
    /// Braces need care: the named-parameter group's own `{` sits INSIDE
    /// the formal list (`C({int a})`), and a default value's map/set/record
    /// literal adds more pairs (`{int a = const {1: 2}}`). A header-level
    /// brace — the class body's `{`, a previous member's `}` — always
    /// follows a `)`, an identifier, or another brace backward; an
    /// in-formals brace always follows `(`, `,`, `=`/`:`/`const` (a
    /// default literal's lead-in). Only the header-level kind ends the
    /// scan.
    fn header_is_external(&self, name_range: SourceRange) -> bool {
        let tokens = self.tokens.as_slice();
        let at = tokens.partition_point(|token| token.utf16_offset < name_range.offset);
        let mut index = match at.checked_sub(1) {
            Some(i) => i,
            None => return false,
        };
        for _ in 0..512 {
            match self.tokens.lexeme(self.source, index) {
                ";" => return false,
                "external" => return true,
                "{" | "}" => {
                    // For `}` skip the balanced group backward to its `{`
                    // (a default literal's braces ride the same
                    // transparent path as the named group's own).
                    let mut brace = index;
                    if self.tokens.lexeme(self.source, index) == "}" {
                        let mut depth = 0i32;
                        loop {
                            match self.tokens.lexeme(self.source, brace) {
                                "}" => depth += 1,
                                "{" => depth -= 1,
                                _ => {}
                            }
                            if depth == 0 || brace == 0 {
                                break;
                            }
                            brace -= 1;
                        }
                        if depth != 0 {
                            return false;
                        }
                    }
                    let before = brace
                        .checked_sub(1)
                        .map(|i| self.tokens.lexeme(self.source, i));
                    match before {
                        Some("(") | Some(",") | Some("=") | Some(":") | Some("{")
                        | Some("const") => {
                            index = brace - 1;
                            continue;
                        }
                        _ => return false,
                    }
                }
                _ => {}
            }
            if index == 0 {
                return false;
            }
            index -= 1;
        }
        false
    }

    /// Drops the staged rows (the native/skipped-body arms of
    /// `defaultValuesAreExpected`). Also drains the wildcard-`_` duplicate
    /// log — every executable end event reaches here, so a unit whose only
    /// wildcard rows sit inside a body still emits them before the walk
    /// ends.
    pub(crate) fn drop_default_checks(&mut self) {
        self.pending_default_checks.clear();
        self.emit_wildcard_dups();
    }

    /// Breaks the `noSuchMethod` shape: any structural expression event
    /// outside the argument window that is not part of
    /// `super . noSuchMethod (` kills it (the official matches ONE
    /// `ReceiverMethodInvocation` node — compound expressions never
    /// match). Poison applies in the expression face and inside a return
    /// statement's expression; argument content is opaque.
    pub(crate) fn nsm_poison(&mut self) {
        self.members.nsm_poison();
    }

    /// The shape machine's `super` transition.
    pub(crate) fn nsm_super(&mut self) {
        self.members.nsm_super();
    }

    /// The selector's member name at the identifier event (`super` face
    /// only — the name precedes the send): latches whether the dot event
    /// will validate.
    pub(crate) fn nsm_member_name(&mut self, token: Token) {
        let hits = self.text_of(token) == Some("noSuchMethod");
        self.members.nsm_member_name(hits);
    }

    /// `handle_type` completed an annotation inside the method's own
    /// parameter list: the pending first-formal read may consume it.
    pub(crate) fn equals_mark_type(&mut self) {
        self.members.equals_mark_type();
    }

    /// The shape machine's `.` validation (`end` is the member name
    /// token). The parser fires the selector's dot AFTER its send for the
    /// `super.m(x)` face, so the name check validates the closed send;
    /// argument content stays opaque. The null-aware spelling is never
    /// the shape.
    pub(crate) fn nsm_dot(&mut self, is_null_aware: bool) {
        self.members.nsm_dot(is_null_aware);
    }

    /// The shape machine's argument-list transitions.
    pub(crate) fn nsm_arguments_open(&mut self) {
        self.members.nsm_arguments_open();
    }

    /// The shape machine's argument-list close: only the outermost list's
    /// count decides (exactly one argument).
    pub(crate) fn nsm_arguments_closed(&mut self, count: usize) {
        self.members.nsm_arguments_closed(count);
    }

    /// A return statement inside the method's own body block: resets the
    /// machine for the statement's expression, marks the window (the
    /// block face's poison gate), and latches the candidate's `return`
    /// keyword for the block-face statement-kind gate. Outside a block
    /// body a return statement cannot be part of the shape
    /// (`=> () { return ...; }()` closures).
    pub(crate) fn nsm_return_open(&mut self, token: Token) {
        self.members.nsm_return_open(token.range.offset);
    }

    /// A return statement's close: the block face latches its candidate —
    /// the FIRST return of the body block only (later returns are other
    /// statements' business; the statement-count gate rules them out).
    pub(crate) fn nsm_return_closed(&mut self, has_expression: bool, end: Token) {
        self.members
            .nsm_return_closed(has_expression, end.range.end());
    }

    /// The method's own block body opened (`begin_block_function_body`):
    /// the first body-shaped block latches; nested blocks and closure
    /// bodies (in the defaults window, before arming) never do.
    pub(crate) fn nsm_block_open(&mut self, begin: Token) {
        self.members.nsm_block_open(begin.range);
    }

    /// The block-body adjudication at the method's own block's close: the
    /// block held exactly one statement and that statement was the
    /// shape-completing return — proven by the statement-kind gate below,
    /// which the bare statement count cannot give (`if (c) return
    /// super.noSuchMethod(i);` also counts 1).
    pub(crate) fn nsm_block_closed(&mut self, count: usize, begin: Token, end: Token) {
        let Some(check) = self.members.nsm_check.take() else {
            return;
        };
        if check.body_block != Some(begin.range) {
            // A nested block's close: keep the latch for the body's own.
            self.members.nsm_check = Some(check);
            return;
        }
        if count == 1 && check.return_ok && self.block_interior_is_the_candidate(begin, end, &check)
        {
            self.emit_unnecessary_no_such_method(check.name_range);
        }
    }

    /// The block-face statement-kind gate: the braces' token interior is
    /// exactly the candidate return — the first token after `{` starts at
    /// the latched `return` keyword and the last token before `}` ends at
    /// the latched `;`. Comments produce no tokens (the scanner stream
    /// contract), so both ends are comment-proof; `{ return X; }` passes,
    /// `{ if (c) return X; }` fails at `if`.
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    pub(crate) fn block_interior_is_the_candidate(
        &self,
        begin: Token,
        end: Token,
        check: &NsmCheck,
    ) -> bool {
        let (Some(kw), Some(semi_end)) = (check.return_kw, check.return_semi_end) else {
            return false;
        };
        let mut first: Option<(u32, u32)> = None;
        let mut last = (0u32, 0u32);
        let mut index = begin.id as usize + 1;
        while let Some(token) = self.tokens.get(index) {
            let span = token.byte_range();
            if span.start as u32 >= end.range.offset {
                break;
            }
            let ends = (span.start as u32, span.end as u32);
            if first.is_none() {
                first = Some(ends);
            }
            last = ends;
            index += 1;
        }
        first == Some((kw, kw + "return".len() as u32)) && last.1 == semi_end
    }

    /// The expression-body adjudication at the arrow body's close: the
    /// body expression was exactly the shape. Closure bodies inside the
    /// method's own block never land here (the block latched first);
    /// defaults-window closures predate the arm.
    pub(crate) fn nsm_expression_closed(&mut self) {
        if let Some(range) = self
            .members
            .nsm_check
            .as_ref()
            .filter(|check| {
                check.armed && check.body_block.is_none() && check.shape == NsmShape::Done
            })
            .map(|check| check.name_range)
        {
            self.emit_unnecessary_no_such_method(range);
        }
    }

    /// The row emission: verbatim WARNING-tier template at the
    /// declaration's name token (`diag.unnecessaryNoSuchMethod.at(node.name)`).
    /// The superchain decides: a concrete `noSuchMethod` declared by a
    /// nominal class in the super-dispatch chain reports; the `Object`
    /// default, exhaustion, or a poisoned resolution stays silent.
    pub(crate) fn emit_unnecessary_no_such_method(&mut self, name_range: SourceRange) {
        if self.superchain_resolves_no_such_method() == Some(true) {
            self.stage_usage(SemanticCode::UnnecessaryNoSuchMethod, name_range, "");
        }
    }

    /// Opens the shape machine to body events (the method's own formals
    /// closed — a getter's absent list counts through
    /// `handle_no_formal_parameters`).
    pub(crate) fn arm_no_such_method(&mut self) {
        self.members.arm_no_such_method();
    }

    /// The `==` first-formal type question is unanswerable past a
    /// function/record type shape (the written heads can never read as a
    /// plain `Object?` annotation; the official's resolved type is not
    /// `dartCoreObject` there either).
    pub(crate) fn equals_type_poison(&mut self) {
        self.members.equals_type_poison();
    }

    /// The send-shaped poison for argless sends: an invocation-completed
    /// shape is the send's own business (`Args`/`Done` stay), a send where
    /// a shape part was expected is broken.
    pub(crate) fn nsm_send_poison(&mut self) {
        self.members.nsm_send_poison();
    }

    /// The 3.13.1 oracle's concrete super-dispatch for `noSuchMethod`
    /// (the `_checkForUnnecessaryNoSuchMethod` resolved-element face,
    /// probed): the walk covers the nominal `extends` chain — mixin
    /// applications are walked THROUGH in application order, and a
    /// CONCRETE mixin declaration of the name poisons the resolution
    /// (probed silent for `class C extends B with M` with a concrete
    /// `M.noSuchMethod`, even when `B` also declares one), while abstract
    /// declarations anywhere are transparent (probed: the walk continues
    /// past an abstract superclass and an abstract mixin declaration).
    /// The first CONCRETE nominal declaration reports; exhaustion (the
    /// `Object` default) or an unresolvable chain stays silent. Names
    /// resolve through each hop's owner library (the #491 S2a rule the
    /// face walks share); only indexed containers are visible — ancestors
    /// from unread files under-report.
    pub(crate) fn superchain_resolves_no_such_method(&self) -> Option<bool> {
        let nsm = self.intern("noSuchMethod");
        let elements = self.index.get(self.file)?;
        let (container, _) = self.container_context()?;
        let decl = elements.container(container)?;
        // The delegating class's own `with` mixins sit closest in the
        // applied chain (super resolves against the first application).
        for mixin in decl.with_clause.iter().rev() {
            if let Some((mixin_decl, _)) = resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                *mixin,
            ) && let Some(entry) = mixin_decl.members.get(&nsm)
                && !entry.is_static
                && !entry.is_abstract
            {
                return Some(false);
            }
        }
        let Some(mut name) = decl.superclass else {
            return Some(false);
        };
        let mut owner = self.file;
        let mut scope_lib = self.lib;
        let mut seen = FxHashSet::default();
        loop {
            if !seen.insert(name) {
                return None;
            }
            let (found, found_owner) = resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                scope_lib,
                owner,
                name,
            )?;
            // An abstract declaration is transparent: the concrete
            // dispatch continues past it.
            if let Some(entry) = found.members.get(&nsm)
                && !entry.is_static
                && !entry.is_abstract
            {
                return Some(true);
            }
            let owner_lib = self
                .owner_library_of(found_owner, name)
                .unwrap_or(scope_lib);
            for mixin in found.with_clause.iter().rev() {
                if let Some((mixin_decl, _)) = resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    owner_lib,
                    found_owner,
                    *mixin,
                ) && let Some(entry) = mixin_decl.members.get(&nsm)
                    && !entry.is_static
                    && !entry.is_abstract
                {
                    return Some(false);
                }
            }
            match found.superclass {
                Some(superclass) => {
                    scope_lib = owner_lib;
                    name = superclass;
                    owner = found_owner;
                }
                None => return Some(false),
            }
        }
    }

    // —— class-modifier restriction families (#600) ——————————————————————

    /// Arms the deferred class-modifier restriction checks at the
    /// container's name event: the element model is complete (the collect
    /// pass ran first), but the supertype entries have not streamed yet —
    /// the outside-library rows anchor their written entries (#1289 E1), so
    /// the emissions wait for the clause spans. `decl_start` is the
    /// declaration's first non-metadata token (the parser's `begin` span) —
    /// the `@sealed` contract's anchor start, the oracle's
    /// `firstTokenAfterCommentAndMetadata` face. A doc comment written
    /// directly ahead of the declaration joins the oracle's node offset but
    /// not this span — the one known residual of the anchor. `super_open`
    /// mirrors the collector's extends-slot gate (`true` for the class
    /// family and named mixin applications, `false` for mixins).
    pub(crate) fn arm_modifier_checks(
        &mut self,
        decl_start: SourceRange,
        super_open: bool,
        name: Option<StringId>,
        name_range: SourceRange,
    ) {
        self.deferred_modifier = Some(DeferredModifierChecks {
            container: name,
            name_range,
            decl_start,
            use_sites_done: false,
            spans: HeaderClauseSpans {
                super_open,
                ..HeaderClauseSpans::default()
            },
        });
        // The ordering recovery's tail key: a fresh header has seen no
        // implements clause yet.
        self.header_implements_seen = false;
    }

    /// One type completed inside the open header window: a clause entry at
    /// depth 0 (the parser re-fires a parameterized head after its own
    /// type-argument group, so `G<int>?` completes once, here). The entry
    /// span runs name through the trailing `?` or the just-closed group's
    /// closer — the oracle's whole-NamedType anchor. A prefixed spelling
    /// (`p.C`) completes on the PREFIX token (the official
    /// `PrefixedType.parseType` protocol), so its span is widened across
    /// the `.`-continuation to the local segment's end
    /// ([`Self::prefixed_head_end`]): the oracle anchors prefix head
    /// through local end (`resolution_visitor.dart:1235-1237`), and the
    /// downstream faces read the span's TEXT (the `with`-clause teardown
    /// resolves a dotted entry through the prefix table,
    /// [`Engine::finalize_mixin_uses`]).
    pub(crate) fn note_header_type_span(&mut self, name: Token, question: Option<Token>) {
        // #1444: the extension-type header's clause-entry ledger (the
        // modifier window below stays unarmed for extension types; the
        // global type-argument depth excludes argument positions).
        if self.extension_clause_phase == ExtensionClausePhase::Clauses && self.type_arg_depth == 0
        {
            self.extension_clause_miss.push(name.range.offset);
        }
        // Read before the deferred window is borrowed mutably below.
        let prefixed_end = self.prefixed_head_end(name.id);
        let Some(deferred) = self.deferred_modifier.as_mut() else {
            return;
        };
        let spans = &mut deferred.spans;
        if spans.arg_depth > 0 {
            return;
        }
        let mut range = name.range;
        if let Some(end) = prefixed_end {
            range.length = end.saturating_sub(range.offset);
        }
        // The closer latch is consumed by every depth-0 completion (the
        // re-fire is the contractual next event); a trailing `?` wins over
        // the group closer, and a stale latch — possible only ahead of a
        // later entry — ends before that entry's name, so the extension
        // guard drops it.
        let closer = spans.args_closer.take();
        if let Some(question) = question {
            range.length = question.range.end() - range.offset;
        } else if let Some(closer) = closer
            && closer.end() > range.end()
        {
            range.length = closer.end() - range.offset;
        }
        if spans.super_open {
            if spans.super_candidate.is_none() {
                spans.super_candidate = Some(range);
            } else {
                spans.buffered.push(range);
            }
        } else {
            spans.buffered.push(range);
        }
    }

    /// The UTF-16 end offset of the local segment when `id` opens a
    /// group-less prefixed head (`prefix.Local`); `None` for every other
    /// token — a bare name, a parser-inserted handle (absent from the token
    /// table), or a spelling whose continuation is not the adjacent
    /// `.` + identifier pair (the scanner drops trivia, so adjacent tokens
    /// are the written segments). A miss leaves the recorded span alone —
    /// the under-report direction.
    fn prefixed_head_end(&self, id: u32) -> Option<u32> {
        let dot = self.tokens.get(id.checked_add(1)? as usize)?;
        if &self.source[dot.byte_range()] != "." {
            return None;
        }
        let leaf = self.tokens.get(id.checked_add(2)? as usize)?;
        let text = &self.source[leaf.byte_range()];
        let length = u32::try_from(text.encode_utf16().count()).ok()?;
        Some(leaf.utf16_offset + length)
    }

    /// A type-argument group opened inside the header window: entries nest
    /// one level deeper (`G<int>`'s `int` is an argument position).
    pub(crate) fn note_header_type_args_open(&mut self) {
        if let Some(deferred) = self.deferred_modifier.as_mut() {
            deferred.spans.arg_depth += 1;
        }
    }

    /// The extends clause completed (the class head and the named mixin
    /// application's head alike — the collector commits both through this
    /// event pair): the latched first entry is the superclass face.
    /// #1314 FAM14: the head joins the miss ledger as `Extends`; a later
    /// `with` clause upgrades the tag to `AliasSuper` — the oracle's clause
    /// switch codes a class-declaration extends miss `mixin_with_non_class_
    /// superclass` when `declaration.withClause != null` (upstream Dart SDK
    /// source, pkg/analyzer/lib/src/dart/resolver/scope_analyzer.dart:
    /// 1216-1221). The tag must therefore stay provisional until the header
    /// knows whether a with clause exists.
    pub(crate) fn header_extends_commit(&mut self) {
        let committed = if let Some(deferred) = self.deferred_modifier.as_mut() {
            let spans = &mut deferred.spans;
            spans.extends = spans.super_candidate.take();
            spans.super_open = false;
            spans.extends.map(|span| span.offset)
        } else {
            None
        };
        if let Some(offset) = committed {
            self.push_clause_miss(offset, ClauseTag::Extends);
        }
    }

    /// The implements clause completed: its trailing `count` entries join
    /// the implements face (unclaimed earlier entries — `with` faces,
    /// extends overflow, recovery residue — stay buffered, never claimed).
    pub(crate) fn header_implements_claim(&mut self, count: usize) {
        if let Some(deferred) = self.deferred_modifier.as_mut() {
            claim_trailing(
                &mut deferred.spans.implements,
                &mut deferred.spans.buffered,
                count,
            );
        }
        // The enum header's clause check drains here: the enum arm of
        // `arm_modifier_checks` latches `container: None` and the enum
        // body-open event never calls [`Self::run_modifier_use_sites`]
        // (its `deferred_modifier` is discarded outright at the enum end
        // event), so `enum E with M implements I` would miss the
        // resolved-element half of the oracle's `_resolveType` clause
        // check entirely (upstream Dart SDK source,
        // `pkg/analyzer/lib/src/dart/resolver/scope_context.dart:144-167`
        // — `EnumDeclaration` visits `withClause` and `implementsClause`
        // through the same `_resolveType` walk). `implements` is the enum
        // grammar's LAST clause — its close (or absent-clause `count` = 0)
        // fires when the header is complete.
        let enum_container = match self.container_context() {
            Some((name, ContainerKind::Enum)) => Some(name),
            _ => None,
        };
        if let Some(container) = enum_container {
            let window = self
                .deferred_modifier
                .as_ref()
                .map(|deferred| Self::clause_window_offsets(&deferred.spans))
                .unwrap_or_default();
            // The outside-library modifier families drive enums too: the
            // oracle's `visitEnumDeclaration` runs `_checkClassInheritance`
            // with a null superclass and the enum's `with`/`implements`
            // clauses whenever either exists (upstream Dart SDK source,
            // `pkg/analyzer/lib/src/generated/error_verifier.dart:823-863`),
            // so `enum E implements BaseClass` owes
            // `base_class_implemented_outside_of_library` at the written
            // entry exactly like a class does (the co19 Class-modifiers
            // suite's `test_enum_outside` family). The deferred slot is
            // taken for the call and restored, so the later `@sealed` drain
            // still reads it.
            let deferred = self.deferred_modifier.take();
            if let Some(deferred) = deferred.as_ref() {
                self.check_modifier_use_sites(container, deferred.name_range, &deferred.spans);
            }
            // The declaration identity for the clause map: the armed
            // deferred's name span, or the enum's own decl-site record when
            // the header never armed (recovery paths).
            let name_offset = deferred
                .as_ref()
                .map(|d| d.name_range.offset)
                .or_else(|| self.enum_decl_site.map(|site| site.0.offset));
            self.deferred_modifier = deferred;
            if let Some(name_offset) = name_offset {
                self.check_clause_element_kinds(container, name_offset, &window);
            }
        }
    }

    /// The whole clause-window's captured type spans: every offset the
    /// header's `note_header_type_span` latch recorded — extends head,
    /// buffered with/recovery entries, and the claimed
    /// `implements`/`on` tails. [`Self::check_clause_element_kinds`] sweeps
    /// the `clause_miss` ledger against this set: an offset here that no
    /// scanned [`SuperEntry`] owns is a parser-recovery artifact (the
    /// `parse_type_not_void` re-fire that pushes a `Missing` row at the
    /// `{`/`,` position after a `void` head — the oracle's own clause
    /// switch anchors nothing there), so the ledger entry and the row both
    /// silence.
    fn clause_window_offsets(spans: &HeaderClauseSpans) -> Vec<u32> {
        spans
            .extends
            .iter()
            .map(|span| span.offset)
            .chain(spans.super_candidate.iter().map(|span| span.offset))
            .chain(spans.buffered.iter().map(|span| span.offset))
            .chain(spans.implements.iter().map(|span| span.offset))
            .chain(spans.on.iter().map(|span| span.offset))
            .collect()
    }

    /// The mixin `on` clause completed: the same trailing-count claim as
    /// [`Self::header_implements_claim`], into the constraint face.
    pub(crate) fn header_on_claim(&mut self, count: usize) {
        if let Some(deferred) = self.deferred_modifier.as_mut() {
            claim_trailing(&mut deferred.spans.on, &mut deferred.spans.buffered, count);
        }
    }

    /// The use-site half of the deferred checks: the clause spans are
    /// complete — normally drained at the body open, with the declaration's
    /// end event as the no-body/recovery safety net (the flag keeps the
    /// double call single-shot).
    pub(crate) fn run_modifier_use_sites(&mut self) {
        let Some(mut deferred) = self.deferred_modifier.take() else {
            return;
        };
        if !deferred.use_sites_done {
            deferred.use_sites_done = true;
            // The clause-element-kind pass runs for EVERY container family
            // whose header scanned faithfully — the modifier use-site half
            // skips unnamed enums (`deferred.container` is `None` for them,
            // listener.rs's enum arm), but the oracle's clause switch keys
            // on the CLAUSE kind alone (upstream Dart SDK source,
            // `pkg/analyzer/lib/src/dart/resolver/resolution_visitor.dart`
            // `_resolveType` :1209-1227 — `EnumDeclaration` reaches the same
            // `with`/`implements` `_resolveType` walk through
            // `scope_context.dart:144-167`), so the enum's container context
            // supplies the name the deferred slot never latched. A mixin
            // application's `container` IS set (named mixin application
            // `class C = S with M`) and drains through the same path.
            let clause_container = deferred.container.or_else(|| {
                self.container_context()
                    .map(|(name, _)| name)
            });
            if let Some(container) = clause_container {
                if deferred.container.is_some() {
                    self.check_modifier_use_sites(
                        container,
                        deferred.name_range,
                        &deferred.spans,
                    );
                }
                // An enum header drains at its own
                // `handle_implements`/`handle_no_implements` event —
                // `use_sites_done` still single-shots it here for the
                // no-implements path (`enum E;`, `enum E {}`).
                if deferred.container.is_none()
                    && matches!(
                        self.index
                            .get(self.file)
                            .and_then(|e| e.container(container))
                            .map(|c| c.kind),
                        Some(ContainerKind::Enum)
                    )
                {
                    // Already drained at the implements claim.
                } else {
                    let window = Self::clause_window_offsets(&deferred.spans);
                    self.check_clause_element_kinds(container, deferred.name_range.offset, &window);
                }
            }
        }
        self.deferred_modifier = Some(deferred);
        self.emit_wildcard_dups();
    }

    /// The `@sealed` half, drained at the declaration's end event: the
    /// official anchor is the whole declaring node (`_checkForInvalidSealedSuperclass`
    /// reports `.at(node)`), whose close is first known here. `end` is the
    /// declaration's closing token (`}`/`;`); the recovery paths that
    /// complete the declaration without a body close fall back to the end
    /// event's token start.
    /// The wildcard-`_` duplicate rows, drained in source order behind
    /// [`Engine::wildcard_dups_emitted`]: a pre-feature unit reports each
    /// second `_` declaration as `duplicate_definition`; the feature's
    /// units emit nothing (upstream `isWildcardVariable` — `_` declares
    /// no name). Called from the drain hooks so the whole log emits
    /// exactly once per unit, at whichever boundary the walk reaches
    /// first (the cursor makes later calls no-ops).
    pub(crate) fn emit_wildcard_dups(&mut self) {
        if self.is_wildcard(self.intern("_")) {
            self.wildcard_dups_emitted = usize::MAX;
            return;
        }
        let Some(elements) = self.index.get(self.file) else {
            return;
        };
        let rows: Vec<SourceRange> = elements
            .wildcard_duplicates()
            .iter()
            .skip(self.wildcard_dups_emitted)
            .map(|dup| dup.range)
            .collect();
        self.wildcard_dups_emitted = usize::MAX;
        for range in rows {
            self.emit(SemanticCode::DuplicateDefinition, range, "_", "");
        }
    }

    /// Whether the member named at `name_range` counts as an augmentation
    /// upstream — `augment` written as the member's FIRST modifier. The
    /// fasta member walk (`parser_impl.dart`'s class-member modifier
    /// chain) binds `augmentKeyword` only in the leading slot: an
    /// `augment` consumed later (`external augment m();`, `static augment
    /// m();`) reports `conflicting_modifiers`/`modifier_out_of_order` yet
    /// the AST's `augmentKeyword` stays null, so the member is a plain
    /// declaration — it claims its scope exactly like an unmodified one
    /// (the parser-fe `member_errors` matrix proves the asymmetry: the
    /// `external augment` members claim while the `augment`-leading ones
    /// report `augmentation_without_declaration` and claim nothing). The
    /// member's header window is the token run after the previous
    /// member's terminator — `;`, or a depth-zero `{`/`}` (the prior
    /// declaration's end); an initializer's `,` chain keeps scanning
    /// through brackets (`var a = f(x), augment b;` cannot occur, but a
    /// group member's name sits mid-declaration, so the walk must not
    /// stop inside the group's own balanced spans).
    fn member_augment_is_leading(&self, name_range: SourceRange) -> bool {
        if name_range.offset == u32::MAX {
            return true;
        }
        let tokens = self.tokens.as_slice();
        let at = tokens.partition_point(|token| token.utf16_offset < name_range.offset);
        let mut index = at;
        let mut depth = 0i32;
        while index > 0 {
            match self.tokens.lexeme(self.source, index - 1) {
                // The previous member's terminator at top level — `;`, or
                // a `}` (its body close) / `{` (the container's own body
                // open). A `)`/`]`/`}` inside a balanced span is no
                // boundary: `augment int Function() f;` must not stop the
                // walk at the function-type's own `)`.
                ";" | "{" | "}" if depth == 0 => break,
                ")" | "]" | "}" => depth += 1,
                "(" | "[" | "{" => depth -= 1,
                _ => {}
            }
            index -= 1;
        }
        matches!(tokens.get(index), Some(token) if &self.source[token.byte_range()] == "augment")
    }

    /// Whether `range` lands on this file's token stream and its token's
    /// text reads as `name` (the `name=` setter spelling counts — the
    /// member map's key keeps it). A merged-face member contributed by
    /// another unit keeps the range it declared at — a span in the OTHER
    /// file's coordinates — so a range that fails the text check is
    /// foreign and this file's token stream cannot answer questions about
    /// it.
    fn name_text_at_range(&self, name: StringId, range: SourceRange) -> bool {
        if range.offset == u32::MAX {
            return false;
        }
        let tokens = self.tokens.as_slice();
        let at = tokens.partition_point(|token| token.utf16_offset < range.offset);
        let Some(token) = tokens.get(at) else {
            return false;
        };
        if token.utf16_offset != range.offset {
            return false;
        }
        let text = self.strings.resolve(name);
        let written = &self.source[token.byte_range()];
        written == text
            || (text.ends_with('=') && written == &text[..text.len() - 1])
    }

    /// Whether the report already holds this `conflicting_static_and_instance`
    /// row (same anchor, same rendered message).
    fn static_instance_reported(&self, range: SourceRange, message: &str) -> bool {
        self.report.diagnostics.iter().any(|d| {
            d.code.name == "conflicting_static_and_instance"
                && d.range == range
                && d.message.to_string() == message
        })
    }

    /// Emits the own-scope `conflicting_static_and_instance` row (the
    /// container names itself as the conflicting owner) unless the same row
    /// is already in the report.
    ///
    /// An extension type reaches the check twice — the body-close hook and
    /// the declaration-end hook both fire, and an augmentation re-fires the
    /// declaration event — so the second run must not duplicate the first.
    /// Rows that differ only in the conflicting owner are distinct upstream
    /// rows (the own-scope and inherited arms report independently) and both
    /// stay.
    fn emit_static_instance(&mut self, range: SourceRange, class: &str, member: &str) {
        let code = SemanticCode::ConflictingStaticAndInstance;
        if !self.static_instance_reported(range, &code.message(class, member)) {
            self.emit(code, range, class, member);
        }
    }

    /// Emits the inherited-arm `conflicting_static_and_instance` row (the
    /// found member's declaring container renders as the conflicting owner)
    /// under the same no-duplicate contract as
    /// [`Engine::emit_static_instance`].
    fn emit_static_instance3(
        &mut self,
        range: SourceRange,
        class: &str,
        member: &str,
        owner: &str,
    ) {
        let code = SemanticCode::ConflictingStaticAndInstance;
        if !self.static_instance_reported(range, &code.message3(class, member, owner)) {
            self.emit3(code, range, class, member, owner);
        }
    }

    /// The `conflicting_static_and_instance` face (three upstream arms):
    /// 1. The OWN-scope arm — a static member whose bare name collides
    ///    with an instance member of the same container reports at the
    ///    static member's own name (`_checkClassStatic`/`_checkEnumStatic`/
    ///    `_checkExtensionStatic`, member_duplicate_definition_verifier.dart
    ///    :179-234/:423-522). Extensions report the dedicated one-argument
    ///    `extension_conflicting_static_and_instance`; `values` is exempt
    ///    on ACCESSORS only (a `values` METHOD still collides).
    /// 2. The ENUM getMember arm — an enum's static member that collides
    ///    with an INTERFACE member (an own instance member, an inherited
    ///    one, the implicit `Enum`/`Object` faces) reports one row whose
    ///    `conflictingClassName` is the enum's own name — the interface
    ///    member's declaring container never renders
    ///    (`ConflictStaticInheritedFoo`/`ConflictEnumValueInheritedFoo`
    ///    both name the enum). An enum constant is a static member, so it
    ///    is a PROBE here but never a target: the interface map
    ///    (`InheritanceManager3._getTypeMembers`) drops every static
    ///    member, so `enum E { one, two }` stays silent while
    ///    `enum E { foo; int get foo => 42; }` reports at the constant.
    /// 3. The INHERITED arm (`ErrorVerifier._checkForConflictingClassMembers`,
    ///    error_verifier.dart:3548-3642 — an arm no enum declaration
    ///    reaches; its three call sites are class/mixin/extension-type) —
    ///    each static member probes `getInherited(name)` then
    ///    `getInherited(name=)`; a non-static hit reports
    ///    `conflictingClassName` = the found owner (`Object`/superclass/
    ///    constraint included). Both an own row and an inherited row can
    ///    fire for one member (upstream emits each independently).
    ///
    /// The comparator scopes upstream applies, both pinned by #3583:
    /// - An EXTENSION TYPE has no implicit `Object` face. Its inherited
    ///   conflicts come from `getInterface(element).conflicts`, whose
    ///   `redeclared` map is built from the `implements` clause
    ///   (`InheritanceManager3._getInterfaceExtensionType`, upstream Dart
    ///   SDK source, pkg/analyzer/lib/src/dart/element/
    ///   inheritance_manager3.dart:778-792) — a `static int hashCode()`
    ///   reports `extension_type_declares_member_of_object` alone.
    /// - A CONSTRUCTOR is not a member of the instance scope
    ///   (`_checkClassStatic` walks `ClassMember` field/method
    ///   declarations only), so a `factory foo()` never collides with a
    ///   static member named `foo`.
    pub(crate) fn check_conflicting_static_and_instance(&mut self) {
        let Some((container, container_kind)) = self.container_context() else {
            return;
        };
        // The merged element face (`resolve_container` → `element_face`):
        // the introduction plus every `augment` fragment's rows across
        // the library's units.
        let merged_decl = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        )
        .map(|(decl, _)| decl);
        // The container's own constructor selectors. A constructor is not a
        // member of the instance scope (`member_duplicate_definition
        // .dart:191-233` walks `ClassMember` field/method declarations), and
        // the member capture keeps a factory head as a site of its own
        // ("factories … keep their member capture"), so the name must be
        // dropped here or a `factory foo()` would read as an instance member
        // of `foo`.
        let ctor_names: FxHashSet<StringId> = merged_decl
            .map(|decl| decl.constructors.iter().copied().collect())
            .unwrap_or_default();
        // The instance-scope bare names of the ELEMENT's members. The
        // oracle keys `_InstanceElementContext` on the merged element's
        // first fragment (`_getElementContext(fragment.element
        // .firstFragment)`, member_duplicate_definition_verifier.dart
        // :185-186/:591-594): members introduced inside `augment`
        // containers of any unit of the library claim into the same scope
        // — `augment class C { int foo; }` lands `foo` beside the
        // introduction's own members — while a member carrying its own
        // `augment` keyword claims nothing (:307-309). Primary-
        // constructor declaring formals (`class C(var v)`) and an
        // extension type's representation field (`extension type ET(int
        // id)`) join through the same map — the induced instance fields
        // the `primaryConstructor` arm claims (:62-97).
        //
        // Two halves union because neither alone is complete:
        // - the decl-site view sees THIS file's instance members verbatim
        //   — including the keep-first losers the member map drops (a
        //   `static m` + instance `m` pair keeps only the static in
        //   `members`, so the instance claim survives nowhere in the map);
        // - the merged element face supplies the OTHER units' augment-
        //   container members — claims a part file's `augment class C`
        //   makes into this element's scope.
        //
        // Upstream `fragment.isAugmentation` is `node.augmentKeyword !=
        // null`, and `augmentKeyword` binds only at the member's LEADING
        // modifier slot (parser_impl.dart's first slot) — `external
        // augment m();` is a plain member that claims. The model's
        // `is_augment`/`site.is_augment` marks `augment` at ANY position,
        // so this file's rows re-derive leading position from the token
        // stream (`member_augment_is_leading`). A foreign member's range
        // is in its own file's coordinates — `name_text_at_range` confirms
        // the range lands here before scanning; a range that doesn't
        // verify is foreign and stays excluded (the common foreign
        // `is_augment` shape is leading anyway; the cross-unit `external
        // augment` claim is the documented residual).
        let mut instance_names: FxHashSet<StringId> = self
            .decl_sites
            .iter()
            .filter(|site| {
                matches!(
                    &site.kind,
                    DeclKind::Member {
                        is_static: false,
                        container: c,
                        ..
                    } if *c == container
                ) && (!site.is_augment || !self.member_augment_is_leading(site.range))
            })
            .map(|site| self.bare_name(site.name))
            .collect();
        // The model-member half adds what decl_sites cannot see: the
        // extension type's representation field and the primary
        // constructor's declaring formals register in `members` but never
        // emit decl sites; cross-unit fragment members claim through the
        // same map. This file's own row comes from the merged face (the
        // element's intro view); the other units' fragment rows are walked
        // per unit — `members`'s keep-first fold drops same-name pairs (a
        // static `m` in one fragment plus an instance `m` in another keeps
        // only the first row, so the merged face alone cannot see the pair
        // — the exact cross-unit shape `_checkUnitStatic` exists to catch).
        for unit in self.graph.unit_files(self.lib) {
            let decl = if *unit == self.file {
                merged_decl
            } else {
                self.index
                    .get(*unit)
                    .and_then(|elements| elements.container(container))
            };
            let Some(decl) = decl else { continue };
            for (name, entry) in decl.members.iter() {
                if entry.is_static {
                    continue;
                }
                if entry.is_augment {
                    // This file's fragment members re-verify leading
                    // position from the token stream; a foreign member's
                    // range is in its own file's coordinates — the scan
                    // cannot run, and the common shape is leading anyway,
                    // so it stays excluded (the cross-unit `external
                    // augment` claim is the documented residual).
                    if *unit == self.file
                        && self.name_text_at_range(*name, entry.name_range)
                        && !self.member_augment_is_leading(entry.name_range)
                    {
                        instance_names.insert(self.bare_name(*name));
                    }
                    continue;
                }
                instance_names.insert(self.bare_name(*name));
            }
        }
        instance_names.retain(|name| {
            !ctor_names.contains(name) && !self.strings.resolve(*name).is_empty()
        });
        // The implicit `Object` instance members a static member can
        // collide with through `getInherited` — the walk has no model rows
        // for them (they live on dart:core's `Object`), so the probe falls
        // back to this fixed set. `index` is `Enum`'s, so an enum's probe
        // set adds it on top (a class inherits no `index`; the enum's own
        // constants are static and answer neither probe). An extension type
        // gets NO implicit face: its conflict set is computed from the
        // `implements` clause alone (`InheritanceManager3
        // ._getInterfaceExtensionType` builds candidates from
        // `element.interfaces`; `Object` is not one), so a `static int
        // hashCode()` reports `extension_type_declares_member_of_object`
        // alone.
        const IMPLICIT_OBJECT: [&str; 5] = [
            "hashCode",
            "toString",
            "noSuchMethod",
            "runtimeType",
            "==",
        ];
        const IMPLICIT_ENUM: [&str; 6] = [
            "index",
            "hashCode",
            "toString",
            "noSuchMethod",
            "runtimeType",
            "==",
        ];
        let values = self.intern("values");
        let is_enum = container_kind == ContainerKind::Enum;
        let is_extension_type = container_kind == ContainerKind::ExtensionType;
        let object_name = self.intern("Object");
        let implicit: &[&str] = if is_extension_type {
            &[]
        } else if is_enum {
            &IMPLICIT_ENUM
        } else {
            &IMPLICIT_OBJECT
        };
        // The static member sites, in declaration order. An enum constant
        // is one of them (a constant IS a static field, and its induced
        // getter joins `_checkEnumStatic`'s accessor walk) — but it is
        // only ever a PROBE. Nothing static answers a probe: the interface
        // map drops every static member (`InheritanceManager3
        // ._getTypeMembers`), so a constant collides with its container's
        // INSTANCE surface only, never with itself or a sibling.
        let sites: Vec<(SourceRange, StringId, bool)> = self
            .decl_sites
            .iter()
            .filter(|site| {
                matches!(
                    &site.kind,
                    DeclKind::Member {
                        is_static: true,
                        container: c,
                        ..
                    } if *c == container
                )
            })
            .map(|site| {
                let bare = self.bare_name(site.name);
                // `values` is exempt only for accessors — a field/getter/
                // setter carries the `values` exemption upstream, a
                // method does not (member_duplicate_definition_verifier
                // .dart:445-458 reads the accessor face). The `kind` flag
                // marks accessor sites for the exemption gate.
                let accessor = matches!(
                    &site.kind,
                    DeclKind::Member {
                        kind: MemberKind::Getter | MemberKind::Setter | MemberKind::Field,
                        ..
                    }
                );
                (site.range, bare, accessor)
            })
            .collect();
        if sites.is_empty() {
            return;
        }
        let container_text = self.strings.resolve(container).to_string();
        let is_extension = container_kind == ContainerKind::Extension;
        for (range, base, accessor) in sites {
            let base_text = self.strings.resolve(base).to_string();
            // A recovery-shaped static site with no name never probes:
            // upstream's `identifier.isSynthetic` gate
            // (member_duplicate_definition_verifier.dart:303) keeps the
            // missing-identifier face out of both scopes, so `static mixin
            // M {}`'s unnamed recovery row cannot collide with anything.
            if base_text.is_empty() {
                continue;
            }
            if is_extension {
                // The extension arm reports the one-argument dedicated
                // code — own-face only (extensions never inherit).
                if base != values || !accessor {
                    if instance_names.contains(&base) {
                        self.emit(
                            SemanticCode::ExtensionConflictingStaticAndInstance,
                            range,
                            &base_text,
                            "",
                        );
                    }
                }
                continue;
            }
            // `values` is exempt on ACCESSORS only
            // (member_duplicate_definition_verifier.dart:436-458 reads the
            // accessor face): a static field named `values` never reports,
            // a method still does. Written getters/setters land in the
            // model under the method kind, so only field sites carry the
            // flag here.
            let values_accessor = base == values && accessor;
            // Arm 1 — own-scope instance members. The decl-site view is
            // the authoritative one: the member map's keep-first row for
            // a colliding pair is the static member itself, so an instance
            // member of the same bare name survives only here.
            let own_hit = instance_names.contains(&base);
            if is_enum {
                // `_checkEnumStatic` probes the enum's INTERFACE once per
                // static member — own instance members, inherited members
                // (`getMember`), the implicit `Enum`/`Object` faces — and
                // reports a single row whose `conflictingClassName` is the
                // enum's own name. Both halves of `_getInterfaceMember`
                // (the getter face, then the `name=` face) are interface
                // lookups, so the strictly-above climb covers what arm 1's
                // decl-site view does not.
                let inherited_hit = matches!(
                    self.walk_from(container, base, true),
                    Walk::Found(entry) if !entry.is_static
                );
                let setter_spelling = self.setter_spelling(base);
                let setter_hit = matches!(
                    self.walk_from(container, setter_spelling, true),
                    Walk::Found(entry) if !entry.is_static
                );
                let implicit_hit = implicit.iter().any(|member| self.intern(member) == base);
                if !values_accessor
                    && (own_hit || inherited_hit || setter_hit || implicit_hit)
                {
                    self.emit_static_instance3(
                        range,
                        &container_text,
                        &base_text,
                        &container_text,
                    );
                }
                continue;
            }
            // Arm 1 — own-scope. `values` accessors are exempt; every
            // other bare name in the instance scope collides.
            if own_hit && !values_accessor {
                self.emit_static_instance(range, &container_text, &base_text);
            }
            // Arm 3 — the inherited probe. Every non-enum kind reads
            // strictly above the container (`skip_own`): the interface
            // arm and the inherited arm are independent upstream, so a
            // member with both an own and an inherited collision reports
            // twice.
            let found = match self.walk_from(container, base, true) {
                Walk::Found(entry) => Some(entry),
                _ => None,
            };
            // The declaring container of the probe's hit (`last_origin`
            // reads the memoized or fresh outcome).
            let origin = self.member_faces.last_origin();
            // An extension type's inherited face is its `implements`
            // clause; `Object` never joins that map, so an `Object`-owned
            // hit is no conflict (the implicit-set exclusion above has the
            // same reason).
            let object_owned = is_extension_type && origin == Some(object_name);
            // The getter probe landed a non-static inherited member: its
            // declaring container is the conflicting owner.
            if let Some(entry) = &found
                && !entry.is_static
                && !object_owned
            {
                let owner = origin
                    .map(|name| self.strings.resolve(name).to_string())
                    .unwrap_or_else(|| container_text.clone());
                self.emit_static_instance3(range, &container_text, &base_text, &owner);
                continue;
            }
            // The implicit `Object` faces answer the same probe without
            // model rows (dart:core owns them); their declaring container
            // is what renders.
            if implicit.iter().any(|member| self.intern(member) == base) {
                self.emit_static_instance3(range, &container_text, &base_text, "Object");
                continue;
            }
            // The setter probe (`getInherited(name=)`): a written setter's
            // `name=` spelling answers where the bare getter probe missed.
            // An extension type skips an `Object`-owned hit for the same
            // reason the getter probe does.
            let setter_spelling = self.setter_spelling(base);
            if let Walk::Found(entry) = self.walk_from(container, setter_spelling, true)
                && !entry.is_static
            {
                let origin = self.member_faces.last_origin();
                if !(is_extension_type && origin == Some(object_name)) {
                    let owner = origin
                        .map(|name| self.strings.resolve(name).to_string())
                        .unwrap_or_else(|| container_text.clone());
                    self.emit_static_instance3(range, &container_text, &base_text, &owner);
                }
            }
        }
        self.emit_wildcard_dups();
    }

    pub(crate) fn run_sealed_contract(&mut self, end: SourceRange) {
        let Some(deferred) = self.deferred_modifier.take() else {
            return;
        };
        if let Some(container) = deferred.container {
            let span = SourceRange {
                offset: deferred.decl_start.offset,
                length: end.end().saturating_sub(deferred.decl_start.offset),
            };
            self.check_sealed_supertype_contract(container, span);
        }
        self.emit_wildcard_dups();
    }

    /// The use-site restriction families, driven with the written clause
    /// spans captured from the header (the element model's clause lists are
    /// complete at the name event, but their entry spans only stream with
    /// the header parse — #1289 E1): the outside-library family
    /// (`error_verifier.dart` `_checkFor*OutsideOfLibrary`, drive order
    /// base-implemented walks → interface extends → final faces → sealed
    /// faces) anchors each row on its written supertype entry, the
    /// base-or-final subtype requirement (`BaseOrFinalTypeVerifier
    /// .checkElement`) anchors the declaring name — `.at(nameToken)`.
    /// Classes and named mixin applications carry extends/with/implements,
    /// mixins implements/`on`; enums ride the class drive with a null
    /// superclass and the `with`/`implements` clauses the grammar allows
    /// (`visitEnumDeclaration` → `_checkClassInheritance`,
    /// error_verifier.dart:823-863), drained from the enum's own
    /// implements-claim event.
    pub(crate) fn check_modifier_use_sites(
        &mut self,
        container: StringId,
        name_range: SourceRange,
        spans: &HeaderClauseSpans,
    ) {
        if !self.reported_modifier_checks.insert(container) {
            return;
        }
        // The `dart:ffi` verifier's post-parse pass reads the same written
        // clause spans (`subtype_of_struct_class` anchors each row on its
        // written entry). The `with` faces sit in `buffered`'s tail — the
        // implements claim consumed the implements entries ahead of them,
        // and nothing claims the with window, so the last `with_clause`
        // entries are the with faces in application order (the
        // `check_mixin_application_overrides` tail-slice precedent).
        let with_count = self
            .index
            .get(self.file)
            .and_then(|elements| elements.container(container))
            .map_or(0, |decl| decl.with_clause.len());
        let withs = if spans.buffered.len() >= with_count {
            spans.buffered[spans.buffered.len() - with_count..].to_vec()
        } else {
            Vec::new()
        };
        self.ffi_clause_spans.insert(
            container,
            crate::diagnose::types::FfiClauseSpans {
                extends: spans.extends,
                implements: spans.implements.clone(),
                withs,
            },
        );
        for finding in self.plan_modifier_use_sites(container, name_range, spans) {
            self.emit_modifier(finding.code, finding.anchor, &finding.a0, &finding.a1);
        }
    }

    /// The package:meta `@sealed` supertype contract
    /// (`_checkForInvalidSealedSuperclass`, `best_practices_verifier.dart`:
    /// 1172-1213, mounted on class/mixin/class-type-alias declarations):
    /// the declaring node's `allSupertypes` closure is walked once
    /// ([`FoldHierarchy::closure`], cycle-guarded), and every supertype
    /// whose annotations recognize the meta `sealed` getter
    /// ([`is_meta_getter`]) reports once at the declaration —
    /// `mixin_on_sealed_class` when the supertype is a direct `on`
    /// constraint, `subtype_of_sealed_class` otherwise. Every row anchors
    /// on the whole declaring node (`.at(node)`) — the deferred drain
    /// passes its span. Supertypes whose declaring library is inside the
    /// analyzed package stay silent (`_isLibraryInWorkspacePackage`: the
    /// sealed contract restricts only other packages), so a same-package
    /// `@sealed` base never fires.
    pub(crate) fn check_sealed_supertype_contract(
        &mut self,
        container: StringId,
        decl_range: SourceRange,
    ) {
        if !self.reported_sealed_checks.insert(container) {
            return;
        }
        let (graph, namespaces, index, strings) =
            (self.graph, self.namespaces, self.index, self.strings);
        let element = (self.file, container);
        let alias_hop =
            |name: StringId, file: FileId, lib: LibraryId| self.alias_clause_hop(name, file, lib);
        let hierarchy = FoldHierarchy {
            graph,
            namespaces,
            index,
            file: self.file,
            lib: self.lib,
            class_modifiers: self.class_modifiers,
            memo: RefCell::default(),
            in_progress: RefCell::default(),
            alias_head: Some(&alias_hop),
        };
        // The constraint face is the DIRECT `on` list (the official
        // `superclassConstraints.contains`, :1195-1196): the same sealed
        // supertype reached through any other path reports the regular
        // subtype face, and only mixins carry `on` constraints. The
        // hierarchy phase (immutable borrows, alias fold included) ends
        // before the report writes below.
        let sealed_hits: Vec<((FileId, StringId), bool)> = {
            let direct_ons: FxHashSet<(FileId, StringId)> =
                hierarchy.face_handles(element).3.into_iter().collect();
            hierarchy
                .closure(element)
                .into_iter()
                .filter_map(|handle| {
                    let Some(decl) = index
                        .get(handle.0)
                        .and_then(|elements| elements.container(handle.1))
                    else {
                        return None;
                    };
                    let owner_lib = hierarchy.library_of(handle);
                    let is_sealed = decl.annotations.iter().any(|annotation| {
                        is_meta_getter(annotation, "sealed", strings, graph, owner_lib)
                    });
                    (is_sealed && !same_workspace_package(graph, owner_lib, self.lib))
                        .then_some((handle, direct_ons.contains(&handle)))
                })
                .collect()
        };
        for (handle, direct_on) in sealed_hits {
            let code = if direct_on {
                SemanticCode::MixinOnSealedClass
            } else {
                SemanticCode::SubtypeOfSealedClass
            };
            let sealed_name = strings.resolve(handle.1).to_owned();
            let correction = code.correction_message(&sealed_name);
            self.report.diagnostics.push(Diagnostic::new(
                code.severity(),
                code.code_info(),
                code.message(&sealed_name, ""),
                self.file,
                decl_range,
                correction,
            ));
        }
    }

    /// The library facts the platform-exemption gate reads
    /// (`_mayIgnoreClassModifiers`, error_verifier.dart:8468-8482):
    /// platform (`dart:`) membership, the `dart:ffi` identity
    /// (`_isDartFfiLibrary`, error_verifier.dart:8876 — the library name
    /// `dart.ffi`, spelled `dart:ffi` as the synthetic face node's URI), and
    /// the library's `class-modifiers` feature state — the defining file's
    /// settled language version (the `@dart` override applied) resolved
    /// through the feature registry and injected per analyze
    /// ([`LibraryClassModifiers`]).
    pub(crate) fn library_facts(&self, lib: LibraryId) -> LibraryFeatureFacts {
        let uri = self.graph.uri(lib);
        LibraryFeatureFacts {
            is_sdk: uri.starts_with("dart:"),
            is_dart_ffi: uri == "dart:ffi",
            has_class_modifiers: self.class_modifiers.is_enabled(lib),
        }
    }

    /// Resolves one written super-face name to its declaring handle and
    /// library (#491 S2a attribution); `None` suppresses the face. `writer`
    /// is the declaring container whose header wrote `name` — the
    /// prefixed-clause probe's key (`implements p.Base` resolves through
    /// `p`'s table). The handle carries the *declaring* file — the same
    /// shape the fold walk's hop produces — so `FoldHierarchy::library_of`
    /// attributes the super's own library and the outside-library gates
    /// (`element.library != _currentLibrary`, error_verifier.dart:5619-5622
    /// /:5917-5921/:3725-3730) compare the right pair.
    pub(crate) fn resolve_super(
        &self,
        writer: StringId,
        name: StringId,
    ) -> Option<((FileId, StringId), LibraryId)> {
        // A face written through an import prefix (`class C implements
        // p.Base`) reaches the element model as its bare leaf — the
        // collector keeps the local name alone — so the bare walk below
        // misses it and the face reads as an unresolvable edge (every
        // outside-library arm then stays silent). The dotted spelling lives
        // in the header tokens ([`Self::prefixed_clause_prefix`], the same
        // facts a scanned [`SuperEntry`]'s `prefix` carries): resolve the
        // leaf through the prefix's merged table, the container identity an
        // unprefixed import of the same library yields. A registered prefix
        // whose table lacks the name suppresses — never fall back to the
        // bare scope, where a same-spelled local would shadow the written
        // face.
        if let Some(prefix) = self.prefixed_clause_prefix(writer, name) {
            let lib = self.graph.library_of_file(self.file).unwrap_or(self.lib);
            let (_, owner) =
                prefixed_container(self.graph, self.namespaces, self.index, lib, prefix, name)?;
            let owner_lib = owner_library(self.graph, self.lib, owner, name).unwrap_or(self.lib);
            return Some(((owner, name), owner_lib));
        }
        if let Some((_, owner)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        ) {
            let lib = owner_library(self.graph, self.lib, owner, name).unwrap_or(self.lib);
            return Some(((owner, name), lib));
        }
        // A type ALIAS in clause position resolves to its right-hand head:
        // every outside-library family reads `namedType.type` — the resolved
        // interface type — so `class B implements TypedefBase` sees the
        // aliased class (the diagnostic names the CLASS, never the alias)
        // while the row still anchors on the written entry
        // (upstream Dart SDK source,
        // `pkg/analyzer/lib/src/generated/error_verifier.dart:3317-3345`
        // `_checkForBaseClassOrMixinImplementedOutsideOfLibrary` and the
        // sibling `_checkFor*OutsideOfLibrary` reads; the co19
        // Class-modifiers `*_t0x` suites name their shapes through
        // `typedef TypedefBaseClass = BaseClass;`).
        let (_, owner, head) = self.alias_container_head(name, self.lib, self.file)?;
        let lib = owner_library(self.graph, self.lib, owner, head).unwrap_or(self.lib);
        Some(((owner, head), lib))
    }

    /// The use-site decisions, computed read-only and emitted by the caller:
    /// every super face resolves through the fold's namespace hop (an
    /// unresolvable face suppresses — the under-report direction), and the
    /// effective-modifier facts (declared + sealed-class induced) come from
    /// the [`FoldHierarchy`]. Each face carries its written entry's span
    /// (`spans`, positional with the element model's clause lists — #1289
    /// E1): the outside-library rows anchor there, the subtype rows on the
    /// declaring name. A face whose capture has no span anchors on the
    /// declaring name.
    #[allow(clippy::too_many_lines)] // the five official passes, one linear sequence
    pub(crate) fn plan_modifier_use_sites(
        &self,
        container: StringId,
        name_range: SourceRange,
        spans: &HeaderClauseSpans,
    ) -> Vec<ModifierFinding> {
        let Some(elements) = self.index.get(self.file) else {
            return Vec::new();
        };
        let Some(decl) = elements.container(container) else {
            return Vec::new();
        };
        let display = |name: StringId| self.strings.resolve(name).to_owned();
        let element = (self.file, container);
        let use_facts = self.library_facts(self.lib);
        let alias_hop =
            |name: StringId, file: FileId, lib: LibraryId| self.alias_clause_hop(name, file, lib);
        let hierarchy = FoldHierarchy {
            graph: self.graph,
            namespaces: self.namespaces,
            index: self.index,
            file: self.file,
            lib: self.lib,
            class_modifiers: self.class_modifiers,
            memo: RefCell::default(),
            in_progress: RefCell::default(),
            alias_head: Some(&alias_hop),
        };
        // The resolved faces, written order within a clause, each with its
        // written entry's span. The extends head exists on class-family
        // declarations only (mixins have no superclass; enums never reach
        // this face).
        let mut extends: Option<ResolvedSuper> = None;
        let mut withs: Vec<ResolvedSuper> = Vec::new();
        let mut implements: Vec<ResolvedSuper> = Vec::new();
        let mut ons: Vec<ResolvedSuper> = Vec::new();
        if let Some(name) = decl.superclass
            && let Some((handle, lib)) = self.resolve_super(container, name)
        {
            extends = Some(ResolvedSuper {
                handle,
                lib,
                relation: Relation::Extends,
                span: spans.extends.unwrap_or(name_range),
            });
        }
        // The `with` faces' written spans: the UNCLAIMED tail of the clause
        // buffer — the implements claim consumed the entries written ahead of
        // them, and nothing claims the `with` window, so the last
        // `with_clause` entries are the `with` faces in application order
        // (the same tail slice [`Self::check_modifier_use_sites`] hands the
        // `dart:ffi` face). A `with`-only header never closes its extends
        // window, so its FIRST entry sits in the (never committed) candidate
        // slot and the buffer stays short — the clause scanner's `With` rows
        // then supply the spans ([`Self::header_with_entry_spans`]). The
        // sealed rows anchor there (`.at(namedType)`,
        // error_verifier.dart:7019-7034); a missing span (a claim/count
        // disagreement in a recovery shape) falls back to the declaring name.
        let with_spans: Vec<SourceRange> = {
            let count = decl.with_clause.len();
            let mut tail: Vec<SourceRange> = Vec::new();
            if count > 0 {
                if decl.superclass.is_none() {
                    tail.extend(spans.super_candidate);
                }
                tail.extend(spans.buffered.iter().copied());
            }
            if count == 0 {
                Vec::new()
            } else if tail.len() >= count {
                tail[tail.len() - count..].to_vec()
            } else {
                self.header_with_entry_spans(name_range.offset)
            }
        };
        for (index, name) in decl.with_clause.iter().enumerate() {
            if let Some((handle, lib)) = self.resolve_super(container, *name) {
                withs.push(ResolvedSuper {
                    handle,
                    lib,
                    relation: Relation::MixesIn,
                    span: with_spans.get(index).copied().unwrap_or(name_range),
                });
            }
        }
        for (index, name) in decl.implements_clause.iter().enumerate() {
            if let Some((handle, lib)) = self.resolve_super(container, *name) {
                implements.push(ResolvedSuper {
                    handle,
                    lib,
                    relation: Relation::Implements,
                    span: spans.implements.get(index).copied().unwrap_or(name_range),
                });
            }
        }
        for (index, name) in decl.on_clause.iter().enumerate() {
            if let Some((handle, lib)) = self.resolve_super(container, *name) {
                ons.push(ResolvedSuper {
                    handle,
                    lib,
                    relation: Relation::MixinConstraint,
                    span: spans.on.get(index).copied().unwrap_or(name_range),
                });
            }
        }
        let super_facts = |entry: &ResolvedSuper| SuperFacts {
            modifiers: hierarchy.effective(entry.handle),
            same_library: hierarchy.library_of(entry.handle) == self.lib,
            may_ignore: may_ignore_class_modifiers(
                self.library_facts(hierarchy.library_of(entry.handle)),
                use_facts,
            ),
        };
        let mut findings = Vec::new();
        // The class-family/mixin drive split (error_verifier.dart:2327-2370
        // and :7808-7845): an ENUM rides the class drive with a null
        // superclass (`visitEnumDeclaration` → `_checkClassInheritance`,
        // :823-863), so it carries the `with` face and never the `on` one.
        let class_drive = matches!(decl.kind, ContainerKind::Class | ContainerKind::Enum);
        // The clause-error drive gate (upstream Dart SDK source,
        // `pkg/analyzer/lib/src/generated/error_verifier.dart`): the
        // outside-library block (passes 1–4) stands down when a clause-error
        // probe already fired — the class drive's four probes
        // (`_checkForExtendsDisallowedClass`,
        // `_checkForImplementsClauseErrorCodes`,
        // `_checkForAllMixinErrorCodes`,
        // `_checkForNoGenerativeConstructorsInSuperclass`, :2746-2753) and
        // the mixin drive's two (`_checkForOnClauseErrorCodes`,
        // `_checkForImplementsClauseErrorCodes`, :8168-8169). Modeled here:
        // the disallowed-subtype arm (`isNonSubtypableClass`/`isDartCoreEnum`
        // — the rows dartq emits as `subtype_of_disallowed_type` plus the
        // core-`Enum` `with` arm, the probes the conformance extras proved).
        // Pass 5 (the resolver-driven `BaseOrFinalTypeVerifier`) is NOT
        // gated. Deliberately under-gated for iteration 1:
        // `class_used_as_mixin` reports WITHOUT setting `problemReported`
        // upstream (error_verifier.dart:2917-2926 — a sealed class reached
        // through `with` still reports beside it,
        // `test_mixinOutside_rawClass`), so a plain class in `with` never
        // gates; `mixin_inherits_not_from_object`/`mixin_instantiate`
        // (non-Object or extension-type with-faces) and
        // `no_generative_constructors_in_superclass` stay unmodeled — a
        // deferred clause's `*_deferred_class` rows live in probe arms
        // dartq does not emit either — so a clause error they alone fire
        // still emits the outside-library rows beside it.
        let clause_error = if class_drive {
            decl.superclass
                .is_some_and(|name| self.non_subtypable_clause_head(name))
                || decl
                    .implements_clause
                    .iter()
                    .chain(decl.with_clause.iter())
                    .any(|&name| self.non_subtypable_clause_head(name))
        } else {
            decl.on_clause
                .iter()
                .chain(decl.implements_clause.iter())
                .any(|&name| self.non_subtypable_clause_head(name))
        };
        if !clause_error {
        // Pass 1 — the base-implemented walks (`implements` entries, the
        // named type plus its transitive supertypes, first report per
        // entry; :3712-3752).
        for entry in &implements {
            for (handle, _) in std::iter::once((entry.handle, true)).chain(
                hierarchy
                    .closure(entry.handle)
                    .into_iter()
                    .map(|h| (h, false)),
            ) {
                let facts = SuperFacts {
                    modifiers: hierarchy.effective(handle),
                    same_library: hierarchy.library_of(handle) == self.lib,
                    may_ignore: may_ignore_class_modifiers(
                        self.library_facts(hierarchy.library_of(handle)),
                        use_facts,
                    ),
                };
                if let Some(code) = base_implemented_candidate(&facts) {
                    findings.push(ModifierFinding {
                        code,
                        a0: display(handle.1),
                        a1: String::new(),
                        anchor: entry.span,
                    });
                    break;
                }
            }
        }
        // Pass 2 — the interface extends face (class drive only;
        // :5896-5926).
        if let Some(entry) = &extends
            && let Some(code) = interface_extends_violation(&super_facts(entry))
        {
            findings.push(ModifierFinding {
                code,
                a0: display(entry.handle.1),
                a1: String::new(),
                anchor: entry.span,
            });
        }
        // Pass 3 — the final faces, official order: extends head
        // (:5612-5627), `implements` walks (:5628-5665), `on` constraints
        // (:5666-5686, mixin drive only).
        if let Some(entry) = &extends
            && let Some(code) = final_extends_violation(&super_facts(entry))
        {
            findings.push(ModifierFinding {
                code,
                a0: display(entry.handle.1),
                a1: String::new(),
                anchor: entry.span,
            });
        }
        for entry in &implements {
            let named_facts = self.library_facts(entry.lib);
            for (handle, is_named) in std::iter::once((entry.handle, true)).chain(
                hierarchy
                    .closure(entry.handle)
                    .into_iter()
                    .map(|h| (h, false)),
            ) {
                let facts = SuperFacts {
                    modifiers: hierarchy.effective(handle),
                    same_library: hierarchy.library_of(handle) == self.lib,
                    may_ignore: may_ignore_class_modifiers(
                        self.library_facts(hierarchy.library_of(handle)),
                        use_facts,
                    ),
                };
                if let Some(code) = final_implements_candidate(
                    &facts,
                    &ImplementsStep {
                        is_named_type: is_named,
                        named_type_has_class_modifiers: named_facts.has_class_modifiers,
                    },
                ) {
                    findings.push(ModifierFinding {
                        code,
                        a0: display(handle.1),
                        a1: String::new(),
                        anchor: entry.span,
                    });
                    break;
                }
            }
        }
        for entry in &ons {
            if let Some(code) = final_mixin_constraint_violation(&super_facts(entry)) {
                findings.push(ModifierFinding {
                    code,
                    a0: display(entry.handle.1),
                    a1: String::new(),
                    anchor: entry.span,
                });
            }
        }
        // Pass 4 — the sealed faces, every written face checked
        // independently (the official NamedType lists: class drive
        // superclass + `with` + `implements` — error_verifier.dart:2363-2367
        // `[?superclass, ...?withClause?.mixinTypes,
        // ...?implementsClause?.interfaces]`; mixin drive `implements` +
        // `on` — :7838-7841). The class drive sits behind the
        // `_checkForAllMixinErrorCodes` gate (:2338-2342), but that gate's
        // class-used-as-mixin arm reports its own row WITHOUT setting
        // `problemReported` (:2488-2500), so a sealed class reached through
        // `with` reports beside `class_used_as_mixin`
        // (`test_mixinOutside_rawClass`). Enums ride the class drive with a
        // null superclass, so their `with` entries join here too.
        let sealed_faces: Vec<&ResolvedSuper> = if class_drive {
            extends
                .iter()
                .chain(withs.iter())
                .chain(implements.iter())
                .collect()
        } else {
            implements.iter().chain(ons.iter()).collect()
        };
        for entry in &sealed_faces {
            if let Some(code) = sealed_supertype_violation(&super_facts(entry)) {
                findings.push(ModifierFinding {
                    code,
                    a0: display(entry.handle.1),
                    a1: String::new(),
                    anchor: entry.span,
                });
            }
        }
        }
        // Pass 5 — the base-or-final subtype requirement, official face
        // order superclass → implements → with → `on` (classes) /
        // implements → `on` (mixins), one error per element
        // (base_or_final_type_verifier.dart:47-74). The `{1}` slot names
        // the nearest explicit `base`/`final` ancestor, not the direct
        // super (:198/:222-315).
        //
        // The verifier mounts on the resolver's class, class-type-alias
        // and mixin visitors ONLY (resolver.dart's three
        // `baseOrFinalTypeVerifier.checkElement` sites) — an enum or
        // extension type never runs it: `enum Bar implements Base` stays
        // silent on this family no matter the modifier face (the co19
        // Class-modifiers `EnumImplements*`/`EnumWith*`/`test_enum_*`
        // rows oracle never emits). The face lists are the ELEMENT's
        // resolved supertype columns, not the written clauses: the
        // extends head joins `element.supertype` only when it resolves
        // to a class (`_isInterfaceTypeClass`, types_builder.dart:40-46
        // — `class C extends SomeMixin` leaves `supertype` null), and
        // the `implements`/`with`/`on` lists keep interface-typed
        // elements only (`isInterfaceTypeInterface`, types_builder.dart
        // :23-37 — enum and extension-type entries drop out).
        let super_kind = |entry: &ResolvedSuper| {
            self.index
                .get(entry.handle.0)
                .and_then(|file| file.container(entry.handle.1))
                .map(|decl| decl.kind)
        };
        let interface_typed = |entry: &ResolvedSuper| {
            matches!(
                super_kind(entry),
                Some(ContainerKind::Class | ContainerKind::Mixin)
            )
        };
        let subtype_faces: Vec<&ResolvedSuper> = if !matches!(
            decl.kind,
            ContainerKind::Class | ContainerKind::Mixin
        ) {
            Vec::new()
        } else if class_drive {
            extends
                .iter()
                .filter(|entry| super_kind(entry) == Some(ContainerKind::Class))
                .chain(implements.iter().filter(|e| interface_typed(e)))
                .chain(withs.iter().filter(|e| interface_typed(e)))
                .collect()
        } else {
            implements
                .iter()
                .filter(|e| interface_typed(e))
                .chain(ons.iter().filter(|e| interface_typed(e)))
                .collect()
        };
        let element_mods = hierarchy.effective(element);
        for entry in subtype_faces {
            let Some(ancestor) = nearest_explicit_base_or_final(&hierarchy, entry.handle) else {
                continue;
            };
            let ancestor_lib = hierarchy.library_of(ancestor);
            // The official same-library gate (:185-187) is
            // `element.library != _definingLibrary` — trivially false for
            // declarations diagnosed in their own library; the ancestor
            // equality that drives the final-family carve-out (:264-279)
            // is the `ExplicitBaseFinal` field.
            let input = SubtypeRestrictionInput {
                element: element_mods,
                element_library: use_facts,
                super_element: hierarchy.effective(entry.handle),
                super_library: self.library_facts(entry.lib),
                same_library: true,
                explicit_base_or_final: Some(ExplicitBaseFinal {
                    modifiers: hierarchy.effective(ancestor),
                    library: self.library_facts(ancestor_lib),
                    same_library: ancestor_lib == self.lib,
                }),
                via_implements: entry.relation == Relation::Implements,
            };
            if let Some(code) = subtype_restriction_violation(&input) {
                // The verifier's `implements`-anchored base arm
                // (base_or_final_type_verifier.dart:239-256) is spelled
                // `base_class_implemented_outside_of_library` /
                // `base_mixin_implemented_outside_of_library` with ONE
                // argument — the explicitly `base` ancestor's name — and
                // anchors on the WRITTEN implements entry, which is exactly
                // Pass 1's row (the base-implemented walk reaches the same
                // ancestor through `implements`). Pass 1 owns it: emitting
                // here under the subtype family's declaring-name anchor and
                // container argument would fabricate a second, mis-shaped
                // row (the co19 `*Sealed*Implement` families: `The class
                // 'SimpleSealedExtendImplement' can't be implemented outside
                // of its library...` at the declaring name).
                if matches!(
                    code,
                    ModifierCode::BaseClassImplementedOutsideOfLibrary
                        | ModifierCode::BaseMixinImplementedOutsideOfLibrary
                ) {
                    break;
                }
                findings.push(ModifierFinding {
                    code,
                    a0: display(container),
                    a1: display(ancestor.1),
                    anchor: name_range,
                });
                break;
            }
        }
        findings
    }

    /// Whether the clause entry `entry` spells a non-subtypable face the
    /// gate's disallowed arm covers (`_checkForExtendsOrImplementsDisallowed
    /// Class`, error_verifier.dart:5171-5187 — `type is InterfaceType &&
    /// _typeProvider.isNonSubtypableClass(...)`, plus the
    /// `_checkDirectSuperType` `isDartCoreEnum` arm the same clause-error
    /// rows fire through, inheritance_override.dart:610-620). The probe
    /// mirrors [`Self::disallowed_subtype_head`] — the shared alias-chase
    /// and the user-shadow exemption (`isNonSubtypableClass` keys on the
    /// RESOLVED element's `dart:*` library, so a user declaration named
    /// `int` is never restricted) — with the core `Enum` carve-out flipped:
    /// `with Enum` *is* a clause error upstream (its `concrete_class_has_
    /// enum_superinterface` report carries `problemReported = true`), so
    /// the gate counts it where `finalize_disallowed_subtypes`'s report arm
    /// does not.
    fn non_subtypable_clause_head(&self, entry: StringId) -> bool {
        self.non_subtypable_clause_head_spelling(entry).is_some()
    }

    /// The head name `entry` resolves to when it spells a non-subtypable SDK
    /// face — [`Self::non_subtypable_clause_head`]'s resolution decision,
    /// split out so the override gate
    /// ([`Self::disallowed_clause_suppresses_members`]) can tell the
    /// core-`Enum` arm (container-kind sensitive) from the plain disallowed
    /// name arm.
    fn non_subtypable_clause_head_spelling(&self, entry: StringId) -> Option<StringId> {
        let mut head = entry;
        // The same resolution decision [`Self::disallowed_subtype_head`]
        // makes: `resolve_container` first (a user typedef named `int`
        // shadows the SDK face before its `var_ty` is read); an unresolved
        // entry chases one level through a non-generic bare alias.
        match resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            entry,
        ) {
            Some((_, owner_file)) => {
                let is_sdk = self
                    .graph
                    .library_of_file(owner_file)
                    .is_some_and(|lib| self.graph.uri(lib).starts_with("dart:"));
                if !is_sdk {
                    return None;
                }
            }
            None => {
                // No indexed container under the spelling: the bare
                // non-generic alias chase is the only face the model
                // keeps; an absent decl is the closed-spelling face (the
                // names-only `dart:*` stub resolves it as a type without
                // an index entry — `disallowed_subtype_head`'s fallthrough
                // contract).
                if let Some(decl) = self.index.get(self.file).and_then(|f| f.decl(entry)) {
                    if decl.kind != MicroKind::TypeAlias || decl.type_params != 0 {
                        return None;
                    }
                    match decl.var_ty {
                        Ty::Named { name, .. } => head = name,
                        Ty::Null => head = self.intern("Null"),
                        _ => return None,
                    }
                    // The chased head rides the same decision: a user
                    // element under that name is never restricted.
                    if let Some((_, owner_file)) = resolve_container(
                        self.graph,
                        self.namespaces,
                        self.index,
                        self.lib,
                        self.file,
                        head,
                    ) {
                        let is_sdk = self
                            .graph
                            .library_of_file(owner_file)
                            .is_some_and(|lib| self.graph.uri(lib).starts_with("dart:"));
                        if !is_sdk {
                            return None;
                        }
                    }
                }
            }
        }
        let spelling = self.strings.resolve(head);
        (spelling == "Enum"
            || super::engine_core::NON_SUBTYPABLE_SDK_NAMES.contains(&spelling))
        .then_some(head)
    }

    /// Whether the container's written direct-supertype clauses make the
    /// oracle's verifier return before EVERY member check — the
    /// `_checkDirectSuperTypes` early return (upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/error/inheritance_override.dart` `verify`
    /// :312-314, `_checkDirectSuperTypes` :649-688, `_checkDirectSuperType`
    /// :590-620): one disallowed name in the written `extends` / `with` /
    /// `implements` (and, for a mixin, `on`) clause reports that clause
    /// error and stands the signature checks down, so no `invalid_override`
    /// row fires on the container's members at all — the rows would compare
    /// against supertype faces the broken header never established.
    fn disallowed_clause_suppresses_members(
        &self,
        decl: &Container,
        kind: ContainerKind,
    ) -> bool {
        // `_checkDirectSuperType`'s first arm: a `dart:*` library returns
        // before any probe — the SDK's own declarations legitimately subtype
        // the restricted faces (`_Smi implements int`).
        if self.graph.uri(self.lib).starts_with("dart:") {
            return false;
        }
        // The core-`Enum` arm's container carve-out (:610-620): an abstract
        // class, an enum, and a mixin may name `Enum` freely — only a
        // CONCRETE class's `Enum` face is the clause error
        // (`concrete_class_has_enum_superinterface`). Every other restricted
        // name errors on any container kind. The element model folds
        // `sealed` into abstractness (`isAbstract = isAbstract || isSealed`,
        // element.dart:150) — the same split `check_face_inheritance`'s
        // concrete-target gate applies.
        let enum_is_clause_error =
            kind == ContainerKind::Class && !decl.is_abstract && !decl.sealed;
        let gated = |entry: StringId| {
            self.non_subtypable_clause_head_spelling(entry)
                .is_some_and(|head| self.strings.resolve(head) != "Enum" || enum_is_clause_error)
        };
        decl.superclass.is_some_and(gated)
            || decl.with_clause.iter().copied().any(gated)
            || decl.implements_clause.iter().copied().any(gated)
            || decl.on_clause.iter().copied().any(gated)
    }

    /// The written `with` entries' spans of the declaration at `name_offset`
    /// (the declaring name token's UTF-16 offset), read from the clause
    /// scanner ([`Self::scan_header_clauses`]) — the fallback anchor source
    /// for the sealed class-drive rows when the header window kept no buffer
    /// tail (a `with`-only header parks its first entry in the never-committed
    /// extends candidate slot, and the listener's own with-item capture that
    /// feeds `class_used_as_mixin` never reaches
    /// [`HeaderClauseSpans`]). One bounded token walk, paid only by headers
    /// whose window channel came up short.
    fn header_with_entry_spans(&self, name_offset: u32) -> Vec<SourceRange> {
        let Some(facts) = self.scan_header_clauses().by_offset.remove(&name_offset) else {
            return Vec::new();
        };
        facts
            .supers
            .iter()
            .filter(|entry| entry.tag == SuperTag::With)
            .map(|entry| entry.span)
            .collect()
    }

    // —— abstract completeness + interface conflicts (#498) ———————————

    /// Mounts the container-face families at the container's name event:
    /// the same-name super-interface conflicts (`inconsistent_inheritance*`),
    /// the concrete class/enum abstract completeness
    /// (`non_abstract_class_inherits_abstract_member`, five tiers), the
    /// concrete-implementation face (`invalid_implementation_override`), and
    /// the `@mustBeOverridden` walk. The element model is complete
    /// (`collect` parsed before this pass) and the name token anchors the
    /// emissions. Extensions and extension types have no inheritance faces;
    /// a mixin declaration participates in the conflicts only (the official
    /// completeness loop is `ClassElementImpl && !isAbstract ||
    /// EnumElementImpl`, `inheritance_override.dart:412`). Every check
    /// suppresses on any unresolvable face chain — the under-report
    /// direction.
    pub(crate) fn check_face_inheritance(
        &mut self,
        container: StringId,
        kind: ContainerKind,
        name_range: SourceRange,
    ) {
        if matches!(
            kind,
            ContainerKind::Extension | ContainerKind::ExtensionType
        ) {
            return;
        }
        // One emission pass per container (an augmentation re-fires the
        // event for the same container).
        if !self.reported_face_checks.insert(container) {
            return;
        }
        let Some(elements) = self.index.get(self.file) else {
            return;
        };
        let Some(decl) = elements.container(container) else {
            return;
        };
        // The clause gate stands every member check down
        // (`_checkDirectSuperTypes`' early return,
        // `inheritance_override.dart:312-314`): a disallowed
        // direct-supertype clause reports its own row and the conflicts /
        // completeness / implementation faces never run.
        if self.disallowed_clause_suppresses_members(decl, kind) {
            return;
        }
        let Some(conflicted) = self.inheritance_conflicts(decl, name_range) else {
            return;
        };
        if kind != ContainerKind::Mixin {
            // The official loop's gate: concrete classes and enums only
            // (`ClassElementImpl && !isAbstract || EnumElementImpl`). The
            // element folds `sealed` into abstractness
            // (element.dart:150 `isAbstract = isAbstract || isSealed`), so
            // a sealed declaration never enters the completeness,
            // implementation-override, or mustBeOverridden loops.
            let concrete_target =
                kind == ContainerKind::Enum || (!decl.is_abstract && !decl.sealed);
            if concrete_target && let Some(view) = self.collect_interface(decl) {
                self.abstract_completeness(kind, decl, &conflicted, &view, name_range);
                self.implementation_overrides(container, decl, &view, name_range);
                self.must_be_overridden(decl, &view, name_range);
            }
        }
    }

    /// The face's linearized chain, every non-static member: the face
    /// container itself, its `with` mixins in reverse application order, its
    /// `implements` interfaces, then the superclass hop, repeating per hop
    /// ([`Engine::face_member`]'s climb, enumerating). `top_concrete` says
    /// the face contributes implementations at all (a superclass or `with`
    /// face); an `implements`/`on` face contributes interface members only,
    /// so every hit is forced `concrete_source: false`
    /// ([`FaceEntry::concrete_source`] — `_addImplemented`'s sources are own
    /// members, mixins, and the super chain only,
    /// `inheritance_manager3.dart:333`). `None` = an unresolvable or cyclic
    /// hop — the caller suppresses rather than reports.
    pub(crate) fn face_entries(
        &self,
        face: StringId,
        top_concrete: bool,
    ) -> Option<Vec<FaceEntry<'a>>> {
        let mut name = face;
        let mut owner = self.file;
        let mut scope_lib = self.lib;
        let mut seen = FxHashSet::default();
        let mut hits: Vec<FaceEntry<'a>> = Vec::new();
        loop {
            if !seen.insert(name) {
                return None;
            }
            // An unresolvable hop degrades to the oracle's `supertype`
            // fallback, not a broken chain: `extends Missing` lands the
            // member-less `Object` face and the walk ends there, so the
            // accumulated hits stand (the abstract-completeness face still
            // reports a resolvable `implements` member — `SecurityLintCode
            // extends DiagnosticCodeImpl implements LintCode`, where the
            // `_fe_analyzer_shared` edge is dead but `LintCode`'s interface
            // names report). A top-level miss returns the empty face; a
            // nested miss ends the climb like a `None` superclass.
            let resolved = resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                scope_lib,
                owner,
                name,
            )
            .or_else(|| {
                // A hop naming a TYPE ALIAS binds the alias's right-hand
                // class (`extends IterableBase<E>` — SDK 3.47 spells
                // `typedef IterableBase<E> = Iterable<E>`): the oracle's
                // supertype is the RHS, whose members the walk climbs.
                let (container, file, head) = self.alias_container_head(name, scope_lib, owner)?;
                name = head;
                Some((container, file))
            });
            let Some((found, found_owner)) = resolved else {
                // A dead hop whose spelling names the frozen list face
                // (`with ListMixin` over a names-only SDK projection the
                // index cannot model) still contributes its members.
                self.frozen_list_face_hits(name, owner, top_concrete, &mut hits);
                return Some(hits);
            };
            if found.members.is_empty() {
                self.frozen_list_face_hits(name, found_owner, top_concrete, &mut hits);
            }
            for (member, entry) in &found.members {
                if !entry.is_static {
                    hits.push(FaceEntry {
                        name: *member,
                        entry: self.boundary.member(found_owner, name, *member, entry),
                        owner_name: name,
                        owner_file: found_owner,
                        concrete_source: top_concrete,
                    });
                }
            }
            // The mixin/superclass names are written in `found_owner` and
            // resolve through that file's library (#491 S2a).
            let owner_lib = self
                .owner_library_of(found_owner, name)
                .unwrap_or(scope_lib);
            for mixin in found.with_clause.iter().rev() {
                if !seen.insert(*mixin) {
                    return None;
                }
                // A resolved mixin CLASS / named application carries its own
                // `with` faces on the application interface (`with A<...>`
                // where `mixin class A = Object with M<...>` exposes M's
                // members exactly like a written `with M` — the same
                // named-application rule the member walk and
                // `override_face_member` apply; upstream Dart SDK source,
                // `pkg/analyzer/lib/src/dart/element/inheritance_manager3.dart`).
                // The applied faces contribute concretely; `on`/`implements`
                // contribute as interfaces. A plain mixin's `with` is empty —
                // this only fires for mixin classes and named applications.
                let mut applied: VecDeque<(StringId, FileId, LibraryId, bool)> = VecDeque::new();
                if let Some((mixin_container, mixin_owner)) = resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    owner_lib,
                    found_owner,
                    *mixin,
                ) {
                    if mixin_container.members.is_empty() {
                        self.frozen_list_face_hits(*mixin, mixin_owner, true, &mut hits);
                    }
                    for (member, entry) in &mixin_container.members {
                        if !entry.is_static {
                            hits.push(FaceEntry {
                                name: *member,
                                entry: self.boundary.member(mixin_owner, *mixin, *member, entry),
                                owner_name: *mixin,
                                owner_file: mixin_owner,
                                concrete_source: true,
                            });
                        }
                    }
                    let mixin_owner_lib = self
                        .owner_library_of(mixin_owner, *mixin)
                        .unwrap_or(owner_lib);
                    for mixin_face in mixin_container.with_clause.iter() {
                        applied.push_back((*mixin_face, mixin_owner, mixin_owner_lib, true));
                    }
                    for mixin_face in mixin_container
                        .on_clause
                        .iter()
                        .chain(mixin_container.implements_clause.iter())
                    {
                        applied.push_back((*mixin_face, mixin_owner, mixin_owner_lib, false));
                    }
                } else {
                    self.frozen_list_face_hits(*mixin, found_owner, true, &mut hits);
                }
                while let Some((face, face_owner, face_lib, concrete)) = applied.pop_front()
                {
                    // A clause cycle in the applied faces poisons the whole
                    // face walk — the same suppression the outer `with`
                    // loop keeps.
                    if !seen.insert(face) {
                        return None;
                    }
                    let Some((face_container, face_owner)) = resolve_container(
                        self.graph,
                        self.namespaces,
                        self.index,
                        face_lib,
                        face_owner,
                        face,
                    ) else {
                        self.frozen_list_face_hits(face, face_owner, concrete, &mut hits);
                        continue;
                    };
                    if face_container.members.is_empty() {
                        self.frozen_list_face_hits(face, face_owner, concrete, &mut hits);
                    }
                    for (member, entry) in &face_container.members {
                        if !entry.is_static {
                            hits.push(FaceEntry {
                                name: *member,
                                entry: self.boundary.member(
                                    face_owner,
                                    face,
                                    *member,
                                    entry,
                                ),
                                owner_name: face,
                                owner_file: face_owner,
                                concrete_source: concrete,
                            });
                        }
                    }
                    // The applied chain keeps climbing the face's own
                    // application faces transitively (`A = Object with B`,
                    // `B = Object with M`): each hop's `with` faces join
                    // concrete, its `on`/`implements` join the interface.
                    let next_lib = self
                        .owner_library_of(face_owner, face)
                        .unwrap_or(face_lib);
                    for next in face_container.with_clause.iter() {
                        applied.push_back((*next, face_owner, next_lib, true));
                    }
                    for next in face_container
                        .on_clause
                        .iter()
                        .chain(face_container.implements_clause.iter())
                    {
                        applied.push_back((*next, face_owner, next_lib, false));
                    }
                }
            }
            for interface in &found.implements_clause {
                if let Some((interface_container, interface_owner)) = resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    owner_lib,
                    found_owner,
                    *interface,
                ) {
                    if interface_container.members.is_empty() {
                        self.frozen_list_face_hits(*interface, interface_owner, false, &mut hits);
                    }
                    for (member, entry) in &interface_container.members {
                        if !entry.is_static {
                            hits.push(FaceEntry {
                                name: *member,
                                entry: self.boundary.member(
                                    interface_owner,
                                    *interface,
                                    *member,
                                    entry,
                                ),
                                owner_name: *interface,
                                owner_file: interface_owner,
                                concrete_source: false,
                            });
                        }
                    }
                } else {
                    self.frozen_list_face_hits(*interface, found_owner, false, &mut hits);
                }
            }
            // The chain ends here (Object is unindexed) — enumeration
            // completes cleanly; only a broken named hop suppresses.
            let Some(superclass) = found.superclass else {
                return Some(hits);
            };
            scope_lib = owner_lib;
            name = superclass;
            owner = found_owner;
        }
    }

    /// The frozen collection-base faces a names-only SDK stub cannot
    /// carry, appended to `hits` when `face`'s spelling names
    /// `dart:collection`'s list or set family ([`LIST_BASE_FACE`] /
    /// [`SET_BASE_FACE`]; upstream Dart SDK source,
    /// `sdk/lib/collection/list.dart` — `ListBase`/`ListMixin` — and
    /// `sdk/lib/collection/set.dart` — `SetBase`/`SetMixin`). The SDK
    /// projection is names-only: a hop whose container the index cannot
    /// model — or models member-less — contributes nothing, so a
    /// `with ListMixin<RawRow>` class looked complete while the oracle
    /// still demanded the base's abstract forwarding points (`setter
    /// List.length` on a class declaring the `length` getter, `[]`,
    /// `[]=`). Every entry carries [`Ty::Unknown`] types, so
    /// override/comparison faces over it suppress — only the
    /// completeness enumeration reads the rows. Not a family spelling:
    /// a no-op (the common case; a couple string compares). The concrete
    /// classes of the set family (`HashSet`/`LinkedHashSet`/
    /// `SplayTreeSet`/`UnmodifiableSetView`) carry no frozen face: they
    /// implement every member, so the only faithful rows would be
    /// all-concrete — pure suppression the walk already models by
    /// contributing nothing.
    fn frozen_list_face_hits(
        &self,
        face: StringId,
        owner_file: FileId,
        top_concrete: bool,
        hits: &mut Vec<FaceEntry<'a>>,
    ) {
        let text = self.strings.resolve(face);
        if text == "List"
            || COLLECTION_INTERFACE_ALIASES
                .iter()
                .any(|(alias, targets)| text == *alias && targets.contains(&"List"))
        {
            let list_name = self.intern("List");
            for (member, r#abstract, kind) in LIST_BASE_FACE {
                let entry = MemberEntry {
                    kind: match kind {
                        1 => MemberKind::Setter,
                        2 => MemberKind::Getter,
                        _ => MemberKind::Method,
                    },
                    is_augment: false,
                    is_static: false,
                    is_abstract: *r#abstract,
                    is_external: false,
                    is_final: false,
                    is_const: false,
                    covariant: false,
                    type_params: 0,
                    type_param_names: Box::default(),
                    type_param_bounds: Box::default(),
                    var_ty: Ty::Unknown,
                    return_ty: Ty::Unknown,
                    formals: None,
                    annotations: Box::default(),
                    name_range: MemberEntry::MISSING_NAME_RANGE,
                };
                hits.push(FaceEntry {
                    name: self.intern(member),
                    entry: Projected::Foreign(std::rc::Rc::new(entry)),
                    owner_name: list_name,
                    owner_file,
                    concrete_source: top_concrete,
                });
            }
            return;
        }
        // `SetMixin` is the SDK's own `typedef SetMixin<E> = SetBase<E>`
        // (set.dart:343) — one face, and the abstract rows' owner renders
        // as `SetBase` (the declaration home the oracle's interface map
        // names).
        if text == "SetBase" || text == "SetMixin" {
            let base_name = self.intern("SetBase");
            for (member, r#abstract, kind) in SET_BASE_FACE {
                let entry = MemberEntry {
                    kind: match kind {
                        1 => MemberKind::Setter,
                        2 => MemberKind::Getter,
                        _ => MemberKind::Method,
                    },
                    is_augment: false,
                    is_static: false,
                    is_abstract: *r#abstract,
                    is_external: false,
                    is_final: false,
                    is_const: false,
                    covariant: false,
                    type_params: 0,
                    type_param_names: Box::default(),
                    type_param_bounds: Box::default(),
                    var_ty: Ty::Unknown,
                    return_ty: Ty::Unknown,
                    formals: None,
                    annotations: Box::default(),
                    name_range: MemberEntry::MISSING_NAME_RANGE,
                };
                hits.push(FaceEntry {
                    name: self.intern(member),
                    entry: Projected::Foreign(std::rc::Rc::new(entry)),
                    owner_name: base_name,
                    owner_file,
                    concrete_source: top_concrete,
                });
            }
        }
    }

    /// The same-name super-interface candidate conflicts
    /// (`inheritance_override.dart:160-207` `_reportInterfaceConflicts`).
    /// Candidates accumulate per name in the oracle's face order — the
    /// superclass face (the applied chain), the `with` mixins in
    /// application order, the `implements` interfaces, the `on` constraints
    /// — with a `with` mixin's *own* member replacing the accumulated list
    /// (the official application sequence). A name conflicts when its
    /// candidates mix getter and method kinds — unresolvable, reported even
    /// when the class declares the name (verified against the oracle) — or
    /// the candidates share a kind, no candidate is a valid override of all
    /// the others, and no class-local member resolves the name (the
    /// official `map.containsKey` gate). Returns the conflicted names — a
    /// conflict drops the name from the class's interface (the official
    /// combine returns no signature), so the completeness check must not
    /// report it missing. `None` = a broken face chain — the caller
    /// suppresses.
    pub(crate) fn inheritance_conflicts(
        &mut self,
        decl: &Container,
        name_range: SourceRange,
    ) -> Option<FxHashSet<StringId>> {
        let mut faces: Vec<(StringId, bool, bool)> = Vec::new();
        if let Some(superclass) = decl.superclass {
            faces.push((superclass, false, true));
        }
        // `with`: candidates in application order, own-member replacement.
        faces.extend(decl.with_clause.iter().map(|m| (*m, true, true)));
        // `implements`/`on`: candidates only — never implementations.
        faces.extend(decl.implements_clause.iter().map(|i| (*i, false, false)));
        faces.extend(decl.on_clause.iter().map(|o| (*o, false, false)));
        let mut order: Vec<StringId> = Vec::new();
        let mut candidate_map: FxHashMap<StringId, Vec<FaceEntry<'a>>> = FxHashMap::default();
        for (face, is_with, contributes) in &faces {
            let entries = self.face_entries(*face, *contributes)?;
            // Keep-first per name (the nearest member wins within a chain),
            // name-sorted for a deterministic emission order.
            let mut per_name: Vec<FaceEntry<'a>> = Vec::new();
            let mut present = FxHashSet::default();
            for hit in entries {
                if present.insert(hit.name) {
                    per_name.push(hit);
                }
            }
            per_name.sort_unstable_by_key(|h| h.name);
            for hit in per_name {
                if let Some(list) = candidate_map.get_mut(&hit.name) {
                    if *is_with && hit.owner_name == *face {
                        // The mixin's own member replaces; a kind mismatch in
                        // the replacement is itself the getter/method
                        // conflict (`inheritance_manager3.dart:594-610`).
                        if Self::kind_class(&list[0].entry) != Self::kind_class(&hit.entry) {
                            self.emit_kind_conflict(hit.name, &list[0], &hit, name_range);
                        }
                        *list = vec![hit];
                    } else {
                        list.push(hit);
                    }
                } else {
                    order.push(hit.name);
                    candidate_map.insert(hit.name, vec![hit]);
                }
            }
        }
        // A class-local instance member resolves the same-kind conflict for
        // its name (fields cover their setter half).
        let mut declared: FxHashSet<StringId> = FxHashSet::default();
        for (name, entry) in &decl.members {
            if entry.is_static {
                continue;
            }
            declared.insert(*name);
            if entry.kind == MemberKind::Field && !entry.is_final {
                let setter = self.setter_spelling(*name);
                declared.insert(setter);
            }
        }
        let mut conflicted: FxHashSet<StringId> = FxHashSet::default();
        for name in order {
            let list = &candidate_map[&name];
            if list.len() >= 2 && self.emit_name_conflict(name, list, &declared, name_range) {
                conflicted.insert(name);
            }
        }
        Some(conflicted)
    }

    /// The candidate kind class: getter-shaped (getters and fields — the
    /// interface sees a field as its getter), method, setter.
    pub(crate) fn kind_class(entry: &MemberEntry) -> u8 {
        match entry.kind {
            MemberKind::Method => 0,
            MemberKind::Setter => 1,
            MemberKind::Getter | MemberKind::Field => 2,
        }
    }

    /// One multi-candidate name's conflict verdict, emitted in place.
    /// Returns whether the name is conflicted — dropped from the class's
    /// interface, so the completeness check must not report it missing.
    ///
    /// The getter/method mix has no resolution
    /// (`_checkForGetterMethodConflict`): the first getter-shaped and first
    /// method candidate in face order name the interfaces. A class-local
    /// instance member resolves the same-kind conflict for its name. The
    /// name-level valid-override search then decides: a candidate not broken
    /// against any other is a valid override. An incomparable pair reads as
    /// compatible (`override_broken`'s suppression direction) — the check
    /// never false-positives on types a subclass edge could still relate.
    pub(crate) fn emit_name_conflict(
        &mut self,
        name: StringId,
        list: &[FaceEntry<'a>],
        declared: &FxHashSet<StringId>,
        name_range: SourceRange,
    ) -> bool {
        let getter = list.iter().find(|c| Self::kind_class(&c.entry) == 2);
        let method = list.iter().find(|c| c.entry.kind == MemberKind::Method);
        if let (Some(getter), Some(method)) = (getter, method) {
            self.emit_kind_conflict(name, getter, method, name_range);
            return true;
        }
        if declared.contains(&name) {
            return false;
        }
        let has_valid = list.iter().enumerate().any(|(i, a)| {
            list.iter().enumerate().all(|(j, b)| {
                i == j
                    || !self.override_broken(
                        &a.entry,
                        (a.owner_file, a.owner_name),
                        &b.entry,
                        (b.owner_file, b.owner_name),
                    )
            })
        });
        if has_valid {
            return false;
        }
        let display = self.strings.resolve(name);
        let signatures = list
            .iter()
            .map(|c| {
                format!(
                    "{}.{} ({})",
                    self.strings.resolve(c.owner_name),
                    display,
                    self.override_signature_text(&c.entry)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        self.emit(
            SemanticCode::InconsistentInheritance,
            name_range,
            display,
            &signatures,
        );
        true
    }

    /// The getter/method kind conflict's emission: the member name, the
    /// getter candidate's interface, the method candidate's interface.
    pub(crate) fn emit_kind_conflict(
        &mut self,
        name: StringId,
        getter: &FaceEntry<'a>,
        method: &FaceEntry<'a>,
        name_range: SourceRange,
    ) {
        self.emit3(
            SemanticCode::InconsistentInheritanceGetterAndMethod,
            name_range,
            self.strings.resolve(name),
            self.strings.resolve(getter.owner_name),
            self.strings.resolve(method.owner_name),
        );
    }

    /// The concrete class/enum abstract-completeness check
    /// (`inheritance_override.dart:412-486`): every interface name without
    /// a concrete implementation is a missing row, the five-tier message
    /// rendering the sorted `Enclosing.member` descriptions. The exemptions
    /// mirror the official walk: `Object` members (always implemented), an
    /// enum's `values`/`values=`, names the class itself declares (the
    /// concrete-class-with-abstract-member code owns those — not
    /// implemented here, so they suppress), names a *concrete* direct
    /// superclass's interface carries (`_isNotImplementedInConcreteSuper
    /// Class` — the issue belongs there), cross-library private names
    /// (`isAccessibleFor`), and a concrete `noSuchMethod` on the
    /// implemented sources (the forwarder gate). A name the conflict pass
    /// dropped (`conflicted`) is absent from the class's interface — never
    /// reported missing here. An abstract field's non-final shape misses
    /// both halves (getter and setter); a final one misses the getter only.
    pub(crate) fn abstract_completeness(
        &mut self,
        kind: ContainerKind,
        decl: &Container,
        conflicted: &FxHashSet<StringId>,
        view: &InterfaceView<'a>,
        name_range: SourceRange,
    ) {
        // The concrete-superclass exemption's face: the direct superclass
        // resolved, concrete, and carrying the name in its interface. A
        // broken chain suppresses the whole check.
        let concrete_super_names: Option<FxHashSet<StringId>> = match decl.superclass {
            Some(superclass) => {
                match resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    superclass,
                ) {
                    // A dead superclass edge is the oracle's `Object`
                    // fallback — no names to exempt, no suppression: the
                    // `implements`/`with` faces' abstract names still
                    // report (the `extends Missing` arm, diagnostic.g.dart's
                    // `non_abstract_class_inherits_abstract_member` over an
                    // unresolvable super).
                    None => None,
                    Some((super_container, _)) => {
                        // A sealed superclass is abstract at the element
                        // level (element.dart:150): its interface names
                        // stay the concrete subclass's to implement — no
                        // exemption.
                        if super_container.kind == ContainerKind::Class
                            && (super_container.is_abstract || super_container.sealed)
                        {
                            None
                        } else {
                            let Some(hits) = self.face_entries(superclass, true) else {
                                return;
                            };
                            Some(hits.into_iter().map(|h| h.name).collect())
                        }
                    }
                }
            }
            None => None,
        };
        // Own instance members block the report for their names: a concrete
        // one implements them, an abstract one is the
        // `concrete_class_with_abstract_member` report's business (not
        // implemented here, so it suppresses — never a false positive).
        let mut implemented: FxHashSet<StringId> = FxHashSet::default();
        let mut blocked: FxHashSet<StringId> = FxHashSet::default();
        for (name, entry) in &decl.members {
            if entry.is_static {
                continue;
            }
            blocked.insert(*name);
            if entry.kind == MemberKind::Field && !entry.is_final {
                let setter = self.setter_spelling(*name);
                blocked.insert(setter);
                if !entry.is_abstract {
                    implemented.insert(setter);
                }
            }
            if !entry.is_abstract {
                implemented.insert(*name);
            }
        }
        if view.no_such_method_gate {
            return;
        }
        implemented.extend(view.implemented.iter().copied());
        // An enum's synthesized `values` pair is always implemented.
        let values = self.members.enum_synth_members[0];
        let values_setter = self.setter_spelling(values);
        let descriptions: Vec<String> = view
            .names
            .iter()
            .filter(|name| !blocked.contains(name))
            .filter(|name| !self.members.object_members.contains(name))
            .filter(|name| {
                kind != ContainerKind::Enum || (**name != values && **name != values_setter)
            })
            .filter(|name| {
                concrete_super_names
                    .as_ref()
                    .is_none_or(|names| !names.contains(name))
            })
            .filter(|name| !conflicted.contains(name))
            .filter(|name| !implemented.contains(name))
            .filter(|name| {
                // `isAccessibleFor`: a *private* name inherited from another
                // library can never be implemented — the oracle skips it.
                // Public names are accessible from any importing library.
                if !self.strings.resolve(**name).starts_with('_') {
                    return true;
                }
                let hit = &view.representative[name];
                hit.owner_file == self.file
                    || self.owner_library_of(hit.owner_file, **name) == Some(self.lib)
            })
            .map(|name| self.completeness_description(view, *name))
            .collect();
        self.emit_missing_members(descriptions, name_range);
    }

    /// The completeness report's named member: the interface map's
    /// MOST-SPECIFIC hit for the name — the oracle reads
    /// `interface.map[name]`, the `InheritanceManager3` selection over every
    /// face's candidates (`inheritance_override.dart:426-451`, then
    /// `element.enclosingElement.displayName` at `:970-975`). The first-face
    /// representative is NOT that selection: `class S extends SetBase<int>
    /// with SetMixin<int>` must name `SetBase.contains` (the superclass's own
    /// abstract redeclaration) even though the mixin's face carries the
    /// `Set.contains` interface row first. A name whose candidates do not
    /// linearize (the conflict shapes) keeps the representative.
    fn completeness_description(&self, view: &InterfaceView<'a>, name: StringId) -> String {
        let candidates: Vec<&FaceEntry<'a>> = view
            .face_hits
            .iter()
            .flatten()
            .filter(|hit| hit.name == name)
            .collect();
        let chosen = self
            .most_specific(&candidates)
            .unwrap_or_else(|| &view.representative[&name]);
        self.member_description(chosen, name)
    }

    /// The interface view for the completeness check: every resolved face's
    /// names (a non-final field carries its setter half), the first face in
    /// order supplying the representative hit, and the names a concrete
    /// source implements. `None` suppresses the whole check — a broken face
    /// chain. The noSuchMethod forwarder gate — a concrete noSuchMethod on
    /// the implemented sources (own members, mixins, the super chain — never
    /// an `implements` face) implements every interface name — rides the
    /// view as a flag: the completeness check returns on it while the
    /// concrete-implementation loop ignores it, matching the oracle's
    /// dispatch order.
    pub(crate) fn collect_interface(&mut self, decl: &Container) -> Option<InterfaceView<'a>> {
        let mut faces: Vec<(StringId, bool)> = Vec::new();
        faces.extend(decl.superclass.iter().map(|s| (*s, true)));
        faces.extend(decl.with_clause.iter().map(|m| (*m, true)));
        faces.extend(decl.implements_clause.iter().map(|i| (*i, false)));
        faces.extend(decl.on_clause.iter().map(|o| (*o, false)));
        let mut face_hits: Vec<Vec<FaceEntry<'a>>> = Vec::with_capacity(faces.len());
        let mut face_sources: Vec<StringId> = Vec::with_capacity(faces.len());
        for (face, contributes) in faces {
            face_sources.push(face);
            face_hits.push(self.face_entries(face, contributes)?);
        }
        let no_such_method = self.members.object_members[3];
        let no_such_method_gate = decl
            .members
            .get(&no_such_method)
            .is_some_and(|e| !e.is_static && !e.is_abstract)
            || face_hits
                .iter()
                .flatten()
                .any(|h| h.name == no_such_method && h.concrete_source && !h.entry.is_abstract);
        let mut representative: FxHashMap<StringId, FaceEntry<'a>> = FxHashMap::default();
        let mut names: Vec<StringId> = Vec::new();
        let mut implemented: FxHashSet<StringId> = FxHashSet::default();
        for hits in &face_hits {
            let mut sorted = hits.clone();
            sorted.sort_unstable_by_key(|h| h.name);
            for hit in sorted {
                let name = hit.name;
                let setter_half = match hit.entry.kind {
                    MemberKind::Field if !hit.entry.is_final => Some(self.setter_spelling(name)),
                    _ => None,
                };
                if hit.concrete_source && !hit.entry.is_abstract {
                    implemented.insert(name);
                    if let Some(setter) = setter_half {
                        implemented.insert(setter);
                    }
                }
                if let std::collections::hash_map::Entry::Vacant(slot) = representative.entry(name)
                {
                    slot.insert(hit.clone());
                    names.push(name);
                }
                if let Some(setter) = setter_half
                    && let std::collections::hash_map::Entry::Vacant(slot) =
                        representative.entry(setter)
                {
                    slot.insert(hit);
                    names.push(setter);
                }
            }
        }
        Some(InterfaceView {
            names,
            representative,
            implemented,
            face_hits,
            face_sources,
            no_such_method_gate,
        })
    }

    /// The interface loop's concrete-implementation face
    /// (`inheritance_override.dart:412-481`): for every interface name the
    /// `implemented` map resolves — the class's own concrete member first,
    /// then the first concrete hit over the `with` mixins and the
    /// superclass — that member must be a valid override of the interface
    /// map's most-specific member of the same name. The own-member source
    /// is not optional: it *shadows* the inherited concrete member in the
    /// official `implemented` map (`_addImplemented`), so a class declaring
    /// its own `substitute` is compared by its own signature — the
    /// inherited walk alone fabricates comparisons the oracle never runs
    /// (`GetterElementImpl`'s own valid substitute read as
    /// `ExecutableElementImpl`'s). Own-member shapes the oracle
    /// double-reports (its TODO,
    /// `inheritance_override.dart:463-469`) stay double-reported here —
    /// the duplication is upstream behavior. When no candidate validly
    /// overrides all the others (the conflict shapes) the pass skips — the
    /// conflict diagnostics own those. Emissions anchor at the container's
    /// name (the oracle's `classNameToken`, whose placement its own TODO
    /// calls out). Kind mismatches skip (the oracle routes them to the
    /// member-kind verifier), and a field entry never carries the setter
    /// face (its getter-shaped type would compare with the wrong variance).
    pub(crate) fn implementation_overrides(
        &mut self,
        container: StringId,
        decl: &'a Container,
        view: &InterfaceView<'a>,
        name_range: SourceRange,
    ) {
        for name in &view.names {
            let setter_face = self.strings.resolve(*name).ends_with('=');
            let mut hits: Vec<&FaceEntry<'a>> = view
                .face_hits
                .iter()
                .flatten()
                .filter(|h| h.name == *name)
                .collect();
            if setter_face {
                let bare = self
                    .strings
                    .intern(self.strings.resolve(*name).strip_suffix('=').unwrap_or(""));
                hits.extend(view.face_hits.iter().flatten().filter(|h| {
                    h.name == bare && h.entry.kind == MemberKind::Field && !h.entry.is_final
                }));
            }
            // The oracle's interface map seats the own declared member first
            // (`_getTypeMembers`: every non-static member, abstract included —
            // `inheritance_manager3.dart:1336-1352`), and named candidates
            // never replace a declared name (`map.containsKey(name) →
            // continue`, `inheritance_manager3.dart:499`). With an own member
            // declared, `interface.map[name]` is that member itself — comparing
            // the concrete implementation against a super-face hit would
            // fabricate a check the oracle never runs (its `concreteElement`
            // is then the same entry, trivially valid).
            let own_declared = self.own_declared_member(container, decl, *name, setter_face);
            let specific_owned;
            let specific = if let Some(own) = own_declared {
                specific_owned = own;
                &specific_owned
            } else {
                let Some(specific) = self.interface_map_member(decl, view, *name, &hits) else {
                    continue;
                };
                specific
            };
            let Some(concrete) = self
                .own_concrete_implementation(container, decl, *name, setter_face)
                .or_else(|| Self::inherited_concrete_implementation(decl, view, *name))
            else {
                continue;
            };
            if concrete.entry.kind != specific.entry.kind {
                continue;
            }
            if setter_face && concrete.entry.kind == MemberKind::Field {
                continue;
            }
            if concrete.entry.ptr() == specific.entry.ptr() {
                continue;
            }
            // The free-parameter suppression gate: either compared signature
            // carrying its declaring container's type parameters is the
            // unsubstituted read — the element model strips clause type
            // arguments, and this loop tracks no edge chain, so the
            // substitution environment is undecidable here (a foreign hop's
            // clauses live in another file's tokens by design). Comparing
            // raw false-breaks on the free heads (the covariant result
            // face's Left-Nullable arm, the dead-edge arm — the
            // `implements TypeVisitor<R>` family), so the comparison skips;
            // the under-report posture every other arm keeps. Upgrade point
            // (the interface-completeness analog of the single-member
            // `check_override` substitution face): composing the per-edge
            // environment here — the `instantiate_inherited` machinery over
            // a chain this loop would have to track — replaces the blanket
            // skip with the substituted comparison.
            if !self
                .super_free_params(&concrete.entry, concrete.owner_file)
                .is_empty()
                || !self
                    .super_free_params(&specific.entry, specific.owner_file)
                    .is_empty()
            {
                continue;
            }
            if !self.override_broken(
                &concrete.entry,
                (concrete.owner_file, concrete.owner_name),
                &specific.entry,
                (specific.owner_file, specific.owner_name),
            ) {
                continue;
            }
            let display = self.strings.resolve(*name);
            let display = self.member_display_name(display);
            let concrete_text = self.strings.resolve(concrete.owner_name);
            let concrete_sig = self.override_signature_text(&concrete.entry);
            let specific_text = self.strings.resolve(specific.owner_name);
            let specific_sig = self.override_signature_text(&specific.entry);
            let code = if concrete.entry.kind == MemberKind::Setter {
                SemanticCode::InvalidImplementationOverrideSetter
            } else {
                SemanticCode::InvalidImplementationOverride
            };
            self.emit5(
                code,
                name_range,
                display,
                concrete_text,
                &concrete_sig,
                specific_text,
                &specific_sig,
            );
        }
    }

    /// The name's `interface.map` member — the oracle's per-name
    /// `namedCandidates` assembly (`inheritance_manager3.dart:547-695`
    /// `_getInterfaceClass`): the superclass face seeds the running
    /// candidate, each `with` mixin's OWN member replaces it
    /// (`candidate.enclosingElement == mixinElement`), an
    /// inherited-through mixin member merges against the running candidate
    /// via the most-specific arm, and the `implements`/`on` faces append —
    /// the final name's signature is the most specific of the accumulated
    /// list, `None` when the candidates do not linearize (conflict
    /// territory). Each face's own contribution is its first hit of the
    /// name: upstream seats ONE member per name per face (`Interface.map`),
    /// whose own collapse already ran the same algorithm on the face's
    /// element — the linearized walk's first same-name hit is that seated
    /// member (the declaring hop walks before the faces it shadows).
    fn interface_map_member<'e>(
        &self,
        decl: &Container,
        view: &'e InterfaceView<'a>,
        name: StringId,
        hits: &[&'e FaceEntry<'a>],
    ) -> Option<&'e FaceEntry<'a>> {
        // Per-face seated member of `name`: the first hit each face
        // contributes, keyed by its clause spelling (`face_sources` rides
        // the same order `collect_interface` pushed them — superclass,
        // `with` in application order, `implements`, `on`).
        let mut seated: FxHashMap<StringId, &FaceEntry<'a>> = FxHashMap::default();
        for (face_hits, face) in view.face_hits.iter().zip(view.face_sources.iter()) {
            if let Some(hit) = face_hits.iter().find(|h| h.name == name) {
                seated.insert(*face, hit);
            }
        }
        // The running candidate: the superclass face's seated member first
        // (upstream seeds `namedCandidates` from the supertype interface's
        // map), then the `with` mixin faces in application order.
        let mut running: Option<&FaceEntry<'a>> = decl.superclass.and_then(|s| seated.get(&s).copied());
        for mixin in decl.with_clause.iter() {
            let Some(&mixin_hit) = seated.get(mixin) else {
                continue;
            };
            if mixin_hit.owner_name == *mixin || running.is_none() {
                // A mixin's own declared member always replaces the running
                // candidate (`candidate.enclosingElement == mixinElement`,
                // `inheritance_manager3.dart:605-613`); with no running
                // candidate the mixin's seated member starts the list.
                running = Some(mixin_hit);
            } else if let Some(current) = running {
                // An inherited-through mixin member merges against the
                // running candidate (`candidatesFromSuperAndMixin` — the
                // more specific wins; an unresolvable pair keeps the
                // running candidate, upstream's failed merge leaving
                // `namedCandidates[name]` untouched).
                running = self.most_specific(&[current, mixin_hit]).or(Some(current));
            }
        }
        // `implements`/`on` faces append their seated members — the final
        // `_findMostSpecificFromNamedCandidates` answers the name's
        // signature over the whole accumulated list.
        let mut candidates: Vec<&FaceEntry<'a>> = Vec::with_capacity(seated.len() + 1);
        if let Some(hit) = running {
            candidates.push(hit);
        }
        for face in decl.implements_clause.iter().chain(decl.on_clause.iter()) {
            if let Some(&hit) = seated.get(face) {
                candidates.push(hit);
            }
        }
        // The `name=` setter query folds bare-spelling field hits in (the
        // caller's `hits` extension): each field's first hit per face joins
        // its seated candidates — a face whose `x=` spelling answered
        // nothing keeps its `x` field's induced setter as the candidate.
        if self.strings.resolve(name).ends_with('=') {
            for hit in hits {
                if hit.entry.kind == MemberKind::Field
                    && !candidates.iter().any(|c| std::ptr::eq(*c, *hit))
                {
                    candidates.push(hit);
                }
            }
        }
        self.most_specific(&candidates)
    }

    /// The name's most-specific interface member: the unique candidate that
    /// validly overrides every other hit (the oracle's interface-map
    /// selection at name level). `None` when the candidates do not
    /// linearize — conflict territory.
    pub(crate) fn most_specific<'e, 'm>(
        &self,
        hits: &[&'e FaceEntry<'m>],
    ) -> Option<&'e FaceEntry<'m>> {
        hits.iter().copied().find(|candidate| {
            hits.iter().all(|other| {
                std::ptr::eq(*candidate, *other)
                    || !self.override_broken(
                        &candidate.entry,
                        (candidate.owner_file, candidate.owner_name),
                        &other.entry,
                        (other.owner_file, other.owner_name),
                    )
            })
        })
    }

    /// The class's own declared member of `name` — the oracle's
    /// `interface.map` seat (`_getTypeMembers`,
    /// `inheritance_manager3.dart:1336-1352`): every non-static member joins
    /// `declared`, abstract members included, and a non-final field's
    /// synthesized setter fills the `name=` slot. Unlike
    /// [`Self::own_concrete_implementation`] abstractness does not disqualify
    /// the seat — an abstract own member still occupies the interface map
    /// (the implemented map then falls through to the inherited sources).
    /// `None` = no own member declares the name; the face candidates decide.
    fn own_declared_member(
        &self,
        container: StringId,
        decl: &'a Container,
        name: StringId,
        setter_face: bool,
    ) -> Option<FaceEntry<'a>> {
        let declared = |e: &MemberEntry| !e.is_static;
        let entry = decl
            .members
            .get(&name)
            .filter(|e| declared(*e))
            .or_else(|| {
                if !setter_face {
                    return None;
                }
                let bare = self
                    .strings
                    .intern(self.strings.resolve(name).strip_suffix('=')?);
                decl.members
                    .get(&bare)
                    .filter(|e| declared(*e) && e.kind == MemberKind::Field && !e.is_final)
            })?;
        Some(FaceEntry {
            name,
            entry: self.boundary.member(self.file, container, name, entry),
            owner_name: container,
            owner_file: self.file,
            concrete_source: true,
        })
    }

    /// The class's own concrete member of `name` — `_addImplemented`'s first
    /// source (upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/dart/element/inheritance_manager3.dart:318-338`
    /// — own methods/getters/setters, non-abstract and non-static, enter the
    /// `implemented` map before any mixin or super face, each level
    /// replacing the last). A non-final field carries the setter half (the
    /// implicit synthetic setter the element model lists under `setters`)
    /// and the getter face accepts a field's implicit getter. `None` = no
    /// own concrete member; the inherited walk decides.
    fn own_concrete_implementation(
        &self,
        container: StringId,
        decl: &'a Container,
        name: StringId,
        setter_face: bool,
    ) -> Option<FaceEntry<'a>> {
        let matches = |e: &MemberEntry| {
            !e.is_static
                && !e.is_abstract
                && if setter_face {
                    e.kind == MemberKind::Setter || (e.kind == MemberKind::Field && !e.is_final)
                } else {
                    // `_addImplemented` adds the own methods/getters/setters
                    // (`inheritance_manager3.dart:333-351`), so an own
                    // Method implements the `name` face too — skipping it
                    // lets the inherited member claim the implemented seat
                    // and fabricates a comparison against the own declared
                    // signature the oracle never runs.
                    e.kind == MemberKind::Method
                        || e.kind == MemberKind::Getter
                        || e.kind == MemberKind::Field
                }
        };
        let entry = decl
            .members
            .get(&name)
            .filter(|e| matches(*e))
            .or_else(|| {
                if !setter_face {
                    return None;
                }
                let bare = self
                    .strings
                    .intern(self.strings.resolve(name).strip_suffix('=')?);
                decl.members.get(&bare).filter(|e| matches(*e))
            })?;
        Some(FaceEntry {
            name,
            entry: self.boundary.member(self.file, container, name, entry),
            owner_name: container,
            owner_file: self.file,
            concrete_source: true,
        })
    }

    /// The name's inherited concrete implementation, `_addImplemented`'s
    /// last-writer wins: the superclass interface's `implemented` map seats
    /// first, each `with` mixin's implemented members then overwrite in
    /// application order — the LAST mixin carrying the name wins the seat
    /// (`inheritance_manager3.dart:333-374` `_addImplemented` /
    /// `_addMixinMembers`; never an `implements` face — its hits are
    /// interface members only). Within a concrete face only
    /// `concrete_source` hits count: a member reached through the face's own
    /// `implements` clause is an interface member, not an implementation the
    /// face contributes (`C extends R with M implements I` takes `M`'s and
    /// `R`'s chain's members, never `I`'s or `M`'s interfaces'). The class's
    /// own members are deliberately absent: the per-member
    /// `invalid_override` pass owns their faces.
    pub(crate) fn inherited_concrete_implementation(
        decl: &Container,
        view: &InterfaceView<'a>,
        name: StringId,
    ) -> Option<FaceEntry<'a>> {
        let hits_of = |face: StringId| {
            view.face_sources
                .iter()
                .position(|f| *f == face)
                .and_then(|i| view.face_hits.get(i))
        };
        // `_addImplemented` seats the superclass interface's implemented map
        // first, then each `with` mixin's implemented members OVERWRITE the
        // name in application order (`_addMixinMembers`,
        // `inheritance_manager3.dart:354-374` — the applied `S&M` class
        // chain means the LAST mixin declaring the name wins the concrete
        // seat). The read order here is the last-writer: `with` mixins in
        // reverse application order, then the superclass face.
        for face in decl
            .with_clause
            .iter()
            .rev()
            .copied()
            .chain(decl.superclass)
        {
            if let Some(hit) = hits_of(face).and_then(|hits| {
                hits.iter()
                    .find(|h| h.name == name && h.concrete_source && !h.entry.is_abstract)
            }) {
                return Some(hit.clone());
            }
        }
        None
    }

    /// The `@mustBeOverridden` walk (`inheritance_override.dart:1050-1168`):
    /// a concrete class/enum must override every `@mustBeOverridden`-annotated
    /// super-interface member with a concrete member of its own, per face
    /// kind — a getter face is satisfied by an own getter or field, a setter
    /// face by an own setter or non-final field, a field face by both halves.
    /// The report groups the missing names into the three registry tiers at
    /// the container's name (`classNameToken`); the oracle's
    /// insertion-ordered set becomes a sorted display (the completeness
    /// pass's precedent). A concrete own `noSuchMethod` gates the whole walk
    /// (own members only — stricter than the completeness forwarder gate).
    /// Annotations resolve from this file's collected declaration sites —
    /// cross-file or later-declared ancestors under-report.
    pub(crate) fn must_be_overridden(
        &mut self,
        decl: &Container,
        view: &InterfaceView<'a>,
        name_range: SourceRange,
    ) {
        let no_such_method = self.members.object_members[3];
        if let Some(entry) = decl.members.get(&no_such_method)
            && !entry.is_static
            && !entry.is_abstract
        {
            return;
        }
        let mut missing: Vec<StringId> = Vec::new();
        for hit in view.face_hits.iter().flatten() {
            if hit.owner_file != self.file
                || !self.site_annotation(hit.owner_name, hit.name, "mustBeOverridden")
                || missing.contains(&hit.name)
            {
                continue;
            }
            let bare = self.strings.intern(
                self.strings
                    .resolve(hit.name)
                    .strip_suffix('=')
                    .unwrap_or(""),
            );
            let concrete_own = |e: &MemberEntry| {
                matches!(
                    e.kind,
                    MemberKind::Getter | MemberKind::Field | MemberKind::Method
                ) && !e.is_abstract
            };
            let satisfied = match hit.entry.kind {
                MemberKind::Method => decl
                    .members
                    .get(&hit.name)
                    .is_some_and(|e| e.kind == MemberKind::Method && !e.is_abstract),
                MemberKind::Getter => decl.members.get(&hit.name).is_some_and(concrete_own),
                MemberKind::Setter => {
                    decl.members
                        .get(&hit.name)
                        .is_some_and(|e| e.kind == MemberKind::Setter && !e.is_abstract)
                        || decl.members.get(&bare).is_some_and(|e| {
                            e.kind == MemberKind::Field && !e.is_final && !e.is_abstract
                        })
                }
                MemberKind::Field => {
                    let getter_ok = decl.members.get(&hit.name).is_some_and(concrete_own);
                    let setter_ok = hit.entry.is_final
                        || decl
                            .members
                            .get(&self.setter_spelling(hit.name))
                            .is_some_and(|e| e.kind == MemberKind::Setter && !e.is_abstract)
                        || decl.members.get(&hit.name).is_some_and(|e| {
                            e.kind == MemberKind::Field && !e.is_final && !e.is_abstract
                        });
                    getter_ok && setter_ok
                }
            };
            if !satisfied {
                missing.push(hit.name);
            }
        }
        let mut displays: Vec<String> = missing
            .iter()
            .map(|n| {
                let text = self.strings.resolve(*n);
                text.strip_suffix('=').unwrap_or(text).to_string()
            })
            .collect();
        displays.sort();
        displays.dedup();
        let code = match displays.len() {
            0 => return,
            1 => SemanticCode::MissingOverrideOfMustBeOverriddenOne,
            2 => SemanticCode::MissingOverrideOfMustBeOverriddenTwo,
            _ => SemanticCode::MissingOverrideOfMustBeOverriddenThreePlus,
        };
        match displays.len() {
            1 => self.emit(code, name_range, &displays[0], ""),
            2 => self.emit3(code, name_range, &displays[0], &displays[1], ""),
            _ => {
                let extra = (displays.len() - 2).to_string();
                self.emit3(code, name_range, &displays[0], &displays[1], &extra);
            }
        }
    }

    /// The description text of one missing member: `Enclosing.member`, the
    /// official `setter ` prefix for setter spellings and none for methods.
    pub(crate) fn member_description(&self, hit: &FaceEntry<'_>, name: StringId) -> String {
        let owner = self.strings.resolve(hit.owner_name);
        let text = self.strings.resolve(name);
        if let Some(base) = text.strip_suffix('=') {
            format!("setter {owner}.{base}")
        } else if hit.entry.kind == MemberKind::Method {
            format!("{owner}.{text}")
        } else {
            format!("getter {owner}.{text}")
        }
    }

    /// The five-tier missing-implementation row: sorted descriptions select
    /// the official message shape.
    pub(crate) fn emit_missing_members(
        &mut self,
        mut descriptions: Vec<String>,
        name_range: SourceRange,
    ) {
        descriptions.sort();
        let code = match descriptions.len() {
            0 => return,
            1 => SemanticCode::InheritsAbstractMemberOne,
            2 => SemanticCode::InheritsAbstractMemberTwo,
            3 => SemanticCode::InheritsAbstractMemberThree,
            4 => SemanticCode::InheritsAbstractMemberFour,
            _ => SemanticCode::InheritsAbstractMemberFivePlus,
        };
        let message = match descriptions.len() {
            1 => code.message(&descriptions[0], ""),
            2 => code.message3(&descriptions[0], &descriptions[1], ""),
            3 => code.message3(&descriptions[0], &descriptions[1], &descriptions[2]),
            4 => code.message4(
                &descriptions[0],
                &descriptions[1],
                &descriptions[2],
                &descriptions[3],
            ),
            n => code.message5(
                &descriptions[0],
                &descriptions[1],
                &descriptions[2],
                &descriptions[3],
                &(n - 4).to_string(),
            ),
        };
        self.report.diagnostics.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            message,
            self.file,
            name_range,
            None,
        ));
    }

    /// The nearest non-static `spelling` member on `face`'s linearized
    /// chain: the face container itself, then its `with` mixins in reverse
    /// application order (the official superclass face of
    /// `class C extends S with M1, M2` is the applied `S&M2` interface —
    /// `inheritance_override.dart:344-351` — so the climb sees M2 over M1
    /// over S), then direct-superclass name edges (each hop resolved
    /// file-locally first, then through the declaring container's library,
    /// #491 S2a; the visited set guards cycles). The third slot is the
    /// defining file — the hit's own declaration-site anchor face (the
    /// `invalid_override` context row resolves through it, cross-file).
    /// `writer` is the container whose supertype clauses wrote `face` (the
    /// prefixed-clause probe's key). `None` = an unresolvable or cyclic
    /// hop — the caller suppresses rather than reports.
    pub(crate) fn face_member(
        &self,
        writer: StringId,
        face: StringId,
        spelling: StringId,
    ) -> Option<(Projected<'a, MemberEntry>, StringId, FileId)> {
        self.face_member_tracked(writer, face, spelling)
            .map(|(entry, name, owner, _)| (entry, name, owner))
    }

    /// [`Self::face_member`] with the climb's hop chain (visited order, the
    /// found hop last): the `check_override` substitution composes the
    /// super-face type-argument environment along it (each edge's written
    /// `extends`/`with`/`implements` arguments instantiate the next hop's
    /// parameters, `Substitution.fromInterfaceType`'s walk).
    pub(crate) fn face_member_tracked(
        &self,
        writer: StringId,
        face: StringId,
        spelling: StringId,
    ) -> Option<(
        Projected<'a, MemberEntry>,
        StringId,
        FileId,
        Vec<(StringId, FileId)>,
    )> {
        let mut name = face;
        let mut hop_writer = writer;
        let mut owner = self.file;
        let mut scope_lib = self.lib;
        let mut seen = FxHashSet::default();
        let mut chain: Vec<(StringId, FileId)> = Vec::new();
        // The `implements`/`on` faces a resolved hop leaves unclimbed — the
        // linear superclass chain handles `extends`+`with`; when it dead-ends
        // (no superclass), the interface faces keep the climb alive in
        // declaration order (the oracle's `getOverridden` covers every
        // superinterface, `UnmodifiableSetMixin implements Set` reaching
        // `Set`'s frozen/interface members). Each entry carries the writer
        // hop's library: a queued face's spelling is written in the writer's
        // file and resolves through that library, whatever other hops the
        // drain climbs in between (#2313).
        let mut pending: VecDeque<(StringId, FileId, LibraryId, StringId)> = VecDeque::new();
        loop {
            if !seen.insert(name) {
                return None;
            }
            let (found, found_owner) =
                match self.resolve_clause_face(hop_writer, owner, scope_lib, name) {
                    Some(pair) => pair,
                    None => {
                        let Some((next, next_owner, next_lib, next_writer)) = pending.pop_front()
                        else {
                            return None;
                        };
                        name = next;
                        owner = next_owner;
                        scope_lib = next_lib;
                        hop_writer = next_writer;
                        continue;
                    }
                };
            chain.push((name, found_owner));
            if let Some(entry) = found.members.get(&spelling)
                && !entry.is_static
                && self.member_visible_to(found_owner, name, spelling, scope_lib)
            {
                // F2: the inherited entry belongs to `found_owner`'s pool —
                // project it before the override faces render or compare it.
                return Some((
                    self.boundary.member(found_owner, name, spelling, entry),
                    name,
                    found_owner,
                    chain,
                ));
            }
            // The mixin/superclass names are written in `found_owner` and
            // resolve through that file's library (probed by the container
            // name it declares, #491 S2a).
            let owner_lib = self
                .owner_library_of(found_owner, name)
                .unwrap_or(scope_lib);
            for mixin in found.with_clause.iter().rev() {
                if !seen.insert(*mixin) {
                    return None;
                }
                let mixin_lib = owner_lib;
                if let Some((mixin_container, mixin_owner)) =
                    self.resolve_clause_face(name, owner, mixin_lib, *mixin)
                {
                    chain.push((*mixin, mixin_owner));
                    if let Some(entry) = mixin_container.members.get(&spelling)
                        && !entry.is_static
                        && self.member_visible_to(mixin_owner, *mixin, spelling, mixin_lib)
                    {
                        return Some((
                            self.boundary.member(mixin_owner, *mixin, spelling, entry),
                            *mixin,
                            mixin_owner,
                            chain,
                        ));
                    }
                    // An applied mixin CLASS keeps its own `with` faces on
                    // the interface (`with A<...>` where `mixin class A =
                    // Object with M<...>` exposes M's members — the same
                    // named-application rule the member walk's mixin hop
                    // applies); the superclass stays out because an applied
                    // mixin's superclass is always `Object`. A plain mixin
                    // contributes nothing here.
                    let mixin_owner_lib = self
                        .owner_library_of(mixin_owner, *mixin)
                        .unwrap_or(mixin_lib);
                    for mixin_face in mixin_container.with_clause.iter() {
                        pending.push_back((*mixin_face, mixin_owner, mixin_owner_lib, *mixin));
                    }
                }
            }
            // The interface faces this hop does not climb inline: `implements`
            // and the mixin `on` constraints join the pending queue — a face
            // whose `extends`/`with` chain ends still contributes its
            // interfaces' members.
            for super_face in found.implements_clause.iter().chain(found.on_clause.iter()) {
                pending.push_back((*super_face, found_owner, owner_lib, name));
            }
            match found.superclass {
                Some(superclass) => {
                    scope_lib = owner_lib;
                    hop_writer = name;
                    name = superclass;
                    owner = found_owner;
                }
                None => {
                    let Some((next, next_owner, next_lib, next_writer)) = pending.pop_front()
                    else {
                        return None;
                    };
                    name = next;
                    owner = next_owner;
                    scope_lib = next_lib;
                    hop_writer = next_writer;
                }
            }
        }
    }

    /// The override faces' resolution: [`Self::face_member_tracked`]'s
    /// climb reshaped to answer the oracle's per-face merged witness
    /// (upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/dart/element/inheritance_manager3.dart` —
    /// a face's interface map carries ONE most-specific signature per
    /// name, built by `_findMostSpecificFromNamedCandidates` over the
    /// candidates every direct super face contributes).
    ///
    /// The climb collects every branch's first hit instead of stopping at
    /// the first one: the linear `extends`/`with` spine's hit plus each
    /// pending `implements`/`on` face's hit. The collection then mirrors
    /// `_getValidOverrides` + `_topMerge`'s decision order
    /// ([`Engine::merge_candidates`]): the candidates that are a valid
    /// override of every other survive; one survivor, or survivors whose
    /// rendered signatures agree, make the first of them the witness; a
    /// differing set is merged type-by-type, landing on whichever candidate
    /// spells the merged signature — or on a synthesized one carrying it
    /// when none does. When no candidate survives, the candidates conflict
    /// and the interface map carries no name — `getMember3` answers null and
    /// the face reports nothing (test_33's `I: int`/`J: num` setter pair:
    /// the `num` setter is the subtype, so the diagnostic anchors at `J`,
    /// not at the first-declared `I`).
    ///
    /// Two synthesis arms the climb owns beside the model lookup:
    /// - a setter spelling resolves an inherited non-final FIELD through
    ///   the field's induced setter (one required positional of the
    ///   field's type — the oracle's `field.setter` element; `A { int x }`
    ///   answers `x=` exactly like a written `set x(int)`), and
    /// - an `Object`/`Enum` hop — the implicit supertype a class, enum, or
    ///   `on`-less mixin carries (`element.supertype` /
    ///   `superclassConstraints`) — answers from the frozen top-face
    ///   signatures ([`Engine::frozen_top_member`]).
    fn override_face_member(
        &self,
        container: StringId,
        face: StringId,
        spelling: StringId,
        facts: &mut Option<HeaderClauses>,
    ) -> Option<FaceHit<'a>> {
        self.face_member_in(
            self.file, container, self.lib, container, face, spelling, facts,
        )
    }

    /// The merged interface signature of `spelling` as `container` sees it
    /// — the oracle's interface-map entry (`inheritance_manager3.dart`'s
    /// `_getValidOverrides` + `_topMerge` through
    /// [`Self::face_member_in`]'s full direct-supertype closure): `D0
    /// extends A<dynamic> implements B<Object?>` reads `member`'s return as
    /// `Object?`, never the first-found `dynamic` the `extends` spine alone
    /// would answer. This is the RESULT-typing face only — existence and
    /// miss verdicts keep [`Self::walk_members`]'s first-found posture. An
    /// empty or conflicting candidate set, an undecidable substitution
    /// environment, or a clause capture outside this file's scanner all
    /// answer `None` (the under-report direction the single-hit callers
    /// already keep).
    pub(crate) fn merged_member_entry(
        &self,
        container: StringId,
        spelling: StringId,
    ) -> Option<MemberEntry> {
        let mut facts: Option<HeaderClauses> = None;
        let hit = self.face_member_in(
            self.file,
            container,
            self.lib,
            container,
            container,
            spelling,
            &mut facts,
        )?;
        // The hit carries the merged candidate's RAW signature (`T
        // member()` on `B<T>`) — the interface-map read the caller wants is
        // the substituted one (`Object? member()` under `implements
        // B<Object?>`), exactly [`Self::combined_inherited`]'s arm; a
        // synthesized or undecidable hit keeps its own entry verbatim.
        Some(
            self.substituted_signature(&hit, container, &mut facts)
                .unwrap_or_else(|| hit.entry().clone()),
        )
    }

    /// [`Self::override_face_member`] rooted at an arbitrary container:
    /// `(root_file, root_writer, root_lib)` is the file / declaring
    /// container / scope library whose written clause heads the climb —
    /// `check_override`'s diagnosing container for the public face, a
    /// foreign declaring hop for the covariance-propagation recursion
    /// ([`Engine::effective_covariant_marks`]). `merge_container` names the
    /// container whose same-file header the candidate merge substitutes
    /// through; a foreign root passes a spelling no header declares, so the
    /// merge keeps its unsubstituted first-candidate posture rather than
    /// borrowing an unrelated local header's arguments.
    fn face_member_in(
        &self,
        root_file: FileId,
        root_writer: StringId,
        root_lib: LibraryId,
        merge_container: StringId,
        face: StringId,
        spelling: StringId,
        facts: &mut Option<HeaderClauses>,
    ) -> Option<FaceHit<'a>> {
        let mut name = face;
        let mut hop_writer = root_writer;
        let mut owner = root_file;
        let mut scope_lib = root_lib;
        let mut seen = FxHashSet::default();
        let mut chain: Vec<(StringId, FileId)> = Vec::new();
        // A pending face carries the chain of the hop that listed it —
        // `instantiate_inherited` walks the chain as the path of written
        // super edges from `merge_container` to the declaring hop, so a
        // branch's own path must be restored when it dequeues; the single
        // accumulator the walk otherwise keeps would graft the resolved
        // branch's hops onto the queued one (`D0 extends A implements B`
        // recording `B` behind `A`, an undecidable edge pair).
        let mut pending: VecDeque<(
            StringId,
            FileId,
            LibraryId,
            StringId,
            Vec<(StringId, FileId)>,
        )> = VecDeque::new();
        let mut collected: Vec<FaceHit<'a>> = Vec::new();
        loop {
            if !seen.insert(name) {
                // A clause cycle: the branches collected so far stand (a
                // cycle discovered before any hit leaves nothing — the
                // same suppression [`Self::face_member_tracked`] keeps).
                break;
            }
            if let Some((witness, entry)) = self.frozen_top_member(name, spelling) {
                chain.push((name, self.file));
                collected.push(FaceHit {
                    entry: HitEntry::Frozen(entry),
                    origin: self.file,
                    face_name: witness,
                    chain,
                });
                break;
            }
            let (found, found_owner) =
                match self.resolve_clause_face(hop_writer, owner, scope_lib, name) {
                    Some(pair) => pair,
                    None => {
                        let Some((next, next_owner, next_lib, next_writer, next_chain)) =
                            pending.pop_front()
                        else {
                            break;
                        };
                        name = next;
                        owner = next_owner;
                        scope_lib = next_lib;
                        hop_writer = next_writer;
                        chain = next_chain;
                        continue;
                    }
                };
            // The merge container itself is not a chain hop: the first
            // entry's incoming edge lives in `merge_container`'s own header
            // (`instantiate_inherited` looks `chain[0]` up in its `supers`),
            // so the root hop records nothing — its empty path is the
            // identity environment an own member already reads.
            if name != merge_container {
                chain.push((name, found_owner));
            }
            if let Some(hit) =
                self.face_own_hit(found_owner, name, found, spelling, scope_lib, chain.clone())
            {
                collected.push(hit);
                // A hit hop's own clauses never join — a spine hop's
                // declared face takes precedence over its
                // `implements`/`on` faces, and a drained interface branch
                // is done. The next candidate comes from the pending
                // queue.
                match pending.pop_front() {
                    Some((next, next_owner, next_lib, next_writer, next_chain)) => {
                        name = next;
                        owner = next_owner;
                        scope_lib = next_lib;
                        hop_writer = next_writer;
                        chain = next_chain;
                        continue;
                    }
                    None => break,
                }
            }
            let owner_lib = self
                .owner_library_of(found_owner, name)
                .unwrap_or(scope_lib);
            let mut mixin_hit = false;
            for mixin in found.with_clause.iter().rev() {
                if !seen.insert(*mixin) {
                    break;
                }
                if let Some((mixin_container, mixin_owner)) =
                    self.resolve_clause_face(name, owner, owner_lib, *mixin)
                {
                    chain.push((*mixin, mixin_owner));
                    let hit = self.face_own_hit(
                        mixin_owner,
                        *mixin,
                        mixin_container,
                        spelling,
                        owner_lib,
                        chain.clone(),
                    );
                    chain.pop();
                    if let Some(hit) = hit {
                        collected.push(hit);
                        mixin_hit = true;
                        break;
                    }
                    // A mixin CLASS applied through `with` also contributes
                    // its own `with` faces: a named application `mixin class
                    // A<U, V> = Object with M<...>` denotes the application
                    // type, so `class C ... with A<...>` puts M's members on
                    // C's interface exactly like a directly-written `with M`
                    // (upstream Dart SDK source,
                    // `pkg/analyzer/lib/src/dart/element/inheritance_manager3.dart`
                    // — the application interface keeps the applied faces'
                    // members). The superclass stays out: an applied mixin's
                    // superclass is always `Object`
                    // (`MIXIN_CLASS_DECLARATION_EXTENDS_NOT_OBJECT`), whose
                    // members the object-members gate owns. A plain mixin's
                    // `with` clause is empty — this only fires for mixin
                    // classes and named applications.
                    let mixin_owner_lib = self
                        .owner_library_of(mixin_owner, *mixin)
                        .unwrap_or(owner_lib);
                    // The mixin face's path runs THROUGH the mixin hop —
                    // its incoming edge lives in the mixin's own header —
                    // so the branch's chain snapshot re-appends the mixin
                    // hop the hit attempt popped.
                    let mut mixin_chain = chain.clone();
                    mixin_chain.push((*mixin, mixin_owner));
                    for mixin_face in mixin_container
                        .with_clause
                        .iter()
                        .chain(mixin_container.on_clause.iter())
                        .chain(mixin_container.implements_clause.iter())
                    {
                        pending.push_back((
                            *mixin_face,
                            mixin_owner,
                            mixin_owner_lib,
                            *mixin,
                            mixin_chain.clone(),
                        ));
                    }
                }
            }
            if mixin_hit {
                // The applied mixin's member replaces the super-face
                // candidate (the namedCandidates replace arm) — the deeper
                // spine drops out, but this hop's own `implements`/`on`
                // faces still join the candidate set.
                for super_face in found.implements_clause.iter().chain(found.on_clause.iter()) {
                    pending.push_back((*super_face, found_owner, owner_lib, name, chain.clone()));
                }
                match pending.pop_front() {
                    Some((next, next_owner, next_lib, next_writer, next_chain)) => {
                        name = next;
                        owner = next_owner;
                        scope_lib = next_lib;
                        hop_writer = next_writer;
                        chain = next_chain;
                        continue;
                    }
                    None => break,
                }
            }
            for super_face in found.implements_clause.iter().chain(found.on_clause.iter()) {
                pending.push_back((*super_face, found_owner, owner_lib, name, chain.clone()));
            }
            match found.superclass {
                Some(superclass) => {
                    scope_lib = owner_lib;
                    hop_writer = name;
                    name = superclass;
                    owner = found_owner;
                }
                None => {
                    let Some((next, next_owner, next_lib, next_writer, next_chain)) =
                        pending.pop_front()
                    else {
                        break;
                    };
                    name = next;
                    owner = next_owner;
                    scope_lib = next_lib;
                    hop_writer = next_writer;
                    chain = next_chain;
                }
            }
        }
        self.merge_face_candidates(collected, merge_container, face, facts)
    }

    /// Whether a member spelled `spelling` on the container `hop` declared
    /// in `owner_file` belongs to the name space of `referrer` — the
    /// library whose scope wrote the face spelling. Public names resolve
    /// from any library; a private one (`_m`) only from the library that
    /// declares it — the oracle's member identity is the `(name, library)`
    /// pair (`inheritance_manager3.dart`'s `Name` carries the declaring
    /// library), so a same-spelled private member in a foreign library is
    /// an unrelated member and can neither override nor be overridden
    /// (`element.library` / `isAccessibleFor` gates,
    /// `error_verifier.dart:5619-5622`).
    fn member_visible_to(
        &self,
        owner_file: FileId,
        hop: StringId,
        spelling: StringId,
        referrer: LibraryId,
    ) -> bool {
        if !self.strings.resolve(spelling).starts_with('_') {
            return true;
        }
        self.owner_library_of(owner_file, hop) == Some(referrer)
    }

    /// One hop's own member face: the stored entry under the lookup
    /// spelling (projected through the boundary, its pointer the dedup
    /// identity), or — for a setter spelling — the non-final field's
    /// induced setter (the oracle's `field.setter` element, synthesized:
    /// one required positional of the field's type). `referrer` is the
    /// library whose scope wrote the face spelling — the private-name
    /// identity's other half.
    fn face_own_hit(
        &self,
        owner_file: FileId,
        hop: StringId,
        found: &'a Container,
        spelling: StringId,
        referrer: LibraryId,
        chain: Vec<(StringId, FileId)>,
    ) -> Option<FaceHit<'a>> {
        // A private spelling names only the member declared in the SAME
        // library: the oracle keys the inheritance map on `(name, library)`
        // — an element name carries its declaring library, so `_m` in a
        // foreign library is a different element, never an inherited face
        // of the referring container (`inheritance_manager3.dart`'s
        // `Name`).
        if !self.member_visible_to(owner_file, hop, spelling, referrer) {
            return None;
        }
        if let Some(entry) = found.members.get(&spelling)
            && !entry.is_static
        {
            return Some(FaceHit {
                entry: HitEntry::Projected(self.boundary.member(owner_file, hop, spelling, entry)),
                origin: owner_file,
                face_name: hop,
                chain,
            });
        }
        let stripped = self.strings.resolve(spelling).strip_suffix('=')?;
        let plain = self.intern(stripped);
        let field = found.members.get(&plain)?;
        if field.is_static || field.kind != MemberKind::Field || field.is_final {
            return None;
        }
        let setter = MemberEntry {
            kind: MemberKind::Setter,
            formals: Some(Box::new(Formals {
                positional: Box::new([Formal {
                    name: plain,
                    ty: field.var_ty,
                    required: true,
                    // The `covariant` field keyword marks the induced
                    // setter's parameter (`element_builder.dart:504`).
                    covariant: field.covariant,
                    default_value: None,
                }]),
                named: Box::new([]),
            })),
            ..field.clone()
        };
        Some(FaceHit {
            entry: HitEntry::Frozen(setter),
            origin: owner_file,
            face_name: hop,
            chain,
        })
    }

    /// The frozen `dart:core` top-face signature for `member` on the
    /// `Object`/`Enum` hop `face` — the inherited signatures the oracle
    /// compares against when no supertype is written (`class C {}` extends
    /// `Object`, an enum's supertype is `Enum`, an `on`-less mixin
    /// constrains `Object`; the `Enum` face falls through to `Object`'s
    /// members — `Enum` redeclares only `index`). The signatures pin the
    /// oracle's inherited-slot renders (`Object.noSuchMethod` is `dynamic
    /// Function(Invocation)`, `Enum.index` is `int Function()`); an
    /// `Enum` face miss falls through to the `Object` table. Answers
    /// `(witness face, entry)` — the witness names `Object`/`Enum` in the
    /// diagnostic's inherited slot.
    fn frozen_top_member(
        &self,
        face: StringId,
        spelling: StringId,
    ) -> Option<(StringId, MemberEntry)> {
        let face_text = self.strings.resolve(face);
        let object = face_text == "Object";
        if !object && face_text != "Enum" {
            return None;
        }
        let member = self.strings.resolve(spelling);
        let witness = |entry: Option<MemberEntry>| {
            entry.map(|entry| (self.intern(if object { "Object" } else { "Enum" }), entry))
        };
        let entry = |kind, ret: &str, params: &[&str]| MemberEntry {
            kind,
            is_augment: false,
            is_static: false,
            is_abstract: false,
            is_external: false,
            is_final: false,
            is_const: false,
            covariant: false,
            type_params: 0,
            type_param_names: Box::default(),
            type_param_bounds: Box::default(),
            var_ty: Ty::UNKNOWN,
            return_ty: Ty::named(self.intern(ret)),
            // A getter declares no parameter list — `None`, the same
            // definite "none" a written getter's entry carries (a `Some`
            // empty list would read as the parameter-list mix arm's
            // recovery residue and suppress the comparison).
            formals: if kind == MemberKind::Getter {
                None
            } else {
                Some(Box::new(Formals {
                    positional: params
                        .iter()
                        .map(|p| Formal {
                            name: self.intern("parameter"),
                            ty: Ty::named(self.intern(p)),
                            required: true,
                            covariant: false,
                            default_value: None,
                        })
                        .collect::<Vec<_>>()
                        .into_boxed_slice(),
                    named: Box::new([]),
                }))
            },
            annotations: Box::default(),
            name_range: MemberEntry::MISSING_NAME_RANGE,
        };
        if object {
            witness(match member {
                "hashCode" => Some(entry(MemberKind::Getter, "int", &[])),
                "runtimeType" => Some(entry(MemberKind::Getter, "Type", &[])),
                "toString" => Some(entry(MemberKind::Method, "String", &[])),
                "noSuchMethod" => Some(entry(MemberKind::Method, "dynamic", &["Invocation"])),
                "==" => Some(entry(MemberKind::Method, "bool", &["Object"])),
                _ => None,
            })
        } else {
            witness(match member {
                "index" => Some(entry(MemberKind::Getter, "int", &[])),
                _ => None,
            })
            .or_else(|| {
                let object_face = self.intern("Object");
                self.frozen_top_member(object_face, spelling)
                    .map(|(_, entry)| (object_face, entry))
            })
        }
    }

    /// The collected candidates' merge
    /// (`inheritance_manager3.dart`'s `_getValidOverrides` + `_topMerge`,
    /// run on each candidate's substituted signature). Zero candidates or a
    /// candidates conflict answer `None` — the interface map carries no name
    /// and the face reports nothing; the single candidate passes through; a
    /// merged signature that belongs to no candidate becomes the merge
    /// target's own synthesized member, named by `merged_name` (the
    /// container whose interface map carries it — `_topMerge`'s
    /// `targetClass`).
    fn merge_face_candidates(
        &self,
        collected: Vec<FaceHit<'a>>,
        container: StringId,
        merged_name: StringId,
        facts: &mut Option<HeaderClauses>,
    ) -> Option<FaceHit<'a>> {
        if collected.len() <= 1 {
            return collected.into_iter().next();
        }
        let merged = {
            let refs: Vec<&FaceHit<'a>> = collected.iter().collect();
            self.merge_candidates(&refs, container, facts)
        };
        match merged {
            Merged::Conflict => None,
            Merged::Candidate(index) => collected.into_iter().nth(index),
            // A synthesized member carries no declaration of its own, so the
            // row's context stays empty rather than pointing at a candidate.
            Merged::Synthetic(entry) => Some(FaceHit {
                entry: HitEntry::Frozen(entry),
                origin: self.file,
                face_name: merged_name,
                chain: Vec::new(),
            }),
        }
    }

    /// [`Self::merge_candidates`]' surviving candidate index, or `None` when
    /// the candidates conflict — or when the combined signature belongs to
    /// no candidate, which has no hit to carry. The covariance-candidate
    /// fold calls this on hits it does not own and keeps its running
    /// candidate on `None`.
    fn merged_candidate_index(
        &self,
        collected: &[&FaceHit<'a>],
        container: StringId,
        facts: &mut Option<HeaderClauses>,
    ) -> Option<usize> {
        match self.merge_candidates(collected, container, facts) {
            Merged::Candidate(index) => Some(index),
            Merged::Conflict | Merged::Synthetic(_) => None,
        }
    }

    /// The candidate set's combined signature (`_getValidOverrides`, then
    /// `_topMerge`), over each candidate's substituted signature.
    ///
    /// `_getValidOverrides` keeps the candidates that are a valid override
    /// of every other candidate. `_topMerge` then answers the first survivor
    /// when there is only one, when every survivor renders the same
    /// signature, or when the type-by-type merge lands on one of them — and
    /// otherwise a signature none of them spells ([`Merged::Synthetic`]).
    ///
    /// Signature comparison rides the rendered text
    /// ([`Self::override_signature_text`], the same renderer the row's slots
    /// use, so an equal text means the merge cannot change what the oracle
    /// prints); the merge itself is [`Self::top_merge_ty`]'s modelled subset
    /// of `TopMergeHelper.topMerge`. A candidate whose substitution is
    /// undecidable keeps the walk's first candidate (the under-report
    /// posture [`Engine::face_member_tracked`]'s single hit kept); a first
    /// candidate that is itself undecidable drops the name.
    fn merge_candidates(
        &self,
        collected: &[&FaceHit<'a>],
        container: StringId,
        facts: &mut Option<HeaderClauses>,
    ) -> Merged {
        let mut substituted: Vec<MemberEntry> = Vec::with_capacity(collected.len());
        for (index, hit) in collected.iter().enumerate() {
            let Some(entry) = self.substituted_signature(hit, container, facts) else {
                return if index == 0 {
                    Merged::Conflict
                } else {
                    Merged::Candidate(0)
                };
            };
            substituted.push(entry);
        }
        let mut valid: Vec<usize> = Vec::new();
        'outer: for i in 0..substituted.len() {
            for j in 0..substituted.len() {
                if i != j
                    && self.override_broken(
                        &substituted[i],
                        (collected[i].origin, collected[i].face_name),
                        &substituted[j],
                        (collected[j].origin, collected[j].face_name),
                    )
                {
                    continue 'outer;
                }
            }
            valid.push(i);
        }
        let Some(&first) = valid.first() else {
            return Merged::Conflict;
        };
        let texts: Vec<String> = valid
            .iter()
            .map(|&index| self.override_signature_text(&substituted[index]))
            .collect();
        // `_topMerge`'s identity arms: one valid override, or every survivor
        // spelling the same signature — the merge could not change the text.
        if texts.iter().all(|text| *text == texts[0]) {
            return Merged::Candidate(first);
        }
        let entries: Vec<&MemberEntry> = valid.iter().map(|&index| &substituted[index]).collect();
        let Some(merged) = self.top_merge_signatures(&entries) else {
            // A pair outside the modelled merge keeps the first valid
            // candidate — the pre-port posture.
            return Merged::Candidate(first);
        };
        let merged_text = self.override_signature_text(&merged);
        match texts.iter().position(|text| *text == merged_text) {
            Some(position) => Merged::Candidate(valid[position]),
            None => Merged::Synthetic(merged),
        }
    }

    /// One candidate's signature under the super-face type-argument
    /// substitution (`Substitution.fromInterfaceType`): the declared types
    /// instantiated by the written clause arguments along the hit's chain.
    /// `None` = the environment is undecidable.
    fn substituted_signature(
        &self,
        hit: &FaceHit<'a>,
        container: StringId,
        facts: &mut Option<HeaderClauses>,
    ) -> Option<MemberEntry> {
        match self.instantiate_inherited(&hit.chain, hit.entry(), container, facts) {
            Instantiated::Unchanged => Some(hit.entry().clone()),
            Instantiated::Applied(env) => Some(self.substitute_entry(hit.entry(), &env)),
            Instantiated::Undecidable => None,
        }
    }

    /// `_topMergeSignatureTypes`: the `topMerge` fold over the valid
    /// candidates' signatures, in candidate order. `None` = a pair the
    /// modelled lattice cannot merge.
    fn top_merge_signatures(&self, valid: &[&MemberEntry]) -> Option<MemberEntry> {
        let (first, rest) = valid.split_first()?;
        let mut merged = (*first).clone();
        for entry in rest {
            merged = self.top_merge_entry(&merged, entry)?;
        }
        Some(merged)
    }

    /// One `topMerge` step over two candidate signatures
    /// (`TopMergeHelper._functionTypes`): the result column merges outright,
    /// every parameter column merges with both sides' required-ness and
    /// named spelling held equal (`_parameterKind` admits only the same
    /// kind), and a covariant column stays covariant. `None` = the pair is
    /// not mergeable — a kind, arity, named-set or generic-header mismatch,
    /// or a column outside [`Self::top_merge_ty`]'s subset.
    fn top_merge_entry(&self, first: &MemberEntry, other: &MemberEntry) -> Option<MemberEntry> {
        // The generic header must already agree (`_typeParameters` merges
        // only the same arity, and a kind split drops the name upstream).
        if first.kind != other.kind
            || first.type_param_names != other.type_param_names
            || first.type_param_bounds != other.type_param_bounds
        {
            return None;
        }
        let mut merged = first.clone();
        merged.return_ty = self.top_merge_ty(first.return_ty, other.return_ty)?;
        if first.kind == MemberKind::Field {
            merged.var_ty = self.top_merge_ty(first.var_ty, other.var_ty)?;
        }
        match (first.formals.as_deref(), other.formals.as_deref()) {
            (None, None) => {}
            (Some(a), Some(b))
                if a.positional.len() == b.positional.len() && a.named.len() == b.named.len() =>
            {
                let formals = merged.formals.as_deref_mut()?;
                for (slot, (x, y)) in formals
                    .positional
                    .iter_mut()
                    .zip(a.positional.iter().zip(b.positional.iter()))
                {
                    if x.required != y.required {
                        return None;
                    }
                    slot.ty = self.top_merge_ty(x.ty, y.ty)?;
                    slot.covariant = x.covariant || y.covariant;
                }
                // Named formals pair by name, not by written order (the
                // function type's named set is a name set).
                for slot in formals.named.iter_mut() {
                    let counterpart = b.named_formal(slot.name)?;
                    if slot.required != counterpart.required {
                        return None;
                    }
                    slot.ty = self.top_merge_ty(slot.ty, counterpart.ty)?;
                    slot.covariant = slot.covariant || counterpart.covariant;
                }
            }
            _ => return None,
        }
        // The synthesized signature has no declaration of its own: the
        // context row stays empty rather than pointing at a candidate.
        merged.name_range = MemberEntry::MISSING_NAME_RANGE;
        Some(merged)
    }

    /// `TopMergeHelper.topMerge` on one type column: the NORM canonical
    /// fold first (`normalize.dart` — `FutureOr<top>` collapses to the top,
    /// every other shape normalizes its columns), then the NNBD lattice's
    /// cross arms, then the structural descent over one shared head's
    /// arguments, one function shape, or one record shape. An unwritten
    /// annotation ([`Ty::Unknown`]) is the oracle's `dynamic`. `None` = the
    /// pair is outside the modelled subset (a different head, one-sided
    /// nullability, an unmergeable shape pair), and the caller keeps its
    /// first valid candidate.
    pub(crate) fn top_merge_ty(&self, first: Ty, other: Ty) -> Option<Ty> {
        let written = |ty: Ty| if ty == Ty::UNKNOWN { Ty::Dynamic } else { ty };
        let a = self.norm_ty(written(first));
        let b = self.norm_ty(written(other));
        // The identity arms: `dynamic`/`dynamic`, `void`/`void`,
        // `Never`/`Never`, `Object?`/`Object?`, one head with equal
        // arguments — every one of them answers its own value.
        if a == b {
            return Some(a);
        }
        let object_question = Ty::Named {
            name: self.intern("Object"),
            nullable: true,
            args: crate::model::ArgListId::EMPTY,
        };
        let question = |ty: Ty| ty == object_question;
        // The lattice's cross arms all land on `Object?`:
        // `(Object?, void)`, `(dynamic, void)`, `(Object?, dynamic)`.
        let crossed = (question(a) && matches!(b, Ty::Dynamic | Ty::Void))
            || (question(b) && matches!(a, Ty::Dynamic | Ty::Void))
            || (matches!(a, Ty::Dynamic) && matches!(b, Ty::Void))
            || (matches!(a, Ty::Void) && matches!(b, Ty::Dynamic));
        if crossed {
            return Some(object_question);
        }
        // The nullability arm: both sides carry the suffix (merge the cores,
        // restore it) or neither does. One-sided is not a mergeable pair —
        // the subtype directions disagree.
        let (a_nullable, a_core) = Self::top_core(a);
        let (b_nullable, b_core) = Self::top_core(b);
        if a_nullable != b_nullable {
            return None;
        }
        let merged_core = match (a_core, b_core) {
            (
                Ty::Named {
                    name: a_name,
                    args: a_args,
                    ..
                },
                Ty::Named {
                    name: b_name,
                    args: b_args,
                    ..
                },
            ) => {
                if a_name != b_name {
                    return None;
                }
                let (a_slots, b_slots) = {
                    let pool = self.boundary.args.borrow();
                    (pool.get(a_args).to_vec(), pool.get(b_args).to_vec())
                };
                if a_slots.len() != b_slots.len() {
                    return None;
                }
                let mut slots = Vec::with_capacity(a_slots.len());
                for (x, y) in a_slots.iter().zip(b_slots.iter()) {
                    slots.push(self.top_merge_ty(*x, *y)?);
                }
                if slots == a_slots {
                    a_core
                } else {
                    Ty::Named {
                        name: a_name,
                        nullable: false,
                        args: self.boundary.args.borrow_mut().intern(&slots),
                    }
                }
            }
            // `_functionTypes`: equal-shape gates only (arity, the
            // required/optional split, the named set); a type-parameter
            // group the shape cannot alpha-rename keeps the pair
            // unmergeable. A covariant column merges toward the subtype
            // verdict — the `Ty` model records no per-parameter variance,
            // so every column takes the merge arm, the documented
            // under-report posture.
            (Ty::Function { shape: x, .. }, Ty::Function { shape: y, .. }) => {
                let (x, y) = {
                    let pool = self.boundary.pool();
                    (pool.function_shape(x).clone(), pool.function_shape(y).clone())
                };
                if x.type_params != 0
                    || y.type_params != 0
                    || x.positional.len() != y.positional.len()
                    || x.required_positional != y.required_positional
                    || x.named.len() != y.named.len()
                {
                    return None;
                }
                let return_ty = self.top_merge_ty(x.return_ty, y.return_ty)?;
                let mut positional = Vec::with_capacity(x.positional.len());
                for (&p, &q) in x.positional.iter().zip(y.positional.iter()) {
                    positional.push(self.top_merge_ty(p, q)?);
                }
                let mut named = Vec::with_capacity(x.named.len());
                for &(name, p) in x.named.iter() {
                    let Some(&(_, q)) = y.named.iter().find(|&&(yn, _)| yn == name) else {
                        return None;
                    };
                    named.push((name, self.top_merge_ty(p, q)?));
                }
                self.boundary.intern_function_ty_required(
                    0,
                    x.required_positional,
                    return_ty,
                    &positional,
                    &named,
                    false,
                )
            }
            // `_recordTypes`: positional pairwise, named pairwise by name.
            (Ty::Record { shape: x, .. }, Ty::Record { shape: y, .. }) => {
                let (x, y) = {
                    let pool = self.boundary.pool();
                    (pool.record_shape(x).clone(), pool.record_shape(y).clone())
                };
                if x.positional.len() != y.positional.len() || x.named.len() != y.named.len() {
                    return None;
                }
                let mut positional = Vec::with_capacity(x.positional.len());
                for (&p, &q) in x.positional.iter().zip(y.positional.iter()) {
                    positional.push(self.top_merge_ty(p, q)?);
                }
                let mut named = Vec::with_capacity(x.named.len());
                for (&(name, p), &(yn, q)) in x.named.iter().zip(y.named.iter()) {
                    if name != yn {
                        return None;
                    }
                    named.push((name, self.top_merge_ty(p, q)?));
                }
                self.boundary
                    .args
                    .borrow_mut()
                    .record(&positional, &named, false)
            }
            _ => return None,
        };
        Some(if a_nullable {
            Self::nullable_form(merged_core)
        } else {
            merged_core
        })
    }

    /// `NormalizeHelper.normalize` (`normalize.dart`): the canonical
    /// representation the merge reads. `FutureOr<NORM(T)>` collapses when
    /// its normalized argument is a top type, `Object`, `Never` (→
    /// `Future<Never>`), or `Null` (→ `Future<Null>?`); `T?` collapses to
    /// the top/`Null` its core normalizes to (a nullable-argument
    /// `FutureOr` keeps the written face); every other shape normalizes
    /// its own columns. An unwritten annotation stays `Unknown` — the
    /// merge's own `dynamic` rewrite runs next.
    pub(crate) fn norm_ty(&self, ty: Ty) -> Ty {
        match ty {
            Ty::Named {
                name,
                nullable: true,
                args,
            } => {
                // NORM(T?): let S = NORM(T); a top stays, `Never?`/`Null?`
                // are `Null`, a nullable-argument `FutureOr` stays, else
                // `S?` (normalize.dart `_nullabilityQuestion`).
                let stripped = self.norm_ty(Ty::Named {
                    name,
                    nullable: false,
                    args,
                });
                if self.is_top_ty(stripped) {
                    return stripped;
                }
                if stripped == Ty::Never || stripped == Ty::Null {
                    return Ty::Null;
                }
                if let Ty::Named {
                    name: inner,
                    nullable: false,
                    args: inner_args,
                } = stripped
                    && inner == self.intern("FutureOr")
                    && inner_args != ArgListId::EMPTY
                {
                    let argument = self.boundary.pool().get(inner_args).first().copied();
                    if argument.is_some_and(|arg| self.nullable_ty(arg)) {
                        return stripped;
                    }
                }
                Self::nullable_form(stripped)
            }
            Ty::Named {
                name,
                nullable: false,
                args,
            } => {
                // NORM(FutureOr<T>): a top argument collapses to itself,
                // `Object` to itself, `Never` to `Future<Never>`, `Null`
                // to `Future<Null>?`, else `FutureOr<NORM(T)>`
                // (normalize.dart `_futureOr`).
                if name == self.intern("FutureOr") && args != ArgListId::EMPTY {
                    if let Some(argument) = self.boundary.pool().get(args).first().copied() {
                        let s = self.norm_ty(argument);
                        let object = Ty::Named {
                            name: self.intern("Object"),
                            nullable: false,
                            args: ArgListId::EMPTY,
                        };
                        if self.is_top_ty(s) || s == object {
                            return s;
                        }
                        if s == Ty::Never {
                            return Ty::Named {
                                name: self.intern("Future"),
                                nullable: false,
                                args: self.boundary.intern_list(&[Ty::Never]),
                            };
                        }
                        if s == Ty::Null {
                            return Ty::Named {
                                name: self.intern("Future"),
                                nullable: true,
                                args: self.boundary.intern_list(&[Ty::Null]),
                            };
                        }
                        if s != argument {
                            return Ty::Named {
                                name,
                                nullable: false,
                                args: self.boundary.intern_list(&[s]),
                            };
                        }
                        return ty;
                    }
                }
                // NORM(C<T0..Tn>) = C<NORM(Ti)>.
                if args != ArgListId::EMPTY {
                    let normalized: Vec<Ty> = {
                        let pool = self.boundary.pool();
                        let list = pool.get(args).to_vec();
                        drop(pool);
                        list.iter().map(|arg| self.norm_ty(*arg)).collect()
                    };
                    let interned = self.boundary.intern_list(&normalized);
                    if interned != args {
                        return Ty::Named {
                            name,
                            nullable: false,
                            args: interned,
                        };
                    }
                }
                ty
            }
            // Record and function columns normalize field-wise — the same
            // descent the named arm runs (`normalize.dart`'s record /
            // function arms), re-interned through the shared pool.
            Ty::Record { shape, nullable } => {
                let pooled = {
                    let pool = self.boundary.pool();
                    pool.record_shape(shape).clone()
                };
                let positional: Vec<Ty> = pooled
                    .positional
                    .iter()
                    .map(|ty| self.norm_ty(*ty))
                    .collect();
                let named: Vec<(StringId, Ty)> = pooled
                    .named
                    .iter()
                    .map(|(name, ty)| (*name, self.norm_ty(*ty)))
                    .collect();
                if positional == *pooled.positional && named == *pooled.named {
                    ty
                } else {
                    self.boundary
                        .args
                        .borrow_mut()
                        .record(&positional, &named, nullable)
                }
            }
            Ty::Function { shape, nullable } => {
                let pooled = {
                    let pool = self.boundary.pool();
                    pool.function_shape(shape).clone()
                };
                let return_ty = self.norm_ty(pooled.return_ty);
                let positional: Vec<Ty> = pooled
                    .positional
                    .iter()
                    .map(|ty| self.norm_ty(*ty))
                    .collect();
                let named: Vec<(StringId, Ty)> = pooled
                    .named
                    .iter()
                    .map(|(name, ty)| (*name, self.norm_ty(*ty)))
                    .collect();
                if return_ty == pooled.return_ty
                    && positional == *pooled.positional
                    && named == *pooled.named
                {
                    ty
                } else {
                    self.boundary.intern_function_ty_required(
                        pooled.type_params,
                        pooled.required_positional,
                        return_ty,
                        &positional,
                        &named,
                        nullable,
                    )
                }
            }
            other => other,
        }
    }

    /// `TypeSystemImpl.isTop` at `Ty` level: `dynamic`, `void`, `Unknown`
    /// (the oracle's `InvalidType`), `T?` over a top or `Object`, and
    /// `FutureOr<T>` over a top.
    fn is_top_ty(&self, ty: Ty) -> bool {
        match ty {
            Ty::Unknown | Ty::Dynamic | Ty::Void => true,
            Ty::Null | Ty::Never => false,
            Ty::Named {
                name,
                nullable,
                args,
            } => {
                if nullable {
                    return name == self.intern("Object")
                        || self.is_top_ty(Ty::Named {
                            name,
                            nullable: false,
                            args,
                        });
                }
                if name == self.intern("FutureOr") && args != ArgListId::EMPTY {
                    let argument = self.boundary.pool().get(args).first().copied();
                    return argument.is_some_and(|argument| self.is_top_ty(argument));
                }
                false
            }
            Ty::Record { .. } | Ty::Function { .. } => false,
        }
    }

    /// `TypeSystemImpl.isNullable` at `Ty` level (`dynamic`, `Null`, a
    /// `?`-flagged shape) — the full verdict, distinct from the
    /// `?`-suffix-only [`Self::is_nullable_ty`] the receiver gates read.
    fn nullable_ty(&self, ty: Ty) -> bool {
        match ty {
            Ty::Dynamic | Ty::Null => true,
            Ty::Named { nullable, .. }
            | Ty::Record { nullable, .. }
            | Ty::Function { nullable, .. } => nullable,
            _ => false,
        }
    }

    /// A type's suffix-less core plus its `?` flag — the pair the
    /// nullability arm merges before restoring the suffix. The suffix-less
    /// tops (`dynamic`, `void`, ...) answer themselves.
    fn top_core(ty: Ty) -> (bool, Ty) {
        match ty {
            Ty::Named {
                name,
                nullable,
                args,
            } => (
                nullable,
                Ty::Named {
                    name,
                    nullable: false,
                    args,
                },
            ),
            Ty::Record { shape, nullable } => (nullable, Ty::Record { shape, nullable: false }),
            Ty::Function { shape, nullable } => (nullable, Ty::Function { shape, nullable: false }),
            other => (false, other),
        }
    }

    /// The declared member's effective signature — the oracle's
    /// instance-member inference (upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/summary2/instance_member_inferrer.dart`
    /// `_inferAccessorOrField` / `_inferExecutable`): every implicit type
    /// slot infers from the combined superinterface signature the override
    /// faces compare, instead of reading `dynamic`-vs-`Unknown`. The
    /// decision order per member shape:
    /// - field: getter candidates non-empty and setter candidates empty →
    ///   the combined getter type; the reverse → the combined setter
    ///   parameter type; both non-empty → the getter type for a `final`
    ///   field, the getter type only when it equals the setter type for a
    ///   non-final one; both empty or a combined-signature conflict →
    ///   `dynamic` (`combineSignatureTypes`' null → `DynamicType`).
    /// - getter: the combined getter type, or the combined setter
    ///   parameter type when only setters override.
    /// - setter: the combined setter parameter type, or the combined
    ///   getter type when only getters override; an overridden covariant
    ///   setter marks the declared one covariant (the propagation runs
    ///   before the implicit-type gate, like upstream).
    /// - method: the combined signature fills the implicit return and each
    ///   implicit parameter (positional by index, named by name); a
    ///   kind-mismatched or conflicting candidate set leaves `dynamic`.
    ///
    /// A member with no implicit slot returns unchanged — the common fully
    /// -annotated face pays one slot check. The `bool` rides the field's
    /// induced setter face: the propagated `covariant` mark the declared
    /// setter entry itself carries for written setters.
    fn effective_declared(
        &self,
        container: StringId,
        entry: &MemberEntry,
        spelling: StringId,
        setter_name: StringId,
        faces: &[StringId],
        facts: &mut Option<HeaderClauses>,
    ) -> (MemberEntry, bool) {
        if !has_implicit_slot(entry) {
            return (entry.clone(), false);
        }
        let getter_candidates = |facts: &mut Option<HeaderClauses>| {
            let mut hits: Vec<FaceHit<'a>> = Vec::new();
            for face in faces {
                if let Some(hit) = self.override_face_member(container, *face, spelling, facts)
                    && matches!(hit.entry().kind, MemberKind::Getter | MemberKind::Field)
                {
                    hits.push(hit);
                }
            }
            hits
        };
        let setter_candidates = |facts: &mut Option<HeaderClauses>| {
            let mut hits: Vec<FaceHit<'a>> = Vec::new();
            for face in faces {
                if let Some(hit) = self.override_face_member(container, *face, setter_name, facts)
                    && hit.entry().kind == MemberKind::Setter
                {
                    hits.push(hit);
                }
            }
            hits
        };
        let getter_ty = |facts: &mut Option<HeaderClauses>,
                         hits: &mut Vec<FaceHit<'a>>| {
            let combined = hits.len();
            let witness = self.combined_inherited(container, std::mem::take(hits), facts);
            match witness {
                Some(entry) => Some(if entry.kind == MemberKind::Field {
                    entry.var_ty
                } else {
                    entry.return_ty
                }),
                None if combined == 0 => None,
                // A non-empty candidate set with no combined signature is
                // the conflict arm: `dynamic` (upstream
                // `combineSignatureTypes`' null → `DynamicTypeImpl`).
                None => Some(Ty::Dynamic),
            }
        };
        let setter_ty = |facts: &mut Option<HeaderClauses>,
                         hits: &mut Vec<FaceHit<'a>>| {
            let combined = hits.len();
            let witness = self.combined_inherited(container, std::mem::take(hits), facts);
            match witness {
                Some(entry) => entry
                    .formals
                    .as_deref()
                    .and_then(|f| f.positional.first())
                    .map(|p| p.ty)
                    .or(Some(Ty::Dynamic)),
                None if combined == 0 => None,
                None => Some(Ty::Dynamic),
            }
        };
        let dynamic = Ty::Dynamic;
        match entry.kind {
            MemberKind::Field => {
                let mut getters = getter_candidates(facts);
                let mut setters = setter_candidates(facts);
                let covariant = setters.iter().any(|hit| {
                    self.candidate_first_param_covariant(hit, setter_name, facts)
                });
                let mut effective = entry.clone();
                effective.var_ty = match (getters.is_empty(), setters.is_empty(), entry.is_final) {
                    (false, true, _) | (false, false, true) => {
                        getter_ty(facts, &mut getters).unwrap_or(dynamic)
                    }
                    (true, false, _) => setter_ty(facts, &mut setters).unwrap_or(dynamic),
                    // A non-final field overriding both: the getter type
                    // only when it equals the setter type, `dynamic`
                    // otherwise (upstream's equality gate).
                    (false, false, false) => {
                        let getter = getter_ty(facts, &mut getters);
                        let setter = setter_ty(facts, &mut setters);
                        match (getter, setter) {
                            (Some(gt), Some(st)) if gt == st => gt,
                            _ => dynamic,
                        }
                    }
                    (true, true, _) => dynamic,
                };
                (effective, covariant)
            }
            MemberKind::Getter => {
                let mut getters = getter_candidates(facts);
                let mut setters = setter_candidates(facts);
                let mut effective = entry.clone();
                effective.return_ty = if !getters.is_empty() {
                    getter_ty(facts, &mut getters).unwrap_or(dynamic)
                } else if !setters.is_empty() {
                    setter_ty(facts, &mut setters).unwrap_or(dynamic)
                } else {
                    dynamic
                };
                (effective, false)
            }
            MemberKind::Setter => {
                let mut setters = setter_candidates(facts);
                let mut getters = getter_candidates(facts);
                let covariant = setters.iter().any(|hit| {
                    self.candidate_first_param_covariant(hit, setter_name, facts)
                });
                let mut effective = entry.clone();
                if covariant
                    && let Some(formals) = effective.formals.as_deref_mut()
                    && let Some(param) = formals.positional.first_mut()
                {
                    param.covariant = true;
                }
                let ty = if !setters.is_empty() {
                    setter_ty(facts, &mut setters).unwrap_or(dynamic)
                } else if !getters.is_empty() {
                    getter_ty(facts, &mut getters).unwrap_or(dynamic)
                } else {
                    dynamic
                };
                if let Some(formals) = effective.formals.as_deref_mut()
                    && let Some(param) = formals.positional.first_mut()
                    && param.ty == Ty::UNKNOWN
                {
                    param.ty = ty;
                }
                (effective, false)
            }
            MemberKind::Method => {
                let mut candidates: Vec<FaceHit<'a>> = Vec::new();
                for face in faces {
                    if let Some(hit) = self.override_face_member(container, *face, spelling, facts)
                        && hit.entry().kind == MemberKind::Method
                    {
                        candidates.push(hit);
                    }
                }
                // A kind-mismatched candidate set (or none) skips the
                // inference entirely (`_allSameElementKind`'s gate).
                if candidates.is_empty() {
                    return (entry.clone(), false);
                }
                let combined = self.combined_inherited(container, candidates, facts);
                let mut effective = entry.clone();
                if effective.return_ty == Ty::UNKNOWN {
                    effective.return_ty = combined.as_ref().map(|c| c.return_ty).unwrap_or(dynamic);
                }
                if let Some(formals) = effective.formals.as_deref_mut() {
                    let slots = combined.as_ref().and_then(|c| c.formals.as_deref());
                    for (index, param) in formals.positional.iter_mut().enumerate() {
                        if param.ty != Ty::UNKNOWN {
                            continue;
                        }
                        param.ty = slots
                            .and_then(|f| f.positional.get(index))
                            .map(|p| p.ty)
                            .unwrap_or(dynamic);
                    }
                    for param in formals.named.iter_mut() {
                        if param.ty != Ty::UNKNOWN {
                            continue;
                        }
                        param.ty = slots
                            .and_then(|f| f.named.iter().find(|p| p.name == param.name))
                            .map(|p| p.ty)
                            .unwrap_or(dynamic);
                    }
                }
                (effective, false)
            }
        }
    }

    /// The combined most-specific signature over the candidate set
    /// (`inheritance_manager3.dart`'s `combineSignatureTypes`): the single
    /// candidate passes; otherwise [`Self::merge_candidates`] answers, so a
    /// set of mutually valid overrides answers the merged signature rather
    /// than the first one. An empty survivor set — the conflict arm —
    /// answers `None`, the caller's `dynamic`.
    ///
    /// Every arm answers the INSTANTIATED read: the oracle's combined
    /// super signature is the interface map's, whose members are already
    /// substituted by the edge arguments (`Substitution.fromInterfaceType`),
    /// and an implicit declared slot infers from THAT read
    /// (`instance_member_inferrer.dart`). Answering the declaring hop's own
    /// parameter spellings instead leaks them into the inferred type —
    /// `class A<E> { E foo() => 0; }` with `class B<T> extends A<T> {
    /// foo() => 0; }` inferred `E` against the comparison's substituted `T`
    /// and fabricated a break on every such one-face chain (the SDK faces
    /// read the same way: a `Future<X>` member's `X` inferred against the
    /// edge's written argument). An undecidable
    /// environment keeps the unsubstituted read — the same suppression
    /// posture the multi-candidate arm takes, and the alignment that
    /// matters: the inference and the subtype pass below now read the SAME
    /// entry, so they can never disagree about one face's signature.
    fn combined_inherited(
        &self,
        container: StringId,
        candidates: Vec<FaceHit<'a>>,
        facts: &mut Option<HeaderClauses>,
    ) -> Option<MemberEntry> {
        if candidates.len() <= 1 {
            let hit = candidates.first()?;
            return Some(
                self.substituted_signature(hit, container, facts)
                    .unwrap_or_else(|| hit.entry().clone()),
            );
        }
        let merged = {
            let refs: Vec<&FaceHit<'a>> = candidates.iter().collect();
            self.merge_candidates(&refs, container, facts)
        };
        match merged {
            Merged::Conflict => None,
            Merged::Synthetic(entry) => Some(entry),
            Merged::Candidate(index) => {
                self.substituted_signature(&candidates[index], container, facts)
            }
        }
    }

    /// The `no_combined_super_signature` candidate text
    /// (`inheritance_override.dart` `_reportNoCombinedSuperSignature` reading
    /// `TopLevelInferenceErrorNoCombinedSuperSignature.candidateSignatures`,
    /// recorded by `instance_member_inferrer.dart` `_inferExecutable` when
    /// `combineSignatureTypes` answers null): the METHOD's multi-candidate
    /// super-face set substitutes cleanly (`_addCandidates`' edge
    /// substitution) and no candidate validly overrides the others
    /// (`_getValidOverrides`' empty survivor set) — the oracle then prints
    /// every candidate as `Class.member (signature)`, joined `', '`.
    /// `None` = no verdict: a single candidate combines trivially (the
    /// `candidates.length == 1` identity arm), an undecidable substitution
    /// keeps the merge's own suppression posture (under-report), and a
    /// surviving candidate is a combined signature — silent.
    fn no_combined_super_candidates(
        &self,
        container: StringId,
        spelling: StringId,
        faces: &[StringId],
        facts: &mut Option<HeaderClauses>,
    ) -> Option<String> {
        let mut candidates: Vec<FaceHit<'a>> = Vec::new();
        for face in faces {
            if let Some(hit) = self.override_face_member(container, *face, spelling, facts)
                && hit.entry().kind == MemberKind::Method
            {
                candidates.push(hit);
            }
        }
        if candidates.len() <= 1 {
            return None;
        }
        let mut substituted: Vec<MemberEntry> = Vec::with_capacity(candidates.len());
        for hit in &candidates {
            substituted.push(self.substituted_signature(hit, container, facts)?);
        }
        // `_getValidOverrides`: a candidate broken against any other drops;
        // any survivor owns the combined signature.
        let has_valid = substituted.iter().enumerate().any(|(i, entry)| {
            substituted.iter().enumerate().all(|(j, other)| {
                i == j
                    || !self.override_broken(
                        entry,
                        (candidates[i].origin, candidates[i].face_name),
                        other,
                        (candidates[j].origin, candidates[j].face_name),
                    )
            })
        });
        if has_valid {
            return None;
        }
        let display = self.strings.resolve(spelling);
        // Every candidate as `Class.member (signature)` (the
        // `CandidatesConflict.candidates` fold, `_inferExecutable`); each
        // hit's chain names its declaring hop, so an empty chain decides
        // nothing.
        candidates
            .iter()
            .zip(&substituted)
            .map(|(hit, entry)| {
                let Some(&(owner, _)) = hit.chain.last() else {
                    return None;
                };
                Some(format!(
                    "{}.{} ({})",
                    self.strings.resolve(owner),
                    display,
                    self.override_signature_text(entry)
                ))
            })
            .collect::<Option<Vec<_>>>()
            .map(|rows| rows.join(", "))
    }

    /// The super-face type-argument substitution
    /// (`inheritance_manager3.dart`'s instantiated member map — the
    /// inherited signature the oracle's `isCorrectOverrideOf` reads is the
    /// member's declaration with the super face's written arguments
    /// substituted for its type parameters). The M1 model strips clauses
    /// to names, so the environment composes from the written clause
    /// facts: exact where every edge from the diagnosing container to the
    /// declaring hop lives in this file (the header token walk supplies
    /// both the arguments and the declaring hop's parameter names),
    /// name-forced where the declaring hop is foreign but its parameter
    /// count is one (the single free spelling maps to the single written
    /// argument), and undecidable elsewhere. Free parameters in the
    /// inherited signature with an undecidable environment suppress the
    /// face comparison — comparing them unsubstituted is how
    /// `D? fromSql(S?)` over `TypeConverter<D?, S?>`'s `D fromSql(S)`
    /// false-broke.
    fn instantiate_inherited(
        &self,
        chain: &[(StringId, FileId)],
        inherited: &MemberEntry,
        container: StringId,
        facts: &mut Option<HeaderClauses>,
    ) -> Instantiated {
        let Some((hop_name, hop_owner)) = chain.last().copied() else {
            return Instantiated::Unchanged;
        };
        let free = self.super_free_params(inherited, hop_owner);
        if free.is_empty() {
            return Instantiated::Unchanged;
        }
        // The environment at the declaring hop, composed along the chain.
        // `None` = an unknowable environment (a foreign edge's arguments
        // never enter the walk).
        fn clause<'a>(
            self_: &Engine<'_>,
            facts: &'a mut Option<HeaderClauses>,
        ) -> &'a mut HeaderClauses {
            facts.get_or_insert_with(|| self_.scan_header_clauses())
        }
        let mut env: Option<FxHashMap<StringId, Ty>> = Some(FxHashMap::default());
        for (index, &(hop, hop_file)) in chain.iter().enumerate() {
            // The edge written for this hop: the direct face's edge lives
            // in the diagnosing container's header; a deeper hop's edge
            // lives in the previous hop's header — same-file headers only.
            let incoming: Option<Vec<Ty>> = if index == 0 {
                clause(self, facts)
                    .for_name(container)
                    .and_then(|c| c.supers.iter().find(|entry| entry.head == hop))
                    .and_then(|entry| self.face_args(&entry.args))
            } else {
                let (prev, prev_file) = chain[index - 1];
                if prev_file != self.file {
                    None
                } else {
                    clause(self, facts)
                        .for_name(prev)
                        .and_then(|c| c.supers.iter().find(|entry| entry.head == hop))
                        .and_then(|entry| self.face_args(&entry.args))
                }
            };
            let Some(args) = incoming else {
                return Instantiated::Undecidable;
            };
            let prev = env.take();
            // The hop's parameter names: same-file headers spell them;
            // foreign hops expose only the count.
            let hop_params: Option<Vec<StringId>> = if hop_file == self.file {
                clause(self, facts).for_name(hop).map(|c| c.params.clone())
            } else {
                None
            };
            env = match (hop_params, prev) {
                (Some(params), prev) if params.len() == args.len() => {
                    let substituted: Vec<crate::model::Ty> = match prev {
                        Some(map) => args
                            .iter()
                            .map(|arg| self.substitute_ty(*arg, &map))
                            .collect(),
                        None => return Instantiated::Undecidable,
                    };
                    Some(params.into_iter().zip(substituted).collect())
                }
                _ => None,
            };
        }
        // The found hop's environment decides. Exact names substitute
        // directly; a foreign single-parameter hop maps its one free
        // spelling to the single written argument (the name-forced rule).
        if let Some(map) = env {
            return Instantiated::Applied(map);
        }
        let foreign_single = hop_owner != self.file
            && self
                .index
                .get(hop_owner)
                .and_then(|f| f.container(hop_name))
                .is_some_and(|c| c.type_params == 1);
        if foreign_single && chain.len() == 1 && free.len() == 1 {
            let incoming = clause(self, facts)
                .for_name(container)
                .and_then(|c| c.supers.iter().find(|entry| entry.head == hop_name))
                .and_then(|entry| self.face_args(&entry.args));
            if let Some([arg]) = incoming.as_deref() {
                let mut map = FxHashMap::default();
                map.insert(free[0], *arg);
                return Instantiated::Applied(map);
            }
        }
        Instantiated::Undecidable
    }

    /// The real type-argument list of a stored super face: the clause
    /// scanner wraps each face as `(head, vec![whole_named_ty])`, so the
    /// stored `Ty::Named`'s own `args` — not the one-element wrapping —
    /// supplies the substitution operands (the same unwrap `mixin_env`
    /// keeps). A face written with no type arguments yields `Some([])` — the
    /// edge is known (identity for a non-generic hop, under-applied for a
    /// generic one, which the arity check then rejects as undecidable).
    /// `None` marks a stored face that is not a plain `Named` — recovery
    /// residue or an unsupported shape where the arguments are unknowable.
    fn face_args(&self, stored: &[Ty]) -> Option<Vec<Ty>> {
        match stored {
            [Ty::Named { args, .. }] => Some(self.boundary.pool().get(*args).to_vec()),
            _ => None,
        }
    }

    /// The dead-superclass-edge subtype verdict — the oracle's `extends
    /// Missing` fallback as a subtype question: an unresolvable, non-type-
    /// parameter hop declares no supertype (the element lands on
    /// `Object`), so the declared closure of `sub` is COMPLETE once every
    /// visited head is either a resolved container (its written clauses),
    /// a dead hop, or a core-chain member. A closed walk that folded at
    /// least one dead edge and never reached `sup` answers `Some(false)` —
    /// provably incomparable, the direction the name lattice's cut cannot
    /// take (`XmlNodeList extends DelegatingList` over a dead
    /// `package:collection` edge still not being a `List`). A walk with no
    /// dead edge stays undecided — resolved hierarchies the M1 model
    /// cannot see (SDK mixin chains) keep the under-report posture, and a
    /// type-parameter hop cuts immediately (its bound is invisible).
    fn dead_edge_subtype(
        &self,
        sub: StringId,
        sub_scope: (FileId, StringId),
        sup: StringId,
    ) -> Option<bool> {
        let Some(start) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.owner_library_of(sub_scope.0, sub_scope.1)
                .unwrap_or(self.lib),
            sub_scope.0,
            sub,
        ) else {
            return None;
        };
        let mut queue: VecDeque<(FileId, StringId)> = VecDeque::new();
        let mut seen: FxHashSet<StringId> = FxHashSet::default();
        seen.insert(sub);
        queue.push_back((start.1, sub));
        let mut dead = false;
        while let Some((file, container)) = queue.pop_front() {
            let Some(decl) = self.index.get(file).and_then(|f| f.container(container)) else {
                continue;
            };
            let lib = self.owner_library_of(file, container).unwrap_or(self.lib);
            let edges = decl
                .superclass
                .into_iter()
                .chain(decl.with_clause.iter().copied())
                .chain(decl.implements_clause.iter().copied())
                .chain(decl.on_clause.iter().copied());
            for edge in edges {
                if edge == sup {
                    return Some(true);
                }
                if !seen.insert(edge) {
                    continue;
                }
                match resolve_container(self.graph, self.namespaces, self.index, lib, file, edge) {
                    Some((_, edge_owner)) => queue.push_back((edge_owner, edge)),
                    None => {
                        // A type parameter's supertypes ride its invisible
                        // bound — the walk cannot close over one.
                        if self
                            .index
                            .get(file)
                            .and_then(|f| f.decl(edge))
                            .is_some_and(|d| d.kind == crate::model::MicroKind::TypeParam)
                        {
                            return None;
                        }
                        // The oracle's dead hop: the face collapses to
                        // `Object`, contributing nothing.
                        dead = true;
                    }
                }
            }
            for (base, parent) in crate::switch_tracker::CORE_SUPERTYPES {
                if self.strings.resolve(container) == base && self.strings.resolve(sup) == parent {
                    return Some(true);
                }
                if self.strings.resolve(container) == base {
                    seen.insert(self.intern(parent));
                }
            }
        }
        dead.then_some(false)
    }

    /// The inherited entry's type-parameter spellings that are free in the
    /// diagnosing scope: not the entry's own generics (a generic method's
    /// parameters shadow its container's), but declared as a type
    /// parameter in the declaring file's declaration set — the
    /// unsubstituted container parameters the oracle never compares raw.
    /// Bare heads AND nested type-argument slots (`void blast(Wrapper<T>
    /// w)`'s `T`) both count — the walk is [`Self::free_params_in`], the
    /// same descent the mixin-application environment uses, so the
    /// substitution reaches parameter positions the bare-head walk
    /// deliberately missed and the nested-form override instantiates
    /// instead of comparing undecidable.
    fn super_free_params(&self, entry: &MemberEntry, scope_file: FileId) -> Vec<StringId> {
        let mut free: Vec<StringId> = Vec::new();
        let mut collect = |ty: Ty| self.free_params_in(ty, entry, scope_file, &mut free);
        collect(entry.var_ty);
        collect(entry.return_ty);
        if let Some(formals) = entry.formals.as_deref() {
            for formal in formals.positional.iter().chain(formals.named.iter()) {
                collect(formal.ty);
            }
        }
        free
    }

    /// Applies the substitution environment to the projected entry: a
    /// mutable copy with every environment key replaced (the declaring
    /// container's parameters instantiated by the super face's written
    /// arguments — the entry's own generics never collide with the keys).
    /// Argument lists re-intern bottom-up into the diagnosing pool, the
    /// same single universe every comparison resolves through.
    fn substitute_entry(&self, entry: &MemberEntry, keys: &FxHashMap<StringId, Ty>) -> MemberEntry {
        let mut copy = entry.clone();
        copy.var_ty = self.substitute_ty(copy.var_ty, keys);
        copy.return_ty = self.substitute_ty(copy.return_ty, keys);
        // A member's own type-parameter bounds substitute like every other
        // signature slot: a bound can spell the container's parameter
        // (`S extends List<T>` under `C<int>`), and the override face's
        // name unification renames the member's own parameters inside the
        // bound (`f<A, B extends A>` → `f<T, S extends T>`).
        for bound in copy.type_param_bounds.iter_mut() {
            *bound = self.substitute_ty(*bound, keys);
        }
        if let Some(formals) = &mut copy.formals {
            for formal in formals
                .positional
                .iter_mut()
                .chain(formals.named.iter_mut())
            {
                formal.ty = self.substitute_ty(formal.ty, keys);
            }
        }
        copy
    }

    /// One `Ty` under the substitution: a bare parameter spelling in the
    /// key set becomes its written argument (the `?` projections
    /// union — `D?` over the argument `S?` stays `S?`), a named type with
    /// arguments recurses into them (re-interned when any argument
    /// changed), every other shape passes through.
    pub(crate) fn substitute_ty(&self, ty: Ty, keys: &FxHashMap<StringId, Ty>) -> Ty {
        if keys.is_empty() {
            return ty;
        }
        // The structural fold ([`crate::ty_fold`]) reaches every leaf —
        // a parameter nested in a function-typed formal (`bool
        // Function(E)`) or a record field instantiates like a bare one.
        self.boundary.args.borrow_mut().map_leaves(ty, &mut |leaf| {
            let Ty::Named { name, nullable, .. } = leaf else {
                return leaf;
            };
            match keys.get(&name) {
                Some(replacement) => {
                    let mut replaced = *replacement;
                    if nullable && !Self::is_nullable_ty(replaced) {
                        replaced = Self::nullable_form(replaced);
                    }
                    replaced
                }
                None => leaf,
            }
        })
    }

    /// The current file's written supertype clauses, one token walk. The
    /// walk is the substitution face's only fact source (the element model
    /// strips clauses to names); it runs on first need — an inherited
    /// signature carrying free type parameters — and stays off the plain
    /// override path.
    fn scan_header_clauses(&self) -> HeaderClauses {
        let mut map = HeaderClauses {
            by_offset: FxHashMap::default(),
            last_of_name: FxHashMap::default(),
        };
        let toks = self.tokens.as_slice();
        let mut i = 0;
        while i < toks.len() {
            match self.tokens.lexeme(self.source, i) {
                "class" | "mixin" | "enum" => i = self.scan_one_header(i + 1, &mut map),
                _ => i += 1,
            }
        }
        map
    }

    /// The written type arguments of `container`'s clause naming
    /// `super_name` (`PathMap<V> extends MapView<String?, V>` answers
    /// `[String?, V]` for `MapView`) — the subclass's written
    /// instantiation for the direct superclass, the substitution source
    /// the element model's name-only clause face cannot carry.
    pub(super) fn clause_instantiation(
        &self,
        container: StringId,
        super_name: StringId,
    ) -> Option<Vec<Ty>> {
        self.scan_header_clauses()
            .for_name(container)?
            .supers
            .iter()
            .find(|entry| entry.head == super_name)
            .map(|entry| entry.args.clone())
    }

    /// Rewrites one pending type-position row into the suppressed `Missing`
    /// shape at the clause-check time ([`Self::check_clause_element_kinds`]):
    /// the row still resolves later (its `record_miss` bookkeeping and the
    /// `Missing` arm's clause/context chains keep running), but the report
    /// gate drops it — the oracle's `hasErrorReported` and the clause-parent
    /// suppressions leave the resolver silent once the clause code emits
    /// (upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/dart/resolver/resolution_visitor.dart:1178-1180`
    /// and `named_type_resolver.dart:673-680`). `Prefixed`/`NonType` rows do
    /// not honor the flag their own arms read, so the rewrite normalizes the
    /// verdict itself — the row's name and range are the miss's identity.
    fn suppress_type_row(&mut self, range: SourceRange) {
        for row in self.rows.iter_mut().chain(self.deferred_miss_rows.iter_mut()) {
            // A `Prefixed` row records the leaf's own span while a scanned
            // entry's span opens at the prefix token — the row matches when
            // it sits inside the entry's span, not only when it equals it.
            if row.range.offset >= range.offset
                && row.range.end() <= range.end()
                && matches!(
                    row.shape,
                    RowShape::TypeUse(
                        TypeVerdict::Missing | TypeVerdict::NonType | TypeVerdict::Prefixed { .. },
                        _
                    )
                )
            {
                row.shape = RowShape::TypeUse(TypeVerdict::Missing, true);
            }
        }
    }

    /// [`Self::suppress_type_row`] at a bare offset — the recovery-artifact
    /// sweep's face (its `Missing` row sits at the offset itself, the
    /// `parse_type_not_void` re-fire's `{`/`,` token).
    fn suppress_type_row_offset(&mut self, offset: u32) {
        for row in self.rows.iter_mut().chain(self.deferred_miss_rows.iter_mut()) {
            if row.range.offset == offset
                && matches!(
                    row.shape,
                    RowShape::TypeUse(
                        TypeVerdict::Missing | TypeVerdict::NonType | TypeVerdict::Prefixed { .. },
                        _
                    )
                )
            {
                row.shape = RowShape::TypeUse(TypeVerdict::Missing, true);
            }
        }
    }

    /// Drops one clause-ledger entry at `offset` without emitting —
    /// [`Self::take_clause_miss`]'s consumption cousin for the resolved-
    /// element clause check. A scanned entry that resolved to a clause-legal
    /// kind, or one the upstream walk never reaches (a mixin declaration's
    /// `with` entries, a trailing `extends` overflow), leaves its ledger
    /// slot behind; an unresolved entry keeps it — the pass-2 `Missing` arm
    /// reports the clause code through it. The ledger stays sorted by
    /// offset (removal only).
    fn remove_clause_miss(&mut self, offset: u32) {
        if let Ok(index) = self
            .clause_miss
            .binary_search_by_key(&offset, |entry| entry.0)
        {
            self.clause_miss.remove(index);
        }
    }

    /// The resolved-element half of the oracle's `_resolveType` clause
    /// check (upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/dart/resolver/resolution_visitor.dart`
    /// :1169-1241 — the clause-kind switch keys the diagnostic on the
    /// CLAUSE, never on what the element resolved to): for each scanned
    /// [`SuperEntry`] of `container`'s header, answer the head's element
    /// and report the clause code when the kind is illegal for the clause
    /// — `ClassElement` is legal for every clause, `MixinElement` only for
    /// `implements`/`with`/`on` (:1194-1202), and every other outcome —
    /// enum, extension-type, alias-to-non-class, type parameter,
    /// `dynamic`, `void`, a resolved non-type, a miss — falls to the
    /// clause arm (:1209-1227).
    ///
    /// Drained once at the body open (the deferred modifier's
    /// `use_sites_done` flag), or at the `implements` claim for an enum —
    /// its arm carries `container: None` and never reaches the body-open
    /// drain. Each entry also re-decides its own resolver row: a
    /// resolved-but-illegal face silences the resolver
    /// (`named_type_resolver.dart`'s `reportNullOrNonTypeElement`
    /// clause-parent suppression :673-680 covers every clause EXCEPT
    /// `MixinOnClause` — the `on` constraint keeps its `undefined_class`/
    /// `not_a_type` row beside `mixin_super_class_constraint_non_
    /// interface`), so the pending row at the same span is suppressed and
    /// the clause-ledger slot consumed here.
    ///
    /// `window` is the header's whole captured type-span set — a
    /// clause-ledger offset no scanned [`SuperEntry`] owns is a
    /// parser-recovery artifact (the `parse_type_not_void` re-fire's
    /// `Missing` row at the `{`/`,` position after a `void` head) and
    /// silences with its row.
    fn check_clause_element_kinds(
        &mut self,
        container: StringId,
        name_offset: u32,
        window: &[u32],
    ) {
        // `name_offset` identifies THIS declaration: a repeated container
        // name (`class C` written eighty times) carries one map entry per
        // declaration, so each header reports its own clauses exactly once —
        // name-keying would re-report the last declaration's clauses at
        // every declaration.
        let Some(facts) = self.scan_header_clauses().by_offset.remove(&name_offset) else {
            return;
        };
        let Some(kind) = self
            .index
            .get(self.file)
            .and_then(|elements| elements.container(container))
            .map(|container| container.kind)
        else {
            return;
        };
        let has_with = facts.supers.iter().any(|entry| entry.tag == SuperTag::With);
        // Reachability across the entry walk: upstream's parser keeps only
        // the FIRST `extends` clause (`handleClassExtends` — a second
        // `extends` and a comma-overflow both fold into
        // `multiple_extends_clauses`, their heads never resolving), EXCEPT a
        // trailing `extends` after `implements` (the ordering-recovery tail
        // — `implements_before_extends` reorders the clauses, and that tail
        // head DOES reach `_resolveType`). Flags:
        // `first_extends_consumed` — an earlier Extends entry already claimed
        // the clause slot; `implements_since_extends` — an Implements entry
        // intervened since the last Extends, reopening the tail.
        let mut first_extends_consumed = false;
        let mut implements_since_extends = false;
        for entry in &facts.supers {
            let span = entry.span;
            // The upstream clause-code decision keys on (declaration,
            // clause) — `class C = S ...` reaches `visitClassTypeAlias` with
            // `clause: null`, and `scope_context.dart`'s recovery shapes
            // route `class A on X` through the `ExtendsClause` arm and
            // `mixin M extends X` through `MixinOnClause` (upstream's
            // `EXPECTED_INSTEAD` recoveries keep the clause's own arm).
            let clause_code = match (kind, entry.tag) {
                (ContainerKind::Class, SuperTag::On) => {
                    if has_with {
                        SemanticCode::MixinWithNonClassSuperclass
                    } else {
                        SemanticCode::ExtendsNonClass
                    }
                }
                (ContainerKind::Mixin, SuperTag::Extends) => {
                    SemanticCode::MixinSuperClassConstraintNonInterface
                }
                (_, SuperTag::AliasHead) => SemanticCode::MixinWithNonClassSuperclass,
                (_, SuperTag::Extends) => {
                    if has_with {
                        SemanticCode::MixinWithNonClassSuperclass
                    } else {
                        SemanticCode::ExtendsNonClass
                    }
                }
                (_, SuperTag::With) => SemanticCode::MixinOfNonClass,
                (_, SuperTag::Implements) => SemanticCode::ImplementsNonClass,
                (_, SuperTag::On) => SemanticCode::MixinSuperClassConstraintNonInterface,
            };
            // The upstream legality by clause (`:1194-1202`): every clause
            // accepts a `ClassElement`; `implements`/`with`/`on` also
            // accept a `MixinElement`.
            let clause_accepts_mixin = matches!(
                entry.tag,
                SuperTag::Implements | SuperTag::With | SuperTag::On
            );
            let legal = |resolved: Option<ContainerKind>| {
                match resolved {
                    Some(ContainerKind::Class) => true,
                    Some(ContainerKind::Mixin) => clause_accepts_mixin,
                    _ => false,
                }
            };
            // A head the upstream `_resolveType` walk never reaches —
            // silence the row and the ledger entry without a report:
            //   * a second+ `extends` head on a class (`extends B extends C`,
            //     `extends B, C` — `multiple_extends_clauses` recovery keeps
            //     only the first clause), unless an `implements` entry
            //     intervened (`implements B extends C` — the
            //     `implements_before_extends` recovery tail head DOES
            //     resolve);
            //   * a mixin declaration's `with` entries (`mixin_with_clause`
            //     recovery — the mixin grammar has no `with` clause);
            //   * enum `extends`/`on`/alias-super residue (`scope_context`
            //     routes enums to with/implements only).
            if entry.tag == SuperTag::Implements {
                implements_since_extends = true;
            }
            let unreachable = (entry.tag == SuperTag::Extends
                && kind == ContainerKind::Class
                && first_extends_consumed
                && !implements_since_extends)
                || (kind == ContainerKind::Mixin && entry.tag == SuperTag::With)
                || (kind == ContainerKind::Enum
                    && matches!(entry.tag, SuperTag::Extends | SuperTag::On | SuperTag::AliasHead));
            if entry.tag == SuperTag::Extends {
                first_extends_consumed = true;
                implements_since_extends = false;
            }
            if unreachable {
                self.suppress_type_row(span);
                self.remove_clause_miss(span.offset);
                continue;
            }
            // The head's resolved element: `void`/`dynamic` keyword heads
            // carry a dedicated `Ty` (never a container); a prefixed head
            // resolves the leaf through the prefix's merged table; a bare
            // name resolves bare — a type alias's RHS head folds through
            // [`Self::alias_container_head`] (the same fold the override
            // faces keep, `resolution_visitor.dart`'s `type.element` read
            // reaches the alias's instantiated element).
            let resolved: Option<ContainerKind> = if entry.head == StringId::NONE {
                None
            } else if let Some(prefix) = entry.prefix {
                match self
                    .namespaces
                    .lookup_prefixed(self.graph, self.lib, prefix, entry.head)
                {
                    Lookup::Found(NamespaceEntry::Decl(owner)) => self
                        .index
                        .get(owner)
                        .and_then(|f| f.container(entry.head))
                        .map(|c| c.kind),
                    _ => None,
                }
            } else {
                let scoped = lookup_scoped(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    entry.head,
                );
                match scoped {
                    ScopedHit::Local(decl) => match decl.kind {
                        crate::model::MicroKind::TypeAlias => self
                            .alias_container_head(entry.head, self.lib, self.file)
                            .map(|(container, _, _)| container.kind),
                        crate::model::MicroKind::Class | crate::model::MicroKind::Mixin => self
                            .index
                            .get(self.file)
                            .and_then(|f| f.container(entry.head))
                            .map(|c| c.kind),
                        _ => None,
                    },
                    ScopedHit::Found(NamespaceEntry::Decl(owner)) => {
                        // A FOREIGN type alias folds to its right-hand head
                        // exactly like the local arm above: the oracle's
                        // `_resolveType` reads `type.element` of the
                        // instantiated alias (`resolution_visitor.dart`
                        // :1170-1185), so `implements TypedefBase` over
                        // `typedef TypedefBase = BaseClass;` is a legal
                        // class entry, never `implements_non_class`.
                        match self.index.get(owner).and_then(|f| f.decl(entry.head)).map(|d| d.kind) {
                            Some(crate::model::MicroKind::TypeAlias) => self
                                .alias_container_head(entry.head, self.lib, self.file)
                                .map(|(container, _, _)| container.kind),
                            _ => self
                                .index
                                .get(owner)
                                .and_then(|f| f.container(entry.head))
                                .map(|c| c.kind),
                        }
                    }
                    _ => None,
                }
            };
            if legal(resolved) {
                // A clause-legal entry resolves without a report — its
                // ledger slot stays behind only because the resolver never
                // consumed it.
                self.remove_clause_miss(span.offset);
                continue;
            }
            // The additive face: `on` (and the recovered `mixin M extends
            // X`) keeps the resolver's own row beside the clause code
            // (`MixinOnClause` is the one clause parent
            // `reportNullOrNonTypeElement` does not suppress,
            // `named_type_resolver.dart:673-680`). Every other face
            // silences the resolver row with the clause code
            // (`hasErrorReported`, `resolution_visitor.dart:1178-1180`).
            let additive = matches!(
                (kind, entry.tag),
                (ContainerKind::Mixin, SuperTag::On) | (ContainerKind::Mixin, SuperTag::Extends)
            );
            if additive {
                self.remove_clause_miss(span.offset);
            } else {
                self.suppress_type_row(span);
                self.remove_clause_miss(span.offset);
            }
            // `shouldIgnoreUndefinedNamedType`'s broken-import posture
            // (:1205-1207) suppresses the clause row itself when the
            // (possibly prefixed) name sits in a broken import's `show`
            // list — the guard consults prefix + leaf the same way it
            // does for the resolver's `undefined_class`.
            if !self.guard.ignore(entry.prefix, entry.head) {
                if clause_code == SemanticCode::ExtendsNonClass
                    || clause_code == SemanticCode::ImplementsNonClass
                {
                    // The correction-carrying clause faces — the oracle's
                    // `correctionMessage` rows land beside the diagnostic
                    // (`emit_clause_miss`'s convention).
                    let message = clause_code.message("", "");
                    self.emit_message_with_correction(clause_code, span, message);
                } else {
                    self.emit(clause_code, span, "", "");
                }
            }
        }
        // The recovery-artifact sweep: a `clause_miss` offset inside the
        // window that no scanned entry owns is the `parse_type_not_void`
        // re-fire's `Missing` row (its own `void`-head span precedes it)
        // or a claim/count disagreement in a recovery shape — both silence
        // with their rows, never reporting the clause code.
        let mut owned: Vec<u32> = facts
            .supers
            .iter()
            .map(|entry| entry.span.offset)
            .collect();
        owned.sort_unstable();
        for offset in window {
            if owned.binary_search(offset).is_err() {
                self.suppress_type_row_offset(*offset);
                self.remove_clause_miss(*offset);
            }
        }
    }
    /// The prefixed supertype-clause faces of this file's container headers:
    /// `(writer container, face local name)` → the written prefix head. The
    /// element model keeps only the clause continuation (`implements idl.X`
    /// records `X` — collector.rs's bare-name convention), so a prefixed
    /// clause face resolves nowhere in the bare scope and every face walk
    /// over it dies. The dotted spellings live only in the source tokens;
    /// this bounded header-window walk recovers them once per pass
    /// ([`FaceMemo::prefixed_clauses`]). A recorded pair whose
    /// [`prefixed_container`] probe misses is inert: an unregistered prefix
    /// or an absent local keeps the dead-edge verdict the bare miss already
    /// answered.
    fn prefixed_clauses(&self) -> FxHashMap<(StringId, StringId), StringId> {
        if let Some(hit) = self.member_faces.prefixed_clauses.borrow().as_ref() {
            return hit.clone();
        }
        let map = self.scan_prefixed_clauses();
        *self.member_faces.prefixed_clauses.borrow_mut() = Some(map.clone());
        map
    }

    /// Whether `face` in `writer`'s supertype clauses was spelled through an
    /// import prefix (`idl.X`) in this file's header text. Only the
    /// diagnosing file's tokens are scanned — a foreign container's clauses
    /// keep the bare-name convention (the under-report direction).
    fn prefixed_clause_prefix(&self, writer: StringId, face: StringId) -> Option<StringId> {
        let map = self.prefixed_clauses();
        if map.is_empty() {
            return None;
        }
        map.get(&(writer, face)).copied()
    }

    /// One linear token pass collecting the dotted clause faces
    /// ([`Self::prefixed_clauses`]): the same `class`/`mixin`/`enum` header
    /// trigger [`Self::scan_header_clauses`] applies, each header window
    /// walked to its body (skipped whole) or `;` mixin-application close,
    /// `<...>` groups depth-tracked so an argument-position `p.T`
    /// (`implements List<p.T>`, a bounded parameter) never reads as a clause
    /// face.
    fn scan_prefixed_clauses(&self) -> FxHashMap<(StringId, StringId), StringId> {
        let mut map: FxHashMap<(StringId, StringId), StringId> = FxHashMap::default();
        let toks = self.tokens.as_slice();
        let mut i = 0;
        while i < toks.len() {
            match self.tokens.lexeme(self.source, i) {
                "class" | "mixin" | "enum" => i = self.scan_one_prefixed_header(i + 1, &mut map),
                _ => i += 1,
            }
        }
        map
    }

    /// Walks one header's window ([`Self::scan_prefixed_clauses`]): an
    /// identifier at depth 0 followed by `.` and an identifier is a prefixed
    /// clause face (`idl.X`), recorded under the declaring container's name.
    /// Recovery residue inside the window can only record a pair whose
    /// prefix probe misses — the lookup, not the scan, validates.
    fn scan_one_prefixed_header(
        &self,
        start: usize,
        map: &mut FxHashMap<(StringId, StringId), StringId>,
    ) -> usize {
        let toks = self.tokens.as_slice();
        let lex_at = |i: usize| toks.get(i).map(|_| self.tokens.lexeme(self.source, i));
        let mut i = start;
        let Some(container) = self.intern_checked(i) else {
            return i;
        };
        i += 1;
        let mut angle = 0usize;
        while let Some(lex) = lex_at(i) {
            match lex {
                "{" => return self.tokens.matching_end(i).map_or(toks.len(), |c| c + 1),
                ";" => return i + 1,
                "<" => {
                    angle += 1;
                    i += 1;
                }
                // The scanner never splits shift tokens (`>>` is always one
                // token — upstream Dart SDK source,
                // `pkg/front_end/lib/src/scanner/abstract_scanner.dart`
                // `tokenizeGreaterThan`), so a nested argument group's
                // closers arrive merged: unwind one depth per `>`.
                ">" | ">>" | ">>>" => {
                    angle = angle.saturating_sub(lex.matches('>').count());
                    i += 1;
                }
                "." if angle == 0 => {
                    // `prefix . local` at a clause-head position: the head
                    // identifier precedes the dot, the continuation follows.
                    if let (Some(prefix), Some(local)) = (
                        i.checked_sub(1).and_then(|p| self.intern_checked(p)),
                        i.checked_add(1).and_then(|n| self.intern_checked(n)),
                    ) {
                        map.insert((container, local), prefix);
                    }
                    i += 2;
                }
                _ => i += 1,
            }
        }
        i
    }

    /// The class a type-alias spelling names, looked up from `file` in
    /// `lib` (`typedef IterableBase<E> = Iterable<E>` → `Iterable`'s
    /// container, its declaring file, and the head spelling). Chained
    /// aliases fold up to the loop depth; only the RHS head resolves (the
    /// clause arguments ride the member sites). `None` for a non-alias, an
    /// alias to a non-class RHS, or an unresolvable head.
    fn alias_container_head(
        &self,
        name: StringId,
        lib: LibraryId,
        file: FileId,
    ) -> Option<(&'a Container, FileId, StringId)> {
        let (mut head, mut scope_lib, mut scope_file) = (name, lib, file);
        for _ in 0..8 {
            let (owner, decl) = match lookup_scoped(
                self.graph,
                self.namespaces,
                self.index,
                scope_lib,
                scope_file,
                head,
            ) {
                ScopedHit::Local(decl) => (scope_file, decl),
                ScopedHit::Found(NamespaceEntry::Decl(owner)) => {
                    (owner, self.index.get(owner)?.decl(head)?)
                }
                _ => return None,
            };
            if decl.kind != crate::model::MicroKind::TypeAlias || decl.var_ty == Ty::UNKNOWN {
                return None;
            }
            let Ty::Named { name: next, .. } = self.boundary.ty(owner, decl.var_ty) else {
                return None;
            };
            let next_lib = self.owner_library_of(owner, head).unwrap_or(scope_lib);
            if let Some((container, found)) = resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                next_lib,
                owner,
                next,
            ) {
                return Some((container, found, next));
            }
            (head, scope_lib, scope_file) = (next, next_lib, owner);
        }
        None
    }

    /// Resolves a supertype-clause face as written in `writer`'s header: a
    /// dotted spelling (`idl.X`, recovered by
    /// [`Self::prefixed_clause_prefix`]) binds through the prefix's merged
    /// table — the same container identity an unprefixed import of the same
    /// library yields ([`prefixed_container`]) — and never falls back to the
    /// bare scope (a same-named local must not shadow the written face).
    /// A bare spelling keeps [`resolve_container`]. Only this file's headers
    /// carry prefix facts; a foreign writer whose bare face misses recovers
    /// the clause's prefix through the writing library's own prefix set
    /// ([`Self::foreign_prefixed_clause`]).
    pub(crate) fn resolve_clause_face(
        &self,
        writer: StringId,
        file: FileId,
        scope_lib: LibraryId,
        face: StringId,
    ) -> Option<(&'a Container, FileId)> {
        if file == self.file
            && let Some(prefix) = self.prefixed_clause_prefix(writer, face)
        {
            let lib = self.graph.library_of_file(file).unwrap_or(scope_lib);
            return prefixed_container(self.graph, self.namespaces, self.index, lib, prefix, face);
        }
        let bare = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            scope_lib,
            file,
            face,
        );
        let bare = match bare {
            Some(hit) => Some(hit),
            // A clause face that names a TYPE ALIAS binds the alias's
            // right-hand container (`with ListMixin<E>` — SDK 3.47 spells
            // `typedef ListMixin<E> = ListBase<E>`; the mixin application's
            // real face is `ListBase`, whose members the oracle walks — the
            // alias's own name resolves to no container). The RHS parameter
            // leaves ride bare — the application's written clause arguments
            // substitute at the member sites — so only the HEAD resolves
            // here; chains and cycles bound by the loop depth.
            None => {
                let mut head = face;
                let mut hit = None;
                for _ in 0..8 {
                    let Some(Ty::Named { name: next, .. }) = self.alias_rhs_any(head) else {
                        break;
                    };
                    if next == head {
                        break;
                    }
                    head = next;
                    if let Some(found) = resolve_container(
                        self.graph,
                        self.namespaces,
                        self.index,
                        scope_lib,
                        file,
                        head,
                    ) {
                        hit = Some(found);
                        break;
                    }
                }
                hit
            }
        };
        if bare.is_some() || file == self.file {
            return bare;
        }
        self.foreign_prefixed_clause(file, face)
    }

    /// A foreign writer's prefixed clause face. The collector stores the
    /// clause's bare leaf (`extends base.SummaryClass` keeps `SummaryClass`),
    /// so when the leaf is visible in the writing file's scope ONLY through
    /// the prefix it was written with, the bare walk misses by construction
    /// and the prefix is unrecoverable from the diagnosing scanner (it
    /// covers `self.file` alone). Probe the writing library's own prefix
    /// set: exactly one bound element answers. Zero hits (no prefix
    /// exports the leaf — the genuine undefined face) or two distinct
    /// elements (two prefixes, an ambiguity the spelling cannot decide)
    /// keep the bare miss — the under-report posture the bare face already
    /// paid; prefixes agreeing on one element (`p` and `q` aliasing one
    /// library) resolve that element.
    fn foreign_prefixed_clause(
        &self,
        file: FileId,
        face: StringId,
    ) -> Option<(&'a Container, FileId)> {
        let lib = self.graph.library_of_file(file)?;
        let mut found: Option<(&'a Container, FileId)> = None;
        let mut tried: Vec<StringId> = Vec::new();
        for prefix in self.graph.import_edges(lib).iter().filter_map(|e| e.prefix) {
            if tried.contains(&prefix) {
                continue;
            }
            tried.push(prefix);
            let Some(hit) =
                prefixed_container(self.graph, self.namespaces, self.index, lib, prefix, face)
            else {
                continue;
            };
            match found {
                Some((_, owner)) if owner != hit.1 => return None,
                Some(_) => {}
                None => found = Some(hit),
            }
        }
        found
    }

    /// The dead-edge probe for a clause face ([`supertype_hop_dead`]'s
    /// prefixed arm): a dotted spelling the prefix table cannot answer
    /// (unregistered prefix, absent local) is the oracle's
    /// `undefined_class`-on-the-continuation — the InvalidType dead edge —
    /// regardless of the bare name's fate; a resolved-but-unindexed target
    /// is the broken chain, not dead. A bare spelling keeps
    /// [`Engine::supertype_hop_dead`].
    fn clause_hop_dead(
        &self,
        writer: StringId,
        file: FileId,
        lib: LibraryId,
        name: StringId,
    ) -> bool {
        if file == self.file
            && let Some(prefix) = self.prefixed_clause_prefix(writer, name)
        {
            let target_lib = self.graph.library_of_file(file).unwrap_or(lib);
            return match self
                .namespaces
                .lookup_prefixed(self.graph, target_lib, prefix, name)
            {
                Lookup::Found(NamespaceEntry::Decl(owner)) => self.index.get(owner).is_none(),
                _ => true,
            };
        }
        self.supertype_hop_dead(name, lib, file)
    }

    /// Parses one class-family header's parameter window and supertype
    /// clauses starting after the family keyword; returns the scan position
    /// past the header (`{` body skipped whole, `;` mixin application
    /// consumed). A header that does not parse faithfully records nothing.
    fn scan_one_header(&self, start: usize, map: &mut HeaderClauses) -> usize {
        let toks = self.tokens.as_slice();
        let lex_at = |i: usize| toks.get(i).map(|_| self.tokens.lexeme(self.source, i));
        let mut i = start;
        // The declaring name is the identifier right after the keyword; its
        // UTF-16 offset is the declaration's identity (`name_range.offset`
        // at the listener's `arm_modifier_checks` site).
        if i >= toks.len() {
            return i;
        }
        let name_offset = toks[i].utf16_offset;
        let Some(name) = self.intern_checked(i) else {
            return i;
        };
        i += 1;
        // The type-parameter window: `<...>` with top-level identifiers the
        // parameters (a bound after `extends` is skipped until the next `,`).
        let mut params: Vec<StringId> = Vec::new();
        if lex_at(i) == Some("<") {
            let mut depth = 1usize;
            let mut after_extends = false;
            i += 1;
            while i < toks.len() && depth > 0 {
                match lex_at(i) {
                    Some("<") => depth += 1,
                    Some(">") => depth -= 1,
                    Some(",") if depth == 1 => after_extends = false,
                    Some("extends") if depth == 1 => after_extends = true,
                    _ if depth == 1 && !after_extends => {
                        if matches!(lex_at(i + 1), Some(",") | Some(">") | Some("extends"))
                            && let Some(id) = self.intern_checked(i)
                        {
                            params.push(id);
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
        }
        // The clause window: supertype faces until the body opens.
        let mut supers: Vec<SuperEntry> = Vec::new();
        let mut faithful = true;
        let mut end = i;
        while let Some(lex) = lex_at(end) {
            match lex {
                "{" | ";" => break,
                "extends" | "with" | "implements" | "on" | "=" => {
                    // The `=` head is the named mixin application's
                    // superclass slot; a plain `extends` head upgrades to
                    // the same alias-super arm when the header also
                    // carries a `with` clause — decided at emit time
                    // (the scan keeps the written clause tag verbatim).
                    let tag = match lex {
                        "extends" => SuperTag::Extends,
                        "with" => SuperTag::With,
                        "implements" => SuperTag::Implements,
                        "on" => SuperTag::On,
                        _ => SuperTag::AliasHead,
                    };
                    end += 1;
                    loop {
                        match self.scan_clause_type(end, tag, &mut supers) {
                            Some(next) => end = next,
                            None => {
                                faithful = false;
                                break;
                            }
                        }
                        match lex_at(end) {
                            Some(",") => end += 1,
                            _ => break,
                        }
                    }
                    if !faithful {
                        break;
                    }
                }
                _ => {
                    faithful = false;
                    break;
                }
            }
        }
        if !faithful {
            // Recovery residue: skip to the next header boundary so the
            // scan resynchronizes without parsing member bodies.
            while let Some(lex) = lex_at(end) {
                if lex == "{" {
                    return self.tokens.matching_end(end).map_or(toks.len(), |c| c + 1);
                }
                if lex == ";" {
                    return end + 1;
                }
                end += 1;
            }
            return end;
        }
        map.by_offset
            .insert(name_offset, HeaderClause { params, supers });
        map.last_of_name.insert(name, name_offset);
        if lex_at(end) == Some("{") {
            self.tokens.matching_end(end).map_or(toks.len(), |c| c + 1)
        } else {
            end + 1
        }
    }

    /// Parses one clause type at `i` — `Head`, `p.Head`, `Head<...args...>`,
    /// `Head?`, `dynamic`/`void` — appending one [`SuperEntry`] to `supers`
    /// (the stored `args` value is the face's whole `Ty::Named`; its own
    /// `args` list is the real argument list — `face_args`/`mixin_env`
    /// unwrap it) and returns the position past it. `None` marks the clause
    /// unfaithful (a non-identifier head, an unbalanced group, a function-
    /// or record-typed argument): the header contributes no facts. The
    /// recorded `span` is the oracle's clause-check anchor — the bare head,
    /// or `p` through `Head` for a prefixed face
    /// (`resolution_visitor.dart:1235-1237`).
    fn scan_clause_type(
        &self,
        start: usize,
        tag: SuperTag,
        supers: &mut Vec<SuperEntry>,
    ) -> Option<usize> {
        let toks = self.tokens.as_slice();
        let lex_at = |i: usize| toks.get(i).map(|_| self.tokens.lexeme(self.source, i));
        let tok_span = |i: usize| {
            let tok = toks.get(i)?;
            Some(SourceRange {
                offset: tok.utf16_offset,
                length: self.utf16_of((tok.byte_offset + tok.byte_len) as usize)
                    - tok.utf16_offset,
            })
        };
        let mut i = start;
        match lex_at(i)? {
            "dynamic" => {
                supers.push(SuperEntry {
                    tag,
                    head: StringId::NONE,
                    prefix: None,
                    span: tok_span(i)?,
                    args: vec![Ty::Dynamic],
                });
                Some(i + 1)
            }
            "void" => {
                supers.push(SuperEntry {
                    tag,
                    head: StringId::NONE,
                    prefix: None,
                    span: tok_span(i)?,
                    args: vec![Ty::Void],
                });
                Some(i + 1)
            }
            _ => {
                let first = self.intern_checked(i)?;
                let head_start = toks.get(i)?.utf16_offset;
                i += 1;
                // A prefixed clause face (`p.C`): the prefix stays the
                // spelling's head, the local name becomes the leaf — the
                // same bare-leaf convention the element model keeps.
                let (prefix, head) = if lex_at(i) == Some(".")
                    && self.intern_checked(i + 1).is_some()
                {
                    let local = self.intern_checked(i + 1)?;
                    i += 2;
                    (Some(first), local)
                } else {
                    (None, first)
                };
                let last = toks.get(i - 1)?;
                let head_end =
                    self.utf16_of((last.byte_offset + last.byte_len) as usize);
                let mut args: Vec<Ty> = Vec::new();
                if lex_at(i) == Some("<") {
                    i = self.scan_type_args(i, &mut args)?;
                }
                let nullable = lex_at(i) == Some("?");
                if nullable {
                    i += 1;
                }
                let head_ty = if args.is_empty() {
                    Ty::Named {
                        name: head,
                        nullable,
                        args: crate::model::ArgListId::EMPTY,
                    }
                } else {
                    Ty::Named {
                        name: head,
                        nullable,
                        args: self.boundary.intern_list(&args),
                    }
                };
                supers.push(SuperEntry {
                    tag,
                    head,
                    prefix,
                    span: SourceRange {
                        offset: head_start,
                        length: head_end - head_start,
                    },
                    args: vec![head_ty],
                });
                Some(i)
            }
        }
    }

    /// Parses a balanced `<...>` type-argument group into `args`, returning
    /// the position past the closing `>`. Nested groups recurse; an
    /// argument that is not a plain named type (a `Function` tail, a record
    /// group, a bare `Function` head) bails `None` — the unmodelable-shape
    /// posture the bounds capture keeps.
    fn scan_type_args(&self, start: usize, args: &mut Vec<Ty>) -> Option<usize> {
        let toks = self.tokens.as_slice();
        let lex_at = |i: usize| toks.get(i).map(|_| self.tokens.lexeme(self.source, i));
        let mut i = start;
        if lex_at(i) != Some("<") {
            return None;
        }
        i += 1;
        loop {
            let ty = match lex_at(i)? {
                "dynamic" => {
                    i += 1;
                    Ty::Dynamic
                }
                "void" => {
                    i += 1;
                    Ty::Void
                }
                _ => {
                    let id = self.intern_checked(i)?;
                    i += 1;
                    let mut nested: Vec<Ty> = Vec::new();
                    if lex_at(i) == Some("<") {
                        i = self.scan_type_args(i, &mut nested)?;
                    }
                    let nullable = lex_at(i) == Some("?");
                    if nullable {
                        i += 1;
                    }
                    if nested.is_empty() {
                        Ty::Named {
                            name: id,
                            nullable,
                            args: crate::model::ArgListId::EMPTY,
                        }
                    } else {
                        Ty::Named {
                            name: id,
                            nullable,
                            args: self.boundary.intern_list(&nested),
                        }
                    }
                }
            };
            args.push(ty);
            match lex_at(i)? {
                "," => i += 1,
                ">" => return Some(i + 1),
                _ => return None,
            }
        }
    }

    /// The token at `i` interned as an identifier spelling (`None` for
    /// anything the clause grammar cannot treat as a type head or a
    /// parameter name — operators, group punctuation, literals).
    fn intern_checked(&self, i: usize) -> Option<StringId> {
        let lexeme = self.tokens.lexeme(self.source, i);
        let first = lexeme.as_bytes().first()?;
        if first.is_ascii_alphabetic() || *first == b'_' || *first == b'$' {
            Some(self.intern(lexeme))
        } else {
            None
        }
    }

    /// The name-level signature verdict: `true` only when the equal-kind
    /// members are decuably incompatible — a positional-arity mismatch, a
    /// named-parameter set mismatch, or a type conflict. The official
    /// verdict is one subtype question on the member function types
    /// (upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/error/correct_override.dart:36-39`
    /// `isCorrectOverrideOf`: `_typeSystem.isSubtypeOf` of the override's
    /// function type against the super member's), and the function-subtype
    /// rule fixes the variance once for every member kind — result
    /// covariant, parameters contravariant (upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/dart/element/subtype.dart:98` and `:117`).
    /// This face encodes that rule in exactly one place: the parameter
    /// faces ask inherited → declared, the result face declared →
    /// inherited, and every type question runs the M1 assignability
    /// ([`Engine::assignable`]; the inherited member was
    /// boundary-projected, so both sides compare through one pool). Pairs
    /// the assignability face cannot decide — distinct non-null named
    /// heads outside the final-primitive table — fall to the name lattice
    /// ([`NameWorld::decide_subtype`]): a declared or core subtype edge
    /// proves the pair comparable, a closed walk proves it incomparable,
    /// and only an undecided pair (an unresolvable head or a cut walk —
    /// forward references, type parameters, cross-file chains) suppresses,
    /// the under-report direction.
    ///
    /// The InvalidType arm (`subtype.dart:274-276` — `T0 is InvalidType ||
    /// T1 is InvalidType` answers the subtype question `true`, so the
    /// override is satisfied and stays silent): a `Ty::Named` whose head
    /// resolves to no type in the *member's defining scope* is the oracle's
    /// `InvalidType` — a broken import's name, a misspelling, an ambiguous
    /// import, a prefix, or a non-type element (the same set
    /// [`Engine::named_type_is_type`] refuses, asked in the declaring
    /// file's scope rather than the diagnosing one). The leniency applies
    /// at the compared pair's own head: an invalid head on either operand
    /// makes the pair compatible outright, while an invalid *argument*
    /// inside an otherwise-resolved pair still descends through
    /// `assignable`'s covariant-argument arm — matching the oracle, where
    /// `List<InvalidType> <: Set<int>` still fails on the head walk.
    /// `owner` is `(defining file, declaring container name)` per side;
    /// the file's scope resolves the head and the container's library
    /// climbs to it via [`Engine::owner_library_of`].
    pub(crate) fn override_broken(
        &self,
        declared: &MemberEntry,
        declared_owner: (FileId, StringId),
        inherited: &MemberEntry,
        inherited_owner: (FileId, StringId),
    ) -> bool {
        // A generic-method override unifies the two signatures' method type
        // parameters by POSITION — the official subtype question runs over
        // fresh variables, so `QueueList<T> cast<T>()` overrides
        // `List<R> cast<R>()` (both spell "the method's first parameter";
        // `QueueList<X> <: List<X>` through the with-application). The raw
        // spellings are unrelated names (`QueueList<T>` vs `List<R>`), so
        // the declared entry renames onto the inherited spellings before
        // any face comparison.
        let unified;
        let declared = if declared.type_params > 0
            && declared.type_params == inherited.type_params
            && declared.type_param_names.len() == inherited.type_param_names.len()
            && !declared.type_param_names.is_empty()
            && declared.type_param_names != inherited.type_param_names
        {
            let env: FxHashMap<StringId, Ty> = declared
                .type_param_names
                .iter()
                .zip(inherited.type_param_names.iter())
                .filter(|(from, to)| **from != StringId::NONE && **to != StringId::NONE)
                .map(|(from, to)| (*from, Ty::named(*to)))
                .collect();
            let mut renamed = self.substitute_entry(declared, &env);
            // The renamed view spells the TARGET's parameter names, so its
            // own name list must say so too: every reader below recognises
            // a type-parameter spelling through `type_param_names`
            // ([`Self::override_decide`]'s InvalidType gate, its reflexive
            // arm and its bound walk), and a renamed signature still
            // carrying the declared spellings has every renamed leaf read
            // as an unresolvable head — the InvalidType leniency then
            // answers "compatible" in BOTH directions, which silently
            // swallows a genuine bound mismatch (`f<A, B extends A>` over
            // `f<T, S extends num>`: the bound pair `A`/`num` unifies to
            // `T`/`num` and must fail, upstream `type_system.dart:1601-1618`
            // compares the two bounds after substituting each side onto the
            // shared fresh parameters).
            renamed.type_param_names = inherited.type_param_names.clone();
            unified = renamed;
            &unified
        } else {
            declared
        };
        // The generic-function gate (upstream Dart SDK source,
        // `pkg/analyzer/lib/src/dart/element/subtype.dart:338-345`
        // `_isFunctionSubtypeOf` → `type_system.dart:1581-1635`
        // `relateTypeParameters`): the two signatures must declare the same
        // number of type parameters AND each pair of bounds must be equal
        // under the unified spellings, else the subtype question fails
        // outright — `void f<T, S>(T)` over `void f<T>(T)` and
        // `f<T extends int>` over `f<T extends num>` are broken overrides
        // whatever the parameter and return faces say. A bounds list that
        // does not line up with the name list (a payload decoded before the
        // field shipped) suppresses the comparison — the under-report
        // direction.
        if declared.type_params != inherited.type_params {
            return true;
        }
        // The single type-decision point for every face below: the M1
        // assignability first, the name lattice on its `None`. No consumer
        // of this verdict re-derives either half — [`Engine::override_decide`]
        // encodes it once.
        let world = self.name_world();
        let decide = |from: (Ty, (FileId, StringId), &MemberEntry),
                      to: (Ty, (FileId, StringId), &MemberEntry)| {
            let (from_ty, from_scope, from_entry) = from;
            let (to_ty, to_scope, to_entry) = to;
            self.override_decide(
                from_ty, from_scope, from_entry, to_ty, to_scope, to_entry, &world,
            )
        };
        let declared_at = |ty: Ty| (ty, declared_owner, declared);
        let inherited_at = |ty: Ty| (ty, inherited_owner, inherited);
        // A field's face is its declared type; every executable's is its
        // return type.
        let member_ty = |e: &MemberEntry| {
            if e.kind == MemberKind::Field {
                e.var_ty
            } else {
                e.return_ty
            }
        };
        // The bound pairs, equal under the unified spellings
        // (`type_system.dart:1601-1618`: `isEqualTo` per pair after both
        // sides instantiate onto the fresh parameters — the declared side
        // already rides the inherited spellings through `unified`, so the
        // two bound lists compare pairwise). Equality is the mutual
        // subtype question: `num` over `num?` fails one leg, `int` over
        // `num` fails the other. Two absent bounds coincide outright (the
        // upstream `bound1 ??= dynamic` pair, both legs decided by the
        // same placeholder); a bound written on exactly one side reads
        // `dynamic` against the other and fails. An undecidable pair
        // (`None` — an unresolvable head in a bound, the lattice's blind
        // spot) suppresses, the under-report direction.
        if declared.type_param_bounds.len() == declared.type_param_names.len()
            && inherited.type_param_bounds.len() == inherited.type_param_names.len()
        {
            for (d_bound, i_bound) in declared
                .type_param_bounds
                .iter()
                .zip(inherited.type_param_bounds.iter())
            {
                if *d_bound == Ty::Dynamic && *i_bound == Ty::Dynamic {
                    continue;
                }
                let forward = decide(declared_at(*d_bound), inherited_at(*i_bound));
                let backward = decide(inherited_at(*i_bound), declared_at(*d_bound));
                if matches!(forward, Some(false)) || matches!(backward, Some(false)) {
                    return true;
                }
            }
        }
        match (declared.formals.as_deref(), inherited.formals.as_deref()) {
            (Some(a), Some(b)) => {
                // The setter faces compare their FUNCTION TYPE's parameter
                // list, which always holds exactly one slot
                // ([`setter_type_formals`]) — the written list of a malformed
                // declaration (`set foo() {}`, its arity reported by
                // `wrong_number_of_parameters_for_setter`) is not the type
                // the oracle compares, and reading it fabricated an
                // `invalid_override` between two `void Function(dynamic)`
                // faces (`test_setter_single_setter_withoutParameter`). A
                // face whose list the model never captured keeps the
                // arm below's suppression (the under-report direction).
                let setter_declared;
                let setter_inherited;
                let (a, b) =
                    if declared.kind == MemberKind::Setter && inherited.kind == MemberKind::Setter
                    {
                        setter_declared = setter_type_formals(Some(a));
                        setter_inherited = setter_type_formals(Some(b));
                        (&setter_declared, &setter_inherited)
                    } else {
                        (a, b)
                    };
                // The function-subtype arity rule, not an equality: the
                // override must accept every call the super member accepts
                // (upstream Dart SDK source,
                // pkg/analyzer/lib/src/error/correct_override.dart:36-39 —
                // one subtype question on the member function types), so
                // fewer positional parameters breaks, and a positional
                // parameter may be ADDED when it is optional
                // (`Buf toUint8List([Buf? bytes])` over `Buf
                // toUint8List()` — archive's stream classes). A newly
                // required positional rejects the super's shorter calls;
                // the model's required positionals lead their list, so the
                // flag counts decide it.
                //
                // Counts are equivalent to the upstream per-index walk
                // (upstream Dart SDK source,
                // pkg/analyzer/lib/src/dart/element/subtype.dart:338
                // `_isFunctionSubtypeOf`, positional arms `:362-383`): a
                // declared required positional needs a required super
                // positional AT ITS INDEX, which — the required group
                // forming one contiguous head — is exactly
                // `a_required <= b_required`; a declared optional
                // positional accepts any super positional, so no per-index
                // rule survives past the counts; and the leftover-super
                // check (`:429-431` `gIndex < gParameters.length` → false)
                // is the length comparison. Both directions are pinned.
                if a.positional.len() < b.positional.len() {
                    return true;
                }
                let a_required = a.positional.iter().filter(|f| f.required).count();
                let b_required = b.positional.iter().filter(|f| f.required).count();
                if a_required > b_required {
                    return true;
                }
                // The named set: every inherited name must survive (a
                // dropped named parameter rejects calls the super accepts),
                // and `required` may only tighten onto a parameter that is
                // required in the super — a newly required parameter (added
                // or strengthened) rejects calls without it. An added
                // optional named parameter is the valid-override widening
                // (oracle-verified: `void f(int x, {int? y})` over
                // `void f(int x)` is clean). Upstream spells the same three
                // verdicts as its name-sorted walk (`:392-415`): a super
                // name absent from the declaration fails (`:409-411`), a
                // declared name absent from the super is allowed only while
                // optional (`:403-408`), and an equal-name pair fails
                // exactly when the declaration is the required side
                // (`:394-396`).
                for f in &b.named {
                    match a.named_formal(f.name) {
                        None => return true,
                        Some(d) if d.required && !f.required => return true,
                        Some(_) => {}
                    }
                }
                if a.named
                    .iter()
                    .any(|d| d.required && b.named_formal(d.name).is_none())
                {
                    return true;
                }
                // The parameter faces, contravariant (the function-subtype
                // rule's parameter arm): the declared parameter must accept
                // the inherited parameter's values. A `covariant` formal is
                // exempt — the oracle rewrites the DECLARED parameter's type
                // to `Object?` before the subtype question
                // (correct_override.dart:70-93
                // `_computeThisTypeForSubtype`), so a covariant parameter
                // never makes the override invalid; its narrowing is
                // enforced by the runtime check instead. The flag is
                // propagated along the override chain
                // (`_inheritCovariance`, inheritance_manager3.dart:1063-1130),
                // so an INHERITED covariant formal marks this parameter too
                // (`B.m` over `A.m(covariant num)` passes however it
                // narrows or widens); both sides therefore skip the face.
                for (d, i) in a.positional.iter().zip(&b.positional) {
                    if d.covariant || i.covariant {
                        continue;
                    }
                    match decide(inherited_at(i.ty), declared_at(d.ty)) {
                        Some(false) => return true,
                        Some(true) => {}
                        None => return false,
                    }
                }
                for d in &a.named {
                    if d.covariant {
                        continue;
                    }
                    let Some(i) = b.named_formal(d.name) else {
                        return false;
                    };
                    if i.covariant {
                        continue;
                    }
                    match decide(inherited_at(i.ty), declared_at(d.ty)) {
                        Some(false) => return true,
                        Some(true) => {}
                        None => return false,
                    }
                }
            }
            (None, None) => {}
            // A parameter-list mix on equal kinds is recovery residue.
            _ => return false,
        }
        // The result face, covariant (the function-subtype rule's return
        // arm): the declared result must fit the inherited one.
        matches!(
            decide(
                declared_at(member_ty(declared)),
                inherited_at(member_ty(inherited))
            ),
            Some(false)
        )
    }

    /// The single type-decision point behind every [`Engine::override_broken`]
    /// face — the M1 assignability first, the name lattice on its `None`.
    /// Both operands read through their type aliases first: the oracle
    /// compares resolved function types, so a parameterless alias's
    /// nullability joins the comparison (`typedef WhenComplete = void
    /// Function(...)?` — the spelled `WhenComplete?` and `WhenComplete` are
    /// the same type; riverpod's `create` override). The fold is identity
    /// for every non-alias spelling, and an alias to an unmodelable RHS
    /// keeps the written head — the same under-fold posture `fold_alias_ty`
    /// keeps.
    ///
    /// An unmodeled TARGET slot (`Ty::Unknown`) decides nothing: it holds
    /// no comparable type, so no break may be reported from it — answers
    /// `None`, the caller's under-report direction.
    ///
    /// The InvalidType arm (`subtype.dart:274-276` — `T0 is InvalidType ||
    /// T1 is InvalidType` answers the subtype question `true`, so the
    /// override is satisfied and stays silent): a `Ty::Named` whose head
    /// resolves to no type in the *member's defining scope* is the oracle's
    /// `InvalidType`. The leniency applies at the compared pair's own head:
    /// an invalid head on either operand makes the pair compatible
    /// outright, while an invalid *argument* inside an otherwise-resolved
    /// pair still descends through `assignable`'s covariant-argument arm —
    /// matching the oracle, where `List<InvalidType> <: Set<int>` still
    /// fails on the head walk. `scope` is `(defining file, declaring
    /// container name)` per side.
    fn override_decide(
        &self,
        from_ty: Ty,
        from_scope: (FileId, StringId),
        from_entry: &MemberEntry,
        to_ty: Ty,
        to_scope: (FileId, StringId),
        to_entry: &MemberEntry,
        world: &NameWorld<'a>,
    ) -> Option<bool> {
        let from_ty = self.fold_alias_ty(from_ty);
        let to_ty = self.fold_alias_ty(to_ty);
        // An UNMODELED target slot (`Ty::Unknown` — a names-only SDK
        // signature, an unwritten column, a face the model could not
        // resolve) carries no comparable type, so it can never *fail* an
        // override: every arm below would answer `Some(false)` from a blank
        // (and the `dynamic`/`void` source arm would too), fabricating a
        // break on each half-modeled face. Undecided is the under-report
        // direction the caller already grants.
        if matches!(to_ty, Ty::Unknown) {
            return None;
        }
        if let Ty::Named { name, .. } = from_ty
            && self.named_head_is_invalid(name, from_scope, from_entry)
        {
            return Some(true);
        }
        if let Ty::Named { name, .. } = to_ty
            && self.named_head_is_invalid(name, to_scope, to_entry)
        {
            return Some(true);
        }
        // The override faces read the SUBTYPE matrix, not the
        // dynamic-dispatch assignment face (`subtype.dart:52-66`'s top
        // rules): `Right Top` — `dynamic`, `void`, and `Object?` take every
        // source, so `dynamic Function()` and `void Function()` are no
        // subtypes of `int Function()` while every result satisfies a
        // `void`/`dynamic`/`Object?` inherited face. `Left Top` — a
        // `dynamic`/`void` source is a subtype only of a top target:
        // `void m()` over `int m()` breaks, `int m()` over `void m()` and
        // `dynamic m()` over `void m()` do not (a `void`/`dynamic`
        // *expression* assigns anywhere, but the override question asks the
        // subtype relation). Decided here rather than through `assignable`,
        // whose arms carry the assignment face's leniency.
        let is_top = |ty: Ty| {
            matches!(ty, Ty::Dynamic | Ty::Void)
                || matches!(
                    ty,
                    Ty::Named {
                        name,
                        nullable: true,
                        ..
                    } if name == self.intern("Object")
                )
        };
        if is_top(to_ty) {
            return Some(true);
        }
        if matches!(from_ty, Ty::Dynamic | Ty::Void) {
            return Some(false);
        }
        // A MEMBER's own type parameter (`subtype.dart:300-320`'s Variable
        // arms): `T <: X` holds iff `T`'s bound is a subtype of `X`
        // (`dynamic` for an unwritten bound, so only a top target passes),
        // and `X <: T` holds only for `Never` or the same variable. The two
        // distinct type variables of an override pair are therefore
        // unrelated unless their bounds relate them (`void m<T extends
        // num>(T)` over `void m<T>(String)`, and the SDK's `Set.removeWhere`
        // pair). CONTAINER parameters never reach here — the super-face
        // substitution rewrote them (`instantiate_inherited`), and an
        // unsubstituted one means an undecidable edge, which the caller
        // already suppressed.
        if let Ty::Named {
            name,
            nullable: false,
            ..
        } = from_ty
            && from_entry.type_param_names.contains(&name)
        {
            // The same variable on both sides relates reflexively (an
            // identical generic signature is no break) — the bound walk must
            // never see that pair.
            if matches!(
                to_ty,
                Ty::Named {
                    name: to_name,
                    nullable: false,
                    ..
                } if to_name == name
            ) {
                return Some(true);
            }
            let bound = from_entry
                .type_param_names
                .iter()
                .position(|param| *param == name)
                .and_then(|index| from_entry.type_param_bounds.get(index).copied())
                .unwrap_or(Ty::Dynamic);
            if bound == from_ty {
                return Some(false);
            }
            return self.override_decide(
                bound,
                from_scope,
                from_entry,
                to_ty,
                to_scope,
                to_entry,
                world,
            );
        }
        if let Ty::Named {
            name,
            nullable: false,
            ..
        } = to_ty
            && to_entry.type_param_names.contains(&name)
        {
            return Some(
                matches!(from_ty, Ty::Never)
                    || matches!(
                        from_ty,
                        Ty::Named {
                            name: from_name,
                            nullable: false,
                            ..
                        } if from_name == name
                    ),
            );
        }
        self.assignable(from_ty, to_ty, false).or_else(|| {
            let (
                Ty::Named {
                    name: from_name,
                    nullable: false,
                    ..
                },
                Ty::Named {
                    name: to_name,
                    nullable: false,
                    ..
                },
            ) = (from_ty, to_ty)
            else {
                return None;
            };
            if from_name == to_name {
                // Same-head pairs stay with the covariant-argument rule;
                // the lattice decides distinct heads only.
                return None;
            }
            world
                .decide_subtype(from_name, to_name)
                .or_else(|| self.dead_edge_subtype(from_name, from_scope, to_name))
        })
    }

    /// Every transitive superinterface's OWN `spelling` member of the
    /// verifier's kind, instantiated at the edge arguments — the
    /// `CovariantParametersVerifier`'s face set (upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/error/correct_override.dart:96-138`:
    /// `classHierarchy.implementedInterfaces` yields every TRANSITIVE
    /// interface, and `_correspondingMember` reads that interface element's
    /// own declarations — `InterfaceElementImpl.getMethod/getGetter
    /// /getSetter` walk the element's own tables, so an intermediate class
    /// that declares the name contributes beside its ancestors, which is why
    /// the nearest hit alone under-reports). The walk follows the written
    /// clauses (`extends`, the `with` mixins, `implements`, the mixin `on`
    /// constraints) transitively; the implicit `Object`/`Enum` face stays
    /// out — its signatures carry no parameter a covariant mark could
    /// correspond to (`toString`, `hashCode`, `index` are nullary, `==`
    /// declares no covariant parameter). Each row is the substituted
    /// signature plus its declaring interface's name and file (the message's
    /// overridden slots).
    fn covariant_own_faces(
        &self,
        container: StringId,
        spelling: StringId,
        face_kind: MemberKind,
        facts: &mut Option<HeaderClauses>,
    ) -> Vec<(MemberEntry, StringId, FileId)> {
        if face_kind == MemberKind::Getter {
            // A getter face declares no parameter list: no covariant mark
            // and no verifier row can exist.
            return Vec::new();
        }
        let mut out: Vec<(MemberEntry, StringId, FileId)> = Vec::new();
        let Some(decl) = self
            .index
            .get(self.file)
            .and_then(|f| f.container(container))
        else {
            return out;
        };
        // (face, its writer, writer file, writer scope library, hop chain).
        let mut stack: Vec<(
            StringId,
            StringId,
            FileId,
            LibraryId,
            Vec<(StringId, FileId)>,
        )> = Vec::new();
        for face in decl
            .superclass
            .iter()
            .copied()
            .chain(decl.with_clause.iter().copied())
            .chain(decl.implements_clause.iter().copied())
            .chain(decl.on_clause.iter().copied())
        {
            stack.push((face, container, self.file, self.lib, Vec::new()));
        }
        let mut seen: FxHashSet<(StringId, FileId)> = FxHashSet::default();
        while let Some((face, writer, file, lib, chain)) = stack.pop() {
            // A clause cycle (or a pathological depth) stops this branch.
            if chain.len() >= 32 {
                continue;
            }
            let Some((found, found_owner)) = self.resolve_clause_face(writer, file, lib, face)
            else {
                continue;
            };
            if !seen.insert((face, found_owner)) {
                continue;
            }
            let mut hop_chain = chain;
            hop_chain.push((face, found_owner));
            if let Some(hit) = self.face_own_hit(
                found_owner,
                face,
                found,
                spelling,
                lib,
                hop_chain.clone(),
            ) {
                let kind_matches = match face_kind {
                    MemberKind::Getter => {
                        matches!(hit.entry().kind, MemberKind::Getter | MemberKind::Field)
                    }
                    _ => hit.entry().kind == face_kind,
                };
                if kind_matches {
                    // The own member's effective signature first — the
                    // element model already inferred its unwritten slots
                    // (`instance_member_inferrer.dart` on the declaring
                    // container); the raw `Unknown` columns would suppress
                    // the verifier's pair comparison.
                    let inferred;
                    let hit_entry: &MemberEntry = match self.inferred_inherited(
                        &hit, spelling, facts,
                    ) {
                        Some(entry) => {
                            inferred = entry;
                            &inferred
                        }
                        None => hit.entry(),
                    };
                    match self.instantiate_inherited(&hop_chain, hit_entry, container, facts) {
                        Instantiated::Unchanged => {
                            out.push((hit_entry.clone(), face, found_owner));
                        }
                        Instantiated::Applied(env) => {
                            out.push((self.substitute_entry(hit_entry, &env), face, found_owner));
                        }
                        Instantiated::Undecidable => {}
                    }
                }
            }
            // The hop's own supertypes join `implementedInterfaces` too.
            let found_lib = self.owner_library_of(found_owner, face).unwrap_or(lib);
            for next in found
                .superclass
                .iter()
                .copied()
                .chain(found.with_clause.iter().copied())
                .chain(found.implements_clause.iter().copied())
                .chain(found.on_clause.iter().copied())
            {
                stack.push((next, face, found_owner, found_lib, hop_chain.clone()));
            }
        }
        out
    }

    /// The oracle's covariance propagation (`_inheritCovariance`,
    /// `inheritance_manager3.dart:1063-1130`, and the declared-member face
    /// of `instance_member_inferrer.dart`'s `_inferParameterCovariance` /
    /// `_isCovariantSetter`): a member's parameter is covariant when the
    /// member itself or any overridden member declares it so — upstream
    /// mutates the flag on the overriding element during inference, so the
    /// candidate `namedCandidates[name]` carries is the *propagated* mark
    /// (a mid-chain `B.m(dynamic)` redeclaring `A.m(covariant dynamic)`
    /// still answers covariant). The overridden members are
    /// `namedCandidates[name]` — each DIRECT superinterface's interface-map
    /// signature (the post-merge `interface.map[name]`, one per clause
    /// face), which [`Engine::override_face_member`]'s collected hits
    /// answer; the transitive own-declaration walk
    /// ([`Engine::covariant_own_faces`]) is the verifier's face set, never
    /// this one's — a covariant mark on an ancestor whose member was
    /// overridden out upstream stays out of the marks here. Correspondence
    /// is `_correspondingParameter`'s: positionals by index, named by name.
    /// `None` = no mark was added (the caller keeps its own entry — the
    /// common uncovariant shape pays the scan only).
    fn inherit_covariance(
        &self,
        entry: &MemberEntry,
        spelling: StringId,
        candidates: &[&FaceHit<'a>],
        facts: &mut Option<HeaderClauses>,
    ) -> Option<MemberEntry> {
        let formals = entry.formals.as_deref()?;
        let mut covariant_positional: Vec<usize> = Vec::new();
        let mut covariant_named: Vec<StringId> = Vec::new();
        let mut visited: FxHashSet<(FileId, StringId)> = FxHashSet::default();
        for hit in candidates {
            self.candidate_covariant_marks(
                hit,
                spelling,
                0,
                &mut visited,
                facts,
                &mut covariant_positional,
                &mut covariant_named,
            );
        }
        let needs_marks = formals
            .positional
            .iter()
            .enumerate()
            .any(|(index, p)| !p.covariant && covariant_positional.contains(&index))
            || formals
                .named
                .iter()
                .any(|p| !p.covariant && covariant_named.contains(&p.name));
        if !needs_marks {
            return None;
        }
        let mut marked = entry.clone();
        if let Some(marked_formals) = marked.formals.as_deref_mut() {
            for (index, p) in marked_formals.positional.iter_mut().enumerate() {
                if covariant_positional.contains(&index) {
                    p.covariant = true;
                }
            }
            for p in marked_formals.named.iter_mut() {
                if covariant_named.contains(&p.name) {
                    p.covariant = true;
                }
            }
        }
        Some(marked)
    }

    /// One candidate's *effective* covariant marks — its declared marks plus
    /// the marks the propagation itself would have set on it (upstream
    /// `_inferParameterCovariance` runs on every declared member, so the
    /// candidate `namedCandidates[name]` supplies already carries the marks
    /// it overrode). The propagation set is recursive: for the candidate's
    /// declaring container, each direct clause face's merged member (the
    /// same [`Engine::face_member_in`] walk, rooted at that container's own
    /// file and library) contributes its own effective marks — the spine of
    /// winning candidates, not the whole transitive own-declaration set.
    /// `visited` keys the recursion per declaring container: a clause cycle
    /// or a shared ancestor walks once. Out parameters are the unioned
    /// positional indices and named-parameter names the chain marks.
    fn candidate_covariant_marks(
        &self,
        hit: &FaceHit<'a>,
        spelling: StringId,
        depth: usize,
        visited: &mut FxHashSet<(FileId, StringId)>,
        facts: &mut Option<HeaderClauses>,
        covariant_positional: &mut Vec<usize>,
        covariant_named: &mut Vec<StringId>,
    ) {
        if let Some(formals) = hit.entry().formals.as_deref() {
            for (index, p) in formals.positional.iter().enumerate() {
                if p.covariant && !covariant_positional.contains(&index) {
                    covariant_positional.push(index);
                }
            }
            for p in &formals.named {
                if p.covariant && !covariant_named.contains(&p.name) {
                    covariant_named.push(p.name);
                }
            }
        }
        // The declaring container's own clause faces — the same list
        // `check_override` builds (superclass or the implicit Object/Enum
        // face, the `with` mixins, `implements`, `on`). Synthesized hits
        // (the frozen top-face members, an induced setter) name no modeled
        // container; their declared marks already rode the entry. The depth
        // cap mirrors the clause-walk's 32-hop bound — a degenerate chain
        // keeps the marks collected so far (the under-report direction).
        if depth >= 32 || !visited.insert((hit.origin, hit.face_name)) {
            return;
        }
        let Some(decl) = self
            .index
            .get(hit.origin)
            .and_then(|f| f.container(hit.face_name))
        else {
            return;
        };
        let mut faces: Vec<StringId> = Vec::with_capacity(
            usize::from(decl.superclass.is_some())
                + decl.with_clause.len()
                + decl.implements_clause.len()
                + decl.on_clause.len(),
        );
        match decl.superclass {
            Some(superclass) => faces.push(superclass),
            None => match decl.kind {
                ContainerKind::Class => faces.push(self.intern("Object")),
                ContainerKind::Enum => faces.push(self.intern("Enum")),
                ContainerKind::Mixin if decl.on_clause.is_empty() => {
                    faces.push(self.intern("Object"))
                }
                _ => {}
            },
        }
        faces.extend(decl.with_clause.iter().copied());
        faces.extend(decl.implements_clause.iter().copied());
        faces.extend(decl.on_clause.iter().copied());
        let owner_lib = self
            .owner_library_of(hit.origin, hit.face_name)
            .unwrap_or(self.lib);
        // The merge's substitution root: a same-file declaring container's
        // header facts live in this file's scan; a foreign one passes a
        // spelling no header declares (the unsubstituted first-candidate
        // posture — marks never depend on the substituted types, only the
        // validity pick does).
        let merge_container = if hit.origin == self.file {
            hit.face_name
        } else {
            self.intern("")
        };
        for face in faces {
            // The candidate's own override spine for the same spelling is
            // its propagation source — `namedCandidates[spelling]` built
            // from the declaring container's direct clause faces.
            if let Some(sub) = self.face_member_in(
                hit.origin,
                hit.face_name,
                owner_lib,
                merge_container,
                face,
                spelling,
                facts,
            ) {
                self.candidate_covariant_marks(
                    &sub,
                    spelling,
                    depth + 1,
                    visited,
                    facts,
                    covariant_positional,
                    covariant_named,
                );
            }
        }
    }

    /// An inherited hit's effective signature — the oracle's instance-member
    /// inference runs on EVERY element's own members
    /// (`instance_member_inferrer.dart` `_inferExecutable` /
    /// `_inferAccessorOrField`): an unwritten parameter or return slot on
    /// the declaring container's member infers from THAT container's
    /// combined superinterface signature at model build, so the model row
    /// the oracle compares is never `Unknown`. The walk-side read here is
    /// the raw collected member (the model stores only the written
    /// columns), so the inference replays against the hit's own declaring
    /// container — `B { method29(a) }` reads its combined signature from
    /// `B`'s own clause faces (`A.method29(int)`), not from the diagnosed
    /// `Class`'s. `None` = no implicit slot or no modeled declaring
    /// container (a synthesized hit — its `Unknown` columns genuinely mean
    /// `dynamic`, the frozen faces' own posture); the caller keeps the raw
    /// entry.
    fn inferred_inherited(
        &self,
        hit: &FaceHit<'a>,
        spelling: StringId,
        facts: &mut Option<HeaderClauses>,
    ) -> Option<MemberEntry> {
        let entry = hit.entry();
        if !has_implicit_slot(entry) || entry.kind == MemberKind::Field {
            return None;
        }
        let decl = self
            .index
            .get(hit.origin)
            .and_then(|f| f.container(hit.face_name))?;
        // The declaring container's direct superinterface faces — the same
        // clause set `candidate_covariant_marks` walks (superclass or the
        // implicit Object/Enum face, the `with` mixins, `implements`,
        // `on`).
        let mut faces: Vec<StringId> = Vec::with_capacity(
            usize::from(decl.superclass.is_some())
                + decl.with_clause.len()
                + decl.implements_clause.len()
                + decl.on_clause.len(),
        );
        match decl.superclass {
            Some(superclass) => faces.push(superclass),
            None => match decl.kind {
                ContainerKind::Class => faces.push(self.intern("Object")),
                ContainerKind::Enum => faces.push(self.intern("Enum")),
                ContainerKind::Mixin if decl.on_clause.is_empty() => {
                    faces.push(self.intern("Object"))
                }
                _ => {}
            },
        }
        faces.extend(decl.with_clause.iter().copied());
        faces.extend(decl.implements_clause.iter().copied());
        faces.extend(decl.on_clause.iter().copied());
        let owner_lib = self
            .owner_library_of(hit.origin, hit.face_name)
            .unwrap_or(self.lib);
        let merge_container = if hit.origin == self.file {
            hit.face_name
        } else {
            self.intern("")
        };
        // The kind-matched candidate set: a method reads its own kind; a
        // getter face reads getter-or-field candidates, a setter face
        // setter-or-non-final-field candidates (the oracle's
        // `_inferAccessorOrField` pairing — an accessor's combined
        // signature comes from its counterpart kind too). A setter hit
        // already resolves under the `name=` spelling the caller's lookup
        // carried — the candidates' own containers read the same spelling.
        let mut candidates: Vec<FaceHit<'a>> = Vec::new();
        for face in faces.iter().copied() {
            if let Some(sub) = self.face_member_in(
                hit.origin,
                hit.face_name,
                owner_lib,
                merge_container,
                face,
                spelling,
                facts,
            ) {
                let kind_ok = match entry.kind {
                    MemberKind::Getter => {
                        matches!(sub.entry().kind, MemberKind::Getter | MemberKind::Field)
                    }
                    MemberKind::Setter => {
                        sub.entry().kind == MemberKind::Setter
                            || (sub.entry().kind == MemberKind::Field && !sub.entry().is_final)
                    }
                    kind => sub.entry().kind == kind,
                };
                if kind_ok {
                    candidates.push(sub);
                }
            }
        }
        if candidates.is_empty() {
            return None;
        }
        let combined = self.combined_inherited(merge_container, candidates, facts);
        let dynamic = Ty::Dynamic;
        let mut effective = entry.clone();
        match entry.kind {
            MemberKind::Method => {
                if effective.return_ty == Ty::UNKNOWN {
                    effective.return_ty = combined
                        .as_ref()
                        .map(|c| c.return_ty)
                        .unwrap_or(dynamic);
                }
                if let Some(formals) = effective.formals.as_deref_mut() {
                    let slots = combined.as_ref().and_then(|c| c.formals.as_deref());
                    for (index, param) in formals.positional.iter_mut().enumerate() {
                        if param.ty == Ty::UNKNOWN {
                            param.ty = slots
                                .and_then(|f| f.positional.get(index))
                                .map(|p| p.ty)
                                .unwrap_or(dynamic);
                        }
                    }
                    for param in formals.named.iter_mut() {
                        if param.ty == Ty::UNKNOWN {
                            param.ty = slots
                                .and_then(|f| f.named.iter().find(|p| p.name == param.name))
                                .map(|p| p.ty)
                                .unwrap_or(dynamic);
                        }
                    }
                }
            }
            MemberKind::Getter => {
                if effective.return_ty == Ty::UNKNOWN {
                    effective.return_ty = combined
                        .as_ref()
                        .map(|c| {
                            if c.kind == MemberKind::Field {
                                c.var_ty
                            } else {
                                c.return_ty
                            }
                        })
                        .unwrap_or(dynamic);
                }
            }
            MemberKind::Setter => {
                if let Some(formals) = effective.formals.as_deref_mut()
                    && let Some(param) = formals.positional.first_mut()
                    && param.ty == Ty::UNKNOWN
                {
                    param.ty = combined
                        .as_ref()
                        .and_then(|c| {
                            if c.kind == MemberKind::Field {
                                Some(c.var_ty)
                            } else {
                                c.formals
                                    .as_deref()
                                    .and_then(|f| f.positional.first())
                                    .map(|p| p.ty)
                            }
                        })
                        .unwrap_or(dynamic);
                }
            }
            MemberKind::Field => unreachable!("the early return above"),
        }
        Some(effective)
    }

    /// One setter-face candidate's effective first-parameter mark — the
    /// `_isCovariantSetter` probe (`instance_member_inferrer.dart`'s
    /// overridden-setter any) asked on the propagated flag, not the raw
    /// declared one: a setter whose own declaration dropped the keyword but
    /// whose override spine carries it still answers covariant.
    fn candidate_first_param_covariant(
        &self,
        hit: &FaceHit<'a>,
        spelling: StringId,
        facts: &mut Option<HeaderClauses>,
    ) -> bool {
        let mut positional: Vec<usize> = Vec::new();
        let mut named: Vec<StringId> = Vec::new();
        let mut visited: FxHashSet<(FileId, StringId)> = FxHashSet::default();
        self.candidate_covariant_marks(
            hit,
            spelling,
            0,
            &mut visited,
            facts,
            &mut positional,
            &mut named,
        );
        positional.contains(&0)
    }

    /// Whether a `Ty::Named` head is the oracle's `InvalidType` — a
    /// spelling that resolves to no type in the member's *defining* scope
    /// (`named_type_resolver.dart`'s `_resolveToElement`: a miss is
    /// `undefined_class`, a non-type element is `notAType`, an ambiguous
    /// import is a `MultiplyDefinedElement`, a bare prefix is never a type —
    /// all four leave the annotation invalid). `owner` is the member's
    /// `(defining file, declaring container)`: the file's own declarations
    /// plus its library's import face decide, and the container's and the
    /// member's type parameters count as types (declaration-scoped
    /// [`MicroKind::TypeParam`] rows ride the file's declaration set; the
    /// member's own ride [`MemberEntry::type_param_names`]). A name the
    /// index cannot model (a namespace hit whose declaring file carries no
    /// model — the SDK projection's names-only stubs) counts as a type, the
    /// same under-report posture [`Engine::named_type_is_type`] keeps.
    fn named_head_is_invalid(
        &self,
        name: StringId,
        owner: (FileId, StringId),
        entry: &MemberEntry,
    ) -> bool {
        let (file, container) = owner;
        if self.members.primitive_ids.contains(&name) || entry.type_param_names.contains(&name) {
            return false;
        }
        // The declaring container's own members are never types
        // (`InstanceScope.lookup` — the same arm
        // [`Engine::named_type_is_type`] keeps): a member name shadows any
        // same-spelled outer class for type uses inside that container.
        if self
            .index
            .get(file)
            .and_then(|f| f.container(container))
            .is_some_and(|c| c.members.contains_key(&name))
        {
            return true;
        }
        let lib = self.owner_library_of(file, container).unwrap_or(self.lib);
        match lookup_scoped(self.graph, self.namespaces, self.index, lib, file, name) {
            ScopedHit::Local(decl) => !Self::decl_is_type(decl.kind),
            ScopedHit::Found(NamespaceEntry::Decl(owner_file)) => self
                .index
                .get(owner_file)
                .and_then(|f| f.decl(name))
                .is_some_and(|decl| !Self::decl_is_type(decl.kind)),
            ScopedHit::Found(NamespaceEntry::Prefix)
            | ScopedHit::Ambiguous(_)
            | ScopedHit::NotFound => true,
        }
    }

    /// The member's name-level signature text (the message's type slots, in
    /// the oracle's function-type shape): nullary faces render
    /// `{ty} Function()`, methods render their parameter list, setters
    /// `void Function({ty})`. Absent declared types render `dynamic` — the
    /// implicit declared type the oracle would display.
    pub(crate) fn override_signature_text(&self, entry: &MemberEntry) -> String {
        match entry.kind {
            MemberKind::Method => {
                let params = match entry.formals.as_deref() {
                    Some(formals) => {
                        // The required positionals lead the list (the
                        // collector's invariant); the optional tail renders
                        // in brackets — the oracle's inherited-signature
                        // slot spells `void Function([int])`.
                        let split = formals
                            .positional
                            .iter()
                            .position(|f| !f.required)
                            .unwrap_or(formals.positional.len());
                        let mut parts: Vec<String> = formals.positional[..split]
                            .iter()
                            .map(|f| self.declared_ty_text(f.ty))
                            .collect();
                        if split < formals.positional.len() {
                            let optional = formals.positional[split..]
                                .iter()
                                .map(|f| self.declared_ty_text(f.ty))
                                .collect::<Vec<_>>()
                                .join(", ");
                            parts.push(format!("[{optional}]"));
                        }
                        if !formals.named.is_empty() {
                            // The oracle renders a function type's named
                            // parameters in name order, not declaration
                            // order (`FunctionTypeImpl` sorts the named set
                            // in its constructor, upstream Dart SDK source
                            // `pkg/analyzer/lib/src/dart/element/type.dart:153-185`).
                            let mut named_params: Vec<&Formal> =
                                formals.named.iter().collect();
                            named_params.sort_by_key(|f| self.strings.resolve(f.name));
                            let named = named_params
                                .iter()
                                .map(|f| {
                                    let text = format!(
                                        "{} {}",
                                        self.declared_ty_text(f.ty),
                                        self.strings.resolve(f.name)
                                    );
                                    if f.required {
                                        format!("required {text}")
                                    } else {
                                        text
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join(", ");
                            parts.push(format!("{{{named}}}"));
                        }
                        parts.join(", ")
                    }
                    None => String::new(),
                };
                // A generic member renders its own type parameters between
                // `Function` and the parameter list, bounds included
                // (`void Function<T extends num>(T)`; the oracle's function
                // type text keeps the declared bounds, upstream Dart SDK
                // source `pkg/analyzer/lib/src/dart/element/type.dart`'s
                // `FunctionTypeImpl` textual face). A bound recorded as
                // `dynamic` is the absent-bound default and stays unwritten.
                let type_params = if entry.type_param_names.is_empty() {
                    String::new()
                } else {
                    let rendered = entry
                        .type_param_names
                        .iter()
                        .enumerate()
                        .map(|(index, name)| {
                            let name = self.strings.resolve(*name);
                            match entry.type_param_bounds.get(index) {
                                Some(bound) if *bound != Ty::Dynamic && *bound != Ty::UNKNOWN => {
                                    format!("{name} extends {}", self.declared_ty_text(*bound))
                                }
                                _ => name.to_owned(),
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("<{rendered}>")
                };
                format!(
                    "{} Function{type_params}({params})",
                    self.declared_ty_text(entry.return_ty)
                )
            }
            MemberKind::Getter | MemberKind::Field => {
                let ty = if entry.kind == MemberKind::Field {
                    entry.var_ty
                } else {
                    entry.return_ty
                };
                format!("{} Function()", self.declared_ty_text(ty))
            }
            MemberKind::Setter => {
                let ty = entry
                    .formals
                    .as_deref()
                    .and_then(|f| f.positional.first())
                    .map_or(Ty::UNKNOWN, |p| p.ty);
                format!("void Function({})", self.declared_ty_text(ty))
            }
        }
    }

    /// The declared-type text as the oracle renders it: `dynamic` for an
    /// absent type (the implicit declared type), `Null`/`void` keep their
    /// spellings, a named type gains the syntactic `?`. Unlike
    /// [`Engine::ty_display`] (whose unknowns stay empty for its
    /// suppression-shaped callers), an override signature always names its
    /// types.
    pub(crate) fn declared_ty_text(&self, ty: Ty) -> String {
        match ty {
            Ty::Unknown | Ty::Dynamic => "dynamic".to_string(),
            Ty::Null => "Null".to_string(),
            Ty::Never => "Never".to_string(),
            Ty::Void => "void".to_string(),
            named @ (Ty::Named { .. } | Ty::Record { .. } | Ty::Function { .. }) => {
                self.render_ty(named)
            }
        }
    }
}

/// The sealed⇒abstract fold pins (#2202): the element model folds `sealed`
/// into abstractness (`element.dart:150`,
/// `isAbstract = isAbstract || isSealed`), so a sealed declaration never
/// enters the completeness / implementation-override /
/// mustBeOverridden loops, and a sealed direct superclass grants its
/// concrete subclass no concreteness exemption.
#[cfg(test)]
mod sealed_abstract_fold_tests {
    use crate::collector::collect;
    use crate::diagnose::{FileReport, StrictModes, fold_unit, merge_source_ordered};
    use crate::library::ElementIndex;
    use crate::model::MicroKind;
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_parser::ParserFeatures;
    use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder, Namespaces};
    use dartq_scanner::ScanConfig;
    use dartq_source::{FileTable, StringTable};
    use std::path::Path;

    /// Diagnoses `source` as the sole file of a library with no imports —
    /// the single-file rig, local to this module.
    fn report(source: &str) -> FileReport {
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
        let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
        let features = ParserFeatures::from_feature_set(set);
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
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let fold = fold_unit(
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
            // The same derivation the engine's fold call site makes from
            // the settled set (the `inference-update-2` release gate).
            set.is_enabled(dartq_language::Feature::InferenceUpdate2),
        );
        let mut report = fold.report;
        report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
        report
    }

    fn code_names(report: &FileReport) -> Vec<&'static str> {
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    }

    /// (a) A sealed declaration is abstract at the element level: an
    /// unimplemented interface member never reports on it.
    #[test]
    fn sealed_class_never_reports_missing_interface_member() {
        let report = report("abstract class I { void m(); }\nsealed class S implements I {}\n");
        let names = code_names(&report);
        assert!(names.is_empty(), "expected no rows: {names:?}");
    }

    /// (b) The extends-head face: sealed over an abstract base stays
    /// silent.
    #[test]
    fn sealed_class_over_abstract_base_stays_silent() {
        let report = report("abstract class B { void m(); }\nsealed class S extends B {}\n");
        let names = code_names(&report);
        assert!(names.is_empty(), "expected no rows: {names:?}");
    }

    /// (c) The concrete subclass of a sealed superclass still reports: a
    /// sealed direct super grants no concreteness exemption.
    #[test]
    fn concrete_subclass_of_sealed_reports_missing_member() {
        let report = report("sealed class S { void m(); }\nclass C extends S {}\n");
        let names = code_names(&report);
        assert_eq!(
            names,
            ["non_abstract_class_inherits_abstract_member"],
            "expected the concrete subclass row: {names:?}"
        );
        assert_eq!(
            report.diagnostics[0].message,
            "Missing concrete implementation of 'S.m'."
        );
    }

    /// (d) The declared-`abstract` control: the gate still admits concrete
    /// subclasses of genuinely abstract supers.
    #[test]
    fn concrete_subclass_of_declared_abstract_still_reports() {
        let report = report("abstract class B { void m(); }\nclass D extends B {}\n");
        let names = code_names(&report);
        assert_eq!(
            names,
            ["non_abstract_class_inherits_abstract_member"],
            "expected the concrete subclass row: {names:?}"
        );
        assert_eq!(
            report.diagnostics[0].message,
            "Missing concrete implementation of 'B.m'."
        );
    }

    /// The written `abstract sealed` combination is a modifier violation
    /// and the class stays abstract for the completeness face — the fold
    /// keys on the sealed bit, not on the abstract keyword's absence. (The
    /// combination row itself ships twice — the parser emits it
    /// parse-tier and the engine's combination face emits it again; a
    /// pre-existing dedup defect outside this fold's scope — so the pin
    /// asserts the completeness silence and the row shapes, not a count.)
    #[test]
    fn abstract_sealed_combo_stays_abstract_for_completeness() {
        let report =
            report("abstract class I { void m(); }\nabstract sealed class S implements I {}\n");
        let names = code_names(&report);
        assert!(
            names.iter().all(|n| *n == "abstract_sealed_class"),
            "only the combination violation may report: {names:?}"
        );
    }

    // —— #2301: super-face type-argument substitution ————————————————

    /// The drift `TypeConverter` shape: `W extends NATC<D?, S?>` inherits
    /// `TC.fromSql` two hops down with the intermediate hop's parameters in
    /// the signature — the composed environment instantiates them to the
    /// override's own arguments. Oracle-verified (#2367): the live analyzer
    /// reports TWO `invalid_override` rows here — `W.fromSql`'s `S`
    /// parameter cannot accept the inherited `S?` (the intermediate hop
    /// widens the signature to `D? Function(S?)`), and `Bad.fromSql`'s
    /// broken return.
    #[test]
    fn substitution_composes_through_intermediate_hop() {
        let source = "class TC<D, S> {\n\
                      \x20 D fromSql(S dbValue) => throw '';\n\
                      }\n\
                      class NATC<D, S> extends TC<D?, S?> {}\n\
                      class W<D, S> extends NATC<D?, S?> {\n\
                      \x20 @override\n\
                      \x20 D fromSql(S dbValue) => throw '';\n\
                      }\n\
                      class Bad extends TC<int, String> {\n\
                      \x20 @override\n\
                      \x20 String fromSql(int dbValue) => throw '';\n\
                      }\n";
        let names = code_names(&report(source));
        let overrides: Vec<_> = names.iter().filter(|n| **n == "invalid_override").collect();
        assert_eq!(
            overrides.len(),
            2,
            "the broken parameter and the broken return both report: {names:?}"
        );
    }

    /// A raw generic `implements A` gives the hop an unknowable environment
    /// — the comparison suppresses instead of comparing bare parameter
    /// spellings (the oracle resolves the face to `A<dynamic>`, where the
    /// same shape is valid, so silence matches).
    #[test]
    fn undecidable_face_suppresses_override_comparison() {
        let report = report(
            "class A<T> {\n\
             \x20 T m() => throw '';\n\
             }\n\
             class C implements A {\n\
             \x20 @override\n\
             \x20 dynamic m() => throw '';\n\
             }\n",
        );
        let names = code_names(&report);
        assert!(
            !names.contains(&"invalid_override"),
            "the undecidable face must not report: {names:?}"
        );
    }

    // —— #2301: the frozen list face over names-only SDK stubs —————————

    /// `with ListMixin` over an unmodeled hop contributes the SDK list
    /// face: a bare class misses the base's four abstract forwarding
    /// points (`setter List.length` among them — the drift
    /// `DriftResultSet` row), and a class declaring all four stays silent.
    #[test]
    fn frozen_list_face_reports_missing_forwarding_points() {
        let bare = report("class C with ListMixin<int> {}\n");
        let names = code_names(&bare);
        assert!(
            names.contains(&"non_abstract_class_inherits_abstract_member"),
            "the bare with-face must report: {names:?}"
        );
        assert!(
            bare.diagnostics
                .iter()
                .any(|d| d.message.contains("setter List.length")),
            "the length setter must be among the missing: {:?}",
            bare.diagnostics
                .iter()
                .map(|d| &d.message)
                .collect::<Vec<_>>()
        );
        let full = report(
            "class D with ListMixin<int> {\n\
             \x20 int get length => 0;\n\
             \x20 set length(int v) {}\n\
             \x20 int operator [](int i) => 0;\n\
             \x20 void operator []=(int i, int v) {}\n\
             }\n",
        );
        let names = code_names(&full);
        assert!(
            !names.contains(&"non_abstract_class_inherits_abstract_member"),
            "declaring the four forwarding points must be silent: {names:?}"
        );
    }

    /// A user `ListMixin` that resolves with members contributes its own
    /// face — the frozen table must never shadow a real container.
    #[test]
    fn resolved_list_mixin_keeps_its_own_members() {
        let report = report(
            "class ListMixin<T> {\n\
             \x20 void add(T v) {}\n\
             }\n\
             class E with ListMixin<int> {}\n",
        );
        let names = code_names(&report);
        assert!(
            !names.contains(&"non_abstract_class_inherits_abstract_member"),
            "a resolved mixin face must not gain the frozen rows: {names:?}"
        );
    }

    // —— #2301: own-member shadowing in the concrete-implementation face —

    /// The class's own concrete member shadows the inherited one in the
    /// official `implemented` map AND seats the interface map (the oracle's
    /// `declared` first — `interface.map[name]` is the own member, so the
    /// concrete-implementation comparison is self-to-self and emits
    /// nothing). Oracle-verified (#2367): the live analyzer reports only
    /// `Bad.substitute`'s member-level `invalid_override` (`B <: M` fails
    /// against the `M` interface member) — no
    /// `invalid_implementation_override` fires for either class.
    #[test]
    fn own_member_shadows_inherited_concrete_implementation() {
        let source = "abstract class B {}\n\
                      abstract class M implements B {\n\
                      \x20 M substitute();\n\
                      }\n\
                      class Base {\n\
                      \x20 B substitute() => throw '';\n\
                      }\n\
                      class Good extends Base implements M {\n\
                      \x20 @override\n\
                      \x20 M substitute() => throw '';\n\
                      }\n\
                      class Bad extends Base implements M {\n\
                      \x20 @override\n\
                      \x20 B substitute() => throw '';\n\
                      }\n";
        let names = code_names(&report(source));
        let implementations: Vec<_> = names
            .iter()
            .filter(|n| **n == "invalid_implementation_override")
            .collect();
        assert_eq!(
            implementations.len(),
            0,
            "own members seat the interface map — no implementation row: {names:?}"
        );
        let overrides: Vec<_> = names.iter().filter(|n| **n == "invalid_override").collect();
        assert_eq!(
            overrides.len(),
            1,
            "only Bad's member-level verdict may report: {names:?}"
        );
    }

    // —— #2301: the `external` exclusion in the top-level default arm ——

    /// An `external` top-level header never expects optional-parameter
    /// defaults (the http `_fetch` row); a sibling without the modifier
    /// still reports.
    #[test]
    fn external_top_level_header_takes_no_defaults() {
        // `C` — a locally declared head — stands in for `int`: this rig
        // mounts no `dart:core`, so an `int` formal is the oracle's
        // `InvalidType`, whose `isPotentiallyNonNullable` gate suppresses
        // the row before the `external` arm under test ever sees it.
        let report = report("class C {}\nexternal C f(C a, [C b]);\nC g(C a, [C b]);\n");
        let names = code_names(&report);
        let defaults: Vec<_> = names
            .iter()
            .filter(|n| n.starts_with("missing_default_value_for_parameter"))
            .copied()
            .collect();
        // Oracle-verified (#2367): the live analyzer emits
        // `missing_default_value_for_parameter` — the bare code name, not
        // the `_positional` suffixed variant.
        assert_eq!(
            defaults,
            ["missing_default_value_for_parameter"],
            "only the non-external header may report: {names:?}"
        );
    }
}

/// The InvalidType-leniency and concrete-source pins (#2192): the oracle's
/// `isCorrectOverrideOf` is one subtype question whose `T0/T1 is
/// InvalidType` arm answers `true` (`subtype.dart:274-276`), so a member
/// signature carrying an unresolvable head always reads as a satisfied
/// override; and `implemented` — the concrete-implementation map the
/// `invalid_implementation_override` loop consults — carries own members,
/// mixins, and the super chain only (`inheritance_manager3.dart:333-351`),
/// never an `implements` face's members.
#[cfg(test)]
mod invalid_type_override_tests {
    use crate::collector::collect;
    use crate::diagnose::{FileReport, StrictModes, diagnose, fold_unit, merge_source_ordered};
    use crate::library::ElementIndex;
    use crate::model::MicroKind;
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_parser::ParserFeatures;
    use dartq_resolution::{
        Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder, Namespaces,
    };
    use dartq_scanner::ScanConfig;
    use dartq_source::{FileTable, StringTable};
    use std::path::Path;

    /// Diagnoses `source` as the sole file of a library with no imports —
    /// the same single-file rig the sibling module keeps.
    fn report(source: &str) -> FileReport {
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
        let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
        let features = ParserFeatures::from_feature_set(set);
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
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let fold = fold_unit(
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
        );
        let mut report = fold.report;
        report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
        report
    }

    fn code_names(report: &FileReport) -> Vec<&'static str> {
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    }

    /// The `with` face's concrete source: `C extends R with M` takes `R`'s
    /// chain member as the concrete implementation, never a member reached
    /// through `M`'s `implements` clause — `M implements I` contributes
    /// `I`'s members to the *interface* only (the oracle's `implemented`
    /// map, `inheritance_manager3.dart:333-351`). Before the
    /// `concrete_source` gate, the loop compared `I.contentLength` (`int?`)
    /// as the concrete implementation against `R.contentLength` (`int`)
    /// and reported `invalid_implementation_override` — the http
    /// `AbortableRequest`/`BaseRequest` shape.
    #[test]
    fn mixin_interface_member_is_no_concrete_implementation() {
        let report = report(
            "abstract class I { int? get contentLength; } \
             abstract class R extends I { @override int get contentLength => 0; } \
             abstract mixin class M implements I {} \
             final class C extends R with M {}",
        );
        let names = code_names(&report);
        assert!(
            !names.contains(&"invalid_implementation_override"),
            "I.x reached through M.implements is interface-only: {names:?}"
        );
    }

    /// The InvalidType arm on the result face: `Missing?` over `Missing`
    /// compares `InvalidType` against `InvalidType` upstream — the subtype
    /// question answers `true` regardless of the `?`, so the override is
    /// satisfied and stays silent (`subtype.dart:274-276`). Only the
    /// `undefined_class` rows for the `Missing` mentions report.
    #[test]
    fn invalid_head_nullability_pair_stays_silent() {
        let report = report(
            "class A { Missing f() => throw 0; } class B extends A { Missing? f() => throw 0; }",
        );
        let names = code_names(&report);
        assert!(
            !names.contains(&"invalid_override"),
            "an InvalidType operand satisfies the override: {names:?}"
        );
        assert!(
            names.iter().all(|n| *n == "undefined_class"),
            "only the Missing mentions may report: {names:?}"
        );
    }

    /// The negative control: a genuinely mismatched override — both heads
    /// resolvable, the declared side widened past the inherited — still
    /// reports `invalid_override` (`int? <: int` is the decidable break;
    /// the `class int` declarations stand the name world's primitive in
    /// for the fixture, the established rig convention).
    #[test]
    fn resolvable_mismatch_still_reports() {
        let report = report(
            "class int {} class A { int f() => int(); } class B extends A { int? f() => int(); }",
        );
        let names = code_names(&report);
        assert_eq!(names, ["invalid_override"], "{names:?}");
    }

    /// The cross-file family shape: the inherited member's type resolves
    /// to nothing in its *defining* file's scope (the broken import the
    /// corpus rows carry — `file_state.dart`'s `FileSource get source`).
    /// The subtype question is satisfied by the `InvalidType` arm on the
    /// defining side, so the per-member override verdict stays silent;
    /// the `undefined_class` rows for `Missing` remain.
    #[test]
    fn broken_import_member_type_stays_silent_cross_file() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file_a = files.intern(Path::new("a.dart"));
        let file_m = files.intern(Path::new("main.dart"));
        // `Missing` resolves to nothing in a.dart's scope — the broken
        // import face (a `URI_DOES_NOT_EXIST` import contributes no names).
        let source_a = "class P { Missing get s => throw 0; }";
        let source_m =
            "import 'a.dart';\nclass C extends P { @override Missing? get s => throw 0; }\n";
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
        assert!(scanned_a.diagnostics.is_empty() && scanned_m.diagnostics.is_empty());
        let elements_a = collect(source_a, &scanned_a.tokens, features, &strings);
        let elements_m = collect(source_m, &scanned_m.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib_a = builder.intern("package:other/a.dart");
        let lib_m = builder.intern("package:t/main.dart");
        let mut index = ElementIndex::default();
        index.insert(file_a, elements_a.clone());
        index.insert(file_m, elements_m.clone());
        for (name, decl) in elements_a.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
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
                prefix: None,
                deferred: false,
                non_library: false,
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
        let names = code_names(&report);
        assert!(
            !names.contains(&"invalid_override"),
            "InvalidType on the defining side satisfies the override: {names:?}"
        );
    }
}

/// The function-subtype verdict pins (#2281): the oracle's
/// `isCorrectOverrideOf` is one subtype question on the member function
/// types (correct_override.dart:36-39), and the function-subtype rule
/// accepts an override that adds an OPTIONAL positional parameter and
/// compares aliased spellings through their resolved types. Both pins were
/// live `invalid_override` false positives on the real corpus (archive's
/// `toUint8List([Uint8List? bytes])`, riverpod's `WhenComplete? create`).
#[cfg(test)]
mod function_subtype_override_tests {
    use crate::collector::collect;
    use crate::diagnose::{FileReport, StrictModes, diagnose, fold_unit, merge_source_ordered};
    use crate::library::ElementIndex;
    use crate::model::MicroKind;
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_parser::ParserFeatures;
    use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder, Namespaces};
    use dartq_scanner::ScanConfig;
    use dartq_source::{FileTable, StringTable};
    use std::path::Path;

    /// Diagnoses `source` as the sole file of a library with no imports —
    /// the same single-file rig the sibling modules keep.
    fn report(source: &str) -> FileReport {
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
        let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
        let features = ParserFeatures::from_feature_set(set);
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
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let fold = fold_unit(
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
        );
        let mut report = fold.report;
        report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
        report
    }

    fn code_names(report: &FileReport) -> Vec<&'static str> {
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    }

    /// An added optional positional parameter is the valid-override
    /// widening: the override accepts every call the super member accepts.
    #[test]
    fn added_optional_positional_parameter_is_a_valid_override() {
        let report = report(
            "class Buf {}\n\
             class Base {\n\
               Buf toUint8List() => Buf();\n\
             }\n\
             class Sub extends Base {\n\
               @override\n\
               Buf toUint8List([Buf? bytes]) => Buf();\n\
             }\n",
        );
        let names = code_names(&report);
        assert!(
            !names.contains(&"invalid_override"),
            "optional-positional addition is a valid override: {names:?}"
        );
    }

    /// A newly required positional parameter rejects the super's shorter
    /// calls — the arity rule's broken direction stays live.
    #[test]
    fn newly_required_positional_parameter_still_breaks() {
        let report = report(
            "class Base {\n\
               void f([int x]) {}\n\
             }\n\
             class Sub extends Base {\n\
               @override\n\
               void f(int x) {}\n\
             }\n",
        );
        let names = code_names(&report);
        assert!(
            names.contains(&"invalid_override"),
            "required-izing an optional positional breaks the override: {names:?}"
        );
    }

    /// Dropping a positional parameter the super has breaks the override.
    #[test]
    fn dropped_positional_parameter_still_breaks() {
        let report = report(
            "class Base {\n\
               void f(int x) {}\n\
             }\n\
             class Sub extends Base {\n\
               @override\n\
               void f() {}\n\
             }\n",
        );
        let names = code_names(&report);
        assert!(
            names.contains(&"invalid_override"),
            "fewer positional parameters breaks the override: {names:?}"
        );
    }

    /// A parameterless alias's nullability joins the comparison: with
    /// `typedef W = void Function()?;` (a nullable RHS — riverpod's
    /// `WhenComplete` shape), the spellings `W` and `W?` fold to the same
    /// resolved nullable function type.
    #[test]
    fn aliased_nullable_result_reads_through_the_alias() {
        let report = report(
            "typedef W = void Function()?;\n\
             class El {\n\
               W create() => () {};\n\
             }\n\
             class Sub extends El {\n\
               @override\n\
               W? create() => () {};\n\
             }\n",
        );
        let names = code_names(&report);
        assert!(
            !names.contains(&"invalid_override"),
            "W and W? fold to the same resolved function type: {names:?}"
        );
    }
}

/// The #2299 pins: the set-base frozen face (the html
/// `_CssClassSetImpl extends SetBase<String> implements CssClassSet`
/// shape — `SetBase.addAll`/`removeAll` implement the interface names)
/// and the field-declaration override faces (a field checks through its
/// implicit getter and — non-final only — implicit setter, the oracle's
/// `_checkDeclaredField`).
#[cfg(test)]
mod field_face_and_set_base_tests {
    use crate::collector::collect;
    use crate::diagnose::{FileReport, StrictModes, fold_unit, merge_source_ordered};
    use crate::library::ElementIndex;
    use crate::model::MicroKind;
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_parser::ParserFeatures;
    use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder, Namespaces};
    use dartq_scanner::ScanConfig;
    use dartq_source::{FileTable, StringTable};
    use std::path::Path;

    /// Diagnoses `source` as the sole file of a library with no imports —
    /// the single-file rig, local to this module.
    fn report(source: &str) -> FileReport {
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
        let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
        let features = ParserFeatures::from_feature_set(set);
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
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let fold = fold_unit(
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
        );
        let mut report = fold.report;
        report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
        report
    }

    fn code_names(report: &FileReport) -> Vec<&'static str> {
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    }

    /// The html shape minus the SDK: an abstract middle class
    /// `extends SetBase` (a dead hop whose spelling names the frozen
    /// set face) `implements` an abstract interface and implements the
    /// seven forwarding points itself (the `_CssClassSetImpl` posture).
    /// The frozen face's concrete rows implement the interface's own
    /// demands — the concrete subclass reports nothing, exactly like the
    /// oracle against the real `dart:collection`.
    #[test]
    fn set_base_concrete_rows_implement_interface_names() {
        let report = report(
            "class V {}\n\
             abstract class CssClassSet {\n\
             \x20 void addAll(V v);\n\
             \x20 void removeAll(V v);\n\
             }\n\
             abstract class Impl extends SetBase implements CssClassSet {\n\
             \x20 void add(v) {}\n\
             \x20 void contains(v) {}\n\
             \x20 void lookup(v) {}\n\
             \x20 void remove(v) {}\n\
             \x20 void toSet() {}\n\
             \x20 get iterator => 0;\n\
             \x20 get length => 0;\n\
             }\n\
             class C extends Impl {}\n",
        );
        let names = code_names(&report);
        assert!(
            !names.contains(&"non_abstract_class_inherits_abstract_member"),
            "the frozen concrete rows and Impl's own members cover the interface: {names:?}"
        );
    }

    /// The set face's abstract forwarding points still demand: a concrete
    /// subclass of the frozen `SetBase` face missing the seven reports
    /// them (the face's `SetBase`-owned rows), never the concrete ones.
    #[test]
    fn set_base_abstract_points_still_demand() {
        let report = report("class C extends SetBase {}\n");
        let missing = report
            .diagnostics
            .iter()
            .find(|d| d.code.name == "non_abstract_class_inherits_abstract_member")
            .expect("the seven forwarding points demand implementations");
        for name in [
            "SetBase.add",
            "SetBase.contains",
            "SetBase.lookup",
            "SetBase.remove",
        ] {
            assert!(
                missing.message.contains(name),
                "the demand names {name}: {}",
                missing.message
            );
        }
        assert!(
            missing.message.contains("and 3 more"),
            "the seven points render at the three-plus tier: {}",
            missing.message
        );
        assert!(
            !missing.message.contains("addAll"),
            "the concrete rows never demand: {}",
            missing.message
        );
    }

    /// The xml `mixin XmlHasAttributes implements XmlAttributesBase`
    /// shape: a field whose declared type is not a subtype of the
    /// interface getter's return type reports `invalid_override` at the
    /// field's name, message rendered on the field's implicit getter.
    #[test]
    fn field_getter_face_over_interface_getter_reports() {
        let report = report(
            "class NodeList {}\n\
             class TList {}\n\
             abstract class XmlAttributesBase {\n\
             \x20 TList get attributes => TList();\n\
             }\n\
             mixin XmlHasAttributes implements XmlAttributesBase {\n\
             \x20 final NodeList attributes = NodeList();\n\
             }\n",
        );
        let row = report
            .diagnostics
            .iter()
            .find(|d| d.code.name == "invalid_override")
            .expect("the field's getter face must report");
        assert_eq!(
            row.message,
            "'XmlHasAttributes.attributes' ('NodeList Function()') isn't a valid \
             override of 'XmlAttributesBase.attributes' ('TList Function()')."
        );
    }

    /// A field of the interface's own type stays silent — the getter face
    /// override is valid.
    #[test]
    fn field_getter_face_valid_stays_silent() {
        let report = report(
            "class TList {}\n\
             abstract class Base {\n\
             \x20 TList get attributes => TList();\n\
             }\n\
             class C extends Base {\n\
             \x20 final TList attributes = TList();\n\
             }\n",
        );
        let names = code_names(&report);
        assert!(
            !names.contains(&"invalid_override"),
            "a valid field getter face never reports: {names:?}"
        );
    }

    /// A non-final field's implicit setter face compares like a written
    /// setter: the declared type rides the parameter position, so an
    /// unfit type reports the setter variant with the setter message
    /// template.
    #[test]
    fn field_setter_face_over_written_setter_reports() {
        let report = report(
            "class TStr {}\n\
             class Zero {}\n\
             class A {\n\
             \x20 set x(TStr v) {}\n\
             }\n\
             class C extends A {\n\
             \x20 Zero x = Zero();\n\
             }\n",
        );
        let row = report
            .diagnostics
            .iter()
            .find(|d| d.code.name == "invalid_override")
            .expect("the field's setter face must report");
        assert_eq!(
            row.message,
            "The setter 'C.x' ('void Function(Zero)') isn't a valid override of \
             'A.x' ('void Function(TStr)')."
        );
    }

    /// A final field has no implicit setter: the setter face never
    /// compares, so a written super setter of an unfit type stays silent
    /// (the oracle's `field.setter == null` early return).
    #[test]
    fn final_field_has_no_setter_face() {
        let report = report(
            "class TStr {}\n\
             class Zero {}\n\
             class A {\n\
             \x20 set x(TStr v) {}\n\
             }\n\
             class C extends A {\n\
             \x20 final Zero x = Zero();\n\
             }\n",
        );
        let names = code_names(&report);
        assert!(
            names.iter().all(|n| *n != "invalid_override"),
            "a final field carries no setter face: {names:?}"
        );
    }

    /// The getter face against an inherited METHOD stays a kind-mismatch
    /// skip — the member-kind verifier owns that report, this face never
    /// manufactures it.
    #[test]
    fn field_getter_face_kind_mismatch_stays_silent() {
        let report = report(
            "class Zero {}\n\
             class A {\n\
             \x20 Zero x() => Zero();\n\
             }\n\
             class C extends A {\n\
             \x20 final Zero x = Zero();\n\
             }\n",
        );
        let names = code_names(&report);
        assert!(
            !names.contains(&"invalid_override"),
            "getter-vs-method is the kind verifier's business: {names:?}"
        );
    }
}

/// The #3001 `invalid_override` type-parameter pins: the generic-function
/// gate's count and bound-equality arms (upstream `relateTypeParameters`,
/// `type_system.dart:1581-1635`) — narrowed / dropped / added bounds,
/// count mismatches, and the renamed-spelling equality that must stay
/// silent.
#[cfg(test)]
#[path = "ticket_3001_tests.rs"]
mod ticket_3001_tests;

/// The #2515 pins: the SDK-units exposure wave's override-signature faces —
/// generic members render their own type parameters (bounds included) in the
/// message slots, a member's type parameter relates only through its bound,
/// and the completeness report names the interface map's most-specific
/// declaring member.
#[cfg(test)]
#[path = "ticket_2515_tests.rs"]
mod ticket_2515_tests;

/// The #3484 pins: `scan_clause_type`'s clause-entry spans are UTF-16
/// [`SourceRange`]s — BMP wide characters, astral surrogate pairs, CRLF
/// breaks, prefixed faces, and the name-anchored control row.
#[cfg(test)]
#[path = "ticket_3484_tests.rs"]
mod ticket_3484_tests;

/// The #3661 pins: the enum/ext-type exemption of the base-or-final
/// subtype verifier (the resolver's `checkElement` mounts on class,
/// class-type-alias and mixin only) plus the merged-element instance
/// scope of `conflicting_static_and_instance` — `augment`-container
/// members, primary-constructor declaring formals, the extension-type
/// representation field, and the non-leading-`augment` claim asymmetry.
#[cfg(test)]
#[path = "ticket_3661_tests.rs"]
mod ticket_3661_tests;
