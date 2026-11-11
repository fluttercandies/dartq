//! Per-library member enumeration over the resolution layer (#117): the
//! library scope face, the in-library scoped lookup, and the name-level
//! member walk — the query surface the `undefined_*`/`wrong_number_*`
//! diagnostic tickets consume.
//!
//! # The library face (adjudicated essential #1)
//!
//! [`LibraryScopes`] materializes, per library, the public name face with a
//! micro-kind tag: local declarations and re-exports from the graph's export
//! scope (a declaration is identified by `Decl(file)` + looked-up name —
//! #117's contract; the kind comes from the declaring file's
//! [`FileElements`]), the exported names of every unprefixed import
//! (combinator filtered, never displacing local names), plus one `prefix`
//! marker per import prefix. A name
//! declared outside the indexed files (`dart:` SDK, an unanalyzed
//! dependency) surfaces as [`ScopeKind::External`] — present, but
//! kind-sensitive diagnostics must be suppressed.
//!
//! # In-library scope (the private-name face)
//!
//! The export scope only carries public names, but library-internal code
//! sees private ones too. [`lookup_scoped`] is the in-library chain:
//! the current file's own declarations first (private included, locals beat
//! imports), then the [`Namespaces`] lookup (import face, build-time
//! combinator filtering, ambiguity with all candidates).
//!
//! # Member walk (adjudicated essentials #2–#3)
//!
//! [`member_lookup`] resolves a container by name (file-local first, then
//! the namespace) and walks superclass name edges, returning the first hit.
//! A miss is **not** proof of undefined: the caller must still check the
//! extension pool ([`FileElements::extension_faces`]) of the library's
//! files before reporting an instance-scope `undefined_*`.
//!
//! # Interface-membership walk
//!
//! [`implements_interface`] answers the transitive supertype question a
//! rule asks as `type.implementsInterface(name, library)` (upstream Dart
//! SDK source, pkg/linter/lib/src/extensions.dart:324-331) over the
//! declared supertype graph — the same name-level edge discipline the
//! member walk applies, plus a small `dart:core` face for the names no
//! index can carry.
//!
//! # The part/part-of seam
//!
//! The graph assigns files to libraries and carries the part tree, so a use
//! site resolves in **its own file's** combined import scope: the locals
//! layer is library-wide (every top-level declaration of every unit, privates
//! included), while the imports and prefixes belong to the file that wrote
//! them and chain up the part tree to the library file
//! ([`Namespaces::lookup_in_file`], [`Namespaces::lookup_prefixed_in_file`]).
//! Callers pass the owning library and the current file explicitly; a name a
//! sibling part imported is not visible here.

use dartq_resolution::{LibraryGraph, LibraryId, LocalLookup, Lookup, NamespaceEntry, Namespaces};
use dartq_source::{FileId, StringId, StringTable};
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::VecDeque;
use std::sync::{Arc, OnceLock};

use crate::collector::{DUP_GETTER, DUP_OTHER, DUP_PAIR, DUP_SETTER};
use crate::model::{
    ArgListId, AugKind, AugmentDecl, Container, ContainerKind, DuplicateDecl, FileElements,
    FnShapeId, Formals, MemberEntry, MemberKind, MicroKind, NameMap, RecordShapeId, TopDecl, Ty,
};

/// Per-file element models, keyed by file. Sized by the analyzed file set.
///
/// Models are shared by [`Arc`]: inserting from a shared store is a refcount
/// bump, so indexing beyond the diagnosing library — the cross-library
/// member-walk face (#236's zero-clone design) — costs no deep clones.
///
/// The index also carries the augmentation-fragment layer (#3418): the
/// per-library element faces the container anchor answers with, built from
/// the collected models on first use (see [`ElementFragments`]).
#[derive(Debug, Default)]
pub struct ElementIndex {
    files: FxHashMap<FileId, Arc<FileElements>>,
    /// Whether any indexed unit writes a container `augment` head — the gate
    /// that keeps the fragment layer off every analysis that uses no
    /// augmentations (one scan of the model set, once).
    augmented: OnceLock<bool>,
    /// One cell per `LibraryId::index`, holding that library's fragment sets;
    /// `None` is a library whose containers are never augmented.
    fragments: OnceLock<Vec<OnceLock<Option<Box<[ElementFragments]>>>>>,
    /// Per-library merged-fragment `duplicate_definition` enrichment
    /// (#3378): one cell per `LibraryId::index`, parallel to
    /// [`Self::fragments`]. Each `library_fragments` build populates its
    /// own cell with the units that gained rows. `get` scans the cells —
    /// the number of libraries is small and `get` already walks the file
    /// table.
    enriched_by_file: OnceLock<Vec<OnceLock<Option<FxHashMap<FileId, Arc<FileElements>>>>>>,
}

impl ElementIndex {
    /// Stores (or replaces, on re-analysis) the model of `file`. An
    /// [`Arc`] clone shares the model; an owned value is taken.
    pub fn insert(&mut self, file: FileId, elements: impl Into<Arc<FileElements>>) {
        self.files.insert(file, elements.into());
        // A new unit can introduce or fragment a container another library
        // already resolved; both caches are pure functions of the model set.
        self.augmented = OnceLock::new();
        self.fragments = OnceLock::new();
        self.enriched_by_file = OnceLock::new();
    }

    /// The model of `file`, if analyzed. When the fragment layer has run,
    /// the enriched variant wins: it carries the merged-fragment duplicate
    /// rows the per-unit collector cannot see (#3378). A unit with no
    /// appended rows has no entry — the raw model answers.
    #[must_use]
    pub fn get(&self, file: FileId) -> Option<&FileElements> {
        if let Some(cells) = self.enriched_by_file.get() {
            for cell in cells.iter() {
                if let Some(Some(map)) = cell.get()
                    && let Some(arc) = map.get(&file)
                {
                    return Some(arc.as_ref());
                }
            }
        }
        self.files.get(&file).map(Arc::as_ref)
    }

    /// The element face of the container `name` declared in `owner`, with the
    /// file that owns it: the unit's own row when the library writes no
    /// `augment` fragment of the name, else the merged fragment face
    /// ([`ElementFragments`]). `None` when `owner` declares no such row.
    fn element_face<'a>(
        &'a self,
        graph: &LibraryGraph,
        lib: LibraryId,
        owner: FileId,
        name: StringId,
    ) -> Option<(&'a Container, FileId)> {
        if let Some(face) = self.fragment_face(graph, lib, owner, name) {
            return Some((face, owner));
        }
        Some((self.get(owner)?.container(name)?, owner))
    }

    /// The merged fragment face `owner` answers for `name`, if the library
    /// augments that container and `owner` writes one of its fragments.
    fn fragment_face<'a>(
        &'a self,
        graph: &LibraryGraph,
        lib: LibraryId,
        owner: FileId,
        name: StringId,
    ) -> Option<&'a Container> {
        if !self.has_augmented_containers() {
            return None;
        }
        // The fragments live in the library that declares the element, which
        // is not necessarily the library the spelling resolves in (a prefixed
        // or imported anchor).
        let lib = graph.library_of_file(owner).unwrap_or(lib);
        let elements = self.library_fragments(graph, lib)?;
        let element = &elements[elements.binary_search_by_key(&name, |e| e.name).ok()?];
        let at = element
            .faces
            .binary_search_by_key(&owner, |(file, _)| *file)
            .ok()?;
        Some(&element.faces[at].1)
    }

    /// Whether `unit`'s container row for `name` is an `augment` fragment's
    /// face rather than the element's own — the augmentation verifier's row
    /// filter (`populate`): a fragment row never joins the chain table as an
    /// introductory declaration. `false` for a name no unit augments.
    pub(crate) fn container_row_is_fragment(
        &self,
        graph: &LibraryGraph,
        lib: LibraryId,
        unit: FileId,
        name: StringId,
    ) -> bool {
        if !self.has_augmented_containers() {
            return false;
        }
        let Some(elements) = self.library_fragments(graph, lib) else {
            return false;
        };
        let Ok(at) = elements.binary_search_by_key(&name, |e| e.name) else {
            return false;
        };
        let element = &elements[at];
        element.intro != Some(unit) && element.fragments.contains(&unit)
    }

    /// Whether any indexed unit writes a container `augment` head.
    fn has_augmented_containers(&self) -> bool {
        *self.augmented.get_or_init(|| {
            self.files.values().any(|elements| {
                elements
                    .augmentations()
                    .iter()
                    .any(writes_container_fragment)
            })
        })
    }

    /// The library's fragment sets, built on first use: one pass over the
    /// library's unit models, cached for the rest of the analysis.
    fn library_fragments(
        &self,
        graph: &LibraryGraph,
        lib: LibraryId,
    ) -> Option<&[ElementFragments]> {
        let cells = self
            .fragments
            .get_or_init(|| (0..graph.len()).map(|_| OnceLock::new()).collect());
        cells
            .get(lib.index() as usize)?
            .get_or_init(|| {
                let (fragments, enriched) = build_fragments(graph, self, lib)?;
                // Populate the per-lib enriched cell for `get` (#3378).
                let enriched_cells = self
                    .enriched_by_file
                    .get_or_init(|| (0..graph.len()).map(|_| OnceLock::new()).collect());
                if let Some(cell) = enriched_cells.get(lib.index() as usize) {
                    let _ = cell.set(if enriched.is_empty() {
                        None
                    } else {
                        Some(enriched)
                    });
                }
                Some(fragments)
            })
            .as_deref()
    }
}

