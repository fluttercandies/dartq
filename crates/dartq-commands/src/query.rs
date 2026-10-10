//! The query face's core: the analyzed workspace's symbol index query
//! (PRD §20.3). The queried name's declaration rows plus resolved reference
//! rows, sorted by file path, then position — the printed contract.
//!
//! The PRD §15.2 query kinds over the current index: `references` serves
//! the same rows as `symbol` (the index's reference surface is name-keyed
//! and already kinded — reference/read/invoked/written/read-written, the
//! frozen v1 enum); `callers` is its invocation-shaped subset. The faces
//! diverge as the later semantics land (`implementations` waits on
//! #281 mixin/override; `references` gains the override typing there).
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

/// The `query callers` face (PRD §15.2): the invocation-shaped subset of the
/// references answer. A callers row is a reference row whose kind is
/// [`RefKind::Invoked`] — dartq-index's definition: "a call site,
/// constructor reference, or tear-off-shaped use".
///
/// # Direction
///
/// The index records use→target edges only (dartq-index `RefRow`: the
/// target's `(file, name)` plus the use site's span). A row therefore reads
/// "the use site at `range` invokes the queried declaration at
/// `target_file`" — the queried name is the callee and the use site is the
/// caller side; the caller is identified by its site span, not by a caller
/// declaration row (the index has no caller-declaration column).
///
/// The frozen v1 reference-row shape carries no use-file column — a gap
/// shared with the `query symbol` REF rows: single-file libraries are
/// unambiguous, while uses of one target from several member files leave
/// the use file implicit in the row. Widening the row is an additive schema
/// change owned by the schema/index lanes.
///
/// # Errors
///
/// [`AnalyzeError`] on discovery failure or cancellation.
pub fn callers<'a>(engine: &'a mut Engine, name: &str) -> Result<SymbolRun<'a>, AnalyzeError> {
    let _ = prelude(engine)?;
    Ok(callers_after_analysis(engine, name))
}

/// The callers answer over an already-analyzed engine — the daemon path
/// (see [`symbol_after_analysis`]).
#[must_use]
pub fn callers_after_analysis<'a>(engine: &'a Engine, name: &str) -> SymbolRun<'a> {
    let mut run = symbol_after_analysis(engine, name);
    run.refs.retain(|reference| reference.kind == RefKind::Invoked);
    run
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::workspace::{open_engine, test_fixture::workspace};

    /// Three libraries over one imported declaration surface: `Foo` is
    /// constructed cross-file and `top` is called. The locals use `var` (no
    /// type annotation) and no assignment statements appear: local
    /// annotation and assignment-site kinds are the parser lane's in-flight
    /// surface (the pinned symbol test's red), so these pins rest on the
    /// observed-stable shapes only.
    const A: &str = "class Foo {\n  void meth() {}\n}\nvoid top() {}\n";
    const B: &str = "import 'a.dart';\nvoid main() {\n  var f = Foo();\n  top();\n}\n";
    const C: &str = "import 'a.dart';\nvoid other() {\n  var f2 = Foo();\n}\n";

    fn fixture(tag: &str) -> PathBuf {
        workspace(
            tag,
            &[("lib/a.dart", A), ("lib/b.dart", B), ("lib/c.dart", C)],
        )
    }

    /// The `query callers` face: only the invocation-shaped rows survive —
    /// non-invocation uses would filter out of the references answer — while
    /// the declaration rows stay for target identity. UTF-16 offsets
    /// hand-traced off the sources above.
    #[test]
    fn callers_are_the_invoked_subset() {
        let root = fixture("callers");
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let a = root.canonicalize().expect("root").join("lib/a.dart");

        let run = callers(&mut engine, "Foo").expect("query");
        assert_eq!(run.decls.len(), 1, "the declaration rows stay");
        assert_eq!(run.decls[0].name, "Foo");
        let refs: Vec<(RefKind, u32)> = run
            .refs
            .iter()
            .map(|r| (r.kind, r.range.offset))
            .collect();
        assert_eq!(
            refs,
            vec![(RefKind::Invoked, 41), (RefKind::Invoked, 43)],
            "both constructor sites, one per use file"
        );
        for reference in &run.refs {
            assert_eq!(
                engine.file_table().path(reference.target_file),
                Some(a.as_path()),
                "rows target a.dart's Foo"
            );
        }

        let run = callers(&mut engine, "top").expect("query");
        assert_eq!(run.refs.len(), 1);
        assert_eq!(run.refs[0].kind, RefKind::Invoked);
        assert_eq!(run.refs[0].range.offset, 50);

        // A declaration with no uses: empty callers, declaration still listed.
        let run = callers(&mut engine, "meth").expect("query");
        assert!(run.refs.is_empty(), "no invocation of meth");
        assert_eq!(run.decls.len(), 1);

        fs_remove(&root);
    }

    /// The references answer (the symbol face's rows): every resolved use,
    /// kinded by the frozen enum — here the cross-file constructor sites and
    /// the `top()` call, all `invoked`.
    #[test]
    fn references_answer_is_kinded() {
        let root = fixture("references");
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");

        let run = symbol(&mut engine, "Foo").expect("query");
        let refs: Vec<(RefKind, u32)> = run
            .refs
            .iter()
            .map(|r| (r.kind, r.range.offset))
            .collect();
        assert_eq!(
            refs,
            vec![(RefKind::Invoked, 41), (RefKind::Invoked, 43)],
            "the constructor sites, offset order across the two use files"
        );

        let run = symbol(&mut engine, "top").expect("query");
        assert_eq!(run.refs.len(), 1);
        assert_eq!(run.refs[0].kind, RefKind::Invoked);
        assert_eq!(run.refs[0].range.offset, 50);

        fs_remove(&root);
    }

    fn fs_remove(root: &std::path::Path) {
        std::fs::remove_dir_all(root).ok();
    }
}
