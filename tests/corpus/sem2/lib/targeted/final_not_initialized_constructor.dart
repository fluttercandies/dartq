// Targets final_not_initialized_constructor (with new_constructor_dot_name
// as a co-target): a representation field left uninitialized by the
// extension-type members below. Verbatim mirror of the regression-nail
// source; oracle messages pinned there: "Constructors declared with the
// 'new' keyword can't use '.' before the constructor name." and "All final
// variables must be initialized, but 'i' isn't."
extension type ET(int i) {
  new.named();
  factory.named();
}
