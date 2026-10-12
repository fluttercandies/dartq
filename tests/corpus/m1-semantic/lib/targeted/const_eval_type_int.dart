// Targets const_eval_type_int: an operand of a bitwise operator in a
// constant expression evaluates to a non-int value. The const variable is
// declared `dynamic` so no undefined-operator static row co-fires.
//
// Oracle rows (dart 3.13.1, `dart analyze --format=machine`; <file> is this
// file's path):
// ERROR|COMPILE_TIME_ERROR|CONST_EVAL_TYPE_INT|<file>|9|11|2|In constant expressions, operands of this operator must be of type 'int'.
const dynamic d = 1.5;
const x = ~d;
