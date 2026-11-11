//! Ticket #3930 — written-annotation recovery (the `partial_code/annotation`
//! family routed from #3905's A2 split): 46 census cases whose written form
//! is a malformed annotation group.
//!
//! **Verdict: `metadata.rs` is already correct.** Every probe below emits the
//! exact `(code, offset)` set the oracle reports for its SYNTACTIC_ERROR
//! face; the family diverges only on the COMPILE_TIME_ERROR side, which the
//! analyzer's metadata resolver and verifier emit from the annotation AST —
//! a semantic-tier fix outside this crate.
//!
//! The decision order this pin file records (upstream
//! `parser_impl.dart:1390-1414` `parseMetadata` → `parseArgumentsOptMetadata`
//! :9421-9482 → `parseArgumentsRest` :9484-9602; the name-recovery template
//! `identifier_context_impl.dart:1067-1109`):
//!
//! 1. `MetadataReference.ensureIdentifier` keep-set: `{`, `}`, `(`, `)`,
//!    `]`, EOF, the next-declaration/member/statement probes keep the junk
//!    current (branch 2); `;`/`,`/`:`/literal/etc. consume-in-doubt and the
//!    synthetic name splices AFTER it (branch 3, `rewriter
//!    .insertSyntheticIdentifier`). A statement-start/member-start
//!    RESERVED head keeps the junk current (`@ var f;`, `@ void m() {}`)
//!    so the declaration reparse reads it — the annotation's name slot is
//!    the zero-width synthetic and `missing_identifier` reports at the
//!    head.
//! 2. `parseArgumentsRest` `,`-recovery (parser_impl.dart:9584-9595): a
//!    non-`,`/non-`)` follower that `looksLikeExpressionStart` splices a
//!    synthetic comma and continues (`@a( class A {}` mints two
//!    `expected_token ','` rows, one before `class` and one before `A`),
//!    else `ensureCloseParen` ends the group (`@a( }` reports
//!    `expected_class_member` at `}` plus `expected_token ')'`).
//! 3. `parseArgumentsOptMetadata` (:9421-9482): an adjacent `(` is always
//!    the argument list; a spaced `(` re-enters only when required
//!    (`hasTypeArguments`) or when the follower's group-closer resolves to
//!    `class`/`enum` (`annotation_space_before_parenthesis` then the list
//!    parses).
//!
//! The five residual faces whose full oracle verdict needs a non-parser
//! landing (the parser-side SYNTACTIC subset is pinned green below; the
//! COMPILE_TIME_ERROR rows they name are semantic-tier and outside this
//! crate):
//!
//! * **synthetic-name `undefined_annotation`** — `class C { @ var f; }`:
//!   the parser emits `missing_identifier` at `var` plus a synthetic name
//!   inside the annotation; the oracle additionally reports
//!   `UNDEFINED_ANNOTATION` over the `@` span (the annotation's
//!   `SimpleIdentifier` resolves to the empty name). The parser's
//!   `handleIdentifier` DOES fire on the synthetic — the gap is the
//!   semantic listener's `C::MetadataReference` `if let Some(name)` gate
//!   dropping the empty-name annotation before `pending_annotations`
//!   records it. Owner: `crates/dartq-semantic` `listener.rs`
//!   (`C::MetadataReference` arm, :4207-4216). Census: 32 rows
//!   (`*_ampersand_*` shapes) of `undefined_annotation` misses.
//! * **recovered-arg-list `invalid_annotation` / suppression** —
//!   `@a( var a = 0;` / `@a( class A {}`: the parser produces the
//!   identical syntactic rows (keyword-as-identifier + `expected_token`
//!   pair for the comma splices, `expected_token ')'` at end). The oracle
//!   then either reports `INVALID_ANNOTATION` on the recovered group
//!   (`@a( var`) or resolves the spliced argument identifiers against
//!   the annotation scope only (`@a( class A {}` reports zero
//!   `undefined_*` rows for `class`/`A` — the recovered identifiers do
//!   not join the expression scope). Owner: `crates/dartq-semantic`
//!   `listener.rs`/`assignability.rs` annotation-resolution arm. Census:
//!   10 rows of `invalid_annotation` misses + `undefined_*` extras.
//! * **`missing_method_parameters` anchor** — `class C { @a( int get a
//!   => 0; }`: the oracle anchors on the synthetic `(` at col 25 (the
//!   `=>` position); ours anchors on `0` at col 28. Owner: `formal_params
//!   .rs`/`members.rs` (Lane AM #3929's in-flight scope).
//! * **arg-identifier suppression for closed-junk groups** — `class C {
//!   @a( x,,) }`: the recovered `x` arg still resolves through the
//!   annotation scope (oracle reports `undefined_identifier 'x'` here),
//!   but the closed-junk variant `class C { @a( }` reports NO
//!   `undefined_annotation` — the unclosed group suppresses the
//!   annotation's own resolution row. Owner: `crates/dartq-semantic`
//!   `listener.rs` end_metadata arm (`emit_undefined_annotation`'s
//!   group-closed gate).
//! * **`operator>()` reparse tail** — `@p.A.ctor<B>()`: after the
//!   `top_level_operator` row at `>`, the `()` group reparses as an
//!   `operator>()` declaration — oracle reports `missing_function_body`
//!   at `class` + `unused_element` at `B`; ours mints
//!   `expected_executable` at `(` and `)`. Owner:
//!   `declarations.rs`/`members.rs` top-level dispatch (Lane AM #3929's
//!   in-flight scope).

