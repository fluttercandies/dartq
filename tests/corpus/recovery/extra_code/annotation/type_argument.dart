// recovery: extra_code_test.dart/test_typeArgument
const annotation = null;
class A<E> {}
class C {
  m() => new A<@annotation C>();
}
