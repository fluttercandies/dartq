//! #3853 pins: the FLOW face of a record-typed receiver — what the flow
//! layer publishes at a null-checked record read, and why the record
//! never arrives at the member face as a record.
//!
//! # The finding these pins freeze
//!
//! A record type has no base in the flow lattice: `(int, String)` and
//! `typedef R1 = (int, String)?` both stage the opaque
//! `Ty { base: Unknown, nullable: <written ?> }`
//! (`record.rs`'s record-type spelling arm, and its top-level latch that
//! feeds `end_typedef`, so the alias's right-hand `?` DOES reach the
//! declared face). Probe #3853 (env-gated `eprintln` at the promotion
//! re-derivation site, `interp_expr.rs`'s `promoted_type_of` call, since
//! removed) showed the null check then behaves exactly as it should:
//!
//! ```text
//! read@51 pre=Ty { base: Unknown, nullable: true }  post=Ty { base: Unknown, nullable: true }
//! read@69 pre=Ty { base: Unknown, nullable: true }  post=Ty { base: Unknown, nullable: false }
//! read@80 pre=Ty { base: Unknown, nullable: true }  post=Ty { base: Unknown, nullable: false }
//! ```
//!
//! i.e. `r1 != null` promotes `Unknown?` to `Unknown` and emits
//! `FlowFact::PromotedRead` at both in-branch reads. The promotion is
//! NOT the missing link; the missing link is that the promoted type is
//! the opaque `Unknown`, never a record: `staged_promotion_typed` maps
//! `Base::Unknown` to `Ty::UNKNOWN`, so the only channel that still
//! carries the record shape is the semantic DECLARED type (a written
//! `Ty::Record`, or the alias fold of `R1` to the same). Hence the
//! member face's fix belongs to dartq-semantic, not here (see the
//! module doc of the ticket's report).
//!
//! An `is`-test to a record type is worse off by one step: the tested
//! spelling is the same opaque `Unknown`, so `try_promote_to(Unknown,
//! Unknown)` is no step and NO promotion fact at all is staged
//! (`(Object,) r3; if (r3 is (int,)) r3.$1` keeps the declared face, and
//! `$1` reads `Object` instead of the refined `int`). Closing that shape
//! needs a record base in this lattice, not a fix in the member face.

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
    /// `Unknown`) with `?` when nullable.
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

    /// Every read row as `(identifier text, flow face)` in fact order, plus
    /// the promoted rows alone. Offsets index the ASCII fixtures byte-wise,
    /// which is also their UTF-16 offset.
    fn trace(source: &str) -> (Vec<(String, String)>, Vec<(String, String)>) {
        let (flow, facts) = analyze(source);
        let flow = flow.0.borrow();
        let text = |span: dartq_parser::TokenSpan| {
            source[span.range.offset as usize..span.range.end() as usize].to_string()
        };
        let reads = facts
            .iter()
            .filter_map(|fact| match fact {
                FlowFact::ReadType { span, ty, .. } => Some((text(*span), face(*ty, &flow))),
                _ => None,
            })
            .collect();
        let promoted = facts
            .iter()
            .filter_map(|fact| match fact {
                FlowFact::PromotedRead { span, ty, .. } => Some((text(*span), face(*ty, &flow))),
                _ => None,
            })
            .collect();
        (reads, promoted)
    }

    fn pairs(rows: &[(&str, &str)]) -> Vec<(String, String)> {
        rows.iter()
            .map(|(name, face)| ((*name).to_string(), (*face).to_string()))
            .collect()
    }

    /// The typedef'd nullable record of co19 `Records/type_promotion_A01_t01`
    /// (`R1 r1; if (r1 != null) { r1.$1; r1.$2; }`): the alias's right-hand
    /// `?` reaches the declared face, the null check promotes the read to
    /// the non-null opaque face, and both in-branch reads carry a
    /// promotion fact. The else-branch `r1?.$1` read keeps the nullable
    /// declared face (the null-aware access is not a promotion).
    #[test]
    fn ticket_3853_typedef_record_null_check_stages_the_promotion() {
        let source = "typedef R1 = (int, String)?;\n\
                      void f(R1 r1) {\n\
                      \x20 if (r1 != null) {\n\
                      \x20   r1.$1;\n\
                      \x20   r1.$2;\n\
                      \x20 } else {\n\
                      \x20   r1?.$1;\n\
                      \x20 }\n\
                      }\n";
        let (reads, promoted) = trace(source);
        assert_eq!(
            reads,
            pairs(&[
                ("r1", "Unknown?"),
                ("r1", "Unknown"),
                ("r1", "Unknown"),
                ("r1", "Unknown?"),
            ])
        );
        assert_eq!(
            promoted,
            pairs(&[("r1", "Unknown"), ("r1", "Unknown")]),
            "the null check promotes the opaque record face to its non-null form"
        );
    }

    /// The written nullable record of the same co19 case (`(String,)? r4`
    /// written in a branch, then `if (r4 != null) r4.$1`): the identical
    /// shape — declared `Unknown?`, promoted `Unknown` with a promotion
    /// fact — so the typedef spelling is not why the member face misses.
    #[test]
    fn ticket_3853_written_record_null_check_stages_the_promotion() {
        let source = "void f() {\n\
                      \x20 (String,)? r4 = null;\n\
                      \x20 if (2 > 1) {\n\
                      \x20   r4 = (\"Lily was here\",);\n\
                      \x20 }\n\
                      \x20 if (r4 != null) {\n\
                      \x20   r4.$1;\n\
                      \x20 }\n\
                      }\n";
        let (reads, promoted) = trace(source);
        assert_eq!(
            reads,
            pairs(&[("r4", "Unknown?"), ("r4", "Unknown")])
        );
        assert_eq!(promoted, pairs(&[("r4", "Unknown")]));
    }

    /// The `is`-test refinement to a record type (co19 `r3`/`r5`): the
    /// tested spelling is the same opaque face, so the test is no
    /// promotion step and NEITHER read inside the body gets a promotion
    /// fact — the receiver keeps its declared face (`(Object,)` decays to
    /// the opaque `Unknown`, the bare `Record` head stays a name), which
    /// is why `$1` reads `Object`/`$1` misses on `Record` downstream.
    #[test]
    fn ticket_3853_is_test_to_record_shape_stages_nothing() {
        let source = "void f() {\n\
                      \x20 (Object,) r3 = (42,);\n\
                      \x20 if (r3 is (int,)) {\n\
                      \x20   r3.$1.isOdd;\n\
                      \x20 }\n\
                      \x20 Record r5 = (1, 2);\n\
                      \x20 if (r5 is (int, int)) {\n\
                      \x20   r5.$1.isOdd;\n\
                      \x20 }\n\
                      }\n";
        let (reads, promoted) = trace(source);
        assert_eq!(
            reads,
            pairs(&[
                ("r3", "Unknown"),
                ("r3", "Unknown"),
                ("r5", "Record"),
                ("r5", "Record"),
            ])
        );
        assert!(
            promoted.is_empty(),
            "an is-test against an opaque record spelling is no promotion step: {promoted:?}"
        );
    }

    /// The silence-critical twin: the same typedef WITHOUT a null check
    /// keeps the nullable declared face and stages no promotion, so the
    /// member face's `unchecked_use_of_nullable_value` verdict (the
    /// oracle's own row for this shape) still lands. A fix that mouths
    /// this read as non-null would over-suppress the documented family.
    #[test]
    fn ticket_3853_unchecked_record_read_keeps_the_nullable_face() {
        let source = "typedef R1 = (int, String)?;\n\
                      void f(R1? r1) {\n\
                      \x20 r1.$1;\n\
                      }\n";
        let (reads, promoted) = trace(source);
        assert_eq!(reads, pairs(&[("r1", "Unknown?")]));
        assert!(
            promoted.is_empty(),
            "no null check stands, so no promotion may stage: {promoted:?}"
        );
    }
}
