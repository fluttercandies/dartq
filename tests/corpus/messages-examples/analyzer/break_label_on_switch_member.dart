// messages-example: analyzer/messages.yaml CompileTimeErrorCode.breakLabelOnSwitchMember
void f(int i) {
  switch (i) {
    l:
    case 0:
      break;
    case 1:
      break l;
  }
}
