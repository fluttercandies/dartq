//! Unit tests for the [`crate::loader`] parse functions.
//!
//! Parsing-level tests run against synthetic JSON plus a real SDK
//! `libraries.json` when `DARTQ_SDK_CHECKOUT` points at a dart-lang/sdk
//! checkout; full-load and error-path tests against complete SDK layouts
//! live in the crate's `tests/` suite.

use std::collections::BTreeSet;

use dartq_language::{Feature, FeatureSet, LanguageVersion};

use super::{parse_allowed_experiments, parse_embedder_yaml, parse_libraries_json};
use crate::metadata::Importability;
use crate::metadata::UnknownExperiments;

/// The real `libraries.json` from a dart-lang/sdk checkout
/// (`DARTQ_SDK_CHECKOUT`), when available.
fn sdk_libraries_json() -> Option<String> {
    let root = std::env::var("DARTQ_SDK_CHECKOUT").ok()?;
    std::fs::read_to_string(std::path::Path::new(&root).join("sdk/lib/libraries.json")).ok()
}

#[test]
fn real_libraries_json_resolves_targets() {
    let Some(text) = sdk_libraries_json() else {
        eprintln!(
            "skipped: set DARTQ_SDK_CHECKOUT to a dart-lang/sdk checkout for the real libraries.json"
        );
        return;
    };
    let libraries = parse_libraries_json(&text).expect("the real file parses");

    // Include expansion: `vm` = `vm_common` ∪ {cli}; comment keys are not
    // segments; internal combination segments resolve too.
    let vm = libraries.get("vm").expect("vm segment exists");
    assert!(vm.libraries.contains_key("cli"), "own vm segment entry");
    for inherited in ["core", "io", "typed_data", "_builtin", "vmservice_io"] {
        assert!(
            vm.libraries.contains_key(inherited),
            "vm_common inherits {inherited}"
        );
    }
    assert_eq!(vm.libraries.len(), 21);
    assert!(
        libraries
            .get("none")
            .expect("none segment")
            .libraries
            .is_empty()
    );
    assert!(!libraries.targets.contains_key("comment:0"));

    // vm patches survive normalization (core carries the vm patch list).
    let core = &vm.libraries["core"];
    assert_eq!(core.uri, "core/core.dart");
    assert_eq!(
        core.patches.first().map(String::as_str),
        Some("_internal/vm/lib/array_patch.dart")
    );
    assert!(core.support_conditional_import);

    // Web targets gate conditional imports on io/isolate/ffi.
    let ddc = libraries.get("dartdevc").expect("dartdevc segment");
    assert!(!ddc.libraries["io"].support_conditional_import);
    assert!(ddc.libraries["ffi"].support_direct_import != Importability::Always);
}

#[test]
fn include_expansion_and_override() {
    let libraries = parse_libraries_json(
        r#"{
            "comment:0": "generated",
            "base": {"libraries": {"x": {"uri": "x/base.dart"}, "y": {"uri": "y.dart"}}},
            "mid": {"include": [{"target": "base"}], "libraries": {}},
            "top": {
                "include": [{"target": "mid"}],
                "libraries": {"x": {"uri": "x/top.dart"}, "z": {"uri": "z.dart"}}
            }
        }"#,
    )
    .expect("parses");
    let top = &libraries.targets["top"].libraries;
    // Diamond inheritance: own definition overrides the inherited one;
    // transitively inherited entries survive.
    assert_eq!(top["x"].uri, "x/top.dart");
    assert_eq!(top["y"].uri, "y.dart");
    assert_eq!(top["z"].uri, "z.dart");
}

#[test]
fn include_errors_are_hard() {
    let cycle = parse_libraries_json(
        r#"{"a": {"include": [{"target": "b"}], "libraries": {}},
            "b": {"include": [{"target": "a"}], "libraries": {}}}"#,
    )
    .expect_err("cycle");
    assert!(cycle.contains("cycle"), "`{cycle}`");

    let unknown =
        parse_libraries_json(r#"{"a": {"include": [{"target": "ghost"}], "libraries": {}}}"#)
            .expect_err("unknown target");
    assert!(unknown.contains("unknown target `ghost`"), "`{unknown}`");
}

