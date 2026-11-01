//! The augmentation verifier (#2784): one fold-time pass over the unit's
//! `augment` declarations, each resolved against the library's declaration
//! space and checked against the fragment-merge semantics of the pinned
//! SDK (upstream Dart SDK source, pkg/analyzer — the merge rule in
//! `lib/src/dart/element/element_builder.dart` and the checks in
//! `lib/src/generated/error_verifier.dart` `_checkAugmentation*` /
//! `_checkForAugmentation*`, with the `messages.yaml` `augmentation*`
//! templates verbatim in [`SemanticCode::message`]).
//!
//! Resolution mirrors the upstream fragment chain: an `augment` merges
//! with the chain when the LAST same-key declaration has its kind and is
//! not a named mixin application; otherwise the merge refuses and the
//! without-declaration family reports (mixin-application rows first, then
//! different-kind, then the plain row). A merged chain resolves to the
//! FIRST declaration of the trailing same-kind run — the introductory
//! declaration the header/return/formal checks read, even when it is
//! itself an augmentation (header checks run against it; the executable
//! body checks bail — upstream `firstFragment.isAugmentation`).
//!
//! Visibility is the application order (upstream `ElementBuilder` links
//! `previousFragment` in `allUnits` walk order — defining unit then each
//! part): an augment sees only the fragments of units before its own
//! unit in [`LibraryGraph::unit_files`], plus its own unit's preceding
//! fragments. Within the diagnosing unit, augmentations resolve in source
//! order; introductory declarations are all visible because the model
//! keeps no per-declaration source offset, so the same-unit
//! augment-before-intro shape is not refused (an orphan would also have
//! to introduce its name — both arms need the offsets the model lacks).
//!
//! Model limitations, each silent or near-silent on the oracle sets:
//! unnamed introductory formals do not join the formal tables (the
//! collector's `capture_name` drops them — a pre-existing capture shape
//! shared with the override faces), a `const` variable introduction is
//! not distinguished from a plain one (the const-variable augmentation
//! gate under-reports), an `augment`-spelled named mixin application
//! keeps the plain-class merge rule (the augment side of
//! `is_mixin_application` is not captured), and the same-unit
//! augment-before-intro ordering is not applied.

use dartq_diagnostics::{Diagnostic, Severity};
use dartq_resolution::{LibraryGraph, LibraryId};
use dartq_source::{FileId, SourceRange, StringId, StringTable};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::codes::{AlreadyCompleteShape, ReturnShape, SemanticCode, WithoutDeclarationShape};
use crate::library::ElementIndex;
use crate::model::{
    AugKind, AugmentDecl, AugmentFormal, Container, ContainerKind, FileElements, Formal, Formals,
    MemberKind, Ty,
};

/// The variable family — a `Field`/`TopLevelVar` declaration and the
/// getter/setter accessors its name space shares (upstream one
/// `lastFragments[name]` slot across the accessor kinds, with the resolved
/// element holding three independent fragment chains: the variable's own,
/// its induced/written getter's, and its induced/written setter's —
/// `_fieldElement(lastFragment)` resolves the element from a fragment of
/// ANY accessor kind).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Accessor {
    /// The variable chain itself (`Field`/`TopLevelVar` fragments).
    Variable,
    /// The getter chain (`Getter` fragments plus every variable fragment's
    /// induced getter).
    Getter,
    /// The setter chain (`Setter` fragments plus every non-final variable
    /// fragment's induced setter).
    Setter,
}

/// One library-space declaration the target lookup resolves: the identity
/// facts of the merge rule plus the pointer the compare-time face reads.
struct Entry {
    unit: FileId,
    kind: AugKind,
    /// An `augment` declaration (chainable but never an introductory
    /// target; when it is the chain's first entry the executable checks
    /// bail — upstream `firstFragment.isAugmentation`).
    is_augment: bool,
    /// Index into the declaring unit's `augmentations` (valid when
    /// `is_augment`).
    aug_index: usize,
    /// The declaration wrote an `extends` clause (a container intro with a
    /// superclass, or a container augmentation with its own clause — the
    /// already-present check reads any preceding fragment).
    extends_present: bool,
    /// A named mixin application: every augment of the name reports
    /// `augmentation_of_mixin_application_class`.
    is_mixin_application: bool,
    /// A `final` variable: induces no setter.
    is_final: bool,
    /// A `const` generative constructor or a `const` factory.
    is_const: bool,
    /// A factory head.
    is_factory: bool,
    /// The fragment's completeness bit (upstream `*FragmentImpl.isComplete`)
    /// — the `declaration_already_complete` family's
    /// `nearestPrecedingCompleteFragment` walk. Executable:
    /// `external || nonEmptyBody`; constructor adds initializers/redirects/
    /// initializing formals; variable-induced accessor: `external ||
    /// !abstract`. Non-executable kinds (containers) never participate.
    is_complete: bool,
}

impl Entry {
    /// The variable family: this fragment's sub-chain membership (upstream
    /// `element_builder.dart` — a `Field`/`TopLevelVar` fragment appends an
    /// induced getter (always) and an induced setter (non-final) alongside
    /// its own variable-chain slot; a `Getter`/`Setter`-only fragment backs
    /// onto a synthetic `isOriginGetterSetter` field in the variable chain
    /// and appends only to its own accessor chain).
    fn in_accessor(&self, accessor: Accessor) -> bool {
        match accessor {
            // Every accessor-family fragment carries a variable-chain slot:
            // a variable fragment by itself, an accessor-only fragment by
            // its synthetic backing field.
            Accessor::Variable => {
                matches!(
                    self.kind,
                    AugKind::Field | AugKind::TopLevelVar | AugKind::Getter | AugKind::Setter
                )
            }
            Accessor::Getter => {
                matches!(
                    self.kind,
                    AugKind::Getter | AugKind::Field | AugKind::TopLevelVar
                )
            }
            Accessor::Setter => {
                self.kind == AugKind::Setter
                    || (matches!(self.kind, AugKind::Field | AugKind::TopLevelVar)
                        && !self.is_final)
            }
        }
    }
}

