//! The class-modifier restriction families end-to-end (#600, M2 T3): the
//! resolution layer's decision set (`dartq_resolution::modifiers`) emitting
//! through [`Engine::analyze`] with the official analyzer codes and message
//! texts. Every pin was anchored verbatim against the pinned SDK registry
//! (`.third/dart-sdk` `pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart`,
//! dart 3.15 main).
//!
//! Faces covered, positive and negative per verdict:
//!
//! * outside-library family (`invalid_use_of_type_outside_library` under
//!   its family-unique machine names): sealed supertype (extends and
//!   implements — the `with` face is unreachable per the official
//!   with-non-mixin gate), final extends/implements/`on`-constraint,
//!   interface extends, base class/mixin implemented;
//! * the base-or-final subtype requirement
//!   (`subtype_of_base_or_final_is_not_base_final_or_sealed` family): the
//!   class and mixin twins, the cross-library base extends, and the
//!   sealed-induced-modifier case;
//! * the declaration-site combination matrix (`abstract sealed`,
//!   `abstract final base`, `abstract final interface`), including the
//!   legal-combination negatives.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use dartq_diagnostics::Diagnostic;
use dartq_package::PackageTable;
use dartq_pub::{LockFile, PackageConfig, Pubspec, build_graph};
use dartq_sdk::SdkSnapshot;
use dartq_source::FileId;

use dartq_engine::{CancellationToken, Engine, EngineConfig, memory_store};

/// Two-library single-package workspace through the real engine (the
/// `semdiag` fixture shape): synthetic SDK, the real pub/package faces, the
/// default diagnoser on. `main` is returned with the merged analyze
/// diagnostics; `other` carries the declarations under test (an empty
/// string keeps the workspace single-file for the same-library fixtures).
fn analyze(main: &str, other: &str) -> (FileId, Vec<Diagnostic>) {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let root = std::env::temp_dir().join(format!(
        "dartq-engine-modifiers-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    let write = |relative: &str, content: &str| {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(path, content).expect("file");
    };
    write(
        "pubspec.yaml",
        "name: t\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n",
    );
    write(
        "pubspec.lock",
        "packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n",
    );
    write(
        ".dart_tool/package_config.json",
        r#"{"configVersion": 2, "packages": [{"name": "t", "rootUri": "../", "languageVersion": "3.5"}], "generator": "pub"}"#,
    );
    write("sdk/version", "3.13.1\n");
    write(
        "sdk/lib/libraries.json",
        r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}}}}"#,
    );
    write(
        "sdk/lib/_internal/allowed_experiments.json",
        r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
    );
    // Canonicalize before the pub faces resolve it (macOS /var vs
    // /private/var).
    let root = root.canonicalize().expect("fixture root resolves");
    let config_dir = root.join(".dart_tool");
    let config = Arc::new(
        PackageConfig::parse(
            &std::fs::read_to_string(config_dir.join("package_config.json")).expect("config"),
        )
        .expect("valid config"),
    );
    let lock =
        LockFile::parse("packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n").expect("valid lock");
    let root_pubspec = Pubspec::parse("name: t\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n")
        .expect("valid pubspec");
    let graph = build_graph(
        &config,
        &lock,
        &root_pubspec,
        &std::collections::BTreeMap::new(),
        None,
        &config_dir,
    )
    .expect("graph");
    let packages = PackageTable::build(&config, &graph, &config_dir).expect("table");

    let mut engine = Engine::new(EngineConfig {
        store: memory_store(),
        sdk: SdkSnapshot::load(&root.join("sdk"), None).expect("synthetic SDK loads"),
        packages,
        pub_config: Arc::clone(&config),
        config_dir,
        semdiag: Some(dartq_engine::semdiag::diagnoser()),
        target: dartq_engine::DEFAULT_TARGET.to_owned(),
        declared_variables: rustc_hash::FxHashMap::default(),
    })
    .expect("engine");
    let main_file = engine.set_content(&root.join("lib/main.dart"), main.as_bytes());
    engine.set_content(&root.join("lib/other.dart"), other.as_bytes());
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("analyze completes");
    (main_file, diagnostics)
}

/// The diagnostics carrying `code`, in analyze output order.
fn with_code<'a>(diagnostics: &'a [Diagnostic], code: &str) -> Vec<&'a Diagnostic> {
    diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code.name == code)
        .collect()
}

