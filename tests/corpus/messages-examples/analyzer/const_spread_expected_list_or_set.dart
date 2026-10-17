// messages-example: analyzer/messages.yaml CompileTimeErrorCode.constSpreadExpectedListOrSet
const dynamic list1 = 42;
const List<int> list2 = [...list1];
