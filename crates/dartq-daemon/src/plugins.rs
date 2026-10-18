//! Plugin lifecycle orchestration for the daemon: discovery,
//! lazy spawn at first use, the per-cycle engine-diagnostics feed, the
//! crash→diagnostic bridge, and the deterministic merge of plugin findings
//! into the diagnostics output. The thin daemon-side layer `dartq-plugin`
//! deliberately leaves to its output consumer.
//!
//! # Representation contract (the merge's type-level boundary)
//!
//! Plugin *findings* convert directly to [`wire::Diagnostic`] with the
//! plugin's manifest name as the `source` tag. They never pass through
//! the engine [`Diagnostic`] — its `CodeInfo` requires `&'static` code
//! names, and plugin codes are free-form. Only
//! [`crash_diagnostic`] output (already a legal engine `Diagnostic`) rides
//! the engine shape: the cycle hands it back beside the findings and the
//! session's merge step converts it through the engine face into the
//! turn's publishes — never into the analyze answers.
//!
//! # Blocking posture (synchronous-core policy)
//!
//! Every face here blocks the calling thread — the main loop's thread — by
//! design: [`PluginSet::launch`] spends only the discovery walks (no
//! sidecar starts there); the first spawn handshake (up to the manifest's
//! `request_ms × (retries + 1)` per plugin, defaults 10 s × 4)
//! lands inside [`PluginSet::cycle`]'s launch pass on the first cycle that
//! has something to feed, and each cycle after that spends at most one
//! [`DRAIN_BUDGET`] per plugin waiting for findings.
//! Findings that arrive after a cycle's budget publish on the next cycle.
//! The cycle is the *merge step* of the core-first publish: the session
//! stages its engine diagnostics publish before consulting the plugin set
//! at all, so the waits here can delay only the plugin rows' own addition —
//! never the core publish ([`DRAIN_BUDGET`] is the per-plugin timeout
//! isolation; a plugin that will not quiesce within it is degraded, its
//! rows deferred, and the move is counted in the stats' `degraded_cycles`).
//!
//! # Known-accepted postures
//!
//! - `feed` with no diagnostics and no analyzed files sends nothing: there
//!   is no "diagnostics cleared" signal to a plugin (the transport has
//!   none). A cycle with analyzed files but an empty diagnostic set still
//!   feeds — the enabled rules must run over the files even when the engine
//!   has nothing to report.
//! - A plugin whose capability is never exercised never spawns: the launch
//!   pass runs only on cycles that would feed, so a plugin-free workload
//!   keeps the sidecar plane at zero resident processes. A sidecar idle
//!   past its manifest's `idle_evict_ms` is evicted by the runtime's
//!   monitor and returns budget-free at the next fed cycle — an eviction
//!   is a host memory action, never a crash row; its cost is one cold
//!   respawn + re-analysis on the first post-idle interaction.
//! - The restart budget never resets: the runtime relaunches
//!   a dead child lazily within the manifest's `request_retries`, and a
//!   spent budget is terminal for the session (eviction respawns are
//!   budget-free and never consume it).
//! - A crash row recorded while the session has no anchor file is withheld,
//!   not dropped: it publishes on the first cycle that has one.
//! - A plugin row whose offsets fall outside the resident file (the classic
//!   stale row surviving a shrink edit) clamps to the file's UTF-16 end in
//!   [`to_wire`]: the row publishes degraded, it never panics the mapping.
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
use std::time::{Duration, Instant};

use dartq_commands::SCHEMA_VERSION;
use dartq_commands::fix::{DeclaredPlugin, declared_plugin_sources, plugin_package_config};
use dartq_commands::query as commands_query;
use dartq_diagnostics::{Diagnostic, FileId};
use dartq_engine::Engine;
use dartq_engine::analysis_options::{AnalysisOptions, ConfiguredSeverity};
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

use crate::wire;

/// Per-cycle wait for plugin findings (the bounded drain): long enough for a
/// warm plugin's push, short enough to keep the main loop responsive. Rows
/// arriving later publish on the next cycle.
const DRAIN_BUDGET: Duration = Duration::from_millis(50);

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
    /// The loaded options of the workspace's governing directories — every
    /// distinct `AnalysisOptions` the context parsed (the root's plus the
    /// nested options files inside the package roots). The per-directory
    /// plugin semantics resolve against these: `plugins:` configurations
    /// merged per file (upstream `AnalysisSetConfigurationsParams` —
    /// `plugin_watcher.dart` pushes one `{dir → {plugin → config}}` map per
    /// options directory), so a plugin serves only the directories whose
    /// governing options actually declare it.
    pub governing_options: &'a [&'a AnalysisOptions],
    /// The reverse-query server (T4): answers the plugins' `request/query`
    /// pulls against the session's current engine view — the daemon's
    /// analyze face always has one (it owns the engine); a cycle without
    /// one declines queued queries with the wire's error channel, so a
    /// querying producer fails fast instead of stalling into its callback
    /// budget. `serve_query` is the canonical implementation.
    pub query_server: Option<QueryServer<'a>>,
}

/// A reverse-query answer: the reply params (`Err` answers the wire's
/// error channel).
type QueryAnswer = Result<serde_json::Map<String, Value>, String>;

/// A reverse-query server (T4): answers one pending query with its reply
/// params (`Err` answers the wire's error channel).
type QueryServer<'a> = &'a dyn Fn(&PendingQuery) -> QueryAnswer;

/// The `diagnostics` map the serving directory's options declares for
/// `plugin` — `None` when the directory's own `plugins:` map does not name
/// the plugin (upstream `_getConfigurationsForFile` longest-prefix match:
/// the plugin serves nothing in a directory whose pluginMap omits it).
/// `options` is the governing directory's merged `AnalysisOptions`; only a
/// file whose own `plugins:` node is a map contributes a directory entry
/// (upstream's `pluginsNode is YamlMap` gate, the
/// [`AnalysisOptions::has_plugins_map`] face).
fn serving_diagnostics<'a>(
    options: &'a AnalysisOptions,
    plugin: &str,
) -> Option<&'a FxHashMap<String, ConfiguredSeverity>> {
    if !options.has_plugins_map {
        return None;
    }
    options
        .plugins
        .iter()
        .find(|config| config.name == plugin)
        .map(|config| &config.diagnostics)
}

/// The census code's attribution for the finding conversion: the declaring
/// rule's lowercased name (the `diagnostics` rule-level gate key) and its
/// kind.
#[derive(Clone)]
struct CodeMeta {
    rule: String,
}

/// One rule of a census, resolved for the cycle.
struct ResolvedRule {
    name_lower: String,
    /// Whether at least one governing options enables the rule — the push
    /// face's enable set is the union across directories (a rule enabled
    /// anywhere must run so its per-directory findings exist; the
    /// per-directory gate itself applies host-side at conversion).
    enabled: bool,
}

