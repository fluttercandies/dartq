//! The #2460 flow-side pins: the generator-body promotion face the
//! semantic consumer reads. The official `suspension`
//! (upstream Dart SDK source,
//! pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart:7909-
//! 7950) demotes only when the suspension point sits inside a local
//! function — `_enclosingFunctionExpressionInfoStack` must be non-empty —
//! so a `sync*`/`async*` body's own `yield` carries the standing
//! promotions across, and the read inside `yield x` after `x ??= C()`
//! answers the promoted non-null type.

use crate::facts::FlowFact;

use crate::forward::SharedFlow;
use crate::record::Flow;
use crate::ty::{Base, Ty};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// Parses with the recorder attached and returns the promoted-read rows
/// `(offset, ty, args)` (the `tests.rs` harness shape).
fn promoted_read_rows(
    source: &str,
) -> (
    std::rc::Rc<std::cell::RefCell<Flow<'_>>>,
    Vec<(u32, Ty, u32)>,
) {
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
    let rows = facts
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::PromotedRead {
                span, ty, args, ..
            } => Some((span.range.offset, ty, args)),
            _ => None,
        })
        .collect();
    (flow.0, rows)
}

fn foo_non_null(flow: &std::rc::Rc<std::cell::RefCell<Flow<'_>>>) -> Ty {
    let foo = flow.borrow_mut().intern_name("Foo");
    Ty {
        base: Base::Name(foo),
        nullable: false,
    }
}

/// `sync*` body: `x ??= Foo(); yield x;` — the yield's read carries the
/// promoted non-null `Foo` (the suspension demotes nothing at the
/// generator body's own top level), which is the flow face the
/// oracle-exact silence rides.
#[test]
fn sync_generator_body_keeps_promotion_across_yield() {
    let source = "class Foo {}\nvoid f(Foo? x) sync* {\n  x ??= Foo();\n  yield x;\n}";
    let (flow, rows) = promoted_read_rows(source);
    let yield_read = source.rfind('x').unwrap() as u32;
    assert_eq!(
        rows,
        vec![(yield_read, foo_non_null(&flow), crate::ty::EMPTY_ARGS)],
        "{rows:?}"
    );
}

/// The `async*` twin: the promotion stands on the way to the yield — the
/// generator body's top level is not a closure, so no suspension
/// demotion applies.
#[test]
fn async_generator_body_keeps_promotion_across_yield() {
    let source = "class Foo {}\nvoid f(Foo? x) async* {\n  x ??= Foo();\n  yield x;\n}";
    let (flow, rows) = promoted_read_rows(source);
    let yield_read = source.rfind('x').unwrap() as u32;
    assert_eq!(
        rows,
        vec![(yield_read, foo_non_null(&flow), crate::ty::EMPTY_ARGS)],
        "{rows:?}"
    );
}

/// The guard form: `if (x != null) { yield x; }` — the type-test
/// promotion stands inside the branch and across the yield.
#[test]
fn sync_generator_guard_promotion_survives_yield() {
    let source =
        "class Foo {}\nvoid f(Foo? x) sync* {\n  if (x != null) {\n    yield x;\n  }\n}";
    let (flow, rows) = promoted_read_rows(source);
    let yield_read = source.rfind('x').unwrap() as u32;
    assert_eq!(
        rows,
        vec![(yield_read, foo_non_null(&flow), crate::ty::EMPTY_ARGS)],
        "{rows:?}"
    );
}

/// The plain-function control: the same `??=` write promotes outside a
/// generator — isolating the generator entry from the write edge.
#[test]
fn plain_function_ifnull_write_promotes() {
    let source = "class Foo {}\nvoid f(Foo? x) {\n  x ??= Foo();\n  x;\n}";
    let (flow, rows) = promoted_read_rows(source);
    let read = source.rfind('x').unwrap() as u32;
    assert_eq!(
        rows,
        vec![(read, foo_non_null(&flow), crate::ty::EMPTY_ARGS)],
        "{rows:?}"
    );
}
