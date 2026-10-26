//! The `dartq/*` request faces: parameter decoding, execution
//! over the session's engine, and response shaping.
//!
//! Every face mirrors one CLI command and shares its implementation
//! through [`dartq_commands`] — this layer adds only the wire contract:
//!
//! * every response carries `v` (the frozen v1 contract version;
//!   dartq-schema's per-domain constants are authoritative);
//! * every request accepts an optional `verbosity` (default compact) whose
//!   tier semantics are the canonical tables in dartq-schema's crate docs —
//!   this module implements exactly those tables and nothing else;
//! * every engine-view request rides the session's read task, so
//!   `dartq/cancel` cancels it through the epoch machinery like any
//!   other read. The three `dartq/overlay.*` faces answer at intake instead
//!   (they mutate session bookkeeping or the worktree, never analyze).
//!
//! # Context routing
//!
//! The engine-view faces accept an optional `context` tag naming a declared
//! context (a `dartq.yaml` row assembled at the shell edge); the request
//! analyzes that context's engine. Without one they analyze the default
//! context. The disk-view faces take neither `context` nor `agent_id` —
//! they never touch a session engine view.
//!
//! # Analyzed view per face
//!
//! A request with `agent_id` analyzes the agent's overlay view (its staged
//! `path → content` deltas over the base view); a request without one
//! analyzes the base view (on-disk content as `dartq/sync` and the watch
//! syncs staged it). The per-face visibility is contract:
//!
//! | face            | view                                            |
//! |-----------------|--------------------------------------------------|
//! | `dartq/index`   | requested view — the analyzed view's resident    |
//! |                 | index tallied (files/libraries/symbols/referenc  |
//! |                 | es); the read task has just analyzed, so the     |
//! |                 | tallies read the maintained state with no scan,  |
//! |                 | no parse, no second capture                      |
//! | `dartq/analyze` | requested view (base, or the agent's overlay)    |
//! | `dartq/check`   | base: the M0 disk pipeline (scan+parse over the  |
//! |                 | discovered file set). `agent_id`: the requested   |
//! |                 | view's engine diagnostics (scan+parse+semantic)  |
//! |                 | restricted to the discovered paths plus the      |
//! |                 | agent's own overlay paths — the two check modes  |
//! |                 | are deliberately different pipelines, and the    |
//! |                 | agent mode is the overlay-aware one              |
//! | `dartq/query`   | requested view                                   |
//! | `dartq/queryBatch` | requested view (one graph walk per batch —     |
//! |                 | the amortization across the round trip)    |
//! | `dartq/resolveAt` | requested view (a point answer over the        |
//! |                 | resident index — no rows for a delta cursor     |
//! |                 | to filter)                                |
//! | `dartq/completion` | requested view (the visible-name set over   |
//! |                 | the resident index — the file's own library   |
//! |                 | faces, its proven import targets, its import |
//! |                 | prefixes; no rows for a delta cursor to      |
//! |                 | filter)                                   |
//! | `dartq/impact`  | requested view                                   |
//! | `dartq/fix`     | disk, always (`agent_id`/`since` rejected)        |
//! | `dartq/compat`  | disk, always — it rebuilds one engine per axis   |
//! |                 | over the on-disk base (`agent_id`/`since`         |
//! |                 | rejected)                                        |
//! | `dartq/explain` | disk, always — it rebuilds one engine over the   |
//! |                 | on-disk base per query (the `dartq/compat`       |
//! |                 | posture: the report's harvest rides the semantic |
//! |                 | fan-out, which the session engine runs over the  |
//! |                 | recomputed libraries only, so a warm session     |
//! |                 | answer would silently miss declarations).        |
//! |                 | `agent_id` is rejected; `since` applies as delta  |
//! |                 | economics — it filters the report's              |
//! |                 | use/import/test rows by the session's            |
//! |                 | written-file cursors, it does not select a view  |
//!
//! # Session economics
//!
//! * **Delta mode** — a `since: <cursor>` param restricts the answer to
//!   rows whose file changed since that cursor and echoes the session's
//!   current cursor in the top-level `since` field. The cursor is a
//!   per-session monotonic counter bumped once per applied content change
//!   (a base overlay apply batch, an `overlay.set`); the changed set is the
//!   files *explicitly written* — cascade-invalidated dependents are not
//!   marked. For an agent view, an overlaid path's change cursor is the
//!   `overlay.set` that staged it (the overlay masks base changes).
//! * **Batch queries** — `dartq/queryBatch` answers every item's
//!   kinds in one round trip over the session's already-built graph: one
//!   frozen v1 query envelope per `(item, kind)` pair, request order, no
//!   aggregate envelope (the schema crate's frozen batch convention). The
//!   session selectors (`agent_id`/`context`) and `since` apply to the whole
//!   batch, exactly as they do to the single face.
//! * **Short-id symbol table** — full-tier `dartq/check` and `dartq/analyze`
//!   answers substitute path-valued row fields (`file`, the analyze rows'
//!   document URIs included) with
//!   session-stable short ids (`s1`, `s2`, …) and carry a `symbols` preamble
//!   (id → path) for ids the client has not seen yet, only when the
//!   substitution saves bytes. Compact answers stay byte-identical to the
//!   pre-economics shape. `dartq/query` and `dartq/impact` keep their paths
//!   spelled out: their published `symbols` field is contract-pinned
//!   (dartq-schema's validator) to `label|name` symbol references — a
//!   different value domain than the file paths these rows repeat — so
//!   emission there waits for the schema/index lanes' symbol-path rows; the
//!   field stays absent-tolerant until then. Full-tier `dartq/explain`
//!   substitutes its path-valued report rows the same way: the schema
//!   declares `symbols` as the id → path table, absent-tolerant, so the
//!   economics apply verbatim (compact explains stay byte-identical to the
//!   CLI's report).
//!
//! # Path labels
//!
//! The CLI-mirror faces (`check`, `query`, `impact`, `explain`, `fix`,
//! `compat`) shape through the CLI faces' own implementations — the query
//! rows' file references are the shared shaper's stable package-form
//! labels (`dartq_commands::pkg_label`, tier-agnostic), the `analyze`
//! face keeps document URIs. Relative request paths resolve
//! against the session's workspace root (the CLI resolves them against the
//! process cwd). The CLI's `$DART_SDK` fallback has no daemon equivalent:
//! `dartq/compat` requires explicit `sdk_roots`, and `dartq/explain` takes
//! an optional `sdk` root per query — absent resolves to the session's own
//! SDK root (the SDK the analyzed view was assembled against); the
//! `$DART_SDK` fallback stays the CLI path's business (the formatting
//! subprocess bridges honor the same variable as their own override — the
//! `formatting` module's contract).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use dartq_commands::analyze::AnalyzeError;
use dartq_commands::compat::CompatError;
use dartq_commands::discover::{self, DiscoverError};
use dartq_commands::explain::{ExplainError, explain as explain_core};
use dartq_commands::fix::FileFixSet;
use dartq_commands::layout::EngineLayout;
use dartq_commands::query::{BatchQuery, QueryKind};
use dartq_commands::resolve as resolve_core;
use dartq_commands::resolve::ResolveError;
use dartq_commands::{Verbosity, check, compat as compat_core, fix, impact as impact_core, query};
use dartq_diagnostics::{Diagnostic, FileLayoutProvider};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::session::{SessionError, StagedFile};
use crate::wire;

