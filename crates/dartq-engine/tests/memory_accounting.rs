//! Per-structure memory accounting pins (goal #778): kilobytes of residency
//! per kilobyte of source for the structures the engine keeps resident per
//! file and per library.
//! The R1 residency audit's widest error bars were code-derived estimates
//! (`FileCapture` ~0.43x source, `LibraryIndex` ~6.5 KB/file) flagged as the
//! least trustworthy precisely because nothing measured them; this test
//! replaces the estimate with a measurement.
//!
//! One synthetic ~10 KB Dart file with realistic declaration and name
//! density drives the real production path — `Engine::set_content` →
//! `Engine::analyze` — and each resident structure is measured through
//! public accessors only: `std::mem::size_of` for the inline header plus
//! the production `heap_bytes` accounting for the owned heap (the
//! `dartq-ast/tests/metrics.rs` / `string_table_memory` posture); the
//! hand-rolled composition remains only where no production face exists
//! (`LineIndex`, the content store).
//!
//! The crate-internal `RawSites` columns (dartq-index capture.rs: five
//! parallel `StringId` + kind + family + offset + length columns, 14 B per
//! site) are counted at reserved capacity via
//! [`FileCapture::sites_capacity`] — capacity is what the allocator holds,
//! and the finish-time shrink must leave `capacity == len`, so a
//! reservation leak (e.g. the token-count preallocation `RawSites` opens
//! with) trips the pin. The expected row count is derived exactly from the
//! library surface: every captured site folds into exactly one parser-level
//! `RefRow` or one built `NameRow` ([`dartq_index::LibraryIndex`] build),
//! and the refs slice additionally carries the semantic member-ref rows the
//! diagnose fold publishes beside the sites (#2618). Those semantic rows
//! are not extra sites — the site they point at already folded into the
//! names table — and their non-`NONE` `target_container` is the documented
//! discriminator (`RefRow::target_container`).
//!
//! The ceilings pin order of magnitude, not exact bytes: on this fixture
//! the measured full-structure numbers are ~2.1 KB/KB (`FileCapture`) and
//! ~3.0 KB/KB (`LibraryIndex`) against 5 KB/KB ceilings, and a row that
//! starts carrying duplicated text or per-row heap structures instead of
//! interned ids and flat columns trips them. The fail messages print the
//! measured numbers, so a trip is itself the evidence needed to
//! recalibrate.
//!
//! `FileElements` (dartq-semantic) is deliberately absent: that crate is
//! mid-flight on a parallel lane and this test must not touch it. Its pin
//! appends behind the same helpers — a `file_elements_bytes` estimator
//! over `Engine::file_facts(file)`, joining the same synthetic file and
//! the same per-KB normalization — once that lane lands.
//!
//! Run: `cargo test -p dartq-engine --test memory_accounting -- --nocapture`

#![allow(clippy::cast_possible_truncation)] // u32 line counts widen; measurement math only
#![allow(clippy::cast_precision_loss)] // measurement math only (byte counts are far below 2^52)

mod common;

use std::fmt::Write as _;
use std::mem::size_of;

use common::Fixture;
use dartq_engine::CancellationToken;
use dartq_index::{FileCapture, LibraryIndex};
use dartq_source::{ContentId, LineIndex, StringId};

/// Site columns of the crate-internal `RawSites` (dartq-index capture.rs):
/// `StringId` + kind byte + family byte + offset + length. The five Vec
/// headers are already inside `size_of::<FileCapture>()`; the column heap
/// enters the capture bytes through `FileCapture::heap_bytes` at reserved
/// capacity. `Builder::finish` shrinks the token-count reservation to the
/// row count, so the pinned slack is `sites_capacity() - sites_len` and
/// the capacity pin below asserts that slack is zero.
const RAW_SITE_ROW_BYTES: usize = size_of::<StringId>() + 2 + 2 * size_of::<u32>();

