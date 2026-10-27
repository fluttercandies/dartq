//! The `explain` query: the AI's one-stop symbol report — one
//! answer carrying the declaration, its supertypes, its members, its
//! reference sites, its library's importers, and the tests exercising it.
//!
//! # The query shape
//!
//! Like [`impact`](crate::impact), the query is a pure function over inputs
//! the caller owns — it never sees the engine or the storage. The inputs
//! are borrowed faces, each mapped to a report section:
//!
//! | Section (wire)    | Input face                                             |
//! |-------------------|--------------------------------------------------------|
//! | `declaration`     | the name's declaration hits (`SymbolDeclHit`)          |
//! | `bases`           | the primary declaration's `extends` supertype edges    |
//! | `ifaces`          | the remaining clause edges (`with`/`on`/`implements`)  |
//! | `ctors`           | the caller-rendered constructor facts of the container |
//! | `methods`         | the declaring container's rows via the `decls_of` probe |
//! | `refs`            | resolved [`RefRow`]s targeting the primary declaration |
//! | `deps`            | the declaring library's importers ([`RevImportGraph`]) |
//! | `tests`           | the declaring library's [`TestGraph`] rows             |
//! | `others`          | the remaining same-named hits, ranked like the primary |
//!
//! The supertype split follows the clause each edge was written in (the
//! `supertypes_of` face carries the index's resolved edges as
//! `(target, clause)` pairs): `extends` heads (the superclass, a
//! mixin-application head) are inheritance; every other clause is an
//! interface row carrying its `clause` spelling. The constructor facts
//! are the semantic element face's constructor rows — rendered
//! caller-side (the assembler stays semantic-free) and placed, sorted,
//! and shaped here: the unnamed constructor under the container's own
//! name, named ones under the `A.named` selector spelling.
//!
//! A member primary (a hit with a container anchor, the
//! `Container.member` spelling's answer) carries no supertype clauses,
//! no constructors, and no member surface of its own: `bases`, `ifaces`,
//! `ctors`, and `methods` are empty by nature for it (#2655) — never the
//! aliased faces a same-named container in the declaring file would
//! answer. Its `refs` are the primary member's own use sites — the
//! target pair `(file, container)` scoping, so same-file containers
//! declaring the same member name never leak into each other — plus the
//! sites statically resolved to any member of its override closure when
//! the caller hands the family face (`is_family_ref`, the same
//! equivalence class the rename face edits).
//!
//! # Provenance and ordering
//!
//! Internal ids (`FileId`, [`StringId`], library ids) never leave the
//! engine boundary: every row carries stable path text through
//! the `file_label_of` / `label_of` closures and the [`StringTable`]. Every
//! array sorts by a documented key so the output is deterministic:
//!
//! * `declaration` — the primary declaration is the selection-order first
//!   hit: non-test files before test files, public containers before
//!   `_`-private ones, then the stable (path, line) order — so a test
//!   tree's same-named fake never shadows the library declaration. The
//!   remaining hits ride `others` in the same rank order (the next-best
//!   candidate first), each as a locator row for re-querying
//!   (`Container.name`).
//! * `bases`/`ifaces` — ascending by supertype name, then by
//!   target file path; unresolved rows (a supertype outside the analyzed
//!   set, no file) sort before resolved rows of the same name.
//! * `methods` — ascending by member name, then by range start. Fields,
//!   getters, and setters ride the array under their own `kind` (there are
//!   no separate member sections).
//! * `ctors` — ascending by constructor name (the unnamed face's
//!   bare container name sorts before the `A.named` selectors).
//! * `refs` — ascending by use-site file path, then by range start.
//!   Rows for other same-named declarations are filtered out (the
//!   primary's target pair decides, not the file alone — two classes in
//!   one file may declare the same member name), so a caller may hand
//!   the whole by-name reference union; the family face widens the
//!   admission past the pair for a member query.
//! * `deps` — ascending by library path, deduplicated. The rows
//!   are the **reverse** edges: the libraries that import the declaring
//!   library (the shared [`RevImportGraph`] CSR — who depends on the
//!   symbol, the same orientation `affected_tests` answers). Symbol-level
//!   forward narrowing (which imports feed this symbol's resolution)
//!   waits for the engine's resolution faces.
//! * `tests` — ascending by test library path, deduplicated.
//!
//! # Verbosity
//!
//! * `minimal` — `declaration` plus a `counts` object (one entry per
//!   section, `others` included); the section arrays are dropped, never
//!   the locators.
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
//! [`decl_kind_name`] / [`ref_kind_name`] / [`clause_name`] map the index's
//! enums to the frozen v1 machine spellings — the same grammar
//! `dartq-schema`'s `DECL_KINDS`/`REF_KINDS`/clause list and the CLI's
//! query face use. Each producer maps its own enum; the spellings are the
//! shared contract, not shared code.

use serde::{Deserialize, Serialize};

use dartq_index::{DeclKind, DeclRow, RefKind, RefRow, SuperClause, TargetRef, TestGraph};
use dartq_source::{FileId, StringId, StringTable};

use crate::impact::AffectedTest;
use crate::rev_import_graph::RevImportGraph;

/// The report contract version: breaking output changes bump
/// the major.
pub const EXPLAIN_SCHEMA_VERSION: u32 = 1;

/// How much detail the report carries. Default
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

