//! The fix face's substrate: the anchored patch document, the
//! drift-correction applier, and the row source mapping. Both the CLI's
//! `dartq fix` and the daemon's `dartq/fix` consume this one implementation;
//! the CLI adds apply-to-disk and human formatting, the daemon adds the
//! JSON-RPC envelope.
//!
//! The machine document is the fix-patch schema verbatim:
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
//! crossing a line) is the drift-correction face: when concurrent
//! overlays shifted offsets, the applier re-locates the edit by its
//! context (and internal original-slice match); an anchor that is gone or
//! ambiguous marks the edit `stale` — the applier never writes at a
//! position it cannot prove.
//!
//! # Fix sources
//!
//! Patches materialize from the dartq-lints layer's diagnostic rows (its
//! `run` face): every wired rule whose official fix is a single-span
//! mechanical rewrite has a [`crate::fix_table::FixSpec`] row, and each
//! hit maps to a verified edit through one of the three action kinds —
//! lexeme drops, span-exact replacements, and span-verified rewrites —
//! each mirroring its official `dart fix` producer's edit behavior
//! (removals, insertions, and replacements of the same spans; whitespace
//! never crosses a line). [`crate::fix_table`] is the exhaustive,
//! count-bearing registry (its pin test holds the row set in place); this
//! module keeps the rewriters and the substrate, not a second rule list.
//! Rules without a mechanical rewrite are never claimed, and a wired hit
//! whose slice no longer verifies is dropped rather than patched on
//! faith. The structural rewrites verify against the token stream
//! (keyword positions are quote-safe only there); the parse is paid once
//! per `from_lints` call and only when one of their hits is present.
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
//! rather than patched on faith.
//!
//! # Semantic source
//!
//! The engine's semantic diagnostics feed the third fix source: the drop
//! family (`unused_import`, `unused_local_variable`, `unused_shown_name`,
//! `unused_catch_clause`, `unused_catch_stack`, `unused_label`) maps to
//! span-exact mechanical rewrites through the same verify-then-anchor
//! pipeline — [`from_semantic`] reads the code-keyed table
//! ([`crate::fix_table`]), and a hit whose slice does not verify drops
//! rather than patches. The null-check family
//! (`unchecked_use_of_nullable_value`, `invalid_assignment`,
//! `argument_type_not_assignable`) joins it with the official
//! `AddNullCheck` producer's edit shapes (the `!` insertion at the
//! producer's target expression, the paren wrap below postfix precedence,
//! the null-aware `?.` → `!.` rewrite), its assignability gate read off
//! the type-display pair the diagnostic's message carries — the fix is
//! offered only where the promoted `from` display reads as the `to`
//! display, the nullable-only mismatch the gate serves. The
//! written-surface family joins them: the emitted codes whose official
//! producer's edit is provable from the report span, the file text, and
//! the token stream alone — the `RemoveConst`/`RemoveQuestionMark`/
//! `RemoveNonNullAssertion`/`ReplaceWithNotNullAware` keyword and operator
//! shapes, `UseEqEqNull`/`UseNotEqNull`, `ReplaceColonWithEquals`,
//! `RemoveComparison`'s `&&`/`||` operand drop, `RemoveUnnecessaryCast`,
//! the pattern faces, `RemoveDeadCode`'s statement-run and catch-clause
//! arms, `RemoveDeadIfNull`, the unreachable switch members, the
//! `unused_element`/`unused_field`/`unused_element_parameter`
//! declaration drops, `MoveTypeArgumentsToClass`, `AddReturnNull`, and
//! `RemoveNameFromDeclarationClause`. The diagnostics
//! come from one engine check pass
//! over the target paths (inside [`collect_set`]: the pass assembles a
//! throwaway engine from the run's SDK hint; one that cannot run — no
//! usable SDK, no workspace — degrades to no rows, noted on stderr, never
//! a failed fix run). Everything else outside those families (`create_*`
//! synthesis, the `undefined_*` inventory, the type/flow codes needing
//! resolved elements) needs element, name, or type facts beyond the
//! message and stays out.
//!
//! # Parser source
//!
//! The same engine pass carries the syntax diagnostics, and the
//! [`crate::fix_table`]'s [`FixSource::Parser`] rows serve them through
//! [`from_parser`]: every parser code the official non-lint registry
//! serves with a mechanical producer (upstream Dart SDK source,
//! pkg/analysis_server/lib/src/services/correction/fix_internal.dart —
//! the `RemoveLexeme` modifier family, the keyword and comma deletions,
//! the separator swaps, the body insertions, the `expected_token` trio,
//! the constructor / on-clause / getter-parameter removals, the
//! constant-pattern `const` wraps, the number-separator cleanup) maps to
//! an edit of that producer's exact shape; the three producers that read
//! resolved types (`AddTypeName`, `AddMissingClosureParameters`,
//! `AddTypeAnnotation`) have no row. Parser reports often ride recovery
//! — the rewriters re-verify the reported shape against the text or the
//! token stream and drop rather than patch a shape the official producer
//! would not reach either. A site the scanner reports more than once (the
//! number separators) yields one edit: identical consecutive edits
//! collapse.
//!
//! # Ignore-diagnostic fixes
//!
//! Three producers serve every diagnostic the way the official registry
//! does (upstream Dart SDK source, pkg/analysis_server_plugin,
//! ignore_diagnostic.dart): a `// ignore: <code>` comment on the line
//! before the hit, an `// ignore_for_file: <code>` line beside the file's
//! header, and an `analyzer: errors: <code>: ignore` entry in the
//! governing `analysis_options.yaml`. All three are text surgery over the
//! comment stream — no semantics: the first two emit anchored rows beside
//! the mechanical ones, the third lands in its own [`FileFixSet`] for the
//! options file. A code in the governing options' `cannot-ignore` set is
//! never offered an ignore (the official gate); the official's
//! already-suppressed-via-`errors:` leg is subsumed — a code whose
//! severity the options downgrade never reaches the fix run as a
//! diagnostic.
//!
//! # Plugin source
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
//! Every emitted row is tagged with its source: [`BUILTIN_SOURCE`] for
//! [`from_lints`] rows, [`BUILTIN_SEMANTIC_SOURCE`] for [`from_semantic`]
//! rows, and [`BUILTIN_PARSER_SOURCE`] for [`from_parser`] rows (all by
//! construction, carried on the [`RuleFix`]), the plugin's
//! manifest name for [`PluginFix`] rows. [`assists`] is the
//! location-directed face over the plugin source alone (the built-in
//! layers contribute no assists); it answers [`AssistAnswer`]s whose rows
//! keep the change grouping the fix document flattens away.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dartq_ast::{NodeKind, SyntaxTable};
use dartq_diagnostics::Diagnostic;
use dartq_engine::analysis_options::AnalysisOptions;
use dartq_index::{DeclRow, FileCapture};
use dartq_language::DEFAULT_LANGUAGE_VERSION;
use dartq_lints::{EnableSet, Features, Lint, RULES, parse_unit, run};
use dartq_package::PackageTable;
use dartq_plugin::{
    Capability, FixesReply, PluginRuntime, PrioritizedChange, PubNative, SpawnSpec,
    discover as discover_plugins,
};
use dartq_pub::{PackageConfig, PackageConfigEntry};
use dartq_scanner::{Keyword, TokenKind, TokenTable};
use dartq_source::{
    FileId, LineIndex, MAX_SOURCE_BYTES, SourceRange, byte_offset_to_utf16, normalize_path,
    utf16_len_of, utf16_offset_to_byte,
};
use rustc_hash::{FxHashMap, FxHashSet};
use serde_yaml::Value as Yaml;

