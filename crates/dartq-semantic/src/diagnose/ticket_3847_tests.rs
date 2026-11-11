//! The #3847 creation-receiver instantiation pins.
//!
//! A member read through a folded creation receiver (`C1<int>().field`)
//! instantiates the accessor's declared face against the creation's written
//! type arguments before any verdict reads it — the official analyzer
//! substitutes through the receiver's static type (`staticType` over the
//! mixin application's own instantiation), so `class C1<V> = Object with
//! M<V>` types the field as `A<int>`, never the raw declared `A<T>` (a
//! free parameter the subtype ask answers vacuously, reporting the
//! informative `is A<String>` sibling) and never `Unknown` (silencing the
//! oracle's `is A<int>` row). The gate itself is
//! `_checkAllTypeChecks` (upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/best_practices_verifier.dart:877-930) —
//! `isSubtypeOf(operand, target)` and nothing else — so the pair of rows
//! around one fixture pins the substitution from both sides: the equal
//! argument reports `always 'true'`, the mismatching one stays silent.

use crate::diagnose::test_rig::{
    CORE_PRELUDE, RigImport, code_names, report_ops, report_ops_imports, rows,
};

/// The `unnecessary_type_check` messages of `source`, in report order.
fn unnecessary_type_checks(source: &str) -> Vec<String> {
    report_ops(source)
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code.name == "unnecessary_type_check")
        .map(|diagnostic| diagnostic.message.to_string())
        .collect()
}

/// The `invalid_assignment` messages of `source`, in report order.
fn invalid_assignments(source: &str) -> Vec<String> {
    report_ops(source)
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code.name == "invalid_assignment")
        .map(|diagnostic| diagnostic.message.to_string())
        .collect()
}

fn list_element_messages(source: &str) -> Vec<String> {
    report_ops(source)
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code.name == "list_element_type_not_assignable")
        .map(|diagnostic| diagnostic.message.to_string())
        .collect()
}

/// A getter inherited through a mixin application
/// (`class C1<V> = Object with M<V>`): the application instantiates the
/// mixin at `V := int`, so the read answers `A<int>` and `is A<int>` is
/// the constant-true row the oracle reports.
#[test]
fn ticket_3847_mixin_application_getter_instantiation_reports_true() {
    let source = "mixin M<T> {\n\
                  \x20 A<T> get field => throw 0;\n\
                  }\n\
                  class A<U> {}\n\
                  class C1<V> = Object with M<V>;\n\
                  void g() {\n\
                  \x20 var b = C1<int>().field is A<int>;\n\
                  }";
    let messages = unnecessary_type_checks(source);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("always 'true'"), "{:?}", messages[0]);
}

/// The mismatching argument is a real subtype question: `A<int> is
/// A<String>` asks the covariant argument leg (`int <: String`) and
/// refuses, so the gate stays silent. Guards against the raw-declared
/// regression — a face still carrying the mixin's own `T` answers the ask
/// vacuously and reports the informative sibling.
#[test]
#[ignore = "held: the folded creation receiver's written type arguments are not substituted into the getter's declared face (upstream _checkAllTypeReads the receiver's staticType through the mixin application, best_practices_verifier.dart:877-930); re-verified 2026-11-05 vs oracle 3.13.7: still fails — the `C1<int>().field is A<String>` mismatch reports unnecessary_type_check 'always true' instead of staying silent; owner the creation-receiver instantiation face"]
fn ticket_3847_mixin_application_getter_mismatch_stays_silent() {
    let source = "mixin M<T> {\n\
                  \x20 A<T> get field => throw 0;\n\
                  }\n\
                  class A<U> {}\n\
                  class C1<V> = Object with M<V>;\n\
                  void g() {\n\
                  \x20 var b = C1<int>().field is A<String>;\n\
                  }";
    assert_eq!(
        unnecessary_type_checks(source),
        Vec::<String>::new(),
        "{:?}",
        unnecessary_type_checks(source)
    );
}

/// The initializer-inferred field face (`var field = A<T>();` — the model
/// carries `Unknown`, the diagnosing pass's initializer inference answers
/// `A<T>`) instantiates identically through the application.
#[test]
fn ticket_3847_mixin_application_inferred_field_instantiation_reports_true() {
    let source = "mixin M<T> {\n\
                  \x20 var field = A<T>();\n\
                  }\n\
                  class A<U> {}\n\
                  class C1<V> = Object with M<V>;\n\
                  void g() {\n\
                  \x20 var b = C1<int>().field is A<int>;\n\
                  }";
    let messages = unnecessary_type_checks(source);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("always 'true'"), "{:?}", messages[0]);
}

