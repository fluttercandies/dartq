//! Frozen gate-universe artifacts: single-source derivations, `--check`
//! pins, and the matching regen entry points (extension-seams audit T5,
//! #2681).
//!
//! A freeze file's entry block is only as trustworthy as the cheapest
//! thing that re-derives it. Before this module the m1/m2 freezes were
//! pinned by tests that silently skipped without a local SDK checkout
//! (`DARTQ_SDK_CHECKOUT`), so CI — which sets no such variable — held no
//! opinion on them at all, and regeneration was a hand-edited entry
//! block. The #2648 lesson: a silently-drifted freeze makes the gate
//! judge the wrong universe without anything failing.
//!
//! Mechanism (one code path per artifact — the check and the regen share
//! the derivation and the serializer, so they cannot disagree):
//!
//! - `freeze_check_m1_codes` / `freeze_check_m2_warning_codes` derive the
//!   frozen sets from the repo-internal analyzer registry table
//!   `crates/dartq-diagnostics/data/analyzer-codes.tsv` (the generated
//!   mirror of the SDK unified registry; its own SDK linkage is
//!   `analyzer-codes-gen --check` in scripts/ci.sh and its content pin is
//!   the blake3 test in crates/dartq-diagnostics). No environment
//!   variable is involved: the derivation always runs in CI. The former
//!   live-registry pins (`m1_codes_file_matches_derivation`,
//!   `m2_warning_codes_file_matches_derivation`) retired into these
//!   checks for exactly that reason — an env-gated skip is not a pin.
//! - `freeze_check_m3_unused_codes` pins the fixture leg of the m3
//!   freeze (one code per targeted fixture); the registry leg stays in
//!   crates/dartq-semantic (`m3_codes_pin`), where its regen lives.
//! - `fixture_coverage_manifest_matches_corpus_derivation` (main.rs)
//!   uses the same byte-equality helper over the fixture-coverage
//!   canonical builder it shares with its regen test.
//!
//! `sem2_codes.txt` is deliberately outside this mechanism: its content
//! re-freeze belongs to the parity session (#2671) and the file is
//! currently behind its live derivation (the existing
//! `sem2_codes_file_matches_derivation` pin in main.rs fails on exactly
//! that drift). A byte check here would be red on day one, and pinning
//! the 14 missing codes as an "expected drift" marker would freeze the
//! drift state itself — the anti-pattern this module exists to remove.
//! Once #2671 re-freezes, adding the tier is a derivation fn + one check
//! test, same shape as the others.

use std::collections::BTreeSet;
use std::path::Path;

/// The generated analyzer registry table: `<class> TAB <name> TAB
/// <MACHINE> TAB <TYPE> TAB ...` rows over the SDK unified registry (see
/// its header for provenance and the fold rules). The repo-internal
/// drift surface for the m1/m2 derivations.
const ANALYZER_CODES_TSV: &str = "crates/dartq-diagnostics/data/analyzer-codes.tsv";

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("harness lives at <repo>/tests/harness")
}

/// The registry table's `(name, DiagnosticType)` rows; `#` comments and
/// blanks skipped. A short or malformed row is a table-format drift and
/// panics here rather than silently narrowing a derivation.
fn analyzer_tsv_rows(root: &Path) -> Vec<(String, String)> {
    let text = std::fs::read_to_string(root.join(ANALYZER_CODES_TSV))
        .unwrap_or_else(|e| panic!("{ANALYZER_CODES_TSV} must be present: {e}"));
    text.lines()
        .filter(|l| !l.trim_end_matches('\r').is_empty() && !l.starts_with('#'))
        .map(|l| {
            let cols: Vec<&str> = l.split('\t').collect();
            assert!(
                cols.len() >= 4,
                "{ANALYZER_CODES_TSV}: row with fewer than 4 columns: {l}"
            );
            (cols[1].to_string(), cols[3].to_string())
        })
        .collect()
}

