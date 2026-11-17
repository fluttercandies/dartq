//! Ticket #4342 — the constructor-head `<` group's recovery shape.
//!
//! A member head whose `<` group cannot commit as type variables takes
//! upstream's CONSTRUCTOR-REFERENCE (field) recovery, never the method
//! shape: `ComplexTypeParamOrArgInfo.compute`'s noType branch skips
//! CONSECUTIVE annotations at a member slot (the inner loop,
//! type_info_impl.dart:1359-1362) before the no-type break, so the group
//! swallows `@Foo @Bar()` whole and ends on the last annotation's last
//! token; `computeSimpleWithTypeArguments` keeps the type (the `}`
//! follower passes `looksLikeNameOrEndOfBlock`), the member dispatch sees
//! a non-identifier name candidate, and `recoverFromInvalidMember` parses
//! the member as a FIELD `C<...>` with a synthetic name. The type
//! arguments then re-parse through `parseArguments`, whose own annotation
//! loop (type_info_impl.dart:1438-1450) reports one
//! `annotation_on_type_argument` per annotation — the rows the oracle
//! pins for corpus ErrorParserTest/test_constructorPartial{,2,3}.
//!
//! The bug this pins against: a single-skip annotation handling left the
//! SECOND annotation as the group's follower, the optional-position veto
//! read the `@` as a non-name, and the member fell to the method arm
//! (`C<@Foo @Bar() }` parsed as a constructor with type parameters —
//! `type_parameter_on_constructor` + `missing_method_parameters` +
//! `missing_function_body`, starving every field-shape row).
//!
//! The control keeps the boundary honest: a CLOSED single-argument group
//! (`class D { D<int> }`) is NOT this recovery — the simple1 fast shape
//! vetoes at the `}` follower and the method arm is upstream's own answer
//! (oracle: `missing_method_parameters` / `type_parameter_on_constructor`
//! / `missing_function_body`).

#![allow(clippy::semicolon_if_nothing_returned)]

use dartq_scanner::{ScanConfig, TokenTable};
use dartq_source::FileTable;

use crate::listener::{IdentifierContext, ParserListener};
use crate::message::Message;
use crate::parser::Parser;
use crate::token_id::TokenSpan;

/// Recovery-shape listener: rows `(code, start, end)`, member-shape
/// markers in fire order, and identifier events `(context, start, end)`.
#[derive(Default)]
struct ShapeLog {
    rows: Vec<(String, u32, u32)>,
    events: Vec<String>,
    idents: Vec<(String, u32, u32)>,
}

impl ParserListener for ShapeLog {
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

    fn begin_type_arguments(&mut self, _: TokenSpan) {
        self.events.push("begin_type_arguments".to_owned());
    }

    fn end_type_arguments(&mut self, count: usize, _: TokenSpan, _: TokenSpan) {
        self.events.push(format!("end_type_arguments:{count}"));
    }

    fn begin_type_variables(&mut self, _: TokenSpan) {
        self.events.push("begin_type_variables".to_owned());
    }

    fn end_type_variables(&mut self, _: TokenSpan, _: TokenSpan) {
        self.events.push("end_type_variables".to_owned());
    }

    fn begin_fields(
        &mut self,
        _: crate::listener::DeclarationKind,
        _: Option<TokenSpan>,
        _: Option<TokenSpan>,
        _: Option<TokenSpan>,
        _: Option<TokenSpan>,
        _: Option<TokenSpan>,
        _: Option<TokenSpan>,
        _: Option<TokenSpan>,
        _: TokenSpan,
    ) {
        self.events.push("begin_fields".to_owned());
    }

    fn end_fields(
        &mut self,
        _: crate::listener::DeclarationKind,
        _: Option<TokenSpan>,
        _: Option<TokenSpan>,
        _: Option<TokenSpan>,
        _: Option<TokenSpan>,
        _: Option<TokenSpan>,
        _: Option<TokenSpan>,
        _: Option<TokenSpan>,
        _: usize,
        _: TokenSpan,
        _: TokenSpan,
    ) {
        self.events.push("end_fields".to_owned());
    }