/// The explain report, JSON contract `v: 1` (the short-vocabulary envelope).
///
/// Section arrays are `Some` at [`Verbosity::Compact`] and
/// [`Verbosity::Full`] (empty sections included — the seam is visible),
/// `None` at [`Verbosity::Minimal`], where [`ExplainReport::counts`]
/// replaces them. `declaration` is `None` when the symbol is unknown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplainReport {
    /// The output contract version ([`EXPLAIN_SCHEMA_VERSION`]). Wire: `v`.
    #[serde(rename = "v")]
    pub schema_version: u32,
    /// The queried name, as spelled by the caller. Wire: `sym`.
    #[serde(rename = "sym")]
    pub symbol: String,
    /// The primary declaration; `None` when the name declares nowhere.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declaration: Option<DeclarationRow>,
    /// Direct supertypes (the `extends` clause / mixin application head).
    /// Wire: `bases`.
    #[serde(rename = "bases", skip_serializing_if = "Option::is_none")]
    pub inheritance: Option<Vec<SuperTypeRow>>,
    /// Implemented/mixed-in types: the `with`/`on`/`implements` supertype
    /// edges, each row naming its clause. Wire: `ifaces`.
    #[serde(rename = "ifaces", skip_serializing_if = "Option::is_none")]
    pub interfaces: Option<Vec<SuperTypeRow>>,
    /// Declared constructors: the unnamed face under the container's own
    /// name, named ones under the `A.named` selector spelling. Wire:
    /// `ctors`.
    #[serde(rename = "ctors", skip_serializing_if = "Option::is_none")]
    pub constructors: Option<Vec<ConstructorRow>>,
    /// The container's members: methods, getters, setters, and fields,
    /// kind-tagged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub methods: Option<Vec<MemberRow>>,
    /// Resolved use sites of the primary declaration. Wire: `refs`.
    #[serde(rename = "refs", skip_serializing_if = "Option::is_none")]
    pub references: Option<Vec<ReferenceRow>>,
    /// The libraries that import the declaring library (reverse edges).
    /// Wire: `deps`.
    #[serde(rename = "deps", skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<DependencyRow>>,
    /// The test libraries exercising the declaring library. Wire: `tests`.
    #[serde(rename = "tests", skip_serializing_if = "Option::is_none")]
    pub affected_tests: Option<Vec<AffectedTest>>,
    /// The remaining same-named declarations the primary was picked over,
    /// in rank order — the disambiguation face (#2621). Wire: `others`.
    #[serde(rename = "others", skip_serializing_if = "Option::is_none")]
    pub others: Option<Vec<OtherDeclRow>>,
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
    /// The name token's start offset (UTF-16 code units).
    pub off: u32,
    /// The name token's span length.
    pub len: u32,
}

/// One row of `others`: a remaining same-named candidate the primary was
/// picked over — a locator shaped for disambiguation and re-querying
/// (`Container.name` on this face), never a full declaration row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OtherDeclRow {
    /// The candidate's declaring file's path.
    pub file: String,
    /// The declaration family (the frozen v1 spelling, [`decl_kind_name`]).
    pub kind: String,
    /// The enclosing container's name; absent for top-level candidates.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    /// The name token's 1-based line.
    pub line: u32,
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
    /// The clause the supertype was written in ([`clause_name`]); absent
    /// only for producers that cannot attribute it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clause: Option<String>,
}

/// One constructor face the caller rendered from the element model —
/// the input row of the `constructors` face, owned text so the caller's
/// transient map needs no borrowed lifetime of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstructorFact {
    /// The constructor's name: the container's own name for the unnamed
    /// face, `A.named` for a named selector.
    pub name: String,
    /// The rendered parameter face (`A.named(int x)`, `A()` for the
    /// implicit default).
    pub signature: String,
}

/// One row of `constructors`. The element model carries no constructor
/// spans (the capture mints no constructor rows), so the row has no
/// `range` — the name and the rendered parameter face are the facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConstructorRow {
    /// The constructor's name: the container's own name for the unnamed
    /// face, `A.named` for a named selector.
    pub name: String,
    /// The member family — always [`MEMBER_KIND_CONSTRUCTOR`].
    pub kind: String,
    /// The rendered parameter face, the caller's [`ConstructorFact`]
    /// signature. Wire: `sig`.
    #[serde(rename = "sig")]
    pub signature: String,
}

/// One member row of `methods`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberRow {
    /// The member's name (lookup spelling).
    pub name: String,
    /// The member family: `method`, `getter`, `setter`, or `field`.
    pub kind: String,
    /// The name token's start offset (UTF-16 code units).
    pub off: u32,
    /// The name token's span length.
    pub len: u32,
}

/// One reference row of `references`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenceRow {
    /// The use site's file (a multi-file library names the member file).
    pub file: String,
    /// The relation kind (the frozen v1 spelling, [`ref_kind_name`]).
    pub kind: String,
    /// The use site's start offset (UTF-16 code units).
    pub off: u32,
    /// The use site's span length.
    pub len: u32,
    /// The use has an explicit qualifier and cannot be shadowed by a
    /// local declaration.
    pub qualified: bool,
}

/// One row of `dependencies`: a library joined to the declaring library
/// by an import edge. On the explain face the rows are the importers
/// (reverse edges); the query face's `dependencies` kind serves the
/// forward direction (the declaring library's import targets) and carries
/// the optional `name` column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyRow {
    /// The joined library's stable path.
    pub path: String,
    /// The symbol this row's join consumes — the query face's forward
    /// rows carry one per resolved use of a dependency-library
    /// declaration; a bare import edge (no resolved use) and the explain
    /// face's importer rows keep the key absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// The `minimal`-tier replacement for the section arrays: one entry per
/// section, always all eight (a zero is a value, not an absent section).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionCounts {
    /// Rows `inheritance` would carry (wire: `bases`).
    #[serde(rename = "bases")]
    pub inheritance: u32,
    /// Rows `interfaces` would carry (wire: `ifaces`).
    #[serde(rename = "ifaces")]
    pub interfaces: u32,
    /// Rows `constructors` would carry (wire: `ctors`).
    #[serde(rename = "ctors")]
    pub constructors: u32,
    /// Rows `methods` would carry.
    pub methods: u32,
    /// Rows `references` would carry (wire: `refs`).
    #[serde(rename = "refs")]
    pub references: u32,
    /// Rows `dependencies` would carry (wire: `deps`).
    #[serde(rename = "deps")]
    pub dependencies: u32,
    /// Rows `affected_tests` would carry (wire: `tests`).
    #[serde(rename = "tests")]
    pub affected_tests: u32,
    /// Rows `others` would carry: the remaining same-named candidates.
    #[serde(rename = "others")]
    pub others: u32,
}

/// Stable machine spelling of the constructor family — the `constructors`
/// section's only kind.
pub const MEMBER_KIND_CONSTRUCTOR: &str = "constructor";

/// Stable machine spelling of a supertype clause — the
/// `inheritance`/`interfaces` rows' grammar, exhaustive over the index's
/// [`SuperClause`].
#[must_use]
pub fn clause_name(clause: SuperClause) -> &'static str {
    match clause {
        SuperClause::Extends => "extends",
        SuperClause::With => "with",
        SuperClause::On => "on",
        SuperClause::Implements => "implements",
    }
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

