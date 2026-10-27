//! Full-load integration tests over synthetic SDK layouts on disk.
//!
//! Parse-level semantics are covered in `src/loader/tests.rs`; here the
//! whole `load` pipeline runs against complete directory fixtures, including
//! every `SdkError` path and the canonical identity rules.

use std::sync::atomic::{AtomicU32, Ordering};

use dartq_language::{Feature, FeatureSet, LanguageVersion};
use dartq_sdk::{Importability, SdkError, SdkSnapshot, SdkUriError};

const LIBRARIES_JSON: &str = r#"{
  "comment:0": "generated",
  "base": {
    "libraries": {
      "core": {"uri": "core/core.dart", "patches": "_internal/vm/lib/array_patch.dart"},
      "io": {"uri": "io/io.dart"},
      "_internal": {"uri": "internal/internal.dart"}
    }
  },
  "web": {
    "libraries": {
      "html": {"uri": "html/html_dart2js.dart", "support_direct_import": "with_flag"}
    }
  },
  "vm": {
    "include": [{"target": "base"}],
    "libraries": {"cli": {"uri": "cli/cli.dart"}}
  }
}"#;

const ALLOWED_EXPERIMENTS_JSON: &str = r#"{
  "version": 1,
  "experimentSets": {"sdkExperiments": [], "macros": ["enhanced-parts", "macros"]},
  "sdk": {"default": {"experimentSet": "sdkExperiments"}},
  "packages": {"json": {"experimentSet": "macros"}}
}"#;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A throwaway SDK directory tree, removed on drop.
struct TempTree {
    root: std::path::PathBuf,
}

impl TempTree {
    fn new(files: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!(
            "dartq-sdk-load-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp root created");
        for (relative, content) in files {
            Self::write(&root, relative, content);
        }
        Self { root }
    }

    fn write(root: &std::path::Path, relative: &str, content: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("relative path has a parent"))
            .expect("fixture directory created");
        std::fs::write(path, content).expect("fixture file written");
    }

    /// Overwrites one fixture file (used to derive broken variants of the
    /// standard layout without rebuilding the whole file list).
    fn rewrite(&self, relative: &str, content: &str) {
        Self::write(&self.root, relative, content);
    }

    /// The dart-sdk directory of the standard layout.
    fn dart_sdk(&self) -> std::path::PathBuf {
        self.root.join("dart-sdk")
    }

    fn load(&self) -> SdkSnapshot {
        SdkSnapshot::load(&self.dart_sdk(), None).expect("fixture SDK loads")
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A complete, valid synthetic SDK: the four metadata files plus library
/// root files for every entry in [`LIBRARIES_JSON`].
fn standard_files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("dart-sdk/version", "3.13.1\n"),
        ("dart-sdk/revision", "852b3e36abc\n"),
        ("dart-sdk/lib/libraries.json", LIBRARIES_JSON),
        (
            "dart-sdk/lib/_internal/allowed_experiments.json",
            ALLOWED_EXPERIMENTS_JSON,
        ),
        ("dart-sdk/lib/core/core.dart", "library core;"),
        ("dart-sdk/lib/core/bool.dart", "part of core;"),
        ("dart-sdk/lib/io/io.dart", "library io;"),
        (
            "dart-sdk/lib/internal/internal.dart",
            "library dart._internal;",
        ),
        ("dart-sdk/lib/cli/cli.dart", "library cli;"),
        ("dart-sdk/lib/html/html_dart2js.dart", "library html;"),
    ]
}

#[test]
fn loads_metadata_resolves_targets_and_uris() {
    let tree = TempTree::new(&standard_files());
    let sdk = tree.load();

    assert_eq!(sdk.dart.version, "3.13.1");
    assert_eq!(sdk.dart.revision.as_deref(), Some("852b3e36abc"));
    assert_eq!(sdk.language_metadata, LanguageVersion::new(3, 13));

    // Include expansion: vm = base ∪ {cli}; comment keys are not segments.
    let vm = sdk.target("vm").expect("vm segment");
    assert_eq!(vm.libraries.len(), 4);
    // String patches normalize to a one-element list.
    assert_eq!(
        vm.libraries["core"].patches,
        vec!["_internal/vm/lib/array_patch.dart"]
    );

    // Library root resolution lands on the real fixture file.
    let core = sdk
        .uri_to_path("vm", "dart:core")
        .expect("dart:core resolves");
    assert_eq!(core, tree.dart_sdk().join("lib/core/core.dart"));
    assert!(core.is_file(), "library root exists on disk");

    // Part files join relative to the library's directory.
    let bool_part = sdk
        .uri_to_path("vm", "dart:core/bool.dart")
        .expect("part resolves");
    assert_eq!(bool_part, tree.dart_sdk().join("lib/core/bool.dart"));
    assert!(bool_part.is_file());

    // A trailing slash means the library root, like the analyzer.
    assert_eq!(
        sdk.uri_to_path("vm", "dart:core/").expect("trailing slash"),
        core
    );
}