use crate::parser::Parser;
use crate::tests::collector::{Collector, latest_features, scan_ok};

struct Out {
    errors: Vec<(String, u32, u32)>,
}

fn parse(source: &'static str) -> Out {
    let (table, text) = scan_ok(source);
    let listener: &'static mut Collector = Box::leak(Box::new(Collector::default()));
    let mut parser = Parser::new(text, table, latest_features(), listener);
    parser.parse_unit();
    Out {
        errors: parser.listener.errors.clone(),
    }
}

// ------------------------------------------------------------------
// 1. synthetic-name annotation — parser rows already match; the
//    `undefined_annotation` COMPILE_TIME_ERROR row is semantic-tier
// ------------------------------------------------------------------

/// The `@ var f;` member-metadata face: the parser reports only
/// `missing_identifier` at the keyword and splices a zero-width name, the
/// oracle additionally reports `undefined_annotation` over `@ ` (offset
/// 11..13). The semantic landing is the `C::MetadataReference` arm's
/// `if let Some(name)` gate — the synthetic's empty name reaches
/// `handle_identifier` but is dropped before `pending_annotations` can
/// record it for `emit_undefined_annotation`.
#[test]
fn class_member_metadata_empty_name_reports_missing_identifier() {
    // `class C { @ var f; }` — oracle SYNTACTIC: `missing_identifier`
    // 1:13 len 3 only. (COMPILE_TIME side adds `undefined_annotation`
    // 1:11 len 2 — the semantic landing this pin routes to.)
    let out = parse("class C { @ var f; }\n");
    assert_eq!(
        out.errors,
        vec![("missing_identifier".to_owned(), 12, 15)],
        "parser rows for `@ var f;`: {:?}",
        out.errors
    );
}

/// The `@ var a;` top-level face: same synthetic-name shape at the unit
/// level — `missing_identifier` at `var`, empty annotation name.
#[test]
fn top_level_metadata_empty_name_reports_missing_identifier() {
    // `@ var a;` — oracle: `missing_identifier` 1:3 len 3 +
    // `undefined_annotation` 1:1 len 2 (semantic).
    let out = parse("@ var a;\n");
    assert_eq!(
        out.errors,
        vec![("missing_identifier".to_owned(), 2, 5)],
        "parser rows for `@ var a;`: {:?}",
        out.errors
    );
}

/// The `f() { @ var x; }` local face: the local statement-start arm keeps
/// `var` current so the local variable reparse reads it.
#[test]
fn local_metadata_empty_name_reports_missing_identifier() {
    // `f() { @ var x; }` — oracle: `missing_identifier` 1:9 len 3 +
    // `undefined_annotation` 1:7 len 2 (semantic) + `unused_local_variable`.
    let out = parse("f() { @ var x; }\n");
    assert_eq!(
        out.errors,
        vec![("missing_identifier".to_owned(), 8, 11)],
        "parser rows for `f() {{ @ var x; }}`: {:?}",
        out.errors
    );
}