/// Resolves a plugin's census against every governing options' `plugins:`
/// configuration with the official `RegistryBase::enabled` semantics —
/// warning rules enabled unless the serving directory's config disables
/// them, lint rules enabled only by an explicit entry. A plugin absent
/// from a directory's plugin map serves nothing there (it contributes
/// neither enablement nor severity entries for that directory).
fn resolve_census(
    census: &[RuleCensus],
    plugin: &str,
    governing: &[&AnalysisOptions],
) -> Vec<ResolvedRule> {
    census
        .iter()
        .map(|rule| {
            let name_lower = rule.name.to_ascii_lowercase();
            let enabled = governing.iter().any(|options| {
                let Some(diagnostics) = serving_diagnostics(options, plugin) else {
                    return false;
                };
                let entry = diagnostics.get(name_lower.as_str());
                match rule.kind {
                    RuleKind::Warning => entry.is_none_or(|severity| severity.is_enabled()),
                    RuleKind::Lint => entry.is_some_and(|severity| severity.is_enabled()),
                }
            });
            ResolvedRule {
                name_lower,
                enabled,
            }
        })
        .collect()
}

/// Builds the cycle's [`CycleConfig`] from the resolved census: the union
/// of enabled rule names across the governing directories, an empty
/// severity-override map (the per-directory re-leveling applies host-side
/// at conversion — a pushed override could encode only one directory's
/// map), and the overlays restricted to the analyzed set.
fn cycle_config(
    resolved: &[ResolvedRule],
    overlays: &[(String, String)],
    analyzed: &[String],
) -> CycleConfig {
    let analyzed_set: FxHashSet<&String> = analyzed.iter().collect();
    let enabled_rules = resolved
        .iter()
        .filter(|rule| rule.enabled)
        .map(|rule| rule.name_lower.clone())
        .collect();
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
        severity_overrides: BTreeMap::new(),
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
    /// The code → owning-plugin attribution (upstream `pluginCodeMapping`,
    /// upstream Dart SDK source,
    /// `pkg/analysis_server_plugin/lib/src/plugin_server.dart`): the first
    /// plugin in discovery order whose enabled census declares a code owns
    /// it — `putIfAbsent` semantics — for `plugin/name`-scoped suppression.
    /// **Attribution only, never a row filter**: upstream
    /// `mergeAnalysisErrors` (`pkg/analysis_server/lib/src/plugin/result_merger.dart`)
    /// concatenates every plugin's diagnostics verbatim, duplicates
    /// included, so a code collision drops nothing. Rebuilt per cycle in
    /// [`PluginSet::cycle`]; the map holds discovery-order indexes.
    code_owners: FxHashMap<String, usize>,
}

/// One plugin's session state. `runtime: None` marks the plugin dead for
/// this session: the first surfaced runtime error means the runtime already
/// spent its restart budget reaching it, and further interaction would burn
/// the manifest's request budget on every cycle for a plugin that cannot
/// come back.
struct PluginSlot {
    /// The manifest name — the wire `source` tag and the crash-message
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
    /// Discovers plugins from the workspace's explicit sources — the
    /// `plugins/` convention and the governing `analysis_options`' pub-native
    /// `plugins:` section resolved through `pub_config` (the parsed
    /// `package_config.json` in `config_dir`, augmented by each
    /// `path:`-declared name's synthetic entry — a pub-resolved entry wins
    /// over a `path:` shadow, matching upstream's precedence) —
    /// and registers each hit. **No sidecar process starts here**: every
    /// runtime is built lazy, and the first generation spawns at the first
    /// cycle that has something to feed ([`PluginSet::cycle`]'s launch
    /// pass), so a plugin whose capability is never needed never costs its
    /// resident set.
    ///
    /// This is the plugin subsystem's whole boot cost, once per session:
    /// the options locator walk and read (`declared_plugin_sources`) and the
    /// `plugins/` convention scan. A workspace that declares
    /// no plugins and ships no `plugins/` directory pays the two absence
    /// probes — one `read_dir` miss, one locator walk — and allocates
    /// nothing (the `declared` source contributes an empty borrowed slice);
    /// the per-cycle path then reduces to [`PluginSet::is_empty`].
    ///
    /// Skipped plugins (malformed, missing entrypoint,
    /// duplicate name, declared but unresolved) are logged and ignored. A
    /// plugin whose later spawn fails surfaces through the ordinary crash
    /// path on the cycle that first needs it — one `dartq/plugin_crashed`
    /// row, the session alive.
    #[must_use]
    pub fn launch(workspace_root: &Path, pub_config: &PackageConfig, config_dir: &Path) -> Self {
        // The declaration seam is the commands side's single fact source
        // (`declared_plugin_sources` + `plugin_package_config` in
        // `dartq-commands`' fix face): the daemon depends on `dartq-commands`,
        // so both faces consume one implementation — the same upstream
        // `_parsePluginSource` arm order and the same `path:`-augmented
        // package config — instead of a re-encoded daemon copy.
        let declared = declared_plugin_sources(workspace_root);
        // The absent shape: a 'static empty path borrow and an empty vec —
        // the pub-native source contributes nothing and allocates nothing.
        let (options_file, names, decls): (&Path, Vec<String>, &[DeclaredPlugin]) =
            match declared.as_ref() {
                Some((file, decls)) => (
                    file.as_path(),
                    decls.iter().map(|decl| decl.name.clone()).collect(),
                    decls.as_slice(),
                ),
                None => (Path::new(""), Vec::new(), &[]),
            };
        // `path:`-declared names absent from pub's resolution get a synthetic
        // config entry so the unchanged discovery arm resolves them like any
        // pub-native hit; the clone costs only when a `path:` declaration
        // exists (the plugin-free workspace never reaches it).
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
                // The analyze face opts into the T4 Query API: the daemon
                // owns the engine view the reverse queries answer against.
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
        Self {
            plugins,
            code_owners: FxHashMap::default(),
        }
    }

