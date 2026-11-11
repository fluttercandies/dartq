//! Ticket #3944 pins — the header-clause GFT argument's signature-variable
//! stitch.
//!
//! A generalized function type written as a supertype clause's type
//! argument (`class C extends G<void Function<U extends int>()>`) used to
//! lose its binder list twice over: the signature's own `<U extends int>`
//! group fired while the header capture was open, so the bound re-parse
//! fed the header sink's open function level (the bound starved, the
//! sealed node mangled), and `end_function`'s header arm dropped the
//! recorded slice outright. The fix routes an open type-variables window
//! ahead of the header capture for every type event (the window owns
//! everything typed in its span — the same gate `alias_poison` already
//! applied) and stitches the recorded variables onto the sealed node
//! through the sink's newest entry, the exact machinery the bound window
//! (#3886) and the use-site capture (#3899) take. The oracle's
//! bound-check message spells the written argument verbatim with its
//! quantifier (upstream Dart SDK source, `pkg/_fe_analyzer_shared/lib/src/
//! parser/type_info_impl.dart:627-636` — the parser completes a
//! signature's variables group ahead of its formals; `pkg/analyzer/lib/
//! src/dart/element/replace_top_bottom_visitor.dart:56` — a function
//! type's own parameters stay verbatim under top/bottom replacement, so
//! the inverted-pass display keeps the binder list), pinned here against
//! live `dart analyze --format=machine` output per clause position.

use super::test_rig::report_ops;
use crate::FileReport;
use dartq_diagnostics::Diagnostic;

/// Every `type_argument_not_matching_bounds` row of a fixture.
fn bounds_rows(report: &FileReport) -> Vec<&Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "type_argument_not_matching_bounds")
        .collect()
}

/// The `extends` clause: the written GFT argument keeps its binder list in
/// the violation message. Live oracle:
/// `'void Function<U extends int>()' doesn't conform to the bound 'num' of
/// the type parameter 'T'.`
#[test]
fn extends_clause_gft_argument_keeps_its_quantifier() {
    let fixture = "\
class G<T extends num> {}
class C extends G<void Function<U extends int>()> {}
";
    let report = report_ops(fixture);
    let rows = bounds_rows(&report);
    assert_eq!(rows.len(), 1, "exactly one bounds row: {report:#?}");
    assert_eq!(
        rows[0].message.to_string(),
        "'void Function<U extends int>()' doesn't conform to the bound \
         'num' of the type parameter 'T'.",
        "the clause argument must keep its quantifier: {}",
        rows[0].message
    );
}

/// The `implements` clause, same face. Live oracle:
/// `'void Function<V extends String>()' doesn't conform to the bound 'num'
/// of the type parameter 'T'.`
#[test]
fn implements_clause_gft_argument_keeps_its_quantifier() {
    let fixture = "\
class G<T extends num> {}
class C implements G<void Function<V extends String>()> {}
";
    let report = report_ops(fixture);
    let rows = bounds_rows(&report);
    assert_eq!(rows.len(), 1, "exactly one bounds row: {report:#?}");
    assert_eq!(
        rows[0].message.to_string(),
        "'void Function<V extends String>()' doesn't conform to the bound \
         'num' of the type parameter 'T'.",
        "the clause argument must keep its quantifier: {}",
        rows[0].message
    );
}

/// The mixin `on` clause, same face. Live oracle:
/// `'void Function<W extends double>()' doesn't conform to the bound 'num'
/// of the type parameter 'T'.`
#[test]
fn mixin_on_clause_gft_argument_keeps_its_quantifier() {
    let fixture = "\
class G<T extends num> {}
mixin M on G<void Function<W extends double>()> {}
";
    let report = report_ops(fixture);
    let rows = bounds_rows(&report);
    assert_eq!(rows.len(), 1, "exactly one bounds row: {report:#?}");
    assert_eq!(
        rows[0].message.to_string(),
        "'void Function<W extends double>()' doesn't conform to the bound \
         'num' of the type parameter 'T'.",
        "the clause argument must keep its quantifier: {}",
        rows[0].message
    );
}

/// The `with` clause against a bounded mixin, same face. Live oracle:
/// `'void Function<Q extends int>()' doesn't conform to the bound 'num' of
/// the type parameter 'T'.`
#[test]
fn with_clause_gft_argument_keeps_its_quantifier() {
    let fixture = "\
mixin Mg<T extends num> {}
class W extends Object with Mg<void Function<Q extends int>()> {}
";
    let report = report_ops(fixture);
    let rows = bounds_rows(&report);
    assert_eq!(rows.len(), 1, "exactly one bounds row: {report:#?}");
    assert_eq!(
        rows[0].message.to_string(),
        "'void Function<Q extends int>()' doesn't conform to the bound \
         'num' of the type parameter 'T'.",
        "the clause argument must keep its quantifier: {}",
        rows[0].message
    );
}

/// The named mixin application (`class W = ... with ...;`), same face.
/// Live oracle: `'void Function<R extends String>()' doesn't conform to
/// the bound 'num' of the type parameter 'T'.`
#[test]
fn named_mixin_application_gft_argument_keeps_its_quantifier() {
    let fixture = "\
mixin Mg<T extends num> {}
class W = Object with Mg<void Function<R extends String>()>;
";
    let report = report_ops(fixture);
    let rows = bounds_rows(&report);
    assert_eq!(rows.len(), 1, "exactly one bounds row: {report:#?}");
    assert_eq!(
        rows[0].message.to_string(),
        "'void Function<R extends String>()' doesn't conform to the bound \
         'num' of the type parameter 'T'.",
        "the clause argument must keep its quantifier: {}",
        rows[0].message
    );
}

