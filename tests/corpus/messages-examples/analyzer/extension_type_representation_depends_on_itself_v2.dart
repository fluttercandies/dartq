// messages-example: analyzer/messages.yaml CompileTimeErrorCode.extensionTypeRepresentationDependsOnItself (variant 2)
extension type A(B b) {}

extension type B(A a) {}
