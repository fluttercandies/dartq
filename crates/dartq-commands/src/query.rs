//! The query face's core: the analyzed workspace's symbol index query
//! (PRD §20.3). The queried name's declaration rows plus resolved reference
//! rows, sorted by file path, then position — the printed contract.
//!
//! The PRD §15.2 query kinds over the current index: `references` serves
//! the same rows as `symbol` (the index's reference surface is name-keyed
//! and already kinded — reference/read/invoked/written/read-written, the
//! frozen v1 enum); `callers` is its invocation-shaped subset;
//! `implementations` walks the index's resolved supertype edges in reverse
//! (the subtypes of the queried type, and the same-name member rows of
//! those subtypes for a queried member — the overrides the edge set can
//! see: `extends`/mixin-application heads, the clauses the capture
//! records); `dependencies` serves the declaring library's importers (the
//! frozen `DependencyRow` shape, the same rows the explain face's
//! `dependencies` section carries).
//!
//! The JSON shaper [`to_json`] is the single implementation of the JSON
//! output face: the CLI's `--format=json` writes it verbatim and the
//! daemon's `dartq/query` compact verbosity returns it (plus
//! `schema_version`), so the two faces cannot drift.
//!
//! The §15.8.4 batch face shares every piece of that: one batch answer
//! expands to the single faces' own runs, one per requested
//! `(symbol, kind)` pair, all walking one resident graph — the round trip
//! buys one graph build for N answers. There is no aggregate envelope (the
//! schema crate's frozen batch convention): each answer is the single
//! query's own JSON line, so batch and single output cannot drift.

// The dependencies row type (the frozen `DependencyRow` shape), re-exported
// so the faces name it in their signatures without a dartq-ai dependency.
pub use dartq_ai::explain::DependencyRow;
use dartq_ai::rev_import_graph::RevImportGraph;
use dartq_diagnostics::FileLayoutProvider as _;
use dartq_engine::{DeclHit, DeclKind, Engine, RefHit, RefKind, SymbolAnswer};
use dartq_index::{DeclRow, LibraryIndex, TargetRef};
use dartq_source::{FileId, StringId};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::Deserialize;
use serde_json::json;

use crate::analyze::{AnalyzeError, prelude};
use crate::impact::impact_substrate;
use crate::layout::EngineLayout;

/// One symbol query's result: the sorted rows plus the layout their
/// positions resolve through. The envelope is shared by every kind — the
/// kind-specific row sets (`implementations`, `dependencies`) stay empty
/// outside their own kinds, and [`to_json`] emits an array only when its
/// rows exist, so the symbol-family faces' bytes never change.
pub struct SymbolRun<'a> {
    /// Every declaration named by the query, path-then-offset order.
    pub decls: Vec<DeclHit<'a>>,
    /// Every resolved reference to those declarations, path-then-offset
    /// order.
    pub refs: Vec<RefHit<'a>>,
    /// The `implementations` kind's rows: the queried type's subtypes and
    /// the queried member's overriding rows, declaration-row typed (the
    /// frozen schema's reserved array), path-then-offset order.
    pub implementations: Vec<DeclHit<'a>>,
    /// The `dependencies` kind's rows: the declaring library's importers,
    /// the frozen `DependencyRow` shape (`path`), deduplicated ascending.
    pub dependencies: Vec<DependencyRow>,
    /// Position/path resolution over the analyzed engine.
    pub layout: EngineLayout<'a>,
}

/// Runs the query face over an assembled engine: the discover+analyze
/// prelude (the read-after-write the query reflects), then the index query.
///
/// # Errors
///
/// [`AnalyzeError`] on discovery failure or cancellation.
pub fn symbol<'a>(engine: &'a mut Engine, name: &str) -> Result<SymbolRun<'a>, AnalyzeError> {
    let _ = prelude(engine)?;
    Ok(symbol_after_analysis(engine, name))
}