/// The m1 freeze's documented derivation: the `undefined_*` and
/// `wrong_number_of_*` wildcard families over the registry names, plus
/// the explicit picks, minus `undefined_lint` (an analysis-options code,
/// not a Dart-language diagnostic; present in the registry, so the
/// exclusion is load-bearing).
///
/// `undefined_annotation` is a pick rather than a wildcard hit because it
/// is the sole analyzer-8.4.1-only member retained (the table mirrors the
/// SDK main registry, which never had it); `wrong_number_of_
/// type_arguments_method`, the other 8.4.1-only name, is intentionally
/// not gated — the SDK-main oracle can never emit it.
fn m1_derived_codes(rows: &[(String, String)]) -> BTreeSet<String> {
    let mut expected: BTreeSet<String> = rows
        .iter()
        .map(|(name, _)| name)
        .filter(|name| name.starts_with("undefined_") || name.starts_with("wrong_number_of_"))
        .cloned()
        .collect();
    for pick in [
        "uri_does_not_exist",
        "duplicate_definition",
        "invalid_assignment",
        "argument_type_not_assignable",
        "return_of_invalid_type",
        "invalid_null_aware_operator",
        // Frozen analyzer-8.4.1-only addition (see doc above).
        "undefined_annotation",
    ] {
        expected.insert(pick.to_string());
    }
    expected.remove("undefined_lint");
    expected
}

/// The m2 freeze's documented per-bucket picks: the WARNING-tier members
/// the M2 gate scores (M1 continuation, flow/null-safety soundness,
/// patterns/records/switch exhaustiveness, class modifiers, const and
/// literal semantics, combinator duplicates, override semantics). The
/// freeze is the intersection of this list with the registry's
/// `STATIC_WARNING` family, so a name the registry re-types or drops
/// falls out of the derivation and fails the check.
const M2_FROZEN_BUCKETS: &[&str] = &[
    // (a) M1 frozen-set continuation
    "invalid_null_aware_operator",
    "undefined_hidden_name",
    "undefined_referenced_parameter",
    "undefined_shown_name",
    // (b) flow analysis & null-safety soundness
    "body_might_complete_normally_nullable",
    "cast_from_null_always_fails",
    "cast_from_nullable_always_fails",
    "dead_code",
    "dead_code_catch_following_catch",
    "dead_code_on_catch_subtype",
    "dead_null_aware_expression",
    "null_argument_to_non_null_type",
    "null_check_always_fails",
    "nullable_type_in_catch_clause",
    "receiver_of_type_never",
    "type_check_with_null",
    "unnecessary_cast",
    "unnecessary_nan_comparison",
    "unnecessary_non_null_assertion",
    "unnecessary_null_comparison",
    "unnecessary_question_mark",
    "unnecessary_type_check",
    // (c) patterns / records / switch exhaustiveness & reachability
    "constant_pattern_never_matches_value_type",
    "missing_enum_constant_in_switch",
    "pattern_never_matches_value_type",
    "unreachable_switch_case",
    "unreachable_switch_default",
    "unnecessary_cast_pattern",
    "unnecessary_null_assert_pattern",
    "unnecessary_null_check_pattern",
    "unnecessary_wildcard_pattern",
    // (d) class modifiers / sealed
    "mixin_on_sealed_class",
    "subtype_of_sealed_class",
    // (e) const & literal semantics
    "equal_elements_in_set",
    "equal_keys_in_map",
    "non_const_call_to_literal_constructor",
    // (f) import/export combinator duplicates
    "duplicate_export",
    "duplicate_hidden_name",
    "duplicate_import",
    "duplicate_shown_name",
    // (g) override & interface-member semantics
    "non_nullable_equals_parameter",
    "override_on_non_overriding_member",
    "unnecessary_no_such_method",
];

