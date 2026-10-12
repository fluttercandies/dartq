// Targets const_eval_primitive_equality: an equality operand in a constant
// expression is an instance of a class that overrides '=='.
//
// Oracle rows (dart 3.13.1, `dart analyze --format=machine`; <file> is this
// file's path):
// ERROR|COMPILE_TIME_ERROR|CONST_EVAL_PRIMITIVE_EQUALITY|<file>|13|11|6|In constant expressions, operands of the equality operator must have primitive equality.
class A {
  const A();
  bool operator ==(other) => false;
}

const a = A();
const b = a == 0;
