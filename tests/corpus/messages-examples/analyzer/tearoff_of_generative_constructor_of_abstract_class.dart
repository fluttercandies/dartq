// messages-example: analyzer/messages.yaml CompileTimeErrorCode.tearoffOfGenerativeConstructorOfAbstractClass
abstract class C {
  C();
}

void f() {
  C.new;
}
