// messages-example: analyzer/messages.yaml CompileTimeErrorCode.extensionTypeImplementsRepresentationNotSupertype
extension type A(num i) {}

extension type B(String s) implements A {}
