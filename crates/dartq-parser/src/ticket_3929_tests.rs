//! Ticket #3929 — parser packet 2: the typedef-head `noType` family root and
//! the written-clause type-argument evidence for the #3847u attribution.
//!
//! 1. **Typedef head `noType`** (`type_args.rs`, `recovered_group_end`). The
//!    official recovery tail of `ComplexTypeParamOrArgInfo.compute`
//!    (type_info_impl.dart:1408-1423) probes `splitCloser(next)` /
//!    `splitCloser(next.next)` and, when neither matches, splices a synthetic
//!    `>` in FRONT of `next` (so the synthetic's follower IS `next`).
//!    `next` is the failed member's own first token in both break shapes
//!    (:1370-1374 no-type break, :1404 failed member test) — probing one
//!    token later put the group's end past the junk, which flipped
//!    `computeSimpleWithTypeArguments`' optional-position veto
//!    (type_info_impl.dart:1056-1060) and sent `typedef F< = ...` down the
//!    old-style branch (a `expected_type_name` + a top-level member cascade).
//!
//! 2. **Written clause arguments** (`class H = Object with p.M<int>;`): the
//!    parser event face the semantic collector pairs into
//!    `Container::with_clause_args`. This pin is the attribution evidence for
//!    #3847u — the parser fires the prefixed head, the continuation, the
//!    qualified event and the argument group in the official order, so a
//!    dropped argument list is a collector-side (semantic) issue.

use crate::parser::Parser;
use crate::tests::collector::{Collector, latest_features, scan_ok};

struct Out {
    errors: Vec<(String, u32, u32)>,
    idents: Vec<(String, u32, u32, usize)>,
    log: Vec<(&'static str, &'static str)>,
}

fn parse(source: &'static str) -> Out {
    let (table, text) = scan_ok(source);
    let listener: &'static mut Collector = Box::leak(Box::new(Collector::default()));
    let mut parser = Parser::new(text, table, latest_features(), listener);
    parser.parse_unit();
    Out {
        errors: parser.listener.errors.clone(),
        idents: parser.listener.idents.clone(),
        log: parser.listener.log.clone(),
    }
}

// ------------------------------------------------------------------
// 1. typedef head `noType` — the recovered group ends in front of the junk
// ------------------------------------------------------------------

#[test]
fn typedef_head_recovered_group_reports_only_the_missing_name() {
    // `typedef F< = int Function(int);` — oracle (`dart analyze
    // --format=machine`, SDK 3.13.7): exactly one row,
    // `missing_identifier` at the `=` (2:12, offset 11). The recovered `<`
    // group must end in front of the `=` so the head type stays `noType` and
    // the typedef takes the new-style `skip.isA(EQ)` arm; an end past the
    // junk re-reads `int` as the follower and cascades into a top-level
    // reparse (`expected_executable` / `missing_const_final_var_or_type`).
    let out = parse("typedef F< = int Function(int);\n");
    assert_eq!(
        out.errors,
        vec![("missing_identifier".to_owned(), 11, 12)],
        "expected the single oracle row, got {:?}",
        out.errors
    );
}

#[test]
fn typedef_head_well_formed_group_is_unchanged() {
    // Control: a well-formed head keeps its type-variable group and reports
    // nothing (oracle: zero rows for `typedef G<T> = List<T>;`).
    let out = parse("typedef G<T> = List<T>;\n");
    assert!(out.errors.is_empty(), "got {:?}", out.errors);
}

// ------------------------------------------------------------------
// 2. written clause arguments (#3847u attribution evidence)
// ------------------------------------------------------------------

#[test]
fn qualified_with_clause_face_emits_its_type_arguments() {
    // `class H = Object with p.M<int>;` — offsets: `p` 22..23, `.` 23,
    // `M` 24..25, `<` 25, `int` 26..29, `>` 29.
    //
    // The parser must fire, in order: the prefixed head identifier, the
    // period's qualified event, the continuation identifier, then the
    // argument group (`begin`/`handle identifier`/`end`), and finally the
    // type event for the clause face. This is the exact event face the
    // semantic collector's `with_clause_args` capture consumes; if the group
    // events are all present here, a dropped argument list downstream is a
    // collector-side defect, not a parser one.
    let out = parse("class H = Object with p.M<int>;\n");
    assert!(out.errors.is_empty(), "got {:?}", out.errors);
    for want in [
        ("PrefixedTypeReference", 22u32, 23u32),
        ("TypeReferenceContinuation", 24, 25),
        ("TypeReference", 26, 29),
    ] {
        assert!(
            out.idents
                .iter()
                .any(|(ctx, s, e, _)| ctx == want.0 && *s == want.1 && *e == want.2),
            "missing identifier event {want:?}: {:?}",
            out.idents
        );
    }
    // The argument group opens and closes exactly once, and the clause face's
    // type event follows it.
    let opens = out
        .log
        .iter()
        .filter(|e| **e == ("begin", "type_arguments"))
        .count();
    let closes = out
        .log
        .iter()
        .filter(|e| **e == ("end", "type_arguments"))
        .count();
    assert_eq!((opens, closes), (1, 1), "log: {:?}", out.log);
    let group_close = out
        .log
        .iter()
        .position(|e| *e == ("end", "type_arguments"))
        .expect("group close");
    assert!(
        out.log[group_close..]
            .iter()
            .any(|e| *e == ("handle", "type")),
        "the clause type event must follow its argument group: {:?}",
        out.log
    );
}

// ------------------------------------------------------------------
// routing: the semantic-tier row of the `<...>`-opening formal family
// ------------------------------------------------------------------

/// Routing pin for the `invalid_inline_function_type` row that the four
/// `test_typeAlias_37733` / `test_parseGenericTypeAlias_*` cases need
/// alongside their parser rows.
///
/// `INVALID_INLINE_FUNCTION_TYPE` is emitted by the analyzer's AST verifier,
/// not by the parser: the fasta parser only builds the generic function type
/// whose parameter is itself an inline function type. The message is
/// "Inline function types can't be used for parameters in a generic function
/// type." (`COMPILE_TIME_ERROR`, oracle `dart analyze --format=machine` on
/// `typedef T=Function(<S>());` at 2:20, i.e. the `<`).
///
/// The engine now emits the full oracle pair — the syntactic
/// `missing_identifier` plus the `invalid_inline_function_type` row — both
/// at the `<` (offset 19, len 1).
#[test]
fn generic_function_type_parameter_inline_function_type_routing() {
    // Oracle row set for this input (SDK 3.13.7):
    //   MISSING_IDENTIFIER        2:20 len 1
    //   INVALID_INLINE_FUNCTION_TYPE 2:20 len 1
    let out = parse("typedef T=Function(<S>());\n");
    assert_eq!(
        out.errors,
        vec![
            ("invalid_inline_function_type".to_owned(), 19, 20),
            ("missing_identifier".to_owned(), 19, 20),
        ],
        "parser rows for the `<...>`-opening formal: {:?}",
        out.errors
    );
}
