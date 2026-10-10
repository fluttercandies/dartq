//! The `explain` query (PRD §15.3): the AI's one-stop symbol report — one
//! answer carrying the declaration, its supertypes, its members, its
//! reference sites, its library's importers, and the tests exercising it.
//!
//! # The query shape
//!
//! Like [`impact`](crate::impact), the query is a pure function over inputs
//! the caller owns — it never sees the engine or the storage. The inputs
//! are borrowed faces, each mapped to a report section:
//!
//! | Section           | Input face                                             |
//! |-------------------|--------------------------------------------------------|
//! | `declaration`     | the name's declaration hits (`SymbolDeclHit`)          |
//! | `inheritance`     | the primary declaration's supertype targets            |
//! | `methods`         | the declaring container's rows via the `decls_of` probe |
//! | `references`      | resolved [`RefRow`]s targeting the primary declaration |
//! | `dependencies`    | the declaring library's importers ([`RevImportGraph`]) |
//! | `affected_tests`  | the declaring library's [`TestGraph`] rows             |
//!
//! Two sections are honest seams — the substrate cannot fill them yet, so
//! they answer empty arrays (never placeholder rows):
//!
//! * `interfaces` — the capture records superclass edges only (`extends`
//!   and a named mixin application's head); `implements`/`with` clause
//!   edges are not captured. When they land, the rows join here.
//! * `constructors` — the member capture mints no constructor rows (the
//!   declaration kind set has no constructor kind). The section fills when
//!   the capture does.
//!
//! # Provenance and ordering
//!
//! Internal ids (`FileId`, [`StringId`], library ids) never leave the
//! engine boundary (PRD §15.7): every row carries stable path text through
//! the `file_label_of` / `label_of` closures and the [`StringTable`]. Every
//! array sorts by a documented key so the output is deterministic:
//!
//! * `declaration` — the primary declaration is the path-ascending first
//!   hit (a name declared in several files resolves to the first path; the
//!   remaining hits are the query face's domain).
//! * `inheritance`/`interfaces` — ascending by supertype name, then by
//!   target file path; unresolved rows (a supertype outside the analyzed
//!   set, no file) sort before resolved rows of the same name.
//! * `methods` — ascending by member name, then by range start. Fields,
//!   getters, and setters ride the array under their own `kind` (the
//!   §15.3 sketch has no separate member sections).
//! * `references` — ascending by use-site file path, then by range start.
//!   Rows for same-named declarations in other files are filtered out, so
//!   a caller may hand the whole by-name reference union.
//! * `dependencies` — ascending by library path, deduplicated. The rows
//!   are the **reverse** edges: the libraries that import the declaring
//!   library (the shared [`RevImportGraph`] CSR — who depends on the
//!   symbol, the same orientation `affected_tests` answers). Symbol-level
//!   forward narrowing (which imports feed this symbol's resolution)
//!   waits for the engine's resolution faces.
//! * `affected_tests` — ascending by test library path, deduplicated.
//!
//! # Verbosity (PRD §15.8)
//!
//! * `minimal` — `declaration` plus a `counts` object (one entry per
//!   section); the section arrays are dropped, never the locators.
//! * `compact` (default) — every section as an array, empty sections
//!   included (a missing section is indistinguishable from an empty one
//!   otherwise).
//! * `full` — equal to `compact` today: every field the substrate can
//!   reach is already a locator or a verdict, so there is nothing extra
//!   to inline. The tier exists in the contract for the faces that gain
//!   full-only content (deep member signatures, the #282 seam).
//!
//! # The kind spellings
//!
//! [`decl_kind_name`] / [`ref_kind_name`] map the index's enums to the
//! frozen v1 machine spellings — the same grammar `dartq-schema`'s
//! `DECL_KINDS`/`REF_KINDS` and the CLI's query face use. Each producer
//! maps its own enum; the spellings are the shared contract, not shared
//! code.

use serde::{Deserialize, Serialize};

use dartq_index::{DeclKind, DeclRow, RefKind, RefRow, TargetRef, TestGraph};
use dartq_source::{FileId, StringId, StringTable};

