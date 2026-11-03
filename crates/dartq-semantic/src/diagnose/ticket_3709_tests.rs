//! #3709 pins — `FlowFact::WriteSite` rides the fold product
//! (`UnitFold::write_sites`) instead of being dropped at the fact match.
//!
//! The flow layer aggregates the official `AssignedVariables` anywhere sets
//! into one `(declaration name span, in_closure)` row per (declaration,
//! in-closure) pair (upstream Dart SDK source,
//! pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart —
//! `functionExpression_begin` joins `anywhere.written`/`anywhere.captured`
//! into the closure's entry state, so a `final` local written only from
//! inside a nested function is NOT definitely unassigned there and the
//! write reports `assignment_to_final_local`). The assignment-to-final face
//! today approximates that verdict with a token probe (`captured_write`,
//! #3697); these pins prove the exact fact carries what that probe needs.
//!
//! Contract pinned (the consumer's binary-search premise):
//!
//! * a write from inside a nested function — a local function declaration
//!   or a function expression — rows `in_closure: true`;
//! * a same-scope write rows `in_closure: false`;
//! * `decl_name_span` is the DECLARATION's name token, never a write's;
//! * a declaration written both ways rows twice, `(offset, false)` ahead of
//!   `(offset, true)` — the explicit sort key, so `(decl, true)` is a direct
//!   `binary_search_by_key` probe (the follow-up MUST key per declaration,
//!   not per write: the rows carry no write positions, so a same-scope
//!   write to a variable that ALSO has a closure write must still consult
//!   the flow's own unassigned verdict, not the closure row);
//! * an initializer is not a write — an initialized, never-reassigned
//!   `final` has no row at all.

use std::path::Path;

use dartq_language::{Feature, FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder, Namespaces,
};
use dartq_scanner::{ScanConfig, scan};
use dartq_source::{FileTable, SourceRange, StringTable};

use crate::collector::collect;
use crate::diagnose::{StrictModes, UnitFold, fold_unit};
use crate::library::ElementIndex;
use crate::model::MicroKind;

/// The names-only core face the fixtures type against (`int`).
const CORE_FACE: &[&str] = &["int"];

/// Runs `source` through the diagnosing fold (the shared rig keeps only
/// the report; the write-site table is a fold-product field, so the fold
/// is built here the way the rig builds it) and returns the whole
/// [`UnitFold`].
fn fold_of(source: &str, strings: &StringTable) -> UnitFold {
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let scanned = scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: set.is_enabled(Feature::Augmentations),
        },
        file,
    );
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {scanned:?}"
    );
    let features = ParserFeatures::from_feature_set(set);
    let elements = collect(source, &scanned.tokens, features, strings);
    let mut builder = LibraryGraphBuilder::new(strings);
    let lib = builder.intern("package:t/main.dart");
    let core_file = files.intern(Path::new("<dart:core>"));
    let core = builder.intern("dart:core");
    for name in CORE_FACE {
        builder.declare(core, strings.intern(name), core_file);
    }
    builder.import(
        lib,
        ImportEdge {
            target: Some(core),
            prefix: None,
            deferred: false,
            non_library: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    builder.unit(lib, file);
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    fold_unit(
        source,
        &scanned.tokens,
        features,
        strings,
        file,
        lib,
        &graph,
        &namespaces,
        &index,
        StrictModes::default(),
        &LibraryClassModifiers::default(),
        set.is_enabled(Feature::InferenceUpdate2),
    )
}

/// The fold's write-site table for `source`.
fn write_sites(source: &str) -> Vec<(SourceRange, bool)> {
    let strings = StringTable::new();
    fold_of(source, &strings).write_sites
}

/// The declaration name's span: the `x` of the fixture's `final int x`.
/// The fixtures are ASCII, so byte offsets are UTF-16 offsets.
fn decl_x(source: &str) -> SourceRange {
    let at = source
        .find("final int x")
        .expect("fixture declares `final int x`")
        + "final int ".len();
    SourceRange {
        offset: u32::try_from(at).expect("fixture offset fits u32"),
        length: 1,
    }
}

/// A write to an enclosing `final` from inside a nested local FUNCTION
/// DECLARATION rows `in_closure: true` at the declaration's name — the
/// shape #3697's `brace_opens_function` token probe classifies by the
/// signature `)` ahead of the body's `{`.
#[test]
fn write_inside_local_function_rows_in_closure() {
    let source = "void f() {\n\
         \x20 final int x;\n\
         \x20 void g() {\n\
         \x20   x = 1;\n\
         \x20 }\n\
         }\n";
    assert_eq!(write_sites(source), vec![(decl_x(source), true)]);
}

/// The same verdict for a FUNCTION EXPRESSION body (the flow crate's own
/// pinned shape): the anchor is still the declaration's `x`, not the
/// write's.
#[test]
fn write_inside_function_expression_rows_in_closure() {
    let source = "void f() {\n\
         \x20 final int x;\n\
         \x20 () {\n\
         \x20   x = 1;\n\
         \x20 };\n\
         }\n";
    assert_eq!(write_sites(source), vec![(decl_x(source), true)]);
}

/// A same-scope write rows `in_closure: false` at the declaration's name.
#[test]
fn same_scope_write_rows_direct() {
    let source = "void f() {\n\
         \x20 final int x;\n\
         \x20 x = 1;\n\
         }\n";
    assert_eq!(write_sites(source), vec![(decl_x(source), false)]);
}

/// A declaration written both ways rows twice, direct ahead of captured —
/// the `(offset, in_closure)` sort key the consumer's `(decl, true)` probe
/// binary-searches on. Both rows anchor at the one declaration.
#[test]
fn write_both_ways_rows_direct_then_captured() {
    let source = "void f() {\n\
         \x20 final int x;\n\
         \x20 x = 1;\n\
         \x20 void g() {\n\
         \x20   x = 2;\n\
         \x20 }\n\
         }\n";
    let x = decl_x(source);
    assert_eq!(write_sites(source), vec![(x, false), (x, true)]);
}

/// An initializer is not a write: an initialized `final` that is never
/// reassigned has no row (the official sets do not count initializers),
/// so a consumer keyed on the table never mistakes `final int x = 0;` for
/// a written variable.
#[test]
fn initializer_only_rows_nothing() {
    let source = "void f() {\n\
         \x20 final int x = 0;\n\
         \x20 x;\n\
         }\n";
    assert_eq!(write_sites(source), vec![]);
}
