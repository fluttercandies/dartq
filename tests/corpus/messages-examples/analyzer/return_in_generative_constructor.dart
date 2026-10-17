// messages-example: analyzer/messages.yaml CompileTimeErrorCode.returnInGenerativeConstructor
class C {
  C() {
    return this;
  }
}