    fn begin_constructor(
        &mut self,
        _: crate::listener::DeclarationKind,
        _: Option<TokenSpan>,
        _: Option<TokenSpan>,
        _: Option<TokenSpan>,
        _: Option<TokenSpan>,
        _: Option<TokenSpan>,
        _: Option<TokenSpan>,
        _: Option<TokenSpan>,
        _: TokenSpan,
        _: Option<&str>,
    ) {
        self.events.push("begin_constructor".to_owned());
    }

    fn begin_method(
        &mut self,
        _: crate::listener::DeclarationKind,
        _: Option<TokenSpan>,
        _: Option<TokenSpan>,
        _: Option<TokenSpan>,
        _: Option<TokenSpan>,
        _: Option<TokenSpan>,
        _: Option<TokenSpan>,
        _: TokenSpan,
        _: Option<&str>,
    ) {
        self.events.push("begin_method".to_owned());
    }
}

fn parse(source: &'static str) -> ShapeLog {
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
    let mut listener = ShapeLog::default();
    let mut parser = Parser::new(
        text,
        table,
        crate::tests::collector::latest_features(),
        &mut listener,
    );
    parser.parse_unit();
    listener
}

/// The oracle's syntactic row set, order-independent (the conformance
/// comparison diffs sorted rows).
fn sorted_rows(log: &ShapeLog) -> Vec<(String, u32, u32)> {
    let mut rows = log.rows.clone();
    rows.sort();
    rows
}

/// The member-shape markers (fields vs constructor/method/type
/// variables) in fire order.
fn shape(log: &ShapeLog) -> Vec<&str> {
    log.events
        .iter()
        .map(String::as_str)
        .filter(|event| {
            matches!(
                *event,
                "begin_fields"
                    | "end_fields"
                    | "begin_constructor"
                    | "begin_method"
                    | "begin_type_variables"
                    | "end_type_variables"
                    | "begin_type_arguments"
            ) | event.starts_with("end_type_arguments:")
        })
        .collect()
}

// ------------------------------------------------------------------
// The family — upstream's constructor-reference (field) recovery
// ------------------------------------------------------------------

/// `class C { C< }` (corpus ErrorParserTest/test_constructorPartial):
/// the empty group recovers as the field type `C<...>` — one synthetic
/// type argument, a synthetic field name at the `}`, and the oracle's
/// three syntactic rows (`expected_token` on the last real token inside
/// the group — the `<` itself; `expected_type_name` and
/// `missing_identifier` on the `}`).
#[test]
fn constructor_partial_bare_lt_recovers_as_field() {
    let log = parse("class C { C< }\n");
    assert_eq!(
        sorted_rows(&log),
        vec![
            ("expected_token".to_owned(), 11, 12),
            ("expected_type_name".to_owned(), 13, 14),
            ("missing_identifier".to_owned(), 13, 14),
        ],
        "rows: {:?}",
        log.rows
    );
    assert_eq!(
        shape(&log),
        vec![
            "begin_fields",
            "begin_type_arguments",
            "end_type_arguments:1",
            "end_fields"
        ],
        "shape: {:?}",
        log.events
    );
}

/// `class C { C<@Foo }` (corpus test_constructorPartial2): one annotation
/// — `parseArguments`' annotation loop reports
/// `annotation_on_type_argument` over the whole `@Foo`, the `;` anchor is
/// the group's last real token (`Foo`), and the field shape holds.
#[test]
fn constructor_partial_single_annotation_recovers_as_field() {
    let log = parse("class C { C<@Foo }\n");
    assert_eq!(
        sorted_rows(&log),
        vec![
            ("annotation_on_type_argument".to_owned(), 12, 16),
            ("expected_token".to_owned(), 13, 16),
            ("expected_type_name".to_owned(), 17, 18),
            ("missing_identifier".to_owned(), 17, 18),
        ],
        "rows: {:?}",
        log.rows
    );
    assert_eq!(
        shape(&log),
        vec![
            "begin_fields",
            "begin_type_arguments",
            "end_type_arguments:1",
            "end_fields"
        ],
        "shape: {:?}",
        log.events
    );
}