use crate::impact::AffectedTest;
use crate::rev_import_graph::RevImportGraph;

/// The report contract version (PRD §15.7): breaking output changes bump
/// the major.
pub const EXPLAIN_SCHEMA_VERSION: u32 = 1;

/// How much detail the report carries (PRD §15.1, §15.8.1). Default
/// [`Verbosity::Compact`].
///
/// Mirrors `dartq-commands`' `Verbosity` (this crate cannot see the
/// commands layer; the wiring maps one to the other — same three tiers,
/// same default).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verbosity {
    /// `declaration` plus per-section `counts`; the arrays are dropped.
    Minimal,
    /// Every section as an array (the default).
    #[default]
    Compact,
    /// Equal to [`Verbosity::Compact`] today (see the module docs).
    Full,
}

/// One declaration hit for the queried name: the index's by-name row plus
/// the library identity the graph faces need (the caller's `u32` id space).
#[derive(Clone, Copy, Debug)]
pub struct SymbolDeclHit<'a> {
    /// The declaring library, in the query's id space (`0..library_count`).
    pub library: u32,
    /// The declaring file.
    pub file: FileId,
    /// The declaration row (name, kind, span, container anchor).
    pub decl: &'a DeclRow,
}

/// The explain report (PRD §15.3), JSON contract `schema_version: 1`.
///
/// Section arrays are `Some` at [`Verbosity::Compact`] and
/// [`Verbosity::Full`] (empty sections included — the seam is visible),
/// `None` at [`Verbosity::Minimal`], where [`ExplainReport::counts`]
/// replaces them. `declaration` is `None` when the symbol is unknown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplainReport {
    /// The output contract version ([`EXPLAIN_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// The queried name, as spelled by the caller.
    pub symbol: String,
    /// The primary declaration; `None` when the name declares nowhere.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declaration: Option<DeclarationRow>,
    /// Direct supertypes (the `extends` clause / mixin application head).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inheritance: Option<Vec<SuperTypeRow>>,
    /// Implemented/mixed-in types — the documented empty seam (module
    /// docs, "Two sections are honest seams").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interfaces: Option<Vec<SuperTypeRow>>,
    /// Declared constructors — the documented empty seam (the capture
    /// mints no constructor rows yet).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub constructors: Option<Vec<MemberRow>>,
    /// The container's members: methods, getters, setters, and fields,
    /// kind-tagged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub methods: Option<Vec<MemberRow>>,
    /// Resolved use sites of the primary declaration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub references: Option<Vec<ReferenceRow>>,
    /// The libraries that import the declaring library (reverse edges).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<DependencyRow>>,
    /// The test libraries exercising the declaring library.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affected_tests: Option<Vec<AffectedTest>>,
    /// Per-section row counts (`minimal` only, replacing the arrays).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counts: Option<SectionCounts>,
}

/// The `declaration` row: where the symbol declares and what it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclarationRow {
    /// The declaring file's path.
    pub file: String,
    /// The declared name (lookup spelling: setters under `name=`).
    pub name: String,
    /// The declaration family (the frozen v1 spelling, [`decl_kind_name`]).
    pub kind: String,
    /// The enclosing container's name; absent for top-level rows and
    /// trimmed at `minimal` (a convenience field, never a locator).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    /// The name token's span, `[startOffset, endOffset)` (§15.8.5).
    pub range: [u32; 2],
}

/// One supertype row of `inheritance`/`interfaces`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuperTypeRow {
    /// The supertype's name.
    pub name: String,
    /// The supertype's declaring file when it resolves inside the
    /// analyzed set; absent for a bare name outside it (`dart:` SDK, an
    /// unanalyzed dependency).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

/// One member row of `methods` (constructors have no rows yet — the seam).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberRow {
    /// The member's name (lookup spelling).
    pub name: String,
    /// The member family: `method`, `getter`, `setter`, or `field`.
    pub kind: String,
    /// The name token's span, `[startOffset, endOffset)`.
    pub range: [u32; 2],
}

