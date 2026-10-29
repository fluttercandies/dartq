//! Type annotation grammar tests.

use super::collector::setup;
use crate::parser::Parser;
use crate::type_args::TypeParamOrArg;
use crate::types::ComplexType;
use crate::types::TypeInfo;

#[test]
fn simple_type() {
    let mut p = setup("int x");
    let end = p.ensure_type_not_void_entry();
    assert_eq!(p.cursor.lexeme(end.id), "int");
    assert_eq!(p.listener.log, [("handle", "type")]);
}

#[test]
fn nullable_type_consumes_question() {
    let mut p = setup("int? x");
    let end = p.ensure_type_not_void_entry();
    assert_eq!(p.cursor.lexeme(end.id), "?");
}

#[test]
fn nested_generics_split_gt_gt() {
    let mut p = setup("List<List<int>> x");
    let end = p.ensure_type_not_void_entry();
    // The split leading `>` is a parser-inserted token with a fixed lexeme.
    assert_eq!(p.cursor.lexeme(end.id), ">");
    assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
}

/// A triple-nested group closing on one `>>>` keeps the `Function` suffix
/// on the OUTER type: the inner types close on inner units of the shared
/// compound, so their successor is the split remainder, never the real
/// `Function` after it (the real `drift` corpus shape
/// `Future<List<List<T>>> Function(...)?` — a debug-assert panic and a
/// cascade of bogus syntax errors before the fix).
#[test]
fn triple_nested_group_keeps_the_function_suffix_outside() {
    for source in [
        "Future<List<List<int>>> Function() x",
        "A<B<C<D<int>>>> Function(int)? x",
        "A<B<C<int>>>? x",
    ] {
        let mut p = setup(source);
        let end = p.ensure_type_not_void_entry();
        assert!(
            p.listener.errors.is_empty(),
            "{source}: {:?}",
            p.listener.errors
        );
        assert_ne!(
            p.cursor.lexeme(end.id),
            ">",
            "{source}: the suffix belongs to the type"
        );
    }
}

#[test]
fn void_type() {
    let mut p = setup("void");
    let end = p.parse_type_or_void_entry();
    assert_eq!(p.cursor.lexeme(end.id), "void");
}

#[test]
fn no_type_reports_expected_type_name() {
    let mut p = setup("+");
    let _ = p.ensure_type_not_void_entry();
    assert_eq!(
        p.listener.error_codes(),
        vec!["expected_type_name".to_string()]
    );
}

// --- #135: compound-closer fast path (official split-remainder rule) ----

/// Runs `compute_type` at the cursor and asserts it consumed nothing and
/// emitted no diagnostics (compute is a read-only analysis).
fn compute_shape(source: &'static str, required: bool) -> TypeInfo {
    let mut p = setup(source);
    let before = p.cursor.progress();
    let info = p.compute_type(required, false);
    assert!(
        p.listener.errors.is_empty(),
        "{source:?}: unexpected diagnostics: {:?}",
        p.listener.errors
    );
    assert_eq!(
        p.cursor.progress(),
        before,
        "{source:?}: compute_type must not consume"
    );
    info
}

#[test]
fn compound_closer_required_keeps_fast_shape() {
    // `A<B>>` (scanner-coalesced). Oracle: `simpleTypeWith1ArgumentGtGt` —
    // required position returns the fast shape regardless of what follows,
    // because the split remainder `>` blocks neither the shape nor
    // recovery (type_info.dart:283-286, type_info_impl.dart:1293).
    let info = compute_shape("A<B>> x", true);
    assert!(matches!(info, TypeInfo::SimpleWith1Arg { nullable: false }));
}

#[test]
fn compound_closer_optional_is_no_type() {
    // Oracle: noType. The decision token is the split remainder `=` of
    // `>=` (splitGtEq, type_info_impl.dart:1263), which is never a name —
    // the optional-position `looksLikeName` check cannot pass
    // (type_info.dart:270-292).
    let info = compute_shape("A<B>=c d", false);
    assert!(matches!(info, TypeInfo::NoType));
}

#[test]
fn compound_closer_question_suffix_not_nullable() {
    // Oracle: non-nullable fast shape. `simpleNullableTypeWith1Argument`
    // requires a plain `>` closer (type_info.dart:283); the remainder `>`
    // of `>>` is never a `?`, so the `?` after the compound token stays
    // outside the type.
    let info = compute_shape("A<B>>? x", true);
    assert!(matches!(info, TypeInfo::SimpleWith1Arg { nullable: false }));
}

