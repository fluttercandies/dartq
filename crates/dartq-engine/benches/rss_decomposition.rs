//! RSS decomposition probe (the multi-instance memory ADR's T-a ticket,
//! execution deferred to the serial phase like every bench here): one cold
//! analysis pass over a synthetic corpus, then a per-segment accounting of
//! where the residency bytes live. The segment map is the probe's own
//! residency-accounting table; every line below is machine-greppable so the
//! serial run can archive it as evidence and gate the P1 slimming moves on it.
//!
//! Observability contract: a segment line's status is exactly one of
//!
//! * **measured** — an exact byte total read off a public counter
//!   ([`dartq_engine::Engine::stored_bytes`] for S3);
//! * **estimated** — a byte total computed from public per-file counters
//!   (map/row lens) times per-entry constants documented below from the
//!   struct layouts (`size_of` at runtime, no hardcoded struct sizes);
//! * **unpinned** — the segment's owner state is not publicly countable,
//!   so the planned byte band is printed verbatim with `unpinned`; each such
//!   line names the engine-internal hook a later lane would need.
//!
//! What is pinned today: S1 (facts minus diagnostics rows, counted per
//! resident file), S2 (capture decl rows + supers + library ref rows +
//! the raw capture sites at reserved capacity via
//! [`dartq_index::FileCapture::sites_capacity`], plus the postings
//! directories' exact bytes via [`dartq_engine::Engine::postings_bytes`] —
//! the S2b line), S3 (exact), S4 (the single-copy face: residency rows
//! counted once with their text bytes; since #1501 the per-library and
//! semdiag stores hold refcount handles, ~8 B per row, and since #1510 the
//! merged output's flat copy shares the text by refcount too — it adds the
//! row struct only — so this probe's S4 total needs no further copy-factor
//! correction), S5 (intern count × the measured ~93 B/entry constant), S6
//! (exact since #1779 — the per-target adjacency indexes' row-level CSR
//! layout counts every owned byte). S7 (facts cache + scanned faces + warm
//! staging) is measured at RSS level: the shed delta between the
//! post-analyze and post-shed readings.
//!
//! Transient split: `peak_rss_kib` is the process `ru_maxrss` at the end
//! of the cold pass — it includes the cold-apply fan-out. The split lines
//! sample the *current* RSS (`current_rss_kib`, mach `task_info` on macOS,
//! `VmRSS` on Linux) after the explicit apply window
//! (`rss_after_apply_kib`), after the analyze
//! (`rss_after_analyze_kib`), and after a budget-ladder shed of every
//! recomputable cache (`steady_after_shed_kib`): peak minus steady is the
//! measured transient (`transient_kib`) the ADR's P1-8 chunking gates on,
//! replacing the bracket estimate.
//!
//! Cost axis: measurement only — no walls are reported and nothing here is
//! comparable to the timing benches' axes.

mod common;

use std::fmt::Write as _;

use common::Fixture;
use dartq_diagnostics::{Diagnostic, SharedStr};
use dartq_engine::CancellationToken;
use dartq_engine::state::DirectiveFacts;
use dartq_index::{DeclRow, RefRow, SuperEdge};
use dartq_semantic::{Container, MemberEntry, TopDecl};
use dartq_source::StringId;

/// Malloc chunk header + size-class rounding per heap allocation (the
/// glibc/Apple allocator midpoint; the same constant for every Vec and map
/// below keeps the model one paragraph).
const ALLOC_OVERHEAD: usize = 48;
/// Amortized Vec growth midpoint: push-grown buffers land between len and
/// 2× len in capacity; 3/2 is the model's documented midpoint.
const VEC_SLACK: (usize, usize) = (3, 2);
/// Hashbrown-style load-factor bound: capacity holds at 87.5% occupancy,
/// plus one control byte per slot.
const MAP_SLACK: (usize, usize) = (8, 7);
/// The measured per-entry footprint of the leaked-`&'static str` intern
/// table (the string-table memory test's figure, the ADR's S5 constant).
const STRING_TABLE_BYTES_PER_ENTRY: usize = 93;
/// [`dartq_source::LineIndex`]'s per-line cost: one `u32` line start; the
/// wide-line records stay empty for the ASCII-only corpus below.
const LINE_INDEX_BYTES_PER_LINE: usize = 4;

fn vec_bytes(items: usize, item_size: usize) -> usize {
    items * item_size * VEC_SLACK.0 / VEC_SLACK.1 + ALLOC_OVERHEAD
}

fn map_bytes(entries: usize, key: usize, value: usize) -> usize {
    entries * (key + value + 1) * MAP_SLACK.0 / MAP_SLACK.1 + ALLOC_OVERHEAD
}