/// Full-capture ceiling: 5 KB of resident capture per KB of source. The
/// audit estimated ~0.43x from the visible rows alone; the honest
/// full-structure measurement — the inline header plus `heap_bytes` (rows
/// and site columns at reserved capacity) — is dominated by the row
/// columns (21364 B over 10343 B, 2.07 KB/KB on the first measured run),
/// and a row that starts carrying duplicated text or per-row heap
/// structures instead of interned ids and flat columns trips the ceiling.
const FILE_CAPTURE_KB_PER_KB_CEILING: f64 = 5.0;

/// Reference-surface ceiling: 5 KB per KB of source. The exact
/// `heap_bytes` accounting on this single-library fixture measures
/// 31240 B over 10343 B (3.02 KB/KB — most uses land in the names table; a
/// workspace with imports shifts them to slightly larger `RefRow`s, not
/// more bytes).
const LIBRARY_INDEX_KB_PER_KB_CEILING: f64 = 5.0;

/// Line-index ceiling: 0.75 KB per KB of source — 4 B per line start plus
/// the sparse wide-line records (zero here; the fixture is ASCII). Realistic
/// line lengths keep this near 0.2; the ceiling trips a per-line record
/// blowup (16 B/line or wide-char records on ASCII).
const LINE_INDEX_KB_PER_KB_CEILING: f64 = 0.75;

/// Per-row cost of the default `MemorySourceStore` above the bytes it
/// indexes: the `ContentId` key plus the Vec header (a row that starts
/// cloning text or carrying per-row metadata more than doubles this —
/// the same loose guard the string-table pin uses).
const STORE_ROW_OVERHEAD_CEILING: usize = 128;

/// Deep bytes of one file's capture: the inline header (which carries the
/// `FileIndex` and `RawSites` Vec headers) plus the production heap
/// accounting — the declaration rows, the supertype edges, the equal-name
/// run directory, and the site columns at reserved capacity (the capacity
/// pin below asserts the finish-time shrink left zero slack).
fn file_capture_bytes(capture: &FileCapture) -> usize {
    size_of::<FileCapture>() + capture.heap_bytes()
}

/// Deep bytes of one library's reference surface: the inline header plus
/// the production heap accounting — every boxed column at its exact length
/// (reference rows, the CSR run tables, the built and seam name rows, the
/// supertype rows, and their run directory).
fn library_index_bytes(surface: &LibraryIndex) -> usize {
    size_of::<LibraryIndex>() + surface.heap_bytes()
}

/// Deep bytes of one file's line index: the inline header plus the
/// line-start column. The wide-character records are sparse per non-ASCII
/// line and are empty here by construction — the fixture guard asserts the
/// synthetic source is pure ASCII.
fn line_index_bytes(index: &LineIndex) -> usize {
    size_of::<LineIndex>() + index.line_count() as usize * size_of::<u32>()
}

/// Overhead of the default `MemorySourceStore` (dartq-source store.rs:
/// `FxHashMap<ContentId, Vec<u8>>`) above the content bytes
/// `stored_bytes()` reports: a key + Vec-header slot per row, plus one
/// shared bucket array of the same slot shape at hashbrown's 7/8 maximum
/// load. Returns `(per_row, total)`.
fn store_overhead(rows: usize) -> (usize, usize) {
    let slot = size_of::<ContentId>() + size_of::<Vec<u8>>();
    let buckets = (rows * 8).div_ceil(7).max(4).next_power_of_two();
    (slot, rows * slot + buckets * slot)
}

/// The pin's normalization unit: resident KB per KB of source. Both sides
/// divide by 1024, so this is the plain byte ratio `bytes / source_bytes`.
fn kb_per_kb(bytes: usize, source_bytes: usize) -> f64 {
    bytes as f64 / source_bytes as f64
}

