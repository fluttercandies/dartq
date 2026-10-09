// Targets invalid_operator_questionmark_period_for_super: the operator '?.'
// cannot be used with 'super' because 'super' cannot be null.
class C {
  int m() => super?.x;
}
