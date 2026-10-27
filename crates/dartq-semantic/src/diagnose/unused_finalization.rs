use super::*;

/// The diagnostic codes whose presence anywhere in the unit skips the
/// whole import-warning pass (upstream Dart SDK source,
/// library_analyzer.dart `_hasDiagnosticReportedThatPreventsImportWarnings`
/// — the exact set, machine spellings lowered to the engine's code names).
const IMPORT_WARNING_GATE: &[&str] = &[
    "ambiguous_import",
    "const_with_non_type",
    "extends_non_class",
    "implements_non_class",
    "mixin_of_non_class",
    "new_with_non_type",
    "not_a_type",
    "prefix_identifier_not_followed_by_dot",
    "undefined_class",
    "undefined_function",
    "undefined_identifier",
    "undefined_prefixed_name",
    "deprecated_export_use",
];

impl<'a> Engine<'a> {
    /// The with-clause entry face (`class_used_as_mixin`, messages.yaml
    /// :2737-2741): every entry span whose name resolves to a plain class —
    /// neither `mixin` nor `mixin class` — reports at the entry name. Runs
    /// after the parse so a same-file later declaration resolves; an
    /// unresolvable entry stays with the clause ledger's own faces.
    pub(crate) fn finalize_mixin_uses(&mut self) {
        for span in std::mem::take(&mut self.pending_mixin_uses) {
            // The span is UTF-16; the slice reads source bytes (#2594).
            let start = self.byte_of(span.offset);
            let end = self.byte_of(span.end()).min(self.source.len());
            if start >= end {
                continue;
            }
            let name = self.intern(&self.source[start..end]);
            let Some((container, _)) = resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                name,
            ) else {
                continue;
            };
            if container.kind != ContainerKind::Class
                || container_modifiers(container).is_mixin_class
            {
                continue;
            }
            let name_text = self.strings.resolve(name).to_string();
            self.emit(SemanticCode::ClassUsedAsMixin, span, &name_text, "");
        }
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
    /// written name, or the shorthand's variable). Found members of any
    /// kind (a method field reads its tear-off), `Object` members, and an
    /// applicable extension face resolve; a broken chain, an unresolvable
    /// or core head, and mixin/extension/extension-type heads (their
    /// `on`/representation faces are invisible here) stay silent.
    pub(crate) fn finalize_pattern_getters(&mut self) {
        for (head, getter, at) in std::mem::take(&mut self.pending_pattern_getters) {
            let kind = match self.container_kind_of(head) {
                Some(kind @ (ContainerKind::Class | ContainerKind::Enum)) => kind,
                _ => continue,
            };
            if !matches!(self.walk_members(head, getter), Walk::Missing)
                || getter == self.intern("==")
                || self.members.object_members.contains(&getter)
                || (kind == ContainerKind::Enum
                    && self.members.enum_synth_members.contains(&getter))
                || !matches!(
                    self.extension_application(head, Ty::UNKNOWN, getter, false, false),
                    ExtensionApp::None
                )
            {
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

    pub(crate) fn finalize_bounds(&mut self) {
        let strings = self.strings;
        for check in std::mem::take(&mut self.pending_bounded_checks) {
            match check {
                PendingBoundedCheck::Site {
                    head,
                    args,
                    in_scope,
                    site,
                } => {
                    let issues = self.bounds.check_site(strings, head, &args, &in_scope);
                    for issue in issues {
                        // The inverted-type context row rides every
                        // super-bounded-path report; the extension-type face
                        // carries none (the raw-type row keys on absent
                        // written arguments, which this capture's sites
                        // always have).
                        match issue.inverted_type {
                            Some(inverted) => self.emit3_with_context(
                                SemanticCode::TypeArgumentNotMatchingBounds,
                                issue.range,
                                &issue.argument,
                                &issue.parameter,
                                &issue.bound,
                                vec![ContextMessage {
                                    message: format!(
                                        "The inverted type '{inverted}' is also not regular-bounded, \
                                         so the type is not well-bounded."
                                    )
                                    .into(),
                                    file: self.file,
                                    range: site,
                                }],
                            ),
                            None => self.emit3(
                                SemanticCode::TypeArgumentNotMatchingBounds,
                                issue.range,
                                &issue.argument,
                                &issue.parameter,
                                &issue.bound,
                            ),
                        }
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
                            if foreign {
                                
                            }
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
        let Some((container, _)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            head,
        ) else {
            return;
        };
        if container.constructors.contains(&selector) {
            self.used_names.insert(selector);
        }
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

    fn credit_derived_extension_uses(&mut self) {
        let Some(elements) = self.index.get(self.file) else {
            return;
        };
        let faces = elements.extension_faces();
        if faces.is_empty() {
            return;
        }
        // Self-use arm: the same-file faces keyed by the container spelling
        // the resolution rows record.
        let empty = self.intern("");
        let mut face_keys: FxHashMap<StringId, Vec<u32>> = FxHashMap::default();
        for (index, face) in faces.iter().enumerate() {
            if let Ok(index) = u32::try_from(index) {
                face_keys.entry(face.name.unwrap_or(empty)).or_default().push(index);
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
            .chain(faces.iter().flat_map(|f| f.type_param_names.iter().copied()))
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
        // The enclosing top-level extension's `on` head, read backward from
        // the reference: an implicit-`this` read inside an EXTENSION body
        // looks up on that extension's own `on` type, so only a face on the
        // same head resolves it (a sibling face on another type never
        // does). `None` = not inside an extension body (a class/mixin/enum
        // body — a subclass of the face's `on` type — or top level).
        let enclosing_extension_on = |offset: u32| -> Option<&str> {
            let bytes = self.source.as_bytes();
            let mut end = self.byte_of(offset).min(bytes.len());
            loop {
                let start = bytes[..end].iter().rposition(|b| *b == b'\n').map_or(0, |p| p + 1);
                let line = &self.source[start..end];
                // A top-level declaration header starts in column 0.
                if !line.is_empty() && !line.starts_with([' ', '\t', '}', '/', '*', ')']) {
                    let words: Vec<&str> = line.split_whitespace().collect();
                    if words.contains(&"extension") {
                        let head = words
                            .iter()
                            .position(|w| *w == "on")
                            .and_then(|at| words.get(at + 1))
                            .map(|w| w.trim_end_matches(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$')))
                            .map(|w| w.split(['<', '?']).next().unwrap_or(w));
                        return Some(head.unwrap_or(""));
                    }
                    if words.iter().any(|w| matches!(*w, "class" | "mixin" | "enum")) {
                        return None;
                    }
                }
                if start == 0 {
                    return None;
                }
                end = start - 1;
            }
        };
        let in_extension_body = |offset: u32, index: u32| -> bool {
            let Some(head) = enclosing_extension_on(offset) else {
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
                Some(face_head) if face_head != head => {
                    let sub = Ty::named(self.intern(head));
                    let sup = Ty::named(self.intern(face_head));
                    self.type_resolution(self.intern(head)).0 != TypeResolution::Missing
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
                
                derived.insert((self.file, index, unit_ref.name));
            }
        }
        for read in &self.member_reads {
            if let Receiver::Typed { type_name, .. } = read.recv
                && type_param_heads.contains(&type_name)
                && let Some(index) = unique_face(read.name)
            {
                
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
                let span_bytes =
                    &self.source.as_bytes()[self.byte_of(start)..self.byte_of(end)];
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
            let Some(&(face, face_file)) =
                faces.iter().find(|(face, _)| face.members.contains_key(&read.name))
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
                self.extension_member_used
                    .insert((face_file, face_index, read.name));
                self.credit_extension_import(face, face_file);
            }
        }
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
        // The element-attributed use pool: `used_names` minus the member
        // spellings that only an unresolved access contributed (the
        // `Receiver::Unknown`/`Prefix` rows — an `InvalidType` or `dynamic`
        // receiver's `.m` resolves no element, so the oracle's
        // `members`/`elements` never carry it). A member spelling that is
        // also a non-member resolved use (a bare `RefTarget::Decl`/`Member`
        // reference, a type-use position) stays — it resolves through a
        // path that is not the member access.
        let member_accessed: FxHashSet<StringId> =
            self.member_reads.iter().map(|read| read.name).collect();
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
            .copied()
            .collect();
        for site_index in 0..self.decl_sites.len() {
            let site = self.decl_sites[site_index].clone();
            let bare = self.bare_name(site.name);
            // The nameless recovery face renders `<unnamed>` (`ElementImpl
            // .displayName`'s `name ?? '<unnamed>'`).
            let raw_text = self.strings.resolve(site.name);
            let name_text = if raw_text.is_empty() {
                "<unnamed>".to_string()
            } else {
                raw_text.to_string()
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
            match site.kind {
                DeclKind::TopLevelFn | DeclKind::TopLevelVar | DeclKind::TypeAlias => {
                    // `_isUsedElement`'s else arm: a public name is used;
                    // otherwise any indexed use spares the declaration.
                    if !site.annotated
                        && (self.is_private_name(site.name) || self.is_nameless(site.name))
                        && !element_used.contains(&bare)
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
                        
                        self.stage_usage(SemanticCode::UnusedElement, range, &name_text);
                    }
                }
                DeclKind::Container => {
                    // `_visitClassElement` (classes, enums, mixins, and
                    // extension types; extensions are never visited): the
                    // same public/used gate.
                    if !site.annotated
                        && (self.is_private_name(site.name) || self.is_nameless(site.name))
                        && !element_used.contains(&bare)
                    {
                        
                        self.stage_usage(SemanticCode::UnusedElement, site.range, &name_text);
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
                    let eligible = !site.annotated
                        && (self.is_private_name(site.name)
                            || (self.is_private_name(container) && is_static));
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
                    let unresolved_read = self.read_names.contains(&bare)
                        && self
                            .member_read_owners
                            .get(&bare)
                            .is_none_or(|owners| owners.is_empty());
                    if self.member_was_read(container, &bare) || enum_values_read || unresolved_read
                    {
                        continue;
                    }
                    self.stage_usage(SemanticCode::UnusedField, site.range, &name_text);
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
                    // An unnamed extension's members never reach this arm's
                    // report path — the face loop below owns them (its
                    // nameless faces carry no `containers` entry, so the
                    // accessibility test's container-miss default spares
                    // here; and the oracle agrees the whole face is
                    // private: `ExtensionElement.isPrivate` answers
                    // `name == null || name.isPrivateName`, element.dart:
                    // 3264-3267, upstream Dart SDK source, so its members
                    // report exactly once, from the face loop).
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
                            .map(|(index, _)| index)
                    });
                    let member_used = match extension_face {
                        Some(index) if !is_static => u32::try_from(index).is_ok_and(|index| {
                            self.extension_member_used
                                .contains(&(self.file, index, site.name))
                                || self
                                    .extension_member_used
                                    .contains(&(self.file, index, bare))
                        }),
                        _ => element_used.contains(&bare),
                    };
                    let spared = site.annotated
                        || (!self.is_nameless(site.name)
                            && self.is_publicly_accessible(site.name, site.kind))
                        || member_used
                        || matches!(self.walk_super(container, site.name), Walk::Found(_));
                    if !spared {
                        
                        self.stage_usage(SemanticCode::UnusedElement, site.range, &name_text);
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
                        // declared constructor, named and unnamed alike; the
                        // written constructor sites carry exactly that set
                        // (one site per head, the selector's rewrite updating
                        // the provisional in place), so the declaration walk
                        // counts them rather than reading the element model's
                        // flags.
                        let multi = self
                            .decl_sites
                            .iter()
                            .filter(|s| {
                                matches!(
                                    s.kind,
                                    DeclKind::Constructor { container: c } if c == container
                                )
                            })
                            .count()
                            > 1;
                        let spared = !multi
                            || site.annotated
                            || (!self.is_nameless(site.name)
                                && self.is_publicly_accessible(site.name, site.kind))
                            || element_used.contains(&bare)
                            || matches!(self.walk_super(container, site.name), Walk::Found(_));
                        if !spared {
                            // The element's displayName qualifies the selector
                            // with its container (`E.foo`); the site stores the
                            // selector spelling only. A provisional head never
                            // rewritten stays at the container spelling — the
                            // unnamed face is never visited (named-only gate).
                            let container_text = self.strings.resolve(container);
                            let display = format!("{container_text}.{name_text}");
                            self.stage_usage(SemanticCode::UnusedElement, site.range, &display);
                        }
                    }
                }
            }
            self.report_optional_parameters(site_index, &site);
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
        // Membership split with `decl_sites`: an unnamed extension
        // declares no sites at all, so its whole member set checks here; a
        // named extension's members are declaration sites — its
        // methods/getters/setters ride the `DeclKind::Member` arm above
        // (the `extension_member_used` credit substituted for the
        // spelling arm there), and its STATIC fields the `DeclKind::
        // Member` field arm — leaving this face the named-private
        // instance fields the site arms are ineligible for.
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
                        !entry.annotations.is_empty(),
                    ));
                }
            }
        }
        for (face_index, spelling, range, kind, is_static, annotated) in extension_members {
            if range == MemberEntry::MISSING_NAME_RANGE || face_index == u32::MAX {
                continue;
            }
            let bare = self.bare_name(spelling);
            let spared = annotated
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
                self.stage_usage(code, range, &name_text);
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
        self.suppress_same_line_ignored_usage();
        self.report_unused_imports();
        self.report_deprecated_sdk_libraries();
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
    /// A `//` head counts only after whitespace (or at line start), so a
    /// `//` inside a string literal never reads as a comment.
    fn suppress_same_line_ignored_usage(&mut self) {
        if !self.source.contains("ignore:") {
            return;
        }
        let local = SemanticCode::UnusedLocalVariable.code_info();
        let field = SemanticCode::UnusedField.code_info();
        let mut drop: Vec<usize> = Vec::new();
        for (index, row) in self.report.usage.iter().enumerate() {
            if row.code != local && row.code != field {
                continue;
            }
            let byte = self.byte_of(row.range.offset).min(self.source.len());
            let start = self.source[..byte].rfind('\n').map_or(0, |at| at + 1);
            let end = self.source[start..]
                .find('\n')
                .map_or(self.source.len(), |at| start + at);
            if self.line_has_trailing_ignore(&self.source[start..end]) {
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

    /// Whether the line carries a `// ignore:` entry naming one of the
    /// suppressed codes with code before the comment (the trailing
    /// form — a standalone comment's entry is the next line's, the root
    /// face's). A head only counts after whitespace (or at line start) —
    /// a `//` inside a string literal is not a comment — and the name
    /// grammar mirrors the root parser's: comma-list, alphanumeric/
    /// underscore heads, case-insensitive.
    fn line_has_trailing_ignore(&self, line: &str) -> bool {
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
                let head = &name[..end];
                head.eq_ignore_ascii_case("unused_local_variable")
                    || head.eq_ignore_ascii_case("unused_field")
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
    /// is accessible exactly when the extension is; a constructor of an
    /// enum never is.
    pub(crate) fn is_publicly_accessible(&self, name: StringId, kind: DeclKind) -> bool {
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
            ContainerKind::Enum => !(is_ctor || (self.is_private_name(container) && is_static)),
            ContainerKind::Extension => !self.is_private_name(container),
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

    /// The `unused_element_parameter` pass over one executable's
    /// declaration site (`visitFormalParameterList` +
    /// `_isUsedElement`'s parameter arm,
    /// unused_local_elements_verifier.dart:1064-1115): optional parameters
    /// of constructors, methods, and top-level functions that no body use
    /// resolves to and no invocation supplies. The official exemptions in
    /// order: a required parameter, a constructor of a generic class, a
    /// generic executable, a publicly accessible executable, an executable
    /// overriding a super-name, and the super-parameter face (unmodelable —
    /// suppressed).
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    pub(crate) fn report_optional_parameters(&mut self, site_index: usize, site: &DeclSite) {
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
        let formals = match container {
            Some(c) => self
                .index
                .get(self.file)
                .and_then(|f| f.container(c))
                .and_then(|x| {
                    if matches!(site.kind, DeclKind::Constructor { .. }) {
                        x.ctor_formals.get(&site.name).map(Box::as_ref)
                    } else {
                        x.members.get(&site.name).and_then(|m| m.formals.as_deref())
                    }
                }),
            None => self
                .index
                .get(self.file)
                .and_then(|f| f.decl(site.name))
                .and_then(|d| d.formals.as_deref()),
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
        // The accessibility and override gates.
        if self.is_publicly_accessible(site.name, site.kind) {
            return;
        }
        if let Some(c) = container
            && matches!(self.walk_super(c, site.name), Walk::Found(_))
        {
            return;
        }
        // A redirecting factory's target receives every parameter the
        // factory forwards — its optional formals are all supplied.
        if let (DeclKind::Constructor { container }, Some(_)) = (site.kind, container) {
            let unnamed = site.name == container
                || self.is_bare_new_head(site.name)
                || site.name == self.intern("new");
            if self
                .factory_redirect_targets
                .contains(&(container, site.name))
                || (unnamed
                    && self
                        .factory_redirect_targets
                        .contains(&(container, container)))
            {
                return;
            }
        }
        let bare = self.bare_name(site.name);
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
        // The implicit prelude edge is exactly one leading entry (the merge
        // inserts it before every written edge), targets `dart:core`, and
        // carries no prefix, no deferral, and no combinator clauses. A
        // written `import 'dart:core';` is payload-identical but sits in
        // written order behind it; the one-extra count keeps the skip
        // honest, and the harness graph (no prelude) aligns unchanged.
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
        if edges.len() != self.imports.len() {
            return;
        }
        let used: Vec<StringId> = self
            .used_names
            .iter()
            .chain(&self.import_used)
            .copied()
            .collect();
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
                continue;
            };
            if self.graph.uri(target) == "dart:core" {
                continue;
            }
            let is_used = match prefix {
                Some(prefix) => {
                    self.used_names.contains(&prefix) || self.import_used.contains(&prefix)
                }
                None => {
                    let hit = used.iter().find(|&&n| {
                        edge.combinators.allows(n) && self.graph.exported(target, n).is_some()
                    });
                    if let Some(n) = hit {
                        
                    }
                    hit.is_some() || self.import_credits_extension_member(edge, target, &used)
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
            for face in elements.extension_faces() {
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
            }
        }
        false
    }

    /// Moves the pass's accumulated use sets into the unit's use-face
    pub(crate) fn take_use_face(&mut self) -> UnitUseFace {
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
        }
    }

    /// The `_`-wildcard test (`_wildCardVariablesEnabled` +
    /// `_isNamedWildcard`, unused_local_elements_verifier.dart:690-698,
    /// 986-995): with the wildcard-variables feature (`enabledIn: 3.7.0`,
    /// `tools/experimental_features.yaml`) only the exact name `_` is a
    /// wildcard — `__`/`___` report; the all-underscores fallback is the
    /// feature-off branch, live only below 3.7. dartq's supported universe
    /// runs the modern posture (the same modern-library default
    /// [`Self::library_facts`] documents for class-modifiers): a pre-3.7
    /// `// @dart=` library under-reports its `__`-shaped bindings — a
    /// documented divergence, unreachable in the oracle corpus.
    pub(crate) const WILDCARD_VARIABLES_ENABLED: bool = true;

    pub(crate) fn is_wildcard(&self, name: StringId) -> bool {
        let text = self.strings.resolve(name);
        if Self::WILDCARD_VARIABLES_ENABLED {
            text == "_"
        } else {
            !text.is_empty() && text.bytes().all(|b| b == b'_')
        }
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
    pub(crate) fn flush_null_aware_staged(&mut self, promotions: &PromotionStaging) {
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
        let asserts = std::mem::take(&mut self.null_assert_staged);
        for (read, bang, nullable_params) in asserts {
            let promoted = promotions.promoted(read).filter(|ty| {
                !matches!(ty, Ty::Named { name, .. } if nullable_params.contains(name))
            });
            
            if promoted.is_some_and(Ty::is_strictly_non_nullable) {
                self.stage_usage(SemanticCode::UnnecessaryNonNullAssertion, bang, "");
            }
        }
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
        });
        self.decl_sites.len() - 1
    }

    /// Whether one of the pending annotations carries the spelling (the
    /// `@override`-family gates read the latch before any site drains it).
    pub(crate) fn pending_annotation_is(&self, spelling: &str) -> bool {
        self.pending_annotations
            .iter()
            .any(|a| self.strings.resolve(a.name) == spelling)
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
    /// chain-breaks, under-reporting).
    pub(crate) fn ur_member_site(&self, container: StringId, member: StringId) -> Option<usize> {
        let mut current = Some(container);
        while let Some(c) = current {
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

#[cfg(test)]
mod pins {
    use super::super::{FileReport, StrictModes, diagnose_with};
    use crate::collector::collect;
    use crate::library::ElementIndex;
    use crate::model::MicroKind;
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_parser::ParserFeatures;
    use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder, Namespaces};
    use dartq_scanner::ScanConfig;
    use dartq_source::{FileTable, StringTable};
    use std::path::Path;

    /// Diagnoses `source` as the sole file of a library with no imports —
    /// the shared single-file rig.
    fn report(source: &str) -> FileReport {
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
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let mut index = ElementIndex::default();
        index.insert(file, elements);
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

    /// The `unused_local_variable` rows of `report`, as (offset, name-token
    /// length) pairs in source order.
    fn unused_local_rows(source: &str) -> Vec<(u32, u32)> {
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
    /// variable keeps only its own arm's use. No row on either.
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
        assert_eq!(unused_local_rows(source), vec![], "{source:?}");
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
}
