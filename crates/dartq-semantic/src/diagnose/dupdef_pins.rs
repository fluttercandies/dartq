//! The #2963 pins: the `duplicate_definition` statement-list and for-head
//! faces the registration path now covers — the upstream
//! `checkStatements`/`checkForVariables` arms
//! (duplicate_definition_verifier.dart:48-113) walk a block, function
//! body, or switch member's statement list and a `for` head's declaration
//! list into one duplicate scope per list. A repeat name in the same list
//! reports `duplicate_definition` at the SECOND name token, the first
//! declaration riding the context row.
//!
//! The statement-level `PatternVariableDeclarationStatement` arm
//! (duplicate_definition_verifier.dart:99-103) adds each declared element
//! to the same scope as plain `VariableDeclarationStatement` names —
//! `var a`/`final a`/`var (a,b)` collide inside the open statement list
//! like any other declaration. Case-header (`case var x`) and if-case
//! binders are NOT `PatternVariableDeclarationStatement` elements — they
//! stay out of the scope (the arm-scoped and if-case paths the
//! `add_tracked_local` group window already distinguishes).
//!
//! The `conflicting_static_and_instance` pins reassert the own-scope arm
//! (`_checkClassStatic`, member_duplicate_definition_verifier.dart:178-233)
//! alongside the new coverage — the inherited face and the extension
//! one-argument variant live in ticket_2914_tests.
//!
//! Oracle provenance: `dart analyze` 3.13.7 over the fixture matrix —
//! `dup_local2`/`dup_local3`/`dup_local4` (statement-list `var`/`int`/`var;`
//! repeats), `e_for_var`/`dup_for_parts` (`for` head's own list),
//! `e_fn_dup_local` (statement-list local function repeat),
//! `dup_mixed` (typed and pattern-declared siblings),
//! `dup_case_pattern` (case-header binder exempt),
//! `static_method`/`static_getter`/`static_instance` (own-scope
//! `conflicting_static_and_instance` arms), `dup_field_mix` (own-scope
//! `conflicting_static_and_instance` over a mixed static+instance pair).

use super::test_rig::{assert_silent, report_ops, report_ops_wildcards};

fn dup_messages(report: &crate::diagnose::FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "duplicate_definition")
        .map(|d| d.message.to_string())
        .collect()
}

fn conflicting_messages(report: &crate::diagnose::FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| {
            matches!(
                d.code.name,
                "conflicting_static_and_instance" | "extension_conflicting_static_and_instance"
            )
        })
        .map(|d| d.message.to_string())
        .collect()
}

// —— statement-list pattern-declared variables ——

/// A `var` repeat inside a block body: the official `checkStatements`
/// (`PatternVariableDeclarationStatement` arm, verifier:99-103) reports
/// `duplicate_definition` at the second name. `var` declarations parse as
/// `PatternVariableDeclarationStatement` under the patterns feature.
#[test]
fn block_var_repeat_reports_duplicate() {
    let report = report_ops(
        "void g() {\n\
         \x20 var a = 1;\n\
         \x20 var a = 2;\n\
         \x20 print(a);\n\
         }\n",
    );
    let dups = dup_messages(&report);
    assert_eq!(
        dups,
        vec!["The name 'a' is already defined.".to_string()],
        "var-repeat inside a block body collides: {dups:?}"
    );
}

/// The typed sibling (`int a = 1; int a = 2;`) — upstream's plain
/// `VariableDeclarationStatement` arm — already rides the
/// `LocalVariableDeclaration` context's `dup_scope_add`; this pin guards
/// that parity against the pattern path's new wiring.
#[test]
fn block_typed_repeat_reports_duplicate() {
    let report = report_ops(
        "void g() {\n\
         \x20 int a = 1;\n\
         \x20 int a = 2;\n\
         \x20 print(a);\n\
         }\n",
    );
    let dups = dup_messages(&report);
    assert_eq!(
        dups,
        vec!["The name 'a' is already defined.".to_string()],
        "int-repeat inside a block body collides: {dups:?}"
    );
}

/// `var a;` (uninitialized) — a `PatternVariableDeclarationStatement`
/// with no initializer still declares `a` under the same
/// `checkStatements` scope.
#[test]
fn block_var_uninitialized_repeat_reports_duplicate() {
    let report = report_ops(
        "void g() {\n\
         \x20 var a;\n\
         \x20 var a;\n\
         \x20 print(a);\n\
         }\n",
    );
    let dups = dup_messages(&report);
    assert_eq!(
        dups,
        vec!["The name 'a' is already defined.".to_string()],
        "uninitialized var-repeat collides: {dups:?}"
    );
}

