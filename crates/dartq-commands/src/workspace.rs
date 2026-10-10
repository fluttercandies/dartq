//! The workspace/SDK assembly behind every engine-backed face: SDK root
//! resolution, the three pub faces, package tables, and the engine context.
//! The CLI shells and the daemon entry both consume these — the daemon face
//! takes the assembled [`EngineConfig`] into `Session::new` (#328).

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dartq_engine::semdiag;
use dartq_engine::{Engine, EngineConfig};
use dartq_package::PackageTable;
use dartq_pub::{LockFile, PackageConfig, Pubspec, build_graph};
use dartq_sdk::SdkSnapshot;
use thiserror::Error;

/// Why a workspace (or its SDK) cannot back an engine context: the CLI's
/// usage tier. The message body is the printed text (the caller prefixes
/// `error: `), byte-identical to the pre-extraction CLI messages.
#[derive(Debug, Error)]
#[error("{0}")]
pub struct WorkspaceError(pub String);

/// The usage-tier workspace assembly shared by the engine-backed faces:
/// SDK root resolution, canonicalized package directory (printed paths are
/// symlink-resolved, the oracle convention), the three pub faces, package
/// tables, SDK snapshot, and the engine context.
///
/// # Errors
///
/// [`WorkspaceError`] when the SDK, the package directory, or the pub
/// faces are unusable.
pub fn open_engine(dir: &Path, sdk: Option<&Path>) -> Result<(Engine, PathBuf), WorkspaceError> {
    let (config, dir) = engine_config(dir, sdk)?;
    let engine = Engine::new(config)
        .map_err(|error| WorkspaceError(format!("engine context unusable: {error}")))?;
    Ok((engine, dir))
}

/// The assembled [`EngineConfig`] behind [`open_engine`], plus the
/// canonicalized package directory.
///
/// # Errors
///
/// [`WorkspaceError`] when the SDK, the package directory, or the pub
/// faces are unusable.
pub fn engine_config(dir: &Path, sdk: Option<&Path>) -> Result<(EngineConfig, PathBuf), WorkspaceError> {
    let sdk_root = sdk
        .map(Path::to_path_buf)
        .or_else(|| env::var_os("DART_SDK").map(PathBuf::from));
    let Some(sdk_root) = sdk_root else {
        return Err(WorkspaceError(
            "no Dart SDK root (pass --sdk PATH or set DART_SDK)".into(),
        ));
    };
    // Canonicalize up front so the printed paths are symlink-resolved (the
    // oracle convention) and package roots derived from `config_dir` inherit
    // the canonical form.
    let dir = dir.canonicalize().map_err(|_| {
        WorkspaceError(format!(
            "Directory or file doesn't exist: {}",
            dir.display()
        ))
    })?;
    let config_dir = dir.join(".dart_tool");
    let (config, lock, pubspec) = load_workspace_faces(&dir, &config_dir)
        .map_err(WorkspaceError)?;
    let graph = build_graph(&config, &lock, &pubspec, &BTreeMap::new(), None, &config_dir)
        .map_err(|error| WorkspaceError(error.to_string()))?;
    let packages = PackageTable::build(&config, &graph, &config_dir)
        .map_err(|error| WorkspaceError(error.to_string()))?;
    let sdk_snapshot = SdkSnapshot::load(&sdk_root, None).map_err(|error| {
        WorkspaceError(format!(
            "Dart SDK at {} unusable: {error}",
            sdk_root.display()
        ))
    })?;
    Ok((
        EngineConfig {
            store: dartq_engine::memory_store(),
            sdk: sdk_snapshot,
            packages,
            pub_config: config,
            config_dir,
            // The semantic families are part of the analyze contract (#236):
            // `undefined_*` / `wrong_number_of_*` ride the default diagnoser.
            semdiag: Some(semdiag::diagnoser()),
        },
        dir,
    ))
}

/// The three pub faces the engine's context derives from (the same assembly
/// the engine integration fixture pins): `package_config.json`,
/// `pubspec.lock`, `pubspec.yaml` — `dir` already canonicalized.
///
/// # Errors
///
/// A human-readable message naming the missing or malformed face.
pub fn load_workspace_faces(
    dir: &Path,
    config_dir: &Path,
) -> Result<(Arc<PackageConfig>, LockFile, Pubspec), String> {
    let config_text = fs::read_to_string(config_dir.join("package_config.json")).map_err(|_| {
        format!(
            "no .dart_tool/package_config.json under {} (run `dart pub get`)",
            dir.display()
        )
    })?;
    let config = PackageConfig::parse(&config_text).map_err(|error| error.to_string())?;
    let lock_text = fs::read_to_string(dir.join("pubspec.lock")).map_err(|_| {
        format!(
            "no pubspec.lock under {} (run `dart pub get`)",
            dir.display()
        )
    })?;
    let lock = LockFile::parse(&lock_text).map_err(|error| error.to_string())?;
    let pubspec_text = fs::read_to_string(dir.join("pubspec.yaml"))
        .map_err(|_| format!("no pubspec.yaml under {}", dir.display()))?;
    let pubspec = Pubspec::parse(&pubspec_text).map_err(|error| error.to_string())?;
    Ok((Arc::new(config), lock, pubspec))
}