/// The compare-time face of a resolved introductory declaration.
enum Face<'e> {
    /// A container intro: modifiers, extends, type parameters.
    Container(&'e Container),
    /// A class-member intro.
    Member(&'e crate::model::MemberEntry),
    /// A top-level function/getter/setter intro (`is_setter`: the unwritten
    /// return type spells `void`, not `dynamic`).
    TopFn {
        decl: &'e crate::model::TopDecl,
        is_setter: bool,
    },
    /// A top-level variable intro: the induced getter's return type is the
    /// variable's own type.
    TopVar(&'e crate::model::TopDecl),
    /// A constructor/factory intro: the formals live in the owning
    /// container's per-selector tables, the const/factory bits in the
    /// chain entry.
    Ctor {
        container: &'e Container,
        name: StringId,
        is_factory: bool,
    },
}

impl Face<'_> {
    /// The intro's formal table (`None` = the capture carries no group —
    /// upstream answers the same shape with an empty parameter list).
    fn formals(&self) -> Option<&Formals> {
        match self {
            Face::Member(m) => m.formals.as_deref(),
            Face::TopFn { decl, .. } | Face::TopVar(decl) => decl.formals.as_deref(),
            Face::Ctor {
                container,
                name,
                is_factory,
            } => {
                let table = if *is_factory {
                    &container.factory_formals
                } else {
                    &container.ctor_formals
                };
                table.get(name).map(|f| f.as_ref())
            }
            Face::Container(_) => None,
        }
    }

    /// The intro's written return face: `(ty, is_setter)`. `Ty::Unknown`
    /// means unwritten — the oracle's element type is `dynamic`, except a
    /// setter's, which is `void` even unwritten. A field (or a top-level
    /// variable) answers its own type: the induced getter's return.
    fn return_face(&self) -> (Ty, bool) {
        match self {
            Face::Member(m) => match m.kind {
                MemberKind::Field => (m.var_ty, false),
                MemberKind::Setter => (m.return_ty, true),
                _ => (m.return_ty, false),
            },
            Face::TopFn { decl, is_setter } => (decl.return_ty, *is_setter),
            Face::TopVar(decl) => (decl.var_ty, false),
            Face::Container(_) | Face::Ctor { .. } => (Ty::UNKNOWN, false),
        }
    }

    /// The intro's own type parameters: parallel name/bound rows, an
    /// absent bound spelled `Ty::Dynamic` (the Dart reading).
    fn type_params(&self) -> (&[StringId], &[Ty]) {
        match self {
            Face::Container(c) => (&c.type_param_names, &c.type_param_bounds),
            Face::Member(m) => (&m.type_param_names, &m.type_param_bounds),
            Face::TopFn { decl, .. } | Face::TopVar(decl) => (&decl.type_param_names, &[]),
            Face::Ctor { .. } => (&[], &[]),
        }
    }

    /// The intro's header modifier bits, upstream check order for a class:
    /// abstract, base, final, interface, sealed, mixin-class.
    fn header_modifiers(&self) -> [bool; 6] {
        match self {
            Face::Container(c) => [
                c.is_abstract,
                c.is_base,
                c.is_final,
                c.is_interface,
                c.sealed,
                c.is_mixin_class,
            ],
            _ => [false; 6],
        }
    }
}

/// The declaration space a lookup key resolves in. Upstream keeps THREE
/// `lastFragments` maps, each keyed by name and covering every kind that
/// lands in it — a method and a getter under one name collide, and the
/// different-kind refusal reads that single slot's last fragment:
/// `_buildTopFragments` shares one map across every top-level declaration
/// (container heads and functions/variables/accessors alike), and
/// `_buildInstanceElementMembers` splits container members into an
/// instance map and a static map — constructors landing in the static one
/// (`ConstructorFragmentImpl => true`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Space {
    /// Instance members (upstream `_instanceNamespace`).
    Member,
    /// Static members AND constructors (upstream `_staticNamespace` —
    /// `ConstructorFragmentImpl` lands there, so a `static m` and a `C.m`
    /// collide under one name).
    StaticMember,
    /// Top-level declarations: container heads, functions, accessors, and
    /// variables all share the one name slot.
    Top,
}

impl Space {
    /// The slot an `augment` declaration resolves into. A container
    /// augmentation targets the top-level name it spells (`augment class C`
    /// keys under C alongside every other top-level `C`); a member
    /// augmentation resolves under its owner by static/instance.
    fn of(aug: &AugmentDecl) -> Space {
        match aug.kind {
            _ if aug.container.is_some() => {
                if aug.is_static || aug.kind == AugKind::Constructor {
                    Space::StaticMember
                } else {
                    Space::Member
                }
            }
            _ => Space::Top,
        }
    }
}

/// The property name a variable-family slot keys under: a setter's
/// `name=` lookup spelling folds to the property's plain name (upstream
/// `lastFragments[name]` keys the element's name, not the declaration
/// spelling — `summary2/library_builder.dart` spells setters `name=` only
/// for the declaration map). Every other kind keeps its spelling.
fn property_name(name: StringId, kind: AugKind, strings: &StringTable) -> StringId {
    if kind != AugKind::Setter {
        return name;
    }
    let spelling = strings.resolve(name);
    match spelling.strip_suffix('=') {
        Some(property) => strings.intern(property),
        None => name,
    }
}

/// The table key of an `augment` declaration: the container it resolves
/// under, its property name, and its family's space.
fn augment_key(aug: &AugmentDecl, strings: &StringTable) -> (Option<StringId>, StringId, Space) {
    let container = match Space::of(aug) {
        Space::Member | Space::StaticMember => aug.container,
        Space::Top => None,
    };
    (
        container,
        property_name(aug.name, aug.kind, strings),
        Space::of(aug),
    )
}

