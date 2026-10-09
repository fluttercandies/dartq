//! CSR-friendly graph exports (the GPU posture directive): the import
//! graph and the reference graph as flat `&[u32]` offsets/edges arrays.
//!
//! The exports are **views built once and shared by slice** — a compute
//! backend reads `offsets`/`edges` without any restructuring of the index
//! store, and node numbering is deterministic (spec §12.4). The import
//! graph projects the #117 [`LibraryGraph`] edges (the single source of
//! truth for import state — the §8 cut keeps it out of the index store);
//! the reference graph is the CSR projection of the built
//! [`LibraryIndex`] tables.

use rustc_hash::FxHashMap;

use dartq_resolution::{LibraryGraph, LibraryId};

use crate::DeclKey;
use crate::SymbolIndex;

/// The import graph as CSR: node `i` is `libs[i]` (sorted ascending by
/// [`LibraryId`], so the projection is deterministic regardless of caller
/// order), and `offsets[i] .. offsets[i+1]` indexes `edges` — the resolved
/// import targets in directive order (unresolved imports contribute
/// nothing, matching the namespace builder). Built from the graph, never
/// duplicated into the index store (the §8 single-source-of-truth cut).
#[derive(Debug)]
pub struct ImportCsr {
    libs: Box<[LibraryId]>,
    offsets: Box<[u32]>,
    edges: Box<[u32]>,
}

impl ImportCsr {
    /// Projects the import edges of `libs` out of `graph`.
    #[must_use]
    pub fn build(graph: &LibraryGraph, libs: &[LibraryId]) -> Self {
        let mut libs = libs.to_vec();
        libs.sort_unstable();
        libs.dedup();
        let mut edges = Vec::new();
        let mut offsets = Vec::with_capacity(libs.len() + 1);
        offsets.push(0u32);
        for lib in &libs {
            for edge in graph.import_edges(*lib) {
                if let Some(target) = edge.target {
                    // Targets outside `libs` keep their id: the edge array
                    // is library ids, not slots — consumers join against
                    // the full graph, which owns every id.
                    edges.push(target.index());
                }
            }
            offsets.push(crate::narrow(edges.len()));
        }
        Self {
            libs: libs.into_boxed_slice(),
            offsets: offsets.into_boxed_slice(),
            edges: edges.into_boxed_slice(),
        }
    }

    /// The node libraries, ascending by [`LibraryId`].
    #[must_use]
    pub fn libs(&self) -> &[LibraryId] {
        &self.libs
    }

    /// The offsets array (`node_count + 1` entries).
    #[must_use]
    pub fn offsets(&self) -> &[u32] {
        &self.offsets
    }

    /// The edges array, grouped by source library.
    #[must_use]
    pub fn edges(&self) -> &[u32] {
        &self.edges
    }

    /// The number of nodes (libraries).
    #[must_use]
    pub fn node_count(&self) -> u32 {
        crate::narrow(self.libs.len())
    }

    /// The resolved import targets of `lib` (library ids, not slots).
    #[must_use]
    pub fn neighbors(&self, lib: LibraryId) -> &[u32] {
        let slot = self.libs.partition_point(|&candidate| candidate < lib);
        if self.libs.get(slot) != Some(&lib) {
            return &[];
        }
        let start = self.offsets[slot] as usize;
        let end = self.offsets[slot + 1] as usize;
        &self.edges[start..end]
    }
}

/// One reference-site address in the source index: which library's table
/// holds the row, at which row index — the attributes (relation kind,
/// span, qualified bit) resolve through [`LibraryIndex::refs`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SiteRef {
    /// The library whose reference table holds the row.
    pub library: LibraryId,
    /// The row index in that library's table.
    pub row: u32,
}

/// The workspace reference graph as CSR over declarations: nodes are the
/// numbered `DeclKey`s (all declared rows plus all reference targets,
/// ascending by `(file, name)`); `offsets[node] .. offsets[node+1]` indexes
/// `edges`, whose entries are ordinals into `sites` — the flat
/// `(library, row)` addresses of the referencing sites.
#[derive(Debug)]
pub struct ReferenceCsr {
    offsets: Box<[u32]>,
    edges: Box<[u32]>,
    nodes: Box<[DeclKey]>,
    sites: Box<[SiteRef]>,
}

