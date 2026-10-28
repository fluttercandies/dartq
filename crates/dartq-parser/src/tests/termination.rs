//! Termination-invariant tests (#18): the parser must terminate on any input
//! bytes with memory bounded by the input plus the per-file caps.
//!
//! Four axes are pinned here:
//! 1. the per-file diagnostics cap latches a terminal bailout and stops the
//!    parse before a pathological input can stream unbounded reports;
//! 2. the statement/expression nesting guards are LOCAL recover-and-continue
//!    fuses (official `recoverFromStackOverflow`: one `stackOverflow` report
//!    per breach site, synthetic-`;`/identifier splice, skip to the enclosing
//!    closer, then keep parsing — the post-fuse cascade rides the ordinary
//!    recovery faces), while the
//!    type-descent paths (record types, type arguments/variables, the
//!    formal-parameter descent, the pattern descent) carry NO observable
//!    cap — the official parser counters only statement/expression
//!    recursion, so oracle-parity depths there must parse to completion;
//!    they are bounded only by the far-above-parity
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

/// Deep nested function-typed formal parameters (`void f(void g(void g(…`
/// 600 levels) are VALID Dart the official parser descends with no counter:
/// the only depth counters in `parser_impl.dart` (3.13.7 checkout) are
/// `statementDepth` (:6686) and `expressionDepth` (:6915), and the
/// `parseFormalParametersRest` (:1916) → `parseFormalParameter` (:2029) →
/// `parseFormalParametersRequiredOpt` (:2307) chain between them carries
/// none — so the oracle parses this shape silently. dartq must match: no
/// observable cap on the formal-parameter descent (bounded only by
/// `TYPE_DEPTH_CRASH_GUARD`), zero diagnostics, no false
/// `stack_overflow`.
#[test]
fn deep_nested_function_typed_formals_parse_silently() {
    let source: &'static str = Box::leak(
        format!(
            "void f({}{}) {{}}",
            "void g(".repeat(600),
            ")".repeat(600)
        )
        .into_boxed_str(),
    );
    // `parse_formal_parameter` carries the largest frame on any descent path,
    // so the pin parses on a dedicated wide-stack thread: the assertion
    // targets the guard semantics (no observable cap below
    // `TYPE_DEPTH_CRASH_GUARD`), not the libtest thread's default stack
    // budget — the same input parses on the ordinary engine entry in a
    // process with a proportional stack.
    let (bailed, errors) = std::thread::Builder::new()
        .stack_size(256 * 1024 * 1024)
        .spawn(move || {
            let (table, text) = scan_ok(source);
            let mut listener = Collector::default();
            let bailed = {
                let mut parser = Parser::new(text, table, latest_features(), &mut listener);
                parser.parse_unit();
                parser.is_bailed()
            };
            (bailed, listener.errors)
        })
        .expect("spawn the wide-stack pin thread")
        .join()
        .expect("the pin thread must not panic");
    assert!(
        !bailed,
        "the formal-parameter descent has no observable cap: the \
         600-deep function-typed formals must parse to completion: {errors:?}"
    );
    assert!(
        errors.is_empty(),
        "the oracle is silent on this valid shape: {:?}",
        errors.iter().take(5).collect::<Vec<_>>()
    );
}

/// Deep parenthesized patterns (`final ((((…a…)))) = x;`, 600 levels) are
/// VALID Dart the official parser descends with no counter: `parsePattern`
/// (:11549) → `parsePrimaryPattern` (:11658) →
/// `parseParenthesizedPatternOrRecordPattern` (:12143) → `parsePattern`
/// recurses between `statementDepth`/`expressionDepth` crossings (only the
/// innermost operand would ride `parseExpression`, and a bare identifier
/// does not even do that) — the oracle parses this shape silently. dartq
/// must match: no observable cap on the pattern descent (bounded only by
/// `TYPE_DEPTH_CRASH_GUARD`), zero diagnostics, no false `stack_overflow`.
#[test]
fn deep_parenthesized_patterns_parse_silently() {
    let source: &'static str = Box::leak(
        format!(
            "void f() {{ final {}a{} = x; }}",
            "(".repeat(600),
            ")".repeat(600)
        )
        .into_boxed_str(),
    );
    let (table, text) = scan_ok(source);
    let mut listener = Collector::default();
    {
        let mut parser = Parser::new(text, table, latest_features(), &mut listener);
        parser.parse_unit();
        assert!(
            !parser.is_bailed(),
            "the pattern descent has no observable cap: the 600-deep \
             parenthesized pattern must parse to completion"
        );
    }
    assert!(
        listener.errors.is_empty(),
        "the oracle is silent on this valid shape: {:?}",
        listener.errors.iter().take(5).collect::<Vec<_>>()
    );
}

