//! The query face's core: the analyzed workspace's symbol index query
//! (PRD §20.3). The queried name's declaration rows plus resolved reference
//! rows, sorted by file path, then position — the printed contract.
//!
//! The JSON shaper [`to_json`] is the single implementation of the JSON
//! output face: the CLI's `--format=json` writes it verbatim and the
//! daemon's `dartq/query` compact verbosity returns it (plus
//! `schema_version`), so the two faces cannot drift.

use dartq_diagnostics::FileLayoutProvider as _;
use dartq_engine::{DeclHit, DeclKind, Engine, RefHit, RefKind, SymbolAnswer};
use serde_json::json;

use crate::analyze::{AnalyzeError, prelude};
use crate::layout::EngineLayout;

/// One symbol query's result: the sorted rows plus the layout their
/// positions resolve through.
pub struct SymbolRun<'a> {
    /// Every declaration named by the query, path-then-offset order.
    pub decls: Vec<DeclHit<'a>>,
    /// Every resolved reference to those declarations, path-then-offset
    /// order.
    pub refs: Vec<RefHit>,
    /// Position/path resolution over the analyzed engine.
    pub layout: EngineLayout<'a>,
}

/// Runs the query face over an assembled engine: the discover+analyze
/// prelude (the read-after-write the query reflects), then the index query.
///
/// # Errors
///
/// [`AnalyzeError`] on discovery failure or cancellation.
pub fn symbol<'a>(
    engine: &'a mut Engine,
    name: &str,
) -> Result<SymbolRun<'a>, AnalyzeError> {
    let _ = prelude(engine)?;
    Ok(symbol_after_analysis(engine, name))
}

/// The query over an already-analyzed engine — the daemon path, whose read
/// task has just run the analysis pass (running it twice would double the
/// read's cost). The rows borrow the engine's resident data.
#[must_use]
pub fn symbol_after_analysis<'a>(engine: &'a Engine, name: &str) -> SymbolRun<'a> {
    let SymbolAnswer { mut decls, mut refs } = engine.query_symbols(name);
    let layout = EngineLayout::over(
        engine,
        decls
            .iter()
            .map(|d| d.file)
            .chain(refs.iter().map(|r| r.target_file)),
    );

    // Output determinism: file path byte order, then position. The engine
    // returns id-ordered rows; path order is the printed contract.
    decls.sort_by(|a, b| {
        crate::layout::path_key(layout.file_path(a.file))
            .cmp(crate::layout::path_key(layout.file_path(b.file)))
            .then_with(|| a.range.offset.cmp(&b.range.offset))
    });
    refs.sort_by(|a, b| {
        crate::layout::path_key(layout.file_path(a.target_file))
            .cmp(crate::layout::path_key(layout.file_path(b.target_file)))
            .then_with(|| a.range.offset.cmp(&b.range.offset))
    });
    SymbolRun { decls, refs, layout }
}

/// The JSON face: one object — `schema_version`, `name`, and the
/// `declarations`/`references` arrays carrying the machine rows' fields
/// (snake_case, the metrics-JSON convention). Zero results produce empty
/// arrays. The envelope is the frozen v1 query contract (dartq-schema's
/// `Domain::Query`), which requires the `schema_version` key.
#[must_use]
pub fn to_json(run: &SymbolRun<'_>, name: &str) -> serde_json::Value {
    let declarations: Vec<serde_json::Value> = run
        .decls
        .iter()
        .map(|decl| {
            let (line, column) = run.layout.line_col(decl.file, decl.range.offset);
            json!({
                "file": run.layout.file_path(decl.file).display().to_string(),
                "container": decl.container,
                "kind": decl_kind_name(decl.kind),
                "name": decl.name,
                "line": line,
                "column": column,
                "length": decl.range.length,
            })
        })
        .collect();
    let references: Vec<serde_json::Value> = run
        .refs
        .iter()
        .map(|reference| {
            json!({
                "target_file": run.layout.file_path(reference.target_file).display().to_string(),
                "kind": ref_kind_name(reference.kind),
                "qualified": reference.qualified,
                "offset": reference.range.offset,
                "length": reference.range.length,
            })
        })
        .collect();
    json!({
        "schema_version": crate::SCHEMA_VERSION,
        "name": name,
        "declarations": declarations,
        "references": references,
    })
}

/// Stable machine spelling of a declaration family (the query face's own
/// grammar — not a diagnostics code).
#[must_use]
pub fn decl_kind_name(kind: DeclKind) -> &'static str {
    match kind {
        DeclKind::Class => "class",
        DeclKind::Enum => "enum",
        DeclKind::Mixin => "mixin",
        DeclKind::Extension => "extension",
        DeclKind::ExtensionType => "extension-type",
        DeclKind::TypeAlias => "type-alias",
        DeclKind::TopLevelFn => "function",
        DeclKind::TopLevelVar => "variable",
        DeclKind::TypeParam => "type-param",
        DeclKind::Method => "method",
        DeclKind::Getter => "getter",
        DeclKind::Setter => "setter",
        DeclKind::Field => "field",
    }
}

/// Stable machine spelling of a reference relation.
#[must_use]
pub fn ref_kind_name(kind: RefKind) -> &'static str {
    match kind {
        RefKind::Referenced => "reference",
        RefKind::Read => "read",
        RefKind::Invoked => "invoked",
        RefKind::Written => "written",
        RefKind::ReadWritten => "read-written",
    }
}
