//! Directive URI parsing, classification, and resolution to interned file
//! targets.
//!
//! The pipeline mirrors the analyzer's: parse the directive URI string with
//! Dart `Uri.parse` validity rules, resolve relative strings against the
//! containing file's canonical URI (`file_state.dart:766-796`), then dispatch
//! the absolute URI by scheme — `dart:` through the SDK's flat
//! `libraries.dart` existence table, `package:` through the `package_config`
//! (package name → lib dir),
//! `file:` to a path, everything else unresolvable (`file_state.dart:1458-
//! 1497`). Resolution is infallible: every failure mode is a classification
//! ([`ResolvedUri::Unresolved`]) plus the diagnostic code the oracle
//! would emit for it.
//!
//! One oracle subtlety is load-bearing here: the containing file's canonical
//! URI is `package:<pkg>/<rel>` for files under a package's lib dir and
//! `file:` otherwise (`PackageMapUriResolver.pathToUri`), so relative
//! composition from a package file happens in package-URI space —
//! `../x.dart` from `package:p/a/b.dart` selects `package:p/x.dart`, i.e.
//! `lib/x.dart`, not `root/x.dart`.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use dartq_pub::{PackageConfig, PubError, ResolvedPackage, normalize};
use dartq_sdk::SdkSnapshot;
use dartq_source::{FileId, FileTable};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::codes::ResolutionCode;
use crate::conditional::{Configuration, SelectedUri, resolve_conditional};

/// How a directive URI resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolvedUri {
    /// A `dart:` URI mapped through the SDK library table.
    Dart {
        /// Library name (`core` for `dart:core`, `core` again for
        /// `dart:core/bool.dart`).
        library: Box<str>,
        /// The SDK source file path (need not exist on disk).
        file: PathBuf,
    },
    /// A `package:` URI mapped through the package config.
    Package {
        /// Package name.
        package: Box<str>,
        /// The lib-relative path, decoded.
        rel: PathBuf,
        /// The resolved filesystem path (lib dir + rel).
        file: PathBuf,
    },
    /// A `file:` or relative URI mapped to a filesystem path.
    File(PathBuf),
    /// Unresolvable; see [`UnresolvedKind`].
    Unresolved(UnresolvedKind),
}

/// Why a directive URI has no target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnresolvedKind {
    /// The URI string does not parse (`Uri.parse` `FormatException`), or the
    /// directive URI is a string interpolation and carries no string at all;
    /// reported as `invalid_uri` / `uri_with_interpolation`.
    BadUri,
    /// Parses but maps to no file: unknown package, unknown `dart:` library,
    /// unrecognized scheme (`data:`, `http:`, …); reported as
    /// `uri_does_not_exist`.
    NotExist,
    /// The target is a generated-file path never produced on disk; reported
    /// as `uri_has_not_been_generated` (the URI layer carries this in
    /// [`UriResolution::code`] while keeping the [`ResolvedUri::Package`]/[
    /// `ResolvedUri::File`] shape, so this variant is for upper layers).
    /// Upper-layer-only contract: never constructed in this crate — the
    /// code face rides [`UriResolution::code`] instead.
    Generated,
    /// The target file exists but is a part file; reported as
    /// `import_of_non_library` / `export_of_non_library`. The is-part
    /// decision is cross-file — the target's own classification — so it
    /// belongs to the consumer over [`crate::FileRole`] (the workspace
    /// target's role, or [`crate::SdkFileKinds`] for a `dart:` target),
    /// never to this layer's per-directive resolution; the variant stays as
    /// the upper-layer marker only.
    NotLibrary,
    /// The selected URI is `dart-ext:`; reported as
    /// `use_of_native_extension` (additive to the spec draft, which has no
    /// carrier for the row).
    DartExt,
}

/// The part-of role of one `dart:` SDK target file — the external half of
/// the file-kind classification (`FileState._updateKind`,
/// file_state.dart:928-949, decided there by parsing the file; SDK files
/// have no [`FileId`] in dartq, so the classification is probed from
/// content instead). The oracle-observable matrix over this face, pinned
/// against `dart analyze` 3.13.7:
///
/// * no `part of` (entry files like `dart:math`, patch libraries like the
///   `_internal` patch files) — a library: `part 'dart:math'` reports
///   `part_of_non_part` with the `dart:` spelling; `import` stays silent;
/// * `part of <dotted.name>;` (e.g. `dart:developer/creation_tracking.dart`,
///   `part of dart.developer;`) — binds by library-name equality: a
///   including library named the same attaches silently, a differently
///   named one reports `part_of_different_library` with the two names, an
///   unnamed one reports `part_of_unnamed_library`;
/// * `part of '<uri>';` (e.g. `dart:async/future.dart`, `part of "dart:async"`)
///   — binds by URI-file identity, which no workspace container can match:
///   the include reports `part_of_different_library` with the two URIs, and
///   an `import` reports `import_of_non_library`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SdkFileKind {
    /// The directive prelude carries no `part of`.
    Library,
    /// `part of a.b.c;` — the dotted name as written.
    NamePartOf(Box<str>),
    /// `part of '<uri>';` — any string-literal form (quotes, raw, adjacent
    /// concatenation all reduce to this marker: the binding target is an
    /// SDK file, never a workspace container).
    UriPartOf,
}

/// A session-stable cache of [`SdkFileKind`] probes keyed by the SDK file
/// path. The probe reads the file and scans its directive prelude, so
/// emitters asking after an external `dart:` part/import target (`fileKinds`
/// walks, `_resolvePartDirective`-style include checks) pay the read once
/// per SDK file per session — SDK content does not rewrite under a running
/// analysis. Not persisted: a probe is a pure function of SDK content and
/// the `(library, part)` spelling.
#[derive(Debug, Default)]
pub struct SdkFileKinds {
    cache: FxHashMap<PathBuf, SdkFileKind>,
}

impl SdkFileKinds {
    /// Probes (or re-reads from cache) the kind of `dart:<library>[/<part>]`.
    /// A spelling the SDK table does not carry, or a file that cannot be
    /// read, classifies as [`SdkFileKind::Library`] — the inert default for
    /// a target the URI layer already coded missing, which no part-family
    /// row reaches.
    pub fn probe(&mut self, sdk: &SdkSnapshot, library: &str, part: &str) -> SdkFileKind {
        let Some(path) = sdk.flat_library_to_path(library, part) else {
            return SdkFileKind::Library;
        };
        if let Some(kind) = self.cache.get(&path) {
            return kind.clone();
        }
        let kind = std::fs::read(&path)
            .map(|bytes| scan_sdk_part_of(&bytes))
            .unwrap_or(SdkFileKind::Library);
        self.cache.insert(path, kind.clone());
        kind
    }
}

/// The canonical `dart:` spelling of a resolved `(library, part)` pair —
/// `dart:async` for an entry, `dart:async/future.dart` for a sub-file. This
/// is the `includedFile.uriStr` the oracle carries into the part-family and
/// non-library rows of external targets (`part_of_non_part`,
/// `part_of_different_library`, `import_of_non_library`/`export_of_non_
/// library`), equal to the written spelling whenever the directive wrote a
/// canonical form (the corpus shape).
#[must_use]
pub fn canonical_dart_uri(library: &str, part: &str) -> Box<str> {
    if part.is_empty() {
        format!("dart:{library}").into_boxed_str()
    } else {
        format!("dart:{library}/{part}").into_boxed_str()
    }
}

/// Engine-facing projection of a [`ResolvedUri`]:
/// `Dart` → [`ResolvedTarget::External`] (SDK content, outside the analysis
/// set), `Package`/`File` → [`ResolvedTarget::Resolved`] (a file target, even
/// when the file is missing — the missing-ness rides on
/// [`UriResolution::code`]), `Unresolved` → [`ResolvedTarget::Missing`] (no
/// reverse edge, no content identity).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolvedTarget {
    /// The interned file target of the directive.
    Resolved(FileId),
    /// An external (SDK) library.
    External,
    /// No target: no reverse edge is built.
    Missing,
}

/// The outcome of resolving one directive URI string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UriResolution {
    /// The classification of the URI.
    pub uri: ResolvedUri,
    /// The engine-facing projection.
    pub target: ResolvedTarget,
    /// The diagnostic code the oracle emits for this outcome, or `None`
    /// when the target is fine.
    pub code: Option<ResolutionCode>,
}

/// Which directive face a URI resolves for (`library_analyzer.dart:699-719`
/// `state.isDocImport`). The target lookup is kind-independent; only the
/// missing-target diagnostic differs — an `@docImport` reports the
/// `uri_does_not_exist_in_doc_import` static warning where an import/export
/// reports `uri_does_not_exist` or `uri_has_not_been_generated`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectiveKind {
    /// An `import` or `export` directive.
    ImportExport,
    /// A `part of` directive. Its URI is resolved only to *bind* the part
    /// against a library (`file_state.dart:938-953` `PartOfUriKnownFileKind`
    /// /`PartOfUriUnknownFileKind`): a missing or unparseable target simply
    /// leaves the part unattributed, and the oracle reports **no** URI
    /// diagnostic for it — not `uri_does_not_exist`, not `invalid_uri`, not
    /// `uri_with_interpolation`. The target still resolves (the part-of
    /// graph needs it); [`UriResolution::code`] is always `None`.
    PartOf,
    /// A documentation `@docImport` directive.
    DocImport,
}

/// The `package:` resolution face of a parsed package config: package name →
/// lib dir.
///
/// Built once per analysis context; lookups are [`FxHashMap`] hits on the
/// per-directive hot path. Also owns the generated-file face the analyzer
/// layers onto `package:` targets (`.dart_tool/build/generated`).
///
/// The lookup face is per-context (the pub universe separation): every
/// nested self-resolved context root — a config entry whose package root,
/// STRICTLY INSIDE the context root, carries its own `pubspec.yaml`
/// together with its own `.dart_tool/package_config.json` — resolves
/// against the package set that root's own config materializes, and every
/// other file (dependency files outside the analyzed tree included)
/// resolves against the root config AS WRITTEN (re-read from disk: the
/// assembly's nested-root extension joins entries pub never wrote into the
/// root config, and those stay invisible outside their own context). The
/// oracle splits the analyzed tree into exactly these context roots
/// (upstream Dart SDK source,
/// pkg/analyzer/lib/src/context/context_locator.dart `_createContextRoots`:
/// a locally different packages file splits the root), so a cross-package
/// name visible only in a SIBLING's universe stays undefined here — the
/// flat union would resolve it.
///
/// Context-root discovery rides two feeds: the merged config's own entries
/// AND the assembly's discovered-roots list ([`Self::new_with_context_roots`])
/// — the merge's one-identity-per-name dedup can drop a same-named sibling
/// root's whole entry set, yet the oracle still gives that directory its own
/// context. Canonical-URI spelling mirrors the split: a file under a nested
/// root spells through that context's own package map (a same-named sibling
/// would otherwise collapse to `file://` or, worse, a `package:` name the
/// containing context cannot even resolve), and a file outside every
/// context spells through the flat `lib_dirs` scan exactly as before.
///
/// Root/lib-dir resolution is [`PackageConfig::resolve`], dartq-pub's
/// single shared face — duplicate names floor the whole config at parse,
/// names follow the loose reader grammar, `packageUri` directories are
/// normalized and subtree-validated — exactly the policies the identity
/// table applies, so the two tables can never diverge.
#[derive(Debug)]
pub struct PackageTable {
    /// Package lib dirs with their names, sorted by component depth (deepest
    /// first) — the single owned copy in the table. It serves both faces:
    /// [`package_of`]'s scan index (the dartq-package `containing` shape:
    /// depths are computed once, a nested root always has strictly more
    /// components than any root it is contained by, and two distinct roots
    /// of equal depth can never both be a component prefix of one path, so
    /// the first hit is the deepest) and, positioned by `indices`,
    /// [`Self::lib_dir`].
    lib_dirs: Vec<(PathBuf, Box<str>)>,
    /// Package name → position in `lib_dirs` — the per-directive hot path's
    /// O(1) `lib_dir` face without a second copy of any lib dir. Names are
    /// unique (dartq-pub floors a whole config at parse on the first
    /// repeated name), so one name owns one position.
    indices: FxHashMap<Box<str>, usize>,
    /// Directory holding `.dart_tool/` — the base of the generated-file
    /// fallback.
    workspace_root: PathBuf,
    /// The context-root universe: the root config as written — the packages
    /// file the oracle parses for the root context, re-read from disk so the
    /// assembly's nested-root extension cannot widen it. Built only when
    /// nested contexts exist; `None` (the common shape) leaves every lookup
    /// on the flat `indices`, the exact pre-separation behavior behind one
    /// `None` test.
    root_universe: Option<ContextUniverse>,
    /// Nested self-resolved context roots, deepest first: the canonical
    /// package root and the universe its own `package_config.json`
    /// materializes. Two discovery feeds union here: the config entries that
    /// survived the assembly's merge (one file read per entry root, no
    /// directory walk) and the assembly's own sibling-root list — which
    /// also carries the roots whose entries the merge's one-identity-per-
    /// name dedup dropped (chunked conformance floors run many same-named
    /// sibling roots; the dropped roots are contexts all the same).
    contexts: Vec<(PathBuf, ContextUniverse)>,
}

/// A lib dir in the spelling universe the identity layer keys files under:
/// `FileTable` interns existing files by `std::fs::canonicalize`
/// (`file_table.rs` `normalize_path`, rule 1) while package-config
/// resolution is lexical (`resolve_root` joins onto `config_dir`), so a
/// workspace under a symlinked ancestor (macOS `/var` → `/private/var`)
/// would otherwise never prefix-match its own lib dirs and every
/// intra-package relative import would fall out of package space. Same
/// discipline as dartq-pub's inclusion map (`workspace.rs`
/// `inclusion_key`); a lib dir with no filesystem identity (a missing
/// package) keeps its normalized lexical form.
fn canonical_dir(dir: &Path) -> PathBuf {
    let normalized = normalize(dir);
    normalized.canonicalize().unwrap_or(normalized)
}

impl PackageTable {
    /// Indexes `config` whose entries resolve against `config_dir`, the
    /// directory holding the `package_config.json` (normally `.dart_tool/`).
    ///
    /// # Errors
    ///
    /// [`PubError`] when [`PackageConfig::resolve`] rejects the config — a
    /// broken `rootUri`, a name outside the reader grammar, or a
    /// `packageUri` escaping its root; a broken config, reported loudly.
    /// Duplicate names never reach here: dartq-pub floors the whole config
    /// at parse on the first repeat.
    pub fn new(config: &PackageConfig, config_dir: &Path) -> Result<Self, PubError> {
        Self::new_with_context_roots(config, config_dir, &[])
    }

    /// [`Self::new`] plus the assembly's discovered sibling context roots:
    /// canonical directories the nested-root walk identified as separate
    /// context roots (own `pubspec.yaml` + own `.dart_tool/package_config.
    /// json`). The merged config keeps only one entry per name, so a
    /// same-named sibling root never survives as an entry — the walk's list
    /// is the only place every root still exists, and each gets its own
    /// [`ContextUniverse`] exactly like an entry-discovered one.
    ///
    /// # Errors
    ///
    /// Same contract as [`Self::new`].
    pub fn new_with_context_roots(
        config: &PackageConfig,
        config_dir: &Path,
        context_roots: &[PathBuf],
    ) -> Result<Self, PubError> {
        let resolved = config.resolve(config_dir)?;
        let mut lib_dirs: Vec<(PathBuf, Box<str>)> = Vec::new();
        for package in &resolved {
            lib_dirs.push((
                canonical_dir(&package.lib_dir),
                Box::from(package.name.as_str()),
            ));
        }
        lib_dirs.sort_by_key(|(dir, _)| std::cmp::Reverse(dir.components().count()));
        let indices: FxHashMap<Box<str>, usize> = lib_dirs
            .iter()
            .enumerate()
            .map(|(index, (_, name))| (name.clone(), index))
            .collect();
        let workspace_root = canonical_dir(
            &config_dir
                .parent()
                .map_or_else(|| config_dir.to_path_buf(), Path::to_path_buf),
        );
        // Per-context universe discovery: a config entry whose package root
        // carries its own `pubspec.yaml` AND its own
        // `.dart_tool/package_config.json` is a separate context root (the
        // oracle's split trigger — see the struct docs). Discovery rides the
        // config's own entries: the assembly's nested-root extension
        // guarantees every self-resolved root under the analyzed tree is an
        // entry, and a `--packages` run skips that extension (the oracle's
        // `_defaultPackageConfigFile` short-circuit) so no entry carries its
        // own config and the table stays one flat universe, exactly there.
        //
        // Containment: only a root STRICTLY INSIDE the context root
        // (`workspace_root`) becomes a context. The oracle's locator walks
        // the analysis roots and only those (`context_locator.dart`
        // `locateRoots`/`_createContextRoots`: a directory outside the
        // included paths never becomes a context root), and everything a
        // context loads — dependencies included — resolves through that one
        // context's package map. An entry outside the context root (a
        // sibling/path dependency carrying its own leftover, possibly stale
        // or empty, `.dart_tool`) is only ever a dependency here: without
        // the gate its own config would hijack its files' `package:` URIs
        // and sever the export edges the analyzed context's fresh map would
        // have landed (every export-forwarded name reporting undefined).
        // The context root itself needs no context: the root universe IS
        // its map.
        // A nested config that exists but cannot back a universe is that
        // context's `Packages.empty` floor: every `package:` URI from under
        // the root stays unresolved — the oracle's parse-failure catch-all
        // (upstream Dart SDK source,
        // pkg/analyzer/lib/src/context/packages.dart
        // `parsePackageConfigJsonFile`), never a fall back to this table's
        // universe.
        fn context_universe(root: &Path) -> ContextUniverse {
            let nested_dir = root.join(".dart_tool");
            std::fs::read_to_string(nested_dir.join("package_config.json"))
                .ok()
                .and_then(|text| PackageConfig::parse(&text).ok())
                .and_then(|nested| nested.resolve(&nested_dir).ok())
                .map_or_else(
                    || ContextUniverse::empty(root.to_path_buf()),
                    |entries| ContextUniverse::new(&entries, root.to_path_buf()),
                )
        }
        let mut contexts: Vec<(PathBuf, ContextUniverse)> = Vec::new();
        for package in &resolved {
            let root = canonical_dir(&package.root);
            if root == workspace_root || !root.strip_prefix(&workspace_root).is_ok() {
                continue;
            }
            if !root.join("pubspec.yaml").is_file()
                || !root.join(".dart_tool/package_config.json").is_file()
            {
                continue;
            }
            let universe = context_universe(&root);
            contexts.push((root, universe));
        }
        // The assembly's discovered roots union in: the merge's name dedup
        // can drop a sibling root's whole entry set, so the scan above never
        // sees it — but the oracle still gives the directory its own
        // context. Same trigger (the walk already checked both files exist),
        // same universe read chain, same strict-inside gate; a root the scan
        // already built is skipped on its canonical path.
        for extra in context_roots {
            let root = normalize(extra);
            if root == workspace_root
                || !root.strip_prefix(&workspace_root).is_ok()
                || !root.join("pubspec.yaml").is_file()
                || !root.join(".dart_tool/package_config.json").is_file()
                || contexts.iter().any(|(known, _)| *known == root)
            {
                continue;
            }
            let universe = context_universe(&root);
            contexts.push((root, universe));
        }
        if contexts.is_empty() {
            return Ok(Self {
                lib_dirs,
                indices,
                workspace_root,
                root_universe: None,
                contexts,
            });
        }
        contexts.sort_by_key(|(root, _)| std::cmp::Reverse(root.components().count()));
        let nested_roots: FxHashSet<PathBuf> =
            contexts.iter().map(|(root, _)| root.clone()).collect();
        // The context-root universe is the root config AS WRITTEN (the
        // oracle parses each context's packages file as written): re-read
        // from disk, the written truth the extension must not widen — a
        // package the extension joined stays invisible outside its own
        // context, a package pub wrote into the root config stays visible.
        // A config that raced the assembly (deleted or corrupted after
        // acceptance) falls back to the given config minus the nested
        // roots — the conservative shape. Unreachable under `--packages`:
        // the override skips the extension, so no context exists here.
        let written = std::fs::read_to_string(config_dir.join("package_config.json"))
            .ok()
            .and_then(|text| PackageConfig::parse(&text).ok())
            .and_then(|written| written.resolve(config_dir).ok())
            .unwrap_or_else(|| {
                resolved
                    .iter()
                    .filter(|package| !nested_roots.contains(&canonical_dir(&package.root)))
                    .cloned()
                    .collect()
            });
        let root_universe = ContextUniverse::new(&written, workspace_root.clone());
        Ok(Self {
            lib_dirs,
            indices,
            workspace_root,
            root_universe: Some(root_universe),
            contexts,
        })
    }

