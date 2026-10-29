//! Built-in-identifier declaration-name faces (#2787): the six
//! per-context templates the analyzer registry carries under
//! `sharedName: builtInIdentifierInDeclaration` (upstream Dart SDK source,
//! pkg/analyzer/messages.yaml:2421-2492), the verifier gate that picks them
//! (`_checkForBuiltInIdentifierAsName`, error_verifier.dart:3760-3781,
//! including the variance `inout`/`out` branch), and the recovery-anchor
//! faces re-clustered in wave-3 (extension missing-`on` expectedAfterButGot
//! anchor, combinator and local-variable reserved-name inserts).

use dartq_scanner::ScanConfig;

use super::collector::{Collector, latest_features};
use crate::parser::Parser;
use crate::ParserFeatures;

/// Parses `source` with `features` and returns the scanner+parser rows as
/// `(code, start, end)` sorted by offset plus the problem texts in emission
/// order (gate_ppin harness shape).
fn rows_and_texts_with(
    features: ParserFeatures,
    source: &'static str,
) -> (Vec<(String, u32, u32)>, Vec<String>) {
    let config = ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let mut files = dartq_source::FileTable::new();
    let file = files.intern(std::path::Path::new("test.dart"));
    let result = dartq_scanner::scan(source, config, file);
    let mut rows: Vec<(String, u32, u32)> = result
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.name.to_owned(),
                d.range.offset,
                d.range.offset + d.range.length,
            )
        })
        .collect();
    let mut parser = Parser::new(
        source,
        Box::leak(Box::new(result.tokens)),
        features,
        Box::leak(Box::new(Collector::default())),
    );
    parser.parse_unit();
    rows.extend(parser.listener.errors.iter().cloned());
    rows.sort_by_key(|(_, start, end)| (*start, *end));
    (rows, parser.listener.texts.clone())
}

fn rows_and_texts(source: &'static str) -> (Vec<(String, u32, u32)>, Vec<String>) {
    rows_and_texts_with(latest_features(), source)
}

/// The built-in face for a declaration name: one `built_in_identifier_in_declaration`
/// row on the name token with the per-context template text.
fn assert_builtin_text(texts: &[String], expected: &str, label: &str) {
    assert!(
        texts.iter().any(|t| t == expected),
        "{label}: expected text {expected:?} in {texts:?}"
    );
}

fn leaked(source: &str) -> &'static str {
    Box::leak(source.to_owned().into_boxed_str())
}

// —— the six templates ————————————————————————————————————————————————

/// Class names take the type-name face (visitClassDeclaration,
/// error_verifier.dart:573; oracle BuiltInIdentifierAsTypeNameTest).
#[test]
fn bi_name_class_type_template() {
    let source = leaked("class Function {}");
    let (rows, texts) = rows_and_texts(source);
    assert_builtin_text(
        &texts,
        "The built-in identifier 'Function' can't be used as a type name.",
        "class name",
    );
    assert!(
        rows.contains(&("built_in_identifier_in_declaration".to_owned(), 6, 14)),
        "built_in row on the class name token: {rows:?}"
    );
}

/// A named mixin application's name takes the typedef-name face
/// (visitClassTypeAlias, error_verifier.dart:635; oracle
/// issue_46346_prime_1 `class abstract = A with B;`). The `=` follower
/// decides, with type parameters skipped (`class as<T> = A with B;`).
#[test]
fn bi_name_class_type_alias_typedef_template() {
    for source in ["class as = A with B;", "class as<T> = A with B;"] {
        let (rows, texts) = rows_and_texts(leaked(source));
        assert_builtin_text(
            &texts,
            "The built-in identifier 'as' can't be used as a typedef name.",
            source,
        );
        assert!(
            rows.contains(&("built_in_identifier_in_declaration".to_owned(), 6, 8)),
            "{source}: built_in row on the name token: {rows:?}"
        );
    }
}

