//! Shared §19.3 benchmark fixture: a throwaway pub workspace + synthetic
//! SDK + the engine assembly the `dartq check` pipeline runs (§2.2).
//! Execution is deferred to the serial phase; these skeletons exist so the
//! metric extraction code is pinned and reviewable (PRD §19.3).

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dartq_engine::{Engine, EngineConfig, memory_store};
use dartq_package::PackageTable;
use dartq_pub::{LockFile, PackageConfig, Pubspec, build_graph};
use dartq_sdk::SdkSnapshot;

/// Three libraries in an import chain a → b → c, so incremental edits
/// cascade through the reverse-edge closure.
pub const LIB_A: &str = "import 'b.dart';\n\nvoid topA() {\n  print(libB());\n}\n";
pub const LIB_B: &str = "import 'c.dart';\n\nString libB() => libC();\n";
pub const LIB_C: &str = "String libC() => 'c';\n";

/// The libraries the incremental bench edits (comment churn: same shape,
/// different bytes — the L1 cascade hits, the ContentId gate passes).
pub const LIB_A_EDITED: &str =
    "import 'b.dart';\n\n// edited\nvoid topA() {\n  print(libB());\n}\n";

/// A throwaway workspace with a synthetic VM-segment SDK.
pub struct Fixture {
    pub root: PathBuf,
    pub config: Arc<PackageConfig>,
    pub config_dir: PathBuf,
    pub packages: PackageTable,
    pub sdk: SdkSnapshot,
}

impl Fixture {
    /// Writes the workspace (pubspec + lock + package config + SDK) and the
    /// given `lib/` files to a fresh temp dir.
    pub fn new(libraries: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!(
            "dartq-engine-bench-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let write = |relative: &str, content: &str| {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(path, content).expect("file");
        };

        write(
            "pubspec.yaml",
            "name: bench\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(
            "pubspec.lock",
            "packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n",
        );
        write(
            ".dart_tool/package_config.json",
            r#"{"configVersion": 2, "packages": [{"name": "bench", "rootUri": "../", "languageVersion": "3.5"}], "generator": "pub"}"#,
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
        for (relative, content) in libraries {
            write(relative, content);
        }

        let config_dir = root.join(".dart_tool");
        let config = Arc::new(
            PackageConfig::parse(
                &std::fs::read_to_string(config_dir.join("package_config.json")).expect("config"),
            )
            .expect("valid config"),
        );
        let lock = LockFile::parse("packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n")
            .expect("valid lock");
        let root_pubspec =
            Pubspec::parse("name: bench\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n")
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
        let sdk = SdkSnapshot::load(&root.join("sdk"), None).expect("synthetic SDK loads");
        Self {
            root,
            config,
            config_dir,
            packages,
            sdk,
        }
    }

    /// A fresh engine over the fixture (the SDK snapshot reloads — it is
    /// not `Clone`, and loading is a few small metadata reads).
    pub fn engine(&self) -> Engine {
        Engine::new(EngineConfig {
            store: memory_store(),
            sdk: SdkSnapshot::load(&self.root.join("sdk"), None).expect("synthetic SDK loads"),
            packages: self.packages.clone(),
            pub_config: Arc::clone(&self.config),
            config_dir: self.config_dir.clone(),
            semdiag: None,
        })
        .expect("engine")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Percentile of a sorted sample (nearest-rank).
pub fn percentile(sorted: &[Duration], p: f64) -> Duration {
    let index = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

/// Milliseconds with one decimal.
pub fn millis(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// A simple wall-clock helper for phase timing.
pub fn stopwatch() -> Instant {
    Instant::now()
}