/// `class C { C<@Foo @Bar() }` (corpus test_constructorPartial3): the
/// regression case — CONSECUTIVE annotations (the second with argument
/// list) must be skipped by the group scan the way upstream's inner loop
/// does (type_info_impl.dart:1359-1362), so the group ends on the `)`,
/// each annotation reports its own `annotation_on_type_argument`
/// (`@Foo`, then `@Bar()` through its closer), the `;` anchor is the
/// group's last real token (the `)`), and the member NEVER opens the
/// method/constructor shape (no `begin_constructor`, no
/// `begin_type_variables`).
#[test]
fn constructor_partial_consecutive_annotations_recovers_as_field() {
    let log = parse("class C { C<@Foo @Bar() }\n");
    assert_eq!(
        sorted_rows(&log),
        vec![
            ("annotation_on_type_argument".to_owned(), 12, 16),
            ("annotation_on_type_argument".to_owned(), 17, 23),
            ("expected_token".to_owned(), 22, 23),
            ("expected_type_name".to_owned(), 24, 25),
            ("missing_identifier".to_owned(), 24, 25),
        ],
        "rows: {:?}",
        log.rows
    );
    assert_eq!(
        shape(&log),
        vec![
            "begin_fields",
            "begin_type_arguments",
            "end_type_arguments:1",
            "end_fields"
        ],
        "shape: {:?}",
        log.events
    );
    // The synthetic field name: a zero-width FieldDeclaration identifier
    // at the `}` where the name was expected (the engine's
    // `<unnamed>`-field face rides this anchor).
    assert!(
        log.idents
            .contains(&("FieldDeclaration".to_owned(), 24, 24)),
        "synthetic field name ident: {:?}",
        log.idents
    );
}

/// `class C { C<@Foo @Bar() int> x; }` — the skip-then-type shape: the
/// consecutive annotations are skipped in the SCAN phase too, and the
/// following `int` parses as the group's single type member (upstream's
/// re-probe after each skip, :1361). No recovery rows at all — the group
/// is a legal type-argument list modulo the annotations, which
/// `parseArguments` reports.
#[test]
fn consecutive_annotations_then_type_still_parses_the_type() {
    let log = parse("class C { C<@Foo @Bar() int> x; }\n");
    assert_eq!(
        sorted_rows(&log),
        vec![
            ("annotation_on_type_argument".to_owned(), 12, 16),
            ("annotation_on_type_argument".to_owned(), 17, 23),
        ],
        "rows: {:?}",
        log.rows
    );
    assert_eq!(
        shape(&log),
        vec![
            "begin_fields",
            "begin_type_arguments",
            "end_type_arguments:1",
            "end_fields"
        ],
        "shape: {:?}",
        log.events
    );
}

// ------------------------------------------------------------------
// The boundary control — the closed group stays method-shaped
// ------------------------------------------------------------------

/// `class D { D<int> }` (oracle probe, test_constructorPartial's closed
/// sibling): the simple1 fast shape (`<int>` with a real `>` closer)
/// vetoes at the `}` follower (`looksLikeName` fails) and the member
/// dispatch's `<` arm takes the METHOD shape — upstream's own answer
/// (`missing_method_parameters`, `type_parameter_on_constructor` over
/// `<int>`, `missing_function_body`). The consecutive-annotation fix
/// must not widen the field recovery into this face.
#[test]
fn closed_single_argument_group_stays_method_shaped() {
    let log = parse("class D { D<int> }\n");
    assert_eq!(
        sorted_rows(&log),
        vec![
            ("missing_function_body".to_owned(), 17, 18),
            ("missing_method_parameters".to_owned(), 10, 11),
            ("type_parameter_on_constructor".to_owned(), 11, 16),
        ],
        "rows: {:?}",
        log.rows
    );
    assert_eq!(
        shape(&log),
        vec![
            "begin_constructor",
            "begin_type_variables",
            "end_type_variables",
        ],
        "shape: {:?}",
        log.events
    );
}
