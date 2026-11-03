//! The #3791 pins: a signature-typed typedef must not register in
//! [`Flow::type_aliases`].
//!
//! `typedef F = int Function(String);` spells a signature, and the alias
//! table's `(Ty, ArgListId)` face carries no formals — registering it
//! could only bind the signature-less `(Function, EMPTY_ARGS)` fallback
//! (its own `end_function_type` row clears the `top_last_type` latch), so
//! every use site would substitute `x is F` into `x is Function`, promote
//! `x`, and kill the always-true `x is int Function(String)` the
//! official's resolved alias answers (dartq-semantic's
//! `ticket_2410_tests::function_shaped_rhs_resolves_at_the_chase` oracle
//! rows). With the alias absent, the use site keeps the name-level face
//! (`F`) and the semantic layer's alias chase folds the signature.
//!
//! The control faces prove the table is still fed by the shapes that CAN
//! round-trip: a bare named RHS substitutes its aliased base (`int`), a
//! record RHS substitutes its opaque face (`Unknown`), and a
//! parameterized RHS keeps its name-level base but rides the right-hand
//! `?` (`L?` — the nullable ride only happens through the table arm). A
//! signature fixture left in the table would render `Function` instead
//! of `F`; a control left out would render its own name instead of the
//! substituted face.

#[cfg(test)]
mod tests {
    use crate::facts::FlowFact;
    use crate::forward::SharedFlow;
    use crate::record::Flow;
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_parser::ParserFeatures;
    use dartq_scanner::{ScanConfig, TokenTable};

    /// Parses with the recorder attached and returns the live recorder
    /// (for name rendering) alongside the taken facts.
    fn analyze(source: &str) -> (SharedFlow<'_>, Vec<FlowFact>) {
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
        let facts = flow.0.borrow_mut().take_facts();
        (flow, facts)
    }

    /// A type's rendered face: the head's lexeme (the opaque head renders
    /// `Unknown`), with `?` when nullable.
    fn face(ty: crate::ty::Ty, flow: &Flow<'_>) -> String {
        match ty.base {
            crate::ty::Base::Name(id) => {
                let mut name = flow.name_lexeme(id).to_string();
                if ty.nullable {
                    name.push('?');
                }
                name
            }
            crate::ty::Base::Dynamic => "dynamic".to_string(),
            crate::ty::Base::Void => "void".to_string(),
            crate::ty::Base::Never => "Never".to_string(),
            crate::ty::Base::Null => "Null".to_string(),
            crate::ty::Base::Function => {
                if ty.nullable {
                    "Function?".to_string()
                } else {
                    "Function".to_string()
                }
            }
            crate::ty::Base::Unknown => {
                if ty.nullable {
                    "Unknown?".to_string()
                } else {
                    "Unknown".to_string()
                }
            }
        }
    }

    /// The declared faces (rendered) of every reachable read, in fact
    /// order.
    fn read_faces(source: &str) -> Vec<String> {
        let (flow, facts) = analyze(source);
        let flow = flow.0.borrow();
        facts
            .into_iter()
            .filter_map(|fact| match fact {
                FlowFact::ReadType { ty, .. } => Some(face(ty, &flow)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn signature_typed_alias_stays_out_of_the_table() {
        // The use-site read keeps the alias's name-level face: the
        // signature-typed typedef registered nothing, so the substitution
        // arm never folds it to the signature-less `Function` face.
        let signature = "typedef F = int Function(String);\n\
                         void g() {\n\
                         \x20 F x;\n\
                         \x20 x.foo();\n\
                         }\n";
        assert_eq!(read_faces(signature), ["F".to_string()]);

        // Control — bare named RHS: the alias substitutes its aliased
        // base (`int`, not `A`).
        let named = "typedef A = int;\n\
                     void g() {\n\
                     \x20 A a;\n\
                     \x20 a.foo();\n\
                     }\n";
        assert_eq!(read_faces(named), ["int".to_string()]);

        // Control — record RHS: the alias substitutes the record's
        // opaque face (`Unknown`, not `R`).
        let record = "typedef R = (int, String);\n\
                      void g() {\n\
                      \x20 R r;\n\
                      \x20 r.foo();\n\
                      }\n";
        assert_eq!(read_faces(record), ["Unknown".to_string()]);

        // Control — parameterized RHS: the alias keeps its name-level
        // base but the right-hand `?` rides it (`L?` — the nullable ride
        // only happens through the table's parameterized arm).
        let parameterized = "typedef L = List<int>?;\n\
                             void g() {\n\
                             \x20 L l;\n\
                             \x20 l.foo();\n\
                             }\n";
        assert_eq!(read_faces(parameterized), ["L?".to_string()]);
    }
}
