//! The library-dependency condensation (PRD §11.2, g485 L3 plan §3.1 —
//! wave W1): iterative Tarjan SCC over the maintained library `deps` edges
//! and one stable blake3-128 fingerprint per strongly-connected component,
//! rebuilt wholesale under the same `structure_dirty` gate that rebuilds
//! the resolution graph (`pipeline::analyze`).
//!
//! Everything here is derived data: nothing accumulates per edit, a rebuild
//! discards the previous mapping wholesale, and retired ids never enter a
//! cycle — the freelist-recycling aliasing hazard (#230/#374) cannot reach
//! a fingerprint because values are content-derived and the mapping is
//! never incrementally maintained. Wave W1 lands the shapes and the
//! rebuild; the wave-2 member-reuse rule and the wave-3 condensation
//! ordering consume them.

use blake3::Hasher;
use dartq_semantic::ApiSignature;
use dartq_source::FileId;

use crate::state::{LibraryId, LibraryTable};

/// Arena slot of a library no cycle covers: a retired id, or a slot the
/// current mapping predates. [`Condensation::cycle_of`] answers `None`.
const NO_CYCLE: u32 = u32::MAX;

/// The fingerprint-composition tag (g485 §3.1): the version rides the
/// `v<N>` suffix. The condensation is pure derived data, so bumping the tag
/// re-derives every digest at the next rebuild automatically — nothing
/// persists a fingerprint before wave 4.
const FINGERPRINT_TAG: &[u8; 9] = b"dq-cyc-v1";

/// The import-cycle condensation over the live library set.
#[derive(Debug, Default)]
pub(crate) struct Condensation {
    /// Arena slot → cycle index, [`NO_CYCLE`] for a retired id. Rebuilt
    /// wholesale at the structure gate — never patched incrementally.
    cycle_of: Vec<u32>,
    /// One entry per strongly-connected component, in Tarjan completion
    /// order: every dependency cycle precedes its dependents, which is the
    /// order the bottom-up fingerprint fold consumes.
    cycles: Vec<CycleInfo>,
}

/// One strongly-connected component of the library graph.
// W1 pins the shapes; the wave-2 member-reuse rule reads `members`/`deps`
// — landing them consumed would unfulfill this and force its removal. The
// unit tests read everything, so the expectation is non-test-only.
#[cfg_attr(not(test), expect(dead_code))]
#[derive(Debug)]
pub(crate) struct CycleInfo {
    /// Member libraries, ascending by id — deterministic across applies,
    /// unlike Tarjan's discovery order.
    pub(crate) members: Box<[LibraryId]>,
    /// Distinct direct dependency cycles, ascending by cycle index.
    pub(crate) deps: Box<[u32]>,
    /// blake3-128 over the member signatures and the dependency-cycle
    /// fingerprints — the cycle-granular face the reuse rule compares.
    pub(crate) fingerprint: ApiSignature,
}

impl Condensation {
    /// The number of cycles in the current mapping.
    pub(crate) fn cycle_count(&self) -> usize {
        self.cycles.len()
    }

    /// The cycle covering `id`, or `None` when the id is retired or the
    /// mapping predates the id's current tenant (the wholesale rebuild at
    /// the structure gate keeps a stale answer from ever being served).
    // The wave-3 condensation ordering consumes this; see [`CycleInfo`].
    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn cycle_of(&self, id: LibraryId) -> Option<u32> {
        match self.cycle_of.get(id.index() as usize) {
            Some(&cycle) if cycle != NO_CYCLE => Some(cycle),
            _ => None,
        }
    }