/// The corpus cascade (oracle case
/// `language/syntax/deep_nesting_statement_test_01`): 10,000 nested
/// `if (x == 0) {` lines inside `main()`, one `}` run closing everything.
/// The statement-depth fuse trips on the 502nd nested statement entry —
/// line 263's block `{` (oracle row `stack_overflow` 263:15) — and, being a
/// LOCAL guard, the parse continues: the open frames unwind through their
/// closers and the leftover `}` run reaches the unit member loop, one
/// `expected_executable` per stray closer, then the trailing top-level
/// declaration `print("survived!");` rides its own faces. The oracle row
/// set is 9,753 ERROR rows: 1 `stack_overflow` + 9,750
/// `expected_executable` + `missing_identifier` (10015:9, the string
/// literal) + `missing_function_body` (10015:21). The per-file diagnostics
/// cap (10,000) must NOT clip the cascade.
#[test]
fn deep_statement_corpus_cascade_matches_oracle_rows() {
    // The corpus shape, regenerated (ASCII throughout, so UTF-16 offsets
    // equal byte offsets; only the line/column geometry matters for the
    // anchors, and every diagnostic-bearing line is byte-identical to the
    // oracle input's geometry).
    let mut source = String::new();
    for _ in 0..8 {
        source.push_str("// Deeply nested statements must not crash the parser.\n");
    }
    source.push('\n'); // line 9
    source.push_str("var x = 0;\n"); // line 10
    source.push('\n'); // line 11
    source.push_str("main() {\n"); // line 12
    source.push_str("  if (x == 0) {\n"); // line 13
    for _ in 14..=10012 {
        source.push_str("  if (x == 0) { //# 01: continued\n");
    }
    // Line 10013 is indented like the rest of the body: the 9,999 closers
    // sit at columns 3..10001 (the first stray cascade anchor is 10013:254
    // — 251 closers consumed by the unwind: 250 block frames + main's body).
    source.push_str("  ");
    source.push_str(&"}".repeat(9_999)); // line 10013
    source.push_str(" //# 01: continued\n");
    source.push_str("  }\n"); // line 10014
    source.push_str("  print(\"survived!\");\n"); // line 10015
    source.push_str("}\n"); // line 10016
    let source: &'static str = Box::leak(source.into_boxed_str());

    // 1-based (line, col) -> 0-based UTF-16 offset.
    fn at(source: &str, line: u32, col: u32) -> u32 {
        let mut offset = 0u32;
        for (i, l) in source.lines().enumerate() {
            if i as u32 + 1 == line {
                return offset + col - 1;
            }
            offset += l.len() as u32 + 1;
        }
        panic!("line {line} beyond the input");
    }

    let (table, text) = scan_ok(source);
    let mut listener = Collector::default();
    {
        let mut parser = Parser::new(text, table, latest_features(), &mut listener);
        parser.parse_unit();
        assert!(
            !parser.is_bailed(),
            "9,753 rows sit below the 10,000 diagnostics cap: the cascade \
             must not be clipped"
        );
    }
    assert_eq!(listener.errors.len(), 9_753, "the oracle's row count");
    let codes = listener.error_codes();
    assert_eq!(
        codes.iter().filter(|c| *c == "stack_overflow").count(),
        1,
        "one fuse report per breach site"
    );
    assert_eq!(
        codes.iter().filter(|c| *c == "expected_executable").count(),
        9_750,
        "one row per stray closer"
    );
    assert_eq!(
        codes.iter().filter(|c| *c == "missing_identifier").count(),
        1
    );
    assert_eq!(
        codes
            .iter()
            .filter(|c| *c == "missing_function_body")
            .count(),
        1
    );
    let so: Vec<u32> = listener
        .errors
        .iter()
        .filter(|(c, _, _)| c == "stack_overflow")
        .map(|(_, s, _)| *s)
        .collect();
    assert_eq!(
        so,
        vec![at(source, 263, 15)],
        "the fuse report anchors on line 263's block `{{`"
    );
    let exec: Vec<u32> = listener
        .errors
        .iter()
        .filter(|(c, _, _)| c == "expected_executable")
        .map(|(_, s, _)| *s)
        .collect();
    assert_eq!(
        exec[0],
        at(source, 10013, 254),
        "the first stray closer is the oracle's first cascade anchor"
    );
    assert_eq!(
        *exec.last().unwrap(),
        at(source, 10016, 1),
        "the last stray closer is the oracle's last cascade anchor"
    );
    for (code, start, end) in &listener.errors {
        if code == "missing_identifier" {
            assert_eq!(*start, at(source, 10015, 9));
            assert_eq!(*end, at(source, 10015, 9) + 11, "the string literal span");
        }
        if code == "missing_function_body" {
            assert_eq!(*start, at(source, 10015, 21));
        }
    }
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
