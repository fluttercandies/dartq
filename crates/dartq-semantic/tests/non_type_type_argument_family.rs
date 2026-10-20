//! Route pins for the `non_type_as_type_argument` family's
//! member-invocation faces (#2253): a name written in an invocation's
//! explicit type-argument group resolves through the oracle's
//! `_isTypeInTypeArgumentList` face whatever the callee's resolution
//! (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/resolver/named_type_resolver.dart, the
//! `reportNullOrNonTypeElement` arm that checks the parent
//! `TypeArgumentList` before the `undefined_class` fallthrough).
//!
//! The mass shape — `recv.method<Undefined>()`, the corpus's
//! `cast<T>()` / `whereType<T>()` / `tryCast<T>()` spellings — died
//! between parse and pass 2: the send links no callee (the group's
//! `TypeUse` rows top the row stack at `(`), so the member merge folded
//! the type argument AS the member. The merge now defers the trailing
//! `TypeUse` rows to pass 2, where the type-argument context chain
//! codes them.
//!
//! The pins split by route:
//!
//! * The member-invocation faces (undefined callee, resolved callee,
//!   with/without arguments, resolved non-type name) — the family mass.
//! * The declaration and bare-function controls — routes that already
//!   emitted, kept so a future stack-geometry change cannot silently
//!   reroute them.
//! * The cascade, tear-off, and keyword-receiver spellings — the same
//!   merge with other geometries.
//! * The silent control (a resolved type argument) — guards the fix
//!   against over-reporting.

use std::path::Path;

use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder, Namespaces};
use dartq_scanner::{ScanConfig, scan};
use dartq_semantic::collector::collect;
use dartq_semantic::diagnose::{StrictModes, UnitFold, fold_unit};
use dartq_semantic::library::ElementIndex;
use dartq_semantic::{FileReport, MicroKind};
use dartq_source::{FileTable, StringTable};

/// The synthetic dart:core names these fragments consume (same
/// provenance posture as the diagnose unit tests' SDK faces).
const CORE_FACE: &[&str] = &["Object", "bool", "double", "int", "String"];

/// The family mass: a receiver invocation's explicit type arguments —
/// undefined callee with empty arguments (the corpus `cast<T>()`
/// shape), resolved callee, an argument list present, and a resolved
/// non-type name — plus the silent control (a resolved type argument).
const MEMBER_INVOCATIONS: &str = "\
class Box {
  int? grab<X>() => null;
}

int notAType = 1;

void f(Box r) {
  r.zork<Missing>();
  r.grab<Missing2>();
  r.zork<Missing3>(1);
  r.grab<notAType>();
  r.grab<int>();
}
";

/// The declaration-position control: routes that already emitted before
/// the fix (a written type annotation and a typed collection literal).
const DECLARATION_POSITIONS: &str = "\
class Holder<T> {}

void f() {
  Holder<Missing>? x;
  var y = <Missing2>[];
}
";

/// The bare-function control: a headless group whose callee row the
/// arity machinery already linked pre-fix.
const BARE_FUNCTION: &str = "\
void g<T>() {}

void f() {
  g<Missing>();
}
";

/// The cascade-section and tear-off-application spellings: the same
/// merge under other dot geometries (no `(` after the group).
const CASCADE_AND_TEAR_OFF: &str = "\
class Box {
  int? grab<X>() => null;
}

void f(Box r) {
  r..grab<Missing>();
  var t = r.grab<Missing2>;
}
";

/// The keyword-receiver spellings: implicit `this`, explicit `this`,
/// and `super` — every merge whose receiver is not a bare row.
const KEYWORD_RECEIVERS: &str = "\
class Base {
  int? grab<X>() => null;
}

class Box extends Base {
  void m() {
    grab<Missing>();
    this.grab<Missing2>();
    super.grab<Missing3>();
  }
}
";

/// A nested type argument (`Holder<Missing>` inside the group) — the
/// drain walks past inner group members to defer every trailing row.
const NESTED_ARGUMENT: &str = "\
class Holder<T> {}

class Box {
  int? grab<X>() => null;
}

void f(Box r) {
  r.grab<Holder<Missing>>();
}
";

