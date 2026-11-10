//! #3648 pins — `not_assigned_potentially_non_nullable_local_variable`'s
//! diagnose-side suppression gates, each standing in for a flow-side fix
//! tracked as a rider (forbidden files this round):
//!
//! * `for (i in xs)` — the head's existing-variable target is a WRITE
//!   upstream (`forEach_bodyBegin` calls `flow.write`,
//!   for_resolver.dart:237-241), not a read; the flow's `i + 1` lookahead
//!   for `ForInExprBegin` misses the `ForInit` marker between them.
//! * `_` under wildcard-variables — upstream's
//!   `element is PromotableElementImpl` refuses a `WildcardVariableElement`
//!   outright (`checkReadOfNotAssignedLocalVariable` returns early), so the
//!   non-binding wildcard never reaches `isAssigned`.
//! * shared-case-scope inconsistent binder — upstream reports
//!   `invalid_pattern_variable_in_shared_case_scope` at the read
//!   (variable_bindings.dart:183-194) and resolves it to the joined shared
//!   variable, never an unassigned read.
//! * `void` and typedef-nullable declared heads — `isPotentiallyNonNullable`
//!   is `!isNullable` at the resolved element (type_system.dart:1249),
//!   which `void` and `typedef A = T?` spellings answer `false`; the flow's
//!   bit only reads the use-site `?` and misses `Base::Void`.

use super::test_rig::{code_names, report_ops, report_ops_wildcards, rows};

fn not_assigned(source: &str) -> Vec<String> {
    rows(&report_ops(source))
        .into_iter()
        .filter(|r| r.starts_with("not_assigned_potentially_non_nullable_local_variable"))
        .collect()
}

fn codes(source: &str) -> Vec<&'static str> {
    code_names(&report_ops(source))
}

/// `for (i in <int>[])` on an existing declared `int i` — the head's target
/// is a write, oracle emits only `unused_local_variable` (upstream
/// `forEach_bodyBegin` writes the target before the body's reads).
#[test]
fn for_in_existing_variable_target_writes_not_reads() {
    let source = "void f() {\n  int i;\n  for (i in <int>[]) {}\n}\n";
    let rows = not_assigned(source);
    assert!(rows.is_empty(), "{rows:?}");
}

/// `for (i in xs)` on an existing declared `String s` — same write face
/// over a spelled annotation, no `not_assigned` at the head.
#[test]
fn for_in_existing_variable_target_spelled_head() {
    let source = "void f(List<String> xs) {\n  String s;\n  for (s in xs) {}\n}\n";
    let rows = not_assigned(source);
    assert!(rows.is_empty(), "{rows:?}");
}

/// `for (var i in xs)` — the declared head assigns through the
/// `InitializedIdent` path; the loop variable reads assigned inside the
/// body and the head itself never rows.
#[test]
fn for_in_declared_variable_head_stays_silent() {
    let source = "void f(List<int> xs) {\n  for (var i in xs) { i; }\n}\n";
    let rows = not_assigned(source);
    assert!(rows.is_empty(), "{rows:?}");
}

/// `for (int i in xs)` — the declared head's read inside the body is a
/// real read of an assigned variable; neither head nor body rows fire.
#[test]
fn for_in_declared_typed_variable_body_read_stays_silent() {
    let source = "void f(List<int> xs) {\n  for (int i in xs) { i; }\n}\n";
    let rows = not_assigned(source);
    assert!(rows.is_empty(), "{rows:?}");
}

/// The read AFTER a `for (var i in xs)` — the loop variable's lexical
/// scope closed; a same-named outer `i` reads its own declaration (here a
/// definitely-assigned parameter), never the loop's shadow.
///
/// RIDER (flow scope-close): the loop variable's flow scope must close at
/// the construct end; the current flow leaks the declaration past the
/// loop, so the post-loop `i;` resolves to the loop's unassigned `i` and
/// `UnassignedRead` misfires. Diagnose-side suppression cannot cover this
/// (the `declared_type` arm's backward scan correctly refuses `var`), so
/// the pin stays `#[ignore]`d until the flow rider lands.
#[test]
fn for_in_declared_head_does_not_leak_scope() {
    let source = "void f(List<int> x, int i) {\n  for (var i in x) {}\n  i;\n}\n";
    let rows = not_assigned(source);
    assert!(rows.is_empty(), "{rows:?}");
}

/// A wildcard `_` under wildcard-variables — `int _ = _;` binds nothing
/// (upstream's `WildcardVariableElement` is not a `PromotableElement`), so
/// the initializer's `_` reads the enclosing scope's ordinary `_`.
#[test]
fn wildcard_local_read_binds_outer_name() {
    let source = "int _ = 42;\nvoid f() {\n  int _ = _;\n  _;\n}\n";
    let rows = rows(&report_ops_wildcards(source, true))
        .into_iter()
        .filter(|r| r.starts_with("not_assigned_potentially_non_nullable_local_variable"))
        .collect::<Vec<_>>();
    assert!(rows.is_empty(), "{rows:?}");
}