    /// The lib dir of package `name`, or `None` for an unknown package.
    #[must_use]
    pub fn lib_dir(&self, name: &str) -> Option<&Path> {
        let index = *self.indices.get(name)?;
        Some(self.lib_dirs[index].0.as_path())
    }

    /// The `package:` universe governing `container`: the deepest nested
    /// self-resolved context root containing the file, else the context-root
    /// set. `None` when no nested context exists — the flat set is the one
    /// universe, and one early return keeps the common shape's hot path
    /// unchanged.
    fn universe_for(&self, container: &Path) -> Option<&ContextUniverse> {
        if self.contexts.is_empty() {
            return None;
        }
        self.context_for(container).or(self.root_universe.as_ref())
    }

    /// The deepest nested context root containing `path`, if any — the
    /// containing-context face spelling consults. A linear scan over the
    /// depth-sorted roots: no allocation, and the context count is bounded
    /// by the discovered nested roots.
    fn context_for(&self, path: &Path) -> Option<&ContextUniverse> {
        self.contexts
            .iter()
            .find(|(root, _)| path.strip_prefix(root).is_ok())
            .map(|(_, universe)| universe)
    }

    /// The canonical directory of the deepest nested context root
    /// containing `path`, or `None` outside every context — the
    /// context-identity face the engine's library keys consult: the oracle
    /// gives each context its own URI space, so the same `package:` URI in
    /// two sibling contexts is two libraries, and the qualifier is the
    /// containing root.
    #[must_use]
    pub fn context_root(&self, path: &Path) -> Option<&Path> {
        self.contexts
            .iter()
            .find(|(root, _)| path.strip_prefix(root).is_ok())
            .map(|(root, _)| root.as_path())
    }
}

/// One analysis context's `package:` universe: the package name → lib-dir
/// map its own `package_config.json` materializes, plus the context root the
/// generated-file fallback probes (`<root>/.dart_tool/build/generated`).
/// The rust-analyzer shape (upstream rust-analyzer source,
/// crates/hir-expand / the crate graph): each crate resolves names through
/// its own dependency closure, never a workspace-global table.
#[derive(Debug)]
struct ContextUniverse {
    /// Lib dirs with their package names, depth-sorted (deepest first) —
    /// the spelling face's prefix-scan index, in the flat table's
    /// `lib_dirs` shape and under the same deepest-first argument. One
    /// owned copy serves both faces through `indices`.
    dirs: Vec<(PathBuf, Box<str>)>,
    /// Package name → position in `dirs`: the resolution face's O(1)
    /// `lib_dir` lookup without a second copy of any dir.
    indices: FxHashMap<Box<str>, usize>,
    workspace_root: PathBuf,
}

impl ContextUniverse {
    /// Indexes the context config's resolved entries. Lib dirs pass through
    /// [`canonical_dir`], the same spelling discipline as the flat table, so
    /// the two faces can never diverge on one path's spelling.
    fn new<'p>(
        entries: impl IntoIterator<Item = &'p ResolvedPackage>,
        workspace_root: PathBuf,
    ) -> Self {
        let mut dirs: Vec<(PathBuf, Box<str>)> = entries
            .into_iter()
            .map(|package| {
                (
                    canonical_dir(&package.lib_dir),
                    Box::from(package.name.as_str()),
                )
            })
            .collect();
        dirs.sort_by_key(|(dir, _)| std::cmp::Reverse(dir.components().count()));
        let indices = dirs
            .iter()
            .enumerate()
            .map(|(index, (_, name))| (name.clone(), index))
            .collect();
        Self {
            dirs,
            indices,
            workspace_root,
        }
    }

    /// The oracle's `Packages.empty` floor: a context whose own config
    /// cannot be read, parsed, or resolved resolves nothing.
    fn empty(workspace_root: PathBuf) -> Self {
        Self {
            dirs: Vec::new(),
            indices: FxHashMap::default(),
            workspace_root,
        }
    }

    /// The lib dir of package `name`, or `None` for a package outside this
    /// context's universe.
    fn lib_dir(&self, name: &str) -> Option<&Path> {
        let index = *self.indices.get(name)?;
        Some(self.dirs[index].0.as_path())
    }

    /// The package whose lib dir (deepest match) contains `path`, with the
    /// lib-relative path — [`package_of`]'s scan over this context's own
    /// universe.
    fn package_of(&self, path: &Path) -> Option<(&str, String)> {
        for (dir, package) in &self.dirs {
            if let Ok(rest) = path.strip_prefix(dir) {
                if rest.as_os_str().is_empty() {
                    continue;
                }
                return Some((package.as_ref(), rest.to_string_lossy().into_owned()));
            }
        }
        None
    }
}

/// The `package:` universe one resolution consults: the flat table
/// ([`UniverseRef::Flat`], no nested context anywhere) or the container's
/// context's own set ([`UniverseRef::Scoped`]).
#[derive(Clone, Copy)]
enum UniverseRef<'a> {
    Flat(&'a PackageTable),
    Scoped(&'a ContextUniverse),
}

impl<'a> UniverseRef<'a> {
    /// The universe's lib dir for package `name`; `None` names a package
    /// outside the container's context — `uri_does_not_exist`.
    fn lib_dir(self, name: &str) -> Option<&'a Path> {
        match self {
            Self::Flat(table) => table.lib_dir(name),
            Self::Scoped(universe) => universe.lib_dir(name),
        }
    }

    /// The universe's generated-file base — the context root the
    /// `.dart_tool/build/generated` fallback probes under.
    fn workspace_root(self) -> &'a Path {
        match self {
            Self::Flat(table) => table.workspace_root.as_path(),
            Self::Scoped(universe) => universe.workspace_root.as_path(),
        }
    }
}

/// The resolution context: package table, SDK snapshot with its target
/// segment, the [`FileTable`] targets are interned into, and the engine's
/// file-residency view.
///
/// The containing file's canonical base URI is memoized per [`FileId`], so
/// each file pays one package scan and every directive afterwards is hash
/// lookups plus string work.
pub struct UriResolver<'a> {
    packages: &'a PackageTable,
    sdk: &'a SdkSnapshot,
    /// The declared-variables context of conditional evaluation
    /// (`_fsState._declaredVariables`, `file_state.dart:738-764`): the
    /// `-D name=value` pairs configurable directives test against — empty
    /// in the engine's oracle-default context today. Consumed by
    /// [`Self::resolve_conditional`].
    declared: &'a FxHashMap<String, String>,
    files: &'a mut FileTable,
    /// The engine's file-state existence face (`FileState.exists`): which
    /// interned ids hold content in the analyzed world. A target the engine
    /// declares resident exists even though it never hit disk — the oracle's
    /// overlay semantics — so the missing-target codes only fire for paths
    /// outside residency.
    resident: &'a dyn Fn(FileId) -> bool,
    bases: FxHashMap<FileId, BaseUri>,
    /// Per-container context universe memo (the pub universe separation): one
    /// context-root scan per file — the memoized answer every directive of
    /// the file resolves against — in the same per-file shape as `bases`.
    universes: FxHashMap<FileId, UniverseRef<'a>>,
    /// Canonical URIs memoized per `FileTable`-normalized path for this
    /// resolver's lifetime — the package table cannot change under a live
    /// resolver, so [`Self::canonical_uri`] pays its package scan once per
    /// path instead of once per call.
    canonical: FxHashMap<PathBuf, Box<str>>,
    /// Existence answers memoized per normalized path for this resolver's
    /// lifetime — the analyzed world is frozen while one resolver runs, so
    /// [`Self::exists`] pays its stat once per target instead of once per
    /// directive naming it.
    existence: FxHashMap<PathBuf, bool>,
}

/// The canonical URI space a containing file resolves relatives against.
#[derive(Clone, Debug, PartialEq, Eq)]
enum BaseUri {
    /// `package:<package>/<path>` — the container is under a package lib dir.
    Package { package: String, path: String },
    /// `file://<path>` — the container is anywhere else.
    File(String),
}

/// An absolute URI after composition, ready to be landed on a target. The
/// parts borrow the parse — nothing is re-joined and re-split; the merge
/// paths of relative composition are the only owned strings, and landing
/// materializes the filesystem paths.
enum Composed<'a> {
    Package {
        package: Cow<'a, str>,
        rel: Cow<'a, str>,
    },
    File(Cow<'a, str>),
    /// A `dart:` URI split at the library boundary: the path's first segment
    /// (`core` for both `dart:core` and `dart:core/bool.dart`; an authority
    /// form leaves it empty, which no SDK table lists) and the raw part
    /// remainder — exactly the pair [`SdkSnapshot::flat_library_to_path`]
    /// consumes.
    Dart {
        library: Cow<'a, str>,
        part: Cow<'a, str>,
    },
    DartExt,
    /// Any other absolute scheme (`data:`, `http:`, …): parses, resolves to
    /// nothing.
    Other,
}

/// The package whose lib dir (deepest match) contains `path`, with the
/// lib-relative path. Scans the depth-sorted index with an early exit on the
/// first component-boundary prefix hit (`Path::strip_prefix` semantics:
/// `/projx/lib/a.dart` does not match a `/proj` package).
fn package_of<'p>(packages: &'p PackageTable, path: &Path) -> Option<(&'p str, String)> {
    for (dir, package) in &packages.lib_dirs {
        if let Ok(rest) = path.strip_prefix(dir) {
            if rest.as_os_str().is_empty() {
                continue; // the lib dir itself is not a file inside it
            }
            return Some((package.as_ref(), rest.to_string_lossy().into_owned()));
        }
    }
    None
}

/// The canonical URI string of `path` (`pathToUri` face) against `memo` —
/// the one implementation of the spelling, memoized per `FileTable`
/// -normalized path. `container_base` reaches it through field-disjoint
/// borrows (the base computation runs while `files` holds the container's
/// interned path).
fn canonical_uri_memo(
    canonical: &mut FxHashMap<PathBuf, Box<str>>,
    packages: &PackageTable,
    path: &Path,
) -> Box<str> {
    if let Some(known) = canonical.get(path) {
        return known.clone();
    }
    let uri = compose_canonical_uri(packages, path);
    canonical.insert(path.to_path_buf(), uri.clone());
    uri
}

/// The pure spelling computation behind [`canonical_uri_memo`] and
/// [`UriResolver::canonical_uri`]: `package:` inside a package lib dir
/// (deepest match wins), `file://` otherwise.
///
/// The containing context's own universe spells first — the oracle gives
/// every context root its own `PackageMapUriResolver`, so a file under a
/// nested self-resolved root spells through that context's package map
/// (and only that map: a miss inside a context is `file://`, never a fall
/// through to a sibling universe). Outside every context the flat merged
/// scan spells exactly as before — the common single-root shape keeps one
/// `contexts`-is-empty check plus the flat scan, byte-identical.
pub fn compose_canonical_uri(packages: &PackageTable, path: &Path) -> Box<str> {
    let spelled = match packages.context_for(path) {
        Some(universe) => universe.package_of(path),
        // Outside every context — including the common no-contexts shape,
        // where the empty `find` short-circuits — the flat merged scan
        // spells byte-identically.
        None => package_of(packages, path),
    };
    if let Some((package, rel)) = spelled {
        Box::from(format!("package:{package}/{}", encode_uri_path(&rel)))
    } else {
        let text = path.to_string_lossy();
        let encoded = encode_uri_path(&text);
        let with_slash = if encoded.starts_with('/') {
            encoded
        } else {
            format!("/{encoded}")
        };
        Box::from(format!("file://{with_slash}"))
    }
}

/// The canonical base URI space `path` composes relatives in, derived from
/// [`canonical_uri_memo`] (the single spelling implementation): the
/// `package:` form splits at the first `/` — package names never carry one,
/// and [`package_of`] guarantees a non-empty lib-relative path — while the
/// `file://` form keeps its slash-anchored path.
fn container_base(
    canonical: &mut FxHashMap<PathBuf, Box<str>>,
    packages: &PackageTable,
    path: &Path,
) -> BaseUri {
    let uri = canonical_uri_memo(canonical, packages, path);
    if let Some(rest) = uri.strip_prefix("package:") {
        let (package, rel) = rest
            .split_once('/')
            .expect("package canonical URIs carry a lib-relative path");
        return BaseUri::Package {
            package: String::from(package),
            path: String::from(rel),
        };
    }
    BaseUri::File(String::from(
        uri.strip_prefix("file://")
            .expect("canonical URIs are package: or file://"),
    ))
}

impl<'a> UriResolver<'a> {
    /// Creates a resolver over `packages` and `sdk`, dispatching `dart:` URIs
    /// against the SDK target segment `target` (e.g. `"vm"`) and interning
    /// file targets into `files`. `resident` reports which interned ids hold
    /// content in the analyzed world (disk files and declared overlays
    /// alike); a target it accepts never codes `uri_does_not_exist`.
    /// `declared` is the conditional-evaluation context (the module pin in
    /// [`crate::conditional`]): the no-`-D` oracle default is an empty map,
    /// under which every configurable directive resolves its primary URI.
    pub fn new(
        packages: &'a PackageTable,
        sdk: &'a SdkSnapshot,
        declared: &'a FxHashMap<String, String>,
        files: &'a mut FileTable,
        resident: &'a dyn Fn(FileId) -> bool,
    ) -> Self {
        Self {
            packages,
            sdk,
            declared,
            files,
            resident,
            bases: FxHashMap::default(),
            universes: FxHashMap::default(),
            canonical: FxHashMap::default(),
            existence: FxHashMap::default(),
        }
    }

    /// Whether the analyzed world holds `path`: on disk, or resident in the
    /// engine (an id the owner of [`Self::resident`] declared content for —
    /// a declared overlay the disk has never seen). Memoized per normalized
    /// path for this resolver's lifetime: the analyzed world does not mutate
    /// while one resolver runs, so a stat answered once is not paid again —
    /// a cold 10⁵-file analysis otherwise pays one stat syscall per resolved
    /// target. Callers pass `FileTable`-normalized paths (both in-crate
    /// sites normalize before asking); drop the resolver to observe later
    /// disk changes.
    fn exists(&mut self, path: &Path) -> bool {
        if let Some(&known) = self.existence.get(path) {
            return known;
        }
        let found = path.exists()
            || self
                .files
                .try_file_id(path)
                .is_some_and(|id| (self.resident)(id));
        self.existence.insert(path.to_path_buf(), found);
        found
    }

