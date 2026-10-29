//! GAP-PPIN parser recovery pins (#1690): the five owned rows of the
//! known-diffs ledger's parser-recovery family (the retired regression-case
//! corpus, cases 3f04615b / 4241f7cb / 6716c102 / 76dd104c / a7c0c7f0).
//! Every expected row set below is the frozen machine-captured oracle side
//! of the case file, converted to `(utf16 start, utf16 end)` spans over the
//! frozen inputs (verbatim under `tests/fixtures/`); semantic-tier and
//! lint-tier codes (unused_*, undefined_*, uri_*, duplicate_definition,
//! extra_positional_arguments, return_of_invalid_type) are outside the
//! parser's surface and pinned by their own tickets.

use std::path::PathBuf;

use dartq_scanner::{ScanConfig, TokenTable};

use super::collector::{Collector, latest_features};
use crate::parser::Parser;

/// The frozen case inputs, verbatim, relative to this crate's manifest.
const FIXTURE_ROOT: &str = "src/tests/fixtures/gate_ppin";

/// Reads a frozen case input and leaks it for the `'static` cursors.
fn corpus_source(rel: &str) -> &'static str {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(FIXTURE_ROOT)
        .join(rel);
    let raw =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    Box::leak(raw.into_boxed_str())
}

/// Scans without requiring scanner-clean input (the unterminated-literal
/// cases carry a scanner row), returning the leaked table plus the scanner's
/// own rows as `(code, start, end)` — the same shape as the parser
/// collector's, so both layers join into one sorted expectation.
fn scan_with_diagnostics(source: &'static str) -> (&'static TokenTable, Vec<(String, u32, u32)>) {
    let config = ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let mut files = dartq_source::FileTable::new();
    let file = files.intern(std::path::Path::new("test.dart"));
    let result = dartq_scanner::scan(source, config, file);
    let rows = result
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.name.to_owned(),
                d.range.offset,
                d.range.offset + d.range.length,
            )
        })
        .collect();
    (Box::leak(Box::new(result.tokens)), rows)
}

/// Parses `source` and returns the scanner+parser rows joined into one
/// `(code, start, end)` multiset sorted by offset (all expected arrays
/// below are written in offset order) plus the problem texts in emission
/// order.
fn rows_and_texts(source: &'static str) -> (Vec<(String, u32, u32)>, Vec<String>) {
    let (table, mut rows) = scan_with_diagnostics(source);
    let mut parser = Parser::new(
        source,
        table,
        latest_features(),
        Box::leak(Box::new(Collector::default())),
    );
    parser.parse_unit();
    rows.extend(parser.listener.errors.iter().cloned());
    rows.sort_by_key(|(_, start, end)| (*start, *end));
    (rows, parser.listener.texts.clone())
}

fn assert_rows(source: &'static str, expected: &[(&str, u32, u32)], label: &str) {
    let (rows, _) = rows_and_texts(source);
    let expected: Vec<(String, u32, u32)> = expected
        .iter()
        .map(|(c, s, e)| ((*c).to_owned(), *s, *e))
        .collect();
    assert_eq!(rows, expected, "{label}: diagnostic rows diverged");
}

// —— a7c0c7f0: expected_token for the scanner-unmatched `{` at EOF ————————

/// An unmatched function-body `{` parks a synthetic closer at the eof
/// offset; the official translation reports `expected_token '}'` there with
/// length 1 (translate_error_token.dart:89-104 `atOffset(length: 1)`, no
/// zero-width shape; the eof decrement in `makeError` never fires because
/// `_isAtEnd`'s walk hits the synthetic closer, a non-BAD token, first).
/// Oracle case a7c0c7f0 (`lib/src/patterns/issue50591_example2.dart`, 74
/// bytes): one row at (74, 75).
#[test]
fn ppin_a7c0c7f0_unmatched_open_brace_at_eof_row_shape() {
    let source = corpus_source("issue50591_example2.dart");
    assert_eq!(source.len(), 74, "frozen input is 74 bytes");
    assert_rows(
        source,
        &[("expected_token", 74, 75)],
        "unmatched `{` at eof carries the length-1 eof row",
    );
    // Focused shape: the same law on a minimal body, one level deep.
    assert_rows(
        "void main() {\n",
        &[("expected_token", 14, 15)],
        "minimal unclosed body parks the row at the eof offset",
    );
}

