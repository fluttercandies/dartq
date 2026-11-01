//! The enum member-set and mixin-application faces (#3373).
//!
//! Six registry codes had no emit site at all: `illegal_concrete_enum_member`
//! (declaration + inheritance arms), `illegal_enum_values` (the same two
//! arms), `enum_with_abstract_member`, `values_declaration_in_enum`,
//! `mixin_application_not_implemented_interface`, and
//! `mixin_application_no_concrete_super_invoked_member`.
//!
//! # The official decisions, mirrored
//!
//! `_ClassVerifier.verify` (upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/inheritance_override.dart:313-470) walks one
//! class-family declaration at a time:
//!
//! * the member loop (:374-405) checks every declared member:
//!   `_checkIllegalConcreteEnumMemberDeclaration` (:785-800) reports a
//!   CONCRETE instance member named `index`, `hashCode`, or `==` on anything
//!   whose supertypes include `Enum` — fields always, accessors/methods only
//!   when they carry a body, setters never; and (for every non-enum
//!   implementor) `_checkIllegalEnumValuesDeclaration` (:842-846) reports a
//!   non-static member named `values`.
//! * `_checkIllegalConcreteEnumMemberInheritance` (:803-840) then looks the
//!   three names up through the CONCRETE superclass+mixin chain
//!   (`getInheritedConcreteMember`, inheritance_manager3.dart:157-172 — the
//!   `implements` faces never contribute), reporting at the class name with
//!   the declaring class whenever that declaration is not `Object`
//!   (`hashCode`/`==`) or not `Enum` (`index`); mixin declarations are
//!   skipped.
//! * `_checkIllegalEnumValuesInheritance` (:848-866) looks `values` up
//!   through the whole INTERFACE (`getInherited` — `extends`, `with`, and
//!   `implements`), reporting at the class name with the declaring class.
//! * the interface walk (:470-520) reports a bodyless member with no
//!   concrete implementation through `_reportConcreteClassWithAbstractMember`
//!   (:912-946): an enum answers `enum_with_abstract_member` at the WHOLE
//!   member declaration (`.at(member)`).
//!
//! The enum's own `values` member face (`values_declaration_in_enum`) is the
//! `_checkValuesDeclarationInEnum` sibling of
//! member_duplicate_definition_verifier.dart:585-589: an enum body declares
//! a member called `values` — any kind except a static setter (and enum
//! constants, which ride the field arm) — anchored at the member name.
//!
//! The remaining mixin-application face is error_verifier.dart's
//! `_checkForMixinSuperclassConstraints` (:6076-6116).
//!
//! # Where it is wired
//!
//! [`Engine::verify_enum_member_set`] runs as a whole-unit post-pass beside
//! [`Engine::finalize_usage`] (the fold's verifier tail), so every container
//! is closed and every member entry carries its kind, abstractness, and name
//! span. The rows stage into the report's usage tail, which the fold sorts by
//! offset before merging — the transport the `#2910` member-use pass uses.
//!
//! # Documented residuals (the under-report direction)
//!
//! * `mixin_application_no_concrete_super_invoked_member` needs the mixin's
//!   per-NAME super-invocation set (`superInvokedNames`); the engine tracks
//!   only a per-executable boolean (`FrameKind::Executable.super_invoked`),
//!   so the face stays unemitted rather than guessing.
//! * Generic instantiations are decided name-level: a constraint reached
//!   through the supertype closure satisfies, an unreachable one does not.
//!   A constraint the real subtype relation satisfies only through type
//!   arguments therefore stays silent, never the reverse.
//! * A clause face naming a type alias resolves to its right-hand container
//!   ([`Engine::resolve_clause_face`]) but the spelling-keyed walk below
//!   cannot follow it further, so the chain stops.

use super::*;
use std::collections::VecDeque;

/// The missing name-span sentinel of a payload-restored entry (never an
/// anchor).
const MISSING_SPAN: u32 = u32::MAX;