use crate::Verbosity;
use crate::analyze::rejection_notice;
use crate::check::{capacity_error, stat_capacity_error};
use crate::discover::{self, DiscoverError};
use crate::fix_table::{FixInput, FixSource};
use crate::lints::features_for;

/// The patch document version.
pub const SCHEMA_VERSION: u32 = 1;

/// The provenance tag of every [`RuleFix`] row born of [`from_lints`]:
/// the dartq-lints layer's wired rules (the semantic rows carry
/// [`BUILTIN_SEMANTIC_SOURCE`]; the plugin rows carry the plugin's
/// manifest name instead).
pub const BUILTIN_SOURCE: &str = "builtin-lint";

/// The provenance tag of every [`RuleFix`] row born of [`from_semantic`]:
/// the engine's semantic diagnostics — the mirror of [`BUILTIN_SOURCE`].
pub const BUILTIN_SEMANTIC_SOURCE: &str = "builtin-semantic";

/// The provenance tag of every [`RuleFix`] row born of [`from_parser`]:
/// the engine's syntax diagnostics, served against the official parser
/// fix producers.
pub const BUILTIN_PARSER_SOURCE: &str = "builtin-parser";

/// The provenance tag of the ignore-diagnostic rows (the `// ignore:`
/// comment insertions and the analysis-options entries): text surgery
/// offered for every diagnostic, official `dart fix`'s ignore producers.
pub const BUILTIN_IGNORE_SOURCE: &str = "builtin-ignore";