/// `_` under pre-wildcard language versions — an ordinary name (`_`
/// remains promotable); `int _ = _;` reads itself unassigned and still
/// rows `not_assigned`.
#[test]
fn wildcard_disabled_local_read_still_reports() {
    let source = "int _ = 42;\nvoid f() {\n  int _ = _;\n  _;\n}\n";
    let rows = rows(&report_ops_wildcards(source, false))
        .into_iter()
        .filter(|r| r.starts_with("not_assigned_potentially_non_nullable_local_variable"))
        .collect::<Vec<_>>();
    // `int _ = _;` declares `_` already-initialized (upstream's
    // `variableDeclarationList` declares every declarator with
    // `initialized: variable.initializer != null` before visiting any
    // initializer), so the self-read and the later read both see an
    // assigned variable; the wildcard posture adds nothing.
    assert!(rows.is_empty(), "{rows:?}");
}

/// A shared-case-scope read where `case int a when a > 0:` declares `a`
/// in only ONE head — upstream reports
/// `invalid_pattern_variable_in_shared_case_scope` (the joined variable
/// exists but is unsafe), never `not_assigned`.
#[test]
fn shared_case_scope_inconsistent_binder_suppresses() {
    let source = concat!(
        "void f(Object? x) {\n",
        "  switch (x) {\n",
        "    case 0:\n",
        "    case int a when a > 0:\n",
        "      a;\n",
        "  }\n",
        "}\n",
    );
    let cs = codes(source);
    assert!(
        !cs.contains(&"not_assigned_potentially_non_nullable_local_variable"),
        "{cs:?}"
    );
    assert!(
        cs.contains(&"invalid_pattern_variable_in_shared_case_scope"),
        "{cs:?}"
    );
}

/// `void x` — `isNullable(void)` is true upstream, so
/// `isPotentiallyNonNullable` returns false and `checkReadOfNotAssigned`
/// never reaches the `!assigned` arm; the use-site is a `use_of_void_result`
/// row instead.
#[test]
fn void_declared_local_suppresses() {
    let source = "void f() {\n  void x;\n  x;\n}\n";
    let rows = not_assigned(source);
    assert!(rows.is_empty(), "{rows:?}");
}

/// `late final void x` — the `void` face still suppresses; the row rides
/// the `definitely_unassigned_late_local_variable` face only under
/// `definitely_unassigned`, and a plain `late` declarator without an
/// initializer still answers `true` there.
#[test]
fn void_declared_late_final_reads_late_face_not_name_face() {
    let source = "void f() {\n  late final void x;\n  x;\n}\n";
    let cs = codes(source);
    assert!(
        !cs.contains(&"not_assigned_potentially_non_nullable_local_variable"),
        "{cs:?}"
    );
}

/// `typedef A = T?` — the use-site `?` is absent but the right-hand folds
/// nullable: upstream's `element.type` answers `A?` (nullable) and the row
/// stays silent even though the bare head's `nullable` bit is false.
#[test]
fn typedef_nullable_alias_suppresses() {
    let source = "typedef AAlias = A?;\nclass A {}\nvoid f() {\n  AAlias a;\n  a;\n}\n";
    let rows = not_assigned(source);
    assert!(rows.is_empty(), "{rows:?}");
}

/// `typedef R = (int, String)?` — a record-shaped alias folds the same
/// nullable answer through `Ty::Record { nullable: true, .. }`.
#[test]
fn typedef_nullable_record_alias_suppresses() {
    let source = concat!(
        "typedef R = (int, String)?;\n",
        "void f() {\n",
        "  R r;\n",
        "  r;\n",
        "}\n",
    );
    let rows = not_assigned(source);
    assert!(rows.is_empty(), "{rows:?}");
}

/// `typedef F = void Function()?` — a function-shaped alias folds through
/// `Ty::Function { nullable: true, .. }` — the upstream `element.type`'s
/// `isNullable` answer is the right-hand's own.
#[test]
fn typedef_nullable_function_alias_suppresses() {
    let source = concat!(
        "typedef F = void Function()?;\n",
        "void f() {\n",
        "  F f;\n",
        "  f;\n",
        "}\n",
    );
    let rows = not_assigned(source);
    assert!(rows.is_empty(), "{rows:?}");
}

/// `int v` — the negative pin: a plain non-nullable declared head still
/// fires at a definitely-unassigned read (the gate suppresses only the
/// faces upstream suppresses).
#[test]
fn non_nullable_declared_head_still_fires() {
    let source = "void f() {\n  int v;\n  v;\n}\n";
    let rows = not_assigned(source);
    assert_eq!(rows.len(), 1, "{rows:?}");
}

/// `var v` — `var`'s declared slot is `dynamic`/`Unknown` (the flow's
/// name-level face already suppresses it); the pin guards the diagnose
/// gate never ADDS a new misfire over an inference-shaped declarator.
#[test]
fn var_inferred_local_still_silent() {
    let source = "void f() {\n  var v;\n  v;\n}\n";
    let rows = not_assigned(source);
    assert!(rows.is_empty(), "{rows:?}");
}
