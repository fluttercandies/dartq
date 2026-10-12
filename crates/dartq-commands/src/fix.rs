//! The fix face's substrate (PRD §15.6): the anchored patch document, the
//! drift-correction applier, and the row source mapping. Both the CLI's
//! `dartq fix` and the daemon's `dartq/fix` consume this one implementation;
//! the CLI adds apply-to-disk and human formatting, the daemon adds the
//! JSON-RPC envelope.
//!
//! The machine document is §15.6 verbatim:
//!
//! ```json
//! { "file": "lib/foo.dart", "schema_version": 1,
//!   "edits": [
//!     { "offset": 123, "length": 8, "replacement": "Bar",
//!       "anchor": { "line": 10, "column": 7, "context_before": "final Fo",
//!                   "context_after": "() = " } } ] }
//! ```
//!
//! `offset`/`length` are the engine's UTF-16 `[offset, offset + length)`
//! span, directly applicable. The `anchor` (1-based line/column of the
//! edit start plus up to eight characters of context on each side, never
//! crossing a line) is the §15.6 drift-correction face: when concurrent
//! overlays shifted offsets, the applier re-locates the edit by its
//! context (and internal original-slice match); an anchor that is gone or
//! ambiguous marks the edit `stale` — the applier never writes at a
//! position it cannot prove.
//!
//! # Fix sources
//!
//! Patches materialize from the dartq-lints layer's diagnostic rows (its
//! `run` face): each wired rule whose official fix is a single-span
//! mechanical rewrite maps a hit to a span-exact rewrite whose source
//! slice is verified before the patch is offered — the keyword drops
//! (`unnecessary_new`, `unnecessary_late`, `unnecessary_const`,
//! `unnecessary_this`, `unnecessary_constructor_name`,
//! `unnecessary_string_escapes`), the syntax drops (`avoid_empty_else`,
//! `unnecessary_brace_in_string_interps`,
//! `unnecessary_string_interpolations`, `unnecessary_null_in_if_null_operators`,
//! `unnecessary_null_aware_assignments`,
//! `unnecessary_nullable_for_final_variable_declarations`), and the
//! rewrites (`prefer_adjacent_string_concatenation`,
//! `prefer_interpolation_to_compose_strings`, `prefer_if_null_operators`,
//! `prefer_null_aware_operators`, `prefer_is_not_operator`,
//! `prefer_contains`, `prefer_collection_literals`, `prefer_inlined_adds`,
//! `prefer_function_declarations_over_variables`, `avoid_init_to_null`)
//! — each mirroring its official `dart fix` producer's edit behavior
//! (removals, insertions, and replacements of the same spans; whitespace
//! never crosses a line). Rules without a mechanical rewrite are never
//! claimed, and a wired hit whose slice no longer verifies is dropped
//! rather than patched on faith. The structural rewrites verify against
//! the token stream (keyword positions are quote-safe only there); the
//! parse is paid once per `from_lints` call and only when one of their
//! hits is present.
//!
//! The rows reach [`from_lints`] through the lints layer's diagnostic
//! face: [`collect_set`] parses each discovered file once through the
//! layer's production entry ([`dartq_lints::parse_unit`] — the #355
//! bridge) and runs the file's governing-options lint set — the same
//! per-subtree resolution the analyze face applies, engine-free (the fix
//! face targets the paths you point it at; options load through
//! [`AnalysisOptions::find_for_dir`] + [`AnalysisOptions::load`], not an
//! engine's table). Rules without a mechanical rewrite are never
//! claimed, and a wired hit whose slice no longer verifies is dropped
//! rather than patched on faith. The built-in semantic fix set is its own
//! face (dartq-ai).
//!
//! # Plugin source (PRD §14, §15.6)
//!
//! Discovered plugin hosts contribute the second fix source. The
//! transport's protocol v1 carries plugin work on one request —
//! `request/fixes` (the `assist` capability is negotiated but has no v1
//! request, so assists ride the same face) — and [`PluginHosts`] owns the
//! dispatch: discovery under `<workspace_root>/plugins/`, one runtime per
//! plugin (spawn + handshake inside the manifest's budgets), a
//! `request/fixes` call per request location, and the isolation contract
//! (a plugin's first surfaced runtime error ends its session — the fix
//! run stays alive, the error surfaces in the answer's `errors` list).
//! [`from_plugin`] converts a reply into [`PluginFix`] rows anchored
//! against the file's current text: every edit is bounds-checked (a
//! change with any out-of-range edit is dropped whole — never half a
//! change), and the anchored edits then ride the same drift-correction
//! applier as the built-in rows.
//!
//! # Provenance
//!
//! Every emitted row is tagged with its source: `builtin-lint` for
//! [`RuleFix`] rows (by construction), the plugin's manifest name for
//! [`PluginFix`] rows. [`assists`] is the location-directed face over the
//! plugin source alone (the built-in layers contribute no assists); it
//! answers [`AssistAnswer`]s whose rows keep the change grouping the fix
//! document flattens away.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dartq_engine::analysis_options::AnalysisOptions;
use dartq_language::DEFAULT_LANGUAGE_VERSION;
use dartq_lints::{EnableSet, Features, Lint, RULES, parse_unit, run};
use dartq_package::PackageTable;
use dartq_plugin::{
    Capability, FixesReply, PluginRuntime, PrioritizedChange, SpawnSpec,
    discover as discover_plugins,
};
use dartq_pub::PackageConfig;
use dartq_scanner::{Keyword, TokenKind, TokenTable};
use dartq_source::{LineIndex, normalize_path, utf16_len_of, utf16_offset_to_byte};
use rustc_hash::FxHashMap;

use crate::Verbosity;
use crate::discover::{self, DiscoverError};
use crate::lints::features_for;

/// The patch document version (PRD §15.6; §15.7 versioning discipline).
pub const SCHEMA_VERSION: u32 = 1;

/// The provenance tag of every [`RuleFix`] row: the dartq-lints layer's
/// wired rules (the plugin rows carry the plugin's manifest name instead).
pub const BUILTIN_SOURCE: &str = "builtin-lint";

/// Context characters captured on each side of an edit. §15.6's example
/// shows eight before and five after; eight covers a keyword plus its
/// neighbor and the exact budget is ours to pick.
const ANCHOR_CONTEXT: usize = 8;

/// The edit's secondary locator: 1-based line/column of the edit start
/// plus the source context flanking it (empty at line boundaries, where
/// the machine document omits the field).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    pub line: u32,
    pub column: u32,
    pub context_before: String,
    pub context_after: String,
}

/// One anchored replacement over the UTF-16 span `[offset, offset +
/// length)`. `source` is the verified original slice — internal drift
/// matching, deliberately not part of the §15.6 document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub offset: u32,
    pub length: u32,
    pub source: String,
    pub replacement: String,
    pub anchor: Anchor,
}

/// One materialized fix: the anchored edit plus the human-facing row.
/// By construction a built-in row — the plugin source emits
/// [`PluginFix`]es.
#[derive(Debug, Clone)]
pub struct RuleFix {
    pub rule: &'static str,
    pub message: String,
    pub edit: Edit,
}

/// One plugin-proposed change: a titled proposal (the analyzer's
/// prioritized-change shape) whose edits anchor against the file's
/// current text and ride the same drift-correction applier as the
/// built-in rows. `plugin` is the provenance tag (the manifest name).
#[derive(Debug, Clone)]
pub struct PluginFix {
    pub plugin: String,
    pub message: String,
    /// Lower sorts first (analyzer convention), the reply's own order on
    /// ties.
    pub priority: i32,
    pub edits: Vec<Edit>,
}

/// One file's fix set: the built-in rows plus the plugin-proposed
/// changes. Files without any row never enter a set.
#[derive(Debug, Clone)]
pub struct FileFixSet {
    pub path: String,
    pub fixes: Vec<RuleFix>,
    pub plugin: Vec<PluginFix>,
}

/// A plugin's terminal failure for this run (spawn refusal, or the first
/// surfaced runtime error — the isolation contract ends the plugin's
/// session). Surfaced in the answer's `errors` list, never silently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginHostError {
    pub plugin: String,
    pub message: String,
}

/// The location-directed assist answer: the plugin source's rows for one
/// file plus the hosts that could not serve the request.
#[derive(Debug, Clone)]
pub struct AssistAnswer {
    pub file: String,
    pub assists: Vec<PluginFix>,
    pub errors: Vec<PluginHostError>,
}

/// Outcome of one edit against the file's current text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditOutcome {
    /// Written at the claimed offset or re-located by its anchor.
    Applied,
    /// The anchor is gone or ambiguous — skipped, per §15.6.
    Stale,
}

/// The plugin dispatch face (PRD §14): every plugin discovered under
/// `<workspace_root>/plugins/` with a live runtime, answering
/// `request/fixes` on the fix and assist faces' behalf.
///
/// [`PluginHosts::launch`] spawns eagerly and completes each handshake
/// before returning — the caller pays up to the manifest's
/// `request_ms × (retries + 1)` per plugin (defaults 10 s × 4). Spawn
/// failures log and surface once through [`PluginHosts::take_errors`];
/// they never fail the face. Dropping the set without
/// [`PluginHosts::shutdown`] kills the children without the graceful
/// handshake.
pub struct PluginHosts {
    hosts: Vec<Host>,
}

/// One plugin's dispatch state. `runtime: None` marks the plugin dead for
/// this run: the first surfaced runtime error means the runtime already
/// spent its restart budget reaching it, and further requests would burn
/// the manifest's budget on a plugin that cannot come back.
struct Host {
    name: String,
    runtime: Option<PluginRuntime>,
    error: Option<String>,
}

impl PluginHosts {
    /// Discovers plugins under `<root>/plugins/` (the convention; no
    /// overrides) and spawns each hit. The daemon passes its workspace
    /// root, the CLI its process working directory.
    #[must_use]
    pub fn launch(root: &Path) -> Self {
        let report = discover_plugins(root, &[]);
        for issue in &report.skipped {
            eprintln!(
                "dartq: skipping plugin manifest `{}`: {}",
                issue.path.display(),
                issue.error
            );
        }
        let hosts = report
            .plugins
            .into_iter()
            .map(|plugin| {
                let name = plugin.manifest.name.clone();
                match PluginRuntime::spawn(SpawnSpec::dart(&plugin.dir, plugin.manifest)) {
                    Ok(runtime) => Host {
                        name,
                        runtime: Some(runtime),
                        error: None,
                    },
                    Err(error) => {
                        eprintln!("dartq: plugin `{name}` failed to launch: {error}");
                        Host {
                            name,
                            runtime: None,
                            error: Some(error.to_string()),
                        }
                    }
                }
            })
            .collect();
        Self { hosts }
    }

    /// A host set with no plugins: the no-dispatch posture (tests, and
    /// callers that resolved their plugin posture before reaching here).
    #[must_use]
    pub fn empty() -> Self {
        Self { hosts: Vec::new() }
    }

    /// True when no plugin was discovered — the faces skip their dispatch
    /// entirely.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }

    /// Asks every live host for fixes at the UTF-16 span
    /// `[offset, offset + length)` of `file`, converting each reply to
    /// [`PluginFix`] rows anchored against `text` (see [`from_plugin`]).
    /// Protocol v1 carries the assists work on the same request; a host
    /// whose negotiated capability set lacks `fix` cannot serve it and is
    /// skipped. A host's first surfaced runtime error ends its session
    /// (the isolation contract) and surfaces through
    /// [`PluginHosts::take_errors`].
    pub fn fixes(
        &mut self,
        file: &str,
        offset: u64,
        length: u64,
        text: &str,
        lines: &LineIndex,
    ) -> Vec<PluginFix> {
        let mut rows = Vec::new();
        for host in &mut self.hosts {
            let Some(runtime) = host.runtime.as_mut() else {
                continue;
            };
            if !runtime
                .negotiated()
                .is_some_and(|caps| caps.contains(Capability::Fix))
            {
                continue;
            }
            match runtime.request_fixes(file, offset, length) {
                Ok(reply) => rows.extend(from_plugin(&host.name, &reply, text, lines)),
                Err(error) => {
                    host.runtime = None;
                    host.error = Some(error.to_string());
                }
            }
        }
        rows
    }

    /// Drains the hosts' terminal failures recorded so far (spawn
    /// refusals and first runtime errors), one per plugin.
    pub fn take_errors(&mut self) -> Vec<PluginHostError> {
        self.hosts
            .iter_mut()
            .filter_map(|host| {
                host.error.take().map(|message| PluginHostError {
                    plugin: host.name.clone(),
                    message,
                })
            })
            .collect()
    }

    /// The graceful teardown (`shutdown` per live runtime). Errors log;
    /// teardown quality does not affect the run's outcome.
    pub fn shutdown(self) {
        for host in self.hosts {
            if let Some(runtime) = host.runtime
                && let Err(error) = runtime.shutdown()
            {
                eprintln!("dartq: plugin `{}` shutdown failed: {error}", host.name);
            }
        }
    }
}

/// The fix source stage: validates the targets through discovery, then
/// parses each file once through the lints layer's production entry and
/// runs its governing-options lint set; every hit the mechanical
/// rewrites can materialize (and whose source slice verifies) becomes an
/// anchored edit, and every lint hit is a `request/fixes` location for
/// the plugin hosts (one request per hit per live host — the protocol's
/// only query granularity). One fix set per patched file — files without
/// rows never enter the set. Each file runs under its containing package's
/// language version ([`unit_features`]).
///
/// The lint set is per-subtree, the analyze face's resolution
/// ([`crate::lints::analyze_lints`]): each file runs under the options
/// entry its own ancestry governs ([`AnalysisOptions::find_for_dir`]),
/// loaded once per distinct options file and keyed once per distinct
/// directory. No options file in the ancestry is the official
/// options-less posture — the empty set, and the file skips the parse
/// (the opt-in cost ruling): nothing runs and no lint hit exists to
/// dispatch on.
///
/// # Errors
///
/// [`DiscoverError`] from target discovery.
pub fn collect_set(
    paths: &[PathBuf],
    hosts: &mut PluginHosts,
) -> Result<Vec<FileFixSet>, DiscoverError> {
    let files = discover::discover(paths)?;
    // One workspace assembly per run, seeded from the first discovered
    // file, best-effort: a target set with no pub context around it — the
    // face's historical shape — stays at the default-version posture
    // instead of failing.
    let assembly = files
        .iter()
        .next()
        .and_then(|first| first.parent())
        .and_then(|dir| crate::workspace::package_assembly(dir).ok());
    // The options-include faces: the assembly's package resolution when it
    // succeeded; a failed assembly degrades to the empty resolution — a
    // `package:` include then contributes nothing (the official
    // unresolvable-include posture) while the options file's own rules
    // still load.
    let (table, packages, config_dir) = match assembly {
        Some((table, packages, config_dir, ..)) => (Some(table), packages, config_dir),
        None => (
            None,
            Arc::new(PackageConfig {
                packages: Vec::new(),
                generator: None,
                generator_version: None,
                additional_properties: BTreeMap::new(),
            }),
            PathBuf::new(),
        ),
    };
    // The per-directory enable sets (governance is by ancestry, so one
    // directory always maps to one entry) and the per-file loaded options
    // (shared through the map, workspace members under one repo-root
    // options file parse it once) — the analyze face's serial precompute,
    // hoisted into the walk's cold path.
    let mut enabled: FxHashMap<PathBuf, EnableSet> = FxHashMap::default();
    let mut loaded: FxHashMap<PathBuf, Arc<AnalysisOptions>> = FxHashMap::default();
    let mut out = Vec::new();
    for path in &files {
        let Some(dir) = path.parent() else {
            continue;
        };
        let set = enabled.entry(dir.to_path_buf()).or_insert_with(|| {
            let rules = AnalysisOptions::find_for_dir(dir).map(|file| {
                loaded
                    .entry(file.clone())
                    .or_insert_with(|| {
                        Arc::new(AnalysisOptions::load(&file, &packages, &config_dir))
                    })
                    .lint_rules
                    .clone()
            });
            crate::lints::enable_set(rules.as_deref())
        });
        if *set == EnableSet::none() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            // Unreadable or non-UTF-8 target: nothing to patch — the apply
            // face re-reads and reports on its own terms.
            continue;
        };
        let parsed = parse_unit(&text, unit_features(table.as_ref(), path));
        // The empty unignorable posture: the fix face reports no
        // `unignorable_ignore` rows of its own (the analyze face's
        // computation is the owning lane's wiring point).
        let unignorable = rustc_hash::FxHashSet::default();
        let hits: Vec<Lint> = run(&parsed.unit(Some(path)), set, &unignorable);
        let patches = from_lints(&hits, &text, &parsed.lines);
        let plugin = dispatch_fixes(hosts, path, &text, &parsed.lines, &hits);
        if !patches.is_empty() || !plugin.is_empty() {
            out.push(FileFixSet {
                path: path.display().to_string(),
                fixes: patches,
                plugin,
            });
        }
    }
    Ok(out)
}

