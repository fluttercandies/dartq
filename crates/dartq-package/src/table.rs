//! The name-keyed identity table over one pub resolution.
//!
//! [`PackageTable::build`] composes the two `dartq-pub` artifacts —
//! [`PackageConfig`] (which carries each package's local `rootUri`) and
//! [`ResolvedPackageGraph`] (which carries the instance identities) — into
//! the map resolution consumes. Root/lib-dir resolution and the shared
//! entry policies (duplicate names, reader name grammar, `packageUri`
//! subtree validation) come from [`PackageConfig::resolve`], dartq-pub's
//! single face; this crate adds only resolution-specific composition and
//! file→package attribution. Entries present in one artifact but missing
//! from the other are an error, never a silent drop: lock-only stragglers
//! and the implicit SDK instance have no config root and are excluded by
//! construction.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{PoisonError, RwLock};

use dartq_pub::{LanguageVersion, PackageConfig, ResolvedPackageGraph, normalize};
use dartq_source::normalize_path;
use rustc_hash::FxHashMap;

use crate::SourceCategory;
use crate::identity::PackageIdentity;
use crate::name::PackageName;

/// The identity view of one resolution: every package that has a
/// `package_config.json` entry, keyed by canonical name.
///
/// The resolution's root is carried by name ([`PackageTable::root`]) even
/// when the config does not cover it — a stale config never aborts the
/// build (config as written; the stale-config advisory reports the
/// finding) — so [`PackageTable::get`] on the root name answers `None`
/// exactly when the config lacks the entry (pub-written configs always
/// have it).
#[derive(Debug)]
pub struct PackageTable {
    packages: BTreeMap<PackageName, PackageIdentity>,
    /// Exact normalized root → name: the ancestor walk's membership test
    /// (every config member root is the pubspec directory pub wrote, so the
    /// nearest exact-root ancestor is the nearest pubspec ancestor that is a
    /// config member — no filesystem probe needed).
    by_root: FxHashMap<PathBuf, PackageName>,
    /// Package names ordered by root depth, deepest first — the fallback
    /// prefix scan for content outside the workspace bound (the pub-cache
    /// tier; depths are computed once here, never per query).
    by_depth: Vec<PackageName>,
    /// The entrypoint package's name, `None` for a context with no named
    /// root package (the bare degraded floor — no pubspec anywhere on the
    /// walk).
    root: Option<PackageName>,
    /// Workspace members when the root is a pub workspace, root excluded;
    /// `None` outside workspaces (mirrors
    /// [`ResolvedPackageGraph::workspace_members`]).
    workspace_members: Option<BTreeSet<PackageName>>,
    /// The workspace root: the directory holding the config's `.dart_tool/`.
    /// The ancestor walk never ascends past it (oracle
    /// `pub.dart:334-358`: `isWithin(root, filePath)` then
    /// `folder.withAncestors`, the `current.path == root` stop).
    workspace_root: PathBuf,
    /// Memoized per-directory attribution (oracle `_workspacePackages`,
    /// `pub.dart:187`, backfilled over the walked chain):
    /// directory → owning package, `None` recording "walked, found nothing
    /// in bound; the fallback also owns the chain". Keys are directories
    /// only — file queries are never memoized, so entries are bounded by
    /// the walked directory count, not the file count. Interior mutability
    /// because attribution is a query over shared immutable state. The
    /// parallel fan-outs query it per file, so the dominant hit — the
    /// file's parent directory already walked — is served under the read
    /// lock; only a walk takes the write lock.
    ancestor_memo: RwLock<FxHashMap<Box<Path>, Option<PackageName>>>,
}

impl Clone for PackageTable {
    fn clone(&self) -> Self {
        Self {
            packages: self.packages.clone(),
            by_root: self.by_root.clone(),
            by_depth: self.by_depth.clone(),
            root: self.root.clone(),
            workspace_members: self.workspace_members.clone(),
            workspace_root: self.workspace_root.clone(),
            // The memo is a pure cache; a clone starts cold.
            ancestor_memo: RwLock::new(FxHashMap::default()),
        }
    }
}

