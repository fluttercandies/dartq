// messages-example: analyzer/messages.yaml CompileTimeErrorCode.undefinedOperator
class C {}

C f(C c) => c + 2;
