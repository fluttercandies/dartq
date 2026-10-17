// messages-example: analyzer/messages.yaml CompileTimeErrorCode.initializerForNonExistentField
class C {
  int? y;

  C() : x = 0;
}
