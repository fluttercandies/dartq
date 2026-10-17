// messages-example: analyzer/messages.yaml CompileTimeErrorCode.nonExhaustiveSwitchExpression
enum E { one, two, three }

String f(E e) => switch (e) {
  E.one => 'one',
  E.two => 'two',
};
