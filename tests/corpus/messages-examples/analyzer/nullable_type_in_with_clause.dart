// messages-example: analyzer/messages.yaml CompileTimeErrorCode.nullableTypeInWithClause
mixin M {}

class C with M? {}
