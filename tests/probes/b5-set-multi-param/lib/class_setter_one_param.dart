// Control: a well-formed class-member setter.
//
// Exactly one required positional parameter, so the official rule stays
// silent (`_ensureSetterFormalParameter` returns the parameter list
// unchanged). Any row on this file means probe contamination or a real
// divergence and must be reported, not folded into the B5 verdict.
class C {
  set ok(a) {}
}
