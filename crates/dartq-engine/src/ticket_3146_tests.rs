//! #3146 pins: `duplicate_definition` statement-list rows — the
//! `diagnose_library` fold at semdiag.rs must drop only the rows the
//! `l1::duplicate_diagnostics` bake re-issues (the capture-time
//! `elements.duplicates()` log — top-level, member, formal, type-param,
//! catch faces), never the statement-list staged rows
//! (`stage_local_duplicate` — block bodies, for-head lists, pattern-decl
//! elements) that share the same code but live outside the element model.
//!
//! Upstream: `library_analyzer.dart:305-315` (the gather-per-file/
//! merge-per-library pair — each file's verifier runs per unit, the
//! library merges the results) and `duplicate_definition_verifier.dart`
//! `checkStatements`:85-105 (the statement-list scope walk — a block
//! body, a function body, and each switch member's statement list own
//! one duplicate scope; a repeat inside the same scope reports at the
//! second name).
//!
//! Faces covered, one pin per statement-list shape plus the element-model
//! faces that must stay green (the bake must not double-report):
//!
//! * block-local variable (`int x; int x;` inside a function body);
//! * for-head declaration list (`for (int i=0,i=0;;)`);
//! * pattern-declaration element + local variable collision;
//! * local function + local variable collision;
//! * formal parameter + local variable (no report — the oracle does not
//!   flag a formal/local shadow in the same body);
//! * sibling blocks (`{ int x; } { int x; }` — separate scopes, no
//!   report);
//! * nested shadowing (`int x; { int x; }` — inner scope shadows
//!   legally, no report);
//! * switch-member statement list (`case 1: int x; int x;`);
//! * class member (`class C { int x; int x; }` — l1 bake covers, stays
//!   green, no double-report);
//! * enum constant (`enum E { a, a }` — l1 bake covers);
//! * top-level typedef (`typedef A = int; typedef A = String;` — l1
//!   bake covers);
//! * formal parameter duplicate (`void f(void g(), void g())` — l1
//!   bake covers).

use super::*;
use dartq_pub::{LockFile, Pubspec, build_graph};
use std::sync::atomic::{AtomicU32, Ordering};

