//! The #3856 pins: the `duplicate_field_formal_parameter` row swap.
//!
//! The formal-parameter duplicate scope splits the row in two codes: when
//! BOTH colliding claims are initializing formals (`this.x`), the verifier
//! returns `duplicateFieldFormalParameter` instead of
//! `duplicateDefinition` (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/error/duplicate_definition_verifier.dart:433-442` —
//! `_FormalParameterDuplicateIdentifierScope.getDiagnostic`). Everything
//! else — a pair with at least one plain formal, a wildcard-shaped formal, a
//! member/type-parameter/statement collision — keeps `duplicate_definition`.
//! The scope keeps the FIRST claim of a colliding run
//! (`_DuplicateIdentifierScope.add` inserts only into an empty slot), so a
//! third same-name formal reports against the first one again.
//!
//! Measured against the pinned 3.13.7 oracle (`dart analyze
//! --format=machine`), one fixture per shape:
//!
//! ```text
//! class A { int x = 0; A(this.x, this.x); }
//!   ERROR|COMPILE_TIME_ERROR|DUPLICATE_FIELD_FORMAL_PARAMETER|<f>|3|18|1|The field 'x' can't be initialized by multiple parameters in the same constructor.
//! class A { int x = 0; A(this.x, this.x, this.x); }
//!   ERROR|...|DUPLICATE_FIELD_FORMAL_PARAMETER|<f>|3|18|1|...   (two rows: 18 and 26)
//!   ERROR|...|DUPLICATE_FIELD_FORMAL_PARAMETER|<f>|3|26|1|...
//! class A { int u = 0; A(this.u, {required this.u}); }
//!   ERROR|...|DUPLICATE_FIELD_FORMAL_PARAMETER|<f>|3|28|1|...
//! class A { int _ = 0; A(this._, this._); }
//!   ERROR|...|DUPLICATE_FIELD_FORMAL_PARAMETER|<f>|3|18|1|The field '_' ...
//! class A { int y = 0; A(this.y, y); }
//!   ERROR|COMPILE_TIME_ERROR|DUPLICATE_DEFINITION|<f>|3|13|1|The name 'y' is already defined.
//! class A { A(int z, int z); }
//!   ERROR|COMPILE_TIME_ERROR|DUPLICATE_DEFINITION|<f>|2|16|1|The name 'z' is already defined.
//! ```
//!
//! Every shape reports exactly one row per collision (no second row at the
//! same span), and the row lands on the SECOND formal's name token with
//! length 1 — the geometry the swap must preserve. The capture bit rides
//! `DuplicateDecl::field_formal`, so the bake
//! (`l1::duplicate_diagnostics`) and the fold
//! (`diagnose::duplicate_definition_diagnostics`) render the same code from
//! the same log, and the engine's bake-versus-fold drop accepts both names
//! (`semdiag.rs`).

use crate::codes::SemanticCode;
use dartq_diagnostics::Severity;

use super::test_rig::report_ops_bare;

/// The pair code (upstream registry, `analyzer-codes.tsv`).
const PAIR: &str = "duplicate_field_formal_parameter";
/// The plain code, still the row for every non-pair collision.
const PLAIN: &str = "duplicate_definition";

/// The duplicate rows of `source`, as `(code, offset)`, in report order —
/// every other family (and the unrelated `unused_field` warnings the
/// fixtures raise) filters out here.
fn duplicate_rows(source: &str) -> Vec<(&'static str, u32)> {
    report_ops_bare(source)
        .diagnostics
        .iter()
        .filter(|row| row.code.name == PAIR || row.code.name == PLAIN)
        .map(|row| (row.code.name, row.range.offset))
        .collect()
}

/// The byte offset of the `nth` (0-based) occurrence's name token of a
/// `this.<name>` formal: ASCII fixtures, so byte offsets are the row's
/// UTF-16 offsets too, and `this.` is five bytes.
fn this_formal_name_offset(source: &str, nth: usize) -> u32 {
    let start = source
        .match_indices("this.")
        .nth(nth)
        .expect("fixture carries the formal")
        .0;
    u32::try_from(start + "this.".len()).expect("fixture is short")
}

#[test]
fn two_field_formals_switch_to_the_pair_code() {
    const SOURCE: &str = "class A {\n  int x = 0;\n  A(this.x, this.x);\n}\n";
    assert_eq!(
        duplicate_rows(SOURCE),
        vec![(PAIR, this_formal_name_offset(SOURCE, 1))],
        "one pair row at the SECOND formal's name, no duplicate_definition row"
    );
}