/// Context characters captured on each side of an edit. the example
/// shows eight before and five after; eight covers a keyword plus its
/// neighbor and the exact budget is ours to pick.
const ANCHOR_CONTEXT: usize = 8;

/// The planner fuse's base operation budget (one walk's fixed work: per-run
/// setup, the engine pass, the per-file parse and options loads).
const PLANNER_FUSE_BASE: u64 = 1 << 20;

/// The planner fuse's per-target operation budget: one file's planner
/// steps are its lint hits + engine diagnostics + the doubled ignore
/// offers + the plugin dispatches + the assist/evidence trials — a
/// diagnostic-dense file plans in the hundreds of steps, and the parser's
/// own 10,000-diagnostic cap bounds the parser-sourced rows, so 16,384 is
/// orders of magnitude above any legitimate file.
const PLANNER_FUSE_PER_FILE: u64 = 1 << 14;

/// The planner walk's monotone operation counter — the fix engine's global
/// termination backstop (zero-hang audit 2026-10-17, top risk 1). Every
/// planner loop family — the per-file walk, the per-hit and
/// per-diagnostic expansions, the ignore offers, the plugin dispatch, the
/// evidence rows, the assist trials — routes one counted [`PlannerFuse::step`]
/// per iteration and stops consuming input on `false`, so a walk whose
/// loops stopped advancing ends bounded with a recorded trip instead of
/// spinning a worker thread forever (the #1687 class: the daemon's fix
/// budget answers the client but does not kill the worker, so the single
/// fix slot would stall for the session's life).
///
/// The budget scales with the input the walk actually consumes —
/// [`PLANNER_FUSE_BASE`] plus [`PLANNER_FUSE_PER_FILE`] per discovered
/// target — so every legitimate run fits with wide headroom while any
/// non-advancing family burns its share without consuming input and trips
/// in bounded time. Arm-internal loops keep their audited
/// advance-or-`?`-abort discipline; this fuse is the reachable global
/// backstop those arms never had.
pub(crate) struct PlannerFuse {
    /// Counted steps so far (interior mutability: one walk's fuse is
    /// shared by `&` across its nested loop families).
    steps: Cell<u64>,
    /// The walk's step budget.
    budget: u64,
    /// Latched once: past the budget every further [`PlannerFuse::step`]
    /// answers `false`.
    tripped: Cell<bool>,
}

impl PlannerFuse {
    /// One walk's budget over `targets` discovered files.
    fn new(targets: usize) -> Self {
        Self {
            steps: Cell::new(0),
            budget: PLANNER_FUSE_BASE.saturating_add(PLANNER_FUSE_PER_FILE * targets as u64),
            tripped: Cell::new(false),
        }
    }

    /// A pin's fuse over an explicit budget (the production walks derive
    /// theirs from the discovered target count).
    #[cfg(test)]
    fn with_budget(budget: u64) -> Self {
        Self {
            steps: Cell::new(0),
            budget,
            tripped: Cell::new(false),
        }
    }