/// Asserts `code` fired exactly once on `main` with the verbatim oracle
/// message (the outside-library family shares the analyzerCode
/// `invalid_use_of_type_outside_library`, so the message text is the
/// load-bearing identity within the family).
fn assert_emitted(diagnostics: &[Diagnostic], code: &str, message: &str) {
    let hits = with_code(diagnostics, code);
    assert_eq!(hits.len(), 1, "{code}: {diagnostics:?}");
    assert_eq!(hits[0].message, message, "{code}");
}

/// Asserts no class-modifier row of any family fired.
fn assert_no_modifier_diagnostics(diagnostics: &[Diagnostic]) {
    let modifier_codes = [
        "invalid_use_of_type_outside_library",
        "subtype_of_base_or_final_is_not_base_final_or_sealed",
        "abstract_sealed_class",
        "abstract_final_base_class",
        "abstract_final_interface_class",
        "final_mixin",
        "interface_mixin",
        "sealed_mixin",
        "final_mixin_class",
        "interface_mixin_class",
        "sealed_mixin_class",
    ];
    for code in modifier_codes {
        assert!(
            with_code(diagnostics, code).is_empty(),
            "{code}: {diagnostics:?}"
        );
    }
}

/// A sealed class cannot be extended outside its library — the extends
/// face, anchored `sealed_class_subtype_outside_of_library`.
#[test]
fn sealed_class_extended_outside_its_library() {
    let (_, diagnostics) = analyze(
        "import 'other.dart';\nclass A extends S {}\n",
        "sealed class S {}\n",
    );
    assert_emitted(
        &diagnostics,
        "invalid_use_of_type_outside_library",
        "The class 'S' can't be extended, implemented, or mixed in outside of its library because it's a sealed class.",
    );
}

/// The same verdict through the `implements` face. The official `with`
/// face is unreachable (the drive block sits behind the with-non-mixin
/// gate and a sealed proper mixin cannot exist), so a `with` of a sealed
/// class reports no modifier row here either — pinned alongside.
#[test]
fn sealed_class_implemented_outside_its_library() {
    let (_, diagnostics) = analyze(
        "import 'other.dart';\nclass B implements S {}\nclass C with S {}\n",
        "sealed class S {}\n",
    );
    assert_eq!(
        with_code(&diagnostics, "invalid_use_of_type_outside_library").len(),
        1
    );
    let hits = with_code(&diagnostics, "invalid_use_of_type_outside_library");
    assert_eq!(
        hits[0].message,
        "The class 'S' can't be extended, implemented, or mixed in outside of its library because it's a sealed class."
    );
}

/// Same-library sealed use is unrestricted — the negative twin.
#[test]
fn sealed_class_used_within_its_library_is_unrestricted() {
    let (_, diagnostics) = analyze(
        "sealed class S {}\nclass A extends S {}\nclass B implements S {}\n",
        "",
    );
    assert_no_modifier_diagnostics(&diagnostics);
}

/// A final class cannot be extended outside its library
/// (`final_class_extended_outside_of_library`).
#[test]
fn final_class_extended_outside_its_library() {
    let (_, diagnostics) = analyze(
        "import 'other.dart';\nclass A extends F {}\n",
        "final class F {}\n",
    );
    assert_emitted(
        &diagnostics,
        "invalid_use_of_type_outside_library",
        "The class 'F' can't be extended outside of its library because it's a final class.",
    );
}

/// A final class cannot be implemented outside its library — the
/// implements walk covers the transitive supertypes, and the subtype
/// requirement stays silent (the outside-library row covers it, the
/// official carve-out).
#[test]
fn final_class_implemented_outside_its_library() {
    let (_, diagnostics) = analyze(
        "import 'other.dart';\nclass A implements F {}\n",
        "final class F {}\n",
    );
    assert_emitted(
        &diagnostics,
        "invalid_use_of_type_outside_library",
        "The class 'F' can't be implemented outside of its library because it's a final class.",
    );
    assert!(
        with_code(
            &diagnostics,
            "subtype_of_base_or_final_is_not_base_final_or_sealed"
        )
        .is_empty()
    );
}

