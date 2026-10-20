//! Deterministic integration tests over a vendored, trimmed copy of a real
//! Dart SDK's metadata (`tests/fixtures/minimal-vm-sdk`, cut from an fvm
//! dart-sdk 3.13.1 tree).
//!
//! Unlike `real_sdk.rs`, these run unconditionally on every machine — no
//! `~/fvm/default` probing — and pin exact expected values, so CI always
//! has SDK-level assertions. Error-path and parse-level semantics live in
//! `load.rs` and `src/loader/tests.rs`; this file anchors the *real* file
//! shapes (patch lists, include chains, target scoping, the
//! `allowed_experiments.json` grant layout).

use dartq_language::{DEFAULT_LANGUAGE_VERSION, Feature, FeatureSet, LanguageVersion};
use dartq_sdk::{Importability, SdkSnapshot, SdkUriError};

/// Loads the vendored fixture (the directory is the dart-sdk root).
fn vendored_sdk() -> SdkSnapshot {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/minimal-vm-sdk");
    SdkSnapshot::load(&root, None).expect("the vendored fixture SDK loads")
}

#[test]
fn loads_vendored_metadata_with_pinned_identity() {
    let sdk = vendored_sdk();

    assert_eq!(sdk.dart.version, "3.13.1");
    assert_eq!(
        sdk.dart.revision.as_deref(),
        Some("852b3e3608906afbe6102573cfd4407aeedd1b78")
    );
    assert_eq!(sdk.language_metadata, LanguageVersion::new(3, 13));

    // Exactly the segments written to the file: the `comment:*` generation
    // keys never become segments (BTreeMap iterates in sorted key order).
    // (The real file's `_example_libraries` doc key is not a libraries.json
    // key at all — it lives under `sdk.` in allowed_experiments.json, whose
    // parser correctly ignores it.)
    let targets = &sdk.sdk_libraries.targets;
    assert_eq!(
        targets.keys().map(String::as_str).collect::<Vec<_>>(),
        ["_dart2js_common", "dart2js", "none", "vm", "vm_common"]
    );
    assert_eq!(
        targets["none"].libraries.len(),
        0,
        "the real `none` segment is empty"
    );
    // Include expansion: vm = vm_common ∪ {cli}; dart2js = _dart2js_common ∪ {html}.
    assert_eq!(targets["vm"].libraries.len(), 6);
    assert_eq!(targets["dart2js"].libraries.len(), 3);

    // Loading twice yields one canonical id over the fixture content.
    assert_eq!(sdk.id, vendored_sdk().id);
}

#[test]
fn uri_resolution_is_pinned() {
    let sdk = vendored_sdk();
    let lib = sdk.dart_sdk_root().join("lib");

    // Library roots join `lib/` with the entry uri, not the library name.
    assert_eq!(
        sdk.uri_to_path("vm", "dart:core").expect("dart:core on vm"),
        lib.join("core/core.dart")
    );
    // Parts resolve next to the library root.
    assert_eq!(
        sdk.uri_to_path("vm", "dart:core/int.dart")
            .expect("core part"),
        lib.join("core/int.dart")
    );
    // Absolute-shaped parts mirror the analyzer's string concat: the
    // leading `/` collapses into a double slash instead of re-anchoring the
    // join (`Path::join` with an absolute part would REPLACE the base and
    // escape the SDK).
    assert_eq!(
        sdk.uri_to_path("vm", "dart:core//etc/passwd")
            .expect("absolute-shaped part"),
        lib.join("core/etc/passwd")
    );
    // An all-anchor part lands at the library root, never at `/`.
    assert_eq!(
        sdk.uri_to_path("vm", "dart:core//")
            .expect("bare-slash part"),
        lib.join("core")
    );
    // `..` climbs stay analyzer-equivalent: concat keeps them, so
    // `<lib>/core/../io/io.dart` is the returned shape.
    assert_eq!(
        sdk.uri_to_path("vm", "dart:core/../io/io.dart")
            .expect("parent-climbing part"),
        lib.join("core/../io/io.dart")
    );
    // dart:_internal's uri differs from its name.
    assert_eq!(
        sdk.uri_to_path("vm", "dart:_internal")
            .expect("dart:_internal on vm"),
        lib.join("internal/internal.dart")
    );
    // The dart2js segment carries its own html implementation uri; vm has
    // no html at all (the real scoping shape).
    assert_eq!(
        sdk.uri_to_path("dart2js", "dart:html")
            .expect("dart:html on dart2js"),
        lib.join("html/dart2js/html_dart2js.dart")
    );
    assert!(matches!(
        sdk.uri_to_path("vm", "dart:html"),
        Err(SdkUriError::UnknownLibrary { target, library })
            if target == "vm" && library == "html"
    ));
    assert_eq!(
        sdk.uri_to_path("ghost", "dart:core"),
        Err(SdkUriError::UnknownTarget {
            target: "ghost".to_owned()
        })
    );
}