#[test]
fn uri_errors_are_explicit() {
    let tree = TempTree::new(&standard_files());
    let sdk = tree.load();

    assert_eq!(
        sdk.uri_to_path("vm", "package:core/core.dart"),
        Err(SdkUriError::NotDartUri {
            uri: "package:core/core.dart".to_owned()
        })
    );
    assert_eq!(
        sdk.uri_to_path("ghost", "dart:core"),
        Err(SdkUriError::UnknownTarget {
            target: "ghost".to_owned()
        })
    );
    // Target-scoped: `dart:html` exists only in the web segment.
    assert_eq!(
        sdk.uri_to_path("vm", "dart:html"),
        Err(SdkUriError::UnknownLibrary {
            target: "vm".to_owned(),
            library: "html".to_owned()
        })
    );
    let html = sdk
        .uri_to_path("web", "dart:html")
        .expect("web resolves html");
    assert_eq!(html, tree.dart_sdk().join("lib/html/html_dart2js.dart"));
    assert_eq!(
        sdk.uri_to_path("web", "dart:"),
        Err(SdkUriError::UnknownLibrary {
            target: "web".to_owned(),
            library: String::new()
        })
    );
}

#[test]
fn flat_existence_face_resolves_across_segments() {
    // The fixture's `vm` segment lists only the base set plus `cli` — the
    // flat table (generated from the oracle's `libraries.dart`) resolves
    // past segment boundaries, matching the oracle's URI-existence posture:
    // no platform flags, no segment filter.
    let tree = TempTree::new(&standard_files());
    let sdk = tree.load();

    let html = sdk
        .flat_library_to_path("html", "")
        .expect("html is in the flat table");
    assert_eq!(
        html,
        tree.dart_sdk().join("lib/html/dart2js/html_dart2js.dart")
    );

    // Parts join the flat entry's path the analyzer's way.
    let part = sdk
        .flat_library_to_path("core", "bool.dart")
        .expect("core is in the flat table");
    assert_eq!(part, tree.dart_sdk().join("lib/core/bool.dart"));

    // The ten web libraries the `vm` segment lacks all resolve; the
    // VM-internal names the oracle's table lacks do not.
    for name in [
        "async",
        "collection",
        "concurrent",
        "convert",
        "core",
        "developer",
        "ffi",
        "html",
        "html_common",
        "indexed_db",
        "io",
        "isolate",
        "js",
        "js_interop",
        "js_interop_unsafe",
        "js_util",
        "math",
        "mirrors",
        "svg",
        "typed_data",
        "web_audio",
        "web_gl",
    ] {
        assert!(
            sdk.flat_library_to_path(name, "").is_some(),
            "dart:{name} must exist on the flat face"
        );
    }
    assert_eq!(
        sdk.flat_library_to_path("_builtin", ""),
        None,
        "VM-internal name outside the oracle's table"
    );
    assert_eq!(
        sdk.flat_library_to_path("web_sql", ""),
        None,
        "on disk in real SDK trees, tabulated by neither side"
    );
}

#[test]
fn support_values_follow_segment_and_gates() {
    let tree = TempTree::new(&standard_files());
    let sdk = tree.load();

    // vm has no html; the web segment's html still participates in
    // conditional imports despite its direct-import gate.
    assert!(!sdk.library_support_value("vm", "html"));
    assert!(sdk.library_support_value("web", "html"));
    assert!(sdk.library_support_value("vm", "io"));
    // `_`-prefixed names are forced off at parse time.
    assert!(!sdk.library_support_value("vm", "_internal"));
    // Unknown target / library simply have no support.
    assert!(!sdk.library_support_value("ghost", "core"));
    assert_eq!(
        sdk.target("web").expect("web segment").libraries["html"].support_direct_import,
        Importability::WithFlag
    );
}

