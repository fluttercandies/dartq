// messages-example: analyzer/messages.yaml CompileTimeErrorCode.nonConstantAnnotationConstructor
@C()
void f() {}

class C {
  C();
}