/// A `final` pattern variable repeats the same way — `final` is a
/// `var_final_or_const` modifier inside the same
/// `PatternVariableDeclarationStatement` face.
#[test]
fn block_final_repeat_reports_duplicate() {
    let report = report_ops(
        "void g() {\n\
         \x20 final a = 1;\n\
         \x20 final a = 2;\n\
         \x20 print(a);\n\
         }\n",
    );
    let dups = dup_messages(&report);
    assert_eq!(
        dups,
        vec!["The name 'a' is already defined.".to_string()],
        "final-repeat collides: {dups:?}"
    );
}

/// A destructuring group (`var (a, b) = pair`) claims each element under
/// the statement list's scope — a second `var a` repeats into the same
/// scope as the group's `a`.
#[test]
fn block_destructuring_then_var_reports_duplicate() {
    let report = report_ops(
        "void g() {\n\
         \x20 var (a, b) = (1, 2);\n\
         \x20 var a = 0;\n\
         \x20 print('$a$b');\n\
         }\n",
    );
    let dups = dup_messages(&report);
    assert_eq!(
        dups,
        vec!["The name 'a' is already defined.".to_string()],
        "destructuring element then var collides: {dups:?}"
    );
}

/// A `var` repeat across SIBLING blocks stays silent — each block owns
/// its own statement list scope (upstream `visitBlock` opens a fresh
/// `checkStatements` per `Block` node).
#[test]
fn sibling_block_var_stays_silent() {
    let report = report_ops(
        "void g() {\n\
         \x20 {\n\
         \x20   var a = 1;\n\
         \x20   print(a);\n\
         \x20 }\n\
         \x20 {\n\
         \x20   var a = 2;\n\
         \x20   print(a);\n\
         \x20 }\n\
         }\n",
    );
    assert_silent(&report);
}

/// A `var` shadowing an OUTER declaration inside a nested block stays
/// silent — the inner block's scope is fresh (upstream `visitBlock`'s
/// scoped `checkStatements` never reaches the outer list's names).
#[test]
fn nested_block_var_shadow_stays_silent() {
    let report = report_ops(
        "void g() {\n\
         \x20 var a = 1;\n\
         \x20 {\n\
         \x20   var a = 2;\n\
         \x20   print(a);\n\
         \x20 }\n\
         \x20 print(a);\n\
         }\n",
    );
    assert_silent(&report);
}

// —— for-head declaration list ——

/// `checkForVariables` (verifier:48-53) walks a `for` head's declaration
/// list into its own scope — `for (var i = 0, i = 0;;)` repeats `i`
/// inside that list and reports `duplicate_definition` at the second.
/// The for-head `var` parses as a `PatternVariableDeclaration` element.
#[test]
fn for_head_var_repeat_reports_duplicate() {
    let report = report_ops(
        "void g() {\n\
         \x20 for (var i = 0, i = 0; i < 5;) {\n\
         \x20   print(i);\n\
         \x20 }\n\
         }\n",
    );
    let dups = dup_messages(&report);
    assert_eq!(
        dups,
        vec!["The name 'i' is already defined.".to_string()],
        "for-head var repeat collides: {dups:?}"
    );
}

/// The for-head's `checkForVariables` scope is independent of the
/// enclosing block's `checkStatements` scope — a `var i` inside the body
/// shadows the head's `i` without colliding.
#[test]
fn for_head_then_body_var_stays_silent() {
    let report = report_ops(
        "void g() {\n\
         \x20 for (var i = 0; i < 5; i++) {\n\
         \x20   var i = 10;\n\
         \x20   print(i);\n\
         \x20 }\n\
         }\n",
    );
    // The body's `var i` shadows the head's — upstream reads this as a
    // separate block-scope declaration (no duplicate), not a repeat of
    // the for-head's `i`.
    assert_silent(&report);
}

// —— local function repeats ——