/// One reference row of `references`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenceRow {
    /// The use site's file (a multi-file library names the member file).
    pub file: String,
    /// The relation kind (the frozen v1 spelling, [`ref_kind_name`]).
    pub kind: String,
    /// The use site's span, `[startOffset, endOffset)`.
    pub range: [u32; 2],
    /// The use has an explicit qualifier and cannot be shadowed by a
    /// local declaration.
    pub qualified: bool,
}

/// One row of `dependencies`: a library that imports the declaring
/// library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyRow {
    /// The importing library's stable path.
    pub path: String,
}

/// The `minimal`-tier replacement for the section arrays: one entry per
/// section, always all seven (a zero is a value, not an absent section).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionCounts {
    /// Rows `inheritance` would carry.
    pub inheritance: u32,
    /// Rows `interfaces` would carry.
    pub interfaces: u32,
    /// Rows `constructors` would carry.
    pub constructors: u32,
    /// Rows `methods` would carry.
    pub methods: u32,
    /// Rows `references` would carry.
    pub references: u32,
    /// Rows `dependencies` would carry.
    pub dependencies: u32,
    /// Rows `affected_tests` would carry.
    pub affected_tests: u32,
}

/// Stable machine spelling of a declaration family — the query face's
/// grammar, exhaustive over the index's [`DeclKind`].
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

/// Stable machine spelling of a reference relation — the query face's
/// grammar, exhaustive over [`RefKind`].
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

