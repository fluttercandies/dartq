//! Declaration reachability over the maintained reference surfaces
//! (#1157 wave L5, #1199): the engine-level pass the
//! `unreachable_from_main` lint shell mounts on.
//!
//! # Layering
//!
//! The rust-analyzer `DeadCode` posture, per the `#1157` ruling: reachability
//! is an engine product, computed once per analysis generation over the
//! shared faces — the maintained captures (declaration rows) and the
//! per-library reference surfaces (resolved reference rows) — and exposed
//! as a per-declaration flag ([`Reachability::is_reachable`]). The lint
//! shell (a later lane) never rescans: it reads flags and applies its own
//! reporting face (public, not `@visibleForTesting`, unreachable).
//!
//! The propagation graph is declaration-level, so the library-level
//! import-cycle condensation ([`crate::cycle::Condensation`]) is not its
//! substrate — a Tarjan pass over the declaration edges would add nothing
//! a worklist propagation lacks (entry-reachability is plain graph
//! reachability: one O(V+E) sweep, mark-before-push, every node enqueues
//! at most once), and the library condensation answers a different
//! question (cycle fingerprints for the reuse gate). What the two share is
//! the posture: derived-only data, never persisted, no per-query rescans.
//!
//! # Exposure contract
//!
//! The flags are a per-generation map on the engine state, built lazily:
//! the analyze only arms the rebuild (the same gate the eager build fired
//! on — changed or removed residency, or any recomputed library), and the
//! first [`crate::Engine::reachability`] read after it pays the build,
//! salvaged per unit ([`build`]); every further read clones the standing
//! map. An edit whose consumer never mounts the reachability lint pays
//! zero for the pass. A read that lands after an apply no analyze has
//! absorbed serves the standing map instead of building over in-flight
//! residency — the read-after-write contract is the [`crate::query`] one:
//! the map reflects the latest completed analyze; a query never recomputes
//! over half-applied state. The flags deliberately do NOT join
//! [`crate::state::UnitFacts`]: facts key on per-file content identity and
//! serve cached, persisted payloads, while a reachable flag is a
//! workspace-derived fact — caching it per file would serve stale flags
//! for unchanged files after any cross-file edit.
//!
//! # Entries
//!
//! A declaration seeds propagation when it is one of:
//!
//! * a top-level function named `main` (a getter spelled `main` also
//!   qualifies — the capture folds functions, getters, and setters into
//!   one top-level kind, and setters carry the `main=` spelling);
//! * a declaration bound to an `@pragma` annotation whose first positional
//!   string argument is `'vm:entry-point'` (any string-literal spelling:
//!   quote style, `r` prefix, triple quotes, trailing arguments); the
//!   annotation must resolve outside the workspace — a local or imported
//!   declaration named `pragma` shadows dart:core and disqualifies,
//!   mirroring the official resolved-element check;
//! * a declaration bound to an `@Preview` annotation whose reference
//!   resolves into
//!   `package:flutter/src/widget_previews/widget_previews.dart` (the
//!   official library-URI check; resolution already applied the import
//!   faces and combinators, so a hidden `Preview` never resolves here).
//!
//! An annotation binds to the next declaration row of its file in source
//! order, so chained annotations bind jointly; an annotation carrying no
//! following declaration binds nothing. The ruling broadens entry-hood to
//! every declaration kind — the official rule seeds only from function
//! declarations, so a `@pragma`/`@Preview`-annotated class or member is a
//! documented dartq entry where the oracle has none.
//!
//! # Edges
//!
//! Nodes are capture declaration rows, `(file, row)` pairs — top-level
//! declarations, members, and type parameters alike (type parameters never
//! gain references and simply stay unreachable). Every resolved reference
//! row of every live library's surface contributes edges from the use
//! site's enclosing declaration(s) to the target declaration:
//!
//! * **Call / tear-off / constructor references** (`Invoked` rows), value
//!   reads and assignment targets (`Read`, `Written`, `ReadWritten` rows),
//!   and **type / clause / metadata names** (`Referenced` rows — clause
//!   names included: an `extends`/`with`/`implements`/`on` name sits in
//!   its container's header, so attribution places it on the declaring
//!   container; the surface's `SubtypeEdge` table is deliberately not
//!   consulted, being the same edges in a second representation);
//! * **annotation arguments** — rows inside an annotation (its name and
//!   its argument group) attribute to the annotated declaration;
//! * **enum-member and field-initializer references** — plain reference
//!   rows whose targets are the constant/field rows; a member-targeted
//!   reference marks the member row AND its enclosing top-level container
//!   (the official `_addDeclaration` double marking);
//! * **cross-library edges** — surfaces resolve through the workspace
//!   export scopes, so `main` calling into an imported library marks the
//!   callee. The official rule is per-library (its declaration map holds
//!   only the analyzed library's declarations), so dartq is over-reachable
//!   wherever a declaration is referenced only from other libraries — a
//!   documented divergence the lint lane owns.
//!
//! **Deliberately excluded** (each changes observable output; named so the
//! lint lane can revisit): references the resolver never resolved (the
//! names table's rows — the official unresolved reference contributes
//! nothing either); the implicit default-super-constructor edge (a
//! subclass with no explicit constructor marks its supertype's unnamed
//! constructor upstream — dartq marks the supertype only through its
//! clause name); the `toJson` auto-reachability rule and the
//! `@reflectiveTest` unnamed-constructor edge (both need member
//! staticness and annotation facts the declaration rows do not carry);
//! the upstream `NamedType` filters (type-literal, alias, extension-type,
//! type-argument, external-variable shapes) — dartq counts EVERY type
//! reference, so a type used only in ordinary type annotations is
//! over-reachable relative to the oracle; and pattern-position references
//! are not suppressed (the official rule excludes constant patterns;
//! reference rows carry no pattern-region bit — the over-reachable
//! direction).
//!
//! # Use-site attribution
//!
//! Reference rows carry the use site's span but not its enclosing
//! declaration, so attribution is geometric: the nearest preceding
//! declaration row of the same file is the edge source, plus the source's
//! top-level container (the official graph walks each gathered container
//! as a whole, so member bodies also feed the container's edges). Known
//! skews, bounded and stated: text preceding a declaration's name token
//! (return types, header type arguments) attributes to the previous
//! declaration, and nothing precedes the file's first declaration;
//! annotation regions are carved out explicitly (the windows above).
//! Members are first-class sources, so intra-file dead code — the lint's
//! dominant shape — propagates exactly.
//!
//! # Performance
//!
//! The build splits into per-unit phases (text decode, attribution
//! geometry, entry detection, edge derivation — independent per member
//! file, rayon-parallel across the moved units' owners) and the global
//! propagation (CSR assembly + worklist, rerun over the full graph on
//! every rebuild: an edge removal can unmark nodes anywhere downstream, so
//! bits cannot be salvaged across an edge move — what salvage removes is
//! the per-row attribution and the standing surfaces' row scan, the
//! dominant cost). One linear sweep per derived unit: rows bucket per use
//! file and sort once, one linear UTF-16→byte walk per file feeds the
//! annotation-head probes, attribution binary-searches the per-file offset
//! tables, and propagation is a worklist over CSR adjacency — 12 bytes per
//! standing edge (the salvage product), one bit per declaration resident.
//! Every loop advances structurally: the byte walk consumes the file, the
//! worklist pops a marked node per iteration, and mark-before-push bounds
//! each node to one enqueue.

