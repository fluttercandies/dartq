//! #2804 parser: UV sig-swallow cluster pins. The super-bounded trio
//! (`super_bounded_types{,_error,_error2}_test`, ~3.1k UV rows) fails on
//! ONE derailment each: a local GFT declaration whose own type variables
//! carry bounds (`void Function<Y extends FcovBound<dynamic>>() x685;`) —
//! the statement recovery then swallows the file tail and every later
//! local goes unregistered. Every other shape here is a neighbor the
//! cluster table attributed to the same family: the curried
//! `ReturnType Function() Function()` composition split across lines
//! (x682-x684), the `>>`-compound and nested-`Function` parameter masses
//! (x676-x681), the multi-line parameter list (error_test x144), the
//! nested GFT parameter (error2 x35), and the typedef head/alias-RHS
//! bound groups. The oracle emits no parser rows on any of them (the
//! trio's fail faces are semantic-tier only), so each pin asserts an
//! EMPTY parser diagnostic stream plus the registration evidence (the
//! declaration names and the following statements survive). The `<`
//! disambiguation pin (Constructor-tear-offs ambiguities) locks the
//! instantiation parse: `f(a<b, c>(d))` is one send with a type-argument
//! group, never a relational chain.

use super::collector::{Collector, setup};
use crate::parser::Parser;

