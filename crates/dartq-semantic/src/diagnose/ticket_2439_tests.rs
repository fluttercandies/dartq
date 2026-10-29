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

/// The one-library rig: `source` is the sole analyzed file of
/// `package:t`, no imports.
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
        // The same derivation the engine's fold call site makes from
        // the settled set (the `inference-update-2` release gate).
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
    )
}

fn count_code(report: &FileReport, name: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .count()
}

fn at(source: &str, needle: &str) -> u32 {
    u32::try_from(
        source
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not in {source:?}")),
    )
    .expect("offset fits u32")
}

/// Exactly one row of `name`, anchored at `offset`.
fn one_at(report: &FileReport, name: &str, offset: u32, source: &str) {
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .collect();
    assert_eq!(rows.len(), 1, "{name}: {rows:?}");
    assert_eq!(rows[0].range.offset, offset, "in {source:?}");
}

// —— the disjointness gate (the constant-over-Enum never-match family) ——

/// The reader shape (`probe2437/pattern`, the bare-rig spelling per the
/// #2432 convention — `List` is a local stub): a generic read whose written
/// bound is the `Enum` root feeds a statement switch whose cases enumerate
/// the read enum's constants. The call solves `T := AstNodeTag`, so the
/// scrutinee reads the enum and every case may overlap it — no case is
/// dead and no never-match row reports (oracle 3.13.1: clean).
#[test]
fn enum_constants_over_enum_bound_scrutinee_stay_live() {
    let source = "\
class List<E> { E get first => throw 0; }
enum AstNodeTag { a, b, c }
class R {
  T readEnum<T extends Enum>(List<T> values) => values.first;
}
f(R reader) {
  var tag = reader.readEnum(<AstNodeTag>[AstNodeTag.a]);
  switch (tag) {
    case AstNodeTag.a:
      return 1;
    case AstNodeTag.b:
      return 2;
    case AstNodeTag.c:
      return 3;
  }
}
";
    let report = unit_report(source);
    assert_eq!(
        count_code(&report, "constant_pattern_never_matches_value_type"),
        0,
        "{:?}",
        report.diagnostics
    );
    assert_eq!(count_code(&report, "dead_code"), 0);
}

/// The unsolved-bound face of the same family: the scrutinee stays the raw
/// parameter spelling (`T extends Enum`, the call site unable to solve it).
/// The parameter head is the name level's blind spot — its bound is
/// invisible — so the gate answers overlap and no case body dies (oracle
/// 3.13.1, probe pd: clean).
#[test]
fn enum_constants_over_raw_param_scrutinee_stay_live() {
    let source = "\
class Enum {}
enum AstNodeTag { a, b, c }
void f<T extends Enum>(T x) {
  switch (x) {
    case AstNodeTag.a:
      break;
    case AstNodeTag.b:
      break;
    case AstNodeTag.c:
      break;
  }
}
";
    let report = unit_report(source);
    assert_eq!(
        count_code(&report, "dead_code"),
        0,
        "{:?}",
        report.diagnostics
    );
}

/// The bare-root shape (`probe2437/p2`): an `Enum`-typed scrutinee whose
/// statement-switch cases enumerate one enum's constants. The space model
/// cannot express "the `Enum` root minus one constant", but the constant's
/// type may overlap the root — the disjointness gate keeps every body live
/// (oracle 3.13.1: `body_might_complete_normally_nullable` alone, no
/// `dead_code`).
#[test]
fn enum_constants_over_enum_scrutinee_keep_bodies_live() {
    let source = "\
enum AstNodeTag { a, b, c }

Object? f(Enum tag) {
  switch (tag) {
    case AstNodeTag.a:
      return 1;
    case AstNodeTag.b:
      return 2;
    case AstNodeTag.c:
      return 3;
  }
}
";
    let report = unit_report(source);
    assert_eq!(
        count_code(&report, "dead_code"),
        0,
        "{:?}",
        report.diagnostics
    );
}

/// A null constant against a non-nullable enum stays genuinely disjoint —
/// the null face of the gate keeps the body row (oracle 3.13.1:
/// `constant_pattern_never_matches_value_type` + `dead_code` over the
/// null case's body).
#[test]
fn null_constant_over_non_nullable_enum_still_dies() {
    let source = "\
enum E { a, b }
int f(E e) {
  switch (e) {
    case E.a:
      return 1;
    case null:
      return 2;
    case E.b:
      return 3;
  }
}
";
    let report = unit_report(source);
    one_at(
        &report,
        "constant_pattern_never_matches_value_type",
        at(source, "null"),
        source,
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "dead_code" && d.range.offset == at(source, "return 2")),
        "{:?}",
        report.diagnostics
    );
}

/// A string constant against an int scrutinee stays genuinely disjoint:
/// the never-match warning reports at the constant (oracle 3.13.1, probe
/// p7 — the warning alone; the disjointness gate must not silence the
/// equality-check face).
#[test]
fn string_constant_over_int_still_reports_never_match() {
    let source = "\
int f(int x) {
  switch (x) {
    case 'str':
      print('a');
    case 1:
      return 2;
  }
  return 0;
}
";
    let report = unit_report(source);
    one_at(
        &report,
        "constant_pattern_never_matches_value_type",
        at(source, "'str'"),
        source,
    );
}

// —— the Enum-root covering face ——————————————————————————————————————

// The bare diagnose rigs cannot resolve the core `Enum` spelling (an
// undefined type degrades the scrutinee the oracle's InvalidType way), so
// the covering pins below declare a local open `Enum` class — the
// container-resolvable face the world decides identically — and the
// core-root arms pin at the world level in
// [`enum_root_world_face_decides_without_being_closed`].