impl<'a> Engine<'a> {
    /// The #3373 whole-unit pass: every container this file declares runs its
    /// enum member-set faces and, when it carries a `with` clause, the
    /// mixin-application face.
    pub(crate) fn verify_enum_member_set(&mut self) {
        // The staged optional-parameter rows await the fold tail: the
        // merged augmentation chain (every `augment` declaration now
        // captured) decides each one's default coverage before it emits.
        self.finalize_default_checks();
        // One run per uniquely-named container: an augmentation's
        // `augment enum E` notes a second container site for the same name,
        // and the faces must report once.
        let mut seen: FxHashSet<StringId> = FxHashSet::default();
        let sites: Vec<(StringId, SourceRange)> = self
            .decl_sites
            .iter()
            .filter(|site| matches!(site.kind, DeclKind::Container))
            .filter(|site| seen.insert(site.name))
            .map(|site| (site.name, site.range))
            .collect();
        for (name, name_range) in sites {
            let index = self.index;
            let Some(container) = index.get(self.file).and_then(|unit| unit.container(name)) else {
                continue;
            };
            if container.kind == ContainerKind::Enum {
                self.verify_enum_declarations(container.kind, container);
                self.verify_enum_inheritance(name, name_range, container);
                self.verify_enum_abstract_members(name, container);
            } else if self.implements_core_enum(name, container) {
                self.verify_enum_declarations(container.kind, container);
                self.verify_enum_inheritance(name, name_range, container);
            }
            self.verify_mixin_application(name, name_range, container);
        }
    }

    /// The member loop's two declaration faces
    /// (inheritance_override.dart:374-405): the illegal concrete enum member
    /// (`index`/`hashCode`/`==`) and the `values` declaration — which is
    /// `illegal_enum_values` for a non-enum implementor and
    /// `values_declaration_in_enum` inside an enum body.
    fn verify_enum_declarations(&mut self, kind: ContainerKind, container: &'a Container) {
        let index_name = self.intern("index");
        let hash_code = self.intern("hashCode");
        let equals = self.intern("==");
        let values = self.intern("values");
        let mut rows: Vec<(SemanticCode, SourceRange, String)> = Vec::new();
        for (member, entry) in container.members.iter() {
            if entry.name_range.offset == MISSING_SPAN {
                continue;
            }
            if (*member == index_name || *member == hash_code || *member == equals)
                && declarable_concrete_member(entry)
            {
                rows.push((
                    SemanticCode::IllegalConcreteEnumMember,
                    entry.name_range,
                    self.member_display(*member),
                ));
            }
            if *member != values {
                continue;
            }
            // The field arm reports every kind; the method arm suppresses
            // only a static setter (:374-405), and an enum body answers the
            // `values_declaration_in_enum` sibling code instead.
            if kind == ContainerKind::Enum {
                if !(entry.is_static && entry.kind == MemberKind::Setter) {
                    rows.push((
                        SemanticCode::ValuesDeclarationInEnum,
                        entry.name_range,
                        String::new(),
                    ));
                }
            } else if !entry.is_static {
                rows.push((
                    SemanticCode::IllegalEnumValues,
                    entry.name_range,
                    String::new(),
                ));
            }
        }
        for (code, range, a0) in rows {
            self.stage_row(code, range, &a0, "");
        }
    }

    /// The two inheritance faces (inheritance_override.dart:803-866).
    fn verify_enum_inheritance(
        &mut self,
        name: StringId,
        name_range: SourceRange,
        container: &'a Container,
    ) {
        // `_checkIllegalConcreteEnumMemberInheritance` ignores mixins (a
        // mixin inherits nothing from its `on` constraints).
        if container.kind == ContainerKind::Mixin {
            return;
        }
        let core_object = self.intern("Object");
        let core_enum = self.intern("Enum");
        for (member, excluded) in [
            (self.intern("hashCode"), core_object),
            (self.intern("=="), core_object),
            (self.intern("index"), core_enum),
        ] {
            let Some((declaring, declaring_kind)) =
                self.inherited_concrete_member(name, container, member)
            else {
                continue;
            };
            // The official filter reports a non-class declaration
            // unconditionally and a class declaration unless it is the
            // excluded core face (:817-831).
            if declaring_kind != ContainerKind::Class || declaring != excluded {
                let a0 = self.member_display(member);
                let a1 = self.display_of(declaring);
                self.stage_row(SemanticCode::IllegalConcreteEnumMember, name_range, &a0, &a1);
            }
        }
        // `_checkIllegalEnumValuesInheritance` reads the merged interface.
        let values = self.intern("values");
        if let Some(declaring) = self.inherited_interface_member(name, container, values) {
            let a0 = self.display_of(declaring);
            self.stage_row(SemanticCode::IllegalEnumValues, name_range, &a0, "");
        }
    }