/// A throwaway single-package workspace + synthetic core-only SDK — the
/// same shape the other pin fixtures build (package `t`, language
/// version 3.5), but with the default diagnoser wired so the
/// `diagnose_library` fold runs.
fn pin_engine() -> (PathBuf, Engine) {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let root = std::env::temp_dir().join(format!(
        "dartq-engine-3146-{}-{}",
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
        r#"{"configVersion": 2, "packages": [{"languageVersion": "3.5", "name": "t", "packageUri": "lib/", "rootUri": "../"}]}"#,
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
    write("sdk/lib/core/core.dart", "library dart.core;\n");
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
    let engine = Engine::new(EngineConfig {
        store: memory_store(),
        sdk: SdkSnapshot::load(&root.join("sdk"), None).expect("synthetic SDK loads"),
        packages: PackageTable::build(&config, &graph, &config_dir).expect("table"),
        pub_config: Arc::clone(&config),
        config_dir,
        context_roots: Vec::new(),
        semdiag: Some(crate::semdiag::diagnoser()),
        target: DEFAULT_TARGET.to_owned(),
        declared_variables: FxHashMap::default(),
        experiments: Vec::new(),
    })
    .expect("engine");
    (root, engine)
}

/// Analyzes a fresh workspace holding `files` (lib-relative) and returns
/// the workspace root with the full diagnostic set.
fn analyze_files(files: &[(&str, &str)]) -> (PathBuf, Vec<Diagnostic>) {
    let (root, mut engine) = pin_engine();
    let lib = root.join("lib");
    std::fs::create_dir_all(&lib).expect("lib dir");
    for (name, text) in files {
        std::fs::write(lib.join(name), text).expect("fixture file");
    }
    engine.discover_workspace().expect("discover");
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("analyze");
    (root, diagnostics)
}

/// The `duplicate_definition` rows of a diagnostic set, sorted for
/// stable comparison. Every fixture carries its rows in a single file,
/// so the file identity is not part of the comparison.
fn dup_rows(diagnostics: &[Diagnostic]) -> Vec<(u32, u32, String)> {
    let mut rows: Vec<(u32, u32, String)> = diagnostics
        .iter()
        .filter(|row| row.code.name == "duplicate_definition")
        .map(|row| (row.range.offset, row.range.length, row.message.to_string()))
        .collect();
    rows.sort();
    rows
}

/// `void f(){ int x; int x; }` — the block-body statement list owns one
/// duplicate scope (`checkStatements`:85-105). The oracle reports column
/// 22 on line 1 (1-based); the byte offset is 22 − 1 = 21 (0-based).
#[test]
fn block_local_variable_duplicate() {
    let (root, diagnostics) = analyze_files(&[("test.dart", "void f(){ int x; int x; }\n")]);
    assert_eq!(
        dup_rows(&diagnostics),
        vec![(21, 1, "The name 'x' is already defined.".to_string())],
        "the second `int x` in the same block reports: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `void f(){ for (int i=0,i=0;;) {} }` — a for-head declaration list
/// checks inside itself (`checkForVariables`), never against the
/// enclosing block. The oracle reports column 24 on line 1 (1-based);
/// the byte offset is 24 − 1 = 23 (0-based).
#[test]
fn for_head_variable_duplicate() {
    let (root, diagnostics) =
        analyze_files(&[("test.dart", "void f(){ for (int i=0,i=0;;) {} }\n")]);
    assert_eq!(
        dup_rows(&diagnostics),
        vec![(23, 1, "The name 'i' is already defined.".to_string())],
        "the second `i` in the for-head list reports: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `void f(){ var (a,b) = (0,0); int a; }` — a pattern-declaration
/// element joins the enclosing statement-list scope; a local variable
/// of the same name in the same scope reports at the second
/// declaration. The oracle reports column 34 on line 1 (1-based); the
/// byte offset is 34 − 1 = 33 (0-based).
#[test]
fn pattern_decl_element_collides_with_local() {
    let (root, diagnostics) =
        analyze_files(&[("test.dart", "void f(){ var (a,b) = (0,0); int a; }\n")]);
    assert_eq!(
        dup_rows(&diagnostics),
        vec![(33, 1, "The name 'a' is already defined.".to_string())],
        "the `int a` after the pattern-declared `a` reports: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `void f(){ int x; void x() {} }` — a local function declaration
/// joins the enclosing statement-list scope; a local variable of the
/// same name reports at the second declaration. The oracle reports
/// column 23 on line 1 (1-based); the byte offset is 23 − 1 = 22
/// (0-based).
#[test]
fn local_function_collides_with_local_variable() {
    let (root, diagnostics) = analyze_files(&[("test.dart", "void f(){ int x; void x() {} }\n")]);
    assert_eq!(
        dup_rows(&diagnostics),
        vec![(22, 1, "The name 'x' is already defined.".to_string())],
        "the `void x()` after `int x` reports: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `void f(int x){ int x; }` — a formal parameter and a local variable
/// in the same body: the oracle does NOT report this (the local
/// shadows the formal, a legal Dart pattern). Pin the silence.
#[test]
fn formal_parameter_and_local_no_report() {
    let (root, diagnostics) = analyze_files(&[("test.dart", "void f(int x){ int x; }\n")]);
    assert!(
        dup_rows(&diagnostics).is_empty(),
        "a local shadowing a formal is legal: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `void f(){ { int x; } { int x; } }` — sibling blocks own separate
/// statement-list scopes; the oracle reports nothing.
#[test]
fn sibling_blocks_no_report() {
    let (root, diagnostics) =
        analyze_files(&[("test.dart", "void f(){ { int x; } { int x; } }\n")]);
    assert!(
        dup_rows(&diagnostics).is_empty(),
        "sibling blocks are separate scopes: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `void f(){ int x; { int x; } }` — an inner block's declaration
/// legally shadows the outer scope's. The oracle reports nothing.
#[test]
fn nested_shadowing_no_report() {
    let (root, diagnostics) = analyze_files(&[("test.dart", "void f(){ int x; { int x; } }\n")]);
    assert!(
        dup_rows(&diagnostics).is_empty(),
        "inner-block shadowing is legal: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `void f(int v){ switch (v) { case 1: int x; int x; default: break; }
/// }` — each switch member's statement list owns one duplicate scope.
/// The oracle reports column 48 on line 1 (1-based); the byte offset is
/// 48 − 1 = 47 (0-based).
#[test]
fn switch_member_statement_list_duplicate() {
    let (root, diagnostics) = analyze_files(&[(
        "test.dart",
        "void f(int v){ switch (v) { case 1: int x; int x; default: break; } }\n",
    )]);
    assert_eq!(
        dup_rows(&diagnostics),
        vec![(47, 1, "The name 'x' is already defined.".to_string())],
        "the second `int x` in the same switch member reports: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `class C { int x; int x; }` — an element-model face the
/// `l1::duplicate_diagnostics` bake covers. The fold must drop the
/// re-issued report row (the bake wins) — exactly one row at the second
/// `x`. The oracle reports column 22 on line 1 (1-based); the byte
/// offset is 22 − 1 = 21 (0-based), no double-report.
#[test]
fn class_member_duplicate_stays_green() {
    let (root, diagnostics) = analyze_files(&[("test.dart", "class C { int x; int x; }\n")]);
    assert_eq!(
        dup_rows(&diagnostics),
        vec![(21, 1, "The name 'x' is already defined.".to_string())],
        "the member duplicate fires once (bake, not fold): {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `enum E { a, a }` — an element-model face the bake covers. Exactly
/// one `duplicate_definition` at the second `a`. The oracle reports
/// column 13 on line 1 (1-based); the byte offset is 13 − 1 = 12
/// (0-based).
#[test]
fn enum_constant_duplicate_stays_green() {
    let (root, diagnostics) = analyze_files(&[("test.dart", "enum E { a, a }\n")]);
    let dups = dup_rows(&diagnostics);
    assert_eq!(
        dups,
        vec![(12, 1, "The name 'a' is already defined.".to_string())],
        "the enum constant duplicate fires once: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `typedef A = int; typedef A = String;` — an element-model face the
/// bake covers. Exactly one row at the second `A`. The oracle reports
/// column 26 on line 1 (1-based); the byte offset is 26 − 1 = 25
/// (0-based).
#[test]
fn top_level_typedef_duplicate_stays_green() {
    let (root, diagnostics) =
        analyze_files(&[("test.dart", "typedef A = int; typedef A = String;\n")]);
    assert_eq!(
        dup_rows(&diagnostics),
        vec![(25, 1, "The name 'A' is already defined.".to_string())],
        "the typedef duplicate fires once: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `void f(void g(), void g()) {}` — a formal-parameter duplicate the
/// bake covers (the element model's formal face). Exactly one row at
/// the second `g`. The oracle reports column 23 on line 1 (1-based);
/// the byte offset is 23 − 1 = 22 (0-based).
#[test]
fn formal_parameter_duplicate_stays_green() {
    let (root, diagnostics) = analyze_files(&[("test.dart", "void f(void g(), void g()) {}\n")]);
    assert_eq!(
        dup_rows(&diagnostics),
        vec![(22, 1, "The name 'g' is already defined.".to_string())],
        "the formal duplicate fires once: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