    /// The context universe `container`'s `package:` lookups consult,
    /// memoized per container: the deepest nested self-resolved root
    /// containing the file, else the context-root set, else — no nested
    /// context anywhere — the flat table.
    fn universe_of(&mut self, container: FileId) -> UniverseRef<'a> {
        if let Some(&known) = self.universes.get(&container) {
            return known;
        }
        let universe = self
            .files
            .path(container)
            .and_then(|path| self.packages.universe_for(path));
        let reference = match universe {
            Some(universe) => UniverseRef::Scoped(universe),
            None => UniverseRef::Flat(self.packages),
        };
        self.universes.insert(container, reference);
        reference
    }

    /// The [`FileTable`] targets were interned into.
    #[must_use]
    pub const fn file_table(&self) -> &FileTable {
        self.files
    }

    /// Resolves one directive URI string against `container`'s canonical URI.
    ///
    /// `uri_str` is `None` when the directive's URI literal is a string
    /// interpolation (the parser sees no resolvable string) — the oracle's
    /// `DirectiveUriWithoutString` face, reported as
    /// `uri_with_interpolation`.
    ///
    /// `kind` only shapes the diagnostic face of a missing target
    /// ([`DirectiveKind`]); the target lookup itself is identical for both.
    ///
    /// # Panics
    ///
    /// Panics when `container` is not interned in this resolver's
    /// [`FileTable`] — a caller bug (ids only come from that table).
    pub fn resolve(
        &mut self,
        container: FileId,
        uri_str: Option<&str>,
        kind: DirectiveKind,
    ) -> UriResolution {
        let resolution = self.resolve_uri(container, uri_str);
        match kind {
            DirectiveKind::ImportExport => resolution,
            DirectiveKind::PartOf => UriResolution {
                code: None,
                ..resolution
            },
            DirectiveKind::DocImport => UriResolution {
                code: resolution.code.map(|code| match code {
                    ResolutionCode::UriDoesNotExist | ResolutionCode::UriHasNotBeenGenerated => {
                        ResolutionCode::UriDoesNotExistInDocImport
                    }
                    other => other,
                }),
                ..resolution
            },
        }
    }

    /// Resolves one configurable directive (`import`/`export` with `if`
    /// clauses, `parseConditionalUriStar` serving both grammars): selects
    /// the directive's URI against this resolver's declared variables —
    /// first matching configuration wins, the primary URI is the fallback
    /// (`file_state.dart:738-764`) — and resolves the chosen candidate
    /// through [`Self::resolve`]. Only the chosen candidate can carry a
    /// missing-target diagnostic; unmatched candidates are never read from
    /// disk (`library_analyzer.dart:698-717` reports `state.selectedUri`
    /// alone).
    ///
    /// `primary_uri` is `None` when the directive's primary URI literal is a
    /// string interpolation; it only matters when no configuration matches.
    pub fn resolve_conditional(
        &mut self,
        container: FileId,
        primary_uri: Option<&str>,
        configs: &[Configuration],
        kind: DirectiveKind,
    ) -> UriResolution {
        let chosen = match resolve_conditional(configs, self.declared) {
            SelectedUri::Primary => primary_uri,
            SelectedUri::Configuration(index) => Some(configs[index].uri.as_ref()),
        };
        self.resolve(container, chosen, kind)
    }

    /// The kind-independent target lookup behind [`Self::resolve`].
    fn resolve_uri(&mut self, container: FileId, uri_str: Option<&str>) -> UriResolution {
        let Some(uri_str) = uri_str else {
            return UriResolution {
                uri: ResolvedUri::Unresolved(UnresolvedKind::BadUri),
                target: ResolvedTarget::Missing,
                code: Some(ResolutionCode::UriWithInterpolation),
            };
        };
        let Ok(raw) = parse_directive_uri(uri_str) else {
            return UriResolution {
                uri: ResolvedUri::Unresolved(UnresolvedKind::BadUri),
                target: ResolvedTarget::Missing,
                code: Some(ResolutionCode::InvalidUri),
            };
        };
        // A fragment (even an empty `#`) makes the URI unresolvable
        // upstream — `import 'q.dart#f'`, `part 'p.dart#f'`,
        // `export`, `package:x/y#f`, `dart:core#f`, and `file:///x#f` all
        // report `uri_does_not_exist` with the target's path on disk
        // irrelevant.
        if raw.has_fragment {
            return UriResolution {
                uri: ResolvedUri::Unresolved(UnresolvedKind::NotExist),
                target: ResolvedTarget::Missing,
                code: Some(ResolutionCode::UriDoesNotExist),
            };
        }
        if !raw.scheme.is_empty() {
            let universe = self.universe_of(container);
            return self.land(compose_absolute(raw), universe);
        }
        // Field-disjoint borrows: the memoized base is read out of `bases`
        // before `land` mutates the other memos and `files`.
        if !self.bases.contains_key(&container) {
            let path = self
                .files
                .path(container)
                .expect("container FileId interned in this resolver's FileTable");
            let base = container_base(&mut self.canonical, self.packages, path);
            self.bases.insert(container, base);
        }
        let composed = compose_relative(&self.bases[&container], raw);
        let universe = self.universe_of(container);
        self.land(composed, universe)
    }

    /// The canonical URI of `path` (`pathToUri` face): `package:` inside a
    /// package lib dir (deepest match wins), `file://` otherwise. This is the
    /// library-identity string the library-graph layer keys on — and the one
    /// implementation of the spelling, so a change to it cannot desync the
    /// composition base (`container_base` derives from it). Memoized per
    /// `FileTable`-normalized path for this resolver's lifetime (the package
    /// table cannot change under a live resolver); the returned borrow shares
    /// that lifetime, and callers needing a longer horizon cache the owned
    /// result — the engine's per-file canonical table does exactly that.
    ///
    /// # Panics
    ///
    /// Panics if the memo table misses after the insert above — a program
    /// bug (the cache is `&mut self`, so no concurrent mutation is
    /// possible).
    #[must_use]
    pub fn canonical_uri(&mut self, path: &Path) -> &str {
        if !self.canonical.contains_key(path) {
            let uri = compose_canonical_uri(self.packages, path);
            self.canonical.insert(path.to_path_buf(), uri);
        }
        self.canonical
            .get(path)
            .map(Box::as_ref)
            .expect("memoized above")
    }

    /// Lands a composed absolute URI on a target: scheme dispatch, existence,
    /// generated-file fallback, interning, and the code. `universe` is
    /// the container's context universe — the only face a `package:` target
    /// resolves through.
    fn land(&mut self, composed: Composed<'_>, universe: UniverseRef<'a>) -> UriResolution {
        match composed {
            Composed::DartExt => UriResolution {
                uri: ResolvedUri::Unresolved(UnresolvedKind::DartExt),
                target: ResolvedTarget::Missing,
                code: Some(ResolutionCode::UseOfNativeExtension),
            },
            Composed::Dart { library, part } => {
                // Library lookup is the SDK's flat face — the oracle's
                // `libraries.dart` map, flat across all target segments
                // (`SdkSnapshot::flat_library_to_path`); the oracle never
                // consults platform flags or per-segment tables for `dart:`
                // URI existence, so neither does resolution.
                //
                // Entry URIs (`dart:math`) resolve on table membership
                // alone: `internalMapDartUri` (dart/sdk/sdk.dart:90-130)
                // maps a known library name to a FileSource
                // unconditionally — it never consults the file system, and
                // every real SDK ships the mapped sources. dartq's
                // generated names-only faces (a library whose SDK root
                // carries no sources) are the same table rows, so they must
                // resolve silently too. Part paths (`dart:math/bar.dart`)
                // keep the existence check: the oracle then tests
                // `importedFile.exists` (library_analyzer.dart:714-728), so
                // a mapped-but-missing part file still reports
                // `uri_does_not_exist`.
                match self.sdk.flat_library_to_path(&library, &part) {
                    Some(file) => {
                        let code = if !part.is_empty() {
                            (!self.exists(&file)).then(|| missing_file_code(&file))
                        } else {
                            None
                        };
                        UriResolution {
                            uri: ResolvedUri::Dart {
                                library: library.into_owned().into_boxed_str(),
                                file,
                            },
                            target: ResolvedTarget::External,
                            code,
                        }
                    }
                    // Unknown dart: library: source is null —
                    // `uri_does_not_exist` per the URI-layer contract.
                    None => UriResolution {
                        uri: ResolvedUri::Unresolved(UnresolvedKind::NotExist),
                        target: ResolvedTarget::Missing,
                        code: Some(ResolutionCode::UriDoesNotExist),
                    },
                }
            }
            Composed::Other => UriResolution {
                uri: ResolvedUri::Unresolved(UnresolvedKind::NotExist),
                target: ResolvedTarget::Missing,
                code: Some(ResolutionCode::UriDoesNotExist),
            },
            Composed::File(path) => {
                let Some(text) = decode_uri_path(&path) else {
                    return UriResolution {
                        uri: ResolvedUri::Unresolved(UnresolvedKind::NotExist),
                        target: ResolvedTarget::Missing,
                        code: Some(ResolutionCode::UriDoesNotExist),
                    };
                };
                let path = normalize(Path::new(&*text));
                let code = (!self.exists(&path)).then(|| missing_file_code(&path));
                let id = self.files.intern(&path);
                UriResolution {
                    uri: ResolvedUri::File(path),
                    target: ResolvedTarget::Resolved(id),
                    code,
                }
            }
            Composed::Package { package, rel } => self.land_package(universe, package, &rel),
        }
    }

    /// Lands a `package:` target: lib-dir lookup in the container's context
    /// universe (the pub universe separation — a package outside it is
    /// unresolvable however global the table), the generated-file fallback
    /// under the universe's own context root, and the missing-file code. The
    /// package name is cloned only into the returned [`ResolvedUri::Package`]
    /// — an unknown package (the lookup miss) allocates nothing.
    fn land_package(
        &mut self,
        universe: UniverseRef<'a>,
        package: Cow<'_, str>,
        rel: &str,
    ) -> UriResolution {
        let unresolvable = || UriResolution {
            uri: ResolvedUri::Unresolved(UnresolvedKind::NotExist),
            target: ResolvedTarget::Missing,
            code: Some(ResolutionCode::UriDoesNotExist),
        };
        let Some(lib_dir) = universe.lib_dir(&package) else {
            return unresolvable();
        };
        let Some(decoded) = decode_uri_path(rel) else {
            return unresolvable();
        };
        let file = normalize(&join_rel(lib_dir, &decoded));
        if !self.exists(&file) {
            // Generated-file face: `.dart_tool/build/generated/<pkg>/lib/`
            // + the raw (undecoded) rel path, verbatim from the resolver
            // chain; used only when present (build output, never an overlay
            // — the probe stays disk-only). The fallback probes the
            // universe's own context root, the context's generated face.
            let built = join_rel(
                &universe
                    .workspace_root()
                    .join(".dart_tool/build/generated")
                    .join(package.as_ref())
                    .join("lib"),
                rel,
            );
            if built.exists() {
                let id = self.files.intern(&built);
                return UriResolution {
                    uri: ResolvedUri::Package {
                        package: package.into_owned().into_boxed_str(),
                        rel: PathBuf::from(&*decoded),
                        file: built,
                    },
                    target: ResolvedTarget::Resolved(id),
                    code: None,
                };
            }
            let code = missing_file_code(&file);
            let id = self.files.intern(&file);
            return UriResolution {
                uri: ResolvedUri::Package {
                    package: package.into_owned().into_boxed_str(),
                    rel: PathBuf::from(&*decoded),
                    file,
                },
                target: ResolvedTarget::Resolved(id),
                code: Some(code),
            };
        }
        let id = self.files.intern(&file);
        UriResolution {
            uri: ResolvedUri::Package {
                package: package.into_owned().into_boxed_str(),
                rel: PathBuf::from(&*decoded),
                file,
            },
            target: ResolvedTarget::Resolved(id),
            code: None,
        }
    }
}

/// The code for a known-path-but-missing file: generated-suffix paths
/// report `uri_has_not_been_generated`, everything else `uri_does_not_exist`
/// (`library_analyzer.dart:694-700`).
fn missing_file_code(path: &Path) -> ResolutionCode {
    const GENERATED_SUFFIXES: [&str; 6] = [
        ".g.dart",
        ".pb.dart",
        ".pbenum.dart",
        ".pbserver.dart",
        ".pbjson.dart",
        ".template.dart",
    ];
    let text = path.to_string_lossy();
    if GENERATED_SUFFIXES
        .iter()
        .any(|suffix| text.ends_with(suffix))
    {
        ResolutionCode::UriHasNotBeenGenerated
    } else {
        ResolutionCode::UriDoesNotExist
    }
}

/// A directive URI string failed Dart `Uri.parse` validity rules — the
/// `invalid_uri` face. Carries no detail by design: the only information is
/// failure itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidUriSyntax;

/// A directive URI string parsed under Dart `Uri.parse` validity rules
/// (`sdk/lib/core/uri.dart` character tables). Query and fragment are
/// validated but dropped: they never participate in file-target resolution.
///
/// The parts borrow `uri` — the parse allocates nothing on the common
/// (already-lowercase scheme) shapes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawUri<'a> {
    /// Lowercased scheme, empty for relative URIs. Borrowed verbatim when the
    /// source spelling is already lowercase; only mixed-case spellings pay
    /// the lowercase copy.
    pub scheme: Cow<'a, str>,
    /// Raw authority text (between `//` and the next `/`, `?`, `#`), `Some("")`
    /// for an empty authority as in `file:///x`.
    pub authority: Option<Cow<'a, str>>,
    /// The path in `Uri.parse` normalized form: every byte `Uri.parse`
    /// would percent-escape is already an uppercase `%XX`, `\` is a `/`,
    /// and escapes decode to their pchar (non-`/`) form
    /// (`%41`→`A`, `%2f`→`%2F`). Borrowed verbatim when the source spelling
    /// was already normalized — the overwhelmingly common shape.
    pub path: Cow<'a, str>,
    /// Whether the URI spelled a `#fragment` (even an empty one). Directive
    /// resolution treats a fragment as unresolvable — upstream every
    /// fragment-carrying URI fails to map to a file (`uri_does_not_exist`
    /// on `import 'q.dart#f'`, `export`, `part`, `dart:core#f`, and
    /// `file:///x#f` alike).
    pub has_fragment: bool,
}

/// Parses `uri` with Dart `Uri.tryParse` fidelity for the forms directive
/// strings take: scheme syntax, authority shape, per-component character
/// tables, and percent-escape well-formedness. Non-ASCII characters are
/// accepted (Dart normalizes them to escapes; targets are decoded paths).
/// The result borrows `uri`.
///
/// # Errors
///
/// [`InvalidUriSyntax`] where Dart throws `FormatException` — reported as
/// `invalid_uri`.
pub fn parse_directive_uri(uri: &str) -> Result<RawUri<'_>, InvalidUriSyntax> {
    // `Uri.parse` routes any string starting with a case-insensitive `data:`
    // to `UriData._parse` before the general scanner runs (uri.dart:931-939),
    // so the path character tables never apply to it.
    if uri.len() >= 5 && uri.as_bytes()[..5].eq_ignore_ascii_case(b"data:") {
        return parse_data_uri(uri);
    }
    // `Uri.parse` splits the string at the FIRST `?`/`#` before any
    // normalization, so a `\` inside the query or fragment stays a literal
    // (escaped to `%5C`), while a `\` in the head parses exactly like a `/`
    // — `a\b.dart` is `a/b.dart` and `file://a\b/x` has authority `a`.
    // Doing the same split here lets the head normalize `\`→`/` up front.
    let head_end = uri
        .as_bytes()
        .iter()
        .position(|&b| matches!(b, b'?' | b'#'))
        .unwrap_or(uri.len());
    // A `\`-folded head is a local String: nothing parsed out of it may
    // borrow home, so every component materializes owned when `head_owned`
    // and borrows `uri` (indices coincide — the fold is byte-wise 1:1).
    let folded;
    let (head_str, head_owned) = if uri.as_bytes()[..head_end].contains(&b'\\') {
        folded = uri[..head_end].replace('\\', "/");
        (folded.as_str(), true)
    } else {
        (&uri[..head_end], false)
    };
    let bytes = head_str.as_bytes();

    // Scheme: a `:` before the first `/` makes its prefix a scheme candidate
    // — every byte before it must satisfy the scheme grammar ("Invalid empty
    // scheme" / "Scheme not starting with alphabetic character" / "Illegal
    // scheme character": `a b:c`, `a%b:c`, `1a:b`, `:x` all throw upstream).
    // A `:` reached only after a `/` (`a/b:c`) stays a path character.
    let first_colon = bytes.iter().position(|&b| b == b':');
    let first_slash = bytes.iter().position(|&b| b == b'/');
    let has_scheme =
        matches!(first_colon, Some(colon) if first_slash.is_none_or(|slash| colon < slash));
    let (scheme, mut index) = if has_scheme {
        let scheme_end = first_colon.expect("has_scheme implies a colon");
        if scheme_end == 0
            || !bytes[0].is_ascii_alphabetic()
            || !bytes[..scheme_end].iter().all(|&b| is_scheme_char(b))
        {
            return Err(InvalidUriSyntax);
        }
        // The folded head is a local: slice `uri` (the fold is byte-wise
        // 1:1, so the indices coincide) for the borrowed shape.
        let scheme = if head_owned {
            Cow::Owned(head_str[..scheme_end].to_ascii_lowercase())
        } else {
            lowered_scheme(&uri[..scheme_end])
        };
        (scheme, scheme_end + 1)
    } else {
        (Cow::Borrowed(""), 0)
    };

    // Authority: everything after `//` up to the next `/` (the head was
    // already `\`-folded, so `\` can no longer appear inside it). The only
    // rejection `Uri.parse` keeps is a malformed `[` IPv6 literal — every
    // other byte, spaces and broken `%` escapes included, normalizes to
    // escapes rather than throwing (`file://a%zz/` parses).
    let mut authority = None;
    if bytes.len() >= index + 2 && bytes[index] == b'/' && bytes[index + 1] == b'/' {
        let start = index + 2;
        let end = bytes[start..]
            .iter()
            .position(|&b| b == b'/')
            .map_or(bytes.len(), |offset| start + offset);
        validate_authority_shape(&head_str[start..end])?;
        authority = Some(if head_owned {
            Cow::Owned(head_str[start..end].to_owned())
        } else {
            Cow::Borrowed(&uri[start..end])
        });
        index = end;
    }

    // Same slicing rule as the scheme: normalize the folded head when it is
    // a local, `uri` when it is not (indices coincide).
    let path = if head_owned {
        Cow::Owned(normalize_component(&head_str[index..]).into_owned())
    } else {
        normalize_component(&uri[index..head_end])
    };
    // A fragment (even an empty `#`) makes the whole URI unresolvable
    // upstream; the query never participates in file-target resolution.
    let tail = &uri[head_end..];
    let has_fragment = tail.contains('#');

    Ok(RawUri {
        scheme,
        authority,
        path,
        has_fragment,
    })
}

/// Parses the `data:` face. The validity boundary is `UriData._parse`'s
/// `FormatException` faces (uri.dart:4254-4313), mirrored here: a non-empty
/// MIME type carries exactly one `/`; every `;` parameter carries an `=`,
/// except a lone `base64` immediately before the `,`; and on the `;base64`
/// face the payload must satisfy `base64.normalize` (base64.dart:97-189).
/// The non-base64 payload never rejects: the oracle only normalizes it
/// (malformed escapes repaired, everything else percent-escaped), so
/// anything after the comma is accepted as written — `?` and `#` are data,
/// not delimiters.
fn parse_data_uri(uri: &str) -> Result<RawUri<'_>, InvalidUriSyntax> {
    let bytes = uri.as_bytes();
    // MIME type: up to the first `,` or `;`.
    let mut index = 5;
    let mut slash_seen = false;
    while index < bytes.len() && bytes[index] != b',' && bytes[index] != b';' {
        if bytes[index] == b'/' {
            if slash_seen {
                return Err(InvalidUriSyntax); // "Invalid MIME type"
            }
            slash_seen = true;
        }
        index += 1;
    }
    if !slash_seen && index > 5 {
        return Err(InvalidUriSyntax); // non-empty MIME type without `/`
    }
    // Parameter sections. The oracle's loop runs until the current character
    // is a comma, so a string that ends without one rejects here too.
    let mut is_base64 = false;
    while bytes.get(index) != Some(&b',') {
        let section = index;
        index += 1;
        let mut equals_seen = false;
        while index < bytes.len() && bytes[index] != b';' && bytes[index] != b',' {
            if bytes[index] == b'=' {
                equals_seen = true;
            }
            index += 1;
        }
        if equals_seen {
            continue;
        }
        // Without `=` the section must be exactly `base64` with the comma
        // right behind it ("base64," is 7 characters).
        if index == section + 7
            && bytes.get(index) == Some(&b',')
            && &bytes[section + 1..index] == b"base64"
        {
            is_base64 = true;
            break;
        }
        return Err(InvalidUriSyntax); // "Expecting '='"
    }
    if is_base64 && !is_normalized_base64(&bytes[index + 1..]) {
        return Err(InvalidUriSyntax); // "Invalid base64 data" / padding
    }
    Ok(RawUri {
        // The oracle spells the scheme without the colon: `Uri.parse`'s
        // `data:` face builds `_DataUri` → `super._internal("data", …)`
        // (uri.dart:5122-5126).
        scheme: lowered_scheme(&uri[..4]),
        authority: None,
        path: Cow::Borrowed(&uri[5..]),
        // `UriData._parse` swallows everything after the `,` — a `#` is
        // data, never a fragment delimiter.
        has_fragment: false,
    })
}

