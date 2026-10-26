//! The query face's core: the analyzed workspace's symbol index query
//!. The queried name's declaration rows plus resolved reference
//! rows, sorted by file path, then position — the printed contract.
//!
//! The query kinds over the current index: `references` serves
//! the same rows as `symbol` (the index's reference surface is name-keyed
//! and already kinded — reference/read/invoked/written/read-written, the
//! frozen v1 enum, plus the query-time `override` rows the engine appends
//! for a queried member's overriding declarations); `callers` is its
//! invocation-shaped subset;
//! `implementations` walks the index's resolved supertype edges in reverse
//! (the subtypes of the queried type, and the same-name member rows of
//! those subtypes for a queried member — the overrides the resolved edge
//! set sees, every clause kind the capture records: `extends`,
//! `with`, `on`, `implements`); `dependencies` serves the declaring
//! library's own import edges and the dependency-library symbols its
//! resolved uses consume (the frozen `DependencyRow` shape — `path`,
//! plus the optional `name` column the forward direction carries). The
//! reverse question — who imports the declaring library — is the impact
//! face's answer (and the explain face's `dependencies` section), not
//! this kind's.
//!
//! The JSON shaper [`to_json`] is the single implementation of the JSON
//! output face: the CLI's `--format=json` writes it verbatim and the
//! daemon's `dartq/query` compact verbosity returns it (plus the `v`
//! envelope stamp), so the two faces cannot drift. [`to_json_at_tier`]
//! is the tier-aware spelling of the same envelope — `minimal` keeps the
//! red-line locators and drops the convenience spellings and the reserved
//! kind arrays, exactly the canonical tables' query rows; compact and full
//! answer today's shape.
//!
//! The batch face shares every piece of that: one batch answer
//! expands to the single faces' own runs, one per requested
//! `(symbol, kind)` pair, all walking one resident graph — the round trip
//! buys one graph build for N answers. There is no aggregate envelope (the
//! schema crate's frozen batch convention): each answer is the single
//! query's own JSON line, so batch and single output cannot drift.

// The dependencies row type (the frozen `DependencyRow` shape), re-exported
// so the faces name it in their signatures without a dartq-ai dependency.
pub use dartq_ai::explain::DependencyRow;
use dartq_diagnostics::FileLayoutProvider as _;
use dartq_engine::{DeclHit, DeclKind, Engine, RefHit, ReferenceKind, SymbolAnswer};
use dartq_index::{DeclRow, LibraryIndex, TargetRef};
use dartq_semantic::model::{
    ArgPool, Formal, Formals, MemberEntry, MemberKind, MicroKind, TopDecl, Ty,
};
use dartq_source::{FileId, StringId, StringTable, saturate_slot_u32};
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
    /// The `dependencies` kind's rows: the declaring library's own
    /// import edges and consumed dependency symbols, the frozen
    /// `DependencyRow` shape (`path`, plus the optional `name` column),
    /// ascending by `(path, name)`.
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

/// The `query callers` face: the invocation-shaped subset of the
/// references answer. A callers row is a reference row whose kind is
/// [`ReferenceKind::Invoked`] — dartq-index's definition: "a call site,
/// constructor reference, or tear-off-shaped use" (the `override` rows the
/// references answer appends for a queried member are declarations, not
/// invocations, and stay out; the engine's `Engine::query_callers` is the
/// declaration-projected face for symbol-level consumers).
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
        .retain(|reference| reference.kind == ReferenceKind::Invoked);
    run
}

/// The `query implementations` face: given a type name, its
/// subtypes' declaration rows; given a member name, the overriding member
/// rows of the subtypes that declare it. Both walk the index's resolved
/// supertype edges in reverse — every clause kind the capture records
/// (`extends`, `with`, `on`, `implements`; the index folds all four and
/// the walk probes them all, so a class implementing the queried
/// interface rides the same rows as one extending the queried
/// superclass).
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
///
/// # Panics
///
/// Panics when a library member lacks its analyzed capture — an internal
/// invariant breach (every resident member is analyzed).
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
                        let anchor = saturate_slot_u32(index);
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

