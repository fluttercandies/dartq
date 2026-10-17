// messages-example: analyzer/messages.yaml CompileTimeErrorCode.missingNamedPatternFieldName
abstract class C {
  int get f;
}

void f(C c) {
  switch (c) {
    case C(:0):
      break;
  }
}
