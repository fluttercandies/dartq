//! #3628 pins — TOP_MERGE over a container's full direct-supertype
//! closure (extends + implements + with), plus merged member result
//! typing.
//!
//! The oracle's `class_hierarchy.dart` folds every duplicate-head
//! instantiation of a supertype through `topMerge` before inference reads
//! it: `class D0 extends A<dynamic> implements B<Object?>` (with
//! `B<T> extends A<T>`) merges `A<dynamic>` and `A<Object?>` into
//! `A<Object?>`, so `deconstruct(this)` binds `x: Object?` — the oracle
//! reports `unchecked_use_of_nullable_value` on `x.foo` and
//! `argument_type_not_assignable` on `takesObject(x)`. dartq's
//! `super_instantiation` climbed only the `extends` spine, so the
//! `B<Object?>` instantiation was never merged and the receiver's
//! superinterface argument resolved to `dynamic` — the family stayed
//! silent.
//!
//! Three faces landed together: `top_merge_ty` gained the NORM pre-pass
//! and structural Function/Record arms (Face C, `overrides.rs`);
//! `super_instantiation` walks the full clause closure and folds
//! duplicate heads through it (Face A, `invocation.rs`), with the
//! `infer_gather` non-generic-receiver drop fixed alongside; and member
//! result typing routes through the merge machinery (`resolved_member` /
//! `merged_member_entry`) so an inherited `member()` answer is the
//! merged signature, not the first-found face (Face B).

use super::test_rig::{code_names, report_ops};

/// Interfaces face, `extends ... implements` (the
/// `top_merge_interfaces_direct_error_test` shape): `D0` merges
/// `A<dynamic>` and `A<Object?>` into `A<Object?>`, so `x: Object?` —
/// `x.foo` is an unchecked nullable access and `takesObject(x)` is not
/// assignable.
///
/// Landed (#3882-M1): `super_instantiation`'s identity instantiation now
/// climbs with the EMPTY identity for a NON-generic container (the
/// `params_of(container)?` early return on an empty param list starved
/// the whole merge), so the downward `deconstruct(this)` inference
/// reaches the duplicate-head topMerge.
#[test]
fn interfaces_direct_merged_object_question_reports() {
    let report = report_ops(
        r#"
class A<T> {}
class B<T> extends A<T> {}
T deconstruct<T>(A<T> x) => throw "unreachable";
void takesObject(Object x) {}

class D0 extends A<dynamic> implements B<Object?> {
  void test() {
    var x = deconstruct(this);
    x.foo;
    x.toString;
    takesObject(x);
  }
}
"#,
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"unchecked_use_of_nullable_value"),
        "expected unchecked_use_of_nullable_value on x.foo, got {names:?}"
    );
    assert!(
        names.contains(&"argument_type_not_assignable"),
        "expected argument_type_not_assignable on takesObject(x), got {names:?}"
    );
}

/// Interfaces face, `extends ... with` (the
/// `top_merge_interfaces_mixin_error_test` shape): the mixin's
/// `implements A<T>` edge contributes the second `A` instantiation, so
/// `with M<Object?>` merges exactly like `implements B<Object?>`.
///
/// Landed with the direct arm (#3882-M1 — the non-generic container's
/// empty identity instantiation).
#[test]
fn interfaces_mixin_merged_object_question_reports() {
    let report = report_ops(
        r#"
class A<T> {}
mixin M<T> implements A<T> {}
T deconstruct<T>(A<T> x) => throw "unreachable";
void takesObject(Object x) {}

class D0 extends A<dynamic> with M<Object?> {
  void test() {
    var x = deconstruct(this);
    x.foo;
    x.toString;
    takesObject(x);
  }
}
"#,
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"unchecked_use_of_nullable_value"),
        "expected unchecked_use_of_nullable_value on x.foo, got {names:?}"
    );
    assert!(
        names.contains(&"argument_type_not_assignable"),
        "expected argument_type_not_assignable on takesObject(x), got {names:?}"
    );
}

/// Members face (the `top_merge_members_direct_error_test` shape): the
/// inherited `member()` answers the merged signature —
/// `T member()` on `A<dynamic>` and `T member()` on `B<Object?>` merge
/// to `Object? member()`, so the call's result is `Object?` (not the
/// first-found `dynamic`).
///
/// The oracle's `takesObject(Object)` formal reads here as `takesNum`:
/// the rig's `dart:core` stub registers `Object` as a name only, so the
/// `Object` formal head resolves to no container and the member-send
/// `unbound_formal` gate suppresses the row (invocation.rs's raw-generic
/// formal erasure latch). `num` is a real declared non-top head — the
/// same `Object?`-source rejection the oracle reports.
#[test]
fn members_direct_merged_result_reports() {
    let report = report_ops(
        r#"
class A<T> {
  T member() => throw "unreachable";
}
class B<T> {
  T member() => throw "unreachable";
}
void takesNum(num x) {}

class D0 extends A<dynamic> implements B<Object?> {
  void test() {
    var self = this;
    var x = self.member();
    x.foo;
    x.toString;
    takesNum(x);
  }
}
"#,
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"unchecked_use_of_nullable_value"),
        "expected unchecked_use_of_nullable_value on x.foo, got {names:?}"
    );
    assert!(
        names.contains(&"argument_type_not_assignable"),
        "expected argument_type_not_assignable on takesNum(x), got {names:?}"
    );
}

