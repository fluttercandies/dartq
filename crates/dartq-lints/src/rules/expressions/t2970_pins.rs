use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

/// The #2970 cluster pins: `null_check_on_nullable_type_parameter`,
/// `unnecessary_null_checks`, and `avoid_futureor_void` oracle shapes
/// through the production bridge (the semantic face mounted where the
/// rule consults resolution). Every pin cites the conformance row it
/// mirrors (`conformance/expected.tsv`, extracted from the pinned
/// `dart analyze` 3.13.7 oracle), so the conformance diff and the pin
/// assert the same verdict.

// —— null_check_on_nullable_type_parameter ————————————————————————————
// Upstream Dart SDK source, pkg/linter .../null_check_on_nullable_type_parameter.dart:59-72
// (visitPostfixExpression): report when the operand's type is a nullable
// type parameter AND the expected type from getExpectedType promotes to
// the same non-null type. A parameter with no bound, a `dynamic` bound,
// or a nullable bound is potentially nullable; `extends Object` is not.

/// `test_expectedSameTypeVariable_arrow#0` —
/// `INFO|…|2:18|1` — the arrow body's expected type is the same bare
/// parameter.
#[test]
fn ncvp_bare_parameter_arrow_reports() {
    let f = Fixture::parse("T f<T>(T? p) => p!;\n");
    let lints = f.run_semantic(&one_rule("null_check_on_nullable_type_parameter"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The null check operator shouldn't be used on a variable whose type is a potentially nullable type parameter."
    );
    let bang = tok(&f.source, &f.tokens, "!", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, bang));
    assert_eq!(lints[0].range.end - lints[0].range.start, 1);
}

/// `test_expectedSameTypeVariable_block#0` — `INFO|…|2:26|1` — the block
/// return reads the written `T` return.
#[test]
fn ncvp_block_return_reports() {
    let f = Fixture::parse("T f<T>(T? p) { return p!; }\n");
    let lints = f.run_semantic(&one_rule("null_check_on_nullable_type_parameter"));
    assert_eq!(lints.len(), 1, "{lints:?}");
}

/// `test_expectedSameTypeVariable_assignment#0` — `INFO|…|4:8|1` — an
/// assignment to a `T`-typed local expects the promoted parameter.
#[test]
fn ncvp_assignment_reports() {
    let f = Fixture::parse("void f<T>(T? p) {\n  T t;\n  t = p!;\n}\n");
    let lints = f.run_semantic(&one_rule("null_check_on_nullable_type_parameter"));
    assert_eq!(lints.len(), 1, "{lints:?}");
}

/// `test_expectedSameTypeVariable_assignmentInDeclaration#0` —
/// `INFO|…|2:25|1` — the declaration initializer carries the same
/// expected type.
#[test]
fn ncvp_assignment_in_declaration_reports() {
    let f = Fixture::parse("void f<T>(T? p) { T t = p!; }\n");
    let lints = f.run_semantic(&one_rule("null_check_on_nullable_type_parameter"));
    assert_eq!(lints.len(), 1, "{lints:?}");
}

/// `test_typeParameterBoundToObject#0` — `-` — `extends Object` is
/// non-nullable, so the operand is not a nullable type parameter.
#[test]
fn ncvp_bound_to_object_stays_silent() {
    let f = Fixture::parse("T f<T extends Object>(T? p) => p!;\n");
    let lints = f.run_semantic(&one_rule("null_check_on_nullable_type_parameter"));
    assert!(lints.is_empty(), "{lints:?}");
}

/// `test_typeParameterBoundToDynamic#0` — `INFO|…|2:34|1` — a `dynamic`
/// bound leaves the parameter potentially nullable.
#[test]
fn ncvp_bound_to_dynamic_reports() {
    let f = Fixture::parse("T f<T extends dynamic>(T? p) => p!;\n");
    let lints = f.run_semantic(&one_rule("null_check_on_nullable_type_parameter"));
    assert_eq!(lints.len(), 1, "{lints:?}");
}

/// `test_typeParameterBoundToNullableObject#0` — `INFO|…|2:34|1` — a
/// nullable bound (`Object?`) keeps the parameter nullable.
#[test]
fn ncvp_bound_to_nullable_object_reports() {
    let f = Fixture::parse("T f<T extends Object?>(T? p) => p!;\n");
    let lints = f.run_semantic(&one_rule("null_check_on_nullable_type_parameter"));
    assert_eq!(lints.len(), 1, "{lints:?}");
}

// —— the collection / yield / await expected-type faces ——————————————
// Upstream `getExpectedType` (pkg/linter .../unnecessary_null_checks.dart
// :107-129 list/set/map members, :59-72 yield, :30-37 withAwait). The
// `null_check_on_nullable_type_parameter` verdict reads the same faces
// through the shared helpers, so each pin below is the element/argument
// type the oracle resolves.

