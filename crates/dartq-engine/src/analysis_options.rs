//! `analysis_options.yaml` loading and include-chain merge (PRD §12.1/§12.2).
//!
//! Semantics are pinned to the official merge in
//! `.third/dart-sdk/pkg/analyzer/lib/src/analysis_options/`:
//!
//! - **Depth-first, far-before-near** — `_applyParsedFiles`
//!   (analysis_options_parser.dart:384-408) applies every include
//!   recursively *before* the including file's own data, so a nearer file's
//!   entry overrides the farther one (scalars are "set if present"
//!   (`_ApplyState._applyLanguage`), lint rules land in a per-name map via
//!   `addAll` (`_applyLinterSection`) where nearer entries replace same-name
//!   farther entries and sets union).
//! - **Include resolution** — `sourceFactory.resolveUri` against the
//!   *including* file (parser:604-611): relative URIs resolve against the
//!   including file's directory, `package:` URIs through the package config.
//!   The include chain guards recursion (`includeChain.contains`, parser
//!   :395).
//! - **Exclude globs** — each pattern is recorded with its *declaring file*
//!   (`AnalysisOptionsExcludePattern`) and compiled relative to that file's
//!   directory (`_getExcludedGlobs`, context_locator.dart:625-639, with the
//!   `foo/**` pattern also excluding the `foo` directory itself so the walk
//!   stops descending). Matching tests the path relative to the declaring
//!   directory (`LocatedGlob.matches`, context_root.dart:183-194). A pattern
//!   that is not a string is ignored; a non-list `exclude:` section is
//!   ignored (`_ParsedExcludesData.parse`, `analysis_options_parse_model.dart`
//!   :634-681).
//! - **Linter rule entries** — `parseLinterSection`/`_parseRuleConfig`
//!   (lint/config.dart:20-104): a list of names, or a map of `name: bool`,
//!   `name: severity-string` (`disable` disables, any other string enables),
//!   or a nested group map; unknown shapes ignored. A `linter:` key that is
//!   present but not a map marks the section invalid, which *clears* the
//!   rules merged so far (`_applyLinterSection`, parser:278-289).
//! - Unknown keys and unreadable or malformed files are tolerated — the
//!   official reports options-file diagnostics on a separate surface and
//!   still produces effective options (missing include → skipped).
//!
//! The whole face is pure functions over paths + YAML: testable without the
//! CLI, and cold-path (workspace assembly), so clarity beats micro-cost.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};
use serde_yaml::Value;

use dartq_pub::PackageConfig;

/// The options file name the analyzer locates walking up from a context
/// root (`ContextLocator.locate`).
const OPTIONS_FILE_NAME: &str = "analysis_options.yaml";

/// One package root's fully merged analysis options (the
/// `ResolvedAnalysisOptions` slice PRD §12.1 names, limited to the sections
/// this engine consumes: the lint rule set and the exclude globs; scalar
/// sections such as `strict-casts` join when their consumers land).
#[derive(Debug, Default)]
pub struct AnalysisOptions {
    /// Effective enabled lint rule names — the union across the include
    /// chain with nearer entries winning per name, sorted and deduplicated
    /// (the data the lint engine resolves its enable set from).
    pub lint_rules: Vec<String>,
    /// Precompiled exclude globs, each anchored to its declaring file's
    /// directory (official `LocatedGlob`).
    excludes: Vec<ExcludeGlob>,
}

/// One compiled exclude pattern plus the directory it is relative to
/// (official `LocatedGlob`: parent + `package:glob`).
#[derive(Debug)]
pub struct ExcludeGlob {
    /// The declaring options file's directory; candidate paths outside it
    /// never match.
    parent: PathBuf,
    /// The pattern, plus the directory pattern for a `foo/**` spelling (the
    /// `**` alternative cannot match `foo` itself, and the walk must stop
    /// descending — context_locator.dart:633-638).
    alternatives: Vec<dartq_pub::glob::Glob>,
}

