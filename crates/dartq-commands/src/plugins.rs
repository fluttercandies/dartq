//! Plugin lifecycle orchestration for the CLI analyze face: discovery,
//! lazy spawn at first use, the per-cycle engine-diagnostics feed, the
//! crash→diagnostic bridge, and the deterministic merge of plugin findings
//! into the diagnostics output — the `dartq analyze` counterpart of the
//! daemon's plugin weave (`dartq-daemon`'s plugin set), built over the same
//! `dartq-plugin` runtime the daemon consumes.
//!
//! Upstream `dart analyze` loads plugins by default (`--plugins`
//! `defaultsTo: true`, hidden; `--no-plugins` disables); this is the face
//! the CLI's analyze pipeline consults when the effective options file
//! declares a `plugins:` section and the flag did not disable it.
//!
//! # Representation contract (the merge's type-level boundary)
//!
//! Plugin *findings* convert to engine [`Diagnostic`]s here — unlike the
//! daemon (which converts them to `wire::Diagnostic`), the CLI analyze
//! output is engine-shaped end to end, so the finding's free-form code is
//! interned as a `&'static` [`CodeInfo`] (the `Box::leak` intern table is
//! bounded by the distinct code count and lives for the one-shot CLI
//! process). Only that interning differs from the daemon; the cycle shape —
//! lazy launch, census resolution, the bounded drain, the crash-row
//! isolation, verbatim findings — is the daemon's semantics verbatim.
//!
//! # Blocking posture (synchronous-core policy)
//!
//! Every face here blocks the calling thread — the analyze run's thread —
//! by design: [`PluginSet::launch`] spends only the discovery walks (no
//! sidecar starts there); the first spawn handshake lands inside
//! [`PluginSet::cycle`]'s launch pass on the first cycle that has something
//! to feed, and the cycle spends at most one [`DRAIN_BUDGET`] per plugin
//! waiting for findings.
//!
//! # Known-accepted postures (the daemon's rulings, carried over)
//!
//! - `feed` with no diagnostics and no analyzed files sends nothing: there
//!   is no "diagnostics cleared" signal to a plugin. A cycle with analyzed
//!   files but an empty diagnostic set still feeds — the enabled rules must
//!   run over the files even when the engine has nothing to report.
//! - A plugin whose capability is never exercised never spawns: the launch
//!   pass runs only on cycles that would feed, so a plugin-free workspace
//!   keeps the sidecar plane at zero resident processes.
//! - The restart budget never resets within the run.
//! - A crash row recorded while the run has no anchor file is withheld, not
//!   dropped: it publishes on the first cycle that has one.
//! - A plugin row whose offsets fall outside the resident file clamps to
//!   the file's UTF-16 end: the row publishes degraded, never panics the
//!   mapping.
//! - The `plugins:` options section is read root-file-only: includes and
//!   nested options directories are not consulted.
//! - The suppression face of `analyzer.errors` does not reach plugin rows:
//!   the pushed config carries re-levelings only, and `// ignore:` handling
//!   for plugin codes is the engine output face's concern.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use dartq_diagnostics::{CodeInfo, Diagnostic, DiagnosticType, FileId, Severity};
use dartq_engine::Engine;
use dartq_index::{DeclRow, FileCapture};
use dartq_plugin::{
    CycleConfig, CyclePush, PendingQuery, PluginDiagnostic, PluginRuntime, PubNative, QueryKind,
    RuleCensus, RuleKind, RuntimeError, Severity as PluginSeverity, SpawnSpec, crash_diagnostic,
    discover,
};
use dartq_pub::PackageConfig;
use dartq_source::{FileTable, LineIndex, SourceRange, utf16_len_of};
use rustc_hash::{FxHashMap, FxHashSet};
use serde_json::{Value, json};
use serde_yaml::Value as Yaml;

use crate::fix::{DeclaredPlugin, declared_plugin_sources, plugin_package_config};
use crate::query as commands_query;

/// Per-cycle wait for plugin findings (the bounded drain): long enough for
/// a warm plugin's push, short enough to keep the analyze run responsive.
/// Rows arriving later are dropped at process end (the CLI runs one cycle).
const DRAIN_BUDGET: Duration = Duration::from_millis(50);

/// The severity a rule-config entry assigns, mirroring the official
/// `ConfiguredSeverity` (`package:analyzer/src/lint/config.dart`): a
/// boolean maps to enable/disable, a severity name re-levels, and an
/// unknown name degrades to enable (`_parseRuleConfig`). `Enable` leaves
/// the declared severity standing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConfiguredSeverity {
    Enable,
    Disable,
    Error,
    Warning,
    Info,
}

impl ConfiguredSeverity {
    /// Official `RuleConfig.isEnabled`: every severity but `Disable`.
    const fn is_enabled(self) -> bool {
        !matches!(self, Self::Disable)
    }

    /// The wire severity of a re-leveling entry; `None` when the entry does
    /// not re-level (`Enable`/`Disable` — the enablement face, not
    /// severity).
    const fn wire_severity(self) -> Option<PluginSeverity> {
        match self {
            Self::Enable | Self::Disable => None,
            Self::Error => Some(PluginSeverity::Error),
            Self::Warning => Some(PluginSeverity::Warning),
            Self::Info => Some(PluginSeverity::Info),
        }
    }

