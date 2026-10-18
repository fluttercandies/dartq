//! The semantic query face (#1056): one compilation unit's diagnosing
//! fold, published as an immutable, query-able projection.
//!
//! [`Analysis`] is the *query-able projection* of one unit's fold result
//! ([`crate::diagnose::fold_unit`]): the flow layer's read-type and
//! write-site rows, the diagnosing pass's published bare-reference
//! resolutions and type-reference positions, the unresolved-name rows,
//! and the flow facts verbatim. The implementation reuses the diagnosing
//! fold's existing machinery — nothing here re-analyzes; the queries are
//! lookups over what the fold already decided (the same decisions the
//! `undefined_*` families consult to avoid false positives), plus live
//! model walks ([`member_lookup`], `resolve_container`) for the
//! member/constructor faces.
//!
//! # Contract
//!
//! **Immutable after construction.** Every query is a `&self` synchronous
//! lookup; the struct holds only owned rows plus shared borrows of the
//! library context, so `Analysis` is `Send + Sync` whenever the context
//! tables are. Callers build one `Analysis` per file per run
//! ([`analyze_unit`], the same argument shape
//! [`crate::diagnose::diagnose_unit`] takes) and share it read-only
//! across rule dispatch; invalidation is whole-snapshot replacement (a
//! fresh fold replaces the old `Analysis` — no per-query revision
//! machinery).
//!
//! # Storage
//!
//! Span-keyed queries binary-search offset-sorted `Vec`s (a few hundred
//! to a few thousand rows per file — cache-friendly, no hashing); the
//! query span must be the row's own range (an identifier/type-reference
//! token's span — exact-offset match).
//!
//! # Under-reporting (the M1 contract)
//!
//! Every query answers `None`/`false` when the fold could not decide —
//! never a fabricated binding or type. Shapes outside the current
//! publication (member/prefixed reference rows, literals, inferred-type
//! receivers, top-level write tracking) are later increments, not
//! guarantees.

use dartq_flow::FlowFact;
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    LibraryClassModifiers, LibraryGraph, LibraryId, Lookup, NamespaceEntry, Namespaces,
};
use dartq_scanner::TokenTable;
use dartq_source::{FileId, SourceRange, StringId, StringTable};
use rustc_hash::FxHashSet;

use crate::diagnose::{NameRow, RefTarget, StrictModes, fold_unit};
use crate::library::{
    ElementIndex, ScopedHit, container_has_annotation, implements_interface, import_shadows,
    lookup_scoped, member_lookup, member_lookup_defining, prefixed_container, resolve_container,
    supertype_has_instance_members,
};
use crate::model::{
    Container, ContainerKind, FnSig, Formals, MemberEntry, MemberKind, MicroKind, TopDecl, Ty,
};

/// One unit's fold result, query-able. See the [module docs](self) for
/// the immutability contract and the storage shape.
pub struct Analysis<'a> {
    graph: &'a LibraryGraph,
    namespaces: &'a Namespaces,
    index: &'a ElementIndex,
    /// The unit's string table — the render face of later query
    /// increments (messages naming resolved spellings); the query
    /// consumers intern selector spellings through it, and the tests
    /// read expected names from it.
    strings: &'a StringTable,
    file: FileId,
    lib: LibraryId,
    /// Family (a): one row per reachable identifier read, offset-sorted —
    /// the fold's join of the flow rows into the semantic `Ty` space.
    read_types: Vec<(SourceRange, Ty)>,
    /// Family (c): the `FlowFact::WriteSite` rows, offset-sorted by
    /// declaration span; the classification parallels the spans.
    write_decls: Vec<SourceRange>,
    write_in_closure: Vec<bool>,
    /// Family (b): the published bare-reference resolutions,
    /// offset-sorted.
    refs: Vec<(SourceRange, StringId, RefTarget)>,
    /// The published type-reference positions, offset-sorted.
    type_uses: Vec<(SourceRange, StringId)>,
    /// The unit's unresolved-name rows (the report's names table).
    names: Vec<NameRow>,
    /// The unit's flow facts verbatim, offset-sorted.
    facts: Vec<FlowFact>,
}

/// Builds one unit's [`Analysis`]: the same fold
/// [`crate::diagnose::diagnose_unit`] runs, with the intermediates kept
/// instead of dropped. The argument shape is `diagnose_unit`'s.
#[must_use]
#[allow(clippy::too_many_arguments)] // the analysis context, one field each
pub fn analyze_unit<'a>(
    source: &str,
    tokens: &TokenTable,
    features: ParserFeatures,
    strings: &'a StringTable,
    file: FileId,
    lib: LibraryId,
    graph: &'a LibraryGraph,
    namespaces: &'a Namespaces,
    index: &'a ElementIndex,
    strict: StrictModes,
    class_modifiers: &LibraryClassModifiers,
) -> Analysis<'a> {
    let fold = fold_unit(
        source,
        tokens,
        features,
        strings,
        file,
        lib,
        graph,
        namespaces,
        index,
        strict,
        class_modifiers,
        // The field-promotion grant rides the diagnosing entrypoints'
        // argument (the caller derives it from the unit's settled feature
        // set). `true` here is observationally inert: every `Analysis`
        // output is flag-independent — `facts`/`read_types`/`refs`/
        // `type_uses` are flow/resolution-derived, and
        // `PromotedPropertyRead` rows emit regardless of the grant — and
        // the grant's only reader is the staged-diagnostic teardown
        // (`Engine::filter_staged`), whose survivors `Analysis` drops
        // (only `report.names` is kept).
        true,
    );
    let mut read_types = fold.read_types;
    let mut write_decls: Vec<SourceRange> = Vec::new();
    let mut write_in_closure: Vec<bool> = Vec::new();
    for fact in &fold.facts {
        if let FlowFact::WriteSite {
            decl_name_span,
            in_closure,
        } = *fact
        {
            write_decls.push(decl_name_span);
            write_in_closure.push(in_closure);
        }
    }
    let mut refs: Vec<(SourceRange, StringId, RefTarget)> = fold
        .refs
        .iter()
        .map(|r| (r.range, r.name, r.target))
        .collect();
    let mut type_uses = fold.type_uses;
    read_types.sort_unstable_by_key(|(range, _)| range.offset);
    // Keep the classification parallel to the spans after the sort.
    let mut order: Vec<usize> = (0..write_decls.len()).collect();
    order.sort_unstable_by_key(|&i| write_decls[i].offset);
    let sorted_decls: Vec<SourceRange> = order.iter().map(|&i| write_decls[i]).collect();
    let sorted_closure: Vec<bool> = order.iter().map(|&i| write_in_closure[i]).collect();
    refs.sort_unstable_by_key(|(range, _, _)| range.offset);
    type_uses.sort_unstable_by_key(|(range, _)| range.offset);
    Analysis {
        graph,
        namespaces,
        index,
        strings,
        file,
        lib,
        read_types,
        write_decls: sorted_decls,
        write_in_closure: sorted_closure,
        refs,
        type_uses,
        names: fold.report.names,
        facts: fold.facts,
    }
}

/// The resolved-reference face of one use site — the query projection of
/// the reference rows the index persists at name level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    /// The file whose model owns the target (the analyzed file for
    /// in-file and local targets).
    pub target_file: FileId,
    /// The target's lookup-spelling name as written at the site.
    pub target_name: StringId,
    /// The resolution class.
    pub kind: BindingKind,
}

/// The resolution class of a [`Binding`] — the M1 cut of the reference
/// kind face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingKind {
    /// A local binding of the enclosing executables (the flow layer owns
    /// its declaration span).
    Local,
    /// An in-scope type parameter.
    TypeParam,
    /// An import prefix.
    Prefix,
    /// A top-level or container declaration in `target_file`.
    Decl,
    /// An instance member of the enclosing container.
    Member,
    /// The `this` keyword — the enclosing container's instance face. The
    /// container's type answers through [`Analysis::type_of`] at the same
    /// span, so member walks on `this` resolve without re-resolving.
    This,
    /// Resolved but unnameable at M1 (extension-pool members, enum and
    /// `Object` synthesized members, ambiguous imports).
    Opaque,
}

/// The constructor face of a container: existence plus the formal table.
#[derive(Clone, Copy, Debug)]
pub struct CtorBinding<'a> {
    /// The file owning the container.
    pub file: FileId,
    /// The container's name (the unnamed formals' lookup spelling).
    pub container: StringId,
    /// The selector (or the unnamed face) names a declared constructor.
    pub exists: bool,
    /// The constructor's formal table (positional + named, with declared
    /// types) when declared.
    pub formals: Option<&'a Formals>,
}

/// The member face of a resolved `receiver.name` access.
#[derive(Clone, Copy, Debug)]
pub struct MemberBinding<'a> {
    /// The container the lookup walked from (the receiver's type name).
    pub container: StringId,
    /// The resolved member entry (kind, declared types, formals).
    pub entry: &'a MemberEntry,
}

