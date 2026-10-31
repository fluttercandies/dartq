//! #2571 pins: a `..`-chain is ONE cascade whose sections each carry their
//! own optional assignment.
//!
//! Upstream parses one section per `parseCascadeExpression` call
//! (`parser_impl.dart:7756-7849`) and the AST builder merges every
//! follow-on chunk back into the open `CascadeExpression` at `beginCascade`
//! (ast_builder.dart:207-224 / 1176-1191), so the final tree has a single
//! target and one section per `..`. dartq's downstream layers build their
//! section receivers from these events, so the parser emits the merged
//! shape directly: one `begin_cascade`/`end_cascade` for the whole chain
//! and one `handle_cascade_access` per send-headed section. Without it the
//! second chunk of `a..x = t..y = s` starts from the first section's RHS
//! value row and resolves `y` against `t`'s type (`archive` deflate.dart
//! 258/262/266: `..staticDesc = ...` read against `Uint16List`).
//!
//! Index (`..[i]`) and anonymous-method section heads fire no
//! `handle_cascade_access`; their chunk boundary is what delimits the
//! section for the flow layer's receiver stack, so those heads keep a
//! cascade chunk of their own.

use crate::listener::ParserListener;
use crate::message::Message;
use crate::token_id::TokenSpan;

use super::collector::{latest_features, scan_ok};

/// The cascade surface: event order plus the `..`/`?..` operator offsets.
#[derive(Default)]
struct CascadeLog {
    events: Vec<&'static str>,
    section_operators: Vec<u32>,
    errors: Vec<String>,
}

impl ParserListener for CascadeLog {
    fn handle_recoverable_error(&mut self, message: Message, _: TokenSpan, _: TokenSpan) {
        self.errors.push(message.code.name.to_owned());
    }

    fn begin_cascade(&mut self, _: TokenSpan) {
        self.events.push("begin");
    }

    fn end_cascade(&mut self) {
        self.events.push("end");
    }

    fn handle_cascade_access(&mut self, operator: TokenSpan, _: TokenSpan, _: bool) {
        self.events.push("access");
        self.section_operators.push(operator.range.offset);
    }

    fn handle_assignment_expression(&mut self, _: TokenSpan, _: TokenSpan) {
        self.events.push("assign");
    }
}

fn log(source: &'static str) -> CascadeLog {
    let (table, text) = scan_ok(source);
    let mut log = CascadeLog::default();
    let mut parser = crate::Parser::new(text, table, latest_features(), &mut log);
    parser.parse_unit();
    log
}

/// `a..x = t..y = s` — the deflate shape: two send sections, each with its
/// own assignment, all inside ONE cascade and resolving against the
/// original target. Pre-fix the second `..y` opened a second cascade whose
/// target was the first section's RHS.
#[test]
fn ticket_2571_assignment_sections_share_one_cascade() {
    let c = log("void f() { a..x = t..y = s; }\n");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    assert_eq!(
        c.events,
        ["begin", "access", "assign", "access", "assign", "end"],
        "one cascade, both sections' accesses and assignments in order"
    );
    assert_eq!(
        c.section_operators.len(),
        2,
        "one cascade-access per send-headed section: {:?}",
        c.section_operators
    );
}

/// A three-section chain stays one cascade (`_lDesc..dynamicTree = X
/// ..staticDesc = Y` followed by a third section).
#[test]
fn ticket_2571_three_sections_stay_one_cascade() {
    let c = log("void f() { a..x = t..y = s..z; }\n");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    assert_eq!(c.events.first(), Some(&"begin"));
    assert_eq!(c.events.last(), Some(&"end"));
    assert_eq!(
        c.events.iter().filter(|e| **e == "begin").count(),
        1,
        "single begin_cascade: {:?}",
        c.events
    );
    assert_eq!(
        c.events.iter().filter(|e| **e == "access").count(),
        3,
        "one cascade-access per section: {:?}",
        c.events
    );
}

/// An index section head (`..[i]`) fires no cascade-access event, so it
/// keeps its own chunk — two sends around it stay in the second chunk.
#[test]
fn ticket_2571_index_section_head_keeps_its_chunk() {
    let c = log("void f() { a..x = t..[0] = 2; }\n");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    assert_eq!(
        c.events.iter().filter(|e| **e == "begin").count(),
        2,
        "send chunk + index chunk: {:?}",
        c.events
    );
    assert_eq!(
        c.events.iter().filter(|e| **e == "access").count(),
        1,
        "the index head fires no cascade-access: {:?}",
        c.events
    );
}

/// A send section after an index head joins THAT chunk (the index chunk's
/// own cascade stays open for it).
#[test]
fn ticket_2571_send_section_after_index_joins_the_index_chunk() {
    let c = log("void f() { a..[0]..y = s; }\n");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    assert_eq!(
        c.events.iter().filter(|e| **e == "begin").count(),
        1,
        "index head and the following send section share one cascade: {:?}",
        c.events
    );
    assert_eq!(
        c.events.iter().filter(|e| **e == "access").count(),
        1,
        "only the send section fires cascade-access: {:?}",
        c.events
    );
}

/// `?..` after a previous section reports once and still joins the same
/// cascade (:6665-6669).
#[test]
fn ticket_2571_question_section_reports_once_and_joins() {
    let c = log("void f() { x..a()?..b(); }\n");
    assert_eq!(
        c.errors,
        ["null_aware_cascade_out_of_order"],
        "one report for the late `?..`"
    );
    assert_eq!(
        c.events.iter().filter(|e| **e == "begin").count(),
        1,
        "the `?..` section joins the open cascade: {:?}",
        c.events
    );
    assert_eq!(
        c.events.iter().filter(|e| **e == "access").count(),
        2,
        "{:?}",
        c.events
    );
}