/// The file's unit features: its containing package's language version
/// through the package table's attribution walk (the engine's
/// `language_version_for` chain, engine-api-spec §1.1), the default
/// version where no table or no package attributes the file. `table` is
/// `None` when the run's workspace assembly failed — every file then runs
/// at the default posture, the face's pre-wiring behavior. This is the
/// package-version fallback the lint face also uses for unsettled files
/// (#722): `collect_set` discovers and parses engine-free, so the
/// engine's settled `@dart`/grant face cannot reach here — a `@dart`-pinned
/// file parses at its package posture, not its pinned one.
fn unit_features(table: Option<&PackageTable>, path: &Path) -> Features {
    // The query joins the table's spelling universe before the walk: the
    // attribution is purely lexical over the table's canonicalized roots
    // (the build's one normalize rule), while this face's paths come raw
    // off discovery — a user-spelled alias (`/var` vs `/private/var`, a
    // symlinked root) would strand at the walk's bound, attribute to no
    // package, and silently run the file at the default posture.
    let version = table
        .and_then(|table| table.containing(&normalize_path(path)))
        .and_then(|identity| identity.language_version)
        .map_or(
            DEFAULT_LANGUAGE_VERSION,
            dartq_pub::LanguageVersion::into_inner,
        );
    features_for(version)
}

/// One `request/fixes` per lint hit against the hosts. The request names
/// the file the way the plugin knows it — the absolute path, since the
/// engine's own diagnostic feed uses the file table's absolute paths.
fn dispatch_fixes(
    hosts: &mut PluginHosts,
    path: &Path,
    text: &str,
    lines: &LineIndex,
    hits: &[Lint],
) -> Vec<PluginFix> {
    if hosts.is_empty() {
        return Vec::new();
    }
    let file = host_path(path);
    let mut rows = Vec::new();
    for hit in hits {
        rows.extend(hosts.fixes(
            &file,
            u64::from(hit.range.start),
            u64::from(hit.range.end - hit.range.start),
            text,
            lines,
        ));
    }
    rows
}

/// The path string a plugin request carries: absolute when the discovery
/// yielded a relative argument (the CLI resolves against the process cwd,
/// the daemon's faces already resolve against the workspace root).
fn host_path(path: &Path) -> String {
    if path.is_absolute() {
        return path.display().to_string();
    }
    let joined = std::env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path));
    joined.display().to_string()
}

// ---------------------------------------------------------------------------
// Anchoring substrate
// ---------------------------------------------------------------------------

/// Anchors an edit over the UTF-16 span `[offset16, offset16 + length16)`.
#[must_use]
pub fn anchored_edit(
    text: &str,
    lines: &LineIndex,
    offset16: u32,
    length16: u32,
    replacement: &str,
) -> Edit {
    let (line, column) = lines.line_col_utf16(text, offset16);
    let start = utf16_offset_to_byte(text, offset16);
    let end = utf16_offset_to_byte(text, offset16 + length16);
    Edit {
        offset: offset16,
        length: length16,
        source: text[start..end].to_owned(),
        replacement: replacement.to_owned(),
        anchor: Anchor {
            line,
            column,
            context_before: line_context_before(text, start),
            context_after: line_context_after(text, end),
        },
    }
}

/// Up to [`ANCHOR_CONTEXT`] characters before `start`, never crossing the
/// line start.
fn line_context_before(text: &str, start: usize) -> String {
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let taken: Vec<char> = text[line_start..start]
        .chars()
        .rev()
        .take(ANCHOR_CONTEXT)
        .collect();
    taken.into_iter().rev().collect()
}

/// Up to [`ANCHOR_CONTEXT`] characters after `end`, never crossing the
/// line end.
fn line_context_after(text: &str, end: usize) -> String {
    let line_end = text[end..].find('\n').map_or(text.len(), |i| end + i);
    text[end..line_end].chars().take(ANCHOR_CONTEXT).collect()
}

/// Whether `text` still holds `edit` at byte span `[start, end)`: the
/// original slice intact and the anchor context flanking it.
fn holds_at(text: &str, edit: &Edit, start: usize, end: usize) -> bool {
    text.get(start..end) == Some(edit.source.as_str())
        && text[..start].ends_with(edit.anchor.context_before.as_str())
        && text[end..].starts_with(edit.anchor.context_after.as_str())
}

/// The byte position to write `edit` at in `text`, or `None` for stale:
/// the claimed offset when the original slice still sits there flanked by
/// the anchor context, else the unique re-location — the drift-correction
/// pass of §15.6. More than one candidate (on the anchor line, or anywhere
/// when no candidate sits on it) is ambiguous and goes stale.
#[must_use]
fn locate(text: &str, edit: &Edit) -> Option<usize> {
    // Exact position first, guarded against offsets drifted past the end.
    if edit.offset + edit.length <= utf16_len_of(text) {
        let start = utf16_offset_to_byte(text, edit.offset);
        let end = start + edit.source.len();
        if holds_at(text, edit, start, end) {
            return Some(start);
        }
    }
    // Re-location: every position where the original slice survives with
    // the anchor context flanking it. Line numbers come from a fresh
    // [`LineIndex`] of the current text — the same index the anchor was
    // born from in [`anchored_edit`], so both faces agree on what "the
    // anchor line" means (the applier's text mutates per write, so an
    // index shared across edits would read stale line starts).
    let lines = LineIndex::new(text);
    let mut line_match: Option<usize> = None;
    let mut line_matches = 0;
    let mut any_match: Option<usize> = None;
    let mut total = 0;
    for (start, _) in text.match_indices(edit.source.as_str()) {
        let end = start + edit.source.len();
        if !holds_at(text, edit, start, end) {
            continue;
        }
        total += 1;
        if any_match.is_none() {
            any_match = Some(start);
        }
        let (line, _) = lines.byte_to_line_col(u32::try_from(start).expect("offset exceeds u32"));
        if line == edit.anchor.line {
            line_matches += 1;
            line_match = Some(start);
        }
    }
    match (line_matches, total) {
        (1, _) => line_match,
        (0, 1) => any_match,
        _ => None,
    }
}

/// Applies `edits` to `text`: exact-position validation, then anchor
/// re-location; overlapping edits and stale anchors are skipped. Returns
/// the patched text plus one outcome per edit, in input order.
///
/// Edits apply in descending offset order, and every locator runs against
/// the *snapshot* text — the buffer as it stood when the fixes were born.
/// A sibling write higher up never disturbs a lower edit's proof (its
/// slice and anchor context still read as they did at birth), which is
/// what keeps a multi-edit change from stranding half of itself: located
/// snapshot spans translate into the patched buffer through the writes
/// already placed below them (each contributes its byte delta). The
/// overlap guard runs in snapshot coordinates too, so a re-located write
/// never shifts a later edit's decision about its own original span.
#[must_use]
pub fn apply(text: &str, edits: &[Edit]) -> (String, Vec<EditOutcome>) {
    let mut order: Vec<usize> = (0..edits.len()).collect();
    order.sort_by(|&a, &b| edits[b].offset.cmp(&edits[a].offset));
    let mut patched = text.to_owned();
    let mut outcomes: Vec<EditOutcome> = vec![EditOutcome::Stale; edits.len()];
    // Snapshot UTF-16 start of the lowest already-placed edit above.
    let mut applied_floor = u32::MAX;
    // Writes already placed, in snapshot byte coordinates: the pristine
    // span each replaced, and the byte delta its replacement contributes
    // to every position above it.
    let mut placed: Vec<(usize, usize, i64)> = Vec::new();
    for &i in &order {
        let edit = &edits[i];
        if edit.offset.saturating_add(edit.length) > applied_floor {
            continue; // overlaps a write already placed above it
        }
        let Some(start) = locate(text, edit) else {
            continue;
        };
        let end = start + edit.source.len();
        // Translate the snapshot span into the patched buffer's
        // coordinates; a span reaching into (not merely touching) a
        // placed write is unplaceable — stale.
        let shifted = |pos: usize| -> Option<usize> {
            let mut shifted = i64::try_from(pos).ok()?;
            for &(placed_start, placed_end, delta) in &placed {
                if placed_end <= pos {
                    shifted += delta;
                } else if placed_start < pos {
                    return None;
                }
            }
            usize::try_from(shifted).ok()
        };
        let (Some(start), Some(end)) = (shifted(start), shifted(end)) else {
            continue;
        };
        let delta = i64::try_from(edit.replacement.len()).unwrap_or(i64::MAX)
            - i64::try_from(edit.source.len()).unwrap_or(i64::MAX);
        patched.replace_range(start..end, &edit.replacement);
        placed.push((start, end, delta));
        applied_floor = edit.offset;
        outcomes[i] = EditOutcome::Applied;
    }
    (patched, outcomes)
}

// ---------------------------------------------------------------------------
// Fix source: the dartq-lints layer's diagnostic rows
// ---------------------------------------------------------------------------

/// Maps the lints layer's rows to anchored edits — the fix source of this
/// round (module docs). Rules without a span-exact mechanical rewrite
/// never produce an edit; a wired hit whose slice no longer verifies is
/// dropped rather than patched on faith.
#[must_use]
pub fn from_lints(hits: &[Lint], text: &str, lines: &LineIndex) -> Vec<RuleFix> {
    // The structural rewriters verify against the token stream (keyword
    // positions are quote-safe only there); the parse is paid once per
    // call, and only when one of their hits is present.
    let needs_tokens = hits.iter().any(|hit| {
        matches!(
            RULES[hit.rule.0 as usize].name,
            "prefer_is_not_operator"
                | "prefer_if_null_operators"
                | "prefer_null_aware_operators"
                | "prefer_contains"
                | "prefer_interpolation_to_compose_strings"
                | "prefer_collection_literals"
                | "prefer_inlined_adds"
                | "prefer_function_declarations_over_variables"
        )
    });
    let tokens = needs_tokens.then(|| parse_unit(text, Features::default()).tokens);
    hits.iter()
        .filter_map(|hit| {
            let rule = RULES[hit.rule.0 as usize].name;
            let (offset, length, replacement) = match rule {
                "unnecessary_new" => drop_keyword(text, hit.range.start, "new")?,
                "unnecessary_late" => drop_keyword(text, hit.range.start, "late")?,
                "unnecessary_const" => drop_keyword(text, hit.range.start, "const")?,
                "avoid_empty_else" => replace_exact(text, &hit.range, ";", "{}")?,
                "unnecessary_brace_in_string_interps" => unbrace(text, &hit.range)?,
                "unnecessary_this" => drop_this_qualifier(text, hit.range.start)?,
                "unnecessary_string_escapes" => replace_exact(text, &hit.range, "\\", "")?,
                "unnecessary_string_interpolations" => {
                    unwrap_string_interpolation(text, &hit.range)?
                }
                "unnecessary_null_in_if_null_operators" => drop_if_null_null(text, &hit.range)?,
                "unnecessary_null_aware_assignments" => {
                    remove_null_aware_assignment(text, &hit.range)?
                }
                "unnecessary_nullable_for_final_variable_declarations" => {
                    drop_type_question_mark(text, hit.range.start)?
                }
                "avoid_init_to_null" => drop_null_initializer(text, &hit.range)?,
                "unnecessary_constructor_name" => drop_dot_new(text, hit.range.start)?,
                "prefer_adjacent_string_concatenation" => drop_keyword(text, hit.range.start, "+")?,
                "prefer_is_not_operator" => {
                    token_rewrite(tokens.as_ref(), text, &hit.range, is_not_operator)?
                }
                "prefer_if_null_operators" => {
                    token_rewrite(tokens.as_ref(), text, &hit.range, if_null_operators)?
                }
                "prefer_null_aware_operators" => {
                    token_rewrite(tokens.as_ref(), text, &hit.range, null_aware_operators)?
                }
                "prefer_contains" => {
                    token_rewrite(tokens.as_ref(), text, &hit.range, contains_call)?
                }
                "prefer_interpolation_to_compose_strings" => {
                    token_rewrite(tokens.as_ref(), text, &hit.range, compose_interpolation)?
                }
                "prefer_collection_literals" => {
                    token_rewrite(tokens.as_ref(), text, &hit.range, collection_literal)?
                }
                "prefer_inlined_adds" => {
                    token_rewrite(tokens.as_ref(), text, &hit.range, inlined_adds)?
                }
                "prefer_function_declarations_over_variables" => {
                    token_rewrite(tokens.as_ref(), text, &hit.range, function_declaration)?
                }
                _ => return None,
            };
            Some(RuleFix {
                rule,
                message: hit.message.clone(),
                edit: anchored_edit(text, lines, offset, length, &replacement),
            })
        })
        .collect()
}

/// Maps one plugin's `request/fixes` reply to anchored rows, priority
/// first (the reply's own order on ties — the sort is stable). Every edit
/// is bounds-checked against `text` before anchoring — plugin offsets are
/// untrusted wire data and [`anchored_edit`]'s converter panics on
/// out-of-range offsets — and one bad edit drops the whole change: a
/// proposal is applied atomically or not at all, never half-applied.
#[must_use]
pub fn from_plugin(
    plugin: &str,
    reply: &FixesReply,
    text: &str,
    lines: &LineIndex,
) -> Vec<PluginFix> {
    let len16 = utf16_len_of(text);
    let mut order: Vec<&PrioritizedChange> = reply.fixes.iter().collect();
    order.sort_by_key(|change| change.priority);
    order
        .into_iter()
        .filter_map(|change| {
            let mut edits = Vec::with_capacity(change.edits.len());
            for edit in &change.edits {
                let offset = u32::try_from(edit.offset).ok()?;
                let length = u32::try_from(edit.length).ok()?;
                if offset.checked_add(length)? > len16 {
                    return None;
                }
                edits.push(anchored_edit(
                    text,
                    lines,
                    offset,
                    length,
                    &edit.replacement,
                ));
            }
            Some(PluginFix {
                plugin: plugin.to_owned(),
                message: change.message.clone(),
                priority: change.priority,
                edits,
            })
        })
        .collect()
}