/// One augmented container's fragment set (#3418).
///
/// Upstream an `augment` head declares a FRAGMENT of the element its
/// introduction declares (`element.firstFragment` is the introduction,
/// upstream Dart SDK source, pkg/_fe_analyzer_shared augmentation
/// handling), so the element's member face is the union of every fragment's
/// members — whichever unit the spelling is written in. The units are
/// collected independently, so the union cannot be a collection-time
/// artifact: it is built once per library here, from the collected models.
///
/// # The cross-pool gate
///
/// A `Ty` carrying an argument-list/record/function slot is only meaningful
/// in the pool of the unit that interned it ([`crate::model::ArgPool`]), and
/// the diagnostic boundary re-interns a foreign entry through the pool of the
/// file the walk resolved it in ([`ElementFragments`]'s faces are answered
/// under that unit's file). A fragment member therefore joins another unit's
/// face only when it carries no such slot ([`entry_is_portable`]); a
/// pool-carrying fragment member stays on its own unit's face — the
/// under-resolution #3418 records for the engine-side multi-face walk, never
/// a foreign slot read (an out-of-range slot is a panic) or a wrong type.
#[derive(Debug)]
struct ElementFragments {
    /// The container name.
    name: StringId,
    /// The introducing unit — the fragment whose container row is the
    /// element's own face. `None` when every fragment is an `augment` head:
    /// no element exists upstream, so nothing is merged
    /// (`augmentation_without_declaration` owns that shape).
    intro: Option<FileId>,
    /// The units writing an `augment` head for this name, in library unit
    /// order.
    fragments: Box<[FileId]>,
    /// One face per unit of the element, file-ordered: that unit's own row
    /// with every other fragment's declarations folded in — members (#3418)
    /// and, since #3516, the columns an augmentation can grow: enum
    /// constants, constructor selectors and formals, `const`-constructor
    /// presence, the `extends` gap and the `with`/`implements`/`on` edges
    /// ([`admit_fragment`]), plus the re-derived implicit default
    /// ([`CtorDeclarations`]). Empty for an introduction-less element.
    faces: Box<[(FileId, Container)]>,
}

/// Whether `aug` declares a fragment of a named container (the class family;
/// an executable or variable `augment` is a member fragment, and a member
/// lookup reaches it through the container face or not at all).
fn writes_container_fragment(aug: &AugmentDecl) -> bool {
    aug.container.is_none()
        && matches!(
            aug.kind,
            AugKind::Class
                | AugKind::Mixin
                | AugKind::Enum
                | AugKind::Extension
                | AugKind::ExtensionType
        )
}