impl ExcludeGlob {
    /// Compiles one pattern string relative to `parent` (the
    /// `addGlob` + `posix.split` normalization of `context_locator.dart`:
    /// 625-639: empty and `.` components dropped, a trailing `**` adds the
    /// prefix pattern). Malformed glob syntax leaves the pattern out — the
    /// official surface reports it and keeps analyzing.
    fn compile(pattern: &str, parent: &Path) -> Option<ExcludeGlob> {
        // A leading `/` anchors the glob at the filesystem root
        // (`excludeInvalidGlob` in the official surface): it can never match
        // a path relative to the declaring directory, so it compiles to
        // nothing.
        if pattern.starts_with('/') {
            return None;
        }
        let components: Vec<&str> = pattern
            .split('/')
            .filter(|component| !component.is_empty() && *component != ".")
            .collect();
        let mut alternatives = vec![dartq_pub::glob::Glob::new(&components.join("/")).ok()?];
        if components.len() > 1
            && components.last() == Some(&"**")
            && let Ok(glob) =
                dartq_pub::glob::Glob::new(&components[..components.len() - 1].join("/"))
        {
            alternatives.push(glob);
        }
        Some(ExcludeGlob {
            parent: parent.to_path_buf(),
            alternatives,
        })
    }

    /// Whether `path` (absolute, same universe as the declaring file) is
    /// excluded: only paths inside the declaring directory can match, and
    /// they match relative to it (`LocatedGlob.matches`).
    #[must_use]
    pub fn matches(&self, path: &Path) -> bool {
        let Ok(relative) = path.strip_prefix(&self.parent) else {
            return false;
        };
        let relative = relative.to_string_lossy();
        !relative.is_empty() && self.alternatives.iter().any(|g| g.matches_path(&relative))
    }
}

impl AnalysisOptions {
    /// Whether any exclude glob matches `path` (the context-root walk calls
    /// this per resource; globs are precompiled at load).
    #[must_use]
    pub fn is_excluded(&self, path: &Path) -> bool {
        self.excludes.iter().any(|glob| glob.matches(path))
    }

    /// The compiled exclude globs, in declaration order (far to near). The
    /// lint face applies the same predicate as discovery — one rule, one
    /// place.
    #[must_use]
    pub fn exclude_globs(&self) -> &[ExcludeGlob] {
        &self.excludes
    }

    /// Locates the options file governing `start`: the nearest
    /// `analysis_options.yaml` at or above `start` (the official walk-up in
    /// `ContextLocator`), `None` when no ancestor declares one.
    #[must_use]
    pub fn find_for_dir(start: &Path) -> Option<PathBuf> {
        let mut dir = Some(start);
        while let Some(current) = dir {
            let candidate = current.join(OPTIONS_FILE_NAME);
            if candidate.is_file() {
                return Some(candidate);
            }
            dir = current.parent();
        }
        None
    }

    /// Loads and merges the options file at `path`. Total: an unreadable or
    /// malformed file, a missing include, or a cycle all degrade to
    /// "that input contributes nothing" (official observable behavior); the
    /// returned options are always usable.
    #[must_use]
    pub fn load(path: &Path, packages: &PackageConfig, config_dir: &Path) -> AnalysisOptions {
        let mut merge = MergeState::default();
        let mut chain = FxHashSet::default();
        merge.apply_file(path, &mut chain, packages, config_dir);
        merge.build()
    }

    /// One merged [`AnalysisOptions`] per package root, sharing loaded files
    /// through `loaded` (workspace members under one repo-root options file
    /// parse it once). Roots without any options file map to the shared
    /// default.
    #[must_use]
    pub fn for_roots(
        roots: &[PathBuf],
        packages: &PackageConfig,
        config_dir: &Path,
    ) -> FxHashMap<PathBuf, Arc<AnalysisOptions>> {
        let mut loaded: FxHashMap<PathBuf, Arc<AnalysisOptions>> = FxHashMap::default();
        let mut per_root = FxHashMap::default();
        for root in roots {
            let file = Self::find_for_dir(root);
            let options = match &file {
                Some(file) => loaded
                    .entry(file.clone())
                    .or_insert_with(|| Arc::new(Self::load(file, packages, config_dir)))
                    .clone(),
                None => Arc::new(Self::default()),
            };
            per_root.insert(root.clone(), options);
        }
        per_root
    }
}