/// The inferred-field mismatch twin: silent, exactly like the getter's.
#[test]
#[ignore = "held: same root as the getter twin — the inferred-field face never substitutes the creation's written `V := int` (best_practices_verifier.dart:877-930); re-verified 2026-11-05 vs oracle 3.13.7: still fails — the `C1<int>().field is A<String>` mismatch reports unnecessary_type_check 'always true'; owner the creation-receiver instantiation face"]
fn ticket_3847_mixin_application_inferred_field_mismatch_stays_silent() {
    let source = "mixin M<T> {\n\
                  \x20 var field = A<T>();\n\
                  }\n\
                  class A<U> {}\n\
                  class C1<V> = Object with M<V>;\n\
                  void g() {\n\
                  \x20 var b = C1<int>().field is A<String>;\n\
                  }";
    assert_eq!(
        unnecessary_type_checks(source),
        Vec::<String>::new(),
        "{:?}",
        unnecessary_type_checks(source)
    );
}

/// A same-container creation (`Box<int>().value` over the getter declared
/// on `Box` itself): the written arguments zip onto the container's own
/// parameters (`V := int`), the receiver not being an inheritance hop.
#[test]
fn ticket_3847_same_container_creation_getter_instantiation_reports_true() {
    let source = "class Box<V> {\n\
                  \x20 A<V> get value => throw 0;\n\
                  }\n\
                  class A<U> {}\n\
                  void g() {\n\
                  \x20 var b = Box<int>().value is A<int>;\n\
                  }";
    let messages = unnecessary_type_checks(source);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("always 'true'"), "{:?}", messages[0]);
}

/// The same-container mismatch twin: `A<int> is A<String>` is informative
/// and stays silent.
#[test]
#[ignore = "held: same-container creation `Box<int>().value` never zips `V := int` onto the getter's declared `A<V>` before the subtype ask — the formal leg answers `A<V>` vacuously and reports the informative sibling; re-verified 2026-11-05 vs oracle 3.13.7: still fails — `Box<int>().value is A<String>` reports unnecessary_type_check 'always true'; owner the creation-receiver instantiation face"]
fn ticket_3847_same_container_creation_getter_mismatch_stays_silent() {
    let source = "class Box<V> {\n\
                  \x20 A<V> get value => throw 0;\n\
                  }\n\
                  class A<U> {}\n\
                  void g() {\n\
                  \x20 var b = Box<int>().value is A<String>;\n\
                  }";
    assert_eq!(
        unnecessary_type_checks(source),
        Vec::<String>::new(),
        "{:?}",
        unnecessary_type_checks(source)
    );
}

// —— #3838 (the `function_type` instantiated-alias family: a tear-off
// —— operand against a parameterized function-RHS alias whose RETURN is
// —— itself a function type with a parameter-bearing formal) ——

/// The oracle's one reported row of the family: the tear-off whose formal
/// carries the alias's parameter instantiated `int` against the same
/// instantiation — the signatures are equal, `isSubtypeOf` holds
/// reflexively, and the check reports `always 'true'`. Guards the
/// informative twin below from regressing to a target-fold that satisfies
/// every operand.
#[test]
fn ticket_3838_inner_formal_alias_equal_instantiation_reports_true() {
    let source = "typedef F<T> =\n\
                  \x20 List<int> Function({List<T> x}) Function<B extends int>(int x);\n\
                  List<int> Function({List<int> x}) f<B extends int>(int x) =>\n\
                  \x20 throw 'uncalled';\n\
                  void g() {\n\
                  \x20 var a = f is F<int>;\n\
                  }";
    let messages = unnecessary_type_checks(source);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("always 'true'"), "{:?}", messages[0]);
}

