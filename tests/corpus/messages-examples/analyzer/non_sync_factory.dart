// messages-example: analyzer/messages.yaml CompileTimeErrorCode.nonSyncFactory
class C {
  factory C() async {
    return C._();
  }

  C._();
}