    /// `_reportConcreteClassWithAbstractMember`'s enum arm: a bodyless member
    /// the enum declares while its interface carries no concrete
    /// implementation of that name reports at the whole declaration
    /// (inheritance_override.dart:912-946). A member written `augment` is a
    /// fragment, not a declaration — upstream skips it in the interface walk
    /// (`interfaceElement.baseElement.isAugmentationWithoutAugmentedDeclaration`,
    /// :431-435), so the face stays silent while the `augmentations` feature
    /// is on (with it off the keyword never lexes and the member reports as
    /// an ordinary abstract declaration).
    fn verify_enum_abstract_members(&mut self, name: StringId, container: &'a Container) {
        let enclosing = self.display_of(name);
        let augment_spans: FxHashSet<u32> = if self.augmentations_enabled {
            self.index
                .get(self.file)
                .map(|unit| {
                    unit.augmentations()
                        .iter()
                        .filter(|aug| {
                            matches!(
                                aug.kind,
                                crate::model::AugKind::Method
                                    | crate::model::AugKind::Getter
                                    | crate::model::AugKind::Setter
                                    | crate::model::AugKind::Field
                            ) && aug.container == Some(name)
                        })
                        .map(|aug| aug.name_range.offset)
                        .collect()
                })
                .unwrap_or_default()
        } else {
            FxHashSet::default()
        };
        let mut rows: Vec<(SourceRange, String)> = Vec::new();
        for (member, entry) in container.members.iter() {
            if entry.name_range.offset == MISSING_SPAN
                || entry.is_static
                || !entry.is_abstract
                || augment_spans.contains(&entry.name_range.offset)
            {
                continue;
            }
            // A field contributes its getter face, and its setter face when
            // the field group is not `final` (:930-940).
            let mut faces: Vec<StringId> = vec![*member];
            if entry.kind == MemberKind::Field && !entry.is_final {
                faces.push(self.intern(&format!("{}=", self.strings.resolve(*member))));
            }
            let unimplemented = faces
                .iter()
                .any(|face| !self.interface_has_concrete(name, container, *face));
            if !unimplemented {
                continue;
            }
            let Some(span) = self.member_declaration_span(entry.name_range) else {
                continue;
            };
            rows.push((span, self.member_display(*member)));
        }
        for (span, display) in rows {
            self.stage_row(
                SemanticCode::EnumWithAbstractMember,
                span,
                &display,
                &enclosing,
            );
        }
    }

    /// `_checkForMixinSuperclassConstraints`: every `with` entry must satisfy
    /// the applied mixin's `on` constraints through the applying class's
    /// supertype or an earlier mixin, reporting at the mixin name
    /// (error_verifier.dart:6076-6116).
    fn verify_mixin_application(
        &mut self,
        name: StringId,
        name_range: SourceRange,
        container: &'a Container,
    ) {
        if container.with_clause.is_empty() {
            return;
        }
        let scope_lib = self.owner_library_of(self.file, name).unwrap_or(self.lib);
        let mut previous: Vec<(StringId, FileId)> = Vec::new();
        for (position, mixin) in container.with_clause.iter().enumerate() {
            let resolved = self.resolve_clause_face(name, self.file, scope_lib, *mixin);
            let owner = resolved.map_or(self.file, |(_, file)| file);
            // `isInterfaceTypeInterface` gates the official loop: only a
            // resolved mixin answers its `on` clause.
            if let Some((mixin_container, _)) = resolved
                && mixin_container.kind == ContainerKind::Mixin
            {
                for constraint in mixin_container.on_clause.iter() {
                    let by_super = container
                        .superclass
                        .is_some_and(|face| self.supertype_reaches(face, self.file, *constraint));
                    let mut satisfied = by_super;
                    for (prior, prior_file) in &previous {
                        if self.supertype_reaches(*prior, *prior_file, *constraint) {
                            satisfied = true;
                            break;
                        }
                    }
                    if satisfied {
                        continue;
                    }
                    let Some(span) = self.with_clause_name_span(name_range, position) else {
                        return;
                    };
                    let a0 = self.display_of(*mixin);
                    let a1 = container
                        .superclass
                        .map_or_else(|| "Object".to_string(), |face| self.display_of(face));
                    let a2 = self.display_of(*constraint);
                    self.stage_row3(
                        SemanticCode::MixinApplicationNotImplementedInterface,
                        span,
                        &a0,
                        &a1,
                        &a2,
                    );
                    return;
                }
            }
            previous.push((*mixin, owner));
        }
    }

