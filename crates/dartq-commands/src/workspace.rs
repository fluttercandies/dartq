//! The workspace/SDK assembly behind every engine-backed face: SDK root
//! resolution, pub workspace resolution, the three pub faces, package
//! tables, and the engine context. The CLI shells and the daemon entry both
//! consume these — the daemon face takes the assembled [`EngineConfig`]
//! into `Session::new` (#328).

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dartq_diagnostics::{
    CodeInfo, DARTQ_PACKAGE_CONFIG_INVALID, DARTQ_PACKAGE_CONFIG_MISSING,
    DARTQ_PACKAGE_CONFIG_OUT_OF_DATE,
};
use dartq_engine::semdiag;
use dartq_engine::{Engine, EngineConfig};
use dartq_package::PackageTable;
use dartq_pub::{
    DependencySpec, EnvSnapshot, Environment, Freshness, LanguageVersion, LockFile, PackageConfig,
    PackageConfigEntry, PubError, Pubspec, Resolution, ResolutionBaseline, ResolvedPackageGraph,
    StaleReason, WorkspaceLayout, build_graph, find_workspace_root, normalize,
};
use dartq_sdk::SdkSnapshot;
use thiserror::Error;

/// Why a workspace (or its SDK) cannot back an engine context: the CLI's
/// usage tier. The message body is the printed text (the caller prefixes
/// `error: `), byte-identical to the pre-extraction CLI messages.
#[derive(Debug, Error)]
#[error("{0}")]
pub struct WorkspaceError(pub String);

/// The one non-gating stale-config advisory of an assembly: the
/// `dartq/package_config_out_of_date` TOOL diagnostic, carrying every
/// concrete reason the resolution looks stale — the freshness report plus
/// the graph's membership findings. Output faces render it once per
/// invocation; it never gates the analysis (the oracle analyzes the config
/// as written, and so does dartq).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaleConfigAdvisory {
    /// The concrete reasons, in report order.
    pub reasons: Vec<StaleReason>,
    /// The `package_config.json` the reasons name — the output faces' row
    /// anchor (canonical, the oracle path convention).
    pub config_path: PathBuf,
}

impl StaleConfigAdvisory {
    /// The diagnostic code, pinned to the reserved registry row
    /// (`dartq_diagnostics::code::DARTQ_PACKAGE_CONFIG_OUT_OF_DATE`).
    pub const CODE: &'static str = "dartq/package_config_out_of_date";

    /// One-line message for the output faces.
    #[must_use]
    pub fn message(&self) -> String {
        let reasons = self
            .reasons
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        format!("package_config.json is out of date; run `dart pub get` ({reasons})")
    }
}

/// The one non-gating advisory of a corrupt resolution: the
/// `dartq/package_config_invalid` TOOL diagnostic. The config exists but is
/// unusable — unreadable, malformed JSON, duplicate package names — and the
/// analyze path degrades to the oracle's `Packages.empty` floor instead of
/// refusing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidConfigAdvisory {
    /// The `package_config.json` that failed — the output faces' row anchor
    /// (canonical, the oracle path convention).
    pub config_path: PathBuf,
    /// The concrete failure (the parse/resolve error), verbatim.
    pub reason: String,
}

impl InvalidConfigAdvisory {
    /// The diagnostic code, pinned to the reserved registry row
    /// (`dartq_diagnostics::code::DARTQ_PACKAGE_CONFIG_INVALID`).
    pub const CODE: &'static str = "dartq/package_config_invalid";

    /// One-line message for the output faces.
    #[must_use]
    pub fn message(&self) -> String {
        format!(
            "package_config.json is unusable ({}); analyzing with `package:` \
             imports unresolved",
            self.reason
        )
    }
}

/// The one non-gating workspace advisory of an assembly, riding the #732
/// output channel (one INFO TOOL row per invocation, never part of the
/// per-file set or the exit tier). The stale variant covers a config that
/// exists but looks stale; the missing variant covers degraded mode (#759):
/// no `package_config.json` found, the context opens on the oracle's
/// `BasicWorkspace` floor and every `package:` import stays unresolved — the
/// oracle analyzes, it does not refuse. The invalid variant covers a config
/// that exists but cannot be used: the oracle's parse-failure catch-all
/// (upstream Dart SDK source, pkg/analyzer/lib/src/context/packages.dart
/// `parsePackageConfigJsonFile`) turns any read/parse failure into
/// `Packages.empty` and keeps analyzing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceAdvisory {
    /// The config as written looks stale
    /// (`dartq/package_config_out_of_date`).
    Stale(StaleConfigAdvisory),
    /// No config anywhere (`dartq/package_config_missing`); the payload is
    /// the path `dart pub get` would materialize — the row's anchor.
    MissingConfig(PathBuf),
    /// The config is unusable (`dartq/package_config_invalid`); the floor
    /// is `Packages.empty` plus the self package.
    InvalidConfig(InvalidConfigAdvisory),
}

impl WorkspaceAdvisory {
    /// The reserved registry row the advisory renders as.
    #[must_use]
    pub fn code(&self) -> CodeInfo {
        match self {
            Self::Stale(_) => DARTQ_PACKAGE_CONFIG_OUT_OF_DATE,
            Self::MissingConfig(_) => DARTQ_PACKAGE_CONFIG_MISSING,
            Self::InvalidConfig(_) => DARTQ_PACKAGE_CONFIG_INVALID,
        }
    }

    /// The row's file anchor: the config the advisory names — existing for
    /// the stale and invalid variants, prospective for the missing one.
    #[must_use]
    pub fn config_path(&self) -> &Path {
        match self {
            Self::Stale(advisory) => &advisory.config_path,
            Self::MissingConfig(config_path) => config_path,
            Self::InvalidConfig(advisory) => &advisory.config_path,
        }
    }

    /// One-line message for the output faces.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Stale(advisory) => advisory.message(),
            Self::InvalidConfig(advisory) => advisory.message(),
            // The row's FILE field already carries the path; the message
            // stays position-free.
            Self::MissingConfig(_) => "package_config.json not found; run `dart pub get` \
                 (analyzing with `package:` imports unresolved)"
                .to_string(),
        }
    }
}

/// The usage-tier workspace assembly shared by the engine-backed faces that
/// print no diagnostics (the query family and `impact`): SDK root
/// resolution, canonicalized package directory (printed paths are
/// symlink-resolved, the oracle convention), the three pub faces, package
/// tables, SDK snapshot, and the engine context. The semantic diagnostic
/// families are disarmed here (`semdiag: None`, the engine's
/// under-reporting posture): these faces answer from the index rows,
/// graphs, and facts the pipeline builds regardless, so the per-library
/// semantic fold would be dead work on every analyzed file.
///
/// # Errors
///
/// [`WorkspaceError`] when the SDK, the package directory, or the pub
/// faces are unusable.
pub fn open_engine(dir: &Path, sdk: Option<&Path>) -> Result<(Engine, PathBuf), WorkspaceError> {
    let (mut config, dir, _) = engine_config_with_advisory(dir, sdk)?;
    config.semdiag = None;
    let engine = Engine::new(config)
        .map_err(|error| WorkspaceError(format!("engine context unusable: {error}")))?;
    Ok((engine, dir))
}

/// [`open_engine`] plus the assembly's one workspace advisory (the #732
/// stale-config row, or its #759 missing-config sibling) — the shape the
/// diagnostics faces (`dartq analyze`) render before their output; the
/// faces that keep the bare [`open_engine`] stay advisory-free.
///
/// # Errors
///
/// [`WorkspaceError`] when the SDK, the package directory, or the pub
/// faces are unusable.
pub fn open_engine_with_advisory(
    dir: &Path,
    sdk: Option<&Path>,
) -> Result<(Engine, PathBuf, Option<WorkspaceAdvisory>), WorkspaceError> {
    open_engine_with_overrides(dir, sdk, &AnalyzeOverrides::default())
}

/// The analyze face's CLI overrides (#938): `--packages` and
/// `--enable-experiment`. Every field defaults to the undiscoverable-flag
/// posture — the plain assembly.
#[derive(Debug, Default, Clone)]
pub struct AnalyzeOverrides {
    /// The package resolution configuration file (`--packages PATH`): the
    /// resolution input in place of the workspace's
    /// `.dart_tool/package_config.json` (the official `packagesFile`), its
    /// directory the `rootUri` resolution base. A missing file is the
    /// usage-tier refusal the oracle ships (`The file doesn't exist: {path}`).
    pub packages: Option<PathBuf>,
    /// The CLI experiment flags (`--enable-experiment`, repeatable,
    /// comma-split): the context-level feature base every package file's
    /// feature set starts from; an options chain declaring its own
    /// `enable-experiment` section replaces it for that subtree (the
    /// official `_applyEnableExperiments` assignment). Unknown names are the
    /// usage error the dartdev CLI ships (`Unknown experiment(s): 'x'`,
    /// `no-` stripped before the known-feature test).
    pub experiments: Vec<String>,
}

/// [`open_engine_with_advisory`] with the analyze face's CLI overrides
/// (#938).
///
/// # Errors
///
/// [`WorkspaceError`] when the SDK, the package directory, the pub faces,
/// or an override value is unusable.
pub fn open_engine_with_overrides(
    dir: &Path,
    sdk: Option<&Path>,
    overrides: &AnalyzeOverrides,
) -> Result<(Engine, PathBuf, Option<WorkspaceAdvisory>), WorkspaceError> {
    let (config, dir, stale_advisory) = engine_config_with_overrides(dir, sdk, overrides)?;
    let engine = Engine::new(config)
        .map_err(|error| WorkspaceError(format!("engine context unusable: {error}")))?;
    Ok((engine, dir, stale_advisory))
}

