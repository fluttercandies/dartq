// messages-example: analyzer/messages.yaml CompileTimeErrorCode.fieldInitializingFormalNotAssignable
class C {
  int f;

  C(String this.f);
}
