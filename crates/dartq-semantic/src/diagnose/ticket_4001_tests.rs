//! #4001 — `check_clause_element_kinds` treated a clause head whose
//! declaring file is NOT in the element index (the names-only `dart:core`
//! stub face) as unresolved and emitted the clause code. The layer-level
//! contract: an unindexed declaring file IS the names-only face — the head
//! resolved to a real top-level declaration, so the arms answer
//! clause-legal (upstream's `_resolveType` always has the full element and
//! its switch stays silent on the class face,
//! `pkg/analyzer/lib/src/dart/resolver/resolution_visitor.dart:1186-1192`)
//! — while a head that IS indexed and is not a class still reports.
//!
//! Both namespace arms carry the contract: the scoped
//! `ScopedHit::Found(NamespaceEntry::Decl(owner))` arm (pinned below) and
//! the mirrored prefixed `Lookup::Found(Decl(owner))` arm (same fold; the
//! single-file rig has no prefixed import into an unindexed library, so
//! the prefixed face is exercised at the CLI layer).
//!
//! Known under-report residual (deliberate, recorded here so it is never a
//! silent drop): the names-only face carries no declaration kinds, so a
//! stub face name that is NOT a class upstream — `dart:core`'s
//! `Comparator` typedef, `implements Comparator<int>` — now stays silent
//! where the oracle emits `implements_non_class`. Lifting it needs the SDK
//! face registration (the engine's `SDK_LIBRARY_FACES` posture) to record
//! declaration kinds, a separate lane; before this fix the same shape
//! emitted for the wrong reason (unresolved rather than a known non-class)
//! and the class face (`implements Future<int>`) diverged instead.

use super::test_rig::{code_names, report_ops};

/// The unindexed-declaring-file face: `Future` resolves through the
/// names-only `dart:core` stub (the element index carries no model for the
/// declaring file), so the implements clause is clause-legal and stays
/// silent — exactly the oracle's posture (probe, oracle 3.13.7: no row on
/// the clause head).
#[test]
fn unindexed_declaring_file_clause_head_is_clause_legal() {
    let report = report_ops(
        r#"
class SubFuture implements Future<int> {
  SubFuture();
}

void f(SubFuture x) {
  x.hashCode;
}
"#,
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"implements_non_class"),
        "a namespace-resolved head whose declaring file is unindexed must not report implements_non_class, got {names:?}"
    );
}

/// The over-silence guard: heads whose declaring file IS indexed and whose
/// kind is not clause-legal still report — a local enum under `implements`
/// and a local mixin under `extends`.
#[test]
fn indexed_non_class_clause_heads_still_report() {
    let report = report_ops(
        r#"
enum Color { red }

mixin Wheel {}

class Paint implements Color {}

class Cart extends Wheel {}
"#,
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"implements_non_class"),
        "an indexed enum head under implements must still report implements_non_class, got {names:?}"
    );
    assert!(
        names.contains(&"extends_non_class"),
        "an indexed mixin head under extends must still report extends_non_class, got {names:?}"
    );
}
