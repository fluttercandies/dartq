// recovery: extra_code_test.dart/test_parenInMapLiteral
class C {}
final Map v = {
  'a': () => new C(),
  'b': () => new C()),
  'c': () => new C(),
};