/// The location-directed assist face: the plugin hosts' proposals for one
/// file at the UTF-16 span `[offset, offset + length)`, anchored against
/// the file's on-disk text. The built-in layers contribute no assists —
/// this face is the plugin source alone.
///
/// # Errors
///
/// [`DiscoverError::Missing`] when the target does not exist (the usage
/// tier, the fix face's message body), [`DiscoverError::Io`] for any
/// other read failure.
pub fn assists(
    path: &Path,
    offset: u64,
    length: u64,
    hosts: &mut PluginHosts,
) -> Result<AssistAnswer, DiscoverError> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            DiscoverError::Missing(path.display().to_string())
        } else {
            DiscoverError::Io(error)
        }
    })?;
    let lines = LineIndex::new(&text);
    let assists = hosts.fixes(&host_path(path), offset, length, &text, &lines);
    Ok(AssistAnswer {
        file: path.display().to_string(),
        assists,
        errors: hosts.take_errors(),
    })
}

/// A width-0 keyword hit (`new`, `late`): the rewrite drops the keyword
/// plus its trailing horizontal whitespace (a newline stays — joining
/// lines is a formatting decision, not a fix). The keyword is ASCII, so
/// its lexeme length is its UTF-16 length.
fn drop_keyword(text: &str, offset16: u32, keyword: &str) -> Option<(u32, u32, String)> {
    debug_assert_eq!(
        keyword.len(),
        keyword.chars().map(char::len_utf16).sum::<usize>()
    );
    let start = utf16_offset_to_byte(text, offset16);
    let rest = text.get(start..)?.strip_prefix(keyword)?;
    let ws = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    let length = u32::try_from(keyword.len() + ws).ok()?;
    Some((offset16, length, String::new()))
}

/// A span-exact rewrite: the hit span must slice to exactly `expect`.
fn replace_exact(
    text: &str,
    range: &core::ops::Range<u32>,
    expect: &str,
    replacement: &str,
) -> Option<(u32, u32, String)> {
    let start = utf16_offset_to_byte(text, range.start);
    let end = utf16_offset_to_byte(text, range.end);
    text.get(start..end).filter(|slice| *slice == expect)?;
    Some((range.start, range.end - range.start, replacement.to_owned()))
}

/// `${id}` → `$id`: the hit span is the whole interpolation group (the
/// rule reports it exactly); the `${…}` shape and a bare ASCII identifier
/// body are verified before rewriting.
fn unbrace(text: &str, range: &core::ops::Range<u32>) -> Option<(u32, u32, String)> {
    let start = utf16_offset_to_byte(text, range.start);
    let end = utf16_offset_to_byte(text, range.end);
    let slice = text.get(start..end)?;
    let inner = slice.strip_prefix("${")?.strip_suffix('}')?;
    if inner.is_empty() || !inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some((range.start, range.end - range.start, format!("${inner}")))
}

/// The UTF-16 length of a string slice (the edits' currency).
fn utf16_count(slice: &str) -> u32 {
    u32::try_from(slice.chars().map(char::len_utf16).sum::<usize>()).unwrap_or(u32::MAX)
}

/// The byte slice of `text` under the UTF-16 `range`.
fn slice16<'s>(text: &'s str, range: &core::ops::Range<u32>) -> Option<&'s str> {
    text.get(utf16_offset_to_byte(text, range.start)..utf16_offset_to_byte(text, range.end))
}

/// The leading (or trailing) run of `quote` bytes.
fn quote_run(bytes: &[u8], quote: u8, trailing: bool) -> usize {
    if trailing {
        bytes.iter().rev().take_while(|&&b| b == quote).count()
    } else {
        bytes.iter().take_while(|&&b| b == quote).count()
    }
}

/// A width-0 `this` hit: the rewrite drops the `this.` qualifier — the
/// keyword plus horizontal whitespace and the dot (the official
/// `RemoveThisExpression` deletes from the keyword through the operator;
/// a newline never joins).
fn drop_this_qualifier(text: &str, offset16: u32) -> Option<(u32, u32, String)> {
    let start = utf16_offset_to_byte(text, offset16);
    let rest = text.get(start..)?.strip_prefix("this")?;
    let ws = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    if !rest[ws..].starts_with('.') {
        return None;
    }
    Some((offset16, 4 + u32::try_from(ws).ok()? + 1, String::new()))
}

/// The whole-literal hit of an interpolation of one string literal
/// (`''${'x'}''`): the rewrite keeps the inner literal verbatim (the
/// official producer deletes the wrapper ranges around it). The empty
/// quote-run wrappers and the `${…}` group are verified first.
fn unwrap_string_interpolation(
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    let bytes = slice16(text, range)?.as_bytes();
    let quote = *bytes.first()?;
    if quote != b'\'' && quote != b'"' {
        return None;
    }
    let lead = quote_run(bytes, quote, false);
    let trail = quote_run(bytes, quote, true);
    // 1/3 quotes: the plain and triple-quote wrappers; 2: the empty
    // adjacent-string face (`''${'x'}''`). The runs must match.
    if lead != trail || !matches!(lead, 1..=3) {
        return None;
    }
    let body = bytes.get(lead..bytes.len() - trail)?.strip_prefix(b"${")?;
    if body.last() != Some(&b'}') {
        return None;
    }
    // The kept expression is the string literal between `${` and the
    // final `}`: it opens with its own quote (optionally raw) and closes
    // with the matching one.
    let inner = &body[..body.len() - 1];
    let open = match inner.first()? {
        b'\'' | b'"' => inner.first()?,
        b'r' => inner.get(1)?,
        _ => return None,
    };
    if *open != b'\'' && *open != b'"' {
        return None;
    }
    if inner.last() != Some(open) || inner.len() < 2 {
        return None;
    }
    let start = utf16_offset_to_byte(text, range.start);
    let first = start + lead + 2;
    let last = start + bytes.len() - trail - 1;
    Some((
        range.start,
        range.end - range.start,
        text[first..last].to_owned(),
    ))
}

/// A `null`-operand hit under `??`: the rewrite drops the operator with
/// the operand (`a ?? null` → `a`, `null ?? a` → `a`; the official
/// deletion spans operator and operand together). The intervening
/// whitespace is horizontal only — a newline stays.
fn drop_if_null_null(text: &str, range: &core::ops::Range<u32>) -> Option<(u32, u32, String)> {
    replace_exact(text, range, "null", "")?;
    let start = utf16_offset_to_byte(text, range.start);
    let end = utf16_offset_to_byte(text, range.end);
    // Right operand (`a ?? null`): the deletion starts at the whitespace
    // before the operator — the official span runs from the left operand's
    // end through the null.
    if let Some(head) = text.get(..start) {
        let op_ws = head.len() - head.trim_end_matches([' ', '\t']).len();
        if head[..head.len() - op_ws].ends_with("??") {
            let before = &head[..head.len() - op_ws - 2];
            let lead = before.len() - before.trim_end_matches([' ', '\t']).len();
            let at = u32::try_from(start - op_ws - 2 - lead).ok()?;
            return Some((at, range.end - at, String::new()));
        }
    }
    // Left operand (`null ?? a`): the text after opens with the operator.
    let rest = text.get(end..)?;
    let ws = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    let after = &rest[ws..];
    if after.starts_with("??") {
        let tail = &after[2..];
        let tail_ws = tail.len() - tail.trim_start_matches([' ', '\t']).len();
        return Some((
            range.start,
            4 + u32::try_from(ws + 2 + tail_ws).ok()?,
            String::new(),
        ));
    }
    None
}

/// The whole-assignment hit of `x ??= null`: a lone expression-statement
/// line loses the whole line (the official `getLinesRange` face); any
/// other context loses `??= null`.
fn remove_null_aware_assignment(
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    let slice = slice16(text, range)?;
    let eq = u32::try_from(slice.find("??=")?).ok()?;
    let rhs = slice.get(eq as usize + 3..)?;
    if rhs.trim() != "null" || slice[..eq as usize].trim().is_empty() {
        return None;
    }
    let start = utf16_offset_to_byte(text, range.start);
    let end = utf16_offset_to_byte(text, range.end);
    // The statement-line face: nothing but whitespace before the hit on
    // its line, and `;` plus horizontal whitespace to the line end after.
    // The whole line goes — the official `getLinesRange` includes the
    // line's own indentation — through the line terminator.
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    if text[line_start..start].trim().is_empty()
        && let Some(rest) = text.get(end..)
    {
        let after = rest.trim_start_matches([' ', '\t']);
        if let Some(after_semi) = after.strip_prefix(';') {
            let trailing = after_semi.len() - after_semi.trim_start_matches([' ', '\t']).len();
            let tail = &after_semi[trailing..];
            let eol = if tail.starts_with("\r\n") {
                2
            } else if tail.starts_with('\n') {
                1
            } else {
                tail.is_empty().then_some(0)?
            };
            let deleted = utf16_count(&text[line_start..end + 1 + trailing + eol]);
            return Some((
                u32::try_from(utf16_count(&text[..line_start])).ok()?,
                deleted,
                String::new(),
            ));
        }
    }
    // The expression face: the official deletion runs from the left-hand
    // side's end through the value's — the whitespace before the operator
    // goes with it.
    let head = &slice[..eq as usize];
    let ws = head.len() - head.trim_end_matches([' ', '\t']).len();
    let at = range.start + eq - u32::try_from(ws).ok()?;
    Some((at, range.end - at, String::new()))
}

fn drop_null_initializer(text: &str, range: &core::ops::Range<u32>) -> Option<(u32, u32, String)> {
    let slice = slice16(text, range)?;
    let eq = u32::try_from(slice.rfind('=')?).ok()?;
    // The initializer is exactly `null` and nothing before the `=` reads
    // as an assignment of its own. The deletion carries the whitespace
    // before the `=` (the official deletion runs from the name's end).
    if slice.get(eq as usize + 1..)?.trim() != "null" || slice[..eq as usize].contains('=') {
        return None;
    }
    let head = &slice[..eq as usize];
    let ws = head.len() - head.trim_end_matches([' ', '\t']).len();
    let at = range.start + eq - u32::try_from(ws).ok()?;
    Some((at, range.end - at, String::new()))
}

/// A width-0 variable-name hit under a final/const declaration with a
/// nullable written type: the rewrite deletes the type's trailing `?`
/// (the official `RemoveQuestionMark` deletes the question token of the
/// written type, never a type argument's).
fn drop_type_question_mark(text: &str, offset16: u32) -> Option<(u32, u32, String)> {
    let start = utf16_offset_to_byte(text, offset16);
    let head = text.get(..start)?;
    let ws = head.len() - head.trim_end_matches([' ', '\t']).len();
    if !head[..head.len() - ws].ends_with('?') {
        return None;
    }
    Some((offset16 - u32::try_from(ws).ok()? - 1, 1, String::new()))
}

/// A width-0 `new` hit: the rewrite drops the `.new` (the official
/// `RemoveConstructorName` deletes from the period through the name).
fn drop_dot_new(text: &str, offset16: u32) -> Option<(u32, u32, String)> {
    let start = utf16_offset_to_byte(text, offset16);
    let rest = text.get(start..)?.strip_prefix("new")?;
    if rest
        .chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || c == '_')
    {
        return None;
    }
    let head = text.get(..start)?;
    let ws = head.len() - head.trim_end_matches([' ', '\t']).len();
    if !head[..head.len() - ws].ends_with('.') {
        return None;
    }
    let at = u32::try_from(start - ws - 1).ok()?;
    Some((at, u32::try_from(ws + 4).ok()?, String::new()))
}

// ---------------------------------------------------------------------------
// Token-verified rewrites
//
// The structural rewrites rebuild their replacement from the token
// stream: keyword and operator positions are quote-safe only there (a
// naive text scan would rewrite `'is'` or split on the `+` inside a
// string). Every rewriter re-verifies the shape its rule reports before
// composing — an unverifiable hit drops, never patches on faith.
// ---------------------------------------------------------------------------

/// Runs a token-verified rewriter; a missing token table (no parse was
/// needed by any hit) drops the fix.
fn token_rewrite(
    tokens: Option<&TokenTable>,
    text: &str,
    range: &core::ops::Range<u32>,
    rewrite: fn(&TokenTable, &str, &core::ops::Range<u32>) -> Option<(u32, u32, String)>,
) -> Option<(u32, u32, String)> {
    rewrite(tokens?, text, range)
}

/// The `[first, last]` token-index window of a hit range. Only real
/// tokens count — the scanner-recovery tail (the phantom bracket pair
/// behind the `[]` index token, `Eof`) is zero-width and unsorted, so a
/// binary search over the stream cannot close the window.
fn hit_tokens(tokens: &TokenTable, range: &core::ops::Range<u32>) -> Option<(usize, usize)> {
    let mut first = None;
    let mut last = None;
    for index in 0..tokens.as_slice().len() {
        let token = tokens.get(index)?;
        if token.byte_len == 0 || token.utf16_offset < range.start {
            continue;
        }
        if token.utf16_offset >= range.end {
            break;
        }
        if first.is_none() {
            first = Some(index);
        }
        last = Some(index);
    }
    Some((first?, last?))
}

/// The UTF-16 `[offset, length)` covering tokens `first..=last`.
fn span_over(text: &str, tokens: &TokenTable, first: usize, last: usize) -> Option<(u32, u32)> {
    let start = tokens.get(first)?.utf16_offset;
    let end = tokens.get(last)?.utf16_offset
        + u32::try_from(tokens.lexeme(text, last).encode_utf16().count()).ok()?;
    Some((start, end.checked_sub(start)?))
}

fn kind_of(tokens: &TokenTable, index: usize) -> Option<TokenKind> {
    tokens.get(index).map(|token| token.kind)
}

/// The token's byte-end position.
fn token_end(token: &dartq_scanner::Token) -> usize {
    token.byte_offset as usize + token.byte_len as usize
}

/// A word-shaped token with the exact lexeme (plain identifiers and
/// pseudo keywords both qualify — the scanner's split between
/// `Identifier` and contextual `Keyword` is contextual).
fn word_at(tokens: &TokenTable, text: &str, index: usize, word: &str) -> bool {
    matches!(
        kind_of(tokens, index),
        Some(TokenKind::Identifier) | Some(TokenKind::Keyword(_))
    ) && tokens.lexeme(text, index) == word
}

fn is_null_token(tokens: &TokenTable, index: usize) -> bool {
    kind_of(tokens, index) == Some(TokenKind::Keyword(Keyword::Null))
}

/// The first depth-0 token in `from..to` matching `pred`, over the `()`,
/// `[]`, `{}`, and `${}` nesting (`<` is not a group here — comparisons
/// and type arguments must not disturb the balance).
fn depth0_find(
    tokens: &TokenTable,
    text: &str,
    from: usize,
    to: usize,
    pred: impl Fn(TokenKind, &str) -> bool,
) -> Option<usize> {
    let mut depth = 0u32;
    for index in from..to {
        let token = tokens.get(index)?;
        match token.kind {
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.checked_sub(1)?;
            }
            _ => {}
        }
        if depth == 0 && pred(token.kind, tokens.lexeme(text, index)) {
            return Some(index);
        }
    }
    None
}

