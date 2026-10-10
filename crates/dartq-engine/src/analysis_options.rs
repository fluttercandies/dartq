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
//! - **Severity overrides** — `analyzer.errors` is a per-code action map
//!   (`_ParsedErrorProcessorsData.parse`, `analysis_options_parse_model.dart`
//!   :541-631): the action is stringified and lowercased
//!   (`ErrorConfig._processMap`, `source/error_processor.dart`:36-54), and
//!   `ignore`/`false` filters the code out while `error`/`warning`/`info`
//!   re-levels it (`severityMap`, `error_processor.dart`:14-18); any other
//!   scalar contributes nothing. The include chain merges per code, a nearer
//!   file's entry replacing the farther one (`_applyErrorProcessors`'
//!   `processorsByCode` rebuild, parser:219-230), and the application point
//!   defers to the override before the declared severity (`computeSeverity`,
//!   `analyzer_cli error_severity.dart`:14-26) — a filtered code is dropped
//!   from the output entirely.
//! - **Cannot-ignore** — `analyzer.cannot-ignore` lists the codes that
//!   `// ignore:` comments cannot suppress (`_ParsedCannotIgnoreData.parse`,
//!   parse_model:346-405; `_applyCannotIgnore`, parser:173-197): explicit
//!   names lowercase into the set, while a severity name (`error`/`warning`/
//!   `info`) marks every registry code whose effective severity — this
//!   file's `errors` overrides first, else the registry severity, since the
//!   errors section applies before cannot-ignore (parser:166-167) — equals
//!   it. Enforcement is the suppression gate shape
//!   (library_analyzer.dart:552-563): a code in the set survives every
//!   ignore-comment suppression.
//! - **Language strictness scalars** — `analyzer.language.strict-casts` /
//!   `strict-inference` / `strict-raw-types` parse as booleans (YAML
//!   booleans or `true`/`false` strings, the `toBool` conversion,
//!   util/yaml.dart:156-169), set-if-present across the chain
//!   (`_ParsedLanguageData.parse`, parse_model:904-998; `_applyLanguage`,
//!   parser:250-260). Parsed and exposed only — the semantic consumption is
//!   #480's.
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

use dartq_diagnostics::{CodeRegistry, Severity};
use dartq_pub::PackageConfig;

/// The options file name the analyzer locates walking up from a context
/// root (`ContextLocator.locate`).
const OPTIONS_FILE_NAME: &str = "analysis_options.yaml";

/// One package root's fully merged analysis options (the
/// `ResolvedAnalysisOptions` slice PRD §12.1 names, limited to the sections
/// this engine consumes: the lint rule set, the exclude globs, the
/// `errors`/`cannot-ignore` severity face, and the language strictness
/// scalars — the latter parsed and exposed only, #480 consumes them).
#[derive(Debug, Default)]
pub struct AnalysisOptions {
    /// Effective enabled lint rule names — the union across the include
    /// chain with nearer entries winning per name, sorted and deduplicated
    /// (the data the lint engine resolves its enable set from).
    pub lint_rules: Vec<String>,
    /// `analyzer.language.strict-casts` (`parse_model:904-998`), merged
    /// set-if-present across the include chain. Parsed and exposed only;
    /// #480 consumes it.
    pub strict_casts: Option<bool>,
    /// `analyzer.language.strict-inference`, same merge and caveat as
    /// [`AnalysisOptions::strict_casts`].
    pub strict_inference: Option<bool>,
    /// `analyzer.language.strict-raw-types`, same merge and caveat as
    /// [`AnalysisOptions::strict_casts`].
    pub strict_raw_types: Option<bool>,
    /// Per-code severity overrides, keyed by lowercased code name; `None`
    /// filters the code, `Some` re-levels it (the official
    /// `ErrorProcessor` list's per-code map form).
    severity_overrides: FxHashMap<String, Option<Severity>>,
    /// Lowercased codes `// ignore:` comments cannot suppress (the official
    /// `unignorableDiagnosticCodeNames`).
    unignorable: FxHashSet<String>,
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

