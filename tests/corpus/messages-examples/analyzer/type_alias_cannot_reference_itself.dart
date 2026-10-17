// messages-example: analyzer/messages.yaml CompileTimeErrorCode.typeAliasCannotReferenceItself
typedef F = void Function(G);
typedef G = void Function(F);