#[test]
fn plain_closer_question_suffix_nullable() {
    // Regression pin: `A<B>?` keeps the nullable fast shape when a name
    // follows (type_info.dart:277-281, simpleNullableTypeWith1Argument).
    let info = compute_shape("A<B>? c", false);
    assert!(matches!(info, TypeInfo::SimpleWith1Arg { nullable: true }));
}

#[test]
fn builtin_recovery_compound_closer_optional_is_no_type() {
    // Built-in identifier (`required`) + `id<id>=`. Oracle: noType — the
    // recovery `looksLikeName(skip(next).next!)` reads the split remainder
    // `=`, which is never a name (type_info.dart:173 +
    // type_info_impl.dart:1263), so the recovered complex shape is not
    // entered in optional position.
    let info = compute_shape("required<B>=c d", false);
    assert!(matches!(info, TypeInfo::NoType));
}

// --- #167: Complex-path compound closer, optional position --------------

#[test]
fn complex_group_raw_compound_optional_is_no_type() {
    // `Map<int,int>>` in optional position: the group's scan stops on the
    // compound's leading `>` (skipEnd = splitCloser,
    // type_info_impl.dart:1409), so computeSimpleWithTypeArguments decides
    // on the remainder `>` — never a name, never end-of-block
    // (type_info_impl.dart:1044-1046) — and stays noType.
    let info = compute_shape("Map<int, int>>x", false);
    assert!(matches!(info, TypeInfo::NoType));
}

#[test]
fn complex_group_gt_eq_optional_is_no_type() {
    // Same rule with a `>=` compound: the decision token is the remainder
    // `=` (splitGtEq, type_info_impl.dart:1263), so optional position is
    // noType even with a name after the compound.
    let info = compute_shape("Map<int, int>=c d", false);
    assert!(matches!(info, TypeInfo::NoType));
}

#[test]
fn builtin_recovery_complex_group_raw_compound_optional_is_no_type() {
    // The built-in recovery arm reads the split remainder for a full group
    // too: `required<B, C>>` ends on the compound's leading `>`
    // (skipEnd = splitCloser, type_info_impl.dart:1409) and the remainder
    // `>` is never a name (type_info.dart:172-175), so optional position
    // stays noType.
    let info = compute_shape("required<B, C>>c d", false);
    assert!(matches!(info, TypeInfo::NoType));
}

#[test]
fn builtin_recovery_complex_group_raw_compound_required_keeps_type() {
    // Required position enters the recovered complex shape regardless of
    // the follower (type_info.dart:172-175); `end` names the compound —
    // the position of the official split leading `>`.
    let info = compute_shape("required<B, C>>c d", true);
    assert!(matches!(info, TypeInfo::Complex(c) if c.end_raw_compound));
}

// --- #178: type-compute defect cluster --------------------------------------

/// Runs `compute_type` and returns the analyzed shape together with the
/// parser (for lexeme/position assertions over the captured ids).
fn compute_complex(
    source: &'static str,
    required: bool,
) -> (
    ComplexType,
    Parser<'static, 'static, 'static, super::collector::Collector>,
) {
    let mut p = setup(source);
    let info = p.compute_type(required, false);
    match info {
        TypeInfo::Complex(c) => (c, p),
        other => panic!("{source:?}: expected complex type, got {other:?}"),
    }
}

#[test]
fn gft_with_return_type_binds() {
    // Fixes A+B (computeRest probe; complex_from_here): the probe must land
    // ON the `(` one step past `Function` (type_info_impl.dart:1110-1113)
    // and `start` is the type's first token (:562-564), so `void Function()`
    // and `int Function()` compute as generalized function types instead of
    // bare void/simple.
    for source in ["void Function() f;", "int Function() f;"] {
        let (c, p) = compute_complex(source, true);
        assert_eq!(c.gft_has_return_type, Some(true), "{source:?}");
        assert_eq!(c.type_variable_starters.len(), 1, "{source:?}");
        assert_eq!(p.cursor.lexeme(c.end), ")", "{source:?}");
    }
}

#[test]
fn gft_without_return_type_binds() {
    // computeNoTypeGFT (type_info_impl.dart:747) anchors computeRest at
    // beforeStart — one token BEFORE `Function` — so `gftHasReturnType`
    // becomes false via `typeVariableStart != start` (:1136).
    let mut p = setup("x; Function() f;");
    p.cursor.advance();
    p.cursor.advance(); // cursor on `Function`, beforeStart = `;`
    let info = p.compute_type(true, false);
    let TypeInfo::Complex(c) = &info else {
        panic!("expected complex, got {info:?}")
    };
    assert_eq!(c.gft_has_return_type, Some(false));
    assert_eq!(p.cursor.lexeme(c.start), "Function");
    assert_eq!(p.cursor.lexeme(c.end), ")");
}