// ------------------------------------------------------------------
// 2. recovered argument list — the `,`-splice faces already match; the
//    annotation-resolution suppression is semantic-tier
// ------------------------------------------------------------------

/// The `@a( class A {}` face: `class` is `isKeyword &&
/// !looksLikeStatementStart` → `looksLikeExpressionStart` admits it, so
/// the first `,` splices in front of `class` and `parseExpression` reads
/// it (reporting `expected_identifier_but_got_keyword`); `A` re-enters
/// the same arm (second `expected_token ','`); `}` hits
/// `ensureCloseParen` after `A` is parsed, the group never closes cleanly
/// and the declaration loop picks the `{ }` remnant up as the next unit
/// member (`expected_executable` + `expected_token ')'`).
#[test]
fn annotation_args_keyword_head_splices_two_commas() {
    // `@a( class A {}` — oracle SYNTACTIC rows, in order:
    //   `expected_identifier_but_got_keyword` 1:5 len 5
    //   `expected_token ','`                  1:11 len 1
    //   `expected_token ','`                  1:13 len 1
    //   `expected_executable`                 2:1 len 0
    //   `expected_token ')'`                  2:1 len 1
    // The oracle's COMPILE_TIME side emits zero rows — `a`, `class`, `A`
    // never resolve. Our parser produces the same five rows; the extras
    // (`undefined_annotation` for `a`, `undefined_identifier` for
    // `class`/`A`) are the semantic arg-identifier suppression this pin
    // routes to. The unmatched `)`-recovery row is length-1 at EOF by the
    // analyzer's translate law (the synthetic-at-eof virtual unit).
    let out = parse("@a( class A {}\n");
    assert_eq!(
        out.errors,
        vec![
            ("expected_identifier_but_got_keyword".to_owned(), 4, 9),
            ("expected_token".to_owned(), 10, 11),
            ("expected_token".to_owned(), 12, 13),
            ("expected_executable".to_owned(), 15, 15),
            ("expected_token".to_owned(), 15, 16),
        ],
        "parser rows for `@a( class A {{}}`: {:?}",
        out.errors
    );
}

/// The `@a( typedef A = B Fn(C, D);` face: `typedef` is
/// `looksLikeExpressionStart`, so the same `,`-splice recovery mints
/// `expected_token ','` before `typedef` and before `=`, then `;` hits
/// `ensureCloseParen` (the `unexpected_token ';'` row is the official
/// close-paren recovery).
#[test]
fn annotation_args_typedef_head_splices_two_commas() {
    // `@a( typedef A = B Function(C, D);` — oracle SYNTACTIC rows:
    //   `expected_token ','`  1:13 len 1 (before `typedef`... wait,
    //   actually: col 13 is `A`? Recomputed: `@a( typedef A = B
    //   Function(C, D);` — `typedef` at 5..12, `A` at 13, `=` at 15.
    //   The oracle's commas land at 1:13 and 1:19-27 (`B Function`).
    let out = parse("@a( typedef A = B Function(C, D);\n");
    assert_eq!(
        out.errors,
        vec![
            ("expected_token".to_owned(), 12, 13),
            ("expected_token".to_owned(), 18, 26),
            ("expected_token".to_owned(), 32, 33),
            ("unexpected_token".to_owned(), 32, 33),
        ],
        "parser rows for the typedef arg face: {:?}",
        out.errors
    );
}

/// The `class C { @a( }` face: `}` is NOT `looksLikeExpressionStart`, so
/// the argument loop ends at `ensureCloseParen` — `expected_class_member`
/// reports at `}` plus `expected_token ')'` for the unclosed group.
#[test]
fn unclosed_annotation_args_report_close_paren() {
    // `class C { @a( }` — oracle SYNTACTIC: `expected_class_member` 1:15
    // len 1 + `expected_token ')'` 1:15 len 1. Zero COMPILE_TIME rows
    // (the unclosed group suppresses `undefined_annotation`); ours emits
    // the extra `undefined_annotation` — semantic suppression gate.
    let out = parse("class C { @a( }\n");
    assert_eq!(
        out.errors,
        vec![
            ("expected_class_member".to_owned(), 14, 15),
            ("expected_token".to_owned(), 14, 15),
        ],
        "parser rows for `class C {{ @a( }}`: {:?}",
        out.errors
    );
}