/// An `Enum`-typed scrutinee is decidable but not exhaustiveness-closed:
/// a switch expression whose cases enumerate one enum's constants leaves
/// the root's own space as the witness, and the covering error reports at
/// the `switch` keyword (oracle 3.13.1, probe p4 `g`:
/// `non_exhaustive_switch_expression`, pattern `Enum()`).
#[test]
fn enum_root_expression_switch_reports_non_exhaustive() {
    let source = "\
class Enum {}
enum A { a, b }
Object f(Enum e) => switch (e) { A.a => 1, A.b => 2 };
";
    let report = unit_report(source);
    one_at(
        &report,
        "non_exhaustive_switch_expression",
        at(source, "switch"),
        source,
    );
}

/// A wildcard case covers the `Enum` root's remainder: the covering error
/// stays silent (oracle 3.13.1, probe pf `f` — clean).
#[test]
fn enum_root_wildcard_covers() {
    let source = "\
class Enum {}
enum A { a, b }
Object f(Enum e) => switch (e) { A.a => 1, A.b => 2, _ => 3 };
";
    assert_eq!(
        count_code(&unit_report(source), "non_exhaustive_switch_expression"),
        0
    );
}

/// The root's own object pattern covers the same remainder (oracle 3.13.1,
/// probe pf `g` — clean): the split over the `Enum` root never enumerates
/// enum declarations as coverable subtypes, so the root-pattern row is the
/// total cover.
#[test]
fn enum_root_object_pattern_covers() {
    let source = "\
class Enum {}
enum A { a, b }
Object f(Enum e) => switch (e) { A.a => 1, A.b => 2, Enum() => 3 };
";
    assert_eq!(
        count_code(&unit_report(source), "non_exhaustive_switch_expression"),
        0
    );
}

/// Statement switches never demand exhaustiveness of the `Enum` root (it
/// is not a closed set): with a default the covering face stays silent
/// (oracle 3.13.1, probe p5 `f` — clean).
#[test]
fn enum_root_statement_switch_with_default_stays_silent() {
    let source = "\
class Enum {}
enum A { a, b }
Object f(Enum e) {
  switch (e) {
    case A.a:
      return 1;
    case A.b:
      return 2;
    default:
      return 3;
  }
}
";
    let report = unit_report(source);
    assert_eq!(count_code(&report, "non_exhaustive_switch_statement"), 0);
    assert_eq!(count_code(&report, "dead_code"), 0);
}

/// Without a default the statement face still reports no covering row —
/// the oracle's only diagnostic there is the flow's body-completion face
/// (oracle 3.13.1, probe p4 `f`).
#[test]
fn enum_root_statement_switch_without_default_stays_silent() {
    let source = "\
class Enum {}
enum A { a, b }
Object f(Enum e) {
  switch (e) {
    case A.a:
      return 1;
    case A.b:
      return 2;
  }
}
";
    let report = unit_report(source);
    assert_eq!(count_code(&report, "non_exhaustive_switch_statement"), 0);
}

/// The dart:core `Enum` root's world-level arms (no diagnose rig resolves
/// the core spelling, so the faces pin directly on a bare [`NameWorld`]):
/// the root is decidable for the covering face ([`NameWorld::name_resolvable`]
/// — the arm that keeps an `Enum`-scrutinee expression switch from
/// suppressing as a blind spot), it is NOT exhaustiveness-closed
/// (`is_always_exhaustive` false — a statement switch over the root
/// demands nothing and the covering witness stays the root itself), and a
/// member enum's constant may overlap the root
/// ([`NameWorld::can_be_subtype_of`] — the disjointness gate's
/// never-match answer for `case AstNodeTag.a` over an `Enum` scrutinee).
#[test]
fn enum_root_world_face_decides_without_being_closed() {
    use crate::member_face::FINAL_PRIMITIVES;
    use crate::switch_tracker::{CORE_SUPERTYPES, NameTy, NameWorld, is_always_exhaustive};
    use dartq_types::ExhaustivenessWorld as _;
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let index = ElementIndex::default();
    let world = NameWorld {
        index: &index,
        file,
        strings: &strings,
        scope: None,
        resolved: std::cell::RefCell::default(),
        bool_id: strings.intern("bool"),
        object_id: strings.intern("Object"),
        enum_id: strings.intern("Enum"),
        primitive_ids: FINAL_PRIMITIVES.map(|p| strings.intern(p)),
        core_supertypes: CORE_SUPERTYPES.map(|(b, p)| (strings.intern(b), strings.intern(p))),
        unknowns: std::cell::Cell::new(0),
    };
    let enum_root = strings.intern("Enum");
    assert!(world.name_resolvable(enum_root));
    assert!(!is_always_exhaustive(
        &world,
        NameTy::Named(enum_root, false)
    ));
    assert!(!is_always_exhaustive(
        &world,
        NameTy::Named(enum_root, true)
    ));
    // The root's split enumerates nothing: enum declarations are never
    // coverable subtypes of the root, so the covering walk's witness over
    // an uncovered `Enum` scrutinee is the root's own space.
    assert!(world.subtypes(&NameTy::Named(enum_root, false)).is_empty());
    // `AstNodeTag.a` may inhabit an `Enum` slot: the never-match gate
    // answers overlap.
    assert!(world.can_be_subtype_of(
        &NameTy::EnumValue(strings.intern("AstNodeTag"), strings.intern("a")),
        &NameTy::Named(enum_root, false),
    ));
    // And the opaque-parameter corner stays overlap: a raw parameter head
    // is the world's blind spot, never a proven disjoint pair.
    assert!(world.can_be_subtype_of(
        &NameTy::EnumValue(strings.intern("AstNodeTag"), strings.intern("a")),
        &NameTy::Named(strings.intern("T"), false),
    ));
}