/// The synthetic fixture: one ~10 KB compilation unit with realistic
/// declaration and name density — 32 mid-sized classes (two fields, two
/// methods, an extends edge each), 24 top-level functions, and a main that
/// exercises every declaration. Deterministic and pure ASCII (the
/// `LineIndex` pin's no-wide-records composition relies on it).
fn synthetic_source() -> String {
    const CLASSES: usize = 32;
    const TOPS: usize = 24;

    let mut source = String::from("class Base {}\n\n");
    for i in 0..CLASSES {
        let _ = write!(
            source,
            "/// A synthetic domain holder {i}.\n\
             class Shape{i} extends Base {{\n  \
             int x{i} = 0;\n  \
             int y{i} = 0;\n  \
             void move{i}(int dx, int dy) {{\n    \
             x{i} = x{i} + dx;\n    \
             y{i} = y{i} + dy;\n  \
             }}\n  \
             int area{i}() => x{i} * y{i};\n\
             }}\n\n"
        );
    }
    for i in 0..TOPS {
        let _ = writeln!(source, "double ratio{i}(int w, int h) => w / h;");
    }
    source.push_str("\nvoid main() {\n  var total = 0;\n");
    for i in 0..CLASSES {
        let _ = write!(
            source,
            "  Shape{i} s{i} = Shape{i}();\n  \
             s{i}.move{i}(3, 4);\n  \
             total += s{i}.area{i}();\n"
        );
    }
    for i in 0..TOPS {
        let _ = writeln!(source, "  total += ratio{i}(3, 4).toInt();");
    }
    source.push_str("  print(total);\n}\n");
    source
}

