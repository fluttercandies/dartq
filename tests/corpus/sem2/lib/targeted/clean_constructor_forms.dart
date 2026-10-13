// Clean face for the constructor-form families (duplicate_constructor,
// invalid_factory_name_not_a_class, factory_constructor_new_name, the
// redirect family, new_with_undefined_constructor_default, and the primary
// constructors in bucket b): well-formed factories, redirects and primary
// constructors; both sides must stay silent.
class A {
  const A();

  const A.named();
}

class B {
  const factory B() = A;

  const factory B.named() = A.named;
}

class P(int i);

extension type ET(int i) {}

enum G { a, b }
