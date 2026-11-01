//! #3471 pins — `body_might_complete_normally[_nullable]` reads the
//! element's own return face: a declaration with no written return type
//! completes `dynamic` upstream and stays silent, and a written head that
//! resolves to nothing (the oracle's `InvalidType`) asks no nullability
//! question at all.
//!
//! The staged declared return passes through
//! `Engine::completion_declared_ty` before the row joins the flow fact:
//! an unarmed latch already answers `Ty::UNKNOWN` for the omitted face,
//! and an unresolved named head now folds to the same stand-in — upstream
//! `isPotentiallyNonNullable`/`isNullable` both admit `InvalidType`, so
//! neither twin fires. Before the fold the nullable twin reported `D?`
//! heads that resolved to nothing (the ERROR twin asked the scope
//! question the nullable arm skipped).
//!
//! Anchors locate themselves with `find` over the prelude-prefixed
//! source. Registered through `diagnose/tests.rs`'s `#[path]` tail.

use super::test_rig::{CORE_PRELUDE, report_ops};

/// Every `body_might_complete_normally*` row as `(code, offset)`,
/// source-ordered — the two completion codes are the only rows these
/// pins decide, so a fixture's other rows (unresolved names, unused
/// declarations) never enter the answer.
fn completion_rows(source: &str) -> Vec<(&'static str, u32)> {
    let report = report_ops(source);
    let mut rows: Vec<(&'static str, u32)> = report
        .diagnostics
        .iter()
        .filter(|row| row.code.name.starts_with("body_might_complete_normally"))
        .map(|row| (row.code.name, row.range.offset))
        .collect();
    rows.sort_unstable();
    rows
}

/// The offset of the `nth` (0-based) occurrence of `needle` in the
/// FIXTURE text, shifted past the prelude the rig mounts ahead of it.
/// Every fixture here is ASCII, so byte and UTF-16 offsets coincide.
fn at(source: &str, needle: &str, nth: usize) -> u32 {
    let found = source
        .match_indices(needle)
        .nth(nth)
        .unwrap_or_else(|| panic!("`{needle}` occurrence #{nth} missing"));
    found.0 as u32 + CORE_PRELUDE.len() as u32
}

#[track_caller]
fn assert_silent(source: &str) {
    assert_eq!(completion_rows(source), Vec::new(), "{source:?}");
}

// —— Omitted return types (the ticket's signature family) ————————————

/// The census signature `(col 1, len 4)`: a top-level `main` written
/// without a return type completes `dynamic` upstream — silent.
#[test]
fn omitted_return_main_stays_silent() {
    assert_silent("main() {\n  print(0);\n}\n");
}

/// The same face inside a class: `test(C c) {` carries no written
/// return, so the body's completion question never fires — the `C`
/// formal is the parameter's own annotation, never the method's.
#[test]
fn omitted_return_method_stays_silent() {
    assert_silent("class C {}\nclass T {\n  test(C c) {\n    print(c);\n  }\n}\n");
}

/// A local function declared without a return type keeps the same
/// dynamic face — the declaration flavor stages at its frame pop.
#[test]
fn omitted_return_local_function_stays_silent() {
    assert_silent("main() {\n  test() {\n    print(0);\n  }\n  test();\n}\n");
}

/// A getter written without a return type completes `dynamic` too —
/// the accessor's absent annotation is no completion target.
#[test]
fn omitted_return_getter_stays_silent() {
    assert_silent("class C {\n  get g {\n    return 0;\n  }\n}\n");
}

/// The discriminating fixture (`NonErrorResolverTest`'s
/// `test_async_flattened`): a `typedef` ahead of `main` arms a function
/// face on the shared slot, and the untyped `main` still reads its own
/// absent return — the stale-arm latch stays cleared.
#[test]
fn typedef_before_main_does_not_bleed() {
    assert_silent(
        "typedef Future<int> CreatesFutureInt();\n\
         main() {\n\
         \x20 CreatesFutureInt createFutureInt = () async => f();\n\
         \x20 Future<int> futureInt = createFutureInt();\n\
         \x20 futureInt.then((int i) => print(i));\n\
         }\n\
         Future<int> f() => throw 0;\n",
    );
}

/// An `async` body written without a return type keeps the imposed-null
/// face — the oracle's `contextType` is null and the walk never fires.
#[test]
fn omitted_return_async_stays_silent() {
    assert_silent("main() async {\n  print(0);\n}\n");
    assert_silent("class C {\n  m() async {}\n}\n");
}

// —— Written faces still decide ——————————————————————————————————————

/// Control: a written non-nullable return type whose block body can
/// complete normally keeps the ERROR twin, anchored at the declaring
/// name. The fix must not silence the code wholesale.
#[test]
fn written_non_nullable_return_still_fires() {
    let source = "int f() {}\n";
    assert_eq!(
        completion_rows(source),
        [("body_might_complete_normally", at(source, "f()", 0))],
    );
}

/// A written nullable return keeps the WARNING twin at the same
/// declaring-name anchor — the fold touches only unresolved heads.
#[test]
fn written_nullable_return_still_fires() {
    let source = "int? f() {}\n";
    assert_eq!(
        completion_rows(source),
        [(
            "body_might_complete_normally_nullable",
            at(source, "f()", 0)
        )],
    );
}

// —— Unresolved written heads (the InvalidType fold) ——————————————————

/// `D? foo(...)` where `D` resolves to nothing: upstream's `returnType`
/// is `InvalidType`, which `isNullable` admits — the oracle stays
/// silent. The staged fold reads the head through the same
/// `named_type_is_type` gate the ERROR twin already applies.
#[test]
fn unresolved_nullable_return_stays_silent() {
    assert_silent("D? foo(int x) {\n  int x1 = x;\n}\n");
}

/// The unresolved head's non-nullable twin asks the same question and
/// stays silent — the fold changes nothing the ERROR arm didn't already
/// decide.
#[test]
fn unresolved_non_nullable_return_stays_silent() {
    assert_silent("D foo(int x) {\n  int x1 = x;\n}\n");
}

/// A member of the enclosing container is never a type — `m?` as a
/// written return names the method upstream (`notAType` → `InvalidType`)
/// and the completion walk spares it.
#[test]
fn member_named_nullable_return_stays_silent() {
    assert_silent("class C {\n  void m() {}\n  m? f() {}\n}\n");
}

/// A declared type parameter is a real type through its frame — `T?`
/// keeps the nullable twin, so the fold's scope gate must not swallow it.
#[test]
fn type_parameter_nullable_return_still_fires() {
    let source = "class C<T> {\n  T? f() {}\n}\n";
    assert_eq!(
        completion_rows(source),
        [(
            "body_might_complete_normally_nullable",
            at(source, "f()", 0)
        )],
    );
}

/// A resolved alias head folds through the alias before the verdict —
/// `typedef V = void; V f() {}` walks `void` (silent) rather than the
/// alias head's potentially-non-nullable face.
#[test]
fn void_alias_return_stays_silent() {
    assert_silent("typedef V = void;\nV f() {}\n");
}
