use super::*;

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
    /// the application classes `S&M2` are still the chain).
    pub(crate) fn walk_from(&self, container: StringId, member: StringId, mut skip_own: bool) -> Walk<'a> {
        let mut name = container;
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
        let mut pending_faces: Vec<Vec<(StringId, FileId)>> = Vec::new();
        loop {
            if !seen.insert(name) {
                return Walk::ChainBroken;
            }
            let Some((found, found_owner)) = resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                scope_lib,
                owner,
                name,
            ) else {
                // The anchor itself unresolved (an undefined receiver-type
                // spelling) is the oracle's InvalidType: member reads ride
                // dynamic and stay silent. A dead-edge HOP (undefined,
                // ambiguous, or non-class supertype spelling) contributes
                // nothing to the interface: the climb ends in a clean miss
                // and the member lookups against the remaining face report.
                // A declaration the index cannot model (the synthetic SDK
                // face) stays the broken chain either way — the oracle sees
                // members we cannot walk.
                if !at_anchor && self.supertype_hop_dead(name, scope_lib, owner) {
                    return Walk::Missing;
                }
                return Walk::ChainBroken;
            };
            at_anchor = false;
            if !skip_own && let Some(entry) = found.members.get(&member) {
                // F1: the found entry belongs to `found_owner`'s pool —
                // project it before it flows into rows or message text.
                return Walk::Found(self.boundary.member(found_owner, name, member, entry));
            }
            skip_own = false;
            // The stacked applications, later first: each shadows the members
            // of the earlier ones and of the superclass. The mixin names are
            // written in `found_owner` and resolve through that file's
            // library (probed by the container name it declares, #491 S2a).
            let owner_lib = self
                .owner_library_of(found_owner, name)
                .unwrap_or(scope_lib);
            for mixin in found.with_clause.iter().rev() {
                if !seen.insert(*mixin) {
                    return Walk::ChainBroken;
                }
                let mixin_lib = owner_lib;
                if let Some((mixin_container, mixin_owner)) = resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    mixin_lib,
                    found_owner,
                    *mixin,
                ) && let Some(entry) = mixin_container.members.get(&member)
                {
                    return Walk::Found(self.boundary.member(mixin_owner, *mixin, member, entry));
                }
            }
            if !found.implements_clause.is_empty() {
                pending_faces.push(
                    found
                        .implements_clause
                        .iter()
                        .map(|face| (*face, found_owner))
                        .collect(),
                );
            }
            let Some(superclass) = found.superclass else {
                // The `extends` chain ends: the collected interfaces still
                // contribute members — every face met on the climb, deepest
                // level first (a superclass interface already merged its
                // `implements` faces, so it shadows the implementor's). The
                // shared face-list climb then expands each face's own
                // supertypes transitively. No faces at all is the clean miss.
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
    /// declaring file)` contributes its own (non-static) members and,
    /// transitively, its supertypes' (a class climbs `extends`/`with`/
    /// `implements`, a mixin interface its own interfaces, a nested extension
    /// type its `implements`). A resolved member ends the walk (`Found`); an
    /// unresolvable or cyclic face poisons the chain (`ChainBroken`, the
    /// under-report boundary) only when nothing answered. Each face name is
    /// written in its declaring file and resolves through that container's
    /// library.
    pub(crate) fn walk_face_list(
        &self,
        seeds: impl Iterator<Item = (StringId, FileId)>,
        mut scope_lib: LibraryId,
        member: StringId,
        seen: &mut FxHashSet<StringId>,
        climb_on: bool,
    ) -> Walk<'a> {
        // Breadth-first: the declaration order is the oracle's face order,
        // and a shallower hit shadows a deeper one.
        let mut queue: VecDeque<(StringId, FileId)> = seeds.collect();
        let mut missing = false;
        while let Some((face, face_owner)) = queue.pop_front() {
            if !seen.insert(face) {
                continue;
            }
            let face_lib = self
                .owner_library_of(face_owner, face)
                .unwrap_or(scope_lib);
            let Some((face_container, face_container_owner)) = resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                face_lib,
                face_owner,
                face,
            ) else {
                // An unresolvable interface face — a dead edge (undefined or
                // non-class spelling, the oracle's InvalidType) contributes
                // nothing; only a declaration the index cannot model (the
                // synthetic SDK face) records the broken hop. Either way the
                // remaining faces keep walking.
                if !self.supertype_hop_dead(face, face_lib, face_owner) {
                    missing = true;
                }
                continue;
            };
            scope_lib = face_lib;
            if let Some(entry) = face_container.members.get(&member)
                && !entry.is_static
            {
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
                face_container.on_clause.as_slice()
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
                queue.push_back((super_face, face_container_owner));
            }
        }
        if missing {
            Walk::ChainBroken
        } else {
            Walk::Missing
        }
    }

    /// `super.member`: the walk starts at the enclosing container but skips
    /// its own members — the official `S&M2` superclass chain still applies
    /// (`super.m` in `class C extends S with M1, M2` resolves M2 over M1
    /// over S). A container without a captured superclass edge and without
    /// mixins (mixins: `on` constraints are not edges) breaks the chain.
    pub(crate) fn walk_super(&self, container: StringId, member: StringId) -> Walk<'a> {
        let Some(container_decl) = self
            .index
            .get(self.file)
            .and_then(|f| f.container(container))
        else {
            return Walk::ChainBroken;
        };
        if container_decl.superclass.is_none() && container_decl.with_clause.is_empty() {
            // A mixin's super face is its `on` clause: `super.m` inside
            // `mixin M on A, B` resolves `m` against the superclass
            // constraints' member pools (each constraint's own members, then
            // its supertypes — the oracle's `unpromotedThisType` interface,
            // type_analyzer.dart `superLookupType`). Constraints are not
            // supertype edges in the model, so the regular `walk_from` never
            // sees them; seed the shared face-list climb with the `on` list.
            if container_decl.kind == ContainerKind::Mixin && !container_decl.on_clause.is_empty() {
                let mut seen: FxHashSet<StringId> = FxHashSet::default();
                seen.insert(container);
                return self.walk_face_list(
                    container_decl
                        .on_clause
                        .iter()
                        .map(|face| (*face, self.file)),
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
        self.member_faces.walk(container, member, true, || {
            self.walk_from(container, member, true)
        })
    }

    // —— invalid_override (#453: the inheritance-override family) ——————

    /// Checks one instance-member declaration against the container's direct
    /// super-faces (upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/error/inheritance_override.dart:498-546`
    /// `_checkDeclaredMember`): for each face, the nearest same-named
    /// non-static member; kind mismatches skip (other checks own them), and
    /// a decidable name-level signature conflict emits `invalid_override` at
    /// the member's name span. Every unresolvable hop, cycle, or
    /// incomparable type suppresses — the under-report direction, never a
    /// false positive.
    ///
    /// Runs from the parse listener, so the element model (built by the
    /// earlier `collect`) is complete and the declaration's name token
    /// supplies the anchor span. Emissions repeat per failing face, matching
    /// the oracle's per-superinterface loop. Each row carries the oracle's
    /// context row anchored at the overridden member's declaration
    /// ([`overridden_member_context`]).
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
        // empty for every other kind).
        let mut faces = Vec::with_capacity(
            usize::from(decl.superclass.is_some())
                + decl.with_clause.len()
                + decl.implements_clause.len()
                + decl.on_clause.len(),
        );
        faces.extend(decl.superclass);
        faces.extend(decl.with_clause.iter().copied());
        faces.extend(decl.implements_clause.iter().copied());
        faces.extend(decl.on_clause.iter().copied());
        let mut broken_seen: Vec<Projected<'a, MemberEntry>> = Vec::new();
        for face in faces {
            let Some((inherited, face_name, origin)) = self.face_member(face, spelling) else {
                // Unresolvable or cyclic face chain: suppression territory
                // (recovery residue in a clause lands here too).
                continue;
            };
            if inherited.kind != entry.kind {
                continue;
            }
            // The stacked climb can surface one inherited member through
            // several direct faces (the applied `S&M2` superclass face and
            // the `M2` face both answer M2's member); the oracle's verdict
            // per inherited member is one diagnostic. The boundary's memo
            // keeps a projected entry's identity stable across faces.
            if broken_seen
                .iter()
                .any(|prior| prior.ptr() == inherited.ptr())
            {
                continue;
            }
            if self.override_broken(
                entry,
                (self.file, container),
                &inherited,
                (origin, face_name),
            ) {
                broken_seen.push(inherited.clone());
                let display = self.strings.resolve(spelling);
                let display = display.strip_suffix('=').unwrap_or(display);
                let container_text = self.strings.resolve(container);
                let declared_text = self.override_signature_text(entry);
                let face_text = self.strings.resolve(face_name);
                let inherited_text = self.override_signature_text(&inherited);
                let setter = entry.kind == MemberKind::Setter;
                let code = if setter {
                    SemanticCode::InvalidOverrideSetter
                } else {
                    SemanticCode::InvalidOverride
                };
                self.emit5_with_context(
                    code,
                    range,
                    display,
                    container_text,
                    &declared_text,
                    face_text,
                    &inherited_text,
                    overridden_member_context(&inherited, origin, setter),
                );
            }
        }
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
        let mut spared = !in_extension && self.face_member_any(decl, spelling);
        if !spared && !in_extension && entry.kind == MemberKind::Field {
            let setter = self.setter_spelling(spelling);
            spared = self.face_member_any(decl, setter);
        }
        if !spared && !in_extension && entry.kind == MemberKind::Setter {
            // The accessor-twin face: an inherited FIELD induces the setter
            // face too (`_CastQueueList`'s `set _head` overrides
            // `QueueList`'s `int _head` field — the field's implicit setter,
            // #2172). The mirror arm above answers a declared field against
            // an inherited setter's spelling.
            let base_text = self.strings.resolve(spelling);
            let base_text = base_text.strip_suffix('=').unwrap_or(base_text);
            spared = self.face_member_any(decl, self.intern(base_text));
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
    /// (the oracle's transitive `getOverridden`, via [`Engine::face_member`]'s
    /// per-face climb). An unresolvable face simply contributes nothing —
    /// except an unindexed dart:core interface, whose declared members the
    /// frozen [`CORE_INTERFACE_MEMBERS`] face answers (a user-declared
    /// same-named container resolves indexed and wins the face first).
    pub(crate) fn face_member_any(&self, decl: &Container, spelling: StringId) -> bool {
        let mut faces = Vec::new();
        faces.extend(decl.superclass);
        faces.extend(decl.with_clause.iter().copied());
        faces.extend(decl.implements_clause.iter().copied());
        faces.extend(decl.on_clause.iter().copied());
        faces
            .into_iter()
            .any(|face| self.face_member(face, spelling).is_some() || self.face_member_frozen(face, spelling))
    }

    /// The frozen-table half of [`Self::face_member_any`]: climbs the face's
    /// resolvable supertype chain like [`Self::face_member`], but answers at
    /// the hops the element index cannot carry — an unindexed dart:core /
    /// `dart:collection` container decides by its frozen member face
    /// ([`CORE_TYPE_MEMBERS`] / [`CORE_INTERFACE_MEMBERS`], aliased for the
    /// collection mixins/bases that re-export a `dart:core` interface's
    /// members, #2161). A hop that resolves to an indexed container whose
    /// own member walk misses is a real miss and stops the climb.
    pub(crate) fn face_member_frozen(&self, face: StringId, spelling: StringId) -> bool {
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
        let mut owner = self.file;
        let mut scope_lib = self.lib;
        let mut seen = FxHashSet::default();
        // Same pending queue [`Self::face_member`] drains: a resolved hop's
        // `implements`/`on` faces keep the frozen climb alive when the linear
        // `extends`/`with` chain ends (`UnmodifiableSetMixin implements Set`
        // reaching `Set`'s frozen members).
        let mut pending: VecDeque<(StringId, FileId)> = VecDeque::new();
        loop {
            if !seen.insert(name) {
                return false;
            }
            let Some((found, found_owner)) = resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                scope_lib,
                owner,
                name,
            ) else {
                // The unindexed hop — a dart:core/`dart:collection` face —
                // answers by its frozen member face.
                if frozen_hit(self.strings.resolve(name)) {
                    return true;
                }
                let Some((next, next_owner)) = pending.pop_front() else {
                    return false;
                };
                name = next;
                owner = next_owner;
                continue;
            };
            if let Some(entry) = found.members.get(&spelling)
                && !entry.is_static
            {
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
                match resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    owner_lib,
                    owner,
                    *mixin,
                ) {
                    Some((mixin_container, _))
                        if mixin_container
                            .members
                            .get(&spelling)
                            .is_some_and(|entry| !entry.is_static) =>
                    {
                        return true;
                    }
                    None if frozen_hit(self.strings.resolve(*mixin)) => {
                        mixin_hit = true;
                    }
                    _ => {}
                }
            }
            if mixin_hit {
                return true;
            }
            for super_face in found
                .implements_clause
                .iter()
                .chain(found.on_clause.iter())
            {
                pending.push_back((*super_face, found_owner));
            }
            match found.superclass {
                Some(superclass) => {
                    scope_lib = owner_lib;
                    name = superclass;
                    owner = found_owner;
                }
                None => {
                    let Some((next, next_owner)) = pending.pop_front() else {
                        return false;
                    };
                    name = next;
                    owner = next_owner;
                    scope_lib = owner_lib;
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
        let mut faces: Vec<StringId> = decl.with_clause.clone();
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
    /// (`isPotentiallyNonNullable`, type_system.dart:1257): a bare written
    /// named type is, `?`-suffixed and `dynamic`/`void` are not. An
    /// in-scope type parameter's head suppresses — its resolved
    /// nullability rests on the bound the name-level model does not keep
    /// (the under-report posture).
    pub(crate) fn written_potentially_non_nullable(&self, ty: Ty) -> bool {
        match ty {
            Ty::Named { name, nullable, .. } => {
                !nullable
                    && !self.type_params.contains(&name)
                    && !self.pending_type_params.contains(&name)
            }
            _ => false,
        }
    }

    /// Stages the current optional formal for the body-verdict flush: only
    /// a potentially-non-nullable written type without a default value
    /// stages (error_verifier.dart:8394-8418); primary constructors decide
    /// immediately (`defaultValuesAreExpected` answers true unconditionally
    /// there, :8368-8370).
    pub(crate) fn stage_default_check(&mut self, check: PendingDefaultCheck) {
        if self.written_potentially_non_nullable(check.written) {
            self.pending_default_checks.push(check);
        }
    }

    /// Emits the staged optional-parameter rows per the body verdict
    /// (`defaultValuesAreExpected`, error_verifier.dart:8327-8372): a
    /// block/arrow body expects defaults for every executable face, an
    /// empty `;` body only for top-level functions (methods answer
    /// `isAbstract` false there, top-level functions only exclude
    /// augment/external/native).
    pub(crate) fn flush_default_checks(&mut self, body_present: bool) {
        if self.open_exec_augment {
            self.pending_default_checks.clear();
            return;
        }
        let checks = std::mem::take(&mut self.pending_default_checks);
        for check in checks {
            let top_level = check.member_kind == dartq_parser::MemberKind::TopLevelMethod;
            if !(body_present || top_level) {
                continue;
            }
            let code = if check.named {
                SemanticCode::MissingDefaultValueForParameter
            } else {
                SemanticCode::MissingDefaultValueForParameterPositional
            };
            let name = self.strings.resolve(check.name).to_string();
            self.stage_usage(code, check.name_range, &name);
        }
    }

    /// Drops the staged rows (the native/skipped-body arms of
    /// `defaultValuesAreExpected`).
    pub(crate) fn drop_default_checks(&mut self) {
        self.pending_default_checks.clear();
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
    pub(crate) fn block_interior_is_the_candidate(&self, begin: Token, end: Token, check: &NsmCheck) -> bool {
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

    /// The declaration-site combination face (`parser_impl.dart:2976-3015`
    /// through [`declaration_violations`]): the written header keywords
    /// decide, and each violation anchors on its keyword token(s) — the
    /// single keyword for the one-anchor rows, the `final`→`base`/
    /// `interface` coverage for the spanned `abstract final …` pairs.
    /// `anchors` carries the present keywords' spans.
    pub(crate) fn check_modifier_declaration(
        &mut self,
        modifiers: ClassModifiers,
        anchors: &[(ModifierKeyword, SourceRange)],
    ) {
        for violation in declaration_violations(&modifiers) {
            let range = violation_span(&violation, anchors);
            // The declaration-site rows are frozen zero-argument templates.
            self.emit_modifier(violation.code, range, "", "");
        }
    }

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
    /// closer — the oracle's whole-NamedType anchor (the prefixed `p.C`
    /// heads complete on the prefix token; the span starts there and stops
    /// at the head token, the model's bare-name convention).
    pub(crate) fn note_header_type_span(&mut self, name: Token, question: Option<Token>) {
        // #1444: the extension-type header's clause-entry ledger (the
        // modifier window below stays unarmed for extension types; the
        // global type-argument depth excludes argument positions).
        if self.extension_clause_phase == ExtensionClausePhase::Clauses && self.type_arg_depth == 0
        {
            self.extension_clause_miss.push(name.range.offset);
        }
        let Some(deferred) = self.deferred_modifier.as_mut() else {
            return;
        };
        let spans = &mut deferred.spans;
        if spans.arg_depth > 0 {
            return;
        }
        let mut range = name.range;
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
            if let Some(container) = deferred.container {
                self.check_modifier_use_sites(container, deferred.name_range, &deferred.spans);
            }
        }
        self.deferred_modifier = Some(deferred);
    }

    /// The `@sealed` half, drained at the declaration's end event: the
    /// official anchor is the whole declaring node (`_checkForInvalidSealedSuperclass`
    /// reports `.at(node)`), whose close is first known here. `end` is the
    /// declaration's closing token (`}`/`;`); the recovery paths that
    /// complete the declaration without a body close fall back to the end
    /// event's token start.
    /// The `conflicting_static_and_instance` face: each static member whose
    /// base name collides with an instance member of the same container
    /// reports at the static member's own name, in declaration order, one
    /// row per site (member_duplicate_definition_verifier.dart:194-232 for
    /// enums, the ErrorVerifier's separated class face :430-470; `values`
    /// is exempt on both).
    pub(crate) fn check_conflicting_static_and_instance(&mut self) {
        let Some((container, _)) = self.container_context() else {
            return;
        };
        let instance_names: FxHashSet<StringId> = self
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
                )
            })
            .map(|site| self.bare_name(site.name))
            .collect();
        if instance_names.is_empty() {
            return;
        }
        let values = self.intern("values");
        let reports: Vec<(SourceRange, StringId)> = self
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
            .map(|site| (site.range, self.bare_name(site.name)))
            .filter(|(_, base)| *base != values && instance_names.contains(base))
            .collect();
        if reports.is_empty() {
            return;
        }
        let container_text = self.strings.resolve(container).to_string();
        for (range, base) in reports {
            let base_text = self.strings.resolve(base).to_string();
            self.emit(
                SemanticCode::ConflictingStaticAndInstance,
                range,
                &container_text,
                &base_text,
            );
        }
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
    /// mixins implements/`on`; enums have no official drive.
    pub(crate) fn check_modifier_use_sites(
        &mut self,
        container: StringId,
        name_range: SourceRange,
        spans: &HeaderClauseSpans,
    ) {
        if !self.reported_modifier_checks.insert(container) {
            return;
        }
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
    pub(crate) fn check_sealed_supertype_contract(&mut self, container: StringId, decl_range: SourceRange) {
        if !self.reported_sealed_checks.insert(container) {
            return;
        }
        let (graph, namespaces, index, strings) =
            (self.graph, self.namespaces, self.index, self.strings);
        let element = (self.file, container);
        let hierarchy = FoldHierarchy {
            graph,
            namespaces,
            index,
            file: self.file,
            lib: self.lib,
            memo: RefCell::default(),
            in_progress: RefCell::default(),
        };
        // The constraint face is the DIRECT `on` list (the official
        // `superclassConstraints.contains`, :1195-1196): the same sealed
        // supertype reached through any other path reports the regular
        // subtype face, and only mixins carry `on` constraints.
        let direct_ons: FxHashSet<(FileId, StringId)> =
            hierarchy.face_handles(element).3.into_iter().collect();
        for handle in hierarchy.closure(element) {
            let Some(decl) = index
                .get(handle.0)
                .and_then(|elements| elements.container(handle.1))
            else {
                continue;
            };
            let owner_lib = hierarchy.library_of(handle);
            let is_sealed = decl
                .annotations
                .iter()
                .any(|annotation| is_meta_getter(annotation, "sealed", strings, graph, owner_lib));
            if !is_sealed || same_workspace_package(graph, owner_lib, self.lib) {
                continue;
            }
            let code = if direct_ons.contains(&handle) {
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
    /// library (#491 S2a attribution); `None` suppresses the face. The
    /// handle carries the *declaring* file — the same shape the fold walk's
    /// hop produces — so `FoldHierarchy::library_of` attributes the super's
    /// own library and the outside-library gates (`element.library !=
    /// _currentLibrary`, error_verifier.dart:5619-5622/:5917-5921/:3725-3730)
    /// compare the right pair.
    pub(crate) fn resolve_super(&self, name: StringId) -> Option<((FileId, StringId), LibraryId)> {
        let (_, owner) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        )?;
        let lib = owner_library(self.graph, self.lib, owner, name).unwrap_or(self.lib);
        Some(((owner, name), lib))
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
        let hierarchy = FoldHierarchy {
            graph: self.graph,
            namespaces: self.namespaces,
            index: self.index,
            file: self.file,
            lib: self.lib,
            memo: RefCell::default(),
            in_progress: RefCell::default(),
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
            && let Some((handle, lib)) = self.resolve_super(name)
        {
            extends = Some(ResolvedSuper {
                handle,
                lib,
                relation: Relation::Extends,
                span: spans.extends.unwrap_or(name_range),
            });
        }
        for name in &decl.with_clause {
            if let Some((handle, lib)) = self.resolve_super(*name) {
                withs.push(ResolvedSuper {
                    handle,
                    lib,
                    relation: Relation::MixesIn,
                    span: name_range,
                });
            }
        }
        for (index, name) in decl.implements_clause.iter().enumerate() {
            if let Some((handle, lib)) = self.resolve_super(*name) {
                implements.push(ResolvedSuper {
                    handle,
                    lib,
                    relation: Relation::Implements,
                    span: spans.implements.get(index).copied().unwrap_or(name_range),
                });
            }
        }
        for (index, name) in decl.on_clause.iter().enumerate() {
            if let Some((handle, lib)) = self.resolve_super(*name) {
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
        // The class-family/mixin drive split (error_verifier.dart:2770-2784
        // and :8195-8206): enums carry neither.
        let class_family = decl.kind == ContainerKind::Class;
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
        // superclass + `implements`, mixin drive `implements` + `on`;
        // :7424-7437). The official class drive also lists the `with` types
        // (:2780-2784), but that drive sits behind the
        // `_checkForAllMixinErrorCodes` gate (:2746-2752) and a sealed
        // *proper mixin* cannot exist (the declaration matrix forbids
        // `sealed mixin`) — every reachable `with` input trips the gate's
        // with-non-mixin problem, so the observable oracle never reports
        // sealed through `with`. The `with` face stays out of the list.
        let sealed_faces: Vec<&ResolvedSuper> = if class_family {
            extends.iter().chain(implements.iter()).collect()
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
        // Pass 5 — the base-or-final subtype requirement, official face
        // order superclass → implements → with → `on` (classes) /
        // implements → `on` (mixins), one error per element
        // (base_or_final_type_verifier.dart:47-74). The `{1}` slot names
        // the nearest explicit `base`/`final` ancestor, not the direct
        // super (:198/:222-315).
        let subtype_faces: Vec<&ResolvedSuper> = if class_family {
            extends
                .iter()
                .chain(implements.iter())
                .chain(withs.iter())
                .collect()
        } else {
            implements.iter().chain(ons.iter()).collect()
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
                self.implementation_overrides(decl, &view, name_range);
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
    pub(crate) fn face_entries(&self, face: StringId, top_concrete: bool) -> Option<Vec<FaceEntry<'a>>> {
        let mut name = face;
        let mut owner = self.file;
        let mut scope_lib = self.lib;
        let mut seen = FxHashSet::default();
        let mut hits: Vec<FaceEntry<'a>> = Vec::new();
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
                if let Some((mixin_container, mixin_owner)) = resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    owner_lib,
                    found_owner,
                    *mixin,
                ) {
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
                let Some((super_container, _)) = resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    superclass,
                ) else {
                    return;
                };
                // A sealed superclass is abstract at the element level
                // (element.dart:150): its interface names stay the concrete
                // subclass's to implement — no exemption.
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
            .map(|name| self.member_description(&view.representative[name], *name))
            .collect();
        self.emit_missing_members(descriptions, name_range);
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
    /// (`inheritance_override.dart:412-481`): for every interface name with
    /// an *inherited* concrete implementation — the first concrete hit over
    /// the superclass and `with` faces — that member must be a valid
    /// override of the interface map's most-specific member of the same
    /// name. This is the residue the per-member `invalid_override` pass
    /// cannot reach (no own declaration exists); the class's own concrete
    /// members stay that pass's exclusive business — the oracle's interface
    /// loop also re-reports own-member shapes, a duplication its TODO
    /// (`inheritance_override.dart:463-469`) calls out as an artifact this
    /// port does not replicate. When no candidate validly overrides all the
    /// others (the conflict shapes) the pass skips — the conflict
    /// diagnostics own those. Emissions anchor at the container's name (the
    /// oracle's `classNameToken`, whose placement its own TODO calls out).
    /// Kind mismatches skip (the oracle routes them to the member-kind
    /// verifier), and a field entry never carries the setter face (its
    /// getter-shaped type would compare with the wrong variance).
    pub(crate) fn implementation_overrides(
        &mut self,
        decl: &Container,
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
            let Some(specific) = self.most_specific(&hits) else {
                continue;
            };
            let Some(concrete) = Self::inherited_concrete_implementation(decl, view, *name) else {
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
            if !self.override_broken(
                &concrete.entry,
                (concrete.owner_file, concrete.owner_name),
                &specific.entry,
                (specific.owner_file, specific.owner_name),
            ) {
                continue;
            }
            let display = self.strings.resolve(*name);
            let display = display.strip_suffix('=').unwrap_or(display);
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

    /// The name's most-specific interface member: the unique candidate that
    /// validly overrides every other hit (the oracle's interface-map
    /// selection at name level). `None` when the candidates do not
    /// linearize — conflict territory.
    pub(crate) fn most_specific<'e>(&self, hits: &[&'e FaceEntry<'e>]) -> Option<&'e FaceEntry<'e>> {
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

    /// The name's inherited concrete implementation, `_addImplemented`'s
    /// source order: the `with` mixins first, then the superclass chain
    /// (never an `implements` face — its hits are interface members only).
    /// Within a concrete face only `concrete_source` hits count: a member
    /// reached through the face's own `implements` clause is an interface
    /// member, not an implementation the face contributes (the oracle's
    /// `implemented` map carries own members, mixins, and the super chain
    /// only — `inheritance_manager3.dart:333-351` `_addImplemented` /
    /// `_addMixinMembers`; `C extends R with M implements I` takes `M`'s
    /// and `R`'s chain's members, never `I`'s or `M`'s interfaces'). The
    /// mixin-before-superclass order is load-bearing: the oracle's
    /// concrete resolution for `C extends A with M implements I` is the
    /// *mixin's* member, and the superclass's would manufacture a conflict
    /// the oracle resolves away. The class's own members are deliberately
    /// absent: the per-member `invalid_override` pass owns their faces.
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
        for face in decl.with_clause.iter().copied().chain(decl.superclass) {
            if let Some(hit) = hits_of(face).and_then(|hits| {
                hits.iter().find(|h| {
                    h.name == name && h.concrete_source && !h.entry.is_abstract
                })
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
    pub(crate) fn member_description(&self, hit: &FaceEntry<'a>, name: StringId) -> String {
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
    pub(crate) fn emit_missing_members(&mut self, mut descriptions: Vec<String>, name_range: SourceRange) {
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

    // —— extension-type representation (#476) ————————————————————————————

    /// The semantic representation checks for one extension type, mounted at
    /// the declaration's name event (the element model is complete —
    /// `collect` parsed before this pass — and the name token anchors the
    /// type-level emissions; the official dispatch order is
    /// depends-on-itself first, the name checks last,
    /// `error_verifier.dart:1184-1193`).
    ///
    /// The syntactic representation shapes (missing formals, `this.x`/`super.x`,
    /// delimiters, function-typed suffixes, modifiers, multiple formals) are
    /// the parser's (`expected_representation_field` /
    /// `multiple_representation_fields` / `representation_field_modifier`);
    /// this side reads the surviving single representation formal.
    pub(crate) fn check_extension_type(&mut self, container: StringId, name_range: SourceRange) {
        // One emission per container per pass: an augmentation declaration
        // re-fires the event for the same container.
        if !self.reported_extension_types.insert(container) {
            return;
        }
        let Some(elements) = self.index.get(self.file) else {
            return;
        };
        let Some(decl) = elements.container(container) else {
            return;
        };
        if decl.kind != ContainerKind::ExtensionType {
            return;
        }
        let Some(rep) = decl.primary_formals.first() else {
            return;
        };
        // The representation can't depend on itself
        // (`error_verifier.dart:5343-5354`, anchored at the type name). M1
        // name-level: a direct `E` reference; a generic `List<E>`-shaped
        // dependency stays an under-report.
        if rep.formal.ty == Ty::named(container) {
            self.emit(
                SemanticCode::ExtensionTypeRepresentationDependsOnItself,
                name_range,
                "",
                "",
            );
        }
        // `_checkForExtensionTypeRepresentationErrorCodes`'s name checks
        // (:5411-5423, both non-returning): the field may not share the
        // type's name, nor an `Object` member's.
        if rep.formal.name == container {
            self.emit(SemanticCode::MemberWithClassName, rep.name_range, "", "");
        } else if self.members.object_members.contains(&rep.formal.name) {
            self.emit(
                SemanticCode::ExtensionTypeDeclaresMemberOfObject,
                rep.name_range,
                "",
                "",
            );
        }
        // The disabled-arm representation-type requirement
        // (:5449-5452): a typeless representation formal — alongside, not
        // instead of, the modifier check the parser owns.
        if rep.formal.ty == Ty::UNKNOWN && !self.primary_constructors_enabled {
            self.emit(
                SemanticCode::ExpectedRepresentationType,
                rep.name_range,
                "",
                "",
            );
        }
        // `_verifyExtensionElementImplements`'s representation face
        // (`extension_type_implements_not_supertype`) decides at the
        // file-tail bounds pass ([`Self::finalize_ext_faces`]): the faces'
        // type arguments name declarations the parse has not reached yet.
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
    /// `None` = an unresolvable or cyclic hop — the caller suppresses
    /// rather than reports.
    pub(crate) fn face_member(
        &self,
        face: StringId,
        spelling: StringId,
    ) -> Option<(Projected<'a, MemberEntry>, StringId, FileId)> {
        let mut name = face;
        let mut owner = self.file;
        let mut scope_lib = self.lib;
        let mut seen = FxHashSet::default();
        // The `implements`/`on` faces a resolved hop leaves unclimbed — the
        // linear superclass chain handles `extends`+`with`; when it dead-ends
        // (no superclass), the interface faces keep the climb alive in
        // declaration order (the oracle's `getOverridden` covers every
        // superinterface, `UnmodifiableSetMixin implements Set` reaching
        // `Set`'s frozen/interface members).
        let mut pending: VecDeque<(StringId, FileId)> = VecDeque::new();
        loop {
            if !seen.insert(name) {
                return None;
            }
            let (found, found_owner) = match resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                scope_lib,
                owner,
                name,
            ) {
                Some(pair) => pair,
                None => {
                    let Some((next, next_owner)) = pending.pop_front() else {
                        return None;
                    };
                    name = next;
                    owner = next_owner;
                    continue;
                }
            };
            if let Some(entry) = found.members.get(&spelling)
                && !entry.is_static
            {
                // F2: the inherited entry belongs to `found_owner`'s pool —
                // project it before the override faces render or compare it.
                return Some((
                    self.boundary.member(found_owner, name, spelling, entry),
                    name,
                    found_owner,
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
                if let Some((mixin_container, mixin_owner)) = resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    mixin_lib,
                    owner,
                    *mixin,
                ) && let Some(entry) = mixin_container.members.get(&spelling)
                    && !entry.is_static
                {
                    return Some((
                        self.boundary.member(mixin_owner, *mixin, spelling, entry),
                        *mixin,
                        mixin_owner,
                    ));
                }
            }
            // The interface faces this hop does not climb inline: `implements`
            // and the mixin `on` constraints join the pending queue — a face
            // whose `extends`/`with` chain ends still contributes its
            // interfaces' members.
            for super_face in found
                .implements_clause
                .iter()
                .chain(found.on_clause.iter())
            {
                pending.push_back((*super_face, found_owner));
            }
            match found.superclass {
                Some(superclass) => {
                    scope_lib = owner_lib;
                    name = superclass;
                    owner = found_owner;
                }
                None => {
                    let Some((next, next_owner)) = pending.pop_front() else {
                        return None;
                    };
                    name = next;
                    owner = next_owner;
                    scope_lib = owner_lib;
                }
            }
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
        let world = self.name_world();
        // The single type-decision point for every face below: the M1
        // assignability first, the name lattice on its `None`. No consumer
        // of this verdict re-derives either half.
        let decide = |from: (Ty, (FileId, StringId), &MemberEntry),
                      to: (Ty, (FileId, StringId), &MemberEntry)| {
            let (from_ty, from_scope, from_entry) = from;
            let (to_ty, to_scope, to_entry) = to;
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
                    // Same-head pairs stay with the covariant-argument
                    // rule; the lattice decides distinct heads only.
                    return None;
                }
                world.decide_subtype(from_name, to_name)
            })
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
        match (declared.formals.as_deref(), inherited.formals.as_deref()) {
            (Some(a), Some(b)) => {
                // Positional arity first: both directions break it.
                if a.positional.len() != b.positional.len() {
                    return true;
                }
                // The named set: every inherited name must survive (a
                // dropped named parameter rejects calls the super accepts),
                // and `required` may only tighten onto a parameter that is
                // required in the super — a newly required parameter (added
                // or strengthened) rejects calls without it. An added
                // optional named parameter is the valid-override widening
                // (oracle-verified: `void f(int x, {int? y})` over
                // `void f(int x)` is clean).
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
                // exempt — the oracle rewrites its type to `Object?` before
                // the subtype question (correct_override.dart
                // `_computeThisTypeForSubtype`), so a covariant parameter
                // never makes the override invalid; its narrowing is
                // enforced by the runtime check instead. A covariant
                // INHERITED formal flips the face covariant: the override
                // may narrow the parameter to any subtype of the
                // inherited type (the petitparser `hasEqualProperties`
                // shape — the live oracle reports no `invalid_override`
                // there).
                for (d, i) in a.positional.iter().zip(&b.positional) {
                    if i.covariant {
                        match decide(declared_at(d.ty), inherited_at(i.ty)) {
                            Some(false) => return true,
                            _ => {}
                        }
                        continue;
                    }
                    if d.covariant {
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
                        match decide(declared_at(d.ty), inherited_at(i.ty)) {
                            Some(false) => return true,
                            _ => {}
                        }
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
        if self.members.primitive_ids.contains(&name)
            || entry.type_param_names.contains(&name)
        {
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
                        let mut parts: Vec<String> = formals
                            .positional
                            .iter()
                            .map(|f| self.declared_ty_text(f.ty))
                            .collect();
                        if !formals.named.is_empty() {
                            let named = formals
                                .named
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
                format!(
                    "{} Function({params})",
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
        let report = report(
            "abstract class I { void m(); }\nabstract sealed class S implements I {}\n",
        );
        let names = code_names(&report);
        assert!(
            names.iter().all(|n| *n == "abstract_sealed_class"),
            "only the combination violation may report: {names:?}"
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
        let source_m = "import 'a.dart';\nclass C extends P { @override Missing? get s => throw 0; }\n";
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