#[test]
fn cross_file_include_path_key_is_rejected() {
    // `{path, target}` loads another libraries.json file (official
    // `libraries_specification.dart` format). dartq does not implement
    // multi-file loading — and the current file *does* define `other` — so
    // the error must fire instead of silently resolving `target` from the
    // wrong file.
    let err = parse_libraries_json(
        r#"{"other": {"libraries": {"o": {"uri": "o.dart"}}},
            "top": {"include": [{"path": "other.json", "target": "other"}], "libraries": {}}}"#,
    )
    .expect_err("cross-file include");
    assert!(err.contains("`path`"), "`{err}`");
    assert!(err.contains("`top`"), "`{err}`");
    // Not the unknown-target failure: `other` exists in this file.
    assert!(!err.contains("unknown target"), "`{err}`");
}

#[test]
fn library_field_semantics() {
    let libraries = parse_libraries_json(
        r#"{
            "t": {"libraries": {
                "plain": {"uri": "p.dart"},
                "string_patches": {"uri": "s.dart", "patches": "one_patch.dart"},
                "opted_in": {"uri": "h.dart", "support_conditional_import": true},
                "gated": {"uri": "g.dart", "support_direct_import": "with_flag",
                          "support_conditional_import": false},
                "_forced_off": {"uri": "u.dart"}
            }}
        }"#,
    )
    .expect("parses");
    let t = &libraries.targets["t"].libraries;
    assert_eq!(t["plain"].patches, Vec::<String>::new());
    assert!(t["plain"].support_conditional_import);
    assert_eq!(t["plain"].support_direct_import, Importability::Always);
    // String patches normalize to a one-element list.
    assert_eq!(
        t["string_patches"].patches,
        vec!["one_patch.dart".to_owned()]
    );
    // An explicit `true` is preserved for a non-`_` name…
    assert!(t["opted_in"].support_conditional_import);
    assert_eq!(t["gated"].support_direct_import, Importability::WithFlag);
    assert!(!t["gated"].support_conditional_import);
    // …and the `_` name prefix forces `false` regardless of the explicit
    // value (`support_conditional_import && !name.startsWith('_')`).
    assert!(!t["_forced_off"].support_conditional_import);
}

#[test]
fn null_patches_parse_as_empty() {
    // The official parser maps `patches: null` to an empty list instead of
    // a type error (libraries_specification.dart:333-335: `data['patches']
    // == null` yields `const []`).
    let libraries =
        parse_libraries_json(r#"{"t": {"libraries": {"x": {"uri": "x.dart", "patches": null}}}}"#)
            .expect("null patches parse");
    assert_eq!(
        libraries.targets["t"].libraries["x"].patches,
        Vec::<String>::new()
    );
}

#[test]
fn library_entry_errors_are_hard() {
    // `uri` is mandatory.
    let missing_uri =
        parse_libraries_json(r#"{"t": {"libraries": {"x": {}}}}"#).expect_err("missing uri");
    assert!(missing_uri.contains("`t`"), "`{missing_uri}`");
    // Unknown direct-import values are rejected, not defaulted.
    let bad_gate = parse_libraries_json(
        r#"{"t": {"libraries": {"x": {"uri": "x.dart", "support_direct_import": "sometimes"}}}}"#,
    )
    .expect_err("bad gate");
    assert!(bad_gate.contains("support_direct_import"), "`{bad_gate}`");
}

#[test]
fn unsupported_uri_schemes_are_rejected() {
    // The official `checkAndResolve` (libraries_specification.dart:300-317)
    // rejects any non-`file` scheme at parse time — for `uri` and patches
    // alike.
    let err = parse_libraries_json(
        r#"{"t": {"libraries": {"x": {"uri": "http://evil.example/x.dart"}}}}"#,
    )
    .expect_err("http uri");
    assert!(err.contains("unsupported URI scheme `http:`"), "`{err}`");
    assert!(err.contains("`uri`") && err.contains("`x`"), "`{err}`");

    let err = parse_libraries_json(
        r#"{"t": {"libraries": {"x": {"uri": "x.dart", "patches": "gopher://p.dart"}}}}"#,
    )
    .expect_err("gopher patch");
    assert!(err.contains("unsupported URI scheme `gopher:`"), "`{err}`");
    assert!(err.contains("`patches`"), "`{err}`");

    // Scheme-less paths and `file:` URIs pass the gate untouched.
    let libraries = parse_libraries_json(
        r#"{"t": {"libraries": {
                "x": {"uri": "x.dart", "patches": ["file:patched.dart"]}
            }}}"#,
    )
    .expect("scheme-less and `file:` values pass");
    assert_eq!(
        libraries.targets["t"].libraries["x"].patches,
        vec!["file:patched.dart".to_owned()]
    );
}

