// Targets not_initialized_non_nullable_instance_field: a non-nullable
// instance field with no initializer that no constructor initializes.
// Oracle message (nail-pinned): "Non-nullable instance field 'z' must be
// initialized."
class C {
  int z;
}
