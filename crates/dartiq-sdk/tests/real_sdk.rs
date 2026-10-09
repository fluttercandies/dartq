//! Smoke tests against the real Dart SDK on this machine:
//! `~/fvm/default` (an fvm flutter checkout whose dart side lives at
//! `bin/cache/dart-sdk`, dart 3.13.1 at the time of writing).
//!
//! The tests skip silently when the SDK is absent, so the suite stays green
//! on machines without it; every assertion that depends on SDK content is
//! derived from the loaded metadata (version numbers are not pinned).

use dartiq_language::{Feature, FeatureSet, LanguageVersion};
use dartiq_sdk::SdkSnapshot;

/// `<flutter>/bin/cache/dart-sdk` of the fvm default version.
fn real_dart_sdk() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    let dart_sdk = std::path::Path::new(&home)
        .join("fvm")
        .join("default")
        .join("bin")
        .join("cache")
        .join("dart-sdk");
    dart_sdk.is_dir().then_some(dart_sdk)
}

#[test]
fn loads_real_sdk_and_resolves_dart_uris() {
    let Some(dart_sdk_root) = real_dart_sdk() else {
        eprintln!("skipped: ~/fvm/default/bin/cache/dart-sdk not present");
        return;
    };
    let sdk = SdkSnapshot::load(&dart_sdk_root, None).expect("the real SDK loads");

    // Identity matches the files on disk; the version string is whatever
    // this SDK ships (3.13.1 at the time of writing).
    let version_on_disk = std::fs::read_to_string(dart_sdk_root.join("version"))
        .expect("version readable")
        .trim()
        .to_owned();
    assert_eq!(sdk.dart.version, version_on_disk);
    assert!(sdk.language_metadata.major >= 3, "sane major: {sdk:?}");
    eprintln!(
        "real SDK: dart {version_on_disk}, language {}, id {}",
        sdk.dart.version, sdk.id
    );

    // dart:core resolves to the real library root and round-trips to an
    // existing file.
    let core = sdk.uri_to_path("vm", "dart:core").expect("dart:core on vm");
    assert!(core.is_file(), "core root exists: {}", core.display());
    assert_eq!(core.file_name().and_then(|n| n.to_str()), Some("core.dart"));

    // Part resolution: dart:core parts are real files next to the root.
    let int_part = sdk
        .uri_to_path("vm", "dart:core/int.dart")
        .expect("dart:core/int.dart");
    assert!(
        int_part.is_file(),
        "core part exists: {}",
        int_part.display()
    );

    // dart:_internal resolves (its uri differs from the library name).
    let internal = sdk
        .uri_to_path("vm", "dart:_internal")
        .expect("dart:_internal on vm");
    assert!(
        internal.is_file(),
        "internal root exists: {}",
        internal.display()
    );

    // Target-scoping: dart:html does not exist on vm, and on dart2js the
    // segment's own (dart2js-flavored) uri resolves.
    assert!(matches!(
        sdk.uri_to_path("vm", "dart:html"),
        Err(dartiq_sdk::SdkUriError::UnknownLibrary { library, .. }) if library == "html"
    ));
    let html = sdk
        .uri_to_path("dart2js", "dart:html")
        .expect("dart:html on dart2js");
    assert!(
        html.is_file(),
        "dart2js html root exists: {}",
        html.display()
    );
    assert!(
        html.to_string_lossy().ends_with("html_dart2js.dart"),
        "dart2js segment carries its own implementation uri"
    );

    // vm patches survive parsing (core carries the vm patch list).
    let vm = sdk.target("vm").expect("vm segment");
    assert_eq!(
        vm.libraries["core"].patches.first().map(String::as_str),
        Some("_internal/vm/lib/array_patch.dart")
    );
}

#[test]
fn real_sdk_conditional_import_matrix() {
    let Some(dart_sdk_root) = real_dart_sdk() else {
        eprintln!("skipped: ~/fvm/default/bin/cache/dart-sdk not present");
        return;
    };
    let sdk = SdkSnapshot::load(&dart_sdk_root, None).expect("the real SDK loads");

    // vm: public platform libraries are supported; `_`-prefixed internal
    // ones never are; web-only libraries are absent from the vm table.
    for name in [
        "async",
        "cli",
        "collection",
        "convert",
        "core",
        "io",
        "isolate",
        "math",
    ] {
        assert!(
            sdk.library_support_value("vm", name),
            "dart.library.{name} on vm"
        );
    }
    for name in ["_internal", "_builtin", "_vm"] {
        assert!(
            !sdk.library_support_value("vm", name),
            "internal {name} never supported"
        );
    }
    assert!(!sdk.library_support_value("vm", "html"));

    // dartdevc gates io (support_conditional_import: false in the file).
    assert!(!sdk.library_support_value("dartdevc", "io"));
    assert!(sdk.library_support_value("dartdevc", "html"));
    // dartdevc's ffi is with_flag-gated and conditionally unsupported.
    let ffi = &sdk.target("dartdevc").expect("dartdevc segment").libraries["ffi"];
    assert_eq!(
        ffi.support_direct_import,
        dartiq_sdk::Importability::WithFlag
    );
    assert!(!ffi.support_conditional_import);
}

#[test]
fn real_sdk_experiment_whitelist() {
    let Some(dart_sdk_root) = real_dart_sdk() else {
        eprintln!("skipped: ~/fvm/default/bin/cache/dart-sdk not present");
        return;
    };
    let sdk = SdkSnapshot::load(&dart_sdk_root, None).expect("the real SDK loads");

    // Every current SDK ships the same file: empty default set, the macros
    // set reserved for the `json` package. So every SDK library maps to the
    // pure version baseline, and unknown SDK libraries fall back to it too.
    let version = sdk.language_metadata;
    let baseline = FeatureSet::for_language_version(version);
    for library in ["core", "io", "html"] {
        assert!(
            sdk.platform_metadata.for_sdk_library(library).is_empty(),
            "{library} has no SDK-level experiments"
        );
        assert_eq!(
            sdk.platform_metadata
                .feature_set_for_sdk_library(library, version)
                .unwrap(),
            baseline
        );
    }
    // FeatureSet integration: triple-shift is version-gated, macros is not
    // enabled anywhere by the whitelist.
    assert!(dartiq_language::triple_shift_enabled(LanguageVersion::new(
        3, 13
    )));
    let granted = sdk
        .platform_metadata
        .feature_set_for_package("json", version)
        .unwrap();
    assert!(granted.is_enabled(Feature::Macros));
    assert!(granted.is_enabled(Feature::EnhancedParts));
}

#[test]
fn real_sdk_identity_is_stable_and_flutter_is_rejected() {
    let Some(dart_sdk_root) = real_dart_sdk() else {
        eprintln!("skipped: ~/fvm/default/bin/cache/dart-sdk not present");
        return;
    };
    let a = SdkSnapshot::load(&dart_sdk_root, None).expect("first load");
    let b = SdkSnapshot::load(&dart_sdk_root, None).expect("second load");
    assert_eq!(a.id, b.id, "the same SDK loads to the same id");

    match SdkSnapshot::load(&dart_sdk_root, Some(&dart_sdk_root)) {
        Err(dartiq_sdk::SdkError::FlutterUnsupported { .. }) => {}
        other => panic!("expected FlutterUnsupported, got {other:?}"),
    }
}
