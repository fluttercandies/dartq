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
use dartq_resolution::{LibraryGraph, LibraryId, Lookup, NamespaceEntry, Namespaces};
use dartq_source::{FileId, SourceRange, StringId, StringTable};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::codes::{AlreadyCompleteShape, ReturnShape, SemanticCode, WithoutDeclarationShape};
use crate::library::ElementIndex;
use crate::model::{
    AugKind, AugmentDecl, AugmentFormal, Container, ContainerKind, FileElements, Formal, Formals,
    MemberEntry, MemberKind, MicroKind, Ty,
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
    // `_checkForFunctionBodyCompleteness` (`:5256-5321`) and
    // `_checkForIncompleteInducedAccessors` (`:5415-5456`) run for every
    // introductory executable and abstract static field under the same gate
    // (`:5259-5261`/`:5419-5421`) — both are intro-side walks, so they share
    // this placement ahead of the augmentation-table early return.
    let mut out = if augmentations_enabled {
        let mut out = factory_body_diagnostics(file, own, index, graph, lib, strings);
        out.extend(executable_body_diagnostics(
            file, own, index, graph, lib, strings,
        ));
        out
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
    // The canonical-type context: the oracle compares the resolved types
    // (a typedef spells its expansion, `p.C` names the same element `C`
    // does), so each written annotation canonicalizes through the writing
    // file's scope while its own spelling stays for the message text.
    let namespaces = Namespaces::build(graph);
    let canon = Canon {
        graph,
        namespaces: &namespaces,
        index,
        strings,
    };
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
            process_variable(&mut out, file, aug, &chain, own, index, strings, &canon);
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
                    check_header(&mut out, file, own, index, aug, intro, run, &canon);
                } else {
                    // The already-complete check runs against the chain's
                    // completeness state before the shape checks (upstream
                    // `visitConstructorDeclaration` /
                    // `_checkForFunctionAlreadyComplete`).
                    check_already_complete(&mut out, file, aug, run);
                    if !intro.is_augment {
                        check_member_shape(
                            &mut out, file, own, index, strings, aug, intro, run, &canon,
                        );
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
    canon: &Canon<'_>,
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
        check_variable_augment(out, file, aug, generation, own, index, strings, canon);
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
            let aug_params = aug_param_names(aug, own);
            check_type_param_rows(
                out,
                file,
                own,
                intro,
                intro_elements,
                canon,
                aug,
                &aug_params,
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
                    canon,
                    aug,
                    &face,
                    intro,
                    false,
                    span,
                );
            }
            check_formals(
                out,
                file,
                own,
                intro_elements,
                index,
                strings,
                canon,
                aug,
                &aug_params,
                intro_params(intro_elements, aug, &face),
                face.formals(),
                &sub,
            );
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
    canon: &Canon<'_>,
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
            canon,
            aug,
            &face,
            intro,
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
    aug: &AugmentDecl,
    intro: &Entry,
    run: &[&Entry],
    canon: &Canon<'_>,
) {
    let Some(intro_elements) = index.get(intro.unit) else {
        return;
    };
    let aug_params = aug_param_names(aug, own);
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
            intro,
            intro_elements,
            canon,
            aug,
            &aug_params,
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
            intro,
            intro_elements,
            canon,
            aug,
            &aug_params,
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
    run: &[&Entry],
    canon: &Canon<'_>,
) {
    let Some(intro_elements) = index.get(intro.unit) else {
        return;
    };
    let Some(face) = intro_face(intro_elements, intro, aug) else {
        return;
    };
    let aug_params = aug_param_names(aug, own);
    match aug.kind {
        AugKind::Constructor => {
            let checks = [
                ("const", aug.const_kw, intro.is_const),
                ("factory", aug.factory_kw, intro.is_factory),
            ];
            check_modifiers(out, file, aug, &checks);
            check_formals(
                out,
                file,
                own,
                intro_elements,
                index,
                strings,
                canon,
                aug,
                &aug_params,
                intro_params(intro_elements, aug, &face),
                face.formals(),
                run,
            );
        }
        AugKind::Function | AugKind::Method => {
            let (intro_names, intro_bounds) = face.type_params();
            check_type_param_rows(
                out,
                file,
                own,
                intro,
                intro_elements,
                canon,
                aug,
                &aug_params,
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
                    canon,
                    aug,
                    &face,
                    intro,
                    false,
                    span,
                );
            }
            check_formals(
                out,
                file,
                own,
                intro_elements,
                index,
                strings,
                canon,
                aug,
                &aug_params,
                intro_params(intro_elements, aug, &face),
                face.formals(),
                run,
            );
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
    intro: &Entry,
    intro_unit: &FileElements,
    canon: &Canon<'_>,
    aug: &AugmentDecl,
    aug_params: &[StringId],
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
            // The bound pair judges canonical types (upstream
            // `boundNode.typeOrThrow != firstBound`): a typedef spelling and
            // an import-prefixed one compare by expansion/element, never by
            // written name.
            let aug_bound = canon_ty(bound, file, own, aug_params, canon, CANON_DEPTH);
            let intro_canon = canon_ty(
                intro_bound,
                intro.unit,
                intro_unit,
                intro_names,
                canon,
                CANON_DEPTH,
            );
            if aug_bound != intro_canon {
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
    canon: &Canon<'_>,
    aug: &AugmentDecl,
    face: &Face<'_>,
    intro: &Entry,
    induced: bool,
    anchor: SourceRange,
) {
    let Some(actual_ty) = aug.return_ty else {
        return;
    };
    let (expected_ty, is_setter) = face.return_face();
    // The judged side: the oracle's `actualType == expectedType` compares
    // resolved types — `p.C` names the element `C` does, a typedef spells
    // its expansion — and an unwritten/invalid face is the oracle's
    // `dynamic`/`InvalidType` bail. The aug side's own type parameters are
    // in scope; the intro face's are.
    let aug_params = aug_param_names(aug, own);
    let intro_params = intro_params(intro_unit, aug, face);
    let Some(actual) = canon_top(actual_ty, file, own, &aug_params, canon) else {
        return;
    };
    let (actual_canon, expected_canon) = match (expected_ty, is_setter) {
        (Ty::UNKNOWN, true) => (actual, "void".to_string()),
        (Ty::UNKNOWN, false) => (actual, "dynamic".to_string()),
        _ => {
            let Some(expected) =
                canon_top(expected_ty, intro.unit, intro_unit, &intro_params, canon)
            else {
                return;
            };
            (actual, expected)
        }
    };
    if actual_canon == expected_canon {
        return;
    }
    // The written side: the oracle's `getDisplayString` prints the resolved
    // element's spelling — a typedef keeps its alias name, a prefixed head
    // drops the prefix.
    let expected = match expected_ty {
        Ty::UNKNOWN if is_setter => "void".to_string(),
        _ => display_ty(expected_ty, intro_unit, strings),
    };
    let actual = display_ty(actual_ty, own, strings);
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
#[expect(clippy::too_many_arguments)] // one column per check input
fn check_formals(
    out: &mut Vec<Diagnostic>,
    file: FileId,
    own: &FileElements,
    intro_unit: &FileElements,
    index: &ElementIndex,
    strings: &StringTable,
    canon: &Canon<'_>,
    aug: &AugmentDecl,
    aug_params: &[StringId],
    intro_params: Vec<StringId>,
    intro_formals: Option<&Formals>,
    run: &[&Entry],
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
        // Every non-wildcard positional name must match a preceding
        // fragment's same-index name (upstream `precedingFragments` —
        // the intro plus every earlier augment fragment of this chain):
        // the first preceding fragment with a different non-wildcard name
        // owns the row's expected name.
        let preceding_names: Vec<Vec<Option<StringId>>> = run
            .iter()
            .map(|entry| {
                if entry.is_augment {
                    index
                        .get(entry.unit)
                        .and_then(|elements| elements.augmentations().get(entry.aug_index))
                        .map(|decl| {
                            decl.formals
                                .iter()
                                .filter(|f| f.required_positional || f.optional_positional)
                                .map(|f| f.name)
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    first
                        .positional
                        .iter()
                        .map(|f| Some(f.name))
                        .collect()
                }
            })
            .collect();
        for (index, cur) in cur_positional.iter().enumerate() {
            let Some(cur_name) = cur.name else {
                continue;
            };
            if strings.resolve(cur_name) == "_" {
                continue;
            }
            for names in &preceding_names {
                let Some(Some(prev)) = names.get(index) else {
                    continue;
                };
                if *prev == cur_name || strings.resolve(*prev) == "_" {
                    continue;
                }
                let code = SemanticCode::AugmentationPositionalFormalParameterName;
                let message = code.message(strings.resolve(cur_name), strings.resolve(*prev));
                out.push(row(file, cur.entity(), code, message));
                break;
            }
        }
        // The positional pairs: covariant, required (named-only — a
        // positional pair never carries `required` on either side), then
        // the written-type comparison.
        let intro_unit_id = run.first().map_or(file, |entry| entry.unit);
        for (index, cur) in cur_positional.iter().enumerate() {
            let Some(first_param) = first.positional.get(index) else {
                continue;
            };
            check_pair(
                out,
                file,
                own,
                intro_unit_id,
                intro_unit,
                strings,
                canon,
                aug_params,
                &intro_params,
                cur,
                first_param,
                false,
            );
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
    let intro_unit_id = run.first().map_or(file, |entry| entry.unit);
    for (name, cur) in &cur_named {
        match first_named.iter().find(|(candidate, _)| candidate == name) {
            Some((_, first_param)) => {
                check_pair(
                    out,
                    file,
                    own,
                    intro_unit_id,
                    intro_unit,
                    strings,
                    canon,
                    aug_params,
                    &intro_params,
                    cur,
                    first_param,
                    true,
                );
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
#[expect(clippy::too_many_arguments)] // one column per check input
fn check_pair(
    out: &mut Vec<Diagnostic>,
    file: FileId,
    own: &FileElements,
    intro_unit_id: FileId,
    intro_unit: &FileElements,
    strings: &StringTable,
    canon: &Canon<'_>,
    aug_params: &[StringId],
    intro_params: &[StringId],
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
        // The judged side: resolved-type equality like the return pair —
        // `p.C` and `C` name one element, a typedef spells its expansion;
        // an unwritten/invalid expected face is `dynamic` and an invalid
        // actual face is the oracle's `InvalidType` bail.
        let matches = match first.ty {
            Ty::UNKNOWN => canon_top(actual_ty, file, own, aug_params, canon)
                .is_some_and(|actual| actual == "dynamic"),
            expected_ty => {
                match (
                    canon_top(actual_ty, file, own, aug_params, canon),
                    canon_top(expected_ty, intro_unit_id, intro_unit, intro_params, canon),
                ) {
                    (Some(actual), Some(expected)) => actual == expected,
                    _ => false,
                }
            }
        };
        if !matches {
            let expected = match first.ty {
                Ty::UNKNOWN => "dynamic".to_string(),
                ty => display_ty(ty, intro_unit, strings),
            };
            let actual = display_ty(actual_ty, own, strings);
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

/// The canonical-type judgment context (upstream `TypeImpl ==` semantics):
/// each written annotation resolves through its own file's combined scope —
/// an import-prefixed `p.C` names the same declaration `C` does, a typedef
/// name expands through its captured right-hand side — and the compared
/// string is that canonical form while the message text keeps the written
/// spelling (the written/judged split: the oracle's `getDisplayString`
/// prints the resolved element, its `==` the resolved identity).
struct Canon<'a> {
    graph: &'a LibraryGraph,
    namespaces: &'a Namespaces,
    index: &'a ElementIndex,
    strings: &'a StringTable,
}

/// The alias-expansion recursion budget: a chain of co-aliases spends one
/// unit per hop; an exhausted chain reads the oracle's `InvalidType` (the
/// suppress direction, never a fabricated pair).
const CANON_DEPTH: u8 = 8;

/// The canonical signature of `ty` as a compared type: `None` when the
/// whole written type is the oracle's `InvalidType` (an unwritten slot or
/// a head its own file cannot resolve) — the callers' suppress arm, the
/// upstream `actualType is InvalidType || expectedType is InvalidType`
/// early return.
fn canon_top(
    ty: Ty,
    file: FileId,
    unit: &FileElements,
    params: &[StringId],
    cx: &Canon<'_>,
) -> Option<String> {
    let mut out = String::new();
    match canon_into(&mut out, ty, file, unit, params, &[], cx, CANON_DEPTH) {
        Leaf::Resolved => Some(out),
        Leaf::Invalid => None,
    }
}

/// The canonical signature for a nested slot (a type argument, a bound, a
/// column inside a function type): an unresolvable leaf compares as the
/// shared `?invalid` marker — two InvalidType occurrences are the same
/// singleton upstream and judge equal, while an invalid leaf never equals
/// a resolved one.
fn canon_ty(
    ty: Ty,
    file: FileId,
    unit: &FileElements,
    params: &[StringId],
    cx: &Canon<'_>,
    depth: u8,
) -> String {
    let mut out = String::new();
    match canon_into(&mut out, ty, file, unit, params, &[], cx, depth) {
        Leaf::Resolved => out,
        Leaf::Invalid => "?invalid".to_string(),
    }
}

/// Whether a canonical leaf resolved: `Invalid` carries the oracle's
/// `InvalidType` semantics (a suppress at the compared type's root, an
/// equal-to-itself marker inside).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Leaf {
    Resolved,
    Invalid,
}

/// One canonical leaf onto `out`: the head's resolved identity (its
/// declaring file plus its declared name — prefix spellings and same-name
/// imports land on one spelling), the written arguments canonicalized the
/// same way, and the `?` projection. A bare name the writing file cannot
/// resolve reads its own declaration's type-parameter scope first — a
/// parameter leaf compares by name — then the namespace; a miss is
/// `Invalid`.
#[expect(clippy::too_many_arguments)]
fn canon_into(
    out: &mut String,
    ty: Ty,
    file: FileId,
    unit: &FileElements,
    params: &[StringId],
    sig: &[StringId],
    cx: &Canon<'_>,
    depth: u8,
) -> Leaf {
    match ty {
        Ty::Unknown => return Leaf::Invalid,
        Ty::Null => out.push_str("Null"),
        Ty::Dynamic => out.push_str("dynamic"),
        Ty::Never => out.push_str("Never"),
        Ty::Void => out.push_str("void"),
        Ty::Named {
            name,
            nullable,
            args,
        } => {
            let spelling = cx.strings.resolve(name);
            match canon_head(spelling, file, params, sig, cx) {
                Head::Invalid => return Leaf::Invalid,
                Head::Param => {
                    out.push_str("param:");
                    out.push_str(spelling);
                }
                Head::SigParam(position) => {
                    out.push_str("param:@");
                    out.push_str(&position.to_string());
                }
                Head::Alias(decl_file, decl_name) => {
                    // The written arguments canonicalize in the use site's
                    // scope BEFORE the substitution walks the alias's own
                    // right-hand side — through `canon_into` so the
                    // signature's own variables still encode by position
                    // (a nested `Function<T>(A<T>)` keeps its `param:@0`).
                    let canon_args: Vec<String> = unit
                        .type_args
                        .get(args)
                        .iter()
                        .map(|arg| {
                            let mut text = String::new();
                            match canon_into(
                                &mut text,
                                *arg,
                                file,
                                unit,
                                params,
                                sig,
                                cx,
                                depth.saturating_sub(1),
                            ) {
                                Leaf::Resolved => text,
                                Leaf::Invalid => "?invalid".to_string(),
                            }
                        })
                        .collect();
                    if canon_alias(out, decl_file, decl_name, &canon_args, cx, depth.saturating_sub(1))
                        == Leaf::Invalid
                    {
                        return Leaf::Invalid;
                    }
                    if nullable {
                        out.push('?');
                    }
                }
                Head::Named(decl_file, decl_name) => {
                    out.push_str("decl:");
                    out.push_str(&decl_file.index().to_string());
                    out.push(':');
                    out.push_str(cx.strings.resolve(decl_name));
                    let list = unit.type_args.get(args);
                    if !list.is_empty() {
                        out.push('<');
                        for (index, arg) in list.iter().enumerate() {
                            if index > 0 {
                                out.push(',');
                            }
                            if canon_into(out, *arg, file, unit, params, sig, cx, depth)
                                == Leaf::Invalid
                            {
                                out.push_str("?invalid");
                            }
                        }
                        out.push('>');
                    }
                    if nullable {
                        out.push('?');
                    }
                }
            }
        }
        Ty::Record { shape, nullable } => {
            let shape = unit.type_args.record_shape(shape);
            out.push('(');
            let mut first = true;
            for field in shape.positional.iter() {
                if !first {
                    out.push(',');
                }
                first = false;
                if canon_into(out, *field, file, unit, params, sig, cx, depth)
                    == Leaf::Invalid
                {
                    out.push_str("?invalid");
                }
            }
            for (field_name, field) in shape.named.iter() {
                if !first {
                    out.push(',');
                }
                first = false;
                if canon_into(out, *field, file, unit, params, sig, cx, depth)
                    == Leaf::Invalid
                {
                    out.push_str("?invalid");
                }
                out.push(' ');
                out.push_str(cx.strings.resolve(*field_name));
            }
            out.push(')');
            if nullable {
                out.push('?');
            }
        }
        Ty::Function { shape, nullable } => {
            let shape = unit.type_args.function_shape(shape);
            // The signature's own variables alpha-rename (upstream
            // `relateTypeFormals` — `<T>T -> T` equals `<U>U -> U`), so the
            // columns judge each variable by position, never by spelling;
            // the signature's own bound group judges by position too.
            let inner: Vec<StringId> =
                shape.type_params_named.iter().map(|(name, _)| *name).collect();
            if canon_into(out, shape.return_ty, file, unit, params, &inner, cx, depth)
                == Leaf::Invalid
            {
                out.push_str("?invalid");
            }
            out.push_str(" Function");
            if !shape.type_params_named.is_empty() {
                out.push('<');
                for (index, (_, bound)) in shape.type_params_named.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    out.push_str(&index.to_string());
                    if let Some(bound) = bound {
                        out.push_str(" extends ");
                        if canon_into(out, *bound, file, unit, params, &inner, cx, depth)
                            == Leaf::Invalid
                        {
                            out.push_str("?invalid");
                        }
                    }
                }
                out.push('>');
            }
            out.push('(');
            let mut first = true;
            for formal in shape.positional.iter() {
                if !first {
                    out.push(',');
                }
                first = false;
                if canon_into(out, *formal, file, unit, params, &inner, cx, depth)
                    == Leaf::Invalid
                {
                    out.push_str("?invalid");
                }
            }
            for (formal_name, formal) in shape.named.iter() {
                if !first {
                    out.push(',');
                }
                first = false;
                if canon_into(out, *formal, file, unit, params, &inner, cx, depth)
                    == Leaf::Invalid
                {
                    out.push_str("?invalid");
                }
                out.push(' ');
                out.push_str(cx.strings.resolve(*formal_name));
            }
            out.push(')');
            out.push_str(&format!("/{}", shape.required_positional));
            if nullable {
                out.push('?');
            }
        }
    }
    Leaf::Resolved
}

/// The head resolution of one `Ty::Named` leaf: the written spelling's
/// declaration identity in `file`'s scope, an in-scope type parameter, a
/// typedef to expand, or the unresolvable (`InvalidType`) face.
enum Head {
    /// Resolved to a non-alias declaration: (declaring file, declared name).
    Named(FileId, StringId),
    /// Resolved to a `MicroKind::TypeAlias` declaration: the judged type is
    /// its right-hand side with the written arguments substituted.
    Alias(FileId, StringId),
    /// A declaration-scope type parameter (compares by spelling across
    /// fragments).
    Param,
    /// The enclosing signature's own type parameter, by position (upstream
    /// `relateTypeFormals` alpha-renames — `<T>T -> T` equals `<U>U -> U`).
    SigParam(usize),
    /// Unresolvable — the oracle's `InvalidType`.
    Invalid,
}

/// Resolves `spelling` (the interned head's text — `p.C` carries its dot)
/// in `file`'s scope: a bare name checks the param scope before the
/// namespace (parameters shadow declarations), then the combined import
/// scope; a prefixed name splits at the first dot and walks the prefix's
/// table. A typedef answer surfaces `Head::Alias` for the caller's
/// substitution expansion; a non-type declaration (function, variable,
/// prefix) is the oracle's `InvalidType`.
fn canon_head(
    spelling: &str,
    file: FileId,
    params: &[StringId],
    sig: &[StringId],
    cx: &Canon<'_>,
) -> Head {
    let (prefix, leaf) = match spelling.split_once('.') {
        Some((prefix, leaf)) => (Some(prefix), leaf),
        None => (None, spelling),
    };
    let name = cx.strings.intern(leaf);
    if prefix.is_none() {
        if let Some(position) = sig.iter().position(|param| *param == name) {
            return Head::SigParam(position);
        }
        if params.contains(&name) {
            return Head::Param;
        }
    }
    let lib = match cx.graph.library_of_file(file) {
        Some(lib) => lib,
        None => return Head::Invalid,
    };
    let lookup = match prefix {
        Some(prefix) => cx.namespaces.lookup_prefixed_in_file(
            cx.graph,
            lib,
            Some(file),
            cx.strings.intern(prefix),
            name,
        ),
        None => cx
            .namespaces
            .lookup_in_file(cx.graph, lib, Some(file), name),
    };
    match lookup {
        Lookup::Found(NamespaceEntry::Decl(decl_file)) => match cx
            .index
            .get(decl_file)
            .and_then(|elements| elements.decl(name))
        {
            Some(decl) if decl.kind == MicroKind::TypeAlias => Head::Alias(decl_file, name),
            Some(decl)
                if matches!(
                    decl.kind,
                    MicroKind::TopLevelFn | MicroKind::TopLevelVar | MicroKind::TypeParam
                ) =>
            {
                // A spelling that resolves to a non-type declaration is no
                // type upstream — `InvalidType`.
                Head::Invalid
            }
            Some(_) | None => {
                // An indexed declaration face (container/enum/mixin/
                // extension) or an unindexed decl row (the SDK projection's
                // names-only grade) — identity by declaring file + name.
                Head::Named(decl_file, name)
            }
        },
        Lookup::Found(NamespaceEntry::Prefix) | Lookup::Ambiguous(_) | Lookup::NotFound => {
            Head::Invalid
        }
    }
}

/// The typedef-expansion pass of [`canon_into`]'s `Ty::Named` arm: the
/// head resolved to a `MicroKind::TypeAlias` declaration, so the judged
/// type is the alias's right-hand side with the written arguments
/// substituted for the alias's own parameters — canonicalized in the
/// declaring file's scope and pool.
fn canon_alias(
    out: &mut String,
    decl_file: FileId,
    decl_name: StringId,
    args: &[String],
    cx: &Canon<'_>,
    depth: u8,
) -> Leaf {
    if depth == 0 {
        return Leaf::Invalid;
    }
    let Some(elements) = cx.index.get(decl_file) else {
        return Leaf::Invalid;
    };
    let Some(decl) = elements.decl(decl_name) else {
        return Leaf::Invalid;
    };
    // The substitution table: the alias's declared parameters map to the
    // use site's written arguments (already canonicalized in the use
    // site's scope); a parameter the site never spelled reads `dynamic`
    // (the instantiate-to-bounds default's first approximation — the model
    // has no bound walk here).
    let mut subst: FxHashMap<StringId, String> = FxHashMap::default();
    for (index, param) in decl.type_param_names.iter().enumerate() {
        let text = args.get(index).cloned().unwrap_or_else(|| "dynamic".to_string());
        subst.insert(*param, text);
    }
    canon_alias_ty(
        out,
        decl.var_ty,
        decl_file,
        elements,
        &subst,
        cx,
        depth.saturating_sub(1),
    )
}

/// The alias right-hand side's canonical walk: `var_ty`'s slots index the
/// DECLARING unit's pool, parameter leaves splice the substitution text,
/// and every other head resolves in the declaring file's own scope.
fn canon_alias_ty(
    out: &mut String,
    ty: Ty,
    decl_file: FileId,
    decl_unit: &FileElements,
    subst: &FxHashMap<StringId, String>,
    cx: &Canon<'_>,
    depth: u8,
) -> Leaf {
    match ty {
        Ty::Unknown => return Leaf::Invalid,
        Ty::Null => out.push_str("Null"),
        Ty::Dynamic => out.push_str("dynamic"),
        Ty::Never => out.push_str("Never"),
        Ty::Void => out.push_str("void"),
        Ty::Named {
            name,
            nullable,
            args,
        } => {
            if let Some(text) = subst.get(&name) {
                out.push_str(text);
            } else {
                let spelling = cx.strings.resolve(name);
                match canon_head(spelling, decl_file, &[], &[], cx) {
                    Head::Invalid => return Leaf::Invalid,
                    Head::Param | Head::SigParam(_) => {
                        out.push_str("param:");
                        out.push_str(spelling);
                    }
                    Head::Named(file_id, decl_name) => {
                        out.push_str("decl:");
                        out.push_str(&file_id.index().to_string());
                        out.push(':');
                        out.push_str(cx.strings.resolve(decl_name));
                        let list = decl_unit.type_args.get(args);
                        if !list.is_empty() {
                            out.push('<');
                            for (index, arg) in list.iter().enumerate() {
                                if index > 0 {
                                    out.push(',');
                                }
                                if canon_alias_ty(
                                    out, *arg, decl_file, decl_unit, subst, cx, depth,
                                ) == Leaf::Invalid
                                {
                                    out.push_str("?invalid");
                                }
                            }
                            out.push('>');
                        }
                    }
                    Head::Alias(file_id, decl_name) => {
                        // A co-alias in the RHS expands the same way (a
                        // typedef inside a typedef's expansion is still a
                        // typedef — depth-bounded); its written arguments
                        // canonicalize through the SAME substitution the
                        // enclosing RHS carries.
                        let canon_args: Vec<String> = decl_unit
                            .type_args
                            .get(args)
                            .iter()
                            .map(|arg| {
                                let mut text = String::new();
                                match canon_alias_ty(
                                    &mut text, *arg, decl_file, decl_unit, subst, cx, depth,
                                ) {
                                    Leaf::Resolved => text,
                                    Leaf::Invalid => "?invalid".to_string(),
                                }
                            })
                            .collect();
                        if canon_alias(out, file_id, decl_name, &canon_args, cx, depth.saturating_sub(1))
                            == Leaf::Invalid
                        {
                            out.push_str("?invalid");
                        }
                    }
                }
            }
            if nullable {
                out.push('?');
            }
        }
        Ty::Record { shape, nullable } => {
            let shape = decl_unit.type_args.record_shape(shape);
            out.push('(');
            let mut first = true;
            for field in shape.positional.iter() {
                if !first {
                    out.push(',');
                }
                first = false;
                if canon_alias_ty(out, *field, decl_file, decl_unit, subst, cx, depth)
                    == Leaf::Invalid
                {
                    out.push_str("?invalid");
                }
            }
            for (field_name, field) in shape.named.iter() {
                if !first {
                    out.push(',');
                }
                first = false;
                if canon_alias_ty(out, *field, decl_file, decl_unit, subst, cx, depth)
                    == Leaf::Invalid
                {
                    out.push_str("?invalid");
                }
                out.push(' ');
                out.push_str(cx.strings.resolve(*field_name));
            }
            out.push(')');
            if nullable {
                out.push('?');
            }
        }
        Ty::Function { shape, nullable } => {
            let shape = decl_unit.type_args.function_shape(shape);
            if canon_alias_ty(out, shape.return_ty, decl_file, decl_unit, subst, cx, depth)
                == Leaf::Invalid
            {
                out.push_str("?invalid");
            }
            out.push_str(" Function(");
            let mut first = true;
            for formal in shape.positional.iter() {
                if !first {
                    out.push(',');
                }
                first = false;
                if canon_alias_ty(out, *formal, decl_file, decl_unit, subst, cx, depth)
                    == Leaf::Invalid
                {
                    out.push_str("?invalid");
                }
            }
            for (formal_name, formal) in shape.named.iter() {
                if !first {
                    out.push(',');
                }
                first = false;
                if canon_alias_ty(out, *formal, decl_file, decl_unit, subst, cx, depth)
                    == Leaf::Invalid
                {
                    out.push_str("?invalid");
                }
                out.push(' ');
                out.push_str(cx.strings.resolve(*formal_name));
            }
            out.push(')');
            out.push_str(&format!("/{}", shape.required_positional));
            if nullable {
                out.push('?');
            }
        }
    }
    Leaf::Resolved
}

/// The written-spelling display of `ty` for a message slot: the resolved
/// head's leaf name (a prefixed `p.C` prints `C` — the oracle's
/// `getDisplayString` writes the element's name, never the import prefix)
/// with the written arguments rendered the same way; every other shape
/// reads `Ty::render`'s official form.
fn display_ty(ty: Ty, unit: &FileElements, strings: &StringTable) -> String {
    match ty {
        Ty::Named {
            name,
            nullable,
            args,
        } => {
            let spelling = strings.resolve(name);
            let leaf = spelling.rsplit('.').next().unwrap_or(spelling);
            let mut text = leaf.to_owned();
            let list = unit.type_args.get(args);
            if !list.is_empty() {
                text.push('<');
                for (index, arg) in list.iter().enumerate() {
                    if index > 0 {
                        text.push_str(", ");
                    }
                    text.push_str(&display_ty(*arg, unit, strings));
                }
                text.push('>');
            }
            if nullable {
                text.push('?');
            }
            text
        }
        Ty::UNKNOWN => "dynamic".to_string(),
        _ => ty.render(strings, &unit.type_args),
    }
}

/// The in-scope type-parameter names of the augmentation's own spelling:
/// its declared `<T, …>` group plus, for a member augment inside an
/// `augment class C<T>`, the enclosing container fragment's group — the
/// scope a member's written `T` resolves to.
fn aug_param_names(aug: &AugmentDecl, own: &FileElements) -> Vec<StringId> {
    let mut names: Vec<StringId> = aug.type_params.iter().map(|p| p.name).collect();
    if let Some(container) = aug.container {
        for decl in own.augmentations() {
            if decl.container.is_none()
                && decl.name == container
                && matches!(
                    decl.kind,
                    AugKind::Class
                        | AugKind::Mixin
                        | AugKind::Enum
                        | AugKind::Extension
                        | AugKind::ExtensionType
                )
            {
                names.extend(decl.type_params.iter().map(|p| p.name));
            }
        }
    }
    names
}

/// The intro face's in-scope type-parameter names: the face's own group
/// plus, for a member or constructor face, the declaring container's
/// group — the scope the intro's written `T` resolves to.
fn intro_params(intro_unit: &FileElements, aug: &AugmentDecl, face: &Face<'_>) -> Vec<StringId> {
    let (names, _) = face.type_params();
    let mut out: Vec<StringId> = names.to_vec();
    if let Some(container_name) = aug.container
        && let Some(container) = intro_unit.container(container_name)
    {
        out.extend_from_slice(&container.type_param_names);
    }
    out
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

/// Whether an `augment` declaration joins the named accessor sub-chain of a
/// variable-family element — the [`Entry::in_accessor`] membership rule
/// read straight off the capture (upstream Dart SDK source,
/// `pkg/analyzer/lib/src/summary2/element_builder.dart`: a variable
/// fragment appends an induced getter (always) and an induced setter
/// (non-final) alongside its own slot; an explicit accessor fragment
/// appends only to its own chain).
fn aug_in_accessor(aug: &AugmentDecl, accessor: Accessor) -> bool {
    match accessor {
        Accessor::Variable => matches!(
            aug.kind,
            AugKind::Field | AugKind::TopLevelVar | AugKind::Getter | AugKind::Setter
        ),
        Accessor::Getter => matches!(
            aug.kind,
            AugKind::Getter | AugKind::Field | AugKind::TopLevelVar
        ),
        Accessor::Setter => {
            aug.kind == AugKind::Setter
                || (matches!(aug.kind, AugKind::Field | AugKind::TopLevelVar) && !aug.is_final)
        }
    }
}

/// The fragments of `key`'s chain that follow the introductory declaration
/// at `intro_offset` in `file`: every same-key `augment` of a later unit in
/// `allUnits` walk order, plus this unit's own augments written after the
/// intro (upstream `firstFragment.augmentations` — the `ElementBuilder`
/// appends in source order within a unit, so an earlier same-unit augment
/// never reaches this intro's element).
fn later_augments<'a>(
    own: &'a FileElements,
    index: &'a ElementIndex,
    after: &'a [FileId],
    intro_offset: u32,
    key: (Option<StringId>, StringId, Space),
    strings: &StringTable,
) -> Vec<&'a AugmentDecl> {
    let mut out: Vec<&AugmentDecl> = Vec::new();
    for aug in own.augmentations() {
        if augment_key(aug, strings) == key && aug.augment_kw.offset > intro_offset {
            out.push(aug);
        }
    }
    for &unit in after {
        let Some(elements) = index.get(unit) else {
            continue;
        };
        for aug in elements.augmentations() {
            if augment_key(aug, strings) == key {
                out.push(aug);
            }
        }
    }
    out
}

/// `_checkForFunctionBodyCompleteness` and
/// `_checkForIncompleteInducedAccessors` (upstream Dart SDK source,
/// `pkg/analyzer/lib/src/generated/error_verifier.dart:5256-5321` and
/// `:5415-5456`): the intro-side completeness walks of the augmentations
/// experiment.
///
/// The executable arm runs for every method/getter/setter intro except the
/// instance members of a class, enum, or mixin (`:5273-5280` — those are
/// validated for the whole interface by other faces) and except a fragment
/// that is itself an augmentation (`:5267-5270` — `MemberEntry::is_augment`
/// is the declaration's own keyword). A chain that stays incomplete with
/// more than one fragment reports `function_not_complete_after_augmentations`
/// at the name token (`:5313-5319`); a singleton stays silent here —
/// `missing_function_body` / `extension_declares_abstract_member` /
/// `extension_type_with_abstract_member` own that arm (`:5286-5311`, other
/// faces' codes). A complete fragment anywhere in the chain silences the
/// row (`:5282-5284`).
///
/// The field arm runs for a `static` field group written `abstract`
/// (`:1073-1081`): the variable's induced getter and (non-`final`) setter
/// elements each report their `*_not_complete_after_augmentations` shape
/// when no chain fragment is complete and either accessor chain grew
/// (`:5430-5455` — the `without_body` sibling of a singleton chain is again
/// another face's code). The enum constants never qualify — the `abstract`
/// bit is never set on their rows.
///
/// Model limitations (each silent on the oracle sets): the top-level arms —
/// a bodiless `void f();` or `abstract int x;` intro — have no name span or
/// completeness/abstract bits on `AugTarget`/`TopDecl` to anchor or judge,
/// so only the member faces emit.
fn executable_body_diagnostics(
    file: FileId,
    own: &FileElements,
    index: &ElementIndex,
    graph: &LibraryGraph,
    lib: LibraryId,
    strings: &StringTable,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let units = graph.unit_files(lib);
    let after: &[FileId] = match units.iter().position(|&unit| unit == file) {
        Some(pos) => &units[pos + 1..],
        None => &[],
    };
    for &container_name in own.container_order() {
        // An `augment` container head's member rows are fragment members,
        // not introductions — the same filter the chain table applies.
        if index.container_row_is_fragment(graph, lib, file, container_name) {
            continue;
        }
        let Some(container) = own.container(container_name) else {
            continue;
        };
        let interface_like = matches!(
            container.kind,
            ContainerKind::Class | ContainerKind::Enum | ContainerKind::Mixin
        );
        for (member_name, member) in container.members.iter() {
            // `firstFragment.isAugmentation` — the declaration's own
            // `augment` keyword makes it a fragment, never an intro.
            if member.is_augment
                || member.name_range.offset == MemberEntry::MISSING_NAME_RANGE.offset
            {
                continue;
            }
            match member.kind {
                MemberKind::Method | MemberKind::Getter | MemberKind::Setter => {
                    executable_member_row(
                        &mut out,
                        file,
                        own,
                        index,
                        after,
                        container_name,
                        *member_name,
                        member,
                        interface_like,
                        strings,
                    );
                }
                MemberKind::Field => {
                    induced_accessor_rows(
                        &mut out,
                        file,
                        own,
                        index,
                        after,
                        container_name,
                        *member_name,
                        member,
                        strings,
                    );
                }
            }
        }
    }
    out
}

/// The executable arm of [`executable_body_diagnostics`]: one member
/// intro's chain, anchored at its name token.
#[expect(clippy::too_many_arguments)] // the walk's shared context columns
fn executable_member_row(
    out: &mut Vec<Diagnostic>,
    file: FileId,
    own: &FileElements,
    index: &ElementIndex,
    after: &[FileId],
    container_name: StringId,
    member_name: StringId,
    member: &MemberEntry,
    interface_like: bool,
    strings: &StringTable,
) {
    // Instance members of a class-family container are validated for the
    // whole interface (upstream `:5273-5280`); extension and extension-type
    // members of either staticness stay in the face.
    if !member.is_static && interface_like {
        return;
    }
    // A complete intro (a body, or `external`) satisfies the chain by
    // itself — `fragments.any(isComplete)` (`:5282-5284`).
    if member.is_external || !member.is_abstract {
        return;
    }
    let aug_kind = match member.kind {
        MemberKind::Method => AugKind::Method,
        MemberKind::Getter => AugKind::Getter,
        MemberKind::Setter => AugKind::Setter,
        MemberKind::Field => unreachable!(),
    };
    let space = if member.is_static {
        Space::StaticMember
    } else {
        Space::Member
    };
    let key = (
        Some(container_name),
        property_name(member_name, aug_kind, strings),
        space,
    );
    // The element's own fragment chain: a method element collects only
    // method fragments; a getter/setter element also collects the induced
    // accessor fragments of merging variable declarations (upstream
    // `_handleInstanceFieldFragment` appends the induced fragment to the
    // accessor element it finds).
    let accessor = match member.kind {
        MemberKind::Method => None,
        MemberKind::Getter => Some(Accessor::Getter),
        MemberKind::Setter => Some(Accessor::Setter),
        MemberKind::Field => unreachable!(),
    };
    let mut fragments = 0usize;
    let mut complete = false;
    for aug in later_augments(
        own,
        index,
        after,
        member.name_range.offset,
        key,
        strings,
    ) {
        match accessor {
            Some(chain) if !aug_in_accessor(aug, chain) => continue,
            None if aug.kind != aug_kind => continue,
            _ => {}
        }
        fragments += 1;
        complete |= aug.is_complete;
    }
    if fragments == 0 || complete {
        return;
    }
    let code = SemanticCode::FunctionNotCompleteAfterAugmentations;
    let display = strings.resolve(key.1);
    out.push(row(file, member.name_range, code, code.message(display, "")));
}

/// The induced-accessor arm of [`executable_body_diagnostics`]: a
/// `static abstract` field's getter and setter chains.
#[expect(clippy::too_many_arguments)] // the walk's shared context columns
fn induced_accessor_rows(
    out: &mut Vec<Diagnostic>,
    file: FileId,
    own: &FileElements,
    index: &ElementIndex,
    after: &[FileId],
    container_name: StringId,
    member_name: StringId,
    member: &MemberEntry,
    strings: &StringTable,
) {
    // Upstream gates the face on `node.isStatic && abstractKeyword != null`
    // (`:1073-1081`): the member row's `is_abstract` is the field group's
    // `abstract` keyword (and an abstract field is never `external`).
    if !member.is_static || !member.is_abstract {
        return;
    }
    let key = (
        Some(container_name),
        property_name(member_name, AugKind::Field, strings),
        Space::StaticMember,
    );
    let augments = later_augments(
        own,
        index,
        after,
        member.name_range.offset,
        key,
        strings,
    );
    // Each accessor element's chain: the intro's induced fragment plus
    // every merging fragment that lands in that accessor's element. A
    // `final` field has no setter at all (upstream `fieldFragment.hasSetter`
    // — the chain count is then zero, `?? 0`).
    let chain = |accessor: Accessor| -> (usize, bool) {
        let mut len = 1usize; // the intro's induced fragment
        let mut complete = member.is_external || !member.is_abstract;
        for aug in &augments {
            if !aug_in_accessor(aug, accessor) {
                continue;
            }
            len += 1;
            complete |= aug.is_complete;
        }
        (len, complete)
    };
    let (getter_len, getter_complete) = chain(Accessor::Getter);
    let (setter_len, setter_complete) = if member.is_final {
        (0, true)
    } else {
        chain(Accessor::Setter)
    };
    // `hasAugmentations`: either accessor element grew past its intro
    // (`:5428-5432`).
    let has_augmentations = getter_len > 1 || setter_len > 1;
    if !has_augmentations {
        return;
    }
    let display = strings.resolve(key.1);
    if !getter_complete {
        let code = SemanticCode::InducedGetterNotCompleteAfterAugmentations;
        out.push(row(file, member.name_range, code, code.message(display, "")));
    }
    if !member.is_final && !setter_complete {
        let code = SemanticCode::InducedSetterNotCompleteAfterAugmentations;
        out.push(row(file, member.name_range, code, code.message(display, "")));
    }
}

// The #3201 pins — the factory-body completeness face
// (`factory_without_body`) with the augmentations experiment on.
#[cfg(test)]
#[path = "ticket_3201_tests.rs"]
mod ticket_3201_tests;

// The #3556 pins — canonical-type comparison for the shape checks
// (written/judged split) and the preceding-fragment chain.
#[cfg(test)]
#[path = "ticket_3556_tests.rs"]
mod ticket_3556_tests;

// The #3569 pins — the intro-side completeness walks
// (`function_not_complete_after_augmentations` member arm and the induced
// getter/setter `static abstract` field arm).
#[cfg(test)]
#[path = "ticket_3569_tests.rs"]
mod ticket_3569_tests;
