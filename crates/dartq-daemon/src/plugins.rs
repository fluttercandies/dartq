//! Plugin lifecycle orchestration for the daemon (PRD §14): discovery,
//! eager spawn, the per-cycle engine-diagnostics feed, the crash→diagnostic
//! bridge, and the deterministic merge of plugin findings into the LSP
//! output. The thin daemon-side layer `dartq-plugin` deliberately leaves to
//! its output consumer.
//!
//! # Representation contract (the merge's type-level boundary)
//!
//! Plugin *findings* convert directly to [`lsp::Diagnostic`] with the
//! plugin's manifest name as the LSP `source` tag. They never pass through
//! the engine [`Diagnostic`] — its `CodeInfo` requires `&'static` code
//! names, and plugin codes are free-form strings. Only
//! [`crash_diagnostic`] output (already a legal engine `Diagnostic`) rides
//! the engine diagnostics vec; the session's convert-once pass then carries
//! it into publishes and answers.
//!
//! # Blocking posture (synchronous-core policy)
//!
//! Every face here blocks the calling thread — the main loop's thread — by
//! design: [`PluginSet::launch`] spends up to the manifest's
//! `request_ms × (retries + 1)` per plugin on the spawn handshake (the
//! manifest defaults 10 s × 4, PRD §14.3); [`PluginSet::cycle`] spends at
//! most one [`DRAIN_BUDGET`] per plugin per cycle waiting for findings.
//! Findings that arrive after a cycle's budget publish on the next cycle.
//!
//! # Known-accepted postures
//!
//! - `feed` with no diagnostics and no analyzed files sends nothing: there
//!   is no "diagnostics cleared" signal to a plugin (the transport has
//!   none). A cycle with analyzed files but an empty diagnostic set still
//!   feeds — the enabled rules must run over the files even when the engine
//!   has nothing to report.
//! - The restart budget never resets (PRD semantics): the runtime relaunches
//!   a dead child lazily within the manifest's `request_retries`, and a
//!   spent budget is terminal for the session.
//! - A crash row recorded while the session has no anchor file is withheld,
//!   not dropped: it publishes on the first cycle that has one.
//! - A plugin row whose offsets fall outside the resident file (the classic
//!   stale row surviving a shrink edit) clamps to the file's UTF-16 end in
//!   [`to_lsp`]: the row publishes degraded, it never panics the mapping.
//! - The `plugins:` options section is read root-file-only (T5): includes
//!   and nested options directories are not consulted, matching the plan's
//!   pinned simplification of the official include-chain merge.
//! - The suppression face of `analyzer.errors` (`processed_severity`
//!   returning `None`) does not reach plugin rows: the pushed config carries
//!   re-levelings only, and `// ignore:` handling for plugin codes is the
//!   engine output face's concern, exactly as it is for the pre-T5 plugin
//!   findings path.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use dartq_diagnostics::{Diagnostic, FileId};
use dartq_plugin::{
    CycleConfig, CyclePush, PluginDiagnostic, PluginRuntime, RuleCensus, RuleKind, RuntimeError,
    Severity as PluginSeverity, SpawnSpec, crash_diagnostic, discover,
};
use dartq_source::{FileTable, LineIndex, utf16_len_of};
use rustc_hash::{FxHashMap, FxHashSet};
use serde_yaml::Value as Yaml;

use crate::lsp;

/// Per-cycle wait for plugin findings (the bounded drain): long enough for a
/// warm plugin's push, short enough to keep the main loop responsive. Rows
/// arriving later publish on the next cycle.
const DRAIN_BUDGET: Duration = Duration::from_millis(50);

