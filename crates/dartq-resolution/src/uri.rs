//! Directive URI parsing, classification, and resolution to interned file
//! targets (`docs/facts/resolution-design-spec.md` §4.1/§5 module 1).
//!
//! The pipeline mirrors the analyzer's: parse the directive URI string with
//! Dart `Uri.parse` validity rules, resolve relative strings against the
//! containing file's canonical URI (`file_state.dart:766-796`), then dispatch
//! the absolute URI by scheme — `dart:` through the SDK's `libraries.json`
//! table, `package:` through the `package_config` (package name → lib dir),
//! `file:` to a path, everything else unresolvable (`file_state.dart:1458-
//! 1497`). Resolution is infallible: every failure mode is a classification
//! ([`ResolvedUri::Unresolved`]) plus the §4.2 diagnostic code the oracle
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

use dartq_pub::{PackageConfig, PubError, normalize};
use dartq_sdk::SdkSnapshot;
use dartq_source::{FileId, FileTable};
use rustc_hash::FxHashMap;

use crate::codes::ResolutionCode;
use crate::conditional::{Configuration, SelectedUri, resolve_conditional};

/// How a directive URI resolved (§5 core types).
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

/// Why a directive URI has no target (§5 core types).
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
    /// Upper-layer-only contract: never constructed in this crate until the
    /// file-role boundary lands (#119 lineage) — the gap is deliberate, not
    /// an oversight.
    Generated,
    /// The target file exists but is a part file — decided by the
    /// library-graph layer, which alone knows file roles; reported as
    /// `import_of_non_library` / `export_of_non_library`. Upper-layer-only
    /// contract: never constructed in this crate until the file-role
    /// boundary lands (#119 lineage) — the gap is deliberate, not an
    /// oversight.
    NotLibrary,
    /// The selected URI is `dart-ext:`; reported as
    /// `use_of_native_extension` (additive to the spec draft, which has no
    /// carrier for the §4.2 row).
    DartExt,
}

/// Engine-facing projection of a [`ResolvedUri`] (§5, engine-api-spec §1):
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
    /// The §4.2 diagnostic code the oracle emits for this outcome, or `None`
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
    /// A documentation `@docImport` directive.
    DocImport,
}

/// The `package:` resolution face of a parsed package config: package name →
/// lib dir (§5 interface table, `dartq-pub` row).
///
/// Built once per analysis context; lookups are [`FxHashMap`] hits on the
/// per-directive hot path. Also owns the generated-file face the analyzer
/// layers onto `package:` targets (`.dart_tool/build/generated`).
///
/// Root/lib-dir resolution is [`PackageConfig::resolve`], dartq-pub's
/// single shared face — duplicate names are a hard error (pub's
/// `package_config.dart:44-51` throw, not a silent last-wins), names follow
/// the loose reader grammar, `packageUri` directories are normalized and
/// subtree-validated — exactly the policies the identity table applies, so
/// the two tables can never diverge.
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
    /// unique (duplicate names are a hard error in the shared dartq-pub
    /// face), so one name owns one position.
    indices: FxHashMap<Box<str>, usize>,
    /// Directory holding `.dart_tool/` — the base of the generated-file
    /// fallback.
    workspace_root: PathBuf,
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
    /// broken `rootUri`, duplicate names, a name outside the reader
    /// grammar, or a `packageUri` escaping its root; a broken config,
    /// reported loudly.
    pub fn new(config: &PackageConfig, config_dir: &Path) -> Result<Self, PubError> {
        let mut lib_dirs: Vec<(PathBuf, Box<str>)> = Vec::new();
        for resolved in config.resolve(config_dir)? {
            lib_dirs.push((
                canonical_dir(&resolved.lib_dir),
                Box::from(resolved.name.as_str()),
            ));
        }
        lib_dirs.sort_by_key(|(dir, _)| std::cmp::Reverse(dir.components().count()));
        let indices: FxHashMap<Box<str>, usize> = lib_dirs
            .iter()
            .enumerate()
            .map(|(index, (_, name))| (name.clone(), index))
            .collect();
        let workspace_root = config_dir
            .parent()
            .map_or_else(|| config_dir.to_path_buf(), Path::to_path_buf);
        Ok(Self {
            lib_dirs,
            indices,
            workspace_root,
        })
    }

    /// The lib dir of package `name`, or `None` for an unknown package.
    #[must_use]
    pub fn lib_dir(&self, name: &str) -> Option<&Path> {
        let index = *self.indices.get(name)?;
        Some(self.lib_dirs[index].0.as_path())
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
    target: Box<str>,
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
    Package { package: Box<str>, path: Box<str> },
    /// `file://<path>` — the container is anywhere else.
    File(Box<str>),
}

