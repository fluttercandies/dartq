//! Stable diagnostic codes: the official analyzer code set dartq aligns to,
//! plus dartq's own `dartq/` namespace (PRD §13.2).
//!
//! The official set — 255 parser/scanner syntax codes plus the 3-code
//! `// @dart=` language-version family — is generated at build time from
//! `data/syntax-codes.tsv`, the authoritative table captured in
//! `docs/facts/syntax-codes.md` §9 (dart 3.13.1 oracle lock). To update it,
//! edit the TSV and rebuild; the per-code constants, the [`OFFICIAL_CODES`]
//! slice, and [`OFFICIAL_CODE_DATA`] are regenerated. Only codes outside that
//! table (the M0-measured compile-time/warning/lint set and the dartq-own
//! namespace) are hand-written below.

use std::collections::BTreeMap;

use crate::model::DiagnosticType;

/// A stable diagnostic code: official `snake_case` name, its `UPPERCASE` machine
/// form, the diagnostic family it belongs to, and whether the official
/// documentation page is published.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CodeInfo {
    /// Official code name, `snake_case`, one-to-one with the analyzer's
    /// `analyzerCode` (e.g. `expected_token`); dartq-own codes carry the
    /// `dartq/` prefix (e.g. `dartq/internal_error`).
    pub name: &'static str,
    /// `UPPERCASE` form as emitted in the machine format's `CODE` field
    /// (e.g. `EXPECTED_TOKEN`).
    pub machine: &'static str,
    /// The diagnostic family this code belongs to.
    pub dtype: DiagnosticType,
    /// Whether the analyzer registry marks this code `hasPublishedDocs: true`.
    /// The oracle's JSON `documentation` field (`errors.dart` `url` getter)
    /// exists only for published codes; dartq-own codes are always `false`.
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

// The `invalid_language_version_override` row keeps the vintage registry's
// representative variant template (AT_SIGN — "The Dart language version
// override number must begin with '@dart'."). dartq's production emission is
// the GREATER variant, whose text is pinned at its emit sites
// (dartq-engine/dartq-cli); both variant texts are in analyzer-8.4.1
// `error/codes.g.dart` — AT_SIGN 11527-11540, GREATER 11560-11571.

/// Message template of an official code, by exact name (TSV column 5; `{0}`/
/// `{1}` are substitution placeholders). Official codes only — dartq-own
/// codes carry their messages at the emit site.
///
/// Test-only: emit sites carry their message texts locally, so the template
/// payload would otherwise ship dead in release binaries. Wiring the emitters
/// to the registry as the single message source is a recorded follow-up.
#[cfg(test)]
#[must_use]
pub fn message_template(name: &str) -> Option<&'static str> {
    OFFICIAL_CODE_DATA
        .iter()
        .find(|(n, ..)| *n == name)
        .map(|(_, _, message, _)| *message)
}

// A code's [`DiagnosticType`] is the machine format's TYPE value. Generated
// rows are SYNTACTIC_ERROR (parser/scanner class types), except the
// language-version family typed by registry class: invalid_language_version_
// override is WarningCode-class STATIC_WARNING, while illegal/inconsistent_
// language_version_override are CompileTimeErrorCode-class COMPILE_TIME_ERROR
// (analyzer-8.4.1 codes.g.dart; re-verified in the pinned live registry —
// diagnostic.g.dart:7356-7365 and :7946-7956 vs :9184-9330, dart 3.15 main).
// Codes the oracle routes by hand to COMPILE_TIME_ERROR at emit time (facts
// §2 pseudoSharedCode routing) are emit-site decisions, not registry facts.

// --- M0-measured codes outside the generated syntax table ---
// hasPublishedDocs values captured from the generated registries
// (analyzer-8.4.1 error/codes.g.dart; re-verified in the pinned live
// registry, diagnostic.g.dart, dart 3.15 main); lints are never published
// (`LintCode.url` is overridden to `null`), and dartq-own codes never are.

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

// --- dartq-own namespace (PRD §13.2): never part of the official value set,
// always rendered with type TOOL. ---

