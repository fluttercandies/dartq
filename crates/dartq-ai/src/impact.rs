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
//! [`RevImportGraph`], the shared flat-CSR build the amortized batch
//! queries and the T8 explain stage reuse — and worklists drain
//! the importers. The visited and semantic marks are dense `Vec<bool>`s
//! over the id space — O(1) probes, no hashing (library ids are a dense
//! arena space, so a hashed set would only add overhead). Output is
//! deterministic: every section sorts by its path.
//!
//! # The signature gate (#282/#316/#338)
//!
//! [`FileSnapshot`] carries the content hash ([`content_hash`], blake3),
//! the API signature digest (the engine's `ApiSignature` as raw bytes —
//! the element face folded with the directive face, PRD §11.3), and the
//! library-face name set. The file-level verdict ([`api_changed`]) is the
//! signature inequality alone: the digest already folds everything a
//! downstream library resolves against, so a body-only edit (hash moves,
//! signature stable) stays content-only without a name-set conjunct. The
//! per-symbol verdict ([`symbol_signature_changed`]) remains a name-presence
//! proxy until member-level signature evidence lands (documented at its
//! definition).

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use dartq_index::TestGraph;
use dartq_source::{StringId, StringTable};

use crate::rev_import_graph::RevImportGraph;

/// The report contract version (PRD §15.7): breaking output changes bump
/// the major.
pub const IMPACT_SCHEMA_VERSION: u32 = 1;

/// The content hash of one file's bytes — the identity [`FileSnapshot`]
/// carries alongside the API signature. blake3: collision-resistant, fast,
/// already the workspace's content-id hash family.
#[must_use]
pub fn content_hash(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}

/// The library-level API signature: blake3-128 over the member digests in
/// member order (defining file first, then parts in part-directive order —
/// the order the library's content identity folds). Fixed-width entries need
/// no framing; the digest is order-sensitive, so a part reorder moves it,
/// exactly as it moves the library's resolution surface.
#[must_use]
pub fn fold_api_signatures(members: &[[u8; 16]]) -> [u8; 16] {
    let mut bytes = Vec::with_capacity(members.len() * 16);
    for member in members {
        bytes.extend_from_slice(member);
    }
    let digest = blake3::hash(&bytes);
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest.as_bytes()[..16]);
    id
}

