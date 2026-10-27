//! The explain face's core: the AI's one-stop symbol report —
//! the commands wiring of the [`dartq_ai::explain`] assembler.
//!
//! The report answers from the engine's maintained index state after the
//! one discover+analyze prelude — no second analysis pass runs. The
//! declaration hits read the per-file captures
//! ([`dartq_engine::Engine::file_capture`]), the reference and supertype
//! rows the per-library reference surfaces
//! ([`dartq_engine::Engine::library_surfaces`]) — the same maintained
//! state `query_symbols` serves, at raw-row fidelity so the assembler's
//! container anchors resolve in place; the constructor rows and the
//! signature faces read the resident element facts
//! ([`dartq_engine::Engine::file_facts`], computed by the same apply
//! phase as the captures). The rows' `StringId`s are native
//! to the engine's intern table, which the answer resolves through. The
//! transient per-answer maps (hit grouping, supertype edges, constructor
//! render) are symbol-scoped and dropped at the end of the call.
//!
//! A `Container.member` spelling (#2655, the last-dot split the query
//! face shares) answers the member's report: the declaration is that
//! member, its references are the member's own sites widened through its
//! override family (the engine's `override_family` face — the same
//! equivalence class the rename face edits), and the container sections
//! (`bases`, `ifaces`, `ctors`, `methods`) are empty by nature for a
//! member. A miss answers the empty seam report, the unknown-name
//! posture.
//!
//! The reverse import graph and the test graph build once per call from
//! the engine's import CSR — the same per-query posture `impact` keeps
//! over the same CSR. The semantic fold runs with the default config:
//! its member-access rows (#2618) are the reference surface's member
//! half, the report's `refs` substrate — only the fold's diagnostics are
//! discarded on this face.
//!
//! The JSON answer is the assembler's serialized report routed through
//! `dartq-schema`'s `validate_at_tier(payload, Domain::Explain, tier)`
//! where the payload is born — the producing tier rides the gate, so a
//! wrong-tier section (the minimal-only `counts` at compact, a section
//! array at minimal) is a producer bug surfaced as the face's
//! internal-tier error — never clamped, never a dead process.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use dartq_ai::diagnostic_explain::{self as diquery};
use dartq_ai::explain::{self as query, ConstructorFact, SymbolDeclHit};
use dartq_ai::rev_import_graph::RevImportGraph;
use dartq_diagnostics::{Diagnostic, FileLayoutProvider};
use dartq_engine::Engine;
use dartq_index::{DeclRow, RefRow, SuperClause, TargetRef};
use dartq_source::{FileId, StringId};
use rustc_hash::FxHashSet;
use serde_json::Value;
use thiserror::Error;

use crate::Verbosity;
use crate::analyze::{AnalyzeError, prelude, run_overlaid};
use crate::fix_table::{FixAction, spec_for};
use crate::impact::impact_substrate;
use crate::workspace::{WorkspaceError, engine_config};