    /// True when no plugin was discovered — the per-cycle hook skips its
    /// work entirely (and the session skips the drain latency with it).
    /// This is the whole absent-path cost of a plugin-free workspace's
    /// analysis cycle: one emptiness check.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// A set carrying one already-dead plugin: `runtime: None` with a
    /// recorded failure awaiting an anchor, exactly what the cycle's launch
    /// pass records when a first-use spawn fails. Test seam for the
    /// session-side weave — reaching
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
            code_owners: FxHashMap::default(),
        }
    }

    /// The owning plugin's discovery-order index for `code` (`plugin/name`
    /// suppression attribution): the first configured plugin that declared
    /// it — `putIfAbsent`, never overwritten by a later claimant. The
    /// row-level upshot of the collision ruling (#1819) is exercised
    /// through the cycle: both emitters' rows publish, this map names the
    /// one owner.
    #[cfg(test)]
    pub(crate) fn code_owner(&self, code: &str) -> Option<usize> {
        self.code_owners.get(&code.to_ascii_lowercase()).copied()
    }

    /// The named plugin's live stats snapshot — the serial phase's evidence
    /// face (the cycle counters land in the runtime's own
    /// [`PluginStatsSnapshot`]; a dead slot has none).
    #[cfg(test)]
    pub(crate) fn runtime_stats(&self, name: &str) -> Option<dartq_plugin::PluginStatsSnapshot> {
        self.plugins
            .iter()
            .find(|slot| slot.name == name)
            .and_then(|slot| slot.runtime.as_ref())
            .map(PluginRuntime::stats)
    }

    /// One analyze cycle against the plugins: pushes the engine's merged
    /// diagnostic set (the lints included by the caller), the cycle's
    /// analyzed file set, and the resolved per-plugin execution config; waits
    /// [`DRAIN_BUDGET`] per live plugin for findings; and returns both.
    ///
    /// `anchor` is the crash anchor, resolved by the caller at cycle entry
    /// (the file table's first published entry) — `None` when nothing has
    /// published yet, in which case crash rows are withheld until a cycle
    /// that has one. Crash rows come back beside the findings as engine
    /// `Diagnostic`s; the caller's merge step converts and publishes them
    /// (they never join the engine vec). At most one is emitted per plugin
    /// per session.
    ///
    /// Rule enablement resolves per plugin against the governing options'
    /// `plugins:` section (official `_computeDiagnosticsFromPlugin`
    /// semantics: `registry.enabled(configuration.diagnosticConfigs)`) and
    /// the plugin's registration census; the resolved config is pushed only
    /// when it changed (the runtime dedups). Code collisions follow the
    /// official face: the `(code → plugin)` attribution map is
    /// `putIfAbsent` first-claim (`plugin_server.dart:576`, upstream Dart
    /// SDK source, `pkg/analysis_server_plugin/lib/src/plugin_server.dart`)
    /// for `plugin/name` suppression, while the rows themselves never
    /// drop — upstream `mergeAnalysisErrors` concatenates every plugin's
    /// diagnostics verbatim, duplicates included. Hand-emitted codes
    /// outside any census never collide and always flow.
    ///
    /// Findings are grouped per plugin in discovery order with their wire
    /// rows intact; the caller resolves them onto resident files and merges
    /// ([`resolve_findings`] + [`merge`]).
    ///
    /// The cycle opens with the lazy launch pass: every slot's runtime is
    /// made alive before the rule census resolves below, so the first fed
    /// cycle resolves enablement against the real registration census (a
    /// never-launched runtime reports an empty census) and a failed first
    /// spawn takes the ordinary crash path. Slots stay unlaunched on cycles
    /// with nothing to feed — a plugin-free workload never spawns a
    /// sidecar.
    #[must_use]
    #[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
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
        // The rule-claim map and the per-plugin configs, resolved once per
        // cycle. The governing options are needed only when some live
        // plugin actually registered rules.
        let has_rules = self.plugins.iter().any(|slot| {
            slot.runtime
                .as_ref()
                .is_some_and(|runtime| !runtime.rule_census().is_empty())
        });
        let governing: &[&AnalysisOptions] = if has_rules {
            context.governing_options
        } else {
            &[]
        };
        // The code-ownership map, rebuilt per cycle: upstream
        // `pluginCodeMapping` is `putIfAbsent` — the first configured
        // plugin (discovery order) owns a code for `plugin/name`
        // suppression attribution. It never filters rows (#1819): a later
        // plugin's diagnostics for a claimed code still publish, matching
        // upstream's verbatim merge.
        self.code_owners.clear();
        let code_owners = &mut self.code_owners;
        let resolved_per_plugin: Vec<(String, CycleConfig, FxHashMap<String, CodeMeta>)> = self
            .plugins
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                let census = slot
                    .runtime
                    .as_ref()
                    .map(PluginRuntime::rule_census)
                    .unwrap_or_default();
                let resolved = resolve_census(&census, &slot.name, governing);
                claim_codes(code_owners, index, &census, &resolved);
                // The finding conversion's rule-level `diagnostics` gate
                // key: every census code to its declaring rule's name.
                let codes = census
                    .iter()
                    .flat_map(|rule| {
                        rule.codes.iter().map(move |code| {
                            (
                                code.name.to_ascii_lowercase(),
                                CodeMeta {
                                    rule: rule.name.to_ascii_lowercase(),
                                },
                            )
                        })
                    })
                    .collect();
                let config = cycle_config(&resolved, context.overlays, context.analyzed);
                (slot.name.clone(), config, codes)
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
            let (_, config, codes) = &resolved_per_plugin[index];
            let push = CyclePush {
                analyzed: context.analyzed.to_vec(),
                config: config.clone(),
            };
            // The reverse-query pump adapter: the injected server behind the
            // runtime's `&mut dyn FnMut` answer face.
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
                    // The cycle's per-plugin cost lands in the stats face:
                    // the busy wall time, and the budget-expired drains
                    // (degraded — the plugin's remaining rows publish on a
                    // later cycle). Errors take the crash path below and
                    // count through the crash machinery instead.
                    if degraded {
                        eprintln!(
                            "dartq-daemon: plugin `{}` exceeded its {} ms drain budget; \
                             remaining rows publish on a later cycle",
                            slot.name,
                            DRAIN_BUDGET.as_millis(),
                        );
                    }
                    runtime.observe_cycle(started.elapsed(), degraded);
                    accept_rows(&mut cycle, &slot.name, codes, rows);
                }
                Err(error) => {
                    // First surfaced error ends the plugin's session (the
                    // isolation contract: the runtime has already spent its
                    // budget; the engine turn stays alive).
                    fail_slot(slot, error, anchor, &mut cycle);
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
    /// outcome. Each live plugin emits one structured stats line (the
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
                     evictions={} frames_in={} frames_out={} cycles={} degraded_cycles={} \
                     busy_ms={}",
                    stats.peak_rss_bytes,
                    stats.restarts,
                    stats.evictions,
                    stats.frames_in,
                    stats.frames_out,
                    stats.cycles,
                    stats.degraded_cycles,
                    stats.busy_nanos / 1_000_000,
                );
            }
        }
    }
}

/// One plugin's enabled census codes claim ownership in `code_owners`
/// (first-claim `putIfAbsent` — upstream `pluginCodeMapping`): the
/// discovery-order-first plugin that declares a code owns it for
/// `plugin/name` suppression attribution. Only *enabled* rules claim —
/// the same gate upstream applies (`pluginCodeMapping` fills inside
/// `_computeDiagnosticsFromPlugin` after `registry.enabled`). Pure
/// attribution: the claim never decides whether a drained row publishes
/// ([`accept_rows`] takes them verbatim).
fn claim_codes(
    code_owners: &mut FxHashMap<String, usize>,
    index: usize,
    census: &[RuleCensus],
    resolved: &[ResolvedRule],
) {
    for (census_rule, resolved_rule) in census.iter().zip(resolved.iter()) {
        if !resolved_rule.enabled {
            continue;
        }
        for code in &census_rule.codes {
            code_owners
                .entry(code.name.to_ascii_lowercase())
                .or_insert(index);
        }
    }
}

/// One plugin's drained rows join the cycle's findings — verbatim, in wire
/// arrival order, one batch per emitting plugin (#1819). A code an earlier
/// plugin claimed still publishes: the `code_owners` claim is `plugin/name`
/// suppression attribution only, and upstream `mergeAnalysisErrors`
/// (upstream Dart SDK source,
/// `pkg/analysis_server/lib/src/plugin/result_merger.dart`) concatenates
/// every plugin's diagnostics — duplicates included — so dropping a
/// claimed row here would under-report versus the oracle. Engine-level
/// dedup of an exact-equal row is the merge step's job
/// (`merge`'s `(file, offset, code, message)` key), not this one's.
fn accept_rows(
    cycle: &mut Cycle,
    name: &str,
    codes: &FxHashMap<String, CodeMeta>,
    rows: Vec<PluginDiagnostic>,
) {
    if rows.is_empty() {
        return;
    }
    cycle.findings.push(PluginBatch {
        name: name.to_owned(),
        codes: codes.clone(),
        rows,
    });
}

