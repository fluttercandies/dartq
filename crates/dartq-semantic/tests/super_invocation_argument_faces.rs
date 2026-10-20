//! Pins for the explicit super-constructor invocation's argument faces:
//! `: super(...)` / `: super.named(...)` in a constructor-initializer list
//! resolve the super constructor and pair its arguments like any other
//! invocation (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/resolver/constructor_invocation_resolver.dart
//! plus `resolveArgumentsToParameters`). The real-corpus burn (17 packages)
//! proved the whole face silent: 10 `extra_positional_arguments` and 10
//! `undefined_named_parameter` oracle-only triples, every one a `super(...)`
//! initializer (bloc `transition.dart`, drift `web_db.dart`, analyzer
//! `lint_codes.dart`).
//!
//! Each fixture mirrors a probed oracle verdict (fvm default `dart analyze`,
//! Dart 3.13-era):
//!
//! * the surplus positional reports `extra_positional_arguments` at the
//!   first extra argument, or the `_could_be_named` variant when the super
//!   constructor declares named parameters;
//! * an unknown named argument reports `undefined_named_parameter` at its
//!   label, and only there (a given required stays clean);
//! * a mismatched argument type reports `argument_type_not_assignable` at
//!   the argument's own span;
//! * an untyped field formal (`this.name`) types from its field's declared
//!   type, so a nullable argument into a non-nullable field formal reports;
//! * a clean call stays silent, and the deficit face keeps the ordinary
//!   count message.

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
/// provenance posture as the sibling rigs' SDK faces).
const CORE_FACE: &[&str] = &["Object", "bool", "double", "int", "String"];

/// Runs `source` through the live pipeline (the corpus posture: language
/// 3.13, a standalone library over the synthetic core face).
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

/// The fragment's rows for `codes` as `(code, 1-based line)` pairs in
/// source order — the exact faces, so an unexpected extra (or a lost) row
/// fails the pin by count as well as position.
fn code_lines(report: &FileReport, source: &str, codes: &[&str]) -> Vec<(String, usize)> {
    let mut rows: Vec<(String, usize)> = report
        .diagnostics
        .iter()
        .filter(|d| codes.contains(&d.code.name))
        .map(|d| {
            (
                d.code.name.to_string(),
                line_of(source, usize::try_from(d.range.offset).unwrap()),
            )
        })
        .collect();
    rows.sort();
    rows
}

/// The line `needle` first appears on in `source` (the fragments spell
/// each target once, so the needle is unambiguous).
fn line_of_needle(source: &str, needle: &str) -> usize {
    let offset = source
        .find(needle)
        .unwrap_or_else(|| panic!("needle {needle:?} absent from fragment"));
    line_of(source, offset)
}

const SURPLUS: &str = "\
class Base {
  Base();
}

class Derived extends Base {
  Derived() : super(42);
}
";

#[test]
fn bare_super_surplus_positional_reports_at_the_argument() {
    let strings = StringTable::new();
    let fold = unit_fold(SURPLUS, &strings);
    let got = code_lines(
        &fold.report,
        SURPLUS,
        &[
            "extra_positional_arguments",
            "extra_positional_arguments_could_be_named",
        ],
    );
    assert_eq!(
        got,
        vec![(
            "extra_positional_arguments".to_string(),
            line_of_needle(SURPLUS, "42")
        )],
        "the surplus reports at the first extra positional argument's own span"
    );
}

const SURPLUS_COULD_BE_NAMED: &str = "\
class Base2 {
  Base2({int? x});
}

class Derived2 extends Base2 {
  Derived2() : super(1, 2);
}
";

#[test]
fn surplus_over_named_formals_reports_could_be_named() {
    let strings = StringTable::new();
    let fold = unit_fold(SURPLUS_COULD_BE_NAMED, &strings);
    let got = code_lines(
        &fold.report,
        SURPLUS_COULD_BE_NAMED,
        &[
            "extra_positional_arguments",
            "extra_positional_arguments_could_be_named",
        ],
    );
    assert_eq!(
        got,
        vec![(
            "extra_positional_arguments_could_be_named".to_string(),
            line_of_needle(SURPLUS_COULD_BE_NAMED, "1")
        )],
        "named super formals upgrade the surplus to the could-be-named variant"
    );
}

const UNKNOWN_NAMED: &str = "\
class Base {
  Base.named({required String s, int v = 0});
}

class Derived extends Base {
  Derived() : super.named(s: 'x', bad: 2);
}
";

#[test]
fn selector_unknown_named_reports_at_the_label_only() {
    let strings = StringTable::new();
    let fold = unit_fold(UNKNOWN_NAMED, &strings);
    let got = code_lines(
        &fold.report,
        UNKNOWN_NAMED,
        &[
            "undefined_named_parameter",
            "missing_required_argument",
            "extra_positional_arguments",
        ],
    );
    assert_eq!(
        got,
        vec![(
            "undefined_named_parameter".to_string(),
            line_of_needle(UNKNOWN_NAMED, "bad:")
        )],
        "the unknown label reports the face; the given required named stays clean"
    );
}