/// Why an explain run aborts: workspace problems are the CLI's usage tier,
/// the prelude the internal tier.
#[derive(Debug, Error)]
pub enum ExplainError {
    /// The workspace faces are unusable (the CLI's usage tier).
    #[error("{0}")]
    Workspace(#[from] WorkspaceError),
    /// The engine context could not bind (the CLI's usage tier).
    #[error("engine context unusable: {0}")]
    Engine(String),
    /// Discovery failure or cancellation, plus the report's own internal
    /// tier: a born report failing its published schema rides this variant
    /// (a producer bug — internal tier, never a usage error).
    #[error("{0}")]
    Analyze(#[from] AnalyzeError),
    /// The `code`/`file`/`line:col` selection matched no emitted row — the
    /// usage-tier refusal (`dartq explain --diagnostic` explains a hit the
    /// run actually carries).
    #[error("no diagnostic '{0}' in this run")]
    NoSuchDiagnostic(String),
}

/// Runs the explain face over the workspace at `dir`: assemble the engine,
/// run the discover+analyze prelude (the read-after-write the report
/// reflects), and answer for `symbol` from the maintained index state as
/// schema-valid JSON.
///
/// # Errors
///
/// [`ExplainError`] on workspace-assembly, engine-bind, discovery,
/// cancellation failure, or a born report failing its published schema
/// (the internal tier — the producer-bug face).
pub fn explain(
    dir: &Path,
    sdk: Option<&Path>,
    symbol: &str,
    verbosity: Verbosity,
) -> Result<Value, ExplainError> {
    let (config, _dir) = engine_config(dir, sdk)?;
    // The semantic fold runs (the default config): its member-access rows
    // (#2618) are the reference surface's member half — without the fold
    // every member query would answer an empty `refs` section. The fold's
    // diagnostics are the discarded part, not the fold itself.
    let mut engine =
        Engine::new(config).map_err(|error| ExplainError::Engine(error.to_string()))?;
    prelude(&mut engine)?;
    explain_answer(&engine, symbol, verbosity)
}

/// The report over an already-analyzed engine: the adapter over the
/// maintained index state (transient, symbol-scoped — dropped at the end
/// of the call) feeding the assembler.
///
/// # Errors
///
/// A born report failing its published schema is the internal-tier
/// [`ExplainError`] — the violation surfaces as the error line (the
/// daemon's `Internal` answer, the CLI's exit 4), never a clamped report
/// and never a dead process.
///
/// # Panics
///
/// Panics on engine-contract violations, surfaced never clamped: a live
/// library member without a capture, or an answer file id missing from
/// the engine's file table, is a program bug.
fn explain_answer(
    engine: &Engine,
    symbol: &str,
    verbosity: Verbosity,
) -> Result<Value, ExplainError> {
    // The queried name interns into the engine's table once — the rows'
    // native table (the documented query posture: the only side effect).
    // A `Container.member` spelling (#2621, the last-dot split shared
    // with the query face since #2655) names the member inside the
    // container: the member segment drives the by-name probe, the
    // container segment filters the hits, and the report still echoes the
    // caller's whole spelling.
    let (container, name) = match crate::query::split_qualified(symbol) {
        Some((container, name)) => (Some(container), name),
        None => (None, symbol),
    };
    let sym = engine.strings().intern(name);

    // Declaration hits from the maintained captures: live libraries ×
    // members, the equal-name run per declaring file, filtered to the
    // spelled container when one rode along. The rows borrow the
    // captures — nothing copies.
    let libraries = engine.libraries();
    let mut hits: Vec<SymbolDeclHit<'_>> = Vec::new();
    let mut hit_files: BTreeMap<u32, Vec<FileId>> = BTreeMap::new();
    for (library, members) in &libraries {
        let library = library.index();
        for &file in members {
            let capture = engine.file_capture(file).expect("analyzed member");
            let Some(rows) = capture.index.by_name(sym) else {
                continue;
            };
            let surface = capture.index.decls();
            let strings = engine.strings();
            let matched: Vec<&DeclRow> = rows
                .iter()
                .filter(|decl| match container {
                    None => true,
                    Some(want) => {
                        decl.container != DeclRow::NONE
                            && strings.resolve(surface[decl.container as usize].name) == want
                    }
                })
                .collect();
            if matched.is_empty() {
                continue;
            }
            hits.extend(matched.into_iter().map(|decl| SymbolDeclHit {
                library,
                file,
                decl,
            }));
            hit_files.entry(library).or_default().push(file);
        }
    }

    // The qualified-member reference scope (#2655): the whole override
    // family of the spelled member — the same equivalence class the
    // rename face edits, whatever member the anchor names — widens the
    // report's reference admission, so a call site statically resolved to
    // an implementing override rides the report beside the member's own
    // sites. A bare name needs no widening face: the assembler scopes to
    // the primary's own target pair.
    let mut family: FxHashSet<(FileId, StringId)> = FxHashSet::default();
    if let Some(container) = container {
        let strings = engine.strings();
        for hit in &hits {
            for (file, spelling) in engine.override_family(hit.file, container, name) {
                family.insert((file, strings.intern(spelling)));
            }
        }
    }

    // The reference rows naming the symbol, the hit files' resolved
    // supertype edges (clause intact — the interfaces/inheritance split
    // reads it), and the hit files' constructor facts (rendered here from
    // the resident element face; the assembler stays semantic-free) — one
    // pass over the maintained state, all transient maps symbol-scoped.
    let mut refs: Vec<RefRow> = Vec::new();
    let mut supers: BTreeMap<FileId, Vec<(TargetRef, SuperClause)>> = BTreeMap::new();
    let mut ctors: BTreeMap<FileId, Vec<ConstructorFact>> = BTreeMap::new();
    for (library, surface) in engine.library_surfaces() {
        refs.extend(
            surface
                .refs()
                .iter()
                .filter(|row| row.target_name == sym)
                .copied(),
        );
        if let Some(files) = hit_files.get(&library.index()) {
            for &file in files {
                supers.insert(
                    file,
                    surface
                        .supertypes_of(file, sym)
                        .map_or(Vec::new(), |edges| {
                            edges
                                .iter()
                                .map(|edge| (edge.supertype, edge.clause))
                                .collect()
                        }),
                );
                let facts = constructor_facts(engine, file, sym);
                if !facts.is_empty() {
                    ctors.insert(file, facts);
                }
            }
        }
    }

    // The graph faces over the engine's import CSR — the shared substrate
    // `impact` reads (one build each per call, #558).
    let substrate = impact_substrate(engine, &libraries);
    let count = substrate.node_count();
    let importers = RevImportGraph::build(count, |library| substrate.neighbors(library));

    // Stable file spellings for every file id the answer carries — the one
    // file locator's projection (the shared [`crate::pkg_label::locate`]
    // rule against the engine's workspace root: root-relative inside it,
    // absolute outside; internal ids never leave the engine boundary).
    let file_table = engine.file_table();
    let mut ids: Vec<FileId> = Vec::new();
    ids.extend(hits.iter().map(|hit| hit.file));
    ids.extend(refs.iter().map(|row| row.file));
    ids.extend(supers.values().flat_map(|edges| {
        edges.iter().filter_map(|(target, _)| match target {
            TargetRef::Decl(file, _) => Some(*file),
            TargetRef::Name(_) => None,
        })
    }));
    ids.sort_unstable();
    ids.dedup();
    // Projected once per answer file here; non-UTF-8 paths spell lossily —
    // the JSON string columns cannot carry them raw either way.
    let analysis_root = engine.packages().workspace_root();
    let file_labels: Vec<String> = ids
        .iter()
        .map(|&file| {
            crate::pkg_label::locate(
                analysis_root,
                file_table.path(file).expect("answer file"),
            )
        })
        .collect();

    // The engine-owned selection faces the pure assembler cannot derive:
    // the hit files' test-tree membership (a real-path property, read
    // from the engine's file table rather than parsed back out of any
    // spelling) and the 1-based line resolution the `others` rows locate
    // by.
    let is_test_of =
        |file: FileId| engine.file_table().path(file).is_some_and(is_test_path);
    let line_of = |file: FileId, offset: u32| {
        let index = engine.line_index(file).expect("answer file line index");
        let content = engine.content(file).expect("answer file content");
        let text = std::str::from_utf8(content).expect("engine content is UTF-8");
        index.line_col_utf16(text, offset).0
    };
    let report = query::explain(
        symbol,
        verbosity.into(),
        &hits,
        &refs,
        |file, container| family.contains(&(file, container)),
        |file| {
            engine
                .file_capture(file)
                .map_or(&[][..], |capture| capture.index.decls())
        },
        |file, _| supers.get(&file).map_or(&[][..], |edges| edges.as_slice()),
        |file, _| ctors.get(&file).map_or(&[][..], |rows| rows.as_slice()),
        &importers,
        |library| substrate.label(library),
        |file| {
            let index = ids.binary_search(&file).expect("answer file label");
            file_labels[index].as_str()
        },
        is_test_of,
        line_of,
        substrate.tests(),
        engine.strings(),
    );
    // One serialization into the answer `Value`, then one serialization of
    // that Value to the validator's &str face (`dartq-schema::validate` is
    // string-typed) — the report is converted exactly twice, never
    // re-parsed. Both conversions are total for this payload (plain
    // derived data: strings, numbers, options, vectors), so the two
    // expects are true invariants; the validator is the real gate.
    let mut value = serde_json::to_value(&report).expect("explain report serializes");
    // The locator envelope: the root rides exactly when the answer carries
    // a file field, and the report's file-bearing row sources all hang off
    // the primary declaration (its own `file` is always present, every
    // section is empty without one) — so the declaration is the ride
    // condition. It slots in right after the version stamp, the reading
    // order every located document keeps; the assembler's struct stays
    // spelling-agnostic, so the envelope is stitched here at the one
    // boundary that knows the root.
    if report.declaration.is_some() {
        let object = value
            .as_object_mut()
            .expect("the explain report serializes as an object");
        let mut located = serde_json::Map::with_capacity(object.len() + 1);
        located.insert(
            "v".to_owned(),
            object.remove("v").expect("the version stamp"),
        );
        located.insert(
            "root".to_owned(),
            serde_json::Value::String(analysis_root.display().to_string()),
        );
        located.append(object);
        *object = located;
    }
    let json = serde_json::to_string(&value).expect("explain report serializes");
    // The published-schema gate, tier-exact: the producing tier threads
    // through, so a wrong-tier section (the minimal-only `counts`
    // elsewhere, a section array at minimal) is a violation, not a born
    // report. A violation is a producer bug and rides the internal tier
    // (the daemon answers `SessionError::Internal`, the CLI exits 4
    // through its catch-all arm) — user-visible and recoverable, never a
    // clamped report and never a dead process. The violation text carries
    // the failure; the variant's discovery-side display prefix is the cost
    // of keeping `ExplainError`'s variant set stable for the daemon's
    // exhaustive tier mapping.
    let tier = match verbosity {
        Verbosity::Minimal => dartq_schema::Verbosity::Minimal,
        Verbosity::Compact => dartq_schema::Verbosity::Compact,
        Verbosity::Full => dartq_schema::Verbosity::Full,
    };
    dartq_schema::validate_at_tier(&json, dartq_schema::Domain::Explain, tier).map_err(
        |violation| {
            ExplainError::Analyze(AnalyzeError::Discovery(io::Error::other(format!(
                "explain report violates the published schema: {violation}"
            ))))
        },
    )?;
    Ok(value)
}

/// The `--file` argument's position clause: `PATH[:LINE[:COL]]` — the
/// 1-based line and UTF-16 column of the diagnostic convention. A bare
/// `PATH` selects the file's first row of the code. `path` resolves
/// against the working directory and canonicalizes inside
/// [`explain_diagnostic`] before it meets the interned paths.
#[derive(Debug, Clone)]
pub struct FileSelector {
    /// The hit file (canonicalized by the caller before resolution).
    pub path: PathBuf,
    /// The 1-based line, when spelled.
    pub line: Option<u32>,
    /// The 1-based column, when spelled.
    pub column: Option<u32>,
}

/// Runs the `explain --diagnostic` face over the workspace at `dir`: the
/// full analyze pipeline (engine rows + the lint stage + the options and
/// provenance rows — the same `dart analyze` set the face echoes), then
/// the one diagnostic selected by `code`/`file` assembled into the
/// schema-valid report.
///
/// `file` narrows the selection to a row in that file; its `:line[:col]`
/// narrows to the row covering that position. Absent `file` the run's
/// first `code` row answers (the oracle-order head).
///
/// # Errors
///
/// [`ExplainError::Workspace`]/[`ExplainError::Engine`]/[`ExplainError::Analyze`]
/// on assembly, engine-bind, discovery, or cancellation failure — the same
/// tiers as the symbol face. A `code`/`file`/`line:col` the run carries no
/// row for answers [`ExplainError::NoSuchDiagnostic`] (the CLI's usage
/// tier).
pub fn explain_diagnostic(
    dir: &Path,
    sdk: Option<&Path>,
    code: &str,
    file: Option<FileSelector>,
    verbosity: Verbosity,
) -> Result<Value, ExplainError> {
    let (config, dir) = engine_config(dir, sdk)?;
    let mut engine =
        Engine::new(config).map_err(|error| ExplainError::Engine(error.to_string()))?;
    // The diagnostics face runs the full pipeline — the lint stage and the
    // options/provenance rows included (prelude alone would emit no rows).
    let run = run_overlaid(&mut engine, &[], std::slice::from_ref(&dir))?;
    // The selector's path resolves against the workspace dir and
    // canonicalizes into the engine's spelling universe before the row
    // comparison — the file table's interned paths are canonical.
    let file = file
        .map(|selector| {
            let base = if selector.path.is_absolute() {
                selector.path.clone()
            } else {
                dir.join(&selector.path)
            };
            base.canonicalize().map(|path| FileSelector {
                path,
                line: selector.line,
                column: selector.column,
            })
        })
        .transpose()
        .map_err(|error| {
            ExplainError::Analyze(AnalyzeError::Discovery(io::Error::other(format!(
                "--file path is unusable: {error}"
            ))))
        })?;
    explain_diagnostic_answer(&run, code, file.as_ref(), verbosity)
}

/// The report over an already-run analyze pass — the assembly the
/// engine-carrying callers (the daemon session) hit the same way.
fn explain_diagnostic_answer(
    run: &crate::analyze::AnalyzeRun<'_>,
    code: &str,
    file: Option<&FileSelector>,
    verbosity: Verbosity,
) -> Result<Value, ExplainError> {
    // The selection scans once: the first row matching the file filter is
    // the candidate, narrowed to the position-filtered row when a line/col
    // selector rides along.
    let selected: Option<&Diagnostic> = match file {
        None => run.diagnostics.iter().find(|d| d.code.name == code),
        Some(sel) => {
            let layout = &run.layout;
            let file_hit = |d: &Diagnostic| {
                layout
                    .engine()
                    .file_table()
                    .path(d.file)
                    .is_some_and(|path| path == sel.path)
            };
            match (sel.line, sel.column) {
                (Some(line), Some(column)) => run.diagnostics.iter().find(|d| {
                    d.code.name == code
                        && file_hit(d)
                        && covers(layout, d, line, column)
                }),
                (Some(line), None) => run
                    .diagnostics
                    .iter()
                    .find(|d| d.code.name == code && file_hit(d) && {
                        let (hit_line, _) = layout.line_col(d.file, d.range.offset);
                        hit_line == line
                    }),
                _ => run
                    .diagnostics
                    .iter()
                    .find(|d| d.code.name == code && file_hit(d)),
            }
        }
    };
    let Some(hit) = selected else {
        return Err(ExplainError::NoSuchDiagnostic(code.to_owned()));
    };
    let layout = &run.layout;
    let code_info = hit.code;
    let file_path = layout.file_path(hit.file).display().to_string();
    let related_lines: Vec<u32> = run
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == code && d.file == hit.file && !std::ptr::eq(*d, hit)
        })
        .map(|d| layout.line_col(d.file, d.range.offset).0)
        .collect();
    let total: u32 = run
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .count() as u32;