/// Answers the explain query for `symbol`.
///
/// The input faces are the caller's, borrowed through `'a`:
///
/// * `decls` — every declaration hit for the name (the index's by-name
///   probe, mapped to [`SymbolDeclHit`]s). The primary declaration is the
///   selection-order first hit; the rest ride `others` (module docs,
///   "Provenance and ordering").
/// * `refs` — resolved reference rows for the name; rows targeting other
///   same-named declarations are filtered by the query, so the by-name
///   union is a valid input.
/// * `is_family_ref(file, container)` — whether a reference row's target
///   pair belongs to the primary's override family, the member-spelling
///   widening (#2655): a row resolved to any member of the closure rides
///   `refs` beside the primary's own sites. The bare face answers false.
/// * `decls_of(file)` — the file's declaration surface (members are found
///   through the container anchor).
/// * `supertypes_of(file, name)` — the declaration's clause-typed
///   supertype edges: `(target, clause)` pairs, the [`TargetRef`] (a
///   `(file, name)` declaration inside the analyzed set, or a bare name
///   outside it) plus the clause the supertype was written in.
/// * `constructors_of(file, name)` — the container's constructor facts,
///   rendered caller-side from the element model
///   ([`ConstructorFact`]; empty for a non-container declaration).
/// * `importers` — the shared inverted import graph over the same library
///   id space as the hits.
/// * `label_of(lib)` / `file_label_of(file)` — stable path text for the
///   two id spaces. Each face carries its own borrow lifetime: the faces
///   usually live in different caller tables, and one shared lifetime
///   would force the longest to cover them all.
/// * `is_test_of(file)` — whether the hit's file is a test-tree file (the
///   selection order's first key; a real-path property the labels do not
///   carry, which is why the caller owns it).
/// * `line_of(file, offset)` — the 1-based line of a position (the
///   `others` rows' locator).
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
#[expect(clippy::too_many_arguments)] // one argument per borrowed face; bundling them would only move the count
pub fn explain<'d, 's, 'c, 'l, 'f>(
    symbol: &str,
    verbosity: Verbosity,
    decls: &[SymbolDeclHit<'d>],
    refs: &[RefRow],
    is_family_ref: impl Fn(FileId, StringId) -> bool,
    decls_of: impl Fn(FileId) -> &'d [DeclRow],
    supertypes_of: impl Fn(FileId, StringId) -> &'s [(TargetRef, SuperClause)],
    constructors_of: impl Fn(FileId, StringId) -> &'c [ConstructorFact],
    importers: &RevImportGraph,
    label_of: impl Fn(u32) -> &'l str,
    file_label_of: impl Fn(FileId) -> &'f str,
    is_test_of: impl Fn(FileId) -> bool,
    line_of: impl Fn(FileId, u32) -> u32,
    tests: &TestGraph,
    strings: &StringTable,
) -> ExplainReport {
    // Primary declaration selection (#2621), one decorated sort: non-test
    // files first, public (effective) containers before `_`-private ones,
    // then the stable (path, offset) order — a test tree's same-named
    // fake never shadows the library declaration, whatever the paths'
    // lexicographic order says. The privacy key is the enclosing
    // container for a member and the declaration itself at top level;
    // the offset is the line proxy within a file (monotone), and FileId
    // breaks a label tie deterministically.
    let rank_of = |hit: &SymbolDeclHit<'_>| {
        let surface = decls_of(hit.file);
        let name = if hit.decl.container != DeclRow::NONE {
            surface[hit.decl.container as usize].name
        } else {
            hit.decl.name
        };
        (
            is_test_of(hit.file),
            strings.resolve(name).starts_with('_'),
            file_label_of(hit.file),
            hit.file,
            hit.decl.range.offset,
        )
    };
    let mut ranked: Vec<_> = decls.iter().map(|hit| (rank_of(hit), hit)).collect();
    ranked.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let primary = ranked.first().map(|&(_, hit)| hit);

    // The primary's target pair (#2655): the `(file, container)` a
    // reference row must name to be one of its sites — the container
    // anchor's name for a member, the absent sentinel at top level. The
    // pair, never the file alone, scopes `refs`: two classes in one file
    // may declare the same member name, and a file-only filter would
    // leak one into the other's answer. A row naming the pair, or a
    // target inside the caller's override family (the member-spelling
    // widening), is in scope.
    let container_of = |hit: &SymbolDeclHit<'_>| -> StringId {
        let surface = decls_of(hit.file);
        if hit.decl.container != DeclRow::NONE {
            surface[hit.decl.container as usize].name
        } else {
            StringId::NONE
        }
    };
    let in_scope = |hit: &SymbolDeclHit<'_>, row: &RefRow| {
        (row.target_file == hit.file && row.target_container == container_of(hit))
            || is_family_ref(row.target_file, row.target_container)
    };

    let declaration = primary.map(|hit| declaration_row(hit, &decls_of, &file_label_of, strings));
    // The container sections are a top-level primary's (#2655): a member
    // has no supertype clauses, constructors, or member surface of its
    // own — the probes would answer a same-named container's aliased
    // faces, so the member arm answers the empty sections by nature.
    let top_level = primary.is_some_and(|hit| hit.decl.container == DeclRow::NONE);
    let edges: &'s [(TargetRef, SuperClause)] = match primary {
        Some(hit) if top_level => supertypes_of(hit.file, hit.decl.name),
        _ => &[],
    };
    let ctors: &[ConstructorFact] = match primary {
        Some(hit) if top_level => constructors_of(hit.file, hit.decl.name),
        _ => &[],
    };

    // `minimal` replaces the arrays with counts and trims the declaration's
    // container (a convenience field, never a locator); `compact`/`full`
    // carry the arrays, empty sections included. Every count mirrors its
    // row builder's predicate exactly — the clause split, the member
    // anchor set, the primary-file reference filter, the label-deduplicated
    // graph joins — taken straight off the faces, so the tier never
    // materializes the per-row Strings of the arrays it drops.
    let minimal = verbosity == Verbosity::Minimal;
    let counts = minimal.then(|| {
        let count =
            |rows: usize| u32::try_from(rows).expect("more than u32::MAX rows in one section");
        // The supertype split: `extends` edges are inheritance rows, the
        // rest interface rows — no filter, no dedup beyond the clause cut.
        let inheritance = edges
            .iter()
            .filter(|(_, clause)| *clause == SuperClause::Extends)
            .count();
        // `member_rows`: rows anchored to a same-named surface row (Dart
        // forbids same-name containers in one file, so the anchor set
        // holds at most one row in honest input) — a top-level primary's
        // face only (#2655).
        let methods = if top_level {
            primary.map_or(0, |hit| {
                let surface = decls_of(hit.file);
                let containers: Vec<u32> = surface
                    .iter()
                    .enumerate()
                    .filter(|(_, row)| row.name == hit.decl.name)
                    .map(|(index, _)| {
                        u32::try_from(index).expect("more than u32::MAX declaration rows")
                    })
                    .collect();
                surface
                    .iter()
                    .filter(|row| containers.contains(&row.container))
                    .count()
            })
        } else {
            0
        };
        // `reference_rows`: only the primary declaration's use sites (its
        // target pair, the family widening admitted).
        let references =
            primary.map_or(0, |hit| refs.iter().filter(|row| in_scope(hit, row)).count());
        // Both graph joins sort and dedup by label text — `importers_of`
        // keeps one entry per import directive, so a duplicated directive
        // repeats the importer and the raw row length overcounts. Distinct
        // borrowed labels are the exact predicate, minus the owned Strings.
        let distinct = |ids: &[u32]| -> usize {
            let mut labels: Vec<&str> = ids.iter().map(|&id| label_of(id)).collect();
            labels.sort_unstable();
            labels.dedup();
            labels.len()
        };
        SectionCounts {
            inheritance: count(inheritance),
            interfaces: count(edges.len() - inheritance),
            constructors: count(ctors.len()),
            methods: count(methods),
            references: count(references),
            dependencies: count(match primary {
                Some(hit) => distinct(importers.importers_of(hit.library as usize)),
                None => 0,
            }),
            affected_tests: count(match primary {
                Some(hit) => distinct(tests.tests_of(hit.library)),
                None => 0,
            }),
            // The candidates the primary was picked over.
            others: count(ranked.len().saturating_sub(usize::from(primary.is_some()))),
        }
    });

    let sections = !minimal;
    let methods = sections.then(|| {
        match primary {
            // A member primary's face is empty by nature (#2655) — the
            // probe answers a container's members, and a member is not
            // one.
            Some(hit) if top_level => member_rows(hit, &decls_of, strings),
            _ => Vec::new(),
        }
    });
    let inheritance = sections.then(|| {
        supertype_rows(
            edges,
            |clause| clause == SuperClause::Extends,
            &file_label_of,
            strings,
        )
    });
    let interfaces = sections.then(|| {
        supertype_rows(
            edges,
            |clause| clause != SuperClause::Extends,
            &file_label_of,
            strings,
        )
    });
    let constructors = sections.then(|| constructor_rows(ctors));
    let references = sections.then(|| match primary {
        Some(hit) => reference_rows(hit, refs, &in_scope, &file_label_of),
        None => Vec::new(),
    });
    let dependencies = sections.then(|| dependency_rows(primary, importers, &label_of));
    let affected_tests = sections.then(|| affected_test_rows(primary, tests, &label_of));
    let others = sections.then(|| {
        ranked
            .iter()
            .skip(1)
            .map(|&(_, hit)| other_row(hit, &decls_of, &file_label_of, &line_of, strings))
            .collect()
    });

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
        inheritance,
        interfaces,
        constructors,
        methods,
        references,
        dependencies,
        affected_tests,
        others,
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
        off: hit.decl.range.offset,
        len: hit.decl.range.length,
    }
}