#[expect(
    clippy::cast_precision_loss,
    reason = "a decomposition table reports MiB, not exact bytes"
)]
fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// Process peak RSS (the highest `ru_maxrss` the kernel recorded): macOS
/// reports bytes, Linux reports KiB — normalized to KiB.
fn peak_rss_kib() -> u64 {
    // Safety: `getrusage` with a valid out-pointer is infallible C.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    // Safety: same — the pointer is a live `rusage` of the right size.
    unsafe {
        libc::getrusage(libc::RUSAGE_SELF, &mut usage);
    }
    #[cfg(target_os = "macos")]
    {
        // macOS reports bytes in a `c_long`.
        u64::try_from(usage.ru_maxrss).unwrap_or(0) / 1024
    }
    #[cfg(not(target_os = "macos"))]
    {
        u64::try_from(usage.ru_maxrss).unwrap_or(0)
    }
}

/// The process's *current* resident set — `ru_maxrss` is monotone, so the
/// steady/transient split needs this to sample the post-shed floor. macOS
/// reads `task_info(MACH_TASK_BASIC_INFO)`; Linux reads `/proc/self/status`'s
/// `VmRSS` (kB, page-size-free). `0` when the platform exposes neither —
/// the split lines then read as unavailable.
fn current_rss_kib() -> u64 {
    #[cfg(target_os = "macos")]
    {
        // Safety: `task_info` with a zeroed out-struct and its flavor's
        // count is the documented Mach read; the kernel writes at most the
        // `mach_task_basic_info` payload.
        let mut info: libc::mach_task_basic_info = unsafe { std::mem::zeroed() };
        let mut count = libc::MACH_TASK_BASIC_INFO_COUNT;
        // Safety: `mach_task_self_` is the static task port; the call only
        // reads it. The static is deprecated in favor of the `mach2` crate,
        // a dependency this bench does not need — libc's own spelling is
        // the zero-cost form here.
        #[expect(
            deprecated,
            reason = "libc's deprecated static is the dependency-free spelling; \
                      mach2 adds a crate for one port read"
        )]
        let task = unsafe { libc::mach_task_self_ };
        let status = unsafe {
            libc::task_info(
                task,
                libc::MACH_TASK_BASIC_INFO,
                std::ptr::from_mut(&mut info).cast::<libc::integer_t>(),
                &mut count,
            )
        };
        if status == libc::KERN_SUCCESS {
            u64::try_from(info.resident_size).map_or(0, |bytes| bytes / 1024)
        } else {
            0
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|status| {
                status.lines().find_map(|line| {
                    let value = line.strip_prefix("VmRSS:")?;
                    let kB = value.trim().strip_suffix("kB")?;
                    kB.trim().parse::<u64>().ok()
                })
            })
            .unwrap_or(0)
    }
}

/// One self-contained library per file (the residency corpus shape): a few
/// following-sibling imports, a class with a field/getter/method, and a
/// top-level function. ASCII-only by construction, so the line-index model
/// above is exact.
fn corpus(files: usize) -> Vec<(String, String)> {
    (0..files)
        .map(|i| {
            let mut body = String::new();
            let deps = 2 + i % 3;
            for t in i + 1..=((i + deps).min(files - 1)) {
                let _ = writeln!(body, "import 'f{t:05}.dart';");
            }
            let _ = write!(
                body,
                "\nclass C{i:05} {{\n  final int field;\n  C{i:05}(this.field);\n  int get doubled => field * 2;\n  int m{i:05}() => field + {i};\n}}\n\nint g{i:05}(int x) => x + {i};\n"
            );
            (format!("lib/f{i:05}.dart"), body)
        })
        .collect()
}

/// One diagnostic row's footprint at the face that owns its text: the
/// inline struct plus the heap faces the row owns — the refcount-shared
/// ([`SharedStr::Shared`]) message buffer and a correction string. Bare
/// templates ride the static side for free. Downstream copies (per-library
/// results, semdiag folds, the merged output) hold the same text by
/// refcount, so they cost only the struct — see [`row_bytes`].
fn diagnostic_bytes(diagnostic: &Diagnostic) -> u64 {
    let message_heap = match &diagnostic.message {
        SharedStr::Shared(text) => vec_bytes(text.len(), 1),
        SharedStr::Static(_) => 0,
    };
    let correction_heap = diagnostic
        .correction
        .as_ref()
        .map_or(0, |text| vec_bytes(text.len(), 1));
    (std::mem::size_of::<Diagnostic>() + message_heap + correction_heap) as u64
}

/// A row copy that shares its text by refcount (the #1510 shape): the
/// struct's handles only — no message or correction bytes.
fn row_bytes() -> u64 {
    std::mem::size_of::<Diagnostic>() as u64
}

