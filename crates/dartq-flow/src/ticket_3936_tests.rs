//! #3936 pins — the shape-family `cannotMatch` state-split.
//!
//! Each of `analyzeListPattern`/`analyzeMapPattern`/`analyzeRecordPattern`
//! hands `promoteForPattern` a NON-NULLABLE required type — `List<…>`,
//! `Map<…>`, the record type (upstream Dart SDK source,
//! `pkg/_fe_analyzer_shared/lib/src/type_inference/type_analyzer.dart`:1003,
//! :1119, :1264). Under sound flow a null-or-equivalent matched value can
//! never match one, so `promoteForPattern`'s `classifyType` legs
//! (`pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart`
//! :6361-6388) mark the MATCHED arm unreachable while the failure join still
//! runs on the pre-verdict state — the else arm stays live.
//!
//! The ordering is the whole fix: killing the matched arm at the
//! `ShapePattern` event also kills the state the failure join reads, which
//! folds the `else` dead alongside the body. The kill commits AFTER the join.

#[cfg(test)]
mod tests {
    use crate::facts::FlowFact;
    use crate::forward::SharedFlow;
    use crate::record::Flow;
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_parser::ParserFeatures;
    use dartq_scanner::{ScanConfig, TokenTable};

    fn parse(source: &str) -> SharedFlow<'_> {
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
        let mut parser = dartq_parser::Parser::new(source, tokens, features, &mut flow);
        parser.parse_unit();
        drop(parser);
        flow
    }

    /// Every `dead_code` row's source text, in source order.
    fn dead(source: &str) -> Vec<String> {
        let flow = parse(source);
        let mut flow = flow.0.borrow_mut();
        let mut facts = flow.take_facts();
        facts.sort_by_key(|fact| match fact {
            FlowFact::DeadCode { span } => span.range.offset,
            _ => u32::MAX,
        });
        facts
            .iter()
            .filter_map(|fact| match fact {
                FlowFact::DeadCode { span } => {
                    Some(source[span.range.offset as usize..span.range.end() as usize].to_string())
                }
                _ => None,
            })
            .collect()
    }

    /// A list pattern against a `Null` scrutinee: the matched arm is dead,
    /// the else arm is live. Oracle: one `DEAD_CODE` over the `if` body.
    #[test]
    fn ticket_3936_null_scrutinee_list_pattern_kills_the_matched_arm() {
        let source = "void f(Null n) {\n\
                      \x20 if (n case []) {\n\
                      \x20   print('dead');\n\
                      \x20 } else {\n\
                      \x20   print('live');\n\
                      \x20 }\n\
                      }\n";
        let rows = dead(source);
        assert_eq!(rows.len(), 1, "one row for the matched arm: {rows:?}");
        assert!(
            rows[0].contains("dead") && !rows[0].contains("live"),
            "the row covers the matched arm only: {rows:?}"
        );
    }

    /// The map and record twins carry the same verdict.
    #[test]
    fn ticket_3936_null_scrutinee_map_and_record_patterns_kill_the_matched_arm() {
        for (label, pattern) in [("map", "{0: 0}"), ("record", "(_,)")] {
            let source = format!(
                "void f(Null n) {{\n\
                 \x20 if (n case {pattern}) {{\n\
                 \x20   print('dead');\n\
                 \x20 }} else {{\n\
                 \x20   print('live');\n\
                 \x20 }}\n\
                 }}\n"
            );
            let rows = dead(&source);
            assert_eq!(
                rows.len(),
                1,
                "{label}: one row for the matched arm: {rows:?}"
            );
            assert!(
                rows[0].contains("dead") && !rows[0].contains("live"),
                "{label}: the row covers the matched arm only: {rows:?}"
            );
        }
    }

    /// The relational exemption: `case == null` matches a null value, so
    /// neither arm may row dead.
    #[test]
    fn ticket_3936_relational_pattern_keeps_both_arms_live() {
        let source = "void f(Null n) {\n\
                      \x20 if (n case == null) {\n\
                      \x20   print('a');\n\
                      \x20 } else {\n\
                      \x20   print('b');\n\
                      \x20 }\n\
                      }\n";
        assert_eq!(
            dead(source),
            Vec::<String>::new(),
            "a relational pattern compares its operand and matches null"
        );
    }

    /// The negative control: a nullable (potentially-nullable) scrutinee
    /// against the same list pattern is compatible, so nothing rows dead.
    #[test]
    fn ticket_3936_nullable_scrutinee_list_pattern_keeps_both_arms_live() {
        let source = "void f(List<int>? n) {\n\
                      \x20 if (n case []) {\n\
                      \x20   print('a');\n\
                      \x20 } else {\n\
                      \x20   print('b');\n\
                      \x20 }\n\
                      }\n";
        assert_eq!(dead(source), Vec::<String>::new());
    }
}
