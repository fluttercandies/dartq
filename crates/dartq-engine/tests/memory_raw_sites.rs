//! Raw-site cache pins (#1859): the engine's site LRU sheds over-cap
//! captures' site columns oldest-first, a dirty-library rebuild after a
//! shed re-derives identical rows, and the occupancy hook reports the
//! cache count.
//!
//! The LRU discipline under pin mirrors the scanned-face fallback
//! (`state.rs`): feed on recapture, evict the oldest feed on overflow,
//! re-derive locally on a rebuild miss without refilling. Sites are a pure
//! function of (content, feature set), so shed + re-derive must be
//! row-identical to the never-shed path — that equality is the second
//! pin, built as two engines over the same source pair (the big one adds
//! enough filler libraries to shed the pair's part file; the small one
//! keeps the pair alone, never shed).
//!
//! Run: `cargo test -p dartq-engine --test memory_raw_sites`

mod common;

use std::fmt::Write as _;

use common::Fixture;
use dartq_engine::{CancellationToken, Engine};
use dartq_source::FileId;

/// Mirror of the engine's private `SITES_CACHE_CAP` (the occupancy face
/// reports the live count; the constant itself stays private).
const SITES_CACHE_CAP: usize = 1024;

/// The main side of the part pair. `partFn` resolves library-locally (the
/// part declares it), so the pair's surface carries resolved rows from
/// both members.
const MAIN_0: &str = "part 'shed.dart';\n\nvoid main() {\n  print(partFn());\n}\n";
/// The main side after the library-dirtying edit: one fresh top-level
/// declaration, so the analyze that follows rebuilds the pair's library
/// surface whole (the `#1859` re-derivation seam runs).
const MAIN_1: &str =
    "part 'shed.dart';\n\nvoid main() {\n  print(partFn());\n}\n\nint extra() => 9;\n";
/// The part side: one resolved intra-file reference (`partFn` →
/// `partHelper`) and one unresolved name (`nowhere`) — both halves of the
/// site fold the library build consumes.
const PART_0: &str =
    "part of 'main.dart';\n\nint partFn() => partHelper() + nowhere();\n\nint partHelper() => 2;\n";

/// Standalone filler library: one declaration plus one unresolved
/// reference, so its capture carries a nonempty site table (a zero-site
/// capture sheds to an indistinguishable twin and pins nothing).
fn filler_source(i: usize) -> String {
    let mut source = format!("int filler{i}() => absent{i};\n");
    let _ = writeln!(source, "int extra{i}() => filler{i}();");
    source
}

/// Resolved reference rows of one file, with ids normalized away:
/// `FileId`s are intern-order-native per engine (the SDK core file alone
/// lands at a different id once 1025 fillers intern first), so rows
/// compare over the resolved target name, kind, and span — the fold's
/// semantic identity. Target anchoring is asserted per engine instead.
fn refs_of(engine: &Engine, file: FileId) -> Vec<(String, String, u32, u32)> {
    let surface = engine
        .library_surfaces()
        .map(|(_, surface)| surface)
        .find(|surface| surface.refs().iter().any(|row| row.file == file))
        .expect("the file's library has a surface");
    let strings = engine.strings();
    surface
        .refs()
        .iter()
        .filter(|row| row.file == file)
        .map(|row| {
            (
                strings.resolve(row.target_name).to_owned(),
                format!("{:?}", row.kind),
                row.range.offset,
                row.range.length,
            )
        })
        .collect()
}

#[test]
fn over_capacity_feed_sheds_oldest_sites_and_reports_occupancy() {
    // CAP + 1 standalone libraries. Feed order is FileId order (the
    // recapture queue sorts by file id), so the first-written file is the
    // shed victim and the last-written one survives.
    let files: Vec<(String, String)> = (0..=SITES_CACHE_CAP)
        .map(|i| (format!("lib/f{i:04}.dart"), filler_source(i)))
        .collect();
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, s)| (p.as_str(), s.as_str()))
        .collect();
    let fixture = Fixture::new(&borrowed);
    let mut engine = fixture.engine();
    let ids: Vec<FileId> = files
        .iter()
        .map(|(path, source)| engine.set_content(&fixture.path(path), source.as_bytes()))
        .collect();
    engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("analyze");

    // Occupancy: the feed capped at SITES_CACHE_CAP, one shed.
    assert_eq!(
        engine.sites_occupancy(),
        SITES_CACHE_CAP,
        "site cache must cap at SITES_CACHE_CAP after the over-cap feed"
    );

    // The oldest feed's sites are shed: the capture keeps its declaration
    // surface and reports the shed twin's empty columns.
    let oldest = engine.file_capture(ids[0]).expect("capture");
    assert!(
        !oldest.sites_present(),
        "the oldest-fed file's sites must be shed"
    );
    assert_eq!(oldest.sites_capacity(), 0, "shed columns report zero");
    assert!(
        !oldest.index.decls().is_empty(),
        "the shed twin must keep the declaration surface"
    );

    // The freshest feed keeps its sites mounted.
    let newest = engine.file_capture(ids[SITES_CACHE_CAP]).expect("capture");
    assert!(
        newest.sites_present(),
        "the newest-fed file keeps its sites"
    );
    assert!(
        newest.sites_capacity() > 0,
        "the mounted site table carries rows"
    );
}