/// The mutable accumulator one include-graph traversal folds into (the
/// official `_ApplyState`: a per-name rule map the nearer file's `addAll`
/// overwrites into, and the declaring-file-anchored exclude list).
#[derive(Default)]
struct MergeState {
    /// Rule name → enabled; insertion order is irrelevant (the built set is
    /// sorted).
    rules: FxHashMap<String, bool>,
    excludes: Vec<ExcludeGlob>,
    /// Deduplication key of the official `Set<AnalysisOptionsExcludePattern>`:
    /// (declaring file, pattern).
    exclude_keys: FxHashSet<(PathBuf, String)>,
}

impl MergeState {
    /// The official `_applyParsedFiles`: apply every resolvable include
    /// first, then this file's own data — depth-first, near data last.
    fn apply_file(
        &mut self,
        file: &Path,
        chain: &mut FxHashSet<PathBuf>,
        packages: &PackageConfig,
        config_dir: &Path,
    ) {
        if !chain.insert(file.to_path_buf()) {
            return; // recursive include: contributes once (parser:395)
        }
        let Ok(text) = std::fs::read_to_string(file) else {
            chain.remove(file);
            return;
        };
        let Ok(yaml) = serde_yaml::from_str::<Value>(&text) else {
            chain.remove(file);
            return;
        };

        for uri in include_uris(&yaml) {
            let Some(included) = resolve_include(&uri, file, packages, config_dir) else {
                continue; // missing include: skipped (official reports, keeps going)
            };
            self.apply_file(&included, chain, packages, config_dir);
        }

        let declaring_dir = file.parent().unwrap_or(Path::new("")).to_path_buf();
        self.apply_local(&yaml, &declaring_dir);
        chain.remove(file);
    }

    /// Applies one parsed file's own sections (the official
    /// `_ApplyState.apply`, the sections this engine consumes).
    fn apply_local(&mut self, yaml: &Value, declaring_dir: &Path) {
        let Some(map) = yaml.as_mapping() else {
            return;
        };
        match map.get(Value::from("linter")) {
            // `linter:` absent or null: inherit what the includes merged.
            None | Some(Value::Null) => {}
            // Present but not a map: invalid section — clears everything
            // merged so far (parser:282-284).
            Some(Value::Mapping(linter)) => {
                if let Some(rules) = linter
                    .get(Value::from("rules"))
                    .filter(|rules| !rules.is_null())
                {
                    apply_rules(rules, &mut self.rules);
                }
            }
            Some(_) => self.rules.clear(),
        }
        if let Some(patterns) = exclude_patterns(map) {
            for pattern in patterns {
                if self
                    .exclude_keys
                    .insert((declaring_dir.to_path_buf(), pattern.clone()))
                    && let Some(glob) = ExcludeGlob::compile(&pattern, declaring_dir)
                {
                    self.excludes.push(glob);
                }
            }
        }
    }

    /// Freezes the merged state: enabled rule names, sorted (the canonical
    /// order the official `build()` derives through the registry).
    fn build(self) -> AnalysisOptions {
        let mut names: Vec<String> = self
            .rules
            .into_iter()
            .filter_map(|(name, enabled)| enabled.then_some(name))
            .collect();
        names.sort();
        names.dedup();
        AnalysisOptions {
            lint_rules: names,
            excludes: self.excludes,
        }
    }
}

