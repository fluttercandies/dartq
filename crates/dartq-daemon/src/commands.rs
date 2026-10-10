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
//! * every request rides the session's read task, so `$/cancelRequest`
//!   cancels it through the epoch machinery like any other read.
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
use dartq_commands::query::{decl_kind_name, ref_kind_name};
use dartq_commands::{Verbosity, check, compat as compat_core, fix, impact as impact_core, query};
use dartq_diagnostics::FileLayoutProvider as _;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::lsp;
use crate::session::SessionError;

/// The response contract version: the frozen v1, mirrored from
/// [`dartq_commands::SCHEMA_VERSION`] (dartq-schema's constants are
/// authoritative).
pub(crate) const SCHEMA_VERSION: u32 = dartq_commands::SCHEMA_VERSION;

/// The `dartq/*` method names, announced under `capabilities.experimental`
/// in `initialize`'s answer.
pub(crate) const METHODS: [&str; 6] = [
    "dartq/analyze",
    "dartq/check",
    "dartq/query",
    "dartq/impact",
    "dartq/fix",
    "dartq/compat",
];

/// A staged `dartq/*` request: decoded parameters. The request id — the
/// identity `$/cancelRequest` keys on — stays on the session side.
#[derive(Debug, Clone)]
pub(crate) enum Request {
    Analyze {
        verbosity: Verbosity,
    },
    Check {
        paths: Vec<PathBuf>,
        verbosity: Verbosity,
    },
    Query {
        name: String,
        verbosity: Verbosity,
    },
    Impact {
        target: PathBuf,
        verbosity: Verbosity,
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
            let params =
                serde_json::from_value::<VerbosityParams>(params.clone()).map_err(invalid)?;
            Ok(Request::Analyze {
                verbosity: params.verbosity,
            })
        }
        "dartq/check" | "dartq/fix" => {
            let params = serde_json::from_value::<CheckParams>(params.clone()).map_err(invalid)?;
            if params.paths.is_empty() {
                return Err("paths must be a non-empty array".into());
            }
            let paths = params
                .paths
                .into_iter()
                .map(|path| resolve(root, path))
                .collect();
            Ok(if method == "dartq/check" {
                Request::Check {
                    paths,
                    verbosity: params.verbosity,
                }
            } else {
                Request::Fix {
                    paths,
                    verbosity: params.verbosity,
                }
            })
        }
        "dartq/query" => {
            let params = serde_json::from_value::<QueryParams>(params.clone()).map_err(invalid)?;
            Ok(Request::Query {
                name: params.name,
                verbosity: params.verbosity,
            })
        }
        "dartq/impact" => {
            let params = serde_json::from_value::<TargetParams>(params.clone()).map_err(invalid)?;
            Ok(Request::Impact {
                target: resolve(root, params.path),
                verbosity: params.verbosity,
            })
        }
        "dartq/compat" => {
            let params = serde_json::from_value::<CompatParams>(params.clone()).map_err(invalid)?;
            if params.sdk_roots.is_empty() {
                return Err(
                    "sdk_roots must name at least one SDK root (the CLI's $DART_SDK default \
                     axis has no daemon equivalent)"
                        .into(),
                );
            }
            Ok(Request::Compat {
                target: resolve(root, params.path),
                sdk_roots: params.sdk_roots,
                verbosity: params.verbosity,
            })
        }
        _ => Err(format!("not a dartq command: {method}")),
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

/// Everything an answer needs from the session at drain time.
pub(crate) struct Drain<'a> {
    /// The session's analyzed engine (the read task has just run the
    /// analysis pass — commands must not re-drive it).
    pub engine: &'a dartq_engine::Engine,
    /// The session workspace root (relative-path resolution).
    pub root: &'a Path,
    /// The shared analyze entries (uri + converted LSP diagnostic per
    /// diagnostic), consumed by `dartq/analyze` answers.
    pub entries: &'a [lsp::AnalyzeEntry],
}

/// Answers one staged request at the drain.
pub(crate) fn answer(drain: &Drain<'_>, request: &Request) -> Result<Value, SessionError> {
    match request {
        Request::Analyze { verbosity } => Ok(analyze_answer(drain.entries, *verbosity)),
        Request::Check { paths, verbosity } => check_answer(paths, *verbosity),
        Request::Query { name, verbosity } => Ok(query_answer(drain.engine, name, *verbosity)),
        Request::Impact { target, verbosity } => impact_answer(drain.engine, target, *verbosity),
        Request::Fix { paths, verbosity } => fix_answer(paths, *verbosity),
        Request::Compat {
            target,
            sdk_roots,
            verbosity,
        } => compat_answer(drain.root, target, sdk_roots, *verbosity),
    }
}

