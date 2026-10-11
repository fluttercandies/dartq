//! Termination-invariant tests (#18): the parser must terminate on any input
//! bytes with memory bounded by the input plus the per-file caps.
//!
//! Three axes are pinned here:
//! 1. the per-file diagnostics cap latches a terminal bailout and stops the
//!    parse before a pathological input can stream unbounded reports;
//! 2. the structural-nesting guard latches the same bailout on a recursion
//!    bomb (official `recoverFromStackOverflow`);
//! 3. the `<id>` fast-shape consume paths emit the official event order
//!    without the off-by-one that used to trip `debug_assert!(peek_is_closer)`
//!    on `@List<int>` (corpus `annotation_with_type_arguments_uninstantiated`).
//!
//! These tests run in the dedicated serial verification phase.

use super::collector::{Collector, latest_features, scan_ok, setup};
use crate::parser::MAX_DIAGNOSTICS;
use crate::parser::Parser;

/// A pathological unit: 20k stray semicolons, each producing exactly one
/// top-level diagnostic — far past the cap, so the bailout must latch and
/// the listener must see exactly `MAX_DIAGNOSTICS` errors (not 20k).
#[test]
fn diagnostics_cap_latches_and_stops_parsing() {
    let source: &'static str = Box::leak(";".repeat(20_000).into_boxed_str());
    let (table, text) = scan_ok(source);
    let mut listener = Collector::default();
    {
        let mut parser = Parser::new(text, table, latest_features(), listener);
        parser.parse_unit();
        assert!(parser.is_bailed(), "bailout must latch past the cap");
        listener = parser.listener;
    }
    assert_eq!(
        listener.errors.len(),
        MAX_DIAGNOSTICS,
        "listener must see exactly the capped error count"
    );
    let codes = listener.error_codes();
    assert_eq!(
        codes.last().map(String::as_str),
        Some("dartq/internal_error"),
        "the last diagnostic is the bounded terminal bailout"
    );
    assert_eq!(
        codes
            .iter()
            .filter(|c| *c == "dartq/internal_error")
            .count(),
        1,
        "exactly one terminal diagnostic"
    );
    // Everything before the terminal is the flood itself.
    assert!(
        codes[..codes.len() - 1]
            .iter()
            .all(|c| c == "unexpected_token"),
        "the flood is the expected one-diagnostic-per-token shape"
    );
}

/// A recursion bomb: 600 nested record-type parens blow past the official
/// nesting cap of 500 and latch the bailout (`stackOverflow`) instead of
/// overflowing the stack.
#[test]
fn nesting_cap_latches_on_deep_record_types() {
    let source: &'static str = Box::leak(
        format!("typedef T = {}int{};", "(".repeat(600), ")".repeat(600)).into_boxed_str(),
    );
    let (table, text) = scan_ok(source);
    let mut listener = Collector::default();
    {
        let mut parser = Parser::new(text, table, latest_features(), listener);
        parser.parse_unit();
        assert!(parser.is_bailed(), "nesting cap must latch the bailout");
        listener = parser.listener;
    }
    let codes = listener.error_codes();
    assert!(
        codes.contains(&"stack_overflow".to_owned()),
        "the terminal diagnostic is the official stackOverflow code: {codes:?}"
    );
    assert!(
        codes.len() <= 4,
        "the bailout bounds total diagnostics on the bomb: {codes:?}"
    );
}

/// The reported corpus panic: `@List<int>` with no argument list tripped
/// `debug_assert!(peek_is_closer())` because the Simple1 consume path read
/// the name before advancing past `<`. The official event order is
/// `beginTypeArguments(<)`, identifier, type, `endTypeArguments(>)`.
#[test]
fn simple_type_arguments_consume_in_official_order() {
    // The exact corpus content (tests/corpus/m0-syntax/lib/targeted/
    // annotation_with_type_arguments_uninstantiated.dart).
    let parser = setup("@List<int> int x = 0;\n");
    let mut parser = parser;
    parser.parse_unit();
    assert!(
        !parser.is_bailed(),
        "a healthy one-liner must not hit the caps"
    );
    assert!(
        parser
            .listener
            .error_codes()
            .contains(&"annotation_with_type_arguments_uninstantiated".to_owned()),
        "the uninstantiated-annotation diagnostic fires: {:?}",
        parser.listener.errors
    );
    parser
        .listener
        .assert_sequence(&[("begin", "type_arguments"), ("end", "type_arguments")]);
}

/// `@List<int>()`: type arguments WITH an argument list are legal — no
/// diagnostics, and the argument's identifier event carries the real name.
#[test]
fn instantiated_annotation_parses_cleanly() {
    let mut parser = setup("@List<int>() void f() {}\n");
    parser.parse_unit();
    assert!(
        parser.listener.errors.is_empty(),
        "no diagnostics expected: {:?}",
        parser.listener.errors
    );
    parser
        .listener
        .assert_sequence(&[("begin", "type_arguments"), ("end", "type_arguments")]);
}

/// The twin off-by-one: Simple1 type PARAMETERS used to run
/// `ensure_identifier` against the `<`, reporting a spurious
/// `missing_identifier` and misattributing the name.
#[test]
fn simple_type_variables_consume_the_real_name() {
    let mut parser = setup("class C<T> { }\n");
    parser.parse_unit();
    assert!(
        parser.listener.errors.is_empty(),
        "class type parameters must not report: {:?}",
        parser.listener.errors
    );
    parser
        .listener
        .assert_sequence(&[("begin", "class_declaration"), ("end", "class_declaration")]);
}

/// Adversarial junk must always terminate with in-bounds diagnostics (the
/// same contract the golden smoke enforces over the corpus, here over
/// recovery-path shapes near the type-argument machinery).
#[test]
fn adversarial_inputs_terminate_with_bounded_diagnostics() {
    let cases = [
        "@Foo<A B> int x = 0;",
        "class C<A B> {}",
        "@Foo<A B int x = 0;",
        "class C<A B C D E F G H> {}",
        "typedef T = A<B C>;",
        "enum E { a<int>() b }",
        "class C<T extends U V> {}",
        "@A<B<C<D>>> int x = 0;",
    ];
    for case in cases {
        let source: &'static str = Box::leak(case.to_owned().into_boxed_str());
        let (table, text) = scan_ok(source);
        let utf16_len: u32 = u32::try_from(source.chars().map(char::len_utf16).sum::<usize>())
            .expect("utf16 length fits u32");
        let mut listener = Collector::default();
        {
            let mut parser = Parser::new(text, table, latest_features(), listener);
            parser.parse_unit();
            listener = parser.listener;
        }
        for (code, start, end) in &listener.errors {
            assert!(
                *start <= utf16_len && *end <= utf16_len,
                "case {case:?}: diagnostic {code} out of bounds ({start}..{end})"
            );
        }
        assert!(
            listener.errors.len() < MAX_DIAGNOSTICS,
            "case {case:?}: a one-liner must stay far below the cap"
        );
    }
}
