//! The `dartq/*` request faces (PRD §15.1): parameter decoding, execution
//! over the session's engine, and response shaping.
//!
//! Every face mirrors one CLI command and shares its implementation
//! through [`dartq_commands`] — this layer adds only the wire contract:
//!
//! * every response carries `schema_version` (the frozen v1 contract;
//!   dartq-schema's per-domain constants are authoritative);
//! * every request accepts an optional `verbosity` (default compact) whose
//!   tier semantics are the canonical tables in dartq-schema's crate docs —
//!   this module implements exactly those tables and nothing else;
//! * every engine-view request rides the session's read task, so
//!   `$/cancelRequest` cancels it through the epoch machinery like any
//!   other read. The two `dartq/overlay.*` faces answer at intake instead
//!   (they mutate session bookkeeping, never analyze).
//!
//! # Analyzed view per face (PRD §16.3 multi-agent overlays)
//!
//! A request with `agentId` analyzes the agent's overlay view (its staged
//! `path → content` deltas over the base view); a request without one
//! analyzes the base view (on-disk content plus the LSP `didOpen`/
//! `didChange` overlay). The per-face visibility is contract:
//!
//! | face            | view                                            |
//! |-----------------|--------------------------------------------------|
//! | `dartq/analyze` | requested view (base, or the agent's overlay)    |
//! | `dartq/check`   | base: the M0 disk pipeline (scan+parse over the  |
//! |                 | discovered file set). `agentId`: the requested   |
//! |                 | view's engine diagnostics (scan+parse+semantic)  |
//! |                 | restricted to the discovered paths plus the      |
//! |                 | agent's own overlay paths — the two check modes  |
//! |                 | are deliberately different pipelines, and the    |
//! |                 | agent mode is the overlay-aware one              |
//! | `dartq/query`   | requested view                                   |
//! | `dartq/impact`  | requested view                                   |
//! | `dartq/fix`     | disk, always (`agentId`/`since` rejected)        |
//! | `dartq/compat`  | disk, always — it rebuilds one engine per axis   |
//! |                 | over the on-disk base (`agentId`/`since`         |
//! |                 | rejected)                                        |
//!
//! # Session economics (PRD §15.8)
//!
//! * **Delta mode** — a `since: <cursor>` param restricts the answer to
//!   rows whose file changed since that cursor and echoes the session's
//!   current cursor in the top-level `since` field. The cursor is a
//!   per-session monotonic counter bumped once per applied content change
//!   (a base overlay apply batch, an `overlay.set`); the changed set is the
//!   files *explicitly written* — cascade-invalidated dependents are not
//!   marked. For an agent view, an overlaid path's change cursor is the
//!   `overlay.set` that staged it (the overlay masks base changes).
//! * **Short-id symbol table** — full-tier `dartq/check` and `dartq/analyze`
//!   answers substitute path-valued row fields (`file`/`uri`) with
//!   session-stable short ids (`s1`, `s2`, …) and carry a `symbols` preamble
//!   (id → path) for ids the client has not seen yet, only when the
//!   substitution saves bytes. Compact answers stay byte-identical to the
//!   pre-economics shape. `dartq/query` and `dartq/impact` keep their paths
//!   spelled out: their published `symbols` field is contract-pinned
//!   (dartq-schema's validator) to `label|name` symbol references — a
//!   different value domain than the file paths these rows repeat — so
//!   emission there waits for the schema/index lanes' symbol-path rows; the
//!   field stays absent-tolerant until then.
//!
//! # Path labels
//!
//! The CLI-mirror faces (`check`, `query`, `impact`, `fix`, `compat`) keep
//! the CLI's canonical path labels; the LSP-native `dartq/analyze` keeps
//! document URIs. Relative request paths resolve against the session's
//! workspace root (the CLI resolves them against the process cwd). The
//! CLI's `$DART_SDK` fallback has no daemon equivalent: `dartq/compat`
//! requires explicit `sdk_roots`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use dartq_commands::analyze::AnalyzeError;
use dartq_commands::compat::CompatError;
use dartq_commands::discover::{self, DiscoverError};
use dartq_commands::fix::{FileFixes, SCHEMA_VERSION as FIX_SCHEMA_VERSION};
use dartq_commands::layout::EngineLayout;
use dartq_commands::query::{decl_kind_name, ref_kind_name};
use dartq_commands::{Verbosity, check, compat as compat_core, fix, impact as impact_core, query};
use dartq_diagnostics::{Diagnostic, FileLayoutProvider};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::lsp;
use crate::session::SessionError;