/// A `void inner() {}` repeat inside a block body: upstream's
/// `checkStatements` `FunctionDeclarationStatement` arm (verifier:92-98)
/// adds each named local function to the statement list's scope.
#[test]
fn block_local_function_repeat_reports_duplicate() {
    let report = report_ops(
        "void g() {\n\
         \x20 void inner() {}\n\
         \x20 void inner() {}\n\
         }\n",
    );
    let dups = dup_messages(&report);
    assert_eq!(
        dups,
        vec!["The name 'inner' is already defined.".to_string()],
        "local function repeat collides: {dups:?}"
    );
}

/// A `var` colliding with a same-named local function inside the same
/// statement list reports once — the scope is unified across
/// `VariableDeclarationStatement`/`FunctionDeclarationStatement`/
/// `PatternVariableDeclarationStatement` members.
#[test]
fn block_var_then_local_fn_reports_duplicate() {
    let report = report_ops(
        "void g() {\n\
         \x20 var inner = 1;\n\
         \x20 void inner() {}\n\
         \x20 print(inner);\n\
         }\n",
    );
    let dups = dup_messages(&report);
    assert_eq!(
        dups,
        vec!["The name 'inner' is already defined.".to_string()],
        "var then local fn collides: {dups:?}"
    );
}

// —— wildcard `_` posture ——

/// Under `wildcard-variables` ON, a `var _` declares no name — two `_`
/// locals stay silent (upstream `isWildcard` gate, :444-457).
#[test]
fn wildcard_local_var_stays_silent() {
    let report = report_ops_wildcards(
        "void g() {\n\
         \x20 var _ = 1;\n\
         \x20 var _ = 2;\n\
         }\n",
        /* wildcards enabled */ true,
    );
    let dups = dup_messages(&report);
    assert!(dups.is_empty(), "wildcard `_` locals stay silent: {dups:?}");
}

/// Feature OFF (pre-3.7): `_` is an ordinary local name — `var _; var _;`
/// reports `duplicate_definition` at the second.
#[test]
fn wildcard_local_var_pre_feature_duplicates() {
    let report = report_ops_wildcards(
        "void g() {\n\
         \x20 var _ = 1;\n\
         \x20 var _ = 2;\n\
         \x20 print(_);\n\
         }\n",
        /* wildcards disabled */ false,
    );
    let dups = dup_messages(&report);
    assert_eq!(
        dups,
        vec!["The name '_' is already defined.".to_string()],
        "pre-feature `_` local is an ordinary name: {dups:?}"
    );
}

// —— case-header and if-case binders stay out ——

/// A `case var x` binder is a `CaseClause` pattern, not a
/// `PatternVariableDeclarationStatement` — the statement-list scope
/// never sees it, so a same-name repeat inside a later case body stays
/// silent (upstream's `checkStatements` never reaches `case` patterns).
#[test]
fn case_binder_then_var_stays_silent() {
    let report = report_ops(
        "void g(int x) {\n\
         \x20 switch (x) {\n\
         \x20   case var v:\n\
         \x20     print(v);\n\
         \x20     break;\n\
         \x20   default:\n\
         \x20     var v = 0;\n\
         \x20     print(v);\n\
         \x20 }\n\
         }\n",
    );
    assert_silent(&report);
}

/// An if-case binder is likewise not a
/// `PatternVariableDeclarationStatement` — its variables scope over the
/// then-branch only.
#[test]
fn if_case_binder_then_var_stays_silent() {
    let report = report_ops(
        "void g(Object? x) {\n\
         \x20 if (x case int v) {\n\
         \x20   print(v);\n\
         \x20 }\n\
         \x20 var v = 0;\n\
         \x20 print(v);\n\
         }\n",
    );
    assert_silent(&report);
}

// —— conflicting_static_and_instance own-scope arms ——

/// The own-scope arm (`_checkClassStatic`,
/// member_duplicate_definition_verifier.dart:190-233): a `static`
/// member colliding with an `instance` member of the same name inside
/// one class reports `conflicting_static_and_instance` at the `static`
/// member's name.
#[test]
fn conflicting_static_method_over_instance_method() {
    let report = report_ops(
        "class S {\n\
         \x20 static void m() {}\n\
         \x20 void m() {}\n\
         }\n",
    );
    let conflicts = conflicting_messages(&report);
    assert_eq!(
        conflicts.len(),
        1,
        "static/instance same-name method collides: {conflicts:?}"
    );
    assert!(
        conflicts[0].contains("static member 'm'"),
        "message names the static member: {conflicts:?}"
    );
}

