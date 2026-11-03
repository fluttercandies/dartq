use crate::testsupport::{Fixture, one_rule};

/// The #2846 pins: a bare-identifier collection element's `Send` leaf
/// tiles through its unconsumed follower token — the element comma when
/// the next element follows directly (`parse_send_from_identifier`'s
/// documented span contract). The shared `slice::comma_segments`
/// boundary kept `comma.end <= piece.end`, so that follower comma read
/// as covered interior and `[j, 1]` folded into one segment rooted at
/// `1` — every per-element face over the shared segmentation
/// under-reported. The fix keeps strict-interior coverage
/// (`comma.end < piece.end`) plus the sits-on separator guard — the
/// same boundary `declarations.rs`'s `collection_elements` walks —
/// because upstream terminates an element's span at its own last token
/// (`upstream Dart SDK source, pkg/analyzer/lib/src/dart/ast/ast.dart`
/// :3117, `AstNodeImpl.end => endToken.end`).
///
/// `await_only_futures` is the stable consumer (its collection-display
/// face reads one display per segment, `await_only_futures.dart`'s
/// inferred-type argument): a folded segment reports only its last
/// row's face, so the display is the segmentation itself.

/// `[j, 1]` is two elements: `String j` and `int` mix to `List<dynamic>`
/// — the comma the `j` send swallows still separates. The folded read
/// answered `List<int>` (the merged segment's `1` root).
#[test]
fn bare_identifier_element_comma_still_separates() {
    let f = Fixture::parse("void f() async {\n  String j = '';\n  await [j, 1];\n}\n");
    let lints = f.run_semantic(&one_rule("await_only_futures"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Uses 'await' on an instance of 'List<dynamic>', which is not a subtype of 'Future'."
    );
}

/// The tail-side polarity: `[1, j]` must not collapse to the `j` send
/// group's `String` face — the two elements still mix to `dynamic`.
#[test]
fn bare_identifier_tail_element_keeps_both_sides() {
    let f = Fixture::parse("void f() async {\n  String j = '';\n  await [1, j];\n}\n");
    let lints = f.run_semantic(&one_rule("await_only_futures"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Uses 'await' on an instance of 'List<dynamic>', which is not a subtype of 'Future'."
    );
}

/// The trailing-comma face (`[j, 1, ]`): the last comma separates
/// nothing but must not disturb the two real segments.
#[test]
fn trailing_comma_keeps_both_elements() {
    let f = Fixture::parse("void f() async {\n  String j = '';\n  await [j, 1, ];\n}\n");
    let lints = f.run_semantic(&one_rule("await_only_futures"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Uses 'await' on an instance of 'List<dynamic>', which is not a subtype of 'Future'."
    );
}

/// A same-type pair keeps the shared face: `[j, 2]` over `int j`
/// derives `List<int>` — the strict-interior change must not flip the
/// agreed spelling.
#[test]
fn same_type_elements_still_spell() {
    let f = Fixture::parse("void f() async {\n  int j = 0;\n  await [j, 2];\n}\n");
    let lints = f.run_semantic(&one_rule("await_only_futures"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Uses 'await' on an instance of 'List<int>', which is not a subtype of 'Future'."
    );
}

/// The control-flow and spread shapes' unchanged faces: each segment's
/// root is the `Spread`/`ForControlFlow`/`IfControlFlow` marker, which
/// `await_only_futures`'s display face does not read (`side_display`'s
/// `_` arm answers `None` — the documented best-effort display
/// under-read), so only the `1` side contributes `int`. The comma
/// boundary itself is exercised — the element markers seal exactly on
/// the follower comma (the same `end_incl` contract the `Send` leaf
/// rides), so these keep the boundary honest as regression pins.

/// `[...z, 1]`: the spread operand's own send swallows the same
/// follower comma — the element still separates; the display reads the
/// unchanged `int` face.
#[test]
fn spread_operand_comma_still_separates() {
    let f = Fixture::parse("void f() async {\n  List<String> z = [];\n  await [...z, 1];\n}\n");
    let lints = f.run_semantic(&one_rule("await_only_futures"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Uses 'await' on an instance of 'List<int>', which is not a subtype of 'Future'."
    );
}

/// `[for (var i in y) i, 1]`: the `ForControlFlow` marker seals on the
/// element comma — the element still separates; the display reads the
/// unchanged `int` face.
#[test]
fn for_element_comma_still_separates() {
    let f = Fixture::parse(
        "void f() async {\n  List<String> y = [];\n  await [for (var i in y) i, 1];\n}\n",
    );
    let lints = f.run_semantic(&one_rule("await_only_futures"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Uses 'await' on an instance of 'List<int>', which is not a subtype of 'Future'."
    );
}

/// `[if (c) j, 1]`: the `IfControlFlow` marker seals on the element
/// comma — the element still separates; the display reads the
/// unchanged `int` face.
#[test]
fn if_element_comma_still_separates() {
    let f = Fixture::parse(
        "void f() async {\n  String j = '';\n  bool c = true;\n  await [if (c) j, 1];\n}\n",
    );
    let lints = f.run_semantic(&one_rule("await_only_futures"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Uses 'await' on an instance of 'List<int>', which is not a subtype of 'Future'."
    );
}
