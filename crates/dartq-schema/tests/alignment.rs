//! Alignment of the published schemas with what the real producers emit:
//! genuine `dartq-ai` impact and compatibility reports must pass validation,
//! and single-field mutations must fail with the violation named. The
//! diagnostics payloads mirror the envelope `dartq-diagnostics` actually
//! writes (`{"version":1,…}`). Hand-derived per-face fixtures with writer
//! provenance live in `tests/output_faces.rs`.

use dartq_ai::compatibility::{COMPAT_SCHEMA_VERSION, CompatReport};
use dartq_ai::explain::{
    ConstructorRow, DeclarationRow, DependencyRow, EXPLAIN_SCHEMA_VERSION, ExplainReport,
    MemberRow, ReferenceRow, SectionCounts, SuperTypeRow,
};
use dartq_ai::impact::{
    AffectedTarget, AffectedTest, ChangedSymbol, EnvironmentImpact, IMPACT_SCHEMA_VERSION,
    ImpactLevel, ImpactReport,
};
use dartq_schema::{
    Domain, SCHEMA_VERSION_COMPAT, SCHEMA_VERSION_DIAGNOSTICS, SCHEMA_VERSION_EXPLAIN,
    SCHEMA_VERSION_IMPACT, SchemaViolation, Verbosity, validate_at_tier,
};

/// Pin: dartq-ai owns the authoritative impact contract version; the schema
/// crate's mirror must never drift.
#[test]
fn impact_version_constant_matches_dartq_ai() {
    assert_eq!(SCHEMA_VERSION_IMPACT, IMPACT_SCHEMA_VERSION);
}

/// Pin: dartq-ai owns the authoritative compatibility contract version.
#[test]
fn compat_version_constant_matches_dartq_ai() {
    assert_eq!(SCHEMA_VERSION_COMPAT, COMPAT_SCHEMA_VERSION);
}

fn fixture_report() -> ImpactReport {
    ImpactReport {
        schema_version: IMPACT_SCHEMA_VERSION,
        changed: vec![ChangedSymbol {
            symbol: "a.dart|fb".to_owned(),
            signature_changed: true,
        }],
        affected: vec![
            AffectedTarget {
                path: "b.dart".to_owned(),
                impact: ImpactLevel::Semantic,
            },
            AffectedTarget {
                path: "c.dart".to_owned(),
                impact: ImpactLevel::ContentOnly,
            },
        ],
        affected_tests: vec![AffectedTest {
            path: "a_test.dart".to_owned(),
        }],
        affected_environments: vec![EnvironmentImpact {
            label: "default".to_owned(),
            compatible: true,
        }],
    }
}

#[test]
fn real_impact_report_passes_validation() {
    let payload = serde_json::to_string(&fixture_report()).expect("serialize");
    validate_ok(&payload, Domain::Impact);
}

/// Tier-invariant face (crate docs, "Verbosity tiers"): every impact field
/// is a locator or a verdict, so the at-tier check is the union check.
#[test]
fn impact_report_is_tier_invariant() {
    let payload = serde_json::to_string(&fixture_report()).expect("serialize");
    for tier in [Verbosity::Minimal, Verbosity::Compact, Verbosity::Full] {
        assert!(
            validate_at_tier(&payload, Domain::Impact, tier).is_ok(),
            "impact report must validate at the {tier} tier"
        );
    }
}

#[test]
fn empty_impact_report_passes_validation() {
    let report = ImpactReport {
        schema_version: IMPACT_SCHEMA_VERSION,
        changed: Vec::new(),
        affected: Vec::new(),
        affected_tests: Vec::new(),
        affected_environments: Vec::new(),
    };
    let payload = serde_json::to_string(&report).expect("serialize");
    validate_ok(&payload, Domain::Impact);
}

/// The session fields ride the impact envelope absent-tolerant: a
/// daemon answer carrying the symbol table and a delta cursor validates.
#[test]
fn impact_report_with_session_fields_passes_validation() {
    let mut value = serde_json::to_value(fixture_report()).expect("value");
    let object = value.as_object_mut().expect("object");
    object.insert(
        "symbols".to_owned(),
        serde_json::json!({"s1": "a.dart|fb", "s2": "package:foo/a.dart|fb.fma"}),
    );
    object.insert("since".to_owned(), serde_json::json!(1_700_000_000));
    validate_ok(&value.to_string(), Domain::Impact);
}

#[test]
fn impact_symbol_table_value_without_path_form_is_named() {
    let mut value = serde_json::to_value(fixture_report()).expect("value");
    value["symbols"] = serde_json::json!({"s1": "fb"});
    let violation = violation_of(&value.to_string(), Domain::Impact);
    assert_eq!(violation.path, "symbols.s1");
    assert_eq!(violation.expected, "a `label|name` path reference");
}