/// `package_config` is missing or stale; deep analysis is withheld.
pub const DARTQ_PACKAGE_CONFIG_OUT_OF_DATE: CodeInfo = CodeInfo::new(
    "dartq/package_config_out_of_date",
    "DARTQ/PACKAGE_CONFIG_OUT_OF_DATE",
    DiagnosticType::Tool,
    false,
);
/// An internal panic/bug was downgraded to a per-file diagnostic (PRD §8).
pub const DARTQ_INTERNAL_ERROR: CodeInfo = CodeInfo::new(
    "dartq/internal_error",
    "DARTQ/INTERNAL_ERROR",
    DiagnosticType::Tool,
    false,
);
/// A memory-budget downgrade step fired (PRD §11).
pub const DARTQ_MEMORY_DEGRADED: CodeInfo = CodeInfo::new(
    "dartq/memory_degraded",
    "DARTQ/MEMORY_DEGRADED",
    DiagnosticType::Tool,
    false,
);
/// A build-generated source is imported but missing (PRD §17).
pub const DARTQ_MISSING_GENERATED_SOURCE: CodeInfo = CodeInfo::new(
    "dartq/missing_generated_source",
    "DARTQ/MISSING_GENERATED_SOURCE",
    DiagnosticType::Tool,
    false,
);
/// A plugin process died or broke the wire contract (PRD §14.3 crash and
/// memory isolation; the restart budget's exhaustion is the terminal form).
pub const DARTQ_PLUGIN_CRASHED: CodeInfo = CodeInfo::new(
    "dartq/plugin_crashed",
    "DARTQ/PLUGIN_CRASHED",
    DiagnosticType::Tool,
    false,
);
/// A plugin overran a wall-clock deadline (handshake/shutdown request budget
/// or the per-callback budget on `request/fixes`); the child is killed
/// (PRD §14.3 CPU and IPC rows).
pub const DARTQ_PLUGIN_TIMEOUT: CodeInfo = CodeInfo::new(
    "dartq/plugin_timeout",
    "DARTQ/PLUGIN_TIMEOUT",
    DiagnosticType::Tool,
    false,
);

/// The hand-written codes above: M0-measured families outside the generated
/// table, plus the dartq-own namespace.
const OWN_CODES: [CodeInfo; 14] = [
    DUPLICATE_DEFINITION,
    AWAIT_IN_WRONG_CONTEXT,
    URI_DOES_NOT_EXIST,
    UNDEFINED_IDENTIFIER,
    ARGUMENT_TYPE_NOT_ASSIGNABLE,
    UNUSED_LOCAL_VARIABLE,
    DEAD_CODE,
    PREFER_SINGLE_QUOTES,
    DARTQ_PACKAGE_CONFIG_OUT_OF_DATE,
    DARTQ_INTERNAL_ERROR,
    DARTQ_MEMORY_DEGRADED,
    DARTQ_MISSING_GENERATED_SOURCE,
    DARTQ_PLUGIN_CRASHED,
    DARTQ_PLUGIN_TIMEOUT,
];

