//! The #2946 pins (C1): the subtype verdict sees the RESOLVED type pair —
//! the destination's typedef-alias head unfolded and the literal's
//! `dynamic`/`Null` argument leaves interned as their dedicated [`Ty`]
//! faces. Two capture gaps starved the ask: a `<dynamic>` literal spelled
//! its argument as `Named{dynamic}`, which every subtype rule reads as an
//! unresolvable head (the covariant element question `dynamic <: int`
//! never ran), and an alias-spelled destination (`AliasToList`,
//! `StrMap<int>`) reached the nominal walk as an unrelated head — the
//! implements walk cannot relate a typedef to `List`/`Map` at all.
//!
//! Upstream premise: the element model never carries a type-alias head
//! into a type question — `named_type_resolver.dart` `_instantiateElement`
//! substitutes the use-site arguments into the alias's right-hand type
//! before `TypeAliasElementImpl.instantiateImpl` returns the use's
//! `TypeImpl`, so `isAssignableTo` in `assignment_expression_resolver.
//! dart` `_checkForInvalidAssignment` (~:793-805) decides over the
//! expanded pair, and `typed_literal_resolver.dart` types the literal
//! against the context's own element type (`_fromContextType`).

use super::async_return_tests::{code_names, report};

/// The rig declares every spelled head (`builtin_types` carries only
/// `dynamic`; `int`/`num`/`String`/`List`/`Map` are final-primitive or
/// collection-head spellings the name-level model resolves locally).
const HEADS: &str = "class int {}\nclass num {}\nclass String {}\n\
                     class List<T> {}\nclass Map<K, V> {}\n";

/// The flat `<dynamic>` literal's argument interns `Ty::Dynamic`, so the
/// covariant element rule decides `dynamic <: int` negatively —
/// `List<int> x = <dynamic>[]` reports `invalid_assignment`.
#[test]
fn dynamic_list_arg_into_written_instantiation_reports() {
    let report = report(&format!("{HEADS}List<int> x = <dynamic>[];"));
    assert_eq!(code_names(&report), ["invalid_assignment"]);
}

/// The map literal's flat group maps the same leaf: `<int, dynamic>`
/// against `Map<int, String>` is the census `invalid_assignment` row.
#[test]
fn dynamic_map_arg_into_written_instantiation_reports() {
    let report = report(&format!("{HEADS}Map<int, String> m = <int, dynamic>{{}};"));
    assert_eq!(code_names(&report), ["invalid_assignment"]);
}

/// The typedef-aliased destination folds for the verdict while the
/// display keeps the written spelling — `AliasToList x = <num>[]` asks
/// `List<num>` against `List<int>` and reports under the alias name.
#[test]
fn typedef_aliased_destination_reports() {
    let report = report(&format!(
        "{HEADS}typedef AliasToList = List<int>;\nAliasToList x = <num>[];"
    ));
    assert_eq!(code_names(&report), ["invalid_assignment"]);
    assert!(
        report.diagnostics[0]
            .message
            .contains("variable of type 'AliasToList'"),
        "{:?}",
        report.diagnostics[0].message
    );
}

/// The parameterized alias's use-site instantiation substitutes into the
/// fold (`StrMap<int>` reads `Map<String, int>`): the mismatched value
/// argument reports.
#[test]
fn parameterized_alias_destination_reports() {
    let report = report(&format!(
        "{HEADS}typedef StrMap<V> = Map<String, V>;\nStrMap<int> m = <String, num>{{}};"
    ));
    assert_eq!(code_names(&report), ["invalid_assignment"]);
}

/// The return face's declared type folds (rtv.dart's `isAssignableTo`
/// ask on the resolved return type): `<num>[]` against an `AliasToList`
/// return reports `return_of_invalid_type`.
#[test]
fn aliased_return_type_reports() {
    let report = report(&format!(
        "{HEADS}typedef AliasToList = List<int>;\nAliasToList f() {{ return <num>[]; }}"
    ));
    assert_eq!(code_names(&report), ["return_of_invalid_type"]);
}

/// The matching-argument control stays silent — the fold must not
/// manufacture mismatches (`AliasToList` IS `List<int>`).
#[test]
fn matching_aliased_destination_stays_silent() {
    let report = report(&format!(
        "{HEADS}typedef AliasToList = List<int>;\nAliasToList x = <int>[];"
    ));
    assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
}

/// A `num` argument is a legitimate element of `List<num>` — the
/// downward-inference positive control keeps its clean verdict.
#[test]
fn wider_argument_stays_silent() {
    let report = report(&format!("{HEADS}List<num> x = <num>[];"));
    assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
}

/// The argument face's alias formal — `argument_type_not_assignable`
/// against an `AI`/`AliasToList` formal — is the same verdict the
/// assignment and return faces already reach through the fold. It is
/// gated OFF upstream of the matrix: the invocation walk's
/// `unbound_formal` latch (invocation.rs's send-args arm, the
/// `Ty::Named { args: EMPTY }` leaf check) reads `resolve_container` —
/// a typedef is not a `Container`, so the alias head answers `None`
/// and the row suppresses as a "raw-type erasure" before
/// [`Engine::argument_verdict`] runs. The verdict itself already
/// folds correctly (`String <: AI` re-decides `String <: int`); the
/// follow-up folds `formal_ty` for the unbound check only — C2's
/// invocation.rs lane owns that site.
#[test]
fn aliased_formal_argument_reports() {
    let report = report(&format!(
        "{HEADS}typedef AliasToList = List<int>;\n\
         void take(AliasToList l) {{}}\n\
         void f() {{ take(<num>[]); }}"
    ));
    assert_eq!(code_names(&report), ["argument_type_not_assignable"]);
}

/// Same gated face through a declared variable — parked with
/// `aliased_formal_argument_reports` on the C2 latch.
#[test]
fn aliased_formal_variable_argument_reports() {
    let report = report(&format!(
        "{HEADS}typedef AliasToList = List<int>;\n\
         void take(AliasToList l) {{}}\n\
         void f() {{ List<num> x = <num>[]; take(x); }}"
    ));
    assert_eq!(code_names(&report), ["argument_type_not_assignable"]);
}
