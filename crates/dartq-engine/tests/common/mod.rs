//! Shared integration-test fixture: a throwaway pub workspace + synthetic
//! VM-segment SDK, assembled through the real dartq-pub/dartq-package/
//! dartq-sdk faces (the engine-api-spec §2.2 M1 assembly, minus the CLI).

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use dartq_engine::{Engine, EngineConfig, memory_store};
use dartq_package::PackageTable;
use dartq_pub::{LockFile, PackageConfig, Pubspec, build_graph};
use dartq_sdk::SdkSnapshot;

/// Defining library importing a sibling (`lib/main.dart` → `lib/greeter.dart`).
pub const MAIN_0: &str = "import 'greeter.dart';\n\nvoid main() {\n  print(greet());\n}\n";
/// The imported library (valid — no diagnostics expected from the pair).
pub const GREETER_0: &str = "String greet() => 'hi';\n";
/// Edited import target: same shape, different bytes (ContentId gate must pass).
pub const GREETER_1: &str = "String greet() => 'hello';\n";
/// Standalone library: imports nothing, imported by nothing — it never enters
/// any cascade, so it serves as the clean analysis target.
pub const STANDALONE_0: &str = "int standalone() => 3;\n";

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A throwaway workspace: root package `fixture` with the given `lib/` files
/// and a synthetic `dart:core`-only SDK.
pub struct Fixture {
    pub root: PathBuf,
    pub config: Arc<PackageConfig>,
    pub config_dir: PathBuf,
    pub packages: PackageTable,
}

impl Fixture {
    pub fn new(libraries: &[(&str, &str)]) -> Self {
        Self::with_language_version(libraries, "3.5")
    }

    /// Same workspace with a different package language version (the
    /// `package_config.json` entry the engine derives per-file versions
    /// from).
    pub fn with_language_version(libraries: &[(&str, &str)], language_version: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "dartq-engine-test-{}-{}",
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
            "name: fixture\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(
            "pubspec.lock",
            "packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n",
        );
        write(
            ".dart_tool/package_config.json",
            &format!(
                r#"{{"configVersion": 2, "packages": [{{"name": "fixture", "rootUri": "../", "languageVersion": "{language_version}"}}], "generator": "pub"}}"#
            ),
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
            Pubspec::parse("name: fixture\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n")
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
        Self {
            root,
            config,
            config_dir,
            packages,
        }
    }

    /// Path under the workspace.
    pub fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// A fresh engine over the fixture (SDK reloads per engine — the
    /// snapshot is not `Clone`, and loading is a few metadata reads).
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

/// Collects events so tests can assert work order and counts.
#[derive(Debug, Default)]
pub struct EventLog {
    pub overlay_applied: Vec<(usize, usize)>,
    pub revalidated: Vec<(dartq_source::FileId, dartq_engine::RevalidateGate)>,
    pub analyzed: Vec<(dartq_engine::LibraryId, bool)>,
    pub cascades: Vec<Vec<dartq_engine::LibraryId>>,
}

impl EventLog {
    pub fn sink(&mut self) -> impl FnMut(dartq_engine::EngineEvent) + '_ {
        |event| match event {
            dartq_engine::EngineEvent::OverlayApplied { changed, removed } => {
                self.overlay_applied.push((changed.len(), removed.len()));
            }
            dartq_engine::EngineEvent::FileRevalidated { file, gate } => {
                self.revalidated.push((file, gate));
            }
            dartq_engine::EngineEvent::LibraryAnalyzed {
                library,
                from_cache,
                ..
            } => {
                self.analyzed.push((library, from_cache));
            }
            dartq_engine::EngineEvent::CascadeInvalidated { libraries } => {
                self.cascades.push(libraries);
            }
        }
    }
}

/// Canonical diagnostic summary the way the oracle compares: path tail,
/// code name, offset — enough to pin ordering without pinning messages.
pub fn diagnostic_summary(
    engine: &Engine,
    diagnostics: &[dartq_diagnostics::Diagnostic],
) -> Vec<String> {
    diagnostics
        .iter()
        .map(|diagnostic| {
            let path = engine
                .file_table()
                .path(diagnostic.file)
                .map(Path::to_string_lossy)
                .map(|p| p.rsplit('/').next().unwrap_or(&p).to_owned())
                .unwrap_or_default();
            format!(
                "{}|{}|{}|{}",
                diagnostic.severity, path, diagnostic.code.name, diagnostic.range.offset
            )
        })
        .collect()
}
