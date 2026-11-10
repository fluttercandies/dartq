//! The #3330 pins: the collector sites behind the #3301/#3305 residuals —
//! the field `covariant` bit, the merged-element member scope, the
//! primary-constructor declaring formal's claims, the wildcard-shaped
//! formal gate, and the non-identifier name slot.
//!
//! Upstream decides each face at one place (upstream Dart SDK source,
//! `pkg/analyzer/lib/src`):
//!
//! - `pkg/analyzer/lib/src/summary2/element_builder.dart:504`
//!   (`valueFragment.isExplicitlyCovariant = fieldFragment
//!   .isExplicitlyCovariant`) copies a `covariant` field's mark onto its
//!   INDUCED setter parameter, so the override faces read it off the
//!   member;
//! - `pkg/analyzer/lib/src/error/member_duplicate_definition_verifier.dart`
//!   `_checkClassMembers` keys the member scope on the MERGED element's
//!   first fragment (`_getElementContext(fragment.element.firstFragment)`),
//!   so an augmentation's own member claims into the introduction's scope
//!   (`test_instance_field_field_inAugmentation`: one row at the augment's
//!   member) while an `augment`-prefixed member never checks nor claims
//!   (`fragment.isAugmentation`, :307-309);
//! - the same function's primary-constructor arm (:64-97) walks the
//!   declaring formals and claims each field's getter and — `if
//!   (fieldElement.setter case var setter?)` — setter, `isFinal` deciding
//!   whether the setter exists (`element_builder.dart:2090-2092`);
//! - `pkg/analyzer/lib/src/error/duplicate_definition_verifier.dart`
//!   `checkParameters` shares one scope per list and skips a wildcard
//!   (:56-82, :444-457: `this._` is never a wildcard, a plain `_` is), and
//!   `_DuplicateIdentifierScope.add` bails on a synthetic identifier
//!   (`parser_impl.dart:2229` consumes a typed formal's name only
//!   `if (next.isIdentifier)`).

use super::test_rig::{report_ops, report_ops_wildcards};
use crate::collector::collect;
use crate::model::FileElements;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The `duplicate_definition` rows' (message, offset) pairs, in arrival
/// order.
fn dup_rows(report: &crate::diagnose::FileReport) -> Vec<(String, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "duplicate_definition")
        .map(|d| (d.message.to_string(), d.range.offset))
        .collect()
}

/// Every `duplicate_definition` row's message.
fn dup_messages(report: &crate::diagnose::FileReport) -> Vec<String> {
    dup_rows(report)
        .into_iter()
        .map(|(message, _)| message)
        .collect()
}

/// The unit offset of `needle`'s first hit: the rig prepends
/// [`super::test_rig::CORE_PRELUDE`] to the fixture (pure ASCII, so the byte
/// and UTF-16 offsets agree), and the pins only ever compare two fixture
/// anchors against each other.
fn offset_of(fixture: &str, needle: &str) -> u32 {
    let at = fixture.find(needle).expect("fixture carries the needle");
    u32::try_from(super::test_rig::CORE_PRELUDE.len() + at).expect("fixture fits u32")
}

/// The collected elements of one unit (the collector-side pins read the
/// member model directly — no diagnose pass needed).
fn collect_elements(source: &str, strings: &StringTable) -> FileElements {
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
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
        "scanner diagnostics: {:?}",
        scanned.diagnostics
    );
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    collect(source, &scanned.tokens, features, strings)
}

// —— face 1: the field `covariant` bit ——

/// A `covariant` field group carries the bit on every declarator: the
/// induced setter's parameter is covariant, which the override faces read
/// off the member.
#[test]
fn covariant_field_carries_the_bit() {
    let strings = StringTable::new();
    let collected = collect_elements("class A { covariant num x, y; }", &strings);
    let container = collected
        .container(strings.intern("A"))
        .expect("class A collected");
    for name in ["x", "y"] {
        let member = container
            .members
            .get(&strings.intern(name))
            .unwrap_or_else(|| panic!("member {name} collected"));
        assert!(member.covariant, "{name} keeps the written mark");
        assert_eq!(member.kind, crate::model::MemberKind::Field);
    }
}

/// The unmarked direction: an ordinary field stays false, and a parameter's
/// `covariant` mark never leaks onto a member (it rides `Formal`).
#[test]
fn unmarked_fields_stay_plain() {
    let strings = StringTable::new();
    let collected = collect_elements("class B { num z; void m(covariant num a) {} }", &strings);
    let container = collected
        .container(strings.intern("B"))
        .expect("class B collected");
    let field = container
        .members
        .get(&strings.intern("z"))
        .expect("field z");
    assert!(!field.covariant, "an unmarked field is not covariant");
    let method = container
        .members
        .get(&strings.intern("m"))
        .expect("method m");
    assert!(!method.covariant, "a parameter's mark stays on the formal");
}

// —— face 2: the merged-element member scope ——