    /// Parses one rule-config scalar: `true`/`false` or a severity name
    /// (case-insensitive; unknown names degrade to enable).
    fn parse(value: &Yaml) -> Self {
        match value {
            Yaml::Bool(false) => Self::Disable,
            Yaml::String(name) => match name.to_ascii_lowercase().as_str() {
                "disable" | "false" => Self::Disable,
                "error" => Self::Error,
                "warning" => Self::Warning,
                "info" => Self::Info,
                _ => Self::Enable,
            },
            _ => Self::Enable,
        }
    }
}

/// One plugin's entry in the governing options' `plugins:` section: the
/// `diagnostics:` rule configs (name → severity), keyed lower-cased — the
/// official enablement face for plugin rules
/// (`_computeDiagnosticsFromPlugin`: `registry.enabled(configuration.
/// diagnosticConfigs)`), not the `linter.rules` section.
#[derive(Default)]
struct PluginOptionsConfig {
    diagnostics: FxHashMap<String, ConfiguredSeverity>,
}

/// The parsed `plugins:` section of the governing root analysis options
/// file, keyed by plugin name (lower-cased). The default is the empty
/// section: with no configuration, lint rules stay disabled and warning
/// rules stay enabled — the official no-configuration posture.
#[derive(Default)]
pub struct PluginOptionsConfigs {
    plugins: FxHashMap<String, PluginOptionsConfig>,
}

impl PluginOptionsConfigs {
    /// The string scalar of a YAML node (mapping keys are scalars; anything
    /// else is skipped).
    fn scalar_text(node: &Yaml) -> Option<&str> {
        match node {
            Yaml::String(text) => Some(text),
            _ => None,
        }
    }

    /// Loads and parses [path]'s top-level `plugins:` section. Any failure
    /// degrades to the empty section: the engine's options face already
    /// reports options diagnostics, and an unreadable plugin configuration
    /// must never take the cycle down.
    pub fn load(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        Some(Self::parse(&text))
    }

    /// Parses the `plugins:` section out of one options file's YAML text.
    /// Root-file-only semantics: the include chain is not followed.
    fn parse(text: &str) -> Self {
        let Ok(Yaml::Mapping(root)) = serde_yaml::from_str(text) else {
            return Self::default();
        };
        let Some(Yaml::Mapping(plugins)) = root.get("plugins") else {
            return Self::default();
        };
        let mut parsed = Self::default();
        for (name_node, config_node) in plugins {
            let Some(name) = Self::scalar_text(name_node) else {
                continue;
            };
            let Yaml::Mapping(config) = config_node else {
                continue;
            };
            if name.eq_ignore_ascii_case("dependency_overrides") {
                continue;
            }
            let Some(Yaml::Mapping(diagnostics)) = config.get("diagnostics") else {
                continue;
            };
            let mut entry = PluginOptionsConfig::default();
            for (rule_node, severity_node) in diagnostics {
                let Some(rule) = Self::scalar_text(rule_node) else {
                    continue;
                };
                entry.diagnostics.insert(
                    rule.to_ascii_lowercase(),
                    ConfiguredSeverity::parse(severity_node),
                );
            }
            parsed.plugins.insert(name.to_ascii_lowercase(), entry);
        }
        parsed
    }

    /// The configuration of the named plugin (matched case-insensitively).
    fn for_plugin(&self, name: &str) -> Option<&PluginOptionsConfig> {
        self.plugins.get(&name.to_ascii_lowercase())
    }
}

/// One analyze cycle's caller-side context: what the plugin face needs
/// beyond the engine diagnostics themselves.
pub struct CycleContext<'a> {
    /// The cycle's analyzed file set (wire-form paths) — the files a
    /// rule-executing plugin runs its enabled rules over.
    pub analyzed: &'a [String],
    /// The open-document overlays (path, unsaved text): the edited view of
    /// the analyzed set. Files not handed here read from disk.
    pub overlays: &'a [(String, String)],
    /// The governing root analysis options file, when one exists — the
    /// source of the `plugins:` rule configs enablement resolves against.
    pub options_file: Option<&'a Path>,
    /// The reverse-query server: answers the plugins' `request/query` pulls
    /// against the engine's current view — the analyze face always has one
    /// (it owns the engine). [`serve_query`] is the canonical
    /// implementation.
    pub query_server: Option<QueryServer<'a>>,
}

/// A reverse-query answer: the reply params (`Err` answers the wire's
/// error channel).
type QueryAnswer = Result<serde_json::Map<String, Value>, String>;

/// A reverse-query server: answers one pending query with its reply params.
type QueryServer<'a> = &'a dyn Fn(&PendingQuery) -> QueryAnswer;

/// One rule of a census, resolved for the cycle: enablement plus the
/// severity overrides for its codes.
struct ResolvedRule {
    name_lower: String,
    enabled: bool,
    code_overrides: Vec<(String, PluginSeverity)>,
}

/// Resolves a plugin's census against its options configuration with the
/// official `RegistryBase::enabled` semantics: warning rules enabled unless
/// a config disables them, lint rules enabled only by an explicit config.
/// Severity overrides come from the same entries keyed by code name.
fn resolve_census(
    census: &[RuleCensus],
    config: Option<&PluginOptionsConfig>,
) -> Vec<ResolvedRule> {
    census
        .iter()
        .map(|rule| {
            let name_lower = rule.name.to_ascii_lowercase();
            let entry = config.and_then(|config| config.diagnostics.get(&name_lower));
            let enabled = match rule.kind {
                RuleKind::Warning => entry.is_none_or(|severity| severity.is_enabled()),
                RuleKind::Lint => entry.is_some_and(|severity| severity.is_enabled()),
            };
            let code_overrides = rule
                .codes
                .iter()
                .filter_map(|code| {
                    let severity = config
                        .and_then(|config| config.diagnostics.get(&code.name.to_ascii_lowercase()))
                        .copied()
                        .and_then(ConfiguredSeverity::wire_severity)?;
                    Some((code.name.to_ascii_lowercase(), severity))
                })
                .collect();
            ResolvedRule {
                name_lower,
                enabled,
                code_overrides,
            }
        })
        .collect()
}