#[test]
fn unknown_keys_are_tolerated() {
    let libraries = parse_libraries_json(
        r#"{
            "t": {
                "documented": "why not",
                "libraries": {"x": {"uri": "x.dart", "categories": "Client", "maturity": "stable"}}
            }
        }"#,
    )
    .expect("unknown keys are ignored");
    assert_eq!(libraries.targets["t"].libraries["x"].uri, "x.dart");
}

#[test]
fn embedder_yaml_honors_embedded_libs() {
    // The real sky_engine shape: quoted dart: keys, relative values,
    // comments, and extra top-level sections the reader never consults.
    let libraries = parse_embedder_yaml(
        r#"# generated
embedded_libs:
  "dart:core": "core/core.dart"
  "dart:ui": "ui/ui.dart"
  "dart:_http": "_http/http.dart"
  "not-a-dart-uri": "ignored/ignored.dart"
embedded_server_libraries:
  whatever: else
"#,
    )
    .expect("usable embedder yaml parses");
    // Keys keep only the library name; non-`dart:` keys are dropped
    // silently (the reader's own rule), `_`-prefixed names stay.
    assert_eq!(
        libraries.keys().map(String::as_str).collect::<Vec<_>>(),
        ["_http", "core", "ui"]
    );
    assert_eq!(libraries["ui"].uri, "ui/ui.dart");
    // No gates on embedder entries — every mapped library answers
    // `dart.library.<name>` true and imports directly, the `_`-forced-off
    // rule of libraries.json parsing does not apply.
    assert!(libraries["_http"].support_conditional_import);
    assert_eq!(libraries["_http"].support_direct_import, Importability::Always);
    assert!(libraries["core"].patches.is_empty());
}

#[test]
fn embedder_yaml_rejects_unusable_shapes() {
    for (yaml, needle) in [
        ("embedded_libs: [not, a, map]\n", "embedded_libs"),
        ("- just\n- a\n- list\n", "embedder yaml"),
        ("embedded_libs:\n  \"dart:x\": [\"not\", \"a\", \"path\"]\n", "string"),
        ("embedded_libs: {}\n", "maps no `dart:`"),
        (
            "embedded_libs:\n  \"only:other\": \"x.dart\"\n",
            "maps no `dart:`",
        ),
        (
            "embedded_libs:\n  \"dart:\": \"x.dart\"\n",
            "names no library",
        ),
        (
            "embedded_libs:\n  \"dart:x\": \"/absolute/x.dart\"\n",
            "not a non-empty path relative",
        ),
        ("missing_key_entirely: true\n", "embedded_libs"),
    ] {
        let err = parse_embedder_yaml(yaml).expect_err(yaml);
        assert!(err.contains(needle), "`{yaml}` → `{err}`");
    }
}

/// The real `allowed_experiments.json` content, verbatim from every SDK on
/// this machine (all four carry the identical file).
const REAL_ALLOWED_EXPERIMENTS: &str = r#"{
  "version": 1,
  "experimentSets": {
    "sdkExperiments": [],
    "macros": ["enhanced-parts", "macros"]
  },
  "sdk": {
    "default": {"experimentSet": "sdkExperiments"},
    "_example_libraries": {"ui": {"experimentSet": "nullSafety"}}
  },
  "packages": {"json": {"experimentSet": "macros"}}
}"#;