    // The registry faces: the type column is the code family; a lint code
    // resolves to its rule-registry line (the `package:lints` group plus
    // the removed/deprecated markers); the fix registry rows render under
    // their action-kind id and provenance tag.
    let rule = lint_rule_fact(code_info.name);
    let why = diquery::WhyFace {
        code_family: code_info.dtype.as_str().to_owned(),
        rule,
    };
    let fixes: Vec<diquery::FixRow> = spec_for(code_info.name)
        .map(|spec| {
            vec![diquery::FixRow {
                id: fix_action_id(&spec.action).to_owned(),
                source: match spec.source {
                    crate::fix_table::FixSource::Lint => "builtin-lint",
                    crate::fix_table::FixSource::Semantic => "builtin-semantic",
                    crate::fix_table::FixSource::Parser => "builtin-parser",
                }
                .to_owned(),
                summary: fix_action_summary(&spec.action).to_owned(),
            }]
        })
        .unwrap_or_default();
    let documentation = code_info
        .has_published_docs
        .then(|| format!("https://dart.dev/diagnostics/{}", code_info.name));
    let report = diquery::explain_diagnostic(
        diquery::DiagnosticEcho {
            code: code_info.name.to_owned(),
            severity: hit.severity.as_machine_str().to_owned(),
            message: hit.message.to_string(),
            file: file_path,
            range: [hit.range.offset, hit.range.offset + hit.range.length],
        },
        why,
        fixes,
        total,
        related_lines,
        documentation,
        verbosity.into(),
    );
    let value = serde_json::to_value(&report).expect("explain report serializes");
    let json = serde_json::to_string(&value).expect("explain report serializes");
    let tier = match verbosity {
        Verbosity::Minimal => dartq_schema::Verbosity::Minimal,
        Verbosity::Compact => dartq_schema::Verbosity::Compact,
        Verbosity::Full => dartq_schema::Verbosity::Full,
    };
    dartq_schema::validate_at_tier(&json, dartq_schema::Domain::DiagnosticExplain, tier)
        .map_err(|violation| {
            ExplainError::Analyze(AnalyzeError::Discovery(io::Error::other(format!(
                "explain report violates the published schema: {violation}"
            ))))
        })?;
    Ok(value)
}