impl PackageTable {
    /// Composes the table from the config and its resolved graph.
    ///
    /// Root/lib-dir resolution — `rootUri` against `config_dir`, lexical
    /// normalization, `packageUri` join, and the shared entry policies
    /// (duplicate names are pub's hard error at
    /// `package_config.dart:44-51`, names against the reader's loose
    /// grammar, `packageUri` subtree validation) — is
    /// [`PackageConfig::resolve`], dartq-pub's single face shared with the
    /// resolution layer's `package:` table. This build adds what is
    /// resolution-specific: the canonical spelling universe (every stored
    /// root, lib dir, and the workspace bound go through dartq-source's
    /// normalize rule — canonicalized longest existing ancestor — so
    /// attribution holds no matter whether the caller canonicalized its
    /// assembly path, the C5 `/var` vs `/private/var` caliber), source
    /// categories from the graph instances' source references, and the
    /// attribution-unique-root invariant.
    ///
    /// `config_dir` must be absolute (enforced by
    /// [`PackageConfig::resolve`] — the engine's assembly step passes the
    /// absolute `.dart_tool/` directory; a relative one would silently
    /// produce wrong roots, and build is one-shot).
    ///
    /// One more root invariant is enforced here: two entries must not
    /// resolve to the same normalized root
    /// ([`crate::PackageError::DuplicateRoot`] — file→package attribution
    /// could not pick an owner). The attribution walk's inclusive bound is
    /// the workspace root (the `config_dir` parent) whether or not it is
    /// itself a config root: pub always writes the root package's
    /// `rootUri` so, and a divergent hand-config merely degrades
    /// attribution — the bound's own tree falls through to the fallback
    /// tier instead of being rejected at build (config as written; the
    /// stale-config advisory reports the finding).
    ///
    /// # Errors
    ///
    /// Returns [`crate::PackageError`] when the artifacts disagree (a config
    /// entry without a graph instance, or a workspace member without a
    /// config entry in a hand-assembled graph), when
    /// [`PackageConfig::resolve`] rejects the config (duplicate names,
    /// grammar, subtree, relative dir), or when two entries resolve to the
    /// same normalized root.
    pub fn build(
        config: &PackageConfig,
        graph: &ResolvedPackageGraph,
        config_dir: &Path,
    ) -> Result<PackageTable, crate::PackageError> {
        let resolved = config
            .resolve(config_dir)
            .map_err(crate::PackageError::Pub)?;

        // Last-wins when the graph carries two instances of one name —
        // unreachable through `build_graph` (it rejects duplicate config
        // names), only via hand-assembled graphs.
        let instances: BTreeMap<&str, &dartq_pub::PackageInstance> = graph
            .instances
            .iter()
            .map(|instance| (instance.name.as_str(), instance))
            .collect();

        let mut packages = BTreeMap::new();
        // Normalized root -> the first config entry claiming it.
        let mut root_owners: BTreeMap<PathBuf, PackageName> = BTreeMap::new();
        for package in &resolved {
            // `resolve` validated the name against the reader grammar.
            let name = PackageName::new_unchecked(package.name.clone());
            let instance = instances.get(package.name.as_str()).ok_or_else(|| {
                crate::PackageError::MissingFromGraph {
                    name: package.name.clone(),
                }
            })?;
            // Canonicalize before storage (#323): `resolve` normalizes
            // lexically only, so a config assembled without the CLI's
            // up-front canonicalize would store alias spellings (`/var`
            // vs `/private/var`) and split attribution. dartq-source's
            // rule — the canonicalized longest existing ancestor — puts
            // every stored root in the one spelling universe the engine's
            // FileTable interns into and the attribution walk matches
            // against, whatever the caller's assembly path.
            let root = normalize_path(&package.root);
            let lib_dir = normalize_path(&package.lib_dir);
            // Duplicate normalized roots make file→package attribution
            // ambiguous (analyzer analog: `package_map_resolver.dart:33-52`
            // scans every root per file, where two equal roots race).
            if let Some(first) = root_owners.get(&root) {
                return Err(crate::PackageError::DuplicateRoot {
                    first: first.to_string(),
                    name: package.name.clone(),
                    root: root.display().to_string(),
                });
            }
            root_owners.insert(root.clone(), name.clone());
            let identity = PackageIdentity {
                lib_dir,
                name,
                root,
                language_version: package.language_version,
                category: SourceCategory::of(&instance.source_ref),
            };
            packages.insert(identity.name.clone(), identity);
        }

        let root_name = PackageName::new(&graph.root.0)?;
        // Carried by name even when the config does not cover it (config as
        // written): `get(root())` answers `None`, consumers' `if let Some`
        // degrade, and the membership finding rides the graph's advisory
        // channel — never a build error.
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

        // The attribution walk's two indexes, both derived from `packages`
        // here at build (never mutated afterwards — rebuild with the table).
        let by_root: FxHashMap<PathBuf, PackageName> = packages
            .values()
            .map(|identity| (identity.root.clone(), identity.name.clone()))
            .collect();
        // Deepest-first root index for the fallback scan: roots are sorted
        // by component depth once, so each query stops at the first prefix
        // hit instead of scoring every package.
        let mut by_depth: Vec<(&PackageName, usize)> = packages
            .iter()
            .map(|(name, identity)| (name, identity.root.components().count()))
            .collect();
        by_depth.sort_by_key(|&(_, depth)| std::cmp::Reverse(depth));
        let by_depth = by_depth.into_iter().map(|(name, _)| name.clone()).collect();

        // Canonicalized through the same dartq-source rule as the member
        // roots, so the walk's inclusive bound compares spelling-canonical
        // against the normalized query and roots no matter which spelling
        // the config_dir arrived in (a dot-segment or alias-spelled
        // `config_dir` would otherwise strand the bound mid-chain or fence
        // off the roots entirely).
        let workspace_root = normalize_path(config_dir.parent().unwrap_or(config_dir));
        // The bound stays the workspace root whether or not it is itself a
        // config root (pub always writes it so; a divergent hand-config
        // degrades — the bound's own tree walks unowned and falls through
        // to the fallback tier below).

        Ok(PackageTable {
            packages,
            by_root,
            by_depth,
            root: Some(root_name),
            workspace_members,
            workspace_root,
            ancestor_memo: RwLock::new(FxHashMap::default()),
        })
    }

