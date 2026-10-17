//! `ResolvedPackageGraph` assembly — the consumption product of
//! the five resolution inputs, with no version solving anywhere.
//!
//! Instance identity and the graph id, scoped to this milestone:
//! - `PackageInstance::content_hash` is the M2 incremental surface (the
//!   per-package Merkle over analyzed files). `build_graph` leaves it
//!   `None`; the field exists so downstream code can already carry the
//!   identity tuple instead of being migrated later.
//! - `PackageGraphId` is derived canonically from **(config + lock)**:
//!   both files are re-serialized deterministically (struct field order,
//!   sorted maps, canonical version strings) and hashed with the
//!   `dartq-source` content hash. Two resolutions with identical
//!   materialized artifacts therefore share a graph id regardless of
//!   whitespace in the files.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use dartq_source::{ContentId, content_hash};

use crate::PubError;
use crate::config::PackageConfig;
use crate::freshness::StaleReason;
use crate::lock::{LockDescription, LockFile, LockedPackage, SourceKind};
use crate::pubspec::Pubspec;
use crate::version::Version;

/// How a dependency edge is usable from the entry-point perspective
///.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DepKind {
    /// Reached through `dependencies` (importable from production code).
    Direct,
    /// Declared directly in the root's `dev_dependencies`.
    Dev,
    /// Reachable only through dev dependencies — a transitive package
    /// pulled in by a root dev dependency.
    DevOfTransitive,
}

/// Where an instance's bytes come from, in lock-description shape.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SourceRef {
    Hosted {
        url: String,
        sha256: Option<String>,
    },
    Git {
        url: String,
        resolved_ref: String,
        path: String,
    },
    Path {
        path: PathBuf,
    },
    Sdk {
        sdk: String,
    },
}

/// One resolved package instance: source identity, version, and
/// the (M2) content Merkle.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PackageInstance {
    pub name: String,
    pub source: SourceKind,
    pub source_ref: SourceRef,
    /// `None` for packages without a pubspec version (e.g. the implicit
    /// SDK instance).
    pub version: Option<Version>,
    /// Per-package content Merkle — always `None` from [`build_graph`] in
    /// M1; see the module docs.
    pub content_hash: Option<ContentId>,
}

/// Handle to a package inside one [`ResolvedPackageGraph`]: the package
/// name, which is unique within a resolution (the lock keys packages by
/// name, as do config entries).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageInstanceId(pub String);

/// The content-hash name of a resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PackageGraphId(ContentId);

impl PackageGraphId {
    #[must_use]
    pub fn as_content_id(&self) -> ContentId {
        self.0
    }
}

impl std::fmt::Display for PackageGraphId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One directed dependency edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageEdge {
    pub from: PackageInstanceId,
    pub to: PackageInstanceId,
    pub kind: DepKind,
}

/// The resolved package graph for one pub resolution.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedPackageGraph {
    pub id: PackageGraphId,
    /// Every package of the resolution: config entries (workspace members
    /// and root included), lock-only stragglers, plus the implicit SDK
    /// instance `"sdk:dart"` used for `dart:` bookkeeping.
    pub instances: Vec<PackageInstance>,
    pub edges: Vec<PackageEdge>,
    pub root: PackageInstanceId,
    /// Workspace members when the root is a pub workspace, root excluded;
    /// `None` outside workspaces. Members absent from the config are
    /// filtered out (config as written — outside the analysis universe).
    pub workspace_members: Option<Vec<PackageInstanceId>>,
    /// Membership disagreements between the pubspec's wishes and the config
    /// as written — root or declared member missing from the config. These
    /// never abort assembly (the oracle reads the config as-is); the caller
    /// turns them into `dartq/package_config_out_of_date` advisory reasons.
    pub membership_findings: Vec<StaleReason>,
}

