// messages-example: analyzer/messages.yaml CompileTimeErrorCode.externalFieldConstructorInitializer
class C {
  external int x;
  C() : x = 0;
}
