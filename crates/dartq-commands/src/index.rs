//! The `dartq index` face: report the dartq-index symbol index
//! the analyzed workspace maintains, or — under `--full` — export it as a
//! JSONL document stream.
//!
//! # Build shape
//!
//! The index is the engine's maintained state, not a face-side rebuild:
//! analyze recaptures every changed file into the per-file captures and
//! rebuilds each live library's reference surface at its `LibraryAnalyzed`
//! point — the same state `query_symbols`
//! serves. This face tallies it through the engine's read hooks
//! ([`dartq_engine::Engine::file_capture`] /
//! [`dartq_engine::Engine::library_surfaces`]): no scan, no parse, no
//! second capture pass. The semantic fold plays no role in the counts, so
//! the face skips it (`semdiag = None`); the collector this face retired
//! skipped it the same way.
//!
//! # The export stream (`--full`)
//!
//! `dartq index --full` serializes the maintained state as JSONL — the
//! SCIP document model (upstream Sourcegraph SCIP) re-expressed as JSON
//! objects, so AI agents consume the stream directly (no protobuf, no new
//! dependency): a header line carrying the stream's row totals, then one
//! document per analyzed file ascending by canonical path. Each document
//! separates its symbol table (`symbols`: short id → `label|name`) from
//! the rows that reference it; rows inside the document cross-reference
//! the table by short id and name out-of-document targets by the full
//! `label|name`. The `-v/--verbosity` tier shapes the documents
//! (minimal = document list + symbol counts, compact = + symbol table and
//! declaration rows, full = + occurrence and supertype rows);
//! `Domain::IndexExport` is the published contract. Serialization walks
//! the maintained state once per pass — zero re-analysis, zero `String`
//! keys (the interned `StringId`s resolve straight through
//! [`dartq_engine::Engine::strings`]).

use std::io::Write;
use std::path::Path;

use dartq_engine::Engine;
use dartq_index::{RefKind, SuperClause, TargetRef};
use dartq_source::FileId;
use serde_json::json;
use thiserror::Error;

use crate::analyze::{AnalyzeError, prelude};
use crate::pkg_label::pkg_label;
use crate::workspace::{WorkspaceError, engine_config};