/// Whether the row's span covers the 1-based `(line, column)` position —
/// the position sits inside `[offset, offset + length)` in the row's file.
fn covers(
    layout: &crate::layout::EngineLayout<'_>,
    d: &Diagnostic,
    line: u32,
    column: u32,
) -> bool {
    let (start_line, start_col) = layout.line_col(d.file, d.range.offset);
    let (end_line, end_col) = layout.line_col(d.file, d.range.offset + d.range.length);
    (line, column) >= (start_line, start_col) && (line, column) <= (end_line, end_col)
}

/// The lint-registry line a `LINT` code resolves to: `name`/`group` always,
/// the removed/deprecated markers when the pinned tables record them.
/// Returns `None` for non-lint codes.
fn lint_rule_fact(code: &str) -> Option<diquery::RuleFact> {
    let id = dartq_lints::rule_by_name(code)?;
    let spec = dartq_lints::rules()[id.0 as usize];
    Some(diquery::RuleFact {
        name: spec.name.to_owned(),
        group: match spec.group {
            dartq_lints::Group::Core => "core",
            dartq_lints::Group::Recommended => "recommended",
            dartq_lints::Group::Diagnostic => "diagnostic",
            dartq_lints::Group::Removed => "removed",
            dartq_lints::Group::Standalone => "standalone",
        }
        .to_owned(),
        removed_since: crate::rule_states::removed_since(code)
            .flatten()
            .map(str::to_owned),
        deprecated_since: crate::rule_states::deprecated_since(code).map(str::to_owned),
    })
}

/// The fix family's stable id — the action-kind spelling.
fn fix_action_id(action: &FixAction) -> &'static str {
    match action {
        FixAction::DropLexeme(_) => "drop-lexeme",
        FixAction::RemoveLexeme(_) => "remove-lexeme",
        FixAction::RemoveKeyword(_) => "remove-keyword",
        FixAction::RemoveKeywordAcrossComments(_) => "remove-keyword-across-comments",
        FixAction::ReplaceExact { .. } => "replace-exact",
        FixAction::InsertAfter(_) => "insert-after",
        FixAction::Rewrite(_) => "rewrite",
        FixAction::MultiRewrite(_) => "multi-rewrite",
        FixAction::CreateFile(_) => "create-file",
    }
}

/// The one-sentence description of the action kind — the fix face's own
/// render of the family a row belongs to.
fn fix_action_summary(action: &FixAction) -> &'static str {
    match action {
        FixAction::DropLexeme(_) => {
            "Remove the lexeme at the hit start (trailing whitespace follows)."
        }
        FixAction::RemoveLexeme(_) => {
            "Remove the lexeme at the hit start and one following character."
        }
        FixAction::RemoveKeyword(_) => {
            "Remove the keyword at the hit start through the next token's trivia."
        }
        FixAction::RemoveKeywordAcrossComments(_) => {
            "Remove the keyword at the hit start through the next token, comments included."
        }
        FixAction::ReplaceExact { .. } => {
            "Replace the hit span with its verified replacement text."
        }
        FixAction::InsertAfter(_) => "Insert the fix text at the hit span's end.",
        FixAction::Rewrite(_) => "Apply the family's verified span rewrite.",
        FixAction::MultiRewrite(_) => "Apply the family's verified multi-span rewrite.",
        FixAction::CreateFile(_) => "Create the file the fix names.",
    }
}

/// The commands [`Verbosity`] onto the assembler's: same three tiers, same
/// default — the spelling mapping is the whole job.
impl From<Verbosity> for query::Verbosity {
    fn from(verbosity: Verbosity) -> Self {
        match verbosity {
            Verbosity::Minimal => query::Verbosity::Minimal,
            Verbosity::Compact => query::Verbosity::Compact,
            Verbosity::Full => query::Verbosity::Full,
        }
    }
}

/// The commands [`Verbosity`] onto the diagnostic assembler's — the same
/// three-tier mapping `query::Verbosity` rides.
impl From<Verbosity> for diquery::Verbosity {
    fn from(verbosity: Verbosity) -> Self {
        match verbosity {
            Verbosity::Minimal => diquery::Verbosity::Minimal,
            Verbosity::Compact => diquery::Verbosity::Compact,
            Verbosity::Full => diquery::Verbosity::Full,
        }
    }
}

