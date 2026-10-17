// messages-example: analyzer/messages.yaml CompileTimeErrorCode.argumentTypeNotAssignable
String f(String x) => x;
String g(num y) => f(y);