/// Why an index run aborts: the workspace assembly (usage tier) or the
/// analyze prelude (internal tier).
#[derive(Debug, Error)]
pub enum IndexError {
    /// The workspace faces are unusable (the CLI's usage tier).
    #[error("{0}")]
    Workspace(#[from] WorkspaceError),
    /// The engine context could not bind (the CLI's usage tier).
    #[error("engine context unusable: {0}")]
    Engine(String),
    /// Discovery failure or cancellation (the CLI's internal tier).
    #[error("{0}")]
    Analyze(#[from] AnalyzeError),
    /// The export stream could not be written (the CLI's internal tier).
    #[error("writing the index export failed: {0}")]
    Io(#[from] std::io::Error),
}

/// What one index run built: the analyzed workspace's row totals.
///
/// `symbols` counts declaration rows (every `FileIndex` entry);
/// `references` counts resolved reference rows (`LibraryIndex::refs` — the
/// rows `query symbol`'s reference surface serves). The unresolved-names
/// table is the recovery surface, not the reference surface, and is not
/// counted; supertype edges are a separate table and not counted.
///
/// Tier-invariant (the canonical verbosity tables): every field is a
/// verdict count — no locator, no prose, nothing to trim — so the CLI's
/// `--verbosity` parses for grammar parity and shapes nothing here.
#[derive(Debug, Default, Clone, Copy)]
pub struct IndexRun {
    /// Member files captured into the index.
    pub files: usize,
    /// Libraries whose reference surfaces were built.
    pub libraries: usize,
    /// Declaration rows indexed.
    pub symbols: usize,
    /// Resolved reference rows indexed.
    pub references: usize,
}

impl IndexRun {
    /// The JSON face: one object — `v` plus the row totals (the
    /// short-vocabulary convention). The shape is
    /// dartq-schema's index-run document (v1); the writer stamps the shared
    /// [`crate::SCHEMA_VERSION`] because the document landed after this
    /// writer — the values are equal at v1, so the payloads conform (the
    /// writer-alignment note the schema crate records).
    ///
    /// The report is schema-valid at birth: the payload is routed through
    /// `dartq_schema::validate(payload, Domain::Index)` right here — the
    /// producer-side gate the explain face holds, shared by the CLI and the
    /// daemon (both call this shaper verbatim).
    ///
    /// # Panics
    ///
    /// Panics when the born payload violates the published schema — a
    /// producer bug, surfaced never clamped (the CLI's crash tier turns it
    /// into exit 4 with stdout kept empty; the daemon's read task contains
    /// it at its panic boundary).
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        let value = json!({
            "v": crate::SCHEMA_VERSION,
            "files": self.files,
            "libraries": self.libraries,
            "symbols": self.symbols,
            "refs": self.references,
        });
        let text = value.to_string();
        dartq_schema::validate(&text, dartq_schema::Domain::Index)
            .expect("the index run summary is schema-valid at birth");
        value
    }
}

/// One index run over the workspace at `dir`: assemble the engine, run the
/// discover+analyze prelude (the read-after-write the counts reflect), and
/// tally the maintained index state's row totals.
///
/// `full` is the grammar's `--full`: the flag's behavioral counterpart is
/// the export stream — [`run_export`] — so this summary path keeps only
/// the parameter for signature stability; every caller passes `false`.
///
/// # Panics
///
/// Panics on an engine-contract violation, surfaced never clamped: a live
/// library member without a capture is a program bug.
///
/// # Errors
///
/// [`IndexError`] on workspace-assembly, engine-bind, discovery, or
/// cancellation failure.
pub fn run(dir: &Path, sdk: Option<&Path>, full: bool) -> Result<IndexRun, IndexError> {
    // The export stream is the flag's behavior (module docs); bound it so
    // the signature stays honest — this path is the summary half.
    let _ = full;
    let (mut config, _dir) = engine_config(dir, sdk)?;
    // The counts read the maintained index state; the semantic fold's
    // diagnostics are discarded on this face, so the fold is skipped.
    config.semdiag = None;
    let mut engine = Engine::new(config).map_err(|error| IndexError::Engine(error.to_string()))?;
    prelude(&mut engine)?;

    // The row totals off the maintained state: member files and their
    // declaration rows (the captures), the live libraries and their
    // resolved reference rows (the surfaces).
    let libraries = engine.libraries();
    let mut files = 0;
    let mut symbols = 0;
    for (_, members) in &libraries {
        files += members.len();
        for &file in members {
            symbols += engine
                .file_capture(file)
                .expect("analyzed member")
                .index
                .len();
        }
    }
    let mut built = 0;
    let mut references = 0;
    for (_, surface) in engine.library_surfaces() {
        built += 1;
        references += surface.reference_count();
    }
    Ok(IndexRun {
        files,
        libraries: built,
        symbols,
        references,
    })
}

/// What one export run emitted: the header line's row totals.
///
/// `occurrences`/`supertypes` are the emitted row counts — zero at tiers
/// that do not emit those rows (the header omits the keys there, so the
/// counts are never both emitted-empty and reported-nonzero).
#[derive(Debug, Default, Clone, Copy)]
pub struct ExportRun {
    /// Document lines written (one per analyzed file).
    pub documents: usize,
    /// Declaration rows across every document.
    pub declarations: usize,
    /// Occurrence rows emitted (`full` tier only; zero below it).
    pub occurrences: usize,
    /// Supertype rows emitted (`full` tier only; zero below it).
    pub supertypes: usize,
}

/// `dartq index --full`: the maintained index state as a JSONL document
/// stream (module docs, "The export stream") — a header line then one
/// document per analyzed file, written to `out` in canonical-path order.
///
/// `tier` is the canonical verbosity contract (`crate::Verbosity`):
/// `minimal` documents carry `path` + `symbol_count`; `compact` adds the
/// short-id `symbols` table and `declarations`; `full` adds `occurrences`
/// and `supertypes`. Every line is validated at birth against
/// [`dartq_schema::Domain::IndexExport`] at its tier — a schema drift is a
/// producer bug, surfaced via panic (the crash tier turns it into exit 4
/// with stdout's already-written prefix; the daemon boundary contains it
/// the same way as the other faces).
///
/// # Panics
///
/// Panics on an engine-contract violation (a live library member without a
/// capture, a row set that broke the index's stored invariants) or a
/// schema drift — surfaced, never clamped.
///
/// # Errors
///
/// [`IndexError`] on workspace-assembly, engine-bind, discovery, or
/// cancellation failure, and [`IndexError::Io`] when `out` rejects a
/// write.
pub fn run_export(
    dir: &Path,
    sdk: Option<&Path>,
    tier: crate::Verbosity,
    out: &mut impl Write,
) -> Result<ExportRun, IndexError> {
    let (mut config, _dir) = engine_config(dir, sdk)?;
    config.semdiag = None;
    let mut engine = Engine::new(config).map_err(|error| IndexError::Engine(error.to_string()))?;
    prelude(&mut engine)?;

    // The document set: every member file of every live library, deduped
    // and ordered by canonical path bytes (the deterministic line order —
    // not the projected label, which two paths could share through
    // different packages).
    let mut documents: Vec<FileId> = Vec::new();
    let mut seen: rustc_hash::FxHashSet<FileId> = rustc_hash::FxHashSet::default();
    for (_, members) in engine.libraries() {
        for &file in &members {
            if seen.insert(file) {
                documents.push(file);
            }
        }
    }
    documents.sort_by(|&a, &b| {
        crate::layout::path_key(engine.file_table().path(a).expect("analyzed member"))
            .cmp(crate::layout::path_key(
                engine.file_table().path(b).expect("analyzed member"),
            ))
    });

    // The per-file row views over the library surfaces — grouped once per
    // library so each document filters nothing: refs/supertypes land under
    // their use-site file (`RefRow::file`, `SubtypeEdge::file`), which for
    // a multi-file library is the member the site sits in (#361).
    let mut occurrences: rustc_hash::FxHashMap<FileId, Vec<&dartq_index::RefRow>> =
        rustc_hash::FxHashMap::default();
    let mut supertypes: rustc_hash::FxHashMap<FileId, Vec<&dartq_index::SubtypeEdge>> =
        rustc_hash::FxHashMap::default();
    if tier == crate::Verbosity::Full {
        for (_, surface) in engine.library_surfaces() {
            for row in surface.refs() {
                occurrences.entry(row.file).or_default().push(row);
            }
            for edge in surface.supers() {
                supertypes.entry(edge.file).or_default().push(edge);
            }
        }
    }

    // Two passes over the same maintained state: the header counts rows the
    // stream is about to emit, so totals precede documents without
    // buffering the stream.
    let mut totals = ExportRun {
        documents: documents.len(),
        ..ExportRun::default()
    };
    for &file in &documents {
        totals.declarations += engine
            .file_capture(file)
            .expect("analyzed member")
            .index
            .len();
        totals.occurrences += occurrences.get(&file).map_or(0, Vec::len);
        totals.supertypes += supertypes.get(&file).map_or(0, Vec::len);
    }

    let mut header = json!({
        "v": crate::SCHEMA_VERSION,
        "documents": totals.documents,
        "declarations": totals.declarations,
    });
    if tier == crate::Verbosity::Full {
        header["occurrences"] = json!(totals.occurrences);
        header["supertypes"] = json!(totals.supertypes);
    }
    write_line(&header, tier, out)?;

    let strings = engine.strings();
    let packages = engine.packages();
    for &file in &documents {
        let label =
            pkg_label(packages, engine.file_table().path(file).expect("analyzed member"));
        let capture = engine.file_capture(file).expect("analyzed member");
        let decls = capture.index.decls();
        let mut document = json!({
            "v": crate::SCHEMA_VERSION,
            "path": label,
            "symbol_count": decls.len(),
        });
        if tier != crate::Verbosity::Minimal {
            // The document's short-id table: `s<k>` → `label|name`, in the
            // stored `(name, offset)` declaration order, plus a name → first
            // id probe for resolving in-document targets back to their ids.
            let mut symbols = serde_json::Map::new();
            let mut local: rustc_hash::FxHashMap<dartq_source::StringId, String> =
                rustc_hash::FxHashMap::default();
            for (k, row) in decls.iter().enumerate() {
                let id = format!("s{}", k + 1);
                symbols.insert(
                    id.clone(),
                    json!(format!("{label}|{}", strings.resolve(row.name))),
                );
                local.entry(row.name).or_insert(id);
            }
            let decl_rows: Vec<serde_json::Value> = decls
                .iter()
                .enumerate()
                .map(|(k, row)| {
                    let mut entry = json!({
                        "symbol": format!("s{}", k + 1),
                        "kind": crate::query::decl_kind_name(row.kind),
                        "name": strings.resolve(row.name),
                        "range": [row.range.offset, row.range.end()],
                    });
                    if row.container != dartq_index::DeclRow::NONE {
                        entry["container"] = json!(format!("s{}", row.container + 1));
                    }
                    entry
                })
                .collect();
            document["symbols"] = serde_json::Value::Object(symbols);
            document["declarations"] = json!(decl_rows);
            if tier == crate::Verbosity::Full {
                let mut rows: Vec<&dartq_index::RefRow> = occurrences
                    .get(&file)
                    .map_or_else(Vec::new, |rows| rows.clone());
                // The stored order is target-major; the document order is
                // use-site order (offset, end, kind, target) — the reading
                // an AI consumer scans top-down. Sort the raw rows before
                // string resolution so the comparator stays id-typed.
                rows.sort_by_key(|row| {
                    (
                        row.range.offset,
                        row.range.end(),
                        row.kind as u8,
                        row.target_file,
                        row.target_name,
                    )
                });
                let occs: Vec<serde_json::Value> = rows
                    .iter()
                    .map(|row| {
                        let mut entry = json!({
                            "symbol": target_ref(
                                file,
                                &local,
                                packages,
                                &engine,
                                row.target_file,
                                row.target_name,
                            ),
                            "kind": ref_kind_name(row.kind),
                            "range": [row.range.offset, row.range.end()],
                        });
                        if row.qualifier != dartq_source::StringId::NONE {
                            entry["qualifier"] = json!(strings.resolve(row.qualifier));
                        }
                        entry
                    })
                    .collect();
                let supers: Vec<serde_json::Value> = supertypes
                    .get(&file)
                    .map_or_else(Vec::new, |edges| {
                        edges
                            .iter()
                            .map(|edge| {
                                json!({
                                    "symbol": target_ref(
                                        file,
                                        &local,
                                        packages,
                                        &engine,
                                        file,
                                        edge.name,
                                    ),
                                    "clause": clause_name(edge.clause),
                                    "supertype": match edge.supertype {
                                        TargetRef::Decl(target, name) => target_ref(
                                            file,
                                            &local,
                                            packages,
                                            &engine,
                                            target,
                                            name,
                                        ),
                                        TargetRef::Name(name) => {
                                            strings.resolve(name).to_owned()
                                        }
                                    },
                                })
                            })
                            .collect()
                    });
                document["occurrences"] = json!(occs);
                document["supertypes"] = json!(supers);
            }
        }
        write_line(&document, tier, out)?;
    }
    out.flush()?;
    Ok(totals)
}

/// The entity-reference spelling of a declaration target: `s<k>` (this
/// document's short-id table) when the target declares in `document`'s own
/// file and the name has a table entry, the stable `label|name` otherwise.
fn target_ref(
    document: FileId,
    local: &rustc_hash::FxHashMap<dartq_source::StringId, String>,
    packages: &dartq_package::PackageTable,
    engine: &Engine,
    file: FileId,
    name: dartq_source::StringId,
) -> String {
    if file == document
        && let Some(id) = local.get(&name)
    {
        return id.clone();
    }
    format!(
        "{}|{}",
        pkg_label(
            packages,
            engine.file_table().path(file).expect("indexed target")
        ),
        engine.strings().resolve(name)
    )
}

/// Stable machine spelling of a stored reference kind — the five
/// `RefKind`s; `override` is a query-face widening and never appears here.
fn ref_kind_name(kind: RefKind) -> &'static str {
    match kind {
        RefKind::Referenced => "reference",
        RefKind::Read => "read",
        RefKind::Invoked => "invoked",
        RefKind::Written => "written",
        RefKind::ReadWritten => "read-written",
    }
}

/// Stable machine spelling of a supertype clause (`SuperClause` order:
/// `extends`/`with`/`on`/`implements`).
fn clause_name(clause: SuperClause) -> &'static str {
    match clause {
        SuperClause::Extends => "extends",
        SuperClause::With => "with",
        SuperClause::On => "on",
        SuperClause::Implements => "implements",
    }
}

