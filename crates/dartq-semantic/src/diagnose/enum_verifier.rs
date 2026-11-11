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

/// The `concrete_class_with_abstract_member` code (upstream Dart SDK source,
/// `pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart:1968-1977`: a
/// `COMPILE_TIME_ERROR` with `hasPublishedDocs: true`). The semantic code
/// table carries no variant for it — this file owns its only emit site, so
/// the descriptor lives beside that site, the same shape
/// `invalid_override_of_non_virtual_member` takes in `overrides.rs`.
const CONCRETE_CLASS_WITH_ABSTRACT_MEMBER: CodeInfo = CodeInfo::new(
    "concrete_class_with_abstract_member",
    "CONCRETE_CLASS_WITH_ABSTRACT_MEMBER",
    DiagnosticType::CompileTimeError,
    true,
);

/// The missing name-span sentinel of a payload-restored entry (never an
/// anchor).
const MISSING_SPAN: u32 = u32::MAX;

/// The superclass-chain hop cap of the private-collision walk (a cycle in
/// the written clause faces terminates here — the chain is name-level and
/// the resolver may answer a face for itself).
const CONTAINER_CHAIN_CAP: u32 = 64;

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
            } else if container.kind == ContainerKind::Class
                && !container.is_abstract
                && !container.sealed
            {
                // The class arm of the same upstream check
                // (`inheritance_override.dart:417-437`): a NON-abstract class
                // whose interface lacks a concrete implementation of a name it
                // declares abstract reports the member. Named mixin
                // applications carry `ContainerKind::Class` too and the
                // oracle's gate is `element is ClassElementImpl &&
                // !element.isAbstract`, so they participate identically.
                //
                // `sealed` makes the class implicitly abstract (the class
                // modifiers' own rule), so it is exempt exactly like an
                // `abstract class`: the oracle on `sealed class S { void m(); }`
                // reports only the SUBCLASS's
                // `non_abstract_class_inherits_abstract_member`, never a
                // `concrete_class_with_abstract_member` on `S`. Every other
                // modifier (`base`/`interface`/`final`/`mixin class`) leaves
                // the class concrete and does report.
                self.verify_class_abstract_members(name, container);
            } else if self.implements_core_enum(name, container) {
                self.verify_enum_declarations(container.kind, container);
                self.verify_enum_inheritance(name, name_range, container);
            }
            self.verify_mixin_application(name, name_range, container);
            self.verify_mixin_private_collision(name, name_range, container);
            self.verify_const_constructor_mixin_fields(name, name_range, container);
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
                self.stage_row(
                    SemanticCode::IllegalConcreteEnumMember,
                    name_range,
                    &a0,
                    &a1,
                );
            }
        }
        // `_checkIllegalEnumValuesInheritance` reads the merged interface.
        let values = self.intern("values");
        if let Some(declaring) = self.inherited_interface_member(name, container, values) {
            let a0 = self.display_of(declaring);
            self.stage_row(SemanticCode::IllegalEnumValues, name_range, &a0, "");
        }
    }

    /// `_reportConcreteClassWithAbstractMember`'s CLASS arm
    /// (upstream Dart SDK source, `pkg/analyzer/lib/src/error/
    /// inheritance_override.dart:909-951`, reached from the `:417-437` loop):
    /// for every interface name with no concrete implementation, the check
    /// walks the class's OWN members and reports the first one that declares
    /// that name without a body — the more specific diagnosis, after which
    /// the caller `continue`s (so no `non_abstract_class_inherits_abstract_member`
    /// row joins it for the same name).
    ///
    /// The member walk's own shape (`:932-949`):
    /// * a method contributes its `name` (a setter contributes `name=`) and
    ///   the row displays the written lexeme;
    /// * a field contributes `name`, plus `name=` when the field group is not
    ///   `final`, both displaying the bare name.
    ///
    /// The row anchors at the whole member declaration (`.at(member)`), the
    /// same span the enum arm uses, and its arguments are the member's display
    /// name plus the enclosing class's own name. Gating mirrors the caller:
    /// only a non-abstract class reaches this (`container.is_abstract` is the
    /// dispatch condition), and a name whose declaration is a fragment of an
    /// augmentation is skipped by the interface walk before the report
    /// (`baseElement.isAugmentationWithoutAugmentedDeclaration`, `:431-435`).
    fn verify_class_abstract_members(&mut self, name: StringId, container: &'a Container) {
        let enclosing = self.display_of(name);
        // The abstract bodyless members the class declares, with the faces
        // they contribute (upstream `checkMemberNameCombo` over the AST
        // member list, `:910-930`): `(face, display)`.
        let mut faces: Vec<(StringId, String)> = Vec::new();
        for (member, entry) in container.members.iter() {
            if entry.name_range.offset == MISSING_SPAN || entry.is_static || !entry.is_abstract {
                continue;
            }
            let display = self.member_display(*member);
            faces.push((*member, display.clone()));
            if entry.kind == MemberKind::Field && !entry.is_final {
                let setter = self.setter_spelling(*member);
                faces.push((setter, display));
            }
        }
        // Upstream reports ONE member per missing name (the first whose face
        // matches, `:939/:943-946` return immediately), so a face is claimed
        // at most once across the container's names.
        let mut rows: Vec<(SourceRange, String)> = Vec::new();
        let mut reported: FxHashSet<u32> = FxHashSet::default();
        for (face, display) in faces {
            if self.interface_has_concrete(name, container, face) {
                continue;
            }
            let Some(entry) = container.members.get(&face).or_else(|| {
                // A setter face's declaration lives on the field's own entry.
                container
                    .members
                    .iter()
                    .find(|(member, entry)| {
                        entry.kind == MemberKind::Field
                            && !entry.is_final
                            && self.setter_spelling(*member) == face
                    })
                    .map(|(_, entry)| entry)
            }) else {
                continue;
            };
            let Some(span) = self.member_declaration_span(entry.name_range) else {
                continue;
            };
            if !reported.insert(span.offset) {
                continue;
            }
            rows.push((span, display));
        }
        for (span, display) in rows {
            self.report.usage.push(Diagnostic::new(
                CONCRETE_CLASS_WITH_ABSTRACT_MEMBER.dtype.severity(),
                CONCRETE_CLASS_WITH_ABSTRACT_MEMBER,
                format!(
                    "'{display}' must have a method body because '{enclosing}' isn't abstract."
                ),
                self.file,
                span,
                Some(format!(
                    "Try making '{enclosing}' abstract, or adding a body to '{display}'."
                )),
            ));
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
        // The clause-error short-circuit (error_verifier.dart:2440-2448):
        // `_checkForExtendsDisallowedClass` / `_checkForAllMixinErrorCodes`
        // (and the implements/constructor siblings) return true on a clause
        // error, and the whole inheritance block — this face included — is
        // skipped. A `with` entry that is no interface type at all
        // (`mixinTypes` whose `typeOrThrow` is not an `InterfaceType` — an
        // unresolved name) reports its own clause error the same way.
        let restricted = container
            .superclass
            .is_some_and(|face| self.disallowed_subtype_head(face).is_some())
            || container
                .with_clause
                .iter()
                .any(|face| self.disallowed_subtype_head(*face).is_some());
        if restricted {
            return;
        }
        let scope_lib = self.owner_library_of(self.file, name).unwrap_or(self.lib);
        let mut unresolved_entry = false;
        for mixin in container.with_clause.iter() {
            match self.resolve_clause_face(name, self.file, scope_lib, *mixin) {
                Some((found, _)) => {
                    let is_mixin = found.kind == ContainerKind::Mixin
                        || (found.kind == ContainerKind::Class && found.is_mixin_class);
                    if !is_mixin {
                        unresolved_entry = true;
                    }
                }
                None => unresolved_entry = true,
            }
        }
        if unresolved_entry {
            return;
        }
        let object_id = self.intern("Object");
        // `enclosingClass.supertype` upstream — the direct superclass, `Enum`
        // for an enum declaration, `Object` for a class with no `extends`.
        // The written name resolves to a container that may live outside
        // `self.file` (an imported supertype), so it goes through
        // `resolve_clause_face` rather than a same-file lookup.
        let implicit_super = match (container.superclass, container.kind) {
            (Some(face), _) => face,
            (None, ContainerKind::Enum) => self.intern("Enum"),
            (None, _) => object_id,
        };
        let resolved_super = self.resolve_clause_face(name, self.file, scope_lib, implicit_super);
        let mut previous: Vec<(StringId, FileId)> = Vec::new();
        for (position, mixin) in container.with_clause.iter().enumerate() {
            let resolved = self.resolve_clause_face(name, self.file, scope_lib, *mixin);
            let owner = resolved.map_or(self.file, |(_, file)| file);
            // `isInterfaceTypeInterface` gates the official loop: only a
            // resolved mixin answers its `on` clause.
            if let Some((mixin_container, mixin_file)) = resolved
                && (mixin_container.kind == ContainerKind::Mixin
                    || (mixin_container.kind == ContainerKind::Class
                        && mixin_container.is_mixin_class))
            {
                // The message spells the INSTANTIATED faces (upstream
                // `mixinType`/`superType`/`notImplementedType` are resolved
                // `InterfaceType`s, never their bare names).
                let wrote_mixin_args = container
                    .with_clause_args
                    .get(position)
                    .is_some_and(|args| *args != crate::model::ArgListId::EMPTY);
                let mixin_slots = self.inst_slots(
                    self.file,
                    container.with_clause_args.get(position).copied(),
                    mixin_container,
                    &[],
                    &[],
                );
                for (slot, constraint) in mixin_container.on_clause.iter().enumerate() {
                    // The constraint's resolved container — the element the
                    // clause walk compares against (alias- and
                    // prefix-spelled targets alike) and the container the
                    // message face reads.
                    let constraint_scope = self
                        .owner_library_of(mixin_file, *mixin)
                        .unwrap_or(scope_lib);
                    let constraint_container =
                        self.resolve_clause_face(*mixin, mixin_file, constraint_scope, *constraint);
                    // `typeSystem.isSubtypeOf(enclosingClass.supertype,
                    // constraint)` over the INSTANTIATED faces: a name-level
                    // hit is not the official answer (`I<dynamic>` is not a
                    // subtype of `I<int>`, `A<double>` not of `A<int>`,
                    // `B<X>` not of `B<Y>`). A `None` (an unmapped head)
                    // keeps the name-level verdict, the under-report
                    // direction.
                    let constraint_id = self.instantiated_constraint(
                        mixin_container,
                        mixin_file,
                        slot,
                        container,
                        position,
                    );
                    let super_answer = constraint_id.and_then(|sup| {
                        self.face_subtype(implicit_super, self.file, container.superclass_args, sup)
                    });
                    let by_super = super_answer.unwrap_or_else(|| {
                        implicit_super == *constraint
                            || *constraint == object_id
                            || resolved_super.is_some_and(|(super_container, super_file)| {
                                self.supertype_closure_contains(
                                    implicit_super,
                                    super_file,
                                    *constraint,
                                    constraint_container,
                                    super_container,
                                )
                            })
                    });
                    let mut satisfied = by_super;
                    for (index, (prior, prior_file)) in previous.iter().enumerate() {
                        let answer = constraint_id.and_then(|sup| {
                            self.face_subtype(
                                *prior,
                                *prior_file,
                                container
                                    .with_clause_args
                                    .get(index)
                                    .copied()
                                    .unwrap_or(crate::model::ArgListId::EMPTY),
                                sup,
                            )
                        });
                        if answer.unwrap_or_else(|| {
                            self.supertype_reaches(
                                *prior,
                                *prior_file,
                                *constraint,
                                constraint_container,
                            )
                        }) {
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
                    let a0 = self.face_display(
                        *mixin,
                        Some(mixin_container),
                        wrote_mixin_args,
                        mixin_slots.as_deref(),
                    );
                    let super_container = resolved_super.map(|(found, _)| found);
                    let super_slots = super_container.and_then(|found| {
                        self.inst_slots(
                            self.file,
                            Some(container.superclass_args)
                                .filter(|_| container.superclass.is_some()),
                            found,
                            &[],
                            &[],
                        )
                    });
                    let a1 = self.face_display(
                        implicit_super,
                        super_container,
                        container.superclass.is_some()
                            && container.superclass_args != crate::model::ArgListId::EMPTY,
                        super_slots.as_deref(),
                    );
                    let constraint_found = constraint_container.map(|(found, _)| found);
                    let wrote_constraint_args = mixin_container
                        .on_clause_args
                        .get(slot)
                        .is_some_and(|args| *args != crate::model::ArgListId::EMPTY);
                    let constraint_slots = self.inst_slots(
                        mixin_file,
                        mixin_container.on_clause_args.get(slot).copied(),
                        constraint_found.unwrap_or(mixin_container),
                        &mixin_container.type_param_names,
                        mixin_slots.as_deref().unwrap_or(&[]),
                    );
                    let a2 = self.face_display(
                        *constraint,
                        constraint_found,
                        wrote_constraint_args,
                        constraint_slots.as_deref(),
                    );
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

    /// `_checkForMixinWithConflictingPrivateMember`: a private member name a
    /// `with` entry brings in from ANOTHER library collides with the same
    /// name either on an earlier `with` entry of that library or on a
    /// concrete member the declared supertype inherits from the same
    /// library. The walk reports the first collision at the offending
    /// entry's name token and stops (error_verifier.dart:6192-6292).
    ///
    /// Upstream gates the whole face on `library != _currentLibrary` (a
    /// same-library private name is the ordinary shadowing case), keys the
    /// seen table per library, and probes the supertype with the
    /// library-qualified name (`Name(library.uri, name)`) — only a member
    /// DECLARED in the mixin's library can answer.
    fn verify_mixin_private_collision(
        &mut self,
        name: StringId,
        name_range: SourceRange,
        container: &'a Container,
    ) {
        if container.with_clause.is_empty() {
            return;
        }
        let current_lib = self.graph.library_of_file(self.file);
        let scope_lib = self.owner_library_of(self.file, name).unwrap_or(self.lib);
        let declared_super = container
            .superclass
            .and_then(|face| self.resolve_clause_face(name, self.file, scope_lib, face));
        let mut mixed: FxHashMap<dartq_resolution::LibraryId, FxHashMap<StringId, StringId>> =
            FxHashMap::default();
        let mut collision: Option<(SourceRange, String, String, String)> = None;
        'clause: for (position, mixin) in container.with_clause.iter().enumerate() {
            let Some((_, mixin_file)) =
                self.resolve_clause_face(name, self.file, scope_lib, *mixin)
            else {
                continue;
            };
            let Some(mixin_lib) = self.graph.library_of_file(mixin_file) else {
                continue;
            };
            // `library != _currentLibrary`: the same-library case never
            // reports (its private names collide by design).
            if current_lib == Some(mixin_lib) {
                continue;
            }
            let written = self.display_of(*mixin);
            for face in self.interface_member_faces(*mixin, mixin_file) {
                if !self.strings.resolve(face).starts_with('_') {
                    continue;
                }
                let Some(span) = self.with_clause_name_span(name_range, position) else {
                    break 'clause;
                };
                let display = self.member_display(face);
                let seen_here = mixed.entry(mixin_lib).or_default();
                if let Some(previous) = seen_here.get(&face).copied() {
                    collision = Some((span, display, written, self.display_of(previous)));
                    break 'clause;
                }
                seen_here.insert(face, *mixin);
                // `getMember(declaredSupertype.element, Name(library.uri,
                // name), concrete: true)` — a concrete member the declared
                // supertype inherits from the mixin's own library.
                if let Some((declaring, declaring_file)) =
                    self.declared_supertype_member(container, declared_super, face)
                    && self.graph.library_of_file(declaring_file) == Some(mixin_lib)
                {
                    collision = Some((span, display, written, self.display_of(declaring)));
                    break 'clause;
                }
            }
        }
        if let Some((span, a0, a1, a2)) = collision {
            self.stage_row3(
                SemanticCode::PrivateCollisionInMixinApplication,
                span,
                &a0,
                &a1,
                &a2,
            );
        }
    }

    /// Const-constructor-with-mixin-field: a const generative constructor on
    /// a class whose `with` applications bring instance fields reports at
    /// the constructor head's `errorRange`
    /// (error_verifier.dart:4040-4100 `_checkForConstConstructorWithNonConstSuper`).
    ///
    /// Upstream collects the instantiated mixins' instance fields (skipping
    /// static ones, a field-inducing accessor's synthetic halves, and an
    /// abstract `final` field — which is getter-only), then reports ONE row
    /// per const constructor: the singular diagnostic for a single field and
    /// the plural spelling for several (`constConstructorWithMixinWithFields`
    /// carries the same machine code name with a different problem text).
    fn verify_const_constructor_mixin_fields(
        &mut self,
        name: StringId,
        name_range: SourceRange,
        container: &'a Container,
    ) {
        if container.with_clause.is_empty() {
            return;
        }
        let heads = self.const_constructor_heads(name_range);
        if heads.is_empty() {
            return;
        }
        let scope_lib = self.owner_library_of(self.file, name).unwrap_or(self.lib);
        let mut fields: Vec<String> = Vec::new();
        for mixin in container.with_clause.iter() {
            let Some((mixin_container, _)) =
                self.resolve_clause_face(name, self.file, scope_lib, *mixin)
            else {
                continue;
            };
            let mixin_display = self.display_of(*mixin);
            for (member, entry) in mixin_container.members.iter() {
                if entry.kind != MemberKind::Field || entry.is_static {
                    continue;
                }
                if entry.is_abstract && entry.is_final {
                    continue;
                }
                fields.push(format!(
                    "'{mixin_display}.{}'",
                    self.member_display(*member)
                ));
            }
        }
        if fields.is_empty() {
            return;
        }
        let (message, correction) = if fields.len() == 1 {
            (
                format!(
                    "This constructor can't be declared 'const' because a mixin adds the \
                     instance field: {}.",
                    fields[0]
                ),
                None,
            )
        } else {
            (
                format!(
                    "This constructor can't be declared 'const' because the mixins add the \
                     instance fields: {}.",
                    fields.join(", ")
                ),
                Some(
                    "Try removing the 'const' keyword or removing the 'with' clause from the \
                     class declaration, or removing the fields from the mixin classes."
                        .to_string(),
                ),
            )
        };
        for head in heads {
            self.stage_row_message(
                SemanticCode::ConstConstructorWithMixinWithField,
                head,
                message.clone(),
                correction.clone(),
            );
        }
    }

    /// The declaration spans upstream anchors the const-constructor rows at
    /// (`ConstructorDeclarationImpl.errorRange`, the type name through the
    /// name / the sole token present). Read off the token table: a `const`
    /// at the container body's own level followed by a non-`factory` head,
    /// plus the primary-constructor `class const B(...)` header form.
    fn const_constructor_heads(&self, name_range: SourceRange) -> Vec<SourceRange> {
        let tokens = self.tokens.as_slice();
        let Some(at) = tokens
            .iter()
            .position(|token| token.utf16_offset == name_range.offset && !token.is_synthetic())
        else {
            return Vec::new();
        };
        let mut heads = Vec::new();
        // The primary-constructor header (`class const B(...)`): the
        // declaration's `errorRange` is the `const` keyword itself.
        if at > 0 && &self.source[tokens[at - 1].byte_range()] == "const" {
            if let Some(span) = self.token_range(tokens, at - 1, at - 1) {
                heads.push(span);
            }
        }
        let mut body = at;
        while body + 1 < tokens.len() {
            body += 1;
            if &self.source[tokens[body].byte_range()] == "{" {
                break;
            }
        }
        let mut depth = 0i32;
        let mut cursor = body;
        while cursor < tokens.len() {
            let text = &self.source[tokens[cursor].byte_range()];
            match text {
                "{" | "(" | "[" => depth += 1,
                "}" | ")" | "]" => {
                    depth -= 1;
                    if depth <= 0 {
                        break;
                    }
                }
                _ => {}
            }
            if depth == 1 && text == "const" {
                if let Some(head) = self.constructor_head_after(tokens, cursor + 1) {
                    heads.push(head);
                }
            }
            cursor += 1;
        }
        heads
    }

    /// The `errorRange` of the constructor whose head starts at `start`:
    /// `external` skipped, `factory` heads excluded (their const-ness is
    /// checked elsewhere), then the type name and selector through the
    /// head's own name token. `None` when the shape is not a constructor
    /// head (a `const` variable group, a `const` literal).
    fn constructor_head_after(
        &self,
        tokens: &[dartq_scanner::Token],
        mut start: usize,
    ) -> Option<SourceRange> {
        let text_at = |index: usize| tokens.get(index).map(|t| &self.source[t.byte_range()]);
        if text_at(start) == Some("external") {
            start += 1;
        }
        if text_at(start) == Some("factory") {
            return None;
        }
        let first = text_at(start)?;
        if !first.starts_with(|c: char| c.is_alphanumeric() || c == '_' || c == '$') {
            return None;
        }
        let mut last = start;
        if text_at(start + 1) == Some(".") {
            if text_at(start + 2)?.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
                last = start + 2;
            }
        }
        if text_at(last + 1) != Some("(") {
            return None;
        }
        self.token_range(tokens, start, last)
    }

    /// One container's non-static private interface member faces, in
    /// upstream's getters → setters → methods visit order, over the whole
    /// interface (the mixin's own rows plus every supertype's).
    fn interface_member_faces(&self, name: StringId, file: FileId) -> Vec<StringId> {
        let mut getters: Vec<StringId> = Vec::new();
        let mut setters: Vec<StringId> = Vec::new();
        let mut methods: Vec<StringId> = Vec::new();
        let mut seen: FxHashSet<(StringId, FileId)> = FxHashSet::default();
        let mut queue: VecDeque<(StringId, FileId)> = VecDeque::new();
        seen.insert((name, file));
        queue.push_back((name, file));
        while let Some((face, face_file)) = queue.pop_front() {
            let index = self.index;
            let Some(found) = index.get(face_file).and_then(|unit| unit.container(face)) else {
                continue;
            };
            for (member, entry) in found.members.iter() {
                if entry.is_static {
                    continue;
                }
                match entry.kind {
                    MemberKind::Getter => getters.push(*member),
                    MemberKind::Setter => setters.push(*member),
                    MemberKind::Method => methods.push(*member),
                    MemberKind::Field => {
                        getters.push(*member);
                        if !entry.is_final && !entry.is_const {
                            setters
                                .push(self.intern(&format!("{}=", self.strings.resolve(*member))));
                        }
                    }
                }
            }
            for (face_name, owner, _) in self.supertype_faces(face, face_file, found) {
                if seen.insert((face_name, owner)) {
                    queue.push_back((face_name, owner));
                }
            }
        }
        getters.extend(setters);
        getters.extend(methods);
        getters
    }

    /// The declaring face of a CONCRETE member named `member` on the declared
    /// supertype's own level, its `with` applications, or its superclass
    /// chain (`getMember(declaredSupertype.element, ..., concrete: true)`).
    /// `None` for an absent or abstract-only member — `implements` edges
    /// never contribute a concrete implementation.
    fn declared_supertype_member(
        &self,
        container: &'a Container,
        declared_super: Option<(&'a Container, FileId)>,
        member: StringId,
    ) -> Option<(StringId, FileId)> {
        let superclass = container.superclass?;
        let (super_container, super_file) = declared_super?;
        if super_container
            .members
            .get(&member)
            .is_some_and(concrete_member)
        {
            return Some((superclass, super_file));
        }
        let mut writer = superclass;
        let mut file = super_file;
        let mut current = super_container;
        let mut hops = 0u32;
        loop {
            if let Some(hit) = self.mixin_concrete_member_with_file(current, file, writer, member) {
                return Some(hit);
            }
            let face = current.superclass?;
            hops += 1;
            if hops > CONTAINER_CHAIN_CAP {
                return None;
            }
            let scope_lib = self.owner_library_of(file, writer).unwrap_or(self.lib);
            let (next, owner) = self.resolve_clause_face(writer, file, scope_lib, face)?;
            if next.members.get(&member).is_some_and(concrete_member) {
                return Some((face, owner));
            }
            writer = face;
            file = owner;
            current = next;
        }
    }

    /// The later-first `with` scan of one level, carrying the declaring
    /// entry's file (the private-collision walk compares libraries).
    fn mixin_concrete_member_with_file(
        &self,
        container: &'a Container,
        file: FileId,
        name: StringId,
        member: StringId,
    ) -> Option<(StringId, FileId)> {
        if container.with_clause.is_empty() {
            return None;
        }
        let scope_lib = self.owner_library_of(file, name).unwrap_or(self.lib);
        for mixin in container.with_clause.iter().rev() {
            let Some((resolved, owner)) = self.resolve_clause_face(name, file, scope_lib, *mixin)
            else {
                continue;
            };
            if resolved.members.get(&member).is_some_and(concrete_member) {
                return Some((*mixin, owner));
            }
        }
        None
    }

    /// One clause face's message spelling: the written name plus the
    /// instantiation the oracle displays (`'M<dynamic>'`, `'A<int>'`,
    /// `'C<X>'`). A face that wrote no argument list instantiates to bounds
    /// (`instantiateTypeFormalsToBounds`); only the all-`dynamic` bound
    /// column is spelled here, every other shape keeps the bare name — the
    /// under-report direction for a face this name-level renderer cannot
    /// spell.
    fn face_display(
        &self,
        written: StringId,
        container: Option<&Container>,
        wrote_args: bool,
        slots: Option<&[String]>,
    ) -> String {
        let mut text = self.display_of(written);
        let Some(container) = container else {
            return text;
        };
        let count = usize::from(container.type_params);
        if count == 0 {
            return text;
        }
        let fill: Vec<String> = if wrote_args {
            match slots {
                Some(slots) if slots.len() == count => slots.to_vec(),
                _ => return text,
            }
        } else {
            if container.type_param_bounds.len() != count
                || !container
                    .type_param_bounds
                    .iter()
                    .all(|bound| *bound == crate::model::Ty::Dynamic)
            {
                return text;
            }
            vec!["dynamic".to_string(); count]
        };
        text.push('<');
        text.push_str(&fill.join(", "));
        text.push('>');
        text
    }

    /// The argument-slot spellings of one clause face: the written list's
    /// slots (read from `unit_file`'s interner, with `subst_params` replaced
    /// by `subst_values`), or the all-`dynamic` bounds fill of `container`
    /// when the face wrote no argument list.
    fn inst_slots(
        &self,
        unit_file: FileId,
        args: Option<crate::model::ArgListId>,
        container: &Container,
        subst_params: &[StringId],
        subst_values: &[String],
    ) -> Option<Vec<String>> {
        if let Some(args) = args.filter(|id| *id != crate::model::ArgListId::EMPTY) {
            return self.clause_arg_slots(unit_file, args, subst_params, subst_values);
        }
        let count = usize::from(container.type_params);
        if count == 0
            || container.type_param_bounds.len() != count
            || !container
                .type_param_bounds
                .iter()
                .all(|bound| *bound == crate::model::Ty::Dynamic)
        {
            return None;
        }
        Some(vec!["dynamic".to_string(); count])
    }

    /// One written argument list's slot spellings with `params` substituted
    /// by `instantiation` (`mixin M<T> on A<T>` reads `A<dynamic>` when `M`
    /// instantiates to bounds). `None` when a slot is a shape this
    /// name-level renderer cannot spell (a nested argument list, a nullable
    /// or function/record face).
    fn clause_arg_slots(
        &self,
        file: FileId,
        args: crate::model::ArgListId,
        params: &[StringId],
        instantiation: &[String],
    ) -> Option<Vec<String>> {
        let unit = self.index.get(file)?;
        let slots = unit.type_args.get(args);
        if slots.is_empty() {
            return None;
        }
        let mut out = Vec::with_capacity(slots.len());
        for slot in slots {
            // A parameter slot inside the mixin's own `on` clause reads its
            // instantiated argument (`mixin M<T> on A<T>` applied bare reads
            // `A<dynamic>`); every other slot renders through the one type
            // display (nested lists, nullability, function and record shapes
            // included).
            if let crate::model::Ty::Named {
                name,
                nullable: false,
                args,
            } = slot
                && *args == crate::model::ArgListId::EMPTY
                && let Some(index) = params.iter().position(|param| param == name)
                && instantiation.len() > index
            {
                out.push(instantiation[index].clone());
                continue;
            }
            out.push(self.ty_display(self.boundary.ty(file, *slot)));
        }
        Some(out)
    }

    /// The `slot`-th `on` constraint of `mixin` as the INSTANTIATED type the
    /// official verdict compares (`mixinType.superclassConstraints` — the
    /// constraint already substituted through the mixin's own type
    /// arguments). `None` where the snapshot cannot back a face (an unmapped
    /// head, an unresolved argument) — the caller then keeps the name-level
    /// verdict.
    fn instantiated_constraint(
        &self,
        mixin: &'a Container,
        mixin_file: FileId,
        slot: usize,
        host: &'a Container,
        position: usize,
    ) -> Option<dartq_types::TypeId> {
        let ops = self.ops?;
        let constraint = *mixin.on_clause.get(slot)?;
        let args = mixin
            .on_clause_args
            .get(slot)
            .copied()
            .unwrap_or(crate::model::ArgListId::EMPTY);
        // The projection runs BEFORE the pool borrow: `Boundary::ty`
        // re-interns the projected face into the same `args` cell, and a
        // live shared `Ref` across that `borrow_mut` is a runtime panic.
        let face = self.boundary.ty(
            mixin_file,
            crate::model::Ty::Named {
                name: constraint,
                nullable: false,
                args,
            },
        );
        let base = {
            let pool = self.boundary.args.borrow();
            ops.map_ty_strict(face, &pool)?
        };
        let count = usize::from(mixin.type_params);
        if count == 0 {
            return Some(base);
        }
        let written = host
            .with_clause_args
            .get(position)
            .copied()
            .unwrap_or(crate::model::ArgListId::EMPTY);
        let pool = self.boundary.args.borrow();
        let mut substitution: Vec<(dartq_types::TypeParamId, dartq_types::TypeId)> =
            Vec::with_capacity(count);
        for (index, param_name) in mixin.type_param_names.iter().enumerate() {
            let param = ops.param_id(*param_name, mixin_file)?;
            let argument = if written == crate::model::ArgListId::EMPTY {
                // A face that wrote no arguments instantiates to bounds
                // (`M` applied bare reads `M<dynamic>`).
                ops.map_ty_strict(crate::model::Ty::Dynamic, &pool)?
            } else {
                let slots = pool.get(written);
                ops.map_ty_strict(*slots.get(index)?, &pool)?
            };
            substitution.push((param, argument));
        }
        Some(ops.substitute(base, &substitution))
    }

    /// `typeSystem.isSubtypeOf(face, sup)` for one clause face instantiated
    /// from its written argument list (`EMPTY` = instantiate to bounds).
    /// `None` only where a face does not map into the snapshot (an
    /// unresolved head); type parameters are decided by the parameter arms
    /// themselves (reflexivity, the left bound, and the bound-based
    /// `X <: Object`).
    fn face_subtype(
        &self,
        name: StringId,
        file: FileId,
        args: crate::model::ArgListId,
        sup: dartq_types::TypeId,
    ) -> Option<bool> {
        let ops = self.ops?;
        // The projection runs BEFORE the pool borrow (`Boundary::ty`
        // re-interns through the same `args` cell — the same discipline
        // `instantiated_constraint` keeps).
        let face = self.boundary.ty(
            file,
            crate::model::Ty::Named {
                name,
                nullable: false,
                args,
            },
        );
        let pool = self.boundary.args.borrow();
        let sub = ops.map_ty_strict(face, &pool)?;
        Some(ops.subtype_ids(sub, sup))
    }

    /// Whether one closure edge's face is the constraint: the same spelling,
    /// or the same RESOLVED element. Upstream compares `InterfaceType`s, so
    /// `I` satisfies a constraint spelled through a typedef
    /// (`typedef IAlias = I`) and a prefixed spelling resolves exactly like
    /// the bare one. The edge's container is handed in by
    /// [`Self::supertype_faces`], so this needs no re-resolution of the
    /// face's own prefix scope.
    fn closure_hits_target(
        &self,
        face: StringId,
        target: StringId,
        edge: &'a Container,
        target_container: Option<(&'a Container, FileId)>,
    ) -> bool {
        if face == target {
            return true;
        }
        target_container.is_some_and(|(found, _)| std::ptr::eq(edge, found))
    }

    // —— model probes ————————————————————————————————————————————————

    /// Whether `container`'s supertypes include `dart:core`'s `Enum`
    /// (`implementsDartCoreEnum`, inheritance_override.dart:287-289). The
    /// check is spelling-level: `Enum` is a sample-stable name in every
    /// modeled environment (the SDK stub and the test rig both declare it),
    /// and an enum declaration answers true by kind.
    fn implements_core_enum(&self, name: StringId, container: &'a Container) -> bool {
        if container.kind == ContainerKind::Enum {
            return true;
        }
        let target = self.intern("Enum");
        self.supertype_closure_contains(name, self.file, target, None, container)
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

    /// Whether the interface's IMPLEMENTED map answers `member` (upstream
    /// `Interface.implemented`, inheritance_manager3.dart): the concrete
    /// member the abstract-member checks compare against. The walk covers
    /// the container's own concrete member (a declared setter can answer a
    /// `name=` face the sibling abstract entry contributes), the supertype
    /// chain's concrete members, and the three synthesis arms upstream
    /// lands on the same map:
    ///
    /// * the field-induced accessors — a concrete non-static field
    ///   contributes its getter (`name`) and, when it is not `final`, its
    ///   setter (`name=`); a bare `get`/`set` lookup misses the pair
    ///   because the model keys fields under their plain name;
    /// * the implicit `Object`/`Enum` faces — a class with no written
    ///   `extends` still carries `Object`'s members in its interface
    ///   (`element.supertype` answers `Object`), and every superclass chain
    ///   bottoms out there; the frozen signatures answer `==`, `hashCode`,
    ///   `toString`, `noSuchMethod`, `runtimeType` (plus `Enum.index`);
    /// * the `noSuchMethod` forwarders — a NON-`Object` concrete
    ///   `noSuchMethod` in `implemented` synthesizes a forwarder for every
    ///   unimplemented interface name (inheritance_manager3.dart:696-708),
    ///   so `class A { void foo(); noSuchMethod(_) {} }` carries a
    ///   concrete `foo` and reports nothing;
    /// * the augmentation chain — a member entry keeps its primary
    ///   fragment's `is_abstract` even when an `augment` supplies the body
    ///   (the fragment never folds into the merged member map), so a
    ///   complete augmentation fragment of the same member makes it
    ///   concrete; a field augmentation induces both halves under the same
    ///   `final` split as the field arm.
    fn interface_has_concrete(
        &self,
        name: StringId,
        container: &'a Container,
        member: StringId,
    ) -> bool {
        // The container's own concrete member (upstream `implemented`
        // merges declared concrete members with the inherited set).
        if container.members.get(&member).is_some_and(concrete_member) {
            return true;
        }
        // The field-induced accessor pair: `member` may be a `name=` face
        // whose declaration lives on the field's own `name` entry. The
        // plain-name hit (a concrete field answers the getter face) already
        // landed above; this arm answers the setter half.
        let text = self.strings.resolve(member);
        if let Some(bare) = text.strip_suffix('=')
            && !bare.is_empty()
        {
            let bare_name = self.intern(bare);
            if container.members.get(&bare_name).is_some_and(|entry| {
                entry.kind == MemberKind::Field && !entry.is_final && concrete_member(entry)
            }) {
                return true;
            }
        }
        if self
            .interface_member(name, container, member, concrete_member)
            .is_some()
        {
            return true;
        }
        // The same induced-accessor arm across the supertype chain: a
        // `name=` face resolves through the declaring container's field
        // entry at every BFS level.
        if let Some(bare) = text.strip_suffix('=')
            && !bare.is_empty()
        {
            let bare_name = self.intern(bare);
            if self
                .interface_member(name, container, bare_name, |entry| {
                    entry.kind == MemberKind::Field && !entry.is_final && concrete_member(entry)
                })
                .is_some()
            {
                return true;
            }
        }
        // The implicit top faces: `Object` (classes, mixins) or `Enum`
        // (enums) are in every interface — `element.supertype` answers them
        // for a clause-less header — and their concrete members land in
        // `implemented` for every name they declare. The frozen member set
        // is fixed: `Object` answers `==`, `hashCode`, `runtimeType`,
        // `toString`, `noSuchMethod`; `Enum` adds `index` and falls through
        // to the `Object` table (the same table `frozen_top_member`
        // serves).
        if matches!(
            text,
            "==" | "hashCode" | "runtimeType" | "toString" | "noSuchMethod"
        ) || (container.kind == ContainerKind::Enum && text == "index")
        {
            return true;
        }
        // The `noSuchMethod` forwarders (inheritance_manager3.dart:696-708):
        // a concrete `noSuchMethod` NOT declared on `Object` fills every
        // unimplemented interface name. The container's own declaration and
        // the supertype chain both contribute; `Object`'s own `noSuchMethod`
        // is the excluded boundary, so the frozen arm does not count.
        let nsm = self.intern("noSuchMethod");
        let has_non_object_nsm = container.members.get(&nsm).is_some_and(concrete_member)
            || self
                .interface_member(name, container, nsm, concrete_member)
                .is_some();
        if has_non_object_nsm {
            return true;
        }
        // The augmentation chain: a complete `augment` fragment of this
        // member (a body or `external`) makes the merged element concrete.
        // Setter faces compare against the `name=` spelling; a complete
        // field fragment induces the setter half when it is not `final`.
        // The fragments live across the library's units — an `augment` in
        // a sibling unit completes the introduction's member the same —
        // so every unit's `augmentations()` table joins the walk.
        self.augmentations_enabled
            && self.graph.unit_files(self.lib).iter().any(|&unit| {
                self.index.get(unit).is_some_and(|elements| {
                    elements.augmentations().iter().any(|aug| {
                        aug.container == Some(name) && aug.is_complete && {
                            aug.name == member
                                || (aug.kind == crate::model::AugKind::Field
                                    && !aug.is_final
                                    && text
                                        .strip_suffix('=')
                                        .is_some_and(|bare| self.intern(bare) == aug.name))
                        }
                    })
                })
            })
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
        let mut queue: VecDeque<(StringId, FileId, &'a Container)> =
            self.supertype_faces(name, self.file, container).into();
        let mut seen: FxHashSet<(StringId, FileId)> = FxHashSet::default();
        seen.insert((name, self.file));
        while let Some((face, file, found)) = queue.pop_front() {
            if !seen.insert((face, file)) {
                continue;
            }
            if found.members.get(&member).is_some_and(&accepts) {
                return Some(face);
            }
            queue.extend(self.supertype_faces(face, file, found));
        }
        None
    }

    /// Whether `container`'s own supertype closure reaches `target`, starting
    /// from the file `container` is declared in (`start_file`) so clause faces
    /// written there resolve against its own imports — a supertype carried
    /// across a library boundary would otherwise read the diagnosing file's
    /// scope and miss the face. `target_container` is the already-resolved
    /// constraint (`verify_mixin_application` holds it), so an alias- or
    /// prefix-spelled target matches the same element the bare spelling
    /// names. `X <: Object` holds unconditionally.
    fn supertype_closure_contains(
        &self,
        name: StringId,
        start_file: FileId,
        target: StringId,
        target_container: Option<(&'a Container, FileId)>,
        container: &'a Container,
    ) -> bool {
        if target == self.intern("Object") {
            return true;
        }
        let mut queue: VecDeque<(StringId, FileId, &'a Container)> =
            self.supertype_faces(name, start_file, container).into();
        let mut seen: FxHashSet<(StringId, FileId)> = FxHashSet::default();
        seen.insert((name, start_file));
        while let Some((face, file, edge)) = queue.pop_front() {
            if self.closure_hits_target(face, target, edge, target_container) {
                return true;
            }
            if !seen.insert((face, file)) {
                continue;
            }
            queue.extend(self.supertype_faces(face, file, edge));
        }
        false
    }

    /// Whether the closure of the container declared as `start` in
    /// `file` reaches `target`.
    fn supertype_reaches(
        &self,
        start: StringId,
        file: FileId,
        target: StringId,
        target_container: Option<(&'a Container, FileId)>,
    ) -> bool {
        if start == target || target == self.intern("Object") {
            return true;
        }
        let index = self.index;
        let Some(container) = index.get(file).and_then(|unit| unit.container(start)) else {
            return false;
        };
        self.supertype_closure_contains(start, file, target, target_container, container)
    }

    /// One container's resolved supertype faces, in written order
    /// (`extends`, `with`, `implements`, and a mixin's `on`), each with the
    /// container it resolves to (the identity a target comparison reads).
    fn supertype_faces(
        &self,
        name: StringId,
        file: FileId,
        container: &'a Container,
    ) -> Vec<(StringId, FileId, &'a Container)> {
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
            if let Some((found, owner)) = self.resolve_clause_face(name, file, scope_lib, *face) {
                resolved.push((*face, owner, found));
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

    /// Stages one three-placeholder row (the mixin-constraint and
    /// private-collision faces). Their corrections interpolate the THIRD
    /// placeholder (`Try extending the class '{2}'` / `Try removing '{2}'
    /// from the 'with' clause`), so the correction reads `a2`.
    fn stage_row3(&mut self, code: SemanticCode, range: SourceRange, a0: &str, a1: &str, a2: &str) {
        let correction = code.correction_message(a2);
        self.report.usage.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            code.message3(a0, a1, a2),
            self.file,
            range,
            correction,
        ));
    }

    /// Stages one row whose problem text is built at the site (the plural
    /// `constConstructorWithMixinWithFields` spelling shares the machine
    /// code but not the message or correction template).
    fn stage_row_message(
        &mut self,
        code: SemanticCode,
        range: SourceRange,
        message: String,
        correction: Option<String>,
    ) {
        self.report.usage.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            message,
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

#[cfg(test)]
#[path = "ticket_3660_tests.rs"]
mod ticket_3660_tests;