    /// Derives the condensation from the live library graph: iterative
    /// Tarjan SCC (explicit frames — a 10⁵-library workspace must not
    /// recurse), each component fingerprinted bottom-up inline at
    /// completion, when every cycle it depends on is already fingerprinted.
    /// `signature` supplies a member file's baked API signature — the same
    /// value the L2 gate compares.
    pub(crate) fn build(
        libraries: &LibraryTable,
        signature: impl Fn(FileId) -> ApiSignature,
    ) -> Self {
        let arena = libraries.arena_len() as usize;
        let mut condensation = Self {
            cycle_of: vec![NO_CYCLE; arena],
            cycles: Vec::new(),
        };
        // Discovery index, NO_CYCLE = unvisited; lowlink/on_stack are the
        // Tarjan bookkeeping, sized once per rebuild (4B+4B+1B per arena
        // slot, transient beyond the mapping itself).
        let mut index = vec![NO_CYCLE; arena];
        let mut lowlink = vec![0u32; arena];
        let mut on_stack = vec![false; arena];
        let mut stack: Vec<LibraryId> = Vec::new();
        // Explicit DFS frames: the node plus the cursor into its deps edge
        // list — the recursion-free twin of the classic visit/loop pair.
        let mut frames: Vec<(LibraryId, usize)> = Vec::new();
        let mut next_index: u32 = 0;

        for (root, _) in libraries.iter_infos() {
            let root_index = root.index() as usize;
            if index[root_index] != NO_CYCLE {
                continue;
            }
            index[root_index] = next_index;
            lowlink[root_index] = next_index;
            next_index += 1;
            stack.push(root);
            on_stack[root_index] = true;
            frames.push((root, 0));
            while let Some(&(node, mut cursor)) = frames.last() {
                let deps = libraries
                    .info(node)
                    .expect("frame node is live")
                    .deps
                    .as_slice();
                let mut descended = false;
                while cursor < deps.len() {
                    let dep = deps[cursor];
                    cursor += 1;
                    // Live-gated (g485 §3.1 retirement rule): a dep id
                    // retired since its importer's formation enters no
                    // cycle and no fingerprint.
                    if libraries.info(dep).is_none() {
                        continue;
                    }
                    let di = dep.index() as usize;
                    if index[di] == NO_CYCLE {
                        index[di] = next_index;
                        lowlink[di] = next_index;
                        next_index += 1;
                        stack.push(dep);
                        on_stack[di] = true;
                        // The advanced cursor belongs to THIS frame — write
                        // it back before the child lands on top of it.
                        frames.last_mut().expect("frame under the cursor").1 = cursor;
                        frames.push((dep, 0));
                        descended = true;
                        break;
                    }
                    if on_stack[di] {
                        let ni = node.index() as usize;
                        lowlink[ni] = lowlink[ni].min(index[di]);
                    }
                }
                if descended {
                    continue;
                }
                frames.pop();
                if let Some(&(parent, _)) = frames.last() {
                    let pi = parent.index() as usize;
                    lowlink[pi] = lowlink[pi].min(lowlink[node.index() as usize]);
                }
                if lowlink[node.index() as usize] == index[node.index() as usize] {
                    // Component complete: drain the stack up to `node`, then
                    // fold the cycle — the members' signatures as a canonical
                    // multiset over the already-fingerprinted dependency
                    // cycles (completion order guarantees they exist).
                    let cycle_index = condensation.cycles.len() as u32;
                    let mut members: Vec<LibraryId> = Vec::new();
                    loop {
                        let member = stack
                            .pop()
                            .expect("component member is on the tarjan stack");
                        on_stack[member.index() as usize] = false;
                        members.push(member);
                        if member == node {
                            break;
                        }
                    }
                    members.sort_unstable();
                    for &member in &members {
                        condensation.cycle_of[member.index() as usize] = cycle_index;
                    }
                    let mut signatures: Vec<ApiSignature> = Vec::new();
                    let mut dep_cycles: Vec<u32> = Vec::new();
                    for &member in &members {
                        let info = libraries.info(member).expect("component member is live");
                        signatures.push(signature(info.defining));
                        signatures.extend(info.parts.iter().map(|&part| signature(part)));
                        for dep in &info.deps {
                            if libraries.info(*dep).is_none() {
                                continue;
                            }
                            let dep_cycle = condensation.cycle_of[dep.index() as usize];
                            assert_ne!(
                                dep_cycle, NO_CYCLE,
                                "dependency cycles complete before dependents",
                            );
                            if dep_cycle != cycle_index {
                                dep_cycles.push(dep_cycle);
                            }
                        }
                    }
                    dep_cycles.sort_unstable();
                    dep_cycles.dedup();
                    let dep_fingerprints: Vec<ApiSignature> = dep_cycles
                        .iter()
                        .map(|&dep| condensation.cycles[dep as usize].fingerprint)
                        .collect();
                    let fingerprint =
                        fold_fingerprint(FINGERPRINT_TAG, &signatures, &dep_fingerprints);
                    condensation.cycles.push(CycleInfo {
                        members: members.into_boxed_slice(),
                        deps: dep_cycles.into_boxed_slice(),
                        fingerprint,
                    });
                }
            }
        }
        condensation
    }
}

