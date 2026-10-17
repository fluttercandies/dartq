//! #1068 adjudication benchmark for the query-time namespace combinator:
//! lookup latency and steady-state memory of the CSR composition shape
//! against the predecessor's eagerly merged per-library maps.
//!
//! Three axes per scale (1k libraries, and the memory charter's 10⁵ red
//! line):
//!
//! 1. **Single-probe latency** — hot probes over the lookup kinds (local
//!    hit, imported hit, prefixed hit, miss, and the every-8th-library
//!    ambiguity), p50/p95/p99 over `DARTQ_BENCH_PROBES` iterations.
//! 2. **Full sweep** — every library probed once per kind, wall clock.
//! 3. **Memory shape** — [`Namespaces::memory_bytes`] (counted `size_of`
//!    sums over the CSR arrays) plus the interner entry count, against the
//!    legacy shape's counted entry total and a stated per-entry cost model
//!    (the legacy maps are only *materialized* at the 1k scale — at 10⁵
//!    they are exactly the red-line bomb this shape removes, so there the
//!    legacy figure is the measured 1k entry count extrapolated linearly).
//!
//! The legacy replica (`mod legacy`) reimplements the predecessor build —
//! eagerly merged bare-name maps and per-prefix tables over the same graph
//! — so the latency delta and the entry counts are measured, not asserted.
//!
//! Timing harness in the engine benches' convention: plain binary,
//! env-tunable probe count, self-printed acceptance axes.

use std::time::Instant;

use dartq_resolution::{
    CombinatorClause, Combinators, ImportEdge, LibraryGraph, LibraryGraphBuilder, LibraryId,
    LocalLookup, Lookup, Namespaces,
};
use dartq_source::{FileTable, StringId, StringTable};

/// The shared big face every consumer imports un-prefixed — the
/// `dart:core`-shaped duplication that dominates the legacy shape.
const CORE_NAMES: usize = 256;
/// Reused package-face pool (32 names each); consumers import members of
/// the pool so scopes are shared, as in a real workspace.
const POOL_LIBS: usize = 2_000;
const POOL_NAMES: usize = 32;
/// Local declarations per consumer library.
const LOCAL_NAMES: usize = 8;
/// Every Nth consumer imports two libraries declaring one colliding name —
/// the ambiguity path, exercised on probes.
const AMBIG_EVERY: usize = 8;

const SCALES: [usize; 2] = [1_000, 100_000];