/// The reverse direction (`instance` first, `static` second) collides
/// the same way — the check walks every `static` member against the
/// already-collected `instanceScope`, never the reverse.
#[test]
fn conflicting_instance_method_over_static_method() {
    let report = report_ops(
        "class S {\n\
         \x20 void m() {}\n\
         \x20 static void m() {}\n\
         }\n",
    );
    let conflicts = conflicting_messages(&report);
    assert_eq!(
        conflicts.len(),
        1,
        "instance/static same-name method collides: {conflicts:?}"
    );
}

/// A `static` field over an `instance` field of the same name collides
/// the same way (`FieldDeclaration` arm, :191-210).
#[test]
fn conflicting_static_field_over_instance_field() {
    let report = report_ops(
        "class D {\n\
         \x20 static int x = 0;\n\
         \x20 int x = 1;\n\
         }\n",
    );
    let conflicts = conflicting_messages(&report);
    assert_eq!(
        conflicts.len(),
        1,
        "static/instance same-name field collides: {conflicts:?}"
    );
}

/// A `static` getter over an `instance` getter collides (the
/// `MethodDeclaration` arm's `isStatic` gate, :212-229 — a `static get`
/// is a static method upstream).
#[test]
fn conflicting_static_getter_over_instance_getter() {
    let report = report_ops(
        "class T {\n\
         \x20 static int get p => 0;\n\
         \x20 int get p => 1;\n\
         }\n",
    );
    let conflicts = conflicting_messages(&report);
    assert_eq!(
        conflicts.len(),
        1,
        "static/instance same-name getter collides: {conflicts:?}"
    );
}

/// A static member + instance member of DIFFERENT names stays silent —
/// the check keys on the member name only.
#[test]
fn static_and_instance_different_names_stay_silent() {
    let report = report_ops(
        "class C {\n\
         \x20 static int x = 0;\n\
         \x20 int y = 1;\n\
         }\n",
    );
    assert_silent(&report);
}

/// A `static` member of a class never conflicts with an instance member
/// of an unrelated class — the `instanceScope` is the declaring
/// fragment's own collection.
#[test]
fn static_in_one_class_instance_in_another_stays_silent() {
    let report = report_ops(
        "class A {\n\
         \x20 static int x = 0;\n\
         \x20 }\n\
         class B {\n\
         \x20 int x = 1;\n\
         }\n",
    );
    assert_silent(&report);
}

// —— member-scope duplicate_definition faces (brief pins) ——

/// Two same-named methods duplicate under `_checkDuplicateIdentifier`
/// (member_duplicate_definition_verifier.dart:296-345): the second
/// `MethodDeclaration` name reports `duplicate_definition`.
#[test]
fn member_method_repeat_reports_duplicate() {
    let report = report_ops(
        "class C {\n\
         \x20 void foo() {}\n\
         \x20 void foo() {}\n\
         }\n",
    );
    let dups = dup_messages(&report);
    assert_eq!(
        dups,
        vec!["The name 'foo' is already defined.".to_string()],
        "two same-named methods collide: {dups:?}"
    );
}

/// A getter+setter pair under the bare lexeme merges into one scope entry
/// (upstream `_ScopeEntryGetterSetterPair` :320-334) — no duplicate.
#[test]
fn member_getter_setter_pair_stays_silent() {
    let report = report_ops(
        "class C {\n\
         \x20 int get foo => 0;\n\
         \x20 set foo(int v) {}\n\
         }\n",
    );
    let dups = dup_messages(&report);
    assert!(dups.is_empty(), "getter/setter pair is legal: {dups:?}");
}

/// A constructor's name lives in the enclosing class's namespace, not
/// the member scope — `C.C()` plus a method `C()` is ill-formed, but a
/// ctor named `C.x` and a method `x` never collide (upstream member
/// scope only collects the member name segment).
#[test]
fn member_ctor_and_method_same_tail_stay_silent() {
    let report = report_ops(
        "class C {\n\
         \x20 C.x();\n\
         \x20 void x() {}\n\
         }\n",
    );
    let dups = dup_messages(&report);
    assert!(
        dups.is_empty(),
        "ctor tail and method are separate namespaces: {dups:?}"
    );
}

// —— clean-file pin ——

/// A minimal file with one top-level function and no repeats emits no
/// diagnostics — the row-free baseline.
#[test]
fn clean_file_emits_no_rows() {
    let report = report_ops("void f() {}\n");
    assert_silent(&report);
}