/// Merges one file's `linter.rules` value into the rule map
/// (`parseDiagnosticsSection`, lint/config.dart:20-64): a list of names, or
/// a map of per-rule entries; unknown shapes contribute nothing.
fn apply_rules(rules: &Value, merged: &mut FxHashMap<String, bool>) {
    match rules {
        Value::Sequence(list) => {
            for item in list {
                if let Some(name) = item.as_str() {
                    merged.insert(name.to_owned(), true);
                }
            }
        }
        Value::Mapping(map) => {
            for (key, value) in map {
                let Some(name) = key.as_str() else {
                    continue;
                };
                merge_rule_entry(name, value, merged);
            }
        }
        _ => {}
    }
}

/// One `name: value` rule entry (`_parseRuleConfig`, lint/config.dart
/// :83-104): `true`/`false`, a severity string (`disable` disables, any
/// other string enables), or a nested group map recursed entry-wise; null
/// and number values are skipped.
fn merge_rule_entry(name: &str, value: &Value, merged: &mut FxHashMap<String, bool>) {
    match value {
        Value::Bool(enabled) => {
            merged.insert(name.to_owned(), *enabled);
        }
        Value::String(severity) => {
            merged.insert(name.to_owned(), severity != "disable");
        }
        Value::Mapping(group) => {
            for (key, value) in group {
                if let Some(rule) = key.as_str() {
                    merge_rule_entry(rule, value, merged);
                }
            }
        }
        _ => {}
    }
}

/// The `analyzer.exclude` pattern strings (the `_ParsedExcludesData.parse`
/// shape: list-only, string entries only, everything else ignored).
fn exclude_patterns(map: &serde_yaml::Mapping) -> Option<impl Iterator<Item = String>> {
    let excludes = map.get(Value::from("analyzer"))?.as_mapping()?;
    match excludes.get(Value::from("exclude")) {
        Some(Value::Sequence(list)) => Some(
            list.iter()
                .filter_map(|pattern| pattern.as_str().map(str::to_owned)),
        ),
        _ => None,
    }
}

/// The `include:` URI strings of one options file: a single string or a list
/// whose string entries are taken in order (parser:589-602; non-string
/// entries are ignored there too).
fn include_uris(yaml: &Value) -> Vec<String> {
    let Some(map) = yaml.as_mapping() else {
        return Vec::new();
    };
    match map.get(Value::from("include")) {
        Some(Value::String(uri)) => vec![uri.clone()],
        Some(Value::Sequence(list)) => list
            .iter()
            .filter_map(|entry| entry.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

/// Resolves one include URI the way `sourceFactory.resolveUri` does against
/// the including file: relative paths against its directory, `package:` URIs
/// through the package config, `file:` URIs absolute; `None` for schemes
/// with no filesystem target (the include is then skipped).
fn resolve_include(
    uri: &str,
    including_file: &Path,
    packages: &PackageConfig,
    config_dir: &Path,
) -> Option<PathBuf> {
    if let Some(rest) = uri.strip_prefix("package:") {
        let (name, subpath) = rest.split_once('/')?;
        if name.is_empty() || subpath.is_empty() {
            return None;
        }
        let entry = packages.entry(name)?;
        let root = entry.resolve_root(config_dir).ok()?;
        let path = root.join(&entry.package_uri).join(percent_decode(subpath));
        Some(dartq_pub::normalize(&path))
    } else if let Some(rest) = uri.strip_prefix("file:") {
        let path = rest.strip_prefix("//").unwrap_or(rest);
        if !path.starts_with('/') {
            return None;
        }
        Some(PathBuf::from(percent_decode(path)))
    } else if uri.contains(':') {
        // Any other scheme (`dart:`, `http:`, …) has no options-file target.
        None
    } else {
        let base = including_file.parent().unwrap_or(Path::new(""));
        Some(dartq_pub::normalize(&base.join(percent_decode(uri))))
    }
}

/// Decodes `%XX` escapes (the URI→path step of `FileSource` resolution);
/// text without `%` passes through untouched.
fn percent_decode(text: &str) -> String {
    if !text.contains('%') {
        return text.to_owned();
    }
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let Some(byte) = bytes
                .get(index + 1..index + 3)
                .and_then(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        {
            decoded.push(byte);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}
