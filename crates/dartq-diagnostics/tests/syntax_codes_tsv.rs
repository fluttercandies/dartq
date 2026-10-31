//! Content pin for the authoritative syntax-codes table (`data/syntax-codes.tsv`).

use std::path::Path;

/// blake3 digest of the pinned TSV snapshot (dart 3.13.7 oracle — the pinned
/// SDK registry `diagnostic.g.dart`; refreshed 2026-11-01 from the 2026-10-08
/// dart 3.13.1 capture, 7 measured-dead rows pruned 2026-10-09 — #222). The
/// table is 259 data rows: 255 registry-gated codes + 4 supplemental rows
/// (the 3-code language-version family plus
/// `invalid_covariant_modifier_in_primary_constructor`).
///
/// Any content edit changes the generated registry: re-verify the full table
/// against the pinned registry sources first, then re-pin this digest
/// deliberately. This test exists so a wrong text cannot ship green.
const PINNED_BLAKE3: &str = "0ca513794ac5b1523688affcb62e93b523288880fa377a23d59b0b16099dbb8b";

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
