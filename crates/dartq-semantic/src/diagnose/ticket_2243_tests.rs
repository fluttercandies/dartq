//! #2243 forensic probes: two flow-promotion differential families.
//!
//! 1. A `var` local initialized from an args-carrying nullable field
//!    (`var l = field;` — field `Box<int>?`), null-checked with an early
//!    return, then returned: the oracle keeps the promotion (silent),
//!    dartq reports the nullable return. The same shape with an
//!    argument-less declared type (`Egg?`) is silent.
//! 2. `x ??= C<int>();` — the if-null assignment through an
//!    args-carrying constructor invocation must promote `x` to
//!    `C<int>`; dartq leaves it unpromoted (an argument-less RHS
//!    promotes).

use crate::collector::collect;
use crate::diagnose::{FileReport, StrictModes, diagnose_with};
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

fn unit_report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let scanned = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {source:?}"
    );
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    diagnose_with(
        source,
        &scanned.tokens,
        features,
        &strings,
        file,
        lib,
        &graph,
        &namespaces,
        &index,
        StrictModes::default(),
        &LibraryClassModifiers::default(),
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
    )
}

fn code_names(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .map(|d| format!("{}@{}", d.code.name, d.range.offset))
        .collect()
}

/// Bug 1: the return-path read of an args-carrying promoted local. The
/// oracle is silent (the promotion stands); dartq reports the nullable
/// return.
#[test]
fn probe_args_local_return_path() {
    let source = "\
class int {}
class Box<T> {
  Box<int>? field;
  Box<int> m() {
    var l = field;
    if (l == null) return Box<int>();
    return l;
  }
}
";
    let report = unit_report(source);
    assert!(
        report.diagnostics.is_empty(),
        "return path: {:#?}",
        code_names(&report)
    );
}

/// Bug 1's argument-less control: the identical shape with `Egg?` is
/// silent today (the promotion stands).
#[test]
fn probe_argsless_local_return_path() {
    let source = "\
class Egg {}
class C {
  Egg? field;
  Egg m() {
    var l = field;
    if (l == null) return Egg();
    return l;
  }
}
";
    let report = unit_report(source);
    assert!(
        report.diagnostics.is_empty(),
        "argument-less control: {:#?}",
        code_names(&report)
    );
}

/// Bug 1's argument-position control: the same promoted local read in
/// argument position.
#[test]
fn probe_args_local_argument_position() {
    let source = "\
class int {}
class Box<T> {
  Box<int>? field;
  void m() {
    var l = field;
    if (l == null) return;
    firstOf(l);
  }
}
void firstOf(Box<int> xs) {}
";
    let report = unit_report(source);
    assert!(
        report.diagnostics.is_empty(),
        "argument position: {:#?}",
        code_names(&report)
    );
}

/// Bug 2: `x ??= C<int>();` must promote `x` to `C<int>`.
#[test]
fn probe_if_null_assign_args_constructor_rhs() {
    let source = "\
class int {}
class C<T> {}
void g(C<int> x) {}
void f() {
  C<int>? x;
  x ??= C<int>();
  g(x);
}
";
    let report = unit_report(source);
    assert!(
        report.diagnostics.is_empty(),
        "??= args constructor RHS: {:#?}",
        code_names(&report)
    );
}

/// Bug 2's argument-less control: `x ??= Foo();` promotes today.
#[test]
fn probe_if_null_assign_argsless_constructor_rhs() {
    let source = "\
class Foo {}
void g(Foo x) {}
void f() {
  Foo? x;
  x ??= Foo();
  g(x);
}
";
    let report = unit_report(source);
    assert!(
        report.diagnostics.is_empty(),
        "??= argument-less RHS: {:#?}",
        code_names(&report)
    );
}