    /// The degraded no-config table (#759) — the oracle's BasicWorkspace
    /// floor (`context_locator.dart:517-522`, `basic.dart:14-18`): a
    /// single-package context rooted at `root_dir` when a pubspec names a
    /// self package, else a bare table. The self package's language version
    /// is its pubspec's sdk lower bound — the value pub materializes into a
    /// written config's `languageVersion`, so the degraded face mirrors the
    /// configured one. The `package:` URI map is a separate table over the
    /// package config (empty here), so this identity never makes a package
    /// URI resolve — it exists for file→package attribution only, exactly
    /// the oracle's `BasicWorkspacePackage` over `Packages.empty`.
    ///
    /// # Errors
    ///
    /// [`crate::PackageError::InvalidName`] when the self package's name is
    /// not a valid pub package name.
    pub fn basic(
        root_dir: &Path,
        self_package: Option<(&str, Option<LanguageVersion>)>,
    ) -> Result<PackageTable, crate::PackageError> {
        let root_dir = normalize_path(root_dir);
        let mut packages = BTreeMap::new();
        let mut by_root = FxHashMap::default();
        // One entry: the deepest-first fallback order is trivially itself.
        let mut by_depth = Vec::new();
        let root = match self_package {
            Some((name, language_version)) => {
                let name = PackageName::new(name)?;
                let identity = PackageIdentity {
                    lib_dir: normalize_path(&root_dir.join("lib")),
                    name: name.clone(),
                    root: root_dir.clone(),
                    language_version,
                    category: SourceCategory::Path,
                };
                by_root.insert(root_dir.clone(), name.clone());
                by_depth.push(name.clone());
                packages.insert(name.clone(), identity);
                Some(name)
            }
            None => None,
        };
        Ok(PackageTable {
            packages,
            by_root,
            by_depth,
            root,
            workspace_members: None,
            workspace_root: root_dir,
            ancestor_memo: RwLock::new(FxHashMap::default()),
        })
    }

    /// The entrypoint package of the resolution, `None` for a context with
    /// no named root package.
    #[must_use]
    pub fn root(&self) -> Option<&PackageName> {
        self.root.as_ref()
    }

    /// The context root the table was built over: the workspace root for a
    /// configured resolution, the analyzed directory itself for the floors
    /// ([`Self::basic`]). The bare no-pubspec floor has no package identity
    /// to enumerate, so its discovery/options faces key off this directory
    /// — the oracle's `BasicWorkspace` roots its context at the analyzed
    /// directory and walks that whole folder (context_locator.dart
    /// :517-522).
    #[must_use]
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Looks a package up by name (raw, as it appears in `package:` URIs).
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&PackageIdentity> {
        self.packages.get(name)
    }

