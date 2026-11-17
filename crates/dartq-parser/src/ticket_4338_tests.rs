//! Ticket #4338 — annotation-metadata parser seams (three faces).
//!
//! 1. Type-variable metadata events: the Complex `parseVariables` fires the
//!    metadata star per variable exactly like the official
//!    `parser.parseMetadataStar(next)` (type_info_impl.dart:1479-1482) —
//!    `f<@a E>() {}` and the extension-type declaration route `<@Unresolved
//!    T>` through the same event set, and the Simple1 arm fires the empty
//!    star pair (`:1204-1207`). The engine-side `end_metadata` drain
//!    (#3937, settled) consumes these; the pins freeze the event shape and
//!    order so a parser regression cannot silently starve it.
//!
//! 2. Generic-metadata anchor: the `annotationWithTypeArgumentsUninstantiated`
//!    report anchors on the LAST CONSUMED token of the annotation's name
//!    chain (`reportRecoverableError(token, …)`, parser_impl.dart:1424) —
//!    the constructor-selector continuation when the period branch ran
//!    (`@p<A>.B.ctor()` anchors on the `B`, corpus
//!    GenericMetadataParserTest/test_typeArguments_after_prefix oracle row
//!    2:7 len 1; `@GenericClass<int, String>.new` on the `new`, corpus
//!    constructor_new_error_test oracle rows 25:28/40:33 len 3), never on
//!    the type-argument closer.
//!
//! 3. Enum-argument recovery (`@a( enum E { v }`, corpus
//!    partial_code/annotation test_annotation_topLevel_leftParen_enum):
//!    the parser rows are the frozen oracle set and the reserved keyword is
//!    used AS the identifier (`Use the keyword as the identifier`,
//!    identifier_context_impl.dart:1100-1106) — the identifier event
//!    carries the real keyword token, no synthetic splice.

#![allow(clippy::semicolon_if_nothing_returned)]

use dartq_scanner::{ScanConfig, TokenTable};
use dartq_source::FileTable;

use crate::listener::{IdentifierContext, ParserListener};
use crate::message::Message;
use crate::parser::Parser;
use crate::token_id::TokenSpan;

/// Metadata-shape listener: recoverable rows `(code, start, end)`,
/// identifier events `(context label, start, end)`, and the metadata /
/// type-variable structural log in fire order.
#[derive(Default)]
struct MetadataLog {
    rows: Vec<(String, u32, u32)>,
    idents: Vec<(String, u32, u32)>,
    events: Vec<String>,
}

impl ParserListener for MetadataLog {
    fn handle_recoverable_error(&mut self, message: Message, start: TokenSpan, end: TokenSpan) {
        self.rows.push((
            message.code.name.to_owned(),
            start.range.offset,
            end.range.offset + end.range.length,
        ));
    }

    fn handle_identifier(&mut self, token: TokenSpan, context: IdentifierContext) {
        self.idents.push((
            format!("{context:?}"),
            token.range.offset,
            token.range.offset + token.range.length,
        ));
    }

    fn begin_metadata_star(&mut self, _: TokenSpan) {
        self.events.push("begin_metadata_star".to_owned());
    }

    fn end_metadata_star(&mut self, count: usize) {
        self.events.push(format!("end_metadata_star:{count}"));
    }

    fn begin_metadata(&mut self, _: TokenSpan) {
        self.events.push("begin_metadata".to_owned());
    }

    fn end_metadata(&mut self, _: TokenSpan, _: Option<TokenSpan>, _: TokenSpan) {
        self.events.push("end_metadata".to_owned());
    }

    fn begin_type_variables(&mut self, _: TokenSpan) {
        self.events.push("begin_type_variables".to_owned());
    }

    fn end_type_variables(&mut self, _: TokenSpan, _: TokenSpan) {
        self.events.push("end_type_variables".to_owned());
    }
}

fn parse(source: &'static str) -> MetadataLog {
    let config = ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let mut files = FileTable::new();
    let file = files.intern(std::path::Path::new("test.dart"));
    let result = dartq_scanner::scan(source, config, file);
    assert!(
        result.diagnostics.is_empty(),
        "scanner diagnostics in {source:?}: {:?}",
        result.diagnostics
    );
    let table: &'static TokenTable = Box::leak(Box::new(result.tokens));
    let text: &'static str = Box::leak(source.to_string().into_boxed_str());
    let mut listener = MetadataLog::default();
    let mut parser = Parser::new(
        text,
        table,
        crate::tests::collector::latest_features(),
        &mut listener,
    );
    parser.parse_unit();
    listener
}

// ------------------------------------------------------------------
// Face 1 — type-variable metadata events
// ------------------------------------------------------------------