fn main() {
    let probes: usize = std::env::var("DARTQ_BENCH_PROBES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50_000);
    for libs in SCALES {
        run_scale(libs, probes);
    }
    println!(
        "// acceptance axis: #1068 memory charter red line (10^5 libraries in 0.5-8 GB) — csr_bytes is the counted steady-state metric; the latency ratio is the composition cost the memory win buys"
    );
}

/// One generated workspace: a core-like face, a reused package-face pool,
/// and `libs` consumers with the standard import mix (bare core + one
/// pooled package face, one prefixed, one `show`-filtered, periodic
/// ambiguity).
struct Workspace {
    strings: StringTable,
    graph: LibraryGraph,
    /// The consumer libraries' ids, in generation order — the probe
    /// schedule addresses them positionally.
    consumers: Vec<LibraryId>,
}

fn add_import(
    builder: &mut LibraryGraphBuilder<'_>,
    strings: &StringTable,
    lib: LibraryId,
    target: LibraryId,
    prefix: Option<&str>,
    combinators: Combinators,
) {
    builder.import(
        lib,
        ImportEdge {
            target: Some(target),
            prefix: prefix.map(|p| strings.intern(p)),
            deferred: false,
            combinators,
        },
    );
}

fn build_workspace(libs: usize) -> Workspace {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let mut builder = LibraryGraphBuilder::new(&strings);
    let mut consumers = Vec::with_capacity(libs);

    let core = builder.intern("dart:core_face");
    let core_file = files.intern(std::path::Path::new("core_face.dart"));
    for k in 0..CORE_NAMES {
        builder.declare(core, strings.intern(&format!("core_{k}")), core_file);
    }
    let pool: Vec<LibraryId> = (0..POOL_LIBS)
        .map(|p| {
            let lib = builder.intern(&format!("package:pool{p}/pool{p}.dart"));
            let file = files.intern(std::path::Path::new(&format!("pool{p}.dart")));
            for k in 0..POOL_NAMES {
                builder.declare(lib, strings.intern(&format!("pool{p}_{k}")), file);
            }
            lib
        })
        .collect();

    for i in 0..libs {
        let lib = builder.intern(&format!("package:m{i}/m{i}.dart"));
        consumers.push(lib);
        let file = files.intern(std::path::Path::new(&format!("m{i}.dart")));
        for k in 0..LOCAL_NAMES {
            builder.declare(lib, strings.intern(&format!("local_{i}_{k}")), file);
        }
        add_import(
            &mut builder,
            &strings,
            lib,
            core,
            None,
            Combinators::unfiltered(),
        );
        add_import(
            &mut builder,
            &strings,
            lib,
            pool[i % POOL_LIBS],
            None,
            Combinators::unfiltered(),
        );
        add_import(
            &mut builder,
            &strings,
            lib,
            pool[(i * 7 + 1) % POOL_LIBS],
            Some("p"),
            Combinators::unfiltered(),
        );
        // `show` of four names the target declares — the filtered row.
        let shown = (i * 13 + 2) % POOL_LIBS;
        let clause = CombinatorClause::Show(Box::from(
            (0..4)
                .map(|k| strings.intern(&format!("pool{shown}_{k}")))
                .collect::<Vec<_>>(),
        ));
        add_import(
            &mut builder,
            &strings,
            lib,
            pool[shown],
            None,
            Combinators::new([clause], &strings),
        );
        if i.is_multiple_of(AMBIG_EVERY) {
            // Two pooled faces both declaring one name: the collision.
            let a = i % POOL_LIBS;
            let b = (i + 1) % POOL_LIBS;
            let clash = strings.intern(&format!("clash_{i}"));
            for p in [a, b] {
                let clash_file = files.intern(std::path::Path::new(&format!("pool{p}.dart")));
                builder.declare(pool[p], clash, clash_file);
            }
            add_import(
                &mut builder,
                &strings,
                lib,
                pool[b],
                None,
                Combinators::unfiltered(),
            );
        }
    }
    let graph = builder.finish();
    Workspace {
        strings,
        graph,
        consumers,
    }
}

/// The probe kinds, resolved per consumer against the fixture's naming
/// scheme: index 0 local hit, 1 imported hit, 2 prefixed hit, 3 miss, and
/// 4 (every 8th consumer only) the ambiguous name.
fn probe_ids(strings: &StringTable, i: usize) -> Vec<StringId> {
    let mut ids = vec![
        strings.intern(&format!("local_{i}_0")),
        strings.intern("core_0"),
        strings.intern(&format!("pool{}_0", (i * 7 + 1) % POOL_LIBS)),
        strings.intern("absent"),
    ];
    if i.is_multiple_of(AMBIG_EVERY) {
        ids.push(strings.intern(&format!("clash_{i}")));
    }
    ids
}

fn run_scale(libs: usize, probes: usize) {
    println!("== scale {libs} libraries ==");
    let t = Instant::now();
    let ws = build_workspace(libs);
    let graph_build = t.elapsed();
    let t = Instant::now();
    let namespaces = Namespaces::build(&ws.graph);
    let ns_build = t.elapsed();
    println!(
        "construct: graph {:?} ({} libs), namespaces {:?} — csr_bytes={} interner_entries={}",
        graph_build,
        ws.graph.len(),
        ns_build,
        namespaces.memory_bytes(),
        ws.strings.len(),
    );

    let strings = &ws.strings;
    // Resolve the probe name ids up front: the measured loop must pay no
    // string work, exactly like the engine's lookups.
    let per_lib: Vec<Vec<StringId>> = (0..libs).map(|i| probe_ids(strings, i)).collect();
    let kinds = per_lib[0].len();
    let prefix = strings.intern("p");

    // Hot-probe latency: cycle consumers and kinds, one timed lookup each.
    let mut durations = Vec::with_capacity(probes);
    for n in 0..probes {
        let i = n % libs;
        let kind = n % kinds;
        let lib = ws.consumers[i];
        let name = per_lib[i][kind];
        let t = Instant::now();
        let hit = if kind == 2 {
            namespaces.lookup_prefixed(&ws.graph, lib, prefix, name)
        } else {
            namespaces.lookup(&ws.graph, lib, name)
        };
        std::hint::black_box(&hit);
        durations.push(t.elapsed().as_nanos());
    }
    let (p50, p95, p99) = percentiles(&durations);
    println!("probe latency ns: p50={p50} p95={p95} p99={p99} n={probes}");

    // Full sweep: every library probed once per kind, wall clock.
    let t = Instant::now();
    let mut hits = 0u64;
    for (i, ids) in per_lib.iter().enumerate() {
        let lib = ws.consumers[i];
        for (kind, &name) in ids.iter().enumerate() {
            let hit = if kind == 2 {
                namespaces.lookup_prefixed(&ws.graph, lib, prefix, name)
            } else {
                namespaces.lookup(&ws.graph, lib, name)
            };
            if !matches!(hit, Lookup::NotFound) {
                hits += 1;
            }
        }
    }
    let sweep = t.elapsed();
    println!(
        "sweep: {:?} total, {} ns/probe mean, {hits} hits",
        sweep,
        sweep.as_nanos() / (libs as u128 * kinds as u128),
    );

    // Legacy adjudication: materialize the predecessor shape at the 1k
    // scale only — at 10^5 it IS the memory bomb (entry counts scale
    // linearly with libraries; the printed extrapolation carries the 10^5
    // figure without holding the maps).
    if libs <= 1_000 {
        let t = Instant::now();
        let legacy = legacy::build(&ws.graph);
        let legacy_build = t.elapsed();
        let (bare, prefixed) = legacy.count_entries();
        // Counted cost model: per FxHashMap entry, a 4-byte interned key
        // plus the 24-byte stored value (an enum over the 8-byte target
        // with tag and padding), with 2x capacity headroom (load factor and
        // raw-table rounding); plus ~64 bytes of table base per map.
        let maps = libs * 2; // the bare map plus at least one prefix table per library
        let est_bytes = (bare + prefixed) * 28 * 2 + maps * 64;
        let mut legacy_durations = Vec::with_capacity(probes);
        for n in 0..probes {
            let i = n % libs;
            let kind = n % kinds;
            let lib = ws.consumers[i];
            let name = per_lib[i][kind];
            let t = Instant::now();
            // The predecessor chain: graph locals first (both shapes pay
            // the same call), then the merged maps.
            let hit = if kind == 0 && ws.graph.local(lib, name) != LocalLookup::Absent {
                true
            } else {
                legacy.lookup(lib, (kind == 2).then_some(prefix), name)
            };
            std::hint::black_box(&hit);
            legacy_durations.push(t.elapsed().as_nanos());
        }
        let (lp50, _, _) = percentiles(&legacy_durations);
        println!(
            "legacy shape: build {:?}, bare_entries={bare} prefix_entries={prefixed} est_bytes={est_bytes} (model: entries*(4+24)*2 + {maps} maps*64) — extrapolated to 10^5 libraries: ~{} GB",
            legacy_build,
            est_bytes as u128 * 100_000 / (libs as u128 * 1_000_000_000),
        );
        println!("lookup latency (legacy p50 / combinator p50): {lp50} / {p50} ns",);
    }
}

fn percentiles(durations: &[u128]) -> (u128, u128, u128) {
    let mut sorted = durations.to_vec();
    sorted.sort_unstable();
    let at = |q: usize| sorted[sorted.len() * q / 100];
    (at(50), at(95), at(99))
}

/// The predecessor shape (#1068): per-library eagerly merged namespace maps,
/// reimplemented here only so the adjudication measures it. Structure
/// mirrors the old `LibraryNamespaces`/`MergeTable` pair — the bare-name
/// map, the per-prefix merged tables, and the lazy ambiguous candidate sets.
mod legacy {
    use std::collections::hash_map;

    use dartq_resolution::{LibraryGraph, LibraryId, NamespaceEntry};
    use dartq_source::StringId;
    use rustc_hash::FxHashMap;

    /// The stored entry: one target or the full candidate set.
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Entry {
        Single(NamespaceEntry),
        Ambiguous(Vec<NamespaceEntry>),
    }

    pub struct LegacyNamespaces {
        libs: Vec<Lib>,
    }

    struct Lib {
        names: FxHashMap<StringId, Entry>,
        prefixes: FxHashMap<StringId, FxHashMap<StringId, Entry>>,
    }

    pub fn build(graph: &LibraryGraph) -> LegacyNamespaces {
        let libs = graph
            .libraries()
            .map(|lib| {
                let mut names: FxHashMap<StringId, Entry> = FxHashMap::default();
                let mut prefixes: FxHashMap<StringId, FxHashMap<StringId, Entry>> =
                    FxHashMap::default();
                for edge in graph.import_edges(lib) {
                    let mut table = edge
                        .prefix
                        .map(|prefix| prefixes.entry(prefix).or_default());
                    let Some(target) = edge.target else {
                        continue;
                    };
                    for (name, entry) in graph.export_scope(target) {
                        if !edge.combinators.allows(name) {
                            continue;
                        }
                        let target_entry = NamespaceEntry::Decl(entry.file);
                        let slot = match table.as_mut() {
                            Some(table) => table,
                            None => &mut names,
                        };
                        // First-come-wins merge; distinct targets accumulate
                        // into the ambiguous candidate set (Dart 18.1).
                        match slot.entry(name) {
                            hash_map::Entry::Vacant(v) => {
                                v.insert(Entry::Single(target_entry));
                            }
                            hash_map::Entry::Occupied(mut occupied) => {
                                match occupied.get().clone() {
                                    Entry::Single(existing) if existing != target_entry => {
                                        occupied
                                            .insert(Entry::Ambiguous(vec![existing, target_entry]));
                                    }
                                    Entry::Ambiguous(mut candidates) => {
                                        if !candidates.contains(&target_entry) {
                                            candidates.push(target_entry);
                                        }
                                        occupied.insert(Entry::Ambiguous(candidates));
                                    }
                                    Entry::Single(_) => {}
                                }
                            }
                        }
                    }
                }
                Lib { names, prefixes }
            })
            .collect();
        LegacyNamespaces { libs }
    }

    impl LegacyNamespaces {
        /// Map-hit face for the latency comparison: `true` when the merged
        /// table holds the name.
        pub fn lookup(&self, lib: LibraryId, prefix: Option<StringId>, name: StringId) -> bool {
            let lib = &self.libs[lib.index() as usize];
            match prefix {
                Some(prefix) => lib
                    .prefixes
                    .get(&prefix)
                    .is_some_and(|table| table.contains_key(&name)),
                None => lib.names.contains_key(&name),
            }
        }

        /// The counted memory metric of the legacy shape: total stored
        /// entries across all bare-name and prefix maps.
        pub fn count_entries(&self) -> (usize, usize) {
            let bare: usize = self.libs.iter().map(|lib| lib.names.len()).sum();
            let prefixed: usize = self
                .libs
                .iter()
                .map(|lib| lib.prefixes.values().map(FxHashMap::len).sum::<usize>())
                .sum();
            (bare, prefixed)
        }
    }
}