/// The `query dependencies` face: what the declaring library itself
/// depends on — the forward direction. One row per workspace import edge
/// of the primary (path-ascending first) declaration's library, on the
/// frozen `DependencyRow` shape (`path`), plus one row per resolved use
/// of a dependency library's declaration (the `name` column) when the
/// library's own reference surface sees one. Edges whose targets are not
/// workspace libraries (the SDK's synthetic nodes, unanalyzed packages)
/// carry no row — the import CSR's node space is the live-library set.
/// The reverse direction — who imports the declaring library — is the
/// impact face's answer (and the explain `dependencies` section), not
/// this kind's.
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
///
/// # Panics
///
/// Panics when the primary declaration's library or surface is missing —
/// an internal invariant breach (the primary's file is analyzed and
/// resident).
#[must_use]
pub fn dependencies_after_analysis<'a>(engine: &'a Engine, name: &str) -> SymbolRun<'a> {
    let mut run = symbol_after_analysis(engine, name);
    run.refs = Vec::new();
    let Some(primary) = run.decls.first() else {
        return run;
    };

    // The shared graph substrate (`impact`'s): one CSR projection per
    // call — the same per-query posture the import faces keep. The
    // forward edges are the substrate's own neighbor lists; no reverse
    // build is needed.
    let libraries = engine.libraries();
    let substrate = impact_substrate(engine, &libraries);
    let library = engine
        .library_of(primary.file)
        .expect("analyzed member")
        .index();

    // The declaring library's own reference surface: every resolved use
    // site, with the target's declaring file and name. A use lands on a
    // dependency library when its `target_file` is one of the target's
    // member files (the import edges are library-level, the members the
    // file-level ones). One pass over the surface attributes every use to
    // its target's library through the member-file map.
    let surfaces: FxHashMap<u32, &LibraryIndex> = engine
        .library_surfaces()
        .map(|(library, surface)| (library.index(), surface))
        .collect();
    let surface = surfaces.get(&library).expect("live library surface");
    let mut file_library: FxHashMap<FileId, u32> = FxHashMap::default();
    for (library, files) in &libraries {
        for &file in files {
            file_library.insert(file, library.index());
        }
    }
    let mut consumed: FxHashMap<u32, Vec<StringId>> = FxHashMap::default();
    for reference in surface.refs() {
        if let Some(&target) = file_library.get(&reference.target_file) {
            consumed
                .entry(target)
                .or_default()
                .push(reference.target_name);
        }
    }

    // Distinct dependency targets, ascending by their slot id (the final
    // rows re-sort by label — the id order only dedups duplicated
    // directives).
    let mut targets = substrate.neighbors(library).to_vec();
    targets.sort_unstable();
    targets.dedup();

    let strings = engine.strings();
    let mut rows: Vec<DependencyRow> = Vec::new();
    for target in targets {
        let label = substrate.label(target).to_owned();
        // The consumed symbols, distinct ascending. A bare import edge
        // (no resolved use) answers the library-level row alone.
        let mut names: Vec<&str> = consumed
            .get(&target)
            .map(|ids| ids.iter().map(|&id| strings.resolve(id)).collect())
            .unwrap_or_default();
        names.sort_unstable();
        names.dedup();
        if names.is_empty() {
            rows.push(DependencyRow {
                path: label,
                name: None,
            });
        } else {
            rows.extend(names.into_iter().map(|name| DependencyRow {
                path: label.clone(),
                name: Some(name.to_owned()),
            }));
        }
    }
    rows.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.name.cmp(&b.name)));
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

/// The query face's kind: which query answers. The lowercase
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
    /// The declaring library's own dependencies — its import edges and
    /// the dependency-library symbols its resolved uses consume — on the
    /// frozen `DependencyRow` shape (the reserved array). The reverse
    /// direction (who consumes the declaring library) is the impact
    /// face's and the explain `dependencies` section's answer.
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
/// the declaring library's own import edges and consumed symbols — the
/// same routing the single faces have always had, now the one copy the
/// batch face reuses.
#[must_use]
pub fn kind_after_analysis<'a>(engine: &'a Engine, name: &str, kind: QueryKind) -> SymbolRun<'a> {
    match kind {
        QueryKind::Symbol | QueryKind::References => symbol_after_analysis(engine, name),
        QueryKind::Callers => callers_after_analysis(engine, name),
        QueryKind::Implementations => implementations_after_analysis(engine, name),
        QueryKind::Dependencies => dependencies_after_analysis(engine, name),
    }
}

/// One batch query item: the symbol name plus the kinds to answer.
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
    /// The queried name (the answer envelope's `sym`); borrows the request
    /// items, zero-copy past the wire boundary.
    pub name: &'name str,
    /// The kind's own single-query rows over the shared graph.
    pub run: SymbolRun<'run>,
}

/// The batch answer over a fresh engine: the discover+analyze
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

/// The JSON face: one object — `v`, `sym`, and the `decls`/`refs`
/// arrays carrying the machine rows' short-vocabulary fields. Zero
/// results produce empty arrays. The `implementations`/`deps` arrays ride
/// the answer only when their kind's rows exist (the frozen reserved
/// slots — additive, optional, dropped by the daemon's minimal tier). The
/// envelope is the query contract (dartq-schema's `Domain::Query`), which
/// requires the `v` stamp; the queried name rides `sym` (the decl rows'
/// own `name` field is a row locator and stays).
///
/// # Panics
///
/// Panics when a row array fails to serialize — an internal invariant
/// breach (the row shapes are schema-pinned and always serialize).
#[must_use]
pub fn to_json(run: &SymbolRun<'_>, name: &str) -> serde_json::Value {
    to_json_at_tier(run, name, crate::Verbosity::Compact)
}