/// The cycle fingerprint (g485 §3.1): blake3 truncated to 128 bits (the
/// `ContentId` / `ApiSignature` width) over the tag, then the
/// member-signature multiset and the dependency-fingerprint set, each
/// canonicalized here — sorted (members as a multiset, deps deduplicated)
/// and length-prefixed. The bare-concatenation formula is self-delimiting
/// only with the two section lengths folded in: without them
/// `{members: [x, y], deps: [z]}` and `{members: [x], deps: [y, z]}` would
/// hash the same bytes.
fn fold_fingerprint(tag: &[u8], members: &[ApiSignature], deps: &[ApiSignature]) -> ApiSignature {
    let mut members = members.to_vec();
    members.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    let mut deps = deps.to_vec();
    deps.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    deps.dedup_by(|a, b| a.as_bytes() == b.as_bytes());
    let mut hasher = Hasher::new();
    hasher.update(tag);
    hasher.update(&(members.len() as u64).to_le_bytes());
    for member in &members {
        hasher.update(member.as_bytes());
    }
    hasher.update(&(deps.len() as u64).to_le_bytes());
    for fingerprint in &deps {
        hasher.update(fingerprint.as_bytes());
    }
    let digest = hasher.finalize();
    let mut truncated = [0; 16];
    truncated.copy_from_slice(&digest.as_bytes()[..16]);
    ApiSignature::from_bytes(truncated)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use dartq_source::FileTable;
    use rustc_hash::FxHashMap;

    use super::*;
    use crate::state::LibraryInfo;

    /// A distinct 16-byte signature per byte.
    const fn sig(byte: u8) -> ApiSignature {
        ApiSignature::from_bytes([byte; 16])
    }

    /// A live one-file library per `(uri, dep-uris)` entry; the defining
    /// file is a throwaway intern (the build reads only ids and the
    /// signature closure), so a test graph is one spec line per library.
    /// Returns the table and the ids in spec order.
    fn graph(spec: &[(&str, &[&str])]) -> (LibraryTable, Vec<LibraryId>) {
        let mut table = LibraryTable::default();
        let ids: Vec<LibraryId> = spec.iter().map(|&(uri, _)| table.intern(uri)).collect();
        let by_uri: FxHashMap<&str, LibraryId> = spec
            .iter()
            .map(|&(uri, _)| uri)
            .zip(ids.iter().copied())
            .collect();
        for (&(uri, deps), &id) in spec.iter().zip(ids.iter()) {
            table.set_info(
                id,
                LibraryInfo {
                    uri: Arc::from(uri),
                    defining: FileTable::new().intern(Path::new("/stub.dart")),
                    parts: Vec::new(),
                    content: Box::new([]),
                    deps: deps.iter().map(|dep| by_uri[dep]).collect(),
                    part_targets: Box::new([]),
                },
            );
        }
        (table, ids)
    }

    #[test]
    fn scc_shapes_two_member_cycle_bridge_and_singletons() {
        let (table, ids) = graph(&[
            ("package:a/a.dart", &["package:b/b.dart"]),
            ("package:b/b.dart", &["package:a/a.dart"]),
            ("package:c/c.dart", &["package:a/a.dart"]),
            ("package:d/d.dart", &[]),
        ]);
        let [a, b, c, d] = ids[..] else {
            unreachable!()
        };
        let condensation = Condensation::build(&table, |_| sig(0));

        assert_eq!(condensation.cycle_count(), 3);
        assert_eq!(condensation.cycle_of(a), Some(0));
        assert_eq!(condensation.cycle_of(b), Some(0));
        assert_eq!(condensation.cycle_of(c), Some(1));
        assert_eq!(condensation.cycle_of(d), Some(2));

        // The cycle: both members, ascending; no dependency outside itself.
        let ab = &condensation.cycles[0];
        assert_eq!(ab.members.as_ref(), &[a, b]);
        assert!(ab.deps.is_empty());
        // The bridge: c depends on the cycle, not on b alone.
        let downstream = &condensation.cycles[1];
        assert_eq!(downstream.members.as_ref(), &[c]);
        assert_eq!(downstream.deps.as_ref(), &[0]);
        assert_ne!(downstream.fingerprint, ab.fingerprint);
        // The singleton: independent, fingerprintless dependencies.
        let lone = &condensation.cycles[2];
        assert_eq!(lone.members.as_ref(), &[d]);
        assert!(lone.deps.is_empty());
    }

    #[test]
    fn acyclic_chain_folds_fingerprints_bottom_up() {
        let (table, _) = graph(&[
            ("package:a/a.dart", &["package:b/b.dart"]),
            ("package:b/b.dart", &["package:c/c.dart"]),
            ("package:c/c.dart", &[]),
        ]);
        let condensation = Condensation::build(&table, |_| sig(0));
        assert_eq!(condensation.cycle_count(), 3, "acyclic: singleton cycles");
        // Completion order: dependencies first.
        assert_eq!(condensation.cycles[1].deps.as_ref(), &[0]);
        assert_eq!(condensation.cycles[2].deps.as_ref(), &[1]);
        // Bottom-up fold: identical member signatures, but each position's
        // fingerprint folds its dependency's, so they all differ.
        let fingerprints: Vec<_> = condensation
            .cycles
            .iter()
            .map(|cycle| cycle.fingerprint)
            .collect();
        assert_ne!(fingerprints[0], fingerprints[1]);
        assert_ne!(fingerprints[1], fingerprints[2]);
        // The signature seam feeds the digest: a moved member signature
        // moves every fingerprint.
        let moved = Condensation::build(&table, |_| sig(1));
        for (old, new) in fingerprints.iter().zip(moved.cycles.iter()) {
            assert_ne!(*old, new.fingerprint);
        }
    }

    #[test]
    fn fingerprint_is_canonical_over_member_order() {
        // The multiset is sorted: member permutation folds identically.
        assert_eq!(
            fold_fingerprint(FINGERPRINT_TAG, &[sig(1), sig(2)], &[sig(3)]),
            fold_fingerprint(FINGERPRINT_TAG, &[sig(2), sig(1)], &[sig(3)]),
        );
        // Every input section moves the digest.
        assert_ne!(
            fold_fingerprint(FINGERPRINT_TAG, &[sig(1), sig(2)], &[sig(3)]),
            fold_fingerprint(FINGERPRINT_TAG, &[sig(1)], &[sig(3)]),
        );
        assert_ne!(
            fold_fingerprint(FINGERPRINT_TAG, &[sig(1), sig(2)], &[sig(3)]),
            fold_fingerprint(FINGERPRINT_TAG, &[sig(1), sig(2)], &[]),
        );
    }

    #[test]
    fn tag_change_moves_every_digest() {
        // The version rides the tag, and the mapping is derived-only: a tag
        // bump re-derives (moves) every fingerprint at the next rebuild.
        assert_ne!(
            fold_fingerprint(b"dq-cyc-v1", &[sig(1)], &[sig(2)]),
            fold_fingerprint(b"dq-cyc-v2", &[sig(1)], &[sig(2)]),
        );
    }

    #[test]
    fn fingerprints_are_stable_across_rebuilds() {
        let table = graph(&[
            ("package:a/a.dart", &["package:b/b.dart"]),
            ("package:b/b.dart", &["package:a/a.dart"]),
            ("package:c/c.dart", &["package:a/a.dart"]),
        ])
        .0;
        let one = Condensation::build(&table, |_| sig(3));
        let two = Condensation::build(&table, |_| sig(3));
        assert_eq!(one.cycle_count(), two.cycle_count());
        for (a, b) in one.cycles.iter().zip(two.cycles.iter()) {
            assert_eq!(
                a.fingerprint, b.fingerprint,
                "content-derived, not discovery-derived"
            );
            assert_eq!(a.members, b.members);
            assert_eq!(a.deps, b.deps);
        }
    }

    #[test]
    fn retired_ids_point_nowhere_and_recycled_ids_rebuild_from_current_facts() {
        let (mut table, ids) = graph(&[
            ("package:a/a.dart", &["package:b/b.dart"]),
            ("package:b/b.dart", &[]),
        ]);
        let (a, b) = (ids[0], ids[1]);
        // Retire b with a's deps edge still naming it: the live-gated build
        // drops the edge and the retired id answers no cycle.
        table.retire(b);
        let condensation = Condensation::build(&table, |_| sig(0));
        assert_eq!(condensation.cycle_count(), 1);
        assert_eq!(condensation.cycle_of(a), Some(0));
        assert_eq!(condensation.cycle_of(b), None, "retired id points nowhere");

        // A recycled id is a fresh tenant: a's stale edge now names the new
        // tenant (the arena's aliasing semantics — the delta's retire-after-
        // interns ordering keeps real infos coherent), and the wholesale
        // rebuild derives every cycle from the current facts, never the
        // previous incarnation's shape.
        let fresh = table.intern("package:fresh/fresh.dart");
        assert_eq!(fresh, b, "the freelist recycles the slot");
        table.set_info(
            fresh,
            LibraryInfo {
                uri: Arc::from("package:fresh/fresh.dart"),
                defining: FileTable::new().intern(Path::new("/stub.dart")),
                parts: Vec::new(),
                content: Box::new([]),
                deps: vec![a],
                part_targets: Box::new([]),
            },
        );
        let condensation = Condensation::build(&table, |_| sig(0));
        assert_eq!(
            condensation.cycle_count(),
            1,
            "a imports fresh, fresh imports a"
        );
        assert_eq!(condensation.cycle_of(fresh), Some(0));
        assert_eq!(condensation.cycles[0].members.as_ref(), &[a, fresh]);
        // The derivation is stateless: a second build reproduces the shape.
        let again = Condensation::build(&table, |_| sig(0));
        assert_eq!(again.cycles[0].members.as_ref(), &[a, fresh]);
    }
}
