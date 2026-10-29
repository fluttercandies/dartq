//! The per-library reference index:
//! identifier uses resolved against the file-local scope and the #117
//! [`Namespaces`], the unresolved-names recovery table, and the resolved
//! supertype edges.
//!
//! # What resolves, what stays a name row
//!
//! A bare site resolves through the [`lookup_scoped`] chain's M1 core — the
//! file's own declarations first (private included, locals beat imports),
//! then the library's namespace. A `.`-continuation resolves through the
//! prefix table when the receiver identifier armed a prefix (prefix markers
//! live in the import namespace, so arming is one bare lookup); without an
//! arm it is a member access on an untyped receiver, and the member name is
//! kept qualified in the names table. Ambiguous lookups are also names-table
//! rows: the ambiguity is diagnosed at the use point by #120, and the index
//! records only the residual signal.
//!
//! Resolution runs **after** the parse pass over the sites in source order
//! (Dart top-levels are order-independent, so a use may precede its local
//! declaration; the file's complete [`FileIndex`] must exist first). Local
//! variables and members accessed without qualification land in the names
//! table too — M1 has no local/member scope chain; that residual is the
//! documented under-report posture, and the names table is
//! exactly where the `undefined_*` recovery reads it.
//!
//! # The #120 seam
//!
//! The `undefined_*` family resolves identifiers as a side effect of its
//! checks; those miss rows belong here, not in a parallel path (the
//! M1 recovery-surface ruling on ticket #120). [`LibraryIndex::record_unresolved`]
//! batches them into an overflow buffer sorted by `(name, offset)` like the
//! built-in table — one sort per recording phase instead of a per-row
//! insert — and [`LibraryIndex::unresolved`] binary-searches both, no
//! re-sort, no allocation on the probe path. Rows recorded this way live
//! until the library rebuilds whole, at which point the resolver that
//! produced them regenerates them.
//!
//! The `lookup_scoped` chain order is re-stated here rather than imported:
//! keeping dartq-semantic out of this crate's dependency set (its model is
//! mid-flight on #120, and the reference surface must not move with it).

use dartq_resolution::{LibraryGraph, LibraryId, Lookup, NamespaceEntry, Namespaces};
use dartq_source::{FileId, SourceRange, StringId};

use crate::capture::{Family, FileCapture, FileIndex, SuperClause};

/// The relation kind of a reference or name row — the M1 subset.
/// Constructor-reference specials, named-argument references, and comment
/// references are folded into these per the M1 cut list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum RefKind {
    /// A non-invocation, non-value use: a type name, an `extends`/`on`
    /// clause name, a metadata name.
    Referenced = 0,
    /// A value read (a bare identifier expression, not called, not the
    /// target of a simple assignment).
    Read = 1,
    /// A call site, constructor reference, or tear-off-shaped use.
    Invoked = 2,
    /// The target of a plain `=` assignment.
    Written = 3,
    /// The target of a compound assignment (`+=`, `++`-shaped reads are
    /// deliberately *not* folded here — see the capture module's cut list).
    ReadWritten = 4,
}

impl RefKind {
    /// Decodes a raw kind byte written by [`RefKind as u8`]. Private
    /// encoding: the only writer is the capture's own `as u8`.
    #[must_use]
    pub(crate) fn from_u8(raw: u8) -> Self {
        match raw {
            0 => Self::Referenced,
            1 => Self::Read,
            2 => Self::Invoked,
            3 => Self::Written,
            _ => Self::ReadWritten,
        }
    }
}

/// One resolved reference site: the use, its target declaration, and the
/// relation kind. The use site's file rides on the row (`file`): a library
/// spans its defining unit plus parts, so the target pair alone cannot
/// locate the use in a multi-file library (#361).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RefRow {
    /// The use site's file — the library member whose capture recorded the
    /// row.
    pub file: FileId,
    /// The target's declaring file.
    pub target_file: FileId,
    /// The target's lookup-spelling name.
    pub target_name: StringId,
    /// The target's declaring CONTAINER name for member targets (#2618) —
    /// `(file, name)` alone cannot name a member (two classes in one file
    /// may declare the same member name); [`StringId::NONE`] for
    /// library-face targets (top-level declarations).
    pub target_container: StringId,
    /// The relation kind.
    pub kind: RefKind,
    /// The use site's span (UTF-16).
    pub range: SourceRange,
    /// The import prefix that armed the resolution (`p` in `p.fa`), interned;
    /// [`StringId::NONE`] for bare uses. `qualified` — the use cannot be
    /// shadowed by a local declaration — derives from the sentinel.
    pub qualifier: StringId,
}

impl RefRow {
    /// Whether the use carries an explicit qualifier (`p.x`) and cannot be
    /// shadowed by a local declaration.
    #[must_use]
    pub fn qualified(&self) -> bool {
        self.qualifier != StringId::NONE
    }
}

/// The packed-row pin: five ids (20 B) + the kind byte + the range (8 B)
/// plus tail pad. `refs` is the crate's largest standing table (~0.5 KB per
/// KB of source at corpus scale); the member-target container column (#2618,
/// the difference between naming a member and renaming the wrong class's)
/// moved the row 28 → 32 B — renegotiated here, never silently.
const _: () = assert!(std::mem::size_of::<RefRow>() == 32);

/// One unresolved name use — a names-table row (the recovery surface
/// for the `undefined_*` family and the only reference signal when
/// resolution is partial).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NameRow {
    /// The used name.
    pub name: StringId,
    /// The relation kind the use would have carried.
    pub kind: RefKind,
    /// The use site's span (UTF-16).
    pub range: SourceRange,
    /// The use is qualified (a member access or prefix miss).
    pub qualified: bool,
}

/// The target of a resolved supertype edge: a workspace declaration, or the
/// bare name when the supertype lies outside the analyzed set (`dart:`
/// SDK, unanalyzed dependency) — the official string-keyed fallback
/// (`SubtypeIndexElementId`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetRef {
    /// A workspace declaration.
    Decl(FileId, StringId),
    /// Outside the analyzed set; the bare name is the signal.
    Name(StringId),
}

/// One resolved supertype edge: the subtype `(file, name)`, its direct
/// supertype, and the clause the supertype was written in (`extends`,
/// `with`, `implements`, or a mixin's `on`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubtypeEdge {
    /// The subtype's file.
    pub file: FileId,
    /// The subtype's name.
    pub name: StringId,
    /// The direct supertype.
    pub supertype: TargetRef,
    /// The clause the supertype was written in.
    pub clause: SuperClause,
}