/// The response contract version: the frozen v1, mirrored from
/// [`dartq_commands::SCHEMA_VERSION`] (dartq-schema's constants are
/// authoritative).
pub(crate) const SCHEMA_VERSION: u32 = dartq_commands::SCHEMA_VERSION;

/// The `dartq/*` method names — the daemon's whole request face.
/// The `dartq/overlay.*` faces answer at intake
/// (the server intercepts them before the scheduler queue — pure staging);
/// the rest, including the two plugin-dispatch faces (`dartq/fix`,
/// `dartq/assists`), stage onto the session's scheduler — the fix faces
/// execute off the main loop at their drain (the session module's
/// fix-dispatch notes).
///
/// `pub` so the drift gate (tests/rpc_schema.rs) can pin the served request
/// set itself, not a parallel copy: the schema test asserts this table IS
/// the dispatchable request family, in both directions.
pub const METHODS: [&str; 15] = [
    "dartq/index",
    "dartq/analyze",
    "dartq/check",
    "dartq/query",
    "dartq/queryBatch",
    "dartq/resolveAt",
    "dartq/completion",
    "dartq/impact",
    "dartq/fix",
    "dartq/assists",
    "dartq/compat",
    "dartq/explain",
    "dartq/overlay.set",
    "dartq/overlay.commit",
    "dartq/overlay.clear",
];

/// A staged `dartq/*` request: decoded parameters. The request id — the
/// identity `dartq/cancel` keys on — stays on the session side.
#[derive(Debug, Clone)]
pub(crate) enum Request {
    /// The analyzed view's resident index tallied (the `index`
    /// face). An engine-view face (the tally reads the view's engine), so
    /// `agent_id`/`context` select the view; `since` is rejected — the
    /// aggregate has no rows for a delta cursor to filter.
    Index {
        verbosity: Verbosity,
        agent: Option<String>,
        context: Option<String>,
    },
    Analyze {
        verbosity: Verbosity,
        agent: Option<String>,
        context: Option<String>,
    },
    Check {
        paths: Vec<PathBuf>,
        verbosity: Verbosity,
        agent: Option<String>,
        context: Option<String>,
        since: Option<u64>,
    },
    /// The symbol-index face; `kind` picks the answered rows (symbol
    /// by default, references, the callers subset, the implementations
    /// walk, or the declaring library's dependencies).
    Query {
        name: String,
        kind: QueryKind,
        verbosity: Verbosity,
        agent: Option<String>,
        context: Option<String>,
        since: Option<u64>,
    },
    /// The batch face: every item's kinds answered over the one
    /// session graph in a single round trip. The answer is one frozen v1
    /// query envelope per `(item, kind)` pair — request order, no aggregate
    /// envelope (the schema crate's frozen batch convention).
    QueryBatch {
        queries: Vec<BatchQuery>,
        verbosity: Verbosity,
        agent: Option<String>,
        context: Option<String>,
        since: Option<u64>,
    },
    /// The position-resolution face: what the analyzed view's maintained
    /// index rows hold under one file offset. An engine-view face
    /// (`agent_id`/`context` select the view); a point answer, so `since`
    /// is rejected — it has no rows for a cursor to filter (the
    /// `dartq/index` rule).
    ResolveAt {
        file: PathBuf,
        offset: u32,
        verbosity: Verbosity,
        agent: Option<String>,
        context: Option<String>,
    },
    /// The built-in completion face: the visible-name set at one cursor
    /// over the analyzed view's maintained index — no plugin escape (the
    /// charter has no `completions` extension point). An engine-view face
    /// like [`Request::ResolveAt`]; a point answer, so `since` is
    /// rejected (the `dartq/index` rule).
    Completion {
        file: PathBuf,
        offset: u32,
        verbosity: Verbosity,
        agent: Option<String>,
        context: Option<String>,
    },
    Impact {
        target: PathBuf,
        verbosity: Verbosity,
        agent: Option<String>,
        context: Option<String>,
        since: Option<u64>,
    },
    /// The report over a per-query disk engine (the visibility
    /// table's `dartq/explain` row): no `agent`/`context` field by
    /// construction, and `sdk` — an explicit value is authoritative; absent
    /// defaults to the session's resolved SDK root.
    Explain {
        symbol: String,
        verbosity: Verbosity,
        since: Option<u64>,
        sdk: Option<PathBuf>,
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
    /// The request's `agent_id`, if it named one. The server validates the
    /// agent exists before the request queues (fail loud on a typo instead
    /// of silently answering the base view).
    pub(crate) fn agent(&self) -> Option<&str> {
        match self {
            Self::Index { agent, .. }
            | Self::Analyze { agent, .. }
            | Self::Check { agent, .. }
            | Self::Query { agent, .. }
            | Self::QueryBatch { agent, .. }
            | Self::ResolveAt { agent, .. }
            | Self::Completion { agent, .. }
            | Self::Impact { agent, .. } => agent.as_deref(),
            Self::Explain { .. } | Self::Fix { .. } | Self::Compat { .. } => None,
        }
    }

    /// The request's `context` tag, if it named one (the multi-root routing
    /// selector; validated at intake like `agent_id`).
    pub(crate) fn context(&self) -> Option<&str> {
        match self {
            Self::Index { context, .. }
            | Self::Analyze { context, .. }
            | Self::Check { context, .. }
            | Self::Query { context, .. }
            | Self::QueryBatch { context, .. }
            | Self::ResolveAt { context, .. }
            | Self::Completion { context, .. }
            | Self::Impact { context, .. } => context.as_deref(),
            Self::Explain { .. } | Self::Fix { .. } | Self::Compat { .. } => None,
        }
    }

    /// The request's verbosity tier. Every face carries one; the read task
    /// uses it to partition the fan-out (full-tier answers write the
    /// short-id table and answer on the session thread).
    pub(crate) fn verbosity(&self) -> Verbosity {
        match self {
            Self::Index { verbosity, .. }
            | Self::Analyze { verbosity, .. }
            | Self::Check { verbosity, .. }
            | Self::Query { verbosity, .. }
            | Self::QueryBatch { verbosity, .. }
            | Self::ResolveAt { verbosity, .. }
            | Self::Completion { verbosity, .. }
            | Self::Impact { verbosity, .. }
            | Self::Explain { verbosity, .. }
            | Self::Fix { verbosity, .. }
            | Self::Compat { verbosity, .. } => *verbosity,
        }
    }
}

/// Whether `method` is one of the `dartq/*` faces this module serves.
#[must_use]
pub(crate) fn is_command(method: &str) -> bool {
    METHODS.contains(&method)
}

// The faces' parameter payloads, decoded with serde (`#[serde(default)]`
// keeps every field optional beyond the contract-critical ones).

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
    /// An unknown spelling fails here with serde's one-of message —
    /// the accepted set, surfaced as `INVALID_PARAMS`.
    #[serde(default)]
    kind: QueryKind,
    #[serde(default)]
    verbosity: Verbosity,
}