/// Ends a slot's session at its first surfaced runtime error: the runtime is
/// dropped (the isolation contract — the runtime has already spent its
/// budget reaching the error), and the failure records as the plugin's
/// one-shot `dartq/plugin_crashed` row, published against `anchor` in this
/// cycle or withheld (pending) until a cycle that has one. Shared by the
/// cycle's launch pass and its feed/drain error arm.
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
    /// merge step converts and publishes (never the engine vec itself; at
    /// most one per plugin per session).
    pub crashes: Vec<Diagnostic>,
}

/// One plugin's drained findings, in wire arrival order, plus the
/// census-derived `code → rule` map the per-directory `diagnostics`
/// rule-disable gate resolves against.
pub struct PluginBatch {
    /// The manifest name (the wire `source` tag for these rows).
    pub name: String,
    /// The plugin's census codes to their declaring rule's lowercased name —
    /// the rule-level `diagnostics` gate key; empty for a rule-less plugin.
    codes: FxHashMap<String, CodeMeta>,
    pub rows: Vec<PluginDiagnostic>,
}

/// One plugin finding resolved onto a resident file, ready for [`merge`].
pub struct PluginFinding {
    pub file: FileId,
    /// Emitting plugin's discovery-order index (the merge's grouping key).
    pub plugin: usize,
    /// The wire row's UTF-16 start offset (the within-plugin sort key).
    pub offset: u64,
    pub diagnostic: wire::Diagnostic,
}

/// The governing options for a file at `path` (its parent directory's
/// longest-prefix match over the governing set) — `None` when no governing
/// directory contains it, in which case upstream falls back to the
/// context's own `pluginConfigurations` (`_getConfigurationsForFile`'s
/// null arm, plugin_server.dart:719-735); the caller supplies the root
/// entry as that fallback.
fn governing_for<'a>(
    path: &Path,
    governing: &'a [&'a AnalysisOptions],
) -> Option<&'a AnalysisOptions> {
    governing
        .iter()
        .copied()
        .filter(|options| {
            options
                .file_path
                .parent()
                .is_some_and(|dir| path.starts_with(dir))
        })
        .max_by_key(|options| options.file_path.components().count())
}

/// The wire severity (1/2/3) the finding emits at under `diagnostics` (the
/// serving directory's `plugins/<name>/diagnostics` map), or `None` when the
/// row drops: a rule-name `disable` removes the finding entirely (the rule
/// is not enabled in that directory — upstream `registry.enabled` gates the
/// rule's run before any code emits), while a code-name entry re-levels
/// (`ConfiguredSeverity::severity` — `enable` keeps the pushed severity,
/// `disable` degrades to INFO through the upstream `?? INFO` fall).
fn configured_severity(
    batch: &PluginBatch,
    row: &PluginDiagnostic,
    diagnostics: &FxHashMap<String, ConfiguredSeverity>,
) -> Option<u8> {
    let code_lower = row.code.to_ascii_lowercase();
    if let Some(configured) = diagnostics.get(&code_lower) {
        match configured.severity() {
            Some(severity) => {
                return Some(match severity {
                    dartq_diagnostics::Severity::Error => 1,
                    dartq_diagnostics::Severity::Warning => 2,
                    dartq_diagnostics::Severity::Info => 3,
                });
            }
            // `enable` leaves the emitted severity standing.
            None => {}
        }
    }
    if let Some(meta) = batch.codes.get(&code_lower)
        && let Some(ConfiguredSeverity::Disable) = diagnostics.get(meta.rule.as_str())
    {
        return None;
    }
    Some(match row.severity {
        PluginSeverity::Error => 1,
        PluginSeverity::Warning => 2,
        PluginSeverity::Info => 3,
    })
}

/// Resolves drained plugin rows onto the files they name and converts them
/// to the wire shape. `resident` answers a `FileId` with its line index and
/// text (the session's engine faces); rows on files the engine does not
/// know are skipped — the same posture the transport's own feed takes for
/// unknown `FileId`s.
///
/// `governing` is the loaded options set of the workspace's governing
/// directories: the per-directory `plugins:` semantics apply at this
/// conversion — a plugin serves only the directories whose own `plugins:`
/// map declares it (a file under no governing directory falls back to the
/// context root's merged configuration), a `diagnostics` entry naming the
/// finding's *rule* with `disable` drops it, and one naming its *code*
/// re-levels the emitted severity (the upstream
/// `severityMapping`/`_configuredSeverity` face).
///
/// Offsets are UTF-16 end to end: the wire rows already carry UTF-16 units
/// and `line_col_utf16` consumes the same, so this is a position mapping,
/// never a unit conversion.
#[must_use]
pub fn resolve_findings<'a>(
    batches: &[PluginBatch],
    files: &FileTable,
    mut resident: impl FnMut(FileId) -> Option<(&'a LineIndex, &'a str)>,
    governing: &[&AnalysisOptions],
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
            // The serving directory's pluginMap for this plugin — `None`
            // drops the row (the plugin is not configured for that
            // directory). With no governing directory at all the context
            // root's merged options stand in (the upstream null arm).
            let row_path = Path::new(&row.file);
            let diagnostics = governing_for(row_path, governing)
                .or_else(|| governing.first().copied())
                .and_then(|options| serving_diagnostics(options, &batch.name));
            let Some(diagnostics) = diagnostics else {
                continue;
            };
            let Some(severity) = configured_severity(batch, row, diagnostics) else {
                continue;
            };
            findings.push(PluginFinding {
                file,
                plugin,
                offset: row.offset,
                diagnostic: to_wire(&batch.name, row, index, text, severity),
            });
        }
    }
    findings
}

