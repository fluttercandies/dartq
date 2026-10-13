// Targets non_constant_record_field: a const record pattern whose fields
// are not constant. Verbatim mirror of the regression-nail source; oracle
// message (nail-pinned): "The fields in a const record literal must be
// constants."
f(x) {
  const (_) = x;
}