/// Builds the cycle's [`CycleConfig`] from the resolved census: enabled
/// rule names, the merged per-code severity overrides, and the overlays
/// restricted to the analyzed set.
fn cycle_config(
    resolved: &[ResolvedRule],
    overlays: &[(String, String)],
    analyzed: &[String],
) -> CycleConfig {
    let analyzed_set: FxHashSet<&String> = analyzed.iter().collect();
    let mut enabled_rules = Vec::new();
    let mut severity_overrides: BTreeMap<String, PluginSeverity> = BTreeMap::new();
    for rule in resolved {
        if !rule.enabled {
            continue;
        }
        enabled_rules.push(rule.name_lower.clone());
        for (code, severity) in &rule.code_overrides {
            severity_overrides.insert(code.clone(), *severity);
        }
    }
    let overlays = overlays
        .iter()
        .filter(|(file, _)| analyzed_set.contains(file))
        .map(|(file, content)| dartq_plugin::OverlayEntry {
            file: file.clone(),
            content: Some(content.clone()),
        })
        .collect();
    CycleConfig {
        enabled_rules,
        severity_overrides,
        overlays,
    }
}

/// Every plugin discovered under the workspace's `plugins/` convention plus
/// the governing options' pub-native `plugins:` section (including
/// `path:`-valued entries, resolved directly against their directories), in
/// discovery order, each with its lazy runtime or its terminal failure.
///
/// Dropping the set without [`PluginSet::shutdown`] kills the child
/// processes (the runtime's `Drop`) without the graceful handshake — the
/// analyze run's teardown must call [`PluginSet::shutdown`].
#[derive(Default)]
pub struct PluginSet {
    plugins: Vec<PluginSlot>,
}

/// One plugin's run state. `runtime: None` marks the plugin dead for this
/// run: the first surfaced runtime error means the runtime already spent
/// its restart budget reaching it, and further interaction would burn the
/// manifest's request budget on a plugin that cannot come back.
struct PluginSlot {
    /// The manifest name — the crash-message subject and the finding's
    /// provenance tag.
    name: String,
    runtime: Option<PluginRuntime>,
    /// A recorded failure awaiting an anchor file to publish against.
    pending_crash: Option<Box<RuntimeError>>,
    /// At most one crash row per plugin per run: repeats are suppressed.
    crash_emitted: bool,
}

impl PluginSet {
    /// Discovers plugins from the workspace's explicit sources — the
    /// `plugins/` convention and the governing `analysis_options`' pub-native
    /// `plugins:` section resolved through `pub_config` (the parsed
    /// `package_config.json` in `config_dir`, augmented by each
    /// `path:`-declared name's synthetic entry — a pub-resolved entry wins
    /// over a `path:` shadow, matching upstream's precedence). **No sidecar
    /// process starts here**: every runtime is built lazy, and the first
    /// generation spawns at the first cycle that has something to feed.
    ///
    /// The whole boot cost, once per run: the options locator walk and read
    /// plus the `plugins/` convention scan. A workspace that declares no
    /// plugins and ships no `plugins/` directory pays the two absence
    /// probes and allocates nothing; the per-cycle path then reduces to
    /// [`PluginSet::is_empty`].
    ///
    /// Skipped plugins (malformed, missing entrypoint, duplicate name,
    /// declared but unresolved) are logged and ignored. A plugin whose
    /// later spawn fails surfaces through the ordinary crash path on the
    /// cycle that first needs it — one `dartq/plugin_crashed` row, the run
    /// alive.
    #[must_use]
    pub fn launch(workspace_root: &Path, pub_config: &PackageConfig, config_dir: &Path) -> Self {
        let declared = declared_plugin_sources(workspace_root);
        let (options_file, names, decls): (&Path, Vec<String>, &[DeclaredPlugin]) =
            match declared.as_ref() {
                Some((file, decls)) => (
                    file.as_path(),
                    decls.iter().map(|decl| decl.name.clone()).collect(),
                    decls.as_slice(),
                ),
                None => (Path::new(""), Vec::new(), &[]),
            };
        // `path:`-declared names absent from pub's resolution get a
        // synthetic config entry so the unchanged discovery arm resolves
        // them like any pub-native hit; the clone costs only when a `path:`
        // declaration exists.
        let augmented;
        let config = if decls.iter().any(|decl| decl.path.is_some()) {
            augmented = plugin_package_config(pub_config, decls);
            &augmented
        } else {
            pub_config
        };
        let report = discover(
            workspace_root,
            &[],
            &PubNative {
                declared: &names,
                config,
                config_dir,
                options_file,
            },
        );
        for issue in &report.skipped {
            eprintln!(
                "dartq: skipping plugin manifest `{}`: {}",
                issue.path.display(),
                issue.error
            );
        }
        let plugins = report
            .plugins
            .into_iter()
            .map(|plugin| {
                let name = plugin.manifest.name.clone();
                // The analyze face opts into the Query API: the run owns the
                // engine view the reverse queries answer against.
                let spec = SpawnSpec {
                    serve_queries: true,
                    ..SpawnSpec::dart(&plugin.dir, plugin.manifest)
                };
                PluginSlot {
                    name,
                    runtime: Some(PluginRuntime::lazy(spec)),
                    pending_crash: None,
                    crash_emitted: false,
                }
            })
            .collect();
        Self { plugins }
    }

