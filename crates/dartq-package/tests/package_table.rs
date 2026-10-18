//! Table-level pins over the public composition API: `packageUri` layout,
//! `languageVersion` provenance, and
//! [`PackageTable::containing`] as the engine's file→package attribution
//! entry point.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use dartq_package::{PackageTable, SourceCategory};
use dartq_pub::{LockFile, PackageConfig, Pubspec, build_graph};

const CONFIG: &str = r#"{
  "configVersion": 2,
  "packages": [
    {"name": "app", "rootUri": "../", "languageVersion": "3.5"},
    {"name": "local_dep", "rootUri": "../deps/local_dep", "languageVersion": "3.5"},
    {
      "name": "hosted_dep",
      "rootUri": "file:///cache/hosted/pub.dev/hosted_dep-1.4.0",
      "packageUri": "src/",
      "languageVersion": "3.2"
    }
  ],
  "generator": "pub"
}"#;

const LOCK: &str = "\
packages:
  hosted_dep:
    dependency: \"direct main\"
    description:
      name: hosted_dep
      url: \"https://pub.dev\"
      sha256: \"aa\"
    source: hosted
    version: \"1.4.0\"
sdks:
  dart: \">=3.5.0 <4.0.0\"
";

fn root_pubspec() -> Pubspec {
    Pubspec::parse(
        "\
name: app
version: 0.1.0
environment:
  sdk: ^3.5.0
dependencies:
  hosted_dep: ^1.0.0
  local_dep:
    path: deps/local_dep
",
    )
    .expect("valid pubspec")
}

fn extra_pubspecs() -> BTreeMap<String, Pubspec> {
    let local = Pubspec::parse("name: local_dep\nversion: 1.0.0\n").expect("valid pubspec");
    BTreeMap::from([("local_dep".to_owned(), local)])
}

fn table_for(config_text: &str) -> PackageTable {
    let config = PackageConfig::parse(config_text).expect("valid config");
    let lock = LockFile::parse(LOCK).expect("valid lock");
    let graph = build_graph(
        &config,
        &lock,
        &root_pubspec(),
        &extra_pubspecs(),
        Some(&["local_dep".to_owned()]),
        Path::new("/proj/.dart_tool"),
    )
    .expect("graph");
    PackageTable::build(&config, &graph, Path::new("/proj/.dart_tool")).expect("table")
}

#[test]
fn custom_package_uri_lands_on_lib_dir() {
    let table = table_for(CONFIG);

    // The entry's `packageUri: src/` (slash-normalized by the parser)
    // decides the source directory — never a hardcoded `lib/`.
    let hosted = table.get("hosted_dep").expect("hosted entry");
    assert_eq!(
        hosted.lib_dir,
        PathBuf::from("/cache/hosted/pub.dev/hosted_dep-1.4.0/src")
    );
    assert_eq!(hosted.category, SourceCategory::PubCache);

    // Entries without `packageUri` anchor the source root at `rootUri`
    // itself — the reader contract (package_config_json.dart:152), never
    // a `lib/` default.
    let app = table.get("app").expect("root entry");
    assert_eq!(app.lib_dir, PathBuf::from("/proj"));
}

#[test]
fn config_entry_language_version_flows_to_identity() {
    // priority 2: an explicit per-entry `languageVersion` is the
    // version source — taken verbatim per entry, independent of the lock's
    // `sdks:` constraint (`>=3.5.0`).
    let table = table_for(CONFIG);
    let app = table.get("app").expect("root entry");
    assert_eq!(
        app.language_version.map(|v| v.to_string()).as_deref(),
        Some("3.5")
    );
    let hosted = table.get("hosted_dep").expect("hosted entry");
    assert_eq!(
        hosted.language_version.map(|v| v.to_string()).as_deref(),
        Some("3.2")
    );
}

#[test]
fn containing_prefers_the_deepest_nested_root() {
    let table = table_for(CONFIG);

    // `local_dep`'s root sits inside the root package's tree: a file under
    // it attributes to `local_dep`, not to the root package at `/proj`.
    let nested = table
        .containing(Path::new("/proj/deps/local_dep/lib/main.dart"))
        .expect("nested dep file");
    assert_eq!(nested.name.as_str(), "local_dep");

    // Root-package files attribute to the root package.
    let own = table
        .containing(Path::new("/proj/lib/main.dart"))
        .expect("root file");
    assert_eq!(own.name.as_str(), "app");

    // The root directory itself attributes to its own package.
    let at_root = table.containing(Path::new("/proj")).expect("root dir");
    assert_eq!(at_root.name.as_str(), "app");
}

#[test]
fn containing_returns_none_outside_and_across_boundaries() {
    let table = table_for(CONFIG);

    // No package root contains the path.
    assert!(
        table
            .containing(Path::new("/elsewhere/lib/main.dart"))
            .is_none()
    );

    // Component-boundary semantics: `/projx` is not inside `/proj`.
    assert!(
        table
            .containing(Path::new("/projx/lib/main.dart"))
            .is_none()
    );
}
