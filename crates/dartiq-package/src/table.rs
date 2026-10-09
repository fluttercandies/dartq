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
use std::path::{Path, PathBuf};

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
    /// Package names ordered by root depth, deepest first —
    /// [`PackageTable::containing`]'s scan index (depths are computed once
    /// here, never per query).
    by_depth: Vec<PackageName>,
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
    /// `config_dir` (the `.dart_tool/` directory) and lexically normalized;
    /// each entry's `packageUri` is joined onto its root and normalized the
    /// same way. Source categories come from the graph instances' source
    /// references.
    ///
    /// `config_dir` must be absolute — the engine's assembly step
    /// (engine-api-spec) passes the absolute `.dart_tool/` directory. A
    /// relative one would silently produce wrong roots, and build is
    /// one-shot (no later step could recover), so the violation is a hard
    /// [`crate::PackageError::RelativeConfigDir`], not a debug-only assert.
    ///
    /// Two more root invariants are enforced at build, mirroring the
    /// oracle reader: a normalized `packageUri` directory must stay inside
    /// its package root ([`crate::PackageError::LibDirOutsideRoot`]), and
    /// two entries must not resolve to the same normalized root
    /// ([`crate::PackageError::DuplicateRoot`] — file→package attribution
    /// could not pick an owner).
    ///
    /// # Errors
    ///
    /// Returns [`crate::PackageError`] when the artifacts disagree (a config
    /// entry without a graph instance, the root or a workspace member
    /// without a config entry, duplicate entries), when a config name is
    /// outside the reader's loose grammar, when a `rootUri` cannot be
    /// resolved, or when a root invariant above is violated.
    pub fn build(
        config: &PackageConfig,
        graph: &ResolvedPackageGraph,
        config_dir: &Path,
    ) -> Result<PackageTable, crate::PackageError> {
        if !config_dir.is_absolute() {
            return Err(crate::PackageError::RelativeConfigDir {
                path: config_dir.display().to_string(),
            });
        }

        // Last-wins when the graph carries two instances of one name —
        // unreachable through `build_graph` (it rejects duplicate config
        // names), only via hand-assembled graphs.
        let instances: BTreeMap<&str, &dartiq_pub::PackageInstance> = graph
            .instances
            .iter()
            .map(|instance| (instance.name.as_str(), instance))
            .collect();

        let mut packages = BTreeMap::new();
        // Normalized root -> the first config entry claiming it.
        let mut root_owners: BTreeMap<PathBuf, PackageName> = BTreeMap::new();
        for entry in &config.packages {
            // Config entries use the reader's loose grammar — stricter
            // pubspec names never reach here as entries (see
            // `is_valid_config_name`).
            if !is_valid_config_name(&entry.name) {
                return Err(crate::PackageError::InvalidConfigName {
                    name: entry.name.clone(),
                });
            }
            let name = PackageName::new_unchecked(entry.name.clone());
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
            let lib_dir = normalize(&root.join(lib_dir_name(&entry.package_uri)));
            // The `packageUri` directory must be the root or below it
            // (`package_config-3.0.0` `package_config.dart:838`); after
            // normalization a `../lib` spelling escapes and is rejected
            // loudly instead of silently mis-resolving.
            if lib_dir.strip_prefix(&root).is_err() {
                return Err(crate::PackageError::LibDirOutsideRoot {
                    name: entry.name.clone(),
                    lib_dir: lib_dir.display().to_string(),
                    root: root.display().to_string(),
                });
            }
            // Duplicate normalized roots make file→package attribution
            // ambiguous (analyzer analog: `package_map_resolver.dart:33-52`
            // scans every root per file, where two equal roots race).
            if let Some(first) = root_owners.get(&root) {
                return Err(crate::PackageError::DuplicateRoot {
                    first: first.to_string(),
                    name: entry.name.clone(),
                    root: root.display().to_string(),
                });
            }
            root_owners.insert(root.clone(), name.clone());
            let identity = PackageIdentity {
                lib_dir,
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

        // Deepest-first root index for `containing`: roots are sorted by
        // component depth once, so each query stops at the first prefix
        // hit instead of scoring every package.
        let mut by_depth: Vec<(&PackageName, usize)> = packages
            .iter()
            .map(|(name, identity)| (name, identity.root.components().count()))
            .collect();
        by_depth.sort_by_key(|&(_, depth)| std::cmp::Reverse(depth));
        let by_depth = by_depth.into_iter().map(|(name, _)| name.clone()).collect();

        Ok(PackageTable {
            packages,
            by_depth,
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
    ///
    /// Roots are scanned deepest-first (the `by_depth` index), with an
    /// early exit on the first hit: a nested root always has strictly more
    /// components than any root it is contained by, and two distinct roots
    /// of equal depth can never both prefix one path, so the first hit is
    /// the deepest — no full-table scan, which matters because the engine
    /// calls this per file.
    #[must_use]
    pub fn containing(&self, path: &Path) -> Option<&PackageIdentity> {
        self.by_depth
            .iter()
            .map(|name| &self.packages[name])
            .find(|package| path.strip_prefix(&package.root).is_ok())
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

/// The package_config reader's loose name grammar — every character in
/// the reader charset with at least one non-`.` character
/// (`package_config-3.0.0` `util.dart:12-36`, `_validPackageNameCharacters`
/// + `checkPackageName`), so config-written names like `1abc`/`-foo` are
/// consumed as pub wrote them. Deliberately looser than the pubspec layer's
/// `is_valid_package_name` (dartiq-pub `pubspec.rs:368`), which keeps its
/// strict grammar: the config file is read, not re-published.
fn is_valid_config_name(name: &str) -> bool {
    const CHARSET: &str =
        "!$&'()*+,-.;=@0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ_abcdefghijklmnopqrstuvwxyz~";
    let mut non_dot = false;
    for c in name.chars() {
        if !CHARSET.contains(c) {
            return false;
        }
        non_dot |= c != '.';
    }
    non_dot
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

    /// A graph over `config` without workspace or extra-pubspec plumbing —
    /// the minimal fixture for table-build-only scenarios.
    fn plain_graph(config: &PackageConfig) -> ResolvedPackageGraph {
        let lock = LockFile::parse(LOCK).expect("valid lock");
        build_graph(
            config,
            &lock,
            &root_pubspec(),
            &BTreeMap::new(),
            None,
            Path::new("/proj/.dart_tool"),
        )
        .expect("graph")
    }

    fn plain_table(config_text: &str) -> PackageTable {
        let config = PackageConfig::parse(config_text).expect("valid config");
        PackageTable::build(
            &config,
            &plain_graph(&config),
            Path::new("/proj/.dart_tool"),
        )
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
        // A space is outside even the reader's loose charset, so the
        // config layer rejects it with the config-grammar error.
        let invalid = PackageConfig::parse(
            r#"{"configVersion": 2, "packages": [{"name": "foo bar", "rootUri": "../"}]}"#,
        )
        .expect("parses fine at the config layer");
        let graph = graph_for(&PackageConfig::parse(CONFIG).expect("valid config"));
        assert!(matches!(
            PackageTable::build(&invalid, &graph, Path::new("/proj/.dart_tool")),
            Err(crate::PackageError::InvalidConfigName { name }) if name == "foo bar"
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
    fn normalizes_package_uri_directory() {
        // `./` segments and trailing slashes in `packageUri` collapse —
        // `lib_dir` is normalized exactly like `root`.
        let table = plain_table(
            r#"{
              "configVersion": 2,
              "packages": [
                {"name": "app", "rootUri": "../", "packageUri": "./lib"},
                {"name": "local_dep", "rootUri": "../deps/local_dep", "packageUri": "src/"}
              ],
              "generator": "pub"
            }"#,
        );
        let app = table.get("app").expect("app entry");
        assert_eq!(app.root, PathBuf::from("/proj"));
        assert_eq!(app.lib_dir, PathBuf::from("/proj/lib"));
        let local = table.get("local_dep").expect("local entry");
        assert_eq!(local.root, PathBuf::from("/proj/deps/local_dep"));
        assert_eq!(local.lib_dir, PathBuf::from("/proj/deps/local_dep/src"));
    }

    #[test]
    fn package_uri_outside_root_is_an_error() {
        // A `../lib` packageUri escapes the package root after
        // normalization — rejected loudly instead of silently
        // mis-resolving (`package_config-3.0.0` `package_config.dart:838`).
        let config = PackageConfig::parse(
            r#"{
              "configVersion": 2,
              "packages": [{"name": "app", "rootUri": "../", "packageUri": "../lib"}],
              "generator": "pub"
            }"#,
        )
        .expect("valid config");
        let graph = plain_graph(&config);
        match PackageTable::build(&config, &graph, Path::new("/proj/.dart_tool")) {
            Err(crate::PackageError::LibDirOutsideRoot {
                name,
                lib_dir,
                root,
            }) => {
                assert_eq!(name, "app");
                assert_eq!(lib_dir, "/lib");
                assert_eq!(root, "/proj");
            }
            other => panic!("expected LibDirOutsideRoot, got {other:?}"),
        }
    }

    #[test]
    fn accepts_config_reader_loose_names() {
        // The reader grammar (`package_config-3.0.0` `util.dart:12-36`)
        // accepts names the pubspec grammar rejects — config entries are
        // read as pub wrote them, not re-published.
        let table = plain_table(
            r#"{
              "configVersion": 2,
              "packages": [
                {"name": "app", "rootUri": "../"},
                {"name": "1abc", "rootUri": "../one"},
                {"name": "-foo", "rootUri": "../-foo"}
              ],
              "generator": "pub"
            }"#,
        );
        assert_eq!(table.len(), 3);
        assert!(table.get("1abc").is_some());
        assert!(table.get("-foo").is_some());

        // All-dot names fail the non-dot rule even though `.` is
        // in-charset.
        let all_dots = PackageConfig::parse(
            r#"{
              "configVersion": 2,
              "packages": [{"name": "..", "rootUri": "../"}],
              "generator": "pub"
            }"#,
        )
        .expect("valid config");
        let graph = plain_graph(&all_dots);
        assert!(matches!(
            PackageTable::build(&all_dots, &graph, Path::new("/proj/.dart_tool")),
            Err(crate::PackageError::InvalidConfigName { name }) if name == ".."
        ));
    }

    #[test]
    fn duplicate_normalized_roots_are_rejected() {
        // Distinct spellings, one directory: `../deps/..` normalizes to
        // `/proj` like `../` — file→package attribution could not pick an
        // owner (`package_map_resolver.dart:33-52` analog).
        let config = PackageConfig::parse(
            r#"{
              "configVersion": 2,
              "packages": [
                {"name": "app", "rootUri": "../"},
                {"name": "alias", "rootUri": "../deps/.."}
              ],
              "generator": "pub"
            }"#,
        )
        .expect("valid config");
        let graph = plain_graph(&config);
        match PackageTable::build(&config, &graph, Path::new("/proj/.dart_tool")) {
            Err(crate::PackageError::DuplicateRoot { first, name, root }) => {
                assert_eq!(first, "app");
                assert_eq!(name, "alias");
                assert_eq!(root, "/proj");
            }
            other => panic!("expected DuplicateRoot, got {other:?}"),
        }
    }

    #[test]
    fn containing_prefers_deepest_root() {
        // Three nested roots: a file attributes to the deepest containing
        // package, at component boundaries.
        let table = plain_table(
            r#"{
              "configVersion": 2,
              "packages": [
                {"name": "app", "rootUri": "../"},
                {"name": "lvl1", "rootUri": "../lvl1"},
                {"name": "inner", "rootUri": "../lvl1/lvl2/inner"}
              ],
              "generator": "pub"
            }"#,
        );
        let expect_owner = |path: &str, package: &str| {
            assert_eq!(
                table
                    .containing(Path::new(path))
                    .map(|identity| identity.name.as_str()),
                Some(package),
                "`{path}` must attribute to `{package}`"
            );
        };
        expect_owner("/proj/lvl1/lvl2/inner/lib/a.dart", "inner");
        // The root directory itself belongs to its package.
        expect_owner("/proj/lvl1/lvl2/inner", "inner");
        expect_owner("/proj/lvl1/lib/b.dart", "lvl1");
        expect_owner("/proj/lib/c.dart", "app");
        expect_owner("/proj/lvl1/lvl2_other/lib/d.dart", "lvl1");
        // Component boundary: `lvl10` shares a string prefix with `lvl1`
        // but is not inside it.
        expect_owner("/proj/lvl10/lib/e.dart", "app");
        // Above every root.
        assert!(
            table
                .containing(Path::new("/elsewhere/lib/f.dart"))
                .is_none()
        );
    }

    #[test]
    fn relative_config_dir_is_an_error() {
        // The absolute-`config_dir` contract is a hard error: build is
        // one-shot and a relative dir would silently produce wrong roots.
        let config = PackageConfig::parse(CONFIG).expect("valid config");
        let graph = graph_for(&config);
        assert!(matches!(
            PackageTable::build(&config, &graph, Path::new(".dart_tool")),
            Err(crate::PackageError::RelativeConfigDir { path }) if path == ".dart_tool"
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
