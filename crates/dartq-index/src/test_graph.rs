//! The test graph (PRD §15.4): `test_of` edges from test files to the
//! libraries they test, stored in the index's dependency-subgraph posture
//! (flat CSR arrays over the library id space, like [`crate::ImportCsr`]).
//!
//! # Edge derivation
//!
//! A library is a test when its identity's file basename ends `_test.dart`
//! (the Dart test convention). What the test *tests* comes from two
//! signals, in priority order:
//!
//! * **Import edges (primary)**: the test's resolved import targets. Dart's
//!   compilation model leaves no reference signal beyond imports — a
//!   cross-library symbol use requires an import of its library — so the
//!   import edges subsume the reference signal, and every resolved target
//!   is an edge (a test helper library a test imports *is* affected when
//!   the test must rerun).
//! * **Naming convention (fallback only)**: when no import resolves in the
//!   analyzed set, `foo_test.dart` tests the library whose basename is
//!   `foo.dart`. This covers the barrel-import shape, where the test
//!   reaches the tested code through a package facade and no direct import
//!   exists. It never runs when imports resolved: the import is the
//!   stronger signal and the convention is only a guess about intent.
//!
//! # Identity and contracts
//!
//! Libraries are addressed in the caller's `u32` id space (`0..count`), the
//! same convention [`crate::ImportCsr`]'s edge arrays use — the graph is
//! built once over whatever interning the caller owns and never mints ids.
//! `imports_of` must return targets below `count` (the CSR projection
//! guarantees it; an out-of-range target is a caller bug and surfaces as
//! one). Output is deterministic: `tests_of` rows ascend.

use rustc_hash::FxHashMap;

/// The basename suffix that marks a test file (PRD §15.4's fallback signal).
const TEST_SUFFIX: &str = "_test.dart";

/// The inverted `test_of` edges: which test libraries test each library.
///
/// Storage is the dependency-subgraph CSR form — `offsets[lib] ..
/// offsets[lib+1]` slices `edges` — sized by the library count and the edge
/// count, nothing else. Probes are a slice index, no hashing.
#[derive(Debug, Default)]
pub struct TestGraph {
    offsets: Box<[u32]>,
    edges: Box<[u32]>,
}

