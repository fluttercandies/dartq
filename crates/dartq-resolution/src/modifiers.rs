//! Class-modifier legality: declaration-site combinations, cross-library
//! use-site restrictions, the base/final subtype requirement, induced
//! modifiers, and the official diagnostic code table — pure decision
//! functions over resolved facts, ready for the semantic layer to wire to
//! diagnostics.
//!
//! # Fact sources (behavior anchored, form native)
//!
//! * `pkg/analyzer/lib/src/error/error_verifier.dart` — the cross-library
//!   use-site checks:
//!   `_checkForSealedSupertypeOutsideOfLibrary` (:7424-7437, driven for
//!   the extends/with/implements faces at :2780-2784 and the
//!   implements/on faces of mixins at :8195-8199),
//!   `_checkForFinalSupertypeOutsideOfLibrary` (:5604-5687 — extends
//!   :5612-5627, implements :5628-5665 with the indirect-supertype skip
//!   :5641-5648, `on` constraints :5666-5686; the `with` parameter is
//!   never read),
//!   `_checkForBaseClassOrMixinImplementedOutsideOfLibrary`
//!   (:3712-3752 — the implements walk covers the named type plus its
//!   transitive supertypes and breaks at the first report),
//!   `_checkForInterfaceClassOrMixinSuperclassOutsideOfLibrary`
//!   (:5896-5926 — extends face only), and the shared
//!   `_mayIgnoreClassModifiers` gate.
//! * `pkg/analyzer/lib/src/error/base_or_final_type_verifier.dart` — the
//!   subtype-modifier requirement: `checkElement` (:40-74 — face order
//!   superclass → implements → with → on, one error per element),
//!   `_getExplicitlyBaseOrFinalElement` (:107-134) and its supertype
//!   sweep (:139-153), `_mayIgnoreClassModifiers` (:164-173), and
//!   `_reportRestrictionError` (:178-315) — the decision order below is
//!   that method's source order, including the pre-feature transitivity
//!   carve-out (:262-279) and the implements-anchored base branch
//!   (:240-256). The kind gates on `isBase`/`isFinal`/`isSealed` are its
//!   private extension (:318-352).
//! * `pkg/analyzer/lib/src/summary2/instance_member_inferrer.dart:649-719`
//!   — `_setInducedModifier`: sealed classes inherit an induced
//!   base/final/interface flag from their direct supertypes, in the
//!   source order below.
//! * `pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart` — the
//!   declaration-site combination matrix: the class branch of
//!   `parseClassOrNamedMixinApplication` (:2976-3015 — `abstract sealed`,
//!   `abstract final base`, `abstract final interface`), the enum branch
//!   (:723-730), the mixin branch (:810-818), and the mixin-class branch
//!   (:866-876).
//!
//! # Violation → official code
//!
//! Every decision answer carries a [`ModifierCode`], whose
//! [`ModifierCode::info`] row is the verbatim analyzer registry entry
//! (`pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart`, dart 3.15 main;
//! line cited per row). The outside-library family shares the machine
//! name `invalid_use_of_type_outside_library` and the
//! must-be-base/final/sealed family shares
//! `subtype_of_base_or_final_is_not_base_final_or_sealed`, so the row
//! also carries the registry's `uniqueName` — the load-bearing identity.
//!
//! | Decision face | Code | Registry row |
//! |---|---|---|
//! | sealed supertype outside its library | `SealedClassSubtypeOutsideOfLibrary` | diagnostic.g.dart:15536 |
//! | final class extended outside | `FinalClassExtendedOutsideOfLibrary` | :6732 |
//! | final class implemented outside | `FinalClassImplementedOutsideOfLibrary` | :6749 |
//! | final class as `on` constraint outside | `FinalClassUsedAsMixinConstraintOutsideOfLibrary` | :6767 |
//! | interface class extended outside | `InterfaceClassExtendedOutsideOfLibrary` | :8432 |
//! | base class implemented outside | `BaseClassImplementedOutsideOfLibrary` | :1506 |
//! | base mixin implemented outside | `BaseMixinImplementedOutsideOfLibrary` | :1533 |
//! | subtype of base not base/final/sealed | `SubtypeOfBaseIsNotBaseFinalOrSealed` | :15818 |
//! | subtype of final not base/final/sealed | `SubtypeOfFinalIsNotBaseFinalOrSealed` | :15840 |
//! | mixin twin of the base case | `MixinSubtypeOfBaseIsNotBase` | :11640 |
//! | mixin twin of the final case | `MixinSubtypeOfFinalIsNotBase` | :11660 |
//! | `abstract` + `sealed` class | `AbstractSealedClass` | :169 |
//! | `abstract final base` class | `AbstractFinalBaseClass` | :134 |
//! | `abstract final interface` class | `AbstractFinalInterfaceClass` | :147 |
//! | `base`/`final`/`interface`/`sealed` enum | `BaseEnum`/`FinalEnum`/`InterfaceEnum`/`SealedEnum` | :1519/:6791/:8445/:15549 |
//! | `final`/`interface`/`sealed` mixin | `FinalMixin`/`InterfaceMixin`/`SealedMixin` | :6830/:8455/:15559 |
//! | `final`/`interface`/`sealed` mixin class | `FinalMixinClass`/`InterfaceMixinClass`/`SealedMixinClass` | :6840/:8466/:15569 |
//!
//! # Sealed-ness fact sharing (ticket #281 interplay)
//!
//! The exhaustiveness checker consumes sealed-ness through
//! `dartq_types::TypeHierarchy::is_sealed` (the `#280` face feeding
//! `ExhaustivenessWorld::is_sealed`); the decisions here consume it
//! through [`ClassModifiers`]. Both faces must be fed from the same
//! declaration fact so case-completeness and the sealed-subtype
//! restriction cannot disagree. The official restriction is on *every*
//! supertype face (extends, with, implements, `on` — the sealed class can
//! be neither subclassed nor implemented outside its library,
//! error_verifier.dart:7424-7437), and it is gated only on library
//! equality — no `_mayIgnoreClassModifiers` exemption.

use dartq_diagnostics::DiagnosticType;

/// The declaration kind a modifier set belongs to — the official
/// element-model split behind the kind-gated `isBase`/`isFinal`/`isSealed`
/// getters (base_or_final_type_verifier.dart:318-352: `base` applies to
/// classes and mixins, `final`/`sealed` to classes only; enums carry none
/// of the three).
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash, Debug)]
pub enum DeclKind {
    /// A `class` or `mixin class` declaration (mixin-class-ness is the
    /// [`ClassModifiers::is_mixin_class`] flag).
    #[default]
    Class,
    /// A `mixin` declaration.
    Mixin,
    /// An `enum` declaration.
    Enum,
}

/// The effective modifier flags of one declaration — the element-model
/// getters the verifiers read (`isBase`/`isFinal`/`isSealed`/`isInterface`
/// include *induced* flags on sealed classes,
/// instance_member_inferrer.dart:649-719). Declaration-site combination
/// checks must be fed the *declared* keywords instead; see
/// [`declaration_violations`].
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is one official modifier keyword; the flag set mirrors the element model"
)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct ClassModifiers {
    /// The declaration kind.
    pub kind: DeclKind,
    /// The `abstract` keyword (classes only).
    pub is_abstract: bool,
    /// The `base` keyword (classes, mixins, mixin classes).
    pub is_base: bool,
    /// The `final` keyword (classes only; mixins/mixin classes carrying it
    /// are a declaration-site violation).
    pub is_final: bool,
    /// The `interface` keyword (classes only).
    pub is_interface: bool,
    /// The `sealed` keyword (classes only).
    pub is_sealed: bool,
    /// The `mixin class` declaration form.
    pub is_mixin_class: bool,
}

impl ClassModifiers {
    /// The kind-gated `isBase` (base_or_final_type_verifier.dart:319-329):
    /// classes and mixins, never enums.
    #[must_use]
    pub const fn base_face(&self) -> bool {
        self.is_base && !matches!(self.kind, DeclKind::Enum)
    }

    /// The kind-gated `isFinal` (:331-340): classes only.
    #[must_use]
    pub const fn final_face(&self) -> bool {
        self.is_final && matches!(self.kind, DeclKind::Class)
    }

    /// The kind-gated `isSealed` (:342-351): classes only — enums are
    /// closed sets for exhaustiveness but do not carry the sealed
    /// *modifier* restriction face.
    #[must_use]
    pub const fn sealed_face(&self) -> bool {
        self.is_sealed && matches!(self.kind, DeclKind::Class)
    }
}

/// The library facts the platform-exemption gate reads
/// (`_mayIgnoreClassModifiers`, base_or_final_type_verifier.dart:164-173):
/// platform (`dart:`) membership and the library's `class-modifiers`
/// feature state.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct LibraryFeatureFacts {
    /// `libraryElement.isInSdk`.
    pub is_sdk: bool,
    /// `featureSet.isEnabled(Feature.class_modifiers)`.
    pub has_class_modifiers: bool,
}

/// `_mayIgnoreClassModifiers` (base_or_final_type_verifier.dart:164-173;
/// the `error_verifier` carries the same gate for its outside-library
/// checks): only modifiers in platform libraries can be ignored — when the
/// *supertype's* library is the SDK and the *use-site* library is either
/// the SDK itself or predates class modifiers.
#[must_use]
pub fn may_ignore_class_modifiers(
    super_library: LibraryFeatureFacts,
    use_library: LibraryFeatureFacts,
) -> bool {
    if !super_library.is_sdk {
        return false;
    }
    if use_library.is_sdk {
        return true;
    }
    !use_library.has_class_modifiers
}

