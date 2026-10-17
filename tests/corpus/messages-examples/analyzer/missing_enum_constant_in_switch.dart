// messages-example: analyzer/messages.yaml StaticWarningCode.missingEnumConstantInSwitch
enum E { e1, e2 }

void f(E e) {
  switch (e) {
    case E.e1:
      break;
  }
}
