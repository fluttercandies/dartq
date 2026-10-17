// messages-example: linter/messages.yaml LinterLintCode.preferIterableWheretype
Iterable<Object> f(Iterable<Object> p) => p.where((e) => e is String);