/// The query over an already-analyzed engine — the daemon path, whose read
/// task has just run the analysis pass (running it twice would double the
/// read's cost). The rows borrow the engine's resident data.
#[must_use]
pub fn symbol_after_analysis<'a>(engine: &'a Engine, name: &str) -> SymbolRun<'a> {
    let SymbolAnswer {
        mut decls,
        mut refs,
    } = engine.query_symbols(name);
    let layout = EngineLayout::over(
        engine,
        decls
            .iter()
            .map(|d| d.file)
            .chain(refs.iter().map(|r| r.target_file)),
    );

    // Output determinism: file path byte order, then position. The engine
    // returns id-ordered rows; path order is the printed contract.
    sort_rows(&layout, &mut decls);
    refs.sort_by(|a, b| {
        crate::layout::path_key(layout.file_path(a.target_file))
            .cmp(crate::layout::path_key(layout.file_path(b.target_file)))
            .then_with(|| a.range.offset.cmp(&b.range.offset))
            // Use-file tiebreak: members of one library can carry equal
            // offsets, and the emitted `file` is part of the row's identity.
            .then_with(|| {
                crate::layout::path_key(layout.file_path(a.file))
                    .cmp(crate::layout::path_key(layout.file_path(b.file)))
            })
    });
    SymbolRun {
        decls,
        refs,
        implementations: Vec::new(),
        dependencies: Vec::new(),
        layout,
    }
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
/// The frozen v1 reference-row shape stayed frozen while gaining the
/// use-file answer: the index row carries the member file (`dartq-index`
/// `RefRow::file`, #361), [`RefHit`] propagates it, and [`to_json`] emits
/// it as the optional `file` string the schema accepts on reference rows
/// (dartq-schema `query.rs`).
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
    run.refs
        .retain(|reference| reference.kind == RefKind::Invoked);
    run
}

/// The `query implementations` face (PRD §15.2): given a type name, its
/// subtypes' declaration rows; given a member name, the overriding member
/// rows of the subtypes that declare it. Both walk the index's resolved
/// supertype edges in reverse — the clause set the capture records
/// (`extends` heads and mixin-application heads); `implements`/`with`/`on`
/// clauses have no edges in the index yet and ride no rows.
///
/// # Errors
///
/// [`AnalyzeError`] on discovery failure or cancellation.
pub fn implementations<'a>(
    engine: &'a mut Engine,
    name: &str,
) -> Result<SymbolRun<'a>, AnalyzeError> {
    let _ = prelude(engine)?;
    Ok(implementations_after_analysis(engine, name))
}

/// The implementations answer over an already-analyzed engine — the daemon
/// path (see [`symbol_after_analysis`]). The declaration rows stay for
/// target identity (the callers face's convention); the references array
/// is empty — use sites are not this kind's semantics.
#[must_use]
pub fn implementations_after_analysis<'a>(engine: &'a Engine, name: &str) -> SymbolRun<'a> {
    let mut run = symbol_after_analysis(engine, name);
    run.refs = Vec::new();

    let strings = engine.strings();
    let sym = strings.intern(name);

    // The queried name's declaring files, keyed by the face a supertype
    // edge can resolve to. An edge target is a library-scope name (the
    // bare-name resolution a clause head goes through), so a library-face
    // hit keys the subtype rows directly; a member hit keys its container
    // instead — the subtypes' same-name members are the overrides.
    let mut type_targets: FxHashSet<(FileId, StringId)> = FxHashSet::default();
    let mut member_containers: FxHashSet<(FileId, StringId)> = FxHashSet::default();
    for (_, files) in engine.libraries() {
        for &file in &files {
            let capture = engine.file_capture(file).expect("analyzed member");
            let Some(rows) = capture.index.by_name(sym) else {
                continue;
            };
            for row in rows {
                if row.is_library_face() {
                    type_targets.insert((file, sym));
                } else if row.container != DeclRow::NONE
                    && matches!(
                        row.kind,
                        DeclKind::Method | DeclKind::Getter | DeclKind::Setter | DeclKind::Field
                    )
                {
                    member_containers.insert((file, capture.index.row(row.container).name));
                }
            }
        }
    }

    // The reverse walk: every subtype container's resolved supertype edges
    // probed against the target sets (binary search per container row —
    // the index's CSR discipline, no linear scan). One row per matching
    // edge, so the same subtype cannot repeat.
    let surfaces: FxHashMap<u32, &LibraryIndex> = engine
        .library_surfaces()
        .map(|(library, surface)| (library.index(), surface))
        .collect();
    let mut rows: Vec<DeclHit<'a>> = Vec::new();
    for (library, files) in engine.libraries() {
        let surface = surfaces
            .get(&library.index())
            .expect("live library surface");
        for &file in &files {
            let capture = engine.file_capture(file).expect("analyzed member");
            for (index, container) in capture.index.decls().iter().enumerate() {
                // Only containers carry supertype clauses; everything else
                // has no edges to probe.
                if !matches!(
                    container.kind,
                    DeclKind::Class
                        | DeclKind::Mixin
                        | DeclKind::Enum
                        | DeclKind::Extension
                        | DeclKind::ExtensionType
                ) {
                    continue;
                }
                let Some(edges) = surface.supertypes_of(file, container.name) else {
                    continue;
                };
                for edge in edges {
                    let TargetRef::Decl(target_file, target_name) = edge.supertype else {
                        continue;
                    };
                    if type_targets.contains(&(target_file, target_name)) {
                        rows.push(DeclHit {
                            file,
                            kind: container.kind,
                            name: strings.resolve(container.name),
                            container: None,
                            range: container.range,
                        });
                    }
                    if member_containers.contains(&(target_file, target_name)) {
                        let anchor = u32::try_from(index).expect("more than u32::MAX rows");
                        for member in capture
                            .index
                            .decls()
                            .iter()
                            .filter(|row| row.container == anchor && row.name == sym)
                        {
                            rows.push(DeclHit {
                                file,
                                kind: member.kind,
                                name: strings.resolve(member.name),
                                container: Some(strings.resolve(container.name)),
                                range: member.range,
                            });
                        }
                    }
                }
            }
        }
    }

    run.layout = EngineLayout::over(
        engine,
        run.decls
            .iter()
            .map(|decl| decl.file)
            .chain(rows.iter().map(|row| row.file)),
    );
    sort_rows(&run.layout, &mut rows);
    run.implementations = rows;
    run
}