/// The modifier keyword as written — the report-anchor spelling of the
/// declaration-site violations.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ModifierKeyword {
    /// `abstract`.
    Abstract,
    /// `base`.
    Base,
    /// `final`.
    Final,
    /// `interface`.
    Interface,
    /// `sealed`.
    Sealed,
}

/// One illegal modifier combination at a declaration site: the code to
/// emit and the keyword token(s) to anchor it on.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DeclarationViolation {
    /// The official diagnostic.
    pub code: ModifierCode,
    /// The keyword anchor(s), in span order — the single token for the
    /// one-keyword diagnostics, `final`→`base`/`interface` for the spanned
    /// `abstract final …` pair (the official reports
    /// `reportRecoverableErrorWithEnd(finalToken, baseToken|interfaceToken)`,
    /// parser_impl.dart:2988-3005).
    pub anchors: Vec<ModifierKeyword>,
}

/// The declaration-site combination matrix, in the official report order.
///
/// Classes (parser_impl.dart:2976-3015, `parseClassOrNamedMixinApplication`):
/// `abstract`+`sealed` (:2983-2987), then the nested `final` pair checks
/// (:2988-3005) — `abstract final base` beats `abstract final interface`
/// exactly as the official `else if` chain nests them. Enums
/// (parser_impl.dart:723-730), mixins (:810-818), and mixin classes
/// (:866-876) accept no class modifier beyond `base`, reported per
/// present keyword in the official order (base, final, interface, sealed
/// for enums; final, interface, sealed for the other two).
///
/// The flags read here are the *declared* keywords: induced flags exist
/// only on sealed classes, which no rule below can flag.
/// `abstract` on mixins/enums/mixin-classes and out-of-order or duplicate
/// keywords are the parser layer's generic modifier diagnostics
/// (`extraneousModifier`, `duplicatedModifier`) and are not decisions of
/// this face.
/// Records one violation with its keyword anchors.
fn push_violation(
    violations: &mut Vec<DeclarationViolation>,
    code: ModifierCode,
    anchors: &[ModifierKeyword],
) {
    violations.push(DeclarationViolation {
        code,
        anchors: anchors.to_vec(),
    });
}

/// Records one violation only when its trigger keyword is present.
fn flag_violation(
    violations: &mut Vec<DeclarationViolation>,
    flagged: bool,
    code: ModifierCode,
    anchors: &[ModifierKeyword],
) {
    if flagged {
        push_violation(violations, code, anchors);
    }
}

#[must_use]
pub fn declaration_violations(modifiers: &ClassModifiers) -> Vec<DeclarationViolation> {
    let mut violations = Vec::new();
    match modifiers.kind {
        DeclKind::Enum => {
            // parser_impl.dart:723-730 — report order base, final,
            // interface, sealed.
            flag_violation(
                &mut violations,
                modifiers.is_base,
                ModifierCode::BaseEnum,
                &[ModifierKeyword::Base],
            );
            flag_violation(
                &mut violations,
                modifiers.is_final,
                ModifierCode::FinalEnum,
                &[ModifierKeyword::Final],
            );
            flag_violation(
                &mut violations,
                modifiers.is_interface,
                ModifierCode::InterfaceEnum,
                &[ModifierKeyword::Interface],
            );
            flag_violation(
                &mut violations,
                modifiers.is_sealed,
                ModifierCode::SealedEnum,
                &[ModifierKeyword::Sealed],
            );
        }
        DeclKind::Mixin => {
            // parser_impl.dart:810-818 — only `base` is legal on a mixin.
            flag_violation(
                &mut violations,
                modifiers.is_final,
                ModifierCode::FinalMixin,
                &[ModifierKeyword::Final],
            );
            flag_violation(
                &mut violations,
                modifiers.is_interface,
                ModifierCode::InterfaceMixin,
                &[ModifierKeyword::Interface],
            );
            flag_violation(
                &mut violations,
                modifiers.is_sealed,
                ModifierCode::SealedMixin,
                &[ModifierKeyword::Sealed],
            );
        }
        DeclKind::Class if modifiers.is_mixin_class => {
            // parser_impl.dart:866-876 — only `base` is legal on a mixin
            // class.
            flag_violation(
                &mut violations,
                modifiers.is_final,
                ModifierCode::FinalMixinClass,
                &[ModifierKeyword::Final],
            );
            flag_violation(
                &mut violations,
                modifiers.is_interface,
                ModifierCode::InterfaceMixinClass,
                &[ModifierKeyword::Interface],
            );
            flag_violation(
                &mut violations,
                modifiers.is_sealed,
                ModifierCode::SealedMixinClass,
                &[ModifierKeyword::Sealed],
            );
        }
        DeclKind::Class => {
            // parser_impl.dart:2976-3015 — the abstract pair checks, in
            // the official nesting.
            if modifiers.is_abstract && modifiers.is_sealed {
                push_violation(
                    &mut violations,
                    ModifierCode::AbstractSealedClass,
                    &[ModifierKeyword::Sealed],
                );
            } else if modifiers.is_abstract && modifiers.is_final {
                if modifiers.is_base {
                    push_violation(
                        &mut violations,
                        ModifierCode::AbstractFinalBaseClass,
                        &[ModifierKeyword::Final, ModifierKeyword::Base],
                    );
                } else if modifiers.is_interface {
                    push_violation(
                        &mut violations,
                        ModifierCode::AbstractFinalInterfaceClass,
                        &[ModifierKeyword::Final, ModifierKeyword::Interface],
                    );
                }
            }
        }
    }
    violations
}

/// The supertype face a use-site restriction was violated through.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Relation {
    /// The `extends` clause.
    Extends,
    /// The `with` clause.
    MixesIn,
    /// The `implements` clause.
    Implements,
    /// A mixin declaration's `on` clause.
    MixinConstraint,
}

/// The resolved facts of one candidate supertype declaration: its
/// effective modifiers, whether it lives in the use site's library, and
/// the precomputed platform-exemption gate for its library.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SuperFacts {
    /// The supertype declaration's effective modifiers.
    pub modifiers: ClassModifiers,
    /// The supertype's library == the use site's library.
    pub same_library: bool,
    /// `may_ignore_class_modifiers(supertype_library, use_library)` —
    /// precomputed by the caller (base_or_final_type_verifier.dart:164-173).
    pub may_ignore: bool,
}

/// `_checkForSealedSupertypeOutsideOfLibrary`
/// (error_verifier.dart:7424-7437): a sealed class cannot be extended,
/// mixed in, or implemented outside its library — every supertype face,
/// gated on library equality alone (no platform exemption). The official
/// pattern-matches `ClassElement`, so the kind gate sits in
/// [`ClassModifiers::sealed_face`]; enums cannot be supertypes anyway.
#[must_use]
pub fn sealed_supertype_violation(supertype: &SuperFacts) -> Option<ModifierCode> {
    if supertype.modifiers.sealed_face() && !supertype.same_library {
        Some(ModifierCode::SealedClassSubtypeOutsideOfLibrary)
    } else {
        None
    }
}

/// The extends face of `_checkForFinalSupertypeOutsideOfLibrary`
/// (error_verifier.dart:5612-5627): a `final` (non-sealed) class from
/// another library cannot be extended.
#[must_use]
pub fn final_extends_violation(supertype: &SuperFacts) -> Option<ModifierCode> {
    if supertype.modifiers.final_face()
        && !supertype.modifiers.sealed_face()
        && !supertype.same_library
        && !supertype.may_ignore
    {
        Some(ModifierCode::FinalClassExtendedOutsideOfLibrary)
    } else {
        None
    }
}

/// The `on`-clause face of `_checkForFinalSupertypeOutsideOfLibrary`
/// (error_verifier.dart:5666-5686): a `final` (non-sealed) class from
/// another library cannot be a mixin superclass constraint.
#[must_use]
pub fn final_mixin_constraint_violation(supertype: &SuperFacts) -> Option<ModifierCode> {
    if supertype.modifiers.final_face()
        && !supertype.modifiers.sealed_face()
        && !supertype.same_library
        && !supertype.may_ignore
    {
        Some(ModifierCode::FinalClassUsedAsMixinConstraintOutsideOfLibrary)
    } else {
        None
    }
}

/// One step of the implements-face walk
/// (`_checkForFinalSupertypeOutsideOfLibrary`, error_verifier.dart:5628-5665):
/// the walk covers the named type plus its transitive supertypes and
/// breaks at the first report, and an *indirect* final supertype is
/// skipped when the directly named declaration's library has class
/// modifiers enabled (:5641-5648 — "there is a nearer declaration that
/// would emit an error, if any").
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ImplementsStep {
    /// `true` for the directly named declaration itself, `false` for the
    /// `allSupertypes` expansion.
    pub is_named_type: bool,
    /// `type.element.library.featureSet.isEnabled(Feature.class_modifiers)`
    /// of the directly named declaration.
    pub named_type_has_class_modifiers: bool,
}

