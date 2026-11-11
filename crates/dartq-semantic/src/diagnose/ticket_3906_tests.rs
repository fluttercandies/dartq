//! #3906 pins — the E13-2 compound-assignment inference window: a `var`
//! whose initializer is a compound assignment (`var x = Base.log += 'a'`)
//! binds the write-back result the assignability half re-typed onto the
//! window's tail row — upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/resolver/assignment_expression_resolver.dart
//! `_resolveTypes` (`assignedType` = the refined operator return,
//! :301-314, recorded on the assignment node; the variable's initializer
//! inference reads the node's static type). Pre-fix the window had no
//! compound arm, the inferred local bound `Unknown`, and every later
//! write or read stayed silent (oracle-only `invalid_assignment` rows).
//!
//! Oracle rows: dart 3.13.7 (`dart analyze --format=machine`) over the
//! fixtures below.

use super::test_rig::report_ops_experiments_with;

/// The fixture's rows of one code: messages in source order.
fn code_messages(source: &str, wanted: &str) -> Vec<String> {
    let mut rows: Vec<(u32, String)> = report_ops_experiments_with(source, &[], None, &[])
        .diagnostics
        .iter()
        .filter(|d| d.code.name == wanted)
        .map(|d| (d.range.offset, d.message.to_string()))
        .collect();
    rows.sort_by_key(|(offset, _)| *offset);
    rows.into_iter().map(|(_, message)| message).collect()
}

/// The oracle's `int`-to-`String` write row.
const INT_TO_STRING: &str =
    "A value of type 'int' can't be assigned to a variable of type 'String'.";

/// The oracle's `String`-to-`G2` write row.
const STRING_TO_G2: &str = "A value of type 'String' can't be assigned to a variable of type 'G2'.";

// —— #3937 (the annotation-family synthetic-name face, absorbed in-lane) ——

/// The `undefined_annotation` row for a recovered empty annotation name.
const UNDEFINED_EMPTY_ANNOTATION: &str = "Undefined name '' used as an annotation.";

/// A `@` whose identifier the parser recovered (the upstream
/// annotation_test.dart `token: <empty> <synthetic>` shape) still joins the
/// annotation latch, so the oracle's `undefined_annotation` fires against
/// the empty spelling exactly as it does for the census's
/// `partial_code/annotation` ampersand family. Pre-fix the synthetic name
/// was dropped and the row never fired.
#[test]
fn synthetic_metadata_name_reports_undefined_annotation() {
    let source = "\
class C { @ var f; }
";
    assert_eq!(
        code_messages(source, "undefined_annotation"),
        [UNDEFINED_EMPTY_ANNOTATION]
    );
}

/// The local-declaration twin of the same recovered shape.
#[test]
fn synthetic_local_metadata_name_reports_undefined_annotation() {
    let source = "\
f() { @ var x; }
";
    assert_eq!(
        code_messages(source, "undefined_annotation"),
        [UNDEFINED_EMPTY_ANNOTATION]
    );
}

/// A defined annotation stays silent — the synthetic-name face never
/// suppresses a real resolution (`const x = 0; @x var f = 0;` analyzes
/// clean, oracle-pinned).
#[test]
fn defined_annotation_stays_silent() {
    let source = "\
const x = 0;
@x var f = 0;
";
    assert_eq!(
        code_messages(source, "undefined_annotation"),
        Vec::<String>::new()
    );
}

/// A static compound write as the whole initializer: `x` binds the
/// write-back result (`String` — the operator result `String += String`),
/// so the later `int` write reports. Pre-fix `x` bound `Unknown` and the
/// write stayed silent (the oracle's only row is the later write).
#[test]
fn compound_initializer_binds_the_write_back_type() {
    let source = "\
class Base {
  static String log = '';
}

void main() {
  var x = Base.log += 'a';
  x = 42;
}
";
    assert_eq!(code_messages(source, "invalid_assignment"), [INT_TO_STRING]);
}

/// The write-back type is the OPERATOR RETURN, never the right operand:
/// `q += 1` over an `operator +` returning `G2` infers `w` as `G2`, so the
/// `G2` member read stays silent and the later `String` write reports
/// against `G2`. Pairing the right operand's row instead would misfire an
/// `undefined_getter` against `int` — the pin holds both faces apart.
#[test]
fn compound_initializer_binds_the_operator_return() {
    let source = "\
class F2 {
  G2 operator +(int i) => G2();
}

class G2 extends F2 {
  int onlyOnG = 0;
}

void main() {
  F2 q = F2();
  var w = q += 1;
  w.onlyOnG;
  w = 's';
}
";
    assert_eq!(
        code_messages(source, "undefined_getter"),
        Vec::<String>::new()
    );
    assert_eq!(code_messages(source, "invalid_assignment"), [STRING_TO_G2]);
}

/// Control — the postfix increment keeps the single-row arm: `y` binds the
/// operand's own static type (`int`), and the undefined member read fires
/// against it exactly as the oracle's.
#[test]
fn postfix_initializer_keeps_the_single_row_arm() {
    let source = "\
void main() {
  var i = 0;
  var y = i++;
  y.foo;
}
";
    assert_eq!(
        code_messages(source, "undefined_getter"),
        ["The getter 'foo' isn't defined for the type 'int'."]
    );
}

/// Control — the plain-`=` chain keeps the right-row arm: `x` binds the
/// right side's type (`C`), and the undefined member read fires against it
/// exactly as the oracle's.
#[test]
fn plain_assignment_initializer_keeps_the_right_row_arm() {
    let source = "\
class C {}

void main() {
  var a = C();
  var x = a = C();
  x.foo;
}
";
    assert_eq!(
        code_messages(source, "undefined_getter"),
        ["The getter 'foo' isn't defined for the type 'C'."]
    );
}