/// The `no-`-stripped known-feature check the dartdev CLI runs before any
/// analysis (`Unknown experiment(s): 'a', 'b'`).
fn validate_experiments(experiments: &[String]) -> Result<(), WorkspaceError> {
    let unknown: Vec<String> = experiments
        .iter()
        .filter_map(|flag| {
            let name = flag.strip_prefix("no-").unwrap_or(flag);
            dartq_language::Feature::try_from(name)
                .is_err()
                .then(|| name.to_owned())
        })
        .collect();
    if unknown.is_empty() {
        return Ok(());
    }
    let listed = unknown
        .iter()
        .map(|name| format!("'{name}'"))
        .collect::<Vec<_>>()
        .join(", ");
    Err(WorkspaceError(format!("Unknown experiment(s): {listed}")))
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
    engine_config_with_advisory(dir, sdk).map(|(config, dir, _)| (config, dir))
}

/// [`engine_config`] plus the assembly's one workspace advisory (see
/// [`open_engine_with_advisory`]) — the daemon shell's assembly shape: the
/// advisory travels into the daemon session as the session diagnostic
/// channel's one startup push.
///
/// # Errors
///
/// [`WorkspaceError`] when the SDK, the package directory, or the pub
/// faces are unusable.
pub fn engine_config_with_advisory(
    dir: &Path,
    sdk: Option<&Path>,
) -> Result<(EngineConfig, PathBuf, Option<WorkspaceAdvisory>), WorkspaceError> {
    engine_config_with_overrides(dir, sdk, &AnalyzeOverrides::default())
}

/// [`engine_config_with_advisory`] with the analyze face's CLI overrides
/// (#938).
///
/// # Errors
///
/// [`WorkspaceError`] when the SDK, the package directory, the pub faces,
/// or an override value is unusable.
pub fn engine_config_with_overrides(
    dir: &Path,
    sdk: Option<&Path>,
    overrides: &AnalyzeOverrides,
) -> Result<(EngineConfig, PathBuf, Option<WorkspaceAdvisory>), WorkspaceError> {
    validate_experiments(&overrides.experiments)?;
    let sdk_root = sdk
        .map(Path::to_path_buf)
        .or_else(|| env::var_os("DART_SDK").map(PathBuf::from));
    let Some(sdk_root) = sdk_root else {
        return Err(WorkspaceError(
            "no Dart SDK root (pass --sdk PATH or set DART_SDK)".into(),
        ));
    };
    // The SDK loads before the assembly: the freshness face observes the
    // snapshot's Dart version against the config's `generatorVersion`. The
    // message is the oracle's usage text for the unusable-SDK tier,
    // verbatim (upstream Dart SDK source, pkg/dartdev — analyze.dart:208).
    let sdk_snapshot = SdkSnapshot::load(&sdk_root, None)
        .map_err(|_| WorkspaceError(format!("Invalid Dart SDK path: {}", sdk_root.display())))?;
    // The stale-config advisory rides `package_assembly`; the engine-backed
    // faces surface it through their output plumbing.
    let (packages, config, config_dir, dir, stale_advisory) = package_assembly_with(
        dir,
        overrides.packages.as_deref(),
        Some(&sdk_snapshot.dart.version),
    )?;
    Ok((
        EngineConfig {
            // The file-backed CAS store: workspace loads ride read-only
            // memory mappings (zero-copy text, content-deduplicated), the
            // engine's cold-load default.
            store: dartq_engine::mmap_store(),
            sdk: sdk_snapshot,
            packages,
            pub_config: config,
            config_dir,
            // The semantic families are part of the analyze contract (#236):
            // `undefined_*` / `wrong_number_of_*` ride the default diagnoser.
            semdiag: Some(semdiag::diagnoser()),
            target: dartq_engine::DEFAULT_TARGET.to_owned(),
            declared_variables: rustc_hash::FxHashMap::default(),
            experiments: overrides.experiments.clone(),
        },
        dir,
        stale_advisory,
    ))
}

/// The SDK-free pub assembly behind every package-attributed face: the
/// canonicalized package directory, the three pub faces, and the package
/// table. [`engine_config`] adds the SDK snapshot; the fix face consumes
/// this alone for the per-file language versions ([`crate::lints`]).
///
/// The fifth element is the one [`WorkspaceAdvisory`] for the resolution —
/// [`WorkspaceAdvisory::Stale`] when a config exists and
/// [`Freshness::check`] reports staleness or the graph recorded membership
/// findings; [`WorkspaceAdvisory::MissingConfig`] when there is no config
/// at all and the assembly degrades to the oracle's `BasicWorkspace` floor.
/// Assembly proceeds either way; the advisory is output, not refusal.
///
/// The degraded floor (#759, the official locator's posture): the root
/// walk finds the pubspec (pub's root discovery, the pubspec half of
/// `context_locator.dart`'s `_contextRootLocation` walk), and when the
/// root's `.dart_tool/package_config.json` is absent the context opens as
/// a single package over an empty `package:` map
/// (`context_locator.dart:517-522` — `BasicWorkspace.find` with
/// `Packages.empty`; :266-273 retains the path-local root). A missing
/// pubspec entirely is the same floor, bare. `pubspec.lock` participates
/// nowhere in the floor's assembly.
///
/// # Errors
///
/// [`WorkspaceError`] when the package directory or the pub faces are
/// unusable.
/// One assembled workspace face: the package table, the resolution config,
/// the workspace root, the resolved pubspec path, and any non-fatal pub
/// advisory.
pub(crate) type WorkspaceAssembly = (
    PackageTable,
    Arc<PackageConfig>,
    PathBuf,
    PathBuf,
    Option<WorkspaceAdvisory>,
);

pub(crate) fn package_assembly(dir: &Path) -> Result<WorkspaceAssembly, WorkspaceError> {
    // No SDK snapshot on this path: the generatorVersion comparison stays
    // skipped (an unobservable face never stales).
    package_assembly_with(dir, None, None)
}

/// [`package_assembly`] with the analyze face's `--packages` override
/// (#938): `packages_override` names the resolution config in place of
/// `workspace.root/.dart_tool/package_config.json` (the official
/// `packagesFile`); its directory is the `rootUri` resolution base. A
/// missing file is the usage-tier refusal the dartdev CLI ships
/// (`The file doesn't exist: {path}`); an override without any pubspec on
/// the walk is refused (dartq's package table is workspace-assembled).
/// `dart_version` is the SDK snapshot's observed version, when the caller
/// has one — the freshness face compares it against the config's
/// `generatorVersion` (upstream pub source, entrypoint.dart:1231-1233
/// `isPackageConfigGeneratedBySameDartSdk`); `None` (the SDK-free faces)
/// skips that comparison, an unobservable face never stales.
pub(crate) fn package_assembly_with(
    dir: &Path,
    packages_override: Option<&Path>,
    dart_version: Option<&str>,
) -> Result<WorkspaceAssembly, WorkspaceError> {
    match assemble(dir, packages_override, dart_version)? {
        AssemblyBody::Floor(assembly) => Ok(assembly),
        AssemblyBody::Resolved {
            packages,
            config,
            config_dir,
            dir,
            advisory,
            ..
        } => Ok((packages, config, config_dir, dir, advisory)),
    }
}

/// The strict assembly's payload: the resolution config, the resolved graph
/// (the compat face rebuilds its per-axis identity tables from it — the
/// table is engine-owned, not `Clone`), the config directory, and the one
/// non-gating advisory (the [`WorkspaceAdvisory::Stale`] variant; the
/// Missing and Invalid variants are the strict refusals here).
pub(crate) type StrictAssembly = (
    Arc<PackageConfig>,
    ResolvedPackageGraph,
    PathBuf,
    Option<WorkspaceAdvisory>,
);

/// The compat face's assembly: the same machinery as
/// [`package_assembly_with`] (root resolution, member/non-member pubspec
/// seeding, nested-root extension, freshness baseline), with the three
/// degrade floors turned into the config-required refusals the face's
/// strictness contract pins — a package with no resolvable config anywhere
/// up the root chain (or a config unusable at all) is the usage tier, where
/// the analyze path degrades and analyzes. `dart_version` is `None` on this
/// path by shape: the compat face assembles before any axis SDK is loaded
/// and the axes may disagree, so no `generatorVersion` comparison runs.
///
/// # Errors
///
/// [`WorkspaceError`] when the package directory or the pub faces are
/// unusable (the floors, verbatim messages below), with the missing-config
/// message byte-identical to [`load_workspace_faces`]'s.
pub(crate) fn package_assembly_strict(dir: &Path) -> Result<StrictAssembly, WorkspaceError> {
    match assemble(dir, None, None)? {
        AssemblyBody::Floor((_, _, _, _, advisory)) => Err(WorkspaceError(match advisory {
            Some(WorkspaceAdvisory::MissingConfig(config_path)) => format!(
                "no package config at {} (run `dart pub get`)",
                config_path.display()
            ),
            Some(WorkspaceAdvisory::InvalidConfig(invalid)) => format!(
                "package_config.json at {} is unusable: {}",
                invalid.config_path.display(),
                invalid.reason
            ),
            // The floors carry Missing or Invalid by construction; Stale
            // and None are the success path's shapes.
            Some(WorkspaceAdvisory::Stale(_)) | None => {
                unreachable!("a degrade floor's advisory is Missing or Invalid")
            }
        })),
        AssemblyBody::Resolved {
            config,
            graph,
            config_dir,
            advisory,
            ..
        } => Ok((config, graph, config_dir, advisory)),
    }
}

/// The shared assembly body's outcome: one of the three degrade floors —
/// the analyze path assembles over it, the strict face refuses on it — or
/// the full resolution, which also carries the graph (the table-backed
/// faces don't consume it; the compat face's per-axis identity-table
/// rebuilds do).
#[derive(Debug)]
enum AssemblyBody {
    Floor(Assembly),
    Resolved {
        packages: PackageTable,
        config: Arc<PackageConfig>,
        graph: ResolvedPackageGraph,
        config_dir: PathBuf,
        dir: PathBuf,
        advisory: Option<WorkspaceAdvisory>,
    },
}

