//! The dependency-interface index pin: a `package:` import naming a
//! non-workspace (pub-cache) package resolves the dependency's class and
//! member faces through the lazy loader, so the workspace's own rows see
//! exactly the faces a same-package import sees — the override check
//! consults the dep class's member, an incompatible override reports
//! `invalid_override`, and the dep files themselves stay row-silent (the
//! differential counts workspace files only).
//!
//! The fixture is the `dep-interface` tree materialized into a fresh temp
//! root from `tests/fixtures/dep-interface/` (tracked verbatim): the `ws`
//! package's hand-written `package_config.json` points the `dep` entry at
//! the sibling directory (a relative `rootUri` is legal per the pub
//! package_config format), and the hosted `pubspec.lock` entry classifies
//! `dep` as cache content — not workspace source — which is the shape the
//! lazy index keys on. The dep files are never fed as content: they reach
//! the analysis only through the on-disk load the import triggers.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dartq_diagnostics::Diagnostic;
use dartq_engine::{Engine, EngineConfig, memory_store};
use dartq_package::PackageTable;
use dartq_pub::{LockFile, PackageConfig, Pubspec, build_graph};
use dartq_sdk::SdkSnapshot;

/// The fixture's file layout, materialized verbatim into a temp root per
/// run. The pub-generated config lives as `ws/dart_tool/package_config.json`
/// in the fixture (a dot-directory cannot be tracked) and lands at
/// `ws/.dart_tool/package_config.json` in the materialized tree.
const FIXTURE_FILES: &[(&str, &str)] = &[
    (
        "ws/pubspec.yaml",
        include_str!("fixtures/dep-interface/ws/pubspec.yaml"),
    ),
    (
        "ws/pubspec.lock",
        include_str!("fixtures/dep-interface/ws/pubspec.lock"),
    ),
    (
        "ws/.dart_tool/package_config.json",
        include_str!("fixtures/dep-interface/ws/dart_tool/package_config.json"),
    ),
    (
        "ws/lib/bad_override.dart",
        include_str!("fixtures/dep-interface/ws/lib/bad_override.dart"),
    ),
    (
        "ws/lib/good_override.dart",
        include_str!("fixtures/dep-interface/ws/lib/good_override.dart"),
    ),
    (
        "ws/lib/local_base.dart",
        include_str!("fixtures/dep-interface/ws/lib/local_base.dart"),
    ),
    (
        "ws/lib/twin_bad.dart",
        include_str!("fixtures/dep-interface/ws/lib/twin_bad.dart"),
    ),
    (
        "ws/lib/twin_good.dart",
        include_str!("fixtures/dep-interface/ws/lib/twin_good.dart"),
    ),
    (
        "dep/lib/dep.dart",
        include_str!("fixtures/dep-interface/dep/lib/dep.dart"),
    ),
    (
        "dep/lib/src/base.dart",
        include_str!("fixtures/dep-interface/dep/lib/src/base.dart"),
    ),
];

const BAD_OVERRIDE: &str = include_str!("fixtures/dep-interface/ws/lib/bad_override.dart");
const GOOD_OVERRIDE: &str = include_str!("fixtures/dep-interface/ws/lib/good_override.dart");
const LOCAL_BASE: &str = include_str!("fixtures/dep-interface/ws/lib/local_base.dart");
const TWIN_BAD: &str = include_str!("fixtures/dep-interface/ws/lib/twin_bad.dart");
const TWIN_GOOD: &str = include_str!("fixtures/dep-interface/ws/lib/twin_good.dart");

/// Materializes the fixture tree into a fresh process-unique temp root and
/// returns the `ws` package path.
fn materialize_fixture(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "dartq-engine-dep-index-fixture-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    for (relative, content) in FIXTURE_FILES {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("fixture dir");
        std::fs::write(&path, content).expect("fixture file");
    }
    root.join("ws")
}

