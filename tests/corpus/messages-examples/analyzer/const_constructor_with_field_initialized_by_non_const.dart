// messages-example: analyzer/messages.yaml CompileTimeErrorCode.constConstructorWithFieldInitializedByNonConst
String x = '3';

class C {
  final String s = x;
  const C();
}
