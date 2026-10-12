// Targets const_eval_type_num_string: an operand of binary '+' in a
// constant expression evaluates to neither a num nor a String. The operand
// is declared `dynamic` so no undefined-operator static row co-fires.
//
// Oracle rows (dart 3.13.1, `dart analyze --format=machine`; <file> is this
// file's path):
// ERROR|COMPILE_TIME_ERROR|CONST_EVAL_TYPE_NUM_STRING|<file>|9|11|7|In constant expressions, operands of this operator must be of type 'num' or 'String'.
const dynamic b = true;
const x = 'a' + b;
