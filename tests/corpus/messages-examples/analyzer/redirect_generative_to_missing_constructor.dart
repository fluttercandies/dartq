// messages-example: analyzer/messages.yaml CompileTimeErrorCode.redirectGenerativeToMissingConstructor
class C {
  C.a() : this.b();
}
