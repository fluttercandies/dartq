// messages-example: analyzer/messages.yaml CompileTimeErrorCode.equalKeysInMapPattern
void f(Map<String, int> x) {
  if (x case {'a': 1, 'a': 2}) {}
}
