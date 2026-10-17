//! The reversed import graph: one shared CSR build for every consumer that
//! asks "who imports this library" — the [`impact`](crate::impact) query today, the
//! amortized batch queries and the T8 explain stage after it.
//!
//! # Layout
//!
//! Edge `importer → imported` inverts to a row of importers per imported
//! library, stored as a flat CSR array pair (the rust-analyzer
//! reverse-dependency posture, `base-db/src/input.rs`
//! `transitive_rev_deps`): `offsets` (`library_count + 1` entries) and
//! `importers` (one entry per edge) — no per-node heap rows. The build is
//! two counting passes over the library id space, one allocation per
//! array. Libraries are visited in ascending order on the fill pass, so
//! every row is ascending — the property `impact`'s deterministic output
//! and the explain stage's row order both stand on.
//!
//! # Contracts
//!
//! The id space is the caller's dense `u32` library space, the same one
//! `impact` uses. Edges naming a target outside `0..library_count` are a
//! caller bug, surfaced never clamped (the dartq-index posture this crate
//! shares): debug builds assert with the offending target named, release
//! builds stop on the array bounds check.
//!
//! The read side is equally strict. An out-of-range `lib` in
//! [`RevImportGraph::importers_of`]
//! panics too — a lenient empty slice would read as "nothing imports it"
//! and silently misstate impact, which is exactly the caller bug the
//! contract exists to catch.

/// The import graph inverted: per imported library, its ascending importer
/// rows, as one flat CSR array pair built once and shared by every
/// reverse-dependency consumer.
#[derive(Debug, Clone)]
pub struct RevImportGraph {
    /// Row starts, `library_count + 1` entries; `offsets[lib]..offsets[lib + 1]`
    /// slices row `lib` out of [`Self::importers`].
    offsets: Vec<u32>,
    /// All importer rows, back to back; length `offsets[library_count]`.
    importers: Vec<u32>,
}

