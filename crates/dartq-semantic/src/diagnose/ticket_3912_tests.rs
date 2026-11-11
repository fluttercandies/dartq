//! #3912 pins — five `scopes_bodies.rs` faces.
//!
//! (1) A generic function type's OWN binder shadows the use-site
//! substitution: `typedef Foo<T> = int Function<T>(T)` — every `T` leaf
//! inside the right-hand signature is bound by the `<T>` group, never by
//! the alias's own `T`, so `Foo<String>` reads `int Function<T>(T)`
//! verbatim. The pre-fix `closure_subst_param_ty` function arm walked
//! the signature's leaves against the unfiltered map, and the oracle's
//! `'T'`-spelled `return_of_invalid_type_from_closure` came out the
//! written argument's `'String'`. The same class of miss lives in the
//! invocation-side `subst_params` rebuild (foreign scope — see the
//! `#[ignore]` routing pin below).
//!
//! (2) `stage_if_null_result` reads the left operand's last row below the
//! group mark and the right operand's last row on the stack — a compound
//! right side (`fa() ?? fb()`, `a.b ?? f(c)`) leaves more than one row
//! where the two-row gate bailed, so `if_null_results` missed every join
//! a `.member` chain on the group decided from (`(fa() ?? fb()).onlyB()`
//! reports `undefined_method 'A'`). The staged join now also reaches the
//! group: `end_binary_expression` re-keys it onto the folded NODE row,
//! which is the row `end_parenthesized_expression`'s `inner_ty` reads
//! (#3945).
//!
//! (5) A `<T>`-headed literal's own type-parameter group is parsed before
//! `begin_function_expression`, so the capture never counted it and the
//! matched-context gate rejected the context (0 vs 1) — the body reported
//! nothing. The literal now seeds the count from the just-closed window
//! (#3945).
//!
//! (3) `p.C(`'s bare-callee formal table resolves through the prefix's
//! import table (`Receiver::Prefix` arm of `callee_formals`): the
//! unnamed-constructor table of the prefix-resolved container supplies
//! the slot — `p.Box((x){…})` binds `x`'s context the same way `Box(`
//! does.
//!
//! (4) The named-constructor selector keys the container's own
//! parameters (`new C<int>.n((x){…})` binds `x := int` through the
//! `C<T>` environment): `closure_callee_params`'s `ctor_selector_params`
//! arm answers where the member walk never reaches `ctor_formals`, and
//! `closure_param_context_ty`'s head lane reads the `named_head` latch
//! for the dropped-row shapes.
//!
//! Oracle provenance: `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze
//! --format=machine` — `RETURN_OF_INVALID_TYPE_FROM_CLOSURE 'T'` on the
//! shadowed-binder fixture; `UNDEFINED_METHOD 'A'`/`'int'` on the
//! if-null and named-constructor fixtures.

use super::test_rig::{CORE_PRELUDE, RigImport, code_names, report_ops, report_ops_with, rows};
use crate::diagnose::FileReport;

fn messages_of<'a>(report: &'a FileReport, code: &str) -> Vec<&'a str> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.message.as_str())
        .collect()
}

/// (1) The alias's own `T` must not substitute through the RHS's `<T>`
/// binder: `Foo<String> f = <T>(T x) { return x; }` opens the literal
/// under `int Function<T>(T)` — the body returns the inner `T`, which
/// fails the `int` context return, and the oracle's message spells the
/// binder's own `'T'` (never the written `'String'`). The bare
/// function-typed declaration carries the same context — the control
/// asserts the check fires at all under a generic signature.
///
/// The `<T>` group is parsed BEFORE `begin_function_expression`
/// (upstream `parseLiteralListSetMapOrFunction` :8740-8753 runs
/// `parseVariables` first), so the literal's own capture never counted it
/// and the matched-context gate rejected the context (0 vs 1) — the body
/// reported nothing. `begin_function_expression` now seeds the count from
/// the just-closed type-variables window (adjacency-gated on the `>` that
/// sits right before the literal's `(`). The substitution this lane owns
/// stays as documented: `closure_subst_param_ty`'s shadowing guard +
/// `tree_ty`'s arity/named-group preservation for the census's
/// bound-display family (the remaining message-side
/// `'exp2 Function(exp2)'` corruption is invocation.rs `subst_params`'s —
/// the routing pin below).
#[test]
fn generic_literal_return_checks_against_the_context_signature() {
    let report = report_ops(
        "void main() {\n\
         \x20 int Function<T>(T) f = <T>(T x) { return x; };\n\
         \x20 f.toString();\n\
         }\n",
    );
    let hits = messages_of(&report, "return_of_invalid_type_from_closure");
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert!(
        hits[0].contains("'T'"),
        "expected the binder's own 'T' in the message, got {:#?}",
        rows(&report)
    );
}