/// Whether the token before `!(…)` lets the parentheses go: the official
/// face keeps them exactly when the `!`'s parent binds at relational
/// strength or tighter (prefix operators, selectors, the relational and
/// bitwise families) — everything delimiter- or low-precedence-flanked
/// drops them.
fn paren_removable(kind: TokenKind) -> bool {
    match kind {
        TokenKind::Semicolon
        | TokenKind::OpenCurly
        | TokenKind::CloseCurly
        | TokenKind::OpenParen
        | TokenKind::OpenSquare
        | TokenKind::Comma
        | TokenKind::Colon
        | TokenKind::Question
        | TokenKind::Eq
        | TokenKind::PlusEq
        | TokenKind::MinusEq
        | TokenKind::StarEq
        | TokenKind::SlashEq
        | TokenKind::PercentEq
        | TokenKind::TildeSlashEq
        | TokenKind::LtLtEq
        | TokenKind::GtGtEq
        | TokenKind::GtGtGtEq
        | TokenKind::AmpersandEq
        | TokenKind::BarEq
        | TokenKind::CaretEq
        | TokenKind::QuestionQuestionEq
        | TokenKind::Function
        | TokenKind::AmpersandAmpersand
        | TokenKind::BarBar
        | TokenKind::EqEq
        | TokenKind::BangEq
        | TokenKind::QuestionQuestion => true,
        TokenKind::Keyword(kw) => matches!(
            kw,
            Keyword::Return
                | Keyword::Throw
                | Keyword::Yield
                | Keyword::Case
                | Keyword::In
                | Keyword::Else
                | Keyword::Do
        ),
        _ => false,
    }
}

/// `!(x is T)` → `x is! T` (or `!(x is! T)` where the parentheses must
/// stay): the `!` goes and an `!` lands after the `is` — the official
/// `ConvertIntoIsNot` deletions and insertion folded into one
/// whole-span replacement.
fn is_not_operator(
    tokens: &TokenTable,
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    let (first, last) = hit_tokens(tokens, range)?;
    if kind_of(tokens, first)? != TokenKind::Bang {
        return None;
    }
    let lparen = first + 1;
    if kind_of(tokens, lparen)? != TokenKind::OpenParen || tokens.matching_end(lparen)? != last {
        return None;
    }
    let is = depth0_find(tokens, text, lparen + 1, last, |kind, _| {
        kind == TokenKind::Keyword(Keyword::Is)
    })?;
    let is_end = token_end(tokens.get(is)?);
    let keep_parens = !first
        .checked_sub(1)
        .and_then(|index| tokens.get(index))
        .is_some_and(|prev| paren_removable(prev.kind));
    let (base, body_end) = if keep_parens {
        (token_end(tokens.get(first)?), token_end(tokens.get(last)?))
    } else {
        (
            token_end(tokens.get(lparen)?),
            tokens.get(last)?.byte_offset as usize,
        )
    };
    let body = &text[base..body_end];
    let at = is_end - base;
    let (start16, length16) = span_over(text, tokens, first, last)?;
    Some((
        start16,
        length16,
        format!("{}!{}", &body[..at], &body[at..]),
    ))
}