    /// The severity one diagnostic surfaces at: an `analyzer.errors`
    /// override for `code` (lowercase name) defers to it — `None` drops the
    /// diagnostic from the output, `Some` re-levels it — and without one the
    /// declared severity stands (`computeSeverity`,
    /// `analyzer_cli error_severity.dart`:14-26: "if there is a processor
    /// for this error, defer to it").
    #[must_use]
    pub fn processed_severity(&self, code: &str, declared: Severity) -> Option<Severity> {
        match self.severity_overrides.get(code) {
            Some(action) => *action,
            None => Some(declared),
        }
    }

    /// Whether `// ignore:`-comment suppression may drop `code` (lowercase
    /// name): codes in the `analyzer.cannot-ignore` set survive every
    /// suppression (library_analyzer.dart:552-563 — "don't allow
    /// un-ignorable codes to be ignored"). The suppression faces consult
    /// this at the same application point as [`AnalysisOptions::processed_severity`].
    #[must_use]
    pub fn is_ignorable(&self, code: &str) -> bool {
        !self.unignorable.contains(code)
    }

    /// Whether any `analyzer.errors` override merged in (the
    /// severity-application fast path decides with this: without overrides
    /// the application is the identity).
    pub(crate) fn has_severity_overrides(&self) -> bool {
        !self.severity_overrides.is_empty()
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
    /// Per-code severity overrides; a nearer file's entry overwrites the
    /// code (the `processorsByCode` rebuild, parser:219-230).
    errors: FxHashMap<String, Option<Severity>>,
    /// The accumulated `unignorableDiagnosticCodeNames` (lowercased).
    unignorable: FxHashSet<String>,
    strict_casts: Option<bool>,
    strict_inference: Option<bool>,
    strict_raw_types: Option<bool>,
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
    /// `_ApplyState.apply`, the sections this engine consumes). The section
    /// order mirrors `_applyAnalyzer` (parser:164-171) where it matters:
    /// `errors` before `cannot-ignore`, so the severity arms see this
    /// file's re-levels.
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
        for (code, action) in severity_override_entries(map) {
            self.errors.insert(code, action);
        }
        apply_cannot_ignore(map, &mut self.unignorable, &self.errors);
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
        let (casts, inference, raw_types) = language_scalars(map);
        self.strict_casts = casts.or(self.strict_casts);
        self.strict_inference = inference.or(self.strict_inference);
        self.strict_raw_types = raw_types.or(self.strict_raw_types);
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
            strict_casts: self.strict_casts,
            strict_inference: self.strict_inference,
            strict_raw_types: self.strict_raw_types,
            severity_overrides: self.errors,
            unignorable: self.unignorable,
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

/// The `analyzer.errors` per-code actions (`_ParsedErrorProcessorsData.parse`,
/// `analysis_options_parse_model.dart`:541-631): a map of string code →
/// scalar action, the action stringified and lowercased
/// (`ErrorConfig._processMap`, `error_processor.dart`:36-54) —
/// `ignore`/`false` filters the code (`ignoreSynonyms`,
/// `analysis_options_file.dart`:46), `error`/`warning`/`info` re-levels it
/// (`severityMap`, `error_processor.dart`:14-18), and any other scalar
/// contributes nothing (the official reports and skips). The merged code is
/// lowercased (the `ErrorProcessor` constructor, `error_processor.dart`:71);
/// non-string keys and non-map sections are skipped/invalid.
fn severity_override_entries(map: &serde_yaml::Mapping) -> Vec<(String, Option<Severity>)> {
    let Some(entries) = map
        .get(Value::from("analyzer"))
        .and_then(|analyzer| analyzer.as_mapping())
        .and_then(|analyzer| analyzer.get(Value::from("errors")))
        .and_then(|errors| errors.as_mapping())
    else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|(key, value)| {
            let code = key.as_str()?.to_lowercase();
            let action = action_scalar(value)?;
            let action = match action.as_str() {
                "ignore" | "false" => None,
                "error" => Some(Severity::Error),
                "warning" => Some(Severity::Warning),
                "info" => Some(Severity::Info),
                _ => return None,
            };
            Some((code, action))
        })
        .collect()
}

/// One `errors` action scalar, stringified and lowercased the way
/// `valueNode.value.toString().toLowerCase()` does: strings verbatim, YAML
/// booleans as `true`/`false` (so bare `code: false` filters); everything
/// else (numbers, null, containers) is illegal.
fn action_scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.to_lowercase()),
        Value::Bool(enabled) => Some(if *enabled { "true" } else { "false" }.to_owned()),
        _ => None,
    }
}