impl RevImportGraph {
    /// Builds the inverted graph over `library_count` libraries.
    ///
    /// `imports_of(lib)` yields the resolved import targets of `lib` (the
    /// caller's id space, `0..library_count`), borrowed through `'a`.
    /// Two counting passes over the id space — one to size the rows, one
    /// to fill them — so the closure runs exactly twice per library.
    ///
    /// # Panics
    ///
    /// Panics when `library_count` exceeds [`u32::MAX`] — the arrays are
    /// sized by input, so a four-billion-library workspace is a program
    /// bug to surface, not truncate.
    ///
    /// Panics when an `imports_of` target is at or beyond `library_count`
    /// (debug builds assert first, naming the offending target; release
    /// builds stop on the array bounds check) — the CSR never drops or
    /// wraps an edge.
    ///
    /// Panics when the edge count exceeds [`u32::MAX`] — like the library
    /// cap, a workspace that size is a program bug to surface, not
    /// truncate (the counters are checked adds, never wrapping).
    #[must_use]
    pub fn build<'a>(library_count: usize, imports_of: impl Fn(u32) -> &'a [u32]) -> Self {
        let count = u32::try_from(library_count).expect("more than u32::MAX libraries");
        let mut offsets = vec![0u32; library_count + 1];
        for lib in 0..count {
            for &target in imports_of(lib) {
                debug_assert!(
                    target < count,
                    "import target {target} out of range 0..{count}"
                );
                let slot = &mut offsets[target as usize + 1];
                *slot = slot
                    .checked_add(1)
                    .expect("more than u32::MAX import edges");
            }
        }
        for i in 1..offsets.len() {
            let prev = offsets[i - 1];
            let slot = &mut offsets[i];
            *slot = slot
                .checked_add(prev)
                .expect("more than u32::MAX import edges");
        }
        let mut importers = vec![0u32; offsets[library_count] as usize];
        let mut fill: Vec<u32> = offsets[..library_count].to_vec();
        for lib in 0..count {
            for &target in imports_of(lib) {
                let slot = fill[target as usize] as usize;
                importers[slot] = lib;
                fill[target as usize] += 1;
            }
        }
        Self { offsets, importers }
    }

    /// The number of libraries the graph covers; every valid id is
    /// `0..library_count()`.
    #[must_use]
    pub fn library_count(&self) -> usize {
        self.offsets.len() - 1
    }

    /// The importers of `lib`, ascending — the libraries that import it,
    /// empty when none do.
    ///
    /// # Panics
    ///
    /// Panics when `lib` is at or beyond [`Self::library_count`] — a
    /// caller bug (debug builds assert with the id named; release builds
    /// stop on the array bounds check). An empty slice for such a read
    /// would silently answer "nothing imports it".
    #[must_use]
    pub fn importers_of(&self, lib: usize) -> &[u32] {
        debug_assert!(
            lib < self.library_count(),
            "library {lib} out of range 0..{}",
            self.library_count()
        );
        &self.importers[self.offsets[lib] as usize..self.offsets[lib + 1] as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_imports(_: u32) -> &'static [u32] {
        &[]
    }

    /// Construction invariants on a multi-edge graph: offsets monotone,
    /// their last entry equals the edge count, rows ascending. Four
    /// libraries `a=0, b=1, c=2, d=3`: `b`, `c`, `d` import `a`, and `c`
    /// also imports `b` — so `a`'s row must come out `[1, 2, 3]`.
    #[test]
    fn invariants_hold_on_multi_edge_graph() {
        let imports: [&[u32]; 4] = [&[], &[0], &[0, 1], &[0]];
        let g = RevImportGraph::build(4, |lib| imports[lib as usize]);
        assert_eq!(g.library_count(), 4);
        assert!(
            g.offsets.windows(2).all(|pair| pair[0] <= pair[1]),
            "offsets are monotone"
        );
        assert_eq!(
            g.offsets[4] as usize,
            g.importers.len(),
            "last offset is the edge count"
        );
        assert_eq!(g.importers_of(0), &[1, 2, 3], "rows ascending");
        assert_eq!(g.importers_of(1), &[2]);
        assert!(g.importers_of(2).is_empty());
        assert!(g.importers_of(3).is_empty());
    }

    /// The empty graph: zero libraries — a single `[0]` offset, no edges.
    #[test]
    fn empty_graph_has_a_single_zero_offset() {
        let g = RevImportGraph::build(0, no_imports);
        assert_eq!(g.library_count(), 0);
        assert_eq!(g.offsets, [0u32]);
        assert!(g.importers.is_empty());
    }

    /// With zero libraries every read is out of range — there is no
    /// library whose importers could be asked for.
    #[test]
    #[should_panic(expected = "out of range")]
    fn empty_graph_read_is_out_of_range() {
        let g = RevImportGraph::build(0, no_imports);
        let _ = g.importers_of(0);
    }

    /// The single-edge graph: `b=1` imports `a=0` — exactly one row entry.
    #[test]
    fn single_edge_graph_answers_one_importer() {
        let imports: [&[u32]; 2] = [&[], &[0]];
        let g = RevImportGraph::build(2, |lib| imports[lib as usize]);
        assert_eq!(g.importers_of(0), &[1]);
        assert!(g.importers_of(1).is_empty());
    }

    /// An out-of-range read is a caller bug that panics with the id named
    /// (debug assert; release stops on the array bounds check) — the
    /// lenient empty slice would silently answer "nothing imports it".
    #[test]
    #[should_panic(expected = "out of range")]
    fn out_of_range_importers_read_panics() {
        let imports: [&[u32]; 2] = [&[], &[0]];
        let g = RevImportGraph::build(2, |lib| imports[lib as usize]);
        let _ = g.importers_of(2);
    }

    /// A build-time edge beyond the id space is a caller bug that panics
    /// with the offending target (debug assert; release stops on the
    /// array bounds check) — the CSR never drops or wraps an edge.
    #[test]
    #[should_panic(expected = "out of range")]
    fn out_of_range_build_target_panics() {
        let imports: [&[u32]; 2] = [&[], &[7]];
        let _ = RevImportGraph::build(2, |lib| imports[lib as usize]);
    }
}