    /// The package that owns `path` — the engine's file→package attribution
    /// entry point, and the single source of truth for that mapping.
    ///
    /// `path` is normalized lexically on entry, once and with zero
    /// syscalls: the indexes are built from normalized roots, so the memo
    /// key and every walk comparison are spelling-canonical —
    /// `/proj/./lib/a.dart` attributes identically to `/proj/lib/a.dart`.
    /// Symlink-alias *resolution* deliberately stays upstream
    /// (dartq-source's FileTable canonicalizes on existence); this method
    /// is purely lexical.
    ///
    /// The oracle's two-tier face (`pub.dart:334-358` +
    /// `feature_set_provider.dart`'s `_findPackage`):
    ///
    /// 1. **The ancestor walk**, bounded by the workspace root: from `path`
    ///    upward, the first directory that is exactly a member root wins.
    ///    Pub writes every config member's `rootUri` as its pubspec
    ///    directory, so the nearest member-root ancestor is the nearest
    ///    pubspec ancestor that is a config member — the memoized walk of
    ///    `PubWorkspace.findPackageFor` without a filesystem probe, the same
    ///    purely lexical membership the version face consults
    ///    (`packages.dart` `packageForPath`, `rootFolder.contains`).
    ///    A nested member therefore owns its own tree, and a directory with
    ///    a pubspec but no config entry is skipped (in workspaces such a
    ///    stray is already a discovery error, dartq-pub's
    ///    `check_stray_pubspecs`).
    /// 2. **The fallback**, for content outside the bound (pub-cache
    ///    packages, path deps above the workspace): the config's innermost
    ///    containing root, scanned deepest-first over `by_depth` with
    ///    component-boundary prefix semantics (`Path::strip_prefix`).
    ///
    /// The memo (oracle `_workspacePackages`, `pub.dart:187`) is keyed by
    /// directories only: it records the owner of every directory the walk
    /// passed, so repeated queries over a shared tree are one hash hit per
    /// directory. A file query is never memoized — per-file entries would
    /// grow without bound over a full analyze — so a sibling file in an
    /// already-walked directory resolves from the memoized parent after one
    /// ancestor step.
    #[must_use]
    pub fn containing(&self, path: &Path) -> Option<&PackageIdentity> {
        let path = normalize(path);
        // Structural file/directory class. The supported query shape is
        // per-file `.dart` attribution: file paths carry an extension, so
        // such queries skip the memoization of the query itself;
        // extension-less queries (directories — including the workspace
        // root) are legitimate memo keys. Directory attribution is not a
        // supported query shape: the class is purely structural, so a
        // directory whose name carries an extension — including one that
        // is itself an in-workspace config root — walks like a file (its
        // own segment is skipped) and attributes to the enclosing package,
        // never to itself.
        let file_query = path.extension().is_some();
        // The read-locked fast path: the query itself, else (a file query)
        // its parent directory — exactly the first memo probes the walk
        // below makes, so a hit here answers what the walk would.
        {
            let memo = self
                .ancestor_memo
                .read()
                .unwrap_or_else(PoisonError::into_inner);
            let probe = if file_query {
                memo.get(path.as_path())
                    .or_else(|| path.parent().and_then(|parent| memo.get(parent)))
            } else {
                memo.get(path.as_path())
            };
            if let Some(hit) = probe {
                return hit.as_ref().map(|name| &self.packages[name]);
            }
        }
        let mut memo = self
            .ancestor_memo
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(hit) = memo.get(path.as_path()) {
            return hit.as_ref().map(|name| &self.packages[name]);
        }
        let mut trail: Vec<Box<Path>> = Vec::new();
        let mut owner: Option<PackageName> = None;
        // Resumable: the fallback backfill continues at the exact point
        // the walk stopped, so directories already in `trail` are never
        // re-pushed. A file query skips its own step first — the file is
        // never a root and never memoized.
        let mut walk = path.ancestors();
        if file_query {
            walk.next();
        }
        for dir in walk.by_ref() {
            if let Some(hit) = memo.get(dir) {
                owner.clone_from(hit);
                break;
            }
            // The bound is inclusive: the workspace root itself is checked
            // (it is a config root in every pub-written layout), its parent
            // is not.
            if !(dir == self.workspace_root || dir.starts_with(&self.workspace_root)) {
                break;
            }
            trail.push(dir.into());
            if let Some(name) = self.by_root.get(dir) {
                owner = Some(name.clone());
                break;
            }
        }
        if owner.is_none() {
            match self.fallback_owner(&path) {
                Some(name) => {
                    // Backfill only the directories inside the matched
                    // root, resuming where the walk stopped: prefix
                    // membership holds downward from a root, but an
                    // ancestor above it (e.g. the pub cache root over one
                    // package's checkout) is owned by nobody or by a
                    // shallower root, never by this package.
                    let root = &self.packages[&name].root;
                    for dir in walk {
                        if !dir.starts_with(root) {
                            break; // strictly above the root, and so are the rest
                        }
                        trail.push(dir.into());
                    }
                    owner = Some(name);
                }
                None => {
                    // No root contains the path, so none contains any
                    // ancestor either (a root prefixing an ancestor would
                    // prefix the path).
                    trail.extend(walk.map(Box::from));
                }
            }
        }
        for dir in &trail {
            memo.insert(dir.clone(), owner.clone());
        }
        owner.as_ref().map(|name| &self.packages[name])
    }

    /// The fallback tier: the innermost config root containing `path`
    /// (deepest-first prefix scan), or `None` above every root.
    fn fallback_owner(&self, path: &Path) -> Option<PackageName> {
        self.by_depth
            .iter()
            .map(|name| &self.packages[name])
            .find(|package| path.strip_prefix(&package.root).is_ok())
            .map(|package| package.name.clone())
    }

    /// Memo keys, sorted — test-only view asserting the per-directory
    /// memo discipline (files never enter, directories do).
    #[cfg(test)]
    fn memo_keys(&self) -> Vec<PathBuf> {
        let mut keys: Vec<PathBuf> = self
            .ancestor_memo
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .map(|dir| dir.to_path_buf())
            .collect();
        keys.sort();
        keys
    }

    /// All packages, ordered by name.
    pub fn iter(&self) -> impl Iterator<Item = &PackageIdentity> {
        self.packages.values()
    }

    /// Number of packages — the config's entry count, never mutated after
    /// build. There is deliberately no `is_empty` query: emptiness is a
    /// config-shape fact the caller already knows, not a table state worth
    /// an API.
    #[expect(clippy::len_without_is_empty)] // emptiness is a config-shape fact the caller knows, not table state worth an API
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

#[cfg(test)]
mod tests {
    use super::*;
    use dartq_pub::{LockFile, Pubspec, StaleReason, build_graph};
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
        assert_eq!(table.root().map(PackageName::as_str), Some("app"));
        assert_eq!(table.workspace_members().map(BTreeSet::len), Some(1));
        assert!(table.is_workspace_member("local_dep"));
        assert!(!table.is_workspace_member("hosted_dep"));
        assert!(!table.is_workspace_member("app"));

        let root = table.get("app").expect("root entry");
        // `../` against .dart_tool resolves and normalizes to the project.
        assert_eq!(root.root, PathBuf::from("/proj"));
        // `packageUri` absent in CONFIG: the source root anchors at
        // `rootUri` itself — the reader contract (package_config_json.dart
        // :152), never a `lib/` default.
        assert_eq!(root.lib_dir, PathBuf::from("/proj"));
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