/// The alias-typed twin: the same `<T>`-headed literal opens under
/// `Foo<String>`'s instantiated `int Function<T>(T)` — the alias's own
/// `T` must not reach through the literal's binder. Same seed as above;
/// the assertion additionally pins that the written `'String'` never
/// leaks into the shadowed signature.
#[test]
fn written_alias_args_never_substitute_a_shadowed_binder() {
    let report = report_ops(
        "typedef Foo<T> = int Function<T>(T);\n\
         void main() {\n\
         \x20 Foo<String> f = <T>(T x) { return x; };\n\
         \x20 f.toString();\n\
         }\n",
    );
    let hits = messages_of(&report, "return_of_invalid_type_from_closure");
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert!(
        hits[0].contains("'T'"),
        "expected the binder's own 'T' in the message, got {:#?}",
        rows(&report)
    );
    assert!(
        !hits[0].contains("'String'"),
        "the written 'String' leaked into the shadowed signature: {:#?}",
        rows(&report)
    );
}

/// (1-control) A non-shadowing alias still substitutes the free spelling:
/// `Bar<T> = T Function(T)` — `Bar<int>` reads `int Function(int)` —
/// the binder-free signature substitutes the alias parameter verbatim.
#[test]
fn written_alias_args_still_substitute_free_spellings() {
    let report = report_ops(
        "typedef Bar<T> = T Function(T);\n\
         void main() {\n\
         \x20 Bar<int> f = (x) { return \"s\"; };\n\
         \x20 f.toString();\n\
         }\n",
    );
    let hits = messages_of(&report, "return_of_invalid_type_from_closure");
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert!(
        hits[0].contains("'String'") && hits[0].contains("'int'"),
        "expected 'String' vs 'int' under the substituted context: {:#?}",
        rows(&report)
    );
}

/// (2) A compound right operand joins the `??` group's static type:
/// `(fa() ?? fb()).onlyB()` where `fa` returns `A?` and `fb` `B`
/// (`B extends A`) — `NonNull(A) ⊔ B` is `A`, so `.onlyB()` reports
/// `undefined_method 'A'` where the two-row gate left the group
/// ungrounded.
///
/// The join now reaches the group: `stage_if_null_result` stages the `A`
/// join on the right operand's row, and `end_binary_expression` re-keys
/// the entry onto the folded NODE row (`fold_binary` collapses both
/// operands into one `Value` row spanning the node, which is the row
/// `end_parenthesized_expression`'s `inner_ty` reads) — so an INVOKED
/// right side, whose pre-fold top row is the callee head rather than a
/// value row, is covered too.
#[test]
fn compound_right_operand_joins_the_if_null_result() {
    let report = report_ops(
        "class A {}\n\
         class B extends A { void onlyB() {} }\n\
         A? fa() => null;\n\
         B fb() => B();\n\
         void main() {\n\
         \x20 var x = (fa() ?? fb()).onlyB();\n\
         \x20 x.toString();\n\
         }\n",
    );
    let hits = messages_of(&report, "undefined_method");
    assert!(
        hits.iter()
            .any(|m| m.contains("'onlyB'") && m.contains("'A'")),
        "expected undefined_method 'onlyB' on 'A': {:#?}",
        rows(&report)
    );
}