/// The response contract version: the frozen v1, mirrored from
/// [`dartq_commands::SCHEMA_VERSION`] (dartq-schema's constants are
/// authoritative).
pub(crate) const SCHEMA_VERSION: u32 = dartq_commands::SCHEMA_VERSION;

/// The `dartq/*` method names, announced under `capabilities.experimental`
/// in `initialize`'s answer. The two `dartq/overlay.*` faces answer at
/// intake (the server intercepts them before the scheduler queue); the rest
/// ride the session's read task.
pub(crate) const METHODS: [&str; 8] = [
    "dartq/analyze",
    "dartq/check",
    "dartq/query",
    "dartq/impact",
    "dartq/fix",
    "dartq/compat",
    "dartq/overlay.set",
    "dartq/overlay.clear",
];

/// A staged `dartq/*` request: decoded parameters. The request id — the
/// identity `$/cancelRequest` keys on — stays on the session side.
#[derive(Debug, Clone)]
pub(crate) enum Request {
    Analyze {
        verbosity: Verbosity,
        agent: Option<String>,
    },
    Check {
        paths: Vec<PathBuf>,
        verbosity: Verbosity,
        agent: Option<String>,
        since: Option<u64>,
    },
    Query {
        name: String,
        verbosity: Verbosity,
        agent: Option<String>,
        since: Option<u64>,
    },
    Impact {
        target: PathBuf,
        verbosity: Verbosity,
        agent: Option<String>,
        since: Option<u64>,
    },
    Fix {
        paths: Vec<PathBuf>,
        verbosity: Verbosity,
    },
    Compat {
        target: PathBuf,
        sdk_roots: Vec<PathBuf>,
        verbosity: Verbosity,
    },
}

impl Request {
    /// The request's `agentId`, if it named one. The server validates the
    /// agent exists before the request queues (fail loud on a typo instead
    /// of silently answering the base view).
    pub(crate) fn agent(&self) -> Option<&str> {
        match self {
            Self::Analyze { agent, .. }
            | Self::Check { agent, .. }
            | Self::Query { agent, .. }
            | Self::Impact { agent, .. } => agent.as_deref(),
            Self::Fix { .. } | Self::Compat { .. } => None,
        }
    }
}

/// Whether `method` is one of the `dartq/*` faces this module serves.
#[must_use]
pub(crate) fn is_command(method: &str) -> bool {
    METHODS.contains(&method)
}

/// Decodes one `dartq/*` request's parameters. Relative paths resolve
/// against `root`; `Err` is the `INVALID_PARAMS` message.
///
/// # Errors
///
/// A structural mismatch (wrong type, missing field, unknown `verbosity`)
/// or an empty `paths`/`sdk_roots` array.
pub(crate) fn parse(method: &str, params: &Value, root: &Path) -> Result<Request, String> {
    #[derive(Deserialize)]
    struct VerbosityParams {
        #[serde(default)]
        verbosity: Verbosity,
    }
    #[derive(Deserialize)]
    struct CheckParams {
        paths: Vec<PathBuf>,
        #[serde(default)]
        verbosity: Verbosity,
    }
    #[derive(Deserialize)]
    struct QueryParams {
        name: String,
        #[serde(default)]
        verbosity: Verbosity,
    }
    #[derive(Deserialize)]
    struct TargetParams {
        path: PathBuf,
        #[serde(default)]
        verbosity: Verbosity,
    }
    #[derive(Deserialize)]
    struct CompatParams {
        path: PathBuf,
        sdk_roots: Vec<PathBuf>,
        #[serde(default)]
        verbosity: Verbosity,
    }

    let invalid = |error: serde_json::Error| error.to_string();
    match method {
        "dartq/analyze" => {
            let (agent, _) = session_fields(params)?;
            let decoded =
                serde_json::from_value::<VerbosityParams>(params.clone()).map_err(invalid)?;
            Ok(Request::Analyze {
                verbosity: decoded.verbosity,
                agent,
            })
        }
        "dartq/check" | "dartq/fix" => {
            let decoded = serde_json::from_value::<CheckParams>(params.clone()).map_err(invalid)?;
            if decoded.paths.is_empty() {
                return Err("paths must be a non-empty array".into());
            }
            let paths = decoded
                .paths
                .into_iter()
                .map(|path| resolve(root, path))
                .collect();
            if method == "dartq/check" {
                let (agent, since) = session_fields(params)?;
                Ok(Request::Check {
                    paths,
                    verbosity: decoded.verbosity,
                    agent,
                    since,
                })
            } else {
                reject_disk_view_only(params, "dartq/fix")?;
                Ok(Request::Fix {
                    paths,
                    verbosity: decoded.verbosity,
                })
            }
        }
        "dartq/query" => {
            let (agent, since) = session_fields(params)?;
            let decoded = serde_json::from_value::<QueryParams>(params.clone()).map_err(invalid)?;
            Ok(Request::Query {
                name: decoded.name,
                verbosity: decoded.verbosity,
                agent,
                since,
            })
        }
        "dartq/impact" => {
            let (agent, since) = session_fields(params)?;
            let decoded =
                serde_json::from_value::<TargetParams>(params.clone()).map_err(invalid)?;
            Ok(Request::Impact {
                target: resolve(root, decoded.path),
                verbosity: decoded.verbosity,
                agent,
                since,
            })
        }
        "dartq/compat" => {
            let decoded =
                serde_json::from_value::<CompatParams>(params.clone()).map_err(invalid)?;
            if decoded.sdk_roots.is_empty() {
                return Err(
                    "sdk_roots must name at least one SDK root (the CLI's $DART_SDK default \
                     axis has no daemon equivalent)"
                        .into(),
                );
            }
            reject_disk_view_only(params, "dartq/compat")?;
            Ok(Request::Compat {
                target: resolve(root, decoded.path),
                sdk_roots: decoded.sdk_roots,
                verbosity: decoded.verbosity,
            })
        }
        _ => Err(format!("not a dartq command: {method}")),
    }
}

