//! Postings benchmark for the `query symbol` faces (execution
//! deferred to the serial phase): a synthetic workspace of N single-file
//! libraries where every file declares a shared name and uses a fixed
//! anchor file's symbols, so both faces carry point, hot (many-hits), and
//! miss query classes. Per class the postings face and the workspace-scan
//! face it replaced are timed side by side over the same index state (the
//! scan rebuilt over the public accessors — the exact `BTreeMap` walks the
//! pre-postings faces ran; the runs double as an equivalence check), plus
//! the index build split into capture / graph / library-surface /
//! declaration-insertion walls, per-file replacement walls (the
//! incremental path), and the postings-byte total.
//! Scale: `DARTQ_BENCH_FILES` (default 2000), `DARTQ_BENCH_QUERIES`
//! (default 200).

use std::env;
use std::hint::black_box;
use std::path::Path;
use std::time::{Duration, Instant};

use dartq_index::{LibraryBuilder, SymbolIndex, capture};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::{FileId, FileTable, StringId, StringTable};

/// The workspace's synthetic file `i`: a class with a member and a field,
/// a unique top-level, the workspace-wide shared name, and (past the
/// anchor) uses of the anchor file's top-level and class plus the local
/// shared name.
fn source(i: usize, anchored: bool) -> String {
    let imports = if anchored {
        "import 'f0.dart';\n\n"
    } else {
        ""
    };
    let uses = if anchored {
        format!("void use{i}() {{\n  top0();\n  C0();\n  shared();\n}}\n")
    } else {
        format!("void use{i}() {{\n  shared();\n}}\n")
    };
    format!(
        "{imports}class C{i} {{\n  void m{i}() {{}}\n  int f{i} = 0;\n}}\nvoid top{i}() {{}}\nvoid shared() {{}}\n\n{uses}"
    )
}

/// The re-indexed variant of file `i`: one member renamed (its declaration
/// listing moves) and one top-level added — resolution untouched.
fn source_v2(i: usize, anchored: bool) -> String {
    source(i, anchored).replace(&format!("void m{i}() {{}}"), &format!("void n{i}() {{}}"))
        + &format!("void extra{i}() {{}}\n")
}

/// Microseconds at quantile `q` of an ascending-sorted wall sample
/// (nearest rank).
fn us_at(sorted: &[Duration], q: f64) -> f64 {
    let at = ((q * sorted.len() as f64) as usize).clamp(1, sorted.len());
    sorted[at - 1].as_secs_f64() * 1e6
}

/// Times one face over a per-query name list; matches counted inside the
/// walls (the answer walk is the cost being measured).
fn bench_symbols(index: &SymbolIndex, names: &[StringId], scan: bool) -> (Vec<Duration>, usize) {
    let mut walls = Vec::with_capacity(names.len());
    let mut matches = 0usize;
    for &name in names {
        let started = Instant::now();
        matches = if scan {
            index
                .files()
                .map(|(_, surface)| surface.by_name(name).map_or(0, |rows| rows.len()))
                .sum::<usize>()
        } else {
            index.symbols(name).count()
        };
        walls.push(started.elapsed());
    }
    black_box(matches);
    (walls, matches)
}

/// The reference faces' twin of [`bench_symbols`]: per-query
/// (target file, name) probes.
fn bench_references(
    index: &SymbolIndex,
    targets: &[(FileId, StringId)],
    scan: bool,
) -> (Vec<Duration>, usize) {
    let mut walls = Vec::with_capacity(targets.len());
    let mut matches = 0usize;
    for &(file, name) in targets {
        let started = Instant::now();
        matches = if scan {
            index
                .libraries()
                .filter_map(|(_, surface)| surface.references_of(file, name))
                .map(|rows| rows.len())
                .sum::<usize>()
        } else {
            index.references(file, name).count()
        };
        walls.push(started.elapsed());
    }
    black_box(matches);
    (walls, matches)
}

/// One JSON line per query class: both faces' p50/p95 side by side.
fn report(
    face: &str,
    class: &str,
    queries: usize,
    mut postings: Vec<Duration>,
    mut scan: Vec<Duration>,
    matches: usize,
) {
    postings.sort();
    scan.sort();
    println!(
        "{{\"bench\":\"query_{face}\",\"class\":\"{class}\",\"queries\":{queries},\"postings_p50_us\":{:.1},\"postings_p95_us\":{:.1},\"scan_p50_us\":{:.1},\"scan_p95_us\":{:.1},\"matches\":{matches}}}",
        us_at(&postings, 0.50),
        us_at(&postings, 0.95),
        us_at(&scan, 0.50),
        us_at(&scan, 0.95),
    );
}

