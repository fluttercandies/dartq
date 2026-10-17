// messages-example: analyzer/messages.yaml CompileTimeErrorCode.noAnnotationConstructorArguments
class C {
  const C();
}

@C
var x = 0;