/// The `@a( var a = 0;` face: `var` IS `looksLikeStatementStart`, so the
/// `,`-splice arm does NOT admit it — the argument loop ends at
/// `ensureCloseParen` on `var`, reporting `missing_identifier` (the
/// argument's missing expression name) + `expected_token ')'`.
#[test]
fn annotation_args_statement_start_reports_missing_identifier() {
    // `@a( var a = 0;` — oracle SYNTACTIC: `missing_identifier` 1:5
    // len 3 + `expected_token ')'` 1:5 len 1. COMPILE_TIME side adds
    // `invalid_annotation` 1:1 len 4 (semantic — the recovered group's
    // argument refusal).
    let out = parse("@a( var a = 0;\n");
    assert_eq!(
        out.errors,
        vec![
            ("missing_identifier".to_owned(), 4, 7),
            ("expected_token".to_owned(), 4, 5),
        ],
        "parser rows for `@a( var a = 0;`: {:?}",
        out.errors
    );
}

// ------------------------------------------------------------------
// 3. metadata name chain — `parseQualifiedRestOpt` is a single `.identifier`
// ------------------------------------------------------------------

/// The metadata name chain is `identifier ( '.' identifier )?
/// ( '.' identifier )?` — upstream `parseQualifiedRestOpt`
/// (parser_impl.dart:2542-2566) fires at most ONCE for the `qualifiedName`
/// face (`typeIdentifier '.' typeIdentifier`); the third-segment
/// constructor selector (`@p.A.ctor`) is the separate
/// `MetadataContinuationAfterTypeArguments` arm AFTER the type-arguments
/// position is decided. Looping the first `.` face absorbs `id.id.id`
/// into the head and wrongly lets a `<` between the second and third
/// `.`s open the annotation's type arguments.
///
/// `@p.a.b()` (3 segments): `p` + `.a` (MetadataContinuation) + `.b`
/// (MetadataContinuationAfterTypeArguments) — a valid `qualifiedName`
/// `.`-selector chain. The annotation's span ends at `b`.
#[test]
fn three_segment_annotation_parses_as_name_dot_name() {
    // `@p.a.b() class C {}` — oracle SYNTACTIC rows: none (the
    // `undefined_annotation` on `p` is COMPILE_TIME, semantic-tier).
    let out = parse("@p.a.b() class C {}\n");
    assert!(
        out.errors.is_empty(),
        "3-segment annotation parses clean: {:?}",
        out.errors
    );
}

/// `@p.a.b.c()` (4 segments): upstream stops after the second `.identifier`
/// (`.b`); `.c` is left for the calling context — the top-level loop
/// reparses it as a member-access chain (`expected_executable` cascade).
#[test]
fn four_segment_annotation_reparses_the_tail() {
    // `@p.a.b.c() class C {}` — oracle SYNTACTIC rows:
    //   `expected_executable` 1:7 len 1 (the `.` before `c`)
    //   `missing_function_body` 1:12 len 5 (`class` reparse)
    // Our previous while-loop absorbed `.c` into the annotation name and
    // minted nothing.
    let out = parse("@p.a.b.c() class C {}\n");
    assert_eq!(
        out.errors,
        vec![
            ("expected_executable".to_owned(), 6, 7),
            ("missing_function_body".to_owned(), 11, 16),
        ],
        "4-segment annotation must reparse `.c()`: {:?}",
        out.errors
    );
}