#[test]
fn dropped_schema_version_is_named() {
    let mut value = serde_json::to_value(fixture_report()).expect("value");
    value
        .as_object_mut()
        .expect("object")
        .remove("schema_version");
    let violation = violation_of(&value.to_string(), Domain::Impact);
    assert_eq!(violation.path, "schema_version");
    assert_eq!(violation.found, "missing");
}

/// v2: `signature_changed` consults member-level signature digests instead
/// of the v1 name-presence proxy (#1561) — the former pin 1 is now stale.
#[test]
fn wrong_schema_version_is_named() {
    let mut value = serde_json::to_value(fixture_report()).expect("value");
    value["schema_version"] = 1.into();
    let violation = violation_of(&value.to_string(), Domain::Impact);
    assert_eq!(violation.path, "schema_version");
    assert_eq!(violation.expected, "the integer 2");
    assert_eq!(violation.found, "1");
}

#[test]
fn misspelled_impact_level_is_named() {
    let mut value = serde_json::to_value(fixture_report()).expect("value");
    value["affected"][0]["impact"] = "semanticx".into();
    let violation = violation_of(&value.to_string(), Domain::Impact);
    assert_eq!(violation.path, "affected[0].impact");
    assert_eq!(violation.expected, "\"content-only\" or \"semantic\"");
    assert_eq!(violation.found, "\"semanticx\"");
}

#[test]
fn bare_label_symbol_ref_is_rejected() {
    let mut value = serde_json::to_value(fixture_report()).expect("value");
    value["changed"][0]["symbol"] = "fb".into();
    let violation = violation_of(&value.to_string(), Domain::Impact);
    assert_eq!(violation.path, "changed[0].symbol");
    assert_eq!(violation.expected, "a `label|name` path reference");
}

#[test]
fn undeclared_impact_field_is_named() {
    let mut value = serde_json::to_value(fixture_report()).expect("value");
    value["changed"][0]["severity"] = 1.into();
    let violation = violation_of(&value.to_string(), Domain::Impact);
    assert_eq!(violation.path, "changed[0].severity");
    assert_eq!(violation.found, "an undeclared field");
}

#[test]
fn non_json_payload_names_the_syntax_error() {
    let violation = dartq_schema::validate("not json", Domain::Impact)
        .expect_err("syntax failure must violate");
    assert_eq!(violation.path, "");
    assert_eq!(violation.expected, "a JSON document");
}

// ---------------------------------------------------------------------------
// Compatibility: a genuine `dartq-ai` report end to end.
// ---------------------------------------------------------------------------

/// The three-shape matrix: clean, error-carrying, and SDK-load-failed axes —
/// the producer's own serialization must validate as published.
#[test]
fn real_compat_report_passes_validation() {
    let report = CompatReport {
        schema_version: COMPAT_SCHEMA_VERSION,
        file: "/w/lib/a.dart".to_owned(),
        matrix: vec![
            dartq_ai::compatibility::AxisCompat {
                sdk: "3.5.0".to_owned(),
                compatible: true,
                first_failure: None,
                error: None,
            },
            dartq_ai::compatibility::AxisCompat {
                sdk: "3.13.1".to_owned(),
                compatible: false,
                first_failure: Some(dartq_ai::compatibility::Failure {
                    code: "uri_does_not_exist".to_owned(),
                    file: "/w/lib/a.dart".to_owned(),
                    line: 1,
                    col: 8,
                    message: "Target of URI doesn't exist: dart:async".to_owned(),
                }),
                error: None,
            },
            dartq_ai::compatibility::AxisCompat {
                sdk: "/sdk/broken".to_owned(),
                compatible: false,
                first_failure: None,
                error: Some(dartq_ai::compatibility::AxisError {
                    kind: dartq_ai::compatibility::AxisError::SDK_UNAVAILABLE.to_owned(),
                    message: "no version file".to_owned(),
                }),
            },
        ],
    };
    let payload = serde_json::to_string(&report).expect("serialize");
    validate_ok(&payload, Domain::Compat);
}

fn envelope() -> serde_json::Value {
    serde_json::json!({
        "version": SCHEMA_VERSION_DIAGNOSTICS,
        "diagnostics": [{
            "code": "undefined_identifier",
            "severity": "ERROR",
            "type": "COMPILE_TIME_ERROR",
            "location": {
                "file": "lib/main.dart",
                "range": {
                    "start": {"offset": 0, "line": 1, "column": 1},
                    "end": {"offset": 8, "line": 1, "column": 9},
                },
            },
            "problemMessage": "Undefined name 'fa'.",
            "correctionMessage": "Try importing the library that defines 'fa'.",
        }],
    })
}

