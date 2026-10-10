//! Content pin for the authoritative syntax-codes table (`data/syntax-codes.tsv`).

use std::path::Path;

/// blake3 digest of the pinned TSV snapshot (dart 3.13.1 oracle;
/// analyzer-8.4.1 + _fe_analyzer_shared-91.0.0 registries; 2026-10-08
/// capture, 7 measured-dead rows pruned 2026-10-09 — #222). The surviving
/// table is byte-audited against those pinned registries (258/258 rows
/// verified).
///
/// Any content edit changes the generated registry: re-verify the full table
/// against the pinned registry sources first, then re-pin this digest
/// deliberately. This test exists so a wrong text cannot ship green.
const PINNED_BLAKE3: &str = "834e5bc1097d77526373e2b4dd862edf0d6474dc0b59e72ee66d5d7af5a563c5";

#[test]
fn syntax_codes_tsv_matches_pinned_blake3() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("data/syntax-codes.tsv");
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let actual = blake3::hash(&bytes).to_hex().to_string();
    assert_eq!(
        actual, PINNED_BLAKE3,
        "syntax-codes.tsv drifted from the pinned snapshot (actual blake3: {actual}); \
         re-verify against the pinned registry sources, then re-pin deliberately"
    );
}