#[test]
fn identity_is_canonical_over_parsed_content() {
    let first = TempTree::new(&standard_files());
    let sdk_a = first.load();

    // Same parsed content in a different raw layout (whitespace, patches as
    // array instead of string) must share the canonical id while the raw
    // hash differs.
    let second = TempTree::new(&standard_files());
    second.rewrite(
        "dart-sdk/lib/libraries.json",
        &LIBRARIES_JSON.replace(
            r#""patches": "_internal/vm/lib/array_patch.dart""#,
            r#" "patches" : ["_internal/vm/lib/array_patch.dart"] "#,
        ),
    );
    let sdk_b = second.load();
    assert_eq!(sdk_a.id, sdk_b.id, "equal parses share one id");
    assert_ne!(
        sdk_a.dart.libraries_json_hash, sdk_b.dart.libraries_json_hash,
        "raw-byte change stays visible in SdkIdentity"
    );

    // A semantic change (different library uri) must fork the id.
    let third = TempTree::new(&standard_files());
    third.rewrite(
        "dart-sdk/lib/libraries.json",
        &LIBRARIES_JSON.replace("core/core.dart", "core/core_impl.dart"),
    );
    let sdk_c = third.load();
    assert_ne!(sdk_a.id, sdk_c.id, "semantic content change forks the id");

    // Flipping a support gate is a semantic change too: the id forks.
    let fourth = TempTree::new(&standard_files());
    fourth.rewrite(
        "dart-sdk/lib/libraries.json",
        &LIBRARIES_JSON.replace(
            r#""support_direct_import": "with_flag""#,
            r#""support_direct_import": "never""#,
        ),
    );
    let sdk_d = fourth.load();
    assert_ne!(sdk_a.id, sdk_d.id, "support-gate change forks the id");

    // A version-annotation-only change (`3.13.1` vs `3.13.1 (stable)`)
    // forks the id via SnapshotFingerprint.dart_version while the derived
    // language version stays 3.13.
    let fifth = TempTree::new(&standard_files());
    fifth.rewrite("dart-sdk/version", "3.13.1 (stable)\n");
    let sdk_e = fifth.load();
    assert_eq!(sdk_e.language_metadata, sdk_a.language_metadata);
    assert_ne!(
        sdk_a.id, sdk_e.id,
        "the declared version text participates in the id"
    );
}

#[test]
fn version_file_trailing_annotation_is_tolerated() {
    // Real SDK version files can carry a release annotation; the official
    // reader consumes only the leading numeric parts (sdk.dart:25-30), so
    // `3.13.1 (stable) …` loads as 3.13.
    let tree = TempTree::new(&standard_files());
    tree.rewrite("dart-sdk/version", "3.13.1 (stable)\n");
    let sdk = tree.load();
    assert_eq!(sdk.language_metadata, LanguageVersion::new(3, 13));
    // The identity keeps the file's declared text verbatim.
    assert_eq!(sdk.dart.version, "3.13.1 (stable)");
}

#[test]
fn load_error_paths_are_distinct() {
    // Missing required file.
    let tree = missing_file_case("dart-sdk/version");
    assert!(matches!(
        SdkSnapshot::load(&tree.dart_sdk(), None),
        Err(SdkError::MissingFile { .. })
    ));

    // Unparseable version.
    let tree = TempTree::new(&standard_files());
    tree.rewrite("dart-sdk/version", "three.three\n");
    assert!(matches!(
        SdkSnapshot::load(&tree.dart_sdk(), None),
        Err(SdkError::InvalidVersion { .. })
    ));

    // Broken JSON.
    let tree = TempTree::new(&standard_files());
    tree.rewrite("dart-sdk/lib/libraries.json", "{ not json");
    assert!(matches!(
        SdkSnapshot::load(&tree.dart_sdk(), None),
        Err(SdkError::InvalidLibrariesJson { .. })
    ));

    // Valid JSON, no library anywhere: no usable dart: surface.
    let tree = TempTree::new(&standard_files());
    tree.rewrite(
        "dart-sdk/lib/libraries.json",
        r#"{"none": {"libraries": {}}, "comment:0": "generated"}"#,
    );
    assert!(matches!(
        SdkSnapshot::load(&tree.dart_sdk(), None),
        Err(SdkError::EmptyLibraries { .. })
    ));

    // Malformed allowed_experiments.json is a hard error, never the
    // analyzer's silent all-experiments-off degradation.
    let tree = TempTree::new(&standard_files());
    tree.rewrite(
        "dart-sdk/lib/_internal/allowed_experiments.json",
        r#"{"version": 2}"#,
    );
    assert!(matches!(
        SdkSnapshot::load(&tree.dart_sdk(), None),
        Err(SdkError::InvalidAllowedExperiments { .. })
    ));

    // A missing allowed_experiments.json is a missing required file.
    let tree = missing_file_case("dart-sdk/lib/_internal/allowed_experiments.json");
    assert!(matches!(
        SdkSnapshot::load(&tree.dart_sdk(), None),
        Err(SdkError::MissingFile { .. })
    ));
}

