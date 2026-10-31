//! #3079 — dead-code pins for REDIRECTING initializer entries
//! (`this.other()` / `super.raw(0)`), transcribed from `dart analyze`
//! 3.13.1 package-context A/B over /tmp/dc3079 fixtures. The upstream
//! expectation is the same shape as the field-entry row
//! (`upstream Dart SDK source, pkg/analyzer/lib/src/error/
//! dead_code_verifier.dart:284-291`, `parent is ConstructorInitializer` →
//! the report spans `parent.offset..parent.end` — the ENTRY's own extent,
//! regardless of which initializer kind it is).
//!
//! A redirecting entry closes its constructor-initializer window like any
//! other entry: the parser fires `end_initializer` on the redirect face
//! (`finish_redirecting_initializer` / `parse_super_initializer_expression`
//! both run `parseInitializerExpressionRest`), the recorder pairs it with
//! `CtorInitBegin`, and the interp rows the entry extent when the walk
//! reached it dead — `C() : x = throw 0, this.other();` reports
//! `this.other()`, and a third entry keeps the official one-interval fold.

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// Dead-code facts of `source` as the exact source text they cover.
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
            FlowFact::DeadCode { span } => {
                Some(source[span.range.offset as usize..span.range.end() as usize].to_string())
            }
            _ => None,
        })
        .collect()
}

// —— redirecting initializer entry rows ——————————————————————————————————

#[test]
fn this_redirect_entry_rows_own_extent() {
    // Oracle 3:22|12 — the row is the redirect entry `this.other()`; the
    // body never folds in (dead_code_verifier.dart:284-291).
    let src = "class C {\n  int x;\n  C() : x = throw 0, this.other();\n  C.other();\n}\n";
    assert_eq!(dead_of(src), vec!["this.other()".to_string()]);
}

#[test]
fn super_redirect_entry_rows_own_extent() {
    // Oracle 6:22|12 — the same face through the `super.name(...)` form.
    let src = "class C {\n  C.raw(int v);\n}\nclass D extends C {\n  int x;\n  D() : x = throw 0, super.raw(0);\n}\n";
    assert_eq!(dead_of(src), vec!["super.raw(0)".to_string()]);
}

#[test]
fn redirect_entry_row_folds_following_entry() {
    // Oracle 3:22|12 — `x = 1` after the dead redirect continues the one
    // dead interval; the redirect entry owns the single row.
    let src = "class C {\n  int x;\n  C() : x = throw 0, this.other(), x = 1;\n  C.other();\n}\n";
    assert_eq!(dead_of(src), vec!["this.other()".to_string()]);
}

// —— live redirect guards —————————————————————————————————————————————————

#[test]
fn live_this_redirect_stays_silent() {
    // No throw ahead: the redirect entry is live — no dead rows.
    let src = "class C {\n  C() : this.other();\n  C.other();\n}\n";
    assert!(dead_of(src).is_empty());
}

#[test]
fn live_super_redirect_stays_silent() {
    let src = "class C {\n  C.raw(int v);\n}\nclass D extends C {\n  D() : super.raw(0);\n}\n";
    assert!(dead_of(src).is_empty());
}

#[test]
fn live_field_then_redirect_stays_silent() {
    // A live field entry ahead of the redirect: neither rows.
    let src = "class C {\n  int x;\n  C() : x = 1, this.other();\n  C.other();\n}\n";
    assert!(dead_of(src).is_empty());
}