/// (2-control) The keep-side: `(fa() ?? fb()).hashCode` — `hashCode`
/// lives on `Object`, so `A`'s face answers it and no member row fires.
#[test]
fn compound_right_operand_join_keeps_object_members_silent() {
    let report = report_ops(
        "class A {}\n\
         class B extends A { void onlyB() {} }\n\
         A? fa() => null;\n\
         B fb() => B();\n\
         void main() {\n\
         \x20 var x = (fa() ?? fb()).hashCode;\n\
         \x20 x.toString();\n\
         }\n",
    );
    assert!(
        !code_names(&report).contains(&"undefined_method"),
        "{:#?}",
        rows(&report)
    );
}

/// (3) A prefix-qualified bare creation callee reads the imported
/// container's unnamed-constructor table: `p.Box((x) { x.noSuch(); })`
/// over `Box(List<int> a)` types `x`'s context `int` — the
/// `undefined_method 'noSuch' on 'int'` row the bare `Receiver::Prefix`
/// arm's `top_level_decl` lane could never answer.
#[test]
fn prefixed_bare_creation_feeds_the_closure_formal() {
    // The import must precede every declaration, so the prelude rides
    // *inside* the source here — `report_ops_with` takes the fixture
    // verbatim (unlike `report_ops_imports`, which prepends it).
    let report = report_ops_with(
        &format!(
            "import 'package:t/a.dart' as p;\n{CORE_PRELUDE}\
             void main() {{\n\
             \x20 p.Box((x) {{ x.noSuch(); }});\n\
             }}\n"
        ),
        &[RigImport {
            uri: "package:t/a.dart",
            prefix: Some("p"),
            source: "class Box {\n  Box(void Function(int) f) {}\n}\n",
        }],
        None,
    );
    let hits = messages_of(&report, "undefined_method");
    assert!(
        hits.iter()
            .any(|m| m.contains("'noSuch'") && m.contains("'int'")),
        "expected undefined_method 'noSuch' on 'int' through p.Box's ctor: {:#?}",
        rows(&report)
    );
}

/// (4) The named-selector creation head binds the closure formal
/// through the container's parameters: `new C<int>.n((x) { x.noSuch();
/// })` — `n`'s `void Function(T)` formal instantiates `T := int` from
/// the written `C<int>` group, so the member check reports
/// `undefined_method 'noSuch' on 'int'`.
#[test]
fn named_ctor_selector_binds_the_closure_formal() {
    let report = report_ops(
        "class C<T> {\n\
         \x20 C.n(void Function(T) f) {}\n\
         }\n\
         void main() {\n\
         \x20 new C<int>.n((x) { x.noSuch(); });\n\
         }\n",
    );
    let hits = messages_of(&report, "undefined_method");
    assert!(
        hits.iter()
            .any(|m| m.contains("'noSuch'") && m.contains("'int'")),
        "expected undefined_method 'noSuch' on 'int' through C<int>.n's ctor env: {:#?}",
        rows(&report)
    );
}

/// (4-keep) The same head keeps a satisfied member silent: `x.toString()`
/// binds `x := int` and `toString` climbs to `Object` — no row. (`abs`
/// sits on `num`, which the prelude stub does not spell, so the stub's
/// `int` cannot answer it — `toString` is the universal member face.)
#[test]
fn named_ctor_selector_keeps_the_satisfied_member_silent() {
    let report = report_ops(
        "class C<T> {\n\
         \x20 C.n(void Function(T) f) {}\n\
         }\n\
         void main() {\n\
         \x20 new C<int>.n((x) { x.toString(); });\n\
         }\n",
    );
    assert!(
        !code_names(&report).contains(&"undefined_method"),
        "{:#?}",
        rows(&report)
    );
}