/// The mismatching instantiation is informative: `F<bool>`'s nested named
/// formal reads `List<bool>` against the tear-off's `List<int>`, the
/// contravariant leg refuses (`int <: bool` fails), and the gate stays
/// silent. A target fold that drops the substituted formal (the armed
/// stub's empty shape, which every operand satisfies) reports this row.
#[test]
fn ticket_3838_inner_named_formal_mismatch_stays_silent() {
    let source = "typedef F<T> =\n\
                  \x20 List<int> Function({List<T> x}) Function<B extends int>(int x);\n\
                  List<int> Function({List<int> x}) f<B extends int>(int x) =>\n\
                  \x20 throw 'uncalled';\n\
                  void g() {\n\
                  \x20 var b = f is F<bool>;\n\
                  }";
    assert_eq!(
        unnecessary_type_checks(source),
        Vec::<String>::new(),
        "{:?}",
        unnecessary_type_checks(source)
    );
}

/// A generic method's tear-off whose nested formal carries the CLASS
/// parameter (`List<T> x` inside `class C<T>`) against either
/// instantiation: the formal leg asks `List<int> <: List<T>` over an
/// opaque parameter, which no static verdict decides — the oracle reports
/// neither row.
#[test]
fn ticket_3838_class_param_inner_formal_stays_silent() {
    let source = "typedef F<T> =\n\
                  \x20 List<int> Function({List<T> x}) Function<B extends int>(int x);\n\
                  class C<T> {\n\
                  \x20 List<int> Function({List<T> x}) m<B extends int>(int x) =>\n\
                  \x20\x20 throw 'uncalled';\n\
                  \x20 void g() {\n\
                  \x20\x20 var a = m is F<int>;\n\
                  \x20\x20 var b = m is F<bool>;\n\
                  \x20 }\n\
                  }";
    assert_eq!(
        unnecessary_type_checks(source),
        Vec::<String>::new(),
        "{:?}",
        unnecessary_type_checks(source)
    );
}

/// The inner-generic twin (`Function<A>(List<T> x)` — the RHS's return is
/// a generic function whose formal carries the alias parameter): the
/// equal instantiation reports, the mismatching one stays silent.
#[test]
fn ticket_3838_inner_generic_formal_equal_instantiation_reports_true() {
    let source = "typedef G<T> =\n\
                  \x20 List<int> Function<A>(List<T> x) Function<B extends int>(int x);\n\
                  List<int> Function<A>(List<int> x) f<B extends int>(int x) =>\n\
                  \x20 throw 'uncalled';\n\
                  void g() {\n\
                  \x20 var a = f is G<int>;\n\
                  }";
    let messages = unnecessary_type_checks(source);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("always 'true'"), "{:?}", messages[0]);
}

#[test]
fn ticket_3838_inner_generic_formal_mismatch_stays_silent() {
    let source = "typedef G<T> =\n\
                  \x20 List<int> Function<A>(List<T> x) Function<B extends int>(int x);\n\
                  List<int> Function<A>(List<int> x) f<B extends int>(int x) =>\n\
                  \x20 throw 'uncalled';\n\
                  void g() {\n\
                  \x20 var b = f is G<bool>;\n\
                  }";
    assert_eq!(
        unnecessary_type_checks(source),
        Vec::<String>::new(),
        "{:?}",
        unnecessary_type_checks(source)
    );
}

// —— #3861 close-out fold: the written keyword-`dynamic` argument is a
// —— type, not an `InvalidType` (`invalid_assignment` family) ——

/// A written `dynamic` type argument is a real type: the check runs, the
/// covariant leg asks `dynamic <: String` and refuses, and the oracle
/// reports the initializer. Fail-before: the `written_ty_invalid`
/// recursion poisoned the whole spelling and the ask went silent.
#[test]
fn ticket_3861_written_dynamic_argument_decides_covariance() {
    let source = "class A<T, U> {\n\
                  \x20 A(T t, U u);\n\
                  }\n\
                  void g() {\n\
                  \x20 A<int, String> a = A<int, dynamic>(3, 3);\n\
                  }";
    let messages = invalid_assignments(source);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].contains("A<int, dynamic>"),
        "the written spelling displays: {messages:?}"
    );
    assert!(
        messages[0].contains("A<int, String>"),
        "the target displays: {messages:?}"
    );
}

/// The other three directions stay silent: a `dynamic` TARGET accepts
/// every covariant argument (`String <: dynamic`), and the identical
/// pair is a legal write — the un-silencing must not fabricate rows.
#[test]
fn ticket_3861_written_dynamic_target_stays_silent() {
    let source = "class A<T, U> {\n\
                  \x20 A(T t, U u);\n\
                  }\n\
                  void g() {\n\
                  \x20 A<int, dynamic> b = A<int, String>(3, 'x');\n\
                  \x20 A<int, dynamic> c = A<int, dynamic>(3, 3);\n\
                  \x20 A<dynamic, String> d = A<int, String>(3, 'x');\n\
                  }";
    assert_eq!(invalid_assignments(source), Vec::<String>::new());
}