fn main() {
    let files = std::env::var("DARTQ_BENCH_FILES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10_000);
    let libraries_src = corpus(files);
    let refs: Vec<(&str, &str)> = libraries_src
        .iter()
        .map(|(path, body)| (path.as_str(), body.as_str()))
        .collect();
    let fixture = Fixture::new(&refs);

    // The cold pass whose residency the table decomposes. The apply window
    // is sampled separately from the analyze: an explicit `apply_pending`
    // drains the overlay here, so the analyze's own phase-1 apply sees an
    // empty queue and emits nothing — the same state transitions, two
    // observable windows. Peak RSS is read immediately after the analyze —
    // before any on-demand projection (the CSR graphs below) can allocate
    // on top.
    let mut engine = fixture.engine();
    let stats = engine.discover_workspace().expect("discovery");
    engine.apply_pending(&mut |_| {});
    let rss_after_apply_kib = current_rss_kib();
    let merged = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("cold analyze");
    let peak_kib = peak_rss_kib();
    let rss_after_analyze_kib = current_rss_kib();
    let libraries = engine.libraries();

    // S1 + the S4 residency face: per resident file, the publicly countable
    // facts rows and their element-model maps.
    let decl_key = std::mem::size_of::<StringId>();
    let top_decl = std::mem::size_of::<TopDecl>();
    let container = std::mem::size_of::<Container>();
    let member_entry = std::mem::size_of::<MemberEntry>();
    let directive = std::mem::size_of::<DirectiveFacts>();
    let (mut facts_bytes, mut diag_rows, mut diag_bytes) = (0u64, 0u64, 0u64);
    let mut resident_files = 0usize;
    for (_, members) in &libraries {
        for file in members {
            let Some(facts) = engine.file_facts(*file) else {
                continue;
            };
            resident_files += 1;
            let elements = facts.elements.as_ref();
            let container_names = elements.container_order();
            let top_decls = elements.declarations().count();
            let mut member_entries = 0usize;
            for name in container_names {
                if let Some(decl) = elements.container(*name) {
                    member_entries += decl.members.len();
                }
            }
            for face in elements.extension_faces() {
                member_entries += face.members.len();
            }
            let elements_bytes = map_bytes(top_decls, decl_key, top_decl)
                + map_bytes(container_names.len(), decl_key, container)
                + map_bytes(member_entries, decl_key, member_entry);
            let directives_bytes = vec_bytes(facts.directives.len(), directive);
            let lines_bytes = vec_bytes(
                facts.line_index.line_count() as usize,
                LINE_INDEX_BYTES_PER_LINE,
            );
            facts_bytes += (elements_bytes + directives_bytes + lines_bytes) as u64;

            // The S4 residency face, counted once here with each row's text
            // bytes (the merged-output copy below adds the struct only —
            // since #1501 the per-library and semdiag stores hold refcount
            // handles, ~8 B/row, and since #1510 its flat copy shares the
            // text by refcount too, so these two faces are the byte total).
            for diagnostic in &facts.diagnostics {
                diag_rows += 1;
                diag_bytes += diagnostic_bytes(diagnostic);
            }
        }
    }

    // S2: capture decl rows + supertype edges + library reference rows +
    // the raw capture sites at reserved capacity (the finish-time shrink
    // leaves `sites_capacity() == sites.len()`, so `vec_bytes`'s 3/2 slack
    // model would overstate; the capacity face reports the held bytes
    // exactly). The engine's postings directories are crate-internal and
    // uncountable from here.
    let decl_row = std::mem::size_of::<DeclRow>();
    let super_edge = std::mem::size_of::<SuperEdge>();
    let ref_row = std::mem::size_of::<RefRow>();
    // `RawSites` (dartq-index capture.rs): `StringId` + kind + family +
    // offset + length per row, counted at reserved capacity.
    let raw_site_row = std::mem::size_of::<StringId>() + 2 + 2 * std::mem::size_of::<u32>();
    let mut captures_bytes = 0u64;
    for (_, members) in &libraries {
        for file in members {
            if let Some(capture) = engine.file_capture(*file) {
                captures_bytes += vec_bytes(capture.index.decls().len(), decl_row) as u64;
                captures_bytes += vec_bytes(capture.index.supers().len(), super_edge) as u64;
                captures_bytes +=
                    (capture.sites_capacity() * raw_site_row + ALLOC_OVERHEAD * 5) as u64;
            }
        }
    }
    for (_, surface) in engine.library_surfaces() {
        captures_bytes += vec_bytes(surface.reference_count(), ref_row) as u64;
    }

    // S3: the store's own byte ledger — exact.
    let source_bytes = engine.stored_bytes();

    // S4's merged-output face: the analyze return holds its own copy of the
    // row structs; the text bytes behind them are shared with the residency
    // face (#1510), so only the struct rides here.
    for _diagnostic in &merged {
        diag_rows += 1;
        diag_bytes += row_bytes();
    }

    // S5: the intern table's entry count × the measured per-entry constant.
    let string_entries = engine.strings().len();
    let strings_bytes = (string_entries * STRING_TABLE_BYTES_PER_ENTRY) as u64;

    // S6: the maintained per-target adjacency indexes (importers,
    // part_offers, import_sources) — exact since the #1779 row-level CSR
    // layout: slot arena at reserved capacity plus each live row's boxed
    // payload, all counted by `Engine::graph_maps_bytes`.
    let graph_maps_bytes = engine.graph_maps_bytes();

    // S7 proxies (counts only — the cache tiers' byte faces stay
    // engine-internal): live libraries, total member files, and the export
    // CSR projections' edge counts stand in for orientation.
    let member_files: usize = libraries.iter().map(|(_, members)| members.len()).sum();
    let csr_edges =
        engine.library_import_graph().len_edges() + engine.file_reference_graph().len_edges();

    // S2's postings half — exact since the probe hook
    // (`Engine::postings_bytes`, the two directories' hash slots at
    // capacity plus their unit lists' reserved bytes). Read beside the
    // other segments: the shed leg below sweeps the store, so every
    // segment's input must come off the pre-shed world.
    let postings_bytes = engine.postings_bytes();
    let stored_contents = engine.stored_contents();

    // The steady/transient split (the audit's serial-window plan): a 1 MiB
    // budget makes every phase boundary of the next passes shed one ladder
    // tier — the facts cache, the scanned faces, and the warm-start staging
    // are all documented-recomputable, and a zero-dirty pass re-derives
    // nothing — so the post-shed reading is the residency floor the
    // transients sit above. Three passes burn the three tiers; the shed's
    // own `dartq/memory_degraded` rows ride the discarded returns.
    engine.set_memory_limit(Some(1));
    for _ in 0..3 {
        let _ = engine.analyze(&[], &mut |_| {}, &CancellationToken::default());
    }
    engine.set_memory_limit(None);
    let rss_after_shed_kib = current_rss_kib();

    let (s1, s2, s3, s4, s5, s6) = (
        facts_bytes,
        captures_bytes,
        source_bytes,
        diag_bytes,
        strings_bytes,
        graph_maps_bytes,
    );
    let total = s1 + s2 + s3 + s4 + s5 + s6 + postings_bytes;
    let face_tag = if files % 1000 == 0 {
        format!("{}k_files", files / 1000)
    } else {
        format!("{files}_files")
    };

    println!("S1 facts_residency={} MiB estimated", mib(s1));
    println!("S2 index_captures={} MiB estimated", mib(s2));
    println!("S2b postings_dirs={} MiB measured", mib(postings_bytes));
    println!("S3 source_bytes={} MiB measured", mib(s3));
    println!("S4 diagnostics={} MiB estimated", mib(s4));
    println!("S5 string_table={} MiB estimated", mib(s5));
    println!("S6 graph_maps={} MiB measured", mib(s6));
    println!(
        "S7 caches_shed_delta={} MiB measured",
        mib(rss_after_analyze_kib.saturating_sub(rss_after_shed_kib))
    );
    println!("total_segments={} MiB estimated", mib(total));
    println!("peak_rss_kib={peak_kib}");
    println!("rss_after_apply_kib={rss_after_apply_kib}");
    println!("rss_after_analyze_kib={rss_after_analyze_kib}");
    println!("steady_after_shed_kib={rss_after_shed_kib}");
    println!(
        "transient_kib={}",
        peak_kib.saturating_sub(rss_after_shed_kib)
    );
    println!(
        "corpus_face={face_tag} files={files} discovered={} loc={} resident_files={resident_files} diag_rows={diag_rows} merged_rows={} strings={string_entries} libraries={} member_files={member_files} csr_edges={csr_edges} stored_contents={}",
        stats.file_count,
        stats.loc_total,
        merged.len(),
        libraries.len(),
        stored_contents,
    );
    println!(
        "// acceptance axis: measurement only (the ADR T-a decomposition) — peak_rss_kib minus steady_after_shed_kib is the measured transient the P1-8 chunking gates on (the cold-apply window is split out as rss_after_apply_kib); S7's shed delta covers the facts cache + scanned faces + warm staging at RSS level — the per-entry enumeration (facts/scanned entries × face bytes) still needs the engine-internal occupancy hook on state.rs's private caches; S4's per-library/semdiag stores hold refcount handles since #1501 (no row-copy factor left to hook); S6 graph maps measure exactly since #1779; S2b postings exact via Engine::postings_bytes"
    );
}