#[test]
fn structural_library_to_path_matches_the_string_form() {
    let sdk = vendored_sdk();
    let lib = sdk.dart_sdk_root().join("lib");

    // The canonical join the string face parses back. The contract: for
    // every input, `library_to_path(segment, library, part)` equals
    // `uri_to_path(segment, join(library, part)).ok()`.
    let join = |library: &str, part: &str| {
        if part.is_empty() {
            format!("dart:{library}")
        } else {
            format!("dart:{library}/{part}")
        }
    };

    // Edge-shape table: (segment, library, part) against its string-form
    // spelling — the pair splits exactly the way the string face (and the
    // resolution composer) splits that spelling at its first `/`.
    let rows: &[(&str, &str, &str, String)] = &[
        ("vm", "core", "", join("core", "")), // library root
        ("vm", "core", "int.dart", join("core", "int.dart")), // plain part
        ("vm", "core", "/", join("core", "/")), // all-anchor part
        ("vm", "core", "/etc/passwd", join("core", "/etc/passwd")), // absolute-shaped part
        ("vm", "core", "../io/io.dart", join("core", "../io/io.dart")), // `..` climb kept
        ("vm", "core", "a%20b.dart", join("core", "a%20b.dart")), // escapes stay intact
        ("vm", "_internal", "", join("_internal", "")), // uri differs from name
        ("dart2js", "html", "", join("html", "")), // segment scoping
        ("vm", "html", "", join("html", "")), // unknown library
        ("ghost", "core", "int.dart", join("core", "int.dart")), // unknown target
    ];
    for (segment, library, part, spelling) in rows {
        assert_eq!(
            sdk.library_to_path(segment, library, part),
            sdk.uri_to_path(segment, spelling).ok(),
            "structural != string for ({segment}, {library}, {part})"
        );
    }

    // Spot pin that the table rows hit the real lookups: `_internal`'s
    // entry uri differs from its name.
    assert_eq!(
        sdk.library_to_path("vm", "_internal", ""),
        Some(lib.join("internal/internal.dart"))
    );

    // Authority form: `dart://x/core` yields the pair `("", "core")` — the
    // empty first segment of the path. No SDK table lists an empty library,
    // on either face, under either spelling that maps to the pair.
    assert_eq!(sdk.library_to_path("vm", "", "core"), None);
    assert_eq!(sdk.uri_to_path("vm", "dart://x/core").ok(), None);
    assert_eq!(sdk.uri_to_path("vm", &join("", "core")).ok(), None);

    // Bare scheme: empty everything, unknown on both faces.
    assert_eq!(sdk.library_to_path("vm", "", ""), None);
    assert_eq!(sdk.uri_to_path("vm", "dart:").ok(), None);
}

#[test]
fn patch_and_support_values_match_the_real_file() {
    let sdk = vendored_sdk();

    // The full vm patch list of dart:core, verbatim from the real SDK.
    assert_eq!(
        sdk.target("vm").expect("vm segment").libraries["core"].patches,
        vec![
            "_internal/vm/lib/array_patch.dart",
            "_internal/vm/lib/core_patch.dart",
            "_internal/vm_shared/lib/bigint_patch.dart",
            "_internal/vm/lib/bigint_patch_patch.dart",
            "_internal/vm_shared/lib/bool_patch.dart",
            "_internal/vm_shared/lib/date_patch.dart",
            "_internal/vm_shared/lib/integers_patch.dart",
            "_internal/vm_shared/lib/map_patch.dart",
            "_internal/vm_shared/lib/null_patch.dart",
            "_internal/vm_shared/lib/string_buffer_patch.dart",
            "_internal/vm_shared/lib/uri_encode_patch.dart",
        ]
    );
    // The real file writes math's single patch as a bare string; it
    // normalizes to a one-element list.
    assert_eq!(
        sdk.target("vm").expect("vm segment").libraries["math"].patches,
        vec!["_internal/vm/lib/math_patch.dart"]
    );
    // Patches stay segment-scoped: dart2js core carries the js-runtime
    // patches, not the vm ones.
    assert_eq!(
        sdk.target("dart2js").expect("dart2js segment").libraries["core"].patches,
        vec![
            "_internal/js_shared/lib/date_time_patch.dart",
            "_internal/js_runtime/lib/core_patch.dart",
        ]
    );

    // Conditional-import matrix: public vm libraries are on; `_`-prefixed
    // internal ones never are; vm-absent html is off; dart2js io is gated
    // off by its real `support_conditional_import: false` entry.
    for name in ["async", "cli", "core", "io", "math"] {
        assert!(
            sdk.library_support_value("vm", name),
            "dart.library.{name} on vm"
        );
    }
    assert!(!sdk.library_support_value("vm", "_internal"));
    assert!(!sdk.library_support_value("vm", "html"));
    assert!(!sdk.library_support_value("dart2js", "io"));
    assert!(sdk.library_support_value("dart2js", "html"));

    // cli has no gate in the real file: direct import defaults to always.
    assert_eq!(
        sdk.target("vm").expect("vm segment").libraries["cli"].support_direct_import,
        Importability::Always
    );
}

