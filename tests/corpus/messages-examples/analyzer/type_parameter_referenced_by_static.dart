// messages-example: analyzer/messages.yaml CompileTimeErrorCode.typeParameterReferencedByStatic
class C<T> {
  static bool hasType(Object o) => o is T;
}
