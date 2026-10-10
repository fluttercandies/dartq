//! The `impact` query (PRD §15.4): what a change to one library does to the
//! workspace — the changed symbols, the reverse-dependency closure with its
//! content-only vs semantic tags, the tests exercising the affected code,
//! and the environment matrix (a single context until §16 lands).
//!
//! # The query shape
//!
//! The query is a pure function over inputs the caller owns:
//!
//! * the **import graph** as a per-library adjacency closure (the engine's
//!   CSR projection, a resolution graph, or a fixture — the query never
//!   sees the storage),
//! * the **test graph** ([`TestGraph`], built by dartq-index),
//! * the **change set**: per changed library, the before/after
//!   [`FileSnapshot`] pair.
//!
//! Library identity is the caller's `u32` id space throughout; internal ids
//! never leave the engine boundary (PRD §15.7) — every report row carries a
//! stable `label|name` path string instead.
//!
//! # Closure mechanics
//!
//! The rust-analyzer reverse-dependency posture (`base-db/src/input.rs`
//! `transitive_rev_deps`): the import graph is inverted once per query —
//! a flat CSR array pair, no per-node heap rows — and a worklist drains
//! the importers. The visited and direct-semantic marks
//! are dense `Vec<bool>`s over the id space — O(1) probes, no hashing
//! (library ids are a dense arena space, so a hashed set would only add
//! overhead). Output is deterministic: every section sorts by its path.
//!
//! # The signature seam (#282)
//!
//! [`FileSnapshot`] carries what the v1 proxy can see — the content hash
//! ([`content_hash`], blake3) and the library-face name set. The two
//! verdict functions ([`api_changed`] and [`symbol_signature_changed`]) are
//! the seam the real `SignatureHash` gate replaces (PRD §11.3,
//! `FileElements::api_signature`): the report shape, the tag rule, and the
//! call sites stay when the gate lands.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use dartq_index::TestGraph;
use dartq_source::{StringId, StringTable};

/// The report contract version (PRD §15.7): breaking output changes bump
/// the major.
pub const IMPACT_SCHEMA_VERSION: u32 = 1;

/// The content hash of one file's bytes — the identity [`FileSnapshot`]
/// compares. blake3: collision-resistant, fast, already the workspace's
/// content-id hash family.
#[must_use]
pub fn content_hash(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}

/// What the v1 proxy sees of one library's state: the content identity and
/// the library-face (top-level) declared names.
///
/// The name set is the sorted, deduplicated [`StringId`]s of the library's
/// face rows (a multi-file library folds its parts before querying). The
/// set lives sorted so the diff and the verdicts are merge walks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSnapshot {
    /// The content identity ([`content_hash`] over the library's bytes).
    pub content_hash: [u8; 32],
    /// The library-face names, ascending, deduplicated — asserted by
    /// [`impact`] in every build (the verdicts are merge walks over it).
    pub top_level_names: Box<[StringId]>,
}

/// One changed library: the before/after surface pair the verdicts compare.
/// Duplicate entries for one library resolve last-wins (see [`impact`]).
#[derive(Debug, Clone)]
pub struct LibraryChange {
    /// The changed library, in the query's id space.
    pub library: u32,
    /// The surface before the change.
    pub before: FileSnapshot,
    /// The surface after the change.
    pub after: FileSnapshot,
}

impl LibraryChange {
    /// The one-shot posture: no baseline exists, so the query assesses what
    /// an API-level edit to `current` would affect — every current face
    /// name counts as changed and the API verdict is `true` (direct
    /// importers tag semantic). The daemon path passes real diffs instead.
    #[must_use]
    pub fn hypothetical(library: u32, current: FileSnapshot) -> Self {
        Self {
            library,
            before: FileSnapshot {
                content_hash: [0; 32],
                top_level_names: Box::from([]),
            },
            after: current,
        }
    }
}

/// The impact report (PRD §15.4), JSON contract `schema_version: 1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImpactReport {
    /// The output contract version ([`IMPACT_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// The changed symbols of the target libraries.
    pub changed: Vec<ChangedSymbol>,
    /// The dependency closure, each entry content-only or semantic.
    pub affected: Vec<AffectedTarget>,
    /// The test files exercising the changed or affected libraries.
    pub affected_tests: Vec<AffectedTest>,
    /// The environment matrix; a single always-compatible context until
    /// §16's multi-context lands (the contracted v1 shape).
    pub affected_environments: Vec<EnvironmentImpact>,
}

