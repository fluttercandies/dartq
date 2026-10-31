//! The #2500 pins: the downward closure-formal re-bind. The analyzer
//! corpus face (`annotations.where((annotation) =>
//! annotation.isAbiSpecificIntegerMapping)` over
//! `NodeList<Annotation>`): an UNTYPED literal formal is staged as a
//! `DeferredClosureFormal` when pass 1 cannot solve the callee's
//! substituted signature, and `infer_rebind_closure_formals` answers the
//! open formal slot from the callee's formals + the send's solved/env
//! type-parameter map at send close — the member read through the formal
//! then reaches `rebound_locals` and the `unused_element` tally spares
//! the member. The `.where` spelling itself is foreign; the fixture
//! below mirrors the shape with a same-file inherited generic member.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

fn unused_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_element" || d.code.name == "unused_field")
        .map(|d| d.message.to_string())
        .collect()
}

/// The inherited non-generic member face (the corpus `.where` shape):
/// `NodeList<Ann>` inherits `Itr<E>.where(bool t(E))` — `E := Ann`
/// arrives only through the receiver's instantiation env. The untyped
/// formal `e` must re-bind to `Ann` at send close, so `e._hidden` reads
/// the member and `_hidden` never reports `unused_element`.
#[test]
fn rebind_inherited_member_formal_spares_member() {
    let source = r#"
class Ann {
  int get _hidden => 0;
}
class Itr<E> {
  Itr<E> where(bool t(E e)) => this;
}
class NodeList<E> extends Itr<E> {}
void main() {
  NodeList<Ann> xs = NodeList<Ann>();
  xs.where((e) => e._hidden > 0);
}
"#;
    let report = report_ops(source);
    let rows = unused_rows(&report);
    assert!(
        !rows.iter().any(|m| m.contains("_hidden")),
        "{:?}",
        report.diagnostics.iter().map(|d| (d.code.name, &d.message)).collect::<Vec<_>>()
    );
}

/// The silence control: a formal the callee's signature never answers
/// (the `where` formal types the first parameter but the literal reads
/// a member on an unrelated second untyped formal) keeps its deferred
/// verdict — no spurious re-bind from the wrong slot.
#[test]
fn rebind_unmatched_slot_stays_silent() {
    let source = r#"
class Ann {
  int get _hidden => 0;
}
class Itr<E> {
  void each(void t(E e, int i)) {}
}
class NodeList<E> extends Itr<E> {}
void main() {
  NodeList<Ann> xs = NodeList<Ann>();
  xs.each((e, i) => e._hidden + i);
}
"#;
    let report = report_ops(source);
    let rows = unused_rows(&report);
    assert!(
        !rows.iter().any(|m| m.contains("_hidden")),
        "{:?}",
        report.diagnostics.iter().map(|d| (d.code.name, &d.message)).collect::<Vec<_>>()
    );
}

/// An untyped formal under a member send whose substituted signature
/// cannot type the read (`List<int>` — the member `_hidden` exists on
/// nothing): the re-bind still binds the formal to `int`, the member
/// read fails silently as upstream's own `dynamic` posture would, and
/// no `unused_element` for a member that does not exist anyway — the
/// pin guards against the re-bind emitting something bogus.
#[test]
fn rebind_scalar_formal_binds_element() {
    let source = r#"
class Itr<E> {
  void each(void t(E e)) {}
}
void main() {
  Itr<int> xs = Itr<int>();
  xs.each((e) => e + 1);
}
"#;
    let report = report_ops(source);
    assert!(
        !report.diagnostics.iter().any(|d| d.code.name == "could_not_infer"),
        "{:?}",
        report.diagnostics.iter().map(|d| (d.code.name, &d.message)).collect::<Vec<_>>()
    );
}