    /// Config as written: a graph built against a config that covers
    /// neither the root nor the declared member still builds its table —
    /// the root is carried by name with no entry, the member is filtered
    /// at the graph level, and the config's own entries attribute.
    #[test]
    fn root_or_member_missing_from_config_degrades() {
        let bare = PackageConfig::parse(
            r#"{"configVersion": 2, "packages": [{"name": "hosted_dep", "rootUri": "../hosted"}]}"#,
        )
        .expect("valid config");
        let lock = LockFile::parse(LOCK).expect("valid lock");
        let graph = build_graph(
            &bare,
            &lock,
            &root_pubspec(),
            &BTreeMap::new(),
            Some(&["local_dep".to_owned()]),
            Path::new("/proj/.dart_tool"),
        )
        .expect("graph");
        // Both membership findings ride the graph for the advisory.
        assert_eq!(
            graph.membership_findings,
            vec![
                StaleReason::RootMissingFromConfig { name: "app".into() },
                StaleReason::MemberMissingFromConfig {
                    name: "local_dep".into()
                },
            ]
        );

        let table =
            PackageTable::build(&bare, &graph, Path::new("/proj/.dart_tool")).expect("table");
        // Root carried by name, with no entry behind it.
        assert_eq!(table.root().map(PackageName::as_str), Some("app"));
        assert!(table.get("app").is_none());
        // The filtered member set mirrors the graph.
        assert_eq!(table.workspace_members().map(BTreeSet::len), Some(0));
        // The config's own entry attributes as usual.
        assert_eq!(
            table
                .containing(Path::new("/proj/hosted/lib/h.dart"))
                .map(|identity| identity.name.as_str()),
            Some("hosted_dep")
        );
        // The bound's own tree is owned by nobody.
        assert_eq!(table.containing(Path::new("/proj/lib/a.dart")), None);
    }

