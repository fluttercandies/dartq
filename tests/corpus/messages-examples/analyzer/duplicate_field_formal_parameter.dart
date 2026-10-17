// messages-example: analyzer/messages.yaml CompileTimeErrorCode.duplicateFieldFormalParameter
class C {
  int f;

  C(this.f, this.f);
}