/// The severity a rule-config entry assigns, mirroring the official
/// `ConfiguredSeverity` (`package:analyzer/src/lint/config.dart`): a boolean
/// maps to enable/disable, a severity name re-levels, and an unknown name
/// degrades to enable (`_parseRuleConfig`). `Enable` leaves the declared
/// severity standing.
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
    /// not re-level (`Enable`/`Disable` — the enablement face, not severity).
    const fn wire_severity(self) -> Option<PluginSeverity> {
        match self {
            Self::Enable | Self::Disable => None,
            Self::Error => Some(PluginSeverity::Error),
            Self::Warning => Some(PluginSeverity::Warning),
            Self::Info => Some(PluginSeverity::Info),
        }
    }

    /// Parses one rule-config scalar: `true`/`false` or a severity name
    /// (case-insensitive, official `asNameMap()[severityString]`; unknown
    /// names degrade to enable).
    fn parse(value: &Yaml) -> Self {
        match value {
            Yaml::Bool(true) => Self::Enable,
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
/// `diagnostics:` rule configs (name → severity), keyed lower-cased. This is
/// the official enablement face for plugin rules
/// (`_computeDiagnosticsFromPlugin`: `registry.enabled(configuration.diagnosticConfigs)`
/// — not the `linter.rules` section, which configures the analyzer's own
/// lints). Names inside match case-insensitively, so every key is stored
/// lower-cased.
#[derive(Default)]
struct PluginOptionsConfig {
    diagnostics: FxHashMap<String, ConfiguredSeverity>,
}

/// The parsed `plugins:` section of the governing root analysis options
/// file, keyed by plugin name (lower-cased). The default is the empty
/// section: with no configuration, lint rules stay disabled and warning
/// rules stay enabled — the official no-configuration posture of
/// `RegistryBase::enabled({})`.
#[derive(Default)]
pub struct PluginOptionsConfigs {
    plugins: FxHashMap<String, PluginOptionsConfig>,
}

impl PluginOptionsConfigs {
    /// The string scalar of a YAML node (mapping keys are scalars; anything
    /// else is skipped, official `_parseRuleConfig` posture).
    fn scalar_text(node: &Yaml) -> Option<&str> {
        match node {
            Yaml::String(text) => Some(text),
            _ => None,
        }
    }

    /// Loads and parses [path]'s top-level `plugins:` section. Any failure —
    /// unreadable file, malformed YAML, wrong section shape — degrades to
    /// the empty section: the engine's options face already reports options
    /// diagnostics, and an unreadable plugin configuration must never take
    /// the cycle down.
    pub fn load(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        Some(Self::parse(&text))
    }

    /// Parses the `plugins:` section out of one options file's YAML text.
    /// Root-file-only semantics (T5): the include chain is not followed —
    /// the governing root file's own section is the whole configuration.
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
            // `dependency_overrides` is a hosting face (official
            // `AnalysisOptionsFileKeys.dependencyOverrides`), not a plugin
            // entry — never a config key.
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

    /// The configuration of the named plugin (name matched case-insensitively).
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
    /// The open-document overlays (path, unsaved text): the client's edited
    /// view of the analyzed set. Files the host is not handed here read from
    /// disk.
    pub overlays: &'a [(String, String)],
    /// The governing root analysis options file, when one exists — the
    /// source of the `plugins:` rule configs enablement resolves against
    /// (root-file-only semantics, T5).
    pub options_file: Option<&'a Path>,
}

/// One rule of a census, resolved for the cycle: enablement plus the
/// severity overrides for its codes.
struct ResolvedRule {
    name_lower: String,
    enabled: bool,
    code_overrides: Vec<(String, PluginSeverity)>,
}

/// Resolves a plugin's census against its options configuration with the
/// official `RegistryBase::enabled` semantics (`RegistryMixin.enabled`,
/// `package:analyzer/src/lint/registry.dart`): warning rules enabled unless
/// a config disables them, lint rules enabled only by an explicit config.
/// Severity overrides come from the same entries keyed by code name
/// (official `_configuredSeverity`: a re-leveling severity replaces the
/// declared one, `enable` leaves it standing).
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

/// Builds the cycle's [`CycleConfig`] from the resolved census: enabled rule
/// names, the merged per-code severity overrides, and the overlays restricted
/// to the analyzed set.
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

/// Every plugin discovered under the workspace's `plugins/` convention, in
/// discovery order, each with its live runtime or its terminal failure.
///
/// Dropping the set without [`PluginSet::shutdown`] kills the child
/// processes (the runtime's `Drop`) without the graceful handshake — the
/// session end must call [`PluginSet::shutdown`]. `Default` is the empty
/// set: the session takes the set at teardown (`mem::take`), and an
/// all-skipped discovery leaves the empty default in place.
#[derive(Default)]
pub struct PluginSet {
    plugins: Vec<PluginSlot>,
}

/// One plugin's session state. `runtime: None` marks the plugin dead for
/// this session: the first surfaced runtime error means the runtime already
/// spent its restart budget reaching it, and further interaction would burn
/// the manifest's request budget on every cycle for a plugin that cannot
/// come back.
struct PluginSlot {
    /// The manifest name — the LSP `source` tag and the crash-message
    /// subject.
    name: String,
    runtime: Option<PluginRuntime>,
    /// A recorded failure awaiting an anchor file to publish against
    /// (recorded when the failure surfaced before any anchor existed).
    pending_crash: Option<Box<RuntimeError>>,
    /// At most one crash row per plugin per session: repeats are suppressed.
    crash_emitted: bool,
}

impl PluginSet {
    /// Discovers plugins under `<workspace_root>/plugins/` (the session's
    /// `config_dir.parent()` convention; no overrides this milestone) and
    /// spawns each hit eagerly, completing the handshake before returning.
    ///
    /// Spawn failures never fail the session: they are logged here and
    /// surface as their `dartq/plugin_crashed` row on the first cycle with
    /// an anchor file. Skipped manifests (malformed, missing entrypoint,
    /// duplicate name) are logged and ignored.
    #[must_use]
    pub fn launch(workspace_root: &Path) -> Self {
        let report = discover(workspace_root, &[]);
        for issue in &report.skipped {
            eprintln!(
                "dartq-daemon: skipping plugin manifest `{}`: {}",
                issue.path.display(),
                issue.error
            );
        }
        let plugins = report
            .plugins
            .into_iter()
            .map(|plugin| {
                let name = plugin.manifest.name.clone();
                match PluginRuntime::spawn(SpawnSpec::dart(&plugin.dir, plugin.manifest)) {
                    Ok(runtime) => PluginSlot {
                        name,
                        runtime: Some(runtime),
                        pending_crash: None,
                        crash_emitted: false,
                    },
                    Err(error) => {
                        eprintln!("dartq-daemon: plugin `{name}` failed to launch: {error}");
                        PluginSlot {
                            name,
                            runtime: None,
                            pending_crash: Some(Box::new(error)),
                            crash_emitted: false,
                        }
                    }
                }
            })
            .collect();
        Self { plugins }
    }

    /// True when no plugin was discovered — the per-cycle hook skips its
    /// work entirely (and the session skips the drain latency with it).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// A set carrying one already-dead plugin: `runtime: None` with a
    /// recorded failure awaiting an anchor, exactly what `launch` produces
    /// when a spawn fails. Test seam for the session-side weave — reaching
    /// the same state through the launch path needs a real child process
    /// (and a manifest whose entrypoint exists; missing-entrypoint
    /// manifests are *skipped*, never spawned-failed, so a session gate
    /// cannot conjure a crash from them deterministically). The crash-row
    /// generation itself is pinned by this module's tests; the session's
    /// gate pins the weave: anchor resolution, convert-once inclusion,
    /// merge order, once-per-session suppression.
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
    /// analyzed file set, and the resolved per-plugin execution config; waits
    /// [`DRAIN_BUDGET`] per live plugin for findings; and returns both.
    ///
    /// `anchor` is the crash anchor, resolved by the caller as the session
    /// file table's first entry — `None` when the table is empty, in which
    /// case crash rows are withheld until a cycle that has one. Crash rows
    /// are engine `Diagnostic`s the caller extends into its engine vec
    /// before the convert-once pass; at most one is emitted per plugin per
    /// session.
    ///
    /// Rule enablement resolves per plugin against the governing options'
    /// `plugins:` section (official `_computeDiagnosticsFromPlugin`
    /// semantics: `registry.enabled(configuration.diagnosticConfigs)`) and
    /// the plugin's registration census; the resolved config is pushed only
    /// when it changed (the runtime dedups). Codes follow the official
    /// `putIfAbsent` claim face: the first configured plugin (discovery
    /// order) claims each enabled census code, and a later plugin's rows for
    /// a claimed code are attributed away — dropped here. Hand-emitted codes
    /// outside any census never collide and always flow.
    ///
    /// Findings are grouped per plugin in discovery order with their wire
    /// rows intact; the caller resolves them onto resident files and merges
    /// ([`resolve_findings`] + [`merge`]).
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
        // The rule-claim map and the per-plugin configs, resolved once per
        // cycle. The options file is read only when some live plugin
        // actually registered rules; a read or parse failure degrades to
        // the empty configuration inside `PluginOptionsConfigs::load`.
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
        let mut claims: FxHashMap<String, usize> = FxHashMap::default();
        let resolved_per_plugin: Vec<(String, CycleConfig)> = self
            .plugins
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                let census = slot
                    .runtime
                    .as_ref()
                    .map(PluginRuntime::rule_census)
                    .unwrap_or_default();
                let resolved = resolve_census(&census, configs.for_plugin(&slot.name));
                for (census_rule, resolved_rule) in census.iter().zip(resolved.iter()) {
                    if !resolved_rule.enabled {
                        continue;
                    }
                    for code in &census_rule.codes {
                        claims
                            .entry(code.name.to_ascii_lowercase())
                            .or_insert(index);
                    }
                }
                let config = cycle_config(&resolved, context.overlays, context.analyzed);
                (slot.name.clone(), config)
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
            let (_, config) = &resolved_per_plugin[index];
            let push = CyclePush {
                analyzed: context.analyzed.to_vec(),
                config: config.clone(),
            };
            let outcome = runtime
                .feed(engine, files, &push)
                .and_then(|()| runtime.wait_diagnostics(DRAIN_BUDGET));
            match outcome {
                Ok(rows) if !rows.is_empty() => {
                    // A code an earlier plugin claimed is attributed away.
                    let rows = rows
                        .into_iter()
                        .filter(|row| {
                            claims
                                .get(&row.code.to_ascii_lowercase())
                                .is_none_or(|&owner| owner == index)
                        })
                        .collect();
                    cycle.findings.push(PluginBatch {
                        name: slot.name.clone(),
                        rows,
                    });
                }
                Ok(_) => {}
                Err(error) => {
                    // First surfaced error ends the plugin's session (the
                    // isolation contract: the runtime has already spent its
                    // budget; the engine turn stays alive).
                    slot.runtime = None;
                    if slot.crash_emitted {
                        continue;
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
            }
        }
        cycle
    }

    /// Disables every live plugin for the rest of the session (#690's
    /// daemon-side repeat-offender penalty): each runtime is dropped
    /// kill-only — the exact posture restart-budget exhaustion leaves — and
    /// its slot records the failure, so the next cycle emits the plugin's
    /// one-shot `dartq/plugin_crashed` row through the normal path
    /// (withheld until an anchor exists, never repeated). Already-dead
    /// slots keep their recorded state. Returns `true` when at least one
    /// live plugin was disabled — exactly once per session; later calls
    /// find nothing live and change nothing.
    pub fn disable_for_session(&mut self, cause: &str) -> bool {
        let mut disabled = false;
        for slot in &mut self.plugins {
            if slot.runtime.take().is_some() {
                slot.pending_crash = Some(Box::new(RuntimeError::Crashed {
                    reason: format!("disabled for the session after {cause}"),
                }));
                disabled = true;
            }
        }
        disabled
    }

    /// The graceful session-end teardown: `shutdown` per live runtime (the
    /// graceful handshake; `Drop` alone is kill-only). Errors are logged,
    /// never propagated — teardown quality does not affect the session's
    /// outcome. Each live plugin emits one structured §14.3 stats line (the
    /// evidence round's per-plugin numbers); the snapshot is read before the
    /// handshake consumes the runtime, and a dead slot's counters died with
    /// its dropped runtime — its crash row already recorded the death.
    pub fn shutdown(self) {
        for slot in self.plugins {
            if let Some(runtime) = slot.runtime {
                let name = &slot.name;
                let stats = runtime.stats();
                if let Err(error) = runtime.shutdown() {
                    eprintln!("dartq-daemon: plugin `{name}` shutdown failed: {error}");
                }
                eprintln!(
                    "dartq-daemon: plugin stats `{name}`: peak_rss_bytes={} restarts={} \
                     frames_in={} frames_out={}",
                    stats.peak_rss_bytes, stats.restarts, stats.frames_in, stats.frames_out,
                );
            }
        }
    }
}

/// One cycle's plugin output.
pub struct Cycle {
    /// Findings per emitting plugin, in discovery order; empty batches are
    /// omitted.
    pub findings: Vec<PluginBatch>,
    /// Crash rows for the caller's engine vec (at most one per plugin per
    /// session).
    pub crashes: Vec<Diagnostic>,
}

/// One plugin's drained findings, in wire arrival order.
pub struct PluginBatch {
    /// The manifest name (the LSP `source` tag for these rows).
    pub name: String,
    pub rows: Vec<PluginDiagnostic>,
}

/// One plugin finding resolved onto a resident file, ready for [`merge`].
pub struct PluginFinding {
    pub file: FileId,
    /// Emitting plugin's discovery-order index (the merge's grouping key).
    pub plugin: usize,
    /// The wire row's UTF-16 start offset (the within-plugin sort key).
    pub offset: u64,
    pub diagnostic: lsp::Diagnostic,
}

/// Resolves drained plugin rows onto the files they name and converts them
/// to the LSP shape. `resident` answers a `FileId` with its line index and
/// text (the session's engine faces); rows on files the engine does not
/// know are skipped — the same posture the transport's own feed takes for
/// unknown `FileId`s.
///
/// Offsets are UTF-16 end to end: the wire rows already carry UTF-16 units
/// and `line_col_utf16` consumes the same, so this is a position mapping,
/// never a unit conversion.
#[must_use]
pub fn resolve_findings<'a>(
    batches: &[PluginBatch],
    files: &FileTable,
    mut resident: impl FnMut(FileId) -> Option<(&'a LineIndex, &'a str)>,
) -> Vec<PluginFinding> {
    let mut findings = Vec::new();
    for (plugin, batch) in batches.iter().enumerate() {
        for row in &batch.rows {
            let Some(file) = files.try_file_id(Path::new(&row.file)) else {
                continue;
            };
            let Some((index, text)) = resident(file) else {
                continue;
            };
            findings.push(PluginFinding {
                file,
                plugin,
                offset: row.offset,
                diagnostic: to_lsp(&batch.name, row, index, text),
            });
        }
    }
    findings
}

/// Converts one wire row to the LSP shape: severity mapped to the LSP
/// 1/2/3 scale, UTF-16 offsets mapped through the line index to zero-based
/// positions, the plugin's manifest name as the `source` tag.
///
/// Plugin offsets are untrusted (the §14.3 isolation posture): a stale row
/// can predate an edit that shrank the file. Both bounds clamp to the
/// text's UTF-16 length before mapping — the file-end degradation
/// [`wire_offset`] starts — so a malformed row degrades to zero-width at
/// the end instead of tripping [`LineIndex::line_col_utf16`]'s
/// engine-internal assert. The monotone clamp keeps `start <= end`.
fn to_lsp(plugin: &str, row: &PluginDiagnostic, index: &LineIndex, text: &str) -> lsp::Diagnostic {
    let total = utf16_len_of(text);
    let start = wire_offset(row.offset).min(total);
    let end = wire_offset(row.offset.saturating_add(row.length)).min(total);
    let (start_line, start_column) = index.line_col_utf16(text, start);
    let (end_line, end_column) = index.line_col_utf16(text, end);
    let severity = match row.severity {
        PluginSeverity::Error => 1,
        PluginSeverity::Warning => 2,
        PluginSeverity::Info => 3,
    };
    lsp::Diagnostic {
        range: lsp::Range {
            start: lsp::Position {
                line: start_line - 1,
                character: start_column - 1,
            },
            end: lsp::Position {
                line: end_line - 1,
                character: end_column - 1,
            },
        },
        severity,
        code: row.code.clone(),
        source: plugin.to_owned(),
        message: row.message.clone(),
    }
}

/// Clamps a wire offset into the engine's u32 UTF-16 range; the resident
/// text's UTF-16-length bound is applied on top in [`to_lsp`].
fn wire_offset(offset: u64) -> u32 {
    u32::try_from(offset).unwrap_or(u32::MAX)
}

/// The dedup key of the merge: file, range start, code, and message — the
/// T5 ruling's `(file, offset, code, message)` exact-equality in the merged
/// rows' LSP coordinates. `message` (not source) is the key's discriminator:
/// a plugin row exactly equal to a built-in row is the same finding twice,
/// and the built-in row wins by seeding `seen` first; rows that differ in
/// wording keep both faces.
type DedupKey = (u32, u32, u32, String, String);

fn dedup_key(file: FileId, diagnostic: &lsp::Diagnostic) -> DedupKey {
    (
        file.index(),
        diagnostic.range.start.line,
        diagnostic.range.start.character,
        diagnostic.code.clone(),
        diagnostic.message.clone(),
    )
}

/// Merges the cycle's engine rows (as `(file, converted)` pairs, in their
/// existing order) with the resolved plugin findings into one deterministic
/// sequence: engine rows first in the given order, then plugin rows grouped
/// by plugin discovery order, offset-sorted within each plugin (the stable
/// sort keeps wire arrival order for equal offsets). A row whose
/// `(file, range.start, code, message)` key already appeared is dropped —
/// a plugin row never duplicates a built-in finding (the built-in row,
/// seeded first, wins), and a plugin re-emitting per cycle cannot churn
/// publishes.
#[must_use]
pub fn merge(
    engine_rows: impl IntoIterator<Item = (FileId, lsp::Diagnostic)>,
    findings: Vec<PluginFinding>,
) -> Vec<(FileId, lsp::Diagnostic)> {
    let mut merged: Vec<(FileId, lsp::Diagnostic)> = engine_rows.into_iter().collect();
    let mut seen: FxHashSet<DedupKey> = merged
        .iter()
        .map(|(file, diagnostic)| dedup_key(*file, diagnostic))
        .collect();
    let mut findings = findings;
    findings.sort_by_key(|finding| (finding.plugin, finding.offset));
    for finding in findings {
        if seen.insert(dedup_key(finding.file, &finding.diagnostic)) {
            merged.push((finding.file, finding.diagnostic));
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

    use dartq_diagnostics::{
        CodeInfo, DARTQ_PLUGIN_CRASHED, DiagnosticType, Severity as EngineSeverity, SourceRange,
    };

    use super::*;

    /// A throwaway workspace tree, removed on drop (a panicked test never
    /// leaks its fixtures into the shared temp dir).
    struct TempWorkspace(PathBuf);

    impl TempWorkspace {
        fn new(tag: &str) -> Self {
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "dartq-daemon-plugins-{tag}-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Relaxed),
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("plugins")).expect("create workspace");
            Self(root)
        }

        fn write_plugin(&self, name: &str, files: &[(&str, &str)]) {
            let dir = self.0.join("plugins").join(name);
            std::fs::create_dir_all(&dir).expect("create plugin dir");
            for (file, content) in files {
                std::fs::write(dir.join(file), content).expect("write plugin file");
            }
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempWorkspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The empty cycle context most tests cycle with: nothing analyzed, no
    /// overlays, no options file.
    fn cycle_context<'a>() -> CycleContext<'a> {
        CycleContext {
            analyzed: &[],
            overlays: &[],
            options_file: None,
        }
    }

    fn lsp_row(code: &str, source: &str, line: u32, character: u32) -> lsp::Diagnostic {
        lsp::Diagnostic {
            range: lsp::Range {
                start: lsp::Position { line, character },
                end: lsp::Position { line, character },
            },
            severity: 2,
            code: code.to_owned(),
            source: source.to_owned(),
            message: "m".to_owned(),
        }
    }

    fn finding(
        file: FileId,
        plugin: usize,
        offset: u64,
        code: &str,
        source: &str,
    ) -> PluginFinding {
        PluginFinding {
            file,
            plugin,
            offset,
            // The start mirrors the offset so distinct offsets carry
            // distinct dedup keys, as real converted rows would.
            diagnostic: lsp_row(code, source, 0, offset as u32),
        }
    }

    fn wire_row(file: &str, code: &str, offset: u64, length: u64) -> PluginDiagnostic {
        PluginDiagnostic {
            file: file.to_owned(),
            severity: PluginSeverity::Warning,
            code: code.to_owned(),
            message: "watch out".to_owned(),
            offset,
            length,
        }
    }

    fn engine_row(file: FileId, message: &'static str) -> Diagnostic {
        Diagnostic::new(
            EngineSeverity::Warning,
            CodeInfo::new("test/row", "TEST_ROW", DiagnosticType::Tool, false),
            message,
            file,
            SourceRange {
                offset: 0,
                length: 2,
            },
            None,
        )
    }

    #[test]
    fn merge_orders_engine_first_then_plugin_groups_by_discovery_order() {
        let mut files = FileTable::new();
        let a = files.intern(Path::new("a.dart"));
        let b = files.intern(Path::new("b.dart"));

        let engine_rows = [
            (a, lsp_row("e/one", "dartq", 0, 0)),
            (a, lsp_row("e/two", "dartq", 1, 0)),
        ];
        let findings = vec![
            finding(b, 1, 1, "p1/a", "p1"),
            finding(a, 0, 5, "p0/b", "p0"),
            finding(a, 0, 2, "p0/a", "p0"),
        ];

        let merged = merge(engine_rows, findings);
        let codes: Vec<&str> = merged.iter().map(|(_, d)| d.code.as_str()).collect();
        assert_eq!(codes, ["e/one", "e/two", "p0/a", "p0/b", "p1/a"]);
        assert_eq!(merged[4].0, b, "the plugin-1 row keeps its own file");
    }

    #[test]
    fn merge_dedups_on_file_start_code_and_message() {
        // The T5 dedup ruling: exact (file, offset, code, message) equality
        // drops the plugin row in favor of the built-in one; rows differing
        // in message keep both faces.
        let mut files = FileTable::new();
        let a = files.intern(Path::new("a.dart"));

        let mut builtin = lsp_row("shared/code", "dartq", 0, 0);
        builtin.message = "the built-in wording".to_owned();
        let engine_rows = [(a, builtin)];

        let mut same = lsp_row("shared/code", "echo", 0, 0);
        same.message = "the built-in wording".to_owned();
        let mut other = lsp_row("shared/code", "echo", 0, 0);
        other.message = "the plugin's own wording".to_owned();
        let mut twin = lsp_row("shared/code", "echo", 0, 0);
        twin.message = "the plugin's own wording".to_owned();
        let findings = vec![
            // Exact duplicate of the built-in row (same code, start, and
            // message): dropped, the built-in row wins.
            PluginFinding {
                file: a,
                plugin: 0,
                offset: 0,
                diagnostic: same,
            },
            // Same code and start, different message: kept.
            PluginFinding {
                file: a,
                plugin: 0,
                offset: 0,
                diagnostic: other,
            },
            // Exact duplicate of the row above: dropped (same key).
            PluginFinding {
                file: a,
                plugin: 0,
                offset: 0,
                diagnostic: twin,
            },
        ];

        let merged = merge(engine_rows, findings);
        let messages: Vec<&str> = merged.iter().map(|(_, d)| d.message.as_str()).collect();
        assert_eq!(
            messages,
            ["the built-in wording", "the plugin's own wording"],
            "exact-equal plugin rows drop; distinct wording keeps both"
        );
    }

    /// A census row for the resolution tests.
    fn census_rule(name: &str, kind: RuleKind, codes: &[(&str, PluginSeverity)]) -> RuleCensus {
        RuleCensus {
            name: name.to_owned(),
            kind,
            codes: codes
                .iter()
                .map(|(name, severity)| dartq_plugin::RuleCodeInfo {
                    name: (*name).to_owned(),
                    severity: *severity,
                })
                .collect(),
        }
    }

    #[test]
    fn options_configs_parse_the_plugins_section() {
        let configs = PluginOptionsConfigs::parse(
            "\
plugins:
  my_plugin:
    diagnostics:
      avoid_sync_io: true
      prefer_off: false
      relevel_me: error
  OTHER:
    diagnostics: {keep_me: true}
  dependency_overrides:
    some_pkg: {path: ../some_pkg}
",
        );
        let mine = configs.for_plugin("My_Plugin").expect("the plugin entry");
        assert_eq!(
            mine.diagnostics.get("avoid_sync_io"),
            Some(&ConfiguredSeverity::Enable)
        );
        assert_eq!(
            mine.diagnostics.get("prefer_off"),
            Some(&ConfiguredSeverity::Disable)
        );
        assert_eq!(
            mine.diagnostics.get("relevel_me"),
            Some(&ConfiguredSeverity::Error)
        );
        // Plugin names match case-insensitively.
        assert!(configs.for_plugin("other").is_some());
        // The hosting face is never a plugin entry.
        assert!(configs.for_plugin("dependency_overrides").is_none());
        // Unknown severity names degrade to enable (official `_parseRuleConfig`).
        let loose =
            PluginOptionsConfigs::parse("plugins:\n  p:\n    diagnostics: {odd_rule: shout}\n");
        assert_eq!(
            loose
                .for_plugin("p")
                .expect("entry")
                .diagnostics
                .get("odd_rule"),
            Some(&ConfiguredSeverity::Enable)
        );
        // A file without a plugins section is the empty configuration.
        assert!(
            PluginOptionsConfigs::parse("linter:\n  rules:\n    x: true\n")
                .for_plugin("p")
                .is_none()
        );
        // Malformed YAML degrades to the empty configuration.
        assert!(
            PluginOptionsConfigs::parse("{not yaml")
                .for_plugin("p")
                .is_none()
        );
    }

    #[test]
    fn census_resolution_follows_the_official_enabled_semantics() {
        // Lint rules are disabled by default and enabled by an explicit
        // config; warning rules are enabled by default and disabled by an
        // explicit config (official `RegistryMixin.enabled`).
        let census = [
            census_rule(
                "opt_in_lint",
                RuleKind::Lint,
                &[("lint_code", PluginSeverity::Warning)],
            ),
            census_rule(
                "opt_out_warning",
                RuleKind::Warning,
                &[("warn_code", PluginSeverity::Warning)],
            ),
        ];
        let plain = resolve_census(&census, None);
        assert!(!plain[0].enabled, "lint rules stay disabled unconfigured");
        assert!(plain[1].enabled, "warning rules run unconfigured");

        let configs = PluginOptionsConfigs::parse(
            "plugins:\n  p:\n    diagnostics:\n      opt_in_lint: true\n      opt_out_warning: false\n",
        );
        let resolved = resolve_census(&census, configs.for_plugin("p"));
        assert!(
            resolved[0].enabled,
            "the explicit config enables the lint rule"
        );
        assert!(
            !resolved[1].enabled,
            "the explicit config disables the warning rule"
        );

        // A re-leveling severity entry is keyed by the code name (official
        // `_configuredSeverity` looks the config map up by
        // `code.lowerCaseName`); enablement is the rule-name face of the
        // same map (official `registry.enabled`).
        let configs = PluginOptionsConfigs::parse(
            "plugins:\n  p:\n    diagnostics: {opt_in_lint: true, lint_code: error}\n",
        );
        let resolved = resolve_census(&census, configs.for_plugin("p"));
        assert!(resolved[0].enabled, "the rule-name entry enables the rule");
        assert_eq!(
            resolved[0].code_overrides,
            vec![("lint_code".to_owned(), PluginSeverity::Error)]
        );
        // The un-releveled warning rule keeps its declared severity (no
        // override entry).
        let plain_resolved = resolve_census(&census, None);
        assert!(plain_resolved[1].code_overrides.is_empty());

        // The official linter family's shape — rule name and code name
        // coincide — one entry both enables and re-levels.
        let census = [census_rule(
            "same_name",
            RuleKind::Lint,
            &[("same_name", PluginSeverity::Warning)],
        )];
        let configs =
            PluginOptionsConfigs::parse("plugins:\n  p:\n    diagnostics: {same_name: error}\n");
        let resolved = resolve_census(&census, configs.for_plugin("p"));
        assert!(resolved[0].enabled);
        assert_eq!(
            resolved[0].code_overrides,
            vec![("same_name".to_owned(), PluginSeverity::Error)]
        );
    }

    #[test]
    fn cycle_config_merges_enabled_rules_and_restricts_overlays() {
        let resolved = vec![
            ResolvedRule {
                name_lower: "kept".to_owned(),
                enabled: true,
                code_overrides: vec![("kept_code".to_owned(), PluginSeverity::Error)],
            },
            ResolvedRule {
                name_lower: "dropped".to_owned(),
                enabled: false,
                code_overrides: vec![("dropped_code".to_owned(), PluginSeverity::Info)],
            },
        ];
        let analyzed = ["lib/a.dart".to_owned()];
        let overlays = [
            ("lib/a.dart".to_owned(), "edited text".to_owned()),
            ("lib/outside.dart".to_owned(), "not analyzed".to_owned()),
        ];
        let config = cycle_config(&resolved, &overlays, &analyzed);
        assert_eq!(config.enabled_rules, ["kept"]);
        assert_eq!(
            config.severity_overrides.get("kept_code"),
            Some(&PluginSeverity::Error)
        );
        assert!(
            config.severity_overrides.get("dropped_code").is_none(),
            "a disabled rule contributes no overrides"
        );
        assert_eq!(
            config.overlays.len(),
            1,
            "overlays restrict to the analyzed set"
        );
        assert_eq!(config.overlays[0].file, "lib/a.dart");
        assert!(config.overlays[0].content.is_some());
        // The empty resolution is the empty config (the dedup's no-push shape).
        assert!(cycle_config(&[], &[], &[]).is_empty());
    }

    #[test]
    fn resolve_findings_maps_positions_and_skips_unresolvable_files() {
        let mut files = FileTable::new();
        let a = files.intern(Path::new("a.dart"));
        let resident_never = files.intern(Path::new("b.dart"));

        let index = LineIndex::new("ab\ncd\ne");
        let text = "ab\ncd\ne";
        let resident = |file: FileId| (file == a).then_some((&index, text));

        let batches = [PluginBatch {
            name: "echo".to_owned(),
            rows: vec![
                wire_row("a.dart", "echo/roundtrip", 3, 2),
                wire_row("ghost.dart", "echo/ghost", 0, 0),
                wire_row("b.dart", "echo/not_resident", 0, 0),
            ],
        }];

        let findings = resolve_findings(&batches, &files, resident);
        assert_eq!(findings.len(), 1, "unknown and non-resident files skip");
        let row = &findings[0];
        assert_eq!(row.file, a);
        assert_eq!(row.plugin, 0);
        assert_eq!(row.offset, 3);
        // "ab\ncd\ne": offset 3 starts line 2 col 1; the row's end offset 5
        // (the newline terminating line 2) answers line 2 col 3 — one-based
        // line_col answers, LSP zero-based positions out.
        assert_eq!(
            row.diagnostic.range.start,
            lsp::Position {
                line: 1,
                character: 0
            }
        );
        assert_eq!(
            row.diagnostic.range.end,
            lsp::Position {
                line: 1,
                character: 2
            }
        );
        assert_eq!(row.diagnostic.severity, 2);
        assert_eq!(row.diagnostic.code, "echo/roundtrip");
        assert_eq!(row.diagnostic.source, "echo");
        assert_eq!(row.diagnostic.message, "watch out");
        assert_eq!(resident_never.index(), 1, "the interned id is exercised");
    }

    #[test]
    fn resolve_findings_clamps_out_of_range_offset_to_file_end() {
        let mut files = FileTable::new();
        let a = files.intern(Path::new("a.dart"));

        let index = LineIndex::new("ab\ncd\ne");
        let text = "ab\ncd\ne";
        let resident = |file: FileId| (file == a).then_some((&index, text));

        // The classic stale row: offsets taken before an edit shrank the
        // file far below them. The mapping must degrade, not panic.
        let batches = [PluginBatch {
            name: "echo".to_owned(),
            rows: vec![wire_row("a.dart", "echo/stale", 10_000_000, 4)],
        }];

        let findings = resolve_findings(&batches, &files, resident);
        assert_eq!(findings.len(), 1, "the row publishes degraded");
        // Both bounds clamp to the file's UTF-16 end (7 units): zero-width
        // at EOF ("e" is line 3, so the 1-based (3, 2) answers LSP (2, 1)).
        let eof = lsp::Position {
            line: 2,
            character: 1,
        };
        assert_eq!(findings[0].diagnostic.range.start, eof);
        assert_eq!(findings[0].diagnostic.range.end, eof);
    }

    #[test]
    fn resolve_findings_clamps_bounds_crossing_the_file_end() {
        let mut files = FileTable::new();
        let a = files.intern(Path::new("a.dart"));

        let index = LineIndex::new("ab\ncd\ne");
        let text = "ab\ncd\ne";
        let resident = |file: FileId| (file == a).then_some((&index, text));

        // Start in range with the length running past EOF, and start beyond
        // EOF with the end further still — the clamp keeps `start <= end`
        // in both shapes.
        let batches = [PluginBatch {
            name: "echo".to_owned(),
            rows: vec![
                wire_row("a.dart", "echo/long_tail", 5, 1_000_000),
                wire_row("a.dart", "echo/past_end", 100, 200),
            ],
        }];

        let findings = resolve_findings(&batches, &files, resident);
        assert_eq!(findings.len(), 2);
        let eof = lsp::Position {
            line: 2,
            character: 1,
        };
        let range_of = |code: &str| {
            &findings
                .iter()
                .find(|f| f.diagnostic.code == code)
                .expect("row present")
                .diagnostic
                .range
        };
        // Offset 5 survives intact (the newline ending line 2 → LSP (1, 2));
        // the out-of-range end clamps to EOF.
        assert_eq!(
            range_of("echo/long_tail").start,
            lsp::Position {
                line: 1,
                character: 2
            }
        );
        assert_eq!(range_of("echo/long_tail").end, eof);
        // Both bounds past EOF: zero-width at the end.
        assert_eq!(range_of("echo/past_end").start, eof);
        assert_eq!(range_of("echo/past_end").end, eof);
    }

    fn dead_set(reason: &str) -> PluginSet {
        PluginSet {
            plugins: vec![PluginSlot {
                name: "echo".to_owned(),
                runtime: None,
                pending_crash: Some(Box::new(RuntimeError::Crashed {
                    reason: reason.to_owned(),
                })),
                crash_emitted: false,
            }],
        }
    }

    #[test]
    fn crash_row_emitted_once_per_plugin_per_session() {
        let mut files = FileTable::new();
        let a = files.intern(Path::new("a.dart"));
        let mut set = dead_set("restart budget of 3 exhausted");

        let first = set.cycle(&[], &files, Some(a), &cycle_context());
        assert_eq!(first.crashes.len(), 1);
        assert!(first.findings.is_empty());
        let crash = &first.crashes[0];
        assert_eq!(crash.code.name, DARTQ_PLUGIN_CRASHED.name);
        assert_eq!(crash.severity, EngineSeverity::Error);
        assert_eq!(
            crash.range,
            SourceRange {
                offset: 0,
                length: 0
            }
        );
        assert_eq!(crash.file, a);
        assert!(crash.message.contains("plugin `echo`"));

        let second = set.cycle(&[], &files, Some(a), &cycle_context());
        assert!(second.crashes.is_empty(), "suppressed after the first");
    }

    #[test]
    fn crash_row_held_until_an_anchor_exists() {
        let mut files = FileTable::new();
        let a = files.intern(Path::new("a.dart"));
        let mut set = dead_set("plugin process closed the connection");

        let anchored = set.cycle(&[], &files, None, &cycle_context());
        assert!(anchored.crashes.is_empty(), "nothing to anchor against");

        let with_anchor = set.cycle(&[], &files, Some(a), &cycle_context());
        assert_eq!(with_anchor.crashes.len(), 1, "the held row publishes once");

        let again = set.cycle(&[], &files, Some(a), &cycle_context());
        assert!(again.crashes.is_empty(), "still once per session");
    }

    #[test]
    fn disable_for_session_spares_dead_slots_and_reports_nothing_live() {
        let mut files = FileTable::new();
        let a = files.intern(Path::new("a.dart"));
        let mut set = dead_set("restart budget of 3 exhausted");

        assert!(!set.disable_for_session("repeated analysis panics"));
        // The recorded failure survived untouched: the row still publishes
        // once, exactly as it would have without the penalty.
        let cycle = set.cycle(&[], &files, Some(a), &cycle_context());
        assert_eq!(cycle.crashes.len(), 1);
        assert!(
            cycle.crashes[0].message.contains("restart budget"),
            "the original reason is kept: {}",
            cycle.crashes[0].message
        );
    }

    /// Serial phase: the live half of the penalty — a real plugin runtime
    /// is dropped kill-only and its slot records the crash, which the next
    /// cycle surfaces as the one-shot row; a second disable finds nothing
    /// live. The dead-set test above pins the slot math without a process.
    #[test]
    #[ignore = "serial phase: spawns a real dart plugin process"]
    fn disable_for_session_drops_a_live_runtime_and_records_its_penalty_crash() {
        let dart_ok = std::process::Command::new("dart")
            .arg("--version")
            .output()
            .is_ok();
        assert!(dart_ok, "dart is not on PATH");

        let script =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../dartq-plugin-host/bin/echo_plugin.dart");
        let workspace = TempWorkspace::new("disable");
        let manifest = format!(
            r#"{{"name": "echo", "version": "0.1.0", "entrypoint": "{}",
                "timeouts": {{"callback_ms": 30000, "request_ms": 120000, "request_retries": 3}}}}"#,
            script.display(),
        );
        workspace.write_plugin("echo", &[("dartq_plugin.json", &manifest)]);

        let mut set = PluginSet::launch(workspace.path());
        assert!(!set.is_empty(), "the echo fixture discovered and launched");

        let mut files = FileTable::new();
        let a = files.intern(&workspace.path().join("lib").join("a.dart"));
        assert!(set.disable_for_session("repeated analysis panics"));
        let cycle = set.cycle(&[], &files, Some(a), &cycle_context());
        assert_eq!(cycle.crashes.len(), 1);
        assert!(
            cycle.crashes[0]
                .message
                .contains("disabled for the session after repeated analysis panics"),
            "the penalty reason rides the row: {}",
            cycle.crashes[0].message
        );
        assert!(
            !set.disable_for_session("again"),
            "nothing live the second time"
        );
        set.shutdown();
    }

    #[test]
    fn launch_skips_broken_manifests_and_empty_workspaces() {
        let workspace = TempWorkspace::new("broken");
        workspace.write_plugin("bad_json", &[("dartq_plugin.json", "{not json")]);
        workspace.write_plugin(
            "no_script",
            &[(
                "dartq_plugin.json",
                r#"{"name": "no_script", "version": "0.1.0", "entrypoint": "missing.dart"}"#,
            )],
        );
        // A subdirectory without a manifest is not a plugin.
        std::fs::create_dir_all(workspace.path().join("plugins").join("notes"))
            .expect("create notes dir");

        let set = PluginSet::launch(workspace.path());
        assert!(set.is_empty(), "every manifest was skipped");

        let empty = TempWorkspace::new("empty");
        std::fs::remove_dir(empty.path().join("plugins")).expect("drop plugins dir");
        assert!(PluginSet::launch(empty.path()).is_empty());
    }

    /// Serial phase: spawns a real `dart` plugin process (the host package's
    /// echo fixture) and exercises launch → feed → drain → shutdown end to
    /// end. Skipped in the ordinary suite — lanes cannot execute dart.
    #[test]
    #[ignore = "serial phase: spawns a real dart plugin process"]
    fn live_launch_cycle_and_shutdown_with_the_echo_fixture() {
        let dart_ok = std::process::Command::new("dart")
            .arg("--version")
            .output()
            .is_ok();
        assert!(dart_ok, "dart is not on PATH");

        let script =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../dartq-plugin-host/bin/echo_plugin.dart");
        let workspace = TempWorkspace::new("live");
        let manifest = format!(
            r#"{{"name": "echo", "version": "0.1.0", "entrypoint": "{}",
                "capabilities": ["lint"],
                "timeouts": {{"callback_ms": 30000, "request_ms": 120000, "request_retries": 3}}}}"#,
            script.display(),
        );
        workspace.write_plugin("echo", &[("dartq_plugin.json", &manifest)]);

        let mut set = PluginSet::launch(workspace.path());
        assert!(!set.is_empty(), "the echo fixture discovered and launched");

        let mut files = FileTable::new();
        let a = files.intern(&workspace.path().join("lib").join("a.dart"));
        let engine_rows = [engine_row(a, "hello")];

        // A cold `dart` boot rides the first cycle's feed (the handshake);
        // the echo reply lands within a later cycle's bounded drain.
        let mut batch = None;
        for _ in 0..200 {
            let cycle = set.cycle(&engine_rows, &files, Some(a), &cycle_context());
            assert!(cycle.crashes.is_empty(), "the echo fixture must not crash");
            if let Some(found) = cycle.findings.into_iter().next() {
                batch = Some(found);
                break;
            }
        }
        let batch = batch.expect("echo replied within the cycle budget");
        assert_eq!(batch.name, "echo");
        assert_eq!(batch.rows.len(), 1);
        let row = &batch.rows[0];
        assert_eq!(row.code, "echo/roundtrip");
        assert_eq!(
            row.file,
            files.path(a).unwrap_or(Path::new("")).to_string_lossy()
        );

        set.shutdown();
    }
}