#[derive(Deserialize)]
struct BatchQueryParams {
    queries: Vec<BatchItemParams>,
    #[serde(default)]
    verbosity: Verbosity,
}

#[derive(Deserialize)]
struct BatchItemParams {
    name: String,
    /// Absent means the symbol face (the single request's default); an
    /// explicit empty set answers nothing for the item.
    #[serde(default = "default_query_kinds")]
    kinds: Vec<QueryKind>,
}

#[derive(Deserialize)]
struct ResolveAtParams {
    file: PathBuf,
    /// UTF-16 code units from the file's start — the engine's external
    /// convention, the same units the query rows' `offset` fields carry.
    offset: u32,
    #[serde(default)]
    verbosity: Verbosity,
}

#[derive(Deserialize)]
struct CompletionParams {
    file: PathBuf,
    /// UTF-16 code units from the file's start — the engine's external
    /// convention, the same units the query rows' `offset` fields carry.
    offset: u32,
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
struct ExplainParams {
    symbol: String,
    #[serde(default)]
    verbosity: Verbosity,
    /// The per-query SDK root; absent defaults to the session's
    /// resolved SDK root (an explicit value wins).
    sdk: Option<PathBuf>,
}

#[derive(Deserialize)]
struct CompatParams {
    path: PathBuf,
    sdk_roots: Vec<PathBuf>,
    #[serde(default)]
    verbosity: Verbosity,
}

/// One agent's staged overlay: its name plus the staged entries —
/// whole-file replacement buffers, or tombstones (`null` content — the
/// agent's view excludes the path) — per path.
pub(crate) type StagedOverlay = (String, Vec<(PathBuf, StagedFile)>);

/// Decodes one `dartq/*` request's parameters. Relative paths resolve
/// against `root`; `Err` is the `INVALID_PARAMS` message.
///
/// # Errors
///
/// A structural mismatch (wrong type, missing field, unknown `verbosity`)
/// or an empty `paths`/`sdk_roots` array.
pub(crate) fn parse(method: &str, params: &Value, root: &Path) -> Result<Request, String> {
    let invalid = |error: serde_json::Error| error.to_string();
    match method {
        "dartq/index" => {
            // An engine-view tally, so the view selectors apply — but a
            // delta cursor has no rows to filter here: the answer is four
            // verdict counts over the whole view.
            let (agent, since) = session_fields(params)?;
            if since.is_some() {
                return Err(
                    "dartq/index tallies the whole analyzed view; since applies to the \
                     row-shaped faces (check/query/impact/explain)"
                        .into(),
                );
            }
            let context = context_field(params)?;
            let decoded =
                serde_json::from_value::<VerbosityParams>(params.clone()).map_err(invalid)?;
            Ok(Request::Index {
                verbosity: decoded.verbosity,
                agent,
                context,
            })
        }
        "dartq/analyze" => {
            let (agent, _) = session_fields(params)?;
            let context = context_field(params)?;
            let decoded =
                serde_json::from_value::<VerbosityParams>(params.clone()).map_err(invalid)?;
            Ok(Request::Analyze {
                verbosity: decoded.verbosity,
                agent,
                context,
            })
        }
        "dartq/check" | "dartq/fix" => parse_check_or_fix(method, params, root),
        "dartq/query" => {
            let (agent, since) = session_fields(params)?;
            let context = context_field(params)?;
            let decoded = serde_json::from_value::<QueryParams>(params.clone()).map_err(invalid)?;
            Ok(Request::Query {
                name: decoded.name,
                kind: decoded.kind,
                verbosity: decoded.verbosity,
                agent,
                context,
                since,
            })
        }
        "dartq/queryBatch" => parse_query_batch(params),
        "dartq/resolveAt" => {
            // A point answer: the view selectors apply, a delta cursor has
            // no rows to filter (the dartq/index rule).
            let (agent, since) = session_fields(params)?;
            if since.is_some() {
                return Err(
                    "dartq/resolveAt answers one position; since applies to the row-shaped \
                     faces (check/query/impact/explain)"
                        .into(),
                );
            }
            let context = context_field(params)?;
            let decoded =
                serde_json::from_value::<ResolveAtParams>(params.clone()).map_err(invalid)?;
            Ok(Request::ResolveAt {
                file: resolve(root, decoded.file),
                offset: decoded.offset,
                verbosity: decoded.verbosity,
                agent,
                context,
            })
        }
        "dartq/completion" => {
            // A point answer over the visible-name set: the view selectors
            // apply, a delta cursor has no rows to filter (the
            // dartq/index rule).
            let (agent, since) = session_fields(params)?;
            if since.is_some() {
                return Err(
                    "dartq/completion answers the visible-name set at one position; since \
                     applies to the row-shaped faces (check/query/impact/explain)"
                        .into(),
                );
            }
            let context = context_field(params)?;
            let decoded =
                serde_json::from_value::<CompletionParams>(params.clone()).map_err(invalid)?;
            Ok(Request::Completion {
                file: resolve(root, decoded.file),
                offset: decoded.offset,
                verbosity: decoded.verbosity,
                agent,
                context,
            })
        }
        "dartq/impact" => {
            let (agent, since) = session_fields(params)?;
            let context = context_field(params)?;
            let decoded =
                serde_json::from_value::<TargetParams>(params.clone()).map_err(invalid)?;
            Ok(Request::Impact {
                target: resolve(root, decoded.path),
                verbosity: decoded.verbosity,
                agent,
                context,
                since,
            })
        }
        "dartq/explain" => parse_explain(params),
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

/// Decodes `dartq/check` and `dartq/fix` (one shared payload shape; check
/// answers the session's view, fix the disk view).
///
/// # Errors
///
/// An empty `paths` array, or (`dartq/fix`) any session view field.
fn parse_check_or_fix(method: &str, params: &Value, root: &Path) -> Result<Request, String> {
    let invalid = |error: serde_json::Error| error.to_string();
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
        let context = context_field(params)?;
        Ok(Request::Check {
            paths,
            verbosity: decoded.verbosity,
            agent,
            context,
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

/// Decodes `dartq/queryBatch`.
///
/// # Errors
///
/// A structural mismatch in `queries` or a malformed session field.
fn parse_query_batch(params: &Value) -> Result<Request, String> {
    let invalid = |error: serde_json::Error| error.to_string();
    let (agent, since) = session_fields(params)?;
    let context = context_field(params)?;
    let decoded = serde_json::from_value::<BatchQueryParams>(params.clone()).map_err(invalid)?;
    let queries = decoded
        .queries
        .into_iter()
        .map(|item| BatchQuery {
            name: item.name,
            kinds: item.kinds,
        })
        .collect();
    Ok(Request::QueryBatch {
        queries,
        verbosity: decoded.verbosity,
        agent,
        context,
        since,
    })
}

/// Decodes `dartq/explain`.
///
/// # Errors
///
/// An empty `symbol`, or any session view selector (the face answers the
/// disk view only).
fn parse_explain(params: &Value) -> Result<Request, String> {
    let invalid = |error: serde_json::Error| error.to_string();
    // Delta economics without a view selector: `since` filters the
    // report's rows (see `explain_answer`), but an `agent_id` or `context`
    // would silently answer a different view than the face's disk contract
    // — the fix/compat rule, minus `since`.
    let (agent, since) = session_fields(params)?;
    let context = context_field(params)?;
    if agent.is_some() || context.is_some() {
        return Err(
            "dartq/explain answers the disk view; agent_id/context apply to the \
             engine-view faces (analyze/check/query/impact)"
                .into(),
        );
    }
    let decoded = serde_json::from_value::<ExplainParams>(params.clone()).map_err(invalid)?;
    if decoded.symbol.is_empty() {
        return Err("symbol must be a non-empty string".into());
    }
    Ok(Request::Explain {
        symbol: decoded.symbol,
        verbosity: decoded.verbosity,
        since,
        sdk: decoded.sdk,
    })
}

/// The batch item's kinds when the request omits them: the single request's
/// own default (the symbol face).
fn default_query_kinds() -> Vec<QueryKind> {
    vec![QueryKind::Symbol]
}

/// The params' session-economics fields: the optional `agent_id` view selector
/// and the optional `since` delta cursor. Absent and `null` are
/// equivalent.
fn session_fields(params: &Value) -> Result<(Option<String>, Option<u64>), String> {
    let agent = match params.get("agent_id") {
        None | Some(Value::Null) => None,
        Some(Value::String(agent)) => Some(agent.clone()),
        Some(_) => return Err("agent_id must be a string".into()),
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

/// The params' optional `context` tag (the multi-root routing selector).
/// Absent and `null` are equivalent.
fn context_field(params: &Value) -> Result<Option<String>, String> {
    match params.get("context") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(tag)) if !tag.is_empty() => Ok(Some(tag.clone())),
        Some(_) => Err("context must be a non-empty string".into()),
    }
}

/// The disk-view faces take neither session field: an `agent_id`, `context`,
/// or `since` there would silently answer a different view than the face's
/// contract.
pub(crate) fn reject_disk_view_only(params: &Value, method: &str) -> Result<(), String> {
    let (agent, since) = session_fields(params)?;
    let context = context_field(params)?;
    if agent.is_some() || since.is_some() || context.is_some() {
        return Err(format!(
            "{method} answers the disk view; agent_id/context/since apply to the engine-view \
             faces (analyze/check/query/impact)"
        ));
    }
    Ok(())
}

/// Decodes `dartq/overlay.set` params: `{agent_id, files: {path → content}}`.
/// A content value is whole-file replacement text; `null` is a tombstone —
/// the agent's view excludes the path (a base-resident file leaves the
/// view at wear time; a committed tombstone deletes the worktree file).
/// Relative paths resolve against `root`.
///
/// # Errors
///
/// A missing or non-string `agent_id`, a missing or non-object `files`, or
/// a non-string-non-null path content.
pub(crate) fn parse_overlay_set(params: &Value, root: &Path) -> Result<StagedOverlay, String> {
    let agent = overlay_agent(params, "set")?;
    let files = match params.get("files") {
        Some(Value::Object(files)) => files,
        Some(_) => return Err("files must be an object mapping path to content".into()),
        None => return Err("overlay.set requires files".into()),
    };
    let mut staged = Vec::with_capacity(files.len());
    for (path, content) in files {
        let staged_file = match content {
            Value::String(content) => StagedFile::Bytes(content.as_bytes().to_vec()),
            Value::Null => StagedFile::Removed,
            _ => {
                return Err(format!(
                    "files[{path}] must be a string (whole-file content) or null (remove from \
                     the view)"
                ));
            }
        };
        staged.push((resolve(root, PathBuf::from(path)), staged_file));
    }
    Ok((agent, staged))
}

/// The `{agent_id}`-shaped overlay faces' agent field: `overlay.clear`
/// (idempotent teardown) and `overlay.commit` take
/// exactly that.
///
/// # Errors
///
/// A missing or non-string `agent_id`.
fn overlay_agent(params: &Value, method: &str) -> Result<String, String> {
    match params.get("agent_id") {
        Some(Value::String(agent)) if !agent.is_empty() => Ok(agent.clone()),
        Some(_) => Err("agent_id must be a non-empty string".into()),
        None => Err(format!("overlay.{method} requires an agent_id")),
    }
}

/// Decodes `dartq/overlay.clear` params: `{agent_id}`. Clearing an unknown
/// agent is answered ok by the caller (idempotent teardown).
pub(crate) fn parse_overlay_clear(params: &Value) -> Result<String, String> {
    overlay_agent(params, "clear")
}

/// Decodes `dartq/overlay.commit` params: `{agent_id}` — the merge writes
/// the agent's staged whole-file buffers back to the worktree;
/// there is no path subset (the overlay declares the end state, the merge
/// commits it whole).
pub(crate) fn parse_overlay_commit(params: &Value) -> Result<String, String> {
    overlay_agent(params, "commit")
}

/// Relative request paths resolve against the session workspace root.
///
/// Path trust is deliberately split by face (#947 F4; the read-side
/// adjudication is #947 F9). The READ faces — `dartq/check` (`paths`),
/// `dartq/impact` (`target`), `dartq/explain` (`sdk` root), `dartq/compat`
/// (`target` + `sdk_roots`), and the plugin-dispatch pair `dartq/fix`
/// (`paths`) and `dartq/assists` (`path`) — keep CLI parity and accept
/// absolute paths anywhere: a daemon session runs inside the operator's own
/// workspace trust domain (one same-user client over stdio), and these
/// faces only read; the fix faces are additionally dry by contract (they
/// report patch documents, never write). The remaining command faces
/// (`dartq/index`, `dartq/analyze`, `dartq/query`, `dartq/queryBatch`)
/// resolve no client paths at all — they answer from the analyzed view — so
/// the split has no say there. The WRITE faces are confined to the
/// workspace root — `Session::set_overlay` rejects any staged path whose
/// normalized form escapes it, and `Session::commit_overlay` re-checks a
/// canonical (symlink-following) prefix before a byte lands on disk.
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

/// The session's stable path → short-id table: ids (`s1`,
/// `s2`, …) are assigned on first payable use and stay stable for the
/// session; `announced` remembers which ids the client has already seen a
/// `symbols` preamble for, so later answers back-reference without
/// re-announcing.
///
/// The table is mutex-guarded (the engine `StringTable`'s sharing posture)
/// so it can ride the shared read view into the answer workers. The
/// writers — full-verbosity answers, the only faces that assign ids — are
/// answered on the session thread (the read task's serial partition), so
/// id assignment stays drain-order deterministic; the lock exists for the
/// `Sync` bound, not for parallel interning.
#[derive(Default)]
pub(crate) struct SymbolTable {
    inner: Mutex<SymbolIds>,
}

#[derive(Default)]
struct SymbolIds {
    ids: FxHashMap<String, String>,
    announced: FxHashSet<String>,
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
        &self,
        answer: &mut Value,
        spots: &'static [(&'static str, &'static str)],
    ) -> bool {
        let mut inner = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                // A panic died holding the lock (#947 F6): recover rather
                // than degrade every later full-verbosity answer for the
                // rest of the session. The dead answer may have marked ids
                // announced without its preamble reaching the client, so
                // the announced set is dropped — every surviving id
                // re-announces on its next payable use instead of
                // back-referencing an id the client never saw. The ids
                // themselves stay sound: numbering derives from the map's
                // size (the only assignment state), so no half-applied
                // update can make two paths collide on one id.
                let mut guard = poisoned.into_inner();
                guard.announced.clear();
                guard
            }
        };
        let SymbolIds { ids, announced } = &mut *inner;
        // Occurrence counts in first-seen order (deterministic id
        // assignment); the map itself is the seen-set, so a row costs one
        // probe, not a scan of every distinct path (#947 F6 — a
        // 10⁵-distinct-path answer must not go quadratic here).
        let mut counts: FxHashMap<String, usize> = FxHashMap::default();
        let mut order: Vec<String> = Vec::new();
        for (array_field, path_field) in spots {
            let Some(rows) = answer[*array_field].as_array() else {
                continue;
            };
            for row in rows {
                let Some(path) = row[*path_field].as_str() else {
                    continue;
                };
                if let Some(count) = counts.get_mut(path) {
                    *count += 1;
                } else {
                    counts.insert(path.to_owned(), 1);
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
            // always `ids.len() + 1`.
            let id = match ids.get(path) {
                Some(id) if announced.contains(id) => {
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
                    // The fresh id's number is the map's size + 1: ids are
                    // never removed and assignment is the only mutation, so
                    // this is exactly what a separate counter would hold —
                    // without the second fact that a mid-assignment panic
                    // could desynchronize and, after a poison recovery,
                    // hand two paths the same id.
                    let candidate = format!("s{}", ids.len() + 1);
                    let savings = count * path.len().saturating_sub(candidate.len());
                    if savings <= cost(&candidate) {
                        continue;
                    }
                    ids.insert(path.clone(), candidate.clone());
                    candidate
                }
            };
            if announced.insert(id.clone()) {
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
                if let Some(path) = row[*path_field].as_str()
                    && let Some(id) = lookup.get(path)
                {
                    row[*path_field] = Value::String((*id).to_owned());
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
    /// The session's resolved SDK root — `dartq/explain`'s default when its
    /// request omits `sdk` (the session analyzes against this SDK, so the
    /// report must come from the same one).
    pub sdk_root: &'a Path,
    /// The analyzed view's raw diagnostic set (engine-canonical order).
    pub diagnostics: &'a [dartq_diagnostics::Diagnostic],
    /// The shared analyze entries (uri + converted wire diagnostic per
    /// diagnostic of this view), consumed by `dartq/analyze` answers.
    pub entries: &'a [wire::AnalyzeEntry],
    /// The analyzed view's changed-file table for `since` filtering.
    pub changes: &'a Changes<'a>,
    /// The session's current delta cursor, echoed by delta answers.
    pub cursor: u64,
    /// The requesting agent's overlay paths (`dartq/check`'s engine view
    /// answers over the discovered set plus these; empty for base groups).
    pub overlay_paths: &'a [PathBuf],
    /// The session symbol table (short-id economics, full tier). Shared:
    /// the read view crosses into the answer workers, and full-tier
    /// shortening — the only writer — runs on the session thread.
    pub symbols: &'a SymbolTable,
}

/// Answers one staged request at the drain. Shared borrows throughout: the
/// read task fans coalesced requests out to the answer workers over these.
pub(crate) fn answer(drain: &Drain<'_>, request: &Request) -> Result<Value, SessionError> {
    match request {
        Request::Index { verbosity, .. } => Ok(index_answer(drain, *verbosity)),
        Request::Analyze { verbosity, .. } => Ok(analyze_answer(drain, *verbosity)),
        Request::Check {
            paths,
            verbosity,
            agent,
            since,
            ..
        } => match agent {
            Some(_) => check_engine_view(drain, paths, *verbosity, *since),
            None => check_disk_view(drain, paths, *verbosity, *since),
        },
        Request::Query {
            name,
            kind,
            verbosity,
            since,
            ..
        } => Ok(query_answer(drain, name, *kind, *verbosity, *since)),
        Request::QueryBatch {
            queries,
            verbosity,
            since,
            ..
        } => Ok(query_batch_answer(drain, queries, *verbosity, *since)),
        Request::ResolveAt { file, offset, .. } => resolve_at_answer(drain, file, *offset),
        Request::Completion { file, offset, .. } => completion_answer(drain, file, *offset),
        Request::Impact {
            target,
            verbosity,
            since,
            ..
        } => impact_answer(drain, target, *verbosity, *since),
        Request::Explain {
            symbol,
            verbosity,
            since,
            sdk,
        } => explain_answer(drain, symbol, *verbosity, *since, sdk.as_deref()),
        Request::Fix { paths, verbosity } => {
            // Dead on the live wire: the server intercepts `dartq/fix` at
            // intake (the fix-worker dispatch owns the live plugin hosts,
            // so the real answer carries them) before the `is_command` arm
            // could stage it here. The arm stays for match exhaustiveness
            // and answers with the built-in rows alone; a debug hit means
            // the intake interception regressed.
            debug_assert!(
                false,
                "Request::Fix reached the scheduler: `dartq/fix` must be intercepted at intake"
            );
            let mut hosts = fix::PluginHosts::empty();
            fix_answer(paths, *verbosity, &mut hosts, drain.sdk_root)
        }
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
    drain: &Drain<'_>,
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
        Some(since) => changes.changed_since(files.file_path(file), since),
    }
}

// ---------------------------------------------------------------------------
// dartq/index — the index-build face (resident tallies, CLI path labels)
// ---------------------------------------------------------------------------

/// The analyzed view's resident index tallied through the engine's read
/// hooks — the same totals the CLI's fresh-build face reports, with no scan,
/// no parse, no second capture (the read task has just run the view's
/// analysis pass, so the maintained captures and library surfaces are
/// current). The answer is the shared [`dartq_commands::index::IndexRun`]
/// shaper verbatim, and it is tier-invariant: every field is a verdict
/// count, nothing to trim (the impact posture).
///
/// A live library member without a capture is an engine-contract violation:
/// the tally skips the member — the counts under-report for this read, never
/// a panic (the read task's panic boundary stays for real faults).
fn index_answer(drain: &Drain<'_>, _verbosity: Verbosity) -> Value {
    let mut files = 0;
    let mut symbols = 0;
    for (_, members) in drain.engine.libraries() {
        files += members.len();
        for &file in &members {
            // The analyzed member's capture is the engine contract; a miss
            // is a pairing violation — the member contributes no tally this
            // read, never a panic.
            debug_assert!(drain.engine.file_capture(file).is_some(), "analyzed member");
            let Some(capture) = drain.engine.file_capture(file) else {
                continue;
            };
            symbols += capture.index.len();
        }
    }
    let mut libraries = 0;
    let mut references = 0;
    for (_, surface) in drain.engine.library_surfaces() {
        libraries += 1;
        references += surface.reference_count();
    }
    dartq_commands::index::IndexRun {
        files,
        libraries,
        symbols,
        references,
    }
    .to_json()
}

// ---------------------------------------------------------------------------
// dartq/analyze — the diagnostics pull face
// ---------------------------------------------------------------------------

/// The merged diagnostic set of the analyzed view. Compact/full keep the
/// flattened row (`code`/`sev`/`file`/`line`/`col`/`len`/`msg` — the wire
/// module's row builder, `file` carrying the document URI); minimal keeps
/// the red line (code + locator, the same shape minus `sev`/`msg`); full
/// equals compact plus the short-id economics over row `file` values. The
/// envelope comes from the wire-shape module (`crate::wire`) — the frozen
/// pull-face shape — plus the session economics here.
/// Rows keep the CLI analyze face's severity-first printed order (the
/// entries are sorted where the read task builds them); the other faces
/// mirror the engine's canonical (file, offset) order.
pub(crate) fn analyze_answer(drain: &Drain<'_>, verbosity: Verbosity) -> Value {
    let rows: Vec<Value> = drain
        .entries
        .iter()
        .map(|entry| match verbosity {
            Verbosity::Minimal => json!({
                "code": entry.diagnostic.code,
                "file": entry.uri,
                "line": entry.diagnostic.range.start.line + 1,
                "col": entry.diagnostic.range.start.character + 1,
                "len": entry.len,
            }),
            Verbosity::Compact | Verbosity::Full => crate::wire::row(entry),
        })
        .collect();
    session_answer(
        crate::wire::envelope(&rows),
        verbosity,
        None,
        drain,
        &[("diags", "file")],
    )
}

// ---------------------------------------------------------------------------
// dartq/check — base: the M0 disk pipeline; agent_id: the overlay view's
// engine diagnostics (the module doc's visibility table is the contract)
// ---------------------------------------------------------------------------

/// One check row, the analyze vocabulary over a disk/engine diagnostic:
/// `{code, sev, file, line, col, len, msg}` (compact/full; `sev` the
/// lowercase word, `line`/`col` 1-based, code in dartq's stable
/// `snake_case` name spelling); minimal keeps the red line (code +
/// locator); full equals compact today.
fn check_row(
    diagnostic: &Diagnostic,
    files: &dyn FileLayoutProvider,
    verbosity: Verbosity,
) -> Value {
    let (line, column) = files.line_col(diagnostic.file, diagnostic.range.offset);
    let file = files.file_path(diagnostic.file).display().to_string();
    match verbosity {
        Verbosity::Minimal => json!({
            "code": diagnostic.code.name,
            "file": file,
            "line": line,
            "col": column,
            "len": diagnostic.range.length,
        }),
        Verbosity::Compact | Verbosity::Full => json!({
            "code": diagnostic.code.name,
            "sev": wire::severity_word(match diagnostic.severity {
                dartq_diagnostics::Severity::Error => 1,
                dartq_diagnostics::Severity::Warning => 2,
                dartq_diagnostics::Severity::Info => 3,
            }),
            "file": file,
            "line": line,
            "col": column,
            "len": diagnostic.range.length,
            "msg": diagnostic.message.as_str(),
        }),
    }
}

/// The base view's check answer: the M0 disk pipeline (scan+parse over the
/// discovered file set), rows filtered by the delta cursor when one came.
fn check_disk_view(
    drain: &Drain<'_>,
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
    let diags: Vec<Value> = output
        .diagnostics
        .iter()
        .filter(|diagnostic| delta_row(output.files, diagnostic.file, drain.changes, since))
        .map(|diagnostic| check_row(diagnostic, output.files, verbosity))
        .collect();
    Ok(session_answer(
        json!({ "v": SCHEMA_VERSION, "diags": diags }),
        verbosity,
        since,
        drain,
        &[("diags", "file")],
    ))
}

/// The agent view's check answer: the analyzed overlay view's engine
/// diagnostics (scan+parse+semantic — a different pipeline than the disk
/// mode above; the visibility table in the module docs pins this), rows
/// restricted to the discovered paths plus the agent's own overlay paths
/// (agent-introduced files are not on disk to discover).
fn check_engine_view(
    drain: &Drain<'_>,
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
    let diags: Vec<Value> = drain
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            allow.contains(layout.file_path(diagnostic.file))
                && delta_row(&layout, diagnostic.file, drain.changes, since)
        })
        .map(|diagnostic| check_row(diagnostic, &layout, verbosity))
        .collect();
    Ok(session_answer(
        json!({ "v": SCHEMA_VERSION, "diags": diags }),
        verbosity,
        since,
        drain,
        &[("diags", "file")],
    ))
}

// ---------------------------------------------------------------------------
// dartq/query — the symbol-index face by kind (symbol / references / callers
// / implementations / dependencies, CLI path labels)
// ---------------------------------------------------------------------------

/// The `dartq/query` answer: `kind` routes to the matching engine query
/// face, then the shared shaper shapes the run — the one shaper the batch
/// face reuses, so the two answers cannot drift.
fn query_answer(
    drain: &Drain<'_>,
    name: &str,
    kind: QueryKind,
    verbosity: Verbosity,
    since: Option<u64>,
) -> Value {
    let run = query::kind_after_analysis(drain.engine, name, kind);
    query_answer_shaped(drain, name, run, verbosity, since)
}

/// The `dartq/queryBatch` answer: one frozen v1 query
/// envelope per `(item, kind)` pair — request order, item order then kind
/// order — every pair walking the session's already-built graph, which is
/// the round trip's whole point (one graph build amortized across N
/// answers). The array carries no aggregate envelope (the schema crate's
/// frozen batch convention); each element is the single face's own answer,
/// shaper-for-shaper.
fn query_batch_answer(
    drain: &Drain<'_>,
    queries: &[BatchQuery],
    verbosity: Verbosity,
    since: Option<u64>,
) -> Value {
    Value::Array(
        query::batch_after_analysis(drain.engine, queries)
            .into_iter()
            .map(|answer| query_answer_shaped(drain, answer.name, answer.run, verbosity, since))
            .collect(),
    )
}

/// The since filter and tier shaping over an already-built query run: a
/// delta answer keeps only the rows whose file changed since the cursor and
/// echoes it. Every tier — minimal included — shapes through the shared
/// `to_json_at_tier` (the CLI JSON face's one implementation, #1556: the
/// package-form label projection is tier-agnostic, so the daemon's answers
/// cannot drift from the CLI's per tier); full adds nothing else (the
/// canonical table lists no full-only query fields besides the session
/// ones, and the rows' paths stay spelled out — the module docs'
/// symbol-table note: the published `symbols` field pins `label|name`
/// values).
fn query_answer_shaped(
    drain: &Drain<'_>,
    name: &str,
    mut run: dartq_commands::query::SymbolRun<'_>,
    verbosity: Verbosity,
    since: Option<u64>,
) -> Value {
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
        // The kind arrays filter on their own row locators: the
        // implementations rows are declaration rows (the declaring file);
        // the dependencies rows carry the importing library's path.
        run.implementations.retain(|decl| {
            drain
                .changes
                .changed_since(run.layout.file_path(decl.file), since)
        });
        run.dependencies
            .retain(|row| drain.changes.changed_since(Path::new(&row.path), since));
    }
    let mut answer = query::to_json_at_tier(&run, name, verbosity);
    if since.is_some() {
        answer["since"] = json!(drain.cursor);
    }
    answer
}

// ---------------------------------------------------------------------------
// dartq/resolveAt — the position-resolution face (daemon-native; the
// offset-keyed substrate the cursor-interactive faces share)
// ---------------------------------------------------------------------------

/// The `dartq/resolveAt` answer over the analyzed view. The wire shape and
/// its contract live in the shared core
/// ([`dartq_commands::resolve`]) — the same implementation the CLI's
/// one-shot `resolve-at` face serves, so the two cannot drift; this layer
/// maps the core's failure set onto the session's error tiers (the
/// residency miss is the params tier, the prelude's own failures cannot
/// arise on this path — the read task has just run the analysis pass).
fn resolve_at_answer(drain: &Drain<'_>, file: &Path, offset: u32) -> Result<Value, SessionError> {
    resolve_core::resolve_at_after_analysis(drain.engine, file, offset).map_err(|error| match error
    {
        ResolveError::NotResident(_) => SessionError::InvalidParams(error.to_string()),
        ResolveError::Analyze(_) => SessionError::Internal(error.to_string()),
    })
}

// ---------------------------------------------------------------------------
// dartq/completion — the built-in completion face (the visible-name set the
// maintained index proves at one cursor; the charter has no plugin escape)
// ---------------------------------------------------------------------------

/// The `dartq/completion` answer over the analyzed view — the shared
/// core's implementation (see [`resolve_at_answer`]'s layering notes).
fn completion_answer(drain: &Drain<'_>, file: &Path, offset: u32) -> Result<Value, SessionError> {
    resolve_core::completion_after_analysis(drain.engine, file, offset).map_err(|error| match error
    {
        ResolveError::NotResident(_) => SessionError::InvalidParams(error.to_string()),
        ResolveError::Analyze(_) => SessionError::Internal(error.to_string()),
    })
}

// ---------------------------------------------------------------------------
// dartq/impact — the report (tier-invariant per the canonical tables)
// ---------------------------------------------------------------------------

fn impact_answer(
    drain: &Drain<'_>,
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
            // The environments axis's tiers (the enum's contract): an
            // unusable context input is the caller's params, a failed axis
            // run is internal.
            impact_core::ImpactError::Usage(message) => SessionError::InvalidParams(message),
            impact_core::ImpactError::Internal(message) => SessionError::Internal(message),
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
    // canonical table trims nothing (the report carries its own version
    // stamp, `v` under the compact vocabulary). The serialization rides
    // the shared producer shaper (`impact_core::to_json`, #1605): one
    // implementation for the CLI and the daemon, schema-validated at
    // birth. Its library labels stay unsubstituted: each row names a
    // distinct library, so short ids save nothing.
    let mut value = impact_core::to_json(&report);
    if since.is_some() {
        value["since"] = json!(drain.cursor);
    }
    Ok(value)
}

// ---------------------------------------------------------------------------
// dartq/explain — the AI report (disk view, the compat posture)
// ---------------------------------------------------------------------------

/// The explain report's path-valued array fields (the short-id economics'
/// substitution spots; full tier only), under the section keys the shared
/// producer (`dartq_commands::explain`) emits today — they flip with the
/// producer's own vocabulary pass, which owns the wire spellings.
/// `declaration.file` names one file — a single occurrence never pays its
/// preamble, so it stays spelled out.
const EXPLAIN_SPOTS: &[(&str, &str)] = &[
    ("bases", "file"),
    ("ifaces", "file"),
    ("refs", "file"),
    ("deps", "path"),
    ("tests", "path"),
];

/// The report for `symbol` over a per-query engine rebuilt on the on-disk
/// base (the visibility table's `dartq/explain` row): the harvest the
/// assembler joins rides the semantic fan-out, which a warm session engine
/// runs over the recomputed libraries only — a session-engine answer would
/// silently miss declarations on every no-op analysis. Tier semantics are
/// the schema's (minimal keeps the declaration locators and swaps the
/// arrays for `counts`; full adds the short-id economics); a delta answer
/// filters the sections that name use/import/test files by the session's
/// written-file cursors and echoes the current cursor. The report is born
/// schema-valid (validated against `Domain::Explain` where it is built).
/// The request's explicit `sdk` is authoritative; absent, the session's
/// own SDK root backs the report.
///
/// # Errors
///
/// Workspace or SDK assembly failure is the usage tier (`InvalidParams`);
/// discovery failure or cancellation the internal tier.
fn explain_answer(
    drain: &Drain<'_>,
    symbol: &str,
    verbosity: Verbosity,
    since: Option<u64>,
    sdk: Option<&Path>,
) -> Result<Value, SessionError> {
    let sdk = sdk.or(Some(drain.sdk_root));
    let mut report =
        explain_core(drain.root, sdk, symbol, verbosity).map_err(|error| match error {
            ExplainError::Workspace(_)
            | ExplainError::Engine(_)
            | ExplainError::NoSuchDiagnostic(_) => SessionError::InvalidParams(error.to_string()),
            ExplainError::Analyze(_) => SessionError::Internal(error.to_string()),
        })?;
    if let Some(since) = since {
        // Delta mode (the impact face's mapping over the report's own
        // sections, under the producer's current section keys): a
        // use/import/test row stays when its file changed in this session
        // after the cursor. The declaration-site sections (`declaration`,
        // `inheritance`, `interfaces`, `constructors`, `methods`) describe
        // the symbol itself and stay — the red line never drops.
        let changed = |path: &str| drain.changes.changed_since(Path::new(path), since);
        for section in ["refs", "deps", "tests"] {
            if let Some(rows) = report.get_mut(section).and_then(Value::as_array_mut) {
                rows.retain(|row| {
                    row["file"]
                        .as_str()
                        .or_else(|| row["path"].as_str())
                        .is_none_or(changed)
                });
            }
        }
    }
    Ok(session_answer(
        report,
        verbosity,
        since,
        drain,
        EXPLAIN_SPOTS,
    ))
}

// ---------------------------------------------------------------------------
// dartq/fix — the patch document (CLI path labels)
// ---------------------------------------------------------------------------

/// The fix answer over `hosts`: the documents for the discovered
/// files (built-in lint rows, the engine check pass's semantic rows, and
/// the plugin rows the hosts returned, every row provenance-tagged) and,
/// when a host failed, the `errors` list. `sdk` backs the semantic pass's
/// engine assembly — the session's resolved root. The fix worker (the
/// session's off-loop dispatch) passes the live host set; the scheduler
/// arm has no host set and answers the built-in rows alone.
pub(crate) fn fix_answer(
    paths: &[PathBuf],
    verbosity: Verbosity,
    hosts: &mut fix::PluginHosts,
    sdk: &Path,
) -> Result<Value, SessionError> {
    let per_file: Vec<FileFixSet> =
        fix::collect_set(paths, hosts, Some(sdk)).map_err(|error| match error {
            DiscoverError::Missing(path) => {
                SessionError::InvalidParams(format!("Directory or file doesn't exist: {path}"))
            }
            DiscoverError::Io(io) => SessionError::Io(io),
        })?;
    let files: Vec<Value> = per_file
        .iter()
        // Plugin-only rows are still a patch document (provenance-tagged).
        .filter(|fixes| !fixes.fixes.is_empty() || !fixes.plugin.is_empty())
        // The document, all tiers canonical (the writer owns the
        // anchor and provenance rows per tier).
        .map(|fixes| fix::patch_json(fixes, verbosity))
        .collect();
    // The daemon face is dry by contract: it reports patches, never writes.
    let mut answer = json!({ "v": SCHEMA_VERSION, "files": files });
    let errors = hosts.take_errors();
    if !errors.is_empty() {
        answer["errors"] = json!(
            errors
                .iter()
                .map(|error| json!({ "plugin": error.plugin, "message": error.message }))
                .collect::<Vec<_>>()
        );
    }
    Ok(answer)
}

// ---------------------------------------------------------------------------
// dartq/compat — the matrix (CLI path labels)
// ---------------------------------------------------------------------------

fn compat_answer(
    root: &Path,
    target: &Path,
    sdk_roots: &[PathBuf],
    verbosity: Verbosity,
) -> Result<Value, SessionError> {
    let (report, notices) =
        compat_core::compat(root, target, sdk_roots).map_err(|error| match error {
            CompatError::Usage(message) => SessionError::InvalidParams(message),
            CompatError::Internal(message) => SessionError::Internal(message),
        })?;
    // The axes' version-skew degrade notices (#1311) log on the daemon's
    // stderr telemetry channel — the RPC answer stays the contract
    // (no new notification face), and the log line keeps the channel's
    // `dartq-daemon:` framing around the shared core's notice verbatim.
    for line in &notices {
        eprintln!("dartq-daemon: {line}");
    }
    // The tier cut and the serialization both ride the shared producer
    // faces (`compat_core::at_verbosity` + `to_json`, #1605) — the same
    // cut the CLI serves and the same schema-validated birth, no hand-cut
    // tier second implementation.
    Ok(compat_core::to_json(&compat_core::at_verbosity(
        report, verbosity,
    )))
}

impl From<AnalyzeError> for SessionError {
    fn from(error: AnalyzeError) -> Self {
        match error {
            AnalyzeError::Discovery(io) => Self::Io(io),
            AnalyzeError::Cancelled => Self::Cancelled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `diags`-shaped spot list the tests shorten against.
    const ROWS: &[(&str, &str)] = &[("diags", "file")];

    /// Ids assign in first-seen row order (#947 F6): the first distinct
    /// path of the answer takes `s1`, the second `s2`, regardless of how
    /// many rows each has — the pinned contract any membership-structure
    /// rewrite must preserve.
    #[test]
    fn shortening_assigns_ids_in_first_seen_order() {
        let table = SymbolTable::default();
        let mut answer = json!({
            "diags": [
                {"file": "/workspace/lib/first.dart"},
                {"file": "/workspace/lib/second_longer_name.dart"},
                {"file": "/workspace/lib/first.dart"},
                {"file": "/workspace/lib/second_longer_name.dart"},
                {"file": "/workspace/lib/first.dart"},
            ]
        });
        assert!(table.shorten(&mut answer, ROWS));
        assert_eq!(answer["diags"][0]["file"], "s1");
        assert_eq!(answer["diags"][1]["file"], "s2");
        assert_eq!(answer["diags"][2]["file"], "s1");
        assert_eq!(answer["diags"][3]["file"], "s2");
        assert_eq!(answer["diags"][4]["file"], "s1");
        assert_eq!(answer["symbols"]["s1"], "/workspace/lib/first.dart");
        assert_eq!(
            answer["symbols"]["s2"],
            "/workspace/lib/second_longer_name.dart"
        );
    }

    /// A poisoned table recovers (#947 F6) instead of degrading every
    /// later full-verbosity answer: shortening keeps working, ids stay
    /// stable, and an id the panicked answer marked announced (its
    /// preamble lost with the answer) re-announces rather than
    /// back-referencing something the client never saw.
    #[test]
    fn poisoned_table_recovers_and_reannounces() {
        let table = SymbolTable::default();
        {
            let mut inner = table.inner.lock().expect("unpoisoned lock");
            inner
                .ids
                .insert("/workspace/open/a.dart".into(), "s1".into());
            inner.announced.insert("s1".into());
        }
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = table.inner.lock().expect("lock for poisoning");
            panic!("poison the symbol-table lock");
        }));

        // Three occurrences of the known path: the recovered table has it
        // already, so the substitution pays without a fresh id — and the
        // cleared announced set forces the preamble back out.
        let mut answer = json!({
            "diags": [
                {"file": "/workspace/open/a.dart"},
                {"file": "/workspace/open/a.dart"},
                {"file": "/workspace/open/a.dart"},
            ]
        });
        assert!(table.shorten(&mut answer, ROWS));
        assert_eq!(answer["diags"][0]["file"], "s1");
        assert_eq!(
            answer["symbols"]["s1"], "/workspace/open/a.dart",
            "the lost preamble is re-announced after recovery"
        );

        // A fresh path still gets a collision-free next id after the
        // recovery: numbering derives from the recovered map, not from a
        // counter a panic could have desynchronized.
        let mut answer = json!({
            "diags": [
                {"file": "/workspace/open/b_other.dart"},
                {"file": "/workspace/open/b_other.dart"},
                {"file": "/workspace/open/b_other.dart"},
            ]
        });
        assert!(table.shorten(&mut answer, ROWS));
        assert_eq!(answer["diags"][0]["file"], "s2");
        assert_eq!(answer["symbols"]["s2"], "/workspace/open/b_other.dart");
    }
}