impl<'a> Analysis<'a> {
    /// The unit's shared string table — the interning face the member
    /// and constructor queries take their `StringId` selectors from: a
    /// rule spells the selector it read and interns it through this
    /// table, the same one the fold published its rows in.
    #[must_use]
    pub fn strings(&self) -> &'a StringTable {
        self.strings
    }

    /// —— family (a): static type + nullability ————————————————————
    ///
    /// The flow type at one identifier read — promoted when a promotion
    /// stands, declared otherwise — in the semantic `Ty` space.
    /// `expr` must be the read's own identifier span; inferred-type reads
    /// (`var x = …`) answer [`Ty::Unknown`] (the name-level reality the
    /// fold shares).
    #[must_use]
    pub fn type_of(&self, expr: SourceRange) -> Option<Ty> {
        row_starting_at(&self.read_types, expr, |row| row.0.offset).map(|(_, ty)| *ty)
    }

    /// Whether the expression's flow type classifies non-nullable
    /// (official `isDefinitelyNonNull` — [`Ty::is_strictly_non_nullable`]).
    /// `false` when the span carries no read row.
    #[must_use]
    pub fn is_definitely_non_null(&self, expr: SourceRange) -> bool {
        self.type_of(expr).is_some_and(Ty::is_strictly_non_nullable)
    }

    /// Whether the expression's flow type is potentially null (official
    /// `isPotentiallyNull`: `Null`, `dynamic`, or a `?` projection — the
    /// nullable record type included).
    /// `false` when the span carries no read row.
    #[must_use]
    pub fn is_potentially_null(&self, expr: SourceRange) -> bool {
        self.type_of(expr).is_some_and(|ty| {
            matches!(
                ty,
                Ty::Null
                    | Ty::Dynamic
                    | Ty::Named { nullable: true, .. }
                    | Ty::Record { nullable: true, .. }
                    | Ty::Function { nullable: true, .. }
            )
        })
    }

    /// —— family (b): element binding & constructors ————————————————
    ///
    /// Where the fold's bare-reference resolution at `use_site` landed.
    /// Bare value references publish (reads, invocation callees);
    /// `None` on an unresolved name or a span the pass did not publish.
    #[must_use]
    pub fn binding_of(&self, use_site: SourceRange) -> Option<Binding> {
        let (name, target) = row_starting_at(&self.refs, use_site, |row| row.0.offset)
            .map(|(_, name, target)| (*name, *target))?;
        let (kind, target_file) = match target {
            RefTarget::Decl { file } => (BindingKind::Decl, file),
            RefTarget::Local => (BindingKind::Local, self.file),
            RefTarget::TypeParam => (BindingKind::TypeParam, self.file),
            RefTarget::Prefix => (BindingKind::Prefix, self.file),
            RefTarget::Member { .. } => (BindingKind::Member, self.file),
            RefTarget::This => (BindingKind::This, self.file),
            RefTarget::Opaque => (BindingKind::Opaque, self.file),
            // The miss families own the report — not a binding.
            RefTarget::Missing => return None,
        };
        Some(Binding {
            target_file,
            target_name: name,
            kind,
        })
    }

    /// Whether `binding`'s target is the synthetic SDK face: a declaration
    /// whose file the element index never carries (`<dart:*>` — the engine
    /// graph's SDK posture; a bare name resolved through the implicit
    /// dart:core import lands here). The upstream element-identity checks
    /// (`isDartCorePrint`, noop_primitive_operations.dart:60) read this
    /// face: a binding into the SDK is the core symbol, a workspace
    /// binding shadows it.
    #[must_use]
    pub fn is_sdk_binding(&self, binding: &Binding) -> bool {
        self.index.get(binding.target_file).is_none()
    }

    /// Whether this unit's declaring library spells `library meta;` — the
    /// resolved form of the oracle's declaring-library test for the
    /// package:meta annotations (`_isPackageMetaGetter`,
    /// `element.dart:2100-2102`: `element.library.name == 'meta'`). The
    /// name is the resolution graph's registered directive text
    /// ([`LibraryGraph::name`], engine-fed from the defining unit's
    /// `library` directive), so a part of a meta library answers true —
    /// its elements' `element.library` is the enclosing library — and a
    /// directive-less vendored meta copy answers false exactly as the
    /// oracle's `hasLiteral` does there.
    #[must_use]
    pub fn is_meta_library(&self) -> bool {
        self.graph.name(self.lib) == "meta"
    }

    /// The constructor face of the container a type-reference position
    /// names (`new C.named()`, `const C()`): existence plus the formal
    /// table. `type_name` must be the type-reference's own span;
    /// `selector` names a named constructor, `None` queries the unnamed
    /// face.
    #[must_use]
    pub fn resolve_constructor(
        &self,
        type_name: SourceRange,
        selector: Option<StringId>,
    ) -> Option<CtorBinding<'_>> {
        let name = row_starting_at(&self.type_uses, type_name, |row| row.0.offset)
            .map(|(_, name)| *name)?;
        let (container, file) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        )?;
        let (exists, formals) = match selector {
            Some(selector) => (
                container.constructors.contains(&selector),
                container.ctor_formals.get(&selector),
            ),
            None => (
                container.has_unnamed_constructor,
                container.ctor_formals.get(&name),
            ),
        };
        Some(CtorBinding {
            file,
            container: name,
            exists,
            formals: formals.map(std::convert::AsRef::as_ref),
        })
    }

    /// The constructor-formal face of a container already known by name —
    /// the declaration-side sibling of [`Analysis::resolve_constructor`]: a
    /// declaration carries no type-reference row to query, so the caller's
    /// own container name resolves through the same scope walk
    /// ([`resolve_container`]). `selector` names a named constructor,
    /// `None` the unnamed face (keyed by the container name, mirroring
    /// `resolve_constructor`).
    ///
    /// `None` under-reports: an unresolvable name or a container without
    /// the queried formal table.
    #[must_use]
    pub fn ctor_formals_of(
        &self,
        container: StringId,
        selector: Option<StringId>,
    ) -> Option<&'a Formals> {
        let (resolved, _) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        )?;
        let formals = match selector {
            Some(selector) => resolved.ctor_formals.get(&selector),
            None => resolved.ctor_formals.get(&container),
        };
        formals.map(std::convert::AsRef::as_ref)
    }

    /// The container a prefix-qualified name resolves to (`p.C` of a
    /// prefixed creation head `new p.C.named()`): `prefix` must be a
    /// registered `as` prefix of this unit's library and `container` a
    /// name in the prefix's merged import table — combinator filtered,
    /// independent of same-named names in the unit's own scope (upstream
    /// Dart SDK source, pkg/analyzer/lib/src/dart/resolver/
    /// named_type_resolver.dart's prefixed-type branch: the head goes
    /// through the import's `PrefixScope`, scope.dart:352-357, not the
    /// library scope). The bare path ([`Analysis::resolve_constructor`])
    /// cannot serve these heads: the prefixed head's type-use row carries
    /// the bare container name, which resolves in the unit's own scope —
    /// the wrong scope for a prefixed spelling.
    ///
    /// `None` under-reports: an unregistered prefix, a prefix whose target
    /// contributes nothing to the graph, a name the prefix's table lacks,
    /// and a resolved name that is not an indexed container. The consumer
    /// need is the stay-silent direction: a `p.Missing.new(…)` head
    /// resolves to nothing and the rule stays silent instead of
    /// over-reporting.
    #[must_use]
    pub fn resolve_prefixed_container(
        &self,
        prefix: StringId,
        container: StringId,
    ) -> Option<(&'a Container, FileId)> {
        prefixed_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            prefix,
            container,
        )
    }

    /// The member face of a `receiver.name` access: the superclass walk
    /// ([`member_lookup`]) from the receiver's resolved type — the
    /// receiver's read row (a declared-typed identifier, `C c; c.m()`)
    /// or its own container-name resolution (`C.m()`). Unknown-type
    /// receivers under-report `None`.
    #[must_use]
    pub fn member_of(&self, receiver: SourceRange, name: StringId) -> Option<MemberBinding<'_>> {
        if let Some(Ty::Named {
            name: container, ..
        }) = self.type_of(receiver)
        {
            return self.member_on(container, name);
        }
        let (spelled, target) = row_starting_at(&self.refs, receiver, |row| row.0.offset)
            .map(|(_, spelled, target)| (*spelled, *target))?;
        if matches!(target, RefTarget::Decl { .. }) {
            return self.member_on(spelled, name);
        }
        None
    }

    fn member_on(&self, container: StringId, member: StringId) -> Option<MemberBinding<'a>> {
        let entry = member_lookup(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
            member,
        )?;
        Some(MemberBinding { container, entry })
    }

    /// The declaration-side inherited-member face, keyed by the container's
    /// lookup name — a declaration has no receiver expression, so the
    /// site-keyed [`Analysis::member_of`] does not apply. The walk is
    /// [`member_lookup`]'s (file-local first, then the library namespace;
    /// superclass name edges + mixin application order, cross-file). The hit
    /// may sit on the container itself: the returned defining-container name
    /// equals `container` there, so the caller tells self-declarations from
    /// inherited ones.
    ///
    /// `concrete_only` skips abstract entries and keeps walking (the
    /// official `getMember(concrete: …)` cut — concreteness is the entry's
    /// [`MemberEntry::is_abstract`] bit).
    ///
    /// `None` under-reports: an unresolvable container name, a broken
    /// superclass edge, or a name absent from the whole chain. A miss is
    /// **not** proof of undefined — the extension pools sit outside this
    /// walk (see [`member_lookup`]).
    #[must_use]
    pub fn inherited_member(
        &self,
        container: StringId,
        name: StringId,
        concrete_only: bool,
    ) -> Option<(StringId, &'a MemberEntry)> {
        member_lookup_defining(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
            name,
            concrete_only,
        )
    }

    /// Whether any container on `container`'s supertype chain — the `with`
    /// mixins (application order) and the direct superclass, transitively;
    /// the same chain [`Analysis::inherited_member`] walks — declares at
    /// least one instance member. Stronger than any by-name lookup: the
    /// whole chain's member face, not one selector. The anchor's own members
    /// are not supertype members; a class written without `extends` sits at
    /// the written chain's end (the implicit `Object` root is outside the
    /// name-level model — its members never count).
    ///
    /// `None` = the chain is unknowable: the anchor does not resolve, or a
    /// written superclass/mixin name resolves to nothing indexed (an SDK
    /// type, an unanalyzed dependency) — that face could carry instance
    /// members, so the caller stays silent (the upstream-unknowable
    /// posture). `Some(false)` = every supertype on the written chain is
    /// indexed and none declares an instance member.
    #[must_use]
    pub fn supertype_has_instance_members(&self, container: StringId) -> Option<bool> {
        supertype_has_instance_members(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        )
    }

    /// Whether the member `member` — resolved by the
    /// [`Analysis::inherited_member`] walk from `container` (self-hits
    /// included, the caller tells them apart by the walk's defining name) —
    /// carries an annotation spelled `annotation`. The declaration-side cut
    /// of the `@override`-presence questions the override families ask
    /// (`element.metadata`, the `hasOverride` face). A field group's star
    /// annotates every group member; enum constants claim their own.
    ///
    /// `None` = the container name does not resolve or the whole chain
    /// lacks the member — the member could still carry the annotation, so
    /// the caller stays silent (the under-report posture).
    #[must_use]
    pub fn member_has_annotation(
        &self,
        container: StringId,
        member: StringId,
        annotation: StringId,
    ) -> Option<bool> {
        let (_, entry) = member_lookup_defining(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
            member,
            false,
        )?;
        Some(entry.annotations.iter().any(|row| row.name == annotation))
    }

    /// Whether any captured formal of `member` — the
    /// [`Analysis::inherited_member`] walk, self-hits included — was
    /// written `covariant`. The `unnecessary_overrides` parameter gate
    /// reads it (upstream Dart SDK source, pkg/linter/lib/src/rules/
    /// unnecessary_overrides.dart `_isCovariant`: a covariant parameter
    /// narrows the inherited signature, so a super-forwarding body is not
    /// trivially removable).
    ///
    /// `None` under-reports exactly like [`Analysis::inherited_member`];
    /// a member with no captured formal table answers `Some(false)`.
    #[must_use]
    pub fn member_has_covariant_formal(
        &self,
        container: StringId,
        member: StringId,
    ) -> Option<bool> {
        let (_, entry) = member_lookup_defining(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
            member,
            false,
        )?;
        Some(entry.formals.as_deref().is_some_and(|formals| {
            formals
                .positional
                .iter()
                .chain(&formals.named)
                .any(|f| f.covariant)
        }))
    }

    /// The default value of `member`'s named formal `formal`, as written
    /// (the interned verbatim source text) — the override-signature
    /// rendering spells it back (a parent's written default is inherited
    /// semantics, not re-derivable from the type). The
    /// [`Analysis::inherited_member`] walk, self-hits included.
    ///
    /// `None` = the member or chain is unknowable; `Some(None)` = the
    /// member is known but carries no captured default under `formal`'s
    /// spelling (not a named formal, no `=`/`:` written, or recovery
    /// dropped the slice).
    #[must_use]
    pub fn member_formal_default_value(
        &self,
        container: StringId,
        member: StringId,
        formal: StringId,
    ) -> Option<Option<&'a str>> {
        let (_, entry) = member_lookup_defining(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
            member,
            false,
        )?;
        let default = entry
            .formals
            .as_deref()
            .and_then(|formals| formals.named_formal(formal))
            .and_then(|row| row.default_value);
        Some(default.map(|text| self.strings.resolve(text)))
    }

    /// Whether the receiver type implements `interface` — the transitive
    /// supertype question the spread rule asks as
    /// `target.staticType.implementsInterface('Iterable', 'dart.core')`
    /// (upstream Dart SDK source, pkg/linter/lib/src/rules/
    /// unnecessary_to_list_in_spreads.dart; the walk:
    /// `library::implements_interface`, the declared supertype graph —
    /// `extends`/`with`/`implements`, a mixin's `on` constraints — walked
    /// transitively, cross-file through the declaring containers'
    /// libraries).
    ///
    /// The keying contract (name-level): `ty` is the model's own [`Ty`] —
    /// the read-row faces' product ([`Analysis::type_of`],
    /// [`Analysis::member_type`]); a caller holding only a type name wraps
    /// it (`Ty::named`). The walk compares membership at interface-name
    /// granularity and type arguments never enter: a nominal supertype
    /// edge is not flipped by a substitution (`C<T> implements I<T>` holds
    /// for every `T`), so the boolean needs no substitution map — one
    /// becomes necessary only when a consumer must compare the
    /// interface's *own* arguments. Nullability is erased (the official
    /// check is element-based); an extension type erases to its
    /// representation type (`extensions.dart:295`
    /// `extensionTypeErasure`).
    ///
    /// A name that resolves to an indexed container is walked as a user
    /// type even when its spelling equals `interface` (the oracle compares
    /// the defining library, `extensions.dart:335-340`); a name outside
    /// the indexed set is answered only by the small `dart:core` face the
    /// walk carries (`Iterable`/`List`/`Set` for an `Iterable` query) —
    /// every other unindexed name under-reports `None` even where the
    /// oracle's full core graph would answer `false`: the consumer keeps
    /// its current behavior, the under-report direction. `Some(false)`
    /// requires every reached name to be an indexed container with no hit.
    ///
    /// Top types: `dynamic`/`void`/`Null`/`Never` are not interface
    /// carriers and answer `Some(false)` outright (`extensions.dart:326`);
    /// [`Ty::Unknown`] (no declared type captured) under-reports `None`.
    #[must_use]
    pub fn implements_core_interface(&self, ty: Ty, interface: StringId) -> Option<bool> {
        let name = match ty {
            Ty::Named { name, .. } => name,
            Ty::Unknown => return None,
            // A function type is no interface carrier either.
            Ty::Dynamic
            | Ty::Void
            | Ty::Null
            | Ty::Never
            | Ty::Record { .. }
            | Ty::Function { .. } => return Some(false),
        };
        implements_interface(
            self.graph,
            self.namespaces,
            self.index,
            self.strings,
            self.lib,
            self.file,
            name,
            interface,
            false,
        )
    }

    /// —— family (e): shadowing & annotation walks ———————————————————
    ///
    /// Whether the name can enter this unit's scope through an import: an
    /// `as` prefix spelled `name`, or an unprefixed import whose target
    /// exports `name` (combinator filtered). The import layer of the
    /// enclosing-name-scope question the shadowing rules ask — a rule
    /// checking whether dropping an explicit `this.` (or another
    /// receiver-less spelling) would re-resolve the name consults this
    /// face for the part its own declaration tables cannot see (upstream
    /// Dart SDK source, pkg/linter/lib/src/rules/unnecessary_this.dart
    /// `_canReferenceElementWithoutThisPrefix`, the `resolveNameInScope`
    /// library-scope half).
    ///
    /// `true` is the conservative direction: an import whose target is
    /// outside the indexed set answers `true` (it could export the name),
    /// so `false` means "no indexed import carries the name" — the rule
    /// keeps its silence on `true`, never fabricates a shadow.
    #[must_use]
    pub fn import_shadows(&self, name: StringId) -> bool {
        import_shadows(self.graph, self.lib, name)
    }

    /// Whether the container `name` — resolved file-locally first, then
    /// through the library namespace, cross-file like
    /// [`Analysis::inherited_member`]'s anchor — carries an annotation
    /// spelled `annotation`. The name-level cut of the JS-interop
    /// classification the runtime-check rules read (the class element's
    /// own `@JS` metadata; upstream Dart SDK source, pkg/linter/lib/src/
    /// rules/invalid_runtime_check_with_js_interop_types.dart
    /// `_jsTypeForStaticInterop`): only the resolved container's own
    /// annotation rows answer, the supertype chain is not walked.
    ///
    /// `None` = the name does not resolve to an indexed container (an SDK
    /// type, an unanalyzed dependency) — that container could carry the
    /// annotation, so the caller stays silent. A prefix-qualified
    /// annotation keeps its dot-joined spelling (`@p.JS()` stores `p.JS`)
    /// and does not match a bare `JS` query — the under-report direction.
    #[must_use]
    pub fn container_has_annotation(
        &self,
        container: StringId,
        annotation: StringId,
    ) -> Option<bool> {
        container_has_annotation(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
            annotation,
        )
    }

    /// The enclosing library's compilation units — the defining unit first,
    /// then each part in part-directive order, exactly the registration
    /// order the graph's unit face holds (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/analysis/library_analyzer.dart
    /// `_resolveDirectives` parses the defining unit before its parts and
    /// publishes the walk as `_libraryFiles`; the oracle's
    /// `RuleContext.allUnits`). A pure read over the graph: no copy, a
    /// deterministic total order — a node registered only as a directive
    /// target answers an empty slice, while every live analyzed library is
    /// described, so rule consumption always sees at least the defining
    /// unit.
    #[must_use]
    pub fn library_units(&self) -> &[FileId] {
        self.graph.unit_files(self.lib)
    }

    /// The enclosing library's canonical identity string (`package:` inside
    /// a package lib dir, `file:` elsewhere) — the defining library's URI,
    /// the string shape the oracle's library-identity comparisons read.
    /// The URI is the **defining** compilation unit's library URI, not the
    /// import spelling (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/element/element.dart: `element.library.uri`
    /// — `import 'package:flutter/material.dart'` still yields
    /// `package:flutter/src/widgets/container.dart` for `Container`,
    /// because material.dart re-exports it).
    #[must_use]
    pub fn library_uri(&self) -> &'a str {
        self.graph.uri(self.lib)
    }

    /// The canonical URI of the library `file` is a registered compilation
    /// unit of — the attribution face for a resolved target's defining file
    /// (the `target_file` a [`Binding`] or [`CtorBinding`] carries):
    /// attribute an indexed declaration to its defining library's URI, the
    /// same defining-unit identity [`Analysis::library_uri`] answers for
    /// the enclosing library. `None` when the file registered no unit with
    /// any graph library — an unindexed file (every SDK or unanalyzed
    /// dependency file) carries no URI here, the under-report direction.
    #[must_use]
    pub fn uri_of_file(&self, file: FileId) -> Option<&'a str> {
        Some(self.graph.uri(self.graph.library_of_file(file)?))
    }

    /// —— doc-comment reference resolution ————————————————————————————
    ///
    /// Whether the bracket reference `parts` (`1..=3` dot-joined identifier
    /// spellings, written in a doc comment attached to `container`) is
    /// visible in the enclosing scope — the `comment_references` gate. The
    /// three-state answer keeps every report exact: `Some(false)` is a
    /// confident miss (the oracle reports), `Some(true)` a hit, and `None`
    /// an undecidable face that must stay silent (an unindexed supertype,
    /// an alias target the model cannot unwrap, an ambiguous import). The
    /// chain mirrors the official resolution
    /// (upstream Dart SDK source, pkg/analyzer/lib/src/dart/resolver/
    /// comment_reference_resolver.dart): the bare face walks the scope
    /// (file locals, import namespace, prefixes — setters probed under
    /// their `name=` spelling), then the enclosing container's declared
    /// members and enum constants, then the inherited chain
    /// ([`member_lookup`]), then the extension faces of the libraries in
    /// scope; the two-part face resolves the head as a prefix
    /// ([`Namespaces::lookup_prefixed`]) or a container (declared members,
    /// named constructors, inherited — no extension channel); the
    /// three-part face requires the head to be an import prefix and judges
    /// only the tail on the middle name (everything else reports — the
    /// official early-returns with the third component unresolved).
    #[must_use]
    pub fn doc_reference_resolves(
        &self,
        container: Option<StringId>,
        parts: &[StringId],
    ) -> Option<bool> {
        match parts {
            [name] => self.doc_reference_bare(container, *name),
            [head, tail] => self.doc_reference_prefixed(*head, *tail),
            [head, middle, tail] => self.doc_reference_property(*head, *middle, *tail),
            _ => None,
        }
    }

    /// The bare-reference face: scope, then (inside a container) the
    /// instance face the official `_resolveUnqualified` walks — declared
    /// members and enum constants, inherited members, extensions in scope.
    fn doc_reference_bare(&self, container: Option<StringId>, name: StringId) -> Option<bool> {
        if self.scoped_hits(name) {
            return Some(true);
        }
        let Some(container) = container else {
            // Top-level docs carry no enclosing instance: the official
            // fallback needs `enclosingInstanceElement`, null at the library
            // scope — a scope miss is a confident miss.
            return Some(false);
        };
        let (found, _) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        )?;
        let setter = self.setter_spelling(name);
        if found.members.get(&name).is_some()
            || found.members.get(&setter).is_some()
            || found.enum_constants.contains(&name)
            || self.inherited_declares(container, name, setter)
        {
            return Some(true);
        }
        // The `Object` root: every container kind reaches it in the bare
        // face — classes through the inheritance root, an extension's
        // members through the receiver (`on`) type — no kind gate.
        if self.is_object_member(name) {
            return Some(true);
        }
        // A `None` from the walk is not proof of absence (a superclass
        // outside the indexed set could declare the name); only a fully
        // walked chain backs a report.
        supertype_has_instance_members(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        )?;
        if self.extension_declares(name, setter) {
            return Some(true);
        }
        Some(false)
    }

    /// The `head.tail` face (upstream `_resolveTwoComponents`): a prefix
    /// resolves through its import table; a container resolves its
    /// declared members, named constructors, and inherited chain; an alias
    /// unwrap is unknown (silent); every other resolved head — and every
    /// head miss, which the official fallback still reports — is a
    /// confident miss.
    fn doc_reference_prefixed(&self, head: StringId, tail: StringId) -> Option<bool> {
        match self.scoped_lookup(head) {
            ScopedHit::Found(NamespaceEntry::Prefix) => {
                match self
                    .namespaces
                    .lookup_prefixed(self.graph, self.lib, head, tail)
                {
                    Lookup::Found(_) => Some(true),
                    Lookup::Ambiguous(_) => None,
                    Lookup::NotFound => Some(false),
                }
            }
            ScopedHit::Local(decl) => self.resolved_head_tail(head, decl.kind, tail),
            ScopedHit::Found(NamespaceEntry::Decl(file)) => {
                match self.index.get(file).and_then(|f| f.decl(head)) {
                    Some(decl) => self.resolved_head_tail(head, decl.kind, tail),
                    None => self.container_member(head, tail),
                }
            }
            ScopedHit::Ambiguous(_) => None,
            ScopedHit::NotFound => Some(false),
        }
    }

    /// The tail judgment once the two-part head has resolved to a
    /// declaration: only a container opens the member walk; an alias
    /// unwrap is unknown (silent — the oracle never lands there either);
    /// every other resolved head (a top-level function or variable, a
    /// type parameter) matches neither upstream arm of
    /// `_resolveTwoComponents`, leaving the tail unresolved — a
    /// confident miss.
    fn resolved_head_tail(&self, head: StringId, kind: MicroKind, tail: StringId) -> Option<bool> {
        match kind {
            MicroKind::Class | MicroKind::Enum | MicroKind::Mixin | MicroKind::Extension => {
                self.container_member(head, tail)
            }
            MicroKind::TypeAlias => None,
            MicroKind::Prefix
            | MicroKind::TopLevelFn
            | MicroKind::TopLevelVar
            | MicroKind::TypeParam => Some(false),
        }
    }

    /// The `head.middle.tail` face (upstream `_resolveThreeComponents`):
    /// only an import prefix opens the chain — any other head (or a miss,
    /// which the official fallback still feeds the early return) leaves the
    /// third component unresolved, a confident miss. The middle name must
    /// resolve to an indexed container through the prefix's own table; an
    /// alias unwrap is unknown, and any other element kind never reaches
    /// the tail lookup.
    fn doc_reference_property(
        &self,
        head: StringId,
        middle: StringId,
        tail: StringId,
    ) -> Option<bool> {
        match self.scoped_lookup(head) {
            ScopedHit::Found(NamespaceEntry::Prefix) => {
                match self
                    .namespaces
                    .lookup_prefixed(self.graph, self.lib, head, middle)
                {
                    Lookup::Found(NamespaceEntry::Decl(owner)) => {
                        let file = self.index.get(owner);
                        if file
                            .and_then(|file| file.decl(middle))
                            .is_some_and(|decl| decl.kind == MicroKind::TypeAlias)
                        {
                            return None;
                        }
                        let found = file.and_then(|file| file.container(middle))?;
                        let setter = self.setter_spelling(tail);
                        if found.members.get(&tail).is_some()
                            || found.members.get(&setter).is_some()
                            || found.enum_constants.contains(&tail)
                            || found.constructors.contains(&tail)
                            || (self.strings.resolve(tail) == "new"
                                && found.has_unnamed_constructor)
                            || self.inherited_declares(middle, tail, setter)
                        {
                            return Some(true);
                        }
                        // The `Object` root, gated to the `InterfaceElement`
                        // family: upstream's `_resolveInstanceMember`
                        // inheritance arm runs under `container is
                        // InterfaceElement`, which an extension is not — an
                        // extension head's tail never reaches the
                        // inheritance root and reports.
                        if matches!(
                            found.kind,
                            ContainerKind::Class
                                | ContainerKind::Enum
                                | ContainerKind::Mixin
                                | ContainerKind::ExtensionType
                        ) && self.is_object_member(tail)
                        {
                            return Some(true);
                        }
                        supertype_has_instance_members(
                            self.graph,
                            self.namespaces,
                            self.index,
                            self.lib,
                            self.file,
                            middle,
                        )?;
                        Some(false)
                    }
                    Lookup::Found(NamespaceEntry::Prefix) | Lookup::Ambiguous(_) => None,
                    Lookup::NotFound => Some(false),
                }
            }
            ScopedHit::Ambiguous(_) => None,
            ScopedHit::Local(_) | ScopedHit::Found(_) | ScopedHit::NotFound => Some(false),
        }
    }

    /// The container-instance face of the two-part tail: declared members
    /// and enum constants, the anchor's named constructors (the official
    /// `allowConstructors` cut — constructors are never inherited), then
    /// the inherited chain; a report needs a fully walked chain.
    fn container_member(&self, container: StringId, member: StringId) -> Option<bool> {
        let (found, _) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        )?;
        let setter = self.setter_spelling(member);
        if found.members.get(&member).is_some()
            || found.members.get(&setter).is_some()
            || found.enum_constants.contains(&member)
            || found.constructors.contains(&member)
            // `A.new` names the unnamed constructor (the doc-comment parser
            // rewrites `new` after a dot to an identifier).
            || (self.strings.resolve(member) == "new" && found.has_unnamed_constructor)
            || self.inherited_declares(container, member, setter)
        {
            return Some(true);
        }
        // The `Object` root, gated to the `InterfaceElement` family (the
        // upstream `_resolveInstanceMember` inheritance arm): an extension
        // head's tail never reaches the inheritance root there and reports.
        if matches!(
            found.kind,
            ContainerKind::Class
                | ContainerKind::Enum
                | ContainerKind::Mixin
                | ContainerKind::ExtensionType
        ) && self.is_object_member(member)
        {
            return Some(true);
        }
        supertype_has_instance_members(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        )?;
        Some(false)
    }

    /// The inherited-member probe over [`member_lookup`], both spellings.
    fn inherited_declares(&self, container: StringId, member: StringId, setter: StringId) -> bool {
        member_lookup(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
            member,
        )
        .is_some()
            || member_lookup(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                container,
                setter,
            )
            .is_some()
    }

    /// The in-library scope probe for one spelling (file locals — type
    /// parameters included, then the import namespace with its prefix
    /// branch), the setter spelling riding as the getter-miss fallback.
    /// Ambiguity resolves toward silence: the oracle defers the conflict
    /// to `ambiguous_import`, never to a missing element.
    fn scoped_hits(&self, name: StringId) -> bool {
        match self.scoped_lookup(name) {
            ScopedHit::NotFound => {
                self.scoped_lookup(self.setter_spelling(name)) != ScopedHit::NotFound
            }
            _ => true,
        }
    }

    fn scoped_lookup(&self, name: StringId) -> ScopedHit<'a> {
        lookup_scoped(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        )
    }

    fn setter_spelling(&self, name: StringId) -> StringId {
        self.strings
            .intern(&format!("{}=", self.strings.resolve(name)))
    }

    /// Whether the name is one of the `Object` implicit-interface members.
    /// The inheritance manager's chains root every `InterfaceElement` at
    /// `Object` (upstream comment_reference_resolver.dart
    /// `_resolveInstanceMember` ends in `inheritance.getMember`), so a hit
    /// on these four names is confident resolution whenever the container
    /// face resolved. A stateless spelling compare — the check runs only on
    /// the miss path of a doc-reference decision, never a hot loop.
    fn is_object_member(&self, name: StringId) -> bool {
        matches!(
            self.strings.resolve(name),
            "toString" | "hashCode" | "runtimeType" | "noSuchMethod"
        )
    }

    /// Whether any extension face of the libraries in scope declares the
    /// member — the conservative gate over the bare face's extension
    /// channel (matched by name only, never by `on` type, so a hit is
    /// always the silent direction). The visible set is this library's own
    /// units plus every unit of a directly imported library, chased
    /// through that library's export chain (cycle-guarded) — extensions
    /// apply from direct imports and re-exports only, matching the
    /// official `findExtension` scope.
    fn extension_declares(&self, name: StringId, setter: StringId) -> bool {
        let mut seen: FxHashSet<u32> = FxHashSet::default();
        let mut queue: Vec<LibraryId> = vec![self.lib];
        seen.insert(self.lib.index());
        while let Some(lib) = queue.pop() {
            for file in self.graph.unit_files(lib) {
                if let Some(elements) = self.index.get(*file) {
                    if elements.extension_faces().iter().any(|face| {
                        face.members.contains_key(&name) || face.members.contains_key(&setter)
                    }) {
                        return true;
                    }
                }
            }
            let reach =
                |target: LibraryId, queue: &mut Vec<LibraryId>, seen: &mut FxHashSet<u32>| {
                    if seen.insert(target.index()) {
                        queue.push(target);
                    }
                };
            for edge in self.graph.import_edges(lib) {
                if let Some(target) = edge.target {
                    reach(target, &mut queue, &mut seen);
                }
            }
            for edge in self.graph.export_edges(lib) {
                if let Some(target) = edge.target {
                    reach(target, &mut queue, &mut seen);
                }
            }
        }
        false
    }

    /// —— family (c): reassignment tracking —————————————————————————
    ///
    /// Whether the declaration is written anywhere in its body (official
    /// `isPotentiallyMutated`, the per-body `AssignedVariables`
    /// face). Declaration initializers do not count (the official sets
    /// exclude them); top-level declarations take no part in any
    /// executable's sets and answer `false`.
    #[must_use]
    pub fn is_potentially_mutated_in_scope(&self, decl: SourceRange) -> bool {
        let (start, end) = self.write_range(decl);
        start < end
    }

    /// Whether any of the declaration's writes sit inside a nested
    /// closure (official `isPotentiallyMutatedInClosure`).
    #[must_use]
    pub fn is_potentially_mutated_in_closure(&self, decl: SourceRange) -> bool {
        let (start, end) = self.write_range(decl);
        self.write_in_closure[start..end]
            .iter()
            .any(|&in_closure| in_closure)
    }

    /// The declaration's write rows — the per-(declaration, in-closure)
    /// aggregation the flow layer emits, anchored at the declaration
    /// itself (per-write positions are a later increment; the rows are
    /// the `isPotentiallyMutated*` faces' substrate).
    #[must_use]
    pub fn write_sites_of(&self, decl: SourceRange) -> &[SourceRange] {
        let (start, end) = self.write_range(decl);
        &self.write_decls[start..end]
    }

    /// —— family (d): expression static types ———————————————————————
    ///
    /// The static type of the expression at an invocation, creation, or
    /// member-access site — composed from the fold's published rows (the
    /// same lookups [`Analysis::binding_of`] and
    /// [`Analysis::resolve_constructor`] read) plus the live model walks,
    /// never a re-analysis. The face answers only types the model can
    /// name: an unresolved target, an unindexed declaring file, or an
    /// uncaptured declared type under-reports `None` silently.
    ///
    /// The static type of the invocation whose callee token starts at
    /// `callee` — a bare function, getter, or top-level variable
    /// (`g()`), an implicit-`this` member (`m()` inside the container),
    /// or a local callable through its flow read row. Explicit
    /// `receiver.method()` callees carry no bare-reference row; they
    /// answer through [`Analysis::member_type`]. `callee` must be the
    /// callee token's own span.
    #[must_use]
    pub fn callee_type(&self, callee: SourceRange) -> Option<Ty> {
        let (name, target) = row_starting_at(&self.refs, callee, |row| row.0.offset)
            .map(|(_, name, target)| (*name, *target))?;
        match target {
            RefTarget::Decl { file } => top_decl_type(self.index.get(file)?.decl(name)?),
            RefTarget::Member { container } => member_entry_type(member_lookup(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                container,
                name,
            )?),
            // A local callable's type is its flow read row (the declared
            // type when captured); the remaining targets carry no
            // callable type at M1.
            RefTarget::Local => decided(self.type_of(callee)?),
            RefTarget::TypeParam
            | RefTarget::Prefix
            | RefTarget::Opaque
            | RefTarget::This
            | RefTarget::Missing => None,
        }
    }

    /// The static type of the `receiver.name` member access — a getter's
    /// or method's declared return type, or a field's declared type (the
    /// property-read and method-invoke faces). Same receiver resolution
    /// as [`Analysis::member_of`]; unknown-type receivers and undecided
    /// member types under-report `None`.
    #[must_use]
    pub fn member_type(&self, receiver: SourceRange, name: StringId) -> Option<Ty> {
        member_entry_type(self.member_of(receiver, name)?.entry)
    }

    /// The type the creation at `type_name` constructs (`new C.named()`,
    /// `const C()`): the resolved container as a non-nullable named type
    /// — the M1 cut, argument-less (the head's written type arguments
    /// are not published with the type-reference row). `type_name` must
    /// be the type-reference's own span; an unresolvable name answers
    /// `None`. The type named at any published type-reference position
    /// answers — the caller passes creation heads.
    #[must_use]
    pub fn creation_type(&self, type_name: SourceRange) -> Option<Ty> {
        self.resolve_constructor(type_name, None)
            .map(|binding| Ty::named(binding.container))
    }

    /// The structural function signature at a bare-reference site — the
    /// tear-off/callee face the lambda-replacement rules read
    /// ([`FnSig`], the composition of a declaration's return type and
    /// formal table). `use_site` must be the reference token's own span.
    ///
    /// Answers a top-level function (a bare `g(…)` callee or tear-off) and
    /// an implicit-`this` method (`m(…)` inside the container). An explicit
    /// `receiver.name` access composes through [`Analysis::member_of`] +
    /// [`FnSig::of_member`].
    ///
    /// Under-reports `None` on every shape the model cannot name: local
    /// functions (formals not modeled), getters and setters (no formal
    /// table / not tear-off targets), fields, type parameters, prefixes,
    /// opaque targets, and unresolved names — the caller stays silent.
    #[must_use]
    pub fn function_signature_of(&self, use_site: SourceRange) -> Option<FnSig<'_>> {
        let (name, target) = row_starting_at(&self.refs, use_site, |row| row.0.offset)
            .map(|(_, name, target)| (*name, *target))?;
        match target {
            RefTarget::Decl { file } => FnSig::of_top_decl(self.index.get(file)?.decl(name)?),
            RefTarget::Member { container } => FnSig::of_member(member_lookup(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                container,
                name,
            )?),
            // A local callable carries no modeled formals; the remaining
            // targets carry no callable face at M1.
            RefTarget::Local
            | RefTarget::TypeParam
            | RefTarget::Prefix
            | RefTarget::Opaque
            | RefTarget::This
            | RefTarget::Missing => None,
        }
    }

    /// The matched-value type of a pattern context — the static type the
    /// pattern at a case clause matches against (official
    /// `getMatchedValueType`, upstream Dart SDK source,
    /// pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart:8753-8760:
    /// the promotion model's head for the matched value, falling back to
    /// its declared type). `scrutinee` must be the scrutinee read's own
    /// identifier span; for a bare variable-read scrutinee the read row
    /// carries exactly that value (the scrutinee evaluates once, before
    /// every clause, so the read row's promoted-or-declared type is the
    /// matched type at each clause).
    ///
    /// The read-row key covers the shapes the flow matched-value model
    /// answers (it too requires a bare variable-read scrutinee — see
    /// `FlowFact::MatchedType`), and the model reaches an if-statement's
    /// case clause the same way it reaches a switch clause: upstream
    /// `ifCaseStatement_afterExpression` stores the evaluated condition
    /// for the pattern walk's `_getMatchedValueType` to read back
    /// (flow_analysis.dart:940-990, :8752-8758), and an `if (x case …)`
    /// condition is parsed and evaluated as the ordinary expression it
    /// is — its bare read carries the same read row, so the same span
    /// key answers. Every other scrutinee shape and an untyped read
    /// (`var`-inferred) under-report `None`, the caller stays silent.
    #[must_use]
    pub fn matched_value_type(&self, scrutinee: SourceRange) -> Option<Ty> {
        decided(self.type_of(scrutinee)?)
    }

    /// —— raw fold products ————————————————————————————————————————
    ///
    /// The unit's flow facts, offset-sorted.
    #[must_use]
    pub fn facts(&self) -> &[FlowFact] {
        &self.facts
    }

    /// The unit's unresolved-name rows (the report's names table).
    #[must_use]
    pub fn names(&self) -> &[NameRow] {
        &self.names
    }

    /// The `[start, end)` window of write rows anchored at `decl`.
    fn write_range(&self, decl: SourceRange) -> (usize, usize) {
        let start = self
            .write_decls
            .partition_point(|range| range.offset < decl.offset);
        let end =
            start + self.write_decls[start..].partition_point(|range| range.offset == decl.offset);
        (start, end)
    }
}

