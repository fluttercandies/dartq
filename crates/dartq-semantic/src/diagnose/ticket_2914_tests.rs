//! The #2914 `duplicate_definition`/`conflicting_static_and_instance` pins:
//! the member-scope and top-level setter faces the verifier gained, plus
//! the wildcard-`_` feature gate.
//!
//! 1. Member-scope getter/setter pairs (upstream
//!    `member_duplicate_definition_verifier.dart:297-346`): a lone getter
//!    merges with a lone setter into a pair under the BARE lexeme; every
//!    other same-name claim in the same staticness scope collides. A field
//!    claims its induced getter always, its induced setter only while
//!    `definesSetter` (`!final`, or `late final` without an initializer).
//! 2. The top-level `definedSetters` face (upstream
//!    `duplicate_definition_verifier.dart:116-307`): a non-final top-level
//!    variable claims `name=`; a real `set name` collides with that claim
//!    and reports under the `name=` spelling (the oracle's `lookupName`
//!    renders `f=`).
//! 3. The wildcard gate (upstream `isWildcardVariable`, :350-360 and the
//!    `_FormalParameterDuplicateIdentifierScope` override :444-457): under
//!    the wildcards feature a `_` declares nothing — no dup, no scope
//!    claim; `this._` is a FieldFormalParameterElement and never a
//!    wildcard, while `super._` and declaring primary-constructor formals
//!    are ordinary parameters and do wildcard.
//! 4. `conflicting_static_and_instance` (upstream
//!    `_checkClassStatic`/`_checkEnumStatic`/`_checkExtensionStatic`,
//!    member_duplicate_definition_verifier.dart:179-522, and
//!    `ErrorVerifier._checkForConflictingClassMembers`,
//!    error_verifier.dart:3548-3642): the own-scope arm, the extension's
//!    dedicated one-argument code, the enum's getMember face with the
//!    implicit `Enum`/`Object` fallback, and the inherited arm reporting
//!    the declaring owner.
//!
//! Oracle provenance: the #2914 census rows —
//! `duplicate_definition` `parameters_*`/`typeParameters_*`/`field_*`/
//! `topLevel_field_setter`/`switchPatternCase`, `conflicting_static_and_instance`
//! `ConflictingStaticAndInstanceClassTest`/`MixinTest`/`EnumTest`/
//! `ExtensionTest` and co19 `enhanced_enums_error_test` (`dart analyze`,
//! Dart 3.13.7).

use super::test_rig::{assert_silent, code_names, report_ops, report_ops_wildcards, rows};

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

// —— member-scope getter/setter pairs ——

/// A lone getter pairs with a lone setter under the bare lexeme (upstream
/// `_ScopeEntryGetterSetterPair` :320-334): no duplicate.
/// Census `duplicate_definition` member scope — getter/setter pair rows.
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

/// A getter over a method's `OTHER` claim collides — the pair merge only
/// accepts getter+setter.
#[test]
fn member_method_then_getter_duplicates() {
    let report = report_ops(
        "class C {\n\
         \x20 int foo() => 0;\n\
         \x20 int get foo => 0;\n\
         }\n",
    );
    let dups = dup_messages(&report);
    assert_eq!(
        dups,
        vec!["The name 'foo' is already defined.".to_string()],
        "method+getter collide: {dups:?}"
    );
}

/// A second setter duplicates the pair (a pair accepts exactly one of each).
#[test]
fn member_two_setters_duplicate() {
    let report = report_ops(
        "class C {\n\
         \x20 int get foo => 0;\n\
         \x20 set foo(int a) {}\n\
         \x20 set foo(int b) {}\n\
         }\n",
    );
    let dups = dup_messages(&report);
    assert_eq!(
        dups,
        vec!["The name 'foo' is already defined.".to_string()],
        "two setters collide: {dups:?}"
    );
}

// —— field induced-accessor claims ——

/// A non-final field claims BOTH its induced getter and setter, so a
/// written `set foo` after `int foo` collides (the `E.e1` face of
/// `enhanced_enums_error_test` and the `field_*` member-scope rows).
#[test]
fn member_field_then_setter_duplicates() {
    let report = report_ops(
        "class C {\n\
         \x20 int foo = 0;\n\
         \x20 set foo(int v) {}\n\
         }\n",
    );
    let dups = dup_messages(&report);
    assert_eq!(
        dups,
        vec!["The name 'foo' is already defined.".to_string()],
        "non-final field defines a setter: {dups:?}"
    );
}

