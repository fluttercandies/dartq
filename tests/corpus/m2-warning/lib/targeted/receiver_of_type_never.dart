// Targets receiver_of_type_never: a member access on a receiver of static
// type Never (property_element_resolver). The never-receiver expression is
// the body's last statement — anything after it would co-trigger the
// universe's dead_code.
Never f() => throw Object();

void main() {
  f().foo();
}