/// Assembles the engine over the materialized fixture tree (read-only): the
/// config, lock, and pubspec parse from disk; the synthetic VM-segment SDK
/// is the usual throwaway. The caller feeds the `ws/lib` files as content —
/// the `dep` tree stays disk-only, which is the lazy load under test.
fn corpus_engine(tag: &str) -> (Engine, PathBuf) {
    let ws = materialize_fixture(tag);
    let config_dir = ws.join(".dart_tool");
    let config = Arc::new(
        PackageConfig::parse(
            &std::fs::read_to_string(config_dir.join("package_config.json"))
                .expect("config"),
        )
        .expect("valid config"),
    );
    let lock = LockFile::parse(
        &std::fs::read_to_string(ws.join("pubspec.lock")).expect("lock"),
    )
    .expect("valid lock");
    let pubspec = Pubspec::parse(
        &std::fs::read_to_string(ws.join("pubspec.yaml")).expect("pubspec"),
    )
    .expect("valid pubspec");
    let graph = build_graph(
        &config,
        &lock,
        &pubspec,
        &std::collections::BTreeMap::new(),
        None,
        &config_dir,
    )
    .expect("graph");
    let packages = PackageTable::build(&config, &graph, &config_dir).expect("table");

    // The synthetic SDK: the throwaway minimum (version file, the `vm`
    // segment's core entry, the empty experiment whitelist).
    let sdk_root = std::env::temp_dir().join(format!(
        "dartq-engine-dep-index-sdk-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&sdk_root);
    let write = |relative: &str, content: &str| {
        let path = sdk_root.join(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(path, content).expect("file");
    };
    write("version", "3.13.1\n");
    write(
        "lib/libraries.json",
        r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}}}}"#,
    );
    write(
        "lib/_internal/allowed_experiments.json",
        r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
    );

    let engine = Engine::new(EngineConfig {
        store: memory_store(),
        sdk: SdkSnapshot::load(&sdk_root, None).expect("synthetic SDK loads"),
        packages,
        pub_config: config,
        config_dir,
        semdiag: Some(dartq_engine::semdiag::diagnoser()),
        target: dartq_engine::DEFAULT_TARGET.to_owned(),
        declared_variables: rustc_hash::FxHashMap::default(),
        experiments: Vec::new(),
    })
    .expect("engine");
    (engine, ws)
}

/// The code names reported for the file whose path ends with `tail`,
/// sorted (the engine's canonical order already sorts; the collect is a
/// per-file projection).
fn codes_for(diagnostics: &[Diagnostic], engine: &Engine, tail: &str) -> Vec<&'static str> {
    let mut codes: Vec<&'static str> = diagnostics
        .iter()
        .filter(|diagnostic| {
            engine
                .file_table()
                .path(diagnostic.file)
                .is_some_and(|path: &Path| path.ends_with(tail))
        })
        .map(|diagnostic| diagnostic.code.name)
        .collect();
    codes.sort_unstable();
    codes.dedup();
    codes
}

/// The dep class's member face resolves exactly like the same-package
/// baseline: the incompatible override reports `invalid_override` against
/// the dep base (its `int method()` member is what the verdict consults),
/// the compatible one reports nothing, and neither the dep-importing files
/// nor the dep files themselves carry a single row — no `uri_does_not_exist`
/// staleness, no `undefined`/`extends_non_class` noise, nothing under the
/// dep tree.
#[test]
fn dep_member_face_matches_the_same_package_baseline() {
    let (mut engine, ws) = corpus_engine("baseline");
    engine.set_content(&ws.join("lib/bad_override.dart"), BAD_OVERRIDE.as_bytes());
    engine.set_content(&ws.join("lib/good_override.dart"), GOOD_OVERRIDE.as_bytes());
    engine.set_content(&ws.join("lib/local_base.dart"), LOCAL_BASE.as_bytes());
    engine.set_content(&ws.join("lib/twin_bad.dart"), TWIN_BAD.as_bytes());
    engine.set_content(&ws.join("lib/twin_good.dart"), TWIN_GOOD.as_bytes());
    let diagnostics = engine
        .analyze(
            &[],
            &mut |_| {},
            &dartq_engine::CancellationToken::default(),
        )
        .expect("analyze completes");

    // The same-package baseline pins the face: exactly one row family, the
    // invalid-override verdict.
    assert_eq!(
        codes_for(&diagnostics, &engine, "twin_bad.dart"),
        vec!["invalid_override"],
        "the same-package baseline must report the incompatible override"
    );
    assert!(
        codes_for(&diagnostics, &engine, "twin_good.dart").is_empty(),
        "the compatible same-package override must stay silent"
    );
    // The dep-importing twin reports the identical face: the dep class's
    // member is visible to the override check, and the import resolves
    // with no staleness or undefined noise.
    assert_eq!(
        codes_for(&diagnostics, &engine, "bad_override.dart"),
        vec!["invalid_override"],
        "the dep base's member face must back the same override verdict"
    );
    assert!(
        codes_for(&diagnostics, &engine, "good_override.dart").is_empty(),
        "the compatible dep override must stay silent: no uri_does_not_exist, \
         no undefined rows"
    );
    // The dep files themselves yield zero rows: the index serves faces,
    // never diagnostics.
    let dep_root = ws.parent().expect("fixture root").join("dep");
    let dep_rows: Vec<&Diagnostic> = diagnostics
        .iter()
        .filter(|diagnostic| {
            engine
                .file_table()
                .path(diagnostic.file)
                .is_some_and(|path: &Path| path.starts_with(&dep_root))
        })
        .collect();
    assert!(
        dep_rows.is_empty(),
        "dependency files must stay row-silent: {dep_rows:?}"
    );
}