/// The verifier's rows: one pass per diagnosing file, the table built once
/// per call (only files carrying augmentations pay for it).
pub(super) fn augmentation_diagnostics(
    file: FileId,
    own: &FileElements,
    graph: &LibraryGraph,
    index: &ElementIndex,
    strings: &StringTable,
    lib: LibraryId,
    augmentations_enabled: bool,
) -> Vec<Diagnostic> {
    // `_checkForFactoryBodyCompleteness` (error_verifier.dart:5078-5107)
    // runs for every factory constructor of the unit, augmentation chains or
    // none — and only with the `augmentations` experiment on (`:5079-5081`).
    let mut out = if augmentations_enabled {
        factory_body_diagnostics(file, own, index, graph, lib, strings)
    } else {
        Vec::new()
    };
    if own.augmentations().is_empty() {
        return out;
    }
    // The application order (upstream `ElementBuilder` walks
    // `RuleContext.allUnits` — the defining unit, then each part in
    // declaration order — and links `previousFragment` within that walk):
    // an augment sees only the fragments of units at or before its own
    // unit's position. `unit_files` answers that same order
    // (library_graph.rs `unit_files_answer_defining_then_parts_per_library`);
    // a diagnosing file outside the library's unit list keeps every entry
    // visible — the pre-ordering behavior, which cannot arise in practice
    // since the caller iterates the library's units.
    let units = graph.unit_files(lib);
    let own_pos = units
        .iter()
        .position(|&unit| unit == file)
        .unwrap_or(units.len());
    let unit_pos: FxHashMap<FileId, usize> = units
        .iter()
        .enumerate()
        .map(|(pos, &unit)| (unit, pos))
        .collect();
    let mut table: FxHashMap<(Option<StringId>, StringId, Space), Vec<Entry>> =
        FxHashMap::default();
    for &unit in units {
        if let Some(elements) = index.get(unit) {
            populate(&mut table, graph, index, lib, unit, elements, strings);
        }
    }
    for (aug_index, aug) in own.augmentations().iter().enumerate() {
        // A type alias augmentation never merges: the parser reports the
        // shape outright (upstream `parser_impl.dart` reports
        // `typedefAugmentation` at the `augment` keyword) and this face
        // adds nothing to that row.
        if aug.kind == AugKind::TypeAlias {
            continue;
        }
        let key = augment_key(aug, strings);
        // The chain visible to this augment: every fragment of an earlier
        // unit, plus this unit's own preceding fragments — augmentations in
        // source order (the `aug_index` walk), introductory declarations
        // unconditionally (the model carries no per-declaration source
        // offset, so the same-unit augment-before-intro shape stays
        // unfiltered — see the module note).
        let chain: Vec<&Entry> = table
            .get(&key)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .filter(|entry| {
                if entry.unit == file {
                    !entry.is_augment || entry.aug_index < aug_index
                } else {
                    unit_pos.get(&entry.unit).is_some_and(|&pos| pos < own_pos)
                }
            })
            .collect();
        let variable_family = matches!(
            aug.kind,
            AugKind::Field | AugKind::TopLevelVar | AugKind::Getter | AugKind::Setter
        );
        if variable_family {
            process_variable(&mut out, file, aug, &chain, own, index, strings);
        } else {
            // The merge rule: the last fragment's kind must match and must
            // not be a named mixin application.
            let merged = matches!(
                chain.last(),
                Some(last) if !last.is_mixin_application && last.kind == aug.kind
            );
            if !merged {
                without_declaration(&mut out, file, aug, &chain);
            } else {
                // The trailing same-kind run: its first entry is the
                // introductory declaration (an earlier different-kind
                // declaration started a new element upstream and never
                // joins this one's fragment list).
                let run_start = chain
                    .iter()
                    .rposition(|entry| entry.kind != aug.kind)
                    .map_or(0, |i| i + 1);
                let run = &chain[run_start..];
                let intro = run[0];
                let header_kind = matches!(
                    aug.kind,
                    AugKind::Class
                        | AugKind::Mixin
                        | AugKind::Enum
                        | AugKind::Extension
                        | AugKind::ExtensionType
                );
                if header_kind {
                    check_header(&mut out, file, own, index, strings, aug, intro, run);
                } else {
                    // The already-complete check runs against the chain's
                    // completeness state before the shape checks (upstream
                    // `visitConstructorDeclaration` /
                    // `_checkForFunctionAlreadyComplete`).
                    check_already_complete(&mut out, file, aug, run);
                    if !intro.is_augment {
                        check_member_shape(&mut out, file, own, index, strings, aug, intro);
                    }
                }
                // An augmentation-only executable chain: the body checks
                // bail (upstream `firstExecutableFragment.isAugmentation`).
            }
        }
        // This augmentation joins the chain for the ones that follow it.
        table.entry(key).or_default().push(Entry {
            unit: file,
            kind: aug.kind,
            is_augment: true,
            aug_index,
            extends_present: aug.extends_kw.is_some(),
            is_mixin_application: false,
            is_final: aug.is_final,
            is_const: aug.const_kw.is_some(),
            is_factory: aug.factory_kw.is_some(),
            is_complete: aug.is_complete,
        });
    }
    out
}

/// The `declaration_already_complete` family on the executable chains
/// (constructor / function / method / getter / setter): when the `augment`
/// fragment is itself complete and a preceding fragment in the same chain
/// is complete, the check reports at the `augment` keyword (upstream
/// `nearestPrecedingCompleteFragment` over `precedingFragments`). The
/// variable family's induced accessors take the dedicated walk in
/// [`process_variable`].
fn check_already_complete(
    out: &mut Vec<Diagnostic>,
    file: FileId,
    aug: &AugmentDecl,
    run: &[&Entry],
) {
    if !aug.is_complete {
        return;
    }
    let shape = match aug.kind {
        AugKind::Constructor => AlreadyCompleteShape::Constructor,
        AugKind::Function | AugKind::Method | AugKind::Getter | AugKind::Setter => {
            AlreadyCompleteShape::Function
        }
        _ => return,
    };
    if run.iter().any(|entry| entry.is_complete) {
        let code = SemanticCode::AugmentationAlreadyComplete(shape);
        out.push(row(file, aug.augment_kw, code, code.message("", "")));
    }
}

/// The variable-family merge and checks (upstream `visitFieldDeclaration` /
/// `visitTopLevelVariableDeclaration` and the `_handleInstance*Accessor`
/// handlers). The unified name slot's `lastFragment` resolves the variable
/// element via `_fieldElement` — only accessor-family fragments resolve;
/// a method/function tail means the element is absent and the augment
/// reports `augmentation_of_different_declaration_kind`. The element's
/// live sub-chains are the accessor-family fragments since the last
/// non-accessor fragment (a method/function claiming the name closes the
/// previous element generation).
fn process_variable(
    out: &mut Vec<Diagnostic>,
    file: FileId,
    aug: &AugmentDecl,
    chain: &[&Entry],
    own: &FileElements,
    index: &ElementIndex,
    strings: &StringTable,
) {
    let accessor_family = |kind: AugKind| {
        matches!(
            kind,
            AugKind::Field | AugKind::TopLevelVar | AugKind::Getter | AugKind::Setter
        )
    };
    // `_fieldElement(lastFragment)`: a non-accessor tail resolves no
    // variable element — the augment faces `previousFragmentOfDifferentKind`.
    let Some(last) = chain.last() else {
        without_declaration(out, file, aug, chain);
        return;
    };
    if !accessor_family(last.kind) || last.is_mixin_application {
        without_declaration(out, file, aug, chain);
        return;
    }
    // The live element generation: the accessor-family run ending at the
    // name's last fragment (a method/function claiming the name closes the
    // previous element generation).
    let gen_start = chain
        .iter()
        .rposition(|entry| !accessor_family(entry.kind))
        .map_or(0, |i| i + 1);
    let generation = &chain[gen_start..];
    // The sub-chains the augment merges into and (for a variable augment)
    // induces.
    let target = match aug.kind {
        AugKind::Getter => Accessor::Getter,
        AugKind::Setter => Accessor::Setter,
        _ => Accessor::Variable,
    };
    let sub: Vec<&Entry> = generation
        .iter()
        .copied()
        .filter(|entry| entry.in_accessor(target))
        .collect();
    // The merge: the resolved sub-chain must be non-empty — a getter
    // augment needs a preceding getter, a setter augment a preceding
    // setter, a variable augment a preceding variable-chain tail.
    if sub.is_empty() {
        without_declaration(out, file, aug, generation);
        return;
    }
    let intro = sub[0];
    if matches!(aug.kind, AugKind::Field | AugKind::TopLevelVar) {
        // A variable augment merges onto the variable chain AND induces
        // each accessor sub-chain independently (upstream the
        // `_checkForAugmentationInducedAccessors*` pass): the induced
        // getter's return comparison at the variable's name, the induced
        // setter's presence row when the resolved setter chain is empty,
        // and each induced accessor's already-complete row.
        check_variable_augment(out, file, aug, generation, own, index, strings);
    } else {
        // A getter/setter augment merges onto its accessor sub-chain: the
        // already-complete row (the augment fragment complete against a
        // complete predecessor) then the executable pair checks.
        if aug.is_complete && sub.iter().any(|entry| entry.is_complete) {
            let code = SemanticCode::AugmentationAlreadyComplete(AlreadyCompleteShape::Function);
            out.push(row(file, aug.augment_kw, code, code.message("", "")));
        }
        if let Some(intro_elements) = index.get(intro.unit)
            && !intro.is_augment
            && let Some(face) = intro_face(intro_elements, intro, aug)
        {
            let (intro_names, intro_bounds) = face.type_params();
            check_type_param_rows(
                out,
                file,
                own,
                intro_elements,
                strings,
                aug,
                intro_names,
                intro_bounds,
            );
            if let Some(span) = aug.return_ty_range {
                check_return(
                    out,
                    file,
                    own,
                    intro_elements,
                    strings,
                    aug,
                    &face,
                    false,
                    span,
                );
            }
            check_formals(out, file, own, intro_elements, strings, aug, face.formals());
        }
    }
}