/// Applies one file's `analyzer.cannot-ignore` list (the official
/// `_applyCannotIgnore`, parser:173-197): a severity name marks every
/// registry code whose effective severity — `errors`, the overrides merged
/// through this file, else the registry severity — equals it; every other
/// name lowercases into the unignorable set (names are not
/// registry-validated there either).
fn apply_cannot_ignore(
    map: &serde_yaml::Mapping,
    unignorable: &mut FxHashSet<String>,
    errors: &FxHashMap<String, Option<Severity>>,
) {
    let Some(names) = cannot_ignore_names(map) else {
        return;
    };
    let arms: Vec<Severity> = names
        .iter()
        .filter_map(|name| severity_of_name(name))
        .collect();
    if !arms.is_empty() {
        let registry = CodeRegistry::builtin();
        for arm in arms {
            for code in registry.iter() {
                let effective = match errors.get(code.name) {
                    Some(Some(severity)) => Some(*severity),
                    Some(None) => None,
                    None => Some(code.dtype.severity()),
                };
                if effective == Some(arm) {
                    unignorable.insert(code.name.to_owned());
                }
            }
        }
    }
    for name in names {
        if severity_of_name(&name).is_none() {
            unignorable.insert(name.to_lowercase());
        }
    }
}

/// The `analyzer.cannot-ignore` name strings (`_ParsedCannotIgnoreData.parse`,
/// parse_model:346-405): list-only, string entries only, everything else
/// ignored.
fn cannot_ignore_names(map: &serde_yaml::Mapping) -> Option<Vec<String>> {
    let analyzer = map.get(Value::from("analyzer"))?.as_mapping()?;
    match analyzer.get(Value::from("cannot-ignore")) {
        Some(Value::Sequence(list)) => Some(
            list.iter()
                .filter_map(|name| name.as_str().map(str::to_owned))
                .collect(),
        ),
        _ => None,
    }
}

/// The severity a `cannot-ignore` arm names (`severityMap[name]`,
/// error_processor.dart:14-18 — case-sensitive, so `ERROR` is an explicit
/// code name, not an arm).
fn severity_of_name(name: &str) -> Option<Severity> {
    match name {
        "error" => Some(Severity::Error),
        "warning" => Some(Severity::Warning),
        "info" => Some(Severity::Info),
        _ => None,
    }
}

/// The `analyzer.language` strictness booleans (`_ParsedLanguageData.parse`,
/// parse_model:904-998): map-only section; the three strict-* keys parse
/// through the `toBool` conversion (util/yaml.dart:156-169 — YAML booleans
/// or `true`/`false` strings, case-insensitive), non-boolean values and
/// unknown keys contribute nothing. `(casts, inference, raw_types)`.
fn language_scalars(map: &serde_yaml::Mapping) -> (Option<bool>, Option<bool>, Option<bool>) {
    let Some(language) = map
        .get(Value::from("analyzer"))
        .and_then(|analyzer| analyzer.as_mapping())
        .and_then(|analyzer| analyzer.get(Value::from("language")))
        .and_then(|language| language.as_mapping())
    else {
        return (None, None, None);
    };
    let parse = |key: &str| -> Option<bool> {
        match language.get(Value::from(key)) {
            Some(Value::Bool(value)) => Some(*value),
            Some(Value::String(text)) => match text.to_lowercase().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            },
            _ => None,
        }
    };
    (
        parse("strict-casts"),
        parse("strict-inference"),
        parse("strict-raw-types"),
    )
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
