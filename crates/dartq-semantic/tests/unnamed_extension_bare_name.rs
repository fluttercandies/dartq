//! Route-parity pins for bare-name resolution inside unnamed extensions
//! (#1701): `extension on T { ... }` bodies must resolve unqualified names
//! exactly like the named twin — upstream treats the two alike on this
//! face (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/resolver/property_element_resolver.dart:2138-
//! 2141, the lexical scope lookup then the implicit-this getter on the
//! on-type; an unnamed declaration still yields an `ExtensionElementImpl`
//! whose `thisType` is the on-type, type_analyzer.dart:1475-1478).
//!
//! Before the face-index latch landed, an unnamed extension's placeholder
//! frame name resolved to nothing, every member walk through it answered
//! `ChainBroken`, and each bare name in the body collapsed into a
//! `Member{""}` suppression — scoped top-level references lost their
//! static type surface and their query-face target.
//!
//! The faces split by their resolution route, and the pins mirror that:
//!
//! * The reference face ([`UnitFold::refs`]) consumes the bare
//!   resolution directly, so the scoped-reference parity pin is the
//!   route's discriminator: pre-latch it reads `Member("")` where the
//!   named twin reads `Decl`.
//! * The diagnostic faces (arity, assignability) re-resolve the callee
//!   through their own arms — a same-named top-level declaration rescues
//!   a scoped call in both twins — so their parity pins are regression
//!   locks rather than discriminators, and the calibration pin keeps
//!   them honest.
//! * An own-member reference carries the placeholder `Member("")` target
//!   on both routes (the container has no name to target): the
//!   own-member arity divergence the named twin does not share is a
//!   separate same-root-cause consumer (the invocation face's
//!   `walk_members` + `member_formals_full` arm), outside the
//!   resolve_bare route this pin set gates.

use std::path::Path;

use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder, Namespaces};
use dartq_scanner::{ScanConfig, scan};
use dartq_semantic::collector::collect;
use dartq_semantic::diagnose::{RefTarget, StrictModes, UnitFold, fold_unit};
use dartq_semantic::library::ElementIndex;
use dartq_semantic::{FileReport, MicroKind};
use dartq_source::{FileTable, StringTable};

/// The synthetic dart:core names these fragments consume (the engine's
/// implicit core import resolves the same spellings; same provenance
/// posture as the diagnose unit tests' SDK faces).
const CORE_FACE: &[&str] = &["Object", "bool", "double", "int", "String"];

/// The named twin of the scoped-reference parity set: a top-level
/// function called under-arity from the extension body.
const TWIN_SCOPED_CALL_NAMED: &str = "\
void f(int x) {}
extension E on int {
  void t() {
    f();
  }
}
";

/// The unnamed twin of [`TWIN_SCOPED_CALL_NAMED`] — the header's name
/// token is the only textual difference.
const TWIN_SCOPED_CALL_UNNAMED: &str = "\
void f(int x) {}
extension on int {
  void t() {
    f();
  }
}
";

/// The named twin of the typed-argument parity set: a top-level function
/// whose declared parameter type the call violates.
const TWIN_ARGUMENT_TYPE_NAMED: &str = "\
class Other {}
void g(Other o) {}
extension E on int {
  void t() {
    g(1);
  }
}
";

/// The unnamed twin of [`TWIN_ARGUMENT_TYPE_NAMED`].
const TWIN_ARGUMENT_TYPE_UNNAMED: &str = "\
class Other {}
void g(Other o) {}
extension on int {
  void t() {
    g(1);
  }
}
";

/// The unnamed own-member shape: the body calls a sibling member of the
/// same extension under-arity.
const OWN_MEMBER_UNNAMED: &str = "\
extension on int {
  void m(int x) {}
  void t() {
    m();
  }
}
";

/// The named twin of [`OWN_MEMBER_UNNAMED`] — the header's name token is
/// the only textual difference.
const OWN_MEMBER_NAMED: &str = "\
extension E on int {
  void m(int x) {}
  void t() {
    m();
  }
}
";