/// `test_inListLiteral#0` — `INFO|…|2:25|1` — the list literal's element
/// type is the written return's first type argument.
#[test]
fn ncvp_list_literal_member_reports() {
    let f = Fixture::parse("\nList<T> f<T>(T? p) => [p!];\n");
    let lints = f.run_semantic(&one_rule("null_check_on_nullable_type_parameter"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let bang = tok(&f.source, &f.tokens, "!", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, bang));
    assert_eq!(lints[0].range.end - lints[0].range.start, 1);
}

/// `test_inSetLiteral#0` — `INFO|…|2:24|1` — a `{…}` with no entry reads
/// the set element slot.
#[test]
fn ncvp_set_literal_member_reports() {
    let f = Fixture::parse("\nSet<T> f<T>(T? p) => {p!};\n");
    let lints = f.run_semantic(&one_rule("null_check_on_nullable_type_parameter"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let bang = tok(&f.source, &f.tokens, "!", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, bang));
}

/// `test_inMapLiteralKey#0` — `INFO|…|2:32|1` — the entry's key side
/// reads the map's `K`.
#[test]
fn ncvp_map_literal_key_reports() {
    let f = Fixture::parse("\nMap<T, String> f<T>(T? p) => {p!: ''};\n");
    let lints = f.run_semantic(&one_rule("null_check_on_nullable_type_parameter"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let bang = tok(&f.source, &f.tokens, "!", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, bang));
}

/// `test_inMapLiteralValue#0` — `INFO|…|2:36|1` — the entry's value side
/// reads the map's `V`.
#[test]
fn ncvp_map_literal_value_reports() {
    let f = Fixture::parse("\nMap<String, T> f<T>(T? p) => {'': p!};\n");
    let lints = f.run_semantic(&one_rule("null_check_on_nullable_type_parameter"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let bang = tok(&f.source, &f.tokens, "!", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, bang));
}

/// `test_inYieldSync#0` / `test_inYieldAsync#0` — `INFO|…|2:38|1` and
/// `INFO|…|2:37|1` — `yield` reads the generator's `Iterable`/`Stream`
/// argument.
#[test]
fn ncvp_yield_faces_report() {
    let f = Fixture::parse("\nIterable<T> f<T>(T? p) sync* {yield p!;}\n");
    let lints = f.run_semantic(&one_rule("null_check_on_nullable_type_parameter"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let bang = tok(&f.source, &f.tokens, "!", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, bang));

    let f = Fixture::parse("\nStream<T> f<T>(T? p) async* {yield p!;}\n");
    let lints = f.run_semantic(&one_rule("null_check_on_nullable_type_parameter"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let bang = tok(&f.source, &f.tokens, "!", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, bang));
}

/// `test_typeParameterExtendsNullableObject_inAwaited#0` —
/// `INFO|…|2:54|1` — the `await` wrapper reads the awaited `T`, not the
/// `Future<T>` return.
#[test]
fn ncvp_awaited_return_reports() {
    let f = Fixture::parse("\nFuture<T> f<T extends Object?>(T? p) async => await p!;\n");
    let lints = f.run_semantic(&one_rule("null_check_on_nullable_type_parameter"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let bang = tok(&f.source, &f.tokens, "!", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, bang));
}

/// `test_typeParameterExtendsNullableObject_inAwaitedInList#0` —
/// `INFO|…|2:61|1` — the awaited `Future<List<T>>` flattens once, then the
/// list element slot answers `T`.
#[test]
fn ncvp_awaited_list_member_reports() {
    let f = Fixture::parse("\nFuture<List<T>> f<T extends Object?>(T? p) async => await [p!];\n");
    let lints = f.run_semantic(&one_rule("null_check_on_nullable_type_parameter"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let bang = tok(&f.source, &f.tokens, "!", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, bang));
}

// —— unnecessary_null_checks ——————————————————————————————————————————
// Upstream Dart SDK source, pkg/linter .../unnecessary_null_checks.dart:26-161
// (getExpectedType): report when the expected type at the `!` already
// admits null — an async return reads only the first type argument of
// `Future`/`FutureOr`, collection members read the element type, `yield`
// reads the generator's `Iterable`/`Stream` argument, and call arguments
// read the formal's declared type (`Future.value`/`Completer.complete`
// fall out: their formals live in the SDK, outside the indexed set).

/// `test_returnAsync_futureOfNullable_await#0` — `INFO|…|2:40|1` — the
/// `await` wrapper is transparent; the async return admits null through
/// `Future<int?>`.
#[test]
fn unc_async_await_return_reports() {
    let f = Fixture::parse("Future<int?> f(int? p) async => await p!;\n");
    let lints = f.run_semantic(&one_rule("unnecessary_null_checks"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Unnecessary use of a null check ('!').");
    let bang = tok(&f.source, &f.tokens, "!", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, bang));
    assert_eq!(lints[0].range.end - lints[0].range.start, 1);
}

/// `test_returnAsync_dynamic#0` — `-` — a bare `dynamic` async return
/// does not admit null through a type argument.
#[test]
fn unc_async_dynamic_return_stays_silent() {
    let f = Fixture::parse("dynamic f(int? p) async => p!;\n");
    let lints = f.run_semantic(&one_rule("unnecessary_null_checks"));
    assert!(lints.is_empty(), "{lints:?}");
}

/// `test_returnAsync_futureOfNullable_typedef#0` — `INFO|…|3:21|1` — the
/// alias resolves one level to `Future<int?>` before the async unwrap.
#[test]
fn unc_async_typedef_alias_reports() {
    let f = Fixture::parse("typedef F = Future<int?>;\nF f(int? p) async => p!;\n");
    let lints = f.run_semantic(&one_rule("unnecessary_null_checks"));
    assert_eq!(lints.len(), 1, "{lints:?}");
}

/// `test_return_list_nullable#0` — `INFO|…|2:28|1` — a list member's
/// expected type is the (nullable) element type.
#[test]
fn unc_list_member_reports() {
    let f = Fixture::parse("List<int?> f(int? p) => [p!];\n");
    let lints = f.run_semantic(&one_rule("unnecessary_null_checks"));
    assert_eq!(lints.len(), 1, "{lints:?}");
}

/// `test_return_mapKey_nullable#0` / `test_return_mapValue_nullable#0` —
/// `INFO|…|2:33|1` each — the map entry's key and value sides read their
/// own expected type.
#[test]
fn unc_map_member_faces_report() {
    let f = Fixture::parse("Map<int?, String> f(int? p) => {p!: ''};\n");
    assert_eq!(
        f.run_semantic(&one_rule("unnecessary_null_checks")).len(),
        1
    );

    let f = Fixture::parse("Map<String, int?> f(int? p) => {'': p!};\n");
    assert_eq!(
        f.run_semantic(&one_rule("unnecessary_null_checks")).len(),
        1
    );
}

/// `test_yieldSyncStar_iterableOfNullable#0` / `test_yieldAsyncStar_streamOfNullable#0`
/// — `INFO|…|2:41|1` each — `yield` reads the generator return's first
/// type argument.
#[test]
fn unc_yield_faces_report() {
    let f = Fixture::parse("Iterable<int?> f(int? p) sync* {yield p!;}\n");
    assert_eq!(
        f.run_semantic(&one_rule("unnecessary_null_checks")).len(),
        1
    );

    let f = Fixture::parse("Stream<int?> f(int? p) async* {yield p!;}\n");
    assert_eq!(
        f.run_semantic(&one_rule("unnecessary_null_checks")).len(),
        1
    );
}

/// `test_functionCall_positionalArgument#0` — `INFO|…|2:29|1` — a
/// positional argument against a nullable formal.
#[test]
fn unc_positional_argument_reports() {
    let f = Fixture::parse("int? i;\nvoid f(int? p) => f(i!);\n");
    let lints = f.run_semantic(&one_rule("unnecessary_null_checks"));
    assert_eq!(lints.len(), 1, "{lints:?}");
}

/// A non-nullable formal needs the null check — the clean mirror of the
/// `*_nonNullable` conformance cases.
#[test]
fn unc_non_nullable_argument_stays_silent() {
    let f = Fixture::parse("int? i;\nvoid f(int p) => f(i!);\n");
    let lints = f.run_semantic(&one_rule("unnecessary_null_checks"));
    assert!(lints.is_empty(), "{lints:?}");
}

/// `test_functionCall_namedArgument#0` — `INFO|…|2:38|1` — a named
/// argument reads its named formal's declared type.
#[test]
fn unc_named_argument_reports() {
    let f = Fixture::parse("void f({int? p, int? i}) => f(p: i!);\n");
    let lints = f.run_semantic(&one_rule("unnecessary_null_checks"));
    assert_eq!(lints.len(), 1, "{lints:?}");
}

/// `test_constructorCall_optionalPositionalArgument#0` — `INFO|…|5:19|1`
/// — the constructor's optional positional formal carries the expected
/// type.
#[test]
fn unc_constructor_argument_reports() {
    let f = Fixture::parse("class A {\n  A([int? p]);\n}\nvoid f(int? i) => A(i!);\n");
    let lints = f.run_semantic(&one_rule("unnecessary_null_checks"));
    assert_eq!(lints.len(), 1, "{lints:?}");
}

/// `test_futureValue#0` — `-` — `Future.value`'s formal is outside the
/// indexed set, so no expected type is known and the check stands.
#[test]
fn unc_future_value_stays_silent() {
    let f = Fixture::parse("void f(int? i) => Future<int>.value(i!);\n");
    let lints = f.run_semantic(&one_rule("unnecessary_null_checks"));
    assert!(lints.is_empty(), "{lints:?}");
}

/// `test_assignment_nullableTarget#0` — `INFO|…|2:9|1` — a top-level
/// declaration's nullable written type admits null at the initializer.
#[test]
fn unc_top_level_declaration_reports() {
    let f = Fixture::parse("int? j = i!;\nint? i;\n");
    let lints = f.run_semantic(&one_rule("unnecessary_null_checks"));
    assert_eq!(lints.len(), 1, "{lints:?}");
}

/// `test_return_mapValue_forElement_nullable#0` — `INFO|…|2:61|1` — the
/// `for` element's entry keeps its map-value slot (:129-137, the element
/// nesting is transparent).
#[test]
fn unc_map_value_for_element_reports() {
    let f = Fixture::parse("\nMap<String, int?> f(int? p) => {for (var a in <int>[]) '': p!};\n");
    let lints = f.run_semantic(&one_rule("unnecessary_null_checks"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let bang = tok(&f.source, &f.tokens, "!", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, bang));
    assert_eq!(lints[0].range.end - lints[0].range.start, 1);
}

/// `test_return_mapValue_ifElement_nullable#0` —
/// `INFO|…|2:50|1` — the `if` element's entry keeps its map-value slot.
#[test]
fn unc_map_value_if_element_reports() {
    let f = Fixture::parse("\nMap<String, int?> f(int? p) => {if (1 != 0) '': p!};\n");
    let lints = f.run_semantic(&one_rule("unnecessary_null_checks"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let bang = tok(&f.source, &f.tokens, "!", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, bang));
}

/// `test_return_mapValue_ifElementNested_nullable#0` —
/// `INFO|…|2:62|1` — the nesting depth does not change the slot.
#[test]
fn unc_map_value_nested_if_element_reports() {
    let f = Fixture::parse("\nMap<String, int?> f(int? p) => {if (1 != 0) if (1 != 0) '': p!};\n");
    let lints = f.run_semantic(&one_rule("unnecessary_null_checks"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let bang = tok(&f.source, &f.tokens, "!", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, bang));
}

/// `test_functionCall_positionalArgument_parenthesized#0` —
/// `INFO|…|3:23|1` — the transparent `(...)` is not the argument's own
/// value: the slot range is the parenthesized one and the formal still
/// answers.
#[test]
fn unc_parenthesized_positional_argument_reports() {
    let f = Fixture::parse("\nint? i;\nvoid f(int? p) => f((i!));\n");
    let lints = f.run_semantic(&one_rule("unnecessary_null_checks"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let bang = tok(&f.source, &f.tokens, "!", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, bang));
}

// —— avoid_futureor_void ——————————————————————————————————————————————
// Upstream Dart SDK source, pkg/linter .../avoid_futureor_void.dart:59-73
// (checkNamedType): report a `FutureOr` whose single type argument is
// `void`, at the full type-annotation span; the ObjectPattern face rides
// the same check (:125 registry).

/// `avoid_futureor_void/doc_bad_0` — `INFO|…|3:1|14` — the report covers
/// the whole `FutureOr<void>` annotation, not just the head.
#[test]
fn afov_span_covers_the_type_arguments() {
    let f = Fixture::parse("import 'dart:async';\nFutureOr<void> x = throw 0;\n");
    let lints = f.run(&one_rule("avoid_futureor_void"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Don't use the type 'FutureOr<void>'.");
    let head = tok(&f.source, &f.tokens, "FutureOr", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, head));
    assert_eq!(lints[0].range.end - lints[0].range.start, 14);
}

/// `AvoidFutureOrVoidTest/test_objectPattern#0` — `INFO|…|5:14|14` — the
/// object pattern's head name is the checked type annotation.
#[test]
fn afov_object_pattern_reports() {
    let f = Fixture::parse(
        "import 'dart:async';\nf(Object? x) {\n  if (x case FutureOr<void>()) return;\n}\n",
    );
    let lints = f.run(&one_rule("avoid_futureor_void"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let head = tok(&f.source, &f.tokens, "FutureOr", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, head));
    assert_eq!(lints[0].range.end - lints[0].range.start, 14);
}

/// `avoid_futureor_void/doc_bad_1` — no lint row (only the semantic
/// `undefined_class`) — an unresolved `FutureOr` (no `dart:async` in
/// scope) is not the lint's type.
#[test]
fn afov_unresolved_head_stays_silent() {
    let f = Fixture::parse("FutureOr<void> x = throw 0;\n");
    let lints = f.run_semantic(&one_rule("avoid_futureor_void"));
    assert!(lints.is_empty(), "{lints:?}");
}
