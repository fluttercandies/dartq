// Targets const_instance_field: a non-static field declared 'const'.
// Oracle message (nail-pinned): "Only static fields can be declared as
// const."
class C {
  const int f = 0;
}
