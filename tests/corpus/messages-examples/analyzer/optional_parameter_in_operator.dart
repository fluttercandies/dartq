// messages-example: analyzer/messages.yaml CompileTimeErrorCode.optionalParameterInOperator
class C {
  C operator +([C? other]) => this;
}