#[test]
fn the_pair_row_carries_the_verbatim_pair_text() {
    const SOURCE: &str = "class A {\n  int x = 0;\n  A(this.x, this.x);\n}\n";
    let report = report_ops_bare(SOURCE);
    let row = report
        .diagnostics
        .iter()
        .find(|row| row.code.name == PAIR)
        .expect("the pair row");
    assert_eq!(
        row.message.as_str(),
        "The field 'x' can't be initialized by multiple parameters in the same constructor."
    );
    assert_eq!(row.code.machine, "DUPLICATE_FIELD_FORMAL_PARAMETER");
    assert_eq!(row.severity, Severity::Error);
    assert_eq!(row.range.length, 1, "the name token, not the formal");
}

#[test]
fn three_field_formals_report_twice_against_the_first() {
    const SOURCE: &str = "class A {\n  int x = 0;\n  A(this.x, this.x, this.x);\n}\n";
    assert_eq!(
        duplicate_rows(SOURCE),
        vec![
            (PAIR, this_formal_name_offset(SOURCE, 1)),
            (PAIR, this_formal_name_offset(SOURCE, 2)),
        ],
        "the scope keeps the first claim: rows 2 and 3 both collide with it"
    );
}

#[test]
fn a_named_field_formal_collides_with_the_positional_one() {
    // One scope spans the positional and named sections, so the pair code
    // applies across them (oracle column 28: the named formal's name).
    const SOURCE: &str = "class A {\n  int u = 0;\n  A(this.u, {required this.u});\n}\n";
    assert_eq!(
        duplicate_rows(SOURCE),
        vec![(PAIR, this_formal_name_offset(SOURCE, 1))]
    );
}

#[test]
fn a_wildcard_shaped_field_formal_is_still_a_field_formal() {
    // `this._` is a FieldFormalParameterElement, never a wildcard
    // (upstream `isWildcard`'s element test exits false), so the pair code
    // applies and the name renders verbatim.
    const SOURCE: &str = "class A {\n  int _ = 0;\n  A(this._, this._);\n}\n";
    assert_eq!(
        duplicate_rows(SOURCE),
        vec![(PAIR, this_formal_name_offset(SOURCE, 1))]
    );
}

#[test]
fn a_mixed_pair_keeps_duplicate_definition() {
    const SOURCE: &str = "class A {\n  int y = 0;\n  A(this.y, y);\n}\n";
    let second = SOURCE.rfind('y').expect("the plain formal's name");
    assert_eq!(
        duplicate_rows(SOURCE),
        vec![(PLAIN, u32::try_from(second).expect("fixture is short"))],
        "one plain formal is enough to keep the plain row"
    );
}

#[test]
fn a_plain_pair_keeps_duplicate_definition() {
    const SOURCE: &str = "class A {\n  A(int z, int z);\n}\n";
    let second = SOURCE.rfind('z').expect("the second formal's name");
    assert_eq!(
        duplicate_rows(SOURCE),
        vec![(PLAIN, u32::try_from(second).expect("fixture is short"))]
    );
}

#[test]
fn a_non_formal_duplicate_keeps_duplicate_definition() {
    // The bit is a formal-scope verdict only: two same-name fields are the
    // plain row even though the members are `this.x`-initialized elsewhere.
    const SOURCE: &str = "class A {\n  int x = 0;\n  int x = 1;\n}\n";
    let second = SOURCE.rfind("x =").expect("the second field's name");
    assert_eq!(
        duplicate_rows(SOURCE),
        vec![(PLAIN, u32::try_from(second).expect("fixture is short"))]
    );
}

/// The determinism acceptance item: the capture bit is part of the persisted
/// element payload (`PersistedDuplicate::field_formal`, `serde(default)` for
/// pre-column payloads), so a warm-start replay re-renders the same code a
/// cold fold would. The `persisted.rs` round-trip test pins the column
/// generically; this pin ties it to the shape that decides the code.
#[test]
fn the_pair_bit_is_what_the_capture_stores() {
    use crate::collector::collect;
    use crate::persisted::PersistedElements;
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_parser::ParserFeatures;
    use dartq_scanner::{ScanConfig, scan};
    use dartq_source::{FileTable, StringTable};
    use std::path::Path;

    const SOURCE: &str = "class A {\n  int x = 0;\n  A(this.x, this.x);\n}\n";
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let scanned = scan(
        SOURCE,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(scanned.diagnostics.is_empty(), "scanner diagnostics");
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let elements = collect(SOURCE, &scanned.tokens, features, &strings);
    assert_eq!(elements.duplicates().len(), 1);
    assert!(
        elements.duplicates()[0].field_formal,
        "the capture marks the pair"
    );

    let payload = PersistedElements::of(&elements, &strings);
    let bytes = serde_json::to_vec(&payload).expect("serialize");
    let decoded: PersistedElements = serde_json::from_slice(&bytes).expect("deserialize");
    let restored = decoded.into_elements(&strings);
    assert_eq!(restored.duplicates(), elements.duplicates());
    assert!(
        restored.duplicates()[0].field_formal,
        "the replayed payload still renders the pair code"
    );
}
