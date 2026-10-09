//! Stable diagnostic codes: the official analyzer code set dartiq aligns to,
//! plus dartiq's own `dartiq/` namespace (PRD §13.2).
//!
//! The official set — 262 parser/scanner syntax codes plus the 3-code
//! `// @dart=` language-version family — is generated at build time from
//! `data/syntax-codes.tsv`, the authoritative table captured in
//! `docs/facts/syntax-codes.md` §9 (dart 3.13.1 oracle lock). To update it,
//! edit the TSV and rebuild; the per-code constants, the [`OFFICIAL_CODES`]
//! slice, and [`OFFICIAL_CODE_DATA`] are regenerated. Only codes outside that
//! table (the M0-measured compile-time/warning/lint set and the dartiq-own
//! namespace) are hand-written below.

use std::collections::BTreeMap;

use crate::model::DiagnosticType;

/// A stable diagnostic code: official `snake_case` name, its `UPPERCASE` machine
/// form, the diagnostic family it belongs to, and whether the official
/// documentation page is published.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CodeInfo {
    /// Official code name, `snake_case`, one-to-one with the analyzer's
    /// `analyzerCode` (e.g. `expected_token`); dartiq-own codes carry the
    /// `dartiq/` prefix (e.g. `dartiq/internal_error`).
    pub name: &'static str,
    /// `UPPERCASE` form as emitted in the machine format's `CODE` field
    /// (e.g. `EXPECTED_TOKEN`).
    pub machine: &'static str,
    /// The diagnostic family this code belongs to.
    pub dtype: DiagnosticType,
    /// Whether the analyzer registry marks this code `hasPublishedDocs: true`.
    /// The oracle's JSON `documentation` field (`errors.dart` `url` getter)
    /// exists only for published codes; dartiq-own codes are always `false`.
    pub has_published_docs: bool,
}

impl CodeInfo {
    /// Creates a code descriptor.
    #[must_use]
    pub const fn new(
        name: &'static str,
        machine: &'static str,
        dtype: DiagnosticType,
        has_published_docs: bool,
    ) -> Self {
        Self {
            name,
            machine,
            dtype,
            has_published_docs,
        }
    }
}

// Per-code doc comments embed verbatim oracle message text — data, not
// prose we may reformat, so doc-text lints are exempt.
#[allow(clippy::doc_markdown, clippy::doc_link_with_quotes)]
mod generated {
    include!(concat!(env!("OUT_DIR"), "/syntax_codes_generated.rs"));
}

pub use generated::*;

/// Message template of an official code, by exact name (TSV column 5; `{0}`/
/// `{1}` are substitution placeholders). Official codes only — dartiq-own
/// codes carry their messages at the emit site.
#[must_use]
pub fn message_template(name: &str) -> Option<&'static str> {
    OFFICIAL_CODE_DATA
        .iter()
        .find(|(n, ..)| *n == name)
        .map(|(_, _, message, _)| *message)
}

// A code's [`DiagnosticType`] is the machine format's TYPE value. Generated
// rows are SYNTACTIC_ERROR (parser/scanner class types) except the
// language-version family, which is WarningCode-class STATIC_WARNING. Codes
// the oracle routes by hand to COMPILE_TIME_ERROR at emit time (facts §2
// pseudoSharedCode routing) are emit-site decisions, not registry facts.

// --- M0-measured codes outside the generated syntax table ---
// hasPublishedDocs values captured from the generated registries
// (analyzer-8.4.1 error/codes.g.dart); lints are never published
// (`LintCode.url` is overridden to `null`), and dartiq-own codes never are.

// --- Compile-time errors ---

/// `The name 'x' is already defined.`
pub const DUPLICATE_DEFINITION: CodeInfo = CodeInfo::new(
    "duplicate_definition",
    "DUPLICATE_DEFINITION",
    DiagnosticType::CompileTimeError,
    true,
);
/// `The await expression can only be used in an async function.`
pub const AWAIT_IN_WRONG_CONTEXT: CodeInfo = CodeInfo::new(
    "await_in_wrong_context",
    "AWAIT_IN_WRONG_CONTEXT",
    DiagnosticType::CompileTimeError,
    false,
);
/// `Target of URI doesn't exist: '…'.`
pub const URI_DOES_NOT_EXIST: CodeInfo = CodeInfo::new(
    "uri_does_not_exist",
    "URI_DOES_NOT_EXIST",
    DiagnosticType::CompileTimeError,
    true,
);
/// `Undefined name 'x'.`
pub const UNDEFINED_IDENTIFIER: CodeInfo = CodeInfo::new(
    "undefined_identifier",
    "UNDEFINED_IDENTIFIER",
    DiagnosticType::CompileTimeError,
    true,
);
/// `The argument type 'T1' can't be assigned to the parameter type 'T2'.`
pub const ARGUMENT_TYPE_NOT_ASSIGNABLE: CodeInfo = CodeInfo::new(
    "argument_type_not_assignable",
    "ARGUMENT_TYPE_NOT_ASSIGNABLE",
    DiagnosticType::CompileTimeError,
    true,
);