// —— 4241f7cb: keyword-named typedef faces —————————————————————————————————

/// The oracle face for a keyword taking the typedef-name position is the
/// verifier's isKeyword-and-not-pseudo gate (error_verifier.dart:3760-3766):
/// RESERVED and built-in words carry the built_in row exactly alike, pseudo
/// keywords stay silent. Over the frozen corpus file that yields 111
/// built_in rows + 111 keyword-name rows (56 non-pseudo keywords x 2
/// shapes, minus the `typedef void = ...` missing-name face on line 132).
#[test]
fn ppin_4241f7cb_typedef_keyword_name_faces() {
    let source = corpus_source("keyword_named_typedefs.dart");
    let (rows, _) = rows_and_texts(source);

    let built_in = rows
        .iter()
        .filter(|(c, _, _)| c == "built_in_identifier_in_declaration")
        .count();
    assert_eq!(built_in, 111, "built_in row count over the frozen corpus");
    let keyword_name = rows
        .iter()
        .filter(|(c, _, _)| c == "expected_identifier_but_got_keyword")
        .count();
    assert_eq!(
        keyword_name, 111,
        "keyword-name row count over the frozen corpus"
    );

    // Shape pins with exact spans: line 1 `typedef void abstract();` names
    // at column 14 (offset 13), line 2 `typedef abstract = ...` at column 9
    // (offset 33); both carry the row pair at the name token.
    for start in [13, 33] {
        for code in [
            "built_in_identifier_in_declaration",
            "expected_identifier_but_got_keyword",
        ] {
            assert!(
                rows.contains(&(code.to_owned(), start, start + 8)),
                "{code} missing at offset {start}"
            );
        }
    }
    // Pseudo names (line 7 `async` at offset 180) stay row-free.
    assert!(
        !rows.iter().any(|(_, s, e)| *s == 180 && *e == 185),
        "pseudo keyword name must stay silent at offset 180"
    );
    // Line 131 `typedef void void();` (offset 3620): the reserved `void`
    // name carries both faces.
    for code in [
        "built_in_identifier_in_declaration",
        "expected_identifier_but_got_keyword",
    ] {
        assert!(
            rows.contains(&(code.to_owned(), 3620, 3624)),
            "{code} missing at the line-131 void name"
        );
    }
    // Line 132 `typedef void = void Function();` takes the missing-name
    // recovery faces and NO keyword-name/built-in row (oracle 132 has no
    // built_in and no expected_identifier).
    for (code, start, end) in [
        ("expected_token", 3636, 3640),
        ("missing_identifier", 3641, 3642),
        ("missing_typedef_parameters", 3641, 3642),
        ("expected_executable", 3641, 3642),
        ("missing_function_body", 3658, 3659),
    ] {
        assert!(
            rows.contains(&(code.to_owned(), start, end)),
            "{code} missing at ({start}, {end}) for the line-132 face"
        );
    }

    // Focused text pins on the minimal shape (the capture-after /
    // report-after dance keeps the parser row ahead of the built-in row).
    let (_, minimal_texts) = rows_and_texts("typedef void abstract();\n");
    assert_eq!(
        minimal_texts,
        vec![
            "'abstract' can't be used as an identifier because it's a keyword.".to_string(),
            "The built-in identifier 'abstract' can't be used as a typedef name.".to_string(),
        ],
        "old-style shape emits the keyword-name row then the built-in row"
    );
}

// —— 3f04615b: accessor recovery inside a function body ———————————————————