#[test]
fn allowed_experiments_queries() {
    let experiments = parse_allowed_experiments(REAL_ALLOWED_EXPERIMENTS).expect("parses");
    // SDK libraries fall back to the default (empty) set.
    assert!(experiments.for_sdk_library("core").is_empty());
    // The `sdk._example_libraries` key is not a library grant: `ui` also
    // falls back to the default instead of inheriting the sample set.
    assert!(experiments.for_sdk_library("ui").is_empty());
    // Package grants resolve through their experiment set.
    let json_set = experiments
        .for_package("json")
        .expect("`json` grant exists")
        .clone();
    assert_eq!(
        json_set,
        BTreeSet::from(["enhanced-parts".to_owned(), "macros".to_owned()])
    );
    assert!(experiments.for_package("other").is_none());
}

#[test]
fn allowed_experiments_errors_are_hard() {
    let cases: [(&str, &str); 5] = [
        (r#"{"version": 2}"#, "`version`"),
        (r#"{"version": 1}"#, "experimentSets"),
        (
            r#"{"version": 1, "experimentSets": {"a": []}, "sdk": {}}"#,
            "sdk.default",
        ),
        (
            r#"{"version": 1, "experimentSets": {}, "sdk": {"default": {"experimentSet": "ghost"}}}"#,
            "undefined experiment set",
        ),
        (
            r#"{"version": 1, "experimentSets": {"a": []}, "sdk": {"default": {"experimentSet": "a"}, "libraries": {"core": {"experimentSet": "ghost"}}}}"#,
            "undefined experiment set",
        ),
    ];
    for (text, fragment) in cases {
        let message = parse_allowed_experiments(text).expect_err(text);
        assert!(
            message.contains(fragment),
            "`{message}` should mention `{fragment}`"
        );
    }
}

#[test]
fn feature_set_mapping_adds_whitelisted_experiments() {
    let experiments = parse_allowed_experiments(REAL_ALLOWED_EXPERIMENTS).expect("parses");
    let version = LanguageVersion::new(3, 13);
    let baseline = FeatureSet::for_language_version(version);

    // The real file's default set is empty: an ungranted library gets the
    // pure version baseline.
    assert_eq!(
        experiments
            .feature_set_for_sdk_library("core", version)
            .unwrap(),
        baseline
    );

    // A package without a grant gets the baseline too (no default fallback).
    assert_eq!(
        experiments
            .feature_set_for_package("other", version)
            .unwrap(),
        baseline
    );

    // The `json` package grant enables both macros experiments at 3.13:
    // both clear their official release gates there (macros 3.3,
    // enhanced-parts 3.6). Grant re-restriction across versions is pinned
    // end-to-end in tests/vendored_sdk.rs.
    let granted = experiments
        .feature_set_for_package("json", version)
        .unwrap();
    for name in ["enhanced-parts", "macros"] {
        let feature = Feature::try_from(name).expect("known registry entry");
        assert!(!baseline.is_enabled(feature), "{name} is opt-in");
        assert!(granted.is_enabled(feature), "{name} enabled by the grant");
    }
}

#[test]
fn feature_set_mapping_reports_unknown_experiments() {
    // An experiment name the vendored registry does not know must surface,
    // not silently disable.
    let skewed = parse_allowed_experiments(
        r#"{
        "version": 1,
        "experimentSets": {"future": ["macros", "not-in-registry"]},
        "sdk": {"default": {"experimentSet": "future"}}
    }"#,
    )
    .expect("parses");
    let err = skewed
        .feature_set_for_sdk_library("core", LanguageVersion::new(3, 13))
        .unwrap_err();
    assert_eq!(err, UnknownExperiments(vec!["not-in-registry".to_owned()]));
    assert!(err.to_string().contains("not-in-registry"));
}