    // —— model probes ————————————————————————————————————————————————

    /// Whether `container`'s supertypes include `dart:core`'s `Enum`
    /// (`implementsDartCoreEnum`, inheritance_override.dart:287-289). The
    /// check is spelling-level: `Enum` is a sample-stable name in every
    /// modeled environment (the SDK stub and the test rig both declare it),
    /// and an enum declaration answers true by kind.
    fn implements_core_enum(&self, name: StringId, container: &Container) -> bool {
        if container.kind == ContainerKind::Enum {
            return true;
        }
        let target = self.intern("Enum");
        self.supertype_closure_contains(name, target, container)
    }

    /// The first concrete member named `member` on the superclass+mixin chain
    /// (`getInheritedConcreteMember`): each level's own members, then its
    /// `with` applications later-first, then the superclass. Returns the
    /// declaring container's spelling and kind.
    fn inherited_concrete_member(
        &self,
        name: StringId,
        container: &'a Container,
        member: StringId,
    ) -> Option<(StringId, ContainerKind)> {
        if let Some(hit) = self.mixin_concrete_member(container, self.file, name, member) {
            return Some(hit);
        }
        let mut writer = name;
        let mut file = self.file;
        let mut current = container;
        let mut seen: FxHashSet<StringId> = FxHashSet::default();
        seen.insert(name);
        loop {
            let superclass = current.superclass?;
            if !seen.insert(superclass) {
                return None;
            }
            let scope_lib = self.owner_library_of(file, writer).unwrap_or(self.lib);
            let (next, owner) = self.resolve_clause_face(writer, file, scope_lib, superclass)?;
            if let Some(entry) = next.members.get(&member)
                && concrete_member(entry)
            {
                return Some((superclass, next.kind));
            }
            if let Some(hit) = self.mixin_concrete_member(next, owner, superclass, member) {
                return Some(hit);
            }
            writer = superclass;
            current = next;
            file = owner;
        }
    }

    /// The later-first `with` scan of one application level.
    fn mixin_concrete_member(
        &self,
        container: &'a Container,
        file: FileId,
        name: StringId,
        member: StringId,
    ) -> Option<(StringId, ContainerKind)> {
        if container.with_clause.is_empty() {
            return None;
        }
        let scope_lib = self.owner_library_of(file, name).unwrap_or(self.lib);
        for mixin in container.with_clause.iter().rev() {
            let Some((resolved, _)) = self.resolve_clause_face(name, file, scope_lib, *mixin)
            else {
                continue;
            };
            if let Some(entry) = resolved.members.get(&member)
                && concrete_member(entry)
            {
                return Some((*mixin, resolved.kind));
            }
        }
        None
    }

    /// The first non-static member named `member` anywhere on the interface
    /// (`getInherited`): breadth-first over `extends`/`with`/`implements`
    /// (and a mixin's `on` constraints).
    fn inherited_interface_member(
        &self,
        name: StringId,
        container: &'a Container,
        member: StringId,
    ) -> Option<StringId> {
        self.interface_member(name, container, member, |entry| !entry.is_static)
    }

    /// Whether any container on the interface declares `member` concretely.
    fn interface_has_concrete(
        &self,
        name: StringId,
        container: &'a Container,
        member: StringId,
    ) -> bool {
        self.interface_member(name, container, member, concrete_member)
            .is_some()
    }

    /// The shared breadth-first interface walk: the first supertype whose own
    /// member map answers `accepts`.
    fn interface_member(
        &self,
        name: StringId,
        container: &'a Container,
        member: StringId,
        accepts: impl Fn(&MemberEntry) -> bool,
    ) -> Option<StringId> {
        let mut queue: VecDeque<(StringId, FileId)> =
            self.supertype_faces(name, self.file, container).into();
        let mut seen: FxHashSet<(StringId, FileId)> = FxHashSet::default();
        seen.insert((name, self.file));
        while let Some((face, file)) = queue.pop_front() {
            if !seen.insert((face, file)) {
                continue;
            }
            let index = self.index;
            let Some(found) = index.get(file).and_then(|unit| unit.container(face)) else {
                continue;
            };
            if found.members.get(&member).is_some_and(&accepts) {
                return Some(face);
            }
            queue.extend(self.supertype_faces(face, file, found));
        }
        None
    }