/// The params' session-economics fields: the optional `agentId` view selector
/// and the optional `since` delta cursor (PRD §15.8). Absent and `null` are
/// equivalent.
fn session_fields(params: &Value) -> Result<(Option<String>, Option<u64>), String> {
    let agent = match params.get("agentId") {
        None | Some(Value::Null) => None,
        Some(Value::String(agent)) => Some(agent.clone()),
        Some(_) => return Err("agentId must be a string".into()),
    };
    let since = match params.get("since") {
        None | Some(Value::Null) => None,
        Some(Value::Number(number)) => Some(
            number
                .as_u64()
                .ok_or("since must be a non-negative integer (the session's delta cursor)")?,
        ),
        Some(_) => {
            return Err("since must be a non-negative integer (the session's delta cursor)".into());
        }
    };
    Ok((agent, since))
}

/// The disk-view faces take neither session field: an `agentId` or `since`
/// there would silently answer a different view than the face's contract.
fn reject_disk_view_only(params: &Value, method: &str) -> Result<(), String> {
    let (agent, since) = session_fields(params)?;
    if agent.is_some() || since.is_some() {
        return Err(format!(
            "{method} answers the disk view; agentId/since apply to the engine-view faces \
             (analyze/check/query/impact)"
        ));
    }
    Ok(())
}

/// Decodes `dartq/overlay.set` params: `{agentId, files: {path → content}}`.
/// Relative paths resolve against `root`; contents are whole-file
/// replacement text (PRD §16.3, M1 semantics).
///
/// # Errors
///
/// A missing or non-string `agentId`, a missing or non-object `files`, or a
/// non-string path or content.
pub(crate) fn parse_overlay_set(
    params: &Value,
    root: &Path,
) -> Result<(String, Vec<(PathBuf, Vec<u8>)>), String> {
    let agent = match params.get("agentId") {
        Some(Value::String(agent)) if !agent.is_empty() => agent.clone(),
        Some(_) => return Err("agentId must be a non-empty string".into()),
        None => return Err("overlay.set requires an agentId".into()),
    };
    let files = match params.get("files") {
        Some(Value::Object(files)) => files,
        Some(_) => return Err("files must be an object mapping path to content".into()),
        None => return Err("overlay.set requires files".into()),
    };
    let mut staged = Vec::with_capacity(files.len());
    for (path, content) in files {
        let Value::String(content) = content else {
            return Err(format!(
                "files[{path}] must be a string (whole-file content)"
            ));
        };
        staged.push((
            resolve(root, PathBuf::from(path)),
            content.as_bytes().to_vec(),
        ));
    }
    Ok((agent, staged))
}

