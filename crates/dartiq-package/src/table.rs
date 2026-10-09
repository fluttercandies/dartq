//! The name-keyed identity table over one pub resolution.
//!
//! [`PackageTable::build`] composes the two `dartiq-pub` artifacts —
//! [`PackageConfig`] (which carries each package's local `rootUri`) and
//! [`ResolvedPackageGraph`] (which carries the instance identities) — into
//! the map resolution consumes. Entries present in one artifact but missing
//! from the other are an error, never a silent drop: lock-only stragglers
//! and the implicit SDK instance have no config root and are excluded by
//! construction.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use dartiq_pub::{PackageConfig, ResolvedPackageGraph, normalize};

use crate::SourceCategory;
use crate::identity::PackageIdentity;
use crate::name::PackageName;

/// The identity view of one resolution: every package that has a
/// `package_config.json` entry, keyed by canonical name.
///
/// A successful [`PackageTable::build`] always contains the resolution's
/// root package — it errors with [`crate::PackageError::MissingFromConfig`]
/// otherwise — so a built table is never empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageTable {
    packages: BTreeMap<PackageName, PackageIdentity>,
    root: PackageName,
    /// Workspace members when the root is a pub workspace, root excluded;
    /// `None` outside workspaces (mirrors
    /// [`ResolvedPackageGraph::workspace_members`]).
    workspace_members: Option<BTreeSet<PackageName>>,
}

impl PackageTable {
    /// Composes the table from the config and its resolved graph.
    ///
    /// Package roots are resolved from the config's `rootUri` against
    /// `config_dir` (the `.dart_tool/` directory) and lexically normalized.
    /// Source categories come from the graph instances' source references.
    ///
    /// `config_dir` must be absolute — the engine's assembly step
    /// (engine-api-spec) passes the absolute `.dart_tool/` directory. That
    /// contract is enforced with `debug_assert!` rather than prose alone:
    /// a violation is an engine-side bug that would silently produce wrong
    /// roots, so it should fail loudly in debug builds and tests at the
    /// boundary where it is introduced.
    ///
    /// # Errors
    ///
    /// Returns [`crate::PackageError`] when the artifacts disagree (a config
    /// entry without a graph instance, the root or a workspace member
    /// without a config entry, duplicate entries), when a name is outside
    /// pub's grammar, or when a `rootUri` cannot be resolved.
    pub fn build(
        config: &PackageConfig,
        graph: &ResolvedPackageGraph,
        config_dir: &Path,
    ) -> Result<PackageTable, crate::PackageError> {
        debug_assert!(
            config_dir.is_absolute(),
            "config_dir must be absolute, got `{}`",
            config_dir.display()
        );

        // Last-wins when the graph carries two instances of one name —
        // unreachable through `build_graph` (it rejects duplicate config
        // names), only via hand-assembled graphs.
        let instances: BTreeMap<&str, &dartiq_pub::PackageInstance> = graph
            .instances
            .iter()
            .map(|instance| (instance.name.as_str(), instance))
            .collect();

        let mut packages = BTreeMap::new();
        for entry in &config.packages {
            let name = PackageName::new(&entry.name)?;
            if packages.contains_key(&name) {
                return Err(crate::PackageError::DuplicateEntry {
                    name: entry.name.clone(),
                });
            }
            let instance = instances.get(entry.name.as_str()).ok_or_else(|| {
                crate::PackageError::MissingFromGraph {
                    name: entry.name.clone(),
                }
            })?;
            let root = normalize(&entry.resolve_root(config_dir)?);
            let identity = PackageIdentity {
                lib_dir: root.join(lib_dir_name(&entry.package_uri)),
                name,
                root,
                language_version: entry.language_version,
                category: SourceCategory::of(&instance.source_ref),
            };
            packages.insert(identity.name.clone(), identity);
        }

        let root_name = PackageName::new(&graph.root.0)?;
        if !packages.contains_key(&root_name) {
            return Err(crate::PackageError::MissingFromConfig {
                name: root_name.to_string(),
            });
        }
        let workspace_members = match &graph.workspace_members {
            None => None,
            Some(members) => {
                let mut names = BTreeSet::new();
                for member in members {
                    let name = PackageName::new(&member.0)?;
                    if !packages.contains_key(&name) {
                        return Err(crate::PackageError::MissingFromConfig {
                            name: name.to_string(),
                        });
                    }
                    names.insert(name);
                }
                Some(names)
            }
        };

        Ok(PackageTable {
            packages,
            root: root_name,
            workspace_members,
        })
    }