/// `int get g => 1;` in statement position splits into (1) the local
/// variable declaration `int get` with the `;` walk-back anchor on `get` and
/// (2) a local function `g` whose missing parameter list reports on the
/// `=>`. The recovered function takes NO return type (the official lane
/// fires `handleNoType` — the noType branch of
/// parseExpressionStatementOrDeclarationAfterModifiers), which is what
/// keeps the oracle free of a `return_of_invalid_type` row against the
/// enclosing `void` function. Oracle case 3f04615b parser rows:
#[test]
fn ppin_3f04615b_accessor_recovery_local_function_shape() {
    let source = corpus_source("blindspot_accessors.dart");
    assert_rows(
        source,
        &[
            ("missing_method_parameters", 236, 237),
            ("missing_function_body", 346, 347),
            ("missing_function_body", 453, 454),
            ("expected_token", 531, 534),
            ("missing_function_parameters", 537, 539),
            ("unexpected_token", 615, 618),
        ],
        "accessor blindspot file parser rows match the frozen oracle set",
    );

    // Focused shape: the recovered local function must open with a
    // no-return-type event (`handle`, "no_type" in the collector log) —
    // the structural fact that keeps the `=> 1` body out of the enclosing
    // function's return check.
    let mut parser = super::collector::setup("void f() {\n  int get g => 1;\n}\n");
    parser.parse_unit();
    let mut rows = parser.listener.errors.clone();
    rows.sort();
    assert_eq!(
        rows,
        vec![
            ("expected_token".to_owned(), 17, 20),
            ("missing_function_parameters".to_owned(), 23, 25),
        ],
        "focused accessor recovery rows"
    );
    parser.listener.assert_sequence(&[("handle", "no_type")]);
}

// —— 6716c102: library-directive / unterminated-URI recovery ——————————————

/// The unterminated `import 'a.dart;` literal closes at end of line; the
/// scanner carries the `unterminated_string_literal` row at the closing
/// offset (26|15, length 1) and the parser recovers through the directive
/// grammar with the oracle's exact row set below (line 32's
/// `duplicate_definition` is semantic-tier and excluded).
#[test]
fn ppin_6716c102_library_directive_unterminated_recovery() {
    let source = corpus_source("blindspot_library_directives.dart");
    let scanner_rows: Vec<_> = scan_with_diagnostics(source).1;
    assert_eq!(
        scanner_rows,
        vec![("unterminated_string_literal".to_owned(), 1190, 1191)],
        "the unterminated literal carries exactly one scanner row at 26|15"
    );
    assert_rows(
        source,
        &[
            ("missing_identifier", 224, 225),
            ("expected_token", 276, 282),
            ("expected_string_literal", 283, 287),
            ("missing_const_final_var_or_type", 283, 287),
            ("expected_token", 283, 287),
            ("expected_executable", 287, 288),
            ("missing_const_final_var_or_type", 288, 292),
            ("directive_after_declaration", 414, 420),
            ("invalid_literal_in_configuration", 454, 456),
            ("directive_after_declaration", 1176, 1182),
            ("expected_token", 1183, 1191),
            ("unterminated_string_literal", 1190, 1191),
            ("directive_after_declaration", 1464, 1470),
            ("expected_token", 1501, 1502),
            ("expected_string_literal", 1503, 1504),
            ("missing_function_parameters", 1503, 1504),
            ("missing_function_body", 1504, 1505),
            ("expected_executable", 1504, 1505),
            ("missing_const_final_var_or_type", 1505, 1509),
        ],
        "library-directive blindspot joined scanner+parser rows match the frozen oracle set",
    );
}

// —— 76dd104c: string-interpolation typing recovery ————————————————————————