/// The per-library fragment sets: every container the library's units
/// augment, with the introduction's identity and one merged face per unit.
///
/// Order is fixed by the library's unit order and each name's row order, so
/// the build is independent of which unit triggered it and of the order the
/// units were collected in (a fragment declaring first and one declaring
/// last produce the same face).
///
/// Returns `(fragments, enriched)` — the merged-fragment
/// `duplicate_definition` enrichment per unit (#3378): cloned
/// `FileElements` carrying the `DuplicateDecl` rows the merged claim
/// evaluation found. Empty when no unit gains a row.
fn build_fragments(
    graph: &LibraryGraph,
    index: &ElementIndex,
    lib: LibraryId,
) -> Option<(
    Box<[ElementFragments]>,
    FxHashMap<FileId, Arc<FileElements>>,
)> {
    let units = graph.unit_files(lib);
    // name → (unit, that unit's `augment`-head commits) in library order.
    let mut order: FxHashMap<StringId, Vec<(FileId, usize)>> = FxHashMap::default();
    for &unit in units {
        let Some(elements) = index.get(unit) else {
            continue;
        };
        for aug in elements.augmentations() {
            if !writes_container_fragment(aug) {
                continue;
            }
            let writers = order.entry(aug.name).or_default();
            match writers.last_mut() {
                Some((last, commits)) if *last == unit => *commits += 1,
                _ => writers.push((unit, 1)),
            }
        }
    }
    if order.is_empty() {
        return None;
    }
    let mut elements: Vec<ElementFragments> = Vec::with_capacity(order.len());
    for (name, writers) in order {
        // The introduction: the first unit on the library's unit order whose
        // row for the name is not an `augment` face — it commits the name
        // more often than it writes `augment` heads of it.
        let intro = units.iter().copied().find(|unit| {
            let Some(unit_elements) = index.get(*unit) else {
                return false;
            };
            if unit_elements.container(name).is_none() {
                return false;
            }
            let commits = unit_elements
                .container_order()
                .iter()
                .filter(|other| **other == name)
                .count();
            let augmenting = writers
                .iter()
                .find(|(writer, _)| writer == unit)
                .map_or(0, |(_, count)| *count);
            commits > augmenting
        });
        let mut faces: Vec<(FileId, Container)> = Vec::new();
        if let Some(intro) = intro
            && let Some(base) = index.get(intro).and_then(|e| e.container(name)).cloned()
        {
            let mut owners: Vec<FileId> = vec![intro];
            owners.extend(
                writers
                    .iter()
                    .map(|(unit, _)| *unit)
                    .filter(|unit| *unit != intro),
            );
            for &owner in &owners {
                let Some(own) = index.get(owner).and_then(|e| e.container(name)) else {
                    continue;
                };
                // The owner's own rows keep their own pool; every other
                // fragment contributes its members with pool-carrying `Ty`s
                // sanitized to `Ty::Unknown` — the name resolves (the
                // fragment scope merge), the foreign slot is never readable
                // through this face. `NameMap`'s from-iterator keeps the
                // FIRST row of a duplicate spelling, so the owner's own
                // declaration wins over a fragment's.
                let mut rows: Vec<(StringId, MemberEntry)> = own.members.iter().cloned().collect();
                for &other in &owners {
                    if other == owner {
                        continue;
                    }
                    let Some(face) = index.get(other).and_then(|e| e.container(name)) else {
                        continue;
                    };
                    rows.extend(
                        face.members
                            .iter()
                            .map(|(member, entry)| (*member, sanitize_entry(entry))),
                    );
                }
                // The element's OTHER columns fold the same way (#3516) —
                // `members` alone left an `augment` fragment's enum
                // constants, constructors, `const`-ability, superclass edge
                // and `with`/`implements`/`on` edges invisible to every
                // lookup made in another unit. The seed is the
                // introduction's row (the element's identity: kind,
                // modifiers, type parameters, metadata) and each fragment
                // adds its declarations in element order, the owner's own
                // fragment verbatim and every other one sanitized
                // ([`admit_fragment`]).
                let mut face = Container {
                    members: rows.into_iter().collect(),
                    ..base.clone()
                };
                // A face belonging to a fragment unit reads the
                // introduction's constructor tables across a pool boundary.
                if owner != intro {
                    let ctor_formals = sanitize_formal_table(&face.ctor_formals);
                    let factory_formals = sanitize_formal_table(&face.factory_formals);
                    face.ctor_formals = ctor_formals;
                    face.factory_formals = factory_formals;
                }
                let mut ctors = CtorDeclarations::of(&base, name);
                for &other in &owners {
                    if other == intro {
                        continue;
                    }
                    let Some(source) = index.get(other).and_then(|e| e.container(name)) else {
                        continue;
                    };
                    admit_fragment(&mut face, source, other != owner);
                    ctors.admit(source, name);
                }
                face.has_unnamed_constructor = ctors.settled();
                faces.push((owner, face));
            }
            faces.sort_by_key(|(file, _)| *file);
        }
        elements.push(ElementFragments {
            name,
            intro,
            fragments: writers
                .into_iter()
                .map(|(unit, _)| unit)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            faces: faces.into_boxed_slice(),
        });
    }
    elements.sort_by_key(|element| element.name);

    // ── #3378: merged-fragment duplicate evaluation ──────────────────────
    // Upstream keys `_checkClassMembers` on `fragment.element
    // .firstFragment` — the merged element's context — so a member the
    // augment fragment repeats reports `duplicate_definition` at the
    // fragment's declaration. The per-unit collector cannot see the other
    // fragments; this pass replays the merged claim scope over every
    // fragment's members in element order (introduction first, then
    // fragments in library unit order) and collects the `DuplicateDecl`
    // rows the unit-local pass could not produce.
    //
    // # Claim shapes (mirroring `member_dup_claim`, collector.rs:953-981)
    //
    // `MemberKind` maps to the claim bits the collector's call sites use:
    // field → `DUP_GETTER` + `DUP_SETTER` when `defines_setter`
    // (approximated by `!is_final` — the `late final` split is a
    // same-unit concern the merged pass cannot reach); getter →
    // `DUP_GETTER`; setter → `DUP_SETTER`; method → `DUP_OTHER`. The
    // scope key is the `members` map key verbatim — for a setter this is
    // the `name=` spelling rather than the stripped bare name upstream
    // uses, so an explicit `set foo` after a field's induced `foo` setter
    // is the one cross-unit claim pair the merged pass cannot normalize
    // (documented residual).
    // DUP_* constants imported from collector.rs — the single source of
    // truth for the member-scope claim bits.
    let mut enriched: FxHashMap<FileId, Arc<FileElements>> = FxHashMap::default();
    for element in &elements {
        let Some(intro) = element.intro else {
            continue;
        };
        // `(claims, claiming_unit)` per name — a collision reports only
        // when the prior claim came from a DIFFERENT unit (the unit-local
        // `member_scopes` already reports same-unit collisions).
        let mut instance: FxHashMap<StringId, (u8, FileId)> = FxHashMap::default();
        let mut statics: FxHashMap<StringId, (u8, FileId)> = FxHashMap::default();
        let mut appended: Vec<(FileId, Vec<DuplicateDecl>)> = Vec::new();
        // Claims in element order: introduction first, then fragments in
        // library unit order — the same sequence upstream's merged
        // `_InstanceElementContext` walks.
        let claim_order: Vec<FileId> = std::iter::once(intro)
            .chain(element.fragments.iter().copied().filter(|f| *f != intro))
            .collect();
        for unit in claim_order {
            let Some(elements) = index.get(unit) else {
                continue;
            };
            let Some(container) = elements.container(element.name) else {
                continue;
            };
            // Source order within the fragment — `members` is name-sorted,
            // so restore the declaration sequence by `name_range`.
            let mut members: Vec<(&StringId, &MemberEntry)> =
                container.members.iter().map(|(k, v)| (k, v)).collect();
            members.sort_by_key(|(_, entry)| entry.name_range.offset);
            let mut unit_dups: Vec<DuplicateDecl> = Vec::new();
            for (name, entry) in members {
                // An augmentation member neither checks nor claims
                // (`fragment.isAugmentation` returns before the scope claim,
                // member_duplicate_definition_verifier.dart:307-309), so it
                // joins no merged replay: a fragment member carrying its own
                // `augment` modifier reports nothing here however many
                // fragments repeat the name. A member WITHOUT the modifier
                // is an introduction the fragment makes and claims as usual.
                if entry.is_augment {
                    continue;
                }
                let claims: &[u8] = match entry.kind {
                    MemberKind::Field => {
                        // An enum constant claims only the getter face
                        // (upstream `_checkClassMembers` pushes the
                        // constant's induced getter into the static
                        // scope — collector.rs:3929-3931). A non-final
                        // field claims its induced getter+setter pair;
                        // `final`/`late final` with initializer claims
                        // getter only.
                        if container.kind == ContainerKind::Enum || entry.is_final {
                            &[DUP_GETTER]
                        } else {
                            &[DUP_GETTER, DUP_SETTER]
                        }
                    }
                    MemberKind::Getter => &[DUP_GETTER],
                    MemberKind::Setter => &[DUP_SETTER],
                    MemberKind::Method => &[DUP_OTHER],
                };
                let scope = if entry.is_static {
                    &mut statics
                } else {
                    &mut instance
                };
                // Claims a fragment (non-intro) member makes always report —
                // augment members neither check nor claim in the unit-local
                // `member_scopes`, so the unit-local pass could not have
                // seen the collision. An intro member's own unit claims
                // already reported same-unit collisions.
                let is_fragment_member = unit != intro;
                for &claim in claims {
                    let prior = scope.get(name).copied();
                    match prior {
                        None => {
                            scope.insert(*name, (claim, unit));
                        }
                        Some((DUP_GETTER, prior_unit)) if claim == DUP_SETTER => {
                            scope.insert(*name, (DUP_PAIR, prior_unit));
                        }
                        Some((DUP_SETTER, prior_unit)) if claim == DUP_GETTER => {
                            scope.insert(*name, (DUP_PAIR, prior_unit));
                        }
                        Some((_, prior_unit)) if prior_unit != unit || is_fragment_member => {
                            // Mirror `record_duplicate`'s absorption
                            // (collector.rs:904-913): a field's getter and
                            // setter claims each collide at the same name
                            // token, and the oracle's pipeline folds those
                            // two reports into one row — only a
                            // back-to-back identical repeat is absorbed.
                            // The merged cross-unit claims are always
                            // members, never formal parameters, so the pair
                            // code (`duplicate_field_formal_parameter`)
                            // never applies here.
                            let row = DuplicateDecl {
                                name: *name,
                                range: entry.name_range,
                                field_formal: false,
                            };
                            if unit_dups.last() != Some(&row) {
                                unit_dups.push(row);
                            }
                        }
                        _ => {}
                    }
                }
            }
            if !unit_dups.is_empty() {
                appended.push((unit, unit_dups));
            }
        }
        // Merge the appended rows into each unit's enriched model. A unit
        // already enriched by a previous element extends its row set rather
        // than re-cloning from raw — `insert` would drop the earlier rows.
        for (unit, dups) in appended {
            let elements = if let Some(existing) = enriched.get(&unit) {
                let mut elements = (**existing).clone();
                let mut all: Vec<DuplicateDecl> = elements.duplicates.iter().copied().collect();
                all.extend(dups);
                all.sort_by_key(|d| d.range.offset);
                elements.duplicates = all.into_boxed_slice();
                elements
            } else {
                let Some(raw) = index.files.get(&unit) else {
                    continue;
                };
                let mut elements = (**raw).clone();
                let mut all: Vec<DuplicateDecl> = elements.duplicates.iter().copied().collect();
                all.extend(dups);
                all.sort_by_key(|d| d.range.offset);
                elements.duplicates = all.into_boxed_slice();
                elements
            };
            enriched.insert(unit, Arc::new(elements));
        }
    }
    Some((elements.into_boxed_slice(), enriched))
}

/// Whether `ty` reads identically through every unit's argument pool: the
/// reserved empty slots are pool-independent, so a value carrying no other
/// slot can cross a unit boundary without re-interning.
fn ty_is_pool_free(ty: Ty) -> bool {
    match ty {
        Ty::Named { args, .. } => args == ArgListId::EMPTY,
        Ty::Record { shape, .. } => shape == RecordShapeId::EMPTY,
        Ty::Function { shape, .. } => shape == FnShapeId::EMPTY,
        _ => true,
    }
}

/// Whether `entry` crosses a unit boundary intact — every declared type it
/// carries is pool-free (see [`ty_is_pool_free`]). Names, spans, flags, and
/// the interned default/annotation texts ride the shared string table, so
/// they never block a merge.
fn entry_is_portable(entry: &MemberEntry) -> bool {
    ty_is_pool_free(entry.var_ty)
        && ty_is_pool_free(entry.return_ty)
        && entry.type_param_bounds.iter().copied().all(ty_is_pool_free)
        && entry.formals.as_ref().is_none_or(|formals| {
            formals
                .positional
                .iter()
                .chain(formals.named.iter())
                .all(|formal| ty_is_pool_free(formal.ty))
        })
}

