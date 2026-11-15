//! The daemon face's whole-root analyzed-set contract (#4315): the
//! session's analyzed set is the `analysis.setAnalysisRoots` equivalent —
//! every package root inside the workspace bound plus each discovered
//! nested self-resolved context root (own `pubspec.yaml` +
//! `.dart_tool/package_config.json`), each carved out of its enclosing
//! root's walk — never the `dart analyze <target>` scope the CLI face
//! filters to. These tests pin that contract on a synthetic nested
//! workspace assembled through the real `dartq-commands` assembly
//! (`extend_config_with_nested_roots` produces the `EngineConfig`'s
//! `context_roots`): a root-wide file outside every member's scope still
//! lands in the publish pushes, and a file under a nested context root's
//! own options file reads that file's strict modes — the
//! deepest-governing-options rule.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use dartq_daemon::Session;
use dartq_snapshot::{DEFAULT_CAPACITY_BYTES, SnapshotConfig, SnapshotStore};

/// Writes `relative` under `root`, creating parents.
fn write(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    std::fs::write(path, content).expect("file");
}

/// A throwaway nested workspace tree: the `outer` pub-workspace root
/// (one `pkgs/member` member), a nested self-resolved `inner` package
/// beside it (own pubspec + own `.dart_tool/package_config.json` — the
/// oracle's separate-context-root trigger), a root-wide `extras/` surface
/// no member owns, and the same synthetic VM-segment SDK the shared
/// fixture builds.
struct Tree {
    root: PathBuf,
}

impl Tree {
    fn new() -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "dartq-daemon-wras-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Relaxed),
        ));
        let _ = std::fs::remove_dir_all(&root);

        // The workspace root package and its member set.
        write(
            &root,
            "pubspec.yaml",
            "name: outer\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - pkgs/member\n",
        );
        write(
            &root,
            "pubspec.lock",
            "packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n",
        );
        write(
            &root,
            ".dart_tool/package_config.json",
            r#"{"configVersion": 2, "packages": [{"name": "outer", "rootUri": "../", "packageUri": "lib/", "languageVersion": "3.5"}, {"name": "member", "rootUri": "../pkgs/member", "packageUri": "lib/", "languageVersion": "3.5"}], "generator": "pub", "generatorVersion": "3.13.1"}"#,
        );
        write(&root, "lib/main.dart", "void main() {}\n");
        write(
            &root,
            "pkgs/member/pubspec.yaml",
            "name: member\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        );
        write(&root, "pkgs/member/lib/member.dart", "void member() {}\n");
        // The root-wide file a `dart analyze pkgs/member`-scoped pass never
        // reaches: a `.dart` surface outside every member carrying a bidi
        // code point in a string literal (the unconditional unicode
        // verifier's row).
        write(&root, "extras/bidi.dart", "var s = 'a\u{202E}b';\n");

        // The nested self-resolved context root: own pubspec + own
        // package_config — `extend_config_with_nested_roots` discovers it
        // and the locator's `packageConfigFileToUse !=
        // containingRoot.packagesFile` split gives it its own context.
        write(
            &root,
            "inner/pubspec.yaml",
            "name: inner\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(
            &root,
            "inner/pubspec.lock",
            "packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n",
        );
        write(
            &root,
            "inner/.dart_tool/package_config.json",
            r#"{"configVersion": 2, "packages": [{"name": "inner", "rootUri": "../", "packageUri": "lib/", "languageVersion": "3.5"}], "generator": "pub", "generatorVersion": "3.13.1"}"#,
        );
        // The nested context root's own options: `strict-raw-types`
        // governs this subtree only — the deepest-governing rule.
        write(
            &root,
            "inner/analysis_options.yaml",
            "analyzer:\n  language:\n    strict-raw-types: true\n",
        );
        write(&root, "inner/lib/raw.dart", "class C<T> {}\nC c = C();\n");
        write(&root, "inner/lib/inner.dart", "void inner() {}\n");

        // The same synthetic SDK the shared fixture writes.
        write(&root, "sdk/version", "3.13.1\n");
        write(
            &root,
            "sdk/lib/libraries.json",
            r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}}}}"#,
        );
        write(
            &root,
            "sdk/lib/_internal/allowed_experiments.json",
            r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
        );

        // macOS /var → /private/var: the engine's package table
        // canonicalizes its config dir, so every assertion derives from
        // the canonical root.
        let root = root.canonicalize().expect("root canonicalizes");
        Self { root }
    }
}

/// One session over the tree: the real `dartq-commands` assembly produces
/// the `EngineConfig` (the nested-root discovery included), the snapshot
/// store pins a throwaway root.
fn session_over(root: &Path, store_name: &str) -> Session {
    let (config, _dir, advisory) =
        dartq_commands::workspace::engine_config_with_advisory(root, Some(&root.join("sdk")))
            .expect("the fixture assembles");
    let store = SnapshotStore::open(SnapshotConfig {
        root: Some(std::env::temp_dir().join(format!(
            "dartq-daemon-wras-store-{}-{store_name}",
            std::process::id(),
        ))),
        capacity_bytes: DEFAULT_CAPACITY_BYTES,
    })
    .expect("store opens on a temp root");
    let mut session = Session::with_store(config, store, advisory).expect("session");
    session.sweep().expect("sweep");
    session
}

/// The publish pushes' uri → code-names map, owned.
fn published(session: &mut Session) -> Vec<(String, Vec<String>)> {
    let turn = session.service();
    turn.publishes
        .iter()
        .map(|push| {
            (
                push.uri.clone(),
                push.diagnostics
                    .iter()
                    .map(|row| row.code.clone())
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

/// The contract, end to end: the session's first drain publishes the
/// whole analyzed set — the root-wide `extras/` file's unicode-verifier
/// row and the nested context root's own-options strict-mode row both
/// land where a member-scoped `dart analyze` pass would surface neither.
#[test]
fn the_analyzed_set_is_the_whole_root_walk() {
    let tree = Tree::new();
    let mut session = session_over(&tree.root, "set");
    let pushes = published(&mut session);

    let uri_of = |relative: &str| dartq_commands::uri::from_path(&tree.root.join(relative));
    let codes_for = |uri: &str| -> Vec<String> {
        pushes
            .iter()
            .find(|(push_uri, _)| push_uri == uri)
            .map(|(_, codes)| codes.clone())
            .unwrap_or_default()
    };

    // Root-wide, outside every member's scope: the unconditional
    // verifier's row proves the file is in the analyzed set — a
    // member-scoped pass never walks `extras/`.
    let extras = codes_for(&uri_of("extras/bidi.dart"));
    assert!(
        extras
            .iter()
            .any(|code| code == "text_direction_code_point_in_literal"),
        "the whole-root set covers the extras surface: {pushes:?}"
    );

    // The nested context root's own options govern its subtree:
    // `strict-raw-types: true` fires the raw-annotation row inside
    // `inner/` — the deepest-governing-options rule — while the root's
    // own files stay under the root posture (no such options file).
    let raw = codes_for(&uri_of("inner/lib/raw.dart"));
    assert!(
        raw.iter().any(|code| code == "strict_raw_type"),
        "the nested context root's own options govern its files: {pushes:?}"
    );
    let main = codes_for(&uri_of("lib/main.dart"));
    assert!(
        main.iter().all(|code| code != "strict_raw_type"),
        "the nested options never leak out of their context root: {pushes:?}"
    );

    std::fs::remove_dir_all(&tree.root).ok();
}