    /// True when no plugin was discovered — the per-cycle hook skips its
    /// work entirely. This is the whole absent-path cost of a plugin-free
    /// workspace's analysis cycle: one emptiness check.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// A set carrying one already-dead plugin: `runtime: None` with a
    /// recorded failure awaiting an anchor, exactly what the cycle's launch
    /// pass records when a first-use spawn fails. Test seam for the weave —
    /// reaching the same state through the launch path needs a real child
    /// process.
    #[cfg(test)]
    pub(crate) fn with_dead_plugin(name: &str, reason: &str) -> Self {
        Self {
            plugins: vec![PluginSlot {
                name: name.to_owned(),
                runtime: None,
                pending_crash: Some(Box::new(RuntimeError::Crashed {
                    reason: reason.to_owned(),
                })),
                crash_emitted: false,
            }],
        }
    }

    /// One analyze cycle against the plugins: pushes the engine's merged
    /// diagnostic set (the lints included by the caller), the cycle's
    /// analyzed file set, and the resolved per-plugin execution config;
    /// waits [`DRAIN_BUDGET`] per live plugin for findings; and returns both
    /// the findings (resolved onto resident files as engine `Diagnostic`s)
    /// and the crash rows (already engine `Diagnostic`s).
    ///
    /// `anchor` is the crash anchor — a `FileId` the run's diagnostics name;
    /// `None` when nothing exists yet, in which case crash rows are withheld
    /// until a cycle that has one. At most one is emitted per plugin per
    /// run.
    #[must_use]
    pub fn cycle(
        &mut self,
        engine: &[Diagnostic],
        files: &FileTable,
        anchor: Option<FileId>,
        context: &CycleContext<'_>,
    ) -> Cycle {
        let mut cycle = Cycle {
            findings: Vec::new(),
            crashes: Vec::new(),
        };
        if !engine.is_empty() || !context.analyzed.is_empty() {
            for slot in &mut self.plugins {
                let Some(runtime) = slot.runtime.as_mut() else {
                    continue;
                };
                if let Err(error) = runtime.ensure_alive() {
                    fail_slot(slot, error, anchor, &mut cycle);
                }
            }
        }
        let has_rules = self.plugins.iter().any(|slot| {
            slot.runtime
                .as_ref()
                .is_some_and(|runtime| !runtime.rule_census().is_empty())
        });
        let configs = if has_rules {
            context
                .options_file
                .and_then(PluginOptionsConfigs::load)
                .unwrap_or_default()
        } else {
            PluginOptionsConfigs::default()
        };
        let resolved_per_plugin: Vec<(String, CycleConfig, FxHashMap<String, RuleKind>)> = self
            .plugins
            .iter()
            .map(|slot| {
                let census = slot
                    .runtime
                    .as_ref()
                    .map(PluginRuntime::rule_census)
                    .unwrap_or_default();
                let resolved = resolve_census(&census, configs.for_plugin(&slot.name));
                // The finding conversion's family lookup: every census code
                // to its rule's kind (a code an uncensused emission names
                // falls back to `STATIC_WARNING` at conversion).
                let kinds = census
                    .iter()
                    .flat_map(|rule| {
                        rule.codes
                            .iter()
                            .map(move |code| (code.name.to_ascii_lowercase(), rule.kind))
                    })
                    .collect();
                let config = cycle_config(&resolved, context.overlays, context.analyzed);
                (slot.name.clone(), config, kinds)
            })
            .collect();
        for (index, slot) in self.plugins.iter_mut().enumerate() {
            // A crash recorded before any anchor existed publishes here.
            if let (Some(error), Some(anchor)) = (&slot.pending_crash, anchor) {
                cycle
                    .crashes
                    .push(crash_diagnostic(&slot.name, error, anchor));
                slot.pending_crash = None;
                slot.crash_emitted = true;
            }
            let Some(runtime) = slot.runtime.as_mut() else {
                continue;
            };
            let (_, config, kinds) = &resolved_per_plugin[index];
            let push = CyclePush {
                analyzed: context.analyzed.to_vec(),
                config: config.clone(),
            };
            let mut serve = context
                .query_server
                .map(|query_server| move |query: &PendingQuery| query_server(query));
            let server: Option<&mut dyn FnMut(&PendingQuery) -> QueryAnswer> = match serve.as_mut()
            {
                Some(serve) => Some(serve),
                None => None,
            };
            let started = Instant::now();
            let outcome = runtime
                .feed(engine, files, &push)
                .and_then(|()| pump_plugin(runtime, server, DRAIN_BUDGET));
            match outcome {
                Ok((rows, degraded)) => {
                    if degraded {
                        eprintln!(
                            "dartq: plugin `{}` exceeded its {} ms drain budget; remaining rows are dropped",
                            slot.name,
                            DRAIN_BUDGET.as_millis(),
                        );
                    }
                    runtime.observe_cycle(started.elapsed(), degraded);
                    accept_rows(&mut cycle, &slot.name, kinds, rows);
                }
                Err(error) => {
                    // First surfaced error ends the plugin's run (the
                    // isolation contract: the runtime has already spent its
                    // budget; the engine turn stays alive).
                    fail_slot(slot, error, anchor, &mut cycle);
                }
            }
        }
        cycle
    }