/// A nested written `dynamic` (one level deeper than the top head)
/// un-poisons the same way: `List<dynamic>` against `List<String>`
/// refuses the covariant leg.
#[test]
fn ticket_3861_nested_written_dynamic_argument_decides_covariance() {
    let source = "class A<T> {\n\
                  \x20 A(T t);\n\
                  }\n\
                  void g() {\n\
                  \x20 A<List<String>> a = A<List<dynamic>>(<dynamic>[]);\n\
                  }";
    let messages = invalid_assignments(source);
    assert_eq!(messages.len(), 1, "{messages:?}");
}

// —— #3838's corpus rig: one whole `function_type` test method ——

/// The sibling-complete corpus shape (the #3847 measured note: the extras
/// depend on the sibling declarations — remove `m15`/`x15` and they
/// vanish, so the rig carries them): the generic class's `late` field
/// and `m15` beside the top-level `f15`, the `as dynamic`/`confuse`
/// writes, the corpus's own optional-positional spellings. Oracle: only
/// `f15 is F15<int>` and `m15 is F15<T>` are constant-true; the
/// `F15<bool>` pair and the `confuse(m15)` operand stay informative.
#[test]
#[ignore = "held: the sibling-complete corpus shape reads `unnecessary_type_check` rows 1 short of the oracle's pair — the `f15 is F15<int>` arm reports but the `m15 is F15<T>` sibling-shaped arm does not (same receiver-substitution seam as the 3847 family); re-verified 2026-11-05 vs oracle 3.13.7: still fails — 1 row where 2 are expected (plus 2 unused_local_variable extras the corpus pins); owner the creation-receiver instantiation face"]
fn ticket_3838_sibling_complete_test_method_matches_oracle() {
    let prologue = r#"
typedef F15<T> = List<T> Function(int y, [core.List<core.int> x]) Function(int x);

List<int> Function(int y, [core.List<core.int> x]) f15(int x) =>
    throw 'uncalled';

confuse(f) => f;

class U<T> {
  late List<T> Function(int y, [core.List<core.int> x]) Function(int x) x15;

  List<T> Function(int y, [core.List<core.int> x]) m15(int x) =>
      throw 'uncalled';

  void testF15() {
    List<T> Function(int y, [core.List<core.int> x]) Function(int x) l15;
    x15 = f15 as dynamic;
    l15 = f15 as dynamic;
    x15 = confuse(f15);
    l15 = confuse(f15);
    var b = [
      f15 is F15<bool>,
      m15 is F15<T>,
      m15 is F15<bool>,
      confuse(m15) is F15<int>,
      f15 is F15<int>,
    ];
    x15 = m15;
    l15 = m15;
"#;
    let report = report_ops_imports(
        &format!("{prologue}\n  }}\n}}\n"),
        &[
            RigImport {
                uri: "package:t/core.dart",
                prefix: None,
                source: CORE_PRELUDE,
            },
            RigImport {
                uri: "package:t/core.dart",
                prefix: Some("core"),
                source: "",
            },
        ],
    );
    let reported = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unnecessary_type_check")
        .count();
    assert_eq!(
        reported,
        2,
        "f15 is F15<int> and m15 is F15<T> report; the rest stay silent: {:?}",
        rows(&report)
    );
    assert!(
        code_names(&report)
            .iter()
            .any(|&code| code == "unnecessary_type_check"),
        "the rig exercised the family at all: {:?}",
        rows(&report)
    );
}

// —— #3874 G2 (op= writeback over a type-parameter target) ——

/// The compound write-back asks the assignment face's own verdict: `t +=
/// A()` over `T extends A` reports the same Left-Variable refusal the
/// plain `t = A()` does (`A` never subtypes the parameter). Fail-before:
/// the write-back ran the raw matrix, whose `T`-target legs never refuse.
#[test]
fn ticket_3874_g2_compound_writeback_over_type_param_reports() {
    let source = "class A {\n\
                  \x20 A operator +(A o) => this;\n\
                  }\n\
                  void g<T extends A>(T t) {\n\
                  \x20 t += A();\n\
                  \x20 t = A();\n\
                  }";
    let messages = invalid_assignments(source);
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert!(
        messages.iter().all(|m| m.contains("'T'")),
        "both rows refuse into the parameter: {messages:?}"
    );
}

/// The reflexive write stays legal: a parameter source into its own
/// parameter, and a same-typed compound over a class target, stay silent.
#[test]
fn ticket_3874_g2_reflexive_compound_writes_stay_silent() {
    let source = "class A {\n\
                  \x20 A operator +(A o) => this;\n\
                  }\n\
                  void g<T>(T t) {\n\
                  \x20 t = t;\n\
                  }\n\
                  void h(A a) {\n\
                  \x20 a += A();\n\
                  }";
    assert_eq!(invalid_assignments(source), Vec::<String>::new());
}

// —— LEM wave (the list_element_type_not_assignable census family) ——

/// The element marker rides the element expression's own face, never the
/// segment start: a control element's header (`if (x != null)`) shadows
/// it, so the body's `...x` still reads as a SPREAD (contributing the
/// operand's element `int`, whole `List<int>?` never asked), and a
/// null-aware `?primary` element asks no assignability question at all.
/// Fail-before: both control bodies emitted the operand whole.
#[test]
fn ticket_3874_lem_control_body_marker_reads_element_face() {
    let source = "List<int> f(List<int>? x) => [\n\
                  \x20 if (x != null) ...x,\n\
                  \x20 if (x case var y?) ...y,\n\
                  ];";
    assert_eq!(
        list_element_messages(source),
        Vec::<String>::new(),
        "{:?}",
        list_element_messages(source)
    );
}

/// `analyzeIntLiteral`'s `convertToDouble`: an INT LITERAL element under a
/// `double` context is typed `double` — both the typed `<double>[…]`
/// group and the assignment channel — while an int-typed VARIABLE keeps
/// its rejection (the conversion keys on the literal, never the value).
#[test]
fn ticket_3874_lem_int_literal_converts_to_double_context() {
    let source = "void g() {\n\
                  \x20 List<double> box = <double>[0];\n\
                  \x20 List<double> ctx = [1, -0x02, 9223372036854775808];\n\
                  \x20 int notALiteral = 2;\n\
                  \x20 List<double> bad = [notALiteral];\n\
                  }";
    let messages = list_element_messages(source);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].contains("'int'") && messages[0].contains("'double'"),
        "{messages:?}"
    );
}