// --- Static warnings ---

/// `The value of the local variable 'x' isn't used.`
pub const UNUSED_LOCAL_VARIABLE: CodeInfo = CodeInfo::new(
    "unused_local_variable",
    "UNUSED_LOCAL_VARIABLE",
    DiagnosticType::StaticWarning,
    true,
);
/// `Dead code.`
pub const DEAD_CODE: CodeInfo = CodeInfo::new(
    "dead_code",
    "DEAD_CODE",
    DiagnosticType::StaticWarning,
    true,
);

// --- Lints ---

/// `Unnecessary use of double quotes.`
pub const PREFER_SINGLE_QUOTES: CodeInfo = CodeInfo::new(
    "prefer_single_quotes",
    "PREFER_SINGLE_QUOTES",
    DiagnosticType::Lint,
    false,
);

// --- dartiq-own namespace (PRD §13.2): never part of the official value set,
// always rendered with type TOOL. ---

/// `package_config` is missing or stale; deep analysis is withheld.
pub const DARTIQ_PACKAGE_CONFIG_OUT_OF_DATE: CodeInfo = CodeInfo::new(
    "dartiq/package_config_out_of_date",
    "DARTIQ/PACKAGE_CONFIG_OUT_OF_DATE",
    DiagnosticType::Tool,
    false,
);
/// An internal panic/bug was downgraded to a per-file diagnostic (PRD §8).
pub const DARTIQ_INTERNAL_ERROR: CodeInfo = CodeInfo::new(
    "dartiq/internal_error",
    "DARTIQ/INTERNAL_ERROR",
    DiagnosticType::Tool,
    false,
);
/// A memory-budget downgrade step fired (PRD §11).
pub const DARTIQ_MEMORY_DEGRADED: CodeInfo = CodeInfo::new(
    "dartiq/memory_degraded",
    "DARTIQ/MEMORY_DEGRADED",
    DiagnosticType::Tool,
    false,
);
/// A build-generated source is imported but missing (PRD §17).
pub const DARTIQ_MISSING_GENERATED_SOURCE: CodeInfo = CodeInfo::new(
    "dartiq/missing_generated_source",
    "DARTIQ/MISSING_GENERATED_SOURCE",
    DiagnosticType::Tool,
    false,
);

/// The hand-written codes above: M0-measured families outside the generated
/// table, plus the dartiq-own namespace.
const OWN_CODES: [CodeInfo; 12] = [
    DUPLICATE_DEFINITION,
    AWAIT_IN_WRONG_CONTEXT,
    URI_DOES_NOT_EXIST,
    UNDEFINED_IDENTIFIER,
    ARGUMENT_TYPE_NOT_ASSIGNABLE,
    UNUSED_LOCAL_VARIABLE,
    DEAD_CODE,
    PREFER_SINGLE_QUOTES,
    DARTIQ_PACKAGE_CONFIG_OUT_OF_DATE,
    DARTIQ_INTERNAL_ERROR,
    DARTIQ_MEMORY_DEGRADED,
    DARTIQ_MISSING_GENERATED_SOURCE,
];

/// Name → [`CodeInfo`] lookup over the builtin code set (generated official
/// table + hand-written own codes).
///
/// Build once at startup via [`CodeRegistry::builtin`]; lookups are then plain
/// map hits. Keys are exact code names (`snake_case`; dartiq-own names include
/// the `dartiq/` prefix).
#[derive(Debug, Clone)]
pub struct CodeRegistry {
    by_name: BTreeMap<&'static str, CodeInfo>,
}

impl CodeRegistry {
    /// Builds the registry over all builtin codes.
    #[must_use]
    pub fn builtin() -> Self {
        Self {
            by_name: OFFICIAL_CODES
                .iter()
                .copied()
                .chain(OWN_CODES)
                .map(|c| (c.name, c))
                .collect(),
        }
    }

