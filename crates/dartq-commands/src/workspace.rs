//! The workspace/SDK assembly behind every engine-backed face: SDK root
//! resolution, pub workspace resolution, the three pub faces, package
//! tables, and the engine context. The CLI shells and the daemon entry both
//! consume these — the daemon face takes the assembled [`EngineConfig`]
//! into `Session::new` (#328).

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dartq_engine::semdiag;
use dartq_engine::{Engine, EngineConfig};
use dartq_package::PackageTable;
use dartq_pub::{LockFile, PackageConfig, Pubspec, build_graph, find_workspace_root};
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
/// The directory is resolved through pub's workspace discovery first
/// ([`find_workspace_root`]): pointing at a workspace member analyzes the
/// root's artifacts over the full member set, and the root pubspec carries
/// its `pubspec_overrides.yaml`. A standalone package resolves to itself,
/// byte-identical to the pre-workspace assembly.
///
/// # Errors
///
/// [`WorkspaceError`] when the SDK, the package directory, or the pub
/// faces are unusable.
pub fn engine_config(
    dir: &Path,
    sdk: Option<&Path>,
) -> Result<(EngineConfig, PathBuf), WorkspaceError> {
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
    let workspace = find_workspace_root(&dir).map_err(|error| WorkspaceError(error.to_string()))?;
    let config_dir = workspace.root.join(".dart_tool");
    let (config, lock, pubspec) =
        load_workspace_faces(&workspace.root, &config_dir).map_err(WorkspaceError)?;
    // Member pubspecs ride the graph as `extra_pubspecs` (member versions
    // and dependency edges); their names are the workspace member set.
    // `find_workspace_root` already loaded and validated every member.
    let mut extra_pubspecs: BTreeMap<String, Pubspec> = BTreeMap::new();
    let mut member_names: Vec<String> = Vec::new();
    for member in &workspace.members {
        let spec = load_pubspec(member).map_err(WorkspaceError)?;
        member_names.push(spec.name.clone());
        extra_pubspecs.insert(spec.name.clone(), spec);
    }
    // `workspace_members: None` outside a pub workspace — the standalone
    // package table must stay byte-identical to the pre-workspace shape.
    let workspace_members = (!pubspec.workspace.is_empty()).then_some(member_names.as_slice());
    let graph = build_graph(
        &config,
        &lock,
        &pubspec,
        &extra_pubspecs,
        workspace_members,
        &config_dir,
    )
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
            target: dartq_engine::DEFAULT_TARGET.to_owned(),
            declared_variables: rustc_hash::FxHashMap::default(),
        },
        dir,
    ))
}

/// The three pub faces the engine's context derives from (the same assembly
/// the engine integration fixture pins): `package_config.json`,
/// `pubspec.lock`, `pubspec.yaml` — `dir` already canonicalized and, on the
/// engine path, the resolved workspace root.
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
    let pubspec = load_pubspec(dir)?;
    Ok((Arc::new(config), lock, pubspec))
}

/// Loads `<dir>/pubspec.yaml` with its optional `pubspec_overrides.yaml`
/// applied — pub walks with `allowOverridesFile: true`
/// (entrypoint.dart:95-101), so every pubspec the engine consumes is
/// overrides-aware (`workspace`/`resolution` precedence, the whole
/// `dependency_overrides` section).
///
/// # Errors
///
/// A human-readable message naming the missing file or the parse error.
fn load_pubspec(dir: &Path) -> Result<Pubspec, String> {
    let text = fs::read_to_string(dir.join("pubspec.yaml"))
        .map_err(|_| format!("no pubspec.yaml under {}", dir.display()))?;
    let overrides_path = dir.join("pubspec_overrides.yaml");
    let overrides = if overrides_path.is_file() {
        Some(
            fs::read_to_string(&overrides_path)
                .map_err(|error| format!("pubspec_overrides.yaml: {error}"))?,
        )
    } else {
        None
    };
    Pubspec::parse_with_overrides(&text, overrides.as_deref()).map_err(|error| error.to_string())
}

#[cfg(test)]
pub(crate) mod test_fixture {
    //! The throwaway pub workspace + synthetic dart:core-only SDK the
    //! engine-backed command tests analyze — the same assembly the CLI
    //! tests' `analyze_fixture` pins and the dartq-engine integration
    //! fixture (tests/common/mod.rs) shapes.