/// Runs `source` through the live pipeline (the corpus posture: language
/// 3.13, `augmentations` off, a standalone library over the synthetic
/// core face) and returns the whole fold — report and reference faces.
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

/// The report's diagnostic rows as `(code name, 1-based line)` pairs in
/// source order. The twin fragments differ only in the header's name
/// token width, so columns normalize away and lines align.
fn rows(report: &FileReport, source: &str) -> Vec<(&'static str, usize)> {
    report
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.name,
                line_of(source, usize::try_from(d.range.offset).unwrap()),
            )
        })
        .collect()
}

/// The fold's published bare references as `(spelling, target)` pairs in
/// parse order — the query face a goto-definition consumer reads. A
/// `Member` target renders with its container's spelling, so the named
/// twin's `Member(E)` never masks the unnamed twin's placeholder shape.
fn ref_targets<'a>(fold: &'a UnitFold, strings: &'a StringTable) -> Vec<(&'a str, String)> {
    fold.refs
        .iter()
        .map(|r| {
            let target = match r.target {
                RefTarget::Decl { .. } => "Decl".to_string(),
                RefTarget::Local => "Local".to_string(),
                RefTarget::TypeParam => "TypeParam".to_string(),
                RefTarget::Prefix => "Prefix".to_string(),
                RefTarget::Member { container } => {
                    format!("Member({})", strings.resolve(container))
                }
                RefTarget::Opaque => "Opaque".to_string(),
                RefTarget::This => "This".to_string(),
                RefTarget::Missing => "Missing".to_string(),
            };
            (strings.resolve(r.name), target)
        })
        .collect()
}

/// The named twin's scoped under-arity call reports the arity row at the
/// call site — the row family the unnamed twin must keep parity on
/// (#1701). This pin also guards the diagnostic parity pins against
/// passing vacuously on an all-silent pair.
#[test]
fn named_twin_scoped_call_reports_the_arity_row() {
    let strings = StringTable::new();
    let fold = unit_fold(TWIN_SCOPED_CALL_NAMED, &strings);
    let got = rows(&fold.report, TWIN_SCOPED_CALL_NAMED);
    assert!(
        got.contains(&("not_enough_positional_arguments", 4)),
        "the named twin must carry not_enough_positional_arguments at the \
         call line, got {got:?}"
    );
}

/// A scoped top-level call resolves with the same rows in an unnamed
/// extension body as in its named twin (route parity, #1701). The one
/// sanctioned divergence is the enclosing member's `unused_element` row:
/// the oracle's `_isPubliclyAccessible` extension arm reads the
/// declaration's `isPrivate` — an unnamed extension answers `name ==
/// null` as private (`unused_local_elements_verifier.dart`,
/// `ExtensionFragmentImpl.isPrivate`), so an uninvoked member of the
/// unnamed twin reports while the named twin's public member stays
/// silent. The pin keeps the full row list, expecting exactly that one
/// extra row at `t`'s declaration.
#[test]
fn unnamed_extension_scoped_call_resolves_like_named_twin() {
    let strings = StringTable::new();
    let named = unit_fold(TWIN_SCOPED_CALL_NAMED, &strings);
    let unnamed = unit_fold(TWIN_SCOPED_CALL_UNNAMED, &strings);
    let mut expected = rows(&named.report, TWIN_SCOPED_CALL_NAMED);
    expected.insert(0, ("unused_element", 3));
    assert_eq!(
        expected,
        rows(&unnamed.report, TWIN_SCOPED_CALL_UNNAMED),
        "the unnamed twin's scoped call must resolve exactly like the named one \
         (plus the oracle's unused_element on the uninvoked enclosing member)"
    );
}