/// The `query dependencies` face (PRD §15.2): the declaring library's
/// importers — the reverse import graph the explain face's `dependencies`
/// section serves, on the frozen `DependencyRow` shape (`path`), the
/// primary (path-ascending first) declaration's library.
///
/// # Errors
///
/// [`AnalyzeError`] on discovery failure or cancellation.
pub fn dependencies<'a>(engine: &'a mut Engine, name: &str) -> Result<SymbolRun<'a>, AnalyzeError> {
    let _ = prelude(engine)?;
    Ok(dependencies_after_analysis(engine, name))
}

/// The dependencies answer over an already-analyzed engine — the daemon
/// path (see [`symbol_after_analysis`]). Declaration rows stay for target
/// identity; the references array is empty.
#[must_use]
pub fn dependencies_after_analysis<'a>(engine: &'a Engine, name: &str) -> SymbolRun<'a> {
    let mut run = symbol_after_analysis(engine, name);
    run.refs = Vec::new();
    let Some(primary) = run.decls.first() else {
        return run;
    };

    // The shared graph substrate (`impact`'s): one CSR projection, one
    // reverse build per call — the same per-query posture explain keeps.
    let libraries = engine.libraries();
    let substrate = impact_substrate(engine, &libraries);
    let importers = RevImportGraph::build(substrate.node_count(), |library| {
        substrate.neighbors(library)
    });
    let library = engine
        .library_of(primary.file)
        .expect("analyzed member")
        .index();
    let mut rows: Vec<DependencyRow> = importers
        .importers_of(library as usize)
        .iter()
        .map(|&importer| DependencyRow {
            path: substrate.label(importer).to_owned(),
        })
        .collect();
    rows.sort_by(|a, b| a.path.cmp(&b.path));
    rows.dedup_by(|a, b| a.path == b.path);
    run.dependencies = rows;
    run
}

/// Path-then-offset order over declaration rows — the printed contract's
/// sort, shared by the `declarations` and `implementations` arrays.
fn sort_rows<'a>(layout: &EngineLayout<'a>, rows: &mut [DeclHit<'a>]) {
    rows.sort_by(|a, b| {
        crate::layout::path_key(layout.file_path(a.file))
            .cmp(crate::layout::path_key(layout.file_path(b.file)))
            .then_with(|| a.range.offset.cmp(&b.range.offset))
    });
}

/// The query face's kind (PRD §15.2): which query answers. The lowercase
/// wire spelling is one surface — the daemon's `dartq/query` params and
/// batch items decode it through serde, the CLI's `--kind` values parse it
/// through [`std::str::FromStr`] — so the two faces cannot disagree on the
/// accepted set. Absent means [`QueryKind::Symbol`] — the face's pre-kind
/// behavior, byte-identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QueryKind {
    /// Declaration rows plus every resolved reference to them (the default).
    #[default]
    Symbol,
    /// The reference rows — the same rows the symbol face serves: the
    /// index's reference surface is name-keyed and already kinded, so there
    /// is nothing further to filter (the CLI references face's seam).
    References,
    /// The invocation-shaped subset of the reference rows (`invoked` only).
    Callers,
    /// The queried type's subtypes / the queried member's overriding rows,
    /// on the declaration-row shape (the frozen schema's reserved array).
    Implementations,
    /// The declaring library's importers, on the frozen `DependencyRow`
    /// shape (the reserved array, the explain section's rows).
    Dependencies,
}

impl std::str::FromStr for QueryKind {
    type Err = String;

