//! Ticket #3899 pins — the `supertype_expands_to_type_parameter` clause
//! faces' registration and the super-bounded argument's binder-list
//! display.
//!
//! Two faces:
//!
//! * The alias-expands-to-type-parameter family: the upstream registry
//!   declares FOUR descriptors sharing one machine-format name
//!   (the generated diagnostic table's extends/implements/mixin-of/
//!   mixin-on entries all read `name: 'supertype_expands_to_type_parameter'`
//!   with distinct `uniqueName`s and problemMessages), and the oracle's
//!   machine format spells `SUPERTYPE_EXPANDS_TO_TYPE_PARAMETER` at every
//!   clause position (verified live against
//!   `dart analyze --format=machine` on the four clause shapes — extends,
//!   implements, `with`, and `on`). The emitted code therefore stays ONE;
//!   the dartq variants split for the per-position messages only, mirroring
//!   the `subtype_of_disallowed_type` shared-name family. These pins hold
//!   the registered descriptors; the clause-kind emit wiring is a separate
//!   ticket's consumption.
//! * The super-bounded pass's message face at a super-allowed site: the
//!   oracle renders the INVERTED argument verbatim with its quantifier —
//!   `'Never Function<T extends int>()'` — because the official inversion
//!   keeps a function type's own parameters verbatim
//!   (replace_top_bottom_visitor.dart:56, `typeParameters:
//!   type.typeParameters`) and `getDisplayString` writes the binder list
//!   beside its bounds. The inverted-type context row spells the whole
//!   inverted instantiation the same way. Both texts must keep the binder
//!   list: the written-argument face renders it from the written tree and
//!   the TypeId-level faces render it from the signature's own parameters.

use super::test_rig::report_ops;
use crate::FileReport;
use crate::codes::SemanticCode;
use dartq_diagnostics::{Diagnostic, DiagnosticType};

/// The one `type_argument_not_matching_bounds` row of a fixture.
fn bounds_row(report: &FileReport) -> &Diagnostic {
    report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "type_argument_not_matching_bounds")
        .expect("the bounds violation reports")
}

/// All four clause faces share the one machine-format name — the registry
/// answer mirrors the oracle's machine output, not the four uniqueNames.
#[test]
fn clause_face_codes_share_one_machine_name() {
    for code in [
        SemanticCode::MixinOfTypeAliasExpandsToTypeParameter,
        SemanticCode::ExtendsTypeAliasExpandsToTypeParameter,
        SemanticCode::ImplementsTypeAliasExpandsToTypeParameter,
        SemanticCode::MixinOnTypeAliasExpandsToTypeParameter,
    ] {
        let info = code.code_info();
        assert_eq!(info.name, "supertype_expands_to_type_parameter");
        assert_eq!(info.machine, "SUPERTYPE_EXPANDS_TO_TYPE_PARAMETER");
        assert_eq!(info.dtype, DiagnosticType::CompileTimeError);
        assert!(
            info.has_published_docs,
            "{} carries hasPublishedDocs upstream",
            info.name
        );
    }
}

/// Each clause position spells its own problemMessage — the reason the
/// four descriptors exist at all.
#[test]
fn clause_face_messages_are_verbatim_per_position() {
    assert_eq!(
        SemanticCode::ExtendsTypeAliasExpandsToTypeParameter.message("", ""),
        "A type alias that expands to a type parameter can't be used as a superclass."
    );
    assert_eq!(
        SemanticCode::ImplementsTypeAliasExpandsToTypeParameter.message("", ""),
        "A type alias that expands to a type parameter can't be implemented."
    );
    assert_eq!(
        SemanticCode::MixinOfTypeAliasExpandsToTypeParameter.message("", ""),
        "A type alias that expands to a type parameter can't be mixed in."
    );
    assert_eq!(
        SemanticCode::MixinOnTypeAliasExpandsToTypeParameter.message("", ""),
        "A type alias that expands to a type parameter can't be used as a superclass constraint."
    );
}