    /// Counts one planner operation; `false` once the budget is spent —
    /// the caller stops consuming input and the walk answers partial.
    fn step(&self) -> bool {
        let steps = self.steps.get() + 1;
        self.steps.set(steps);
        if steps <= self.budget {
            return true;
        }
        self.tripped.set(true);
        false
    }

    /// Whether the budget was spent (latched for the walk's remainder).
    fn tripped(&self) -> bool {
        self.tripped.get()
    }
}

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
/// matching, deliberately not part of the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub offset: u32,
    pub length: u32,
    pub source: String,
    pub replacement: String,
    pub anchor: Anchor,
}

/// One materialized fix: the anchored edit plus the human-facing row.
/// `source` is the row's provenance tag ([`BUILTIN_SOURCE`] or
/// [`BUILTIN_SEMANTIC_SOURCE`] by construction — the plugin source emits
/// [`PluginFix`]es). A row whose `target` is `Some` is the cross-file
/// face ([`crate::fix_table::FixAction::CreateFile`]): its edit anchors
/// against the target file's text — empty, the file does not exist yet —
/// not the hit file's, and it never rides the hit file's own edit set.
#[derive(Debug, Clone)]
pub struct RuleFix {
    pub rule: &'static str,
    pub message: String,
    pub source: &'static str,
    pub edit: Edit,
    /// The file this row's edit writes when it is not the hit file — the
    /// created file's path, lexically normalized. `None` (every same-file
    /// row) addresses the edit to the hit file itself.
    pub target: Option<String>,
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
    /// The anchor is gone or ambiguous — skipped.
    Stale,
}

// Domain modules split out of this file (mechanical move; this file stays the
// module root). Glob re-exports keep every historical `crate::fix::<name>`
// path — including fix_table's registry imports — resolving unchanged.
mod assists;
mod evidence;
mod ignore;
mod pipeline;
mod plugin_host;
mod rewrite_decl;
mod rewrite_expr;
mod rewrite_literals;
mod rewrite_parser;
mod rewrite_stmt;
mod rewrite_structure;
mod rewrite_text;
mod scan;

pub(crate) use assists::*;
pub(crate) use evidence::*;
pub(crate) use ignore::*;
// Test-only resolution rides this glob (`from_lints`/`from_semantic`/
// `from_parser`/`collect_set_with_evidence` have no non-test callers outside
// `pipeline`), so the lint fires only in non-test builds.
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use pipeline::*;
pub(crate) use rewrite_decl::*;
pub(crate) use rewrite_expr::*;
pub(crate) use rewrite_literals::*;
pub(crate) use rewrite_parser::*;
pub(crate) use rewrite_stmt::*;
pub(crate) use rewrite_structure::*;
pub(crate) use rewrite_text::*;
pub(crate) use scan::*;

// Public API preserved byte-for-byte: these items were `pub` in this module
// before the split; daemon/CLI (and any external consumer) import them via
// `dartq_commands::fix::`.
pub use assists::AssistContext;
pub use evidence::{EvidenceName, FileEvidence, FixEvidence, evidence_json};
pub use ignore::{ignore_file_fix, ignore_line_fix, ignore_options_edit};
pub use pipeline::{assists, collect_set, collect_set_with_evidence, from_plugin};
pub use plugin_host::{
    DeclaredPlugin, PluginHosts, declared_plugin_sources, declared_plugins,
    plugin_package_config,
};

#[cfg(test)]
mod tests;


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
/// pass. More than one candidate (on the anchor line, or anywhere
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
// Document
// ---------------------------------------------------------------------------

/// The anchor object: `line`/`column` plus the context flanks; empty
/// context strings are omitted.
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
/// provenance tag — the red line the patch rows never drop (the minimal
/// tier trims prose and convenience, never location).
fn edit_floor(edit: &Edit, source: &str) -> serde_json::Map<String, serde_json::Value> {
    let mut row = serde_json::Map::new();
    row.insert("offset".into(), edit.offset.into());
    row.insert("length".into(), edit.length.into());
    row.insert("replacement".into(), edit.replacement.clone().into());
    row.insert("source".into(), source.into());
    row
}