/// Answers the explain query for `symbol` (PRD §15.3).
///
/// The input faces are the caller's, borrowed through `'a`:
///
/// * `decls` — every declaration hit for the name (the index's by-name
///   probe, mapped to [`SymbolDeclHit`]s). The primary declaration is the
///   path-ascending first hit.
/// * `refs` — resolved reference rows for the name; rows targeting other
///   same-named declarations are filtered by the query, so the by-name
///   union is a valid input.
/// * `decls_of(file)` — the file's declaration surface (members are found
///   through the container anchor).
/// * `supertypes_of(file, name)` — the declaration's resolved supertype
///   targets ([`TargetRef`]: a `(file, name)` declaration inside the
///   analyzed set, or a bare name outside it).
/// * `importers` — the shared inverted import graph over the same library
///   id space as the hits.
/// * `label_of(lib)` / `file_label_of(file)` — stable path text for the
///   two id spaces. Each face carries its own borrow lifetime: the faces
///   usually live in different caller tables, and one shared lifetime
///   would force the longest to cover them all.
/// * `tests` — the test graph over the same library id space.
/// * `strings` — the intern table the ids resolve through.
///
/// # Panics
///
/// Panics on out-of-contract caller data, surfaced never clamped: a hit's
/// `library` beyond the graph's id space stops in
/// [`RevImportGraph::importers_of`] / [`TestGraph::tests_of`]; a
/// container anchor beyond `decls_of(file)`'s surface stops on the array
/// bounds check.
#[must_use]
// One argument per borrowed face: the report is the join, and the faces
// are what the commands layer wires — bundling them behind a struct would
// only move the count.
#[allow(clippy::too_many_arguments)]
pub fn explain<'d, 's, 'l, 'f>(
    symbol: &str,
    verbosity: Verbosity,
    decls: &[SymbolDeclHit<'d>],
    refs: &[RefRow],
    decls_of: impl Fn(FileId) -> &'d [DeclRow],
    supertypes_of: impl Fn(FileId, StringId) -> &'s [TargetRef],
    importers: &RevImportGraph,
    label_of: impl Fn(u32) -> &'l str,
    file_label_of: impl Fn(FileId) -> &'f str,
    tests: &TestGraph,
    strings: &StringTable,
) -> ExplainReport {
    // Primary declaration: path-ascending first hit (FileId breaks a
    // label tie deterministically).
    let mut hits: Vec<&SymbolDeclHit<'_>> = decls.iter().collect();
    hits.sort_by(|a, b| {
        file_label_of(a.file)
            .cmp(file_label_of(b.file))
            .then_with(|| a.file.cmp(&b.file))
    });
    let primary = hits.first().copied();

    let declaration = primary.map(|hit| declaration_row(hit, &decls_of, &file_label_of, strings));
    let methods = primary.map_or_else(Vec::new, |hit| member_rows(hit, &decls_of, strings));
    let targets: &'s [TargetRef] =
        primary.map_or(&[], |hit| supertypes_of(hit.file, hit.decl.name));
    let supers = supertype_rows(targets, &file_label_of, strings);
    let references = reference_rows(primary.map(|hit| hit.file), refs, &file_label_of);

    let mut dependencies: Vec<DependencyRow> = match primary {
        Some(hit) => importers
            .importers_of(hit.library as usize)
            .iter()
            .map(|&lib| DependencyRow {
                path: label_of(lib).to_owned(),
            })
            .collect(),
        None => Vec::new(),
    };
    dependencies.sort_by(|a, b| a.path.cmp(&b.path));
    dependencies.dedup_by(|a, b| a.path == b.path);

    let mut affected: Vec<AffectedTest> = match primary {
        Some(hit) => tests
            .tests_of(hit.library)
            .iter()
            .map(|&lib| AffectedTest {
                path: label_of(lib).to_owned(),
            })
            .collect(),
        None => Vec::new(),
    };
    affected.sort_by(|a, b| a.path.cmp(&b.path));
    affected.dedup_by(|a, b| a.path == b.path);

    // `minimal` replaces the arrays with counts and trims the
    // declaration's container (a convenience field, never a locator);
    // `compact`/`full` carry the arrays, empty sections included.
    let minimal = verbosity == Verbosity::Minimal;
    let count = |rows: usize| u32::try_from(rows).expect("more than u32::MAX rows in one section");
    let counts = minimal.then(|| SectionCounts {
        inheritance: count(supers.len()),
        interfaces: 0,
        constructors: 0,
        methods: count(methods.len()),
        references: count(references.len()),
        dependencies: count(dependencies.len()),
        affected_tests: count(affected.len()),
    });
    let sections = !minimal;

    ExplainReport {
        schema_version: EXPLAIN_SCHEMA_VERSION,
        symbol: symbol.to_owned(),
        declaration: declaration.map(|row| {
            if minimal {
                DeclarationRow {
                    container: None,
                    ..row
                }
            } else {
                row
            }
        }),
        inheritance: sections.then_some(supers),
        interfaces: sections.then_some(Vec::new()),
        constructors: sections.then_some(Vec::new()),
        methods: sections.then_some(methods),
        references: sections.then_some(references),
        dependencies: sections.then_some(dependencies),
        affected_tests: sections.then_some(affected),
        counts,
    }
}

/// The `declaration` row of the primary hit: the declaring file's surface
/// answers the container name (an out-of-range container anchor is a
/// caller bug and stops on the slice bounds check, surfaced never
/// clamped).
fn declaration_row<'d, 'f>(
    hit: &SymbolDeclHit<'_>,
    decls_of: &impl Fn(FileId) -> &'d [DeclRow],
    file_label_of: &impl Fn(FileId) -> &'f str,
    strings: &StringTable,
) -> DeclarationRow {
    let surface = decls_of(hit.file);
    let container = (hit.decl.container != DeclRow::NONE)
        .then(|| strings.resolve(surface[hit.decl.container as usize].name))
        .map(str::to_owned);
    DeclarationRow {
        file: file_label_of(hit.file).to_owned(),
        name: strings.resolve(hit.decl.name).to_owned(),
        kind: decl_kind_name(hit.decl.kind).to_owned(),
        container,
        range: [
            hit.decl.range.offset,
            hit.decl.range.offset + hit.decl.range.length,
        ],
    }
}

/// The container's member rows: rows whose container anchor points at a
/// row named like the primary declaration (Dart forbids same-name
/// containers in one file, so the anchor set holds at most one row in
/// honest input; the Vec keeps even broken input's answer well-defined).
/// Constructors are the documented empty seam — the capture mints no
/// constructor rows, so the member split is total and every member lands
/// here under its own kind. Sorted by (name, range start).
fn member_rows<'d>(
    hit: &SymbolDeclHit<'_>,
    decls_of: &impl Fn(FileId) -> &'d [DeclRow],
    strings: &StringTable,
) -> Vec<MemberRow> {
    let surface = decls_of(hit.file);
    let containers: Vec<u32> = surface
        .iter()
        .enumerate()
        .filter(|(_, row)| row.name == hit.decl.name)
        .map(|(index, _)| u32::try_from(index).expect("more than u32::MAX declaration rows"))
        .collect();
    let mut rows: Vec<MemberRow> = surface
        .iter()
        .filter(|row| containers.contains(&row.container))
        .map(|row| MemberRow {
            name: strings.resolve(row.name).to_owned(),
            kind: decl_kind_name(row.kind).to_owned(),
            range: [row.range.offset, row.range.offset + row.range.length],
        })
        .collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.range.cmp(&b.range)));
    rows
}

