//! Ticket #2557: `AnalysisOptions::locate_nested` must terminate on a
//! cyclic directory graph. The production hang was `dartq analyze` on a
//! file under a cyclic-symlink root: the nested-options walk inherited the
//! official locator's obligation (`context_locator.dart:384-388` — the
//! `visited.add(canonicalFolderPath)` break) without the guard itself, so a
//! directory reachable through infinitely many spellings (symlink,
//! bind-mount, firmlink) kept the queue non-empty forever.
//!
//! The fix mirrors the official posture: each directory entered is
//! canonicalized once at pop and the canonical key joins a `visited` set —
//! a repeat stops the descent — plus a hard depth cap as the second net
//! for the residual case (canonicalize fails while `read_dir` still
//! yields). These pins exercise the observable contract through
//! [`AnalysisOptions::for_roots`]: cyclic-link roots return promptly,
//! nested options still resolve, and the depth cap bounds descent.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use dartq_engine::analysis_options::AnalysisOptions;
use dartq_pub::PackageConfig;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A throwaway directory removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "dartq-2557-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        TempDir(dir)
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }

    fn write(&self, relative: &str, content: &str) {
        let path = self.path(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(path, content).expect("write");
    }

    fn mkdir(&self, relative: &str) {
        std::fs::create_dir_all(self.path(relative)).expect("mkdir");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A package config with no packages (the options walk needs none).
fn empty_config() -> PackageConfig {
    PackageConfig::parse(r#"{"configVersion": 2, "packages": []}"#).expect("empty config")
}

/// A cyclic-symlink root: `loop -> .` and `a/parent -> ..` make the real
/// directory reachable through infinitely many spellings. `locate_nested`
/// never descends a link (`entry.file_type` reports link-host facts), but
/// the canonical visited set is the guard that keeps any link-shaped cycle
/// — and the bind-mount loops it also keys — finite; the pin is that the
/// call returns promptly with the real nested options still registered.
#[cfg(unix)]
#[test]
fn cyclic_symlink_root_terminates() {
    let dir = TempDir::new("cycle");
    dir.write(
        "analysis_options.yaml",
        "linter:\n  rules:\n    - root_rule\n",
    );
    dir.write(
        "a/nested/analysis_options.yaml",
        "linter:\n  rules:\n    - nested_rule\n",
    );
    std::os::unix::fs::symlink(".", dir.path("loop")).expect("self link");
    std::os::unix::fs::symlink("..", dir.path("a/parent")).expect("parent link");
    std::os::unix::fs::symlink("../a", dir.path("a/sibling")).expect("sibling link");

    let start = Instant::now();
    let per_root = AnalysisOptions::for_roots(&[dir.path("")], &empty_config(), &dir.0);
    assert!(
        start.elapsed() < Duration::from_secs(30),
        "the cyclic root resolved in {:?}",
        start.elapsed()
    );

    assert_eq!(
        per_root.get(&dir.path("")).expect("root").lint_rules,
        ["root_rule"]
    );
    assert_eq!(
        per_root
            .get(&dir.path("a/nested"))
            .expect("nested entry")
            .lint_rules,
        ["nested_rule"]
    );
}

/// A root spelled through a symlink resolves the same tree once: the
/// canonical visited key collapses the alias onto the real directory, so
/// the walk enters each real directory exactly once and registers the
/// nested options under the queue spelling.
#[cfg(unix)]
#[test]
fn symlink_spelled_root_resolves_nested_options() {
    let dir = TempDir::new("alias");
    dir.write(
        "real/analysis_options.yaml",
        "linter:\n  rules:\n    - root_rule\n",
    );
    dir.write(
        "real/inner/analysis_options.yaml",
        "linter:\n  rules:\n    - inner_rule\n",
    );
    let alias = dir.path("alias");
    std::os::unix::fs::symlink(dir.path("real"), &alias).expect("alias link");

    let per_root = AnalysisOptions::for_roots(&[alias.clone()], &empty_config(), &dir.0);

    assert_eq!(
        per_root.get(&alias).expect("root entry").lint_rules,
        ["root_rule"]
    );
    assert_eq!(
        per_root
            .get(&alias.join("inner"))
            .expect("nested under the alias spelling")
            .lint_rules,
        ["inner_rule"]
    );
}

/// Regression: an ordinary nested options tree still resolves — every
/// directory carrying a file governs its subtree, dot dirs stay skipped.
#[test]
fn nested_options_still_register() {
    let dir = TempDir::new("nested");
    dir.write(
        "analysis_options.yaml",
        "linter:\n  rules:\n    - root_rule\n",
    );
    dir.write(
        "sub/analysis_options.yaml",
        "linter:\n  rules:\n    - sub_rule\n",
    );
    dir.write(
        ".hidden/analysis_options.yaml",
        "linter:\n  rules:\n    - hidden_rule\n",
    );
    dir.mkdir("plain/leaf");

    let per_root = AnalysisOptions::for_roots(&[dir.path("")], &empty_config(), &dir.0);

    assert_eq!(
        per_root.get(&dir.path("")).expect("root").lint_rules,
        ["root_rule"]
    );
    assert_eq!(
        per_root.get(&dir.path("sub")).expect("sub").lint_rules,
        ["sub_rule"]
    );
    assert!(
        !per_root.contains_key(&dir.path(".hidden")),
        "dot directories are never walked"
    );
}

/// A root with no options file at all walks to its leaves and returns the
/// synthesized default entry — no hang, no spurious entries.
#[test]
fn no_options_walks_to_leaves_and_returns_default() {
    let dir = TempDir::new("plain");
    dir.mkdir("a/b/c");
    dir.mkdir("d");

    let per_root = AnalysisOptions::for_roots(&[dir.path("")], &empty_config(), &dir.0);

    assert_eq!(per_root.len(), 1);
    assert!(per_root.contains_key(&dir.path("")));
}

/// The second termination net: descent stops at the depth cap, so a deeper
/// options file is never registered while a shallower one still is.
#[test]
fn depth_cap_bounds_the_descent() {
    let dir = TempDir::new("deep");
    dir.write(
        "analysis_options.yaml",
        "linter:\n  rules:\n    - root_rule\n",
    );
    // Single-letter components keep the lexical path far under PATH_MAX
    // (~1.2KB at depth 300) while exceeding the 256 descent bound.
    let mut deep = String::new();
    for _ in 0..300 {
        deep.push_str("d/");
    }
    deep.push_str("analysis_options.yaml");
    dir.write(&deep, "linter:\n  rules:\n    - deep_rule\n");
    dir.write(
        "shallow/analysis_options.yaml",
        "linter:\n  rules:\n    - shallow_rule\n",
    );

    let per_root = AnalysisOptions::for_roots(&[dir.path("")], &empty_config(), &dir.0);

    assert_eq!(
        per_root.get(&dir.path("")).expect("root").lint_rules,
        ["root_rule"]
    );
    assert_eq!(
        per_root
            .get(&dir.path("shallow"))
            .expect("shallow entry")
            .lint_rules,
        ["shallow_rule"]
    );
    let deep_dir = deep.trim_end_matches("analysis_options.yaml");
    let deep_dir = deep_dir.trim_end_matches('/');
    assert!(
        !per_root.contains_key(&dir.path(deep_dir)),
        "depth > 256 is never descended"
    );
}
