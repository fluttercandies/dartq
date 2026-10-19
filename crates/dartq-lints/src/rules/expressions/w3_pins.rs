use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

/// The written-bool operand faces: local, top-level variable,
/// parameter, both-literal and same-file class field all report at
/// the literal (no_literal_bool_comparisons.dart:55-60).
#[test]
fn no_literal_bool_written_bool_faces() {
    let f = Fixture::parse(
        "bool flag = true;\nclass Box { bool flag = true; }\nvoid f(bool b) {\n  bool value = true;\n  Box box = Box();\n  var r1 = value && true;\n  var r2 = true && value;\n  var r3 = b == true;\n  var r4 = flag != false;\n  var r5 = true == true;\n  var r6 = box.flag || true;\n}\n",
    );
    let lints = f.run(&one_rule("no_literal_bool_comparisons"));
    assert_eq!(lints.len(), 6, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unnecessary comparison to a boolean literal."
    );
    // In source order: the literals of r1..r6.
    let starts = [
        tok(&f.source, &f.tokens, "true", 3),  // value && true
        tok(&f.source, &f.tokens, "true", 4),  // true && value
        tok(&f.source, &f.tokens, "true", 5),  // b == true
        tok(&f.source, &f.tokens, "false", 0), // flag != false
        tok(&f.source, &f.tokens, "true", 6),  // true == true
        tok(&f.source, &f.tokens, "true", 8),  // box.flag || true
    ]
    .map(|index| utf16_at(&f, index));
    for (lint, start) in lints.iter().zip(starts) {
        assert_eq!(lint.range.start, start);
    }
}

/// The `bool Function()` invocation face reads the return type; the
/// getter member face reads the same-file class.
#[test]
fn no_literal_bool_function_and_getter_faces() {
    let f = Fixture::parse(
        "bool Function() probe = () => true;\nvoid f() {\n  var r = probe() && true;\n}\n",
    );
    let lints = f.run(&one_rule("no_literal_bool_comparisons"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let t = tok(&f.source, &f.tokens, "true", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, t));

    let f = Fixture::parse(
        "class C {\n  bool get ok => true;\n}\nvoid f(C c) {\n  var r = c.ok || true;\n}\n",
    );
    assert_eq!(
        f.run(&one_rule("no_literal_bool_comparisons")).len(),
        1,
        "{:?}",
        f.run(&one_rule("no_literal_bool_comparisons"))
    );
}

/// Nullable, dynamic, non-bool and unknown operands, the
/// non-comparison `??`, and literal-free comparisons stay silent
/// (:41, :55-60).
#[test]
fn no_literal_bool_silent_faces() {
    let f = Fixture::parse(
        "void f(bool? maybe, dynamic dyn, int n, bool v) {\n  var g1 = maybe ?? true;\n  var g2 = dyn == true;\n  var g3 = v || maybe;\n  var g4 = n == true;\n  var g5 = v == maybe;\n}\n",
    );
    assert!(f.run(&one_rule("no_literal_bool_comparisons")).is_empty());
}