#[test]
fn diagnostics_envelope_passes_validation() {
    validate_ok(&envelope().to_string(), Domain::Diagnostics);
}

/// The oracle emits the bare envelope even with no diagnostics; the schema
/// must accept exactly that line.
#[test]
fn bare_empty_envelope_passes_validation() {
    validate_ok("{\"version\":1,\"diagnostics\":[]}", Domain::Diagnostics);
}

#[test]
fn envelope_without_optional_keys_passes_validation() {
    let mut value = envelope();
    value["diagnostics"][0]
        .as_object_mut()
        .expect("object")
        .remove("correctionMessage");
    validate_ok(&value.to_string(), Domain::Diagnostics);
}

/// The minimal tier keeps exactly `code` + `location` per row —
/// a valid shape of the same envelope.
#[test]
fn minimal_diagnostic_row_passes_validation() {
    let mut value = envelope();
    value["diagnostics"][0] = serde_json::json!({
        "code": "undefined_identifier",
        "location": {
            "file": "lib/main.dart",
            "range": {
                "start": {"offset": 0, "line": 1, "column": 1},
                "end": {"offset": 8, "line": 1, "column": 9},
            },
        },
    });
    validate_ok(&value.to_string(), Domain::Diagnostics);
}

/// Red line: a row without its location is never valid, in any
/// tier.
#[test]
fn diagnostic_row_without_location_is_named() {
    let mut value = envelope();
    value["diagnostics"][0] = serde_json::json!({"code": "undefined_identifier"});
    let violation = violation_of(&value.to_string(), Domain::Diagnostics);
    assert_eq!(violation.path, "diagnostics[0].location");
    assert_eq!(violation.found, "missing");
}

#[test]
fn lowercase_severity_is_named() {
    let mut value = envelope();
    value["diagnostics"][0]["severity"] = "error".into();
    let violation = violation_of(&value.to_string(), Domain::Diagnostics);
    assert_eq!(violation.path, "diagnostics[0].severity");
    assert_eq!(violation.expected, "\"INFO\" or \"WARNING\" or \"ERROR\"");
    assert_eq!(violation.found, "\"error\"");
}

#[test]
fn zero_based_line_is_named() {
    let mut value = envelope();
    value["diagnostics"][0]["location"]["range"]["start"]["line"] = 0.into();
    let violation = violation_of(&value.to_string(), Domain::Diagnostics);
    assert_eq!(violation.path, "diagnostics[0].location.range.start.line");
    assert_eq!(violation.expected, "an integer >= 1");
}

#[test]
fn wrong_version_key_is_named() {
    let mut value = envelope();
    value["version"] = 2.into();
    let violation = violation_of(&value.to_string(), Domain::Diagnostics);
    assert_eq!(violation.path, "version");
    assert_eq!(violation.expected, "the integer 1");
}

/// Sanity pin: both published documents are well-formed draft-07 subset
/// documents carrying the right version constants.
#[test]
fn schema_documents_carry_the_version_constants() {
    let impact = dartq_schema::impact_schema();
    assert_eq!(impact["$schema"], "http://json-schema.org/draft-07/schema#");
    assert_eq!(impact["properties"]["schema_version"]["const"], 2);
    let diagnostics = dartq_schema::diagnostics_schema();
    assert_eq!(
        diagnostics["$schema"],
        "http://json-schema.org/draft-07/schema#"
    );
    assert_eq!(
        diagnostics["properties"]["version"]["const"],
        SCHEMA_VERSION_DIAGNOSTICS
    );
}

/// Pin: dartq-ai owns the authoritative explain contract version; the
/// schema crate's mirror must never drift.
#[test]
fn explain_version_constant_matches_dartq_ai() {
    assert_eq!(SCHEMA_VERSION_EXPLAIN, EXPLAIN_SCHEMA_VERSION);
}

/// The compact explain report a real assembler emits.
fn compact_explain_report() -> ExplainReport {
    ExplainReport {
        schema_version: EXPLAIN_SCHEMA_VERSION,
        symbol: "Foo".to_owned(),
        declaration: Some(DeclarationRow {
            file: "a.dart".to_owned(),
            name: "Foo".to_owned(),
            kind: "class".to_owned(),
            container: None,
            range: [40, 43],
        }),
        inheritance: Some(vec![SuperTypeRow {
            name: "Bar".to_owned(),
            file: Some("a.dart".to_owned()),
            clause: Some("extends".to_owned()),
        }]),
        interfaces: Some(Vec::new()),
        constructors: Some(vec![ConstructorRow {
            name: "Foo".to_owned(),
            kind: "constructor".to_owned(),
            signature: "Foo()".to_owned(),
        }]),
        methods: Some(vec![MemberRow {
            name: "area".to_owned(),
            kind: "method".to_owned(),
            range: [90, 93],
        }]),
        references: Some(vec![ReferenceRow {
            file: "b.dart".to_owned(),
            kind: "invoked".to_owned(),
            range: [10, 13],
            qualified: false,
        }]),
        dependencies: Some(vec![DependencyRow {
            path: "package:t/b.dart".to_owned(),
            name: None,
        }]),
        affected_tests: Some(vec![AffectedTest {
            path: "package:t/a_test.dart".to_owned(),
        }]),
        counts: None,
    }
}

