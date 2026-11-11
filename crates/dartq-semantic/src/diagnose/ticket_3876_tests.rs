//! #3876 pins — the augment chain's merged positional-parameter scope:
//! per position, the first non-`_` spelling along the chain names the slot
//! and EVERY body on the chain (intro + augments) resolves against it, so
//! an intro body reads names spelled only in a later augment and an
//! augment body reads ones spelled only in the intro; a position spelled
//! `_` on every fragment contributes no name and the read falls through.
//!
//! Pre-fix, each fragment's body bound only its own formals, so a body
//! referencing another fragment's positional name fell through to the
//! library face — an `invalid_assignment` against a same-named top-level
//! variable of the wrong type (the census face: co19
//! `augmenting_variables_getters_setters_A02_t13` 22 rows +
//! `augmenting_constructors_A01_t29` 4 rows, all dartq-extra) or an
//! `undefined_identifier` when no namesake exists.
//!
//! Upstream Dart SDK source, pkg/analyzer/lib/src/summary2/
//! element_builder.dart `_linkFormalParameters` — the fragments' parameter
//! lists link into one element per position and each body's formal scope
//! is populated from the merged element.
//!
//! Oracle rows: dart 3.13.7 (`dart analyze --format=machine`, the
//! augmentations experiment on via analysis options — the pinned CLI flag
//! is a no-op in that SDK) over the fixtures below; every silent fixture
//! analyzed clean. Assertions compare `invalid_assignment` messages (the
//! assigned/variable type pair pins the row's direction without pinning
//! rig-shifted offsets).

use super::test_rig::report_ops_experiments_with;

/// The fixture's `invalid_assignment` messages, in source order.
fn invalid_assignment_messages(source: &str) -> Vec<String> {
    let mut rows: Vec<(u32, String)> =
        report_ops_experiments_with(source, &[], None, &[dartq_language::Feature::Augmentations])
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "invalid_assignment")
            .map(|d| (d.range.offset, d.message.to_string()))
            .collect();
    rows.sort_by_key(|(offset, _)| *offset);
    rows.into_iter().map(|(_, message)| message).collect()
}

/// The fixture's `undefined_identifier` messages, in source order.
fn undefined_rows(source: &str) -> Vec<String> {
    let mut rows: Vec<(u32, String)> =
        report_ops_experiments_with(source, &[], None, &[dartq_language::Feature::Augmentations])
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "undefined_identifier")
            .map(|d| (d.range.offset, d.message.to_string()))
            .collect();
    rows.sort_by_key(|(offset, _)| *offset);
    rows.into_iter().map(|(_, message)| message).collect()
}

/// The oracle's `String`-to-`int` write row (the merged scope never fired
/// and the read fell through to the top-level `String`).
const STRING_TO_INT: &str =
    "A value of type 'String' can't be assigned to a variable of type 'int'.";

/// The oracle's `int`-to-`String` write row (the merged slot bound and its
/// `int` type still gates the write against the `String` variable).
const INT_TO_STRING: &str =
    "A value of type 'int' can't be assigned to a variable of type 'String'.";

// —— the census faces: both chain directions over setters and constructors ——

/// Chain shape one (the `topLevelSetter1` family): the intro's body writes
/// the name only the LATER augment spells. The write is `int = int` once
/// `x` reads the merged slot — the pre-fix row (String top-level) is gone.
/// Oracle: zero rows over the whole file.
#[test]
fn intro_body_reads_the_augment_spelled_name() {
    let source = "\
String x = '';
int y = 0;
void set s1(int _) {
  y = x;
}
augment void set s1(int x);
main() {
  s1 = 1;
}
";
    assert_eq!(invalid_assignment_messages(source), Vec::<String>::new());
}

/// Chain shape two (the `topLevelSetter2` family): the augment's `_` body
/// writes the name only the intro spells. Oracle: zero rows.
#[test]
fn augment_body_reads_the_intro_spelled_name() {
    let source = "\
String x = '';
int y = 0;
void set s1(int x);
augment void set s1(int _) {
  y = x;
}
main() {
  s1 = 1;
}
";
    assert_eq!(invalid_assignment_messages(source), Vec::<String>::new());
}