/// The reference surface of one library (defining unit plus parts), built
/// whole by [`LibraryBuilder`] and replaced whole on re-analysis (no
/// partial index update).
///
/// Invariants: `refs` ascend by
/// `(target, use file, offset)` with `Self::runs` holding each
/// distinct-target run's start — a CSR offsets array over the rows;
/// `name_refs` re-groups those runs by `target_name` (ascending; within a
/// name, the runs keep their target-ascending table order) with `name_runs`
/// holding each distinct name's group start; `names` ascend by
/// `(name, offset)`; `names_overflow` (the #120 seam's insertions) ascend by
/// `(name, offset)` too; `supers` ascend by `(subtype file, name, clause)`.
/// Every probe is a binary search plus an equal-run scan.
///
/// The name-keyed run index costs 4 B per `(target, name)` run plus 4 B per
/// distinct target name of standing memory — the documented trade for the
/// workspace query join's one-search-per-library shape (hub names were
/// quadratic through the per-(decl file, library) probe the exact-target
/// search forced).
///
/// Every frozen column is a boxed slice — a build writes each column once,
/// the whole-unit law replaces the index on re-analysis, so the capacity
/// field a `Vec` header carries would be dead weight and its push-growth
/// slack resident for the session. `names_overflow` stays a `Vec`: the
/// #120 seam appends after the build.
#[derive(Debug, Default)]
pub struct LibraryIndex {
    refs: Box<[RefRow]>,
    runs: Box<[u32]>,
    name_runs: Box<[u32]>,
    name_refs: Box<[u32]>,
    names: Box<[NameRow]>,
    names_overflow: Vec<NameRow>,
    supers: Box<[SubtypeEdge]>,
}

impl LibraryIndex {
    /// The reference rows targeting `(file, name)`, ascending by offset.
    #[must_use]
    pub fn references_of(&self, file: FileId, name: StringId) -> Option<&[RefRow]> {
        let key = |row: &RefRow| (row.target_file, row.target_name);
        let start = self
            .runs
            .partition_point(|&run| key(&self.refs[run as usize]) < (file, name));
        let run = *self.runs.get(start)?;
        if key(&self.refs[run as usize]) != (file, name) {
            return None;
        }
        let end = self
            .runs
            .get(start + 1)
            .map_or(self.refs.len(), |&end| end as usize);
        Some(&self.refs[run as usize..end])
    }

    /// The reference runs naming `name` as their target, whatever the
    /// declaring file: one binary search over the name-keyed run groups,
    /// the matched group's runs yielded target-ascending, each run's rows
    /// ascending by `(use file, offset)` (the table order). The workspace
    /// query join's entry point — one search per referencing library
    /// instead of one per (decl file, library) pair.
    ///
    /// The group slice runs from the SLOT `name_runs[start]` — the start
    /// index into `name_runs` itself under-counts (the k-th distinct name
    /// starts at slot ≥ k), and slicing `name_refs[start..]` leaked the
    /// first run of every name group preceding the queried one in id
    /// order (#2652): `query references` answered sibling members' rows
    /// — which exact siblings leaked followed the StringId allocation
    /// order, so parallel folds' interleaved interning made the answer
    /// vary run to run.
    #[must_use]
    pub fn references_named(&self, name: StringId) -> impl Iterator<Item = &[RefRow]> + '_ {
        let group_name = |slot: usize| {
            let run = self.name_refs[slot] as usize;
            self.refs[self.runs[run] as usize].target_name
        };
        let start = self
            .name_runs
            .partition_point(|&slot| group_name(slot as usize) < name);
        let (group, end) = match self.name_runs.get(start) {
            Some(&slot) if group_name(slot as usize) == name => (
                slot as usize,
                self.name_runs
                    .get(start + 1)
                    .map_or(self.name_refs.len(), |&next| next as usize),
            ),
            _ => (0, 0),
        };
        self.name_refs[group..end].iter().map(move |&run| {
            let run = run as usize;
            let end = self
                .runs
                .get(run + 1)
                .map_or(self.refs.len(), |&next| next as usize);
            &self.refs[self.runs[run] as usize..end]
        })
    }

    /// The distinct reference-target names, ascending — the name-keyed run
    /// groups themselves, one name per group.
    #[must_use]
    pub fn distinct_reference_names(&self) -> impl Iterator<Item = StringId> + '_ {
        self.name_runs.iter().copied().map(move |slot| {
            let run = self.name_refs[slot as usize] as usize;
            self.refs[self.runs[run] as usize].target_name
        })
    }

    /// The unresolved uses of `name`: built-in rows from the sorted table
    /// plus seam rows recorded after the build
    /// ([`Self::record_unresolved`]), each table ascending by
    /// `(name, offset)`.
    pub fn unresolved(&self, name: StringId) -> impl Iterator<Item = &NameRow> + '_ {
        let lo = self.names.partition_point(|row| row.name < name);
        let hi = self.names.partition_point(|row| row.name <= name);
        let overflow_lo = self.names_overflow.partition_point(|row| row.name < name);
        let overflow_hi = self.names_overflow.partition_point(|row| row.name <= name);
        self.names[lo..hi]
            .iter()
            .chain(self.names_overflow[overflow_lo..overflow_hi].iter())
    }

    /// The direct supertypes of `(file, name)`, every clause kind included
    /// (`extends`, `with`, `implements`, a mixin's `on`).
    #[must_use]
    pub fn supertypes_of(&self, file: FileId, name: StringId) -> Option<&[SubtypeEdge]> {
        let key = |edge: &SubtypeEdge| (edge.file, edge.name);
        let start = self.supers.partition_point(|edge| key(edge) < (file, name));
        let end = self.supers[start..].partition_point(|edge| key(edge) <= (file, name)) + start;
        (start < end).then(|| &self.supers[start..end])
    }

    /// Records unresolved-name rows after the build — the #120 seam
    /// (`undefined_*` lookup byproducts; see the module docs).
    ///
    /// One call per recording phase: the rows join the overflow buffer and
    /// a single sort restores the `(name, offset)` order, so a miss-heavy
    /// rebuild pays one O(n log n) pass instead of a per-row insert, and
    /// [`Self::unresolved`] probes stay a binary search either way. A
    /// library rebuild replaces the whole index, and the resolver that
    /// produced the byproducts regenerates them.
    pub fn record_unresolved(&mut self, rows: impl IntoIterator<Item = NameRow>) {
        self.names_overflow.extend(rows);
        self.names_overflow
            .sort_unstable_by_key(|row| (row.name, row.range.offset));
    }

    /// Every reference row, ordered by the table's sort (CSR source data).
    #[must_use]
    pub fn refs(&self) -> &[RefRow] {
        &self.refs
    }

    /// Every resolved supertype edge, ordered by the table's sort
    /// (`(subtype file, name, clause)` — the export face's document
    /// assembly filters this slice by file, keeping the stored order).
    #[must_use]
    pub fn supers(&self) -> &[SubtypeEdge] {
        &self.supers
    }

    /// The surface's owned heap bytes: the boxed columns at their exact
    /// lengths plus the overflow rows at reserved capacity. Exact over the
    /// columns this type owns; the allocator's per-block rounding is not
    /// counted. The engine's memory decomposition reads it.
    #[must_use]
    pub fn heap_bytes(&self) -> usize {
        use std::mem::size_of;
        self.refs.len() * size_of::<RefRow>()
            + (self.runs.len() + self.name_runs.len() + self.name_refs.len()) * size_of::<u32>()
            + self.names.len() * size_of::<NameRow>()
            + self.names_overflow.capacity() * size_of::<NameRow>()
            + self.supers.len() * size_of::<SubtypeEdge>()
    }

    /// The number of resolved reference rows.
    #[must_use]
    pub fn reference_count(&self) -> usize {
        self.refs.len()
    }
}

/// The outcome of a bare-name resolution attempt.
enum BareHit {
    /// A declaration: the current file (locals beat imports) or a namespace
    /// target.
    Decl(FileId),
    /// An import prefix: its name, which arms the next continuation.
    Prefix(StringId),
    /// Not found, or lazily ambiguous — a names-table row either way.
    Miss,
}