/// A final class cannot be a mixin superclass constraint outside its
/// library (`final_class_used_as_mixin_constraint_outside_of_library`).
#[test]
fn final_class_used_as_mixin_constraint_outside_its_library() {
    let (_, diagnostics) = analyze(
        "import 'other.dart';\nmixin M on F {}\n",
        "final class F {}\n",
    );
    assert_emitted(
        &diagnostics,
        "invalid_use_of_type_outside_library",
        "The class 'F' can't be used as a mixin superclass constraint outside of its library because it's a final class.",
    );
}

/// Same-library final subtyping needs no modifier rows when the subtypes
/// carry the required modifiers — the negative twin of the final family.
#[test]
fn final_class_subtyped_within_its_library_with_modifiers() {
    let (_, diagnostics) = analyze(
        "final class F {}\nfinal class A extends F {}\nbase mixin M on F {}\n",
        "",
    );
    assert_no_modifier_diagnostics(&diagnostics);
}

/// An interface class cannot be extended outside its library
/// (`interface_class_extended_outside_of_library`), while implementing it
/// outside stays legal — the interface contract.
#[test]
fn interface_class_extended_but_not_implemented_outside_its_library() {
    let (_, extended) = analyze(
        "import 'other.dart';\nclass A extends I {}\n",
        "interface class I {}\n",
    );
    assert_emitted(
        &extended,
        "invalid_use_of_type_outside_library",
        "The class 'I' can't be extended outside of its library because it's an interface class.",
    );
    let (_, implemented) = analyze(
        "import 'other.dart';\nclass A implements I {}\n",
        "interface class I {}\n",
    );
    assert_no_modifier_diagnostics(&implemented);
}

/// A base class cannot be implemented outside its library
/// (`base_class_implemented_outside_of_library`); the subtype requirement
/// accompanies it (the unmarked implementer is a subtype of a base
/// supertype), and extending it outside is legal for a *base*-marked
/// extender — the negative twin.
#[test]
fn base_class_implemented_outside_its_library() {
    let (_, diagnostics) = analyze(
        "import 'other.dart';\nclass A implements B {}\n",
        "base class B {}\n",
    );
    assert_emitted(
        &diagnostics,
        "invalid_use_of_type_outside_library",
        "The class 'B' can't be implemented outside of its library because it's a base class.",
    );
    assert_emitted(
        &diagnostics,
        "subtype_of_base_or_final_is_not_base_final_or_sealed",
        "The type 'A' must be 'base', 'final' or 'sealed' because the supertype 'B' is 'base'.",
    );
    let (_, extended) = analyze(
        "import 'other.dart';\nbase class A extends B {}\n",
        "base class B {}\n",
    );
    assert_no_modifier_diagnostics(&extended);
}

/// A base mixin cannot be implemented outside its library
/// (`base_mixin_implemented_outside_of_library`), with the subtype row
/// alongside (the class implementer is an unmarked subtype of a base
/// element).
#[test]
fn base_mixin_implemented_outside_its_library() {
    let (_, diagnostics) = analyze(
        "import 'other.dart';\nclass A implements BM {}\n",
        "base mixin BM {}\n",
    );
    assert_emitted(
        &diagnostics,
        "invalid_use_of_type_outside_library",
        "The mixin 'BM' can't be implemented outside of its library because it's a base mixin.",
    );
    assert_emitted(
        &diagnostics,
        "subtype_of_base_or_final_is_not_base_final_or_sealed",
        "The type 'A' must be 'base', 'final' or 'sealed' because the supertype 'BM' is 'base'.",
    );
}

/// Extending a base class from another library requires the extender to be
/// `base`/`final`/`sealed` — the subtype requirement is not same-library
/// (base_or_final_type_verifier.dart:189-315; the :185-187 gate is the
/// element's own library). The outside-library family stays silent on the
/// extends face (base restricts implements, not extends).
#[test]
fn unmarked_class_extending_cross_library_base_class() {
    let (_, diagnostics) = analyze(
        "import 'other.dart';\nclass C extends B {}\n",
        "base class B {}\n",
    );
    assert_emitted(
        &diagnostics,
        "subtype_of_base_or_final_is_not_base_final_or_sealed",
        "The type 'C' must be 'base', 'final' or 'sealed' because the supertype 'B' is 'base'.",
    );
    assert!(with_code(&diagnostics, "invalid_use_of_type_outside_library").is_empty());
}

