// messages-example: analyzer/messages.yaml CompileTimeErrorCode.invalidImplementationOverride
class A {
  int add(int a) => a;
}

class B extends A {
  int add(num a);
}
