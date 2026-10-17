// messages-example: analyzer/messages.yaml CompileTimeErrorCode.fieldInitializedByMultipleInitializers
class C {
  int f;

  C() : f = 0, f = 1;
}