    /// The graceful run-end teardown: `shutdown` per live runtime (the
    /// graceful handshake; `Drop` alone is kill-only). Errors are logged,
    /// never propagated — teardown quality does not affect the run's
    /// outcome.
    pub fn shutdown(self) {
        for slot in self.plugins {
            if let Some(runtime) = slot.runtime
                && let Err(error) = runtime.shutdown()
            {
                eprintln!("dartq: plugin `{}` shutdown failed: {error}", slot.name);
            }
        }
    }
}

/// One plugin's drained rows join the cycle's findings — verbatim, in wire
/// arrival order, one batch per emitting plugin, carrying the plugin's
/// census-derived `code → RuleKind` map so the finding conversion picks the
/// diagnostic family (lint codes `LINT`, warning codes `STATIC_WARNING`,
/// uncensused codes `STATIC_WARNING`). Upstream `mergeAnalysisErrors`
/// concatenates every plugin's diagnostics verbatim, duplicates included,
/// so a claimed-code collision drops nothing.
fn accept_rows(
    cycle: &mut Cycle,
    name: &str,
    kinds: &FxHashMap<String, RuleKind>,
    rows: Vec<PluginDiagnostic>,
) {
    if rows.is_empty() {
        return;
    }
    cycle.findings.push(PluginBatch {
        name: name.to_owned(),
        kinds: kinds.clone(),
        rows,
    });
}

/// Ends a slot's run at its first surfaced runtime error: the runtime is
/// dropped (the isolation contract), and the failure records as the
/// plugin's one-shot `dartq/plugin_crashed` row, published against `anchor`
/// in this cycle or withheld (pending) until a cycle that has one.
fn fail_slot(
    slot: &mut PluginSlot,
    error: RuntimeError,
    anchor: Option<FileId>,
    cycle: &mut Cycle,
) {
    slot.runtime = None;
    if slot.crash_emitted {
        return;
    }
    if let Some(anchor) = anchor {
        cycle
            .crashes
            .push(crash_diagnostic(&slot.name, &error, anchor));
        slot.crash_emitted = true;
    } else {
        slot.pending_crash = Some(Box::new(error));
    }
}

/// One cycle's plugin output.
pub struct Cycle {
    /// Findings per emitting plugin, in discovery order; empty batches are
    /// omitted.
    pub findings: Vec<PluginBatch>,
    /// Crash rows beside the findings — engine `Diagnostic`s the caller's
    /// merge appends (at most one per plugin per run).
    pub crashes: Vec<Diagnostic>,
}

/// One plugin's drained findings, in wire arrival order, plus the
/// census-derived `code → RuleKind` map the diagnostic-family conversion
/// resolves against.
pub struct PluginBatch {
    /// The manifest name (the finding's provenance tag).
    pub name: String,
    /// The plugin's census codes to their rule kind — the diagnostic
    /// family lookup ([`intern_code`]); empty for a rule-less plugin.
    kinds: FxHashMap<String, RuleKind>,
    pub rows: Vec<PluginDiagnostic>,
}

/// Resolves drained plugin rows onto the files they name and converts them
/// to engine `Diagnostic`s, in (plugin discovery order, offset) merge order.
/// `resident` answers a `FileId` with its line index and text (the engine
/// faces); rows on files the engine does not know are skipped — the same
/// posture the transport's own feed takes for unknown `FileId`s.
///
/// The finding's free-form code interns as a `&'static` [`CodeInfo`]
/// (leaked — bounded by the distinct code count, reclaimed at the one-shot
/// CLI process's exit): a code its emitting plugin's census names keeps the
/// census rule's kind (lint → `LINT`, warning → `STATIC_WARNING`), an
/// uncensused code `STATIC_WARNING` (upstream's new-protocol plugin-row
/// default).
#[must_use]
pub fn resolve_findings<'a>(
    batches: &[PluginBatch],
    files: &FileTable,
    mut resident: impl FnMut(FileId) -> Option<(&'a LineIndex, &'a str)>,
) -> Vec<Diagnostic> {
    let mut interned: FxHashMap<String, CodeInfo> = FxHashMap::default();
    let mut findings: Vec<Diagnostic> = Vec::new();
    for batch in batches {
        // One plugin's rows: offset-sorted within the plugin (the stable
        // sort keeps wire arrival order for equal offsets), the plugin
        // groups concatenated in discovery order — the merge order.
        let mut rows: Vec<(u32, Diagnostic)> = Vec::new();
        for row in &batch.rows {
            let Some(file) = files.try_file_id(Path::new(&row.file)) else {
                continue;
            };
            let Some((index, text)) = resident(file) else {
                continue;
            };
            rows.push((
                u32::try_from(row.offset).unwrap_or(u32::MAX),
                to_diagnostic(batch, row, file, index, text, &mut interned),
            ));
        }
        rows.sort_by_key(|(offset, _)| *offset);
        findings.extend(rows.into_iter().map(|(_, diagnostic)| diagnostic));
    }
    findings
}

/// The code's interned [`CodeInfo`]: the census kind picks the diagnostic
/// family (a lint-rule code is `LINT`, a warning-rule or uncensused code
/// `STATIC_WARNING`), the machine form is the uppercase code. One intern
/// per distinct code per run — the `FxHashMap` keeps the leak at the
/// distinct count.
fn intern_code(code: &str, kind: RuleKind, interned: &mut FxHashMap<String, CodeInfo>) -> CodeInfo {
    if let Some(info) = interned.get(code) {
        return *info;
    }
    let name: &'static str = Box::leak(code.to_ascii_lowercase().into_boxed_str());
    let machine: &'static str = Box::leak(code.to_ascii_uppercase().into_boxed_str());
    let dtype = match kind {
        RuleKind::Lint => DiagnosticType::Lint,
        RuleKind::Warning => DiagnosticType::StaticWarning,
    };
    let info = CodeInfo::new(name, machine, dtype, false);
    interned.insert(code.to_owned(), info);
    info
}