/// An enum's `implements` clause, same face. Live oracle:
/// `'void Function<Y extends bool>()' doesn't conform to the bound 'num'
/// of the type parameter 'T'.`
#[test]
fn enum_implements_clause_gft_argument_keeps_its_quantifier() {
    let fixture = "\
class G<T extends num> {}
enum E implements G<void Function<Y extends bool>()> { one }
";
    let report = report_ops(fixture);
    let rows = bounds_rows(&report);
    assert_eq!(rows.len(), 1, "exactly one bounds row: {report:#?}");
    assert_eq!(
        rows[0].message.to_string(),
        "'void Function<Y extends bool>()' doesn't conform to the bound \
         'num' of the type parameter 'T'.",
        "the clause argument must keep its quantifier: {}",
        rows[0].message
    );
}

/// A GFT argument nested below an argument root (`G<H<...>>`) reaches the
/// nested `NamedType` walk, where the super pass runs and the row spells
/// the INVERTED argument with its binder list (the void return flips to
/// `Never`, the signature's own parameters stay verbatim). Live oracle:
/// `'Never Function<X extends int>()' doesn't conform to the bound 'num'
/// of the type parameter 'T'.`
#[test]
fn nested_argument_gft_keeps_its_quantifier_inverted() {
    let fixture = "\
class G<T> {}
class H<T extends num> {}
class C extends G<H<void Function<X extends int>()>> {}
";
    let report = report_ops(fixture);
    let rows = bounds_rows(&report);
    assert_eq!(rows.len(), 1, "exactly one bounds row: {report:#?}");
    assert_eq!(
        rows[0].message.to_string(),
        "'Never Function<X extends int>()' doesn't conform to the bound \
         'num' of the type parameter 'T'.",
        "the inverted argument keeps the binder list: {}",
        rows[0].message
    );
}

/// The clause GFT's own variable bound is a checked annotation in its own
/// right: the violation below the binder (`H<String>` against `H<T extends
/// num>`) reports, beside the argument row. Live oracle rows:
/// `'void Function<Z extends H<String>>()' doesn't conform to the bound
/// 'num' of the type parameter 'T'.` and `'String' doesn't conform to the
/// bound 'num' of the type parameter 'T'.`
#[test]
fn gft_variable_bound_violation_reports_beside_the_argument_row() {
    let fixture = "\
class G<T extends num> {}
class H<T extends num> {}
class C extends G<void Function<Z extends H<String>>()> {}
";
    let report = report_ops(fixture);
    let rows = bounds_rows(&report);
    let messages: Vec<String> = rows.iter().map(|row| row.message.to_string()).collect();
    assert!(
        messages.contains(&format!(
            "'void Function<Z extends H<String>>()' doesn't conform to \
             the bound 'num' of the type parameter 'T'."
        )),
        "the argument row must keep its quantifier: {messages:#?}"
    );
    assert!(
        messages.contains(&format!(
            "'String' doesn't conform to the bound 'num' of the type \
             parameter 'T'."
        )),
        "the bound annotation below the binder must report: {messages:#?}"
    );
}

/// Control: a clean clause argument and a clean representation type stay
/// silent — the stitch may not invent rows (live oracle: exit 0, no
/// diagnostics).
#[test]
fn clean_clause_gft_argument_stays_silent() {
    let fixture = "\
class A<T> {}
class C extends A<void Function<X extends int>()> {}
extension type Et(void Function<S extends int>() rep) {}
";
    let report = report_ops(fixture);
    assert!(
        bounds_rows(&report).is_empty(),
        "a clean argument reports nothing: {report:#?}"
    );
}

/// Routed residual (AP #3932's A03_t05 row): a constructor tear-off's
/// receiver carries a written, bound-violating type-argument group
/// (`MyList<String>.filled` over `typedef MyList<T extends num> =
/// List<T>`), and the oracle reports the alias-instantiation bounds face
/// at the argument (`'String' doesn't conform to the bound 'num' of the
/// type parameter 'T.'`). An expression head's group enters no capture
/// latch — the recovering byte walk lives in the engine's receiver face,
/// and the bounds verdict wires in there, not in the parse-time capture
/// this file's lane owns. Ignored until that lane lands; the fixture and
/// the oracle-verbatim expectation are the acceptance anchor.
#[test]
#[ignore = "wiring owned by the engine receiver face (ticket #3932 residual): the tear-off receiver's written group recovers there and feeds the bounds alias-instantiation check"]
fn tearoff_receiver_bound_violation_reports() {
    let fixture = "\
typedef MyList<T extends num> = List<T>;

main() {
  var v = MyList<String>.filled;
}
";
    let report = report_ops(fixture);
    let rows = bounds_rows(&report);
    assert_eq!(rows.len(), 1, "exactly one bounds row: {report:#?}");
    assert_eq!(
        rows[0].message.to_string(),
        "'String' doesn't conform to the bound 'num' of the type \
         parameter 'T'.",
        "the tear-off receiver's written argument reports verbatim: {}",
        rows[0].message
    );
}