#[test]
#[expect(clippy::too_many_lines)] // one fixture pins four structures end to end
fn synthetic_file_structures_pin_bytes_per_kb() {
    let source = synthetic_source();
    let source_bytes = source.len();
    assert!(
        (9 * 1024..=12 * 1024).contains(&source_bytes),
        "fixture drifted off the ~10 KB design point: {source_bytes} bytes"
    );
    assert!(source.is_ascii(), "the LineIndex pin assumes ASCII source");

    let fixture = Fixture::new(&[("lib/main.dart", "")]);
    let mut engine = fixture.engine();
    let file = engine.set_content(&fixture.path("lib/main.dart"), source.as_bytes());
    engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("analyze");

    let capture = engine.file_capture(file).expect("maintained capture");
    assert!(
        !capture.index.decls().is_empty(),
        "fixture must carry a declaration mass"
    );
    let line_index = engine.line_index(file).expect("resident line index");
    let surface = engine
        .library_surfaces()
        .next()
        .map(|(_, surface)| surface)
        .expect("one live library");

    // Exact site derivation (see the module docs): every captured site
    // folds into exactly one parser-level reference row or one built
    // names-table row, so the parser half plus the names table is the raw
    // site count. The refs slice also carries the semantic member-ref rows
    // the diagnose fold publishes beside the sites (#2618) — rows whose
    // non-`NONE` `target_container` marks them as member targets, not extra
    // sites. The seam overflow rows are not site-derived and stay out.
    let refs = surface.refs();
    let semantic_refs = refs
        .iter()
        .filter(|row| row.target_container != StringId::NONE)
        .count();
    let parser_refs = refs.len() - semantic_refs;
    let name_rows = surface.names().len();
    assert!(parser_refs > 0, "fixture must exercise the resolved half");
    assert!(name_rows > 0, "fixture must exercise the unresolved half");
    assert!(
        semantic_refs > 0,
        "fixture must exercise the semantic member-ref fold"
    );
    let site_rows = parser_refs + name_rows;
    // The finish-time freeze shrinks the token-count reservation
    // `RawSites::with_capacity` opened with down to the row count, so the
    // pinned slack is `capacity - len`. `Vec::shrink_to_fit` sets
    // `capacity = len` on the global allocator's shrink path (the column
    // either reallocs to the exact row count or, at `len == 0`, frees),
    // so zero is the deterministic residual, not a tolerance guess.
    let site_capacity = capture.sites_capacity();
    assert_eq!(
        site_capacity, site_rows,
        "FileCapture sites carry reservation slack: capacity {site_capacity} \
         vs {site_rows} rows ({RAW_SITE_ROW_BYTES} B/row; Builder::finish's \
         shrink_to_fit must leave capacity == len)"
    );

    // Content store: the engine's default memory store, one distinct
    // content after the apply.
    let rows = engine.stored_contents();
    let content_bytes = engine.stored_bytes();
    assert!(rows >= 1 && content_bytes > 0, "store must hold the file");
    let (store_row, store_total) = store_overhead(rows);

    // The pins: full-structure deep bytes per KB of source.
    let capture_bytes = file_capture_bytes(capture);
    let capture_per_kb = kb_per_kb(capture_bytes, source_bytes);
    assert!(
        capture_per_kb < FILE_CAPTURE_KB_PER_KB_CEILING,
        "FileCapture residency ballooned: {capture_bytes} bytes for a \
         {source_bytes}-byte file ({capture_per_kb:.2} KB per KB; ceiling \
         {FILE_CAPTURE_KB_PER_KB_CEILING}; decls {}, supers {}, sites {site_rows})",
        capture.index.decls().len(),
        capture.index.supers().len(),
    );

    let surface_bytes = library_index_bytes(surface);
    let surface_per_kb = kb_per_kb(surface_bytes, source_bytes);
    assert!(
        surface_per_kb < LIBRARY_INDEX_KB_PER_KB_CEILING,
        "LibraryIndex residency ballooned: {surface_bytes} bytes for a \
         {source_bytes}-byte file ({surface_per_kb:.2} KB per KB; ceiling \
         {LIBRARY_INDEX_KB_PER_KB_CEILING}; refs {}, parser {parser_refs}, \
         semantic {semantic_refs}, names {name_rows})",
        refs.len()
    );

    let line_bytes = line_index_bytes(line_index);
    let line_per_kb = kb_per_kb(line_bytes, source_bytes);
    assert!(
        line_per_kb < LINE_INDEX_KB_PER_KB_CEILING,
        "LineIndex residency ballooned: {line_bytes} bytes for a \
         {source_bytes}-byte file ({line_per_kb:.2} KB per KB; ceiling \
         {LINE_INDEX_KB_PER_KB_CEILING}; {} lines)",
        line_index.line_count()
    );

    assert!(
        store_row <= STORE_ROW_OVERHEAD_CEILING,
        "content-store row overhead ballooned: {store_row} B per row above \
         the bytes (ceiling {STORE_ROW_OVERHEAD_CEILING}; key {} + header {})",
        size_of::<ContentId>(),
        size_of::<Vec<u8>>()
    );
    assert!(
        store_total * 8 < content_bytes as usize,
        "content-store structure overhead ballooned: {store_total} B of \
         slots/buckets over {rows} rows carrying {content_bytes} content bytes"
    );

    println!(
        "synthetic file: {source_bytes} bytes, {} lines",
        line_index.line_count()
    );
    println!(
        "decls {}, supers {}, sites {site_rows} rows {site_capacity} cap x {RAW_SITE_ROW_BYTES} B",
        capture.index.decls().len(),
        capture.index.supers().len()
    );
    println!(
        "refs {} (parser {parser_refs}, semantic {semantic_refs}), names {name_rows}",
        refs.len()
    );
    println!(
        "FileCapture:   {capture_bytes:>6} B  ({capture_per_kb:.2} KB/KB; audit est 0.43, ceiling {FILE_CAPTURE_KB_PER_KB_CEILING})"
    );
    println!(
        "LibraryIndex:  {surface_bytes:>6} B  ({surface_per_kb:.2} KB/KB; audit est 0.65, ceiling {LIBRARY_INDEX_KB_PER_KB_CEILING})"
    );
    println!(
        "LineIndex:     {line_bytes:>6} B  ({line_per_kb:.2} KB/KB; ceiling {LINE_INDEX_KB_PER_KB_CEILING})"
    );
    println!(
        "content store: {rows} row(s), {content_bytes} content bytes, +{store_row} B/row (+{store_total} B structure)"
    );
}