/// Converts one plugin row to an engine [`Diagnostic`]: severity mapped to
/// the engine's three-tier scale, UTF-16 offsets clamped to the resident
/// text's UTF-16 length (a stale row survives a shrink edit by degrading to
/// zero-width at the file end, never tripping the line index's in-range
/// contract), the code interned by [`intern_code`] against the emitting
/// batch's census `code → kind` map.
fn to_diagnostic(
    batch: &PluginBatch,
    row: &PluginDiagnostic,
    file: FileId,
    index: &LineIndex,
    text: &str,
    interned: &mut FxHashMap<String, CodeInfo>,
) -> Diagnostic {
    let total = utf16_len_of(text);
    let offset = u32::try_from(row.offset).unwrap_or(u32::MAX).min(total);
    let end = u32::try_from(row.offset.saturating_add(row.length))
        .unwrap_or(u32::MAX)
        .min(total);
    // `index` is the caller's residency proof: the file has a line index,
    // so its positions are mappable — the clamp above keeps a stale row
    // inside them. No line-column arithmetic runs here (the oracle sort is
    // offset-keyed; the writers resolve positions at render).
    let _ = index;
    let severity = match row.severity {
        PluginSeverity::Error => Severity::Error,
        PluginSeverity::Warning => Severity::Warning,
        PluginSeverity::Info => Severity::Info,
    };
    let kind = batch
        .kinds
        .get(&row.code.to_ascii_lowercase())
        .copied()
        .unwrap_or(RuleKind::Warning);
    let code = intern_code(&row.code, kind, interned);
    Diagnostic::new(
        severity,
        code,
        row.message.clone(),
        file,
        SourceRange {
            offset,
            length: end.saturating_sub(offset),
        },
        None,
    )
}

/// The dedup key of the merge: file, range start offset, code name, and
/// message — the `(file, offset, code, message)` exact-equality in the
/// merged rows' engine coordinates. `message` (not provenance) is the
/// discriminator: a plugin row exactly equal to a built-in row is the same
/// finding twice, and the built-in row wins by seeding `seen` first.
type DedupKey = (u32, u32, String, String);

fn dedup_key(diagnostic: &Diagnostic) -> DedupKey {
    (
        diagnostic.file.index(),
        diagnostic.range.offset,
        diagnostic.code.name.to_owned(),
        diagnostic.message.as_str().to_owned(),
    )
}

/// Merges the run's engine rows (in their existing order) with the resolved
/// plugin findings into one deterministic sequence: engine rows first, then
/// the findings in [`resolve_findings`]' (plugin discovery order, offset)
/// order. A row whose `(file, offset, code, message)` key already appeared
/// is dropped — a plugin row never duplicates a built-in finding (the
/// built-in row, seeded first, wins).
#[must_use]
pub fn merge(engine_rows: Vec<Diagnostic>, findings: Vec<Diagnostic>) -> Vec<Diagnostic> {
    let mut seen: FxHashSet<DedupKey> = engine_rows.iter().map(dedup_key).collect();
    let mut merged = engine_rows;
    for diagnostic in findings {
        if seen.insert(dedup_key(&diagnostic)) {
            merged.push(diagnostic);
        }
    }
    merged
}

/// One plugin's bounded drain (the cycle's wait half): parks for at most
/// [budget] total, hands back every diagnostic that arrived plus whether
/// the drain was *degraded* — the budget expired before the plugin
/// quiesced — and pumps the plugin's reverse `request/query` frames through
/// [server] as they land. A cycle without a server declines the queries.
fn pump_plugin(
    runtime: &mut PluginRuntime,
    mut server: Option<&mut dyn FnMut(&PendingQuery) -> QueryAnswer>,
    budget: Duration,
) -> Result<(Vec<PluginDiagnostic>, bool), RuntimeError> {
    let deadline = Instant::now() + budget;
    let mut rows = Vec::new();
    loop {
        let drain = runtime.wait_output(deadline.saturating_duration_since(Instant::now()))?;
        rows.extend(drain.diagnostics);
        if drain.pending_queries == 0 {
            return Ok((rows, false));
        }
        match server.as_deref_mut() {
            Some(serve) => {
                runtime.serve_queries(serve)?;
            }
            None => {
                runtime.decline_queued_queries()?;
            }
        }
        if Instant::now() >= deadline {
            return Ok((rows, true));
        }
    }
}

/// The canonical reverse-query server: one `request/query` answered against
/// the run's current engine view. The index kinds route through the shared
/// `dartq-commands` query faces; `element_at` is the offset-keyed face over
/// the engine's maintained index. `Err` rides the wire's error channel — a
/// declined query never tears the run down.
pub fn serve_query(
    engine: &Engine,
    query: &PendingQuery,
) -> Result<serde_json::Map<String, Value>, String> {
    match query.request.kind {
        QueryKind::ElementAt => {
            let file = query.request.file.as_deref().unwrap_or_default();
            let offset = query.request.offset.unwrap_or(0);
            element_at(engine, file, offset)
        }
        kind => {
            let name = query.request.name.as_deref().unwrap_or_default();
            let run = commands_query::kind_after_analysis(engine, name, index_kind(kind));
            match commands_query::to_json(&run, name) {
                Value::Object(map) => Ok(map),
                _ => unreachable!("the query envelope is always a JSON object"),
            }
        }
    }
}