    /// The entrypoint package of the resolution.
    #[must_use]
    pub fn root(&self) -> &PackageName {
        &self.root
    }

    /// Looks a package up by name (raw, as it appears in `package:` URIs).
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&PackageIdentity> {
        self.packages.get(name)
    }

    /// The package whose root directory contains `path` — the engine's
    /// file→package attribution entry point, and the single source of truth
    /// for that mapping.
    ///
    /// Matching is a lexical, component-boundary prefix over the normalized
    /// roots (`Path::strip_prefix` semantics): `/projx/lib.dart` does not
    /// match a `/proj` package. When roots nest — a path dependency inside
    /// the root package's tree — the deepest matching root wins, so a file
    /// under `/proj/deps/local_dep` attributes to `local_dep`, not to the
    /// root package at `/proj`.
    #[must_use]
    pub fn containing(&self, path: &Path) -> Option<&PackageIdentity> {
        self.packages
            .values()
            .filter(|package| path.strip_prefix(&package.root).is_ok())
            .max_by_key(|package| package.root.components().count())
    }

    /// All packages, ordered by name.
    pub fn iter(&self) -> impl Iterator<Item = &PackageIdentity> {
        self.packages.values()
    }

    /// Number of packages. A successful build always contains the root
    /// package, so there is deliberately no `is_empty` query — the answer
    /// would be a constant `false`.
    #[allow(clippy::len_without_is_empty)]
    #[must_use]
    pub fn len(&self) -> usize {
        self.packages.len()
    }

    /// True when the resolution is a pub workspace and `name` is one of its
    /// members (the root is a member of its own workspace per pub, but is
    /// reported via [`PackageTable::root`], not here).
    #[must_use]
    pub fn is_workspace_member(&self, name: &str) -> bool {
        self.workspace_members
            .as_ref()
            .is_some_and(|members| members.contains(name))
    }

    /// Workspace member names, ordered; `None` outside workspaces.
    #[must_use]
    pub fn workspace_members(&self) -> Option<&BTreeSet<PackageName>> {
        self.workspace_members.as_ref()
    }
}

/// The `lib/`-style directory name from a config entry's `packageUri`
/// (already slash-normalized by the parser).
fn lib_dir_name(package_uri: &str) -> &str {
    package_uri.trim_end_matches('/')
}