/// The machinery [`package_assembly_with`] and [`package_assembly_strict`]
/// share; the callers diverge exactly at the [`AssemblyBody::Floor`] sites.
fn assemble(
    dir: &Path,
    packages_override: Option<&Path>,
    dart_version: Option<&str>,
) -> Result<AssemblyBody, WorkspaceError> {
    let override_config = packages_override
        .map(|given| {
            let path = std::path::absolute(given)
                .map_err(|error| WorkspaceError(format!("--packages: {error}")))?;
            if !path.is_file() {
                return Err(WorkspaceError(format!(
                    "The file doesn't exist: {}",
                    given.display()
                )));
            }
            let parent = override_parent(&path)?;
            Ok((path, parent))
        })
        .transpose()?;
    // Canonicalize up front so the printed paths are symlink-resolved (the
    // oracle convention) and package roots derived from `config_dir` inherit
    // the canonical form.
    let dir = dir.canonicalize().map_err(|_| {
        WorkspaceError(format!(
            "Directory or file doesn't exist: {}",
            dir.display()
        ))
    })?;
    // pub's root walk. No pubspec anywhere is the BasicWorkspace floor, not
    // a refusal; every other discovery finding (a workspace-resolution
    // pubspec with no root above it) stays a usage error. The discriminator
    // is the walk's typed no-root variant (`PubError::NoRoot`), the one
    // error that means "no pubspec on the chain". An unparseable pubspec on
    // the chain is the oracle's empty-model floor, not a refusal: the
    // analyzer reads every pubspec through the catching `Pubspec.parse`
    // (upstream Dart SDK source, `pkg/analyzer/lib/src/lint/pub.dart`;
    // the workspace face `pkg/analyzer/lib/src/workspace/pub.dart:487-494`
    // builds through the same constructor), so the run roots at the
    // nearest pubspec with empty facts where pub's validation walk refuses
    // (entrypoint.dart).
    let workspace = match find_workspace_root(&dir) {
        Ok(workspace) => workspace,
        Err(PubError::NoRoot(_)) => return no_root_posture(&dir, override_config.is_some()),
        Err(PubError::Pubspec(_)) => match nearest_pubspec_dir(&dir) {
            // A parse failure implies a pubspec on the chain at discovery
            // time; a file deleted between the two walks (TOCTOU) is the
            // same no-pubspec chain, the posture below — not a panic.
            Some(root) => WorkspaceLayout {
                root,
                members: Vec::new(),
                member_pubspecs: Vec::new(),
            },
            None => return no_root_posture(&dir, override_config.is_some()),
        },
        Err(error) => return Err(WorkspaceError(error.to_string())),
    };
    let packages_overridden = override_config.is_some();
    let (config_path, config_dir) = override_config.unwrap_or_else(|| {
        let config_dir = workspace.root.join(".dart_tool");
        let config_path = config_dir.join("package_config.json");
        (config_path, config_dir)
    });
    // Known divergence: the oracle's PackageConfigWorkspaceFindSession walks
    // ancestors, so a config ABOVE the pubspec root wins there; dartq checks
    // only `workspace.root/.dart_tool` and degrades.
    if !config_path.is_file() {
        // Known divergence (degraded mode only): the oracle's BasicWorkspace
        // roots the self package at the CLOSEST pubspec (the context_locator
        // `_contextRootLocation` walk = the member dir), so for a member with
        // no config anywhere dartq attributes files outside the member to the
        // self package where the oracle attributes nothing — attribution-only.
        return degraded_assembly(&workspace.root, &config_path, &dir).map(AssemblyBody::Floor);
    }
    // A config face failure degrades (the oracle's `Packages.empty`
    // catch-all, packages.dart `parsePackageConfigJsonFile`); a malformed
    // lock degrades to the lock-absent posture inside `load_workspace_faces`
    // (the analyzer never reads `pubspec.lock`), and a malformed pubspec
    // degrades to the empty model (the oracle's `Pubspec.parse` catch).
    let (config, lock, mut pubspec, malformed_lock) =
        match load_workspace_faces(&workspace.root, &config_path) {
            Ok(faces) => faces,
            Err(WorkspaceFacesError::PackageConfig(reason)) => {
                return corrupt_config_assembly(&workspace.root, &config_path, &dir, &reason)
                    .map(AssemblyBody::Floor);
            }
            Err(error) => return Err(WorkspaceError(error.to_string())),
        };
    // The empty-model floor's root identity borrows the config's self-entry
    // name (`config_self_name`): the table's name grammar has no null, and
    // the attribution is root-based either way.
    if pubspec.name.is_empty()
        && let Some(name) = config_self_name(&config, &config_dir, &workspace.root)
    {
        pubspec.name = name;
    }
    // Nested self-resolved packages: a directory under the analyzed tree
    // carrying its own `pubspec.yaml` AND its own
    // `.dart_tool/package_config.json` (a Flutter plugin repo's `example/`
    // after `pub get`) is a separate context root for the oracle
    // (context_locator.dart `_createContextRoots`: a locally different
    // packages file splits the root). dartq's single context stands in by
    // giving the nested package a config identity, so its files attribute
    // to it at the nested pubspec's sdk lower bound instead of the
    // enclosing package's (or the default). With `--packages` the locator's
    // `_defaultPackageConfigFile` short-circuit keeps one root everywhere,
    // so discovery is skipped exactly there. The freshness face re-reads
    // the on-disk config, so the in-memory extension never moves the
    // advisory.
    let config = if packages_overridden {
        config
    } else {
        extend_config_with_nested_roots(config, &config_dir, &dir)
    };
    let (extra_pubspecs, member_names) = assemble_extra_pubspecs(&workspace, &pubspec);
    // `workspace_members: None` outside a pub workspace — the standalone
    // package table must stay byte-identical to the pre-workspace shape.
    let workspace_members = (!pubspec.workspace.is_empty()).then_some(member_names.as_slice());
    // A graph or table failure is a config-content defect (duplicate
    // entries, a rejected `rootUri`/name/`packageUri`) — the same corrupt
    // floor as a parse failure, the reason naming the defect.
    let graph = match build_graph(
        &config,
        &lock,
        &pubspec,
        &extra_pubspecs,
        workspace_members,
        &config_dir,
    ) {
        Ok(graph) => graph,
        Err(error) => {
            return corrupt_config_assembly(
                &workspace.root,
                &config_path,
                &dir,
                &error.to_string(),
            )
            .map(AssemblyBody::Floor);
        }
    };
    let packages = match PackageTable::build(&config, &graph, &config_dir) {
        Ok(packages) => packages,
        Err(error) => {
            return corrupt_config_assembly(
                &workspace.root,
                &config_path,
                &dir,
                &error.to_string(),
            )
            .map(AssemblyBody::Floor);
        }
    };
    // The one stale-config advisory: freshness reasons plus the graph's
    // membership findings, never a gate. The environment faces other than
    // the SDK version stay skipped (dartq observes no Flutter/pub-cache
    // environment here). The content baseline is captured once per accepted
    // resolution, right here — the config and per-entry pubspec bytes the
    // graph was just built from — so an mtime-chain failure (a VCS checkout
    // or `touch`) is excused when the content is unchanged,
    // pub's own content-check posture; a capture failure (an entry pubspec
    // vanished mid-run) keeps the conservative mtime verdict. The check
    // stats a fixed file set (config, lock, per-entry pubspecs); it adds no
    // directory scan. The lock's absence skips the freshness face entirely
    // (#759): no lock means no freshness comparison, not an advisory. A
    // malformed lock degrades to the same no-lock posture — the mtime chain
    // is meaningless against a lock whose content cannot be trusted — and
    // the parse failure itself is the staleness signal (pub would
    // re-resolve): it surfaces as the `MalformedLockFile` reason.
    // Cross-run seam: the baseline reflects the bytes as analyzed, so a
    // pubspec edited after the real `pub get` but before this run is not
    // caught; catching it needs a baseline persisted at acceptance time.
    let mut reasons = if let Some(error) = malformed_lock {
        vec![StaleReason::MalformedLockFile(error)]
    } else if workspace.root.join("pubspec.lock").is_file() {
        let baseline = ResolutionBaseline::capture(&workspace.root).ok();
        let env = EnvSnapshot {
            dart_version: dart_version.map(str::to_owned),
            ..EnvSnapshot::default()
        };
        Freshness::check(&workspace.root, &env, baseline.as_ref()).stale_reasons
    } else {
        Vec::new()
    };
    reasons.extend(graph.membership_findings.iter().cloned());
    let stale_advisory = (!reasons.is_empty()).then(|| {
        WorkspaceAdvisory::Stale(StaleConfigAdvisory {
            reasons,
            config_path: config_path.clone(),
        })
    });
    Ok(AssemblyBody::Resolved {
        packages,
        config,
        graph,
        config_dir,
        dir,
        advisory: stale_advisory,
    })
}

/// The `--packages` override's `rootUri` resolution base: the config
/// file's parent directory. Fallible for the no-parent spellings (`/`,
/// the empty path) — the usage-tier error, not a panic; the `is_file`
/// guard refuses those first through the assembly, so the arm is
/// hardening only.
fn override_parent(path: &Path) -> Result<PathBuf, WorkspaceError> {
    let parent = path.parent().ok_or_else(|| {
        WorkspaceError(format!(
            "--packages: path has no parent: {}",
            path.display()
        ))
    })?;
    Ok(parent.to_path_buf())
}

/// The no-pubspec-chain posture, shared by the root walk's typed no-root
/// finding and the unparseable-pubspec arm whose nearest-pubspec walk
/// comes up empty (the pubspec vanished mid-run): the bare floor without
/// `--packages`, the usage refusal with it.
fn no_root_posture(dir: &Path, packages_overridden: bool) -> Result<AssemblyBody, WorkspaceError> {
    if packages_overridden {
        Err(WorkspaceError(format!(
            "--packages requires a package directory (no pubspec above {})",
            dir.display()
        )))
    } else {
        bare_assembly(dir).map(AssemblyBody::Floor)
    }
}

