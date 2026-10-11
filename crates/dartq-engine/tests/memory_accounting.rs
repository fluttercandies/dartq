//! Per-structure memory accounting pins (goal #778): kilobytes of residency
//! per kilobyte of source for the structures the engine keeps resident per
//! file and per library.
//! The R1 residency audit's widest error bars were code-derived estimates
//! (FileCapture ~0.43x source, LibraryIndex ~6.5 KB/file) flagged as the
//! least trustworthy precisely because nothing measured them; this test
//! replaces the estimate with a measurement.
//!
//! One synthetic ~10 KB Dart file with realistic declaration and name
//! density drives the real production path — `Engine::set_content` →
//! `Engine::analyze` — and each resident structure is walked through public
//! accessors only: `std::mem::size_of` for the inline header plus
//! collection length × element size for the heap (the
//! `dartq-ast/tests/metrics.rs` / `string_table_memory` posture). Two
//! internals are not nameable from an integration test and are counted by
//! documented composition instead:
//!
//! * `RawSites` (dartq-index, crate-internal): five parallel columns of
//!   StringId + kind + family + offset + length = 14 B per site (the five
//!   Vec headers are already inside `size_of::<FileCapture>()`). The site
//!   count is derived exactly: every captured site folds into exactly one
//!   `RefRow` or one `NameRow` ([`dartq_index::LibraryIndex`] build), so
//!   sites = the library's reference count plus its unresolved-name count,
//!   and the names are enumerable by probing every word token of the
//!   source — site names are always word tokens and keywords never have
//!   rows, so the over-probe is free and the sum is exact.
//! * `SubtypeEdge` is not re-exported, so its row is counted by field-list
//!   composition (file + name + supertype, dartq-index `library.rs`).
//!
//! The ceilings pin order of magnitude, not exact bytes. They sit ~3x
//! above the expected measurement (the audit's 0.43x estimate could not
//! see the crate-internal site columns, so the honest full-structure
//! number is higher) and trip where a row starts carrying duplicated text
//! or per-row heap structures instead of interned ids and flat columns.
//! The fail messages print the measured numbers, so a trip is itself the
//! evidence needed to recalibrate.
//!
//! FileElements (dartq-semantic) is deliberately absent: that crate is
//! mid-flight on a parallel lane and this test must not touch it. Its pin
//! appends behind the same helpers — a `file_elements_bytes` estimator
//! over `Engine::file_facts(file)`, joining the same synthetic file and
//! the same per-KB normalization — once that lane lands.
//!
//! Run: `cargo test -p dartq-engine --test memory_accounting -- --nocapture`

#![allow(clippy::cast_possible_truncation)] // u32 line counts widen; measurement math only
#![allow(clippy::cast_precision_loss)] // measurement math only (byte counts are far below 2^52)

mod common;

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::mem::size_of;

use common::Fixture;
use dartq_index::{DeclRow, FileCapture, LibraryIndex, NameRow, RefRow, SuperEdge, TargetRef};
use dartq_source::{ContentId, FileId, LineIndex, StringId};

/// Site columns of the crate-internal `RawSites` (dartq-index capture.rs):
/// StringId + kind byte + family byte + offset + length. The five Vec
/// headers are already inside `size_of::<FileCapture>()`.
const RAW_SITE_ROW_BYTES: usize = size_of::<StringId>() + 2 + 2 * size_of::<u32>();

/// Full-capture ceiling: 5 KB of resident capture per KB of source. The
/// audit estimated ~0.43x from the visible rows alone; the honest
/// full-structure measurement (declaration rows + superclass edges + the
/// derived site columns) is expected near 1.8 KB/KB on this fixture
/// (first run measured 1.57: 16276 B over 10343 B).
const FILE_CAPTURE_KB_PER_KB_CEILING: f64 = 5.0;

/// Reference-surface ceiling: 5 KB per KB of source. Expected near
/// 1.7 KB/KB on this single-library fixture (most uses land in the names
/// table; a workspace with imports shifts them to slightly larger RefRows,
/// not more bytes).
const LIBRARY_INDEX_KB_PER_KB_CEILING: f64 = 5.0;

/// Line-index ceiling: 0.75 KB per KB of source — 4 B per line start plus
/// the sparse wide-line records (zero here; the fixture is ASCII). Realistic
/// line lengths keep this near 0.2; the ceiling trips a per-line record
/// blowup (16 B/line or wide-char records on ASCII).
const LINE_INDEX_KB_PER_KB_CEILING: f64 = 0.75;

