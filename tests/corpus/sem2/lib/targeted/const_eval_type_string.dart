// Targets const_eval_type_string: a property access in a constant
// expression requires a String operand. Mirrors the m1-semantic fixture of
// the same name: the code is emitted on that shared shape but sits outside
// m1_codes.txt, so the sem2 tier gates it. Oracle rows (dart 3.13.1,
// `dart analyze --format=machine`; <file> is this file's path):
// ERROR|COMPILE_TIME_ERROR|CONST_EVAL_TYPE_STRING|<file>|17|11|4|In constant expressions, operands of this operator must be of type 'String'.
// ERROR|COMPILE_TIME_ERROR|CONST_WITH_NON_CONSTANT_ARGUMENT|<file>|17|13|1|Arguments of a constant creation must be constant expressions.
// ERROR|COMPILE_TIME_ERROR|UNDEFINED_IDENTIFIER|<file>|17|13|1|Undefined name 'x'.
class B {
  final l;
  const B(String o) : l = o.length;
}

const y = B(x);