/// The degraded floor's assembly: a single-package context rooted at the
/// pubspec directory, an empty `package:` map, and the missing-config
/// advisory. The self package's language version is its pubspec's sdk
/// lower bound — what pub would materialize into the config's
/// `languageVersion`. `dir` (the canonicalized target) returns unchanged:
/// the context root is an assembly fact, the analyzed directory is the
/// caller's.
fn degraded_assembly(
    root: &Path,
    config_path: &Path,
    dir: &Path,
) -> Result<Assembly, WorkspaceError> {
    let pubspec = load_pubspec_or_empty(root).map_err(WorkspaceError)?;
    let language_version = LanguageVersion::from_sdk_constraint(pubspec.environment.sdk.as_ref());
    let packages = PackageTable::basic(root, self_package_facts(&pubspec, language_version))
        .map_err(|error| WorkspaceError(error.to_string()))?;
    Ok((
        packages,
        empty_package_config(),
        root.join(".dart_tool"),
        dir.to_path_buf(),
        Some(WorkspaceAdvisory::MissingConfig(config_path.to_path_buf())),
    ))
}

/// The corrupt-config floor: the oracle's `Packages.empty` catch-all — any
/// read/parse/resolve failure of an existing `package_config.json` becomes
/// an empty configuration and the run keeps analyzing with every `package:`
/// import unresolved (upstream Dart SDK source,
/// pkg/analyzer/lib/src/context/packages.dart `parsePackageConfigJsonFile`
/// catch; duplicate names reach it because package:package_config's default
/// `onError` throws). Same single-package `BasicWorkspace` shape as
/// [`degraded_assembly`] (the pubspec root is retained), with the
/// invalid-config advisory carrying the failure.
fn corrupt_config_assembly(
    root: &Path,
    config_path: &Path,
    dir: &Path,
    reason: &str,
) -> Result<Assembly, WorkspaceError> {
    let pubspec = load_pubspec_or_empty(root).map_err(WorkspaceError)?;
    let language_version = LanguageVersion::from_sdk_constraint(pubspec.environment.sdk.as_ref());
    let packages = PackageTable::basic(root, self_package_facts(&pubspec, language_version))
        .map_err(|error| WorkspaceError(error.to_string()))?;
    Ok((
        packages,
        empty_package_config(),
        root.join(".dart_tool"),
        dir.to_path_buf(),
        Some(WorkspaceAdvisory::InvalidConfig(InvalidConfigAdvisory {
            config_path: config_path.to_path_buf(),
            reason: reason.to_owned(),
        })),
    ))
}

/// The bare floor: no pubspec anywhere on the walk — the oracle's
/// path-local `BasicWorkspace` with `Packages.empty`
/// (`context_locator.dart:517-522`). No self package (the workspace-default
/// language version applies per file), every `package:` import unresolved.
fn bare_assembly(dir: &Path) -> Result<Assembly, WorkspaceError> {
    let packages =
        PackageTable::basic(dir, None).map_err(|error| WorkspaceError(error.to_string()))?;
    Ok((
        packages,
        empty_package_config(),
        dir.join(".dart_tool"),
        dir.to_path_buf(),
        Some(WorkspaceAdvisory::MissingConfig(
            dir.join(".dart_tool/package_config.json"),
        )),
    ))
}

/// The empty package resolution behind both degraded floors: every
/// `package:` URI is an unknown package, the oracle's `Packages.empty`.
fn empty_package_config() -> Arc<PackageConfig> {
    Arc::new(PackageConfig {
        packages: Vec::new(),
        generator: None,
        generator_version: None,
        additional_properties: BTreeMap::new(),
    })
}

/// The nearest directory at or above `from` carrying a `pubspec.yaml` —
/// the root of the unparseable-pubspec floor (the oracle's `BasicWorkspace`
/// roots at the closest pubspec, `context_locator.dart`
/// `_contextRootLocation`).
fn nearest_pubspec_dir(from: &Path) -> Option<PathBuf> {
    let mut dir = from.to_path_buf();
    loop {
        if dir.join("pubspec.yaml").is_file() {
            return Some(dir);
        }
        dir = dir.parent()?.to_path_buf();
    }
}

/// The [`package_assembly`] tuple, named for the two floor helpers.
type Assembly = (
    PackageTable,
    Arc<PackageConfig>,
    PathBuf,
    PathBuf,
    Option<WorkspaceAdvisory>,
);

/// Why [`load_workspace_faces`] failed, split on the face the two callers
/// treat differently: the config face's failure is the analyze path's
/// degrade trigger (the oracle's `Packages.empty` catch-all), while the
/// compat face requires the config and the lock/pubspec faces always
/// refuse. The message text is the printed failure, byte-identical to the
/// pre-typing strings.
#[derive(Debug)]
pub enum WorkspaceFacesError {
    /// `package_config.json` is unreadable or unparseable.
    PackageConfig(String),
    /// `pubspec.yaml` is missing (an unparseable pubspec degrades —
    /// [`load_pubspec_or_empty`]; a malformed `pubspec.lock` degrades to
    /// the lock-absent posture, never an error here).
    PubFaces(String),
}

impl std::fmt::Display for WorkspaceFacesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PackageConfig(message) | Self::PubFaces(message) => f.write_str(message),
        }
    }
}

/// The three pub faces the engine's context derives from (the same assembly
/// the engine integration fixture pins): `package_config.json`,
/// `pubspec.lock`, `pubspec.yaml` — `dir` already canonicalized and, on the
/// engine path, the resolved workspace root.
///
/// The lock is optional (#759): its absence yields an empty
/// [`LockFile`] — the graph resolves every config entry as a local path
/// source and the freshness face stays silent. A present but malformed
/// lock degrades the same way — the analyzer never reads `pubspec.lock`,
/// so an unparseable (or unreadable) lock behaves exactly like a missing
/// one: the assembly continues on the empty lock and the fourth tuple
/// element carries the failure text for the caller's advisory. A missing
/// or malformed config still fails the assembly: the caller
/// (`dartq compat`) owns the config-required posture; the analyze path
/// matches the [`WorkspaceFacesError::PackageConfig`] variant into its
/// degrade floor. A malformed `pubspec.yaml` degrades to the empty model
/// ([`load_pubspec_or_empty`], the oracle's `Pubspec.parse` catch) instead
/// of refusing.
///
/// # Errors
///
/// A [`WorkspaceFacesError`] naming the missing or malformed face.
pub fn load_workspace_faces(
    dir: &Path,
    config_path: &Path,
) -> Result<(Arc<PackageConfig>, LockFile, Pubspec, Option<String>), WorkspaceFacesError> {
    let config_text = fs::read_to_string(config_path).map_err(|_| {
        WorkspaceFacesError::PackageConfig(format!(
            "no package config at {} (run `dart pub get`)",
            config_path.display()
        ))
    })?;
    let config = PackageConfig::parse(&config_text)
        .map_err(|error| WorkspaceFacesError::PackageConfig(error.to_string()))?;
    let lock_path = dir.join("pubspec.lock");
    let mut malformed_lock = None;
    let lock = if lock_path.is_file() {
        match fs::read_to_string(&lock_path)
            .map_err(|error| error.to_string())
            .and_then(|text| LockFile::parse(&text).map_err(|error| error.to_string()))
        {
            Ok(lock) => lock,
            Err(error) => {
                malformed_lock = Some(error);
                LockFile::empty()
            }
        }
    } else {
        LockFile::empty()
    };
    let pubspec = load_pubspec_or_empty(dir).map_err(WorkspaceFacesError::PubFaces)?;
    Ok((Arc::new(config), lock, pubspec, malformed_lock))
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
    let (text, overrides) = read_pubspec_sources(dir)?;
    Pubspec::parse_with_overrides(&text, overrides.as_deref()).map_err(|error| error.to_string())
}

/// The assembly faces' pubspec read: a present but unparseable
/// `pubspec.yaml` is the oracle's empty model, never a refusal — the
/// analyzer builds every pubspec through a constructor that catches the
/// YAML failure and yields an empty document (upstream Dart SDK source,
/// `pkg/analyzer/lib/src/lint/pub.dart`, `Pubspec.parse`'s catch; the
/// workspace face `pkg/analyzer/lib/src/workspace/pub.dart` reads through
/// the same constructor, so the run assembles with name/workspace/override
/// facts at their empty floors instead of refusing). A missing pubspec
/// stays an error.
///
/// # Errors
///
/// A human-readable message naming the missing file or the overrides read.
fn load_pubspec_or_empty(dir: &Path) -> Result<Pubspec, String> {
    let (text, overrides) = read_pubspec_sources(dir)?;
    Ok(
        Pubspec::parse_with_overrides(&text, overrides.as_deref()).unwrap_or_else(|_| Pubspec {
            name: String::new(),
            version: None,
            environment: Environment::default(),
            workspace: Vec::new(),
            resolution: Resolution::Unspecified,
            dependencies: BTreeMap::new(),
            dev_dependencies: BTreeMap::new(),
            dependency_overrides: BTreeMap::new(),
        }),
    )
}

/// The empty-model floor's self-package facts for [`PackageTable::basic`]:
/// a nameless pubspec (the unparseable floor) carries no self package —
/// the name grammar has no null, and the bare floor is the closest
/// representable posture.
fn self_package_facts(
    pubspec: &Pubspec,
    language_version: LanguageVersion,
) -> Option<(&str, Option<LanguageVersion>)> {
    if pubspec.name.is_empty() {
        None
    } else {
        Some((&pubspec.name, Some(language_version)))
    }
}

