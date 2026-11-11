//! #4013 — a typed collection literal initializer's WRITTEN type
//! arguments are the inferred declaration's face.
//!
//! A typed literal whose written group the literal's own row cannot spell
//! (the census spelling `var list2 = <List<int>> [if …]` — a nested group
//! with whitespace before the bracket) is recorded as an unannotated
//! literal, so the inferred local was bound to the element-inference face
//! (`List<List<dynamic>>`, `Set<dynamic>` for `<List<int>> {}`) or left
//! unknown, and every check reading the local's slot answered wrong or
//! stayed silent. The read now spells the initializer from the token
//! stream (`initializer_literal_ty`): upstream the written arguments ARE
//! the literal's static type (`typed_literal_resolver.dart`'s
//! explicit-arguments arms — no inference runs), so `<E>[…]` is `List<E>`,
//! `<E>{…}` `Set<E>`, `<K, V>{…}` `Map<K, V>`, a list with any other arity
//! is `List<dynamic>` (`_resolveListLiteral2`), and an empty annotated
//! literal keeps its written face verbatim (`<List<int>> {}` is a set,
//! never the empty-braces map default). A written `Null` is the null type
//! at every level. Preserved arms: an unannotated literal keeps its
//! inference face, an annotated declarator keeps its declared type, and an
//! initializer the literal does not end (`… [[1]].first`,
//! `… [] as List<Object>`) types a different expression — the slot's own
//! face stands.
//!
//! Oracle provenance: `dart analyze` 3.13.7 over the shapes below.

use super::FileReport;
use super::test_rig::report_ops;

/// The messages of one code's rows, in source order.
fn texts(report: &FileReport, code: &str) -> Vec<String> {
    let mut rows = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .collect::<Vec<_>>();
    rows.sort_by_key(|d| d.range.offset);
    rows.iter().map(|d| d.message.to_string()).collect()
}

/// The typed list local carries its nested written argument through the
/// control-flow elements (the co19 `type_inference_A01` shape) in both
/// spellings — the bracket flush against the group and the census's
/// whitespace before it: the matching checks are constant-true, the
/// mismatched nested argument refuses, and the face reaches the
/// assignability check's display.
#[test]
fn typed_list_local_carries_its_written_nested_args() {
    let report = report_ops(
        "void main() {\n\
         \x20 var l = <List<String>>[if (1 > 0) ['s'] else ['t']];\n\
         \x20 print(l is List<List<String>>);\n\
         \x20 var l2 = <List<String>> [if (1 > 0) ['s'] else ['t']];\n\
         \x20 print(l2 is List<List<String>>);\n\
         \x20 print(l2 is List<List<int>>);\n\
         \x20 var list2 = <List<int>> [\n\
         \x20   if (true) [],\n\
         \x20   if (false) [] else [],\n\
         \x20   for (var i = 0; i < 1; i++) []\n\
         \x20 ];\n\
         \x20 print(list2 is List<List<int>>);\n\
         \x20 int n = l2;\n\
         \x20 print(n);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        3,
        "both spellings of `<List<String>>[…]` and `<List<int>> […]` read \
         their written faces; `List<List<int>>` refuses the `String` local; \
         got {utc:?}"
    );
    assert_eq!(
        texts(&report, "invalid_assignment"),
        vec![
            "A value of type 'List<List<String>>' can't be assigned to a \
             variable of type 'int'."
                .to_string()
        ],
    );
}

/// The typed set and map locals carry their nested written arguments
/// (the co19 `type_inference_A05` shape): one argument is a set, two a
/// map whatever the control-flow entries, and the narrower key column
/// refuses.
#[test]
fn typed_set_and_map_locals_carry_their_written_args() {
    let report = report_ops(
        "void main() {\n\
         \x20 var s = <List<String>> {['a']};\n\
         \x20 print(s is Set<List<String>>);\n\
         \x20 var m = <num, List<int>> {\n\
         \x20   if (true) 11: [],\n\
         \x20   if (false) 1: [] else 22: [],\n\
         \x20   for (var i = 0; i < 1; i++) i: []\n\
         \x20 };\n\
         \x20 print(m is Map<num, List<int>>);\n\
         \x20 print(m is Map<int, List<int>>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        2,
        "`<List<String>> {{…}}` is `Set<List<String>>` and `<num, \
         List<int>> {{…}}` is `Map<num, List<int>>`; the `int` key column \
         refuses; got {utc:?}"
    );
}

