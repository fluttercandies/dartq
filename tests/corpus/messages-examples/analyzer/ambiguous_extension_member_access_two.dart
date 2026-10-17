// messages-example: analyzer/messages.yaml CompileTimeErrorCode.ambiguousExtensionMemberAccessTwo
extension E1 on String {
  int get charCount => 1;
}

extension E2 on String {
  int get charCount => 2;
}

void f(String s) {
  print(s.charCount);
}
