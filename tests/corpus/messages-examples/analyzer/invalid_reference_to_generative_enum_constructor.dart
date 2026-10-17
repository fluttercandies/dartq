// messages-example: analyzer/messages.yaml CompileTimeErrorCode.invalidReferenceToGenerativeEnumConstructor
enum E {
  a(0);

  const E(int x);
}

E f() => const E(2);
