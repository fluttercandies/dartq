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
fn flutter_root_is_rejected_until_embedder_support_lands() {
    let tree = TempTree::new(&standard_files());
    let flutter_root = tree.root.join("flutter");
    std::fs::create_dir_all(&flutter_root).expect("flutter root created");
    match SdkSnapshot::load(&tree.dart_sdk(), Some(&flutter_root)) {
        Err(SdkError::FlutterUnsupported { root }) => assert_eq!(root, flutter_root),
        other => panic!("expected FlutterUnsupported, got {other:?}"),
    }
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