/// The implements face of `_checkForFinalSupertypeOutsideOfLibrary`
/// (error_verifier.dart:5628-5665) for one walked candidate. The caller
/// drives the walk order and the per-named-type break.
#[must_use]
pub fn final_implements_candidate(
    supertype: &SuperFacts,
    step: &ImplementsStep,
) -> Option<ModifierCode> {
    if !(supertype.modifiers.final_face()
        && !supertype.modifiers.sealed_face()
        && !supertype.same_library
        && !supertype.may_ignore)
    {
        return None;
    }
    if !step.is_named_type && step.named_type_has_class_modifiers {
        return None;
    }
    Some(ModifierCode::FinalClassImplementedOutsideOfLibrary)
}

/// `_checkForInterfaceClassOrMixinSuperclassOutsideOfLibrary`
/// (error_verifier.dart:5896-5926): an `interface` (non-sealed) class from
/// another library cannot be extended. The official reads
/// `superclassElement.isInterface` — declared `interface` keyword plus the
/// sealed-only induction (`instance_member_inferrer.dart:693/717`), and the
/// sealed case is excluded here — so the effective flag is the declared
/// keyword in every reachable input.
#[must_use]
pub fn interface_extends_violation(supertype: &SuperFacts) -> Option<ModifierCode> {
    if supertype.modifiers.is_interface
        && !supertype.modifiers.sealed_face()
        && !supertype.same_library
        && !supertype.may_ignore
    {
        Some(ModifierCode::InterfaceClassExtendedOutsideOfLibrary)
    } else {
        None
    }
}

/// One candidate of the implements-face base walk
/// (`_checkForBaseClassOrMixinImplementedOutsideOfLibrary`,
/// error_verifier.dart:3712-3752): the caller walks the named type plus
/// its transitive supertypes per `implements` entry and breaks at the
/// first report. A base *class* reports unless it is sealed (the sealed
/// case is the sealed check's); a base *mixin* always reports; enums
/// never carry the base face.
#[must_use]
pub fn base_implemented_candidate(supertype: &SuperFacts) -> Option<ModifierCode> {
    if !(supertype.modifiers.base_face() && !supertype.same_library && !supertype.may_ignore) {
        return None;
    }
    match supertype.modifiers.kind {
        DeclKind::Class if !supertype.modifiers.sealed_face() => {
            Some(ModifierCode::BaseClassImplementedOutsideOfLibrary)
        }
        DeclKind::Mixin => Some(ModifierCode::BaseMixinImplementedOutsideOfLibrary),
        _ => None,
    }
}

/// The resolved facts of one declared subtype (class or mixin) for the
/// base/final subtype-requirement family
/// (`BaseOrFinalTypeVerifier.checkElement` → `_reportRestrictionError`,
/// base_or_final_type_verifier.dart:40-74, :178-315). The caller drives
/// the face order — superclass first, then implements, then with, then
/// `on` constraints — and stops at the first violation per element
/// (:47-74's early returns).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SubtypeRestrictionInput {
    /// The subtype being declared (`element` in the official).
    pub element: ClassModifiers,
    /// The subtype's own library's facts (`element.library`).
    pub element_library: LibraryFeatureFacts,
    /// The *direct* super declaration named by the clause
    /// (`superElement`).
    pub super_element: ClassModifiers,
    /// The direct super declaration's library's facts.
    pub super_library: LibraryFeatureFacts,
    /// `superElement.library == element.library` (the :185-187 gate —
    /// only elements within the current library are checked).
    pub same_library: bool,
    /// The nearest explicitly `base`/`final` ancestor of the super
    /// element, per [`nearest_explicit_base_or_final`] — the official's
    /// `_getExplicitlyBaseOrFinalElement(superElement)` (:198). `None`
    /// ends the decision (…:204-206).
    pub explicit_base_or_final: Option<ExplicitBaseFinal>,
    /// `true` when the clause face is `implements` (the official's
    /// non-null `implementsNamedType`, :240-256).
    pub via_implements: bool,
}

/// The nearest explicit `base`/`final` ancestor's facts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ExplicitBaseFinal {
    /// The ancestor's effective modifiers.
    pub modifiers: ClassModifiers,
    /// The ancestor's library's facts.
    pub library: LibraryFeatureFacts,
    /// The ancestor's library == the subtype element's library.
    pub same_library: bool,
}

/// `_reportRestrictionError` (base_or_final_type_verifier.dart:178-315),
/// in source order:
///
/// 1. Only same-library super elements are checked (:185-187).
/// 2. The trigger: the super element is `base` or `final`, or it comes
///    from a pre-modifiers library while the use site has them
///    (:189-202). Otherwise there are no restrictions (:200-202).
/// 3. The explicit ancestor must exist (:204-206).
/// 4. The platform exemption, measured against the *ancestor's* library
///    (:208-210).
/// 5. `implements` + sealed super element + cross-library ancestor that
///    is `base`: the base-implemented family (:240-256) — the transitive
///    twin of the `error_verifier`'s direct check, anchored on the
///    implements entry.
/// 6. An element that is itself neither base, final, nor sealed (:258)
///    under a `final` ancestor reports the final family (:259-294) unless
///    the pre-feature transitivity carve-out applies (:262-279); under a
///    `base` ancestor it reports the base family (:295-311). Mixins get
///    their own message pair ("must be 'base'", :280-282/:296-298).
///
/// Message arguments: the outside-library family fills `{0}` with the
/// super/ancestor display name; the subtype family fills `{0}` with the
/// subtype's name and `{1}` with the ancestor's. The context message
/// ("… is defined here") attaches exactly when the direct super element
/// itself is not the explicit ancestor — the "induced modifier" case
/// (:222-236, `superElement.isSealed ? contextMessages : const []`).
#[must_use]
pub fn subtype_restriction_violation(input: &SubtypeRestrictionInput) -> Option<ModifierCode> {
    // (1) base_or_final_type_verifier.dart:185-187.
    if !input.same_library {
        return None;
    }
    // (2) :189-202 — including the pre-modifiers-library cross case.
    let triggered = input.super_element.base_face()
        || input.super_element.final_face()
        || (!input.super_library.has_class_modifiers && input.element_library.has_class_modifiers);
    if !triggered {
        return None;
    }
    // (3) :204-206.
    let ancestor = input.explicit_base_or_final?;
    // (4) :208-210 — the ancestor's library is the gate's super library.
    if may_ignore_class_modifiers(ancestor.library, input.element_library) {
        return None;
    }
    // (5) :240-256.
    if input.via_implements
        && input.super_element.sealed_face()
        && !ancestor.same_library
        && ancestor.modifiers.base_face()
    {
        return Some(match ancestor.modifiers.kind {
            DeclKind::Mixin => ModifierCode::BaseMixinImplementedOutsideOfLibrary,
            _ => ModifierCode::BaseClassImplementedOutsideOfLibrary,
        });
    }
    // (6) :258-312.
    if input.element.base_face() || input.element.final_face() || input.element.sealed_face() {
        return None;
    }
    if ancestor.modifiers.final_face() {
        // The carve-out (:262-279): a cross-library final ancestor under a
        // modifiers-enabled (or non-SDK) super library, or any
        // implements-anchored check, stays silent — the direct
        // outside-library errors already cover it.
        if !ancestor.same_library
            && (input.super_library.has_class_modifiers
                || !ancestor.library.is_sdk
                || input.via_implements)
        {
            return None;
        }
        return Some(match input.element.kind {
            DeclKind::Mixin => ModifierCode::MixinSubtypeOfFinalIsNotBase,
            _ => ModifierCode::SubtypeOfFinalIsNotBaseFinalOrSealed,
        });
    }
    if ancestor.modifiers.base_face() {
        return Some(match input.element.kind {
            DeclKind::Mixin => ModifierCode::MixinSubtypeOfBaseIsNotBase,
            _ => ModifierCode::SubtypeOfBaseIsNotBaseFinalOrSealed,
        });
    }
    None
}

/// The hierarchy facts [`nearest_explicit_base_or_final`] walks. The
/// semantic layer implements this over its element model; declaration ids
/// stay opaque.
pub trait ModifierHierarchy {
    /// The caller's declaration handle.
    type Decl: Copy + Eq + std::hash::Hash;
    /// The declaration's effective modifiers.
    fn modifiers(&self, decl: Self::Decl) -> ClassModifiers;
    /// The direct supertype declarations in the official walk order:
    /// the extends supertype first, then the `implements` list, then the
    /// `with` list, then the `on` constraints
    /// (base_or_final_type_verifier.dart:115-132).
    fn supertypes(&self, decl: Self::Decl) -> Vec<Self::Decl>;
}

/// `_getExplicitlyBaseOrFinalElement`
/// (base_or_final_type_verifier.dart:107-134): the first declaration in
/// the preorder walk — the element itself, then its supertypes in
/// [`ModifierHierarchy::supertypes`] order — that is explicitly `base` or
/// `final` and not `sealed`. A declaration carrying the keyword directly
/// qualifies immediately; induced-only carriers do not.
///
/// Termination note: the official relies on the acyclic, validated
/// element graph and recurses. The walk below is the same preorder with
/// an explicit stack and a visited set — a cycle-safe adaptation, the
/// same deviation the exhaustiveness port makes for its queue walk
/// (a revisit can only reproduce an earlier answer).
pub fn nearest_explicit_base_or_final<H: ModifierHierarchy>(
    hierarchy: &H,
    start: H::Decl,
) -> Option<H::Decl> {
    let qualifies = |decl: H::Decl| {
        let modifiers = hierarchy.modifiers(decl);
        (modifiers.base_face() || modifiers.final_face()) && !modifiers.sealed_face()
    };
    if qualifies(start) {
        return Some(start);
    }
    let mut visited = rustc_hash::FxHashSet::default();
    visited.insert(start);
    let mut stack: Vec<H::Decl> = hierarchy.supertypes(start);
    // Reverse-push keeps the official order: the last pushed supertype
    // (the first in declaration order) pops first.
    stack.reverse();
    while let Some(decl) = stack.pop() {
        if !visited.insert(decl) {
            continue;
        }
        if qualifies(decl) {
            return Some(decl);
        }
        let mut supertypes = hierarchy.supertypes(decl);
        supertypes.reverse();
        stack.extend(supertypes);
    }
    None
}