#[test]
fn rebuild_after_shed_rederives_identical_rows() {
    // Big engine: the part pair plus SITES_CACHE_CAP filler libraries —
    // CAP + 2 feeds, so the two oldest (the part, then the first filler)
    // shed. The part is written first, making it the victim; main is
    // written last and stays mounted. Small engine: the same pair alone,
    // never shed — the never-evicted baseline.
    let big_files: Vec<(String, String)> =
        std::iter::once(("lib/shed.dart".to_owned(), PART_0.to_owned()))
            .chain((0..SITES_CACHE_CAP).map(|i| (format!("lib/f{i:04}.dart"), filler_source(i))))
            .chain(std::iter::once((
                "lib/main.dart".to_owned(),
                MAIN_0.to_owned(),
            )))
            .collect();
    let borrowed: Vec<(&str, &str)> = big_files
        .iter()
        .map(|(p, s)| (p.as_str(), s.as_str()))
        .collect();
    let big_fixture = Fixture::new(&borrowed);
    let mut big = big_fixture.engine();
    let mut part_id = None;
    let mut main_id = None;
    for (path, source) in &big_files {
        let id = big.set_content(&big_fixture.path(path), source.as_bytes());
        if path == "lib/shed.dart" {
            part_id = Some(id);
        }
        if path == "lib/main.dart" {
            main_id = Some(id);
        }
    }
    let (part, main) = (part_id.expect("part id"), main_id.expect("main id"));
    big.analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("analyze");
    assert!(
        !big.file_capture(part).expect("capture").sites_present(),
        "the pair's part must be shed in the over-cap engine"
    );

    // Library-dirtying edit on main: the pair's library rebuilds whole at
    // the next analyze, and the rebuild face re-derives the part's shed
    // sites from content.
    big.set_content(&big_fixture.path("lib/main.dart"), MAIN_1.as_bytes());
    big.analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("rebuild analyze");

    // Re-derivation never refills the cache (the scanned-face fallback
    // posture): the part's capture stays shed after the rebuild. Main was
    // recaptured by the same edit, so its sites re-fed and stay mounted —
    // the two halves of the discipline in one engine.
    assert!(
        !big.file_capture(part).expect("capture").sites_present(),
        "a rebuild miss re-derives locally; the site cache must stay capped"
    );
    assert!(
        big.file_capture(main).expect("capture").sites_present(),
        "the recaptured main re-feeds the site cache"
    );

    // The never-evicted baseline: same pair alone, sites never shed.
    let small_fixture = Fixture::new(&[("lib/main.dart", MAIN_0), ("lib/shed.dart", PART_0)]);
    let mut small = small_fixture.engine();
    let small_part = small.set_content(&small_fixture.path("lib/shed.dart"), PART_0.as_bytes());
    small.set_content(&small_fixture.path("lib/main.dart"), MAIN_0.as_bytes());
    small
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("analyze");
    assert!(
        small
            .file_capture(small_part)
            .expect("capture")
            .sites_present(),
        "the baseline engine never sheds"
    );
    assert_eq!(small.sites_occupancy(), 2, "both pair members stay cached");

    // Row equality: the rebuilt surface's part-file reference rows are
    // identical to the never-shed build's (ids normalized; see refs_of).
    let rebuilt = refs_of(&big, part);
    let baseline = refs_of(&small, small_part);
    assert_eq!(
        rebuilt, baseline,
        "re-derived sites must fold into identical reference rows"
    );
    assert!(
        !rebuilt.is_empty(),
        "fixture must carry resolved rows through the re-derivation"
    );

    // Target anchoring, per engine's own id universe: the intra-file
    // `partHelper` row resolves to the part file, the `int` rows to the
    // SDK core file.
    for (engine, part_file) in [(&big, part), (&small, small_part)] {
        for row in engine
            .library_surfaces()
            .flat_map(|(_, surface)| surface.refs().iter())
            .filter(|row| row.file == part_file)
        {
            let target = engine.strings().resolve(row.target_name);
            let expected = if target == "partHelper" {
                part_file
            } else {
                continue;
            };
            assert_eq!(
                row.target_file, expected,
                "{target} row must anchor to the part file"
            );
        }
    }

    // The unresolved half folds identically too: the part's `nowhere`
    // reference is a names-table row in each engine (fillers use distinct
    // names, so the row count over all surfaces is exact).
    for engine in [&big, &small] {
        let id = engine.strings().intern("nowhere");
        let count: usize = engine
            .library_surfaces()
            .map(|(_, surface)| surface.unresolved(id).count())
            .sum();
        assert_eq!(count, 1, "exactly one `nowhere` row in each engine");
    }
}
