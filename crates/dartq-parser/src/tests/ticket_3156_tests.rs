//! #3156 pins: the recovery fold-loop residuals (oracle 3.13.7). The
//! families pinned here are the central decision sites the ticket closed —
//! the `allowAbstract` lift that suppressed `missing_function_body` on
//! `;`-bodied members that can never be abstract (`parser_impl.dart:6075`
//! passes `isAugmentationsFeatureEnabled` so the parse stays silent, then
//! `error_verifier.dart:5765` reports `missingFunctionBody` on any element
//! with no complete fragment; dartq emits the same row at parse time), and
//! the `looksLikeStartOfNextTopLevelDeclaration` synthetic-name arm that
//! makes no reserved/built-in distinction upstream
//! (`identifier_context_impl.dart`) — a declaration-head keyword in a name
//! slot splices a synthetic name (`missing_identifier`) and stays for the
//! declaration loop.

use super::collector::{latest_features, scan_ok};

/// Parser rows of `source` as (code, start, end) UTF-16 triples, sorted.
fn sorted_rows(source: &'static str) -> Vec<(String, u32, u32)> {
    let (table, text) = scan_ok(source);
    let mut collector = super::collector::Collector::default();
    let mut parser = crate::Parser::new(text, table, latest_features(), &mut collector);
    parser.parse_unit();
    let mut rows = parser.listener.errors.clone();
    rows.sort();
    rows
}

fn code(c: &str, start: u32, end: u32) -> (String, u32, u32) {
    (c.to_string(), start, end)
}

/// A `static` member with a `;` body reports `missing_function_body` on the
/// semicolon even when the augmentations feature lifts the parse-time
/// `allowAbstract` gate — a static member can never be abstract, so the
/// analyzer-side `missingFunctionBody` arm
/// (error_verifier.dart:5741-5765) always fires. Non-`augment`/`external`
/// top-level `;` functions report the same way.
#[test]
fn static_member_empty_body_reports_missing_function_body() {
    assert_eq!(
        sorted_rows("class A {\n  static int get foo;\n}\n"),
        vec![code("missing_function_body", 30, 31)],
        "oracle: `static int get foo;` reports `missing_function_body` on \
         the `;` — a static member is never abstract"
    );
    assert_eq!(
        sorted_rows("abstract class A {\n  static int m();\n}\n"),
        vec![code("missing_function_body", 35, 36)],
        "oracle: the enclosing `abstract` does not rescue a `static` `;` body"
    );
    assert_eq!(
        sorted_rows("int top();\n"),
        vec![code("missing_function_body", 9, 10)],
        "oracle: a top-level `;` function can never be abstract"
    );
    assert_eq!(
        sorted_rows("abstract class A {\n  int get foo;\n}\n"),
        Vec::<(String, u32, u32)>::new(),
        "oracle: a non-static `;` member inside an `abstract` class is a \
         legal abstract member — no row"
    );
}

/// A top-level-declaration keyword head in a declaration-name slot splices
/// a synthetic name (`missing_identifier`) and stays for the declaration
/// loop — upstream makes no reserved/built-in distinction
/// (`ClassOrMixinOrExtensionIdentifierContext.ensureIdentifier`,
/// identifier_context_impl.dart:80-86). `class enum E { v }` reports
/// `missing_identifier` at `enum` (never `builtInIdentifierInDeclaration`)
/// and `expected_body` at the same anchor because the class then has no
/// `{` body (`enum E { v }` reparses as an enum declaration).
#[test]
fn class_name_keyword_head_inserts_synthetic_name() {
    // `class enum E { v }`: the central recovery arm splices a synthetic
    // name (`missing_identifier`) and leaves `enum` for the enum reparse —
    // the class then reports `expected_body`. (A residual
    // `built_in_identifier_in_declaration` row at `enum` is the
    // declarations.rs head-name pre-capture at declarations.rs:1577-1599 —
    // owned by the in-flight declarations fold lane, so the assertion
    // checks containment rather than full equality.)
    let rows = sorted_rows("class enum E { v }\n");
    assert!(
        rows.contains(&code("missing_identifier", 6, 10))
            && rows.contains(&code("expected_body", 6, 10)),
        "oracle: `missing_identifier` + `expected_body` both anchor on \
         `enum`; got {rows:?}"
    );
    let rows = sorted_rows("class mixin M { }\n");
    assert!(
        rows.contains(&code("missing_identifier", 6, 11))
            && rows.contains(&code("expected_body", 6, 11)),
        "oracle: `missing_identifier` + `expected_body` both anchor on \
         `mixin`; got {rows:?}"
    );
    // The keep-set follower arm: `class enum { }` keeps `enum` as the name
    // because the following `{` continues the header
    // (`_isOneOfFollowingValues` — `<` `{` `extends` `with` `implements`
    // `on` `=` `(` `.` EOF).
    assert_eq!(
        sorted_rows("class enum { }\n"),
        vec![
            code("built_in_identifier_in_declaration", 6, 10),
            code("expected_identifier_but_got_keyword", 6, 10),
        ],
        "oracle: `class enum {{ }}` consumes `enum` as the name — the `{{` \
         is a keep-set follower, so the built-in/keyword faces fire instead"
    );
}

