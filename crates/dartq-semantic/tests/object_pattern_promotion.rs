//! End-to-end pins for the matched-object-pattern promotion (#2122): a
//! `case Some():` clause's body must resolve members on the promoted
//! scrutinee type, not the declared one (official `promoteForPattern`,
//! upstream Dart SDK source,
//! pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart:7703-
//! 7755, fed by `analyzeObjectPattern` through the clause's pattern walk).
//! The regression is the analyzer corpus's `issue60264` family: the
//! diagnoser reported `undefined_getter 'value'` on the declared `Option`
//! because the listener dropped the object-pattern event the flow layer
//! promotes on.

use std::path::Path;

use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder, Namespaces};
use dartq_scanner::{ScanConfig, scan};
use dartq_semantic::MicroKind;
use dartq_semantic::collector::collect;
use dartq_semantic::diagnose::{StrictModes, UnitFold, fold_unit};
use dartq_semantic::library::ElementIndex;
use dartq_semantic::model::Ty;
use dartq_source::{FileTable, StringTable};

const CORE_FACE: &[&str] = &["Object", "bool", "double", "int", "String", "Null"];

const OPTION_SOME_NONE: &str = "\
class Option<T> {
  const Option();
}

class Some<T> extends Option<T> {
  final T value;
  const Some(this.value);
}

class None extends Option<Never> {
  bool get noneProp => false;
}

void fun<T>(Option<T> option) {
  switch (option) {
    case Some():
      final _ = option.value;
    case None():
      final _ = option.noneProp;
  }
}
";

/// Runs `source` through the live pipeline (the corpus posture: language
/// 3.13, a standalone library over the synthetic core face) and returns
/// the whole fold — report and read-type faces.
fn unit_fold<'s>(source: &'s str, strings: &StringTable) -> UnitFold {
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 13));
    let scanned = scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: set.is_enabled(dartq_language::Feature::Augmentations),
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
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
    )
}

/// The `case Some():` body reads the promoted member: no
/// `undefined_getter` on either clause body (the `case None():` arm keeps
/// its own matched type — the earlier promotion does not leak into it,
/// the failed arm joins the unmatched chain).
#[test]
fn matched_object_pattern_resolves_case_body_members() {
    let mut strings = StringTable::default();
    let fold = unit_fold(OPTION_SOME_NONE, &strings);
    let getters: Vec<_> = fold
        .report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_getter")
        .collect();
    assert!(getters.is_empty(), "{getters:?}");
}

/// The promotion is the clause body's face only: the case-body read rows
/// carry the matched types (`Some`, then `None`), and the flow `is`-test
/// face keeps its own promotion — same source shape, same result.
#[test]
fn case_body_read_rows_carry_the_matched_types() {
    let source = concat!(
        "class Option<T> { const Option(); } ",
        "class Some<T> extends Option<T> { final T value; const Some(this.value); } ",
        "class None extends Option<Never> { bool get noneProp => false; } ",
        "void fun<T>(Option<T> option) { switch (option) { case Some(): option.value; ",
        "case None(): option.noneProp; } if (option is Some) { option.value; } }"
    );
    let mut strings = StringTable::default();
    let fold = unit_fold(source, &strings);
    let spelling = |offset: usize| -> String {
        let row = fold
            .read_types
            .iter()
            .find(|(r, _)| r.offset as usize == offset)
            .unwrap_or_else(|| panic!("no read row at {offset}: {:?}", fold.read_types));
        match row.1 {
            Ty::Named { name, .. } => strings.resolve(name).to_string(),
            ref other => format!("{other:?}"),
        }
    };
    // The switch promotion does not leak past the switch: the `if` face
    // re-tests (`option is Some`), so its body read promotes on its own.
    let body_none = source.find("option.noneProp").unwrap();
    let if_body = source.rfind("option.value").unwrap();
    assert_eq!(spelling(body_none), "None", "{:?}", fold.read_types);
    assert_eq!(spelling(if_body), "Some", "{:?}", fold.read_types);
}
