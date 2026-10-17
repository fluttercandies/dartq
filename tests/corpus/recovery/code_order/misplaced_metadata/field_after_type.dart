// recovery: code_order_test.dart/test_field_afterType
class A {
  const A([x]);
}
class B {
  dynamic @A(const A()) x;
}