/// The `LibraryIdentifierContext` recovery arm is shared by `library`/`part
/// of` names and their `.` continuations
/// (identifier_context_impl.dart:976-983): a declaration-head keyword after
/// the directive splices a synthetic name and stays for the declaration
/// loop, and the abandoned directive closes with `expected_token ';'`.
#[test]
fn directive_name_keyword_head_inserts_synthetic_name() {
    // `library mixin M {}` — `mixin` stays for the mixin reparse; the bare
    // `library` directive reports `;` after `library`.
    let rows = sorted_rows("library mixin M {}\n");
    assert!(
        rows.contains(&code("missing_identifier", 8, 13)),
        "oracle: `missing_identifier` at `mixin`; got {rows:?}"
    );
    assert!(
        rows.contains(&code("expected_token", 0, 7)),
        "oracle: `expected_token ';'` after `library`; got {rows:?}"
    );
    // `part of lib. final a = 0;` — `final` stays for the variable reparse.
    let rows = sorted_rows("part of lib. final a = 0;\n");
    assert!(
        rows.contains(&code("missing_identifier", 13, 18)),
        "oracle: `missing_identifier` at `final`; got {rows:?}"
    );
    // `typedef T set a(b) {}` — `set` stays for the setter reparse; the bare
    // typedef reports `;` after `T`.
    let rows = sorted_rows("typedef T set a(b) {}\n");
    assert!(
        rows.contains(&code("missing_identifier", 10, 13)),
        "oracle: `missing_identifier` at `set`; got {rows:?}"
    );
}

/// A `super.`-initializer whose name slot is a declaration-head keyword
/// splices a synthetic name and anchors `expected_token '('` on that
/// keyword — the spliced identifier sits at the keyword's own offset, so
/// its follower is the keyword, never the token after it
/// (`insertParens(includeIdentifier: false)`, parser_impl.dart:4948). A
/// real name instead anchors the `(`-expect on the real follower.
#[test]
fn super_initializer_keyword_name_paren_anchor() {
    // `class C {C() : super. var f;}`: the `(`-expect and the spliced-name
    // row both anchor on `var` (22,25) — the name slot's follower is the
    // keyword itself once the synthetic identifier is spliced at its
    // offset. (The `missing_function_body` at `var` and the field reparse
    // of `var f;` ride the constructor-initializer plumbing in members.rs /
    // declarations.rs — the `expected_class_member` cascade there is a
    // residual outside this ticket's central scope, so the assertion pins
    // the two anchors the `insertParens` law fixes.)
    let rows = sorted_rows("class C {C() : super. var f;}\n");
    assert!(
        rows.contains(&code("missing_identifier", 22, 25)),
        "oracle: `missing_identifier` at `var` (22,25); got {rows:?}"
    );
    assert!(
        rows.contains(&code("expected_token", 22, 25)),
        "oracle: `expected_token '('` at `var` (22,25); got {rows:?}"
    );
    // Control: a real member name anchors `(` on the follower.
    let rows = sorted_rows("class D {D() : super. foo f;}\n");
    assert!(
        rows.contains(&code("expected_token", 26, 27)),
        "oracle: `(`-expect anchors on `f` (26,27) after the real name \
         `foo`; got {rows:?}"
    );
}