// ---------------------------------------------------------------------------
// dartq/analyze — the LSP-native pull face
// ---------------------------------------------------------------------------

/// The merged diagnostic set of the fresh analysis. Compact keeps the
/// entry shape (uri + LSP diagnostic); minimal keeps the red line (code +
/// location, here `uri` + `range`); full equals compact today (the
/// canonical table lists no full-only diagnostics fields).
pub(crate) fn analyze_answer(entries: &[lsp::AnalyzeEntry], verbosity: Verbosity) -> Value {
    let diagnostics: Vec<Value> = entries
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
    json!({ "schema_version": SCHEMA_VERSION, "diagnostics": diagnostics })
}

// ---------------------------------------------------------------------------
// dartq/check — the M0 scan+parse face (CLI path labels)
// ---------------------------------------------------------------------------

/// One check row. Compact mirrors the machine face's field set (severity /
/// type / code / file / position / length / message, code in dartq's
/// stable snake_case name spelling); minimal keeps the red line (`code` +
/// location); full equals compact today.
fn check_row(
    diagnostic: &dartq_diagnostics::Diagnostic,
    files: &dyn dartq_diagnostics::FileLayoutProvider,
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

fn check_answer(paths: &[PathBuf], verbosity: Verbosity) -> Result<Value, SessionError> {
    let files: BTreeSet<PathBuf> = discover::discover(paths).map_err(|error| match error {
        DiscoverError::Missing(path) => SessionError::InvalidParams(format!(
            "Directory or file doesn't exist: {path}"
        )),
        DiscoverError::Io(io) => SessionError::Io(io),
    })?;
    let run = check::analyze_files(&files).map_err(SessionError::Io)?;
    let output = run.output();
    let diagnostics: Vec<Value> = output
        .diagnostics
        .iter()
        .map(|diagnostic| check_row(diagnostic, output.files, verbosity))
        .collect();
    Ok(json!({ "schema_version": SCHEMA_VERSION, "diagnostics": diagnostics }))
}

// ---------------------------------------------------------------------------
// dartq/query — the symbol index face (CLI path labels)
// ---------------------------------------------------------------------------

fn query_answer(engine: &dartq_engine::Engine, name: &str, verbosity: Verbosity) -> Value {
    let run = query::symbol_after_analysis(engine, name);
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
            json!({
                "schema_version": SCHEMA_VERSION,
                "name": name,
                "declarations": declarations,
                "references": references,
            })
        }
        // Compact is the CLI JSON face verbatim (the shared shaper, which
        // stamps `schema_version`); the canonical table lists no full-only
        // query fields, so full equals compact today.
        Verbosity::Compact | Verbosity::Full => query::to_json(&run, name),
    }
}

// ---------------------------------------------------------------------------
// dartq/impact — the §15.4 report (tier-invariant per the canonical tables)
// ---------------------------------------------------------------------------

fn impact_answer(
    engine: &dartq_engine::Engine,
    target: &Path,
    _verbosity: Verbosity,
) -> Result<Value, SessionError> {
    let report = impact_core::impact_after_analysis(engine, target).map_err(|error| match error {
        impact_core::ImpactError::Missing(path) => {
            SessionError::InvalidParams(format!("Directory or file doesn't exist: {}", path.display()))
        }
        impact_core::ImpactError::NotAnalyzed(path) => {
            SessionError::InvalidParams(format!("not an analyzed .dart file: {}", path.display()))
        }
        impact_core::ImpactError::Step(step) => step.into(),
    })?;
    // Tier-invariant: every report field is a locator or a verdict — the
    // canonical table trims nothing (the report carries its own
    // `schema_version`).
    Ok(serde_json::to_value(&report).expect("ImpactReport serializes"))
}

// ---------------------------------------------------------------------------
// dartq/fix — the §15.6 patch document (CLI path labels)
// ---------------------------------------------------------------------------

fn fix_answer(paths: &[PathBuf], verbosity: Verbosity) -> Result<Value, SessionError> {
    let per_file: Vec<FileFixes> = fix::collect(paths).map_err(|error| match error {
        DiscoverError::Missing(path) => SessionError::InvalidParams(format!(
            "Directory or file doesn't exist: {path}"
        )),
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