/// `f<@a E>() {}` (corpus TopLevelParserTest/
/// test_parseCompilationUnitMember_function_generic_noReturnType_annotated):
/// the Complex variable walk fires the metadata star INSIDE the group, in
/// the official order — `beginTypeVariables`, then the star/annotation
/// pair for `@a`, then the variable's own identifier events.
#[test]
fn type_variable_metadata_events_complex_group() {
    let log = parse("f<@a E>() {}\n");
    // `a` at 3..4 flows through MetadataReference; `E` at 5..6 declares the
    // type variable.
    assert!(
        log.idents.contains(&("MetadataReference".to_owned(), 3, 4)),
        "metadata reference ident for `a`: {:?}",
        log.idents
    );
    assert!(
        log.idents
            .contains(&("TypeVariableDeclaration".to_owned(), 5, 6)),
        "type-variable ident for `E`: {:?}",
        log.idents
    );
    assert_eq!(
        log.events,
        vec![
            // Top-level declaration head star (empty).
            "begin_metadata_star".to_owned(),
            "end_metadata_star:0".to_owned(),
            // The group: variables open, `@a` parses with events, closes.
            "begin_type_variables".to_owned(),
            "begin_metadata_star".to_owned(),
            "begin_metadata".to_owned(),
            "end_metadata".to_owned(),
            "end_metadata_star:1".to_owned(),
            "end_type_variables".to_owned(),
        ],
        "event order for `f<@a E>() {{}}`"
    );
    // Clean shape: no parser rows.
    assert!(log.rows.is_empty(), "rows: {:?}", log.rows);
}

/// `f<E>() {}`: the Simple1 variable arm fires the EMPTY star pair inside
/// the group (`SimpleTypeArgument1.parseVariables`,
/// type_info_impl.dart:1204-1207) — never an annotation event.
#[test]
fn type_variable_metadata_events_simple1_group() {
    let log = parse("f<E>() {}\n");
    assert_eq!(
        log.events,
        vec![
            "begin_metadata_star".to_owned(),
            "end_metadata_star:0".to_owned(),
            "begin_type_variables".to_owned(),
            "begin_metadata_star".to_owned(),
            "end_metadata_star:0".to_owned(),
            "end_type_variables".to_owned(),
        ],
        "event order for `f<E>() {{}}`"
    );
    assert!(
        !log.idents.iter().any(|(c, _, _)| c == "MetadataReference"),
        "no metadata reference on the simple group: {:?}",
        log.idents
    );
}

/// `extension type E<@Unresolved T>(int it) {}` (corpus
/// ExtensionTypeResolutionTest/test_typeParameter_metadata_undefined): the
/// extension-type declaration's variable group fires the same metadata
/// event set (parse_extension_type_declaration → parse_variables).
#[test]
fn extension_type_type_variable_metadata_events() {
    let log = parse("extension type E<@Unresolved T>(int it) {}\n");
    // `Unresolved` at 18..28 through MetadataReference.
    assert!(
        log.idents
            .contains(&("MetadataReference".to_owned(), 18, 28)),
        "metadata reference ident for `Unresolved`: {:?}",
        log.idents
    );
    assert!(
        log.idents
            .contains(&("TypeVariableDeclaration".to_owned(), 29, 30)),
        "type-variable ident for `T`: {:?}",
        log.idents
    );
    assert_eq!(
        log.events,
        vec![
            "begin_metadata_star".to_owned(),
            "end_metadata_star:0".to_owned(),
            "begin_type_variables".to_owned(),
            "begin_metadata_star".to_owned(),
            "begin_metadata".to_owned(),
            "end_metadata".to_owned(),
            "end_metadata_star:1".to_owned(),
            "end_type_variables".to_owned(),
            // The representation formal `(int it)` fires its own empty
            // metadata star (official `parseFormalParameter` head), after
            // the variable group.
            "begin_metadata_star".to_owned(),
            "end_metadata_star:0".to_owned(),
        ],
        "event order for the extension-type group"
    );
    assert!(log.rows.is_empty(), "rows: {:?}", log.rows);
}

/// `Function<@a T>()? x;` (corpus
/// GenericFunctionTypeResolutionTest/test_metadata_typeParameter): the GFT
/// suffix's variable group fires the metadata events at parse time
/// (ComplexType::parse_type → parse_variables per `Function` starter).
#[test]
fn gft_type_variable_metadata_events() {
    let log = parse("Function<@a T>()? x;\n");
    assert!(
        log.idents
            .contains(&("MetadataReference".to_owned(), 10, 11)),
        "metadata reference ident for `a`: {:?}",
        log.idents
    );
    assert!(
        log.idents
            .contains(&("TypeVariableDeclaration".to_owned(), 12, 13)),
        "type-variable ident for `T`: {:?}",
        log.idents
    );
    assert!(
        log.events.windows(6).any(|w| {
            w[0] == "begin_type_variables"
                && w[1] == "begin_metadata_star"
                && w[2] == "begin_metadata"
                && w[3] == "end_metadata"
                && w[4] == "end_metadata_star:1"
                && w[5] == "end_type_variables"
        }),
        "metadata pair nested in the GFT variable group: {:?}",
        log.events
    );
    assert!(log.rows.is_empty(), "rows: {:?}", log.rows);
}