/// Decodes `dartq/overlay.clear` params: `{agentId}`. Clearing an unknown
/// agent is answered ok by the caller (idempotent teardown).
///
/// # Errors
///
/// A missing or non-string `agentId`.
pub(crate) fn parse_overlay_clear(params: &Value) -> Result<String, String> {
    match params.get("agentId") {
        Some(Value::String(agent)) if !agent.is_empty() => Ok(agent.clone()),
        Some(_) => Err("agentId must be a non-empty string".into()),
        None => Err("overlay.clear requires an agentId".into()),
    }
}

/// Relative request paths resolve against the session workspace root.
fn resolve(root: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

/// The analyzed view's changed-file cursors, for `since` delta filtering.
///
/// The base table is the session's (paths explicitly written through the
/// base overlay); the agent list carries the `overlay.set` cursors of the
/// analyzed agent's own files — an overlay masks base changes to the same
/// path, so an overlaid path's cursor is solely its staging cursor.
pub(crate) struct Changes<'a> {
    base: &'a FxHashMap<PathBuf, u64>,
    agent: Option<Vec<(PathBuf, u64)>>,
}

impl<'a> Changes<'a> {
    /// The analyzed view's changed-file table: the session's base table plus
    /// the analyzed agent's staging cursors (`None` for base-view groups).
    pub(crate) fn new(
        base: &'a FxHashMap<PathBuf, u64>,
        agent: Option<Vec<(PathBuf, u64)>>,
    ) -> Self {
        Self { base, agent }
    }

    /// Whether `path` changed in this view after `since`.
    pub(crate) fn changed_since(&self, path: &Path, since: u64) -> bool {
        let cursor = if let Some(agent) = &self.agent {
            agent
                .iter()
                .find(|(overlaid, _)| overlaid == path)
                .map(|(_, at)| *at)
                .or_else(|| self.base.get(path).copied())
        } else {
            self.base.get(path).copied()
        };
        cursor.unwrap_or(0) > since
    }
}

/// The session's stable path → short-id table (PRD §15.8.2): ids (`s1`,
/// `s2`, …) are assigned on first payable use and stay stable for the
/// session; `announced` remembers which ids the client has already seen a
/// `symbols` preamble for, so later answers back-reference without
/// re-announcing.
#[derive(Default)]
pub(crate) struct SymbolTable {
    ids: FxHashMap<String, String>,
    announced: FxHashSet<String>,
    next: u32,
}

/// Conservative per-entry cost of a `symbols` preamble entry
/// (`"sN":"<path>",` plus the object's own key/brace overhead folded in):
/// a new id must beat this plus the first reference's non-saving to pay.
const SYMBOL_ENTRY_COST: usize = 10;

impl SymbolTable {
    /// Substitutes short ids for path-valued row fields when that saves
    /// bytes. `spots` names the `(row array field, path field)` pairs of one
    /// answer's shape. Returns whether any substitution landed (the caller
    /// does nothing more — the `symbols` preamble is attached here).
    pub(crate) fn shorten(
        &mut self,
        answer: &mut Value,
        spots: &'static [(&'static str, &'static str)],
    ) -> bool {
        // Occurrence counts in first-seen order (deterministic id assignment).
        let mut order: Vec<String> = Vec::new();
        let mut counts: FxHashMap<String, usize> = FxHashMap::default();
        for (array_field, path_field) in spots {
            let Some(rows) = answer[*array_field].as_array() else {
                continue;
            };
            for row in rows {
                let Some(path) = row[*path_field].as_str() else {
                    continue;
                };
                *counts.entry(path.to_owned()).or_insert(0) += 1;
                if !order.iter().any(|seen| seen == path) {
                    order.push(path.to_owned());
                }
            }
        }

        // Plan: a known, already-announced id always pays when the path is
        // longer than the id; anything else (a new id, or a known id the
        // client never saw a preamble for) must beat its preamble cost.
        let mut substitutions: Vec<(String, String)> = Vec::new(); // (path, id)
        let mut announce: Vec<(String, String)> = Vec::new(); // preamble entries
        for path in &order {
            let count = counts[path];
            let cost = |id: &str| id.len() + path.len() + SYMBOL_ENTRY_COST;
            // Commits happen inline in plan order, so the next fresh id is
            // always `next + 1`.
            let id = match self.ids.get(path) {
                Some(id) if self.announced.contains(id) => {
                    if path.len() > id.len() {
                        id.clone()
                    } else {
                        continue;
                    }
                }
                Some(id) => {
                    let savings = count * path.len().saturating_sub(id.len());
                    if savings <= cost(id) {
                        continue;
                    }
                    id.clone()
                }
                None => {
                    let candidate = format!("s{}", self.next + 1);
                    let savings = count * path.len().saturating_sub(candidate.len());
                    if savings <= cost(&candidate) {
                        continue;
                    }
                    self.ids.insert(path.clone(), candidate.clone());
                    self.next += 1;
                    candidate
                }
            };
            if self.announced.insert(id.clone()) {
                announce.push((id.clone(), path.clone()));
            }
            substitutions.push((path.clone(), id));
        }
        if substitutions.is_empty() {
            return false;
        }

        // Substitute, then attach the preamble for the newly announced ids.
        let lookup: FxHashMap<&str, &str> = substitutions
            .iter()
            .map(|(path, id)| (path.as_str(), id.as_str()))
            .collect();
        for (array_field, path_field) in spots {
            let Some(rows) = answer[*array_field].as_array_mut() else {
                continue;
            };
            for row in rows {
                if let Some(path) = row[*path_field].as_str() {
                    if let Some(id) = lookup.get(path) {
                        row[*path_field] = Value::String((*id).to_owned());
                    }
                }
            }
        }
        if !announce.is_empty() {
            let mut preamble = serde_json::Map::new();
            for (id, path) in announce {
                preamble.insert(id, Value::String(path));
            }
            answer["symbols"] = Value::Object(preamble);
        }
        true
    }
}

