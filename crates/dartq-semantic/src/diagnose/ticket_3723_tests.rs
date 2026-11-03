//! #3723 pins — the `super._f` promoted-receiver latch.
//!
//! A `super._f` read has its own promotion identity upstream: the resolver
//! feeds `flow.propertyGet` a dedicated `SuperPropertyTarget` — a version
//! chain separate from `this`'s (`_getSsaNode` answers
//! `helper._superVersion`, upstream Dart SDK source,
//! `pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart:5946-5958`
//! and `pkg/analyzer/lib/src/dart/resolver/property_element_resolver.dart:2063-2069`)
//! — so a private final field genuinely promotes through `super`, and the
//! member merge arms `promoted_recv_reads` for `Receiver::Super` exactly
//! like the `Receiver::This` arm. The eligibility gate is the same shared
//! `isPropertyPromotable` verdict (`addField`,
//! `pkg/_fe_analyzer_shared/lib/src/field_promotability.dart:192-224`):
//! private + field + final + not external + no same-named conflict — a
//! public or non-final `super` read keeps its unchecked verdict.
//!
//! Fixtures declare their own member-bearing class (`N`) so the pin does
//! not depend on the shared prelude's stub surface.

use super::FileReport;
use super::test_rig::{assert_silent, report_ops, report_ops_augment, rows};

/// Every `unchecked_use_of_nullable_value` message in report order.
fn unchecked_messages(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == "unchecked_use_of_nullable_value")
        .map(|row| row.message.to_string())
        .collect()
}

/// The member-bearing receiver class every fixture uses: a single instance
/// method, so `m()` on a non-null receiver is clean and on a nullable one
/// is the unchecked-dereference row.
const N_CLASS: &str = "class N { void m() {} }\n";

/// A private final field read through `super` promotes under its own null
/// guard: `super._n.m()` inside `if (super._n != null)` is silent.
///
/// Oracle (`super_this_distinction_error_test`'s super-arm twins): the
/// `(super, _n)` property chain promotes independently of `this`'s, so the
/// guarded member access resolves on the promoted non-null face.
#[test]
fn super_private_final_field_promotes() {
    let report = report_ops(&format!(
        "{N_CLASS}class Base {{\n  final N? _n;\n  Base(this._n);\n}}\n\
         class Derived extends Base {{\n  Derived(super._n);\n  void t() {{\n\
         if (super._n != null) {{\n      super._n.m();\n    }}\n  }}\n}}\n"
    ));
    assert_silent(&report);
}

/// The field-read face beside the invocation: `super._n.isEven`-shaped
/// member reads under the guard stay silent too (the promoted receiver
/// feeds the read verdict exactly like the send verdict).
#[test]
fn super_private_final_field_read_promotes() {
    let report = report_ops(&format!(
        "{N_CLASS}class Base {{\n  final N? _n;\n  Base(this._n);\n}}\n\
         class Derived extends Base {{\n  Derived(super._n);\n  void t() {{\n\
         if (super._n != null) {{\n      super._n.m;\n    }}\n  }}\n}}\n"
    ));
    assert_silent(&report);
}

/// A public `super` field is never promotable (the official `addField`
/// `isNotPrivate` arm): the member read under the null check keeps the
/// nullable receiver and reports.
#[test]
fn super_public_field_is_not_promotable() {
    let report = report_ops(&format!(
        "{N_CLASS}class Base {{\n  N? pub;\n  Base(this.pub);\n}}\n\
         class Derived extends Base {{\n  Derived(super.pub);\n  void t() {{\n\
         if (super.pub != null) {{\n      super.pub.m();\n    }}\n  }}\n}}\n"
    ));
    assert_eq!(
        unchecked_messages(&report).len(),
        1,
        "a public super field must not promote: {:#?}",
        rows(&report)
    );
}

/// A private but non-final `super` field is not promotable (the official
/// `addField` `isNotFinal` arm): the member read reports.
#[test]
fn super_non_final_private_field_is_not_promotable() {
    let report = report_ops(&format!(
        "{N_CLASS}class Base {{\n  N? _n;\n  Base(this._n);\n}}\n\
         class Derived extends Base {{\n  Derived(super._n);\n  void t() {{\n\
         if (super._n != null) {{\n      super._n.m();\n    }}\n  }}\n}}\n"
    ));
    assert_eq!(
        unchecked_messages(&report).len(),
        1,
        "a non-final private super field must not promote: {:#?}",
        rows(&report)
    );
}

/// The superclass living in a second unit of the same library keeps the
/// promotion: `_n` stays library-private (the `super` access resolves
/// through the declaring library's name), the field is final, and the
/// guarded read is silent.
#[test]
fn super_field_promotes_across_units() {
    let report = report_ops_augment(
        "class N { void m() {} }\n\
         class Base {\n  final N? _n;\n  Base(this._n);\n}\n",
        "class Derived extends Base {\n  Derived(super._n);\n  void t() {\n\
         if (super._n != null) {\n      super._n.m();\n    }\n  }\n}\n",
    );
    assert_silent(&report);
}
