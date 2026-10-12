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
//!   other read. The three `dartq/overlay.*` faces answer at intake instead
//!   (they mutate session bookkeeping or the worktree, never analyze).
//!
//! # Context routing (PRD §16.2, multi-root)
//!
//! The engine-view faces accept an optional `context` tag naming a declared
//! context (a `dartq.yaml` row assembled at the shell edge); the request
//! analyzes that context's engine. Without one they analyze the default
//! context. The disk-view faces take neither `context` nor `agentId` —
//! they never touch a session engine view.
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
//! | `dartq/index`   | requested view — the analyzed view's resident    |
//! |                 | index tallied (files/libraries/symbols/referenc  |
//! |                 | es); the read task has just analyzed, so the     |
//! |                 | tallies read the maintained state with no scan,  |
//! |                 | no parse, no second capture                      |
//! | `dartq/analyze` | requested view (base, or the agent's overlay)    |
//! | `dartq/check`   | base: the M0 disk pipeline (scan+parse over the  |
//! |                 | discovered file set). `agentId`: the requested   |
//! |                 | view's engine diagnostics (scan+parse+semantic)  |
//! |                 | restricted to the discovered paths plus the      |
//! |                 | agent's own overlay paths — the two check modes  |
//! |                 | are deliberately different pipelines, and the    |
//! |                 | agent mode is the overlay-aware one              |
//! | `dartq/query`   | requested view                                   |
//! | `dartq/queryBatch` | requested view (one graph walk per batch —     |
//! |                 | §15.8.4's amortization across the round trip)    |
//! | `dartq/impact`  | requested view                                   |
//! | `dartq/fix`     | disk, always (`agentId`/`since` rejected)        |
//! | `dartq/compat`  | disk, always — it rebuilds one engine per axis   |
//! |                 | over the on-disk base (`agentId`/`since`         |
//! |                 | rejected)                                        |
//! | `dartq/explain` | disk, always — it rebuilds one engine over the   |
//! |                 | on-disk base per query (the `dartq/compat`       |
//! |                 | posture: the report's harvest rides the semantic |
//! |                 | fan-out, which the session engine runs over the  |
//! |                 | recomputed libraries only, so a warm session     |
//! |                 | answer would silently miss declarations).        |
//! |                 | `agentId` is rejected; `since` applies as delta  |
//! |                 | economics — it filters the report's              |
//! |                 | use/import/test rows by the session's            |
//! |                 | written-file cursors, it does not select a view  |
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
//! * **Batch queries** — `dartq/queryBatch` (§15.8.4) answers every item's
//!   kinds in one round trip over the session's already-built graph: one
//!   frozen v1 query envelope per `(item, kind)` pair, request order, no
//!   aggregate envelope (the schema crate's frozen batch convention). The
//!   session selectors (`agentId`/`context`) and `since` apply to the whole
//!   batch, exactly as they do to the single face.
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
//!   field stays absent-tolerant until then. Full-tier `dartq/explain`
//!   substitutes its path-valued report rows the same way: the §15.3 schema
//!   declares `symbols` as the id → path table, absent-tolerant, so the
//!   economics apply verbatim (compact explains stay byte-identical to the
//!   CLI's report).
//!
//! # Path labels
//!
//! The CLI-mirror faces (`check`, `query`, `impact`, `explain`, `fix`,
//! `compat`) keep the CLI's canonical path labels; the LSP-native
//! `dartq/analyze` keeps document URIs. Relative request paths resolve
//! against the session's workspace root (the CLI resolves them against the
//! process cwd). The CLI's `$DART_SDK` fallback has no daemon equivalent:
//! `dartq/compat` requires explicit `sdk_roots`, and `dartq/explain` takes
//! an optional `sdk` root per query — absent resolves to the session's own
//! SDK root (the SDK the analyzed view was assembled against); the
//! `$DART_SDK` fallback stays the CLI path's business.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use dartq_commands::analyze::AnalyzeError;
use dartq_commands::compat::CompatError;
use dartq_commands::discover::{self, DiscoverError};
use dartq_commands::explain::{ExplainError, explain as explain_core};
use dartq_commands::fix::FileFixSet;
use dartq_commands::layout::EngineLayout;
use dartq_commands::query::{BatchQuery, QueryKind, decl_kind_name, ref_kind_name};
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
/// in `initialize`'s answer. The `dartq/overlay.*` faces answer at intake
/// (the server intercepts them before the scheduler queue — pure staging);
/// the rest, including the two plugin-dispatch faces (`dartq/fix`,
/// `dartq/assists`), stage onto the session's scheduler — the fix faces
/// execute off the main loop at their drain (the session module's
/// fix-dispatch notes).
pub(crate) const METHODS: [&str; 13] = [
    "dartq/index",
    "dartq/analyze",
    "dartq/check",
    "dartq/query",
    "dartq/queryBatch",
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
/// identity `$/cancelRequest` keys on — stays on the session side.
#[derive(Debug, Clone)]
pub(crate) enum Request {
    /// The analyzed view's resident index tallied (the §15.1 `index`
    /// face). An engine-view face (the tally reads the view's engine), so
    /// `agentId`/`context` select the view; `since` is rejected — the
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
    /// The §15.2 symbol-index face; `kind` picks the answered rows (symbol
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
    /// The §15.8.4 batch face: every item's kinds answered over the one
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
    Impact {
        target: PathBuf,
        verbosity: Verbosity,
        agent: Option<String>,
        context: Option<String>,
        since: Option<u64>,
    },
    /// The §15.3 report over a per-query disk engine (the visibility
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
    /// The request's `agentId`, if it named one. The server validates the
    /// agent exists before the request queues (fail loud on a typo instead
    /// of silently answering the base view).
    pub(crate) fn agent(&self) -> Option<&str> {
        match self {
            Self::Index { agent, .. }
            | Self::Analyze { agent, .. }
            | Self::Check { agent, .. }
            | Self::Query { agent, .. }
            | Self::QueryBatch { agent, .. }
            | Self::Impact { agent, .. } => agent.as_deref(),
            Self::Explain { .. } | Self::Fix { .. } | Self::Compat { .. } => None,
        }
    }

    /// The request's `context` tag, if it named one (the multi-root routing
    /// selector; validated at intake like `agentId`).
    pub(crate) fn context(&self) -> Option<&str> {
        match self {
            Self::Index { context, .. }
            | Self::Analyze { context, .. }
            | Self::Check { context, .. }
            | Self::Query { context, .. }
            | Self::QueryBatch { context, .. }
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
        "dartq/queryBatch" => {
            let (agent, since) = session_fields(params)?;
            let context = context_field(params)?;
            let decoded =
                serde_json::from_value::<BatchQueryParams>(params.clone()).map_err(invalid)?;
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
        "dartq/explain" => {
            // Delta economics without a view selector: `since` filters the
            // report's rows (see `explain_answer`), but an `agentId` or
            // `context` would silently answer a different view than the
            // face's disk contract — the fix/compat rule, minus `since`.
            let (agent, since) = session_fields(params)?;
            let context = context_field(params)?;
            if agent.is_some() || context.is_some() {
                return Err(
                    "dartq/explain answers the disk view; agentId/context apply to the \
                     engine-view faces (analyze/check/query/impact)"
                        .into(),
                );
            }
            let decoded =
                serde_json::from_value::<ExplainParams>(params.clone()).map_err(invalid)?;
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

/// The batch item's kinds when the request omits them: the single request's
/// own default (the symbol face).
fn default_query_kinds() -> Vec<QueryKind> {
    vec![QueryKind::Symbol]
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

/// The params' optional `context` tag (the multi-root routing selector,
/// PRD §16.2). Absent and `null` are equivalent.
fn context_field(params: &Value) -> Result<Option<String>, String> {
    match params.get("context") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(tag)) if !tag.is_empty() => Ok(Some(tag.clone())),
        Some(_) => Err("context must be a non-empty string".into()),
    }
}

/// The disk-view faces take neither session field: an `agentId`, `context`,
/// or `since` there would silently answer a different view than the face's
/// contract.
pub(crate) fn reject_disk_view_only(params: &Value, method: &str) -> Result<(), String> {
    let (agent, since) = session_fields(params)?;
    let context = context_field(params)?;
    if agent.is_some() || since.is_some() || context.is_some() {
        return Err(format!(
            "{method} answers the disk view; agentId/context/since apply to the engine-view \
             faces (analyze/check/query/impact)"
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
    let agent = overlay_agent(params, "set")?;
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

/// The `{agentId}`-shaped overlay faces' agent field: `overlay.clear`
/// (idempotent teardown) and `overlay.commit` (the §16.3 merge) take
/// exactly that.
///
/// # Errors
///
/// A missing or non-string `agentId`.
fn overlay_agent(params: &Value, method: &str) -> Result<String, String> {
    match params.get("agentId") {
        Some(Value::String(agent)) if !agent.is_empty() => Ok(agent.clone()),
        Some(_) => Err("agentId must be a non-empty string".into()),
        None => Err(format!("overlay.{method} requires an agentId")),
    }
}

/// Decodes `dartq/overlay.clear` params: `{agentId}`. Clearing an unknown
/// agent is answered ok by the caller (idempotent teardown).
pub(crate) fn parse_overlay_clear(params: &Value) -> Result<String, String> {
    overlay_agent(params, "clear")
}

/// Decodes `dartq/overlay.commit` params: `{agentId}` — the merge writes
/// the agent's staged whole-file buffers back to the worktree (PRD §16.3);
/// there is no path subset (the overlay declares the end state, the merge
/// commits it whole).
pub(crate) fn parse_overlay_commit(params: &Value) -> Result<String, String> {
    overlay_agent(params, "commit")
}

/// Relative request paths resolve against the session workspace root.
///
/// Path trust is deliberately split by face (#947 F4). The READ faces
/// (check/explain/compat/impact/sdk) keep CLI parity and accept absolute
/// paths anywhere: a daemon session runs inside the operator's own
/// workspace trust domain and these faces only read. The WRITE faces are
/// confined to the workspace root — `Session::set_overlay` rejects any
/// staged path whose normalized form escapes it, and `Session::commit_overlay`
/// re-checks a canonical (symlink-following) prefix before a byte lands on
/// disk. The daemon fix face is dry by contract (it reports patch
/// documents, never writes), so it stays on the read side.
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
        &self,
        answer: &mut Value,
        spots: &'static [(&'static str, &'static str)],
    ) -> bool {
        let mut inner = self.inner.lock().expect("symbol table mutex poisoned");
        let SymbolIds {
            ids,
            announced,
            next,
        } = &mut *inner;
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
                    let candidate = format!("s{}", *next + 1);
                    let savings = count * path.len().saturating_sub(candidate.len());
                    if savings <= cost(&candidate) {
                        continue;
                    }
                    ids.insert(path.clone(), candidate.clone());
                    *next += 1;
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
    /// The session's resolved SDK root — `dartq/explain`'s default when its
    /// request omits `sdk` (the session analyzes against this SDK, so the
    /// report must come from the same one).
    pub sdk_root: &'a Path,
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
            // The scheduler arm has no host set (the intake dispatch owns
            // the live one); the built-in rows alone answer here.
            let mut hosts = fix::PluginHosts::empty();
            fix_answer(paths, *verbosity, &mut hosts)
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
        Some(since) => changes.changed_since(&files.file_path(file), since),
    }
}

// ---------------------------------------------------------------------------
// dartq/index — the §15.1 index-build face (resident tallies, CLI path labels)
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
/// the tally panics on it like the CLI face does, contained by the read
/// task's panic boundary.
fn index_answer(drain: &Drain<'_>, _verbosity: Verbosity) -> Value {
    let mut files = 0;
    let mut symbols = 0;
    for (_, members) in drain.engine.libraries() {
        files += members.len();
        for &file in &members {
            symbols += drain
                .engine
                .file_capture(file)
                .expect("analyzed member")
                .index
                .len();
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
// dartq/analyze — the LSP-native pull face
// ---------------------------------------------------------------------------

/// The merged diagnostic set of the analyzed view. Compact keeps the
/// entry shape (uri + LSP diagnostic); minimal keeps the red line (code +
/// location, here `uri` + `range`); full equals compact plus the short-id
/// economics over entry URIs. Rows keep the CLI analyze face's
/// severity-first printed order (the entries are sorted where the read
/// task builds them); the other faces mirror the engine's canonical
/// (file, offset) order.
pub(crate) fn analyze_answer(drain: &Drain<'_>, verbosity: Verbosity) -> Value {
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

/// The `dartq/queryBatch` answer (PRD §15.8.4): one frozen v1 query
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
/// echoes it; the minimal tier drops the convenience spellings (the
/// declarations' `container`, the references' `qualified`); compact is the
/// CLI JSON face verbatim (the shared shaper, which stamps
/// `schema_version`); full adds nothing else (the canonical table lists no
/// full-only query fields besides the session ones, and the rows' paths
/// stay spelled out — the module docs' symbol-table note: the published
/// `symbols` field pins `label|name` values).
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
    match verbosity {
        // Canonical query table: minimal drops the convenience fields
        // (declarations[].container, references[].qualified) and the
        // reserved kind arrays (non-locating detail sections — the frozen
        // tier table); the row locators stay.
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
        Verbosity::Compact | Verbosity::Full => {
            session_answer(query::to_json(&run, name), verbosity, since, drain, &[])
        }
    }
}

// ---------------------------------------------------------------------------
// dartq/impact — the §15.4 report (tier-invariant per the canonical tables)
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
// dartq/explain — the §15.3 AI report (disk view, the compat posture)
// ---------------------------------------------------------------------------

/// The explain report's path-valued array fields (the short-id economics'
/// substitution spots; full tier only). `declaration.file` names one file —
/// a single occurrence never pays its preamble, so it stays spelled out.
const EXPLAIN_SPOTS: &[(&str, &str)] = &[
    ("inheritance", "file"),
    ("references", "file"),
    ("dependencies", "path"),
    ("affected_tests", "path"),
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
            ExplainError::Workspace(_) | ExplainError::Engine(_) => {
                SessionError::InvalidParams(error.to_string())
            }
            ExplainError::Analyze(_) => SessionError::Internal(error.to_string()),
        })?;
    if let Some(since) = since {
        // Delta mode (the impact face's mapping over the report's own
        // sections): a use/import/test row stays when its file changed in
        // this session after the cursor. The declaration-site sections
        // (`declaration`, `inheritance`, `interfaces`, `constructors`,
        // `methods`) describe the symbol itself and stay — the red line
        // never drops.
        let changed = |path: &str| drain.changes.changed_since(Path::new(path), since);
        for section in ["references", "dependencies", "affected_tests"] {
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
// dartq/fix — the §15.6 patch document (CLI path labels)
// ---------------------------------------------------------------------------

/// The fix answer over `hosts`: the §15.6 documents for the discovered
/// files (built-in rows plus the plugin rows the hosts returned, every row
/// provenance-tagged) and, when a host failed, the `errors` list. The fix
/// worker (the session's off-loop dispatch) passes the live host set; the
/// scheduler arm has no host set and answers the built-in rows alone.
pub(crate) fn fix_answer(
    paths: &[PathBuf],
    verbosity: Verbosity,
    hosts: &mut fix::PluginHosts,
) -> Result<Value, SessionError> {
    let per_file: Vec<FileFixSet> =
        fix::collect_set(paths, hosts).map_err(|error| match error {
            DiscoverError::Missing(path) => {
                SessionError::InvalidParams(format!("Directory or file doesn't exist: {path}"))
            }
            DiscoverError::Io(io) => SessionError::Io(io),
        })?;
    let files: Vec<Value> = per_file
        .iter()
        // Plugin-only rows are still a patch document (provenance-tagged).
        .filter(|fixes| !fixes.fixes.is_empty() || !fixes.plugin.is_empty())
        // The §15.6 document, all tiers canonical (the writer owns the
        // anchor and provenance rows per tier).
        .map(|fixes| fix::patch_json(fixes, verbosity))
        .collect();
    // The daemon face is dry by contract: it reports patches, never writes.
    let mut answer = json!({ "schema_version": SCHEMA_VERSION, "files": files });
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