/// Mixin names take the type-name face (visitMixinDeclaration,
/// error_verifier.dart:1840).
#[test]
fn bi_name_mixin_type_template() {
    let (rows, texts) = rows_and_texts(leaked("mixin as {}"));
    assert_builtin_text(
        &texts,
        "The built-in identifier 'as' can't be used as a type name.",
        "mixin name",
    );
    assert!(
        rows.contains(&("built_in_identifier_in_declaration".to_owned(), 6, 8)),
        "built_in row on the mixin name token: {rows:?}"
    );
}

/// Enum names take the type-name face (visitEnumDeclaration,
/// error_verifier.dart:1019).
#[test]
fn bi_name_enum_type_template() {
    let (rows, texts) = rows_and_texts(leaked("enum as { x }"));
    assert_builtin_text(
        &texts,
        "The built-in identifier 'as' can't be used as a type name.",
        "enum name",
    );
    assert!(
        rows.contains(&("built_in_identifier_in_declaration".to_owned(), 5, 7)),
        "built_in row on the enum name token: {rows:?}"
    );
}

/// Extension names take the extension-name face (visitExtensionDeclaration,
/// error_verifier.dart:1124).
#[test]
fn bi_name_extension_template() {
    let (rows, texts) = rows_and_texts(leaked("extension as on int {}"));
    assert_builtin_text(
        &texts,
        "The built-in identifier 'as' can't be used as an extension name.",
        "extension name",
    );
    assert!(
        rows.contains(&("built_in_identifier_in_declaration".to_owned(), 10, 12)),
        "built_in row on the extension name token: {rows:?}"
    );
}

/// Extension-type names take the extension-type-name face
/// (visitExtensionTypeDeclaration, error_verifier.dart:1150).
#[test]
fn bi_name_extension_type_template() {
    let (rows, texts) = rows_and_texts(leaked("extension type as(int x) {}"));
    assert_builtin_text(
        &texts,
        "The built-in identifier 'as' can't be used as an extension type name.",
        "extension type name",
    );
    assert!(
        rows.contains(&("built_in_identifier_in_declaration".to_owned(), 15, 17)),
        "built_in row on the extension type name token: {rows:?}"
    );
}

/// Import prefixes take the prefix-name face (visitImportDirective,
/// error_verifier.dart:1562).
#[test]
fn bi_name_prefix_template() {
    let (rows, texts) = rows_and_texts(leaked("import 'a.dart' as as;"));
    assert_builtin_text(
        &texts,
        "The built-in identifier 'as' can't be used as a prefix name.",
        "import prefix",
    );
    assert!(
        rows.contains(&("built_in_identifier_in_declaration".to_owned(), 19, 21)),
        "built_in row on the prefix token: {rows:?}"
    );
}

/// Type parameters take the type-parameter-name face (visitTypeParameter,
/// error_verifier.dart:2478; oracle issue_45703 line 2).
#[test]
fn bi_name_type_parameter_template() {
    let (rows, texts) = rows_and_texts(leaked("class C<as> {}"));
    assert_builtin_text(
        &texts,
        "The built-in identifier 'as' can't be used as a type parameter name.",
        "type parameter",
    );
    assert!(
        rows.contains(&("built_in_identifier_in_declaration".to_owned(), 8, 10)),
        "built_in row on the type parameter token: {rows:?}"
    );
    // The tolerant keyword face must not fire in addition (issue_45703
    // carries no expected_identifier_but_got_keyword at type parameters).
    assert!(
        !rows.iter().any(|(c, _, _)| c == "expected_identifier_but_got_keyword"),
        "type parameter carries no keyword face: {rows:?}"
    );
}

/// Typedef names (both styles) keep the typedef-name face
/// (visitFunctionTypeAlias :1480 / visitGenericTypeAlias :1493; oracle
/// keyword_named_typedefs.dart).
#[test]
fn bi_name_typedef_template() {
    for source in ["typedef as = int;", "typedef void as();"] {
        let (_, texts) = rows_and_texts(leaked(source));
        assert_builtin_text(
            &texts,
            "The built-in identifier 'as' can't be used as a typedef name.",
            source,
        );
    }
}

// —— the variance branch ———————————————————————————————————————————————