use std::fmt;
use std::sync::Arc;

use dartq_index::{DeclKind, DeclRow, FileCapture, LibraryIndex, RefKind, RefRow};
use dartq_source::{ContentId, FileId, StringId};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::pipeline::member_files;
use crate::state::{EngineState, LibraryId};

/// The Flutter widget-previews library whose `Preview` class the official
/// rule recognizes (`unreachable_from_main.dart`
/// `_flutterWidgetPreviewLibraryUri`).
const PREVIEW_LIBRARY: &str = "package:flutter/src/widget_previews/widget_previews.dart";

/// The pragma argument that marks a VM entry point (the official
/// `_isValidVmEntryPoint` constant).
const VM_ENTRY_POINT: &[u8] = b"vm:entry-point";

/// Per-declaration reachability flags for one analysis generation: one bit
/// per capture declaration row, keyed by file. Built by [`build`] at the
/// first read after an analyze moved the pass's inputs; read through
/// [`crate::Engine::reachability`].
#[derive(Debug, Default)]
pub struct Reachability {
    reachable: FxHashMap<FileId, Box<[u64]>>,
    /// The per-unit salvage products of the build that produced these bits:
    /// one entry per live member file — the unit's node count, its entry
    /// seeds, and its propagation edges in file-local coordinates. The map
    /// carries the cache because its lifetime (one `Arc`, replaced at the
    /// next rebuild) is exactly the salvage horizon: the next [`build`]
    /// re-derives only the units whose identity key moved.
    units: FxHashMap<FileId, UnitProduct>,
}

impl Reachability {
    /// Whether the declaration at capture row `row` of `file` is reachable
    /// from an entry. `None` when the file has no map (never analyzed, or
    /// retired since) or `row` is beyond its capture's declaration count —
    /// a stale row index against a recaptured file answers `None`, never a
    /// borrowed verdict.
    #[must_use]
    pub fn is_reachable(&self, file: FileId, row: u32) -> Option<bool> {
        let bits = self.reachable.get(&file)?;
        let word = *bits.get(row as usize / 64)?;
        Some(word & (1 << (row % 64)) != 0)
    }
}

/// One member file's slice of the propagation graph: everything the global
/// phases consume, in file-local coordinates so a dense renumbering
/// elsewhere (a membership or row-count move in an earlier unit) cannot
/// stale it.
#[derive(Clone)]
struct UnitProduct {
    /// The inputs' identity: the capture's content id, the owning library,
    /// and the owner's result generation (the surface's version — the two
    /// move together at every recompute). Key equality is product freshness.
    key: (ContentId, LibraryId, u32),
    /// The unit's node count (its capture's declaration row count).
    rows: u32,
    /// Entry rows (`main`, `vm:entry-point`, `@Preview`), row-local.
    seeds: Vec<u32>,
    /// Propagation edges `(source row, target file, target row)`, each
    /// coordinate local to its own file.
    edges: Arc<[(u32, FileId, u32)]>,
}

impl UnitProduct {
    /// Whether the derived payload is byte-equal. The key is deliberately
    /// excluded: a capture move that changes no seed and no edge (comment
    /// churn) must still hit the standing-bits fast path.
    fn same_payload(&self, fresh: &Self) -> bool {
        self.rows == fresh.rows && self.seeds == fresh.seeds && *self.edges == *fresh.edges
    }
}

impl fmt::Debug for UnitProduct {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnitProduct")
            .field("rows", &self.rows)
            .field("seeds", &self.seeds)
            .field("edges", &self.edges.len())
            .finish_non_exhaustive()
    }
}