/// One changed symbol with its signature verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedSymbol {
    /// The stable path reference (`label|name`, PRD §15.7).
    pub symbol: String,
    /// Whether the symbol's signature changed (the v1 proxy verdict — see
    /// [`symbol_signature_changed`]).
    pub signature_changed: bool,
}

/// One library of the dependency closure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AffectedTarget {
    /// The stable path reference (the library's label).
    pub path: String,
    /// How the change reaches this library.
    pub impact: ImpactLevel,
}

/// How a change reaches an affected library (PRD §15.4's
/// "仅内容受影响 vs 语义受影响").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImpactLevel {
    /// The library's text is only re-processed; no name it reads moved.
    ContentOnly,
    /// A directly imported library's API surface moved; the library's
    /// resolution must rerun.
    Semantic,
}

/// One test file of the affected-tests section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AffectedTest {
    /// The stable path reference (the test library's label).
    pub path: String,
}

/// One environment of the matrix (§16); v1 carries the single default
/// context, compatible by construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentImpact {
    /// The context label.
    pub label: String,
    /// Whether the affected set analyzes clean in the environment.
    pub compatible: bool,
}

/// The file-level API-change verdict — the aggregate the semantic tag keys
/// on. The v1 proxy: the content moved **and** the top-level name set moved
/// with it. #282 replaces the name-set compare with the `api_signature`
/// compare; a comment- or body-only edit (hash moves, set stable) stays
/// content-only.
#[must_use]
pub fn api_changed(before: &FileSnapshot, after: &FileSnapshot) -> bool {
    before.content_hash != after.content_hash && before.top_level_names != after.top_level_names
}

/// The per-symbol signature verdict — the #282 seam's per-name half. The v1
/// proxy keys on the name's presence: an added or removed declaration is a
/// signature change; a body edit under a stable name is invisible until the
/// `SignatureHash` gate lands.
///
/// `before` and `after` must be ascending and deduplicated (the
/// [`FileSnapshot`] contract) — `binary_search` on unsorted input answers
/// arbitrarily instead of panicking.
#[must_use]
pub fn symbol_signature_changed(before: &[StringId], after: &[StringId], name: StringId) -> bool {
    debug_assert!(
        is_ascending(before) && is_ascending(after),
        "name sets must be ascending and deduplicated"
    );
    before.binary_search(&name).is_ok() != after.binary_search(&name).is_ok()
}

/// The snapshot name-set precondition the verdicts stand on: strictly
/// ascending, which also rules out duplicates.
fn is_ascending(names: &[StringId]) -> bool {
    names.windows(2).all(|pair| pair[0] < pair[1])
}

/// The names present in exactly one of the two sorted, deduplicated sets —
/// the Changed section's rows, ascending.
fn diff_names(before: &[StringId], after: &[StringId]) -> Vec<StringId> {
    let mut diff = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < before.len() && j < after.len() {
        match before[i].cmp(&after[j]) {
            Ordering::Less => {
                diff.push(before[i]);
                i += 1;
            }
            Ordering::Greater => {
                diff.push(after[j]);
                j += 1;
            }
            Ordering::Equal => {
                i += 1;
                j += 1;
            }
        }
    }
    diff.extend_from_slice(&before[i..]);
    diff.extend_from_slice(&after[j..]);
    diff
}

