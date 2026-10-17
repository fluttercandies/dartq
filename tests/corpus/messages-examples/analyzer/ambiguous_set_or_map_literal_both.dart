// messages-example: analyzer/messages.yaml CompileTimeErrorCode.ambiguousSetOrMapLiteralBoth
union(Map<String, String> a, List<String> b, Map<String, String> c) => {
  ...a,
  ...b,
  ...c,
};
