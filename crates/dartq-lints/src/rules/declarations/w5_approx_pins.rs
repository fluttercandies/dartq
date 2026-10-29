use super::remove_deprecations_in_breaking_versions::{SPEC, is_breaking_version};
use crate::registry::Group;
use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

// —— deprecated_member_use_from_same_package ———————————————————————————

/// The corpus `test_deprecatedClass` — a type-annotation use of a
/// same-file deprecated class reports at the use.
#[test]
fn deprecated_use_reports_the_type_reference() {
    let f = Fixture::parse("@deprecated\nclass C {}\n\nvoid f(C c) {}\n");
    let lints = f.run(&one_rule("deprecated_member_use_from_same_package"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "'C' is deprecated and shouldn't be used.");
    let start = utf16_at(&f, tok(&f.source, &f.tokens, "C", 1));
    assert_eq!(lints[0].range, start..start + 1);
}

/// The `@Deprecated('…')` tag rides the with-message variant through
/// the official normalization (`Use B instead` gains the final period).
#[test]
fn deprecated_use_reports_the_normalized_tag() {
    let f = Fixture::parse("@Deprecated('Use B instead')\nclass C {}\n\nvoid f(C c) {}\n");
    let lints = f.run(&one_rule("deprecated_member_use_from_same_package"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "'C' is deprecated and shouldn't be used. Use B instead."
    );
}

/// A same-file deprecated method's invocation reports at the callee.
#[test]
fn deprecated_use_reports_the_invocation() {
    let f = Fixture::parse("@deprecated\nvoid gone() {}\n\nvoid f() {\n  gone();\n}\n");
    let lints = f.run(&one_rule("deprecated_member_use_from_same_package"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let start = utf16_at(&f, tok(&f.source, &f.tokens, "gone", 1));
    assert_eq!(lints[0].range, start..start + 4);
}

/// The frontier exemption (span containment): a use inside the
/// deprecated declaration's own span never reports.
#[test]
fn deprecated_use_inside_the_declaration_stays_silent() {
    let f = Fixture::parse("@deprecated\nclass C {\n  void m(C? c) {}\n}\n");
    assert!(
        f.run(&one_rule("deprecated_member_use_from_same_package"))
            .is_empty()
    );
}

/// A non-deprecated same-file declaration never reports, and a
/// declaration's own name is not a use.
#[test]
fn plain_declaration_uses_stay_silent() {
    let f = Fixture::parse("class C {}\n\nvoid f(C c) {}\n");
    assert!(
        f.run(&one_rule("deprecated_member_use_from_same_package"))
            .is_empty()
    );
}

/// The deprecated-context suppression (upstream
/// `test_deprecatedClass_usedInDeprecated*`): a use inside another
/// `@deprecated` declaration's span never reports — only the
/// non-deprecated container's uses do. And a `@deprecated` `library`
/// directive suppresses every use in the unit.
#[test]
fn deprecated_context_suppresses_uses() {
    // A use inside a deprecated sibling is suppressed; a use in a
    // non-deprecated function still reports.
    let f = Fixture::parse(
        "@deprecated\nclass C {}\n\n@deprecated\nvoid g(C c) {}\n\nvoid f(C c) {}\n",
    );
    let lints = f.run(&one_rule("deprecated_member_use_from_same_package"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    // Only `f`'s `C` (the last `C` reference) reports.
    assert_eq!(lints[0].message, "'C' is deprecated and shouldn't be used.");

    // A `@deprecated library` directive suppresses the whole unit.
    let f =
        Fixture::parse("@deprecated\nlibrary lib;\n\n@deprecated\nclass C {}\n\nvoid f(C c) {}\n");
    assert!(
        f.run(&one_rule("deprecated_member_use_from_same_package"))
            .is_empty()
    );
}

/// The corpus's mid-file face (`test_deprecatedClass_usedInDeprecatedLibrary`
/// transcribed into a shared file): the deprecated `library` directive
/// sits after other declarations — the unit still goes fully silent
/// (the live oracle leg keys zero rows on the corpus file).
#[test]
fn deprecated_midfile_library_suppresses_the_unit() {
    let f = Fixture::parse(
        "@deprecated\nclass Early {}\n\nvoid useEarly(Early e) {}\n\n// NEGATIVE upstream test_deprecatedClass_usedInDeprecatedLibrary.\n@deprecated\nlibrary aDepLib;\n\n@deprecated\nclass CDepLib {}\n\nCDepLib? xDepLib;\n",
    );
    assert!(
        f.run(&one_rule("deprecated_member_use_from_same_package"))
            .is_empty()
    );
}

/// The remaining corpus negative families (upstream
/// `test_deprecatedClass_usedInDeprecated*`): the use rides inside a
/// deprecated field, formal parameter, function-typed parameter,
/// field-formal parameter, extension, extension type or mixin
/// application — every pushed shape suppresses its contents.
#[test]
fn deprecated_member_faces_suppress_their_contents() {
    for source in [
        // Deprecated field initializer + type annotation.
        "@deprecated\nclass CDepFieldInit {}\n\nclass DFieldInit {\n  @deprecated\n  Object f = CDepFieldInit;\n}\n",
        "@deprecated\nclass CDepFieldType {}\n\nclass DFieldType {\n  @deprecated\n  CDepFieldType? f;\n}\n",
        // Deprecated simple / function-typed / field-formal parameters.
        "@deprecated\nclass CDepSimple {}\n\nvoid fDepSimple({@deprecated CDepSimple? c}) {}\n",
        "@deprecated\nclass CDepFnTyped {}\n\nvoid fFnTyped({@deprecated required void p(CDepFnTyped c)}) {}\n",
        "@deprecated\nclass CDepFieldFormal {}\n\nclass DFieldFormal {\n  Object c;\n  DFieldFormal({@deprecated required CDepFieldFormal this.c});\n}\n",
        // Deprecated extension / extension type / mixin application.
        "@deprecated\nclass CDepExt {}\n\n@deprecated\nextension EDepExt on CDepExt {}\n",
        "@deprecated\nclass CDepExtType {}\n\n@deprecated\nextension type EDepExtType(CDepExtType c) { }\n",
        "@deprecated\nmixin class CDepAlias {}\n\n@deprecated\nclass DFromDepAlias = Object with CDepAlias;\n",
    ] {
        let f = Fixture::parse(source);
        assert!(
            f.run(&one_rule("deprecated_member_use_from_same_package"))
                .is_empty(),
            "{source}"
        );
    }
}

/// The mixin application's alias name — not the `=` supertype — keys
/// the declaration table: a plain `Object` use never reports.
#[test]
fn mixin_application_keys_on_the_alias_name() {
    let f = Fixture::parse(
        "@deprecated\nmixin class CDepAlias {}\n\n@deprecated\nclass DFromDepAlias = Object with CDepAlias;\n\nvoid g(Object o) {}\n",
    );
    assert!(
        f.run(&one_rule("deprecated_member_use_from_same_package"))
            .is_empty(),
        "the deprecated alias suppresses its own clause and `Object` is not a deprecated name"
    );
}

/// The directive faces (upstream `test_deprecatedLibrary_*` /
/// `test_deprecatedClass_usedInShowCombinator`): an import/export of a
/// deprecated library reports at the whole directive with the target's
/// package URI; a `show` combinator name of a deprecated class reports
/// at the name; `hide` and a non-deprecated import stay silent.
#[test]
fn directive_faces_report_at_the_directive_and_the_shown_name() {
    let root = std::env::temp_dir().join("dartq_f5_directive_faces");
    let lib = root.join("lib");
    std::fs::create_dir_all(&lib).expect("temp lib dir");
    std::fs::write(lib.join("deprecated_lib.dart"), "@deprecated\nlibrary a;\n")
        .expect("fixture file");
    std::fs::write(
        lib.join("deprecated_class.dart"),
        "@deprecated\nclass C {}\n",
    )
    .expect("fixture file");
    std::fs::write(lib.join("plain.dart"), "class P {}\n").expect("fixture file");
    let facts = crate::WorkspaceFacts {
        own_package: "probe".to_owned(),
        dependencies: rustc_hash::FxHashSet::default(),
        dev_dependencies: rustc_hash::FxHashSet::default(),
        root: root.clone(),
        lib_dir: lib.clone(),
    };
    let run = |source: &str| {
        let mut f = Fixture::parse(source);
        f.path = lib.join("importer.dart");
        f.run_workspace(&one_rule("deprecated_member_use_from_same_package"), &facts)
    };

    // The export face: the whole directive, the package URI as the name.
    let lints = run("export 'deprecated_lib.dart';\n");
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "'package:probe/deprecated_lib.dart' is deprecated and shouldn't be used."
    );
    assert_eq!(lints[0].range, 0..29);

    // The import face, with-message variant through the tag.
    std::fs::write(
        lib.join("deprecated_lib.dart"),
        "@Deprecated('Use B instead')\nlibrary a;\n",
    )
    .expect("fixture file");
    let lints = run("import 'deprecated_lib.dart';\n");
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "'package:probe/deprecated_lib.dart' is deprecated and shouldn't be used. Use B instead."
    );

    // The `show` combinator face: the shown deprecated class name.
    let lints = run("import 'deprecated_class.dart' show C;\n");
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "'C' is deprecated and shouldn't be used.");
    let f = Fixture::parse("import 'deprecated_class.dart' show C;\n");
    let c = tok(&f.source, &f.tokens, "C", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, c));

    // `hide` is forgiving; a non-deprecated import stays silent.
    assert!(run("import 'deprecated_class.dart' hide C;\n").is_empty());
    assert!(run("import 'plain.dart' show P;\n").is_empty());
    // A cross-package URI is the analyzer's face, not this rule's.
    assert!(run("import 'package:other/other.dart';\n").is_empty());

    let _ = std::fs::remove_file(lib.join("deprecated_lib.dart"));
    let _ = std::fs::remove_file(lib.join("deprecated_class.dart"));
    let _ = std::fs::remove_file(lib.join("plain.dart"));
}

// —— remove_deprecations_in_breaking_versions ——————————————————————————
/// The upstream corpus matrix (`test_breakingFirstMajor` through
/// `test_breakingBuild`): a zero-segment boundary with no build
/// metadata is breaking; a prerelease counts, a build does not, and a
/// patch/minor increment past the boundary does not.
#[test]
fn breaking_version_corpus() {
    for version in ["1.0.0", "2.0.0", "0.1.0", "0.0.0", "2.0.0-dev", "10.0.0"] {
        assert!(is_breaking_version(version), "{version}");
    }
    for version in ["2.1.0", "2.0.1", "0.1.1", "0.0.1", "2.0.0+mybuild"] {
        assert!(!is_breaking_version(version), "{version}");
    }
    // The registration bail (:44-46): an unparseable value is not
    // breaking.
    assert!(!is_breaking_version("main"));
    assert!(!is_breaking_version(""));
}

/// Empty optional segments fail closed: upstream pub_semver 2.2.1
/// requires at least one identifier character in each optional
/// segment, so `Version.parse` rejects `1.0.0-` / `1.0.0+` and the
/// official gate never registers — neither may read as a bare
/// breaking `1.0.0`. A populated prerelease still rides the boundary.
#[test]
fn empty_prerelease_or_build_segments_fail_closed() {
    assert!(!is_breaking_version("1.0.0-"));
    assert!(!is_breaking_version("1.0.0+"));
    assert!(!is_breaking_version("1.0.0-beta+"));
    assert!(is_breaking_version("1.0.0-beta.1"));
}

/// The registration shape: `kinds: &[]` — the gate fact is the pubspec
/// `version:` value, the pubspec family's mounting seam
/// (`rules/pubspec.rs`); the #1153 closeout pins the name into
/// `STRUCTURALLY_SILENT` with the freeze regen.
#[test]
fn remove_deprecations_is_structurally_silent() {
    assert_eq!(SPEC.name, "remove_deprecations_in_breaking_versions");
    assert_eq!(SPEC.group, Group::Standalone);
    assert!(SPEC.kinds.is_empty(), "{:?}", SPEC.kinds);
}