/// One `others` row: the candidate's locator — the declaring file, the
/// family, the enclosing container (absent at top level), and the name
/// token's 1-based line. The container anchor resolution shares the
/// `declaration_row` contract (an out-of-range anchor is a caller bug and
/// stops on the slice bounds check).
fn other_row<'d, 'f>(
    hit: &SymbolDeclHit<'_>,
    decls_of: &impl Fn(FileId) -> &'d [DeclRow],
    file_label_of: &impl Fn(FileId) -> &'f str,
    line_of: &impl Fn(FileId, u32) -> u32,
    strings: &StringTable,
) -> OtherDeclRow {
    let surface = decls_of(hit.file);
    let container = (hit.decl.container != DeclRow::NONE)
        .then(|| strings.resolve(surface[hit.decl.container as usize].name).to_owned());
    OtherDeclRow {
        file: file_label_of(hit.file).to_owned(),
        kind: decl_kind_name(hit.decl.kind).to_owned(),
        container,
        line: line_of(hit.file, hit.decl.range.offset),
    }
}

/// The container's member rows: rows whose container anchor points at a
/// row named like the primary declaration (Dart forbids same-name
/// containers in one file, so the anchor set holds at most one row in
/// honest input; the Vec keeps even broken input's answer well-defined).
/// Constructors answer in their own section — the capture mints no
/// constructor rows, so the member split is total. Sorted by
/// (name, range start).
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
            off: row.range.offset,
            len: row.range.length,
        })
        .collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.off.cmp(&b.off)));
    rows
}

/// The supertype rows whose clause satisfies `keep`: a resolved target
/// keeps its declaring file, a bare name outside the analyzed set keeps
/// `file` absent; every row names its clause. Sorted by (name, file) with
/// unresolved rows first.
fn supertype_rows<'f>(
    edges: &[(TargetRef, SuperClause)],
    keep: impl Fn(SuperClause) -> bool,
    file_label_of: &impl Fn(FileId) -> &'f str,
    strings: &StringTable,
) -> Vec<SuperTypeRow> {
    let mut rows: Vec<SuperTypeRow> = edges
        .iter()
        .filter(|(_, clause)| keep(*clause))
        .map(|(target, clause)| SuperTypeRow {
            name: match target {
                TargetRef::Decl(_, name) | TargetRef::Name(name) => {
                    strings.resolve(*name).to_owned()
                }
            },
            file: match target {
                TargetRef::Decl(file, _) => Some(file_label_of(*file).to_owned()),
                TargetRef::Name(_) => None,
            },
            clause: Some(clause_name(*clause).to_owned()),
        })
        .collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.file.cmp(&b.file)));
    rows
}