/// `entry` with every pool-carrying `Ty` rewritten to `Ty::Unknown` — the
/// member joins another unit's merged face for name resolution, but the
/// foreign argument-pool slot is never exposed: the merged face reads a
/// `Ty::Unknown` where the original interned into the declaring unit's
/// [`crate::model::ArgPool`] (an out-of-range slot would panic). Pool-free
/// entries return a verbatim clone.
fn sanitize_entry(entry: &MemberEntry) -> MemberEntry {
    if entry_is_portable(entry) {
        return entry.clone();
    }
    let mut entry = entry.clone();
    if !ty_is_pool_free(entry.var_ty) {
        entry.var_ty = Ty::Unknown;
    }
    if !ty_is_pool_free(entry.return_ty) {
        entry.return_ty = Ty::Unknown;
    }
    for bound in entry.type_param_bounds.iter_mut() {
        if !ty_is_pool_free(*bound) {
            *bound = Ty::Unknown;
        }
    }
    if let Some(formals) = entry.formals.as_mut() {
        for formal in formals
            .positional
            .iter_mut()
            .chain(formals.named.iter_mut())
        {
            if !ty_is_pool_free(formal.ty) {
                formal.ty = Ty::Unknown;
            }
        }
    }
    entry
}

/// [`sanitize_entry`]'s constructor-table half: every pool-carrying declared
/// type of one formal list rewritten to `Ty::Unknown`. Names, positions, the
/// required bit and the naming mode ride the shared tables, so only the
/// declared types are lost — the same under-report-never-panic posture the
/// member gate takes.
fn sanitize_formals(formals: &Formals) -> Box<Formals> {
    let mut formals = formals.clone();
    for formal in formals
        .positional
        .iter_mut()
        .chain(formals.named.iter_mut())
    {
        if !ty_is_pool_free(formal.ty) {
            formal.ty = Ty::Unknown;
        }
    }
    Box::new(formals)
}

/// A `Container::ctor_formals`/`factory_formals` table with every list run
/// through [`sanitize_formals`] — the cross-pool gate for the columns #3516
/// folds into the merged face.
fn sanitize_formal_table(table: &NameMap<Box<Formals>>) -> NameMap<Box<Formals>> {
    table
        .iter()
        .map(|(selector, formals)| (*selector, sanitize_formals(formals)))
        .collect()
}

/// `base` followed by every name of `add` it lacks — a fragment's
/// declaration-order list joined onto the element's (enum constants,
/// constructor selectors, clause edges). The same rule the same-unit fold
/// applies (`collector::Builder::fold_fragment`).
fn union_names(base: &[StringId], add: &[StringId]) -> Box<[StringId]> {
    let mut names: Vec<StringId> = base.to_vec();
    for name in add {
        if !names.contains(name) {
            names.push(*name);
        }
    }
    names.into_boxed_slice()
}

/// Folds one `augment` fragment's declarations into the element's face — the
/// library-layer half of `collector::Builder::fold_fragment`, for the
/// fragments that live in another unit of the same library. The columns an
/// augmentation can grow join the element's: enum constants in declaration
/// order, the constructor selectors and their formals (keep-first — the
/// element's own spelling wins), `const`-constructor presence, the `extends`
/// gap, and the `with`/`implements`/`on` edges (`augment class C with M {}`
/// in a part file makes `C().m()` resolve). Columns the introduction owns
/// alone (kind, modifiers, type parameters, metadata, representation
/// formals) stay untouched. `foreign` applies the cross-pool gate to the
/// formal tables; the name-only columns need none.
fn admit_fragment(face: &mut Container, fragment: &Container, foreign: bool) {
    face.enum_constants = union_names(&face.enum_constants, &fragment.enum_constants);
    face.constructors = union_names(&face.constructors, &fragment.constructors);
    let formals = |table: &NameMap<Box<Formals>>| -> Vec<(StringId, Box<Formals>)> {
        table
            .iter()
            .map(|(selector, list)| {
                let list = if foreign {
                    sanitize_formals(list)
                } else {
                    list.clone()
                };
                (*selector, list)
            })
            .collect()
    };
    face.ctor_formals = face
        .ctor_formals
        .iter()
        .cloned()
        .chain(formals(&fragment.ctor_formals))
        .collect();
    face.factory_formals = face
        .factory_formals
        .iter()
        .cloned()
        .chain(formals(&fragment.factory_formals))
        .collect();
    face.has_const_constructor |= fragment.has_const_constructor;
    // `extends` is one edge — the introduction's superclass stands, a
    // fragment's fills the gap; the other three are lists the fragment adds
    // to.
    face.superclass = face.superclass.or(fragment.superclass);
    face.with_clause = union_names(&face.with_clause, &fragment.with_clause);
    face.implements_clause = union_names(&face.implements_clause, &fragment.implements_clause);
    face.on_clause = union_names(&face.on_clause, &fragment.on_clause);
}

/// One element's constructor declarations across the fragments folded so far
/// — the cross-unit half of the implicit-default rule: the default exists
/// only when NO fragment declares a constructor, so `class C {}` in one unit
/// plus `augment class C { C.named(); }` in another leaves `C()` undefined
/// (oracle `new_with_undefined_constructor_default`).
///
/// A finished [`Container`] cannot answer that through
/// `has_unnamed_constructor` alone — an explicit `C()` and the implicit
/// default both read `true` with an empty selector list — so the facts come
/// off the columns a declaration leaves behind: the constructor tables key
/// the unnamed face by the container's own name (`C()` and `C.new()` alike),
/// and any table row proves a declaration even when it registers no selector
/// (`factory H()` inside `class G`).
#[derive(Default)]
struct CtorDeclarations {
    /// Some fragment declares a constructor (named or unnamed).
    declared: bool,
    /// Some fragment declares an EXPLICIT unnamed constructor.
    explicit_unnamed: bool,
}

impl CtorDeclarations {
    /// The element's facts with its introduction's row folded in.
    fn of(container: &Container, name: StringId) -> Self {
        let mut facts = Self::default();
        facts.admit(container, name);
        facts
    }

    /// Folds one more fragment's row into the element's facts.
    fn admit(&mut self, container: &Container, name: StringId) {
        let declares = !container.constructors.is_empty()
            || !container.ctor_formals.is_empty()
            || !container.factory_formals.is_empty();
        self.declared |= declares;
        self.explicit_unnamed |= container.ctor_formals.contains_key(&name)
            || container.factory_formals.contains_key(&name)
            // A named selector alongside the unnamed flag can only mean an
            // explicit unnamed constructor was declared as well.
            || (container.has_unnamed_constructor && declares);
    }

    /// The merged implicit-default flag: an explicit unnamed constructor
    /// anywhere, or no constructor anywhere (the element synthesizes the
    /// default off its empty constructor list).
    fn settled(&self) -> bool {
        self.explicit_unnamed || !self.declared
    }
}

/// The kind tag of a name in a library face: a known [`MicroKind`], or
/// [`ScopeKind::External`] when the declaring file is outside the indexed
/// set — present, but kind-sensitive diagnostics must be suppressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeKind {
    /// Declared in an indexed file; the kind is known.
    Kind(MicroKind),
    /// Declared outside the indexed files (`dart:` SDK, unanalyzed
    /// dependency) — presence is certain, kind is unknown.
    External,
}

/// The public name face of one library: name → kind tag.
#[derive(Debug, Default)]
pub struct ScopeFace {
    names: FxHashMap<StringId, ScopeKind>,
}

impl ScopeFace {
    /// The kind tag of `name`, if the face carries it.
    #[must_use]
    pub fn kind(&self, name: StringId) -> Option<ScopeKind> {
        self.names.get(&name).copied()
    }

    /// The number of names in the face.
    #[must_use]
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether the face is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Every name with its tag, in arbitrary order.
    pub fn iter(&self) -> impl Iterator<Item = (StringId, ScopeKind)> + '_ {
        self.names.iter().map(|(name, kind)| (*name, *kind))
    }
}

/// The public faces of the analyzed libraries, indexed by
/// [`LibraryId::index`]; libraries not included in [`LibraryScopes::build`]
/// have no face. Sized by the graph; nothing grows after build.
#[derive(Debug, Default)]
pub struct LibraryScopes {
    scopes: Vec<Option<ScopeFace>>,
}

