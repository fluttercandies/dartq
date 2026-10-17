// messages-example: analyzer/messages.yaml CompileTimeErrorCode.continueLabelInvalid
void f(int i) {
  l:
  switch (i) {
    case 0:
      continue l;
  }
}