/// The `constructors` rows: the caller-rendered facts placed under the
/// section's own kind, ascending by name.
fn constructor_rows(facts: &[ConstructorFact]) -> Vec<ConstructorRow> {
    let mut rows: Vec<ConstructorRow> = facts
        .iter()
        .map(|fact| ConstructorRow {
            name: fact.name.clone(),
            kind: MEMBER_KIND_CONSTRUCTOR.to_owned(),
            signature: fact.signature.clone(),
        })
        .collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    rows
}

/// The reference rows in `in_scope`'s admission — the primary's target
/// pair plus the family widening, the filter keeping a caller's whole
/// by-name union honest. Sorted by (use-site file, range start).
fn reference_rows<'f>(
    hit: &SymbolDeclHit<'_>,
    refs: &[RefRow],
    in_scope: &impl Fn(&SymbolDeclHit<'_>, &RefRow) -> bool,
    file_label_of: &impl Fn(FileId) -> &'f str,
) -> Vec<ReferenceRow> {
    let mut rows: Vec<ReferenceRow> = refs
        .iter()
        .filter(|row| in_scope(hit, row))
        .map(|row| ReferenceRow {
            file: file_label_of(row.file).to_owned(),
            kind: ref_kind_name(row.kind).to_owned(),
            off: row.range.offset,
            len: row.range.length,
            qualified: row.qualified(),
        })
        .collect();
    rows.sort_by(|a, b| a.file.cmp(&b.file).then_with(|| a.off.cmp(&b.off)));
    rows
}

/// The `dependencies` rows: the declaring library's importers (the reverse
/// edges), deduplicated by label, ascending.
fn dependency_rows<'l>(
    primary: Option<&SymbolDeclHit<'_>>,
    importers: &RevImportGraph,
    label_of: &impl Fn(u32) -> &'l str,
) -> Vec<DependencyRow> {
    let mut rows: Vec<DependencyRow> = match primary {
        Some(hit) => importers
            .importers_of(hit.library as usize)
            .iter()
            .map(|&lib| DependencyRow {
                path: label_of(lib).to_owned(),
                name: None,
            })
            .collect(),
        None => Vec::new(),
    };
    rows.sort_by(|a, b| a.path.cmp(&b.path));
    rows.dedup_by(|a, b| a.path == b.path);
    rows
}

