//! The SDK facade pins (ticket #2005): the lints-corpus acceptance files run
//! through the real pipeline over the generated flat existence face and the
//! generated SDK export faces. Each test is the in-repo twin of the corpus
//! differential rows — the fixture content is the tracked corpus file
//! itself, so the pin and the differential can never drift apart.

mod common;

use common::Fixture;
use dartq_engine::CancellationToken;

const JS_INTEROP_FAMILY: &str =
    include_str!("../../../tests/corpus/lints/lib/js_interop_family.dart");
const DIRECTIVES_ORDERING: &str =
    include_str!("../../../tests/corpus/lints/lib/directives_ordering.dart");
const COMBINATORS_ORDERING: &str =
    include_str!("../../../tests/corpus/lints/lib/combinators_ordering.dart");

/// One diagnostic as `code: message` — enough to pin the face rows the
/// corpus differential keys on.
fn analyze(source: &str) -> Vec<String> {
    let fixture = Fixture::new(&[("lib/target.dart", source)]);
    let mut engine = fixture.engine();
    engine.discover_workspace().expect("discovery");
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("analyze");
    diagnostics
        .iter()
        .map(|diagnostic| format!("{}: {}", diagnostic.code.name, diagnostic.message))
        .collect()
}

fn codes(rows: &[String]) -> Vec<&str> {
    rows.iter()
        .map(|row| row.split(": ").next().expect("code prefix"))
        .collect()
}

#[test]
fn js_interop_family_uri_and_annotation_faces_resolve() {
    // The fixture SDK's `vm` segment lists only core/math — dart:js_interop
    // resolves through the flat existence face, and the joined import face
    // carries the annotation classes, so the two dartq-only FP rows of the
    // pre-facade shape (`uri_does_not_exist` + `undefined_annotation` pair)
    // are gone. The file's remaining expected rows are the js-interop
    // runtime-check semantic face — deliberately not this ticket's.
    let rows = analyze(JS_INTEROP_FAMILY);
    assert!(
        rows.is_empty(),
        "js_interop_family expects zero rows at this wave, got {rows:?}"
    );
}

#[test]
fn combinators_ordering_export_faces_match_oracle_rows() {
    // The generated faces close the census gap rows exactly: `show
    // Completer` resolves (dart:async exports it — the FP is gone), `hide
    // Base64` reports (dart:convert never exported a bare `Base64`), and the
    // otherwise-unused dart:convert import reports — the two oracle rows the
    // faceless posture silently skipped, with no new dartq-only rows.
    let rows = analyze(COMBINATORS_ORDERING);
    assert!(
        !codes(&rows).contains(&"undefined_shown_name"),
        "`show Completer, Stream` must resolve against dart:async: {rows:?}"
    );
    let hidden = rows
        .iter()
        .filter(|row| row.starts_with("undefined_hidden_name"))
        .count();
    assert_eq!(
        hidden, 3,
        "`hide zeta, alpha` (dart:math) plus `hide Base64` (dart:convert): {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|row| row.starts_with("undefined_hidden_name") && row.contains("Base64")),
        "the `hide Base64` row must be present: {rows:?}"
    );
    let unused = rows
        .iter()
        .filter(|row| row.starts_with("unused_import"))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        unused.len(),
        3,
        "dart:math, dart:async, dart:convert are all unused imports: {unused:?}"
    );
    assert!(
        unused.iter().any(|row| row.contains("dart:convert")),
        "the dart:convert unused import must be reported: {unused:?}"
    );
}

#[test]
fn directives_ordering_dart_html_resolves_without_uri_error() {
    // `dart:html` resolves through the flat existence face: no
    // `uri_does_not_exist` ERROR on the import. The package:meta row stays —
    // the fixture carries no meta package, and the oracle reports the same.
    let rows = analyze(DIRECTIVES_ORDERING);
    let offenders: Vec<&String> = rows
        .iter()
        .filter(|row| row.starts_with("uri_does_not_exist") && row.contains("dart:html"))
        .collect();
    assert!(
        offenders.is_empty(),
        "dart:html must resolve through the flat face: {offenders:?}"
    );
    assert!(
        !codes(&rows).contains(&"undefined_annotation"),
        "no annotation face regressions: {rows:?}"
    );
    // The full face join makes the genuinely-unused dart:html import report
    // — the oracle-parity row the faceless posture skipped.
    assert!(
        rows.iter()
            .any(|row| row.starts_with("unused_import") && row.contains("dart:html")),
        "dart:html unused import must be reported once the face joins: {rows:?}"
    );
}
