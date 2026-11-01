//! The #3484 pins — `scan_clause_type` builds each [`SuperEntry`]'s `span`
//! from token fields: the span is a diagnostic [`SourceRange`], whose
//! coordinates are UTF-16 code units, but the constructor read the token's
//! `byte_offset`/`byte_len`. On a file with any non-ASCII text before the
//! header every clause row drifted by the byte−UTF-16 deficit (the corpus's
//! `mixin_of_non_class` +8-column family, `31:24` reported as `32:6`), and
//! the drift broke more than the reported column: `suppress_type_row` /
//! `remove_clause_miss` key on the same span, so the resolver's
//! `undefined_class` row inside the clause escaped suppression on wide-
//! character files.
//!
//! Every pin asserts the exact `offset`/`length` in UTF-16 units —
//! presence alone cannot catch the drift. `report_ops` prepends
//! [`CORE_PRELUDE`] (pure ASCII, so its byte length is its UTF-16 length);
//! all expected offsets are `PRELUDE + <in-fixture units>`.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::{Diagnostic, FileReport};

/// The prelude's UTF-16 length — pure ASCII, so byte length = unit count.
const PRELUDE: u32 = CORE_PRELUDE.len() as u32;

/// The rows of one code, in emission order.
fn rows_of<'a>(report: &'a FileReport, code: &str) -> Vec<&'a Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .collect()
}

/// The `extends` and `on` clause faces carry the same span constructor —
/// `extends_non_class` and `mixin_super_class_constraint_non_interface`
/// (the corpus's second drifting code) must convert identically.
#[test]
fn extends_and_on_clause_spans_are_utf16() {
    // `// é\n` = 5 units.
    // `class D extends Missing {}` — `Missing` at in-line 16 → 5 + 16 = 21.
    // Line 2 is 26 units + `\n` = 27, so line 3 `mixin M on Missing {}`
    // starts at unit 5 + 27 = 32; its `Missing` sits at in-line 11 →
    // 32 + 11 = 43.
    let report = report_ops("// \u{00E9}\nclass D extends Missing {}\nmixin M on Missing {}");
    let extends_rows = rows_of(&report, "extends_non_class");
    assert_eq!(extends_rows.len(), 1, "{:?}", report.diagnostics);
    assert_eq!(extends_rows[0].range.offset, PRELUDE + 21);
    let on_rows = rows_of(&report, "mixin_super_class_constraint_non_interface");
    assert_eq!(on_rows.len(), 1, "{:?}", report.diagnostics);
    assert_eq!(on_rows[0].range.offset, PRELUDE + 43);
}

/// `’ “ ” —` on the line before the header: four BMP wide characters,
/// eight bytes of byte−UTF-16 deficit — the corpus drift shape. Line 1 is
/// `// ’ “ ” —\n` = 11 UTF-16 units (19 bytes); `Missing` sits at in-line
/// offset 28 → absolute unit 11 + 28 = 39. Before the fix the span carried
/// the BYTE position (47 for `Missing`) — eight units late, and the
/// byte-keyed anchor also missed every UTF-16-keyed suppression and ledger
/// match.
#[test]
fn with_clause_span_is_utf16_not_bytes() {
    let report = report_ops(
        "// \u{2019} \u{201C} \u{201D} \u{2014}\nclass C extends Object with Missing {}",
    );
    let rows = rows_of(&report, "mixin_of_non_class");
    assert_eq!(rows.len(), 1, "{:?}", report.diagnostics);
    assert_eq!(
        rows[0].range.offset,
        PRELUDE + 39,
        "utf16 offset of `Missing`"
    );
    assert_eq!(rows[0].range.length, 7);
    // The drift's second face: the resolver row inside the clause span must
    // stay suppressed — before the fix the byte-keyed span never covered the
    // utf16-keyed row, so `undefined_class` leaked beside the clause code.
    assert_eq!(
        rows_of(&report, "undefined_class").len(),
        0,
        "resolver row must stay suppressed: {:?}",
        report.diagnostics
    );
}

/// The same file without wide characters: byte and UTF-16 offsets agree, so
/// the fix must be a no-op — the pure-ASCII control.
#[test]
fn ascii_only_file_is_unchanged() {
    let report = report_ops("// plain comment\nclass C extends Object with Missing {}");
    let rows = rows_of(&report, "mixin_of_non_class");
    assert_eq!(rows.len(), 1, "{:?}", report.diagnostics);
    // `// plain comment\n` = 17 units; `Missing` at 17 + 28 = 45.
    assert_eq!(rows[0].range.offset, PRELUDE + 45);
    assert_eq!(rows[0].range.length, 7);
    assert_eq!(rows_of(&report, "undefined_class").len(), 0);
}

