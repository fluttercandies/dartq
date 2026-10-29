//! Wave-4 recovery families (#2791): the metadata-reference reserved-name
//! insert (annotation local-decl `;` anchor family), the import-prefix
//! declaration insert for reserved and built-in top-level heads
//! (configured-import family), the for-in stall-report gating
//! (for-each await family), and the name-less `(`-suffix face behind a
//! `=>` (lints-doc `)` drift tail). Every pinned span is the frozen oracle
//! row (conformance/expected.tsv, the cluster's expected face list).

use dartq_scanner::ScanConfig;

use super::collector::{Collector, latest_features};
use crate::parser::Parser;
use crate::ParserFeatures;

/// Parses `source` and returns the scanner+parser rows as
/// `(code, start, end)` sorted by offset (gate_bi_name_faces harness shape).
fn rows_with(features: ParserFeatures, source: &'static str) -> Vec<(String, u32, u32)> {
    let config = ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let mut files = dartq_source::FileTable::new();
    let file = files.intern(std::path::Path::new("test.dart"));
    let result = dartq_scanner::scan(source, config, file);
    assert!(
        result.diagnostics.is_empty(),
        "scanner diagnostics in {source:?}: {:?}",
        result.diagnostics
    );
    let mut rows: Vec<(String, u32, u32)> = result
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
    let mut parser = Parser::new(
        source,
        Box::leak(Box::new(result.tokens)),
        features,
        Box::leak(Box::new(Collector::default())),
    );
    parser.parse_unit();
    rows.extend(parser.listener.errors.iter().cloned());
    // Canonical order: position, then code — rows sharing an anchor (the
    // oracle's position-sorted case annotations) compare deterministically.
    rows.sort_by(|a, b| (a.1, a.2, &a.0).cmp(&(b.1, b.2, &b.0)));
    rows
}

fn rows(source: &'static str) -> Vec<(String, u32, u32)> {
    rows_with(latest_features(), source)
}

fn leaked(source: &str) -> &'static str {
    Box::leak(source.to_owned().into_boxed_str())
}

/// The parser-tier face set for `source` equals `expected` exactly: the
/// oracle's parser faces, in offset order. (Semantic-tier faces —
/// `undefined_annotation`, `unused_local_variable`, … — are outside this
/// crate and stay out of the pins.)
fn assert_faces(source: &'static str, expected: &[(&str, u32, u32)]) {
    let rows = rows(source);
    let expected: Vec<(String, u32, u32)> = expected
        .iter()
        .map(|(c, s, e)| ((*c).to_owned(), *s, *e))
        .collect();
    assert_eq!(rows, expected, "faces for {source:?}");
}

// —— annotation local-decl `;` anchor family ————————————————————————————

/// `@` + reserved statement-start keyword: the keyword is never eaten as
/// the annotation name — a synthetic name splices in front of it
/// (MetadataReferenceIdentifierContext.ensureIdentifier,
/// identifier_context_impl.dart:1086-1097, looksLikeStatementStart arm), the
/// empty annotation becomes metadata of an empty variable declaration, and
/// the ensureSemicolon row anchors on the `@`
/// (findPreviousNonZeroLengthToken over the two zero-width synthetics;
/// oracle partial_code/annotation local_ampersand_assert).
#[test]
fn annotation_local_reserved_keyword_semicolon_anchor() {
    assert_faces(
        leaked("\nf() { @ assert (true); }"),
        &[
            ("expected_token", 7, 8),      // ';' at the `@`
            ("missing_identifier", 9, 15), // at `assert`
        ],
    );
}

/// `@ var x;`: the annotation carries the empty name and `var x` stays the
/// declaration — one `missing_identifier` row, no semicolon row (oracle
/// local_ampersand_localVariable: the variables declaration absorbs the
/// annotation).
#[test]
fn annotation_local_var_keeps_declaration() {
    assert_faces(
        leaked("\nf() { @ var x; }"),
        &[("missing_identifier", 9, 12)],
    );
}

/// Class-member metadata with a reserved head: same MetadataReference arm
/// (oracle classMember_ampersand_methodVoid — `missing_identifier` on
/// `void`).
#[test]
fn annotation_class_member_reserved_keyword() {
    assert_faces(
        leaked("class C { @ void f() {} }"),
        &[("missing_identifier", 12, 16)],
    );
}

// —— configured-import family ————————————————————————————————————————————