/// The correction faces follow the descriptors verbatim: extends and
/// implements carry one, the `with` and `on` descriptors declare none.
#[test]
fn clause_face_corrections_follow_the_descriptors() {
    assert_eq!(
        SemanticCode::ExtendsTypeAliasExpandsToTypeParameter.correction_message(""),
        Some("Try specifying a different superclass, or removing the extends clause.".to_string())
    );
    assert_eq!(
        SemanticCode::ImplementsTypeAliasExpandsToTypeParameter.correction_message(""),
        Some("Try specifying a class or mixin, or removing the list.".to_string())
    );
    assert_eq!(
        SemanticCode::MixinOfTypeAliasExpandsToTypeParameter.correction_message(""),
        None
    );
    assert_eq!(
        SemanticCode::MixinOnTypeAliasExpandsToTypeParameter.correction_message(""),
        None
    );
}

/// The super-bounded row at a super-allowed site (a variable annotation —
/// no clause/constructor-name parent denies the inverted pass): the
/// argument text is the INVERTED written argument with its binder list
/// intact, and the inverted-type context row spells the whole inverted
/// instantiation. The oracle strings (live `dart analyze`):
/// `'Never Function<T extends int>()' doesn't conform to the bound
/// 'void Function<T>()' of the type parameter 'X'.` with context
/// `The inverted type 'C<Never Function<T extends int>()>' is also not
/// regular-bounded, so the type is not well-bounded.`
#[test]
fn super_bounded_argument_and_context_keep_the_binder_list() {
    let fixture = "\
class C<X extends void Function<T>()> {}
C<void Function<T extends int>()>? x;
";
    let report = report_ops(fixture);
    let rows: Vec<&Diagnostic> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "type_argument_not_matching_bounds")
        .collect();
    assert_eq!(rows.len(), 1, "exactly one bounds row: {report:#?}");
    let row = rows[0];
    assert_eq!(
        row.message.to_string(),
        "'Never Function<T extends int>()' doesn't conform to the bound \
         'void Function<T>()' of the type parameter 'X'.",
        "the inverted argument must keep its quantifier: {}",
        row.message
    );
    let context = row
        .context
        .as_ref()
        .expect("the inverted context row rides");
    assert_eq!(context.len(), 1, "one context row: {context:#?}");
    assert_eq!(
        context[0].message.to_string(),
        "The inverted type 'C<Never Function<T extends int>()>' is also not \
         regular-bounded, so the type is not well-bounded.",
        "the inverted type must keep the argument's binder list: {}",
        context[0].message
    );
}

/// Control: a binder-bound ARGUMENT mismatch (a formal count differs) at
/// the same super-allowed site — the oracle reports the inverted argument
/// with its quantifier verbatim
/// (`'Never Function<T>(T)' doesn't conform to the bound
/// 'void Function<T>()' of the type parameter 'X'.` and the context row
/// `The inverted type 'C<Never Function<T>(T)>' is also not
/// regular-bounded, so the type is not well-bounded.`): the argument face
/// carries the binder list on every pass, not just the first.
#[test]
fn regular_pass_argument_keeps_its_written_face() {
    let fixture = "\
class C<X extends void Function<T>()> {}
C<void Function<T>(T)>? x;
";
    let report = report_ops(fixture);
    let rows: Vec<&Diagnostic> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "type_argument_not_matching_bounds")
        .collect();
    assert_eq!(rows.len(), 1, "exactly one bounds row: {report:#?}");
    let row = rows[0];
    assert_eq!(
        row.message.to_string(),
        "'Never Function<T>(T)' doesn't conform to the bound \
         'void Function<T>()' of the type parameter 'X'.",
        "the inverted argument keeps its quantifier: {}",
        row.message
    );
    let context = row
        .context
        .as_ref()
        .expect("the inverted context row rides");
    assert_eq!(context.len(), 1, "one context row: {context:#?}");
    assert_eq!(
        context[0].message.to_string(),
        "The inverted type 'C<Never Function<T>(T)>' is also not \
         regular-bounded, so the type is not well-bounded.",
        "the inverted type keeps the argument's binder list: {}",
        context[0].message
    );
}
