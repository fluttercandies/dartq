//! #2844 parser: a parameterized bound inside a function type's own
//! type-variables group (`void Function<Y extends List<num>>() x;`). The
//! bound's own `<num>` group shares the `>>` compound closer with the GFT
//! group, so the group parse ends on a split-remainder closer. The official
//! walk pass continues at the end group's successor
//! (`token = typeVariableEndGroups[endGroupIndex]` feeding
//! `parseFormalParametersRequiredOpt`, type_info_impl.dart:704-712); the
//! pre-fix walk navigated by the split-remainder id, which the pass-one
//! mark/reset discards, ran to EOF, and derailed into
//! `missing_function_parameters` + `expected_token` rows at the `>>`. The
//! identical nesting in a class header (`class C<Y extends List<num>>`)
//! always parsed. The group scan itself runs under declaration semantics
//! (`computeTypeParamOrArg(..., inDeclaration: true)`,
//! type_info_impl.dart:631-638). Oracle face on every shape pinned here:
//! zero parser rows; the formal parameter list parses and the surrounding
//! declarations register.

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
/// in `context`.
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

/// `void Function<Y extends List<num>>() x1;` as a local declaration —
/// the ticket's exact corpus shape. The bound `List<num>` closes on the
/// `>>`'s leading `>`, the GFT's own group on the split remainder, and the
/// formal parameter list `()` must parse (the second of the two
/// `formal_parameters` groups — the enclosing `f`'s is the first). The
/// declaration and the tail statement register.
#[test]
fn parameterized_bound_gft_local_declaration() {
    let source = "\nvoid f() {\n  void Function<Y extends List<num>>() x1;\n  int tail = 1;\n}\n";
    let p = parse_clean(source);
    let c = &p.listener;
    assert_eq!(
        count(c, "begin", "formal_parameters"),
        2,
        "the GFT's formal list must parse after its own type variables: {:?}",
        c.log
    );
    assert!(
        named_in(c, source, "<Y extends", "Y", "TypeVariableDeclaration"),
        "Y must register as a type-variable declaration: {:?}",
        c.idents
    );
    assert!(
        named_in(c, source, "x1;", "x1", "LocalVariableDeclaration"),
        "x1 must register as the declared name: {:?}",
        c.idents
    );
    assert!(
        named_in(c, source, "tail = 1", "tail", "LocalVariableDeclaration"),
        "the tail statement must survive the GFT declaration: {:?}",
        c.idents
    );
}

/// The class-header twin: the identical bound nesting in a class header
/// always parsed — parity lock for the type-variable group walk the local
/// GFT shares.
#[test]
fn class_header_twin_parses_clean() {
    let source = "\nclass C<Y extends List<num>> {}\n";
    let p = parse_clean(source);
    let c = &p.listener;
    assert_eq!(
        count(c, "begin", "class_declaration"),
        1,
        "the class header must parse: {:?}",
        c.log
    );
    assert!(
        named_in(c, source, "<Y extends", "Y", "TypeVariableDeclaration"),
        "Y must register as a type-variable declaration: {:?}",
        c.idents
    );
}

/// Top-level field: the same GFT shape in a top-level variable type, with
/// the declaration and its neighbor registering.
#[test]
fn parameterized_bound_gft_top_level_field() {
    let source = "\nvoid Function<Y extends List<num>>() g;\nint after = 0;\n";
    let p = parse_clean(source);
    let c = &p.listener;
    assert_eq!(
        count(c, "begin", "formal_parameters"),
        1,
        "the GFT's formal list must parse: {:?}",
        c.log
    );
    assert!(
        named_in(c, source, "<Y extends", "Y", "TypeVariableDeclaration"),
        "Y must register as a type-variable declaration: {:?}",
        c.idents
    );
    assert!(
        named_in(
            c,
            source,
            "after = 0",
            "after",
            "TopLevelVariableDeclaration"
        ),
        "the following top-level declaration must register: {:?}",
        c.idents
    );
}

/// Alias RHS: `typedef T1 = void Function<Y extends List<num>>();` — the
/// new-style alias body carries the group; the next typedef registers.
#[test]
fn parameterized_bound_gft_alias_rhs() {
    let source =
        "\ntypedef T1 = void Function<Y extends List<num>>();\ntypedef T2 = int Function();\n";
    let p = parse_clean(source);
    let c = &p.listener;
    assert_eq!(
        count(c, "begin", "function_type"),
        2,
        "both alias RHS GFTs must parse: {:?}",
        c.log
    );
    assert!(
        named_in(c, source, "<Y extends", "Y", "TypeVariableDeclaration"),
        "Y must register as a type-variable declaration: {:?}",
        c.idents
    );
    assert!(
        named_in(c, source, "typedef T2", "T2", "TypedefDeclaration"),
        "the second typedef must register its name: {:?}",
        c.idents
    );
}