/// A `Field`/`TopLevelVar` augmentation's induced-accessor pass (upstream
/// `_checkAugmentationWithoutDeclarationForInducedAccessors` and
/// `_checkForAugmentationInducedAccessorsAlreadyComplete` over the merged
/// variable fragment): the absent-induced-setter row first, then each
/// induced accessor's already-complete row, then the induced getter's
/// return-type comparison at the variable's name.
fn check_variable_augment(
    out: &mut Vec<Diagnostic>,
    file: FileId,
    aug: &AugmentDecl,
    generation: &[&Entry],
    own: &FileElements,
    index: &ElementIndex,
    strings: &StringTable,
) {
    let getter_chain: Vec<&Entry> = generation
        .iter()
        .copied()
        .filter(|entry| entry.in_accessor(Accessor::Getter))
        .collect();
    let setter_chain: Vec<&Entry> = generation
        .iter()
        .copied()
        .filter(|entry| entry.in_accessor(Accessor::Setter))
        .collect();
    // The induced-accessor presence rows (upstream
    // `_checkAugmentationWithoutDeclarationForInducedAccessors`): the
    // augment field always induces a getter, a non-final one a setter —
    // each induced accessor whose chain has no preceding fragment reports
    // its `augmentationWithout{Getter,Setter}Declaration` variant at the
    // variable's name.
    if getter_chain.is_empty() {
        let code = SemanticCode::AugmentationWithoutDeclaration(WithoutDeclarationShape::Getter);
        let message = code.message(strings.resolve(aug.display_name), "");
        out.push(row(file, aug.name_range, code, message));
    }
    if !aug.is_final && setter_chain.is_empty() {
        let code = SemanticCode::AugmentationWithoutDeclaration(WithoutDeclarationShape::Setter);
        let message = code.message(strings.resolve(aug.display_name), "");
        out.push(row(file, aug.name_range, code, message));
    }
    // The already-complete rows: each induced accessor the augment carries
    // (a non-final variable carries both) reports when both sides are
    // complete — the augment's `is_complete` is the induced accessors'
    // `external || !abstract`.
    if aug.is_complete {
        if getter_chain.iter().any(|entry| entry.is_complete) {
            let code =
                SemanticCode::AugmentationAlreadyComplete(AlreadyCompleteShape::InducedGetter);
            out.push(row(file, aug.name_range, code, code.message("", "")));
        }
        if !aug.is_final && setter_chain.iter().any(|entry| entry.is_complete) {
            let code =
                SemanticCode::AugmentationAlreadyComplete(AlreadyCompleteShape::InducedSetter);
            out.push(row(file, aug.name_range, code, code.message("", "")));
        }
    }
    // The induced getter's return-type comparison at the variable's name
    // (upstream `errorEntity: variable.name`) — against the getter chain's
    // introductory face.
    if let Some(intro) = getter_chain.first().copied()
        && let Some(intro_elements) = index.get(intro.unit)
        && !intro.is_augment
        && let Some(face) = intro_face(intro_elements, intro, aug)
    {
        check_return(
            out,
            file,
            own,
            intro_elements,
            strings,
            aug,
            &face,
            true,
            aug.name_range,
        );
    }
}

/// The container-family header checks (upstream `visitClassDeclaration`
/// order: type parameters, extends, modifiers). With an introductory
/// declaration the comparisons read its captured rows; with an
/// augmentation-only chain they read the first fragment's own augmentation
/// capture (upstream reads `firstFragment` even when it is an
/// augmentation).
fn check_header(
    out: &mut Vec<Diagnostic>,
    file: FileId,
    own: &FileElements,
    index: &ElementIndex,
    strings: &StringTable,
    aug: &AugmentDecl,
    intro: &Entry,
    run: &[&Entry],
) {
    let Some(intro_elements) = index.get(intro.unit) else {
        return;
    };
    if intro.is_augment {
        let Some(intro_aug) = intro_elements.augmentations().get(intro.aug_index) else {
            return;
        };
        let intro_names: Vec<StringId> = intro_aug.type_params.iter().map(|p| p.name).collect();
        let intro_bounds: Vec<Ty> = intro_aug
            .type_params
            .iter()
            .map(|p| p.bound.map_or(Ty::Dynamic, |(ty, _)| ty))
            .collect();
        check_type_param_rows(
            out,
            file,
            own,
            intro_elements,
            strings,
            aug,
            &intro_names,
            &intro_bounds,
        );
        check_extends(out, file, aug, run);
        let checks: Vec<(&str, Option<SourceRange>, bool)> = match aug.kind {
            AugKind::Class => {
                let i = &intro_aug.modifiers;
                vec![
                    (
                        "abstract",
                        aug.modifiers.abstract_kw,
                        i.abstract_kw.is_some(),
                    ),
                    ("base", aug.modifiers.base_kw, i.base_kw.is_some()),
                    ("final", aug.modifiers.final_kw, i.final_kw.is_some()),
                    (
                        "interface",
                        aug.modifiers.interface_kw,
                        i.interface_kw.is_some(),
                    ),
                    ("sealed", aug.modifiers.sealed_kw, i.sealed_kw.is_some()),
                    ("mixin", aug.modifiers.mixin_kw, i.mixin_kw.is_some()),
                ]
            }
            AugKind::Mixin => {
                vec![(
                    "base",
                    aug.modifiers.base_kw,
                    intro_aug.modifiers.base_kw.is_some(),
                )]
            }
            _ => Vec::new(),
        };
        check_modifiers(out, file, aug, &checks);
    } else if let Some(face) = intro_face(intro_elements, intro, aug) {
        let (intro_names, intro_bounds) = face.type_params();
        check_type_param_rows(
            out,
            file,
            own,
            intro_elements,
            strings,
            aug,
            intro_names,
            intro_bounds,
        );
        check_extends(out, file, aug, run);
        let intro_bits = face.header_modifiers();
        let checks: Vec<(&str, Option<SourceRange>, bool)> = match aug.kind {
            AugKind::Class => {
                let m = &aug.modifiers;
                vec![
                    ("abstract", m.abstract_kw, intro_bits[0]),
                    ("base", m.base_kw, intro_bits[1]),
                    ("final", m.final_kw, intro_bits[2]),
                    ("interface", m.interface_kw, intro_bits[3]),
                    ("sealed", m.sealed_kw, intro_bits[4]),
                    ("mixin", m.mixin_kw, intro_bits[5]),
                ]
            }
            AugKind::Mixin => {
                vec![("base", aug.modifiers.base_kw, intro_bits[1])]
            }
            _ => Vec::new(),
        };
        check_modifiers(out, file, aug, &checks);
    }
}