/// The config entry whose root is `root` — the resolution artifacts' name
/// for the self package (the config is the fresher face of the package
/// identity). The empty-model floor's root identity borrows it: the table's
/// name grammar has no null, and attribution is root-based either way.
fn config_self_name(config: &PackageConfig, config_dir: &Path, root: &Path) -> Option<String> {
    let target = normalize(root);
    config
        .packages
        .iter()
        .find(|entry| {
            entry
                .resolve_root(config_dir)
                .map(|resolved| normalize(&resolved) == target)
                .unwrap_or(false)
        })
        .map(|entry| entry.name.to_string())
}

/// Reads `<dir>/pubspec.yaml` plus its optional `pubspec_overrides.yaml`
/// (`allowOverridesFile: true`, entrypoint.dart:95-101).
///
/// # Errors
///
/// A human-readable message naming the missing file or the overrides read.
fn read_pubspec_sources(dir: &Path) -> Result<(String, Option<String>), String> {
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
    Ok((text, overrides))
}

/// The graph's `extra_pubspecs` and the workspace member names: every member
/// pubspec (member versions and dependency edges; the names are the
/// workspace member set) plus the pubspecs of non-member path dependencies
/// declared by the root or a member, so their own dependency edges reach
/// the graph. Member pubspecs ride the validated [`WorkspaceLayout`] —
/// discovery parsed them already, so no face re-reads the files
/// (audit #708 P3-3).
fn assemble_extra_pubspecs(
    workspace: &WorkspaceLayout,
    root_pubspec: &Pubspec,
) -> (BTreeMap<String, Pubspec>, Vec<String>) {
    let mut extra_pubspecs: BTreeMap<String, Pubspec> = BTreeMap::new();
    let mut member_names: Vec<String> = Vec::with_capacity(workspace.members.len());
    for spec in &workspace.member_pubspecs {
        member_names.push(spec.name.clone());
        extra_pubspecs.insert(spec.name.clone(), spec.clone());
    }
    seed_non_member_path_deps(
        &workspace.root,
        root_pubspec,
        &workspace.members,
        &member_names,
        &mut extra_pubspecs,
    );
    (extra_pubspecs, member_names)
}

/// Loads the pubspecs of non-member path dependencies declared by the root
/// or a member into `extra_pubspecs`. Seeded with members alone, a plain
/// path dep contributed no `graph.edges` — `collect_edges` walks a spec's
/// dependencies only when its pubspec is provided, so every edge below a
/// non-member path dep was dropped (audit #708 P3-4).
///
/// Discovery is path-resolved, not name-only: each declared `path:` is
/// resolved against the declaring package's directory and canonicalized;
/// targets resolving to the root or a member are skipped (the validated
/// workspace load already seeded those). A target whose pubspec cannot be
/// loaded is skipped too — stale artifacts keep the thinner-graph behavior
/// instead of failing the assembly, the same tolerance `collect_edges`
/// applies to pubspecs it was not given.
fn seed_non_member_path_deps(
    root: &Path,
    root_pubspec: &Pubspec,
    members: &[PathBuf],
    member_names: &[String],
    extra_pubspecs: &mut BTreeMap<String, Pubspec>,
) {
    // Directories already covered by the validated workspace load; targets
    // resolving into one are that package, keyed by name in `extra_pubspecs`
    // already.
    let seeded: BTreeSet<PathBuf> = std::iter::once(root)
        .chain(members.iter().map(PathBuf::as_path))
        .map(|dir| dir.canonicalize().unwrap_or_else(|_| normalize(dir)))
        .collect();
    let sources = std::iter::once((root, root_pubspec)).chain(
        members
            .iter()
            .zip(member_names)
            .map(|(dir, name)| (dir.as_path(), &extra_pubspecs[name])),
    );
    let mut targets: BTreeSet<PathBuf> = BTreeSet::new();
    for (dir, spec) in sources {
        let declared = spec
            .dependencies
            .values()
            .chain(spec.dev_dependencies.values())
            .chain(spec.dependency_overrides.values());
        for dep in declared {
            if let DependencySpec::Path { path } = dep {
                let resolved = normalize(&dir.join(path));
                let target = match resolved.canonicalize() {
                    Ok(canonical) => canonical,
                    Err(_) => resolved,
                };
                if !seeded.contains(&target) {
                    targets.insert(target);
                }
            }
        }
    }
    for target in targets {
        if let Ok(spec) = load_pubspec(&target) {
            // A name already present means inconsistent artifacts (two
            // packages resolving to one name); the seeded spec must win.
            let name = spec.name.clone();
            extra_pubspecs.entry(name).or_insert(spec);
        }
    }
}

/// Directories visited before [`extend_config_with_nested_roots`]'s scan
/// stops: the bounded-walk fuse. A tree beyond the budget keeps today's
/// attribution for its undiscovered nested packages instead of growing the
/// startup cost without bound.
const NESTED_ROOT_WALK_BUDGET: usize = 10_000;

/// Extends the context config with one entry per nested self-resolved
/// package: a directory under `dir` (the canonicalized analyzed tree)
/// carrying its own `pubspec.yaml` together with its own
/// `.dart_tool/package_config.json` — the oracle's separate-context-root
/// trigger (`context_locator.dart` `_createContextRoots`,
/// `packageConfigFileToUse != containingRoot.packagesFile`). The entry's
/// language version is the nested pubspec's sdk lower bound — what pub
/// materializes into a written config's `languageVersion`, the same
/// inference the degraded floor applies.
///
/// Skips: directories already covered by a config entry root (the root,
/// workspace members, path deps), names already present or added (one
/// identity per name is a table invariant), and pubspecs that fail to load
/// — a stray malformed pubspec degrades to today's attribution instead of
/// failing the assembly. Hidden directories hold no package source
/// (`.dart_tool`, `.git`) and are not descended; discovered roots are,
/// mirroring the locator's recursion; the walk stops at
/// [`NESTED_ROOT_WALK_BUDGET`] directories.
///
/// The common case — no nested roots — returns `config` unchanged.
fn extend_config_with_nested_roots(
    config: Arc<PackageConfig>,
    config_dir: &Path,
    dir: &Path,
) -> Arc<PackageConfig> {
    let taken_names: BTreeSet<&str> = config
        .packages
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    let taken_roots: BTreeSet<PathBuf> = config
        .packages
        .iter()
        .filter_map(|entry| entry.resolve_root(config_dir).ok())
        .map(|root| normalize(&root))
        .collect();

    let mut entries = Vec::new();
    let mut added_roots: BTreeSet<PathBuf> = BTreeSet::new();
    let mut added_names: BTreeSet<String> = BTreeSet::new();
    let mut visited = 0usize;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        visited += 1;
        if visited > NESTED_ROOT_WALK_BUDGET {
            break;
        }
        let mut children: Vec<PathBuf> = fs::read_dir(&current)
            .map(|listing| {
                listing
                    .filter_map(|entry| entry.ok())
                    .map(|entry| entry.path())
                    .filter(|path| path.is_dir())
                    .collect()
            })
            .unwrap_or_default();
        children.sort();
        for child in children {
            if child
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('.'))
            {
                continue;
            }
            stack.push(child);
        }
        // The nested-root trigger: own pubspec, own resolution artifacts.
        if !current.join("pubspec.yaml").is_file()
            || !current.join(".dart_tool/package_config.json").is_file()
        {
            continue;
        }
        // One spelling universe with the config's resolved roots; the walk
        // starts canonical, and a symlinked candidate resolves too.
        let Ok(target) = current.canonicalize() else {
            continue;
        };
        let target = normalize(&target);
        if taken_roots.contains(&target) || !added_roots.insert(target.clone()) {
            continue;
        }
        let Ok(spec) = load_pubspec(&target) else {
            continue;
        };
        if taken_names.contains(spec.name.as_str()) || !added_names.insert(spec.name.clone()) {
            continue;
        }
        let base = config_dir.parent().unwrap_or(config_dir);
        let Ok(relative) = target.strip_prefix(base) else {
            continue;
        };
        entries.push(PackageConfigEntry {
            name: spec.name.clone(),
            root_uri: relative_root_uri(relative),
            package_uri: Some("lib/".to_owned()),
            language_version: Some(LanguageVersion::from_sdk_constraint(
                spec.environment.sdk.as_ref(),
            )),
            additional_properties: BTreeMap::new(),
        });
    }
    if entries.is_empty() {
        return config;
    }
    let mut extended = (*config).clone();
    extended.packages.extend(entries);
    Arc::new(extended)
}

/// The `rootUri` for a nested root: a relative URI path from the config's
/// directory (`"../example"`). The raw spelling rides `resolve_root`'s
/// percent-decode, so a directory name carrying an encoded `%` would
/// mis-decode — a spelling real package directories never carry.
fn relative_root_uri(relative: &Path) -> String {
    let mut uri = String::from("../");
    for (index, component) in relative.components().enumerate() {
        if index > 0 {
            uri.push('/');
        }
        uri.push_str(&component.as_os_str().to_string_lossy());
    }
    uri
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
            // The pub-writer shape: pub always emits `packageUri: "lib/"`
            // (upstream pub source, lib/src/entrypoint.dart:511) — without
            // it the reader contract anchors the source root at the
            // package root itself (package_config_json.dart:152), and no
            // `package:` URI addresses the `lib/` members.
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "packageUri": "lib/", "languageVersion": "3.5"}], "generator": "pub", "generatorVersion": "3.13.1"}"#,
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
    use dartq_pub::DepKind;
    use filetime::{FileTime, set_file_mtime};

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

    /// Extends the member fixture with the P3-4 shape: the root depends on
    /// member `a`, which path-depends on non-member `b`, which path-depends
    /// back on the root package — the hop whose edges the member-only
    /// seeding dropped.
    fn path_dep_fixture(tag: &str) -> PathBuf {
        let root = member_fixture(tag);
        fs::write(
            root.join("pubspec.yaml"),
            "name: fixture\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - packages/a\ndependencies:\n  a:\n    path: packages/a\n",
        )
        .expect("root pubspec");
        fs::write(
            root.join("packages/a/pubspec.yaml"),
            "name: a\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\ndependencies:\n  b:\n    path: ../b\n",
        )
        .expect("member pubspec");
        fs::create_dir_all(root.join("packages/b")).expect("path dep dir");
        fs::write(
            root.join("packages/b/pubspec.yaml"),
            "name: b\nversion: 0.1.0\ndependencies:\n  fixture:\n    path: ../../\n",
        )
        .expect("path dep pubspec");
        fs::write(
            root.join(".dart_tool/package_config.json"),
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "languageVersion": "3.5"}, {"name": "a", "rootUri": "../packages/a", "languageVersion": "3.5"}, {"name": "b", "rootUri": "../packages/b", "languageVersion": "3.5"}], "generator": "pub"}"#,
        )
        .expect("config");
        root
    }

    /// Stale resolution: exactly one advisory, carrying the reason — and
    /// the assembly still succeeds (advisory, not refusal). The trigger is
    /// a config entry whose package directory has no pubspec: the only
    /// freshness faces the in-run content baseline cannot excuse
    ///, so an mtime touch no longer stales — see
    /// [`touched_resolution_stays_fresh_with_baseline`].
    #[test]
    fn stale_workspace_reports_exactly_one_advisory() {
        let root = workspace("ws_stale", &[]);
        fs::write(
            root.join(".dart_tool/package_config.json"),
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "languageVersion": "3.5"}, {"name": "ghost", "rootUri": "../ghost", "languageVersion": "3.5"}], "generator": "pub", "generatorVersion": "3.13.1"}"#,
        )