impl ReferenceCsr {
    /// Projects every indexed library's reference rows into the flat form.
    ///
    /// Build-time maps are dropped on return; the result is sized by the
    /// declaration and reference counts.
    #[must_use]
    pub fn build(index: &SymbolIndex) -> Self {
        // Nodes: declared keys first, then reference targets, deduped —
        // ascending (file, name) so numbering is deterministic.
        let mut keys: Vec<DeclKey> = Vec::new();
        for (file, file_index) in index.files() {
            keys.extend(file_index.decls().iter().map(|row| DeclKey {
                file,
                name: row.name,
            }));
        }
        for (_, library) in index.libraries() {
            keys.extend(library.refs().iter().map(|row| DeclKey {
                file: row.target_file,
                name: row.target_name,
            }));
        }
        keys.sort_unstable();
        keys.dedup();
        let numbers: FxHashMap<DeclKey, u32> = keys
            .iter()
            .enumerate()
            .map(|(number, key)| (*key, crate::narrow(number)))
            .collect();

        // Sites in canonical (library, row) order, numbered for the edge
        // payload.
        let sites: Vec<SiteRef> = index
            .libraries()
            .flat_map(|(lib, library)| {
                (0..crate::narrow(library.reference_count()))
                    .map(move |row| SiteRef { library: lib, row })
            })
            .collect();
        let ordinals: FxHashMap<SiteRef, u32> = sites
            .iter()
            .enumerate()
            .map(|(ordinal, site)| (*site, crate::narrow(ordinal)))
            .collect();

        // Edges grouped by target node; ties in site order (libraries
        // ascend by id, rows by table order), so the projection is
        // deterministic. Every reference target was added to `keys` and
        // every table row to `sites` above, so both lookups are hits; a
        // miss would be a build bug and surfaces as one.
        let mut pairs: Vec<(u32, u32)> = Vec::new();
        for (lib, library) in index.libraries() {
            for (row, ref_row) in library.refs().iter().enumerate() {
                let key = DeclKey {
                    file: ref_row.target_file,
                    name: ref_row.target_name,
                };
                let site = SiteRef {
                    library: lib,
                    row: crate::narrow(row),
                };
                pairs.push((numbers[&key], ordinals[&site]));
            }
        }
        pairs.sort_unstable();

        let mut offsets = vec![0u32; keys.len() + 1];
        for &(node, _) in &pairs {
            offsets[node as usize + 1] += 1;
        }
        for i in 1..offsets.len() {
            offsets[i] += offsets[i - 1];
        }
        let edges: Vec<u32> = pairs.iter().map(|&(_, ordinal)| ordinal).collect();
        Self {
            offsets: offsets.into_boxed_slice(),
            edges: edges.into_boxed_slice(),
            nodes: keys.into_boxed_slice(),
            sites: sites.into_boxed_slice(),
        }
    }

    /// The node keys, ascending by `(file, name)`.
    #[must_use]
    pub fn nodes(&self) -> &[DeclKey] {
        &self.nodes
    }

    /// The offsets array (`node_count + 1` entries).
    #[must_use]
    pub fn offsets(&self) -> &[u32] {
        &self.offsets
    }

    /// The edges array; each entry is an ordinal into [`Self::sites`].
    #[must_use]
    pub fn edges(&self) -> &[u32] {
        &self.edges
    }

    /// The site addresses behind the edges, in edge order.
    #[must_use]
    pub fn sites(&self) -> &[SiteRef] {
        &self.sites
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::LibraryBuilder;
    use crate::library::tests::fixture;
    use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder};
    use dartq_source::StringTable;
    use std::path::Path;