/// Everything an answer needs from the session at drain time.
pub(crate) struct Drain<'a> {
    /// The session's analyzed engine — already wearing the requesting
    /// group's view (base, or the agent's overlay); the read task has just
    /// run that view's analysis pass.
    pub engine: &'a dartq_engine::Engine,
    /// The session workspace root (relative-path resolution).
    pub root: &'a Path,
    /// The analyzed view's raw diagnostic set (engine-canonical order).
    pub diagnostics: &'a [dartq_diagnostics::Diagnostic],
    /// The shared analyze entries (uri + converted LSP diagnostic per
    /// diagnostic of this view), consumed by `dartq/analyze` answers.
    pub entries: &'a [lsp::AnalyzeEntry],
    /// The analyzed view's changed-file table for `since` filtering.
    pub changes: &'a Changes<'a>,
    /// The session's current delta cursor, echoed by delta answers.
    pub cursor: u64,
    /// The requesting agent's overlay paths (`dartq/check`'s engine view
    /// answers over the discovered set plus these; empty for base groups).
    pub overlay_paths: &'a [PathBuf],
    /// The session symbol table (short-id economics, full tier).
    pub symbols: &'a mut SymbolTable,
}

/// Answers one staged request at the drain. `&mut` only for the symbol
/// table's economics; everything else reads.
pub(crate) fn answer(drain: &mut Drain<'_>, request: &Request) -> Result<Value, SessionError> {
    match request {
        Request::Analyze { verbosity, .. } => Ok(analyze_answer(drain, *verbosity)),
        Request::Check {
            paths,
            verbosity,
            agent,
            since,
        } => match agent {
            Some(_) => check_engine_view(drain, paths, *verbosity, *since),
            None => check_disk_view(drain, paths, *verbosity, *since),
        },
        Request::Query {
            name,
            verbosity,
            since,
            ..
        } => Ok(query_answer(drain, name, *verbosity, *since)),
        Request::Impact {
            target,
            verbosity,
            since,
            ..
        } => impact_answer(drain, target, *verbosity, *since),
        Request::Fix { paths, verbosity } => fix_answer(paths, *verbosity),
        Request::Compat {
            target,
            sdk_roots,
            verbosity,
        } => compat_answer(drain.root, target, sdk_roots, *verbosity),
    }
}

/// Tier wiring shared by the row-shaped faces: full tier applies the
/// short-id economics over `spots`; a delta answer echoes the session's
/// current cursor (the value the client's next `since` chains from).
fn session_answer(
    mut answer: Value,
    verbosity: Verbosity,
    since: Option<u64>,
    drain: &mut Drain<'_>,
    spots: &'static [(&'static str, &'static str)],
) -> Value {
    if verbosity == Verbosity::Full {
        drain.symbols.shorten(&mut answer, spots);
    }
    if since.is_some() {
        answer["since"] = json!(drain.cursor);
    }
    answer
}