#[test]
fn gft_parses_as_one_type() {
    // The bound GFT consumes exactly `void Function()` and emits the
    // begin/end pair; the `Function()` tail no longer leaks into member
    // parsing (this is the trace behind the committed `typedef_new_style`
    // empty-errors assertion).
    let mut p = setup("void Function() f;");
    let end = p.ensure_type_not_void_entry();
    assert_eq!(p.cursor.lexeme(end.id), ")");
    assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
    p.listener
        .assert_sequence(&[("begin", "function_type"), ("end", "function_type")]);
}

#[test]
fn skip_type_positions() {
    // SimpleWith1Argument.skipType names the closer (start+3) and the
    // nullable variant the `?` (start+4) — type_info_impl.dart:302/:239.
    let mut p = setup("List<int> x");
    let info = p.compute_type(true, false);
    assert!(matches!(info, TypeInfo::SimpleWith1Arg { nullable: false }));
    assert_eq!(p.cursor.lexeme(info.skip_type(&p)), ">");

    let mut p = setup("List<int>? x");
    let info = p.compute_type(true, false);
    assert!(matches!(info, TypeInfo::SimpleWith1Arg { nullable: true }));
    assert_eq!(p.cursor.lexeme(info.skip_type(&p)), "?");

    // Prefixed keeps its three-token shape (:204).
    let mut p = setup("p.C x");
    let info = p.compute_type(true, false);
    assert!(matches!(info, TypeInfo::Prefixed));
    assert_eq!(p.cursor.lexeme(info.skip_type(&p)), "C");

    // A compound closer still names the combined token: the position of the
    // official split leading `>` (splitGtGt, util.dart:156, #135).
    let mut p = setup("A<B>> x");
    let info = p.compute_type(true, false);
    assert!(matches!(info, TypeInfo::SimpleWith1Arg { nullable: false }));
    assert_eq!(p.cursor.lexeme(info.skip_type(&p)), ">>");
}

#[test]
fn skip_type_void_consumes_type_arguments() {
    // VoidType.skipType (type_info_impl.dart:469-479): `void` followed by
    // type arguments skips the group and lands on its closer (the compound
    // convention names the split leading `>`, #135); plain `void` still
    // lands on the keyword itself.
    let mut p = setup("void x");
    let info = p.compute_type(true, false);
    assert!(matches!(info, TypeInfo::Void));
    assert_eq!(p.cursor.lexeme(info.skip_type(&p)), "void");

    let mut p = setup("void<int> x");
    let info = p.compute_type(true, false);
    assert!(matches!(info, TypeInfo::Void));
    assert_eq!(p.cursor.lexeme(info.skip_type(&p)), ">");

    let mut p = setup("void<List<int>> x");
    let info = p.compute_type(true, false);
    assert!(matches!(info, TypeInfo::Void));
    assert_eq!(p.cursor.lexeme(info.skip_type(&p)), ">>");
}

#[test]
fn prefixed_type_args_optional_position_stays_expression_head() {
    // computePrefixedType's optional-shape veto (type_info_impl.dart
    // :1081-1083, the computeSimpleWithTypeArguments :1044 arm): in a
    // non-required position `id.id<...>` is noType unless a declaration
    // name follows the type end — `c.m<D>(x)` stays an expression head,
    // `p.F<int> v` declares.
    let mut p = setup("c.m<D>(x);");
    let info = p.compute_type(false, false);
    assert!(matches!(info, TypeInfo::NoType), "{info:?}");

    let mut p = setup("p.F<int> v;");
    let info = p.compute_type(false, false);
    assert!(matches!(info, TypeInfo::Complex(_)), "{info:?}");
}

#[test]
fn prefixed_type_with_arguments_computes_group() {
    // The `id . id <...>` group is computed at the continuation token
    // (type_info.dart:319), where `<` sits at cursor+3 — the old at(2)
    // probe never saw it, so required position fell back to the bare
    // prefixed shape and leaked `<T>`.
    let (c, p) = compute_complex("p.C<T> x;", true);
    assert!(matches!(c.type_args, TypeParamOrArg::Simple1 { .. }));
    assert_eq!(p.cursor.lexeme(c.end), ">");

    let mut p = setup("p.C<T> x;");
    let end = p.ensure_type_not_void_entry();
    assert_eq!(p.cursor.lexeme(end.id), ">");
    assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
    p.listener
        .assert_sequence(&[("begin", "type_arguments"), ("end", "type_arguments")]);
}

