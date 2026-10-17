// messages-example: analyzer/messages.yaml CompileTimeErrorCode.getterNotSubtypeSetterTypes
// @dart = 3.8
class C {
  num get x => 0;

  set x(int y) {}
}
