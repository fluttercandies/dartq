// Targets non_bool_negation_expression: the operand of '!' is not bool.
// Oracle message (nail-pinned): "A negation operand must have a static
// type of 'bool'."
void f(int i) {
  if (!i) {
    print(i);
  }
}