impl LibraryScopes {
    /// Builds the faces of `libs` over `graph` and `index`. The graph
    /// already folded each library's part tree into its local declarations
    /// (the caller declared every file), so a face needs no part information
    /// here.
    ///
    /// Beyond the library's own export scope, each unprefixed import joins
    /// the face with the target's exported names (#117's merge contract:
    /// the face is the library's visible-name surface) — combinator
    /// filtered, and never displacing a local or earlier-seeded name.
    #[must_use]
    pub fn build(graph: &LibraryGraph, index: &ElementIndex, libs: &[LibraryId]) -> Self {
        let mut scopes: Vec<Option<ScopeFace>> = (0..graph.len()).map(|_| None).collect();
        for &lib in libs {
            let mut face = ScopeFace::default();
            for (name, entry) in graph.export_scope(lib) {
                let kind = Self::kind_of(index, name, entry.file);
                face.names.insert(name, kind);
            }
            for edge in graph.import_edges(lib) {
                if let Some(prefix) = edge.prefix {
                    face.names
                        .entry(prefix)
                        .or_insert(ScopeKind::Kind(MicroKind::Prefix));
                    continue;
                }
                let Some(target) = edge.target else {
                    continue;
                };
                for (name, entry) in graph.export_scope(target) {
                    if !edge.combinators.allows(name) {
                        continue;
                    }
                    let kind = Self::kind_of(index, name, entry.file);
                    face.names.entry(name).or_insert(kind);
                }
            }
            scopes[lib.index() as usize] = Some(face);
        }
        Self { scopes }
    }

    /// The kind tag of an exported name from its declaring file: a known
    /// [`MicroKind`] inside the indexed set, [`ScopeKind::External`]
    /// otherwise.
    fn kind_of(index: &ElementIndex, name: StringId, file: FileId) -> ScopeKind {
        index
            .get(file)
            .and_then(|file| file.decl(name))
            .map_or(ScopeKind::External, |decl| ScopeKind::Kind(decl.kind))
    }

    /// The face of `lib`, if built.
    #[must_use]
    pub fn face(&self, lib: LibraryId) -> Option<&ScopeFace> {
        self.scopes
            .get(lib.index() as usize)
            .and_then(|s| s.as_ref())
    }
}

/// The outcome of an in-library scoped lookup ([`lookup_scoped`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScopedHit<'a> {
    /// Not defined in the current file nor the import face.
    NotFound,
    /// Declared in the current file itself (private names included; the
    /// local declaration shadows imports).
    Local(&'a TopDecl),
    /// Resolved through the library namespace to one target.
    Found(NamespaceEntry),
    /// Ambiguous between distinct targets — reportable as `ambiguous_import`
    /// at the use point, with every candidate kept. The candidates are owned
    /// (the namespace layer's ambiguous set is built per probe, #1068).
    Ambiguous(Box<[NamespaceEntry]>),
}

/// The in-library scope chain for `name`: the current file's declarations
/// (private included), then `file`'s **combined import scope** — its own
/// imports and prefixes, then each including file's up the part tree, then
/// the library-wide rows ([`Namespaces::lookup_in_file`]). Use this for
/// diagnostics **inside** the library; the public face
/// ([`LibraryScopes`]) answers import-side checks.
///
/// The per-file half is what the enhanced-parts rule requires: "each Dart
/// file defines a combined import scope which extends the scope chain of the
/// parent file's combined import scope with its own imports and import
/// prefixes". A name a part file imports resolves in that part and its
/// sub-parts only — never in a sibling part or in the library file, whose own
/// scope the library-level [`Namespaces::lookup`] answers.
#[must_use]
pub fn lookup_scoped<'a>(
    graph: &LibraryGraph,
    namespaces: &'a Namespaces,
    index: &'a ElementIndex,
    lib: LibraryId,
    file: FileId,
    name: StringId,
) -> ScopedHit<'a> {
    if let Some(decl) = index.get(file).and_then(|f| f.decl(name)) {
        return ScopedHit::Local(decl);
    }
    match namespaces.lookup_in_file(graph, lib, Some(file), name) {
        Lookup::Found(entry) => ScopedHit::Found(entry),
        Lookup::Ambiguous(candidates) => ScopedHit::Ambiguous(candidates),
        Lookup::NotFound => ScopedHit::NotFound,
    }
}

/// The library whose scope resolves a name written in `owner_file`: the
/// superclass/superface names of a container resolve in the declaring
/// container's library (the oracle climbs the declaring library's scope
/// chain), not in the diagnosing one. The graph maps `(library, name)` to
/// the declaring file and `LibraryId` values are only constructible through
/// the edges, so the answer is the first edge-reachable library (BFS over
/// import/export edges from `start`) whose locals layer returns
/// `owner_file` for the resolved spelling.
pub(crate) fn owner_library(
    graph: &LibraryGraph,
    start: LibraryId,
    owner_file: FileId,
    name: StringId,
) -> Option<LibraryId> {
    if graph.local(start, name) == LocalLookup::Found(owner_file) {
        return Some(start);
    }
    let mut seen = FxHashSet::default();
    seen.insert(start.index());
    let mut queue = Vec::new();
    queue.push(start);
    while let Some(lib) = queue.pop() {
        let targets = graph
            .import_edges(lib)
            .iter()
            .filter_map(|edge| edge.target)
            .chain(
                graph
                    .export_edges(lib)
                    .iter()
                    .filter_map(|edge| edge.target),
            );
        for target in targets {
            if !seen.insert(target.index()) {
                continue;
            }
            if graph.local(target, name) == LocalLookup::Found(owner_file) {
                return Some(target);
            }
            queue.push(target);
        }
    }
    None
}

/// Resolves a container by name: the file's own declarations first, then the
/// library namespace. Returns the container and the file that owns it (for
/// the superclass walk's next hop).
///
/// An augmented container answers its merged fragment face on both arms
/// (#3418, #3516): the element's declarations are the introduction's plus
/// every fragment's — members, enum constants, constructors, superclass and
/// clause edges — so a lookup written in any unit of the library, the
/// introduction's or an `augment` head's, sees the same set
/// ([`ElementFragments`], which also states the cross-pool gate the merge
/// obeys).
///
/// Crate-visible so the diagnostic engine's member walk can tell a cleanly
/// walked chain (member genuinely absent) from a broken one (a superclass
/// name that resolves to nothing — suppression territory, never a report).
pub(crate) fn resolve_container<'a>(
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &'a ElementIndex,
    lib: LibraryId,
    file: FileId,
    name: StringId,
) -> Option<(&'a Container, FileId)> {
    if let Some(face) = index.element_face(graph, lib, file, name) {
        return Some(face);
    }
    match namespaces.lookup_in_file(graph, lib, Some(file), name) {
        Lookup::Found(NamespaceEntry::Decl(owner)) => index.element_face(graph, lib, owner, name),
        _ => None,
    }
}

/// The dot-joined spelling's container (`io.File`, the intern table's
/// one-head form the prefixed annotations arm): splits on the FIRST dot
/// and resolves the leaf through the prefix's table — the same lookup
/// [`crate::resolution::arm_type_prefix`]'s live prefix keys. A spelling
/// with no dot, a dead prefix, or a non-container leaf answers `None`
/// (the caller's own miss face decides).
pub(crate) fn resolve_joined_container<'a>(
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &'a ElementIndex,
    lib: LibraryId,
    strings: &StringTable,
    name: StringId,
) -> Option<(&'a Container, FileId)> {
    let (prefix, leaf) = strings.resolve(name).split_once('.')?;
    prefixed_container(
        graph,
        namespaces,
        index,
        lib,
        strings.intern(prefix),
        strings.intern(leaf),
    )
}

/// Resolves a container through an import prefix: `prefix` must be a
/// registered `as` prefix of `lib` and `name` a name in its merged table —
/// all same-prefix imports in directive order, combinator filtered,
/// first-come-wins ([`Namespaces::lookup_prefixed`], the oracle's
/// `PrefixScope` composition, scope.dart:352-357). The bare-scope walk
/// ([`resolve_container`]) cannot serve a prefixed spelling: it resolves in
/// the diagnosing library's own scope, so a prefixed creation head
/// (`new p.C.named()`) would read a same-named local container in the wrong
/// scope and a missing one as unresolved.
///
/// `None` under-reports: an unregistered prefix, a prefix whose target
/// contributes nothing to the graph, a name the prefix's table lacks, and
/// a resolved name that is not an indexed container.
pub(crate) fn prefixed_container<'a>(
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &'a ElementIndex,
    lib: LibraryId,
    prefix: StringId,
    name: StringId,
) -> Option<(&'a Container, FileId)> {
    match namespaces.lookup_prefixed(graph, lib, prefix, name) {
        Lookup::Found(NamespaceEntry::Decl(owner)) => index.element_face(graph, lib, owner, name),
        _ => None,
    }
}

