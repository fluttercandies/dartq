//! The extension-type declaration family's semantic home (#2807).
//!
//! The family's two faces split by trigger site, mirroring the pinned SDK
//! (upstream Dart SDK source, pkg/analyzer):
//!
//! * the *syntactic* representation shapes — `expected_representation_field`
//!   (missing formals, `this.x`/`super.x` initializing formals, delimiters,
//!   function-typed suffixes, the no-constructor-head synthesized list) and
//!   `missing_primary_constructor` — are the PARSER's: their anchors are
//!   parse-time token facts (the `this`/`super` keyword, the formal's begin
//!   token, `insertParens`'s zero-length synthetic `()` at the token after
//!   the type name), reproduced in `dartq-parser`'s
//!   `parse_primary_constructor_opt` with the verifier's decision order
//!   (`error_verifier.dart:5353-5460`). A semantic re-emission would double
//!   every parse row in the merged report, so this layer never reports
//!   them.
//! * the *name-level* checks — representation-depends-on-itself,
//!   `member_with_class_name`, `extension_type_declares_member_of_object`,
//!   the disabled-arm `expected_representation_type` — read the surviving
//!   single representation formal from the collected element model and live
//!   here.
//!
//! The split is the established house posture (recorded here from
//! overrides.rs in #2807): the collector does not carry the syntactic
//! shapes' token facts, and adding them would duplicate what the parser
//! already holds.

use super::*;

impl Engine<'_> {
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
}