/// The standard layout minus one file.
fn missing_file_case(relative: &str) -> TempTree {
    let files: Vec<(&str, &str)> = standard_files()
        .into_iter()
        .filter(|(path, _)| *path != relative)
        .collect();
    TempTree::new(&files)
}

#[test]
fn embedder_dir_without_yaml_is_missing_file() {
    let tree = TempTree::new(&standard_files());
    let embedder_dir = tree.root.join("no-yaml-here");
    std::fs::create_dir_all(&embedder_dir).expect("embedder dir created");
    // A directory without `_embedder.yaml` is the structured missing-file
    // error, never a panic.
    match SdkSnapshot::load(&tree.dart_sdk(), Some(&embedder_dir)) {
        Err(SdkError::MissingFile { .. }) => {}
        other => panic!("expected MissingFile, got {other:?}"),
    }
}

/// The sky_engine-shaped embedder package over the standard fixture:
/// `dart:core` overridden onto a different file, `dart:ui` added, one
/// `../`-climbing path, one non-`dart:` key (ignored), one extra top-level
/// section (ignored).
const EMBEDDER_YAML: &str = r#"# generated
embedded_libs:
  "dart:core": "embedder_core/core.dart"
  "dart:ui": "ui/ui.dart"
  "dart:nativewrappers": "../../shared/nativewrappers.dart"
  "not-a-dart-uri": "ignored.dart"
embedded_server_libraries:
  key: value
"#;

fn embedder_files() -> Vec<(&'static str, &'static str)> {
    let mut files = standard_files();
    files.extend([
        ("sky_engine/lib/_embedder.yaml", EMBEDDER_YAML),
        ("sky_engine/lib/embedder_core/core.dart", "library core;"),
        ("sky_engine/lib/ui/ui.dart", "library ui;"),
        ("shared/nativewrappers.dart", "library nativewrappers;"),
    ]);
    files
}

fn sky_engine_lib(tree: &TempTree) -> std::path::PathBuf {
    tree.root.join("sky_engine").join("lib")
}

fn load_embedder(tree: &TempTree) -> SdkSnapshot {
    SdkSnapshot::load(&tree.dart_sdk(), Some(&sky_engine_lib(tree)))
        .expect("embedder-backed SDK loads")
}

#[test]
fn embedder_table_replaces_every_resolution_face() {
    let tree = TempTree::new(&embedder_files());
    let sdk = load_embedder(&tree);

    // Full replacement, not an overlay: `dart:core` resolves onto the
    // embedder's file for EVERY segment — the plain `libraries.json` table
    // (whose vm segment maps core → core/core.dart) is never consulted.
    let core = sdk
        .uri_to_path("vm", "dart:core")
        .expect("core resolves through the embedder");
    assert_eq!(core, sky_engine_lib(&tree).join("embedder_core/core.dart"));
    assert!(core.is_file());
    // The embedder's own additions resolve, including on segments the
    // plain table lacks entirely.
    let ui = sdk
        .uri_to_path("ghost-segment", "dart:ui")
        .expect("the embedder answers every segment");
    assert_eq!(ui, sky_engine_lib(&tree).join("ui/ui.dart"));
    // Part files join the embedder library's directory.
    assert_eq!(
        sdk.uri_to_path("vm", "dart:ui/painting.dart").expect("part"),
        sky_engine_lib(&tree).join("ui/painting.dart")
    );
    // The flat existence face answers from the replacement table too —
    // `dart:web_sql` exists in neither table, `dart:ui` only in the
    // embedder's.
    assert_eq!(
        sdk.flat_library_to_path("ui", ""),
        Some(sky_engine_lib(&tree).join("ui/ui.dart"))
    );
    assert_eq!(sdk.flat_library_to_path("web_sql", ""), None);
    // Every embedder entry answers `dart.library.<name>` true.
    assert!(sdk.library_support_value("vm", "ui"));
    assert!(sdk.library_support_value("vm", "nativewrappers"));
    // Version and experiments stay the dart-sdk's faces.
    assert_eq!(sdk.dart.version, "3.13.1");
    assert_eq!(sdk.language_metadata, LanguageVersion::new(3, 13));
    assert!(sdk.platform_metadata.for_sdk_library("core").is_empty());
}

