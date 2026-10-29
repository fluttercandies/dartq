//! The #2830 pins: the recovered `?..{ ... }` cascade section must not
//! kill its enclosing executable.
//!
//! Upstream face: no grammar gives a cascade section a block body, so the
//! official parser's recovery drops the section body — its flow model
//! never sees the braces, and `a?..{ return foo; }` leaves the rest of
//! the enclosing function live. Our parser instead re-parses the braces
//! as a sibling statement; the recorder drops that block's events (the
//! `?..{` fingerprint in `begin_cascade`, the drop in `begin_block`), so
//! the re-parsed `return` cannot kill the enclosing path.

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// Dead-code facts of `source` as source text (the production channel:
/// `fold_unit_with_ops` reads `features.sound_flow_analysis` into
/// `Flow::set_sound_flow`).
fn dead_of(source: &str) -> Vec<String> {
    let result = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        dartq_source::FileTable::new().intern(std::path::Path::new("test.dart")),
    );
    assert!(result.diagnostics.is_empty(), "{source:?}");
    let tokens: &'static TokenTable = Box::leak(Box::new(result.tokens));
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let mut flow = SharedFlow(std::rc::Rc::new(std::cell::RefCell::new(Flow::new(
        tokens, source,
    ))));
    flow.0
        .borrow_mut()
        .set_sound_flow(features.sound_flow_analysis);
    let mut parser = dartq_parser::Parser::new(source, tokens, features, &mut flow);
    parser.parse_unit();
    drop(parser);
    flow.0
        .borrow_mut()
        .take_facts()
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::DeadCode { span } => Some(
                source
                    [span.range.offset as usize..(span.range.offset + span.range.length) as usize]
                    .to_string(),
            ),
            _ => None,
        })
        .collect()
}

#[test]
fn recovered_cascade_block_does_not_kill() {
    for (label, src) in [
        (
            "initializer",
            "class C {}\nint foo = 42;\nvoid use(Object? x) {}\nvoid f() { C? buffer; final v1 = buffer?..{\n  return foo;\n};\nuse(v1);\n}",
        ),
        (
            "nested",
            "class C {}\nint foo = 42;\nvoid use(Object? x) {}\nvoid f() { C? buffer; final v2 = (buffer?..{\n  return foo;\n})?..{\n  return foo;\n};\nuse(v2);\n}",
        ),
        (
            "statement",
            "class C { void g() {} }\nvoid f() { C? buffer; buffer?..{\n  return 1;\n};\nbuffer?..g();\n}",
        ),
        (
            "chained_bare_sections",
            "class C {}\nint foo = 42;\nvoid use(Object? x) {}\nvoid f() { C? buffer; buffer?..{\n  return foo;\n}..{\n  return foo;\n};\nuse(buffer);\n}",
        ),
        (
            "anonymous_method_section",
            "class C {}\nvoid use(Object? x) {}\nvoid f() { C? buffer; final v = buffer?..(int p) {\n  return p;\n};\nuse(v);\n}",
        ),
    ] {
        let dead = dead_of(src);
        assert!(dead.is_empty(), "{label}: {dead:?}");
    }
}

#[test]
fn real_sibling_block_after_cascade_still_kills() {
    // The `?..{` fingerprint arms on `{` directly after the operator only:
    // a REAL block statement after a healthy cascade keeps its events, so
    // its `return` kills the tail (the plain statement-block face).
    let src = "class C { void g() {} }\nvoid use(Object? x) {}\nvoid f() { C? buffer; buffer?..g();\n{\n  return 1;\n}\nuse(buffer);\n}";
    assert_eq!(dead_of(src), vec!["use(buffer);".to_string()]);
}

#[test]
fn recovered_block_drops_close_at_their_own_end_block() {
    // Two recovered blocks in a row: each drop must close at its own
    // `EndBlock` — a leak would swallow the second cascade's framing or
    // resurrect the first block's `return` as a live-path kill.
    let src = "class C { void g() {} }\nvoid use(Object? x) {}\nvoid f() { C? b1; C? b2; b1?..{\n  return 1;\n};\nb2?..{\n  return 2;\n};\nuse(b1);\nuse(b2);\n}";
    let dead = dead_of(src);
    assert!(dead.is_empty(), "{dead:?}");
}