// ------------------------------------------------------------------
// Face 2 — generic-metadata uninstantiated anchor
// ------------------------------------------------------------------

/// `@p<A>.B.ctor() class C {}` (corpus
/// GenericMetadataParserTest/test_typeArguments_after_prefix, oracle rows
/// 2:7 / 2:8 / 2:16): the uninstantiated report anchors on the
/// constructor-selector continuation `B` (offset 6), never on the `>`
/// closer (offset 4) — and the parser row set is exactly the oracle's
/// three syntactic rows.
#[test]
fn uninstantiated_anchor_after_constructor_continuation() {
    let log = parse("@p<A>.B.ctor() class C {}\n");
    assert_eq!(
        log.rows,
        vec![
            (
                "annotation_with_type_arguments_uninstantiated".to_owned(),
                6,
                7
            ),
            ("expected_executable".to_owned(), 7, 8),
            ("missing_function_body".to_owned(), 15, 20),
        ],
        "parser rows for `@p<A>.B.ctor() class C {{}}`"
    );
}

/// `@GenericClass<int, String>.new` continuation face (corpus
/// constructor_new_error_test, oracle anchor 25:28 len 3 = the `new`): the
/// `new`-after-dot rewrite keeps the real token, so the anchor is the
/// three-character `new` (8..11), not the `>` closer (6..7).
#[test]
fn uninstantiated_anchor_after_new_continuation() {
    let log = parse("@A<int>.new x;\n");
    assert!(
        log.rows.contains(&(
            "annotation_with_type_arguments_uninstantiated".to_owned(),
            8,
            11
        )),
        "anchor must be the `new` continuation (8,11): {:?}",
        log.rows
    );
    assert!(
        !log.rows.contains(&(
            "annotation_with_type_arguments_uninstantiated".to_owned(),
            6,
            7
        )),
        "the `>` closer must not carry the row: {:?}",
        log.rows
    );
}

/// `@p.A.ctor<B>() class C {}` (corpus
/// test_typeArguments_after_constructorName): no uninstantiated row at all
/// (the type arguments sit after the third name segment, so
/// `hasTypeArguments` is false upstream) and the synthetic `<unnamed>`
/// name the cascade splices for the `()` group carries the `(` offset
/// (12, zero width) — the oracle's `unused_element` anchor 2:13 len 0.
#[test]
fn constructor_name_type_arguments_cascade_synthetic_anchor() {
    let log = parse("@p.A.ctor<B>() class C {}\n");
    assert!(
        !log.rows
            .iter()
            .any(|(c, _, _)| c == "annotation_with_type_arguments_uninstantiated"),
        "no uninstantiated row after the third segment: {:?}",
        log.rows
    );
    assert!(
        log.idents
            .contains(&("TopLevelFunctionDeclaration".to_owned(), 12, 12)),
        "the `<unnamed>` synthetic name must carry the `(` offset (12,12): {:?}",
        log.idents
    );
}

// ------------------------------------------------------------------
// Face 3 — enum-argument recovery
// ------------------------------------------------------------------

/// `@a( enum E { v }` (corpus partial_code/annotation
/// test_annotation_topLevel_leftParen_enum, oracle rows 2:5/2:10/2:12/
/// 3:1×2): the parser rows are the frozen oracle set, and the reserved
/// keyword argument is USED AS the identifier — the identifier event
/// carries the real `enum` token (4..8), the upstream
/// `expectedIdentifierButGotKeyword` + keyword-as-identifier pair
/// (identifier_context_impl.dart:1100-1106). No synthetic splice.
#[test]
fn enum_argument_recovery_rows_and_keyword_as_identifier() {
    let log = parse("@a( enum E { v }\n");
    assert_eq!(
        log.rows,
        vec![
            ("expected_identifier_but_got_keyword".to_owned(), 4, 8),
            ("expected_token".to_owned(), 9, 10),
            ("expected_token".to_owned(), 11, 12),
            // EOF rows sit after the trailing newline: zero-width at 17,
            // length-1 law at 17 — the oracle's 3:1 pair.
            ("expected_executable".to_owned(), 17, 17),
            ("expected_token".to_owned(), 17, 18),
        ],
        "parser rows for `@a( enum E {{ v }}`"
    );
    assert!(
        log.idents.iter().any(|(_, s, e)| *s == 4 && *e == 8),
        "the `enum` keyword is used AS the identifier (4,8): {:?}",
        log.idents
    );
}