/// The final twin of the subtype requirement for a class, same library
/// (`subtype_of_final_is_not_base_final_or_sealed`).
#[test]
fn unmarked_class_extending_same_library_final_class() {
    let (_, diagnostics) = analyze("final class F {}\nclass C extends F {}\n", "");
    assert_emitted(
        &diagnostics,
        "subtype_of_base_or_final_is_not_base_final_or_sealed",
        "The type 'C' must be 'base', 'final' or 'sealed' because the supertype 'F' is 'final'.",
    );
}

/// The mixin twin of the base case: a mixin with a base `on` constraint
/// must be `base` (`mixin_subtype_of_base_is_not_base`).
#[test]
fn unmarked_mixin_on_base_constraint() {
    let (_, diagnostics) = analyze("base class B {}\nmixin M on B {}\n", "");
    assert_emitted(
        &diagnostics,
        "subtype_of_base_or_final_is_not_base_final_or_sealed",
        "The mixin 'M' must be 'base' because the supertype 'B' is 'base'.",
    );
}

/// The mixin twin of the final case
/// (`mixin_subtype_of_final_is_not_base`).
#[test]
fn unmarked_mixin_on_final_constraint() {
    let (_, diagnostics) = analyze("final class F {}\nmixin M on F {}\n", "");
    assert_emitted(
        &diagnostics,
        "subtype_of_base_or_final_is_not_base_final_or_sealed",
        "The mixin 'M' must be 'base' because the supertype 'F' is 'final'.",
    );
}

/// The induced-modifier face: a sealed class under a `base` supertype
/// carries an induced `base` flag (instance_member_inferrer.dart:649-719),
/// so the unmarked grandchild reports against the explicit ancestor's name.
#[test]
fn sealed_class_induces_base_on_its_subclass_hierarchy() {
    let (_, diagnostics) = analyze(
        "base class B {}\nsealed class S extends B {}\nclass C extends S {}\n",
        "",
    );
    assert_emitted(
        &diagnostics,
        "subtype_of_base_or_final_is_not_base_final_or_sealed",
        "The type 'C' must be 'base', 'final' or 'sealed' because the supertype 'B' is 'base'.",
    );
    // The sealed carrier itself satisfies the requirement.
    assert_eq!(
        with_code(
            &diagnostics,
            "subtype_of_base_or_final_is_not_base_final_or_sealed"
        )
        .len(),
        1
    );
}

/// `abstract sealed` — the declaration-site pair check
/// (`abstract_sealed_class`, anchored at the `sealed` keyword, with the
/// official correction text).
#[test]
fn abstract_sealed_class_declaration() {
    let (_, diagnostics) = analyze("abstract sealed class S {}\n", "");
    assert_emitted(
        &diagnostics,
        "abstract_sealed_class",
        "A 'sealed' class can't be marked 'abstract' because it's already implicitly abstract.",
    );
    let hits = with_code(&diagnostics, "abstract_sealed_class");
    assert_eq!(
        hits[0].correction.as_deref(),
        Some("Try removing the 'abstract' keyword.")
    );
}

/// `abstract final base` beats the interface twin in the official
/// else-if nesting (`abstract_final_base_class`, spanned `final`→`base`).
#[test]
fn abstract_final_base_class_declaration() {
    let (_, diagnostics) = analyze("abstract final base class C {}\n", "");
    assert_emitted(
        &diagnostics,
        "abstract_final_base_class",
        "An 'abstract' class can't be declared as both 'final' and 'base'.",
    );
}

/// `abstract final interface` — the interface twin
/// (`abstract_final_interface_class`).
#[test]
fn abstract_final_interface_class_declaration() {
    let (_, diagnostics) = analyze("abstract final interface class C {}\n", "");
    assert_emitted(
        &diagnostics,
        "abstract_final_interface_class",
        "An 'abstract' class can't be declared as both 'final' and 'interface'.",
    );
}

/// The legal declaration-site combinations emit nothing: every pair the
/// frozen matrix admits (`abstract final` without base/interface, `base`
/// mixin, `mixin class`, plain `abstract`/`sealed`/`base`/`final`/
/// `interface`).
#[test]
fn legal_modifier_combinations_stay_silent() {
    let (_, diagnostics) = analyze(
        "abstract final class A {}\nabstract base class B {}\nsealed class C {}\nfinal class D {}\ninterface class E {}\nbase class F {}\nbase mixin M {}\nmixin class MC {}\nabstract class AC {}\n",
        "",
    );
    assert_no_modifier_diagnostics(&diagnostics);
}
