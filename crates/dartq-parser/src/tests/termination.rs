//! Termination-invariant tests (#18): the parser must terminate on any input
//! bytes with memory bounded by the input plus the per-file caps.
//!
//! Four axes are pinned here:
//! 1. the per-file diagnostics cap latches a terminal bailout and stops the
//!    parse before a pathological input can stream unbounded reports;
//! 2. the statement/expression nesting guards latch the same bailout on a
//!    recursion bomb (official `recoverFromStackOverflow`), while the
//!    type-descent path (record types, type arguments/variables) carries NO
//!    observable cap — the official parser counters only
//!    statement/expression recursion, so oracle-parity depths there must
//!    parse to completion; it is bounded only by the far-above-parity
//!    `TYPE_DEPTH_CRASH_GUARD` (structured bail, never a panic);
//! 3. the `<id>` fast-shape consume paths emit the official event order
//!    without the off-by-one that used to trip `debug_assert!(peek_is_closer)`
//!    on `@List<int>` (corpus `annotation_with_type_arguments_uninstantiated`);
//! 4. the constructor-initializer loop's turn cap latches the same bailout
//!    when its no-progress guard's blind spot is occupied (a parked real
//!    stream whose inserted queue never drains — zero diagnostic burn, so no
//!    other cap would ever fire).
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
        let mut parser = Parser::new(text, table, latest_features(), &mut listener);
        parser.parse_unit();
        assert!(parser.is_bailed(), "bailout must latch past the cap");
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

/// The 600-deep record-type case (manual/diagnostic/stack_overflow/1): the
/// official parser has NO depth counter on the type-descent path
/// (`computeType` → `parseRecordType` → `parseRecordTypeField` → `computeType`
/// recurses unguarded), so the oracle iterates and emits one
/// `recordTypeOnePositionalFieldNoTrailingComma` row per nesting level at that
/// level's closing paren (columns 616→1215) and never reports
/// `stackOverflow`. dartq must reproduce that row set 1:1: no observable cap
/// on this path — the `TYPE_DEPTH_CRASH_GUARD` sits far above any
/// oracle-reachable depth and is a stack-safety crash guard, not a limit.
#[test]
fn deep_record_type_matches_oracle_per_level_rows() {
    let source: &'static str = Box::leak(
        format!("typedef T = {}int{};", "(".repeat(600), ")".repeat(600)).into_boxed_str(),
    );
    let (table, text) = scan_ok(source);
    let mut listener = Collector::default();
    {
        let mut parser = Parser::new(text, table, latest_features(), &mut listener);
        parser.parse_unit();
        assert!(
            !parser.is_bailed(),
            "the type-descent path has no observable cap: the 600-deep \
             record type must parse to completion"
        );
    }
    let code = "record_type_one_positional_no_trailing_comma";
    assert_eq!(
        listener.errors.len(),
        600,
        "the row set is exactly the oracle's — no stack_overflow, no other noise: {:?}",
        listener.error_codes().iter().skip(599).take(3).collect::<Vec<_>>()
    );
    assert!(
        listener
            .errors
            .iter()
            .all(|(c, _, _)| c == code),
        "every row is the per-level record-type diagnostic"
    );
    // Reports unwind innermost-first: the innermost `)` sits at UTF-16 offset
    // 615 (`typedef T = ` is 12 chars plus 600 open parens), the outermost at
    // 1214 — the oracle's 1:616 → 1:1215 columns, one-based.
    assert_eq!(listener.errors[0].1, 615, "innermost level's closer first");
    assert_eq!(listener.errors[599].1, 1214, "outermost level's closer last");
}