#[test]
fn allowed_experiments_map_to_feature_sets() {
    let sdk = vendored_sdk();
    let version = sdk.language_metadata;
    let baseline = FeatureSet::for_language_version(version);

    // The real file ships an empty default set (`sdkExperiments`), so every
    // SDK library sits on the pure version baseline.
    assert!(sdk.platform_metadata.for_sdk_library("core").is_empty());
    for library in ["async", "core", "html", "io"] {
        assert_eq!(
            sdk.platform_metadata
                .feature_set_for_sdk_library(library, version)
                .expect("feature set"),
            baseline,
            "{library} maps to the version baseline"
        );
    }
    // The real `json` package grant carries both macros experiments
    // (macros: release 3.3, enhanced-parts: release 3.6), so each grant
    // sticks only where the official release gate passes at the effective
    // language version.
    let json = |v: LanguageVersion| {
        sdk.platform_metadata
            .feature_set_for_package("json", v)
            .expect("feature set")
    };

    // At the SDK's own version (3.13) both releases have shipped: both on.
    let granted = json(version);
    assert!(granted.is_enabled(Feature::Macros));
    assert!(granted.is_enabled(Feature::EnhancedParts));
    assert!(!baseline.is_enabled(Feature::Macros));

    // Below both releases (package-config language 2.19) the grant is a
    // no-op: the result is the pure 2.19 baseline.
    let old_version = LanguageVersion::new(2, 19);
    let old = json(old_version);
    assert!(!old.is_enabled(Feature::Macros));
    assert!(!old.is_enabled(Feature::EnhancedParts));
    assert_eq!(old, FeatureSet::for_language_version(old_version));

    // At the current SDK version both gates pass again.
    let current = json(DEFAULT_LANGUAGE_VERSION);
    assert!(current.is_enabled(Feature::Macros));
    assert!(current.is_enabled(Feature::EnhancedParts));
}

/// The deprecated-`dart:`-library face: the frozen table mirrors every
/// library-level `@Deprecated`/`@deprecated` annotation in the oracle SDK's
/// `lib/` tree (3.13.1), name → raw annotation message (`""` for the bare
/// `@deprecated`). The lookup ignores snapshot state — every SDK ships the
/// same library files — so the pin asserts the table itself plus the
/// query's hit/miss arms.
#[test]
fn deprecated_library_table_matches_the_sdk_annotations() {
    let expected: &[(&str, &str)] = &[
        (
            "cli",
            "The functionality of this library is incomplete and may be removed \
             in a later version",
        ),
        ("html", "Use package:web and dart:js_interop instead."),
        ("indexed_db", "Use package:web and dart:js_interop instead."),
        ("js", "Use dart:js_interop instead."),
        ("js_util", "Use dart:js_interop instead."),
        ("svg", "Use package:web and dart:js_interop instead."),
        ("web_audio", "Use package:web and dart:js_interop instead."),
        ("web_gl", "Use package:web and dart:js_interop instead."),
        ("web_sql", ""),
    ];
    assert_eq!(dartq_sdk::DEPRECATED_SDK_LIBRARIES, expected);

    for &(name, message) in expected {
        assert_eq!(
            dartq_sdk::deprecated_library_message(name),
            Some(message),
            "{name}"
        );
    }
    // Member-level deprecation only (`io`), an ordinary library (`core`),
    // an `_internal` helper, and an unknown name all miss.
    for name in ["io", "core", "_internal", "no_such_library"] {
        assert_eq!(dartq_sdk::deprecated_library_message(name), None, "{name}");
    }
}