/// The `since` row filter: without a cursor every row stays; with one, a row
/// stays when its file changed in the analyzed view since the cursor.
fn delta_row(
    files: &dyn FileLayoutProvider,
    file: dartq_diagnostics::FileId,
    changes: &Changes<'_>,
    since: Option<u64>,
) -> bool {
    match since {
        None => true,
        Some(since) => changes.changed_since(&files.file_path(file), since),
    }
}

// ---------------------------------------------------------------------------
// dartq/analyze — the LSP-native pull face
// ---------------------------------------------------------------------------

/// The merged diagnostic set of the analyzed view. Compact keeps the
/// entry shape (uri + LSP diagnostic); minimal keeps the red line (code +
/// location, here `uri` + `range`); full equals compact plus the short-id
/// economics over entry URIs.
pub(crate) fn analyze_answer(drain: &mut Drain<'_>, verbosity: Verbosity) -> Value {
    let diagnostics: Vec<Value> = drain
        .entries
        .iter()
        .map(|entry| match verbosity {
            Verbosity::Minimal => json!({
                "uri": entry.uri,
                "code": entry.diagnostic.code,
                "range": entry.diagnostic.range,
            }),
            Verbosity::Compact | Verbosity::Full => {
                serde_json::to_value(entry).expect("analyze entry serializes")
            }
        })
        .collect();
    session_answer(
        json!({ "schema_version": SCHEMA_VERSION, "diagnostics": diagnostics }),
        verbosity,
        None,
        drain,
        &[("diagnostics", "uri")],
    )
}

// ---------------------------------------------------------------------------
// dartq/check — base: the M0 disk pipeline; agentId: the overlay view's
// engine diagnostics (the module doc's visibility table is the contract)
// ---------------------------------------------------------------------------

/// One check row. Compact mirrors the machine face's field set (severity /
/// type / code / file / position / length / message, code in dartq's
/// stable snake_case name spelling); minimal keeps the red line (`code` +
/// location); full equals compact today.
fn check_row(
    diagnostic: &Diagnostic,
    files: &dyn FileLayoutProvider,
    verbosity: Verbosity,
) -> Value {
    let (line, column) = files.line_col(diagnostic.file, diagnostic.range.offset);
    let file = files.file_path(diagnostic.file).display().to_string();
    match verbosity {
        Verbosity::Minimal => json!({
            "file": file,
            "code": diagnostic.code.name,
            "line": line,
            "column": column,
            "length": diagnostic.range.length,
        }),
        Verbosity::Compact | Verbosity::Full => json!({
            "file": file,
            "severity": diagnostic.severity.as_machine_str(),
            "type": diagnostic.code.dtype.as_str(),
            "code": diagnostic.code.name,
            "line": line,
            "column": column,
            "length": diagnostic.range.length,
            "message": diagnostic.message,
        }),
    }
}

/// The base view's check answer: the M0 disk pipeline (scan+parse over the
/// discovered file set), rows filtered by the delta cursor when one came.
fn check_disk_view(
    drain: &mut Drain<'_>,
    paths: &[PathBuf],
    verbosity: Verbosity,
    since: Option<u64>,
) -> Result<Value, SessionError> {
    let files: BTreeSet<PathBuf> = discover::discover(paths).map_err(|error| match error {
        DiscoverError::Missing(path) => {
            SessionError::InvalidParams(format!("Directory or file doesn't exist: {path}"))
        }
        DiscoverError::Io(io) => SessionError::Io(io),
    })?;
    let run = check::analyze_files(&files).map_err(SessionError::Io)?;
    let output = run.output();
    let diagnostics: Vec<Value> = output
        .diagnostics
        .iter()
        .filter(|diagnostic| delta_row(output.files, diagnostic.file, drain.changes, since))
        .map(|diagnostic| check_row(diagnostic, output.files, verbosity))
        .collect();
    Ok(session_answer(
        json!({ "schema_version": SCHEMA_VERSION, "diagnostics": diagnostics }),
        verbosity,
        since,
        drain,
        &[("diagnostics", "file")],
    ))
}

