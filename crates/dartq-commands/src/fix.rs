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
//! `run` face): each wired rule maps a hit to a span-exact rewrite whose
//! source slice is verified before the patch is offered —
//! `unnecessary_new` and `unnecessary_late` drop the keyword plus its
//! trailing horizontal whitespace, `avoid_empty_else` rewrites the else
//! `;` to `{}`, `unnecessary_brace_in_string_interps` drops the braces of
//! a simple interpolation. Rules without a mechanical rewrite are never
//! claimed, and a wired hit whose slice no longer verifies is dropped
//! rather than patched on faith.
//!
//! The rows reach [`from_lints`] through the lints layer's diagnostic
//! face: [`collect_set`] parses each discovered file once through the
//! layer's production entry ([`dartq_lints::parse_unit`] — the #355
//! bridge) and runs the default `package:lints` posture (the fix face
//! targets the paths you point it at, without the analyze pipeline's
//! options context). Rules without a mechanical rewrite are never
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

use std::path::{Path, PathBuf};

use dartq_lints::{EnableSet, Features, Lint, RULES, parse_unit, run};
use dartq_plugin::{
    Capability, FixesReply, PluginRuntime, PrioritizedChange, SpawnSpec,
    discover as discover_plugins,
};
use dartq_source::{LineIndex, utf16_len_of, utf16_offset_to_byte};

use crate::Verbosity;
use crate::discover::{self, DiscoverError};

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
        Self {
            hosts: Vec::new(),
        }
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
            if let Some(runtime) = host.runtime {
                if let Err(error) = runtime.shutdown() {
                    eprintln!("dartq: plugin `{}` shutdown failed: {error}", host.name);
                }
            }
        }
    }
}

/// The fix source stage: validates the targets through discovery, then
/// parses each file once through the lints layer's production entry and
/// runs the default `package:lints` posture; every hit the mechanical
/// rewrites can materialize (and whose source slice verifies) becomes an
/// anchored edit, and every lint hit is a `request/fixes` location for
/// the plugin hosts (one request per hit per live host — the protocol's
/// only query granularity). One fix set per patched file — files without
/// rows never enter the set.
///
/// # Errors
///
/// [`DiscoverError`] from target discovery.
pub fn collect_set(
    paths: &[PathBuf],
    hosts: &mut PluginHosts,
) -> Result<Vec<FileFixSet>, DiscoverError> {
    let files = discover::discover(paths)?;
    let enabled = crate::lints::enable_set(None);
    if enabled == EnableSet::none() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            // Unreadable or non-UTF-8 target: nothing to patch — the apply
            // face re-reads and reports on its own terms.
            continue;
        };
        let parsed = parse_unit(&text, Features::default());
        let hits: Vec<Lint> = run(&parsed.unit(Some(path)), &enabled);
        let fixes = from_lints(&hits, &text, &parsed.lines);
        let plugin = dispatch_fixes(hosts, path, &text, &parsed.lines, &hits);
        if !fixes.is_empty() || !plugin.is_empty() {
            out.push(FileFixSet {
                path: path.display().to_string(),
                fixes,
                plugin,
            });
        }
    }
    Ok(out)
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
    hits.iter()
        .filter_map(|hit| {
            let rule = RULES[hit.rule.0 as usize].name;
            let (offset, length, replacement) = match rule {
                "unnecessary_new" => drop_keyword(text, hit.range.start, "new")?,
                "unnecessary_late" => drop_keyword(text, hit.range.start, "late")?,
                "avoid_empty_else" => replace_exact(text, &hit.range, ";", "{}")?,
                "unnecessary_brace_in_string_interps" => unbrace(text, &hit.range)?,
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
                edits.push(anchored_edit(text, lines, offset, length, &edit.replacement));
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
        json.insert("context_before".into(), anchor.context_before.clone().into());
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
        document["errors"] = serde_json::json!(answer
            .errors
            .iter()
            .map(|error| serde_json::json!({
                "plugin": error.plugin,
                "message": error.message,
            }))
            .collect::<Vec<_>>());
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
    /// (builtin rows `builtin-lint`, plugin rows the manifest name).
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

        let edits = doc["edits"].as_array().expect("edits array");
        assert_eq!(edits.len(), 2, "the built-in row first, then the plugin row");
        for (edit, expected_source) in edits.iter().zip(["builtin-lint", "live"]) {
            let keys: Vec<_> = edit.as_object().expect("edit object").keys().collect();
            assert_eq!(keys, ["anchor", "length", "offset", "replacement", "source"]);
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

        let full = patch_json(&set, Verbosity::Full);
        let builtin: Vec<_> = full["edits"][0]
            .as_object()
            .expect("edit object")
            .keys()
            .collect();
        assert_eq!(builtin, ["anchor", "length", "message", "offset", "replacement", "rule", "source"]);
        assert_eq!(full["edits"][0]["rule"], "unnecessary_new");
        let plugin_row: Vec<_> = full["edits"][1]
            .as_object()
            .expect("edit object")
            .keys()
            .collect();
        assert_eq!(
            plugin_row,
            ["anchor", "length", "message", "offset", "priority", "replacement", "source"]
        );
        assert_eq!(full["edits"][1]["message"], "insert const");
        assert_eq!(full["edits"][1]["priority"], 1);

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
                .map(|(priority, message, offset, length, replacement)| PrioritizedChange {
                    priority: *priority,
                    message: (*message).to_owned(),
                    edits: vec![SourceEdit {
                        offset: *offset,
                        length: *length,
                        replacement: (*replacement).to_owned(),
                    }],
                })
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
        assert_eq!(titles, ["first", "first tie", "second"], "stable priority sort");
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
            &fixes.iter().flat_map(|fix| fix.edits.iter().cloned()).collect::<Vec<_>>(),
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
                        SourceEdit { offset: 0, length: 3, replacement: "x".to_owned() },
                        // 8 + 10 runs past the file's UTF-16 length.
                        SourceEdit { offset: 8, length: 10, replacement: "y".to_owned() },
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
                    edits: vec![SourceEdit { offset: 4, length: 1, replacement: "=".to_owned() }],
                },
            ],
        };
        let fixes = from_plugin("live", &reply, source, &lines);
        let titles: Vec<&str> = fixes.iter().map(|fix| fix.message.as_str()).collect();
        assert_eq!(titles, ["in range"], "the two broken changes dropped atomically");
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
        assert_eq!(patched, "// header overlay\nvoid f() { var a = const B(); }\n");

        // The construct is gone: stale, and the buffer is untouched.
        let (patched, outcomes) = apply("void f() { var a = B(); }\n", std::slice::from_ref(&edit));
        assert_eq!(outcomes, &[EditOutcome::Stale]);
        assert_eq!(patched, "void f() { var a = B(); }\n");
    }

    /// End to end through the #355 bridge: [`collect_set`] parses a real
    /// file on disk, the default `package:lints` posture fires
    /// `unnecessary_new`, the mechanical rewrite materializes an anchored
    /// row with the builtin provenance, and applying it rewrites the
    /// source. Plugin-less hosts contribute no rows and no dispatch.
    #[test]
    fn collect_set_emits_real_patches_through_the_bridge() {
        let root = std::env::temp_dir().join(format!("dq_fix_bridge_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
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
        assert_eq!(keys, ["length", "offset", "replacement", "source"], "anchor drops");

        let mut clean = answer.clone();
        clean.errors.clear();
        let compact = assists_json(&clean, Verbosity::Compact);
        assert!(
            compact.get("errors").is_none(),
            "empty error lists are omitted (§15.7)"
        );
    }
}