.expect("config");

        let root = root.canonicalize().expect("root");
        let (_, _, _, _, advisory) = package_assembly(&root).expect("assembly");
        let advisory = advisory.expect("stale");
        let WorkspaceAdvisory::Stale(stale) = &advisory else {
            panic!("stale advisory, got {advisory:?}")
        };
        assert_eq!(
            stale.reasons,
            vec![StaleReason::PackageDirMissing {
                package: "ghost".into(),
            }]
        );
        assert!(advisory.message().contains("run `dart pub get`"));

        fs::remove_dir_all(&root).ok();
    }

    /// Fresh chain: zero advisories — the assembly is silent.
    #[test]
    fn fresh_workspace_reports_no_advisory() {
        let root = workspace("ws_fresh_adv", &[]);
        set_file_mtime(
            root.join("pubspec.yaml"),
            FileTime::from_unix_time(1_000, 0),
        )
        .expect("set mtime");
        set_file_mtime(
            root.join("pubspec.lock"),
            FileTime::from_unix_time(2_000, 0),
        )
        .expect("set mtime");
        set_file_mtime(
            root.join(".dart_tool/package_config.json"),
            FileTime::from_unix_time(3_000, 0),
        )
        .expect("set mtime");

        let root = root.canonicalize().expect("root");
        let (_, _, _, _, advisory) = package_assembly(&root).expect("assembly");
        assert!(advisory.is_none(), "unexpected: {advisory:?}");

        fs::remove_dir_all(&root).ok();
    }

    /// A config predating the workspace (the member has no config entry):
    /// the membership finding rides the same single advisory, and the
    /// member is simply outside the analysis universe.
    #[test]
    fn membership_findings_ride_the_advisory() {
        let root = member_fixture("ws_member_finding");
        // Drop the member entry — a hand-edited config passing the mtime
        // chain is still flagged.
        fs::write(
            root.join(".dart_tool/package_config.json"),
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "languageVersion": "3.5"}], "generator": "pub"}"#,
        )
        .expect("config");
        // Deterministic fresh chain, overrides file included.
        let old = FileTime::from_unix_time(1_000, 0);
        set_file_mtime(root.join("pubspec.yaml"), old).expect("set mtime");
        set_file_mtime(root.join("pubspec_overrides.yaml"), old).expect("set mtime");
        set_file_mtime(
            root.join("pubspec.lock"),
            FileTime::from_unix_time(2_000, 0),
        )
        .expect("set mtime");
        set_file_mtime(
            root.join(".dart_tool/package_config.json"),
            FileTime::from_unix_time(3_000, 0),
        )
        .expect("set mtime");

        let root = root.canonicalize().expect("root");
        let (_, _, _, _, advisory) = package_assembly(&root).expect("assembly");
        let advisory = advisory.expect("member finding");
        let WorkspaceAdvisory::Stale(stale) = &advisory else {
            panic!("stale advisory, got {advisory:?}")
        };
        assert_eq!(
            stale.reasons,
            vec![StaleReason::MemberMissingFromConfig { name: "a".into() }]
        );

        fs::remove_dir_all(&root).ok();
    }

    /// The advisory names its config: `config_path` is the workspace's
    /// `package_config.json` (canonical) — the output faces' row anchor.
    #[test]
    fn advisory_names_the_config_path() {
        let root = workspace("ws_adv_path", &[]);
        fs::write(
            root.join(".dart_tool/package_config.json"),
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "languageVersion": "3.5"}, {"name": "ghost", "rootUri": "../ghost", "languageVersion": "3.5"}], "generator": "pub", "generatorVersion": "3.13.1"}"#,
        )