/// The executable/variable/constructor checks against an introductory
/// declaration (upstream per-kind visit order; every body check bails when
/// the chain's first fragment is itself an augmentation).
fn check_member_shape(
    out: &mut Vec<Diagnostic>,
    file: FileId,
    own: &FileElements,
    index: &ElementIndex,
    strings: &StringTable,
    aug: &AugmentDecl,
    intro: &Entry,
) {
    let Some(intro_elements) = index.get(intro.unit) else {
        return;
    };
    let Some(face) = intro_face(intro_elements, intro, aug) else {
        return;
    };
    match aug.kind {
        AugKind::Constructor => {
            let checks = [
                ("const", aug.const_kw, intro.is_const),
                ("factory", aug.factory_kw, intro.is_factory),
            ];
            check_modifiers(out, file, aug, &checks);
            check_formals(out, file, own, intro_elements, strings, aug, face.formals());
        }
        AugKind::Function | AugKind::Method => {
            let (intro_names, intro_bounds) = face.type_params();
            check_type_param_rows(
                out,
                file,
                own,
                intro_elements,
                strings,
                aug,
                intro_names,
                intro_bounds,
            );
            if let Some(span) = aug.return_ty_range {
                check_return(
                    out,
                    file,
                    own,
                    intro_elements,
                    strings,
                    aug,
                    &face,
                    false,
                    span,
                );
            }
            check_formals(out, file, own, intro_elements, strings, aug, face.formals());
        }
        _ => {}
    }
}

/// The library-space table: containers (with their members), the
/// target-kind rows (top-level executables, variables, constructors,
/// typedefs), then every unit's own augmentations (chainable rows). Intros
/// precede this unit's augments — the shape every well-formed fixture and
/// real library carries.
fn populate(
    table: &mut FxHashMap<(Option<StringId>, StringId, Space), Vec<Entry>>,
    graph: &LibraryGraph,
    index: &ElementIndex,
    lib: LibraryId,
    unit: FileId,
    elements: &FileElements,
    strings: &StringTable,
) {
    // Named mixin applications, marked by the collector's target rows
    // (introductory spellings only — the merge rule reads the intro).
    let mixin_apps: FxHashSet<StringId> = elements
        .decl_kinds()
        .iter()
        .filter(|target| target.is_mixin_application)
        .map(|target| target.name)
        .collect();
    // A unit's container row is an `augment` head's own face when the
    // element introduces the name somewhere else: the chain-table row of
    // such a unit would claim an introduction that never happened, so the
    // fragment index (the same one the container anchor merges through)
    // answers the filter.
    for &name in elements.container_order() {
        if index.container_row_is_fragment(graph, lib, unit, name) {
            continue;
        }
        let Some(container) = elements.container(name) else {
            continue;
        };
        let kind = match container.kind {
            ContainerKind::Class => AugKind::Class,
            ContainerKind::Mixin => AugKind::Mixin,
            ContainerKind::Enum => AugKind::Enum,
            ContainerKind::Extension => AugKind::Extension,
            ContainerKind::ExtensionType => AugKind::ExtensionType,
        };
        push_entry(
            table,
            unit,
            // A container head joins the single top-level name slot —
            // `augment class C` and `void C()` collide under `C` (upstream
            // `_buildTopFragments` keys every kind by `fragment.name`).
            (None, name, Space::Top),
            kind,
            false,
            0,
            container.superclass.is_some(),
            mixin_apps.contains(&name),
            false,
            false,
            false,
            // A container head is never an executable — the
            // already-complete walk skips it.
            false,
        );
        for (member_name, member) in container.members.iter() {
            let member_kind = match member.kind {
                MemberKind::Method => AugKind::Method,
                MemberKind::Getter => AugKind::Getter,
                MemberKind::Setter => AugKind::Setter,
                MemberKind::Field => AugKind::Field,
            };
            let space = if member.is_static {
                Space::StaticMember
            } else {
                Space::Member
            };
            push_entry(
                table,
                unit,
                (
                    Some(name),
                    property_name(*member_name, member_kind, strings),
                    space,
                ),
                member_kind,
                false,
                0,
                false,
                false,
                member.is_final,
                false,
                false,
                // The accessor/variable completeness bit: `external ||
                // !abstract` — a method/getter/setter's `has_body ||
                // external` (the `is_abstract` complement), a field's
                // induced accessors' `external || !abstract`.
                member.is_external || !member.is_abstract,
            );
        }
    }
    for target in elements.decl_kinds() {
        if target.is_mixin_application {
            // Already folded into the container entry above.
            continue;
        }
        if target.kind == AugKind::TypeAlias {
            // The parser rejects typedef augmentations outright — the row
            // never joins a lookup chain (the augment early-continues).
            continue;
        }
        // A constructor selector lands in its owner's static namespace
        // (upstream `_staticNamespace`: `ConstructorFragmentImpl => true`),
        // everything else in the single top-level slot.
        let space = match target.kind {
            AugKind::Constructor => Space::StaticMember,
            _ => Space::Top,
        };
        push_entry(
            table,
            unit,
            (
                target.container,
                property_name(target.name, target.kind, strings),
                space,
            ),
            target.kind,
            false,
            0,
            false,
            false,
            target.is_final,
            target.is_const,
            target.is_factory,
            target.is_complete,
        );
    }
    for (aug_index, aug) in elements.augmentations().iter().enumerate() {
        if aug.kind == AugKind::TypeAlias {
            continue;
        }
        // Cross-file augmentations join as chain rows; the diagnosing
        // file's own join happens at process time (source order among
        // siblings).
        push_entry(
            table,
            unit,
            augment_key(aug, strings),
            aug.kind,
            true,
            aug_index,
            aug.extends_kw.is_some(),
            false,
            aug.is_final,
            aug.const_kw.is_some(),
            aug.factory_kw.is_some(),
            aug.is_complete,
        );
    }
}

