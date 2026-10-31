//! #3190 pins: the two context-specific `covariant`-modifier reports that
//! replace the generic `extraneous_modifier` face.
//!
//! Upstream `parseFormalParameterModifiers` (modifier_context.dart:223-247)
//! consumes `covariant` as an ordinary modifier in the walk and picks the
//! report by member kind at walk end:
//! - `ExtensionTypeNonStaticMethod` / `ExtensionTypeStaticMethod` →
//!   `reportExtraneousModifierInExtensionType` (:230-234), analyzer code
//!   `extraneous_modifier_in_extension_type`;
//! - `PrimaryConstructor` with the primary-constructors feature OFF →
//!   `reportExtraneousModifierInPrimaryConstructor` (:236-248), analyzer code
//!   `extraneous_modifier_in_primary_constructor`; with the feature ON only a
//!   `var` formal keeps `covariant`
//!   (`invalid_covariant_modifier_in_primary_constructor`);
//! - the plain method flavors keep the generic `extraneous_modifier`; a
//!   `covariant` in a plain `extension` body takes
//!   `invalid_use_of_covariant_in_extension` in the walk.

use super::collector::{latest_features, scan_ok, version_features};

struct Rows {
    rows: Vec<(String, u32, u32)>,
    texts: Vec<String>,
}

fn parse_with(source: &'static str, features: crate::ParserFeatures) -> Rows {
    let (table, text) = scan_ok(source);
    let mut collector = super::collector::Collector::default();
    let mut parser = crate::Parser::new(text, table, features, &mut collector);
    parser.parse_unit();
    let mut rows = parser.listener.errors.clone();
    rows.sort();
    Rows {
        rows,
        texts: parser.listener.texts.clone(),
    }
}

fn parse(source: &'static str) -> Rows {
    parse_with(source, latest_features())
}

fn row(code: &str, start: u32, end: u32) -> (String, u32, u32) {
    (code.to_string(), start, end)
}

fn text_of<'a>(parsed: &'a Rows, code: &str) -> Option<&'a str> {
    parsed
        .rows
        .iter()
        .position(|(c, _, _)| c == code)
        .map(|index| parsed.texts[index].as_str())
}

const EXT_TYPE_TEXT: &str = "Can't have modifier 'covariant' in an extension type.";
const PRIMARY_CTOR_TEXT: &str = "Can't have modifier 'covariant' in a primary constructor.";

/// `extension type` instance method formal: the dedicated extension-type
/// code over the `covariant` token (oracle
/// `test_error_formalParameterModifier_covariant_method_instance`, 3:12|9).
#[test]
fn ticket_3190_extension_type_instance_method() {
    let parsed = parse("extension type E(int it) { void m(covariant int x) {} }\n");
    assert_eq!(
        parsed.rows,
        vec![row("extraneous_modifier_in_extension_type", 34, 43)],
        "instance method formal: dedicated extension-type row only"
    );
    assert_eq!(
        text_of(&parsed, "extraneous_modifier_in_extension_type"),
        Some(EXT_TYPE_TEXT)
    );
}

/// Static method inside an extension type: same code, same anchor rule.
#[test]
fn ticket_3190_extension_type_static_method() {
    let parsed = parse("extension type E(int it) { static void m(covariant int x) {} }\n");
    assert_eq!(
        parsed.rows,
        vec![row("extraneous_modifier_in_extension_type", 41, 50)],
        "static method formal: dedicated extension-type row only"
    );
    assert_eq!(
        text_of(&parsed, "extraneous_modifier_in_extension_type"),
        Some(EXT_TYPE_TEXT)
    );
}

/// With primary constructors off (language 3.10), a `covariant` formal takes
/// `extraneous_modifier_in_primary_constructor` (oracle
/// `test_primaryConstructor_formalParameters_keyword_covariant_language310`,
/// 3:18|9), never the enabled-feature sibling.
#[test]
fn ticket_3190_primary_constructor_feature_off() {
    let parsed = parse_with(
        "extension type A(covariant int it) {}\n",
        version_features(3, 10),
    );
    assert!(
        parsed
            .rows
            .contains(&row("extraneous_modifier_in_primary_constructor", 17, 26)),
        "feature-off primary-constructor covariant reports: {:?}",
        parsed.rows
    );
    assert_eq!(
        text_of(&parsed, "extraneous_modifier_in_primary_constructor"),
        Some(PRIMARY_CTOR_TEXT)
    );
    assert!(
        parsed
            .rows
            .iter()
            .all(|(code, _, _)| code != "invalid_covariant_modifier_in_primary_constructor"),
        "the enabled-feature sibling must not fire: {:?}",
        parsed.rows
    );
}

/// A `covariant var` primary-constructor formal with the feature on is legal:
/// no extraneous row from either context code.
#[test]
fn ticket_3190_primary_constructor_var_is_legal() {
    let parsed = parse("extension type A(covariant var int it) {}\n");
    assert!(
        parsed.rows.iter().all(|(code, _, _)| {
            code != "extraneous_modifier_in_primary_constructor"
                && code != "invalid_covariant_modifier_in_primary_constructor"
        }),
        "covariant var stays silent: {:?}",
        parsed.rows
    );
}

/// A plain `extension` body member keeps `invalid_use_of_covariant_in_extension`
/// — the extension-type code never fires there (member-kind context split).
#[test]
fn ticket_3190_plain_extension_keeps_its_own_code() {
    let parsed = parse("extension X on int { void m(covariant int x) {} }\n");
    assert!(
        parsed
            .rows
            .contains(&row("invalid_use_of_covariant_in_extension", 28, 37)),
        "plain extension keeps its own code: {:?}",
        parsed.rows
    );
    assert!(
        parsed
            .rows
            .iter()
            .all(|(code, _, _)| code != "extraneous_modifier_in_extension_type"),
        "extension-type code must not leak into plain extensions: {:?}",
        parsed.rows
    );
}

/// No `covariant`, no row: the context codes are modifier-driven only.
#[test]
fn ticket_3190_no_covariant_stays_silent() {
    let parsed = parse("extension type E(int it) { void m(int x) {} }\n");
    assert!(parsed.rows.is_empty(), "{:?}", parsed.rows);
}
