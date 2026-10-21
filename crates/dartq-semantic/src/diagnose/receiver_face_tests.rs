//! The pass-2 receiver re-derivation ([`super::receiver_face`]): a
//! promoted head re-types every later chain node, and a trailing
//! null-assert on a chain link applies before the next member resolves.
//!
//! Oracle provenance (`dart analyze` 3.13.1): every shape is silent.

use super::test_rig::{assert_silent, code_names, report_ops, rows};

/// `t is Sub` promotes `t`; `t.element` then resolves on `Sub`, whose
/// own `element` field (`ParamEl`, non-null) is the face `.bound` reads —
/// never the base getter's frozen `El?`.
#[test]
fn promoted_head_retypes_the_chain_continuation() {
    let report = report_ops(
        "class El { int? get bound => null; }\n\
         class ParamEl extends El {}\n\
         class Ty { El? get element => null; }\n\
         class Sub extends Ty { final ParamEl element; Sub(this.element); }\n\
         void f(Ty t) { if (t is Sub) { if (t.element.bound != null) {} } }\n",
    );
    assert_silent(&report);
}

/// The same chain without the promotion keeps reporting: the base
/// getter's `El?` is the receiver of `bound`.
#[test]
fn unpromoted_chain_keeps_the_base_getter_face() {
    let report = report_ops(
        "class El { int? get bound => null; }\n\
         class Ty { El? get element => null; }\n\
         void f(Ty t) { if (t.element.bound != null) {} }\n",
    );
    assert_eq!(
        code_names(&report),
        vec!["unchecked_use_of_nullable_value"],
        "{:?}",
        rows(&report)
    );
}

/// `n.declaredFragment!.element` over a promoted `n`: the link's `!`
/// applies to the re-derived `Fr?` before `element` resolves.
#[test]
fn null_assert_applies_on_a_promoted_chain_link() {
    let report = report_ops(
        "class Fr { int get element => 0; }\n\
         abstract class N {}\n\
         class DI extends N { Fr? get declaredFragment => null; }\n\
         void f(N n) { if (n is DI) { n.declaredFragment!.element; } }\n\
         void g(DI n) { n.declaredFragment!.element; }\n\
         void h(N n) { if (n is DI) { n.declaredFragment?.element; } }\n",
    );
    assert_silent(&report);
}

/// A `!=` after the link is the comparison operator, never a null-assert:
/// the nullable face stays and the next member still reports.
#[test]
fn not_equals_after_a_link_is_no_null_assert() {
    let report = report_ops(
        "class Fr { int get element => 0; }\n\
         abstract class N {}\n\
         class DI extends N { Fr? get declaredFragment => null; }\n\
         void f(N n) { if (n is DI) { if (n.declaredFragment != null) {} n.declaredFragment.element; } }\n",
    );
    assert_eq!(
        code_names(&report),
        vec!["unchecked_use_of_nullable_value"],
        "{:?}",
        rows(&report)
    );
}