/// The container `sym`'s constructor facts rendered from the resident
/// element face: named selectors under `A.named`, the unnamed face under
/// the container's own name (the implicit default — `has_unnamed_-
/// constructor` with no captured formals — renders the empty list). The
/// capture mints no constructor rows, so this face is where the section's
/// rows are born; a non-container declaration answers none.
///
/// # Panics
///
/// Panics when the file is resident without element facts — an internal
/// invariant breach (captures and facts are computed by the same apply
/// phase; impact's resident-facts join leans on the same posture).
fn constructor_facts(engine: &Engine, file: FileId, sym: StringId) -> Vec<ConstructorFact> {
    let elements = &engine
        .file_facts(file)
        .expect("resident member facts")
        .elements;
    let Some(container) = elements.container(sym) else {
        return Vec::new();
    };
    let strings = engine.strings();
    let args = elements.type_args();
    let name = strings.resolve(sym);
    let mut rows = Vec::with_capacity(
        container.constructors.len() + usize::from(container.has_unnamed_constructor),
    );
    for selector_id in &container.constructors {
        let selector = strings.resolve(*selector_id);
        rows.push(ConstructorFact {
            name: format!("{name}.{selector}"),
            signature: format!(
                "{name}.{selector}{}",
                crate::query::render_formals(
                    container.ctor_formals.get(selector_id).map(Box::as_ref),
                    strings,
                    args
                )
            ),
        });
    }
    if container.has_unnamed_constructor {
        rows.push(ConstructorFact {
            name: name.to_owned(),
            signature: format!(
                "{name}{}",
                crate::query::render_formals(
                    container.ctor_formals.get(&sym).map(Box::as_ref),
                    strings,
                    args
                )
            ),
        });
    }
    rows
}