/// The `(condition-op, non-null operand token range, then, else)` of a
/// null-check conditional — the shared read of the two null-aware
/// rewrites. The separators are the depth-0 `?` and `:` of the span.
fn null_check_tokens<'s>(
    tokens: &TokenTable,
    text: &'s str,
    first: usize,
    last: usize,
) -> Option<(
    TokenKind,
    core::ops::Range<usize>,
    core::ops::Range<usize>,
    core::ops::Range<usize>,
)> {
    let question = depth0_find(tokens, text, first, last + 1, |kind, _| {
        kind == TokenKind::Question
    })?;
    // The conditional's own `:`: each depth-0 `?` after the first opens a
    // nested conditional (a bare conditional then-arm is legal), whose
    // `:` closes it before the outer's can appear.
    let mut depth = 0u32;
    let mut pending = 0u32;
    let mut colon = None;
    for index in question + 1..=last {
        match kind_of(tokens, index)? {
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.checked_sub(1)?;
            }
            TokenKind::Question if depth == 0 => pending += 1,
            TokenKind::Colon if depth == 0 => {
                if pending == 0 {
                    colon = Some(index);
                    break;
                }
                pending -= 1;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let op = depth0_find(tokens, text, first, question, |kind, _| {
        matches!(kind, TokenKind::EqEq | TokenKind::BangEq)
    })?;
    let operand = if is_null_token(tokens, op + 1) {
        first..op
    } else if op > first && is_null_token(tokens, op - 1) {
        op + 1..question
    } else {
        return None;
    };
    Some((
        kind_of(tokens, op)?,
        operand,
        question + 1..colon,
        colon + 1..last + 1,
    ))
}

/// The trimmed source text of a token range (an empty range is the empty
/// string).
fn token_text(tokens: &TokenTable, text: &str, range: core::ops::Range<usize>) -> Option<String> {
    if range.is_empty() {
        return Some(String::new());
    }
    let (start, length) = span_over(text, tokens, range.start, range.end - 1)?;
    text.get(utf16_offset_to_byte(text, start)..utf16_offset_to_byte(text, start + length))
        .map(str::trim)
        .map(str::to_owned)
}

/// Whether the token ranges hold the same lexeme sequence.
fn same_tokens(
    tokens: &TokenTable,
    text: &str,
    a: core::ops::Range<usize>,
    b: core::ops::Range<usize>,
) -> bool {
    a.len() == b.len()
        && !a.is_empty()
        && a.enumerate()
            .all(|(i, index)| tokens.lexeme(text, index) == tokens.lexeme(text, b.start + i))
}

/// `x == null ? a : x` → `x ?? a` (the official `ConvertToIfNull`
/// replacement text: nullable operand, `??`, default arm — parenthesized
/// when the arm is itself a bare conditional).
fn if_null_operators(
    tokens: &TokenTable,
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    let (first, last) = hit_tokens(tokens, range)?;
    let (op, operand, then_arm, else_arm) = null_check_tokens(tokens, text, first, last)?;
    let (nullable, default) = match op {
        TokenKind::EqEq => (else_arm, then_arm),
        _ => (then_arm, else_arm),
    };
    if !same_tokens(tokens, text, operand, nullable.clone()) {
        return None;
    }
    let default_text = token_text(tokens, text, default.clone())?;
    // A default whose precedence sits below `??` (a bare conditional —
    // the only such shape) needs its parentheses.
    let wrap = depth0_find(tokens, text, default.start, default.end, |kind, _| {
        kind == TokenKind::Question
    })
    .is_some();
    let (start16, length16) = span_over(text, tokens, first, last)?;
    Some((
        start16,
        length16,
        format!(
            "{} ?? {}{}{}",
            token_text(tokens, text, nullable)?,
            if wrap { "(" } else { "" },
            default_text,
            if wrap { ")" } else { "" },
        ),
    ))
}

/// `x == null ? null : x.y` → `x?.y`: the arm's access chain peels back
/// to the null-check operand (the rule's own test, re-verified), the
/// peeled access goes null-aware — `.` becomes `?.`, a trailing `!`
/// becomes `?` (or vanishes when bare), an existing `?.` stays.
fn null_aware_operators(
    tokens: &TokenTable,
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    let (first, last) = hit_tokens(tokens, range)?;
    let (op, operand, then_arm, else_arm) = null_check_tokens(tokens, text, first, last)?;
    let result = match op {
        TokenKind::EqEq if is_null_token(tokens, then_arm.start) => else_arm,
        TokenKind::BangEq if is_null_token(tokens, else_arm.start) => then_arm,
        _ => return None,
    };
    let n = operand.len();
    if result.len() <= n
        || !same_tokens(
            tokens,
            text,
            operand.clone(),
            result.start..result.start + n,
        )
    {
        return None;
    }
    let receiver = token_text(tokens, text, operand)?;
    // The access right after the peeled receiver decides the rewrite.
    let after = result.start + n;
    let access = tokens.get(after)?;
    let arm_end = token_end(tokens.get(result.end - 1)?);
    let tail = &text[token_end(access)..arm_end];
    let rewritten = match access.kind {
        // The access token itself is excluded from `tail`: `.` and `?.`
        // both grow to `?.`, a trailing `!` becomes `?` (or vanishes when
        // nothing follows it).
        TokenKind::Period | TokenKind::QuestionPeriod => format!("{receiver}?.{tail}"),
        TokenKind::Bang => {
            if after + 1 == result.end {
                receiver
            } else {
                format!("{receiver}?{tail}")
            }
        }
        _ => return None,
    };
    let (start16, length16) = span_over(text, tokens, first, last)?;
    Some((start16, length16, rewritten))
}

/// The value of the int literal at `index` (optionally negated by the
/// preceding `-`): decimal/hex with digit separators, the lint's own
/// `getIntValue` face.
fn int_literal_value(tokens: &TokenTable, text: &str, index: usize) -> Option<i64> {
    let (negative, at) = if kind_of(tokens, index) == Some(TokenKind::Minus) {
        (true, index + 1)
    } else {
        (false, index)
    };
    if kind_of(tokens, at) != Some(TokenKind::Int) {
        return None;
    }
    let lexeme = tokens.lexeme(text, at);
    let hex = lexeme.starts_with("0x") || lexeme.starts_with("0X");
    let digits: String = lexeme
        .trim_start_matches("0x")
        .trim_start_matches("0X")
        .chars()
        .filter(|&ch| ch != '_')
        .collect();
    i64::from_str_radix(&digits, if hex { 16 } else { 10 })
        .ok()
        .map(|value| if negative { -value } else { value })
}

/// The `indexOf` comparison rewrite (`x.indexOf(y) == -1` →
/// `!x.contains(y)`, the official `ConvertToContains`): the rewrite
/// matrix alone produces an edit — the always-true/false judgments get
/// none, exactly as the official producer returns without one.
fn contains_call(
    tokens: &TokenTable,
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    let (first, last) = hit_tokens(tokens, range)?;
    // The method-name `indexOf` is the last word followed by `(` (a
    // receiver's own `indexOf` call sits before it).
    let call = (first..last)
        .filter(|&index| word_at(tokens, text, index, "indexOf"))
        .filter(|&index| kind_of(tokens, index + 1) == Some(TokenKind::OpenParen))
        .next_back()?;
    let lparen = call + 1;
    let rparen = tokens.matching_end(lparen)?;
    // The start-index argument must be the literal 0 (the lint's own
    // gate) and is dropped from the rewrite.
    let comma = depth0_find(tokens, text, lparen + 1, rparen, |kind, _| {
        kind == TokenKind::Comma
    });
    let argument = match comma {
        Some(comma) => {
            let second = comma + 1..rparen;
            if second.len() != 1 || tokens.lexeme(text, second.start) != "0" {
                return None;
            }
            lparen + 1..comma
        }
        None => lparen + 1..rparen,
    };
    // The int literal sits on one side of the comparison operator; the
    // left-literal layout reads the operator with its operands swapped to
    // reach the matrix's `indexOf OP literal` layout.
    let left_literal = kind_of(tokens, first) == Some(TokenKind::Minus)
        || kind_of(tokens, first) == Some(TokenKind::Int);
    let is_comparison = |kind: Option<TokenKind>| {
        matches!(
            kind,
            Some(TokenKind::EqEq)
                | Some(TokenKind::BangEq)
                | Some(TokenKind::Lt)
                | Some(TokenKind::LtEq)
                | Some(TokenKind::Gt)
                | Some(TokenKind::GtEq)
        )
    };
    let (value, effective, receiver_start) = if left_literal {
        // `-1 != x.indexOf(y)`: the operator sits after the literal, the
        // hit ends at the invocation.
        let op_index = if kind_of(tokens, first) == Some(TokenKind::Minus) {
            first + 2
        } else {
            first + 1
        };
        if !is_comparison(kind_of(tokens, op_index)) || rparen != last {
            return None;
        }
        let after_op = utf16_offset_to_byte(text, tokens.get(op_index + 1)?.utf16_offset);
        (
            int_literal_value(tokens, text, first)?,
            mirror_op(tokens.lexeme(text, op_index)).to_owned(),
            after_op,
        )
    } else {
        let op_index = rparen + 1;
        if op_index >= last || !is_comparison(kind_of(tokens, op_index)) {
            return None;
        }
        (
            int_literal_value(tokens, text, op_index + 1)?,
            tokens.lexeme(text, op_index).to_owned(),
            tokens.get(first)?.byte_offset as usize,
        )
    };
    let negated = match (value, effective.as_str()) {
        (-1, "==" | "<=") => true,
        (-1, "!=" | ">") => false,
        (0, ">=") => false,
        (0, "<") => true,
        _ => return None,
    };
    let argument_text = token_text(tokens, text, argument)?;
    let receiver = &text[receiver_start..tokens.get(call)?.byte_offset as usize];
    let (start16, length16) = span_over(text, tokens, first, last)?;
    Some((
        start16,
        length16,
        format!(
            "{}{receiver}contains({argument_text})",
            if negated { "!" } else { "" }
        ),
    ))
}

/// The comparison operator read with its operands swapped (`a < b` ≡
/// `b > a`): the left-literal face canonicalizes to the matrix's layout.
fn mirror_op(op: &str) -> &str {
    match op {
        "<" => ">=",
        "<=" => ">",
        ">" => "<=",
        ">=" => "<",
        other => other,
    }
}

/// One merged interpolation component: a string literal's contents, a
/// bare identifier, or an arbitrary expression's source.
struct Component {
    text: String,
    identifier: bool,
}

fn compose_interpolation(
    tokens: &TokenTable,
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    let (first, last) = hit_tokens(tokens, range)?;
    // The chain's depth-0 `+` splits the operands.
    let mut operands: Vec<core::ops::Range<usize>> = Vec::new();
    let mut depth = 0u32;
    let mut start = first;
    for index in first..=last {
        match kind_of(tokens, index)? {
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.checked_sub(1)?;
            }
            TokenKind::Plus if depth == 0 => {
                operands.push(start..index);
                start = index + 1;
            }
            _ => {}
        }
    }
    operands.push(start..last + 1);
    if operands.len() < 2 {
        return None;
    }
    let mut components = Vec::new();
    let mut style: Option<(u8, bool)> = None;
    for operand in operands {
        let (component, literal_style) = flatten_operand(tokens, text, operand.clone())?;
        match (style, literal_style) {
            (None, _) => style = literal_style,
            (Some(existing), Some(new)) if existing != new => return None,
            _ => {}
        }
        components.push(component);
    }
    let (quote, triple) = style?;
    let open = char::from(quote)
        .to_string()
        .repeat(if triple { 3 } else { 1 });
    // A bare identifier absorbs a following identifier-part head, so it
    // takes the braced form when the next component would continue it.
    let mut out = open.clone();
    for (index, component) in components.iter().enumerate() {
        let next_continues = components
            .get(index + 1)
            .and_then(|next| next.text.chars().next())
            .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$');
        if component.identifier {
            if next_continues {
                out.push_str("${");
                out.push_str(&component.text);
                out.push('}');
            } else {
                out.push('$');
                out.push_str(&component.text);
            }
        } else {
            out.push_str(&component.text);
        }
    }
    out.push_str(&open);
    let (start16, length16) = span_over(text, tokens, first, last)?;
    Some((start16, length16, out))
}

/// One operand of the `+` chain → its component (with the string style
/// when the operand is a literal). Adjacent strings flatten piece-wise,
/// `x.toString()` unwraps to `x`, parentheses unwrap — the official
/// `_extractComponentsInto` walk. Raw strings drop the whole fix.
fn flatten_operand(
    tokens: &TokenTable,
    text: &str,
    operand: core::ops::Range<usize>,
) -> Option<(Component, Option<(u8, bool)>)> {
    let first = operand.start;
    let last = operand.end - 1;
    match kind_of(tokens, first)? {
        TokenKind::String => {
            let lexeme = tokens.lexeme(text, first);
            if lexeme.starts_with('r') {
                return None;
            }
            let quote = *lexeme.as_bytes().first()?;
            // Adjacent strings: consecutive bare String tokens, each its
            // own literal piece — all sharing one quote style.
            let mut split = first;
            while split <= last && kind_of(tokens, split) == Some(TokenKind::String) {
                split += 1;
            }
            if split > last {
                let mut contents = String::new();
                for index in first..=last {
                    if string_style(tokens, text, index)?
                        != (quote, quote_is_triple(tokens, text, first)?)
                    {
                        return None;
                    }
                    contents.push_str(&segment_contents(tokens, text, index)?);
                }
                return Some((
                    Component {
                        text: contents,
                        identifier: false,
                    },
                    Some((quote, quote_is_triple(tokens, text, first)?)),
                ));
            }
            // An interpolated literal: `${…}` groups and segments to the
            // end, contents verbatim between the outer quote runs.
            let mut cursor = split;
            while cursor <= last {
                match kind_of(tokens, cursor)? {
                    TokenKind::String => cursor += 1,
                    TokenKind::StringInterpolationExpression => {
                        cursor = tokens.matching_end(cursor)? + 1;
                    }
                    _ => return None,
                }
            }
            let head = tokens.get(first)?.byte_offset as usize
                + quote_run(lexeme.as_bytes(), quote, false);
            let tail_lexeme = tokens.lexeme(text, last);
            let tail = tokens.get(last)?.byte_offset as usize + tail_lexeme.len()
                - quote_run(
                    tail_lexeme.as_bytes(),
                    *tail_lexeme.as_bytes().last()?,
                    true,
                );
            Some((
                Component {
                    text: text[head..tail].to_owned(),
                    identifier: false,
                },
                Some((quote, quote_run(lexeme.as_bytes(), quote, false) > 1)),
            ))
        }
        TokenKind::Identifier | TokenKind::Keyword(_) if first == last => Some((
            Component {
                text: tokens.lexeme(text, first).to_owned(),
                identifier: true,
            },
            None,
        )),
        _ => {
            // `x.toString()` unwraps to `x`; parentheses unwrap.
            if last >= first + 4
                && kind_of(tokens, last - 3) == Some(TokenKind::Period)
                && word_at(tokens, text, last - 2, "toString")
                && kind_of(tokens, last - 1) == Some(TokenKind::OpenParen)
                && kind_of(tokens, last) == Some(TokenKind::CloseParen)
            {
                return flatten_operand(tokens, text, first..last - 3);
            }
            if kind_of(tokens, first) == Some(TokenKind::OpenParen)
                && tokens.matching_end(first)? == last
            {
                return flatten_operand(tokens, text, first + 1..last);
            }
            let (start16, length16) = span_over(text, tokens, first, last)?;
            let slice = text.get(
                utf16_offset_to_byte(text, start16)..utf16_offset_to_byte(text, start16 + length16),
            )?;
            Some((
                Component {
                    text: slice.to_owned(),
                    identifier: false,
                },
                None,
            ))
        }
    }
}

/// The `(quote byte, is-triple)` style of a bare string segment.
fn string_style(tokens: &TokenTable, text: &str, index: usize) -> Option<(u8, bool)> {
    let _ = text;
    let lexeme = tokens.lexeme(text, index);
    let quote = if lexeme.starts_with('r') {
        *lexeme.as_bytes().get(1)?
    } else {
        *lexeme.as_bytes().first()?
    };
    Some((quote, quote_run(lexeme.as_bytes(), quote, false) > 1))
}

/// The triple-quote flag of the operand's head segment.
fn quote_is_triple(tokens: &TokenTable, text: &str, index: usize) -> Option<bool> {
    string_style(tokens, text, index).map(|(_, triple)| triple)
}

/// The contents of one bare string segment: the lexeme minus its outer
/// quote runs (escapes stay as written).
fn segment_contents(tokens: &TokenTable, text: &str, index: usize) -> Option<String> {
    let lexeme = tokens.lexeme(text, index);
    let quote = if lexeme.starts_with('r') {
        *lexeme.as_bytes().get(1)?
    } else {
        *lexeme.as_bytes().first()?
    };
    let lead = quote_run(lexeme.as_bytes(), quote, false);
    let trail = quote_run(lexeme.as_bytes(), quote, true);
    let start = tokens.get(index)?.byte_offset as usize;
    Some(text[start + lead..start + lexeme.len() - trail].to_owned())
}

/// `Map()` → `{}` (written type arguments ride along), `Set()` →
/// `<dynamic>{}` unless the context declares `Set`, `Set.from([…])` /
/// `Set.of([…])` and `[…].toSet()` → the set literal — the official
/// `ConvertToMapLiteral` / `ConvertToSetLiteral` replacements.
fn collection_literal(
    tokens: &TokenTable,
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    let (first, last) = hit_tokens(tokens, range)?;
    // The `.toSet()` face: an optional type argument group, the list
    // literal, then `.toSet()`.
    let mut cursor = first;
    let mut head_args = String::new();
    if kind_of(tokens, cursor) == Some(TokenKind::Lt) {
        let close = tokens.matching_end(cursor)?;
        head_args = token_text(tokens, text, cursor..close + 1)?;
        cursor = close + 1;
    }
    // The `.toSet()` face: the target list literal — a `[…]` group, or
    // the scanner's single `[]` token for the empty literal — then
    // `.toSet()`.
    if matches!(
        kind_of(tokens, cursor),
        Some(TokenKind::OpenSquare) | Some(TokenKind::Index)
    ) {
        let (close, contents) = if kind_of(tokens, cursor) == Some(TokenKind::Index) {
            (cursor, String::new())
        } else {
            let close = tokens.matching_end(cursor)?;
            (close, token_text(tokens, text, cursor + 1..close)?)
        };
        if close + 4 == last
            && kind_of(tokens, close + 1) == Some(TokenKind::Period)
            && word_at(tokens, text, close + 2, "toSet")
            && kind_of(tokens, close + 3) == Some(TokenKind::OpenParen)
            && kind_of(tokens, last) == Some(TokenKind::CloseParen)
        {
            let unambiguous = !contents.is_empty() && !contents.starts_with("...");
            let prefix = if head_args.is_empty() && !unambiguous {
                "<dynamic>"
            } else {
                ""
            };
            let (start16, length16) = span_over(text, tokens, first, last)?;
            return Some((
                start16,
                length16,
                format!("{head_args}{prefix}{{{contents}}}"),
            ));
        }
        return None;
    }
    // The creation faces: the callee identifier (after an optional
    // `new`), its type arguments, then the constructor shape.
    let mut cursor = cursor.max(first);
    if kind_of(tokens, cursor) == Some(TokenKind::Keyword(Keyword::New)) {
        cursor += 1;
    }
    let name = tokens.lexeme(text, cursor).to_owned();
    if !matches!(
        kind_of(tokens, cursor),
        Some(TokenKind::Identifier) | Some(TokenKind::Keyword(_))
    ) || !matches!(
        name.as_str(),
        "Map" | "LinkedHashMap" | "Set" | "LinkedHashSet"
    ) {
        return None;
    }
    cursor += 1;
    // The written type arguments ride into the replacement, then the
    // constructor shape: `.from`/`.of` (the named face) or `(`.
    let mut ctor_args = String::new();
    if kind_of(tokens, cursor) == Some(TokenKind::Lt) {
        let close = tokens.matching_end(cursor)?;
        ctor_args = token_text(tokens, text, cursor..close + 1)?;
        cursor = close + 1;
    }
    let named = kind_of(tokens, cursor) == Some(TokenKind::Period)
        && (word_at(tokens, text, cursor + 1, "from") || word_at(tokens, text, cursor + 1, "of"));
    if named {
        cursor += 2;
    }
    if kind_of(tokens, cursor) != Some(TokenKind::OpenParen) {
        return None;
    }
    let close = tokens.matching_end(cursor)?;
    if !named {
        // The unnamed face: empty parentheses close the creation.
        if close != last || close != cursor + 1 {
            return None;
        }
        let prefix = match name.as_str() {
            "Set" | "LinkedHashSet"
                if !declares_set_context(tokens, text, first).unwrap_or(false) =>
            {
                "<dynamic>"
            }
            _ => "",
        };
        let (start16, length16) = span_over(text, tokens, first, last)?;
        return Some((start16, length16, format!("{ctor_args}{prefix}{{}}")));
    }
    // The single argument is a list literal — a `[…]` group or the
    // scanner's single `[]` token — and the argument list closes right
    // after it.
    let mut arg = cursor + 1;
    let mut list_args = String::new();
    if kind_of(tokens, arg) == Some(TokenKind::Lt) {
        let close = tokens.matching_end(arg)?;
        list_args = token_text(tokens, text, arg..close + 1)?;
        arg = close + 1;
    }
    let (close, elements) = match kind_of(tokens, arg) {
        Some(TokenKind::OpenSquare) => {
            let close = tokens.matching_end(arg)?;
            (close, token_text(tokens, text, arg + 1..close)?)
        }
        Some(TokenKind::Index) => (arg, String::new()),
        _ => return None,
    };
    if close + 1 != last || tokens.matching_end(cursor)? != last {
        return None;
    }
    let unambiguous = !elements.is_empty() && !elements.starts_with("...");
    let inferred = !ctor_args.is_empty()
        || !list_args.is_empty()
        || unambiguous
        || declares_set_context(tokens, text, first).unwrap_or(false);
    let prefix = if inferred { "" } else { "<dynamic>" };
    let (start16, length16) = span_over(text, tokens, first, last)?;
    Some((
        start16,
        length16,
        format!("{ctor_args}{prefix}{{{elements}}}"),
    ))
}

/// Whether the creation sits in a declaration whose written type is
/// `Set` (`Set s = Set()`): the official inference face, read backwards
/// over the declaration shape `= name [type]`.
fn declares_set_context(tokens: &TokenTable, text: &str, first: usize) -> Option<bool> {
    let Some(at) = first
        .checked_sub(1)
        .filter(|&i| kind_of(tokens, i) == Some(TokenKind::Eq))
    else {
        return Some(false);
    };
    let Some(name) = at
        .checked_sub(1)
        .filter(|&i| kind_of(tokens, i) == Some(TokenKind::Identifier))
    else {
        return Some(false);
    };
    let mut cursor = name - 1;
    // Skip a type-argument group, then expect the type identifier.
    if matches!(
        kind_of(tokens, cursor),
        Some(TokenKind::Gt) | Some(TokenKind::GtGt) | Some(TokenKind::GtGtGt)
    ) {
        let mut depth = 0u32;
        loop {
            match kind_of(tokens, cursor)? {
                TokenKind::Gt => depth += 1,
                TokenKind::GtGt => depth += 2,
                TokenKind::GtGtGt => depth += 3,
                TokenKind::Lt => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
            cursor = cursor.checked_sub(1)?;
        }
        cursor = cursor.checked_sub(1)?;
    }
    Some(
        kind_of(tokens, cursor) == Some(TokenKind::Identifier)
            && tokens.lexeme(text, cursor) == "Set",
    )
}

/// `['a']..add(x)` → `['a', x]` (and the `addAll` literal form): the
/// cascade sections after the target compose into the literal — one
/// whole-tail replacement per cascade (the official insert-and-delete
/// pair folded; a section that is not `add`/`addAll` ends the tail and
/// stays put).
fn inlined_adds(
    tokens: &TokenTable,
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    let (name_index, _) = hit_tokens(tokens, range)?;
    let name = tokens.lexeme(text, name_index);
    if !matches!(name, "add" | "addAll")
        || kind_of(tokens, name_index + 1) != Some(TokenKind::OpenParen)
    {
        return None;
    }
    let lparen = name_index + 1;
    let rparen = tokens.matching_end(lparen)?;
    if depth0_find(tokens, text, lparen + 1, rparen, |kind, _| {
        kind == TokenKind::Comma
    })
    .is_some()
    {
        return None; // the official face takes exactly one argument
    }
    // Backwards: the cascade marker, then the target literal's `]`.
    match kind_of(tokens, name_index.checked_sub(1)?) {
        Some(TokenKind::PeriodPeriod) | Some(TokenKind::QuestionPeriodPeriod) => {}
        _ => return None,
    }
    let close_square = name_index - 2;
    // The target literal's `]`: a `[…]` group — or the scanner's single
    // `[]` token for the empty literal, its own opener and closer.
    let open_square = match kind_of(tokens, close_square) {
        Some(TokenKind::Index) => close_square,
        Some(TokenKind::CloseSquare) => {
            let mut depth = 0u32;
            let mut cursor = close_square;
            loop {
                cursor = cursor.checked_sub(1)?;
                match kind_of(tokens, cursor)? {
                    TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                        depth += 1
                    }
                    TokenKind::OpenParen | TokenKind::OpenCurly => depth = depth.checked_sub(1)?,
                    TokenKind::StringInterpolationExpression => depth = depth.checked_sub(1)?,
                    TokenKind::OpenSquare => {
                        if depth == 0 {
                            break cursor;
                        }
                        depth = depth.checked_sub(1)?;
                    }
                    _ => {}
                }
            }
        }
        _ => return None,
    };
    // The hit's own section opens the composition; the sections after it
    // follow while they are `add`/`addAll`.
    let target_nonempty = close_square > open_square + 1;
    let mut additions = vec![section_addition(tokens, text, name, lparen, rparen)?];
    let mut cursor = rparen;
    loop {
        let next = cursor + 1;
        if !matches!(
            kind_of(tokens, next),
            Some(TokenKind::PeriodPeriod) | Some(TokenKind::QuestionPeriodPeriod)
        ) {
            break;
        }
        let section = next + 1;
        let section_name = tokens.lexeme(text, section);
        if !matches!(section_name, "add" | "addAll")
            || kind_of(tokens, section + 1) != Some(TokenKind::OpenParen)
        {
            break;
        }
        let section_rparen = tokens.matching_end(section + 1)?;
        match section_addition(tokens, text, section_name, section + 1, section_rparen) {
            Some(addition) => {
                additions.push(addition);
                cursor = section_rparen;
            }
            None => break,
        }
    }
    let (start16, length16) = span_over(text, tokens, close_square, cursor)?;
    let joined = additions.join(", ");
    // The empty literal is one `[]` token: the whole literal is rewritten.
    let replacement = if close_square == open_square {
        format!("[{joined}]")
    } else {
        format!("{}{}]", if target_nonempty { ", " } else { "" }, joined)
    };
    Some((start16, length16, replacement))
}

/// The composed element source of one `add`/`addAll` section: the single
/// argument, or the list literal's elements for `addAll` (`None` when the
/// section is not the official face — multiple arguments, a non-literal
/// `addAll` argument, an empty one).
fn section_addition(
    tokens: &TokenTable,
    text: &str,
    name: &str,
    lparen: usize,
    rparen: usize,
) -> Option<String> {
    if depth0_find(tokens, text, lparen + 1, rparen, |kind, _| {
        kind == TokenKind::Comma
    })
    .is_some()
    {
        return None; // the official face takes exactly one argument
    }
    if name == "addAll" {
        if kind_of(tokens, lparen + 1) != Some(TokenKind::OpenSquare) {
            return None; // the spread rule's face, not ours
        }
        let sq = tokens.matching_end(lparen + 1)?;
        if sq + 1 != rparen {
            return None;
        }
        let elements = token_text(tokens, text, lparen + 2..sq)?;
        if elements.is_empty() {
            return None; // the official face returns without a fix here
        }
        Some(elements)
    } else {
        let argument = token_text(tokens, text, lparen + 1..rparen)?;
        if argument.is_empty() {
            return None;
        }
        Some(argument)
    }
}

/// `var f = () {};` → `void f() {}` (the official
/// `ConvertToFunctionDeclaration`, its resolution-free face): the
/// declaration prefix goes, the name keeps, the closure's parameters and
/// body follow the inferred-`void` return type (`Future<void>` under
/// `async`). Value-returning closures and expression bodies drop — their
/// return type is not derivable here.
fn function_declaration(
    tokens: &TokenTable,
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    let (first, last) = hit_tokens(tokens, range)?;
    if kind_of(tokens, first) != Some(TokenKind::Identifier)
        || kind_of(tokens, first + 1) != Some(TokenKind::Eq)
        || kind_of(tokens, last + 1) != Some(TokenKind::Semicolon)
    {
        return None;
    }
    let name = tokens.lexeme(text, first);
    // The declaration prefix, walked backwards: `late`/`final`/`const`/
    // `var`, then the optional written type (dotted, nullable, with type
    // arguments) — a boundary that is not a statement edge drops the fix
    // (a sibling variable, a bare expression context).
    let mut cursor = first;
    loop {
        let Some(prev) = cursor
            .checked_sub(1)
            .and_then(|index| kind_of(tokens, index))
        else {
            break;
        };
        match prev {
            TokenKind::Keyword(kw)
                if matches!(
                    kw,
                    Keyword::Final | Keyword::Const | Keyword::Var | Keyword::Late
                ) => {}
            TokenKind::Question => {}
            TokenKind::Gt | TokenKind::GtGt | TokenKind::GtGtGt => {
                let mut depth = 0u32;
                loop {
                    cursor -= 1;
                    match kind_of(tokens, cursor)? {
                        TokenKind::Gt => depth += 1,
                        TokenKind::GtGt => depth += 2,
                        TokenKind::GtGtGt => depth += 3,
                        TokenKind::Lt => {
                            if depth == 0 {
                                break;
                            }
                            depth -= 1;
                        }
                        _ => {}
                    }
                }
            }
            TokenKind::Identifier | TokenKind::Keyword(Keyword::Function) => {}
            _ => break,
        }
        cursor -= 1;
    }
    if let Some(boundary) = cursor.checked_sub(1)
        && !matches!(
            kind_of(tokens, boundary),
            Some(TokenKind::Semicolon) | Some(TokenKind::OpenCurly) | Some(TokenKind::CloseCurly)
        )
    {
        return None;
    }
    // The closure: the parameter list, an optional async marker after it,
    // then a block body without a value `return`.
    let scan = first + 2;
    if kind_of(tokens, scan) != Some(TokenKind::OpenParen) {
        return None;
    }
    let params_end = tokens.matching_end(scan)?;
    let mut return_type = "void ";
    let mut marker = params_end + 1;
    match kind_of(tokens, marker) {
        Some(TokenKind::Keyword(Keyword::Async)) => {
            return_type = "Future<void> ";
            marker += 1;
            if kind_of(tokens, marker) == Some(TokenKind::Star) {
                return None;
            }
        }
        Some(TokenKind::Keyword(Keyword::Sync)) => return None,
        _ => {}
    }
    if kind_of(tokens, marker) != Some(TokenKind::OpenCurly) {
        return None;
    }
    for index in first + 2..=last {
        if kind_of(tokens, index) == Some(TokenKind::Keyword(Keyword::Return))
            && kind_of(tokens, index + 1) != Some(TokenKind::Semicolon)
        {
            return None;
        }
    }
    let (start16, length16) = span_over(text, tokens, cursor, last + 1)?;
    let closure = &text
        [tokens.get(first + 2)?.byte_offset as usize..tokens.get(last + 1)?.byte_offset as usize];
    Some((start16, length16, format!("{return_type}{name}{closure}")))
}

// ---------------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------------

/// The anchor object: `line`/`column` plus the context flanks; empty
/// context strings are omitted (§15.7).
fn anchor_json(anchor: &Anchor) -> serde_json::Map<String, serde_json::Value> {
    let mut json = serde_json::Map::new();
    json.insert("line".into(), anchor.line.into());
    json.insert("column".into(), anchor.column.into());
    if !anchor.context_before.is_empty() {
        json.insert(
            "context_before".into(),
            anchor.context_before.clone().into(),
        );
    }
    if !anchor.context_after.is_empty() {
        json.insert("context_after".into(), anchor.context_after.clone().into());
    }
    json
}

/// One edit row at its locator floor: offset/length/replacement plus the
/// provenance tag — the red line the patch rows never drop (§15.8: the
/// minimal tier trims prose and convenience, never location).
fn edit_floor(edit: &Edit, source: &str) -> serde_json::Map<String, serde_json::Value> {
    let mut row = serde_json::Map::new();
    row.insert("offset".into(), edit.offset.into());
    row.insert("length".into(), edit.length.into());
    row.insert("replacement".into(), edit.replacement.clone().into());
    row.insert("source".into(), source.into());
    row
}

/// The §15.6 document for one file — `file`, `schema_version`, and the
/// `edits`: the built-in rows first, then each plugin change's edits
/// flattened in change order, every row carrying its provenance tag
/// (`builtin-lint`, or the plugin's manifest name). Empty context strings
/// are omitted (§15.7); the anchor rides compact and full, the full tier
/// adds the row's human name (`rule` for built-in rows, `message` +
/// `priority` for plugin rows). Files without rows never enter a set.
#[must_use]
pub fn patch_json(set: &FileFixSet, verbosity: Verbosity) -> serde_json::Value {
    let anchored = verbosity != Verbosity::Minimal;
    let mut edits = Vec::new();
    for fix in &set.fixes {
        let mut row = edit_floor(&fix.edit, BUILTIN_SOURCE);
        if anchored {
            row.insert("anchor".into(), anchor_json(&fix.edit.anchor).into());
        }
        if verbosity == Verbosity::Full {
            row.insert("rule".into(), fix.rule.into());
            row.insert("message".into(), fix.message.clone().into());
        }
        edits.push(serde_json::Value::Object(row));
    }
    for fix in &set.plugin {
        for edit in &fix.edits {
            let mut row = edit_floor(edit, &fix.plugin);
            if anchored {
                row.insert("anchor".into(), anchor_json(&edit.anchor).into());
            }
            if verbosity == Verbosity::Full {
                row.insert("message".into(), fix.message.clone().into());
                row.insert("priority".into(), fix.priority.into());
            }
            edits.push(serde_json::Value::Object(row));
        }
    }
    serde_json::json!({
        "file": set.path,
        "schema_version": SCHEMA_VERSION,
        "edits": edits,
    })
}

/// The assists document for one file: the plugin changes in priority
/// order, each carrying its provenance (`source`), title, priority, and
/// anchored edits (the anchor drops at the minimal tier); the hosts that
/// could not serve the request answer in `errors` (omitted when empty,
/// §15.7).
///
/// # Panics
///
/// Panics if a constructed assist row is not a JSON object — impossible
/// by construction (`json!` builds the object literal); the expect pins
/// that invariant.
#[must_use]
pub fn assists_json(answer: &AssistAnswer, verbosity: Verbosity) -> serde_json::Value {
    let anchored = verbosity != Verbosity::Minimal;
    let assists: Vec<serde_json::Value> = answer
        .assists
        .iter()
        .map(|fix| {
            let edits: Vec<serde_json::Value> = fix
                .edits
                .iter()
                .map(|edit| {
                    let mut row = serde_json::Value::Object(edit_floor(edit, &fix.plugin));
                    if anchored {
                        row["anchor"] = anchor_json(&edit.anchor).into();
                    }
                    row
                })
                .collect();
            let mut row = serde_json::json!({
                "message": fix.message,
                "source": fix.plugin,
                "priority": fix.priority,
                "edits": edits,
            });
            if verbosity == Verbosity::Minimal {
                row.as_object_mut()
                    .expect("assists row object")
                    .remove("priority");
            }
            row
        })
        .collect();
    let mut document = serde_json::json!({
        "file": answer.file,
        "schema_version": SCHEMA_VERSION,
        "assists": assists,
    });
    if !answer.errors.is_empty() {
        document["errors"] = serde_json::json!(
            answer
                .errors
                .iter()
                .map(|error| serde_json::json!({
                    "plugin": error.plugin,
                    "message": error.message,
                }))
                .collect::<Vec<_>>()
        );
    }
    document
}

#[cfg(test)]
mod tests {
    //! The rows here sit at dartq-lints' diagnostic output face (`Lint` is
    //! the crate's externally constructible row; its `Unit` feature bits
    //! are not part of the public face, so a run cannot be driven from
    //! outside). Each row carries the real registry rule id and the
    //! official message, and [`super::from_lints`]' slice verification is
    //! the correctness gate: a row that does not match the source produces
    //! no patch at all.

    use dartq_lints::{Lint, rule_by_name};
    use dartq_plugin::SourceEdit;
    use dartq_source::LineIndex;

    use super::*;

    /// The single fix of `hits[0]`, materialized over `source`.
    fn one_fix(source: &str, hits: &[Lint]) -> RuleFix {
        let lines = LineIndex::new(source);
        let fixes = from_lints(hits, source, &lines);
        assert_eq!(fixes.len(), 1, "expected exactly one fix for {source:?}");
        fixes.into_iter().next().expect("one fix")
    }

    /// A width-0 `unnecessary_new` row over the keyword at `offset` — the
    /// span and message the rule reports (`test/rules/unnecessary_new`).
    fn new_row(offset: u32) -> Lint {
        Lint {
            rule: rule_by_name("unnecessary_new").expect("wired registry"),
            range: offset..offset,
            message: "Unnecessary 'new' keyword.".to_owned(),
            correction: None,
        }
    }

    /// A row of `rule` over `range` (messages ride the lint rows, not the
    /// fix face — empty here).
    fn row(rule: &str, range: core::ops::Range<u32>) -> Lint {
        Lint {
            rule: rule_by_name(rule).expect("wired registry"),
            range,
            message: String::new(),
            correction: None,
        }
    }

    /// The golden shape each wired rewrite must hold: the hit materializes
    /// exactly one edit whose application rewrites `source` into
    /// `expected`.
    fn assert_rewrite(rule: &str, source: &str, range: core::ops::Range<u32>, expected: &str) {
        let fix = one_fix(source, &[row(rule, range)]);
        assert_eq!(fix.rule, rule);
        let (patched, outcomes) = apply(source, std::slice::from_ref(&fix.edit));
        assert_eq!(outcomes, &[EditOutcome::Applied], "{rule} on {source:?}");
        assert_eq!(patched, expected, "{rule} on {source:?}");
    }

    /// No fix materializes for `rule` over `range` on `source`.
    fn assert_no_fix(rule: &str, source: &str, range: core::ops::Range<u32>) {
        let lines = LineIndex::new(source);
        let fixes = from_lints(&[row(rule, range)], source, &lines);
        assert!(fixes.is_empty(), "{rule} on {source:?} produced a fix");
    }

    /// The `avoid_empty_else` row over the else `;` at `offset..offset+1`.
    fn empty_else_row(offset: u32) -> Lint {
        Lint {
            rule: rule_by_name("avoid_empty_else").expect("wired registry"),
            range: offset..offset + 1,
            message: "Empty statements are not allowed in an 'else' clause.".to_owned(),
            correction: None,
        }
    }

    /// End to end: the `unnecessary_new` row becomes an anchored edit whose
    /// application rewrites the source; the anchor flanks the keyword.
    #[test]
    fn new_expression_end_to_end() {
        let source = "void f() { var a = new B(); }\n";
        let fix = one_fix(source, &[new_row(19)]);
        assert_eq!(fix.rule, "unnecessary_new");
        assert_eq!(fix.edit.offset, 19);
        assert_eq!(fix.edit.length, 4, "the keyword plus its trailing space");
        assert_eq!(fix.edit.replacement, "");
        assert_eq!(fix.edit.anchor.line, 1);
        assert_eq!(fix.edit.anchor.column, 20);
        assert_eq!(fix.edit.anchor.context_before, "var a = ");
        assert_eq!(fix.edit.anchor.context_after, "B(); }");

        let (patched, outcomes) = apply(source, std::slice::from_ref(&fix.edit));
        assert_eq!(patched, "void f() { var a = B(); }\n");
        assert_eq!(outcomes, &[EditOutcome::Applied]);
    }

    /// Second rule end to end: the else `;` rewrites to `{}`.
    #[test]
    fn avoid_empty_else_end_to_end() {
        let source = "if (b) ; else ;";
        let fix = one_fix(source, &[empty_else_row(14)]);
        assert_eq!(fix.rule, "avoid_empty_else");
        assert_eq!(fix.edit.offset, 14);
        assert_eq!(fix.edit.length, 1);
        assert_eq!(fix.edit.replacement, "{}");

        let (patched, outcomes) = apply(source, std::slice::from_ref(&fix.edit));
        assert_eq!(patched, "if (b) ; else {}");
        assert_eq!(outcomes, &[EditOutcome::Applied]);
    }

    // -----------------------------------------------------------------------
    // T6 breadth: one golden per wired mechanical rewrite
    // -----------------------------------------------------------------------

    /// Keyword drops: the reported keyword plus its trailing horizontal
    /// whitespace goes.
    #[test]
    fn keyword_drops() {
        assert_rewrite(
            "unnecessary_const",
            "var x = [const 1];",
            9..9,
            "var x = [1];",
        );
        assert_rewrite(
            "unnecessary_this",
            "int get y => this.x;",
            13..13,
            "int get y => x;",
        );
        // `this` in a constructor field initializer.
        assert_rewrite(
            "unnecessary_this",
            "A(int x) : this.x = x;",
            11..11,
            "A(int x) : x = x;",
        );
        assert_rewrite(
            "unnecessary_constructor_name",
            "class A { A.new(); }",
            12..12,
            "class A { A(); }",
        );
        assert_rewrite(
            "unnecessary_constructor_name",
            "var a = A.new();",
            10..10,
            "var a = A();",
        );
        // The `this`/`new` lexemes verify before the drop: a drifted row
        // over other text produces no fix.
        assert_no_fix("unnecessary_const", "var x = [late 1];", 9..9);
        assert_no_fix("unnecessary_this", "int get y => super.x;", 13..13);
        assert_no_fix(
            "unnecessary_constructor_name",
            "class A { A.foo(); }",
            13..13,
        );
    }

    /// Syntax drops around strings: the escape's backslash and the
    /// redundant interpolation wrapper go; the kept text stays verbatim.
    #[test]
    fn string_drops() {
        assert_rewrite(
            "unnecessary_string_escapes",
            r"var s = 'a\qb';",
            10..11,
            "var s = 'aqb';",
        );
        assert_rewrite(
            "unnecessary_string_interpolations",
            "var s = ''${'ab'}'';",
            8..19,
            "var s = 'ab';",
        );
        assert_no_fix("unnecessary_string_escapes", "var s = 'a?b';", 10..11);
        // A non-empty wrapper body is not this rule's hit.
        assert_no_fix(
            "unnecessary_string_interpolations",
            "var s = 'x${'ab'}y';",
            8..19,
        );
    }

    /// Null-shape drops: the `??`'s null operand leaves with its operator,
    /// the `??= null` assignment leaves (whole line when the statement is
    /// alone on it), and the written type's `?` goes.
    #[test]
    fn null_drops() {
        assert_rewrite(
            "unnecessary_null_in_if_null_operators",
            "var x = a ?? null;",
            13..17,
            "var x = a;",
        );
        assert_rewrite(
            "unnecessary_null_in_if_null_operators",
            "var x = null ?? a;",
            8..12,
            "var x = a;",
        );
        // The statement line goes whole; the expression context keeps the
        // assignment target.
        assert_rewrite(
            "unnecessary_null_aware_assignments",
            "void f() {\n  x ??= null;\n}\n",
            13..23,
            "void f() {\n}\n",
        );
        assert_rewrite(
            "unnecessary_null_aware_assignments",
            "var y = x ??= null;",
            8..18,
            "var y = x;",
        );
        assert_rewrite(
            "unnecessary_nullable_for_final_variable_declarations",
            "final int? x = 1;",
            11..11,
            "final int x = 1;",
        );
        // A `?` that is not the written type's own (a type argument's)
        // never drops.
        assert_no_fix(
            "unnecessary_nullable_for_final_variable_declarations",
            "final List<int?> x = const [];",
            17..17,
        );
    }

    /// `avoid_init_to_null`: the initializer tail ` = null` goes, keeping
    /// the name (and the parameter's written type).
    #[test]
    fn init_to_null_drop() {
        assert_rewrite("avoid_init_to_null", "var x = null;", 4..12, "var x;");
        assert_rewrite(
            "avoid_init_to_null",
            "void f({int x = null}) {}",
            8..20,
            "void f({int x}) {}",
        );
    }

    /// `!(x is T)` → `x is! T`; the parentheses go when the `!`'s context
    /// binds looser than `is!`, stay when it does not (a prefix `!`
    /// chain). A drifted row over a non-matching span drops.
    #[test]
    fn is_not_operator_rewrite() {
        assert_rewrite(
            "prefer_is_not_operator",
            "var b = !(a is int);",
            8..19,
            "var b = a is! int;",
        );
        assert_rewrite(
            "prefer_is_not_operator",
            "var b = !!(a is int);",
            9..20,
            "var b = !(a is! int);",
        );
        // The `is` inside a string literal is never the operator; the
        // rewrite still lands on the real one.
        assert_rewrite(
            "prefer_is_not_operator",
            "var b = !('is' is int);",
            8..22,
            "var b = 'is' is! int;",
        );
        // A hit span that does not open with the `!` never matches.
        assert_no_fix("prefer_is_not_operator", "var b = !(a is int);", 9..19);
    }

    /// `x == null ? a : x` → `x ?? a`, with the default arm parenthesized
    /// exactly when it is itself a bare conditional.
    #[test]
    fn if_null_operators_rewrite() {
        assert_rewrite(
            "prefer_if_null_operators",
            "var x = a == null ? b : a;",
            8..25,
            "var x = a ?? b;",
        );
        assert_rewrite(
            "prefer_if_null_operators",
            "var x = a != null ? a : b;",
            8..25,
            "var x = a ?? b;",
        );
        assert_rewrite(
            "prefer_if_null_operators",
            "var x = null == a ? b : a;",
            8..25,
            "var x = a ?? b;",
        );
        // A bare conditional default arm takes parentheses.
        assert_rewrite(
            "prefer_if_null_operators",
            "var x = a == null ? c ? d : e : a;",
            8..33,
            "var x = a ?? (c ? d : e);",
        );
    }

    /// `x == null ? null : x.y` → `x?.y`: the access chain peels to the
    /// checked operand; `.` goes `?.`, a trailing `!` goes `?`.
    #[test]
    fn null_aware_operators_rewrite() {
        assert_rewrite(
            "prefer_null_aware_operators",
            "var x = a == null ? null : a.b;",
            8..30,
            "var x = a?.b;",
        );
        assert_rewrite(
            "prefer_null_aware_operators",
            "var x = a != null ? a.b.c : null;",
            8..32,
            "var x = a?.b.c;",
        );
        assert_rewrite(
            "prefer_null_aware_operators",
            "var x = a == null ? null : a!.b;",
            8..31,
            "var x = a?.b;",
        );
    }

    /// The `indexOf` comparisons rewrite to `contains` per the official
    /// matrix; the always-true/false judgments get no fix.
    #[test]
    fn contains_rewrite() {
        assert_rewrite(
            "prefer_contains",
            "var b = x.indexOf(y) == -1;",
            8..26,
            "var b = !x.contains(y);",
        );
        assert_rewrite(
            "prefer_contains",
            "var b = x.indexOf(y) != -1;",
            8..26,
            "var b = x.contains(y);",
        );
        assert_rewrite(
            "prefer_contains",
            "var b = x.indexOf(y, 0) >= 0;",
            8..28,
            "var b = x.contains(y);",
        );
        assert_rewrite(
            "prefer_contains",
            "var b = -1 != x.indexOf(y);",
            8..26,
            "var b = x.contains(y);",
        );
        // `indexOf < -1` is the always-false judgment — no mechanical fix.
        assert_no_fix("prefer_contains", "var b = x.indexOf(y) < -1;", 8..25);
    }

    /// `'a' + b` → `'a$b'`: quote style rides the chain's literals, a
    /// bare identifier takes `$`, an identifier before an
    /// identifier-continuation head takes `${}`, `x.toString()` unwraps.
    #[test]
    fn interpolation_compose_rewrite() {
        assert_rewrite(
            "prefer_interpolation_to_compose_strings",
            "var s = 'a' + b;",
            8..15,
            "var s = 'a$b';",
        );
        assert_rewrite(
            "prefer_interpolation_to_compose_strings",
            "var s = a + 'b';",
            8..15,
            "var s = '${a}b';",
        );
        assert_rewrite(
            "prefer_interpolation_to_compose_strings",
            r"var s = 'a\\' + b.toString() + c;",
            8..32,
            r"var s = 'a\\${b}$c';",
        );
        // Mixed quote styles drop; raw strings drop.
        assert_no_fix(
            "prefer_interpolation_to_compose_strings",
            "var s = 'a' + b + \"c\";",
            8..21,
        );
        assert_no_fix(
            "prefer_interpolation_to_compose_strings",
            r"var s = r'a' + b;",
            8..16,
        );
    }

    /// The collection-literal rewrites: `Map()` → `{}` (type arguments
    /// ride), `Set()` → `<dynamic>{}` unless the context declares `Set`,
    /// `Set.from/of([…])` and `[…].toSet()` → the set literal.
    #[test]
    fn collection_literal_rewrite() {
        assert_rewrite(
            "prefer_collection_literals",
            "var m = Map();",
            8..13,
            "var m = {};",
        );
        assert_rewrite(
            "prefer_collection_literals",
            "var m = Map<String, int>();",
            8..26,
            "var m = <String, int>{};",
        );
        assert_rewrite(
            "prefer_collection_literals",
            "var s = Set();",
            8..13,
            "var s = <dynamic>{};",
        );
        assert_rewrite(
            "prefer_collection_literals",
            "Set s = Set();",
            8..13,
            "Set s = {};",
        );
        assert_rewrite(
            "prefer_collection_literals",
            "var s = Set.from([1, 2]);",
            8..24,
            "var s = {1, 2};",
        );
        assert_rewrite(
            "prefer_collection_literals",
            "var s = Set<int>.of([]);",
            8..23,
            "var s = <int>{};",
        );
        assert_rewrite(
            "prefer_collection_literals",
            "var s = [1].toSet();",
            8..19,
            "var s = {1};",
        );
        assert_rewrite(
            "prefer_collection_literals",
            "var s = [].toSet();",
            8..18,
            "var s = <dynamic>{};",
        );
    }

    /// The cascade `add`/`addAll` sections compose into the target
    /// literal — one whole-tail replacement per cascade, chained
    /// sections included, later sections' hits dropping.
    #[test]
    fn inlined_adds_rewrite() {
        assert_rewrite(
            "prefer_inlined_adds",
            "var l = [1]..add(2);",
            13..16,
            "var l = [1, 2];",
        );
        assert_rewrite(
            "prefer_inlined_adds",
            "var l = []..add(2);",
            12..15,
            "var l = [2];",
        );
        assert_rewrite(
            "prefer_inlined_adds",
            "var l = [1]..addAll([2, 3]);",
            13..19,
            "var l = [1, 2, 3];",
        );
        // A chained cascade: the first section's fix composes every
        // `add` after it in one edit.
        let source = "var l = [1]..add(2)..add(3);";
        let fix = one_fix(source, &[row("prefer_inlined_adds", 13..16)]);
        assert_eq!(fix.edit.replacement, ", 2, 3]");
        // The second section's own hit cannot anchor at the literal and
        // produces no competing edit.
        let lines = LineIndex::new(source);
        let second = from_lints(&[row("prefer_inlined_adds", 21..24)], source, &lines);
        assert!(second.is_empty());
    }

    /// `var f = () {};` → `void f() {};` — wait, the semicolon goes with
    /// a block body (the official face): `void f() {}`.
    #[test]
    fn function_declaration_rewrite() {
        assert_rewrite(
            "prefer_function_declarations_over_variables",
            "var f = () {};",
            4..13,
            "void f() {}",
        );
        assert_rewrite(
            "prefer_function_declarations_over_variables",
            "final f = (a) { print(a); };",
            6..27,
            "void f(a) { print(a); }",
        );
        assert_rewrite(
            "prefer_function_declarations_over_variables",
            "var f = () async {};",
            4..19,
            "Future<void> f() async {}",
        );
        // A value return has no derivable return type: no fix.
        assert_no_fix(
            "prefer_function_declarations_over_variables",
            "var f = () { return 1; };",
            4..24,
        );
        // An expression body likewise; a sibling variable cannot split.
        assert_no_fix(
            "prefer_function_declarations_over_variables",
            "var f = () => 1;",
            4..15,
        );
        assert_no_fix(
            "prefer_function_declarations_over_variables",
            "var a = 1, f = () {};",
            11..20,
        );
    }

    /// §15.6's drift case: an unrelated overlay above the edit shifts its
    /// offset; the anchor re-locates it (a unique context match off the
    /// anchor line) and the rewrite lands on the moved construct.
    #[test]
    fn drift_relocates_by_anchor() {
        let source = "void f() { var a = new B(); }\n";
        let edit = one_fix(source, &[new_row(19)]).edit;

        let drifted = format!("// header overlay\n{source}");
        let (patched, outcomes) = apply(&drifted, &[edit]);
        assert_eq!(outcomes, &[EditOutcome::Applied]);
        assert_eq!(patched, "// header overlay\nvoid f() { var a = B(); }\n");
    }

    /// A vanished construct goes stale — never a blind write; two
    /// equally-flanked candidates on the anchor line are ambiguous and go
    /// stale too. Both scenarios shift the claimed position first (an
    /// exact-position match is provable wherever it still holds).
    #[test]
    fn stale_when_gone_or_ambiguous() {
        let source = "x(new B());\n";
        let edit = one_fix(source, &[new_row(2)]).edit;
        assert_eq!(edit.anchor.context_before, "x(");

        let (patched, outcomes) = apply("void f() { var a = C(); }\n", std::slice::from_ref(&edit));
        assert_eq!(outcomes, &[EditOutcome::Stale]);
        assert_eq!(patched, "void f() { var a = C(); }\n", "stale never writes");

        // Two spaces shift both occurrences off the claimed offset; both
        // flank equally on the anchor line → ambiguous → stale.
        let (patched, outcomes) = apply("  x(new B()); x(new B());\n", std::slice::from_ref(&edit));
        assert_eq!(outcomes, &[EditOutcome::Stale]);
        assert_eq!(patched, "  x(new B()); x(new B());\n");
    }

    /// Two edits claiming one span: the first (highest offset, input order
    /// on ties) applies, the overlap goes stale.
    #[test]
    fn overlap_goes_stale() {
        let source = "if (b) ; else ;";
        let fix = one_fix(source, &[empty_else_row(14)]);
        let overwrite = Edit {
            offset: fix.edit.offset,
            length: 1,
            source: ";".to_owned(),
            replacement: String::new(),
            anchor: fix.edit.anchor.clone(),
        };
        let (patched, outcomes) = apply(source, &[fix.edit.clone(), overwrite]);
        assert_eq!(outcomes, &[EditOutcome::Applied, EditOutcome::Stale]);
        assert_eq!(patched, "if (b) ; else {}");
    }

    /// The machine document is §15.6 verbatim plus the provenance tag:
    /// exactly `file`, `schema_version`, and per-edit `offset`/`length`/
    /// `replacement`/`anchor`/`source` — the anchor carrying `line`/
    /// `column`/`context_before`/`context_after`, an empty context
    /// omitted (§15.7), and `source` naming the row's fix source
    /// (builtin rows `builtin-lint`, plugin rows the manifest name). Each
    /// tier's document passes the published schema contract
    /// (`dartq-schema`'s `Domain::FixPatch`) at the tier the writer
    /// claims — the provenance surface stays schema-valid.
    #[test]
    fn schema_pins_section_15_6_fields() {
        let source = "void f() { var a = new B(); }\n";
        let lines = LineIndex::new(source);
        let mut plugin = one_plugin_fix(source, 19, 4);
        plugin.plugin = "live".to_owned();
        let doc = patch_json(
            &FileFixSet {
                path: "/abs/t.dart".to_owned(),
                fixes: from_lints(&[new_row(19)], source, &lines),
                plugin: vec![plugin],
            },
            Verbosity::Compact,
        );

        let keys: Vec<_> = doc.as_object().expect("document object").keys().collect();
        assert_eq!(keys, ["edits", "file", "schema_version"]);
        assert_eq!(doc["schema_version"], 1);
        assert_eq!(doc["file"], "/abs/t.dart");
        let text = doc.to_string();
        dartq_schema::validate(&text, dartq_schema::Domain::FixPatch).expect("schema-valid");
        dartq_schema::validate_at_tier(
            &text,
            dartq_schema::Domain::FixPatch,
            dartq_schema::Verbosity::Compact,
        )
        .expect("schema-valid at the compact tier");

        let edits = doc["edits"].as_array().expect("edits array");
        assert_eq!(
            edits.len(),
            2,
            "the built-in row first, then the plugin row"
        );
        for (edit, expected_source) in edits.iter().zip(["builtin-lint", "live"]) {
            let keys: Vec<_> = edit.as_object().expect("edit object").keys().collect();
            assert_eq!(
                keys,
                ["anchor", "length", "offset", "replacement", "source"]
            );
            assert_eq!(edit["source"], expected_source);
        }
        assert_eq!(edits[0]["offset"], 19);
        assert_eq!(edits[0]["length"], 4);
        assert_eq!(edits[0]["replacement"], "");

        let anchor = &edits[0]["anchor"];
        let keys: Vec<_> = anchor.as_object().expect("anchor object").keys().collect();
        assert_eq!(keys, ["column", "context_after", "context_before", "line"]);
        assert_eq!(anchor["line"], 1);
        assert_eq!(anchor["column"], 20);
        assert_eq!(anchor["context_before"], "var a = ");
        assert_eq!(anchor["context_after"], "B(); }");

        // The minimal tier keeps the locator floor and the provenance,
        // drops the anchor; the full tier adds the human names.
        let set = FileFixSet {
            path: "t.dart".to_owned(),
            fixes: from_lints(&[new_row(19)], source, &lines),
            plugin: vec![one_plugin_fix(source, 19, 4)],
        };
        let minimal = patch_json(&set, Verbosity::Minimal);
        let keys: Vec<_> = minimal["edits"][0]
            .as_object()
            .expect("edit object")
            .keys()
            .collect();
        assert_eq!(keys, ["length", "offset", "replacement", "source"]);
        dartq_schema::validate_at_tier(
            &minimal.to_string(),
            dartq_schema::Domain::FixPatch,
            dartq_schema::Verbosity::Minimal,
        )
        .expect("schema-valid at the minimal tier");

        let full = patch_json(&set, Verbosity::Full);
        let builtin: Vec<_> = full["edits"][0]
            .as_object()
            .expect("edit object")
            .keys()
            .collect();
        assert_eq!(
            builtin,
            [
                "anchor",
                "length",
                "message",
                "offset",
                "replacement",
                "rule",
                "source"
            ]
        );
        assert_eq!(full["edits"][0]["rule"], "unnecessary_new");
        let plugin_row: Vec<_> = full["edits"][1]
            .as_object()
            .expect("edit object")
            .keys()
            .collect();
        assert_eq!(
            plugin_row,
            [
                "anchor",
                "length",
                "message",
                "offset",
                "priority",
                "replacement",
                "source"
            ]
        );
        assert_eq!(full["edits"][1]["message"], "insert const");
        assert_eq!(full["edits"][1]["priority"], 1);
        dartq_schema::validate_at_tier(
            &full.to_string(),
            dartq_schema::Domain::FixPatch,
            dartq_schema::Verbosity::Full,
        )
        .expect("schema-valid at the full tier");

        // An edit at a line start carries no context_before: the empty
        // string is omitted from the document (§15.7).
        let text = "var a;\n";
        let lines = LineIndex::new(text);
        let doc = patch_json(
            &FileFixSet {
                path: "t.dart".to_owned(),
                fixes: vec![RuleFix {
                    rule: "unnecessary_new",
                    message: String::new(),
                    edit: anchored_edit(text, &lines, 0, 3, "out"),
                }],
                plugin: Vec::new(),
            },
            Verbosity::Compact,
        );
        let anchor = &doc["edits"][0]["anchor"];
        let keys: Vec<_> = anchor.as_object().expect("anchor object").keys().collect();
        assert_eq!(keys, ["column", "context_after", "line"]);
        assert_eq!(anchor["context_after"], " a;");
        dartq_schema::validate(&doc.to_string(), dartq_schema::Domain::FixPatch)
            .expect("the omitted-context document is schema-valid");
    }

    /// Rules without a mechanical rewrite are never claimed: a hit of a
    /// non-wired rule produces no fix, and a wired rule's row whose slice
    /// does not verify produces none either.
    #[test]
    fn unverified_rows_produce_no_fix() {
        let source = "class foo {}\n";
        let lines = LineIndex::new(source);
        let non_wired = vec![Lint {
            rule: rule_by_name("camel_case_types").expect("wired registry"),
            range: 6..9,
            message: "Type names should be CamelCase.".to_owned(),
            correction: None,
        }];
        assert!(from_lints(&non_wired, source, &lines).is_empty());

        // `unnecessary_new` over text that does not start with the keyword.
        let wrong = vec![new_row(6)];
        assert!(from_lints(&wrong, source, &lines).is_empty());

        // `avoid_empty_else` over a span that is not the `;`.
        let wrong = vec![empty_else_row(0)];
        assert!(from_lints(&wrong, source, &lines).is_empty());
    }

    // -----------------------------------------------------------------------
    // Plugin source
    // -----------------------------------------------------------------------

    /// A reply of single-edit changes, `(priority, message, offset,
    /// length, replacement)` tuples — the fixture plugins' reply shape.
    fn plugin_reply(changes: &[(i32, &str, u64, u64, &str)]) -> FixesReply {
        FixesReply {
            fixes: changes
                .iter()
                .map(
                    |(priority, message, offset, length, replacement)| PrioritizedChange {
                        priority: *priority,
                        message: (*message).to_owned(),
                        edits: vec![SourceEdit {
                            offset: *offset,
                            length: *length,
                            replacement: (*replacement).to_owned(),
                        }],
                    },
                )
                .collect(),
        }
    }

    /// The fixer fixture's canonical change (`insert const`, priority 1)
    /// anchored at `offset`/`length` over `source`.
    fn one_plugin_fix(source: &str, offset: u32, length: u32) -> PluginFix {
        let lines = LineIndex::new(source);
        let reply = plugin_reply(&[(
            1,
            "insert const",
            u64::from(offset),
            u64::from(length),
            "const ",
        )]);
        let mut fixes = from_plugin("fixer", &reply, source, &lines);
        assert_eq!(fixes.len(), 1, "expected exactly one plugin fix");
        fixes.swap_remove(0)
    }

    /// The reply converts priority-first (ties keep arrival order), and
    /// each edit anchors against the source: the anchored span flanks and
    /// the 1-based line/column match the text.
    #[test]
    fn plugin_rows_sort_by_priority_and_anchor() {
        let source = "var a = 1;\nvar b = 2;\n";
        let lines = LineIndex::new(source);
        let reply = plugin_reply(&[
            (2, "second", 15, 1, "B"),
            (1, "first", 0, 3, "aaa"),
            (1, "first tie", 11, 3, "ccc"),
        ]);
        let fixes = from_plugin("live", &reply, source, &lines);
        let titles: Vec<&str> = fixes.iter().map(|fix| fix.message.as_str()).collect();
        assert_eq!(
            titles,
            ["first", "first tie", "second"],
            "stable priority sort"
        );
        assert!(fixes.iter().all(|fix| fix.plugin == "live"));
        assert_eq!(fixes[0].priority, 1);

        let edit = &fixes[0].edits[0];
        assert_eq!(edit.offset, 0);
        assert_eq!(edit.length, 3);
        assert_eq!(edit.replacement, "aaa");
        assert_eq!(edit.source, "var");
        assert_eq!(edit.anchor.line, 1);
        assert_eq!(edit.anchor.column, 1);
        assert_eq!(edit.anchor.context_before, "");
        assert_eq!(edit.anchor.context_after, " a = 1;");

        let (patched, outcomes) = apply(
            source,
            &fixes
                .iter()
                .flat_map(|fix| fix.edits.iter().cloned())
                .collect::<Vec<_>>(),
        );
        assert_eq!(outcomes, &[EditOutcome::Applied; 3]);
        assert_eq!(patched, "aaa a = 1;\nccc B = 2;\n");
    }

    /// Plugin offsets are untrusted wire data: a change with any
    /// out-of-range edit is dropped whole (never half a change), and the
    /// in-range changes around it survive.
    #[test]
    fn plugin_change_with_an_out_of_range_edit_drops_whole() {
        let source = "var a = 1;\n";
        let lines = LineIndex::new(source);
        let reply = FixesReply {
            fixes: vec![
                PrioritizedChange {
                    priority: 1,
                    message: "spills past the end".to_owned(),
                    edits: vec![
                        SourceEdit {
                            offset: 0,
                            length: 3,
                            replacement: "x".to_owned(),
                        },
                        // 8 + 10 runs past the file's UTF-16 length.
                        SourceEdit {
                            offset: 8,
                            length: 10,
                            replacement: "y".to_owned(),
                        },
                    ],
                },
                PrioritizedChange {
                    priority: 2,
                    message: "u64 beyond u32".to_owned(),
                    edits: vec![SourceEdit {
                        offset: u64::from(u32::MAX) + 1,
                        length: 1,
                        replacement: "z".to_owned(),
                    }],
                },
                PrioritizedChange {
                    priority: 3,
                    message: "in range".to_owned(),
                    edits: vec![SourceEdit {
                        offset: 4,
                        length: 1,
                        replacement: "=".to_owned(),
                    }],
                },
            ],
        };
        let fixes = from_plugin("live", &reply, source, &lines);
        let titles: Vec<&str> = fixes.iter().map(|fix| fix.message.as_str()).collect();
        assert_eq!(
            titles,
            ["in range"],
            "the two broken changes dropped atomically"
        );
    }

    /// The anchor-stale contract holds for plugin rows exactly as for the
    /// built-in rows — they share the anchored [`Edit`]: an unrelated
    /// overlay above the edit re-locates it by its anchor, a vanished
    /// construct goes stale without writing, and offsets shifted onto an
    /// ambiguous position go stale too.
    #[test]
    fn plugin_rows_apply_drift_and_stale() {
        let source = "void f() { var a = new B(); }\n";
        let edit = one_plugin_fix(source, 19, 4).edits.swap_remove(0);
        assert_eq!(edit.replacement, "const ");

        let (patched, outcomes) = apply(source, std::slice::from_ref(&edit));
        assert_eq!(outcomes, &[EditOutcome::Applied]);
        assert_eq!(patched, "void f() { var a = const B(); }\n");

        // A header overlay shifted every offset by one line: re-located.
        let drifted = format!("// header overlay\n{source}");
        let (patched, outcomes) = apply(&drifted, std::slice::from_ref(&edit));
        assert_eq!(outcomes, &[EditOutcome::Applied]);
        assert_eq!(
            patched,
            "// header overlay\nvoid f() { var a = const B(); }\n"
        );

        // The construct is gone: stale, and the buffer is untouched.
        let (patched, outcomes) = apply("void f() { var a = B(); }\n", std::slice::from_ref(&edit));
        assert_eq!(outcomes, &[EditOutcome::Stale]);
        assert_eq!(patched, "void f() { var a = B(); }\n");
    }

    /// End to end through the #355 bridge: [`collect_set`] parses a real
    /// file on disk, the governing options file's `linter.rules` include
    /// fires `unnecessary_new`, the mechanical rewrite materializes an
    /// anchored row with the builtin provenance, and applying it rewrites
    /// the source. Plugin-less hosts contribute no rows and no dispatch.
    #[test]
    fn collect_set_emits_real_patches_through_the_bridge() {
        let root = std::env::temp_dir().join(format!("dq_fix_bridge_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        std::fs::write(
            root.join("analysis_options.yaml"),
            "linter:\n  rules:\n    - unnecessary_new\n",
        )
        .expect("options file");
        let path = root.join("a.dart");
        let source = "void f() { var a = new B(); }\n";
        std::fs::write(&path, source).expect("write");

        let mut hosts = PluginHosts::empty();
        let sets = collect_set(&[root.clone()], &mut hosts).expect("collect");
        assert_eq!(sets.len(), 1, "the one file with fixes enters the set");
        assert_eq!(sets[0].path, path.display().to_string());
        assert!(sets[0].plugin.is_empty(), "no hosts, no plugin rows");
        assert_eq!(hosts.take_errors(), Vec::new());

        let fix = &sets[0].fixes[0];
        assert_eq!(fix.rule, "unnecessary_new");
        assert_eq!(fix.edit.offset, 19);
        assert_eq!(fix.edit.length, 4, "keyword plus trailing space");
        assert_eq!(fix.edit.replacement, "");
        assert_eq!(fix.edit.anchor.line, 1);

        let (patched, outcomes) = apply(source, std::slice::from_ref(&fix.edit));
        assert_eq!(outcomes, &[EditOutcome::Applied]);
        assert_eq!(patched, "void f() { var a = B(); }\n");

        // The document carries the builtin provenance on the row.
        let doc = patch_json(&sets[0].clone(), Verbosity::Compact);
        assert_eq!(doc["edits"][0]["source"], BUILTIN_SOURCE);

        std::fs::remove_dir_all(&root).ok();
    }

    /// The official options-less posture at the fix face: no
    /// `analysis_options.yaml` in the target's ancestry is the empty lint
    /// set — a fixable shape in the file produces no lint-driven rows (the
    /// #936 ruling; the fix face enables nothing on its own).
    #[test]
    fn collect_set_options_less_runs_no_lints() {
        let root = std::env::temp_dir().join(format!("dq_fix_bare_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        std::fs::write(root.join("a.dart"), "void f() { var a = new B(); }\n").expect("write");

        let mut hosts = PluginHosts::empty();
        let sets = collect_set(&[root.clone()], &mut hosts).expect("collect");
        assert!(sets.is_empty(), "no governing options, no lint rows");
        assert_eq!(hosts.take_errors(), Vec::new());

        std::fs::remove_dir_all(&root).ok();
    }

    /// The fix face's per-unit version wiring: the fixture's
    /// `languageVersion` attributes through the workspace assembly and each
    /// bit tracks its official `enabledIn` gate
    /// (`tools/experimental_features.yaml`): a 3.5 package is past
    /// `patterns` (`3.0.0`) and `unnamed_libraries` (`2.19.0`) but before
    /// `wildcard_variables` (`3.7.0`); a 2.19 package additionally drops
    /// `patterns`. `enhanced_parts` has no `enabledIn` — off everywhere.
    #[test]
    fn unit_features_derive_from_the_containing_package() {
        let fixture = |version: &str, tag: &str| {
            let root = crate::workspace::test_fixture::workspace(tag, &[]);
            std::fs::write(
                root.join(".dart_tool/package_config.json"),
                format!(
                    r#"{{"configVersion": 2, "packages": [{{"name": "fixture", "rootUri": "../", "languageVersion": "{version}"}}], "generator": "pub"}}"#
                ),
            )
            .expect("config");
            let (table, _, _, _, _) = crate::workspace::package_assembly(&root).expect("assembly");
            let features = unit_features(Some(&table), &root.join("lib/a.dart"));
            std::fs::remove_dir_all(&root).ok();
            features
        };
        assert_eq!(
            fixture("3.5", "fix_features_pre_wildcard"),
            Features {
                wildcard_variables: false,
                ..Features::default()
            },
            "3.5 predates wildcard-variables' 3.7.0 enabledIn",
        );
        assert_eq!(
            fixture("2.19", "fix_features_legacy"),
            Features {
                wildcard_variables: false,
                patterns: false,
                ..Features::default()
            },
            "the wave9 pin: legacy duplicates report",
        );
    }

    /// The assists document: rows keep the change grouping with their
    /// provenance, title, and priority; `errors` appears exactly when a
    /// host failed; the minimal tier drops the anchor and the priority.
    #[test]
    fn assists_document_shape() {
        let source = "void f() { var a = new B(); }\n";
        let answer = AssistAnswer {
            file: "lib/t.dart".to_owned(),
            assists: vec![one_plugin_fix(source, 19, 4)],
            errors: vec![PluginHostError {
                plugin: "dead".to_owned(),
                message: "restart budget exhausted".to_owned(),
            }],
        };
        let compact = assists_json(&answer, Verbosity::Compact);
        assert_eq!(compact["file"], "lib/t.dart");
        assert_eq!(compact["schema_version"], SCHEMA_VERSION);
        let row = &compact["assists"][0];
        assert_eq!(row["message"], "insert const");
        assert_eq!(row["source"], "fixer");
        assert_eq!(row["priority"], 1);
        assert_eq!(row["edits"][0]["offset"], 19);
        assert_eq!(row["edits"][0]["replacement"], "const ");
        assert_eq!(row["edits"][0]["anchor"]["line"], 1);
        assert_eq!(compact["errors"][0]["plugin"], "dead");
        let keys: Vec<_> = compact
            .as_object()
            .expect("document object")
            .keys()
            .collect();
        assert_eq!(keys, ["assists", "errors", "file", "schema_version"]);

        let minimal = assists_json(&answer, Verbosity::Minimal);
        let row = &minimal["assists"][0];
        let keys: Vec<_> = row.as_object().expect("assists row").keys().collect();
        assert_eq!(keys, ["edits", "message", "source"], "priority drops");
        let keys: Vec<_> = row["edits"][0]
            .as_object()
            .expect("edit row")
            .keys()
            .collect();
        assert_eq!(
            keys,
            ["length", "offset", "replacement", "source"],
            "anchor drops"
        );

        let mut clean = answer.clone();
        clean.errors.clear();
        let compact = assists_json(&clean, Verbosity::Compact);
        assert!(
            compact.get("errors").is_none(),
            "empty error lists are omitted (§15.7)"
        );
    }
}
