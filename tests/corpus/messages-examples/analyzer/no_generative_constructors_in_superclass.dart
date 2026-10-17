// messages-example: analyzer/messages.yaml CompileTimeErrorCode.noGenerativeConstructorsInSuperclass
class A {
  factory A.none() => throw '';
}

class B extends A {}
