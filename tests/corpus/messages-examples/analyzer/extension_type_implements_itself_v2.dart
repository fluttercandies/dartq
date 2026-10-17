// messages-example: analyzer/messages.yaml CompileTimeErrorCode.extensionTypeImplementsItself (variant 2)
extension type A(int i) implements B {}

extension type B(int i) implements A {}