/// Builds one library's [`LibraryIndex`] from its files' captures.
///
/// The caller captures every file of the library (defining unit plus
/// parts) and adds them in any order; the namespaces must already exist
/// (the graph's export scopes fold the whole workspace, so cross-library
/// targets resolve exactly like the L1 `LibraryAnalyzed` point — the
/// M1 build timing).
pub struct LibraryBuilder<'a> {
    lib: LibraryId,
    graph: &'a LibraryGraph,
    namespaces: &'a Namespaces,
    refs: Vec<RefRow>,
    names: Vec<NameRow>,
    supers: Vec<SubtypeEdge>,
}

impl<'a> LibraryBuilder<'a> {
    /// Starts a build for `lib` over the workspace's `graph` and
    /// `namespaces`.
    #[must_use]
    pub fn new(lib: LibraryId, graph: &'a LibraryGraph, namespaces: &'a Namespaces) -> Self {
        Self {
            lib,
            graph,
            namespaces,
            refs: Vec::new(),
            names: Vec::new(),
            supers: Vec::new(),
        }
    }

    /// Resolves and folds one file's raw sites into the tables.
    pub fn add_file(&mut self, capture: &FileCapture) {
        let file = capture.file;
        let sites = &capture.sites;
        let mut armed: Option<StringId> = None;
        for i in 0..sites.len() {
            let name = sites.name(i);
            let kind = sites.kind(i);
            let range = sites.range(i);
            match sites.family(i) {
                Family::Bare => {
                    // A prefix arms only the continuation immediately
                    // following it: any intervening bare site disarms.
                    armed = None;
                    match self.resolve_bare(file, &capture.index, name) {
                        BareHit::Decl(target) => self.refs.push(RefRow {
                            file,
                            target_file: target,
                            target_name: name,
                            target_container: StringId::NONE,
                            kind,
                            range,
                            qualifier: StringId::NONE,
                        }),
                        BareHit::Prefix(prefix) => armed = Some(prefix),
                        BareHit::Miss => self.names.push(NameRow {
                            name,
                            kind,
                            range,
                            qualified: false,
                        }),
                    }
                }
                Family::Continuation => {
                    let hit = armed.take().map(|prefix| {
                        (
                            prefix,
                            self.namespaces
                                .lookup_prefixed(self.graph, self.lib, prefix, name),
                        )
                    });
                    match hit {
                        Some((prefix, Lookup::Found(NamespaceEntry::Decl(target)))) => {
                            self.refs.push(RefRow {
                                file,
                                target_file: target,
                                target_name: name,
                                target_container: StringId::NONE,
                                kind,
                                range,
                                qualifier: prefix,
                            });
                        }
                        // No armed prefix (member access on an untyped
                        // receiver), an ambiguous prefix hit, or a miss.
                        _ => self.names.push(NameRow {
                            name,
                            kind,
                            range,
                            qualified: true,
                        }),
                    }
                }
            }
        }
        for edge in capture.index.supers() {
            let container = capture.index.row(edge.container);
            let supertype = match self.resolve_bare(capture.file, &capture.index, edge.name) {
                BareHit::Decl(target) => TargetRef::Decl(target, edge.name),
                BareHit::Prefix(_) | BareHit::Miss => TargetRef::Name(edge.name),
            };
            self.supers.push(SubtypeEdge {
                file: capture.file,
                name: container.name,
                supertype,
                clause: edge.clause,
            });
        }
    }

    /// The M1 scope chain for a bare name: the file's own declarations
    /// first (library faces only — members and type parameters are not
    /// scope names), then the library's namespace.
    fn resolve_bare(&self, file: FileId, index: &FileIndex, name: StringId) -> BareHit {
        if index.has_library_face(name) {
            return BareHit::Decl(file);
        }
        match self.namespaces.lookup(self.graph, self.lib, name) {
            Lookup::Found(NamespaceEntry::Decl(target)) => BareHit::Decl(target),
            Lookup::Found(NamespaceEntry::Prefix) => BareHit::Prefix(name),
            Lookup::Ambiguous(_) | Lookup::NotFound => BareHit::Miss,
        }
    }

    /// Folds the semantically resolved member-access reference edges of
    /// one file (#2618): each row is a complete [`RefRow`] (use site, kind,
    /// target) pushed beside the parser-level sites. Member sends resolve
    /// only with receiver type information, so the capture layer cannot
    /// produce them — the diagnose fold publishes them and the engine
    /// hands them in here. Order is irrelevant ([`Self::finish`] sorts the
    /// merged table); duplicate protection is by construction (one row per
    /// use site — the capture's continuation sites for member sends never
    /// resolve bare, so no parser-level row can double a semantic one).
    pub fn add_resolved_refs(&mut self, rows: impl IntoIterator<Item = RefRow>) {
        self.refs.extend(rows);
    }

    /// Sorts the tables into their probed order and finishes the index.
    #[must_use]
    pub fn finish(mut self) -> LibraryIndex {
        // Deterministic total orders (queries merge name-keyed
        // and element-keyed data, so any nondeterminism leaks into output).
        // The use `file` joins the key before the offset: a multi-file
        // library's member captures can carry equal offsets, and the caller
        // may add files in any order — the file breaks the tie
        // deterministically.
        self.refs
            .sort_by_key(|row| (row.target_file, row.target_name, row.file, row.range.offset));
        let mut runs = Vec::new();
        let mut prev: Option<(FileId, StringId)> = None;
        for (i, row) in self.refs.iter().enumerate() {
            let key = (row.target_file, row.target_name);
            if prev != Some(key) {
                runs.push(crate::narrow(i));
                prev = Some(key);
            }
        }
        // The name-keyed face of the same runs: run indices regrouped by
        // target name — the stable sort keeps each name's runs in their
        // target-ascending table order — plus one group-start offset per
        // distinct name (see `LibraryIndex`'s memory note for the trade).
        let mut name_refs: Vec<u32> = (0..crate::narrow(runs.len())).collect();
        name_refs.sort_by_key(|&run| self.refs[runs[run as usize] as usize].target_name);
        let mut name_runs = Vec::new();
        let mut prev_name: Option<StringId> = None;
        for (slot, &run) in name_refs.iter().enumerate() {
            let name = self.refs[runs[run as usize] as usize].target_name;
            if prev_name != Some(name) {
                name_runs.push(crate::narrow(slot));
                prev_name = Some(name);
            }
        }
        self.names.sort_by_key(|row| (row.name, row.range.offset));
        self.supers
            .sort_by_key(|edge| (edge.file, edge.name, edge.clause));
        // Freeze: the push-grown tables move into the long-lived index as
        // boxed slices, so std's doubling slack (up to ~2× capacity resident
        // for the session) is shed by construction — `into_boxed_slice`
        // reallocates to the exact row count on every column, run tables
        // included (they were the slack the earlier `Vec` shape could not
        // shed).
        LibraryIndex {
            refs: self.refs.into_boxed_slice(),
            runs: runs.into_boxed_slice(),
            name_runs: name_runs.into_boxed_slice(),
            name_refs: name_refs.into_boxed_slice(),
            names: self.names.into_boxed_slice(),
            names_overflow: Vec::new(),
            supers: self.supers.into_boxed_slice(),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::capture::{DeclKind, capture};
    use crate::{SymbolDecl, SymbolIndex};
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_parser::ParserFeatures;
    use dartq_resolution::{
        Combinators, ImportEdge, LibraryGraph, LibraryGraphBuilder, Namespaces,
    };
    use dartq_scanner::{ScanConfig, TokenTable};
    use dartq_source::{FileTable, StringTable};
    use std::mem::size_of;
    use std::path::Path;

    const SRC_A: &str = "class A {\n  void m() {}\n}\n\nvoid fa() {}\nint top = 0;\n";
    pub(crate) const SRC_B: &str = "import 'a.dart';\nimport 'a.dart' as p;\n\nclass B extends A {\n  void use() {\n    A();\n    fa();\n    p.fa();\n    top = 1;\n    nope;\n  }\n}\n";

    /// The two-file fixture: `a.dart` declares
    /// the surface, `b.dart` imports it twice (bare and prefixed), extends
    /// `A`, invokes across the boundary, writes a top-level, and holds one
    /// unresolvable use.
    pub(crate) struct Fixture {
        pub(crate) strings: &'static StringTable,
        pub(crate) files: FileTable,
        pub(crate) graph: LibraryGraph,
        pub(crate) namespaces: Namespaces,
        pub(crate) lib_a: LibraryId,
        pub(crate) lib_b: LibraryId,
        pub(crate) capture_a: FileCapture,
        pub(crate) capture_b: FileCapture,
    }

    pub(crate) fn capture_one(
        strings: &StringTable,
        files: &mut FileTable,
        path: &str,
        source: &str,
    ) -> FileCapture {
        let file = files.intern(Path::new(path));
        let result = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(result.diagnostics.is_empty(), "{source:?}");
        let tokens: &'static TokenTable = Box::leak(Box::new(result.tokens));
        let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
            LanguageVersion::new(3, 15),
        ));
        capture(file, source, tokens, features, strings)
    }