/// A non-`augment` member of an `augment` block joins the introduction's
/// member scope: the oracle reports at the augment's own member
/// (`test_instance_field_field_inAugmentation`).
#[test]
fn augment_member_claims_into_the_introduction_scope() {
    let fixture = "class A {\n  int foo = 0;\n}\n\naugment class A {\n  int foo = 42;\n}\n";
    let report = report_ops(fixture);
    let rows = dup_rows(&report);
    assert_eq!(
        rows.iter()
            .map(|(message, _)| message.as_str())
            .collect::<Vec<_>>(),
        vec!["The name 'foo' is already defined."],
        "the augmentation's repeat reports once: {rows:?}"
    );
    let offset = rows[0].1;
    assert!(
        offset > offset_of(fixture, "int foo = 0"),
        "the row anchors at the AUGMENT's member, not the introduction's"
    );
}

/// An `augment`-prefixed member neither checks nor claims, even though the
/// scope it would claim into is now shared with the introduction.
#[test]
fn augment_prefixed_member_stays_silent() {
    let report = report_ops(
        "class A {\n  int foo = 0;\n}\n\naugment class A {\n  augment abstract int foo;\n}\n",
    );
    assert!(
        dup_messages(&report).is_empty(),
        "an `augment` member never collides: {:?}",
        dup_messages(&report)
    );
}

/// Two separate non-augment introductions of one name are two elements with
/// two member scopes — the shared store must never join them
/// (`duplicate_container_members_do_not_cross_collide`).
#[test]
fn same_name_introductions_keep_their_own_scopes() {
    let report = report_ops("class C {\n  int x = 0;\n}\nclass C {\n  int x = 0;\n}\n");
    assert_eq!(
        dup_messages(&report),
        vec!["The name 'C' is already defined.".to_string()],
        "only the container name collides"
    );
}

// —— face 3: the primary-constructor declaring formal's claims ——

/// A `var` declaring formal induces a MUTABLE field: its claims cover the
/// setter too, so a body setter of the same name collides
/// (`fieldElement.setter` exists).
#[test]
fn mutable_declaring_formal_claims_its_setter() {
    let report = report_ops("class A(var int a) {\n  set a(int v) {}\n}\n");
    assert_eq!(
        dup_messages(&report),
        vec!["The name 'a' is already defined.".to_string()],
        "the mutable field's setter claim collides with the written setter"
    );
}

/// A `final` declaring formal induces a getter only: the written setter
/// completes a lone getter into a legal pair.
#[test]
fn final_declaring_formal_leaves_the_setter_free() {
    let report = report_ops("class A(final int a) {\n  set a(int v) {}\n}\n");
    assert!(
        dup_messages(&report).is_empty(),
        "a final field pairs with a written setter: {:?}",
        dup_messages(&report)
    );
}

// —— face 4: the wildcard-shaped formal gate ——

/// Under the wildcard-variables feature a plain `_` formal binds nothing, so
/// `A(int _, this._)` is silent: the plain formal is skipped and the field
/// formal claims an empty scope (`test_parameters_constructor_field_second_wildcard`).
#[test]
fn wildcard_formal_never_blocks_the_field_formal() {
    let report = report_ops("class A {\n  int? _;\n  A(int _, this._);\n}\n");
    assert!(
        dup_messages(&report).is_empty(),
        "the plain wildcard is skipped, the field formal claims alone: {:?}",
        dup_messages(&report)
    );
}

/// Without the feature `_` is an ordinary name, so the same fixture reports
/// at the second formal (`test_parameters_constructor_field_second_wildcard_preWildcards`).
#[test]
fn wildcard_formal_reports_without_the_feature() {
    let report = report_ops_wildcards("class A {\n  int? _;\n  A(int _, this._);\n}\n", false);
    assert_eq!(
        dup_messages(&report),
        vec!["The name '_' is already defined.".to_string()],
        "pre-feature, the second `_` collides"
    );
}

// —— face 5: the non-identifier name slot ——

/// A typed formal's junk name slot is not a name (`parser_impl.dart:2229`
/// consumes one only `if (next.isIdentifier)`): two junk slots never fake a
/// duplicate. The parser's own recovery rows are not this face's business.
#[test]
fn junk_named_formals_never_claim() {
    let report = report_ops("void junk(int 5, int 5) {}\n");
    assert!(
        dup_messages(&report).is_empty(),
        "junk name slots never collide: {:?}",
        dup_messages(&report)
    );
}

/// An FFI varargs signature spells unnamed parameters inside its function
/// type; those slots capture no name, so no `duplicate_definition` row
/// points at a comma or a closer.
#[test]
fn ffi_varargs_parameters_never_claim() {
    let report = report_ops(
        "@Native<Void Function(Pointer<Void>, VarArgs<(Int32, Int32)>)>\n\
         external void ffiSum(Pointer<Void> ptr, [int a, int b]);\n",
    );
    assert!(
        dup_messages(&report).is_empty(),
        "the function type's unnamed parameters claim nothing: {:?}",
        dup_messages(&report)
    );
}
