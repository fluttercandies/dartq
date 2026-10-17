// messages-example: analyzer/messages.yaml CompileTimeErrorCode.lateFinalFieldWithConstConstructor
class A {
  late final int f;

  const A();
}