/// Under the variance experiment the pseudo pair `inout`/`out` takes the
/// built-in face (error_verifier.dart:3774-3780); without it the pair stays
/// silent (the `isPseudo != true` gate).
#[test]
fn bi_name_variance_pseudo_pair() {
    let mut features = latest_features();
    features.variance_enabled = true;
    for (source, expected) in [
        ("class inout {}", "type name"),
        ("class C<out> {}", "type parameter name"),
    ] {
        let (_, texts) = rows_and_texts_with(features, leaked(source));
        assert_builtin_text(
            &texts,
            &format!("The built-in identifier '{0}' can't be used as a {1}.", if source.contains('<') { "out" } else { "inout" }, expected),
            source,
        );
    }
    // Without the experiment the same shapes stay row-free.
    for source in ["class inout {}", "class C<out> {}"] {
        let (rows, _) = rows_and_texts(leaked(source));
        assert!(
            !rows
                .iter()
                .any(|(c, _, _)| c == "built_in_identifier_in_declaration"),
            "{source}: no built_in row without variance: {rows:?}"
        );
    }
}

// —— wave-3 C5 anchors ———————————————————————————————————————————————— _

/// Extension missing-`on`: the expectedAfterButGot row anchors on the last
/// consumed header token — the extension name — not the junk body opener
/// (parser_impl.dart:3720-3728; oracle test_missing_on_withBlock anchors at
/// 2:11 on `E`, with the type-name row on the `{`).
#[test]
fn extension_missing_on_anchor() {
    let source = leaked("extension E {}");
    let (rows, texts) = rows_and_texts(source);
    assert_builtin_text(&texts, "Expected to find 'on'.", "missing on");
    assert!(
        rows.contains(&("expected_token".to_owned(), 10, 11)),
        "'on' row anchored on the extension name: {rows:?}"
    );
    assert!(
        rows.contains(&("expected_type_name".to_owned(), 12, 13)),
        "type-name row on the junk opener: {rows:?}"
    );
}

/// `show`/`hide` combinator names reject reserved words with the
/// insert-synthetic face (CombinatorIdentifierContext,
/// identifier_context_impl.dart:133-165): the keyword stays current, the
/// `;` walk-back anchors on `show`.
#[test]
fn combinator_reserved_name_insert_face() {
    let source = leaked("export 'a.dart' hide A show class A {}");
    let (rows, texts) = rows_and_texts(source);
    assert_builtin_text(&texts, "Expected an identifier.", "combinator name");
    assert!(
        rows.contains(&("missing_identifier".to_owned(), 28, 33)),
        "missing_identifier on the reserved name: {rows:?}"
    );
    assert!(
        rows.contains(&("expected_token".to_owned(), 23, 27)),
        "';' row anchored on `show`: {rows:?}"
    );
    assert!(
        !rows
            .iter()
            .any(|(c, _, _)| c == "expected_identifier_but_got_keyword"),
        "no tolerant keyword face at the combinator name: {rows:?}"
    );
}

/// Local-variable names reject statement-starting reserved words with the
/// insert-synthetic face (LocalVariableDeclarationIdentifierContext,
/// identifier_context_impl.dart:1011-1050); the for-initializer `;`
/// walk-back then anchors on `var` (upstream `ensureSemicolon`).
#[test]
fn local_variable_reserved_statement_start_insert_face() {
    let source = leaked("f() { var while (x) y; }");
    let (rows, texts) = rows_and_texts(source);
    assert_builtin_text(&texts, "Expected an identifier.", "local variable name");
    assert!(
        rows.contains(&("missing_identifier".to_owned(), 10, 15)),
        "missing_identifier on the reserved name: {rows:?}"
    );
    assert!(
        !rows
            .iter()
            .any(|(c, _, _)| c == "expected_identifier_but_got_keyword"),
        "no tolerant keyword face at the local variable name: {rows:?}"
    );
    // The declaration's `;` expectation anchors on `var`, the last
    // consumed token before the reserved word.
    assert!(
        rows.contains(&("expected_token".to_owned(), 6, 9)),
        "';' row anchored on `var`: {rows:?}"
    );
}