/// Converts one plugin row to the wire shape: severity mapped to the
/// 1/2/3 scale, UTF-16 offsets mapped through the line index to zero-based
/// positions, the plugin's manifest name as the `source` tag.
///
/// Plugin offsets are untrusted: a stale row
/// can predate an edit that shrank the file. Both bounds clamp to the
/// text's UTF-16 length before mapping — the file-end degradation
/// [`wire_offset`] starts — so a malformed row degrades to zero-width at
/// the end instead of tripping [`LineIndex::line_col_utf16`]'s
/// engine-internal assert. The monotone clamp keeps `start <= end`.
fn to_wire(
    plugin: &str,
    row: &PluginDiagnostic,
    index: &LineIndex,
    text: &str,
    severity: u8,
) -> wire::Diagnostic {
    let total = utf16_len_of(text);
    let start = wire_offset(row.offset).min(total);
    let end = wire_offset(row.offset.saturating_add(row.length)).min(total);
    let (start_line, start_column) = index.line_col_utf16(text, start);
    let (end_line, end_column) = index.line_col_utf16(text, end);
    wire::Diagnostic {
        range: wire::Range {
            start: wire::Position {
                line: start_line - 1,
                character: start_column - 1,
            },
            end: wire::Position {
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
/// text's UTF-16-length bound is applied on top in [`to_wire`].
fn wire_offset(offset: u64) -> u32 {
    u32::try_from(offset).unwrap_or(u32::MAX)
}

/// The dedup key of the merge: file, range start, code, and message — the
/// T5 ruling's `(file, offset, code, message)` exact-equality in the merged
/// rows' wire coordinates. `message` (not source) is the key's discriminator:
/// a plugin row exactly equal to a built-in row is the same finding twice,
/// and the built-in row wins by seeding `seen` first; rows that differ in
/// wording keep both faces.
type DedupKey = (u32, u32, u32, String, String);

fn dedup_key(file: FileId, diagnostic: &wire::Diagnostic) -> DedupKey {
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
    engine_rows: impl IntoIterator<Item = (FileId, wire::Diagnostic)>,
    findings: Vec<PluginFinding>,
) -> Vec<(FileId, wire::Diagnostic)> {
    let mut merged: Vec<(FileId, wire::Diagnostic)> = engine_rows.into_iter().collect();
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

/// One plugin's bounded drain (the cycle's wait half): parks for at most
/// [budget] total, hands back every diagnostic that arrived plus whether
/// the drain was *degraded* — the budget expired before the plugin
/// quiesced, so its remaining rows publish on a later cycle (the timeout
/// isolation: the caller never waits past [budget] for a plugin) — and
/// pumps the plugin's reverse `request/query` frames through [server] as
/// they land — a producer that queries while its findings are computed
/// stalls itself, so the answers go out within the same drain instead of
/// past the plugin's own callback budget. A cycle without a server declines
/// the queries (the immediate error reply — the producer fails fast, the
/// session stays up).
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

/// The canonical reverse-query server (T4): one `request/query` answered
/// against the session's current engine view. The index kinds route through
/// the shared `dartq-commands` query faces — the reply is the frozen query
/// envelope, the same rows the daemon's own `dartq/query` answer carries
/// (schema-identical by construction). `element_at` is the offset-keyed
/// face over the engine's maintained index. `Err` rides the wire's error
/// channel (`{"error": message}`) — a declined query never tears the
/// session down.
pub(crate) fn serve_query(
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
                // `to_json` builds the envelope as `json!({..})`.
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

/// The `element_at` answer (T4): the element rows the `{file, offset}`
/// position denotes — its own declaration when the offset falls on a name
/// token, else the declarations the use at the offset resolves to (the
/// engine's maintained reference surface is the resolution). A miss is an
/// empty `elements` array: the position legitimately denotes nothing. An
/// unknown file is an error: the plugin asked about a file the session has
/// never seen.
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
            // A use site: the enclosing library's surface holds the
            // resolved reference rows; the position's row names its target.
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
        "schema_version": SCHEMA_VERSION,
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
/// convention. A plugin's offset is untrusted: the
/// clamp keeps `line_col_utf16`'s in-range assert out of a stale query's
/// reach, exactly like the findings path clamps in [`to_wire`].
fn line_col(engine: &Engine, file: FileId, offset: u32) -> (u32, u32) {
    let index = engine
        .line_index(file)
        .expect("captured file has a line index");
    let text = engine.content(file).expect("captured file has content");
    let text = std::str::from_utf8(text).expect("engine content is UTF-8");
    index.line_col_utf16(text, offset.min(utf16_len_of(text)))
}

/// Whether the UTF-16 [offset] falls inside [range] (half-open: the end
/// boundary belongs to the next token; a zero-length span holds nothing).
fn span_contains(range: SourceRange, offset: u32) -> bool {
    range.length > 0 && range.offset <= offset && offset < range.offset.saturating_add(range.length)
}

#[cfg(test)]
mod query_server_tests {
    //! The T4 reverse-query server over a real analyzed workspace: the
    //! index kinds answer the frozen envelope (schema-identical with the
    //! daemon's own `dartq/query` face), `element_at` resolves both faces
    //! (the position's own declaration, and the target a use resolves to).

    use dartq_engine::CancellationToken;
    use dartq_plugin::QueryRequest;

    use super::*;

    /// `lib/main.dart` of the fixture: imports the a.dart pair below.
    const MAIN: &str = "import 'a.dart';\nvoid main() { fa(1); }\n";
    /// `lib/a.dart`: one class and one function. All-ASCII, so byte
    /// offsets are UTF-16 offsets: `fa`'s name token spans 17..19.
    const A: &str = "class Foo {}\nint fa(int x) => x;\n";

    fn analyzed() -> (crate::test_fixtures::Workspace, Engine) {
        let workspace = crate::test_fixtures::Workspace::new(MAIN);
        std::fs::write(workspace.path("lib/a.dart"), A).expect("write a.dart");
        let mut engine = Engine::new(workspace.engine_config()).expect("engine");
        engine.discover_workspace().expect("discovery");
        engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
            .expect("analyze");
        (workspace, engine)
    }

    fn query(
        kind: QueryKind,
        name: Option<&str>,
        file: Option<&str>,
        offset: Option<u64>,
    ) -> PendingQuery {
        PendingQuery {
            id: 1,
            request: QueryRequest {
                kind,
                name: name.map(str::to_owned),
                file: file.map(str::to_owned),
                offset,
            },
        }
    }

    #[test]
    fn index_kinds_answer_the_frozen_envelope_schema_identically() {
        let (_workspace, engine) = analyzed();

        let symbol = serve_query(&engine, &query(QueryKind::Symbol, Some("fa"), None, None))
            .expect("symbol");
        assert_eq!(symbol["schema_version"], 1);
        assert_eq!(symbol["name"], "fa");
        let declarations = symbol["declarations"].as_array().expect("declarations");
        assert_eq!(declarations.len(), 1);
        assert_eq!(declarations[0]["name"], "fa");
        assert_eq!(declarations[0]["kind"], "function");

        // The daemon's own `dartq/query` face produces the same object: the
        // server reuses the shared routing and shaper verbatim.
        let run = commands_query::symbol_after_analysis(&engine, "fa");
        assert_eq!(
            Value::Object(symbol),
            commands_query::to_json(&run, "fa"),
            "the wire reply is the daemon face's own answer"
        );

        // `callers` keeps the invocation-shaped subset: `fa(1)` in main.
        let callers = serve_query(&engine, &query(QueryKind::Callers, Some("fa"), None, None))
            .expect("callers");
        let references = callers["references"].as_array().expect("references");
        assert_eq!(references.len(), 1);
        assert_eq!(references[0]["kind"], "invoked");

        // `dependencies` is the forward face: the declaring library's own
        // imports. `fa` lives in a.dart, which imports nothing, so the
        // additive array is omitted (the schema's empty-row omission) —
        // the importer answer (main.dart) is the impact face's, not this
        // kind's.
        let dependencies = serve_query(
            &engine,
            &query(QueryKind::Dependencies, Some("fa"), None, None),
        )
        .expect("dependencies");
        assert!(
            dependencies.get("dependencies").is_none(),
            "a.dart imports nothing — the additive array is omitted: {dependencies:?}"
        );
    }

    #[test]
    fn element_at_resolves_declarations_and_use_sites() {
        let (workspace, engine) = analyzed();
        // The wire spelling is the analyzed set's: the table's canonical
        // (absolute) path form.
        let a_path = workspace.path("lib/a.dart").to_string_lossy().into_owned();
        let main_path = workspace
            .path("lib/main.dart")
            .to_string_lossy()
            .into_owned();

        // On the name token: the position denotes its own declaration.
        let on_decl = serve_query(
            &engine,
            &query(QueryKind::ElementAt, None, Some(&a_path), Some(17)),
        )
        .expect("on the declaration");
        let elements = on_decl["elements"].as_array().expect("elements");
        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0]["name"], "fa");
        assert_eq!(elements[0]["kind"], "function");
        assert_eq!(elements[0]["line"], 2);
        assert_eq!(elements[0]["column"], 5);
        assert_eq!(elements[0]["length"], 2);

        // On the use: the position resolves to the target declaration.
        let on_use = serve_query(
            &engine,
            &query(QueryKind::ElementAt, None, Some(&main_path), Some(31)),
        )
        .expect("on the use");
        let elements = on_use["elements"].as_array().expect("elements");
        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0]["name"], "fa");
        assert_eq!(
            elements[0]["file"], a_path,
            "the target lives in the declaring file"
        );

        // Off either surface: the position denotes nothing — an empty
        // answer, not an error.
        let miss = serve_query(
            &engine,
            &query(QueryKind::ElementAt, None, Some(&main_path), Some(0)),
        )
        .expect("off any row");
        assert_eq!(
            miss["elements"].as_array().expect("elements").len(),
            0,
            "the import keyword denotes no element"
        );

        // A file the session never saw is the one error.
        let unknown = serve_query(
            &engine,
            &query(QueryKind::ElementAt, None, Some("lib/gone.dart"), Some(0)),
        )
        .expect_err("unknown file");
        assert!(unknown.contains("unknown file"), "{unknown}");
    }
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
    /// overlays, no governing options, and no query server (every test here
    /// runs dead-plugin sets — no live runtime ever reaches the pump).
    fn cycle_context<'a>() -> CycleContext<'a> {
        CycleContext {
            analyzed: &[],
            overlays: &[],
            governing_options: &[],
            query_server: None,
        }
    }

    fn wire_row(code: &str, source: &str, line: u32, character: u32) -> wire::Diagnostic {
        wire::Diagnostic {
            range: wire::Range {
                start: wire::Position { line, character },
                end: wire::Position { line, character },
            },
            severity: 2,
            code: code.to_owned(),
            source: source.to_owned(),
            message: "m".to_owned(),
        }
    }

    #[expect(clippy::cast_possible_truncation)] // fixture offsets are tiny; the cast only fills the wire character column
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
            diagnostic: wire_row(code, source, 0, offset as u32),
        }
    }

    fn plugin_row(file: &str, code: &str, offset: u64, length: u64) -> PluginDiagnostic {
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
            (a, wire_row("e/one", "dartq", 0, 0)),
            (a, wire_row("e/two", "dartq", 1, 0)),
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

        let mut builtin = wire_row("shared/code", "dartq", 0, 0);
        builtin.message = "the built-in wording".to_owned();
        let engine_rows = [(a, builtin)];

        let mut same = wire_row("shared/code", "echo", 0, 0);
        same.message = "the built-in wording".to_owned();
        let mut other = wire_row("shared/code", "echo", 0, 0);
        other.message = "the plugin's own wording".to_owned();
        let mut twin = wire_row("shared/code", "echo", 0, 0);
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

    /// One governing-options fixture declaring `plugin` with `diagnostics`
    /// entries — built through the engine parser so the test exercises the
    /// real merge, never a re-encoded map.
    fn options_with_plugin(file: &Path, plugin: &str, diagnostics: &str) -> AnalysisOptions {
        let text = if diagnostics.is_empty() {
            format!("plugins:\n  {plugin}:\n    path: .\n")
        } else {
            format!("plugins:\n  {plugin}:\n    path: .\n    diagnostics:\n{diagnostics}")
        };
        std::fs::write(file, text).expect("write options");
        let config = PackageConfig::parse(r#"{"configVersion": 2, "packages": []}"#)
            .expect("valid config");
        AnalysisOptions::load(file, &config, file.parent().unwrap_or(Path::new("")))
    }

    #[test]
    fn census_resolution_unions_enablement_across_governing_directories() {
        // Lint rules are disabled by default and enabled by an explicit
        // config; warning rules are enabled by default and disabled by an
        // explicit config (official `RegistryMixin.enabled`). With no
        // governing directory declaring the plugin, nothing resolves.
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
        let none: Vec<&AnalysisOptions> = Vec::new();
        let plain = resolve_census(&census, "p", &none);
        assert!(
            !plain[0].enabled && !plain[1].enabled,
            "a plugin no governing directory declares serves nothing"
        );

        let workspace = TempWorkspace::new("census_governing");
        let file = workspace.path().join("analysis_options.yaml");
        // One directory enables the lint, leaves the warning unconfigured
        // (still enabled — a warning rule's default-on posture holds while
        // the plugin is declared in its map).
        let declared =
            options_with_plugin(&file, "p", "      opt_in_lint: true\n      opt_out_warning: false\n");
        let governing: Vec<&AnalysisOptions> = vec![&declared];
        let resolved = resolve_census(&census, "p", &governing);
        assert!(resolved[0].enabled, "the explicit config enables the lint");
        assert!(
            !resolved[1].enabled,
            "the explicit config disables the warning rule"
        );
    }

    #[test]
    fn cycle_config_collects_enabled_rules_and_restricts_overlays() {
        let resolved = vec![
            ResolvedRule {
                name_lower: "kept".to_owned(),
                enabled: true,
            },
            ResolvedRule {
                name_lower: "dropped".to_owned(),
                enabled: false,
            },
        ];
        let analyzed = ["lib/a.dart".to_owned()];
        let overlays = [
            ("lib/a.dart".to_owned(), "edited text".to_owned()),
            ("lib/outside.dart".to_owned(), "not analyzed".to_owned()),
        ];
        let config = cycle_config(&resolved, &overlays, &analyzed);
        assert_eq!(config.enabled_rules, ["kept"]);
        // Per-directory re-leveling applies host-side at conversion; the
        // pushed map is empty.
        assert!(config.severity_overrides.is_empty());
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

    /// The code-claim collision ruling (#1819): upstream
    /// `pluginCodeMapping` is `putIfAbsent` — the first configured plugin
    /// owns a shared code for `plugin/name` suppression attribution — while
    /// upstream `mergeAnalysisErrors` concatenates every plugin's
    /// diagnostics verbatim. dartq mirrors both halves: two plugins
    /// emitting the same code both reach the findings, and the attribution
    /// keeps the first claimant.
    #[test]
    fn a_code_collision_keeps_every_plugins_rows_and_the_first_claim() {
        // Both plugins declare `shared/code` on an enabled warning rule
        // (warning rules are enabled by default — `resolve_census` with no
        // options section).
        let census_a = [census_rule(
            "rule_a",
            RuleKind::Warning,
            &[("Shared/Code", PluginSeverity::Warning)],
        )];
        let census_b = [census_rule(
            "rule_b",
            RuleKind::Warning,
            &[("shared/code", PluginSeverity::Warning)],
        )];
        // Both plugins are declared in one governing options file, so their
        // warning-kind rules resolve enabled (the default-on posture while
        // the plugin is in the directory's map).
        let workspace = TempWorkspace::new("collision");
        let file = workspace.path().join("analysis_options.yaml");
        let declared = options_with_plugin(&file, "plugin_a", "");
        let _ = file; // the second declaration rides the same options file.
        let file_b = workspace.path().join("nested").join("analysis_options.yaml");
        std::fs::create_dir_all(file_b.parent().expect("nested dir")).expect("nested dir");
        let declared_b = options_with_plugin(&file_b, "plugin_b", "");
        let governing: Vec<&AnalysisOptions> = vec![&declared, &declared_b];
        let mut set = PluginSet::default();
        claim_codes(
            &mut set.code_owners,
            0,
            &census_a,
            &resolve_census(&census_a, "plugin_a", &governing),
        );
        claim_codes(
            &mut set.code_owners,
            1,
            &census_b,
            &resolve_census(&census_b, "plugin_b", &governing),
        );
        // First-claim wins (case-insensitive key), and the second plugin's
        // declaration neither overwrites nor strips the row below.
        assert_eq!(
            set.code_owner("shared/code"),
            Some(0),
            "the discovery-order-first plugin owns the code"
        );

        // The drain side: both plugins' rows for the claimed code land —
        // nothing filters the later plugin's batch.
        let mut cycle = Cycle {
            findings: Vec::new(),
            crashes: Vec::new(),
        };
        let empty_codes = FxHashMap::default();
        accept_rows(
            &mut cycle,
            "plugin_a",
            &empty_codes,
            vec![plugin_row("a.dart", "shared/code", 0, 2)],
        );
        accept_rows(
            &mut cycle,
            "plugin_b",
            &empty_codes,
            vec![
                plugin_row("a.dart", "shared/code", 0, 2),
                plugin_row("a.dart", "plugin_b/solo", 4, 1),
            ],
        );
        assert_eq!(cycle.findings.len(), 2, "both plugins' batches survive");
        assert_eq!(cycle.findings[0].name, "plugin_a");
        assert_eq!(cycle.findings[1].name, "plugin_b");
        let codes: Vec<&str> = cycle
            .findings
            .iter()
            .flat_map(|batch| batch.rows.iter().map(|row| row.code.as_str()))
            .collect();
        assert_eq!(
            codes,
            ["shared/code", "shared/code", "plugin_b/solo"],
            "the claimed code's rows are not attributed away"
        );
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
            codes: FxHashMap::default(),
            rows: vec![
                plugin_row("a.dart", "echo/roundtrip", 3, 2),
                plugin_row("ghost.dart", "echo/ghost", 0, 0),
                plugin_row("b.dart", "echo/not_resident", 0, 0),
            ],
        }];

        let workspace = TempWorkspace::new("resolve_positions");
        let declared = options_with_plugin(
            &workspace.path().join("analysis_options.yaml"),
            "echo",
            "",
        );
        let findings =
            resolve_findings(&batches, &files, resident, &[&declared]);
        assert_eq!(findings.len(), 1, "unknown and non-resident files skip");
        let row = &findings[0];
        assert_eq!(row.file, a);
        assert_eq!(row.plugin, 0);
        assert_eq!(row.offset, 3);
        // "ab\ncd\ne": offset 3 starts line 2 col 1; the row's end offset 5
        // (the newline terminating line 2) answers line 2 col 3 — one-based
        // line_col answers, wire zero-based positions out.
        assert_eq!(
            row.diagnostic.range.start,
            wire::Position {
                line: 1,
                character: 0
            }
        );
        assert_eq!(
            row.diagnostic.range.end,
            wire::Position {
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
            codes: FxHashMap::default(),
            rows: vec![plugin_row("a.dart", "echo/stale", 10_000_000, 4)],
        }];

        let workspace = TempWorkspace::new("resolve_stale");
        let declared = options_with_plugin(
            &workspace.path().join("analysis_options.yaml"),
            "echo",
            "",
        );
        let findings = resolve_findings(&batches, &files, resident, &[&declared]);
        assert_eq!(findings.len(), 1, "the row publishes degraded");
        // Both bounds clamp to the file's UTF-16 end (7 units): zero-width
        // at EOF ("e" is line 3, so the 1-based (3, 2) answers wire (2, 1)).
        let eof = wire::Position {
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
            codes: FxHashMap::default(),
            rows: vec![
                plugin_row("a.dart", "echo/long_tail", 5, 1_000_000),
                plugin_row("a.dart", "echo/past_end", 100, 200),
            ],
        }];

        let workspace = TempWorkspace::new("resolve_bounds");
        let declared = options_with_plugin(
            &workspace.path().join("analysis_options.yaml"),
            "echo",
            "",
        );
        let findings = resolve_findings(&batches, &files, resident, &[&declared]);
        assert_eq!(findings.len(), 2);
        let eof = wire::Position {
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
        // Offset 5 survives intact (the newline ending line 2 → wire (1, 2));
        // the out-of-range end clamps to EOF.
        assert_eq!(
            range_of("echo/long_tail").start,
            wire::Position {
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
            code_owners: FxHashMap::default(),
        }
    }

    /// Launches over a workspace against the empty pub resolution — the
    /// test shape of [`PluginSet::launch`]'s three inputs (these fixtures
    /// ship no `analysis_options`, so the pub-native source is absent and
    /// only the convention scan runs).
    fn launch(workspace: &Path) -> PluginSet {
        let config =
            PackageConfig::parse(r#"{"configVersion": 2, "packages": []}"#).expect("valid config");
        PluginSet::launch(workspace, &config, &workspace.join(".dart_tool"))
    }

    /// The absent path of the core-first invariant, end to end: a
    /// nonexistent workspace root makes every absence probe — the options
    /// locator walk, the options read, the convention `read_dir` — miss,
    /// and the result is the empty set with no panic, no spawn, and no
    /// issue row. This is the most a plugin-free workspace can cost at
    /// launch; the per-cycle cost is [`PluginSet::is_empty`] alone.
    #[test]
    fn absent_plugin_configuration_costs_nothing_at_launch() {
        let nowhere = std::env::temp_dir().join(format!("dartq-absent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&nowhere);
        assert!(
            declared_plugin_sources(&nowhere).is_none(),
            "no options file declares nothing"
        );
        assert!(
            launch(&nowhere).is_empty(),
            "the empty discovery is the empty set"
        );
    }

    /// An options file without a `plugins:` section (the common
    /// configuration) is the same absent shape: `None` — no declaration,
    /// no allocation, no issue.
    #[test]
    fn options_without_a_plugins_section_declare_nothing() {
        let workspace = TempWorkspace::new("no_section");
        std::fs::write(
            workspace.path().join("analysis_options.yaml"),
            "linter:\n  rules:\n    avoid_print: true\n",
        )
        .expect("write options");
        assert!(declared_plugin_sources(workspace.path()).is_none());
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

        let mut set = launch(workspace.path());
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

        let set = launch(workspace.path());
        assert!(set.is_empty(), "every manifest was skipped");

        let empty = TempWorkspace::new("empty");
        std::fs::remove_dir(empty.path().join("plugins")).expect("drop plugins dir");
        assert!(launch(empty.path()).is_empty());
    }

    /// The launch seam over the real discovery face, driven from a
    /// `path:`-declared `plugins:` section — the daemon mirror of the CLI
    /// analyze face's identical pin: the manifest resolves through the same
    /// `path:`-augmented package config (the commands side's
    /// `declared_plugin_sources` + `plugin_package_config`, both faces'
    /// single fact source) and registers a lazy runtime — the set is
    /// non-empty with zero sidecar processes started (the spawn lands on
    /// the first cycle that has something to feed), and the plugin-free
    /// workspace next door discovers nothing. The empty package config
    /// forces the synthetic-entry path: without the augmentation the
    /// `path:`-declared name could not resolve at all.
    #[test]
    fn launch_discovers_path_declared_plugins_and_the_plugin_free_workspace_stays_empty() {
        let declared = TempWorkspace::new("path_declared");
        std::fs::write(
            declared.path().join("analysis_options.yaml"),
            "plugins:\n  demo:\n    path: plugins/demo\n",
        )
        .expect("options");
        let dir = declared.path().join("plugins/demo");
        std::fs::create_dir_all(dir.join("lib")).expect("plugin tree");
        std::fs::write(
            dir.join("dartq_plugin.json"),
            r#"{"name": "demo", "version": "1.0.0", "entrypoint": "lib/main.dart"}"#,
        )
        .expect("manifest");
        std::fs::write(dir.join("lib/main.dart"), "void main() {}\n").expect("entrypoint");

        let set = launch(declared.path());
        assert!(!set.is_empty(), "the declared plugin registers lazily");
        assert_eq!(
            set.plugins
                .iter()
                .map(|slot| slot.name.as_str())
                .collect::<Vec<_>>(),
            ["demo"],
            "the path:-declared manifest registers under its declared name"
        );
        set.shutdown();

        let plain = TempWorkspace::new("path_plain");
        let set = launch(plain.path());
        assert!(
            set.is_empty(),
            "no declaration, no convention manifest, no set"
        );
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

        let mut set = launch(workspace.path());
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

        // The cycle face recorded every hosted cycle: at least one counted,
        // with busy wall time on it (the per-plugin attribution the
        // benchmark reads; a cold `dart` boot also degrades some early
        // drains, which the degraded counter catches whenever it happens).
        let stats = set
            .runtime_stats("echo")
            .expect("the live runtime carries stats");
        assert!(stats.cycles >= 1, "cycles counted: {stats:?}");
        assert!(stats.busy_nanos > 0, "busy time recorded: {stats:?}");

        set.shutdown();
    }

    /// Serializes the env-mutating live test against any other test touching
    /// the process-wide environment (the dartq-snapshot `env_lock` shape):
    /// the runtime resolves its idle-eviction deadline from
    /// `DARTQ_PLUGIN_IDLE_EVICT_MS` at construction, and env writes are
    /// process-global.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .expect("env lock")
    }

    /// Removes `DARTQ_PLUGIN_IDLE_EVICT_MS` when dropped: the override is
    /// process-global, so a failing assert mid-test must still unarm it
    /// before the next serial test constructs a runtime.
    struct EvictMsOverride;

    impl Drop for EvictMsOverride {
        fn drop(&mut self) {
            // SAFETY: runs while `env_lock` is still held (the guard is
            // declared before this value, so it drops last).
            unsafe { std::env::remove_var("DARTQ_PLUGIN_IDLE_EVICT_MS") };
        }
    }

    /// Serial phase: pins the idle-eviction economics end to end (the
    /// module-note contract): a sidecar idle past its `idle_evict_ms` is
    /// evicted — counted in the stats' `evictions`, never surfaced as a
    /// crash row — and the next fed cycle respawns it budget-free (the
    /// restart counter does not move, and the plugin still answers).
    ///
    /// The env override arms a short deadline for measurement; it must be
    /// set before `launch` (the runtime constructor reads it) and the
    /// deadline clamps up to `request_ms + callback_ms`, so the manifest
    /// carries short budgets while the retry budget still prices a cold
    /// `dart` boot generously.
    #[test]
    #[ignore = "serial phase: spawns a real dart plugin process"]
    fn live_idle_eviction_counts_without_a_crash_and_respawns_budget_free() {
        const EVICT_MS: u64 = 10_500;
        let dart_ok = std::process::Command::new("dart")
            .arg("--version")
            .output()
            .is_ok();
        assert!(dart_ok, "dart is not on PATH");

        let script =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../dartq-plugin-host/bin/echo_plugin.dart");
        let workspace = TempWorkspace::new("evict");
        let manifest = format!(
            r#"{{"name": "echo", "version": "0.1.0", "entrypoint": "{}",
                "capabilities": ["lint"],
                "timeouts": {{"callback_ms": 500, "request_ms": 10000, "request_retries": 3}}}}"#,
            script.display(),
        );
        workspace.write_plugin("echo", &[("dartq_plugin.json", &manifest)]);

        let _guard = env_lock();
        // SAFETY: single-threaded set through env_lock; the drop guard
        // below removes the override even when an assert fails mid-test.
        unsafe { std::env::set_var("DARTQ_PLUGIN_IDLE_EVICT_MS", EVICT_MS.to_string()) };
        let _override = EvictMsOverride;
        let mut set = launch(workspace.path());
        assert!(!set.is_empty(), "the echo fixture discovered and launched");

        // The first generation: a cold `dart` boot rides the first cycle's
        // feed (the handshake); the echo reply lands within a later cycle's
        // bounded drain.
        let mut files = FileTable::new();
        let a = files.intern(&workspace.path().join("lib").join("a.dart"));
        let engine_rows = [engine_row(a, "hello")];
        let mut batch = None;
        for _ in 0..200 {
            let cycle = set.cycle(&engine_rows, &files, Some(a), &cycle_context());
            assert!(cycle.crashes.is_empty(), "the echo fixture must not crash");
            if let Some(found) = cycle.findings.into_iter().next() {
                batch = Some(found);
                break;
            }
        }
        assert_eq!(
            batch.expect("echo replied within the cycle budget").name,
            "echo"
        );
        let before = set
            .runtime_stats("echo")
            .expect("the live runtime carries stats");

        // Idle past the deadline: the monitor's eviction arm kills the
        // child within a poll tick of the deadline. No interaction touches
        // the clock during the wait.
        std::thread::sleep(Duration::from_millis(EVICT_MS + 1_500));
        let after_eviction = set
            .runtime_stats("echo")
            .expect("the evicted runtime stays in its slot");
        assert!(
            after_eviction.evictions >= 1,
            "the idle eviction is counted: {after_eviction:?}"
        );

        // The next fed cycle respawns budget-free: the plugin answers
        // again, no crash row surfaced, and the restart counter did not
        // move (an eviction is a host memory action, not a crash).
        let mut respawned = false;
        for _ in 0..200 {
            let cycle = set.cycle(&engine_rows, &files, Some(a), &cycle_context());
            assert!(
                cycle.crashes.is_empty(),
                "an eviction never surfaces a crash row: {:?}",
                cycle.crashes
            );
            if cycle.findings.into_iter().next().is_some() {
                respawned = true;
                break;
            }
        }
        assert!(respawned, "the plugin answers again after the eviction");
        let after = set
            .runtime_stats("echo")
            .expect("the live runtime carries stats");
        assert_eq!(
            after.restarts, before.restarts,
            "the eviction respawn is budget-free: {after:?}"
        );

        set.shutdown();
    }
}