/// The agent view's check answer: the analyzed overlay view's engine
/// diagnostics (scan+parse+semantic — a different pipeline than the disk
/// mode above; the visibility table in the module docs pins this), rows
/// restricted to the discovered paths plus the agent's own overlay paths
/// (agent-introduced files are not on disk to discover).
fn check_engine_view(
    drain: &mut Drain<'_>,
    paths: &[PathBuf],
    verbosity: Verbosity,
    since: Option<u64>,
) -> Result<Value, SessionError> {
    let mut allow: BTreeSet<PathBuf> = discover::discover(paths).map_err(|error| match error {
        DiscoverError::Missing(path) => {
            SessionError::InvalidParams(format!("Directory or file doesn't exist: {path}"))
        }
        DiscoverError::Io(io) => SessionError::Io(io),
    })?;
    allow.extend(drain.overlay_paths.iter().cloned());
    let layout = EngineLayout::over(drain.engine, drain.diagnostics.iter().map(|d| d.file));
    let diagnostics: Vec<Value> = drain
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            allow.contains(layout.file_path(diagnostic.file))
                && delta_row(&layout, diagnostic.file, drain.changes, since)
        })
        .map(|diagnostic| check_row(diagnostic, &layout, verbosity))
        .collect();
    Ok(session_answer(
        json!({ "schema_version": SCHEMA_VERSION, "diagnostics": diagnostics }),
        verbosity,
        since,
        drain,
        &[("diagnostics", "file")],
    ))
}

// ---------------------------------------------------------------------------
// dartq/query — the symbol index face (CLI path labels)
// ---------------------------------------------------------------------------

fn query_answer(
    drain: &mut Drain<'_>,
    name: &str,
    verbosity: Verbosity,
    since: Option<u64>,
) -> Value {
    let mut run = query::symbol_after_analysis(drain.engine, name);
    if let Some(since) = since {
        run.decls.retain(|decl| {
            drain
                .changes
                .changed_since(run.layout.file_path(decl.file), since)
        });
        run.refs.retain(|reference| {
            drain
                .changes
                .changed_since(run.layout.file_path(reference.target_file), since)
        });
    }
    match verbosity {
        // Canonical query table: minimal drops the convenience fields
        // (declarations[].container, references[].qualified); the row
        // locators stay.
        Verbosity::Minimal => {
            let declarations: Vec<Value> = run
                .decls
                .iter()
                .map(|decl| {
                    let (line, column) = run.layout.line_col(decl.file, decl.range.offset);
                    json!({
                        "file": run.layout.file_path(decl.file).display().to_string(),
                        "kind": decl_kind_name(decl.kind),
                        "name": decl.name,
                        "line": line,
                        "column": column,
                        "length": decl.range.length,
                    })
                })
                .collect();
            let references: Vec<Value> = run
                .refs
                .iter()
                .map(|reference| {
                    json!({
                        "target_file": run.layout.file_path(reference.target_file).display().to_string(),
                        "kind": ref_kind_name(reference.kind),
                        "offset": reference.range.offset,
                        "length": reference.range.length,
                    })
                })
                .collect();
            let mut answer = json!({
                "schema_version": SCHEMA_VERSION,
                "name": name,
                "declarations": declarations,
                "references": references,
            });
            if since.is_some() {
                answer["since"] = json!(drain.cursor);
            }
            answer
        }
        // Compact is the CLI JSON face verbatim (the shared shaper, which
        // stamps `schema_version`); full adds the short-id economics. The
        // canonical table lists no full-only query fields besides the
        // session ones.
        // Compact is the CLI JSON face verbatim (the shared shaper, which
        // stamps `schema_version`) plus the delta echo when one came. The
        // rows' paths stay spelled out — see the module docs' symbol-table
        // note (the published `symbols` field pins `label|name` values).
        Verbosity::Compact | Verbosity::Full => {
            session_answer(query::to_json(&run, name), verbosity, since, drain, &[])
        }
    }
}

// ---------------------------------------------------------------------------
// dartq/impact — the §15.4 report (tier-invariant per the canonical tables)
// ---------------------------------------------------------------------------