/// Runs `source` through the live pipeline (the corpus posture: language
/// 3.13, a standalone library over the synthetic core face) and returns
/// the whole fold.
fn unit_fold(source: &str, strings: &StringTable) -> UnitFold {
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

/// The 1-based line of a diagnostic offset in `source`.
fn line_of(source: &str, offset: usize) -> usize {
    source[..offset].bytes().filter(|b| *b == b'\n').count() + 1
}

/// The fragment's `non_type_as_type_argument` rows as `(1-based line)`
/// values in source order — the family face exactly, so an unexpected
/// extra (or a lost) row fails the pin by count as well as position.
fn non_type_lines(report: &FileReport, source: &str) -> Vec<usize> {
    let mut lines: Vec<usize> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "non_type_as_type_argument")
        .map(|d| line_of(source, usize::try_from(d.range.offset).unwrap()))
        .collect();
    lines.sort_unstable();
    lines
}

/// The line `needle` first appears on in `source` (the fragments spell
/// each target name once, so the needle is unambiguous).
fn line_of_needle(source: &str, needle: &str) -> usize {
    let offset = source
        .find(needle)
        .unwrap_or_else(|| panic!("needle {needle:?} absent from fragment"));
    line_of(source, offset)
}

#[test]
fn member_invocation_type_arguments_report_the_face() {
    let strings = StringTable::new();
    let fold = unit_fold(MEMBER_INVOCATIONS, &strings);
    let got = non_type_lines(&fold.report, MEMBER_INVOCATIONS);
    let want: Vec<usize> = ["<Missing>", "<Missing2>", "<Missing3>", "<notAType>"]
        .iter()
        .map(|needle| line_of_needle(MEMBER_INVOCATIONS, needle))
        .collect();
    assert_eq!(
        got, want,
        "every written type-argument name on a member invocation reports \
         the face; the resolved type argument (`int`) stays silent"
    );
}

#[test]
fn declaration_positions_keep_the_face() {
    let strings = StringTable::new();
    let fold = unit_fold(DECLARATION_POSITIONS, &strings);
    let got = non_type_lines(&fold.report, DECLARATION_POSITIONS);
    let want: Vec<usize> = ["Missing", "Missing2"]
        .iter()
        .map(|name| line_of_needle(DECLARATION_POSITIONS, name))
        .collect();
    assert_eq!(
        got, want,
        "the annotation and the collection literal both report the face"
    );
}

#[test]
fn bare_function_type_arguments_keep_the_face() {
    let strings = StringTable::new();
    let fold = unit_fold(BARE_FUNCTION, &strings);
    let got = non_type_lines(&fold.report, BARE_FUNCTION);
    assert_eq!(
        got,
        vec![line_of_needle(BARE_FUNCTION, "Missing")],
        "the pre-fix bare-callee route must stay intact"
    );
}

#[test]
fn cascade_and_tear_off_type_arguments_report_the_face() {
    let strings = StringTable::new();
    let fold = unit_fold(CASCADE_AND_TEAR_OFF, &strings);
    let got = non_type_lines(&fold.report, CASCADE_AND_TEAR_OFF);
    let want: Vec<usize> = ["Missing", "Missing2"]
        .iter()
        .map(|name| line_of_needle(CASCADE_AND_TEAR_OFF, name))
        .collect();
    assert_eq!(
        got, want,
        "a cascade section's group and a tear-off application report the \
         written names like the invoked spelling"
    );
}

#[test]
fn keyword_receiver_type_arguments_report_the_face() {
    let strings = StringTable::new();
    let fold = unit_fold(KEYWORD_RECEIVERS, &strings);
    let got = non_type_lines(&fold.report, KEYWORD_RECEIVERS);
    let want: Vec<usize> = ["Missing", "Missing2", "Missing3"]
        .iter()
        .map(|name| line_of_needle(KEYWORD_RECEIVERS, name))
        .collect();
    assert_eq!(
        got, want,
        "implicit-this, explicit-this, and super receivers all defer the \
         group's rows through the merge"
    );
}

#[test]
fn nested_type_argument_reports_the_inner_name() {
    let strings = StringTable::new();
    let fold = unit_fold(NESTED_ARGUMENT, &strings);
    let got = non_type_lines(&fold.report, NESTED_ARGUMENT);
    assert_eq!(
        got,
        vec![line_of_needle(NESTED_ARGUMENT, "Missing")],
        "the drain defers the inner group's row; the resolved head (`Holder`) \
         stays silent"
    );
}