    /// Whether the start container's own supertype closure reaches `target`.
    fn supertype_closure_contains(
        &self,
        name: StringId,
        target: StringId,
        container: &Container,
    ) -> bool {
        let mut queue: VecDeque<(StringId, FileId)> =
            self.supertype_faces(name, self.file, container).into();
        let mut seen: FxHashSet<(StringId, FileId)> = FxHashSet::default();
        seen.insert((name, self.file));
        while let Some((face, file)) = queue.pop_front() {
            if face == target {
                return true;
            }
            if !seen.insert((face, file)) {
                continue;
            }
            let index = self.index;
            if let Some(found) = index.get(file).and_then(|unit| unit.container(face)) {
                queue.extend(self.supertype_faces(face, file, found));
            }
        }
        false
    }

    /// Whether the closure of the container declared as `start` in
    /// `file` reaches `target`.
    fn supertype_reaches(&self, start: StringId, file: FileId, target: StringId) -> bool {
        if start == target {
            return true;
        }
        let index = self.index;
        let Some(container) = index.get(file).and_then(|unit| unit.container(start)) else {
            return false;
        };
        self.supertype_closure_contains(start, target, container)
    }

    /// One container's resolved supertype faces, in written order
    /// (`extends`, `with`, `implements`, and a mixin's `on`).
    fn supertype_faces(
        &self,
        name: StringId,
        file: FileId,
        container: &Container,
    ) -> Vec<(StringId, FileId)> {
        let scope_lib = self.owner_library_of(file, name).unwrap_or(self.lib);
        let on: &[StringId] = if container.kind == ContainerKind::Mixin {
            &container.on_clause
        } else {
            &[]
        };
        let faces = container
            .superclass
            .iter()
            .chain(container.with_clause.iter())
            .chain(container.implements_clause.iter())
            .chain(on.iter());
        let mut resolved = Vec::new();
        for face in faces {
            if let Some((_, owner)) = self.resolve_clause_face(name, file, scope_lib, *face) {
                resolved.push((*face, owner));
            }
        }
        resolved
    }

    // —— spans ——————————————————————————————————————————————————————

    /// The whole span of the member declaration whose name sits at
    /// `name_range`: the walk starts after the previous declaration boundary
    /// and ends at the terminating `;` (a bodyless member's own close).
    /// `None` when the name token or the closer is not in the table (never a
    /// guessed anchor).
    fn member_declaration_span(&self, name_range: SourceRange) -> Option<SourceRange> {
        let tokens = self.tokens.as_slice();
        let at = tokens
            .iter()
            .position(|token| token.utf16_offset == name_range.offset && !token.is_synthetic())?;
        let mut start = at;
        while start > 0 {
            let text = &self.source[tokens[start - 1].byte_range()];
            if text == ";" || text == "{" || text == "}" {
                break;
            }
            start -= 1;
        }
        let mut end = at;
        let mut depth: i32 = 0;
        loop {
            end += 1;
            let token = tokens.get(end)?;
            let text = &self.source[token.byte_range()];
            match text {
                "(" | "[" => depth += 1,
                ")" | "]" => depth -= 1,
                ";" if depth == 0 => break,
                "}" | "{" => return None,
                _ => {}
            }
        }
        self.token_range(tokens, start, end)
    }