/// A typed top-level call resolves with the same rows in an unnamed
/// extension body as in its named twin: the argument check fires on the
/// scoped declaration's static type instead of collapsing into the
/// placeholder-name suppression (#1701). The `unused_element` extra at
/// `t`'s declaration is the oracle's private-container asymmetry (see the
/// scoped-call pin for the reasoning), not a resolution divergence.
#[test]
fn unnamed_extension_scoped_argument_type_resolves_like_named_twin() {
    let strings = StringTable::new();
    let named = unit_fold(TWIN_ARGUMENT_TYPE_NAMED, &strings);
    let unnamed = unit_fold(TWIN_ARGUMENT_TYPE_UNNAMED, &strings);
    let mut expected = rows(&named.report, TWIN_ARGUMENT_TYPE_NAMED);
    expected.push(("unused_element", 4));
    assert_eq!(
        expected,
        rows(&unnamed.report, TWIN_ARGUMENT_TYPE_UNNAMED),
        "the unnamed twin's typed call must resolve exactly like the named one \
         (plus the oracle's unused_element on the uninvoked enclosing member)"
    );
}

/// A scoped top-level reference carries the same query-face target in an
/// unnamed extension body as in its named twin: `Decl`, not the
/// placeholder `Member("")` suppression (#1701 — the route's
/// discriminator).
#[test]
fn unnamed_extension_scoped_reference_targets_like_named_twin() {
    let strings = StringTable::new();
    let named = unit_fold(TWIN_SCOPED_CALL_NAMED, &strings);
    let unnamed = unit_fold(TWIN_SCOPED_CALL_UNNAMED, &strings);
    assert_eq!(
        ref_targets(&named, &strings),
        ref_targets(&unnamed, &strings),
        "the unnamed twin's scoped reference must target exactly like the named one"
    );
}

/// An own-member reference carries the placeholder `Member("")` target:
/// the face hit suppresses like a named container's member hit, and the
/// container has no name to target. The named twin's `Member(E)` shape is
/// deliberately not asserted here — the on-type member-modeling ticket
/// owns that divergence, and this pin locks only the unnamed route's own
/// contract (#1701).
#[test]
fn unnamed_extension_own_member_reference_carries_the_face_target() {
    let strings = StringTable::new();
    let fold = unit_fold(OWN_MEMBER_UNNAMED, &strings);
    assert_eq!(
        ref_targets(&fold, &strings),
        vec![("m", "Member()".to_string())],
        "the unnamed twin's own-member reference must carry the placeholder member target"
    );
}

/// An own-member under-arity call reports the deficit row in the unnamed
/// twin exactly as the named twin does (#1881): the invocation face routes
/// the frame's application-face member map the same way the named twin's
/// container walk resolves — the placeholder container's own walk always
/// breaks, which is what kept the unnamed twin silent before the route
/// landed. The reference-face target above is untouched (this fixes the
/// diagnostic face, not `resolve_bare`).
#[test]
fn unnamed_extension_own_member_call_reports_the_arity_row() {
    let strings = StringTable::new();
    let named = unit_fold(OWN_MEMBER_NAMED, &strings);
    let unnamed = unit_fold(OWN_MEMBER_UNNAMED, &strings);
    assert!(
        rows(&named.report, OWN_MEMBER_NAMED).contains(&("not_enough_positional_arguments", 4)),
        "the named twin must carry not_enough_positional_arguments at the \
         call line, got {:?}",
        rows(&named.report, OWN_MEMBER_NAMED)
    );
    // The own-member rows align plus the same oracle asymmetry the
    // scoped-call pin documents: the unnamed twin's uninvoked `t` reports
    // `unused_element` (the unnamed extension reads private, so its
    // members are checked like private declarations).
    let mut expected = rows(&named.report, OWN_MEMBER_NAMED);
    expected.insert(0, ("unused_element", 3));
    assert_eq!(
        expected,
        rows(&unnamed.report, OWN_MEMBER_UNNAMED),
        "the unnamed twin's own-member call must report exactly like the named one \
         (plus the oracle's unused_element on the uninvoked enclosing member)"
    );
}