/// Assembles the graph from the resolution inputs.
///
/// `config_dir` is the directory holding `package_config.json` (the
/// workspace root's `.dart_tool/`), needed to resolve relative `rootUri`s
/// and relative path descriptions. `extra_pubspecs` carries parsed pubspecs
/// for non-cache packages whose sources are available (workspace members,
/// path/git checkouts); hosted cache packages contribute instances but no
/// outgoing edges.
///
/// The graph is a faithful product of `(config, lock)`, not of the
/// pubspec's wishes: a root or declared member missing from the config
/// never aborts assembly (the oracle reads the config as-is) — the finding
/// lands in [`ResolvedPackageGraph::membership_findings`] for the caller's
/// stale-config advisory, and the root identity stays the pubspec name
/// (backed by an instance exactly when the config covers it).
///
/// # Errors
///
/// Returns [`PubError::Graph`] on duplicate config entry names — pub's
/// hard error at config load (`package_config.dart:44-51`), which the
/// oracle throws on too.
pub fn build_graph(
    config: &PackageConfig,
    lock: &LockFile,
    root: &Pubspec,
    extra_pubspecs: &BTreeMap<String, Pubspec>,
    workspace_members: Option<&[String]>,
    config_dir: &Path,
) -> Result<ResolvedPackageGraph, PubError> {
    let mut membership_findings = Vec::new();
    if config.entry(&root.name).is_none() {
        membership_findings.push(StaleReason::RootMissingFromConfig {
            name: root.name.clone(),
        });
    }

    let mut instances = Vec::new();
    let mut names = BTreeSet::new();
    for entry in &config.packages {
        if !names.insert(entry.name.clone()) {
            return Err(PubError::Graph(format!(
                "duplicate package_config entry for `{}`",
                entry.name
            )));
        }
        let (source, source_ref, version) = match lock.packages.get(&entry.name) {
            Some(locked) => (
                locked.source,
                source_ref_of(locked, config_dir),
                Some(locked.version.clone()),
            ),
            // Workspace members and the root are not in the lock; they are
            // local directories.
            None => (
                SourceKind::Path,
                SourceRef::Path {
                    path: crate::workspace::normalize(&entry.resolve_root(config_dir)?),
                },
                pubspec_version(&entry.name, root, extra_pubspecs),
            ),
        };
        instances.push(PackageInstance {
            name: entry.name.clone(),
            source,
            source_ref,
            version,
            content_hash: None,
        });
    }
    // Lock entries absent from the config: inconsistent artifacts, but the
    // data is real — carry it so the graph never silently drops packages.
    for (name, locked) in &lock.packages {
        if names.insert(name.clone()) {
            instances.push(PackageInstance {
                name: name.clone(),
                source: locked.source,
                source_ref: source_ref_of(locked, config_dir),
                version: Some(locked.version.clone()),
                content_hash: None,
            });
        }
    }
    instances.push(PackageInstance {
        name: SDK_INSTANCE_NAME.to_owned(),
        source: SourceKind::Sdk,
        source_ref: SourceRef::Sdk {
            sdk: "dart".to_owned(),
        },
        version: None,
        content_hash: None,
    });

    let edges = collect_edges(root, extra_pubspecs, &names);

    let workspace_members = match workspace_members {
        None => None,
        Some(members) => {
            // Config as written: a member without a config entry is simply
            // outside the analysis universe — filtered, and reported.
            let mut ids = Vec::with_capacity(members.len());
            for name in members {
                if names.contains(name) {
                    ids.push(PackageInstanceId(name.clone()));
                } else {
                    membership_findings
                        .push(StaleReason::MemberMissingFromConfig { name: name.clone() });
                }
            }
            Some(ids)
        }
    };

    Ok(ResolvedPackageGraph {
        id: graph_id(config, lock)?,
        instances,
        edges,
        root: PackageInstanceId(root.name.clone()),
        workspace_members,
        membership_findings,
    })
}