#[expect(clippy::too_many_arguments)] // one column per `Entry` field
fn push_entry(
    table: &mut FxHashMap<(Option<StringId>, StringId, Space), Vec<Entry>>,
    unit: FileId,
    key: (Option<StringId>, StringId, Space),
    kind: AugKind,
    is_augment: bool,
    aug_index: usize,
    extends_present: bool,
    is_mixin_application: bool,
    is_final: bool,
    is_const: bool,
    is_factory: bool,
    is_complete: bool,
) {
    table.entry(key).or_default().push(Entry {
        unit,
        kind,
        is_augment,
        aug_index,
        extends_present,
        is_mixin_application,
        is_final,
        is_const,
        is_factory,
        is_complete,
    });
}

/// The merge refused entirely: no same-kind fragment precedes the
/// augmentation. The anchor is the variable's name for variable
/// declarations (upstream `visitFieldDeclaration` /
/// `visitTopLevelVariableDeclaration` pass `variable.name`), the `augment`
/// keyword everywhere else. The refusal faces rank mixin-application
/// first, then different-kind, then the plain row (upstream
/// `_checkAugmentationWithoutDeclaration`'s switch order).
fn without_declaration(
    out: &mut Vec<Diagnostic>,
    file: FileId,
    aug: &AugmentDecl,
    chain: &[&Entry],
) {
    let anchor = match aug.kind {
        AugKind::TopLevelVar | AugKind::Field => aug.name_range,
        _ => aug.augment_kw,
    };
    match chain.last() {
        Some(entry) if entry.is_mixin_application => {
            let code = SemanticCode::AugmentationOfMixinApplicationClass;
            out.push(row(file, anchor, code, code.message("", "")));
        }
        Some(entry) => {
            let code = SemanticCode::AugmentationOfDifferentDeclarationKind;
            let message = code.message(entry.kind.word(), aug.kind.word());
            out.push(row(file, anchor, code, message));
        }
        None => {
            let code = SemanticCode::AugmentationWithoutDeclaration(WithoutDeclarationShape::Plain);
            out.push(row(file, anchor, code, code.message("", "")));
        }
    }
}

/// The modifier pair checks (upstream `_checkForAugmentationModifierMismatch`):
/// an extra modifier anchors at its own keyword, a missing one at the
/// `augment` keyword.
fn check_modifiers(
    out: &mut Vec<Diagnostic>,
    file: FileId,
    aug: &AugmentDecl,
    checks: &[(&str, Option<SourceRange>, bool)],
) {
    for (word, kw, intro_has) in checks {
        match (*kw, *intro_has) {
            (Some(span), false) => {
                let code = SemanticCode::AugmentationModifierExtra;
                out.push(row(file, span, code, code.message(word, "")));
            }
            (None, true) => {
                let code = SemanticCode::AugmentationModifierMissing;
                out.push(row(file, aug.augment_kw, code, code.message(word, "")));
            }
            _ => {}
        }
    }
}

/// An augmentation with an `extends` clause where any preceding fragment of
/// the same element already carries one (upstream
/// `_checkForAugmentationExtendsClauseAlreadyPresent` over
/// `precedingFragments`, anchored at the augmentation's own `extends`
/// keyword).
fn check_extends(out: &mut Vec<Diagnostic>, file: FileId, aug: &AugmentDecl, run: &[&Entry]) {
    let Some(span) = aug.extends_kw else {
        return;
    };
    if run.iter().any(|entry| entry.extends_present) {
        let code = SemanticCode::AugmentationExtendsClauseAlreadyPresent;
        out.push(row(file, span, code, code.message("", "")));
    }
}

/// The type-parameter triple (upstream `_checkForAugmentationTypeParameters`):
/// count first — more parameters anchor at the first surplus name, fewer at
/// the closing `>`, none written against a generic intro at the
/// declaration's own name — then per-parameter name and one-directional
/// bound checks (an absent introductory bound never passes a written one).
fn check_type_param_rows(
    out: &mut Vec<Diagnostic>,
    file: FileId,
    own: &FileElements,
    intro_unit: &FileElements,
    strings: &StringTable,
    aug: &AugmentDecl,
    intro_names: &[StringId],
    intro_bounds: &[Ty],
) {
    if aug.type_params.is_empty() {
        if !intro_names.is_empty() {
            let code = SemanticCode::AugmentationTypeParameterCount;
            out.push(row(file, aug.name_range, code, code.message("", "")));
        }
        return;
    }
    if aug.type_params.len() > intro_names.len() {
        let code = SemanticCode::AugmentationTypeParameterCount;
        out.push(row(
            file,
            aug.type_params[intro_names.len()].name_range,
            code,
            code.message("", ""),
        ));
        return;
    }
    if aug.type_params.len() < intro_names.len() {
        if let Some(close) = aug.type_params_close {
            let code = SemanticCode::AugmentationTypeParameterCount;
            out.push(row(file, close, code, code.message("", "")));
        }
        return;
    }
    for (index, param) in aug.type_params.iter().enumerate() {
        if param.name != intro_names[index] {
            let code = SemanticCode::AugmentationTypeParameterName;
            out.push(row(file, param.name_range, code, code.message("", "")));
        }
        if let Some((bound, span)) = param.bound {
            let intro_bound = intro_bounds.get(index).copied().unwrap_or(Ty::Dynamic);
            if render(bound, own, strings) != render(intro_bound, intro_unit, strings) {
                let code = SemanticCode::AugmentationTypeParameterBound;
                out.push(row(file, span, code, code.message("", "")));
            }
        }
    }
}

