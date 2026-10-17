// messages-example: analyzer/messages.yaml CompileTimeErrorCode.restElementInMapPattern
void f(Map<int, String> x) {
  if (x case {0: _, ...}) {}
}