const SELECTOR_TYPE_MISMATCH: &str = "\
class Base {
  Base.wide(String a);
}

class Derived extends Base {
  Derived() : super.wide(3.5);
}
";

#[test]
fn selector_positional_type_mismatch_reports_at_the_argument() {
    let strings = StringTable::new();
    let fold = unit_fold(SELECTOR_TYPE_MISMATCH, &strings);
    let got = code_lines(
        &fold.report,
        SELECTOR_TYPE_MISMATCH,
        &["argument_type_not_assignable"],
    );
    assert_eq!(
        got,
        vec![(
            "argument_type_not_assignable".to_string(),
            line_of_needle(SELECTOR_TYPE_MISMATCH, "3.5")
        )],
        "the paired formal decides the assignability face at the argument's span"
    );
}

const FIELD_FORMAL_TYPES_FROM_FIELDS: &str = "\
class Page {
  String name;
  Page.named(this.name, int n);
}

void use(Page p) {}
void go(String? t) {
  use(Page.named(t, 3));
}
";

#[test]
fn untyped_field_formal_types_from_its_field() {
    let strings = StringTable::new();
    let fold = unit_fold(FIELD_FORMAL_TYPES_FROM_FIELDS, &strings);
    let got = code_lines(
        &fold.report,
        FIELD_FORMAL_TYPES_FROM_FIELDS,
        &["argument_type_not_assignable"],
    );
    assert_eq!(
        got,
        vec![(
            "argument_type_not_assignable".to_string(),
            line_of_needle(FIELD_FORMAL_TYPES_FROM_FIELDS, "t, 3")
        )],
        "an untyped `this.name` formal takes the field's declared type, so a \
         nullable argument reports; the annotated formal beside it stays clean"
    );
}

const CLEAN_CALL_STAYS_SILENT: &str = "\
class Base {
  Base.n(int x, {String? tag});
}

class Derived extends Base {
  Derived() : super.n(3, tag: 'x');
  Derived.other() : super.n(3);
}
";

#[test]
fn clean_super_invocations_stay_silent() {
    let strings = StringTable::new();
    let fold = unit_fold(CLEAN_CALL_STAYS_SILENT, &strings);
    let got = code_lines(
        &fold.report,
        CLEAN_CALL_STAYS_SILENT,
        &[
            "argument_type_not_assignable",
            "extra_positional_arguments",
            "extra_positional_arguments_could_be_named",
            "undefined_named_parameter",
            "not_enough_positional_arguments",
        ],
    );
    assert_eq!(got, vec![], "matching arguments emit nothing");
}

const DEFICIT: &str = "\
class Base {
  Base(int x, int y);
}

class Derived extends Base {
  Derived() : super(1);
}
";

#[test]
fn deficit_reports_the_count_face() {
    let strings = StringTable::new();
    let fold = unit_fold(DEFICIT, &strings);
    let got = code_lines(
        &fold.report,
        DEFICIT,
        &[
            "not_enough_positional_arguments",
            "extra_positional_arguments",
        ],
    );
    assert_eq!(
        got,
        vec![(
            "not_enough_positional_arguments".to_string(),
            line_of_needle(DEFICIT, "1)")
        )],
        "the positional deficit reports the count face inside the parens"
    );
}

const UNRESOLVED_SUPERCLASS: &str = "\
class Orphan extends Missing {
  Orphan() : super(42, x: 1);
}
";

#[test]
fn unresolved_superclass_pairs_against_an_empty_table() {
    let strings = StringTable::new();
    let fold = unit_fold(UNRESOLVED_SUPERCLASS, &strings);
    let got = code_lines(
        &fold.report,
        UNRESOLVED_SUPERCLASS,
        &[
            "extra_positional_arguments",
            "undefined_named_parameter",
            "not_enough_positional_arguments",
        ],
    );
    assert_eq!(
        got,
        vec![
            (
                "extra_positional_arguments".to_string(),
                line_of_needle(UNRESOLVED_SUPERCLASS, "42")
            ),
            (
                "undefined_named_parameter".to_string(),
                line_of_needle(UNRESOLVED_SUPERCLASS, "x:")
            ),
        ],
        "the dead-edge super face resolves no constructor, so the oracle's \
         null-element arm pairs the arguments against an empty formal table"
    );
}

const NO_EXTENDS_SUPER: &str = "\
class Orphan {
  Orphan() : super(42, x: 1);
}
";