/// An absolute URI after composition, ready to be landed on a target. The
/// parsed parts move structurally — nothing is re-joined and re-split.
enum Composed {
    Package {
        package: Box<str>,
        rel: Box<str>,
    },
    File(Box<str>),
    /// A `dart:` URI split at the library boundary: the path's first segment
    /// (`core` for both `dart:core` and `dart:core/bool.dart`; an authority
    /// form leaves it empty, which no SDK table lists) and the raw part
    /// remainder — exactly the pair [`SdkSnapshot::library_to_path`]
    /// consumes.
    Dart {
        library: Box<str>,
        part: Box<str>,
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
/// -normalized path. [`container_base`] reaches it through field-disjoint
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
fn compose_canonical_uri(packages: &PackageTable, path: &Path) -> Box<str> {
    if let Some((package, rel)) = package_of(packages, path) {
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
            package: Box::from(package),
            path: Box::from(rel),
        };
    }
    BaseUri::File(Box::from(
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
        target: &str,
        declared: &'a FxHashMap<String, String>,
        files: &'a mut FileTable,
        resident: &'a dyn Fn(FileId) -> bool,
    ) -> Self {
        Self {
            packages,
            sdk,
            target: Box::from(target),
            declared,
            files,
            resident,
            bases: FxHashMap::default(),
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
        if !raw.scheme.is_empty() {
            return self.land(compose_absolute(raw));
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
        let composed = compose_relative(&self.bases[&container], &raw);
        self.land(composed)
    }

    /// The canonical URI of `path` (`pathToUri` face): `package:` inside a
    /// package lib dir (deepest match wins), `file://` otherwise. This is the
    /// library-identity string the library-graph layer keys on — and the one
    /// implementation of the spelling, so a change to it cannot desync the
    /// composition base ([`container_base`] derives from it). Memoized per
    /// `FileTable`-normalized path for this resolver's lifetime (the package
    /// table cannot change under a live resolver); the returned borrow shares
    /// that lifetime, and callers needing a longer horizon cache the owned
    /// result — the engine's per-file canonical table does exactly that.
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
    /// generated-file fallback, interning, and the §4.2 code.
    fn land(&mut self, composed: Composed) -> UriResolution {
        match composed {
            Composed::DartExt => UriResolution {
                uri: ResolvedUri::Unresolved(UnresolvedKind::DartExt),
                target: ResolvedTarget::Missing,
                code: Some(ResolutionCode::UseOfNativeExtension),
            },
            Composed::Dart { library, part } => {
                match self.sdk.library_to_path(&self.target, &library, &part) {
                    Some(file) => UriResolution {
                        uri: ResolvedUri::Dart { library, file },
                        target: ResolvedTarget::External,
                        code: None,
                    },
                    // Unknown dart: library (or target): source is null —
                    // `uri_does_not_exist` per §4.2.
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
            Composed::Package { package, rel } => self.land_package(package, &rel),
        }
    }

    /// Lands a `package:` target: lib-dir lookup, the generated-file
    /// fallback, and the missing-file code.
    fn land_package(&mut self, package: Box<str>, rel: &str) -> UriResolution {
        let unresolvable = || UriResolution {
            uri: ResolvedUri::Unresolved(UnresolvedKind::NotExist),
            target: ResolvedTarget::Missing,
            code: Some(ResolutionCode::UriDoesNotExist),
        };
        let Some(lib_dir) = self.packages.lib_dir(&package) else {
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
            // — the probe stays disk-only).
            let built = join_rel(
                &self
                    .packages
                    .workspace_root
                    .join(".dart_tool/build/generated")
                    .join(package.as_ref())
                    .join("lib"),
                rel,
            );
            if built.exists() {
                let id = self.files.intern(&built);
                return UriResolution {
                    uri: ResolvedUri::Package {
                        package,
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
                    package,
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
                package,
                rel: PathBuf::from(&*decoded),
                file,
            },
            target: ResolvedTarget::Resolved(id),
            code: None,
        }
    }
}

/// The §4.2 code for a known-path-but-missing file: generated-suffix paths
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawUri {
    /// Lowercased scheme, empty for relative URIs.
    pub scheme: Box<str>,
    /// Raw authority text (between `//` and the next `/`, `?`, `#`), `Some("")`
    /// for an empty authority as in `file:///x`.
    pub authority: Option<Box<str>>,
    /// Raw path as written (percent-escapes intact).
    pub path: Box<str>,
}

/// Parses `uri` with Dart `Uri.tryParse` fidelity for the forms directive
/// strings take: scheme syntax, authority shape, per-component character
/// tables, and percent-escape well-formedness. Non-ASCII characters are
/// accepted (Dart normalizes them to escapes; targets are decoded paths).
///
/// # Errors
///
/// [`InvalidUriSyntax`] where Dart throws `FormatException` — reported as
/// `invalid_uri`.
pub fn parse_directive_uri(uri: &str) -> Result<RawUri, InvalidUriSyntax> {
    // `Uri.parse` routes any string starting with a case-insensitive `data:`
    // to `UriData._parse` before the general scanner runs (uri.dart:931-939),
    // so the path character tables never apply to it.
    if uri.len() >= 5 && uri.as_bytes()[..5].eq_ignore_ascii_case(b"data:") {
        return parse_data_uri(uri);
    }
    let bytes = uri.as_bytes();

    // Scheme: `:` after a run of scheme characters; must start alphabetic
    // ("Invalid empty scheme" / "Scheme not starting with alphabetic
    // character" / "Illegal scheme character").
    let mut scheme_end = 0;
    while scheme_end < bytes.len() && is_scheme_char(bytes[scheme_end]) {
        scheme_end += 1;
    }
    let (scheme, mut index) = if scheme_end < bytes.len() && bytes[scheme_end] == b':' {
        if scheme_end == 0 || !bytes[0].is_ascii_alphabetic() {
            return Err(InvalidUriSyntax);
        }
        (
            uri[..scheme_end].to_ascii_lowercase().into_boxed_str(),
            scheme_end + 1,
        )
    } else {
        (Box::from(""), 0)
    };

    // Authority: everything after `//` up to the next path/query/fragment
    // delimiter.
    let mut authority = None;
    if bytes.len() >= index + 2 && bytes[index] == b'/' && bytes[index + 1] == b'/' {
        let start = index + 2;
        let end = bytes[start..]
            .iter()
            .position(|&b| matches!(b, b'/' | b'?' | b'#'))
            .map_or(bytes.len(), |offset| start + offset);
        validate_authority(&uri[start..end])?;
        authority = Some(Box::from(&uri[start..end]));
        index = end;
    }

    // Path, then query, then fragment — each validated with its own table.
    let path_end = bytes[index..]
        .iter()
        .position(|&b| matches!(b, b'?' | b'#'))
        .map_or(bytes.len(), |offset| index + offset);
    validate_component(&uri[index..path_end], PathOrQuery::Path)?;
    let mut rest = path_end;
    if rest < bytes.len() && bytes[rest] == b'?' {
        rest += 1;
        let query_end = bytes[rest..]
            .iter()
            .position(|&b| b == b'#')
            .map_or(bytes.len(), |offset| rest + offset);
        validate_component(&uri[rest..query_end], PathOrQuery::Query)?;
        rest = query_end;
    }
    if rest < bytes.len() {
        validate_component(&uri[rest + 1..], PathOrQuery::Query)?;
    }

    Ok(RawUri {
        scheme,
        authority,
        path: Box::from(&uri[index..path_end]),
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
fn parse_data_uri(uri: &str) -> Result<RawUri, InvalidUriSyntax> {
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
        scheme: Box::from("data"),
        authority: None,
        path: Box::from(&uri[5..]),
    })
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

/// Path vs query/fragment character tables (`_pathCharOrSlashMask` vs
/// `_queryCharMask`): the query table adds `?`, and fragments share it.
#[derive(Clone, Copy)]
enum PathOrQuery {
    Path,
    Query,
}

fn is_scheme_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')
}

fn validate_component(text: &str, component: PathOrQuery) -> Result<(), InvalidUriSyntax> {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let b = bytes[index];
        match b {
            b'%' => {
                let hex = bytes.get(index + 1..index + 3).ok_or(InvalidUriSyntax)?;
                if !hex.iter().all(u8::is_ascii_hexdigit) {
                    return Err(InvalidUriSyntax);
                }
                index += 3;
            }
            _ if b < 0x80 => {
                let allowed = match component {
                    PathOrQuery::Path => {
                        matches!(
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
                        ) || b.is_ascii_alphanumeric()
                    }
                    PathOrQuery::Query => {
                        matches!(b, b'/' | b'?')
                            || matches!(
                                b,
                                b':' | b'@'
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
                            || b.is_ascii_alphanumeric()
                    }
                };
                if !allowed {
                    return Err(InvalidUriSyntax);
                }
                index += 1;
            }
            // Non-ASCII: accepted, as in Dart (normalized to escapes there).
            _ => index += 1,
        }
    }
    Ok(())
}

/// Authority shape: optional IPv6 `[...]` host (with optional `:port`), else
/// reg-name/userinfo characters (`_regNameMask`/`_userinfoMask` plus the `@`
/// and port `:` delimiters).
fn validate_authority(text: &str) -> Result<(), InvalidUriSyntax> {
    let bytes = text.as_bytes();
    if bytes.first() == Some(&b'[') {
        let close = bytes
            .iter()
            .position(|&b| b == b']')
            .ok_or(InvalidUriSyntax)?;
        if !bytes[1..close]
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
    // Reg-name/userinfo characters (`_regNameMask`/`_userinfoMask` plus the
    // `@` and port `:` delimiters), with percent escapes validated as 2-hex —
    // Dart `Uri.parse` checks escape well-formedness in every component, so
    // `file://a%zz/` throws (`invalid_uri` per `file_state.dart:445-451`).
    let mut index = 0;
    while index < bytes.len() {
        let b = bytes[index];
        let allowed = b >= 0x80
            || b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'.'
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
                    | b'%'
            );
        if !allowed {
            return Err(InvalidUriSyntax);
        }
        if b == b'%' {
            let hex = bytes.get(index + 1..index + 3).ok_or(InvalidUriSyntax)?;
            if !hex.iter().all(u8::is_ascii_hexdigit) {
                return Err(InvalidUriSyntax);
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    Ok(())
}

/// Classifies an absolute (scheme-carrying) [`RawUri`] for landing, consuming
/// the parse — the parts move structurally into [`Composed`], never re-joined
/// and re-split.
fn compose_absolute(raw: RawUri) -> Composed {
    match (raw.scheme.as_ref(), raw.authority.as_deref()) {
        ("dart-ext", _) => Composed::DartExt,
        ("dart", _) => {
            // The library name is the path's first segment (`core` for both
            // `dart:core` and `dart:core/bool.dart`); an authority form
            // (`dart://x/core`) leaves it empty, which no SDK table lists.
            // One split, no re-join: the pair moves structurally into
            // `land` and on to the SDK's `library_to_path`.
            let (library, part) = match raw.path.split_once('/') {
                Some((name, part)) => (Box::from(name), Box::from(part)),
                None => (raw.path, Box::from("")),
            };
            Composed::Dart { library, part }
        }
        ("package", _) => {
            // A leading `/` (any authority, or `package:///x`) or a missing
            // `/` (no package name) leaves the resolver without a package —
            // `slash < 1` → null in the oracle.
            if raw.path.starts_with('/') || !raw.path.contains('/') {
                Composed::Other
            } else {
                let (package, rel) = raw.path.split_once('/').unwrap_or(("", ""));
                Composed::Package {
                    package: Box::from(package),
                    rel: Box::from(rel),
                }
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
fn compose_relative(base: &BaseUri, reference: &RawUri) -> Composed {
    // An empty reference is the base itself (`resolveUri` empty-path rule) —
    // `import ''` imports the containing file, as in the oracle.
    if reference.path.is_empty() {
        return match base {
            BaseUri::Package { package, path } => Composed::Package {
                package: package.clone(),
                rel: path.clone(),
            },
            BaseUri::File(path) => Composed::File(path.clone()),
        };
    }
    // A reference with its own authority keeps it under the base scheme; on a
    // package base any authority leaves a leading `/` in the path, which the
    // package resolver rejects (`slash < 1`).
    if let Some(authority) = &reference.authority {
        return match base {
            BaseUri::File(_) => compose_absolute(RawUri {
                scheme: Box::from("file"),
                authority: Some(authority.clone()),
                path: reference.path.clone(),
            }),
            BaseUri::Package { .. } => Composed::Other,
        };
    }
    // An absolute-path reference replaces the base path wholesale, keeping
    // its leading `/`: on a package base the result (`package:/x`) parses no
    // package name → unresolvable.
    if reference.path.starts_with('/') {
        return match base {
            BaseUri::File(_) => Composed::File(reference.path.clone()),
            BaseUri::Package { .. } => Composed::Other,
        };
    }
    match base {
        BaseUri::Package { package, path } => {
            let merged = remove_dot_segments(&merge_paths(path, &reference.path));
            // Escaped the package root: the FIRST segment is exactly `..`
            // (the oracle's `pathSegments.first == '..'` equality check), so
            // no package URI can express it. A segment merely *starting* with
            // `..` (`..foo.dart`) is a legal name, not an escape.
            if merged == ".." || merged.starts_with("../") {
                Composed::Other
            } else {
                Composed::Package {
                    package: package.clone(),
                    rel: Box::from(merged),
                }
            }
        }
        BaseUri::File(path) => Composed::File(Box::from(remove_dot_segments(&merge_paths(
            path,
            &reference.path,
        )))),
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
/// segments survive.
fn remove_dot_segments(path: &str) -> String {
    if !path.contains('.') {
        return path.to_owned();
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

        let data = parse_directive_uri("data:text/plain,main(){}").expect("data");
        assert_eq!(data.scheme.as_ref(), "data");

        let ext = parse_directive_uri("dart-ext:native.so").expect("dart-ext");
        assert_eq!(ext.scheme.as_ref(), "dart-ext");

        // Scheme case is lowered; escapes and query/fragment split correctly.
        let upper = parse_directive_uri("PACKAGE:p/a.dart").expect("upper");
        assert_eq!(upper.scheme.as_ref(), "package");
        let query = parse_directive_uri("a.dart?x=1#frag").expect("query");
        assert_eq!(query.path.as_ref(), "a.dart");
        let escaped = parse_directive_uri("my%20lib/a.dart").expect("escaped");
        assert_eq!(escaped.path.as_ref(), "my%20lib/a.dart");
    }

    #[test]
    fn rejects_uri_syntax_dart_rejects() {
        // Empty scheme / non-alphabetic scheme start / illegal scheme char.
        assert!(parse_directive_uri("::x").is_err());
        assert!(parse_directive_uri("1a:b").is_err());
        assert!(parse_directive_uri("a b:c").is_err());
        // Space and general delimiters are not path characters.
        assert!(parse_directive_uri("a b.dart").is_err());
        assert!(parse_directive_uri("a<b.dart").is_err());
        assert!(parse_directive_uri("a\\b.dart").is_err());
        assert!(parse_directive_uri("a^b.dart").is_err());
        assert!(parse_directive_uri("a`b.dart").is_err());
        assert!(parse_directive_uri("a{b}.dart").is_err());
        assert!(parse_directive_uri("a[b].dart").is_err());
        // Broken percent escapes.
        assert!(parse_directive_uri("a%zz.dart").is_err());
        assert!(parse_directive_uri("a%.dart").is_err());
        assert!(parse_directive_uri("a%2").is_err());
        // …including inside an authority: Dart Uri.parse validates escapes in
        // every component (`file://a%zz/` throws → invalid_uri).
        assert!(parse_directive_uri("file://a%zz/x.dart").is_err());
        assert!(parse_directive_uri("file://a%2/host.dart").is_err());
        // Well-formed authority escapes still parse.
        assert!(parse_directive_uri("file://a%2f/x.dart").is_ok());
        // Authority shapes.
        assert!(parse_directive_uri("file://ho[st]/x.dart").is_err());
        assert!(parse_directive_uri("package://[::1/x.dart").is_err());
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
            write("app/bin/tool.dart", "// fixture\n");
            write("hosted/lib/h.dart", "// fixture\n");
            write(
                "app/.dart_tool/package_config.json",
                &format!(
                    // rootUris resolve against the config dir
                    // (`app/.dart_tool`), so `../../pkg_a` is the fixture's
                    // sibling package.
                    r#"{{
                      "configVersion": 2,
                      "packages": [
                        {{"name": "pkg_a", "rootUri": "../../pkg_a"}},
                        {{"name": "hosted", "rootUri": "file://{}"}}
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
            "vm",
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

        let core = resolver.resolve(container, Some("dart:core"), DirectiveKind::ImportExport);
        assert_eq!(
            core.uri,
            ResolvedUri::Dart {
                library: "core".into(),
                file: fixture.sdk.root.join("lib/core/core.dart"),
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
        assert_eq!(file, fixture.sdk.root.join("lib/core/bool.dart"));
        assert_eq!(part.target, ResolvedTarget::External);

        // Unknown dart library (and unknown on this target): source null.
        let unknown =
            resolver.resolve(container, Some("dart:mirrors"), DirectiveKind::ImportExport);
        assert_eq!(
            unknown.uri,
            ResolvedUri::Unresolved(UnresolvedKind::NotExist)
        );
        assert_eq!(code_of(&unknown), ResolutionCode::UriDoesNotExist);
        assert_eq!(unknown.target, ResolvedTarget::Missing);
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

        // A broken escape inside an authority: Dart Uri.parse throws, so the
        // oracle reports invalid_uri (file_state.dart:445-451).
        let bad_authority = resolver.resolve(
            container,
            Some("file://a%zz/x.dart"),
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

    /// The shared dartq-pub face applies to this table too: duplicate
    /// config names are pub's hard error (`package_config.dart:44-51`, the
    /// verbatim ArgumentError text), never a silent last-wins over the
    /// lib-dir index.
    #[test]
    fn duplicate_names_fail_loudly() {
        let config = PackageConfig::parse(
            r#"{"configVersion": 2, "packages": [
                {"name": "a", "rootUri": "../x"},
                {"name": "a", "rootUri": "../y"}]}"#,
        )
        .expect("parses");
        let error =
            PackageTable::new(&config, Path::new("/p/.dart_tool")).expect_err("duplicate name");
        assert!(
            error
                .to_string()
                .contains("Duplicate name a in generated package config"),
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
}