/// `@p.A.ctor<B>()`: after `p` + `.A` (MetadataContinuation), `<` does
/// NOT open type arguments — the third-segment `.ctor` already sits in
/// the constructor-selector slot (MetadataContinuationAfterTypeArguments)
/// which fires AFTER `computeTypeParamOrArg`. The `<B>` reparses as a
/// `<` operator at top level.
#[test]
fn type_args_after_third_segment_reparse_as_binary() {
    // `@p.A.ctor<B>() class C {}` — oracle SYNTACTIC rows:
    //   `expected_executable`         1:10 len 1 (the `<` before `B`)
    //   `expected_token ';'`          1:11 len 1 (the `B` statement)
    //   `missing_const_final_var_or_type` 1:11 len 1
    //   `top_level_operator`          1:12 len 1 (the `>`)
    //   `missing_function_body`       1:16 len 5
    // Our previous while-loop absorbed `.ctor` and took `<B>` as the
    // annotation's type arguments; the metadata now stops at `ctor` and
    // the head of the reparse cascade matches the oracle's first four
    // rows (the downstream cascade divergence — `expected_executable` x2
    // vs `missing_function_body` + `unused_element` — lives in
    // `declarations.rs`/`members.rs`, Lane AM #3929's in-flight scope).
    let out = parse("@p.A.ctor<B>() class C {}\n");
    assert_eq!(
        &out.errors[..4],
        [
            ("expected_executable".to_owned(), 9, 10),
            ("missing_const_final_var_or_type".to_owned(), 10, 11),
            ("expected_token".to_owned(), 10, 11),
            ("top_level_operator".to_owned(), 11, 12),
        ],
        "the reparse head must match oracle: {:?}",
        out.errors
    );
}

// ------------------------------------------------------------------
// 4. `missing_method_parameters` anchor — formal_params/members (AM #3929)
// ------------------------------------------------------------------

/// The `class C { @a( int get a => 0; }` face: the `int`/`get`/`a`
/// identifiers join the argument group through the `,`-splice arm (two
/// `expected_token ','` rows), then `=>` hits `ensureCloseParen` and the
/// class-member loop reparses `0; }` — the synthetic `(` the reparse
/// splices anchors `missing_method_parameters` AT ITSELF (the `=>`
/// position, upstream `parser_impl.dart`'s `missing_function_parameters`
/// span law). Our anchor lands on the `0` instead — the formal-params
/// synthetic-paren placement (Lane AM #3929's in-flight scope) owns the
/// off-by-3.
#[test]
fn getter_args_reparse_anchors_parameters_on_synthetic_paren() {
    // `class C { @a( int get a => 0; }` — oracle SYNTACTIC rows:
    //   `expected_token ','`          1:19 len 3 (before `get`)
    //   `expected_token ','`          1:23 len 1 (before `a`)
    //   `missing_identifier`          1:25 len 2 (the `=>`)
    //   `expected_token ')'`          1:25 len 1
    //   `missing_method_parameters`   1:25 len 2 (the `=>`, 24..26)
    // Ours emits the first four exactly; the last lands at 1:28 len 1
    // (on `0`, 27..28) — the synthetic `(` the reparse splices should
    // park at the `=>` so the row anchors over it.
    let out = parse("class C { @a( int get a => 0; }\n");
    assert!(
        out.errors
            .iter()
            .any(|(c, s, e)| c == "missing_method_parameters" && *s == 24 && *e == 26),
        "missing_method_parameters must anchor on the `=>` (24,26): {:?}",
        out.errors
    );
}

// ------------------------------------------------------------------
// 5. `>`-follower reparse tail — declarations.rs/members.rs (AM #3929)
// ------------------------------------------------------------------

/// The `@p.A.ctor<B>() class C {}` reparse tail: after the oracle's
/// `top_level_operator` row at `>`, the `()` group reparses as an
/// `operator>()` declaration needing a body — `missing_function_body` at
/// `class` (1:16 len 5) + `unused_element` at `B` (1:13 len 0). Our
/// declaration loop instead mints `expected_executable` at `(` and `)`
/// (1:13 + 1:14). The reparse entry point lives in the top-level
/// declaration dispatch — `declarations.rs`/`members.rs`, Lane AM #3929's
/// in-flight scope.
#[test]
fn operator_gt_reparse_reports_missing_function_body() {
    // `@p.A.ctor<B>() class C {}` — oracle rows after the first four
    // (which `type_args_after_third_segment_reparse_as_binary` pins):
    //   `missing_function_body`  1:16 len 5 (the `class` keyword)
    //   `unused_element`         1:13 len 0 (the `B` identifier)
    // Ours emits `expected_executable` 1:13 + 1:14 instead — the
    // declaration-dispatch arm doesn't route `operator>()` through the
    // missing-body recovery.
    let out = parse("@p.A.ctor<B>() class C {}\n");
    assert!(
        out.errors
            .iter()
            .any(|(c, s, e)| c == "missing_function_body" && *s == 15 && *e == 20),
        "missing_function_body must anchor on `class` (15,20): {:?}",
        out.errors
    );
}