/// Splits a raw path at its first `/` — the library/part pair of a `dart:`
/// URI and the package/rel pair of a `package:` URI. A borrowed path (the
/// common shape out of [`parse_directive_uri`]) splits into borrowed slices;
/// the owned variant (a path the parse normalized) is re-spliced owned so
/// the helper is total over the public type.
fn split_path_cow(path: Cow<'_, str>) -> (Cow<'_, str>, Cow<'_, str>) {
    match path {
        Cow::Borrowed(path) => match path.split_once('/') {
            Some((head, tail)) => (Cow::Borrowed(head), Cow::Borrowed(tail)),
            None => (Cow::Borrowed(path), Cow::Borrowed("")),
        },
        Cow::Owned(path) => {
            let (head, tail) = match path.split_once('/') {
                Some((head, tail)) => (String::from(head), String::from(tail)),
                None => (path, String::new()),
            };
            (Cow::Owned(head), Cow::Owned(tail))
        }
    }
}

/// The acceptance half of `base64.normalize` (base64.dart:97-189), which
/// `UriData._parse` applies to `;base64` payloads: `%XX` escapes decode
/// first (short or non-hex escapes, and a decoded `%`, are invalid), the
/// alphabet is `A-Za-z0-9+/ -_`, padding must be one or two `=` closing the
/// payload with the total length a multiple of four, and an unpadded
/// payload whose length is 1 modulo 4 is the one rejected length.
fn is_normalized_base64(bytes: &[u8]) -> bool {
    let mut length = 0;
    let mut first_padding = None;
    let mut padding = 0;
    let mut index = 0;
    while index < bytes.len() {
        let mut char = bytes[index];
        index += 1;
        if char == b'%' {
            let Some(hex) = bytes.get(index..index + 2) else {
                return false;
            };
            let Some(high) = (hex[0] as char).to_digit(16) else {
                return false;
            };
            let Some(low) = (hex[1] as char).to_digit(16) else {
                return false;
            };
            let Ok(byte) = u8::try_from(high * 16 + low) else {
                return false;
            };
            char = byte;
            if char == b'%' {
                return false;
            }
        }
        match char {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'/' | b'-' | b'_' => length += 1,
            b'=' => {
                if first_padding.is_none() {
                    first_padding = Some(length);
                }
                padding += 1;
                length += 1;
            }
            _ => return false,
        }
    }
    match first_padding {
        Some(first) => length % 4 == 0 && first + padding == length && padding <= 2,
        None => length % 4 != 1,
    }
}

/// The path character table (`_pathCharOrSlashMask`): pchar + `/`.
/// `Uri.parse` never rejects a byte outside the table — it percent-escapes
/// it — so the table decides *spelling*, not validity. Query/fragment text
/// never reaches file resolution, so only the path table is needed.

/// Whether `b` is legal in a path under `_pathCharOrSlashMask`
/// (pchar + `/`); the authority table adds `:`/`@`, and the query table adds
/// `?` on top of that — neither is needed here since the authority keeps its
/// raw spelling and query/fragment text never reaches file resolution.
fn is_path_char(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'/' | b':'
                | b'@'
                | b'-'
                | b'.'
                | b'_'
                | b'~'
                | b'!'
                | b'$'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b';'
                | b'='
        )
}

/// Whether `b` is RFC 3986 unreserved — the only class `Uri.parse` decodes
/// a `%XX` escape to in the path (`%41`→`A`, `%2D`→`-`, but `%2F`/`%3A`
////`%20` stay escapes).
fn is_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')
}

fn is_scheme_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')
}

/// The scheme spelling Dart normalizes to: already-lowercase schemes (the
/// overwhelmingly common directive shapes — `package`, `file`, `dart`,
/// `data`) borrow the slice verbatim; only mixed-case spellings pay the
/// ASCII-lowercase copy.
fn lowered_scheme(slice: &str) -> Cow<'_, str> {
    if slice.bytes().any(|b| b.is_ascii_uppercase()) {
        Cow::Owned(slice.to_ascii_lowercase())
    } else {
        Cow::Borrowed(slice)
    }
}

/// `Uri.parse`'s lenient normalization of one component (`_normalize` +
/// `_normalizeEscape`): never rejects — everything outside the component's
/// table percent-encodes — and never decodes except an escape that names an
/// unreserved byte. Bytes:
///
/// - legal per the table: verbatim;
/// - `%XX` well-formed: unreserved decoded (`%41`→`A`), everything else
///   re-spelled as uppercase `%XX` (`%2f`→`%2F`);
/// - `%` not starting a valid escape: `%25` (`a%zz`→`a%25zz`);
/// - non-table ASCII (space, `<`, `[`, `"`, `|`, `{`, `}`, `^`, `` ` ``):
///   `%XX`;
/// - non-ASCII: each UTF-8 byte `%XX` (`ü`→`%C3%BC`).
///
/// `\` never reaches here — the head folded it to `/` before scanning
/// (a `\` inside the query/fragment escapes to `%5C`, which this would
/// produce too if `Uri.parse` ever saw it here). Borrows `text` verbatim
/// when it is already normalized — the overwhelmingly common shape.
fn normalize_component(text: &str) -> Cow<'_, str> {
    fn needs_normalization(bytes: &[u8]) -> bool {
        let mut index = 0;
        while index < bytes.len() {
            let b = bytes[index];
            if b == b'%' {
                match bytes.get(index + 1..index + 3) {
                    Some(hex) if hex.iter().all(u8::is_ascii_hexdigit) => {
                        let value = (hex[0] as char).to_digit(16).unwrap() as u8 * 16
                            + (hex[1] as char).to_digit(16).unwrap() as u8;
                        if is_unreserved(value)
                            || !hex[0].is_ascii_uppercase()
                            || !hex[1].is_ascii_uppercase()
                        {
                            return true;
                        }
                    }
                    _ => return true,
                }
                index += 3;
            } else {
                if b >= 0x80 || !is_path_char(b) {
                    return true;
                }
                index += 1;
            }
        }
        false
    }
    if !needs_normalization(text.as_bytes()) {
        return Cow::Borrowed(text);
    }
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let b = bytes[index];
        if b == b'%' {
            match bytes.get(index + 1..index + 3) {
                Some(hex) if hex.iter().all(u8::is_ascii_hexdigit) => {
                    let value = (hex[0] as char).to_digit(16).unwrap() as u8 * 16
                        + (hex[1] as char).to_digit(16).unwrap() as u8;
                    if is_unreserved(value) {
                        out.push(value);
                    } else {
                        out.extend_from_slice(&[
                            b'%',
                            hex[0].to_ascii_uppercase(),
                            hex[1].to_ascii_uppercase(),
                        ]);
                    }
                    index += 3;
                }
                // Bad escape: `%` alone becomes `%25`, the tail rescans.
                _ => {
                    out.extend_from_slice(b"%25");
                    index += 1;
                }
            }
        } else if b < 0x80 && is_path_char(b) {
            out.push(b);
            index += 1;
        } else {
            // Non-table ASCII or non-ASCII UTF-8 byte: percent-encode.
            out.extend_from_slice(&[b'%', HEX[(b >> 4) as usize], HEX[(b & 0xF) as usize]]);
            index += 1;
        }
    }
    // Every byte pushed is ASCII (`%`, hex digits, table chars, decoded
    // unreserved), so the normalized form is UTF-8 by construction.
    Cow::Owned(String::from_utf8(out).expect("normalized bytes are ASCII"))
}

/// The only authority shapes `Uri.parse` still rejects: an authority
/// beginning `[` must close as an IPv6 literal (`[hex:colons.dots]` plus an
/// optional `:port`, nothing else after `]`), and a bare `]` — leading or
/// mid-authority — is a `FormatException` (`file://]/x`, `file://a]/x`,
/// `file://[::1]x/` all throw). Every other byte — spaces, broken `%`
/// escapes, `:`/`@` delimiters — normalizes rather than throwing
/// (`file://a%zz/` and `file://ho st/` both parse), so no character table
/// applies here. The authority's text is never consulted for file targets
/// (`file://host/x` drops it, `package://x/` is unresolvable regardless),
/// so there is nothing else to validate.
fn validate_authority_shape(text: &str) -> Result<(), InvalidUriSyntax> {
    let bytes = text.as_bytes();
    if bytes.first() == Some(&b'[') {
        let close = bytes
            .iter()
            .position(|&b| b == b']')
            .ok_or(InvalidUriSyntax)?;
        // Empty literal (`[]`) throws; a literal without a `:` is no IPv6
        // address either (`[x]`, `[ab]`, `[1.2.3.4]` — upstream rejects all
        // as too-short/invalid IPv6). Full IPv6 grammar is unnecessary: the
        // authority text never participates in file resolution.
        let literal = &bytes[1..close];
        if literal.is_empty()
            || !literal.contains(&b':')
            || !literal
                .iter()
                .all(|&b| b.is_ascii_hexdigit() || matches!(b, b':' | b'.'))
        {
            return Err(InvalidUriSyntax);
        }
        let rest = &bytes[close + 1..];
        if !rest.is_empty() && (rest[0] != b':' || !rest[1..].iter().all(u8::is_ascii_digit)) {
            return Err(InvalidUriSyntax);
        }
        return Ok(());
    }
    if bytes.contains(&b'[') || bytes.contains(&b']') {
        return Err(InvalidUriSyntax);
    }
    Ok(())
}

/// Classifies an absolute (scheme-carrying) [`RawUri`] for landing, consuming
/// the parse — the parts borrow the parsed slices into [`Composed`], never
/// re-joined and re-split.
fn compose_absolute(raw: RawUri<'_>) -> Composed<'_> {
    match (raw.scheme.as_ref(), raw.authority) {
        ("dart-ext", _) => Composed::DartExt,
        ("dart", _) => {
            // The library name is the path's first segment (`core` for both
            // `dart:core` and `dart:core/bool.dart`); an authority form
            // (`dart://x/core`) leaves it empty, which no SDK table lists.
            // One split, no re-join: the pair moves structurally into
            // `land` and on to the SDK's `flat_library_to_path`.
            let (library, part) = split_path_cow(raw.path);
            Composed::Dart { library, part }
        }
        ("package", _) => {
            // A leading `/` (any authority, or `package:///x`) or a missing
            // `/` (no package name) leaves the resolver without a package —
            // `slash < 1` → null in the oracle.
            if raw.path.starts_with('/') || !raw.path.contains('/') {
                Composed::Other
            } else {
                let (package, rel) = split_path_cow(raw.path);
                Composed::Package { package, rel }
            }
        }
        ("file", _) => {
            // The oracle maps file: URIs through the posix `pathContext`
            // (`ResourceUriResolver` → `fileUriToNormalizedPath` →
            // `path-1.9.1 posix.dart:51-56`), which drops the authority and
            // decodes the path — `file://host/x.dart` resolves to `/x.dart`.
            // Only a relative path has no filesystem meaning here.
            if raw.path.starts_with('/') {
                Composed::File(raw.path)
            } else {
                Composed::Other
            }
        }
        _ => Composed::Other,
    }
}

/// Resolves a relative [`RawUri`] against the container's canonical base
/// (`Uri.resolveUri` for `file:`/`package:` bases). Query/fragment were
/// already dropped by the parser.
fn compose_relative<'a>(base: &BaseUri, reference: RawUri<'a>) -> Composed<'a> {
    // An empty reference is the base itself (`resolveUri` empty-path rule) —
    // `import ''` imports the containing file, as in the oracle.
    if reference.path.is_empty() {
        return match base {
            BaseUri::Package { package, path } => Composed::Package {
                package: Cow::Owned(package.clone()),
                rel: Cow::Owned(path.clone()),
            },
            BaseUri::File(path) => Composed::File(Cow::Owned(path.clone())),
        };
    }
    // A reference with its own authority keeps it under the base scheme; on a
    // package base any authority leaves a leading `/` in the path, which the
    // package resolver rejects (`slash < 1`).
    if let Some(authority) = reference.authority {
        return match base {
            BaseUri::File(_) => compose_absolute(RawUri {
                scheme: Cow::Borrowed("file"),
                authority: Some(authority),
                path: reference.path,
                has_fragment: false,
            }),
            BaseUri::Package { .. } => Composed::Other,
        };
    }
    // An absolute-path reference replaces the base path wholesale, keeping
    // its leading `/`: on a package base the result (`package:/x`) parses no
    // package name → unresolvable.
    if reference.path.starts_with('/') {
        return match base {
            BaseUri::File(_) => Composed::File(reference.path),
            BaseUri::Package { .. } => Composed::Other,
        };
    }
    match base {
        BaseUri::Package { package, path } => {
            let merged = remove_dot_segments(merge_paths(path, &reference.path));
            // Escaped the package root: the FIRST segment is exactly `..`
            // (the oracle's `pathSegments.first == '..'` equality check), so
            // no package URI can express it. A segment merely *starting* with
            // `..` (`..foo.dart`) is a legal name, not an escape.
            if merged == ".." || merged.starts_with("../") {
                Composed::Other
            } else {
                Composed::Package {
                    package: Cow::Owned(package.clone()),
                    rel: Cow::Owned(merged),
                }
            }
        }
        BaseUri::File(path) => Composed::File(Cow::Owned(remove_dot_segments(merge_paths(
            path,
            &reference.path,
        )))),
    }
}

/// Scans one SDK file's directive prelude for its `part of` form — the
/// content probe behind [`SdkFileKinds::probe`]. The prelude is the same
/// region the grammar allows directives in: whitespace and comments, `@`
/// annotations, then `library`/`import`/`export`/`part` keywords, each
/// ending at its `;`. The first token outside that set is a declaration and
/// ends the prelude (a library by then). The scanner is byte-level and
/// string-aware (`;` inside a URI literal never terminates a directive);
/// every shape the 3.13.7 SDK tree actually uses — plain and raw strings,
/// single and double quotes, dotted names, license headers — classifies
/// exactly as `FileState._updateKind` would over the parsed unit.
fn scan_sdk_part_of(bytes: &[u8]) -> SdkFileKind {
    let mut scan = Scan { bytes, at: 0 };
    loop {
        scan.skip_trivia();
        let Some(word) = scan.word() else {
            // `@` annotation (metadata precedes a directive) or the prelude's
            // end: a declaration keyword, an identifier, any other token.
            if scan.peek() == Some(b'@') {
                scan.skip_annotation();
                continue;
            }
            return SdkFileKind::Library;
        };
        match word.as_str() {
            // A `library` directive classifies the file as a library outright
            // (`_updateKind`'s first branch) — nothing later can change it.
            "library" => return SdkFileKind::Library,
            "import" | "export" | "part" => {
                if word == "part" {
                    scan.skip_trivia();
                    if scan.peek_word_is(b"of") {
                        scan.at += 2;
                        return scan.part_of_payload();
                    }
                }
                scan.skip_to_semicolon();
            }
            _ => return SdkFileKind::Library,
        }
    }
}

/// The shared cursor of [`scan_sdk_part_of`].
struct Scan<'b> {
    bytes: &'b [u8],
    at: usize,
}