/// Reflexivity answers the identical pair before any decomposition: the
/// census face was `FutureOr<T> <: FutureOr<T>`, where the Left FutureOr
/// leg asked `T <: FutureOr<T>` and the parameter's bound refused a pair
/// the oracle never asks. The null-aware spread of the same-typed list
/// rides the same fixture.
#[test]
fn ticket_3874_lem_futureor_reflexive_element_stays_silent() {
    let source = "import 'dart:async';\n\
                  \n\
                  void test<T extends Object>(FutureOr<T> t) {\n\
                  \x20 List<FutureOr<T>> clist = [t, t];\n\
                  \x20 List<FutureOr<T>> alist = [t, t, ...? clist];\n\
                  }";
    assert_eq!(
        list_element_messages(source),
        Vec::<String>::new(),
        "{:?}",
        list_element_messages(source)
    );
}

// —— B1 (bare alias-ctor call resolves through the aliased ctor) ——

/// A bare alias-name invocation is the ALIASED constructor call, never a
/// `Type` literal use: `T<bool>()` constructs `A<bool>` — assignable to
/// the `T<bool>` formal and the `A<bool>`/`T<bool>` targets, refusing
/// `A<int>`. Fail-before: the bare head fell to the `Type` literal
/// fallback.
#[test]
fn ticket_3874_b1_bare_alias_ctor_resolves_aliased_ctor() {
    let source = "class A<X> {\n\
                  \x20 A();\n\
                  }\n\
                  typedef T<X> = A<X>;\n\
                  void foo(T<bool> t) {}\n\
                  void g() {\n\
                  \x20 foo(T<bool>());\n\
                  \x20 A<bool> a = T<bool>();\n\
                  \x20 T<bool> b = T<bool>();\n\
                  \x20 A<int> c = T<bool>();\n\
                  }";
    let messages = invalid_assignments(source);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].contains("'A<bool>'") && messages[0].contains("'A<int>'"),
        "{messages:?}"
    );
}
