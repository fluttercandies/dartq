// messages-example: analyzer/messages.yaml CompileTimeErrorCode.constSpreadExpectedMap
const dynamic map1 = 42;
const Map<String, int> map2 = {...map1};
