//! #2241 pins: the unchecked-use receiver verdict against the flow's
//! variable promotions.
//!
//! A `var`-inferred local whose initializer is nullable (`var files =
//! this._files;`) promoted by a null check reads non-null at its member
//! accesses — the oracle is silent. The member walk's receiver verdict
//! re-decides against the flow's staged promotion ([`PromotionStaging`]):
//! the opaque head (`Ty::Unknown`, the lattice's only spelling for the
//! promoted `var`-inferred local) substitutes the parse-time type's
//! non-null form, exactly like `filter_staged`'s general arm. The
//! `== null` branch (promoted to `Null`), the else branch, and the
//! unguarded read keep the report.

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

fn unchecked(report: &FileReport) -> Vec<String> {
    code_names(report)
        .into_iter()
        .filter(|name| name.starts_with("unchecked_use_of_nullable_value"))
        .collect()
}

/// Cell a: `var`-inferred from an implicit-this field read, `!= null`
/// then-branch. The oracle is silent.
#[test]
fn pin_var_inferred_field_init_then_silent() {
    let source = "\
class int {}
class Box {
  int length = 0;
}
class C {
  Box? _files;
  void a() {
    var files = _files;
    if (files != null) {
      sink(files.length);
    }
  }
}
void sink(int x) {}
";
    let report = unit_report(source);
    assert!(
        unchecked(&report).is_empty(),
        "cell a: {:#?}",
        code_names(&report)
    );
}

/// Cell b: `var`-inferred from a nullable-returning invocation,
/// `!= null` then-branch. The oracle is silent.
#[test]
fn pin_var_inferred_invoke_init_then_silent() {
    let source = "\
class int {}
class Box {
  int length = 0;
}
class C {
  Box? getter() => null;
  void b() {
    var files = getter();
    if (files != null) sink(files.length);
  }
}
void sink(int x) {}
";
    let report = unit_report(source);
    assert!(
        unchecked(&report).is_empty(),
        "cell b: {:#?}",
        code_names(&report)
    );
}

/// Cell c (control): the explicit `Box?` declaration with an assignment
/// initializer behaved before the fix.
#[test]
fn pin_explicit_declared_then_silent() {
    let source = "\
class int {}
class Box {
  int length = 0;
}
class C {
  Box? _files;
  void c() {
    Box? files;
    files = _files;
    if (files != null) sink(files.length);
  }
}
void sink(int x) {}
";
    let report = unit_report(source);
    assert!(
        unchecked(&report).is_empty(),
        "cell c: {:#?}",
        code_names(&report)
    );
}

/// Cell d: `var`-inferred from an explicit `this.` field read,
/// `!= null` then-branch. The oracle is silent.
#[test]
fn pin_var_inferred_this_init_then_silent() {
    let source = "\
class int {}
class Box {
  int length = 0;
}
class C {
  Box? _files;
  void d() {
    var files = this._files;
    if (files != null) sink(files.length);
  }
}
void sink(int x) {}
";
    let report = unit_report(source);
    assert!(
        unchecked(&report).is_empty(),
        "cell d: {:#?}",
        code_names(&report)
    );
}

/// Cell i (control): the parameter form behaved before the fix.
#[test]
fn pin_parameter_then_silent() {
    let source = "\
class int {}
class Box {
  int length = 0;
}
void i(Box? p) {
  if (p != null) sink(p.length);
}
void sink(int x) {}
";
    let report = unit_report(source);
    assert!(
        unchecked(&report).is_empty(),
        "cell i: {:#?}",
        code_names(&report)
    );
}

/// Cell e: the `== null` then-branch promotes to `Null` — the oracle
/// reports, and the staged `Null` face must keep the verdict.
#[test]
fn pin_null_branch_keeps_report() {
    let source = "\
class int {}
class Box {
  int length = 0;
}
class C {
  Box? _files;
  void e() {
    var files = this._files;
    if (files == null) {
      sink(files.length);
    }
  }
}
void sink(int x) {}
";
    let report = unit_report(source);
    assert!(
        !unchecked(&report).is_empty(),
        "cell e lost the report: {:#?}",
        code_names(&report)
    );
}

/// Cell g: the else branch of `!= null` — the oracle reports.
#[test]
fn pin_else_branch_keeps_report() {
    let source = "\
class int {}
class Box {
  int length = 0;
}
class C {
  Box? _files;
  void g() {
    var files = this._files;
    if (files != null) {
    } else {
      sink(files.length);
    }
  }
}
void sink(int x) {}
";
    let report = unit_report(source);
    assert!(
        !unchecked(&report).is_empty(),
        "cell g lost the report: {:#?}",
        code_names(&report)
    );
}

/// Cell h: the unguarded read — no promotion stands, the oracle reports.
#[test]
fn pin_unguarded_keeps_report() {
    let source = "\
class int {}
class Box {
  int length = 0;
}
class C {
  Box? _files;
  void h() {
    var files = this._files;
    sink(files.length);
  }
}
void sink(int x) {}
";
    let report = unit_report(source);
    assert!(
        !unchecked(&report).is_empty(),
        "cell h lost the report: {:#?}",
        code_names(&report)
    );
}