    /// The `position`-th `with`-clause type name's token span (the
    /// `_checkForMixinSuperclassConstraints` anchor, `.at(mixinName.name)`).
    fn with_clause_name_span(
        &self,
        name_range: SourceRange,
        position: usize,
    ) -> Option<SourceRange> {
        let tokens = self.tokens.as_slice();
        let mut at = tokens
            .iter()
            .position(|token| token.utf16_offset == name_range.offset && !token.is_synthetic())?;
        let mut depth: i32 = 0;
        loop {
            at += 1;
            let text = &self.source[tokens.get(at)?.byte_range()];
            match text {
                "(" | "[" => depth += 1,
                ")" | "]" => depth -= 1,
                "{" | ";" if depth == 0 => return None,
                "with" if depth == 0 => break,
                _ => {}
            }
            depth += angle_delta(text);
        }
        let mut current = 0usize;
        let mut at_entry_start = true;
        loop {
            at += 1;
            let token = tokens.get(at)?;
            let text = &self.source[token.byte_range()];
            if depth == 0 {
                match text {
                    "," => {
                        current += 1;
                        at_entry_start = true;
                        continue;
                    }
                    "{" | ";" | "implements" | "on" => return None,
                    _ => {}
                }
                if at_entry_start && text.starts_with(|c: char| c.is_alphabetic() || c == '_') {
                    if current == position {
                        return self.token_range(tokens, at, at);
                    }
                    at_entry_start = false;
                }
            }
            depth += angle_delta(text);
            if depth < 0 {
                return None;
            }
        }
    }

    /// The UTF-16 span of the token range `first..=last`.
    fn token_range(
        &self,
        tokens: &[dartq_scanner::Token],
        first: usize,
        last: usize,
    ) -> Option<SourceRange> {
        let start = tokens.get(first)?.utf16_offset;
        let end_token = tokens.get(last)?;
        let end = end_token.utf16_offset + utf16_len(&self.source[end_token.byte_range()]);
        Some(SourceRange {
            offset: start,
            length: end.checked_sub(start)?,
        })
    }

    // —— rows ———————————————————————————————————————————————————————

    /// Stages one one-argument row into the source-ordered usage tail.
    fn stage_row(&mut self, code: SemanticCode, range: SourceRange, a0: &str, a1: &str) {
        let correction = code.correction_message(a0);
        self.report.usage.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            code.message(a0, a1),
            self.file,
            range,
            correction,
        ));
    }

    /// Stages one three-placeholder row (the mixin-constraint face).
    fn stage_row3(
        &mut self,
        code: SemanticCode,
        range: SourceRange,
        a0: &str,
        a1: &str,
        a2: &str,
    ) {
        let correction = code.correction_message(a0);
        self.report.usage.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            code.message3(a0, a1, a2),
            self.file,
            range,
            correction,
        ));
    }

    /// The interned spelling as an owned string (a message argument).
    fn display_of(&self, name: StringId) -> String {
        self.strings.resolve(name).to_owned()
    }

    /// A member's display name: a setter's `name=` spelling renders bare,
    /// while an operator whose own spelling ends in `=` (`==`, `[]=`) keeps
    /// it (the `operator `=`-glyph discriminator of
    /// `Element.displayName`).
    fn member_display(&self, name: StringId) -> String {
        let text = self.strings.resolve(name);
        match text.strip_suffix('=') {
            Some(bare)
                if bare
                    .chars()
                    .last()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$') =>
            {
                bare.to_owned()
            }
            _ => text.to_owned(),
        }
    }
}

/// The member declares a body the illegal-member face reports
/// (inheritance_override.dart:374-405): a field always (its implicit
/// accessors are concrete), an accessor/method only when it carries a body,
/// and a setter never.
fn declarable_concrete_member(entry: &MemberEntry) -> bool {
    match entry.kind {
        MemberKind::Setter => false,
        MemberKind::Field => !entry.is_static,
        MemberKind::Getter | MemberKind::Method => {
            !entry.is_static && !entry.is_abstract && !entry.is_external
        }
    }
}

/// The member counts as a concrete inherited implementation
/// (`hasConcreteImplementation`: non-static, non-abstract, non-external).
fn concrete_member(entry: &MemberEntry) -> bool {
    !entry.is_static && !entry.is_abstract && !entry.is_external
}

/// The `<`/`>` count of one token's text (`>>` and `>>>` arrive as one token
/// under the scanner's triple-shift mode).
fn angle_delta(text: &str) -> i32 {
    let mut delta = 0i32;
    for c in text.chars() {
        match c {
            '<' => delta += 1,
            '>' => delta -= 1,
            _ => {}
        }
    }
    delta
}

/// The UTF-16 length of one source slice.
fn utf16_len(text: &str) -> u32 {
    u32::try_from(text.chars().map(char::len_utf16).sum::<usize>()).unwrap_or(u32::MAX)
}
