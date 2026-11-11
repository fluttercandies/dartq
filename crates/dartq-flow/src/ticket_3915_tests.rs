//! #3915 pins — the flow-crate continuation packet: the live read-latch
//! bridge's publication cursor (#3914 segment 1) and the dead-code residual
//! clusters whose roots live in this crate.
//!
//! # The publication cursor (#3914 segment 1)
//!
//! The semantic layer's `Engine::read_types` latch has one writer today: the
//! post-parse fill in `diagnose.rs`'s `fold_unit_with_ops`, which runs after
//! `parser.parse_unit`. Every listener fold that runs *during* the parse
//! (the binary and conditional fold callbacks) therefore consults an empty
//! latch and falls back to the declared face. Upstream keeps the promotion
//! state live while typing — a promoted read's static type is answered by
//! the local-variable type provider during resolution (upstream Dart SDK
//! source, `pkg/analyzer/lib/src/dart/resolver/flow_analysis_visitor.dart`
//! :1394-1417, `flow.promotedType`).
//!
//! Flow's replay runs per executable *inside* the parse (the recorder's
//! `exec_end`), so every executable's rows are known before the parse
//! finishes — the cursor below ([`Flow::published_facts`] /
//! [`Flow::mark_facts_published`]) publishes them at that point. These pins
//! freeze the cursor's contract: every read row is published, the published
//! order is source order per executable, the spans are the read tokens, and
//! the whole-unit view (`Flow::take_facts`) is unchanged — the post-parse
//! consumer keeps working.

#[cfg(test)]
mod tests {
    use crate::facts::FlowFact;
    use crate::forward::SharedFlow;
    use crate::record::Flow;
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_parser::ParserFeatures;
    use dartq_scanner::{ScanConfig, TokenTable};

    /// Parses with the recorder attached; the recorder keeps every row, so
    /// the publication cursor and `take_facts` can both be read.
    fn parse_only(source: &str) -> SharedFlow<'_> {
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

    /// A type's rendered face: the head's lexeme with `?` when nullable.
    fn face(ty: crate::ty::Ty, flow: &Flow<'_>) -> String {
        let suffix = if ty.nullable { "?" } else { "" };
        match ty.base {
            crate::ty::Base::Name(id) => format!("{}{suffix}", flow.name_lexeme(id)),
            crate::ty::Base::Dynamic => format!("dynamic{suffix}"),
            crate::ty::Base::Void => format!("void{suffix}"),
            crate::ty::Base::Never => format!("Never{suffix}"),
            crate::ty::Base::Null => format!("Null{suffix}"),
            crate::ty::Base::Function => format!("Function{suffix}"),
            crate::ty::Base::Unknown => format!("Unknown{suffix}"),
        }
    }

    /// `(offset, source text)` of every `ReadType` row, in the order given.
    fn read_rows(source: &str, facts: &[FlowFact]) -> Vec<(u32, String)> {
        facts
            .iter()
            .filter_map(|fact| match fact {
                FlowFact::ReadType { span, .. } => Some((
                    span.range.offset,
                    source[span.range.offset as usize..span.range.end() as usize].to_string(),
                )),
                _ => None,
            })
            .collect()
    }

    /// Every `dead_code` row's source text, in fact order.
    fn dead(source: &str) -> Vec<String> {
        let flow = parse_only(source);
        let mut flow = flow.0.borrow_mut();
        flow.take_facts()
            .iter()
            .filter_map(|fact| match fact {
                FlowFact::DeadCode { span } => {
                    Some(source[span.range.offset as usize..span.range.end() as usize].to_string())
                }
                _ => None,
            })
            .collect()
    }

    /// Cluster 1 (`issue_51230`, 24 extras): a case head whose binder writes
    /// a type this lattice cannot spell — a record type (`final () b`,
    /// `final (int, String) b`, `final () _`) — is refutable upstream, so the
    /// following members stay live. Oracle: no `dead_code` row at all.
    #[test]
    fn ticket_3915_record_typed_binder_keeps_the_switch_chain_live() {
        let source = "void f(o) {\n\
                      \x20 switch (o) {\n\
                      \x20   case final int a:\n\
                      \x20     break;\n\
                      \x20   case final () b:\n\
                      \x20     break;\n\
                      \x20   case String c:\n\
                      \x20     break;\n\
                      \x20 }\n\
                      }\n";
        assert_eq!(
            dead(source),
            Vec::<String>::new(),
            "a record-typed binder's clause cannot cover the scrutinee"
        );
    }

    /// The typed-wildcard twin: `case final () _` carries the same
    /// unspellable written annotation and must not kill the chain either.
    #[test]
    fn ticket_3915_record_typed_wildcard_keeps_the_switch_chain_live() {
        let source = "void f(o) {\n\
                      \x20 switch (o) {\n\
                      \x20   case final int a:\n\
                      \x20     break;\n\
                      \x20   case final () _:\n\
                      \x20     break;\n\
                      \x20   case String c:\n\
                      \x20     break;\n\
                      \x20 }\n\
                      }\n";
        assert_eq!(dead(source), Vec::<String>::new());
    }

