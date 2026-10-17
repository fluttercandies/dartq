// messages-example: analyzer/messages.yaml CompileTimeErrorCode.enumMixinWithInstanceVariable
mixin M {
  int x = 0;
}

enum E with M { a }
