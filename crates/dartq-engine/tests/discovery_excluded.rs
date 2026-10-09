//! Discovery exclusion parity pins (goal #291): the only built-in
//! exclusion is the oracle's dot rule — directories and files alike, every
//! component below the root (context_root.dart:146-166 `_isExcluded`;
//! context_locator.dart:438-447 for root discovery). `build`/`node_modules`
//! are NOT excluded by default: the official server reports *unexcluded*
//! `node_modules` dirs as a performance warning and advises adding them to
//! `analysis_options` excludes (performance_page.dart:272-354), so the old
//! name-based skip diverged from the oracle's file universe. Options
//! excludes compose on top of the dot rule; neither layer overrides the
//! other.

mod common;

use common::Fixture;

#[test]
fn builtin_exclusion_is_dot_rule_only_and_options_compose_on_top() {
    let fixture = Fixture::new(&[
        ("lib/keep.dart", "void keep() {}\n"),
        // Inside a surface: the oracle analyzes build/ and node_modules/
        // content unless options exclude it — a name-based skip here made
        // the engine's file universe diverge from the oracle's.
        ("lib/build/gen.dart", "void gen() {}\n"),
        ("lib/node_modules/vendor.dart", "void vendor() {}\n"),
        // The dot rule, files and directories alike.
        ("lib/.secret.dart", "void secret() {}\n"),
        ("lib/.skipped/deep.dart", "void deep() {}\n"),
        (".hidden_root.dart", "void hiddenRoot() {}\n"),
    ]);
    // Options excludes compose on top of the built-in: `vendored/` leaves
    // the set while `build/` stays in — the two layers are orthogonal.
    std::fs::write(
        fixture.path("analysis_options.yaml"),
        "analyzer:\n  exclude:\n    - \"lib/vendored/**\"\n",
    )
    .expect("options");
    std::fs::create_dir_all(fixture.path("lib/vendored")).expect("mkdir vendored");
    std::fs::write(fixture.path("lib/vendored/v.dart"), "void v() {}\n")
        .expect("write vendored file");

    let mut engine = fixture.engine();
    let stats = engine.discover_workspace().expect("discovery");
    engine.apply_pending(&mut |_| {});

    assert_eq!(
        stats.file_count, 3,
        "keep + build/gen + node_modules/vendor enter the set"
    );
    for interned in [
        "lib/keep.dart",
        "lib/build/gen.dart",
        "lib/node_modules/vendor.dart",
    ] {
        assert!(
            engine
                .file_table()
                .try_file_id(&fixture.path(interned))
                .is_some(),
            "{interned} must be interned (no built-in name exclusions)"
        );
    }
    for skipped in [
        "lib/.secret.dart",
        "lib/.skipped/deep.dart",
        ".hidden_root.dart",
        "lib/vendored/v.dart",
    ] {
        assert!(
            engine
                .file_table()
                .try_file_id(&fixture.path(skipped))
                .is_none(),
            "{skipped} must not be interned"
        );
    }
}