/// The `affected_tests` rows: the test libraries exercising the declaring
/// library, deduplicated by label, ascending.
fn affected_test_rows<'l>(
    primary: Option<&SymbolDeclHit<'_>>,
    tests: &TestGraph,
    label_of: &impl Fn(u32) -> &'l str,
) -> Vec<AffectedTest> {
    let mut rows: Vec<AffectedTest> = match primary {
        Some(hit) => tests
            .tests_of(hit.library)
            .iter()
            .map(|&lib| AffectedTest {
                path: label_of(lib).to_owned(),
            })
            .collect(),
        None => Vec::new(),
    };
    rows.sort_by(|a, b| a.path.cmp(&b.path));
    rows.dedup_by(|a, b| a.path == b.path);
    rows
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use dartq_source::{FileTable, SourceRange};

    use super::*;

    /// One file's workspace: `a.dart` declares `Bar` and `Foo extends Bar
    /// with Mix implements Remote` (methods `area`, field `label`),
    /// `b.dart` and `c.dart` reference `Foo`, and `a_test.dart` tests `a`.
    /// Ids: a=0, b=1, c=2, t=3.
    struct Fixture {
        strings: StringTable,
        bar: StringId,
        foo: StringId,
        file_a: FileId,
        file_b: FileId,
        file_c: FileId,
        surface_a: Vec<DeclRow>,
        surface_b: Vec<DeclRow>,
        /// The primary declaration's supertype edges (extends + with +
        /// implements — the three-way split the two sections pin).
        supers: Vec<(TargetRef, SuperClause)>,
        /// The primary container's constructor facts, deliberately
        /// unsorted (the builder's name sort is the pin).
        ctors: Vec<ConstructorFact>,
    }

    fn row(name: StringId, kind: DeclKind, offset: u32, container: u32) -> DeclRow {
        DeclRow {
            name,
            kind,
            unnamed_extension_member: false,
            range: SourceRange { offset, length: 3 },
            container,
            decl_end: DeclRow::UNKNOWN,
            body_open: DeclRow::UNKNOWN,
        }
    }

    fn ref_row(file: FileId, target: FileId, name: StringId, kind: RefKind, offset: u32) -> RefRow {
        RefRow {
            file,
            target_file: target,
            target_name: name,
            target_container: StringId::NONE,
            kind,
            range: SourceRange { offset, length: 3 },
            qualifier: StringId::NONE,
        }
    }

    fn fixture() -> Fixture {
        let strings = StringTable::new();
        let id = |text: &str| strings.intern(text);
        let (bar, foo, mix) = (id("Bar"), id("Foo"), id("Mix"));
        let remote = id("Remote");
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
        let supers = vec![
            (TargetRef::Decl(file_a, bar), SuperClause::Extends),
            (TargetRef::Decl(file_b, mix), SuperClause::With),
            (TargetRef::Name(remote), SuperClause::Implements),
        ];
        let ctors = vec![
            ConstructorFact {
                name: "Foo.named".to_owned(),
                signature: "Foo.named(int x)".to_owned(),
            },
            ConstructorFact {
                name: "Foo".to_owned(),
                signature: "Foo()".to_owned(),
            },
        ];
        Fixture {
            strings,
            bar,
            foo,
            file_a,
            file_b,
            file_c,
            surface_a,
            surface_b,
            supers,
            ctors,
        }
    }

    fn labels(lib: u32) -> &'static str {
        [
            "package:t/a.dart",
            "package:t/b.dart",
            "package:t/c.dart",
            "package:t/a_test.dart",
        ][lib as usize]
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
        let tests = TestGraph::build(4, imports, |lib| {
            Some(labels(lib).rsplit('/').next().unwrap_or("x"))
        });
        (importers, tests)
    }

    fn hits(fx: &Fixture) -> Vec<SymbolDeclHit<'_>> {
        vec![SymbolDeclHit {
            library: 0,
            file: fx.file_a,
            decl: &fx.surface_a[1],
        }]
    }

    fn supertypes(fx: &Fixture) -> Vec<(TargetRef, SuperClause)> {
        fx.supers.clone()
    }

    /// The constructors face: the fixture's facts, keyed by the primary
    /// declaration's file.
    fn ctors(fx: &Fixture) -> Vec<ConstructorFact> {
        fx.ctors.clone()
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
        let ctors = ctors(&fx);
        let refs = refs(&fx);
        let report = explain(
            "Foo",
            Verbosity::Compact,
            &hits(&fx),
            &refs,
            |_, _| false,
            |file| match file.index() {
                0 => &fx.surface_a,
                _ => &fx.surface_b,
            },
            |_, name| {
                assert_eq!(name, fx.foo);
                &supers
            },
            |_, _| &ctors,
            &importers,
            labels,
            file_labels,
            |_| false,
            |_, off| off + 1,
            &tests,
            &fx.strings,
        );
        let value = serde_json::to_value(&report).expect("serialize");
        assert_eq!(
            value,
            serde_json::json!({
                "v": 1,
                "sym": "Foo",
                "declaration": {
                    "file": "a.dart",
                    "name": "Foo",
                    "kind": "class",
                    "off": 40,
                    "len": 3,
                },
                "bases": [{"name": "Bar", "file": "a.dart", "clause": "extends"}],
                "ifaces": [
                    {"name": "Mix", "file": "b.dart", "clause": "with"},
                    {"name": "Remote", "clause": "implements"},
                ],
                "ctors": [
                    {"name": "Foo", "kind": "constructor", "sig": "Foo()"},
                    {"name": "Foo.named", "kind": "constructor", "sig": "Foo.named(int x)"},
                ],
                "methods": [
                    {"name": "area", "kind": "method", "off": 90, "len": 3},
                    {"name": "label", "kind": "field", "off": 70, "len": 3},
                ],
                "refs": [
                    {"file": "b.dart", "kind": "reference", "off": 10, "len": 3, "qualified": false},
                    {"file": "b.dart", "kind": "invoked", "off": 50, "len": 3, "qualified": false},
                    {"file": "c.dart", "kind": "invoked", "off": 300, "len": 3, "qualified": false},
                ],
                "deps": [
                    {"path": "package:t/a_test.dart"},
                    {"path": "package:t/b.dart"},
                    {"path": "package:t/c.dart"},
                ],
                "tests": [{"path": "package:t/a_test.dart"}],
                "others": [],
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
        let ctors = ctors(&fx);
        let report = explain(
            "Foo",
            Verbosity::Minimal,
            &hits(&fx),
            &[],
            |_, _| false,
            |file| match file.index() {
                0 => &fx.surface_a,
                _ => &fx.surface_b,
            },
            |_, _| &supers,
            |_, _| &ctors,
            &importers,
            labels,
            file_labels,
            |_| false,
            |_, off| off + 1,
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
                interfaces: 2,
                constructors: 2,
                methods: 2,
                references: 0,
                dependencies: 3,
                affected_tests: 1,
                others: 0,
            })
        );
        let value = serde_json::to_value(&report).expect("serialize");
        assert!(value.get("bases").is_none(), "arrays are dropped");
        assert_eq!(value["counts"]["methods"], 2);
    }

    /// Duplicate import directives (two resolved imports of one library)
    /// put the same importer in the graph row twice; the compact rows dedup
    /// by label and the minimal count must match them — the raw row length
    /// would overcount.
    #[test]
    fn minimal_counts_match_the_rows_on_duplicate_imports() {
        let fx = fixture();
        let imports = |lib: u32| -> &'static [u32] {
            match lib {
                1 => &[0, 0], // b imports a through two directives
                2..=3 => &[0],
                _ => &[],
            }
        };
        let importers = RevImportGraph::build(4, imports);
        let tests = TestGraph::build(4, imports, |lib| {
            Some(labels(lib).rsplit('/').next().unwrap_or("x"))
        });
        let supers = supertypes(&fx);
        let faces = |verbosity| {
            explain(
                "Foo",
                verbosity,
                &hits(&fx),
                &[],
                |_, _| false,
                |file| match file.index() {
                    0 => &fx.surface_a,
                    _ => &fx.surface_b,
                },
                |_, _| &supers,
                |_, _| &[],
                &importers,
                labels,
                file_labels,
                |_| false,
                |_, off| off + 1,
                &tests,
                &fx.strings,
            )
        };
        let counts = faces(Verbosity::Minimal)
            .counts
            .expect("minimal carries counts");
        assert_eq!(
            counts.dependencies, 3,
            "b's duplicate directive counts once"
        );
        assert_eq!(counts.affected_tests, 1);
        let compact = faces(Verbosity::Compact);
        assert_eq!(
            compact.dependencies.as_ref().map(Vec::len),
            Some(3),
            "the rows the count must mirror"
        );
        assert_eq!(compact.affected_tests.as_ref().map(Vec::len), Some(1));
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
                |_, _| false,
                |file| match file.index() {
                    0 => &fx.surface_a,
                    _ => &fx.surface_b,
                },
                |_, _| &supers,
                |_, _| &[],
                &importers,
                labels,
                file_labels,
                |_| false,
                |_, off| off + 1,
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
            |_, _| false,
            |_| &fx.surface_b,
            |_, _| &[],
            |_, _| &[],
            &importers,
            labels,
            file_labels,
            |_| false,
            |_, off| off + 1,
            &tests,
            &fx.strings,
        );
        assert!(report.declaration.is_none());
        assert_eq!(report.interfaces, Some(Vec::new()));
        assert_eq!(report.constructors, Some(Vec::new()));
        assert_eq!(report.methods, Some(Vec::new()));
        assert_eq!(report.others, Some(Vec::new()), "the seam stays visible");
        assert!(report.counts.is_none(), "counts are a minimal-tier field");
    }

    /// Faces without edges or constructor facts answer empty sections —
    /// the populated shape is the fixture's, never a placeholder.
    #[test]
    fn empty_faces_answer_empty_sections() {
        let fx = fixture();
        let (importers, tests) = graphs();
        let report = explain(
            "Foo",
            Verbosity::Compact,
            &hits(&fx),
            &[],
            |_, _| false,
            |file| match file.index() {
                0 => &fx.surface_a,
                _ => &fx.surface_b,
            },
            |_, _| &[],
            |_, _| &[],
            &importers,
            labels,
            file_labels,
            |_| false,
            |_, off| off + 1,
            &tests,
            &fx.strings,
        );
        assert_eq!(report.inheritance, Some(Vec::new()), "no edges, no rows");
        assert_eq!(report.interfaces, Some(Vec::new()), "no edges, no rows");
        assert_eq!(report.constructors, Some(Vec::new()), "no facts, no rows");
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
            (TargetRef::Decl(fx.file_a, fx.bar), SuperClause::Extends),
            (TargetRef::Name(object), SuperClause::Extends),
        ];
        let report = explain(
            "Foo",
            Verbosity::Compact,
            &hits(&fx),
            &[],
            |_, _| false,
            |file| match file.index() {
                0 => &fx.surface_a,
                _ => &fx.surface_b,
            },
            |_, _| &supers,
            |_, _| &[],
            &importers,
            labels,
            file_labels,
            |_| false,
            |_, off| off + 1,
            &tests,
            &fx.strings,
        );
        assert_eq!(
            report.inheritance,
            Some(vec![
                SuperTypeRow {
                    name: "Bar".to_owned(),
                    file: Some("a.dart".to_owned()),
                    clause: Some("extends".to_owned()),
                },
                SuperTypeRow {
                    name: "Object".to_owned(),
                    file: None,
                    clause: Some("extends".to_owned()),
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
            |_, _| false,
            |file| match file.index() {
                0 => &fx.surface_a,
                _ => &fx.surface_b,
            },
            |_, _| &[],
            |_, _| &[],
            &importers,
            labels,
            file_labels,
            |_| false,
            |_, off| off + 1,
            &tests,
            &fx.strings,
        );
        let declaration = compact.declaration.expect("declaration");
        assert_eq!(declaration.container.as_deref(), Some("Foo"));
        assert_eq!(declaration.kind, "method");
        assert_eq!(
            compact.methods,
            Some(Vec::new()),
            "members are Foo's, not area's"
        );

        let minimal = explain(
            "area",
            Verbosity::Minimal,
            &hits,
            &[],
            |_, _| false,
            |file| match file.index() {
                0 => &fx.surface_a,
                _ => &fx.surface_b,
            },
            |_, _| &[],
            |_, _| &[],
            &importers,
            labels,
            file_labels,
            |_| false,
            |_, off| off + 1,
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

    /// The member-primary scoping (#2655): a member hit's container
    /// sections answer empty by nature — never the faces a same-named
    /// container would answer — and its references scope to the member's
    /// `(file, container)` pair (a same-file container's same-named member
    /// never leaks in) plus the override-family widening.
    #[test]
    fn member_primary_scopes_sections_and_widens_refs_through_the_family() {
        let fx = fixture();
        let strings = &fx.strings;
        let (m, repo, other, sub) = (
            strings.intern("m"),
            strings.intern("Repo"),
            strings.intern("Other"),
            strings.intern("Sub"),
        );
        // a.dart: Repo declaring the member `m` (the primary), Other
        // declaring its same-named `m`, and Sub — the family member the
        // widening admits.
        let surface_a = vec![
            row(repo, DeclKind::Class, 0, DeclRow::NONE),
            row(m, DeclKind::Method, 40, 0),
            row(other, DeclKind::Class, 100, DeclRow::NONE),
            row(m, DeclKind::Method, 140, 2),
            row(sub, DeclKind::Class, 200, DeclRow::NONE),
        ];
        let hit = SymbolDeclHit {
            library: 0,
            file: fx.file_a,
            decl: &surface_a[1],
        };
        // The aliased faces: probed with the member's name they would
        // answer the same-named container's rows — the member arm must
        // never consult them.
        let aliased = vec![(
            TargetRef::Decl(fx.file_a, repo),
            SuperClause::Extends,
        )];
        let aliased_ctors = vec![ConstructorFact {
            name: "m".to_owned(),
            signature: "m()".to_owned(),
        }];
        // Three use sites in the by-name union: Repo's (the primary's
        // pair), Other's (a same-file, same-named member — out), and
        // Sub's (out of the pair, in through the family face).
        let member_ref = |file: FileId, container: StringId, offset: u32| RefRow {
            file,
            target_file: fx.file_a,
            target_name: m,
            target_container: container,
            kind: RefKind::Invoked,
            range: SourceRange {
                offset,
                length: 1,
            },
            qualifier: StringId::NONE,
        };
        let refs = vec![
            member_ref(fx.file_b, repo, 10),
            member_ref(fx.file_b, other, 50),
            member_ref(fx.file_c, sub, 300),
        ];
        let (importers, tests) = graphs();
        let report = explain(
            "Repo.m",
            Verbosity::Compact,
            std::slice::from_ref(&hit),
            &refs,
            |file, container| file == fx.file_a && container == sub,
            |file| match file.index() {
                0 => surface_a.as_slice(),
                _ => &fx.surface_b,
            },
            |_, name| {
                assert_ne!(name, m, "a member primary probes no container faces");
                aliased.as_slice()
            },
            |_, name| {
                assert_ne!(name, m, "a member primary probes no constructor faces");
                aliased_ctors.as_slice()
            },
            &importers,
            labels,
            file_labels,
            |_| false,
            |_, off| off + 1,
            &tests,
            strings,
        );
        assert_eq!(
            report.declaration.as_ref().expect("primary").container,
            Some("Repo".to_owned())
        );
        assert_eq!(report.inheritance, Some(Vec::new()), "no clauses by nature");
        assert_eq!(report.interfaces, Some(Vec::new()));
        assert_eq!(report.constructors, Some(Vec::new()));
        assert_eq!(report.methods, Some(Vec::new()), "no member surface");
        assert_eq!(
            report
                .references
                .expect("references")
                .iter()
                .map(|row| (row.file.as_str(), row.off))
                .collect::<Vec<_>>(),
            vec![("b.dart", 10), ("c.dart", 300)],
            "the primary's pair plus the family row; Other's site stays out"
        );
        // The minimal counts mirror the same scoping.
        let minimal = explain(
            "Repo.m",
            Verbosity::Minimal,
            std::slice::from_ref(&hit),
            &refs,
            |file, container| file == fx.file_a && container == sub,
            |file| match file.index() {
                0 => surface_a.as_slice(),
                _ => &fx.surface_b,
            },
            |_, name| {
                assert_ne!(name, m, "a member primary probes no container faces");
                aliased.as_slice()
            },
            |_, name| {
                assert_ne!(name, m, "a member primary probes no constructor faces");
                aliased_ctors.as_slice()
            },
            &importers,
            labels,
            file_labels,
            |_| false,
            |_, off| off + 1,
            &tests,
            strings,
        );
        let counts = minimal.counts.expect("counts");
        assert_eq!(counts.methods, 0);
        assert_eq!(counts.references, 2);
        assert_eq!(counts.inheritance, 0);
        assert_eq!(counts.constructors, 0);
    }

    /// Several hits for one name across files: the selection-order first
    /// is primary (both non-test public containers here, so the path
    /// decides), and only its references ride the report (the union input
    /// is filtered by target file).
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
            |_, _| false,
            // `b.dart`'s surface stays empty; the same-named second hit
            // lives in `c.dart`, whose surface is the same Vec here.
            |file| match file.index() {
                0 => &fx.surface_a,
                _ => &fx.surface_b,
            },
            |_, _| &[],
            |_, _| &[],
            &importers,
            labels,
            file_labels,
            |_| false,
            |_, off| off + 1,
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

    /// The selection order (#2621): a test-tree hit and a `_`-private
    /// lib hit both sort after the public lib hit — even when their paths
    /// sort first — and the remaining candidates ride `others` in rank
    /// order (next-best first) as locator rows. `a.dart` is the test
    /// file, `b.dart` the private container's file, `c.dart` the public
    /// primary's: the path-ascending order of the old rule would have
    /// answered the test fake.
    #[test]
    fn selection_order_beats_the_path_order_and_others_list_the_rest() {
        let fx = fixture();
        // The line face under test: offset/10 + 1.
        // a.dart (the test file): the fake — a member of Holder.
        let surface_a = vec![
            row(fx.strings.intern("Holder"), DeclKind::Class, 0, DeclRow::NONE),
            row(fx.foo, DeclKind::Field, 300, 0),
        ];
        // b.dart: the private container's member.
        let surface_b = vec![
            row(fx.strings.intern("_Priv"), DeclKind::Class, 100, DeclRow::NONE),
            row(fx.foo, DeclKind::Method, 200, 0),
        ];
        // c.dart: the public lib declaration.
        let surface_c = vec![row(fx.foo, DeclKind::Class, 500, DeclRow::NONE)];
        let (importers, tests) = graphs();
        let hits = vec![
            SymbolDeclHit {
                library: 0,
                file: fx.file_a,
                decl: &surface_a[1], // the test fake
            },
            SymbolDeclHit {
                library: 1,
                file: fx.file_b,
                decl: &surface_b[1], // the private container's member
            },
            SymbolDeclHit {
                library: 2,
                file: fx.file_c,
                decl: &surface_c[0], // the public lib declaration
            },
        ];
        let surfaces = |file: FileId| match file.index() {
            0 => surface_a.as_slice(),
            1 => surface_b.as_slice(),
            _ => surface_c.as_slice(),
        };
        let run = |verbosity| {
            explain(
                "Foo",
                verbosity,
                &hits,
                &[],
                |_, _| false,
                surfaces,
                |_, _| &[],
                |_, _| &[],
                &importers,
                labels,
                file_labels,
                |file| file == fx.file_a,
                |_, off| off / 10 + 1,
                &tests,
                &fx.strings,
            )
        };
        let report = run(Verbosity::Compact);
        assert_eq!(
            report.declaration.as_ref().expect("primary").file,
            "c.dart",
            "the public non-test declaration wins over both lower paths"
        );
        assert_eq!(
            report.others,
            Some(vec![
                OtherDeclRow {
                    file: "b.dart".to_owned(),
                    kind: "method".to_owned(),
                    container: Some("_Priv".to_owned()),
                    line: 21,
                },
                OtherDeclRow {
                    file: "a.dart".to_owned(),
                    kind: "field".to_owned(),
                    container: Some("Holder".to_owned()),
                    line: 31,
                },
            ]),
            "rank order: the private lib candidate, then the test fake"
        );

        // The same hits at minimal: the array drops, the count carries.
        let minimal = run(Verbosity::Minimal);
        assert!(minimal.others.is_none(), "the array drops at minimal");
        assert_eq!(minimal.counts.expect("counts").others, 2);
    }

    /// The privacy key's top-level arm: a `_`-private top-level candidate
    /// orders after a same-named member of a public container (both hits
    /// non-test, the public container wins), and the top-level candidate's
    /// row carries no `container` key.
    #[test]
    fn top_level_privacy_orders_by_the_own_name() {
        let fx = fixture();
        let under = fx.strings.intern("_helper");
        // a.dart: the private top-level function.
        let surface_a = vec![row(under, DeclKind::TopLevelFn, 0, DeclRow::NONE)];
        // b.dart: the public container's same-named member.
        let surface_b = vec![
            row(fx.strings.intern("Repo"), DeclKind::Class, 100, DeclRow::NONE),
            row(under, DeclKind::Method, 200, 0),
        ];
        let (importers, tests) = graphs();
        let hits = vec![
            SymbolDeclHit {
                library: 0,
                file: fx.file_a,
                decl: &surface_a[0],
            },
            SymbolDeclHit {
                library: 1,
                file: fx.file_b,
                decl: &surface_b[1],
            },
        ];
        let report = explain(
            "_helper",
            Verbosity::Compact,
            &hits,
            &[],
            |_, _| false,
            |file| match file.index() {
                0 => surface_a.as_slice(),
                _ => surface_b.as_slice(),
            },
            |_, _| &[],
            |_, _| &[],
            &importers,
            labels,
            file_labels,
            |_| false,
            |_, off| off / 10 + 1,
            &tests,
            &fx.strings,
        );
        assert_eq!(
            report
                .declaration
                .as_ref()
                .expect("primary")
                .container
                .as_deref(),
            Some("Repo"),
            "the public container's member wins over the private top level"
        );
        assert_eq!(
            report.others,
            Some(vec![OtherDeclRow {
                file: "a.dart".to_owned(),
                kind: "function".to_owned(),
                container: None,
                line: 1,
            }]),
            "the top-level candidate carries no container key"
        );
    }
}
