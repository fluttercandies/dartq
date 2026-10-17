// messages-example: linter/messages.yaml LinterLintCode.unnecessaryToListInSpreads
List<String> toLowercase(List<String> strings) {
  return [...strings.map((String s) => s.toLowerCase()).toList()];
}
