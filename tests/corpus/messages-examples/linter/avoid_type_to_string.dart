// messages-example: linter/messages.yaml LinterLintCode.avoidTypeToString
bool isC(Object o) => o.runtimeType.toString() == 'C';

class C {}