fn m2_derived_codes(rows: &[(String, String)]) -> BTreeSet<String> {
    let warning_family: BTreeSet<&str> = rows
        .iter()
        .filter(|(_, dtype)| dtype == "STATIC_WARNING")
        .map(|(name, _)| name.as_str())
        .collect();
    M2_FROZEN_BUCKETS
        .iter()
        .filter(|code| warning_family.contains(**code))
        .map(|code| (*code).to_string())
        .collect()
}

/// The m3 freeze's fixture leg: the targeted fixtures are the freeze —
/// one code per `tests/corpus/m3-unused/lib/targeted/<code>.dart` file
/// ("one code per fixture, no parked remainder", the freeze header's
/// inclusion rule). Companion files (the `show`-companion exports) carry
/// the `_other` suffix and are not codes.
fn m3_targeted_codes(root: &Path) -> BTreeSet<String> {
    let dir = root.join("tests/corpus/m3-unused/lib/targeted");
    let mut codes = BTreeSet::new();
    for entry in std::fs::read_dir(&dir)
        .unwrap_or_else(|e| {
            panic!(
                "m3 targeted fixtures must be present ({}): {e}",
                dir.display()
            )
        })
        .flatten()
    {
        let path = entry.path();
        let is_dart = path
            .extension()
            .map(|e| e.to_string_lossy().into_owned())
            .is_some_and(|e| e == "dart");
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned());
        if let Some(stem) = stem
            && is_dart
            && !stem.ends_with("_other")
        {
            codes.insert(stem);
        }
    }
    codes
}

/// A codes freeze's canonical bytes: the committed `#`-comment header
/// block verbatim (it is deliberate prose — regens preserve it and
/// reconcile it by hand), then the derived codes sorted, one per line.
/// Sorted = `BTreeSet` iteration order = plain byte collation; the entry
/// block carries no grouping information the gate could observe (the
/// loader parses the file into a set).
pub(crate) fn codes_file_canonical(file_text: &str, codes: &BTreeSet<String>) -> String {
    let mut out = String::new();
    for line in file_text.lines().take_while(|l| {
        let l = l.trim_end_matches('\r');
        l.is_empty() || l.starts_with('#')
    }) {
        out.push_str(line.trim_end_matches('\r'));
        out.push('\n');
    }
    for code in codes {
        out.push_str(code);
        out.push('\n');
    }
    out
}

/// The mismatch report shared by every freeze check: names the artifact,
/// the exact regen command, and the first diverging lines on each side —
/// the failure is self-healing instructions, never a bare "failed".
fn freeze_mismatch_message(
    path: &str,
    file_text: &str,
    expected: &str,
    regen_command: &str,
) -> String {
    let mut report = format!(
        "{path}: the checked-in freeze does not match its derivation; \
         regenerate with `{regen_command}` (then reconcile the header prose)"
    );
    let file_lines: Vec<&str> = file_text.lines().collect();
    let expected_lines: Vec<&str> = expected.lines().collect();
    let mut shown = 0;
    for (i, (actual, want)) in file_lines.iter().zip(&expected_lines).enumerate() {
        if actual != want {
            report.push_str(&format!(
                "\n  first divergence at line {}: file has `{actual}`, derivation has `{want}`",
                i + 1
            ));
            shown = i + 1;
            break;
        }
    }
    if shown == 0 && file_lines.len() != expected_lines.len() {
        let short_side = if file_lines.len() < expected_lines.len() {
            format!(
                "the file ends early; derivation continues with `{}`",
                expected_lines[file_lines.len()]
            )
        } else {
            format!(
                "the file carries extra lines after the derived body, starting with `{}`",
                file_lines[expected_lines.len()]
            )
        };
        report.push_str(&format!(
            "\n  first divergence at line {}: {short_side}",
            expected_lines.len() + 1
        ));
    } else if shown == 0 {
        report.push_str(
            "\n  the line streams are identical; the difference is in the trailing bytes \
             (newline hygiene)",
        );
    }
    report
}