/// Answers the impact query over `changes` (PRD §15.4).
///
/// `imports_of(lib)` yields the resolved import targets of `lib` (the
/// caller's id space, `0..library_count`); `label_of(lib)` yields the
/// library's stable path text. Both borrow caller state through `'a`.
/// Duplicate entries for one library resolve last-wins: the last
/// [`LibraryChange`] for a library is authoritative, earlier ones
/// contribute nothing.
///
/// # Panics
///
/// Panics when the library count exceeds [`u32::MAX`] — the tables are
/// sized by input, so a four-billion-library workspace is a program bug to
/// surface, not truncate (the dartq-index "surface, never clamp" contract
/// this query shares).
///
/// Panics on out-of-contract caller data too: a [`LibraryChange`] id or an
/// `imports_of` target at or beyond `library_count` stops on the array
/// bounds check (debug builds assert first, naming the offending id), and
/// a snapshot whose `top_level_names` are not ascending and deduplicated
/// is rejected in every build — the merge walks behind the verdicts would
/// silently mis-diff it otherwise.
#[must_use]
pub fn impact<'a>(
    library_count: usize,
    imports_of: impl Fn(u32) -> &'a [u32],
    label_of: impl Fn(u32) -> &'a str,
    tests: &TestGraph,
    strings: &StringTable,
    changes: &[LibraryChange],
) -> ImpactReport {
    // The verdicts are merge walks over the snapshot name sets: an
    // unsorted or duplicated set would silently mis-diff (arbitrary
    // binary_search answers, dropped merge entries), so the caller
    // contract is asserted in every build — a rejected input beats a
    // wrong verdict.
    for change in changes {
        assert!(
            is_ascending(&change.before.top_level_names)
                && is_ascending(&change.after.top_level_names),
            "FileSnapshot.top_level_names must be ascending and deduplicated"
        );
    }

    // The inverted graph, built once (the rust-analyzer posture) as flat
    // CSR — two counting passes over the id space, one allocation per
    // array; the ascending fill keeps each row's importers ascending.
    // Edge `importer → imported` inverts to `importers_of(imported)`.
    let count = u32::try_from(library_count).expect("more than u32::MAX libraries");
    let mut offsets = vec![0u32; library_count + 1];
    for lib in 0..count {
        for &target in imports_of(lib) {
            debug_assert!(
                target < count,
                "import target {target} out of range 0..{count}"
            );
            offsets[target as usize + 1] += 1;
        }
    }
    for i in 1..offsets.len() {
        offsets[i] += offsets[i - 1];
    }
    let mut edges = vec![0u32; offsets[library_count] as usize];
    let mut fill: Vec<u32> = offsets[..library_count].to_vec();
    for lib in 0..count {
        for &target in imports_of(lib) {
            let slot = fill[target as usize] as usize;
            edges[slot] = lib;
            fill[target as usize] += 1;
        }
    }
    let importers_of =
        |lib: usize| -> &[u32] { &edges[offsets[lib] as usize..offsets[lib + 1] as usize] };

    // Seed marks, then the direct-semantic marks: the first hop out of an
    // API-changing library. Transitively reached importers stay
    // content-only — the v1 proxy sees only the direct edge (the L2 gate
    // refines the propagation).
    let mut seed_changed = vec![false; library_count];
    let mut api = vec![false; library_count];
    for change in changes {
        let lib = change.library as usize;
        debug_assert!(
            lib < library_count,
            "LibraryChange.library {} out of range 0..{library_count}",
            change.library
        );
        seed_changed[lib] = true;
        api[lib] = api_changed(&change.before, &change.after);
    }
    let mut direct_semantic = vec![false; library_count];
    for (lib, &is_api) in api.iter().enumerate() {
        if is_api {
            for &importer in importers_of(lib) {
                direct_semantic[importer as usize] = true;
            }
        }
    }

    // The closure: seeds pre-marked, importers drained worklist-style.
    let mut visited = seed_changed.clone();
    let mut worklist: Vec<u32> = (0..count)
        .filter(|&lib| seed_changed[lib as usize])
        .collect();
    while let Some(lib) = worklist.pop() {
        for &importer in importers_of(lib as usize) {
            if visited[importer as usize] {
                continue;
            }
            visited[importer as usize] = true;
            worklist.push(importer);
        }
    }

    // Changed rows: the name-set diff per change, each row's verdict from
    // the seam. Honest no-op changes (equal hashes, equal sets) yield no
    // rows and a content-only closure. Duplicate entries for one library
    // resolve last-wins — the api marks already overwrite that way — so
    // the superseded entries ship no rows and contradictory symbol rows
    // are impossible by construction.
    let mut superseded = vec![false; changes.len()];
    let mut seen = vec![false; library_count];
    for (i, change) in changes.iter().enumerate().rev() {
        if seen[change.library as usize] {
            superseded[i] = true;
        } else {
            seen[change.library as usize] = true;
        }
    }
    let mut changed_rows: Vec<ChangedSymbol> = Vec::new();
    for (i, change) in changes.iter().enumerate() {
        if superseded[i] {
            continue;
        }
        let before = &change.before.top_level_names;
        let after = &change.after.top_level_names;
        for name in diff_names(before, after) {
            changed_rows.push(ChangedSymbol {
                symbol: format!("{}|{}", label_of(change.library), strings.resolve(name)),
                signature_changed: symbol_signature_changed(before, after, name),
            });
        }
    }
    changed_rows.sort_by(|a, b| a.symbol.cmp(&b.symbol));

    let mut affected_rows: Vec<AffectedTarget> = (0..count)
        .filter(|&lib| visited[lib as usize] && !seed_changed[lib as usize])
        .map(|lib| AffectedTarget {
            path: label_of(lib).to_string(),
            impact: if direct_semantic[lib as usize] {
                ImpactLevel::Semantic
            } else {
                ImpactLevel::ContentOnly
            },
        })
        .collect();
    affected_rows.sort_by(|a, b| a.path.cmp(&b.path));

    // Affected tests: the test-graph join over the changed and affected
    // libraries, deduplicated, path-sorted.
    let mut test_rows: Vec<AffectedTest> = (0..count)
        .filter(|&lib| visited[lib as usize])
        .flat_map(|lib| tests.tests_of(lib))
        .map(|&test| AffectedTest {
            path: label_of(test).to_string(),
        })
        .collect();
    test_rows.sort_by(|a, b| a.path.cmp(&b.path));
    test_rows.dedup_by(|a, b| a.path == b.path);

    ImpactReport {
        schema_version: IMPACT_SCHEMA_VERSION,
        changed: changed_rows,
        affected: affected_rows,
        affected_tests: test_rows,
        affected_environments: vec![EnvironmentImpact {
            label: "default".to_string(),
            compatible: true,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three libraries in a chain plus the test file: `a.dart` (declares
    /// `fa`), `b.dart` imports `a`, `c.dart` imports `b`, and `a_test.dart`
    /// imports `a`. Ids: a=0, b=1, c=2, t=3.
    struct Fixture {
        strings: StringTable,
        fa: StringId,
        fb: StringId,
    }

    fn fixture() -> Fixture {
        let strings = StringTable::new();
        let fa = strings.intern("fa");
        let fb = strings.intern("fb");
        Fixture { strings, fa, fb }
    }

    fn snapshot(strings: &StringTable, names: &[&str]) -> FileSnapshot {
        let mut top_level_names: Vec<StringId> =
            names.iter().map(|name| strings.intern(name)).collect();
        top_level_names.sort_unstable();
        top_level_names.dedup();
        FileSnapshot {
            content_hash: content_hash(names.join(",").as_bytes()),
            top_level_names: top_level_names.into_boxed_slice(),
        }
    }

    fn imports_of(_: u32) -> &'static [u32] {
        &[]
    }

    /// Acceptance core: an API-level change in `a` (a name added) marks the
    /// direct importer `b` semantic, the transitive `c` content-only, and
    /// lists `a_test` in AffectedTests.
    #[test]
    fn api_change_tags_direct_semantic_and_transitive_content_only() {
        let fx = fixture();
        // a=0, b=1 imports a, c=2 imports b, t=3 imports a.
        let imports: [&[u32]; 4] = [&[], &[0], &[1], &[0]];
        let tests = TestGraph::build(
            4,
            |lib| imports[lib as usize],
            |lib| Some(["a.dart", "b.dart", "c.dart", "a_test.dart"][lib as usize]),
        );
        let changes = [LibraryChange {
            library: 0,
            before: snapshot(&fx.strings, &["fa"]),
            after: snapshot(&fx.strings, &["fa", "fb"]),
        }];
        let report = impact(
            4,
            |lib| imports[lib as usize],
            |lib| ["a.dart", "b.dart", "c.dart", "a_test.dart"][lib as usize],
            &tests,
            &fx.strings,
            &changes,
        );
        assert_eq!(report.schema_version, IMPACT_SCHEMA_VERSION);
        assert_eq!(
            report
                .changed
                .iter()
                .map(|row| (row.symbol.as_str(), row.signature_changed))
                .collect::<Vec<_>>(),
            vec![("a.dart|fb", true)],
            "the added name is the changed symbol"
        );
        assert_eq!(
            report
                .affected
                .iter()
                .map(|row| (row.path.as_str(), row.impact))
                .collect::<Vec<_>>(),
            vec![
                ("a_test.dart", ImpactLevel::Semantic),
                ("b.dart", ImpactLevel::Semantic),
                ("c.dart", ImpactLevel::ContentOnly),
            ],
            "direct importers semantic, the transitive hop content-only"
        );
        assert_eq!(
            report
                .affected_tests
                .iter()
                .map(|row| row.path.as_str())
                .collect::<Vec<_>>(),
            vec!["a_test.dart"]
        );
        assert_eq!(
            report.affected_environments,
            vec![EnvironmentImpact {
                label: "default".to_string(),
                compatible: true,
            }]
        );
    }

    /// A body-only edit (hash moves, name set stable) yields no Changed
    /// rows and an all-content-only closure — the proxy's documented
    /// under-report until #282.
    #[test]
    fn body_only_edit_stays_content_only() {
        let fx = fixture();
        let imports: [&[u32]; 2] = [&[], &[0]];
        let tests = TestGraph::build(
            2,
            |lib| imports[lib as usize],
            |lib| Some(["a.dart", "b.dart"][lib as usize]),
        );
        let mut after = snapshot(&fx.strings, &["fa"]);
        after.content_hash = content_hash(b"reformatted");
        let changes = [LibraryChange {
            library: 0,
            before: snapshot(&fx.strings, &["fa"]),
            after,
        }];
        let report = impact(
            2,
            |lib| imports[lib as usize],
            |lib| ["a.dart", "b.dart"][lib as usize],
            &tests,
            &fx.strings,
            &changes,
        );
        assert!(report.changed.is_empty(), "no name moved, no rows");
        assert_eq!(report.affected.len(), 1);
        assert_eq!(report.affected[0].impact, ImpactLevel::ContentOnly);
        assert!(report.affected_tests.is_empty(), "no test graph here");
    }

    /// A removed declaration is detected and flagged: the name-set diff
    /// runs both directions.
    #[test]
    fn removed_declaration_is_detected() {
        let fx = fixture();
        let imports: [&[u32]; 1] = [&[]];
        let tests = TestGraph::default();
        let changes = [LibraryChange {
            library: 0,
            before: snapshot(&fx.strings, &["fa", "fb"]),
            after: snapshot(&fx.strings, &["fa"]),
        }];
        let report = impact(
            1,
            |lib| imports[lib as usize],
            |lib| ["a.dart"][lib as usize],
            &tests,
            &fx.strings,
            &changes,
        );
        assert_eq!(
            report
                .changed
                .iter()
                .map(|row| (row.symbol.as_str(), row.signature_changed))
                .collect::<Vec<_>>(),
            vec![("a.dart|fb", true)]
        );
    }

    /// Multiple changed libraries: a diamond importer tags semantic when
    /// either source changed its API, and one honest no-op contributes
    /// nothing.
    #[test]
    fn diamond_importer_takes_the_strongest_tag() {
        let fx = fixture();
        // a1=0 (api change), a2=1 (body-only change), d=2 imports both.
        let imports: [&[u32]; 3] = [&[], &[], &[0, 1]];
        let tests = TestGraph::default();
        let changes = [
            LibraryChange {
                library: 0,
                before: snapshot(&fx.strings, &["fa"]),
                after: snapshot(&fx.strings, &["fa", "fb"]),
            },
            LibraryChange {
                library: 1,
                before: snapshot(&fx.strings, &["ga"]),
                after: {
                    let mut same = snapshot(&fx.strings, &["ga"]);
                    same.content_hash = content_hash(b"comment");
                    same
                },
            },
        ];
        let report = impact(
            3,
            |lib| imports[lib as usize],
            |lib| ["a1.dart", "a2.dart", "d.dart"][lib as usize],
            &tests,
            &fx.strings,
            &changes,
        );
        assert_eq!(
            report
                .changed
                .iter()
                .map(|row| row.symbol.as_str())
                .collect::<Vec<_>>(),
            vec!["a1.dart|fb"],
            "the body-only change contributes no rows"
        );
        assert_eq!(report.affected.len(), 1);
        assert_eq!(report.affected[0].path, "d.dart");
        assert_eq!(
            report.affected[0].impact,
            ImpactLevel::Semantic,
            "one API-changing source is enough"
        );
    }

    /// The one-shot posture: a hypothetical change lists every current face
    /// name as changed and tags direct importers semantic.
    #[test]
    fn hypothetical_change_assumes_full_api_surface() {
        let fx = fixture();
        let imports: [&[u32]; 2] = [&[], &[0]];
        let tests = TestGraph::default();
        let change = LibraryChange::hypothetical(0, snapshot(&fx.strings, &["fa", "fb"]));
        let report = impact(
            2,
            |lib| imports[lib as usize],
            |lib| ["a.dart", "b.dart"][lib as usize],
            &tests,
            &fx.strings,
            &[change],
        );
        assert_eq!(
            report
                .changed
                .iter()
                .map(|row| row.symbol.as_str())
                .collect::<Vec<_>>(),
            vec!["a.dart|fa", "a.dart|fb"],
            "every face name counts as changed"
        );
        assert!(
            report.changed.iter().all(|row| row.signature_changed),
            "the hypothetical is an API-level edit"
        );
        assert_eq!(report.affected[0].impact, ImpactLevel::Semantic);
    }

    /// The JSON contract: `schema_version: 1` round-trips, section keys are
    /// the §15.4 names, and the impact tag spells `content-only` /
    /// `semantic`.
    #[test]
    fn json_round_trip_with_schema_version() {
        let fx = fixture();
        let imports: [&[u32]; 2] = [&[], &[0]];
        let tests = TestGraph::default();
        let report = impact(
            2,
            |lib| imports[lib as usize],
            |lib| ["a.dart", "b.dart"][lib as usize],
            &tests,
            &fx.strings,
            &[LibraryChange {
                library: 0,
                before: snapshot(&fx.strings, &[]),
                after: snapshot(&fx.strings, &["fa"]),
            }],
        );
        let text = serde_json::to_string(&report).expect("serialize");
        let back: ImpactReport = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(back, report, "byte-stable round trip");
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(value["schema_version"], 1);
        assert!(value["changed"].is_array());
        assert!(value["affected"].is_array());
        assert!(value["affected_tests"].is_array());
        assert!(value["affected_environments"].is_array());
        assert_eq!(
            value["affected"][0]["impact"].as_str(),
            Some("semantic"),
            "kebab-case tag spelling"
        );
    }

    /// A no-op change (identical snapshots) answers the empty report with
    /// the environment row still present — the section shape never
    /// disappears.
    #[test]
    fn no_op_change_answers_the_empty_report() {
        let fx = fixture();
        let tests = TestGraph::default();
        let same = snapshot(&fx.strings, &["fa"]);
        let report = impact(
            1,
            imports_of,
            |_| "a.dart",
            &tests,
            &fx.strings,
            &[LibraryChange {
                library: 0,
                before: same.clone(),
                after: same,
            }],
        );
        assert!(report.changed.is_empty());
        assert!(report.affected.is_empty());
        assert!(report.affected_tests.is_empty());
        assert_eq!(report.affected_environments.len(), 1);
    }

    /// An out-of-range [`LibraryChange`] id is a caller bug that panics
    /// with the offending id in the message (debug assert; release stops
    /// on the array bounds check) — surfaced, never clamped.
    #[test]
    #[should_panic(expected = "out of range")]
    fn out_of_range_change_library_panics() {
        let fx = fixture();
        let tests = TestGraph::default();
        let change = LibraryChange {
            library: 9,
            before: snapshot(&fx.strings, &["fa"]),
            after: snapshot(&fx.strings, &["fa", "fb"]),
        };
        let _ = impact(1, imports_of, |_| "a.dart", &tests, &fx.strings, &[change]);
    }

    /// An `imports_of` target beyond the id space is a caller bug that
    /// panics with the offending target (debug assert; release stops on
    /// the array bounds check) — the inverted CSR never drops or wraps an
    /// edge.
    #[test]
    #[should_panic(expected = "out of range")]
    fn out_of_range_import_target_panics() {
        let fx = fixture();
        let imports: [&[u32]; 2] = [&[], &[7]];
        let tests = TestGraph::default();
        let _ = impact(
            2,
            |lib| imports[lib as usize],
            |lib| ["a.dart", "b.dart"][lib as usize],
            &tests,
            &fx.strings,
            &[],
        );
    }

    /// An unsorted snapshot is rejected with a panic in every build — the
    /// merge walks behind the verdicts would silently mis-diff it
    /// otherwise, and a wrong verdict is the one failure this query must
    /// never ship.
    #[test]
    #[should_panic(expected = "ascending")]
    fn unsorted_snapshot_is_rejected_not_misdiffed() {
        let fx = fixture();
        let mut after = snapshot(&fx.strings, &["fa", "fb"]);
        after.top_level_names = vec![fx.fb, fx.fa].into_boxed_slice();
        let tests = TestGraph::default();
        let change = LibraryChange {
            library: 0,
            before: snapshot(&fx.strings, &["fa"]),
            after,
        };
        let _ = impact(1, imports_of, |_| "a.dart", &tests, &fx.strings, &[change]);
    }

    /// The other face of the sortedness contract: a contract-valid input
    /// still answers byte-identically — the defenses add nothing to the
    /// output.
    #[test]
    fn sorted_snapshot_answers_byte_identically() {
        let fx = fixture();
        let imports: [&[u32]; 2] = [&[], &[0]];
        let tests = TestGraph::default();
        let report = impact(
            2,
            |lib| imports[lib as usize],
            |lib| ["a.dart", "b.dart"][lib as usize],
            &tests,
            &fx.strings,
            &[LibraryChange {
                library: 0,
                before: snapshot(&fx.strings, &["fa"]),
                after: snapshot(&fx.strings, &["fa", "fb"]),
            }],
        );
        assert_eq!(
            report,
            ImpactReport {
                schema_version: IMPACT_SCHEMA_VERSION,
                changed: vec![ChangedSymbol {
                    symbol: "a.dart|fb".to_string(),
                    signature_changed: true,
                }],
                affected: vec![AffectedTarget {
                    path: "b.dart".to_string(),
                    impact: ImpactLevel::Semantic,
                }],
                affected_tests: vec![],
                affected_environments: vec![EnvironmentImpact {
                    label: "default".to_string(),
                    compatible: true,
                }],
            },
        );
    }

    /// Two changes for one library resolve last-wins: the superseded
    /// earlier entry ships no rows and its api verdict is discarded, so
    /// contradictory symbol rows are impossible by construction.
    #[test]
    fn duplicate_change_last_entry_wins() {
        let fx = fixture();
        let imports: [&[u32]; 2] = [&[], &[0]];
        let tests = TestGraph::default();
        let adds_fb = LibraryChange {
            library: 0,
            before: snapshot(&fx.strings, &["fa"]),
            after: snapshot(&fx.strings, &["fa", "fb"]),
        };
        let mut body_only = LibraryChange {
            library: 0,
            before: snapshot(&fx.strings, &["fa"]),
            after: snapshot(&fx.strings, &["fa"]),
        };
        body_only.after.content_hash = content_hash(b"reformatted");
        let report = impact(
            2,
            |lib| imports[lib as usize],
            |lib| ["a.dart", "b.dart"][lib as usize],
            &tests,
            &fx.strings,
            &[adds_fb, body_only],
        );
        assert!(
            report.changed.is_empty(),
            "the superseded entry ships no rows"
        );
        assert_eq!(
            report
                .affected
                .iter()
                .map(|row| (row.path.as_str(), row.impact))
                .collect::<Vec<_>>(),
            vec![("b.dart", ImpactLevel::ContentOnly)],
            "the last entry's content-only verdict rules"
        );
    }
}