    #[test]
    fn invalid_names_are_rejected_and_duplicates_fold() {
        // An invalid name is the shared dartq-pub face's hard error: a name
        // outside even the reader's loose charset — pub's constructor
        // sanity check (`package_config.dart:44-51`). A duplicate name
        // folds to its first entry instead (package_config-3.0.0
        // `package_config.dart:624-633`): the oracle never surfaces a
        // repeated name as a diagnostic, and pub's writer cannot produce
        // one.
        let invalid = PackageConfig::parse(
            r#"{"configVersion": 2, "packages": [{"name": "foo bar", "rootUri": "../"}]}"#,
        )
        .expect("parses fine at the config layer");
        let graph = graph_for(&PackageConfig::parse(CONFIG).expect("valid config"));
        let error = PackageTable::build(&invalid, &graph, Path::new("/proj/.dart_tool"))
            .expect_err("invalid name");
        assert!(
            error.to_string().contains("invalid package name `foo bar`"),
            "unexpected error: {error}"
        );

        let duplicated = PackageConfig::parse(
            r#"{"configVersion": 2, "packages": [
                {"name": "app", "rootUri": "../"},
                {"name": "app", "rootUri": "../"}]}"#,
        )
        .expect("parses with the fold");
        assert_eq!(
            duplicated.packages.len(),
            1,
            "the later duplicate is skipped"
        );
        let table =
            PackageTable::build(&duplicated, &graph_for(&duplicated), Path::new("/proj/.dart_tool"))
                .expect("the folded config builds");
        assert_eq!(
            table.get("app").expect("app entry").root,
            PathBuf::from("/proj")
        );
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
        // mis-resolving (`package_config-3.0.0` `package_config.dart:838`),
        // by the shared dartq-pub face.
        let config = PackageConfig::parse(
            r#"{
              "configVersion": 2,
              "packages": [{"name": "app", "rootUri": "../", "packageUri": "../lib"}],
              "generator": "pub"
            }"#,
        )
        .expect("valid config");
        let graph = plain_graph(&config);
        let error = PackageTable::build(&config, &graph, Path::new("/proj/.dart_tool"))
            .expect_err("escaping packageUri");
        let text = error.to_string();
        assert!(text.contains("not below the package root"), "{text}");
        assert!(text.contains("`/lib`"), "{text}");
        assert!(text.contains("`/proj`"), "{text}");
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
        // in-charset. The root entry keeps the config graph-buildable —
        // the rejection must surface from build's resolve face, so the
        // fixture is otherwise a valid resolution.
        let all_dots = PackageConfig::parse(
            r#"{
              "configVersion": 2,
              "packages": [
                {"name": "app", "rootUri": "../"},
                {"name": "..", "rootUri": "../dots"}
              ],
              "generator": "pub"
            }"#,
        )
        .expect("valid config");
        let graph = plain_graph(&all_dots);
        let error = PackageTable::build(&all_dots, &graph, Path::new("/proj/.dart_tool"))
            .expect_err("all-dot name");
        assert!(
            error.to_string().contains("invalid package name `..`"),
            "unexpected error: {error}"
        );
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
        // The absolute-`config_dir` contract is a hard error (the shared
        // resolve face): build is one-shot and a relative dir would
        // silently produce wrong roots.
        let config = PackageConfig::parse(CONFIG).expect("valid config");
        let graph = graph_for(&config);
        let error =
            PackageTable::build(&config, &graph, Path::new(".dart_tool")).expect_err("relative");
        assert!(
            error
                .to_string()
                .contains("config_dir must be absolute, got `.dart_tool`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn normalizes_relative_and_dot_paths() {
        // Pins the shared `dartq_pub::normalize` contract this crate now
        // reuses — the equivalence evidence for the deduplicated normalizer.
        assert_eq!(
            dartq_pub::normalize(Path::new("/proj/.dart_tool/..")),
            PathBuf::from("/proj")
        );
        assert_eq!(
            dartq_pub::normalize(Path::new("/a/./b//c")),
            PathBuf::from("/a/b/c")
        );
        assert_eq!(
            dartq_pub::normalize(Path::new("a/../b")),
            PathBuf::from("b")
        );
        assert_eq!(
            dartq_pub::normalize(Path::new("../up")),
            PathBuf::from("../up")
        );
        assert_eq!(
            dartq_pub::normalize(Path::new("../../..")),
            PathBuf::from("../../..")
        );
        // `..` past an absolute root clamps at the root —
        // the oracle's drop semantics the normalizer converged on.
        assert_eq!(
            dartq_pub::normalize(Path::new("/a/../..")),
            PathBuf::from("/")
        );
    }

    /// The oracle attribution walk: an `example/` sub-package that is a
    /// config member carries its own language version on every file under
    /// it, while plain members inherit the enclosing package's version.
    #[test]
    fn attribution_walk_member_subpackage_gets_own_version() {
        let table = plain_table(
            r#"{
              "configVersion": 2,
              "packages": [
                {"name": "app", "rootUri": "../", "languageVersion": "3.5"},
                {"name": "example", "rootUri": "../example", "languageVersion": "3.9"}
              ],
              "generator": "pub"
            }"#,
        );
        let version_at = |path: &str| {
            table
                .containing(Path::new(path))
                .map(|identity| identity.language_version.map(|v| v.to_string()))
                .expect("attributed")
        };
        // Hand-trace (walk over `/proj/example/lib/a.dart`): ancestor
        // `/proj/example/lib/a.dart` — not a member root; `/proj/example/lib`
        // — not a member root; `/proj/example` — exact member root →
        // `example`, own 3.9. Every dir on the chain is memoized.
        assert_eq!(
            version_at("/proj/example/lib/a.dart"),
            Some("3.9".to_owned())
        );
        // Hand-trace (`/proj/lib/m.dart`): `/proj/lib` — miss; `/proj` —
        // member root → `app`, 3.5.
        assert_eq!(version_at("/proj/lib/m.dart"), Some("3.5".to_owned()));
        // The root directory itself belongs to its package.
        assert_eq!(version_at("/proj"), Some("3.5".to_owned()));
        // A memo-hit chain: a later query under an already-attributed dir
        // resolves identically (`/proj/example` memoized from the first
        // query).
        assert_eq!(
            version_at("/proj/example/tool/x.dart"),
            Some("3.9".to_owned())
        );
    }

    /// Content outside the workspace bound — pub-cache packages — falls
    /// back to the config's innermost containing root (the oracle's
    /// `packageForPath` face), and ancestors above the matched root are
    /// never mis-attributed by the memo.
    #[test]
    fn attribution_outside_workspace_falls_back_to_config_root() {
        let table = table_for(&PackageConfig::parse(CONFIG).expect("valid config"));
        let owner_at = |path: &str| {
            table
                .containing(Path::new(path))
                .map(|identity| identity.name.to_string())
        };
        // Hand-trace (`/cache/hosted/pub.dev/hosted_dep-1.4.0/lib/h.dart`):
        // first ancestor is outside the bound `/proj` → walk yields nothing
        // → fallback scan finds the deepest prefixing root, the hosted
        // package. Its version rides along.
        let hosted = table.containing(Path::new(
            "/cache/hosted/pub.dev/hosted_dep-1.4.0/lib/h.dart",
        ));
        assert_eq!(
            hosted.map(|identity| identity.name.to_string()),
            Some("hosted_dep".to_owned())
        );
        assert_eq!(
            hosted
                .expect("hosted")
                .language_version
                .map(|v| v.to_string()),
            Some("3.2".to_owned())
        );
        // The package root directory itself: attributed.
        assert_eq!(
            owner_at("/cache/hosted/pub.dev/hosted_dep-1.4.0"),
            Some("hosted_dep".to_owned())
        );
        // Ancestors above the matched root must not inherit its owner (the
        // memo backfill stops at the root): the shared cache dirs are owned
        // by nobody.
        assert_eq!(owner_at("/cache/hosted"), None);
        assert_eq!(owner_at("/cache"), None);
        // And a sibling package under the same cache tree still resolves to
        // itself, proving no wrong `None` was cached above either.
        assert_eq!(
            owner_at("/cache/hosted/pub.dev/other-1.0.0/lib/x.dart"),
            None,
            "not a config member"
        );
    }

    /// A directory with a pubspec but no config entry is not a member: the
    /// walk skips it and attributes to the enclosing package. Outside pub
    /// workspaces such strays are legal layouts (pub only rejects them
    /// between workspace members, dartq-pub's `check_stray_pubspecs`),
    /// and the oracle's version face lands on the same owner — its
    /// `PubPackage` there carries no language version
    /// (`workspace.dart:94`), so the config lookup decides.
    #[test]
    fn non_member_pubspec_dir_is_skipped_by_the_walk() {
        let table = plain_table(
            r#"{
              "configVersion": 2,
              "packages": [
                {"name": "app", "rootUri": "../", "languageVersion": "3.5"},
                {"name": "example", "rootUri": "../example", "languageVersion": "3.9"}
              ],
              "generator": "pub"
            }"#,
        );
        // `example` is a member (3.9); `example/stray` has a pubspec on
        // disk in the real layout but no config entry — its files inherit
        // the nearest member ancestor's version. (No fixture on disk: the
        // walk is purely lexical over config membership.)
        let version = table
            .containing(Path::new("/proj/example/stray/lib/s.dart"))
            .map(|identity| identity.language_version.map(|v| v.to_string()))
            .expect("attributed");
        assert_eq!(version, Some("3.9".to_owned()));
    }

    /// Repeated queries — the memo-hit path — return identical owners to
    /// the direct walk, including for the `None` face.
    #[test]
    fn memoized_queries_match_the_walk() {
        let table = plain_table(
            r#"{
              "configVersion": 2,
              "packages": [
                {"name": "app", "rootUri": "../", "languageVersion": "3.5"},
                {"name": "lvl1", "rootUri": "../lvl1", "languageVersion": "3.6"},
                {"name": "inner", "rootUri": "../lvl1/lvl2/inner", "languageVersion": "3.7"}
              ],
              "generator": "pub"
            }"#,
        );
        let owner = |path: &str| {
            table
                .containing(Path::new(path))
                .map(|identity| identity.name.to_string())
        };
        // First pass: cold walk.
        let cold: Vec<Option<String>> = [
            "/proj/lvl1/lvl2/inner/lib/a.dart",
            "/proj/lvl1/lib/b.dart",
            "/proj/lib/c.dart",
            "/elsewhere/lib/f.dart",
        ]
        .iter()
        .map(|path| owner(path))
        .collect();
        assert_eq!(
            cold,
            [
                Some("inner".to_owned()),
                Some("lvl1".to_owned()),
                Some("app".to_owned()),
                None,
            ]
        );
        // Second pass: every query now rides the memo (or re-walks over
        // memoized ancestors) and must land identically.
        let warm: Vec<Option<String>> = [
            "/proj/lvl1/lvl2/inner/lib/a.dart",
            "/proj/lvl1/lvl2/inner/lib/sub/deep.dart",
            "/proj/lvl1/lib/b.dart",
            "/proj/lvl1/lib/sub/b.dart",
            "/proj/lib/c.dart",
            "/proj/tool/d.dart",
            "/elsewhere/lib/f.dart",
        ]
        .iter()
        .map(|path| owner(path))
        .collect();
        assert_eq!(
            warm,
            [
                Some("inner".to_owned()),
                Some("inner".to_owned()),
                Some("lvl1".to_owned()),
                Some("lvl1".to_owned()),
                Some("app".to_owned()),
                Some("app".to_owned()),
                None,
            ]
        );
        // Component boundary: `lvl10` shares a string prefix with `lvl1`
        // but is not inside it.
        assert_eq!(owner("/proj/lvl10/lib/e.dart"), Some("app".to_owned()));
    }

    /// Spelling-canonical attribution: non-normal spellings of a path
    /// (dot segments, `..` round-trips) attribute identically to their
    /// canonical form — both inside the bound and in the fallback tier —
    /// and a path outside every root stays `None` in any spelling. This
    /// is the entry-side half of the normalize contract (the indexes are
    /// built from normalized roots).
    #[test]
    fn containing_is_spelling_canonical() {
        let table = table_for(&PackageConfig::parse(CONFIG).expect("valid config"));
        let owner = |path: &str| {
            table
                .containing(Path::new(path))
                .map(|identity| identity.name.to_string())
        };
        assert_eq!(owner("/proj/lib/a.dart"), Some("app".to_owned()));
        assert_eq!(owner("/proj/./lib/a.dart"), Some("app".to_owned()));
        assert_eq!(owner("/proj/lib/../lib/a.dart"), Some("app".to_owned()));
        assert_eq!(
            owner("/proj/.dart_tool/../lib/a.dart"),
            Some("app".to_owned())
        );
        // The fallback tier aligns too.
        assert_eq!(
            owner("/cache/hosted/pub.dev/hosted_dep-1.4.0/./lib/h.dart"),
            Some("hosted_dep".to_owned())
        );
        assert_eq!(owner("/elsewhere/lib/f.dart"), None);
        assert_eq!(owner("/elsewhere/./lib/f.dart"), None);
    }

    /// Build-side spelling canon (#323): `build` canonicalizes member
    /// roots, lib dirs, and the workspace bound through dartq-source's
    /// normalize rule, so an assembly path that skips the CLI's up-front
    /// canonicalization cannot reintroduce the C5 split (a config resolved
    /// through an alias spelling would store alias roots while the engine's
    /// FileTable attributes through canonical spellings — the canonical
    /// query would find no owner at all). Mirrors the path_universe
    /// fixture shape: a real temp workspace plus an in-tree alias symlink,
    /// the table built from the alias-spelled config dir.
    #[cfg(unix)]
    #[test]
    fn build_canonicalizes_alias_spelled_roots() {
        let root = std::env::temp_dir().join(format!("dartq-package-323-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".dart_tool")).expect("config dir");
        std::os::unix::fs::symlink(&root, root.join("link")).expect("alias symlink");

        let config_dir = root.join("link").join(".dart_tool");
        let config = PackageConfig::parse(
            r#"{
              "configVersion": 2,
              "packages": [{"name": "app", "rootUri": "../"}],
              "generator": "pub"
            }"#,
        )
        .expect("valid config");
        let lock = LockFile::parse(LOCK).expect("valid lock");
        let graph = build_graph(
            &config,
            &lock,
            &root_pubspec(),
            &BTreeMap::new(),
            None,
            &config_dir,
        )
        .expect("graph");
        let table = PackageTable::build(&config, &graph, &config_dir).expect("table");

        // Stored spelling: the alias component (and any symlinked temp-dir
        // prefix, macOS `/var` → `/private/var`) is resolved — the same
        // canonical universe `fs::canonicalize` reports, regardless of the
        // spelling assembly came in through.
        let canonical = root.canonicalize().expect("canonical root");
        let app = table.get("app").expect("app entry");
        assert_eq!(app.root, canonical, "stored root must be canonical");
        // `packageUri` absent: `lib_dir` anchors at `root` itself.
        assert_eq!(app.lib_dir, canonical);

        // Attribution in both spellings: the canonical one (what the
        // resolver and FileTable produce — pre-#323 an alias-assembled
        // table left this query ownerless on symlinked temp dirs) and the
        // in-tree alias one (the purely lexical walk climbs to the
        // canonical root inside the bound).
        let owner = |path: &Path| {
            table
                .containing(path)
                .map(|identity| identity.name.as_str().to_owned())
        };
        assert_eq!(owner(&canonical.join("lib/a.dart")), Some("app".to_owned()));
        assert_eq!(
            owner(&canonical.join("link/lib/a.dart")),
            Some("app".to_owned())
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The memo is keyed by directories only: querying a file memoizes
    /// its parent directory and the walked ancestors, never the file
    /// itself — so two files in one directory cost no per-file entries
    /// and the memo cannot grow with the file count.
    #[test]
    fn memo_is_keyed_by_directories_not_files() {
        let table = plain_table(
            r#"{
              "configVersion": 2,
              "packages": [{"name": "app", "rootUri": "../"}],
              "generator": "pub"
            }"#,
        );
        let owner = |path: &str| {
            table
                .containing(Path::new(path))
                .map(|identity| identity.name.to_string())
        };
        assert_eq!(owner("/proj/lib/a.dart"), Some("app".to_owned()));
        let keys = table.memo_keys();
        assert!(
            !keys.contains(&PathBuf::from("/proj/lib/a.dart")),
            "the file itself must not be memoized: {keys:?}"
        );
        assert!(
            keys.contains(&PathBuf::from("/proj/lib")),
            "the parent directory must be memoized: {keys:?}"
        );
        assert!(
            keys.contains(&PathBuf::from("/proj")),
            "the workspace root must be memoized: {keys:?}"
        );

        // A sibling file rides the memoized parent: same owner, zero new
        // memo entries.
        assert_eq!(owner("/proj/lib/b.dart"), Some("app".to_owned()));
        assert_eq!(table.memo_keys(), keys);
    }

    /// The walk's inclusive bound is the workspace root (the config dir's
    /// parent) whether or not it is itself a config root. pub always writes
    /// the root package's `rootUri` so; a divergent hand-config degrades:
    /// member trees attribute through the walk, the bound's own tree falls
    /// through to the fallback tier (the outside-workspace tier) and lands
    /// `None` when no config root covers it — never a mis-attribution, and
    /// never a build error (config as written).
    #[test]
    fn workspace_root_not_among_config_roots_degrades_to_the_fallback_tier() {
        // The only entry roots at /proj/app; nothing owns /proj, the
        // walk's inclusive bound.
        let config = PackageConfig::parse(
            r#"{"configVersion": 2, "packages": [{"name": "app", "rootUri": "../app"}]}"#,
        )
        .expect("valid config");
        let table = PackageTable::build(
            &config,
            &plain_graph(&config),
            Path::new("/proj/.dart_tool"),
        )
        .expect("table");
        let owner = |path: &str| {
            table
                .containing(Path::new(path))
                .map(|identity| identity.name.as_str().to_owned())
        };
        // Inside the one config root: the walk owns it.
        assert_eq!(owner("/proj/app/lib/a.dart"), Some("app".to_owned()));
        assert_eq!(owner("/proj/app"), Some("app".to_owned()));
        // The bound's own tree: walked unowned, fallback finds no root → None.
        assert_eq!(owner("/proj/lib/a.dart"), None);
        assert_eq!(owner("/proj"), None);
        // The memo records the `None`s without poisoning the member tree.
        assert_eq!(owner("/proj/app/lib/b.dart"), Some("app".to_owned()));
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