/// The return-type pair (upstream `_checkForAugmentationReturnTypeMismatch`):
/// compared only when the augmentation writes an explicit type; both sides
/// invalid (uncaptured) never report. A variable's induced getter reports
/// the induced variant (`augmentationInducedGetterReturnTypeMismatch`); the
/// variable rows anchor at the variable's name (upstream
/// `errorEntity: variable.name`), the executable rows at the written return
/// type.
fn check_return(
    out: &mut Vec<Diagnostic>,
    file: FileId,
    own: &FileElements,
    intro_unit: &FileElements,
    strings: &StringTable,
    aug: &AugmentDecl,
    face: &Face<'_>,
    induced: bool,
    anchor: SourceRange,
) {
    let Some(actual_ty) = aug.return_ty else {
        return;
    };
    let (expected_ty, is_setter) = face.return_face();
    let expected = match expected_ty {
        Ty::UNKNOWN if is_setter => "void".to_string(),
        _ => render(expected_ty, intro_unit, strings),
    };
    let actual = render(actual_ty, own, strings);
    if actual == expected {
        return;
    }
    let shape = if induced {
        ReturnShape::InducedGetter
    } else {
        ReturnShape::Plain
    };
    let code = SemanticCode::AugmentationReturnTypeMismatch(shape);
    out.push(row(file, anchor, code, code.message(&actual, &expected)));
}

/// The formal-parameter pass (upstream
/// `_checkForAugmentationFormalParameters`): the required count, then the
/// optional count (only when the required counts agree), then — with the
/// positional shape matching — the positional name and pair checks, and
/// finally the named extra/missing passes (which run regardless of the
/// positional shape). No parameter list on the augmentation skips the
/// whole pass.
fn check_formals(
    out: &mut Vec<Diagnostic>,
    file: FileId,
    own: &FileElements,
    intro_unit: &FileElements,
    strings: &StringTable,
    aug: &AugmentDecl,
    intro_formals: Option<&Formals>,
) {
    let Some((_, right_paren)) = aug.parens else {
        return;
    };
    let empty = Formals {
        positional: Box::default(),
        named: Box::default(),
    };
    let first = intro_formals.unwrap_or(&empty);
    let cur_positional: Vec<&AugmentFormal> = aug
        .formals
        .iter()
        .filter(|f| f.required_positional || f.optional_positional)
        .collect();
    let cur_required = cur_positional
        .iter()
        .filter(|f| f.required_positional)
        .count();
    let cur_optional = cur_positional.len() - cur_required;
    let first_required = first.positional.iter().filter(|f| f.required).count();
    let first_optional = first.positional.len() - first_required;
    // The oracle's `formalParameterErrorEntity`: the parameter's name token,
    // else the parameter, else the `)`.
    let entity_at = |index: usize| {
        cur_positional
            .get(index)
            .map(|f| f.entity())
            .unwrap_or(right_paren)
    };
    if cur_required < first_required {
        let code = SemanticCode::AugmentationRequiredPositionalFormalParameterCount;
        let message = code.message(&cur_required.to_string(), &first_required.to_string());
        let span = aug.group_open.unwrap_or_else(|| entity_at(cur_required));
        out.push(row(file, span, code, message));
    } else if cur_required > first_required {
        let code = SemanticCode::AugmentationRequiredPositionalFormalParameterCount;
        let message = code.message(&cur_required.to_string(), &first_required.to_string());
        out.push(row(file, entity_at(first_required), code, message));
    } else if cur_optional != first_optional {
        let code = SemanticCode::AugmentationOptionalPositionalFormalParameterCount;
        let message = code.message(&cur_optional.to_string(), &first_optional.to_string());
        let span = if cur_optional < first_optional {
            right_paren
        } else if first_optional == 0 {
            aug.group_open.unwrap_or_else(|| entity_at(first_required))
        } else {
            entity_at(first_required + first_optional)
        };
        out.push(row(file, span, code, message));
    }
    let shape_matches = cur_required == first_required && cur_optional == first_optional;
    if shape_matches {
        // Every non-wildcard positional name must match the preceding
        // fragments' same-index name (upstream
        // `augmentationPositionalFormalParameterName`; the intro's row is
        // the decisive preceding name for the single-intro chains the
        // fixtures carry).
        for (index, cur) in cur_positional.iter().enumerate() {
            let Some(cur_name) = cur.name else {
                continue;
            };
            if strings.resolve(cur_name) == "_" {
                continue;
            }
            let Some(prev) = first.positional.get(index).map(|f| f.name) else {
                continue;
            };
            if prev == cur_name || strings.resolve(prev) == "_" {
                continue;
            }
            let code = SemanticCode::AugmentationPositionalFormalParameterName;
            let message = code.message(strings.resolve(cur_name), strings.resolve(prev));
            out.push(row(file, cur.entity(), code, message));
        }
        // The positional pairs: covariant, required (named-only — a
        // positional pair never carries `required` on either side), then
        // the written-type comparison.
        for (index, cur) in cur_positional.iter().enumerate() {
            let Some(first_param) = first.positional.get(index) else {
                continue;
            };
            check_pair(out, file, own, intro_unit, strings, cur, first_param, false);
        }
    }
    // The named passes run regardless of the positional shape (upstream
    // keeps them outside the shape guard). The intro's map keeps the LAST
    // duplicate spelling (upstream overwrites), the augmentation's the
    // first (upstream `putIfAbsent`).
    let mut first_named: Vec<(StringId, &Formal)> = Vec::new();
    for param in first.named.iter() {
        if let Some(slot) = first_named.iter_mut().find(|(name, _)| *name == param.name) {
            slot.1 = param;
        } else {
            first_named.push((param.name, param));
        }
    }
    let mut cur_named: Vec<(StringId, &AugmentFormal)> = Vec::new();
    for param in aug.formals.iter().filter(|f| f.named) {
        let Some(name) = param.name else {
            continue;
        };
        if !cur_named.iter().any(|(existing, _)| *existing == name) {
            cur_named.push((name, param));
        }
    }
    for (name, cur) in &cur_named {
        match first_named.iter().find(|(candidate, _)| candidate == name) {
            Some((_, first_param)) => {
                check_pair(out, file, own, intro_unit, strings, cur, first_param, true);
            }
            None => {
                let code = SemanticCode::AugmentationNamedFormalParameterExtra;
                let message = code.message(strings.resolve(*name), "");
                out.push(row(file, cur.entity(), code, message));
            }
        }
    }
    for (name, _) in &first_named {
        if !cur_named.iter().any(|(existing, _)| existing == name) {
            let code = SemanticCode::AugmentationNamedFormalParameterMissing;
            let message = code.message(strings.resolve(*name), "");
            out.push(row(file, right_paren, code, message));
        }
    }
}

