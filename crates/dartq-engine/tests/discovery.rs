//! Discovery symlink pins (goal #215; same rule as the CLI walk, commit
//! b4d9bc9): a symlinked `.dart` file is discovered when its target stats
//! as a regular file (the oracle analyzes linked files too, so skipping
//! them would flood the differential gate with phantom oracle-only
//! entries), a symlinked directory is never descended into (cycle
//! safety), and a dangling link is skipped without error.

mod common;

#[cfg(unix)]
#[test]
fn discovery_includes_symlinked_files_not_symlinked_dirs() {
    use std::os::unix::fs::symlink;

    use common::Fixture;

    // `.hidden/` is a dot directory, never walked — `nested.dart` can only
    // enter the discovered set through one of the dir links below, so
    // interning it would mean a dir link was descended. (The file table canonicalizes
    // lookups, so a file reachable canonically can't serve as this pin.)
    let fixture = Fixture::new(&[
        ("lib/real.dart", "void real() {}\n"),
        (".hidden/nested.dart", "void nested() {}\n"),
    ]);
    std::fs::create_dir(fixture.path("test")).expect("mkdir test");
    symlink(fixture.path("lib/real.dart"), fixture.path("lib/link.dart")).expect("file link");
    symlink(fixture.path(".hidden"), fixture.path("test/linked_lib")).expect("dir link");
    // A dir link named `*.dart`: without the target stat it would fall
    // into the file branch and crash discovery reading a directory.
    symlink(
        fixture.path(".hidden"),
        fixture.path("test/dir_as_dart.dart"),
    )
    .expect("dir link with dart name");
    symlink(
        fixture.path("lib/gone.dart"),
        fixture.path("lib/dangling.dart"),
    )
    .expect("dangling link");

    let mut engine = fixture.engine();
    let stats = engine.discover_workspace().expect("discovery");
    engine.apply_pending(&mut |_| {});

    // real.dart once plus its link; the dir links contributed nothing and
    // the dangling link was skipped.
    assert_eq!(
        stats.file_count, 2,
        "linked file in, dir links not descended, dangling link skipped"
    );

    let real_id = engine
        .file_table()
        .try_file_id(&fixture.path("lib/real.dart"))
        .expect("real file discovered");
    assert_eq!(
        engine.content(real_id),
        Some("void real() {}\n".as_bytes()),
        "bytes read through the link"
    );

    for skipped in [".hidden/nested.dart", "lib/dangling.dart"] {
        assert!(
            engine
                .file_table()
                .try_file_id(&fixture.path(skipped))
                .is_none(),
            "{skipped} must not be interned"
        );
    }
}

/// A discovered file over the engine's span capacity
/// ([`dartq_source::MAX_SOURCE_BYTES`]) is rejected at the open gate and
/// never enters the set: not interned, not counted, and reported in
/// `DiscoveryStats::oversized` for the faces' notice channel (#1543). The
/// fixture is a sparse file — `set_len` sizes it without writing blocks,
/// so the test costs no disk and no read pass; what the gate checks is
/// exactly the metadata length the fixture sets.
#[test]
fn discovery_skips_files_over_the_span_capacity() {
    use common::Fixture;

    let fixture = Fixture::new(&[("lib/real.dart", "void real() {}\n")]);
    let huge = std::fs::File::create(fixture.path("lib/huge.dart")).expect("create fixture");
    huge.set_len(dartq_source::MAX_SOURCE_BYTES as u64 + 1)
        .expect("size fixture");
    drop(huge);

    let mut engine = fixture.engine();
    let stats = engine.discover_workspace().expect("discovery");
    engine.apply_pending(&mut |_| {});

    assert_eq!(stats.file_count, 1, "only real.dart enters the set");
    assert_eq!(stats.oversized, vec![fixture.path("lib/huge.dart")]);
    assert!(
        engine
            .file_table()
            .try_file_id(&fixture.path("lib/huge.dart"))
            .is_none(),
        "rejected file must not be interned"
    );
}