fn impact_answer(
    drain: &mut Drain<'_>,
    target: &Path,
    _verbosity: Verbosity,
    since: Option<u64>,
) -> Result<Value, SessionError> {
    let mut report =
        impact_core::impact_after_analysis(drain.engine, target).map_err(|error| match error {
            impact_core::ImpactError::Missing(path) => SessionError::InvalidParams(format!(
                "Directory or file doesn't exist: {}",
                path.display()
            )),
            impact_core::ImpactError::NotAnalyzed(path) => SessionError::InvalidParams(format!(
                "not an analyzed .dart file: {}",
                path.display()
            )),
            impact_core::ImpactError::Step(step) => step.into(),
        })?;
    if let Some(since) = since {
        // Delta mode: affected rows and test rows carry their file path
        // verbatim; changed rows carry the stable `label|name` symbol form
        // whose label is the defining file's path.
        let changed = |path: &str| drain.changes.changed_since(Path::new(path), since);
        report.affected.retain(|row| changed(&row.path));
        report.affected_tests.retain(|row| changed(&row.path));
        report.changed.retain(|row| {
            let label = row
                .symbol
                .split_once('|')
                .map_or(row.symbol.as_str(), |(label, _)| label);
            changed(label)
        });
    }
    // Tier-invariant: every report field is a locator or a verdict — the
    // canonical table trims nothing (the report carries its own
    // `schema_version`). Its library labels stay unsubstituted: each row
    // names a distinct library, so short ids save nothing.
    let mut value = serde_json::to_value(&report).expect("ImpactReport serializes");
    if since.is_some() {
        value["since"] = json!(drain.cursor);
    }
    Ok(value)
}

// ---------------------------------------------------------------------------
// dartq/fix — the §15.6 patch document (CLI path labels)
// ---------------------------------------------------------------------------

fn fix_answer(paths: &[PathBuf], verbosity: Verbosity) -> Result<Value, SessionError> {
    let per_file: Vec<FileFixes> = fix::collect(paths).map_err(|error| match error {
        DiscoverError::Missing(path) => {
            SessionError::InvalidParams(format!("Directory or file doesn't exist: {path}"))
        }
        DiscoverError::Io(io) => SessionError::Io(io),
    })?;
    let files: Vec<Value> = per_file
        .iter()
        .filter(|fixes| !fixes.fixes.is_empty())
        .map(|fixes| match verbosity {
            // Minimal: file + offset/length/replacement — the red line; the
            // anchor drops.
            Verbosity::Minimal => {
                let edits: Vec<Value> = fixes
                    .fixes
                    .iter()
                    .map(|row| {
                        json!({
                            "offset": row.edit.offset,
                            "length": row.edit.length,
                            "replacement": row.edit.replacement,
                        })
                    })
                    .collect();
                json!({
                    "file": fixes.path,
                    "schema_version": FIX_SCHEMA_VERSION,
                    "edits": edits,
                })
            }
            // Compact is the §15.6 machine document verbatim.
            Verbosity::Compact => fix::patch_json(fixes),
            // Full adds the per-edit rule (cheaply available on the row;
            // declared absent-tolerant in the canonical table).
            Verbosity::Full => {
                let mut document = fix::patch_json(fixes);
                let edits = document["edits"]
                    .as_array_mut()
                    .expect("§15.6 document carries an edits array");
                for (edit, row) in edits.iter_mut().zip(&fixes.fixes) {
                    edit["rule"] = row.rule.into();
                }
                document
            }
        })
        .collect();
    // The daemon face is dry by contract: it reports patches, never writes.
    Ok(json!({ "schema_version": SCHEMA_VERSION, "files": files }))
}

// ---------------------------------------------------------------------------
// dartq/compat — the §15.5 matrix (CLI path labels)
// ---------------------------------------------------------------------------

fn compat_answer(
    root: &Path,
    target: &Path,
    sdk_roots: &[PathBuf],
    verbosity: Verbosity,
) -> Result<Value, SessionError> {
    let report = compat_core::compat(root, target, sdk_roots).map_err(|error| match error {
        CompatError::Usage(message) => SessionError::InvalidParams(message),
        CompatError::Internal(message) => SessionError::Internal(message),
    })?;
    let mut value = serde_json::to_value(&report).expect("CompatReport serializes");
    if verbosity == Verbosity::Minimal {
        // Canonical compat table: minimal keeps the verdict + axis identity;
        // the evidence rows (first_failure / error) drop.
        for axis in value["matrix"]
            .as_array_mut()
            .expect("CompatReport carries a matrix")
        {
            if let Some(axis) = axis.as_object_mut() {
                axis.remove("first_failure");
                axis.remove("error");
            }
        }
    }
    Ok(value)
}

impl From<AnalyzeError> for SessionError {
    fn from(error: AnalyzeError) -> Self {
        match error {
            AnalyzeError::Discovery(io) => Self::Io(io),
            AnalyzeError::Cancelled => Self::Cancelled,
        }
    }
}