/// The tier-aware spelling of [`to_json`] — the one implementation of the
/// query JSON face's verbosity axis. `minimal` is the canonical tables'
/// red line: the declaration rows keep `file` (the stable package-form
/// label)/`kind`/`name`/`line`/`col`/`len`, the reference rows keep
/// `target`/`kind`/`off`/`len`; the convenience spellings
/// (`container`, `qualified`, the use-file column, the element-face
/// `sig`/`annotations` columns) and the reserved kind arrays
/// (`implementations`, `deps`) drop. `compact` and `full` answer
/// [`to_json`] verbatim (the canonical tables list no full-only extras for
/// query today).
#[must_use]
pub fn to_json_at_tier(
    run: &SymbolRun<'_>,
    name: &str,
    verbosity: crate::Verbosity,
) -> serde_json::Value {
    if verbosity == crate::Verbosity::Minimal {
        return json!({
            "v": crate::SCHEMA_VERSION,
            "sym": name,
            "decls": minimal_declaration_rows(run, &run.decls),
            "refs": minimal_reference_rows(run, &run.refs),
        });
    }
    let mut json = json!({
        "v": crate::SCHEMA_VERSION,
        "sym": name,
        "decls": declaration_rows(run, &run.decls),
        "refs": reference_rows(run, &run.refs, name),
    });
    if !run.implementations.is_empty() {
        json["implementations"] = serde_json::to_value(declaration_rows(run, &run.implementations))
            .expect("declaration rows serialize");
    }
    if !run.dependencies.is_empty() {
        json["deps"] =
            serde_json::to_value(&run.dependencies).expect("dependency rows serialize");
    }
    json
}

/// The declaration-row shape, shared verbatim by the `decls` and
/// `implementations` arrays (the schema's one declaration-row contract),
/// plus the additive signature/annotation columns joined from the file's
/// resident element facts: a member row carries its rendered signature, a
/// container row its annotations — shapes the facts do not model (a
/// generic top-level function's unnamed type parameters, an unnamed
/// extension member) keep the keys omitted.
fn declaration_rows(run: &SymbolRun<'_>, decls: &[DeclHit<'_>]) -> Vec<serde_json::Value> {
    let engine = run.layout.engine();
    let packages = engine.packages();
    let label = |file| crate::pkg_label::pkg_label(packages, run.layout.file_path(file));
    decls
        .iter()
        .map(|decl| {
            let (line, column) = run.layout.line_col(decl.file, decl.range.offset);
            let mut row = json!({
                "file": label(decl.file),
                "kind": decl_kind_name(decl.kind),
                "name": decl.name,
                "line": line,
                "col": column,
                "len": decl.range.length,
            });
            // The convenience column is omit-on-absent: a top-level row has
            // no container, so the key drops rather than spelling a null.
            if let Some(container) = decl.container {
                row["container"] = json!(container);
            }
            let (signature, annotations) = declaration_extras(engine, decl);
            if let Some(signature) = signature {
                row["sig"] = json!(signature);
            }
            if !annotations.is_empty() {
                row["annotations"] = json!(annotations);
            }
            row
        })
        .collect()
}

/// The element-facts join behind the additive row columns: the signature
/// from the declaration's own face (a member's [`MemberEntry`], a
/// top-level executable's [`TopDecl`]) and the annotations from the
/// container face (the only face the model carries metadata on). A
/// declaration with no matching face — an unnamed extension member, a
/// type parameter — answers the bare row.
///
/// Public as the machine grammar's column source: the CLI's machine
/// printer spells the same pair as the JSON face's trailing `signature`/
/// `annotations` columns, so the two grammars cannot drift.
pub fn declaration_extras(engine: &Engine, decl: &DeclHit<'_>) -> (Option<String>, Vec<String>) {
    let facts = engine.file_facts(decl.file).expect("resident member facts");
    let strings = engine.strings();
    let elements = &facts.elements;
    let args = elements.type_args();
    match decl.container {
        // A named container's member: its own entry face.
        Some(container) => {
            let signature = elements
                .container(strings.intern(container))
                .and_then(|c| c.members.get(&strings.intern(decl.name)))
                .map(|entry| member_signature(entry, decl.name, strings, args));
            (signature, Vec::new())
        }
        // A top-level name: the executable face carries the signature; the
        // container face carries the annotations (container kinds only —
        // an unnamed extension member has no container face and its
        // name never aliases a top-level declaration probe).
        None => {
            let signature = matches!(decl.kind, DeclKind::TopLevelFn | DeclKind::TopLevelVar)
                .then(|| strings.intern(decl.name))
                .and_then(|name| elements.decl(name))
                .and_then(|top| top_signature(top, decl.name, strings, args));
            let annotations = matches!(
                decl.kind,
                DeclKind::Class
                    | DeclKind::Enum
                    | DeclKind::Mixin
                    | DeclKind::Extension
                    | DeclKind::ExtensionType
            )
            .then(|| strings.intern(decl.name))
            .and_then(|name| elements.container(name))
            .map(|c| {
                c.annotations
                    .iter()
                    .map(|annotation| annotation_text(annotation, strings))
                    .collect()
            })
            .unwrap_or_default();
            (signature, annotations)
        }
    }
}

