// Targets mixin_of_non_class: a mixin application whose superclass is not
// a class. Verbatim mirror of the regression-nail source (oracle 3.13.1
// rows pinned there; co-fires duplicate_definition, m1, and expected_token
// / missing_const_final_var_or_type, syntax tier).
inline class Class {}
inline class Class = Object with Mixin;