/// Byte-equality gate for freeze artifacts whose canonical text is built
/// by the caller (the fixture-coverage manifest).
pub(crate) fn assert_freeze_bytes(
    path: &str,
    file_text: &str,
    expected: &str,
    regen_command: &str,
) {
    if file_text != expected {
        panic!(
            "{}",
            freeze_mismatch_message(path, file_text, expected, regen_command)
        );
    }
}

/// Byte-equality gate for the codes freezes: serializes the derived set
/// through [`codes_file_canonical`] and, on mismatch, additionally names
/// the set-level missing/extra codes (which a line diff over a sorted
/// body renders as a confusing criss-cross).
fn assert_codes_freeze(
    path: &str,
    file_text: &str,
    derived: &BTreeSet<String>,
    regen_command: &str,
) {
    let expected = codes_file_canonical(file_text, derived);
    if file_text == expected {
        return;
    }
    let file_codes = crate::parse_codes_file(file_text).expect("current freeze parses");
    let missing: Vec<&String> = derived.difference(&file_codes).collect();
    let extra: Vec<&String> = file_codes.difference(derived).collect();
    let mut set_lines = String::new();
    if !missing.is_empty() {
        set_lines.push_str(&format!(
            "\n  codes the derivation has but the file lacks: {missing:?}"
        ));
    }
    if !extra.is_empty() {
        set_lines.push_str(&format!(
            "\n  codes the file has but the derivation lacks: {extra:?}"
        ));
    }
    if set_lines.is_empty() {
        set_lines.push_str(
            "\n  the code sets match; the drift is ordering or formatting in the entry block",
        );
    }
    let mut message = freeze_mismatch_message(path, file_text, &expected, regen_command);
    let first_newline = message.find('\n').unwrap_or(message.len());
    message.insert_str(first_newline, &set_lines);
    panic!("{message}");
}

/// The m1 freeze check: `tests/harness/m1_codes.txt` must equal its
/// derivation over the repo-internal analyzer registry table, byte for
/// byte. Registry drift that moves a wildcard family member fails here
/// before any differential run judges the wrong universe.
#[test]
fn freeze_check_m1_codes() {
    let root = repo_root();
    let rows = analyzer_tsv_rows(root);
    // The exclusion is load-bearing: `undefined_lint` sits in the table
    // (an analysis-options STATIC_WARNING name) and must stay out of the
    // gate.
    assert!(
        rows.iter().any(|(name, _)| name == "undefined_lint"),
        "the undefined_lint exclusion guard lost its anchor: the name left the registry table"
    );
    let derived = m1_derived_codes(&rows);
    let path = "tests/harness/m1_codes.txt";
    let text = std::fs::read_to_string(root.join(path))
        .unwrap_or_else(|e| panic!("{path} must be present: {e}"));
    assert_codes_freeze(
        path,
        &text,
        &derived,
        "cargo test -p harness regen_m1_codes -- --ignored",
    );
}

/// The m2 freeze check: `tests/harness/m2_warning_codes.txt` must equal
/// the per-bucket freeze intersected with the registry table's
/// `STATIC_WARNING` family, byte for byte. Load-bearing exclusions:
/// family members kept out on purpose (the file-header exclusion
/// families) must remain in the family (else this guard rots) and out of
/// the gate. `multiple_combinators` is excluded either way — the table
/// folds its dual registration to the SYNTACTIC_ERROR variant, and it is
/// not a bucket member.
#[test]
fn freeze_check_m2_warning_codes() {
    let root = repo_root();
    let rows = analyzer_tsv_rows(root);
    let warning_family: BTreeSet<&str> = rows
        .iter()
        .filter(|(_, dtype)| dtype == "STATIC_WARNING")
        .map(|(name, _)| name.as_str())
        .collect();
    for anchored in ["undefined_lint", "unused_import", "strict_raw_type"] {
        assert!(
            warning_family.contains(anchored),
            "the {anchored} exclusion guard lost its anchor: the name left the STATIC_WARNING family"
        );
    }
    let derived = m2_derived_codes(&rows);
    for excluded in [
        "undefined_lint",
        "unused_import",
        "strict_raw_type",
        "multiple_combinators",
    ] {
        assert!(
            !derived.contains(excluded),
            "{excluded} must stay excluded from the M2 WARNING tier"
        );
    }
    let path = "tests/harness/m2_warning_codes.txt";
    let text = std::fs::read_to_string(root.join(path))
        .unwrap_or_else(|e| panic!("{path} must be present: {e}"));
    assert_codes_freeze(
        path,
        &text,
        &derived,
        "cargo test -p harness regen_m2_warning_codes -- --ignored",
    );
}

