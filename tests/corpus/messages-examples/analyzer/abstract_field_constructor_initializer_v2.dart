// messages-example: analyzer/messages.yaml CompileTimeErrorCode.abstractFieldConstructorInitializer (variant 2)
abstract class C {
  abstract int f;

  C() : f = 0;
}