#[test]
fn extends_clause_scans_following_parameters() {
    // Official `token = typeInfo.skipType(next)` (type_info_impl.dart:1530):
    // the discarded result left the cursor on the supertype, so every
    // following type parameter went through missing-comma recovery.
    for source in [
        "class C<T extends num, U> {}",
        "class C<T extends List<int>, U> {}",
    ] {
        let mut p = setup(source);
        p.parse_unit();
        assert!(
            p.listener.errors.is_empty(),
            "{source:?}: {:?}",
            p.listener.errors
        );
        p.listener
            .assert_sequence(&[("begin", "class_declaration"), ("end", "class_declaration")]);
    }
}

#[test]
fn extends_clause_composite_closer_needs_no_extra_row() {
    // The supertype's argument group and the type-variable group close
    // through one compound: the official split-remainder (parseCloser,
    // type_info_impl.dart:1765) closes the type-variable group with the
    // remainder, so the composite-bound declarations are clean Dart — no
    // expectedAfterButGot('>') at the body.
    for source in [
        "class C<T extends List<int>> {}",
        "class C<T extends Map<int, int>> {}",
    ] {
        let mut p = setup(source);
        p.parse_unit();
        assert!(
            p.listener.errors.is_empty(),
            "{source:?}: {:?}",
            p.listener.errors
        );
        p.listener
            .assert_sequence(&[("begin", "class_declaration"), ("end", "class_declaration")]);
    }
}

#[test]
fn skip_type_len_counts_type_prefix() {
    // The group length covers only the `<...>` tail; the identifier (or
    // `id.id`) prefix must stay in the total (`n += len`).
    for (source, len) in [
        ("List<int> x", 4),
        ("p.C<T> x", 6),
        ("List<List<int>> x", 6),
        ("Map<List<int>, String> x", 9),
    ] {
        let p = setup(source);
        assert_eq!(p.skip_type_len(0), len, "{source:?}");
    }
}

#[test]
fn nested_first_then_more_is_complex() {
    // With the length overwrite (`n = len`) the group scan landed on the
    // inner closer, the type "ended" mid-group, and the optional-position
    // follower check (`,`) turned `Map<List<int>, String> x` into noType.
    let info = compute_shape("Map<List<int>, String> x", false);
    assert!(matches!(info, TypeInfo::Complex(_)));
}

#[test]
fn raw_compound_remainder_blocks_gft() {
    // `A<B, C>>` stops on a raw compound closer: the official parser splits
    // it (splitCloser, type_info_impl.dart:1772) and reads the remainder `>`
    // as the successor, which is never a `Function` intro (#168) — required
    // keeps the plain argumentized type, optional is noType (:1044-1046).
    let (c, p) = compute_complex("A<B, C>> Function() f;", true);
    assert_eq!(c.gft_has_return_type, None);
    assert_eq!(p.cursor.lexeme(c.end), ">>");
    let info = compute_shape("A<B, C>> f", false);
    assert!(matches!(info, TypeInfo::NoType));
}

#[test]
fn shared_compound_closer_keeps_gft_and_nested_type() {
    // Control for the raw-compound rule: when a nested member's group
    // already consumed the compound's leading `>`, the compound's LAST
    // closer closes the outer group and the real successor follows —
    // `List<List<int>> Function()` is still a function type and
    // `List<List<int>> x` still computes in optional position.
    let (c, _p) = compute_complex("List<List<int>> Function() f;", true);
    assert_eq!(c.gft_has_return_type, Some(true));
    let info = compute_shape("List<List<int>> x", false);
    assert!(matches!(info, TypeInfo::Complex(_)));
}

#[test]
fn bound_ending_on_shared_closer_closes_variable_group() {
    // `T extends Comparable<T>>`: the variable's bound ends on the
    // compound `>>` — the shared closer also closes this `<...>` group
    // (official `isCloser` breaks the forward variable scan on the bound's
    // last token, type_info_impl.dart:1539). Without the break the scanner
    // consumed the `>>` remainder as a new variable head and the group
    // never closed — the torture `g007/g019/g031` faces
    // (`expected_token`/`expected_body`/`expected_type_name`).
    for source in [
        "extension Best7<T extends Comparable<T>> on List<T> {}",
        "class C<T extends Comparable<T>> {}",
        "typedef F<T extends Comparable<T>> = T Function(T);",
        "void g<T extends Comparable<T>>() {}",
    ] {
        let mut p = setup(source);
        p.parse_unit();
        assert!(
            p.listener.errors.is_empty(),
            "{source:?}: {:?}",
            p.listener.errors
        );
    }
}