    /// An import the graph could not resolve contributes no edge: only the
    /// resolved directive shows up as a neighbor.
    #[test]
    fn unresolved_imports_contribute_no_edges() {
        let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
        let mut builder = LibraryGraphBuilder::new(strings);
        let lib_l = builder.intern("package:t/l.dart");
        let lib_r = builder.intern("package:t/r.dart");
        builder.import(
            lib_l,
            ImportEdge {
                target: None,
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        builder.import(
            lib_l,
            ImportEdge {
                target: Some(lib_r),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();

        let imports = ImportCsr::build(&graph, &[lib_l, lib_r]);
        assert_eq!(
            imports.neighbors(lib_l),
            &[lib_r.index()],
            "only the resolved directive is an edge"
        );
        assert_eq!(imports.neighbors(lib_r), &[] as &[u32]);
        assert_eq!(imports.edges().len(), 1);
    }

    /// Duplicate library ids collapse to one node: the node list is a set,
    /// so a caller repeating an id neither duplicates nodes nor edges.
    #[test]
    fn duplicate_libraries_dedup_to_one_node() {
        let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
        let mut builder = LibraryGraphBuilder::new(strings);
        let lib_l = builder.intern("package:t/l.dart");
        let lib_r = builder.intern("package:t/r.dart");
        let graph = builder.finish();

        let imports = ImportCsr::build(&graph, &[lib_r, lib_l, lib_r]);
        assert_eq!(imports.libs(), &[lib_l, lib_r], "duplicates collapse");
        assert_eq!(imports.node_count(), 2);
        assert_eq!(imports.offsets().len(), 3);
    }

    /// The CSR faces over the two-file fixture: flat `&[u32]` arrays with
    /// deterministic numbering, the import edges projected from the graph,
    /// and the reference graph grouped by declaration.
    #[test]
    fn csr_exports_are_flat_and_deterministic() {
        let fx = fixture();
        let file_a = fx.files.try_file_id(Path::new("a.dart")).unwrap();
        let strings = fx.strings;

        let mut builder = LibraryBuilder::new(fx.lib_b, &fx.graph, &fx.namespaces);
        builder.add_file(&fx.capture_b);
        let b_surface = builder.finish();
        let mut builder = LibraryBuilder::new(fx.lib_a, &fx.graph, &fx.namespaces);
        builder.add_file(&fx.capture_a);
        let a_surface = builder.finish();

        let mut index = SymbolIndex::default();
        index.insert_file(fx.capture_a.file, fx.capture_a.index);
        index.insert_file(fx.capture_b.file, fx.capture_b.index);
        index.insert_library(fx.lib_a, a_surface);
        index.insert_library(fx.lib_b, b_surface);

        // Import graph: both b directives resolve to a; edges are library
        // ids in directive order; a imports nothing.
        let imports = ImportCsr::build(&fx.graph, &[fx.lib_b, fx.lib_a]);
        assert_eq!(imports.libs(), &[fx.lib_a, fx.lib_b], "sorted nodes");
        assert_eq!(imports.node_count(), 2);
        assert_eq!(imports.offsets().len(), 3);
        assert_eq!(
            imports.neighbors(fx.lib_b),
            &[fx.lib_a.index(), fx.lib_a.index()]
        );
        assert_eq!(imports.neighbors(fx.lib_a), &[] as &[u32]);

        // Reference graph: every declared row plus every target is a node.
        let csr = ReferenceCsr::build(&index);
        let fa_node = csr
            .nodes()
            .iter()
            .position(|key| key.file == file_a && key.name == strings.intern("fa"))
            .expect("fa node");
        let start = csr.offsets()[fa_node] as usize;
        let end = csr.offsets()[fa_node + 1] as usize;
        assert_eq!(end - start, 2, "both call sites group under fa");
        // Offsets are monotone and total the edge count.
        assert_eq!(*csr.offsets().last().unwrap() as usize, csr.edges().len());
        assert!(csr.offsets().windows(2).all(|w| w[0] <= w[1]));
        // Edges are site ordinals resolving back into the library table.
        for &ordinal in csr.edges() {
            let site = &csr.sites()[ordinal as usize];
            assert_eq!(site.library, fx.lib_b);
            assert!((site.row as usize) < index.library(fx.lib_b).unwrap().reference_count());
        }
        // Deterministic numbering (spec §12.4): rebuilding reproduces the
        // exact arrays.
        let again = ReferenceCsr::build(&index);
        assert_eq!(csr.nodes(), again.nodes());
        assert_eq!(csr.offsets(), again.offsets());
        assert_eq!(csr.edges(), again.edges());
    }
}