/// Whether `path` is a test-tree file — the selection order's first key
/// (#2621): any `test` path component (the directory convention) or a
/// `_test.dart` file name. A real-path property, read from the engine's
/// file table before the label projection can blur it.
fn is_test_path(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == OsStr::new("test"))
        || path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.ends_with("_test.dart"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;
    use crate::workspace::open_engine;
    use crate::workspace::test_fixture::workspace;

    /// One import chain plus its test: `animal.dart` declares `Animal`
    /// (member `breathe`), the `Loyal` mixin, and the `Pet` interface;
    /// `dog.dart` declares `Dog extends Animal with Loyal implements Pet`
    /// (member `bark`, an unnamed and a named constructor), `main.dart`
    /// imports `dog.dart` and constructs `Dog`, and `test/dog_test.dart`
    /// imports it by package URI.
    const ANIMAL: &str =
        "class Animal {\n  void breathe() {}\n}\nmixin Loyal {}\nabstract class Pet {}\n";
    const DOG: &str = "import 'animal.dart';\nclass Dog extends Animal with Loyal implements Pet {\n  void bark() {}\n  Dog();\n  Dog.named();\n}\n";
    const MAIN: &str = "import 'dog.dart';\nvoid main() {\n  var d = Dog();\n  d.bark();\n}\n";
    const TEST: &str = "import 'package:fixture/dog.dart';\nvoid main() {\n  var d = Dog();\n}\n";

    fn fixture(tag: &str) -> PathBuf {
        workspace(
            tag,
            &[
                ("lib/animal.dart", ANIMAL),
                ("lib/dog.dart", DOG),
                ("lib/main.dart", MAIN),
                ("test/dog_test.dart", TEST),
            ],
        )
    }

    /// The retired-slot twin on the explain face (#558): removing a file
    /// retires its library, the arena's slot count rises above the live
    /// count, and the substrate's label table answers anyway — the dead
    /// slot is flood-unreachable (the `Substrate::label` proof). Under the
    /// retired live-count sizing this panicked in `TestGraph::build`.
    #[test]
    fn explain_answers_after_a_library_retires() {
        let root = workspace(
            "explain_retired",
            &[
                ("lib/animal.dart", ANIMAL),
                ("lib/dog.dart", DOG),
                (
                    "test/animal_test.dart",
                    "import 'package:fixture/animal.dart';\nvoid main() {}\n",
                ),
            ],
        );
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        prelude(&mut engine).expect("analyze");

        // Retire dog's library; animal's and the test's stay live. The
        // disk file must leave FIRST — the prelude drives residency from
        // discovery, and a still-present file is re-settled (the pending
        // overlay is keyed by file, so a discovery Set replaces the Remove
        // and the library never retires — the #558 fixture defect).
        std::fs::remove_file(root.join("lib/dog.dart")).expect("delete dog.dart");
        engine
            .remove_file(&root.join("lib/dog.dart"))
            .expect("remove");
        prelude(&mut engine).expect("re-analyze");
        // The state guard: the retirement must actually have happened —
        // live libraries below the arena's slot count is the exact shape
        // the substrate's arena sizing exists for (a silent no-op
        // retirement would fail the dependency assertion vacuously).
        assert_eq!(
            engine.libraries().len(),
            2,
            "animal and the test library stay live"
        );
        assert_eq!(
            engine.library_import_graph().nodes(),
            3,
            "dog's arena slot stays retired-but-counted"
        );

        let json = explain_answer(&engine, "Animal", Verbosity::Compact).expect("explain answer");
        assert_eq!(json["sym"], "Animal");
        // The dependency rows enumerate the live importers only: dog's
        // library retired, the test library's row survives — the dead
        // slot's name (and its row) are gone, never a mislabel.
        assert_eq!(
            json["deps"],
            json!([{ "path": "test/animal_test.dart" }]),
            "the retired importer's row is gone, the live one intact"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The name token's start offset in `source` (ASCII sources, so byte
    /// offset = UTF-16 offset).
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    fn offset(source: &str, needle: &str) -> u32 {
        source
            .find(needle)
            .unwrap_or_else(|| panic!("{needle} in source")) as u32
    }

    /// End-to-end over the real engine + workspace fixture: the report
    /// validates against `Domain::Explain` and every section the substrate
    /// can reach carries its row — the declaration, the superclass, the
    /// member, both use sites (each naming its use file), the importing
    /// libraries, and the test library.
    #[test]
    fn explain_report_pins_the_sections_and_validates() {
        let root = fixture("explain");
        let json = explain(&root, Some(&root.join("sdk")), "Dog", Verbosity::Compact)
            .expect("explain run");
        // Every file column spells through the locator: root-relative
        // inside the analysis root (the lib and test trees alike), and the
        // envelope carries the root once, right after the version stamp.
        let root = root.canonicalize().expect("root");
        let animal = "lib/animal.dart".to_owned();
        let dog = "lib/dog.dart".to_owned();
        let main = "lib/main.dart".to_owned();
        let dog_test = "test/dog_test.dart".to_owned();

        // The entry routes through the published validator at the producing
        // tier; the assertion pins that the answer is born schema-valid.
        let text = serde_json::to_string(&json).expect("serialize");
        dartq_schema::validate_at_tier(
            &text,
            dartq_schema::Domain::Explain,
            dartq_schema::Verbosity::Compact,
        )
        .expect("schema-valid at the producing tier");

        assert_eq!(json["v"], 1);
        assert_eq!(json["root"], root.display().to_string());
        assert_eq!(json["sym"], "Dog");
        assert_eq!(
            json["declaration"],
            json!({
                "file": dog,
                "name": "Dog",
                "kind": "class",
                "off": offset(DOG, "Dog"),
                "len": 3,
            })
        );
        assert_eq!(
            json["bases"],
            json!([{"name": "Animal", "file": animal, "clause": "extends"}])
        );
        assert_eq!(
            json["ifaces"],
            json!([
                {"name": "Loyal", "file": animal, "clause": "with"},
                {"name": "Pet", "file": animal, "clause": "implements"},
            ]),
            "the with/implements edges, each naming its clause"
        );
        assert_eq!(
            json["ctors"],
            json!([
                {"name": "Dog", "kind": "constructor", "sig": "Dog()"},
                {"name": "Dog.named", "kind": "constructor", "sig": "Dog.named()"},
            ]),
            "the unnamed face under the container's own name, the selector under A.named"
        );
        assert_eq!(
            json["methods"],
            json!([{"name": "bark", "kind": "method", "off": offset(DOG, "bark"), "len": 4}])
        );
        assert_eq!(
            json["refs"],
            json!([
                {"file": main, "kind": "invoked", "off": offset(MAIN, "Dog"), "len": 3, "qualified": false},
                {"file": dog_test, "kind": "invoked", "off": offset(TEST, "Dog"), "len": 3, "qualified": false},
            ]),
            "each row names its use site's file, label order (`lib/` sorts before `test/`)"
        );
        assert_eq!(
            json["deps"],
            json!([{"path": main}, {"path": dog_test}]),
            "the libraries that import dog.dart, label order"
        );
        assert_eq!(json["tests"], json!([{"path": dog_test}]));
        assert_eq!(json["others"], json!([]), "one declaration, no candidates");

        std::fs::remove_dir_all(&root).ok();
    }

    /// `minimal` keeps the declaration locators and replaces every array
    /// with the `counts` object.
    #[test]
    fn minimal_truncates_to_declaration_and_counts() {
        let root = fixture("explain_minimal");
        let json = explain(&root, Some(&root.join("sdk")), "Dog", Verbosity::Minimal)
            .expect("explain run");
        let root = root.canonicalize().expect("root");

        assert_eq!(json["sym"], "Dog");
        assert_eq!(json["declaration"]["name"], "Dog", "the red line stays");
        assert!(json["declaration"].get("container").is_none());
        for section in [
            "bases", "ifaces", "ctors", "methods", "refs", "deps", "tests",
        ] {
            assert!(json.get(section).is_none(), "{section} is dropped");
        }
        assert_eq!(
            json["counts"],
            json!({
                "bases": 1,
                "ifaces": 2,
                "ctors": 2,
                "methods": 1,
                "refs": 2,
                "deps": 2,
                "tests": 1,
                "others": 0,
            })
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The birth gate is tier-exact, mutation-probed: a compact answer is
    /// born without `counts`, and the gate it passes through —
    /// `validate_at_tier` with the producing tier — rejects the
    /// minimal-only section a producer bug would inject (the validator
    /// tests' idiom: build the payload, expect the violation, name it).
    #[test]
    fn the_birth_gate_rejects_a_wrong_tier_section() {
        let root = fixture("explain_gate");
        let json = explain(&root, Some(&root.join("sdk")), "Dog", Verbosity::Compact)
            .expect("explain run");
        let mut mutated = json;
        assert!(
            mutated.get("counts").is_none(),
            "compact carries no counts section"
        );
        mutated["counts"] = json!({
            "bases": 1,
            "ifaces": 2,
            "ctors": 2,
            "methods": 1,
            "refs": 2,
            "deps": 2,
            "tests": 1,
            "others": 0,
        });
        let text = serde_json::to_string(&mutated).expect("serialize");
        let violation = dartq_schema::validate_at_tier(
            &text,
            dartq_schema::Domain::Explain,
            dartq_schema::Verbosity::Compact,
        )
        .expect_err("counts at compact violate the producing tier");
        assert!(
            violation.path.contains("counts"),
            "the minimal-only section is named: {violation}"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// A name that declares nowhere answers the seam report: no
    /// declaration, every section empty (never absent at compact).
    #[test]
    fn unknown_symbol_answers_the_empty_seam_report() {
        let root = fixture("explain_unknown");
        let json = explain(&root, Some(&root.join("sdk")), "Nope", Verbosity::Compact)
            .expect("explain run");

        assert_eq!(json["v"], 1);
        assert_eq!(json["sym"], "Nope");
        assert!(
            json.get("root").is_none(),
            "no declaration means no file field, so no locator root"
        );
        assert!(json.get("declaration").is_none());
        assert_eq!(json["bases"], json!([]));
        assert_eq!(json["ifaces"], json!([]));
        assert_eq!(json["ctors"], json!([]));
        assert_eq!(json["methods"], json!([]));
        assert_eq!(json["refs"], json!([]));
        assert_eq!(json["deps"], json!([]));
        assert_eq!(json["tests"], json!([]));
        assert_eq!(json["others"], json!([]));
        assert!(
            json.get("counts").is_none(),
            "counts are a minimal-tier field"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The #2621 shape: `createTask` declares in two public lib containers
    /// and in the test tree's private fake — `createTask` at line 2 of the
    /// lib file (the abstract repository), line 5 (the client), and line 3
    /// of the test file.
    const REPO_LIB: &str = "abstract class TodoRepository {\n  Future<void> createTask(String title);\n}\nclass TodoRepositoryClient implements TodoRepository {\n  Future<void> createTask(String title) {}\n}\n";
    const REPO_TEST: &str = "import 'package:fixture/todo_repository.dart';\nclass _FailingTodoRepository implements TodoRepository {\n  Future<void> createTask(String title) => throw 'boom';\n}\n";

    fn multi_declaration_fixture(tag: &str) -> PathBuf {
        workspace(
            tag,
            &[
                ("lib/todo_repository.dart", REPO_LIB),
                ("test/todo_repository_test.dart", REPO_TEST),
            ],
        )
    }

    /// The multi-declaration pick (#2621): a bare name shared by lib
    /// declarations and a test-tree fake answers the lib declaration —
    /// the test file loses on the selection order's first key, whatever
    /// its path sorts like — and `others` lists the remaining candidates
    /// in rank order, each as a `file`/`kind`/`container`/`line` locator.
    #[test]
    fn same_name_picks_the_lib_declaration_and_lists_the_rest() {
        let root = multi_declaration_fixture("explain_multi_decl");
        let json = explain(
            &root,
            Some(&root.join("sdk")),
            "createTask",
            Verbosity::Compact,
        )
        .expect("explain run");
        let repo = "lib/todo_repository.dart".to_owned();
        let test = "test/todo_repository_test.dart".to_owned();

        assert_eq!(json["sym"], "createTask");
        assert_eq!(json["declaration"]["file"], repo);
        assert_eq!(json["declaration"]["container"], "TodoRepository");
        assert_eq!(
            json["declaration"]["off"],
            offset(REPO_LIB, "createTask"),
            "the earliest lib declaration of the name"
        );
        assert_eq!(
            json["others"],
            json!([
                {"file": repo, "kind": "method", "container": "TodoRepositoryClient", "line": 5},
                {"file": test, "kind": "method", "container": "_FailingTodoRepository", "line": 3},
            ]),
            "rank order: the other lib declaration, then the test fake"
        );

        // The minimal tier swaps the array for the count.
        let minimal = explain(
            &root,
            Some(&root.join("sdk")),
            "createTask",
            Verbosity::Minimal,
        )
        .expect("minimal run");
        assert!(minimal.get("others").is_none(), "the array drops");
        assert_eq!(minimal["counts"]["others"], 2);

        std::fs::remove_dir_all(&root).ok();
    }

    /// The `Container.name` disambiguator (#2621): the spelling picks the
    /// named container's member out of the same-named set, the report
    /// echoes the caller's spelling, and a container without the member —
    /// or one that declares nowhere — answers the empty seam report, the
    /// unknown-symbol posture (never an error).
    #[test]
    fn container_qualified_symbol_picks_the_named_declaration() {
        let root = multi_declaration_fixture("explain_qualified");
        let json = explain(
            &root,
            Some(&root.join("sdk")),
            "TodoRepositoryClient.createTask",
            Verbosity::Compact,
        )
        .expect("explain run");
        let repo = "lib/todo_repository.dart".to_owned();

        assert_eq!(
            json["sym"],
            "TodoRepositoryClient.createTask",
            "the caller's spelling rides"
        );
        assert_eq!(json["declaration"]["file"], repo);
        assert_eq!(json["declaration"]["container"], "TodoRepositoryClient");
        assert_eq!(
            json["declaration"]["off"],
            offset(REPO_LIB, "createTask(String title) {}"),
            "the client's member, not the first same-named row"
        );
        assert_eq!(json["others"], json!([]), "the filter leaves one hit");

        for miss in ["TodoRepository.absent", "NoSuchClass.createTask"] {
            let json = explain(&root, Some(&root.join("sdk")), miss, Verbosity::Compact)
                .expect("the seam report, never an error");
            assert!(json.get("declaration").is_none(), "{miss}: no hit");
            assert_eq!(json["others"], json!([]), "{miss}: no candidates");
        }

        std::fs::remove_dir_all(&root).ok();
    }

    /// The #2655 member fixture: two same-file containers declaring the
    /// same member `m`, an overriding subtype of `A`, and one typed call
    /// site per receiver.
    const MEMBER_A: &str = concat!(
        "class A {\n",
        "  void m() {}\n",
        "}\n",
        "class B {\n",
        "  void m() {}\n",
        "}\n",
        "class Sub extends A {\n",
        "  void m() {}\n",
        "}\n"
    );
    const MEMBER_USE: &str = concat!(
        "import 'a.dart';\n",
        "void run(A a, B b, Sub s) {\n",
        "  a.m();\n",
        "  b.m();\n",
        "  s.m();\n",
        "}\n"
    );

    fn member_fixture(tag: &str) -> PathBuf {
        workspace(
            tag,
            &[("lib/a.dart", MEMBER_A), ("lib/main.dart", MEMBER_USE)],
        )
    }

    /// The qualified member report (#2655): `A.m` answers A's member as
    /// the declaration, A's call site plus the site resolved to the
    /// family's overriding member as `refs` (the override-closure
    /// widening), the container sections empty by nature, and B's site
    /// out. The miss spellings answer the empty seam report.
    #[test]
    fn qualified_member_report_is_the_members() {
        let root = member_fixture("explain_qualified_member");
        let json = explain(&root, Some(&root.join("sdk")), "A.m", Verbosity::Compact)
            .expect("explain run");

        assert_eq!(json["sym"], "A.m", "the caller's spelling rides");
        assert_eq!(json["declaration"]["file"], "lib/a.dart");
        assert_eq!(json["declaration"]["name"], "m");
        assert_eq!(json["declaration"]["kind"], "method");
        assert_eq!(json["declaration"]["container"], "A");
        assert_eq!(
            json["declaration"]["off"],
            offset(MEMBER_A, "void m()") + 5,
            "A's member, the first `m` in a.dart"
        );

        // The references: A's own site and the site statically resolved
        // to the family's overriding member — B's same-named member is
        // another container's, out of the answer.
        let refs = json["refs"].as_array().expect("references array");
        let sites: Vec<u64> = refs.iter().filter_map(|row| row["off"].as_u64()).collect();
        assert_eq!(
            sites,
            vec![
                u64::from(offset(MEMBER_USE, "a.m();") + 2),
                u64::from(offset(MEMBER_USE, "s.m();") + 2)
            ],
            "A's site and Sub's site (the family widening), never B's"
        );

        // The container sections are empty by nature for a member; the
        // library faces still answer (main.dart imports the declaring
        // library).
        for section in ["bases", "ifaces", "ctors", "methods", "others"] {
            assert_eq!(json[section], json!([]), "{section} is empty by nature");
        }
        assert_eq!(json["deps"], json!([{"path": "lib/main.dart"}]));
        assert_eq!(json["tests"], json!([]));

        // Born schema-valid at the producing tier.
        let text = serde_json::to_string(&json).expect("serialize");
        dartq_schema::validate_at_tier(
            &text,
            dartq_schema::Domain::Explain,
            dartq_schema::Verbosity::Compact,
        )
        .expect("schema-valid at the producing tier");

        // The miss spellings: the container without the member, the
        // container that declares nowhere — the empty seam report.
        for miss in ["A.absent", "Nope.m"] {
            let json = explain(&root, Some(&root.join("sdk")), miss, Verbosity::Compact)
                .expect("the seam report, never an error");
            assert!(json.get("declaration").is_none(), "{miss}: no hit");
            assert_eq!(json["refs"], json!([]), "{miss}: no references");
            assert_eq!(json["others"], json!([]), "{miss}: no candidates");
        }

        std::fs::remove_dir_all(&root).ok();
    }

    /// The constructor rows carry the captured formal face: required
    /// positional bare, the named group in braces with the `required`
    /// keyword, the implicit-default face rendered as the empty list.
    #[test]
    fn constructor_rows_carry_the_formal_face() {
        const CAT: &str =
            "class Cat {\n  Cat(String name, {int? lives});\n  Cat.kitten() : this('mew');\n}\n";
        let root = workspace("explain_ctor_formals", &[("lib/cat.dart", CAT)]);
        let json = explain(&root, Some(&root.join("sdk")), "Cat", Verbosity::Compact)
            .expect("explain run");
        assert_eq!(
            json["ctors"],
            json!([
                {"name": "Cat", "kind": "constructor", "sig": "Cat(String name, {int? lives})"},
                {"name": "Cat.kitten", "kind": "constructor", "sig": "Cat.kitten()"},
            ]),
            "the unnamed face's captured formals, the redirect's empty list"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    // -- the diagnostic face (`explain --diagnostic`) -------------------------

    /// Two unused imports in one file — `related` can count the same-code
    /// rows, and a `line`/`file` selector can discriminate them.
    const TWO_UNUSED: &str = "import 'dart:async';\nimport 'dart:io';\nvoid main() {}\n";

    /// The diagnostic explain report end to end: born schema-valid at the
    /// producing tier, the echoed row carries the canonical file and the
    /// `[start, end)` range, `why` names the registry facts, `related`
    /// counts every same-code row in the run, and `documentation` carries
    /// the published page (`unused_import` publishes one).
    #[test]
    fn diagnostic_explain_pins_the_sections_and_validates() {
        let root = workspace("explain_diag", &[("lib/unused.dart", TWO_UNUSED)]);
        let canonical = root.canonicalize().expect("root");
        let file = canonical.join("lib/unused.dart").display().to_string();

        let json = explain_diagnostic(
            &root,
            Some(&root.join("sdk")),
            "unused_import",
            None,
            Verbosity::Compact,
        )
        .expect("explain_diagnostic run");
        assert_eq!(json["diagnostic"]["code"], "unused_import");
        assert_eq!(json["diagnostic"]["sev"], "WARNING");
        assert_eq!(json["diagnostic"]["file"], file);
        assert_eq!(json["diagnostic"]["off"], 7);
        assert_eq!(
            json["diagnostic"]["len"],
            12,
            "the unused import's URI-string span (`'dart:async'`)"
        );
        assert_eq!(json["why"]["code_family"], "STATIC_WARNING");
        assert_eq!(json["why"]["trigger_kind"], "unused-family");
        assert_eq!(
            json["doc"],
            "https://dart.dev/diagnostics/unused_import"
        );
        assert_eq!(json["related"]["total"], 2, "two unused imports in the run");
        assert!(
            json["related"].get("lines").is_none(),
            "lines join at full only"
        );
        assert!(json.get("fixes").is_none(), "fixes join at full only");

        // Full tier: fixes and related.lines join.
        let json = explain_diagnostic(
            &root,
            Some(&root.join("sdk")),
            "unused_import",
            None,
            Verbosity::Full,
        )
        .expect("full run");
        assert!(json.get("fixes").is_some());
        assert_eq!(
            json["related"]["lines"],
            json!([2]),
            "the other same-code row's line"
        );

        // Minimal: the echo alone.
        let json = explain_diagnostic(
            &root,
            Some(&root.join("sdk")),
            "unused_import",
            None,
            Verbosity::Minimal,
        )
        .expect("minimal run");
        assert!(json.get("why").is_none());
        assert!(json.get("related").is_none());
        assert_eq!(json["diagnostic"]["code"], "unused_import");

        std::fs::remove_dir_all(&root).ok();
    }

    /// The `--file` selector narrows the row set to that file; the
    /// `:line[:col]` suffix picks the covering row; a code the run carries
    /// no row for (or a file/position with no hit) answers
    /// `NoSuchDiagnostic` — the CLI's usage tier.
    #[test]
    fn diagnostic_explain_file_and_position_selectors() {
        let root = workspace("explain_diag_sel", &[("lib/unused.dart", TWO_UNUSED)]);
        let canonical = root.canonicalize().expect("root");
        let file = canonical.join("lib/unused.dart");

        // The file selector alone picks the file's first same-code row.
        let selector = FileSelector {
            path: file.clone(),
            line: None,
            column: None,
        };
        let json = explain_diagnostic(
            &root,
            Some(&root.join("sdk")),
            "unused_import",
            Some(selector),
            Verbosity::Compact,
        )
        .expect("file selector run");
        assert_eq!(json["diagnostic"]["off"], 7);
        assert_eq!(json["diagnostic"]["len"], 12);

        // The line suffix picks the covering row (line 2 = the
        // `'dart:io'` URI literal).
        let selector = FileSelector {
            path: file.clone(),
            line: Some(2),
            column: None,
        };
        let json = explain_diagnostic(
            &root,
            Some(&root.join("sdk")),
            "unused_import",
            Some(selector),
            Verbosity::Compact,
        )
        .expect("line selector run");
        assert_eq!(json["diagnostic"]["off"], 28);
        assert_eq!(json["diagnostic"]["len"], 9);

        // A code with no row in the run → NoSuchDiagnostic.
        let error = explain_diagnostic(
            &root,
            Some(&root.join("sdk")),
            "no_such_code",
            None,
            Verbosity::Compact,
        )
        .expect_err("no such code");
        assert!(matches!(error, ExplainError::NoSuchDiagnostic(_)));

        std::fs::remove_dir_all(&root).ok();
    }
}
