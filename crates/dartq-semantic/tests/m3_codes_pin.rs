//! Governance pin for the frozen M3 unused-family gate universe
//! (`tests/harness/m3_unused_codes.txt`, the `--codes-file` of
//! `scripts/m3-gate.sh`): the freeze must stay exactly the usage-analysis
//! `unused_*` family the semantic registry implements — no orphans (rows
//! the implementation cannot emit), no gaps (implemented family codes the
//! gate would never judge).
//!
//! The implementation side is the live [`SemanticCode`] registry: each
//! family member below is a compile-checked variant, and every frozen
//! name is read back through `code_info`, not copied. A stale freeze
//! fails here with the offending rows named.

use std::collections::BTreeSet;
use std::path::Path;

use dartq_diagnostics::DiagnosticType;
use dartq_semantic::codes::SemanticCode;

/// The freeze file `scripts/m3-gate.sh` scopes the WARNING-tier run with;
/// relative to this crate's manifest.
const M3_UNUSED_CODES_FREEZE: &str = "../../tests/harness/m3_unused_codes.txt";

/// The usage-analysis `unused_*` family (`unused_local_elements_verifier.dart`
/// plus the import/`useResult`/label siblings — the 11 registry
/// STATIC_WARNING names the M2 freeze excludes). Variants, not strings:
/// removing or renaming a registry variant breaks this list, which is the
/// strongest possible implementation-sync signal.
const USAGE_FAMILY: &[SemanticCode] = &[
    SemanticCode::UnusedLocalVariable,
    SemanticCode::UnusedElement,
    SemanticCode::UnusedField,
    SemanticCode::UnusedElementParameter,
    SemanticCode::UnusedCatchClause,
    SemanticCode::UnusedCatchStack,
    SemanticCode::UnusedImport,
    SemanticCode::UnusedShownName,
    SemanticCode::UnusedResult,
    SemanticCode::UnusedLabel,
    SemanticCode::UnusedFieldFromPrimaryConstructor,
];

/// The registry-derived gate universe: the family's machine names, each
/// asserted `STATIC_WARNING` (the freeze scopes a WARNING-tier run).
fn usage_universe() -> BTreeSet<&'static str> {
    USAGE_FAMILY
        .iter()
        .map(|code| {
            let info = code.code_info();
            assert!(
                matches!(code.dtype(), DiagnosticType::StaticWarning),
                "{} left the STATIC_WARNING tier the m3 gate scopes",
                info.name
            );
            info.name
        })
        .collect()
}

/// The freeze file's code rows: one code per line, `#` comments and blanks
/// ignored (the gate script's own reading).
fn freeze_rows(text: &str) -> Vec<&str> {
    text.lines()
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

/// The governance pin: the freeze and the registry-derived universe must
/// be the same set.
#[test]
fn m3_unused_codes_freeze_matches_the_usage_registry() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(M3_UNUSED_CODES_FREEZE);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let rows = freeze_rows(&text);

    let mut seen = BTreeSet::new();
    let dupes: Vec<&str> = rows
        .iter()
        .copied()
        .filter(|row| !seen.insert(*row))
        .collect();
    assert!(
        dupes.is_empty(),
        "{M3_UNUSED_CODES_FREEZE}: duplicate entries: {dupes:?}"
    );

    let universe = usage_universe();
    let frozen: BTreeSet<&str> = rows.iter().copied().collect();

    let orphans: Vec<&str> = rows
        .iter()
        .copied()
        .filter(|row| !universe.contains(*row))
        .collect();
    assert!(
        orphans.is_empty(),
        "{M3_UNUSED_CODES_FREEZE}: rows the usage registry cannot emit: {orphans:?}"
    );

    let gaps: Vec<&str> = universe
        .iter()
        .copied()
        .filter(|name| !frozen.contains(name))
        .collect();
    assert!(
        gaps.is_empty(),
        "{M3_UNUSED_CODES_FREEZE}: implemented unused-family codes missing from the \
         freeze (the gate would never judge them): {gaps:?} — re-freeze: add each \
         code's targeted fixture under tests/corpus/m3-unused/lib/targeted/, then \
         regenerate (cargo test -p dartq-semantic --test m3_codes_pin \
         regen_m3_unused_codes -- --ignored) and reconcile the header prose"
    );
}

/// Regenerates the freeze file's entry block from the registry-derived
/// universe, preserving the curated comment header verbatim. Oracle-free
/// on the code side, but the freeze's inclusion rule also requires each
/// code to carry a targeted fixture under
/// `tests/corpus/m3-unused/lib/targeted/` — add the fixture first, or the
/// new row mints permanent oracle-only denominator rows in the gate. Run
/// explicitly, then reconcile the header prose:
///
/// ```text
/// cargo test -p dartq-semantic --test m3_codes_pin regen_m3_unused_codes -- --ignored
/// ```
#[test]
#[ignore = "rewrites tests/harness/m3_unused_codes.txt; explicit re-freeze maintenance (targeted fixture first)"]
fn regen_m3_unused_codes() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(M3_UNUSED_CODES_FREEZE);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let header_end = text
        .lines()
        .position(|line| {
            let line = line.trim_end_matches('\r');
            !line.is_empty() && !line.starts_with('#')
        })
        .expect("the freeze file carries an entry block");

    let mut entries: Vec<String> = usage_universe().into_iter().map(str::to_owned).collect();
    entries.sort();

    let old: BTreeSet<String> = freeze_rows(&text).into_iter().map(str::to_owned).collect();
    let new: BTreeSet<String> = entries.iter().cloned().collect();
    let added: Vec<&String> = new.difference(&old).collect();
    let removed: Vec<&String> = old.difference(&new).collect();

    let mut out = String::with_capacity(text.len() + entries.len() * 32);
    for line in text.lines().take(header_end) {
        out.push_str(line.trim_end_matches('\r'));
        out.push('\n');
    }
    for entry in &entries {
        out.push_str(entry);
        out.push('\n');
    }
    std::fs::write(&path, out).unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));

    println!(
        "regen_m3_unused_codes: {} entries (was {}); reconcile the comment header",
        entries.len(),
        old.len()
    );
    println!("  added: {added:?}");
    println!("  removed: {removed:?}");
}
