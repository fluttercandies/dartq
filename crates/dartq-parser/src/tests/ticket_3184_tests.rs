//! #3184 pins (oracle 3.13.7): a built-in top-level-declaration keyword is a
//! legal enum VALUE name — `EnumValueDeclarationIdentifierContext.
//! ensureIdentifier` early-returns `identifier.isIdentifier`
//! (identifier_context_impl.dart:349-351) BEFORE the recovery arm, and
//! `KeywordToken.isIdentifier` is `isPseudo || isBuiltIn` (token.dart:794),
//! so `enum E { mixin }` reports ZERO rows. The previous port admitted
//! `EnumValueDeclaration` into the built-in `looksLikeStartOfNextTopLevel`
//! synthetic-splice arm (the `classOrMixinOrExtension` template's posture —
//! that context early-returns only `isPseudo`), which spliced a nameless
//! identifier at the keyword, left the keyword current, and spun the
//! enum-elements loop forever.
//!
//! Reserved top-level heads still take the splice arm — upstream reaches it
//! because a reserved word is never `isIdentifier`.

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

/// Every built-in top-level-declaration head is a legal enum value name:
/// `mixin` (the #3184 hang), `typedef`, `extension`, `export`, `import`,
/// `library`, `part`, `get`, `set` all early-return at the `isIdentifier`
/// gate upstream. The SDK's own `enum _DeprecationKind { ... mixin ... }`
/// rides this arm — the parse must terminate AND stay silent.
#[test]
fn enum_value_builtin_top_level_heads_are_plain_names() {
    for member in [
        "mixin",
        "typedef",
        "extension",
        "export",
        "import",
        "library",
        "part",
        "get",
        "set",
    ] {
        let source = format!("enum E {{ {member} }}\n").leak();
        assert_eq!(
            sorted_rows(source),
            Vec::<(String, u32, u32)>::new(),
            "oracle: `enum E {{ {member} }}` is clean — `{member}` is a \
             built-in keyword, legal as an enum value name"
        );
    }
    // The real-world shape from the SDK's `meta/annotations.dart`.
    assert_eq!(
        sorted_rows(
            "enum _DeprecationKind { use, implement, extend, subclass, instantiate, mixin, optional }\n"
        ),
        Vec::<(String, u32, u32)>::new(),
        "oracle: the SDK `_DeprecationKind` member list is clean"
    );
}

/// A built-in value name followed by constructor-call argument syntax keeps
/// the same clean face — the element tail (type arguments / `.named` /
/// argument list) parses normally.
#[test]
fn enum_value_builtin_names_with_element_tails() {
    assert_eq!(
        sorted_rows("enum E { mixin(1), typedef.named() }\n"),
        Vec::<(String, u32, u32)>::new(),
        "oracle: `mixin(1)` / `typedef.named()` value forms are clean"
    );
}

/// Reserved words are never `isIdentifier`, so the recovery arm's
/// `looksLikeStartOfNextTopLevelDeclaration` disjunct DOES fire for them:
/// `enum E { void f() {} }` splices a synthetic name at `void` (upstream
/// `missing_identifier`), `void f() {}` reparses as the member head — the
/// splice arm itself is still required, just not for built-ins.
#[test]
fn enum_value_reserved_top_level_head_still_splices() {
    let rows = sorted_rows("enum E { void f() {} }\n");
    assert!(
        rows.contains(&code("missing_identifier", 9, 13)),
        "oracle: `void` in the value-name slot reports `missing_identifier` \
         and reparses as a member head; got {rows:?}"
    );
}