/// The supertype rows: a resolved target keeps its declaring file, a bare
/// name outside the analyzed set keeps `file` absent. Sorted by (name,
/// file) with unresolved rows first.
fn supertype_rows<'f>(
    targets: &[TargetRef],
    file_label_of: &impl Fn(FileId) -> &'f str,
    strings: &StringTable,
) -> Vec<SuperTypeRow> {
    let mut rows: Vec<SuperTypeRow> = targets
        .iter()
        .copied()
        .map(|target| match target {
            TargetRef::Decl(file, name) => SuperTypeRow {
                name: strings.resolve(name).to_owned(),
                file: Some(file_label_of(file).to_owned()),
            },
            TargetRef::Name(name) => SuperTypeRow {
                name: strings.resolve(name).to_owned(),
                file: None,
            },
        })
        .collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.file.cmp(&b.file)));
    rows
}

/// The reference rows targeting `primary_file` — the filter keeps a
/// caller's whole by-name union honest, attributing only the primary
/// declaration's sites. Sorted by (use-site file, range start).
fn reference_rows<'f>(
    primary_file: Option<FileId>,
    refs: &[RefRow],
    file_label_of: &impl Fn(FileId) -> &'f str,
) -> Vec<ReferenceRow> {
    let mut rows: Vec<ReferenceRow> = match primary_file {
        Some(file) => refs
            .iter()
            .filter(|row| row.target_file == file)
            .map(|row| ReferenceRow {
                file: file_label_of(row.file).to_owned(),
                kind: ref_kind_name(row.kind).to_owned(),
                range: [row.range.offset, row.range.offset + row.range.length],
                qualified: row.qualified,
            })
            .collect(),
        None => Vec::new(),
    };
    rows.sort_by(|a, b| a.file.cmp(&b.file).then_with(|| a.range.cmp(&b.range)));
    rows
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use dartq_source::{FileTable, SourceRange};

    use super::*;

    /// One file's workspace: `a.dart` declares `Bar` and `Foo extends Bar`
    /// (methods `area`, field `label`), `b.dart` and `c.dart` reference
    /// `Foo`, and `a_test.dart` tests `a`. Ids: a=0, b=1, c=2, t=3.
    struct Fixture {
        strings: StringTable,
        bar: StringId,
        foo: StringId,
        file_a: FileId,
        file_b: FileId,
        file_c: FileId,
        surface_a: Vec<DeclRow>,
        surface_b: Vec<DeclRow>,
    }

    fn row(name: StringId, kind: DeclKind, offset: u32, container: u32) -> DeclRow {
        DeclRow {
            name,
            kind,
            unnamed_extension_member: false,
            range: SourceRange {
                offset,
                length: 3,
            },
            container,
        }
    }

    fn ref_row(file: FileId, target: FileId, name: StringId, kind: RefKind, offset: u32) -> RefRow {
        RefRow {
            file,
            target_file: target,
            target_name: name,
            kind,
            range: SourceRange {
                offset,
                length: 3,
            },
            qualified: false,
        }
    }

    fn fixture() -> Fixture {
        let strings = StringTable::new();
        let id = |text: &str| strings.intern(text);
        let (bar, foo) = (id("Bar"), id("Foo"));
        let mut files = FileTable::new();
        let file_a = files.intern(Path::new("a.dart"));
        let file_b = files.intern(Path::new("b.dart"));
        let file_c = files.intern(Path::new("c.dart"));
        // Capture order: Bar, Foo, then Foo's members — the member
        // anchors point at Foo's row index 1.
        let surface_a = vec![
            row(bar, DeclKind::Class, 0, DeclRow::NONE),
            row(foo, DeclKind::Class, 40, DeclRow::NONE),
            row(id("label"), DeclKind::Field, 70, 1),
            row(id("area"), DeclKind::Method, 90, 1),
        ];
        let surface_b = vec![];
        Fixture {
            strings,
            bar,
            foo,
            file_a,
            file_b,
            file_c,
            surface_a,
            surface_b,
        }
    }

    fn labels(lib: u32) -> &'static str {
        ["package:t/a.dart", "package:t/b.dart", "package:t/c.dart", "package:t/a_test.dart"][lib as usize]
    }

    fn file_labels(file: FileId) -> &'static str {
        ["a.dart", "b.dart", "c.dart"][file.index() as usize]
    }

    /// The graph faces over the four-library id space: `b`, `c`, and the
    /// test import `a`.
    fn graphs() -> (RevImportGraph, TestGraph) {
        let imports = |lib: u32| -> &'static [u32] {
            match lib {
                1..=3 => &[0],
                _ => &[],
            }
        };
        let importers = RevImportGraph::build(4, imports);
        let tests = TestGraph::build(
            4,
            imports,
            |lib| Some(labels(lib).rsplit('/').next().unwrap_or("x")),
        );
        (importers, tests)
    }

    fn hits(fx: &Fixture) -> Vec<SymbolDeclHit<'_>> {
        vec![SymbolDeclHit {
            library: 0,
            file: fx.file_a,
            decl: &fx.surface_a[1],
        }]
    }

    fn supertypes(fx: &Fixture) -> Vec<TargetRef> {
        vec![TargetRef::Decl(fx.file_a, fx.bar)]
    }

    /// Three reference rows in shuffled file/offset order (the caller's
    /// union may arrive unsorted; two of the rows sit in `b.dart`, one in
    /// `c.dart`).
    fn refs(fx: &Fixture) -> Vec<RefRow> {
        vec![
            ref_row(fx.file_c, fx.file_a, fx.foo, RefKind::Invoked, 300),
            ref_row(fx.file_b, fx.file_a, fx.foo, RefKind::Referenced, 10),
            ref_row(fx.file_b, fx.file_a, fx.foo, RefKind::Invoked, 50),
        ]
    }

    #[test]
    fn end_to_end_report_pins_the_json_shape() {
        let fx = fixture();
        let (importers, tests) = graphs();
        let supers = supertypes(&fx);
        let refs = refs(&fx);
        let report = explain(
            "Foo",
            Verbosity::Compact,
            &hits(&fx),
            &refs,
            |file| match file.index() {
                0 => &fx.surface_a,
                _ => &fx.surface_b,
            },
            |_, name| {
                assert_eq!(name, fx.foo);
                &supers
            },
            &importers,
            labels,
            file_labels,
            &tests,
            &fx.strings,
        );
        let value = serde_json::to_value(&report).expect("serialize");
        assert_eq!(
            value,
            serde_json::json!({
                "schema_version": 1,
                "symbol": "Foo",
                "declaration": {
                    "file": "a.dart",
                    "name": "Foo",
                    "kind": "class",
                    "range": [40, 43],
                },
                "inheritance": [{"name": "Bar", "file": "a.dart"}],
                "interfaces": [],
                "constructors": [],
                "methods": [
                    {"name": "area", "kind": "method", "range": [90, 93]},
                    {"name": "label", "kind": "field", "range": [70, 73]},
                ],
                "references": [
                    {"file": "b.dart", "kind": "reference", "range": [10, 13], "qualified": false},
                    {"file": "b.dart", "kind": "invoked", "range": [50, 53], "qualified": false},
                    {"file": "c.dart", "kind": "invoked", "range": [300, 303], "qualified": false},
                ],
                "dependencies": [
                    {"path": "package:t/a_test.dart"},
                    {"path": "package:t/b.dart"},
                    {"path": "package:t/c.dart"},
                ],
                "affected_tests": [{"path": "package:t/a_test.dart"}],
            }),
            "every section present, sorted by its documented key"
        );
    }

    /// `minimal` keeps the declaration and replaces every array with
    /// `counts`.
    #[test]
    fn minimal_carries_counts_and_declaration_only() {
        let fx = fixture();
        let (importers, tests) = graphs();
        let supers = supertypes(&fx);
        let report = explain(
            "Foo",
            Verbosity::Minimal,
            &hits(&fx),
            &[],
            |file| match file.index() {
                0 => &fx.surface_a,
                _ => &fx.surface_b,
            },
            |_, _| &supers,
            &importers,
            labels,
            file_labels,
            &tests,
            &fx.strings,
        );
        assert!(report.declaration.is_some(), "the red line stays");
        assert!(report.inheritance.is_none() && report.methods.is_none());
        assert!(report.references.is_none() && report.dependencies.is_none());
        assert_eq!(
            report.counts,
            Some(SectionCounts {
                inheritance: 1,
                interfaces: 0,
                constructors: 0,
                methods: 2,
                references: 0,
                dependencies: 3,
                affected_tests: 1,
            })
        );
        let value = serde_json::to_value(&report).expect("serialize");
        assert!(value.get("inheritance").is_none(), "arrays are dropped");
        assert_eq!(value["counts"]["methods"], 2);
    }

    /// `full` is byte-equal to `compact` today (module docs, Verbosity).
    #[test]
    fn full_matches_compact_byte_for_byte() {
        let fx = fixture();
        let (importers, tests) = graphs();
        let supers = supertypes(&fx);
        let refs = refs(&fx);
        let run = |verbosity| {
            let report = explain(
                "Foo",
                verbosity,
                &hits(&fx),
                &refs,
                |file| match file.index() {
                    0 => &fx.surface_a,
                    _ => &fx.surface_b,
                },
                |_, _| &supers,
                &importers,
                labels,
                file_labels,
                &tests,
                &fx.strings,
            );
            serde_json::to_string(&report).expect("serialize")
        };
        assert_eq!(run(Verbosity::Full), run(Verbosity::Compact));
    }

    /// An unknown symbol answers the seam report: no declaration, every
    /// section empty (never absent at compact), zero counts.
    #[test]
    fn unknown_symbol_answers_the_empty_seam_report() {
        let fx = fixture();
        let (importers, tests) = graphs();
        let report = explain(
            "Nope",
            Verbosity::Compact,
            &[],
            &[],
            |_| &fx.surface_b,
            |_, _| &[],
            &importers,
            labels,
            file_labels,
            &tests,
            &fx.strings,
        );
        assert!(report.declaration.is_none());
        assert_eq!(report.interfaces, Some(Vec::new()));
        assert_eq!(report.constructors, Some(Vec::new()));
        assert_eq!(report.methods, Some(Vec::new()));
        assert!(report.counts.is_none(), "counts are a minimal-tier field");
    }

    /// The two documented seams: `interfaces` and `constructors` answer
    /// empty even for a class whose source carries both — the capture has
    /// no edges or rows for them yet.
    #[test]
    fn interfaces_and_constructors_stay_empty_seams() {
        let fx = fixture();
        let (importers, tests) = graphs();
        let report = explain(
            "Foo",
            Verbosity::Compact,
            &hits(&fx),
            &[],
            |file| match file.index() {
                0 => &fx.surface_a,
                _ => &fx.surface_b,
            },
            |_, _| &[],
            &importers,
            labels,
            file_labels,
            &tests,
            &fx.strings,
        );
        assert_eq!(report.inheritance, Some(Vec::new()), "no edges, no rows");
        assert_eq!(report.interfaces, Some(Vec::new()), "the capture seam");
        assert_eq!(report.constructors, Some(Vec::new()), "the capture seam");
        assert_eq!(report.methods.as_ref().map(Vec::len), Some(2));
    }

    /// A supertype outside the analyzed set keeps its bare name, without
    /// a file, and sorts before resolved rows of the same name.
    #[test]
    fn unresolved_supertype_keeps_the_bare_name() {
        let fx = fixture();
        let (importers, tests) = graphs();
        let object = fx.strings.intern("Object");
        let supers = vec![
            TargetRef::Decl(fx.file_a, fx.bar),
            TargetRef::Name(object),
        ];
        let report = explain(
            "Foo",
            Verbosity::Compact,
            &hits(&fx),
            &[],
            |file| match file.index() {
                0 => &fx.surface_a,
                _ => &fx.surface_b,
            },
            |_, _| &supers,
            &importers,
            labels,
            file_labels,
            &tests,
            &fx.strings,
        );
        assert_eq!(
            report.inheritance,
            Some(vec![
                SuperTypeRow {
                    name: "Bar".to_owned(),
                    file: Some("a.dart".to_owned()),
                },
                SuperTypeRow {
                    name: "Object".to_owned(),
                    file: None,
                },
            ])
        );
    }

    /// Querying a member: the declaration names its container (trimmed at
    /// minimal), and the container's members do not re-appear.
    #[test]
    fn member_declaration_names_the_container() {
        let fx = fixture();
        let (importers, tests) = graphs();
        let hits = vec![SymbolDeclHit {
            library: 0,
            file: fx.file_a,
            decl: &fx.surface_a[3], // `area`, container row 1 = Foo
        }];
        let compact = explain(
            "area",
            Verbosity::Compact,
            &hits,
            &[],
            |file| match file.index() {
                0 => &fx.surface_a,
                _ => &fx.surface_b,
            },
            |_, _| &[],
            &importers,
            labels,
            file_labels,
            &tests,
            &fx.strings,
        );
        let declaration = compact.declaration.expect("declaration");
        assert_eq!(declaration.container.as_deref(), Some("Foo"));
        assert_eq!(declaration.kind, "method");
        assert_eq!(compact.methods, Some(Vec::new()), "members are Foo's, not area's");

        let minimal = explain(
            "area",
            Verbosity::Minimal,
            &hits,
            &[],
            |file| match file.index() {
                0 => &fx.surface_a,
                _ => &fx.surface_b,
            },
            |_, _| &[],
            &importers,
            labels,
            file_labels,
            &tests,
            &fx.strings,
        );
        let declaration = minimal.declaration.as_ref().expect("declaration");
        assert!(
            declaration.container.is_none(),
            "container is compact-tier, trimmed at minimal"
        );
        let value = serde_json::to_value(&minimal).expect("serialize");
        assert!(value["declaration"].get("container").is_none());
    }

    /// Several hits for one name across files: the path-ascending first
    /// is primary, and only its references ride the report (the union
    /// input is filtered by target file).
    #[test]
    fn multi_file_hits_pick_the_primary_and_filter_references() {
        let mut fx = fixture();
        // A second, same-named class in c.dart (path-descending hit order
        // in the input — the sort still picks a.dart first).
        fx.surface_b = vec![row(fx.foo, DeclKind::Class, 500, DeclRow::NONE)];
        let (importers, tests) = graphs();
        let hits = vec![
            SymbolDeclHit {
                library: 2,
                file: fx.file_c,
                decl: &fx.surface_b[0],
            },
            SymbolDeclHit {
                library: 0,
                file: fx.file_a,
                decl: &fx.surface_a[1],
            },
        ];
        let report = explain(
            "Foo",
            Verbosity::Compact,
            &hits,
            &refs(&fx),
            // `b.dart`'s surface stays empty; the same-named second hit
            // lives in `c.dart`, whose surface is the same Vec here.
            |file| match file.index() {
                0 => &fx.surface_a,
                _ => &fx.surface_b,
            },
            |_, _| &[],
            &importers,
            labels,
            file_labels,
            &tests,
            &fx.strings,
        );
        assert_eq!(report.declaration.as_ref().expect("primary").file, "a.dart");
        assert_eq!(
            report
                .references
                .expect("references")
                .iter()
                .map(|row| row.file.as_str())
                .collect::<Vec<_>>(),
            vec!["b.dart", "b.dart", "c.dart"],
            "only the primary declaration's rows, sorted by file then offset"
        );
        assert_eq!(
            report.dependencies.as_ref().map(Vec::len),
            Some(3),
            "the primary's library drives the graph joins"
        );
    }
}