/// The member's signature text — modifier keywords in written order, then
/// the type and parameter face (`static final int x`, `T pick<T>(T a)`,
/// `int get x`, `void set x(int v)`; setters render under their plain
/// display name, the lookup spelling's trailing `=` stripped).
fn member_signature(
    entry: &MemberEntry,
    name: &str,
    strings: &StringTable,
    args: &ArgPool,
) -> String {
    let mut text = String::new();
    if entry.is_external {
        text.push_str("external ");
    }
    if entry.is_static {
        text.push_str("static ");
    }
    if entry.is_abstract {
        text.push_str("abstract ");
    }
    if entry.is_final {
        text.push_str("final ");
    }
    match entry.kind {
        MemberKind::Method => {
            text.push_str(&render_ty(entry.return_ty, strings, args));
            text.push(' ');
            text.push_str(name);
            text.push_str(&type_params_text(&entry.type_param_names, strings));
            text.push_str(&render_formals(entry.formals.as_deref(), strings, args));
        }
        MemberKind::Getter => {
            text.push_str(&render_ty(entry.return_ty, strings, args));
            text.push_str(" get ");
            text.push_str(name);
        }
        MemberKind::Setter => {
            text.push_str(&render_ty(entry.return_ty, strings, args));
            text.push_str(" set ");
            text.push_str(name.strip_suffix('=').unwrap_or(name));
            text.push_str(&render_formals(entry.formals.as_deref(), strings, args));
        }
        MemberKind::Field => {
            text.push_str(&render_ty(entry.var_ty, strings, args));
            text.push(' ');
            text.push_str(name);
        }
    }
    text
}

/// The top-level declaration's signature text; `None` where the shape has
/// none (containers, aliases, type parameters, import prefixes) or the
/// model cannot render it fully (a generic function's type parameters
/// carry no names — a lossy signature is never emitted).
fn top_signature(
    decl: &TopDecl,
    name: &str,
    strings: &StringTable,
    args: &ArgPool,
) -> Option<String> {
    match decl.kind {
        MicroKind::TopLevelFn if decl.type_params == 0 => {
            let return_ty = render_ty(decl.return_ty, strings, args);
            Some(match decl.formals.as_deref() {
                // Setters are spelled `name=` in the lookup surface.
                _ if name.ends_with('=') => format!(
                    "{} set {}{}",
                    return_ty,
                    name.strip_suffix('=').unwrap_or(name),
                    render_formals(decl.formals.as_deref(), strings, args)
                ),
                // No captured parameter list is the getter shape.
                None => format!("{return_ty} get {name}"),
                Some(formals) => format!(
                    "{return_ty} {name}{}",
                    render_formals(Some(formals), strings, args)
                ),
            })
        }
        MicroKind::TopLevelVar => Some(format!("{} {name}", render_ty(decl.var_ty, strings, args))),
        _ => None,
    }
}

/// One annotation's text: `@` plus the spelled name and, when captured,
/// the verbatim argument group (`@Deprecated('use Other')`).
fn annotation_text(
    annotation: &dartq_semantic::model::Annotation,
    strings: &StringTable,
) -> String {
    let mut text = format!("@{}", strings.resolve(annotation.name));
    if let Some(args) = annotation.args_text {
        text.push_str(strings.resolve(args));
    }
    text
}

/// The member's own type-parameter list (`<T, U>`), empty when absent.
fn type_params_text(names: &[dartq_source::StringId], strings: &StringTable) -> String {
    if names.is_empty() {
        return String::new();
    }
    let joined: Vec<&str> = names.iter().map(|&id| strings.resolve(id)).collect();
    format!("<{}>", joined.join(", "))
}

/// Renders a type in the signature text's display form — the official
/// display (`List<int>?`) with the model's uncaptured shape spelled
/// `dynamic` (Dart's element display renders an undeclared type as
/// `dynamic`, never as an empty slot).
pub(crate) fn render_ty(ty: Ty, strings: &StringTable, args: &ArgPool) -> String {
    match ty {
        Ty::Unknown => "dynamic".to_owned(),
        Ty::Null => "Null".to_owned(),
        Ty::Dynamic => "dynamic".to_owned(),
        Ty::Never => "Never".to_owned(),
        Ty::Void => "void".to_owned(),
        // A record type renders in the model's own display form
        // (`(int, String)?`): the shape is canonical in `args`, and the
        // official `InvalidType` splice inside a recovered field is not
        // this face's `dynamic` convention. A function type renders the
        // same way (`int Function(String)?`).
        Ty::Record { .. } | Ty::Function { .. } => ty.render(strings, args),
        Ty::Named {
            name,
            nullable,
            args: list,
        } => {
            let mut text = strings.resolve(name).to_owned();
            let list = args.get(list);
            if !list.is_empty() {
                text.push('<');
                for (index, arg) in list.iter().enumerate() {
                    if index > 0 {
                        text.push_str(", ");
                    }
                    text.push_str(&render_ty(*arg, strings, args));
                }
                text.push('>');
            }
            if nullable {
                text.push('?');
            }
            text
        }
    }
}