/// Deep-but-valid nested type arguments (`A<A<...>>`, 600 levels) are VALID
/// Dart the oracle parses silently; the type-argument descent sites share the
/// record type's crash-guard posture — no observable cap, zero diagnostics,
/// no false-positive `stack_overflow` on valid code.
#[test]
fn deep_valid_type_arguments_produce_zero_diagnostics() {
    let source: &'static str = Box::leak(
        format!("{}int{} x;", "A<".repeat(600), ">".repeat(600)).into_boxed_str(),
    );
    let (table, text) = scan_ok(source);
    let mut listener = Collector::default();
    {
        let mut parser = Parser::new(text, table, latest_features(), &mut listener);
        parser.parse_unit();
        assert!(
            !parser.is_bailed(),
            "valid deep type arguments must not trip any cap"
        );
    }
    assert!(
        listener.errors.is_empty(),
        "the oracle is silent on this shape: {:?}",
        listener.errors.iter().take(5).collect::<Vec<_>>()
    );
}

/// A written-out-operator chain: every `xor` link fires the entry-floor
/// recovery acceptance, so the walk must iterate in place. A per-link
/// re-entry of the walk (native tail recursion, invisible to the depth
/// guard — the entry runs outside `enter_depth`) grows the native stack
/// linearly with the chain and overflows the process on ~40k links; the
/// parse must instead complete with the caps bounding the diagnostics.
#[test]
fn written_out_operator_chain_terminates() {
    let mut source = String::from("void f() {\n  var x = a");
    for _ in 0..50_000 {
        source.push_str(" xor 1");
    }
    source.push_str(";\n}\n");
    let source: &'static str = Box::leak(source.into_boxed_str());
    let (table, text) = scan_ok(source);
    let mut listener = Collector::default();
    {
        let mut parser = Parser::new(text, table, latest_features(), &mut listener);
        parser.parse_unit();
    }
    assert!(
        listener.errors.len() <= MAX_DIAGNOSTICS,
        "the caps bound the per-link flood: {}",
        listener.errors.len()
    );
}