/// The induced modifier `_setInducedModifier` lands on a sealed class
/// (instance_member_inferrer.dart:649-719).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum InducedModifier {
    /// `classElement.isFinal = true`.
    Final,
    /// `classElement.isBase = true`.
    Base,
    /// `classElement.isInterface = true`.
    Interface,
}

/// `_setInducedModifier` (instance_member_inferrer.dart:649-719): only
/// *sealed* classes propagate induced modifiers, in the source order
/// below. The flags read here are effective; the official's
/// `isInterface` reads carry no kind gate in this method (mixins and
/// enums cannot be in these lists with the flag set — the parser forbids
/// `interface` on mixins, and enums never appear as supertypes).
#[must_use]
pub fn induced_modifier(
    element: &ClassModifiers,
    supertype: Option<&ClassModifiers>,
    interfaces: &[ClassModifiers],
    mixins: &[ClassModifiers],
) -> Option<InducedModifier> {
    // Only sealed classes propagate induced modifiers (:651-653).
    if !element.sealed_face() {
        return None;
    }
    if mixins.iter().any(ClassModifiers::final_face) {
        return Some(InducedModifier::Final); // :655-659
    }
    if let Some(supertype) = supertype {
        if supertype.final_face() {
            return Some(InducedModifier::Final); // :661-666
        }
        if supertype.base_face() {
            if mixins.iter().any(|m| m.is_interface) {
                return Some(InducedModifier::Final); // :668-677
            }
            return Some(InducedModifier::Base); // :679-680
        }
        if supertype.is_interface {
            if interfaces.iter().any(ClassModifiers::base_face)
                || mixins.iter().any(ClassModifiers::base_face)
            {
                return Some(InducedModifier::Final); // :682-694
            }
            return Some(InducedModifier::Interface); // :696-697
        }
    }
    if interfaces.iter().any(|i| i.base_face() || i.final_face())
        || mixins.iter().any(|m| m.base_face() || m.final_face())
    {
        return Some(InducedModifier::Base); // :700-706
    }
    if mixins.iter().any(|m| m.is_interface) {
        if interfaces.iter().any(ClassModifiers::base_face) {
            return Some(InducedModifier::Final); // :708-716
        }
        return Some(InducedModifier::Interface); // :717-718
    }
    None
}

/// The class-modifier diagnostic code set — one variant per registry
/// `uniqueName`. The outside-library family shares the machine name
/// `invalid_use_of_type_outside_library` and the subtype family shares
/// `subtype_of_base_or_final_is_not_base_final_or_sealed`, so
/// [`ModifierCodeInfo`] carries `unique_name` as the load-bearing
/// identity alongside the shared `name`/`machine` pair (the
/// [`crate::codes::ResolutionCode`] table's shape plus that one field).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModifierCode {
    /// error_verifier.dart:7424-7437 — sealed supertype outside its
    /// library, any face ([`sealed_supertype_violation`]).
    SealedClassSubtypeOutsideOfLibrary,
    /// error_verifier.dart:5612-5627 — extends face
    /// ([`final_extends_violation`]).
    FinalClassExtendedOutsideOfLibrary,
    /// error_verifier.dart:5628-5665 — implements face
    /// ([`final_implements_candidate`]).
    FinalClassImplementedOutsideOfLibrary,
    /// error_verifier.dart:5666-5686 — `on` face
    /// ([`final_mixin_constraint_violation`]).
    FinalClassUsedAsMixinConstraintOutsideOfLibrary,
    /// error_verifier.dart:5896-5926 — extends face
    /// ([`interface_extends_violation`]).
    InterfaceClassExtendedOutsideOfLibrary,
    /// error_verifier.dart:3712-3752 — base class in the implements walk
    /// ([`base_implemented_candidate`], and the
    /// [`subtype_restriction_violation`] branch :240-256).
    BaseClassImplementedOutsideOfLibrary,
    /// error_verifier.dart:3744-3749 — base mixin in the implements walk.
    BaseMixinImplementedOutsideOfLibrary,
    /// base_or_final_type_verifier.dart:295-311 — class/mixin-class
    /// subtype of a base ancestor ([`subtype_restriction_violation`]).
    SubtypeOfBaseIsNotBaseFinalOrSealed,
    /// base_or_final_type_verifier.dart:259-294 — subtype of a final
    /// ancestor.
    SubtypeOfFinalIsNotBaseFinalOrSealed,
    /// base_or_final_type_verifier.dart:296-298 — the mixin twin of the
    /// base case.
    MixinSubtypeOfBaseIsNotBase,
    /// base_or_final_type_verifier.dart:280-282 — the mixin twin of the
    /// final case.
    MixinSubtypeOfFinalIsNotBase,
    /// parser_impl.dart:2983-2987 — `abstract sealed class`.
    AbstractSealedClass,
    /// parser_impl.dart:2988-2996 — `abstract final base class`.
    AbstractFinalBaseClass,
    /// parser_impl.dart:2996-3005 — `abstract final interface class`.
    AbstractFinalInterfaceClass,
    /// parser_impl.dart:725-727 — `base enum`.
    BaseEnum,
    /// parser_impl.dart:728-729 — `final enum`.
    FinalEnum,
    /// parser_impl.dart:731-733 — `interface enum`.
    InterfaceEnum,
    /// parser_impl.dart:734-736 — `sealed enum`.
    SealedEnum,
    /// parser_impl.dart:813-815 — `final mixin`.
    FinalMixin,
    /// parser_impl.dart:817-819 — `interface mixin`.
    InterfaceMixin,
    /// parser_impl.dart:820-822 — `sealed mixin`.
    SealedMixin,
    /// parser_impl.dart:869-871 — `final mixin class`.
    FinalMixinClass,
    /// parser_impl.dart:873-875 — `interface mixin class`.
    InterfaceMixinClass,
    /// parser_impl.dart:876-878 — `sealed mixin class`.
    SealedMixinClass,
}

/// Official descriptor of one [`ModifierCode`]: the registry row verbatim
/// (`pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart`, dart 3.15 main;
/// line cited per row). `{0}`… are placeholders. All outside-library and
/// subtype-family rows are `COMPILE_TIME_ERROR` and published; the
/// declaration-site rows are `SYNTACTIC_ERROR`, and exactly
/// [`ModifierCode::AbstractSealedClass`] among them is published.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModifierCodeInfo {
    /// Official `snake_case` code name (`analyzerCode` — shared inside
    /// the two families).
    pub name: &'static str,
    /// `UPPERCASE` machine form.
    pub machine: &'static str,
    /// The registry `uniqueName` — the disambiguating identity where
    /// `name` is shared.
    pub unique_name: &'static str,
    /// Verbatim oracle `problemMessage` template.
    pub message: &'static str,
    /// Verbatim oracle `correctionMessage`; none of these rows carry one.
    pub correction: Option<&'static str>,
    /// The registry row's family — the machine-format TYPE value.
    pub dtype: DiagnosticType,
    /// The registry row's `hasPublishedDocs`.
    pub has_published: bool,
}

