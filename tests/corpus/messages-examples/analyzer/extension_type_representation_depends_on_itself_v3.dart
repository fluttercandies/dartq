// messages-example: analyzer/messages.yaml CompileTimeErrorCode.extensionTypeRepresentationDependsOnItself (variant 3)
extension type A(List<B> b) {}

extension type B(List<A> a) {}