/// The wire query kind → the shared query face's kind (the same five index
/// kinds, one spelling each).
fn index_kind(kind: QueryKind) -> commands_query::QueryKind {
    match kind {
        QueryKind::Symbol => commands_query::QueryKind::Symbol,
        QueryKind::References => commands_query::QueryKind::References,
        QueryKind::Callers => commands_query::QueryKind::Callers,
        QueryKind::Implementations => commands_query::QueryKind::Implementations,
        QueryKind::Dependencies => commands_query::QueryKind::Dependencies,
        QueryKind::ElementAt => unreachable!("routed to `element_at` before this match"),
    }
}

/// The `element_at` answer: the element rows the `{file, offset}` position
/// denotes — its own declaration when the offset falls on a name token,
/// else the declarations the use at the offset resolves to. A miss is an
/// empty `elements` array; an unknown file is an error.
fn element_at(
    engine: &Engine,
    file: &str,
    offset: u64,
) -> Result<serde_json::Map<String, Value>, String> {
    let Some(file_id) = engine.file_table().try_file_id(Path::new(file)) else {
        return Err(format!("unknown file `{file}`"));
    };
    let offset = u32::try_from(offset).unwrap_or(u32::MAX);
    let mut elements: Vec<Value> = Vec::new();
    if let Some(capture) = engine.file_capture(file_id) {
        for row in capture.index.decls() {
            if span_contains(row.range, offset) {
                elements.push(declaration_row(engine, capture, row));
            }
        }
        if elements.is_empty() {
            let library = engine.library_of(file_id);
            for (surface_library, surface) in engine.library_surfaces() {
                if Some(surface_library) != library {
                    continue;
                }
                for reference in surface.refs() {
                    if reference.file != file_id || !span_contains(reference.range, offset) {
                        continue;
                    }
                    let Some(target) = engine.file_capture(reference.target_file) else {
                        continue;
                    };
                    let Some(rows) = target.index.by_name(reference.target_name) else {
                        continue;
                    };
                    elements.extend(rows.iter().map(|row| declaration_row(engine, target, row)));
                }
            }
        }
    }
    let answer = json!({
        "schema_version": crate::SCHEMA_VERSION,
        "file": file,
        "offset": offset,
        "elements": elements,
    });
    let Some(map) = answer.as_object() else {
        unreachable!("the element_at envelope is built as a JSON object");
    };
    Ok(map.clone())
}

/// The frozen declaration-row shape (the query face's `declarations[]`
/// entry): file, container, kind, name, 1-based line/column through the
/// engine's line index, and the UTF-16 name-span length.
fn declaration_row(engine: &Engine, capture: &FileCapture, row: &DeclRow) -> Value {
    let strings = engine.strings();
    let container = (row.container != DeclRow::NONE)
        .then(|| strings.resolve(capture.index.row(row.container).name));
    let (line, column) = line_col(engine, capture.file, row.range.offset);
    let path = engine
        .file_table()
        .path(capture.file)
        .map_or_else(String::new, |path| path.display().to_string());
    json!({
        "file": path,
        "container": container,
        "kind": commands_query::decl_kind_name(row.kind),
        "name": strings.resolve(row.name),
        "line": line,
        "column": column,
        "length": row.range.length,
    })
}

/// The 1-based (line, UTF-16 column) of an offset, the query face's
/// convention. A plugin's offset is untrusted: the clamp keeps
/// `line_col_utf16`'s in-range contract out of a stale query's reach.
fn line_col(engine: &Engine, file: FileId, offset: u32) -> (u32, u32) {
    let index = engine
        .line_index(file)
        .expect("captured file has a line index");
    let text = engine.content(file).expect("captured file has content");
    let text = std::str::from_utf8(text).expect("engine content is UTF-8");
    index.line_col_utf16(text, offset.min(utf16_len_of(text)))
}