/// Parses `source` and returns the parser for event assertions: the
/// parser diagnostic stream must be empty (the oracle's parser face on
/// every shape pinned here is empty).
fn parse_clean(source: &'static str) -> Parser<'static, 'static, 'static, Collector> {
    let mut p = setup(source);
    p.parse_unit();
    assert!(
        p.listener.errors.is_empty(),
        "parser faces must be empty for {source:?}: {:?}",
        p.listener.errors
    );
    p
}

/// Count of a structural event in the log.
fn count(c: &Collector, phase: &'static str, name: &'static str) -> usize {
    c.log
        .iter()
        .filter(|got| got.0 == phase && got.1 == name)
        .count()
}

/// Whether an identifier event fired for `target` inside `needle`'s span
/// in `context`: `needle` disambiguates which occurrence (e.g. `"<Y
/// extends"` for the type variable `Y`), `target` names the token at its
/// position inside the needle.
fn named_in(c: &Collector, source: &str, needle: &str, target: &str, context: &str) -> bool {
    let at = source.find(needle).unwrap_or_else(|| {
        panic!("needle {needle:?} absent from source");
    }) + needle
        .find(target)
        .unwrap_or_else(|| panic!("target {target:?} absent from needle {needle:?}"));
    let (start, end) = (at as u32, (at + target.len()) as u32);
    c.idents
        .iter()
        .any(|(k, s, e, _)| k == context && *s == start && *e == end)
}

// —— curried GFT composition split across lines (x682-x684) ————————————

/// `void Function(C<Function()>)\nFunction()\nx1;` — the composition's
/// second `Function()` head sits on its own line; the annotation is one
/// GFT (return type `void Function(C<Function()>)`, no parameters), the
/// name follows on the third line, and the next statement still parses.
/// Three `function_type` pairs: the type argument's `Function()`, the
/// return type, and the composition (x682-x684 are NOT among the trio's
/// missing UVs — this pins them staying clean).
#[test]
fn split_curried_gft_local_declaration() {
    let source =
        "\nvoid f() {\n  void Function(C<Function()>)\n  Function()\n  x1;\n  int tail = 1;\n}\n";
    let p = parse_clean(source);
    let c = &p.listener;
    assert_eq!(count(c, "begin", "function_type"), 3, "log: {:?}", c.log);
    assert_eq!(count(c, "end", "function_type"), 3, "log: {:?}", c.log);
    assert!(
        named_in(c, source, "x1;", "x1", "LocalVariableDeclaration"),
        "x1 must register as the declared name: {:?}",
        c.idents
    );
    assert!(
        named_in(c, source, "tail = 1", "tail", "LocalVariableDeclaration"),
        "the statement after the split annotation must still parse: {:?}",
        c.idents
    );
}

// —— `>>`-compound parameter closer + composition suffix (x676-x678) ————

/// `void Function(C<FutureOr<int?>>) Function() x2;` — the parameter's
/// type-argument group closes on the split `>>`, then the composition's
/// `Function()` suffix follows. The oracle parses this clean (x678).
#[test]
fn gft_param_compound_closer_composition_suffix() {
    let source =
        "\nvoid f() {\n  void Function(C<FutureOr<int?>>) Function() x2;\n  int tail = 1;\n}\n";
    let p = parse_clean(source);
    let c = &p.listener;
    assert!(
        named_in(c, source, "x2;", "x2", "LocalVariableDeclaration"),
        "x2 must register: {:?}",
        c.idents
    );
    assert!(
        named_in(c, source, "tail = 1", "tail", "LocalVariableDeclaration"),
        "the following statement must still parse: {:?}",
        c.idents
    );
}

// —— nested `Function` heads inside type arguments (x679-x681) ——————————

/// `void Function(C<Function(Function(dynamic))>) Function() x3;` — two
/// GFT heads nested inside the parameter's type arguments, then the
/// composition suffix. The oracle parses this clean (x679-x681).
#[test]
fn gft_param_nested_function_type_args() {
    let source = "\nvoid f() {\n  void Function(C<Function(Function(dynamic))>) Function() x3;\n  int tail = 1;\n}\n";
    let p = parse_clean(source);
    let c = &p.listener;
    assert!(
        named_in(c, source, "x3;", "x3", "LocalVariableDeclaration"),
        "x3 must register: {:?}",
        c.idents
    );
    assert!(
        named_in(c, source, "tail = 1", "tail", "LocalVariableDeclaration"),
        "the following statement must still parse: {:?}",
        c.idents
    );
}

// —— multi-line GFT parameter list (super_bounded_types_error_test x144) —

/// The parameter list itself split across lines with a nested GFT
/// parameter carrying `Function(Function(...))` type arguments, the `)`
/// on its own line, the name after. The oracle parses this clean.
#[test]
fn gft_multiline_parameter_list() {
    let source = "\nvoid f() {\n  void Function(\n    void Function(C<Function(Function(dynamic))>),\n  )\n  x4;\n  int tail = 1;\n}\n";
    let p = parse_clean(source);
    let c = &p.listener;
    assert!(
        named_in(c, source, "x4;", "x4", "LocalVariableDeclaration"),
        "x4 must register: {:?}",
        c.idents
    );
    assert!(
        named_in(c, source, "tail = 1", "tail", "LocalVariableDeclaration"),
        "the following statement must still parse: {:?}",
        c.idents
    );
}

// —— nested GFT parameter (super_bounded_types_error2_test x35) —————————

/// `void Function(void Function(C<Function(dynamic)>)) x5;` — a GFT
/// parameter that is itself a GFT whose parameter's type arguments carry
/// a bare `Function(dynamic)` GFT (the x35 shape
/// `void Function(void Function(CcovCyclicCoBound<Function(Null)>)) x35;`).
/// Oracle-clean. (A trailing `()` after the type would be the deprecated
/// inline-function-type face — a genuinely different shape.)
#[test]
fn gft_param_nested_gft_parameter() {
    let source = "\nvoid f() {\n  void Function(void Function(C<Function(dynamic)>)) x5;\n  int tail = 1;\n}\n";
    let p = parse_clean(source);
    let c = &p.listener;
    assert!(
        named_in(c, source, "x5;", "x5", "LocalVariableDeclaration"),
        "x5 must register: {:?}",
        c.idents
    );
    assert!(
        named_in(c, source, "tail = 1", "tail", "LocalVariableDeclaration"),
        "the following statement must still parse: {:?}",
        c.idents
    );
}

// —— GFT own type variables with bounds (x685, the swallow root) ————————

/// `void Function<Y extends FcovBound<dynamic>>() x6;` — the GFT's own
/// type-variable group carries a bound; the group must scan under
/// declaration semantics (`computeTypeParamOrArg(…, inDeclaration:
/// true)`, type_info_impl.dart:632-638) and the parameter list that
/// follows must parse, leaving the file's tail intact. This is the
/// declaration whose mis-parse swallowed the trio's tails (#2804).
#[test]
fn gft_own_type_variables_bound_local() {
    let source = "\ntypedef FcovBound<X extends num> = X Function();\nvoid f() {\n  void Function<Y extends FcovBound<dynamic>>() x6;\n  int tail = 1;\n}\n";
    let p = parse_clean(source);
    let c = &p.listener;
    assert!(
        named_in(c, source, "<Y extends", "Y", "TypeVariableDeclaration"),
        "Y must register as a type-variable declaration: {:?}",
        c.idents
    );
    assert!(
        named_in(c, source, "x6;", "x6", "LocalVariableDeclaration"),
        "x6 must register: {:?}",
        c.idents
    );
    assert!(
        named_in(c, source, "tail = 1", "tail", "LocalVariableDeclaration"),
        "the tail statement must survive the GFT declaration: {:?}",
        c.idents
    );
}

/// The cyclic-bound variant (`<Y extends FcovCyclicCoBound<Function(
/// Never?)>>()`, error2 x55 — that file's swallow root): a bound whose
/// own type arguments end on the split `>>`.
#[test]
fn gft_own_type_variables_cyclic_bound_local() {
    let source = "\ntypedef F<X extends Function(X)> = Function();\nvoid f() {\n  void Function<Y extends F<Function(dynamic)>>() x7;\n  int tail = 1;\n}\n";
    let p = parse_clean(source);
    let c = &p.listener;
    assert!(
        named_in(c, source, "x7;", "x7", "LocalVariableDeclaration"),
        "x7 must register: {:?}",
        c.idents
    );
    assert!(
        named_in(c, source, "tail = 1", "tail", "LocalVariableDeclaration"),
        "the tail statement must survive: {:?}",
        c.idents
    );
}

// —— typedef head / alias-RHS bound groups (Instantiate-to-bound) ———————

/// The typedef family around the trio: a bound-carrying head group with
/// a function-type alias, a cyclic head bound, a bare-`Function` alias,
/// and a new-style generic-function-type alias whose RHS carries its own
/// type variables (`typedef G<Y> = void Function<Y>();`). The head group
/// only scans under declaration semantics in the OLD-style arm, so the
/// new-style/old-style decision (parser_impl.dart:1487-1518) must route
/// on the head scan with `inDeclaration: false` (computeType, :1457) —
/// the alias RHS likewise (:1518). Oracle-clean on the trio's headers.
#[test]
fn typedef_bound_head_and_alias_rhs() {
    let source = "\ntypedef Fcov<X extends num> = X Function();\ntypedef Fcyc<X extends List<X>> = X Function(X);\ntypedef Funu<X> = Function();\ntypedef G<Y> = void Function<Y>();\nvoid f() {\n  int tail = 1;\n}\n";
    let p = parse_clean(source);
    let c = &p.listener;
    assert_eq!(count(c, "begin", "typedef"), 4, "log: {:?}", c.log);
    assert!(
        named_in(c, source, "G<Y>", "Y", "TypeVariableDeclaration"),
        "G's head type variable must register: {:?}",
        c.idents
    );
    assert!(
        named_in(c, source, "Function<Y>()", "Y", "TypeVariableDeclaration"),
        "the alias RHS GFT's own type variable must register: {:?}",
        c.idents
    );
    assert!(
        named_in(c, source, "tail = 1", "tail", "LocalVariableDeclaration"),
        "the tail statement must survive: {:?}",
        c.idents
    );
}

// —— the trio's exact neighborhood, in file order (x677-x685) ——————————

/// The statement run around the trio's divergence point, in source order:
/// the `FutureOr<...>` compound-closer compositions, the nested
/// `Function(Function(...))` compositions, the split three-line
/// composition (x682), and the bound-carrying GFT type variables (x685,
/// the swallow root). Pre-fix, this neighborhood derailed at x685 and
/// the statement recovery swallowed to EOF (missing UV mass 773-2354,
/// plus EOF `missing_function_parameters`/`missing_identifier`); the
/// whole run must now parse with every name registered and the tail
/// statement intact.
#[test]
fn trio_neighborhood_sequence_parses_clean() {
    let source = "\nvoid f() {\n  void Function(C<FutureOr<dynamic>>) Function() x1;\n  void Function(C<FutureOr<void>>) Function() x2;\n  void Function(C<FutureOr<Object?>>) Function() x3;\n  void Function(C<Function(Function(dynamic))>) Function() x4;\n  void Function(C<Function(Function(dynamic))>)\n  Function()\n  x5;\n  void Function<Y extends C>() x6;\n  int tail = 1;\n}\n";
    let p = parse_clean(source);
    let c = &p.listener;
    for (needle, target) in [
        ("x1;", "x1"),
        ("x2;", "x2"),
        ("x3;", "x3"),
        ("x4;", "x4"),
        ("x5;", "x5"),
        ("x6;", "x6"),
        ("tail = 1", "tail"),
    ] {
        assert!(
            named_in(c, source, needle, target, "LocalVariableDeclaration"),
            "{target} must register: {:?}",
            c.idents
        );
    }
    assert!(
        named_in(c, source, "<Y extends", "Y", "TypeVariableDeclaration"),
        "Y must register as a type-variable declaration: {:?}",
        c.idents
    );
}

// —— `<` disambiguation keeps the argument reads (ambiguities_A01) ——————

/// `f(a<b,\n  c>(d))` — the `(` after the matching `>` makes `<b, c>`
/// method type arguments (`computeMethodTypeArguments` +
/// `_mayFollowTypeArgs`, type_info.dart:485-574): one send whose operand
/// `a` carries a type-argument group, then the invocation of `(d)`. The
/// oracle resolves the type-argument identifiers against the locals
/// (`non_type_as_type_argument` at both names, co19 ambiguities_A01_t01
/// 58:7/61:7 — semantic tier, outside this crate) — so the parser must
/// emit the type-reference identifier events for `b` and `c` (the reads
/// the resolver registers) and NEVER take the relational arm (no binary
/// expressions anywhere in the statement).
#[test]
fn lt_disambiguation_type_arguments_not_relational() {
    let source = "\nString f(a, [b]) => \"$a, $b\";\nmain() {\n  int a = 1;\n  int b = 2;\n  int c = 3;\n  int d = 4;\n  f(a<b,\n      c>(d));\n}\n";
    let mut p = setup(source);
    p.parse_unit();
    let c = &p.listener;
    assert!(
        c.errors.is_empty(),
        "parser faces must be empty for {source:?}: {:?}",
        c.errors
    );
    assert_eq!(
        count(c, "begin", "binary_expression"),
        0,
        "the relational arm must never fire: {:?}",
        c.log
    );
    assert_eq!(
        count(c, "begin", "type_arguments"),
        1,
        "exactly one method type-argument group: {:?}",
        c.log
    );
    assert!(
        named_in(c, source, "a<b,", "b", "TypeReference"),
        "`b` must fire a type-reference identifier event (the read the resolver registers): {:?}",
        c.idents
    );
    assert!(
        named_in(c, source, "c>(d)", "c", "TypeReference"),
        "`c` must fire a type-reference identifier event: {:?}",
        c.idents
    );
    assert!(
        count(c, "handle", "send") >= 2,
        "the outer `f(...)` call and the instantiation invocation `(d)` must both send: {:?}",
        c.log
    );
}
