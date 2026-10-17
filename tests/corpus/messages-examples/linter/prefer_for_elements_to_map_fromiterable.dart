// messages-example: linter/messages.yaml LinterLintCode.preferForElementsToMapFromiterable
void f(Iterable<String> data) {
  Map<String, int>.fromIterable(
    data,
    key: (element) => element,
    value: (element) => element.length,
  );
}