impl TestGraph {
    /// Derives the edges over `library_count` libraries: `imports_of(lib)`
    /// yields the resolved import targets of `lib`, `basename_of(lib)` its
    /// identity basename (`None` when the id holds no live library — dead
    /// slots never gain edges). Both run once per library; order of the
    /// calls is ascending id, so id-free stateful closures are safe. The
    /// borrowed returns outlive the call through `'a` (the caller's table
    /// lifetime).
    ///
    /// # Panics
    ///
    /// Panics when the library count exceeds [`u32::MAX`] — the tables are
    /// sized by input, so a four-billion-library workspace is a program bug
    /// to surface, not truncate (the crate-wide "surface, never clamp"
    /// contract).
    #[must_use]
    pub fn build<'a>(
        library_count: usize,
        mut imports_of: impl FnMut(u32) -> &'a [u32],
        mut basename_of: impl FnMut(u32) -> Option<&'a str>,
    ) -> Self {
        let count = u32::try_from(library_count).expect("more than u32::MAX libraries");
        // The basename index behind the naming fallback, built lazily: at
        // most one pass over the id space, paid only when some import-less
        // test needs the convention (an import-resolved workspace never
        // does). Keys borrow the caller's table through 'a — no copies.
        let mut by_basename: Option<FxHashMap<&'a str, Vec<u32>>> = None;
        // Per-tested-library rows, inverted from per-test edges.
        let mut rows: Vec<Vec<u32>> = vec![Vec::new(); library_count];
        for lib in 0..count {
            let Some(basename) = basename_of(lib) else {
                continue;
            };
            let Some(stem) = basename.strip_suffix(TEST_SUFFIX) else {
                continue;
            };
            // Primary signal: the resolved import targets, deduped (one
            // library may be reached by several directives).
            let mut targets: Vec<u32> = imports_of(lib).to_vec();
            targets.sort_unstable();
            targets.dedup();
            if targets.is_empty() && !stem.is_empty() {
                // Fallback only: the convention names the tested file when
                // nothing resolved in the analyzed set — one hash lookup
                // instead of scanning the whole id space per test.
                let needle = format!("{stem}.dart");
                let index = by_basename.get_or_insert_with(|| {
                    let mut map: FxHashMap<&'a str, Vec<u32>> = FxHashMap::default();
                    for other in 0..count {
                        if let Some(name) = basename_of(other) {
                            map.entry(name).or_default().push(other);
                        }
                    }
                    map
                });
                if let Some(named) = index.get(needle.as_str()) {
                    for &other in named {
                        if other != lib {
                            targets.push(other);
                        }
                    }
                }
            }
            for target in targets {
                rows[target as usize].push(lib);
            }
        }
        let mut edges = Vec::new();
        let mut offsets = Vec::with_capacity(library_count + 1);
        offsets.push(0u32);
        for row in &mut rows {
            row.sort_unstable();
            edges.extend_from_slice(row);
            offsets.push(u32::try_from(edges.len()).expect("more than u32::MAX edges"));
        }
        Self {
            offsets: offsets.into_boxed_slice(),
            edges: edges.into_boxed_slice(),
        }
    }

    /// The test libraries whose edges point at `lib`, ascending — the
    /// `AffectedTests` join of the impact query (PRD §15.4). An id outside
    /// the graph's library space — including any probe of a
    /// default-built, edge-free graph — holds no tests and answers empty.
    #[must_use]
    pub fn tests_of(&self, lib: u32) -> &[u32] {
        if lib as usize + 1 >= self.offsets.len() {
            return &[];
        }
        let start = self.offsets[lib as usize] as usize;
        let end = self.offsets[lib as usize + 1] as usize;
        &self.edges[start..end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One id space, five libraries: `a.dart`, `b.dart`, two tests, and an
    /// unrelated library. `a_test` imports `a` twice (directive dedup);
    /// `u_test` imports `a` and `b`.
    #[test]
    fn import_edges_invert_into_tests_of() {
        let imports: [(&[u32], &str); 5] = [
            (&[], "a.dart"),
            (&[], "b.dart"),
            (&[0, 0], "a_test.dart"),
            (&[0, 1], "u_test.dart"),
            (&[], "c.dart"),
        ];
        let graph = TestGraph::build(
            imports.len(),
            |lib| imports[lib as usize].0,
            |lib| Some(imports[lib as usize].1),
        );
        assert_eq!(graph.tests_of(0), &[2, 3], "both tests, ascending");
        assert_eq!(graph.tests_of(1), &[3]);
        assert_eq!(
            graph.tests_of(4),
            &[] as &[u32],
            "the untested library has no edges"
        );
        // Duplicate directives dedup: `a` is listed once per test.
        assert_eq!(graph.tests_of(0).len(), 2);
    }

    /// The naming fallback runs only when no import resolved into the
    /// analyzed set: `a_test` imports just the external suite library (the
    /// `package:test`-only shape — external targets yield no edge), so the
    /// convention adds the edge to `a.dart`; the helper gains nothing.
    #[test]
    fn naming_fallback_applies_when_imports_miss() {
        let imports: [(&[u32], &str); 4] = [
            (&[], "a.dart"),
            (&[], "helper.dart"),
            (&[], "a_test.dart"),
            (&[], "c.dart"),
        ];
        let graph = TestGraph::build(
            imports.len(),
            |lib| imports[lib as usize].0,
            |lib| Some(imports[lib as usize].1),
        );
        assert_eq!(graph.tests_of(0), &[2], "the convention names a.dart");
        assert_eq!(
            graph.tests_of(1),
            &[] as &[u32],
            "the helper is not a tested library"
        );
    }

    /// Imports win over the convention: `a_test` resolving `b.dart` keeps
    /// the edge on `b` alone — the fallback never runs alongside resolved
    /// imports, even when `a.dart` exists.
    #[test]
    fn imports_win_over_the_convention() {
        let imports: [(&[u32], &str); 4] = [
            (&[], "a.dart"),
            (&[], "b.dart"),
            (&[1], "a_test.dart"),
            (&[], "c.dart"),
        ];
        let graph = TestGraph::build(
            imports.len(),
            |lib| imports[lib as usize].0,
            |lib| Some(imports[lib as usize].1),
        );
        assert_eq!(graph.tests_of(1), &[2]);
        assert_eq!(
            graph.tests_of(0),
            &[] as &[u32],
            "the fallback is suppressed"
        );
    }

    /// Non-test libraries never gain outbound edges regardless of imports,
    /// and a test whose convention names nothing stays edge-free.
    #[test]
    fn non_tests_and_unmatched_stems_stay_edge_free() {
        let imports: [(&[u32], &str); 3] = [
            (&[1], "a.dart"),
            (&[], "b.dart"),
            (&[0], "orphan_test.dart"),
        ];
        let graph = TestGraph::build(
            imports.len(),
            |lib| imports[lib as usize].0,
            |lib| Some(imports[lib as usize].1),
        );
        assert_eq!(graph.tests_of(0), &[2], "the test's imports still count");
        assert_eq!(graph.tests_of(1), &[] as &[u32], "importer, not a test");
        assert_eq!(
            graph.tests_of(2),
            &[] as &[u32],
            "no b.dart to fall back to"
        );
    }

    /// Dead slots (no basename) contribute nothing and probe empty, and an
    /// empty id space is a valid, empty graph.
    #[test]
    fn dead_slots_and_empty_space() {
        let imports: [(&[u32], &str); 2] = [(&[], "a.dart"), (&[0], "a_test.dart")];
        let graph = TestGraph::build(
            3,
            |lib| imports.get(lib as usize).map_or(&[] as &[u32], |row| row.0),
            |lib| imports.get(lib as usize).map(|row| row.1),
        );
        assert_eq!(graph.tests_of(0), &[1]);
        assert_eq!(graph.tests_of(2), &[] as &[u32], "dead slot probes empty");

        let empty = TestGraph::build(0, |_| &[] as &[u32], |_| None);
        let all: Vec<u32> = (0..0)
            .flat_map(|lib| empty.tests_of(lib).to_vec())
            .collect();
        assert!(all.is_empty(), "no libraries, no edges");
    }
}