/// What the query sees of one library's state: the content identity, the
/// API signature digest, and the library-face (top-level) declared names.
///
/// The name set is the sorted, deduplicated [`StringId`]s of the library's
/// face rows (a multi-file library folds its parts before querying). The
/// set lives sorted so the diff and the verdicts are merge walks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSnapshot {
    /// The content identity ([`content_hash`] over the library's bytes).
    pub content_hash: [u8; 32],
    /// The API signature digest (#316/#338): the engine's `ApiSignature` as
    /// raw bytes (`as_bytes()` — deliberately not the type, so this query
    /// carries no dartq-semantic dependency edge), folded across the
    /// library's members by [`fold_api_signatures`]. Folds the element face
    /// with the directive face; `api_changed` verdicts on it alone.
    pub api_signature: [u8; 16],
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
    /// name counts as changed and the API verdict is `true` (importers at
    /// every hop tag semantic). The daemon path passes real diffs instead.
    /// The before side carries all-zero digests — no-baseline sentinels
    /// (a real blake3 digest never answers all-zero), so the signature
    /// verdict and the name diff both fire by construction.
    #[must_use]
    pub fn hypothetical(library: u32, current: FileSnapshot) -> Self {
        Self {
            library,
            before: FileSnapshot {
                content_hash: [0; 32],
                api_signature: [0; 16],
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
    /// Whether the symbol's signature changed. Member-level proxy (see
    /// [`symbol_signature_changed`]): it keys on the name's presence in the
    /// face set, not on a per-symbol signature digest — member-level
    /// signature evidence is deferred (#338 posture), and this flag stays
    /// the proxy until that evidence lands.
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
/// on: the API signature moved (#316/#338). The signature comparison stands
/// alone, deliberately without a content-hash conjunct: the digest already
/// folds the element face with the directive face, so the conjunct would
/// only cause false negatives — a signature can move while the content hash
/// holds (a signature reframe across an engine upgrade: every byte
/// identical, the baked digest reframed) and must still verdict as an API
/// change. A body-only edit needs no conjunct to stay content-only: the
/// signature is stable under it by construction.
#[must_use]
pub fn api_changed(before: &FileSnapshot, after: &FileSnapshot) -> bool {
    before.api_signature != after.api_signature
}

/// The per-symbol signature verdict — the member-level half of the gate.
/// Proxy until member-level signature evidence lands (#338 defers it): it
/// keys on the name's presence, so an added or removed declaration is a
/// signature change while a body edit under a stable name is invisible —
/// and so is a signature-bearing edit (default value, type refinement)
/// under a stable name. The file-level [`api_changed`] verdict carries the
/// real signal until per-symbol digests exist.
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

/// The transitive semantic marks (L3): `true` for every library reachable
/// across the reverse import graph from an API-changing seed — importers at
/// every hop resolve against the moved face, so the tag floods past the
/// direct edge.
fn semantic_flood<'a>(
    count: u32,
    api: &[bool],
    importers_of: impl Fn(usize) -> &'a [u32],
) -> Vec<bool> {
    let mut semantic = vec![false; api.len()];
    let mut flood: Vec<u32> = (0..count).filter(|&lib| api[lib as usize]).collect();
    while let Some(lib) = flood.pop() {
        for &importer in importers_of(lib as usize) {
            if semantic[importer as usize] {
                continue;
            }
            semantic[importer as usize] = true;
            flood.push(importer);
        }
    }
    semantic
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
/// Panics when the library count exceeds [`u32::MAX`] — the inverted graph
/// ([`RevImportGraph::build`]) and the mark tables are sized by input, so
/// a four-billion-library workspace is a program bug to surface, not
/// truncate (the dartq-index "surface, never clamp" contract this query
/// shares).
///
/// Panics on out-of-contract caller data too: a [`LibraryChange`] id or an
/// `imports_of` target at or beyond `library_count` stops on the array
/// bounds check (debug builds assert first, naming the offending id — the
/// import target in [`RevImportGraph::build`]), and a snapshot whose
/// `top_level_names` are not ascending and deduplicated is rejected in
/// every build — the merge walks behind the verdicts would silently
/// mis-diff it otherwise.
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

    // The inverted graph, built once (the rust-analyzer posture) via the
    // shared CSR build — the amortized batch queries and the T8 explain
    // stage reuse the same one. Edge `importer → imported` inverts to
    // `importers_of(imported)`.
    let count = u32::try_from(library_count).expect("more than u32::MAX libraries");
    let graph = RevImportGraph::build(library_count, imports_of);
    let importers_of = |lib: usize| graph.importers_of(lib);

    // Seed marks, then the semantic propagation (L3, #338): a signature
    // change floods the reverse import graph, so importers at every hop
    // downstream of an API-changing library carry the Semantic tag — each
    // of them resolves against the moved face. Libraries reached only
    // through body-only changes stay content-only.
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
    let semantic = semantic_flood(count, &api, importers_of);

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
    // the seam. Honest no-op changes (equal snapshots) yield no rows and a
    // content-only closure. Duplicate entries for one library
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
            impact: if semantic[lib as usize] {
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
        // The signature rides the face the way the real producer's does: a
        // digest of the face itself — stable under body edits (the tests
        // move `content_hash` alone), moving when the name set moves.
        let face = names.join(",");
        let digest = blake3::hash(face.as_bytes());
        let mut api_signature = [0u8; 16];
        api_signature.copy_from_slice(&digest.as_bytes()[..16]);
        FileSnapshot {
            content_hash: content_hash(face.as_bytes()),
            api_signature,
            top_level_names: top_level_names.into_boxed_slice(),
        }
    }

    fn imports_of(_: u32) -> &'static [u32] {
        &[]
    }

    /// Acceptance core: an API-level change in `a` floods the reverse
    /// import graph — the direct importer `b`, the transitive `c`, and the
    /// direct test import `a_test` all carry the Semantic tag (L3), and
    /// `a_test` lists in AffectedTests.
    #[test]
    fn api_change_propagates_semantic_to_every_hop() {
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
                ("c.dart", ImpactLevel::Semantic),
            ],
            "the signature flood reaches every downstream hop"
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

    /// Cycle termination: `a` imports `b`, `b` imports `a` — both floods
    /// (the L3 semantic flood and the visited closure) mark before they
    /// push, so each node is enqueued at most once and the report returns:
    /// the seed ships its Changed row, the cycle partner is reached exactly
    /// once and tagged Semantic.
    #[test]
    fn two_cycle_import_graph_terminates_and_tags_both_sides() {
        let fx = fixture();
        // a=0 imports b, b=1 imports a — the two-cycle.
        let imports: [&[u32]; 2] = [&[1], &[0]];
        let tests = TestGraph::default();
        let changes = [LibraryChange {
            library: 0,
            before: snapshot(&fx.strings, &["fa"]),
            after: snapshot(&fx.strings, &["fa", "fb"]),
        }];
        let report = impact(
            2,
            |lib| imports[lib as usize],
            |lib| ["a.dart", "b.dart"][lib as usize],
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
            vec!["a.dart|fb"],
            "the seed's change ships its row"
        );
        assert_eq!(
            report
                .affected
                .iter()
                .map(|row| (row.path.as_str(), row.impact))
                .collect::<Vec<_>>(),
            vec![("b.dart", ImpactLevel::Semantic)],
            "the cycle partner is visited once and tagged semantic"
        );
    }

    /// Self-import termination: `a` importing itself is the one-node cycle
    /// — the seed is marked before its self-edge can re-enqueue it, so the
    /// report returns with the seed alone: its Changed row, no Affected row
    /// (seeds never list as affected), no tests.
    #[test]
    fn self_import_cycle_terminates_on_the_seed_alone() {
        let fx = fixture();
        let imports: [&[u32]; 1] = [&[0]];
        let tests = TestGraph::default();
        let changes = [LibraryChange {
            library: 0,
            before: snapshot(&fx.strings, &["fa"]),
            after: snapshot(&fx.strings, &["fa", "fb"]),
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
                .map(|row| row.symbol.as_str())
                .collect::<Vec<_>>(),
            vec!["a.dart|fb"]
        );
        assert!(
            report.affected.is_empty(),
            "the seed never lists as affected, self-edge or not"
        );
        assert!(report.affected_tests.is_empty());
    }

    /// The flood boundary: a body-only change (hash moves, signature
    /// stable) visits its importers but leaves every one of them
    /// content-only — at every hop, not just the direct edge.
    #[test]
    fn body_only_change_stays_content_only_at_every_hop() {
        let fx = fixture();
        // a=0 body-only, b=1 imports a, c=2 imports b.
        let imports: [&[u32]; 3] = [&[], &[0], &[1]];
        let tests = TestGraph::default();
        let mut after = snapshot(&fx.strings, &["fa"]);
        after.content_hash = content_hash(b"reformatted");
        let changes = [LibraryChange {
            library: 0,
            before: snapshot(&fx.strings, &["fa"]),
            after,
        }];
        let report = impact(
            3,
            |lib| imports[lib as usize],
            |lib| ["a.dart", "b.dart", "c.dart"][lib as usize],
            &tests,
            &fx.strings,
            &changes,
        );
        assert!(report.changed.is_empty(), "no name moved, no rows");
        assert_eq!(
            report
                .affected
                .iter()
                .map(|row| (row.path.as_str(), row.impact))
                .collect::<Vec<_>>(),
            vec![
                ("b.dart", ImpactLevel::ContentOnly),
                ("c.dart", ImpactLevel::ContentOnly),
            ],
            "visited, but the signature never moved"
        );
        assert!(report.affected_tests.is_empty(), "no test graph here");
    }

    /// The verdict rule (#316/#338): the signature alone. A body-only edit
    /// (hash moves, signature stable) is not an API change; a signature
    /// move over a stable hash — the engine-upgrade reframe — is one,
    /// exactly the case the content-hash conjunct the rule replaced would
    /// have suppressed.
    #[test]
    fn api_changed_verdicts_on_the_signature_alone() {
        let fx = fixture();
        let before = snapshot(&fx.strings, &["fa"]);
        let mut body_edit = before.clone();
        body_edit.content_hash = content_hash(b"body");
        assert!(!api_changed(&before, &body_edit));

        let mut reframed = before.clone();
        reframed.api_signature = fold_api_signatures(&[[1; 16]]);
        assert_eq!(reframed.content_hash, before.content_hash);
        assert!(api_changed(&before, &reframed));
        assert!(!api_changed(&before, &before));
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
