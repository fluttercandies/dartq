//! Discovery walks the whole package root (#2165): `dart analyze` includes
//! every `.dart` file under an included folder (`context_root.dart`
//! `_includedFilesInFolder`), skipping only dot-paths and option excludes —
//! not just `lib`/`bin`/`test`/`tool`. Files in any other top-level
//! directory (`zadv/`, `example/`, `integration_test/`) enter the analysis
//! set, dot-directories stay out, and an exclude glob still applies.

mod common;

use common::Fixture;

#[test]
fn every_non_dot_directory_under_the_root_is_discovered() {
    let fixture = Fixture::new(&[
        ("lib/a.dart", "void a() {}\n"),
        ("zadv/deep/b.dart", "void b() {}\n"),
        ("example/c.dart", "void c() {}\n"),
        (".hidden/d.dart", "void d() {}\n"),
        ("skipped/e.dart", "void e() {}\n"),
    ]);
    std::fs::write(
        fixture.path("analysis_options.yaml"),
        "analyzer:\n  exclude:\n    - \"skipped/**\"\n",
    )
    .expect("options");

    let mut engine = fixture.engine();
    let stats = engine.discover_workspace().expect("discovery");
    let interned = |relative: &str| {
        engine
            .file_table()
            .try_file_id(&fixture.path(relative))
            .is_some()
    };

    assert!(interned("lib/a.dart"), "lib surface");
    assert!(interned("zadv/deep/b.dart"), "a non-surface top-level dir");
    assert!(interned("example/c.dart"), "example/");
    assert!(!interned(".hidden/d.dart"), "dot directories stay out");
    assert!(!interned("skipped/e.dart"), "option excludes still apply");
    assert_eq!(stats.file_count, 3, "exactly the three included files");
}