#[test]
fn no_extends_super_pairs_against_the_object_face() {
    let strings = StringTable::new();
    let fold = unit_fold(NO_EXTENDS_SUPER, &strings);
    let got = code_lines(
        &fold.report,
        NO_EXTENDS_SUPER,
        &[
            "extra_positional_arguments",
            "undefined_named_parameter",
            "not_enough_positional_arguments",
        ],
    );
    assert_eq!(
        got,
        vec![
            (
                "extra_positional_arguments".to_string(),
                line_of_needle(NO_EXTENDS_SUPER, "42")
            ),
            (
                "undefined_named_parameter".to_string(),
                line_of_needle(NO_EXTENDS_SUPER, "x:")
            ),
        ],
        "the implicit Object super constructor declares no formals"
    );
}

const MISSING_SELECTOR_STAYS_SILENT: &str = "\
class Base {
  Base();
}

class Derived extends Base {
  Derived() : super.missing(42, x: 1);
}
";

#[test]
fn missing_selector_defers_to_the_existence_face() {
    let strings = StringTable::new();
    let fold = unit_fold(MISSING_SELECTOR_STAYS_SILENT, &strings);
    let got = code_lines(
        &fold.report,
        MISSING_SELECTOR_STAYS_SILENT,
        &[
            "extra_positional_arguments",
            "undefined_named_parameter",
            "not_enough_positional_arguments",
        ],
    );
    assert_eq!(
        got, vec![],
        "a resolved class without the selector reports \
         undefined_constructor_in_initializer only — the oracle pairs no \
         arguments against it"
    );
}

const ALIAS_FORMAL: &str = "\
typedef Pred = bool Function(String c);

bool ok(String c) => true;
bool nullable(String? c) => true;
int wrong(String c) => 1;

void takesPred(Pred p) {}

void go() {
  takesPred(ok);
  takesPred(nullable);
  takesPred(wrong);
}
";

#[test]
fn alias_unfolds_for_tear_off_arguments() {
    let strings = StringTable::new();
    let fold = unit_fold(ALIAS_FORMAL, &strings);
    let got = code_lines(
        &fold.report,
        ALIAS_FORMAL,
        &["argument_type_not_assignable"],
    );
    assert_eq!(
        got,
        vec![(
            "argument_type_not_assignable".to_string(),
            line_of_needle(ALIAS_FORMAL, "takesPred(wrong)")
        )],
        "the alias RHS decides: contravariant-ok and equal tear-offs stay \
         silent, the return-type mismatch reports"
    );
}

const SELECTOR_OVER_DEAD_SUPERCLASS: &str = "\
class T extends Missing {
  T(int p) : super.delegate(p, x: 1);
}
";

#[test]
fn selector_over_dead_superclass_stays_silent_on_argument_faces() {
    let strings = StringTable::new();
    let fold = unit_fold(SELECTOR_OVER_DEAD_SUPERCLASS, &strings);
    let got = code_lines(
        &fold.report,
        SELECTOR_OVER_DEAD_SUPERCLASS,
        &[
            "extra_positional_arguments",
            "undefined_named_parameter",
            "not_enough_positional_arguments",
        ],
    );
    assert_eq!(
        got, vec![],
        "a written selector over a dead superclass resolves against the \
         implicit Object and misses it — undefined_constructor_in_initializer \
         reports alone, with no argument faces (probed)"
    );
}

const SELECTOR_NO_EXTENDS: &str = "\
class T {
  T(int p) : super.delegate(p, x: 1);
}
";

#[test]
fn selector_with_no_extends_stays_silent_on_argument_faces() {
    let strings = StringTable::new();
    let fold = unit_fold(SELECTOR_NO_EXTENDS, &strings);
    let got = code_lines(
        &fold.report,
        SELECTOR_NO_EXTENDS,
        &[
            "extra_positional_arguments",
            "undefined_named_parameter",
            "not_enough_positional_arguments",
        ],
    );
    assert_eq!(
        got, vec![],
        "the same Object-miss shape applies to the no-extends face"
    );
}

const EMPTY_TABLE_COMPOUND_NAMED_VALUE: &str = "\
class T extends Missing {
  T(bool c) : super(x: c ? 1 : 2);
}
";

#[test]
fn empty_table_counts_only_attributed_positional_windows() {
    let strings = StringTable::new();
    let fold = unit_fold(EMPTY_TABLE_COMPOUND_NAMED_VALUE, &strings);
    let got = code_lines(
        &fold.report,
        EMPTY_TABLE_COMPOUND_NAMED_VALUE,
        &["extra_positional_arguments", "extra_positional_arguments_could_be_named"],
    );
    assert_eq!(
        got, vec![],
        "a compound named-argument value leaves an unattributed window — \
         no phantom surplus against the empty table (the oracle sees one \
         named argument)"
    );
}

