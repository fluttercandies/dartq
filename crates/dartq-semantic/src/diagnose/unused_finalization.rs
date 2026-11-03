use super::*;

/// The diagnostic codes whose presence anywhere in the unit skips the
/// whole import-warning pass (upstream Dart SDK source,
/// library_analyzer.dart `_hasDiagnosticReportedThatPreventsImportWarnings`
/// — the exact set, machine spellings lowered to the engine's code names).
/// The upstream list's `constWithNonType` and `newWithNonType` entries both
/// carry the machine name `creation_with_non_type` (diagnostic.g.dart
/// :3237 and :12035 — one registry spelling shared by the two unique
/// names), which is also this engine's emitted spelling (`codes.rs`), so a
/// single entry covers both.
const IMPORT_WARNING_GATE: &[&str] = &[
    "ambiguous_import",
    "creation_with_non_type",
    "extends_non_class",
    "implements_non_class",
    "mixin_of_non_class",
    "not_a_type",
    "prefix_identifier_not_followed_by_dot",
    "undefined_annotation",
    "undefined_class",
    "undefined_function",
    "undefined_identifier",
    "undefined_prefixed_name",
    "deprecated_export_use",
];

/// The super-initialization token walk's backward cap (tokens scanned from
/// a `super` occurrence to its containing construct opener and its class
/// header): a constructor header plus its initializer entries never
/// approach this, so exhaustion means recovery residue — the occurrence is
/// skipped (the under-report direction), never spun over.
const SUPER_WALK_BACKWARD_CAP: usize = 8192;

impl<'a> Engine<'a> {
    /// The with-clause entry face (`class_used_as_mixin`, messages.yaml
    /// :2737-2741): every entry span whose name resolves to a plain class —
    /// neither `mixin` nor `mixin class` — reports at the entry name. Runs
    /// after the parse so a same-file later declaration resolves; an
    /// unresolvable entry stays with the clause ledger's own faces.
    pub(crate) fn finalize_mixin_uses(&mut self) {
        for span in std::mem::take(&mut self.pending_mixin_uses) {
            // The span is UTF-16; the slice reads source bytes (#2594). It
            // anchors the whole NamedType — `p.C`, `C<int>`, `C?` — so the
            // resolvable head is the spelling up to the first `<` or `?`,
            // not the full slice (`error_verifier.dart` `_checkForMixinIn
            // WithClause` resolves the type's ELEMENT, never the written
            // suffixes).
            let start = self.byte_of(span.offset);
            let end = self.byte_of(span.end()).min(self.source.len());
            if start >= end {
                continue;
            }
            let text = &self.source[start..end];
            let head_end = text.find(['<', '?']).map(|i| start + i).unwrap_or(end);
            let head = self.intern(&self.source[start..head_end]);
            // A dotted spelling (`p.C`) resolves the leaf through the
            // prefix's own table — the bare-scope walk would answer the
            // diagnosing library's same-named container, the wrong scope.
            let resolved = if self.strings.resolve(head).contains('.') {
                resolve_joined_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.strings,
                    head,
                )
            } else {
                resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    head,
                )
            };
            // The container-miss alias fold (`_checkForAllMixinErrorCodes`
            // reads `mixinType.element` — the instantiated alias's element,
            // error_verifier.dart:2879-2890): `with A` over `typedef A = C`
            // resolves C's element and reports with C's name; the same fold
            // lands a mixin-class target (`typedef A = MixinClass`) silent.
            let (resolved, element) = match resolved {
                Some(face) => (Some(face), head),
                None => match self.mixin_use_alias_fold(head) {
                    Some((container, owner, folded)) => {
                        (Some((container, owner)), folded)
                    }
                    None => (None, head),
                },
            };
            let Some((container, owner)) = resolved else {
                continue;
            };
            if container.kind != ContainerKind::Class
                || container_modifiers(container).is_mixin_class
            {
                continue;
            }
            // The resolved element's non-subtypable-class gate
            // (`_checkForAllMixinErrorCodes`'s first arm reads
            // `_checkForExtendsOrImplementsDisallowedClass`, which answers
            // `isNonSubtypableClass`, error_verifier.dart:2886-2888 and
            // :5176-5188): a restricted `dart:*` element routes to the
            // `subtype_of_disallowed_type` face this engine already emits
            // and never reaches `classUsedAsMixin` — except `Enum`, whose
            // `isDartCoreEnum` conjunct lets the element fall through to
            // the class arm. The element leaf decides; a user shadow
            // resolves off `dart:*` and keeps the report.
            let element_leaf = {
                let spelled = self.strings.resolve(element);
                spelled.rsplit('.').next().unwrap_or(spelled)
            };
            let element_is_sdk = self
                .graph
                .library_of_file(owner)
                .is_some_and(|lib| self.graph.uri(lib).starts_with("dart:"));
            if element_is_sdk
                && element_leaf != "Enum"
                && super::engine_core::NON_SUBTYPABLE_SDK_NAMES.contains(&element_leaf)
            {
                continue;
            }
            // The mixin element's own library must carry class modifiers
            // and the use must not ride the platform exemption
            // (error_verifier.dart `_checkForMixinClass...`: the
            // `featureSet.isEnabled(Feature.class_modifiers)` and
            // `!_mayIgnoreClassModifiers` conjuncts) — a pre-3.0 library's
            // plain class stays a legal mixin.
            let owner_lib = self.graph.library_of_file(owner).unwrap_or(self.lib);
            let owner_facts = self.library_facts(owner_lib);
            if !owner_facts.has_class_modifiers
                || may_ignore_class_modifiers(owner_facts, self.library_facts(self.lib))
            {
                continue;
            }
            // The oracle's message names the resolved ELEMENT (the leaf of
            // a prefixed spelling, the head of a parameterized one, the
            // folded head of an alias entry), not the whole written entry.
            let name_text = {
                let element_text = self.strings.resolve(element);
                element_text
                    .rsplit('.')
                    .next()
                    .unwrap_or(element_text)
                    .to_string()
            };
            self.emit(SemanticCode::ClassUsedAsMixin, span, &name_text, "");
        }
    }

    /// The container element a `with`-clause entry's alias spelling stands
    /// for (`typedef A = C` written as `with A` or `with p.A`): the entry's
    /// head resolves to a type alias rather than a container, so the
    /// element the upstream `mixinType.element` read sees is the aliased
    /// one — the right-hand side's named head, chased through co-alias
    /// chains (`A = B`, `B = C`) until a container resolves in the alias's
    /// OWN library scope (a foreign alias's RHS names resolve where the
    /// alias declared them). Only the RHS head matters — written arguments
    /// ride the application, never the element. Returns the container,
    /// its owning file, and the folded element spelling (the message
    /// reports this name, error_verifier.dart:2923-2926's
    /// `mixinElement.name`). `None` under-reports: a non-alias head, a
    /// function/record/`dynamic` right-hand side (the
    /// `mixin_of_non_class` face's shapes), a self-referencing or
    /// eight-hop-unresolved chain, or a right-hand head nothing resolves.
    fn mixin_use_alias_fold(&self, head: StringId) -> Option<(&'a Container, FileId, StringId)> {
        let (mut name, mut scope_lib, mut scope_file) = (head, self.lib, self.file);
        for _ in 0..8 {
            // The alias declaration `name` resolves to in this hop's scope:
            // a dot-joined spelling reads its leaf through the prefix's
            // merged table, a bare name walks the file-local then
            // import-scope chain — the same order the container arm
            // resolves.
            let (owner, decl) = if self.strings.resolve(name).contains('.') {
                let spelled = self.strings.resolve(name);
                let (prefix, leaf) = spelled.split_once('.')?;
                let (prefix, leaf) = (self.intern(prefix), self.intern(leaf));
                match self
                    .namespaces
                    .lookup_prefixed(self.graph, scope_lib, prefix, leaf)
                {
                    Lookup::Found(NamespaceEntry::Decl(owner)) => {
                        (owner, self.index.get(owner)?.decl(leaf)?)
                    }
                    _ => return None,
                }
            } else {
                match lookup_scoped(
                    self.graph,
                    self.namespaces,
                    self.index,
                    scope_lib,
                    scope_file,
                    name,
                ) {
                    ScopedHit::Local(decl) => (scope_file, decl),
                    ScopedHit::Found(NamespaceEntry::Decl(owner)) => {
                        (owner, self.index.get(owner)?.decl(name)?)
                    }
                    _ => return None,
                }
            };
            if decl.kind != MicroKind::TypeAlias || decl.var_ty == Ty::UNKNOWN {
                return None;
            }
            let Ty::Named { name: next, .. } = self.boundary.ty(owner, decl.var_ty) else {
                return None;
            };
            if next == name {
                return None;
            }
            // The RHS names resolve in the alias's declaring library: an
            // imported `typedef A = C` stands for its own scope's `C`, not
            // the use site's same-spelled import. A dot-joined hop name is
            // not a scoped spelling — the owning file's library answers.
            let next_lib = if self.strings.resolve(name).contains('.') {
                self.graph.library_of_file(owner).unwrap_or(scope_lib)
            } else {
                self.owner_library_of(owner, name).unwrap_or(scope_lib)
            };
            let resolved = if self.strings.resolve(next).contains('.') {
                resolve_joined_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    next_lib,
                    self.strings,
                    next,
                )
            } else {
                resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    next_lib,
                    owner,
                    next,
                )
            };
            if let Some((container, found)) = resolved {
                return Some((container, found, next));
            }
            (name, scope_lib, scope_file) = (next, next_lib, owner);
        }
        None
    }

    /// The end-of-unit boundedness checks (#610): the deferred explicit
    /// type-argument sites and enum values-field faces, in source order,
    /// against the whole-file snapshot; then the deferred
    /// `strict_raw_type` rows, their `{0}` display rendered from the
    /// instantiated-to-bounds form. Runs after the parse, when every
    /// in-file container is captured.
    /// `_verifyExtensionElementImplements` (scope_analyzer.dart:1312-1360):
    /// every interface `implements` face must be a supertype of the
    /// representation type (`isSubtypeOf(declaredRepresentation, type)`),
    /// else `extension_type_implements_not_supertype` at the face. The
    /// argument-bearing snapshot decides first (`int` against
    /// `Comparable<Score>` — `int`'s `Comparable` instantiation is
    /// `Comparable<num>`); a face it cannot model falls back to the
    /// name-level lattice for bare heads, which answers `None` across any
    /// unresolvable edge — the under-report direction.
    /// `resolveObjectPatternPropertyGet` (type_analyzer.dart:1669-1715):
    /// each object-pattern field reads a getter on the head type; a clean
    /// miss reports `undefined_getter` at the field's name token (the
    /// written name, or the shorthand's variable). The verdict runs on the
    /// head's resolved static type: a written alias folds to its right-hand
    /// side first while the row keeps the ALIAS spelling the oracle
    /// displays, and every interface-bearing kind answers — a mixin's `on`
    /// chain and an extension type's own members/representation both read
    /// like a class (`case M(:var q)` over a bare mixin reports "for the
    /// type 'M'", probe 3.13.7). The resolver's property consult is the
    /// ordinary member path (`type_property_resolver.dart:186-234`): a
    /// function head resolves `call` on its own signature, the extension
    /// pool answers next, and the `Object` interface closes the lookup, so
    /// `case Fn(:var hashCode)` over `typedef Fn = void Function()` is
    /// silent (probe 3.13.7 — `element: dart:core::Object::hashCode`). A
    /// NULLABLE folded head never reaches the `undefined_*` family — the
    /// `isNullable` branch resolves `Object` members and applicable
    /// extensions silently, then reports the shared unchecked code at the
    /// TYPE token (`uncheckedPropertyAccessOfNullableValue`, probe
    /// 3.13.7's `typedef A = int?` row) — a span the pending triple does
    /// not carry, so a nullable head's miss stands down rather than spell
    /// `undefined_getter` in its place. Found members of any kind (a
    /// method field reads its tear-off), `Object` members, enum synthesis,
    /// and an applicable extension face resolve; `dynamic` and record
    /// heads answer their own shapes (under-report), and an unresolvable
    /// head stays silent.
    pub(crate) fn finalize_pattern_getters(&mut self) {
        for (head, getter, at) in std::mem::take(&mut self.pending_pattern_getters) {
            // The head's static type (`ObjectPattern.type` → `NamedType` →
            // the aliased element's type): a written alias stands for its
            // right-hand side's member columns, one fold hop chain deep.
            let folded = self.fold_alias_ty(Ty::named(head));
            let (container, kind) = match folded {
                Ty::Named {
                    name,
                    nullable: false,
                    ..
                } => {
                    let Some(kind) = self.container_kind_of(name) else {
                        continue;
                    };
                    (name, kind)
                }
                // A function head's consult (the non-interface arms of the
                // property resolver): `call` resolves the receiver's own
                // signature on a non-nullable head, `Object`'s interface
                // closes every lookup, and the extension pool answers
                // between them. A NULLABLE head's miss is the oracle's
                // unchecked row at the TYPE token — outside the family
                // this row spells, so the verdict stands down.
                Ty::Function { nullable, .. } => {
                    // `call` resolves a NON-nullable receiver's own
                    // signature (type_property_resolver.dart:202-209); a
                    // nullable head reaches it only through the extension
                    // pool, where nothing declares it — the `isNullable`
                    // branch's unchecked row. `Object` members resolve on
                    // both nullabilities (the branch's first lookup hits
                    // them); an applicable extension face resolves on the
                    // head's own nullability (`on` matching inside
                    // `function_extension_face`).
                    // The consult order splits on nullability upstream:
                    // `call`'s signature arm, then the `isNullable` branch
                    // hits `Object` FIRST while the non-nullable path asks
                    // the extension pool before `Object`
                    // (type_property_resolver.dart:103-110 vs :227-232) —
                    // an extension member of an `Object` name binds its
                    // own element only on a non-nullable head.
                    if getter == self.intern("call") && !nullable {
                        self.read_names.insert(getter);
                        continue;
                    }
                    if nullable {
                        let object_hit = getter == self.intern("==")
                            || self.members.object_members.contains(&getter);
                        let ext_hit =
                            !object_hit && self.function_extension_face(folded, getter).is_some();
                        if object_hit || ext_hit {
                            self.read_names.insert(getter);
                            if ext_hit {
                                self.credit_record_extension_use(folded, getter, at.offset);
                            }
                        }
                        // A nullable head's genuine miss is the oracle's
                        // unchecked row at the TYPE token
                        // (type_property_resolver.dart:103-185 —
                        // `isNullable` precedes the `undefined_*` family)
                        // — a span the pending triple does not carry, so
                        // the verdict stands down rather than spell
                        // `undefined_getter` in its place.
                        continue;
                    }
                    if self.function_extension_face(folded, getter).is_some() {
                        self.read_names.insert(getter);
                        self.credit_record_extension_use(folded, getter, at.offset);
                        continue;
                    }
                    if getter == self.intern("==") || self.members.object_members.contains(&getter)
                    {
                        self.read_names.insert(getter);
                        continue;
                    }
                    let name_text = self.strings.resolve(getter).to_string();
                    let type_text = self.strings.resolve(head).to_string();
                    self.stage_typed(SemanticCode::UndefinedGetter, at, &name_text, &type_text);
                    continue;
                }
                // `dynamic` matches every shape; a record's columns are its
                // own shape (no member table at name level); a nullable
                // folded head (`typedef A = int?`, `typedef B = C?`)
                // reports the unchecked family upstream — never
                // `undefined_getter` — so the verdict stands down. All
                // keep the established silence.
                _ => continue,
            };
            let resolved = !matches!(self.walk_members(container, getter), Walk::Missing)
                || getter == self.intern("==")
                || self.members.object_members.contains(&getter)
                || (kind == ContainerKind::Enum
                    && self.members.enum_synth_members.contains(&getter))
                || !matches!(
                    self.extension_application(head, Ty::UNKNOWN, getter, false, false),
                    ExtensionApp::None
                );
            if resolved {
                // `visitPatternField` (unused_local_elements_verifier.dart:
                // 673-678): a resolved object-pattern field read joins the
                // oracle's `members` ∪ `readMembers` — the getter's element
                // is used AND read. Credit both faces at name level here;
                // the element-level `member_read_owners` gets its handle
                // from the head container (the same owner `member_found`
                // resolved) — the FOLDED container, where the member lives.
                self.read_names.insert(getter);
                self.member_read_owners
                    .entry(getter)
                    .or_default()
                    .insert((self.file, container));
                continue;
            }
            let name_text = self.strings.resolve(getter).to_string();
            let type_text = self.strings.resolve(head).to_string();
            self.stage_typed(SemanticCode::UndefinedGetter, at, &name_text, &type_text);
        }
    }

    pub(crate) fn finalize_ext_faces(&mut self) {
        let strings = self.strings;
        let elements = self.index.get(self.file);
        let declared = |name: StringId| elements.is_some_and(|f| f.container(name).is_some());
        for row in self.bounds.ext_face_rows(strings, declared) {
            let (face_text, rep_text) = match (row.verdict, row.displays) {
                (Some(true), _) => continue,
                (Some(false), Some(displays)) => displays,
                _ => {
                    let (Some(face), Some(rep)) = (row.face_bare, row.rep_bare) else {
                        continue;
                    };
                    if self.name_world().decide_subtype(rep, face) != Some(false) {
                        continue;
                    }
                    (
                        strings.resolve(face).to_string(),
                        strings.resolve(rep).to_string(),
                    )
                }
            };
            self.stage_typed(
                SemanticCode::ExtensionTypeImplementsNotSupertype,
                row.range,
                &face_text,
                &rep_text,
            );
        }
    }

    /// One `type_argument_not_matching_bounds` row with its context rows
    /// (`buildContextMessages`, type_arguments_verifier.dart:417-435): the
    /// raw-type instantiation row first when the checked annotation spelled
    /// no arguments, then the inverted-type row when the super-bounded pass
    /// ran — both anchored at the checked `NamedType`'s own span.
    fn emit_bounded_issue(&mut self, issue: crate::bounds::BoundedIssue) {
        // The main row anchors the offending argument's own extent: a
        // bare `Function` head grows through its formals group (#3571 —
        // the whole written TypeAnnotation the oracle's
        // `_typeArgumentErrorNode` anchors); every other captured face
        // already spans whole. The offset never moves, so the batch's
        // pre-emit offset sort stays honest.
        let range = self
            .written_function_literal_span(issue.range)
            .unwrap_or(issue.range);
        let mut context = Vec::new();
        if let Some(raw) = &issue.raw_instantiated {
            context.push(ContextMessage {
                message: format!(
                    "The raw type was instantiated as '{raw}', and is not regular-bounded."
                )
                .into(),
                file: self.file,
                range: issue.site,
            });
        }
        if let Some(inverted) = &issue.inverted_type {
            context.push(ContextMessage {
                message: format!(
                    "The inverted type '{inverted}' is also not regular-bounded, \
                     so the type is not well-bounded."
                )
                .into(),
                file: self.file,
                range: issue.site,
            });
        }
        if context.is_empty() {
            self.emit3(
                SemanticCode::TypeArgumentNotMatchingBounds,
                range,
                &issue.argument,
                &issue.parameter,
                &issue.bound,
            );
        } else {
            self.emit3_with_context(
                SemanticCode::TypeArgumentNotMatchingBounds,
                range,
                &issue.argument,
                &issue.parameter,
                &issue.bound,
                context,
            );
        }
    }

    pub(crate) fn finalize_bounds(&mut self) {
        let strings = self.strings;
        // The annotation faces share one document-ordered stream — the
        // oracle's `checkNamedType` runs at every `NamedType` during the
        // resolution walk, so a bound's nested argument reports ahead of a
        // later-spelled alias-RHS argument regardless of which capture owns
        // the tree. The three enumerations feed one offset-sorted batch:
        // - the clause-site face (`checkNamedType` at every supertype
        //   annotation; `_shouldAllowSuperBoundedTypes` denies the super
        //   pass on the clause contexts, so a violation reports plain —
        //   plus the raw-type context row a bare clause head carries,
        //   type_arguments_verifier.dart:441-446);
        // - the alias-RHS face (#2936): a type alias's named right-hand
        //   side is a `NamedType` under a `GenericTypeAlias` parent — the
        //   deny list's same arm — so its arguments answer the regular
        //   verdict with no super pass; a function-shaped RHS's own named
        //   types keep the pass (their parents are the function type's
        //   nodes);
        // - the bound-annotation face (#2908): the bound trees parse
        //   inside the declaring `<...>` window, which claims them before
        //   the use-site capture can open; `TypeParameter` is not in the
        //   deny list, so the super pass runs and the context row rides
        //   conditionally like the site arm.
        let mut issues = self.bounds.check_clause_sites(strings);
        issues.extend(self.bounds.check_alias_rhs_sites(strings));
        issues.extend(self.bounds.check_bound_annotation_sites(strings));
        issues.sort_by_key(|issue| issue.range.offset);
        for issue in issues {
            self.emit_bounded_issue(issue);
        }
        // The #3341 alias self-reference face: the oracle reports at the
        // typedef's name token, one row per self-referencing declaration
        // (`_checkForTypeAliasCannotReferenceItself`, error_verifier.dart:
        // 7116-7127), so a co-alias pair reports on both declarations.
        let self_referencing = self.bounds.check_alias_self_references();
        if !self_referencing.is_empty() {
            let mut rows: Vec<SourceRange> = self
                .decl_sites
                .iter()
                .filter(|site| {
                    matches!(site.kind, DeclKind::TypeAlias)
                        && self_referencing.contains(&site.name)
                })
                .map(|site| site.range)
                .collect();
            rows.sort_by_key(|range| range.offset);
            for range in rows {
                self.emit(SemanticCode::TypeAliasCannotReferenceItself, range, "", "");
            }
        }
        for check in std::mem::take(&mut self.pending_bounded_checks) {
            match check {
                PendingBoundedCheck::Site {
                    head,
                    args,
                    in_scope,
                    site,
                } => {
                    let issues = self
                        .bounds
                        .check_site(strings, head, &args, &in_scope, site);
                    for issue in issues {
                        self.emit_bounded_issue(issue);
                    }
                }
                PendingBoundedCheck::Enum { name, range } => {
                    if self.bounds.check_enum_values(strings, name) {
                        let code = SemanticCode::EnumInstantiatedToBoundsIsNotWellBounded;
                        self.report.diagnostics.push(Diagnostic::new(
                            code.severity(),
                            code.code_info(),
                            code.message("", ""),
                            self.file,
                            range,
                            code.correction_message(""),
                        ));
                    }
                }
            }
        }
        for (name, range) in std::mem::take(&mut self.raw_type_sites) {
            let display = self
                .bounds
                .raw_display(strings, name)
                .unwrap_or_else(|| self.strings.resolve(name).to_string());
            self.stage_usage(SemanticCode::StrictRawType, range, &display);
        }
    }

    /// The member-access spellings that resolved to a real element (the
    /// oracle's `usedElements.members`/`elements` face at name granularity).
    /// `used_names` credits every member access unconditionally — resolved
    /// and unresolved alike (`resolve_member_row`'s spelling insert) — so a
    /// `receiver._m` whose receiver the model cannot type (`Receiver::
    /// Unknown`) still spares the member. The oracle's element sets join
    /// only on a resolved access: an `InvalidType` receiver resolves no
    /// member, so the declaration stays unused.
    ///
    /// `member_reads` holds every `receiver.member` access with its
    /// parse-time receiver (read, invoke, write, operator — one record per
    /// merged selector, surviving pass 2's stack take). Re-deciding each
    /// receiver's member walk reproduces the resolve-time verdict read-only:
    /// a `Typed`/`This`/`Super`/`Container`/`Created`/`PrefixedContainer`
    /// receiver walks its container's member face (`member_found`), an
    /// `Unknown`/`Prefix` receiver binds nothing in this model. Resolved
    /// implicit extension applications ride `extension_member_used` already
    /// (`extension_application` credited them at resolve); resolved reads
    /// ride `member_read_owners`. The union is the oracle's resolved-member
    /// pool — minus the promotions the flow layer refines a `Typed` receiver
    /// into, applied here exactly as `resolve_member_row` applied them.
    fn resolved_member_uses(&self) -> FxHashSet<StringId> {
        let mut resolved: FxHashSet<StringId> = FxHashSet::default();
        resolved.extend(
            self.extension_member_used
                .iter()
                .map(|&(_, _, spelling)| spelling),
        );
        // The inference-bound receiver gate's suppressed spellings are
        // resolved uses in the oracle's eyes — credit them the same or the
        // member's declaration flips to `unused_element`.
        resolved.extend(self.inferred_recv_used_members.iter().copied());
        resolved.extend(self.member_read_owners.keys().copied());
        // `member_refs` records every resolved `.member` access — reads,
        // invokes, and writes alike. Their target spellings are resolved
        // member-only uses: `member_reads` alone misses non-read intents
        // (an `Invoke` never enters the field-read index).
        resolved.extend(self.report.member_refs.iter().map(|r| r.target_name));
        for read in &self.member_reads {
            let recv = self.promoted_receiver(read.offset, read.recv);
            let found = match recv {
                // `This`/`Typed`/`Created`/`Container` receivers all name
                // the container the member walk keys on — `member_found`
                // resolves the spelling against this file's index (the same
                // entry the resolve-time `check_instance_member`/
                // `check_container_member` walked, `Receiver::Typed`'s
                // `container = type_name` arm). `this` and a folded creation
                // carry the instance face; a bare `C`/`E`/`Ext` name the
                // container's static face — the `members` map holds both.
                Receiver::This { container, .. }
                | Receiver::Typed {
                    type_name: container,
                    ..
                }
                | Receiver::Created {
                    name: container, ..
                }
                | Receiver::Container {
                    name: container, ..
                } => {
                    self.member_found(container, read.name, false)
                        || self.ctor_found(container, read.name)
                        // A head that resolves to no type in this scope (a
                        // foreign class's unsubstituted parameter —
                        // `WeakReference<C>.target` read as `T?`) names no
                        // container the walk can probe; the oracle's
                        // substituted receiver resolves the member, so the
                        // access stays resolved like an `Unknown` receiver.
                        || {
                            let foreign = !self.is_scope_param(container)
                                && self.type_resolution(container).0 == TypeResolution::Missing;
                            if foreign {}
                            foreign
                        }
                }
                Receiver::Super { container, .. } => self.member_found(container, read.name, true),
                // The prefix-resolved container (`io.Directory.gone`): the
                // name lives behind the prefix table, so the bare-scope
                // `member_found` cannot reach it — resolve through
                // `prefixed_container` first, then probe its member face.
                Receiver::PrefixedContainer { prefix, name, .. } => prefixed_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    prefix,
                    name,
                )
                .is_some_and(|(container, _)| {
                    container.members.contains_key(&read.name)
                        || container.constructors.contains(&read.name)
                }),
                // An untyped receiver (`var m = …; m.x`) or a prefix
                // namespace access carries no container the model can walk —
                // the oracle's full inference may still resolve the member,
                // so the access keeps the under-report (the suppression the
                // raw `used_names` spelling already provided). Only a
                // resolvable receiver's clean member miss poisons a
                // declaration; these stay resolved.
                Receiver::Prefix { .. } | Receiver::Unknown => true,
            };
            if found {
                resolved.insert(read.name);
            }
        }
        resolved
    }

    /// The member spellings an extension import can be credited by: the
    /// oracle credits an import only when the resolver applied one of
    /// its extensions (`notifyExtensionUsed` after
    /// `applicable_extensions.dart`' `applicableTo` accepts the
    /// receiver's static type), and an invalid-typed receiver — the
    /// unresolved `Source` behind a broken re-export — applies nothing.
    /// The name-level model cannot re-run applicability at the directive
    /// gate, so a deny needs PROOF the receiver was invalid: a typed
    /// head that resolves nowhere in this unit and in no import target's
    /// export scope, or an `Unknown` receiver whose local the
    /// closure-return probe marked. Everything else keeps the credit
    /// (the oracle's receiver may be a perfectly valid type this model
    /// never saw). A spelling no member access carries at all
    /// (bare/type uses) keeps the raw posture.
    fn extension_credit_names(&self, used: &[StringId]) -> Vec<StringId> {
        let mut accessed: FxHashSet<StringId> = FxHashSet::default();
        let mut demonstrating: FxHashSet<StringId> = FxHashSet::default();
        // The reachability fallback's edge set: a typed head that resolves
        // nowhere in this unit still names a REAL type when an import
        // target exports it (the oracle resolves applicability where the
        // type was written; a `show`-hidden spelling is inaccessible, not
        // invalid).
        let edges = self.graph.import_edges(self.lib);
        for read in &self.member_reads {
            accessed.insert(read.name);
            let recv = self.promoted_receiver(read.offset, read.recv);
            let demonstrates = match recv {
                Receiver::Unknown => !self.read_roots_at_invalid_local(read.offset),
                Receiver::Typed { type_name, .. } => {
                    // The merge-time witness, not a finalization re-ask:
                    // the scope stack is gone here, so a method type
                    // parameter receiver (`DB extends …`) would read as
                    // invalid and deny a credit the oracle's applicability
                    // granted (the extension's own `on DB` face).
                    !self.invalid_typed_member_rows.contains(&read.offset)
                        || edges.iter().any(|edge| {
                            edge.target
                                .is_some_and(|t| self.graph.exported(t, type_name).is_some())
                        })
                }
                _ => true,
            };
            if demonstrates {
                demonstrating.insert(read.name);
            }
        }
        // Standing `Unknown`-receiver rows deny the same provable way —
        // an unproven access keeps the credit (the receiver the oracle
        // typed may be perfectly valid; only the closure probe's marked
        // locals are known `InvalidType`).
        for &(offset, name) in &self.unresolved_member_rows {
            accessed.insert(name);
            if !self.read_roots_at_invalid_local(offset) {
                demonstrating.insert(name);
            }
        }
        used.iter()
            .copied()
            .filter(|&name| demonstrating.contains(&name) || !accessed.contains(&name))
            .collect()
    }

    /// Whether the bare identifier a `.`-selection's receiver ends with
    /// names a local the closure-return probe marked (`InvalidType`
    /// static type in the oracle): the extension-import credit's deny
    /// proof for an `Unknown` receiver (`var source = run('…', (…) { …
    /// return forUri(uri); … }); source.tryCast(…)` — the inferred
    /// receiver is invalid, no extension ever applies).
    fn read_roots_at_invalid_local(&self, member_offset: u32) -> bool {
        let Some((start, end)) = self.ident_span_before_dot(member_offset) else {
            return false;
        };
        let span = &self.source.as_bytes()[self.byte_of(start)..self.byte_of(end)];
        !span.is_empty()
            && self
                .invalid_typed_locals
                .contains(&self.intern(std::str::from_utf8(span).unwrap_or("")))
    }

    /// Whether the byte before the `.` that precedes `offset` is a `?` —
    /// the `?.`/`?..` token faces the shared dot-shorthand probe's
    /// catch-all cannot separate from a leading-dot shorthand (both leave
    /// a `.`-then-selector byte pair; only the token kind does, and the
    /// byte walk sees neither). A null-aware member/cascade selector
    /// (`x?._()`) is a receiver access, never the fold's headless shape.
    fn byte_before_dot_is_question(&self, offset: u32) -> bool {
        let bytes = self.source.as_bytes();
        let mut i = self.byte_of(offset);
        while i > 0 && bytes[i - 1].is_ascii_whitespace() {
            i -= 1;
        }
        if i == 0 || bytes[i - 1] != b'.' {
            return false;
        }
        i -= 1;
        while i > 0 && bytes[i - 1].is_ascii_whitespace() {
            i -= 1;
        }
        i > 0 && bytes[i - 1] == b'?'
    }

    /// The creation/metadata selector credit (`const C._x()`,
    /// `new C._x()`, `@C._x`): the parser routes a constructor
    /// reference's selector through the creation-head events instead of a
    /// member-access row (`listener.rs`'s `C::ConstructorReference`
    /// family), so `used_names` never learns the spelling and the
    /// element-use pool reports the constructor as unused. The oracle
    /// resolves the creation's constructor element, so the credit is
    /// element-accurate: the head must resolve to a container that
    /// declares the selector as a constructor. An implicit-const or plain
    /// `C._x()` invocation rides the member path and needs no credit; an
    /// unresolvable head (a prefix segment, a dead name) credits nothing —
    /// the under-report direction every unmodelable corner keeps.
    pub(crate) fn credit_constructor_selector(&mut self, head: StringId, selector: StringId) {
        let Some((container, owner)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            head,
        ) else {
            return;
        };
        // The `new` selector spells the unnamed face
        // (`ctor_face_exists`'s same fold): `C.new` resolves the unnamed
        // constructor's element upstream, so the credit joins under the
        // container spelling the declaration site and the
        // `member_used_elements` tuple already key on. A genuinely named
        // `sel` keeps its own spelling.
        let named = container.constructors.contains(&selector);
        let unnamed = selector == self.intern("new") && container.has_unnamed_constructor;
        if !(named || unnamed) {
            return;
        }
        let target = if named { selector } else { head };
        self.used_names.insert(target);
        // The element-level edge (#2618's `member_refs` face): a
        // resolved `C.sel` creation credits the exact constructor
        // element — `D.sel` never shares the credit. The
        // `member_used_elements` set in `finalize_usage` reads it.
        // `report_optional_parameters`'s non-call arm does NOT read this
        // edge: `end_constructor_reference` fires for `new C.sel(` /
        // `const C.sel(` call sites too, where only the supplied
        // arguments credit — the zero-length range cannot distinguish
        // call from tear-off, so the token-walked `noncall_reads` set
        // (tear-offs + `this.sel` redirects) owns that arm instead.
        self.report.member_refs.push(MemberRef {
            range: SourceRange {
                offset: 0,
                length: 0,
            },
            kind: MemberRefKind::Invoked,
            target_file: owner,
            target_name: target,
            target_container: head,
        });
    }

    /// The constructor half of a resolved `C.sel` access (`_E._`,
    /// `C.named`): the member walk keys on `members`, while constructors
    /// live in the container's own `constructors` list (the official
    /// `allowConstructors` cut — never inherited). `new` names the unnamed
    /// head, flagged by `has_unnamed_constructor`.
    fn ctor_found(&self, container: StringId, member: StringId) -> bool {
        let Some((found, _)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        ) else {
            return false;
        };
        found.constructors.contains(&member)
            || (self.strings.resolve(member) == "new" && found.has_unnamed_constructor)
    }

    /// The extension-element uses the resolve-side credit never records
    /// (#2465), re-attributed onto the element key the consume arms check
    /// (`extension_member_used`). Two derivations:
    ///
    /// The SELF-USE arm: inside an extension body a bare reference or an
    /// explicit `this.m` access resolves against the extension's OWN member
    /// scope first (the enclosing container's member walk answers before
    /// the `on` face — the oracle's implicit-`this` lookup), so the
    /// resolved row names the extension as its owner
    /// (`RefTarget::Member`/`Receiver::This` carry the extension). The
    /// oracle's element model records that use on the member element
    /// (`usedElements.members`), but the application credit only runs on
    /// member ACCESSES, never on the body's own scope resolutions — the
    /// census's implicit-`this` shapes (`_nameTokens`, `_writeElement`)
    /// over-reported. The container spelling names the face; the empty
    /// spelling is the unnamed faces' shared key, so every same-file
    /// unnamed face declaring the member takes the credit (the
    /// sibling-same-spelling corner stays unmodelable at the recorded
    /// granularity).
    ///
    /// The UNIVERSAL-`on`-TYPE arm: a face whose `on` type's head is the
    /// face's own type parameter (`extension<T> on T` / `on T?`) applies to
    /// every valid receiver in the oracle — the application instantiates
    /// the parameter to the receiver's type — but the name-level
    /// reachability probe can never walk into a type parameter, so
    /// `extension_application` answered no candidates and the resolve-side
    /// credit never fired (the `visitWithOverride`/`mapOrNull` rows). For a
    /// member access whose recomputed instance-member walk missed (a real
    /// member beats every extension) on a resolvable receiver, the face
    /// whose member spelling is UNIQUE among the accessible faces is the
    /// resolution the oracle reached; a second declarer would be the
    /// ambiguous application that credits nothing and leaves every
    /// candidate reporting.
    /// The plain identifier a `.`-selection's receiver ends with (`x.m`
    /// at `m`'s offset answers `x`'s span; `a.b.m` answers `b`'s):
    /// `None` when the receiver is not a bare identifier link (a call or
    /// index result closes with `)`/`]`, not the `.` the backscan
    /// requires). Offsets are UTF-16 like every token range; the
    /// identifier charset is ASCII, so the unit arithmetic is exact.
    fn ident_span_before_dot(&self, offset: u32) -> Option<(u32, u32)> {
        let ident_char = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$';
        let mut at = offset;
        while at > 0 && matches!(self.byte_at(at - 1), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            at -= 1;
        }
        if at == 0 || self.byte_at(at - 1) != Some(b'.') {
            return None;
        }
        at -= 1;
        let end = at;
        while at > 0 && self.byte_at(at - 1).is_some_and(ident_char) {
            at -= 1;
        }
        (at != end).then_some((at, end))
    }

    /// Whether the bytes after a declarator's name spell `= <…>(…` — the
    /// local's initializer is a CALL expression (a plain `= ident;` or a
    /// literal initializer never matches). The untyped-call-local premise
    /// keys on it: only a send's return can be inferred by the oracle
    /// where the walk's tables cannot.
    fn decl_initializer_is_call(&self, name_end: u32) -> bool {
        let mut at = name_end;
        while matches!(self.byte_at(at), Some(b' ' | b'\t')) {
            at += 1;
        }
        if self.byte_at(at) != Some(b'=') || self.byte_at(at + 1) == Some(b'=') {
            return false;
        }
        at += 1;
        let limit = at + 300;
        while at < limit {
            match self.byte_at(at) {
                Some(b'(') => return true,
                Some(b';') | Some(b'\n') => return false,
                _ => at += 1,
            }
        }
        false
    }

    /// The enclosing top-level extension's `on` head, read backward from
    /// the reference's source position: `Some(head)` when the reference
    /// sits inside an extension body (the `on` type's spelling, stripped
    /// of generic arguments and nullability), `None` for a class/mixin/
    /// enum body or top level. A sibling face on another `on` type never
    /// resolves an implicit-`this` read inside the body.
    fn enclosing_extension_on(&self, offset: u32) -> Option<String> {
        let bytes = self.source.as_bytes();
        let mut end = self.byte_of(offset).min(bytes.len());
        loop {
            let start = bytes[..end]
                .iter()
                .rposition(|b| *b == b'\n')
                .map_or(0, |p| p + 1);
            let line = &self.source[start..end];
            // A top-level declaration header starts in column 0.
            if !line.is_empty() && !line.starts_with([' ', '\t', '}', '/', '*', ')']) {
                let words: Vec<&str> = line.split_whitespace().collect();
                if words.contains(&"extension") {
                    let head = words
                        .iter()
                        .position(|w| *w == "on")
                        .and_then(|at| words.get(at + 1))
                        .map(|w| {
                            w.trim_end_matches(|c: char| {
                                !(c.is_alphanumeric() || c == '_' || c == '$')
                            })
                        })
                        .map(|w| w.split(['<', '?']).next().unwrap_or(w).to_string());
                    return Some(head.unwrap_or_default());
                }
                if words
                    .iter()
                    .any(|w| matches!(*w, "class" | "mixin" | "enum"))
                {
                    return None;
                }
            }
            if start == 0 {
                return None;
            }
            end = start - 1;
        }
    }

    fn credit_derived_extension_uses(&mut self) {
        let Some(elements) = self.index.get(self.file) else {
            return;
        };
        let faces = elements.extension_faces();
        if faces.is_empty() {
            return;
        }
        // The index-read pool (`[]` alone): upstream
        // `IndexExpression.writeOrReadElement` (extensions.dart:244-246)
        // resolves `[]=` for EVERY assignment form (`=`, `??=`, `+=`,
        // `++`/`--`), so a write-only unit's `[]` declaration is still
        // `unused_element` — but the resolve side's `extension_application`
        // records the `[]` READ spelling beside `[]=` for every write row
        // (`member` + `setter` pair at row_resolution's applied-face
        // credit), crediting a member the oracle never used. The read-only
        // set re-derives the `[]` verdict from the pass's own read-position
        // credits: a `(face_file, face_index)` lands here only where a
        // genuine `[]` READ applied — a pure `x[i]`/`L[i]` access, never a
        // compound's write half. At the pass's end the pool is scrubbed:
        // a `(file, index, "[]")` entry paired with `[]=` but missing here
        // is the write-artifact.
        let index_get = self.intern("[]");
        let index_set = self.intern("[]=");
        let mut index_read_used: FxHashSet<(FileId, u32)> = FxHashSet::default();
        // Self-use arm: the same-file faces keyed by the container spelling
        // the resolution rows record.
        let empty = self.intern("");
        let mut face_keys: FxHashMap<StringId, Vec<u32>> = FxHashMap::default();
        for (index, face) in faces.iter().enumerate() {
            if let Ok(index) = u32::try_from(index) {
                face_keys
                    .entry(face.name.unwrap_or(empty))
                    .or_default()
                    .push(index);
            }
        }
        let mut derived: FxHashSet<(FileId, u32, StringId)> = FxHashSet::default();
        for unit_ref in &self.refs {
            if let RefTarget::Member { container } = unit_ref.target {
                self.note_extension_self_use(&face_keys, &mut derived, container, unit_ref.name);
            }
        }
        for read in &self.member_reads {
            if let Receiver::This { container, .. } = read.recv {
                self.note_extension_self_use(&face_keys, &mut derived, container, read.name);
            }
        }
        // The IMPLICIT-THIS / TYPE-PARAMETER-RECEIVER arm: a bare
        // reference the scope walk answered through the extension pool
        // (`RefTarget::Opaque` — `_kind(1)` inside a subclass of the
        // face's `on` type) and a member read whose receiver is a type
        // parameter (`e.eff` over `E extends EI` — the bound's extension
        // face resolves it) both resolve in the oracle, which records the
        // use on the member element. A spelling declared by exactly one
        // same-file face is that resolution; a second declarer is the
        // ambiguity the oracle leaves unresolved.
        let type_param_heads: FxHashSet<StringId> = elements
            .containers()
            .flat_map(|(_, c)| c.type_param_names.iter().copied())
            .chain(
                faces
                    .iter()
                    .flat_map(|f| f.type_param_names.iter().copied()),
            )
            .chain(
                self.exec_bounds
                    .values()
                    .chain(self.method_exec_bounds.values())
                    .flat_map(|w| w.iter().map(|(p, _)| *p)),
            )
            .collect();
        let unique_face = |name: StringId| -> Option<u32> {
            let mut hit = None;
            for (index, face) in faces.iter().enumerate() {
                if face.members.contains_key(&name) {
                    if hit.is_some() {
                        return None;
                    }
                    hit = u32::try_from(index).ok();
                }
            }
            hit
        };
        // The enclosing top-level declaration's keyword, read backward from
        // the reference: an implicit-`this` read inside an EXTENSION body
        // looks up on that extension's own `on` type, never a sibling
        // face — only class/mixin/enum bodies (a subclass of the face's
        // `on` type) carry the pool resolution.
        let in_extension_body = |offset: u32, index: u32| -> bool {
            let Some(head) = self.enclosing_extension_on(offset) else {
                return false;
            };
            let face_head = faces
                .get(index as usize)
                .and_then(|face| face.on_type)
                .map(|on| self.strings.resolve(on));
            // The enclosing `on` type must be the face's own head or a
            // subtype of it (`ClassElement` reaching an `InterfaceElement`
            // face); an unresolvable head keeps the pool credit.
            let foreign = match face_head {
                Some(face_head) if face_head != head.as_str() => {
                    let sub = Ty::named(self.intern(&head));
                    let sup = Ty::named(self.intern(face_head));
                    self.type_resolution(self.intern(&head)).0 != TypeResolution::Missing
                        && !self.name_subtype(sub, sup)
                }
                _ => false,
            };

            foreign
        };
        for unit_ref in &self.refs {
            if matches!(unit_ref.target, RefTarget::Opaque)
                && let Some(index) = unique_face(unit_ref.name)
                && !in_extension_body(unit_ref.range.offset, index)
            {
                // An Opaque `x[i]` row inside an extension body is the
                // implicit-this `[]` READ — a genuine index read credit.
                if unit_ref.name == index_get {
                    index_read_used.insert((self.file, index));
                }
                derived.insert((self.file, index, unit_ref.name));
            }
        }
        for read in &self.member_reads {
            if let Receiver::Typed { type_name, .. } = read.recv
                && type_param_heads.contains(&type_name)
                && let Some(index) = unique_face(read.name)
            {
                if read.name == index_get {
                    index_read_used.insert((self.file, index));
                }
                derived.insert((self.file, index, read.name));
            }
        }
        self.extension_member_used.extend(derived);
        // The CLOSURE-FORMAL-CHAIN arm: a member row whose receiver chain
        // roots at a link the walk could not type — a closure formal the
        // callee inference never solved (`((x ?? const []).map((sub) =>
        // sub.source.meta))` leaves `.map` unresolvable) or a var-inferred
        // local from an untyped call (`final sub = ext._addListener(…)`).
        // The oracle instantiates the real callback/send return and
        // resolves the whole chain, so each link's spelling is a resolved
        // use — the same premise the inference-bound receiver gate rides.
        // The chain walks backward by bytes: the receiver's last
        // identifier is either SPELLED like a recorded closure formal
        // (`sub`), names an untyped-call local, or is itself a member row
        // this pass just credited (`source`), so one in-order sweep
        // carries the premise outward (`meta`).
        if !self.closure_formal_names.is_empty() {
            let mut premise_ends: FxHashSet<u32> = FxHashSet::default();
            let mut chain_credits: Vec<StringId> = Vec::new();
            // Pass A: the merged-read record (`member_reads`) — the reads
            // whose receiver rows folded away at merge time. Pass B: the
            // still-standing member rows with `Unknown` receivers (the
            // `sub.impl` chains the merge never recorded). One shared
            // premise set carries both.
            let mut reads: Vec<(u32, StringId)> = self
                .member_reads
                .iter()
                .filter(|read| matches!(read.recv, Receiver::Unknown))
                .map(|read| (read.offset, read.name))
                .collect();
            let mut extra: Vec<(u32, StringId)> = self
                .unresolved_member_rows
                .iter()
                .copied()
                .filter(|&(offset, _)| {
                    !self
                        .member_reads
                        .binary_search_by(|read| read.offset.cmp(&offset))
                        .is_ok()
                })
                .collect();
            reads.append(&mut extra);
            reads.sort_unstable();
            reads.dedup_by(|a, b| a.0 == b.0);
            reads.sort_unstable();
            reads.dedup_by(|a, b| a.0 == b.0);
            for (read_offset, read_name) in reads {
                let Some((start, end)) = self.ident_span_before_dot(read_offset) else {
                    continue;
                };
                let span_bytes = &self.source.as_bytes()[self.byte_of(start)..self.byte_of(end)];
                let rooted_at_formal = self
                    .closure_formal_names
                    .iter()
                    .any(|formal| self.strings.resolve(*formal).as_bytes() == span_bytes);
                // A var-inferred local whose initializer is a CALL the walk
                // could not type (`final sub = listenable._addListener(…)`
                // — the extension method's return stays outside the
                // element tables) roots the same premise: the oracle
                // infers the send's return and every member chain beyond
                // the local resolves (`sub.impl._listenedElement`).
                let rooted_at_untyped_call_local = !rooted_at_formal
                    && self.locals.iter().any(|decl| {
                        self.strings.resolve(decl.name).as_bytes() == span_bytes
                            && matches!(
                                self.frames
                                    .iter()
                                    .rev()
                                    .find_map(|frame| frame.var_types.get(&decl.name).copied()),
                                None | Some(Ty::Unknown)
                            )
                            && self.decl_initializer_is_call(decl.range.end())
                    });
                if !rooted_at_formal
                    && !rooted_at_untyped_call_local
                    && !premise_ends.contains(&end)
                {
                    continue;
                }
                chain_credits.push(read_name);
                let name_len = self.strings.resolve(read_name).len() as u32;
                premise_ends.insert(read_offset + name_len);
            }
            for name in &chain_credits {
                self.inferred_recv_used_members.insert(*name);
                if let Some(elements) = self.index.get(self.file) {
                    let credits: Vec<u32> = elements
                        .extension_faces()
                        .iter()
                        .enumerate()
                        .filter(|(_, face)| face.members.contains_key(name))
                        .filter_map(|(index, _)| u32::try_from(index).ok())
                        .collect();
                    for index in credits {
                        if *name == index_get {
                            index_read_used.insert((self.file, index));
                        }
                        self.extension_member_used.insert((self.file, index, *name));
                    }
                }
            }
        }
        // Universal-`on`-type arm: the re-adjudicated accesses. The
        // declarer count rides the whole accessible pool (own units +
        // imports — the resolve-side ambiguity set), so the guard keeps the
        // oracle's no-unique-resolution posture.
        let faces: Vec<(&ExtensionFace, FileId)> = self.accessible_extension_faces().to_vec();
        if faces.is_empty() {
            return;
        }
        let mut declarers: FxHashMap<StringId, u32> = FxHashMap::default();
        for (face, _) in &faces {
            for spelling in face.members.keys() {
                *declarers.entry(*spelling).or_default() += 1;
            }
        }
        // Index iteration over a snapshot length (the clone's exact
        // semantics): the loop body mutates the engine (the credit set,
        // the import credits), so a shared iterator would hold the borrow
        // across the mutations. The reads are `Copy` and `member_reads`
        // is never written here, so each item re-borrows for the one
        // copy-out — no whole-list clone per file.
        let read_count = self.member_reads.len();
        for read_index in 0..read_count {
            let read = self.member_reads[read_index];
            let (head, ty) = match self.promoted_receiver(read.offset, read.recv) {
                Receiver::Typed { type_name, ty, .. } => (type_name, ty),
                Receiver::Created { name, .. } | Receiver::Container { name, .. } => {
                    (name, Ty::UNKNOWN)
                }
                // `this`/`super` ride the self-use arm; a prefix namespace
                // access and an unresolvable receiver keep the element
                // posture (an unresolvable receiver resolves no member).
                _ => continue,
            };
            if self.member_found(head, read.name, false) || self.ctor_found(head, read.name) {
                continue;
            }
            let receiver_valid = !matches!(ty, Ty::Dynamic)
                && if ty == Ty::UNKNOWN {
                    self.type_resolution(head).0 == TypeResolution::Type
                } else {
                    self.extension_receiver_resolves(ty)
                };
            if !receiver_valid || declarers.get(&read.name).copied() != Some(1) {
                continue;
            }
            let Some(&(face, face_file)) = faces
                .iter()
                .find(|(face, _)| face.members.contains_key(&read.name))
            else {
                continue;
            };
            // The face must be universal: its `on` type's head is its own
            // type parameter — the one shape the reachability probe cannot
            // decide and the oracle's instantiation always admits.
            let universal = match face.on_ty {
                Ty::Named { name, .. } => face.type_param_names.contains(&name),
                _ => false,
            };
            if !universal {
                continue;
            }
            if let Some(face_index) = self
                .index
                .get(face_file)
                .and_then(|elements| {
                    elements
                        .extension_faces()
                        .iter()
                        .position(|entry| std::ptr::eq(entry, face))
                })
                .and_then(|index| u32::try_from(index).ok())
            {
                // A `member_reads` `[]` row is the resolved `x[i]` read —
                // a genuine read-position credit the write-artifact scrub
                // below honors.
                if read.name == index_get {
                    index_read_used.insert((face_file, face_index));
                }
                self.extension_member_used
                    .insert((face_file, face_index, read.name));
                self.credit_extension_import(face, face_file);
            }
        }
        // The operator / index / implicit-`call` credit faces (the #3223
        // census): the gather visitor records a member use for every
        // operator or index access over the extended type
        // (`visitBinaryExpression`/`visitIndexExpression`/
        // `visitPrefixExpression`/`visitPostfixExpression` →
        // `usedElements.addMember`, unused_local_elements_verifier.dart
        // :59-64/:196-201/:256-267) and for a value's implicit `call`
        // (`visitFunctionExpressionInvocation` :175-180). The resolve layer
        // leaves those receivers untyped, so the row pool decides here: an
        // operator row names its own operand (the row's captured type, or
        // the literal's spelling), and a callee-less `Invoke` row is the
        // invoked value. The credit stays over-report-proof: the member
        // spelling must be declared by exactly one accessible face (the
        // resolve-side ambiguity set) whose `on` head equals the
        // receiver's head, and a real container operator takes the use
        // (its own member join owns it).
        let call_spelling = self.intern("call");
        // The operator / index / implicit-`call` credit faces (the #3223
        // census): the gather visitor records a member use for every
        // operator or index access over the extended type
        // (`visitBinaryExpression`/`visitIndexExpression`/
        // `visitPrefixExpression`/`visitPostfixExpression` →
        // `usedElements.addMember`, unused_local_elements_verifier.dart
        // :59-64/:196-201/:256-267) and for a value's implicit `call`
        // (`visitFunctionExpressionInvocation` :175-180). The row pool is
        // drained by finalize time (the listener folds statements away as
        // it goes), so the source bytes decide: a literal operand's head is
        // its spelling (`true`/`false` → `bool`, a digit run → `int`) and
        // the bracket or paren that follows it is the application. The
        // credit stays over-report-proof through the same guard chain as
        // every other arm: the member spelling must be declared by exactly
        // one accessible face (the resolve-side ambiguity set) whose `on`
        // head equals the receiver's head, and a real container member
        // takes the use (`member_found`).
        let bool_id = self.intern("bool");
        let int_id = self.intern("int");
        let bytes = self.source.as_bytes();
        let ident_before = |bytes: &[u8], i: usize| {
            i > 0
                && matches!(
                    bytes[i - 1],
                    b'_' | b'$' | b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z'
                )
        };
        let mut at = 0usize;
        while at < bytes.len() {
            let mut head = None;
            let mut after = at;
            if bytes[at..].starts_with(b"true") && !ident_before(bytes, at) {
                head = Some(bool_id);
                after = at + 4;
            } else if bytes[at..].starts_with(b"false") && !ident_before(bytes, at) {
                head = Some(bool_id);
                after = at + 5;
            } else if bytes[at].is_ascii_digit() && !ident_before(bytes, at) {
                head = Some(int_id);
                after = at
                    + bytes[at..]
                        .iter()
                        .take_while(|b| b.is_ascii_digit() || **b == b'.')
                        .count();
            }
            if let Some(head) = head {
                let mut next = after;
                while next < bytes.len() && bytes[next].is_ascii_whitespace() {
                    next += 1;
                }
                match bytes.get(next) {
                    // An index application: the read credits `[]`, the
                    // write (`a[i] = v`) credits `[]=` — the token after
                    // the matching `]` decides (`=` alone; `==` compares
                    // the read and `=>` belongs to a closure body).
                    Some(b'[') => {
                        let member = if self.index_write_after(next) {
                            self.intern("[]=")
                        } else {
                            self.intern("[]")
                        };
                        self.credit_face_member(&faces, &declarers, member, head, &mut index_read_used);
                    }
                    // An implicit-`call` invocation (`0()`).
                    Some(b'(') => {
                        self.credit_face_member(&faces, &declarers, call_spelling, head, &mut index_read_used);
                    }
                    _ => {}
                }
                at = after;
            }
            at += 1;
        }
        // The function-valued and type-literal receiver heads, built once:
        // the operator arm below and the member-access arm after it share
        // them.
        let mut fn_heads: FxHashSet<StringId> = self
            .decl_sites
            .iter()
            .filter(|s| matches!(s.kind, DeclKind::TopLevelFn))
            .map(|s| s.name)
            .collect();
        let mut type_heads: FxHashSet<StringId> = self
            .decl_sites
            .iter()
            .filter(|s| matches!(s.kind, DeclKind::TypeAlias | DeclKind::Container))
            .map(|s| s.name)
            .collect();
        for frame in &self.frames {
            fn_heads.extend(frame.local_fn_sigs.keys().copied());
            for (name, ty) in &frame.var_types {
                // A local or parameter of the same spelling shadows any
                // same-spelled declaration: it is the value the receiver
                // expression names, never the type or the function.
                fn_heads.remove(name);
                type_heads.remove(name);
                if matches!(ty, Ty::Function { .. }) {
                    fn_heads.insert(*name);
                }
            }
        }
        // The operator-receiver arm (the #3289 census): the gather visitor
        // records a member use for every operator access over the extended
        // type (`visitBinaryExpression`/`visitPrefixExpression`/
        // `visitPostfixExpression` → `usedElements.addMember`,
        // unused_local_elements_verifier.dart:59-64/:196-201/:256-267),
        // but the resolve layer leaves an identifier or literal operand
        // untyped, so the source bytes and the declaration tables decide
        // the receiver's head: a string literal is a `String`, a
        // function-valued identifier is a `Function`, and a type name used
        // as a value is a `Type`. The operator token must follow whitespace
        // (a `<` glued to an identifier is a generic instantiation or the
        // `operator<` declaration head, never a binary operator), and
        // `credit_face_member`'s guard chain (exactly one accessible face
        // declares the spelling, the receiver head has no native member of
        // that spelling, the face's `on` head matches) keeps the credit
        // over-report-proof.
        let string_head = self.intern("String");
        let function_head = self.intern("Function");
        let type_head = self.intern("Type");
        let mut scan = 0usize;
        while scan < bytes.len() {
            let byte = bytes[scan];
            // Comments never carry a use: skip a line comment to its
            // newline and a (nest-aware) block comment to its close so a
            // doc example like `a < b` credits nothing.
            if byte == b'/' && bytes.get(scan + 1) == Some(&b'/') {
                scan += 2;
                while scan < bytes.len() && bytes[scan] != b'\n' {
                    scan += 1;
                }
                continue;
            }
            if byte == b'/' && bytes.get(scan + 1) == Some(&b'*') {
                scan += 2;
                let mut depth = 1usize;
                while scan < bytes.len() && depth > 0 {
                    if bytes[scan] == b'/' && bytes.get(scan + 1) == Some(&b'*') {
                        depth += 1;
                        scan += 2;
                    } else if bytes[scan] == b'*' && bytes.get(scan + 1) == Some(&b'/') {
                        depth -= 1;
                        scan += 2;
                    } else {
                        scan += 1;
                    }
                }
                continue;
            }
            if byte == b'\'' || byte == b'"' {
                // A string literal receiver: skip to the closing quote
                // (backslash escapes consumed) and credit a following
                // operator.
                let mut end = scan + 1;
                while end < bytes.len() && bytes[end] != byte {
                    if bytes[end] == b'\\' {
                        end += 1;
                    }
                    end += 1;
                }
                if let Some(op) = self.binary_operator_after(end.saturating_add(1)) {
                    self.credit_face_member(&faces, &declarers, op, string_head, &mut index_read_used);
                }
                scan = end.saturating_add(1);
                continue;
            }
            if byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$' {
                let start = scan;
                let mut end = scan + 1;
                while end < bytes.len() {
                    let next = bytes[end];
                    if next.is_ascii_alphanumeric() || next == b'_' || next == b'$' {
                        end += 1;
                    } else {
                        break;
                    }
                }
                let mut cursor = end;
                let mut spaced = false;
                while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                    cursor += 1;
                    spaced = true;
                }
                if spaced {
                    if let Some(op) = self.binary_operator_at(cursor) {
                        let name = self.intern(&self.source[start..end]);
                        let head = if fn_heads.contains(&name) {
                            Some(function_head)
                        } else if type_heads.contains(&name) {
                            Some(type_head)
                        } else {
                            None
                        };
                        if let Some(head) = head {
                            self.credit_face_member(&faces, &declarers, op, head, &mut index_read_used);
                        }
                    }
                }
                scan = end;
                continue;
            }
            scan += 1;
        }
        // The function-receiver member-access arm: an `extension on
        // Function` face applies to any function-valued receiver — a bare
        // or generic-instantiated function reference (`foo.m<int>`,
        // `foo<int>.m()`; the `ExtensionElement` arm of
        // `visitSimpleIdentifier`, unused_local_elements_verifier.dart
        // :341-348, credits `usedElements.members`). The resolve layer
        // cannot type the instantiation, so the receiver's head identifier
        // decides: a head that binds a top-level function, a local
        // function, or a function-typed formal/local, with the source
        // adjacency `head.member` (an optional balanced `<...>`
        // instantiation may sit between) as the application.
        for &(face, _) in &faces {
            if !matches!(&face.on_ty, Ty::Named { name, .. }
                if self.strings.resolve(*name) == "Function")
            {
                continue;
            }
            for member in face.members.keys() {
                if declarers.get(member).copied() != Some(1) {
                    continue;
                }
                let member_text = self.strings.resolve(*member);
                let applied = fn_heads.iter().any(|&head| {
                    !self.member_found(head, *member, false)
                        && self.source_member_access(self.strings.resolve(head), &member_text)
                });
                if applied {
                    self.credit_face_member(&faces, &declarers, *member, self.intern("Function"), &mut index_read_used);
                }
            }
        }
        // The read-record-driven arm (#3477): the byte-scan arms above
        // answer the receiver's head only for the literal/`Function`/`Type`
        // shapes — every other resolved application never joins
        // `extension_member_used`, so the unnamed/private face loop
        // reported the member as unused. The row stack is drained by
        // finalize (`resolve_rows` takes it), so the re-derivation reads
        // the surviving records instead: `refs` for the bare operands
        // (`f += 1` over `extension on void Function<T>(T)`, `a + 0` over
        // `on (String,)`, `~"hello"` over `on String`, `a()`'s `call`
        // sugar) and `member_reads` for the member-target writes
        // (`x.p ??= v`, `x.f += 1`, `x.f++`). The intent the drained row
        // carried re-derives from the ADJACENT token — a `??=`/compound/
        // `++`/`--` suffix or a `~`/`-`/`++`/`--` prefix — and the
        // operand's `Ty` from the read-type tables, the flow-blind
        // initializer derivations, and the member's declared face. The
        // same ambiguity guard every other arm runs gates the credit:
        // exactly one accessible face declares the spelling and the face's
        // `on` shape admits the receiver.
        let read_count2 = self.member_reads.len();
        for read_index in 0..read_count2 {
            let read = self.member_reads[read_index];
            let promoted = self.promoted_receiver(read.offset, read.recv);
            let receiver = match promoted {
                Receiver::Typed { ty, .. } => Some(ty),
                Receiver::This { container, kind } => {
                    // An UNNAMED extension's `this` face resolves through the
                    // frame latch, and no frame is live at finalize: the
                    // lookup falls back to the container spelling itself
                    // (never the `on` head), so `this.p ??= v` inside
                    // `extension on String` credits nothing and the accessor
                    // pair reports. The header scan re-derives the `on` head
                    // the fallback lost (`this` inside an extension body is
                    // the `on` type by definition, upstream
                    // `ExtensionElement.extendedType`).
                    let ty = self.this_static_ty(container, kind);
                    let unresolved_unnamed = kind == ContainerKind::Extension
                        && matches!(&ty, Ty::Named { name, .. } if *name == container);
                    if unresolved_unnamed
                        && let Some(head) = self.enclosing_extension_on(read.offset)
                    {
                        Some(Ty::named(self.intern(&head)))
                    } else {
                        Some(ty)
                    }
                }
                Receiver::Created { ty: Some(ty), .. } => Some(ty),
                Receiver::Created { name, .. } => Some(Ty::named(name)),
                _ => None,
            };
            // `x.p ??= v` / `x.p += v` / `x.f++`: the member the write
            // targets credits its getter+setter pair on the RECEIVER's
            // type (the oracle's `writeElement`/`readElement` join —
            // `accept_setter`'s resolve-side latch).
            let name_units = self.member_name_units(read.name);
            if let Some(member) = self.adjacent_write_member_at(read.offset, name_units)
                && let Some(receiver) = receiver
            {
                self.credit_face_member_ty(&faces, &declarers, member, receiver, &mut index_read_used);
                let setter = self.setter_spelling(member);
                self.credit_face_member_ty(&faces, &declarers, setter, receiver, &mut index_read_used);
            }
            // The operator half (`x.f += 1`/`x.f++`): the application
            // resolves `operator<op>` on the member's DECLARED value type,
            // never on the receiver's container.
            if let Some(op) = self.adjacent_operator_at(read.offset, name_units) {
                let operand = self
                    .member_read_value_ty(&read)
                    .or(receiver)
                    .unwrap_or(Ty::UNKNOWN);
                self.credit_face_member_ty(&faces, &declarers, op, operand, &mut index_read_used);
            }
        }
        // The bare-record arm: `refs` carries every bare identifier the
        // push resolved — local operands, implicit-this members, extension
        // `Opaque` hits, top-levels. The adjacent token re-derives the
        // same write/operator/`call` applications on the operand's own
        // type; the operand's face answers through the read-type tables
        // first, the declaration model's shape next.
        let mut ref_index = 0usize;
        let ref_count = self.refs.len();
        while ref_index < ref_count {
            let unit_ref = self.refs[ref_index];
            ref_index += 1;
            // A prefix/selector token is a member access's middle segment —
            // its own adjacent token is the `.`, never an application.
            if matches!(unit_ref.target, RefTarget::Prefix | RefTarget::This) {
                continue;
            }
            let name_units = self.member_name_units(unit_ref.name);
            // The write half: an implicit-this member's `??=`/compound/
            // increment credits the accessor pair on the enclosing `on`/
            // container type (`this.p ??= v` inside `extension on String`
            // resolves both). A local or top-level write credits nothing —
            // the member arms own the element credit.
            if let Some(member) = self.adjacent_write_member_at(unit_ref.range.offset, name_units) {
                let owner_ty = match unit_ref.target {
                    RefTarget::Member { container } => Some(Ty::named(container)),
                    // `RefTarget::Opaque` inside an extension body is the
                    // `on`-face member scope's anonymous resolution (the
                    // mixin constraint / enum-synth / extension `has_extension_member`
                    // miss — `resolve_bare`'s Opaque family). `container_context()`
                    // reads dead frames at finalize, so the enclosing container
                    // re-derives from the source: the backward extension-header
                    // scan already defined above answers the `on` head the
                    // write's receiver type is.
                    RefTarget::Opaque => self
                        .enclosing_extension_on(unit_ref.range.offset)
                        .map(|head| Ty::named(self.intern(&head))),
                    _ => None,
                };
                if let Some(receiver) = owner_ty {
                    self.credit_face_member_ty(&faces, &declarers, member, receiver, &mut index_read_used);
                    let setter = self.setter_spelling(member);
                    self.credit_face_member_ty(&faces, &declarers, setter, receiver, &mut index_read_used);
                }
            }
            if let Some(op) = self.adjacent_operator_at(unit_ref.range.offset, name_units)
                && let Some(operand) = self.operand_ty_of_ref(&unit_ref)
            {
                self.credit_face_member_ty(
                    &faces,
                    &declarers,
                    op,
                    operand,
                    &mut index_read_used,
                );
            }
            // The implicit-`call` arm (`visitFunctionExpressionInvocation`,
            // unused_local_elements_verifier.dart:175-180): a bare callee
            // the tables type record or function resolves `call` through
            // the shape face — `a()` over `extension on (String,)`.
            if self.byte_at(self.skip_whitespace(unit_ref.range.end())) == Some(b'(')
                && let Some(operand) = self.operand_ty_of_ref(&unit_ref)
                && matches!(operand, Ty::Record { .. } | Ty::Function { .. })
            {
                self.credit_face_member_ty(
                    &faces,
                    &declarers,
                    call_spelling,
                    operand,
                    &mut index_read_used,
                );
            }
        }
        // The literal-prefix arm: a unary `~`/`-`/`++`/`--` ahead of a
        // literal operand resolves `operator` on the literal's own type
        // (`~"hello"` → `String`, `-1` → `int`). The bare-ref arm covers
        // only identifier operands — a literal carries no `refs` entry, so
        // the token stream re-derives this one family directly.
        self.credit_prefix_literal_operators(&faces, &declarers, &mut index_read_used);
        // The write-artifact scrub (`writeOrReadElement` is `[]=` alone for
        // every index write form): the resolve side credits the `[]` read
        // spelling beside `[]=` on every applied write row, so a `(file,
        // index, "[]")` entry that pairs with `[]=` but no read-position
        // credit ever justified is the artifact — purge it before the
        // verdict walks read the pool. A lone `[]` entry (no `[]=` pair) is
        // a pure read the resolve side already settled — it stays.
        let artifact_reads: Vec<(FileId, u32)> = self
            .extension_member_used
            .iter()
            .filter(|&&(face_file, face_index, spelling)| {
                spelling == index_get
                    && !index_read_used.contains(&(face_file, face_index))
                    && self
                        .extension_member_used
                        .contains(&(face_file, face_index, index_set))
            })
            .map(|&(face_file, face_index, _)| (face_file, face_index))
            .collect();
        for (face_file, face_index) in artifact_reads {
            self.extension_member_used
                .remove(&(face_file, face_index, index_get));
        }
    }

    /// The member spelling a `??=`/compound/`++`/`--` write credits beside
    /// the operator: the write's own target name (the getter whose write
    /// the `visitCompoundAssignmentExpression`/`visitIfNullAssignment`
    /// records — `p ??= v` reads `p`, writes `p=`). `offset` is the name
    /// token's UTF-16 offset, `name_units` its UTF-16 length; `None` when
    /// the adjacent token is not an assignment-operator face (a plain `=`
    /// — pure write, no read half; a read-position access — nothing to
    /// double-credit).
    fn adjacent_write_member_at(&mut self, offset: u32, name_units: u32) -> Option<StringId> {
        let tokens = self.tokens.as_slice();
        let end = offset + name_units;
        let name = self
            .tokens
            .as_slice()
            .iter()
            .find(|token| token.utf16_offset == offset)
            .map(|token| self.intern(&self.source[token.byte_range()]))?;
        let at = tokens.partition_point(|token| token.utf16_offset < end);
        let suffix = tokens.get(at).map(|token| &self.source[token.byte_range()]);
        match suffix {
            // `??=` and every compound/`++`/`--` suffix run the
            // read-plus-write resolution — the member's getter and setter
            // both credit.
            Some(
                "??=" | "++" | "--" | "+=" | "-=" | "*=" | "/=" | "%=" | "~/=" | "&=" | "|=" | "^="
                | "<<=" | ">>=" | ">>>=",
            ) => Some(name),
            // A `++`/`--` prefix is the same read-write (the operand's own
            // name credits the pair the postfix face does).
            _ => tokens
                .iter()
                .rposition(|token| token.utf16_offset < offset)
                .map(|index| &self.source[tokens[index].byte_range()])
                .filter(|before| matches!(*before, "++" | "--"))
                .map(|_| name),
        }
    }

    /// The `operator` member spelling an adjacent-token access resolves
    /// on the operand's type — the row model stages no `Intent` marker for
    /// the family. The suffix decides compounds, postfix increments,
    /// plain binary operators, and index applications; the token before
    /// the name decides prefixes (`~x`, `-x`, `++x`, `--x`) — `unary-` is
    /// the analyzer's member spelling for the negation. `??=` is not an
    /// operator member (the `ifNull` resolution credits no `operator`).
    /// `==`/`!=` resolve through `Object.==`, never an extension member.
    fn adjacent_operator_at(&mut self, offset: u32, name_units: u32) -> Option<StringId> {
        let tokens = self.tokens.as_slice();
        let end = offset + name_units;
        let at = tokens.partition_point(|token| token.utf16_offset < end);
        let suffix = tokens.get(at).map(|token| &self.source[token.byte_range()]);
        let member = match suffix {
            Some("+=" | "++") => Some("+"),
            Some("-=" | "--") => Some("-"),
            Some("*=") => Some("*"),
            Some("/=") => Some("/"),
            Some("%=") => Some("%"),
            Some("~/=") => Some("~/"),
            Some("&=") => Some("&"),
            Some("|=") => Some("|"),
            Some("^=") => Some("^"),
            Some("<<=") => Some("<<"),
            Some(">>=") => Some(">>"),
            Some(">>>=") => Some(">>>"),
            _ => None,
        };
        if member.is_some() {
            return member.map(|op| self.intern(op));
        }
        // The plain binary / index face: `a + 0` / `a[i]` — the name's own
        // operand joins the extension face's operator member. `==`/`!=` are
        // `Object` members, never extension; `<`/`>` need whitespace before
        // the angle (a glued `<` after an identifier is a generic
        // instantiation, never `operator<`). `&&`/`||`/`??` are built-in
        // lazies, not operator members.
        match suffix {
            Some("+" | "-" | "*" | "/" | "%" | "~/" | "&" | "|" | "^" | "<<" | ">>") => {
                return suffix.map(|op| self.intern(op));
            }
            Some("<" | ">" | "<=" | ">=") => {
                // Spaced-gate: `a < b` is `operator<`/`operator<=` but
                // `List<int>` has no whitespace before the `<` — the token
                // before the suffix is the name itself, so the whitespace
                // between the name and the operator decides. `<=`/`>=`
                // never follow an identifier without space in valid code.
                let name_end_byte = self.byte_of(end);
                let suffix_start_byte =
                    self.byte_of(tokens.get(at).map_or(end, |token| token.utf16_offset));
                if suffix_start_byte > name_end_byte {
                    return suffix.map(|op| self.intern(op));
                }
            }
            Some("[") => {
                // `a[i]` → `[]`, `a[i] = v` → `[]=` — the token after the
                // matching `]` decides. Upstream's `writeOrReadElement`
                // resolves `[]=` for EVERY assignment form (`=`, `??=`,
                // `+=`, `++`/`--`), so a compound tail credits the write
                // element alone.
                let member = if tokens.get(at).is_some_and(|token| {
                    self.index_write_after(token.byte_offset as usize)
                        || self.index_compound_after(token.byte_offset as usize)
                }) {
                    "[]="
                } else {
                    "[]"
                };
                return Some(self.intern(member));
            }
            Some("?")
                if tokens
                    .get(at)
                    .is_some_and(|token| token.utf16_offset == end)
                    && tokens
                        .get(at + 1)
                        .is_some_and(|token| &self.source[token.byte_range()] == "[") =>
            {
                // The null-aware index `a?[i]` — `?` scans as its own
                // token glued to the name's end (a spaced `?` is a
                // conditional and never reaches this arm). The same
                // write-element rule applies after the `]`.
                let member = if tokens.get(at + 1).is_some_and(|token| {
                    self.index_write_after(token.byte_offset as usize)
                        || self.index_compound_after(token.byte_offset as usize)
                }) {
                    "[]="
                } else {
                    "[]"
                };
                return Some(self.intern(member));
            }
            _ => {}
        }
        // The prefix face: `~`/`-`/`++`/`--` immediately before the name —
        // a binary `-` before it reads as the left operand's own minus,
        // so only the unary forms count (`++`/`--`/`~` are unambiguous,
        // `-` takes the `unary-` spelling only when the token before IT
        // is not an operand-ending token).
        let before = tokens
            .iter()
            .rposition(|token| token.utf16_offset < offset)
            .map(|index| (index, &self.source[tokens[index].byte_range()]));
        match before {
            Some((_, "~")) => Some(self.intern("~")),
            Some((index, "-")) => {
                // `-x` vs `y - x`: the token before the `-` ends an
                // operand (an identifier, literal, `)`, `]`, `}`) it's the
                // binary minus; a prefix position (`(`, `=`, `,`, `return`,
                // a statement boundary) spells `unary-`.
                let prior = index
                    .checked_sub(1)
                    .and_then(|prior| tokens.get(prior))
                    .map(|token| &self.source[token.byte_range()]);
                let unary = prior.is_none_or(|text| {
                    matches!(
                        text,
                        "(" | "["
                            | "{"
                            | ","
                            | "="
                            | "=="
                            | "!="
                            | "=>"
                            | "return"
                            | "case"
                            | ";"
                            | ":"
                            | "?"
                            | "&&"
                            | "||"
                            | "??"
                            | "??="
                    )
                });
                unary.then(|| self.intern("unary-"))
            }
            Some((_, "++")) => Some(self.intern("+")),
            Some((_, "--")) => Some(self.intern("-")),
            _ => None,
        }
    }

    /// The `Ty`-shaped twin of [`Self::credit_face_member`]: the head-
    /// spelled guard chain plus the `on`-shape admission the generic
    /// function/record/type-parameter faces need (a bare `on` head match
    /// cannot see `void Function<T>(T)` apply to a `void Function(int)`
    /// receiver, or `(String,)` apply to a record literal). The
    /// `member_found` pre-gate keeps the real-container priority — a
    /// member the receiver's own class declares takes the use before any
    /// extension face does.
    fn credit_face_member_ty(
        &mut self,
        faces: &[(&ExtensionFace, FileId)],
        declarers: &FxHashMap<StringId, u32>,
        member: StringId,
        receiver: Ty,
        index_read_used: &mut FxHashSet<(FileId, u32)>,
    ) -> bool {
        if declarers.get(&member).copied() != Some(1) {
            return false;
        }
        if let Ty::Named { name, .. } = receiver
            && self.member_found(name, member, false)
        {
            return false;
        }
        let Some(&(face, face_file)) = faces.iter().find(|(face, _)| {
            face.members.contains_key(&member) && self.extension_on_admits(face, receiver)
        }) else {
            return false;
        };
        if let Some(face_index) = self
            .index
            .get(face_file)
            .and_then(|elements| {
                elements
                    .extension_faces()
                    .iter()
                    .position(|entry| std::ptr::eq(entry, face))
            })
            .and_then(|index| u32::try_from(index).ok())
        {
            // A `[]` credit through this arm is always read-position — the
            // caller's `adjacent_operator_at` already split the write
            // verdict (`[]=`) out, so the read spelling here justifies the
            // pool entry the write-artifact scrub honors.
            if member == self.intern("[]") {
                index_read_used.insert((face_file, face_index));
            }
            self.extension_member_used
                .insert((face_file, face_index, member));
            self.credit_extension_import(face, face_file);
            return true;
        }
        false
    }

    /// Whether the face's `on` type admits a receiver of `ty`: the
    /// `extension_application` applicability the row-driven arm replays.
    /// A `Named` `on` head answers by named-head equality or the type-
    /// parameter-bound arm (`extension<T> on T` admits any receiver whose
    /// type satisfies the bound — the universal arm's shape at row level);
    /// a `Record` `on` admits a record receiver of the same interned
    /// shape; a `Function` `on` admits a function-typed receiver or a
    /// `Named` head the alias fold unwraps to a function shape
    /// (`on Function` itself stays `Named` — the bare head never lands
    /// here).
    fn extension_on_admits(&self, face: &ExtensionFace, ty: Ty) -> bool {
        match (face.on_ty, ty) {
            (Ty::Named { name: on_name, .. }, Ty::Named { name, .. }) => {
                if on_name == name {
                    return true;
                }
                // `extension<T> on T` / `on T?`: the parameter's bound is
                // the admitting type (an unbounded parameter admits every
                // type — the universal arm). `named_sub` settles the
                // receiver against the bound.
                if face.type_param_names.contains(&on_name) {
                    let bound = face
                        .type_param_names
                        .iter()
                        .position(|p| *p == on_name)
                        .and_then(|index| face.type_param_bounds.get(index).copied())
                        .unwrap_or(Ty::Dynamic);
                    return matches!(bound, Ty::Dynamic)
                        || self.name_world().named_sub(name, {
                            match bound {
                                Ty::Named {
                                    name: bound_name, ..
                                } => bound_name,
                                _ => return false,
                            }
                        });
                }
                false
            }
            // A record `on` admits the same interned record shape.
            (
                Ty::Record {
                    shape: on_shape, ..
                },
                Ty::Record { shape, .. },
            ) => on_shape == shape,
            // A function `on` admits a function-typed receiver only when
            // the signatures unify — `on void Function<T>(T)` applies to a
            // `void Function(int)` value (the oracle's `applicableTo`
            // instantiates T), never to `Function(bool, bool)`. The
            // unification the arm replays is name-level: same positional
            // arity and named set, each `on` formal slot either a face
            // type parameter (binds the receiver's slot) or the same `Ty`.
            (
                Ty::Function {
                    shape: on_shape, ..
                },
                Ty::Function { shape, .. },
            ) => self.fn_on_admits(face, on_shape, shape),
            // `on void Function(T)` written through a type parameter or an
            // alias head: the folded receiver type decides — the same
            // signature gate runs against the folded shape, so a
            // `typedef F = void Function(int)` receiver admits `on
            // Function<T>(T)` exactly as the written form does.
            (
                Ty::Function {
                    shape: on_shape, ..
                },
                _,
            ) => match self.fold_alias_ty(ty) {
                Ty::Function { shape, .. } => self.fn_on_admits(face, on_shape, shape),
                _ => false,
            },
            _ => false,
        }
    }

    /// The function-`on` unification the row-driven arm replays:
    /// `on R Function<T>(A, {B b})` applies to a `R Function(A, {B b})`
    /// receiver when the signatures match slot-for-slot — same positional
    /// arity and named set — with each `on` formal either a face type
    /// parameter (the oracle instantiates it to the receiver's type) or
    /// the same interned `Ty`. Return-type drift is the established
    /// under-report the arm keeps (the oracle's return covariance is a
    /// wider gate the name-level shape cannot fully replay).
    fn fn_on_admits(
        &self,
        face: &ExtensionFace,
        on: crate::model::FnShapeId,
        got: crate::model::FnShapeId,
    ) -> bool {
        let is_face_param = |ty: &Ty| match ty {
            Ty::Named { name, .. } => face.type_param_names.contains(name),
            _ => false,
        };
        let pool = self.boundary.pool();
        let on = pool.function_shape(on);
        let got = pool.function_shape(got);
        on.positional.len() == got.positional.len()
            && on.named.len() == got.named.len()
            && (is_face_param(&on.return_ty) || on.return_ty == got.return_ty)
            && on
                .positional
                .iter()
                .zip(got.positional.iter())
                .all(|(on, got)| is_face_param(on) || on == got)
            && on
                .named
                .iter()
                .zip(got.named.iter())
                .all(|((on_name, on), (got_name, got))| {
                    on_name == got_name && (is_face_param(on) || on == got)
                })
    }

    /// The operand's `Ty` for a `member_reads` record's own value — the
    /// accessor's declared face on the receiver's container (`x.f += 1`
    /// reads `f`'s `void Function(int)`, not `x`'s `C`). The receiver's
    /// re-derived expression type names the walk's container; the member
    /// table's `var_ty`/`return_ty` answers the value.
    fn member_read_value_ty(&self, read: &super::types::MemberRead) -> Option<Ty> {
        // The unknown-receiver side table holds the receiver's own shape
        // (record/function) — for THAT face the member's value is not a
        // container lookup at all; the chain face's record field or
        // `call` shape is outside the name-level model (under-report).
        let recv_ty = self
            .recv_expression_ty(read.recv_span)
            .or(match read.recv {
                Receiver::Typed { ty, .. } => Some(ty),
                Receiver::This { container, kind } => Some(self.this_static_ty(container, kind)),
                Receiver::Created { ty: Some(ty), .. } => Some(ty),
                Receiver::Created { ty: None, name, .. } => Some(Ty::named(name)),
                _ => None,
            })?;
        let Ty::Named { name, .. } = self.fold_alias_ty(recv_ty) else {
            return None;
        };
        let ty = self.member_declared_ty(name, read.name);
        (ty != Ty::UNKNOWN).then_some(ty)
    }

    /// The operand `Ty` a bare `refs` entry carries — the flow read type
    /// first (the promoted/captured static face), then the declaration
    /// model's shape: a local's derived initializer face, a top-level
    /// function's signature, an implicit-this member's declared face, a
    /// type alias folded to a record or function shape.
    fn operand_ty_of_ref(&self, unit_ref: &UnitRef) -> Option<Ty> {
        if let Some(ty) = self
            .read_type_at(unit_ref.range)
            .filter(|ty| *ty != Ty::UNKNOWN)
        {
            return Some(ty);
        }
        if let Some(&decl) = self.local_decl_at_read.get(&unit_ref.range.offset)
            && let Some(ty) = self.init_derived_types.borrow().get(&decl).copied()
            && ty != Ty::UNKNOWN
        {
            return Some(ty);
        }
        match unit_ref.target {
            RefTarget::Decl { .. } => {
                let decl = self.top_level_decl(unit_ref.name)?;
                let ty = if matches!(decl.kind, MicroKind::TopLevelFn) {
                    decl.formals.as_deref().map(|formals| {
                        let sig = FnVarSig {
                            params: Box::default(),
                            required_positional: u16::try_from(
                                formals.positional.iter().filter(|f| f.required).count(),
                            )
                            .unwrap_or(u16::MAX),
                            positional: formals
                                .positional
                                .iter()
                                .map(|f| f.ty)
                                .collect::<Vec<_>>()
                                .into_boxed_slice(),
                            named: formals
                                .named
                                .iter()
                                .map(|f| (f.name, f.ty, f.required))
                                .collect::<Vec<_>>()
                                .into_boxed_slice(),
                            return_ty: decl.return_ty,
                        };
                        self.fn_sig_ty(&sig)
                    })
                } else {
                    Some(decl.var_ty)
                }?;
                (ty != Ty::UNKNOWN).then_some(self.fold_alias_ty(ty))
            }
            RefTarget::Member { container } => {
                let ty = self.member_declared_ty(container, unit_ref.name);
                (ty != Ty::UNKNOWN).then_some(ty)
            }
            RefTarget::Local => {
                // Post-walk derivation — the frame roster is gone by
                // finalize, so `local_fn_sig`/`local_declared_ty` cannot
                // answer. `local_decl_at_read` maps the read to the
                // declarator's name-token offset; the byte after it and
                // the type-use rows ending before it re-derive the face
                // (the same legs the resolution side's `Local` arm runs):
                // `name(` is a local function, `name = (` an inferred
                // closure, otherwise the last type annotation ending
                // ahead of the name is the declared face — a record
                // (`(String,) a`), a function (`void Function<T>(T) f`),
                // or a type parameter whose bound carries the shape.
                let decl_offset = *self.local_decl_at_read.get(&unit_ref.range.offset)?;
                let name_len = self.strings.resolve(unit_ref.name).len() as u32;
                let bytes = self.source.as_bytes();
                let mut at = self.byte_of(decl_offset + name_len);
                while bytes.get(at).is_some_and(|b| b.is_ascii_whitespace()) {
                    at += 1;
                }
                match bytes.get(at) {
                    Some(b'(') => {
                        let sig_ty = self.closure_init_ty(bytes, at);
                        let Ty::Function { shape, .. } = sig_ty else {
                            return None;
                        };
                        let (type_params, positional, named) = {
                            let pool = self.boundary.pool();
                            let shape = pool.function_shape(shape);
                            (
                                shape.type_params,
                                shape.positional.to_vec(),
                                shape.named.to_vec(),
                            )
                        };
                        let decl_byte = self.byte_of(decl_offset);
                        let Some(ret) = self
                            .type_uses
                            .iter()
                            .filter(|(range, _)| range.end() <= decl_offset)
                            .filter(|(range, _)| {
                                bytes[self.byte_of(range.end())..decl_byte]
                                    .iter()
                                    .all(u8::is_ascii_whitespace)
                            })
                            .last()
                            .map(|(_, name)| Ty::named(*name))
                        else {
                            return Some(sig_ty);
                        };
                        Some(self.boundary.intern_function_ty(
                            type_params,
                            ret,
                            &positional,
                            &named,
                            false,
                        ))
                    }
                    Some(b'=') => {
                        let mut init = at + 1;
                        while bytes.get(init).is_some_and(|b| b.is_ascii_whitespace()) {
                            init += 1;
                        }
                        if bytes.get(init) == Some(&b'(') {
                            let ty = self.closure_init_ty(bytes, init);
                            return (ty != Ty::UNKNOWN).then_some(ty);
                        }
                        None
                    }
                    _ => {
                        let decl_byte = self.byte_of(decl_offset);
                        let annotation = self
                            .type_uses
                            .iter()
                            .filter(|(range, _)| range.end() <= decl_offset)
                            .filter(|(range, _)| {
                                bytes[self.byte_of(range.end())..decl_byte]
                                    .iter()
                                    .all(u8::is_ascii_whitespace)
                            })
                            .last()
                            .map(|(_, name)| *name)?;
                        let folded = self.fold_alias_ty(Ty::named(annotation));
                        if matches!(folded, Ty::Record { .. } | Ty::Function { .. }) {
                            return Some(folded);
                        }
                        // A type-parameter spelling (`T extends int
                        // Function(int)`): the bound's shape decides the
                        // operator/call face.
                        if let Ty::Named { name, .. } = folded
                            && let Some(bound) = self.param_bound_of(name)
                        {
                            let bound = self.fold_alias_ty(bound);
                            if matches!(bound, Ty::Record { .. } | Ty::Function { .. }) {
                                return Some(bound);
                            }
                        }
                        None
                    }
                }
            }
            _ => None,
        }
    }

    /// The literal-operand prefix arm (`~"hello"`, `-1`, `--x`): a unary
    /// `~`/`-`/`++`/`--` immediately before a literal token resolves the
    /// operator member on the literal's own head — the one operand shape
    /// no `refs`/`member_reads` record carries.
    fn credit_prefix_literal_operators(
        &mut self,
        faces: &[(&ExtensionFace, FileId)],
        declarers: &FxHashMap<StringId, u32>,
        index_read_used: &mut FxHashSet<(FileId, u32)>,
    ) {
        let tokens = self.tokens.as_slice();
        let mut index = 0usize;
        while index + 1 < tokens.len() {
            let text = &self.source[tokens[index].byte_range()];
            let op = match text {
                "~" => Some("~"),
                "-" => {
                    // Unary only — the token before must not end an operand.
                    let prior = index
                        .checked_sub(1)
                        .and_then(|prior| tokens.get(prior))
                        .map(|token| &self.source[token.byte_range()]);
                    prior
                        .is_none_or(|text| {
                            matches!(
                                text,
                                "(" | "["
                                    | "{"
                                    | ","
                                    | "="
                                    | "=="
                                    | "!="
                                    | "=>"
                                    | "return"
                                    | "case"
                                    | ";"
                                    | ":"
                                    | "?"
                                    | "&&"
                                    | "||"
                                    | "??"
                                    | "??="
                            )
                        })
                        .then_some("unary-")
                }
                "++" => Some("+"),
                "--" => Some("-"),
                _ => None,
            };
            let Some(op) = op else {
                index += 1;
                continue;
            };
            let operand = &tokens[index + 1];
            let head = match operand.kind {
                dartq_scanner::TokenKind::String
                | dartq_scanner::TokenKind::StringInterpolationIdentifier => {
                    Some(self.intern("String"))
                }
                dartq_scanner::TokenKind::Int | dartq_scanner::TokenKind::Hexadecimal => {
                    Some(self.intern("int"))
                }
                dartq_scanner::TokenKind::Double => Some(self.intern("double")),
                _ => None,
            };
            if let Some(head) = head {
                self.credit_face_member_ty(
                    faces,
                    declarers,
                    self.intern(op),
                    Ty::named(head),
                    index_read_used,
                );
            }
            index += 1;
        }
    }

    /// The #3479 correction: strip the read credit a write-position access
    /// carried into the pools. Three shapes land here:
    ///
    /// - A MEMBER-target read/compound the expression statement discarded
    ///   (`a._f += 1;`, `a._f++;`, `!a._f;`, `-a._f;`, `a._f!;`): the
    ///   walk's `statement_discarded_rows` gates bare rows only (the
    ///   receiver's own credit stands either way), so the member's
    ///   `member_read_owners`/`read_names` entries ride a read the oracle
    ///   never recorded (`_isReadIdentifier`'s `ExpressionStatement`
    ///   gate, unused_local_elements_verifier.dart:418-440). The token
    ///   probe re-decides the same shapes: a postfix `++`/`--`/`!` or a
    ///   compound assign right after the member spelling, or a prefix
    ///   `++`/`--`/`-`/`~`/`!` right before the receiver, discards the
    ///   member's read when the whole thing sits at statement root —
    ///   nested positions (`x = a._f += 1`, `return a._f++`) keep it.
    /// - A bare `refs`-member target in the same discarded window: the
    ///   `statement_discarded_rows` set already keys its offset, the
    ///   `field_read_rows` join supplies the owner the member walk would
    ///   have recorded.
    /// - An initializing-formal read bound to the field: `C({this._x}) :
    ///   _y = _x` — the `_x` reference resolves the `this._x` formal
    ///   upstream, never the field. The field's `member_read_owners`
    ///   credit came from the member-target join; the owner container's
    ///   constructor declaring `this._x` proves the formal bound the
    ///   name, so the read belongs to the parameter.
    ///
    /// The pools are corrected in place (`read_names`,
    /// `member_read_owners`): a name keeps its credit while any
    /// non-discarded evidence remains — the under-report direction.
    fn discard_write_position_reads(&mut self) {
        if self.member_read_owners.is_empty() && self.read_names.is_empty() {
            return;
        }
        // The self-read arm (#3478): `field_read_rows` latches a
        // `Member{container}` resolution at push time — including the
        // field's OWN declaration token (`int _f = 0`'s `_f`), which
        // resolves `Member{A}` exactly like a read would but is a
        // declaration, not a use. Its offset sits in `field_read_rows`
        // and credits `member_read_owners` through `resolve_bare_row`'s
        // owner join. `decl_sites` identifies the declaration: every
        // `field_read_rows` offset that IS a decl-site anchor is the
        // declaration's self-credit — strip it before the spare checks
        // run. A `_f` read in `m(int _f)` — a local resolution — never
        // reaches `field_read_rows` at all, so its `(0, A)` credit came
        // from the declaration alone.
        for (&offset, &container) in self.field_read_rows.clone().iter() {
            let is_decl = self
                .decl_sites
                .iter()
                .any(|site| site.range.offset == offset);
            if !is_decl {
                continue;
            }
            let Some(name) = self
                .refs
                .iter()
                .find(|r| r.range.offset == offset)
                .map(|r| r.name)
            else {
                continue;
            };
            if let Some(owners) = self.member_read_owners.get_mut(&name) {
                owners.remove(&(self.file, container));
            }
        }
        // The discarded member-read offsets: every `member_reads` entry
        // whose finalized intent reads and whose statement-position probe
        // fires. `member_reads` has no intent column — the row at the same
        // offset answers it (a Write-intent row carries no read evidence
        // to strip).
        let mut discarded: FxHashSet<u32> = FxHashSet::default();
        for read in &self.member_reads {
            if self.member_read_discarded(read) {
                discarded.insert(read.offset);
            }
        }
        // The bare discarded set: the walk's own verdict plus the
        // increment operands the member-target probe cannot see (a bare
        // `_f++` already sits in `statement_discarded_rows`; a bare
        // `_f` used as a formal write — `this._x = _x`'s RHS — is the
        // initializing-formal arm below).
        let statement_discarded: FxHashSet<u32> = self
            .statement_discarded_rows
            .iter()
            .copied()
            .chain(discarded.iter().copied())
            .collect();
        if statement_discarded.is_empty() && self.ctor_formals_init.is_empty() {
            return;
        }
        // The member owner a discarded read recorded — the same handle
        // `note_member_read` keys on, re-derived from the receiver (the
        // frames are gone; `This`'s extension arm resolves the `on` type
        // the read credits).
        let discarded_owner_of = |engine: &Self,
                                  read: &super::types::MemberRead|
         -> Option<(FileId, StringId)> {
            match read.recv {
                Receiver::This { container, kind } if kind == ContainerKind::Extension => engine
                    .enclosing_extension_on_type(container)
                    .map(|on| (engine.file, on)),
                Receiver::This { container, .. } | Receiver::Super { container, .. } => {
                    Some((engine.file, container))
                }
                Receiver::Container { name, .. } => Some((engine.file, name)),
                Receiver::Typed { type_name, .. } => resolve_container(
                    engine.graph,
                    engine.namespaces,
                    engine.index,
                    engine.lib,
                    engine.file,
                    type_name,
                )
                .map(|(_, owner)| (owner, type_name)),
                _ => None,
            }
        };
        // The pure-write member accesses (`a._f = 1`, `A() : _f = 0`'s
        // LHS): the recorded read's owner credit is a write the oracle
        // never ran — strip it the same way the discarded set does. The
        // row stack is drained by finalize, so the intent re-derives from
        // the surviving pools: `refs` `pure_write_at` probes (a `v = 3`
        // write) joined with `member_refs` `Written` edges (the `_f = 0`
        // initializer LHS, a bare `this`-implicit assignment).
        let write_offsets: FxHashSet<u32> = self
            .refs
            .iter()
            .filter(|r| self.pure_write_at(r.range.offset, r.range.end()))
            .map(|r| r.range.offset)
            .chain(
                self.report
                    .member_refs
                    .iter()
                    .filter(|edge| matches!(edge.kind, MemberRefKind::Written))
                    .map(|edge| edge.range.offset),
            )
            .collect();
        for read in &self.member_reads {
            if !(discarded.contains(&read.offset) || write_offsets.contains(&read.offset)) {
                continue;
            }
            if let Some(owner) = discarded_owner_of(self, read)
                && let Some(owners) = self.member_read_owners.get_mut(&read.name)
            {
                owners.remove(&owner);
            }
        }
        // The bare discarded rows: a `field_read_rows`-backed bare read
        // the statement dropped credits the pushed container — the same
        // (file, owner) the pass-2 read gate joined.
        for (&offset, &container) in self.field_read_rows.clone().iter() {
            if !self.statement_discarded_rows.contains(&offset) {
                continue;
            }
            // The bare row's own spelling is the `refs` payload at the
            // offset — `rows` is drained by `resolve_rows` before this
            // pass runs, so `refs` is the surviving carrier; `field_read_rows`
            // keys the field's declaring container, the name the row
            // resolved.
            let Some(name) = self
                .refs
                .iter()
                .find(|r| r.range.offset == offset)
                .map(|r| r.name)
            else {
                continue;
            };
            if let Some(owners) = self.member_read_owners.get_mut(&name) {
                owners.remove(&(self.file, container));
            }
        }
        // The shadowed-local arm (#3478): `field_read_rows` latched the
        // field credit at push time on a name-match, blind to shadowing —
        // a parameter or local of the same spelling resolves
        // `RefTarget::Local`, so the read is the local's, never the
        // field's. `local_decl_at_read` proves the local resolution:
        // every `field_read_rows` offset that also has a local-decl
        // entry strips its (file, container) owner credit.
        for (&offset, &container) in self.field_read_rows.clone().iter() {
            if !self.local_decl_at_read.contains_key(&offset) {
                continue;
            }
            let Some(name) = self
                .refs
                .iter()
                .find(|r| r.range.offset == offset)
                .map(|r| r.name)
            else {
                continue;
            };
            if let Some(owners) = self.member_read_owners.get_mut(&name) {
                owners.remove(&(self.file, container));
            }
        }
        // The initializing-formal arm: a bare read inside a constructor
        // initializer whose spelling matches one of that constructor's
        // `this.` formals is the FORMAL's read, never the field's — strip
        // every `member_read_owners` credit the member-bound join
        // recorded for it. The site walks its own constructor's formals:
        // the row sits inside the initializer list of the nearest
        // preceding `Constructor` site of the owner container.
        if !self.ctor_formals_init.is_empty() {
            let formal_names: FxHashSet<StringId> = self
                .ctor_formals_init
                .iter()
                .map(|(_, name)| *name)
                .collect();
            // A member read the formal shadowed: the row's container
            // declares an initializing formal under the same spelling —
            // drop the (file, owner) credit for every discarded formal
            // name the owner's ctors recorded.
            let formal_shadowed: FxHashSet<StringId> = formal_names
                .iter()
                .copied()
                .filter(|name| {
                    // The row stack is drained by finalize — the read
                    // re-derives from `refs`: an `init_rows` offset whose
                    // entry is not a pure write is the read the formal
                    // bound (a `Intent::Read`/`ReadWrite` the drained row
                    // recorded).
                    self.refs.iter().any(|r| {
                        r.name == *name
                            && self.init_rows.contains_key(&r.range.offset)
                            && !self.pure_write_at(r.range.offset, r.range.end())
                    })
                })
                .collect();
            for name in formal_shadowed {
                // The container whose own ctor declared `this.name` keeps
                // no credit — that read is the formal's. A DIFFERENT
                // container's same-spelled field keeps its read. Resolved
                // up front so the mutable borrow below stays flat.
                let dead: FxHashSet<(FileId, StringId)> = self
                    .member_read_owners
                    .get(&name)
                    .into_iter()
                    .flatten()
                    .copied()
                    .filter(|(_, container)| self.container_declares_init_formal(*container, name))
                    .collect();
                if dead.is_empty() {
                    continue;
                }
                if let Some(owners) = self.member_read_owners.get_mut(&name) {
                    owners.retain(|owner| !dead.contains(owner));
                }
            }
        }
        // The name-level pool: a name keeps its `read_names` entry while
        // any non-discarded evidence remains — a member read outside the
        // discarded set or a member-bound `refs` row outside it. Names
        // whose every read was discarded drop out, so the `unresolved_
        // read` arm's spelling fallback cannot rescue them either.
        // Dead evidence = the discarded set PLUS every member-read offset
        // whose row finalized a pure `Intent::Write` — a write-position
        // access (`A() : _y = _x`'s LHS, `a._f = 1;`) recorded the member
        // write but the getter never ran, so it dead-ends the name's read
        // pool the same way a discarded statement does.
        let mut shadowed_offsets: FxHashMap<StringId, Vec<u32>> = FxHashMap::default();
        for (&offset, _) in self.field_read_rows.iter() {
            if self.local_decl_at_read.contains_key(&offset)
                && let Some(name) = self
                    .refs
                    .iter()
                    .find(|r| r.range.offset == offset)
                    .map(|r| r.name)
            {
                shadowed_offsets.entry(name).or_default().push(offset);
            }
        }
        let names: Vec<StringId> = self.read_names.iter().copied().collect();
        for name in names {
            let mut total = 0usize;
            let mut dead = 0usize;
            for offset in self
                .member_reads
                .iter()
                .filter(|read| read.name == name)
                .map(|read| read.offset)
                .chain(
                    self.refs
                        .iter()
                        .filter(|r| r.name == name && matches!(r.target, RefTarget::Member { .. }))
                        .map(|r| r.range.offset),
                )
                .chain(shadowed_offsets.get(&name).into_iter().flatten().copied())
            {
                total += 1;
                if statement_discarded.contains(&offset)
                    || write_offsets.contains(&offset)
                    || shadowed_offsets
                        .get(&name)
                        .is_some_and(|offsets| offsets.contains(&offset))
                {
                    dead += 1;
                }
            }
            let owners_empty = self
                .member_read_owners
                .get(&name)
                .is_none_or(|owners| owners.is_empty());
            if owners_empty && total > 0 && dead == total {
                self.read_names.remove(&name);
            }
        }
    }

    /// Whether `container`'s constructors declare `name` as an
    /// initializing formal (`this.name`) — the `ctor_formals_init` rows
    /// keyed by decl-site index; the site belongs to the container when
    /// its own `DeclKind::Constructor` names it.
    fn container_declares_init_formal(&self, container: StringId, name: StringId) -> bool {
        self.ctor_formals_init.iter().any(|(site, formal)| {
            *formal == name
                && self
                    .decl_sites
                    .get(*site)
                    .is_some_and(|site| matches!(site.kind, DeclKind::Constructor { container: c } if c == container))
        })
    }

    /// The nameless member's report anchor: the enclosing container's
    /// declared-name token at zero length. Upstream the `<unnamed>`
    /// element's `firstFragment.name` is null and the row lands on the
    /// parent fragment's name (`class C { var }` reports `unused_field`
    /// at `C`, `|0`; enum/mixin/extension-type containers answer the
    /// same). `site_index` is the member's own slot in `decl_sites`, so
    /// the scan finds the nearest preceding container site of that
    /// spelling — a duplicate `class C … class C` pair maps each member
    /// to its own header. No container site (a nameless container, or a
    /// named extension the `Container` kind never joins) falls back to
    /// the file start, `nameless_site_range`'s own default.
    fn nameless_member_anchor(&self, site_index: usize, container: StringId) -> SourceRange {
        self.decl_sites
            .get(..site_index)
            .unwrap_or(&[])
            .iter()
            .rev()
            .find(|site| matches!(site.kind, DeclKind::Container) && site.name == container)
            .map_or(SourceRange { offset: 0, length: 0 }, |site| {
                SourceRange {
                    offset: site.range.offset,
                    length: 0,
                }
            })
    }

    /// Whether a `member_reads` entry's read sat in a discarded statement
    /// position: a postfix `++`/`--`/`!` or a compound assign (`+=`,
    /// `-=`, `*=`, `/=`, `%=`, `~/=`, `&=`, `|=`, `^=`, `<<=`, `>>=`,
    /// `>>>=`) immediately after the member token, or a prefix
    /// `++`/`--`/`-`/`~`/`!` immediately before the receiver's first
    /// byte — with the whole expression rooted at an expression
    /// statement (a nested position keeps the read: `x = a._f += 1` is
    /// not discarded upstream either).
    fn member_read_discarded(&self, read: &super::types::MemberRead) -> bool {
        let tokens = self.tokens.as_slice();
        let Some(member_index) = tokens
            .iter()
            .position(|token| token.utf16_offset == read.offset)
        else {
            return false;
        };
        // The postfix / compound-write arm: the token right after the
        // member spelling decides. `!` discards ONLY at the expression's
        // own tail — a `.`/`?.`/`[`/`(` after it keeps the member access
        // going, so `_f` was read for its value (`a._f!.m()` reads
        // `_f` the same way a plain read does).
        let postfix = tokens.get(member_index + 1).is_some_and(|token| {
            let text = &self.source[token.byte_range()];
            if text == "!" {
                return tokens.get(member_index + 2).is_none_or(|after| {
                    !matches!(&self.source[after.byte_range()], "." | "?." | "[" | "(")
                });
            }
            // `++`/`--` discard the operand's read upstream. Compound
            // assigns (`+=`, `??=`, `*=`, …) are read+write — the read
            // half credits the member (the oracle's `writeElement` /
            // `readElement` join), so they keep the credit.
            matches!(text, "++" | "--")
        });
        // The prefix arm: the token right before the receiver's first
        // byte (`this._f` roots at `this`, `a.b._f` at `a`). A prefix
        // `++`/`--`/`-`/`~`/`!` discards the read upstream; the same
        // statement-root gate keeps the nested positions (`x = !a._f`).
        let recv_start = if read.recv_span.length > 0 {
            read.recv_span.offset
        } else {
            read.offset
        };
        let prefix_index = tokens
            .iter()
            .rposition(|token| token.utf16_offset < recv_start);
        let prefix = prefix_index.is_some_and(|index| {
            matches!(
                &self.source[tokens[index].byte_range()],
                "++" | "--" | "-" | "~" | "!"
            ) && self.statement_rooted_at(tokens[index].utf16_offset)
        });
        if !(postfix || prefix) {
            return false;
        }
        // The postfix arm's own statement check: the compound assign's
        // right operand may carry reads (`a._f += i` reads `i`), so the
        // boundary probe runs from the member expression's own start —
        // a `(`/`=`/`=>` upstream is a nested context.
        self.statement_rooted_at(recv_start)
    }

    /// Whether the expression starting at `offset` is rooted at an
    /// expression statement: walk backward over the token stream with a
    /// bracket depth — `)`/`]` deepen, `(`/`[` at depth 0 prove an open
    /// group (a nested position), `;`/`{`/`}` at depth 0 confirm the
    /// statement boundary, and every other depth-0 token (an operator,
    /// `=`, `,`, `=>`, a keyword) is a larger construct the expression
    /// nests inside — its read counts upstream.
    fn statement_rooted_at(&self, offset: u32) -> bool {
        let mut depth = 0u32;
        for token in self
            .tokens
            .as_slice()
            .iter()
            .rev()
            .filter(|token| token.utf16_offset < offset)
        {
            let text = &self.source[token.byte_range()];
            match text {
                ")" | "]" => depth += 1,
                "(" | "[" => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                }
                _ if depth == 0 => {
                    return matches!(text, ";" | "{" | "}");
                }
                _ => {}
            }
        }
        true
    }

    /// The shared operator/`call`/function-receiver credit path: the guard
    /// chain (exactly one accessible face declares the member, no real
    /// container member carries the spelling, the face's `on` head equals
    /// the receiver head) then the element-level insert the member arms
    /// read. Returns whether the credit landed.
    fn credit_face_member(
        &mut self,
        faces: &[(&ExtensionFace, FileId)],
        declarers: &FxHashMap<StringId, u32>,
        member: StringId,
        head: StringId,
        index_read_used: &mut FxHashSet<(FileId, u32)>,
    ) -> bool {
        if declarers.get(&member).copied() != Some(1) {
            return false;
        }
        if self.member_found(head, member, false) {
            return false;
        }
        let Some(&(face, face_file)) = faces.iter().find(|(face, _)| {
            face.members.contains_key(&member)
                && matches!(&face.on_ty, Ty::Named { name, .. } if *name == head)
        }) else {
            return false;
        };
        if let Some(face_index) = self
            .index
            .get(face_file)
            .and_then(|elements| {
                elements
                    .extension_faces()
                    .iter()
                    .position(|entry| std::ptr::eq(entry, face))
            })
            .and_then(|index| u32::try_from(index).ok())
        {
            // The literal-index byte arm calls this with `[]` for a read
            // follower alone — a genuine read-position credit.
            if member == self.intern("[]") {
                index_read_used.insert((face_file, face_index));
            }
            self.extension_member_used
                .insert((face_file, face_index, member));
            self.credit_extension_import(face, face_file);
            return true;
        }
        false
    }

    /// The overloadable operator starting at byte `at`, as its `operator`
    /// member spelling (`+=` and friends map to the base operator; `<=`,
    /// `>=`, `==`, `!=` are their own operators). `None` for a non-operator
    /// token — `=` alone, `=>`, `&&`, `||`, `??`, and punctuation are never
    /// `operator` declarations, so they never carry a member use. Longest
    /// spellings match first (`<<=` never reads as `<`).
    fn binary_operator_at(&self, at: usize) -> Option<StringId> {
        const TABLE: &[(&str, &str)] = &[
            ("<<=", "<<"),
            (">>=", ">>"),
            ("~/=", "~/"),
            ("+=", "+"),
            ("-=", "-"),
            ("*=", "*"),
            ("/=", "/"),
            ("%=", "%"),
            ("&=", "&"),
            ("|=", "|"),
            ("^=", "^"),
            ("<<", "<<"),
            (">>", ">>"),
            ("<=", "<="),
            (">=", ">="),
            ("==", "=="),
            ("!=", "!="),
            ("~/", "~/"),
            ("+", "+"),
            ("-", "-"),
            ("*", "*"),
            ("/", "/"),
            ("%", "%"),
            ("<", "<"),
            (">", ">"),
            ("&", "&"),
            ("|", "|"),
            ("^", "^"),
        ];
        let rest = self.source.as_bytes().get(at..)?;
        for &(token, member) in TABLE {
            if rest.starts_with(token.as_bytes()) {
                return Some(self.intern(member));
            }
        }
        None
    }

    /// The operator member at the first non-whitespace byte from `from`.
    fn binary_operator_after(&self, from: usize) -> Option<StringId> {
        let bytes = self.source.as_bytes();
        let mut at = from;
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        self.binary_operator_at(at)
    }

    /// Whether the index application opened at `open` (the `[` byte) is a
    /// WRITE (`a[i] = v`): the matching `]` is followed by a lone `=`
    /// (`==` compares the read, `=>` belongs to a closure body).
    fn index_write_after(&self, open: usize) -> bool {
        let bytes = self.source.as_bytes();
        let mut depth: i32 = 0;
        let mut i = open;
        while i < bytes.len() {
            match bytes[i] {
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        let mut k = i + 1;
                        while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                            k += 1;
                        }
                        return bytes.get(k) == Some(&b'=')
                            && bytes.get(k + 1) != Some(&b'=')
                            && bytes.get(k + 1) != Some(&b'>');
                    }
                }
                _ => {}
            }
            i += 1;
        }
        false
    }

    /// Whether the index expression whose `[` begins at byte `open` is
    /// followed by a COMPOUND write — `a[i] ??= v`, `a[i] += v`,
    /// `a[i]++`, `a[i]--`, or any `op=` compound. Upstream's
    /// `IndexExpression.writeOrReadElement` resolves `[]=` for every
    /// assignment form (the read element applies only through the
    /// compound's own read half — `unused_local_elements_verifier.dart`
    /// :196-200 credits the single element the write resolved), so the
    /// credit answer is the same `[]=` the lone `=` takes.
    fn index_compound_after(&self, open: usize) -> bool {
        let bytes = self.source.as_bytes();
        let mut depth: i32 = 0;
        let mut i = open;
        while i < bytes.len() {
            match bytes[i] {
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        let mut k = i + 1;
                        while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                            k += 1;
                        }
                        let tail = &bytes[k..];
                        return tail.starts_with(b"??=")
                            || tail.starts_with(b"<<=")
                            || tail.starts_with(b">>>=")
                            || tail.starts_with(b">>=")
                            || tail.starts_with(b"+=")
                            || tail.starts_with(b"-=")
                            || tail.starts_with(b"*=")
                            || tail.starts_with(b"/=")
                            || tail.starts_with(b"%=")
                            || tail.starts_with(b"~/=")
                            || tail.starts_with(b"&=")
                            || tail.starts_with(b"|=")
                            || tail.starts_with(b"^=")
                            || tail.starts_with(b"++")
                            || tail.starts_with(b"--");
                    }
                }
                _ => {}
            }
            i += 1;
        }
        false
    }

    /// Whether the source spells a member access `head.member` — an
    /// optional balanced `<...>` instantiation may sit between the head
    /// and the dot (the generic instantiation `foo<int>.m()`), and both
    /// spellings are word-bounded (an identifier that merely CONTAINS the
    /// head's spelling never matches). The byte walk is the same
    /// finalize-time posture the closure-chain arm rides.
    fn source_member_access(&self, head: &str, member: &str) -> bool {
        if head.is_empty() || member.is_empty() {
            return false;
        }
        let bytes = self.source.as_bytes();
        let ident_byte = |b: u8| matches!(b, b'_' | b'$' | b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z');
        let mut from = 0usize;
        while let Some(rel) = self.source[from..].find(head) {
            let hit = from + rel;
            let head_start_ok = hit == 0 || !ident_byte(bytes[hit - 1]);
            let mut k = hit + head.len();
            let head_end_ok = bytes.get(k).is_none_or(|b| !ident_byte(*b));
            if head_start_ok && head_end_ok {
                while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                    k += 1;
                }
                if bytes.get(k) == Some(&b'<') {
                    if let Some(end) = self.skip_balanced(k, b'<', b'>') {
                        k = end;
                    } else {
                        k = bytes.len();
                    }
                }
                while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                    k += 1;
                }
                if bytes.get(k) == Some(&b'.') {
                    let mut m = k + 1;
                    while m < bytes.len() && bytes[m].is_ascii_whitespace() {
                        m += 1;
                    }
                    if bytes[m..].starts_with(member.as_bytes()) {
                        let tail = m + member.len();
                        if bytes.get(tail).is_none_or(|b| !ident_byte(*b)) {
                            return true;
                        }
                    }
                }
            }
            from = hit + head.len();
        }
        false
    }

    /// Forward balanced-bracket walk from `open` (a `begin` byte); answers
    /// the byte AFTER the matching `end`, or `None` when unbalanced.
    fn skip_balanced(&self, open: usize, begin: u8, end: u8) -> Option<usize> {
        let bytes = self.source.as_bytes();
        let mut depth: i32 = 0;
        let mut i = open;
        while i < bytes.len() {
            if bytes[i] == begin {
                depth += 1;
            } else if bytes[i] == end {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            i += 1;
        }
        None
    }

    /// Whether the identifier at `[offset, end)` sits in a PURE-WRITE
    /// position: the next token is a lone `=` (`==` compares, `=>` belongs
    /// to a closure body, `+=`-family reads too — every compound and
    /// null-aware form keeps the read credit, matching the upstream
    /// write-credits-only-the-setter posture).
    fn pure_write_at(&self, offset: u32, end: u32) -> bool {
        let bytes = self.source.as_bytes();
        let stop = self.byte_of(end).min(bytes.len());
        let mut k = stop;
        while k < bytes.len() && bytes[k].is_ascii_whitespace() {
            k += 1;
        }
        bytes.get(k) == Some(&b'=') && bytes.get(k + 1).is_none_or(|b| *b != b'=' && *b != b'>')
    }

    /// Whether the source spells an object pattern's field reading
    /// `member` off `container` — `container(_: sub)` or
    /// `container<T>(_: sub)` (an object pattern REQUIRES the type
    /// spelling, so the container-anchored needle is the shape itself;
    /// `visitPatternField`'s `addMember`/`addReadMember`,
    /// unused_local_elements_verifier.dart:249-253). Positional fields
    /// (`container(: _)`) match by position, not spelling — a face the
    /// byte walk cannot key — so they keep the report.
    fn pattern_field_read_of(&self, container: StringId, member: StringId) -> bool {
        let container_text = self.strings.resolve(container);
        let member_text = self.strings.resolve(member);
        if container_text.is_empty() || member_text.is_empty() {
            return false;
        }
        let bytes = self.source.as_bytes();
        let ident_byte = |b: u8| matches!(b, b'_' | b'$' | b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z');
        let mut from = 0usize;
        while let Some(rel) = self.source[from..].find(&container_text) {
            let hit = from + rel;
            let bounded = (hit == 0 || !ident_byte(bytes[hit - 1]))
                && bytes
                    .get(hit + container_text.len())
                    .is_none_or(|b| !ident_byte(*b));
            if bounded {
                let mut k = hit + container_text.len();
                while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                    k += 1;
                }
                if bytes.get(k) == Some(&b'<') {
                    k = self.skip_balanced(k, b'<', b'>').unwrap_or(bytes.len());
                }
                while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                    k += 1;
                }
                if bytes.get(k) == Some(&b'(') {
                    let mut m = k + 1;
                    while m < bytes.len() && bytes[m].is_ascii_whitespace() {
                        m += 1;
                    }
                    if bytes[m..].starts_with(member_text.as_bytes()) {
                        let mut e = m + member_text.len();
                        while e < bytes.len() && bytes[e].is_ascii_whitespace() {
                            e += 1;
                        }
                        if bytes.get(e) == Some(&b':') {
                            return true;
                        }
                    }
                }
            }
            from = hit + container_text.len().max(1);
        }
        false
    }

    /// One self-use re-attribution: the container spelling keys the
    /// same-file face(s), and the member must sit on the face under its
    /// written or bare spelling. The credits land on the derived set the
    /// caller folds into [`Engine::extension_member_used`].
    fn note_extension_self_use(
        &self,
        face_keys: &FxHashMap<StringId, Vec<u32>>,
        derived: &mut FxHashSet<(FileId, u32, StringId)>,
        container: StringId,
        member: StringId,
    ) {
        let Some(indices) = face_keys.get(&container) else {
            return;
        };
        let Some(elements) = self.index.get(self.file) else {
            return;
        };
        let faces = elements.extension_faces();
        let bare = self.bare_name(member);
        for index in indices {
            let Some(face) = faces.get(*index as usize) else {
                continue;
            };
            if face.members.contains_key(&member) || face.members.contains_key(&bare) {
                derived.insert((self.file, *index, member));
                derived.insert((self.file, *index, bare));
            }
        }
    }

    /// The usage cluster's library-level pass, after every use is indexed:
    /// the declaration sites of this file walk against the use index (the
    /// official `UnusedLocalElementsVerifier`), then the import directives
    /// against the contributed-element index (`imports_verifier.dart`).
    /// Each arm's gate mirrors the official `_isUsed*` split; every
    /// unmodelable corner suppresses (the under-report direction).
    pub(crate) fn finalize_usage(&mut self) {
        self.credit_derived_extension_uses();
        // The #3479 write-position correction: the read pools the member
        // verdicts join (`member_read_owners`, `read_names`,
        // `field_read_rows`'s bare credits) carried three write shapes the
        // oracle's `_isReadIdentifier` never records — a member access the
        // expression statement discarded (`a._f += 1;`, `a._f++;`), a
        // constructor initializer's LHS write (`A() : _f = 0`), and an
        // initializing formal's field-typed read (`C({this._x}) : _y =
        // _x` binds the FORMAL, never the field). Strip the credit before
        // the decl-site walk reads the pools.
        self.discard_write_position_reads();
        // The super-initialization pass: the invocation givens land in the
        // given sets the parameter face reads, and the resolved-target map
        // feeds the constructor corresponding-parameter face.
        let super_ctor_targets = self.walk_super_invocation_givens();
        // The non-call read set the parameter face's all-formals arm
        // reads: `this.sel(...)` redirecting-initializer targets joined
        // with the bare `C.sel` / `C.new` / `C<T>.sel` / `@C.sel` tear-off
        // reads (both resolved `(container, selector)` pairs — upstream's
        // `visitSimpleIdentifier` non-call arm credits the whole formal
        // list either way).
        let mut noncall_reads = self.walk_this_redirect_targets();
        noncall_reads.extend(self.walk_ctor_tearoffs());
        // The element-attributed use pool: `used_names` minus the member
        // spellings that only an unresolved access contributed (the
        // `Receiver::Unknown`/`Prefix` rows — an `InvalidType` or `dynamic`
        // receiver's `.m` resolves no element, so the oracle's
        // `members`/`elements` never carry it). A member spelling that is
        // also a non-member resolved use (a bare `RefTarget::Decl`/`Member`
        // reference, a type-use position) stays — it resolves through a
        // path that is not the member access.
        // Every `.member` access name — `member_reads` covers read intents;
        // `member_refs` covers resolved invokes and writes the read index
        // never records (an `Invoke` is not a field read). Both pools are
        // needed: without `member_refs` an invoked-only name stays in
        // `element_used` and over-credits same-spelling siblings.
        let member_accessed: FxHashSet<StringId> = self
            .member_reads
            .iter()
            .map(|read| read.name)
            .chain(self.report.member_refs.iter().map(|r| r.target_name))
            .collect();
        let resolved = self.resolved_member_uses();
        let non_member_resolved: FxHashSet<StringId> = self
            .refs
            .iter()
            .filter(|r| {
                matches!(
                    r.target,
                    RefTarget::Decl { .. } | RefTarget::Member { .. } | RefTarget::TypeParam
                )
            })
            .map(|r| r.name)
            .chain(self.type_uses.iter().map(|(_, name)| *name))
            .collect();
        let unresolved_member_only: FxHashSet<StringId> = member_accessed
            .difference(&resolved)
            .filter(|name| !non_member_resolved.contains(name))
            .copied()
            .collect();
        let element_used: FxHashSet<StringId> = self
            .used_names
            .iter()
            .filter(|name| !unresolved_member_only.contains(*name))
            // An unresolved `import ''` head credits the synthetic empty
            // spelling — a name-level `element_used` hit would spare every
            // nameless decl site in the file. The nameless recovery has no
            // use edge; scrub the mark.
            .filter(|name| !self.is_nameless(**name))
            .copied()
            .collect();
        // The oracle's `usedElements.members` element set at (file,
        // container, spelling) granularity (#2618's `member_refs` edge):
        // a resolved member access credits the exact element it bound,
        // never a same-spelling member of a sibling class. The Member arm's
        // `_isUsedMember` check reads this set — `element_used`'s name-level
        // set would spare `A._m` from `B._m`'s use.
        let member_used_elements: FxHashSet<(FileId, StringId, StringId)> = self
            .report
            .member_refs
            .iter()
            .map(|r| (r.target_file, r.target_container, r.target_name))
            .collect();
        // The implicit-this setter face (upstream `preferSetter`,
        // lexical_lookup.dart:22-35): a bare `_s = 1` resolves
        // `RefTarget::Missing` at push time (the plain-name `_s` is not in
        // `A.members` — only `_s=` is), and `bare_write_target_exists`
        // rescues the write silently without a `member_refs` edge. The
        // write still credits the setter element — the `refs` pool's
        // `Missing` arm re-derives it (the row stack is drained by
        // finalize): a `RefTarget::Missing` entry in write position
        // (`pure_write_at`) re-derives the `this_type` the drained row
        // carried through `enclosing_container_at`, and the setter's
        // `name=` spelling is the `member_found` probe.
        let missing_write_candidates: Vec<(Option<StringId>, StringId)> = self
            .refs
            .iter()
            .filter(|r| {
                matches!(r.target, RefTarget::Missing)
                    && self.pure_write_at(r.range.offset, r.range.end())
            })
            .map(|r| {
                let container = self.enclosing_container_at(r.range.offset);
                (container, r.name)
            })
            .collect();
        let mut missing_write_setters: FxHashSet<(FileId, StringId, StringId)> =
            FxHashSet::default();
        let mut missing_write_top_levels: FxHashSet<StringId> = FxHashSet::default();
        for (this_type, name) in missing_write_candidates {
            let setter = self.setter_spelling(name);
            match this_type {
                Some(container)
                    if self
                        .container_kind_of(container)
                        .is_some_and(|kind| kind != ContainerKind::Extension)
                        && self.member_found(container, setter, false) =>
                {
                    missing_write_setters.insert((self.file, container, setter));
                }
                // A top-level write's `this_type` is `None` — the setter's
                // `name=` spelling resolves through the scope lookup
                // (`resolve_bare` at push time already tried it and missed:
                // `BareInfo::Missing` is the push-time result, not the
                // write-upgraded second chance `bare_write_target_exists`
                // ran). A `RefTarget::Decl` hit means the top-level setter
                // exists and the write used it. An extension body's
                // `this_type` is `None` upstream (the `on` face carries
                // the write), so it falls here too — the extension's
                // `on` type setter resolves through the scope lookup the
                // same way a top-level's does.
                _ => {
                    if matches!(self.resolve_bare(setter).1, RefTarget::Decl { .. }) {
                        missing_write_top_levels.insert(name);
                    }
                }
            }
        }
        // `element_used`'s Member-arm view: names that resolved ONLY through
        // a `.member` access (no bare `Decl`/`Member`/`TypeParam` ref, no
        // type-use) are the `member_refs` set's domain — a `_B._m` use must
        // not spare `_A._m`. `element_used` minus that subset keeps the
        // non-member paths (a bare `_foo = 0` inside `_A`, a type-use) so
        // the arm's fallback credits only same-class non-`.`-member uses.
        let member_only_resolved: FxHashSet<StringId> = member_accessed
            .intersection(&resolved)
            .filter(|name| !non_member_resolved.contains(name))
            .copied()
            .collect();
        // The setter-twin read face: `C()._p` on a class declaring ONLY
        // `set _p` resolves no getter (`undefined_getter` reports), but
        // the oracle's `usedElements.members` still carries the SETTER
        // element — `visitSimpleIdentifier`'s `node.element` binds the
        // induced property's setter member when the getter face misses
        // (probed: `set _p` + `C()._p` read spares the setter). The read
        // reaches `member_reads` with no `member_refs` edge (nothing
        // resolved), so the same receiver walk `resolved_member_uses`
        // applies re-decides the container, and a plain-member miss paired
        // with a `name=` hit credits the setter element — the
        // getter+setter pair never lands here (the getter resolves, no
        // `member_found` miss).
        let read_containers: Vec<(StringId, StringId, bool)> = self
            .member_reads
            .iter()
            .filter_map(|read| {
                let recv = self.promoted_receiver(read.offset, read.recv);
                match recv {
                    Receiver::This { container, .. }
                    | Receiver::Typed {
                        type_name: container,
                        ..
                    }
                    | Receiver::Created {
                        name: container, ..
                    }
                    | Receiver::Container {
                        name: container, ..
                    } => Some((container, read.name, false)),
                    Receiver::Super { container, .. } => Some((container, read.name, true)),
                    _ => None,
                }
            })
            .collect();
        let mut setter_twin_uses: FxHashSet<(FileId, StringId, StringId)> = FxHashSet::default();
        for (container, name, super_walk) in read_containers {
            if !self.member_found(container, name, super_walk) {
                let setter = self.setter_spelling(name);
                if setter != name && self.member_found(container, setter, super_walk) {
                    setter_twin_uses.insert((self.file, container, setter));
                }
            }
        }
        // The `missing_write_setters` and `setter_twin_uses` rows join the
        // element set — the `member_refs` edge missed them (the push-time
        // `RefTarget::Missing` never reached `note_member_ref`; the
        // setter-only read resolved no getter), but the oracle's
        // `usedElements.members` carries the setter element.
        let member_used_elements: FxHashSet<(FileId, StringId, StringId)> = member_used_elements
            .union(&missing_write_setters)
            .copied()
            .chain(setter_twin_uses.iter().copied())
            .collect();
        // `element_used`'s Member-arm view: names that resolved ONLY through
        // a `RefTarget::Member` ref (a bare `_m1` inside `B`'s body binds
        // `B._m1`, never `A._m1`) are element-attributed to their own
        // container via `member_used_elements` — keeping the spelling in
        // the name-level pool would spare every same-named member of a
        // sibling class (#3478). A `Decl`/`TypeParam`/`type_use` path on the
        // same spelling keeps it (a non-member resolution also credits the
        // name).
        let member_resolved_names: FxHashSet<StringId> = self
            .refs
            .iter()
            .filter(|r| matches!(r.target, RefTarget::Member { .. }))
            .map(|r| r.name)
            .collect();
        let non_member_paths: FxHashSet<StringId> = self
            .refs
            .iter()
            .filter(|r| matches!(r.target, RefTarget::Decl { .. } | RefTarget::TypeParam))
            .map(|r| r.name)
            .chain(self.type_uses.iter().map(|(_, name)| *name))
            .collect();
        let member_only_resolved_names: FxHashSet<StringId> = member_resolved_names
            .difference(&non_member_paths)
            .copied()
            .collect();
        // The shadowed-local arm (#3478): a `_f` spelling that resolved
        // ONLY through `RefTarget::Local`/`Missing`/`Opaque`/`Prefix` —
        // never `Member`/`Decl`/`TypeParam`/`type_use` — is the local's
        // credit, never the field's (`this._f`'s). `element_used` carries
        // it because `used_names` marked the spelling at read time, but
        // the member-level fallback must not let a parameter or for-in
        // binder's read spare the same-named field. Names absent from
        // `refs` entirely (credited through `read_names`/`import_used`/
        // `extension_credit_names`) keep the credit — the filter only
        // strips names the `refs` pool proves are local-bound.
        let local_only_names: FxHashSet<StringId> = {
            let resolved_paths: FxHashSet<StringId> = member_resolved_names
                .iter()
                .chain(&non_member_paths)
                .copied()
                .collect();
            let referenced: FxHashSet<StringId> = self.refs.iter().map(|r| r.name).collect();
            referenced.difference(&resolved_paths).copied().collect()
        };
        // The strict subset of `local_only_names` that resolved
        // `RefTarget::Local` — the proven local read a parameter or
        // binder owns. The `unresolved_read` member face excludes only
        // this arm: a `Missing`/`Opaque`/`Prefix` miss stays unresolved
        // (the `.member` resolution genuinely bound nothing), while a
        // `Local` hit means the bare name belongs to a variable the
        // field's credit must not claim.
        let local_resolved_names: FxHashSet<StringId> = self
            .refs
            .iter()
            .filter(|r| matches!(r.target, RefTarget::Local))
            .map(|r| r.name)
            .collect();
        let member_element_used: FxHashSet<StringId> = element_used
            .difference(&member_only_resolved)
            .filter(|name| !member_only_resolved_names.contains(name))
            .filter(|name| !local_only_names.contains(name))
            .copied()
            .collect();
        // `_isUsedElement` for top-level declarations is element-level: a
        // local `_f` shadows the top-level `_f`, and the local's use credits
        // `LocalFunctionElement`, never the top-level `TopLevelFunctionElement`.
        // `element_used`'s name-level set would spare the top-level decl
        // because both share the `_f` spelling. Restrict the check to names
        // that resolved to a top-level declaration (`RefTarget::Decl`) or a
        // type-annotation position — the same paths the upstream element
        // check resolves through.
        //
        // `_isReadIdentifier` (upstream :429): a write `v = 3` adds the
        // SETTER element, never the variable — `v` reports `unused_element`
        // unless a read also credits it. `rows` records the finalized write,
        // and the refs pool carries the resolution of every surviving
        // identifier — the source bytes decide the intent (`v = 3` is a
        // pure write; `v == 3` compares and `v += 1` reads too, so both
        // keep the credit; `=>` belongs to a closure body).
        // `_isReadIdentifier` (upstream :429): a write `v = 3` adds the
        // SETTER element, never the variable — `v` reports `unused_element`
        // unless a read also credits it. The row stack is drained by
        // finalize, so the write offsets re-derive from the surviving
        // pools: `refs` `pure_write_at` probes (a `v = 3` write) joined
        // with `member_refs` `Written` edges (the `_f = 0` initializer
        // LHS, a bare `this`-implicit assignment).
        let write_offsets: FxHashSet<u32> = self
            .refs
            .iter()
            .filter(|r| self.pure_write_at(r.range.offset, r.range.end()))
            .map(|r| r.range.offset)
            .chain(
                self.report
                    .member_refs
                    .iter()
                    .filter(|edge| matches!(edge.kind, MemberRefKind::Written))
                    .map(|edge| edge.range.offset),
            )
            .collect();
        let pure_write_refs: FxHashSet<u32> = self
            .refs
            .iter()
            .filter(|r| matches!(r.target, RefTarget::Decl { .. } | RefTarget::Missing))
            .filter(|r| self.pure_write_at(r.range.offset, r.range.end()))
            .map(|r| r.range.offset)
            .collect();
        let top_level_used: FxHashSet<StringId> = self
            .refs
            .iter()
            .filter(|r| {
                matches!(r.target, RefTarget::Decl { .. })
                    && !write_offsets.contains(&r.range.offset)
                    && !pure_write_refs.contains(&r.range.offset)
                    && !self.statement_discarded_rows.contains(&r.range.offset)
            })
            .map(|r| r.name)
            .chain(self.type_uses.iter().map(|(_, name)| *name))
            .chain(missing_write_top_levels.iter().copied())
            .collect();
        // The resolved member reads the refs pool already carries: the
        // `member_refs` edge pool misses the access shapes whose credit the
        // gather visitor still records — an object pattern's field read
        // (`visitPatternField`'s `addMember`/`addReadMember`,
        // unused_local_elements_verifier.dart:249-253) among them. Each
        // entry keys the same (container, spelling) element tuple the
        // member arms read, so those paths gain exactly the upstream
        // credit and nothing more (a write intent stays excluded).
        // The write re-decision (#3479): the row stack is drained by
        // finalize, so a `RefTarget::Member` ref's write form re-derives
        // from its `member_refs` edge — `Written` is a pure write (the
        // `_f = 0` constructor-initializer LHS, a bare `this`-implicit
        // assignment) the read join excludes, while `ReadWritten` is a
        // real compound read the oracle credits.
        let written_ref_offsets: FxHashSet<u32> = self
            .report
            .member_refs
            .iter()
            .filter(|edge| matches!(edge.kind, MemberRefKind::Written))
            .map(|edge| edge.range.offset)
            .collect();
        let ref_member_reads: FxHashSet<(StringId, StringId)> = self
            .refs
            .iter()
            .filter_map(|r| match &r.target {
                RefTarget::Member { container }
                    if !write_offsets.contains(&r.range.offset)
                        && !pure_write_refs.contains(&r.range.offset)
                        && !written_ref_offsets.contains(&r.range.offset)
                        && !self.pure_write_at(r.range.offset, r.range.end())
                        && !self.statement_discarded_rows.contains(&r.range.offset) =>
                {
                    Some((*container, r.name))
                }
                _ => None,
            })
            .collect();
        // The write-side setter credit: a write whose resolution binds the
        // setter spelling credits the SETTER element
        // (`_useIdentifierElement(node.writeElement)`,
        // unused_local_elements_verifier.dart:304-308) — `usedElements
        // .elements` carries the accessor, and `_isUsedMember` spares it.
        // The push-time row keeps the bare spelling and its write intent,
        // so the name-level read set never sees it; re-resolve the setter
        // spelling live for every Decl- or Missing-bound write (a bare
        // setter write's plain-name lookup misses — only `_s=` resolves —
        // so its ref lands `Missing` and the live probe recovers it). A
        // variable element is never credited by a write — the added
        // element is the accessor — so only setter sites read this set.
        let decl_write_names: FxHashSet<StringId> = self
            .refs
            .iter()
            .filter(|r| {
                matches!(r.target, RefTarget::Decl { .. } | RefTarget::Missing)
                    && (write_offsets.contains(&r.range.offset)
                        || pure_write_refs.contains(&r.range.offset))
            })
            .map(|r| r.name)
            .collect();
        // (`resolve_bare` takes `&mut self`, so the probe runs after the
        // `refs` borrow ends.)
        let top_level_setter_writes: FxHashSet<StringId> = decl_write_names
            .clone()
            .into_iter()
            .filter(|&name| {
                let spelling = self.setter_spelling(name);
                matches!(self.resolve_bare(spelling).1, RefTarget::Decl { .. })
            })
            .collect();
        // The scope's first-wins slot: upstream's library scope binds each
        // getter/setter spelling with `??=` (scope.dart:880-892), so a
        // later same-spelling top-level declaration is shadowed — no name
        // lookup reaches its element, the element never joins
        // `usedElements.elements`, and
        // `_visitTopLevelVariableElement` reports it whenever it is
        // private (the shadowed top-level `_` face, #3241). The name-level
        // `top_level_used` set would mask the twin behind the winning
        // spelling's use, so every later claimant skips that credit.
        let mut top_level_first_claims: FxHashSet<StringId> = FxHashSet::default();
        // The dot-shorthand constructor invocation (`._(...)` under a
        // written-type declaration): the parser folds the selector to a
        // bare name and drops the `.` (the const capture's fold — the
        // root node's span starts at the selector token), so no
        // creation-head event routes `credit_constructor_selector` and
        // the constructor reads unused. The oracle resolves the
        // shorthand against the declaration's context type and credits
        // that constructor element (upstream Dart SDK source,
        // pkg/analyzer/lib/src/dart/constant/evaluation.dart
        // visitDotShorthandConstructorInvocation2:880-916). The fold's
        // exact shape is re-derived here: a read row spelled at a
        // leading-dot position ([`Engine::dot_shorthand_context`]) whose
        // next token opens the argument group — the invocation face, not
        // a plain wildcard read (`f(_)` closes on `)`) and not a
        // receiver-qualified access (`x._()` keeps its receiver bytes).
        // The credit is spelling-scoped, not element-scoped: the
        // shorthand's context type is the declaration's written type,
        // which the finalize pass does not model — a same-spelling
        // selector on a sibling class shares the credit (the
        // under-report direction every spelling-level corner keeps).
        let shorthand_ctor_refs: FxHashSet<StringId> = self
            .refs
            .iter()
            .filter(|r| {
                self.dot_shorthand_context(r.range.offset)
                    && !self.byte_before_dot_is_question(r.range.offset)
                    && self.byte_at(r.range.end()) == Some(b'(')
            })
            .map(|r| r.name)
            .collect();
        // The public-typedef exposure face (`visitGenericTypeAlias`,
        // unused_local_elements_verifier.dart:175-190): a non-private
        // `typedef Public = _Private` adds every NON-private constructor
        // of the aliased interface type to `usedElements.elements` —
        // `_PrivateClass.named` is reachable through the alias even though
        // no `named(` call exists anywhere. The resolution is type-level
        // (`node.type.type` — a `typedef AlsoPublic = _PrivateAlias`
        // indirection resolves through the middle alias to the same
        // element), so the name model walks the alias→head edges to a
        // fixed point before resolving the leaf as a container. A
        // private-named alias contributes nothing itself but still
        // forwards on the chain (the oracle reads the resolved type, not
        // the spelling). The element-tuple set matches
        // `member_used_elements`' keying so the Constructor arm can read
        // it directly — the exposure spares only the resolved file's own
        // constructors.
        let alias_heads: FxHashMap<StringId, (StringId, Option<StringId>)> = self
            .decl_sites
            .iter()
            .filter(|site| matches!(site.kind, DeclKind::TypeAlias))
            .filter_map(|site| self.typedef_rhs_head(site).map(|rhs| (site.name, rhs)))
            .collect();
        let mut alias_used_ctors: FxHashSet<(FileId, StringId, StringId)> =
            FxHashSet::default();
        for site in &self.decl_sites {
            if !matches!(site.kind, DeclKind::TypeAlias) || self.is_private_name(site.name) {
                continue;
            }
            // The alias's own RHS opens the walk; each hop follows a
            // same-file alias's RHS head (`A = B`, `B = _C`) until the
            // head names no alias — a `prefix.Leaf` arm or an unresolvable
            // name ends the chain there. Cycle-guarded: typedef cycles
            // are an error upstream, but the walk terminates regardless.
            let Some(&init) = alias_heads.get(&site.name) else {
                continue;
            };
            let mut rhs = init;
            let mut seen: FxHashSet<StringId> = FxHashSet::default();
            while rhs.1.is_none() && seen.insert(rhs.0) {
                let Some(&next) = alias_heads.get(&rhs.0) else {
                    break;
                };
                rhs = next;
            }
            let (leaf, prefix) = rhs;
            let target = match prefix {
                Some(p) => prefixed_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    p,
                    leaf,
                ),
                None => resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    leaf,
                ),
            };
            let Some((container, container_file)) = target else {
                continue;
            };
            for &ctor in container.constructors.iter() {
                if !self.is_private_name(ctor) {
                    alias_used_ctors.insert((container_file, leaf, ctor));
                }
            }
        }
        // The augment-fragment skip: upstream's `_reportDiagnosticForElement`
        // anchors every `unused_element`/`unused_element_parameter`/`unused_field`
        // row at `element.firstFragment.nameOffset` — the introductory
        // fragment's position (unused_local_elements_verifier.dart:1051-1067).
        // An `augment` declaration's element merges into the same element the
        // introductory declaration declares, so the merged verdict reports
        // once — at the intro site — and never at the augmenting fragment's
        // own offset. The `augmentations()` model row keys every `augment`
        // fragment (container, top-level, member, constructor alike) on the
        // name token's span — the same anchor `DeclSite::range` stores —
        // letting this gate read membership without a new listener latch.
        // A member declared inside an `augment` container without its own
        // `augment` keyword is a NEW element (firstFragment IS the augment
        // fragment), so it joins no row and still reports at its own site.
        let augment_ranges: FxHashSet<(u32, u32)> = self
            .index
            .get(self.file)
            .map(|f| {
                f.augmentations()
                    .iter()
                    .map(|a| (a.name_range.offset, a.name_range.length))
                    .collect()
            })
            .unwrap_or_default();
        for site_index in 0..self.decl_sites.len() {
            let site = self.decl_sites[site_index].clone();
            // Skip both the element-face match below and the parameter-face
            // tail — an augment fragment's own formals never carry the row
            // (the introductory parameter's element owns the verdict and
            // reports at its own site through its own site entry).
            if site.is_augment
                || augment_ranges.contains(&(site.range.offset, site.range.length))
            {
                continue;
            }
            let bare = self.bare_name(site.name);
            // The nameless recovery face renders `<unnamed>` (`ElementImpl
            // .displayName`'s `name ?? '<unnamed>'`).
            let raw_text = self.strings.resolve(site.name);
            let name_text = if raw_text.is_empty() {
                "<unnamed>".to_string()
            } else {
                raw_text.to_string()
            };
            // The diagnostic's `{0}` is the element's `displayName`, whose
            // shape never carries the accessor suffix or the unnamed-head
            // selector: a setter's element name is the bare spelling (`_foo`,
            // not `_foo=`), and a written `.new` head renders as its
            // container alone (element.dart's `displayName`: `name == 'new'`
            // answers the enclosing class name).
            let display_text = match name_text.strip_suffix('=') {
                // Only identifier setter spellings lose the suffix — an
                // operator's `=` is its own name (`operator []=`'s element
                // displayName keeps `[]=` upstream; the setter-encoded
                // `_s=` folds to `_s`). `new` folds to the container's
                // spelling (element.dart's `displayName`).
                Some(bare)
                    if bare != "new"
                        && bare
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_') =>
                {
                    bare.to_string()
                }
                _ => name_text.clone(),
            };
            // The `strict-inference` return-type face
            // (`_checkStrictInferenceReturnType`,
            // best_practices_verifier.dart:1587-1606): a top-level function
            // or getter that declares no return type reports. Setters are
            // exempt (`!node.isSetter`, and the getter visitor names
            // `TopLevelGetterDeclaration` only); local functions are the
            // oracle's own open point (its visit skips them) and members stay
            // deferred — the override exemption needs the full supertype
            // walk M1's name edges cannot answer.
            if self.strict.inference
                && matches!(site.kind, DeclKind::TopLevelFn)
                && !name_text.ends_with('=')
                && self
                    .top_level_decl(site.name)
                    .is_some_and(|decl| decl.return_ty == Ty::Unknown)
            {
                self.stage_usage(
                    SemanticCode::InferenceFailureOnFunctionReturnType,
                    site.range,
                    &name_text,
                );
            }
            // The one annotation upstream honours on the element/member
            // faces is `@pragma('vm:entry-point')`
            // (`_hasPragmaVmEntryPoint`, unused_local_elements_verifier.dart
            // :1179-1181, read by `_isUsedElement` :971 and `_isUsedMember`
            // :981); `@JS` spares the CLASS element alone
            // (`visitClassDeclaration` :82-93 → `ElementMetadata.hasJS`,
            // element.dart:8572-8581). Every other annotation spares
            // nothing: the annotated-private corpus reports for
            // `@visibleForTesting`, the `@Deprecated.*` family and
            // `@Preview` alike, so a blanket "any annotation spares" gate
            // under-reports the whole `invalid_visibility_annotation` /
            // `invalid_deprecated_*` families. Both gates stay
            // spelling-level — the declaration site keeps the annotation
            // names, not their argument text — so a `@pragma('…')` of any
            // argument spares (the safe under-report direction).
            let pragma_spared = site.annotated
                && site
                    .annotations
                    .iter()
                    .any(|name| self.annotation_named(*name, "pragma"));
            match site.kind {
                DeclKind::TopLevelFn | DeclKind::TopLevelVar | DeclKind::TypeAlias => {
                    // A nameless recovery site never claims the slot
                    // (upstream's `_addGetter` skips a null lookupName
                    // entirely — scope.dart:880-885 — so nameless elements
                    // stay outside the first-wins map).
                    let shadowed =
                        !self.is_nameless(site.name) && !top_level_first_claims.insert(site.name);
                    // `_isUsedElement`'s else arm: a public name is used;
                    // otherwise a resolved top-level use spares it —
                    // `top_level_used` excludes locals, members, and
                    // parameters (element-level check, upstream
                    // `usedElements.elements`). A shadowed duplicate never
                    // takes that credit (its element is unreachable by
                    // name), and a written setter reads its own write
                    // credit set — the accessor is the credited element.
                    let is_setter_site = bare != site.name;
                    if !pragma_spared
                        && (self.is_private_name(site.name) || self.is_nameless(site.name))
                        && (shadowed || !top_level_used.contains(&bare))
                        && !(is_setter_site && top_level_setter_writes.contains(&bare))
                    {
                        // A nameless recovery declaration anchors at the file
                        // start — the spliced synthetic name never reaches
                        // the written source, so its splice offset is not
                        // the oracle's row anchor (`<unnamed>` at 1:1, the
                        // corpus nameless-declaration capture).
                        let range = if self.is_nameless(site.name) {
                            self.nameless_site_range()
                        } else {
                            site.range
                        };

                        self.stage_usage(SemanticCode::UnusedElement, range, &display_text);
                    }
                }
                DeclKind::Container => {
                    // `_visitClassElement` (classes, enums, mixins, and
                    // extension types; extensions are never visited): the
                    // same public/used gate. A NAMELESS recovery container
                    // never reports: the class/mixin/enum/extension-type
                    // element answers `isPrivate` through its fragment
                    // (`InstanceElementImpl.isPrivate` →
                    // `_firstFragment.isPrivate`), and
                    // `FragmentImpl.isPrivate` maps a null name to PUBLIC
                    // (element.dart:4539-4544, upstream Dart SDK source) —
                    // the parser-recovery `<unnamed>` face is exported and
                    // exempt (the fragment_offset `_missingName` corpus).
                    // The opposite null-name reading belongs to
                    // `ElementImpl` (top-level fns/vars/type aliases, the
                    // arm above) and to extension fragments; neither is a
                    // `Container` site.
                    //
                    // The enum-constructor self-reference leak: upstream
                    // `visitClassDeclaration` alone sets `_enclosingClass`
                    // (unused_local_elements_verifier.dart:94) — the enum,
                    // mixin, and extension-type visitors never do — so a
                    // constructor declaration's own `returnType` NamedType
                    // (`const _E(this._f)`'s `_E`) visits
                    // `visitNamedType` → `_useIdentifierElement` with
                    // `_enclosingClass == null`, and
                    // `identical(element, _enclosingClass)` fails open:
                    // the enum element joins `usedElements.elements`
                    // (:243-246, :396-422). Only the ENUM surface shows
                    // the leak — a mixin's constructors are a syntax error
                    // (`mixin_declares_constructor`), and an extension
                    // type's representation declaration never repeats the
                    // head as a `NamedType` child — so an enum that
                    // declares any constructor (named or unnamed) reads
                    // used; an enum with constants but no constructors
                    // still reports (the constant's implicit invocation
                    // resolves the SYNTHETIC constructor, which declares
                    // no `NamedType` — probed: `a(0)` over a
                    // constructor-less enum reports `_E`, over a declared
                    // `const _E(int i)` does not).
                    let enum_declares_ctor = self
                        .container_kind_of(site.name)
                        .is_some_and(|kind| kind == ContainerKind::Enum)
                        && self.decl_sites.iter().any(|s| {
                            matches!(
                                s.kind,
                                DeclKind::Constructor { container: c } if c == site.name
                            )
                        });
                    // The `@JS` metadata gate spares the CLASS element
                    // (`visitClassDeclaration` adds it to
                    // `usedElements.elements`, unused_local_elements_verifier
                    // .dart:82-93 → `ElementMetadata.hasJS`,
                    // element.dart:8572-8581; the `package:js` and
                    // `dart:js_interop` `JS` constructors). An enum, mixin or
                    // extension type carries no such gate upstream.
                    let js_spared = site.annotated
                        && site
                            .annotations
                            .iter()
                            .any(|name| self.annotation_named(*name, "JS"))
                        && self
                            .container_kind_of(site.name)
                            .is_some_and(|kind| kind == ContainerKind::Class);
                    // The mixin-application `_` spare (#3738): upstream
                    // reports a `class _ {}` wildcard container but never a
                    // `class _ = S with M` application — the synthetic
                    // application's element binds the `with` clause's
                    // forwarding constructors into `usedElements.elements`
                    // (`addElement`'s mixin-application super-constructor
                    // arm), which the element check spares. The `=` after
                    // the name token is the application's written shape.
                    let mixin_app = {
                        let mut at = self.byte_of(site.range.end());
                        let bytes = self.source.as_bytes();
                        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
                            at += 1;
                        }
                        bytes.get(at) == Some(&b'=')
                    };
                    if !js_spared
                        && !pragma_spared
                        && !mixin_app
                        && self.is_private_name(site.name)
                        && !element_used.contains(&bare)
                        && !enum_declares_ctor
                    {
                        self.stage_usage(SemanticCode::UnusedElement, site.range, &display_text);
                    }
                }
                DeclKind::Member {
                    kind: MemberKind::Field,
                    is_static,
                    container,
                } => {
                    // `_isReadMember`: a public field of a public container
                    // is used; a public static field of a private container
                    // is checked like a private one. Used only when READ
                    // (the implicit getter enters the read set; writes never
                    // spare it), or when a super-name it overrides is read.
                    // The read credit is element-scoped: the receiver
                    // handles joined against this container's supertype
                    // closure (`Sub`'s read of an inherited `_f` counts;
                    // a sibling class's same-spelled field does not).
                    // No annotation spares a field: `_visitFieldElement`
                    // routes through `_isReadMember`
                    // (unused_local_elements_verifier.dart:1030-1055), which
                    // consults the read sets alone — neither the `@JS` gate
                    // nor `_hasPragmaVmEntryPoint` is reachable from it
                    // (probed: a `@pragma('vm:entry-point')` field still
                    // reports).
                    let eligible = self.is_private_name(site.name)
                        // A nameless recovery field maps a null name to
                        // PRIVATE upstream (`ElementImpl.isPrivate`'s
                        // `name == null → true`, element.dart:2179) — the
                        // same gate the top-level arm applies, so the
                        // `<unnamed>` member reports like a private field
                        // rather than reading public.
                        || self.is_nameless(site.name)
                        || (self.is_private_name(container) && is_static);
                    if !eligible {
                        continue;
                    }
                    // `_recordEnumValuesUse`
                    // (unused_local_elements_verifier.dart:442-455): reading
                    // an enum's `values` getter credits every constant's
                    // getter at once — `E.values` and any `E`-typed
                    // receiver's `.values` read both spare the whole
                    // constant set (the element.dart corpus family).
                    let enum_values_read = self
                        .index
                        .get(self.file)
                        .and_then(|f| f.container(container))
                        .is_some_and(|c| c.enum_constants.contains(&site.name))
                        && self.member_was_read(container, &self.intern("values"));
                    // `unresolvedReadMembers`
                    // (unused_local_elements_verifier.dart:1030-1055): a
                    // read-shaped access whose resolution bound no element
                    // (`f._x` on a `dynamic`/invalid receiver) still spares
                    // the field by spelling — `read_names` carries the
                    // spelling for every read row, while a resolved read
                    // also records its owner; a spelling that never
                    // resolved an owner is the oracle's unresolved set.
                    // A name that resolved through a non-member path (a
                    // bare `_f` inside `B` resolves `B._f` via
                    // `RefTarget::Member`) is NOT unresolved — its owner
                    // resolution lives in `refs`, not `member_read_owners`.
                    // A LOCAL resolution is not unresolved either — a
                    // parameter or for-in binder's `_f` read binds the
                    // local, not the field (#3478: the member-level
                    // unresolved set is `.member` accesses only, upstream
                    // `visitPrefixedIdentifier`/`visitPropertyAccess` —
                    // a bare local read never lands there).
                    let unresolved_read = self.read_names.contains(&bare)
                        && !non_member_resolved.contains(&bare)
                        && !local_resolved_names.contains(&bare)
                        && self
                            .member_read_owners
                            .get(&bare)
                            .is_none_or(|owners| owners.is_empty());
                    if self.member_was_read(container, &bare)
                        || enum_values_read
                        || unresolved_read
                        || ref_member_reads.contains(&(container, bare))
                        || ref_member_reads.contains(&(container, site.name))
                        // The object-pattern field read (`C(_: 42)`): the
                        // resolve layer records the spelling (`resolved`),
                        // the container-anchored needle proves the shape.
                        || (resolved.contains(&bare)
                            && self.pattern_field_read_of(container, bare))
                    {
                        continue;
                    }
                    // #2499: an unnamed face's field defers to the face loop
                    // below, like the method arm — the loop owns the whole
                    // unnamed face's verdicts, and without the deferral the
                    // field stages its row twice (here and in the loop).
                    let unnamed_face_owns = self.index.get(self.file).is_some_and(|f| {
                        f.extension_faces().iter().any(|face| {
                            face.name.is_none()
                                && self.is_nameless(container)
                                && face.members.contains_key(&site.name)
                        })
                    });
                    if unnamed_face_owns {
                        continue;
                    }
                    // A nameless recovery field has no name token to anchor
                    // at: upstream's row lands on the enclosing container's
                    // declared-name token at zero length (`class C { var }`
                    // reports `unused_field` at `C` — the oracle's
                    // `<unnamed>` element location is the parent fragment's
                    // name), so the declaration range remaps to the
                    // container anchor. Every unnamed member of one
                    // container collapses to the same site; the usage
                    // tail's identical-row fold emits it once.
                    let range = if self.is_nameless(site.name) {
                        self.nameless_member_anchor(site_index, container)
                    } else {
                        site.range
                    };
                    self.stage_usage(SemanticCode::UnusedField, range, &name_text);
                }
                DeclKind::Member {
                    container,
                    is_static,
                    ..
                } => {
                    // `_isUsedMember` for methods/getters/setters: the
                    // accessibility gate first, then indexed uses, then the
                    // override face (a member overriding a super-name is
                    // spared — the walk cannot see whether the overridden
                    // member is itself used, so this only under-reports).
                    // The parameter pass below runs regardless — the
                    // official `visitFormalParameterList` is independent of
                    // the member's own unused-ness.
                    // #2188: an extension's INSTANCE member spares only on
                    // an element-attributed application — the oracle's
                    // `usedElements.members` credit joins on a resolved
                    // implicit application, never a spelling-level read (a
                    // same-spelling access that resolves to nothing credits
                    // the member all the same in the `used_names` arm, so
                    // the extension arm drops it; statics keep the
                    // spelling arms — they resolve through the
                    // container-static path `_E._x`). The credit set is
                    // receiver-validity-gated at the resolution side: an
                    // access through an unresolvable receiver type (the
                    // broken-import corpus) credits nothing, while an
                    // extension whose `on` type is itself invalid still
                    // applies vacuously to a valid receiver — probed:
                    // `c.g` on a valid `C` with `extension _E on Missing`
                    // neither reports `undefined_getter` nor leaves `g`
                    // unused, so the credit alone decides.
                    // The extension-member arm keys both verdicts on the
                    // declaring face (the oracle's `usedElements.members` is
                    // an element set — a same-spelled member of a sibling
                    // extension never shares the credit). `container` is the
                    // extension's written name for a named extension and the
                    // empty spelling for an unnamed one (`container_context`
                    // interns it); the unnamed face never reaches
                    // `containers`, so the site maps back by member lookup —
                    // the position in `extension_faces()` the resolve-side
                    // credit keyed. A member no face claims keeps the
                    // spelling arm.
                    // The oracle agrees an unnamed extension's whole face is
                    // private: `ExtensionElement.isPrivate` answers
                    // `name == null || name.isPrivateName`, element.dart:
                    // 3264-3267, upstream Dart SDK source, so its members
                    // report exactly once, from the face loop — and this arm
                    // DEFERS to that loop (`unnamed_face_owns` below): an
                    // unnamed extension's members DO carry declaration sites
                    // (the container frame interns the empty spelling), so
                    // without the deferral both this arm and the loop stage
                    // the row (#2499's duplicate emission — instance and
                    // static alike). The loop's verdict keys the same
                    // `extension_member_used` credit pool on the member's
                    // own face index; its static spelling arm reads the
                    // `element_used`/`read_names` sets the unnamed face can
                    // actually resolve through.
                    let extension_face = self.index.get(self.file).and_then(|f| {
                        f.extension_faces()
                            .iter()
                            .enumerate()
                            .find(|(_, face)| {
                                face.name.map_or_else(
                                    || self.is_nameless(container),
                                    |name| name == container,
                                ) && face.members.contains_key(&site.name)
                            })
                            .map(|(index, face)| (index, face.name.is_none()))
                    });
                    let unnamed_face_owns = matches!(extension_face, Some((_, true)));
                    let member_used = match extension_face {
                        Some((index, false)) if !is_static => {
                            u32::try_from(index).is_ok_and(|index| {
                                self.extension_member_used
                                    .contains(&(self.file, index, site.name))
                                    || self
                                        .extension_member_used
                                        .contains(&(self.file, index, bare))
                            })
                        }
                        // `_isUsedMember`'s element-level `members` ∪
                        // `elements` join (unused_local_elements_verifier
                        // .dart:977-992): a resolved member access credits
                        // the exact element it bound — never a same-spelling
                        // member of a sibling class. The `member_refs` edge
                        // is that set: a `B._m` use spares `B._m` alone.
                        // `member_element_used` keeps the non-`.`-member
                        // paths (a bare `_foo = 0` inside `_A` resolves
                        // `this._foo` with no `.` token — no `member_reads`/
                        // `member_refs` record; a `RefTarget::Decl`/`Member`
                        // ref credits the spelling).
                        _ => {
                            member_used_elements
                            .contains(&(self.file, container, site.name))
                            || member_element_used.contains(&bare)
                            || ref_member_reads.contains(&(container, site.name))
                            || ref_member_reads.contains(&(container, bare))
                            // The object-pattern field read (`C(_: 42)`)
                            // spares the getter the same way.
                            || (resolved.contains(&bare)
                                && self.pattern_field_read_of(container, bare))
                        }
                    };
                    // `_overridesUsedElement`: the overridden element must
                    // be USED — `walk_super`'s existence answer is the
                    // over-spare the wildcard `get _` corpus pinned (a
                    // `C1Extends extends ContainsWildcardVariable` getter
                    // over an unread super field still reports). The
                    // nameless container arms stay out — a synthetic
                    // container has no supertype to climb.
                    let overrides_used = !self.is_nameless(container)
                        && self.overrides_used_element(
                            container,
                            site.name,
                            &|file, decl_container, name| {
                                member_used_elements.contains(&(file, decl_container, name))
                                    || (file == self.file
                                        && (self.member_was_read(decl_container, &name)
                                            || ref_member_reads
                                                .contains(&(decl_container, name))))
                                    || (!self.is_wildcard(name)
                                        && member_element_used.contains(&name))
                            },
                            8,
                        );
                    let spared = unnamed_face_owns
                        || pragma_spared
                        || (!self.is_nameless(site.name)
                            && self.is_publicly_accessible(site.name, site.kind, false))
                        || member_used
                        || overrides_used;
                    if !spared {
                        // The nameless recovery member anchors like the
                        // field arm above: the enclosing container's
                        // declared-name token at zero length (the oracle's
                        // `<unnamed>` element location is the parent
                        // fragment's name).
                        let range = if self.is_nameless(site.name) {
                            self.nameless_member_anchor(site_index, container)
                        } else {
                            site.range
                        };
                        self.stage_usage(SemanticCode::UnusedElement, range, &display_text);
                    }
                }
                DeclKind::Constructor { container } => {
                    // `_visitConstructorElement`: named constructors only —
                    // the official visit gates on `node.name != null`
                    // (unused_local_elements_verifier.dart:716-723), so an
                    // unnamed constructor is never reported even when the
                    // class declares several (the class face owns the
                    // reference). The capture side stores the written head's
                    // selector; an unnamed head keeps the container spelling.
                    // The named-only gate stops the element face alone — the
                    // parameter pass at the loop bottom still runs for an
                    // unnamed head (`visitFormalParameterList` visits every
                    // formal list; unused_local_elements_verifier.dart:781).
                    if bare != container && !self.is_bare_new_head(bare) {
                        // Only when the class declares more than one — a
                        // single constructor may serve to prevent extension.
                        // The upstream count is
                        // `enclosingElement.constructors.length` — every
                        // declared constructor, named and unnamed alike.
                        // The element model's `constructors` +
                        // `has_unnamed_constructor` carries exactly that set
                        // (extension-type primaries land there without a
                        // `decl_sites` entry — the listener gates
                        // `ContainerKind::Class | Enum` for the site).
                        let multi = self
                            .index
                            .get(self.file)
                            .and_then(|f| f.container(container))
                            .is_some_and(|c| {
                                c.constructors.len() + usize::from(c.has_unnamed_constructor) > 1
                            });
                        // `_isUsedElement`'s element check for ctors: the
                        // `member_refs` edge `credit_constructor_selector`
                        // pushes carries the resolved `(file, container,
                        // selector)` tuple — a `D._named` use never spares
                        // `C._named`. No `element_used` fallback: the
                        // name-level set credits every same-spelling ctor
                        // across classes (the over-credit the element set
                        // exists to prevent). `super.named()` initializers
                        // ride `note_ctor_use_in`'s annotated-only edge —
                        // an unannotated super-target's credit is a known
                        // under-report face (upstream spares it via
                        // `usedElements.elements`).
                        let ctor_used = member_used_elements
                            .contains(&(self.file, container, site.name))
                            || alias_used_ctors.contains(&(self.file, container, site.name))
                            || shorthand_ctor_refs.contains(&site.name);
                        let spared = !multi
                            || pragma_spared
                            || (!self.is_nameless(site.name)
                                && self.is_publicly_accessible(
                                    site.name,
                                    site.kind,
                                    site.is_factory,
                                ))
                            || ctor_used
                            || matches!(self.walk_super(container, site.name), Walk::Found(_));
                        if !spared {
                            // The element's displayName qualifies the selector
                            // with its container (`E.foo`); the site stores the
                            // selector spelling only. A written `.new` head
                            // renders as the container alone (`displayName`'s
                            // `name == 'new'` arm answers the enclosing name).
                            // A provisional head never
                            // rewritten stays at the container spelling — the
                            // unnamed face is never visited (named-only gate).
                            let container_text = self.strings.resolve(container);
                            let display = if name_text == "new" {
                                container_text.to_string()
                            } else {
                                format!("{container_text}.{display_text}")
                            };
                            self.stage_usage(SemanticCode::UnusedElement, site.range, &display);
                        }
                    }
                }
            }
            self.report_optional_parameters(site_index, &site, &super_ctor_targets, &noncall_reads);
        }
        // The recursive-constant-constructor face: a const constructor
        // whose own formal default values const-instantiate the SAME
        // constructor depends on itself (`const A({A e = const A()})` —
        // evaluating the constructor requires evaluating its own default).
        // The default's leading `const Name(` / `const Name.sel(` spelling
        // decides the target identity; anything else (generic arguments,
        // non-const news, other classes) never reports. A non-const
        // constructor's defaults are evaluation-time and stay outside the
        // face.
        // Snapshot the candidate facts before the staging pass (the loop
        // body borrows the engine immutably, `stage_usage` mutably).
        let const_ctor_sites: Vec<(StringId, StringId, SourceRange)> = self
            .decl_sites
            .iter()
            .filter_map(|site| {
                let DeclKind::Constructor { container } = site.kind else {
                    return None;
                };
                site.const_ctor
                    .then_some((container, site.name, site.range))
            })
            .collect();
        for (container, name, range) in const_ctor_sites {
            let Some(formals) = self
                .index
                .get(self.file)
                .and_then(|f| f.container(container))
                .and_then(|c| c.ctor_formals.get(&name))
            else {
                continue;
            };
            let class_text = self.strings.resolve(container);
            let self_key = self.strings.resolve(name).to_string();
            let depends_on_self =
                formals
                    .positional
                    .iter()
                    .chain(formals.named.iter())
                    .any(|formal| {
                        formal
                            .default_value
                            .map(|d| self.strings.resolve(d))
                            .is_some_and(|text| {
                                let rest = text.trim_start();
                                let Some(rest) = rest.strip_prefix("const ") else {
                                    return false;
                                };
                                let rest = rest.trim_start();
                                let Some(head) = rest.strip_prefix(class_text) else {
                                    return false;
                                };
                                head.starts_with('(')
                                    || head
                                        .strip_prefix('.')
                                        .and_then(|sel| sel.trim_start().strip_prefix(&self_key))
                                        .is_some_and(|tail| tail.starts_with('('))
                            })
                    });
            if depends_on_self {
                self.stage_usage(SemanticCode::RecursiveConstantConstructor, range, &self_key);
            }
        }
        // The `unused_field_from_primary_constructor` face
        // (unused_local_elements_verifier.dart:887-920): each field-declaring
        // primary-constructor formal of a class or enum whose field no read
        // spares. A public name is always readable (`_isReadMember`'s
        // public-instance arm returns used), extension-type representation
        // formals are exempt (the official switch's third arm), and the
        // anchor is the parameter's name token.
        for field in std::mem::take(&mut self.primary_fields) {
            let bare = self.bare_name(field.name);
            // The `unresolvedReadMembers` name fallback the `DeclKind::
            // Member` field arm carries: a read that bound no element
            // still spares by spelling.
            let unresolved_read = self.read_names.contains(&bare)
                && self
                    .member_read_owners
                    .get(&bare)
                    .is_none_or(|owners| owners.is_empty());
            if !self.is_private_name(field.name)
                || self.member_was_read(field.container, &bare)
                || unresolved_read
            {
                continue;
            }
            let code = SemanticCode::UnusedFieldFromPrimaryConstructor;
            let name_text = self.strings.resolve(field.name).to_string();
            let keyword_text = self.strings.resolve(field.keyword).to_string();
            let correction = code.correction_message(&keyword_text);
            self.report.usage.push(Diagnostic::new(
                code.severity(),
                code.code_info(),
                code.message(&name_text, ""),
                self.file,
                field.range,
                correction,
            ));
        }
        // Members of a non-public extension (`_isPubliclyAccessible`'s
        // extension arm — a member of an extension is accessible exactly
        // when the extension is, so unnamed extensions and private-named
        // extensions (`_E`) both check their members like private
        // declarations): a member no resolved application credits reports.
        // The oracle's credit is element-level (`usedElements.members`
        // gains the member only when the access's `read.element` resolved —
        // `element_usage_detector.dart`'s `extensionOverride`/named-read
        // arms): a same-spelling access that never resolved to the member
        // credits nothing, so the bare `used_names`/`read_names` spelling
        // arms do NOT spare instance members here. The credit set is
        // receiver-validity-gated at the resolution side — an access
        // through an unresolvable receiver (the broken-import corpus)
        // credits nothing, while an invalid `on` type applies vacuously to
        // a valid receiver — so the credit alone decides. Static members
        // keep the spelling arms — they resolve through the
        // container-static path (`_E._x`), not the implicit application
        // probe. A write-only field still needs a READ (the `unused_field`
        // rule). The candidates clone out first — the staging below
        // mutates the engine.
        //
        // Membership split with `decl_sites`: an unnamed extension's
        // members carry declaration sites (the container frame interns the
        // empty spelling), but the `DeclKind::Member` arm defers their
        // verdicts to this loop (#2499) — so the whole unnamed set still
        // checks here; a named extension's members are
        // declaration sites — its methods/getters/setters ride the
        // `DeclKind::Member` arm above (the `extension_member_used` credit
        // substituted for the spelling arm there), and its STATIC fields
        // the `DeclKind::Member` field arm — leaving this face the
        // named-private instance fields the site arms are ineligible for.
        let mut extension_members: Vec<(u32, StringId, SourceRange, MemberKind, bool, bool)> =
            Vec::new();
        if let Some(elements) = self.index.get(self.file) {
            for (face_index, face) in elements.extension_faces().iter().enumerate() {
                let named_private = face.name.is_some_and(|name| self.is_private_name(name));
                if !named_private && face.name.is_some() {
                    continue;
                }
                for (spelling, entry) in &face.members {
                    // A named extension's members are declaration sites —
                    // this face keeps only its public-named instance fields
                    // (the `DeclKind::Member` arms own the rest; unnamed
                    // extensions declare no member sites — their `container`
                    // is the empty spelling and the arm's face lookup owns
                    // them).
                    let site_owned = named_private
                        && !(entry.kind == MemberKind::Field
                            && !entry.is_static
                            && !self.is_private_name(*spelling));
                    if site_owned {
                        continue;
                    }
                    extension_members.push((
                        u32::try_from(face_index).unwrap_or(u32::MAX),
                        *spelling,
                        entry.name_range,
                        entry.kind,
                        entry.is_static,
                        // The `_isUsedMember` pragma gate alone
                        // (`_hasPragmaVmEntryPoint`,
                        // unused_local_elements_verifier.dart:1180-1181):
                        // every other annotation on an extension member
                        // spares nothing.
                        entry
                            .annotations
                            .iter()
                            .any(|annotation| self.annotation_named(annotation.name, "pragma")),
                    ));
                }
            }
        }
        for (face_index, spelling, range, kind, is_static, pragma_spared) in extension_members {
            if range == MemberEntry::MISSING_NAME_RANGE || face_index == u32::MAX {
                continue;
            }
            let bare = self.bare_name(spelling);
            let spared = pragma_spared
                || self
                    .extension_member_used
                    .contains(&(self.file, face_index, spelling))
                || (is_static
                    && (element_used.contains(&bare)
                        || (matches!(kind, MemberKind::Field) && self.read_names.contains(&bare))));
            if !spared {
                let code = if matches!(kind, MemberKind::Field) {
                    SemanticCode::UnusedField
                } else {
                    SemanticCode::UnusedElement
                };
                let name_text = self.strings.resolve(spelling).to_string();
                // The `{0}` argument is the element `displayName`: never the
                // setter's `=` suffix.
                let display_text = match name_text.strip_suffix('=') {
                    // Same operator-name guard as the decl-site arm — an
                    // `operator []=` member's displayName keeps `[]=`.
                    Some(bare)
                        if bare
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_') =>
                    {
                        bare.to_string()
                    }
                    _ => name_text.clone(),
                };
                self.stage_usage(code, range, &display_text);
            }
        }
        // The element-attributed pool replaces the raw spelling set for the
        // downstream consumers that read `used_names` after this point —
        // `report_unused_imports` credits an import only through a resolved
        // use (the same `exported(target, n)` join), and `take_use_face`
        // publishes it as the library-close `consumed.used` the staged
        // `unused_*` rows re-decide against. The raw pool's unresolved
        // member spellings would over-credit both — an `InvalidType`
        // receiver's access resolving nothing still released the row.
        self.used_names = element_used;
        self.credit_forward_references();
        self.retire_joined_case_binders();
        self.report_pattern_assignment_binders();
        self.drop_pre_feature_underscore_locals();
        self.suppress_same_line_ignored_usage();
        self.report_unused_imports();
        self.report_deprecated_sdk_libraries();
    }

    /// The case-head binder joins (#3558): a pattern variable of a switch
    /// case head, an if-case head or a switch-expression arm reports only
    /// while its upstream ELEMENT is unread, and two same-named binders of
    /// one head are one element in two shapes:
    ///
    /// - across a logical-or (`int a || [int a]`) the branches join into one
    ///   `JoinPatternVariableElement`, and a read of any spelling — the
    ///   `when` guard's included — marks every branch's binder used
    ///   (`UsedLocalElements.addElement`'s `transitiveVariables`);
    /// - inside one conjunction or collection (`[var v] && [var v, …]`) the
    ///   repeat is a duplicate the verifier never visits (`isDuplicate`),
    ///   and the spelling's reads bind the first binder.
    ///
    /// The frame close credits a read to ONE same-named row, so the staged
    /// verdicts are re-decided over the head's token span: a row drops when
    /// a same-named binder of its head staged no row (that element was
    /// read), and a duplicate's row drops outright. A chain nobody reads
    /// keeps one row per branch — upstream reports every binder.
    fn retire_joined_case_binders(&mut self) {
        use dartq_scanner::TokenKind;
        let local = SemanticCode::UnusedLocalVariable.code_info();
        let staged: FxHashSet<u32> = self
            .report
            .usage
            .iter()
            .filter(|row| row.code == local)
            .map(|row| row.range.offset)
            .collect();
        if staged.is_empty() {
            return;
        }
        let tokens = self.tokens.as_slice();
        let spelling = |i: usize| self.source.get(tokens[i].byte_range());
        let mut retired: FxHashSet<u32> = FxHashSet::default();
        for (x, token) in tokens.iter().enumerate() {
            if token.kind != TokenKind::Identifier || !staged.contains(&token.utf16_offset) {
                continue;
            }
            // A nameless recovery row (`<unnamed>`) is never a binder.
            let name = spelling(x).filter(|text| !text.is_empty());
            if name.is_none() {
                continue;
            }
            let Some((start, end)) = case_pattern_extent(self.tokens, x) else {
                continue;
            };
            let shared = (start..end).any(|y| {
                y != x
                    && binder_like(tokens, y)
                    && spelling(y) == name
                    && (!staged.contains(&tokens[y].utf16_offset)
                        || (y < x && !or_joined(tokens, y, x)))
            });
            if shared {
                retired.insert(token.utf16_offset);
            }
        }
        if !retired.is_empty() {
            self.report
                .usage
                .retain(|row| row.code != local || !retired.contains(&row.range.offset));
        }
    }

    /// The assignment-pattern declaring binder face (#3302): a variable
    /// pattern that carries `var`/`final` or a written type inside a
    /// pattern ASSIGNMENT (`(var a) = 1;`, `((a, var b) && r) = (1, 2);`)
    /// is the compile-time error the parser reports
    /// (`pattern_assignment_declares_variable`), yet the analyzer mints a
    /// local element for the binder all the same. That element enters no
    /// scope — the rest of the block keeps resolving the outer spelling —
    /// so nothing can ever read it and the unused verifier reports
    /// `unused_local_variable` at the binder name, at the very range the
    /// error carries (oracle 3.13.7: a same-spelled read before or after
    /// the assignment spares nothing, and a wildcard spelling is spared by
    /// the same `_isNamedWildcard` arms the frame close applies).
    ///
    /// The parse-side ledger has no entry to read here: an
    /// assignment-context binder reaches `handle_declared_variable_pattern`
    /// but registers no local (that arm is the bare-identifier pattern,
    /// which declares nothing), so this face re-derives its population
    /// from the token stream. A plain `=` whose shortest balanced left
    /// suffix is one bracket group is a pattern assignment only when the
    /// whole pattern — the group plus an object pattern's `Type` /
    /// `prefix.Type` head or a typed collection's `<…>` arguments — starts
    /// where an expression may start ([`assignment_pattern_context`]). A
    /// declaration's own head (`var (…) = …`, `var Square(…) = …`,
    /// `final <int>[…] = …`), a redirecting factory's formal list
    /// (`factory A.f(int a) = A;` — #3558), a function-typed formal's
    /// default (`String this.f(int x) = g`) and a `for` head's
    /// unterminated group all fail the balance or the context. Inside the
    /// span a `var`/`final` head or a written-type run at a
    /// pattern-element position declares the name it introduces. A
    /// repeated spelling inside one pattern reports once (the first
    /// binder — the duplicate already reports
    /// `duplicate_variable_pattern`).
    fn report_pattern_assignment_binders(&mut self) {
        let local = SemanticCode::UnusedLocalVariable;
        let local_info = local.code_info();
        let tokens = self.tokens.as_slice();
        let mut rows: Vec<(SourceRange, StringId)> = Vec::new();
        let mut staged: FxHashSet<u32> = self
            .report
            .usage
            .iter()
            .filter(|row| row.code == local_info)
            .map(|row| row.range.offset)
            .collect();
        for (eq, token) in tokens.iter().enumerate() {
            if token.kind != dartq_scanner::TokenKind::Eq {
                continue;
            }
            let Some((start, end)) = pattern_span_left_of(tokens, eq) else {
                continue;
            };
            if start >= end
                || !matches!(
                    tokens[start].kind,
                    dartq_scanner::TokenKind::OpenParen
                        | dartq_scanner::TokenKind::OpenSquare
                        | dartq_scanner::TokenKind::OpenCurly
                )
                || !assignment_pattern_context(tokens, pattern_head_start(tokens, start))
            {
                continue;
            }
            let mut seen: FxHashSet<StringId> = FxHashSet::default();
            for at in start..end {
                let Some((name_at, name)) = self.declaring_binder_at(tokens, start, at) else {
                    continue;
                };
                if self.is_wildcard(name) || !seen.insert(name) {
                    continue;
                }
                let name_token = &tokens[name_at];
                let length: u32 = self
                    .source
                    .get(name_token.byte_range())
                    .map(|text| text.chars().map(char::len_utf16).sum::<usize>())
                    .and_then(|length| u32::try_from(length).ok())
                    .unwrap_or(0);
                let range = SourceRange {
                    offset: name_token.utf16_offset,
                    length,
                };
                if staged.insert(range.offset) {
                    rows.push((range, name));
                }
            }
        }
        for (range, name) in rows {
            let text = self.strings.resolve(name).to_string();
            self.stage_usage(local, range, &text);
        }
    }

    /// The declaring binder introduced at token `at` inside the pattern
    /// span `start..`: `var a` / `final a` (the keyword declares the next
    /// identifier, or the one after a written type — `final int a`
    /// anchors on `a`, never on the type token) or a written-type leaf
    /// (`int a`, `List<int>? a`). The head must sit at a pattern-element
    /// position — right after a sub-pattern opener, a field/positional
    /// separator, or a logical-and/or junction — so an ordinary `a b`
    /// adjacency (a named field's value, a call's arguments) never reads
    /// as a declaration. A bare identifier is an ASSIGNED variable, never
    /// a binder.
    fn declaring_binder_at(
        &self,
        tokens: &[dartq_scanner::Token],
        start: usize,
        at: usize,
    ) -> Option<(usize, StringId)> {
        use dartq_scanner::{Keyword, TokenKind};
        let previous = at.checked_sub(1).filter(|&i| i >= start)?;
        if !pattern_element_position(tokens[previous].kind) {
            return None;
        }
        let is_identifier = |i: usize| tokens.get(i).map(|t| t.kind) == Some(TokenKind::Identifier);
        let name_at = match tokens[at].kind {
            TokenKind::Keyword(Keyword::Var | Keyword::Final) => type_run_end(tokens, at + 1)
                .filter(|&end| is_identifier(end))
                .unwrap_or(at + 1),
            // `int a` / `List<int> a`: the written type's run ends at the
            // declared name. A name that is not an identifier (another
            // operator, a call's `(`) is a type use, not a variable
            // pattern.
            TokenKind::Identifier => type_run_end(tokens, at)?,
            _ => return None,
        };
        if !is_identifier(name_at) {
            return None;
        }
        let name = self.source.get(tokens[name_at].byte_range());
        Some((name_at, self.intern(name?)))
    }

    /// The forward-reference credit (`referenced_before_declaration`,
    /// referenced_before_declaration_test.dart's hideInBlock family): a
    /// bare read of `v` before `var v` in the same block chain binds the
    /// LOCAL in the oracle — the resolver's block scopes see the whole
    /// block's declarations, so the read reports
    /// `referenced_before_declaration` and marks the local element
    /// referenced — and the local never reports
    /// `unused_local_variable`. dartq's single-pass walk credits a read
    /// only against an ALREADY-tracked local: the forward read bound the
    /// top-level spelling (`RefTarget::Decl`) or missed outright, so the
    /// not-yet-declared local stays un-referenced and stages a spurious
    /// row. The walk files are held, so the credit lands here, at the
    /// cluster's staging point: a staged local-variable/local-function
    /// row whose name carries an earlier non-local read inside the
    /// declaration's block chain drops.
    ///
    /// The chain test walks the real token stream between read and
    /// declaration ([`Self::block_chain_holds`]): the declaration's block
    /// must enclose the read, which holds exactly when the relative
    /// brace depth at the declaration is zero or negative and the
    /// running minimum never dips below it — a read in a sibling or
    /// later-opened block (another executable, a local function's body)
    /// stays excluded, so the credit cannot leak across executables. A
    /// plain-assignment target never credits (a write-only life does not
    /// spare an unused local — the verifier's credit counts reads);
    /// compound targets read and credit. Pattern-group windows and
    /// case-header binders decide their all-or-nothing joins inside the
    /// frame close, before this pass — their rows keep the staged
    /// verdict, the documented boundary.
    fn credit_forward_references(&mut self) {
        let local = SemanticCode::UnusedLocalVariable.code_info();
        let element = SemanticCode::UnusedElement.code_info();
        let site_anchors: FxHashSet<u32> = self
            .decl_sites
            .iter()
            .map(|site| site.range.offset)
            .collect();
        // The candidate reads, keyed by spelling: every published bare
        // reference that resolved to something other than a local or a
        // type parameter (a local hit already credited its declaration at
        // push time; a type parameter is never the oracle's block-scope
        // answer for a value read), paired with its token end for the
        // write-target peek.
        let mut reads: FxHashMap<StringId, Vec<(u32, u32)>> = FxHashMap::default();
        for r in &self.refs {
            if !matches!(r.target, RefTarget::Local | RefTarget::TypeParam) {
                reads
                    .entry(r.name)
                    .or_default()
                    .push((r.range.offset, r.range.end()));
            }
        }
        let mut drop: Vec<usize> = Vec::new();
        for (index, row) in self.report.usage.iter().enumerate() {
            if row.code != local && row.code != element {
                continue;
            }
            let at = row.range.offset;
            // A top-level or member declaration anchor: the element pool's
            // own rows, never a frame local's.
            if site_anchors.contains(&at) {
                continue;
            }
            let byte = self.byte_of(at).min(self.source.len());
            let end = self.byte_of(row.range.end()).min(self.source.len());
            let name = self.intern(&self.source[byte..end]);
            let Some(offsets) = reads.get(&name) else {
                continue;
            };
            if offsets
                .iter()
                .filter(|&&(from, _)| from < at)
                .any(|&(from, end)| {
                    !self.is_plain_assignment_target(end) && self.block_chain_holds(from, at)
                })
            {
                drop.push(index);
            }
        }
        if drop.is_empty() {
            return;
        }
        let mut next = 0usize;
        let mut index = 0usize;
        self.report.usage.retain(|_| {
            let keep = next >= drop.len() || index != drop[next];
            if !keep {
                next += 1;
            }
            index += 1;
            keep
        });
    }

    /// Whether the block that declares `to` also encloses `from`: walking
    /// the real tokens between the two offsets, the relative `{`/`}`
    /// depth at `to` must be zero or negative (the declaration sits at
    /// the read's depth or shallower — an ancestor block) and the running
    /// minimum must never fall below it (the declaration's block never
    /// closed on the way — a sibling or later-opened block fails).
    /// Interpolation `${` opens count with plain braces, so a balanced
    /// interpolation nets zero; string literals and comments carry no
    /// tokens and never distort the count.
    fn block_chain_holds(&self, from: u32, to: u32) -> bool {
        let tokens = self.tokens.as_slice();
        let start = tokens.partition_point(|t| t.utf16_offset <= from);
        let mut rel: i32 = 0;
        let mut min: i32 = 0;
        for t in &tokens[start..] {
            if t.utf16_offset >= to {
                break;
            }
            match t.kind {
                dartq_scanner::TokenKind::OpenCurly
                | dartq_scanner::TokenKind::StringInterpolationExpression => rel += 1,
                dartq_scanner::TokenKind::CloseCurly => {
                    rel -= 1;
                    min = min.min(rel);
                }
                _ => {}
            }
        }
        rel <= 0 && min >= rel
    }

    // ------------------------------------------------------------------
    // Phase 2a/2b credits. Both passes re-derive element-level evidence
    // ------------------------------------------------------------------
    // Phase 2b credits. The pass re-derives element-level evidence at
    // finalization from what the parse already published (the resolver
    // files answer the same questions but hold no write lane here),
    // keyed to the upstream `ImportsVerifier` this cluster mirrors. The
    // invocation-form `call` credit of the same upstream family needs the
    // parse-side frame (locals and written types are truncated per frame
    // before this pass) and stays at its resolver wiring point.
    // ------------------------------------------------------------------

    /// The prefixed-import use evidence (Phase 2b): the StringId-level
    /// pool cannot tell a prefix spelling from a same-spelling local or a
    /// same-spelling member merge, so the prefixed `unused_import` credit
    /// re-derives element-level evidence per prefix — which names the
    /// prefix's namespace resolved, and whether the prefix itself was
    /// referenced bare. The records:
    ///
    /// - member reads through a `Prefix` receiver are frame-aware at the
    ///   merge (a shadowed spelling classifies as a local access there),
    ///   and attribute their member name — except `loadLibrary`, which
    ///   upstream resolves through `PrefixScope.lookup`'s early return
    ///   (scope.dart 3.13 :630-638: the synthetic `loadLibraryFunction`
    ///   answers before `lookupResult` ever runs) and so credits the
    ///   import in neither the invoke nor the tear-off form
    ///   (`MethodInvocationResolutionTest/
    ///   test_hasReceiver_deferredImportPrefix_loadLibrary#0`);
    /// - a member read the prefix scope did not resolve credits nothing
    ///   (upstream `lookupResult(null)` records no element — the oracle's
    ///   `undefined_getter`-on-a-found-prefix face is ungated and the
    ///   import reports unused); the broken-import suppression face
    ///   (`guard.ignore`) keeps its attribution — those edges carry no
    ///   resolvable target, so the directive pass skips them regardless;
    /// - bare `Prefix` references in expression position (the receiver
    ///   occurrence of a member read dedupes by offset) credit bare;
    /// - type-annotation continuations (`p.T`) re-derive the shadow
    ///   decision the Continuation arm made frame-aware at parse time —
    ///   the type-use record is frame-blind, so
    ///   [`Self::prefix_shadowed_at`] replays it from the declaration
    ///   tables (the shadow corpus:
    ///   `PrefixShadowedByLocalDeclarationTest/
    ///   test_local_variable_type_inside_function_with_shadowing_variable_before#0`
    ///   — the local's own uses must not resurrect the import);
    /// - prefixed creation/metadata heads (`new p.C()`, `@p.C.a`)
    ///   attribute their selector under the same shadow gate.
    fn prefix_use_evidence(&self) -> FxHashMap<StringId, (FxHashSet<StringId>, bool)> {
        let mut evidence: FxHashMap<StringId, (FxHashSet<StringId>, bool)> = FxHashMap::default();
        let load_library = self.intern("loadLibrary");
        // The parse's frame-aware shadow emissions, keyed by head-token
        // offset ([`Self::prefix_shadowed_at`]).
        let shadowed_heads: FxHashSet<u32> = self
            .report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "prefix_shadowed_by_local_declaration")
            .map(|d| d.range.offset)
            .collect();
        let prefixes: Vec<StringId> = self.imports.iter().filter_map(|site| site.prefix).collect();
        for prefix in prefixes {
            let entry = evidence.entry(prefix).or_default();
            let mut receiver_uses: FxHashSet<u32> = FxHashSet::default();
            for read in &self.member_reads {
                let Receiver::Prefix { name } = read.recv else {
                    continue;
                };
                if name != prefix {
                    continue;
                }
                receiver_uses.insert(read.recv_span.offset);
                if read.name == load_library {
                    // The early-return face: resolved through the
                    // synthetic function, never a namespace lookup — no
                    // element, no credit (both invocation forms).
                    continue;
                }
                if self.prefixed_hit(prefix, read.name)
                    || self.guard.ignore(Some(prefix), read.name)
                {
                    entry.0.insert(read.name);
                }
                // A member read the prefix scope did not resolve credits
                // nothing: upstream `PrefixScope.lookup` answers the miss
                // and `lookupResult(null)` records no element (scope.dart
                // :651-653) — a bare credit here would hold the import open
                // beside a miss the oracle answers with `undefined_getter`
                // on the FOUND prefix (ungated — the Unquoted-imports
                // `keywords_A01_t01` family), where the import stays
                // unused. Inert under the engine's current
                // `undefined_prefixed_name` classification of the same miss
                // (a gate-listed code returns the pass before this pool is
                // consulted); the mirror must stand when that
                // classification lands the oracle's split.
            }
            for r in &self.refs {
                if r.name == prefix
                    && matches!(r.target, RefTarget::Prefix)
                    && !receiver_uses.contains(&r.range.offset)
                {
                    entry.1 = true;
                }
            }
            for (range, name) in &self.type_uses {
                let head_offset = self
                    .prefix_head_before(range.offset)
                    .or_else(|| self.prefix_head_at(range.offset));
                let Some(head_offset) = head_offset else {
                    continue;
                };
                if self.spelling_at(head_offset) != Some(prefix)
                    || self.prefix_shadowed_at(&shadowed_heads, prefix, head_offset)
                {
                    continue;
                }
                entry.0.insert(*name);
            }
            for &offset in &self.ctor_prefix_heads {
                if self.spelling_at(offset) != Some(prefix)
                    || self.prefix_shadowed_at(&shadowed_heads, prefix, offset)
                {
                    continue;
                }
                match self.selector_after_prefix_head(offset) {
                    Some(selector) => {
                        entry.0.insert(selector);
                    }
                    None => entry.1 = true,
                }
            }
        }
        evidence
    }

    /// Whether the prefix spelling is shadowed at `at`: a top-level
    /// declaration of the spelling shadows file-wide (the library scope's
    /// flat answer — the oracle's declarations shadow imports), and the
    /// parse's own frame-aware shadow decision at the head token — the
    /// type-position arm emits `prefixShadowedByLocalDeclaration` exactly
    /// at the dead head (`resolution.rs`'s `arm_type_prefix`), the one
    /// record that survives the per-frame local truncation with the
    /// binding facts intact. `shadowed_heads` is that emission's offset
    /// set, precomputed by [`Self::prefix_use_evidence`].
    fn prefix_shadowed_at(
        &self,
        shadowed_heads: &FxHashSet<u32>,
        prefix: StringId,
        at: u32,
    ) -> bool {
        self.decl_sites.iter().any(|site| {
            self.bare_name(site.name) == prefix
                && matches!(
                    site.kind,
                    DeclKind::TopLevelFn
                        | DeclKind::TopLevelVar
                        | DeclKind::TypeAlias
                        | DeclKind::Container
                )
        }) || shadowed_heads.contains(&at)
    }

    /// The identifier token ending in `head . selector` before
    /// `selector_offset`: the head token's offset, when the two preceding
    /// tokens are exactly an identifier and a dot.
    fn prefix_head_before(&self, selector_offset: u32) -> Option<u32> {
        let tokens = self.tokens.as_slice();
        let index = tokens.partition_point(|t| t.utf16_offset < selector_offset);
        let selector = tokens.get(index)?;
        if selector.utf16_offset != selector_offset
            || selector.kind != dartq_scanner::TokenKind::Identifier
            || index < 2
            || tokens[index - 1].kind != dartq_scanner::TokenKind::Period
            || tokens[index - 2].kind != dartq_scanner::TokenKind::Identifier
        {
            return None;
        }
        Some(tokens[index - 2].utf16_offset)
    }

    /// Whether a `head . selector` group STARTS at `offset` (the head is
    /// the token there) — the shape a type-use record spans whole (`p.T`).
    fn prefix_head_at(&self, offset: u32) -> Option<u32> {
        let tokens = self.tokens.as_slice();
        let index = tokens.partition_point(|t| t.utf16_offset < offset);
        let head = tokens.get(index)?;
        if head.utf16_offset != offset
            || head.kind != dartq_scanner::TokenKind::Identifier
            || tokens.get(index + 1)?.kind != dartq_scanner::TokenKind::Period
            || tokens.get(index + 2)?.kind != dartq_scanner::TokenKind::Identifier
        {
            return None;
        }
        Some(offset)
    }

    /// The interned spelling of the identifier token at `offset`.
    fn spelling_at(&self, offset: u32) -> Option<StringId> {
        let tokens = self.tokens.as_slice();
        let index = tokens.partition_point(|t| t.utf16_offset < offset);
        let token = tokens.get(index)?;
        if token.utf16_offset != offset || token.kind != dartq_scanner::TokenKind::Identifier {
            return None;
        }
        let byte = token.byte_offset as usize;
        Some(self.intern(&self.source[byte..byte + token.byte_len as usize]))
    }

    /// The selector identifier following a prefix head at `offset`
    /// (`head . selector`): the attributed member of a prefixed
    /// creation/metadata head.
    fn selector_after_prefix_head(&self, offset: u32) -> Option<StringId> {
        let tokens = self.tokens.as_slice();
        let index = tokens.partition_point(|t| t.utf16_offset < offset);
        let head = tokens.get(index)?;
        if head.utf16_offset != offset
            || head.kind != dartq_scanner::TokenKind::Identifier
            || tokens.get(index + 1)?.kind != dartq_scanner::TokenKind::Period
        {
            return None;
        }
        let selector = tokens.get(index + 2)?;
        if selector.kind != dartq_scanner::TokenKind::Identifier {
            return None;
        }
        let byte = selector.byte_offset as usize;
        Some(self.intern(&self.source[byte..byte + selector.byte_len as usize]))
    }

    /// The `typedef NAME<T> = RHS` head the alias exposes (`RHS` or
    /// `prefix.RHS`): `(head, prefix)` interned off the token stream, `None`
    /// for the function-style form (`typedef void F()`) or a missing `=`.
    /// The returned head is the first written identifier after `=` — the
    /// declaration-site token's own span cannot carry it, so the type-uses
    /// index never learns which name a given alias RHS opened with.
    fn typedef_rhs_head(&self, site: &DeclSite) -> Option<(StringId, Option<StringId>)> {
        let tokens = self.tokens.as_slice();
        let mut index = tokens.partition_point(|t| t.utf16_offset < site.range.end());
        // `NAME` may open a type-parameter group (`A<T>`): the alias's own
        // `= ` is the first Eq AFTER the group's matching close, or the
        // first Eq outright when no `<` follows the name.
        if tokens.get(index)?.kind == dartq_scanner::TokenKind::Lt {
            index = self.tokens.matching_end(index)? + 1;
        }
        if tokens.get(index)?.kind != dartq_scanner::TokenKind::Eq {
            return None;
        }
        let head = tokens.get(index + 1)?;
        if head.kind != dartq_scanner::TokenKind::Identifier {
            return None;
        }
        let head_name = self.spelling_at(head.utf16_offset)?;
        // `= prefix.Name`: the dot-joined spelling resolves through the
        // prefix table — return both sides so the caller walks
        // `prefixed_container` instead of the bare scope.
        if tokens.get(index + 2).map(|t| t.kind) == Some(dartq_scanner::TokenKind::Period)
            && tokens.get(index + 3).map(|t| t.kind)
                == Some(dartq_scanner::TokenKind::Identifier)
        {
            let leaf = tokens.get(index + 3)?;
            let byte = leaf.byte_offset as usize;
            let leaf_name = self.intern(&self.source[byte..byte + leaf.byte_len as usize]);
            return Some((leaf_name, Some(head_name)));
        }
        Some((head_name, None))
    }

    /// Whether the identifier whose token ends at `end` is a plain
    /// assignment target: the next non-whitespace byte is a single `=`
    /// (not `==`). Only the bare-write shape loses the credit — compound
    /// assignments read.
    fn is_plain_assignment_target(&self, end: u32) -> bool {
        let mut byte = self.byte_of(end);
        let bytes = self.source.as_bytes();
        while byte < bytes.len() && bytes[byte].is_ascii_whitespace() {
            byte += 1;
        }
        bytes.get(byte) == Some(&b'=') && bytes.get(byte + 1) != Some(&b'=')
    }

    /// The unused-local verifier's pre-feature wildcard fallback
    /// (`_isNamedWildcard`'s else branch, unused_local_elements_verifier.dart
    /// :836-844): below 3.7 an ALL-UNDERSCORES name spares the
    /// local-variable and catch-exception arms (neither carries the
    /// explicit `_wildCardVariablesEnabled` conjunct the local-function
    /// arm has, :1103 — a pre-feature `_()` function reports, the
    /// preWildcards pin), and the catch-STACK arm has no wildcard test at
    /// all. [`Self::is_wildcard`] mirrors the feature-gated exact-`_`
    /// `isWildcardVariable` every other consumer applies, so this pass
    /// restores the two fallback arms: with the feature off, staged
    /// `unused_local_variable` / `unused_catch_clause` rows on
    /// all-underscores names drop. A no-op under the feature — the
    /// modern corpus pays one feature-bit check.
    fn drop_pre_feature_underscore_locals(&mut self) {
        if self.class_modifiers.wildcards_enabled(self.lib) {
            return;
        }
        let local = SemanticCode::UnusedLocalVariable.code_info();
        let catch_clause = SemanticCode::UnusedCatchClause.code_info();
        let mut drop: Vec<usize> = Vec::new();
        for (index, row) in self.report.usage.iter().enumerate() {
            if row.code != local && row.code != catch_clause {
                continue;
            }
            let byte = self.byte_of(row.range.offset).min(self.source.len());
            let end = self.byte_of(row.range.end()).min(self.source.len());
            let name = &self.source[byte..end];
            if !name.is_empty() && name.bytes().all(|b| b == b'_') {
                drop.push(index);
            }
        }
        if drop.is_empty() {
            return;
        }
        let mut next = 0usize;
        let mut index = 0usize;
        self.report.usage.retain(|_| {
            let keep = next >= drop.len() || index != drop[next];
            if !keep {
                next += 1;
            }
            index += 1;
            keep
        });
    }

    /// The trailing same-line `// ignore:` face over the local-variable
    /// and field rows (`IgnoreInfo.forDart`, ignore_info.dart:119-143): an
    /// ignore comment whose line carries code before it applies to THAT
    /// line, so `var x = 0; // ignore: unused_local_variable` and
    /// `int _f = 0; // ignore: unused_field` spare the row exactly as the
    /// standalone form does through the module root's [`IgnoreSuppression`]
    /// — which materializes standalone comments only, so the same-line
    /// form never reached the table (the archive census rows, #2189-c).
    /// Scoped to the cluster's verified population (`unused_local_variable`,
    /// `unused_field`); every other code keeps its documented under-report.
    /// The entry must name the ROW's own code — `// ignore: unused_field`
    /// on a local variable suppresses nothing (the ignore-list's per-name
    /// match, ignore_info.dart:150-163). A `//` head counts only after
    /// whitespace (or at line start), so a `//` inside a string literal
    /// never reads as a comment.
    fn suppress_same_line_ignored_usage(&mut self) {
        if !self.source.contains("ignore:") {
            return;
        }
        let local = SemanticCode::UnusedLocalVariable.code_info();
        let field = SemanticCode::UnusedField.code_info();
        let mut drop: Vec<usize> = Vec::new();
        for (index, row) in self.report.usage.iter().enumerate() {
            let code = if row.code == local {
                "unused_local_variable"
            } else if row.code == field {
                "unused_field"
            } else {
                continue;
            };
            let byte = self.byte_of(row.range.offset).min(self.source.len());
            let start = self.source[..byte].rfind('\n').map_or(0, |at| at + 1);
            let end = self.source[start..]
                .find('\n')
                .map_or(self.source.len(), |at| start + at);
            if self.line_has_trailing_ignore(&self.source[start..end], code) {
                drop.push(index);
            }
        }
        if drop.is_empty() {
            return;
        }
        let mut next = 0usize;
        let mut index = 0usize;
        self.report.usage.retain(|_| {
            let keep = next >= drop.len() || index != drop[next];
            if !keep {
                next += 1;
            }
            index += 1;
            keep
        });
    }

    /// Whether the line carries a `// ignore:` entry naming `code` with
    /// code before the comment (the trailing form — a standalone
    /// comment's entry is the next line's, the root face's). A head only
    /// counts after whitespace (or at line start) — a `//` inside a
    /// string literal is not a comment — and the name grammar mirrors
    /// the root parser's: comma-list, alphanumeric/underscore heads,
    /// case-insensitive.
    fn line_has_trailing_ignore(&self, line: &str, code: &str) -> bool {
        let bytes = line.as_bytes();
        let mut from = 0usize;
        while let Some(at) = line[from..].find("//") {
            let head = from + at;
            from = head + 2;
            // The head guard: a comment head opens after whitespace (or
            // at the line's start), never mid-token.
            if head > 0 && !bytes[head - 1].is_ascii_whitespace() {
                continue;
            }
            // The trailing guard: the official same-line form is a
            // comment with code before it; an all-whitespace prefix is
            // the standalone form (the next line's — the root face's).
            if !bytes[..head].iter().any(|b| !b.is_ascii_whitespace()) {
                continue;
            }
            let bare = line[from..]
                .trim_start_matches('/')
                .trim_start()
                .strip_prefix("ignore:");
            let Some(rest) = bare else {
                continue;
            };
            if rest.split(',').any(|name| {
                let name = name.trim();
                let end = name
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(name.len());
                name[..end].eq_ignore_ascii_case(code)
            }) {
                return true;
            }
        }
        false
    }

    /// `_isPubliclyAccessible` (`unused_local_elements_verifier.dart`:
    /// 1002-1028) over the site's facts: a private name is never
    /// accessible; an instance member of a private interface stays
    /// reachable through exported instances; a static member or
    /// constructor of a private interface is not; a member of an extension
    /// is accessible exactly when the extension is; a GENERATIVE
    /// constructor of an enum never is, while an enum's FACTORY
    /// constructor falls through to the interface arm — a public enum's
    /// `factory E.named()` is exported (`isGenerative` gates only the
    /// generative face, unused_local_elements_verifier.dart:1010-1014,
    /// upstream Dart SDK source — the constructor_body constFactory corpus,
    /// where the error'd factory body never turns into an unused row).
    /// `_overridesUsedElement`
    /// (unused_local_elements_verifier.dart:1016-1022): a member is used
    /// when any element it overrides is used — recursively (the overridden
    /// element's own overrides chain). The climb reads `walk_super`'s
    /// `Found` + `found_origin` latch for the declaring
    /// `(container, file)` hop, asks `is_used` of that element's own
    /// verdict, then continues the climb from there. `depth` bounds the
    /// recursion (an override cycle answers a miss the same way upstream's
    /// memoized-any walk does — the bounded climb keeps the pass's
    /// termination invariant without a fixpoint set).
    fn overrides_used_element(
        &self,
        container: StringId,
        member: StringId,
        is_used: &dyn Fn(FileId, StringId, StringId) -> bool,
        depth: u32,
    ) -> bool {
        if depth == 0 {
            return false;
        }
        if !matches!(self.walk_super(container, member), Walk::Found(_)) {
            return false;
        }
        let Some((decl_container, decl_file)) = self.member_faces.found_origin.get() else {
            return false;
        };
        is_used(decl_file, decl_container, member)
            || self.overrides_used_element(decl_container, member, is_used, depth - 1)
    }

    pub(crate) fn is_publicly_accessible(
        &self,
        name: StringId,
        kind: DeclKind,
        is_factory: bool,
    ) -> bool {
        if self.is_private_name(name) {
            return false;
        }
        let (container, is_static, is_ctor) = match kind {
            DeclKind::Member {
                is_static,
                container,
                ..
            } => (container, is_static, false),
            DeclKind::Constructor { container } => (container, false, true),
            // A top-level executable's accessibility is just its name.
            _ => return !self.is_private_name(name),
        };
        let Some(c) = self
            .index
            .get(self.file)
            .and_then(|f| f.container(container))
        else {
            return true;
        };
        match c.kind {
            ContainerKind::Enum if is_ctor => is_factory && !self.is_private_name(container),
            ContainerKind::Enum => !(self.is_private_name(container) && is_static),
            // `enclosingElement.isPublic` for an `ExtensionElement`
            // (unused_local_elements_verifier.dart:870-873): a private
            // name is never public, and a NAMELESS face (`extension on T`)
            // isn't public either — its members stay candidates for the
            // unused family exactly as a private extension's do.
            ContainerKind::Extension => {
                !self.is_private_name(container) && !self.is_nameless(container)
            }
            _ => !(self.is_private_name(container) && (is_static || is_ctor)),
        }
    }

    /// The resolving-super-formal givenness feed (`visitSuperFormalParameter`,
    /// upstream Dart SDK source, unused_local_elements_verifier.dart:377-387):
    /// a `super.x` formal associated against the resolved super constructor
    /// supplies that constructor's parameter at every invocation — the target
    /// joins the given-at-call set exactly as an argument would
    /// (`_addParametersForArguments`). Runs beside
    /// [`Engine::drain_super_formals`] while the owner's entries are still
    /// buffered: an associated named parameter credits the target's written
    /// formal name, a positional one the prefix count the
    /// `unused_element_parameter` face reads.
    pub(crate) fn credit_super_formal_givens(
        &mut self,
        owner: Option<usize>,
        target: Option<StringId>,
    ) {
        let entries: Vec<_> = self
            .pending_super_formals
            .iter()
            .filter(|(entry_owner, ..)| *entry_owner == owner)
            .cloned()
            .collect();
        for (_, super_index, named, name, _, container) in entries {
            // The same resolved super-constructor face `check_super_formal`
            // associates against: the superclass's formals under the drained
            // target's spelling (the unnamed face keys on the superclass
            // name).
            let Some(superclass) = self
                .index
                .get(self.file)
                .and_then(|f| f.container(container))
                .and_then(|c| c.superclass)
            else {
                continue;
            };
            let spelling = target.unwrap_or(superclass);
            let Some(formals) = resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                superclass,
            )
            .and_then(|(super_container, _)| {
                super_container.ctor_formals.get(&spelling).map(Box::as_ref)
            }) else {
                continue;
            };
            if named {
                // The associated lookup spells the public face
                // (`correspondingPublicName`): a `super.x` forwarding the
                // target's written `this._x` credits whichever spelling the
                // target's formal carries.
                let public = self.formal_public_name(name);
                if let Some(matched) = formals
                    .named
                    .iter()
                    .find(|f| f.name == name || f.name == public)
                {
                    self.named_given.insert((spelling, matched.name));
                }
            } else if let Ok(index) = usize::try_from(super_index)
                && formals.positional.len() > index
                && let Ok(count) = u16::try_from(super_index + 1)
            {
                let given = self.positional_given.entry(spelling).or_default();
                *given = (*given).max(count);
            }
        }
    }

    /// The super-initialization pass (`visitSuperConstructorInvocation` +
    /// `_addParametersForArguments`, upstream Dart SDK source,
    /// unused_local_elements_verifier.dart:353-355 and 394-398): every
    /// constructor-initializer `super(...)`/`super.sel(...)` invocation
    /// credits the resolved super constructor's formal parameters as given
    /// at the call — a named argument (`super(e: 1)`) spares the target's
    /// named formal, a positional count the prefix of its positional
    /// formals. The credits land in the given sets the
    /// `unused_element_parameter` face already reads
    /// ([`Engine::named_given`]/[`Engine::positional_given`]), keyed by the
    /// target's spelling exactly as [`Engine::credit_super_formal_givens`]
    /// keys them (the written selector, else the superclass name).
    ///
    /// The pass runs at finalization reading the token table (the parse-side
    /// walk lives in the held invocation lane). Containment is decided by
    /// the balanced-brace backward walk: the innermost `{` still holding the
    /// `super` token opens the class body exactly when the occurrence sits
    /// in an initializer list — a method body's `{` opens closer and fails
    /// the class-header parse, so `super.m(...)` sends never credit here
    /// (the under-report direction). Bounded: the backward walk caps its
    /// distance, the argument walk stops at the matching `)`.
    ///
    /// Returns the resolved super-initialization target per constructor —
    /// `(container, constructor selector) → super target selector` (the
    /// unnamed face keyed by the container spelling, `None` = the unnamed
    /// super constructor) — the map the constructor corresponding-parameter
    /// face reads for `enclosingElement.superConstructor`.
    fn walk_super_invocation_givens(
        &mut self,
    ) -> FxHashMap<(StringId, StringId), Option<StringId>> {
        let mut targets: FxHashMap<(StringId, StringId), Option<StringId>> = FxHashMap::default();
        let tokens = self.tokens.as_slice();
        // The interpolation closers: a `${…}` group's `}` closes a
        // `StringInterpolationExpression` begin and carries no matching `{`
        // in the stream — every brace walk below excludes them so the
        // balance invariant holds (files without interpolations never build
        // the table).
        let table = self.tokens;
        let mut interp_closers: FxHashSet<usize> = FxHashSet::default();
        if tokens
            .iter()
            .any(|t| t.kind == dartq_scanner::TokenKind::StringInterpolationExpression)
        {
            for (index, t) in tokens.iter().enumerate() {
                if t.kind == dartq_scanner::TokenKind::StringInterpolationExpression
                    && let Some(end) = table.matching_end(index)
                {
                    interp_closers.insert(end);
                }
            }
        }
        for i in 0..tokens.len() {
            if tokens[i].kind != dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Super) {
                continue;
            }
            // The invocation shape: `super (` or `super . ident (` — a
            // property access (`super.x = v`) or any other follower carries
            // no argument list.
            let (selector, open_index) = match tokens.get(i + 1).map(|t| &t.kind) {
                Some(dartq_scanner::TokenKind::OpenParen) => (None, i + 1),
                Some(dartq_scanner::TokenKind::Period) => {
                    let Some(sel) = tokens
                        .get(i + 2)
                        .filter(|t| t.kind == dartq_scanner::TokenKind::Identifier)
                    else {
                        continue;
                    };
                    if !matches!(
                        tokens.get(i + 3).map(|t| &t.kind),
                        Some(dartq_scanner::TokenKind::OpenParen)
                    ) {
                        continue;
                    }
                    let sel = self.intern(self.token_text(sel));
                    let selector = (sel != self.intern("new")).then_some(sel);
                    (selector, i + 3)
                }
                _ => continue,
            };
            // The innermost containing `{`: every `}` before an opener nets
            // out against its own `{`, so the first `{` met at depth zero is
            // the opener of the construct holding the invocation.
            let Some(opener) = (|interp: &FxHashSet<usize>| {
                let mut depth = 0usize;
                for j in (0..i).rev() {
                    if i - j > SUPER_WALK_BACKWARD_CAP {
                        return None;
                    }
                    match tokens[j].kind {
                        dartq_scanner::TokenKind::CloseCurly if !interp.contains(&j) => depth += 1,
                        dartq_scanner::TokenKind::OpenCurly => {
                            if depth == 0 {
                                return Some(j);
                            }
                            depth -= 1;
                        }
                        _ => {}
                    }
                }
                None
            })(&interp_closers) else {
                continue;
            };
            // The class-header parse decides the initializer context: a
            // method body or closure opens closer, and its backward header
            // walk hits the enclosing `{` (or a `;`) before any
            // `class`/`enum` keyword.
            let Some(container) = self.class_header_name(tokens, opener) else {
                continue;
            };
            self.credit_super_invocation_arguments(
                tokens,
                &interp_closers,
                open_index,
                container,
                selector,
            );
            // The constructor header between the member anchor and its
            // initializer colon names the owner the target map keys on (`C`,
            // `C.sel`, `C(params)`); a field or method header contributes no
            // entry and the corresponding face keeps its unnamed default.
            if let Some(ctor_sel) = self.ctor_header_selector(tokens, &interp_closers, opener, i) {
                let key = ctor_sel.unwrap_or(container);
                targets.insert((container, key), selector);
            }
        }
        targets
    }

    /// One invocation's givenness credit: the argument group's top-level
    /// shape against the resolved super constructor's formals (the credit
    /// key mirrors [`Engine::credit_super_formal_givens`] — the written
    /// selector, else the superclass spelling). A group whose target
    /// declares no formals (`undefined_constructor_in_initializer`
    /// territory) credits nothing, matching the oracle's unresolved
    /// `correspondingParameter`.
    fn credit_super_invocation_arguments(
        &mut self,
        tokens: &[dartq_scanner::Token],
        interp_closers: &FxHashSet<usize>,
        open_index: usize,
        container: StringId,
        selector: Option<StringId>,
    ) {
        let Some(decl) = self
            .index
            .get(self.file)
            .and_then(|f| f.container(container))
            .and_then(|c| c.superclass)
        else {
            return;
        };
        let key = selector.unwrap_or(decl);
        // The target must declare formals — an invocation of a constructor
        // the superclass lacks resolves no corresponding parameter upstream
        // (`undefined_constructor_in_initializer` owns that face).
        if !resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            decl,
        )
        .is_some_and(|(super_container, _)| super_container.ctor_formals.get(&key).is_some())
        {
            return;
        }
        // The argument walk: top-level commas split the arguments; an
        // identifier at an argument start immediately followed by `:` labels
        // a named argument (a ternary's colon never follows an argument-start
        // identifier, and nested groups sit below depth zero).
        let mut depth = 0u32;
        let mut commas = 0usize;
        let mut saw_argument = false;
        let mut trailing_comma = false;
        let mut at_argument_start = true;
        let mut named: Vec<StringId> = Vec::new();
        let mut k = open_index + 1;
        while let Some(t) = tokens.get(k) {
            match t.kind {
                dartq_scanner::TokenKind::CloseParen if depth == 0 => break,
                dartq_scanner::TokenKind::OpenParen
                | dartq_scanner::TokenKind::OpenSquare
                | dartq_scanner::TokenKind::OpenCurly => {
                    depth += 1;
                    at_argument_start = false;
                }
                dartq_scanner::TokenKind::CloseParen | dartq_scanner::TokenKind::CloseSquare => {
                    depth = depth.saturating_sub(1);
                    at_argument_start = false;
                }
                // An interpolation's `}` carries no `{` in the stream — it
                // moves neither the depth nor the argument shape.
                dartq_scanner::TokenKind::CloseCurly if !interp_closers.contains(&k) => {
                    depth = depth.saturating_sub(1);
                    at_argument_start = false;
                }
                dartq_scanner::TokenKind::Comma if depth == 0 => {
                    commas += 1;
                    at_argument_start = true;
                    trailing_comma = true;
                }
                dartq_scanner::TokenKind::Identifier
                    if depth == 0
                        && at_argument_start
                        && tokens
                            .get(k + 1)
                            .is_some_and(|n| n.kind == dartq_scanner::TokenKind::Colon) =>
                {
                    named.push(self.intern(self.token_text(t)));
                    at_argument_start = false;
                    k += 1;
                }
                _ => {
                    at_argument_start = false;
                    trailing_comma = false;
                }
            }
            if !matches!(t.kind, dartq_scanner::TokenKind::Comma) {
                saw_argument = true;
            }
            k += 1;
        }
        let args = if trailing_comma {
            commas
        } else {
            commas + usize::from(saw_argument)
        };
        let positional = args.saturating_sub(named.len());
        if let Ok(count) = u16::try_from(positional)
            && count > 0
        {
            let given = self.positional_given.entry(key).or_default();
            *given = (*given).max(count);
        }
        for name in named {
            self.named_given.insert((key, name));
        }
    }

    /// The constructor tear-off pass (`visitSimpleIdentifier`'s non-call
    /// arm, upstream Dart SDK source,
    /// unused_local_elements_verifier.dart:311-327): a `C.sel` / `C.new`
    /// / `C<T>.sel` reference that is NOT followed by an invocation group
    /// resolves the constructor element in a non-call position — a
    /// tear-off value, a redirecting target, or a metadata selector — and
    /// the arm marks every formal of that constructor used (the
    /// `functionReferenceIsCall` gate only excuses the
    /// `ConstructorName`-under-`InstanceCreationExpression` shape; every
    /// other read credits the whole list). A `@C.sel` metadata reference
    /// is always non-call (its `ConstructorName` sits under `Annotation`,
    /// never `InstanceCreationExpression`), arguments or not.
    ///
    /// Returns the resolved `(container, selector)` set — `sel` keeps its
    /// written spelling (`new` stays `new`; the reader folds it against
    /// the unnamed face's container key). A redirecting FACTORY's
    /// `= C.sel` target is the one non-call shape that does NOT credit
    /// the whole list upstream (`_matchParameters` pairs the factory's
    /// own formals against the target's — the `redirect_formals_into`
    /// lane already models that pairing), so the walk excludes it by
    /// the `factory` keyword behind the `=`.
    fn walk_ctor_tearoffs(&mut self) -> FxHashSet<(StringId, StringId)> {
        let mut targets: FxHashSet<(StringId, StringId)> = FxHashSet::default();
        let tokens = self.tokens.as_slice();
        let new = self.intern("new");
        for i in 0..tokens.len() {
            if tokens[i].kind != dartq_scanner::TokenKind::Period {
                continue;
            }
            // The selector token: `new` scans as the keyword (a `C.new`
            // reference); every other selector is a plain identifier.
            let sel_token = tokens.get(i + 1);
            let selector = match sel_token.map(|t| &t.kind) {
                Some(dartq_scanner::TokenKind::Identifier) => {
                    Some(self.intern(self.token_text(&tokens[i + 1])))
                }
                Some(dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::New)) => Some(new),
                _ => None,
            };
            let Some(selector) = selector else {
                continue;
            };
            // The class head behind the `.`: a bare identifier, or the
            // identifier in front of a balanced `<…>` group
            // (`C<int>.sel`). The walk answers an index — the token kind
            // gates the arm, so a non-identifier head resolves `None`.
            let mut head_index = i;
            let head_token = loop {
                let Some(index) = head_index.checked_sub(1) else {
                    break None;
                };
                head_index = index;
                match tokens[head_index].kind {
                    dartq_scanner::TokenKind::Identifier => break Some(tokens[head_index]),
                    dartq_scanner::TokenKind::Gt => {
                        // Backward `<…>` skip: a shallow depth walk — the
                        // group's own nested `<`/`>` pairs balance, and a
                        // `>` with no matching `<` ends the head search
                        // (a comparison, never a type-argument group).
                        let mut angle = 1usize;
                        let mut lt = None;
                        while let Some(k) = head_index.checked_sub(1) {
                            match tokens[k].kind {
                                dartq_scanner::TokenKind::Gt => angle += 1,
                                dartq_scanner::TokenKind::Lt => {
                                    angle -= 1;
                                    if angle == 0 {
                                        lt = Some(k);
                                        break;
                                    }
                                }
                                _ => {}
                            }
                            head_index = k;
                        }
                        let Some(lt) = lt else {
                            break None;
                        };
                        head_index = lt;
                    }
                    _ => break None,
                }
            };
            let Some(head_token) = head_token else {
                continue;
            };
            let metadata = head_index > 0
                && matches!(tokens[head_index - 1].kind, dartq_scanner::TokenKind::At);
            // The call gate: `(` immediately after the selector is an
            // invocation (`C.sel(...)`, `new C.sel(...)`, `const C.sel(...)`)
            // — `_addParametersForArguments` owns its per-argument credit
            // through the givens sets. A `<` group first can still close
            // into `(` (`C.sel<int>(...)`) — resolve its follower too.
            // Metadata (`@C.sel(...)`) is exempt: the `Annotation` parent
            // keeps the selector read non-call upstream, arguments or not.
            if !metadata {
                let mut next = i + 2;
                let mut called = matches!(
                    tokens.get(next).map(|t| &t.kind),
                    Some(dartq_scanner::TokenKind::OpenParen)
                );
                if !called
                    && matches!(
                        tokens.get(next).map(|t| &t.kind),
                        Some(dartq_scanner::TokenKind::Lt)
                    )
                    && let Some(gt) = self.tokens.matching_end(next)
                    && matches!(
                        tokens.get(gt + 1).map(|t| &t.kind),
                        Some(dartq_scanner::TokenKind::OpenParen)
                    )
                {
                    called = true;
                }
                if called {
                    continue;
                }
            }
            // The redirecting-factory target (`factory X(...) = C.sel`): the
            // `=` right before the head with `factory` inside the same
            // member header — its `_matchParameters` pairing lives in the
            // `redirect_formals_into` lane, not here. A `var x = C.sel`
            // tear-off keeps its `=` but finds no `factory` behind it. The
            // backward depth scan reads closers as openers.
            let mut factory_target = false;
            if head_index > 0 && matches!(tokens[head_index - 1].kind, dartq_scanner::TokenKind::Eq)
            {
                let mut depth = 0usize;
                let mut k = head_index;
                while let Some(prev) = k.checked_sub(1) {
                    if head_index - prev > SUPER_WALK_BACKWARD_CAP {
                        break;
                    }
                    match tokens[prev].kind {
                        dartq_scanner::TokenKind::CloseCurly
                        | dartq_scanner::TokenKind::CloseParen
                        | dartq_scanner::TokenKind::CloseSquare => depth += 1,
                        dartq_scanner::TokenKind::OpenCurly
                        | dartq_scanner::TokenKind::OpenParen
                        | dartq_scanner::TokenKind::OpenSquare => depth = depth.saturating_sub(1),
                        dartq_scanner::TokenKind::Semicolon | dartq_scanner::TokenKind::Colon
                            if depth == 0 =>
                        {
                            break;
                        }
                        dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Factory)
                            if depth == 0 =>
                        {
                            factory_target = true;
                            break;
                        }
                        _ => {}
                    }
                    k = prev;
                }
            }
            if factory_target {
                continue;
            }
            let head = self.intern(self.token_text(&head_token));
            // The constructor face must actually exist upstream's way: a
            // named selector joins through the container's `constructors`
            // set, `new` through the unnamed face. A prefix segment, a
            // variable head, or a method selector resolves nothing — the
            // under-report posture every unmodelable corner keeps.
            let Some((decl, _)) = resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                head,
            ) else {
                continue;
            };
            let named = decl.constructors.contains(&selector);
            let unnamed = selector == new && decl.has_unnamed_constructor;
            if !(named || unnamed) {
                continue;
            }
            // The element-use credit: a tear-off reads the constructor
            // element (`usedElements.elements` — `C.sel` spares `sel`, the
            // `new` face spares the container-spelled element).
            self.used_names.insert(if named { selector } else { head });
            targets.insert((head, selector));
        }
        targets
    }

    /// The `this.sel(...)` redirecting-initializer pass
    /// (`visitSimpleIdentifier`'s tear-off arm, upstream Dart SDK source,
    /// unused_local_elements_verifier.dart:311-327): a redirecting
    /// `this.sel(...)` initializer resolves `sel` to the target constructor
    /// element, and the read is NOT call-shaped (its parent is a
    /// `ConstructorName` under a `RedirectingConstructorInvocation`, never
    /// an `InstanceCreationExpression`), so the tear-off arm marks every
    /// formal of the target used — regardless of what the argument list
    /// actually supplies. A bare `this(...)` initializer carries no
    /// selector identifier, so the unnamed target receives no credit there
    /// (probed 3.13.1: `_A([int? p]); _A.sel() : this();` reports `p`,
    /// `_B._t([int? p]); _B.sel() : this._t();` spares `p`).
    ///
    /// Containment mirrors [`Self::walk_super_invocation_givens`]: the
    /// innermost `{` still holding the `this` token opens a class body
    /// exactly when the occurrence sits in an initializer list — a method
    /// body's `this.m(...)` send opens closer and fails the class-header
    /// parse, so member sends never credit here. Returns the resolved
    /// `(container, selector)` set the parameter face's non-call arm
    /// reads; `sel` keeps its written spelling (a `this.new(...)` head
    /// names the `new` selector, which the `ctor_formals` lookup spells
    /// as the container — the reader folds it).
    fn walk_this_redirect_targets(&mut self) -> FxHashSet<(StringId, StringId)> {
        let mut targets: FxHashSet<(StringId, StringId)> = FxHashSet::default();
        let tokens = self.tokens.as_slice();
        // The interpolation-closer table mirrors the super walk's: a
        // `${…}` group's `}` closes a `StringInterpolationExpression` begin
        // and carries no matching `{`, so it must not move the brace
        // balance below.
        let mut interp_closers: FxHashSet<usize> = FxHashSet::default();
        if tokens
            .iter()
            .any(|t| t.kind == dartq_scanner::TokenKind::StringInterpolationExpression)
        {
            for (index, t) in tokens.iter().enumerate() {
                if t.kind == dartq_scanner::TokenKind::StringInterpolationExpression
                    && let Some(end) = self.tokens.matching_end(index)
                {
                    interp_closers.insert(end);
                }
            }
        }
        for i in 0..tokens.len() {
            if tokens[i].kind != dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::This) {
                continue;
            }
            // The selector shape `this . sel (` — a bare `this(` carries no
            // selector identifier and credits nothing (the oracle's
            // `this()` arm never reaches a formal element).
            if !matches!(
                tokens.get(i + 1).map(|t| &t.kind),
                Some(dartq_scanner::TokenKind::Period)
            ) {
                continue;
            }
            let Some(sel) = tokens
                .get(i + 2)
                .filter(|t| t.kind == dartq_scanner::TokenKind::Identifier)
            else {
                continue;
            };
            if !matches!(
                tokens.get(i + 3).map(|t| &t.kind),
                Some(dartq_scanner::TokenKind::OpenParen)
            ) {
                continue;
            }
            // The innermost containing `{` decides the initializer
            // context — the same balanced-backward walk the super pass
            // runs (a member body's `{` opens closer and its header walk
            // hits the enclosing `{`/`;` before any `class` keyword).
            let Some(opener) = (|interp: &FxHashSet<usize>| {
                let mut depth = 0usize;
                for j in (0..i).rev() {
                    if i - j > SUPER_WALK_BACKWARD_CAP {
                        return None;
                    }
                    match tokens[j].kind {
                        dartq_scanner::TokenKind::CloseCurly if !interp.contains(&j) => depth += 1,
                        dartq_scanner::TokenKind::OpenCurly => {
                            if depth == 0 {
                                return Some(j);
                            }
                            depth -= 1;
                        }
                        _ => {}
                    }
                }
                None
            })(&interp_closers) else {
                continue;
            };
            let Some(container) = self.class_header_name(tokens, opener) else {
                continue;
            };
            let selector = self.intern(self.token_text(sel));
            // The selector's element credit (`usedElements.elements` — the
            // same join the super-initializer arm makes): `this.sel(...)`
            // reads the target constructor element, so a private-selector
            // target never reports `unused_element`.
            self.used_names.insert(selector);
            targets.insert((container, selector));
        }
        targets
    }

    /// The container name a class-family header writes before its body
    /// opener — the identifier directly behind a `class` keyword, scanned
    /// backward across the extends/with/implements clauses and the primary
    /// constructor's formals. `None` for every other construct (a method
    /// body, a closure, a local class): the header walk stops at the
    /// enclosing `{` or a `;` first.
    fn class_header_name(
        &self,
        tokens: &[dartq_scanner::Token],
        opener: usize,
    ) -> Option<StringId> {
        let start = opener.saturating_sub(SUPER_WALK_BACKWARD_CAP);
        for j in (start..opener).rev() {
            match tokens[j].kind {
                dartq_scanner::TokenKind::OpenCurly
                | dartq_scanner::TokenKind::CloseCurly
                | dartq_scanner::TokenKind::Semicolon => return None,
                dartq_scanner::TokenKind::Identifier
                    if j > 0
                        && matches!(
                            tokens[j - 1].kind,
                            dartq_scanner::TokenKind::Keyword(
                                dartq_scanner::Keyword::Class
                                    | dartq_scanner::Keyword::Enum
                                    | dartq_scanner::Keyword::Mixin
                            )
                        ) =>
                {
                    return Some(self.intern(self.token_text(&tokens[j])));
                }
                _ => {}
            }
        }
        None
    }

    /// The constructor selector the initializer list at `opener` belongs to:
    /// the member header between the last `;`-anchored member start (or the
    /// class body opener) and its initializer colon, read as `C` /
    /// `C.sel` / `C(params)` — `None` for the unnamed face. A header that
    /// parses as a field or method (`int x = …`, `void f() …`) answers
    /// `None` and the caller skips the map entry; the anchor scan keeps to
    /// brace-depth zero so a closure's interior `;` never steals it.
    fn ctor_header_selector(
        &self,
        tokens: &[dartq_scanner::Token],
        interp_closers: &FxHashSet<usize>,
        opener: usize,
        super_at: usize,
    ) -> Option<Option<StringId>> {
        let mut anchor = opener;
        let mut depth = 0i32;
        for j in opener + 1..super_at {
            match tokens[j].kind {
                dartq_scanner::TokenKind::OpenCurly => depth += 1,
                dartq_scanner::TokenKind::CloseCurly if !interp_closers.contains(&j) => depth -= 1,
                dartq_scanner::TokenKind::Semicolon if depth == 0 => anchor = j,
                _ => {}
            }
        }
        // The header run ends at its initializer colon (or a later member's
        // boundary).
        let mut end = anchor + 1;
        depth = 0;
        while end < super_at {
            match tokens[end].kind {
                dartq_scanner::TokenKind::OpenParen
                | dartq_scanner::TokenKind::OpenSquare
                | dartq_scanner::TokenKind::OpenCurly => depth += 1,
                dartq_scanner::TokenKind::CloseParen | dartq_scanner::TokenKind::CloseSquare => {
                    depth -= 1
                }
                dartq_scanner::TokenKind::CloseCurly if !interp_closers.contains(&end) => {
                    depth -= 1
                }
                dartq_scanner::TokenKind::Colon | dartq_scanner::TokenKind::Semicolon
                    if depth == 0 =>
                {
                    break;
                }
                _ => {}
            }
            end += 1;
        }
        let run = tokens.get(anchor + 1..end.min(super_at))?;
        let new = self.intern("new");
        match run {
            // `C(...)` — the unnamed face (a primary constructor's formals
            // ride the same shape).
            [first, second, ..]
                if first.kind == dartq_scanner::TokenKind::Identifier
                    && second.kind == dartq_scanner::TokenKind::OpenParen =>
            {
                Some(None)
            }
            // `C.sel(...)` / `C.new(...)` — the written selector (the `new`
            // fold answers the unnamed face).
            [first, dot, sel, paren, ..]
                if first.kind == dartq_scanner::TokenKind::Identifier
                    && dot.kind == dartq_scanner::TokenKind::Period
                    && sel.kind == dartq_scanner::TokenKind::Identifier
                    && paren.kind == dartq_scanner::TokenKind::OpenParen =>
            {
                let sel = self.intern(self.token_text(sel));
                Some((sel != new).then_some(sel))
            }
            _ => None,
        }
    }

    /// The written text of one scanner token (byte span straight off the
    /// token — the identifier charset is ASCII, the same shape
    /// [`Engine::ident_span_before_dot`] reads).
    fn token_text(&self, token: &dartq_scanner::Token) -> &str {
        let start = token.byte_offset as usize;
        let end = start + token.byte_len as usize;
        self.source.get(start..end).unwrap_or("")
    }

    /// The selector a named primary constructor's declaration site points
    /// at (`class C.named(...)` — `C._n(...)` alike): the site anchors on
    /// the `.` token its `begin` event latched, so the selector is the
    /// identifier immediately following it. `None` for every other site
    /// shape — secondary constructor sites already carry the selector as
    /// their name (the continuation rewrote it), unnamed primary sites
    /// anchor on `(`, and a `.new` primary head names the unnamed face its
    /// `ctor_formals` key already spells (the class name). The
    /// `unused_element_parameter` arm reads it to join the named primary's
    /// site with its formal table and its call-site givens.
    fn primary_ctor_selector(&self, site: &DeclSite) -> Option<StringId> {
        let tokens = self.tokens.as_slice();
        let index = tokens.partition_point(|t| t.utf16_offset < site.range.offset);
        let dot = tokens.get(index)?;
        if dot.utf16_offset != site.range.offset || dot.kind != dartq_scanner::TokenKind::Period {
            return None;
        }
        let selector = tokens.get(index + 1)?;
        if selector.kind != dartq_scanner::TokenKind::Identifier {
            return None;
        }
        Some(self.intern(self.token_text(selector)))
    }

    /// The formal lists of the redirecting factories that target one
    /// constructor — `_matchParameters`' `firstList` face
    /// (unused_local_elements_verifier.dart:102-114): for each factory
    /// constructor site of this file, the `= C.sel` / `= C` tail of its
    /// declaration resolves to a `(container, selector)` pair; when that
    /// pair names the constructor under report, the factory's own written
    /// formals (`factory_formals`) are the correspondence source. The
    /// token walk is local to the declaration's `;`-terminated head —
    /// the only positions a `=` can precede the target are the factory
    /// redirect's own (a default value's `=`/`:` sits inside the formal
    /// group's parens, balanced below the depth the scan reads).
    fn redirect_formals_into(&self, container: StringId, ctor_spelling: StringId) -> Vec<Formals> {
        let tokens = self.tokens.as_slice();
        let mut out: Vec<Formals> = Vec::new();
        for site in &self.decl_sites {
            let DeclKind::Constructor {
                container: factory_container,
            } = site.kind
            else {
                continue;
            };
            if !site.is_factory {
                continue;
            }
            // The head's forward walk: `= C` or `= C.sel` before the
            // declaration's `;`/`{` terminator.
            let mut index = tokens.partition_point(|t| t.utf16_offset < site.range.end());
            let mut depth = 0u32;
            let target = loop {
                let Some(t) = tokens.get(index) else {
                    break None;
                };
                match t.kind {
                    dartq_scanner::TokenKind::OpenParen | dartq_scanner::TokenKind::OpenSquare => {
                        depth += 1
                    }
                    dartq_scanner::TokenKind::OpenCurly => {
                        if depth == 0 {
                            // The factory's own body opened — no redirect.
                            break None;
                        }
                        depth += 1;
                    }
                    dartq_scanner::TokenKind::CloseParen
                    | dartq_scanner::TokenKind::CloseSquare => {
                        depth = depth.saturating_sub(1);
                    }
                    dartq_scanner::TokenKind::CloseCurly => {
                        depth = depth.saturating_sub(1);
                    }
                    dartq_scanner::TokenKind::Semicolon if depth == 0 => break None,
                    dartq_scanner::TokenKind::Eq if depth == 0 => {
                        // `= C` or `= C.sel` — a `new` selector spells the
                        // unnamed face (the class-name key).
                        let Some(head) = tokens.get(index + 1) else {
                            break None;
                        };
                        if head.kind != dartq_scanner::TokenKind::Identifier {
                            break None;
                        }
                        let class = self.intern(self.token_text(head));
                        let selector = match tokens.get(index + 2).map(|t| t.kind) {
                            Some(dartq_scanner::TokenKind::Period) => tokens
                                .get(index + 3)
                                .filter(|t| {
                                    t.kind == dartq_scanner::TokenKind::Identifier
                                        || (t.kind
                                            == dartq_scanner::TokenKind::Keyword(
                                                dartq_scanner::Keyword::New,
                                            ))
                                })
                                .map(|t| self.token_text(t)),
                            _ => None,
                        };
                        break Some((class, selector));
                    }
                    _ => {}
                }
                index += 1;
            };
            let Some((class, selector)) = target else {
                continue;
            };
            let key = selector
                .filter(|s| *s != "new")
                .map(|s| self.intern(s))
                .unwrap_or(class);
            if class != container || key != ctor_spelling {
                continue;
            }
            // The factory's own formal list (`factory_formals`) keyed by
            // its written selector — `factory A()` and the `factory
            // A.new()` head spell the unnamed face under the declaring
            // class's name, matching the `ctor_formals` key convention.
            let Some(decl) = self
                .index
                .get(self.file)
                .and_then(|f| f.container(factory_container))
            else {
                continue;
            };
            let factory_sel = if self.strings.resolve(site.name) == "new" {
                factory_container
            } else {
                site.name
            };
            if let Some(formals) = decl.factory_formals.get(&factory_sel) {
                out.push((**formals).clone());
            }
        }
        out
    }

    /// The overridden instance members behind one member declaration
    /// (`_overriddenElements`, inheritance_manager3.dart `getOverridden`):
    /// the nearest same-named non-static member of each direct super face —
    /// the `extends` face, each `with` mixin, each `implements` interface, a
    /// mixin's `on` constraints — deduplicated by the entry identity the
    /// boundary's projection keeps stable (one member surfaced through
    /// several faces is one overridden executable).
    fn overridden_executables(
        &self,
        container: StringId,
        name: StringId,
    ) -> Vec<(Projected<'a, MemberEntry>, StringId, FileId)> {
        let Some(decl) = self
            .index
            .get(self.file)
            .and_then(|f| f.container(container))
        else {
            return Vec::new();
        };
        let mut faces: Vec<StringId> = Vec::new();
        faces.extend(decl.superclass);
        faces.extend(decl.with_clause.iter().copied());
        faces.extend(decl.implements_clause.iter().copied());
        faces.extend(decl.on_clause.iter().copied());
        let mut out: Vec<(Projected<'a, MemberEntry>, StringId, FileId)> = Vec::new();
        let mut seen: Vec<*const MemberEntry> = Vec::new();
        for face in faces {
            if let Some((entry, face_name, owner)) = self.face_member(container, face, name)
                && seen.iter().all(|ptr| !std::ptr::eq(*ptr, entry.ptr()))
            {
                seen.push(entry.ptr());
                out.push((entry, face_name, owner));
            }
        }
        out
    }

    /// The per-parameter override gate (`_overridesUsedParameter`,
    /// unused_local_elements_verifier.dart:1027-1049): the formal spares
    /// only when an overridden member's CORRESPONDING parameter — the same
    /// name for a named formal, the same positional index otherwise — is
    /// required there, or is itself used: an invocation supplied it (the
    /// spelling-keyed given index; overrides share the member name), or the
    /// overridden executable's own body references it (its declaration
    /// site's parameter-use set, the `_useIdentifierElement` credit — same
    /// library only, exactly where a private member's overrides live). A
    /// formal the overridden member lacks (added in the override) spares on
    /// nothing.
    fn override_parameter_spared(
        &self,
        overridden: &[(Projected<'a, MemberEntry>, StringId, FileId)],
        bare: StringId,
        positional: Option<usize>,
        formal: StringId,
    ) -> bool {
        for (entry, container, owner) in overridden {
            let Some(of) = entry.formals.as_deref() else {
                continue;
            };
            let Some(corresponding) = (match positional {
                Some(index) => of.positional.get(index),
                None => of.named.iter().find(|f| f.name == formal),
            }) else {
                continue;
            };
            if corresponding.required {
                return true;
            }
            let given_used = match positional {
                Some(index) => self
                    .positional_given
                    .get(&bare)
                    .is_some_and(|&n| usize::from(n) > index),
                None => self.named_given.contains(&(bare, corresponding.name)),
            };
            if given_used {
                return true;
            }
            if owner == &self.file
                && self.decl_sites.iter().enumerate().any(|(index, site)| {
                    matches!(
                        &site.kind,
                        DeclKind::Member {
                            container: c,
                            ..
                        } if c == container
                    ) && site.name == bare
                        && self
                            .site_param_uses
                            .get(&index)
                            .is_some_and(|uses| uses.contains(&corresponding.name))
                })
            {
                return true;
            }
        }
        false
    }

    /// The `unused_element_parameter` pass over one executable's
    /// declaration site (`visitFormalParameterList` +
    /// `_isUsedElement`'s parameter arm,
    /// unused_local_elements_verifier.dart:1064-1115): optional parameters
    /// of constructors, methods, and top-level functions that no body use
    /// resolves to and no invocation supplies. The official exemptions in
    /// order: a required parameter, a constructor of a generic class, a
    /// generic executable, a publicly accessible executable, the
    /// super-parameter corresponding faces (a constructor's resolved super
    /// constructor carrying a required corresponding parameter), and the
    /// per-parameter override gate (a method whose overridden member's
    /// corresponding parameter is required or used).
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    pub(crate) fn report_optional_parameters(
        &mut self,
        site_index: usize,
        site: &DeclSite,
        super_ctor_targets: &FxHashMap<(StringId, StringId), Option<StringId>>,
        noncall_reads: &FxHashSet<(StringId, StringId)>,
    ) {
        let (member_kind, container) = match &site.kind {
            DeclKind::Member {
                kind, container, ..
            } => (Some(*kind), Some(*container)),
            DeclKind::Constructor { container } => (None, Some(*container)),
            _ => (None, None),
        };
        if matches!(member_kind, Some(MemberKind::Field | MemberKind::Setter)) {
            return;
        }
        // A named PRIMARY constructor's site keeps the container spelling —
        // its `begin` token anchors on the `.` (never rewritten by a
        // method-continuation, which secondary heads get) — while the
        // formal table and the call-site givens key on the selector
        // (`class _A._n(...)` files under `_n`). Derive that selector from
        // the site's own `.`+identifier pair; body-constructor sites already
        // carry the selector as their name, and unnamed primary sites anchor
        // on `(` so both fall through unchanged.
        let ctor_spelling = if matches!(site.kind, DeclKind::Constructor { .. }) {
            self.primary_ctor_selector(site).unwrap_or(site.name)
        } else {
            site.name
        };
        let augment_ctor_formals = |engine: &Self, c: StringId| -> Option<Formals> {
            // An `augment` constructor's written list never joins
            // `ctor_formals` (the collector keeps the introductory
            // declaration's table intact) — its own `augmentations` row is
            // the table upstream's `visitFormalParameterList` walks (the
            // fragment's `parameterFragments` list). The conversion reads
            // `required_kw` for the named face's required bit and skips
            // unnamed recovery formals.
            engine
                .index
                .get(engine.file)
                .and_then(|f| {
                    f.augmentations().iter().find(|a| {
                        a.kind == crate::model::AugKind::Constructor
                            && a.container == Some(c)
                            && a.name == ctor_spelling
                    })
                })
                .map(|a| {
                    let to_formal = |f: &crate::model::AugmentFormal| -> Option<Formal> {
                        Some(Formal {
                            name: f.name?,
                            ty: f.ty.unwrap_or(Ty::UNKNOWN),
                            required: f.required_positional || f.required_kw.is_some(),
                            covariant: f.covariant_kw.is_some(),
                            default_value: None,
                        })
                    };
                    Formals {
                        positional: a
                            .formals
                            .iter()
                            .filter(|f| !f.named)
                            .filter_map(to_formal)
                            .collect(),
                        named: a
                            .formals
                            .iter()
                            .filter(|f| f.named)
                            .filter_map(to_formal)
                            .collect(),
                    }
                })
        };
        let formals = match container {
            Some(c) => self
                .index
                .get(self.file)
                .and_then(|f| f.container(c))
                .and_then(|x| {
                    if matches!(site.kind, DeclKind::Constructor { .. }) {
                        // A factory's written list lives in the separate
                        // `factory_formals` lane — a private named factory's
                        // optional formals report exactly like a generative
                        // constructor's upstream (the enclosing element is a
                        // ConstructorElement either way).
                        if site.is_factory {
                            x.factory_formals.get(&ctor_spelling).map(Box::as_ref)
                        } else {
                            x.ctor_formals.get(&ctor_spelling).map(Box::as_ref)
                        }
                    } else {
                        x.members.get(&site.name).and_then(|m| m.formals.as_deref())
                    }
                })
                .map(Formals::clone)
                // An unnamed extension keeps no container entry — its
                // member tables live on the nameless application face
                // (`close_container` moves them out of the container open).
                // The face's own member entry answers here so the
                // parameter pass reads the same written list upstream's
                // `visitFormalParameterList` walks.
                .or_else(|| {
                    if matches!(site.kind, DeclKind::Member { .. }) {
                        self.index.get(self.file).and_then(|f| {
                            f.extension_faces()
                                .iter()
                                .find(|face| {
                                    face.name.is_none()
                                        && self.is_nameless(c)
                                        && face.members.contains_key(&site.name)
                                })
                                .and_then(|face| face.members.get(&site.name))
                                .and_then(|m| m.formals.as_deref())
                                .map(Formals::clone)
                        })
                    } else {
                        None
                    }
                })
                .or_else(|| {
                    (site.is_augment && matches!(site.kind, DeclKind::Constructor { .. }))
                        .then(|| augment_ctor_formals(self, c))
                        .flatten()
                }),
            None => self
                .index
                .get(self.file)
                .and_then(|f| f.decl(site.name))
                .and_then(|d| d.formals.as_deref().cloned()),
        };
        let Some(formals) = formals else {
            return;
        };
        // The 47839 exemptions: a constructor of a generic class, or an
        // executable with its own type parameters, is never reported.
        let executable = self.index.get(self.file).and_then(|f| {
            container.and_then(|c| f.container(c)).map(|x| {
                (
                    x.type_params > 0,
                    x.members.get(&site.name).is_some_and(|m| m.type_params > 0),
                )
            })
        });
        let (class_generic, member_generic) = match (site.kind, executable) {
            (DeclKind::Constructor { .. }, Some((class_generic, _))) => (class_generic, false),
            (_, Some((class_generic, member_generic))) => (class_generic, member_generic),
            _ => (false, false),
        };
        if member_generic || (class_generic && matches!(site.kind, DeclKind::Constructor { .. })) {
            return;
        }
        // The accessibility and override gates. Constructors keep the
        // blanket super-name pardon (the walk cannot see per-parameter
        // correspondence — the under-report direction) plus the
        // super-initialization corresponding face below; methods run the
        // official per-parameter override gate in the loops. The
        // accessibility face keys on the executable's own spelling — a
        // public class's private-selector constructor (`A._n`) is NOT
        // publicly accessible upstream, so the derived `ctor_spelling`
        // answers where the container-keyed site name could not.
        //
        // An unnamed extension's member is never publicly accessible:
        // `_isPubliclyAccessible`'s extension arm answers
        // `enclosingElement.isPublic` outright (the enclosing element is
        // an `ExtensionElement`, never an `InterfaceElement`), and a
        // nameless extension is never public
        // (`ExtensionElement.isPublic` requires a name — element.dart:
        // 3264-3267, upstream Dart SDK source), so its parameters report
        // exactly like a private extension's. The exemption rides here
        // and not in the shared helper because the member/constructor
        // faces own their unnamed-extension handling in the face loop —
        // the helper's container-miss default answers public there, and
        // a flip would double-report the element row the loop already
        // emits. The nameless-extension membership check keeps a
        // recovered nameless class/mixin frame (whose null name maps to
        // PUBLIC through the fragment, unlike an extension) on the
        // helper's public default.
        let unnamed_extension_member = match &site.kind {
            DeclKind::Member { container, .. } => {
                self.is_nameless(*container)
                    && self.index.get(self.file).is_some_and(|f| {
                        f.extension_faces().iter().any(|face| {
                            face.name.is_none() && face.members.contains_key(&site.name)
                        })
                    })
            }
            _ => false,
        };
        if !unnamed_extension_member
            && self.is_publicly_accessible(ctor_spelling, site.kind, site.is_factory)
        {
            return;
        }
        let overridden = match site.kind {
            DeclKind::Member { container, .. } => self.overridden_executables(container, site.name),
            _ => Vec::new(),
        };
        if overridden.is_empty()
            && matches!(site.kind, DeclKind::Constructor { .. })
            && let Some(c) = container
            && matches!(self.walk_super(c, ctor_spelling), Walk::Found(_))
        {
            return;
        }
        // The super-initialization corresponding face (`_isUsedElement`'s
        // ConstructorElement block, unused_local_elements_verifier.dart
        // :932-946): the resolved super constructor — the written
        // `super(...)`/`super.sel(...)` target, else the unnamed face —
        // spares the formal whose corresponding parameter is required
        // there. A formal the super constructor lacks spares on nothing.
        let super_formals: Option<Formals> = match site.kind {
            DeclKind::Constructor { .. } => container.and_then(|c| {
                let superclass = self
                    .index
                    .get(self.file)
                    .and_then(|f| f.container(c))
                    .and_then(|d| d.superclass)?;
                let target = super_ctor_targets
                    .get(&(c, ctor_spelling))
                    .or_else(|| super_ctor_targets.get(&(c, c)))
                    .copied()
                    .flatten();
                let key = target
                    .filter(|s| *s != self.intern("new"))
                    .unwrap_or(superclass);
                resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    superclass,
                )
                .and_then(|(super_container, _)| super_container.ctor_formals.get(&key))
                .map(|f| (**f).clone())
            }),
            _ => None,
        };
        // A redirecting factory's target receives only the parameters the
        // factory's own formal list declares — `_matchParameters` pairs
        // them positionally and by name against the target's list and
        // credits the matched target formals alone
        // (unused_local_elements_verifier.dart:102-114/:458-498): a
        // `factory T.named() = T` forwards nothing, so `T`'s optional
        // formals still report.
        let redirect_sources: Vec<Formals> = if let DeclKind::Constructor { container } = site.kind
        {
            self.redirect_formals_into(container, ctor_spelling)
        } else {
            Vec::new()
        };
        // Upstream `visitSimpleIdentifier`'s tear-off arm
        // (unused_local_elements_verifier.dart:318-326): an executable
        // element read in a NON-call position — a method/function/
        // constructor tear-off, or a `super.x()`/`this.x()` initializer
        // the AST never shapes as an `InstanceCreationExpression` —
        // credits every formal it declares. A real invocation credits
        // only the supplied arguments (`_addParametersForArguments`),
        // which the `positional_given`/`named_given` sets already carry.
        // The call shape is the `(` immediately following the reference's
        // name span; every other follower is a non-call read.
        let invocation_after = |engine: &Self, range: SourceRange| -> bool {
            let tokens = engine.tokens.as_slice();
            let index = tokens.partition_point(|t| t.utf16_offset < range.end());
            matches!(
                tokens.get(index).map(|t| t.kind),
                Some(dartq_scanner::TokenKind::OpenParen)
            )
        };
        let all_params_used = match site.kind {
            DeclKind::Member { container, .. } => self.report.member_refs.iter().any(|r| {
                r.kind == MemberRefKind::Invoked
                    && r.target_file == self.file
                    && r.target_container == container
                    // The zero-span `credit_constructor_selector` edge
                    // credits a CONSTRUCTOR element — never this member —
                    // and its synthetic (0,0) range would read non-call.
                    && r.range.length > 0
                    && r.target_name == site.name
                    && !invocation_after(self, r.range)
            }),
            DeclKind::Constructor { container } => {
                // The non-call arm's three read faces, all crediting every
                // formal upstream (unused_local_elements_verifier.dart
                // `visitSimpleIdentifier`:311-327 — every
                // non-`InstanceCreationExpression` executable read):
                //   * `member_refs` real-range edges — a resolved `C.sel`
                //     /`C.new` access the collector recorded. The
                //     zero-length `credit_constructor_selector` edge is
                //     excluded: it fires on `end_constructor_reference`,
                //     which the listener also emits for `new C.sel(` /
                //     `const C.sel(` call sites — a call-shape read that
                //     must credit only the supplied arguments, not the
                //     whole list, so its always-`token[0]` range cannot
                //     gate `invocation_after`.
                //   * `noncall_reads` — the token-walked union of the
                //     `this.sel(...)` redirect targets and the bare
                //     `C.sel` / `C.new` / `C<T>.sel` / `@C.sel` tear-off
                //     reads (`walk_ctor_tearoffs`), each resolved against
                //     the container's real constructor set.
                //   * `super_ctor_targets` — the `super.sel(...)`
                //     initializer invocations whose selector names this
                //     constructor.
                let noncall_ref = self.report.member_refs.iter().any(|r| {
                    r.kind == MemberRefKind::Invoked
                        && r.target_file == self.file
                        && r.target_container == container
                        && r.range.length > 0
                        && (r.target_name == site.name || r.target_name == ctor_spelling)
                        && !invocation_after(self, r.range)
                });
                let new_sel = self.intern("new");
                let noncall_targeted = noncall_reads.iter().any(|(c, sel)| {
                    *c == container
                        && (*sel == ctor_spelling
                            || *sel == site.name
                            // `this.new(...)` / `C.new` name the unnamed
                            // face — `sel` keeps the written spelling while
                            // the site's `ctor_spelling` answers the
                            // container.
                            || (*sel == new_sel && ctor_spelling == container))
                });
                noncall_ref
                    || noncall_targeted
                    || super_ctor_targets.iter().any(|((sub, _sub_key), sel)| {
                        sel.is_some_and(|sel| {
                            sel == ctor_spelling
                                && self
                                    .index
                                    .get(self.file)
                                    .and_then(|f| f.container(*sub))
                                    .and_then(|d| d.superclass)
                                    == Some(container)
                        })
                    })
            }
            _ => self.refs.iter().any(|r| {
                r.name == site.name
                    && matches!(r.target, RefTarget::Decl { .. })
                    && !invocation_after(self, r.range)
            }),
        };
        if all_params_used {
            return;
        }
        let bare = self.bare_name(ctor_spelling);
        // Cloned: the emit below mutates the report while the set is read.
        let body_uses = self.site_param_uses.get(&site_index).cloned();
        let given_positional = self.positional_given.get(&bare).copied().unwrap_or(0);
        let name_span = |engine: &Self, formal: StringId| -> SourceRange {
            engine
                .site_params
                .get(&site_index)
                .and_then(|ps| ps.iter().find(|(n, _)| *n == formal).map(|(_, r)| *r))
                .unwrap_or(site.range)
        };
        for (index, formal) in formals.positional.iter().enumerate() {
            if formal.required {
                continue;
            }
            let referenced = body_uses.as_ref().is_some_and(|u| u.contains(&formal.name));
            if referenced || given_positional > index as u16 {
                continue;
            }
            // The super-initialization corresponding face (constructors) and
            // the per-parameter override gate (methods).
            let spared_by_super = super_formals
                .as_ref()
                .and_then(|f| f.positional.get(index))
                .is_some_and(|f| f.required);
            // `_matchParameters`' positional leg: a redirecting factory
            // carrying a formal at this index supplies the target's.
            let spared_by_redirect = redirect_sources.iter().any(|f| f.positional.len() > index);
            if spared_by_super
                || spared_by_redirect
                || self.override_parameter_spared(&overridden, bare, Some(index), formal.name)
            {
                continue;
            }
            let text = self.strings.resolve(formal.name).to_string();
            let range = name_span(self, formal.name);
            self.stage_usage(SemanticCode::UnusedElementParameter, range, &text);
        }
        for formal in &formals.named {
            if formal.required {
                continue;
            }
            let referenced = body_uses.as_ref().is_some_and(|u| u.contains(&formal.name));
            if referenced || self.named_given.contains(&(bare, formal.name)) {
                continue;
            }
            let spared_by_super = super_formals.as_ref().is_some_and(|f| {
                f.named_formal(formal.name)
                    .or_else(|| f.named_formal(self.formal_public_name(formal.name)))
                    .is_some_and(|f| f.required)
            });
            // `_matchParameters`' named leg: a redirecting factory formal
            // of the same name supplies the target's.
            let spared_by_redirect = redirect_sources
                .iter()
                .any(|f| f.named_formal(formal.name).is_some());
            if spared_by_super
                || spared_by_redirect
                || self.override_parameter_spared(&overridden, bare, None, formal.name)
            {
                continue;
            }
            let text = self.strings.resolve(formal.name).to_string();
            let range = name_span(self, formal.name);
            self.stage_usage(SemanticCode::UnusedElementParameter, range, &text);
        }
    }

    /// The `unused_import` pass: an import with a resolved, existing,
    /// non-`dart:core` target reports when it contributed no used element
    /// (`generateUnusedImportWarnings`, imports_verifier.dart:228-268).
    /// Edge matching is positional and only when the directive count agrees
    /// with the library's edge count — a mismatch suppresses (under-report).
    /// The engine's merge prepends each library's implicit `dart:core`
    /// import in front of the written directives; the alignment excludes
    /// that prelude edge so the gate compares written directives only.
    pub(crate) fn report_unused_imports(&mut self) {
        if self.imports.is_empty() {
            return;
        }
        // The library-wide gate (upstream Dart SDK source,
        // library_analyzer.dart `_hasDiagnosticReportedThatPreventsImportWarnings`):
        // a resolution failure anywhere in the unit skips the whole
        // `ImportsVerifier` pass — the undefined name might have come from
        // one of the imports, so no `unused_import`/`unused_shown_name`
        // row may second-guess it (probed 3.13.1: `import 'a.dart' as p;
        // fa();` reports `undefined_function` alone, the unused prefix
        // import stays silent; `import 'a.dart'; p.fa();` likewise keeps
        // the wholly unused import silent beside `undefined_identifier`).
        if self
            .report
            .diagnostics
            .iter()
            .any(|row| IMPORT_WARNING_GATE.contains(&row.code.name))
        {
            return;
        }
        let mut edges = self.graph.import_edges(self.lib);
        let runs = self.graph.import_runs(self.lib);
        if runs.is_empty() {
            // The implicit prelude edge is exactly one leading entry (the
            // merge inserts it before every written edge), targets
            // `dart:core`, and carries no prefix, no deferral, and no
            // combinator clauses. A written `import 'dart:core';` is
            // payload-identical but sits in written order behind it; the
            // one-extra count keeps the skip honest, and the harness graph
            // (no prelude) aligns unchanged.
            if edges.len() == self.imports.len() + 1
                && edges.first().is_some_and(|edge| {
                    edge.prefix.is_none()
                        && !edge.deferred
                        && edge.combinators.written().next().is_none()
                        && edge
                            .target
                            .is_some_and(|t| self.graph.uri(t) == "dart:core")
                })
            {
                edges = &edges[1..];
            }
        } else {
            // Enhanced-parts per-file alignment (upstream
            // `generateUnusedImportWarnings` walks `fileAnalysis.unit.
            // directives` — THIS file's written imports — and
            // `importsTracking` credits only the elements that file's own
            // imports resolved): `self.imports` aligns against this file's
            // run alone, never a sibling part's edges. The runs slice out
            // the shared `dart:core` prelude by construction (it sits in
            // the library-wide group ahead of every run), so a part-file
            // pass no longer dies on the count gate.
            let Some(pos) = runs.iter().position(|&(file, _)| file == self.file) else {
                return;
            };
            let start = runs[pos].1 as usize;
            let end = runs
                .get(pos + 1)
                .map_or(edges.len(), |&(_, next)| next as usize);
            edges = &edges[start..end];
        }
        if edges.len() != self.imports.len() {
            return;
        }
        let used: Vec<StringId> = self
            .used_names
            .iter()
            .chain(&self.import_used)
            .copied()
            .collect();
        // The per-prefix element evidence ([`Self::prefix_use_evidence`]):
        // the prefixed credit decides on it, never on the raw spelling
        // pool — a local or member merge sharing the prefix's spelling
        // would otherwise hold the import open (the shadow corpus).
        let prefix_evidence = self.prefix_use_evidence();
        // The extension-credit pool: `used` minus the spellings only
        // non-demonstrating member accesses contributed (see
        // [`Self::extension_credit_names`]).
        let credit: Vec<StringId> = self.extension_credit_names(&used);
        // The library's own top-level names — the defining unit and every
        // part ([`LibraryGraph::unit_files`]). A name the library declares
        // resolves LOCALLY in the library scope: upstream's lookup answers
        // the local element before any import is consulted (`PrefixScope`
        // holds imported elements alone), so a local spelling never credits
        // an import through a bare name overlap (the local class shadows
        // the imported one and the import stays unused — `unsorted/
        // extends_test`; the same shadow through a part unit is
        // `part/of_multiple_libs_test`).
        let local_names: FxHashSet<StringId> = self
            .graph
            .unit_files(self.lib)
            .iter()
            .filter_map(|&file| self.index.get(file))
            .flat_map(|elements| elements.declarations().map(|(name, _)| name))
            .collect();
        // Names attributed to a prefixed access (`p.A`, `one.A()`) credit
        // the prefix's edge alone — upstream's `PrefixScope.lookup` records
        // the element on the prefixed import, never on a sibling unprefixed
        // edge. `resolve_member_row`/`push_type_use` flatten both paths
        // into `used_names`, so the unprefixed arm must check whether a
        // `used` spelling resolved through THIS edge — the resolved
        // declaration's file must sit in the edge's target library
        // (`RefTarget::Decl.file`), not a sibling's (`lib1`'s `File` never
        // credits `dart:io`'s edge).
        let prefix_recv_offsets: FxHashSet<u32> = self
            .member_reads
            .iter()
            .filter(|read| matches!(read.recv, Receiver::Prefix { .. }))
            .map(|read| read.offset)
            .collect();
        // The literal-receiver applications (`ImportsTracking.
        // notifyExtensionUsed`, scope.dart:308-321): upstream marks the
        // applied extension element used while the resolver runs, so its
        // contributing import stays used. A literal receiver's application
        // folds into an operator/invoke intent on a `Value` row — pass 2
        // leaves it inert and no member spelling reaches `used_names` — so
        // the credit derives from the token shape directly: the literal's
        // static type head pairs with the applied member spelling, the
        // same (receiver type, member) join the resolver's applicability
        // walk made.
        let literal_apps = self.literal_extension_applications();
        // `bare_decls`: each spelling → the files its bare accesses resolved
        // to. `refs` carries the `Decl{file}` directly for member-row and
        // bare-ref resolutions; `type_uses` resolve through the top-level
        // namespace — a `namespaces.lookup` re-ask gives the same file the
        // push-time walk found (the bare arm, never `PrefixScope`).
        let mut bare_decls: FxHashMap<StringId, FxHashSet<FileId>> = FxHashMap::default();
        for r in &self.refs {
            let RefTarget::Decl { file } = r.target else {
                continue;
            };
            if prefix_recv_offsets.contains(&r.range.offset) {
                continue;
            }
            bare_decls.entry(r.name).or_default().insert(file);
        }
        for &(range, name) in &self.type_uses {
            if self
                .prefix_head_before(range.offset)
                .or_else(|| self.prefix_head_at(range.offset))
                .is_some()
            {
                continue;
            }
            // A bare type-use resolves through the top-level namespace of
            // the FILE under diagnosis — the same per-file combined import
            // scope the push-time `type_resolution` walked (a part's own
            // imports are invisible to the defining unit's scope, and the
            // oracle walks `fileAnalysis.unit`'s per-file scope chain).
            if let Lookup::Found(NamespaceEntry::Decl(file)) = self
                .namespaces
                .lookup_in_file(self.graph, self.lib, Some(self.file), name)
            {
                bare_decls.entry(name).or_default().insert(file);
            }
        }
        // A resolved annotation credits its import the same way (the
        // `check_annotation` use-face credit — `used_names` ∪
        // `annotation_uses`), but the metadata events carry no row, so the
        // head never reaches `refs`/`type_uses` and the element gate above
        // would drop the credit a genuinely used `@UseResult` import owns.
        // The same per-file scope answers the declaring file exactly as it
        // does for the type-use loop.
        for &name in &self.annotation_uses {
            if let Lookup::Found(NamespaceEntry::Decl(file)) = self
                .namespaces
                .lookup_in_file(self.graph, self.lib, Some(self.file), name)
            {
                bare_decls.entry(name).or_default().insert(file);
            }
        }
        for (index, edge) in edges.iter().enumerate() {
            // Copied first: the emit below mutates the report.
            let (uri_text, uri_range, prefix, shown) = {
                let site = &self.imports[index];
                (
                    site.uri_text.clone(),
                    site.uri_range,
                    site.prefix,
                    site.shown.clone(),
                )
            };
            let Some(target) = edge.target else {
                // The unresolved-URI face (upstream
                // `DirectiveUriWithLibraryImpl.relativeUriString` = `''` —
                // the `unused_import` argument renders the URI the parser
                // recovered, not the resolution): a written URI that
                // resolves to no library still reports `unused_import` —
                // the unquoted-imports empty-path family, `import ''`, and
                // the directive-recovery shape. A named-but-missing target
                // keeps the `isOriginNotExistingFile` skip: the
                // `uri_does_not_exist` face already reports the directive.
                if uri_text.is_empty() {
                    self.stage_usage(SemanticCode::UnusedImport, uri_range, &uri_text);
                }
                continue;
            };
            if self.graph.uri(target) == "dart:core" {
                continue;
            }
            let is_used = match prefix {
                Some(prefix) => match prefix_evidence.get(&prefix) {
                    Some((attributed, bare)) => {
                        *bare
                            || attributed.iter().any(|&name| {
                                edge.combinators.allows(name)
                                    && self.graph.exported(target, name).is_some()
                            })
                            // The implicit-extension credit is prefix-blind
                            // upstream: `notifyExtensionUsed` walks the
                            // tracking map per prefix scope too (scope.dart
                            // :204-321) — an extension reached through a
                            // prefixed import applies implicitly with or
                            // without a bare prefix use (`accessibility_
                            // A01_t02`'s `"".id1` over `as p`, the
                            // wildcard-prefix corpus `wildcard_variables/
                            // import/import_test`), so the applied face's
                            // members spare this edge exactly as the
                            // unprefixed arm's do.
                            || self.import_credits_extension_member(edge, target, &credit)
                            || self.literal_extension_credit(edge, target, &literal_apps)
                    }
                    // No evidence record for the spelling — keep the raw
                    // posture (the under-report direction).
                    None => {
                        self.used_names.contains(&prefix)
                            || self.import_used.contains(&prefix)
                            || self.literal_extension_credit(edge, target, &literal_apps)
                    }
                },
                None => {
                    // The element-attributed hit: the resolved declaration's
                    // file must sit in this edge's target library — a bare
                    // `File` resolving to `lib1.dart`'s element never
                    // credits `dart:io`'s edge (upstream's `PrefixScope`/
                    // `ImportScope` records the lookup on the resolving
                    // edge alone, scope.dart:638-655).
                    let hit = used.iter().find(|&&n| {
                        edge.combinators.allows(n)
                            && !local_names.contains(&n)
                            && self.graph.exported(target, n).is_some()
                            && bare_decls.get(&n).is_some_and(|files| {
                                // The resolved declaration's file must be
                                // the file this edge's target library
                                // declares the spelling in — a bare `File`
                                // resolving to `lib1.dart`'s element never
                                // credits `dart:io`'s edge. `graph.local`
                                // reads the target's own declaration slot
                                // (`LocalSlot::Decl(file)`) — `library_of_
                                // file`'s units map cannot see aux files
                                // that never registered a unit.
                                matches!(
                                    self.graph.local(target, n),
                                    dartq_resolution::LocalLookup::Found(
                                        decl_file
                                    ) if files.contains(&decl_file)
                                )
                            })
                    });
                    let credited = self.import_credits_extension_member(edge, target, &credit)
                        || self.literal_extension_credit(edge, target, &literal_apps);
                    hit.is_some() || credited
                }
            };
            if !is_used {
                self.stage_usage(SemanticCode::UnusedImport, uri_range, &uri_text);
                continue;
            }
            // The shown-name face (`generateUnusedShownNameHints`,
            // imports_verifier.dart:270-324): runs only when the whole
            // import is used — a wholly unused import keeps its single
            // `unused_import` row — and reports each shown name this
            // import's namespace resolved but never credited. The
            // resolution gate (the official `element == null` skip) leaves
            // an undefined shown name to its own code.
            for (name, range) in shown {
                // The resolution gate (the official `element == null`
                // skip): an undefined shown name keeps its own code. The
                // library-wide import-warning gate above already silenced
                // every list beside a resolution failure.
                if self.graph.exported(target, name).is_none() {
                    continue;
                }
                let name_used = (self.used_names.contains(&name)
                    || self.import_used.contains(&name))
                    && edge.combinators.allows(name);
                if !name_used {
                    let text = self.strings.resolve(name).to_string();
                    self.stage_usage(SemanticCode::UnusedShownName, range, &text);
                }
            }
        }
    }

    /// The `deprecated_member_use` face over `dart:` SDK libraries
    /// (`element_usage_detector.dart:769-775` for imports, `:256` for
    /// exports): each written `dart:` directive whose target carries an
    /// SDK-level `@Deprecated` reports once at the whole-directive anchor
    /// (the upstream `usageRange: node.sourceRange` — the ImportDirective
    /// or ExportDirective, not the URI literal). The same-package rule
    /// never applies — the `dart:` namespace has no package of its own —
    /// but the URI must resolve: untabulated names (`web_sql`) report
    /// nothing, matching the upstream resolved-library gate.
    ///
    /// The import-side directive span is derived lazily at emit time:
    /// [`ImportSite`] carries no directive field (the sibling lane's
    /// `types.rs`/`scopes_bodies.rs` boundary), so the anchor walks
    /// `self.tokens` backward from `uri_range` to the `import` keyword and
    /// forward to `;`. The export side arrives pre-captured on
    /// [`ExportUseFace`].
    pub(crate) fn report_deprecated_sdk_libraries(&mut self) {
        for index in 0..self.imports.len() {
            // Copied first: the emit below mutates the report while the
            // site is read.
            let (uri_text, uri_range) = {
                let site = &self.imports[index];
                (site.uri_text.clone(), site.uri_range)
            };
            let Some(range) =
                self.directive_range_around(uri_range, dartq_scanner::Keyword::Import)
            else {
                continue;
            };
            self.emit_deprecated_sdk_library(&uri_text, range);
        }
        for index in 0..self.exports.len() {
            let (uri_text, range) = {
                let face = &self.exports[index];
                (face.uri_text.clone(), face.directive_range)
            };
            // A zero-length span is a face built outside the listener — the
            // capture never ran, so there is no anchor to report at.
            if range.length == 0 {
                continue;
            }
            self.emit_deprecated_sdk_library(&uri_text, range);
        }
    }

    /// One `dart:` directive's library-level deprecation row, when the
    /// written URI names a deprecated SDK library: the
    /// `deprecatedMemberUse` / `deprecatedMemberUseWithMessage` shapes
    /// (upstream Dart SDK source, `diagnostic.g.dart:3940/:4038`, both
    /// named `deprecated_member_use`) with the whole-directive anchor.
    fn emit_deprecated_sdk_library(&mut self, uri_text: &str, range: SourceRange) {
        const CODE: dartq_diagnostics::CodeInfo = dartq_diagnostics::CodeInfo::new(
            "deprecated_member_use",
            "DEPRECATED_MEMBER_USE",
            dartq_diagnostics::DiagnosticType::Hint,
            true,
        );
        const CORRECTION: &str =
            "Try replacing the use of the deprecated member with the replacement.";
        let Some(name) = uri_text.strip_prefix("dart:") else {
            return;
        };
        let (library, _) = name.split_once('/').unwrap_or((name, ""));
        let Some(tag) = deprecated_sdk_library(library) else {
            return;
        };
        let message = normalize_deprecation(tag).map_or_else(
            || format!("'{uri_text}' is deprecated and shouldn't be used."),
            |tail| format!("'{uri_text}' is deprecated and shouldn't be used. {tail}"),
        );
        self.report.usage.push(Diagnostic::new(
            CODE.dtype.severity(),
            CODE,
            message,
            self.file,
            range,
            Some(CORRECTION.to_string()),
        ));
    }

    /// Derives the whole-directive span around a URI-literal range: walks
    /// `self.tokens` backward from the token at `uri_range.offset` to the
    /// `kw` keyword, forward to `;`, and returns `keyword start →`
    /// `semicolon end`. `None` on any structural miss (recovery shapes —
    /// the under-report direction matches the URI site's own gate).
    fn directive_range_around(
        &self,
        uri_range: SourceRange,
        kw: dartq_scanner::Keyword,
    ) -> Option<SourceRange> {
        let tokens = self.tokens.as_slice();
        // Tokens are in source order, so the URI token is the partition
        // point at its offset — the per-import lookup stays logarithmic
        // rather than a fresh whole-table scan.
        let uri_index = tokens.partition_point(|t| t.utf16_offset < uri_range.offset);
        if tokens.get(uri_index)?.utf16_offset != uri_range.offset {
            return None;
        }
        // Both searches stop at the directive's own boundary: `rposition`
        // walks backward from the URI (the nearest preceding keyword is
        // this directive's), `position` walks forward (the nearest `;`).
        let kw_index = tokens[..uri_index]
            .iter()
            .rposition(|t| t.kind == dartq_scanner::TokenKind::Keyword(kw))?;
        let semi_index = tokens[uri_index..]
            .iter()
            .position(|t| t.kind == dartq_scanner::TokenKind::Semicolon)
            .map(|i| uri_index + i)?;
        let start = tokens[kw_index].utf16_offset;
        // `;` is one UTF-16 unit — the directive's end is its offset + 1.
        let end = tokens[semi_index].utf16_offset + 1;
        Some(SourceRange {
            offset: start,
            length: end - start,
        })
    }

    /// The `notifyExtensionUsed` credit at the import site (scope.dart
    /// :204-321): a used member of an extension this import provides marks
    /// the import used even when no top-level name from it resolves —
    /// `x.m`/`m` on a receiver the extension covers resolves through
    /// implicit application, and the oracle credits the import that
    /// contributed the extension. The name-level model cannot re-run
    /// applicability at the directive gate (the receiver's static type is
    /// gone by the usage pass), so the credit joins the import's exported
    /// extension faces' member spellings against the unit's used set — the
    /// same granularity the top-level `exported(target, n)` arm keeps, and
    /// the under-report direction the resolved-only `import_used` credit
    /// (`credit_extension_import`) leaves behind.
    ///
    /// The `show` combinator gates the extension's own export name — a
    /// `hide Ext` import contributes the face to no scope upstream, so its
    /// members credit nothing here either. An unnamed extension carries no
    /// export entry (names only), matching the oracle's `_elementImports`
    /// build which keys on names.
    fn import_credits_extension_member(
        &self,
        edge: &dartq_resolution::ImportEdge,
        target: dartq_resolution::LibraryId,
        used: &[StringId],
    ) -> bool {
        for (export_name, entry) in self.graph.export_scope(target) {
            if !edge.combinators.allows(export_name) {
                continue;
            }
            let Some(elements) = self.index.get(entry.file) else {
                continue;
            };
            for (face_index, face) in elements.extension_faces().iter().enumerate() {
                if face.name != Some(export_name) {
                    continue;
                }
                // The oracle's credit requires an APPLICABLE extension: the
                // `on` type's head must resolve in the DECLARING file's
                // scope (the spelling need not be visible to the importer —
                // `on XmlNode` applies in files that never import node.dart;
                // an extension over a type unresolvable in its own scope —
                // `on YamlMap` under the missing package:yaml — never
                // applies, so its member names lend the import no credit;
                // imports_verifier.dart counts extension elements the
                // resolver actually applied).
                if let Some(on) = face.on_type
                    && !self.named_type_is_type_in(entry.file, on)
                {
                    continue;
                }
                if face.members.keys().any(|member| used.contains(member)) {
                    return true;
                }
                // The applied-face credit (`notifyExtensionUsed`,
                // scope.dart:308-321): upstream marks the whole extension
                // element used when the resolver APPLIES it, and the
                // application shapes that resolve no selector spelling into
                // the name pool — operators (`-e`, `a + b`), index
                // (`e[i]`, `e[i] = v`), and setter writes (`e.foo = v`
                // resolves the `foo=` spelling) — credit exactly this way.
                // The resolve side records every unique application on
                // `extension_member_used` keyed by (declaring file, face
                // slot); a record on THIS face spares its defining edge.
                if let Ok(slot) = u32::try_from(face_index)
                    && self
                        .extension_member_used
                        .iter()
                        .any(|&(file, index, _)| file == entry.file && index == slot)
                {
                    return true;
                }
            }
        }
        false
    }

    /// The literal-receiver extension applications of this unit, as
    /// `(receiver type head, member spelling)` pairs. A literal's static
    /// type is fixed by its token kind — `Int`/`Hexadecimal` → `int`,
    /// `Double` → `double`, `String` → `String` — and its application
    /// shapes the row model folds inert carry no identifier, so the
    /// (type, member) join is read straight off the token stream:
    ///
    /// - `L[i]` / `L[i] = v` / `L[i] op= v` → `[]`, `[]=`, or both —
    ///   a write applies `[]=` alone upstream, a read `[]` alone, a
    ///   read-modify-write (`op=`, `++`, `--`) both;
    /// - `L(args)` → `call`;
    /// - `L op x` (any overloadable binary spelling), `op L` (`-`, `~`),
    ///   `L++`/`L--` (the `-`/`+` member), `L op= x` (the compound base
    ///   member) → the operator's own spelling;
    ///
    /// `==`/`!=`/`&&`/`||`/`??`/`?.` shapes are excluded upstream — `==`
    /// answers through `Object` always, the boolean operators never
    /// apply an extension.
    fn literal_extension_applications(&self) -> Vec<(StringId, StringId)> {
        let tokens = self.tokens.as_slice();
        let int = self.intern("int");
        let double = self.intern("double");
        let string = self.intern("String");
        let type_head = |kind: dartq_scanner::TokenKind| match kind {
            dartq_scanner::TokenKind::Int | dartq_scanner::TokenKind::Hexadecimal => Some(int),
            dartq_scanner::TokenKind::Double => Some(double),
            dartq_scanner::TokenKind::String => Some(string),
            _ => None,
        };
        let binary_member = |kind: dartq_scanner::TokenKind| match kind {
            dartq_scanner::TokenKind::Lt => Some("<"),
            dartq_scanner::TokenKind::Gt => Some(">"),
            dartq_scanner::TokenKind::LtEq => Some("<="),
            dartq_scanner::TokenKind::GtEq => Some(">="),
            dartq_scanner::TokenKind::Minus | dartq_scanner::TokenKind::MinusEq => Some("-"),
            dartq_scanner::TokenKind::Plus | dartq_scanner::TokenKind::PlusEq => Some("+"),
            dartq_scanner::TokenKind::Star | dartq_scanner::TokenKind::StarEq => Some("*"),
            dartq_scanner::TokenKind::Slash | dartq_scanner::TokenKind::SlashEq => Some("/"),
            dartq_scanner::TokenKind::TildeSlash | dartq_scanner::TokenKind::TildeSlashEq => {
                Some("~/")
            }
            dartq_scanner::TokenKind::Percent | dartq_scanner::TokenKind::PercentEq => Some("%"),
            dartq_scanner::TokenKind::LtLt | dartq_scanner::TokenKind::LtLtEq => Some("<<"),
            dartq_scanner::TokenKind::GtGt | dartq_scanner::TokenKind::GtGtEq => Some(">>"),
            dartq_scanner::TokenKind::GtGtGt | dartq_scanner::TokenKind::GtGtGtEq => Some(">>>"),
            dartq_scanner::TokenKind::Ampersand | dartq_scanner::TokenKind::AmpersandEq => Some("&"),
            dartq_scanner::TokenKind::Bar | dartq_scanner::TokenKind::BarEq => Some("|"),
            dartq_scanner::TokenKind::Caret | dartq_scanner::TokenKind::CaretEq => Some("^"),
            _ => None,
        };
        let mut apps: Vec<(StringId, StringId)> = Vec::new();
        for (i, token) in tokens.iter().enumerate() {
            // Prefix-operator arm: `-L`, `~L` apply the unary member; the
            // preceding literal-shape check never fires on these (the op
            // token itself is not a literal).
            if matches!(
                token.kind,
                dartq_scanner::TokenKind::Minus
                    | dartq_scanner::TokenKind::Tilde
                    | dartq_scanner::TokenKind::PlusPlus
                    | dartq_scanner::TokenKind::MinusMinus
            ) && let Some(head) = tokens.get(i + 1).and_then(|t| type_head(t.kind))
            {
                let spelling = match token.kind {
                    dartq_scanner::TokenKind::Tilde => "~",
                    dartq_scanner::TokenKind::PlusPlus => "+",
                    _ => "-",
                };
                apps.push((head, self.intern(spelling)));
                continue;
            }
            let Some(head) = type_head(token.kind) else {
                continue;
            };
            let Some(next) = tokens.get(i + 1) else {
                break;
            };
            match next.kind {
                dartq_scanner::TokenKind::OpenSquare => {
                    // The write-shape split mirrors the resolver: `L[i] = v`
                    // applies `[]=` alone, `L[i] op= v`/`L[i]++` applies
                    // `[]` for the read and `[]=` for the write, every
                    // other shape applies `[]` alone.
                    let read = self.intern("[]");
                    let write = self.intern("[]=");
                    let follower = self
                        .tokens
                        .matching_end(i + 1)
                        .and_then(|end| tokens.get(end + 1))
                        .map(|t| t.kind);
                    match follower {
                        Some(dartq_scanner::TokenKind::Eq) => {
                            apps.push((head, write));
                        }
                        // Every assignment form resolves `[]=` alone —
                        // `IndexExpression.writeOrReadElement` answers the
                        // write element for `=`/compound/`++`/`--` alike
                        // (extensions.dart:244-246 +
                        // unused_local_elements_verifier.dart:196-200);
                        // the compound's read half stays a TYPE-level
                        // consult (invocation.rs `check_index_argument`),
                        // never a `usedElements` credit.
                        Some(
                            dartq_scanner::TokenKind::PlusPlus
                            | dartq_scanner::TokenKind::MinusMinus
                            | dartq_scanner::TokenKind::PlusEq
                            | dartq_scanner::TokenKind::MinusEq
                            | dartq_scanner::TokenKind::StarEq
                            | dartq_scanner::TokenKind::SlashEq
                            | dartq_scanner::TokenKind::TildeSlashEq
                            | dartq_scanner::TokenKind::PercentEq
                            | dartq_scanner::TokenKind::AmpersandEq
                            | dartq_scanner::TokenKind::BarEq
                            | dartq_scanner::TokenKind::CaretEq
                            | dartq_scanner::TokenKind::LtLtEq
                            | dartq_scanner::TokenKind::GtGtEq
                            | dartq_scanner::TokenKind::GtGtGtEq
                            | dartq_scanner::TokenKind::QuestionQuestionEq,
                        ) => {
                            apps.push((head, write));
                        }
                        _ => apps.push((head, read)),
                    }
                }
                dartq_scanner::TokenKind::OpenParen => {
                    apps.push((head, self.intern("call")));
                }
                dartq_scanner::TokenKind::PlusPlus => {
                    apps.push((head, self.intern("+")));
                }
                dartq_scanner::TokenKind::MinusMinus => {
                    apps.push((head, self.intern("-")));
                }
                kind => {
                    if let Some(spelling) = binary_member(kind) {
                        apps.push((head, self.intern(spelling)));
                    }
                }
            }
        }
        apps
    }

    /// The literal-application arm of the extension credit: `apps`
    /// ([`Self::literal_extension_applications`]) carries `(receiver type
    /// head, member spelling)` pairs, and an exported extension face
    /// applies exactly when its `on` head IS the literal's type and it
    /// declares the member — the same (receiver type, member) granularity
    /// [`Self::import_credits_extension_member`]'s applied-face arm keeps.
    /// A face whose `on` head does not resolve a type in its own file
    /// never applies (the oracle's inapplicable-extension face).
    fn literal_extension_credit(
        &self,
        edge: &dartq_resolution::ImportEdge,
        target: dartq_resolution::LibraryId,
        apps: &[(StringId, StringId)],
    ) -> bool {
        if apps.is_empty() {
            return false;
        }
        for (export_name, entry) in self.graph.export_scope(target) {
            if !edge.combinators.allows(export_name) {
                continue;
            }
            let Some(elements) = self.index.get(entry.file) else {
                continue;
            };
            for face in elements.extension_faces() {
                if face.name != Some(export_name) {
                    continue;
                }
                let Some(on) = face.on_type else {
                    continue;
                };
                if !self.named_type_is_type_in(entry.file, on) {
                    continue;
                }
                if apps
                    .iter()
                    .any(|&(head, member)| head == on && face.members.contains_key(&member))
                {
                    return true;
                }
            }
        }
        false
    }

    /// Moves the pass's accumulated use sets into the unit's use-face
    pub(crate) fn take_use_face(&mut self) -> UnitUseFace {
        // The demonstrating pool computes before the drains: the receiver
        // facts (`member_reads`) stay live here, and the input set is the
        // same `used_names` ∪ `import_used` union the directive pass
        // credits by.
        let credit_pool: Vec<StringId> = self
            .used_names
            .iter()
            .chain(&self.import_used)
            .copied()
            .collect();
        let extension_credit = self
            .extension_credit_names(&credit_pool)
            .into_iter()
            .collect();
        UnitUseFace {
            used: std::mem::take(&mut self.used_names),
            reads: std::mem::take(&mut self.read_names),
            annotations: std::mem::take(&mut self.annotation_uses),
            extension_members: std::mem::take(&mut self.extension_member_used),
            imports: self
                .imports
                .drain(..)
                .map(|site| ImportUseFace {
                    prefix: site.prefix,
                    shown: site.shown.into_iter().map(|(name, _)| name).collect(),
                })
                .collect(),
            exports: std::mem::take(&mut self.exports),
            extension_credit,
        }
    }

    /// The `_`-wildcard test (`isWildcardVariable`, upstream Dart SDK
    /// source, pkg/analyzer/lib/src/dart/element/extensions.dart:81): the
    /// exact name `_` acting as a wildcard, gated on the DECLARING
    /// library's wildcard-variables feature (`enabledIn: 3.7.0`,
    /// `tools/experimental_features.yaml` — below 3.7 a `// @dart = 3.5`
    /// library treats `_` as an ordinary name). ONE gate for every
    /// wildcard kind (local variable, local function, prefix, type
    /// parameter, formal parameter — the call sites are already
    /// kind-restricted): the same shape the duplicate-definition
    /// verifier's `isWildcard` applies (duplicate_definition_verifier.dart
    /// :398-402). The unused-local verifier's own pre-feature fallback
    /// (all-underscores names spare the local-variable and
    /// catch-exception arms only, `_isNamedWildcard`'s else branch,
    /// unused_local_elements_verifier.dart:836-844) is the
    /// [`Self::drop_pre_feature_underscore_locals`] mirror.
    pub(crate) fn is_wildcard(&self, name: StringId) -> bool {
        self.class_modifiers.wildcards_enabled(self.lib) && self.strings.resolve(name) == "_"
    }

    /// The offset of the first non-whitespace byte at or after `from`,
    /// when it opens an index (`[`): the `?.[` recovery's member anchor —
    /// the dot access folded the `[...]` operand, and the oracle resolves
    /// its member at the bracket.
    /// The next non-whitespace byte position after `from` (a UTF-16
    /// offset), returned as a UTF-16 offset when it is `[`. Byte-vs-UTF-16:
    /// `from` is a source range end (`member.range.end()`), so the byte
    /// cursor goes through `byte_of` — a wide char upstream of the bracket
    /// would otherwise desync the probe (#2161's doc-comment `—` row).
    pub(crate) fn bracket_after(&self, from: u32) -> Option<u32> {
        let start = self.byte_of(from).min(self.source.len());
        let bytes = self.source.as_bytes();
        bytes[start..]
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .map(|index| self.utf16_of(start + index))
            .filter(|&offset| self.byte_at(offset) == Some(b'['))
    }

    /// The staged null-aware faces re-decide against the flow's promoted
    /// receiver types (`_checkForUnnecessaryNullAware` reads the
    /// flow-augmented static type, which the walk-time row cannot see).
    ///
    /// `props` carries the promoted-PROPERTY rows
    /// ([`FlowFact::PromotedPropertyRead`]) the `null_aware_staged` staging
    /// gate cannot see: a member/property receiver (`_g?.abs()`,
    /// `this._f?.x`, `obj._f?.x`) never lands in `local_hits` — only bare
    /// local reads do — so it is never pushed onto `null_aware_staged` and
    /// the local-receiver loop below cannot re-decide it. The member
    /// receiver rides `null_aware_members`/`member_reads` instead: every
    /// `?.`/`?..` merge latches its member offset there, and the staged
    /// `MemberRead` keeps the receiver span and classification. The
    /// promoted-property re-decision below asks the same
    /// `promotable_field_substitution` verdict the condition-face join asks
    /// (upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/generated/error_verifier.dart`
    /// `_checkForUnnecessaryNullAware`'s promoted-property arm).
    pub(crate) fn flush_null_aware_staged(
        &mut self,
        promotions: &PromotionStaging,
        props: &PropPromotionStaging,
    ) {
        let accesses = std::mem::take(&mut self.null_aware_staged);
        for (read, op_range, kind) in accesses {
            if promotions
                .promoted(read)
                .is_some_and(Ty::is_strictly_non_nullable)
            {
                let (op, replacement) = kind.lexemes();
                self.emit3(
                    SemanticCode::InvalidNullAwareOperator,
                    op_range,
                    op,
                    replacement,
                    "",
                );
            }
        }
        // The member/property `?.`/`?..` receivers the staging gate skipped:
        // re-decide each against the promoted-property rows. A bare local
        // receiver stays with `null_aware_staged` (its promotion rides the
        // local table); a member receiver resolves through
        // `member_read_resolution`/`promotable_field_substitution`, which
        // gate the promotion on the official `isPropertyPromotable` verdict
        // (private, final, non-external, unit-unconflicted). The operator
        // span is the `?.`/`?..` token that follows the receiver — the merge
        // does not record it on `member_reads`, but the receiver end anchors
        // it directly.
        let members: Vec<u32> = self.null_aware_members.iter().copied().collect();
        for member_offset in members {
            let Ok(index) = self
                .member_reads
                .binary_search_by(|row| row.offset.cmp(&member_offset))
            else {
                continue;
            };
            let member = self.member_reads[index];
            let recv_span = member.recv_span;
            // The promoted-property anchor is the receiver's own MEMBER
            // token: `_g` carries it at the bare read's span; `this._f` /
            // `obj._f` carry it at the trailing member token (the receiver
            // read's end — `member_read_ends` hops that end to the receiver's
            // own member row). A bare LOCAL receiver (`obj?.x`) resolves to
            // `RefTarget::Local` below, not a member — `member_read_resolution`
            // returns `None` and the row stays with `null_aware_staged`'s
            // local arm, so there is no double report. An unresolved receiver
            // cannot be a promoted field.
            let anchor = self
                .member_read_ends
                .get(&recv_span.end())
                .and_then(|&i| self.member_reads.get(i as usize))
                .map_or(recv_span.offset, |row| row.offset);
            let Some((entry, _, _)) = self.member_read_resolution(anchor) else {
                continue;
            };
            let from = entry.var_ty;
            let anchor_range = SourceRange {
                offset: anchor,
                length: 0,
            };
            let Some(promoted) = self.promotable_field_substitution(props, anchor_range, from)
            else {
                continue;
            };
            if !promoted.is_strictly_non_nullable() {
                continue;
            }
            // The `?.`/`?..` operator sits at the receiver's end (modulo
            // whitespace). Anything but a `.`-led `?` is outside this face —
            // `?[`/`?..[` index sections stage elsewhere and are not the
            // promoted-PROPERTY receiver this pass re-decides.
            let question = self.skip_whitespace(recv_span.end());
            if self.byte_at(question) != Some(b'?') || self.byte_at(question + 1) != Some(b'.') {
                continue;
            }
            let (op_range, kind) = if self.byte_at(question + 2) == Some(b'.') {
                (
                    SourceRange {
                        offset: question,
                        length: 3,
                    },
                    NullAwareKind::Cascade,
                )
            } else {
                (
                    SourceRange {
                        offset: question,
                        length: 2,
                    },
                    NullAwareKind::Access,
                )
            };
            let (op, replacement) = kind.lexemes();
            self.emit3(
                SemanticCode::InvalidNullAwareOperator,
                op_range,
                op,
                replacement,
                "",
            );
        }
        let asserts = std::mem::take(&mut self.null_assert_staged);
        for (read, bang, nullable_params) in asserts {
            let promoted = promotions.promoted(read).filter(
                |ty| !matches!(ty, Ty::Named { name, .. } if nullable_params.contains(name)),
            );

            if promoted.is_some_and(Ty::is_strictly_non_nullable) {
                self.stage_usage(SemanticCode::UnnecessaryNonNullAssertion, bang, "");
            }
        }
        // The `??`/`??=` dead-coalesce re-decision
        // (`_checkForDeadNullCoalesce`, error_verifier.dart:4733-4739): the
        // operand read's flow-promoted type — a declared-nullable local the
        // walk's static verdict could not anchor (`if (x != null) x ?? d`,
        // `x ??= 1` then `x ??= 2` through the `??=` write's own
        // promotion). Each staged pair carries the operand's read span and
        // the report range the walk already computed (the `??` right
        // operand's node range / the `??=` value's expression span).
        let coalesces = std::mem::take(&mut self.coalesce_staged);
        for (read, range) in coalesces {
            // The local arm (the promotion table) and the target arm (the
            // promotion-free declared read face) mirror upstream's two
            // `readType` sources: a local target reads its flow-promoted
            // type (`localVariableTypeProvider.getType`), while a member
            // target reads the getter/field's declared return — never in
            // the local table (`setReadElement`, resolver.dart:1754-1792),
            // so its staged pair rides the declared face instead.
            let anchored = promotions
                .promoted(read)
                .is_some_and(Ty::is_strictly_non_nullable)
                || self.staged_target_read_never_null(read);
            if anchored {
                self.stage_usage(SemanticCode::DeadNullAwareExpression, range, "");
            }
        }
    }

    /// The staged `??`/`??=` pair's non-local target face: the promotion
    /// table never carries a member read (its rows key the bare local
    /// read's own span — a member row's merged span never matches), so a
    /// staged pair the local arm missed re-decides on the target row's
    /// declared/static type — upstream `readType`'s promotion-free member
    /// source (`setReadElement`: the getter/field's declared return, the
    /// compound-assignment read face `visitAssignmentExpression` feeds
    /// `_checkForDeadNullCoalesce`, error_verifier.dart:444-449).
    ///
    /// The verdict mirrors [`crate::diagnose::scopes_bodies`]'s walk-time
    /// face: a `?.`-merged read stays nullable (`widen_null_aware`'s
    /// posture — the short-circuit read never anchors), the row's own
    /// static type answers first, and a member row re-asks the
    /// parse-recorded member read (the merge's receiver classification
    /// survives the walk) for its declared accessor face. Every
    /// unknown/invalid answer keeps `false` — the arm only ever anchors
    /// the strictly-non-nullable declared face, never widens it.
    fn staged_target_read_never_null(&self, read: SourceRange) -> bool {
        // A `?.`-merged read is nullable whatever the declared face.
        if self.null_aware_members.contains(&read.offset) {
            return false;
        }
        // The staged span IS the target row's own range (`check_dead_null_assignment`
        // stages `target_range`): rows are offset-sorted and a receiver's
        // bare read can share the offset, so the bounded tail scan matches
        // the full range.
        let start = self
            .rows
            .partition_point(|row| row.range.offset < read.offset);
        let Some(row) = self.rows[start..]
            .iter()
            .rev()
            .find(|row| row.range == read)
        else {
            return false;
        };
        let declared = self.operand_static_ty(row);
        if declared != Ty::UNKNOWN
            && !self.written_ty_invalid(declared)
            && self.strictly_non_nullable(declared)
        {
            return true;
        }
        // The parse-recorded member face: the walk-time accessor path
        // answers from the row shape, the staged join re-asks through the
        // recorded read (its receiver classification was taken at the
        // merge, before any fold reshaped the row). The member read keys
        // at the member token inside the staged span, never the receiver
        // head the row's own offset spells.
        if !matches!(row.shape, RowShape::Member { .. }) {
            return false;
        }
        let end = self
            .member_reads
            .partition_point(|member| member.offset < read.end());
        let Some(anchor) = self.member_reads[..end]
            .iter()
            .rev()
            .find_map(|member| (member.recv_span.offset >= read.offset).then_some(member.offset))
        else {
            return false;
        };
        let Some((entry, _, _)) = self.member_read_resolution(anchor) else {
            return false;
        };
        let face = match entry.kind {
            MemberKind::Field => entry.var_ty,
            _ => entry.return_ty,
        };
        face != Ty::UNKNOWN && !self.written_ty_invalid(face) && self.strictly_non_nullable(face)
    }

    /// The enclosing executable's lookup spelling once its name has fired
    /// (the official `_enclosingExec`); constructors, factories stay
    /// `None` (their `ret` latch is unset by design). A local function's
    /// own body is covered by the open local-function stack.
    pub(crate) fn exec_self_name(&self) -> Option<StringId> {
        // The self-reference test keys on the innermost EXECUTABLE frame,
        // not the innermost frame: statement blocks own their own frames
        // (`begin_block`), so a self-reference inside a nested block must
        // skip past them to the enclosing executable's slot.
        let inner_exec = self
            .frames
            .iter()
            .rev()
            .position(|f| matches!(f.kind, FrameKind::Executable { .. }))
            .map(|pos| self.frames.len() - 1 - pos);
        if let Some((fn_depth, name)) = self.local_fn_stack.last()
            && inner_exec.is_some_and(|frame_depth| frame_depth == *fn_depth)
        {
            return Some(*name);
        }
        self.frames.iter().rev().find_map(|f| match &f.kind {
            FrameKind::Executable { ret: Some(ret), .. } => Some(ret.name),
            _ => None,
        })
    }

    /// The official self-reference test: `name` names the enclosing
    /// executable or the enclosing container (`_useIdentifierElement`'s
    /// identical checks).
    pub(crate) fn in_own_named_scope(&self, name: StringId) -> bool {
        self.exec_self_name() == Some(name)
            || self.container_context().is_some_and(|(c, _)| c == name)
    }

    /// Drains the pending annotation latch: the annotations since the last
    /// declaration site, each with its captured `UseResult.unless` face
    /// (the `@override`-family gates compare names; non-empty marks the
    /// pragma/JS exemption gates' under-report superset).
    pub(crate) fn take_annotations(&mut self) -> Vec<PendingAnnotation> {
        std::mem::take(&mut self.pending_annotations)
    }

    /// Pushes one library-level declaration site (the usage cluster's
    /// library-level side) and returns its index.
    pub(crate) fn note_decl_site(
        &mut self,
        name: StringId,
        range: SourceRange,
        kind: DeclKind,
    ) -> usize {
        let pending = self.take_annotations();
        let annotated = !pending.is_empty();
        // The `@UseResult` face (`isUseResult`, element.dart:2014-2019):
        // the meta getter (`@useResult`) or the class face (`@UseResult`,
        // whose `unless` selector carries the `parameterDefined` value).
        // The first recognized annotation wins (the official walk returns
        // the first isUseResult entry); the resolution walks run once per
        // annotation — metadata is rare and the graph lookups are set
        // probes.
        let mut use_result = None;
        for annotation in &pending {
            let face = crate::model::Annotation {
                name: annotation.name,
                range,
                args_text: None,
            };
            // The unless face rode the latch (the `end_metadata` source
            // walk); re-shaping it as the helper's argument faces keeps the
            // selector + meta-import resolution in one place — the value
            // face for a literal, the written face for a reference.
            let args = annotation.unless.map(|(arg, span)| match arg {
                UnlessArg::Literal(value) => crate::annotation::AnnotationArg {
                    label: Some(self.strings.intern("parameterDefined")),
                    string: Some((value, span)),
                    reference: None,
                },
                UnlessArg::Reference(name) => crate::annotation::AnnotationArg {
                    label: Some(self.strings.intern("parameterDefined")),
                    string: None,
                    reference: Some((name, span)),
                },
            });
            let args = args.as_slice();
            if let Some(unless) = crate::annotation::use_result_unless(
                &face,
                args,
                self.strings,
                self.graph,
                self.lib,
            ) {
                let (arg, span) = match unless {
                    crate::annotation::UnlessFace::Literal { value, span } => {
                        (UnlessArg::Literal(value), span)
                    }
                    crate::annotation::UnlessFace::Reference { name, span } => {
                        (UnlessArg::Reference(name), span)
                    }
                };
                use_result = Some(UrFace {
                    unless: Some((arg, span)),
                });
                break;
            }
            if crate::annotation::is_meta_getter(
                &face,
                "useResult",
                self.strings,
                self.graph,
                self.lib,
            ) || crate::annotation::is_meta_class(
                &face,
                "UseResult",
                self.strings,
                self.graph,
                self.lib,
            ) {
                use_result = Some(UrFace { unless: None });
                break;
            }
        }
        let annotations = pending.iter().map(|a| a.name).collect();
        self.decl_sites.push(DeclSite {
            name,
            range,
            kind,
            annotated,
            annotations,
            use_result,
            // The const latch rides onto the site at creation; the named
            // selector's in-place rewrite keeps it (`begin_constructor` /
            // `begin_factory` set it before any site of theirs exists).
            const_ctor: self.in_const_ctor,
            is_factory: false,
            // The site-consumed augment latch. A member site carries its
            // own `augment` keyword — the executable kinds read the method
            // head's `open_exec_augment`, a field declarator reads the
            // group head's `group_augment` (both latched before any of the
            // declaration's name events). Every other site keeps the
            // constructor latch's `take` so a stale bit never crosses a
            // site.
            is_augment: match &kind {
                DeclKind::Member {
                    kind: MemberKind::Field,
                    ..
                } => self.group_augment,
                DeclKind::Member { .. } => self.open_exec_augment,
                _ => std::mem::take(&mut self.ctor_augment_pending),
            },
        });
        let site = self.decl_sites.len() - 1;
        // A method/getter/setter site opens the member face's span slots:
        // the header's own type variables and formals fire while this
        // frame is innermost — the depth keeps a nested executable's
        // header events out (fields never need the face: their arms keep
        // the name anchor).
        if matches!(kind, DeclKind::Member {
            kind: MemberKind::Method,
            ..
        }) {
            self.open_member_face = Some(MemberFaceSpans {
                site,
                depth: self.frames.len().saturating_sub(1),
                ..MemberFaceSpans::default()
            });
        }
        // The FFI-named annotations latch onto the declaration they
        // annotate (the `dart:ffi` verifier's post-parse pass reads them
        // by site). Anything still pending at the next site is the next
        // declaration's own metadata — the latch never crosses a
        // declaration.
        let ffi_annos = std::mem::take(&mut self.ffi_pending_annos);
        if !ffi_annos.is_empty() {
            self.ffi_decl_annos.insert(site, ffi_annos);
        }
        site
    }

    /// Whether one of the pending annotations carries the spelling (the
    /// `@override`-family gates read the latch before any site drains it).
    pub(crate) fn pending_annotation_is(&self, spelling: &str) -> bool {
        self.pending_annotations
            .iter()
            .any(|a| self.strings.resolve(a.name) == spelling)
    }

    /// Whether the annotation's own name is `spelling`, compared on its last
    /// dotted segment — `@JS`, `@js.JS` and `@meta.JS` alike (the unused
    /// verifier's two metadata gates, `ElementMetadata.hasJS` /
    /// `hasPragmaVmEntryPoint`, element.dart:8572-8581 and :8662-8671,
    /// resolve the annotation's constructor; the declaration site keeps the
    /// written spelling, so the segment is the observable proxy).
    fn annotation_named(&self, name: StringId, spelling: &str) -> bool {
        self.strings.resolve(name).rsplit('.').next() == Some(spelling)
    }

    // —— the discarded-value machine (`use_result_verifier.dart`) ————————

    /// Registers one completed expression whose value may be discarded:
    /// the send callee's or selector's name token offset. The expression
    /// stays pending until a context event answers its parent-chain walk.
    pub(crate) fn ur_register(&mut self, offset: u32) {
        self.ur_pending.push(offset);
    }

    /// Registers a creation fold: the official anchor spans the whole
    /// invocation (`nodeToAnnotate` — the `A()` shape), not just the head
    /// token.
    pub(crate) fn ur_register_creation(
        &mut self,
        offset: u32,
        span: SourceRange,
        container: StringId,
    ) {
        self.ur_spans.insert(offset, span);
        self.ur_creations.push((offset, span, container));
        self.ur_pending.push(offset);
    }

    /// Seals every pending expression as used (`_isUsed` true — the value
    /// flows somewhere observable).
    pub(crate) fn ur_seal_used(&mut self) {
        self.ur_used.extend(self.ur_pending.drain(..));
    }

    /// Seals every pending expression as discarded (statement end, cascade
    /// sections, for updaters): the offsets simply stay absent from the
    /// used set, and the resolve arms emit against them.
    pub(crate) fn ur_seal_discard(&mut self) {
        self.ur_pending.clear();
    }

    /// `end_arguments`: seals only the pendings registered inside the
    /// closing argument list used (the official `ArgumentList` parent arm)
    /// — the callee's own candidate predates the bracket watermark and
    /// stays pending for the enclosing context to answer.
    pub(crate) fn ur_seal_used_within(&mut self) {
        if let Some(base) = self.ur_arg_base.pop() {
            let tail = self.ur_pending.split_off(base.min(self.ur_pending.len()));
            self.ur_used.extend(tail);
        }
    }

    /// `handle_expression_statement`: outside for-parts, the statement's
    /// value is discarded (the official parent, `ExpressionStatement`, is
    /// absent from both `_isUsed` sets); the for-condition arm fires here,
    /// while the updaters drain at `handle_for_loop_parts`.
    pub(crate) fn ur_expression_statement(&mut self) {
        if self.in_for_parts.last() == Some(&1) || self.in_for_parts.last() == Some(&2) {
            self.ur_seal_used();
        } else {
            self.ur_seal_discard();
        }
    }

    /// Registers the non-invoked identifier expression completing at
    /// `begin` (the read row sits on the row stack): the
    /// `checkNameExpression` candidate face.
    pub(crate) fn ur_register_bare_read(&mut self, begin: Token) {
        if let Some(row) = self
            .rows
            .iter()
            .rev()
            .find(|row| row.range.offset == begin.range.offset)
        {
            let offset = row.range.offset;
            self.ur_register(offset);
        }
    }

    /// `end_metadata`: captures the `parameterDefined:` face of the
    /// annotation that just closed — a source-level walk of the annotation
    /// span finding `parameterDefined` `:` value at argument depth. A
    /// string literal captures its cooked value; a bare identifier
    /// captures the written name (the official `toString` fallback's
    /// message face) with the gate value resolving from the const
    /// environment at the check. Any other shape under-captures (`None`),
    /// under-reporting.
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    pub(crate) fn note_metadata_args(&mut self, begin: Token, end: Token) {
        // Token ranges are UTF-16; the walk reads source bytes — convert so a
        // wide char upstream cannot desync (or cut mid-character, #2594).
        let start = self.byte_of(begin.range.offset);
        let stop = self.byte_of(end.range.end());
        // The annotation head's use credit (the official `_visitNameExpression`
        // read — `unused_local_elements_verifier.dart`'s `visitNode` routes
        // every `NameExpression`, the annotation's identifier included, so
        // `@a` and `@a.b()` both mark the resolved `a` element used): the
        // head segment resolves lexically, exactly as a bare read does —
        // a local `a` in scope at the annotation takes the hit and spares
        // the `unused_local_variable` row. The pending entry's name may
        // already carry the joined selector (`@A.named`), so only the head
        // segment consults the frame stack — a qualified head resolves
        // nothing locally and misses (the same `resolve_local` answer the
        // `check_annotation` miss gate's `f.locals` membership made).
        if let Some(head) = self
            .pending_annotations
            .last()
            .map(|open| open.name)
            .and_then(|name| {
                let text = self.strings.resolve(name);
                let end = text.find('.').unwrap_or(text.len());
                (end > 0).then(|| self.strings.intern(&text[..end]))
            })
            && let Some(hit) = self.resolve_local(head)
        {
            self.locals[hit as usize].uses += 1;
        }
        let Some(open) = self.pending_annotations.last_mut() else {
            return;
        };
        if open.unless.is_some() {
            return;
        }
        let text = self.source.get(start..stop).unwrap_or_default();
        let bytes = text.as_bytes();
        let label = b"parameterDefined";
        let mut cursor = 0usize;
        while let Some(rel) = bytes[cursor.min(bytes.len())..]
            .windows(label.len())
            .position(|w| w == label)
        {
            let at = cursor + rel;
            cursor = at + label.len();
            // The label must start an argument: preceded by `(` or `,`
            // (whitespace between), so a mention inside another string
            // never matches.
            let head = text[..at].trim_end();
            if !matches!(head.as_bytes().last(), Some(b'(' | b',')) {
                continue;
            }
            // Between label and value sits exactly a `:` (whitespace
            // around); anything else is a different argument shape.
            let tail = &text[cursor..];
            let Some(colon) = tail.find(':') else {
                continue;
            };
            if !tail[..colon].bytes().all(|b| b.is_ascii_whitespace()) {
                continue;
            }
            let body = tail[colon + 1..].trim_start();
            let at = (start + text.len() - body.len()) as u32;
            let Some((_, quote, triple)) = Self::string_literal_at(body) else {
                // A bare reference argument: the official walk messages
                // the written text (`undefinedParameter.toString()`) and
                // gates on the const value it resolves to. Prefixed
                // (`p.s`) and invoked (`f()`) shapes under-capture — the
                // under-report direction.
                if let Some((name, len)) = Self::bare_identifier_at(body) {
                    let name = self.strings.intern(name);
                    open.unless = Some((
                        UnlessArg::Reference(name),
                        SourceRange {
                            offset: at,
                            length: len as u32,
                        },
                    ));
                    return;
                }
                continue;
            };
            let Some(value_len) = Self::string_literal_len(body, quote, triple) else {
                continue;
            };
            let raw = &body[..value_len];
            let inner = raw
                .trim_start_matches('r')
                .trim_start_matches('#')
                .trim_matches(|c| c == '\'' || c == '"');
            let cooked = inner.replace("\\'", "'").replace("\\\"", "\"");
            open.unless = Some((
                UnlessArg::Literal(self.strings.intern(&cooked)),
                SourceRange {
                    offset: at,
                    length: value_len as u32,
                },
            ));
            return;
        }
    }

    /// The string literal starting at `text[0]`, if any: its quote byte
    /// and whether it is the triple-quoted shape. A raw marker (`r`)
    /// before the quote is admitted (the len walk then takes the body
    /// verbatim).
    pub(crate) fn string_literal_at(text: &str) -> Option<(usize, u8, bool)> {
        let bytes = text.as_bytes();
        let mut at = 0usize;
        if bytes.first() == Some(&b'r') {
            at = 1;
        }
        match bytes.get(at)? {
            b'\'' | b'"' => {
                let quote = bytes[at];
                let triple = bytes.get(at + 1) == Some(&quote) && bytes.get(at + 2) == Some(&quote);
                Some((at, quote, triple))
            }
            _ => None,
        }
    }

    /// The full literal length at `body[0]` for `quote`/`triple`,
    /// terminator included. Escapes (`\x`) never end the literal; an
    /// unterminated literal yields `None` (recovery — the under-report).
    pub(crate) fn string_literal_len(body: &str, quote: u8, triple: bool) -> Option<usize> {
        let bytes = body.as_bytes();
        let close: &[u8] = if triple {
            &[quote, quote, quote]
        } else {
            &[quote]
        };
        let mut at = if triple { 3 } else { 1 };
        while at < bytes.len() {
            if bytes[at] == b'\\' {
                at += 2;
                continue;
            }
            if bytes[at..].starts_with(close) {
                return Some(at + close.len());
            }
            at += 1;
        }
        None
    }

    /// The bare identifier starting at `body[0]`: its slice and byte
    /// length. ASCII identifier shape (the official scanner's identifier
    /// face); a following `(` (an invocation) or `.` (a prefixed
    /// reference) is a message face the walk cannot name — `None`, the
    /// under-report.
    pub(crate) fn bare_identifier_at(body: &str) -> Option<(&str, usize)> {
        let bytes = body.as_bytes();
        let ok = |b: u8, first: bool| {
            b == b'_' || b == b'$' || b.is_ascii_alphabetic() || (!first && b.is_ascii_digit())
        };
        let mut end = 0;
        while bytes.get(end).is_some_and(|&b| ok(b, end == 0)) {
            end += 1;
        }
        if end == 0 || matches!(bytes.get(end), Some(b'(' | b'.')) {
            return None;
        }
        Some((&body[..end], end))
    }

    /// Builds the face lookup (the face-bearing sites keyed by their
    /// declaring container) once, at resolve time.
    pub(crate) fn build_ur_sites(&mut self) {
        for index in 0..self.decl_sites.len() {
            if self.decl_sites[index].use_result.is_some() {
                let (container, spelling) = match &self.decl_sites[index].kind {
                    DeclKind::Member { container, .. } | DeclKind::Constructor { container } => {
                        (Some(*container), self.decl_sites[index].name)
                    }
                    _ => (None, self.decl_sites[index].name),
                };
                self.ur_sites.insert((container, spelling), index);
            }
        }
    }

    /// The face-bearing site of `member` on `container`'s chain (the
    /// resolution's same-file walk; a hop into another file's index
    /// chain-breaks, under-reporting, and so does a revisited container —
    /// a cyclic chain, mutual `extends` or a self-shadowed edge, ends the
    /// climb the same way instead of spinning).
    pub(crate) fn ur_member_site(&self, container: StringId, member: StringId) -> Option<usize> {
        let mut current = Some(container);
        let mut seen: FxHashSet<StringId> = FxHashSet::default();
        while let Some(c) = current {
            if !seen.insert(c) {
                return None;
            }
            if let Some(&site) = self.ur_sites.get(&(Some(c), member)) {
                return Some(site);
            }
            current = self
                .index
                .get(self.file)
                .and_then(|f| f.container(c))
                .and_then(|x| x.superclass);
        }
        None
    }

    /// The unless gate parameter (`computeConstantValue`'s
    /// `toStringValue`): a literal's cooked value, or a bare reference's
    /// const value from the file scope. `None` when the reference names no
    /// string constant — the official null arm: no gate, no bypass, no
    /// report. Class statics are out of a top-level annotation's scope on
    /// both sides; cross-file constants under-resolve (the const
    /// environment is per-file), the under-report direction.
    pub(crate) fn unless_param(&self, arg: UnlessArg) -> Option<StringId> {
        match arg {
            UnlessArg::Literal(value) => Some(value),
            UnlessArg::Reference(name) => match self.const_scopes.first()?.get(&name) {
                Some(ConstBinding::Value(ConstantValue::Str(text))) => {
                    Some(self.strings.intern(text))
                }
                _ => None,
            },
        }
    }

    /// The unless bypass (`_passesUsingParam`): the invocation supplies an
    /// argument that maps to a formal named
    /// `p` — a named argument spelled `p`, or a positional argument whose
    /// mapped formal is `p` (the official walk reads the argument list's
    /// resolved static parameters). Reads and constructor invocations
    /// carry no argument list the official switch accepts — their callers
    /// pass `None` formals.
    pub(crate) fn unless_bypassed(
        &self,
        callee: StringId,
        unless: StringId,
        formals: Option<&Formals>,
    ) -> bool {
        let Some(formals) = formals else {
            return false;
        };
        if self.named_given.contains(&(callee, unless)) && formals.named_formal(unless).is_some() {
            return true;
        }
        self.positional_given.get(&callee).is_some_and(|count| {
            formals
                .positional
                .get(..(*count as usize))
                .is_some_and(|given| given.iter().any(|formal| formal.name == unless))
        })
    }

    /// The member-row emission (`_check`): `row` resolved against
    /// `container`'s chain — or, for a constructor selector, against the
    /// container's own constructor set. The `unless` bypass reads the
    /// invocation's supplied-argument faces on the invoke intent only
    /// (constructor invocations pass no argument list to the official
    /// switch; a selector row anchors at the selector token — the
    /// whole-invocation span needs the receiver extent the row fold
    /// dropped, a documented under-length).
    pub(crate) fn check_member_unused_result(
        &mut self,
        row: &Row,
        container: StringId,
        recv: &Receiver,
        selector: bool,
    ) {
        let site_index = if selector {
            self.ur_sites.get(&(Some(container), row.name)).copied()
        } else {
            self.ur_member_site(container, row.name)
        };
        let Some(site_index) = site_index else {
            return;
        };
        let bypassed = matches!(row.intent, Intent::Invoke)
            && !selector
            && self.decl_sites[site_index].use_result.is_some_and(|face| {
                face.unless
                    .as_ref()
                    .and_then(|(arg, _)| self.unless_param(*arg))
                    .is_some_and(|param| {
                        let formals = self.member_formals_full(recv, row.name);
                        self.unless_bypassed(row.name, param, formals.as_deref())
                    })
            });
        let display = self.strings.resolve(row.name).to_string();
        self.check_unused_result(row.range.offset, row.range, site_index, &display, bypassed);
    }

    /// Emits the discarded-use row for a resolved reference unless its
    /// context sealed it used or its `unless` bypass applied (`_check`'s
    /// final gates; the anchor is the name token, or the whole invocation
    /// for the creation folds).
    pub(crate) fn check_unused_result(
        &mut self,
        offset: u32,
        span: SourceRange,
        _site_index: usize,
        display: &str,
        bypassed: bool,
    ) {
        if bypassed || self.ur_used.contains(&offset) {
            return;
        }
        let span = self.ur_spans.get(&offset).copied().unwrap_or(span);
        self.stage_usage(SemanticCode::UnusedResult, span, display);
    }

    /// The discarded-value face of a name resolved by scope (the
    /// `checkNameExpression` arm, and the cascade section whose target type
    /// the model cannot name): an invocation or bare read of a
    /// UseResult-carrying top-level declaration, not sealed used. The
    /// `unless` bypass reads the declaration's own formals.
    pub(crate) fn check_top_level_unused_result(&mut self, row: &Row) {
        // The face rides the DECLARATION the reference resolved to, never its
        // spelling: the official verifier reads the resolved element's own
        // metadata (upstream Dart SDK source, pkg/analyzer/lib/src/error/
        // use_result_verifier.dart `_getUseResultMetadata`), so a lexical
        // binding or the enclosing container's own member of the same
        // spelling owns the reference instead and the same-spelled top-level
        // site `ur_sites` keys by name is a different element. Both win the
        // upstream lookup ahead of the library scope (a local/formal/type
        // parameter lexically, an own member through the implicit `this`
        // face — probe: `int f = 2; void m() { String s = f; }` beside a
        // top-level `double f()` binds the field). The row's published
        // push-time target is the authority; a row with no publication is a
        // merged selector (the cascade section), which binds no bare name.
        //
        // The `Opaque` and `Missing` faces stay with the name lookup: the
        // object/extension faces behind `Opaque` lose to a same-spelled
        // library declaration upstream (probe: a top-level `double
        // hashCode()` beside `String s = hashCode;` inside a class binds the
        // top-level function), so their rows are real reports. An enum body's
        // synthesized member (`values`/`index`/`name`) does win that lookup
        // upstream and is the one over-report face left here — the
        // resolution publishes it as `Opaque` beside the object/extension
        // misses, so telling them apart needs a finer target, not a check
        // here.
        if let Some(found) = self
            .refs
            .iter()
            .rev()
            .find(|r| r.range.offset == row.range.offset)
            && matches!(
                found.target,
                RefTarget::Local | RefTarget::TypeParam | RefTarget::Member { .. }
            )
        {
            return;
        }
        let Some(&site_index) = self.ur_sites.get(&(None, row.name)) else {
            return;
        };
        let bypassed = matches!(row.intent, Intent::Invoke)
            && self.decl_sites[site_index].use_result.is_some_and(|face| {
                face.unless
                    .as_ref()
                    .and_then(|(arg, _)| self.unless_param(*arg))
                    .is_some_and(|param| {
                        self.top_level_decl(row.name).is_some_and(|decl| {
                            self.unless_bypassed(row.name, param, decl.formals.as_deref())
                        })
                    })
            });
        let display = self.strings.resolve(row.name).to_string();
        self.check_unused_result(row.range.offset, row.range, site_index, &display, bypassed);
    }

    /// The creation-fold emissions (`checkConstructorInvocation`): a
    /// folded `A()` whose unnamed-constructor site carries the face. The
    /// display name is the class name (the unnamed constructor's
    /// `displayName`), and the anchor the whole `A()` span.
    pub(crate) fn finalize_unused_result(&mut self) {
        for (offset, span, container) in std::mem::take(&mut self.ur_creations) {
            if self.ur_used.contains(&offset) {
                continue;
            }
            let Some(&site) = self.ur_sites.get(&(Some(container), container)) else {
                continue;
            };
            if self.decl_sites[site].use_result.is_none() {
                continue;
            }
            let text = self.strings.resolve(container).to_string();
            self.stage_usage(SemanticCode::UnusedResult, span, &text);
        }
    }

    /// The `_findUndefinedUseResultParameter` co-row
    /// (`annotation_verifier.dart:669-731`): a
    /// `@UseResult.unless(parameterDefined: ...)` on a top-level function
    /// or method whose formal list names no gate parameter reports at the
    /// written value expression (`{0}` the parameter, `{1}` the member).
    /// The gate reads the const value (a literal's cooked text, a
    /// reference's const-environment value — an unresolvable reference is
    /// the official null arm, silent); the message face is the literal's
    /// value (the `SimpleStringLiteral` arm) or the reference's written
    /// text (the `toString` fallback). Getters carry no parameter list and
    /// constructors are not `MethodDeclaration` parents — both stay
    /// silent; local functions are out of the declaration-site walk.
    pub(crate) fn finalize_undefined_use_result_params(&mut self) {
        // `(site name, member container — `None` for top-level — captured
        // argument face, written value span)` per `unless`-carrying
        // function/method site.
        let flagged: Vec<(StringId, Option<StringId>, UnlessArg, SourceRange)> = self
            .decl_sites
            .iter()
            .filter_map(|site| {
                let (arg, span) = site.use_result.as_ref().and_then(|face| face.unless)?;
                match &site.kind {
                    DeclKind::TopLevelFn => Some((site.name, None, arg, span)),
                    DeclKind::Member {
                        kind: MemberKind::Method | MemberKind::Setter,
                        container,
                        ..
                    } => Some((site.name, Some(*container), arg, span)),
                    _ => None,
                }
            })
            .collect();
        for (name, container, arg, span) in flagged {
            // The official predicate requires a parent declaration WITH a
            // formal parameter list: the null `parameterList` arm returns —
            // getters carry none, and a list the walk cannot fetch
            // (cross-file container) stays silent the same way. A known
            // list that names no gate parameter reports.
            let Some(param) = self.unless_param(arg) else {
                continue;
            };
            let message = match arg {
                UnlessArg::Literal(_) => param,
                UnlessArg::Reference(written) => written,
            };
            let names_parameter = |formals: &Formals| {
                formals.positional.iter().any(|formal| formal.name == param)
                    || formals.named.iter().any(|formal| formal.name == param)
            };
            let defined = match container {
                None => self
                    .top_level_decl(name)
                    .and_then(|decl| decl.formals.as_deref().map(names_parameter)),
                Some(container) => {
                    let entry = resolve_container(
                        self.graph,
                        self.namespaces,
                        self.index,
                        self.lib,
                        self.file,
                        container,
                    )
                    .and_then(|(resolved, _)| resolved.members.get(&name));
                    match entry {
                        Some(entry)
                            if matches!(entry.kind, MemberKind::Method | MemberKind::Setter) =>
                        {
                            entry.formals.as_deref().map(names_parameter)
                        }
                        _ => None,
                    }
                }
            };
            match defined {
                Some(true) | None => continue,
                Some(false) => {}
            }
            let param_text = self.strings.resolve(message).to_string();
            let member_text = self.strings.resolve(name).to_string();
            self.stage_typed(
                SemanticCode::UndefinedReferencedParameter,
                span,
                &param_text,
                &member_text,
            );
        }
    }

    /// Whether the already-collected declaration site of `member` in
    /// `container` carries the annotation (`@mustCallSuper` /
    /// `@mustBeOverridden` lookups against the walk's ancestors). Sites
    /// exist for this file's declarations only — cross-file ancestors
    /// under-report.
    pub(crate) fn site_annotation(
        &self,
        container: StringId,
        member: StringId,
        spelling: &str,
    ) -> bool {
        self.decl_sites.iter().any(|site| {
            matches!(site.kind, DeclKind::Member { container: c, .. } if c == container)
                && site.name == member
                && site
                    .annotations
                    .iter()
                    .any(|a| self.strings.resolve(*a) == spelling)
        })
    }
}

/// The usage gate in front of [`dartq_sdk::deprecated_library_message`]:
/// upstream only reaches the deprecation check through a resolved library,
/// so a `dart:` URI the SDK does not tabulate (unresolvable — `web_sql` is
/// on disk but absent from the flat table) reports nothing.
fn deprecated_sdk_library(library: &str) -> Option<&'static str> {
    if dartq_sdk::FLAT_LIBRARIES
        .binary_search_by_key(&library, |entry| entry.0)
        .is_err()
    {
        return None;
    }
    dartq_sdk::deprecated_library_message(library)
}

/// `normalizeDeprecationMessage` (upstream Dart SDK source,
/// `pkg/analyzer/lib/src/error/deprecated_member_use_verifier.dart:18-29`):
/// trim; `""` or `"."` collapses to `None` (the no-message face emits the
/// plain `deprecatedMemberUse` shape); a `.`/`?`/`!` tail keeps; anything
/// else gains a `.`.
fn normalize_deprecation(message: &str) -> Option<String> {
    let message = message.trim();
    if message.is_empty() || message == "." {
        None
    } else if message.ends_with(['.', '?', '!']) {
        Some(message.to_string())
    } else {
        Some(format!("{message}."))
    }
}

/// The shortest bracket-balanced token run ending at the `=` at `eq`: the
/// left side of a candidate pattern assignment. Walking back from the `=`
/// the running bracket depth returns to zero at the run's own head — the
/// opener for a bracketed pattern (`(var a)`, `[var a]`, `{…}`), or the
/// very first token for a bare expression (`x = …`, where the span is the
/// single identifier and no binder can hide). An opener at depth zero
/// (the `(` of a `for` head, whose group never closes before the `=`)
/// settles below zero and the run is refused, so an ordinary initializer
/// never reads as a pattern.
fn pattern_span_left_of(tokens: &[dartq_scanner::Token], eq: usize) -> Option<(usize, usize)> {
    use dartq_scanner::TokenKind;
    // One pattern's left side never spans more tokens than this; the cap
    // keeps a pathological `=` from walking the whole unit.
    const BACKWARD_CAP: usize = 4096;
    let floor = eq.saturating_sub(BACKWARD_CAP);
    let mut depth = 0i32;
    let mut at = eq;
    while at > floor {
        at -= 1;
        match tokens[at].kind {
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => depth += 1,
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => depth -= 1,
            _ => {}
        }
        if depth < 0 {
            return None;
        }
        if depth == 0 {
            return Some((at, eq));
        }
    }
    None
}

/// Whether a token at this shape opens a pattern element position: the
/// head of a sub-pattern, a positional/named separator, a logical-and
/// (`&&`) / logical-or (`||`) junction, or a rest element (`...` — its
/// sub-pattern is an element position, so `... var c` declares `c`) —
/// the only places a variable pattern may start.
fn pattern_element_position(kind: dartq_scanner::TokenKind) -> bool {
    use dartq_scanner::TokenKind;
    matches!(
        kind,
        TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::Comma
            | TokenKind::Colon
            | TokenKind::AmpersandAmpersand
            | TokenKind::BarBar
            | TokenKind::PeriodPeriodPeriod
    )
}

/// The first token of the pattern whose bracket group opens at `open`: a
/// typed collection's `<…>` arguments and an object pattern's `Type` /
/// `prefix.Type` name precede the group (`<int>[…]`, `Square<int>(…)`,
/// `p.Square(…)`); a bare group heads itself.
fn pattern_head_start(tokens: &[dartq_scanner::Token], open: usize) -> usize {
    use dartq_scanner::TokenKind;
    let mut head = open
        .checked_sub(1)
        .and_then(|close| type_arguments_start(tokens, close))
        .unwrap_or(open);
    if tokens[open].kind == TokenKind::OpenParen
        && head
            .checked_sub(1)
            .is_some_and(|at| tokens[at].kind == TokenKind::Identifier)
    {
        head -= 1;
        if head >= 2
            && tokens[head - 1].kind == TokenKind::Period
            && tokens[head - 2].kind == TokenKind::Identifier
        {
            head -= 2;
        }
    }
    head
}

/// The `<` opening the type-argument list that closes at `close` (a `>`,
/// or the `>>`/`>>>` nested lists fuse into). Only type-argument tokens
/// may sit inside; anything else — or no balancing `<` within the bound —
/// means `close` is no type-argument closer.
fn type_arguments_start(tokens: &[dartq_scanner::Token], close: usize) -> Option<usize> {
    use dartq_scanner::TokenKind;
    // A written type argument list never spans more tokens than this.
    const TYPE_ARGUMENTS_CAP: usize = 256;
    let mut depth: u32 = match tokens[close].kind {
        TokenKind::Gt => 1,
        TokenKind::GtGt => 2,
        TokenKind::GtGtGt => 3,
        _ => return None,
    };
    let floor = close.saturating_sub(TYPE_ARGUMENTS_CAP);
    let mut at = close;
    while at > floor {
        at -= 1;
        match tokens[at].kind {
            TokenKind::Lt => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            TokenKind::Gt => depth += 1,
            TokenKind::GtGt => depth += 2,
            TokenKind::GtGtGt => depth += 3,
            TokenKind::Identifier
            | TokenKind::Keyword(_)
            | TokenKind::Comma
            | TokenKind::Period
            | TokenKind::Question
            | TokenKind::OpenParen
            | TokenKind::CloseParen => {}
            _ => return None,
        }
    }
    None
}

/// Whether a pattern whose first token is `head` stands where an
/// expression may start — the only place a pattern assignment can: after
/// a statement boundary or a control head (`;`, `{`, `}`, `)`, `else`,
/// `do`), as an operand (`=`, `=>`, `return`, `throw`, `yield`, `?`,
/// `:`), or as an element or argument (`(`, `[`, `,`). Everything else
/// heads a non-assignment group: a declaration keyword (`var Square(…) =
/// …`), `factory` (a redirecting factory's formals, #3558), a type or a
/// member name (`String f(int x) = g`, `this.f(int x) = g`).
fn assignment_pattern_context(tokens: &[dartq_scanner::Token], head: usize) -> bool {
    use dartq_scanner::{Keyword, TokenKind};
    head.checked_sub(1).is_some_and(|at| {
        matches!(
            tokens[at].kind,
            TokenKind::Semicolon
                | TokenKind::OpenCurly
                | TokenKind::CloseCurly
                | TokenKind::OpenParen
                | TokenKind::CloseParen
                | TokenKind::OpenSquare
                | TokenKind::Comma
                | TokenKind::Colon
                | TokenKind::Question
                | TokenKind::Eq
                | TokenKind::Function
                | TokenKind::Keyword(
                    Keyword::Return | Keyword::Throw | Keyword::Yield | Keyword::Else | Keyword::Do
                )
        )
    })
}

/// The index just past the written type starting at `at`: an identifier,
/// an optional `.name` (a prefixed type), optional `<…>` arguments and an
/// optional `?`. `None` when `at` is no identifier or the arguments run
/// into a token no type argument list carries.
fn type_run_end(tokens: &[dartq_scanner::Token], at: usize) -> Option<usize> {
    use dartq_scanner::TokenKind;
    let kind = |i: usize| tokens.get(i).map(|t| t.kind);
    if kind(at) != Some(TokenKind::Identifier) {
        return None;
    }
    let mut cursor = at + 1;
    if kind(cursor) == Some(TokenKind::Period) && kind(cursor + 1) == Some(TokenKind::Identifier) {
        cursor += 2;
    }
    if kind(cursor) == Some(TokenKind::Lt) {
        let mut depth = 0i32;
        loop {
            match kind(cursor)? {
                TokenKind::Lt => depth += 1,
                TokenKind::Gt => depth -= 1,
                TokenKind::GtGt => depth -= 2,
                TokenKind::GtGtGt => depth -= 3,
                TokenKind::Identifier
                | TokenKind::Keyword(_)
                | TokenKind::Comma
                | TokenKind::Period
                | TokenKind::Question
                | TokenKind::OpenParen
                | TokenKind::CloseParen => {}
                _ => return None,
            }
            cursor += 1;
            if depth <= 0 {
                break;
            }
        }
    }
    if kind(cursor) == Some(TokenKind::Question) {
        cursor += 1;
    }
    Some(cursor)
}

/// The token span `start..end` of the case pattern holding token `x` — a
/// switch or if-case head (`case P when …:`, `case P)`) or a
/// switch-expression arm (`P when … => …`) — or `None` when `x` sits in
/// no such pattern. The walk back stops at the enclosing `case` keyword
/// or switch-expression body (a statement boundary first means no case
/// pattern); the pattern then has to reach past `x` before its own end, so
/// a case BODY never reads as its head.
fn case_pattern_extent(table: &dartq_scanner::TokenTable, x: usize) -> Option<(usize, usize)> {
    use dartq_scanner::{Keyword, TokenKind};
    // A case pattern never spans more tokens than this.
    const BACKWARD_CAP: usize = 4096;
    let tokens = table.as_slice();
    let floor = x.saturating_sub(BACKWARD_CAP);
    let mut depth = 0i32;
    let mut at = x;
    let start = loop {
        if at <= floor {
            return None;
        }
        at -= 1;
        match tokens[at].kind {
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => depth += 1,
            TokenKind::OpenParen | TokenKind::OpenSquare => depth -= 1,
            TokenKind::OpenCurly => {
                depth -= 1;
                if depth < 0 && switch_expression_body(tokens, at) {
                    break arm_start(table, at, x)?;
                }
            }
            TokenKind::Keyword(Keyword::Case) if depth <= 0 => break at + 1,
            TokenKind::Semicolon if depth <= 0 => return None,
            _ => {}
        }
    };
    let end = pattern_end(table, start)?;
    (x < end).then_some((start, end))
}

/// Whether the `{` at `open` opens a switch EXPRESSION's arm list:
/// `switch (…) {` whose first member is no statement-case label.
fn switch_expression_body(tokens: &[dartq_scanner::Token], open: usize) -> bool {
    use dartq_scanner::{Keyword, TokenKind};
    // A scrutinee never spans more tokens than this.
    const SCRUTINEE_CAP: usize = 4096;
    let Some(close) = open
        .checked_sub(1)
        .filter(|&at| tokens[at].kind == TokenKind::CloseParen)
    else {
        return false;
    };
    let floor = close.saturating_sub(SCRUTINEE_CAP);
    let mut depth = 0i32;
    let mut at = close;
    let scrutinee = loop {
        match tokens[at].kind {
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => depth += 1,
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                depth -= 1;
                if depth == 0 {
                    break at;
                }
            }
            _ => {}
        }
        if at <= floor {
            return false;
        }
        at -= 1;
    };
    scrutinee
        .checked_sub(1)
        .is_some_and(|at| tokens[at].kind == TokenKind::Keyword(Keyword::Switch))
        && !tokens
            .get(open + 1)
            .is_some_and(|t| matches!(t.kind, TokenKind::Keyword(Keyword::Case | Keyword::Default)))
}

/// The first token of the switch-expression arm (body opened at `open`)
/// that holds token `x`: the token after the last top-level `,` before
/// `x`, or the body's first token.
fn arm_start(table: &dartq_scanner::TokenTable, open: usize, x: usize) -> Option<usize> {
    use dartq_scanner::TokenKind;
    let tokens = table.as_slice();
    let mut start = open + 1;
    let mut at = open + 1;
    while at < x {
        match tokens[at].kind {
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                let close = table.matching_end(at)?;
                if close >= x {
                    break;
                }
                at = close + 1;
                continue;
            }
            TokenKind::Comma => start = at + 1,
            _ => {}
        }
        at += 1;
    }
    Some(start)
}

/// The end (exclusive) of the case pattern starting at `start`: the first
/// top-level `when`, `:`, `=>`, `,` or `;`, or the closer of the enclosing
/// group (an if-case's `)`).
fn pattern_end(table: &dartq_scanner::TokenTable, start: usize) -> Option<usize> {
    use dartq_scanner::{Keyword, TokenKind};
    let tokens = table.as_slice();
    let mut at = start;
    while let Some(token) = tokens.get(at) {
        match token.kind {
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                at = table.matching_end(at)? + 1;
                continue;
            }
            TokenKind::Keyword(Keyword::When)
            | TokenKind::Colon
            | TokenKind::Function
            | TokenKind::Comma
            | TokenKind::Semicolon
            | TokenKind::CloseParen
            | TokenKind::CloseSquare
            | TokenKind::CloseCurly => return Some(at),
            _ => {}
        }
        at += 1;
    }
    Some(at)
}

/// Whether the identifier at `at` is a pattern variable's declared name:
/// a `var`/`final` head or a written type right before it (`var a`,
/// `final a`, `int a`, `List<int>? a`), and nothing continuing an
/// expression or naming a field after it (`a(…)`, `a.b`, `a<…>`, `a:`). A
/// relational operand (`> a`) or a constant reference stays out.
fn binder_like(tokens: &[dartq_scanner::Token], at: usize) -> bool {
    use dartq_scanner::{Keyword, TokenKind};
    if tokens[at].kind != TokenKind::Identifier
        || tokens.get(at + 1).is_some_and(|t| {
            matches!(
                t.kind,
                TokenKind::OpenParen
                    | TokenKind::Period
                    | TokenKind::QuestionPeriod
                    | TokenKind::Lt
                    | TokenKind::Colon
            )
        })
    {
        return false;
    }
    let Some(previous) = at.checked_sub(1) else {
        return false;
    };
    match tokens[previous].kind {
        TokenKind::Keyword(Keyword::Var | Keyword::Final) | TokenKind::Identifier => true,
        // A written type's closer or nullable marker (`List<int> a`,
        // `int? a`) — never a relational operator, whose left neighbor is
        // a pattern position, not a type.
        TokenKind::Gt | TokenKind::GtGt | TokenKind::GtGtGt | TokenKind::Question => {
            previous.checked_sub(1).is_some_and(|before| {
                matches!(
                    tokens[before].kind,
                    TokenKind::Identifier
                        | TokenKind::Gt
                        | TokenKind::GtGt
                        | TokenKind::GtGtGt
                        | TokenKind::CloseParen
                )
            })
        }
        _ => false,
    }
}

/// Whether the binders at `a < b` of one pattern sit in different
/// operands of a logical-or. The lowest nesting level the tokens between
/// them reach holds their common ancestor: a logical-or exactly when that
/// level carries a `||` and no element separator (`&&` binds tighter, a
/// `,` makes them siblings of one collection).
fn or_joined(tokens: &[dartq_scanner::Token], a: usize, b: usize) -> bool {
    use dartq_scanner::TokenKind;
    let mut depth = 0i32;
    let mut floor = 0i32;
    let mut or_at_floor = false;
    let mut comma_at_floor = false;
    for token in &tokens[a + 1..b] {
        match token.kind {
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth -= 1;
                if depth < floor {
                    floor = depth;
                    or_at_floor = false;
                    comma_at_floor = false;
                }
            }
            TokenKind::BarBar if depth == floor => or_at_floor = true,
            TokenKind::Comma if depth == floor => comma_at_floor = true,
            _ => {}
        }
    }
    or_at_floor && !comma_at_floor
}

#[cfg(test)]
mod pins {
    use super::super::{FileReport, StrictModes, diagnose_with};
    use crate::collector::collect;
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
    /// the shared single-file rig, at the modern feature posture.
    pub(crate) fn report(source: &str) -> FileReport {
        report_posture(source, None)
    }

    /// [`report`] with the analyzed library's wildcard-variables posture
    /// pinned ([`LibraryClassModifiers::insert_wildcards`], the engine's
    /// per-library feature channel): `Some(false)` models a pre-3.7
    /// library (`// @dart = 3.4`), where the exact-`_` wildcard face is
    /// off and the all-underscores fallback governs.
    pub(crate) fn report_posture(source: &str, wildcard_variables: Option<bool>) -> FileReport {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file = files.intern(Path::new("main.dart"));
        let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
        let features = ParserFeatures::from_feature_set(set);
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
        let elements = collect(source, &scanned.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib = builder.intern("package:t/main.dart");
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
        // The implicit dart:core face (the sibling rigs' minimal seed):
        // core-typed fixtures resolve instead of reporting
        // `undefined_class`. The lone prelude edge is skipped by
        // `report_unused_imports`' alignment (the harness carries no
        // written directives, so the edge list drains to empty).
        builder.seed_implicit_core(
            lib,
            file,
            &[
                "Object", "String", "int", "bool", "double", "num", "List", "Map", "void",
            ],
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let mut index = ElementIndex::default();
        index.insert(file, elements);
        let mut class_modifiers = LibraryClassModifiers::default();
        if let Some(enabled) = wildcard_variables {
            class_modifiers.insert_wildcards(lib, enabled);
        }
        diagnose_with(
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
            &class_modifiers,
            set.is_enabled(dartq_language::Feature::InferenceUpdate2),
        )
    }

    /// The `unused_local_variable` rows of `report`, as (offset, name-token
    /// length) pairs in source order.
    pub(crate) fn unused_local_rows(source: &str) -> Vec<(u32, u32)> {
        report(source)
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "unused_local_variable")
            .map(|d| (d.range.offset, d.range.length))
            .collect()
    }

    /// #2189-a, leak direction (search.dart's `var element = switch (…)`
    /// shape): the arm's pattern variable is out of scope at the
    /// post-switch read, which binds the OUTER declaration. Both locals
    /// are used — the arm variable by the arm body, the outer one by the
    /// post-switch read — so no row reports.
    #[test]
    fn switch_arm_pattern_variable_does_not_steal_post_switch_read() {
        let source = "class Box {\n\
                      \x20 Box e = Box();\n\
                      }\n\
                      Box sink(Box x) => x;\n\
                      void t(Box r) {\n\
                      \x20 var e = switch (r) {\n\
                      \x20   Box(:var e) => e,\n\
                      \x20   _ => null,\n\
                      \x20 };\n\
                      \x20 sink(e);\n\
                      }\n";
        assert_eq!(unused_local_rows(source), vec![], "{source:?}");
    }

    /// #2189-a, leak boundary: without the post-switch read the outer
    /// declaration is genuinely unused and reports at its own token — the
    /// arm variable's in-arm use never spares it.
    #[test]
    fn outer_declaration_still_reports_when_only_the_arm_binding_is_read() {
        let source = "class Box {\n\
                      \x20 Box e = Box();\n\
                      }\n\
                      void t(Box r) {\n\
                      \x20 var e = switch (r) {\n\
                      \x20   Box(:var e) => e,\n\
                      \x20   _ => null,\n\
                      \x20 };\n\
                      }\n";
        let outer_e = source.find("var e = switch").unwrap() as u32 + 4;
        assert_eq!(unused_local_rows(source), vec![(outer_e, 1)], "{source:?}");
    }

    /// #2189-a, under-bind direction (driver.dart's
    /// `if (workingStatistics case var workingStatistics?)` shape): the
    /// body read binds the pattern variable, and the member increment on
    /// it is a useful read of the receiver (the official `_isUsefulRead`
    /// keys on the identifier's own parent, never the statement's
    /// operator) — the pattern variable reports nothing.
    #[test]
    fn if_case_body_increment_spares_the_pattern_variable() {
        let source = "class C {\n\
                      \x20 C f = C();\n\
                      }\n\
                      void t(C? v) {\n\
                      \x20 if (v case var v?) {\n\
                      \x20   v.f++;\n\
                      \x20 }\n\
                      }\n";
        assert_eq!(unused_local_rows(source), vec![], "{source:?}");
    }

    /// #2189-b (drift versioned_schema.dart's clean-file proof): a local
    /// read in receiver position of a member WRITE is a use — the
    /// official `_visitNameExpression` credits the receiver through its
    /// lexical resolution regardless of the member access.
    #[test]
    fn receiver_position_write_spares_the_local() {
        let source = "class D {\n\
                      \x20 D rs = D();\n\
                      }\n\
                      void t() {\n\
                      \x20 var n = D();\n\
                      \x20 n.rs = D();\n\
                      }\n";
        assert_eq!(unused_local_rows(source), vec![], "{source:?}");
    }

    /// #2189-b, broken-member face: the receiver occurrence credits the
    /// local even where the member access then fails (`n.f++` over an
    /// undefined member shape still reads `n` usefully).
    #[test]
    fn receiver_position_increment_spares_the_local() {
        let source = "class D {\n\
                      \x20 D f = D();\n\
                      }\n\
                      void t() {\n\
                      \x20 var n = D();\n\
                      \x20 n.f++;\n\
                      }\n";
        assert_eq!(unused_local_rows(source), vec![], "{source:?}");
    }

    /// #2189-b boundary: only a bare row IS the local — a plain
    /// assignment to the local itself stays a write-only life and
    /// reports.
    #[test]
    fn bare_write_target_still_reports() {
        let source = "class D {}\n\
                      void t() {\n\
                      \x20 var v = D();\n\
                      \x20 v = D();\n\
                      }\n";
        let v = source.find("var v").unwrap() as u32 + 4;
        assert_eq!(unused_local_rows(source), vec![(v, 1)], "{source:?}");
    }

    /// A bare for-in loop target is a WRITE per iteration — the official
    /// `ForEachPartsWithIdentifier` assignment (`_isReadIdentifier`'s
    /// `inGetterContext` fails, unused_local_elements_verifier.dart
    /// :429-453) — so the loop's mention never spares the declaration and
    /// the write-only life reports.
    #[test]
    fn for_in_write_target_still_reports() {
        let source = "void f(List<int> xs) {\n\
                      \x20 int i;\n\
                      \x20 for (i in xs) {}\n\
                      }\n";
        let i = source.find("int i").unwrap() as u32 + 4;
        assert_eq!(unused_local_rows(source), vec![(i, 1)], "{source:?}");
    }

    /// Control for [`Self::for_in_write_target_still_reports`]: a body
    /// read survives the loop-target retraction — only the loop target's
    /// own offset is a write position.
    #[test]
    fn for_in_body_read_spares_the_local() {
        let source = "void sink(int x) {}\n\
                      void f(List<int> xs) {\n\
                      \x20 int i;\n\
                      \x20 for (i in xs) {\n\
                      \x20   sink(i);\n\
                      \x20 }\n\
                      }\n";
        assert_eq!(unused_local_rows(source), vec![], "{source:?}");
    }

    /// A `when` guard's read resolves the guarded head's own binding, so
    /// it never joins the shared-case spelling pair: a second, unguarded
    /// `case int a:` stays a read-free life and reports (each case clause
    /// visits its pattern variables individually,
    /// unused_local_elements_verifier.dart:565-579; the guard credit is
    /// the individual `BindPatternVariableElement`,
    /// resolution_visitor.dart:926-929). dartq anchors the row on the
    /// first binder — the flat frame credits the guard read on the newest
    /// same-name entry, leaving the first entry's own `uses` at zero
    /// while the join refuses to spare it; the oracle anchors the second
    /// binder, so the (severity, code, file) triplet matches and only the
    /// in-file anchor differs.
    #[test]
    fn guard_read_spares_only_its_own_arm() {
        let source = "void f(Object? x) {\n\
                      \x20 switch (x) {\n\
                      \x20   case int a when a > 0:\n\
                      \x20   case int a:\n\
                      \x20     break;\n\
                      \x20 }\n\
                      }\n";
        let a = source.find("case int a").unwrap() as u32 + 9;
        assert_eq!(unused_local_rows(source), vec![(a, 1)], "{source:?}");
    }

    /// The logical-or chain IS one element upstream: both branches bind
    /// the same variable and the guard's read joins them
    /// (`UsedLocalElements.addElement` over the
    /// `JoinPatternVariableElement`'s transitiveVariables,
    /// unused_local_elements_verifier.dart:1237-1241), so the guard-only
    /// read spares every same-name binder of the chain and nothing
    /// reports.
    #[test]
    fn same_arm_or_chain_guard_read_spares_all() {
        let source = "void f(Object? x) {\n\
                      \x20 switch (x) {\n\
                      \x20   case int a || int a when a > 0:\n\
                      \x20     break;\n\
                      \x20 }\n\
                      }\n";
        assert_eq!(unused_local_rows(source), vec![], "{source:?}");
    }

    /// #2189-c (archive bzip2_encoder/zlib rows): a trailing same-line
    /// `// ignore: unused_local_variable` suppresses the row anchored on
    /// that line (`IgnoreInfo.forDart` — a comment with code before it
    /// applies to its own line).
    #[test]
    fn trailing_same_line_ignore_suppresses() {
        let source = "class D {}\n\
                      void t() {\n\
                      \x20 var x = 0; // ignore: unused_local_variable\n\
                      }\n";
        assert_eq!(unused_local_rows(source), vec![], "{source:?}");
    }

    /// #2189-c control: without the comment the row reports.
    #[test]
    fn same_line_without_ignore_still_reports() {
        let source = "class D {}\n\
                      void t() {\n\
                      \x20 var x = 0;\n\
                      }\n";
        let x = source.find("var x").unwrap() as u32 + 4;
        assert_eq!(unused_local_rows(source), vec![(x, 1)], "{source:?}");
    }

    /// #2189-c guard: a `//` inside a string literal is not a comment
    /// head — the decoy text neither suppresses its own row nor mangles
    /// the face.
    #[test]
    fn ignore_text_inside_string_literal_is_not_a_comment() {
        let source = "class D {}\n\
                      void t() {\n\
                      \x20 var s = '// ignore: unused_local_variable';\n\
                      }\n";
        let s = source.find("var s").unwrap() as u32 + 4;
        assert_eq!(unused_local_rows(source), vec![(s, 1)], "{source:?}");
    }

    /// #2189-a identity guard: a later sibling switch at the same stack
    /// depth is never the declaring switch — its arm's read of the same
    /// name binds the OUTER declaration, and the first switch's arm
    /// variable keeps only its own arm's use. No row on either `e`. The
    /// second switch's own binder `z` is never read and reports
    /// individually (#2805: a case-header binder exempts only same-name
    /// siblings of its switch — the first arm's used `e` no longer spares
    /// it, the official per-binder `visitDeclaredVariablePattern` rule).
    #[test]
    fn later_sibling_switch_does_not_revive_the_arm_binding() {
        let source = "class B {\n\
                      \x20 B e = B();\n\
                      }\n\
                      B sink(B x) => x;\n\
                      void t(B r) {\n\
                      \x20 var e = B();\n\
                      \x20 var x = switch (r) {\n\
                      \x20   B(:var e) => e,\n\
                      \x20   _ => null,\n\
                      \x20 };\n\
                      \x20 var y = switch (r) {\n\
                      \x20   B(:var z) => e,\n\
                      \x20   _ => null,\n\
                      \x20 };\n\
                      \x20 sink(x);\n\
                      \x20 sink(y);\n\
                      }\n";
        let z = source.find("var z").unwrap() as u32 + 4;
        assert_eq!(unused_local_rows(source), vec![(z, 1)], "{source:?}");
    }

    /// The `unused_field` rows of `report`, as (offset, name-token length)
    /// pairs in source order.
    fn unused_field_rows(source: &str) -> Vec<(u32, u32)> {
        report(source)
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "unused_field")
            .map(|d| (d.range.offset, d.range.length))
            .collect()
    }

    /// The trailing same-line `// ignore:` form over a field (the archive
    /// deflate.dart census row: `late int _method; // ignore:
    /// unused_field`): an ignore comment whose line carries code before it
    /// spares the row exactly as the standalone form does through the
    /// module root's suppression, which materializes standalone comments
    /// only. The un-ignored twin still reports.
    #[test]
    fn same_line_ignore_spares_the_field() {
        let source = "class C {\n\
                      \x20 int _kept = 0; // ignore: unused_field\n\
                      \x20 int _gone = 0;\n\
                      }\n";
        let gone_at = source.find("_gone").unwrap() as u32;
        assert_eq!(unused_field_rows(source), vec![(gone_at, 5)], "{source:?}");
    }

    /// `unused_element` rows with the diagnostic's message text — the
    /// ctor-selector face asserts on `C._`/`C.make`-shaped display names.
    fn unused_element_rows(source: &str) -> Vec<(u32, String)> {
        report(source)
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "unused_element")
            .map(|d| (d.range.offset, d.message.to_string()))
            .collect()
    }

    /// The element-attributed pool (B): a private named constructor called
    /// through `C.named(...)` keeps the declaration — the selector lives in
    /// the container's `constructors` list, not the member map, so the
    /// resolved-member probe must reach it through `ctor_found` (the corpus
    /// `_ParsedAnalyzerData._` regression). Without the constructor half
    /// the element-level subtraction poisons the ctor and reports.
    #[test]
    fn private_named_constructor_used_via_qualified_call_is_spared() {
        let source = "class _C {\n\
                      \x20 _C.named();\n\
                      }\n\
                      void f() {\n\
                      \x20 _C.named();\n\
                      }\n";
        // The private class itself reports (nothing references `_C` as a
        // type — the ctor call is the only use), but its `named` ctor does
        // not join the report.
        let rows = unused_element_rows(source);
        assert!(
            !rows.iter().any(|(_, text)| text.contains("_C.named")),
            "ctor reported: {rows:?} in {source:?}"
        );
    }

    /// The unnamed-selector half: a factory's `=> _E._()` body resolves the
    /// `_` constructor through the container's `constructors` list — the
    /// oracle's `allowConstructors` cut (`_E._` at 2:6 on the corpus's
    /// `FileUriProperties` family stayed an oracle-only miss before the
    /// ctor probe). The bare `_` spelling must reach `element_used`.
    #[test]
    fn private_unnamed_selector_constructor_used_via_factory_is_spared() {
        let source = "class _E {\n\
                      \x20 _E._();\n\
                      \x20 factory _E.make() => _E._();\n\
                      }\n";
        let rows = unused_element_rows(source);
        // `_` is never a separate reported element — the unnamed-ctor gate
        // keeps it out entirely; the pin is that `_E._` never appears in
        // the message text.
        assert!(
            !rows.iter().any(|(_, text)| text.contains("_E._")),
            "unnamed ctor reported: {rows:?} in {source:?}"
        );
    }

    /// An untyped (`var`-inferred) receiver keeps the under-report: the
    /// oracle's full inference may still resolve `m._x()` to a member
    /// `Receiver::Unknown` cannot name, so the access stays resolved in the
    /// element pool and `_x` spares — the raw `used_names` posture the
    /// subtraction preserves. A resolved-receiver miss is what poisons a
    /// declaration, never an unnameable one.
    #[test]
    fn inferred_receiver_access_spares_private_member() {
        let source = "class C {\n\
                      \x20 void _x() {}\n\
                      }\n\
                      void f() {\n\
                      \x20 var m = C();\n\
                      \x20 m._x();\n\
                      }\n";
        let rows = unused_element_rows(source);
        assert!(
            !rows.iter().any(|(_, text)| text.contains("_x")),
            "member reported: {rows:?} in {source:?}"
        );
    }

    /// The poison direction (B): a private method read only through a
    /// receiver the member walk settles against (`this` on a second,
    /// unrelated container) misses the declaring container's face — the
    /// access never resolved the member, so `used_names`' spelling credit
    /// is revoked and the declaration reports, matching the oracle's
    /// element-level `_isUsedMember`.
    #[test]
    fn this_typed_miss_reports_private_member() {
        let source = "class A {\n\
                      \x20 void _gone() {}\n\
                      }\n\
                      class B {\n\
                      \x20 void f() {\n\
                      \x20   this._gone();\n\
                      \x20 }\n\
                      }\n";
        let gone_at = source.find("_gone").unwrap() as u32;
        let rows = unused_element_rows(source);
        assert!(
            rows.iter().any(|(offset, _)| *offset == gone_at),
            "_gone not reported at {gone_at}: {rows:?} in {source:?}"
        );
    }

    /// #2282: the index-merged read. `H._blOrder[0]` folds the member row
    /// into `Intent::Operator("[]")` — the operand still reads the field's
    /// implicit getter, so the static const is used and no `unused_field`
    /// reports. The oracle reads `C.f[i]` as a getter access on `f`.
    #[test]
    fn static_const_index_read_spares_the_field() {
        let source = "class H {\n\
                      \x20 static const List<int> _blOrder = [3];\n\
                      }\n\
                      void f() {\n\
                      \x20 print(H._blOrder[0]);\n\
                      }\n";
        assert_eq!(unused_field_rows(source), vec![], "{source:?}");
    }

    /// #2282: the instance-field twin — `x._table[0]` reads the getter
    /// before the `[]` application, so the private field is used.
    #[test]
    fn instance_index_read_spares_the_field() {
        let source = "class H {\n\
                      \x20 final List<int> _table = [1, 2];\n\
                      }\n\
                      void f(H h) {\n\
                      \x20 print(h._table[0]);\n\
                      }\n";
        assert_eq!(unused_field_rows(source), vec![], "{source:?}");
    }

    /// #2282: an inherited-field read through a subtype-typed receiver.
    /// `s._inherited` where `s: Sub` reads `Base._inherited` — the join
    /// walks the RECEIVER's supertype closure toward the declaration, so
    /// the base field is used even though `Sub` never names it.
    #[test]
    fn subtype_receiver_read_spares_the_inherited_field() {
        let source = "class Base {\n\
                      \x20 int _inherited = 0;\n\
                      }\n\
                      class Sub extends Base {}\n\
                      void f() {\n\
                      \x20 var s = Sub();\n\
                      \x20 print(s._inherited);\n\
                      }\n";
        assert_eq!(unused_field_rows(source), vec![], "{source:?}");
    }

    /// #2282: an extension body's implicit-`this` read credits the `on`
    /// type's member. `extension X on C { => _isOpen }` reads `C._isOpen`
    /// — `this`'s static type inside the extension IS the extended type,
    /// so the field is used. A same-named field on the extension itself
    /// would not spare (the extension is not the owner).
    #[test]
    fn extension_this_read_spares_the_on_type_field() {
        let source = "class C {\n\
                      \x20 bool _isOpen = false;\n\
                      }\n\
                      extension X on C {\n\
                      \x20 bool get open => _isOpen;\n\
                      }\n\
                      void f(C c) {\n\
                      \x20 print(c.open);\n\
                      }\n";
        assert_eq!(unused_field_rows(source), vec![], "{source:?}");
    }

    /// #2282 boundary: a same-spelled field on a SIBLING class never
    /// spares — only the receiver's own supertype closure reaches the
    /// declaration. `other._f` where `other: Other` (unrelated) reads
    /// `Other._f`, leaving `C._f` unused.
    #[test]
    fn sibling_same_spelled_field_read_does_not_spare() {
        let source = "class C {\n\
                      \x20 int _f = 0;\n\
                      }\n\
                      class Other {\n\
                      \x20 int _f = 1;\n\
                      \x20 int get g => _f;\n\
                      }\n";
        let f_at = source.find("int _f = 0").unwrap() as u32 + 4;
        let rows = unused_field_rows(source);
        assert!(
            rows.iter().any(|(offset, _)| *offset == f_at),
            "C._f not reported at {f_at}: {rows:?} in {source:?}"
        );
    }

    /// The code-filtered rows of `report`, as (offset, length) pairs in
    /// source order — the shared lens for the #2286 unused-family pins.
    fn rows_with_code(source: &str, code: &str) -> Vec<(u32, u32)> {
        report(source)
            .diagnostics
            .iter()
            .filter(|d| d.code.name == code)
            .map(|d| (d.range.offset, d.range.length))
            .collect()
    }

    /// #2286-b: the block-scope local close (`begin_block`'s frame push).
    /// The inner block's `expression` shadows the outer one and dies with
    /// its block — the post-block read binds the OUTER declaration, so
    /// both locals are used and nothing reports. Without the block frame
    /// the read leaked to the dead inner binding and the outer local
    /// reported (`probe2286/b`'s endCase shape).
    #[test]
    fn post_block_read_binds_the_outer_local() {
        let source = "int pop() => 0;\n\
                      void push(Object? x) {}\n\
                      void endCase(bool c) {\n\
                      \x20 var expression = pop();\n\
                      \x20 if (c) {\n\
                      \x20   var expression = pop();\n\
                      \x20   push(expression);\n\
                      \x20 }\n\
                      \x20 push(expression);\n\
                      }\n";
        assert_eq!(unused_local_rows(source), vec![], "{source:?}");
    }

    /// #2286-b boundary: the inner declaration is still reported when only
    /// its own block reads it — the block scope closes it there, never at
    /// the enclosing function's end.
    #[test]
    fn inner_block_local_still_reports_when_unread() {
        let source = "int pop() => 0;\n\
                      void endCase(bool c) {\n\
                      \x20 var outer = pop();\n\
                      \x20 if (c) {\n\
                      \x20   var inner = pop();\n\
                      \x20 }\n\
                      \x20 push2(outer);\n\
                      }\n\
                      void push2(int x) {}\n";
        let inner_at = source.find("var inner").unwrap() as u32 + 4;
        assert_eq!(unused_local_rows(source), vec![(inner_at, 5)], "{source:?}");
    }

    /// #2286-d: a resolving `super.enc` formal supplies the super
    /// constructor's parameter — the target counts as given, so the base
    /// constructor's optional `enc` never reports. The derived formal
    /// itself stays ungiven and still reports (the oracle's split, probed:
    /// `class _D extends _B { _D({super.enc}); }` over `_B({this.enc})`).
    #[test]
    fn resolving_super_formal_spares_the_target_parameter() {
        let source = "class _B {\n\
                      \x20 int? enc;\n\
                      \x20 _B({this.enc});\n\
                      }\n\
                      class _D extends _B {\n\
                      \x20 _D({super.enc});\n\
                      }\n\
                      int f() => _D().enc ?? 0;\n";
        let derived_enc = source.find("super.enc").unwrap() as u32 + 6;
        assert_eq!(
            rows_with_code(source, "unused_element_parameter"),
            vec![(derived_enc, 3)],
            "{source:?}"
        );
        // The super-formal association itself resolved — no
        // without-associated row fires on either side.
        assert!(
            rows_with_code(source, "super_formal_parameter_without_associated_named").is_empty(),
            "{source:?}"
        );
    }

    /// #2286-e: an annotation's own argument group joins the given-at-call
    /// index — `@_C('a', flag: true)` gives `flag`, so the optional named
    /// formal never reports (`_addParametersForArguments` walks every
    /// Annotation's argumentList; the group carries no callee row, which
    /// used to starve the registration).
    #[test]
    fn annotation_named_argument_spares_the_parameter() {
        let source = "class _C {\n\
                      \x20 final String needle;\n\
                      \x20 final bool flag;\n\
                      \x20 const _C(this.needle, {this.flag = false});\n\
                      }\n\
                      @_C('a', flag: true)\n\
                      int g = 0;\n\
                      @_C('b')\n\
                      int h = 0;\n";
        assert_eq!(
            rows_with_code(source, "unused_element_parameter"),
            vec![],
            "{source:?}"
        );
    }

    /// #2286-f1: an invoked operand's operator resolves through the
    /// extension pool — the send's declared return feeds the operator gate
    /// (`isNotNull() & isNotNull()` inside a mixin `on` the operand's
    /// container: the callee return answers through the on-face climb and
    /// `operator &` applies from the extension). Without the feed the gate
    /// read `Unknown` and `undefined_operator` fired where the oracle
    /// stays silent (the drift column.dart shape).
    #[test]
    fn invoked_operand_resolves_the_extension_operator() {
        let source = "class Expression<T extends Object> {\n\
                      \x20 Expression<bool> isNotNull() => Expression<bool>();\n\
                      }\n\
                      extension BoolOps on Expression<bool> {\n\
                      \x20 Expression<bool> operator &(Expression<bool> other) => this;\n\
                      }\n\
                      mixin Column<D extends Object> on Expression<D> {\n\
                      \x20 Expression<bool> t() => isNotNull() & isNotNull();\n\
                      }\n";
        assert_eq!(
            rows_with_code(source, "undefined_operator"),
            vec![],
            "{source:?}"
        );
    }

    /// #2286-d second half: a preceding-line `// ignore:` citation spares
    /// the `unused_element_parameter` row only when it NAMES the code (the
    /// fold's [`IgnoreSuppression`] applies at the assembly point,
    /// `IgnoredDiagnosticName._matches` — name or canonical uniqueName).
    /// A `unused_element` citation is a different code: the row stands
    /// (probe2286f/igp — the oracle spares `_b` and reports `_c`).
    #[test]
    fn preceding_line_ignore_spares_only_the_named_parameter_code() {
        let source = "class _F {\n\
                      \x20 final int a;\n\
                      \x20 // ignore: unused_element_parameter\n\
                      \x20 _F(this.a, [this._b]);\n\
                      \x20 final int _b;\n\
                      }\n\
                      class _H {\n\
                      \x20 final int a;\n\
                      \x20 // ignore: unused_element\n\
                      \x20 _H(this.a, [this._c]);\n\
                      \x20 final int _c;\n\
                      }\n\
                      void main() {\n\
                      \x20 _F(1)._b;\n\
                      \x20 _H(1)._c;\n\
                      }\n";
        let c_at = source.find("this._c").unwrap() as u32 + 5;
        assert_eq!(
            rows_with_code(source, "unused_element_parameter"),
            vec![(c_at, 2)],
            "{source:?}"
        );
    }

    /// #2326: the SDK deprecation annotation normalizes exactly like
    /// upstream `normalizeDeprecationMessage` — trim; empty or a bare `.`
    /// collapses to `None` (the plain `deprecatedMemberUse` shape), a
    /// `.`/`?`/`!` tail keeps, anything else gains a terminal period.
    #[test]
    fn deprecation_annotation_normalizes_like_upstream() {
        assert_eq!(super::normalize_deprecation(""), None);
        assert_eq!(super::normalize_deprecation("   "), None);
        assert_eq!(super::normalize_deprecation("."), None);
        assert_eq!(super::normalize_deprecation("  .  "), None);
        assert_eq!(
            super::normalize_deprecation("Use dart:js_interop instead."),
            Some("Use dart:js_interop instead.".to_string())
        );
        assert_eq!(
            super::normalize_deprecation("Done?"),
            Some("Done?".to_string())
        );
        assert_eq!(
            super::normalize_deprecation("  may be removed  "),
            Some("may be removed.".to_string())
        );
    }

    /// #2745: a cyclic same-file superclass chain (mutual `extends`)
    /// must not spin the `ur_member_site` climb — the member row
    /// resolves on the chain with no UseResult face anywhere, so the
    /// walk terminates (revisit = chain-broken terminus) and reports
    /// nothing.
    #[test]
    fn mutual_extends_cycle_terminates_the_ur_member_climb() {
        let source = "class A extends B {\n\
                      \x20 void m() {}\n\
                      }\n\
                      class B extends A {}\n\
                      void t(A a) {\n\
                      \x20 a.m();\n\
                      }\n";
        let r = report(source);
        assert_eq!(r.diagnostics, vec![], "{source:?}");
    }

    /// #2745: the self-shadowed edge (`class S extends S`) is the same
    /// climb meeting its own name one hop out — terminates, reports
    /// nothing.
    #[test]
    fn self_extends_shadow_edge_terminates_the_ur_member_climb() {
        let source = "class S extends S {\n\
                      \x20 void m() {}\n\
                      }\n\
                      void t(S s) {\n\
                      \x20 s.m();\n\
                      }\n";
        let r = report(source);
        assert_eq!(r.diagnostics, vec![], "{source:?}");
    }

    // —— the per-library wildcard gate (#2888 1a) ————————————————————————

    /// The pre-3.7 local-function `_` (DeadCodeTest
    /// test_localFunction_wildcard_preWildcards#0): with the
    /// wildcard-variables feature off, a lone `_` is an ordinary name —
    /// the unused local function reports (`_visitLocalFunctionElement`'s
    /// `_wildCardVariablesEnabled && _isNamedWildcard` cut demands the
    /// feature explicitly). The old hard-coded gate answered `true`
    /// unconditionally and under-reported exactly this shape.
    #[test]
    fn local_function_wildcard_pre_feature_reports() {
        let source = "void f() {\n\
                      \x20 _(){}\n\
                      }\n";
        let at = source.find('_').unwrap() as u32;
        let rows = report_posture(source, Some(false));
        let rows: Vec<_> = rows
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "unused_element")
            .map(|d| d.range.offset)
            .collect();
        assert_eq!(rows, vec![at], "{source:?}");
    }

    /// The ≥3.7 twin: the same `_()` local function is a wildcard (name
    /// `_`, kind LocalFunction, feature on — extensions.dart:81's one
    /// gate for all five kinds), so the unused-element face stays silent.
    /// The whole-declaration `dead_code` row still reports (the
    /// DeadCodeVerifier's own `_` face, dead_code_verifier.dart:72-82) —
    /// the pin is that nothing else reports beside it.
    #[test]
    fn local_function_wildcard_feature_enabled_stays_silent() {
        let source = "void f() {\n\
                      \x20 _(){}\n\
                      }\n";
        let r = report(source);
        assert!(
            r.diagnostics.iter().all(|d| d.code.name == "dead_code"),
            "{:?} in {source:?}",
            r.diagnostics
        );
    }

    /// `__`/`___` are never wildcards under the feature (only the exact
    /// `_` is) and report; below 3.7 the all-underscores fallback spares
    /// them — the two postures diverge exactly here.
    #[test]
    fn multi_underscore_local_splits_by_posture() {
        let source = "void f() {\n\
                      \x20 var __ = 0;\n\
                      \x20 var ___ = 1;\n\
                      }\n";
        let dunder = source.find("__").unwrap() as u32;
        let triunder = source.find("___").unwrap() as u32;
        assert_eq!(
            unused_local_rows(source),
            vec![(dunder, 2), (triunder, 3)],
            "{source:?}"
        );
        let disabled = report_posture(source, Some(false));
        let rows: Vec<_> = disabled
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "unused_local_variable")
            .collect();
        assert!(rows.is_empty(), "{source:?}");
    }

    /// The local-variable `_` across both postures: silent with the
    /// feature (the exact-`_` wildcard) and silent below 3.7 (the
    /// all-underscores fallback spares it too — the local-VARIABLE arm
    /// calls `_isNamedWildcard` without the explicit feature conjunct,
    /// unlike the local-function arm).
    #[test]
    fn local_variable_underscore_silent_under_both_postures() {
        let source = "void f() {\n\
                      \x20 var _ = 0;\n\
                      }\n";
        assert_eq!(unused_local_rows(source), vec![], "{source:?}");
        assert_eq!(
            report_posture(source, Some(false)).diagnostics,
            vec![],
            "{source:?}"
        );
    }

    // —— the forward-reference credit (#2888 1b) ——————————————————————————

    /// ReferencedBeforeDeclarationTest test_hideInBlock_local: `print(v)`
    /// before `var v = 2` binds the LOCAL in the oracle (the read reports
    /// `referenced_before_declaration` and marks the element referenced),
    /// so the local never reports unused. dartq's read bound the
    /// top-level `v` and the un-referenced local staged a spurious row —
    /// the forward-reference credit drops it.
    #[test]
    fn forward_reference_credits_the_local() {
        let source = "var v = 1;\n\
                      void main() {\n\
                      \x20 print(v);\n\
                      \x20 var v = 2;\n\
                      }\n\
                      print(x) {}\n";
        assert_eq!(unused_local_rows(source), vec![], "{source:?}");
    }

    /// test_hideInBlock_local_subBlock: the read sits in a nested block,
    /// the declaration in the enclosing block afterwards — the
    /// declaration's block still encloses the read, so the credit holds.
    /// The inline control is the sibling-block boundary: a read in a
    /// SIBLING block (the declaration's block closed on the way) never
    /// credits, and the genuinely unused local keeps its row.
    #[test]
    fn nested_block_forward_reference_credits_sibling_does_not() {
        let source = "var v = 1;\n\
                      void main() {\n\
                      \x20 {\n\
                      \x20   print(v);\n\
                      \x20 }\n\
                      \x20 var v = 2;\n\
                      }\n\
                      print(x) {}\n";
        assert_eq!(unused_local_rows(source), vec![], "{source:?}");
        let control = "void v() {}\n\
                       void main() {\n\
                       \x20 {\n\
                       \x20   print(v);\n\
                       \x20 }\n\
                       \x20 {\n\
                       \x20   var v = 2;\n\
                       \x20 }\n\
                       }\n\
                       print(x) {}\n";
        let at = control.find("var v = 2").unwrap() as u32 + 4;
        assert_eq!(unused_local_rows(control), vec![(at, 1)], "{control:?}");
    }

    /// test_hideInBlock_function: the read also credits a LOCAL FUNCTION
    /// declared after it (`v() {}` — the element the oracle's rbd binds),
    /// so no `unused_element` row reports for it.
    #[test]
    fn forward_reference_credits_the_local_function() {
        let source = "var v = 1;\n\
                      void main() {\n\
                      \x20 print(v);\n\
                      \x20 v() {}\n\
                      }\n\
                      print(x) {}\n";
        let r = report(source);
        assert_eq!(r.diagnostics, vec![], "{source:?}");
    }

    // -- Phase 2b: the prefixed-import evidence rework -------------------
    // (The invocation-form `call` credit of the same upstream family —
    // corpus FunctionExpressionInvocationTest/test_expression_interfaceType_nullable_hasCall#0,
    // `a()` over an extension-covered receiver — has no finalization-time
    // evidence: the frame's locals and written types are truncated per
    // frame before this pass. Its wiring point is the resolver's invoke
    // arm, where the `Local` binding and the declaration-site type are
    // both alive; the face is reported as held-file wiring.)

    /// The `unused_import` rows of `r`, as (offset, name-token length)
    /// pairs in source order.
    fn import_rows(r: &FileReport) -> Vec<(u32, u32)> {
        r.diagnostics
            .iter()
            .filter(|d| d.code.name == "unused_import")
            .map(|d| (d.range.offset, d.range.length))
            .collect()
    }

    /// One aux file of the import rig: its body, the written prefix (if
    /// any), and the deferred bit — the import edge appended in written
    /// order.
    struct Aux {
        prefix: Option<&'static str>,
        deferred: bool,
        body: &'static str,
    }

    /// [`report`] with written import directives: each [`Aux`] is
    /// collected and declared into its own graph library node
    /// (`package:t/a<n>.dart`), wired into the main library through an
    /// import edge in written order — the positional alignment
    /// `report_unused_imports` requires (the harness graph carries no
    /// `dart:core` prelude, so no edge is skipped). The main file
    /// registers as the library's defining unit, so the directive pass's
    /// library-local shadow face sees the unit set.
    fn report_with_imports(source: &str, aux: &[Aux]) -> FileReport {
        report_with_imports_parts(source, aux, &[])
    }

    /// [`report_with_imports`] with part units: each body is collected and
    /// declared into the main library under its own file id, registered as
    /// a compilation unit — the `part` tree the caller folds in production.
    fn report_with_imports_parts(source: &str, aux: &[Aux], parts: &[&'static str]) -> FileReport {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file = files.intern(Path::new("main.dart"));
        let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
        let features = ParserFeatures::from_feature_set(set);
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
        let elements = collect(source, &scanned.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib = builder.intern("package:t/main.dart");
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
        builder.unit(lib, file);
        // The implicit dart:core face (the sibling rigs' minimal seed):
        // without it every core-typed fixture reports `undefined_class`,
        // which both pollutes the rows and trips the import-warning gate.
        // Seeded first so the prelude edge sits at index 0, where
        // `report_unused_imports`' alignment skips it.
        builder.seed_implicit_core(
            lib,
            file,
            &[
                "Object", "String", "int", "bool", "double", "num", "List", "Map", "void",
            ],
        );
        let mut part_units = Vec::new();
        for (position, body) in parts.iter().enumerate() {
            let path = format!("p{position}.dart");
            let part_file = files.intern(Path::new(&path));
            let scanned_part = dartq_scanner::scan(
                body,
                ScanConfig {
                    enable_triple_shift: true,
                    enable_augmentations: true,
                },
                part_file,
            );
            assert!(
                scanned_part.diagnostics.is_empty(),
                "scanner diagnostics: {body}"
            );
            let part_elements = collect(body, &scanned_part.tokens, features, &strings);
            for (name, decl) in part_elements.declarations() {
                if decl.kind == MicroKind::TypeParam {
                    continue;
                }
                builder.declare(lib, name, part_file);
            }
            builder.unit(lib, part_file);
            part_units.push((part_file, part_elements));
        }
        let mut aux_units = Vec::new();
        for (position, item) in aux.iter().enumerate() {
            let path = format!("a{position}.dart");
            let aux_file = files.intern(Path::new(&path));
            let scanned_aux = dartq_scanner::scan(
                item.body,
                ScanConfig {
                    enable_triple_shift: true,
                    enable_augmentations: true,
                },
                aux_file,
            );
            assert!(
                scanned_aux.diagnostics.is_empty(),
                "scanner diagnostics: {}",
                item.body
            );
            let aux_elements = collect(item.body, &scanned_aux.tokens, features, &strings);
            let aux_lib = builder.intern(&format!("package:t/{path}"));
            for (name, decl) in aux_elements.declarations() {
                if decl.kind == MicroKind::TypeParam {
                    continue;
                }
                builder.declare(aux_lib, name, aux_file);
            }
            builder.import(
                lib,
                ImportEdge {
                    target: Some(aux_lib),
                    prefix: item.prefix.map(|p| strings.intern(p)),
                    deferred: item.deferred,
                    non_library: false,
                    combinators: Combinators::new(std::iter::empty(), &strings),
                },
            );
            aux_units.push((aux_file, aux_elements));
        }
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let mut index = ElementIndex::default();
        index.insert(file, elements);
        for (part_file, part_elements) in part_units {
            index.insert(part_file, part_elements);
        }
        for (aux_file, aux_elements) in aux_units {
            index.insert(aux_file, aux_elements);
        }
        diagnose_with(
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
        )
    }

    /// Corpus `MethodInvocationResolutionTest/
    /// test_hasReceiver_deferredImportPrefix_loadLibrary#0`: `p.loadLibrary()`
    /// resolves through `PrefixScope.lookup`'s early return (the synthetic
    /// `loadLibraryFunction` answers before the namespace lookup ever
    /// credits), so the deferred import still reports `unused_import` —
    /// and nothing else (the loadLibrary face never turns undefined).
    #[test]
    fn load_library_invoke_keeps_the_deferred_import_unused() {
        let source = "import 'a.dart' deferred as p;\n\
                      main() {\n\
                      \x20 p.loadLibrary();\n\
                      }\n";
        let r = report_with_imports(
            source,
            &[Aux {
                prefix: Some("p"),
                deferred: true,
                body: "class A {}\n",
            }],
        );
        let uri = source.find("'a.dart'").unwrap() as u32;
        assert_eq!(import_rows(&r), vec![(uri, 8)], "{source:?}");
        assert!(
            r.diagnostics
                .iter()
                .all(|d| !d.code.name.starts_with("undefined_")),
            "{source:?}: {:?}",
            r.diagnostics
        );
    }

    /// Corpus `PrefixShadowedByLocalDeclarationTest/
    /// test_local_variable_type_inside_function_with_shadowing_variable_before#0`
    /// (dart:async modeled by an aux `Future`): the local `a`'s own uses
    /// share the prefix's spelling, and the shadowed `a.Future` type head
    /// is dead — the import must still report `unused_import` beside the
    /// `prefixShadowedByLocalDeclaration` face, never held open by the
    /// spelling collision.
    #[test]
    fn prefix_shadowed_by_local_still_reports_unused_import() {
        let source = "import 'a.dart' as a;\n\
                      f() {\n\
                      \x20 int a = 0;\n\
                      \x20 a.Future? x = null;\n\
                      \x20 return [x, a];\n\
                      }\n";
        let r = report_with_imports(
            source,
            &[Aux {
                prefix: Some("a"),
                deferred: false,
                body: "class Future {}\n",
            }],
        );
        let uri = source.find("'a.dart'").unwrap() as u32;
        assert_eq!(import_rows(&r), vec![(uri, 8)], "{source:?}");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code.name == "prefix_shadowed_by_local_declaration"),
            "{source:?}: {:?}",
            r.diagnostics
        );
    }

    /// Corpus `UnusedImportTest/test_library_prefixed_samePrefix_notUsed#0`:
    /// two imports share the prefix `one` and only `one.A` resolves — the
    /// element-level attribution names the first target alone, so only the
    /// second directive reports (the shared spelling must not hold both
    /// open).
    #[test]
    fn prefixed_same_prefix_attribution_reports_only_the_unresolved_edge() {
        let source = "import 'a.dart' as one;\n\
                      import 'b.dart' as one;\n\
                      one.A a = one.A();\n";
        let r = report_with_imports(
            source,
            &[
                Aux {
                    prefix: Some("one"),
                    deferred: false,
                    body: "class A {}\n",
                },
                Aux {
                    prefix: Some("one"),
                    deferred: false,
                    body: "class B {}\n",
                },
            ],
        );
        let second = source.find("'b.dart'").unwrap() as u32;
        assert_eq!(import_rows(&r), vec![(second, 8)], "{source:?}");
    }

    /// The same-prefix control: a second edge whose target does export the
    /// attributed name stays used — the attribution join, not the shared
    /// prefix spelling, decides.
    #[test]
    fn prefixed_same_prefix_both_exporting_stay_used() {
        let source = "import 'a.dart' as one;\n\
                      import 'b.dart' as one;\n\
                      one.A a = one.A();\n\
                      one.B b = one.B();\n";
        let r = report_with_imports(
            source,
            &[
                Aux {
                    prefix: Some("one"),
                    deferred: false,
                    body: "class A {}\n",
                },
                Aux {
                    prefix: Some("one"),
                    deferred: false,
                    body: "class B {}\n",
                },
            ],
        );
        assert_eq!(import_rows(&r), vec![], "{source:?}");
        assert_eq!(r.diagnostics, vec![], "{source:?}");
    }

    /// The import-warning gate face (the deprecated/experimental corpus
    /// family, `DeprecatedMemberUse_PackageConfigWorkspaceTest/
    /// test_constructor_inAnnotation#0`): an `undefined_annotation`
    /// anywhere in the unit skips the whole import pass — the annotation
    /// name might have come from the import — so the otherwise-unused
    /// import stays silent (`library_analyzer.dart`
    /// `_hasDiagnosticReportedThatPreventsImportWarnings`).
    #[test]
    fn undefined_annotation_gates_the_import_pass() {
        let source = "import 'a.dart';\n\
                      @nope\n\
                      class C {}\n";
        let r = report_with_imports(
            source,
            &[Aux {
                prefix: None,
                deferred: false,
                body: "class A {}\n",
            }],
        );
        assert_eq!(import_rows(&r), vec![], "{source:?}");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code.name == "undefined_annotation"),
            "{source:?}: {:?}",
            r.diagnostics
        );
    }

    /// The gate control: without the annotation face the pass runs and the
    /// unused import reports — the rig itself never silences it.
    #[test]
    fn unused_import_reports_without_a_gating_face() {
        let source = "import 'a.dart';\n\
                      class C {}\n";
        let r = report_with_imports(
            source,
            &[Aux {
                prefix: None,
                deferred: false,
                body: "class A {}\n",
            }],
        );
        let uri = source.find("'a.dart'").unwrap() as u32;
        assert_eq!(import_rows(&r), vec![(uri, 8)], "{source:?}");
    }

    /// The prefixed-import control: a resolved `p.A` member read credits
    /// exactly the edge whose target exports the name — the evidence
    /// model's baseline posture (the shadow/same-prefix pins hold the
    /// delta faces).
    #[test]
    fn prefixed_member_read_credits_the_exporting_edge() {
        let source = "import 'a.dart' as p;\n\
                      var x = p.A();\n";
        let r = report_with_imports(
            source,
            &[Aux {
                prefix: Some("p"),
                deferred: false,
                body: "class A {}\n",
            }],
        );
        assert_eq!(r.diagnostics, vec![], "{source:?}");
    }

    /// Corpus `UnusedImportTest/test_library_prefixed#0` family control,
    /// gate side (`regress/regress21793_test`'s `CREATION_WITH_NON_TYPE`
    /// marker): a prefixed creation miss on a resolvable prefix emits
    /// `creation_with_non_type` — the machine spelling BOTH upstream gate
    /// entries `constWithNonType`/`newWithNonType` share
    /// (diagnostic.g.dart :3237/:12035) — and the gate skips the whole
    /// import pass, so the untouched import stays silent.
    #[test]
    fn creation_with_non_type_gates_the_import_pass() {
        let source = "import 'a.dart' as p;\n\
                      main() {\n\
                      \x20 new p.B();\n\
                      }\n";
        let r = report_with_imports(
            source,
            &[Aux {
                prefix: Some("p"),
                deferred: false,
                body: "class A {}\n",
            }],
        );
        assert_eq!(import_rows(&r), vec![], "{source:?}");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code.name == "creation_with_non_type"),
            "{source:?}: {:?}",
            r.diagnostics
        );
    }

    /// Corpus `unsorted/extends_test`: the local `class A` shadows the
    /// imported one — `S extends A` resolves the LOCAL declaration, the
    /// library scope answers before any import is consulted, and the name
    /// overlap must not credit `a.dart` (the import reports unused).
    #[test]
    fn local_declaration_shadows_the_imported_name() {
        let source = "import 'a.dart';\n\
                      class S extends A {}\n\
                      class A {}\n";
        let r = report_with_imports(
            source,
            &[Aux {
                prefix: None,
                deferred: false,
                body: "class A {}\n",
            }],
        );
        let uri = source.find("'a.dart'").unwrap() as u32;
        assert_eq!(import_rows(&r), vec![(uri, 8)], "{source:?}");
    }

    /// Corpus `part/of_multiple_libs_test`: the shadowing declaration
    /// lives in a PART unit (`part of` file declaring `foo`), and the
    /// library scope covers every unit — the part's `foo` answers `m()`'s
    /// call locally, so the import of the also-exporting library reports
    /// unused.
    #[test]
    fn part_declared_name_shadows_the_imported_name() {
        let source = "import 'a.dart';\n\
                      part 'p0.dart';\n\
                      void m() {\n\
                      \x20 foo();\n\
                      }\n";
        let r = report_with_imports_parts(
            source,
            &[Aux {
                prefix: None,
                deferred: false,
                body: "void foo() {}\n",
            }],
            &["part of 'main.dart';\nvoid foo() {}\n"],
        );
        let uri = source.find("'a.dart'").unwrap() as u32;
        assert_eq!(import_rows(&r), vec![(uri, 8)], "{source:?}");
    }

    /// Corpus `Extension-methods/accessibility_A01_t02` (and the
    /// wildcard-prefix twin `wildcard_variables/import/import_test`): an
    /// extension reached through a PREFIXED import applies implicitly —
    /// `"".id1` needs no bare prefix use — and `notifyExtensionUsed`
    /// credits the prefix-scoped edge exactly as the unprefixed arm's
    /// (scope.dart :204-321 walks the tracking map per prefix scope), so
    /// the import stays used.
    #[test]
    fn prefixed_extension_application_spares_the_import() {
        let source = "import 'a.dart' as p;\n\
                      void m() {\n\
                      \x20 ''.id1;\n\
                      }\n";
        let r = report_with_imports(
            source,
            &[Aux {
                prefix: Some("p"),
                deferred: false,
                body: "extension StringExt on String {\n  String get id1 => 'StringExt';\n}\n",
            }],
        );
        assert_eq!(import_rows(&r), vec![], "{source:?}");
    }

    /// #2898-a: a super-initialization's NAMED argument gives the resolved
    /// super constructor's formal — `super(e: 1)` supplies `_A({this.e})`'s
    /// `e` (`_addParametersForArguments` at `visitSuperConstructorInvocation`
    /// marks `argument.correspondingParameter` used), so the field formal
    /// never reports. The corpus shape
    /// (`test_classPrivate_secondaryConstructor_unnamed_fieldFormal_
    /// optionalNamed_superInvocation_isUsed`) used to report `_A.e` — the
    /// parse-side argument walk lives in the held invocation lane, so the
    /// credit runs from this pass over the token table.
    #[test]
    fn super_invocation_named_argument_spares_the_target_formal() {
        let source = "class _A {\n\
                      \x20 final int? e;\n\
                      \x20 _A({this.e});\n\
                      }\n\
                      \n\
                      class _B extends _A {\n\
                      \x20 _B([int? e]) : super(e: 1);\n\
                      }\n\
                      \n\
                      var b = _B(1);\n";
        assert_eq!(
            rows_with_code(source, "unused_element_parameter"),
            vec![],
            "{source:?}"
        );
    }

    /// #2898-b: the positional counterpart — `super(e)` supplies
    /// `_A([this.e])`'s first positional formal, and the supplied-argument
    /// count the credit records spares it (`isUsed` corpus twin of the
    /// positional shape).
    #[test]
    fn super_invocation_positional_argument_spares_the_target_formal() {
        let source = "class _A {\n\
                      \x20 final int? e;\n\
                      \x20 _A([this.e]);\n\
                      }\n\
                      \n\
                      class _B extends _A {\n\
                      \x20 _B(int e) : super(e);\n\
                      }\n\
                      \n\
                      var b = _B(1);\n";
        assert_eq!(
            rows_with_code(source, "unused_element_parameter"),
            vec![],
            "{source:?}"
        );
    }

    /// #2898-c: a named super-initialization keys the credit on the written
    /// selector — `super._a(e: 1)` supplies `_A._a`'s formal, not the
    /// unnamed face's (the same spelling key `credit_super_formal_givens`
    /// writes); a credit keyed on the superclass alone would spare nothing
    /// here and `_A._a`'s `e` would report.
    #[test]
    fn super_invocation_selector_keys_the_credit() {
        let source = "class _A {\n\
                      \x20 final int? e;\n\
                      \x20 _A._a({this.e});\n\
                      \x20 _A();\n\
                      }\n\
                      \n\
                      class _B extends _A {\n\
                      \x20 _B() : super._a(e: 1);\n\
                      }\n\
                      var b = _B();\n";
        assert_eq!(
            rows_with_code(source, "unused_element_parameter"),
            vec![],
            "{source:?}"
        );
    }

    /// #2898-d: the per-parameter override gate (`_overridesUsedParameter`)
    /// — an override chain whose corresponding parameter is used nowhere
    /// reports BOTH sides. The blanket super-name pardon used to spare
    /// `_B._m` wholesale (the miss side of the face); the gate spares only
    /// when the overridden member's corresponding parameter is required or
    /// used.
    #[test]
    fn override_chain_with_unused_param_reports_both_sides() {
        let source = "class _A {\n\
                      \x20 void _m({int? x}) {}\n\
                      }\n\
                      \n\
                      class _B extends _A {\n\
                      \x20 void _m({int? x}) {}\n\
                      }\n";
        let a_x = source.find("int? x").unwrap() as u32 + 5;
        let b_x = source.rfind("int? x").unwrap() as u32 + 5;
        assert_eq!(
            rows_with_code(source, "unused_element_parameter"),
            vec![(a_x, 1), (b_x, 1)],
            "{source:?}"
        );
    }

    /// #2898-e: the constructor's super-initialization corresponding face
    /// (`_isUsedElement`'s ConstructorElement block): `_B`'s optional
    /// `super.b` corresponds to `A.b` — a REQUIRED named parameter — so it
    /// spares (`overrideRequired_noDiagnostic` corpus shape; the implicit
    /// `super()` resolves the unnamed super constructor). Without the face
    /// the formal reported.
    #[test]
    fn super_formal_corresponding_required_spares() {
        let source = "class A {\n\
                      \x20 A({required this.a, required this.b});\n\
                      \x20 final String a;\n\
                      \x20 final String b;\n\
                      }\n\
                      \n\
                      class _B extends A {\n\
                      \x20 _B({required super.a, super.b = 'b'});\n\
                      }\n\
                      \n\
                      var foo = _B(a: 'a');\n";
        assert_eq!(
            rows_with_code(source, "unused_element_parameter"),
            vec![],
            "{source:?}"
        );
    }

    /// #2898-f: the override gate's used arm — the overridden executable's
    /// own body references the formal (`_useIdentifierElement` credits the
    /// parameter element), so the override's corresponding formal spares.
    /// Must not report (the negative against over-tightening).
    #[test]
    fn override_used_corresponding_spares() {
        let source = "class _A {\n\
                      \x20 void _m({int? x}) {\n\
                      \x20   if (x != null) {}\n\
                      \x20 }\n\
                      }\n\
                      \n\
                      class _B extends _A {\n\
                      \x20 void _m({int? x}) {}\n\
                      }\n";
        assert_eq!(
            rows_with_code(source, "unused_element_parameter"),
            vec![],
            "{source:?}"
        );
    }

    /// #2898-g: the partial-chain negative — the override gate walks every
    /// direct super face, and the required arm holds across them:
    /// `_B._m`'s `x` corresponds to a required parameter on the superclass
    /// face and to an optional-unused one on the interface face, so it
    /// spares (reporting on the interface answer alone would create the
    /// extra the tightening forbids). `_I._m`'s own `x` reports exactly as
    /// upstream — a private method of a private interface is not publicly
    /// accessible and overrides nothing.
    #[test]
    fn override_multi_face_required_holds() {
        let source = "class _A {\n\
                      \x20 void _m({required int x}) {}\n\
                      }\n\
                      \n\
                      abstract class _I {\n\
                      \x20 void _m({int? x});\n\
                      }\n\
                      \n\
                      class _B extends _A implements _I {\n\
                      \x20 void _m({int? x}) {}\n\
                      }\n";
        let i_x = source.find("int? x").unwrap() as u32 + 5;
        assert_eq!(
            rows_with_code(source, "unused_element_parameter"),
            vec![(i_x, 1)],
            "{source:?}"
        );
    }

    /// #2898-h: the override gate's given arm — an invocation supplying the
    /// corresponding parameter (`a._m(1)` maps its argument to `_A._m`'s
    /// first positional formal) spares the override's corresponding formal
    /// through the spelling-keyed given index.
    #[test]
    fn override_invocation_given_spares_the_override() {
        let source = "class _A {\n\
                      \x20 void _m([int? x]) {}\n\
                      }\n\
                      \n\
                      class _B extends _A {\n\
                      \x20 void _m([int? x]) {}\n\
                      }\n\
                      \n\
                      void t(_A a) {\n\
                      \x20 a._m(1);\n\
                      }\n";
        assert_eq!(
            rows_with_code(source, "unused_element_parameter"),
            vec![],
            "{source:?}"
        );
    }

    /// #2911-a, the nameless recovery face (the census `unused_element`
    /// over-fire family, fragment_offset `_missingName` corpus): a class
    /// whose name the parser never produced reports `missing_identifier`
    /// and NOTHING else — the element answers `isPrivate` through its
    /// fragment and `FragmentImpl.isPrivate` maps a null name to PUBLIC
    /// (`InstanceElementImpl.isPrivate` → `_firstFragment.isPrivate`,
    /// element.dart:4539-4544, upstream Dart SDK source), so the
    /// `<unnamed>` face never joins the unused walk. Before the gate fix
    /// this fired `The declaration '<unnamed>' isn't referenced.` at 1:1.
    #[test]
    fn nameless_class_recovery_is_silent() {
        let source = "library;\n\n\
                      class {}\n";
        assert_eq!(unused_element_rows(source), vec![], "{source:?}");
    }

    /// #2911-b: the class-type-alias recovery shape (`class = Object with
    /// M;` — the name token is missing, the recovered head carries a null
    /// name): same public-face exemption, the `with M` clause keeps `M`
    /// alive and the `<unnamed>` alias face stays silent.
    #[test]
    fn nameless_class_type_alias_recovery_is_silent() {
        let source = "mixin M {}\n\
                      class = Object with M;\n";
        assert_eq!(unused_element_rows(source), vec![], "{source:?}");
    }

    /// #2911-c: the enum recovery shape — a nameless enum declaration is
    /// public through the same fragment route; its constant's
    /// `unused_field` face is outside this pin (the oracle reports the
    /// constant separately when unused).
    #[test]
    fn nameless_enum_recovery_is_silent() {
        let source = "enum { v; }\n";
        assert_eq!(unused_element_rows(source), vec![], "{source:?}");
    }

    /// #2911-d, the errored-declaration face (constructor_body
    /// `test_enum_secondaryConstructor_constFactory_blockBody` corpus): a
    /// const factory with a block body carries the `constFactory` error,
    /// and the constructor still never reports unused — `_isPubliclyAccessible`'s
    /// EnumElement arm gates only GENERATIVE constructors
    /// (`element.isGenerative`, unused_local_elements_verifier.dart:
    /// 1010-1014, upstream Dart SDK source); the factory falls through to
    /// the interface arm and a public enum's factory is exported. Before
    /// the gate fix this fired `The declaration 'E.named' isn't
    /// referenced.`.
    #[test]
    fn errored_enum_factory_constructor_is_silent() {
        let source = "enum E {\n\
                      \x20 v;\n\
                      \x20 const E();\n\
                      \x20 const factory E.named() {\n\
                      \x20   return v;\n\
                      \x20 }\n\
                      }\n";
        assert_eq!(unused_element_rows(source), vec![], "{source:?}");
    }

    /// #2911-e: the boundary that keeps #2911-d honest — a GENERATIVE
    /// named constructor of an enum is not publicly accessible, so an
    /// unused one among several declared constructors still reports
    /// (`_visitConstructorElement`'s more-than-one gate holds).
    #[test]
    fn enum_generative_named_constructor_still_reports() {
        let source = "enum E {\n\
                      \x20 v;\n\
                      \x20 const E();\n\
                      \x20 const E.named();\n\
                      }\n";
        let offset = source.find("named").unwrap() as u32;
        assert_eq!(
            unused_element_rows(source),
            vec![(
                offset,
                "The declaration 'E.named' isn't referenced.".to_string()
            )],
            "{source:?}"
        );
    }

    /// #2911-f: the over-suppression regression guard — a genuinely unused
    /// PRIVATE named declaration still reports after the recovery gates.
    /// `_C` is kept alive by the public function's signature; `_dead` has
    /// no reference and reports exactly as before.
    #[test]
    fn real_unused_named_member_still_reports() {
        let source = "class _C {\n\
                      \x20 void _dead() {}\n\
                      }\n\
                      void f(_C c) {}\n";
        let offset = source.find("_dead").unwrap() as u32;
        assert_eq!(
            unused_element_rows(source),
            vec![(
                offset,
                "The declaration '_dead' isn't referenced.".to_string()
            )],
            "{source:?}"
        );
    }

    /// Element-level member credit (`member_refs` edge): a resolved member
    /// access on `B._m` spares `B._m` alone — `A._m` still reports. The
    /// name-level `element_used` pool would spare both.
    #[test]
    fn same_name_member_of_sibling_class_does_not_share_credit() {
        let source = "class _A {\n\
                      \x20 void _m() {}\n\
                      }\n\
                      class _B {\n\
                      \x20 void _m() {}\n\
                      \x20 void use() { _m(); }\n\
                      }\n\
                      void f(_B b) { b.use(); }\n";
        let a_offset = source.find("class _A").unwrap() as u32;
        let m_offset = source.find("_m").unwrap() as u32;
        // `_A` reports (unused class), `_A._m` reports (never called —
        // `_B._m`'s call is element-scoped). `_B`, `_B._m`, `_B.use` are
        // all used through `f`'s access chain.
        let rows = unused_element_rows(source);
        assert!(
            rows.iter().any(|(o, _)| *o == m_offset),
            "expected _A._m unused_element at {m_offset}: {rows:?} in {source:?}"
        );
    }

    /// Element-level ctor credit (`member_refs` edge via
    /// `credit_constructor_selector`): a resolved `D._named()` creation
    /// spares `D._named` alone — `C._named` still reports.
    #[test]
    fn same_name_ctor_of_sibling_class_does_not_share_credit() {
        let source = "class _C {\n\
                      \x20 _C._named();\n\
                      \x20 _C._other();\n\
                      }\n\
                      class _D {\n\
                      \x20 _D._named();\n\
                      \x20 _D._other();\n\
                      }\n\
                      void f() {\n\
                      \x20 _D._named();\n\
                      \x20 _D._other();\n\
                      }\n";
        let c_named = source.find("_C._named").unwrap() as u32 + 3;
        // `_C._named` reports (never called — `_D._named()`'s creation is
        // element-scoped). `_C` itself reports as an unused class too.
        let rows = unused_element_rows(source);
        assert!(
            rows.iter().any(|(_, text)| text.contains("_C._named")),
            "expected _C._named unused_element: {rows:?} in {source:?}"
        );
    }

    /// Object-pattern getter read credit: `Box(:var e)` reads the `e`
    /// getter on `Box`'s instance face — the getter's element is used
    /// AND read. A private field matching the getter's spelling is spared
    /// by `member_was_read`'s owner join.
    #[test]
    fn object_pattern_field_credits_getter_read() {
        let source = "class _Box {\n\
                      \x20 int? e;\n\
                      }\n\
                      void t(_Box b) {\n\
                      \x20 if (b case _Box(:var e)) { print(e); }\n\
                      }\n";
        let field_offset = source.find("int? e").unwrap() as u32 + 5;
        // `_Box` is used via `t`'s parameter. `e` is read by the object
        // pattern — `visitPatternField` credits `addMember` + `addReadMember`
        // (unused_local_elements_verifier.dart:673-678), so `unused_field`
        // stays silent.
        assert!(
            !report(source)
                .diagnostics
                .iter()
                .any(|d| d.code.name == "unused_field" && d.range.offset == field_offset),
            "unused_field reported on e: {source:?}"
        );
    }

    /// `unused_import` element attribution: `one.A` resolves through the
    /// `one`-prefixed edge alone — the sibling unprefixed `import 'a0.dart'`
    /// reports unused (upstream `PrefixScope.lookup` records the element
    /// on the prefixed import, never the bare edge).
    #[test]
    fn prefixed_use_never_credits_unprefixed_sibling() {
        let source = "import 'a0.dart';\n\
                      import 'a0.dart' as one;\n\
                      one.A a = one.A();\n";
        let r = report_with_imports(
            source,
            &[
                Aux {
                    prefix: None,
                    deferred: false,
                    body: "class A {}\n",
                },
                Aux {
                    prefix: Some("one"),
                    deferred: false,
                    body: "class A {}\n",
                },
            ],
        );
        let unused_offset = source.find("'a0.dart'").unwrap() as u32;
        assert_eq!(import_rows(&r), vec![(unused_offset, 9)], "{source:?}");
    }

    /// `unused_import` element attribution: `A` resolves through `a0.dart`
    /// alone — `a1.dart` contributes no accessed element, so its edge
    /// reports (the `bare_decls` file→library join keeps the resolution
    /// path's attribution rather than the spelling's export-scope match).
    #[test]
    fn unaccessed_target_reports_unused_import() {
        let source = "import 'a0.dart';\n\
                      import 'a1.dart';\n\
                      A? a;\n";
        let r = report_with_imports(
            source,
            &[
                Aux {
                    prefix: None,
                    deferred: false,
                    body: "class A {}\n",
                },
                Aux {
                    prefix: None,
                    deferred: false,
                    body: "class B {}\n",
                },
            ],
        );
        let a1_offset = source.find("'a1.dart'").unwrap() as u32;
        assert_eq!(import_rows(&r), vec![(a1_offset, 9)], "{source:?}");
    }

    // —— #2977 pins: the primary-constructor parameter faces ————————

    /// `note_parameter`'s site fallback: a primary-constructor header arms
    /// `open_ctor_site` (never `open_exec_site` — no method-declaration
    /// name event runs), so its formals must join the site under the ctor
    /// lane. `class _A([this.f])` reports `unused_element_parameter` at the
    /// parameter name — not the site's `(` anchor the empty `site_params`
    /// window fell back to (oracle: `test_classPrivate_primaryConstructor_
    /// unnamed_fieldFormal_optionalPositional_notUsed`, 2:23).
    #[test]
    fn primary_ctor_field_formal_reports_at_the_parameter_name() {
        let source = "class _A([this.f]) {\n\
                      \x20 final int? f;\n\
                      }\n\
                      f() => _A();\n";
        let f_at = source.find("this.f").unwrap() as u32 + 5;
        assert_eq!(
            rows_with_code(source, "unused_element_parameter"),
            vec![(f_at, 1)],
            "{source:?}"
        );
    }

    /// The unnamed primary's named formal: the same site-join through
    /// `open_ctor_site` supplies both the anchor and the (unused) verdict.
    #[test]
    fn primary_ctor_named_formal_reports_at_the_parameter_name() {
        let source = "class _A({int? a}) {}\n\
                      f() => _A();\n";
        let a_at = source.find("int? a").unwrap() as u32 + 5;
        assert_eq!(
            rows_with_code(source, "unused_element_parameter"),
            vec![(a_at, 1)],
            "{source:?}"
        );
    }

    /// A named primary constructor's site keeps the container spelling —
    /// its `ctor_formals` key is the `.` selector the
    /// `primary_ctor_selector` arm derives. `class _A._n({this.f})` joins
    /// its table and reports at `f` (oracle: `test_classPrivate_
    /// primaryConstructor_namedPrivate_fieldFormal_optionalNamed_notUsed`).
    #[test]
    fn named_primary_ctor_reports_against_the_selector_key() {
        let source = "class _A._n({this.f}) {\n\
                      \x20 final int? f;\n\
                      }\n\
                      f() => _A._n();\n";
        let f_at = source.find("this.f").unwrap() as u32 + 5;
        assert_eq!(
            rows_with_code(source, "unused_element_parameter"),
            vec![(f_at, 1)],
            "{source:?}"
        );
    }

    /// A private secondary constructor's `super.a` formal associates
    /// against the super constructor's OPTIONAL `a` — no required
    /// corresponding parameter spares it, so it reports (oracle:
    /// `test_classPrivate_secondaryConstructor_unnamed_superFormal_
    /// optionalNamed_notUsed`, 3:24).
    #[test]
    fn secondary_ctor_super_formal_still_reports() {
        let source = "class _A {\n\
                      \x20 _A({int? a});\n\
                      }\n\
                      \n\
                      class _B extends _A {\n\
                      \x20 _B({super.a});\n\
                      }\n\
                      \n\
                      var b = _B();\n";
        let a_at = source.find("super.a").unwrap() as u32 + 6;
        assert_eq!(
            rows_with_code(source, "unused_element_parameter"),
            vec![(a_at, 1)],
            "{source:?}"
        );
    }

    /// The used-direction twin: a `this.f` primary formal the creation
    /// supplies stays silent — `_A(f: 1)` keys `named_given` on the
    /// container spelling, the unnamed face's own key.
    #[test]
    fn primary_ctor_field_formal_supplied_at_call_stays_silent() {
        let source = "class _A({this.f}) {\n\
                      \x20 final int? f;\n\
                      }\n\
                      f() => _A(f: 1);\n";
        assert_eq!(
            rows_with_code(source, "unused_element_parameter"),
            vec![],
            "{source:?}"
        );
    }

    /// A primary formal read inside the `this : ...` body binds the
    /// parameter — `site_param_uses` credits through the same
    /// `open_ctor_site` join, so `x`'s read in the `assert` spares it.
    /// The formal must be OPTIONAL to reach the verdict at all
    /// (`_isUsedElement`'s `!isOptional` arm spares required positionals
    /// unconditionally).
    #[test]
    fn primary_ctor_body_read_spares_the_formal() {
        let source = "class _A([int? x]) {\n\
                      \x20 this : assert((x ?? 0) > 0);\n\
                      }\n\
                      f() => _A();\n";
        assert_eq!(
            rows_with_code(source, "unused_element_parameter"),
            vec![],
            "{source:?}"
        );
    }

    /// The `.new` primary head is the UNNAMED face — its formals key on
    /// the class name (the `note_constructor_continuation` rule applied to
    /// `end_primary_constructor`), so `class _A.new({int? a})` reports at
    /// `a` and `_A()` creations still resolve the face.
    #[test]
    fn new_primary_head_keeps_the_unnamed_formal_key() {
        let source = "class _A.new({int? a}) {}\n\
                      f() => _A();\n";
        let a_at = source.find("int? a").unwrap() as u32 + 5;
        assert_eq!(
            rows_with_code(source, "unused_element_parameter"),
            vec![(a_at, 1)],
            "{source:?}"
        );
    }

    /// The ordinary lanes keep their shape: a required positional formal
    /// is never reported upstream (`_isUsedElement`'s `!isOptional` arm),
    /// and a private method's body-read optional formal stays silent.
    #[test]
    fn required_and_body_read_formals_stay_silent() {
        let source = "void _f(int x, {int? y}) {\n\
                      \x20 print(y);\n\
                      }\n";
        assert_eq!(
            rows_with_code(source, "unused_element_parameter"),
            vec![],
            "{source:?}"
        );
    }
}

/// The #3152 `unused_local_variable` family pins (the annotation-mention
/// read credit, the same-scope-duplicate binding order, the
/// pattern-assignment declaration, the same-line ignore's code match, and
/// the switch-expression arm binder window), kept as a standalone module
/// so the shared `pins` rig stays untouched while its lane is in flight.
#[cfg(test)]
#[path = "ticket_3152_tests.rs"]
mod ticket_3152_tests;

/// The #3159 `class_used_as_mixin` pins (the with-clause head-name
/// resolution — `p.C` prefixed, `A<int>` parameterized, `A?` nullable, and
/// the modifier exemptions), kept as a standalone module so the shared
/// `pins` rig stays untouched while its lane is in flight.
#[cfg(test)]
#[path = "ticket_3159_tests.rs"]
mod ticket_3159_tests;

/// The #2482 `unused_result` by-resolution pins (the discarded-value face
/// against a same-spelled formal/field/local, and the inherited-object-member
/// keep-honest twin), kept as a standalone module so the shared `pins` rig
/// stays untouched.
#[cfg(test)]
#[path = "ticket_2482_tests.rs"]
mod ticket_2482_tests;

/// The #3044 `unused_element_parameter` constructor-formals pins — the
/// positional / named / field-formal (`this.x`) / super-param (`super.x`)
/// matrix against used-via-call, used-in-body, and unused shapes — kept as
/// a standalone module so the shared `pins` rig stays untouched.
#[cfg(test)]
#[path = "ticket_3044_tests.rs"]
mod ticket_3044_tests;

/// The #3636 nameless-member anchor pins — the `<unnamed>` field/method
/// remap onto the enclosing container's declared-name token at zero
/// length, the two-member same-site fold to one row, and the named-member
/// keep-honest twins — kept as a standalone module so the shared `pins`
/// rig stays untouched.
#[cfg(test)]
#[path = "ticket_3636_tests.rs"]
mod ticket_3636_tests;

/// The #3643 `unused_import` oracle-parity pins — the per-file import-run
/// edge alignment, the per-file combined-scope `bare_decls` lookup, the
/// unresolved-URI `''` staging face, and the literal-receiver extension
/// application credit — kept as a standalone module so the shared `pins`
/// rig stays untouched.
#[cfg(test)]
#[path = "ticket_3643_tests.rs"]
mod ticket_3643_tests;