/// Serializes one line, validates it at birth against the tier-exact
/// `Domain::IndexExport` contract (the producer-side gate — schema drift is
/// a producer bug, surfaced via `expect`), and writes it.
fn write_line(
    value: &serde_json::Value,
    tier: crate::Verbosity,
    out: &mut impl Write,
) -> Result<(), IndexError> {
    let text = serde_json::to_string(value).expect("export rows serialize");
    let schema_tier = match tier {
        crate::Verbosity::Minimal => dartq_schema::Verbosity::Minimal,
        crate::Verbosity::Compact => dartq_schema::Verbosity::Compact,
        crate::Verbosity::Full => dartq_schema::Verbosity::Full,
    };
    dartq_schema::validate_at_tier(&text, dartq_schema::Domain::IndexExport, schema_tier)
        .expect("an export line is schema-valid at birth");
    out.write_all(text.as_bytes())?;
    out.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::test_fixture::workspace;

    /// Three libraries over one imported declaration surface (the same
    /// fixture the query-face tests pin): a.dart declares 3 rows, b.dart and
    /// c.dart construct `Foo` cross-file and b.dart calls `top`. Hand-traced
    /// totals: files 3, libraries 3, symbols 3+1+1, references 2 (b.dart) +
    /// 1 (c.dart).
    #[test]
    fn counts_pin_the_fixture_totals() {
        let root = workspace(
            "index",
            &[
                (
                    "lib/a.dart",
                    "class Foo {\n  void meth() {}\n}\nvoid top() {}\n",
                ),
                (
                    "lib/b.dart",
                    "import 'a.dart';\nvoid main() {\n  var f = Foo();\n  top();\n}\n",
                ),
                (
                    "lib/c.dart",
                    "import 'a.dart';\nvoid other() {\n  var f2 = Foo();\n}\n",
                ),
            ],
        );
        let run = run(&root, Some(&root.join("sdk")), true).expect("index run");
        assert_eq!(run.files, 3);
        assert_eq!(run.libraries, 3);
        assert_eq!(run.symbols, 5, "3 decl rows in a.dart, 1 each in b/c");
        assert_eq!(run.references, 3, "2 resolved uses in b.dart, 1 in c.dart");

        let json = run.to_json();
        assert_eq!(json["v"], crate::SCHEMA_VERSION);
        assert_eq!(json["files"], 3);
        assert_eq!(json["libraries"], 3);
        assert_eq!(json["symbols"], 5);
        assert_eq!(json["refs"], 3);

        std::fs::remove_dir_all(&root).ok();
    }

    /// The AI-face golden (`goldens/ai/index_run.json`): the promoted run
    /// summary over the counts fixture, pinned against the live writer.
    /// The comparison rides the schema validate-at-birth gate inside
    /// [`to_json`]. Values derive from the pinned totals above; a mismatch
    /// is a regression, not a golden to edit.
    #[test]
    fn the_ai_golden_pins_the_run_summary() {
        let root = workspace(
            "index_golden",
            &[
                (
                    "lib/a.dart",
                    "class Foo {\n  void meth() {}\n}\nvoid top() {}\n",
                ),
                (
                    "lib/b.dart",
                    "import 'a.dart';\nvoid main() {\n  var f = Foo();\n  top();\n}\n",
                ),
                (
                    "lib/c.dart",
                    "import 'a.dart';\nvoid other() {\n  var f2 = Foo();\n}\n",
                ),
            ],
        );
        let run = run(&root, Some(&root.join("sdk")), true).expect("index run");
        let golden = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../goldens/ai/index_run.json");
        let expected: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&golden).expect("the index golden rides the repo"),
        )
        .expect("one JSON object");
        assert_eq!(run.to_json(), expected, "index golden drift");

        std::fs::remove_dir_all(&root).ok();
    }

    /// A workspace with no Dart files still reports a valid empty index.
    #[test]
    fn empty_workspace_counts_are_zero() {
        let root = workspace("index_empty", &[]);
        let run = run(&root, Some(&root.join("sdk")), false).expect("index run");
        assert_eq!(
            (run.files, run.libraries, run.symbols, run.references),
            (0, 0, 0, 0)
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// A missing `package_config.json` is no longer a workspace error
    /// (#759, the oracle `ContextLocator` floor): the assembly degrades to
    /// the single-package context over an empty `package:` map — the
    /// advisory is output, not refusal — so the run reports a valid empty
    /// index and the usage tier stays reserved for unusable pub faces.
    #[test]
    fn missing_package_config_degrades_to_an_empty_run() {
        let root = workspace("index_broken", &[]);
        std::fs::remove_file(root.join(".dart_tool/package_config.json")).ok();
        let run = run(&root, Some(&root.join("sdk")), false).expect("degraded run");
        assert_eq!(
            (run.files, run.libraries, run.symbols, run.references),
            (0, 0, 0, 0)
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The `--full` export over the same fixture the summary pins: a
    /// header line then one document per file, every line schema-valid at
    /// its tier, and the emitted rows matching the maintained state the
    /// summary counts (3 files, 5 declaration rows, 3 resolved uses, 1
    /// supertype edge).
    #[test]
    fn export_streams_schema_valid_documents_per_tier() {
        let root = workspace(
            "index_export",
            &[
                (
                    "lib/a.dart",
                    "class Foo {\n  void meth() {}\n}\nvoid top() {}\n",
                ),
                (
                    "lib/b.dart",
                    "import 'a.dart';\nvoid main() {\n  var f = Foo();\n  top();\n}\n",
                ),
                (
                    "lib/c.dart",
                    "import 'a.dart';\nclass Sub extends Foo {\n  void use() {\n    top();\n  }\n}\n",
                ),
            ],
        );
        let sdk = root.join("sdk");
        for tier in [
            crate::Verbosity::Minimal,
            crate::Verbosity::Compact,
            crate::Verbosity::Full,
        ] {
            let mut out: Vec<u8> = Vec::new();
            let totals = run_export(&root, Some(&sdk), tier, &mut out).expect("export run");
            assert_eq!(totals.documents, 3, "one document per member file");
            assert_eq!(totals.declarations, 6, "a=3, b=1, c=2 declaration rows");
            let text = String::from_utf8(out).expect("UTF-8 stream");
            let lines: Vec<&str> = text.lines().collect();
            assert_eq!(lines.len(), 1 + totals.documents, "header + documents");
            let schema_tier = match tier {
                crate::Verbosity::Minimal => dartq_schema::Verbosity::Minimal,
                crate::Verbosity::Compact => dartq_schema::Verbosity::Compact,
                crate::Verbosity::Full => dartq_schema::Verbosity::Full,
            };
            for line in &lines {
                dartq_schema::validate_at_tier(
                    line,
                    dartq_schema::Domain::IndexExport,
                    schema_tier,
                )
                .unwrap_or_else(|v| panic!("line failed {tier:?} validation: {v}\n{line}"));
            }
            let header: serde_json::Value =
                serde_json::from_str(lines[0]).expect("header parses");
            assert_eq!(header["documents"], 3);
            assert_eq!(header["declarations"], 6);
            match tier {
                crate::Verbosity::Full => {
                    assert_eq!(totals.occurrences, 4, "2 in b, 2 in c");
                    assert_eq!(totals.supertypes, 1, "Sub extends Foo");
                    assert_eq!(header["occurrences"], 4);
                    assert_eq!(header["supertypes"], 1);
                }
                _ => {
                    assert!(header.get("occurrences").is_none());
                    assert!(header.get("supertypes").is_none());
                }
            }
            // Documents ascend by canonical path: a < b < c under lib/.
            let doc_paths: Vec<String> = lines[1..]
                .iter()
                .map(|line| {
                    serde_json::from_str::<serde_json::Value>(line)
                        .expect("document parses")["path"]
                        .as_str()
                        .expect("path")
                        .to_owned()
                })
                .collect();
            assert_eq!(
                doc_paths,
                [
                    "package:fixture/a.dart",
                    "package:fixture/b.dart",
                    "package:fixture/c.dart"
                ]
            );
            if tier != crate::Verbosity::Minimal {
                continue;
            }
            // Minimal documents are the red line only.
            let doc: serde_json::Value =
                serde_json::from_str(lines[1]).expect("document parses");
            assert_eq!(doc["symbol_count"], 3);
            assert!(doc.get("symbols").is_none());
            assert!(doc.get("declarations").is_none());
        }
        std::fs::remove_dir_all(&root).ok();
    }

    /// Determinism: two runs over the same workspace emit byte-identical
    /// streams (the output-determinism release gate).
    #[test]
    fn export_is_byte_deterministic() {
        let root = workspace(
            "index_export_det",
            &[
                ("lib/a.dart", "class Foo {}\nvoid top() {}\n"),
                (
                    "lib/b.dart",
                    "import 'a.dart';\nvoid main() {\n  top();\n}\n",
                ),
            ],
        );
        let sdk = root.join("sdk");
        let mut first: Vec<u8> = Vec::new();
        let mut second: Vec<u8> = Vec::new();
        run_export(&root, Some(&sdk), crate::Verbosity::Full, &mut first).expect("run 1");
        run_export(&root, Some(&sdk), crate::Verbosity::Full, &mut second).expect("run 2");
        assert_eq!(first, second, "same input, same bytes");
        std::fs::remove_dir_all(&root).ok();
    }

    /// Tier monotonicity at the field level: every key a minimal document
    /// carries persists at compact, every compact key persists at full —
    /// the canonical table's monotone growth.
    #[test]
    fn export_tiers_grow_monotonically() {
        let root = workspace(
            "index_export_tiers",
            &[("lib/a.dart", "class Foo {}\n")],
        );
        let sdk = root.join("sdk");
        let keys = |tier: crate::Verbosity| -> Vec<String> {
            let mut out: Vec<u8> = Vec::new();
            run_export(&root, Some(&sdk), tier, &mut out).expect("export");
            let text = String::from_utf8(out).expect("UTF-8");
            let doc = text.lines().nth(1).expect("a document line");
            let mut keys: Vec<String> = serde_json::from_str::<serde_json::Value>(doc)
                .expect("document parses")
                .as_object()
                .expect("object")
                .keys()
                .cloned()
                .collect();
            keys.sort_unstable();
            keys
        };
        let minimal = keys(crate::Verbosity::Minimal);
        let compact = keys(crate::Verbosity::Compact);
        let full = keys(crate::Verbosity::Full);
        assert!(
            minimal.iter().all(|k| compact.contains(k)),
            "minimal ⊆ compact: {minimal:?} vs {compact:?}"
        );
        assert!(
            compact.iter().all(|k| full.contains(k)),
            "compact ⊆ full: {compact:?} vs {full:?}"
        );
        assert_eq!(minimal, ["path", "symbol_count", "v"]);
        assert!(
            compact.contains(&"declarations".to_owned())
                && compact.contains(&"symbols".to_owned()),
            "compact adds the table and decl rows"
        );
        assert!(
            full.contains(&"occurrences".to_owned())
                && full.contains(&"supertypes".to_owned()),
            "full adds the ref and super rows"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