    fn from_str(spelling: &str) -> Result<Self, Self::Err> {
        match spelling {
            "symbol" => Ok(Self::Symbol),
            "references" => Ok(Self::References),
            "callers" => Ok(Self::Callers),
            "implementations" => Ok(Self::Implementations),
            "dependencies" => Ok(Self::Dependencies),
            _ => Err(format!(
                "unknown query kind `{spelling}`; expected one of `symbol`, `references`, \
                 `callers`, `implementations`, `dependencies`"
            )),
        }
    }
}

/// The one kind→rows routing behind every query face: `symbol` and
/// `references` serve the same rows, `callers` the invocation-shaped
/// subset, `implementations` the reverse supertype walk, `dependencies`
/// the declaring library's importers — the same routing the single faces
/// have always had, now the one copy the batch face reuses.
#[must_use]
pub fn kind_after_analysis<'a>(engine: &'a Engine, name: &str, kind: QueryKind) -> SymbolRun<'a> {
    match kind {
        QueryKind::Symbol | QueryKind::References => symbol_after_analysis(engine, name),
        QueryKind::Callers => callers_after_analysis(engine, name),
        QueryKind::Implementations => implementations_after_analysis(engine, name),
        QueryKind::Dependencies => dependencies_after_analysis(engine, name),
    }
}

/// One §15.8.4 batch query item: the symbol name plus the kinds to answer.
/// The wire form is the daemon's `dartq/queryBatch` item
/// (`{"name": ..., "kinds": [...]}`, kinds defaulting to the symbol face);
/// the CLI's `dartq query batch` assembles the same items from repeated
/// `--symbol`/`--kind` flags.
#[derive(Debug, Clone)]
pub struct BatchQuery {
    /// The symbol name to look up (echoed into the answer envelope).
    pub name: String,
    /// The kinds to answer for this name, in request order. An empty set
    /// answers nothing for the item — the expansion is the contract (one
    /// answer per requested `(symbol, kind)` pair).
    pub kinds: Vec<QueryKind>,
}

/// One batch answer: the queried name plus the single-query run for one
/// `(item, kind)` pair — [`SymbolRun`] itself, not a new per-item shape,
/// so a batch answer and the single faces' answer cannot drift.
pub struct BatchAnswer<'run, 'name> {
    /// The queried name (the answer envelope's `name`); borrows the request
    /// items, zero-copy past the wire boundary.
    pub name: &'name str,
    /// The kind's own single-query rows over the shared graph.
    pub run: SymbolRun<'run>,
}

/// The §15.8.4 batch answer over a fresh engine: the discover+analyze
/// prelude once (the read-after-write every answer reflects), then every
/// `(item, kind)` pair answered over the one analyzed graph.
///
/// # Errors
///
/// [`AnalyzeError`] on discovery failure or cancellation.
pub fn batch<'run, 'name>(
    engine: &'run mut Engine,
    queries: &'name [BatchQuery],
) -> Result<Vec<BatchAnswer<'run, 'name>>, AnalyzeError> {
    let _ = prelude(engine)?;
    Ok(batch_after_analysis(engine, queries))
}

/// The batch answer over an already-analyzed engine — the daemon path,
/// whose read task has just run the analysis pass. Expansion is request
/// order: item order, then each item's kind order; every pair's run is the
/// single face's own answer for that kind ([`kind_after_analysis`]).
#[must_use]
pub fn batch_after_analysis<'run, 'name>(
    engine: &'run Engine,
    queries: &'name [BatchQuery],
) -> Vec<BatchAnswer<'run, 'name>> {
    let pairs = queries.iter().map(|query| query.kinds.len()).sum::<usize>();
    let mut answers = Vec::with_capacity(pairs);
    for query in queries {
        for kind in &query.kinds {
            answers.push(BatchAnswer {
                name: &query.name,
                run: kind_after_analysis(engine, &query.name, *kind),
            });
        }
    }
    answers
}

/// The JSON face: one object — `schema_version`, `name`, and the
/// `declarations`/`references` arrays carrying the machine rows' fields
/// (snake_case, the metrics-JSON convention). Zero results produce empty
/// arrays. The `implementations`/`dependencies` arrays ride the answer
/// only when their kind's rows exist (the frozen v1 reserved slots —
/// additive, optional, dropped by the daemon's minimal tier). The envelope
/// is the frozen v1 query contract (dartq-schema's `Domain::Query`), which
/// requires the `schema_version` key.
#[must_use]
pub fn to_json(run: &SymbolRun<'_>, name: &str) -> serde_json::Value {
    let mut json = json!({
        "schema_version": crate::SCHEMA_VERSION,
        "name": name,
        "declarations": declaration_rows(run, &run.decls),
        "references": reference_rows(run, &run.refs, name),
    });
    if !run.implementations.is_empty() {
        json["implementations"] = serde_json::to_value(declaration_rows(run, &run.implementations))
            .expect("declaration rows serialize");
    }
    if !run.dependencies.is_empty() {
        json["dependencies"] =
            serde_json::to_value(&run.dependencies).expect("dependency rows serialize");
    }
    json
}

