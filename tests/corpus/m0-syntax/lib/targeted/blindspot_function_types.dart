// Targets named_function_type (syntax-codes.tsv:240): a general function
// type alias can't name the function.
typedef NamedFunctionType = void g();
// Targets missing_function_keyword (TSV:205): a general function type alias
// must use the 'Function' keyword before the parameter list.
typedef MissingFunctionKeyword = int(String s);
// Targets invalid_generic_function_type (TSV:171): a function-typed formal
// parameter can't declare its own type parameters.
void genericFunctionTypedParameter(int g<T>(int x)) {}
// Targets type_arguments_on_type_variable (TSV:277): type arguments on a
// type variable.
class TypeArgsOnTypeVariable<T> {
  T<int>? member;
}
// Targets missing_variable_in_for_each (TSV:223): no loop variable before
// 'in'.
void forEachWithoutVariable(List<int> xs) {
  for (in xs) {
    break;
  }
}
