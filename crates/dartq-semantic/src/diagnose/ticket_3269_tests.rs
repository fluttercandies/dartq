//! The #3269 pins — the #3252 covariant-field residual: the capture chain
//! #3330 landed ([`MemberEntry::covariant`] off the group's `covariant`
//! keyword) plus the faces that read the mark. The ticket's audit question
//! was whether the bit is populated and whether the INDUCED setter's
//! parameter carries it — both ride one column: the collected field entry
//! is the induced-setter face's single source.
//!
//! Upstream correspondence (upstream Dart SDK source, `pkg/analyzer`):
//! - `pkg/analyzer/lib/src/summary2/element_builder.dart:504`
//!   (`valueFragment.isExplicitlyCovariant = fieldFragment
//!   .isExplicitlyCovariant`) — a `covariant` field's mark rides its
//!   INDUCED setter's parameter, which the override faces read off the
//!   member;
//! - the group latch is per declaration (`beginFields` re-latches the
//!   keyword, the `end*Fields` seams clear it), so a marked group never
//!   bleeds into the next one;
//! - the summary payload carries the column
//!   (`PersistedMemberEntry.covariant`), so a restored element model keeps
//!   the mark.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block: the module
//! stays inside the `diagnose` subtree so `crate::diagnose::test_rig`
//! resolves.

use crate::collector::collect;
use crate::diagnose::FileReport;
use crate::diagnose::test_rig::{report_ops, rows};
use crate::model::FileElements;
use crate::persisted::PersistedElements;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

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

/// The `invalid_override` rows of one report, verbatim.
fn override_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_override")
        .map(|d| d.message.to_string())
        .collect()
}

// —— the capture column ——

/// The group latch resets between declarations: `x`'s marked group keeps
/// the mark, the NEXT group's `y` never inherits it (`begin_fields`
/// re-latches per declaration, the `end*Fields` seams clear the latch).
#[test]
fn covariant_group_latch_resets_between_groups() {
    let strings = StringTable::new();
    let collected = collect_elements("class C { covariant num x; num y; }", &strings);
    let container = collected
        .container(strings.intern("C"))
        .expect("class C collected");
    let x = container
        .members
        .get(&strings.intern("x"))
        .expect("field x");
    assert!(x.covariant, "the marked group keeps the mark");
    let y = container
        .members
        .get(&strings.intern("y"))
        .expect("field y");
    assert!(!y.covariant, "the mark never bleeds into the next group");
}

/// The mark survives the summary payload's round trip: persist → JSON →
/// restore keeps the marked member and its unmarked neighbor exactly (the
/// restored element model is what a resumed session's faces read).
#[test]
fn covariant_bit_survives_the_persist_roundtrip() {
    let strings = StringTable::new();
    let collected = collect_elements("class C { covariant num x; num y; }", &strings);
    let payload = PersistedElements::of(&collected, &strings);
    let bytes = serde_json::to_vec(&payload).expect("serialize");
    let decoded: PersistedElements = serde_json::from_slice(&bytes).expect("deserialize");
    let restored = decoded.into_elements(&strings);
    let before = collected
        .container(strings.intern("C"))
        .expect("class C collected");
    let after = restored
        .container(strings.intern("C"))
        .expect("class C restored");
    for (name, marked) in [("x", true), ("y", false)] {
        let id = strings.intern(name);
        assert_eq!(
            after.members[&id], before.members[&id],
            "{name} round-trips unchanged"
        );
        assert_eq!(
            after.members[&id].covariant, marked,
            "{name} keeps covariant={marked} through the payload"
        );
    }
}

// —— the induced-setter verdict ——

/// The brief's shape end-to-end: `A`'s covariant field induces a setter
/// whose parameter carries the mark, so `B`'s narrowing `set x(int)` skips
/// the parameter pair — silent (`CovariantParametersVerifier`'s covariant
/// skip; the super face's induced parameter reads
/// `fieldFragment.isExplicitlyCovariant`).
#[test]
fn covariant_field_silences_the_narrowing_setter_override() {
    let report = report_ops(
        "class A { covariant num x = 0; }\n\
         class B extends A { void set x(int v) {} }\n",
    );
    assert_eq!(
        rows(&report),
        Vec::<String>::new(),
        "{:?}",
        report.diagnostics
    );
}

/// The negative: without the keyword the induced parameter is plain, and
/// the same narrowing setter draws exactly one `invalid_override` row
/// against the induced face (`B.x` over `A.x`).
#[test]
fn plain_field_draws_the_narrowing_setter_row() {
    let report = report_ops(
        "class A { num x = 0; }\n\
         class B extends A { void set x(int v) {} }\n",
    );
    let found = override_rows(&report);
    assert_eq!(found.len(), 1, "{:?}", report.diagnostics);
    assert!(
        found[0].contains("B.x") && found[0].contains("A.x"),
        "{}",
        found[0]
    );
}