/// The reported corpus panic: `@List<int>` with no argument list tripped
/// `debug_assert!(peek_is_closer())` because the Simple1 consume path read
/// the name before advancing past `<`. The official event order is
/// `beginTypeArguments(<)`, identifier, type, `endTypeArguments(>)`.
#[test]
fn simple_type_arguments_consume_in_official_order() {
    // The exact corpus content (the retired m0-syntax targeted fixture
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

/// The insert-then-drain backstop (#1855): recovery's synthetic-drain loops
/// share [`crate::cursor::TokenCursor::drain_to`], which must terminate when
/// the target id is unreachable — a queue-reordering regression would
/// otherwise spin forever on the EOF-pinned `advance` with zero diagnostic
/// burn (the diagnostic caps never see a drain loop). The pre-backstop shape
/// (`while advance() != id {}`) hangs on exactly this queue state: queued
/// entries drain, then the walk pins at EOF with the target still ahead.
#[test]
fn synthetic_drain_terminates_at_eof_on_unreachable_target() {
    let (table, text) = scan_ok("var x = 1;");
    let mut cursor = crate::cursor::TokenCursor::new(text, table);
    // Two queued synthetics: draining to the first consumes exactly it and
    // leaves the second current (the correct-path posture is unchanged).
    let first = cursor.insert_synthetic(dartq_scanner::TokenKind::Identifier, 0);
    let second = cursor.insert_synthetic(dartq_scanner::TokenKind::Identifier, 0);
    cursor.drain_to(first);
    assert_eq!(
        cursor.peek_id(),
        second,
        "the drain consumed the target only"
    );
    // An unreachable target: the drain pops the remaining queue entry, walks
    // the real stream to EOF, and returns there instead of spinning.
    cursor.drain_to(table.len() as crate::token_id::TokenId + 5);
    assert!(cursor.at_eof(), "the drain exits pinned at EOF");
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
            let mut parser = Parser::new(text, table, latest_features(), &mut listener);
            parser.parse_unit();
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

/// A malformed super-initializer must not stall the initializer loop: the
/// `expectedAfterButGot('(')` recovery queues its synthetic parens AHEAD of
/// the `super` head, so the expression re-read returns without consuming it
/// and the missing-comma continuation re-parsed the same head forever —
/// each turn re-reporting and re-queueing tokens (unbounded diagnostics,
/// GBs of RSS on the SDK's own illegal_initializer_test). The no-progress
/// guard breaks the stall and hands the tokens to the member loop
/// (tests/language/variable/illegal_initializer_test.dart,
/// co19 Generative_Constructors/initializers_t07,
/// invalid_super_in_initializer test_constructor_name_is_keyword).
#[test]
fn initializer_loop_terminates_on_stalled_super_recovery() {
    let cases = [
        "class A { A(); A.foo(); } class B extends A { B.c1() : super.foo; }",
        "class C { C() : super.() {} }",
        "class C { C() : super.const() {} }",
        "class A {} class B extends A { B() : this.foo; B.z() : this; }",
    ];
    for case in cases {
        let source: &'static str = Box::leak(case.to_owned().into_boxed_str());
        let (table, text) = scan_ok(source);
        let mut listener = Collector::default();
        {
            let mut parser = Parser::new(text, table, latest_features(), &mut listener);
            parser.parse_unit();
            assert!(!parser.is_bailed(), "case {case:?}: no cap trip expected");
        }
        assert!(
            !listener.errors.is_empty(),
            "case {case:?}: the malformed initializer still reports"
        );
        assert!(
            listener.errors.len() < 20,
            "case {case:?}: a stalled recovery must not flood ({})",
            listener.errors.len()
        );
    }
}

/// The turn cap over the initializer loop's no-progress guard (#2731
/// residual, hang-audit #2729): the guard only breaks when the real stream
/// is parked AND the inserted-token queue has drained; a state whose every
/// turn re-queues as much as it consumes keeps the queue permanently
/// non-empty, and the repeated same-anchor reports dedupe to zero diagnostic
/// burn — no cap would ever latch and the loop would spin forever. No real
/// input shape reaches that state (the corpora hang-audit found none), so
/// the belt is driven directly at the module level: a crafted inserted-token
/// queue feeds 2500 well-formed synthetic entries (`foo = bar,`) through the
/// loop while the real stream stays pinned — every turn consumes exactly the
/// four queued tokens, the queue never drains before the cap, and the turn
/// cap (members.rs `INITIALIZER_TURN_CAP`, 4× the nesting cap) must latch
/// the terminal bailout instead of running to the end of the queue. The
/// queue leads with a synthetic `:` because the entry gate peeks the
/// queue-first stream.
#[test]
fn initializer_turn_cap_latches_on_parked_queue() {
    let (table, text) = scan_ok(": x");
    let mut listener = Collector::default();
    {
        let mut parser = Parser::new(text, table, latest_features(), &mut listener);
        parser.cursor.insert_synthetic(dartq_scanner::TokenKind::Colon, 0);
        for _ in 0..2500 {
            let at = 3; // past the real `: x`, at the EOF pin
            parser.cursor.insert_synthetic(dartq_scanner::TokenKind::Identifier, at);
            parser.cursor.insert_synthetic(dartq_scanner::TokenKind::Eq, at);
            parser.cursor.insert_synthetic(dartq_scanner::TokenKind::Identifier, at);
            parser.cursor.insert_synthetic(dartq_scanner::TokenKind::Comma, at);
        }
        parser.parse_initializers_opt();
        assert!(
            parser.is_bailed(),
            "the turn cap must latch the bailout past 2000 turns"
        );
    }
    assert_eq!(
        listener.errors.len(),
        1,
        "the crafted entries are well formed: only the terminal reports: {:?}",
        listener.errors
    );
    assert_eq!(
        listener.error_codes(),
        vec!["dartq/internal_error".to_owned()],
        "the one diagnostic is the bounded terminal bailout"
    );
}
