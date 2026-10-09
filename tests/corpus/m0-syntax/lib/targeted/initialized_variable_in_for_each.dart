// Targets initialized_variable_in_for_each: the loop variable in a for-each
// loop can't be initialized.
void f(List<int> l) {
  for (var i = 0 in l) {}
}