    pub(crate) fn fixture() -> Fixture {
        let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
        let mut files = FileTable::new();
        let capture_a = capture_one(strings, &mut files, "a.dart", SRC_A);
        let capture_b = capture_one(strings, &mut files, "b.dart", SRC_B);

        let mut builder = LibraryGraphBuilder::new(strings);
        let lib_a = builder.intern("package:t/a.dart");
        let lib_b = builder.intern("package:t/b.dart");
        let file_a = files.try_file_id(Path::new("a.dart")).unwrap();
        let file_b = files.try_file_id(Path::new("b.dart")).unwrap();
        for (lib, file, elements) in [
            (lib_a, file_a, &capture_a.index),
            (lib_b, file_b, &capture_b.index),
        ] {
            for row in elements.decls() {
                if row.kind == DeclKind::TypeParam || row.container != crate::capture::DeclRow::NONE
                {
                    continue; // members and type params are not library faces
                }
                builder.declare(lib, row.name, file);
            }
        }
        builder.import(
            lib_b,
            ImportEdge {
                target: Some(lib_a),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        builder.import(
            lib_b,
            ImportEdge {
                target: Some(lib_a),
                prefix: Some(strings.intern("p")),
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        Fixture {
            strings,
            files,
            graph,
            namespaces,
            lib_a,
            lib_b,
            capture_a,
            capture_b,
        }
    }

    /// The fallthrough arms of the resolution core: a continuation with no
    /// armed prefix (member access on an undeclared receiver), a prefix
    /// armed but the name missing from its scope, a supertype outside the
    /// analyzed set (the `TargetRef::Name` fallback), the absent-reference
    /// probe, and a seam row merging with built-in rows of the same name.
    #[test]
    fn name_row_fallthrough_and_seam_merge() {
        let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
        let mut files = FileTable::new();
        let main = capture_one(
            strings,
            &mut files,
            "m.dart",
            "import 'd.dart' as p;\nclass C extends Missing {}\nvoid main() {\n  x.foo;\n  p.missing();\n}\n",
        );
        let dep = capture_one(strings, &mut files, "d.dart", "void gone() {}\n");

        let mut builder = LibraryGraphBuilder::new(strings);
        let lib_m = builder.intern("package:t/m.dart");
        let lib_d = builder.intern("package:t/d.dart");
        let file_m = files.try_file_id(Path::new("m.dart")).unwrap();
        let file_d = files.try_file_id(Path::new("d.dart")).unwrap();
        for (lib, file, cap) in [(lib_m, file_m, &main.index), (lib_d, file_d, &dep.index)] {
            for row in cap.decls() {
                if row.kind == DeclKind::TypeParam || row.container != crate::capture::DeclRow::NONE
                {
                    continue; // members and type params are not library faces
                }
                builder.declare(lib, row.name, file);
            }
        }
        builder.import(
            lib_m,
            ImportEdge {
                target: Some(lib_d),
                prefix: Some(strings.intern("p")),
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);

        let mut builder = LibraryBuilder::new(lib_m, &graph, &namespaces);
        builder.add_file(&main);
        builder.add_file(&dep);
        let surface = builder.finish();

        // `x.foo;`: the receiver is an unresolvable bare name (unqualified
        // row); `foo` is a member access with no armed prefix (qualified
        // row).
        let x_rows: Vec<&NameRow> = surface.unresolved(strings.intern("x")).collect();
        assert_eq!(x_rows.len(), 1);
        assert_eq!(x_rows[0].kind, RefKind::Read);
        assert!(!x_rows[0].qualified);
        let x_offset = x_rows[0].range.offset;
        let foo: Vec<&NameRow> = surface.unresolved(strings.intern("foo")).collect();
        assert_eq!(foo.len(), 1);
        assert!(foo[0].qualified, "member access with no prefix arm");

        // `p.missing();`: the prefix arms, the name misses its scope — the
        // invocation keeps its kind and gains the qualified bit.
        let missing: Vec<&NameRow> = surface.unresolved(strings.intern("missing")).collect();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].kind, RefKind::Invoked);
        assert!(missing[0].qualified);

        // No bare use resolved, so the reference table stays empty and
        // every probe misses.
        assert_eq!(surface.reference_count(), 0);
        assert!(
            surface
                .references_of(file_d, strings.intern("gone"))
                .is_none()
        );

        // `extends Missing`: the supertype lies outside the analyzed set —
        // the bare-name fallback, not a declaration.
        let supers = surface
            .supertypes_of(file_m, strings.intern("C"))
            .expect("C supertype edge");
        assert_eq!(supers.len(), 1);
        assert_eq!(
            supers[0].supertype,
            TargetRef::Name(strings.intern("Missing"))
        );

        // The seam merge: a byproduct row for a name that already has a
        // built-in row is served after it, from one probe.
        let mut surface = surface;
        surface.record_unresolved(std::iter::once(NameRow {
            name: strings.intern("x"),
            kind: RefKind::Read,
            range: SourceRange {
                offset: 9_999,
                length: 1,
            },
            qualified: false,
        }));
        let merged: Vec<(u32, bool)> = surface
            .unresolved(strings.intern("x"))
            .map(|row| (row.range.offset, row.qualified))
            .collect();
        assert_eq!(
            merged,
            vec![(x_offset, false), (9_999, false)],
            "built-in rows first, seam rows after"
        );
    }

    /// The seam's scale shape: 10k rows land in one out-of-order batch and
    /// one sort serves every probe in `(name, offset)` order — the batch
    /// contract the `undefined_*` rebuild flow will drive (correctness of
    /// the ordering here; its cost is the bench phase's business).
    #[test]
    fn seam_batch_serves_out_of_order_rows_sorted() {
        let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
        let names: Vec<StringId> = (0..100)
            .map(|i| strings.intern(&format!("seam_name_{i:03}")))
            .collect();
        let mut surface = LibraryIndex::default();

        // Name-major descending, offset descending within — the exact
        // reverse of the probed order.
        let rows: Vec<NameRow> = names
            .iter()
            .rev()
            .flat_map(|&name| {
                (0..100u32).rev().map(move |k| NameRow {
                    name,
                    kind: RefKind::Read,
                    range: SourceRange {
                        offset: k * 16,
                        length: 4,
                    },
                    qualified: false,
                })
            })
            .collect();
        assert_eq!(rows.len(), 10_000);
        surface.record_unresolved(rows);

        let expected: Vec<u32> = (0..100u32).map(|k| k * 16).collect();
        for &name in &names {
            let offsets: Vec<u32> = surface
                .unresolved(name)
                .map(|row| row.range.offset)
                .collect();
            assert_eq!(offsets, expected, "{name:?}: rows offset-ascending");
        }
        assert_eq!(
            surface.unresolved(strings.intern("seam_absent")).count(),
            0,
            "a name outside the batch misses"
        );
    }

    /// The two-file fixture through the whole face: cross-file resolution
    /// (bare and prefixed), write tagging, the supertype edge, the
    /// unresolved-names table, the #120 seam, and the workspace query
    /// round-trip.
    #[test]
    fn cross_file_resolution_and_query_round_trip() {
        let fx = fixture();
        let strings = fx.strings;
        let file_a = fx.files.try_file_id(Path::new("a.dart")).unwrap();
        let file_b = fx.files.try_file_id(Path::new("b.dart")).unwrap();

        let mut builder = LibraryBuilder::new(fx.lib_b, &fx.graph, &fx.namespaces);
        builder.add_file(&fx.capture_b);
        let b_surface = builder.finish();
        let mut builder = LibraryBuilder::new(fx.lib_a, &fx.graph, &fx.namespaces);
        builder.add_file(&fx.capture_a);
        let a_surface = builder.finish();

        // Bare cross-file invocation, and the prefixed twin: same target,
        // one unqualified, one qualified — the `isQualified` bit.
        let fa_refs = b_surface
            .references_of(file_a, strings.intern("fa"))
            .expect("fa referenced from b");
        assert_eq!(fa_refs.len(), 2);
        assert!(fa_refs[0].range.offset < fa_refs[1].range.offset);
        assert!(fa_refs.iter().all(|row| row.kind == RefKind::Invoked));
        assert_eq!(
            fa_refs
                .iter()
                .map(|row| row.qualified())
                .collect::<Vec<_>>(),
            vec![false, true]
        );
        assert_eq!(
            fa_refs.iter().map(|row| row.qualifier).collect::<Vec<_>>(),
            vec![StringId::NONE, strings.intern("p")],
            "the prefixed twin carries its import prefix, the bare twin none"
        );
        // The call span pins to the source (`fa()` line 6 of b).
        let fa_call = SRC_B.find("    fa();").expect("bare call") + 4;
        assert_eq!(fa_refs[0].range.offset as usize, fa_call);
        assert_eq!(fa_refs[0].range.length as usize, 2);

        // Write tagging across the boundary: `top = 1` writes.
        let top_refs = b_surface
            .references_of(file_a, strings.intern("top"))
            .expect("top written from b");
        assert_eq!(top_refs.len(), 1);
        assert_eq!(top_refs[0].kind, RefKind::Written);

        // `A()`: an invocation; `extends A`: a reference — same target.
        let a_refs = b_surface
            .references_of(file_a, strings.intern("A"))
            .expect("A referenced from b");
        assert_eq!(
            a_refs.iter().map(|row| row.kind).collect::<Vec<_>>(),
            vec![RefKind::Referenced, RefKind::Invoked]
        );

        // Unresolved: the member access `nope` (unknown receiver shape →
        // the names table), and nothing else from this fixture's uses.
        let nope = b_surface
            .unresolved(strings.intern("nope"))
            .collect::<Vec<_>>();
        assert_eq!(nope.len(), 1);
        assert_eq!(nope[0].kind, RefKind::Read);
        assert!(!nope[0].qualified);
        assert!(
            b_surface.unresolved(strings.intern("fa")).next().is_none(),
            "resolved uses never land in the names table"
        );

        // Supertype edge: B extends A resolves cross-file.
        let supers = b_surface
            .supertypes_of(file_b, strings.intern("B"))
            .expect("B supertype edge");
        assert_eq!(supers.len(), 1);
        assert_eq!(
            supers[0].supertype,
            TargetRef::Decl(file_a, strings.intern("A"))
        );

        // The #120 seam: a byproduct row joins the probe immediately.
        let mut b_surface = b_surface;
        b_surface.record_unresolved(std::iter::once(NameRow {
            name: strings.intern("orphan"),
            kind: RefKind::Read,
            range: SourceRange {
                offset: 42,
                length: 6,
            },
            qualified: false,
        }));
        assert_eq!(
            b_surface.unresolved(strings.intern("orphan")).count(),
            1,
            "seam rows are served merged"
        );

        // Workspace round trip: query symbol by name → files+kinds+spans;
        // the reverse face loops libraries.
        let mut index = SymbolIndex::default();
        index.insert_file(fx.capture_a.file, fx.capture_a.index);
        index.insert_file(fx.capture_b.file, fx.capture_b.index);
        index.insert_library(fx.lib_a, a_surface);
        index.insert_library(fx.lib_b, b_surface);
        let hits: Vec<SymbolDecl<'_>> = index.symbols(strings.intern("A")).collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].file, file_a);
        assert_eq!(hits[0].decl.kind, DeclKind::Class);
        let back: Vec<&RefRow> = index.references(file_a, strings.intern("fa")).collect();
        assert_eq!(
            back.len(),
            2,
            "both b-side call sites, one per library scan"
        );
        assert!(index.library(fx.lib_b).is_some());
        assert!(index.file(file_a).is_some());
    }

    /// The local hop of a bare lookup matches library-face rows only: a
    /// member sharing a name with an imported top-level must not shadow it,
    /// a type parameter is no scope name either, and a genuine top-level
    /// declaration still beats the namespace.
    #[test]
    fn local_hop_matches_library_faces_only() {
        let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
        let mut files = FileTable::new();
        let dep = capture_one(strings, &mut files, "d.dart", "void fa() {}\n");
        let main = capture_one(
            strings,
            &mut files,
            "m.dart",
            "import 'd.dart';\nclass K {\n  void fa() {}\n}\nclass K2<Q> {\n  Q? f;\n}\nvoid fa2() {}\nvoid main() {\n  fa();\n  fa2();\n}\n",
        );

        let mut builder = LibraryGraphBuilder::new(strings);
        let lib_m = builder.intern("package:t/m.dart");
        let lib_d = builder.intern("package:t/d.dart");
        let file_m = files.try_file_id(Path::new("m.dart")).unwrap();
        let file_d = files.try_file_id(Path::new("d.dart")).unwrap();
        for (lib, file, cap) in [(lib_m, file_m, &main.index), (lib_d, file_d, &dep.index)] {
            for row in cap.decls() {
                if row.kind == DeclKind::TypeParam || row.container != crate::capture::DeclRow::NONE
                {
                    continue; // members and type params are not library faces
                }
                builder.declare(lib, row.name, file);
            }
        }
        builder.import(
            lib_m,
            ImportEdge {
                target: Some(lib_d),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);

        let mut builder = LibraryBuilder::new(lib_m, &graph, &namespaces);
        builder.add_file(&main);
        builder.add_file(&dep);
        let surface = builder.finish();

        // `fa();`: declared only as a member locally — the member row must
        // not satisfy the local hop, so the namespace hit on d.dart wins.
        let fa = surface
            .references_of(file_d, strings.intern("fa"))
            .expect("fa resolves through the namespace");
        assert_eq!(fa.len(), 1);
        assert_eq!(fa[0].kind, RefKind::Invoked);
        assert!(
            surface
                .references_of(file_m, strings.intern("fa"))
                .is_none(),
            "the member row must not shadow the import"
        );

        // `fa2();`: a genuine top-level declaration still beats imports.
        let fa2 = surface
            .references_of(file_m, strings.intern("fa2"))
            .expect("local top-level wins");
        assert_eq!(fa2.len(), 1);

        // `Q? f;`: a type parameter is not a library face — the use falls
        // through to the namespace, misses, and lands in the names table.
        assert!(
            surface.references_of(file_m, strings.intern("Q")).is_none(),
            "a type parameter must not satisfy the local hop"
        );
        let q = surface.unresolved(strings.intern("Q")).collect::<Vec<_>>();
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].kind, RefKind::Referenced);
    }

    /// A member of an unnamed extension is not a library face (the
    /// `unnamed_extension_member` bit): a same-named import wins the bare
    /// use instead of the file-local hop shadowing it.
    #[test]
    fn unnamed_extension_member_loses_to_import() {
        let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
        let mut files = FileTable::new();
        let dep = capture_one(strings, &mut files, "d.dart", "void fa() {}\n");
        let main = capture_one(
            strings,
            &mut files,
            "m.dart",
            "import 'd.dart';\nextension on String {\n  int get fa => 1;\n}\nvoid main() {\n  fa;\n}\n",
        );

        let mut builder = LibraryGraphBuilder::new(strings);
        let lib_m = builder.intern("package:t/m.dart");
        let lib_d = builder.intern("package:t/d.dart");
        let file_m = files.try_file_id(Path::new("m.dart")).unwrap();
        let file_d = files.try_file_id(Path::new("d.dart")).unwrap();
        for (lib, file, cap) in [(lib_m, file_m, &main.index), (lib_d, file_d, &dep.index)] {
            for row in cap.decls() {
                if row.is_library_face() {
                    builder.declare(lib, row.name, file);
                }
            }
        }
        builder.import(
            lib_m,
            ImportEdge {
                target: Some(lib_d),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);

        let mut builder = LibraryBuilder::new(lib_m, &graph, &namespaces);
        builder.add_file(&main);
        builder.add_file(&dep);
        let surface = builder.finish();

        // The bare use falls through the local hop (the member row is not a
        // face) and resolves through the namespace to d.dart.
        let fa = surface
            .references_of(file_d, strings.intern("fa"))
            .expect("the import wins the bare use");
        assert_eq!(fa.len(), 1);
        assert_eq!(fa[0].kind, RefKind::Read);
        assert!(
            surface
                .references_of(file_m, strings.intern("fa"))
                .is_none(),
            "the unnamed-extension member must not shadow the import"
        );
    }

    /// A prefix arms only the continuation immediately following it: an
    /// intervening bare site disarms, so the next continuation is a member
    /// access even when its name exists in the prefix's scope.
    #[test]
    fn prefix_arm_does_not_leak_across_bare_sites() {
        let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
        let mut files = FileTable::new();
        let dep = capture_one(
            strings,
            &mut files,
            "d.dart",
            "void gone() {}\nvoid w() {}\n",
        );
        let main = capture_one(
            strings,
            &mut files,
            "m.dart",
            "import 'd.dart' as p;\nvoid main() {\n  p.gone;\n  z.w;\n}\n",
        );

        let mut builder = LibraryGraphBuilder::new(strings);
        let lib_m = builder.intern("package:t/m.dart");
        let lib_d = builder.intern("package:t/d.dart");
        let file_m = files.try_file_id(Path::new("m.dart")).unwrap();
        let file_d = files.try_file_id(Path::new("d.dart")).unwrap();
        for (lib, file, cap) in [(lib_m, file_m, &main.index), (lib_d, file_d, &dep.index)] {
            for row in cap.decls() {
                if row.kind == DeclKind::TypeParam || row.container != crate::capture::DeclRow::NONE
                {
                    continue; // members and type params are not library faces
                }
                builder.declare(lib, row.name, file);
            }
        }
        builder.import(
            lib_m,
            ImportEdge {
                target: Some(lib_d),
                prefix: Some(strings.intern("p")),
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);

        let mut builder = LibraryBuilder::new(lib_m, &graph, &namespaces);
        builder.add_file(&main);
        builder.add_file(&dep);
        let surface = builder.finish();

        // `p.gone;`: the prefix arms and the continuation resolves.
        let gone = surface
            .references_of(file_d, strings.intern("gone"))
            .expect("prefixed hit");
        assert_eq!(gone.len(), 1);
        assert!(gone[0].qualified());
        assert_eq!(gone[0].qualifier, strings.intern("p"));

        // `z.w;`: the bare miss `z` disarmed the prefix, so `w` is a member
        // access even though d.dart declares it.
        assert!(
            surface.references_of(file_d, strings.intern("w")).is_none(),
            "the disarmed prefix must not resolve the later continuation"
        );
        let w = surface.unresolved(strings.intern("w")).collect::<Vec<_>>();
        assert_eq!(w.len(), 1);
        assert!(w[0].qualified, "member access shape");
        let z = surface.unresolved(strings.intern("z")).collect::<Vec<_>>();
        assert_eq!(z.len(), 1);
        assert!(!z[0].qualified);
    }

    /// The use-file column (#361): a multi-file library folds its members'
    /// captures into one surface, and every row carries the member file the
    /// use site sits in — the target pair alone cannot locate it.
    #[test]
    fn use_file_tracks_the_member_file() {
        let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
        let mut files = FileTable::new();
        let dep = capture_one(strings, &mut files, "d.dart", "void gone() {}\n");
        let primary = capture_one(
            strings,
            &mut files,
            "m.dart",
            "import 'd.dart';\nvoid main() {\n  gone();\n}\n",
        );
        let part = capture_one(
            strings,
            &mut files,
            "m_part.dart",
            "part of 'm.dart';\nvoid helper() {\n  gone();\n}\n",
        );

        let mut builder = LibraryGraphBuilder::new(strings);
        let lib_m = builder.intern("package:t/m.dart");
        let lib_d = builder.intern("package:t/d.dart");
        let file_d = files.try_file_id(Path::new("d.dart")).unwrap();
        let file_m = files.try_file_id(Path::new("m.dart")).unwrap();
        let file_p = files.try_file_id(Path::new("m_part.dart")).unwrap();
        for (lib, file, cap) in [
            (lib_m, file_m, &primary.index),
            (lib_m, file_p, &part.index),
            (lib_d, file_d, &dep.index),
        ] {
            for row in cap.decls() {
                if row.is_library_face() {
                    builder.declare(lib, row.name, file);
                }
            }
        }
        builder.import(
            lib_m,
            ImportEdge {
                target: Some(lib_d),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);

        // One library, two members: the folding order is the builder's
        // input order, and both members' uses resolve to the same target.
        let mut builder = LibraryBuilder::new(lib_m, &graph, &namespaces);
        builder.add_file(&primary);
        builder.add_file(&part);
        let surface = builder.finish();

        let gone: &[RefRow] = surface
            .references_of(file_d, strings.intern("gone"))
            .expect("both members' uses resolve");
        assert_eq!(gone.len(), 2);
        let members: Vec<(FileId, RefKind)> = gone.iter().map(|row| (row.file, row.kind)).collect();
        assert_eq!(
            members,
            vec![(file_m, RefKind::Invoked), (file_p, RefKind::Invoked)],
            "each row names the member file its use site sits in"
        );
        assert!(
            members.iter().all(|&(file, _)| file != file_d),
            "the use file is the library member, never the target's file"
        );
    }

    /// The name-keyed run groups (the workspace query join's face): one
    /// library referencing the same name in two target files answers both
    /// runs target-ascending off a single search, each group's slice
    /// matches the exact-target probe, a miss searches to empty, and the
    /// distinct-name walk names each group once.
    #[test]
    fn name_groups_serve_every_target_off_one_search() {
        let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
        let mut files = FileTable::new();
        let dep_d = capture_one(strings, &mut files, "d.dart", "void fa() {}\n");
        let dep_e = capture_one(strings, &mut files, "e.dart", "void fa() {}\n");
        let main = capture_one(
            strings,
            &mut files,
            "m.dart",
            "import 'd.dart' as p;\nimport 'e.dart' as q;\n\nvoid main() {\n p.fa();\n q.fa();\n}\n",
        );

        let mut builder = LibraryGraphBuilder::new(strings);
        let lib_d = builder.intern("package:t/d.dart");
        let lib_e = builder.intern("package:t/e.dart");
        let lib_m = builder.intern("package:t/m.dart");
        let file_d = files.try_file_id(Path::new("d.dart")).unwrap();
        let file_e = files.try_file_id(Path::new("e.dart")).unwrap();
        for (lib, cap) in [(lib_d, &dep_d), (lib_e, &dep_e), (lib_m, &main)] {
            for row in cap.index.decls() {
                if row.is_library_face() {
                    builder.declare(lib, row.name, cap.file);
                }
            }
        }
        for (prefix, target) in [("p", lib_d), ("q", lib_e)] {
            builder.import(
                lib_m,
                ImportEdge {
                    target: Some(target),
                    prefix: Some(strings.intern(prefix)),
                    deferred: false,
                    combinators: Combinators::unfiltered(),
                },
            );
        }
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);

        let mut builder = LibraryBuilder::new(lib_m, &graph, &namespaces);
        builder.add_file(&main);
        let surface = builder.finish();

        let fa = strings.intern("fa");
        let groups: Vec<&[RefRow]> = surface.references_named(fa).collect();
        assert_eq!(groups.len(), 2, "one run per target file");
        assert_eq!(groups[0][0].target_file, file_d, "runs target-ascending");
        assert_eq!(groups[1][0].target_file, file_e);
        assert_eq!(groups[0], surface.references_of(file_d, fa).expect("d hit"));
        assert_eq!(groups[1], surface.references_of(file_e, fa).expect("e hit"));
        assert!(
            surface
                .references_named(strings.intern("zz_absent"))
                .next()
                .is_none(),
            "a miss searches to empty"
        );
        let names: Vec<StringId> = surface.distinct_reference_names().collect();
        assert_eq!(names, vec![fa], "one group per distinct target name");
    }

    /// The name-keyed group of one name serves exactly that name's runs:
    /// a queried name whose group starts PAST a multi-run name's group
    /// must not leak the sibling's trailing run (#2652 — the group slice
    /// once started at the `name_runs` index instead of the slot, so a
    /// group whose start slot exceeds its group index — exactly what a
    /// multi-run predecessor creates — leaked the predecessor's later
    /// runs into every following name's answer, and parallel interning
    /// made the leaked set vary run to run). The rows ride
    /// `add_resolved_refs` (the member-edge face) so one name can span
    /// two target files exactly as a member declared beside an interface
    /// double does.
    #[test]
    fn name_groups_do_not_leak_neighbor_groups() {
        let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
        let mut files = FileTable::new();
        let main = capture_one(strings, &mut files, "m.dart", "void main() {}\n");
        let twin = capture_one(strings, &mut files, "w.dart", "void main() {}\n");
        let mut builder = LibraryGraphBuilder::new(strings);
        let lib_m = builder.intern("package:t/m.dart");
        builder.declare(lib_m, main.index.decls()[0].name, main.file);
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);

        // Interned in this order, so ids ascend: first < middle < last.
        // `first` spans TWO target files — two runs — so `middle`'s group
        // starts at slot 2 while its group index is 1.
        let first = strings.intern("alphaMember");
        let middle = strings.intern("betaMember");
        let last = strings.intern("gammaMember");
        let row = |name: StringId, target: FileId, offset: u32| RefRow {
            file: main.file,
            target_file: target,
            target_name: name,
            target_container: strings.intern("Host"),
            kind: RefKind::Invoked,
            range: dartq_source::SourceRange {
                offset,
                length: strings.resolve(name).len() as u32,
            },
            qualifier: StringId::NONE,
        };
        let mut builder = LibraryBuilder::new(lib_m, &graph, &namespaces);
        builder.add_file(&main);
        builder.add_resolved_refs([
            row(first, main.file, 10),
            row(first, twin.file, 20),
            row(middle, main.file, 30),
            row(last, main.file, 40),
        ]);
        let surface = builder.finish();

        for (probe, expected) in [(middle, 30), (last, 40)] {
            let rows: Vec<u32> = surface
                .references_named(probe)
                .flatten()
                .map(|row| row.range.offset)
                .collect();
            assert_eq!(
                rows,
                vec![expected],
                "{:?}: exactly its own run, no neighbor group's rows",
                strings.resolve(probe)
            );
        }
        // The first group keeps its whole two-run span, one run per target
        // file, and a miss stays empty.
        let rows: Vec<u32> = surface
            .references_named(first)
            .flatten()
            .map(|row| row.range.offset)
            .collect();
        assert_eq!(rows, vec![10, 20]);
        assert!(
            surface
                .references_named(strings.intern("zz_absent"))
                .next()
                .is_none()
        );
    }

    /// Clause-typed supers through the fold: every clause kind resolves by
    /// name the way extends did and carries its clause column through, and
    /// a same-named member (not a library face) does not shadow a clause
    /// target into a fabricated declaration.
    #[test]
    fn clause_typed_supers_fold_with_targets_and_clauses() {
        let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
        let mut files = FileTable::new();
        let dep = capture_one(
            strings,
            &mut files,
            "d.dart",
            "class A {}\nmixin M {}\nclass I {}\n",
        );
        let main = capture_one(
            strings,
            &mut files,
            "m.dart",
            "import 'd.dart';\nclass K {\n  I not_a_face;\n}\nclass C extends A with M implements I {}\n",
        );

        let mut builder = LibraryGraphBuilder::new(strings);
        let lib_m = builder.intern("package:t/m.dart");
        let lib_d = builder.intern("package:t/d.dart");
        let file_m = files.try_file_id(Path::new("m.dart")).unwrap();
        let file_d = files.try_file_id(Path::new("d.dart")).unwrap();
        for (lib, file, cap) in [(lib_m, file_m, &main.index), (lib_d, file_d, &dep.index)] {
            for row in cap.decls() {
                if row.is_library_face() {
                    builder.declare(lib, row.name, file);
                }
            }
        }
        builder.import(
            lib_m,
            ImportEdge {
                target: Some(lib_d),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);

        let mut builder = LibraryBuilder::new(lib_m, &graph, &namespaces);
        builder.add_file(&main);
        let surface = builder.finish();

        // All three clause kinds resolve cross-file, in `(name, clause)`
        // order, each carrying its clause.
        let edges = surface
            .supertypes_of(file_m, strings.intern("C"))
            .expect("C supertype edges");
        assert_eq!(
            edges
                .iter()
                .map(|edge| (edge.supertype, edge.clause))
                .collect::<Vec<_>>(),
            vec![
                (
                    TargetRef::Decl(file_d, strings.intern("A")),
                    SuperClause::Extends
                ),
                (
                    TargetRef::Decl(file_d, strings.intern("M")),
                    SuperClause::With
                ),
                (
                    TargetRef::Decl(file_d, strings.intern("I")),
                    SuperClause::Implements
                ),
            ]
        );
        // The member row `I not_a_face` is no library face: the implements
        // edge resolved to d.dart's I, not to m.dart (the shadowing
        // negative at fold level).
        assert!(
            edges
                .iter()
                .all(|edge| edge.supertype != TargetRef::Decl(file_m, strings.intern("I"))),
            "a member must not shadow a clause target"
        );
        assert!(
            surface.supertypes_of(file_m, strings.intern("K")).is_none(),
            "a container without clauses carries no edges"
        );
    }

    /// Whole-unit eviction: removing a file's or a library's surface drops
    /// it from every probe, and removing an absent id is a no-op.
    #[test]
    fn removal_drops_surfaces() {
        let fx = fixture();
        let mut builder = LibraryBuilder::new(fx.lib_a, &fx.graph, &fx.namespaces);
        builder.add_file(&fx.capture_a);
        let surface = builder.finish();
        let mut index = SymbolIndex::default();
        index.insert_file(fx.capture_a.file, fx.capture_a.index);
        index.insert_library(fx.lib_a, surface);

        assert!(index.remove_library(fx.lib_a).is_some());
        assert!(index.library(fx.lib_a).is_none());
        assert!(index.remove_library(fx.lib_a).is_none(), "absent id: no-op");
        assert!(index.remove_file(fx.capture_a.file).is_some());
        assert!(index.file(fx.capture_a.file).is_none());
    }

    /// The freeze-time shrink: `finish` moves the push-grown tables into
    /// the long-lived index as boxed slices — len == capacity by
    /// construction, so `heap_bytes` must equal the exact len sum and a
    /// `Vec`-backed regression (up to ~2× resident slack) trips here.
    #[test]
    fn finish_sheds_push_growth_slack() {
        let fx = fixture();
        let mut builder = LibraryBuilder::new(fx.lib_b, &fx.graph, &fx.namespaces);
        builder.add_file(&fx.capture_b);
        let surface = builder.finish();
        assert!(!surface.refs.is_empty(), "fixture has resolved uses");
        assert!(!surface.names.is_empty(), "fixture has an unresolved use");
        assert!(!surface.supers.is_empty(), "fixture has a supertype edge");
        let exact = surface.refs.len() * size_of::<RefRow>()
            + (surface.runs.len() + surface.name_runs.len() + surface.name_refs.len())
                * size_of::<u32>()
            + surface.names.len() * size_of::<NameRow>()
            + surface.supers.len() * size_of::<SubtypeEdge>();
        assert_eq!(
            surface.heap_bytes(),
            exact,
            "the frozen columns must hold no growth slack"
        );
    }

    /// The compacted shape answers identically on a shared-name fixture:
    /// two target files declaring the same name produce one name-keyed
    /// group whose runs resolve to each target exactly — the join shape the
    /// boxed-slice rebuild must not perturb (ordering, slicing, and run
    /// boundaries checked against the exact-target probes).
    #[test]
    fn shared_name_targets_answer_identically() {
        let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
        let mut files = FileTable::new();
        let dep_d = capture_one(strings, &mut files, "d.dart", "void fa() {}\n");
        let dep_e = capture_one(strings, &mut files, "e.dart", "void fa() {}\n");
        let main = capture_one(
            strings,
            &mut files,
            "m.dart",
            "import 'd.dart' as p;\nimport 'e.dart' as q;\n\nvoid main() {\n p.fa();\n q.fa();\n}\n",
        );

        let mut builder = LibraryGraphBuilder::new(strings);
        let lib_d = builder.intern("package:t/d.dart");
        let lib_e = builder.intern("package:t/e.dart");
        let lib_m = builder.intern("package:t/m.dart");
        let file_d = files.try_file_id(Path::new("d.dart")).unwrap();
        let file_e = files.try_file_id(Path::new("e.dart")).unwrap();
        for (lib, cap) in [(lib_d, &dep_d), (lib_e, &dep_e), (lib_m, &main)] {
            for row in cap.index.decls() {
                if row.is_library_face() {
                    builder.declare(lib, row.name, cap.file);
                }
            }
        }
        for (prefix, target) in [("p", lib_d), ("q", lib_e)] {
            builder.import(
                lib_m,
                ImportEdge {
                    target: Some(target),
                    prefix: Some(strings.intern(prefix)),
                    deferred: false,
                    combinators: Combinators::unfiltered(),
                },
            );
        }
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);

        let mut builder = LibraryBuilder::new(lib_m, &graph, &namespaces);
        builder.add_file(&main);
        let surface = builder.finish();

        // One name-keyed group spanning both targets; each run's rows are
        // exactly what the exact-target probe answers — the boxed-slice
        // layout serves the same slices.
        let fa = strings.intern("fa");
        let groups: Vec<&[RefRow]> = surface.references_named(fa).collect();
        assert_eq!(groups.len(), 2, "one run per shared-name target");
        let d_run = surface.references_of(file_d, fa).expect("d run");
        let e_run = surface.references_of(file_e, fa).expect("e run");
        assert!(groups.contains(&d_run) && groups.contains(&e_run));
        // The distinct-name walk yields the shared name exactly once, and
        // the name-keyed group order is the targets' ascending order.
        let names: Vec<StringId> = surface.distinct_reference_names().collect();
        assert_eq!(names, vec![fa]);
        assert_eq!(groups[0][0].target_file, file_d);
        assert_eq!(groups[1][0].target_file, file_e);
        // Heap accounting covers the shared run index exactly: two runs,
        // one name group.
        let exact = surface.refs.len() * size_of::<RefRow>()
            + (surface.runs.len() + surface.name_runs.len() + surface.name_refs.len())
                * size_of::<u32>()
            + surface.names.len() * size_of::<NameRow>()
            + surface.supers.len() * size_of::<SubtypeEdge>();
        assert_eq!(surface.heap_bytes(), exact);
        assert_eq!(surface.runs.len(), 2);
        assert_eq!(surface.name_runs.len(), 1);
    }
}