// --- the unresolved-superclass Object fallback (`undefined_constructor_in_initializer`) ---
//
// The superclass itself unresolvable (the `Missing` posture the sibling
// fixtures share) suppresses the ordinary selector check, but the oracle
// keeps walking: the implicit `super()` is satisfied by Object's unnamed
// constructor, while a *written named* selector misses Object and reports
// (upstream Dart SDK source, `error_verifier.dart`'s
// unresolved-superclass walk feeding `visitSuperConstructorInvocation`).

const UCII_NAMED_OVER_UNDEFINED: &str = "\
class Host extends Missing {
  Host() : super.named();
}
";

#[test]
fn named_super_selector_over_undefined_superclass_reports() {
    let strings = StringTable::new();
    let fold = unit_fold(UCII_NAMED_OVER_UNDEFINED, &strings);
    let got = code_lines(
        &fold.report,
        UCII_NAMED_OVER_UNDEFINED,
        &["undefined_constructor_in_initializer"],
    );
    assert_eq!(
        got,
        vec![(
            "undefined_constructor_in_initializer".to_string(),
            line_of_needle(UCII_NAMED_OVER_UNDEFINED, "super.named")
        )],
        "a written named selector over an undefined superclass falls through \
         to Object and misses — the oracle reports at the initializer's span"
    );
}

const UCII_UNNAMED_OVER_UNDEFINED: &str = "\
class Host extends Missing {
  Host() : super();
}
";

#[test]
fn unnamed_super_over_undefined_superclass_stays_silent() {
    let strings = StringTable::new();
    let fold = unit_fold(UCII_UNNAMED_OVER_UNDEFINED, &strings);
    let got = code_lines(
        &fold.report,
        UCII_UNNAMED_OVER_UNDEFINED,
        &["undefined_constructor_in_initializer"],
    );
    assert_eq!(
        got, vec![],
        "the implicit super() face is satisfied by Object's unnamed \
         constructor — no report"
    );
}

const UCII_NAMED_OVER_RESOLVED: &str = "\
class Base {
  Base.named();
}

class Host extends Base {
  Host() : super.named();
}
";

#[test]
fn named_super_selector_over_resolved_superclass_stays_silent() {
    let strings = StringTable::new();
    let fold = unit_fold(UCII_NAMED_OVER_RESOLVED, &strings);
    let got = code_lines(
        &fold.report,
        UCII_NAMED_OVER_RESOLVED,
        &["undefined_constructor_in_initializer"],
    );
    assert_eq!(
        got, vec![],
        "the fallback arm only covers unresolvable superclasses — a resolved \
         named constructor is the ordinary satisfied path"
    );
}

/// The two-file rig for the prefixed-superclass posture: `B` lives in a
/// sibling library imported under the `p` prefix, so the subclass's
/// superclass edge (which keeps the bare continuation name) resolves no
/// container through the bare lookup.
fn unit_fold_prefixed(source: &str, other_source: &str, strings: &StringTable) -> UnitFold {
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let other_file = files.intern(Path::new("rel.dart"));
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
    let other = scan(
        other_source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: set.is_enabled(dartq_language::Feature::Augmentations),
        },
        other_file,
    );
    assert!(
        other.diagnostics.is_empty(),
        "scanner diagnostics: {other:?}"
    );
    let features = dartq_parser::ParserFeatures::from_feature_set(set);
    let elements = collect(source, &scanned.tokens, features, strings);
    let other_elements = collect(other_source, &other.tokens, features, strings);
    let mut builder = LibraryGraphBuilder::new(strings);
    let lib = builder.intern("package:t/main.dart");
    let other_lib = builder.intern("package:t/rel.dart");
    let core_file = files.intern(Path::new("<dart:core>"));
    let core = builder.intern("dart:core");
    for name in CORE_FACE {
        builder.declare(core, strings.intern(name), core_file);
    }
    for (name, decl) in other_elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(other_lib, name, other_file);
    }
    builder.import(
        lib,
        ImportEdge {
            target: Some(other_lib),
            prefix: Some(strings.intern("p")),
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    builder.import(
        lib,
        ImportEdge {
            target: Some(core),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    index.insert(other_file, other_elements);
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

const UCII_PREFIXED_REACHABLE: &str = "\
class Host extends p.B {
  Host() : super.named();
}
";

const PREFIXED_SIBLING: &str = "\
class B {
  B.named();
}
";

#[test]
fn named_super_selector_over_prefixed_but_reachable_superclass_stays_silent() {
    let strings = StringTable::new();
    let fold = unit_fold_prefixed(UCII_PREFIXED_REACHABLE, PREFIXED_SIBLING, &strings);
    let got = code_lines(
        &fold.report,
        UCII_PREFIXED_REACHABLE,
        &["undefined_constructor_in_initializer"],
    );
    assert_eq!(
        got, vec![],
        "a superclass reachable through a prefixed import resolves for the \
         oracle — the Object fallback must not report through the bare \
         continuation edge"
    );
}
