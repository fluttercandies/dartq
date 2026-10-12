//! The test graph (PRD §15.4): `test_of` edges from test files to the
//! libraries they test, stored in the index's dependency-subgraph posture
//! (flat CSR arrays over the library id space).
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
//!   analyzed set, `foo_test.dart` tests the library whose defining file is
//!   `foo.dart`. The convention's implied import is the relative shape
//!   `'foo.dart'`, which the official resolver composes against the
//!   importing file's directory (`_fe_analyzer_shared`
//!   `util/resolve_relative_uri.dart` — RFC 3986 composition against the
//!   base URI; dartq-resolution `uri.rs` mirrors the same rule) — so the
//!   resolution order over same-basename candidates is fixed: the
//!   **sibling** (same directory) resolves first and wins outright, else a
//!   single remaining candidate resolves (the barrel-import shape, where
//!   the test reaches the tested code through a package facade and no
//!   direct import exists), else two or more candidates are a documented
//!   ambiguity and gain no edges. The fallback never fans out to every
//!   same-basename library (the ai-audit X1 over-report), and it never
//!   runs when imports resolved: the import is the stronger signal and the
//!   convention is only a guess about intent.
//!
//! # Identity and contracts
//!
//! Libraries are addressed in the caller's `u32` id space (`0..count`), the
//! same convention the workspace's CSR edge arrays use — the graph is
//! built once over whatever interning the caller owns and never mints ids.
//! `imports_of` must return targets below `count` (the CSR projection
//! guarantees it; an out-of-range target is a caller bug and surfaces as
//! one). Output is deterministic: `tests_of` rows ascend.

use rustc_hash::FxHashMap;
use std::path::Path;

/// The basename suffix that marks a test file (PRD §15.4's fallback signal).
const TEST_SUFFIX: &str = "_test.dart";

/// The basename index behind the naming fallback: basename -> each
/// candidate's (defining directory, library id), borrowed from the caller's
/// identity table through `'a`.
type BasenameIndex<'a> = FxHashMap<&'a str, Vec<(Option<&'a Path>, u32)>>;

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
    /// yields the resolved import targets of `lib`, `identity_of(lib)` its
    /// defining-file identity path (`None` when the id holds no live
    /// library — dead slots never gain edges). Both take the id space in
    /// ascending order, so id-free stateful closures are safe; the identity
    /// of one id may be read twice (its own pass plus the fallback index's
    /// lazy build). The borrowed returns outlive the call through `'a`
    /// (the caller's table lifetime). The naming fallback reads the
    /// directory and basename the identity path carries.
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
        mut identity_of: impl FnMut(u32) -> Option<&'a str>,
    ) -> Self {
        let count = u32::try_from(library_count).expect("more than u32::MAX libraries");
        // The basename index behind the naming fallback, built lazily: at
        // most one pass over the id space, paid only when some import-less
        // test needs the convention (an import-resolved workspace never
        // does). Keys borrow the caller's table through 'a — no copies;
        // values pair each candidate's defining directory with its id (the
        // sibling rule resolves against the directory).
        let mut by_basename: Option<BasenameIndex<'a>> = None;
        // Per-tested-library rows, inverted from per-test edges.
        let mut rows: Vec<Vec<u32>> = vec![Vec::new(); library_count];
        for lib in 0..count {
            let Some(identity) = identity_of(lib) else {
                continue;
            };
            let path = Path::new(identity);
            let Some(basename) = path.file_name().and_then(std::ffi::OsStr::to_str) else {
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
                let index = by_basename.get_or_insert_with(|| {
                    let mut map: BasenameIndex<'a> = FxHashMap::default();
                    for other in 0..count {
                        if let Some(identity) = identity_of(other) {
                            let path = Path::new(identity);
                            if let Some(basename) =
                                path.file_name().and_then(std::ffi::OsStr::to_str)
                            {
                                map.entry(basename)
                                    .or_default()
                                    .push((path.parent(), other));
                            }
                        }
                    }
                    map
                });
                // The convention's implied import composes `'stem.dart'`
                // against the test's own directory (the official relative-
                // URI rule — see the module docs), so the sibling resolves
                // first and wins outright; ids ascend in the vec, so the
                // first sibling is the deterministic pick. Without a
                // sibling, exactly one remaining candidate still resolves
                // (the barrel shape); several are an ambiguity — no edge,
                // never a fan-out.
                let needle = format!("{stem}.dart");
                let dir = path.parent();
                let mut sibling = None;
                let mut lone: Option<u32> = None;
                let mut homonyms = false;
                for &(candidate_dir, other) in index.get(needle.as_str()).into_iter().flatten() {
                    if other == lib {
                        continue;
                    }
                    if candidate_dir == dir {
                        sibling = Some(other);
                        break;
                    }
                    if lone.replace(other).is_some() {
                        homonyms = true;
                    }
                }
                if let Some(other) = sibling.or_else(|| (!homonyms).then_some(lone).flatten()) {
                    targets.push(other);
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

    /// The sibling rule (X1 precision): two same-basename `a.dart` in
    /// different directories and a sibling `test/a_test.dart` — the edge
    /// lands on the directory pairing alone, never on both homonyms. The
    /// resolved one wins; the fan-out is gone.
    #[test]
    fn sibling_wins_over_same_basename_elsewhere() {
        let identities = ["lib/a.dart", "test/a.dart", "test/a_test.dart"];
        let graph = TestGraph::build(
            identities.len(),
            |_| &[] as &[u32],
            |lib| Some(identities[lib as usize]),
        );
        assert_eq!(graph.tests_of(1), &[2], "the sibling resolves");
        assert_eq!(
            graph.tests_of(0),
            &[] as &[u32],
            "the same-basename homonym gains nothing"
        );
    }

    /// Deterministic ambiguity: no sibling, two same-basename candidates in
    /// different directories — neither gains the edge (the X1 over-report
    /// is the old all-candidates fan-out; the answer is now `none`).
    #[test]
    fn ambiguous_homonyms_gain_no_edge() {
        let identities = ["lib1/a.dart", "lib2/a.dart", "test/a_test.dart"];
        let graph = TestGraph::build(
            identities.len(),
            |_| &[] as &[u32],
            |lib| Some(identities[lib as usize]),
        );
        assert_eq!(graph.tests_of(0), &[] as &[u32]);
        assert_eq!(graph.tests_of(1), &[] as &[u32]);
    }

    /// A single non-sibling candidate still resolves (the barrel-facade
    /// shape the fallback exists for): one `a.dart` anywhere, no sibling.
    #[test]
    fn unique_non_sibling_still_resolves() {
        let identities = ["lib/a.dart", "test/a_test.dart"];
        let graph = TestGraph::build(
            identities.len(),
            |_| &[] as &[u32],
            |lib| Some(identities[lib as usize]),
        );
        assert_eq!(graph.tests_of(0), &[1], "the unique candidate resolves");
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