/// `final int foo = 0;` induces NO setter — a written `set foo` stays
/// legal (`definesSetter` false, upstream :128-149).
#[test]
fn member_final_field_then_setter_stays_silent() {
    let report = report_ops(
        "class C {\n\
         \x20 final int foo = 0;\n\
         \x20 set foo(int v) {}\n\
         }\n",
    );
    let dups = dup_messages(&report);
    assert!(dups.is_empty(), "final field defines no setter: {dups:?}");
}

/// `late final foo = 1` initializes at the declaring site — no induced
/// setter.
#[test]
fn member_late_final_initialized_field_then_setter_stays_silent() {
    let report = report_ops(
        "class C {\n\
         \x20 late final int foo = 1;\n\
         \x20 set foo(int v) {}\n\
         }\n",
    );
    let dups = dup_messages(&report);
    assert!(
        dups.is_empty(),
        "initialized late-final defines no setter: {dups:?}"
    );
}

/// `late final foo;` (no initializer) DOES define a setter — the written
/// `set foo` collides.
#[test]
fn member_late_final_uninitialized_field_then_setter_duplicates() {
    let report = report_ops(
        "class C {\n\
         \x20 late final int foo;\n\
         \x20 set foo(int v) {}\n\
         }\n",
    );
    let dups = dup_messages(&report);
    assert_eq!(
        dups,
        vec!["The name 'foo' is already defined.".to_string()],
        "uninitialized late-final defines a setter: {dups:?}"
    );
}

// —— top-level setter scope (`definedSetters`) ——

/// `int f;` claims `f=` (non-final induced setter), so `set f` reports
/// the `f=` face — the `topLevel_field_setter` census row (expected.tsv
/// :8034/8037/8038 render `'f='`).
#[test]
fn top_level_var_then_setter_reports_setter_name() {
    let report = report_ops(
        "int f = 0;\n\
         set f(int v) {}\n",
    );
    let dups = dup_messages(&report);
    assert_eq!(
        dups,
        vec!["The name 'f=' is already defined.".to_string()],
        "the var's induced setter collides with the written one: {dups:?}"
    );
}

/// `final int f = 0;` induces no setter — `set f` stays legal.
#[test]
fn top_level_final_var_then_setter_stays_silent() {
    let report = report_ops(
        "final int f = 0;\n\
         set f(int v) {}\n",
    );
    let dups = dup_messages(&report);
    assert!(dups.is_empty(), "final var defines no setter: {dups:?}");
}

// —— wildcard `_` gate ——

/// Feature ON: `_, _, _` formal parameters declare nothing and report
/// nothing (upstream `isWildcard` :444-457 — a plain `_` formal is a
/// wildcard).
/// Census `duplicate_definition` `parameters_*` wildcards rows.
#[test]
fn wildcard_formal_parameters_stay_silent() {
    let report = report_ops_wildcards(
        "void f(int _, int _, {int? _}) {}\n",
        /* wildcards enabled */ true,
    );
    let dups = dup_messages(&report);
    assert!(dups.is_empty(), "wildcards declare no name: {dups:?}");
}

/// Feature OFF (pre-3.7): every `_` is an ordinary name — `_, _` reports
/// the second `_` as `duplicate_definition`.
#[test]
fn wildcard_formal_parameters_pre_feature_duplicate() {
    let report = report_ops_wildcards(
        "void f(int _, int _) {}\n",
        /* wildcards disabled */ false,
    );
    let dups = dup_messages(&report);
    assert_eq!(
        dups,
        vec!["The name '_' is already defined.".to_string()],
        "pre-feature `_` is an ordinary name: {dups:?}"
    );
}

/// `this._` is a FieldFormalParameterElement — never a wildcard, so the
/// second `this._` duplicates even under the feature (upstream :444-457).
/// Two initializing formals pair on `duplicate_field_formal_parameter`, not
/// `duplicate_definition` (`getDiagnostic` reads both nodes' kinds,
/// duplicate_definition_verifier.dart:437-441); the live 3.13.7 oracle on
/// this exact fixture reports the field-formal row on the second formal —
/// a wildcard reading would emit nothing, so this row is the discriminator.
#[test]
fn this_underscore_formal_is_not_a_wildcard() {
    let report = report_ops_wildcards(
        "class C {\n\
         \x20 int _ = 0;\n\
         \x20 C(this._, {this._ = 0});\n\
         }\n",
        /* wildcards enabled */ true,
    );
    let rows: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "duplicate_field_formal_parameter")
        .map(|d| d.message.to_string())
        .collect();
    assert_eq!(
        rows,
        vec![
            "The field '_' can't be initialized by multiple parameters in the same constructor."
                .to_string()
        ],
        "this._ is a FieldFormal, not a wildcard: {rows:?}"
    );
}

