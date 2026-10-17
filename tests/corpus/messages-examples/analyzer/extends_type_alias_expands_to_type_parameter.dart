// messages-example: analyzer/messages.yaml CompileTimeErrorCode.extendsTypeAliasExpandsToTypeParameter
typedef T<S> = S;

class C extends T<Object> {}