/// Per-row cost of the default `MemorySourceStore` above the bytes it
/// indexes: the ContentId key plus the Vec header (a row that starts
/// cloning text or carrying per-row metadata more than doubles this —
/// the same loose guard the string-table pin uses).
const STORE_ROW_OVERHEAD_CEILING: usize = 128;

/// Deep bytes of one file's capture: the inline header (which already
/// carries the FileIndex and RawSites Vec headers) plus the declaration
/// rows, the superclass edges, and the derived site columns.
fn file_capture_bytes(capture: &FileCapture, site_rows: usize) -> usize {
    size_of::<FileCapture>()
        + capture.index.decls().len() * size_of::<DeclRow>()
        + capture.index.supers().len() * size_of::<SuperEdge>()
        + site_rows * RAW_SITE_ROW_BYTES
}

/// Deep bytes of one library's reference surface. The names table and the
/// CSR runs array have no public length; both are derived — names rows
/// from `name_rows` (the probe sum), runs as the distinct-target count of
/// the sorted refs (one u32 run start per target). The supertype edge row
/// is `FileId + StringId + TargetRef` by field list.
fn library_index_bytes(surface: &LibraryIndex, name_rows: usize, super_rows: usize) -> usize {
    let refs = surface.refs();
    let mut runs = 0usize;
    let mut prev: Option<(FileId, StringId)> = None;
    for row in refs {
        let key = (row.target_file, row.target_name);
        if prev != Some(key) {
            runs += 1;
            prev = Some(key);
        }
    }
    let super_row = 2 * size_of::<FileId>() + size_of::<StringId>() + size_of::<TargetRef>();
    size_of::<LibraryIndex>()
        + refs.len() * size_of::<RefRow>()
        + runs * size_of::<u32>()
        + name_rows * size_of::<NameRow>()
        + super_rows * super_row
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
/// LineIndex pin's no-wide-records composition relies on it).
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
        .analyze(&[], &mut |_| {}, &Default::default())
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

    // Exact site/name derivation (see the module docs): probe every word
    // token; resolved uses are the reference rows, everything else is a
    // names-table row, and the two together are the raw site count.
    let strings = engine.strings();
    let words: BTreeSet<&str> = source
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|word| {
            word.chars()
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        })
        .collect();
    let name_rows: usize = words
        .iter()
        .map(|word| surface.unresolved(strings.intern(word)).count())
        .sum();
    let refs = surface.reference_count();
    assert!(refs > 0, "fixture must exercise the resolved half");
    assert!(name_rows > 0, "fixture must exercise the unresolved half");
    let site_rows = refs + name_rows;

    // Content store: the engine's default memory store, one distinct
    // content after the apply.
    let rows = engine.stored_contents();
    let content_bytes = engine.stored_bytes();
    assert!(rows >= 1 && content_bytes > 0, "store must hold the file");
    let (store_row, store_total) = store_overhead(rows);

    // The pins: full-structure deep bytes per KB of source.
    let capture_bytes = file_capture_bytes(capture, site_rows);
    let capture_per_kb = kb_per_kb(capture_bytes, source_bytes);
    assert!(
        capture_per_kb < FILE_CAPTURE_KB_PER_KB_CEILING,
        "FileCapture residency ballooned: {capture_bytes} bytes for a \
         {source_bytes}-byte file ({capture_per_kb:.2} KB per KB; ceiling \
         {FILE_CAPTURE_KB_PER_KB_CEILING}; decls {}, supers {}, sites {site_rows})",
        capture.index.decls().len(),
        capture.index.supers().len(),
    );

    let surface_bytes = library_index_bytes(surface, name_rows, capture.index.supers().len());
    let surface_per_kb = kb_per_kb(surface_bytes, source_bytes);
    assert!(
        surface_per_kb < LIBRARY_INDEX_KB_PER_KB_CEILING,
        "LibraryIndex residency ballooned: {surface_bytes} bytes for a \
         {source_bytes}-byte file ({surface_per_kb:.2} KB per KB; ceiling \
         {LIBRARY_INDEX_KB_PER_KB_CEILING}; refs {refs}, names {name_rows})"
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
        "decls {} x {} B, supers {} x {} B, sites {site_rows} x {RAW_SITE_ROW_BYTES} B",
        capture.index.decls().len(),
        size_of::<DeclRow>(),
        capture.index.supers().len(),
        size_of::<SuperEdge>()
    );
    println!(
        "refs {refs} x {} B, names {name_rows} x {} B",
        size_of::<RefRow>(),
        size_of::<NameRow>()
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
