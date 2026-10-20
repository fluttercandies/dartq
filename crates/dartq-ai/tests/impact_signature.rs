//! The impact report's signature-evidence face (#338 → #1561): since v2
//! the `changed` section's per-symbol verdict compares the member-level
//! signature digests — a stable name whose canonical face moved ships its
//! row, the v1 name-presence proxy's documented under-report. These pins
//! date the contract: the row carries exactly `symbol` and
//! `signature_changed` (the digest pair stays internal), and the verdict
//! keys on the digest comparison.

use dartq_ai::impact::{
    self as query, FileSnapshot, IMPACT_SCHEMA_VERSION, ImpactLevel, LibraryChange,
};
use dartq_index::TestGraph;
use dartq_source::StringTable;

fn imports_of(_: u32) -> &'static [u32] {
    &[]
}

/// The blake3-128 of one member's face text.
fn member_digest(text: &str) -> [u8; 16] {
    let hash = blake3::hash(text.as_bytes());
    let mut digest = [0u8; 16];
    digest.copy_from_slice(&hash.as_bytes()[..16]);
    digest
}

/// A snapshot over (name, face-text) members: the digest columns ride the
/// face texts, so a signature-bearing edit under a stable name is a
/// changed face text behind an unchanged name — the v2 verdict's input.
fn snapshot(strings: &StringTable, members: &[(&str, &str)]) -> FileSnapshot {
    let mut pairs: Vec<(dartq_source::StringId, [u8; 16])> = members
        .iter()
        .map(|&(name, face)| (strings.intern(name), member_digest(face)))
        .collect();
    pairs.sort_unstable();
    pairs.dedup_by(|a, b| a.0 == b.0);
    let names: Vec<&str> = members.iter().map(|&(name, _)| name).collect();
    let face = names.join(",");
    let file_digest = blake3::hash(face.as_bytes());
    let mut api_signature = [0u8; 16];
    api_signature.copy_from_slice(&file_digest.as_bytes()[..16]);
    FileSnapshot {
        content_hash: query::content_hash(face.as_bytes()),
        api_signature,
        member_names: pairs.iter().map(|(name, _)| *name).collect(),
        member_digests: pairs.into_iter().map(|(_, digest)| digest).collect(),
    }
}

/// A signature digest keyed by `tag` — any two distinct tags answer two
/// distinct file-level digests.
fn signature_digest(tag: u8) -> [u8; 16] {
    query::fold_api_signatures(&[[tag; 16]])
}

/// An API-level change (a face name added) ships the verdict row with
/// `signature_changed: true` — and the row carries exactly the `symbol`
/// and `signature_changed` keys: the digest pair behind the verdict stays
/// internal to the query.
#[test]
fn signature_change_ships_the_verdict_without_evidence() {
    let strings = StringTable::new();
    let imports: [&[u32]; 2] = [&[], &[0]];
    let tests = TestGraph::default();
    let report = query::impact(
        2,
        |lib| imports[lib as usize],
        |lib| ["a.dart", "b.dart"][lib as usize],
        &tests,
        &strings,
        &[LibraryChange {
            library: 0,
            before: snapshot(&strings, &[("fa", "int fa")]),
            after: snapshot(&strings, &[("fa", "int fa"), ("fb", "int fb")]),
        }],
        |_| Vec::new(),
    );
    assert_eq!(
        report
            .changed
            .iter()
            .map(|row| (row.symbol.as_str(), row.signature_changed))
            .collect::<Vec<_>>(),
        vec![("a.dart|fb", true)],
        "the added name ships its API-level verdict"
    );

    let value = serde_json::to_value(&report).expect("serialize");
    let row = value["changed"][0].as_object().expect("row object");
    let mut keys: Vec<&str> = row.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["signature_changed", "symbol"],
        "the row shape — the digest pair stays internal"
    );
}

/// The v2 pin (#1561): a signature-bearing edit under a stable name — a
/// default value, a type refinement; here the engine-upgrade reframe —
/// moves the file-level digest and the member digest alike, and the
/// stable name now ships its row. The v1 proxy shipped nothing (the
/// name-presence walk saw no added or removed name).
#[test]
fn signature_edit_under_a_stable_name_ships_the_verdict_row() {
    let strings = StringTable::new();
    let imports: [&[u32]; 2] = [&[], &[0]];
    let tests = TestGraph::default();
    let report = query::impact(
        2,
        |lib| imports[lib as usize],
        |lib| ["a.dart", "b.dart"][lib as usize],
        &tests,
        &strings,
        &[LibraryChange {
            library: 0,
            before: snapshot(&strings, &[("fa", "int fa")]),
            after: {
                let mut after = snapshot(&strings, &[("fa", "String fa")]);
                after.api_signature = signature_digest(2);
                after
            },
        }],
        |_| Vec::new(),
    );
    assert_eq!(
        report
            .changed
            .iter()
            .map(|row| (row.symbol.as_str(), row.signature_changed))
            .collect::<Vec<_>>(),
        vec![("a.dart|fa", true)],
        "the stable name's moved digest ships the row"
    );
    assert_eq!(
        report
            .affected
            .iter()
            .map(|row| (row.path.as_str(), row.impact))
            .collect::<Vec<_>>(),
        vec![("b.dart", ImpactLevel::Semantic)],
        "the file-level digest carries the closure signal"
    );
}

/// The no-change case: identical snapshots answer the empty report. The
/// environment section stays empty on the pure query face — an environment
/// verdict needs diagnostics, so the commands face owns that section.
#[test]
fn no_change_answers_the_empty_report() {
    let strings = StringTable::new();
    let tests = TestGraph::default();
    let same = snapshot(&strings, &[("fa", "int fa")]);
    let report = query::impact(
        1,
        imports_of,
        |_| "a.dart",
        &tests,
        &strings,
        &[LibraryChange {
            library: 0,
            before: same.clone(),
            after: same,
        }],
        |_| Vec::new(),
    );
    assert_eq!(report.schema_version, IMPACT_SCHEMA_VERSION);
    assert!(report.changed.is_empty());
    assert!(report.affected.is_empty());
    assert!(report.affected_tests.is_empty());
    assert!(report.affected_environments.is_empty());
}