/// Name of the implicit SDK instance; `:` cannot occur in pub package
/// names, so no real package can collide with it.
pub const SDK_INSTANCE_NAME: &str = "sdk:dart";

fn pubspec_version(
    name: &str,
    root: &Pubspec,
    extra: &BTreeMap<String, Pubspec>,
) -> Option<Version> {
    let spec = if name == root.name {
        Some(root)
    } else {
        extra.get(name)
    };
    spec.and_then(|spec| spec.version.clone())
}

fn source_ref_of(locked: &LockedPackage, config_dir: &Path) -> SourceRef {
    match &locked.description {
        LockDescription::Hosted { url, sha256, .. } => SourceRef::Hosted {
            url: url.clone(),
            sha256: sha256.clone(),
        },
        LockDescription::Git {
            url,
            resolved_ref,
            path,
            ..
        } => SourceRef::Git {
            url: url.clone(),
            resolved_ref: resolved_ref.clone(),
            path: path.clone(),
        },
        LockDescription::Path { path, relative } => {
            // Relative path descriptions anchor at the lock's directory,
            // which sits next to `.dart_tool/`.
            let resolved = if *relative {
                crate::workspace::normalize(&config_dir.join("..").join(path))
            } else {
                Path::new(path).to_path_buf()
            };
            SourceRef::Path { path: resolved }
        }
        LockDescription::Sdk(sdk) => SourceRef::Sdk { sdk: sdk.clone() },
    }
}

/// Walks dependencies outward from the root. Only the root's dev sections
/// exist for pub (transitive dev_dependencies are not resolved), so deeper
/// hops read `dependencies` alone.
fn collect_edges(
    root: &Pubspec,
    extra: &BTreeMap<String, Pubspec>,
    known: &BTreeSet<String>,
) -> Vec<PackageEdge> {
    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    enum Reach {
        Dev,
        Direct,
    }

    let spec_of = |name: &str| -> Option<&Pubspec> {
        if name == root.name {
            Some(root)
        } else {
            extra.get(name)
        }
    };

    // (from, to) -> kind; only a Direct discovery may overwrite an earlier
    // Dev one, mirroring pub's main > dev priority.
    let mut edges: BTreeMap<(String, String), DepKind> = BTreeMap::new();
    let mut insert = |from: &str, to: &str, kind: DepKind| {
        edges
            .entry((from.to_owned(), to.to_owned()))
            .and_modify(|existing| {
                if kind == DepKind::Direct {
                    *existing = kind;
                }
            })
            .or_insert(kind);
    };

    let mut visited: BTreeMap<String, Reach> = BTreeMap::new();
    let mut frontier: Vec<(String, Reach)> = Vec::new();

    // Root sections: main + overrides are Direct, dev is Dev.
    for name in root.dependencies.keys() {
        insert(&root.name, name, DepKind::Direct);
        frontier.push((name.clone(), Reach::Direct));
    }
    for name in root.dependency_overrides.keys() {
        insert(&root.name, name, DepKind::Direct);
        frontier.push((name.clone(), Reach::Direct));
    }
    for name in root.dev_dependencies.keys() {
        insert(&root.name, name, DepKind::Dev);
        frontier.push((name.clone(), Reach::Dev));
    }

    while let Some((name, reach)) = frontier.pop() {
        if visited.get(&name).is_some_and(|seen| *seen >= reach) {
            continue;
        }
        visited.insert(name.clone(), reach);
        let Some(spec) = spec_of(&name) else {
            continue; // cache-hosted package without a provided pubspec
        };
        let edge_kind = match reach {
            Reach::Direct => DepKind::Direct,
            Reach::Dev => DepKind::DevOfTransitive,
        };
        for dep in spec.dependencies.keys() {
            if !known.contains(dep) {
                continue; // stale artifacts; the freshness advisory reports this
            }
            insert(&name, dep, edge_kind);
            frontier.push((dep.clone(), reach));
        }
    }

    edges
        .into_iter()
        .map(|((from, to), kind)| PackageEdge {
            from: PackageInstanceId(from),
            to: PackageInstanceId(to),
            kind,
        })
        .collect()
}

