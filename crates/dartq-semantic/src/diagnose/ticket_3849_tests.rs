//! #3849 pins — an integer literal in a `double` context through `?:`.
//!
//! Upstream types the literal, not the join: `analyzeIntLiteral`
//! (`_fe_analyzer_shared/lib/src/type_inference/type_analyzer.dart:950-967`)
//! reads the literal's CONTEXT SCHEMA and answers `double` whenever the
//! schema does not satisfy `int` but does satisfy `double`. A conditional is
//! analyzed under that same context, so `double s = c ? 42 : 3.14` joins
//! `double`/`double` and the assignment is clean; the row model, which joins
//! a conditional's arms upward before the context is known, joined
//! `int`/`double` into `num` and reported `num` against `double`.
//!
//! Oracle (3.13.7, `dart analyze --format=machine` over a single-file
//! package), verbatim rows for the fixtures below:
//!
//! ```text
//! // double s2 = (c ? 42 : 3.14);        double? n1 = (c ? 42 : 3.14);
//! // double e = (c ? 42 : 0);            double r = (c ? 3.14 : 42);
//! // double b = (c ? (c2 ? 42 : 7) : 3.14);
//! // field/global/top-level declarators, this.f = …, obj.f = …, loc = …
//! (no rows)
//! // int i = (c ? 42 : 3.14);
//! ERROR|COMPILE_TIME_ERROR|INVALID_ASSIGNMENT|...|A value of type 'num'
//!   can't be assigned to a variable of type 'int'.
//! // num n = 1; double d = (c ? 42 : n);
//! ERROR|COMPILE_TIME_ERROR|INVALID_ASSIGNMENT|...|A value of type 'num'
//!   can't be assigned to a variable of type 'double'.
//! ```
//!
//! The second row is the discriminating one: the conversion keys on the
//! ARM's kind, so a `num`-typed arm keeps the `num` join. "int literals are
//! always lenient" would silence a real oracle row here.

use super::FileReport;
use super::test_rig::report_ops;

/// Every `invalid_assignment` message in report order, with the analyzer's
/// trailing space trimmed (the machine format appends one; the wording and
/// both type spellings stay byte-exact).
fn assignment_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == "invalid_assignment")
        .map(|row| row.message.to_string().trim_end().to_string())
        .collect()
}

/// The declaration face: a `double`/`double?` annotation is the context that
/// types the conditional's arms, so an int literal arm converts and the
/// declarator is clean — local, field, and top-level groups alike, with the
/// arms in either order, a nested conditional, and an all-int-literal pair
/// (`d` is `double` there too: BOTH arms convert).
#[test]
fn ticket_3849_double_context_conditional_declarations_stay_silent() {
    let report = report_ops(
        "double g2 = (1 > 0 ? 42 : 3.14);\n\
         class C {\n\
         \x20 double m2 = (1 > 0 ? 42 : 3.14);\n\
         \x20 double? m3;\n\
         \x20 void instanceMethod(bool c, bool c2) {\n\
         \x20   double s2 = (c ? 42 : 3.14);\n\
         \x20   double? n1 = (c ? 42 : 3.14);\n\
         \x20   double r = (c ? 3.14 : 42);\n\
         \x20   double e = (c ? 42 : 0);\n\
         \x20   double b = (c ? (c2 ? 42 : 7) : 3.14);\n\
         \x20   print([s2, n1, r, e, b]);\n\
         \x20 }\n\
         }\n\
         void main() {\n\
         \x20 double l2 = (1 > 0 ? 42 : 3.14);\n\
         \x20 print([l2, g2]);\n\
         }\n",
    );
    assert_eq!(
        assignment_rows(&report),
        Vec::<String>::new(),
        "declaration face should accept a double-context conditional"
    );
}

/// The reverse direction: an `int` annotation does not satisfy the schema
/// test (`intType` IS satisfied there), so the int literal keeps `int`, the
/// arm pair joins to `num`, and `num` into `int` reports — the 3.13.7 row
/// verbatim.
#[test]
fn ticket_3849_int_context_conditional_still_reports() {
    let report = report_ops(
        "void main() {\n\
         \x20 bool c = true;\n\
         \x20 int i = (c ? 42 : 3.14);\n\
         \x20 print(i);\n\
         }\n",
    );
    assert_eq!(
        assignment_rows(&report),
        ["A value of type 'num' can't be assigned to a variable of type 'int'."],
    );
}

/// The conversion keys on the ARM, never on "an int literal is somewhere in
/// the expression": a `num`-typed arm keeps the `num` join, so `double d =
/// c ? 42 : n` reports — an approximation that widened every int literal
/// would trade this oracle row for the silenced one above.
#[test]
fn ticket_3849_num_armed_conditional_still_reports() {
    let report = report_ops(
        "void main() {\n\
         \x20 bool c = true;\n\
         \x20 num n = 1;\n\
         \x20 double d = (c ? 42 : n);\n\
         \x20 print(d);\n\
         }\n",
    );
    assert_eq!(
        assignment_rows(&report),
        ["A value of type 'num' can't be assigned to a variable of type 'double'."],
    );
}

/// The write face (`=`) reads the same context: `this.f = …`, `obj.f = …`
/// and a plain local write into a `double`/`double?` slot are all clean,
/// while the `num`-armed twin still reports through that face too.
#[test]
fn ticket_3849_double_context_conditional_writes_stay_silent() {
    let report = report_ops(
        "class C {\n\
         \x20 double? m1;\n\
         \x20 double m2 = 0;\n\
         \x20 void instanceMethod(bool c) {\n\
         \x20   this.m1 = (c ? 42 : 3.14);\n\
         \x20   this.m2 = (c ? 42 : 3.14);\n\
         \x20   double loc = 0;\n\
         \x20   loc = (c ? 42 : 3.14);\n\
         \x20   C o = C();\n\
         \x20   o.m2 = (c ? 42 : 3.14);\n\
         \x20   print([m1, m2, loc, o]);\n\
         \x20 }\n\
         }\n\
         void main() { C().instanceMethod(true); }\n",
    );
    assert_eq!(
        assignment_rows(&report),
        Vec::<String>::new(),
        "write face should accept a double-context conditional"
    );
}

/// The same write face keeps the `num`-armed rejection.
#[test]
fn ticket_3849_num_armed_conditional_write_still_reports() {
    let report = report_ops(
        "class C {\n\
         \x20 double? m1;\n\
         \x20 void instanceMethod(bool c, num n) {\n\
         \x20   this.m1 = (c ? 42 : n);\n\
         \x20   print(m1);\n\
         \x20 }\n\
         }\n\
         void main() { C().instanceMethod(true, 1); }\n",
    );
    assert_eq!(
        assignment_rows(&report),
        ["A value of type 'num' can't be assigned to a variable of type 'double?'."],
    );
}