/// The row of an offset-sorted table whose span starts exactly at
/// `span.offset` — the queries' span contract (a row and its query share
/// the identifier/type-reference token's own range).
fn row_starting_at<R>(rows: &[R], span: SourceRange, key: impl Fn(&R) -> u32) -> Option<&R> {
    let start = rows.partition_point(|row| key(row) < span.offset);
    rows.get(start).filter(|row| key(row) == span.offset)
}

/// The declared type a top-level declaration carries — a function
/// family's return type, a variable's declared type, nothing for the
/// other families. An uncaptured type under-reports [`None`].
fn top_decl_type(decl: &TopDecl) -> Option<Ty> {
    let ty = match decl.kind {
        MicroKind::TopLevelFn => decl.return_ty,
        MicroKind::TopLevelVar => decl.var_ty,
        MicroKind::Class
        | MicroKind::Enum
        | MicroKind::Mixin
        | MicroKind::Extension
        | MicroKind::TypeAlias
        | MicroKind::Prefix
        | MicroKind::TypeParam => return None,
    };
    decided(ty)
}

/// The declared type a member access yields — a method's or getter's
/// return type, a field's declared type; a setter never binds a read.
/// An uncaptured type under-reports [`None`].
fn member_entry_type(entry: &MemberEntry) -> Option<Ty> {
    let ty = match entry.kind {
        MemberKind::Method | MemberKind::Getter => entry.return_ty,
        MemberKind::Field => entry.var_ty,
        MemberKind::Setter => return None,
    };
    decided(ty)
}