/// `as` + reserved declaration head: the ImportPrefixDeclaration insert
/// splices a synthetic prefix, the `;` row anchors on `as`, and the class
/// parses at top level (ImportPrefixIdentifierContext branch 3,
/// identifier_context_impl.dart:706-716; oracle import_directive as_class).
#[test]
fn import_prefix_reserved_head() {
    assert_faces(
        leaked("import 'a.dart' as class A {}"),
        &[
            ("expected_token", 16, 18),    // ';' at `as`
            ("missing_identifier", 19, 24), // at `class`
        ],
    );
}

/// The built-in top-level heads take the same insert (branch 3 precedes the
/// branch-2 built-in-prefix face when the follower does not continue the
/// clause; oracle import_directive as_setter — `missing_identifier` on
/// `set`).
#[test]
fn import_prefix_builtin_head() {
    assert_faces(
        leaked("import 'a.dart' as set a(b) {}"),
        &[
            ("expected_token", 16, 18),   // ';' at `as`
            ("missing_identifier", 19, 22), // at `set`
        ],
    );
}

/// Combinator + reserved head (the #2786 Combinator arm, locked here for
/// the directive shape; oracle import_directive show_class).
#[test]
fn import_combinator_reserved_head() {
    assert_faces(
        leaked("import 'a.dart' show class A {}"),
        &[
            ("expected_token", 16, 20),    // ';' at `show`
            ("missing_identifier", 21, 26), // at `class`
        ],
    );
}

// —— for-each family —————————————————————————————————————————————————————

/// `await for (… in <statement-keyword fragment> …)` with an unclosed `(`:
/// the for-in tail has NO stall row — the iterable takes the recovered
/// identifier face and the `)` row anchors there (ensureCloseParen's
/// moveSynthetic; oracle for_each_statement await_in_break).
#[test]
fn for_in_await_iterable_recovery_no_stall() {
    assert_faces(
        leaked("\nf() async { await for (A a in break; }"),
        &[
            ("expected_token", 31, 32),    // ')' at `break`
            ("missing_identifier", 31, 36), // at `break`
        ],
    );
}

/// `for (var a in {} }`: the set literal is the iterable; the body's
/// synthetic-identifier expression statement walks the `;` anchor back over
/// the relocated synthetic `)` to the literal's closer (oracle
/// for_each_statement in_block).
#[test]
fn for_in_block_iterable_semicolon_anchor() {
    assert_faces(
        leaked("\nf() { for (var a in {} }"),
        &[
            ("expected_token", 22, 23),    // ';' at the literal's `}`
            ("expected_token", 24, 25),    // ')' at the block's `}`
            ("missing_identifier", 24, 25), // at the block's `}`
        ],
    );
}

/// The C-style stall row survives where it belongs: an unclosed `(` in a
/// C-style for header still reports the enclosing block's `}`
/// (parseForLoopPartsRest :10369-10373, the wave-2 pin's shape re-locked).
#[test]
fn for_rest_stall_survives() {
    let rows = rows(leaked("main() {\n  for (var\n}\n"));
    assert!(
        rows.contains(&("unexpected_token".to_owned(), 20, 21)),
        "stall row on the block closer: {rows:?}"
    );
}

// —— lints-doc `)` drift tail —————————————————————————————————————————————

/// A `(`-headed parameter followed by `=>`: the name-less `(`-suffix face
/// (parser_impl.dart:2233 OPEN_PAREN arm) fires behind the arrow too — the
/// `missing_identifier` anchors on the `(` and the parameter list's `)` row
/// anchors on `=>` (oracle lints/avoid_final_parameters doc_bad_4).
#[test]
fn lambda_parameter_nameless_paren_face() {
    assert_faces(
        leaked("[1, 4, 6, 8].forEach((final value) => print(value + 2)); // LINT"),
        &[
            ("expected_executable", 0, 1),
            ("expected_executable", 1, 2),
            ("expected_executable", 2, 3),
            ("expected_executable", 4, 5),
            ("expected_executable", 5, 6),
            ("expected_executable", 7, 8),
            ("expected_executable", 8, 9),
            ("expected_executable", 10, 11),
            ("expected_executable", 11, 12),
            ("expected_executable", 12, 13),
            ("missing_identifier", 21, 22),  // at the parameter's `(`
            ("extraneous_modifier", 22, 27), // `final`
            ("expected_token", 35, 37),      // ')' at `=>`
            ("missing_function_body", 55, 56), // at the final `;`
        ],
    );
}
