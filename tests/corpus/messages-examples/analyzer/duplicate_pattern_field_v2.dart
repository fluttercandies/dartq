// messages-example: analyzer/messages.yaml CompileTimeErrorCode.duplicatePatternField (variant 2)
void f(Object o) {
  switch (o) {
    case C(f: 1, f: 2):
      return;
  }
}

class C {
  int? f;
}
