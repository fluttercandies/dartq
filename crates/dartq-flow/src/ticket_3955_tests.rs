//! #3955 pins — the `??=` RHS post-state must not escape the if-null join.
//!
//! Upstream splits at `ifNullExpression_rightBegin` and rejoins at
//! `ifNullExpression_end` with `_join(_current, context._shortcutState)`
//! (upstream Dart SDK source,
//! `pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart`
//! :5835-5868), so a promotion the RHS made cannot survive the operator.
//! The flattened replay ran the restore only when the RHS left the state
//! unreachable, so a REACHABLE RHS (`y ??= i!`) leaked its promotion into
//! the following statements and the trailing `i!` misreported. The fix
//! joins the reachable RHS post-state with the parked skip state — the
//! join drops every fact the two paths do not share.

#[cfg(test)]
mod tests {
    use crate::facts::FlowFact;
    use crate::forward::SharedFlow;
    use crate::record::Flow;
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_parser::ParserFeatures;
    use dartq_scanner::{ScanConfig, TokenTable};

    fn facts_of(source: &str) -> Vec<FlowFact> {
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
        flow.0.borrow_mut().take_facts()
    }

    /// Source text of every `PromotedRead` row, in fact order (offset
    /// sorted by [`Flow::take_facts`]). Promotion rows exist only for real
    /// promotions, so an absent row IS the unpromoted face.
    fn promoted_snippets(source: &str) -> Vec<String> {
        facts_of(source)
            .into_iter()
            .filter_map(|fact| match fact {
                FlowFact::PromotedRead { span, .. } => {
                    Some(source[span.range.offset as usize..span.range.end() as usize].to_string())
                }
                _ => None,
            })
            .collect()
    }

    /// Source text of every `DeadCode` row, in fact order.
    fn dead_snippets(source: &str) -> Vec<String> {
        facts_of(source)
            .into_iter()
            .filter_map(|fact| match fact {
                FlowFact::DeadCode { span } => {
                    Some(source[span.range.offset as usize..span.range.end() as usize].to_string())
                }
                _ => None,
            })
            .collect()
    }

    /// The minimal repro: `y ??= i!` promotes `i` on the RHS path alone,
    /// and the join with the skip path drops it — the trailing `i` reads
    /// unpromoted (no row) while the write's own promotion stands (the
    /// trailing `y` reads `int`). Oracle: silent on the trailing `i!`.
    #[test]
    fn ticket_3955_if_null_write_rhs_promotion_dies_at_the_join() {
        let source = "\
void g(int? i) {
  int? y;
  y ??= i!;
  i;
  y;
}
";
        assert_eq!(promoted_snippets(source), vec!["y".to_string()]);
    }

    /// The cascade shape: the next section's argument must read the
    /// unpromoted face — the section boundary is the join. Oracle: silent
    /// on both `!`s.
    #[test]
    fn ticket_3955_cascade_if_null_write_rhs_promotion_dies_at_the_join() {
        let source = "\
abstract class C {
  int? x;
  void f(int i);
}

void g(C c, int? i) {
  c..x ??= i!..f(i!);
}
";
        assert_eq!(promoted_snippets(source), Vec::<String>::new());
        // The section's LHS read is the member `int? x` — potentially
        // nullable, so the RHS path is live upstream and no dead row
        // exists. Oracle: silent on every row.
        assert_eq!(dead_snippets(source), Vec::<String>::new());
    }

    /// The within-RHS face is untouched: the second `i!` operand reads the
    /// promotion the first one staged (the join runs at the write, after
    /// the whole RHS replayed). Oracle: exactly one row, on the first `!`.
    #[test]
    fn ticket_3955_if_null_write_rhs_promotion_holds_within_the_rhs() {
        let source = "\
void g(int? i) {
  int? y;
  y ??= i! + i!;
}
";
        assert_eq!(promoted_snippets(source), vec!["i".to_string()]);
    }

    /// A condition-valued RHS rejoins its own arms first (the value face),
    /// then joins with the skip path — the equality fold's narrowed arm
    /// must not leak either. Oracle: silent on the trailing `i!`.
    #[test]
    fn ticket_3955_if_null_write_condition_rhs_promotion_dies_at_the_join() {
        let source = "\
void g(int? i) {
  int? y;
  y ??= i != null;
  i;
}
";
        assert_eq!(promoted_snippets(source), Vec::<String>::new());
    }

    /// The plain-RHS control: joining identical states is a no-op — the
    /// write's own promotion still stands and nothing else changes.
    #[test]
    fn ticket_3955_if_null_write_plain_rhs_join_is_a_no_op() {
        let source = "\
void g(int? i, int z) {
  int? y;
  y ??= z;
  i;
  y;
}
";
        assert_eq!(promoted_snippets(source), vec!["y".to_string()]);
    }
}