/// Looks up `member` on `container`'s instance/static face: the container's
/// member set, then the mixin-application order (its own `with` mixins,
/// last-applied first — `class X extends S with M1, M2` is the official
/// sequence `S&M1 extends S` / `S&M2 extends S&M1` / `X extends S&M2`,
/// `inheritance_manager3.dart:571-579`), then direct-superclass name edges.
/// Each hop resolves file-locally first, then through the library that owns
/// the declaring container (cycle-guarded).
///
/// # Cross-pool boundary (G4 phase 2, funnel F3)
///
/// The returned entry is owned by the resolving file's
/// [`crate::model::ArgPool`]: a cross-file hit carries that unit's
/// [`crate::model::ArgListId`] slots. This walk has no
/// diagnosing-engine state to project into, so a caller that drives the
/// diagnostic engine must route the hit through the engine's boundary
/// ([`mod@crate::diagnose`]'s `Engine` boundary — the F1/F2 funnels' projection)
/// before its declared types reach rows or message text; the diagnosing
/// engine itself reaches member faces through `Engine`-owned walks and
/// never calls this free function.
///
/// A `None` here is **not** proof of undefined — before reporting an
/// instance-scope `undefined_*`, check the extension faces
/// ([`FileElements::extension_faces`]) of the library's files (the caller
/// owns the file set; see the part seam in the module docs).
#[must_use]
pub fn member_lookup<'a>(
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &'a ElementIndex,
    lib: LibraryId,
    file: FileId,
    container: StringId,
    member: StringId,
) -> Option<&'a MemberEntry> {
    member_lookup_defining(
        graph, namespaces, index, lib, file, container, member, false,
    )
    .map(|(_, entry)| entry)
}

