//! #3902 pins — the flow-crate half of the ticket: the type-parameter
//! promotion's nullability leg (M16b) and the unary-prefix dead anchor.
//!
//! # M16b: `T & C?` keeps the nullable leg
//!
//! The oracle promotes `t` to `T & C?` after `if (t is C?)` on
//! `void f<T>(T t)` — the intersection keeps C?'s nullability, so the
//! promoted read is still potentially null (its members need `?`/`!`, and
//! an implicit tear-off off it is refused). Upstream builds the promoted
//! `TypeParameterTypeImpl` with `_promotedTypeParameterTypeNullability`
//! (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/element/type_system.dart:2104-2121), which
//! answers `question` only when BOTH the reference and the tested bound
//! are nullable — and a `?`-less `T` over an implicit `Object?` bound IS
//! nullable (its bound admits null). dartq's lattice spelled the head as
//! the tested type with the variable's WRITTEN flag deciding the leg, so
//! `T` (written without `?`) dropped it and the promoted read came out
//! non-null `C` — every downstream row of the family vanished
//! (`implicit_tearoff_type_variable_error_test` 29:30 and the two
//! `unchecked_use_of_nullable_value` rows its probe fixture carries).
//!
//! The fix routes the variable's effective nullability through the
//! boundary-aware [`crate::interp::Interp::classify`]; these pins freeze
//! both directions: a nullable bound keeps the `?`, a non-nullable bound
//! still narrows to the non-null face.
//!
//! # The unary-prefix dead anchor
//!
//! `x ??= -42` on a definitely-non-null `x` rows `-42;` in the oracle: the
//! `??=` RHS is the first dead NODE, so its begin token is the `-` of the
//! negative literal (`dead_code_verifier.dart` anchors at
//! `_firstDeadNode.beginToken`). The recorder's `Ev::UnaryPrefix` carried
//! no span, so the operand latch (`Interp::step` reading `Ev::span()`)
//! missed the sign and anchored on the numeric literal instead — a
//! one-column-short row for every `??= <negative literal>` site
//! (`int-to-double/assignment_local_variable_t01/t02`, 8 census rows).

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
        analyze_with(source, None)
    }

    /// [`analyze`] with an optional mounted back-end. `intern` names are
    /// interned on the recorder's own table before parsing (the ops'
    /// seed), and the ops sees them through the ids returned alongside.
    fn analyze_with<'t>(
        source: &'t str,
        ops: Option<&'t ResolvedShapeOps>,
    ) -> (SharedFlow<'t>, Vec<FlowFact>) {
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
        if let Some(ops) = ops {
            let mut recorder = flow.0.borrow_mut();
            let object = recorder.intern_name("Object");
            recorder.set_ops(ops);
            ops.seed(object);
        }
        let mut parser = dartq_parser::Parser::new(source, tokens, features, &mut flow);
        parser.parse_unit();
        drop(parser);
        let facts = flow.0.borrow_mut().take_facts();
        (flow, facts)
    }

    /// The resolved back-end's shape for these pins: `Object` is the top
    /// every named type subtypes (so `C? <: Object?` answers true, the
    /// name-level lattice's one blind spot here), and the promotion gate
    /// is the official strict one (`to <: from && !(from <: to)`) — the
    /// posture under which the type-parameter arm is the one that decides,
    /// exactly as in the mounted pipeline. Everything else delegates to
    /// the name-level ops.
    struct ResolvedShapeOps {
        object: std::cell::Cell<Option<u32>>,
    }

    impl ResolvedShapeOps {
        fn new() -> Self {
            Self {
                object: std::cell::Cell::new(None),
            }
        }

        fn seed(&self, object: u32) {
            self.object.set(Some(object));
        }

        fn is_top(&self, ty: crate::ty::Ty) -> bool {
            matches!(ty.base, crate::ty::Base::Name(id) if self.object.get() == Some(id))
        }
    }

    impl crate::ty::TypeOps for ResolvedShapeOps {
        fn is_resolved_seam(&self) -> bool {
            true
        }

        fn is_subtype(&self, left: crate::ty::Ty, right: crate::ty::Ty) -> bool {
            if self.is_top(right) {
                return !left.nullable || right.nullable;
            }
            crate::ty::NameTypeOps.is_subtype(left, right)
        }

        fn factor(&self, from: crate::ty::Ty, what: crate::ty::Ty) -> crate::ty::Ty {
            crate::ty::NameTypeOps.factor(from, what)
        }

        fn try_promote_to(&self, to: crate::ty::Ty, from: crate::ty::Ty) -> Option<crate::ty::Ty> {
            (self.is_subtype(to, from) && !self.is_subtype(from, to)).then_some(to)
        }

        fn try_promote_to_typed(
            &self,
            to: crate::ty::Ty,
            to_args: u32,
            from: crate::ty::Ty,
            from_args: u32,
            arena: &[Vec<crate::ty::ArgTy>],
            names: &[Box<str>],
        ) -> Option<crate::ty::Ty> {
            // The mounted pipeline's strict gate (`isValidPromotionStep`):
            // no optimistic cross-base arm, so a type-variable operand
            // falls through to the boundary-aware arm
            // (`Interp::type_parameter_promotion`).
            let strict = self.is_subtype_typed(to, to_args, from, from_args, arena, names)
                && !self.is_subtype_typed(from, from_args, to, to_args, arena, names);
            strict.then_some(to)
        }

        fn promote_to_non_null(&self, t: crate::ty::Ty) -> crate::ty::Ty {
            crate::ty::NameTypeOps.promote_to_non_null(t)
        }

        fn classify(&self, t: crate::ty::Ty) -> crate::ty::Classification {
            crate::ty::NameTypeOps.classify(t)
        }

        fn is_bottom(&self, t: crate::ty::Ty) -> bool {
            crate::ty::NameTypeOps.is_bottom(t)
        }
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

    /// Every promoted read as `(identifier text, flow face)` in fact order,
    /// replayed over the resolved-shape back-end.
    fn promoted(source: &str) -> Vec<(String, String)> {
        let ops = ResolvedShapeOps::new();
        let (flow, facts) = analyze_with(source, Some(&ops));
        let flow = flow.0.borrow();
        facts
            .iter()
            .filter_map(|fact| match fact {
                FlowFact::PromotedRead { span, ty, .. } => Some((
                    source[span.range.offset as usize..span.range.end() as usize].to_string(),
                    face(*ty, &flow),
                )),
                _ => None,
            })
            .collect()
    }

    /// Every dead-code row's source text, in fact order.
    fn dead(source: &str) -> Vec<String> {
        let (_flow, facts) = analyze(source);
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

    fn pairs(rows: &[(&str, &str)]) -> Vec<(String, String)> {
        rows.iter()
            .map(|(name, face)| ((*name).to_string(), (*face).to_string()))
            .collect()
    }

    /// M16b, positive: `void f<T>(T t)` (implicit `Object?` bound) tested
    /// with `is C?` promotes to `T & C?` — the promoted read keeps the
    /// nullable leg. Oracle: `context<void Function()>(t)` inside the
    /// branch reports `The argument type 'T & C?' can't be assigned to the
    /// parameter type 'void Function()'` and `t.call()` two
    /// `UNCHECKED_USE_OF_NULLABLE_VALUE` rows.
    #[test]
    fn ticket_3902_nullable_bound_test_keeps_the_nullability_leg() {
        let source = "class C {\n\
                      \x20 void call() {}\n\
                      }\n\
                      \n\
                      void f<T>(T t) {\n\
                      \x20 if (t is C?) {\n\
                      \x20   t;\n\
                      \x20 }\n\
                      }\n";
        assert_eq!(
            promoted(source),
            pairs(&[("t", "C?")]),
            "T & C? keeps the tested type's nullable leg"
        );
    }

    /// M16b, negative guard: a non-nullable bound (`T extends C`) already
    /// satisfies `is C?` (`C? <: T` fails, `C?` is not under the non-null
    /// bound), so no intersection stands at all — the oracle reports only
    /// `unnecessary_type_check` and keeps `T` (non-null), and the fix must
    /// not mint a `C?` read here.
    #[test]
    fn ticket_3902_non_nullable_bound_test_promotes_nothing() {
        let source = "class C {\n\
                      \x20 void call() {}\n\
                      }\n\
                      \n\
                      void f<T extends C>(T t) {\n\
                      \x20 if (t is C?) {\n\
                      \x20   t;\n\
                      \x20 }\n\
                      }\n";
        assert!(
            promoted(source).is_empty(),
            "a tested type outside the bound is no intersection: {:?}",
            promoted(source)
        );
    }

    /// The `??=`-on-non-null face anchors the dead row at the RHS
    /// expression's FIRST token — the `-` of the negative literal, never
    /// the numeric literal alone (oracle: `DEAD_CODE 5:9|4` = `-42;`).
    #[test]
    fn ticket_3902_if_null_assign_negative_literal_anchors_at_the_sign() {
        let source = "void f(int? x) {\n\
                      \x20 if (x == null) {\n\
                      \x20   x = 1;\n\
                      \x20 }\n\
                      \x20 x ??= -42;\n\
                      }\n";
        assert_eq!(dead(source), vec!["-42;".to_string()]);
    }

    /// The twin without a sign: the anchor is the operand's own first
    /// token, so nothing about the unary-span fix may shift a plain
    /// literal's row (`x ??= 42` -> `42;`).
    #[test]
    fn ticket_3902_if_null_assign_plain_literal_anchor_is_unchanged() {
        let source = "void f(int? x) {\n\
                      \x20 if (x == null) {\n\
                      \x20   x = 1;\n\
                      \x20 }\n\
                      \x20 x ??= 42;\n\
                      }\n";
        assert_eq!(dead(source), vec!["42;".to_string()]);
    }
}
