// messages-example: analyzer/messages.yaml CompileTimeErrorCode.implicitThisReferenceInInitializer
class C {
  int x;

  C() : x = defaultX;

  int get defaultX => 0;
}
