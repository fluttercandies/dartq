// messages-example: analyzer/messages.yaml CompileTimeErrorCode.extensionConflictingStaticAndInstance
extension E on Object {
  int get a => 0;
  static int a() => 0;
}