/// The document for one file — `file`, `schema_version`, and the
/// `edits`: the built-in rows first, then each plugin change's edits
/// flattened in change order, every row carrying its provenance tag
/// (`builtin-lint`, or the plugin's manifest name). Empty context strings
/// are omitted; the anchor rides compact and full, the full tier
/// adds the row's human name (`rule` for built-in rows, `message` +
/// `priority` for plugin rows). Files without rows never enter a set.
/// This is the proposal shape — the dry-run and daemon faces answer it
/// verbatim; the apply run's record is [`patch_json_applied`].
#[must_use]
pub fn patch_json(set: &FileFixSet, verbosity: Verbosity) -> serde_json::Value {
    patch_document(set, verbosity, None)
}

/// The apply-mode record of [`patch_json`]: the same document with every
/// row tagged `"outcome": "applied" | "stale"` and the document-level
/// `"summary"` counts (`{"applied": N, "stale": M}`) — an all-stale run
/// writes nothing yet stays legible to the machine consumer without
/// scanning rows. `outcomes` is [`apply`]'s answer, one entry per row in
/// this document's row order.
#[must_use]
pub fn patch_json_applied(
    set: &FileFixSet,
    verbosity: Verbosity,
    outcomes: &[EditOutcome],
) -> serde_json::Value {
    patch_document(set, verbosity, Some(outcomes))
}

/// The shared builder behind both faces: `Some(outcomes)` renders the
/// apply record (per-row outcome + summary), `None` the bare proposal.
fn patch_document(
    set: &FileFixSet,
    verbosity: Verbosity,
    outcomes: Option<&[EditOutcome]>,
) -> serde_json::Value {
    let anchored = verbosity != Verbosity::Minimal;
    let mut labels = outcomes.iter().flat_map(|outcomes| outcomes.iter());
    let mut edits = Vec::new();
    for fix in &set.fixes {
        let mut row = edit_floor(&fix.edit, fix.source);
        if anchored {
            row.insert("anchor".into(), anchor_json(&fix.edit.anchor).into());
        }
        // The cross-file row names its created file — the hit file's
        // coordinates never appear for it.
        if let Some(target) = &fix.target {
            row.insert("target".into(), target.clone().into());
        }
        if verbosity == Verbosity::Full {
            row.insert("rule".into(), fix.rule.into());
            row.insert("message".into(), fix.message.clone().into());
        }
        if let Some(outcome) = labels.next() {
            row.insert("outcome".into(), outcome_label(outcome).into());
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
            if let Some(outcome) = labels.next() {
                row.insert("outcome".into(), outcome_label(outcome).into());
            }
            edits.push(serde_json::Value::Object(row));
        }
    }
    let mut document = serde_json::json!({
        "file": set.path,
        "schema_version": SCHEMA_VERSION,
        "edits": edits,
    });
    if let Some(outcomes) = outcomes {
        let applied = outcomes
            .iter()
            .filter(|outcome| **outcome == EditOutcome::Applied)
            .count();
        let summary = serde_json::json!({ "applied": applied, "stale": outcomes.len() - applied });
        document
            .as_object_mut()
            .expect("patch document object")
            .insert("summary".into(), summary);
    }
    document
}

/// The outcome's JSON label.
fn outcome_label(outcome: &EditOutcome) -> &'static str {
    match outcome {
        EditOutcome::Applied => "applied",
        EditOutcome::Stale => "stale",
    }
}

/// The assists document for one file: the plugin changes in priority
/// order, each carrying its provenance (`source`), title, priority, and
/// anchored edits (the anchor drops at the minimal tier); the hosts that
/// could not serve the request answer in `errors` (omitted when empty).
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

/// The provenance tag of every built-in assist row: the location-directed
/// producers below (the mirror of [`BUILTIN_SOURCE`] on the assist face;
/// plugin rows keep the plugin's manifest name).
pub const BUILTIN_ASSIST_SOURCE: &str = "builtin-assist";