impl<'b> Scan<'b> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    /// Reads an identifier (`[a-zA-Z_$][a-zA-Z0-9_$]*`), advancing past it;
    /// `None` leaves the cursor untouched for the caller's byte dispatch.
    fn word(&mut self) -> Option<String> {
        let first = self.bytes.get(self.at).copied()?;
        if !first.is_ascii_alphabetic() && first != b'_' && first != b'$' {
            return None;
        }
        let start = self.at;
        self.at += 1;
        while let Some(&byte) = self.bytes.get(self.at) {
            if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$' {
                self.at += 1;
            } else {
                break;
            }
        }
        Some(String::from_utf8_lossy(&self.bytes[start..self.at]).into_owned())
    }

    /// Whether the cursor sits on the exact word `word` (a word-boundary
    /// check, no consumption on a miss).
    fn peek_word_is(&self, word: &[u8]) -> bool {
        self.bytes[self.at..].starts_with(word)
            && !self
                .bytes
                .get(self.at + word.len())
                .is_some_and(|next| next.is_ascii_alphanumeric() || *next == b'_' || *next == b'$')
    }

    /// Skips whitespace and both comment forms (block comments do not nest).
    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(byte) if byte.is_ascii_whitespace() => self.at += 1,
                Some(b'/') if self.bytes.get(self.at + 1) == Some(&b'/') => {
                    self.at += 2;
                    while self.peek().is_some_and(|byte| byte != b'\n') {
                        self.at += 1;
                    }
                }
                Some(b'/') if self.bytes.get(self.at + 1) == Some(&b'*') => {
                    self.at += 2;
                    while self.peek().is_some() {
                        if self.bytes[self.at..].starts_with(b"*/") {
                            self.at += 2;
                            break;
                        }
                        self.at += 1;
                    }
                }
                _ => return,
            }
        }
    }

    /// Skips one string literal (raw or plain, either quote) — escape-aware
    /// for plain forms (`\$` never closes), backslash-inert for raw forms.
    /// A lone `r` not before a quote is not a raw prefix and is left in
    /// place (the caller's byte loop resumes from it).
    fn skip_string(&mut self) {
        let mut raw = false;
        if self.peek() == Some(b'r')
            && matches!(self.bytes.get(self.at + 1), Some(b'\'') | Some(b'"'))
        {
            raw = true;
            self.at += 1;
        }
        let Some(quote) = self.peek() else { return };
        if quote != b'\'' && quote != b'"' {
            return;
        }
        self.at += 1;
        while let Some(byte) = self.peek() {
            match byte {
                b'\\' if !raw => self.at = (self.at + 2).min(self.bytes.len()),
                byte if byte == quote => {
                    self.at += 1;
                    return;
                }
                _ => self.at += 1,
            }
        }
    }

    /// Skips a directive's tail up to its `;`, string- and comment-aware.
    fn skip_to_semicolon(&mut self) {
        loop {
            match self.peek() {
                None | Some(b';') => {
                    if self.peek().is_some() {
                        self.at += 1;
                    }
                    return;
                }
                Some(b'/') if self.bytes.get(self.at + 1) == Some(&b'/') => self.skip_trivia(),
                Some(b'/') if self.bytes.get(self.at + 1) == Some(&b'*') => self.skip_trivia(),
                _ if self.at_string() => self.skip_string(),
                _ => self.at += 1,
            }
        }
    }

    /// Skips `@` metadata: `@` qualified-name `[argument-list]` — trivia
    /// allowed at each junction, the parens balanced with string awareness.
    fn skip_annotation(&mut self) {
        self.at += 1; // `@`
        self.skip_trivia();
        while self.word().is_some() {
            if self.peek() != Some(b'.') {
                break;
            }
            self.at += 1;
            self.skip_trivia();
        }
        self.skip_trivia();
        if self.peek() == Some(b'(') {
            let mut depth = 0usize;
            loop {
                match self.peek() {
                    None => return,
                    Some(b'(') => {
                        depth += 1;
                        self.at += 1;
                    }
                    Some(b')') => {
                        depth -= 1;
                        self.at += 1;
                        if depth == 0 {
                            return;
                        }
                    }
                    _ if self.at_string() => self.skip_string(),
                    _ => self.at += 1,
                }
            }
        }
    }

    /// The payload after `part of`: string literal(s) (raw, either quote,
    /// adjacent concatenation) are the URI form; a dotted name is the name
    /// form with its written spelling; anything else is a malformed part-of
    /// the probe declines to honor (the file classifies as a library — the
    /// part-family rows then behave as they would for a parse that never
    /// produced a well-formed part-of directive).
    fn part_of_payload(&mut self) -> SdkFileKind {
        self.skip_trivia();
        if self.at_string() {
            while {
                self.skip_trivia();
                self.at_string()
            } {
                self.skip_string();
            }
            return SdkFileKind::UriPartOf;
        }
        match self.peek() {
            Some(first) if first.is_ascii_alphabetic() || first == b'_' || first == b'$' => {
                let mut name = self.word().expect("peeked an identifier head");
                while self.peek() == Some(b'.') {
                    self.at += 1;
                    self.skip_trivia();
                    match self.word() {
                        Some(component) => {
                            name.push('.');
                            name.push_str(&component);
                        }
                        None => return SdkFileKind::Library,
                    }
                }
                SdkFileKind::NamePartOf(name.into_boxed_str())
            }
            _ => SdkFileKind::Library,
        }
    }

    /// Whether the cursor sits on a string literal (a quote, or `r` directly
    /// before one).
    fn at_string(&self) -> bool {
        match self.peek() {
            Some(b'\'') | Some(b'"') => true,
            Some(b'r') => matches!(self.bytes.get(self.at + 1), Some(b'\'') | Some(b'"')),
            _ => false,
        }
    }
}

/// RFC 3986 §5.2.3 merge (Dart `_mergePaths`): the base path up to and
/// including its final `/`, plus the reference path.
fn merge_paths(base: &str, reference: &str) -> String {
    let base_end = base.rfind('/').map_or(0, |index| index + 1);
    format!("{}{reference}", &base[..base_end])
}

/// RFC 3986 §5.2.4 remove-dot-segments on the percent-encoded path (escapes
/// contain no raw `/`, so segment operations are safe). `..` past the root of
/// an absolute path clamps at the root; on a relative path leading `..`
/// segments survive. Takes the merged path by value: a path without a single
/// `.` (the common shape) is returned as-is, with no second copy.
fn remove_dot_segments(path: String) -> String {
    if !path.contains('.') {
        return path;
    }
    let absolute = path.starts_with('/');
    let segments: Vec<&str> = path.split('/').collect();
    let last = segments.len() - 1;
    let mut output: Vec<&str> = Vec::new();
    let mut trailing = false;
    for (index, segment) in segments.iter().enumerate() {
        match *segment {
            "." => trailing = index == last,
            ".." => {
                if absolute {
                    // Segment 0 is the root marker `""`; never pop it.
                    if output.len() > 1 {
                        output.pop();
                    }
                } else if output.last().copied().is_some_and(|s| s != "..") {
                    output.pop();
                } else {
                    output.push("..");
                }
                trailing = index == last;
            }
            other => output.push(other),
        }
    }
    let mut joined = output.join("/");
    if trailing && !joined.ends_with('/') {
        joined.push('/');
    }
    joined
}

/// Percent-decodes a URI path into UTF-8 text, or `None` when an escape does
/// not decode to valid UTF-8 (`fromUri` throws → resolver null). Borrowed on
/// the escape-free path — the overwhelmingly common shape — so only a `%`
/// forces the owned decode.
fn decode_uri_path(text: &str) -> Option<Cow<'_, str>> {
    if !text.contains('%') {
        return Some(Cow::Borrowed(text));
    }
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = bytes.get(index + 1..index + 3)?;
            let high = (hex[0] as char).to_digit(16)?;
            let low = (hex[1] as char).to_digit(16)?;
            decoded.push(u8::try_from(high * 16 + low).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok().map(Cow::Owned)
}

/// Percent-encodes a filesystem path into a URI path: everything outside the
/// pchar set (and `/`) becomes uppercase `%XX` over UTF-8 bytes.
#[must_use]
pub fn encode_uri_path(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut encoded = String::with_capacity(bytes.len());
    for &b in bytes {
        let plain = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'/' | b'-'
                    | b'.'
                    | b'_'
                    | b'~'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
                    | b':'
                    | b'@'
            );
        if plain {
            encoded.push(b as char);
        } else {
            encoded.push('%');
            encoded.push(
                char::from_digit(u32::from(b >> 4), 16)
                    .unwrap_or('0')
                    .to_ascii_uppercase(),
            );
            encoded.push(
                char::from_digit(u32::from(b & 0xF), 16)
                    .unwrap_or('0')
                    .to_ascii_uppercase(),
            );
        }
    }
    encoded
}