    use std::fs;
    use std::path::PathBuf;

    /// Writes the workspace faces plus `libraries` (path relative to the
    /// root, content) and returns the root. `tag` namespaces the temp dir.
    pub(crate) fn workspace(tag: &str, libraries: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("dq_cmd_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let write = |relative: &str, content: &str| {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            fs::write(path, content).expect("file");
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
        for (relative, content) in libraries {
            write(relative, content);
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use super::test_fixture::workspace;
    use super::*;

    const MEMBER_PUBSPEC: &str =
        "name: a\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n";
    // Overrides may not target workspace packages (package.dart:531-555),
    // so the override names a package outside the member set — the payload
    // the pre-workspace assembly silently dropped.
    const ROOT_OVERRIDES: &str = "dependency_overrides:\n  ext:\n    path: ../ext\n";

    /// Turns the standalone fixture into a pub workspace: root declares
    /// `packages/a`, the member opts in, and the root's overrides file
    /// carries a path override — the faces `engine_config` must honor.
    fn member_fixture(tag: &str) -> PathBuf {
        let root = workspace(tag, &[]);
        fs::write(
            root.join("pubspec.yaml"),
            "name: fixture\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - packages/a\n",
        )
        .expect("root pubspec");
        fs::write(root.join("pubspec_overrides.yaml"), ROOT_OVERRIDES).expect("overrides");
        fs::create_dir_all(root.join("packages/a")).expect("member dir");
        fs::write(root.join("packages/a/pubspec.yaml"), MEMBER_PUBSPEC).expect("member pubspec");
        fs::write(
            root.join(".dart_tool/package_config.json"),
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "languageVersion": "3.5"}, {"name": "a", "rootUri": "../packages/a", "languageVersion": "3.5"}], "generator": "pub"}"#,
        )
        .expect("config");
        root
    }

    /// The workspace-assembly pin: pointing at a MEMBER assembles the
    /// ROOT's artifacts over the resolved member set, with the root's
    /// `pubspec_overrides.yaml` applied — the shapes the pre-workspace
    /// assembly dropped (a misleading `dart pub get` error, an
    /// override-free graph).
    #[test]
    fn member_dir_assembles_over_the_root_workspace() {
        let root = member_fixture("ws_member");
        let member = root.join("packages/a");
        let (config, dir) = engine_config(&member, Some(&root.join("sdk"))).expect("assembly");
        let root = root.canonicalize().expect("root");

        assert_eq!(dir, root.join("packages/a"), "the pointed-at dir");
        assert_eq!(
            config.config_dir,
            root.join(".dart_tool"),
            "the root's artifacts"
        );
        assert!(
            config.packages.is_workspace_member("a"),
            "the graph carries the member set"
        );

        let (_, _, pubspec) = load_workspace_faces(&root, &root.join(".dart_tool")).expect("faces");
        assert!(
            pubspec.dependency_overrides.contains_key("ext"),
            "the root pubspec is overrides-aware"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// The error texts that survive the workspace routing keep their exact
    /// bytes: a standalone package without `dart pub get`, and pub's own
    /// no-root message when no pubspec exists on the walk.
    #[test]
    fn workspace_error_texts_stay_stable() {
        let root = workspace("ws_pubget", &[]);
        fs::remove_file(root.join(".dart_tool/package_config.json")).expect("drop config");
        let Err(error) = engine_config(&root, Some(&root.join("sdk"))) else {
            panic!("no pub get must fail")
        };
        assert_eq!(
            error.to_string(),
            format!(
                "no .dart_tool/package_config.json under {} (run `dart pub get`)",
                root.canonicalize().expect("root").display()
            )
        );
        fs::remove_dir_all(&root).ok();

        let bare = std::env::temp_dir().join(format!("dq_cmd_ws_bare_{}", std::process::id()));
        fs::create_dir_all(&bare).expect("bare dir");
        let Err(error) = engine_config(&bare, Some(&bare.join("sdk"))) else {
            panic!("no pubspec must fail")
        };
        assert_eq!(
            error.to_string(),
            format!(
                "workspace: Found no `pubspec.yaml` file in `{}` or parent directories",
                bare.canonicalize().expect("bare").display()
            )
        );
        fs::remove_dir_all(&bare).ok();
    }
}