/// Name → [`CodeInfo`] lookup over the builtin code set (generated official
/// table + hand-written own codes).
///
/// Build once at startup via [`CodeRegistry::builtin`]; lookups are then plain
/// map hits. Keys are exact code names (`snake_case`; dartq-own names include
/// the `dartq/` prefix).
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

    /// Table shape: 255 syntax codes + 3 language-version codes (the 7
    /// measured-dead rows were pruned from the snapshot, #222), globally
    /// unique names, machine forms are the uppercase names, and every row is
    /// reachable through the registry next to the hand-written own codes.
    #[test]
    fn generated_table_is_complete_and_wellformed() {
        assert_eq!(OFFICIAL_CODES.len(), 258);
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
    /// (so emitters must emit severity `Error`), the oracle-WARNING row is
    /// the static-warning family, and ERROR rows never carry a warning
    /// family. TYPE follows the oracle registry class, not per-row severity:
    /// `illegal/inconsistent_language_version_override` are CompileTime-
    /// ErrorCode entries (diagnostic.g.dart:7356-7365 and :7946-7956, ERROR +
    /// COMPILE_TIME_ERROR) while `invalid_language_version_override` is
    /// WarningCode (diagnostic.g.dart:9184-9330, WARNING + STATIC_WARNING) —
    /// severity alone does not decide the family.
    #[test]
    fn dtype_matches_oracle_severity() {
        let registry = CodeRegistry::builtin();
        for (name, sev, _, _) in OFFICIAL_CODE_DATA {
            let code = registry.get(name).expect("code present in table");
            match *sev {
                "WARNING" => {
                    assert_eq!(code.dtype, DiagnosticType::StaticWarning, "{name}");
                }
                "ERROR" => {
                    assert_ne!(
                        code.dtype,
                        DiagnosticType::StaticWarning,
                        "{name}: ERROR rows are never STATIC_WARNING-typed"
                    );
                }
                other => panic!("{name}: unknown oracle SEV {other:?}"),
            }
            if code.dtype == DiagnosticType::SyntacticError {
                assert_eq!(
                    *sev, "ERROR",
                    "{name}: SYNTACTIC_ERROR codes must be oracle-ERROR"
                );
            }
        }
        // Audit #170 F1/F2 pin: the two ERROR-level language-version rows are
        // CompileTimeErrorCode (COMPILE_TIME_ERROR), not WarningCode.
        for name in [
            "illegal_language_version_override",
            "inconsistent_language_version_override",
        ] {
            assert_eq!(
                registry.get(name).expect(name).dtype,
                DiagnosticType::CompileTimeError,
                "{name}"
            );
        }
        assert_eq!(
            registry
                .get("invalid_language_version_override")
                .expect("present")
                .dtype,
            DiagnosticType::StaticWarning
        );
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
            ("dartq/package_config_out_of_date", DiagnosticType::Tool),
            ("dartq/internal_error", DiagnosticType::Tool),
            ("dartq/memory_degraded", DiagnosticType::Tool),
            ("dartq/missing_generated_source", DiagnosticType::Tool),
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
        // dartq-own codes are not in the official table.
        assert_eq!(message_template("dartq/internal_error"), None);
    }

    /// Audit F1: exactly 15 published codes among the 258 table rows — the 13
    /// `hasPublishedDocs: true` syntax codes (captured from analyzer-8.4.1
    /// `syntactic_errors.g.dart`, re-verified in the pinned live registry
    /// `diagnostic.g.dart`, which absorbed the syntactic and scanner rows)
    /// plus `invalid_language_version_override` (all 9 of its variants are
    /// published, diagnostic.g.dart:9184-9330) and
    /// `inconsistent_language_version_override` (:7946-7956);
    /// `illegal_language_version_override` (:7356-7365) and every scanner
    /// code are unpublished — no `hasPublishedDocs` in `diagnostic.g.dart`
    /// (the 8.4.1-vintage `_fe_analyzer_shared-91.0.0` `scanner/errors.g.dart`
    /// is not in the pinned checkout). `expected_token` is unpublished — the
    /// golden tests pin its missing `documentation` key.
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
        // dartq-own codes never carry a published docs page.
        for code in OWN_CODES {
            if code.name.starts_with("dartq/") {
                assert!(!code.has_published_docs, "{}", code.name);
            }
        }
    }

    /// The hand-written constants and the semantic table define the same
    /// official codes independently (the constants feed the golden tests, the
    /// semantic emit path builds its own descriptors), so nothing else ties
    /// them together. Pin the shared-name pairs' string identity here — drift
    /// on either side fails loudly. `DEAD_CODE` and `AWAIT_IN_WRONG_CONTEXT`
    /// have no semantic counterpart: they sit outside the frozen M1 gate set.
    #[test]
    fn hand_written_constants_match_semantic_table() {
        use dartq_semantic::codes::SemanticCode;

        let pairs = [
            (DUPLICATE_DEFINITION, SemanticCode::DuplicateDefinition),
            (UNDEFINED_IDENTIFIER, SemanticCode::UndefinedIdentifier),
            (
                ARGUMENT_TYPE_NOT_ASSIGNABLE,
                SemanticCode::ArgumentTypeNotAssignable,
            ),
        ];
        for (constant, semantic) in pairs {
            let info = semantic.code_info();
            assert_eq!(constant.name, info.name, "{}", constant.machine);
            assert_eq!(constant.machine, info.machine, "{}", constant.name);
        }
    }

    /// The resolution family (dartq-resolution `ResolutionCode`, frozen at
    /// resolution-design-spec §4.2) shares official codes with this registry,
    /// but dartq-resolution is not a dependency of this crate, so the 15
    /// name/machine pairs are pinned as string literals transcribed from its
    /// `ResolutionCode::info` — a spelling change on either side must update
    /// both and fails here until it does. The pair that also lives above as a
    /// hand-written constant is tied to it.
    #[test]
    fn resolution_codes_match_literal_table() {
        const RESOLUTION_PAIRS: [(&str, &str); 15] = [
            ("uri_does_not_exist", "URI_DOES_NOT_EXIST"),
            (
                "uri_does_not_exist_in_doc_import",
                "URI_DOES_NOT_EXIST_IN_DOC_IMPORT",
            ),
            ("uri_has_not_been_generated", "URI_HAS_NOT_BEEN_GENERATED"),
            ("invalid_uri", "INVALID_URI"),
            ("uri_with_interpolation", "URI_WITH_INTERPOLATION"),
            ("use_of_native_extension", "USE_OF_NATIVE_EXTENSION"),
            ("import_of_non_library", "IMPORT_OF_NON_LIBRARY"),
            ("export_of_non_library", "EXPORT_OF_NON_LIBRARY"),
            ("part_of_non_part", "PART_OF_NON_PART"),
            ("part_of_different_library", "PART_OF_DIFFERENT_LIBRARY"),
            ("part_of_unnamed_library", "PART_OF_UNNAMED_LIBRARY"),
            ("duplicate_part", "DUPLICATE_PART"),
            ("ambiguous_import", "AMBIGUOUS_IMPORT"),
            ("ambiguous_export", "AMBIGUOUS_EXPORT"),
            ("shared_deferred_prefix", "SHARED_DEFERRED_PREFIX"),
        ];
        for (name, machine) in RESOLUTION_PAIRS {
            assert_eq!(machine, name.to_uppercase(), "{name}: name/machine drift");
        }
        assert_eq!(URI_DOES_NOT_EXIST.name, "uri_does_not_exist");
        assert_eq!(URI_DOES_NOT_EXIST.machine, "URI_DOES_NOT_EXIST");
    }
}
