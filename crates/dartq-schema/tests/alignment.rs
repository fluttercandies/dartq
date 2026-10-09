//! Alignment of the published schemas with what the real producers emit:
//! a genuine `dartq-ai` impact report must pass validation, and single-field
//! mutations must fail with the violation named. The diagnostics payloads
//! mirror the envelope `dartq-diagnostics` actually writes (`{"version":1,…}`).

use dartq_ai::impact::{
    AffectedTarget, AffectedTest, ChangedSymbol, EnvironmentImpact, IMPACT_SCHEMA_VERSION,
    ImpactLevel, ImpactReport,
};
use dartq_schema::{Domain, SCHEMA_VERSION_DIAGNOSTICS, SCHEMA_VERSION_IMPACT, SchemaViolation};

/// Pin: dartq-ai owns the authoritative impact contract version; the schema
/// crate's mirror must never drift.
#[test]
fn impact_version_constant_matches_dartq_ai() {
    assert_eq!(SCHEMA_VERSION_IMPACT, IMPACT_SCHEMA_VERSION);
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

#[test]
fn wrong_schema_version_is_named() {
    let mut value = serde_json::to_value(fixture_report()).expect("value");
    value["schema_version"] = 2.into();
    let violation = violation_of(&value.to_string(), Domain::Impact);
    assert_eq!(violation.path, "schema_version");
    assert_eq!(violation.expected, "the integer 1");
    assert_eq!(violation.found, "2");
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
fn missing_problem_message_is_named() {
    let mut value = envelope();
    value["diagnostics"][0]
        .as_object_mut()
        .expect("object")
        .remove("problemMessage");
    let violation = violation_of(&value.to_string(), Domain::Diagnostics);
    assert_eq!(violation.path, "diagnostics[0].problemMessage");
    assert_eq!(violation.found, "missing");
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
    assert_eq!(impact["properties"]["schema_version"]["const"], 1);
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

fn validate_ok(payload: &str, domain: Domain) {
    if let Err(violation) = dartq_schema::validate(payload, domain) {
        panic!("{domain:?} payload rejected: {violation}");
    }
}

fn violation_of(payload: &str, domain: Domain) -> SchemaViolation {
    dartq_schema::validate(payload, domain).expect_err("mutation must violate")
}