/// [`member_lookup`] with the walk facts its by-name callers drop: the name
/// of the container that declares the hit (the anchor itself when the hit
/// is a self-declaration), and `concrete_only` — abstract entries are
/// skipped and the walk continues (the official `getMember(concrete: …)`
/// cut; concreteness is the entry's `is_abstract` bit, the only concreteness
/// dimension the model carries). Crate-visible: the declaration-side
/// L-family queries need the defining container to tell an inherited hit
/// from a self-declaration.
pub(crate) fn member_lookup_defining<'a>(
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &'a ElementIndex,
    lib: LibraryId,
    file: FileId,
    container: StringId,
    member: StringId,
    concrete_only: bool,
) -> Option<(StringId, &'a MemberEntry)> {
    let acceptable =
        |entry: &'a MemberEntry| (!concrete_only || !entry.is_abstract).then_some(entry);
    let mut name = container;
    let mut seen: FxHashSet<StringId> = FxHashSet::default();
    let mut owner = file;
    let mut scope_lib = lib;
    // The `implements` faces met along the `extends`/`with` climb: an
    // implementor inherits its interfaces' members (the oracle's interface
    // build), but a superclass's interface — which already merges its
    // `implements` faces — shadows the implementor's own. Collect per level,
    // then walk deepest-first once the chain ends.
    let mut pending_faces: Vec<Vec<(StringId, FileId)>> = Vec::new();
    let mut owner_lib = lib;
    loop {
        if !seen.insert(name) {
            return None; // malformed cyclic chain: treat as unresolved
        }
        let Some((found, found_owner)) =
            resolve_container(graph, namespaces, index, scope_lib, owner, name)
        else {
            break;
        };
        if let Some(entry) = found.members.get(&member).and_then(acceptable) {
            return Some((name, entry));
        }
        // The stacked applications shadow the superclass chain in reverse
        // declaration order (the later application wins). The mixin and
        // superclass names are written in `found_owner` and resolve through
        // that file's library (probed by the container name it declares,
        // #491 S2a).
        owner_lib = owner_library(graph, lib, found_owner, name).unwrap_or(scope_lib);
        for mixin in found.with_clause.iter().rev() {
            if !seen.insert(*mixin) {
                return None;
            }
            if let Some(entry) =
                resolve_container(graph, namespaces, index, owner_lib, found_owner, *mixin)
                    .and_then(|(mixin_container, _)| mixin_container.members.get(&member))
                    .and_then(acceptable)
            {
                return Some((*mixin, entry));
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
            break;
        };
        name = superclass;
        owner = found_owner;
        scope_lib = owner_lib;
    }
    // The `extends`/`with` chain exhausted without a hit: the collected
    // interfaces still contribute members, deepest level first. Each face's
    // own supertypes expand breadth-first (a class climbs
    // `extends`/`with`/`implements`, a nested extension type its
    // `implements` — the same face set `walk_face_list` climbs for the
    // receiver-side walk).
    let mut queue: VecDeque<(StringId, FileId)> =
        pending_faces.iter().rev().flatten().copied().collect();
    let mut scope_lib = owner_lib;
    while let Some((face, face_owner)) = queue.pop_front() {
        if !seen.insert(face) {
            continue;
        }
        // The queued face's spelling is written in `face_owner`'s file and
        // resolves through that file's library (#2313) — not a probe keyed on
        // the face declaring there (an `implements` face usually declares
        // elsewhere).
        let face_lib = graph.library_of_file(face_owner).unwrap_or(scope_lib);
        let Some((face_container, face_container_owner)) =
            resolve_container(graph, namespaces, index, face_lib, face_owner, face)
        else {
            // An unresolvable interface face contributes nothing — the
            // remaining faces keep walking.
            continue;
        };
        scope_lib = face_lib;
        if let Some(entry) = face_container
            .members
            .get(&member)
            .filter(|entry| !entry.is_static)
            .and_then(acceptable)
        {
            return Some((face, entry));
        }
        for super_face in face_container
            .superclass
            .into_iter()
            .chain(face_container.with_clause.iter().copied())
            .chain(face_container.implements_clause.iter().copied())
        {
            queue.push_back((super_face, face_container_owner));
        }
    }
    None
}

/// Whether any container on `container`'s supertype chain — the `with`
/// mixins (application order) and the direct superclass, transitively; the
/// same chain [`member_lookup`] walks — declares at least one instance
/// member. The anchor's own members are not supertype members and do not
/// count; a class written without `extends` sits at the written chain's end
/// (the implicit `Object` root is outside the name-level model — its
/// members never count, the way the official
/// `avoid_classes_with_only_static_members` gate excludes
/// `dart:core Object`).
///
/// Returns `None` when the chain is unknowable: the anchor does not resolve,
/// or a written superclass/mixin name resolves to nothing indexed (an SDK
/// type, an unanalyzed dependency) — that face could carry instance members,
/// so the caller stays silent. `Some(false)` means every supertype on the
/// written chain is indexed and none declares an instance member.
pub(crate) fn supertype_has_instance_members(
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &ElementIndex,
    lib: LibraryId,
    file: FileId,
    container: StringId,
) -> Option<bool> {
    let (mut found, mut found_owner) =
        resolve_container(graph, namespaces, index, lib, file, container)?;
    let mut name = container;
    let mut scope_lib = lib;
    let mut seen: FxHashSet<StringId> = FxHashSet::default();
    seen.insert(container);
    loop {
        let owner_lib = owner_library(graph, lib, found_owner, name).unwrap_or(scope_lib);
        // A `with` name that does not resolve leaves the chain's member face
        // unknown — `?` answers `None`, not "absent".
        for mixin in found.with_clause.iter().rev() {
            if !seen.insert(*mixin) {
                return None; // malformed cyclic chain
            }
            let (mixin_container, _) =
                resolve_container(graph, namespaces, index, owner_lib, found_owner, *mixin)?;
            if mixin_container
                .members
                .values()
                .any(|entry| !entry.is_static)
            {
                return Some(true);
            }
        }
        let Some(super_name) = found.superclass else {
            return Some(false); // the written chain ends with no instance member on it
        };
        if !seen.insert(super_name) {
            return None; // malformed cyclic chain
        }
        (found, found_owner) =
            resolve_container(graph, namespaces, index, owner_lib, found_owner, super_name)?;
        if found.members.values().any(|entry| !entry.is_static) {
            return Some(true);
        }
        name = super_name;
        scope_lib = owner_lib;
    }
}

/// The `dart:core` interface-membership face for names outside the indexed
/// set: `(type, interface)` pairs whose membership is definitional —
/// `Iterable` is itself, and the direct supertype of `List` and `Set`
/// (upstream Dart SDK source, sdk/lib/core/{iterable,list,set}.dart). The
/// same face answers the `dart:collection`/`dart:convert` bases whose
/// clause heads live in unindexed SDK units: the typedef aliases
/// (`IterableBase`/`IterableMixin` are `Iterable` itself, `SetMixin` is
/// `SetBase`, `MapMixin` is `MapBase`, the two `*SinkBase` names their
/// sink — sdk/lib/collection/iterable.dart:11,18, set.dart:322,
/// maps.dart:204, sdk/lib/convert/{string_conversion,byte_conversion}
/// .dart:162,38), the `implements` clauses the alias targets carry
/// (set.dart:23, maps.dart:21), and `SplayTreeSet`'s `with Iterable<E>,
/// SetMixin<E>` application (splay_tree.dart:998) whose private `_SplayTree`
/// superclass never resolves outside the SDK. An unindexed name is answered
/// only when it appears here as implementing `interface`; every other
/// unindexed name (an unanalyzed dependency, an SDK type absent from the
/// table) leaves the question open. The table carries no negative facts:
/// an unindexed name never answers `false`, only the open direction.
const CORE_INTERFACE_IMPLS: [(&str, &str); 14] = [
    ("Iterable", "Iterable"),
    ("List", "Iterable"),
    ("Set", "Iterable"),
    ("IterableMixin", "Iterable"),
    ("IterableBase", "Iterable"),
    ("ListMixin", "List"),
    ("ListBase", "List"),
    ("SetMixin", "Set"),
    ("SetBase", "Set"),
    ("MapMixin", "Map"),
    ("MapBase", "Map"),
    ("SplayTreeSet", "Set"),
    ("StringConversionSinkBase", "StringConversionSink"),
    ("ByteConversionSinkBase", "ByteConversionSink"),
];

/// Whether the name implements the interface, per
/// [`CORE_INTERFACE_IMPLS`] — the core face's definite hit.
fn core_implements(core: &[(StringId, StringId)], ty: StringId, interface: StringId) -> bool {
    core.iter()
        .any(|(name, iface)| *name == ty && *iface == interface)
}

/// The written supertype edges of `container` — the face the
/// interface-membership walk expands: the direct superclass, the `with`
/// mixins, and the `implements` interfaces for the class family; a mixin's
/// declared `on` constraints joined with its own `implements` clause (a
/// constraint type is a supertype of the mixin, and of every application
/// that satisfies it); the `implements` clause alone for the remaining
/// kinds (an extension type's representation is not a supertype — the
/// erasure face handles it — and an extension is not a type at all). The
/// implicit `Object` root is outside the name-level model and never an
/// edge, so the walk ends where the written chain ends.
fn supertype_edges(container: &Container, strings: &StringTable) -> Vec<StringId> {
    let mixin_constraints = (container.kind == ContainerKind::Mixin)
        .then(|| container.on_clause.iter())
        .into_iter()
        .flatten();
    let mut edges: Vec<StringId> = container
        .superclass
        .iter()
        .chain(container.with_clause.iter())
        .chain(container.implements_clause.iter())
        .chain(mixin_constraints)
        .copied()
        .collect();
    // An enum declaration implicitly implements `Enum` (the language's
    // implicit interface — the spec makes it a compile-time error for an
    // enum's interface not to include dart:core's `Enum`): a declared-edge
    // walk never sees it, so the edge rides here. The spelling resolves
    // through the ordinary namespace face like any written edge (a user
    // `Enum` shadowing the core name routes through the same walk).
    if container.kind == ContainerKind::Enum {
        edges.push(strings.intern("Enum"));
    }
    edges
}

/// Whether `name` implements `interface` — the transitive supertype
/// question `type.implementsInterface(interface, 'dart.core')` answers
/// (upstream Dart SDK source, pkg/linter/lib/src/extensions.dart:324-331):
/// the interface spelling itself, then the declared supertype graph
/// ([`supertype_edges`]) walked transitively, cross-file, each hop
/// resolving through its declaring container's library — the same
/// discipline [`member_lookup`]'s walk applies to superclass names.
///
/// The three-valued contract: `Some(true)` on a definite hit (an edge
/// spelled `interface` outside the indexed set — the core spelling — or a
/// [`CORE_INTERFACE_IMPLS`] hit anywhere on the graph); `Some(false)` only
/// when every reached name is an indexed container and no edge hits; `None`
/// when any reached name is outside both the index and the core face (that
/// face could implement the interface — the open direction outranks a
/// tentative `false`, a definite `true` outranks the open one). A written
/// supertype cycle is impossible in code the oracle analyzes; the seen-set
/// makes it terminate as an ordinary shared node.
/// Whether an unresolvable supertype hop is a dead edge rather than a
/// broken chain. A name the scope cannot resolve at all — undefined,
/// ambiguous, a non-class declaration — is the oracle's InvalidType
/// supertype: it contributes nothing to the interface. A name that resolves
/// to a declaration whose file carries no model (the synthetic SDK face)
/// is the broken chain: the oracle sees members we cannot index.
pub(crate) fn supertype_hop_dead(
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &ElementIndex,
    lib: LibraryId,
    file: FileId,
    name: StringId,
) -> bool {
    if index.get(file).is_some_and(|f| f.container(name).is_some()) {
        return false;
    }
    match namespaces.lookup(graph, lib, name) {
        Lookup::Found(NamespaceEntry::Decl(owner)) => index.get(owner).is_some(),
        _ => true,
    }
}

pub(crate) fn implements_interface(
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &ElementIndex,
    strings: &StringTable,
    lib: LibraryId,
    file: FileId,
    name: StringId,
    interface: StringId,
    entity_ask: bool,
) -> Option<bool> {
    // The core face, pre-interned once per query: the walk compares ids,
    // never re-resolves spellings per node.
    let core: Vec<(StringId, StringId)> = CORE_INTERFACE_IMPLS
        .iter()
        .map(|(ty, iface)| (strings.intern(ty), strings.intern(iface)))
        .collect();
    // The seed: the name as the diagnosing scope resolves it. An extension
    // type seeds its own declaration — the STATIC subtype question walks
    // the declared `implements` closure (`supertype_edges`), never the
    // representation type: `extension type CB._(int _) implements IA`
    // makes `CB <: IA` while `CB <: int` stays false (the representation
    // is a static supertype of nothing; `typeForInterfaceCheck`'s
    // `extensionTypeErasure` is the RUNTIME `is` face, which this walk
    // never answers). An unindexed seed is answered by the core face
    // alone — the name could implement anything else.
    let seed: (StringId, FileId, LibraryId);
    match resolve_container(graph, namespaces, index, lib, file, name) {
        Some((_, owner)) => seed = (name, owner, lib),
        None => {
            return core_implements(&core, name, interface).then_some(true);
        }
    }
    let (start, start_file, start_lib) = seed;
    // An entity ask whose interface spelling resolves to nothing (an
    // undefined name or a type formal — formals answer through their
    // bound, never the graph) is the oracle's InvalidType target: the
    // assignability faces stay silent. A core ask keeps the core-face
    // answer.
    if entity_ask && resolve_container(graph, namespaces, index, lib, file, interface).is_none() {
        return None;
    }
    let Some((container, owner)) =
        resolve_container(graph, namespaces, index, start_lib, start_file, start)
    else {
        if entity_ask {
            // An InvalidType source is dynamic-shaped: assignable to
            // anything the oracle would still check — the open verdict.
            return None;
        }
        return core_implements(&core, start, interface).then_some(true);
    };
    let mut answer = Some(false);
    let mut seen: FxHashSet<StringId> = FxHashSet::default();
    seen.insert(start);
    let mut queue: Vec<(StringId, FileId, LibraryId, &Container)> = Vec::new();
    queue.push((start, owner, start_lib, container));
    while let Some((current, current_owner, scope_lib, container)) = queue.pop() {
        let owner_lib = owner_library(graph, lib, current_owner, current).unwrap_or(scope_lib);
        for edge in supertype_edges(container, strings) {
            let resolved =
                resolve_container(graph, namespaces, index, owner_lib, current_owner, edge);
            // Entity-level asks match the interface the moment the chain
            // spells it through a live edge. A dead edge spelled the same
            // is the oracle's InvalidType — it contributes nothing (the
            // `LocaleImplementation implements Locale` shape with the
            // Locale edge unresolved is a definite miss). Core-face asks
            // never spelling-match an indexed edge: a user declaration
            // shadowing the spelling is not the core face — the
            // resolution decides, never the spelling.
            if entity_ask && edge == interface && resolved.is_some() {
                return Some(true);
            }
            if !seen.insert(edge) {
                continue;
            }
            match resolved {
                Some((next, next_owner)) => queue.push((edge, next_owner, owner_lib, next)),
                // An indexed name spelled `interface` is a user type, never
                // the core spelling (the oracle compares the defining
                // library, extensions.dart:335-340) — it walked above. Only
                // an unindexed edge can be the core face; a dead edge (the
                // oracle's InvalidType — undefined, ambiguous, or non-class)
                // contributes nothing to the interface, and any other
                // unindexed edge could implement the interface, so the
                // definite answer dies (a later definite `true` still
                // outranks it).
                None => {
                    if core_implements(&core, edge, interface) {
                        return Some(true);
                    }
                    if !supertype_hop_dead(graph, namespaces, index, owner_lib, current_owner, edge)
                    {
                        answer = None;
                    }
                }
            }
        }
    }
    answer
}

/// Whether `name` can enter `lib`'s unit scope through an import: an
/// `as` prefix spelled `name` (import prefixes are library-scope names,
/// upstream Dart SDK source, pkg/analyzer/lib/src/dart/resolver/
/// scope_analyzer.dart), or an unprefixed import whose target's export
/// scope carries `name` under the directive's combinators — the same
/// merge [`LibraryScopes::build`] runs over the import face, scoped to
/// the one-name question a shadowing check asks (a prefixed import
/// contributes its prefix spelling alone; its target's exports stay
/// outside the unit scope).
///
/// The direction is conservative: an unprefixed import whose target
/// contributes nothing to the graph (an unresolvable URI, an unanalyzed
/// dependency) answers `true` — that library could export any name.
/// `false` therefore means "no indexed import carries the name", never a
/// proof of absence everywhere.
pub(crate) fn import_shadows(graph: &LibraryGraph, lib: LibraryId, name: StringId) -> bool {
    graph
        .import_edges(lib)
        .iter()
        .any(|edge| match edge.prefix {
            Some(prefix) => prefix == name,
            None => match edge.target {
                None => true, // the target's export set is unknowable
                Some(target) => {
                    edge.combinators.allows(name) && graph.exported(target, name).is_some()
                }
            },
        })
}

/// Whether the container `name` resolves to — file-local first, then the
/// library namespace, cross-file exactly like [`member_lookup`]'s anchor
/// resolution — carries an annotation whose spelled name is `annotation`.
/// The name-level cut of the JS-interop classification, which reads the
/// class element's own metadata (`metadata.hasJS`, upstream Dart SDK
/// source, pkg/linter/lib/src/rules/
/// invalid_runtime_check_with_js_interop_types.dart
/// `_jsTypeForStaticInterop`): only the resolved container's own
/// annotation rows answer, the supertype chain is not walked.
///
/// `None` = the name does not resolve to an indexed container (an SDK
/// type, an unanalyzed dependency) — that container could carry the
/// annotation, so the caller stays silent. A prefix-qualified annotation
/// keeps its dot-joined spelling (`@p.JS()` stores `p.JS`), so it does
/// not match a bare `JS` query — the under-report direction.
pub(crate) fn container_has_annotation(
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &ElementIndex,
    lib: LibraryId,
    file: FileId,
    container: StringId,
    annotation: StringId,
) -> Option<bool> {
    let (found, _) = resolve_container(graph, namespaces, index, lib, file, container)?;
    Some(found.annotations.iter().any(|a| a.name == annotation))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::collect;
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_parser::ParserFeatures;
    use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder};
    use dartq_scanner::{ScanConfig, TokenTable};
    use dartq_source::{FileTable, StringTable};
    use std::path::Path;

    /// Scans + collects one unit, interning its names into `strings`.
    fn unit(
        source: &str,
        strings: &StringTable,
        files: &mut FileTable,
        path: &str,
    ) -> FileElements {
        let file = files.intern(Path::new(path));
        let result = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(result.diagnostics.is_empty(), "{source:?}");
        let tokens: &'static TokenTable = Box::leak(Box::new(result.tokens));
        let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
            LanguageVersion::new(3, 15),
        ));
        collect(source, tokens, features, strings)
    }

    /// Two-file library merge through #117: a's public declarations are
    /// visible in b's face with their origin kinds; the prefix is tagged;
    /// b's locals shadow the import; a's private name stays invisible.
    #[test]
    fn library_merge_through_namespaces() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let elements_a = unit(
            "class A {} \
             void fa({int? n}) {} \
             int _private = 0;",
            &strings,
            &mut files,
            "a.dart",
        );
        let elements_b = unit(
            "import 'a.dart'; \
             import 'a.dart' as p; \
             class A {} \
             class B {}",
            &strings,
            &mut files,
            "b.dart",
        );
        let file_a = files.try_file_id(Path::new("a.dart")).unwrap();
        let file_b = files.try_file_id(Path::new("b.dart")).unwrap();

        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib_a = builder.intern("package:t/a.dart");
        let lib_b = builder.intern("package:t/b.dart");
        let mut index = ElementIndex::default();
        for (lib, file, elements) in [(lib_a, file_a, &elements_a), (lib_b, file_b, &elements_b)] {
            index.insert(file, elements.clone());
            for (name, decl) in elements.declarations() {
                if decl.kind == MicroKind::TypeParam {
                    continue;
                }
                builder.declare(lib, name, file);
            }
        }
        builder.import(
            lib_b,
            ImportEdge {
                target: Some(lib_a),
                prefix: None,
                deferred: false,
                non_library: false,
                combinators: Combinators::unfiltered(),
            },
        );
        builder.import(
            lib_b,
            ImportEdge {
                target: Some(lib_a),
                prefix: Some(strings.intern("p")),
                deferred: false,
                non_library: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let scopes = LibraryScopes::build(&graph, &index, &[lib_a, lib_b]);

        let kind = |lib: LibraryId, name: &str| {
            scopes
                .face(lib)
                .and_then(|face| face.kind(strings.intern(name)))
        };
        assert_eq!(
            kind(lib_b, "A"),
            Some(ScopeKind::Kind(MicroKind::Class)),
            "b's local class A wins the face seeding"
        );
        assert_eq!(
            kind(lib_b, "fa"),
            Some(ScopeKind::Kind(MicroKind::TopLevelFn))
        );
        assert_eq!(kind(lib_b, "p"), Some(ScopeKind::Kind(MicroKind::Prefix)));
        assert_eq!(kind(lib_b, "nope"), None);
        // Private names never reach the public face.
        assert_eq!(kind(lib_a, "_private"), None);

        // In-library scoped lookup: locals (incl. private) then imports.
        let hit = |file: FileId, lib: LibraryId, name: &str| {
            lookup_scoped(&graph, &namespaces, &index, lib, file, strings.intern(name))
        };
        assert_eq!(
            hit(file_b, lib_b, "A"),
            ScopedHit::Local(
                index
                    .get(file_b)
                    .unwrap()
                    .decl(strings.intern("A"))
                    .unwrap()
            ),
            "b's local A shadows the import"
        );
        assert_eq!(
            hit(file_b, lib_b, "fa"),
            ScopedHit::Found(NamespaceEntry::Decl(file_a))
        );
        assert!(matches!(
            hit(file_b, lib_b, "_private"),
            ScopedHit::NotFound
        ));
        assert!(matches!(
            hit(file_a, lib_a, "_private"),
            ScopedHit::Local(_)
        ));
    }

    /// Member walk: superclass edges resolve across files through the
    /// namespace; a miss falls to the extension-pool recipe.
    #[test]
    fn member_walk_crosses_superclass_edges() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let base = unit(
            "class B { void m() {} int get g => 1; } \
             extension on B { int get extG => 1; }",
            &strings,
            &mut files,
            "base.dart",
        );
        let derived = unit("class C extends B {}", &strings, &mut files, "use.dart");
        let base_file = files.try_file_id(Path::new("base.dart")).unwrap();
        let use_file = files.try_file_id(Path::new("use.dart")).unwrap();

        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib = builder.intern("package:t/lib.dart");
        for (file, elements) in [(base_file, &base), (use_file, &derived)] {
            for (name, decl) in elements.declarations() {
                if decl.kind != MicroKind::TypeParam {
                    builder.declare(lib, name, file);
                }
            }
        }
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let mut index = ElementIndex::default();
        index.insert(base_file, base.clone());
        index.insert(use_file, derived);

        let m = member_lookup(
            &graph,
            &namespaces,
            &index,
            lib,
            use_file,
            strings.intern("C"),
            strings.intern("m"),
        )
        .expect("inherited through the superclass edge");
        assert_eq!(m.kind, crate::model::MemberKind::Method);
        assert!(
            member_lookup(
                &graph,
                &namespaces,
                &index,
                lib,
                use_file,
                strings.intern("C"),
                strings.intern("g")
            )
            .is_some()
        );
        // The miss that must consult the extension pool before reporting.
        assert!(
            member_lookup(
                &graph,
                &namespaces,
                &index,
                lib,
                use_file,
                strings.intern("C"),
                strings.intern("extG")
            )
            .is_none()
        );
        assert!(
            index
                .get(base_file)
                .unwrap()
                .has_extension_member(strings.intern("extG"))
        );
    }
}