/// Typing inside `"${foo.` splits the literal at the unclosed interpolation;
/// the oracle recovers with one row set over lines 11-13 (the `${` closer at
/// 11|12, the `.`-chase nameless identifier, the argument-list commas at the
/// line-12 identifiers, the `;` and the `)`). The comma rows surface through
/// `expectedButGot`, whose pseudoSharedCode the analyzer re-renders as the
/// registry `Expected to find '<x>'.` text — pinned here verbatim.
///
/// The `missing_identifier` row anchors on the got token per the official
/// span law (upstream Dart SDK source, pkg/_fe_analyzer_shared
/// parser/util.dart `findNonZeroLengthToken` + the analyzer
/// `handleRecoverableError` offset/end formula): the got token here is the
/// scanner's zero-width synthetic interpolation closer parked at the
/// discarded `${` (offset 182), so the row keeps the got offset and borrows
/// the first non-zero-length token's length — the walked-to segment `foo.`
/// (length 4): row span 182..186, exactly the frozen oracle capture.
#[test]
fn ppin_76dd104c_interpolation_recovery_rows_and_texts() {
    let source = corpus_source("../curly/typing_string_interpolation_2.dart");
    let scanner_rows: Vec<_> = scan_with_diagnostics(source).1;
    assert_eq!(
        scanner_rows,
        vec![
            ("unterminated_string_literal".to_owned(), 187, 188),
            ("unterminated_string_literal".to_owned(), 213, 214),
        ],
        "both split-literal tails carry the scanner row"
    );
    let (rows, mut texts) = rows_and_texts(source);
    assert_eq!(
        rows,
        vec![
            ("expected_token".to_owned(), 182, 183),
            ("missing_identifier".to_owned(), 182, 186),
            ("unterminated_string_literal".to_owned(), 187, 188),
            ("expected_token".to_owned(), 193, 199),
            ("expected_token".to_owned(), 200, 205),
            ("expected_token".to_owned(), 206, 211),
            ("expected_token".to_owned(), 211, 214),
            ("expected_token".to_owned(), 211, 214),
            ("unterminated_string_literal".to_owned(), 213, 214),
            ("expected_token".to_owned(), 217, 218),
        ],
        "interpolation recovery joined scanner+parser rows match the frozen oracle set"
    );
    texts.sort();
    assert_eq!(
        texts,
        vec![
            "Expected an identifier.".to_string(),
            "Expected to find ')'.".to_string(),
            "Expected to find ','.".to_string(),
            "Expected to find ','.".to_string(),
            "Expected to find ','.".to_string(),
            "Expected to find ','.".to_string(),
            "Expected to find ';'.".to_string(),
            "Expected to find '}'.".to_string(),
        ],
        "the analyzer re-renders every expectedButGot flavor as the registry text"
    );
}

/// The missing-identifier span law over the live-probed `"${<word>.;`
/// shapes (dart 3.13.1 oracle probes, one per got-token shape): a real
/// got token anchors the row on its own span — the `;` at column 24/23/18,
/// length 1 — with the registry message `Expected an identifier.` (the
/// template carries no lexeme parameter, so the text is lexeme-free even
/// over a real got token). The zero-width got shape (the scanner's
/// synthetic interpolation closer) is the corpus pin above: span 182..186.
#[test]
fn ppin_interpolation_missing_identifier_span_law() {
    for (source, semi_offset) in [
        ("void m() { print(\"${foo.;", 24u32),
        ("void m() { print(\"${fo.;", 23u32),
        ("void m() { f(\"${x.;", 18u32),
    ] {
        let source: &'static str = Box::leak(source.to_string().into_boxed_str());
        // Parser-only rows and their parallel problem texts: the merged
        // `rows_and_texts` view sorts rows, so its text parallelism is
        // lost — the message pin needs the collector's own order.
        let (table, _) = scan_with_diagnostics(source);
        let mut listener = Collector::default();
        let mut parser = Parser::new(source, table, latest_features(), &mut listener);
        parser.parse_unit();
        let faces: Vec<(u32, u32, String)> = listener
            .errors
            .iter()
            .zip(&listener.texts)
            .filter(|((c, _, _), _)| c == "missing_identifier")
            .map(|((_, s, e), t)| (*s, *e, t.clone()))
            .collect();
        assert_eq!(
            faces,
            vec![(
                semi_offset,
                semi_offset + 1,
                "Expected an identifier.".to_string()
            )],
            "one missing_identifier on the got `;`'s own span: {source}"
        );
    }
}