/// The m3 freeze check, fixture leg: the entry block must equal the
/// targeted fixture stems. The registry leg (freeze == the implemented
/// usage family) lives in crates/dartq-semantic (`m3_codes_pin`); the
/// canonical writer for this file is that crate's regen — after adding a
/// targeted fixture, re-freeze there and reconcile this header.
#[test]
fn freeze_check_m3_unused_codes() {
    let root = repo_root();
    let derived = m3_targeted_codes(root);
    let path = "tests/harness/m3_unused_codes.txt";
    let text = std::fs::read_to_string(root.join(path))
        .unwrap_or_else(|e| panic!("{path} must be present: {e}"));
    assert_codes_freeze(
        path,
        &text,
        &derived,
        "add/remove the targeted fixture under tests/corpus/m3-unused/lib/targeted/, \
         then cargo test -p dartq-semantic --test m3_codes_pin regen_m3_unused_codes \
         -- --ignored",
    );
}

/// Regen (writes `tests/harness/m1_codes.txt`; not run by default): the
/// derivation's codes through the same serializer the check compares
/// with, preserving the header block verbatim. Reconcile the header prose
/// (it narrates counts) after running.
#[test]
#[ignore = "rewrites tests/harness/m1_codes.txt; explicit re-freeze maintenance"]
fn regen_m1_codes() {
    regen_codes_freeze(
        "tests/harness/m1_codes.txt",
        &m1_derived_codes(&analyzer_tsv_rows(repo_root())),
    );
}

/// Regen (writes `tests/harness/m2_warning_codes.txt`; not run by
/// default): same contract as `regen_m1_codes`.
#[test]
#[ignore = "rewrites tests/harness/m2_warning_codes.txt; explicit re-freeze maintenance"]
fn regen_m2_warning_codes() {
    regen_codes_freeze(
        "tests/harness/m2_warning_codes.txt",
        &m2_derived_codes(&analyzer_tsv_rows(repo_root())),
    );
}

/// The shared regen body for the codes freezes: derive, serialize through
/// `codes_file_canonical` (the exact bytes the check asserts), and write;
/// print the added/removed codes so the re-freeze is a reviewed act.
fn regen_codes_freeze(path: &str, derived: &BTreeSet<String>) {
    let root = repo_root();
    let full = root.join(path);
    let text = std::fs::read_to_string(&full)
        .unwrap_or_else(|e| panic!("reading {}: {e}", full.display()));
    let canonical = codes_file_canonical(&text, derived);
    let old: BTreeSet<String> = crate::parse_codes_file(&text).expect("current freeze parses");
    let added: Vec<&String> = derived.difference(&old).collect();
    let removed: Vec<&String> = old.difference(derived).collect();
    std::fs::write(&full, canonical).unwrap_or_else(|e| panic!("writing {}: {e}", full.display()));
    println!("regen {path}: {} codes (was {})", derived.len(), old.len());
    println!("  added: {added:?}");
    println!("  removed: {removed:?}");
    println!("  reconcile the header prose (it narrates counts and buckets)");
}