/// The constructor faces (the `augmenting_constructors_A01_t29` rows): the
/// unnamed and `.named` chains in both directions. Oracle: zero
/// invalid_assignment rows (the file's own faces — the defaulted augment
/// curing the missing default — are outside this ticket's code).
#[test]
fn constructor_chains_merge_both_directions() {
    let source = "\
String x = '';
int y = 0;
class C {
  int? v;
  C(int x);
  C._([int _]) {
    v = x;
  }
}
augment class C {
  augment C(int _) {
    v = x;
  }
  augment C._([int x = 0]);
}
main() {
  print(C);
}
";
    assert_eq!(invalid_assignment_messages(source), Vec::<String>::new());
}

/// An augment body reading a name the intro spells, over a constructor
/// selector chain (the `augment ET.foo` shape of A01_t29).
#[test]
fn named_constructor_augment_body_reads_the_intro_name() {
    let source = "\
String x = '';
int y = 0;
extension type ET(int z) {
  ET.foo(int x);
}
augment extension type ET {
  augment ET.foo(int _) {
    y = x;
  }
}
main() {
  print(ET);
}
";
    assert_eq!(invalid_assignment_messages(source), Vec::<String>::new());
}

// —— the slot-naming rules ——

/// Multi-fragment ordering: the FIRST non-`_` spelling per position wins
/// across the whole chain, and the intro body sees names introduced two
/// augments later. Every position lands a spelling (intro `_`/augment one
/// `y1`/augment two `x1` and `z1`), so all three reads bind — the oracle
/// analyzes the file clean.
#[test]
fn first_non_wildcard_spelling_names_the_slot_across_fragments() {
    let source = "\
String x1 = 'x1';
String y1 = 'y1';
String z1 = 'z1';
int iy = 0;
void f(int _, int _, int _) {
  iy = x1;
  iy = y1;
  iy = z1;
}
augment void f(int _, int y1, int _);
augment void f(int x1, int _, int z1);
main() {
  f(1, 2, 3);
}
";
    assert_eq!(invalid_assignment_messages(source), Vec::<String>::new());
}

/// A position spelled `_` on every fragment binds nothing: the body's read
/// falls through to the top-level variable (oracle: invalid_assignment at
/// the read). The merged scope never invents a name.
#[test]
fn all_wildcard_position_contributes_no_name() {
    let source = "\
String x1 = 'x1';
int iy = 0;
void set s1(int _) {
  iy = x1;
}
augment void set s1(int _);
main() {
  s1 = 1;
}
";
    assert_eq!(invalid_assignment_messages(source), [STRING_TO_INT]);
}

/// The merged slot's type decides: a body write that the merged `int` slot
/// rejects still reports with the slot's own types — dropping the
/// wrong-scope rows never suppresses a genuine type conflict.
#[test]
fn merged_slot_type_still_gates_the_write() {
    let source = "\
String x1 = 'x1';
String s = '';
void set s1(int _) {
  s = x1;
}
augment void set s1(int x1);
main() {
  s1 = 1;
}
";
    assert_eq!(invalid_assignment_messages(source), [INT_TO_STRING]);
}

// —— the undefined family (the corpus-wide same-root face) ——

/// An augment body reading another fragment's positional name with NO
/// top-level namesake: the merged scope binds the read (the oracle stays
/// silent); pre-fix it reported `undefined_identifier`.
#[test]
fn augment_body_reference_without_namesake_resolves() {
    let source = "\
int iy = 0;
void set s1(int x1) {}
augment void set s1(int _) {
  iy = x1;
}
main() {
  s1 = 1;
}
";
    assert_eq!(undefined_rows(source), Vec::<String>::new());
}