    /// The record biner's negative twin: a *spellable* type that covers the
    /// scrutinee still kills the chain (the kill itself must stay armed).
    /// `bool` splits every value, so a `case bool b` head leaves the later
    /// member dead — the oracle's `unreachable_switch_case` face.
    #[test]
    fn ticket_3915_covering_binder_still_kills_the_chain() {
        let source = "void f(bool o) {\n\
                      \x20 switch (o) {\n\
                      \x20   case bool b:\n\
                      \x20     break;\n\
                      \x20   case true:\n\
                      \x20     break;\n\
                      \x20 }\n\
                      }\n";
        assert!(
            !dead(source).is_empty(),
            "a covering binder must still kill the chain"
        );
    }

    /// Cluster 3 (`pattern_not_matching_null_test`, `pattern_not_matching_
    /// non_nullable_test`): a declared-variable pattern whose required type is
    /// disjoint from the scrutinee's nullability kills the MATCHED arm only.
    /// Oracle: the `if` body rows dead, the `else` stays live.
    #[test]
    fn ticket_3915_null_scrutinee_typed_pattern_kills_the_matched_arm() {
        let source = "void f(Null n) {\n\
                      \x20 if (n case int i) {\n\
                      \x20   print(i);\n\
                      \x20 } else {\n\
                      \x20   print('live');\n\
                      \x20 }\n\
                      }\n";
        let rows = dead(source);
        assert_eq!(
            rows.len(),
            1,
            "one row for the dead matched arm only: {rows:?}"
        );
        assert!(
            rows[0].contains("print(i)") && !rows[0].contains("live"),
            "the row covers the matched arm's block, never the else: {rows:?}"
        );
    }

    /// The negative control: a potentially-nullable scrutinee against a
    /// non-nullable required type is compatible, so neither arm rows dead.
    #[test]
    fn ticket_3915_nullable_scrutinee_typed_pattern_leaves_both_arms_live() {
        let source = "void f(int? n) {\n\
                      \x20 if (n case int i) {\n\
                      \x20   print(i);\n\
                      \x20 } else {\n\
                      \x20   print('live');\n\
                      \x20 }\n\
                      }\n";
        assert_eq!(dead(source), Vec::<String>::new());
    }

    /// The fixture: one promoted read inside an `is`-narrowed branch beside
    /// an unpromoted read after it, in two executables so the cursor spans
    /// more than one publish point.
    const TWO_BODIES: &str = "void f(num n) {\n\
                              \x20 if (n is int) {\n\
                              \x20   n + 1;\n\
                              \x20 }\n\
                              \x20 n;\n\
                              }\n\
                              void g(int? m) {\n\
                              \x20 if (m != null) {\n\
                              \x20   m;\n\
                              \x20 }\n\
                              }\n";

    /// Every read row the replay produced is reachable through the cursor:
    /// the published stream is the whole-unit stream (nothing is dropped or
    /// reordered), and the read spans point at the read tokens.
    #[test]
    fn ticket_3915_published_cursor_carries_every_read_row() {
        let flow = parse_only(TWO_BODIES);
        let mut flow = flow.0.borrow_mut();
        let published = read_rows(TWO_BODIES, flow.published_facts());
        let whole = read_rows(TWO_BODIES, &flow.take_facts());
        assert_eq!(
            published, whole,
            "the cursor publishes exactly the whole-unit read rows"
        );
        assert_eq!(
            published
                .iter()
                .map(|(_, text)| text.as_str())
                .collect::<Vec<_>>(),
            vec!["n", "n", "n", "m", "m"],
            "every read carries its identifier span"
        );
        let offsets = published
            .iter()
            .map(|(offset, _)| *offset)
            .collect::<Vec<_>>();
        let mut sorted = offsets.clone();
        sorted.sort_unstable();
        assert_eq!(offsets, sorted, "publication order is source order");
    }

    /// The cursor is incremental and idempotent: after a mark it reports
    /// nothing new, and it never removes rows from the whole-unit view the
    /// semantic layer's post-parse fill reads.
    #[test]
    fn ticket_3915_published_cursor_is_incremental_and_keeps_take_facts_whole() {
        let flow = parse_only(TWO_BODIES);
        let mut flow = flow.0.borrow_mut();
        let first = flow.published_facts().len();
        assert!(first > 0, "the cursor starts with the unit's rows");
        flow.mark_facts_published();
        assert!(
            flow.published_facts().is_empty(),
            "a marked cursor reports nothing new"
        );
        flow.mark_facts_published();
        assert!(
            flow.published_facts().is_empty(),
            "marking twice is a no-op"
        );
        assert_eq!(
            flow.take_facts().len(),
            first,
            "the cursor never removes rows from the whole-unit view"
        );
    }

    /// The published read rows carry the promoted face, which is the whole
    /// point of the bridge: the in-branch `n` read is `int` (the promotion
    /// the fold is missing), the post-branch one `num`.
    #[test]
    fn ticket_3915_published_read_rows_carry_the_promoted_face() {
        let flow = parse_only(TWO_BODIES);
        let mut flow = flow.0.borrow_mut();
        let faces = flow
            .published_facts()
            .iter()
            .filter_map(|fact| match fact {
                FlowFact::ReadType { ty, .. } => Some(face(*ty, &flow)),
                _ => None,
            })
            .collect::<Vec<_>>();
        let whole = flow
            .take_facts()
            .iter()
            .filter_map(|fact| match fact {
                FlowFact::ReadType { ty, .. } => Some(face(*ty, &flow)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(faces, whole, "the cursor keeps the row payloads");
        assert_eq!(
            faces,
            vec!["num", "int", "num", "int?", "int"],
            "the branch read is published on its promoted face"
        );
    }
}