/// Members face through a mixin clause (the
/// `top_merge_members_mixin_error_test` shape): `A<dynamic>`'s
/// `T member()` vs the mixin application's `Object? member()` merge to
/// `Object? member()`. `takesNum` stands in for the oracle's
/// `takesObject(Object)` — the same rig's `Object` suppression as the
/// direct arm.
#[test]
fn members_mixin_merged_result_reports() {
    let report = report_ops(
        r#"
class A<T> {
  T member() => throw "unreachable";
}
mixin M<T> {
  T member() => throw "unreachable";
}
void takesNum(num x) {}

class D0 extends A<dynamic> with M<Object?> {
  void test() {
    var self = this;
    var x = self.member();
    x.foo;
    x.toString;
    takesNum(x);
  }
}
"#,
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"unchecked_use_of_nullable_value"),
        "expected unchecked_use_of_nullable_value on x.foo, got {names:?}"
    );
    assert!(
        names.contains(&"argument_type_not_assignable"),
        "expected argument_type_not_assignable on takesObject(x), got {names:?}"
    );
}

/// Override face (the `top_merge_members_override_error_test` shape):
/// `D0 extends A<dynamic>` declares `Object? member()` — the override's
/// own return wins the merge against the inherited `dynamic member()`
/// face, so the call result is `Object?`. `takesNum` stands in for the
/// oracle's `takesObject(Object)` — the same rig's `Object` suppression
/// as the direct arm.
#[test]
fn members_override_merged_result_reports() {
    let report = report_ops(
        r#"
class A<T> {
  T member() => throw "unreachable";
}
void takesNum(num x) {}

class D0 extends A<dynamic> {
  Object? member() => throw "unreachable";
  void test() {
    var self = this;
    var x = self.member();
    x.foo;
    x.toString;
    takesNum(x);
  }
}
"#,
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"unchecked_use_of_nullable_value"),
        "expected unchecked_use_of_nullable_value on x.foo, got {names:?}"
    );
    assert!(
        names.contains(&"argument_type_not_assignable"),
        "expected argument_type_not_assignable on takesNum(x), got {names:?}"
    );
}

/// Control: `D7 extends A<dynamic> implements B<dynamic>` — TOP_MERGE of
/// identical `dynamic` instantiations keeps `dynamic`, so `x: dynamic`
/// and the oracle stays silent on `x.foo`/`takesObject(x)`. Guards the
/// merge against over-reporting.
#[test]
fn interfaces_direct_dynamic_dynamic_stays_silent() {
    let report = report_ops(
        r#"
class A<T> {}
class B<T> extends A<T> {}
T deconstruct<T>(A<T> x) => throw "unreachable";
void takesObject(Object x) {}

class D7 extends A<dynamic> implements B<dynamic> {
  void test() {
    var x = deconstruct(this);
    x.foo;
    x.toString;
    takesObject(x);
  }
}
"#,
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"unchecked_use_of_nullable_value"),
        "x.foo must stay silent on dynamic, got {names:?}"
    );
    assert!(
        !names.contains(&"argument_type_not_assignable"),
        "takesObject(x) must stay silent on dynamic, got {names:?}"
    );
}

/// Void merge (the `D6` rows): `A<void>` merged with `B<void>` yields
/// `A<void>`, so `x: void` — every use of the value is
/// `use_of_void_result` (the oracle reports three rows on this shape).
///
/// Landed with the direct arm (#3882-M1 — the non-generic container's
/// empty identity instantiation).
#[test]
fn interfaces_direct_void_void_reports_void_use() {
    let report = report_ops(
        r#"
class A<T> {}
class B<T> extends A<T> {}
T deconstruct<T>(A<T> x) => throw "unreachable";
void takesObject(Object x) {}

class D6 extends A<void> implements B<void> {
  void test() {
    var x = deconstruct(this);
    x.foo;
    x.toString;
    takesObject(x);
  }
}
"#,
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"use_of_void_result"),
        "expected use_of_void_result on uses of x, got {names:?}"
    );
}