#[test]
fn embedder_paths_climb_relative_segments() {
    let tree = TempTree::new(&embedder_files());
    let sdk = load_embedder(&tree);
    // `../` climbs resolve against the yaml's directory, the analyzer's
    // `libDir.canonicalizePath` posture. The composed path keeps the climb
    // lexical (analyzer string form); the filesystem resolves it on open —
    // the same file as the sibling-directory target.
    let climbed = sdk
        .flat_library_to_path("nativewrappers", "")
        .expect("climbing entry resolves");
    assert!(climbed.is_file(), "the climb resolves on disk");
    assert_eq!(
        climbed.canonicalize().expect("climb canonicalizes"),
        tree.root
            .join("shared")
            .join("nativewrappers.dart")
            .canonicalize()
            .expect("target canonicalizes")
    );
}

#[test]
fn embedder_snapshots_fork_identity() {
    let plain_tree = TempTree::new(&standard_files());
    let plain = plain_tree.load();
    let embedder_tree = TempTree::new(&embedder_files());
    let embedder = load_embedder(&embedder_tree);
    assert_ne!(
        plain.id, embedder.id,
        "the replacement table forks the snapshot id"
    );
    // Same table, different install location: one id (the fingerprint
    // hashes parsed content, never paths).
    let moved_tree = TempTree::new(&embedder_files());
    let moved = load_embedder(&moved_tree);
    assert_eq!(embedder.id, moved.id);
    // A semantic change in the yaml forks it again.
    let changed_tree = TempTree::new(&embedder_files());
    changed_tree.rewrite(
        "sky_engine/lib/_embedder.yaml",
        &EMBEDDER_YAML.replace("ui/ui.dart", "ui/ui_v2.dart"),
    );
    let changed = load_embedder(&changed_tree);
    assert_ne!(embedder.id, changed.id);
}

#[test]
fn embedder_yaml_load_errors_are_distinct() {
    // Unparsable yaml.
    let tree = TempTree::new(&embedder_files());
    tree.rewrite("sky_engine/lib/_embedder.yaml", "embedded_libs: [unclosed\n");
    assert!(matches!(
        load_embedder_result(&tree),
        Err(SdkError::InvalidEmbedderYaml { .. })
    ));

    // Parses, but no usable table.
    let tree = TempTree::new(&embedder_files());
    tree.rewrite("sky_engine/lib/_embedder.yaml", "embedded_libs: {}\n");
    assert!(matches!(
        load_embedder_result(&tree),
        Err(SdkError::InvalidEmbedderYaml { .. })
    ));

    // The yaml vanished between discovery and load (TOCTOU): the missing
    // file tier, not a panic.
    let tree = TempTree::new(&embedder_files());
    std::fs::remove_file(sky_engine_lib(&tree).join("_embedder.yaml")).expect("removed");
    assert!(matches!(
        load_embedder_result(&tree),
        Err(SdkError::MissingFile { .. })
    ));
}

fn load_embedder_result(tree: &TempTree) -> Result<SdkSnapshot, SdkError> {
    SdkSnapshot::load(&tree.dart_sdk(), Some(&sky_engine_lib(tree)))
}

#[test]
fn experiment_whitelist_maps_to_feature_sets() {
    let tree = TempTree::new(&standard_files());
    let sdk = tree.load();
    let version = sdk.language_metadata;
    let baseline = FeatureSet::for_language_version(version);

    // The fixture's default set is empty.
    assert_eq!(
        sdk.platform_metadata
            .feature_set_for_sdk_library("core", version)
            .unwrap(),
        baseline
    );
    // The `json` package grant turns on both macros experiments.
    let granted = sdk
        .platform_metadata
        .feature_set_for_package("json", version)
        .unwrap();
    assert!(granted.is_enabled(Feature::Macros));
    assert!(granted.is_enabled(Feature::EnhancedParts));
    assert!(!baseline.is_enabled(Feature::Macros));
}
