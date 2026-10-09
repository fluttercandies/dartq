//! Shared test fixture: a throwaway pub workspace + synthetic `dart:core`
//! VM-segment SDK, assembled through the real dartq-pub/dartq-package/
//! dartq-sdk faces (the same assembly the CLI's `open_engine` performs).

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;

use dartq_engine::{Engine, EngineConfig, memory_store};
use dartq_package::PackageTable;
use dartq_pub::{LockFile, PackageConfig, Pubspec, build_graph};
use dartq_sdk::SdkSnapshot;

/// Seeded disk content: one resolvable URI-layer error (`uri_does_not_exist`),
/// deterministic under the synthetic SDK.
pub const BROKEN_MAIN: &str = "import 'missing.dart';\nvoid main() {}\n";
/// Clean replacement pushed through didChange.
pub const CLEAN_MAIN: &str = "void main() {}\n";

/// A throwaway single-package workspace with `lib/main.dart` on disk.
pub struct Workspace {
    pub root: PathBuf,
    pub config: Arc<PackageConfig>,
    pub config_dir: PathBuf,
    pub packages: PackageTable,
}

impl Workspace {
    #[must_use]
    pub fn new(main_dart: &str) -> Self {
        let root = std::env::temp_dir().join(format!("dartq-daemon-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |relative: &str, content: &str| {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(path, content).expect("file");
        };
        write(
            "pubspec.yaml",
            "name: fixture\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(
            "pubspec.lock",
            "packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n",
        );
        write(
            ".dart_tool/package_config.json",
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "languageVersion": "3.5"}], "generator": "pub"}"#,
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
        write("lib/main.dart", main_dart);

        // macOS /var → /private/var: the engine's package table canonicalizes
        // its config dir, so every face and every test assertion must derive
        // from the canonical root or the URIs disagree.
        let root = root.canonicalize().expect("root canonicalizes");
        let config_dir = root.join(".dart_tool");
        let config = Arc::new(
            PackageConfig::parse(
                &std::fs::read_to_string(config_dir.join("package_config.json")).expect("config"),
            )
            .expect("valid config"),
        );
        let lock =
            LockFile::parse("packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n").expect("lock");
        let pubspec =
            Pubspec::parse("name: fixture\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n")
                .expect("pubspec");
        let graph = build_graph(
            &config,
            &lock,
            &pubspec,
            &std::collections::BTreeMap::new(),
            None,
            &config_dir,
        )
        .expect("graph");
        let packages = PackageTable::build(&config, &graph, &config_dir).expect("table");
        Self {
            root,
            config,
            config_dir,
            packages,
        }
    }

    /// The workspace's canonical root.
    #[must_use]
    pub fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// A fresh engine over the workspace, semantic diagnoser on (the same
    /// posture the CLI's analyze path ships).
    #[must_use]
    pub fn engine(&self) -> Engine {
        Engine::new(EngineConfig {
            store: memory_store(),
            sdk: SdkSnapshot::load(&self.root.join("sdk"), None).expect("synthetic SDK loads"),
            packages: self.packages.clone(),
            pub_config: Arc::clone(&self.config),
            config_dir: self.config_dir.clone(),
            semdiag: Some(dartq_engine::semdiag::diagnoser()),
        })
        .expect("engine")
    }
}

/// Decodes the framed transcript back into JSON values.
#[must_use]
pub fn frames(bytes: &[u8]) -> Vec<serde_json::Value> {
    let mut values = Vec::new();
    let mut rest: &str = std::str::from_utf8(bytes).expect("transcript is UTF-8");
    while let Some(position) = rest.find("Content-Length: ") {
        let after = &rest[position + "Content-Length: ".len()..];
        let line_end = after.find("\r\n").expect("header terminator");
        let length: usize = after[..line_end].parse().expect("Content-Length value");
        let body_start = line_end + "\r\n\r\n".len();
        let body = &after[body_start..body_start + length];
        values.push(serde_json::from_str(body).expect("frame body is JSON"));
        rest = &after[body_start + length..];
    }
    values
}
