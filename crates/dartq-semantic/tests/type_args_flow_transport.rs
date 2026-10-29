//! End-to-end pins for the written-type-arguments transport (#2125): the
//! listener's `begin_type_arguments` / `end_type_arguments` /
//! `handle_no_type_arguments` forwards must feed the flow recorder exactly
//! what the direct-drive face sees, or every written type-argument list
//! (`List<int>`) latches as `EMPTY_ARGS` on the flow rows of
//! listener-driven runs (the diagnoser's mount). The #1010 transport
//! rides this trio: `end_type_arguments` sets the pending argument list,
//! `handle_type` consumes it, `handle_no_type_arguments` clears it — all
//! three fire immediately before the row (official `parseArguments`,
//! upstream Dart SDK source,
//! pkg/_fe_analyzer_shared/lib/src/parser/type_info_impl.dart:1158/1191).

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use dartq_flow::ty::{Base, EMPTY_ARGS};
use dartq_flow::{Flow, FlowFact, SharedFlow};
use dartq_language::{Feature, FeatureSet, LanguageVersion};
use dartq_parser::{ParserFeatures, parse_unit_with};
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder, Namespaces};
use dartq_scanner::{ScanConfig, scan};
use dartq_semantic::MicroKind;
use dartq_semantic::collector::collect;
use dartq_semantic::diagnose::{StrictModes, UnitFold, fold_unit};
use dartq_semantic::library::ElementIndex;
use dartq_source::{FileTable, StringTable};

const CORE_FACE: &[&str] = &["List", "int"];

const TYPED_PARAM: &str = "void fun(List<int> xs) {\n  xs;\n}\n";
const BARE_PARAM: &str = "void fun(List xs) {\n  xs;\n}\n";
const NESTED_PARAM: &str = "void fun(List<List<int>> xss) {\n  xss;\n}\n";

/// Runs `source` through the live pipeline (the corpus posture: language
/// 3.13, a standalone library over the synthetic core face) and returns
/// the whole fold — the flow facts ride `UnitFold::facts`.
fn unit_fold(source: &str, strings: &StringTable) -> UnitFold {
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 13));
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
    let features = dartq_parser::ParserFeatures::from_feature_set(set);
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
            combinators: Combinators::unfiltered(),
        },
    );
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
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
        &dartq_resolution::LibraryClassModifiers::default(),
        set.is_enabled(Feature::InferenceUpdate2),
    )
}

/// The `ReadType` row at `offset` — the body read of the parameter's
/// identifier. Panics when absent (the pin's premise), returning the
/// row's `(base, nullable, args)` payload.
fn read_type_at(facts: &[FlowFact], offset: usize) -> (Base, bool, u32) {
    for fact in facts {
        if let FlowFact::ReadType { span, ty, args } = fact {
            if span.range.offset as usize == offset {
                return (ty.base, ty.nullable, *args);
            }
        }
    }
    panic!("no ReadType row at {offset}: {facts:?}");
}

/// Parses `source` into a bare [`Flow`] (the direct-drive face the
/// listener forwards must reproduce) and returns the flow with its
/// offset-sorted facts.
fn direct_drive<'t>(
    source: &'t str,
    tokens: &'t dartq_scanner::TokenTable,
) -> (Rc<RefCell<Flow<'t>>>, Vec<FlowFact>) {
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 13));
    let features = ParserFeatures::from_feature_set(set);
    let flow = Rc::new(RefCell::new(Flow::new(tokens, source)));
    parse_unit_with(source, tokens, features, &mut SharedFlow(Rc::clone(&flow)));
    let facts = flow.borrow_mut().take_facts();
    (flow, facts)
}

/// The scanner pass over `source` at the harness's posture.
fn scanned(source: &str) -> dartq_scanner::ScanResult {
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 13));
    scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: set.is_enabled(Feature::Augmentations),
        },
        file,
    )
}

/// A written `List<int>` parameter's body read carries the written
/// argument list on the listener-driven face, byte-identical to the
/// direct-drive face: same arena slot, and the tree resolves to exactly
/// one `int` argument (the #1010 payload).
#[test]
fn written_type_args_reach_listener_driven_rows() {
    let source = TYPED_PARAM;
    let read = source.find("xs;").unwrap();
    let strings = StringTable::default();
    let fold = unit_fold(source, &strings);
    let (_, _, args) = read_type_at(&fold.facts, read);
    assert_ne!(args, EMPTY_ARGS, "written args dropped as EMPTY_ARGS");

    let scanned = scanned(source);
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {scanned:?}"
    );
    let (flow, facts) = direct_drive(source, &scanned.tokens);
    let (base, nullable, direct_args) = read_type_at(&facts, read);
    assert!(!nullable);
    assert_eq!(
        args, direct_args,
        "listener face diverged from direct drive"
    );
    let flow = flow.borrow();
    let Base::Name(list_id) = base else {
        panic!("expected a named head, got {base:?}")
    };
    assert_eq!(flow.name_lexeme(list_id), "List");
    let tree = flow.type_args_at(direct_args);
    assert_eq!(tree.len(), 1, "one written argument: {tree:?}");
    let Base::Name(arg_id) = tree[0].base else {
        panic!("expected a named argument, got {:?}", tree[0])
    };
    assert_eq!(flow.name_lexeme(arg_id), "int");
    assert!(!tree[0].nullable);
    assert_eq!(tree[0].args, EMPTY_ARGS, "`int` carries no arguments");
}

/// A bare `List` parameter keeps `EMPTY_ARGS`: the forwarded trio must
/// not mint argument lists out of nothing (`handle_no_type_arguments`
/// clears the pending latch the group-less head would otherwise inherit).
#[test]
fn bare_type_stays_argument_less() {
    let source = BARE_PARAM;
    let read = source.find("xs;").unwrap();
    let strings = StringTable::default();
    let fold = unit_fold(source, &strings);
    let (_, _, args) = read_type_at(&fold.facts, read);
    assert_eq!(args, EMPTY_ARGS, "bare type minted arguments: {args}");
}

/// The nested face: `List<List<int>>` carries the full tree — the outer
/// argument is itself a `List` with one written `int` argument.
#[test]
fn nested_type_args_reach_listener_driven_rows() {
    let source = NESTED_PARAM;
    let read = source.find("xss;").unwrap();
    let strings = StringTable::default();
    let fold = unit_fold(source, &strings);
    let (_, _, args) = read_type_at(&fold.facts, read);
    assert_ne!(args, EMPTY_ARGS, "nested written args dropped");

    let scanned = scanned(source);
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {scanned:?}"
    );
    let (flow, facts) = direct_drive(source, &scanned.tokens);
    let (_, _, direct_args) = read_type_at(&facts, read);
    assert_eq!(
        args, direct_args,
        "listener face diverged from direct drive"
    );
    let flow = flow.borrow();
    let tree = flow.type_args_at(direct_args);
    assert_eq!(tree.len(), 1, "one written argument: {tree:?}");
    let inner = tree[0].args;
    assert_ne!(inner, EMPTY_ARGS, "outer argument lost its own list");
    let inner_tree = flow.type_args_at(inner);
    assert_eq!(inner_tree.len(), 1, "inner list holds one argument");
    let Base::Name(arg_id) = inner_tree[0].base else {
        panic!("expected a named argument, got {:?}", inner_tree[0])
    };
    assert_eq!(flow.name_lexeme(arg_id), "int");
}