/// Renders the parameter face: required positional bare, optional
/// positional in `[...]`, named in `{...}` with the `required` keyword
/// (`(int a, [int? b], {required String s})`). `None` — no captured
/// parameter list — renders the empty list.
pub(crate) fn render_formals(
    formals: Option<&Formals>,
    strings: &StringTable,
    args: &ArgPool,
) -> String {
    let Some(formals) = formals else {
        return "()".to_owned();
    };
    let formal_text = |formal: &Formal| {
        format!(
            "{} {}",
            render_ty(formal.ty, strings, args),
            strings.resolve(formal.name)
        )
    };
    // The positional slice runs required group first; the optional group
    // begins at the first non-required formal.
    let split = formals
        .positional
        .iter()
        .position(|formal| !formal.required)
        .unwrap_or(formals.positional.len());
    let mut groups: Vec<String> = Vec::with_capacity(3);
    let required = &formals.positional[..split];
    if !required.is_empty() {
        groups.push(
            required
                .iter()
                .map(formal_text)
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    let optional = &formals.positional[split..];
    if !optional.is_empty() {
        groups.push(format!(
            "[{}]",
            optional
                .iter()
                .map(formal_text)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !formals.named.is_empty() {
        groups.push(format!(
            "{{{}}}",
            formals
                .named
                .iter()
                .map(|formal| {
                    if formal.required {
                        format!("required {}", formal_text(formal))
                    } else {
                        formal_text(formal)
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    format!("({})", groups.join(", "))
}

/// The reference-row shape: the wire contract plus the optional
/// use-file and qualified-name columns. The `target`/`file` labels
/// are the stable package-form references (the shared
/// [`crate::pkg_label::pkg_label`] projection), the sort order above stays
/// keyed on canonical paths.
fn reference_rows(run: &SymbolRun<'_>, refs: &[RefHit<'_>], name: &str) -> Vec<serde_json::Value> {
    let packages = run.layout.engine().packages();
    let label = |file| crate::pkg_label::pkg_label(packages, run.layout.file_path(file));
    refs.iter()
        .map(|reference| {
            let mut row = json!({
                "target": label(reference.target_file),
                "kind": ref_kind_name(reference.kind),
                "file": label(reference.file),
                "off": reference.range.offset,
                "len": reference.range.length,
            });
            // The convenience column: the qualified-name spelling,
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

/// The minimal tier's declaration rows: the red-line locators only — the
/// stable package-form `file` label, `kind`, `name`, `line`, `col`,
/// `len` (the daemon's `dartq/query` minimal shaping, mirrored).
fn minimal_declaration_rows(run: &SymbolRun<'_>, decls: &[DeclHit<'_>]) -> Vec<serde_json::Value> {
    let packages = run.layout.engine().packages();
    let label = |file| crate::pkg_label::pkg_label(packages, run.layout.file_path(file));
    decls
        .iter()
        .map(|decl| {
            let (line, column) = run.layout.line_col(decl.file, decl.range.offset);
            json!({
                "file": label(decl.file),
                "kind": decl_kind_name(decl.kind),
                "name": decl.name,
                "line": line,
                "col": column,
                "len": decl.range.length,
            })
        })
        .collect()
}

/// The minimal tier's reference rows: `target`/`kind`/`off`/
/// `len` — the qualified spelling and the use-file column are the
/// compact tier's contribution (the daemon's minimal shaping, mirrored).
fn minimal_reference_rows(run: &SymbolRun<'_>, refs: &[RefHit<'_>]) -> Vec<serde_json::Value> {
    let packages = run.layout.engine().packages();
    let label = |file| crate::pkg_label::pkg_label(packages, run.layout.file_path(file));
    refs.iter()
        .map(|reference| {
            json!({
                "target": label(reference.target_file),
                "kind": ref_kind_name(reference.kind),
                "off": reference.range.offset,
                "len": reference.range.length,
            })
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

/// Stable machine spelling of a reference relation: the engine's stored
/// capture kinds map 1:1 onto the first five spellings, and the query-time
/// [`ReferenceKind::Override`] widening (the overriding-declaration rows
/// the references answer appends for a queried member) spells `override` —
/// the schema's sixth `REF_KINDS` entry.
#[must_use]
pub fn ref_kind_name(kind: ReferenceKind) -> &'static str {
    match kind {
        ReferenceKind::Referenced => "reference",
        ReferenceKind::Read => "read",
        ReferenceKind::Invoked => "invoked",
        ReferenceKind::Written => "written",
        ReferenceKind::ReadWritten => "read-written",
        ReferenceKind::Override => "override",
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
    /// A prefixed import plus use site: the qualified-name column
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
        let refs: Vec<(ReferenceKind, u32)> =
            run.refs.iter().map(|r| (r.kind, r.range.offset)).collect();
        assert_eq!(
            refs,
            vec![(ReferenceKind::Invoked, 41), (ReferenceKind::Invoked, 43)],
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
        assert_eq!(run.refs[0].kind, ReferenceKind::Invoked);
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
        let refs: Vec<(ReferenceKind, u32)> =
            run.refs.iter().map(|r| (r.kind, r.range.offset)).collect();
        assert_eq!(
            refs,
            vec![(ReferenceKind::Invoked, 41), (ReferenceKind::Invoked, 43)],
            "the constructor sites, offset order across the two use files"
        );

        let run = symbol(&mut engine, "top").expect("query");
        assert_eq!(run.refs.len(), 1);
        assert_eq!(run.refs[0].kind, ReferenceKind::Invoked);
        assert_eq!(run.refs[0].range.offset, 50);

        // The use-file column rides the emitted rows: `file` is the use
        // site's member label (b.dart, c.dart — the stable package-form
        // reference), distinct from the target's (a.dart) — the frozen-v1
        // schema's optional reference-row string.
        let run = symbol(&mut engine, "Foo").expect("query");
        let json = to_json(&run, "Foo");
        let use_files: Vec<Option<&str>> = json["refs"]
            .as_array()
            .expect("references array")
            .iter()
            .map(|row| row["file"].as_str())
            .collect();
        assert_eq!(
            use_files,
            vec![
                Some("package:fixture/b.dart"),
                Some("package:fixture/c.dart")
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
        assert_eq!(json["v"], dartq_schema::SCHEMA_VERSION_QUERY);
        // The qualified-name column rides armed-prefix rows only:
        // the bare sites keep the key omitted, the prefixed site spells
        // `<qualifier>.<name>` (the schema's optional string). Rows sort
        // by offset then use path: b (41), then c and d both 43 with the
        // path tiebreak.
        let qualified: Vec<Option<&str>> = json["refs"]
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

    /// The expansion: 3 symbols × 2 kinds answer all 6 pairs in
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
        assert_eq!(answers[1].run.refs[0].kind, ReferenceKind::Invoked);
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
        assert_eq!(json["sym"], "Nope");
        assert_eq!(json["decls"].as_array().expect("arrays").len(), 0);
        assert_eq!(json["refs"].as_array().expect("arrays").len(), 0);

        fs_remove(&root);
    }

    /// The inheritance fixture: `Animal` declares the abstract member
    /// `breathe`, `Dog extends Animal` overrides it, `Cat extends Animal`
    /// does not, and the one import makes a the dependency face's forward
    /// edge from b.
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
        // Top-level rows omit the container key (null/empty fields are
        // omitted, never spelled as null).
        assert!(!rows[0].as_object().expect("row").contains_key("container"));
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
        let (name, container, file, container_json) = {
            let row = &run.implementations[0];
            (
                row.name,
                row.container,
                row.file,
                to_json(&run, "breathe")["implementations"][0]["container"].clone(),
            )
        };
        assert_eq!(name, "breathe");
        assert_eq!(container, Some("Dog"));
        assert_eq!(
            engine.file_table().path(file),
            Some(b.as_path()),
            "the override declares in Dog, b.dart"
        );
        // The member row keeps the container key on the JSON face (the
        // omission rule applies to top-level rows only).
        assert_eq!(
            container_json,
            json!("Dog"),
            "a member row carries its container"
        );

        fs_remove(&root);
    }

    /// The `query dependencies` face, forward direction: the declaring
    /// library's own import edges plus the dependency-library symbols its
    /// resolved uses consume. Querying b.dart's `Dog` names the a.dart
    /// edge and the `Animal` use (`extends Animal` resolves there);
    /// querying a.dart's `Animal` — a library with no imports — answers
    /// the absent array (the additive slot's empty-row omission).
    #[test]
    fn dependencies_rows_are_the_declaring_library_own_imports() {
        let root = inherit_fixture("deps");
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");

        let run = dependencies(&mut engine, "Dog").expect("query");
        assert_eq!(run.decls.len(), 1, "the declaration row stays for identity");
        assert_eq!(run.refs.len(), 0);
        let rows: Vec<(&str, Option<&str>)> = run
            .dependencies
            .iter()
            .map(|row| (row.path.as_str(), row.name.as_deref()))
            .collect();
        assert_eq!(
            rows,
            vec![("package:fixture/a.dart", Some("Animal"))],
            "b.dart's import edge plus the consumed symbol"
        );

        let json = to_json(&run, "Dog");
        let deps = json["deps"].as_array().expect("array");
        assert_eq!(deps.len(), 1);
        assert_eq!(
            deps[0],
            json!({"path": "package:fixture/a.dart", "name": "Animal"}),
            "the DependencyRow shape: the joined library's path plus the consumed name"
        );
        dartq_schema::validate(&json.to_string(), dartq_schema::Domain::Query)
            .expect("schema-valid");

        // A name whose declaring library imports nothing: the
        // dependencies array is absent (the schema's additive slots are
        // empty-row omitted), the envelope stays the frozen one.
        let run = dependencies(&mut engine, "Animal").expect("query");
        assert!(run.dependencies.is_empty());
        let json = to_json(&run, "Animal");
        assert!(json.get("deps").is_none());
        assert!(dartq_schema::validate(&json.to_string(), dartq_schema::Domain::Query).is_ok());

        fs_remove(&root);
    }

    /// A bare import edge — one the declaring library's reference surface
    /// resolves no use through — answers the library-level row alone: the
    /// `path`, the `name` key omitted.
    #[test]
    fn a_dependency_with_no_consumed_symbol_is_the_bare_row() {
        let root = workspace(
            "deps_bare",
            &[
                ("lib/a.dart", "class Unused {}\n"),
                ("lib/b.dart", "class Leaf {}\n"),
                // c.dart imports both a.dart (whose `Unused` it uses) and
                // b.dart (a bare edge — `Leaf` is never named).
                (
                    "lib/c.dart",
                    "import 'a.dart';\nimport 'b.dart';\nclass C extends Unused {}\n",
                ),
            ],
        );
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");

        let run = dependencies(&mut engine, "C").expect("query");
        let rows: Vec<(&str, Option<&str>)> = run
            .dependencies
            .iter()
            .map(|row| (row.path.as_str(), row.name.as_deref()))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("package:fixture/a.dart", Some("Unused")),
                ("package:fixture/b.dart", None),
            ],
            "the consumed symbol rides its library, the bare edge stands alone"
        );
        let json = to_json(&run, "C");
        let deps = json["deps"].as_array().expect("array");
        assert_eq!(
            deps[0],
            json!({"path": "package:fixture/a.dart", "name": "Unused"})
        );
        assert_eq!(deps[1], json!({"path": "package:fixture/b.dart"}));
        dartq_schema::validate(&json.to_string(), dartq_schema::Domain::Query)
            .expect("schema-valid");

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
            assert_eq!(json["sym"], "Nope");
            assert_eq!(json["decls"].as_array().expect("arrays").len(), 0);
            assert_eq!(json["refs"].as_array().expect("arrays").len(), 0);
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
        assert!(
            answers[1].run.dependencies.is_empty(),
            "a.dart imports nothing — the forward direction's empty set"
        );
        for answer in &answers {
            let text = to_json(&answer.run, answer.name).to_string();
            dartq_schema::validate(&text, dartq_schema::Domain::Query).expect("schema-valid");
        }

        fs_remove(&root);
    }

    /// The signature-surface fixture: an annotated container (the
    /// annotation column's source), a member set covering every member
    /// kind and formal group, a top-level executable, and an unnamed
    /// extension whose member answers the bare row (no container face to
    /// join).
    const SIG_A: &str = concat!(
        "@Deprecated('use Other')\n",
        "class Widget {\n",
        "  static final int limit = 9;\n",
        "  int size = 0;\n",
        "  T pick<T>(T a, [int? b]) => a;\n",
        "  int get twice => 2;\n",
        "  void set twice(int v) {}\n",
        "  void named({required String s, int n = 0}) {}\n",
        "}\n",
        "int top(int a, {String? b}) => a;\n",
        "extension on int {\n",
        "  void ext() {}\n",
        "}\n",
    );

    /// The additive element-face columns: a container row carries its
    /// annotations (never a signature), a member row its rendered
    /// signature (never annotations), a top-level executable its own
    /// signature, and a shape without a matching face keeps both keys
    /// omitted — every shape born schema-valid.
    #[test]
    fn declaration_rows_carry_the_element_face_columns() {
        let root = workspace("query_signature", &[("lib/a.dart", SIG_A)]);
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");

        let row_for = |engine: &mut Engine, name: &str| {
            let run = symbol(engine, name).expect("query");
            let json = to_json(&run, name);
            let row = json["decls"]
                .as_array()
                .expect("decls array")
                .iter()
                .find(|row| row["name"] == json!(name))
                .unwrap_or_else(|| panic!("the queried declaration {name}: {json}"))
                .clone();
            assert!(
                dartq_schema::validate(&json.to_string(), dartq_schema::Domain::Query).is_ok(),
                "schema-valid: {name}"
            );
            row
        };

        // The container row: the annotation face, no signature.
        let row = row_for(&mut engine, "Widget");
        assert_eq!(
            row["annotations"],
            json!(["@Deprecated('use Other')"]),
            "the spelled name plus the verbatim argument group"
        );
        assert!(row.get("sig").is_none());

        // The members: every kind's rendered face, the modifier keywords
        // in written order, the formal groups in source shape.
        for (name, expected) in [
            ("limit", "static final int limit"),
            ("size", "int size"),
            ("pick", "T pick<T>(T a, [int? b])"),
            ("twice", "int get twice"),
            ("twice=", "void set twice(int v)"),
            ("named", "void named({required String s, int n})"),
        ] {
            let row = row_for(&mut engine, name);
            assert_eq!(row["sig"], json!(expected), "signature of {name}");
            assert!(row.get("annotations").is_none(), "{name} is a member");
        }

        // The top-level executable's own face; the unnamed extension
        // member keeps the bare row (no facts face answers it).
        let row = row_for(&mut engine, "top");
        assert_eq!(row["sig"], json!("int top(int a, {String? b})"));
        let row = row_for(&mut engine, "ext");
        assert_eq!(row["kind"], "method");
        assert!(row.get("sig").is_none(), "no face, no signature key");
        assert!(row.get("annotations").is_none());

        fs_remove(&root);
    }

    /// The minimal tier over the signature fixture: the element-face
    /// columns (`signature`, `annotations`) and the convenience spellings
    /// drop — the answer is tier-exact under the frozen contract's minimal
    /// check — while compact and full answer [`to_json`] verbatim.
    #[test]
    fn minimal_tier_keeps_the_red_line_only() {
        let root = workspace("query_minimal", &[("lib/a.dart", SIG_A)]);
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");

        // A member row (compact carries its signature) and a container row
        // (compact carries its annotations) both trim to the locators.
        for name in ["limit", "Widget"] {
            let run = symbol(&mut engine, name).expect("query");
            let minimal = to_json_at_tier(&run, name, crate::Verbosity::Minimal);
            dartq_schema::validate_at_tier(
                &minimal.to_string(),
                dartq_schema::Domain::Query,
                dartq_schema::Verbosity::Minimal,
            )
            .expect("tier-exact at minimal");
            assert!(
                minimal["decls"]
                    .as_array()
                    .expect("decls")
                    .iter()
                    .all(|row| row.get("container").is_none()
                        && row.get("sig").is_none()
                        && row.get("annotations").is_none()),
                "locators only: {name}"
            );
            assert!(minimal.get("implementations").is_none());
            assert!(minimal.get("deps").is_none());

            // Compact and full answer the frozen shape verbatim.
            assert_eq!(
                to_json_at_tier(&run, name, crate::Verbosity::Compact),
                to_json(&run, name)
            );
            assert_eq!(
                to_json_at_tier(&run, name, crate::Verbosity::Full),
                to_json(&run, name)
            );
        }

        fs_remove(&root);
    }

    /// The golden fixture's use file (goldens/ai/README.md): the
    /// constructor site lands at the shared fixtures' pinned offset 41.
    const GOLDEN_B: &str = "import 'a.dart';\nvoid main() {\n  var w = Widget();\n  top(1);\n}\n";
    /// The golden fixture's non-lib member: no `package:` URI addresses
    /// it, so its label is the canonical path — root-dependent, which is
    /// why the golden carries the `FIXTURE_ROOT` placeholder token.
    const GOLDEN_NOTE: &str = "import 'package:fixture/a.dart';\nWidget? probe() => null;\n";

    /// The AI-face goldens (`goldens/ai/query_symbol_{compact,minimal}
    /// .json`): the promoted envelopes over the golden fixture, pinned
    /// against live producer output. They carry #1560's element-face
    /// columns, #1556's package-form labels, and the non-lib canonical
    /// fallback (the `FIXTURE_ROOT` placeholder substituted with the
    /// canonical fixture root). Values derive from the pinned tests
    /// (goldens/ai/README.md); a mismatch is a regression, not a golden to
    /// edit.
    #[test]
    fn the_ai_goldens_pin_the_promoted_envelopes() {
        let root = workspace(
            "query_golden",
            &[
                ("lib/a.dart", SIG_A),
                ("lib/b.dart", GOLDEN_B),
                ("tool/note.dart", GOLDEN_NOTE),
            ],
        );
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let run = symbol(&mut engine, "Widget").expect("query");

        let goldens = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../goldens/ai");
        let load = |file: &str| {
            let raw = std::fs::read_to_string(goldens.join(file))
                .unwrap_or_else(|e| panic!("read goldens/ai/{file}: {e}"));
            let raw = raw.replace(
                "FIXTURE_ROOT",
                root.canonicalize()
                    .expect("canonical root")
                    .to_str()
                    .expect("UTF-8 root"),
            );
            serde_json::from_str(&raw).expect("one JSON object")
        };

        let compact: serde_json::Value = load("query_symbol_compact.json");
        assert_eq!(to_json(&run, "Widget"), compact, "compact golden drift");
        let minimal: serde_json::Value = load("query_symbol_minimal.json");
        assert_eq!(
            to_json_at_tier(&run, "Widget", crate::Verbosity::Minimal),
            minimal,
            "minimal golden drift"
        );

        fs_remove(&root);
    }

    fn fs_remove(root: &std::path::Path) {
        std::fs::remove_dir_all(root).ok();
    }
}