/// Computes the entry-reachable declaration set over the live libraries'
/// captures and reference surfaces, salvaging the previous map's per-unit
/// products: a unit whose `(content, owner, owner generation)` identity
/// stood keeps its seeds and edges, and only the moved units re-derive.
/// `salvage` is the standing map (`None` for the wholesale build — the
/// first build, and the tests' byte-parity oracle).
///
/// The propagation itself (CSR assembly + worklist) reruns over the full
/// graph on every rebuild: an edge removal can unmark nodes anywhere
/// downstream, so bits cannot be salvaged across an edge move — what
/// salvage removes is the per-row attribution and the standing surfaces'
/// row scan. The one exception is the standing-bits fast path below: when
/// every unit's product stood (by identity key or by re-derived bytes) the
/// previous graph IS this graph and the previous map returns untouched.
#[expect(clippy::too_many_lines)] // the three build phases: salvage decision, parallel derive, global propagation
pub(crate) fn build(state: &EngineState, salvage: Option<Arc<Reachability>>) -> Arc<Reachability> {
    let strings = &state.strings;
    let main = strings.intern("main");
    let pragma = strings.intern("pragma");
    let preview = strings.intern("Preview");

    // Phase A — the unit universe and the salvage decision: one pass over
    // the live members in the deterministic library order (the dense bases
    // renumber from it below). O(files): one capture-presence probe and one
    // key comparison per member — no text decode, no rows.
    let mut order: Vec<FileId> = Vec::new();
    let mut universe: FxHashSet<FileId> = FxHashSet::default();
    let mut products: FxHashMap<FileId, UnitProduct> = FxHashMap::default();
    let mut dirty_by_owner: FxHashMap<LibraryId, Vec<FileId>> = FxHashMap::default();
    let mut misses: Vec<(FileId, &'static str)> = Vec::new();
    for (library, info) in state.libraries.iter_infos() {
        for file in member_files(info) {
            // Members are resident and captured after an analyze — the same
            // contract `library_surface` builds on. The probe is also the
            // coherence precondition: the memo only builds post-drain. A
            // pairing miss (an engine bug) drops the file from the unit set
            // and records a degrade row: the map answers nothing for the
            // file (never a borrowed verdict) until the next rebuild
            // recaptures it.
            if state.capture(file).is_none() {
                // "live member file has a capture after analyze" — the
                // analyze contract.
                misses.push((file, "capture"));
                continue;
            }
            let Some(facts) = state.residency.get(&file) else {
                // "live member file is resident" — the residency contract.
                misses.push((file, "residency"));
                continue;
            };
            let key = (
                facts.content(),
                library,
                state
                    .library_generations
                    .get(&library)
                    .copied()
                    .unwrap_or(0),
            );
            universe.insert(file);
            order.push(file);
            match salvage
                .as_ref()
                .and_then(|prev| prev.units.get(&file))
                .filter(|product| product.key == key)
            {
                Some(product) => {
                    products.insert(file, product.clone());
                }
                None => {
                    dirty_by_owner.entry(library).or_default().push(file);
                }
            }
        }
    }
    let all_stood = dirty_by_owner.is_empty();

    // Phase B — re-derive the moved units, one rayon job per owning
    // library: a use file's reference rows live only in its owner's surface
    // (a surface is built from its own members' captures), so the job scans
    // exactly the standing surfaces the moved units belong to and touches
    // no other.
    let derived: Vec<Vec<(FileId, Result<UnitProduct, &'static str>)>> = dirty_by_owner
        .into_par_iter()
        .map(|(library, files)| {
            let surface_rows: &[RefRow] = state.surface(library).map_or(&[], LibraryIndex::refs);
            let mut buckets: FxHashMap<FileId, Vec<&RefRow>> =
                files.iter().map(|&file| (file, Vec::new())).collect();
            for row in surface_rows {
                if let Some(bucket) = buckets.get_mut(&row.file) {
                    bucket.push(row);
                }
            }
            let owner_generation = state
                .library_generations
                .get(&library)
                .copied()
                .unwrap_or(0);
            files
                .into_iter()
                .map(|file| {
                    // The moved member's pairing contracts (capture,
                    // residency, store, UTF-8): a miss skips the unit and
                    // rides the face back to the sequential tail as a
                    // degrade row — the rayon closure takes no locks, and a
                    // panic here would abort the whole analyze.
                    let Some(capture) = state.capture(file) else {
                        return (file, Err("capture"));
                    };
                    let Some(facts) = state.residency.get(&file) else {
                        return (file, Err("residency"));
                    };
                    let Some(bytes) = state.store.get(facts.content()) else {
                        return (file, Err("store"));
                    };
                    let Ok(text) = std::str::from_utf8(bytes) else {
                        return (file, Err("utf8"));
                    };
                    // Every moved file was bucketed above (function-local
                    // pairing: the bucket map was seeded from this same
                    // `files` list) — a miss contributes no reference
                    // edges.
                    let mut bucket = buckets.remove(&file).unwrap_or_default();
                    let product = derive_unit(
                        state,
                        capture,
                        text,
                        &mut bucket,
                        (facts.content(), library, owner_generation),
                        main,
                        pragma,
                        preview,
                        &universe,
                    );
                    (file, Ok(product))
                })
                .collect()
        })
        .collect();
    // Whether every re-derived payload came out byte-equal (a capture moved
    // under it — comment churn — without moving a seed or an edge). A unit
    // whose derive skipped on a pairing miss drops from the unit set here —
    // phase C indexes `products` by the surviving `order`, so a skipped unit
    // must leave it — and its face rides the degrade log.
    let mut payload_stood = salvage.is_some();
    let mut skipped = FxHashSet::default();
    for (file, product) in derived.into_iter().flatten() {
        match product {
            Ok(product) => {
                payload_stood &= salvage
                    .as_ref()
                    .and_then(|prev| prev.units.get(&file))
                    .is_some_and(|prev| prev.same_payload(&product));
                products.insert(file, product);
            }
            Err(face) => {
                misses.push((file, face));
                skipped.insert(file);
            }
        }
    }
    state.residency_degrades.record(&misses);
    if !skipped.is_empty() {
        universe.retain(|file| !skipped.contains(file));
        order.retain(|file| !skipped.contains(file));
    }

    // The standing-bits fast path: the unit set is unchanged and every
    // product either was salvaged (identity key stood) or re-derived
    // byte-equal, so the propagation graph is exactly the previous one and
    // its bits are the product — the map returns as-is, propagation-free.
    if let Some(prev) = salvage.as_ref() {
        let same_units = prev.units.len() == order.len()
            && order.iter().all(|file| prev.units.contains_key(file));
        if same_units && (all_stood || payload_stood) {
            return salvage.expect("checked fresh above");
        }
    }

    // Phase C — the global pass: dense bases in library order, the seed
    // stack, CSR assembly over every unit's edges, then the mark-before-
    // push worklist. A cached product may reference a file that left
    // residency since it was derived; such edges drop here, exactly where
    // the derive's universe check drops them for fresh products.
    let mut unit_of: FxHashMap<FileId, u32> = FxHashMap::default();
    let mut bases: Vec<u32> = Vec::with_capacity(order.len());
    let mut node_total: u32 = 0;
    for (index, file) in order.iter().enumerate() {
        unit_of.insert(*file, u32::try_from(index).expect("unit index fits u32"));
        bases.push(node_total);
        node_total = node_total
            .checked_add(products[file].rows)
            .expect("the declaration universe fits u32");
    }
    let mut bits: Vec<Box<[u64]>> = Vec::with_capacity(order.len());
    let mut stack: Vec<u32> = Vec::new();
    for (index, file) in order.iter().enumerate() {
        let product = &products[file];
        let mut words = vec![0u64; product.rows as usize / 64 + 1];
        for &row in &product.seeds {
            set_bit(&mut words, row);
            stack.push(bases[index] + row);
        }
        bits.push(words.into_boxed_slice());
    }
    let edge_total: usize = products.values().map(|product| product.edges.len()).sum();
    let mut edges: Vec<(u32, u32)> = Vec::with_capacity(edge_total);
    for (index, file) in order.iter().enumerate() {
        let from_base = bases[index];
        for &(source, target_file, target_row) in products[file].edges.iter() {
            let Some(&target_unit) = unit_of.get(&target_file) else {
                continue;
            };
            edges.push((from_base + source, bases[target_unit as usize] + target_row));
        }
    }

    // CSR adjacency, then the mark-before-push worklist from the entries.
    let mut offsets = vec![0u32; node_total as usize + 1];
    for &(from, _) in &edges {
        offsets[from as usize + 1] += 1;
    }
    for i in 1..offsets.len() {
        offsets[i] += offsets[i - 1];
    }
    let mut adjacency = vec![0u32; edges.len()];
    let mut cursor = offsets.clone();
    for &(from, to) in &edges {
        adjacency[cursor[from as usize] as usize] = to;
        cursor[from as usize] += 1;
    }
    while let Some(node) = stack.pop() {
        for &to in &adjacency[offsets[node as usize] as usize..offsets[node as usize + 1] as usize]
        {
            let (unit_index, row) = locate(&bases, to);
            if get_bit(&bits[unit_index], row) {
                continue;
            }
            set_bit(&mut bits[unit_index], row);
            stack.push(to);
        }
    }

    Arc::new(Reachability {
        reachable: order.into_iter().zip(bits).collect(),
        units: products,
    })
}

/// Derives one unit's product: the entry seeds and the propagation edges of
/// one member file, from its capture (the node rows and the attribution
/// geometry), its owner surface's reference rows for the file, and its
/// text. Every coordinate is file-local; [`build`] renumbers at assembly.
#[expect(clippy::too_many_lines)]
#[expect(clippy::too_many_arguments)] // one argument per derive input; a bundling struct would only relocate the list
fn derive_unit(
    state: &EngineState,
    capture: &FileCapture,
    text: &str,
    rows: &mut Vec<&RefRow>,
    key: (ContentId, LibraryId, u32),
    main: StringId,
    pragma: StringId,
    preview: StringId,
    universe: &FxHashSet<FileId>,
) -> UnitProduct {
    rows.sort_unstable_by_key(|row| row.range.offset);
    // The attribution geometry: declaration rows as `(name-token offset,
    // row)`, ascending — the table binary searches (`decls` sorts by
    // name, not offset).
    let mut by_offset: Vec<(u32, u32)> = capture
        .index
        .decls()
        .iter()
        .enumerate()
        .map(|(row, decl)| {
            (
                decl.range.offset,
                u32::try_from(row).expect("decl row fits u32"),
            )
        })
        .collect();
    by_offset.sort_unstable_by_key(|&(offset, _)| offset);

    let mut seeds: Vec<u32> = Vec::new();
    // Entry kind (a): top-level functions named `main` — a getter spelled
    // `main` also qualifies (one folded top-level kind; a setter carries
    // the `main=` spelling and never matches).
    for (row, decl) in capture.index.decls().iter().enumerate() {
        if decl.kind == DeclKind::TopLevelFn && decl.name == main {
            seeds.push(u32::try_from(row).expect("decl row fits u32"));
        }
    }

    let byte_at = utf16_byte_map(text, rows);

    // Annotation windows: an `@`-headed reference row opens
    // `(start, end, bound)` — everything between the annotation head
    // and the next declaration's name attributes to the bound row.
    // An annotation with no following declaration opens no window.
    let mut windows: Vec<(u32, u32, u32)> = rows
        .iter()
        .enumerate()
        .filter(|&(index, row)| {
            row.kind == RefKind::Referenced
                && !row.qualified()
                && preceded_by_annotation(text, byte_at[index])
        })
        .filter_map(|(_, row)| {
            let (end, bound) = next_decl_bound(&by_offset, row.range.offset)?;
            Some((row.range.offset, end, bound))
        })
        .collect();
    windows.sort_unstable_by_key(|&(start, _, _)| start);

    let mut edges: Vec<(u32, FileId, u32)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        // Entries: pragma / Preview annotations on their bound row. A
        // bare `@name` head binds through its own window; a qualified
        // tail (`@core.pragma`, `@p.Preview`) binds by its own
        // geometry — the prefix-armed head records no reference row
        // and opens no window.
        if row.kind == RefKind::Referenced {
            let bound = match window_at(&windows, row.range.offset) {
                Some((_, _, bound)) => Some(bound),
                None if row.qualified() => {
                    next_decl_bound(&by_offset, row.range.offset).map(|(_, bound)| bound)
                }
                None => None,
            };
            if let Some(bound) = bound {
                let entry = if row.target_name == pragma {
                    // Bare `@pragma` resolves through the implicit
                    // dart:core import to the synthetic face node, whose
                    // file is interned but never resident; a workspace
                    // target means a shadowing declaration won.
                    !state.residency.contains_key(&row.target_file)
                        && is_vm_entry_point_pragma(
                            text,
                            // Name tokens are ASCII, so the UTF-16 length
                            // is the byte length.
                            byte_at[index] + row.range.length as usize,
                        )
                } else if row.target_name == preview {
                    state
                        .library_of_file
                        .get(&row.target_file)
                        .and_then(|library| state.libraries.info(*library))
                        .is_some_and(|info| info.uri.as_ref() == PREVIEW_LIBRARY)
                } else {
                    false
                };
                if entry {
                    seeds.push(bound);
                }
            }
        }

        // Edges: the use site's enclosing declaration(s) → target.
        let Some(source) = source_row(&by_offset, &windows, row.range.offset) else {
            continue;
        };
        let nearest = &capture.index.decls()[source as usize];
        let second = (nearest.container != DeclRow::NONE).then(|| top_container(capture, source));
        let sources = [Some(source), second];
        if !universe.contains(&row.target_file) {
            // An SDK or synthetic target has no workspace row — the
            // official declaration-map miss contributes nothing.
            continue;
        }
        let target = state
            .capture(row.target_file)
            .expect("universe member has a capture after analyze");
        let decls = target.index.decls();
        let first = decls.partition_point(|decl| decl.name < row.target_name);
        let run = decls[first..].partition_point(|decl| decl.name == row.target_name);
        if run == 0 {
            // A resolved reference row's target has a declaring row (the
            // resolver admits library faces of live captures only); a
            // phase disagreement contributes nothing, like the SDK miss
            // above.
            continue;
        }
        for source in sources.into_iter().flatten() {
            for target_row in first..first + run {
                edges.push((
                    source,
                    row.target_file,
                    u32::try_from(target_row).expect("decl row fits u32"),
                ));
            }
        }
        // A member-targeted reference also marks the enclosing
        // top-level container (the official double marking).
        if decls[first].container != DeclRow::NONE {
            let top = top_container(target, u32::try_from(first).expect("decl row fits u32"));
            for source in sources.into_iter().flatten() {
                edges.push((source, row.target_file, top));
            }
        }
    }

    UnitProduct {
        key,
        rows: u32::try_from(capture.index.decls().len()).expect("row count fits u32"),
        seeds,
        edges: edges.into(),
    }
}

/// The declaration row a use at `offset` attributes to: the bound row of
/// an enclosing annotation window, else the nearest preceding declaration
/// row. `None` when nothing precedes the use (the file's leading region).
fn source_row(by_offset: &[(u32, u32)], windows: &[(u32, u32, u32)], offset: u32) -> Option<u32> {
    if let Some((_, _, bound)) = window_at(windows, offset) {
        return Some(bound);
    }
    let index = by_offset.partition_point(|&(start, _)| start <= offset);
    let &(_, row) = index.checked_sub(1).and_then(|i| by_offset.get(i))?;
    Some(row)
}

/// The `(end, bound)` of the annotation window opening at `start`: the
/// next declaration row's name offset and its index.
fn next_decl_bound(by_offset: &[(u32, u32)], start: u32) -> Option<(u32, u32)> {
    let index = by_offset.partition_point(|&(offset, _)| offset <= start);
    by_offset.get(index).copied()
}

/// The window covering `offset`, if any: the last window starting at or
/// before it, provided the offset precedes the window's end.
fn window_at(windows: &[(u32, u32, u32)], offset: u32) -> Option<(u32, u32, u32)> {
    let index = windows
        .partition_point(|&(start, _, _)| start <= offset)
        .checked_sub(1)?;
    let window = windows[index];
    (offset < window.1).then_some(window)
}

/// The top of `row`'s container chain — the enclosing top-level
/// declaration row (containers are declared before their members, so the
/// walk strictly descends the row indexes and terminates).
fn top_container(capture: &FileCapture, row: u32) -> u32 {
    let decls = capture.index.decls();
    let mut row = row;
    while decls[row as usize].container != DeclRow::NONE {
        row = decls[row as usize].container;
    }
    row
}

/// Byte offsets parallel to `rows` (sorted by UTF-16 offset): one linear
/// walk over the text instead of a per-row scan. Offsets past the text's
/// UTF-16 length clamp to `text.len()`.
#[expect(clippy::cast_possible_truncation)] // a char's UTF-16 width is 1 or 2
fn utf16_byte_map(text: &str, rows: &[&RefRow]) -> Vec<usize> {
    let mut map = Vec::with_capacity(rows.len());
    let mut consumed: u32 = 0;
    let mut chars = text.char_indices();
    let mut cursor = chars.next();
    for row in rows {
        let target = row.range.offset;
        while let Some((_, ch)) = cursor {
            let width = ch.len_utf16() as u32;
            if consumed + width > target {
                break;
            }
            consumed += width;
            cursor = chars.next();
        }
        map.push(match cursor {
            Some((byte, _)) if consumed == target => byte,
            _ => text.len(),
        });
    }
    map
}

/// Whether the bytes ending at `byte` (the start of a reference row) place
/// an `@` immediately before the name, skipping whitespace — the
/// annotation-head shape. A comment between `@` and the name hides the
/// head (the under-attribution direction; the grammar allows the gap).
fn preceded_by_annotation(text: &str, byte: usize) -> bool {
    let bytes = text.as_bytes();
    let mut i = byte;
    loop {
        if i == 0 {
            return false;
        }
        i -= 1;
        while i > 0 && bytes[i] & 0xC0 == 0x80 {
            i -= 1;
        }
        match bytes[i] {
            b' ' | b'\t' | b'\n' | b'\r' => {}
            b'@' => return true,
            _ => return false,
        }
    }
}

/// Whether the annotation whose name ends at `after_name` carries
/// `'vm:entry-point'` as its first positional argument — the pragma
/// `name` field is always the leading argument (`const pragma(this.name)`).
/// Plain string-literal spellings only: quote style, `r` prefix, and
/// triple quotes are handled, escape sequences compare raw (a
/// `'vm:\x65ntry-point'` spelling does not match — the under-entry
/// direction). Anything before the argument group's `(` but after the
/// name other than whitespace disqualifies.
fn is_vm_entry_point_pragma(text: &str, after_name: usize) -> bool {
    let bytes = text.as_bytes();
    let mut i = after_name;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if bytes.get(i) != Some(&b'(') {
        return false;
    }
    i += 1;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if matches!(bytes.get(i), Some(b'r' | b'R')) {
        i += 1;
    }
    let Some(&quote) = bytes.get(i) else {
        return false;
    };
    if quote != b'\'' && quote != b'"' {
        return false;
    }
    if bytes.len() >= i + 3 && bytes[i + 1] == quote && bytes[i + 2] == quote {
        // Triple-quoted: the body runs to the matching three-byte closer.
        let body = i + 3;
        let Some(relative) = bytes[body..]
            .windows(3)
            .position(|w| w == [quote, quote, quote])
        else {
            return false;
        };
        return &bytes[body..body + relative] == VM_ENTRY_POINT;
    }
    let body = i + 1;
    let mut j = body;
    while j < bytes.len() {
        if bytes[j] == b'\\' {
            j += 2;
            continue;
        }
        if bytes[j] == quote {
            break;
        }
        j += 1;
    }
    j < bytes.len() && &bytes[body..j] == VM_ENTRY_POINT
}

/// Dense id → `(unit index, row)` — bases ascend with the units.
fn locate(bases: &[u32], dense: u32) -> (usize, u32) {
    let index = bases.partition_point(|&base| base <= dense) - 1;
    (index, dense - bases[index])
}

fn get_bit(bits: &[u64], row: u32) -> bool {
    bits[row as usize / 64] & (1 << (row % 64)) != 0
}

fn set_bit(bits: &mut [u64], row: u32) {
    bits[row as usize / 64] |= 1 << (row % 64);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use dartq_pub::{LockFile, PackageConfig, Pubspec, build_graph};
    use rustc_hash::FxHashMap;

    use crate::{CancellationToken, DEFAULT_TARGET, Engine, EngineConfig, EngineEvent};

    /// A throwaway workspace: package `t` plus the `flutter` package the
    /// Preview test imports, the synthetic-SDK fixture shape of the facade
    /// tests. The Dart sources mount in memory (`set_content`), so only
    /// the pub faces and the SDK land on disk. Returns the engine and the
    /// mounted sources' file ids in input order, analyzed and warm.
    fn fixture(files: &[(&str, &str)]) -> (Engine, Vec<dartq_source::FileId>) {
        fixture_inner(files, true)
    }

    /// [`fixture`] without the warm analyze — the memo-never-armed shape.
    fn fixture_cold(files: &[(&str, &str)]) -> (Engine, Vec<dartq_source::FileId>) {
        fixture_inner(files, false)
    }

    fn fixture_inner(files: &[(&str, &str)], warm: bool) -> (Engine, Vec<dartq_source::FileId>) {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "dartq-engine-reach-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let write = |relative: &str, content: &str| {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(path, content).expect("file");
        };
        write(
            "pubspec.yaml",
            "name: t\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(
            "pubspec.lock",
            "packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n",
        );
        write(
            ".dart_tool/package_config.json",
            r#"{"configVersion": 2, "packages": [{"name": "t", "rootUri": "../", "languageVersion": "3.5"}, {"name": "flutter", "rootUri": "../flutter", "languageVersion": "3.5"}], "generator": "pub"}"#,
        );
        write("sdk/version", "3.13.1\n");
        write(
            "sdk/lib/libraries.json",
            r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}}}}"#,
        );
        write(
            "sdk/lib/_internal/allowed_experiments.json",
            r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
        );
        // Canonicalize before the pub faces resolve it (macOS /var vs
        // /private/var).
        let root = root.canonicalize().expect("fixture root resolves");
        let config_dir = root.join(".dart_tool");
        let config = Arc::new(
            PackageConfig::parse(
                &std::fs::read_to_string(config_dir.join("package_config.json")).expect("config"),
            )
            .expect("valid config"),
        );
        let lock = LockFile::parse("packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n")
            .expect("valid lock");
        let root_pubspec = Pubspec::parse("name: t\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n")
            .expect("valid pubspec");
        let graph = build_graph(
            &config,
            &lock,
            &root_pubspec,
            &std::collections::BTreeMap::new(),
            None,
            &config_dir,
        )
        .expect("graph");
        let mut engine = Engine::new(EngineConfig {
            store: crate::memory_store(),
            sdk: dartq_sdk::SdkSnapshot::load(&root.join("sdk"), None).expect("synthetic SDK"),
            packages: dartq_package::PackageTable::build(&config, &graph, &config_dir)
                .expect("table"),
            pub_config: Arc::clone(&config),
            config_dir,
            semdiag: None,
            target: DEFAULT_TARGET.to_owned(),
            declared_variables: FxHashMap::default(),
            experiments: Vec::new(),
        })
        .expect("engine");
        let ids = files
            .iter()
            .map(|&(path, source)| engine.set_content(&root.join(path), source.as_bytes()))
            .collect();
        if warm {
            engine
                .analyze(&[], &mut |_: EngineEvent| {}, &CancellationToken::new())
                .expect("analyze");
        }
        (engine, ids)
    }

    /// The capture row of `name`'s first declaration in `file`.
    fn row_of(engine: &Engine, file: dartq_source::FileId, name: &str) -> u32 {
        let capture = engine.file_capture(file).expect("analyzed file");
        let id = engine.strings().intern(name);
        let decls = capture.index.decls();
        let start = decls.partition_point(|row| row.name < id);
        assert!(
            start < decls.len() && decls[start].name == id,
            "declaration `{name}` present",
        );
        u32::try_from(start).expect("decl row fits u32")
    }

    /// The reachability flag of `name`'s first declaration in `file`.
    fn flag(engine: &Engine, file: dartq_source::FileId, name: &str) -> bool {
        let row = row_of(engine, file, name);
        engine
            .reachability()
            .expect("the map builds at analyze")
            .is_reachable(file, row)
            .expect("the row is in range")
    }

    /// Entry kind (a): `main` seeds; its direct callee follows; an
    /// unreferenced declaration stays unreachable.
    #[test]
    fn main_reaches_calls_and_leaves_the_rest() {
        let (engine, ids) = fixture(&[(
            "lib/a.dart",
            "void main() { helper(); }\nvoid helper() {}\nvoid stray() {}\n",
        )]);
        let [a] = ids[..] else { unreachable!() };
        assert!(flag(&engine, a, "main"));
        assert!(flag(&engine, a, "helper"));
        assert!(!flag(&engine, a, "stray"));
    }

    /// Entry kind (b): the vm:entry-point pragma, any string-literal
    /// spelling; a non-matching pragma argument is no entry.
    #[test]
    fn vm_entry_point_pragma_seeds() {
        let (engine, ids) = fixture(&[(
            "lib/a.dart",
            "void main() {}\n\n@pragma('vm:entry-point')\nvoid single() {}\n\n@pragma(\"vm:entry-point\", true)\nvoid doubleQuoted() {}\n\n@pragma('other')\nvoid notAnEntry() {}\n",
        )]);
        let [a] = ids[..] else { unreachable!() };
        assert!(flag(&engine, a, "single"));
        assert!(flag(&engine, a, "doubleQuoted"));
        assert!(!flag(&engine, a, "notAnEntry"));
    }

    /// A workspace declaration named `pragma` shadows dart:core, so the
    /// annotation resolves outside the SDK and disqualifies (the official
    /// resolved-element check).
    #[test]
    fn local_shadow_disqualifies_pragma() {
        let (engine, ids) = fixture(&[(
            "lib/a.dart",
            "class pragma {}\n\nvoid main() {}\n\n@pragma('vm:entry-point')\nvoid seeded() {}\n",
        )]);
        let [a] = ids[..] else { unreachable!() };
        assert!(!flag(&engine, a, "seeded"));
    }

    /// The prefixed spelling `@core.pragma` binds through its qualified
    /// tail (the prefix-armed head opens no window).
    #[test]
    fn prefixed_core_pragma_seeds() {
        let (engine, ids) = fixture(&[(
            "lib/a.dart",
            "import 'dart:core' as core;\n\nvoid main() {}\n\n@core.pragma('vm:entry-point')\nvoid seeded() {}\n",
        )]);
        let [a] = ids[..] else { unreachable!() };
        assert!(flag(&engine, a, "seeded"));
    }

    /// Entry kind (c): `@Preview` resolves into the widget-previews
    /// library through the import faces.
    #[test]
    fn preview_annotation_seeds() {
        let (engine, ids) = fixture(&[
            (
                "lib/a.dart",
                "import 'package:flutter/src/widget_previews/widget_previews.dart';\n\nvoid main() {}\n\n@Preview()\nvoid seeded() {}\n\nvoid stray() {}\n",
            ),
            (
                "flutter/lib/src/widget_previews/widget_previews.dart",
                "class Preview {\n  const Preview();\n}\n",
            ),
        ]);
        let [a, _flutter] = ids[..] else {
            unreachable!()
        };
        assert!(flag(&engine, a, "seeded"));
        assert!(!flag(&engine, a, "stray"));
    }

    /// A `@pragma`-annotated member is an entry (the ruling broadens
    /// entry-hood to every declaration kind; upstream seeds only from
    /// functions).
    #[test]
    fn pragma_on_member_is_an_entry() {
        let (engine, ids) = fixture(&[(
            "lib/a.dart",
            "class Holder {\n  @pragma('vm:entry-point')\n  static void entered() {}\n\n  static void stray() {}\n}\n",
        )]);
        let [a] = ids[..] else { unreachable!() };
        assert!(flag(&engine, a, "entered"));
        assert!(!flag(&engine, a, "stray"));
    }

    /// Surfaces resolve through the workspace export scopes: `main`
    /// reaching into an imported library marks the callee, and the
    /// imported library's unreferenced declarations stay unreachable.
    #[test]
    fn cross_library_edges_mark_imported_declarations() {
        let (engine, ids) = fixture(&[
            (
                "lib/a.dart",
                "import 'b.dart';\n\nvoid main() { fromB(); }\n",
            ),
            ("lib/b.dart", "void fromB() {}\nvoid isolatedB() {}\n"),
        ]);
        let [a, b] = ids[..] else { unreachable!() };
        assert!(flag(&engine, a, "main"));
        assert!(flag(&engine, b, "fromB"));
        assert!(!flag(&engine, b, "isolatedB"));
    }

    /// An annotation's argument references attribute to the annotated
    /// declaration (the window), alongside the annotation name itself.
    #[test]
    fn annotation_argument_edges() {
        let (engine, ids) = fixture(&[(
            "lib/a.dart",
            "class Ann {\n  const Ann(this.value);\n  final Object value;\n}\n\nconst kEntry = 1;\n\n@Ann(kEntry)\nvoid main() {}\n",
        )]);
        let [a] = ids[..] else { unreachable!() };
        assert!(flag(&engine, a, "main"));
        assert!(flag(&engine, a, "kEntry"));
        assert!(flag(&engine, a, "Ann"));
    }

    /// Reachability follows the reference graph, not co-location: a
    /// referenced-only-from-unreachable chain stays unreachable even
    /// though it sits next to `main`.
    #[test]
    fn unreachable_subtree_stays_unreachable() {
        let (engine, ids) = fixture(&[(
            "lib/a.dart",
            "void main() {}\n\nvoid a() { b(); }\nvoid b() {}\nvoid c() { d(); }\nvoid d() {}\n",
        )]);
        let [a_file] = ids[..] else { unreachable!() };
        for name in ["a", "b", "c", "d"] {
            assert!(!flag(&engine, a_file, name), "`{name}` stays unreachable");
        }
    }

    /// A resolved reference whose target name run covers a member row marks
    /// that member and its enclosing top-level container (the official
    /// double marking — the bare `s()` call resolves to the top-level
    /// face, and its run holds the same-named member `C.s` ahead of it);
    /// an unreferenced sibling member stays unreachable.
    #[test]
    fn member_reference_marks_member_and_container() {
        let (engine, ids) = fixture(&[(
            "lib/a.dart",
            "class C {\n  static void s() {}\n  static void t() {}\n}\n\nvoid s() {}\n\nvoid main() {\n  s();\n}\n",
        )]);
        let [a] = ids[..] else { unreachable!() };
        assert!(flag(&engine, a, "main"));
        assert!(flag(&engine, a, "C"));
        assert!(flag(&engine, a, "s"));
        assert!(!flag(&engine, a, "t"));
    }

    /// Private declarations carry flags like any other — the pass is
    /// visibility-agnostic; the lint shell applies the public face.
    #[test]
    fn private_declarations_carry_flags() {
        let (engine, ids) = fixture(&[(
            "lib/a.dart",
            "void main() { _used(); }\n\nvoid _used() {}\nvoid _dead() {}\n",
        )]);
        let [a] = ids[..] else { unreachable!() };
        assert!(flag(&engine, a, "_used"));
        assert!(!flag(&engine, a, "_dead"));
    }

    /// The map answers `None` outside its universe: an unanalyzed path
    /// (never mounted) has no file entry, and a stale row index answers
    /// no verdict.
    #[test]
    fn out_of_universe_probes_answer_none() {
        let (mut engine, ids) = fixture(&[("lib/a.dart", "void main() {}\n")]);
        let [a] = ids[..] else { unreachable!() };
        let map = engine.reachability().expect("map built");
        assert_eq!(map.is_reachable(a, 0), Some(true));
        assert_eq!(map.is_reachable(a, 9_999), None);
        // A file mounted after the map's generation has no entry: the flag
        // never answers for content the map never saw.
        let unanalyzed = engine.set_content(
            std::path::Path::new("/nowhere/mounted-late.dart"),
            b"void f() {}\n",
        );
        assert_eq!(map.is_reachable(unanalyzed, 0), None);
    }

    /// The salvage pin (#1383): the map the engine serves must agree flag
    /// for flag — `Option<bool>` for `Option<bool>`, so a universe
    /// divergence surfaces too — with a wholesale rebuild over the same
    /// state (the pre-salvage eager build, [`build`] without salvage).
    fn assert_matches_wholesale(engine: &Engine) {
        let map = engine.reachability().expect("the map builds at read");
        let whole = crate::reachability::build(&engine.state, None);
        for (_library, members) in engine.libraries() {
            for file in members {
                let capture = engine.file_capture(file).expect("analyzed member");
                let rows = u32::try_from(capture.index.decls().len()).expect("decl row fits u32");
                for row in 0..rows {
                    assert_eq!(
                        map.is_reachable(file, row),
                        whole.is_reachable(file, row),
                        "flag divergence at {:?} row {row}",
                        engine.file_table().path(file).map(|path| path.display()),
                    );
                }
            }
        }
    }

    /// The residency-pairing degrade pin (stability lane #1459): a live
    /// member whose capture vanished after the analyze — the pairing
    /// contract broken, so an engine bug — drops the unit from the map
    /// (the file answers no verdict, never a crash) and records exactly
    /// one bounded row per (file, face) pair; a rebuild over the same
    /// broken state reports nothing new (the dedup bounds the log).
    #[test]
    fn missing_capture_skips_the_unit_and_records_one_bounded_degrade() {
        let (engine, ids) = fixture(&[(
            "lib/a.dart",
            "void main() { helper(); }\nvoid helper() {}\n",
        )]);
        let [a] = ids[..] else { unreachable!() };
        // The pairing break: the capture vanishes post-analyze.
        let mut engine = engine;
        engine.state.clear_capture(a);
        let map = crate::reachability::build(&engine.state, None);
        assert_eq!(
            map.is_reachable(a, 0),
            None,
            "the skipped unit answers no verdict"
        );
        assert_eq!(
            engine.state.residency_degrades.take(),
            vec![(a, "capture")],
            "one degrade row for the (file, face) pair"
        );
        // The dedup bounds the bookkeeping: the same miss derives no row.
        let _ = crate::reachability::build(&engine.state, None);
        assert!(
            engine.state.residency_degrades.take().is_empty(),
            "already-reported pairs no-op"
        );
    }

    /// The laziness contract (#1383): no analyze, no map — mounting
    /// content never arms the memo by itself. A warm engine's apply with
    /// no following analyze serves the standing map (the map reflects the
    /// last completed analyze, not in-flight residency); the edit lands at
    /// the next analyze.
    #[test]
    fn reads_serve_none_then_the_standing_map_until_an_analyze_lands() {
        let (mut engine, ids) = fixture_cold(&[(
            "lib/a.dart",
            "void main() { helper(); }\nvoid helper() {}\n",
        )]);
        let [a] = ids[..] else { unreachable!() };
        assert!(engine.reachability().is_none(), "no analyze, no map");
        engine
            .analyze(&[], &mut |_: EngineEvent| {}, &CancellationToken::new())
            .expect("analyze");
        assert!(flag(&engine, a, "helper"));
        // Apply without analyze: the pre-edit map stays served.
        let a_path = engine.file_table().path(a).expect("mounted").to_path_buf();
        engine.set_content(&a_path, b"void main() {}\nvoid helper() {}\n");
        engine.apply_pending(&mut |_: EngineEvent| {});
        assert!(
            flag(&engine, a, "helper"),
            "in-flight residency is not served"
        );
        engine
            .analyze(&[], &mut |_: EngineEvent| {}, &CancellationToken::new())
            .expect("analyze");
        assert!(!flag(&engine, a, "helper"), "the absorbed edit unmarks");
    }

    /// The salvage pin over an edit sequence (#1383): every step hits a
    /// different salvage path, and the served map stays flag-identical to
    /// a wholesale rebuild over the same state. Step 2 is the one where
    /// keeping the previous bits wholesale would be wrong: the rename
    /// recomputes both libraries, the `main → fromB` edge REMOVES, and
    /// `fromB`/`alsoB` must unmark through a fresh propagation.
    #[test]
    fn salvaged_map_matches_wholesale_over_edits() {
        let a_source = "import 'b.dart';\n\nvoid main() { fromB(); }\n\nvoid unusedA() {}\n";
        let (engine, ids) = fixture(&[
            ("lib/a.dart", a_source),
            (
                "lib/b.dart",
                "void fromB() { alsoB(); }\nvoid alsoB() {}\nvoid strayB() {}\n",
            ),
        ]);
        let [a, b] = ids[..] else { unreachable!() };
        let path_of = |engine: &Engine, file: dartq_source::FileId| {
            engine
                .file_table()
                .path(file)
                .expect("mounted file")
                .to_path_buf()
        };
        let mut engine = engine;
        let analyze = |engine: &mut Engine| {
            engine
                .analyze(&[], &mut |_: EngineEvent| {}, &CancellationToken::new())
                .expect("analyze");
        };

        // 0 — the cold build: every unit fresh.
        assert!(flag(&engine, a, "main"));
        assert!(flag(&engine, b, "alsoB"));
        assert!(!flag(&engine, b, "strayB"));
        assert!(!flag(&engine, a, "unusedA"));
        assert_matches_wholesale(&engine);

        // 1 — comment churn in a: the capture moves (the salvage key with
        // it), the derived payload stands — the standing-bits fast path.
        let a_path = path_of(&engine, a);
        let mut churned = a_source.to_owned();
        churned.insert_str(churned.find("void main").expect("anchor"), "// churn\n");
        engine.set_content(&a_path, churned.as_bytes());
        analyze(&mut engine);
        assert!(flag(&engine, a, "main"), "main still reaches fromB");
        assert!(flag(&engine, b, "fromB"));
        assert_matches_wholesale(&engine);

        // 2 — cross-library rename in b: both libraries recompute (a via
        // the cycle fingerprint), the reference row for `fromB` vanishes
        // from a's fresh surface, and the re-run propagation unmarks the
        // callee chain. The old `fromB` row left the universe with the
        // rename (a name probe would answer nothing), so the unmark is
        // asserted on `alsoB` and the slot parity rides the wholesale
        // check below.
        engine.set_content(
            &path_of(&engine, b),
            b"void fromB2() { alsoB(); }\nvoid alsoB() {}\nvoid strayB() {}\n",
        );
        analyze(&mut engine);
        assert!(!flag(&engine, b, "alsoB"), "the removed edge unmarks");
        assert!(!flag(&engine, b, "fromB2"));
        assert_matches_wholesale(&engine);

        // 3 — a mount: the unit universe grows and main reaches into the
        // new file.
        let c_path = a_path.parent().expect("the lib directory").join("c.dart");
        let c = engine.set_content(&c_path, b"void fromC() {}\n");
        engine.set_content(
            &a_path,
            b"import 'b.dart';\nimport 'c.dart';\n\nvoid main() { fromC(); }\n\nvoid unusedA() {}\n",
        );
        analyze(&mut engine);
        assert!(flag(&engine, c, "fromC"));
        assert_matches_wholesale(&engine);

        // 4 — a removal: c leaves the universe with the call to it.
        engine.set_content(
            &a_path,
            b"import 'b.dart';\n\nvoid main() {}\n\nvoid unusedA() {}\n",
        );
        engine.remove_file(&c_path).expect("remove");
        analyze(&mut engine);
        assert_matches_wholesale(&engine);
    }

    /// Replaces `file`'s capture with one captured over `text` — the seam
    /// that forges a producer/consumer phase disagreement (a resolved use
    /// whose target capture lost the declaring rows) without going through
    /// the apply path.
    fn forge_capture(engine: &mut Engine, file: dartq_source::FileId, text: &str) {
        let capture = {
            let state = &engine.state;
            let version = crate::l1::language_version_for(&state.packages, &state.files, file);
            let (feature_set, settled, scanned, _) =
                crate::l1::scan_unit(text, version, file, &state.grants());
            let face = crate::state::ScannedFace {
                tokens: scanned.tokens,
                features: feature_set,
                version: settled,
            };
            crate::pipeline::capture_surface(file, text, &face, &state.strings)
        };
        engine.state.set_capture(file, Arc::new(capture));
    }

    /// A resolved reference whose target capture lost its declaring row is
    /// a phase disagreement: the edge contributes nothing (the SDK-miss
    /// arm's under-report) and the map still builds — never a panic.
    #[test]
    fn a_target_phase_disagreement_degrades_to_a_map_miss() {
        let (mut engine, ids) = fixture(&[
            ("lib/a.dart", "void target() {}\n"),
            (
                "lib/b.dart",
                "import 'a.dart';\n\nvoid main() { target(); }\n",
            ),
        ]);
        let [a, b] = ids[..] else { unreachable!() };
        // Sanity: the cross-file call edge resolves before the forge.
        assert!(flag(&engine, a, "target"));
        assert!(flag(&engine, b, "main"));

        // Forge the disagreement: a's capture loses the declaring row its
        // resolved use points at. b is a's mirror here — a content move on
        // b busts the unit salvage, so the fresh derive walks the stale use
        // into the decl-less capture, while a itself stays undirty and the
        // forge survives the recapture drain.
        forge_capture(&mut engine, a, "// the declaration is gone\n");
        let b_path = engine
            .file_table()
            .path(b)
            .expect("mounted file")
            .to_path_buf();
        engine.set_content(&b_path, b"import 'a.dart';\n\nvoid main() { target(); }\n");
        engine
            .analyze(&[], &mut |_: EngineEvent| {}, &CancellationToken::new())
            .expect("analyze");

        // The fresh derive skips the target-less edge; `main` still seeds
        // from its own capture.
        assert!(flag(&engine, b, "main"), "the map builds and main seeds");
    }
}