fn main() {
    let mut files: usize = env::var("DARTQ_BENCH_FILES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000);
    files = files.max(2);
    let queries: usize = env::var("DARTQ_BENCH_QUERIES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);

    let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
    let mut table = FileTable::new();
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let config = ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let scan_and_capture = |table: &mut FileTable, path: String, text: &str| {
        let file = table.intern(Path::new(&path));
        let scanned = dartq_scanner::scan(text, config, file);
        (
            file,
            capture(file, text, &scanned.tokens, features, strings),
        )
    };

    // Capture: scan + parse every file once (the cost any index build
    // sits on top of).
    let started = Instant::now();
    let mut file_ids = Vec::with_capacity(files);
    let mut captures = Vec::with_capacity(files);
    for i in 0..files {
        let text = source(i, i > 0);
        let (file, captured) = scan_and_capture(&mut table, format!("f{i}.dart"), &text);
        captures.push((file, captured));
        file_ids.push(file);
    }
    let capture_s = started.elapsed();

    // Graph and namespaces: the resolution input the library surfaces read.
    let started = Instant::now();
    let mut builder = LibraryGraphBuilder::new(strings);
    let libs: Vec<_> = (0..files)
        .map(|i| builder.intern(&format!("package:b/f{i}.dart")))
        .collect();
    for (i, (_, captured)) in captures.iter().enumerate() {
        for row in captured
            .index
            .decls()
            .iter()
            .filter(|row| row.is_library_face())
        {
            builder.declare(libs[i], row.name, file_ids[i]);
        }
    }
    for lib in libs.iter().skip(1) {
        builder.import(
            *lib,
            ImportEdge {
                target: Some(libs[0]),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let graph_s = started.elapsed();

    // Library surfaces (reference postings build) — captures borrowed here,
    // moved into the declaration inserts after.
    let started = Instant::now();
    let mut index = SymbolIndex::default();
    let mut ref_rows = 0usize;
    for (i, (_, captured)) in captures.iter().enumerate() {
        let mut builder = LibraryBuilder::new(libs[i], &graph, &namespaces);
        builder.add_file(captured);
        let surface = builder.finish();
        ref_rows += surface.reference_count();
        index.insert_library(libs[i], surface);
    }
    let libraries_s = started.elapsed();

    // Declaration postings build — captures consumed.
    let mut decl_rows = 0usize;
    let started = Instant::now();
    for (file, captured) in captures {
        decl_rows += captured.index.len();
        index.insert_file(file, captured.index);
    }
    let declarations_s = started.elapsed();
    println!(
        "{{\"bench\":\"postings_build\",\"files\":{files},\"capture_ms\":{:.1},\"graph_ms\":{:.1},\"libraries_ms\":{:.1},\"declarations_ms\":{:.1},\"decl_rows\":{decl_rows},\"ref_rows\":{ref_rows}}}",
        capture_s.as_secs_f64() * 1e3,
        graph_s.as_secs_f64() * 1e3,
        libraries_s.as_secs_f64() * 1e3,
        declarations_s.as_secs_f64() * 1e3,
    );

    // Query classes. Point rotates unique names (one declaring file / one
    // referencing library), hot hammers names every file declares or
    // targets (the anchor's top0: N-1 referencing libraries), miss probes
    // an absent one. Both faces answer identical probes; the runs
    // double as the equivalence check.
    let point_names: Vec<StringId> = (0..queries)
        .map(|q| strings.intern(&format!("top{}", 1 + q % (files - 1))))
        .collect();
    let hot_names = vec![strings.intern("shared"); queries];
    let miss_names = vec![strings.intern("zz_absent"); queries];
    let point_refs: Vec<(FileId, StringId)> = (0..queries)
        .map(|q| (file_ids[q % files], strings.intern("shared")))
        .collect();
    let hot_refs = vec![(file_ids[0], strings.intern("top0")); queries];
    let miss_refs = vec![(file_ids[0], strings.intern("zz_absent")); queries];

    for (class, names) in [
        ("point", point_names),
        ("hot", hot_names),
        ("miss", miss_names),
    ] {
        let (postings, matches) = bench_symbols(&index, &names, false);
        let (scan, scan_matches) = bench_symbols(&index, &names, true);
        assert_eq!(matches, scan_matches, "{class}: faces disagree");
        report("symbols", class, queries, postings, scan, matches);
    }
    for (class, targets) in [
        ("point", point_refs),
        ("hot", hot_refs),
        ("miss", miss_refs),
    ] {
        let (postings, matches) = bench_references(&index, &targets, false);
        let (scan, scan_matches) = bench_references(&index, &targets, true);
        assert_eq!(matches, scan_matches, "{class}: faces disagree");
        report("references", class, queries, postings, scan, matches);
    }

    // Incremental: replace M files whole (capture pre-paid) — the
    // declaration-directory update — then their library surfaces, then a
    // full restore cycle per file (remove + reinsert both faces).
    let m = files.min(50);
    let mut v2 = Vec::with_capacity(m);
    for i in 0..m {
        let text = source_v2(i, i > 0);
        let (file, captured) = scan_and_capture(&mut table, format!("f{i}.dart"), &text);
        v2.push((file, captured));
    }
    let mut v2_surfaces: Vec<dartq_index::LibraryIndex> = Vec::with_capacity(m);
    for (i, (_, captured)) in v2.iter().enumerate() {
        let mut builder = LibraryBuilder::new(libs[i], &graph, &namespaces);
        builder.add_file(captured);
        v2_surfaces.push(builder.finish());
    }
    let mut replace_file = Vec::with_capacity(m);
    for (file, captured) in v2 {
        let started = Instant::now();
        index.insert_file(file, captured.index);
        replace_file.push(started.elapsed());
    }
    let mut replace_library = Vec::with_capacity(m);
    for (i, surface) in v2_surfaces.into_iter().enumerate() {
        let started = Instant::now();
        index.insert_library(libs[i], surface);
        replace_library.push(started.elapsed());
    }

    let mut v1 = Vec::with_capacity(m);
    for i in 0..m {
        let text = source(i, i > 0);
        let (file, captured) = scan_and_capture(&mut table, format!("f{i}.dart"), &text);
        v1.push((file, captured));
    }
    let mut v1_surfaces: Vec<dartq_index::LibraryIndex> = Vec::with_capacity(m);
    for (i, (_, captured)) in v1.iter().enumerate() {
        let mut builder = LibraryBuilder::new(libs[i], &graph, &namespaces);
        builder.add_file(captured);
        v1_surfaces.push(builder.finish());
    }
    let mut restore = Vec::with_capacity(m);
    for (i, ((file, captured), surface)) in v1.into_iter().zip(v1_surfaces).enumerate() {
        let started = Instant::now();
        index.remove_file(file);
        index.insert_file(file, captured.index);
        index.remove_library(libs[i]);
        index.insert_library(libs[i], surface);
        restore.push(started.elapsed());
    }
    replace_file.sort();
    replace_library.sort();
    restore.sort();
    println!(
        "{{\"bench\":\"postings_incremental\",\"files\":{m},\"replace_file_p50_us\":{:.1},\"replace_file_p95_us\":{:.1},\"replace_library_p50_us\":{:.1},\"replace_library_p95_us\":{:.1},\"restore_cycle_p50_us\":{:.1}}}",
        us_at(&replace_file, 0.50),
        us_at(&replace_file, 0.95),
        us_at(&replace_library, 0.50),
        us_at(&replace_library, 0.95),
        us_at(&restore, 0.50),
    );

    // Memory: the postings directories' bytes over the rows they index —
    // the per-entry figure the memory axis adjudicates (rows live once, in
    // the per-unit surfaces; the scan baseline carried no directory).
    let mut live_decls = 0usize;
    let mut live_refs = 0usize;
    for (_, surface) in index.files() {
        live_decls += surface.len();
    }
    for (_, surface) in index.libraries() {
        live_refs += surface.reference_count();
    }
    let total_bytes = index.postings_bytes();
    println!(
        "{{\"bench\":\"postings_memory\",\"files\":{files},\"decl_rows\":{live_decls},\"ref_rows\":{live_refs},\"total_bytes\":{total_bytes},\"bytes_per_row\":{:.2}}}",
        total_bytes as f64 / (live_decls + live_refs).max(1) as f64,
    );
    println!(
        "// acceptance axis: #1069 — per-query cost is match-sized (postings) vs workspace-sized (scan); build/incremental walls and total_bytes adjudicate the memory-for-latency trade"
    );
}
