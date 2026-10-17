// messages-example: analyzer/messages.yaml CompileTimeErrorCode.defaultValueInRedirectingFactoryConstructor
class A {
  factory A([int x = 0]) = B;
}

class B implements A {
  B([int x = 1]);
}