/// Whether the UTF-16 [offset] falls inside [range] (half-open; a
/// zero-length span holds nothing).
fn span_contains(range: SourceRange, offset: u32) -> bool {
    range.length > 0 && range.offset <= offset && offset < range.offset.saturating_add(range.length)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use dartq_pub::{PackageConfigEntry, normalize};

    use super::*;

    /// A scratch workspace: `analysis_options.yaml` declaring the named
    /// plugins, an empty `package_config.json`, and a `plugins/` convention
    /// directory when asked.
    struct Workspace(PathBuf);

    impl Workspace {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("dartq-cmd-plugins-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("workspace root");
            Self(root)
        }

        fn options(&self, text: &str) -> PathBuf {
            let file = self.0.join("analysis_options.yaml");
            std::fs::write(&file, text).expect("options");
            file
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn empty_config() -> PackageConfig {
        PackageConfig::parse(r#"{"configVersion": 2, "packages": []}"#).expect("config")
    }

    /// The absent posture: no options file at all yields `None` — the
    /// plugin-free workspace's only cost is the locator walk.
    #[test]
    fn absent_options_file_declares_nothing() {
        let workspace = Workspace::new("absent");
        assert!(declared_plugin_sources(workspace.path()).is_none());
    }

    /// A `plugins:` section naming `path:`-valued entries resolves each
    /// `path` against the options file's parent, normalized — upstream's
    /// `_parsePluginSource` arm order.
    #[test]
    fn path_declarations_resolve_against_the_options_dir() {
        let workspace = Workspace::new("path_decl");
        workspace.options(
            "plugins:\n  p_one:\n    path: ../plugins/one\n  p_two:\n    path: /abs/two\n  p_ver: ^1.0.0\n",
        );
        let (_file, decls) = declared_plugin_sources(workspace.path()).expect("declared");
        let by_name = |name: &str| decls.iter().find(|d| d.name == name);
        // Relative `path:` joins the options dir and normalizes (`..` folds).
        let one = by_name("p_one").expect("p_one");
        assert_eq!(
            one.path.as_deref(),
            Some(dartq_pub::normalize(&workspace.path().join("../plugins/one")).as_path())
        );
        // Absolute `path:` stands.
        assert_eq!(
            by_name("p_two").expect("p_two").path.as_deref(),
            Some(Path::new("/abs/two"))
        );
        // A scalar version constraint never carries a path — pub resolves it.
        assert!(by_name("p_ver").expect("p_ver").path.is_none());
    }

    /// The upstream source precedence: `version`/`hosted`/`git` shadow
    /// `path` (first match wins), so a mapping carrying both yields no
    /// `path:` source.
    #[test]
    fn higher_precedence_sources_shadow_path() {
        let workspace = Workspace::new("precedence");
        workspace.options(
            "plugins:\n  both:\n    version: ^1.0.0\n    path: ../plugins/x\n  gitty:\n    git: https://x/y\n    path: ../plugins/y\n",
        );
        let (_file, decls) = declared_plugin_sources(workspace.path()).expect("declared");
        for decl in &decls {
            assert!(
                decl.path.is_none(),
                "`{}`'s higher-precedence source shadows `path:`",
                decl.name
            );
        }
    }

    /// `plugin_package_config` synthesizes one entry per `path:`-declared
    /// name absent from the real config — and never shadows an entry pub
    /// already resolved.
    #[test]
    fn package_config_augments_only_path_declared_absent_names() {
        let mut config = empty_config();
        // A real pub entry for `resolved` — the synthetic path never wins.
        config.packages.push(PackageConfigEntry {
            name: "resolved".to_owned(),
            root_uri: "file:///real/resolved".to_owned(),
            package_uri: "lib/".to_owned(),
            language_version: None,
            additional_properties: BTreeMap::new(),
        });
        let declared = vec![
            DeclaredPlugin {
                name: "resolved".to_owned(),
                path: Some(PathBuf::from("/synth/resolved")),
            },
            DeclaredPlugin {
                name: "synthetic".to_owned(),
                path: Some(PathBuf::from("/synth/synthetic")),
            },
            DeclaredPlugin {
                name: "pub_only".to_owned(),
                path: None,
            },
        ];
        let augmented = plugin_package_config(&config, &declared);
        // `resolved` keeps pub's entry (the `file:///real` root, not the
        // synthetic one); `synthetic` gains an entry; `pub_only` is untouched.
        assert_eq!(
            augmented.entry("resolved").expect("resolved").root_uri,
            "file:///real/resolved"
        );
        assert_eq!(
            augmented.entry("synthetic").expect("synthetic").root_uri,
            "file:///synth/synthetic"
        );
        assert!(augmented.entry("pub_only").is_none());
    }

    /// The merge's dedup seeds from the engine rows: a plugin row exactly
    /// equal to a built-in finding drops, a differing one appends after the
    /// engine rows in finding order.
    #[test]
    fn merge_dedups_plugin_rows_against_engine_rows() {
        let mut table = FileTable::new();
        let file = table.intern(Path::new("/w/lib/a.dart"));
        let engine_row = Diagnostic::new(
            Severity::Warning,
            CodeInfo::new("unused_x", "UNUSED_X", DiagnosticType::StaticWarning, false),
            "msg".to_owned(),
            file,
            SourceRange {
                offset: 4,
                length: 1,
            },
            None,
        );
        let dup = engine_row.clone();
        let fresh = Diagnostic::new(
            Severity::Info,
            CodeInfo::new("plug_lint", "PLUG_LINT", DiagnosticType::Lint, false),
            "other".to_owned(),
            file,
            SourceRange {
                offset: 9,
                length: 1,
            },
            None,
        );
        let merged = merge(vec![engine_row], vec![dup, fresh.clone()]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[1], fresh, "the distinct plugin row appends");
    }

    /// A dead plugin's recorded crash publishes once, against the cycle's
    /// anchor, as an ERROR `dartq/plugin_crashed` row — and never repeats.
    #[test]
    fn dead_plugin_emits_one_crash_row_per_run() {
        let mut set = PluginSet::with_dead_plugin("demo", "spawn refused");
        let mut files = FileTable::new();
        let file = files.intern(Path::new("/w/lib/a.dart"));
        let analyzed = vec!["x.dart".to_owned()];
        let context = CycleContext {
            analyzed: &analyzed,
            overlays: &[],
            options_file: None,
            query_server: None,
        };
        // A live runtime would feed here; the dead slot only records its
        // crash once the launch pass offers an anchor.
        let cycle = set.cycle(&[], &files, Some(file), &context);
        assert_eq!(cycle.crashes.len(), 1);
        assert_eq!(cycle.crashes[0].code.name, "dartq/plugin_crashed");
        assert_eq!(cycle.crashes[0].severity, Severity::Error);
        // A second cycle emits nothing — once per run.
        let cycle = set.cycle(&[], &files, Some(file), &context);
        assert!(cycle.crashes.is_empty());
    }
}