/// `Some` when the model can name the type — [`Ty::Unknown`] (no
/// declared type captured) is no answer, the under-report direction.
fn decided(ty: Ty) -> Option<Ty> {
    (ty != Ty::Unknown).then_some(ty)
}

#[cfg(test)]
mod tests {
    use super::{Analysis, FnSig, analyze_unit};
    use crate::diagnose::StrictModes;
    use crate::library::ElementIndex;
    use crate::model::{ArgListId, MicroKind, Ty};
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_parser::ParserFeatures;
    use dartq_resolution::{
        Combinators, ImportEdge, LibraryClassModifiers, LibraryGraph, LibraryGraphBuilder,
        Namespaces,
    };
    use dartq_scanner::{ScanConfig, TokenTable};
    use dartq_source::{FileTable, SourceRange, StringTable};
    use std::path::Path;

    /// Builds the `Analysis` of `source` as the sole analyzed file of
    /// `package:t`, with `dependency` analyzed and imported unprefixed
    /// when given. The context tables leak (`'static`) — the tests are
    /// one-shot processes.
    fn analyze(source: &str, dependency: Option<&str>) -> Analysis<'static> {
        let deps: Vec<(&str, Option<&str>)> = dependency.map(|d| (d, None)).into_iter().collect();
        analyze_imports(source, &deps)
    }

    /// [`analyze`] with arbitrary import edges: each `(dependency, prefix)`
    /// is analyzed and imported, prefixed when `prefix` is given. An empty
    /// dependency source registers an import whose target is outside the
    /// indexed set (an unresolvable URI — the edge contributes no files).
    fn analyze_imports(source: &str, deps: &[(&str, Option<&str>)]) -> Analysis<'static> {
        let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
        let mut files = FileTable::new();
        let file = files.intern(Path::new("main.dart"));
        let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
        let features = ParserFeatures::from_feature_set(set);
        let scan_config = ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        };
        let scanned = dartq_scanner::scan(source, scan_config, file);
        assert!(
            scanned.diagnostics.is_empty(),
            "scanner diagnostics: {source:?}"
        );
        let tokens: &'static TokenTable = Box::leak(Box::new(scanned.tokens));
        let elements = crate::collector::collect(source, tokens, features, strings);
        let mut builder = LibraryGraphBuilder::new(strings);
        let lib = builder.intern("package:t/main.dart");
        // The defining unit registers like the engine's graph build does
        // (`builder.unit(graph_id, info.defining)`) — the unit face the
        // library-URI queries attribute through.
        builder.unit(lib, file);
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
        let mut index = ElementIndex::default();
        index.insert(file, elements);
        for (nth, (dep_source, prefix)) in deps.iter().enumerate() {
            let target = if dep_source.is_empty() {
                None
            } else {
                let dep_file = files.intern(Path::new(&format!("kit{nth}.dart")));
                let scanned_dep = dartq_scanner::scan(dep_source, scan_config, dep_file);
                assert!(
                    scanned_dep.diagnostics.is_empty(),
                    "scanner diagnostics: {dep_source:?}"
                );
                let dep_tokens: &'static TokenTable = Box::leak(Box::new(scanned_dep.tokens));
                let dep_elements =
                    crate::collector::collect(dep_source, dep_tokens, features, strings);
                let dep_lib = builder.intern(&format!("package:t/kit{nth}.dart"));
                builder.unit(dep_lib, dep_file);
                for (name, decl) in dep_elements.declarations() {
                    if decl.kind == MicroKind::TypeParam {
                        continue;
                    }
                    builder.declare(dep_lib, name, dep_file);
                }
                index.insert(dep_file, dep_elements);
                Some(dep_lib)
            };
            builder.import(
                lib,
                ImportEdge {
                    target,
                    prefix: prefix.map(|spelling| strings.intern(spelling)),
                    deferred: false,
                    combinators: Combinators::unfiltered(),
                },
            );
        }
        let graph: &'static LibraryGraph = Box::leak(Box::new(builder.finish()));
        let namespaces: &'static Namespaces = Box::leak(Box::new(Namespaces::build(graph)));
        let index: &'static ElementIndex = Box::leak(Box::new(index));
        analyze_unit(
            source,
            tokens,
            features,
            strings,
            file,
            lib,
            graph,
            namespaces,
            index,
            StrictModes::default(),
            &LibraryClassModifiers::default(),
        )
    }

    /// The `SourceRange` of `needle`'s first occurrence in `source`
    /// (ASCII corpus: byte offsets == UTF-16 offsets).
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    fn span_of(source: &str, needle: &str) -> SourceRange {
        let offset: u32 = source
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not in {source:?}"))
            .try_into()
            .expect("offset fits u32");
        SourceRange {
            offset,
            length: needle.len() as u32,
        }
    }

    /// The one-token span at `needle`'s first occurrence.
    fn token_of(source: &str, needle: &str) -> SourceRange {
        let span = span_of(source, needle);
        SourceRange {
            offset: span.offset,
            length: 1,
        }
    }

    /// `Analysis` is immutable and shared across fan-out workers: the
    /// Send + Sync bound is part of the contract.
    #[test]
    fn analysis_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>(_: &T) {}
        let analysis = analyze("void f() {}", None);
        assert_send_sync(&analysis);
    }

    #[test]
    fn type_of_answers_declared_and_promoted_reads() {
        let source = "void f(int? x) { if (x is int) { x; } }";
        let analysis = analyze(source, None);
        let int = analysis_strings(&analysis).intern("int");
        // The read inside the `is` branch carries the promoted (non-null)
        // declared type.
        assert_eq!(
            analysis.type_of(token_of(source, "x;")),
            Some(Ty::named(int))
        );
        // The parameter's declaration-position identifier is no read.
        assert_eq!(analysis.type_of(token_of(source, "x)")), None);
    }

    /// A typed formal's conditional-position read publishes its ReadType
    /// row (#1056 — the row `type_of` answers), and the row's type folds
    /// through a type alias to the aliased interface type (upstream Dart
    /// SDK source, pkg/analyzer/lib/src/dart/element/element.dart — a
    /// `TypeAliasElement` use resolves through its aliased type before any
    /// type question reads it): `IntAlias` (of `int`) answers `int`, a
    /// `dynamic` formal answers `dynamic`, an alias chain folds to its
    /// terminal named type with the use-site `?` joined, and a
    /// parameterized alias use keeps its written spelling (its
    /// substitution face is not modeled).
    #[test]
    fn type_of_folds_a_typed_formal_alias_in_conditional_position() {
        let source = "typedef IntAlias = int;\nvoid f(IntAlias x) {\n  if (x is int) {}\n}\n";
        let analysis = analyze(source, None);
        let strings = analysis_strings(&analysis);
        let operand = token_of(source, "x is int");
        assert_eq!(
            analysis.type_of(operand),
            Some(Ty::named(strings.intern("int"))),
            "the conditional-position read's row answers the aliased type"
        );

        let dynamic_source = "void g(dynamic x) {\n  if (x is int) {}\n}\n";
        let dynamic = analyze(dynamic_source, None);
        assert_eq!(
            dynamic.type_of(token_of(dynamic_source, "x is int")),
            Some(Ty::Dynamic),
            "a dynamic formal answers as written"
        );

        let chain_source =
            "typedef A = int;\ntypedef B = A;\nvoid h(B? x) {\n  if (x is int) {}\n}\n";
        let chain = analyze(chain_source, None);
        assert_eq!(
            chain.type_of(token_of(chain_source, "x is int")),
            Some(Ty::Named {
                name: analysis_strings(&chain).intern("int"),
                nullable: true,
                args: ArgListId::EMPTY,
            }),
            "the alias chain folds to its terminal type, the use-site ? joined"
        );

        let generic_source = "typedef L<T> = List<T>;\nvoid k(L<int> x) {\n  if (x is int) {}\n}\n";
        let generic = analyze(generic_source, None);
        match generic.type_of(token_of(generic_source, "x is int")) {
            Some(Ty::Named { name, .. }) => {
                assert_eq!(
                    name,
                    analysis_strings(&generic).intern("L"),
                    "a parameterized alias use keeps its written head (its \
                     substitution face is not modeled)"
                );
            }
            other => panic!("the written spelling answers, got {other:?}"),
        }
    }

    #[test]
    fn nullability_faces_classify_the_read_type() {
        let source = "void f(int? x, int y) { x; y; }";
        let analysis = analyze(source, None);
        assert!(analysis.is_potentially_null(token_of(source, "x;")));
        assert!(!analysis.is_definitely_non_null(token_of(source, "x;")));
        assert!(analysis.is_definitely_non_null(token_of(source, "y;")));
        assert!(!analysis.is_potentially_null(token_of(source, "y;")));
    }

    /// The multi-byte-prefix regression (#2019): em-dash comment lines
    /// ahead of the queried code must not desynchronize the query face.
    /// Every public face is UTF-16 keyed (the substrate convention), so a
    /// byte-vs-UTF-16 drift anywhere between the fold's publication and
    /// the lookup silences the answer exactly on files whose prefix
    /// carries multi-byte characters — an ASCII twin cannot see it. The
    /// em-dash comments put the byte coordinates 12 ahead of the UTF-16
    /// ones at the query target; the join must answer at UTF-16 and miss
    /// at the byte-shifted twin, identically to the ASCII control.
    #[test]
    fn binding_of_joins_utf16_coordinates_after_a_multibyte_prefix() {
        for (source, comment_head) in [
            (
                "// ———\n// ———\nvoid f() {\n  var bad = 1;\n  print(bad);\n}\n",
                12_usize,
            ),
            (
                "// c1\n// c2\nvoid f() {\n  var bad = 1;\n  print(bad);\n}\n",
                0,
            ),
        ] {
            let analysis = analyze(source, None);
            let strings = analysis_strings(&analysis);
            let use_byte = source.find("print(bad);").expect("repro shape") + "print(".len();
            let use_utf16: usize = source[..use_byte].chars().map(char::len_utf16).sum();
            assert_eq!(
                use_byte - use_utf16,
                comment_head,
                "the fixture must carry the documented byte/UTF-16 delta"
            );

            // The bare read joins at its UTF-16 coordinate: a local
            // binding of the queried spelling.
            let use_site = SourceRange {
                offset: use_utf16 as u32,
                length: 1,
            };
            let binding = analysis
                .binding_of(use_site)
                .expect("the bare read's row must join at UTF-16 coordinates");
            assert_eq!(binding.kind, super::BindingKind::Local);
            assert_eq!(binding.target_name, strings.intern("bad"));

            // The read-type row joins at the same coordinate (the
            // `var`-initialized read carries no declared type).
            assert!(
                analysis.type_of(use_site).is_some(),
                "the read-type row must join at UTF-16 coordinates"
            );

            // On the multi-byte arm the byte coordinate is 12 past the row:
            // a byte-keyed span must miss (the convention does not
            // cross-match). The ASCII twin has no second coordinate system.
            if comment_head == 0 {
                continue;
            }
            let byte_site = SourceRange {
                offset: use_byte as u32,
                length: 1,
            };
            assert_eq!(
                analysis
                    .binding_of(byte_site)
                    .map(|b| (b.kind, strings.resolve(b.target_name))),
                None,
                "byte coordinates must not join the UTF-16 rows (offset={})",
                byte_site.offset
            );
            assert_eq!(analysis.type_of(byte_site), None);
        }
    }

    /// The shadow-guard join (#2019): a user `print` shadow queried at the
    /// callee site answers its declaration identically after a multi-byte
    /// comment prefix — the consumer face (the lints shadow validation)
    /// must suppress on both files alike.
    #[test]
    fn binding_of_answers_a_shadowed_callee_after_a_multibyte_prefix() {
        for source in [
            "// ———\nvoid f() {\n  void print(Object? o) {}\n  print('local');\n}\n",
            "// c1\nvoid f() {\n  void print(Object? o) {}\n  print('local');\n}\n",
        ] {
            let analysis = analyze(source, None);
            let callee_byte = source.find("  print('local');").expect("repro shape") + 2;
            let callee_utf16: usize = source[..callee_byte].chars().map(char::len_utf16).sum();
            let binding = analysis
                .binding_of(SourceRange {
                    offset: callee_utf16 as u32,
                    length: 1,
                })
                .expect("the shadowed callee's row must join at UTF-16 coordinates");
            assert_eq!(binding.kind, super::BindingKind::Local);
            assert_eq!(
                binding.target_name,
                analysis_strings(&analysis).intern("print")
            );
        }
    }

    #[test]
    fn binding_of_publishes_bare_resolutions() {
        let source = "int g() => 0; void f() { g(); }";
        let analysis = analyze(source, None);
        // The needle carries the trailing `;` so the span lands on the
        // invocation callee: bare `g()` first matches the declaration's own
        // name (`g() =>`), which publishes no bare reference row (a
        // declaration name is not a reference).
        let binding = analysis
            .binding_of(token_of(source, "g();"))
            .expect("the callee resolves");
        assert_eq!(binding.kind, super::BindingKind::Decl);
        assert_eq!(binding.target_name, analysis_strings(&analysis).intern("g"));
        // A type-annotation position publishes no bare binding.
        assert_eq!(analysis.binding_of(token_of(source, "int")), None);
    }

    #[test]
    fn this_publishes_the_binding_and_type_faces() {
        // #1869-2: the keyword binds to the enclosing container, its type
        // answers, the member walk resolves on it — and the implicit-`this`
        // bare read carries the member's declared type, so member queries
        // chain through it exactly as through the explicit `this.` form.
        let source = "class C { int g = 0; int f() { this.g; return g; } }";
        let analysis = analyze(source, None);
        let strings = analysis_strings(&analysis);
        let this = token_of(source, "this.g");
        assert_eq!(
            analysis.type_of(this),
            Some(Ty::named(strings.intern("C"))),
            "the keyword's static type is the container"
        );
        let binding = analysis
            .binding_of(this)
            .expect("the keyword publishes a reference row");
        assert_eq!(binding.kind, super::BindingKind::This);
        let member = analysis
            .member_of(this, strings.intern("g"))
            .expect("the member walk answers on the keyword");
        assert_eq!(member.container, strings.intern("C"));
        // The implicit-`this` bare read: the member's declared type answers
        // and the chain continues through it. The needle anchors the
        // `return` so the span lands on the bare read, not on the `g` of
        // the explicit `this.g` form above (a member-name row carries no
        // read-type row).
        let ret = span_of(source, "return g;");
        let bare = SourceRange {
            offset: ret.offset + 7,
            length: 1,
        };
        assert_eq!(
            analysis.type_of(bare),
            Some(Ty::named(strings.intern("int"))),
            "the bare member read carries the declared type"
        );
        // `this` outside any container publishes nothing (a miss stays a
        // miss — the invalid-reference diagnostic owns the report).
        let orphan_source = "void f() { this; }";
        let orphan = analyze(orphan_source, None);
        assert_eq!(orphan.binding_of(token_of(orphan_source, "this;")), None);
    }

    #[test]
    fn implicit_this_read_chains_member_queries() {
        let source = "class D { D? self; void m() { self.self; } }";
        let analysis = analyze(source, None);
        let strings = analysis_strings(&analysis);
        // The bare `self` read is an implicit-`this` member read: its type
        // is the declared `D?`, and the member walk resolves `self` on it —
        // the query shape the alias families compare the explicit `this.`
        // form against.
        let receiver = token_of(source, "self.self");
        assert_eq!(
            analysis.type_of(receiver),
            Some(Ty::Named {
                name: strings.intern("D"),
                args: ArgListId::EMPTY,
                nullable: true,
            }),
        );
        let member = analysis
            .member_of(receiver, strings.intern("self"))
            .expect("the chain answers through the implicit-this read");
        assert_eq!(member.container, strings.intern("D"));
    }

    #[test]
    fn implicit_creation_head_answers_the_constructor_face() {
        // #1869-3: a bare implicit creation head `C()` publishes the
        // `(span, name)` type-use row `resolve_constructor` keys on — the
        // prefer-const family's entry point — with the same answer the
        // explicit `const C()` spelling already gives.
        let source = "class C { const C(); }\nvoid f() { var c = C(); }";
        let analysis = analyze(source, None);
        let strings = analysis_strings(&analysis);
        // The needle anchors past the `const C();` declaration (whose head
        // publishes no type-use row); `+ 2` skips `= ` onto the head token.
        let init = span_of(source, "= C();");
        let head = SourceRange {
            offset: init.offset + 2,
            length: 1,
        };
        let ctor = analysis
            .resolve_constructor(head, None)
            .expect("the implicit head publishes its creation face");
        assert!(ctor.exists, "the unnamed constructor exists");
        assert_eq!(ctor.container, strings.intern("C"));
    }

    #[test]
    fn write_rows_answer_the_mutation_faces() {
        // `a` is reassigned; `b`'s only write is its initializer (the
        // official sets exclude initializers — the final-suggestion
        // premise).
        let source = "void f() { int a = 0; int b = 0; a = 1; b; }";
        let analysis = analyze(source, None);
        let decl_a = token_of(source, "a = 0;");
        let decl_b = token_of(source, "b = 0;");
        assert!(analysis.is_potentially_mutated_in_scope(decl_a));
        assert!(!analysis.is_potentially_mutated_in_scope(decl_b));
        assert!(!analysis.is_potentially_mutated_in_closure(decl_a));
        assert_eq!(analysis.write_sites_of(decl_a).len(), 1);
        assert_eq!(analysis.write_sites_of(decl_b).len(), 0);
    }

    #[test]
    fn closure_writes_classify_in_closure() {
        let source = "void f() { int a = 0; () { a = 1; }; }";
        let analysis = analyze(source, None);
        let decl_a = token_of(source, "a = 0;");
        assert!(analysis.is_potentially_mutated_in_scope(decl_a));
        assert!(analysis.is_potentially_mutated_in_closure(decl_a));
        // The closure write is the only counted row: the initializer
        // subtracts, so the non-closure classification carries none.
        assert_eq!(analysis.write_sites_of(decl_a).len(), 1);
    }

    #[test]
    fn member_of_walks_the_declared_receiver() {
        let source = "class C { void m() {} } void f(C c) { c.m(); }";
        let analysis = analyze(source, None);
        let strings = analysis_strings(&analysis);
        let binding = analysis
            .member_of(token_of(source, "c."), strings.intern("m"))
            .expect("the member resolves");
        assert_eq!(binding.container, strings.intern("C"));
    }

    #[test]
    fn resolve_constructor_answers_existence_and_formals() {
        // The use precedes the declaration so the first `C.named` span is
        // the creation head (`new` makes the head a constructor
        // reference — the published type-use position).
        let source = "void f() { var c = new C.named(0); } class C { C.named(int a) {} }";
        let analysis = analyze(source, None);
        let strings = analysis_strings(&analysis);
        let binding = analysis
            .resolve_constructor(token_of(source, "C.named"), Some(strings.intern("named")))
            .expect("the container resolves");
        assert!(binding.exists);
        assert!(binding.formals.is_some());
        // The unnamed face: the container declares only `named`.
        let unnamed = analysis
            .resolve_constructor(token_of(source, "C.named"), None)
            .expect("the container resolves");
        assert!(!unnamed.exists);
    }

    #[test]
    fn names_and_facts_expose_the_raw_tables() {
        let source = "void f() { missing(); }";
        let analysis = analyze(source, None);
        let missing = analysis_strings(&analysis).intern("missing");
        assert!(analysis.names().iter().any(|row| row.name == missing));
        assert!(!analysis.facts().is_empty());
    }

    #[test]
    fn callee_type_answers_resolved_invocations() {
        let source = "int g() => 0; class C { int m() => 0; void n() { m(); } } void f() { g(); }";
        let analysis = analyze(source, None);
        let int = analysis_strings(&analysis).intern("int");
        // The bare top-level callee resolves to its declared return type.
        assert_eq!(
            analysis.callee_type(token_of(source, "g();")),
            Some(Ty::named(int))
        );
        // The implicit-`this` member callee walks the enclosing container.
        assert_eq!(
            analysis.callee_type(token_of(source, "m();")),
            Some(Ty::named(int))
        );
    }

    #[test]
    fn member_type_walks_the_declared_receiver() {
        let source = "class C { int x = 0; int m() => 0; } void f() { C c = new C(); c.x; c.m(); }";
        let analysis = analyze(source, None);
        let strings = analysis_strings(&analysis);
        let int = strings.intern("int");
        let receiver = token_of(source, "c.");
        // The declared-typed local's field read carries the field's type.
        assert_eq!(
            analysis.member_type(receiver, strings.intern("x")),
            Some(Ty::named(int))
        );
        // The method invoke carries the declared return type.
        assert_eq!(
            analysis.member_type(receiver, strings.intern("m")),
            Some(Ty::named(int))
        );
    }

    #[test]
    fn creation_type_names_the_container() {
        let source = "void f() { var c = new C.named(0); var d = new D(); } class C { C.named(int a) {} } class D {}";
        let analysis = analyze(source, None);
        let strings = analysis_strings(&analysis);
        // The uses precede the declarations, so the matched spans are the
        // creation heads (the published type-use positions).
        assert_eq!(
            analysis.creation_type(token_of(source, "C.named")),
            Some(Ty::named(strings.intern("C")))
        );
        // The unnamed face of a constructor-less class still names the
        // constructed type.
        assert_eq!(
            analysis.creation_type(token_of(source, "D()")),
            Some(Ty::named(strings.intern("D")))
        );
    }

    #[test]
    fn expression_types_under_report_undecided_shapes() {
        let source = "void f() { missing(); } void g(x) { x.foo; }";
        let analysis = analyze(source, None);
        let strings = analysis_strings(&analysis);
        // An unresolved callee answers nothing, silently.
        assert_eq!(analysis.callee_type(token_of(source, "missing();")), None);
        // An unknown-type receiver (no declared type) under-reports.
        assert_eq!(
            analysis.member_type(token_of(source, "x."), strings.intern("foo")),
            None
        );
        // A span the face publishes no type-reference row at.
        assert_eq!(analysis.creation_type(token_of(source, "missing")), None);
    }

    /// The analysis' shared string table — the child tests module reads
    /// the private field directly (same crate, module tree).
    fn analysis_strings<'a>(analysis: &'a Analysis<'a>) -> &'a StringTable {
        analysis.strings
    }

    /// The signature at a bare top-level callee composes the declaration's
    /// return type and formal table.
    #[test]
    fn function_signature_of_answers_top_level_callees() {
        let source = "int g(int x) => x; void f() { g(0); }";
        let analysis = analyze(source, None);
        let strings = analysis_strings(&analysis);
        let sig = analysis
            .function_signature_of(token_of(source, "g(0)"))
            .expect("the top-level callee composes");
        assert_eq!(sig.type_params, 0);
        assert_eq!(sig.return_ty, Ty::named(strings.intern("int")));
        assert_eq!(sig.formals.positional.len(), 1);
        assert_eq!(
            sig.formals.positional[0].ty,
            Ty::named(strings.intern("int"))
        );
        assert!(sig.formals.positional[0].required);
        assert!(sig.formals.named.is_empty());
    }

    /// An implicit-`this` method callee composes through the member face.
    #[test]
    fn function_signature_of_answers_implicit_this_methods() {
        let source = "class C { int m(int x) => x; void n() { m(0); } }";
        let analysis = analyze(source, None);
        let sig = analysis
            .function_signature_of(token_of(source, "m(0)"))
            .expect("the member callee composes");
        assert_eq!(sig.formals.positional.len(), 1);
    }

    /// The signature resolves through the declaring file — a declaration
    /// imported from a dependency composes from that file's model.
    #[test]
    fn function_signature_of_answers_cross_file_declarations() {
        let source = "void f() { top(0); }";
        let analysis = analyze(source, Some("int top(int x) => x;"));
        assert!(
            analysis
                .function_signature_of(token_of(source, "top(0)"))
                .is_some(),
            "the dependency's declaration composes"
        );
    }

    /// A generic function composes, carrying its type-parameter count —
    /// the predicate (not the composition) stays silent on it.
    #[test]
    fn generic_signatures_compose_with_their_parameter_count() {
        let source = "T g<T>(T x) => x; void f() { g(0); }";
        let analysis = analyze(source, None);
        let sig = analysis
            .function_signature_of(token_of(source, "g(0)"))
            .expect("the generic callee composes");
        assert_eq!(sig.type_params, 1);
    }

    /// Shapes the model cannot name under-report `None` silently: local
    /// functions (formals not modeled), top-level getters (no formal
    /// table), and unresolved names.
    #[test]
    fn function_signature_of_under_reports_unmodeled_shapes() {
        let source = "int get gt => 0; void f() { void local(int x) {} local(0); gt; missing(0); }";
        let analysis = analyze(source, None);
        assert_eq!(
            analysis.function_signature_of(token_of(source, "local(0)")),
            None,
            "a local function's formals are not modeled"
        );
        assert_eq!(
            analysis.function_signature_of(token_of(source, "gt;")),
            None,
            "a top-level getter carries no formal table"
        );
        assert_eq!(
            analysis.function_signature_of(token_of(source, "missing(0)")),
            None,
            "an unresolved name stays silent"
        );
    }

    /// The explicit-receiver path composes through the member face the
    /// rule side already queries: `member_of` + `FnSig::of_member`.
    #[test]
    fn member_signatures_compose_through_the_member_face() {
        let source = "class C { int m(int x) => x; } void f(C c) { c.m(0); }";
        let analysis = analyze(source, None);
        let strings = analysis_strings(&analysis);
        let binding = analysis
            .member_of(token_of(source, "c."), strings.intern("m"))
            .expect("the member resolves");
        let sig = FnSig::of_member(binding.entry).expect("the method composes");
        assert_eq!(sig.return_ty, Ty::named(strings.intern("int")));
        assert_eq!(sig.formals.positional.len(), 1);
    }

    /// `inherited_member` walks the chain from a bare container name: an
    /// inherited hit carries the defining container's name (with the
    /// declaring side's facts), a self-declaration stays distinguishable,
    /// and a name the whole chain lacks under-reports.
    #[test]
    fn inherited_member_answers_from_a_bare_container_name() {
        let source = "abstract class B { void a(); void m() {} } \
                      class C extends B { void m() {} }";
        let analysis = analyze(source, None);
        let strings = analysis_strings(&analysis);
        let c = strings.intern("C");
        // Inherited through the superclass edge; the entry is B's own row.
        let (defining, entry) = analysis
            .inherited_member(c, strings.intern("a"), false)
            .expect("the inherited member resolves");
        assert_eq!(defining, strings.intern("B"));
        assert!(entry.is_abstract);
        // The chain answers a self-hit too: the defining name tells it apart.
        let (defining, _) = analysis
            .inherited_member(c, strings.intern("m"), false)
            .expect("the member resolves");
        assert_eq!(defining, c);
        // A name no chain member declares under-reports.
        assert_eq!(
            analysis.inherited_member(c, strings.intern("gone"), false),
            None
        );
    }

    /// The #1637 capture columns answer through the declaration-side read
    /// face, across an inherited chain: the walk lands on the declaring
    /// row and its annotation star, covariant bits, and default-value
    /// text speak; an unresolvable member under-reports.
    #[test]
    fn member_read_face_answers_from_the_capture_columns() {
        // B declares the annotated, covariant, defaulted member; D
        // inherits it through B2's edge.
        let source = "class B { @override void m(covariant int x, {int y = 3, int z}) {} \
                      void plain() {} } \
                      class B2 extends B {} \
                      class D extends B2 {}";
        let analysis = analyze(source, None);
        let strings = analysis_strings(&analysis);
        let d = strings.intern("D");
        let m = strings.intern("m");
        let plain = strings.intern("plain");
        let override_ = strings.intern("override");
        let deprecated = strings.intern("Deprecated");
        // The inherited row (B's own) carries the star.
        assert_eq!(
            analysis.member_has_annotation(d, m, override_),
            Some(true),
            "the inherited member's @override answers"
        );
        assert_eq!(
            analysis.member_has_annotation(d, m, deprecated),
            Some(false),
            "an absent annotation answers false, not None"
        );
        assert_eq!(
            analysis.member_has_annotation(d, plain, override_),
            Some(false),
            "a member without a star answers false"
        );
        assert_eq!(
            analysis.member_has_annotation(d, strings.intern("gone"), override_),
            None,
            "a member no chain row declares under-reports"
        );
        // The covariant gate: the inherited row's positional formal is
        // covariant; a plain method is not.
        assert_eq!(analysis.member_has_covariant_formal(d, m), Some(true));
        assert_eq!(analysis.member_has_covariant_formal(d, plain), Some(false));
        // The written default text renders back; a default-less named
        // formal answers Some(None); a positional (name-erased) formal is
        // only ever reachable by name.
        assert_eq!(
            analysis.member_formal_default_value(d, m, strings.intern("y")),
            Some(Some("3"))
        );
        assert_eq!(
            analysis.member_formal_default_value(d, m, strings.intern("z")),
            Some(None)
        );
        assert_eq!(
            analysis.member_formal_default_value(d, plain, strings.intern("y")),
            Some(None),
            "a member with no formal table answers no default"
        );
    }

    /// `concrete_only` skips an abstract hit and keeps walking: the concrete
    /// redeclaration further up answers (naming its own defining
    /// container); with nothing concrete on the chain the query
    /// under-reports.
    #[test]
    fn inherited_member_concrete_only_keeps_walking_past_abstract_hits() {
        // M redeclares `m` bodyless — the abstract shape (a method is
        // abstract iff it has no body; the `abstract class` modifier alone
        // never abstracts a body-ful member) — so `concrete_only` has a
        // genuine abstract hit to walk past.
        let source = "class B { void m() {} } \
                      abstract class M extends B { void m(); } \
                      class C extends M {}";
        let analysis = analyze(source, None);
        let strings = analysis_strings(&analysis);
        let c = strings.intern("C");
        // Unfiltered: the first hit wins, abstractness notwithstanding.
        let (defining, entry) = analysis
            .inherited_member(c, strings.intern("m"), false)
            .expect("the first hit resolves");
        assert_eq!(defining, strings.intern("M"));
        assert!(entry.is_abstract);
        // Concrete-only: M's abstract row is skipped, B's concrete one answers.
        let (defining, entry) = analysis
            .inherited_member(c, strings.intern("m"), true)
            .expect("the concrete hit resolves");
        assert_eq!(defining, strings.intern("B"));
        assert!(!entry.is_abstract);
        // A chain with no concrete row at all under-reports.
        let source = "abstract class B { void a(); } class C extends B {}";
        let analysis = analyze(source, None);
        assert_eq!(
            analysis.inherited_member(strings.intern("C"), strings.intern("a"), true),
            None
        );
    }

    /// `supertype_has_instance_members` answers the three-value contract: an
    /// instance member on any written supertype (superclass or `with` mixin)
    /// is `true`, a fully indexed instance-free chain is `false` (the
    /// implicit `Object` root never counts, the anchor's own members are not
    /// supertype members), and a chain touching a name outside the indexed
    /// set — or an unresolvable anchor — is unknowable.
    #[test]
    fn supertype_instance_members_answer_the_three_values() {
        let source = "mixin M { void mm() {} } \
                      class B { void m() {} } \
                      class Empty {} \
                      class WithSuper extends B {} \
                      class WithMixin extends Empty with M {} \
                      class Bare extends Empty {} \
                      class Unknown extends Missing {}";
        let analysis = analyze(source, None);
        let strings = analysis_strings(&analysis);
        let has = |name: &str| analysis.supertype_has_instance_members(strings.intern(name));
        // The superclass chain carries an instance member.
        assert_eq!(has("WithSuper"), Some(true));
        // The `with` mixin application is part of the chain.
        assert_eq!(has("WithMixin"), Some(true));
        // Fully indexed, instance-free chain.
        assert_eq!(has("Bare"), Some(false));
        // The anchor's own members are not supertype members.
        assert_eq!(has("B"), Some(false));
        // A superclass outside the indexed set leaves the chain unknowable.
        assert_eq!(has("Unknown"), None);
        // So does an unresolvable anchor.
        assert_eq!(has("Missing"), None);
    }

    /// `implements_core_interface` answers the declared supertype graph: a
    /// chain reaching the interface spelling answers `true` (a definite
    /// `true` outranks everything), a chain whose only miss is a dead edge
    /// (an undefined/non-class spelling — the oracle's InvalidType
    /// contributes nothing to the interface) answers `false`, an edge that
    /// resolves but sits outside the indexed set leaves the answer open,
    /// and the core face answers the SDK spellings directly — while a user
    /// container spelled `Iterable` resolves indexed and is walked as a
    /// user type, never a core match.
    #[test]
    fn implements_core_interface_walks_the_supertype_graph() {
        let source = "class A implements B {} \
                      class B implements Iterable<int> {} \
                      class Alone {} \
                      class Open implements Missing {} \
                      class HalfOpen implements Missing, Iterable<int> {} \
                      class ThroughCore<T> implements List<T> {}";
        let analysis = analyze(source, None);
        let strings = analysis_strings(&analysis);
        let iterable = strings.intern("Iterable");
        let asks = |name: &str| {
            analysis.implements_core_interface(Ty::named(strings.intern(name)), iterable)
        };
        // The chain hits the interface spelling two hops out.
        assert_eq!(asks("A"), Some(true));
        assert_eq!(asks("B"), Some(true));
        // Fully indexed, no hit anywhere on the graph.
        assert_eq!(asks("Alone"), Some(false));
        // A dead edge (undefined spelling) contributes nothing: the definite
        // miss survives as `false` (the oracle decides
        // `argument_type_not_assignable` on such receivers — the
        // `dead_implements_edge_breaks_assignability` pin).
        assert_eq!(asks("Open"), Some(false));
        // …but a definite `true` on a sibling edge outranks the dead one.
        assert_eq!(asks("HalfOpen"), Some(true));
        // The core face: `List` implements `Iterable` by definition, so
        // `ThroughCore<T> implements List<T>` answers without the written
        // type-argument substitution entering.
        assert_eq!(asks("ThroughCore"), Some(true));
        assert_eq!(asks("List"), Some(true));
        assert_eq!(asks("Set"), Some(true));
        // The core spelling of the interface itself.
        assert_eq!(asks("Iterable"), Some(true));
        // An unindexed name outside the core face stays open (`int` is
        // definitely not an `Iterable` in the oracle's core graph, but the
        // face carries no negative facts — the under-report direction).
        assert_eq!(asks("int"), None);
        // Top types: not interface carriers vs. undecided.
        assert_eq!(
            analysis.implements_core_interface(Ty::Dynamic, iterable),
            Some(false)
        );
        assert_eq!(
            analysis.implements_core_interface(Ty::UNKNOWN, iterable),
            None
        );

        // The shadowing corpus: a user `Iterable` is not `dart:core`'s, at
        // the seed or on an edge — the resolution decides, never the
        // spelling.
        let source = "class Iterable {} \
                      class Shadowed implements Iterable {}";
        let analysis = analyze(source, None);
        let strings = analysis_strings(&analysis);
        let iterable = strings.intern("Iterable");
        let asks = |name: &str| {
            analysis.implements_core_interface(Ty::named(strings.intern(name)), iterable)
        };
        assert_eq!(asks("Iterable"), Some(false));
        assert_eq!(asks("Shadowed"), Some(false));
    }

    /// The walk carries the application and constraint edges: a `with`
    /// mixin's own `implements` clause answers, a mixin's `on` constraints
    /// are its supertypes (an application satisfying the constraint through
    /// its superclass reaches the interface through the mixin's graph), and
    /// the `Iterable` spelling itself answers through the core face when no
    /// user container shadows it.
    #[test]
    fn implements_core_interface_walks_mixin_edges() {
        let source = "mixin M implements Iterable<int> {} \
                      class WithMixin with M {} \
                      class Base {} \
                      mixin N on Base implements Iterable<int> {} \
                      class OnMixin extends Base with N {}";
        let analysis = analyze(source, None);
        let strings = analysis_strings(&analysis);
        let iterable = strings.intern("Iterable");
        let asks = |name: &str| {
            analysis.implements_core_interface(Ty::named(strings.intern(name)), iterable)
        };
        // The applied mixin's declared interfaces answer.
        assert_eq!(asks("WithMixin"), Some(true));
        // The constraint edge: the mixin's graph reaches the interface.
        assert_eq!(asks("OnMixin"), Some(true));
        // The constraint type's own graph is empty and indexed.
        assert_eq!(asks("Base"), Some(false));
        // The core spelling of the interface itself.
        assert_eq!(asks("Iterable"), Some(true));
    }

    /// The walk crosses files: the interface hit sits in the dependency's
    /// model, reached through the superclass edge the namespace resolves.
    #[test]
    fn implements_core_interface_walks_cross_file() {
        let dependency = "class Dep implements Iterable<int> {}";
        let analysis = analyze_imports(
            "import 'kit0.dart';\nclass Sub extends Dep {}",
            &[(dependency, None)],
        );
        let strings = analysis_strings(&analysis);
        assert_eq!(
            analysis.implements_core_interface(
                Ty::named(strings.intern("Sub")),
                strings.intern("Iterable")
            ),
            Some(true)
        );
    }

    /// An extension type erases to its representation type for the
    /// interface check (`extensionTypeErasure`): a wrapper over `List`
    /// answers through the core face, its own `implements` clause never
    /// enters, and an unmodeled representation shape stays open.
    #[test]
    fn implements_core_interface_erases_extension_types() {
        let source = "extension type Wrapper(List<int> it) {} \
                      extension type Eager(int it) implements Iterable<int> {} \
                      extension type Fn(void Function() it) {}";
        let analysis = analyze(source, None);
        let strings = analysis_strings(&analysis);
        let iterable = strings.intern("Iterable");
        let asks = |name: &str| {
            analysis.implements_core_interface(Ty::named(strings.intern(name)), iterable)
        };
        // The representation type answers: `List` is core-`Iterable`.
        assert_eq!(asks("Wrapper"), Some(true));
        // Erasure ignores the declared clause: the erased `int` sits
        // outside the core face — open, though the oracle's core graph
        // would answer `false` (the under-report direction).
        assert_eq!(asks("Eager"), None);
        // A function-typed representation captures `Unknown` — no name to
        // erase to, the question stays open.
        assert_eq!(asks("Fn"), None);
    }

    /// `import_shadows` answers the import face and nothing else: a
    /// prefixed import contributes its prefix spelling alone (its target's
    /// exports stay outside the unit scope), an unprefixed import
    /// contributes its target's export set, and the unit's own
    /// declarations are not the import face.
    #[test]
    fn import_shadows_answers_the_import_face() {
        let dependency = "class A {}\nvoid shared() {}";
        let analysis = analyze_imports(
            "import 'kit0.dart' as p;\nclass B {}",
            &[(dependency, Some("p"))],
        );
        let strings = analysis_strings(&analysis);
        assert!(
            !analysis.import_shadows(strings.intern("A")),
            "a prefixed import's target names stay outside the unit scope"
        );
        assert!(analysis.import_shadows(strings.intern("p")));
        assert!(
            !analysis.import_shadows(strings.intern("B")),
            "the unit's own declarations are not the import face"
        );

        let analysis = analyze_imports("import 'kit0.dart';", &[(dependency, None)]);
        let strings = analysis_strings(&analysis);
        assert!(analysis.import_shadows(strings.intern("A")));
        assert!(analysis.import_shadows(strings.intern("shared")));
        assert!(!analysis.import_shadows(strings.intern("gone")));
    }

    /// An unprefixed import whose target is outside the indexed set may
    /// export any name: the conservative `true` keeps the shadowing
    /// question open.
    #[test]
    fn import_shadows_answers_true_on_an_unresolvable_target() {
        let analysis = analyze_imports("import 'missing.dart';", &[("", None)]);
        let strings = analysis_strings(&analysis);
        assert!(analysis.import_shadows(strings.intern("anything")));
    }

    /// `library_uri` answers the enclosing library's canonical identity —
    /// the `package:` spelling the graph interned it under, the
    /// defining-library string shape the oracle's `element.library.uri`
    /// comparisons read (upstream Dart SDK source, pkg/analyzer: the
    /// defining compilation unit's URI, not the import spelling).
    #[test]
    fn library_uri_answers_the_defining_library_identity() {
        let analysis = analyze("void f() {}", None);
        assert_eq!(analysis.library_uri(), "package:t/main.dart");
    }

    /// `uri_of_file` attributes a registered unit to its defining library's
    /// URI: the enclosing unit's own file answers the enclosing library,
    /// and an imported dependency's defining file — reached exactly as a
    /// rule consumer reaches it, a resolved [`Binding`]'s `target_file` —
    /// answers the dependency's own library, not the importer's. The
    /// lib/package/part mapping itself is pinned at the graph layer
    /// (`library_of_file`).
    #[test]
    fn uri_of_file_attributes_registered_units_to_their_owning_library() {
        let dependency = "void dep() {}";
        let source = "import 'kit0.dart';\nvoid f() { dep(); }";
        let analysis = analyze_imports(source, &[(dependency, None)]);
        let own = analysis.library_units()[0];
        assert_eq!(analysis.uri_of_file(own), Some("package:t/main.dart"));
        // The needle carries the trailing `;` so the span lands on the
        // invocation callee, not any declaration-side spelling.
        let binding = analysis
            .binding_of(token_of(source, "dep();"))
            .expect("the dependency's declaration resolves");
        assert_eq!(
            analysis.uri_of_file(binding.target_file),
            Some("package:t/kit0.dart"),
            "the dependency's defining file attributes to its own library"
        );
    }

    /// `container_has_annotation` reads only the resolved container's own
    /// annotation rows, cross-file like the member walk: an imported and
    /// an in-file `@JS` container answer `Some(true)`, an unannotated one
    /// `Some(false)`, and an unresolvable name `None`.
    #[test]
    fn container_has_annotation_walks_the_resolved_container() {
        let dependency = "@JS()\nclass I {}";
        let source = "import 'kit0.dart';\n@JS()\nclass L {}\nclass P {}";
        let analysis = analyze_imports(source, &[(dependency, None)]);
        let strings = analysis_strings(&analysis);
        let js = strings.intern("JS");
        assert_eq!(
            analysis.container_has_annotation(strings.intern("I"), js),
            Some(true),
            "the imported container's own rows answer"
        );
        assert_eq!(
            analysis.container_has_annotation(strings.intern("L"), js),
            Some(true)
        );
        assert_eq!(
            analysis.container_has_annotation(strings.intern("P"), js),
            Some(false)
        );
        assert_eq!(
            analysis.container_has_annotation(strings.intern("Missing"), js),
            None,
            "an unresolvable container is unknowable"
        );
    }

    /// `resolve_prefixed_container` resolves through the prefix's target
    /// library, not the unit's own scope: `p.C` answers the dependency's
    /// container (distinguished by its named constructor, which the
    /// same-named local lacks), a name the prefix's table lacks and an
    /// unregistered prefix under-report.
    #[test]
    fn resolve_prefixed_container_walks_the_prefix_target() {
        let dependency = "class C { C.named(); }";
        let source = "import 'kit0.dart' as p;\nclass C {}";
        let analysis = analyze_imports(source, &[(dependency, Some("p"))]);
        let strings = analysis_strings(&analysis);
        let p = strings.intern("p");
        let c = strings.intern("C");
        // The prefix's table answers: the dependency's C, not the local one.
        let (container, _) = analysis
            .resolve_prefixed_container(p, c)
            .expect("the prefixed name resolves through the prefix's target");
        assert!(
            container.constructors.contains(&strings.intern("named")),
            "the resolved container is the prefix target's C, not the local one"
        );
        // A name the prefix's table lacks stays silent.
        assert!(
            analysis
                .resolve_prefixed_container(p, strings.intern("Missing"))
                .is_none()
        );
        // So does an unregistered prefix.
        assert!(
            analysis
                .resolve_prefixed_container(strings.intern("q"), c)
                .is_none()
        );
    }

    /// `matched_value_type` answers the scrutinee read's flow type — the
    /// nullable declared type a pattern matches against — and under-reports
    /// for untyped reads and non-read scrutinees.
    #[test]
    fn matched_value_type_answers_the_scrutinee_read() {
        let source = "void f(int? x) { switch (x) { case 1: break; } } \
                      void g(y) { switch (yy) { case 1: break; } } \
                      void h() { switch (1) { case 1: break; } }";
        let analysis = analyze(source, None);
        let strings = analysis_strings(&analysis);
        // The token one past an opening parenthesis: the scrutinee read.
        let at = |needle: &str| SourceRange {
            offset: span_of(source, needle).offset + 1,
            length: 1,
        };
        assert_eq!(
            analysis.matched_value_type(at("(x)")),
            Some(Ty::Named {
                name: strings.intern("int"),
                nullable: true,
                args: ArgListId::EMPTY,
            })
        );
        assert_eq!(
            analysis.matched_value_type(at("(yy)")),
            None,
            "an untyped read under-reports"
        );
        assert_eq!(
            analysis.matched_value_type(at("(1)")),
            None,
            "a literal scrutinee carries no read row"
        );
    }

    /// The if-case condition answers through the same read-row path: the
    /// condition of `if (x case …)` is parsed and evaluated as the ordinary
    /// expression it is, so its bare read carries the read row the face
    /// keys on — the same model the official pattern walk reaches an
    /// if-statement's case clause by (upstream Dart SDK source,
    /// pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart
    /// `ifCaseStatement_afterExpression` + `_getMatchedValueType`).
    #[test]
    fn matched_value_type_answers_an_if_case_condition() {
        let source = "void f(int? x) { if (x case 1) {} } \
                      void g(y) { if (yy case 1) {} }";
        let analysis = analyze(source, None);
        let strings = analysis_strings(&analysis);
        let at = |needle: &str| SourceRange {
            offset: span_of(source, needle).offset + 1,
            length: 1,
        };
        assert_eq!(
            analysis.matched_value_type(at("(x case")),
            Some(Ty::Named {
                name: strings.intern("int"),
                nullable: true,
                args: ArgListId::EMPTY,
            })
        );
        assert_eq!(
            analysis.matched_value_type(at("(yy case")),
            None,
            "an untyped read under-reports"
        );
    }
}
