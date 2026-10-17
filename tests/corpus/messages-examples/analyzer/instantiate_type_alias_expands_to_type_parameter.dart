// messages-example: analyzer/messages.yaml CompileTimeErrorCode.instantiateTypeAliasExpandsToTypeParameter
typedef A<T> = T;

void f() {
  const A<int>();
}
