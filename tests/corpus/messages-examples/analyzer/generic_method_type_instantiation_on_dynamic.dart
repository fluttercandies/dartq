// messages-example: analyzer/messages.yaml CompileTimeErrorCode.genericMethodTypeInstantiationOnDynamic
void f(dynamic list) {
  list.fold<int>;
}