/// `super._` IS a wildcard — the second one stays silent under the feature
/// (upstream `isWildcard` reads the element kind; a `SuperFormalParameter`
/// is no `FieldFormalParameterElement`, :444-457).
#[test]
fn super_underscore_formal_is_a_wildcard() {
    let report = report_ops_wildcards(
        "class B {\n\
         \x20 B(int a, {int b = 0});\n\
         }\n\
         class C extends B {\n\
         \x20 C(super._, {super._ = 0});\n\
         }\n",
        /* wildcards enabled */ true,
    );
    let dups = dup_messages(&report);
    assert!(
        dups.is_empty(),
        "super._ is an ordinary formal and wildcards: {dups:?}"
    );
}

// —— positional/named formal shared scope ——

/// `checkParameters` shares one scope across the whole list: a positional
/// `a` collides with a named `a` (upstream :56-82 — the `parameters_*`
/// census rows report positional-position collisions the named-only scan
/// missed).
#[test]
fn positional_and_named_formal_share_scope() {
    let report = report_ops("void f(int a, {int a = 0}) {}\n");
    let dups = dup_messages(&report);
    assert_eq!(
        dups,
        vec!["The name 'a' is already defined.".to_string()],
        "positional and named formals share the scope: {dups:?}"
    );
}

/// Two positional formals with the same name duplicate (the
/// `parameters_positional_*` census rows).
#[test]
fn two_positional_formals_duplicate() {
    let report = report_ops("void f(int a, int a) {}\n");
    let dups = dup_messages(&report);
    assert_eq!(
        dups,
        vec!["The name 'a' is already defined.".to_string()],
        "positional collision reports once: {dups:?}"
    );
}

// —— type-parameter wildcard gate ——

/// Feature ON: `_, _` type parameters declare nothing and report nothing.
/// Census `typeParameters_*` wildcards rows.
#[test]
fn wildcard_type_parameters_stay_silent() {
    let report = report_ops_wildcards("class C<_, _> {}\n", /* wildcards enabled */ true);
    let dups = dup_messages(&report);
    assert!(
        dups.is_empty(),
        "wildcard type params declare no name: {dups:?}"
    );
}

/// Feature OFF: the second `_` type parameter is an ordinary duplicate.
#[test]
fn wildcard_type_parameters_pre_feature_duplicate() {
    let report = report_ops_wildcards("class C<_, _> {}\n", /* wildcards disabled */ false);
    let dups = dup_messages(&report);
    assert_eq!(
        dups,
        vec!["The name '_' is already defined.".to_string()],
        "pre-feature `_` type param is an ordinary name: {dups:?}"
    );
}

// —— conflicting_static_and_instance: own scope ——

/// A static method over an instance member of the same name reports
/// `'{C}.{name}'` (the own-face arm, census `ConflictingStaticAndInstance
/// ClassTest` rows).
#[test]
fn conflicting_static_method_over_instance_member() {
    let report = report_ops(
        "class C {\n\
         \x20 int foo = 0;\n\
         \x20 static int foo() => 0;\n\
         }\n",
    );
    let rows = conflicting_messages(&report);
    assert_eq!(
        rows,
        vec![
            "Class 'C' can't define static member 'foo' and have instance member \
             'C.foo' with the same name."
                .to_string()
        ],
        "own-scope static vs instance: {rows:?}"
    );
}

/// `values` carries no class-face exemption — a `static int get values`
/// over an instance `values` accessor reports the plain row (the live
/// 3.13.7 oracle fires `conflicting_static_and_instance` on this exact
/// fixture; the `values` carve-outs live in the enhanced-enum machinery,
/// not in the generic member verifier).
#[test]
fn conflicting_static_values_getter_exempt() {
    let report = report_ops(
        "class C {\n\
         \x20 int get values => 0;\n\
         \x20 static int get values => 0;\n\
         }\n",
    );
    let rows = conflicting_messages(&report);
    assert_eq!(
        rows,
        vec![
            "Class 'C' can't define static member 'values' and have instance member \
             'C.values' with the same name."
                .to_string()
        ],
        "a class's values accessor pair conflicts: {rows:?}"
    );
}

// —— conflicting_static_and_instance: extension ——