/// (4-control) A static member on the container receiver still takes the
/// member arm — `C.m((x){…})` binds `x` through `m`'s own signature, not
/// the container's `T`.
#[test]
fn static_member_on_a_container_receiver_keeps_the_member_arm() {
    let report = report_ops(
        "class C<T> {\n\
         \x20 static void m(void Function(String) f) {}\n\
         \x20 C.n(void Function(T) f) {}\n\
         }\n\
         void main() {\n\
         \x20 C.m((x) { x.noSuch(); });\n\
         }\n",
    );
    let hits = messages_of(&report, "undefined_method");
    assert!(
        hits.iter()
            .any(|m| m.contains("'noSuch'") && m.contains("'String'")),
        "expected undefined_method 'noSuch' on 'String' through C.m's member face: {:#?}",
        rows(&report)
    );
}

/// Routed — the invocation-side `subst_params` function arm
/// (invocation.rs) substitutes a bound's WRITTEN `Ty::Function` against
/// the call's `{param: arg}` map with no binder check, and rebuilds via
/// the count-only interner (`intern_function_ty_required`), so the
/// `type_argument_not_matching_bounds` message for
/// `func1<T extends T Function<T extends num>(T)>(); func1<exp2>()`
/// prints `'exp2 Function(exp2)'` where the oracle prints the binder
/// verbatim — `'T Function<T extends num>(T)'` (15 paired rows / 8
/// cases plus 10 unpaired missing in the 2026-11-04 census). Owner:
/// invocation.rs (`subst_params`'s `Ty::Function` arm) — a binder-aware
/// rebuild is that file's fix; this lane's `closure_subst_param_ty`
/// twin landed the same shadowing rule.
#[test]
// un-ignored 2026-11-07: passing per stale-ignore triage (ticket #3973)
fn routed_bound_display_keeps_the_generic_signature_binder() {
    let report = report_ops(
        "typedef exp2 = void Function<T extends num>();\n\
         void func1<T extends T Function<T extends num>(T)>() {}\n\
         void main() {\n\
         \x20 func1<exp2>();\n\
         }\n",
    );
    let hits = messages_of(&report, "type_argument_not_matching_bounds");
    assert!(
        hits.iter()
            .any(|m| m.contains("'T Function<T extends num>(T)'")),
        "expected the binder-verbatim bound 'T Function<T extends num>(T)': {:#?}",
        rows(&report)
    );
}

/// Routed — the var-bind seam's `invalid_assignment` misses
/// (`test_downwardsInferenceOnInstanceCreations_inferDownwards`: eight
/// The var-bind seam's `invalid_assignment` rows
/// (`test_downwardsInferenceOnInstanceCreations_inferDownwards`: the eight
/// `A<int, dynamic>`/`B<String, dynamic>`/`C<dynamic>`/`D<num, dynamic>`
/// rows the oracle reports against `A<int, String>`/`A<int, int>`).
///
/// The read side is verbatim — `bind_inferred_ty` preserves every written
/// group including `dynamic` segments (`instantiated_head_ty` fills only
/// argument-less heads) and `local_declared_ty` returns the stored face —
/// and the ask side now decides them too: the engine CLI reproduces all
/// eight census rows 1:1 against `dart analyze` 3.13.7. The residual was
/// the ask verdict, not the slot, and it closed in `assignability.rs`
/// (a sibling lane's work); this pin keeps the shape covered.
#[test]
fn routed_var_bind_read_preserves_dynamic_written_groups() {
    let report = report_ops(
        "class A<S, T> {\n\
         \x20 A(S s, T t) {}\n\
         }\n\
         void main() {\n\
         \x20 var v = A<int, dynamic>(0, null);\n\
         \x20 A<int, String> a = v;\n\
         }\n",
    );
    let hits = messages_of(&report, "invalid_assignment");
    assert!(
        hits.iter()
            .any(|m| m.contains("'A<int, dynamic>'") && m.contains("'A<int, String>'")),
        "expected invalid_assignment 'A<int, dynamic>' vs 'A<int, String>': {:#?}",
        rows(&report)
    );
}