/// The declaration-row shape, shared verbatim by the `declarations` and
/// `implementations` arrays (the frozen schema's one `DECLARATION_ITEM`).
fn declaration_rows(run: &SymbolRun<'_>, decls: &[DeclHit<'_>]) -> Vec<serde_json::Value> {
    decls
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
        .collect()
}

/// The reference-row shape: the frozen v1 contract plus the optional
/// use-file and qualified-name columns.
fn reference_rows(run: &SymbolRun<'_>, refs: &[RefHit<'_>], name: &str) -> Vec<serde_json::Value> {
    refs.iter()
        .map(|reference| {
            let mut row = json!({
                "target_file": run.layout.file_path(reference.target_file).display().to_string(),
                "kind": ref_kind_name(reference.kind),
                "file": run.layout.file_path(reference.file).display().to_string(),
                "offset": reference.range.offset,
                "length": reference.range.length,
            });
            // The §15.8 convenience column: the qualified-name spelling,
            // emitted only when an armed import prefix resolved the use
            // (the schema's optional string — bare rows keep the key
            // omitted). RefRows exist only for the queried name, so the
            // queried spelling completes the column.
            if let Some(prefix) = reference.qualified_name {
                row["qualified"] = json!(format!("{prefix}.{name}"));
            }
            row
        })
        .collect()
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
    /// A prefixed import plus use site: the §15.8 qualified-name column
    /// needs an armed import prefix to exercise the row (the shared
    /// fixture's imports are bare).
    const D: &str = "import 'a.dart' as p;\n\nvoid from_d() {\n  p.Foo();\n}\n";

    fn fixture(tag: &str) -> PathBuf {
        workspace(
            tag,
            &[("lib/a.dart", A), ("lib/b.dart", B), ("lib/c.dart", C)],
        )
    }

    /// The shared surface plus the prefixed use site (`lib/d.dart`).
    fn prefixed_fixture(tag: &str) -> PathBuf {
        workspace(
            tag,
            &[
                ("lib/a.dart", A),
                ("lib/b.dart", B),
                ("lib/c.dart", C),
                ("lib/d.dart", D),
            ],
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
        let refs: Vec<(RefKind, u32)> = run.refs.iter().map(|r| (r.kind, r.range.offset)).collect();
        assert_eq!(
            refs,
            vec![(RefKind::Invoked, 41), (RefKind::Invoked, 43)],
            "both constructor sites, one per use file"
        );
        // RefHit borrows the engine's interned spellings, so the rows' ids
        // are copied out before the table is consulted again.
        let targets: Vec<_> = run.refs.iter().map(|r| r.target_file).collect();
        for target in targets {
            assert_eq!(
                engine.file_table().path(target),
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
        let refs: Vec<(RefKind, u32)> = run.refs.iter().map(|r| (r.kind, r.range.offset)).collect();
        assert_eq!(
            refs,
            vec![(RefKind::Invoked, 41), (RefKind::Invoked, 43)],
            "the constructor sites, offset order across the two use files"
        );

        let run = symbol(&mut engine, "top").expect("query");
        assert_eq!(run.refs.len(), 1);
        assert_eq!(run.refs[0].kind, RefKind::Invoked);
        assert_eq!(run.refs[0].range.offset, 50);

        // The use-file column rides the emitted rows: `file` is the use
        // site's member path (b.dart, c.dart), distinct from the target's
        // (a.dart) — the frozen-v1 schema's optional reference-row string.
        let run = symbol(&mut engine, "Foo").expect("query");
        let json = to_json(&run, "Foo");
        let root = root.canonicalize().expect("root");
        let use_files: Vec<Option<&str>> = json["references"]
            .as_array()
            .expect("references array")
            .iter()
            .map(|row| row["file"].as_str())
            .collect();
        assert_eq!(
            use_files,
            vec![
                root.join("lib/b.dart").to_str(),
                root.join("lib/c.dart").to_str(),
            ],
            "each row names the member file its use site sits in"
        );

        fs_remove(&root);
    }

    /// The JSON face is the frozen v1 query contract: the real writer
    /// output passes the domain validator at the compact and full tiers the
    /// face claims, and the envelope version is the schema crate's pinned
    /// constant.
    #[test]
    fn json_face_passes_the_query_contract() {
        let root = prefixed_fixture("query_json");
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");

        let run = symbol(&mut engine, "Foo").expect("query");
        let json = to_json(&run, "Foo");
        assert_eq!(json["schema_version"], dartq_schema::SCHEMA_VERSION_QUERY);
        // The §15.8 qualified-name column rides armed-prefix rows only:
        // the bare sites keep the key omitted, the prefixed site spells
        // `<qualifier>.<name>` (the schema's optional string). Rows sort
        // by offset then use path: b (41), then c and d both 43 with the
        // path tiebreak.
        let qualified: Vec<Option<&str>> = json["references"]
            .as_array()
            .expect("references array")
            .iter()
            .map(|row| row["qualified"].as_str())
            .collect();
        assert_eq!(
            qualified,
            vec![None, None, Some("p.Foo")],
            "bare b, bare c, prefixed d"
        );
        let text = json.to_string();
        dartq_schema::validate(&text, dartq_schema::Domain::Query).expect("schema-valid");
        for tier in [
            dartq_schema::Verbosity::Compact,
            dartq_schema::Verbosity::Full,
        ] {
            dartq_schema::validate_at_tier(&text, dartq_schema::Domain::Query, tier)
                .expect("schema-valid at every tier the writer claims");
        }

        fs_remove(&root);
    }

    /// The batch fixture: the shared surface with an annotated use, so the
    /// symbol and callers kinds answer different row counts (`Foo? f = …`
    /// is a plain `reference`, the constructor site is `invoked`).
    const BATCH_B: &str = "import 'a.dart';\nvoid main() {\n  Foo? f = Foo();\n  top();\n}\n";

    /// The §15.8.4 expansion: 3 symbols × 2 kinds answer all 6 pairs in
    /// request order (item order, then kind order), each pair the single
    /// face's own rows over the one analyzed graph, each answer
    /// schema-valid under the frozen v1 query contract.
    #[test]
    fn batch_answers_every_pair_in_request_order() {
        let root = workspace("batch_order", &[("lib/a.dart", A), ("lib/b.dart", BATCH_B)]);
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let queries = vec![
            BatchQuery {
                name: "Foo".to_owned(),
                kinds: vec![QueryKind::Symbol, QueryKind::Callers],
            },
            BatchQuery {
                name: "top".to_owned(),
                kinds: vec![QueryKind::Symbol, QueryKind::Callers],
            },
            BatchQuery {
                name: "meth".to_owned(),
                kinds: vec![QueryKind::Symbol, QueryKind::Callers],
            },
        ];

        let answers = batch(&mut engine, &queries).expect("batch");
        assert_eq!(answers.len(), 6, "3 symbols x 2 kinds");
        let names: Vec<_> = answers.iter().map(|answer| answer.name).collect();
        assert_eq!(names, ["Foo", "Foo", "top", "top", "meth", "meth"]);
        // Foo: the symbol face keeps the annotation use plus the invocation;
        // the callers face keeps the invocation only.
        assert_eq!(answers[0].run.refs.len(), 2);
        assert_eq!(answers[1].run.refs.len(), 1);
        assert_eq!(answers[1].run.refs[0].kind, RefKind::Invoked);
        // top: one call site under both kinds; meth: declared, never used.
        assert_eq!(answers[2].run.refs.len(), 1);
        assert_eq!(answers[3].run.refs.len(), 1);
        assert_eq!(answers[4].run.refs.len(), 0);
        assert_eq!(answers[4].run.decls.len(), 1);
        for answer in &answers {
            let text = to_json(&answer.run, answer.name).to_string();
            dartq_schema::validate(&text, dartq_schema::Domain::Query).expect("schema-valid");
        }

        fs_remove(&root);
    }

    /// An empty batch answers nothing — no items, no results, no error.
    #[test]
    fn empty_batch_answers_nothing() {
        let root = fixture("batch_empty");
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        assert!(batch(&mut engine, &[]).expect("batch").is_empty());
        fs_remove(&root);
    }

    /// An unknown symbol answers the single-query face's own not-found
    /// shape (empty arrays under the echoed name), not an error.
    #[test]
    fn unknown_symbol_is_the_single_faces_not_found_shape() {
        let root = fixture("batch_unknown");
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let queries = vec![BatchQuery {
            name: "Nope".to_owned(),
            kinds: vec![QueryKind::Symbol],
        }];

        let json = {
            let answers = batch(&mut engine, &queries).expect("batch");
            assert_eq!(answers.len(), 1);
            to_json(&answers[0].run, answers[0].name)
        };
        let single = {
            let run = symbol_after_analysis(&engine, "Nope");
            to_json(&run, "Nope")
        };
        assert_eq!(
            json, single,
            "the not-found shape is the single-query face's own"
        );
        assert_eq!(json["name"], "Nope");
        assert_eq!(json["declarations"].as_array().expect("arrays").len(), 0);
        assert_eq!(json["references"].as_array().expect("arrays").len(), 0);

        fs_remove(&root);
    }

    /// The inheritance fixture: `Animal` declares the abstract member
    /// `breathe`, `Dog extends Animal` overrides it, `Cat extends Animal`
    /// does not, and the one import makes b the dependency face's importer
    /// row.
    const INHERIT_A: &str = "abstract class Animal {\n  void breathe();\n}\n";
    const INHERIT_B: &str = "import 'a.dart';\nclass Dog extends Animal {\n  void breathe() {}\n}\nclass Cat extends Animal {}\n";

    fn inherit_fixture(tag: &str) -> PathBuf {
        workspace(tag, &[("lib/a.dart", INHERIT_A), ("lib/b.dart", INHERIT_B)])
    }

    /// The `query implementations` face over a type name: the subtypes'
    /// declaration rows in path-then-offset order (Dog, then Cat), the
    /// declaration row kept for identity, no reference rows, and the
    /// answer's `implementations` array on the shared declaration-row
    /// shape, schema-valid at the tiers the writer claims.
    #[test]
    fn implementations_of_a_type_are_its_subtype_rows() {
        let root = inherit_fixture("impl_type");
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let b = root.canonicalize().expect("root").join("lib/b.dart");

        let run = implementations(&mut engine, "Animal").expect("query");
        assert_eq!(run.decls.len(), 1, "the declaration row stays for identity");
        assert_eq!(run.refs.len(), 0, "use sites are not this kind's rows");
        // The run borrows the engine, so everything it answers is copied
        // (and its JSON frozen) inside this block; the file-table asserts
        // follow once the borrow ends.
        let (subtypes, files, json) = {
            let subtypes: Vec<(&str, &str)> = run
                .implementations
                .iter()
                .map(|row| (row.name, decl_kind_name(row.kind)))
                .collect();
            let files: Vec<_> = run.implementations.iter().map(|row| row.file).collect();
            let json = to_json(&run, "Animal");
            (subtypes, files, json)
        };
        assert_eq!(
            subtypes,
            vec![("Dog", "class"), ("Cat", "class")],
            "both extends subtypes, path-then-offset order"
        );
        for file in files {
            assert_eq!(
                engine.file_table().path(file),
                Some(b.as_path()),
                "the subtypes declare in b.dart"
            );
        }
        let rows = json["implementations"].as_array().expect("array");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["name"], "Dog");
        assert_eq!(rows[0]["kind"], "class");
        assert_eq!(rows[0]["container"], serde_json::Value::Null);
        assert!(rows[0]["line"].as_i64().expect("line") >= 1);
        let text = json.to_string();
        dartq_schema::validate(&text, dartq_schema::Domain::Query).expect("schema-valid");
        for tier in [
            dartq_schema::Verbosity::Compact,
            dartq_schema::Verbosity::Full,
        ] {
            dartq_schema::validate_at_tier(&text, dartq_schema::Domain::Query, tier)
                .expect("schema-valid at every tier the writer claims");
        }

        fs_remove(&root);
    }

    /// The `query implementations` face over a member name: the overriding
    /// member rows of the subtypes that declare it — Dog's `breathe` rides,
    /// Cat (which overrides nothing) contributes no row, and the declaring
    /// abstract member stays a declaration row, not an implementation row.
    #[test]
    fn implementations_of_a_member_are_the_overriding_rows() {
        let root = inherit_fixture("impl_member");
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let b = root.canonicalize().expect("root").join("lib/b.dart");

        let run = implementations(&mut engine, "breathe").expect("query");
        assert_eq!(run.decls.len(), 2, "the declaring and overriding members");
        assert_eq!(run.implementations.len(), 1, "only Dog overrides breathe");
        let (name, container, file) = {
            let row = &run.implementations[0];
            (row.name, row.container, row.file)
        };
        assert_eq!(name, "breathe");
        assert_eq!(container, Some("Dog"));
        assert_eq!(
            engine.file_table().path(file),
            Some(b.as_path()),
            "the override declares in Dog, b.dart"
        );

        fs_remove(&root);
    }

    /// The `query dependencies` face: the declaring library's importers on
    /// the frozen `DependencyRow` shape — b.dart imports a.dart, so
    /// querying a.dart's `Animal` names b.dart's path, deduplicated
    /// ascending; the schema's reserved array carries the row.
    #[test]
    fn dependencies_rows_are_the_declaring_library_importers() {
        let root = inherit_fixture("deps");
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let b = root.canonicalize().expect("root").join("lib/b.dart");

        let run = dependencies(&mut engine, "Animal").expect("query");
        assert_eq!(run.decls.len(), 1, "the declaration row stays for identity");
        assert_eq!(run.refs.len(), 0);
        let paths: Vec<&str> = run
            .dependencies
            .iter()
            .map(|row| row.path.as_str())
            .collect();
        assert_eq!(
            paths,
            vec![b.to_str().expect("utf-8 path")],
            "the one importer, its defining-file path"
        );

        let json = to_json(&run, "Animal");
        let rows = json["dependencies"].as_array().expect("array");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0]["path"],
            json!(b.to_str().expect("utf-8 path")),
            "the frozen DependencyRow shape: `path` only"
        );
        let text = json.to_string();
        dartq_schema::validate(&text, dartq_schema::Domain::Query).expect("schema-valid");

        // A name whose library nothing imports (`Dog` declares in b.dart,
        // which nothing imports — unlike `breathe`, whose primary
        // declaration is a.dart's, and a.dart IS imported): the
        // dependencies array is absent (the schema's additive slots are
        // empty-row omitted), the envelope stays the frozen one.
        let run = dependencies(&mut engine, "Dog").expect("query");
        assert!(run.dependencies.is_empty());
        let json = to_json(&run, "Dog");
        assert!(json.get("dependencies").is_none());
        assert!(dartq_schema::validate(&json.to_string(), dartq_schema::Domain::Query).is_ok());

        fs_remove(&root);
    }

    /// An unknown name under the new kinds answers the established
    /// not-found shape — the symbol face's own empty envelope, byte-equal.
    #[test]
    fn new_kinds_unknown_name_is_the_not_found_shape() {
        let root = inherit_fixture("impl_deps_unknown");
        let (engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");

        for kind in [QueryKind::Implementations, QueryKind::Dependencies] {
            let run = kind_after_analysis(&engine, "Nope", kind);
            let json = to_json(&run, "Nope");
            let single = to_json(&symbol_after_analysis(&engine, "Nope"), "Nope");
            assert_eq!(json, single, "the not-found shape is the symbol face's own");
            assert_eq!(json["name"], "Nope");
            assert_eq!(json["declarations"].as_array().expect("arrays").len(), 0);
            assert_eq!(json["references"].as_array().expect("arrays").len(), 0);
        }

        fs_remove(&root);
    }

    /// The wire spellings parse through the one [`QueryKind`] surface (the
    /// CLI's `--kind` and the daemon's params share it), and an unknown
    /// spelling names the whole accepted set.
    #[test]
    fn query_kind_spellings_parse() {
        assert_eq!("symbol".parse::<QueryKind>(), Ok(QueryKind::Symbol));
        assert_eq!("references".parse::<QueryKind>(), Ok(QueryKind::References));
        assert_eq!("callers".parse::<QueryKind>(), Ok(QueryKind::Callers));
        assert_eq!(
            "implementations".parse::<QueryKind>(),
            Ok(QueryKind::Implementations)
        );
        assert_eq!(
            "dependencies".parse::<QueryKind>(),
            Ok(QueryKind::Dependencies)
        );
        let error = "subclasses".parse::<QueryKind>().unwrap_err();
        assert!(error.contains("`implementations`") && error.contains("`dependencies`"));
    }

    /// The batch expansion serves the new kinds for free: one item's
    /// `[implementations, dependencies]` answers both pairs over the one
    /// graph, in kind order, each answer schema-valid.
    #[test]
    fn batch_serves_the_new_kinds_through_the_expansion() {
        let root = inherit_fixture("batch_new_kinds");
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let queries = vec![BatchQuery {
            name: "Animal".to_owned(),
            kinds: vec![QueryKind::Implementations, QueryKind::Dependencies],
        }];

        let answers = batch(&mut engine, &queries).expect("batch");
        assert_eq!(answers.len(), 2, "one answer per (item, kind) pair");
        assert_eq!(answers[0].run.implementations.len(), 2, "Dog and Cat");
        assert_eq!(answers[1].run.dependencies.len(), 1, "the b.dart importer");
        for answer in &answers {
            let text = to_json(&answer.run, answer.name).to_string();
            dartq_schema::validate(&text, dartq_schema::Domain::Query).expect("schema-valid");
        }

        fs_remove(&root);
    }

    fn fs_remove(root: &std::path::Path) {
        std::fs::remove_dir_all(root).ok();
    }
}