/// The `const` spellings, verbatim from the co19 `type_inference_A01`/
/// `A05` const shapes: a `const` riding the literal and a `const`
/// declarator both keep the written face through the control-flow
/// elements (a trailing comma included), the empty `const` literal too,
/// and the narrower key column refuses.
#[test]
fn const_typed_literal_locals_carry_their_written_args() {
    let report = report_ops(
        "class C {}\n\
         void main() {\n\
         \x20 var list3 = const <List<String>> [\n\
         \x20   if (true) [],\n\
         \x20   if (false) [] else [],\n\
         \x20 ];\n\
         \x20 print(list3 is List<List<String>>);\n\
         \x20 const list4 = <List<C>> [\n\
         \x20   if (true) [],\n\
         \x20   if (false) [] else []\n\
         \x20 ];\n\
         \x20 print(list4 is List<List<C>>);\n\
         \x20 var map3 = const <num, List<int>> {\n\
         \x20   if (true) 1: [],\n\
         \x20   if (false) 2: [] else 3: [],\n\
         \x20 };\n\
         \x20 print(map3 is Map<num, List<int>>);\n\
         \x20 const map4 = <num, List<int>> {\n\
         \x20   if (true) 1: [],\n\
         \x20   if (false) 2: [] else 3: [],\n\
         \x20 };\n\
         \x20 print(map4 is Map<num, List<int>>);\n\
         \x20 print(map4 is Map<int, List<int>>);\n\
         \x20 var c = const <List<C>> [];\n\
         \x20 print(c is List<List<C>>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        5,
        "the `const` literals and the `const` declarators keep their written \
         faces; the narrower `int` key column refuses; got {utc:?}"
    );
}

/// An empty annotated literal keeps its written face verbatim:
/// `<List<int>> {}` is `Set<List<int>>` (one written argument decides the
/// set — never the empty-braces `Map<dynamic, dynamic>` default) and
/// `<String, List<int>> {}` is `Map<String, List<int>>`.
#[test]
fn empty_annotated_literals_keep_their_written_face() {
    let report = report_ops(
        "void main() {\n\
         \x20 var e = <List<int>> [];\n\
         \x20 print(e is List<List<int>>);\n\
         \x20 var es = <List<int>> {};\n\
         \x20 print(es is Set<List<int>>);\n\
         \x20 print(es is Map<dynamic, dynamic>);\n\
         \x20 var em = <String, List<int>> {};\n\
         \x20 print(em is Map<String, List<int>>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        3,
        "the empty list, set and map keep their written faces; the set \
         refuses the map default; got {utc:?}"
    );
}

/// The wrong-arity list reads `List<dynamic>` (`_resolveListLiteral2`:
/// any written arity but one is a `dynamic` element), and a written
/// `Null` is the null type at every level — `List<List<Null>>` refuses
/// `List<List<Object>>` (Left Null) the way the declared spelling does.
#[test]
fn wrong_arity_list_and_written_null_follow_the_oracle() {
    let report = report_ops(
        "void main() {\n\
         \x20 var w = <List<int>, String> [];\n\
         \x20 print(w is List<dynamic>);\n\
         \x20 print(w is List<String>);\n\
         \x20 var ln = <List<Null>> [];\n\
         \x20 print(ln is List<List<Null>>);\n\
         \x20 print(ln is List<List<Object>>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        2,
        "`<List<int>, String> []` is `List<dynamic>` and `<List<Null>> []` \
         keeps the null type; the `String`/`Object` asks refuse; got {utc:?}"
    );
}

/// The literal must end the declarator: a cascade keeps its target's
/// type (`..add` returns the list itself) and a later declarator in the
/// group reads its own literal.
#[test]
fn cascade_and_grouped_declarators_keep_the_written_face() {
    let report = report_ops(
        "void main() {\n\
         \x20 var cas = <List<int>> []..add([1]);\n\
         \x20 print(cas is List<List<int>>);\n\
         \x20 var multi = <List<int>> [[1]], other = <Set<String>> {};\n\
         \x20 print(multi is List<List<int>>);\n\
         \x20 print(other is Set<Set<String>>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        3,
        "the cascade target and both grouped declarators keep their written \
         faces; got {utc:?}"
    );
}

/// The negative control: an unannotated literal keeps its inference face
/// (`[1, 2.0]` joins to `List<num>`), an annotated declarator keeps its
/// declared type (`List<Object> a = <List<int>> []` refuses
/// `List<List<int>>`), and an initializer the literal does not end types a
/// different expression — `.first` is a `List<int>`, `as List<Object>` a
/// `List<Object>` — so the written face never leaks onto them.
#[test]
fn unannotated_and_non_literal_initializers_keep_their_faces() {
    let report = report_ops(
        "void main() {\n\
         \x20 var u = [1, 2.0];\n\
         \x20 print(u is List<num>);\n\
         \x20 print(u is List<int>);\n\
         \x20 List<Object> a = <List<int>> [];\n\
         \x20 print(a is List<List<int>>);\n\
         \x20 print(a is List<Object>);\n\
         \x20 var f = <List<int>> [[1]].first;\n\
         \x20 print(f is List<List<int>>);\n\
         \x20 var q = <List<int>> [] as List<Object>;\n\
         \x20 print(q is List<List<int>>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        2,
        "only `u is List<num>` and `a is List<Object>` are constant-true: \
         the written face never replaces an inference face, a declared type, \
         or a non-literal initializer; got {utc:?}"
    );
}
