use super::test_rig::{code_names, report_ops, rows};

#[test]
fn probe_tearoff_nongeneric() {
    let report = report_ops(
        r#"
int idI(int t) => t;
void main() {
  var f = idI;
  f("s");
}
"#,
    );
    panic!("{:#?}", rows(&report));
}

#[test]
fn probe_direct_generic() {
    let report = report_ops(
        r#"
T id<T>(T t) => t;
void main() {
  id<int>("s");
}
"#,
    );
    panic!("{:#?}", rows(&report));
}

#[test]
fn probe_top_merge_min() {
    let report = report_ops(
        r#"
void takesObject(Object x) {}
class A<T> {}
T deconstruct<T>(A<T> x) => throw "";
class D0 extends A<dynamic> {}
void main() {
  var x = deconstruct(D0());
  takesObject(x);
}
"#,
    );
    panic!("{:#?}", rows(&report));
}

#[test]
fn probe_top_merge_direct() {
    let report = report_ops(
        r#"
void takesObject(Object x) {}
class A<T> {}
class B<T> extends A<T> {}
T deconstruct<T>(A<T> x) => throw "";
class D0 extends A<dynamic> implements B<Object?> {}
void main() {
  Object? y;
  takesObject(y);
}
"#,
    );
    panic!("{:#?}", rows(&report));
}