/// An extension's static-vs-instance collision reports the dedicated
/// one-argument code (upstream `_checkExtensionStatic` :487-522).
#[test]
fn extension_conflicting_static_and_instance() {
    let report = report_ops(
        "class C {}\n\
         extension Ext on C {\n\
         \x20 int foo() => 0;\n\
         \x20 static int foo() => 0;\n\
         }\n",
    );
    let rows = conflicting_messages(&report);
    assert_eq!(
        rows,
        vec![
            "An extension can't define static member 'foo' and an instance member with the same name."
                .to_string()
        ],
        "extension's dedicated arm: {rows:?}"
    );
}

// —— conflicting_static_and_instance: enum getMember + implicit faces ——

/// An enum's static member over its own constant is a plain
/// same-container duplicate — the pair reports `duplicate_definition`,
/// not the static/instance row (the live 3.13.7 oracle on this fixture).
#[test]
fn enum_static_over_own_constant() {
    let report = report_ops(
        "enum E {\n\
         \x20 e1;\n\
         \x20 static int e1 = 0;\n\
         }\n",
    );
    let rows = conflicting_messages(&report);
    // Live 3.13.7 oracle on this fixture: `duplicate_definition` "The name
    // 'e1' is already defined." — the enum constant and the static field
    // are same-container same-name declarations, and the member-duplicate
    // face answers before any static/instance classification, so no
    // `conflicting_static_and_instance` row rides on top.
    let all: Vec<String> = report
        .diagnostics
        .iter()
        .map(|d| d.message.to_string())
        .collect();
    assert_eq!(rows, Vec::<String>::new(), "no static/instance row: {rows:?}");
    assert_eq!(
        all,
        vec!["The name 'e1' is already defined.".to_string()],
        "the pair reports as a plain member duplicate: {all:?}"
    );
}

/// An enum's static `index` collides with the induced instance `index`
/// — the live 3.13.7 oracle renders the owner as `E.index` (the induced
/// getter presents as the enum's own member, not a spelled-out `Enum.`
/// qualifier).
#[test]
fn enum_static_over_implicit_enum_index() {
    let report = report_ops(
        "enum E {\n\
         \x20 e1;\n\
         \x20 static int index = 0;\n\
         }\n",
    );
    let rows = conflicting_messages(&report);
    assert_eq!(
        rows,
        vec![
            "Class 'E' can't define static member 'index' and have instance member \
             'E.index' with the same name."
                .to_string()
        ],
        "the induced index getter renders under the enum's own name: {rows:?}"
    );
}

/// A class's static `toString`/`runtimeType` collides with the implicit
/// `Object` member (the `inSuper_implicitObject` rows).
#[test]
fn class_static_over_implicit_object_member() {
    let report = report_ops(
        "class A {\n\
         \x20 static int toString() => 0;\n\
         }\n",
    );
    let rows = conflicting_messages(&report);
    assert_eq!(
        rows,
        vec![
            "Class 'A' can't define static member 'toString' and have instance member \
             'Object.toString' with the same name."
                .to_string()
        ],
        "implicit Object.toString answers: {rows:?}"
    );
}

// —— conflicting_static_and_instance: inherited arm ——

/// A static member over an INHERITED instance member reports the
/// declaring container (upstream
/// `ErrorVerifier._checkForConflictingClassMembers` — `getInherited`
/// answers the superclass's member, `conflictingClassName` is its owner).
/// Census `ConflictingStaticAndInstanceClassTest`/`MixinTest` inherited
/// rows.
#[test]
fn conflicting_static_over_inherited_instance_member() {
    let report = report_ops(
        "class A {\n\
         \x20 int foo = 0;\n\
         }\n\
         class B extends A {\n\
         \x20 static int foo() => 0;\n\
         }\n",
    );
    let rows = conflicting_messages(&report);
    assert_eq!(
        rows,
        vec![
            "Class 'B' can't define static member 'foo' and have instance member \
             'A.foo' with the same name."
                .to_string()
        ],
        "inherited member names its owner: {rows:?}"
    );
}

/// A mixin's static member over its `on` constraint's instance member
/// reports the constraint's name (`dartCoreEnum_index_*` shape — the
/// constraint face owns the member).
#[test]
fn mixin_static_over_constraint_instance_member() {
    let report = report_ops(
        "class S {\n\
         \x20 int foo = 0;\n\
         }\n\
         mixin M on S {\n\
         \x20 static int foo() => 0;\n\
         }\n",
    );
    let rows = conflicting_messages(&report);
    assert_eq!(
        rows,
        vec![
            "Class 'M' can't define static member 'foo' and have instance member \
             'S.foo' with the same name."
                .to_string()
        ],
        "the on-constraint owns the member: {rows:?}"
    );
}