/// An intro body reading a later augment's positional name with no
/// namesake: same silence (the merged element's parameter is in scope from
/// the intro's own body).
#[test]
fn intro_body_reference_without_namesake_resolves() {
    let source = "\
int iy = 0;
void set s1(int _) {}
augment void set s1(int x1);
main() {
  s1 = 1;
}
";
    assert_eq!(undefined_rows(source), Vec::<String>::new());
}

// —— controls: the non-chain postures stay put ——

/// A lone `_` formal (no augment anywhere) keeps the fall-through: the
/// wildcard binds nothing and the read resolves to the top-level variable
/// (oracle: invalid_assignment). The merged scope never fires without a
/// chain.
#[test]
fn lone_wildcard_formal_keeps_the_fall_through() {
    let source = "\
String x1 = 'x1';
int iy = 0;
void set s1(int _) {
  iy = x1;
}
main() {
  s1 = 1;
}
";
    assert_eq!(invalid_assignment_messages(source), [STRING_TO_INT]);
}

/// Member methods (the instance-setter faces of A02_t13) merge the same
/// way — the same-key instance space, both directions in one file.
#[test]
fn instance_member_chains_merge_both_directions() {
    let source = "\
String x = '';
int y = 0;
class C {
  void set s1(int _) {
    y = x;
  }
  void set s2(int x);
  augment void set s2(int _) {
    y = x;
  }
}
augment class C {
  augment void set s1(int x);
}
main() {
  print(C);
}
";
    assert_eq!(invalid_assignment_messages(source), Vec::<String>::new());
}

/// A static member chain keys apart from the instance one (the
/// static-setter faces of A02_t13): same spellings, static space.
#[test]
fn static_member_chains_merge() {
    let source = "\
String x = '';
int y = 0;
class C {
  static void set s1(int _) {
    y = x;
  }
  augment static void set s1(int x);
}
main() {
  print(C);
}
";
    assert_eq!(invalid_assignment_messages(source), Vec::<String>::new());
}

/// The experiment gate: without the augmentations opt-in the unit keeps
/// the per-fragment binding — the `_` slot binds nothing and the read
/// falls through to the top-level variable (the pre-fix row stands).
#[test]
fn without_the_experiment_the_binding_stays_per_fragment() {
    let source = "\
String x = '';
int y = 0;
void set s1(int _) {
  y = x;
}
augment void set s1(int x);
main() {
  s1 = 1;
}
";
    let rows: Vec<String> = report_ops_experiments_with(source, &[], None, &[])
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .map(|d| d.message.to_string())
        .collect();
    assert_eq!(rows, [STRING_TO_INT]);
}

// —— the parameterized-alias chain walk (resolution.rs
// `fold_parameterized_alias`; the queue item folded into this ticket) ——

/// A chain-walked parameterized alias use lands on the CONTAINER head with
/// its substituted arguments (`U<String>` folds through the alias `T` to
/// `C<List<String>>`), so a well-typed initialization stays clean.
/// Oracle-pinned: the fixture analyzes clean.
#[test]
fn chain_walked_alias_use_lands_on_the_container_head() {
    let source = "\
class C<T> {}
typedef T<X> = C<X>;
typedef U<Y> = T<List<Y>>;
main() {
  U<String> u = C<List<String>>();
}
";
    assert_eq!(invalid_assignment_messages(source), Vec::<String>::new());
}

/// The landed head's types gate the write: `U<String>` resolves to
/// `C<List<String>>`, so an int initializer reports (oracle-pinned: the
/// row fires; pre-fix the walk kept the alias spelling and the row went
/// missing).
#[test]
fn chain_walked_alias_use_types_the_assignment() {
    let source = "\
class C<T> {}
typedef T<X> = C<X>;
typedef U<Y> = T<List<Y>>;
main() {
  U<String> u = 42;
}
";
    let messages = invalid_assignment_messages(source);
    assert_eq!(messages.len(), 1, "got: {messages:?}");
    assert!(
        messages[0].starts_with("A value of type 'int' can't be assigned"),
        "got: {messages:?}"
    );
}
