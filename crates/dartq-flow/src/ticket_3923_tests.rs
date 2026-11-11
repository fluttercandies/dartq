//! #3923 pins — the inc/dec write arm stores the operator's result type.
//!
//! `i++` on a promoted `int` local must keep the promotion: upstream's
//! resolver hands flow `SharedTypeView(operatorReturnType)` (upstream Dart
//! SDK source,
//! `pkg/analyzer/lib/src/dart/resolver/postfix_expression_resolver.dart`
//! :174-186) and `_write` demotes against THAT type, so a write whose result
//! still sits under the standing promotion keeps it
//! (`pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart`:6319,
//! :7881-7905). Writing the declared face instead demotes the local on every
//! `i++` — `covers(Object, int)` is false — and every later read answers the
//! unpromoted `Object`, which is the `undefined_operator` family the arm
//! owned.
//!
//! These pins read the flow layer's own [`FlowFact::ReadType`] faces: the
//! read after the increment must carry the promoted head, not the declared
//! one.

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

    /// Every `ReadType` row as `(identifier text, flow face)`, in source
    /// order.
    fn reads(source: &str) -> Vec<(String, String)> {
        let flow = parse(source);
        let mut flow = flow.0.borrow_mut();
        let mut facts = flow.take_facts();
        facts.sort_by_key(|fact| match fact {
            FlowFact::ReadType { span, .. } => span.range.offset,
            _ => u32::MAX,
        });
        facts
            .iter()
            .filter_map(|fact| match fact {
                FlowFact::ReadType { span, ty, .. } => Some((
                    source[span.range.offset as usize..span.range.end() as usize].to_string(),
                    face(*ty, &flow),
                )),
                _ => None,
            })
            .collect()
    }

    fn pairs(rows: &[(&str, &str)]) -> Vec<(String, String)> {
        rows.iter()
            .map(|(name, face)| ((*name).to_string(), (*face).to_string()))
            .collect()
    }

    /// The headline: after `i++` on a promoted `int`, the next read is still
    /// `int`. Oracle-verified — `i is! int` returns early, so the only
    /// reachable reads are on the promoted face and `d += 1`'s operator
    /// resolves on `int` (no `undefined_operator`).
    #[test]
    fn ticket_3923_inc_dec_keeps_the_promoted_face() {
        let source = "void f(Object i) {\n\
                      \x20 if (i is! int) return;\n\
                      \x20 i++;\n\
                      \x20 i;\n\
                      }\n";
        assert_eq!(
            reads(source),
            pairs(&[("i", "Object"), ("i", "int"), ("i", "int")]),
            "the read after `i++` keeps the promoted int face"
        );
    }

    /// The prefix and postfix twins, each followed by a read on the same
    /// line-independent path: every form keeps the promotion.
    #[test]
    fn ticket_3923_all_four_inc_dec_forms_keep_the_promotion() {
        let source = "void f(Object i) {\n\
                      \x20 if (i is! int) return;\n\
                      \x20 ++i;\n\
                      \x20 i;\n\
                      \x20 --i;\n\
                      \x20 i;\n\
                      \x20 i++;\n\
                      \x20 i;\n\
                      \x20 i--;\n\
                      \x20 i;\n\
                      }\n";
        assert_eq!(
            reads(source),
            pairs(&[
                ("i", "Object"),
                ("i", "int"),
                ("i", "int"),
                ("i", "int"),
                ("i", "int"),
                ("i", "int"),
                ("i", "int"),
                ("i", "int"),
                ("i", "int"),
            ]),
            "every inc/dec form keeps the promoted face"
        );
    }

    /// The negative control: an inc/dec on a local that was never promoted
    /// stays on the declared face — the arm must not invent a promotion.
    #[test]
    fn ticket_3923_unpromoted_local_stays_on_the_declared_face() {
        let source = "void f(Object i) {\n\
                      \x20 i++;\n\
                      \x20 i;\n\
                      }\n";
        assert_eq!(
            reads(source),
            pairs(&[("i", "Object"), ("i", "Object")]),
            "no promotion is invented for an unpromoted local"
        );
    }
}
