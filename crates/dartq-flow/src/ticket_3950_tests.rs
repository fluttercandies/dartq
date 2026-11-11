//! #3950 pins — the null-aware dead window's two remaining roots.
//!
//! Root 1 (extension override receiver). `E(n)?.m()` over an extension
//! override whose argument is `Null` must row the member dead exactly like
//! `n?.m()` does. Upstream wraps the argument and starts the null-shorting
//! over THAT expression's static type (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/generated/resolver.dart:4814-4840`: the
//! `ExtensionOverride` arm of `_startNullAwareAccess` hands the
//! `argumentExpression` to `startNullShorting`), so the receiver face is the
//! argument's type — never the extension's own name. The event stream names
//! only the head (`E`), so the recorder collects the unit's named extensions
//! and the replay adopts the argument's face for such a send.
//!
//! Root 2 (null-aware index write). `c1n?[nv] = 0` opens a LIVE access path
//! (a potentially-nullable receiver) and the operand's own `Never` read kills
//! the walk; the RHS `0` is then the official's first dead node, and
//! `handleNullShortingFinished` runs `flowEnd` over the ASSIGNMENT
//! (`resolver.dart:1195-1200`, `type_analyzer.dart:2616`) — the row is `0`,
//! never `0;`. The flow side must therefore trim an interior-death window to
//! the assignment's end, not the statement's `;`.

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

    /// The oracle's row for `E(null)?.extensionMethod(1);`: the member name
    /// through the invocation's `)`. The override's value is its argument,
    /// `Null`, so the access path dies at the open.
    #[test]
    fn ticket_3950_extension_override_null_argument_rows_the_member_dead() {
        let source = "class C {\n\
                      \x20 void method(Object? o) {}\n\
                      }\n\
                      extension E on Null {\n\
                      \x20 C extensionMethod(Object? o) => C();\n\
                      }\n\
                      void f() {\n\
                      \x20 E(null)?.extensionMethod(1);\n\
                      }\n";
        assert_eq!(dead(source), vec!["extensionMethod(1)".to_string()]);
    }

    /// The `Null`-typed argument twin: `E(n)?.extensionMethod(1)` over
    /// `Null n` rows the same member (the argument's declared face is
    /// `Null`, so the open folds the access path dead).
    #[test]
    fn ticket_3950_extension_override_null_typed_argument_rows_the_member_dead() {
        let source = "class C {\n\
                      \x20 void method(Object? o) {}\n\
                      }\n\
                      extension E on Null {\n\
                      \x20 C extensionMethod(Object? o) => C();\n\
                      }\n\
                      void f(Null n) {\n\
                      \x20 E(n)?.extensionMethod(1);\n\
                      }\n";
        assert_eq!(dead(source), vec!["extensionMethod(1)".to_string()]);
    }

    /// The negative control: a LIVE argument keeps the chain live, so the
    /// override's own name must never arm a dead row. `E(c)` over a
    /// non-nullable `C` makes the SKIP path dead (sound flow), which rows
    /// nothing.
    #[test]
    fn ticket_3950_extension_override_live_argument_rows_nothing() {
        let source = "class C {\n\
                      \x20 void method(Object? o) {}\n\
                      }\n\
                      extension E on C {\n\
                      \x20 C extensionMethod(Object? o) => C();\n\
                      }\n\
                      void f(C c) {\n\
                      \x20 E(c)?.extensionMethod(1);\n\
                      }\n";
        assert_eq!(dead(source), Vec::<String>::new());
    }

    /// The plain-receiver control that must keep matching the oracle
    /// byte-for-byte: `n?.method(1)` on `Null n` rows `method(1)`.
    #[test]
    fn ticket_3950_plain_null_receiver_chain_still_rows_the_member() {
        let source = "class C {\n\
                      \x20 void method(Object? o) {}\n\
                      }\n\
                      void f(Null n) {\n\
                      \x20 n?.method(1);\n\
                      }\n";
        assert_eq!(dead(source), vec!["method(1)".to_string()]);
    }

    /// Root 2: a null-aware index WRITE whose operand kills the walk rows
    /// the assignment's right-hand side alone — the official `flowEnd` runs
    /// over the assignment, so the statement's `;` never joins the row.
    #[test]
    fn ticket_3950_null_aware_index_write_interior_death_rows_the_rhs() {
        let source = "class C1 {\n\
                      \x20 C1 operator [](Object? i) => this;\n\
                      \x20 operator []=(Object? i, Object? v) {}\n\
                      }\n\
                      void f(C1? c1n, Never nv) {\n\
                      \x20 c1n?[nv] = 0;\n\
                      }\n";
        assert_eq!(dead(source), vec!["0".to_string()]);
    }

    /// The same shape with a non-literal RHS: the row still ends at the
    /// assignment, so the operand's own text is the row.
    #[test]
    fn ticket_3950_null_aware_index_write_interior_death_rows_the_rhs_expression() {
        let source = "class C1 {\n\
                      \x20 C1 operator [](Object? i) => this;\n\
                      \x20 operator []=(Object? i, Object? v) {}\n\
                      }\n\
                      void f(C1? c1n, Never nv, int x) {\n\
                      \x20 c1n?[nv] = x;\n\
                      }\n";
        assert_eq!(dead(source), vec!["x".to_string()]);
    }

    /// The override's index-selector twin: `E(null)?[0] = 1` dies at the
    /// same open (the operand row `0] = 1` — the chain face).
    #[test]
    fn ticket_3950_extension_override_null_argument_index_write_rows_the_operand() {
        let source = "class C {\n\
                      \x20 C operator [](Object? i) => this;\n\
                      \x20 operator []=(Object? i, Object? v) {}\n\
                      }\n\
                      extension E on Null {\n\
                      \x20 C operator [](Object? i) => C();\n\
                      \x20 operator []=(Object? i, Object? v) {}\n\
                      }\n\
                      void f() {\n\
                      \x20 E(null)?[0] = 1;\n\
                      }\n";
        assert_eq!(dead(source), vec!["0] = 1".to_string()]);
    }

    /// The negative control: a live operand and a live RHS row nothing.
    #[test]
    fn ticket_3950_null_aware_index_write_live_operand_rows_nothing() {
        let source = "class C1 {\n\
                      \x20 C1 operator [](Object? i) => this;\n\
                      \x20 operator []=(Object? i, Object? v) {}\n\
                      }\n\
                      void f(C1? c1n, int i) {\n\
                      \x20 c1n?[i] = 0;\n\
                      }\n";
        assert_eq!(dead(source), Vec::<String>::new());
    }

    /// The dead-at-open write keeps its own face: `Null`-typed receiver,
    /// live operand and RHS — the chain row starts at the operand and runs
    /// to the assignment's end.
    #[test]
    fn ticket_3950_null_aware_index_write_dead_at_open_keeps_the_chain_row() {
        let source = "class C1 {\n\
                      \x20 C1 operator [](Object? i) => this;\n\
                      \x20 operator []=(Object? i, Object? v) {}\n\
                      }\n\
                      void f(Null n, int i, int j) {\n\
                      \x20 n?[i] = j;\n\
                      }\n";
        assert_eq!(dead(source), vec!["i] = j".to_string()]);
    }
}