/// An astral character before the header — the surrogate-pair case: one
/// emoji is four bytes but TWO UTF-16 units (deficit 2, not 3). A fix that
/// merely subtracted one per char would land one unit short.
#[test]
fn astral_char_counts_two_utf16_units() {
    // `// 😀\n` = 8 bytes, 6 UTF-16 units; `Missing` at 6 + 28 = 34.
    let report = report_ops("// \u{1F600}\nclass C extends Object with Missing {}");
    let rows = rows_of(&report, "mixin_of_non_class");
    assert_eq!(rows.len(), 1, "{:?}", report.diagnostics);
    assert_eq!(rows[0].range.offset, PRELUDE + 34);
    assert_eq!(rows[0].range.length, 7);
    assert_eq!(rows_of(&report, "undefined_class").len(), 0);
}

/// An accented BMP character — the single-unit multi-byte case (deficit 1):
/// `é` is two bytes, one UTF-16 unit.
#[test]
fn bmp_multibyte_char_counts_one_utf16_unit() {
    // `// é\n` = 6 bytes, 5 UTF-16 units; `Missing` at 5 + 28 = 33.
    let report = report_ops("// \u{00E9}\nclass C extends Object with Missing {}");
    let rows = rows_of(&report, "mixin_of_non_class");
    assert_eq!(rows.len(), 1, "{:?}", report.diagnostics);
    assert_eq!(rows[0].range.offset, PRELUDE + 33);
    assert_eq!(rows[0].range.length, 7);
}

/// CRLF line endings: the `\r\n` pair is one break whose two bytes are two
/// UTF-16 units — the conversion must not misread the two-byte break as a
/// wide character.
#[test]
fn crlf_breaks_do_not_shift_the_span() {
    // `// é\r\n` = 7 bytes, 6 UTF-16 units; `Missing` at 6 + 28 = 34.
    let report = report_ops("// \u{00E9}\r\nclass C extends Object with Missing {}");
    let rows = rows_of(&report, "mixin_of_non_class");
    assert_eq!(rows.len(), 1, "{:?}", report.diagnostics);
    assert_eq!(rows[0].range.offset, PRELUDE + 34);
    assert_eq!(rows[0].range.length, 7);
}

/// A wide character inside the SAME line as the header — the in-line
/// column drift, the corpus's `31:24` shape. A `//` comment would eat the
/// header, so the wide char rides a block comment: `/* ’ */ ` is 8
/// characters (’ one unit) → `Missing` at 8 + 28 = 36.
#[test]
fn same_line_wide_char_shifts_the_column() {
    let report = report_ops("/* \u{2019} */ class C extends Object with Missing {}");
    let rows = rows_of(&report, "mixin_of_non_class");
    assert_eq!(rows.len(), 1, "{:?}", report.diagnostics);
    assert_eq!(rows[0].range.offset, PRELUDE + 36);
    assert_eq!(rows[0].range.length, 7);
}

/// A row anchored on the class NAME (not a clause span) beside a `with`
/// clause row: the name anchor was never built from `byte_offset` and must
/// stay put while the clause anchor converts — proving the fix changed only
/// the token-sourced span, not the name-range path.
#[test]
fn name_anchored_row_and_clause_row_agree_in_utf16() {
    // `// ’ “ ” —\n` = 11 units.
    // `abstract class B { void f(); } class C extends B with Missing {}`
    // `C` at in-line 37 → 11 + 37 = 48; `Missing` at in-line 54 → 65.
    let report = report_ops(
        "// \u{2019} \u{201C} \u{201D} \u{2014}\nabstract class B { void f(); } class C extends B with Missing {}",
    );
    let name_rows = rows_of(&report, "non_abstract_class_inherits_abstract_member");
    assert_eq!(name_rows.len(), 1, "{:?}", report.diagnostics);
    assert_eq!(
        name_rows[0].range.offset,
        PRELUDE + 48,
        "name anchor is utf16"
    );
    let clause_rows = rows_of(&report, "mixin_of_non_class");
    assert_eq!(clause_rows.len(), 1, "{:?}", report.diagnostics);
    assert_eq!(
        clause_rows[0].range.offset,
        PRELUDE + 65,
        "clause anchor is utf16"
    );
}

/// A prefixed clause face (`with p.Missing`): the span opens at the PREFIX
/// token and closes at the leaf's end — both ends of the byte→UTF-16
/// conversion must run. `p` at in-line 28 → 5 + 28 = 33; `p.Missing` is
/// nine units.
#[test]
fn prefixed_face_span_covers_prefix_through_leaf() {
    let report = report_ops("// \u{00E9}\nclass C extends Object with p.Missing {}");
    // `p` is an unregistered prefix — the face never resolves — but the
    // scan still records the entry and reports at its written span.
    let clause_rows = rows_of(&report, "mixin_of_non_class");
    assert_eq!(clause_rows.len(), 1, "{:?}", report.diagnostics);
    assert_eq!(clause_rows[0].range.offset, PRELUDE + 33);
    assert_eq!(clause_rows[0].range.length, 9);
}
