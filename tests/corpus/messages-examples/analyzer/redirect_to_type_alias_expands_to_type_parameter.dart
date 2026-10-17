// messages-example: analyzer/messages.yaml CompileTimeErrorCode.redirectToTypeAliasExpandsToTypeParameter
class A implements C {}

typedef B<T> = T;

abstract class C {
  factory C() = B<A>;
}