/// Joins a lib-dir path with a `/`-separated decoded relative path.
fn join_rel(dir: &Path, rel: &str) -> PathBuf {
    let mut joined = dir.to_path_buf();
    for segment in rel.split('/') {
        joined.push(segment);
    }
    joined
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    // ---- parse: Dart Uri.parse validity, per sdk/lib/core/uri.dart ----

    #[test]
    fn parses_directive_uri_shapes() {
        let relative = parse_directive_uri("src/a.dart").expect("relative");
        assert_eq!(relative.scheme.as_ref(), "");
        assert_eq!(relative.authority, None);
        assert_eq!(relative.path.as_ref(), "src/a.dart");

        let package = parse_directive_uri("package:p/a/b.dart").expect("package");
        assert_eq!(package.scheme.as_ref(), "package");
        assert_eq!(package.path.as_ref(), "p/a/b.dart");

        let dart = parse_directive_uri("dart:core/bool.dart").expect("dart");
        assert_eq!(dart.scheme.as_ref(), "dart");

        let file = parse_directive_uri("file:///tmp/x.dart").expect("file");
        assert_eq!(file.authority.as_deref(), Some(""));
        assert_eq!(file.path.as_ref(), "/tmp/x.dart");
        assert!(!file.has_fragment);

        let data = parse_directive_uri("data:text/plain,main(){}").expect("data");
        assert_eq!(data.scheme.as_ref(), "data");

        let ext = parse_directive_uri("dart-ext:native.so").expect("dart-ext");
        assert_eq!(ext.scheme.as_ref(), "dart-ext");

        // Scheme case is lowered; escapes and query/fragment split correctly.
        let upper = parse_directive_uri("PACKAGE:p/a.dart").expect("upper");
        assert_eq!(upper.scheme.as_ref(), "package");
        let query = parse_directive_uri("a.dart?x=1#frag").expect("query");
        assert_eq!(query.path.as_ref(), "a.dart");
        assert!(query.has_fragment);
        let escaped = parse_directive_uri("my%20lib/a.dart").expect("escaped");
        assert_eq!(escaped.path.as_ref(), "my%20lib/a.dart");
    }

    /// `Uri.parse` rejects only the scheme grammar and malformed `[` IPv6
    /// authorities — everything else *normalizes*: spaces, delimiters, and
    /// broken `%` escapes become escapes rather than throwing (upstream pin:
    /// `my file.dart` resolves `lib/my file.dart`, `a%zz`→`a%25zz`→file
    /// `a%zz`, `a\b`→`a/b`).
    #[test]
    fn rejects_only_what_uri_parse_throws_on() {
        // Empty scheme / non-alphabetic scheme start / illegal scheme char.
        // A `:` before the first `/` validates its whole prefix as a scheme —
        // `a%b:c` throws too (upstream pins: `a%b:c`, `a b:c`, `:x`, `1a:b`,
        // `::x` all FormatException).
        for uri in ["::x", ":x", "1a:b", "0a:c", "a b:c", "a%b:c", "a b.c:d"] {
            assert!(parse_directive_uri(uri).is_err(), "{uri}");
        }
        // Malformed IPv6 authority shapes (and a stray `]` or `[`).
        for uri in [
            "file://ho[st]/x.dart",
            "package://[::1/x.dart",
            "file://]/x.dart",
            "file://a]/x.dart",
            "file://[x]/y.dart",
            "file://[::1]x/y.dart",
            "file://[]/x.dart",
        ] {
            assert!(parse_directive_uri(uri).is_err(), "{uri}");
        }
    }

    /// `Uri.parse`'s lenient path normalization: non-table bytes escape,
    /// `\` folds to `/`, a `%` not starting an escape becomes `%25`, and an
    /// escape decoding to an unreserved byte decodes (everything else keeps
    /// its escape, uppercased).
    #[test]
    fn parse_normalizes_path_like_uri_parse() {
        let cases: [(&str, &str); 12] = [
            ("my file.dart", "my%20file.dart"),
            ("a<b.dart", "a%3Cb.dart"),
            ("a^b.dart", "a%5Eb.dart"),
            ("a`b.dart", "a%60b.dart"),
            ("a{b}.dart", "a%7Bb%7D.dart"),
            ("a[b].dart", "a%5Bb%5D.dart"),
            ("a\\b.dart", "a/b.dart"),
            ("a%zz.dart", "a%25zz.dart"),
            ("a%.dart", "a%25.dart"),
            ("a%2", "a%252"),
            ("a%41b.dart", "aAb.dart"),
            ("a%2fb.dart", "a%2Fb.dart"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                parse_directive_uri(input).expect(input).path.as_ref(),
                expected,
                "{input}"
            );
        }
        // Lenient authority: escapes and spaces normalize instead of
        // throwing (`file://a%zz/` parses upstream too).
        assert!(parse_directive_uri("file://a%zz/x.dart").is_ok());
        assert!(parse_directive_uri("file://ho st/x.dart").is_ok());
        // A `:` before any `/` is a scheme position; after a `/` it is a
        // path char (`a/b:c` stays relative).
        let nested = parse_directive_uri("a/b:c").expect("colon path");
        assert_eq!(nested.scheme.as_ref(), "");
        assert_eq!(nested.path.as_ref(), "a/b:c");
    }

    #[test]
    fn host_and_empty_authority_are_kept_for_dispatch() {
        let hosted = parse_directive_uri("package://host/a.dart").expect("hosted");
        assert_eq!(hosted.authority.as_deref(), Some("host"));
        assert_eq!(hosted.path.as_ref(), "/a.dart");
        let empty = parse_directive_uri("package:///a.dart").expect("empty");
        assert_eq!(empty.authority.as_deref(), Some(""));
        assert_eq!(empty.path.as_ref(), "/a.dart");
    }

    // ---- resolve: fixture workspace + synthetic SDK ----

    const LIBRARIES_JSON: &str = r#"{
      "vm": {
        "libraries": {
          "core": {"uri": "core/core.dart"},
          "io": {"uri": "io/io.dart"}
        }
      }
    }"#;
    const ALLOWED_EXPERIMENTS_JSON: &str = r#"{
      "version": 1,
      "experimentSets": {"sdkExperiments": []},
      "sdk": {"default": {"experimentSet": "sdkExperiments"}},
      "packages": {}
    }"#;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A throwaway workspace: `app/` holds `.dart_tool` + the config, `pkg_a`
    /// is a workspace package, `hosted` a pub-cache package.
    struct Workspace {
        root: PathBuf,
    }

    impl Workspace {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "dartq-resolution-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).expect("root dir");
            // The identity layer interns existing files by their canonical
            // path (`file_table.rs` `normalize_path`) and the resolver
            // composes from those spellings, so the fixture pins every
            // expectation to the canonical root: on macOS `temp_dir()` is
            // `/var/folders/...` and `/var` is a symlink to `/private/var`.
            let root = root.canonicalize().expect("canonical root");
            let write = |relative: &str, content: &str| {
                let path = root.join(relative);
                fs::create_dir_all(path.parent().expect("parent")).expect("dir");
                fs::write(&path, content).expect("file");
            };
            write("pkg_a/lib/a.dart", "// fixture\n");
            write("pkg_a/lib/sub/b.dart", "// fixture\n");
            write("pkg_a/lib/c.dart", "// fixture\n");
            // Names whose first segment merely begins with `..` are legal
            // files (the oracle pins a `pathSegments.first == '..'`
            // EQUALITY check, not a prefix test).
            write("pkg_a/lib/..foo.dart", "// fixture\n");
            write("pkg_a/lib/..foo/c.dart", "// fixture\n");
            write("pkg_a/lib/..foo/x.dart", "// fixture\n");
            write("app/lib/main.dart", "// fixture\n");
            write("app/lib/util.dart", "// fixture\n");
            // A literal space in the file name: `Uri.parse` escapes the raw
            // spelling `'my file.dart'` to `my%20file.dart`, which decodes
            // back to this file — the lenient parse lands it.
            write("app/lib/my file.dart", "// fixture\n");
            write("app/lib/a%zz.dart", "// fixture\n");
            write("app/lib/a/b.dart", "// fixture\n");
            write("app/bin/tool.dart", "// fixture\n");
            write("hosted/lib/h.dart", "// fixture\n");
            write(
                "app/.dart_tool/package_config.json",
                &format!(
                    // rootUris resolve against the config dir
                    // (`app/.dart_tool`), so `../../pkg_a` is the fixture's
                    // sibling package. `packageUri: "lib/"` is the pub
                    // writer's shape; an omitted field anchors the source
                    // root at the package root itself
                    // (package_config_json.dart:150-152, `packageRoot =
                    // root`), which would put every `package:` target one
                    // level short of these `lib/` files.
                    r#"{{
                      "configVersion": 2,
                      "packages": [
                        {{"name": "pkg_a", "rootUri": "../../pkg_a", "packageUri": "lib/"}},
                        {{"name": "hosted", "rootUri": "file://{}", "packageUri": "lib/"}}
                      ]
                    }}"#,
                    encode_uri_path(&root.join("hosted").to_string_lossy()),
                ),
            );
            Self { root }
        }

        fn config_dir(&self) -> PathBuf {
            self.root.join("app/.dart_tool")
        }

        fn packages(&self) -> PackageTable {
            let json =
                fs::read_to_string(self.config_dir().join("package_config.json")).expect("config");
            let config = PackageConfig::parse(&json).expect("valid config");
            PackageTable::new(&config, &self.config_dir()).expect("indexable config")
        }
    }

    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    /// A synthetic dart SDK exposing `dart:core`/`dart:io` on segment `vm`.
    struct SdkFixture {
        root: PathBuf,
    }

    impl SdkFixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "dartq-resolution-sdk-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&root);
            let write = |relative: &str, content: &str| {
                let path = root.join(relative);
                fs::create_dir_all(path.parent().expect("parent")).expect("dir");
                fs::write(&path, content).expect("file");
            };
            write("version", "3.13.1\n");
            write("lib/libraries.json", LIBRARIES_JSON);
            write(
                "lib/_internal/allowed_experiments.json",
                ALLOWED_EXPERIMENTS_JSON,
            );
            write("lib/core/core.dart", "library core;");
            write("lib/core/bool.dart", "part of core;");
            // The probe fixture set: every part-of shape the 3.13.7 SDK tree
            // uses, plus the library-shaped sub-files (patch libraries) and
            // the malformed tail.
            write(
                "lib/core/int.dart",
                "// License header.\n\npart of \"dart:core\";\n",
            );
            write(
                "lib/core/raw.dart",
                "// License.\nimport 'dart:core';\n\npart of r'dart:core';\n",
            );
            write(
                "lib/core/adjacent.dart",
                "part of 'dart:' 'core';\n",
            );
            write(
                "lib/io/patch.dart",
                "// Patch file for dart:io.\n\nimport 'dart:async';\n\nvoid f() {}\n",
            );
            write(
                "lib/io/broken.dart",
                "// Malformed: the probe declines to honor a part-of with no\n// payload.\npart of ;\n",
            );
            write(
                "lib/io/annotated.dart",
                "@ pragma('vm:entry-point')\npart of io;\n",
            );
            write("lib/io/io.dart", "library io;");
            Self { root }
        }

        fn load(&self) -> SdkSnapshot {
            SdkSnapshot::load(&self.root, None).expect("synthetic SDK loads")
        }
    }

    impl Drop for SdkFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    /// Shared context of one fixture workspace + synthetic SDK.
    struct Fixture {
        workspace: Workspace,
        sdk: SdkFixture,
        packages: PackageTable,
        snapshot: SdkSnapshot,
        /// The declared-variables context, empty (the oracle default);
        /// conditional tests needing defines build their own map and use
        /// [`resolver_with`].
        declared: FxHashMap<String, String>,
    }

    impl Fixture {
        fn new() -> Self {
            let workspace = Workspace::new();
            let sdk = SdkFixture::new();
            let packages = workspace.packages();
            let snapshot = sdk.load();
            Self {
                workspace,
                sdk,
                packages,
                snapshot,
                declared: FxHashMap::default(),
            }
        }
    }

    /// Builds a resolver over the fixture with `declared` as the
    /// conditional-evaluation context, interning into `files` (kept by the
    /// test so fixture reads stay possible while the resolver is alive).
    /// Disk-only existence: no residency beyond the fixture's real files.
    fn resolver_with<'a>(
        fixture: &'a Fixture,
        declared: &'a FxHashMap<String, String>,
        files: &'a mut FileTable,
    ) -> UriResolver<'a> {
        UriResolver::new(
            &fixture.packages,
            &fixture.snapshot,
            declared,
            files,
            &|_| false,
        )
    }

    /// [`resolver_with`] under the oracle-default empty context.
    fn resolver<'a>(fixture: &'a Fixture, files: &'a mut FileTable) -> UriResolver<'a> {
        resolver_with(fixture, &fixture.declared, files)
    }

    /// Interns a fixture-relative path as a container file.
    fn container(fixture: &Fixture, files: &mut FileTable, relative: &str) -> FileId {
        files.intern(&fixture.workspace.root.join(relative))
    }

    fn code_of(resolution: &UriResolution) -> ResolutionCode {
        resolution.code.expect("resolution carries a code")
    }

    #[test]
    fn relative_hit_round_trips_through_file_table() {
        let fixture = Fixture::new();
        let mut files = FileTable::new();
        let container = container(&fixture, &mut files, "app/lib/main.dart");
        let resolution = resolver(&fixture, &mut files).resolve(
            container,
            Some("util.dart"),
            DirectiveKind::ImportExport,
        );
        assert_eq!(
            resolution.uri,
            ResolvedUri::File(fixture.workspace.root.join("app/lib/util.dart"))
        );
        assert_eq!(resolution.code, None);
        let ResolvedTarget::Resolved(id) = resolution.target else {
            panic!("expected a file target");
        };
        assert_eq!(
            files.path(id),
            Some(fixture.workspace.root.join("app/lib/util.dart").as_path())
        );
        // The same file reached through a different spelling shares one id.
        let via_dot = resolver(&fixture, &mut files).resolve(
            container,
            Some("./util.dart"),
            DirectiveKind::ImportExport,
        );
        assert_eq!(via_dot.target, resolution.target);
        // And through a percent escape decoding to the same name.
        let via_escape = resolver(&fixture, &mut files).resolve(
            container,
            Some("uti%6C.dart"),
            DirectiveKind::ImportExport,
        );
        assert_eq!(via_escape.target, resolution.target);
    }

    #[test]
    fn relative_composition_against_file_base() {
        let fixture = Fixture::new();
        let mut files = FileTable::new();
        let container = container(&fixture, &mut files, "app/lib/main.dart");
        let resolution = resolver(&fixture, &mut files).resolve(
            container,
            Some("../bin/tool.dart"),
            DirectiveKind::ImportExport,
        );
        assert_eq!(
            resolution.uri,
            ResolvedUri::File(fixture.workspace.root.join("app/bin/tool.dart"))
        );
        assert_eq!(resolution.code, None);
        // Dot segments collapse lexically before existence is consulted.
        let dotdot = resolver(&fixture, &mut files).resolve(
            container,
            Some("sub/../util.dart"),
            DirectiveKind::ImportExport,
        );
        assert_eq!(
            dotdot.uri,
            ResolvedUri::File(fixture.workspace.root.join("app/lib/util.dart"))
        );
    }

    /// The oracle pin: a container under a package lib dir composes relatives
    /// in package-URI space, so `../c.dart` from `lib/sub/b.dart` selects
    /// `package:pkg_a/c.dart` — `lib/c.dart`, not `pkg_a/c.dart`.
    #[test]
    fn relative_composition_from_package_file_stays_in_lib_space() {
        let fixture = Fixture::new();
        let mut files = FileTable::new();
        let container = container(&fixture, &mut files, "pkg_a/lib/sub/b.dart");
        let resolution = resolver(&fixture, &mut files).resolve(
            container,
            Some("../c.dart"),
            DirectiveKind::ImportExport,
        );
        let ResolvedUri::Package { package, rel, file } = resolution.uri.clone() else {
            panic!("expected a package target");
        };
        assert_eq!(package.as_ref(), "pkg_a");
        assert_eq!(rel, PathBuf::from("c.dart"));
        assert_eq!(file, fixture.workspace.root.join("pkg_a/lib/c.dart"));
        assert_eq!(resolution.code, None);

        // Escaping past the package root is not expressible as a package URI
        // → unresolvable.
        let escape = resolver(&fixture, &mut files).resolve(
            container,
            Some("../../../x.dart"),
            DirectiveKind::ImportExport,
        );
        assert_eq!(
            escape.uri,
            ResolvedUri::Unresolved(UnresolvedKind::NotExist)
        );
        assert_eq!(code_of(&escape), ResolutionCode::UriDoesNotExist);
        assert_eq!(escape.target, ResolvedTarget::Missing);
    }

    /// A first segment merely *beginning* with `..` is a legal name, not an
    /// escape: the oracle checks `pathSegments.first == '..'` (equality), so
    /// `..foo.dart` resolves inside the package instead of reporting a
    /// nonexistent URI.
    #[test]
    fn dotdot_prefixed_segment_is_not_a_package_escape() {
        let fixture = Fixture::new();
        let mut files = FileTable::new();
        // Lib-root container: the lib-relative reference passes through
        // `merge_paths` verbatim (base path has no `/`).
        let root = container(&fixture, &mut files, "pkg_a/lib/a.dart");
        let sibling = resolver(&fixture, &mut files).resolve(
            root,
            Some("..foo.dart"),
            DirectiveKind::ImportExport,
        );
        let ResolvedUri::Package { package, rel, file } = sibling.uri.clone() else {
            panic!("expected a package target");
        };
        assert_eq!(package.as_ref(), "pkg_a");
        assert_eq!(rel, PathBuf::from("..foo.dart"));
        assert_eq!(file, fixture.workspace.root.join("pkg_a/lib/..foo.dart"));
        assert_eq!(sibling.code, None);

        // A container whose own lib-relative path starts with such a
        // segment composes relatives inside it.
        let nested = container(&fixture, &mut files, "pkg_a/lib/..foo/c.dart");
        let resolution = resolver(&fixture, &mut files).resolve(
            nested,
            Some("x.dart"),
            DirectiveKind::ImportExport,
        );
        let ResolvedUri::Package { package, rel, file } = resolution.uri.clone() else {
            panic!("expected a package target");
        };
        assert_eq!(package.as_ref(), "pkg_a");
        assert_eq!(rel, PathBuf::from("..foo/x.dart"));
        assert_eq!(file, fixture.workspace.root.join("pkg_a/lib/..foo/x.dart"));
        assert_eq!(resolution.code, None);

        // True escapes still escape: `../x.dart` from the lib root and a
        // bare `..` both leave the package unresolvably.
        for reference in ["../x.dart", ".."] {
            let escape = resolver(&fixture, &mut files).resolve(
                root,
                Some(reference),
                DirectiveKind::ImportExport,
            );
            assert_eq!(
                escape.uri,
                ResolvedUri::Unresolved(UnresolvedKind::NotExist)
            );
            assert_eq!(code_of(&escape), ResolutionCode::UriDoesNotExist);
            assert_eq!(escape.target, ResolvedTarget::Missing);
        }
    }

    /// `import ''` resolves to the containing file itself (`resolveUri`
    /// empty-reference rule), interning to the container's own id.
    #[test]
    fn empty_uri_self_import() {
        let fixture = Fixture::new();
        let mut files = FileTable::new();
        let container = container(&fixture, &mut files, "app/lib/main.dart");
        let resolution = resolver(&fixture, &mut files).resolve(
            container,
            Some(""),
            DirectiveKind::ImportExport,
        );
        assert_eq!(resolution.target, ResolvedTarget::Resolved(container));
        assert_eq!(resolution.code, None);
    }

    #[test]
    fn package_targets_map_through_lib_dirs() {
        let fixture = Fixture::new();
        let mut files = FileTable::new();
        // The container lives in the resolver's own table: ids are
        // table-local, and the relative face below composes from the
        // container's interned path.
        let container = container(&fixture, &mut files, "app/lib/main.dart");
        let mut resolver = resolver(&fixture, &mut files);

        let hit = resolver.resolve(
            container,
            Some("package:pkg_a/a.dart"),
            DirectiveKind::ImportExport,
        );
        assert_eq!(
            hit.uri,
            ResolvedUri::Package {
                package: "pkg_a".into(),
                rel: PathBuf::from("a.dart"),
                file: fixture.workspace.root.join("pkg_a/lib/a.dart"),
            }
        );
        assert_eq!(hit.code, None);

        let sub = resolver.resolve(
            container,
            Some("package:pkg_a/sub/b.dart"),
            DirectiveKind::ImportExport,
        );
        assert_eq!(
            sub.uri,
            ResolvedUri::Package {
                package: "pkg_a".into(),
                rel: PathBuf::from("sub/b.dart"),
                file: fixture.workspace.root.join("pkg_a/lib/sub/b.dart"),
            }
        );

        // A package from an absolute hosted rootUri resolves identically.
        let hosted = resolver.resolve(
            container,
            Some("package:hosted/h.dart"),
            DirectiveKind::ImportExport,
        );
        assert_eq!(
            hosted.uri,
            ResolvedUri::Package {
                package: "hosted".into(),
                rel: PathBuf::from("h.dart"),
                file: fixture.workspace.root.join("hosted/lib/h.dart"),
            }
        );

        // One target through both faces shares one FileId: relative
        // `../../pkg_a/lib/a.dart` and `package:pkg_a/a.dart`.
        let via_relative = resolver.resolve(
            container,
            Some("../../pkg_a/lib/a.dart"),
            DirectiveKind::ImportExport,
        );
        assert_eq!(via_relative.target, hit.target);
    }

    #[test]
    fn package_failures_carry_oracle_codes() {
        let fixture = Fixture::new();
        let mut files = FileTable::new();
        let container = container(&fixture, &mut files, "app/lib/main.dart");
        let mut files = FileTable::new();
        let mut resolver = resolver(&fixture, &mut files);

        // Unknown package: no target at all.
        let unknown = resolver.resolve(
            container,
            Some("package:missing/a.dart"),
            DirectiveKind::ImportExport,
        );
        assert_eq!(
            unknown.uri,
            ResolvedUri::Unresolved(UnresolvedKind::NotExist)
        );
        assert_eq!(code_of(&unknown), ResolutionCode::UriDoesNotExist);
        assert_eq!(unknown.target, ResolvedTarget::Missing);

        // No package name (`package:x`, `package:`).
        for uri in ["package:pkg_a", "package:"] {
            let none = resolver.resolve(container, Some(uri), DirectiveKind::ImportExport);
            assert_eq!(
                none.uri,
                ResolvedUri::Unresolved(UnresolvedKind::NotExist),
                "{uri}"
            );
            assert_eq!(code_of(&none), ResolutionCode::UriDoesNotExist, "{uri}");
        }

        // Authority form: the path starts with `/`, no package parseable.
        let hosted = resolver.resolve(
            container,
            Some("package://host/a.dart"),
            DirectiveKind::ImportExport,
        );
        assert_eq!(
            hosted.uri,
            ResolvedUri::Unresolved(UnresolvedKind::NotExist)
        );

        // Known package, missing file: target exists (the lib path), the
        // code carries the missing-ness.
        let missing = resolver.resolve(
            container,
            Some("package:pkg_a/gone.dart"),
            DirectiveKind::ImportExport,
        );
        assert!(matches!(missing.uri, ResolvedUri::Package { .. }));
        assert!(matches!(missing.target, ResolvedTarget::Resolved(_)));
        assert_eq!(code_of(&missing), ResolutionCode::UriDoesNotExist);

        // Missing with a generated suffix: hasn't-been-generated instead.
        let generated = resolver.resolve(
            container,
            Some("package:pkg_a/made.g.dart"),
            DirectiveKind::ImportExport,
        );
        assert!(matches!(generated.uri, ResolvedUri::Package { .. }));
        assert_eq!(code_of(&generated), ResolutionCode::UriHasNotBeenGenerated);
    }

    /// The generated-file face: a missing lib target falls back to
    /// `.dart_tool/build/generated/<pkg>/lib/<raw rel>` when that exists.
    #[test]
    fn package_generated_fallback_used_when_present() {
        let fixture = Fixture::new();
        let mut files = FileTable::new();
        let built = fixture
            .workspace
            .root
            .join("app/.dart_tool/build/generated/pkg_a/lib/gen.g.dart");
        fs::create_dir_all(built.parent().expect("parent")).expect("built dir");
        fs::write(&built, "// generated\n").expect("built file");

        let container = container(&fixture, &mut files, "app/lib/main.dart");
        let resolution = resolver(&fixture, &mut files).resolve(
            container,
            Some("package:pkg_a/gen.g.dart"),
            DirectiveKind::ImportExport,
        );
        let ResolvedUri::Package { file, .. } = resolution.uri else {
            panic!("expected a package target");
        };
        assert_eq!(file, built);
        assert_eq!(resolution.code, None);
    }

    #[test]
    fn dart_targets_map_through_sdk_table() {
        let fixture = Fixture::new();
        let mut files = FileTable::new();
        let container = container(&fixture, &mut files, "app/lib/main.dart");
        let mut files = FileTable::new();
        let mut resolver = resolver(&fixture, &mut files);

        // The product canonicalizes the SDK root on load; on macOS
        // `temp_dir()` spells it through the /var symlink while the
        // canonical form carries the /private prefix (identity on Linux),
        // so the expected joins must come from the canonicalized root.
        let sdk_root = fixture
            .sdk
            .root
            .canonicalize()
            .expect("canonical SDK root");

        let core = resolver.resolve(container, Some("dart:core"), DirectiveKind::ImportExport);
        assert_eq!(
            core.uri,
            ResolvedUri::Dart {
                library: "core".into(),
                file: sdk_root.join("lib/core/core.dart"),
            }
        );
        assert_eq!(core.target, ResolvedTarget::External);
        assert_eq!(core.code, None);

        // `dart:core/bool.dart` keeps the library name and joins the part.
        let part = resolver.resolve(
            container,
            Some("dart:core/bool.dart"),
            DirectiveKind::ImportExport,
        );
        let ResolvedUri::Dart { library, file } = part.uri else {
            panic!("expected a dart target");
        };
        assert_eq!(library.as_ref(), "core");
        assert_eq!(file, sdk_root.join("lib/core/bool.dart"));
        assert_eq!(part.target, ResolvedTarget::External);
        assert_eq!(part.code, None);

        // Existence is the SDK's flat face, not the vm segment's table: the
        // synthetic SDK lists only core/io on `vm`, and `dart:mirrors`
        // still resolves through the generated flat table. An ENTRY URI
        // resolves on table membership alone — `internalMapDartUri`
        // (dart/sdk/sdk.dart:90-130) maps known library names
        // unconditionally and never consults the file system, so a
        // mapped-but-missing source (the names-only-face posture) stays
        // silent; the `importedFile.exists` check belongs to part paths
        // only, covered by `dart:io/nope.dart` below.
        let mirrors =
            resolver.resolve(container, Some("dart:mirrors"), DirectiveKind::ImportExport);
        assert_eq!(
            mirrors.uri,
            ResolvedUri::Dart {
                library: "mirrors".into(),
                file: sdk_root.join("lib/mirrors/mirrors.dart"),
            }
        );
        assert_eq!(mirrors.target, ResolvedTarget::External);
        assert_eq!(mirrors.code, None);

        // A known library with a missing part file — `dart:io/nope.dart` —
        // codes the same way; a generated suffix carries the
        // hasn't-been-generated code (the same `isGenerated` suffix table,
        // file_paths.dart:89-105).
        let missing_part = resolver.resolve(
            container,
            Some("dart:io/nope.dart"),
            DirectiveKind::ImportExport,
        );
        assert_eq!(code_of(&missing_part), ResolutionCode::UriDoesNotExist);
        assert_eq!(missing_part.target, ResolvedTarget::External);
        let generated_part = resolver.resolve(
            container,
            Some("dart:io/nope.g.dart"),
            DirectiveKind::ImportExport,
        );
        assert_eq!(
            code_of(&generated_part),
            ResolutionCode::UriHasNotBeenGenerated
        );

        // `web_sql` is on disk in the SDK tree but tabulated by neither the
        // oracle's flat table nor dartq's: source null —
        // `uri_does_not_exist` per the URI-layer contract.
        let unknown =
            resolver.resolve(container, Some("dart:web_sql"), DirectiveKind::ImportExport);
        assert_eq!(
            unknown.uri,
            ResolvedUri::Unresolved(UnresolvedKind::NotExist)
        );
        assert_eq!(code_of(&unknown), ResolutionCode::UriDoesNotExist);
        assert_eq!(unknown.target, ResolvedTarget::Missing);
    }

    /// The external kind probe over the shapes the 3.13.7 SDK tree uses
    /// (`FileState._updateKind` decided from content, pinned against
    /// `dart analyze` 3.13.7: `dart:math` is a library, `dart:async/
    /// future.dart` is `part of "dart:async"`, `dart:developer/
    /// creation_tracking.dart` is `part of dart.developer;`). The scanner
    /// pins: license comments, an import before the part-of, plain/raw/
    /// adjacent strings, dotted names, annotations, patch-shaped sub-files
    /// with no part-of, malformed payloads, and the flat-table miss.
    #[test]
    fn sdk_file_kinds_probe_the_part_of_form() {
        let fixture = Fixture::new();
        let snapshot = fixture.sdk.load();
        let mut probe = SdkFileKinds::default();
        // Entry libraries.
        assert_eq!(probe.probe(&snapshot, "core", ""), SdkFileKind::Library);
        assert_eq!(probe.probe(&snapshot, "io", ""), SdkFileKind::Library);
        // Name-form part (single component, as `lib/core/bool.dart`).
        assert_eq!(
            probe.probe(&snapshot, "core", "bool.dart"),
            SdkFileKind::NamePartOf("core".into())
        );
        // URI form: plain double-quoted behind a license header.
        assert_eq!(
            probe.probe(&snapshot, "core", "int.dart"),
            SdkFileKind::UriPartOf
        );
        // URI form: raw string behind an import directive.
        assert_eq!(
            probe.probe(&snapshot, "core", "raw.dart"),
            SdkFileKind::UriPartOf
        );
        // URI form: adjacent string concatenation.
        assert_eq!(
            probe.probe(&snapshot, "core", "adjacent.dart"),
            SdkFileKind::UriPartOf
        );
        // A patch-shaped sub-file: imports and declarations, no part-of —
        // a library, the part_of_non_part face of external includes.
        assert_eq!(
            probe.probe(&snapshot, "io", "patch.dart"),
            SdkFileKind::Library
        );
        // A malformed part-of payload declines to a library.
        assert_eq!(
            probe.probe(&snapshot, "io", "broken.dart"),
            SdkFileKind::Library
        );
        // Annotation metadata before the directive.
        assert_eq!(
            probe.probe(&snapshot, "io", "annotated.dart"),
            SdkFileKind::NamePartOf("io".into())
        );
        // A name outside the flat table and a file that does not exist:
        // the inert library default (the URI layer already coded the
        // target missing).
        assert_eq!(probe.probe(&snapshot, "web_sql", ""), SdkFileKind::Library);
        assert_eq!(
            probe.probe(&snapshot, "io", "nope.dart"),
            SdkFileKind::Library
        );
    }

    /// The canonical `dart:` spelling — the `includedFile.uriStr` the
    /// oracle carries into the external part-family rows.
    #[test]
    fn canonical_dart_uri_spells_library_and_part() {
        assert_eq!(canonical_dart_uri("math", "").as_ref(), "dart:math");
        assert_eq!(
            canonical_dart_uri("async", "future.dart").as_ref(),
            "dart:async/future.dart"
        );
    }

    #[test]
    fn file_scheme_and_unsupported_schemes() {
        let fixture = Fixture::new();
        let mut files = FileTable::new();
        let container = container(&fixture, &mut files, "app/lib/main.dart");
        let mut resolver = resolver(&fixture, &mut files);

        let file = resolver.resolve(
            container,
            Some(&format!(
                "file://{}",
                encode_uri_path(
                    &fixture
                        .workspace
                        .root
                        .join("app/lib/util.dart")
                        .to_string_lossy()
                )
            )),
            DirectiveKind::ImportExport,
        );
        assert_eq!(
            file.uri,
            ResolvedUri::File(fixture.workspace.root.join("app/lib/util.dart"))
        );
        assert_eq!(file.code, None);

        // A missing file: URI target with the does-not-exist code.
        let missing = resolver.resolve(
            container,
            Some("file:///definitely/not/there.dart"),
            DirectiveKind::ImportExport,
        );
        assert!(matches!(missing.uri, ResolvedUri::File(_)));
        assert!(matches!(missing.target, ResolvedTarget::Resolved(_)));
        assert_eq!(code_of(&missing), ResolutionCode::UriDoesNotExist);

        // A non-empty authority is dropped, not rejected: the oracle resolves
        // through posix `fromUri`, which reads only the path —
        // `file://host/x.dart` lands on `/x.dart` (missing on disk → the
        // does-not-exist code on a live file target).
        let hosted = resolver.resolve(
            container,
            Some("file://host/x.dart"),
            DirectiveKind::ImportExport,
        );
        assert_eq!(hosted.uri, ResolvedUri::File(PathBuf::from("/x.dart")));
        assert!(matches!(hosted.target, ResolvedTarget::Resolved(_)));
        assert_eq!(code_of(&hosted), ResolutionCode::UriDoesNotExist);

        // Unsupported schemes (data:, http:) parse but resolve to nothing.
        for uri in ["data:text/plain,main(){}", "http://x/y.dart"] {
            let none = resolver.resolve(container, Some(uri), DirectiveKind::ImportExport);
            assert_eq!(
                none.uri,
                ResolvedUri::Unresolved(UnresolvedKind::NotExist),
                "{uri}"
            );
            assert_eq!(code_of(&none), ResolutionCode::UriDoesNotExist, "{uri}");
            assert_eq!(none.target, ResolvedTarget::Missing, "{uri}");
        }

        // dart-ext: carries its own frozen code.
        let ext = resolver.resolve(
            container,
            Some("dart-ext:native.so"),
            DirectiveKind::ImportExport,
        );
        assert_eq!(ext.uri, ResolvedUri::Unresolved(UnresolvedKind::DartExt));
        assert_eq!(code_of(&ext), ResolutionCode::UseOfNativeExtension);
    }

    /// The `data:` acceptance boundary mirrors `UriData._parse`'s
    /// `FormatException` faces (uri.dart:4254-4313) — bad MIME shapes, a
    /// parameter section without `=`, a `base64` marker not immediately
    /// before the comma, and the `base64.normalize` payload rules
    /// (padding/length variants, a decoded `%`). Each lands as the crate's
    /// typed `invalid_uri` face through the full resolver pipeline.
    #[test]
    fn data_uri_rejection_boundary() {
        let fixture = Fixture::new();
        let mut files = FileTable::new();
        let container = container(&fixture, &mut files, "app/lib/main.dart");
        let cases: [(&str, &str); 7] = [
            ("non-empty MIME type without a `/`", "data:x"),
            ("second `/` in the MIME type", "data:a/b/c,payload"),
            ("parameter section without `=`", "data:a/b;;"),
            (
                "base64 marker not immediately before the comma",
                "data:a/b;base64;x=1,QQ",
            ),
            (
                "padded payload not closing a 4-multiple",
                "data:a/b;base64,QQ=",
            ),
            ("unpadded payload length 1 mod 4", "data:a/b;base64,AAAAA"),
            ("decoded `%` inside base64", "data:a/b;base64,%25"),
        ];
        for (why, uri) in cases {
            let rejected = resolver(&fixture, &mut files).resolve(
                container,
                Some(uri),
                DirectiveKind::ImportExport,
            );
            assert_eq!(
                rejected.uri,
                ResolvedUri::Unresolved(UnresolvedKind::BadUri),
                "{why}: {uri}"
            );
            assert_eq!(
                code_of(&rejected),
                ResolutionCode::InvalidUri,
                "{why}: {uri}"
            );
            assert_eq!(rejected.target, ResolvedTarget::Missing, "{why}: {uri}");
        }
    }

    #[test]
    fn no_string_and_bad_syntax_report_their_frozen_codes() {
        let fixture = Fixture::new();
        let mut files = FileTable::new();
        let container = container(&fixture, &mut files, "app/lib/main.dart");
        let mut files = FileTable::new();
        let mut resolver = resolver(&fixture, &mut files);

        // Interpolation: the parser saw no string to resolve.
        let interpolated = resolver.resolve(container, None, DirectiveKind::ImportExport);
        assert_eq!(
            interpolated.uri,
            ResolvedUri::Unresolved(UnresolvedKind::BadUri)
        );
        assert_eq!(code_of(&interpolated), ResolutionCode::UriWithInterpolation);
        assert_eq!(interpolated.target, ResolvedTarget::Missing);

        // Unparseable syntax.
        let bad = resolver.resolve(container, Some("a b:c"), DirectiveKind::ImportExport);
        assert_eq!(bad.uri, ResolvedUri::Unresolved(UnresolvedKind::BadUri));
        assert_eq!(code_of(&bad), ResolutionCode::InvalidUri);
        assert_eq!(bad.target, ResolvedTarget::Missing);

        // A broken escape inside an authority no longer throws: `Uri.parse`
        // normalizes it to `%25` (`file://a%zz/` parses upstream), so the
        // file: URI lands on `/x.dart` and reports `uri_does_not_exist`,
        // not `invalid_uri`.
        let lenient_authority = resolver.resolve(
            container,
            Some("file://a%zz/x.dart"),
            DirectiveKind::ImportExport,
        );
        assert_eq!(
            lenient_authority.uri,
            ResolvedUri::File(PathBuf::from("/x.dart"))
        );
        assert_eq!(code_of(&lenient_authority), ResolutionCode::UriDoesNotExist);

        // A malformed IPv6 authority is the residual `invalid_uri` shape.
        let bad_authority = resolver.resolve(
            container,
            Some("file://[::1/x.dart"),
            DirectiveKind::ImportExport,
        );
        assert_eq!(
            bad_authority.uri,
            ResolvedUri::Unresolved(UnresolvedKind::BadUri)
        );
        assert_eq!(code_of(&bad_authority), ResolutionCode::InvalidUri);
        assert_eq!(bad_authority.target, ResolvedTarget::Missing);
    }

    /// The `@docImport` diagnostic face (library_analyzer.dart:699-719): every
    /// missing-target outcome — unresolvable, plain missing, generated-suffix
    /// missing — reports `uri_does_not_exist_in_doc_import` where an
    /// import/export reports the compile-time codes; hits and directive-shape
    /// errors keep their codes.
    #[test]
    fn doc_import_swaps_missing_target_codes() {
        let fixture = Fixture::new();
        let mut files = FileTable::new();
        let container = container(&fixture, &mut files, "app/lib/main.dart");
        let mut resolver = resolver(&fixture, &mut files);

        // Known package, missing file: the target stays (the lib path), the
        // code carries the missing-ness in the doc-import voice.
        let missing = resolver.resolve(
            container,
            Some("package:pkg_a/gone.dart"),
            DirectiveKind::DocImport,
        );
        assert!(matches!(missing.target, ResolvedTarget::Resolved(_)));
        assert_eq!(
            code_of(&missing),
            ResolutionCode::UriDoesNotExistInDocImport
        );

        // The generated-suffix face collapses into the warning too (the
        // oracle's second isDocImport branch, library_analyzer.dart:711-719).
        let generated = resolver.resolve(
            container,
            Some("package:pkg_a/made.g.dart"),
            DirectiveKind::DocImport,
        );
        assert_eq!(
            code_of(&generated),
            ResolutionCode::UriDoesNotExistInDocImport
        );

        // Unresolvable package: importedSource == null in the oracle — same
        // swap, and no target at all.
        let unknown = resolver.resolve(
            container,
            Some("package:missing/a.dart"),
            DirectiveKind::DocImport,
        );
        assert_eq!(unknown.target, ResolvedTarget::Missing);
        assert_eq!(
            code_of(&unknown),
            ResolutionCode::UriDoesNotExistInDocImport
        );

        // A hit stays clean, and dart-ext: keeps its own frozen code.
        let hit = resolver.resolve(
            container,
            Some("package:pkg_a/a.dart"),
            DirectiveKind::DocImport,
        );
        assert_eq!(hit.code, None);
        let ext = resolver.resolve(
            container,
            Some("dart-ext:native.so"),
            DirectiveKind::DocImport,
        );
        assert_eq!(code_of(&ext), ResolutionCode::UseOfNativeExtension);

        // Directive-shape errors are kind-independent (the parser reports
        // them before any target lookup).
        let interpolated = resolver.resolve(container, None, DirectiveKind::DocImport);
        assert_eq!(code_of(&interpolated), ResolutionCode::UriWithInterpolation);
    }

    /// The `part of` URI face (`file_state.dart:938-953`): the oracle binds
    /// the part to a library through `PartOfUriKnownFileKind`/`UnknownFile-
    /// Kind` and reports NO URI diagnostic for it — a missing target is an
    /// `UnknownFileKind`, not a `uri_does_not_exist`; a broken or
    /// interpolated string resolves the same way (`_fileForRelativeUri`
    /// returns null, `isPartOf` is false). The target still resolves for
    /// the part-of graph, so the face differs from `ImportExport` only on
    /// [`UriResolution::code`].
    #[test]
    fn part_of_face_never_reports_uri_codes() {
        let fixture = Fixture::new();
        let mut files = FileTable::new();
        let container = container(&fixture, &mut files, "app/lib/main.dart");
        let mut resolver = resolver(&fixture, &mut files);

        // Missing file: the ImportExport face reports `uri_does_not_exist`;
        // the part-of face reports nothing while still interning the target
        // (the library-attachment check needs the resolved path).
        let missing = resolver.resolve(container, Some("nonexistent.dart"), DirectiveKind::PartOf);
        assert_eq!(missing.code, None);
        assert!(matches!(missing.target, ResolvedTarget::Resolved(_)));

        // An existing file resolves silently too.
        let hit = resolver.resolve(container, Some("util.dart"), DirectiveKind::PartOf);
        assert_eq!(hit.code, None);
        assert!(matches!(hit.target, ResolvedTarget::Resolved(_)));

        // Unresolvable, invalid, and interpolated spellings all stay silent:
        // the oracle's unknown-kind path emits nothing in every case.
        for uri in [
            Some("package:missing/a.dart"),
            Some("dart:nolib"),
            Some("a b:c"),
            None,
        ] {
            let silent = resolver.resolve(container, uri, DirectiveKind::PartOf);
            assert_eq!(silent.code, None, "{uri:?}");
        }
    }

    /// A `#fragment` on a directive URI is unresolvable upstream no matter
    /// what the path names — `import 'q.dart#f'` reports
    /// `uri_does_not_exist` even with `lib/q.dart` on disk, and the same
    /// holds for `package:`, `file:`, `dart:`, `part`, and `export` faces
    /// (verified against the oracle pin on all of them). The query, by
    /// contrast, is ignored: `'q.dart?x=1'` resolves the file.
    #[test]
    fn fragment_makes_any_uri_unresolvable() {
        let fixture = Fixture::new();
        let mut files = FileTable::new();
        let container = container(&fixture, &mut files, "app/lib/main.dart");
        let mut resolver = resolver(&fixture, &mut files);

        for uri in [
            "util.dart#f",
            "package:pkg_a/a.dart#f",
            "file:///nonexistent/x.dart#f",
            "dart:core#f",
            "util.dart#",
        ] {
            let resolution = resolver.resolve(container, Some(uri), DirectiveKind::ImportExport);
            assert_eq!(
                resolution.uri,
                ResolvedUri::Unresolved(UnresolvedKind::NotExist),
                "{uri}"
            );
            assert_eq!(
                code_of(&resolution),
                ResolutionCode::UriDoesNotExist,
                "{uri}"
            );
            assert_eq!(resolution.target, ResolvedTarget::Missing, "{uri}");
        }

        // A query is dropped before the file lookup: the target exists.
        let queried = resolver.resolve(
            container,
            Some("util.dart?x=1"),
            DirectiveKind::ImportExport,
        );
        assert_eq!(
            queried.uri,
            ResolvedUri::File(fixture.workspace.root.join("app/lib/util.dart"))
        );
        assert_eq!(queried.code, None);
    }

    /// `Uri.parse`'s lenient path normalization reaches the file lookup:
    /// `'my file.dart'` escapes to `my%20file.dart` and lands the literal
    /// file; `'a%zz.dart'` escapes the bare `%` to `%25` and lands
    /// `a%zz.dart`; `'a\\b.dart'` folds to `a/b.dart`. Misses report
    /// `uri_does_not_exist` — never `invalid_uri`.
    #[test]
    fn lenient_path_spellings_resolve_to_literal_files() {
        let fixture = Fixture::new();
        let mut files = FileTable::new();
        let container = container(&fixture, &mut files, "app/lib/main.dart");
        let mut resolver = resolver(&fixture, &mut files);

        for (uri, expected) in [
            ("my file.dart", "app/lib/my file.dart"),
            ("my%20file.dart", "app/lib/my file.dart"),
            ("a%zz.dart", "app/lib/a%zz.dart"),
            ("a\\b.dart", "app/lib/a/b.dart"),
        ] {
            let resolution = resolver.resolve(container, Some(uri), DirectiveKind::ImportExport);
            assert_eq!(
                resolution.uri,
                ResolvedUri::File(fixture.workspace.root.join(expected)),
                "{uri}"
            );
            assert_eq!(resolution.code, None, "{uri}");
        }

        // The same spellings on a missing file report `uri_does_not_exist`
        // — the oracle's exact outcome for `'[invalid uri]'`.
        for uri in ["gone file.dart", "gone%zz.dart", "[invalid uri]"] {
            let resolution = resolver.resolve(container, Some(uri), DirectiveKind::ImportExport);
            assert_eq!(
                code_of(&resolution),
                ResolutionCode::UriDoesNotExist,
                "{uri}"
            );
        }
    }

    #[test]
    fn canonical_uri_prefers_package_space() {
        let fixture = Fixture::new();
        let mut files = FileTable::new();
        let mut resolver = resolver(&fixture, &mut files);

        assert_eq!(
            resolver.canonical_uri(&fixture.workspace.root.join("pkg_a/lib/sub/b.dart")),
            "package:pkg_a/sub/b.dart"
        );
        assert_eq!(
            resolver.canonical_uri(&fixture.workspace.root.join("app/lib/main.dart")),
            format!(
                "file://{}",
                encode_uri_path(
                    &fixture
                        .workspace
                        .root
                        .join("app/lib/main.dart")
                        .to_string_lossy()
                )
            )
        );
    }

    #[test]
    fn broken_root_uri_fails_loudly() {
        let config = PackageConfig::parse(
            r#"{"configVersion": 2, "packages": [{"name": "a", "rootUri": "http://x/y"}]}"#,
        )
        .expect("parses");
        let error = PackageTable::new(&config, Path::new("/p/.dart_tool"))
            .expect_err("unsupported scheme is a hard error");
        assert!(
            error.to_string().contains("unsupported rootUri scheme"),
            "unexpected error: {error}"
        );
    }

    /// The shared dartq-pub face applies to this table too: a repeated
    /// name floors the WHOLE config at parse (dartq-pub `config.rs`
    /// `seen_names` — the loader's `_validatePackages` throws through the
    /// analyzer's default handler and the catch degrades to a silent
    /// `Packages.empty`; mixed-entry probe #2434), so `PackageTable::new`
    /// never sees one and answers nothing for either entry.
    #[test]
    fn duplicate_names_floor_the_whole_config() {
        let error = PackageConfig::parse(
            r#"{"configVersion": 2, "packages": [
                {"name": "a", "rootUri": "../x"},
                {"name": "a", "rootUri": "../y"}]}"#,
        )
        .expect_err("duplicate names floor the config");
        assert!(
            error.to_string().contains("duplicate package name `a`"),
            "unexpected error: {error}"
        );
    }

    // ---- resolve_conditional: configurable directives under the oracle
    // declared-variables pin (conditional.rs; the empty context is the
    // no-`-D` oracle truth) ----

    /// The differential anchor under the empty context:
    /// `import 'a.dart' if (dart.library.io) 'b.dart';` selects the PRIMARY
    /// `a.dart` — declared variables carry no `dart.library.*` rows, so the
    /// condition misses (`file_state.dart:738-764`; the CFE's
    /// target-availability face would pick the branch — the documented
    /// divergence, step 6). The missing branch candidate is never read: no
    /// diagnostic fires.
    #[test]
    fn conditional_primary_wins_under_empty_context() {
        let fixture = Fixture::new();
        let mut files = FileTable::new();
        let container = container(&fixture, &mut files, "app/lib/main.dart");
        let configs = [Configuration::new(
            "dart.library.io",
            None,
            "absent_branch.dart",
        )];
        let resolution = resolver(&fixture, &mut files).resolve_conditional(
            container,
            Some("util.dart"),
            &configs,
            DirectiveKind::ImportExport,
        );
        assert_eq!(
            resolution.uri,
            ResolvedUri::File(fixture.workspace.root.join("app/lib/util.dart"))
        );
        assert_eq!(resolution.code, None);
    }

    /// A declared `== 'value'` match selects its clause: exact string
    /// equality, first match in source order — an earlier clause with the
    /// right name but the wrong value never shadows a later match
    /// (`selectedConfigurationUri ??=`, step 3).
    #[test]
    fn conditional_declared_match_selects_first_matching_clause() {
        let fixture = Fixture::new();
        let mut files = FileTable::new();
        let container = container(&fixture, &mut files, "app/lib/main.dart");
        let declared = FxHashMap::from_iter([("dart.library.io".to_string(), "true".to_string())]);
        let configs = [
            Configuration::new("dart.library.io", Some("false"), "absent_branch.dart"),
            Configuration::new("dart.library.io", Some("true"), "util.dart"),
        ];
        let resolution = resolver_with(&fixture, &declared, &mut files).resolve_conditional(
            container,
            Some("primary.dart"),
            &configs,
            DirectiveKind::ImportExport,
        );
        assert_eq!(
            resolution.uri,
            ResolvedUri::File(fixture.workspace.root.join("app/lib/util.dart"))
        );
        assert_eq!(resolution.code, None);
    }

    /// No match under a populated context falls back to the primary URI —
    /// the oracle fallback (`file_state.dart:759`).
    #[test]
    fn conditional_without_match_falls_back_to_primary() {
        let fixture = Fixture::new();
        let mut files = FileTable::new();
        let container = container(&fixture, &mut files, "app/lib/main.dart");
        let declared = FxHashMap::from_iter([("dart.library.io".to_string(), "true".to_string())]);
        let configs = [Configuration::new(
            "dart.library.io",
            Some("false"),
            "absent_branch.dart",
        )];
        let resolution = resolver_with(&fixture, &declared, &mut files).resolve_conditional(
            container,
            Some("util.dart"),
            &configs,
            DirectiveKind::ImportExport,
        );
        assert_eq!(
            resolution.uri,
            ResolvedUri::File(fixture.workspace.root.join("app/lib/util.dart"))
        );
        assert_eq!(resolution.code, None);
    }

    /// Only the selected candidate is diagnosed: the io-matched
    /// `absent.dart` reports `uri_does_not_exist` on the file projection
    /// while the unmatched-but-also-missing html branch is never read
    /// (`library_analyzer.dart:698-717` reports `state.selectedUri` alone).
    #[test]
    fn conditional_reports_missing_selected_candidate_only() {
        let fixture = Fixture::new();
        let mut files = FileTable::new();
        let container = container(&fixture, &mut files, "app/lib/main.dart");
        let declared = FxHashMap::from_iter([("dart.library.io".to_string(), "true".to_string())]);
        let configs = [
            Configuration::new("dart.library.html", None, "absent_branch.dart"),
            Configuration::new("dart.library.io", None, "absent.dart"),
        ];
        let resolution = resolver_with(&fixture, &declared, &mut files).resolve_conditional(
            container,
            Some("util.dart"),
            &configs,
            DirectiveKind::ImportExport,
        );
        assert_eq!(
            resolution.uri,
            ResolvedUri::File(fixture.workspace.root.join("app/lib/absent.dart"))
        );
        assert_eq!(code_of(&resolution), ResolutionCode::UriDoesNotExist);
        let ResolvedTarget::Resolved(id) = resolution.target else {
            panic!("a missing selected branch keeps its file target");
        };
        assert_eq!(
            files.path(id),
            Some(fixture.workspace.root.join("app/lib/absent.dart").as_path())
        );
    }

    /// Export directives select and resolve identically
    /// (`parseConditionalUriStar` serves both grammars).
    #[test]
    fn conditional_export_parity() {
        let fixture = Fixture::new();
        let mut files = FileTable::new();
        let container = container(&fixture, &mut files, "app/lib/main.dart");
        let declared = FxHashMap::from_iter([("dart.library.io".to_string(), "true".to_string())]);
        let configs = [Configuration::new("dart.library.io", None, "util.dart")];
        let resolution = resolver_with(&fixture, &declared, &mut files).resolve_conditional(
            container,
            Some("primary.dart"),
            &configs,
            DirectiveKind::ImportExport,
        );
        assert_eq!(
            resolution.uri,
            ResolvedUri::File(fixture.workspace.root.join("app/lib/util.dart"))
        );
    }

    /// An interpolated primary URI with no matching configuration keeps the
    /// interpolation face (`file_state.dart:759`: selected ?? primary).
    #[test]
    fn conditional_interpolated_primary_without_match() {
        let fixture = Fixture::new();
        let mut files = FileTable::new();
        let container = container(&fixture, &mut files, "app/lib/main.dart");
        let configs = [Configuration::new("dart.library.io", None, "util.dart")];
        let resolution = resolver(&fixture, &mut files).resolve_conditional(
            container,
            None,
            &configs,
            DirectiveKind::ImportExport,
        );
        assert_eq!(code_of(&resolution), ResolutionCode::UriWithInterpolation);
    }

    // ---- per-context universes: the pub universe separation ----

    static NESTED_COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A nested-root workspace: `app` is the analyzed root (its as-written
    /// config lists only itself), `app/nested` carries its own pubspec and
    /// its own `package_config.json` listing itself plus `nested_dep` (a
    /// sibling dir only nested's config sees), and the in-memory extended
    /// config adds the nested entry — the exact shape the assembly's
    /// nested-root extension produces. Returns the canonical root and the
    /// extended [`PackageConfig`].
    fn nested_workspace(tag: &str, nested_config_text: &str) -> (PathBuf, PackageConfig) {
        let root = std::env::temp_dir().join(format!(
            "dartq-resolution-nested-{tag}-{}-{}",
            std::process::id(),
            NESTED_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        let write = |relative: &str, content: &str| {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            fs::write(&path, content).expect("file");
        };
        // The as-written truth: only `app`. `packageUri: "lib/"` is the pub
        // writer's shape (an omitted field anchors the source root at the
        // package root — package_config_json.dart:150-152 — one level short
        // of these `lib/` files).
        write("app/lib/main.dart", "// fixture\n");
        write(
            "app/.dart_tool/package_config.json",
            r#"{"configVersion": 2, "packages": [{"name": "app", "rootUri": "../", "packageUri": "lib/", "languageVersion": "3.5"}], "generator": "pub"}"#,
        );
        // The nested self-resolved root, with its own dep closure.
        write(
            "app/nested/pubspec.yaml",
            "name: nested\nenvironment:\n  sdk: ^3.2.0\n",
        );
        write("app/nested/lib/n.dart", "// fixture\n");
        write("app/dep/lib/d.dart", "// fixture\n");
        write(
            "app/nested/.dart_tool/package_config.json",
            nested_config_text,
        );
        let root = root.canonicalize().expect("canonical root");
        // The extended config the assembly hands the engine: the as-written
        // entry plus the nested root (rootUri relative to `app/.dart_tool`).
        let extended = PackageConfig::parse(
            r#"{"configVersion": 2, "packages": [{"name": "app", "rootUri": "../", "packageUri": "lib/", "languageVersion": "3.5"}, {"name": "nested", "rootUri": "../nested", "packageUri": "lib/", "languageVersion": "3.2"}], "generator": "pub"}"#,
        )
.expect("valid extended config");
        (root, extended)
    }

    /// The nested boundary: each context resolves `package:` URIs against
    /// its OWN universe. Root files see only the as-written root config —
    /// the extension-added nested entry stays invisible (the oracle reports
    /// `uri_does_not_exist` there). Nested files resolve their own config's
    /// closure — including `nested_dep`, which exists ONLY in nested's
    /// config: the flat union had the opposite failure (it resolved
    /// cross-context names the oracle rejects and never knew this dep).
    /// Canonical spelling stays flat: the nested file spells
    /// `package:nested/n.dart` through the same table.
    #[test]
    fn nested_context_resolves_only_its_own_universe() {
        let (root, extended) = nested_workspace(
            "boundary",
            r#"{"configVersion": 2, "packages": [{"name": "nested", "rootUri": "../", "packageUri": "lib/"}, {"name": "nested_dep", "rootUri": "../../dep", "packageUri": "lib/"}], "generator": "pub"}"#,
        );
        let config_dir = root.join("app/.dart_tool");
        let table = PackageTable::new(&extended, &config_dir).expect("indexable config");
        let snapshot = SdkFixture::new().load();
        let declared = FxHashMap::default();
        let mut files = FileTable::new();
        let app = files.intern(&root.join("app/lib/main.dart"));
        let nested = files.intern(&root.join("app/nested/lib/n.dart"));

        let mut resolver = UriResolver::new(&table, &snapshot, &declared, &mut files, &|_| false);

        // Root context: its own package resolves; nothing from the
        // sibling's universe does.
        let own = resolver.resolve(
            app,
            Some("package:app/main.dart"),
            DirectiveKind::ImportExport,
        );
        assert_eq!(own.code, None, "the root config as written resolves");
        let sibling = resolver.resolve(
            app,
            Some("package:nested/n.dart"),
            DirectiveKind::ImportExport,
        );
        assert_eq!(
            (sibling.uri.clone(), sibling.target),
            (
                ResolvedUri::Unresolved(UnresolvedKind::NotExist),
                ResolvedTarget::Missing
            ),
            "the extension-added nested entry is invisible outside its context"
        );
        assert_eq!(code_of(&sibling), ResolutionCode::UriDoesNotExist);
        let sibling_dep = resolver.resolve(
            app,
            Some("package:nested_dep/d.dart"),
            DirectiveKind::ImportExport,
        );
        assert_eq!(code_of(&sibling_dep), ResolutionCode::UriDoesNotExist);

        // Nested context: its own config's closure resolves — the dep only
        // its config lists included — and the root package does not.
        let own = resolver.resolve(
            nested,
            Some("package:nested/n.dart"),
            DirectiveKind::ImportExport,
        );
        assert_eq!(own.code, None, "the nested config resolves its own name");
        let dep = resolver.resolve(
            nested,
            Some("package:nested_dep/d.dart"),
            DirectiveKind::ImportExport,
        );
        // Both targets land in the lib dirs the nested config materializes:
        // `nested` through `../` and `nested_dep` through `../../dep`, each
        // relative to nested's own `.dart_tool`. The two identities are
        // distinct packages — the pin is the exact landing of each.
        let ResolvedUri::Package { package, rel, file } = own.uri.clone() else {
            panic!("expected a package target");
        };
        assert_eq!(package.as_ref(), "nested");
        assert_eq!(rel, PathBuf::from("n.dart"));
        assert_eq!(file, root.join("app/nested/lib/n.dart"));
        let ResolvedUri::Package { package, rel, file } = dep.uri.clone() else {
            panic!("expected a package target");
        };
        assert_eq!(package.as_ref(), "nested_dep");
        assert_eq!(rel, PathBuf::from("d.dart"));
        assert_eq!(file, root.join("app/dep/lib/d.dart"));
        assert_eq!(dep.code, None, "nested's own dep closure resolves");
        let root_package = resolver.resolve(
            nested,
            Some("package:app/main.dart"),
            DirectiveKind::ImportExport,
        );
        assert_eq!(
            code_of(&root_package),
            ResolutionCode::UriDoesNotExist,
            "the root package is outside the nested context's universe"
        );

        // The spelling face is unchanged: nested files still spell through
        // the flat table.
        assert_eq!(
            resolver.canonical_uri(&root.join("app/nested/lib/n.dart")),
            "package:nested/n.dart"
        );
    }

    /// A nested config that exists but cannot be parsed is that context's
    /// `Packages.empty` floor: every `package:` URI from under the root
    /// stays unresolved — never a fall back to the flat universe — while
    /// the root context keeps resolving its own config.
    #[test]
    fn corrupt_nested_config_degrades_to_empty_context() {
        let (root, extended) = nested_workspace("corrupt", "{not json");
        let config_dir = root.join("app/.dart_tool");
        let table = PackageTable::new(&extended, &config_dir).expect("indexable config");
        let snapshot = SdkFixture::new().load();
        let declared = FxHashMap::default();
        let mut files = FileTable::new();
        let app = files.intern(&root.join("app/lib/main.dart"));
        let nested = files.intern(&root.join("app/nested/lib/n.dart"));
        let mut resolver = UriResolver::new(&table, &snapshot, &declared, &mut files, &|_| false);
        let own = resolver.resolve(
            app,
            Some("package:app/main.dart"),
            DirectiveKind::ImportExport,
        );
        assert_eq!(own.code, None, "the root context is unaffected");
        let from_nested = resolver.resolve(
            nested,
            Some("package:nested/n.dart"),
            DirectiveKind::ImportExport,
        );
        assert_eq!(
            code_of(&from_nested),
            ResolutionCode::UriDoesNotExist,
            "the empty-context floor, not the flat fallback"
        );
    }

    /// A package the root config lists AS WRITTEN stays visible to root
    /// files even when it carries its own `.dart_tool/package_config.json`
    /// (an independently resolved path dep): the oracle's root context
    /// parses its config as written, so `package:dep` resolves from root
    /// files — while the dep's own files resolve only its own universe.
    /// Pins the as-written re-read against the naive
    /// extended-minus-nested shape, which would hide `dep` from the root.
    #[test]
    fn path_dep_with_own_config_stays_visible_to_the_root_context() {
        let root = std::env::temp_dir().join(format!(
            "dartq-resolution-nested-pathdep-{}-{}",
            std::process::id(),
            NESTED_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        let write = |relative: &str, content: &str| {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            fs::write(&path, content).expect("file");
        };
        write("app/lib/main.dart", "// fixture\n");
        // The root config AS WRITTEN lists both app and dep (`packageUri:
        // "lib/"` — the pub writer's shape; the omitted-field reader
        // default anchors the source root at the package root itself,
        // package_config_json.dart:150-152).
        write(
            "app/.dart_tool/package_config.json",
            r#"{"configVersion": 2, "packages": [{"name": "app", "rootUri": "../", "packageUri": "lib/"}, {"name": "dep", "rootUri": "../dep", "packageUri": "lib/"}], "generator": "pub"}"#,
        );
        write(
            "app/dep/pubspec.yaml",
            "name: dep\nenvironment:\n  sdk: ^3.2.0\n",
        );
        write("app/dep/lib/d.dart", "// fixture\n");
        write(
            "app/dep/.dart_tool/package_config.json",
            r#"{"configVersion": 2, "packages": [{"name": "dep", "rootUri": "../", "packageUri": "lib/"}], "generator": "pub"}"#,
        );
        let root = root.canonicalize().expect("canonical root");
        let config_dir = root.join("app/.dart_tool");
        let config = PackageConfig::parse(
            &fs::read_to_string(config_dir.join("package_config.json")).expect("config"),
        )
        .expect("valid config");
        let table = PackageTable::new(&config, &config_dir).expect("indexable config");
        let snapshot = SdkFixture::new().load();
        let declared = FxHashMap::default();
        let mut files = FileTable::new();
        let app = files.intern(&root.join("app/lib/main.dart"));
        let dep = files.intern(&root.join("app/dep/lib/d.dart"));
        let mut resolver = UriResolver::new(&table, &snapshot, &declared, &mut files, &|_| false);
        // Root context: the as-written dep entry resolves.
        let from_root =
            resolver.resolve(app, Some("package:dep/d.dart"), DirectiveKind::ImportExport);
        assert_eq!(from_root.code, None, "the root config as written lists dep");
        // Dep's context: its own universe only.
        let own = resolver.resolve(dep, Some("package:dep/d.dart"), DirectiveKind::ImportExport);
        assert_eq!(own.code, None);
        let root_package = resolver.resolve(
            dep,
            Some("package:app/main.dart"),
            DirectiveKind::ImportExport,
        );
        assert_eq!(
            code_of(&root_package),
            ResolutionCode::UriDoesNotExist,
            "the root package is outside dep's universe"
        );
    }

    /// A dependency OUTSIDE the context root is never its own context: the
    /// oracle's locator walks only the analysis roots (`context_locator.dart`
    /// `locateRoots`), and everything a context loads — dependencies
    /// included — resolves through that one context's package map. So a
    /// sibling dep carrying its own leftover `.dart_tool` (here the
    /// `Packages.empty` floor — the stale-config kill shot) must not hijack
    /// its files' `package:` URIs: they resolve through the analyzed
    /// context's map, keeping the dep's export edges landed instead of
    /// severed (every export-forwarded name reporting undefined).
    #[test]
    fn sibling_dep_outside_the_root_resolves_through_the_root_universe() {
        let root = std::env::temp_dir().join(format!(
            "dartq-resolution-nested-sibling-{}-{}",
            std::process::id(),
            NESTED_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        let write = |relative: &str, content: &str| {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            fs::write(&path, content).expect("file");
        };
        write("app/lib/main.dart", "// fixture\n");
        // The root config AS WRITTEN lists the sibling dep outside `app`.
        write(
            "app/.dart_tool/package_config.json",
            r#"{"configVersion": 2, "packages": [{"name": "app", "rootUri": "../", "packageUri": "lib/"}, {"name": "dep", "rootUri": "../../dep", "packageUri": "lib/"}], "generator": "pub"}"#,
        );
        write(
            "dep/pubspec.yaml",
            "name: dep\nenvironment:\n  sdk: ^3.2.0\n",
        );
        write("dep/lib/d.dart", "// fixture\n");
        // The dep's own leftover config: parseable but empty — its own
        // universe would resolve nothing.
        write(
            "dep/.dart_tool/package_config.json",
            r#"{"configVersion": 2, "packages": [], "generator": "pub"}"#,
        );
        let root = root.canonicalize().expect("canonical root");
        let config_dir = root.join("app/.dart_tool");
        let config = PackageConfig::parse(
            &fs::read_to_string(config_dir.join("package_config.json")).expect("config"),
        )
        .expect("valid config");
        let table = PackageTable::new(&config, &config_dir).expect("indexable config");
        let snapshot = SdkFixture::new().load();
        let declared = FxHashMap::default();
        let mut files = FileTable::new();
        let app = files.intern(&root.join("app/lib/main.dart"));
        let dep = files.intern(&root.join("dep/lib/d.dart"));
        let mut resolver = UriResolver::new(&table, &snapshot, &declared, &mut files, &|_| false);
        // The root context resolves the dep, as ever.
        let from_root =
            resolver.resolve(app, Some("package:dep/d.dart"), DirectiveKind::ImportExport);
        assert_eq!(from_root.code, None, "the root config as written lists dep");
        // The dep's OWN files resolve through the analyzed context's map —
        // the empty leftover config never governs them.
        let own = resolver.resolve(dep, Some("package:dep/d.dart"), DirectiveKind::ImportExport);
        assert_eq!(
            own.code, None,
            "a dependency outside the context root resolves through the root universe"
        );
        let ResolvedUri::Package { package, file, .. } = own.uri.clone() else {
            panic!("expected a package target");
        };
        assert_eq!(package.as_ref(), "dep");
        assert_eq!(file, root.join("dep/lib/d.dart"));
    }
}