/// One formal-parameter pair: the covariant and (named-only) required
/// modifier checks, then the written-type comparison — upstream
/// `checkFormalParameterPair`'s order. An unwritten intro type compares as
/// `dynamic` (the oracle's parameter type); the augmentation's omitted
/// type never reports.
fn check_pair(
    out: &mut Vec<Diagnostic>,
    file: FileId,
    own: &FileElements,
    intro_unit: &FileElements,
    strings: &StringTable,
    cur: &AugmentFormal,
    first: &Formal,
    named: bool,
) {
    match (cur.covariant_kw, first.covariant) {
        (Some(span), false) => {
            let code = SemanticCode::AugmentationFormalParameterModifierExtra;
            out.push(row(file, span, code, code.message("covariant", "")));
        }
        (None, true) => {
            let code = SemanticCode::AugmentationFormalParameterModifierMissing;
            out.push(row(file, cur.entity(), code, code.message("covariant", "")));
        }
        _ => {}
    }
    if named {
        match (cur.required_kw, first.required) {
            (Some(span), false) => {
                let code = SemanticCode::AugmentationFormalParameterModifierExtra;
                out.push(row(file, span, code, code.message("required", "")));
            }
            (None, true) => {
                let code = SemanticCode::AugmentationFormalParameterModifierMissing;
                out.push(row(file, cur.entity(), code, code.message("required", "")));
            }
            _ => {}
        }
    }
    if let (Some(actual_ty), Some(span)) = (cur.ty, cur.ty_range) {
        let expected = match first.ty {
            Ty::UNKNOWN => "dynamic".to_string(),
            ty => render(ty, intro_unit, strings),
        };
        let actual = render(actual_ty, own, strings);
        if actual != expected {
            let code = SemanticCode::AugmentationFormalParameterTypeMismatch;
            let message = code.message(&actual, &expected);
            out.push(row(file, span, code, message));
        }
    }
}

/// The compare-time face of a resolved intro (never an augmentation): the
/// declaring unit's own tables answer per kind. Members and top-level
/// faces share the accessor kinds — the container key decides.
fn intro_face<'e>(
    elements: &'e FileElements,
    intro: &Entry,
    aug: &AugmentDecl,
) -> Option<Face<'e>> {
    match intro.kind {
        AugKind::Class
        | AugKind::Mixin
        | AugKind::Enum
        | AugKind::Extension
        | AugKind::ExtensionType => elements.container(aug.name).map(Face::Container),
        AugKind::Constructor => {
            let container = aug.container?;
            Some(Face::Ctor {
                container: elements.container(container)?,
                name: aug.name,
                is_factory: intro.is_factory,
            })
        }
        AugKind::Field | AugKind::Method if aug.container.is_some() => {
            let container = aug.container?;
            elements
                .container(container)?
                .members
                .get(&aug.name)
                .map(Face::Member)
        }
        AugKind::Getter | AugKind::Setter if aug.container.is_some() => {
            let container = aug.container?;
            elements
                .container(container)?
                .members
                .get(&aug.name)
                .map(Face::Member)
        }
        AugKind::TopLevelVar => elements.decl(aug.name).map(Face::TopVar),
        _ => elements.decl(aug.name).map(|decl| Face::TopFn {
            decl,
            is_setter: intro.kind == AugKind::Setter,
        }),
    }
}

/// Renders a written type through its owning unit's argument pool; the
/// model's unknown sentinel spells `dynamic` (the oracle's element type
/// for an unwritten annotation).
fn render(ty: Ty, unit: &FileElements, strings: &StringTable) -> String {
    match ty {
        Ty::UNKNOWN => "dynamic".to_string(),
        _ => ty.render(strings, &unit.type_args),
    }
}

fn row(file: FileId, range: SourceRange, code: SemanticCode, message: String) -> Diagnostic {
    Diagnostic::new(
        Severity::Error,
        code.code_info(),
        message,
        file,
        range,
        None,
    )
}

/// `_checkForFactoryBodyCompleteness` (upstream Dart SDK source,
/// `pkg/analyzer/lib/src/generated/error_verifier.dart:5078-5107`): a
/// factory constructor with no body in a fragment chain that never
/// completes reports `factory_without_body` at the constructor's
/// `errorRange`. The experiment gate is the caller's (`:5079-5081`); an
/// `augment` fragment never reports (`:5083-5086` — the introductory
/// declaration owns the row); a chain with more than one fragment that
/// stays incomplete is `factory_not_complete_after_augmentations`
/// (`:5101-5106`), which has no registry row yet — that arm stays silent,
/// since `factory_without_body` there would be an extra.
fn factory_body_diagnostics(
    file: FileId,
    own: &FileElements,
    index: &ElementIndex,
    graph: &LibraryGraph,
    lib: LibraryId,
    strings: &StringTable,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    // Only augmentations applied AFTER the introductory declaration join
    // its chain (upstream `firstFragment.augmentations` — the
    // `ElementBuilder` appends in `allUnits` walk order, so an augment
    // in an earlier unit never reaches this intro).
    let units = graph.unit_files(lib);
    let after: &[FileId] = match units.iter().position(|&unit| unit == file) {
        Some(pos) => &units[pos + 1..],
        None => &[],
    };
    for target in own.decl_kinds() {
        if target.kind != AugKind::Constructor || !target.is_factory || target.is_complete {
            continue;
        }
        let key = (
            target.container,
            property_name(target.name, target.kind, strings),
            Space::StaticMember,
        );
        let mut fragments = 0usize;
        let mut complete = false;
        for &unit in after {
            let Some(elements) = index.get(unit) else {
                continue;
            };
            for aug in elements.augmentations() {
                if augment_key(aug, strings) != key {
                    continue;
                }
                fragments += 1;
                complete |= aug.is_complete;
            }
        }
        // The diagnosing unit's own augmentations count only when they
        // follow the factory head in source order — `AugTarget::head_range`
        // is the head's `errorRange` and `augment_kw` the augment keyword's
        // offset, so the same-unit before/after relation is decidable for
        // constructors.
        for aug in own.augmentations() {
            if augment_key(aug, strings) != key {
                continue;
            }
            if let Some(head) = target.head_range
                && aug.augment_kw.offset <= head.offset
            {
                continue;
            }
            fragments += 1;
            complete |= aug.is_complete;
        }
        if complete {
            continue;
        }
        let Some(head) = target.head_range else {
            continue;
        };
        // A chain that grew past the introductory declaration is the
        // `factory_not_complete_after_augmentations` face
        // (error_verifier.dart:5101-5106): the same anchor, the message
        // naming the selector (the unnamed face reads `new`).
        let (code, message) = if fragments > 0 {
            let spelled = strings.resolve(target.name);
            let display = match target.container {
                Some(container) if spelled == strings.resolve(container) => "new",
                _ => spelled,
            };
            let code = SemanticCode::FactoryNotCompleteAfterAugmentations;
            (code, code.message(display, ""))
        } else {
            let code = SemanticCode::FactoryWithoutBody;
            (code, code.message("", ""))
        };
        out.push(row(file, head, code, message));
    }
    out
}

// The #3201 pins — the factory-body completeness face
// (`factory_without_body`) with the augmentations experiment on.
#[cfg(test)]
#[path = "ticket_3201_tests.rs"]
mod ticket_3201_tests;