impl ModifierCode {
    /// The frozen official descriptor of this code. Length is the frozen
    /// table itself — a data table, not logic.
    #[expect(
        clippy::too_many_lines,
        reason = "frozen one-arm-per-row data table of verbatim registry strings"
    )]
    #[must_use]
    pub const fn info(self) -> ModifierCodeInfo {
        match self {
            Self::SealedClassSubtypeOutsideOfLibrary => ModifierCodeInfo {
                name: "invalid_use_of_type_outside_library",
                machine: "INVALID_USE_OF_TYPE_OUTSIDE_LIBRARY",
                unique_name: "sealed_class_subtype_outside_of_library",
                message: "The class '{0}' can't be extended, implemented, or mixed in outside of its library because it's a sealed class.",
                correction: None,
                dtype: DiagnosticType::CompileTimeError,
                has_published: true,
            },
            Self::FinalClassExtendedOutsideOfLibrary => ModifierCodeInfo {
                name: "invalid_use_of_type_outside_library",
                machine: "INVALID_USE_OF_TYPE_OUTSIDE_LIBRARY",
                unique_name: "final_class_extended_outside_of_library",
                message: "The class '{0}' can't be extended outside of its library because it's a final class.",
                correction: None,
                dtype: DiagnosticType::CompileTimeError,
                has_published: true,
            },
            Self::FinalClassImplementedOutsideOfLibrary => ModifierCodeInfo {
                name: "invalid_use_of_type_outside_library",
                machine: "INVALID_USE_OF_TYPE_OUTSIDE_LIBRARY",
                unique_name: "final_class_implemented_outside_of_library",
                message: "The class '{0}' can't be implemented outside of its library because it's a final class.",
                correction: None,
                dtype: DiagnosticType::CompileTimeError,
                has_published: true,
            },
            Self::FinalClassUsedAsMixinConstraintOutsideOfLibrary => ModifierCodeInfo {
                name: "invalid_use_of_type_outside_library",
                machine: "INVALID_USE_OF_TYPE_OUTSIDE_LIBRARY",
                unique_name: "final_class_used_as_mixin_constraint_outside_of_library",
                message: "The class '{0}' can't be used as a mixin superclass constraint outside of its library because it's a final class.",
                correction: None,
                dtype: DiagnosticType::CompileTimeError,
                has_published: true,
            },
            Self::InterfaceClassExtendedOutsideOfLibrary => ModifierCodeInfo {
                name: "invalid_use_of_type_outside_library",
                machine: "INVALID_USE_OF_TYPE_OUTSIDE_LIBRARY",
                unique_name: "interface_class_extended_outside_of_library",
                message: "The class '{0}' can't be extended outside of its library because it's an interface class.",
                correction: None,
                dtype: DiagnosticType::CompileTimeError,
                has_published: true,
            },
            Self::BaseClassImplementedOutsideOfLibrary => ModifierCodeInfo {
                name: "invalid_use_of_type_outside_library",
                machine: "INVALID_USE_OF_TYPE_OUTSIDE_LIBRARY",
                unique_name: "base_class_implemented_outside_of_library",
                message: "The class '{0}' can't be implemented outside of its library because it's a base class.",
                correction: None,
                dtype: DiagnosticType::CompileTimeError,
                has_published: true,
            },
            Self::BaseMixinImplementedOutsideOfLibrary => ModifierCodeInfo {
                name: "invalid_use_of_type_outside_library",
                machine: "INVALID_USE_OF_TYPE_OUTSIDE_LIBRARY",
                unique_name: "base_mixin_implemented_outside_of_library",
                message: "The mixin '{0}' can't be implemented outside of its library because it's a base mixin.",
                correction: None,
                dtype: DiagnosticType::CompileTimeError,
                has_published: true,
            },
            Self::SubtypeOfBaseIsNotBaseFinalOrSealed => ModifierCodeInfo {
                name: "subtype_of_base_or_final_is_not_base_final_or_sealed",
                machine: "SUBTYPE_OF_BASE_OR_FINAL_IS_NOT_BASE_FINAL_OR_SEALED",
                unique_name: "subtype_of_base_is_not_base_final_or_sealed",
                message: "The type '{0}' must be 'base', 'final' or 'sealed' because the supertype '{1}' is 'base'.",
                correction: None,
                dtype: DiagnosticType::CompileTimeError,
                has_published: true,
            },
            Self::SubtypeOfFinalIsNotBaseFinalOrSealed => ModifierCodeInfo {
                name: "subtype_of_base_or_final_is_not_base_final_or_sealed",
                machine: "SUBTYPE_OF_BASE_OR_FINAL_IS_NOT_BASE_FINAL_OR_SEALED",
                unique_name: "subtype_of_final_is_not_base_final_or_sealed",
                message: "The type '{0}' must be 'base', 'final' or 'sealed' because the supertype '{1}' is 'final'.",
                correction: None,
                dtype: DiagnosticType::CompileTimeError,
                has_published: true,
            },
            Self::MixinSubtypeOfBaseIsNotBase => ModifierCodeInfo {
                name: "subtype_of_base_or_final_is_not_base_final_or_sealed",
                machine: "SUBTYPE_OF_BASE_OR_FINAL_IS_NOT_BASE_FINAL_OR_SEALED",
                unique_name: "mixin_subtype_of_base_is_not_base",
                message: "The mixin '{0}' must be 'base' because the supertype '{1}' is 'base'.",
                correction: None,
                dtype: DiagnosticType::CompileTimeError,
                has_published: true,
            },
            Self::MixinSubtypeOfFinalIsNotBase => ModifierCodeInfo {
                name: "subtype_of_base_or_final_is_not_base_final_or_sealed",
                machine: "SUBTYPE_OF_BASE_OR_FINAL_IS_NOT_BASE_FINAL_OR_SEALED",
                unique_name: "mixin_subtype_of_final_is_not_base",
                message: "The mixin '{0}' must be 'base' because the supertype '{1}' is 'final'.",
                correction: None,
                dtype: DiagnosticType::CompileTimeError,
                has_published: true,
            },
            Self::AbstractSealedClass => ModifierCodeInfo {
                name: "abstract_sealed_class",
                machine: "ABSTRACT_SEALED_CLASS",
                unique_name: "abstract_sealed_class",
                message: "A 'sealed' class can't be marked 'abstract' because it's already implicitly abstract.",
                correction: Some("Try removing the 'abstract' keyword."),
                dtype: DiagnosticType::SyntacticError,
                has_published: true,
            },
            Self::AbstractFinalBaseClass => ModifierCodeInfo {
                name: "abstract_final_base_class",
                machine: "ABSTRACT_FINAL_BASE_CLASS",
                unique_name: "abstract_final_base_class",
                message: "An 'abstract' class can't be declared as both 'final' and 'base'.",
                correction: Some("Try removing either the 'final' or 'base' keyword."),
                dtype: DiagnosticType::SyntacticError,
                has_published: false,
            },
            Self::AbstractFinalInterfaceClass => ModifierCodeInfo {
                name: "abstract_final_interface_class",
                machine: "ABSTRACT_FINAL_INTERFACE_CLASS",
                unique_name: "abstract_final_interface_class",
                message: "An 'abstract' class can't be declared as both 'final' and 'interface'.",
                correction: Some("Try removing either the 'final' or 'interface' keyword."),
                dtype: DiagnosticType::SyntacticError,
                has_published: false,
            },
            Self::BaseEnum => ModifierCodeInfo {
                name: "base_enum",
                machine: "BASE_ENUM",
                unique_name: "base_enum",
                message: "Enums can't be declared to be 'base'.",
                correction: Some("Try removing the keyword 'base'."),
                dtype: DiagnosticType::SyntacticError,
                has_published: false,
            },
            Self::FinalEnum => ModifierCodeInfo {
                name: "final_enum",
                machine: "FINAL_ENUM",
                unique_name: "final_enum",
                message: "Enums can't be declared to be 'final'.",
                correction: Some("Try removing the keyword 'final'."),
                dtype: DiagnosticType::SyntacticError,
                has_published: false,
            },
            Self::InterfaceEnum => ModifierCodeInfo {
                name: "interface_enum",
                machine: "INTERFACE_ENUM",
                unique_name: "interface_enum",
                message: "Enums can't be declared to be 'interface'.",
                correction: Some("Try removing the keyword 'interface'."),
                dtype: DiagnosticType::SyntacticError,
                has_published: false,
            },
            Self::SealedEnum => ModifierCodeInfo {
                name: "sealed_enum",
                machine: "SEALED_ENUM",
                unique_name: "sealed_enum",
                message: "Enums can't be declared to be 'sealed'.",
                correction: Some("Try removing the keyword 'sealed'."),
                dtype: DiagnosticType::SyntacticError,
                has_published: false,
            },
            Self::FinalMixin => ModifierCodeInfo {
                name: "final_mixin",
                machine: "FINAL_MIXIN",
                unique_name: "final_mixin",
                message: "A mixin can't be declared 'final'.",
                correction: Some("Try removing the 'final' keyword."),
                dtype: DiagnosticType::SyntacticError,
                has_published: false,
            },
            Self::InterfaceMixin => ModifierCodeInfo {
                name: "interface_mixin",
                machine: "INTERFACE_MIXIN",
                unique_name: "interface_mixin",
                message: "A mixin can't be declared 'interface'.",
                correction: Some("Try removing the 'interface' keyword."),
                dtype: DiagnosticType::SyntacticError,
                has_published: false,
            },
            Self::SealedMixin => ModifierCodeInfo {
                name: "sealed_mixin",
                machine: "SEALED_MIXIN",
                unique_name: "sealed_mixin",
                message: "A mixin can't be declared 'sealed'.",
                correction: Some("Try removing the 'sealed' keyword."),
                dtype: DiagnosticType::SyntacticError,
                has_published: false,
            },
            Self::FinalMixinClass => ModifierCodeInfo {
                name: "final_mixin_class",
                machine: "FINAL_MIXIN_CLASS",
                unique_name: "final_mixin_class",
                message: "A mixin class can't be declared 'final'.",
                correction: Some("Try removing the 'final' keyword."),
                dtype: DiagnosticType::SyntacticError,
                has_published: false,
            },
            Self::InterfaceMixinClass => ModifierCodeInfo {
                name: "interface_mixin_class",
                machine: "INTERFACE_MIXIN_CLASS",
                unique_name: "interface_mixin_class",
                message: "A mixin class can't be declared 'interface'.",
                correction: Some("Try removing the 'interface' keyword."),
                dtype: DiagnosticType::SyntacticError,
                has_published: false,
            },
            Self::SealedMixinClass => ModifierCodeInfo {
                name: "sealed_mixin_class",
                machine: "SEALED_MIXIN_CLASS",
                unique_name: "sealed_mixin_class",
                message: "A mixin class can't be declared 'sealed'.",
                correction: Some("Try removing the 'sealed' keyword."),
                dtype: DiagnosticType::SyntacticError,
                has_published: false,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustc_hash::FxHashMap;

    const SDK: LibraryFeatureFacts = LibraryFeatureFacts {
        is_sdk: true,
        has_class_modifiers: true,
    };
    const USER: LibraryFeatureFacts = LibraryFeatureFacts {
        is_sdk: false,
        has_class_modifiers: true,
    };
    const PRE_MODIFIERS: LibraryFeatureFacts = LibraryFeatureFacts {
        is_sdk: false,
        has_class_modifiers: false,
    };

    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "test shorthand mirrors the four declared keyword flags"
    )]
    fn class_mods(
        is_base: bool,
        is_final: bool,
        is_interface: bool,
        is_sealed: bool,
    ) -> ClassModifiers {
        ClassModifiers {
            kind: DeclKind::Class,
            is_base,
            is_final,
            is_interface,
            is_sealed,
            ..ClassModifiers::default()
        }
    }

    fn super_facts(modifiers: ClassModifiers, same_library: bool, may_ignore: bool) -> SuperFacts {
        SuperFacts {
            modifiers,
            same_library,
            may_ignore,
        }
    }

    #[test]
    fn kind_gates_mirror_the_official_extension() {
        // base_or_final_type_verifier.dart:318-352.
        assert!(class_mods(true, false, false, false).base_face());
        let mut mixin = class_mods(true, true, true, true);
        mixin.kind = DeclKind::Mixin;
        assert!(mixin.base_face());
        assert!(!mixin.final_face());
        assert!(!mixin.sealed_face());
        let mut enumeration = class_mods(true, true, true, true);
        enumeration.kind = DeclKind::Enum;
        assert!(!enumeration.base_face());
        assert!(!enumeration.final_face());
        assert!(!enumeration.sealed_face());
    }

    #[test]
    fn may_ignore_matches_the_official_gate() {
        // base_or_final_type_verifier.dart:164-173.
        // Non-SDK supertype library: never.
        assert!(!may_ignore_class_modifiers(USER, USER));
        // SDK supertype, SDK use site: yes.
        assert!(may_ignore_class_modifiers(SDK, SDK));
        // SDK supertype, pre-modifiers use site: yes.
        assert!(may_ignore_class_modifiers(SDK, PRE_MODIFIERS));
        // SDK supertype, modern user library: no.
        assert!(!may_ignore_class_modifiers(SDK, USER));
    }

    #[test]
    fn declaration_matrix_rejects_every_enum_modifier() {
        // parser_impl.dart:723-730 — order base, final, interface, sealed.
        let mut enumeration = ClassModifiers {
            kind: DeclKind::Enum,
            ..ClassModifiers::default()
        };
        assert!(declaration_violations(&enumeration).is_empty());
        enumeration.is_sealed = true;
        enumeration.is_base = true;
        enumeration.is_final = true;
        enumeration.is_interface = true;
        let violations = declaration_violations(&enumeration);
        assert_eq!(
            violations.iter().map(|v| v.code).collect::<Vec<_>>(),
            vec![
                ModifierCode::BaseEnum,
                ModifierCode::FinalEnum,
                ModifierCode::InterfaceEnum,
                ModifierCode::SealedEnum,
            ]
        );
        assert!(violations[3].anchors.contains(&ModifierKeyword::Sealed));
    }

    #[test]
    fn declaration_matrix_mixins_and_mixin_classes_accept_only_base() {
        let mut mixin = ClassModifiers {
            kind: DeclKind::Mixin,
            is_base: true,
            ..ClassModifiers::default()
        };
        assert!(declaration_violations(&mixin).is_empty());
        mixin.is_final = true;
        mixin.is_interface = true;
        mixin.is_sealed = true;
        assert_eq!(
            declaration_violations(&mixin)
                .iter()
                .map(|v| v.code)
                .collect::<Vec<_>>(),
            vec![
                ModifierCode::FinalMixin,
                ModifierCode::InterfaceMixin,
                ModifierCode::SealedMixin,
            ]
        );

        let mut mixin_class = ClassModifiers {
            kind: DeclKind::Class,
            is_mixin_class: true,
            is_base: true,
            ..ClassModifiers::default()
        };
        assert!(declaration_violations(&mixin_class).is_empty());
        mixin_class.is_final = true;
        assert_eq!(
            declaration_violations(&mixin_class)
                .iter()
                .map(|v| v.code)
                .collect::<Vec<_>>(),
            vec![ModifierCode::FinalMixinClass]
        );
    }

    #[test]
    fn declaration_matrix_class_combinations() {
        // Legal combinations.
        for mods in [
            class_mods(false, false, false, false),
            class_mods(true, false, false, false),
            class_mods(false, true, false, false),
            class_mods(true, true, false, false),
            class_mods(false, false, false, true),
            class_mods(false, false, true, false),
            {
                let mut m = class_mods(false, false, false, false);
                m.is_abstract = true;
                m
            },
            {
                let mut m = class_mods(false, true, false, false);
                m.is_abstract = true;
                m
            },
            {
                let mut m = class_mods(true, false, false, false);
                m.is_abstract = true;
                m
            },
            {
                let mut m = class_mods(false, false, false, false);
                m.is_mixin_class = true;
                m
            },
        ] {
            assert!(
                declaration_violations(&mods).is_empty(),
                "unexpected violation for {mods:?}"
            );
        }
        // abstract sealed — reported at the sealed keyword
        // (parser_impl.dart:2983-2987).
        let mut abstract_sealed = class_mods(false, false, false, true);
        abstract_sealed.is_abstract = true;
        let violations = declaration_violations(&abstract_sealed);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].code, ModifierCode::AbstractSealedClass);
        assert_eq!(violations[0].anchors, vec![ModifierKeyword::Sealed]);
        // abstract final base — spanned final→base
        // (parser_impl.dart:2988-2996); the official else-if nesting
        // prefers it over the interface twin.
        let mut abstract_final_base = abstract_sealed;
        abstract_final_base.is_sealed = false;
        abstract_final_base.is_final = true;
        abstract_final_base.is_base = true;
        abstract_final_base.is_interface = true;
        let violations = declaration_violations(&abstract_final_base);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].code, ModifierCode::AbstractFinalBaseClass);
        assert_eq!(
            violations[0].anchors,
            vec![ModifierKeyword::Final, ModifierKeyword::Base]
        );
        // abstract final interface (:2996-3005).
        let mut abstract_final_interface = abstract_final_base;
        abstract_final_interface.is_base = false;
        let violations = declaration_violations(&abstract_final_interface);
        assert_eq!(violations.len(), 1);
        assert_eq!(
            violations[0].code,
            ModifierCode::AbstractFinalInterfaceClass
        );
    }

    #[test]
    fn sealed_restriction_has_no_platform_exemption() {
        // error_verifier.dart:7424-7437 — library equality only; the
        // may_ignore gate is deliberately absent.
        let cross = super_facts(class_mods(false, false, false, true), false, true);
        assert_eq!(
            sealed_supertype_violation(&cross),
            Some(ModifierCode::SealedClassSubtypeOutsideOfLibrary)
        );
        let local = super_facts(class_mods(false, false, false, true), true, false);
        assert_eq!(sealed_supertype_violation(&local), None);
        // The official checks `ClassElement.isSealed` — non-sealed or
        // induced-only carriers do not fire.
        assert_eq!(
            sealed_supertype_violation(&super_facts(
                class_mods(true, false, false, false),
                false,
                false
            )),
            None
        );
    }

    #[test]
    fn final_restriction_faces() {
        let cross = super_facts(class_mods(false, true, false, false), false, false);
        assert_eq!(
            final_extends_violation(&cross),
            Some(ModifierCode::FinalClassExtendedOutsideOfLibrary)
        );
        assert_eq!(
            final_mixin_constraint_violation(&cross),
            Some(ModifierCode::FinalClassUsedAsMixinConstraintOutsideOfLibrary)
        );
        // Gates: same library, platform exemption, sealed final.
        assert_eq!(
            final_extends_violation(&super_facts(
                class_mods(false, true, false, false),
                true,
                false
            )),
            None
        );
        assert_eq!(
            final_extends_violation(&super_facts(
                class_mods(false, true, false, false),
                false,
                true
            )),
            None
        );
        assert_eq!(
            final_extends_violation(&super_facts(
                class_mods(false, true, false, true),
                false,
                false
            )),
            None
        );
        // Implements face: the indirect skip (:5641-5648).
        let indirect = ImplementsStep {
            is_named_type: false,
            named_type_has_class_modifiers: true,
        };
        assert_eq!(final_implements_candidate(&cross, &indirect), None);
        let indirect_pre_feature = ImplementsStep {
            is_named_type: false,
            named_type_has_class_modifiers: false,
        };
        assert_eq!(
            final_implements_candidate(&cross, &indirect_pre_feature),
            Some(ModifierCode::FinalClassImplementedOutsideOfLibrary)
        );
        let direct = ImplementsStep {
            is_named_type: true,
            named_type_has_class_modifiers: true,
        };
        assert_eq!(
            final_implements_candidate(&cross, &direct),
            Some(ModifierCode::FinalClassImplementedOutsideOfLibrary)
        );
    }

    #[test]
    fn interface_extends_and_base_implements_faces() {
        let cross = super_facts(class_mods(false, false, true, false), false, false);
        assert_eq!(
            interface_extends_violation(&cross),
            Some(ModifierCode::InterfaceClassExtendedOutsideOfLibrary)
        );
        assert_eq!(
            interface_extends_violation(&super_facts(
                class_mods(false, false, true, false),
                true,
                false
            )),
            None
        );

        // base_implemented_candidate (:3712-3752): base class non-sealed
        // reports, sealed class falls to the sealed check, base mixin
        // reports, enums carry no base face.
        assert_eq!(
            base_implemented_candidate(&super_facts(
                class_mods(true, false, false, false),
                false,
                false
            )),
            Some(ModifierCode::BaseClassImplementedOutsideOfLibrary)
        );
        assert_eq!(
            base_implemented_candidate(&super_facts(
                class_mods(true, false, false, true),
                false,
                false
            )),
            None
        );
        let mut base_mixin = class_mods(true, false, false, false);
        base_mixin.kind = DeclKind::Mixin;
        assert_eq!(
            base_implemented_candidate(&super_facts(base_mixin, false, false)),
            Some(ModifierCode::BaseMixinImplementedOutsideOfLibrary)
        );
    }

    /// Fixed hierarchy for the ancestor walk: `Sub -> Mid -> Base`,
    /// `Sub` also implementing `IfaceBase`.
    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
    enum D {
        Sub,
        Mid,
        Base,
        IfaceBase,
        Plain,
    }

    struct Fixture {
        mods: FxHashMap<D, ClassModifiers>,
        supers: FxHashMap<D, Vec<D>>,
    }

    impl ModifierHierarchy for Fixture {
        type Decl = D;
        fn modifiers(&self, decl: D) -> ClassModifiers {
            self.mods[&decl]
        }
        fn supertypes(&self, decl: D) -> Vec<D> {
            self.supers.get(&decl).cloned().unwrap_or_default()
        }
    }

    #[test]
    fn nearest_explicit_base_or_final_walks_in_declaration_order() {
        let fixture = Fixture {
            mods: FxHashMap::from_iter([
                (D::Sub, class_mods(false, false, false, false)),
                (D::Mid, class_mods(false, false, false, false)),
                (D::Base, class_mods(true, false, false, false)),
                (D::IfaceBase, class_mods(false, true, false, false)),
                (D::Plain, class_mods(false, false, false, false)),
            ]),
            supers: FxHashMap::from_iter([
                // extends first, then implements — the official face order
                // (base_or_final_type_verifier.dart:115-124).
                (D::Sub, vec![D::IfaceBase, D::Mid]),
                (D::Mid, vec![D::Base]),
            ]),
        };
        // The start itself qualifies (:111-113).
        assert_eq!(
            nearest_explicit_base_or_final(&fixture, D::Base),
            Some(D::Base)
        );
        // Deep walk: Sub -> IfaceBase (final) wins over Mid's subtree.
        assert_eq!(
            nearest_explicit_base_or_final(&fixture, D::Sub),
            Some(D::IfaceBase)
        );
        // Mid reaches Base.
        assert_eq!(
            nearest_explicit_base_or_final(&fixture, D::Mid),
            Some(D::Base)
        );
        // No qualifier anywhere.
        assert_eq!(nearest_explicit_base_or_final(&fixture, D::Plain), None);
        // Sealed carriers do not qualify ((isBase||isFinal) && !isSealed,
        // :111).
        let mut sealed_fixture = Fixture {
            mods: fixture.mods.clone(),
            supers: fixture.supers.clone(),
        };
        sealed_fixture
            .mods
            .insert(D::Base, class_mods(true, false, false, true));
        assert_eq!(
            nearest_explicit_base_or_final(&sealed_fixture, D::Mid),
            None
        );
    }

    /// Cycle pin: a mutual-hierarchy cycle terminates and answers `None`
    /// when nothing in it qualifies — the explicit visited set (the
    /// cycle-safe adaptation noted on [`nearest_explicit_base_or_final`])
    /// stops the revisit, so an adversarial or erroneous cyclic hierarchy
    /// cannot hang the walk. A cycle member that does qualify is still
    /// found: the guard stops revisits, not the search.
    #[test]
    fn cyclic_hierarchy_terminates_and_answers() {
        let fixture = Fixture {
            mods: FxHashMap::from_iter([
                (D::Sub, class_mods(false, false, false, false)),
                (D::Mid, class_mods(false, false, false, false)),
            ]),
            supers: FxHashMap::from_iter([(D::Sub, vec![D::Mid]), (D::Mid, vec![D::Sub])]),
        };
        assert_eq!(nearest_explicit_base_or_final(&fixture, D::Sub), None);

        let mut qualifying = Fixture {
            mods: fixture.mods.clone(),
            supers: fixture.supers.clone(),
        };
        qualifying
            .mods
            .insert(D::Mid, class_mods(true, false, false, false));
        assert_eq!(
            nearest_explicit_base_or_final(&qualifying, D::Sub),
            Some(D::Mid)
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one scenario per official gate of report_restriction_error, asserted in gate order"
    )]
    fn subtype_restriction_family_matches_report_restriction_error() {
        let plain_element = class_mods(false, false, false, false);
        let base_super = class_mods(true, false, false, false);
        let final_super = class_mods(false, true, false, false);
        let ancestor_base = ExplicitBaseFinal {
            modifiers: base_super,
            library: USER,
            same_library: false,
        };
        let input = |element: ClassModifiers,
                     super_element: ClassModifiers,
                     ancestor: Option<ExplicitBaseFinal>,
                     via_implements: bool| SubtypeRestrictionInput {
            element,
            element_library: USER,
            super_element,
            super_library: USER,
            same_library: true,
            explicit_base_or_final: ancestor,
            via_implements,
        };

        // Gate 1: cross-library super elements are skipped (:185-187).
        let mut cross = input(plain_element, base_super, Some(ancestor_base), false);
        cross.same_library = false;
        assert_eq!(subtype_restriction_violation(&cross), None);

        // Gate 2: an unmodified super element triggers nothing (:200-202).
        assert_eq!(
            subtype_restriction_violation(&input(
                plain_element,
                class_mods(false, false, false, false),
                None,
                false
            )),
            None
        );

        // Base family (:295-311) and its mixin twin.
        assert_eq!(
            subtype_restriction_violation(&input(
                plain_element,
                base_super,
                Some(ancestor_base),
                false
            )),
            Some(ModifierCode::SubtypeOfBaseIsNotBaseFinalOrSealed)
        );
        let mut mixin_element = plain_element;
        mixin_element.kind = DeclKind::Mixin;
        assert_eq!(
            subtype_restriction_violation(&input(
                mixin_element,
                base_super,
                Some(ancestor_base),
                false
            )),
            Some(ModifierCode::MixinSubtypeOfBaseIsNotBase)
        );

        // Final family (:259-294). The reportable cross-library shape
        // (:262-279): the super library predates class modifiers, the
        // ancestor library is the SDK, and the check is not
        // implements-anchored — only then does the carve-out stay silent
        // and the error report.
        let ancestor_final_sdk = ExplicitBaseFinal {
            modifiers: final_super,
            library: SDK,
            same_library: false,
        };
        let mut reportable_final =
            input(plain_element, final_super, Some(ancestor_final_sdk), false);
        reportable_final.super_library = PRE_MODIFIERS;
        assert_eq!(
            subtype_restriction_violation(&reportable_final),
            Some(ModifierCode::SubtypeOfFinalIsNotBaseFinalOrSealed)
        );
        // A same-library ancestor skips the carve-out guard entirely
        // (:263 — the guard sits inside `ancestor.library != element.library`).
        let ancestor_final_local = ExplicitBaseFinal {
            modifiers: final_super,
            library: USER,
            same_library: true,
        };
        assert_eq!(
            subtype_restriction_violation(&input(
                plain_element,
                final_super,
                Some(ancestor_final_local),
                false
            )),
            Some(ModifierCode::SubtypeOfFinalIsNotBaseFinalOrSealed)
        );
        // Carve-out leg 1: cross-library ancestor under a
        // modifiers-enabled super library stays silent.
        let mut carve_out = input(plain_element, final_super, Some(ancestor_final_sdk), false);
        carve_out.super_library = USER;
        assert_eq!(subtype_restriction_violation(&carve_out), None);
        // Carve-out leg 2: a non-SDK ancestor library stays silent
        // (the `!baseOrFinalSuperElement.library.isInSdk` disjunct).
        let ancestor_final_user = ExplicitBaseFinal {
            modifiers: final_super,
            library: USER,
            same_library: false,
        };
        let mut carve_out_user =
            input(plain_element, final_super, Some(ancestor_final_user), false);
        carve_out_user.super_library = PRE_MODIFIERS;
        assert_eq!(subtype_restriction_violation(&carve_out_user), None);
        // Carve-out leg 3: the implements anchor stays silent
        // (the `implementsNamedType != null` disjunct).
        let mut carve_out_impl = input(plain_element, final_super, Some(ancestor_final_sdk), true);
        carve_out_impl.super_library = PRE_MODIFIERS;
        assert_eq!(subtype_restriction_violation(&carve_out_impl), None);

        // An element that is already base/final/sealed never reports
        // (:258).
        assert_eq!(
            subtype_restriction_violation(&input(
                class_mods(true, false, false, false),
                base_super,
                Some(ancestor_base),
                false
            )),
            None
        );

        // The implements-anchored base branch (:240-256): the super
        // element is sealed *and* carries the induced base flag — sealed
        // classes inherit `isBase` from a base supertype
        // (instance_member_inferrer.dart:679-680) — so the trigger
        // (:189-202) fires and the cross-library base ancestor reports.
        let sealed_base_super = class_mods(true, false, false, true);
        assert_eq!(
            subtype_restriction_violation(&input(
                plain_element,
                sealed_base_super,
                Some(ancestor_base),
                true
            )),
            Some(ModifierCode::BaseClassImplementedOutsideOfLibrary)
        );
        // Without the induced base flag the trigger never fires — that
        // shape is the direct error_verifier walk's (:3712-3752), not
        // this verifier's.
        assert_eq!(
            subtype_restriction_violation(&input(
                plain_element,
                class_mods(false, false, false, true),
                Some(ancestor_base),
                true
            )),
            None
        );

        // The platform exemption is measured against the ANCESTOR's
        // library (:164-166, :208-210): an SDK use site under an SDK
        // ancestor is exempt; a user-library ancestor never exempts.
        let ancestor_base_sdk = ExplicitBaseFinal {
            modifiers: base_super,
            library: SDK,
            same_library: false,
        };
        let mut sdk_use = input(plain_element, base_super, Some(ancestor_base_sdk), false);
        sdk_use.element_library = SDK;
        assert_eq!(subtype_restriction_violation(&sdk_use), None);
        let mut user_ancestor_sdk_use =
            input(plain_element, base_super, Some(ancestor_base), false);
        user_ancestor_sdk_use.element_library = SDK;
        assert_eq!(
            subtype_restriction_violation(&user_ancestor_sdk_use),
            Some(ModifierCode::SubtypeOfBaseIsNotBaseFinalOrSealed)
        );
    }

    #[test]
    fn induced_modifier_order_matches_the_inferrer() {
        // instance_member_inferrer.dart:649-719.
        let sealed = class_mods(false, false, false, true);
        let plain = class_mods(false, false, false, false);
        let base = class_mods(true, false, false, false);
        let final_ = class_mods(false, true, false, false);
        let interface_ = class_mods(false, false, true, false);

        // Non-sealed elements never carry induced flags (:651-653).
        assert_eq!(induced_modifier(&plain, Some(&base), &[], &[]), None);
        // `mixins.any(isFinal)` (:655-659) is unreachable from the element
        // model: visitMixinDeclaration records only `isBase` on mixin
        // fragments (element_builder.dart:1973-1980), and the verifier's
        // own kind gate agrees (base_or_final_type_verifier.dart:331-340).
        // The Mixin-kind value below carries an unrepresentable flag; the
        // gate ignores it and the walk falls to the base-supertype branch.
        let mut final_mixin = plain;
        final_mixin.kind = DeclKind::Mixin;
        final_mixin.is_final = true;
        assert_eq!(
            induced_modifier(&sealed, Some(&base), &[], &[final_mixin]),
            Some(InducedModifier::Base)
        );
        // Final supertype (:661-666).
        assert_eq!(
            induced_modifier(&sealed, Some(&final_), &[], &[]),
            Some(InducedModifier::Final)
        );
        // Base supertype + interface mixin → Final (:668-677); base alone
        // → Base (:679-680).
        assert_eq!(
            induced_modifier(&sealed, Some(&base), &[], &[interface_]),
            Some(InducedModifier::Final)
        );
        assert_eq!(
            induced_modifier(&sealed, Some(&base), &[], &[]),
            Some(InducedModifier::Base)
        );
        // Interface supertype: base interfaces → Final, else Interface
        // (:682-697).
        assert_eq!(
            induced_modifier(&sealed, Some(&interface_), &[base], &[]),
            Some(InducedModifier::Final)
        );
        assert_eq!(
            induced_modifier(&sealed, Some(&interface_), &[], &[]),
            Some(InducedModifier::Interface)
        );
        // No supertype: base/final interfaces → Base (:700-706); interface
        // mixin → Interface (:717-718).
        assert_eq!(
            induced_modifier(&sealed, None, &[base], &[]),
            Some(InducedModifier::Base)
        );
        assert_eq!(
            induced_modifier(&sealed, None, &[], &[interface_]),
            Some(InducedModifier::Interface)
        );
        // Nothing qualifies.
        assert_eq!(induced_modifier(&sealed, None, &[plain], &[plain]), None);
    }

    /// Every [`ModifierCode`], in declaration order.
    const ALL_CODES: [ModifierCode; 24] = [
        ModifierCode::SealedClassSubtypeOutsideOfLibrary,
        ModifierCode::FinalClassExtendedOutsideOfLibrary,
        ModifierCode::FinalClassImplementedOutsideOfLibrary,
        ModifierCode::FinalClassUsedAsMixinConstraintOutsideOfLibrary,
        ModifierCode::InterfaceClassExtendedOutsideOfLibrary,
        ModifierCode::BaseClassImplementedOutsideOfLibrary,
        ModifierCode::BaseMixinImplementedOutsideOfLibrary,
        ModifierCode::SubtypeOfBaseIsNotBaseFinalOrSealed,
        ModifierCode::SubtypeOfFinalIsNotBaseFinalOrSealed,
        ModifierCode::MixinSubtypeOfBaseIsNotBase,
        ModifierCode::MixinSubtypeOfFinalIsNotBase,
        ModifierCode::AbstractSealedClass,
        ModifierCode::AbstractFinalBaseClass,
        ModifierCode::AbstractFinalInterfaceClass,
        ModifierCode::BaseEnum,
        ModifierCode::FinalEnum,
        ModifierCode::InterfaceEnum,
        ModifierCode::SealedEnum,
        ModifierCode::FinalMixin,
        ModifierCode::InterfaceMixin,
        ModifierCode::SealedMixin,
        ModifierCode::FinalMixinClass,
        ModifierCode::InterfaceMixinClass,
        ModifierCode::SealedMixinClass,
    ];
    /// The outside-library family — one machine name,
    /// `INVALID_USE_OF_TYPE_OUTSIDE_LIBRARY`.
    const OUTSIDE_LIBRARY_CODES: [ModifierCode; 7] = [
        ModifierCode::SealedClassSubtypeOutsideOfLibrary,
        ModifierCode::FinalClassExtendedOutsideOfLibrary,
        ModifierCode::FinalClassImplementedOutsideOfLibrary,
        ModifierCode::FinalClassUsedAsMixinConstraintOutsideOfLibrary,
        ModifierCode::InterfaceClassExtendedOutsideOfLibrary,
        ModifierCode::BaseClassImplementedOutsideOfLibrary,
        ModifierCode::BaseMixinImplementedOutsideOfLibrary,
    ];
    /// The subtype family — one machine name,
    /// `SUBTYPE_OF_BASE_OR_FINAL_IS_NOT_BASE_FINAL_OR_SEALED`.
    const SUBTYPE_CODES: [ModifierCode; 4] = [
        ModifierCode::SubtypeOfBaseIsNotBaseFinalOrSealed,
        ModifierCode::SubtypeOfFinalIsNotBaseFinalOrSealed,
        ModifierCode::MixinSubtypeOfBaseIsNotBase,
        ModifierCode::MixinSubtypeOfFinalIsNotBase,
    ];

    #[test]
    fn code_table_rows_are_registry_true() {
        // Every row: machine == name uppercased, message non-empty, and
        // the family split matches the registry — the use-site and
        // subtype families COMPILE_TIME_ERROR + published, the
        // declaration-site rows SYNTACTIC_ERROR with only
        // abstract_sealed_class published.
        for code in ALL_CODES {
            let info = code.info();
            assert_eq!(
                info.name.to_uppercase(),
                info.machine,
                "{}",
                info.unique_name
            );
            assert!(!info.message.is_empty(), "{}", info.unique_name);
            let use_site_or_subtype =
                OUTSIDE_LIBRARY_CODES.contains(&code) || SUBTYPE_CODES.contains(&code);
            if use_site_or_subtype {
                assert_eq!(
                    info.dtype,
                    DiagnosticType::CompileTimeError,
                    "{}",
                    info.unique_name
                );
                assert!(info.has_published, "{}", info.unique_name);
                assert_eq!(info.correction, None, "{}", info.unique_name);
            } else {
                assert_eq!(
                    info.dtype,
                    DiagnosticType::SyntacticError,
                    "{}",
                    info.unique_name
                );
                assert_eq!(
                    info.has_published,
                    code == ModifierCode::AbstractSealedClass,
                    "{}",
                    info.unique_name
                );
            }
        }
        // The shared machine names: the whole outside-library family plus
        // sealed_class_subtype_outside_of_library ride
        // invalid_use_of_type_outside_library; the subtype family rides
        // subtype_of_base_or_final_is_not_base_final_or_sealed.
        for code in OUTSIDE_LIBRARY_CODES {
            assert_eq!(code.info().name, "invalid_use_of_type_outside_library");
        }
        for code in SUBTYPE_CODES {
            assert_eq!(
                code.info().name,
                "subtype_of_base_or_final_is_not_base_final_or_sealed"
            );
        }
        // Spot-check two verbatim messages (registry :15536 and :15818).
        assert_eq!(
            ModifierCode::SealedClassSubtypeOutsideOfLibrary
                .info()
                .message,
            "The class '{0}' can't be extended, implemented, or mixed in outside of its library because it's a sealed class."
        );
        assert_eq!(
            ModifierCode::SubtypeOfBaseIsNotBaseFinalOrSealed
                .info()
                .message,
            "The type '{0}' must be 'base', 'final' or 'sealed' because the supertype '{1}' is 'base'."
        );
    }
}
