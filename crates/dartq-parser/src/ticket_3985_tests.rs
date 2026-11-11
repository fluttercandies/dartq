//! Ticket #3985 — the `ensure_identifier` event gate now fires
//! `handle_identifier` for the metadata contexts.
//!
//! A junk follower of `@` (a second `@`, a literal — anything outside the
//! keep set) falls into the recovery's consume-in-doubt tail, which inserts
//! the synthetic identifier. Upstream reports the name unconditionally:
//! `MetadataReferenceIdentifierContext.ensureIdentifier`
//! (upstream Dart SDK source, `identifier_context_impl.dart`
//! :1085-1093) inserts the synthetic and `parseMetadata`'s
//! `ensureIdentifier` wrapper fires `handleIdentifier` on whatever came back
//! (`parser_impl.dart` :4031-4040). dartq's event gate omitted the
//! `MetadataReference` / `MetadataContinuation` /
//! `MetadataContinuationAfterTypeArguments` contexts, so `class C
//! { @ @annotation var f; }`'s first `@` contributed no `MetadataReference`
//! event — the semantic latch never staged the `''` annotation and the
//! oracle's `undefined_annotation ''` row was missed. These pins lock the
//! restored event.

use crate::parser::Parser;
use crate::tests::collector::{Collector, latest_features, scan_ok};

struct Out {
    errors: Vec<(String, u32, u32)>,
    idents: Vec<(String, u32, u32, usize)>,
}

fn parse(source: &'static str) -> Out {
    let (table, text) = scan_ok(source);
    let listener: &'static mut Collector = Box::leak(Box::new(Collector::default()));
    let mut parser = Parser::new(text, table, latest_features(), listener);
    parser.parse_unit();
    Out {
        errors: parser.listener.errors.clone(),
        idents: parser.listener.idents.clone(),
    }
}

/// `class C { @ @annotation var f; }`: the first `@`'s junk follower (the
/// second `@`) drops into the consume-in-doubt tail — `missing_identifier`
/// at the `@` and a `MetadataReference` event on the synthetic name — then
/// the second `@`'s metadata parses `annotation` normally.
///
/// Oracle rows (machine format):
///   `UNDEFINED_ANNOTATION 1:11 len 2`  ('' — the first `@`'s synthetic)
///   `MISSING_IDENTIFIER     1:13 len 1`  (at the second `@`)
///   `UNDEFINED_ANNOTATION 1:13 len 11` ('annotation')
#[test]
fn at_at_annotation_first_at_fires_metadata_reference() {
    let out = parse("class C { @ @annotation var f; }\n");
    assert!(
        out.errors
            .iter()
            .any(|(c, s, e)| c == "missing_identifier" && *s == 12 && *e == 13),
        "missing_identifier must anchor on the second `@` (12,13): {:?}",
        out.errors
    );
    let metadata: Vec<_> = out
        .idents
        .iter()
        .filter(|(ctx, _, _, _)| ctx == "MetadataReference")
        .collect();
    assert_eq!(
        metadata.len(),
        2,
        "expected the first `@`'s synthetic + `annotation`'s real name, got {metadata:?} (all idents: {:?})",
        out.idents
    );
    // The first `@`'s synthetic name is zero-width at the second `@`'s
    // offset; `annotation`'s event names the real span.
    assert!(
        metadata.iter().any(|(_, s, e, _)| s == e),
        "first `@`'s synthetic MetadataReference must be zero-width: {metadata:?}"
    );
    assert!(
        metadata.iter().any(|(_, s, e, _)| *s == 13 && *e == 23),
        "`annotation`'s MetadataReference must span (13,23): {metadata:?}"
    );
}

/// The field declarator's own name still fires `FieldDeclaration` after the
/// recovered metadata — the event-gate change inserts an event, not a
/// different recovery, so the downstream declaration face is undisturbed.
#[test]
fn at_at_annotation_field_declaration_still_fires() {
    let out = parse("class C { @ @annotation var f; }\n");
    assert!(
        out.idents
            .iter()
            .any(|(ctx, s, e, _)| ctx == "FieldDeclaration" && *s == 28 && *e == 29),
        "`f`'s FieldDeclaration event must span (28,29): {:?}",
        out.idents
    );
}
