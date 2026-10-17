// messages-example: analyzer/messages.yaml CompileTimeErrorCode.constWithTypeParameters
class C<T> {
  const C();
}

C<T> newC<T>() => const C<T>();