.expect("config");

        let root = root.canonicalize().expect("root");
        let (_, _, _, _, advisory) = package_assembly(&root).expect("assembly");
        assert_eq!(
            advisory.expect("stale").config_path(),
            root.join(".dart_tool/package_config.json")
        );

        fs::remove_dir_all(&root).ok();
    }

    /// The `--packages` override seam (#938): a missing file is the
    /// usage-tier refusal the dartdev CLI ships (`The file doesn't exist:
    /// {path}`, analyze.dart's packagesFile check); an override over a
    /// pubspec-less directory is refused (dartq's package table is
    /// workspace-assembled); a real override supplies the resolution input
    /// in place of `.dart_tool/package_config.json`, its directory the
    /// `rootUri` base.
    #[test]
    fn packages_override_pins_usage_and_resolution_faces() {
        let root = workspace("pkgs_override", &[]);

        // Missing file: the usage error, verbatim.
        let missing = root.join("absent.json");
        let error = package_assembly_with(&root, Some(&missing), None).expect_err("missing");
        assert!(
            error
                .to_string()
                .contains(&format!("The file doesn't exist: {}", missing.display())),
            "{error}"
        );

        // An override with no pubspec anywhere: refused, not degraded.
        let bare = std::env::temp_dir().join(format!("dq_cmd_pkgs_bare_{}", std::process::id()));
        fs::create_dir_all(&bare).expect("bare dir");
        let config = bare.join("custom.json");
        fs::write(
            &config,
            r#"{"configVersion": 2, "packages": [], "generator": "pub"}"#,
        )
        .expect("config");
        let error = package_assembly_with(&bare, Some(&config), None).expect_err("no pubspec");
        assert!(error.to_string().contains("no pubspec above"), "{error}");

        // A real override: the config comes from the given path (the
        // workspace's own `.dart_tool` config is not consulted) and the
        // entry roots resolve against the override's directory.
        let other = root.join("other");
        fs::create_dir_all(&other).expect("other dir");
        let config = other.join("custom.json");
        fs::write(
            &config,
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "languageVersion": "3.5"}], "generator": "pub"}"#,
        )
        .expect("config");
        let (packages, _, config_dir, _, advisory) =
            package_assembly_with(&root, Some(&config), None).expect("override assembly");
        assert_eq!(config_dir, other, "the override's directory");
        assert!(packages.get("fixture").is_some(), "the override's entries");
        assert!(
            advisory.is_none(),
            "no stale advisory without pubspec.lock staleness: {advisory:?}"
        );

        fs::remove_dir_all(&root).ok();
        fs::remove_dir_all(&bare).ok();
    }

    /// The `--packages` parent hardening: the no-parent spellings (`/`,
    /// the empty path) are the usage-tier error, not a panic. The
    /// assembly's `is_file` guard refuses those first, so the arm is only
    /// reachable at this seam.
    #[test]
    fn packages_override_without_parent_is_the_error_row() {
        let error = override_parent(Path::new("/")).expect_err("no parent");
        assert!(error.to_string().contains("path has no parent"), "{error}");
    }

    /// The TOCTOU degrade: an unparseable-pubspec finding whose
    /// nearest-pubspec walk comes up empty (the file vanished mid-run)
    /// takes the no-root posture — the bare floor's advisory without
    /// `--packages`, the usage refusal with it — never a panic.
    #[test]
    fn pubspec_vanished_midrun_takes_the_no_root_posture() {
        let bare = std::env::temp_dir().join(format!("dq_cmd_ws_noroot_{}", std::process::id()));
        fs::create_dir_all(&bare).expect("bare dir");

        let floor = no_root_posture(&bare, false).expect("bare floor");
        let AssemblyBody::Floor((_, _, _, _, advisory)) = floor else {
            panic!("bare floor, got a Resolved body")
        };
        assert!(
            matches!(advisory, Some(WorkspaceAdvisory::MissingConfig(_))),
            "{advisory:?}"
        );

        let Err(error) = no_root_posture(&bare, true) else {
            panic!("expected the usage refusal")
        };
        assert!(error.to_string().contains("no pubspec above"), "{error}");

        fs::remove_dir_all(&bare).ok();
    }

    /// The `--enable-experiment` known-check (#938): `no-` prefixes strip
    /// before the known-feature test, known flags pass through, and unknown
    /// names are the usage error the dartdev CLI ships
    /// (`Unknown experiment(s): 'x', 'y'`).
    #[test]
    fn unknown_experiments_are_usage_errors() {
        let root = workspace("experiments", &[]);
        let overrides = AnalyzeOverrides {
            packages: None,
            experiments: vec!["variance".to_owned(), "no-wildcard-variables".to_owned()],
        };
        // The known flags pass the check and the full config builds over
        // the fixture's synthetic SDK — including the flags in the config
        // (the CLI context base, #936's replace posture consumes them).
        let (config, _, _) =
            engine_config_with_overrides(&root, Some(&root.join("sdk")), &overrides)
                .expect("known flags");
        assert_eq!(config.experiments, overrides.experiments);

        let unknown = AnalyzeOverrides {
            packages: None,
            experiments: vec!["not-a-real-flag".to_owned()],
        };
        let Err(error) = engine_config_with_overrides(&root, None, &unknown) else {
            panic!("unknown flag");
        };
        assert_eq!(
            error.to_string(),
            "Unknown experiment(s): 'not-a-real-flag'"
        );

        let mixed = AnalyzeOverrides {
            packages: None,
            experiments: vec![
                "variance".to_owned(),
                "nope-one".to_owned(),
                "nope-two".to_owned(),
            ],
        };
        let Err(error) = engine_config_with_overrides(&root, None, &mixed) else {
            panic!("mixed flags");
        };
        assert_eq!(
            error.to_string(),
            "Unknown experiment(s): 'nope-one', 'nope-two'"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// The advisory's code is the reserved registry row, TOOL-typed — the
    /// `dartq/*` namespace sits outside the official value set, so emitting
    /// it cannot move the differential Jaccard.
    #[test]
    fn advisory_code_is_the_reserved_registry_row() {
        let info = dartq_diagnostics::CodeRegistry::builtin()
            .get(StaleConfigAdvisory::CODE)
            .expect("reserved code");
        assert_eq!(info, dartq_diagnostics::DARTQ_PACKAGE_CONFIG_OUT_OF_DATE);
        assert_eq!(info.name, "dartq/package_config_out_of_date");
        assert_eq!(info.machine, "DARTQ/PACKAGE_CONFIG_OUT_OF_DATE");
        assert_eq!(info.dtype, dartq_diagnostics::DiagnosticType::Tool);
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

        let (_, _, pubspec, _) =
            load_workspace_faces(&root, &root.join(".dart_tool/package_config.json"))
                .expect("faces");
        assert!(
            pubspec.dependency_overrides.contains_key("ext"),
            "the root pubspec is overrides-aware"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// The P3-4 pin: a non-member path dep contributes its own dependency
    /// edges once its pubspec is seeded — `b -> fixture` below member `a` —
    /// while the member-only seeding dropped everything below `b`.
    #[test]
    fn non_member_path_dep_edges_reach_the_graph() {
        let root = path_dep_fixture("ws_path_dep");
        let member = root.join("packages/a");
        let workspace = find_workspace_root(&member).expect("layout");
        let config_dir = workspace.root.join(".dart_tool");
        let (config, lock, pubspec, _) =
            load_workspace_faces(&workspace.root, &config_dir.join("package_config.json"))
                .expect("faces");
        let (extra_pubspecs, member_names) = assemble_extra_pubspecs(&workspace, &pubspec);
        let graph = build_graph(
            &config,
            &lock,
            &pubspec,
            &extra_pubspecs,
            Some(&member_names),
            &config_dir,
        )
        .expect("graph");

        let kind = |from: &str, to: &str| {
            graph
                .edges
                .iter()
                .find(|edge| edge.from.0 == from && edge.to.0 == to)
                .map(|edge| edge.kind)
        };
        assert_eq!(kind("fixture", "a"), Some(DepKind::Direct));
        assert_eq!(kind("a", "b"), Some(DepKind::Direct));
        // The new edge: b's own dependency, previously dropped.
        assert_eq!(kind("b", "fixture"), Some(DepKind::Direct));
        assert_eq!(member_names, vec!["a".to_owned()], "b is not a member");

        fs::remove_dir_all(&root).ok();
    }

    /// The degraded floor (#759, oracle parity): no `package_config.json`
    /// anywhere no longer refuses — the context opens as a single package
    /// rooted at the pubspec directory (self name, pubspec sdk lower bound
    /// as the language version) over an EMPTY `package:` map, with the
    /// missing-config advisory anchored at the path `dart pub get` would
    /// materialize. The oracle's no-config run emits `uri_does_not_exist`
    /// for every `package:` import plus the usual cascades; the empty map
    /// is what makes dartq's resolution face produce exactly that.
    #[test]
    fn missing_config_degrades_to_single_package_context() {
        let root = workspace("ws_no_config", &[("lib/main.dart", "var ok = 1;\n")]);
        fs::remove_file(root.join(".dart_tool/package_config.json")).expect("drop config");

        let root = root.canonicalize().expect("root");
        let (config, dir, advisory) =
            engine_config_with_advisory(&root, Some(&root.join("sdk"))).expect(
                "no config degrades, it does not refuse (oracle ContextLocator BasicWorkspace floor)",
            );
        assert_eq!(dir, root, "the pointed-at dir");

        // The resolution face is the empty map: every `package:` URI is an
        // unknown package (the oracle's `Packages.empty`).
        assert!(
            config.pub_config.packages.is_empty(),
            "the package: map must be empty"
        );
        // The self package still attributes, at its pubspec's sdk lower
        // bound — the value pub would write as `languageVersion`.
        let identity = config
            .packages
            .containing(&root.join("lib/main.dart"))
            .expect("self package attribution");
        assert_eq!(identity.name.as_str(), "fixture");
        let version = identity.language_version.expect("pubspec sdk bound");
        assert_eq!((version.major(), version.minor()), (3, 5));

        // The advisory: the missing-config sibling, anchored at the path
        // `dart pub get` would write.
        let advisory = advisory.expect("missing-config advisory");
        assert_eq!(
            advisory,
            WorkspaceAdvisory::MissingConfig(root.join(".dart_tool/package_config.json"))
        );
        assert_eq!(advisory.code().machine, "DARTQ/PACKAGE_CONFIG_MISSING");
        assert!(advisory.message().contains("run `dart pub get`"));

        fs::remove_dir_all(&root).ok();
    }

    /// The bare floor: no pubspec anywhere — the oracle's path-local
    /// `BasicWorkspace` (`Packages.empty`). A bare context opens with no
    /// packages at all (per-file language version falls to the workspace
    /// default) and the same missing-config advisory.
    #[test]
    fn bare_directory_opens_a_bare_context() {
        // The fixture's synthetic SDK backs the engine; the pub faces are
        // all dropped, leaving a bare directory.
        let bare = workspace("ws_bare", &[]);
        fs::remove_file(bare.join("pubspec.yaml")).expect("drop pubspec");
        fs::remove_file(bare.join("pubspec.lock")).expect("drop lock");
        fs::remove_file(bare.join(".dart_tool/package_config.json")).expect("drop config");

        let bare = bare.canonicalize().expect("bare");
        let (config, dir, advisory) = engine_config_with_advisory(&bare, Some(&bare.join("sdk")))
            .expect("no pubspec degrades to the bare floor (oracle BasicWorkspace, no refusal)");
        assert_eq!(dir, bare);
        assert_eq!(config.packages.len(), 0, "no self package");
        assert!(config.packages.root().is_none());
        assert!(config.pub_config.packages.is_empty());
        assert_eq!(
            advisory.expect("advisory").config_path(),
            bare.join(".dart_tool/package_config.json")
        );

        fs::remove_dir_all(&bare).ok();
    }

    /// The lock is optional (#759): config present, lock absent — the
    /// assembly succeeds with no advisory (no lock means no freshness
    /// comparison, not a staleness finding), and the config's entries still
    /// back the table.
    #[test]
    fn missing_lock_analyzes_without_advisory() {
        let root = workspace("ws_no_lock", &[]);
        fs::remove_file(root.join("pubspec.lock")).expect("drop lock");

        let root = root.canonicalize().expect("root");
        let (config, _, advisory) =
            engine_config_with_advisory(&root, Some(&root.join("sdk"))).expect("assembly");
        assert!(
            advisory.is_none(),
            "no lock is not an advisory: {advisory:?}"
        );
        assert_eq!(config.packages.len(), 1, "the config backs the table");

        fs::remove_dir_all(&root).ok();
    }

    /// The nested self-resolved package: `example/` carries its own
    /// pubspec (lower sdk bound) and its own `package_config.json` — its
    /// files attribute to the nested package at the nested pubspec's sdk
    /// lower bound, not the enclosing package's version, and its
    /// `package:` face joins the context universe (the single-context
    /// stand-in for the oracle's separate root, context_locator.dart
    /// `_createContextRoots`).
    #[test]
    fn nested_self_resolved_package_gets_its_own_language_version() {
        let root = workspace("ws_nested", &[("lib/main.dart", "var ok = 1;\n")]);
        fs::create_dir_all(root.join("example")).expect("example dir");
        fs::write(
            root.join("example/pubspec.yaml"),
            "name: example\nenvironment:\n  sdk: ^3.2.0\n",
        )
        .expect("example pubspec");
        fs::create_dir_all(root.join("example/.dart_tool")).expect("example dart_tool");
        fs::write(
            root.join("example/.dart_tool/package_config.json"),
            r#"{"configVersion": 2, "packages": [{"name": "example", "rootUri": "../", "languageVersion": "3.2"}], "generator": "pub"}"#,
        )
.expect("example config");
        fs::create_dir_all(root.join("example/lib")).expect("example lib dir");
        fs::write(root.join("example/lib/main.dart"), "var nested = 1;\n").expect("nested file");

        let root = root.canonicalize().expect("root");
        let (config, _, _) =
            engine_config_with_advisory(&root, Some(&root.join("sdk"))).expect("assembly");

        let nested = config
            .packages
            .containing(&root.join("example/lib/main.dart"))
            .expect("nested attribution");
        assert_eq!(nested.name.as_str(), "example");
        let version = nested.language_version.expect("nested pubspec sdk bound");
        assert_eq!((version.major(), version.minor()), (3, 2));

        // The root's own files stay attributed to the root package at the
        // root config's version.
        let own = config
            .packages
            .containing(&root.join("lib/main.dart"))
            .expect("own attribution");
        assert_eq!(own.name.as_str(), "fixture");
        assert_eq!(
            own.language_version.map(|v| (v.major(), v.minor())),
            Some((3, 5))
        );

        // The nested package's own `package:` face resolves in the context
        // universe.
        assert!(
            config
                .pub_config
                .packages
                .iter()
                .any(|entry| entry.name == "example"),
            "the nested entry joins the resolution face"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// With `--packages` the locator's `_defaultPackageConfigFile`
    /// short-circuit keeps one root everywhere, so nested-root discovery
    /// is skipped: the nested file falls back to the enclosing config
    /// entry instead of gaining its own identity.
    #[test]
    fn packages_override_skips_nested_root_discovery() {
        let root = workspace("ws_nested_override", &[]);
        fs::create_dir_all(root.join("example")).expect("example dir");
        fs::write(
            root.join("example/pubspec.yaml"),
            "name: example\nenvironment:\n  sdk: ^3.2.0\n",
        )
        .expect("example pubspec");
        fs::create_dir_all(root.join("example/.dart_tool")).expect("example dart_tool");
        fs::write(
            root.join("example/.dart_tool/package_config.json"),
            r#"{"configVersion": 2, "packages": [{"name": "example", "rootUri": "../", "languageVersion": "3.2"}], "generator": "pub"}"#,
        )
.expect("example config");
        fs::create_dir_all(root.join("example/lib")).expect("example lib dir");
        fs::write(root.join("example/lib/main.dart"), "var nested = 1;\n").expect("nested file");
        let other = root.join("other");
        fs::create_dir_all(&other).expect("other dir");
        let override_config = other.join("custom.json");
        fs::write(
            &override_config,
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "languageVersion": "3.5"}], "generator": "pub"}"#,
        )
.expect("override config");

        let root = root.canonicalize().expect("root");
        let (packages, _, _, _, _) =
            package_assembly_with(&root, Some(&override_config), None).expect("override assembly");
        let nested = packages
            .containing(&root.join("example/lib/main.dart"))
            .expect("enclosing attribution");
        assert_eq!(nested.name.as_str(), "fixture", "no nested identity added");
        assert_eq!(
            nested.language_version.map(|v| (v.major(), v.minor())),
            Some((3, 5))
        );

        fs::remove_dir_all(&root).ok();
    }

    /// The config-required tier's message source: `load_workspace_faces`'s
    /// missing-config line is the text [`package_assembly_strict`] refuses
    /// with (the `dartq compat` face's posture, through the shared body).
    #[test]
    fn compat_face_still_requires_the_config() {
        let root = workspace("ws_pubget", &[]);
        fs::remove_file(root.join(".dart_tool/package_config.json")).expect("drop config");
        let root = root.canonicalize().expect("root");
        let Err(error) = load_workspace_faces(&root, &root.join(".dart_tool/package_config.json"))
        else {
            panic!("compat's config requirement must hold")
        };
        assert_eq!(
            error.to_string(),
            format!(
                "no package config at {} (run `dart pub get`)",
                root.join(".dart_tool/package_config.json").display()
            )
        );
        fs::remove_dir_all(&root).ok();
    }

    /// A malformed `package_config.json` degrades to the oracle's
    /// `Packages.empty` floor instead of refusing (packages.dart
    /// `parsePackageConfigJsonFile` catch): the assembly succeeds with the
    /// self package only, every `package:` import unresolved, and ONE
    /// TOOL-severity advisory naming the config and the failure.
    #[test]
    fn corrupt_config_degrades_to_empty_floor() {
        let root = workspace("ws_corrupt", &[]);
        fs::write(root.join(".dart_tool/package_config.json"), "{not json")
            .expect("corrupt config");

        let root = root.canonicalize().expect("root");
        let config_path = root.join(".dart_tool/package_config.json");
        let (config, _, advisory) = engine_config_with_advisory(&root, Some(&root.join("sdk")))
            .expect(
                "a corrupt config degrades, never refuses (the oracle analyzes with \
                 Packages.empty)",
            );
        assert!(
            config.pub_config.packages.is_empty(),
            "the Packages.empty floor"
        );
        assert_eq!(config.packages.len(), 1, "the self package stays");
        assert!(config.packages.get("fixture").is_some());

        let advisory = advisory.expect("one advisory");
        assert_eq!(advisory.code(), DARTQ_PACKAGE_CONFIG_INVALID);
        assert_eq!(advisory.config_path(), config_path);
        let WorkspaceAdvisory::InvalidConfig(invalid) = &advisory else {
            panic!("invalid-config advisory, got {advisory:?}")
        };
        assert!(!invalid.reason.is_empty(), "the failure is named");
        assert!(
            advisory
                .message()
                .contains("package_config.json is unusable (")
                && advisory
                    .message()
                    .contains("analyzing with `package:` imports unresolved"),
            "{advisory:?}"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// Duplicate package names reach the same floor: the reader grammar
    /// tolerates them in the file (package:package_config reports and
    /// skips), the graph build rejects them, and the analyze path degrades
    /// with the advisory naming the defect — the compat face is the one
    /// that still refuses (its own `load_workspace_faces` call).
    #[test]
    fn duplicate_config_names_degrade_to_empty_floor() {
        let root = workspace("ws_dupes", &[]);
        fs::write(
            root.join(".dart_tool/package_config.json"),
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "languageVersion": "3.5"}, {"name": "fixture", "rootUri": "../", "languageVersion": "3.5"}], "generator": "pub", "generatorVersion": "3.13.1"}"#,
        )
.expect("duplicate config");

        let root = root.canonicalize().expect("root");
        let (config, _, advisory) = engine_config_with_advisory(&root, Some(&root.join("sdk")))
            .expect("duplicate names degrade, never refuse");
        assert!(config.pub_config.packages.is_empty(), "the empty floor");
        assert_eq!(config.packages.len(), 1, "the self package stays");

        let advisory = advisory.expect("one advisory");
        let WorkspaceAdvisory::InvalidConfig(invalid) = &advisory else {
            panic!("invalid-config advisory, got {advisory:?}")
        };
        assert!(
            invalid.reason.contains("duplicate"),
            "the reason names the defect: {}",
            invalid.reason
        );

        fs::remove_dir_all(&root).ok();
    }

    /// A malformed `pubspec.lock` never aborts the assembly: the analyzer
    /// never reads the lock, so an unparseable one degrades to the
    /// lock-absent posture — the run proceeds on the empty lock and the
    /// failure surfaces as the stale advisory's `MalformedLockFile` reason,
    /// never a refusal (daemon/CLI startup parity with the oracle).
    #[test]
    fn malformed_lock_degrades_to_lock_absent() {
        let root = workspace("ws_badlock", &[]);
        // A `source:` key nested inside `description:` — hand-mangled shape.
        fs::write(
            root.join("pubspec.lock"),
            "packages:\n  a:\n    dependency: \"direct main\"\n    description:\n      name: a\n      source: hosted\n      url: \"https://pub.dev\"\n    version: \"1.0.0\"\n",
        )
        .expect("malformed lock");

        let root = root.canonicalize().expect("root");
        let (config, _, advisory) = engine_config_with_advisory(&root, Some(&root.join("sdk")))
            .expect("a malformed lock degrades, never refuses");
        assert_eq!(config.packages.len(), 1, "the self package stays");
        assert!(config.packages.get("fixture").is_some());

        let advisory = advisory.expect("the malformed lock advisory");
        let WorkspaceAdvisory::Stale(stale) = &advisory else {
            panic!("stale advisory, got {advisory:?}")
        };
        assert!(
            stale
                .reasons
                .iter()
                .any(|reason| matches!(reason, StaleReason::MalformedLockFile(_))),
            "the parse failure is named: {:?}",
            stale.reasons
        );

        fs::remove_dir_all(&root).ok();
    }

    /// The content baseline at the assembly: a VCS-style
    /// checkout or `touch` — every mtime equal, bytes unchanged — stays
    /// fresh. Without the baseline this exact chain stales (the pubspec is
    /// not strictly older than the lock); with it, byte-identical inputs
    /// cannot invalidate the resolution the graph was just built from.
    #[test]
    fn touched_resolution_stays_fresh_with_baseline() {
        let root = workspace("ws_touched", &[]);
        let stamp = FileTime::from_unix_time(7_000, 0);
        set_file_mtime(root.join("pubspec.yaml"), stamp).expect("set mtime");
        set_file_mtime(root.join("pubspec.lock"), stamp).expect("set mtime");
        set_file_mtime(root.join(".dart_tool/package_config.json"), stamp).expect("set mtime");

        let root = root.canonicalize().expect("root");
        let (_, _, advisory) =
            engine_config_with_advisory(&root, Some(&root.join("sdk"))).expect("assembly");
        assert!(
            advisory.is_none(),
            "touched-but-unchanged inputs stay fresh: {advisory:?}"
        );

        fs::remove_dir_all(&root).ok();
    }

    /// The observed SDK version joins the freshness judgment: a config
    /// generated by a different Dart major.minor stales (upstream pub
    /// source, entrypoint.dart `isPackageConfigGeneratedBySameDartSdk`),
    /// a patch-only bump does not.
    #[test]
    fn generator_sdk_mismatch_stales() {
        let root = workspace("ws_generator", &[]);
        fs::write(root.join("sdk/version"), "3.14.0\n").expect("sdk version");

        let root = root.canonicalize().expect("root");
        let (_, _, advisory) =
            engine_config_with_advisory(&root, Some(&root.join("sdk"))).expect("assembly");
        let advisory = advisory.expect("generator mismatch stales");
        let WorkspaceAdvisory::Stale(stale) = &advisory else {
            panic!("stale advisory, got {advisory:?}")
        };
        assert_eq!(
            stale.reasons,
            vec![StaleReason::GeneratorSdkMismatch {
                config: "3.13.1".into(),
                current: "3.14.0".into(),
            }]
        );

        fs::remove_dir_all(&root).ok();
    }
}