/// The compact explain report a real assembler emits passes validation.
#[test]
fn real_explain_report_passes_validation() {
    let payload = serde_json::to_string(&compact_explain_report()).expect("serialize");
    validate_ok(&payload, Domain::Explain);
}

/// The minimal explain report — declaration plus counts, no arrays —
/// passes validation too.
#[test]
fn minimal_explain_report_passes_validation() {
    let report = ExplainReport {
        schema_version: EXPLAIN_SCHEMA_VERSION,
        symbol: "Foo".to_owned(),
        declaration: Some(DeclarationRow {
            file: "a.dart".to_owned(),
            name: "Foo".to_owned(),
            kind: "class".to_owned(),
            container: None,
            range: [40, 43],
        }),
        inheritance: None,
        interfaces: None,
        constructors: None,
        methods: None,
        references: None,
        dependencies: None,
        affected_tests: None,
        counts: Some(SectionCounts {
            inheritance: 1,
            interfaces: 0,
            constructors: 0,
            methods: 1,
            references: 1,
            dependencies: 1,
            affected_tests: 1,
        }),
    };
    let payload = serde_json::to_string(&report).expect("serialize");
    validate_ok(&payload, Domain::Explain);
}

/// Tier exactness over the real producers: the compact report is exact at
/// compact (and full — they coincide today), the minimal report at minimal
/// only; its `counts` object is the minimal tier's exclusive content.
#[test]
fn explain_reports_are_exact_at_their_tier() {
    let compact = serde_json::to_string(&compact_explain_report()).expect("serialize");
    assert!(validate_at_tier(&compact, Domain::Explain, Verbosity::Compact).is_ok());
    assert!(validate_at_tier(&compact, Domain::Explain, Verbosity::Full).is_ok());

    let minimal = ExplainReport {
        schema_version: EXPLAIN_SCHEMA_VERSION,
        symbol: "Foo".to_owned(),
        declaration: Some(DeclarationRow {
            file: "a.dart".to_owned(),
            name: "Foo".to_owned(),
            kind: "class".to_owned(),
            container: None,
            range: [40, 43],
        }),
        inheritance: None,
        interfaces: None,
        constructors: None,
        methods: None,
        references: None,
        dependencies: None,
        affected_tests: None,
        counts: Some(SectionCounts {
            inheritance: 1,
            interfaces: 0,
            constructors: 0,
            methods: 1,
            references: 1,
            dependencies: 1,
            affected_tests: 1,
        }),
    };
    let payload = serde_json::to_string(&minimal).expect("serialize");
    assert!(validate_at_tier(&payload, Domain::Explain, Verbosity::Minimal).is_ok());
    let violation = validate_at_tier(&payload, Domain::Explain, Verbosity::Compact)
        .expect_err("compact must not carry the minimal counts");
    assert_eq!(violation.path, "counts");
    assert_eq!(violation.expected, "omitted at the compact tier");
}

#[test]
fn misspelled_explain_member_kind_is_named() {
    let value = serde_json::to_value(ExplainReport {
        schema_version: EXPLAIN_SCHEMA_VERSION,
        symbol: "Foo".to_owned(),
        declaration: None,
        inheritance: Some(Vec::new()),
        interfaces: Some(Vec::new()),
        constructors: Some(Vec::new()),
        methods: Some(vec![MemberRow {
            name: "area".to_owned(),
            kind: "constructor".to_owned(),
            range: [90, 93],
        }]),
        references: Some(Vec::new()),
        dependencies: Some(Vec::new()),
        affected_tests: Some(Vec::new()),
        counts: None,
    })
    .expect("value");
    let violation = violation_of(&value.to_string(), Domain::Explain);
    assert_eq!(violation.path, "methods[0].kind");
    assert_eq!(violation.found, "\"constructor\"");
}

fn validate_ok(payload: &str, domain: Domain) {
    if let Err(violation) = dartq_schema::validate(payload, domain) {
        panic!("{domain:?} payload rejected: {violation}");
    }
}

fn violation_of(payload: &str, domain: Domain) -> SchemaViolation {
    dartq_schema::validate(payload, domain).expect_err("mutation must violate")
}
