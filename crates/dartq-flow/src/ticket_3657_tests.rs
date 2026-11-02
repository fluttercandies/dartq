//! #3657 — two flow-layer definite-assignment faces behind the
//! `not_assigned_potentially_non_nullable_local_variable` family:
//!
//! * A declarator WITH an initializer is declared assigned before its
//!   initializer runs: the official `variableDeclarationList` declares
//!   every declarator of the list with `initialized:
//!   variable.initializer != null` before any initializer is visited
//!   (upstream Dart SDK source,
//!   `pkg/analyzer/lib/src/dart/resolver/flow_analysis_visitor.dart`). A
//!   read inside the declaration's own initializer therefore reads
//!   assigned; a declarator WITHOUT one still starts unassigned.
//! * A statement `for`'s declared head binds into the loop's own scope,
//!   which closes at the statement's end (upstream
//!   `pkg/analyzer/lib/src/dart/resolver/resolution_visitor.dart`,
//!   `visitForStatement`: `withLocalScope` around the loop parts and the
//!   body). A read after the loop resolves the enclosing declaration of
//!   the same name, never the loop variable.
//!
//! The rows carry the read offset and the `definitely_unassigned` bit: when
//! an outer never-written local and a loop variable share a name, the bit
//! says which of the two a post-loop read resolved to.

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// The unassigned-read rows of `source`: each read's offset with its
/// `definitely_unassigned` bit.
fn unassigned_rows(source: &str) -> Vec<(u32, bool)> {
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
    flow.0
        .borrow_mut()
        .set_sound_flow(features.sound_flow_analysis);
    let mut parser = dartq_parser::Parser::new(source, tokens, features, &mut flow);
    parser.parse_unit();
    drop(parser);
    flow.0
        .borrow_mut()
        .take_facts()
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::UnassignedRead {
                span,
                definitely_unassigned,
                ..
            } => Some((span.range.offset, definitely_unassigned)),
            _ => None,
        })
        .collect()
}

/// The offset of the read `needle` starts with (the needle must occur
/// exactly once in `source`).
fn at(source: &str, needle: &str) -> u32 {
    assert_eq!(source.matches(needle).count(), 1, "{needle:?} in {source:?}");
    u32::try_from(source.find(needle).expect("needle present")).expect("offset fits u32")
}

// —— the declaration's own initializer ————————————————————————————————

/// A read inside the declaration's own initializer reads assigned (the
/// declarator is declared `initialized: true`): no row for the plain,
/// `final`, `late` (definitely-unassigned gate) and multi-declarator faces.
#[test]
fn self_initializer_read_reads_assigned() {
    let src = concat!(
        "void f() {\n",
        "  int x = x;\n",
        "  final int y = y + 1;\n",
        "  late int z = z;\n",
        "  int a = 1, b = a;\n",
        "}\n",
    );
    assert_eq!(unassigned_rows(src), Vec::new(), "{src:?}");
}

/// Negative control: a declarator WITHOUT an initializer still starts
/// unassigned — its read in a later declaration's initializer (a separate
/// list, and the same list's next declarator) reports definitely
/// unassigned.
#[test]
fn uninitialized_declarator_read_still_reports() {
    let src = concat!(
        "void f() {\n",
        "  int x;\n",
        "  int y = x;\n",
        "  int p, q = p;\n",
        "}\n",
    );
    assert_eq!(
        unassigned_rows(src),
        vec![(at(src, "x;\n  int p"), true), (at(src, "p;\n}"), true)],
        "{src:?}"
    );
}

// —— the statement `for` scope ———————————————————————————————————————

/// The #3648 rider shape: the post-loop `i` resolves to the assigned
/// parameter, not the loop's potentially-unassigned `i`.
#[test]
fn post_loop_read_resolves_the_outer_parameter() {
    let src = "void f(List<int> xs, int i) {\n  for (var i in xs) {}\n  i;\n}\n";
    assert_eq!(unassigned_rows(src), Vec::new(), "{src:?}");
}

/// The post-loop read attributes to the OUTER never-written local: the
/// row's definitely-unassigned bit is the outer declaration's (the loop
/// variable would read assigned after a C-style `for`, and only
/// potentially unassigned after a for-in). Covers the for-in, typed
/// for-in, function-typed for-in and C-style heads.
#[test]
fn post_loop_read_attributes_to_the_outer_local() {
    for src in [
        "void f(List<int> xs) {\n  int i;\n  for (var i in xs) {}\n  i;\n}\n",
        "void f(List<int> xs) {\n  int i;\n  for (final int i in xs) {}\n  i;\n}\n",
        "void f(List<void Function(int)> fs) {\n  int i;\n  for (void Function(int) i in fs) {}\n  i;\n}\n",
        "void f() {\n  int i;\n  for (var i = 0; i < 3; i++) {}\n  i;\n}\n",
    ] {
        assert_eq!(unassigned_rows(src), vec![(at(src, "i;\n}"), true)], "{src:?}");
    }
}

/// Inside the loop the loop variable still resolves and reads assigned:
/// the for-in head is written before the body, the C-style head by its
/// initializer — no row in either body.
#[test]
fn loop_variable_reads_inside_the_body_stay_assigned() {
    let src = concat!(
        "void f(List<int> xs) {\n",
        "  for (var i in xs) {\n",
        "    i;\n",
        "  }\n",
        "  for (var j = 0; j < 3; j++) {\n",
        "    j;\n",
        "  }\n",
        "}\n",
    );
    assert_eq!(unassigned_rows(src), Vec::new(), "{src:?}");
}

/// The scope stack stays balanced: nested declared heads close exactly
/// their own scopes, and an identifier head (`for (m in xs)`) declares
/// nothing and closes nothing — the enclosing block's never-written `k`
/// still resolves after all three loops and reports definitely
/// unassigned.
#[test]
fn nested_and_identifier_heads_keep_the_scope_stack_balanced() {
    let src = concat!(
        "void f(List<int> xs) {\n",
        "  int k;\n",
        "  for (var i in xs) {\n",
        "    for (var j in xs) {\n",
        "      i;\n",
        "      j;\n",
        "    }\n",
        "  }\n",
        "  int m = 0;\n",
        "  for (m in xs) {}\n",
        "  k;\n",
        "}\n",
    );
    assert_eq!(unassigned_rows(src), vec![(at(src, "k;\n}"), true)], "{src:?}");
}