#[cfg(test)]
mod tests {
    use super::*;
    use dartiq_pub::{LockFile, Pubspec, build_graph};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    const CONFIG: &str = r#"{
      "configVersion": 2,
      "packages": [
        {"name": "app", "rootUri": "../", "languageVersion": "3.5"},
        {"name": "local_dep", "rootUri": "../deps/../deps/local_dep", "languageVersion": "3.5"},
        {"name": "hosted_dep", "rootUri": "file:///cache/hosted/pub.dev/hosted_dep-1.4.0", "languageVersion": "3.2"}
      ],
      "generator": "pub"
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
",
        )
        .expect("valid pubspec")
    }

    fn extra_pubspecs() -> BTreeMap<String, Pubspec> {
        let local = Pubspec::parse("name: local_dep\nversion: 1.0.0\n").expect("valid pubspec");
        BTreeMap::from([("local_dep".to_owned(), local)])
    }

    fn graph_for(config: &PackageConfig) -> ResolvedPackageGraph {
        let lock = LockFile::parse(LOCK).expect("valid lock");
        build_graph(
            config,
            &lock,
            &root_pubspec(),
            &extra_pubspecs(),
            Some(&["local_dep".to_owned()]),
            Path::new("/proj/.dart_tool"),
        )
        .expect("graph")
    }

    fn table_for(config: &PackageConfig) -> PackageTable {
        PackageTable::build(config, &graph_for(config), Path::new("/proj/.dart_tool"))
            .expect("table")
    }

    #[test]
    fn composes_roots_categories_and_membership() {
        let table = table_for(&PackageConfig::parse(CONFIG).expect("valid config"));

        assert_eq!(table.len(), 3);
        assert_eq!(table.root().as_str(), "app");
        assert_eq!(table.workspace_members().map(BTreeSet::len), Some(1));
        assert!(table.is_workspace_member("local_dep"));
        assert!(!table.is_workspace_member("hosted_dep"));
        assert!(!table.is_workspace_member("app"));

        let root = table.get("app").expect("root entry");
        // `../` against .dart_tool resolves and normalizes to the project.
        assert_eq!(root.root, PathBuf::from("/proj"));
        assert_eq!(root.lib_dir, PathBuf::from("/proj/lib"));
        assert_eq!(
            root.language_version.map(|v| v.to_string()).as_deref(),
            Some("3.5")
        );
        assert_eq!(root.category, SourceCategory::Path);

        // Dot segments in rootUri collapse lexically.
        let local = table.get("local_dep").expect("member entry");
        assert_eq!(local.root, PathBuf::from("/proj/deps/local_dep"));
        assert_eq!(local.category, SourceCategory::Path);

        // Hosted content lives in the pub cache.
        let hosted = table.get("hosted_dep").expect("hosted entry");
        assert_eq!(
            hosted.root,
            PathBuf::from("/cache/hosted/pub.dev/hosted_dep-1.4.0")
        );
        assert_eq!(hosted.category, SourceCategory::PubCache);

        // Ordered iteration, keyed by name.
        let names: Vec<&str> = table.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["app", "hosted_dep", "local_dep"]);
    }

    #[test]
    fn skips_lock_only_and_sdk_instances() {
        // A transitive lock package absent from the config, plus the
        // implicit `sdk:dart` instance, must not appear: no config root.
        let config = r#"{
          "configVersion": 2,
          "packages": [{"name": "app", "rootUri": "../"}],
          "generator": "pub"
        }"#;
        let config = PackageConfig::parse(config).expect("valid config");
        let lock = LockFile::parse(
            "\
packages:
  straggler:
    dependency: transitive
    description:
      name: straggler
      url: \"https://pub.dev\"
    source: hosted
    version: \"0.1.0\"
sdks:
  dart: \">=3.5.0 <4.0.0\"
",
        )
        .expect("valid lock");
        let graph = build_graph(
            &config,
            &lock,
            &root_pubspec(),
            &BTreeMap::new(),
            None,
            Path::new("/proj/.dart_tool"),
        )
        .expect("graph");
        assert!(graph.instances.len() > config.packages.len());

        let table =
            PackageTable::build(&config, &graph, Path::new("/proj/.dart_tool")).expect("table");
        assert_eq!(table.len(), 1);
        assert!(table.get("straggler").is_none());
        assert!(table.get("sdk:dart").is_none());
    }

    #[test]
    fn config_entry_without_graph_instance_is_an_error() {
        let full = PackageConfig::parse(CONFIG).expect("valid config");
        let graph = graph_for(&full);
        // A config the graph was not built from: one entry has no instance.
        let mismatched = PackageConfig::parse(
            r#"{"configVersion": 2, "packages": [
                {"name": "app", "rootUri": "../"},
                {"name": "phantom", "rootUri": "../phantom"}]}"#,
        )
        .expect("valid config");
        assert!(matches!(
            PackageTable::build(&mismatched, &graph, Path::new("/proj/.dart_tool")),
            Err(crate::PackageError::MissingFromGraph { name }) if name == "phantom"
        ));
    }

    #[test]
    fn graph_root_or_member_without_config_entry_is_an_error() {
        let full = PackageConfig::parse(CONFIG).expect("valid config");
        let graph = graph_for(&full);
        // Same graph (root `app`, member `local_dep`), config without both.
        let bare = PackageConfig::parse(
            r#"{"configVersion": 2, "packages": [{"name": "hosted_dep", "rootUri": "../"}]}"#,
        )
        .expect("valid config");
        assert!(matches!(
            PackageTable::build(&bare, &graph, Path::new("/proj/.dart_tool")),
            Err(crate::PackageError::MissingFromConfig { name }) if name == "app"
        ));
    }

    #[test]
    fn invalid_and_duplicate_names_are_rejected() {
        let invalid = PackageConfig::parse(
            r#"{"configVersion": 2, "packages": [{"name": "foo bar", "rootUri": "../"}]}"#,
        )
        .expect("parses fine at the config layer");
        let graph = graph_for(&PackageConfig::parse(CONFIG).expect("valid config"));
        assert!(matches!(
            PackageTable::build(&invalid, &graph, Path::new("/proj/.dart_tool")),
            Err(crate::PackageError::InvalidName { .. })
        ));

        let duplicated = PackageConfig::parse(
            r#"{"configVersion": 2, "packages": [
                {"name": "app", "rootUri": "../"},
                {"name": "app", "rootUri": "../"}]}"#,
        )
        .expect("parses fine at the config layer");
        let graph = graph_for(&PackageConfig::parse(CONFIG).expect("valid config"));
        assert!(matches!(
            PackageTable::build(&duplicated, &graph, Path::new("/proj/.dart_tool")),
            Err(crate::PackageError::DuplicateEntry { name }) if name == "app"
        ));
    }

    #[test]
    fn normalizes_relative_and_dot_paths() {
        // Pins the shared `dartiq_pub::normalize` contract this crate now
        // reuses — the equivalence evidence for the deduplicated normalizer.
        assert_eq!(
            dartiq_pub::normalize(Path::new("/proj/.dart_tool/..")),
            PathBuf::from("/proj")
        );
        assert_eq!(
            dartiq_pub::normalize(Path::new("/a/./b//c")),
            PathBuf::from("/a/b/c")
        );
        assert_eq!(
            dartiq_pub::normalize(Path::new("a/../b")),
            PathBuf::from("b")
        );
        assert_eq!(
            dartiq_pub::normalize(Path::new("../up")),
            PathBuf::from("../up")
        );
        assert_eq!(
            dartiq_pub::normalize(Path::new("../../..")),
            PathBuf::from("../../..")
        );
    }

    #[test]
    fn sdk_sourced_config_entry_maps_to_sdk_category() {
        // An sdk-source package with a config entry classifies as Sdk via
        // its graph instance's source ref, regardless of its location.
        let config = r#"{
          "configVersion": 2,
          "packages": [
            {"name": "app", "rootUri": "../"},
            {"name": "flutter_pkg", "rootUri": "file:///flutter/bin/cache/pkg/flutter_pkg"}
          ],
          "generator": "pub"
        }"#;
        let config = PackageConfig::parse(config).expect("valid config");
        let lock = LockFile::parse(
            "\
packages:
  flutter_pkg:
    dependency: \"direct main\"
    description: flutter
    source: sdk
    version: \"0.0.0\"
sdks:
  dart: \">=3.5.0 <4.0.0\"
",
        )
        .expect("valid lock");
        let graph = build_graph(
            &config,
            &lock,
            &root_pubspec(),
            &BTreeMap::new(),
            None,
            Path::new("/proj/.dart_tool"),
        )
        .expect("graph");
        let table =
            PackageTable::build(&config, &graph, Path::new("/proj/.dart_tool")).expect("table");
        assert_eq!(table.len(), 2);
        let flutter = table.get("flutter_pkg").expect("sdk package");
        assert_eq!(flutter.category, SourceCategory::Sdk);
        assert_eq!(
            flutter.root,
            PathBuf::from("/flutter/bin/cache/pkg/flutter_pkg")
        );
    }
}
