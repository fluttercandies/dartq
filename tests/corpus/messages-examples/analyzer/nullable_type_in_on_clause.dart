// messages-example: analyzer/messages.yaml CompileTimeErrorCode.nullableTypeInOnClause
class C {}

mixin M on C? {}