    /// Looks a code up by exact name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<CodeInfo> {
        self.by_name.get(name).copied()
    }

    /// Iterates all builtin codes, ordered by name.
    pub fn iter(&self) -> impl Iterator<Item = CodeInfo> + '_ {
        self.by_name.values().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Table shape: 262 syntax codes + 3 language-version codes, globally
    /// unique names, machine forms are the uppercase names, and every row is
    /// reachable through the registry next to the hand-written own codes.
    #[test]
    fn generated_table_is_complete_and_wellformed() {
        assert_eq!(OFFICIAL_CODES.len(), 265);
        assert_eq!(OFFICIAL_CODE_DATA.len(), OFFICIAL_CODES.len());
        let registry = CodeRegistry::builtin();
        for code in OFFICIAL_CODES {
            assert_eq!(code.machine, code.name.to_uppercase());
            assert_eq!(registry.get(code.name), Some(*code));
        }
        let mut names: Vec<&str> = OFFICIAL_CODES.iter().map(|c| c.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), OFFICIAL_CODES.len());
        for code in OWN_CODES {
            assert_eq!(registry.get(code.name), Some(code));
        }
        assert_eq!(
            registry.iter().count(),
            OFFICIAL_CODES.len() + OWN_CODES.len()
        );
    }

    /// Audit F7: every `SYNTACTIC_ERROR` code carries oracle severity ERROR
    /// (so emitters must emit severity `Error`), and the oracle-WARNING row
    /// is the static-warning family. The language-version ERROR rows are
    /// STATIC_WARNING-typed by class (facts §2/§3: class-level TYPE with
    /// per-code severity override), so severity ⇒ family holds only in the
    /// WARNING direction.
    #[test]
    fn dtype_matches_oracle_severity() {
        let registry = CodeRegistry::builtin();
        for (name, sev, _, _) in OFFICIAL_CODE_DATA {
            let code = registry.get(name).expect("code present in table");
            match *sev {
                "WARNING" => {
                    assert_eq!(code.dtype, DiagnosticType::StaticWarning, "{name}");
                }
                "ERROR" => {}
                other => panic!("{name}: unknown oracle SEV {other:?}"),
            }
            if code.dtype == DiagnosticType::SyntacticError {
                assert_eq!(
                    *sev, "ERROR",
                    "{name}: SYNTACTIC_ERROR codes must be oracle-ERROR"
                );
            }
        }
    }

    /// Message templates: placeholders are consecutive from `{0}` and never
    /// exceed the measured upper bound of 1 (2 arguments, e.g.
    /// `conflicting_modifiers`); literal braces must survive untouched.
    /// Mirrors build.rs `placeholders`: a `{`, digit run, `}` scan, so
    /// multi-digit indices (`{10}`) are visible here too.
    #[test]
    fn message_placeholders_are_consecutive_and_bounded() {
        for (name, _, message, _) in OFFICIAL_CODE_DATA {
            let mut highest: u32 = 0;
            for (index, _) in placeholder_spans(message) {
                assert!(
                    index <= 1,
                    "{name}: placeholder index {index} out of bounds"
                );
                assert_eq!(index, highest, "{name}: placeholders must be consecutive");
                highest = index + 1;
            }
        }
        assert_eq!(
            message_template("named_parameter_outside_group"),
            Some("Named parameters must be enclosed in curly braces ('{' and '}').")
        );
    }

    /// `(index, byte offset)` of every `{N}` placeholder — the test-module
    /// twin of build.rs `placeholders` (single pass, digit run between `{`
    /// and `}`, multi-digit indices included).
    fn placeholder_spans(message: &str) -> Vec<(u32, usize)> {
        let bytes = message.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'{' {
                let digits = bytes[i + 1..]
                    .iter()
                    .take_while(|&&b| b.is_ascii_digit())
                    .count();
                if digits > 0 && bytes.get(i + 1 + digits) == Some(&b'}') {
                    let index: u32 = message[i + 1..i + 1 + digits]
                        .parse()
                        .expect("ASCII digits fit u32");
                    out.push((index, i));
                    i += digits + 2;
                    continue;
                }
            }
            i += 1;
        }
        out
    }

    /// Audit A5: the scan must see multi-digit placeholders and skip literal
    /// braces exactly like the build-time validator (the build.rs twin is
    /// exercised by every rebuild over the real TSV).
    #[test]
    fn placeholder_scan_handles_multi_digit_and_literals() {
        assert_eq!(placeholder_spans("{0} and {1}"), vec![(0, 0), (1, 8)]);
        assert_eq!(placeholder_spans("up to {10}"), vec![(10, 6)]);
        assert_eq!(placeholder_spans("{' literal {} brace"), vec![]);
        assert_eq!(placeholder_spans("{0x} {-1} {x0}"), vec![]);
        assert_eq!(placeholder_spans("{{0}}"), vec![(0, 1)]);
    }

    /// The 19 pre-generation constants keep their exact name, machine form
    /// and family across the migration.
    #[test]
    fn legacy_constants_keep_their_shape() {
        let registry = CodeRegistry::builtin();
        let expected: [(&str, DiagnosticType); 19] = [
            ("expected_token", DiagnosticType::SyntacticError),
            ("missing_identifier", DiagnosticType::SyntacticError),
            (
                "unterminated_string_literal",
                DiagnosticType::SyntacticError,
            ),
            ("break_outside_of_loop", DiagnosticType::SyntacticError),
            ("encoding", DiagnosticType::SyntacticError),
            (
                "unexpected_dollar_in_string",
                DiagnosticType::SyntacticError,
            ),
            ("duplicate_definition", DiagnosticType::CompileTimeError),
            ("await_in_wrong_context", DiagnosticType::CompileTimeError),
            ("uri_does_not_exist", DiagnosticType::CompileTimeError),
            ("undefined_identifier", DiagnosticType::CompileTimeError),
            (
                "argument_type_not_assignable",
                DiagnosticType::CompileTimeError,
            ),
            ("unused_local_variable", DiagnosticType::StaticWarning),
            (
                "invalid_language_version_override",
                DiagnosticType::StaticWarning,
            ),
            ("dead_code", DiagnosticType::StaticWarning),
            ("prefer_single_quotes", DiagnosticType::Lint),
            ("dartiq/package_config_out_of_date", DiagnosticType::Tool),
            ("dartiq/internal_error", DiagnosticType::Tool),
            ("dartiq/memory_degraded", DiagnosticType::Tool),
            ("dartiq/missing_generated_source", DiagnosticType::Tool),
        ];
        for (name, dtype) in expected {
            assert_eq!(registry.get(name).expect(name).dtype, dtype, "{name}");
        }
    }

    #[test]
    fn message_template_lookup() {
        assert_eq!(
            message_template("expected_token"),
            Some("Expected to find '{0}'.")
        );
        assert_eq!(message_template("no_such_code"), None);
        // dartiq-own codes are not in the official table.
        assert_eq!(message_template("dartiq/internal_error"), None);
    }

    /// Audit F1: exactly 15 published codes among the 265 table rows — the 13
    /// `hasPublishedDocs: true` syntax codes (analyzer-8.4.1
    /// `syntactic_errors.g.dart`) plus `invalid_language_version_override`
    /// (all 9 of its variants are published) and
    /// `inconsistent_language_version_override`; `illegal_language_version_override`
    /// and every scanner code are unpublished
    /// (`_fe_analyzer_shared-91.0.0` `scanner/errors.g.dart`). `expected_token`
    /// is unpublished — the golden tests pin its missing `documentation` key.
    #[test]
    fn published_docs_flags_match_oracle_registry() {
        let is_published = |name: &str| {
            OFFICIAL_CODE_DATA
                .iter()
                .find(|(n, ..)| *n == name)
                .expect("code present in table")
                .3
        };
        assert_eq!(
            OFFICIAL_CODE_DATA
                .iter()
                .filter(|(.., published)| *published)
                .count(),
            15
        );
        for name in [
            "abstract_sealed_class",
            "default_value_in_function_type",
            "empty_record_literal_with_comma",
            "extension_declares_constructor",
            "field_initializer_outside_constructor",
            "record_literal_one_positional_no_trailing_comma",
            "variable_pattern_keyword_in_declaration_context",
            "wrong_number_of_parameters_for_setter",
            "invalid_language_version_override",
            "inconsistent_language_version_override",
        ] {
            assert!(is_published(name), "{name} must be published");
        }
        for name in [
            "expected_token",
            "unterminated_string_literal",
            "encoding",
            "illegal_language_version_override",
        ] {
            assert!(!is_published(name), "{name} must be unpublished");
        }
        // dartiq-own codes never carry a published docs page.
        for code in OWN_CODES {
            if code.name.starts_with("dartiq/") {
                assert!(!code.has_published_docs, "{}", code.name);
            }
        }
    }
}
