// messages-example: analyzer/messages.yaml CompileTimeErrorCode.constWithNonConstantArgument
class C {
  final int i;

  const C(this.i);
}

C f(int i) => const C(i);
