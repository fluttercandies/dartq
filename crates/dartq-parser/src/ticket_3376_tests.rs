//! Ticket #3376 — arg/param trailing-comma family: parser-tier contract.
//!
//! `tests/language/unsorted/arg_param_trailing_comma_test.dart` yields 211
//! conformance variants. The `topBad*` syntax-error variants open or split a
//! formal-parameter list with a stray separator (`void f(,)`, `void f(, a)`,
//! `void f(a,, b)`, ...). These pins freeze what the PARSER tier owns for that
//! family, and what the downstream tiers consume:
//!
//! * the recoverable row (`missing_identifier`) anchors on the offending
//!   separator, matching the oracle rows (`missing_identifier` at the `,`,
//!   `]`, `}` in `arg_param_trailing_comma_test_N.dart`), and
//! * the recovered list still counts every slot: a separator with nothing
//!   before it is a synthetic-named REQUIRED positional formal, so
//!   `void topBadEmpty(,)` presents ONE positional and `void topBadStart(, a)`
//!   presents TWO. The oracle's `not_enough_positional_arguments` at the
//!   0-argument call in `testBadCalls` agrees ("1 positional argument
//!   expected by 'topBadEmpty'" / "2 ... by 'topBadStart'").
//!
//! The empty slot is therefore a real parameter slot at this tier. The
//! observed deficit in the conformance family (`dartq` reports "1 positional
//! argument expected" for `topBadStart`, one fewer than the oracle's 2) is
//! `dartq-semantic`'s `end_formal_parameter` dropping the name-less formal
//! (`collector.rs`, `capture_name(...)` -> `return`), not the parser. The
//! `dead_code` rows the same oracle file emits are the flow-tier
//! `unnecessary_type_check` verdict marking the `if (topx is! fx) throw`
//! branch dead — again not a parser row. Both downstream consumers are
//! separate crates; these pins keep the parser side of that contract stable.

use std::collections::BTreeSet;

use crate::listener::{MemberKind, ParserListener, Token};
use crate::message::Message;
use crate::parser::Parser;
use crate::tests::collector::latest_features;
use crate::token_id::TokenSpan;
use dartq_scanner::ScanConfig;

#[derive(Default)]
struct ParamProbe {
    rows: BTreeSet<(String, u32, u32)>,
    formal_counts: Vec<usize>,
}

impl ParserListener for ParamProbe {
    fn handle_recoverable_error(&mut self, message: Message, start: TokenSpan, end: TokenSpan) {
        self.rows.insert((
            message.code.name.to_owned(),
            start.range.offset,
            end.range.offset + end.range.length,
        ));
    }

    fn end_formal_parameters(
        &mut self,
        count: usize,
        _begin: Token,
        _end: Token,
        _kind: MemberKind,
    ) {
        self.formal_counts.push(count);
    }
}

/// Parses `source` and returns the recoverable rows plus every formal-list
/// count (`end_formal_parameters` in fire order).
fn parse(source: &'static str) -> (BTreeSet<(String, u32, u32)>, Vec<usize>) {
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
    let mut probe = ParamProbe::default();
    let mut parser = Parser::new(
        source,
        Box::leak(Box::new(result.tokens)),
        latest_features(),
        &mut probe,
    );
    parser.parse_unit();
    (
        parser.listener.rows.clone(),
        parser.listener.formal_counts.clone(),
    )
}

fn rows_set(expected: &[(&str, u32, u32)]) -> BTreeSet<(String, u32, u32)> {
    expected
        .iter()
        .map(|(c, s, e)| ((*c).to_owned(), *s, *e))
        .collect()
}

#[test]
fn top_bad_empty_leading_comma_counts_one_slot() {
    // `void topBadEmpty(,)` — oracle: `missing_identifier` on the leading
    // `,` (the empty slot's name slot) and ONE positional formal, so the
    // later `topBadEmpty()` call is one short.
    let (rows, counts) = parse("void topBadEmpty(,) {}\n");
    assert_eq!(rows, rows_set(&[("missing_identifier", 17, 18)]), "rows");
    assert_eq!(counts, vec![1], "formal-list count");
}

#[test]
fn top_bad_start_leading_comma_keeps_both_slots() {
    // `void topBadStart(, a)` — oracle "2 positional arguments expected by
    // 'topBadStart'": the leading empty slot AND `a` both count.
    let (rows, counts) = parse("void topBadStart(, a) {}\n");
    assert_eq!(rows, rows_set(&[("missing_identifier", 17, 18)]), "rows");
    assert_eq!(counts, vec![2], "formal-list count");
}

#[test]
fn top_bad_middle_stray_comma_counts_three_slots() {
    // `void topBadMiddle(a,, b)` — oracle "3 positional arguments expected":
    // the empty middle slot counts too; the row anchors on the SECOND comma.
    let (rows, counts) = parse("void topBadMiddle(a,, b) {}\n");
    assert_eq!(rows, rows_set(&[("missing_identifier", 20, 21)]), "rows");
    assert_eq!(counts, vec![3], "formal-list count");
}

#[test]
fn top_bad_pos_empty_leading_comma_then_empty_group() {
    // `void topBadPosEmpty(,[])` — the leading empty slot plus the empty
    // `[]` group: the first slot is the only counted positional (the `[]`
    // group is empty), and the leading `,` still reports.
    let (rows, counts) = parse("void topBadPosEmpty(,[]) {}\n");
    assert!(
        rows.contains(&("missing_identifier".to_owned(), 20, 21)),
        "leading-comma row missing: {rows:?}"
    );
    assert_eq!(counts, vec![1], "formal-list count");
}

#[test]
fn top_bad_nam_empty_leading_comma_then_empty_group() {
    // `void topBadNamEmpty(,{})` — same shape with the empty named group.
    let (rows, counts) = parse("void topBadNamEmpty(,{}) {}\n");
    assert!(
        rows.contains(&("missing_identifier".to_owned(), 20, 21)),
        "leading-comma row missing: {rows:?}"
    );
    assert_eq!(counts, vec![1], "formal-list count");
}

#[test]
fn valid_trailing_commas_stay_clean_and_count_one() {
    // Negative control: the family's VALID sibling forms. A trailing comma
    // is not a separator-with-empty-slot; it must not report and must keep
    // the single declared formal.
    let (rows, counts) = parse("void topx(x,) {}\n");
    assert!(rows.is_empty(), "unexpected rows: {rows:?}");
    assert_eq!(counts, vec![1], "formal-list count");

    let (rows, counts) = parse("typedef fx(x, );\n");
    assert!(rows.is_empty(), "unexpected rows: {rows:?}");
    assert_eq!(counts, vec![1], "typedef formal-list count");
}