/// blake3-128 over the canonical (config, lock) pair — the M1 graph
/// identity (see the module docs for what this intentionally excludes).
///
/// The serialization is serde's snake_case struct-field form, an internal
/// canonical-hash dialect only — it is not package_config.json (camelCase
/// `rootUri`/`generatorVersion`) and must never be used to write one; see
/// the derive docs in `config.rs`.
fn graph_id(config: &PackageConfig, lock: &LockFile) -> Result<PackageGraphId, PubError> {
    let mut bytes = serde_json::to_vec(config).map_err(|e| PubError::Graph(e.to_string()))?;
    bytes.push(0);
    let lock_bytes = serde_json::to_vec(lock).map_err(|e| PubError::Graph(e.to_string()))?;
    bytes.extend_from_slice(&lock_bytes);
    Ok(PackageGraphId(content_hash(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PackageConfig;

    fn config_from(json: &str) -> PackageConfig {
        PackageConfig::parse(json).expect("valid config")
    }

    fn lock_from(yaml: &str) -> LockFile {
        LockFile::parse(yaml).expect("valid lock")
    }

    const CONFIG: &str = r#"{
      "configVersion": 2,
      "packages": [
        {"name": "app", "rootUri": "../", "languageVersion": "3.5"},
        {"name": "local_dep", "rootUri": "../deps/local_dep", "languageVersion": "3.5"},
        {"name": "hosted_dep", "rootUri": "file:///cache/hosted/pub.dev/hosted_dep-1.4.0", "languageVersion": "3.2"}
      ],
      "generator": "pub",
      "generatorVersion": "3.13.1"
    }"#;

    const LOCK: &str = "\
packages:
  hosted_dep:
    dependency: \"direct main\"
    description:
      name: hosted_dep
      url: \"https://pub.dev\"
      sha256: \"aa\"
    source: hosted
    version: \"1.4.0\"
  transitive_of_hosted:
    dependency: transitive
    description:
      name: transitive_of_hosted
      url: \"https://pub.dev\"
    source: hosted
    version: \"0.9.0\"
sdks:
  dart: \">=3.5.0 <4.0.0\"
";

    fn root_pubspec() -> Pubspec {
        Pubspec::parse(
            "\
name: app
version: 0.1.0
environment:
  sdk: ^3.5.0
dependencies:
  hosted_dep: ^1.0.0
  local_dep:
    path: deps/local_dep
dev_dependencies:
  dev_tool: ^2.0.0
dependency_overrides:
  hosted_dep: ^1.4.0
",
        )
        .expect("valid pubspec")
    }

    fn extra_pubspecs() -> BTreeMap<String, Pubspec> {
        let local = Pubspec::parse(
            "name: local_dep\nversion: 1.0.0\ndependencies:\n  hosted_dep: ^1.0.0\n",
        )
        .expect("valid pubspec");
        let dev_tool =
            Pubspec::parse("name: dev_tool\nversion: 2.0.0\ndependencies:\n  meta: ^1.0.0\n")
                .expect("valid pubspec");
        BTreeMap::from([
            ("local_dep".to_owned(), local),
            ("dev_tool".to_owned(), dev_tool),
        ])
    }

    #[test]
    fn builds_instances_and_typed_edges() {
        let config = config_from(CONFIG);
        let lock = lock_from(LOCK);
        let graph = build_graph(
            &config,
            &lock,
            &root_pubspec(),
            &extra_pubspecs(),
            None,
            Path::new("/proj/.dart_tool"),
        )
        .expect("graph");

        // Config entries + lock-only straggler + implicit SDK instance.
        assert_eq!(graph.instances.len(), 5);
        assert!(
            graph
                .instances
                .iter()
                .any(|i| i.name == "transitive_of_hosted")
        );
        assert!(
            graph
                .instances
                .iter()
                .any(|i| i.name == SDK_INSTANCE_NAME && i.version.is_none())
        );

        // Root instance carries its pubspec version.
        let app = graph
            .instances
            .iter()
            .find(|i| i.name == "app")
            .expect("root");
        assert_eq!(app.version.as_ref().expect("version").to_string(), "0.1.0");
        assert_eq!(app.source, SourceKind::Path);
        // Relative rootUri resolves against .dart_tool/ then normalizes.
        assert_eq!(
            app.source_ref,
            SourceRef::Path {
                path: Path::new("/proj").to_path_buf()
            }
        );

        // Hosted instance comes from the lock description.
        let hosted = graph
            .instances
            .iter()
            .find(|i| i.name == "hosted_dep")
            .expect("hosted");
        assert_eq!(
            hosted.source_ref,
            SourceRef::Hosted {
                url: "https://pub.dev".into(),
                sha256: Some("aa".into()),
            }
        );

        let kind = |from: &str, to: &str| {
            graph
                .edges
                .iter()
                .find(|edge| edge.from.0 == from && edge.to.0 == to)
                .map(|edge| edge.kind)
        };
        assert_eq!(kind("app", "hosted_dep"), Some(DepKind::Direct));
        assert_eq!(kind("app", "local_dep"), Some(DepKind::Direct));
        assert_eq!(kind("app", "dev_tool"), Some(DepKind::Dev));
        // local_dep's own dep of hosted_dep: direct at its hop.
        assert_eq!(kind("local_dep", "hosted_dep"), Some(DepKind::Direct));
        // dev_tool is only reached through dev: its deps are dev-of-transitive.
        assert_eq!(
            kind("dev_tool", "meta"),
            None,
            "`meta` has no instance (not in config/lock)"
        );
        // Overrides fold into Direct with no duplicate edge for the same
        // target.
        assert_eq!(
            graph
                .edges
                .iter()
                .filter(|edge| edge.from.0 == "app")
                .count(),
            3
        );

        assert_eq!(graph.root, PackageInstanceId("app".into()));
        assert!(graph.workspace_members.is_none());
    }

    #[test]
    fn dev_transitive_edges_and_dedup() {
        let config = config_from(CONFIG);
        let lock = lock_from(LOCK);
        // dev_tool depends on hosted_dep, which is also a main dep.
        let dev_tool =
            Pubspec::parse("name: dev_tool\nversion: 2.0.0\ndependencies:\n  hosted_dep: ^1.0.0\n")
                .expect("ok");
        let extra = BTreeMap::from([("dev_tool".to_owned(), dev_tool)]);
        let graph = build_graph(
            &config,
            &lock,
            &root_pubspec(),
            &extra,
            None,
            Path::new("/proj/.dart_tool"),
        )
        .expect("graph");
        let kind = |from: &str, to: &str| {
            graph
                .edges
                .iter()
                .find(|edge| edge.from.0 == from && edge.to.0 == to)
                .map(|edge| edge.kind)
                .expect("edge")
        };
        // dev_tool reached via dev: its dependency edge is DevOfTransitive.
        assert_eq!(kind("dev_tool", "hosted_dep"), DepKind::DevOfTransitive);
        // Single edge despite the target also being reachable directly.
        assert_eq!(
            graph
                .edges
                .iter()
                .filter(|edge| edge.from.0 == "dev_tool")
                .count(),
            1
        );
    }

    #[test]
    fn graph_id_stable_and_input_sensitive() {
        let config = config_from(CONFIG);
        let lock = lock_from(LOCK);
        let a = build_graph(
            &config,
            &lock,
            &root_pubspec(),
            &extra_pubspecs(),
            None,
            Path::new("/proj/.dart_tool"),
        )
        .expect("graph")
        .id;
        let b = build_graph(
            &config,
            &lock,
            &root_pubspec(),
            &extra_pubspecs(),
            None,
            Path::new("/proj/.dart_tool"),
        )
        .expect("graph")
        .id;
        assert_eq!(a, b, "same inputs must yield the same graph id");

        // Formatting-only differences in the config must not change the id.
        let compact = config_from(&CONFIG.replace(['\n', ' '], ""));
        let c = build_graph(
            &compact,
            &lock,
            &root_pubspec(),
            &extra_pubspecs(),
            None,
            Path::new("/proj/.dart_tool"),
        )
        .expect("graph")
        .id;
        assert_eq!(a, c, "canonical form must abstract away formatting");

        // A version bump in the lock changes the id.
        let bumped = lock_from(&LOCK.replace("\"1.4.0\"", "\"1.4.1\""));
        let d = build_graph(
            &config,
            &bumped,
            &root_pubspec(),
            &extra_pubspecs(),
            None,
            Path::new("/proj/.dart_tool"),
        )
        .expect("graph")
        .id;
        assert_ne!(a, d);
    }

    #[test]
    fn workspace_members_filter_to_the_config() {
        let config = config_from(CONFIG);
        let lock = lock_from(LOCK);
        let members = ["local_dep".to_owned()];
        let graph = build_graph(
            &config,
            &lock,
            &root_pubspec(),
            &extra_pubspecs(),
            Some(&members),
            Path::new("/proj/.dart_tool"),
        )
        .expect("graph");
        assert_eq!(
            graph.workspace_members,
            Some(vec![PackageInstanceId("local_dep".into())])
        );
        assert!(graph.membership_findings.is_empty());

        // A member without a config entry is filtered, not rejected — the
        // config as written defines the analysis universe — and the finding
        // is reported for the caller's advisory.
        let bogus = ["local_dep".to_owned(), "not_in_config".to_owned()];
        let graph = build_graph(
            &config,
            &lock,
            &root_pubspec(),
            &extra_pubspecs(),
            Some(&bogus),
            Path::new("/proj/.dart_tool"),
        )
        .expect("graph");
        assert_eq!(
            graph.workspace_members,
            Some(vec![PackageInstanceId("local_dep".into())])
        );
        assert_eq!(
            graph.membership_findings,
            vec![StaleReason::MemberMissingFromConfig {
                name: "not_in_config".into()
            }]
        );
    }

    #[test]
    fn root_missing_from_config_proceeds_with_a_finding() {
        let config = config_from(CONFIG);
        let lock = lock_from(LOCK);
        let stranger = Pubspec::parse("name: stranger\n").expect("ok");
        let graph = build_graph(
            &config,
            &lock,
            &stranger,
            &BTreeMap::new(),
            None,
            Path::new("/proj/.dart_tool"),
        )
        .expect("graph");
        // The root identity stays the pubspec name — defined, but backed by
        // no instance because the config does not cover it.
        assert_eq!(graph.root, PackageInstanceId("stranger".into()));
        assert!(graph.instances.iter().all(|i| i.name != "stranger"));
        assert_eq!(
            graph.membership_findings,
            vec![StaleReason::RootMissingFromConfig {
                name: "stranger".into()
            }]
        );
    }

    #[test]
    fn duplicate_config_names_stay_a_hard_error() {
        let config = config_from(
            r#"{
              "configVersion": 2,
              "packages": [
                {"name": "app", "rootUri": "../"},
                {"name": "app", "rootUri": "../app"}
              ],
              "generator": "pub"
            }"#,
        );
        let lock = lock_from(LOCK);
        let result = build_graph(
            &config,
            &lock,
            &root_pubspec(),
            &BTreeMap::new(),
            None,
            Path::new("/proj/.dart_tool"),
        );
        assert!(matches!(result, Err(PubError::Graph(_))));
    }
}
