//! The #3652 pins: post-pattern promotion faces, mined from the 2026-11-02
//! census's extra `type_argument_not_matching_bounds` rows. Those rows ride
//! the corpus's `expectStaticType<Exactly<X>>()` family (upstream Dart SDK
//! source, pkg/expect/lib/static_type_helper.dart — `T expectStaticType<R
//! extends Exactly<T>>()` with `typedef Exactly<T> = T Function(T)`, so the
//! bound check fires exactly when the receiver's static type differs from
//! X). For every mirrored case `conformance/expected.tsv` carries no
//! `type_argument_not_matching_bounds` row: the oracle reads each receiver
//! at the pattern's promoted face. These pins hold the flow layer to that
//! face — each asserts the scrutinee's own read (the declared type) and the
//! matched arm's body read (the promoted type), in fact order.

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

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

/// Every read named `needle`, as `(head name, [argument head names])` in
/// fact order. Non-name heads render empty so an untyped/unknown read is
/// distinguishable from any named face; the nullability marker rides the
/// name (`int?`) so a nullable declared face is distinguishable from its
/// promoted non-null form.
fn read_faces(source: &str, needle: &str) -> Vec<(String, Vec<String>)> {
    let (flow, facts) = analyze(source);
    let flow = flow.0.borrow();
    let head_name = |base: crate::ty::Base, nullable: bool, flow: &Flow<'_>| -> String {
        match base {
            crate::ty::Base::Name(id) => {
                let mut name = flow.name_lexeme(id).to_string();
                if nullable {
                    name.push('?');
                }
                name
            }
            crate::ty::Base::Dynamic => "dynamic".to_string(),
            _ => String::new(),
        }
    };
    let arg_names = |args: u32, flow: &Flow<'_>| -> Vec<String> {
        flow.type_args_at(args)
            .iter()
            .map(|arg| head_name(arg.base, arg.nullable, flow))
            .collect()
    };
    facts
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::ReadType { span, ty, args } => {
                let snippet = &source
                    [span.range.offset as usize..(span.range.offset + span.range.length) as usize];
                if snippet != needle {
                    return None;
                }
                let name = head_name(ty.base, ty.nullable, &flow);
                Some((name, arg_names(args, &flow)))
            }
            _ => None,
        })
        .collect()
}

/// The census rep (language/patterns/object_pattern_inference_test.dart,
/// `simpleInference`; its `conformance/expected.tsv` row is empty): a
/// FIELD-BEARING generic object head promotes the switched variable for
/// the case body (official `analyzeObjectPattern` —
/// `downwardInferObjectPatternRequiredType` then
/// `flow.promoteForPattern(matchedType, knownType: requiredType)`, shared
/// type_analyzer.dart:1504-1512). The field sub-pattern's binder runs
/// before the head event (post-order) and must not disturb the head's
/// promotion. Under the name-level back-end the required type is the raw
/// head — `NameTypeOps::downcast_infer` solves nothing without a hierarchy;
/// the solved `C<num>` is the resolved back-end's `downcast_infer` face —
/// so the body read spells `C` with no written arguments.
#[test]
fn field_bearing_object_head_promotes_the_switch_scrutinee() {
    let source = "class B<T> { B(); }\n\
                  class C<T> extends B<T> { final T t; C(this.t); }\n\
                  void f(B<num> b) {\n\
                  \x20 switch (b) {\n\
                  \x20   case C(t: var x):\n\
                  \x20     b;\n\
                  \x20 }\n\
                  }";
    assert_eq!(
        read_faces(source, "b"),
        [
            ("B".to_string(), vec!["num".to_string()]),
            ("C".to_string(), Vec::new()),
        ],
        "{source:?}"
    );
}

/// The same face over a top-type scrutinee (object_pattern_inference_test's
/// `inferBound`, `case D(...)` over `Object o`; empty expected row): the
/// promotion lands on the head. The to-bounds argument the oracle spells
/// (`D<num>`) is again the resolved back-end's inference.
#[test]
fn raw_object_head_over_top_scrutinee_promotes_to_the_head() {
    let source = "class D<T extends num> { final T t; D(this.t); }\n\
                  void f(Object o) {\n\
                  \x20 switch (o) {\n\
                  \x20   case D(t: var x):\n\
                  \x20     o;\n\
                  \x20 }\n\
                  }";
    assert_eq!(
        read_faces(source, "o"),
        [
            ("Object".to_string(), Vec::new()),
            ("D".to_string(), Vec::new())
        ],
        "{source:?}"
    );
}

/// A top-level typed variable pattern promotes the scrutinee (official
/// `analyzeDeclaredVariablePattern` — `flow.promoteForPattern(matchedType,
/// knownType: staticType)` promotes the matched value's reference, shared
/// type_analyzer.dart:605-609). Mirrors
/// flow_analysis/variable_pattern_error_test.dart block 4 (`if (x case int
/// y)` on `num x`), whose expected rows carry no bounds row: the matched
/// arm reads `int`.
#[test]
fn typed_variable_pattern_promotes_the_scrutinee() {
    let source = "void f(num x) {\n\
                  \x20 if (x case int y) {\n\
                  \x20   x;\n\
                  \x20 }\n\
                  }";
    assert_eq!(
        read_faces(source, "x"),
        [
            ("num".to_string(), Vec::new()),
            ("int".to_string(), Vec::new())
        ],
        "{source:?}"
    );
}

/// The logical-and's LEFT operand promotes the scrutinee through the whole
/// head (official `analyzeLogicalAndPattern` dispatches both operands over
/// the same matched value, and `promoteForPattern`'s promotion persists —
/// shared type_analyzer.dart:1112-1133). Mirrors
/// flow_analysis/logical_and_pattern_error_test.dart block 1 (`int _ &&
/// var y` on `num x`), whose expected rows carry no bounds row.
#[test]
fn typed_wildcard_operand_promotes_the_scrutinee() {
    let source = "void f(num x) {\n\
                  \x20 if (x case int _ && var y) {\n\
                  \x20   x;\n\
                  \x20 }\n\
                  }";
    assert_eq!(
        read_faces(source, "x"),
        [
            ("num".to_string(), Vec::new()),
            ("int".to_string(), Vec::new())
        ],
        "{source:?}"
    );
}

/// OR-alternation branch promotions join to their common face (official
/// `analyzeLogicalOrPattern` joins the branches' promotion results —
/// shared type_analyzer.dart:1156-1180): the left branch promotes to `num`
/// then `int`, the right to `num`, and the body reads `num`. Mirrors
/// flow_analysis/logical_or_pattern_error_test.dart block 1, whose expected
/// rows carry no bounds row.
#[test]
fn or_alternation_joins_branch_promotions() {
    let source = "void f(Object x) {\n\
                  \x20 if (x case (num() && int()) || num()) {\n\
                  \x20   x;\n\
                  \x20 }\n\
                  }";
    assert_eq!(
        read_faces(source, "x"),
        [
            ("Object".to_string(), Vec::new()),
            ("num".to_string(), Vec::new())
        ],
        "{source:?}"
    );
}
